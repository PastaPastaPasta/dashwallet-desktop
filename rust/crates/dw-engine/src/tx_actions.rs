//! Transaction actions and exports (QT-031…033, QT-075, QT-091…093,
//! IOS-031, IOS-034; docs/contracts/m2-engine.md §2.2).
//!
//! Abandon goes through key-wallet's `abandon_transaction` (the transaction
//! and every recorded descendant leave the wallet; the coins they spent are
//! released from the spent set) and is persisted as a sweep batch whose
//! inputs are all released, so wallet.sqlite drops the rows and a restart
//! neither re-credits the change nor re-sends the transaction. The app
//! database keeps the raw transaction so the history still shows it as
//! `Abandoned`. Key-wallet rediscovers the released coins through a filter
//! rescan, which abandon schedules from the lowest funding height.
//!
//! SPV sees no mempool: `in_mempool` is always `None` and abandon is allowed
//! without proof; the host warns that the transaction may still confirm
//! (DESIGN-opus §1.14). If it does, the history shows it confirmed again.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::consensus::encode::{deserialize, serialize_hex};
use dashcore::{BlockHash, OutPoint, Transaction, Txid};
use dw_appdb::{AppDb, LockReason};
use dw_units::{Chain, Unit};
use platform_wallet::changeset::changeset::SweepBatch;
use platform_wallet::changeset::{
    CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
};

use crate::csv_export::{TX_TYPE_COUNT, write_csv};
use crate::history::{
    ChainView, DropCause, DropInfo, HistoryFilter, HistorySort, Labels, TxEntry, TxType,
    WalletHistory, Winner, amounts, beaten_unrecorded, chain_locked, chainlock_beaten, depth_of,
    filtered, gone_parents, instant_locked, orphaned_unmined, records_of, rival_heights, status_of,
};
use crate::rescan_hold::{RescanHold, hold_marks};
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};
use key_wallet::transaction_checking::TransactionContext;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

/// Why `abandon_transaction` / `resend_transaction` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxActionRefusal {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    InMempool,
    NotSentByWallet,
    /// The ChainLock exception (#170 case 14) lacks a coin the payment's
    /// own spend claim, restored at the wallet's load, keeps out of the
    /// wallet: a restart (which no longer restores it) and a rescan give
    /// the evidence.
    RescanFirst,
    /// A later transaction that spends from it is InstantSend-locked, in a
    /// block or finalized: the network holds this one too (#446).
    SettledDescendant,
    /// A stored transaction record of the loaded wallet named didn't read
    /// at its load, so what descends from the payment is unknown (#611). A
    /// restart clears it once the record reads; a record that never does
    /// stays until that wallet is removed and restored.
    RecordUnreadable(WalletId),
    /// It, or a later transaction that spends from it, funds a Platform
    /// transfer, top-up or registration whose asset lock is still tracked
    /// and unsent or unproved: the catch-up resumes it, sending it again.
    PlatformFunding,
    /// The network's data was restored from its backup: funds actions wait
    /// for its first full sync (the money gate, `restore_gate`).
    Restored,
}

impl TxActionRefusal {
    /// The refusal's message: what to do for [`Self::RescanFirst`] and
    /// [`Self::RecordUnreadable`], the refusal's name otherwise.
    pub fn message(&self) -> String {
        match self {
            Self::RescanFirst => {
                "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again."
                    .into()
            }
            Self::RecordUnreadable(wallet) => format!(
                "A transaction record in wallet {wallet} can't be read. Restore that wallet from \
                 its recovery phrase, and wait until it has finished syncing before using Abandon \
                 or Remove Unconfirmed."
            ),
            other => format!("transaction action refused: {other:?}"),
        }
    }
}

/// The dash-qt details fields `TxDetail` lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxDetailExtras {
    pub txid: String,
    pub is_coinbase: bool,
    pub total_credit: u64,
    pub total_debit: Option<u64>,
    /// Some input spends a coin this wallet holds no record of: another
    /// party's, or this wallet's whose funding it has not seen. The debit
    /// cannot tell those apart.
    pub inputs_unresolved: bool,
    pub net: i64,
    pub matures_in: Option<u32>,
    pub in_mempool: Option<bool>,
    pub abandoned: bool,
    pub can_abandon: bool,
    pub can_resend: bool,
    pub dust_locked_outputs: Vec<OutPoint>,
    /// Outputs of this transaction kept out of spending and every balance
    /// while it, or one it was built on, may never confirm (Q100 E1,
    /// `crate::withhold`): shown as pending.
    pub withheld_outputs: Vec<OutPoint>,
    /// It is abandoned and a coin it spent, given back, still waits for
    /// the rescan that checks it (Q100 E2, `crate::rescan_hold`).
    pub checking_spent_coins: bool,
    pub last_announced_at: Option<u64>,
    /// A restore hold exists for it (`restore_gate`).
    pub restore_held: bool,
}

/// One notification row (one per dash-qt record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxNotice {
    pub txid: String,
    pub record_index: u32,
    pub amount: i64,
    pub timestamp: Option<u64>,
    pub tx_type: TxType,
    pub address: Option<String>,
    pub label: Option<String>,
    pub coinjoin_internal: bool,
}

/// A payment this app dropped because the coin it spent came from a
/// transaction that was never valid, once the wallet's own coins it spent
/// are back, until it is dismissed (#766): one per payee (dash-qt record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedNotice {
    pub txid: String,
    pub record_index: u32,
    /// What it paid this payee, without the fee.
    pub amount: u64,
    pub timestamp: Option<u64>,
    pub address: Option<String>,
    pub label: Option<String>,
    pub counterparty: Option<crate::platform::Counterparty>,
}

/// Settings key prefix (wallet scope) of a dropped payment whose notice
/// was dismissed ([`DroppedNotice`]).
const DROP_SEEN_PREFIX: &str = "drop_seen:";

/// How long `resend_transaction` waits for dash-spv to refuse the send
/// before it reports the transaction as handed over.
const RESEND_HANDOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Settings key prefix (wallet scope) of an abandoned transaction; the
/// value is its consensus hex.
const ABANDONED_PREFIX: &str = "abandoned:";
const ABANDON_RELEASE_PREFIX: &str = "abandon_release:";
/// A transaction this app dropped as one that can never confirm, or built
/// on one (#739): a load seeds the walk of what to drop with it, and never
/// restores it from a sent mark.
const DROPPED_PREFIX: &str = "dropped:";

/// The transactions dropped as ones that can never confirm, or built on
/// one, each with why ([`DROPPED_PREFIX`]).
pub(crate) type Dead = BTreeMap<Txid, DropCause>;

#[derive(Clone, Copy)]
pub(crate) enum DeadSource {
    Load,
    Restore,
    Event,
}

/// A [`DROPPED_PREFIX`] mark's value: why its transaction was dropped. A
/// mark only its key names matters to the load's walk; the value is read
/// for what the history shows alone.
#[derive(serde::Serialize, serde::Deserialize)]
struct DropMark {
    cause: DropMarkCause,
    /// The wallet's own inputs, by index, with their values and addresses:
    /// a load lists a payment whose record is gone as sent with them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    inputs: BTreeMap<u32, (u64, String)>,
}

/// A dropped transaction's own inputs, by index ([`DropMark::inputs`]).
pub(crate) type OwnInputs = BTreeMap<u32, (u64, dashcore::Address)>;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum DropMarkCause {
    InvalidSource,
    InvalidClaim,
    Replaced,
}

/// The value of the [`DROPPED_PREFIX`] mark of a transaction dropped for
/// `cause`, which spent the wallet's own `inputs`.
fn drop_mark(cause: DropCause, inputs: &OwnInputs) -> String {
    let cause = match cause {
        DropCause::InvalidClaim => DropMarkCause::InvalidClaim,
        DropCause::InvalidSource => DropMarkCause::InvalidSource,
        DropCause::Replaced => DropMarkCause::Replaced,
    };
    let inputs = (inputs.iter())
        .map(|(n, (value, address))| (*n, (*value, address.to_string())))
        .collect();
    serde_json::to_string(&DropMark { cause, inputs }).unwrap_or_default()
}

/// Why a [`DROPPED_PREFIX`] mark's transaction was dropped: what is built
/// on a never-valid claim for a value that is empty (a mark written before
/// the value said), or does not parse.
fn drop_mark_cause(value: Option<&str>) -> DropCause {
    match value
        .and_then(|v| serde_json::from_str::<DropMark>(v).ok())
        .map(|m| m.cause)
    {
        Some(DropMarkCause::InvalidClaim) => DropCause::InvalidClaim,
        Some(DropMarkCause::Replaced) => DropCause::Replaced,
        _ => DropCause::InvalidSource,
    }
}

/// Whether a [`DROPPED_PREFIX`] mark's value is a never-valid verdict, as
/// the orphan rule reads it (#749, Q81d): an empty value (every one this
/// app wrote is the claim rule's), or a mark naming an invalid claim or
/// source. A replaced transaction's mark is no verdict by its bytes, nor is
/// any other value, which [`drop_mark_cause`] shows as an invalid source.
pub(crate) fn never_valid(value: Option<&str>) -> bool {
    match value {
        Some("") => true,
        Some(v) => matches!(
            serde_json::from_str::<DropMark>(v).map(|m| m.cause),
            Ok(DropMarkCause::InvalidClaim | DropMarkCause::InvalidSource)
        ),
        None => false,
    }
}

/// Settings key prefix (wallet scope) of a transaction dropped as dead
/// ([`NetworkSession::drop_dead_unmined`]), written with its abandoned
/// mark: the coins it spent come back only through the drop's rewind, so
/// an open that finishes its sweep keeps them all stored spent. The value
/// lists the coins it spent that the drop holds for good (`txid:vout`,
/// comma-separated), restored as claims at each load ([`dropped_holds`]).
/// Not `dropped:`, which marks a transaction the claim rule dropped (#739)
/// and holds nothing: neither reads the other's keys.
const DROP_HOLDS_PREFIX: &str = "drop_holds:";

/// Settings key prefix (wallet scope) of what beat a transaction dropped as
/// dead (#749, Q81b), written with its [`DROP_HOLDS_PREFIX`] note: its
/// [`crate::history::DropOrigin`]. Only the held report reads it
/// ([`crate::history::held_awaiting`]); it frees and claims nothing.
const DROP_WINNER_PREFIX: &str = "drop_winner:";

/// Settings key prefix (wallet scope) of a block's spend of a coin the
/// drops hold or contest, by outpoint (#749, Q81b S3): a
/// [`crate::history::SpendNote`], written while key-wallet observes the
/// spend, which it forgets once final. Only the held report reads it, and
/// only once the SPV header chain proves its block final; it frees and
/// claims nothing. A note the header chain contradicts goes, and so does
/// one not final above a fork's height, and every one at a reset of the
/// chain data.
const DROP_SPENT_PREFIX: &str = "drop_spent:";

/// Settings key prefix (wallet scope) of a coin the drops hold that an
/// InstantSend lock of a transaction the wallet holds no copy of may spend
/// (#749, Q81c C1): the lock's txids, comma-separated. key-wallet keeps
/// such locks in memory only, so the note keeps the coin awaiting after a
/// restart. It goes on the coin's final spend, or once no drop holds the
/// coin, and each txid once its transaction is learned and ChainLocked or
/// beaten in a block the header chain proves final, or is dead for good. A
/// note that doesn't read keeps its coin awaiting until the coin's final
/// spend, and one naming no coin keeps every held coin awaiting. Only the
/// held report reads it; it frees and claims nothing.
const DROP_LOCKED_PREFIX: &str = "drop_locked:";

/// Settings key prefix (wallet scope) of a transaction key-wallet swept,
/// for every one of a sweep (#749, Q81d S1): a [`crate::history::SweptBy`],
/// written with the sweep's other marks. What proves an orphan's gone
/// parent dead ([`crate::history::dead_for_good`]); a note that doesn't
/// read is absent. Its block goes at a fork at or below it, a reset of the
/// chain data, or a header naming another block. The note goes once its
/// transaction is stored or live again, with the last orphan's drop naming
/// it when that drop is undone, and at a load once no drop names it and
/// nothing spends its outputs (S6). Only the held report reads it.
const SWEPT_BY_PREFIX: &str = "swept_by:";

/// Settings key prefix (wallet scope) of a transaction key-wallet swept
/// for a winner that funds it: a claim, whose descendants the claim rule
/// judges (#739). A load does not take them as orphaned.
const CLAIM_SWEPT_PREFIX: &str = "claim_swept:";

/// The transactions of `wallet` swept as claims ([`forget_swept`]); a
/// note whose key names no transaction is left out.
pub(crate) fn claims_swept(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<HashSet<Txid>, dw_appdb::AppDbError> {
    let rows = appdb.settings_with_prefix_lossy(&wallet.to_string(), CLAIM_SWEPT_PREFIX)?;
    Ok((rows.into_iter())
        .filter_map(|(key, _)| {
            let txid = Txid::from_str(&key[CLAIM_SWEPT_PREFIX.len()..]).ok();
            if txid.is_none() {
                tracing::warn!(%wallet, "a claim swept note names no transaction");
            }
            txid
        })
        .collect())
}

/// The coins each wallet's drops hold for good, as its open read them.
pub(crate) type DroppedHolds = HashMap<WalletId, BTreeSet<OutPoint>>;

/// A test's fault point on [`dropped_holds`]'s read.
#[cfg(all(test, debug_assertions))]
pub(crate) const DROPPED_HOLDS_FAULT: &str = "tx_actions.dropped_holds";

/// A test's fault point on the held report's read of the drops' notes.
#[cfg(all(test, debug_assertions))]
pub(crate) const HELD_REPORT_FAULT: &str = "tx_actions.held_report";

/// The coins the drops of `wallet` hold for good ([`DROP_HOLDS_PREFIX`]); a
/// note whose value does not parse holds nothing.
pub(crate) fn dropped_holds(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<BTreeSet<OutPoint>, dw_appdb::AppDbError> {
    #[cfg(all(test, debug_assertions))]
    appdb.fault_point(DROPPED_HOLDS_FAULT, dw_appdb::fault::Stage::Before)?;
    let rows = appdb.settings_with_prefix_lossy(&wallet.to_string(), DROP_HOLDS_PREFIX)?;
    Ok((rows.into_iter())
        .filter_map(|(_, value)| value)
        .flat_map(|v| (v.split(',').filter_map(|o| OutPoint::from_str(o).ok())).collect::<Vec<_>>())
        .collect())
}

/// What the held report reads of a wallet's drops (#749): by dropped
/// transaction, the coins its note holds and its origin; the spend notes
/// and the lock notes, by coin; and what proves an orphan's gone parent
/// dead.
#[derive(Debug, Default)]
pub(crate) struct DropNotes {
    pub drops: crate::history::HeldDrops,
    pub spent: BTreeMap<OutPoint, crate::history::SpendNote>,
    pub locked: BTreeMap<OutPoint, BTreeSet<Txid>>,
    /// The coins whose lock note doesn't read: each awaits until its own
    /// final spend (Q81c C1).
    pub locked_unread: BTreeSet<OutPoint>,
    /// A lock note names no coin: every held coin awaits.
    pub locked_astray: bool,
    pub deaths: crate::history::Deaths,
}

/// Whether a load deletes a sweep or claim-sweep note: a note of a transaction
/// key-wallet stores (`stored`) or the history holds live again, and one
/// nothing names. A note is named by an origin's gone parents, a
/// transaction of the history spending its transaction, or a transaction
/// of the drops' chains ([`crate::history::drop_chains`]) spending it, so
/// a chain keeps each note its walk needs.
fn obsolete_sweep_note<'a>(
    notes: &'a DropNotes,
    txs: &'a HashMap<Txid, TxEntry>,
    stored: &'a HashSet<Txid>,
) -> impl Fn(&Txid) -> bool + 'a {
    let named: HashSet<&Txid> = (notes.drops.values())
        .filter_map(|(_, origin)| origin.as_ref())
        .flat_map(|o| &o.gone)
        .collect();
    let chains = crate::history::drop_chains(&notes.drops, &notes.deaths);
    let parents: HashSet<Txid> = (txs.values().flat_map(|e| e.tx.input.iter()))
        .map(|i| i.previous_output)
        .chain(chains.iter().flat_map(|t| notes.deaths.inputs_of(t)))
        .map(|o| o.txid)
        .collect();
    move |t| {
        stored.contains(t)
            || txs.get(t).is_some_and(|e| !e.is_abandoned())
            || (!named.contains(t) && !parents.contains(t))
    }
}

/// The coins whose spends in the history the held report reads the blocks
/// of: those `noted`, and each input of the drops' chains
/// ([`crate::history::drop_chains`]), whose rival's spend
/// [`crate::history::chain_dead_for_good`] can find final.
pub(crate) fn spends_read<'a>(
    noted: impl Iterator<Item = &'a OutPoint>,
    drops: &crate::history::HeldDrops,
    deaths: &crate::history::Deaths,
) -> HashSet<OutPoint> {
    let chains = crate::history::drop_chains(drops, deaths);
    (noted.copied())
        .chain(chains.iter().flat_map(|t| deaths.inputs_of(t)))
        .collect()
}

/// [`DropNotes`] of `wallet`: a note whose key or value does not parse is
/// left out. A drop with no origin note is taken as an older drop
/// ([`crate::history::older_origin`]); one whose origin note doesn't read
/// keeps no origin, and awaits.
pub(crate) fn drop_notes(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<DropNotes, dw_appdb::AppDbError> {
    let scope = wallet.to_string();
    let rows = |prefix: &str| -> Result<Vec<(String, Option<String>)>, dw_appdb::AppDbError> {
        Ok((appdb
            .settings_with_prefix_lossy(&scope, prefix)?
            .into_iter())
        .map(|(key, value)| (key[prefix.len()..].to_owned(), value))
        .collect())
    };
    let txid = |t: &str| Txid::from_str(t).ok();
    let mut notes = DropNotes::default();
    for (t, held) in rows(DROP_HOLDS_PREFIX)? {
        if let (Some(t), Some(held)) = (txid(&t), held) {
            let held = held.split(',').filter_map(|o| OutPoint::from_str(o).ok());
            notes.drops.entry(t).or_default().0 = held.collect();
        }
    }
    let mut unread_origin = BTreeSet::new();
    for (t, origin) in rows(DROP_WINNER_PREFIX)? {
        let Some((t, drop)) = txid(&t).and_then(|t| Some(t).zip(notes.drops.get_mut(&t))) else {
            continue;
        };
        drop.1 = origin.and_then(|o| crate::history::DropOrigin::decode(&o));
        if drop.1.is_none() {
            tracing::warn!(%wallet, txid = %t, "a drop's winner note doesn't read: its held coins wait");
            unread_origin.insert(t);
        }
    }
    for (coin, note) in rows(DROP_SPENT_PREFIX)? {
        let note = note.and_then(|n| crate::history::SpendNote::decode(&n));
        if let (Ok(coin), Some(note)) = (OutPoint::from_str(&coin), note) {
            notes.spent.insert(coin, note);
        }
    }
    for (coin, locks) in rows(DROP_LOCKED_PREFIX)? {
        let Ok(coin) = OutPoint::from_str(&coin) else {
            notes.locked_astray = true;
            continue;
        };
        let locks: Option<BTreeSet<Txid>> = (locks.as_deref())
            .and_then(|l| l.split(',').map(txid).collect::<Option<_>>())
            .filter(|l: &BTreeSet<Txid>| !l.is_empty());
        match locks {
            Some(locks) => {
                notes.locked.insert(coin, locks);
            }
            None => {
                notes.locked_unread.insert(coin);
            }
        }
    }
    let deaths = &mut notes.deaths;
    for (t, value) in rows(DROPPED_PREFIX)? {
        if let Some(t) = txid(&t).filter(|_| never_valid(value.as_deref())) {
            deaths.never_valid.insert(t);
        }
    }
    for (t, value) in rows(SWEPT_BY_PREFIX)? {
        let t = txid(&t);
        if let Some(note) = t
            .zip(value)
            .and_then(|(t, v)| crate::history::SweptBy::decode(&t, &v))
        {
            deaths.swept_by.insert(note.tx.txid(), note);
        }
    }
    for (t, value) in rows(ABANDONED_PREFIX)? {
        let copy = (value.and_then(|v| hex::decode(v).ok()))
            .and_then(|b| deserialize::<Transaction>(&b).ok());
        if let Some((t, tx)) = txid(&t).zip(copy).filter(|(t, tx)| tx.txid() == *t) {
            deaths.copies.insert(t, tx);
        }
    }
    let dropped: BTreeSet<Txid> = notes.drops.keys().copied().collect();
    let deaths = &notes.deaths;
    let copy_of = |t: &Txid| (deaths.copies.get(t)).or(deaths.swept_by.get(t).map(|s| &s.tx));
    let dead = |t: &Txid| dropped.contains(t) || deaths.swept_by.contains_key(t);
    for (t, (_, origin)) in &mut notes.drops {
        if origin.is_none() && !unread_origin.contains(t) {
            *origin = Some(crate::history::older_origin(*t, copy_of, dead));
        }
    }
    Ok(notes)
}

/// Deletes every spend note of `wallet` ([`DROP_SPENT_PREFIX`]), and the
/// winner's block of every sweep note ([`SWEPT_BY_PREFIX`], Q81d S5): a
/// reset of the chain data leaves no header chain to prove them against.
pub(crate) fn forget_spend_notes(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<(), dw_appdb::AppDbError> {
    let scope = wallet.to_string();
    let mut writes: Vec<(String, Option<String>)> = (appdb
        .settings_with_prefix_lossy(&scope, DROP_SPENT_PREFIX)?)
    .into_iter()
    .map(|(key, _)| (key, None))
    .collect();
    for (key, value) in appdb.settings_with_prefix_lossy(&scope, SWEPT_BY_PREFIX)? {
        let note = Txid::from_str(&key[SWEPT_BY_PREFIX.len()..])
            .ok()
            .zip(value)
            .and_then(|(t, v)| crate::history::SweptBy::decode(&t, &v))
            .filter(|n| n.block.is_some());
        if let Some(note) = note {
            let note = crate::history::SweptBy {
                block: None,
                ..note
            };
            writes.push((key, Some(note.encode())));
        }
    }
    if writes.is_empty() {
        return Ok(());
    }
    let writes: Vec<(&str, Option<&str>)> = (writes.iter())
        .map(|(k, v)| (k.as_str(), v.as_deref()))
        .collect();
    appdb.set_settings_durable(&scope, &writes)
}

/// Settings key prefix (wallet scope) of a payment this app signed and
/// handed to the network (#88): the evidence a spend claim is restored
/// from at load. Written before the first broadcast, removed when it was
/// never sent, and dropped once the payment is gone from the history or
/// proven ChainLocked: in the session by its wallet's ChainLock of its own
/// block, or at load by the SPV header chain.
pub(crate) const SENT_PREFIX: &str = "sent:";

/// Tests: the next abandon sweep of a session's wallet store stops right
/// after it is stored, as if the process stopped there.
#[cfg(test)]
pub(crate) mod sweep_stop {
    use std::sync::Mutex;

    static ARMED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

    fn key<T>(store: &T) -> usize {
        std::ptr::from_ref(store) as usize
    }

    pub(crate) fn arm<T>(store: &T) {
        ARMED.lock().unwrap().push(key(store));
    }

    /// Whether a stop is still armed for `store`.
    pub(crate) fn armed<T>(store: &T) -> bool {
        ARMED.lock().unwrap().contains(&key(store))
    }

    pub(crate) fn take<T>(store: &T) -> bool {
        let mut armed = ARMED.lock().unwrap();
        let found = armed.iter().position(|k| *k == key(store));
        found.map(|i| armed.remove(i)).is_some()
    }
}

/// Tests: the load's step fails once for a wallet with marks, in the
/// session of `db_path`.
#[cfg(test)]
pub(crate) mod finish_fault {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static ARMED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(crate) fn arm(db_path: &Path) {
        ARMED.lock().unwrap().push(db_path.to_path_buf());
    }

    pub(crate) fn take(db_path: &Path) -> bool {
        let mut armed = ARMED.lock().unwrap();
        let found = armed.iter().position(|p| p == db_path);
        found.map(|i| armed.remove(i)).is_some()
    }
}

/// Settings key prefix (wallet scope) of the signed transaction of a sent
/// mark, its consensus hex, written and dropped with the mark (#170 case
/// 1). Apart from the mark, so a build that knows no such key still reads
/// the mark; one an earlier build wrote has none.
pub(crate) const SENT_TX_PREFIX: &str = "senttx:";

/// Settings key prefix of a sent mark's inputs: each signed coin's
/// outpoint, value and address, so it is listed when its funding is not.
pub(crate) const SENT_IN_PREFIX: &str = "sentin:";

/// The payments of `wallet` this app sent, each with the wallet's coins it
/// signed and its transaction when the mark holds it ([`SENT_PREFIX`]),
/// and whether every mark was read.
pub(crate) fn load_sent(appdb: &AppDb, wallet: WalletId) -> (Sent, bool) {
    match read_sent(appdb, wallet) {
        (sent, SentRead::Complete) => (sent, true),
        _ => (Sent::new(), false),
    }
}

/// How much of a wallet's sent marks [`read_sent`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SentRead {
    Complete,
    /// The marks read but their saved copies' query failed: a mark written
    /// with its copy is kept with no copy, so its coins stay claimed as a
    /// damaged copy's.
    CoinsOnly,
    Failed,
}

/// The sent marks for a history load: [`load_sent`], except that a failed
/// query of the saved copies keeps the marks that read ([`SentRead::CoinsOnly`]).
pub(crate) fn read_sent(appdb: &AppDb, wallet: WalletId) -> (Sent, SentRead) {
    read_sent_with(appdb, wallet, |_| {})
}

/// History can use partial evidence. Replay needs to retry any failed query.
fn read_replay_sent(appdb: &AppDb, wallet: WalletId) -> dw_appdb::Result<Sent> {
    let mut reads = Ok(());
    let (sent, _) = read_sent_with(appdb, wallet, |error| reads = Err(error));
    reads.map(|()| sent)
}

fn read_sent_with(
    appdb: &AppDb,
    wallet: WalletId,
    mut read_failed: impl FnMut(dw_appdb::AppDbError),
) -> (Sent, SentRead) {
    let scope = wallet.to_string();
    let mut rows = |prefix: &str| match appdb.settings_with_prefix_lossy(&scope, prefix) {
        Ok(rows) => Some(
            (rows.into_iter())
                .filter_map(|(key, value)| {
                    Some((Txid::from_str(key.get(prefix.len()..)?).ok()?, value))
                })
                .collect::<BTreeMap<_, _>>(),
        ),
        Err(e) => {
            tracing::warn!(%wallet, prefix, error = %e, "could not read the sent marks");
            read_failed(e);
            None
        }
    };
    let Some(marks) = rows(SENT_PREFIX) else {
        return (Sent::new(), SentRead::Failed);
    };
    let txs = rows(SENT_TX_PREFIX);
    let read = match txs {
        Some(_) => SentRead::Complete,
        None => SentRead::CoinsOnly,
    };
    let txs = txs.unwrap_or_default();
    let mut inputs = rows(SENT_IN_PREFIX).unwrap_or_default();
    let sent = (marks.into_iter())
        .filter_map(|(txid, coins)| {
            let inputs = (inputs.remove(&txid).flatten())
                .map(|v| parse_inputs(&v))
                .unwrap_or_default();
            // Unread copies: only a mark written with its copy (one with its
            // coins' values, which an earlier build's lacks) counts as kept.
            // When the values' query failed too, no mark does, and only a
            // payment the history lists keeps its coins claimed (each failed
            // query is logged).
            let kept = match read {
                SentRead::CoinsOnly => !inputs.is_empty(),
                _ => txs.contains_key(&txid),
            };
            let tx = (txs.get(&txid).cloned().flatten())
                .and_then(|hex| hex::decode(hex).ok())
                .and_then(|b| deserialize::<Transaction>(&b).ok())
                .filter(|tx| tx.txid() == txid);
            if kept && tx.is_none() && read == SentRead::Complete {
                tracing::warn!(%wallet, "a sent payment's stored transaction does not read back");
            }
            let read = coins.and_then(|coins| {
                (coins.split(',').filter(|c| !c.is_empty()))
                    .map(OutPoint::from_str)
                    .collect::<Result<BTreeSet<_>, _>>()
                    .ok()
            });
            let coins = match read {
                Some(coins) => coins,
                None => {
                    tracing::warn!(%wallet, "a sent mark does not read back");
                    let mut coins: BTreeSet<OutPoint> = inputs.keys().copied().collect();
                    if coins.is_empty() {
                        coins.extend(
                            tx.iter()
                                .flat_map(|tx| tx.input.iter().map(|i| i.previous_output)),
                        );
                    }
                    if coins.is_empty() {
                        return None;
                    }
                    coins
                }
            };
            let mark = SentMark {
                coins,
                tx,
                kept,
                inputs,
            };
            Some((txid, mark))
        })
        .collect();
    (sent, read)
}

/// A `sentin:` value: each coin's outpoint, value and address.
fn parse_inputs(value: &str) -> BTreeMap<OutPoint, (u64, String)> {
    (value.split(','))
        .filter_map(|e| match e.split(' ').collect::<Vec<_>>()[..] {
            [o, v, a] => Some((OutPoint::from_str(o).ok()?, (v.parse().ok()?, a.to_owned()))),
            _ => None,
        })
        .collect()
}

/// What a sent mark holds: the wallet's coins the app signed, and the
/// signed transaction ([`SENT_TX_PREFIX`]), which a mark an earlier build
/// wrote lacks. `kept`: a transaction is stored for it, read or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SentMark {
    pub coins: BTreeSet<OutPoint>,
    pub tx: Option<Transaction>,
    pub kept: bool,
    /// The coins' values and addresses ([`SENT_IN_PREFIX`]).
    pub inputs: BTreeMap<OutPoint, (u64, String)>,
}

/// The payments this app sent.
pub(crate) type Sent = BTreeMap<Txid, SentMark>;

/// Drops the marks of the payments of `wallet` (`gone`) gone from the
/// history or ChainLocked.
pub(crate) fn forget_sent(
    appdb: &AppDb,
    wallet: WalletId,
    gone: &[Txid],
) -> Result<(), dw_appdb::AppDbError> {
    forget_swept(appdb, wallet, gone, &Dead::new(), &[], &[], None)
}

/// What a sweep of key-wallet names of how it removed its transactions,
/// for their [`SWEPT_BY_PREFIX`] notes: the winner, its block when the
/// event names one, whether the sweep is a claim's, and the swept
/// transactions' bytes as the history held them.
pub(crate) struct Sweep<'a> {
    pub winner: Txid,
    pub block: Option<(u32, BlockHash)>,
    pub claim: bool,
    pub txs: &'a [Transaction],
}

