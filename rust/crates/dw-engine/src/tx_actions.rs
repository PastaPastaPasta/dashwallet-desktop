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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    filtered, orphaned_unmined, records_of, rival_heights, status_of,
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
/// A transaction this app dropped as one that can never confirm, or built
/// on one (#739): a load seeds the walk of what to drop with it, and never
/// restores it from a sent mark.
const DROPPED_PREFIX: &str = "dropped:";

/// The transactions dropped as ones that can never confirm, or built on
/// one, each with why ([`DROPPED_PREFIX`]).
pub(crate) type Dead = BTreeMap<Txid, DropCause>;

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

/// Settings key prefix (wallet scope) of a transaction dropped as dead
/// ([`NetworkSession::drop_dead_unmined`]), written with its abandoned
/// mark: the coins it spent come back only through the drop's rewind, so
/// an open that finishes its sweep keeps them all stored spent. The value
/// lists the coins it spent that the drop holds for good (`txid:vout`,
/// comma-separated), restored as claims at each load ([`dropped_holds`]).
/// Not `dropped:`, which marks a transaction the claim rule dropped (#739)
/// and holds nothing: neither reads the other's keys.
const DROP_HOLDS_PREFIX: &str = "drop_holds:";

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
    let scope = wallet.to_string();
    let rows = |prefix: &str| match appdb.settings_with_prefix_lossy(&scope, prefix) {
        Ok(rows) => Some(
            (rows.into_iter())
                .filter_map(|(key, value)| {
                    Some((Txid::from_str(key.get(prefix.len()..)?).ok()?, value))
                })
                .collect::<BTreeMap<_, _>>(),
        ),
        Err(e) => {
            tracing::warn!(%wallet, prefix, error = %e, "could not read the sent marks");
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
    forget_swept(appdb, wallet, gone, &Dead::new(), &[], &[])
}

/// [`forget_sent`] of a sweep's `gone`, recording `dead` as dropped
/// ([`DROPPED_PREFIX`]) and `claims`, swept as claims
/// ([`CLAIM_SWEPT_PREFIX`]), in the same write: a load seeds its walk with
/// the dead even when the drop of what is built on them never ran (#739),
/// and takes no child of a claim as orphaned. Each of `kept`, swept
/// transactions the history keeps listed as dropped, is marked abandoned
/// with its copy in it too, so a load lists it again.
pub(crate) fn forget_swept(
    appdb: &AppDb,
    wallet: WalletId,
    gone: &[Txid],
    dead: &Dead,
    claims: &[Txid],
    kept: &[(Txid, Transaction, OwnInputs)],
) -> Result<(), dw_appdb::AppDbError> {
    if gone.is_empty() && dead.is_empty() && claims.is_empty() {
        return Ok(());
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
        .flat_map(|t| [SENT_PREFIX, SENT_TX_PREFIX, SENT_IN_PREFIX].map(|p| format!("{p}{t}")))
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
        .collect();
    #[cfg(all(test, debug_assertions))]
    appdb.fault_point(FORGET_SENT_FAULT, dw_appdb::fault::Stage::Before)?;
    appdb.set_settings_durable(&wallet.to_string(), &writes)
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
    let spends_by = |i: &dashcore::TxIn| i.previous_output.txid == by;
    let claims: Vec<&Transaction> = (swept.iter())
        .filter(|tx| tx.input.iter().any(spends_by))
        .collect();
    if claims.is_empty() {
        return None;
    }
    if let Some(tx) = (claims.iter())
        .find(|tx| (tx.input.iter()).any(|i| spends_by(i) && !push_only(&i.script_sig)))
    {
        tracing::warn!(%wallet, claim = %tx.txid(), %by, "kept what is built on a dropped claim whose input script isn't push-only");
        return None;
    }
    let mut dead: Dead = (claims.iter())
        .map(|tx| (tx.txid(), DropCause::InvalidClaim))
        .collect();
    loop {
        let before = dead.len();
        for tx in swept {
            if (tx.input.iter()).any(|i| dead.contains_key(&i.previous_output.txid)) {
                dead.entry(tx.txid()).or_insert(DropCause::InvalidSource);
            }
        }
        if dead.len() == before {
            return Some(dead);
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

/// Finishes in the wallet store the abandons whose marks are stored but
/// whose sweeps are not (the mark is the commit point): of `wallets`, each
/// payment marked abandoned that is still stored out of a block and
/// unlocked (its record in the mempool, or only its outputs) is swept, and
/// the coins it spent that no other stored record spends are released,
/// except a dropped one's ([`DROP_HOLDS_PREFIX`]).
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
        let (outputs, locked) = crate::history_ops::txids_with_outputs_and_locks(
            db_path,
            wallet,
            marks.keys().copied(),
        )
        .map_err(|e| storage(&e))?;
        let mut pending: BTreeMap<Txid, Option<Transaction>> = BTreeMap::new();
        for (t, copy) in marks {
            let mined = persisted.recordless.contains(&t) && !persisted.unconfirmed.contains(&t);
            if mined || locked.contains(&t) {
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
        for t in persisted
            .records
            .iter()
            .filter(|t| !pending.contains_key(*t))
        {
            if let Some(r) = persister
                .get_core_tx_record(wallet.0, t)
                .map_err(|e| storage(&e))?
            {
                spent_by_others.extend(r.transaction.input.iter().map(|i| i.previous_output));
            }
        }
        let drop_held: HashSet<Txid> = (appdb
            .settings_with_prefix_lossy(&wallet.to_string(), DROP_HOLDS_PREFIX))
        .map_err(|e| storage(&e))?
        .into_iter()
        .filter_map(|(key, _)| Txid::from_str(&key[DROP_HOLDS_PREFIX.len()..]).ok())
        .collect();
        let mut sent = None;
        let (mut spent, mut held) = (BTreeSet::new(), BTreeSet::new());
        for (t, copy) in &pending {
            let spent = match drop_held.contains(t) {
                true => &mut held,
                false => &mut spent,
            };
            match copy {
                Some(tx) => spent.extend(tx.input.iter().map(|i| i.previous_output)),
                None => {
                    if sent.is_none() {
                        let (read, complete) = load_sent(appdb, wallet);
                        if !complete {
                            return Err(storage(&"the sent marks do not read"));
                        }
                        sent = Some(read);
                    }
                    let coins = (sent.iter()).filter_map(|s| s.get(t));
                    spent.extend(coins.flat_map(|m| m.coins.iter().copied()));
                }
            }
        }
        let released: BTreeSet<OutPoint> = (spent.into_iter())
            .filter(|o| {
                !pending.contains_key(&o.txid) && !spent_by_others.contains(o) && !held.contains(o)
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

/// What an abandon that drops transactions that can never confirm (#739)
/// carries: those already dropped whose outputs it spends (`dead`: none of
/// their outputs is released or scanned for again, as none exists), and
/// what must still hold, read under the lock that applies it.
pub(crate) struct DeadDrop {
    dead: BTreeSet<Txid>,
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

/// Whether `t`, or anything that descends from it in this wallet
/// ([`settled_in_tree`]) or in any loaded one (`settled.under`), is
/// settled: then nothing of its tree is dropped (#739).
fn tree_settled(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    settled: &Settled,
    t: Txid,
) -> Option<Txid> {
    settled_in_tree(txs, core, t).or_else(|| settled.under.contains(&t).then_some(t))
}

/// The first transaction of `root`'s tree (it and all that descends from
/// it, in the history and in key-wallet's records) that is mined,
/// InstantSend-locked, or held settled by key-wallet.
fn settled_in_tree(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    root: Txid,
) -> Option<Txid> {
    let locks = core.instant_send_locks();
    let records: Vec<(Txid, &Transaction)> = (core.accounts.all_accounts().into_iter())
        .filter(|a| !a.managed_account_type().is_contact_owned())
        .flat_map(|a| a.transactions().iter().map(|(t, r)| (*t, &r.transaction)))
        .collect();
    let (mut todo, mut seen) = (vec![root], HashSet::new());
    while let Some(t) = todo.pop() {
        if !seen.insert(t) {
            continue;
        }
        let settled = txs
            .get(&t)
            .is_some_and(|e| e.context.block_info().is_some() || locked(e))
            || locks.contains(&t)
            || held_settled(core, &t).is_some();
        if settled {
            return Some(t);
        }
        let spends = |tx: &Transaction| tx.input.iter().any(|i| i.previous_output.txid == t);
        todo.extend((txs.iter()).filter(|(_, e)| spends(&e.tx)).map(|(c, _)| *c));
        todo.extend(
            (records.iter())
                .filter(|(_, tx)| spends(tx))
                .map(|(c, _)| *c),
        );
    }
    None
}

/// Whether every instruction of `script` parses and pushes bytes, as
/// [`key_wallet::transaction_checking::verify_input_authorization`] reads
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
/// [`verify_input_authorization`]: key_wallet::transaction_checking::verify_input_authorization
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
        key_wallet::transaction_checking::verify_input_authorization(&e.tx, n, output) == Ok(false)
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
        let k = known.entry(t).or_insert_with(|| Known {
            parents: tx.input.iter().map(|i| i.previous_output.txid).collect(),
            ..Known::default()
        });
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

/// Adds the transactions with a verified InstantSend lock stored and no
/// record, linked to their parents by the inputs their lock names (#611):
/// a locked child whose record is gone still proves its parents.
fn add_locked_without_record(
    known: &mut HashMap<Txid, Known>,
    lock_parents: &HashMap<Txid, Vec<Txid>>,
) {
    for (t, parents) in lock_parents {
        known
            .entry(*t)
            .or_insert_with(|| Known {
                parents: parents.clone(),
                ..Known::default()
            })
            .settled = true;
    }
}

/// Adds one loaded wallet to `known`: its history and records, then its
/// stored locks without a record. Neither clears what another wallet's
/// entry for the same transaction set.
fn add_wallet(known: &mut HashMap<Txid, Known>, h: &WalletHistory, core: &ManagedWalletInfo) {
    add_known(known, &h.txs, core);
    add_locked_without_record(known, &h.lock_parents);
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
        infos: impl IntoIterator<Item = (&'a [u8; 32], &'a ManagedWalletInfo)>,
    ) -> Settled {
        let (mut known, mut incomplete) = (HashMap::new(), None);
        for (id, core) in infos {
            let unread = (self.hub.history).read_wallet(&WalletId(*id), |h| {
                add_wallet(&mut known, h, core);
                h.load_incomplete
            });
            if unread {
                incomplete.get_or_insert(WalletId(*id));
            }
        }
        let (under, mined) = walk_known(&known);
        Settled {
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
    core: &ManagedWalletInfo,
    roots: &[Txid],
    winner: Winner,
    links: &BTreeSet<Txid>,
    (complete, chainlock, under): (bool, Option<u32>, &HashSet<Txid>),
) -> Option<DropPlan> {
    let (mut dead, mut links) = (BTreeSet::new(), links.clone());
    for root in roots {
        let open = (txs.get(root))
            .is_some_and(|e| e.context.block_info().is_none() && !e.is_abandoned() && !locked(e));
        if !open
            || dead.contains(root)
            || under.contains(root)
            || held_settled(core, root).is_some()
        {
            continue;
        }
        let (descendants, passed) = unmined_descendants(txs, core, *root);
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
    let beaten_lock = |t: &Txid, tx: &Transaction| {
        (tx.input.iter()).any(|i| {
            (observed.get(&i.previous_output)).is_some_and(|h| chainlock.is_some_and(|cl| cl >= *h))
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
    locked_spent.retain(|o| !observed.contains_key(o));
    // A lock key-wallet heard of for a transaction neither holds: what it
    // spends is unknown, so nothing is freed.
    let unknown_lock = (core.instant_send_locks().iter()).any(|t| {
        !txs.contains_key(t) && !accounts.iter().any(|a| a.transactions().contains_key(t))
    });
    let known = complete
        && !unknown_lock
        && (winner.mined.is_some() || winner.txid.is_some_and(|w| txs.contains_key(&w)));
    let freed: BTreeSet<OutPoint> = (inputs.iter())
        .filter(|o| known && !kept.contains(*o) && !observed.contains_key(*o))
        .copied()
        .collect();
    kept.extend(inputs.iter().filter(|o| !freed.contains(*o)));
    let inputs_observed = (inputs.iter())
        .filter(|o| observed.contains_key(*o))
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
        } else if self.settled_descendant {
            Some(TxActionRefusal::SettledDescendant)
        } else if self.coinbase {
            Some(TxActionRefusal::Coinbase)
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
        // The exception's evidence on the inputs, as the abandon checks it:
        // not offered when it would refuse as locked.
        if offer && !f.instant_locked && f.abandon_refusal().is_none() {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let infos = wm.get_all_wallet_infos().iter();
            let settled = self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)));
            f.settled_descendant = (self.hub.history).read(&wallet, |txs| {
                settled_descendant(txs, &settled, &txid, &proof)
            });
        }
        if offer && f.chainlock_beaten && f.abandon_refusal().is_none() {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let infos = wm.get_all_wallet_infos().iter();
            let settled = self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)));
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
        f.abandon_marked = self.abandon_marked(wallet, txid).await;
        Ok((entry, f))
    }

    /// Whether an Abandon of `txid` committed its mark, which it writes
    /// before its sweep: a mark that cannot be read counts.
    pub(crate) async fn abandon_marked(&self, wallet: WalletId, txid: Txid) -> bool {
        let (scope, key) = (wallet.to_string(), format!("{ABANDONED_PREFIX}{txid}"));
        (self.appdb_op(move |db| {
            Ok((db.settings_with_prefix_lossy(&scope, &key)?.into_iter()).any(|(k, _)| k == key))
        }))
        .await
        .unwrap_or(true)
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
                can_resend: f.resend_refusal().is_none(),
                dust_locked_outputs,
                withheld_outputs,
                checking_spent_coins: f.abandoned && {
                    let held = this.hub.rescan_held(&wallet);
                    (entry.tx.input.iter()).any(|i| held.contains(&i.previous_output))
                },
                last_announced_at: this.hub.announced_at(&txid),
            })
        })
        .await
    }

    /// dash-qt "Abandon transaction" (QT-091). Refusals as
    /// [`TxActionRefusal`]; the abandoned transaction and its recorded
    /// descendants show as `Abandoned`, their change leaves the balance and
    /// the coins they spent come back through a rescan.
    ///
    /// Needs a running SPV client (`SpvNotRunning`, checked first, review
    /// M2): the spent coins return only through the filter rescan, which
    /// rewinds the running client's in-memory sync height. Abandoning
    /// offline would leave them missing from the balance.
    pub async fn abandon_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // Not while a wallet loads: its spend claims are read before
            // they are restored.
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            this.require_wallet(&wallet)?;
            let spv_running = manager.spv().is_started();
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
        let mut recorded = None;
        let result = self
            .abandon_unrewound(manager, wallet, root, None, &mut recorded)
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
    /// abandon drops transactions that can never confirm (#739).
    async fn abandon_unrewound(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
        dead_drop: Option<&DeadDrop>,
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
        let proof = self.root_proof(manager, wallet, root).await;
        let _gate = self.abandon_gate(wallet).write_owned().await;
        // Planned on a copy under the lock that applies it, so the replay
        // record stored before it covers what is abandoned; held until the
        // sweep is stored, so no descendant is credited between the plan and
        // the sweep.
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let held_since = std::time::Instant::now();
        let (abandoned, released, overridden, txs, links) = {
            let settled = {
                let infos = wm.get_all_wallet_infos().iter();
                self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)))
            };
            let info = wm
                .get_wallet_info_mut(&wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
            if let Some(d) = dead_drop
                && !(self.hub.history)
                    .read(&wallet, |txs| (d.holds)(txs, &info.core_wallet, &settled))
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
            if let Some(refusal) =
                (descendants.iter().chain(&links)).find_map(|t| held_settled(&info.core_wallet, t))
            {
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
            // A coin another transaction of the wallet still spends stays
            // spent, and is neither released nor rescanned for: one
            // key-wallet records and does not abandon (it keeps conflicting
            // mempool spends), or one the history holds and this abandon
            // leaves (it loads every stored record; key-wallet holds none
            // for a payment hydrated at load).
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
            // Read before key-wallet abandons anything (which leaves its
            // observed spends as they are), so the coins' holds go with the
            // marks.
            let mut released = overridden.clone().unwrap_or_else(|| {
                self.released(
                    wallet,
                    &planned,
                    &links,
                    &copies,
                    info.core_wallet.observed_spent_outpoints(),
                )
            });
            released.retain(|o| !still_spent.contains(o));
            // The rescan covers every coin given back too: each waits for
            // it (#818).
            let checkpoint = [
                self.abandon_rewind(wallet, &planned, &links, &still_spent, &info.core_wallet),
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
            self.write_abandoned_marks(wallet, marks).await?;
            let previous = (self.hub.history).with_wallet(wallet, |h| {
                crate::rescan_hold::take_over(&mut h.rescan_holds, &holds)
            });
            let abandoned = abandon_with(&mut info.core_wallet, root, &descendants);
            if abandoned.is_empty() {
                self.hub.history.with_wallet(wallet, |h| {
                    crate::rescan_hold::put_back(&mut h.rescan_holds, &holds, &previous)
                });
                let undo = crate::rescan_hold::undo_marks(&holds, &previous);
                if let Err(e) = self.write_marks(wallet, undo).await {
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
            (abandoned, released, overridden.is_some(), txs, links)
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
        drop(wm);
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
                }
            }
        });
        (self.hub.history).drop_claims(&wallet, abandoned.iter().copied());
        // Each abandoned transaction's own claim (DEC-179); a coin another
        // owner holds stays reserved.
        for t in &abandoned {
            self.spends
                .release(&wallet, crate::send::SpendOwner::Tx(*t));
        }
        Ok(abandoned)
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
        let _gate = self.hub.drop_gate.lock().await;
        let mut work: Vec<_> = (self.hub.take_dead_children(wallet).into_iter())
            .map(|(roots, winner, swept)| (roots, winner, swept, false))
            .collect();
        {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            if let Some(info) = wm.get_wallet_info(&wallet.0) {
                let core = &info.core_wallet;
                let chainlock = self.chainlock_height(core);
                let recorded = |t: &Txid| {
                    (core.accounts.all_accounts().into_iter()).any(|a| a.has_transaction(t))
                };
                let mut owned = None;
                let pays_wallet = |tx: &Transaction| {
                    let owned =
                        owned.get_or_insert_with(|| crate::history_ops::owned_scripts(info));
                    (tx.output.iter()).any(|o| owned.contains_key(&o.script_pubkey))
                };
                (self.hub.history).read_wallet(&wallet, |h| {
                    if h.load_incomplete || !h.loaded {
                        return;
                    }
                    let observed = core.observed_spent_outpoints();
                    let mut beaten: BTreeMap<Winner, Vec<Txid>> = BTreeMap::new();
                    for (t, w) in
                        beaten_unrecorded(&h.txs, recorded, observed, chainlock, pays_wallet)
                    {
                        beaten.entry(w).or_default().push(t);
                    }
                    work.extend((beaten.into_iter()).map(|(w, ts)| (ts, w, BTreeSet::new(), true)));
                    let orphans = stored.map(|(s, exempt)| orphaned_unmined(&h.txs, s, exempt));
                    if let Some(orphans) = orphans.filter(|o| !o.is_empty()) {
                        work.push((orphans, Winner::default(), BTreeSet::new(), false));
                    }
                });
            }
        }
        for (roots, winner, links, recheck) in work {
            let dropped = self.drop_dead_unmined(manager, wallet, &roots, winner, &links, recheck);
            match dropped.await {
                Ok(dropped) if !dropped.is_empty() => {
                    tracing::info!(%wallet, ?dropped, ?winner, "dropped transactions that can never confirm")
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(%wallet, error = %e, "could not drop transactions that can never confirm")
                }
            }
        }
    }

    /// Drops `roots` and what descends from them ([`plan_drop`]) as an
    /// abandon does: marked abandoned and dropped first, then abandoned in
    /// key-wallet and swept from the store; the history shows them
    /// abandoned, as their marks do at the next load, and their sent marks
    /// go. A coin they spent comes back only through the rewind, which is
    /// recorded before the marks and made after the sweep, and only when
    /// the plan frees it; the sweep stores none unspent. Every other coin
    /// they spent is claimed for good. Holds the wallet's abandon gate for
    /// writing through its marks, as an abandon does, so no resend hands off
    /// bytes it has marked. Returns the dropped txids.
    pub(crate) async fn drop_dead_unmined(
        &self,
        manager: &Manager,
        wallet: WalletId,
        roots: &[Txid],
        winner: Winner,
        links: &BTreeSet<Txid>,
        recheck: bool,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let _gate = self.abandon_gate(wallet).write_owned().await;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        // What any loaded wallet settled can descend from this one's
        // payment (#611): nothing goes while another's history is not whole
        // (an incomplete load, or an open under way); this one's own
        // incomplete load frees nothing (`plan_drop`).
        let other_unread = (wm.get_all_wallet_infos().keys())
            .filter(|id| **id != wallet.0)
            .any(|id| self.history_unread(WalletId(*id)));
        if other_unread {
            return Ok(BTreeSet::new());
        }
        let settled = {
            let infos = wm.get_all_wallet_infos().iter();
            self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)))
        };
        let Some(info) = wm.get_wallet_info_mut(&wallet.0) else {
            return Ok(BTreeSet::new());
        };
        let plan = (self.hub.history).read_wallet(&wallet, |h| {
            let complete = h.loaded && !h.load_incomplete;
            let chainlock = self.chainlock_height(&info.core_wallet);
            let facts = (complete, chainlock, &settled.under);
            // A beaten root is beaten still under this lock: a truncation
            // since its detection may have undone its winner.
            let rechecked: Vec<Txid>;
            let roots = match recheck {
                false => roots,
                true => {
                    let core = &info.core_wallet;
                    let recorded = |t: &Txid| {
                        (core.accounts.all_accounts().into_iter()).any(|a| a.has_transaction(t))
                    };
                    let owned = crate::history_ops::owned_scripts(info);
                    let pays = |tx: &Transaction| {
                        (tx.output.iter()).any(|o| owned.contains_key(&o.script_pubkey))
                    };
                    let observed = core.observed_spent_outpoints();
                    let beaten: HashSet<Txid> =
                        beaten_unrecorded(&h.txs, recorded, observed, chainlock, pays)
                            .into_iter()
                            .filter(|(_, w)| *w == winner)
                            .map(|(t, _)| t)
                            .collect();
                    rechecked = roots
                        .iter()
                        .filter(|t| beaten.contains(*t))
                        .copied()
                        .collect();
                    &rechecked
                }
            };
            plan_drop(
                &h.txs,
                &h.swept,
                &info.core_wallet,
                roots,
                winner,
                links,
                facts,
            )
        });
        let Some(plan) = plan else {
            return Ok(BTreeSet::new());
        };
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
                    (format!("{DROP_HOLDS_PREFIX}{t}"), Some(holds(tx))),
                ]
            })
            .chain(hold_marks(&rescan_holds))
            .collect();
        self.write_abandoned_marks(wallet, marks).await?;
        let previous = (self.hub.history).with_wallet(wallet, |h| {
            crate::rescan_hold::take_over(&mut h.rescan_holds, &rescan_holds)
        });
        let mut dead = plan.dead.iter().copied();
        let root = dead.next().expect("a plan drops something");
        let rest: Vec<Txid> = dead.collect();
        let core = &mut info.core_wallet;
        let abandoned = abandon_with(core, root, &rest);
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
                        (format!("{DROP_HOLDS_PREFIX}{t}"), None),
                    ]
                })
                .chain(crate::rescan_hold::undo_marks(&rescan_holds, &previous))
                .collect();
            self.write_marks(wallet, undo).await?;
            return Ok(abandoned);
        }
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
        let held = core.restore_spent_outpoints(&claims);
        for account in core.accounts.all_funding_accounts_mut() {
            for o in &held {
                account.utxos.remove(o);
            }
        }
        core.update_balance();
        // The winner's height stamps the held coins only when its block
        // spent each of them: a stamped placeholder goes once final.
        let observed = core.observed_spent_outpoints();
        let all_won = (claims.iter()).all(|(o, _)| observed.contains_key(o));
        let sweep = SweepBatch {
            txids: abandoned.iter().chain(&plan.links).copied().collect(),
            superseded_by: winner.txid.unwrap_or(root),
            winner_mined_height: winner.mined.filter(|_| all_won),
            released_outpoints: Vec::new(),
        };
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
        // key-wallet holds no record of a payment restored at load, so
        // it cannot see the InstantSend lock rehydrated for it.
        let locks = core.instant_send_locks();
        let overridden =
            self.lock_override(wallet, &root, &planned, proof, core, &trial, &settled.under)?;
        let exempt = |t: &Txid| overridden.is_some() && *t == root;
        // key-wallet walks by its own records: one it holds as pending can
        // be locked or mined in the history, and then what spends it is
        // not the wallet's to abandon.
        let (mined, held_locked) = (self.hub.history).read(&wallet, |txs| {
            let settled = |f: &dyn Fn(&TxEntry) -> bool| {
                (planned.iter()).any(|t| !exempt(t) && txs.get(t).is_some_and(f))
            };
            (
                settled(&|e| e.context.block_info().is_some()),
                settled(&|e| locked(e)),
            )
        });
        if mined {
            return Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed));
        }
        if held_locked || planned.iter().any(|t| locks.contains(t) && !exempt(t)) {
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
    /// history or by key-wallet's locks: only then does the exception read
    /// it.
    async fn root_proof(&self, manager: &Manager, wallet: WalletId, root: Txid) -> ChainProof {
        let locked = match self
            .hub
            .history
            .read(&wallet, |txs| txs.get(&root).map(locked))
        {
            None => false,
            Some(true) => true,
            Some(false) => {
                let wm = manager.wallet_manager_arc();
                let wm = wm.read().await;
                (wm.get_wallet_info(&wallet.0)).is_some_and(|i| {
                    i.core_wallet.instant_send_locks().contains(&root) || {
                        let infos = wm.get_all_wallet_infos().iter();
                        let settled = self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)));
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
        let (incomplete, claimed) =
            (self.hub.history).with_wallet(wallet, |h| (h.load_incomplete, h.claimed.clone()));
        self.hub.history.read(&wallet, |txs| {
            let Some(e) = txs.get(root) else {
                return Ok(None);
            };
            if !locked(e) && !core.instant_send_locks().contains(root) {
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
                || incomplete
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
                    None if claimed.get(&o) == Some(root) => claim_held = true,
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
            let roots: Vec<Txid> = self.hub.history.read(&wallet, |txs| {
                (txs.iter())
                    .filter(|(_, e)| locked(e) && e.context.block_info().is_none())
                    .map(|(t, _)| *t)
                    .collect()
            });
            if roots.is_empty() {
                return ChainProof::default();
            }
            let settled = {
                let infos = wm.get_all_wallet_infos().iter();
                self.settled_all(infos.map(|(id, i)| (id, &i.core_wallet)))
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
            let funded = (self.hub.history.get(&wallet, &o.txid))
                .and_then(|e| e.context.block_info().map(|b| b.height()));
            match funded {
                Some(h) => lowest = Some(lowest.map_or(h, |l| l.min(h))),
                None => return Some(birth),
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
            let manager = this.manager()?;
            let (entry, f) = this.tx_facts(&manager, wallet, txid, false).await?;
            let peers = this.hub.tracker().snapshot().connected_peers;
            if let Some(r) = activity_resend_refusal(&f, manager.spv().is_started(), peers) {
                return Err(r.into());
            }
            #[cfg(test)]
            let pause = this.hub.pause_resend.lock().unwrap().take();
            #[cfg(test)]
            if let Some(pause) = pause {
                pause.mark_reached();
                pause.release.notified().await;
            }
            // Read again, held against an abandon through the hand-off: one
            // since the check committed its mark, and its bytes stay unsent.
            let gate = this.abandon_gate(wallet).read_owned().await;
            if this.abandon_marked(wallet, txid).await {
                return Err(EngineError::TxActionRefused(
                    TxActionRefusal::AlreadyAbandoned,
                ));
            }
            let handle = this.wallet(&wallet).await?;
            #[cfg(test)]
            let wire = this.hub.resend_wire.lock().unwrap().clone();
            // platform-wallet's only broadcast entry point waits for the
            // network's acceptance (up to dash-spv's 60 s timeout). The send
            // itself happens at once, so the call returns after a short
            // hand-off window and leaves the wait to a background task; a
            // send dash-spv refuses outright (no connection) is reported.
            let mut task = tokio::spawn(async move {
                let _gate = gate;
                #[cfg(test)]
                if let Some(wire) = wire {
                    wire.lock().unwrap().push(entry.tx.txid());
                    return Ok(entry.tx.txid());
                }
                let outcome = handle.core().broadcast_transaction(&entry.tx).await;
                match &outcome {
                    Ok(_) => tracing::info!(%txid, "resent transaction accepted"),
                    Err(e) => {
                        tracing::info!(%txid, error = %e, "resent transaction: no acceptance seen")
                    }
                }
                outcome
            });
            {
                let mut resends = this.resends.lock().unwrap_or_else(|p| p.into_inner());
                resends.retain(|r| !r.is_finished());
                resends.push(task.abort_handle());
            }
            match tokio::time::timeout(RESEND_HANDOFF, &mut task).await {
                Ok(Ok(Err(platform_wallet::PlatformWalletError::TransactionBroadcast(_)))) => {
                    return Err(EngineError::NoPeers);
                }
                Ok(Err(e)) => return Err(e.into()),
                Ok(Ok(_)) | Err(_) => {}
            }
            this.hub.note_announced(txid);
            Ok(())
        })
        .await
    }

    /// iOS "remove unconfirmed" (IOS-034): abandons every transaction of
    /// `wallet` (all loaded wallets for `None`) that Abandon would: out of a
    /// block, not abandoned, and not InstantSend-locked unless a ChainLocked
    /// spend of one of its inputs beat it; and rescans from the lowest
    /// funding height of the coins they spent. Needs a running SPV client
    /// for the rescan. Returns how many transactions were dropped
    /// (descendants included).
    pub async fn drop_unconfirmed(
        self: &Arc<Self>,
        wallet: Option<WalletId>,
    ) -> Result<u32, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            if let Some(id) = &wallet {
                this.require_wallet(id)?;
            }
            if !manager.spv().is_started() {
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
                        .abandon_unrewound(&manager, id, txid, None, &mut lowest)
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
                        .abandon_unrewound(&manager, id, txid, None, &mut lowest)
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
    /// from it, as Remove Unconfirmed does, unless what it spends of `dead`
    /// has a settled descendant in any loaded wallet.
    pub(crate) async fn abandon_on_dead(
        &self,
        manager: &Manager,
        wallet: WalletId,
        dead: &Dead,
        at_load: bool,
    ) {
        if at_load {
            self.prune_dropped(wallet, dead).await;
        }
        // A dropped one the history holds pending again (a peer relayed it
        // since) is dropped again itself; each is vetoed by what it spends
        // of `dead` only.
        let children: Vec<(Txid, BTreeSet<Txid>)> = (self.hub.history).read(&wallet, |txs| {
            (txs.iter())
                .filter(|(_, e)| pending(e))
                .filter_map(|(t, e)| {
                    let parents: BTreeSet<Txid> = (e.tx.input.iter())
                        .map(|i| i.previous_output.txid)
                        .filter(|p| dead.contains_key(p))
                        .collect();
                    (dead.contains_key(t) || !parents.is_empty()).then_some((*t, parents))
                })
                .collect()
        });
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
                let spent = parents.clone();
                let holds = move |txs: &HashMap<Txid, TxEntry>,
                                  core: &ManagedWalletInfo,
                                  settled: &Settled| {
                    let still = txs.get(&child).is_some_and(|e| {
                        pending(e)
                            && (again
                                || (e.tx.input.iter())
                                    .any(|i| spent.contains(&i.previous_output.txid)))
                    });
                    if !still {
                        return false;
                    }
                    match (tree.iter()).find_map(|t| tree_settled(txs, core, settled, *t)) {
                        Some(at) => {
                            tracing::warn!(%wallet, %child, settled = %at, "kept what descends from a dead transaction: one of its tree is settled");
                            false
                        }
                        None => true,
                    }
                };
                let drop = DeadDrop {
                    dead: parents,
                    claims,
                    holds: Box::new(holds),
                };
                (child, drop)
            })
            .collect();
        self.drop_trees(manager, wallet, drops).await;
    }

    /// Forgets the `dropped:` rows of `dead` the history now holds mined or
    /// InstantSend-locked: the verdict that dropped them was wrong, and
    /// they no longer seed the load's walk.
    async fn prune_dropped(&self, wallet: WalletId, dead: &Dead) {
        let settled: Vec<Txid> = (self.hub.history).read(&wallet, |txs| {
            (dead.keys())
                .filter(|t| {
                    txs.get(*t)
                        .is_some_and(|e| e.context.block_info().is_some() || locked(e))
                })
                .copied()
                .collect()
        });
        if settled.is_empty() {
            return;
        }
        tracing::warn!(%wallet, ?settled, "dropped transactions are settled since: their records go");
        let scope = wallet.to_string();
        let result = self
            .appdb_op(move |db| {
                let keys: Vec<String> = (settled.iter())
                    .map(|t| format!("{DROPPED_PREFIX}{t}"))
                    .collect();
                let writes: Vec<(&str, Option<&str>)> =
                    keys.iter().map(|k| (k.as_str(), None)).collect();
                db.set_settings_durable(&scope, &writes)
            })
            .await;
        if let Err(e) = result {
            tracing::warn!(%wallet, error = %e, "could not forget the records of settled dropped transactions");
        }
    }

    /// After a restart key-wallet holds no pending record, so it can't drop
    /// a claim a new transaction (`funded`) shows unsigned: the history's
    /// claims on a coin of the wallet `funded` pays ([`unsigned_claim`]).
    pub(crate) async fn unsigned_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        funded: &BTreeSet<Txid>,
    ) -> Vec<Txid> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&wallet.0) else {
            return Vec::new();
        };
        (self.hub.history).read(&wallet, |txs| {
            (txs.keys())
                .filter(|t| unsigned_claim(txs, &info.core_wallet, **t, funded))
                .copied()
                .collect()
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
                    match tree_settled(txs, core, settled, claim) {
                        Some(at) => {
                            tracing::warn!(%wallet, %claim, settled = %at, "kept an unsigned claim: what descends from it is settled");
                            false
                        }
                        None => true,
                    }
                };
                let drop = DeadDrop {
                    dead: BTreeSet::new(),
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
            match (self.abandon_unrewound(manager, wallet, root, Some(&drop), &mut lowest)).await {
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

    /// Hooks [`Self::abandon_on_dead`] to the hub's sweep events, and
    /// [`Self::abandon_unsigned_claims`] to its new transactions.
    pub(crate) fn attach_dead_claims(self: &Arc<Self>) {
        let (weak, rt) = (Arc::downgrade(self), self.rt.clone());
        self.hub.attach_swept_dead(Box::new(move |id, dead| {
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
                (session.abandon_on_dead(&manager, id, &dead, false)).await;
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
                // Most new transactions fund no claim: checked before the
                // wallets' lifecycle lock is taken.
                let claims = session.unsigned_claims(&manager, id, &funded).await;
                if claims.is_empty() {
                    return;
                }
                let _lifecycle = session.wallet_lifecycle.hold().await;
                (session.abandon_unsigned_claims(&manager, id, claims, &funded)).await;
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

    /// The wallet-scope marks are read by prefix (the drops' holds, the
    /// claim rule's dropped records, the claims swept, the abandoned and
    /// sent marks): no key of one is read as another's.
    #[test]
    fn the_drop_note_prefixes_are_apart() {
        assert_ne!(DROP_HOLDS_PREFIX, DROPPED_PREFIX);
        let prefixes = [
            DROP_HOLDS_PREFIX,
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
        }
    }

    /// Which transactions the dropped marks name comes from their keys
    /// alone (#790): an empty value (an earlier build's), a cause, or a
    /// value that does not parse name the same set, and none fails the
    /// read. The value gives only why: a never-valid claim's, else what is
    /// built on one.
    #[test]
    fn the_dropped_marks_name_the_same_transactions_whatever_their_values() {
        use dashcore::hashes::Hash;
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
        use dashcore::hashes::Hash;
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
        use dashcore::hashes::Hash;
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
        use dashcore::hashes::Hash;
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
        use dashcore::hashes::Hash;
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