/// [`forget_sent`] of a sweep's `gone`, recording `dead` as dropped
/// ([`DROPPED_PREFIX`]) and `claims`, swept as claims
/// ([`CLAIM_SWEPT_PREFIX`]), in the same write: a load seeds its walk with
/// the dead even when the drop of what is built on them never ran (#739),
/// and takes no child of a claim as orphaned. Each of `kept`, swept
/// transactions the history keeps listed as dropped, is marked abandoned
/// with its copy in it too, so a load lists it again. Each transaction of
/// `sweep` gets its [`SWEPT_BY_PREFIX`] note, and a sweep whose winner's
/// block is named fills it in the notes of earlier sweeps of that winner
/// that lack it (Q81d S4).
pub(crate) fn forget_swept(
    appdb: &AppDb,
    wallet: WalletId,
    gone: &[Txid],
    dead: &Dead,
    claims: &[Txid],
    kept: &[(Txid, Transaction, OwnInputs)],
    sweep: Option<&Sweep<'_>>,
) -> Result<(), dw_appdb::AppDbError> {
    if gone.is_empty() && dead.is_empty() && claims.is_empty() {
        return Ok(());
    }
    let mut swept_by: Vec<(String, String)> = Vec::new();
    if let Some(sweep) = sweep {
        let note = |tx: &Transaction| crate::history::SweptBy {
            claim: sweep.claim,
            winner: sweep.winner,
            block: sweep.block.filter(|_| !sweep.claim),
            tx: tx.clone(),
        };
        if sweep.block.is_some() {
            let earlier =
                match appdb.settings_with_prefix_lossy(&wallet.to_string(), SWEPT_BY_PREFIX) {
                    Ok(rows) => rows,
                    Err(e) => {
                        tracing::warn!(%wallet, error = %e, "could not read the sweep notes");
                        Vec::new()
                    }
                };
            for (key, value) in earlier {
                let filled = Txid::from_str(&key[SWEPT_BY_PREFIX.len()..])
                    .ok()
                    .zip(value)
                    .and_then(|(t, v)| crate::history::SweptBy::decode(&t, &v))
                    .filter(|n| !n.claim && n.block.is_none() && n.winner == sweep.winner)
                    .map(|n| crate::history::SweptBy {
                        block: sweep.block,
                        ..n
                    });
                if let Some(filled) = filled {
                    swept_by.push((key, filled.encode()));
                }
            }
        }
        swept_by.extend(
            (sweep.txs.iter())
                .map(|tx| (format!("{SWEPT_BY_PREFIX}{}", tx.txid()), note(tx).encode())),
        );
    }
    let abandoned: Vec<(String, String)> = (kept.iter())
        .map(|(t, tx, _)| (format!("{ABANDONED_PREFIX}{t}"), serialize_hex(tx)))
        .collect();
    let none = OwnInputs::new();
    let inputs_of = |t: &Txid| {
        (kept.iter())
            .find(|(k, ..)| k == t)
            .map_or(&none, |(.., i)| i)
    };
    let keys: Vec<String> = (gone.iter())
        .flat_map(|t| {
            [
                SENT_PREFIX,
                SENT_TX_PREFIX,
                SENT_IN_PREFIX,
                crate::restore_gate::RESTORE_HOLD_PREFIX,
            ]
            .map(|p| format!("{p}{t}"))
        })
        .collect();
    let dropped: Vec<(String, String)> = (dead.iter())
        .map(|(t, cause)| {
            (
                format!("{DROPPED_PREFIX}{t}"),
                drop_mark(*cause, inputs_of(t)),
            )
        })
        .collect();
    let claimed: Vec<String> = (claims.iter())
        .map(|t| format!("{CLAIM_SWEPT_PREFIX}{t}"))
        .collect();
    let writes: Vec<(&str, Option<&str>)> = (keys.iter().map(|k| (k.as_str(), None)))
        .chain(dropped.iter().map(|(k, v)| (k.as_str(), Some(v.as_str()))))
        .chain(claimed.iter().map(|k| (k.as_str(), Some(""))))
        .chain(
            abandoned
                .iter()
                .map(|(k, v)| (k.as_str(), Some(v.as_str()))),
        )
        .chain(swept_by.iter().map(|(k, v)| (k.as_str(), Some(v.as_str()))))
        .collect();
    let scope = wallet.to_string();
    let write = || {
        #[cfg(all(test, debug_assertions))]
        appdb.fault_point(FORGET_SENT_FAULT, dw_appdb::fault::Stage::Before)?;
        appdb.set_settings_durable(&scope, &writes)
    };
    match write() {
        Err(e) if !claims.is_empty() || !dead.is_empty() => {
            tracing::warn!(%wallet, error = %e, "retrying the claim sweep marks");
            write()
        }
        result => result,
    }
}

/// Whether `wallet` keeps a sent mark of `txid` (its coins or its copy).
pub(crate) fn has_sent_mark(
    appdb: &AppDb,
    wallet: WalletId,
    txid: Txid,
) -> Result<bool, dw_appdb::AppDbError> {
    let scope = wallet.to_string();
    Ok(appdb
        .setting_lossy(&scope, &format!("{SENT_PREFIX}{txid}"))?
        .is_some()
        || appdb
            .setting_lossy(&scope, &format!("{SENT_TX_PREFIX}{txid}"))?
            .is_some())
}

/// A test's fault point on the sent-mark read of an abandon's stops.
#[cfg(all(test, debug_assertions))]
pub(crate) const SENT_MARK_READ_FAULT: &str = "tx_actions.sent_mark_read";

/// A test's fault point on [`forget_sent`]'s delete.
#[cfg(all(test, debug_assertions))]
pub(crate) const FORGET_SENT_FAULT: &str = "tx_actions.forget_sent";

/// dash-qt hides popups for these unless "show CoinJoin popups" is on
/// (`WalletView::processNewTransaction`).
pub fn is_coinjoin_internal(t: TxType) -> bool {
    matches!(
        t,
        TxType::CoinJoinMixing
            | TxType::CoinJoinCollateralPayment
            | TxType::CoinJoinMakeCollaterals
            | TxType::CoinJoinCreateDenominations
    )
}

/// The abandoned marks of `wallet`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Abandoned {
    /// Each payment's copy.
    pub copies: Vec<(Txid, Transaction)>,
    /// The payments known by their mark's key whose copy does not read
    /// back: abandoned, so they claim nothing.
    pub unread: BTreeSet<Txid>,
}

/// The abandoned marks of `wallet` recorded in the app database, and false
/// only when they cannot be read at all. A damaged mark affects only its
/// own payment: a row whose key is not text or names no txid is left out,
/// and one whose copy does not read back is in `unread`.
pub(crate) fn load_abandoned(appdb: &AppDb, wallet: WalletId) -> (Abandoned, bool) {
    let rows = match appdb.settings_with_prefix_lossy(&wallet.to_string(), ABANDONED_PREFIX) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%wallet, error = %e, "could not read the abandoned marks");
            return (Abandoned::default(), false);
        }
    };
    let mut out = Abandoned::default();
    for (key, value) in rows {
        let Ok(txid) = Txid::from_str(&key[ABANDONED_PREFIX.len()..]) else {
            tracing::warn!(%wallet, "an abandoned mark names no transaction");
            continue;
        };
        let copy = (value.and_then(|v| hex::decode(v).ok()))
            .and_then(|b| deserialize::<Transaction>(&b).ok())
            .filter(|tx| tx.txid() == txid);
        match copy {
            Some(tx) => out.copies.push((txid, tx)),
            None => {
                tracing::warn!(%wallet, "an abandoned payment's copy does not read back");
                out.unread.insert(txid);
            }
        }
    }
    (out, true)
}

/// What a sweep superseded by `by` dropped that can never confirm (#739):
/// of the `swept` transactions the history held, the unsigned claims
/// key-wallet dropped (each spends an output of `by`), and the swept ones
/// that descend from them, as built on one. `None` when the sweep dropped no claim the
/// history held (a plain conflict sweep), or one through an input script
/// that isn't push-only, whose verdict may be wrong.
pub(crate) fn swept_claims(wallet: WalletId, by: Txid, swept: &[Transaction]) -> Option<Dead> {
    let tree = claim_tree(by, swept);
    if tree.is_empty() {
        return None;
    }
    if let Some(tx) = swept.iter().find(|tx| {
        (tx.input.iter()).any(|i| i.previous_output.txid == by && !push_only(&i.script_sig))
    }) {
        tracing::warn!(%wallet, claim = %tx.txid(), %by, "kept what is built on a dropped claim whose input script isn't push-only");
        return None;
    }
    let dead = (swept.iter())
        .filter(|tx| tree.contains(&tx.txid()))
        .map(|tx| {
            let cause = match (tx.input.iter()).any(|i| i.previous_output.txid == by) {
                true => DropCause::InvalidClaim,
                false => DropCause::InvalidSource,
            };
            (tx.txid(), cause)
        })
        .collect();
    Some(dead)
}

/// Of the `swept` transactions, those that spend an output of `by` (claims
/// on it) and those that descend from them, whatever their input scripts.
pub(crate) fn claim_tree(by: Txid, swept: &[Transaction]) -> BTreeSet<Txid> {
    let claims = (swept.iter())
        .filter(|tx| (tx.input.iter()).any(|i| i.previous_output.txid == by))
        .map(|tx| tx.txid());
    built_on(claims.collect(), swept)
}

/// `tree` with every one of the `swept` transactions that descends from it.
pub(crate) fn built_on(mut tree: BTreeSet<Txid>, swept: &[Transaction]) -> BTreeSet<Txid> {
    loop {
        let before = tree.len();
        for tx in swept {
            if (tx.input.iter()).any(|i| tree.contains(&i.previous_output.txid)) {
                tree.insert(tx.txid());
            }
        }
        if tree.len() == before {
            return tree;
        }
    }
}

/// The transactions this app dropped as ones that can never confirm, or
/// built on one ([`DROPPED_PREFIX`]), each with why ([`drop_mark_cause`]),
/// and whether they read. Which transactions they are comes from the keys
/// alone: no value keeps one out.
pub(crate) fn load_dropped(appdb: &AppDb, wallet: WalletId) -> (Dead, bool) {
    match appdb.settings_with_prefix_lossy(&wallet.to_string(), DROPPED_PREFIX) {
        // A record whose key names no transaction is left out, as a damaged
        // abandoned mark is: it can't be dropped, and an incomplete read
        // would refuse every Abandon for good.
        Ok(rows) => {
            let dropped = (rows.into_iter())
                .filter_map(
                    |(key, value)| match Txid::from_str(&key[DROPPED_PREFIX.len()..]) {
                        Ok(txid) => Some((txid, drop_mark_cause(value.as_deref()))),
                        Err(_) => {
                            tracing::warn!(%wallet, %key, "a dropped record names no transaction");
                            None
                        }
                    },
                )
                .collect();
            (dropped, true)
        }
        Err(e) => {
            tracing::warn!(%wallet, error = %e, "could not read the dropped transactions");
            (Dead::new(), false)
        }
    }
}

/// The own inputs `wallet`'s [`DROPPED_PREFIX`] marks keep
/// ([`DropMark::inputs`]), for a load that lists a dropped payment from its
/// abandoned mark alone. A mark that doesn't read keeps none.
pub(crate) fn load_drop_inputs(
    appdb: &AppDb,
    wallet: WalletId,
    network: dashcore::Network,
) -> BTreeMap<Txid, OwnInputs> {
    let rows =
        (appdb.settings_with_prefix_lossy(&wallet.to_string(), DROPPED_PREFIX)).unwrap_or_default();
    (rows.into_iter())
        .filter_map(|(key, value)| {
            let txid = Txid::from_str(&key[DROPPED_PREFIX.len()..]).ok()?;
            let mark = serde_json::from_str::<DropMark>(value.as_deref()?).ok()?;
            let inputs: OwnInputs = (mark.inputs.into_iter())
                .filter_map(|(n, (amount, address))| {
                    let address = address.parse::<dashcore::Address<_>>().ok()?;
                    Some((n, (amount, address.require_network(network).ok()?)))
                })
                .collect();
            (!inputs.is_empty()).then_some((txid, inputs))
        })
        .collect()
}

/// The txids `wallet`'s abandoned marks name, each by its key (an Abandon's
/// commit point): one whose copy does not read back still counts.
pub(crate) fn abandoned_txids(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<HashSet<Txid>, dw_appdb::AppDbError> {
    Ok(
        (appdb.settings_with_prefix_lossy(&wallet.to_string(), ABANDONED_PREFIX)?)
            .into_iter()
            .filter_map(|(key, _)| Txid::from_str(&key[ABANDONED_PREFIX.len()..]).ok())
            .collect(),
    )
}

fn release_mark(
    txid: Txid,
    tx: &Transaction,
    released: &BTreeSet<OutPoint>,
) -> (String, Option<String>) {
    let coins = tx
        .input
        .iter()
        .map(|input| input.previous_output)
        .filter(|outpoint| released.contains(outpoint))
        .map(|outpoint| outpoint.to_string())
        .collect::<Vec<_>>()
        .join(",");
    (format!("{ABANDON_RELEASE_PREFIX}{txid}"), Some(coins))
}

fn settled_in_store(
    persister: &platform_wallet_storage::SqlitePersister,
    wallets: &HashMap<[u8; 32], crate::history_ops::Persisted>,
    wallet: WalletId,
    candidates: &HashSet<Txid>,
) -> Result<HashSet<Txid>, platform_wallet::changeset::PersistenceError> {
    let mut settled = HashSet::new();
    for (id, other) in wallets.iter().filter(|(id, _)| **id != wallet.0) {
        settled.extend(
            other
                .recordless
                .difference(&other.unconfirmed)
                .chain(other.lock_parents.keys())
                .filter(|t| candidates.contains(*t))
                .copied(),
        );
        for t in other.records.iter().filter(|t| candidates.contains(*t)) {
            if persister.get_core_tx_record(*id, t)?.is_some_and(|record| {
                record.context.block_info().is_some()
                    || matches!(record.context, TransactionContext::InstantSend(_))
            }) {
                settled.insert(*t);
            }
        }
    }
    Ok(settled)
}

/// Finishes in the wallet store the abandons whose marks are stored but
/// whose sweeps are not (the mark is the commit point): of `wallets`, each
/// payment marked abandoned that is still stored out of a block and
/// unlocked (its record in the mempool, or only its outputs) is swept.
/// Replays the original release set subject to newer spend evidence.
/// Only marked payments are swept; one mined or InstantSend-locked since
/// (a height-only row, a record in a block or locked, a stored lock) counts
/// again and is left. Run before the wallets load, so a swept payment's
/// change is never credited. A mark is known by its key, so one whose copy
/// does not read back still counts, its coins then taken from its sent
/// mark. A read that fails (the marks, the drops' holds, a stored record,
/// the sent marks) or a sweep that cannot be stored fails the load.
/// Returns each wallet's drops' holds, which its load claims.
pub(crate) fn finish_marked_abandons(
    persister: &platform_wallet_storage::SqlitePersister,
    appdb: &AppDb,
    db_path: &std::path::Path,
    wallets: &[WalletId],
) -> Result<DroppedHolds, EngineError> {
    let storage = |e: &dyn std::fmt::Display| {
        EngineError::Storage(format!("finishing an abandoned payment: {e}"))
    };
    let (mut listed, mut holds) = (None, DroppedHolds::new());
    for &wallet in wallets {
        // Read once, here: one that cannot be read fails the load, as the
        // marks do, and the load claims what this read.
        let held = dropped_holds(appdb, wallet).map_err(|e| storage(&e))?;
        holds.insert(wallet, held);
        // Each mark by its key: one whose copy does not read back still
        // names its payment.
        let rows = (appdb.settings_with_prefix_lossy(&wallet.to_string(), ABANDONED_PREFIX))
            .map_err(|e| storage(&e))?;
        let marks: BTreeMap<Txid, Option<Transaction>> = (rows.into_iter())
            .filter_map(|(key, value)| {
                let txid = Txid::from_str(&key[ABANDONED_PREFIX.len()..]).ok()?;
                let copy = (value.and_then(|v| hex::decode(v).ok()))
                    .and_then(|b| deserialize::<Transaction>(&b).ok())
                    .filter(|tx| tx.txid() == txid);
                Some((txid, copy))
            })
            .collect();
        if marks.is_empty() {
            continue;
        }
        #[cfg(test)]
        if finish_fault::take(db_path) {
            return Err(EngineError::Storage("test: the load's step fails".into()));
        }
        let all = match &mut listed {
            Some(all) => all,
            None => listed
                .insert(crate::history_ops::persisted_txids(db_path).map_err(|e| storage(&e))?),
        };
        let persisted = all.get(&wallet.0).cloned().unwrap_or_default();
        let marked: HashSet<_> = marks.keys().copied().collect();
        let settled_elsewhere =
            settled_in_store(persister, all, wallet, &marked).map_err(|e| storage(&e))?;
        let (outputs, locked) = crate::history_ops::txids_with_outputs_and_locks(
            db_path,
            wallet,
            marks.keys().copied(),
        )
        .map_err(|e| storage(&e))?;
        let mut pending: BTreeMap<Txid, Option<Transaction>> = BTreeMap::new();
        for (t, copy) in marks {
            let mined = persisted.recordless.contains(&t) && !persisted.unconfirmed.contains(&t);
            if mined || locked.contains(&t) || settled_elsewhere.contains(&t) {
                continue;
            }
            if !persisted.records.contains(&t) {
                if outputs.contains(&t) {
                    pending.insert(t, copy);
                }
                continue;
            }
            match persister
                .get_core_tx_record(wallet.0, &t)
                .map_err(|e| storage(&e))?
            {
                Some(r) => {
                    if matches!(
                        r.context,
                        key_wallet::transaction_checking::TransactionContext::Mempool
                    ) {
                        pending.insert(t, Some(r.transaction));
                    }
                }
                None if outputs.contains(&t) => {
                    pending.insert(t, copy);
                }
                None => {}
            }
        }
        let Some(&root) = pending.keys().next() else {
            continue;
        };
        let mut spent_by_others = HashSet::new();
        let mut mined_funding: HashSet<_> = persisted
            .recordless
            .difference(&persisted.unconfirmed)
            .copied()
            .collect();
        let mut pending_funding = persisted.unconfirmed.clone();
        for t in persisted
            .records
            .iter()
            .filter(|t| !pending.contains_key(*t))
        {
            if let Some(r) = persister
                .get_core_tx_record(wallet.0, t)
                .map_err(|e| storage(&e))?
            {
                if r.context.block_info().is_some()
                    || matches!(r.context, TransactionContext::InstantSend(_))
                {
                    mined_funding.insert(*t);
                } else {
                    pending_funding.insert(*t);
                }
                spent_by_others.extend(r.transaction.input.iter().map(|i| i.previous_output));
            }
        }
        let scope = wallet.to_string();
        let planned: HashMap<_, _> = (appdb
            .settings_with_prefix_lossy(&scope, ABANDON_RELEASE_PREFIX))
        .map_err(|e| storage(&e))?
        .into_iter()
        .filter_map(|(key, value)| {
            let txid = Txid::from_str(&key[ABANDON_RELEASE_PREFIX.len()..]).ok()?;
            let coins = value
                .as_deref()
                .unwrap_or_default()
                .split(',')
                .filter_map(|o| OutPoint::from_str(o).ok())
                .collect::<BTreeSet<_>>();
            Some((txid, coins))
        })
        .collect();
        let drop_held: HashSet<Txid> = (appdb
            .settings_with_prefix_lossy(&wallet.to_string(), DROP_HOLDS_PREFIX))
        .map_err(|e| storage(&e))?
        .into_iter()
        .filter_map(|(key, _)| Txid::from_str(&key[DROP_HOLDS_PREFIX.len()..]).ok())
        .collect();
        let mut sent = None;
        let (mut spent, mut held) = (BTreeSet::new(), BTreeSet::new());
        for (t, copy) in &pending {
            let inputs: BTreeSet<_> = match copy {
                Some(tx) => tx.input.iter().map(|i| i.previous_output).collect(),
                None => {
                    if sent.is_none() {
                        let (read, complete) = load_sent(appdb, wallet);
                        if !complete {
                            return Err(storage(&"the sent marks do not read"));
                        }
                        sent = Some(read);
                    }
                    let coins = (sent.iter()).filter_map(|s| s.get(t));
                    coins.flat_map(|m| m.coins.iter().copied()).collect()
                }
            };
            for input in inputs {
                let approved = match planned.get(t) {
                    Some(coins) => coins.contains(&input),
                    None => !drop_held.contains(t) && mined_funding.contains(&input.txid),
                };
                if approved {
                    spent.insert(input);
                } else {
                    held.insert(input);
                }
            }
        }
        if !spent.is_empty() {
            let sent = match sent {
                Some(sent) => sent,
                None => {
                    let (sent, complete) = load_sent(appdb, wallet);
                    if !complete {
                        return Err(storage(&"the sent marks do not read"));
                    }
                    sent
                }
            };
            spent_by_others.extend(
                sent.iter()
                    .filter(|(t, _)| !marked.contains(*t))
                    .flat_map(|(_, mark)| mark.coins.iter().copied()),
            );
        }
        let released: BTreeSet<OutPoint> = (spent.into_iter())
            .filter(|o| {
                (!marked.contains(&o.txid) || mined_funding.contains(&o.txid))
                    && !pending_funding.contains(&o.txid)
                    && !spent_by_others.contains(o)
                    && !held.contains(o)
            })
            .collect();
        tracing::info!(%wallet, payments = pending.len(), "finishing abandoned payments the wallet store still holds");
        let changeset = PlatformWalletChangeSet {
            core: Some(CoreChangeSet {
                sweeps: vec![SweepBatch {
                    txids: pending.keys().copied().collect(),
                    superseded_by: root,
                    winner_mined_height: None,
                    released_outpoints: released.into_iter().collect(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        persister
            .store(wallet.0, changeset)
            .and_then(|()| persister.flush(wallet.0))
            .map_err(|e| storage(&e))?;
    }
    Ok(holds)
}

/// What decides a transaction's actions.
#[derive(Debug, Clone, Copy)]
struct Facts {
    depth: u32,
    chain_locked: bool,
    instant_locked: bool,
    uncertain_fork: bool,
    /// [`chainlock_beaten`]: a block under the wallet's ChainLock spends
    /// one of its inputs.
    chainlock_beaten: bool,
    /// Something settled descends from it (#446); read for the detail only.
    settled_descendant: bool,
    coinbase: bool,
    abandoned: bool,
    from_me: usize,
    credit: u64,
    debit: u64,
    /// The debit counts none of the inputs, some are unresolved, and the
    /// transaction pays one of this wallet's change addresses: a payment of
    /// this wallet whose coins it does not know yet.
    debit_unknown: bool,
    /// Some inputs are not known to be this wallet's.
    inputs_unresolved: bool,
    matures_in: Option<u32>,
    /// An Abandon of it committed its mark (its sweep may not be stored
    /// yet): never resent.
    abandon_marked: bool,
    /// Its asset lock is tracked ([`TxActionRefusal::PlatformFunding`]).
    platform_funding: bool,
}

/// Whether one of the transactions `is` picks funds an asset lock of
/// `locks` the catch-up's resume may still send (`Built`, `Broadcast`): is
/// its funding, or has an output the funding spends. A lock not yet sent is
/// in no history or key-wallet record, so only its inputs show it.
fn funds_a_pending_lock<'a, L>(locks: L, is: impl Fn(&Txid) -> bool) -> bool
where
    L: IntoIterator<Item = &'a platform_wallet::TrackedAssetLock> + Clone,
{
    let pending = |l: &&platform_wallet::TrackedAssetLock| {
        crate::platform::lease::TrackedStatus::from(&l.status).pending()
    };
    (locks.clone().into_iter())
        .filter(pending)
        .any(|l| is(&l.out_point.txid))
        || crate::coins::pending_lock_inputs(locks)
            .iter()
            .any(|o| is(&o.txid))
}

/// The asset locks every loaded wallet tracks: a payment between two of
/// them funds the recipient's lock as much as the sender's.
fn loaded_locks(
    wm: &platform_wallet::key_wallet_manager::WalletManager<platform_wallet::PlatformWalletInfo>,
) -> Vec<&platform_wallet::TrackedAssetLock> {
    (wm.get_all_wallet_infos().values())
        .flat_map(|i| i.tracked_asset_locks.values())
        .collect()
}

/// Abandons `root`, then each of `descendants` key-wallet's own walk did
/// not reach; nothing when it refuses `root`.
fn abandon_with(
    core: &mut key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    root: Txid,
    descendants: &[Txid],
) -> BTreeSet<Txid> {
    let mut abandoned = core
        .abandon_transaction_with_spends(root, &BTreeMap::new())
        .abandoned;
    if abandoned.is_empty() {
        return abandoned;
    }
    for t in descendants {
        if !abandoned.contains(t) {
            let more = core.abandon_transaction_with_spends(*t, &BTreeMap::new());
            abandoned.extend(more.abandoned);
        }
    }
    abandoned
}

/// What [`chainlock_beaten`] reads: the wallet's ChainLock when the SPV
/// header chain holds its block, and that chain's block hash at each
/// height it has of those asked. `blocked`: the locked
/// transactions out of a block the exception cannot apply to
/// ([`override_blocked`]) when it was read.
#[derive(Default)]
struct ChainProof {
    chainlock: Option<(u32, BlockHash)>,
    active: HashMap<u32, BlockHash>,
    blocked: HashSet<Txid>,
}

fn locked(e: &TxEntry) -> bool {
    e.instant_locked || matches!(e.context, TransactionContext::InstantSend(_))
}

/// Out of a block, not InstantSend-locked and not abandoned.
fn pending(e: &TxEntry) -> bool {
    !e.is_abandoned() && e.context.block_info().is_none() && !locked(e)
}

fn pending_for_dead_drop(t: &Txid, e: &TxEntry, core: &ManagedWalletInfo) -> bool {
    pending(e)
        || (e.is_abandoned()
            && !core.is_dismissed(t)
            && core
                .accounts
                .all_accounts()
                .iter()
                .any(|a| a.has_transaction(t)))
}

/// What an abandon that drops transactions that can never confirm (#739)
/// carries: those already dropped whose outputs it spends (`dead`: none of
/// their outputs is released or scanned for again, as none exists), and
/// what must still hold, read under the lock that applies it.
pub(crate) struct DeadDrop {
    dead: BTreeSet<Txid>,
    known_dead: Arc<Dead>,
    event_marks: Dead,
    /// Of what it drops and `dead`, the never-valid claims themselves;
    /// the rest is built on one.
    claims: BTreeSet<Txid>,
    holds: Box<DeadCheck>,
}

impl DeadDrop {
    fn cause_of(&self, txid: &Txid) -> DropCause {
        match self.claims.contains(txid) {
            true => DropCause::InvalidClaim,
            false => DropCause::InvalidSource,
        }
    }
}

/// What a [`DeadDrop`] checks under the lock: the history, key-wallet's
/// state and what is settled across the loaded wallets.
type DeadCheck =
    dyn Fn(&HashMap<Txid, TxEntry>, &ManagedWalletInfo, &Settled) -> bool + Send + Sync;

/// A transaction or descendant settled in any loaded wallet vetoes the drop.
fn tree_settled(core: &ManagedWalletInfo, settled: &Settled, t: &Txid) -> bool {
    settled.direct.contains_key(t) || settled.under.contains(t) || held_settled(core, t).is_some()
}

/// Whether every instruction of `script` parses and pushes bytes, as
/// [`crate::input_authorization::verify_input_authorization`] reads
/// a push-only input script.
pub(crate) fn push_only(script: &dashcore::Script) -> bool {
    (script.instructions()).all(|i| {
        matches!(
            i,
            Ok(dashcore::blockdata::script::Instruction::PushBytes(_))
        )
    })
}

/// Whether `claim` is a pending transaction of the history that spends a
/// coin of the wallet a transaction of `funded` pays, through a push-only
/// input [`verify_input_authorization`] proves unsigned by the coin's owner
/// (`Ok(false)`). The coin's output is read from the funding transaction
/// the history holds under that txid, once it hashes to it, and must be one
/// key-wallet holds unspent. An input the rule can't decide (`Err`) or one
/// not push-only, whose key or signature the script could build as it
/// runs, keeps the claim.
///
/// [`verify_input_authorization`]: crate::input_authorization::verify_input_authorization
fn unsigned_claim(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    claim: Txid,
    funded: &BTreeSet<Txid>,
) -> bool {
    let Some(e) = txs.get(&claim).filter(|e| pending(e)) else {
        return false;
    };
    if core.instant_send_locks().contains(&claim) {
        return false;
    }
    let held = |o: &OutPoint| {
        (core.accounts.all_funding_accounts().into_iter()).any(|a| a.utxos.contains_key(o))
    };
    (e.tx.input.iter().enumerate()).any(|(n, input)| {
        let o = input.previous_output;
        if !funded.contains(&o.txid) || !held(&o) || !push_only(&input.script_sig) {
            return false;
        }
        let Some(output) = (txs.get(&o.txid))
            .filter(|f| f.tx.txid() == o.txid)
            .and_then(|f| f.tx.output.get(o.vout as usize))
        else {
            return false;
        };
        crate::input_authorization::verify_input_authorization(&e.tx, n, output) == Ok(false)
    })
}

/// Whether the ChainLock exception cannot apply to a root: key-wallet
/// holds the root's own record in a block, InstantSend-locked or finalized
/// (it refuses the abandon), or a transaction that descends from it is in
/// a block, InstantSend-locked or finalized in any loaded wallet (`under`,
/// from [`NetworkSession::settled_all`]). For any number of roots.
fn override_blocked<'a>(
    under: &'a HashSet<Txid>,
    core: &'a ManagedWalletInfo,
) -> impl Fn(&Txid) -> bool + 'a {
    let settled = |c: &TransactionContext| {
        c.block_info().is_some() || matches!(c, TransactionContext::InstantSend(_))
    };
    let accounts = core.accounts.all_accounts();
    move |root| {
        under.contains(root)
            || accounts.iter().any(|a| {
                a.transaction_is_finalized(root)
                    || a.transactions()
                        .get(root)
                        .is_some_and(|r| settled(&r.context))
            })
    }
}

/// A transaction's parents and whether it is settled (InstantSend-locked,
/// in a block or finalized) or mined (in a block or finalized), as one
/// wallet or several know it.
#[derive(Default)]
struct Known {
    parents: Vec<Txid>,
    settled: bool,
    mined: bool,
}

/// Adds what one wallet knows to `known`: its history (abandoned entries
/// too), key-wallet's records, its locks and the txids it keeps finalized.
/// Reads `txs` once.
fn add_known(
    known: &mut HashMap<Txid, Known>,
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
) {
    let accounts = core.accounts.all_accounts();
    let finalized = |t: &Txid| accounts.iter().any(|a| a.transaction_is_finalized(t));
    let mut add = |t: Txid, tx: &Transaction, locked: bool, mined: bool| {
        let k = known.entry(t).or_default();
        if k.parents.is_empty() {
            k.parents
                .extend(tx.input.iter().map(|i| i.previous_output.txid));
        }
        let mined = mined || finalized(&t);
        k.settled |= locked || mined || core.instant_send_locks().contains(&t);
        k.mined |= mined;
    };
    for (t, e) in txs {
        add(*t, &e.tx, locked(e), e.context.block_info().is_some());
    }
    for account in &accounts {
        for (t, r) in account.transactions() {
            let instant = matches!(r.context, TransactionContext::InstantSend(_));
            add(
                *t,
                &r.transaction,
                instant,
                r.context.block_info().is_some(),
            );
        }
    }
}

/// Adds one loaded wallet's records and verified locks to `known`.
/// A lock proves the parents it names even without a transaction record.
/// Neither clears what another wallet's entry for the same transaction set.
fn add_wallet(known: &mut HashMap<Txid, Known>, h: &WalletHistory, core: &ManagedWalletInfo) {
    add_known(known, &h.txs, core);
    for t in &h.session_locks {
        if h.txs
            .get(t)
            .is_none_or(|entry| h.knows_instant_lock(t, entry))
        {
            known.entry(*t).or_default().settled = true;
        }
    }
    for (t, parents) in h.lock_parent_links() {
        let k = known.entry(*t).or_default();
        if k.parents.is_empty() {
            k.parents.extend_from_slice(parents);
        }
        k.settled = true;
    }
}

/// The transactions something settled descends from in `known`: first
/// under one InstantSend-locked, in a block or finalized, then under one in
/// a block or finalized.
fn walk_known(known: &HashMap<Txid, Known>) -> (HashSet<Txid>, HashSet<Txid>) {
    let ancestors = |pick: fn(&Known) -> bool| {
        let mut under = HashSet::new();
        let mut parents: Vec<Txid> = (known.values())
            .filter(|k| pick(k))
            .flat_map(|k| k.parents.iter().copied())
            .collect();
        while let Some(t) = parents.pop() {
            if under.insert(t) {
                parents.extend(
                    known
                        .get(&t)
                        .into_iter()
                        .flat_map(|k| k.parents.iter().copied()),
                );
            }
        }
        under
    };
    (ancestors(|k| k.settled), ancestors(|k| k.mined))
}

/// [`walk_known`] over one wallet.
#[cfg(test)]
pub(crate) fn settled_under(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
) -> (HashSet<Txid>, HashSet<Txid>) {
    let mut known = HashMap::new();
    add_known(&mut known, txs, core);
    walk_known(&known)
}

/// Whether abandoning `root`, not InstantSend-locked itself, is refused
/// because something settled descends from it (#446): Dash locks or mines
/// a transaction only once its parents are, so the network holds `root`.
/// Not when a ChainLocked spend of one of its inputs beat it (`proof`) and
/// what descends from it is only InstantSend-locked: a ChainLock outranks
/// an InstantSend lock.
fn settled_descendant(
    txs: &HashMap<Txid, TxEntry>,
    settled: &Settled,
    root: &Txid,
    proof: &ChainProof,
) -> bool {
    settled.under.contains(root)
        && (settled.mined.contains(root)
            || !chainlock_beaten(txs, root, proof.chainlock, &proof.active))
}

/// What something settled descends from, walked over every loaded
/// wallet's transactions at once: what one wallet settled can descend from
/// another's payment, through transfers between them either way (#611).
/// `incomplete`: the first of them a stored record of didn't read at its
/// load.
#[derive(Default)]
struct Settled {
    direct: HashMap<Txid, TxActionRefusal>,
    under: HashSet<Txid>,
    mined: HashSet<Txid>,
    incomplete: Option<WalletId>,
}

impl NetworkSession {
    /// Whether wallet `id`'s history is not whole for a drop: a stored
    /// record or mark didn't read at its load, or it is part-way through
    /// opening, its history not loaded yet. Abandon's rule is the first
    /// alone ([`Self::settled_all`]).
    pub(crate) fn history_unread(&self, id: WalletId) -> bool {
        (self.hub.history).read_wallet(&id, |h| h.load_incomplete)
            || self.hub.wallet_state(&id).is_some_and(|s| s.loading)
    }

    fn settled_all<'a>(
        &self,
        infos: impl Iterator<Item = (&'a [u8; 32], &'a ManagedWalletInfo)> + Clone,
        candidates: impl IntoIterator<Item = Txid>,
    ) -> Settled {
        let (mut known, mut incomplete) = (HashMap::new(), None);
        let mut core_locks = HashSet::new();
        for (id, core) in infos.clone() {
            core_locks.extend(core.instant_send_locks().iter().copied());
            let unread = (self.hub.history).read_wallet(&WalletId(*id), |h| {
                add_wallet(&mut known, h, core);
                for t in h.drops.keys() {
                    known.entry(*t).or_default();
                }
                h.load_incomplete
            });
            if unread {
                incomplete.get_or_insert(WalletId(*id));
            }
        }
        for t in core_locks {
            known.entry(t).or_default().settled = true;
        }
        let parents: HashSet<_> = known
            .values()
            .flat_map(|k| k.parents.iter().copied())
            .collect();
        for t in parents.into_iter().chain(candidates) {
            known.entry(t).or_default();
        }
        for (_, core) in infos {
            let accounts = core.accounts.all_accounts();
            for (t, k) in known.iter_mut().filter(|(_, k)| !k.mined) {
                if accounts.iter().any(|a| a.transaction_is_finalized(t)) {
                    k.mined = true;
                    k.settled = true;
                }
            }
        }
        let (under, mined) = walk_known(&known);
        let direct = known
            .into_iter()
            .filter(|(_, k)| k.settled)
            .map(|(t, k)| {
                let refusal = if k.mined {
                    TxActionRefusal::Confirmed
                } else {
                    TxActionRefusal::InstantLocked
                };
                (t, refusal)
            })
            .collect();
        Settled {
            direct,
            under,
            mined,
            incomplete,
        }
    }
}

/// How key-wallet holds `t` in this wallet's own accounts when it is
/// settled: in a block or finalized (`Confirmed`), or InstantSend-locked
/// (`InstantLocked`). The history can lag key-wallet.
fn held_settled(core: &ManagedWalletInfo, t: &Txid) -> Option<TxActionRefusal> {
    let mut locked = core.instant_send_locks().contains(t);
    for a in core.accounts.all_accounts() {
        if a.managed_account_type().is_contact_owned() {
            continue;
        }
        if a.transaction_is_finalized(t) {
            return Some(TxActionRefusal::Confirmed);
        }
        if let Some(r) = a.transactions().get(t) {
            if r.context.block_info().is_some() {
                return Some(TxActionRefusal::Confirmed);
            }
            locked |= matches!(r.context, TransactionContext::InstantSend(_));
        }
    }
    locked.then_some(TxActionRefusal::InstantLocked)
}

/// The transactions out of a block that descend from `root` and are not
/// InstantSend-locked, every child of an output followed, in the history
/// `txs` and in key-wallet's records of `core` (which credits a mempool
/// transaction before the history hears of it): those not abandoned in
/// the order found (key-wallet holds no record of a payment restored as a
/// claimant at load, #88), and the abandoned ones passed through. The
/// history's view of a transaction it holds wins.
fn unmined_descendants(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    root: Txid,
) -> (Vec<Txid>, BTreeSet<Txid>) {
    // A contact's account records the contact's own spends: not this
    // wallet's to abandon.
    let accounts: Vec<_> = (core.accounts.all_accounts().into_iter())
        .filter(|a| !a.managed_account_type().is_contact_owned())
        .collect();
    let locks = core.instant_send_locks();
    let mut open: Vec<(Txid, &Transaction, bool)> = (txs.iter())
        .filter(|(_, e)| e.context.block_info().is_none() && !locked(e))
        .map(|(t, e)| (*t, &e.tx, e.abandoned))
        .collect();
    let mut recorded = HashSet::new();
    for account in &accounts {
        for (t, r) in account.transactions() {
            if !txs.contains_key(t)
                && recorded.insert(*t)
                && r.context.block_info().is_none()
                && !matches!(r.context, TransactionContext::InstantSend(_))
                && !locks.contains(t)
            {
                open.push((*t, &r.transaction, false));
            }
        }
    }
    let mut found = BTreeSet::from([root]);
    let (mut live, mut links) = (Vec::new(), BTreeSet::new());
    let mut parents = vec![root];
    while let Some(parent) = parents.pop() {
        for (t, tx, abandoned) in &open {
            if !found.contains(t) && tx.input.iter().any(|i| i.previous_output.txid == parent) {
                found.insert(*t);
                match abandoned {
                    true => links.insert(*t),
                    false => {
                        live.push(*t);
                        true
                    }
                };
                parents.push(*t);
            }
        }
    }
    (live, links)
}

/// What dropping dead transactions does ([`plan_drop`]).
struct DropPlan {
    dead: BTreeSet<Txid>,
    links: BTreeSet<Txid>,
    copies: Vec<(Txid, Transaction)>,
    /// The coins they spent that none of them nor `links` made.
    inputs: BTreeSet<OutPoint>,
    /// Of `inputs`, those that come back through the rewind.
    freed: BTreeSet<OutPoint>,
    /// The other `inputs`, and each coin another transaction of the wallet
    /// spends: the rewind is not for them.
    kept: HashSet<OutPoint>,
    /// The coins an InstantSend-locked transaction out of a block spends
    /// that no processed block spent: nothing mined takes them yet, so they
    /// are held at each load.
    locked_spent: HashSet<OutPoint>,
    /// The winner's inputs were known: otherwise every coin no processed
    /// block spent is held at each load.
    known: bool,
    /// Of `inputs`, those a processed block spent.
    observed: HashSet<OutPoint>,
}

/// What a drop's detection under the read lock saw, for its recheck under
/// the write lock (#818): the truncations the hub had applied, and, when
/// the winner is a block's spend the wallet does not know
/// ([`unknown_block`]), the header chain's hash at that block's height and
/// the coins of the roots key-wallet held as spent, each at its block's
/// height, and key-wallet's processed height then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recheck {
    truncations: u64,
    /// key-wallet's last processed block height at the detection.
    processed: u32,
    block: Option<BlockHash>,
    spent: Arc<BTreeMap<OutPoint, u32>>,
    /// The rechecks this detection had already.
    tries: u8,
}

/// What a drop leaves for [`NetworkSession::drop_dead`]: some roots had
/// another winner, so the drop finds them again; the roots kept on the
/// detection's evidence that could not go yet, to recheck with it, and the
/// wallet's applied ChainLock that recheck's proof read; and whether another
/// wallet's history was not whole, so the drop waits for it
/// ([`SessionHub::defer_drops`](crate::events::SessionHub::defer_drops));
/// and whether only key-wallet's processed height rising put off a drop
/// with no recheck.
#[derive(Debug, Default)]
pub(crate) struct Rechecked {
    regrouped: bool,
    retry: Vec<Txid>,
    applied: Option<u32>,
    deferred: bool,
    put_off: bool,
}

/// A drop [`NetworkSession::drop_dead`] has queued: its roots, their
/// winner, the transactions swept with them, its detection's evidence
/// when it rechecks them, and its origin: an orphan's gone parents, and
/// what a sweep's winner took.
pub(crate) type DeadWork = (
    Vec<Txid>,
    Winner,
    BTreeSet<Txid>,
    Option<Recheck>,
    (BTreeSet<Txid>, BTreeSet<OutPoint>),
);

fn group_carried_drops(
    carried: Vec<(Txid, crate::events::CarriedDrop)>,
) -> Vec<(Vec<Txid>, Winner, Recheck, u32)> {
    let mut groups = BTreeMap::new();
    for (txid, (winner, recheck, height, _)) in carried {
        let key = (winner, height, Arc::as_ptr(&recheck.spent).addr());
        let (roots, _) = groups.entry(key).or_insert_with(|| (Vec::new(), recheck));
        roots.push(txid);
    }
    groups
        .into_iter()
        .map(|((winner, height, _), (roots, recheck))| (roots, winner, recheck, height))
        .collect()
}

/// The height whose block a swept payment's children put off by a block
/// must still find in the header chain (#891): key-wallet's processed
/// height at the take, or the winner's block above it, which a wallet
/// behind the chain is swept by.
pub(crate) fn take_anchor(processed: u32, winner: Winner) -> u32 {
    processed.max(winner.mined.unwrap_or(0))
}

/// Key-wallet's processed height for `wallet`, if it is loaded.
async fn processed_height(manager: &Manager, wallet: WalletId) -> Option<u32> {
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    (wm.get_wallet_info(&wallet.0)).map(|i| i.core_wallet.last_processed_height())
}

/// How often a detection's roots are rechecked with its evidence.
const RECHECKS: u8 = 3;

/// How many times [`NetworkSession::drop_dead`] finds beaten payments again
/// after a recheck found other winners.
const DETECTIONS: u8 = 3;

/// What [`NetworkSession::find_dead`] reads: the truncations, key-wallet's
/// last processed height, the beaten payments with their winners and the
/// coins of [`Recheck::spent`], and the orphans with their gone parents.
type FoundDead = (
    u64,
    u32,
    Vec<(Txid, Winner, BTreeMap<OutPoint, u32>)>,
    Option<(Vec<Txid>, BTreeSet<Txid>)>,
);

/// The height of `winner`'s block when the winner is a block's spend the
/// wallet does not know (no txid).
fn unknown_block(winner: Winner) -> Option<u32> {
    match winner {
        Winner {
            txid: None,
            mined: Some(at),
        } => Some(at),
        _ => None,
    }
}

/// The coins of `root` that the detection saw key-wallet hold as spent by
/// `winner`'s block, if key-wallet holds none of them so any more and
/// `root` is still out of a block, not abandoned and not
/// InstantSend-locked, key-wallet processed that block and none since the
/// detection, and the detection saw no coin of `root` spent by another
/// block (#818, Q89a).
/// The caller proves the block final under the wallet's own ChainLock with
/// no truncation since the detection, and that key-wallet holds no record
/// of `root`.
fn spend_forgotten_as_final(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    root: &Txid,
    winner: Winner,
    detected: &Recheck,
) -> BTreeMap<OutPoint, u32> {
    let (Some(at), Some(e)) = (unknown_block(winner), txs.get(root)) else {
        return BTreeMap::new();
    };
    let observed = core.observed_spent_outpoints();
    let coins: Vec<OutPoint> = e.tx.input.iter().map(|i| i.previous_output).collect();
    let open = e.context.block_info().is_none() && !e.is_abandoned() && !instant_locked(e);
    if !open || core.last_processed_height() < at || coins.iter().any(|o| observed.contains_key(o))
    {
        return BTreeMap::new();
    }
    // A coin of `root` the detection saw spent at another height, or any
    // move of key-wallet's processed height since the detection, may hide a
    // spend this proof can't carry: no drop, as when nothing beat it (#818).
    // A block processed again at or below that height does not move it.
    if core.last_processed_height() != detected.processed
        || (coins.iter()).any(|o| detected.spent.get(o).is_some_and(|h| *h != at))
    {
        return BTreeMap::new();
    }
    (coins.into_iter())
        .filter(|o| detected.spent.get(o) == Some(&at))
        .map(|o| (o, at))
        .collect()
}

/// Plans dropping `roots` and what descends from them in the history `txs`
/// and key-wallet's records of `core`: a root locked, mined, abandoned, or
/// with something settled under it in any loaded wallet (`under`, from
/// `settled_all`) is left. A coin they spent is freed
/// only when the history is loaded and `complete`, `winner`'s inputs are
/// known (the history holds it, or it is mined and key-wallet observed its
/// block's spends), and none of them, of
/// `links` or of the payments swept this session (`swept`) made it, no
/// other transaction of the history or of key-wallet spends it, and no
/// processed block did. `None` when nothing is to go.
fn plan_drop(
    txs: &HashMap<Txid, TxEntry>,
    swept: &HashSet<Txid>,
    (core, forgotten): (&ManagedWalletInfo, &BTreeMap<OutPoint, u32>),
    roots: &[Txid],
    winner: Winner,
    links: &BTreeSet<Txid>,
    (release_proven, chainlock, settled): (bool, Option<u32>, &Settled),
) -> Option<DropPlan> {
    let (mut dead, mut links) = (BTreeSet::new(), links.clone());
    for root in roots {
        let open = (txs.get(root))
            .is_some_and(|e| e.context.block_info().is_none() && !e.is_abandoned() && !locked(e));
        if !open
            || dead.contains(root)
            || settled.under.contains(root)
            || settled.direct.contains_key(root)
            || held_settled(core, root).is_some()
        {
            continue;
        }
        let (descendants, passed) = unmined_descendants(txs, core, *root);
        if descendants
            .iter()
            .chain(&passed)
            .any(|t| settled.direct.contains_key(t))
        {
            continue;
        }
        dead.insert(*root);
        dead.extend(descendants);
        links.extend(passed);
    }
    if dead.is_empty() {
        return None;
    }
    let accounts: Vec<_> = (core.accounts.all_accounts().into_iter())
        .filter(|a| !a.managed_account_type().is_contact_owned())
        .collect();
    let copies: Vec<(Txid, Transaction)> = (dead.iter())
        .filter_map(|t| {
            let tx = (txs.get(t).map(|e| e.tx.clone())).or_else(|| {
                (accounts.iter())
                    .find_map(|a| a.transactions().get(t).map(|r| r.transaction.clone()))
            })?;
            Some((*t, tx))
        })
        .collect();
    let gone = |t: &Txid| dead.contains(t) || links.contains(t);
    let inputs: BTreeSet<OutPoint> = (copies.iter())
        .flat_map(|(_, tx)| tx.input.iter().map(|i| i.previous_output))
        .filter(|o| !gone(&o.txid) && !swept.contains(&o.txid))
        .collect();
    let mut kept: HashSet<OutPoint> = (core.accounts.all_accounts().into_iter())
        .flat_map(|a| a.transactions().iter())
        .filter(|(t, _)| !gone(t))
        .flat_map(|(_, r)| r.transaction.input.iter().map(|i| i.previous_output))
        .collect();
    kept.extend(
        (txs.iter())
            .filter(|(t, e)| !e.is_abandoned() && !gone(t))
            .flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output)),
    );
    // A lock a ChainLocked spend of one of its inputs beat takes nothing
    // (#170 case 14).
    let observed = core.observed_spent_outpoints();
    let spent_at = |o: &OutPoint| observed.get(o).or_else(|| forgotten.get(o));
    let beaten_lock = |t: &Txid, tx: &Transaction| {
        (tx.input.iter()).any(|i| {
            (spent_at(&i.previous_output)).is_some_and(|h| chainlock.is_some_and(|cl| cl >= *h))
                || (txs.iter()).any(|(w, we)| {
                    w != t
                        && chain_locked(&we.context, chainlock)
                        && (we.tx.input.iter()).any(|j| j.previous_output == i.previous_output)
                })
        })
    };
    let mut locked_spent: HashSet<OutPoint> = (txs.iter())
        .filter(|(t, e)| {
            locked(e) && e.context.block_info().is_none() && !gone(t) && !beaten_lock(t, &e.tx)
        })
        .flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output))
        .collect();
    locked_spent.extend(
        (core.accounts.all_accounts().into_iter())
            .flat_map(|a| a.transactions().iter())
            .filter(|(t, r)| {
                !gone(t)
                    && r.context.block_info().is_none()
                    && (matches!(r.context, TransactionContext::InstantSend(_))
                        || core.instant_send_locks().contains(*t))
                    && !beaten_lock(t, &r.transaction)
            })
            .flat_map(|(_, r)| r.transaction.input.iter().map(|i| i.previous_output)),
    );
    locked_spent.retain(|o| spent_at(o).is_none());
    // A lock key-wallet heard of for a transaction neither holds: what it
    // spends is unknown, so nothing is freed.
    let unknown_lock = (core.instant_send_locks().iter()).any(|t| {
        !txs.contains_key(t) && !accounts.iter().any(|a| a.transactions().contains_key(t))
    });
    let known = release_proven
        && !unknown_lock
        && (winner.mined.is_some() || winner.txid.is_some_and(|w| txs.contains_key(&w)));
    let freed: BTreeSet<OutPoint> = (inputs.iter())
        .filter(|o| {
            known
                && !kept.contains(*o)
                && spent_at(o).is_none()
                && !txs
                    .get(&o.txid)
                    .is_some_and(|e| pending(e) || e.is_abandoned())
        })
        .copied()
        .collect();
    kept.extend(inputs.iter().filter(|o| !freed.contains(*o)));
    let inputs_observed = (inputs.iter())
        .filter(|o| spent_at(o).is_some())
        .copied()
        .collect();
    Some(DropPlan {
        dead,
        links,
        copies,
        inputs,
        freed,
        kept,
        locked_spent,
        known,
        observed: inputs_observed,
    })
}

fn facts(entry: &TxEntry, view: &ChainView<'_>, proof: &ChainProof) -> Facts {
    let (credit, debit, from_me) = amounts(entry, view);
    let all_from_me = from_me > 0 && from_me == entry.tx.input.len();
    let (status, _) = status_of(entry, view, all_from_me);
    let depth = depth_of(entry, view.tip);
    Facts {
        depth,
        chain_locked: status.chain_locked,
        instant_locked: status.instant_locked,
        uncertain_fork: false,
        chainlock_beaten: status.instant_locked
            && depth == 0
            && !proof.blocked.contains(&entry.tx.txid())
            && chainlock_beaten(
                view.history,
                &entry.tx.txid(),
                proof.chainlock,
                &proof.active,
            ),
        settled_descendant: false,
        coinbase: entry.tx.is_coin_base(),
        abandoned: entry.is_abandoned(),
        from_me,
        credit,
        debit,
        inputs_unresolved: from_me < entry.tx.input.len() && !entry.tx.is_coin_base(),
        debit_unknown: debit == 0
            && from_me < entry.tx.input.len()
            && !entry.tx.is_coin_base()
            && (entry.tx.output.iter()).any(|o| {
                (view.owned.get(&o.script_pubkey))
                    .is_some_and(|w| w.chain == crate::history::AddressChain::Change)
            }),
        matures_in: status.matures_in,
        abandon_marked: false,
        platform_funding: false,
    }
}

impl Facts {
    /// dash-qt Abandon enablement. "In a mempool" cannot be known on SPV
    /// and does not refuse; an InstantSend lock does, unless a ChainLocked
    /// spend of one of its inputs overrode it.
    fn abandon_refusal(&self) -> Option<TxActionRefusal> {
        if self.abandoned {
            Some(TxActionRefusal::AlreadyAbandoned)
        } else if self.depth > 0 || self.chain_locked {
            Some(TxActionRefusal::Confirmed)
        } else if self.instant_locked && !self.chainlock_beaten {
            Some(TxActionRefusal::InstantLocked)
        } else if self.uncertain_fork {
            Some(TxActionRefusal::RescanFirst)
        } else if self.settled_descendant {
            Some(TxActionRefusal::SettledDescendant)
        } else if self.coinbase {
            Some(TxActionRefusal::Coinbase)
        } else if self.platform_funding {
            Some(TxActionRefusal::PlatformFunding)
        } else {
            None
        }
    }

    /// dash-qt Resend enablement.
    fn resend_refusal(&self) -> Option<TxActionRefusal> {
        if self.abandoned || self.abandon_marked {
            Some(TxActionRefusal::AlreadyAbandoned)
        } else if self.depth > 0 || self.chain_locked {
            Some(TxActionRefusal::Confirmed)
        } else if self.coinbase {
            Some(TxActionRefusal::Coinbase)
        } else if self.instant_locked {
            Some(TxActionRefusal::InstantLocked)
        } else if self.uncertain_fork {
            Some(TxActionRefusal::RescanFirst)
        } else if self.from_me == 0 {
            Some(TxActionRefusal::NotSentByWallet)
        } else {
            None
        }
    }
}

/// Why Activity's Abandon or Resend of a transaction refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityRefusal {
    SpvNotRunning,
    NoPeers,
    Tx(TxActionRefusal),
}

impl From<ActivityRefusal> for EngineError {
    fn from(r: ActivityRefusal) -> Self {
        match r {
            ActivityRefusal::SpvNotRunning => EngineError::SpvNotRunning,
            ActivityRefusal::NoPeers => EngineError::NoPeers,
            ActivityRefusal::Tx(r) => EngineError::TxActionRefused(r),
        }
    }
}

/// [`NetworkSession::abandon_transaction`]'s refusal, in its order: no SPV
/// client first, then dash-qt's enablement.
fn activity_abandon_refusal(spv_running: bool, f: &Facts) -> Option<ActivityRefusal> {
    if !spv_running {
        return Some(ActivityRefusal::SpvNotRunning);
    }
    f.abandon_refusal().map(ActivityRefusal::Tx)
}

/// [`NetworkSession::resend_transaction`]'s refusal, in its order: dash-qt's
/// enablement, then no SPV client, then no connected peer.
fn activity_resend_refusal(f: &Facts, spv_running: bool, peers: u32) -> Option<ActivityRefusal> {
    if let Some(r) = f.resend_refusal() {
        return Some(ActivityRefusal::Tx(r));
    }
    if !spv_running {
        return Some(ActivityRefusal::SpvNotRunning);
    }
    (peers == 0).then_some(ActivityRefusal::NoPeers)
}

/// What Activity's Abandon and Resend would say of one transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivityExits {
    pub abandon: Option<ActivityRefusal>,
    pub resend: Option<ActivityRefusal>,
}

impl NetworkSession {
    /// Activity's verdicts on `txid` of `wallet` with the SPV client
    /// `spv_running` and `peers` connected, as the detail offers them:
    /// `None` when the history doesn't hold it.
    pub(crate) async fn activity_exits(
        &self,
        wallet: WalletId,
        txid: Txid,
        spv_running: bool,
        peers: u32,
    ) -> Result<Option<ActivityExits>, EngineError> {
        let manager = self.manager()?;
        match self.tx_facts(&manager, wallet, txid, true).await {
            Ok((_, f)) => Ok(Some(ActivityExits {
                abandon: activity_abandon_refusal(spv_running, &f),
                resend: activity_resend_refusal(&f, spv_running, peers),
            })),
            Err(EngineError::TxNotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Facts of one transaction of `wallet`, `TxNotFound` when unknown.
    /// With `offer`, the ChainLock exception counts only when the abandon's
    /// plan would not refuse it (what the detail offers); without, the
    /// abandon plans it once, under its lock.
    async fn tx_facts(
        &self,
        manager: &Manager,
        wallet: WalletId,
        txid: Txid,
        offer: bool,
    ) -> Result<(TxEntry, Facts), EngineError> {
        let proof = self.chain_proof(manager, wallet).await;
        let (entry, mut f) = self
            .with_view(manager, wallet, |view| {
                view.history
                    .get(&txid)
                    .map(|entry| (entry.clone(), facts(entry, view, &proof)))
            })
            .await?
            .ok_or_else(|| EngineError::TxNotFound(txid.to_string()))?;
        f.uncertain_fork = self
            .hub
            .history
            .read_wallet(&wallet, |h| h.uncertain_fork_spends.contains_key(&txid));
        if offer {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let locks = loaded_locks(&wm);
            f.platform_funding = (wm.get_wallet_info(&wallet.0)).is_some_and(|i| {
                let (descendants, _) = (self.hub.history).read(&wallet, |txs| {
                    unmined_descendants(txs, &i.core_wallet, txid)
                });
                funds_a_pending_lock(locks.iter().copied(), |t| {
                    *t == txid || descendants.contains(t)
                })
            });
        }
        // The exception's evidence on the inputs, as the abandon checks it:
        // not offered when it would refuse as locked.
        if offer && !f.instant_locked && f.abandon_refusal().is_none() {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let infos = wm.get_all_wallet_infos().iter();
            let settled = self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)), [txid]);
            f.settled_descendant = (self.hub.history).read(&wallet, |txs| {
                settled_descendant(txs, &settled, &txid, &proof)
            });
        }
        if offer && f.chainlock_beaten && f.abandon_refusal().is_none() {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let infos = wm.get_all_wallet_infos().iter();
            let settled = self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)), [txid]);
            let planned = (wm.get_wallet_info(&wallet.0)).map(|i| {
                let (descendants, _) = (self.hub.history).read(&wallet, |txs| {
                    unmined_descendants(txs, &i.core_wallet, txid)
                });
                let core = &i.core_wallet;
                self.plan_abandon(wallet, txid, &descendants, &proof, core, &settled)
            });
            if let Some(Err(EngineError::TxActionRefused(
                TxActionRefusal::InstantLocked | TxActionRefusal::Confirmed,
            ))) = planned
            {
                f.chainlock_beaten = false;
            }
        }
        // Read after the history: an Abandon writes its mark under the
        // manager's lock the view waits on, and flags the entry only once
        // its sweep is stored.
        f.abandon_marked = self.read_abandon_mark(wallet, txid).await?;
        Ok((entry, f))
    }

    /// Of `txids`, those `wallet` sent: each spends one of its coins, as
    /// key-wallet's record, the history's own inputs or an input's funding
    /// entry say (the rule Activity's Resend counts by), or keeps its sent
    /// mark. dash-spv's rebroadcast is the client's, shared by every
    /// loaded wallet, so an abandon stops only these: a payment abandoned
    /// as received leaves its sender's retries. A mark that cannot be read
    /// is the error: it may be another wallet's payment.
    async fn sent_by(
        &self,
        wallet: WalletId,
        txids: &BTreeSet<Txid>,
        info: &platform_wallet::PlatformWalletInfo,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let owned = std::cell::OnceCell::new();
        let spends: HashSet<Txid> = (info.core_wallet.accounts.all_accounts().into_iter())
            .filter(|a| !a.managed_account_type().is_contact_owned())
            .flat_map(|a| a.transactions().iter())
            .filter(|(t, r)| txids.contains(*t) && !r.input_details.is_empty())
            .map(|(t, _)| *t)
            .collect();
        let (mut sent, rest): (BTreeSet<Txid>, BTreeSet<Txid>) =
            (txids.iter().copied()).partition(|t| {
                spends.contains(t)
                    || (self.hub.history).read_wallet(&wallet, |h| {
                        let own = |i: &dashcore::TxIn| {
                            let owned =
                                owned.get_or_init(|| crate::history_ops::owned_scripts(info));
                            let prev = Some(&i.previous_output);
                            crate::history::resolve_own_input(None, prev, &h.txs, owned).is_some()
                        };
                        h.txs
                            .get(t)
                            .is_some_and(|e| !e.own_inputs.is_empty() || e.tx.input.iter().any(own))
                            || h.drop_inputs.get(t).is_some_and(|i| !i.is_empty())
                    })
            });
        if rest.is_empty() {
            return Ok(sent);
        }
        let marks = self
            .appdb_op(move |db| {
                #[cfg(all(test, debug_assertions))]
                db.fault_point(SENT_MARK_READ_FAULT, dw_appdb::fault::Stage::Before)?;
                let sweep_scope = dw_appdb::local_scope(&wallet.to_string());
                (rest.into_iter())
                    .map(|t| {
                        let key = format!("{}{t}", crate::coinjoin::SWEEP_OUT_PREFIX);
                        let sent = has_sent_mark(db, wallet, t)?
                            || db.setting(&sweep_scope, &key)?.is_some();
                        Ok((t, sent))
                    })
                    .collect::<Result<Vec<_>, dw_appdb::AppDbError>>()
            })
            .await?;
        sent.extend(marks.into_iter().filter(|(_, m)| *m).map(|(t, _)| t));
        Ok(sent)
    }

    /// The pinned client owns the whole rebroadcast map. Replacing it after
    /// a durable abandon or drop retires every affected transaction.
    pub(crate) async fn stop_rebroadcasts(
        &self,
        wallet: WalletId,
        txids: impl Iterator<Item = Txid>,
    ) {
        let mut txids = txids.peekable();
        if txids.peek().is_none() {
            return;
        }
        #[cfg(not(test))]
        let _ = wallet;
        #[cfg(test)]
        for txid in txids {
            let marked = self.abandon_marked(wallet, txid).await;
            self.hub
                .stopped_rebroadcasts
                .lock()
                .unwrap()
                .push((txid, marked));
        }
        self.request_rebroadcast_rebuild();
    }

    /// Whether an Abandon of `txid` committed its mark, which it writes
    /// before its sweep: a mark that cannot be read counts.
    pub(crate) async fn abandon_marked(&self, wallet: WalletId, txid: Txid) -> bool {
        self.read_abandon_mark(wallet, txid).await.unwrap_or(true)
    }

    pub(crate) async fn read_abandon_mark(
        &self,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<bool, EngineError> {
        let (scope, key) = (wallet.to_string(), format!("{ABANDONED_PREFIX}{txid}"));
        self.appdb_op(move |db| {
            Ok((db.settings_with_prefix_lossy(&scope, &key)?.into_iter()).any(|(k, _)| k == key))
        })
        .await
    }

    async fn can_restart_abandoned_sweep(&self, wallet: WalletId, txid: Txid, f: &Facts) -> bool {
        f.abandoned
            && f.abandon_marked
            && f.depth == 0
            && !f.instant_locked
            && !f.chain_locked
            && self
                .pending_sweep_chunk(wallet, txid)
                .await
                .is_ok_and(|chunk| chunk.is_some())
    }

    /// Called under the abandon write gate for an explicitly resumed sweep.
    pub(crate) async fn clear_sweep_abandon(
        &self,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        if !self.read_abandon_mark(wallet, txid).await?
            || self
                .hub
                .history
                .read_wallet(&wallet, |h| h.drops.contains_key(&txid))
        {
            return Err(EngineError::TxActionRefused(
                TxActionRefusal::AlreadyAbandoned,
            ));
        }
        self.write_abandoned_marks(
            wallet,
            vec![
                (format!("{ABANDONED_PREFIX}{txid}"), None),
                (format!("{ABANDON_RELEASE_PREFIX}{txid}"), None),
            ],
        )
        .await?;
        self.hub.history.with_wallet(wallet, |history| {
            history.abandoned_here.remove(&txid);
            if let Some(entry) = history.txs.get_mut(&txid) {
                entry.abandoned = false;
            }
        });
        self.hub.pump.mark_history(wallet, Some(&[txid]));
        Ok(())
    }

    /// dash-qt detail fields and action enablement for one transaction.
    pub async fn tx_detail_extras(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<TxDetailExtras, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let (entry, f) = this.tx_facts(&live.manager, wallet, txid, true).await?;
            // A payment this app sent whose coins are not resolved yet (one
            // with no change too): its debit is unknown. Its mark or the
            // mark's copy of it says so. A failed read is taken as one.
            let debit_unknown = f.debit_unknown
                || (f.debit == 0 && f.inputs_unresolved && {
                    let scope = wallet.to_string();
                    let marked = this.appdb_op(move |db| {
                        let read = |p: &str| db.setting(&scope, &format!("{p}{txid}"));
                        Ok(read(SENT_PREFIX)?.is_some() || read(SENT_TX_PREFIX)?.is_some())
                    });
                    !matches!(marked.await, Ok(false))
                });
            let txid_s = txid.to_string();
            let id = wallet.to_string();
            let locks = this.appdb_op(move |db| db.locks(&id)).await?;
            let dust_locked_outputs = locks
                .into_iter()
                .filter(|row| {
                    row.txid == txid_s && row.reason == LockReason::Dust && row.is_locked()
                })
                .filter(|row| (row.vout as usize) < entry.tx.output.len())
                .map(|row| OutPoint::new(txid, row.vout))
                .collect();
            let withheld_outputs = {
                let wm = live.manager.wallet_manager_arc();
                let wm = wm.read().await;
                (wm.get_wallet_info(&wallet.0))
                    .map(|i| this.hub.excluded_coins(wallet, &i.core_wallet))
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(o, r)| {
                        o.txid == txid && *r == crate::ExclusionReason::PendingUnproven
                    })
                    .map(|(o, _)| o)
                    .collect()
            };
            Ok(TxDetailExtras {
                txid: txid.to_string(),
                is_coinbase: f.coinbase,
                total_credit: f.credit,
                total_debit: (!debit_unknown).then_some(f.debit),
                inputs_unresolved: f.inputs_unresolved,
                net: i64::try_from(f.credit).unwrap_or(i64::MAX)
                    - i64::try_from(f.debit).unwrap_or(i64::MAX),
                matures_in: f.matures_in,
                in_mempool: None,
                abandoned: f.abandoned,
                can_abandon: f.abandon_refusal().is_none(),
                can_resend: f.resend_refusal().is_none()
                    || this.can_restart_abandoned_sweep(wallet, txid, &f).await,
                dust_locked_outputs,
                withheld_outputs,
                checking_spent_coins: f.abandoned && {
                    let held = this.hub.rescan_held(&wallet);
                    (entry.tx.input.iter()).any(|i| held.contains(&i.previous_output))
                },
                last_announced_at: this.hub.announced_at(&txid),
                restore_held: (this.hub.history)
                    .read_wallet(&wallet, |h| h.restore_holds_in_effect().contains_key(&txid)),
            })
        })
        .await
    }

    /// dash-qt "Abandon transaction" (QT-091). Refusals as
    /// [`TxActionRefusal`]; the abandoned transaction and its recorded
    /// descendants show as `Abandoned`, their change leaves the balance and
    /// the coins they spent come back through a rescan.
    ///
    /// Needs SPV running or rebuilding (`SpvNotRunning`, checked first):
    /// the spent coins return only through the filter rescan. A rebuild
    /// waits for the wallet's rewind before starting the replacement.
    pub async fn abandon_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.refuse_tx_action_while_restored()?;
            // Not while a wallet loads: its spend claims are read before
            // they are restored.
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            this.require_wallet(&wallet)?;
            let spv_running = this.spv_can_abandon(&manager);
            if !spv_running {
                return Err(EngineError::SpvNotRunning);
            }
            let (_, f) = this.tx_facts(&manager, wallet, txid, false).await?;
            if let Some(r) = activity_abandon_refusal(spv_running, &f) {
                return Err(r.into());
            }
            // Its rewind record stays stored until its rescan starts, as a
            // Remove Unconfirmed batch's does: a sync commit meanwhile
            // would otherwise delete it, and a stop then lose the scan.
            let _batch = this.hub.abandon_batch(wallet);
            this.abandon_inner(&manager, wallet, txid).await?;
            Ok(())
        })
        .await
    }

    /// Abandons `root` and its recorded descendants (checks done by the
    /// caller). Returns the abandoned txids. An InstantSend-locked root goes
    /// only by [`Self::lock_override`], and loses its lock in the history
    /// too, as the store's sweep drops it.
    pub(crate) async fn abandon_inner(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        self.abandon_inner_funding(manager, wallet, root, &BTreeSet::new())
            .await
    }

    /// [`Self::abandon_inner`] that may abandon the funding of the tracked
    /// locks `own`: Repair's own stuck-funding locks
    /// (`platform/lease/repair.rs`), never an Activity Abandon. A planned
    /// transaction funding any other pending lock is still refused.
    pub(crate) async fn abandon_inner_funding(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
        own: &BTreeSet<Txid>,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let mut recorded = None;
        let result = self
            .abandon_unrewound(manager, wallet, root, None, own, &mut recorded)
            .await;
        let abandoned = result.as_ref().ok().cloned().unwrap_or_default();
        #[cfg(test)]
        if (self.hub.quit_before_rescan).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit before the rescan".into()));
        }
        #[cfg(test)]
        let pause = self.hub.pause_before_rescan.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause.mark_reached();
            pause.release.notified().await;
        }
        // A failure leaves the abandon before its sweep is stored: nothing
        // is released, and a rewind it recorded runs at the next start.
        if result.is_ok() {
            self.finish_abandons(manager, wallet, &abandoned, recorded)
                .await?;
        }
        result
    }

    /// [`Self::abandon_inner`] up to the rescan: the rewind is recorded,
    /// not made, and `recorded` lowered to the checkpoint it starts from,
    /// failure or not. Returns the abandoned txids. `dead_drop`: when the
    /// abandon drops transactions that can never confirm (#739). `own`:
    /// tracked locks whose funding it may abandon
    /// ([`Self::abandon_inner_funding`]).
    async fn abandon_unrewound(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
        dead_drop: Option<&DeadDrop>,
        own: &BTreeSet<Txid>,
        recorded: &mut Option<u32>,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        #[cfg(test)]
        let pause = match (self.hub.pause_abandon_skip).fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_sub(1),
        ) {
            Ok(_) => None,
            Err(_) => self.hub.pause_abandon.lock().unwrap().take(),
        };
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let mut funding_gates = Vec::with_capacity(own.len());
        for txid in own {
            funding_gates.push(self.asset_lock_gate(wallet, *txid).write_owned().await);
        }
        let proof = self.root_proof(manager, wallet, root).await;
        let _gate = self.abandon_gate(wallet).write_owned().await;
        // Planned on a copy under the lock that applies it, so the replay
        // record stored before it covers what is abandoned; held until the
        // sweep is stored, so no descendant is credited between the plan and
        // the sweep.
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let held_since = std::time::Instant::now();
        let (abandoned, released, overridden, txs, links, sent) = {
            let settled = {
                let infos = wm.get_all_wallet_infos().iter();
                self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)), [root])
            };
            let locks: Vec<platform_wallet::TrackedAssetLock> = (loaded_locks(&wm).into_iter())
                .filter(|l| !own.contains(&l.out_point.txid))
                .cloned()
                .collect();
            let info = wm
                .get_wallet_info_mut(&wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
            if let Some(d) = dead_drop
                && !(self.hub.history).read_wallet(&wallet, |h| {
                    d.event_marks
                        .iter()
                        .all(|(t, cause)| h.invalid_mark(t) == Some(*cause))
                        && (d.holds)(&h.txs, &info.core_wallet, &settled)
                })
            {
                return Err(EngineError::Internal(
                    "the drop's check no longer holds under the lock".into(),
                ));
            }
            let (descendants, mut links) = (self.hub.history).read(&wallet, |txs| {
                unmined_descendants(txs, &info.core_wallet, root)
            });
            // A dead parent is swept again with the rest: the store's open or
            // key-wallet's sweep already deleted its record and outputs, so
            // this only keeps its outputs from being released (#739).
            links.extend(dead_drop.into_iter().flat_map(|d| d.dead.iter().copied()));
            // Never one key-wallet holds settled, whatever the history
            // still says of it: its parents are the network's too.
            if let Some(refusal) = (descendants.iter().chain(&links)).find_map(|t| {
                settled
                    .direct
                    .get(t)
                    .copied()
                    .or_else(|| held_settled(&info.core_wallet, t))
            }) {
                return Err(EngineError::TxActionRefused(refusal));
            }
            let (planned, overridden) = self.plan_abandon(
                wallet,
                root,
                &descendants,
                &proof,
                &info.core_wallet,
                &settled,
            )?;
            // Repair's abandon of its own locks takes nothing else: no
            // descendant, and no dead parent.
            if !own.is_empty() && (!links.is_empty() || planned.iter().any(|t| !own.contains(t))) {
                return Err(EngineError::TxActionRefused(
                    TxActionRefusal::PlatformFunding,
                ));
            }
            // A drop's are dead: the network refuses a resume's send of one.
            if dead_drop.is_none() && funds_a_pending_lock(&locks, |t| planned.contains(t)) {
                return Err(EngineError::TxActionRefused(
                    TxActionRefusal::PlatformFunding,
                ));
            }
            // What key-wallet abandons and the history has not heard of is
            // marked, and its coins released, from key-wallet's copy in this
            // wallet's own accounts.
            let copies: HashMap<Txid, Transaction> = (info.core_wallet.accounts.all_accounts())
                .into_iter()
                .filter(|a| !a.managed_account_type().is_contact_owned())
                .flat_map(|a| a.transactions().iter())
                .filter(|(t, _)| planned.contains(*t))
                .map(|(t, r)| (*t, r.transaction.clone()))
                .collect();
            if let Some(d) = dead_drop {
                let veto = self.hub.history.read_wallet(&wallet, |h| {
                    let txs = &h.txs;
                    let parents = planned
                        .iter()
                        .chain(&links)
                        .filter_map(|t| txs.get(t).map(|e| &e.tx).or_else(|| copies.get(t)))
                        .flat_map(|tx| tx.input.iter().map(|i| i.previous_output.txid))
                        .filter(|t| h.invalid_mark(t).is_some());
                    d.known_dead
                        .keys()
                        .copied()
                        .chain(parents)
                        .any(|t| tree_settled(&info.core_wallet, &settled, &t))
                });
                if veto {
                    return Err(EngineError::TxActionRefused(
                        TxActionRefusal::SettledDescendant,
                    ));
                }
            }
            // A coin another transaction of the wallet still spends stays
            // spent, and is neither released nor rescanned for: one
            // key-wallet records and does not abandon (it keeps conflicting
            // mempool spends), or one the history holds and this abandon
            // leaves (it loads every stored record; key-wallet holds none
            // for a payment hydrated at load).
            let chainlock = self.chainlock_height(&info.core_wallet);
            let finalized: Vec<OutPoint> = info.finalized_spends.iter().copied().collect();
            // A ChainLocked block's spend outlives the note key-wallet prunes
            // and the record it drops. Releasing the wallet's own payment
            // must not give that coin back (#268, #400).
            let durable = self.chainlocked_block_spends(wallet, chainlock).await?;
            let mut still_spent: HashSet<OutPoint> = (info.core_wallet.accounts.all_accounts())
                .into_iter()
                .flat_map(|a| a.transactions().iter())
                .filter(|(t, _)| !planned.contains(*t))
                .flat_map(|(_, r)| r.transaction.input.iter().map(|i| i.previous_output))
                .collect();
            (self.hub.history).read(&wallet, |txs| {
                let left = (txs.iter()).filter(|(t, e)| {
                    !e.is_abandoned() && !planned.contains(*t) && !links.contains(*t)
                });
                still_spent
                    .extend(left.flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output)));
            });
            still_spent.extend(durable.keys().copied());
            still_spent.extend(finalized);
            // Read before key-wallet abandons anything (which leaves its
            // observed spends as they are), so the coins' holds go with the
            // marks.
            let mut no_outputs = links.clone();
            if let Some(d) = dead_drop {
                no_outputs.extend(d.known_dead.keys().copied());
            }
            self.hub.history.read_wallet(&wallet, |h| {
                no_outputs.extend(h.drops.keys().filter(|t| h.dead_claim(t).is_some()));
            });
            let mut released = overridden.clone().unwrap_or_else(|| {
                self.released(
                    wallet,
                    &planned,
                    &no_outputs,
                    &copies,
                    info.core_wallet.observed_spent_outpoints(),
                )
            });
            (self.hub.history).read(&wallet, |txs| {
                released.retain(|o| {
                    !still_spent.contains(o)
                        && !no_outputs.contains(&o.txid)
                        && !txs
                            .get(&o.txid)
                            .is_some_and(|e| pending(e) || e.is_abandoned())
                });
            });
            // The rescan covers every coin given back too: each waits for
            // it (#818).
            let checkpoint = [
                self.abandon_rewind(
                    wallet,
                    &planned,
                    &no_outputs,
                    &still_spent,
                    &info.core_wallet,
                ),
                self.rescan_from(wallet, &released, info),
            ]
            .into_iter()
            .flatten()
            .min();
            let mut holds: BTreeMap<OutPoint, RescanHold> = BTreeMap::new();
            if let Some(checkpoint) = checkpoint {
                let target = self.record_rewind_locked(wallet, info, checkpoint).await?;
                *recorded = (*recorded).into_iter().chain([checkpoint]).min();
                // No processed block's spend and no stored transaction is
                // evidence the coins are unspent: each waits for the rewind's
                // rescan (#818).
                holds = (released.iter())
                    .map(|o| (*o, RescanHold::new(checkpoint, target)))
                    .collect();
            }
            let txs: Vec<(Txid, Transaction)> = (planned.iter())
                .filter_map(|t| {
                    let tx = (self.hub.history.get(&wallet, t).map(|e| e.tx))
                        .or_else(|| copies.get(t).cloned())?;
                    Some((*t, tx))
                })
                .collect();
            // Durable before anything is applied: a later load must never
            // see the payment live with its coins released. An override's
            // root is marked with the rest: an exit before the sweep loads it
            // InstantSend-locked, as it was (a mark on a locked payment
            // abandons nothing), and the open's sweep of marked payments
            // passes over it and takes its unlocked descendants.
            let mut marks: Vec<(String, Option<String>)> = (txs.iter())
                .map(|(t, tx)| (format!("{ABANDONED_PREFIX}{t}"), Some(serialize_hex(tx))))
                .collect();
            let planned_release = released.iter().copied().collect();
            marks.extend(
                txs.iter()
                    .map(|(t, tx)| release_mark(*t, tx, &planned_release)),
            );
            let sent = self.sent_by(wallet, &planned, info).await?;
            // A drop of what can never confirm is recorded with the marks,
            // and its sent marks go in the same write: a stop before the
            // sweep leaves it for the open's sweep of marked payments and
            // the load's walk, never live (#739).
            if let Some(d) = dead_drop {
                for t in planned.iter().chain(&d.dead) {
                    let own = (self.hub.history).read_wallet(&wallet, |h| {
                        (h.txs.get(t).map(|e| e.own_inputs.clone()))
                            .filter(|own| !own.is_empty())
                            .or_else(|| h.drop_inputs.get(t).cloned())
                            .unwrap_or_default()
                    });
                    marks.push((
                        format!("{DROPPED_PREFIX}{t}"),
                        Some(drop_mark(d.cause_of(t), &own)),
                    ));
                }
                for t in &planned {
                    for prefix in [SENT_PREFIX, SENT_TX_PREFIX, SENT_IN_PREFIX] {
                        marks.push((format!("{prefix}{t}"), None));
                    }
                }
            }
            marks.extend(hold_marks(&holds));
            // The planned payments' restore holds stay, on disk and in
            // memory: once a payment is abandoned its hold holds nothing
            // (H2), and a refused abandon has nothing to put back.
            self.write_abandoned_marks(wallet, marks).await?;
            let previous = (self.hub.history).with_wallet(wallet, |h| {
                crate::rescan_hold::take_over(&mut h.rescan_holds, &holds)
            });
            #[cfg(test)]
            let refuse =
                (self.hub.refuse_next_abandon).swap(false, std::sync::atomic::Ordering::SeqCst);
            #[cfg(not(test))]
            let refuse = false;
            let abandoned = match refuse {
                true => BTreeSet::new(),
                false => {
                    self.hub.hold_rebroadcast_coins(wallet, &released);
                    abandon_with(&mut info.core_wallet, root, &descendants)
                }
            };
            if abandoned.is_empty() {
                #[cfg(test)]
                (self.hub.refused_abandon_undos).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.hub.history.with_wallet(wallet, |h| {
                    crate::rescan_hold::put_back(&mut h.rescan_holds, &holds, &previous)
                });
                let undo = crate::rescan_hold::undo_marks(&holds, &previous);
                #[cfg(test)]
                let failed =
                    (self.hub.fail_abandon_undo).swap(false, std::sync::atomic::Ordering::SeqCst);
                #[cfg(not(test))]
                let failed = false;
                let written = match failed {
                    true => Err(EngineError::Storage("abandon: test undo failure".into())),
                    false => self.write_marks(wallet, undo).await,
                };
                if let Err(e) = written {
                    tracing::warn!(%wallet, error = %e, "could not drop the hold marks of a refused abandon");
                }
            } else {
                info.core_wallet.update_balance();
            }
            // key-wallet took another set than planned: nothing planned is
            // stored released.
            if !abandoned.is_empty() && abandoned != planned {
                tracing::warn!(%wallet, %root, "key-wallet abandoned another set than planned: no coin is released");
                released.clear();
            }
            (abandoned, released, overridden.is_some(), txs, links, sent)
        };
        #[cfg(test)]
        let pause = self.hub.pause_abandon_sweep.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        // key-wallet refuses (returns nothing) when it holds the root as
        // settled; its own record of an InstantSend lock is that.
        if abandoned.is_empty() {
            return Err(EngineError::TxActionRefused(match overridden {
                true => TxActionRefusal::InstantLocked,
                false => TxActionRefusal::Confirmed,
            }));
        }
        #[cfg(test)]
        if (self.hub.quit_before_sweep).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit before the sweep".into()));
        }
        let live = self.live()?;
        let persister = Arc::clone(&live.persister);
        #[cfg(test)]
        {
            *self.hub.last_release.lock().unwrap() = Some(released.clone());
        }
        #[cfg(test)]
        let pause = self.hub.pause_sweep.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let sweep = SweepBatch {
            // The abandoned transactions passed through are swept again, so
            // an input of theirs is a dead parent's output, not a claim.
            txids: abandoned.iter().chain(&links).copied().collect(),
            // An input not released (a block spent it) stays stored spent,
            // attributed to the root.
            superseded_by: root,
            winner_mined_height: None,
            released_outpoints: released.clone(),
        };
        #[cfg(test)]
        if (self.hub.fail_abandon_sweep).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Storage("abandon: test sweep failure".into()));
        }
        tokio::task::spawn_blocking(move || {
            let changeset = PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    sweeps: vec![sweep],
                    ..Default::default()
                }),
                ..Default::default()
            };
            persister.store(wallet.0, changeset)?;
            persister.flush(wallet.0)?;
            #[cfg(test)]
            if sweep_stop::take(&*persister) {
                return Err(platform_wallet::changeset::PersistenceError::LockPoisoned);
            }
            Ok::<_, platform_wallet::changeset::PersistenceError>(())
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("abandon: {e}")))?;
        self.stop_rebroadcasts(wallet, sent.intersection(&abandoned).copied())
            .await;
        #[cfg(test)]
        {
            let mut fail = self.hub.fail_after_sweep_of.lock().unwrap();
            if *fail == Some(root) {
                *fail = None;
                return Err(EngineError::Storage(
                    "abandon: test failure after the sweep".into(),
                ));
            }
        }
        let held = held_since.elapsed();
        tracing::debug!(%wallet, held_ms = held.as_millis() as u64, "an abandon held the wallet manager through its sweep");
        #[cfg(test)]
        {
            *self.hub.last_abandon_hold.lock().unwrap() = Some(held);
        }

        self.hub.history.with_wallet(wallet, |h| {
            let unheard: Vec<(Txid, Transaction)> = (txs.iter())
                .filter(|(t, _)| !h.txs.contains_key(t))
                .cloned()
                .collect();
            h.mark_abandoned(&unheard);
            for t in &abandoned {
                if let Some(e) = h.txs.get_mut(t) {
                    e.abandoned = true;
                    if overridden && *t == root {
                        e.instant_locked = false;
                        h.fork_kept_locks.remove(t);
                        if matches!(e.context, TransactionContext::InstantSend(_)) {
                            e.context = TransactionContext::Mempool;
                        }
                    }
                }
            }
            h.abandoned_here.extend(abandoned.iter().copied());
            if let Some(d) = dead_drop {
                for t in &abandoned {
                    h.drops.insert(*t, DropInfo::new(d.cause_of(t)));
                    h.checking_claims.remove(t);
                }
            }
        });
        drop(wm);
        self.hub.refresh_restore_held(wallet);
        (self.hub.history).drop_claims(&wallet, abandoned.iter().copied());
        // Each abandoned transaction's own claim (DEC-179); a coin another
        // owner holds stays reserved.
        for t in &abandoned {
            self.spends
                .release(&wallet, crate::send::SpendOwner::Tx(*t));
        }
        Ok(abandoned)
    }

    /// [`beaten_unrecorded`] of `wallet` under the manager's read lock,
    /// each with its coins key-wallet holds as spent, at their blocks'
    /// heights, when its winner is a block's spend the wallet does not know
    /// ([`unknown_block`]); the truncations the hub had applied and
    /// key-wallet's last processed height; and, with `stored`, the
    /// [`orphaned_unmined`] ones. `None` while the history is not whole.
    async fn find_dead(
        &self,
        manager: &Manager,
        wallet: WalletId,
        stored: Option<(&HashSet<Txid>, &HashSet<Txid>)>,
    ) -> Option<FoundDead> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&wallet.0)?;
        let core = &info.core_wallet;
        let chainlock = self.chainlock_height(core);
        let truncations = self.hub.truncations();
        let recorded =
            |t: &Txid| (core.accounts.all_accounts().into_iter()).any(|a| a.has_transaction(t));
        let mut owned = None;
        let pays_wallet = |tx: &Transaction| {
            let owned = owned.get_or_insert_with(|| crate::history_ops::owned_scripts(info));
            (tx.output.iter()).any(|o| owned.contains_key(&o.script_pubkey))
        };
        (self.hub.history).read_wallet(&wallet, |h| {
            if h.load_incomplete || !h.loaded {
                return None;
            }
            let observed = core.observed_spent_outpoints();
            let beaten = beaten_unrecorded(&h.txs, recorded, observed, chainlock, pays_wallet);
            let beaten = (beaten.into_iter())
                .map(|(t, w)| {
                    let spent = match (unknown_block(w), h.txs.get(&t)) {
                        (Some(_), Some(e)) => (e.tx.input.iter())
                            .map(|i| i.previous_output)
                            .filter_map(|o| observed.get(&o).map(|h| (o, *h)))
                            .collect(),
                        _ => BTreeMap::new(),
                    };
                    (t, w, spent)
                })
                .collect();
            let orphans = stored.map(|(s, exempt)| {
                let orphans = orphaned_unmined(&h.txs, s, exempt);
                let gone = gone_parents(&h.txs, s, &orphans);
                (orphans, gone)
            });
            Some((truncations, core.last_processed_height(), beaten, orphans))
        })
    }

    /// The height of the wallet's own applied ChainLock when the header
    /// chain holds its block, and the block `(height, hash)` at or below it
    /// (Q89a C1), with the height of the applied ChainLock the proof read;
    /// read apart from the manager's lock.
    async fn block_proven_final(
        &self,
        manager: &Manager,
        wallet: WalletId,
        (height, hash): (u32, BlockHash),
    ) -> (Option<u32>, Option<u32>) {
        let chainlock = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (wm.get_wallet_info(&wallet.0))
                .and_then(|i| i.core_wallet.metadata.last_applied_chain_lock.as_ref())
                .map(|l| (l.block_height, l.block_hash))
        };
        let Some(chainlock) = chainlock else {
            return (None, None);
        };
        let headers = (self.headers_under(chainlock, BTreeSet::from([height]))).await;
        let proven = headers.filter(|headers| {
            crate::history::proven_final((height, hash), Some(chainlock), headers)
        });
        (proven.map(|_| chainlock.0), Some(chainlock.0))
    }

    /// Block spends of `wallet` a ChainLock covers, including ones the
    /// history holds and the store does not yet.
    ///
    /// Key-wallet drops the in-memory note at the ChainLock and, with
    /// finalized records off, the spender's record. The store's row is what
    /// a later release and the next load honour, so a coin a final block
    /// spent is not offered again (#268, #400). A row already stored is
    /// left as it is.
    async fn chainlocked_block_spends(
        &self,
        wallet: WalletId,
        chainlock: Option<u32>,
    ) -> Result<BTreeMap<OutPoint, platform_wallet::changeset::BlockSpend>, EngineError> {
        use platform_wallet::changeset::BlockSpend;

        let wanted: Vec<BlockSpend> = match chainlock {
            None => Vec::new(),
            Some(cl) => self.hub.history.read(&wallet, |txs| {
                let created: HashSet<Txid> = txs.keys().copied().collect();
                let mut wanted = Vec::new();
                for (txid, e) in txs {
                    if e.is_abandoned() || !crate::history::chain_locked(&e.context, Some(cl)) {
                        continue;
                    }
                    let Some(block) = e.context.block_info() else {
                        continue;
                    };
                    for input in &e.tx.input {
                        let outpoint = input.previous_output;
                        if !created.contains(&outpoint.txid) || outpoint.txid == *txid {
                            continue;
                        }
                        wanted.push(BlockSpend {
                            outpoint,
                            height: block.height(),
                            block_hash: block.block_hash(),
                            spender_txid: *txid,
                        });
                    }
                }
                wanted
            }),
        };
        let persister = Arc::clone(&self.live()?.persister);
        let wallet_id = wallet.0;
        tokio::task::spawn_blocking(move || {
            let storage = |e: &dyn std::fmt::Display| {
                EngineError::Storage(format!("reading block spends: {e}"))
            };
            persister.flush(wallet_id).map_err(|e| storage(&e))?;
            let mut stored = persister
                .block_spent_outpoints(&wallet_id)
                .map_err(|e| storage(&e))?;
            for spend in wanted {
                if stored.contains_key(&spend.outpoint) {
                    continue;
                }
                persister
                    .record_found_block_spend(&wallet_id, spend)
                    .map_err(|e| EngineError::Storage(format!("recording a block spend: {e}")))?;
                stored.insert(spend.outpoint, spend);
            }
            Ok(stored)
        })
        .await?
    }

    /// Drops what can never confirm and key-wallet's conflict handling
    /// left (#170 cases 3 and 4), one drop at a time: the children of the
    /// payments a sweep took, which its event queued with its winner; the
    /// payments [`beaten_unrecorded`] finds; and, at a load that read every
    /// stored transaction (`stored`, with the parents whose children are
    /// the claim rule's), the [`orphaned_unmined`] ones, whose winner is
    /// unknown. Reads under the manager's read lock, and takes the write
    /// lock only to drop something.
    pub(crate) async fn drop_dead(
        &self,
        manager: &Manager,
        wallet: WalletId,
        stored: Option<(&HashSet<Txid>, &HashSet<Txid>)>,
    ) {
        let Some(registration) = self.hub.drop_registration(wallet) else {
            return;
        };
        let _gate = self.hub.drop_gate.lock().await;
        if self.hub.drop_registration(wallet) != Some(registration) {
            return;
        }
        let truncations = self.hub.truncations();
        // Key-wallet's processed height before the take, and before
        // `carried_drops` reads anything: a fork the hub has not counted
        // yet moves it (#891).
        let processed = processed_height(manager, wallet).await;
        let take = (truncations, processed);
        // A sweep's children queued before a truncation may have lost their
        // winner (#891).
        let children = (self.hub.take_dead_children(wallet, registration).into_iter()).filter(|(t, (roots, ..))| {
            let stale = *t != truncations;
            if stale {
                tracing::warn!(%wallet, ?roots, "a swept payment's children are not dropped after a truncation");
            }
            !stale
        });
        let mut work: VecDeque<_> = children
            .map(|(_, (roots, winner, swept, took))| {
                (roots, winner, swept, None, (BTreeSet::new(), took))
            })
            .collect();
        // A drop with no recheck that waited (for another wallet's open, or
        // for the chain to be read again) goes on only while what it was
        // found at holds: a fork since may have undone its winner or brought
        // its gone parent back, and it waits for the next sweep or orphan
        // pass to be found again (#891). A beaten root's recheck reads its
        // winner again under the lock.
        for (was, w) in self.hub.take_deferred_drops(wallet, registration) {
            if w.3.is_some() || self.take_holds(was, w.1, take).await {
                work.push_back(w);
            } else {
                tracing::warn!(%wallet, roots = ?w.0, ?was, ?take, "a waiting drop is void: the chain it was found on may have changed");
            }
        }
        // The blocks the header chain holds, with the take, at the heights
        // the drops with no recheck anchor to (#891): the processed height
        // too, for the orphans `queue_dead` finds below. Read with key-wallet
        // where it was at the take; if it moved meanwhile, a drop that waits
        // is stamped with no height, and void at its next take.
        let (stamped, anchors) = match processed {
            Some(p) => {
                let unrechecked = (work.iter()).filter(|w| w.3.is_none());
                let heights = unrechecked.map(|(_, winner, ..)| take_anchor(p, *winner));
                let anchors = self.hub.header_hashes(heights.chain([p]).collect()).await;
                match processed_height(manager, wallet).await == processed {
                    true => (processed, anchors),
                    false => (None, HashMap::new()),
                }
            }
            None => (None, HashMap::new()),
        };
        let found_at = |winner: Winner| {
            let anchor = stamped.and_then(|p| anchors.get(&take_anchor(p, winner)).copied());
            (truncations, stamped, anchor)
        };
        work.extend(self.carried_drops(wallet, registration, truncations).await);
        let all = stored;
        let mut stored = stored;
        for _ in 0..=DETECTIONS {
            self.queue_dead(manager, wallet, stored.take(), &mut work)
                .await;
            let mut again = false;
            while let Some((roots, winner, links, recheck, (gone, took))) = work.pop_front() {
                let mut out = Rechecked::default();
                let dropped = self.drop_dead_unmined(
                    manager,
                    wallet,
                    &roots,
                    (winner, &gone, &took),
                    (&links, truncations, processed, registration),
                    (recheck.as_ref(), &mut out),
                );
                if let Err(e) = dropped.await {
                    tracing::warn!(%wallet, error = %e, "could not drop transactions that can never confirm")
                }
                // Only key-wallet's height rose since the take: the drop
                // waits with what it was found at, and the next take decides
                // it (`Self::take_holds`).
                if out.put_off {
                    let w = (roots, winner, links, recheck, (gone, took));
                    self.hub
                        .defer_drops(wallet, registration, [(found_at(winner), w)]);
                    continue;
                }
                // Another wallet's history is not whole: this drop and the
                // rest wait, with their evidence, for its open to end.
                // The held report covers the drops done before it; the
                // sweep notes stay until a load whose drops all went.
                if out.deferred {
                    work.push_front((roots, winner, links, recheck, (gone, took)));
                    let work = work.into_iter().map(|w| (found_at(w.1), w));
                    self.hub.defer_drops(wallet, registration, work);
                    self.refresh_held(manager, wallet).await;
                    return;
                }
                again |= out.regrouped;
                match recheck.filter(|_| !out.retry.is_empty()) {
                    Some(r) if r.tries < RECHECKS => {
                        let r = Recheck {
                            tries: r.tries + 1,
                            ..r
                        };
                        work.push_back((
                            out.retry,
                            winner,
                            BTreeSet::new(),
                            Some(r),
                            Default::default(),
                        ));
                    }
                    Some(r) => match unknown_block(winner) {
                        // Kept, with the detection's evidence, for a drop an
                        // applied ChainLock at the block starts (Q99).
                        // One a fork or another header voided meanwhile is not.
                        Some(at) if self.carried_void(&r, at, self.hub.truncations()).await => {
                            tracing::info!(%wallet, roots = ?out.retry, ?winner, "beaten payments whose block's proof is missing are void");
                        }
                        Some(at) => {
                            tracing::info!(%wallet, roots = ?out.retry, ?winner, "beaten payments wait for their block's proof");
                            let r = Recheck { tries: 0, ..r };
                            let drop = (winner, r, at, out.applied);
                            self.hub.carry_drops(wallet, registration, &out.retry, drop);
                            let now = self.applied_chain_lock(manager, wallet).await;
                            if now.is_some_and(|n| n >= at && out.applied.is_none_or(|s| n > s)) {
                                self.hub.schedule_reconcile(wallet);
                            }
                        }
                        None => {
                            tracing::warn!(%wallet, roots = ?out.retry, ?winner, "beaten payments not dropped: their recheck could not finish")
                        }
                    },
                    None => {}
                }
            }
            if !again {
                break;
            }
        }
        if let Some((stored, _)) = all {
            self.prune_sweep_notes(wallet, stored).await;
        }
        self.refresh_held(manager, wallet).await;
    }

    /// Whether a drop with no recheck found at `was` may go on at `now`
    /// (#891): no truncation since, and key-wallet's processed height
    /// where it was or risen, with the header chain still holding the block
    /// it held at the anchor height then. A height that fell, or a header
    /// not read where one was, does not hold; with no anchor read, only an
    /// unmoved height does.
    async fn take_holds(
        &self,
        (truncations, processed, anchor): crate::events::DropTake,
        winner: Winner,
        now: (u64, Option<u32>),
    ) -> bool {
        if truncations != now.0 {
            return false;
        }
        match (processed, now.1, anchor) {
            (Some(was), Some(is), Some(hash)) if is >= was => {
                self.hub.header_hash(take_anchor(was, winner)).await == Some(hash)
            }
            _ => processed == now.1 && anchor.is_none(),
        }
    }

    /// The drops that wait are taken again, with the evidence they were
    /// found on (#818, S1), each going only while what it was found at
    /// holds (#891): after any drop that saw another wallet's open, or the
    /// unload or removal that ended it, under way has put itself to wait.
    /// Those of unloaded wallets went with them.
    pub(crate) async fn drain_deferred_drops(&self, manager: &Manager) {
        drop(self.hub.drop_gate.lock().await);
        let Ok(live) = self.live() else {
            return;
        };
        for wallet in self.hub.deferred_drop_wallets() {
            if !live.store.is_unloaded(&wallet.0) {
                self.drop_dead(manager, wallet, None).await;
            }
        }
    }

    /// Hooks [`Self::drain_deferred_drops`] to the hub's unload and removal
    /// of a wallet: run on its own task, so it holds none of the caller's
    /// locks.
    pub(crate) fn attach_drain_drops(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.attach_drain_drops(Box::new(move || {
            let task = weak.clone();
            let started = rt.spawn(async move {
                let Some(session) = task.upgrade() else {
                    return;
                };
                if let (Ok(_op), Ok(manager)) = (session.try_enter(), session.manager()) {
                    session.drain_deferred_drops(&manager).await;
                }
            });
            #[cfg(test)]
            if let Some(session) = weak.upgrade() {
                (session.hub.drains.lock().unwrap()).push(started);
            }
            #[cfg(not(test))]
            drop(started);
        }));
    }

    /// The beaten payments `wallet` kept to recheck, grouped by their original
    /// detection evidence, except those voided by a truncation or
    /// a header at its block's height that differs from its block voids
    /// (Q99 K3).
    async fn carried_drops(
        &self,
        wallet: WalletId,
        registration: u64,
        truncations: u64,
    ) -> Vec<DeadWork> {
        let mut work = Vec::new();
        for (roots, winner, recheck, at) in
            group_carried_drops(self.hub.take_carried_drops(wallet, registration))
        {
            if self.carried_void(&recheck, at, truncations).await {
                tracing::info!(%wallet, ?roots, "beaten payments kept to recheck are void");
                continue;
            }
            work.push((
                roots,
                winner,
                BTreeSet::new(),
                Some(recheck),
                Default::default(),
            ));
        }
        work
    }

    /// Whether a truncation since `r`'s detection, or a header at its
    /// block's height `at` that is another block, voids a beaten payment
    /// kept to recheck (Q99 K3).
    async fn carried_void(&self, r: &Recheck, at: u32, truncations: u64) -> bool {
        r.truncations != truncations
            || (self.hub.header_hash(at).await).is_some_and(|h| r.block != Some(h))
    }

    /// The height of the wallet's applied ChainLock.
    async fn applied_chain_lock(&self, manager: &Manager, wallet: WalletId) -> Option<u32> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&wallet.0)?;
        info.core_wallet
            .last_applied_chain_lock()
            .map(|l| l.block_height)
    }

    /// Queues the payments [`Self::find_dead`] finds beaten in `wallet`, by
    /// winner, each group with its detection's evidence ([`Recheck`]), and
    /// with `stored` the orphans.
    async fn queue_dead(
        &self,
        manager: &Manager,
        wallet: WalletId,
        stored: Option<(&HashSet<Txid>, &HashSet<Txid>)>,
        work: &mut VecDeque<DeadWork>,
    ) {
        let Some((truncations, processed, beaten, mut orphans)) =
            self.find_dead(manager, wallet, stored).await
        else {
            return;
        };
        let heights: BTreeSet<u32> = (beaten.iter())
            .filter_map(|(_, w, _)| unknown_block(*w))
            .collect();
        let mut found = Some((truncations, processed, beaten));
        let mut headers = HashMap::new();
        if !heights.is_empty() {
            // The header chain's hashes at the heights of the blocks whose
            // spends key-wallet holds under the next read: a fork truncates
            // the wallet before the header chain, so each is its block's
            // (Q89a C1). A truncation the hub applied since the first read
            // binds none. The orphans are read again too: a parent may be
            // back.
            headers = self.hub.header_hashes(heights).await;
            found = (self.find_dead(manager, wallet, stored).await).map(
                |(again, processed, beaten, now)| {
                    if again != truncations {
                        headers.clear();
                    }
                    orphans = now;
                    (truncations, processed, beaten)
                },
            );
        }
        if let Some((truncations, processed, beaten)) = found {
            let mut groups: BTreeMap<Winner, (Vec<Txid>, BTreeMap<OutPoint, u32>)> =
                BTreeMap::new();
            for (t, w, spent) in beaten {
                let group = groups.entry(w).or_default();
                group.0.push(t);
                group.1.extend(spent);
            }
            work.extend((groups.into_iter()).map(|(w, (roots, spent))| {
                let block = unknown_block(w).and_then(|h| headers.get(&h).copied());
                let recheck = Recheck {
                    truncations,
                    processed,
                    block,
                    spent: Arc::new(spent),
                    tries: 0,
                };
                (roots, w, BTreeSet::new(), Some(recheck), Default::default())
            }));
        }
        if let Some((orphans, gone)) = orphans.filter(|(o, _)| !o.is_empty()) {
            let origin = (gone, BTreeSet::new());
            work.push_back((orphans, Winner::default(), BTreeSet::new(), None, origin));
        }
    }

    /// Deletes, at a load that read every stored transaction (`stored`),
    /// the sweep notes no drop needs (Q81d S6): of a transaction stored or
    /// live again, and of one that no drop names as gone and whose outputs
    /// no transaction of the history and none of the drops' chains spends,
    /// so no load can find a child of it orphaned and a chain's walk keeps
    /// what it reads ([`obsolete_sweep_note`]). A note a drop names stays.
    async fn prune_sweep_notes(&self, wallet: WalletId, stored: &HashSet<Txid>) {
        let (notes, claims) = match self
            .appdb_op(move |db| Ok((drop_notes(db, wallet)?, claims_swept(db, wallet)?)))
            .await
        {
            Ok(notes) => notes,
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "could not read the drops' notes");
                return;
            }
        };
        let keys: Vec<String> = (self.hub.history).read(&wallet, |txs| {
            let obsolete = obsolete_sweep_note(&notes, txs, stored);
            notes
                .deaths
                .swept_by
                .keys()
                .filter(|t| obsolete(t))
                .map(|t| format!("{SWEPT_BY_PREFIX}{t}"))
                .chain(
                    claims
                        .iter()
                        .filter(|t| obsolete(t))
                        .map(|t| format!("{CLAIM_SWEPT_PREFIX}{t}")),
                )
                .collect()
        });
        if !keys.is_empty() {
            self.keep_notes(wallet, keys.into_iter().map(|k| (k, None)).collect())
                .await;
        }
    }

    /// Reads again what wallet `wallet`'s drops hold (the coins their notes
    /// list, which the drops claim for good), [`crate::history::held_total`]
    /// and [`crate::history::held_awaiting`], and publishes them with the
    /// balances when they changed. Runs after each reconcile of the
    /// wallet's blocks and at its load. On the way it keeps the spend notes
    /// (Q81b S3): one for each spend key-wallet observes of a coin the drops
    /// hold or contest, or of an input of an orphan's gone parent, in a
    /// block the SPV header chain holds; a note the chain contradicts goes,
    /// and one not final above the height of a fork found since the last
    /// run. It keeps the sweep notes' blocks and the lock notes too. The
    /// notes and the report free and claim nothing.
    pub(crate) async fn refresh_held(&self, manager: &Manager, wallet: WalletId) {
        if self.hub.wallet_state(&wallet).is_none() {
            return;
        }
        let held = match self.hub.dropped_holds(wallet).is_empty() {
            true => (0, 0, false),
            false => match self.held_report(manager, wallet).await {
                Some(held) => held,
                None => return,
            },
        };
        let changed = self.hub.update_wallet_state(wallet, |s| {
            let changed = s.held != held;
            s.held = held;
            changed
        });
        if changed {
            self.hub.pump.mark_balances(wallet);
        }
    }

    /// [`Self::refresh_held`]'s report: the held total, the coins of unknown
    /// value, and whether one awaits: a held coin, or one a block not yet
    /// final spent. When the notes don't read, the last total, awaiting.
    /// `None` when the wallet is gone. On the way it keeps the sweep notes (Q81d S4-S6)
    /// and the lock notes (Q81c C1).
    async fn held_report(&self, manager: &Manager, wallet: WalletId) -> Option<(u64, u32, bool)> {
        use crate::history::{SpendNote, proven_final};
        type Block = (u32, BlockHash);
        let notes = (self.appdb_op(move |db| {
            #[cfg(all(test, debug_assertions))]
            db.fault_point(HELD_REPORT_FAULT, dw_appdb::fault::Stage::Before)?;
            drop_notes(db, wallet)
        }))
        .await;
        let DropNotes {
            drops,
            mut spent,
            mut locked,
            mut locked_unread,
            locked_astray,
            mut deaths,
        } = match notes {
            Ok(notes) => notes,
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "could not read the drops' notes");
                let (sum, unvalued, _) =
                    (self.hub.wallet_state(&wallet)).map_or((0, 0, false), |s| s.held);
                return Some((sum, unvalued, true));
            }
        };
        let origins = || drops.values().filter_map(|(_, origin)| origin.as_ref());
        let gone: BTreeSet<Txid> = origins().flat_map(|o| o.gone.iter().copied()).collect();
        let holds: BTreeSet<OutPoint> = (drops.values())
            .flat_map(|(held, _)| held.iter().copied())
            .collect();
        let network_locks = self.hub.held_locks(&holds);
        let watched: BTreeSet<OutPoint> = (holds.iter().copied())
            .chain(origins().flat_map(|o| o.contested.iter().copied()))
            .chain(gone.iter().flat_map(|p| deaths.inputs_of(p)))
            .collect();
        // What key-wallet holds of the locks it heard and the noted ones:
        // each locked transaction's record, in any account, with its block.
        let (seen, chainlock, heard, recorded) = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let core = &wm.get_wallet_info(&wallet.0)?.core_wallet;
            let observed = core.observed_spent_outpoints();
            let seen: BTreeMap<OutPoint, u32> = (watched.iter())
                .filter_map(|o| observed.get(o).map(|h| (*o, *h)))
                .collect();
            let chainlock = (core.metadata.last_applied_chain_lock.as_ref())
                .map(|c| (c.block_height, c.block_hash));
            let heard = core.instant_send_locks().clone();
            let noted: HashSet<&Txid> = (locked.values().flatten())
                .chain(&heard)
                .chain(network_locks.keys())
                .collect();
            let recorded: HashMap<Txid, (Transaction, Option<Block>)> =
                (core.accounts.all_accounts().into_iter())
                    .flat_map(|a| a.transactions().iter())
                    .filter(|(t, _)| noted.contains(t))
                    .map(|(t, r)| {
                        let block = (r.context.block_info()).map(|b| (b.height(), b.block_hash()));
                        (*t, (r.transaction.clone(), block))
                    })
                    .collect();
            (seen, chainlock, heard, recorded)
        };
        let forked = self.hub.take_held_fork(wallet);
        let locks: BTreeSet<Txid> = (locked.values().flatten())
            .chain(&heard)
            .chain(network_locks.keys())
            .copied()
            .collect();
        // Of each locked transaction any loaded wallet's history or
        // key-wallet holds, its block and the blocks of the transactions
        // spending one of its inputs: either proven final settles it.
        let lock_blocks: HashMap<Txid, Vec<Block>> = (self.hub.history).read_all(|all| {
            let blocks = |tx: &Transaction, own: Option<Block>, t: &Txid| -> Vec<Block> {
                let inputs: HashSet<OutPoint> =
                    tx.input.iter().map(|i| i.previous_output).collect();
                let rivals = (all.iter().flat_map(|txs| txs.iter()))
                    .filter(|(w, we)| {
                        *w != t
                            && !we.is_abandoned()
                            && (we.tx.input.iter()).any(|i| inputs.contains(&i.previous_output))
                    })
                    .filter_map(|(_, we)| we.context.block_info())
                    .map(|b| (b.height(), b.block_hash()));
                own.into_iter().chain(rivals).collect()
            };
            (locks.iter())
                .filter_map(|t| {
                    let found = (all.iter().find_map(|txs| txs.get(t)))
                        .map(|e| {
                            let own = e.context.block_info().map(|b| (b.height(), b.block_hash()));
                            (e.tx.clone(), own)
                        })
                        .or_else(|| recorded.get(t).cloned())?;
                    Some((*t, blocks(&found.0, found.1, t)))
                })
                .collect()
        });
        // The winners' blocks the history holds, the swept transactions
        // live again, and the blocks of the history's spends of the coins
        // watched or noted, or of an input of the drops' chains.
        let (mined, live, spends): (HashMap<Txid, Block>, Vec<Txid>, Vec<Block>) =
            (self.hub.history).read(&wallet, |txs| {
                let mined = (deaths.swept_by.values())
                    .filter_map(|n| {
                        let b = txs.get(&n.winner)?.context.block_info()?;
                        Some((n.winner, (b.height(), b.block_hash())))
                    })
                    .collect();
                let live = (deaths.swept_by.keys())
                    .filter(|t| txs.get(*t).is_some_and(|e| !e.is_abandoned()))
                    .copied()
                    .collect();
                let noted = (watched.iter()).chain(locked.keys()).chain(&locked_unread);
                let coins = spends_read(noted, &drops, &deaths);
                let spends = (txs.values())
                    .filter(|e| (e.tx.input.iter()).any(|i| coins.contains(&i.previous_output)))
                    .filter_map(|e| e.context.block_info())
                    .map(|b| (b.height(), b.block_hash()))
                    .collect();
                (mined, live, spends)
            });
        let heights: BTreeSet<u32> = (spent.values().map(|n| n.height))
            .chain(seen.values().copied())
            .chain(
                deaths
                    .swept_by
                    .values()
                    .filter_map(|n| n.block.map(|b| b.0)),
            )
            .chain(mined.values().map(|b| b.0))
            .chain(lock_blocks.values().flatten().map(|b| b.0))
            .chain(spends.iter().map(|b| b.0))
            .collect();
        let under = match chainlock {
            Some(cl) => self.headers_under(cl, heights.clone()).await,
            None => None,
        };
        let headers = match &under {
            Some(headers) => headers.clone(),
            None => self.hub.header_hashes(heights).await,
        };
        let proven = |b: Block| (under.as_ref()).is_some_and(|h| proven_final(b, chainlock, h));
        let is_final = |n: &SpendNote| proven((n.height, n.block));
        let mut writes: Vec<(String, Option<String>)> = Vec::new();
        spent.retain(|coin, note| {
            let contradicted = headers.get(&note.height).is_some_and(|h| *h != note.block);
            let above_fork = forked.is_some_and(|f| note.height > f) && !is_final(note);
            let keep = !contradicted && !above_fork;
            if !keep {
                writes.push((format!("{DROP_SPENT_PREFIX}{coin}"), None));
            }
            keep
        });
        for t in live {
            deaths.swept_by.remove(&t);
            writes.push((format!("{SWEPT_BY_PREFIX}{t}"), None));
        }
        for (t, note) in deaths.swept_by.iter_mut().filter(|(_, n)| !n.claim) {
            let mined = mined.get(&note.winner).copied();
            let block = crate::history::swept_by_block(note, mined, &headers, forked);
            if block != note.block {
                note.block = block;
                writes.push((format!("{SWEPT_BY_PREFIX}{t}"), Some(note.encode())));
            }
        }
        let fresh: Vec<(OutPoint, u32, BlockHash)> = (seen.iter())
            .filter_map(|(coin, height)| Some((*coin, *height, *headers.get(height)?)))
            .filter(|(coin, height, block)| {
                !(spent.get(coin)).is_some_and(|n| n.height == *height && n.block == *block)
            })
            .collect();
        if !fresh.is_empty() {
            (self.hub.history).read(&wallet, |txs| {
                for (coin, height, block) in fresh {
                    let spender = (txs.iter())
                        .find(|(_, e)| {
                            (e.context.block_info())
                                .is_some_and(|b| b.height() == height && b.block_hash() == block)
                                && e.tx.input.iter().any(|i| i.previous_output == coin)
                        })
                        .map(|(t, _)| *t);
                    let note = SpendNote {
                        height,
                        block,
                        spender,
                    };
                    writes.push((format!("{DROP_SPENT_PREFIX}{coin}"), Some(note.encode())));
                    spent.insert(coin, note);
                }
            });
        }
        if !self.keep_notes(wallet, writes).await
            && let Some(height) = forked
        {
            self.hub.note_held_fork(wallet, height);
        }
        let proven_spends: BTreeSet<OutPoint> = (spent.iter())
            .filter(|(_, n)| is_final(n))
            .map(|(o, _)| *o)
            .collect();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&wallet.0)?;
        let core = &info.core_wallet;
        let chainlock_height = self.chainlock_height(core);
        let observed = core.observed_spent_outpoints();
        let owned = crate::history_ops::owned_scripts(info);
        let by_record: HashSet<OutPoint> = (core.accounts.all_accounts().into_iter())
            .filter(|a| !a.managed_account_type().is_contact_owned())
            .flat_map(|a| a.transactions().values())
            .flat_map(|r| r.transaction.input.iter().map(|i| i.previous_output))
            .filter(|o| holds.contains(o))
            .collect();
        let (report, lock_writes) = (self.hub.history).read(&wallet, |txs| {
            let left_out = |o: &OutPoint| proven_spends.contains(o) || gone.contains(&o.txid);
            let held = crate::history::still_held(&holds, txs, observed, left_out);
            let (sum, unvalued) = crate::history::held_total(&held, txs, |s| owned.contains_key(s));
            let dead_for_good =
                |p: &Txid| crate::history::dead_for_good(p, &deaths, txs, &spent, proven);
            // A lock is settled once its transaction is learned and its
            // block or a rival's spend of an input is proven final, or the
            // transaction is dead for good.
            let settled_lock = |t: &Txid| {
                (lock_blocks.get(t)).is_some_and(|b| b.iter().copied().any(proven))
                    || dead_for_good(t)
            };
            let outstanding =
                |t: &&Txid| !txs.contains_key(*t) && !recorded.contains_key(*t) && !settled_lock(t);
            // A lock heard on the network names the coins it spends; one
            // only key-wallet heard may spend any held coin.
            let unknown: BTreeSet<Txid> = (heard.iter())
                .filter(|t| !network_locks.contains_key(*t))
                .filter(outstanding)
                .copied()
                .collect();
            let network_unknown: BTreeMap<OutPoint, BTreeSet<Txid>> = (network_locks.iter())
                .filter(|(t, _)| outstanding(t))
                .flat_map(|(t, spent)| spent.iter().map(move |o| (*o, *t)))
                .fold(BTreeMap::new(), |mut by_coin, (o, t)| {
                    by_coin.entry(o).or_default().insert(t);
                    by_coin
                });
            let final_spend = |o: &OutPoint| {
                proven_spends.contains(o)
                    || (observed.get(o))
                        .is_some_and(|h| chainlock_height.is_some_and(|cl| *h <= cl))
            };
            let spent_finally = |o: &OutPoint| {
                final_spend(o)
                    || (txs.values()).any(|e| {
                        !e.is_abandoned()
                            && (e.context.block_info())
                                .is_some_and(|b| proven((b.height(), b.block_hash())))
                            && e.tx.input.iter().any(|i| i.previous_output == *o)
                    })
            };
            let mut lock_writes: Vec<(String, Option<String>)> = Vec::new();
            let key = |coin: &OutPoint| format!("{DROP_LOCKED_PREFIX}{coin}");
            locked_unread.retain(|coin| {
                let keep = holds.contains(coin) && !spent_finally(coin);
                if !keep {
                    lock_writes.push((key(coin), None));
                }
                keep
            });
            let coins: BTreeSet<OutPoint> = (locked.keys().copied())
                .chain(held.iter().copied().filter(|_| !unknown.is_empty()))
                .chain(network_unknown.keys().copied().filter(|c| held.contains(c)))
                .filter(|c| !locked_unread.contains(c))
                .collect();
            for coin in coins {
                let before = locked.get(&coin).cloned().unwrap_or_default();
                let after: BTreeSet<Txid> = match !holds.contains(&coin) || spent_finally(&coin) {
                    true => BTreeSet::new(),
                    false => (before.iter())
                        .filter(|t| !settled_lock(t))
                        .chain(unknown.iter().filter(|_| held.contains(&coin)))
                        .chain(
                            (network_unknown.get(&coin).into_iter().flatten())
                                .filter(|_| held.contains(&coin)),
                        )
                        .copied()
                        .collect(),
                };
                if after != before {
                    let value = (!after.is_empty()).then(|| {
                        (after.iter().map(|t| t.to_string()))
                            .collect::<Vec<_>>()
                            .join(",")
                    });
                    lock_writes.push((key(&coin), value));
                }
                match after.is_empty() {
                    true => locked.remove(&coin),
                    false => locked.insert(coin, after),
                };
            }
            let chain_final = |e: &crate::history::TxEntry| {
                (e.context.block_info()).is_some_and(|b| proven((b.height(), b.block_hash())))
            };
            // Each drop key's verdict, judged once per report.
            let dead_keys = std::cell::RefCell::new(HashMap::new());
            let evidence = crate::history::HeldEvidence {
                chain_final: &chain_final,
                final_spend: &final_spend,
                spent_by_record: &|o| by_record.contains(o),
                unknown_lock: &|o| {
                    locked_astray
                        || !unknown.is_empty()
                        || locked.contains_key(o)
                        || locked_unread.contains(o)
                },
                dead_for_good: &dead_for_good,
                drop_dead: &|t| {
                    if let Some(dead) = dead_keys.borrow().get(t) {
                        return *dead;
                    }
                    let ancestor = |p: &Txid| crate::history::chain_parent(p, &drops, &deaths);
                    let dead =
                        crate::history::chain_dead_for_good(t, &deaths, ancestor, dead_for_good);
                    dead_keys.borrow_mut().insert(*t, dead);
                    dead
                },
            };
            // A coin out of `held` for a spend in a block not yet final
            // awaits that block (#749).
            let unsettled_spend = (holds.iter())
                .filter(|o| !held.contains(o) && !gone.contains(&o.txid))
                .any(|o| !spent_finally(o));
            let awaiting =
                unsettled_spend || crate::history::held_awaiting(&held, &drops, txs, &evidence);
            ((sum, unvalued, awaiting), lock_writes)
        });
        drop(wm);
        self.keep_notes(wallet, lock_writes).await;
        Some(report)
    }

    /// Writes the held report's `writes` to wallet `wallet`'s scope in one
    /// durable write; false when it failed.
    async fn keep_notes(&self, wallet: WalletId, writes: Vec<(String, Option<String>)>) -> bool {
        if writes.is_empty() {
            return true;
        }
        let scope = wallet.to_string();
        let written = self
            .appdb_op(move |db| {
                let writes: Vec<(&str, Option<&str>)> = (writes.iter())
                    .map(|(k, v)| (k.as_str(), v.as_deref()))
                    .collect();
                db.set_settings_durable(&scope, &writes)
            })
            .await;
        if let Err(e) = &written {
            tracing::warn!(%wallet, error = %e, "could not keep the drops' notes");
        }
        written.is_ok()
    }

    /// Drops `roots` and what descends from them ([`plan_drop`]) as an
    /// abandon does: marked abandoned and dropped first, then abandoned in
    /// key-wallet and swept from the store; the history shows them
    /// abandoned, as their marks do at the next load, and their sent marks
    /// go. A coin they spent comes back only through the rewind, which is
    /// recorded before the marks and made after the sweep, and only when
    /// the plan frees it. The sweep records those releases; the rescan hold
    /// keeps them out of spending until that scan finishes. Every other coin
    /// they spent is claimed for good. Holds the wallet's abandon gate for
    /// writing through its marks, as an abandon does, so no resend hands off
    /// bytes it has marked. Returns the dropped txids.
    ///
    /// With `recheck`, `roots` were found beaten under the read lock, with
    /// that detection's evidence, and are found again under the write
    /// lock, each with what beats it now: the call drops one winner's
    /// roots, and the drop finds the others again with their winner now
    /// (Q89 R1); `out` says so. A root `winner` beat by a block's spend
    /// that key-wallet no longer holds keeps `winner` when that block is
    /// proven final ([`spend_forgotten_as_final`]), and its drop plans with
    /// that spend (Q89a C3); when only the proof is missing it goes to
    /// `out` to recheck. While another wallet's history is not whole,
    /// nothing goes and `out` says the drop waits.
    async fn drop_dead_unmined(
        &self,
        manager: &Manager,
        wallet: WalletId,
        roots: &[Txid],
        (winner, gone, took): (Winner, &BTreeSet<Txid>, &BTreeSet<OutPoint>),
        (links, since, processed, registration): (&BTreeSet<Txid>, u64, Option<u32>, u64),
        (recheck, out): (Option<&Recheck>, &mut Rechecked),
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let unrechecked = recheck.is_none();
        #[cfg(test)]
        let pause = match (unrechecked, links.is_empty()) {
            (true, false) => self.hub.pause_drop_child.lock().unwrap().take(),
            (true, true) => self.hub.pause_drop_orphan.lock().unwrap().take(),
            (false, _) => self.hub.pause_drop_recheck.lock().unwrap().take(),
        };
        #[cfg(test)]
        if let Some(pause) = &pause {
            pause.mark_reached();
            pause.release.notified().await;
        }
        // The detection's block proven final, read apart from the lock: a
        // truncation below the ChainLock it reads is refused.
        let (proven, read) = match (recheck, unknown_block(winner)) {
            (Some(r), Some(at)) => match r.block {
                Some(block) => (self.block_proven_final(manager, wallet, (at, block))).await,
                None => (None, None),
            },
            _ => (None, None),
        };
        #[cfg(test)]
        let pause = (recheck.is_some())
            .then(|| self.hub.pause_drop_proven.lock().unwrap().take())
            .flatten()
            .or_else(|| {
                (recheck.is_some_and(|r| r.tries == RECHECKS))
                    .then(|| self.hub.pause_drop_proven_last.lock().unwrap().take())
                    .flatten()
            });
        #[cfg(test)]
        if let Some(pause) = &pause {
            pause.mark_reached();
            pause.release.notified().await;
        }
        let _gate = self.abandon_gate(wallet).write_owned().await;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        if self.hub.drop_registration(wallet) != Some(registration) {
            return Ok(BTreeSet::new());
        }
        // What any loaded wallet settled can descend from this one's
        // payment (#611): nothing goes while another's history is not whole
        // (an incomplete load, or an open under way); this one's own
        // incomplete load frees nothing (`plan_drop`).
        let other_unread = (wm.get_all_wallet_infos().keys())
            .filter(|id| **id != wallet.0)
            .any(|id| self.history_unread(WalletId(*id)));
        if other_unread {
            out.deferred = true;
            return Ok(BTreeSet::new());
        }
        let settled = {
            let infos = wm.get_all_wallet_infos().iter();
            self.settled_all(
                infos.map(|(id, i)| (id, &i.core_wallet)),
                roots.iter().chain(links).chain(gone).copied(),
            )
        };
        let Some(info) = wm.get_wallet_info_mut(&wallet.0) else {
            return Ok(BTreeSet::new());
        };
        // A fork key-wallet truncated and the hub has not heard of yet
        // counts as one since the detection or the take.
        self.hub.detect_truncation(wallet, &info.core_wallet);
        let truncations = self.hub.truncations();
        // A drop with no recheck (a sweep's children, or an orphan) goes
        // on what it was taken on only while no fork has happened since and
        // every swept payment or gone parent is still gone (#891): a fork
        // may have undone the sweep's winner, and one that is mined, or
        // that the history holds again and no longer abandons, may confirm.
        // Key-wallet's processed height moving since the take covers a fork
        // the hub has not counted yet.
        if unrechecked {
            let core = &info.core_wallet;
            let held = |t: &Txid| {
                settled.direct.contains_key(t)
                    || held_settled(core, t).is_some()
                    || (!core.is_dismissed(t)
                        && core
                            .accounts
                            .all_accounts()
                            .iter()
                            .any(|a| a.has_transaction(t)))
                    || (self.hub.history.get(&wallet, t)).is_some_and(|e| !e.is_abandoned())
            };
            let held_again: Vec<Txid> = (links.iter().chain(gone))
                .copied()
                .filter(|t| held(t))
                .collect();
            let moved = processed != Some(core.last_processed_height());
            if truncations != since || moved || !held_again.is_empty() {
                // Only key-wallet's height rose: the drop decides once it
                // has read the chain again (`NetworkSession::drop_dead`). A
                // height that fell is a fork key-wallet applied before the
                // header chain did.
                let rose = processed.is_some_and(|p| core.last_processed_height() > p);
                out.put_off = truncations == since && held_again.is_empty() && rose;
                tracing::warn!(%wallet, ?roots, ?held_again, moved, put_off = out.put_off, "a drop with no recheck does not go: a fork, block or lock since its take may have made it valid");
                return Ok(BTreeSet::new());
            }
        }
        // A ChainLock prunes key-wallet's note of the spend and may drop the
        // spender's record. The stored row, written here when the history
        // still has the block, is what keeps the coin spent (#268, #400).
        let chainlock_now = self.chainlock_height(&info.core_wallet);
        let durable_spends = self.chainlocked_block_spends(wallet, chainlock_now).await?;
        let (plan, winner, forgotten) = (self.hub.history).read_wallet(&wallet, |h| {
            let complete = h.loaded && !h.load_incomplete;
            let core = &info.core_wallet;
            let chainlock = self.chainlock_height(core);
            let swept_spends_may_be_gone = unrechecked && !links.is_empty()
                && winner.mined.is_some_and(|won| {
                    won <= core.synced_height()
                        && (chainlock.is_some_and(|cl| won <= cl)
                            || !core.observed_spent_outpoints().values().any(|h| *h == won)
                                && !durable_spends.values().any(|spend| {
                                    spend.height == won && Some(spend.spender_txid) == winner.txid
                                }))
                });
            let facts = (complete && !swept_spends_may_be_gone, chainlock, &settled);
            // A beaten root is beaten still under this lock, by what beats
            // it now: a truncation since its detection may have undone its
            // winner, and a block or a record since may have named another.
            let rechecked: Vec<Txid>;
            let mut forgotten = durable_spends
                .into_iter()
                .map(|(outpoint, spend)| (outpoint, spend.height))
                .collect::<BTreeMap<_, _>>();
            // Same session, before that row is written: the ChainLock already
            // dropped the record and the note.
            let at_lock = chainlock.unwrap_or(0);
            for outpoint in &info.finalized_spends {
                forgotten.entry(*outpoint).or_insert(at_lock);
            }
            let (roots, winner) = match recheck {
                None => (roots, winner),
                Some(r) => {
                    let core = &info.core_wallet;
                    let recorded = |t: &Txid| {
                        (core.accounts.all_accounts().into_iter()).any(|a| a.has_transaction(t))
                    };
                    let owned = crate::history_ops::owned_scripts(info);
                    let pays = |tx: &Transaction| {
                        (tx.output.iter()).any(|o| owned.contains_key(&o.script_pubkey))
                    };
                    let observed = core.observed_spent_outpoints();
                    let now: HashMap<Txid, Winner> =
                        beaten_unrecorded(&h.txs, recorded, observed, chainlock, pays)
                            .into_iter()
                            .collect();
                    let mut groups: BTreeMap<Winner, Vec<Txid>> = BTreeMap::new();
                    let applied = core.last_applied_chain_lock().map(|l| l.block_height);
                    // What a payment kept to recheck saw is what its proof
                    // read: a ChainLock applied after that starts its drop.
                    out.applied = read;
                    let still_final =
                        proven.is_some_and(|c| applied.is_some_and(|l| l >= c));
                    for t in roots {
                        if let Some(w) = now.get(t) {
                            groups.entry(*w).or_default().push(*t);
                            continue;
                        }
                        let bound = r.block.is_some() && r.truncations == truncations;
                        if r.block.is_some() && !bound && !r.spent.is_empty() {
                            tracing::warn!(%wallet, root = %t, "a beaten payment's detected spend is void after a truncation");
                        }
                        let spent = match bound && !recorded(t) {
                            true => spend_forgotten_as_final(&h.txs, core, t, winner, r),
                            false => BTreeMap::new(),
                        };
                        if spent.is_empty() {
                            continue;
                        }
                        // A ChainLock applied after the proof was read is
                        // read again.
                        if !still_final {
                            out.retry.push(*t);
                            continue;
                        }
                        forgotten.extend(spent);
                        groups.entry(winner).or_default().push(*t);
                    }
                    // The detection's group goes first: what it keeps on
                    // forgotten evidence is planned with that evidence.
                    let (now, first) = match groups.remove(&winner) {
                        Some(first) => (winner, first),
                        None => groups.pop_first().unwrap_or((winner, Vec::new())),
                    };
                    out.regrouped |= !groups.is_empty();
                    if now != winner && !first.is_empty() {
                        tracing::debug!(%wallet, ?winner, ?now, "a beaten payment's winner changed before its drop");
                    }
                    rechecked = first;
                    (&rechecked[..], now)
                }
            };
            // What the winner took: the roots' inputs, and the swept
            // payments', which their sweep read before the history forgot
            // them (`took`), or the history holds.
            let contested = (roots.iter().chain(links))
                .filter_map(|t| h.txs.get(t))
                .flat_map(|e| e.tx.input.iter().map(|i| i.previous_output))
                .chain(took.iter().copied())
                .collect();
            let origin = crate::history::DropOrigin {
                winner: winner.txid,
                contested,
                gone: gone.clone(),
            };
            let plan = plan_drop(
                &h.txs,
                &h.swept,
                (&info.core_wallet, &forgotten),
                roots,
                winner,
                links,
                facts,
            );
            (plan.map(|plan| (plan, origin)), winner, forgotten)
        });
        let Some((plan, origin)) = plan else {
            return Ok(BTreeSet::new());
        };
        let origin = origin.encode();
        let mut checkpoint = None;
        let mut rescan_holds = BTreeMap::new();
        if !plan.freed.is_empty() {
            checkpoint = [
                self.abandon_rewind(
                    wallet,
                    &plan.dead,
                    &plan.links,
                    &plan.kept,
                    &info.core_wallet,
                ),
                self.rescan_from(wallet, &plan.freed, info),
            ]
            .into_iter()
            .flatten()
            .min();
            if let Some(checkpoint) = checkpoint {
                let target = self.record_rewind_locked(wallet, info, checkpoint).await?;
                // What the drop frees no processed block spent and no stored
                // transaction spends: it waits for the rewind's rescan (#818).
                rescan_holds = (plan.freed.iter())
                    .map(|o| (*o, RescanHold::new(checkpoint, target)))
                    .collect();
            }
        }
        let persister = Arc::clone(&self.live()?.persister);
        // A mined winner's block spends its coins again in any rescan; the
        // coins an unmined or unknown winner, or a locked transaction out of
        // a block, may spend, and every coin no processed block spent when
        // the winner's inputs are unknown, are held at each load.
        let holds = |tx: &Transaction| {
            let held = (tx.input.iter()).map(|i| i.previous_output).filter(|o| {
                (winner.mined.is_none()
                    || plan.locked_spent.contains(o)
                    || (!plan.known && !plan.observed.contains(o)))
                    && plan.inputs.contains(o)
                    && !plan.freed.contains(o)
            });
            held.map(|o| o.to_string()).collect::<Vec<_>>().join(",")
        };
        let marks = (plan.copies.iter())
            .flat_map(|(t, tx)| {
                [
                    (format!("{ABANDONED_PREFIX}{t}"), Some(serialize_hex(tx))),
                    release_mark(*t, tx, &plan.freed),
                    (format!("{DROP_HOLDS_PREFIX}{t}"), Some(holds(tx))),
                    (format!("{DROP_WINNER_PREFIX}{t}"), Some(origin.clone())),
                ]
            })
            .chain(hold_marks(&rescan_holds))
            .collect();
        let noted: Vec<OutPoint> = (plan.copies.iter())
            .flat_map(|(_, tx)| {
                let held = holds(tx);
                (held.split(',').filter_map(|o| OutPoint::from_str(o).ok())).collect::<Vec<_>>()
            })
            .collect();
        self.write_abandoned_marks(wallet, marks).await?;
        let previous = (self.hub.history).with_wallet(wallet, |h| {
            crate::rescan_hold::take_over(&mut h.rescan_holds, &rescan_holds)
        });
        let mut dead = plan.dead.iter().copied();
        let root = dead.next().expect("a plan drops something");
        let rest: Vec<Txid> = dead.collect();
        let core = &mut info.core_wallet;
        #[cfg(test)]
        let refused = (self.hub.refuse_drops).load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(test)]
        if refused {
            (self.hub.drops_refused).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        #[cfg(not(test))]
        let refused = false;
        let abandoned = match refused {
            true => BTreeSet::new(),
            false => {
                self.hub.hold_rebroadcast_coins(
                    wallet,
                    &plan.freed.iter().copied().collect::<Vec<_>>(),
                );
                abandon_with(core, root, &rest)
            }
        };
        if abandoned.is_empty() {
            // key-wallet refused (it settled the root meanwhile): the marks
            // go, so no hold outlives a drop that did not happen.
            self.hub.history.with_wallet(wallet, |h| {
                crate::rescan_hold::put_back(&mut h.rescan_holds, &rescan_holds, &previous)
            });
            let undo: Vec<(String, Option<String>)> = (plan.copies.iter())
                .flat_map(|(t, _)| {
                    [
                        (format!("{ABANDONED_PREFIX}{t}"), None),
                        (format!("{ABANDON_RELEASE_PREFIX}{t}"), None),
                        (format!("{DROP_HOLDS_PREFIX}{t}"), None),
                        (format!("{DROP_WINNER_PREFIX}{t}"), None),
                    ]
                })
                .chain(crate::rescan_hold::undo_marks(&rescan_holds, &previous))
                .collect();
            let scope = wallet.to_string();
            let (gone, dropped): (BTreeSet<Txid>, BTreeSet<Txid>) =
                (gone.clone(), plan.copies.iter().map(|(t, _)| *t).collect());
            self.appdb_op(move |db| {
                let mut rows = undo;
                // An orphan's drop goes with the sweep notes of the parents
                // no other drop names as gone (Q81d S6).
                if !gone.is_empty() {
                    let notes = drop_notes(db, wallet)?;
                    let named: BTreeSet<&Txid> = (notes.drops.iter())
                        .filter(|(t, _)| !dropped.contains(*t))
                        .filter_map(|(_, (_, origin))| origin.as_ref())
                        .flat_map(|o| &o.gone)
                        .collect();
                    rows.extend(
                        (gone.iter())
                            .filter(|p| !named.contains(p))
                            .map(|p| (format!("{SWEPT_BY_PREFIX}{p}"), None)),
                    );
                }
                let writes: Vec<(&str, Option<&str>)> = (rows.iter())
                    .map(|(k, v)| (k.as_str(), v.as_deref()))
                    .collect();
                db.set_settings_durable(&scope, &writes)
            })
            .await?;
            return Ok(abandoned);
        }
        // Only a drop that happened holds: what another drop holds stays.
        self.hub.add_dropped_holds(wallet, noted);
        // key-wallet took another than planned: nothing comes back.
        let freed = match abandoned == plan.dead {
            true => plan.freed.clone(),
            false => {
                tracing::warn!(%wallet, %root, "key-wallet dropped another set than planned: no coin is freed");
                BTreeSet::new()
            }
        };
        let claims: Vec<(OutPoint, Option<Txid>)> = (plan.inputs.iter())
            .filter(|o| !freed.contains(*o))
            .map(|o| (*o, None))
            .collect();
        let held = crate::history_ops::restore_spent_claims(info, &claims);
        let core = &mut info.core_wallet;
        for account in core.accounts.all_funding_accounts_mut() {
            for o in &held {
                account.utxos.remove(o);
            }
        }
        core.update_balance();
        // The winner's height stamps the held coins only when its block
        // spent each of them: a stamped placeholder goes once final.
        let observed = core.observed_spent_outpoints();
        let all_won =
            (claims.iter()).all(|(o, _)| observed.contains_key(o) || forgotten.contains_key(o));
        let sweep = SweepBatch {
            txids: abandoned.iter().chain(&plan.links).copied().collect(),
            superseded_by: winner.txid.unwrap_or(root),
            winner_mined_height: winner.mined.filter(|_| all_won),
            released_outpoints: freed.into_iter().collect(),
        };
        #[cfg(test)]
        if (self.hub.quit_before_sweep).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit before the sweep".into()));
        }
        tokio::task::spawn_blocking(move || {
            let changeset = PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    sweeps: vec![sweep],
                    ..Default::default()
                }),
                ..Default::default()
            };
            persister.store(wallet.0, changeset)?;
            persister.flush(wallet.0)
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("dropping a dead transaction: {e}")))?;
        self.stop_rebroadcasts(wallet, abandoned.iter().copied())
            .await;
        self.hub.history.with_wallet(wallet, |h| {
            let unheard: Vec<(Txid, Transaction)> = (plan.copies.iter())
                .filter(|(t, _)| abandoned.contains(t) && !h.txs.contains_key(t))
                .cloned()
                .collect();
            h.mark_abandoned(&unheard);
            for t in &abandoned {
                if let Some(e) = h.txs.get_mut(t) {
                    e.abandoned = true;
                }
                h.swept.insert(*t);
            }
            h.abandoned_here.extend(abandoned.iter().copied());
        });
        drop(wm);
        let gone: Vec<Txid> = abandoned.iter().copied().collect();
        let forgotten = self
            .appdb_op(move |db| forget_sent(db, wallet, &gone))
            .await;
        match forgotten {
            Ok(()) => (self.hub.history).drop_claims(&wallet, abandoned.iter().copied()),
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "could not drop the marks of dropped payments")
            }
        }
        for t in &abandoned {
            self.spends
                .release(&wallet, crate::send::SpendOwner::Tx(*t));
        }
        self.finish_abandons(manager, wallet, &abandoned, checkpoint)
            .await?;
        tracing::info!(%wallet, dropped = ?abandoned, ?winner, "dropped transactions that can never confirm");
        Ok(abandoned)
    }

    /// Plans abandoning `root` and `descendants` on a copy of `core`: the
    /// txids key-wallet would abandon, and the coins the ChainLock
    /// exception releases (`None` without it). Refuses as the abandon
    /// would.
    fn plan_abandon(
        &self,
        wallet: WalletId,
        root: Txid,
        descendants: &[Txid],
        proof: &ChainProof,
        core: &ManagedWalletInfo,
        settled: &Settled,
    ) -> Result<(BTreeSet<Txid>, Option<Vec<OutPoint>>), EngineError> {
        let mut trial = core.clone();
        let planned = abandon_with(&mut trial, root, descendants);
        if matches!(settled.direct.get(&root), Some(TxActionRefusal::Confirmed)) {
            return Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed));
        }
        let uncertain = self.hub.history.read_wallet(&wallet, |h| {
            std::iter::once(&root)
                .chain(&planned)
                .any(|t| h.uncertain_fork_spends.contains_key(t))
        });
        if uncertain {
            return Err(EngineError::TxActionRefused(TxActionRefusal::RescanFirst));
        }
        // key-wallet holds no record of a payment restored at load, so
        // it cannot see the InstantSend lock rehydrated for it.
        let overridden =
            self.lock_override(wallet, &root, &planned, proof, core, &trial, &settled.under)?;
        let exempt = |t: &Txid| overridden.is_some() && *t == root;
        let (mined, held_locked) = self.hub.history.read_wallet(&wallet, |h| {
            let mut mined = false;
            let mut held_locked = false;
            for t in std::iter::once(&root)
                .chain(&planned)
                .filter(|t| !exempt(t))
            {
                mined |= settled.direct.get(t) == Some(&TxActionRefusal::Confirmed);
                held_locked |= settled.direct.get(t) == Some(&TxActionRefusal::InstantLocked)
                    || core.instant_send_locks().contains(t)
                    || h.txs
                        .get(t)
                        .is_some_and(|entry| h.knows_instant_lock(t, entry));
            }
            (mined, held_locked)
        });
        if mined {
            return Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed));
        }
        if held_locked {
            return Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked));
        }
        // While a stored record of any loaded wallet failed to read, what
        // descends from the root is not known: one settled could be behind
        // it (#611). Only a root the ChainLock exception proved dead, which
        // this wallet's own incomplete load already refuses, goes ahead.
        if let (None, Some(at)) = (&overridden, settled.incomplete) {
            return Err(EngineError::TxActionRefused(
                TxActionRefusal::RecordUnreadable(at),
            ));
        }
        // A descendant settled anywhere, in this wallet or another loaded
        // one, which the walk passes over: the network holds the root too
        // (#446).
        if overridden.is_none()
            && (self.hub.history).read(&wallet, |txs| {
                settled_descendant(txs, settled, &root, proof)
            })
        {
            return Err(EngineError::TxActionRefused(
                TxActionRefusal::SettledDescendant,
            ));
        }
        Ok((planned, overridden))
    }

    /// [`Self::chain_proof`] when `root` is InstantSend-locked, in the
    /// history, the session's verified locks or by key-wallet's locks:
    /// only then does the exception read it.
    async fn root_proof(&self, manager: &Manager, wallet: WalletId, root: Txid) -> ChainProof {
        let locked = match self.hub.history.read_wallet(&wallet, |h| {
            h.txs.get(&root).map(|e| h.knows_instant_lock(&root, e))
        }) {
            None => false,
            Some(true) => true,
            Some(false) => {
                let wm = manager.wallet_manager_arc();
                let wm = wm.read().await;
                (wm.get_wallet_info(&wallet.0)).is_some_and(|i| {
                    i.core_wallet.instant_send_locks().contains(&root) || {
                        let infos = wm.get_all_wallet_infos().iter();
                        let settled =
                            self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)), [root]);
                        settled.under.contains(&root)
                    }
                })
            }
        };
        match locked {
            true => self.chain_proof(manager, wallet).await,
            false => ChainProof::default(),
        }
    }

    /// Rescans `wallet` from `checkpoint`, after abandons of `abandoned`
    /// recorded it: the released coins come back when the filter scan
    /// passes their funding blocks again.
    async fn finish_abandons(
        &self,
        manager: &Manager,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        checkpoint: Option<u32>,
    ) -> Result<(), EngineError> {
        if let Some(checkpoint) = checkpoint {
            self.rewind_recorded(manager, wallet, checkpoint).await?;
        }
        self.refresh_wallet_state(manager, wallet).await;
        let changed: Vec<Txid> = abandoned.iter().copied().collect();
        self.hub.pump.mark_history(wallet, Some(&changed));
        self.hub.pump.mark_balances(wallet);
        // A masternode operation whose transaction this abandoned holds its
        // coins no longer (MG-10 §1.10, D49). The mark is durable, so a
        // release that fails here runs again at the next settling.
        if let Err(e) = self.settle_operation_locks(wallet).await {
            tracing::warn!(%wallet, error = %e, "releasing an abandoned ProTx operation's coins");
        }
        Ok(())
    }

    /// The ChainLock exception (#170 case 14) for `root`, whose abandon
    /// plans `planned`, when it is InstantSend-locked: a transaction
    /// in a block of `proof`'s ChainLocked chain spends one of its inputs,
    /// nothing settled descends from it, the history loaded every stored
    /// record and holds each of `planned`, and the scan is past the
    /// ChainLock with no replay pending. Each input of `planned` that a
    /// recorded block spends must be spent in a block of that chain at or
    /// below the ChainLock, and stays stored spent, claimed by that record;
    /// each other one must be a coin key-wallet holds unspent from a
    /// scanned block once `planned` is abandoned (`trial`), and is
    /// released; `RescanFirst` when all that is missing is coins the root's
    /// own spend claim, restored at the load, holds (a descendant's comes
    /// back at the next load). `None` for a root
    /// without a lock, `InstantLocked` when the evidence falls short.
    #[expect(clippy::too_many_arguments, reason = "one plan's evidence")]
    fn lock_override(
        &self,
        wallet: WalletId,
        root: &Txid,
        planned: &BTreeSet<Txid>,
        proof: &ChainProof,
        core: &ManagedWalletInfo,
        trial: &ManagedWalletInfo,
        under: &HashSet<Txid>,
    ) -> Result<Option<Vec<OutPoint>>, EngineError> {
        let refused = Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked));
        self.hub.history.read_wallet(&wallet, |h| {
            let txs = &h.txs;
            let Some(e) = txs.get(root) else {
                return Ok(None);
            };
            if !h.knows_instant_lock(root, e) && !core.instant_send_locks().contains(root) {
                return Ok(None);
            }
            let synced = core.synced_height();
            let (Some((chainlock, _)), Some(state)) =
                (proof.chainlock, self.hub.wallet_state(&wallet))
            else {
                return refused;
            };
            let (live, links) = unmined_descendants(txs, core, *root);
            if !chainlock_beaten(txs, root, proof.chainlock, &proof.active)
                || !live.iter().all(|t| planned.contains(t))
                || override_blocked(under, core)(root)
                || h.load_incomplete
                || synced < chainlock
                || state.replaying_at(synced)
                || !planned.iter().all(|t| txs.contains_key(t))
            {
                return refused;
            }
            let proven = |w: &TxEntry| {
                (w.context.block_info()).is_some_and(|b| {
                    b.height() <= chainlock
                        && proof.active.get(&b.height()) == Some(&b.block_hash())
                })
            };
            let mut mined: HashMap<OutPoint, bool> = HashMap::new();
            for w in txs.values().filter(|w| w.context.block_info().is_some()) {
                for i in &w.tx.input {
                    *mined.entry(i.previous_output).or_insert(true) &= proven(w);
                }
            }
            let held: HashSet<OutPoint> = (trial.utxos().iter())
                .filter(|u| u.is_confirmed && u.height <= synced)
                .map(|u| u.outpoint)
                .collect();
            let inputs: BTreeSet<OutPoint> = (planned.iter())
                .filter_map(|t| txs.get(t))
                .flat_map(|p| p.tx.input.iter().map(|i| i.previous_output))
                .filter(|o| !planned.contains(&o.txid) && !links.contains(&o.txid))
                .collect();
            let (mut free, mut claim_held) = (Vec::new(), false);
            for o in inputs {
                match mined.get(&o) {
                    Some(true) => {}
                    Some(false) => return refused,
                    None if held.contains(&o) => free.push(o),
                    None if h.claimed.get(&o) == Some(root) => claim_held = true,
                    None => return refused,
                }
            }
            match claim_held {
                true => Err(EngineError::TxActionRefused(TxActionRefusal::RescanFirst)),
                false => Ok(Some(free)),
            }
        })
    }

    /// Writes `rows` (abandoned marks) in one durable write.
    async fn write_abandoned_marks(
        &self,
        wallet: WalletId,
        rows: Vec<(String, Option<String>)>,
    ) -> Result<(), EngineError> {
        if rows.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        if (self.hub.fail_abandon_mark).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Storage("abandon: test mark failure".into()));
        }
        self.write_marks(wallet, rows).await
    }

    /// Writes `rows` of `wallet`'s scope durably in one write (a `None`
    /// value deletes its key).
    pub(crate) async fn write_marks(
        &self,
        wallet: WalletId,
        rows: Vec<(String, Option<String>)>,
    ) -> Result<(), EngineError> {
        let scope = wallet.to_string();
        self.appdb_op(move |db| {
            let writes: Vec<(&str, Option<&str>)> = (rows.iter())
                .map(|(k, v)| (k.as_str(), v.as_deref()))
                .collect();
            db.set_settings_durable(&scope, &writes)
        })
        .await
    }

    /// What [`chainlock_beaten`] reads for `wallet`: the SPV header chain's
    /// block hash at each height [`rival_heights`] names, and the height of
    /// the wallet's ChainLock when that chain holds its block before and
    /// after those are read (a Rebuild Index keeps the ChainLock but not
    /// the headers), and the locked transactions [`override_blocked`] keeps
    /// Abandon off for. No ChainLock while the scan is below it or replays,
    /// or the history's load was incomplete.
    async fn chain_proof(&self, manager: &Manager, wallet: WalletId) -> ChainProof {
        let (chainlock, blocked) = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(core) = wm.get_wallet_info(&wallet.0).map(|i| &i.core_wallet) else {
                return ChainProof::default();
            };
            let chainlock = core.metadata.last_applied_chain_lock.clone();
            let synced = core.synced_height();
            let scanned = (self.hub.wallet_state(&wallet)).is_some_and(|s| !s.replaying_at(synced))
                && !self.hub.history.with_wallet(wallet, |h| h.load_incomplete);
            let Some(chainlock) = chainlock.filter(|c| scanned && synced >= c.block_height) else {
                return ChainProof::default();
            };
            let roots: Vec<Txid> = self.hub.history.read_wallet(&wallet, |h| {
                h.txs
                    .iter()
                    .filter(|(t, e)| e.context.block_info().is_none() && h.knows_instant_lock(t, e))
                    .map(|(t, _)| *t)
                    .collect()
            });
            if roots.is_empty() {
                return ChainProof::default();
            }
            let settled = {
                let infos = wm.get_all_wallet_infos().iter();
                self.settled_all(
                    infos.map(|(id, i)| (id, &i.core_wallet)),
                    roots.iter().copied(),
                )
            };
            let blocks = override_blocked(&settled.under, core);
            (chainlock, roots.into_iter().filter(|t| blocks(t)).collect())
        };
        let cl = (chainlock.block_height, chainlock.block_hash);
        let heights = self.hub.history.read(&wallet, rival_heights);
        let Some(active) = self.headers_under(cl, heights).await else {
            return ChainProof::default();
        };
        ChainProof {
            chainlock: Some(cl),
            active,
            blocked,
        }
    }

    /// The coins abandoning `abandoned` releases: the inputs of each (from
    /// the history, or key-wallet's copy of one it has not heard of) that
    /// none of them nor of the abandoned `links` between them created and
    /// no processed block spent (`observed`): key-wallet keeps the claim on
    /// a coin a block spent.
    fn released(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        links: &BTreeSet<Txid>,
        copies: &HashMap<Txid, Transaction>,
        observed: &BTreeMap<OutPoint, u32>,
    ) -> Vec<OutPoint> {
        (abandoned.iter())
            .filter_map(|t| {
                (self.hub.history.get(&wallet, t).map(|e| e.tx)).or_else(|| copies.get(t).cloned())
            })
            .flat_map(|tx| tx.input.into_iter().map(|i| i.previous_output))
            .filter(|o| {
                !abandoned.contains(&o.txid)
                    && !links.contains(&o.txid)
                    && !observed.contains_key(o)
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The checkpoint abandoning `abandoned` rewinds to: below the lowest
    /// funding height of the wallet's coins it releases, or below the birth
    /// height when a coin's funding block is unknown. Each transaction's
    /// coins are the union of the wallet-owned inputs its key-wallet records
    /// name and those the history's record names (key-wallet holds none for
    /// a send an earlier session recorded, and may name fewer); one that
    /// neither holds replays from the birth height. No coins released, no
    /// rewind. A coin of the abandoned `links` between them is none, and so
    /// is one another transaction still spends (`still_spent`): it stays
    /// spent, so nothing needs to be scanned for it again.
    fn abandon_rewind(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        links: &BTreeSet<Txid>,
        still_spent: &HashSet<OutPoint>,
        info: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    ) -> Option<u32> {
        let birth = info.birth_height().saturating_sub(1);
        let own = |tx: &Transaction, indexes: &mut dyn Iterator<Item = u32>| -> Vec<OutPoint> {
            indexes
                .filter_map(|i| tx.input.get(i as usize))
                .map(|i| i.previous_output)
                .collect()
        };
        let mut released = BTreeSet::new();
        for t in abandoned {
            let records: Vec<_> = (info.accounts.all_accounts().into_iter())
                .filter_map(|a| a.transactions().get(t))
                .collect();
            let entry = self.hub.history.get(&wallet, t);
            let marked = (self.hub.history).read_wallet(&wallet, |h| h.marked.get(t).cloned());
            if records.is_empty() && entry.is_none() {
                return Some(birth);
            }
            let recorded = (records.iter())
                .flat_map(|r| own(&r.transaction, &mut r.input_details.iter().map(|i| i.index)));
            let held = (entry.iter()).flat_map(|e| own(&e.tx, &mut e.own_inputs.keys().copied()));
            released.extend(
                recorded
                    .chain(held)
                    .chain(marked.into_iter().flatten())
                    .filter(|o| {
                        !abandoned.contains(&o.txid)
                            && !links.contains(&o.txid)
                            && !still_spent.contains(o)
                    }),
            );
        }
        let mut lowest = None::<u32>;
        for o in &released {
            let Some(funding) = self.hub.history.get(&wallet, &o.txid) else {
                return Some(birth);
            };
            if funding.is_abandoned() {
                continue;
            }
            if let Some(h) = funding.context.block_info().map(|b| b.height()) {
                lowest = Some(lowest.map_or(h, |l| l.min(h)));
            } else if locked(&funding) {
                return Some(birth);
            }
        }
        lowest.map(|h| h.saturating_sub(1))
    }

    /// The checkpoint a rescan of the wallet's coins among `coins` starts
    /// from: below the lowest block funding one. A coin the history holds
    /// no block of is none key-wallet can give back as processed.
    fn rescan_from<'a>(
        &self,
        wallet: WalletId,
        coins: impl IntoIterator<Item = &'a OutPoint>,
        info: &platform_wallet::PlatformWalletInfo,
    ) -> Option<u32> {
        let owned = crate::history_ops::owned_scripts(info);
        (coins.into_iter())
            .filter_map(|o| {
                let funding = self.hub.history.get(&wallet, &o.txid)?;
                let out = funding.tx.output.get(o.vout as usize)?;
                owned.contains_key(&out.script_pubkey).then_some(())?;
                funding.context.block_info().map(|b| b.height())
            })
            .min()
            .map(|h| h.saturating_sub(1))
    }

    /// Restores retries when the rebuilt client gets a peer. The sent
    /// mark identifies Desktop's own send; a pending wallet-store record
    /// proves it reached the wallet pipeline and has not since been swept.
    pub(crate) async fn resume_rebroadcasts(self: &Arc<Self>) {
        use std::sync::atomic::Ordering;

        let Ok(_op) = self.enter().await else {
            return;
        };
        let Ok(manager) = self.manager() else {
            return;
        };
        // A reconnect during a failing attempt must see the generation it rearms.
        let _replay = self.rebroadcast_replay_gate.lock().await;
        let generation = manager.spv().run_session();
        if generation == 0
            || self
                .rebroadcast_replay
                .compare_exchange(generation, 0, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        let mut restored = HashSet::new();
        let mut replay = async || {
            self.restore_pending_rebroadcasts(&manager, generation, &mut restored)
                .await
        };
        let mut result = replay().await;
        if matches!(result, Err(EngineError::Storage(_))) {
            // A brief read failure can clear while the peer stays connected.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if manager.spv().run_session() != generation {
                return;
            }
            result = replay().await;
        }
        if matches!(result, Err(EngineError::NoPeers | EngineError::Storage(_))) {
            #[cfg(test)]
            if matches!(result, Err(EngineError::NoPeers)) {
                let pause = self.hub.pause_rebroadcast_rearm.lock().unwrap().take();
                crate::coinjoin::test_pause(pause).await;
            }
            // Leave persistent failures for the next peer, preserving a newer rebuild.
            let _ = self.rebroadcast_replay.compare_exchange(
                0,
                generation,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
    }

    async fn restore_pending_rebroadcasts(
        &self,
        manager: &Manager,
        generation: u64,
        restored: &mut HashSet<(WalletId, Txid)>,
    ) -> Result<(), EngineError> {
        let mut reads = Ok(());
        for wallet in manager.wallet_ids().await.into_iter().map(WalletId) {
            let sent = match self.appdb_op(move |db| read_replay_sent(db, wallet)).await {
                Ok(sent) => sent,
                Err(e @ EngineError::Storage(_)) => {
                    reads = Err(e);
                    continue;
                }
                Err(e) => return Err(e),
            };
            for (txid, mark) in sent {
                if (!mark.kept && mark.coins.is_empty()) || restored.contains(&(wallet, txid)) {
                    continue;
                }
                match self
                    .resend_transaction_inner(wallet, txid, Some(generation))
                    .await
                {
                    Ok(()) => {
                        restored.insert((wallet, txid));
                    }
                    Err(e @ (EngineError::NoPeers | EngineError::SpvNotRunning)) => return Err(e),
                    Err(e @ EngineError::Storage(_)) => reads = Err(e),
                    Err(_) => {}
                }
            }
        }
        reads
    }

    /// dash-qt "Resend transaction" (QT-091): hands the stored transaction
    /// to the SPV client again. Returns once handed over (or `NoPeers` when
    /// dash-spv refused to send it); the network's verdict is only logged.
    /// The send continues only while the session stays open.
    pub async fn resend_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.resend_transaction_inner(wallet, txid, None).await
        })
        .await
    }

    async fn resend_transaction_inner(
        &self,
        wallet: WalletId,
        txid: Txid,
        replay_generation: Option<u64>,
    ) -> Result<(), EngineError> {
        self.refuse_tx_action_while_restored()?;
        let manager = self.manager()?;
        let (entry, f) = self.tx_facts(&manager, wallet, txid, false).await?;
        let peers = self.hub.tracker().snapshot().connected_peers;
        if replay_generation.is_none() && self.can_restart_abandoned_sweep(wallet, txid, &f).await {
            return self.restart_abandoned_sweep(wallet, txid).await;
        }
        if let Some(r) = activity_resend_refusal(&f, manager.spv().is_started(), peers) {
            return Err(r.into());
        }
        #[cfg(test)]
        let pause = self.hub.pause_resend.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause.mark_reached();
            pause.release.notified().await;
        }
        // Track the replay task before a stop can abort the old client's resends.
        let replay_lifecycle = if let Some(generation) = replay_generation {
            let lifecycle = self.platform.lifecycle.lock().await;
            if manager.spv().run_session() != generation || !manager.spv().is_started() {
                return Err(EngineError::SpvNotRunning);
            }
            Some(lifecycle)
        } else {
            None
        };
        // Read again, held against an abandon through the hand-off: one
        // since the check committed its mark, and its bytes stay unsent.
        let gate = self.abandon_gate(wallet).read_owned().await;
        if self.read_abandon_mark(wallet, txid).await? {
            return Err(EngineError::TxActionRefused(
                TxActionRefusal::AlreadyAbandoned,
            ));
        }
        let transaction = if replay_generation.is_some() {
            let persister = self.live()?.persister;
            let pending =
                tokio::task::spawn_blocking(move || persister.get_core_tx_record(wallet.0, &txid))
                    .await?
                    .map_err(|e| EngineError::Storage(e.to_string()))?;
            let Some(record) = pending.filter(|r| {
                matches!(r.context, TransactionContext::Mempool) && r.transaction.txid() == txid
            }) else {
                return Ok(());
            };
            record.transaction
        } else {
            entry.tx
        };
        let handle = self.wallet(&wallet).await?;
        #[cfg(test)]
        let wire = self.hub.resend_wire.lock().unwrap().clone();
        // platform-wallet's only broadcast entry point waits for the
        // network's acceptance (up to dash-spv's 60 s timeout). The send
        // itself happens at once, so the call returns after a short
        // hand-off window and leaves the wait to a background task; a
        // send dash-spv refuses outright (no connection) is reported.
        let mut task = tokio::spawn(async move {
            let _gate = gate;
            #[cfg(test)]
            if let Some(wire) = wire {
                wire.lock().unwrap().push(transaction.txid());
                return Ok(transaction.txid());
            }
            let outcome = handle.core().broadcast_transaction(&transaction).await;
            match &outcome {
                Ok(_) => tracing::info!(%txid, "resent transaction accepted"),
                Err(e) => {
                    tracing::info!(%txid, error = %e, "resent transaction: no acceptance seen")
                }
            }
            outcome
        });
        {
            let mut resends = self.resends.lock().unwrap_or_else(|p| p.into_inner());
            resends.retain(|r| !r.is_finished());
            resends.push(task.abort_handle());
        }
        drop(replay_lifecycle);
        match tokio::time::timeout(RESEND_HANDOFF, &mut task).await {
            Ok(Ok(Err(platform_wallet::PlatformWalletError::TransactionBroadcast(_)))) => {
                return Err(EngineError::NoPeers);
            }
            Ok(Err(e)) => return Err(e.into()),
            Ok(Ok(_)) | Err(_) => {}
        }
        self.hub.note_announced(txid);
        Ok(())
    }

    /// iOS "remove unconfirmed" (IOS-034): abandons every transaction of
    /// `wallet` (all loaded wallets for `None`) that Abandon would: out of a
    /// block, not abandoned, and not InstantSend-locked unless a ChainLocked
    /// spend of one of its inputs beat it; and rescans from the lowest
    /// funding height of the coins they spent. Needs SPV running or
    /// rebuilding for the rescan. Returns how many transactions were dropped
    /// (descendants included).
    pub async fn drop_unconfirmed(
        self: &Arc<Self>,
        wallet: Option<WalletId>,
    ) -> Result<u32, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.refuse_tx_action_while_restored()?;
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            if let Some(id) = &wallet {
                this.require_wallet(id)?;
            }
            if !this.spv_can_abandon(&manager) {
                return Err(EngineError::SpvNotRunning);
            }
            let wallets: Vec<WalletId> = match wallet {
                Some(id) => vec![id],
                None => manager
                    .list_wallet_ids_blocking()
                    .into_iter()
                    .map(WalletId)
                    .collect(),
            };
            // Every wallet's marks are read before anything is abandoned:
            // when one doesn't read, no wallet is touched.
            let mut marked = Vec::with_capacity(wallets.len());
            for id in wallets {
                let (sent, complete) = this.appdb_op(move |db| Ok(load_sent(db, id))).await?;
                #[cfg(test)]
                let complete = complete && *this.hub.fail_sent_read.lock().unwrap() != Some(id);
                if !complete {
                    return Err(EngineError::Storage(
                        "removing unconfirmed transactions: the sent marks do not read".into(),
                    ));
                }
                marked.push((id, sent));
            }
            let (mut dropped, mut unreadable) = (0u32, None);
            for (id, sent) in marked {
                let proof = this.chain_proof(&manager, id).await;
                let candidates: Vec<Txid> = this
                    .with_view(&manager, id, |view| {
                        view.history
                            .iter()
                            .filter(|(_, e)| facts(e, view, &proof).abandon_refusal().is_none())
                            .map(|(t, _)| *t)
                            .collect()
                    })
                    .await?;
                // One rescan after all: one made between them would hold
                // the next ChainLock exception's scan below its ChainLock.
                // Until it is made, a sync commit leaves the rewind records
                // the abandons store.
                let _batch = this.hub.abandon_batch(id);
                let (mut done, mut lowest, mut failed) = (BTreeSet::new(), None::<u32>, None);
                for txid in candidates {
                    if done.contains(&txid) {
                        continue;
                    }
                    match this
                        .abandon_unrewound(&manager, id, txid, None, &BTreeSet::new(), &mut lowest)
                        .await
                    {
                        Ok(set) => done.extend(set),
                        // Unknown while a record doesn't read: kept, and
                        // said so once the rest is done (#611).
                        Err(EngineError::TxActionRefused(
                            refusal @ TxActionRefusal::RecordUnreadable(_),
                        )) => {
                            unreadable = Some(refusal);
                        }
                        // Settled meanwhile: nothing to drop.
                        Err(EngineError::TxActionRefused(_)) => {}
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    }
                }
                // A payment whose saved copy does not read back is listed
                // nowhere but keeps its coins out: retired here, unless a
                // payment under way holds one of them.
                let damaged: Vec<Txid> = (sent.into_iter())
                    .filter(|(t, m)| {
                        m.kept && m.tx.is_none() && this.hub.history.get(&id, t).is_none()
                    })
                    .filter(|(t, m)| {
                        (this
                            .spends
                            .held_by_others(&id, crate::send::SpendOwner::Tx(*t)))
                        .is_disjoint(&m.coins.iter().copied().collect())
                    })
                    .map(|(t, _)| t)
                    .collect();
                for txid in damaged.into_iter().filter(|_| failed.is_none()) {
                    match this
                        .abandon_unrewound(&manager, id, txid, None, &BTreeSet::new(), &mut lowest)
                        .await
                    {
                        Ok(set) => {
                            // Counted only once its marks are gone: a mark
                            // left behind claims its coins again at the next
                            // load.
                            if let Err(e) =
                                this.appdb_op(move |db| forget_sent(db, id, &[txid])).await
                            {
                                failed = Some(e);
                                break;
                            }
                            this.hub.history.drop_claims(&id, [txid]);
                            done.extend(set);
                        }
                        Err(EngineError::TxActionRefused(
                            refusal @ TxActionRefusal::RecordUnreadable(_),
                        )) => {
                            unreadable = Some(refusal);
                        }
                        Err(EngineError::TxActionRefused(_)) => {}
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    }
                }
                if !done.is_empty() || lowest.is_some() {
                    this.finish_abandons(&manager, id, &done, lowest).await?;
                }
                dropped += done.len() as u32;
                if let Some(e) = failed {
                    return Err(e);
                }
            }
            if let Some(refusal) = unreadable {
                return Err(EngineError::TxActionRefused(refusal));
            }
            Ok(dropped)
        })
        .await
    }

    /// Drops what this wallet built on the outputs of `dead`, transactions
    /// that can never confirm (#739): claims the store dropped as unsigned
    /// by their coin's owner or this app dropped (`at_load`), or what a
    /// sweep of such a claim by key-wallet dropped ([`swept_claims`]). Each
    /// unconfirmed, unlocked transaction of the history that is one of
    /// `dead` or spends an output of one is abandoned with what descends
    /// from it, as Remove Unconfirmed does. A settled transaction anywhere
    /// in the dead tree vetoes the drop.
    pub(crate) async fn abandon_on_dead(
        &self,
        manager: &Manager,
        wallet: WalletId,
        dead: &Dead,
        source: DeadSource,
    ) {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&wallet.0) else {
            return;
        };
        let core = &info.core_wallet;
        let mut dead = dead.clone();
        let from_event = matches!(source, DeadSource::Event);
        if from_event {
            self.hub.history.read_wallet(&wallet, |h| {
                dead.retain(|t, cause| h.invalid_mark(t) == Some(*cause));
            });
        }
        if matches!(source, DeadSource::Load) {
            let infos = wm.get_all_wallet_infos().iter();
            let settled = self.settled_all(
                infos.map(|(id, i)| (id, &i.core_wallet)),
                dead.keys().copied(),
            );
            for t in self.prune_dropped(wallet, &dead, &settled).await {
                dead.remove(&t);
            }
        }
        let dead = Arc::new(dead);
        let known_dead = Arc::new(self.hub.history.read_wallet(&wallet, |h| {
            dead.iter()
                .map(|(t, cause)| (*t, *cause))
                .chain(
                    h.drops
                        .keys()
                        .filter_map(|t| Some((*t, h.invalid_mark(t)?))),
                )
                .collect()
        }));
        // Include a dropped payment key-wallet recorded again after a relay.
        let children: Vec<(Txid, BTreeSet<Txid>)> = (self.hub.history).read(&wallet, |txs| {
            (txs.iter())
                .filter(|(t, e)| pending_for_dead_drop(t, e, core))
                .filter_map(|(t, e)| {
                    let parents: BTreeSet<Txid> = (e.tx.input.iter())
                        .map(|i| i.previous_output.txid)
                        .filter(|p| dead.contains_key(p))
                        .collect();
                    (dead.contains_key(t) || !parents.is_empty()).then_some((*t, parents))
                })
                .collect()
        });
        drop(wm);
        let drops = (children.into_iter())
            .map(|(child, parents)| {
                let again = dead.contains_key(&child);
                let tree: Vec<Txid> = (parents.iter().copied())
                    .chain(again.then_some(child))
                    .collect();
                let claims = (tree.iter())
                    .filter(|t| dead.get(*t) == Some(&DropCause::InvalidClaim))
                    .copied()
                    .collect();
                let event_marks = (tree.iter())
                    .filter(|_| from_event)
                    .map(|t| (*t, dead[t]))
                    .collect();
                let spent = parents.clone();
                let holds = move |txs: &HashMap<Txid, TxEntry>,
                                  core: &ManagedWalletInfo,
                                  settled: &Settled| {
                    let still = txs.get(&child).is_some_and(|e| {
                        pending_for_dead_drop(&child, e, core)
                            && (again
                                || (e.tx.input.iter())
                                    .any(|i| spent.contains(&i.previous_output.txid)))
                    });
                    if !still {
                        return false;
                    }
                    !tree.iter().any(|t| tree_settled(core, settled, t))
                };
                let drop = DeadDrop {
                    dead: parents,
                    known_dead: Arc::clone(&known_dead),
                    event_marks,
                    claims,
                    holds: Box::new(holds),
                };
                (child, drop)
            })
            .collect();
        self.drop_trees(manager, wallet, drops).await;
    }

    async fn prune_dropped(
        &self,
        wallet: WalletId,
        dead: &Dead,
        settled: &Settled,
    ) -> HashSet<Txid> {
        let settled: HashSet<Txid> = dead
            .keys()
            .filter(|t| settled.direct.contains_key(*t))
            .copied()
            .collect();
        if settled.is_empty() {
            return settled;
        }
        let expired = self.hub.history.read(&wallet, |txs| {
            let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
            for (t, e) in txs {
                for input in &e.tx.input {
                    children
                        .entry(input.previous_output.txid)
                        .or_default()
                        .push(*t);
                }
            }
            let mut descendants = HashSet::new();
            let mut todo: Vec<_> = settled.iter().copied().collect();
            while let Some(t) = todo.pop() {
                for &child in children.get(&t).into_iter().flatten() {
                    if descendants.insert(child) {
                        todo.push(child);
                    }
                }
            }
            let mut expired: HashSet<_> = dead
                .iter()
                .filter(|(t, cause)| {
                    **cause == DropCause::InvalidSource && descendants.contains(*t)
                })
                .map(|(t, _)| *t)
                .collect();
            // An older dependent mark can have lost its transaction and
            // ancestry. Keep the settled anchors while any such verdict
            // could still seed another drop.
            if dead.iter().all(|(t, cause)| {
                *cause != DropCause::InvalidSource || expired.contains(t) || settled.contains(t)
            }) {
                expired.extend(&settled);
            }
            expired
        });
        if expired.is_empty() {
            return expired;
        }
        tracing::warn!(%wallet, ?expired, "forgetting drop verdicts contradicted by settlement");
        let scope = wallet.to_string();
        let removed = expired.clone();
        let result = self
            .appdb_op(move |db| {
                let keys: Vec<String> = (removed.iter())
                    .flat_map(|t| {
                        [
                            format!("{DROPPED_PREFIX}{t}"),
                            crate::store_repair::record_key(t),
                        ]
                    })
                    .collect();
                let writes: Vec<(&str, Option<&str>)> =
                    keys.iter().map(|k| (k.as_str(), None)).collect();
                db.set_settings_durable(&scope, &writes)
            })
            .await;
        match result {
            Ok(()) => {
                self.hub.history.with_wallet(wallet, |h| {
                    h.drops.retain(|t, _| !expired.contains(t));
                });
                self.hub.reread_loaded_balances();
                expired
            }
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "could not forget the records of settled dropped transactions");
                HashSet::new()
            }
        }
    }

    /// After a restart key-wallet holds no pending record, so it can't drop
    /// a claim a new transaction (`funded`) shows unsigned: the history's
    /// claims on a coin of the wallet `funded` pays ([`unsigned_claim`]).
    #[cfg(test)]
    pub(crate) async fn unsigned_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        funded: &BTreeSet<Txid>,
    ) -> Vec<Txid> {
        self.find_unsigned_claims(manager, wallet, funded, false)
            .await
    }

    async fn find_unsigned_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        funded: &BTreeSet<Txid>,
        withhold: bool,
    ) -> Vec<Txid> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&wallet.0) else {
            return Vec::new();
        };
        (self.hub.history).with_wallet(wallet, |h| {
            let claims: Vec<_> = (h.txs.keys())
                .filter(|t| unsigned_claim(&h.txs, &info.core_wallet, **t, funded))
                .copied()
                .collect();
            if withhold {
                h.checking_claims.extend(&claims);
            }
            claims
        })
    }

    /// Abandons each of `claims`, the history's claims `funded` shows
    /// unsigned, with what descends from it, when under the lock that
    /// applies it the claim still is one and nothing of its tree is
    /// settled in any loaded wallet (#739).
    pub(crate) async fn abandon_unsigned_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        claims: Vec<Txid>,
        funded: &BTreeSet<Txid>,
    ) {
        let drops = (claims.into_iter())
            .map(|claim| {
                let funded = funded.clone();
                let holds = move |txs: &HashMap<Txid, TxEntry>,
                                  core: &ManagedWalletInfo,
                                  settled: &Settled| {
                    if !unsigned_claim(txs, core, claim, &funded) {
                        return false;
                    }
                    !tree_settled(core, settled, &claim)
                };
                let drop = DeadDrop {
                    dead: BTreeSet::new(),
                    known_dead: Arc::new(Dead::new()),
                    event_marks: Dead::new(),
                    claims: BTreeSet::from([claim]),
                    holds: Box::new(holds),
                };
                (claim, drop)
            })
            .collect();
        self.drop_trees(manager, wallet, drops).await;
    }

    /// Abandons each root of `drops` with what descends from it, in one
    /// batch; one refused, or whose check fails under the lock, is kept.
    async fn drop_trees(&self, manager: &Manager, wallet: WalletId, drops: Vec<(Txid, DeadDrop)>) {
        if drops.is_empty() {
            return;
        }
        let _batch = self.hub.abandon_batch(wallet);
        let (mut done, mut lowest) = (BTreeSet::new(), None::<u32>);
        for (root, drop) in drops {
            if done.contains(&root) {
                continue;
            }
            match (self.abandon_unrewound(
                manager,
                wallet,
                root,
                Some(&drop),
                &BTreeSet::new(),
                &mut lowest,
            ))
            .await
            {
                Ok(set) => done.extend(set),
                Err(e) => {
                    tracing::warn!(%wallet, txid = %root, error = %e, "kept a transaction that can never confirm or is built on one")
                }
            }
        }
        if done.is_empty() && lowest.is_none() {
            return;
        }
        if let Err(e) = self.finish_abandons(manager, wallet, &done, lowest).await {
            tracing::warn!(%wallet, error = %e, "the abandon of transactions that can never confirm did not finish");
        }
    }

    /// Hooks the held reports of the wallets with holds to the locks the
    /// hub hears (Q81c C1): each one notes a lock that may spend a held
    /// coin before a restart forgets it.
    pub(crate) fn attach_held_refresh(self: &Arc<Self>) {
        let (weak, rt) = (Arc::downgrade(self), self.rt.clone());
        self.hub.attach_held_refresh(Box::new(move || {
            let weak = weak.clone();
            rt.spawn(async move {
                let Some(session) = weak.upgrade() else {
                    return;
                };
                let Ok(_op) = session.enter().await else {
                    return;
                };
                let Ok(manager) = session.manager() else {
                    return;
                };
                let _lifecycle = session.wallet_lifecycle.hold().await;
                let _gate = session.hub.drop_gate.lock().await;
                for wallet in session.hub.wallets_with_holds() {
                    session.refresh_held(&manager, wallet).await;
                }
            });
        }));
    }

    /// The money gate of Abandon, Resend and Remove Unconfirmed
    /// (`restore_gate`).
    fn refuse_tx_action_while_restored(&self) -> Result<(), EngineError> {
        match self.hub.restore.is_restored() {
            true => Err(EngineError::TxActionRefused(TxActionRefusal::Restored)),
            false => Ok(()),
        }
    }

    /// Hooks [`Self::abandon_on_dead`] to the hub's sweep events, and
    /// [`Self::abandon_unsigned_claims`] to its new transactions; reads the
    /// restored mark and starts what clears it (`restore_gate`), before the
    /// history loads.
    pub(crate) fn attach_dead_claims(self: &Arc<Self>) {
        self.attach_restore_gate();
        let (weak, rt) = (Arc::downgrade(self), self.rt.clone());
        self.hub
            .attach_swept_dead(Box::new(move |id, registration, dead| {
                let weak = weak.clone();
                rt.spawn(async move {
                    let Some(session) = weak.upgrade() else {
                        return;
                    };
                    let Ok(_op) = session.enter().await else {
                        return;
                    };
                    let Ok(manager) = session.manager() else {
                        return;
                    };
                    #[cfg(test)]
                    let pause = session.hub.pause_swept_dead.lock().unwrap().take();
                    #[cfg(test)]
                    if let Some(pause) = pause {
                        pause.mark_reached();
                        pause.release.notified().await;
                    }
                    let _lifecycle = session.wallet_lifecycle.hold().await;
                    if session.hub.drop_registration(id) == Some(registration) {
                        session
                            .abandon_on_dead(&manager, id, &dead, DeadSource::Event)
                            .await;
                    }
                    #[cfg(test)]
                    (session.hub.dead_hooks_done).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                });
            }));
        let (weak, rt) = (Arc::downgrade(self), self.rt.clone());
        self.hub.attach_funded(Box::new(move |id, funded| {
            let weak = weak.clone();
            rt.spawn(async move {
                let Some(session) = weak.upgrade() else {
                    return;
                };
                let Ok(_op) = session.enter().await else {
                    return;
                };
                let Ok(manager) = session.manager() else {
                    return;
                };
                let _lifecycle = session.wallet_lifecycle.hold().await;
                let claims = session
                    .find_unsigned_claims(&manager, id, &funded, true)
                    .await;
                if claims.is_empty() {
                    return;
                }
                session.hub.reread_loaded_balances();
                (session.abandon_unsigned_claims(&manager, id, claims, &funded)).await;
                session.refresh_wallet_state(&manager, id).await;
            });
        }));
    }

    /// dash-qt CSV export of the filtered history (QT-093), exact bytes.
    /// `type_names`: 19 names in `TxType` order, or empty for English.
    pub async fn export_history_csv(
        self: &Arc<Self>,
        wallet: WalletId,
        filter: HistoryFilter,
        sort: HistorySort,
        unit: Unit,
        type_names: Vec<String>,
        utc_offset_secs: i32,
    ) -> Result<String, EngineError> {
        if !type_names.is_empty() && type_names.len() != TX_TYPE_COUNT {
            return Err(EngineError::InvalidQuery(format!(
                "type_names has {} entries, expected 0 or {TX_TYPE_COUNT}",
                type_names.len()
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let watch_only = !this.vault.has_wallet_secret(&wallet.0);
            let records = filtered(this.wallet_records(wallet).await?, &filter, sort)?;
            let chain = match this.network {
                crate::DashNetwork::Mainnet => Chain::Main,
                _ => Chain::Test,
            };
            Ok(write_csv(
                &records,
                watch_only,
                unit,
                chain,
                &type_names,
                utc_offset_secs,
            ))
        })
        .await
    }

    /// The notices of the payments of `wallet` this app dropped as built on
    /// a transaction that was never valid ([`DroppedNotice`]): each a
    /// payment the wallet sent, its own coins back ([`DropState::Released`])
    /// and its notice not dismissed. Never the never-valid transaction
    /// itself, nor a payment dropped for another reason.
    ///
    /// [`DropState::Released`]: crate::history::DropState::Released
    pub async fn dropped_notices(
        self: &Arc<Self>,
        wallet: WalletId,
    ) -> Result<Vec<DroppedNotice>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            if (this.hub.history).read_wallet(&wallet, |h| h.drops.is_empty()) {
                return Ok(Vec::new());
            }
            let labels: Labels = Self::labels(&live.appdb, wallet).await?;
            let seen: HashSet<Txid> = this
                .appdb_op(move |db| {
                    db.settings_with_prefix_lossy(&wallet.to_string(), DROP_SEEN_PREFIX)
                })
                .await?
                .into_iter()
                .filter_map(|(key, _)| Txid::from_str(&key[DROP_SEEN_PREFIX.len()..]).ok())
                .collect();
            let mut notices = this
                .with_view_naming_payees(&live.manager, wallet, |view| {
                    (view.drops.states.iter())
                        .filter(|(t, d)| {
                            d.cause == DropCause::InvalidSource
                                && d.state == crate::history::DropState::Released
                                && !seen.contains(*t)
                        })
                        .filter_map(|(t, _)| view.history.get(t).map(|e| (t, e)))
                        .flat_map(|(t, e)| records_of(t, e, view, &labels).0)
                        .filter(|r| {
                            r.category == crate::history::TxCategory::Sent
                                && r.status.dropped.is_some()
                        })
                        .map(|r| {
                            let fee = match r.record_index {
                                0 => r.fee.unwrap_or(0),
                                _ => 0,
                            };
                            DroppedNotice {
                                amount: r.amount.unsigned_abs().saturating_sub(fee),
                                txid: r.txid,
                                record_index: r.record_index,
                                timestamp: r.timestamp,
                                address: r.address,
                                label: r.label,
                                counterparty: r.counterparty,
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await?;
            notices.sort_by(|a, b| {
                (b.timestamp, &a.txid, a.record_index).cmp(&(a.timestamp, &b.txid, b.record_index))
            });
            Ok(notices)
        })
        .await
    }

    /// Dismisses the notices of the dropped payments `txids` of `wallet`
    /// ([`Self::dropped_notices`]) for good.
    pub async fn dismiss_dropped_notices(
        self: &Arc<Self>,
        wallet: WalletId,
        txids: Vec<Txid>,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_wallet(&wallet)?;
            if txids.is_empty() {
                return Ok(());
            }
            this.appdb_op(move |db| {
                let keys: Vec<String> = (txids.iter())
                    .map(|t| format!("{DROP_SEEN_PREFIX}{t}"))
                    .collect();
                let writes: Vec<(&str, Option<&str>)> =
                    keys.iter().map(|k| (k.as_str(), Some(""))).collect();
                db.set_settings_durable(&wallet.to_string(), &writes)
            })
            .await
        })
        .await
    }

    /// Notification rows for `txids` (a `NewTransactions` event). Unknown
    /// txids are skipped.
    pub async fn tx_notices(
        self: &Arc<Self>,
        wallet: WalletId,
        txids: Vec<Txid>,
    ) -> Result<Vec<TxNotice>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let labels: Labels = Self::labels(&live.appdb, wallet).await?;
            this.with_view(&live.manager, wallet, |view| {
                txids
                    .iter()
                    .filter_map(|t| view.history.get(t).map(|e| (t, e)))
                    .flat_map(|(t, e)| records_of(t, e, view, &labels).0)
                    .map(|r| TxNotice {
                        coinjoin_internal: is_coinjoin_internal(r.tx_type),
                        txid: r.txid,
                        record_index: r.record_index,
                        amount: r.amount,
                        timestamp: r.timestamp,
                        tx_type: r.tx_type,
                        address: r.address,
                        label: r.label,
                    })
                    .collect()
            })
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rescan_hold::RESCAN_HOLD_PREFIX;

    struct NoEvents;

    impl crate::EventSink for NoEvents {
        fn emit(&self, _: crate::EngineEvent) {}
    }

    fn recheck(at: u32, truncations: u64) -> Recheck {
        let coin = OutPoint::new(Txid::from_byte_array([at as u8; 32]), 0);
        Recheck {
            truncations,
            processed: at,
            block: Some(BlockHash::from_byte_array([at as u8; 32])),
            spent: Arc::new(BTreeMap::from([(coin, at)])),
            tries: 0,
        }
    }

    #[test]
    fn kept_roots_share_only_their_groups_detection_evidence() {
        let [a, b, c, d] = [1, 2, 3, 4].map(|n| Txid::from_byte_array([n; 32]));
        let first = Winner {
            txid: None,
            mined: Some(10),
        };
        let second = Winner {
            txid: None,
            mined: Some(11),
        };
        let one = recheck(10, 0);
        let two = recheck(11, 0);
        let newer = recheck(10, 1);
        let groups = group_carried_drops(vec![
            (a, (first, one.clone(), 10, None)),
            (b, (first, one.clone(), 10, None)),
            (c, (second, two.clone(), 11, None)),
            (d, (first, newer.clone(), 10, None)),
        ]);
        assert_eq!(groups.len(), 3);
        for (roots, winner, got, height) in groups {
            let (expected_winner, expected) = match roots.as_slice() {
                [l, r] if (*l, *r) == (a, b) => (first, &one),
                [t] if *t == c => (second, &two),
                [t] if *t == d => (first, &newer),
                _ => panic!("unexpected grouping {roots:?}"),
            };
            assert_eq!((winner, height), (expected_winner, expected.processed));
            assert_eq!(&got, expected);
            assert!(Arc::ptr_eq(&got.spent, &expected.spent));
        }
    }

    #[test]
    fn queued_and_in_flight_drops_do_not_survive_a_wallet_reopen() {
        let hub = crate::events::SessionHub::new(crate::DashNetwork::Regtest, Arc::new(NoEvents));
        let wallet = WalletId([9; 32]);
        let root = Txid::from_byte_array([1; 32]);
        let winner = Winner {
            txid: None,
            mined: Some(10),
        };
        let registration = hub.drop_registration(wallet).unwrap();
        let evidence = (winner, recheck(10, 0), 10, None);
        hub.carry_drops(wallet, registration, &[root], evidence.clone());
        hub.carry_drops(wallet, registration, &[root], evidence.clone());
        assert_eq!(hub.take_carried_drops(wallet, registration).len(), 1);
        hub.carry_drops(wallet, registration, &[root], evidence.clone());
        let work = (
            vec![root],
            winner,
            BTreeSet::new(),
            None,
            Default::default(),
        );
        let stamp = (0, Some(10), None);
        hub.defer_drops(wallet, registration, [(stamp, work.clone())]);

        hub.unload_wallet(&wallet);
        hub.admit(wallet);
        let renewed = hub.drop_registration(wallet).unwrap();
        hub.carry_drops(wallet, registration, &[root], evidence);
        hub.defer_drops(wallet, registration, [(stamp, work)]);
        assert!(hub.take_carried_drops(wallet, renewed).is_empty());
        assert!(hub.take_deferred_drops(wallet, renewed).is_empty());
    }

    #[test]
    fn a_deferred_sweep_child_is_kept_once_with_its_sweep_evidence() {
        let hub = crate::events::SessionHub::new(crate::DashNetwork::Regtest, Arc::new(NoEvents));
        let wallet = WalletId([9; 32]);
        let root = Txid::from_byte_array([1; 32]);
        let parent = Txid::from_byte_array([2; 32]);
        let coin = OutPoint::new(parent, 1);
        let stamp = (0, Some(10), None);
        let registration = hub.drop_registration(wallet).unwrap();
        let orphan = (
            vec![root],
            Winner::default(),
            BTreeSet::new(),
            None,
            (BTreeSet::from([parent]), BTreeSet::new()),
        );
        let winner = Winner {
            txid: Some(parent),
            mined: Some(10),
        };
        let swept = (
            vec![root],
            winner,
            BTreeSet::from([parent]),
            None,
            (BTreeSet::new(), BTreeSet::from([coin])),
        );
        for work in [[orphan.clone(), swept.clone()], [swept.clone(), orphan]] {
            hub.defer_drops(wallet, registration, work.map(|w| (stamp, w)));
            assert_eq!(
                hub.take_deferred_drops(wallet, registration),
                [(stamp, swept.clone())]
            );
        }
    }

    /// The wallet-scope marks are read by prefix (the drops' holds, origins,
    /// spend and lock notes, the sweep notes, the claim rule's dropped
    /// records, the claims swept, the abandoned and sent marks): no key of
    /// one is read as another's.
    #[test]
    fn the_drop_note_prefixes_are_apart() {
        assert_ne!(DROP_HOLDS_PREFIX, DROPPED_PREFIX);
        let prefixes = [
            DROP_HOLDS_PREFIX,
            DROP_WINNER_PREFIX,
            DROP_SPENT_PREFIX,
            DROP_LOCKED_PREFIX,
            SWEPT_BY_PREFIX,
            DROPPED_PREFIX,
            CLAIM_SWEPT_PREFIX,
            ABANDONED_PREFIX,
            SENT_PREFIX,
            SENT_TX_PREFIX,
            SENT_IN_PREFIX,
            RESCAN_HOLD_PREFIX,
            DROP_SEEN_PREFIX,
        ];
        for a in prefixes {
            for b in prefixes {
                assert!(a == b || !a.starts_with(b), "{a} is read as {b}");
            }
        }
    }

    fn f() -> Facts {
        Facts {
            depth: 0,
            chain_locked: false,
            instant_locked: false,
            uncertain_fork: false,
            chainlock_beaten: false,
            settled_descendant: false,
            coinbase: false,
            abandoned: false,
            from_me: 1,
            credit: 0,
            debit: 0,
            debit_unknown: false,
            inputs_unresolved: false,
            matures_in: None,
            abandon_marked: false,
            platform_funding: false,
        }
    }

    /// Q81d: the orphan rule's reader of a `dropped:` mark takes an empty
    /// value and an invalid claim's or source's mark as never-valid, and
    /// neither a replaced transaction's mark nor any other value (#803's
    /// pre-merge lists of held coins among them), unlike the display's.
    #[test]
    fn only_a_claim_rule_verdict_reads_as_never_valid() {
        let none = OwnInputs::new();
        assert!(never_valid(Some("")));
        assert!(never_valid(Some(&drop_mark(
            DropCause::InvalidClaim,
            &none
        ))));
        assert!(never_valid(Some(&drop_mark(
            DropCause::InvalidSource,
            &none
        ))));
        let replaced = drop_mark(DropCause::Replaced, &none);
        assert!(!never_valid(Some(&replaced)));
        let held = format!("{},{}", OutPoint::null(), OutPoint::null());
        for value in [held.as_str(), "garbage", "{}", "null"] {
            assert!(!never_valid(Some(value)), "{value:?}");
            assert_eq!(drop_mark_cause(Some(value)), DropCause::InvalidSource);
        }
        assert!(!never_valid(None));
    }

    /// #951: a load keeps every sweep note a drop's chain walks through, a
    /// grandparent's too: K, dropped, builds on Q, swept, which builds on P
    /// and P on R, both swept. With K's drop gone, nothing names the chain
    /// and every note goes; a note of a transaction stored again goes too.
    #[test]
    fn a_load_keeps_each_sweep_note_a_drops_chain_reads() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let spend = |of: Txid| Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: OutPoint::new(of, 0),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let r = spend(t(1));
        let p = spend(r.txid());
        let q = spend(p.txid());
        let k = spend(q.txid());
        let swept = |tx: &Transaction| {
            let note = crate::history::SweptBy {
                claim: false,
                winner: t(2),
                block: None,
                tx: tx.clone(),
            };
            (tx.txid(), note)
        };
        let notes = |dropped: bool| DropNotes {
            drops: (dropped.then(|| (k.txid(), (BTreeSet::from([OutPoint::new(t(3), 0)]), None))))
                .into_iter()
                .collect(),
            deaths: crate::history::Deaths {
                swept_by: BTreeMap::from([swept(&r), swept(&p), swept(&q)]),
                copies: BTreeMap::from([(k.txid(), k.clone())]),
                ..Default::default()
            },
            ..Default::default()
        };
        let none = HashMap::new();
        let claim_only = t(1);
        let all = BTreeSet::from([claim_only, r.txid(), p.txid(), q.txid()]);
        let prune = |dropped, stored: &HashSet<Txid>| {
            let notes = notes(dropped);
            let obsolete = obsolete_sweep_note(&notes, &none, stored);
            all.iter()
                .copied()
                .filter(obsolete)
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(
            prune(true, &HashSet::new()),
            BTreeSet::new(),
            "K's chain reads Q, P, R and the parent with only a claim-sweep note"
        );
        assert_eq!(
            prune(false, &HashSet::new()),
            all,
            "no drop names the chain"
        );
        assert_eq!(
            prune(true, &HashSet::from([p.txid()])),
            BTreeSet::from([p.txid()]),
            "P stored again"
        );
    }

    /// #951: the held report reads the blocks of the spends of each input
    /// of a drop's chain, not only of the coins it holds or contests: K,
    /// dropped, holds D and builds on Q, swept, which builds on P, swept:
    /// the coins whose rivals could kill K's chain are K's, Q's and P's
    /// inputs. A sweep note no chain reads adds nothing.
    #[test]
    fn the_held_report_reads_the_spends_of_each_input_of_a_drops_chain() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let spend = |of: &[OutPoint]| Transaction {
            version: 2,
            lock_time: 0,
            input: (of.iter())
                .map(|o| dashcore::TxIn {
                    previous_output: *o,
                    ..Default::default()
                })
                .collect(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let (a, d) = (OutPoint::new(t(1), 0), OutPoint::new(t(3), 0));
        let p = spend(&[a]);
        let q = spend(&[OutPoint::new(p.txid(), 0)]);
        let k = spend(&[OutPoint::new(q.txid(), 0), d]);
        let z = spend(&[OutPoint::new(t(9), 0)]);
        let swept = |tx: &Transaction| {
            let note = crate::history::SweptBy {
                claim: false,
                winner: t(2),
                block: None,
                tx: tx.clone(),
            };
            (tx.txid(), note)
        };
        let deaths = crate::history::Deaths {
            swept_by: BTreeMap::from([swept(&p), swept(&q), swept(&z)]),
            copies: BTreeMap::from([(k.txid(), k.clone())]),
            ..Default::default()
        };
        let drops = BTreeMap::from([(k.txid(), (BTreeSet::from([d]), None))]);
        assert_eq!(
            spends_read([d].iter(), &drops, &deaths),
            HashSet::from([d, OutPoint::new(q.txid(), 0), OutPoint::new(p.txid(), 0), a]),
            "K's chain: K, Q and P"
        );
        assert_eq!(
            spends_read([d].iter(), &BTreeMap::new(), &deaths),
            HashSet::from([d]),
            "no drop"
        );
    }

    /// Q81d S1-S5: a sweep notes every transaction it swept with its kind,
    /// winner and bytes, a claim sweep's with no block; a later sweep that
    /// names the winner's block fills it in its earlier conflict notes; a
    /// reset of the chain data drops every note's block and keeps the rest.
    /// The drops' notes read them back, with the lock notes (those that
    /// don't read set apart, by coin or astray), the strict
    /// reading of the `dropped:` marks and the abandoned copies, and take a
    /// drop with no origin note as an older one; one whose origin note
    /// doesn't read keeps none.
    #[test]
    fn a_sweeps_notes_are_written_filled_and_reset() {
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let spend = |of: OutPoint| Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: of,
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let (p, q) = (spend(OutPoint::new(t(1), 0)), spend(OutPoint::new(t(1), 1)));
        let block = (7, BlockHash::from_byte_array([3; 32]));
        let read = |txid: Txid| {
            let value = db
                .setting_lossy(&scope, &format!("{SWEPT_BY_PREFIX}{txid}"))
                .unwrap()?;
            crate::history::SweptBy::decode(&txid, &value)
        };
        let sweep = |claim, block, txs: &[Transaction]| {
            let gone: Vec<Txid> = txs.iter().map(|tx| tx.txid()).collect();
            let sweep = Sweep {
                winner: t(2),
                block,
                claim,
                txs,
            };
            forget_swept(&db, wallet, &gone, &Dead::new(), &[], &[], Some(&sweep)).unwrap();
        };
        sweep(false, None, std::slice::from_ref(&p));
        let note = read(p.txid()).unwrap();
        assert_eq!((note.claim, note.winner, note.block), (false, t(2), None));
        assert_eq!(note.tx, p);
        sweep(true, Some(block), std::slice::from_ref(&q));
        assert_eq!(
            read(q.txid()).map(|n| (n.claim, n.block)),
            Some((true, None))
        );
        assert_eq!(read(p.txid()).unwrap().block, Some(block), "filled");

        let coin = OutPoint::new(t(4), 0);
        let r = spend(OutPoint::new(t(5), 0));
        let c = spend(OutPoint::new(r.txid(), 0));
        let spent_key = format!("{DROP_SPENT_PREFIX}{coin}");
        let locked_key = format!("{DROP_LOCKED_PREFIX}{coin}");
        let note_value = crate::history::SpendNote {
            height: 7,
            block: block.1,
            spender: None,
        }
        .encode();
        let writes: Vec<(String, String)> = vec![
            (spent_key.clone(), note_value),
            (locked_key.clone(), format!("{},{}", t(8), t(9))),
            (format!("{DROPPED_PREFIX}{}", t(1)), String::new()),
            (format!("{DROPPED_PREFIX}{}", t(5)), "x:0".into()),
            (format!("{ABANDONED_PREFIX}{}", r.txid()), serialize_hex(&r)),
            (format!("{ABANDONED_PREFIX}{}", c.txid()), serialize_hex(&c)),
            (format!("{DROP_HOLDS_PREFIX}{}", r.txid()), String::new()),
            (format!("{DROP_HOLDS_PREFIX}{}", c.txid()), coin.to_string()),
        ];
        let writes: Vec<(&str, Option<&str>)> = (writes.iter())
            .map(|(k, v)| (k.as_str(), Some(v.as_str())))
            .collect();
        db.set_settings_durable(&scope, &writes).unwrap();
        let notes = drop_notes(&db, wallet).unwrap();
        assert_eq!(notes.locked.get(&coin), Some(&BTreeSet::from([t(8), t(9)])));
        assert!(notes.locked_unread.is_empty() && !notes.locked_astray);
        assert_eq!(notes.deaths.never_valid, BTreeSet::from([t(1)]));
        assert_eq!(notes.deaths.swept_by.len(), 2);
        assert_eq!(notes.deaths.copies.len(), 2);
        let older = notes.drops.get(&c.txid()).and_then(|d| d.1.clone());
        assert_eq!(
            older.map(|o| o.contested),
            Some(BTreeSet::from([OutPoint::new(t(5), 0)])),
            "C's root R's input"
        );
        let winner_key = format!("{DROP_WINNER_PREFIX}{}", c.txid());
        db.set_settings_durable(&scope, &[(winner_key.as_str(), Some("junk"))])
            .unwrap();
        let notes = drop_notes(&db, wallet).unwrap();
        assert_eq!(notes.drops.get(&c.txid()).map(|d| &d.1), Some(&None));
        db.set_settings_durable(&scope, &[(winner_key.as_str(), None)])
            .unwrap();

        let unread: Vec<(OutPoint, String)> = vec![
            (OutPoint::new(t(10), 0), "junk".into()),
            (OutPoint::new(t(10), 1), format!("{},junk", t(8))),
            (OutPoint::new(t(10), 2), String::new()),
        ];
        let writes: Vec<(String, String)> = (unread.iter())
            .map(|(o, v)| (format!("{DROP_LOCKED_PREFIX}{o}"), v.clone()))
            .collect();
        let writes: Vec<(&str, Option<&str>)> = (writes.iter())
            .map(|(k, v)| (k.as_str(), Some(v.as_str())))
            .collect();
        db.set_settings_durable(&scope, &writes).unwrap();
        let notes = drop_notes(&db, wallet).unwrap();
        assert_eq!(notes.locked.len(), 1);
        assert_eq!(
            notes.locked_unread,
            unread.iter().map(|(o, _)| *o).collect::<BTreeSet<_>>()
        );
        assert!(!notes.locked_astray);
        let astray = format!("{DROP_LOCKED_PREFIX}not-a-coin");
        db.set_settings_durable(&scope, &[(astray.as_str(), Some(""))])
            .unwrap();
        assert!(drop_notes(&db, wallet).unwrap().locked_astray);

        forget_spend_notes(&db, wallet).unwrap();
        assert_eq!(db.setting_lossy(&scope, &spent_key).unwrap(), None);
        let reset = read(p.txid()).unwrap();
        assert_eq!((reset.block, reset.winner, reset.tx), (None, t(2), p));
        assert!(db.setting_lossy(&scope, &locked_key).unwrap().is_some());
    }

    /// Which transactions the dropped marks name comes from their keys
    /// alone (#790): an empty value (an earlier build's), a cause, or a
    /// value that does not parse name the same set, and none fails the
    /// read. The value gives only why: a never-valid claim's, else what is
    /// built on one.
    #[test]
    fn the_dropped_marks_name_the_same_transactions_whatever_their_values() {
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let values = [
            "",
            r#"{"cause":"invalidSource"}"#,
            r#"{"cause":"invalidClaim"}"#,
            r#"{"cause":"somethingNew"}"#,
            "not json",
            r#"{"cause":"invalidClaim","extra":1}"#,
        ];
        let keys: Vec<String> = (1..=values.len() as u8)
            .map(|n| format!("{DROPPED_PREFIX}{}", t(n)))
            .collect();
        let writes: Vec<(&str, Option<&str>)> = (keys.iter().zip(values))
            .map(|(k, v)| (k.as_str(), Some(v)))
            .collect();
        db.set_settings_durable(&scope, &writes).unwrap();
        let (dead, read) = load_dropped(&db, wallet);
        assert!(read);
        let names: BTreeSet<Txid> = dead.keys().copied().collect();
        assert_eq!(names, (1..=values.len() as u8).map(t).collect());
        use DropCause::*;
        let causes: Vec<DropCause> = dead.values().copied().collect();
        assert_eq!(
            causes,
            [
                InvalidSource,
                InvalidSource,
                InvalidClaim,
                InvalidSource,
                InvalidSource,
                InvalidClaim
            ]
        );
        for cause in [InvalidSource, InvalidClaim, Replaced] {
            assert_eq!(
                drop_mark_cause(Some(&drop_mark(cause, &OwnInputs::new()))),
                cause
            );
        }
        assert_eq!(drop_mark_cause(None), InvalidSource);
    }

    /// A mark with its signed transaction stored apart (#170 case 1) reads
    /// back with it; one an earlier build wrote reads without it; and one
    /// whose stored transaction does not decode reads without it but kept,
    /// the read of the others whole.
    #[test]
    fn a_sent_mark_reads_back_its_transaction() {
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let coin = OutPoint::new(Txid::from_byte_array([3; 32]), 1);
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: coin,
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let (old, bad) = (
            Txid::from_byte_array([4; 32]),
            Txid::from_byte_array([5; 32]),
        );
        let set = |key: &str, value: &str| db.set_setting(&scope, key, Some(value)).unwrap();
        for t in [tx.txid(), old, bad] {
            set(&format!("{SENT_PREFIX}{t}"), &coin.to_string());
        }
        set(
            &format!("{SENT_TX_PREFIX}{}", tx.txid()),
            &serialize_hex(&tx),
        );
        set(&format!("{SENT_TX_PREFIX}{bad}"), "zz");
        let mark = |tx, kept| SentMark {
            inputs: Default::default(),
            coins: BTreeSet::from([coin]),
            tx,
            kept,
        };
        let want = Sent::from([
            (tx.txid(), mark(Some(tx.clone()), true)),
            (old, mark(None, false)),
            (bad, mark(None, true)),
        ]);
        assert_eq!(load_sent(&db, wallet), (want, true));
    }

    /// The saved copies' query fails: the marks still read, a mark written
    /// with its copy (it has its coins' values) is kept with no copy, and an
    /// earlier build's mark, which has neither, is not.
    #[test]
    fn a_failed_read_of_the_copies_keeps_only_marks_written_with_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(dw_appdb::APP_DB_FILE);
        let db = AppDb::open(&path).unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let coin = OutPoint::new(Txid::from_byte_array([3; 32]), 1);
        let (new, old) = (
            Txid::from_byte_array([4; 32]),
            Txid::from_byte_array([5; 32]),
        );
        let set = |key: &str, value: &str| db.set_setting(&scope, key, Some(value)).unwrap();
        for t in [new, old] {
            set(&format!("{SENT_PREFIX}{t}"), &coin.to_string());
        }
        set(&format!("{SENT_TX_PREFIX}{new}"), "00");
        set(
            &format!("{SENT_IN_PREFIX}{new}"),
            &format!("{coin} 1000 yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd"),
        );
        dw_appdb::fault::arm(
            &path,
            "settings.read_senttx:",
            dw_appdb::fault::Stage::Before,
        );
        let (sent, read) = read_sent(&db, wallet);
        dw_appdb::fault::disarm(&path);
        assert_eq!(read, SentRead::CoinsOnly);
        let kept = |t: &Txid| {
            sent.get(t)
                .map(|m| (m.kept, m.tx.is_some(), m.coins.clone()))
        };
        assert_eq!(kept(&new), Some((true, false, BTreeSet::from([coin]))));
        assert_eq!(kept(&old), Some((false, false, BTreeSet::from([coin]))));
        assert!(load_sent(&db, wallet).1, "read again");
    }

    /// A mark whose txid suffix or value does not parse is skipped and the
    /// other marks are still read; a malformed abandoned mark also makes
    /// that read incomplete, a malformed sent mark does not.
    #[test]
    fn a_malformed_mark_is_skipped() {
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: Vec::new(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let txid = tx.txid();
        let coin = OutPoint::new(txid, 0);
        let set = |key: &str, value: Option<&str>| db.set_setting(&scope, key, value).unwrap();
        set(&format!("{SENT_PREFIX}{txid}"), Some(&coin.to_string()));
        set(
            &format!("{ABANDONED_PREFIX}{txid}"),
            Some(&serialize_hex(&tx)),
        );
        let sent = Sent::from([(
            txid,
            SentMark {
                inputs: Default::default(),
                coins: BTreeSet::from([coin]),
                tx: None,
                kept: false,
            },
        )]);
        assert_eq!(load_sent(&db, wallet), (sent.clone(), true));
        let copies = vec![(txid, tx.clone())];
        assert_eq!(
            load_abandoned(&db, wallet),
            (
                Abandoned {
                    copies: copies.clone(),
                    unread: BTreeSet::new(),
                },
                true
            )
        );

        let other = Txid::all_zeros();
        for (key, value) in [
            (format!("{SENT_PREFIX}not-a-txid"), coin.to_string()),
            (
                format!("{SENT_PREFIX}{other}"),
                "not-an-outpoint".to_owned(),
            ),
        ] {
            set(&key, Some(&value));
            assert_eq!(load_sent(&db, wallet), (sent.clone(), true), "{key}");
            set(&key, None);
        }
        // A damaged mark affects only its own payment: one naming no txid
        // is left out, one whose copy does not read back (or names another
        // transaction) is known by its key.
        for (key, value, unread) in [
            (
                format!("{ABANDONED_PREFIX}not-a-txid"),
                serialize_hex(&tx),
                BTreeSet::new(),
            ),
            (
                format!("{ABANDONED_PREFIX}{other}"),
                "zz".to_owned(),
                BTreeSet::from([other]),
            ),
            (
                format!("{ABANDONED_PREFIX}{other}"),
                serialize_hex(&tx),
                BTreeSet::from([other]),
            ),
        ] {
            set(&key, Some(&value));
            let want = Abandoned {
                copies: copies.clone(),
                unread,
            };
            assert_eq!(load_abandoned(&db, wallet), (want, true), "{key}");
            set(&key, None);
        }
    }

    #[test]
    fn test_qt_091_abandon_and_resend_enablement() {
        assert_eq!(f().abandon_refusal(), None);
        assert_eq!(f().resend_refusal(), None);
        let confirmed = Facts { depth: 1, ..f() };
        assert_eq!(
            confirmed.abandon_refusal(),
            Some(TxActionRefusal::Confirmed)
        );
        assert_eq!(confirmed.resend_refusal(), Some(TxActionRefusal::Confirmed));
        let is = Facts {
            instant_locked: true,
            ..f()
        };
        assert_eq!(is.abandon_refusal(), Some(TxActionRefusal::InstantLocked));
        assert_eq!(is.resend_refusal(), Some(TxActionRefusal::InstantLocked));
        let beaten = Facts {
            chainlock_beaten: true,
            ..is
        };
        assert_eq!(beaten.abandon_refusal(), None);
        let under_settled = Facts {
            settled_descendant: true,
            ..f()
        };
        assert_eq!(
            under_settled.abandon_refusal(),
            Some(TxActionRefusal::SettledDescendant)
        );
        assert_eq!(
            beaten.resend_refusal(),
            Some(TxActionRefusal::InstantLocked)
        );
        let gone = Facts {
            abandoned: true,
            ..f()
        };
        assert_eq!(
            gone.abandon_refusal(),
            Some(TxActionRefusal::AlreadyAbandoned)
        );
        assert_eq!(
            gone.resend_refusal(),
            Some(TxActionRefusal::AlreadyAbandoned)
        );
        let incoming = Facts { from_me: 0, ..f() };
        assert_eq!(
            incoming.abandon_refusal(),
            None,
            "dash-qt abandons any unconfirmed"
        );
        assert_eq!(
            incoming.resend_refusal(),
            Some(TxActionRefusal::NotSentByWallet)
        );
        let cb = Facts {
            coinbase: true,
            ..f()
        };
        assert_eq!(cb.abandon_refusal(), Some(TxActionRefusal::Coinbase));
        assert_eq!(cb.resend_refusal(), Some(TxActionRefusal::Coinbase));
    }

    /// An abandoned transaction seen since with an InstantSend lock counts
    /// again (the history's status rule), so Abandon refuses it as locked.
    #[test]
    fn an_abandoned_transaction_seen_instant_locked_refuses_as_locked() {
        let entry = TxEntry {
            tx: Transaction {
                version: 2,
                lock_time: 0,
                input: Vec::new(),
                output: Vec::new(),
                special_transaction_payload: None,
            },
            context: key_wallet::transaction_checking::TransactionContext::InstantSend(
                Default::default(),
            ),
            instant_locked: false,
            first_seen: None,
            own_inputs: Default::default(),
            abandoned: true,
        };
        let (owned, history) = (Default::default(), Default::default());
        let payees = Default::default();
        let view = ChainView {
            owned: &owned,
            history: &history,
            tip: 100,
            chainlock_height: None,
            network: dashcore::Network::Regtest,
            payees: &payees,
            drops: &crate::history::NO_DROPS,
        };
        assert_eq!(
            facts(&entry, &view, &ChainProof::default()).abandon_refusal(),
            Some(TxActionRefusal::InstantLocked)
        );
    }

    /// Two loaded wallets: A stored only the lock of C, which spends P; B
    /// holds C's record unlocked. In either order, B's record doesn't clear
    /// the settled C that A's lock added, so P stays under it.
    #[test]
    fn a_lock_only_child_stays_settled_under_another_wallets_unlocked_record() {
        let p = Txid::from_byte_array([1; 32]);
        let c = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: OutPoint::new(p, 0),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let wallet = key_wallet::Wallet::new_random(
            dashcore::Network::Regtest,
            key_wallet::wallet::initialization::WalletAccountCreationOptions::Default,
        )
        .unwrap();
        let core = ManagedWalletInfo::from_wallet(&wallet, 0);
        let mut a = WalletHistory::default();
        a.lock_parents.insert(c.txid(), vec![p]);
        let mut b = WalletHistory::default();
        b.txs.insert(
            c.txid(),
            TxEntry {
                tx: c.clone(),
                context: TransactionContext::Mempool,
                instant_locked: false,
                first_seen: None,
                own_inputs: Default::default(),
                abandoned: false,
            },
        );
        let (unlocked, _) = settled_under(&b.txs, &core);
        assert!(!unlocked.contains(&p), "B alone settles nothing");
        for order in [[&a, &b], [&b, &a]] {
            let mut known = HashMap::new();
            for h in order {
                add_wallet(&mut known, h, &core);
            }
            assert!(known[&c.txid()].settled);
            let (under, mined) = walk_known(&known);
            assert!(under.contains(&p));
            assert!(!mined.contains(&p));
        }
    }

    #[test]
    fn test_qt_033_coinjoin_internal_types() {
        assert!(is_coinjoin_internal(TxType::CoinJoinMixing));
        assert!(is_coinjoin_internal(TxType::CoinJoinCreateDenominations));
        assert!(!is_coinjoin_internal(TxType::CoinJoinSend));
        assert!(!is_coinjoin_internal(TxType::RecvWithAddress));
    }
}

/// Tests: whether an abandon asked to stop the rebroadcast of `txid`
/// ([`NetworkSession::stop_rebroadcasts`]).
#[cfg(test)]
pub(crate) fn rebroadcast_stop_asked(session: &NetworkSession, txid: Txid) -> bool {
    (session.hub.stopped_rebroadcasts.lock().unwrap().iter()).any(|(t, _)| *t == txid)
}
