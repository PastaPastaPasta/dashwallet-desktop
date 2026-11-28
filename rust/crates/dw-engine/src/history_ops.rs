//! Session operations over the history read model: loading persisted
//! history at open, `history_page` and `tx_detail`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::{OutPoint, ScriptBuf, Txid};
use dw_appdb::{AppDb, LabelKind};
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::managed_account_type::ManagedAccountType;
use key_wallet::managed_account::managed_core_funds_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::transaction_record::TransactionRecord;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletInfo;
use platform_wallet::changeset::PlatformWalletPersistence;

use crate::history::{
    AddressChain, ChainView, HistoryPage, HistoryQuery, Labels, Owned, TxDetail, TxEntry,
    detail_of, page, records_of, username_purposes, validate_query,
};
use crate::platform::attribution::Payees;
use crate::session::{Manager, WALLET_DB_FILE};
use crate::{EngineError, NetworkSession, WalletId};

/// Every spent claim the engine restores goes through this one helper, so
/// platform-wallet's claim mirror can see each claimed outpoint: a block
/// that later spends one is then captured as a block spend even though the
/// wallet holds neither the coin nor its claimant's record.
pub(crate) fn restore_spent_claims(
    info: &mut PlatformWalletInfo,
    claims: &[(OutPoint, Option<Txid>)],
) -> Vec<OutPoint> {
    #[cfg(test)]
    (claims_restored().lock().unwrap_or_else(|p| p.into_inner()))
        .extend(claims.iter().map(|(o, _)| *o));
    info.core_wallet.restore_spent_outpoints(claims)
}

/// Tests: every outpoint [`restore_spent_claims`] restored a claim on.
#[cfg(test)]
pub(crate) fn claims_restored() -> &'static std::sync::Mutex<HashSet<OutPoint>> {
    static RESTORED: std::sync::OnceLock<std::sync::Mutex<HashSet<OutPoint>>> =
        std::sync::OnceLock::new();
    RESTORED.get_or_init(Default::default)
}

/// Every script the wallet's funds accounts own: BIP44 and BIP32 accounts,
/// DIP9 CoinJoin accounts and DashPay receiving accounts. Watch-only DashPay
/// contact accounts are not the wallet's.
pub(crate) fn owned_scripts(info: &PlatformWalletInfo) -> HashMap<ScriptBuf, Owned> {
    let accounts = &info.core_wallet.accounts;
    let mut map = HashMap::new();
    let mut add = |account: &ManagedCoreFundsAccount, coinjoin: bool| {
        let pools: Vec<(
            &key_wallet::managed_account::address_pool::AddressPool,
            AddressChain,
        )> = match account.managed_account_type() {
            ManagedAccountType::Standard {
                external_addresses,
                internal_addresses,
                ..
            }
            | ManagedAccountType::CoinJoin {
                external_addresses,
                internal_addresses,
                ..
            } => vec![
                (external_addresses, AddressChain::Receiving),
                (internal_addresses, AddressChain::Change),
            ],
            other => other
                .address_pools()
                .into_iter()
                .map(|p| (p, AddressChain::Receiving))
                .collect(),
        };
        for (pool, chain) in pools {
            for info in pool.addresses().values() {
                map.insert(
                    info.address.script_pubkey(),
                    Owned::new(info.address.clone(), chain, coinjoin),
                );
            }
        }
    };
    for a in accounts.standard_bip44_accounts.values() {
        add(a, false);
    }
    for a in accounts.standard_bip32_accounts.values() {
        add(a, false);
    }
    for a in accounts.coinjoin_accounts.values() {
        add(a, true);
    }
    for a in accounts.dashpay_receival_accounts.values() {
        add(a, false);
    }
    map
}

/// Tests: how the next history load's first stored record read goes wrong
/// (`SessionHub::record_read_fault`).
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum RecordFault {
    /// The read fails.
    Error,
    /// The listed record reads as absent.
    Missing,
    /// The record read names another txid.
    Mismatched,
    /// The record read holds another transaction.
    OtherTransaction,
}

#[cfg(test)]
impl RecordFault {
    fn apply(
        self,
        read: Result<Option<TransactionRecord>, platform_wallet::changeset::PersistenceError>,
    ) -> Result<Option<TransactionRecord>, platform_wallet::changeset::PersistenceError> {
        match self {
            Self::Error => Err(platform_wallet::changeset::PersistenceError::LockPoisoned),
            Self::Missing => Ok(None),
            Self::Mismatched => read.map(|r| {
                r.map(|mut r| {
                    r.txid = Txid::all_zeros();
                    r
                })
            }),
            Self::OtherTransaction => read.map(|r| {
                r.map(|mut r| {
                    r.transaction.lock_time = r.transaction.lock_time.wrapping_add(1);
                    r
                })
            }),
        }
    }
}

/// Height of the ChainLock key-wallet applied to the wallet: the only one
/// its finality order lets beat an InstantSend lock.
pub(crate) fn applied_chainlock(info: &PlatformWalletInfo) -> Option<u32> {
    (info.core_wallet.metadata.last_applied_chain_lock.as_ref()).map(|c| c.block_height)
}

/// One wallet's stored transactions: those whose record is stored, and the
/// recordless ones (height-only rows, kept for their coins).
#[derive(Default, Clone)]
pub(crate) struct Persisted {
    pub(crate) records: Vec<Txid>,
    pub(crate) recordless: HashSet<Txid>,
    /// Of `recordless`, the rows without a height: an unconfirmed coin's,
    /// no evidence that its transaction settled.
    pub(crate) unconfirmed: HashSet<Txid>,
    /// Spends the store's open dropped as not signed by their coins'
    /// owner: never restored from a mark. This build's repair records them
    /// in the app database ([`crate::store_repair::store_dropped`]); a store
    /// upgraded from the fork keeps the fork's own in
    /// `core_dropped_unsigned_spends`.
    pub(crate) dropped: HashSet<Txid>,
    /// Of `dropped`, those whose verdict may be wrong, so nothing built on
    /// them is dropped with them (#739): a record that doesn't read, the
    /// repair's structural drops, and the fork's drops whose record has an
    /// input script that isn't push-only.
    pub(crate) dropped_doubtful: HashSet<Txid>,
    /// Transactions with a verified InstantSend lock stored and no record,
    /// by the transactions their lock names inputs of: what links a locked
    /// child whose record is gone to its parents (#611).
    pub(crate) lock_parents: HashMap<Txid, Vec<Txid>>,
    /// A stored lock row didn't decode: which transactions it links is
    /// unknown.
    pub(crate) locks_unread: bool,
}

impl Persisted {
    /// Adds the claims the open's repair dropped from `wallet`'s store
    /// ([`crate::store_repair::store_dropped`]); false when they don't read.
    pub(crate) fn add_store_dropped(&mut self, appdb: &AppDb, wallet: WalletId) -> bool {
        let held = |t: &Txid| self.records.contains(t) || self.recordless.contains(t);
        match crate::store_repair::store_dropped(appdb, wallet, held) {
            Ok((dropped, doubtful)) => {
                self.dropped.extend(dropped);
                self.dropped_doubtful.extend(doubtful);
                true
            }
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "could not read the claims the store's repair dropped");
                false
            }
        }
    }

    /// Whether a load restores `txid`'s claims from its sent mark: not a
    /// spend the store's open dropped, nor an abandoned payment whose saved
    /// copy doesn't read back (`unread`) unless the store still holds it.
    pub(crate) fn keeps_mark(&self, txid: &Txid, unread: &BTreeSet<Txid>) -> bool {
        !self.dropped.contains(txid)
            && (!unread.contains(txid)
                || self.records.contains(txid)
                || self.recordless.contains(txid))
    }
}

/// Txids of the stored transactions per wallet, read from `wallet.sqlite`
/// over a read-only connection (SqlitePersister has no enumeration API;
/// `get_core_tx_record` then reads each record).
/// The stored records of `wallet` named by `txids` that `read` returns:
/// each the listed key's record, of the transaction it names. False when
/// one was missing, another's or unreadable (logged).
fn read_records<'a, E: std::fmt::Display>(
    wallet: WalletId,
    txids: impl IntoIterator<Item = &'a Txid>,
    mut read: impl FnMut(&Txid) -> Result<Option<TransactionRecord>, E>,
) -> (Vec<TransactionRecord>, bool) {
    let mut clean = true;
    let records = (txids.into_iter())
        .filter_map(|t| match read(t) {
            Ok(Some(r)) if r.txid == *t && r.transaction.txid() == *t => Some(r),
            Ok(found) => {
                tracing::warn!(%wallet, txid = %t, found = ?found.map(|r| r.txid), "a listed transaction record is missing or another's");
                clean = false;
                None
            }
            Err(e) => {
                tracing::warn!(%wallet, txid = %t, error = %e, "unreadable transaction record");
                clean = false;
                None
            }
        })
        .collect();
    (records, clean)
}

pub(crate) fn persisted_txids(
    db_path: &Path,
) -> Result<HashMap<[u8; 32], Persisted>, rusqlite::Error> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut stmt = conn.prepare(
        "SELECT wallet_id, txid, record_blob IS NULL, height IS NULL FROM core_transactions",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, bool>(2)?,
            r.get::<_, bool>(3)?,
        ))
    })?;
    let mut out: HashMap<[u8; 32], Persisted> = HashMap::new();
    for row in rows {
        let (wallet, txid, recordless, unconfirmed) = row?;
        let (Ok(wallet), Ok(txid)) = (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid)) else {
            continue;
        };
        let persisted = out.entry(wallet).or_default();
        if recordless && unconfirmed {
            persisted.unconfirmed.insert(txid);
        }
        if recordless {
            persisted.recordless.insert(txid);
        } else {
            persisted.records.push(txid);
        }
    }
    type Row = (Vec<u8>, Vec<u8>, Option<Vec<u8>>);
    let dropped = || -> Result<Vec<Row>, rusqlite::Error> {
        let mut stmt = conn.prepare(
            "SELECT wallet_id, txid, \
             CASE WHEN length(record_blob) <= ?1 THEN record_blob END \
             FROM core_dropped_unsigned_spends",
        )?;
        let rows = stmt.query_map([lock_size_limit()], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?;
        rows.collect()
    };
    match dropped() {
        Ok(rows) => {
            for (wallet, txid, blob) in rows {
                let (Ok(wallet), Ok(txid)) =
                    (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid))
                else {
                    continue;
                };
                let record = blob.as_deref().and_then(decode_record);
                // The store doesn't say which input it found unsigned: one
                // input that isn't push-only makes the verdict doubtful.
                let doubtful = !record.is_some_and(|r| {
                    r.txid == txid
                        && r.transaction.txid() == txid
                        && (r.transaction.input.iter())
                            .all(|i| crate::tx_actions::push_only(&i.script_sig))
                });
                let persisted = out.entry(wallet).or_default();
                persisted.dropped.insert(txid);
                if doubtful {
                    persisted.dropped_doubtful.insert(txid);
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read the spends the store dropped as unsigned")
        }
    }
    // Verified locks of transactions without a record; one with a record
    // links through its record's inputs. A failed query fails the listing,
    // so no wallet loads complete without them. As in the store, a blob
    // over its size limit isn't read (SQLite returns NULL for it), and the
    // wallet's locks count as unread.
    let mut stmt = conn.prepare(LOCKS_WITHOUT_RECORD)?;
    let rows = stmt.query_map([lock_size_limit()], |r| {
        Ok((
            r.get::<_, Vec<u8>>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, Option<Vec<u8>>>(2)?,
        ))
    })?;
    for row in rows {
        let (wallet, txid, blob) = row?;
        let Ok(wallet) = <[u8; 32]>::try_from(wallet) else {
            continue;
        };
        let persisted = out.entry(wallet).or_default();
        let Ok(txid) = Txid::from_slice(&txid) else {
            persisted.locks_unread = true;
            continue;
        };
        match blob.as_deref().and_then(decode_lock) {
            Some(lock) if lock.txid == txid => {
                let parents = lock.inputs.iter().map(|o| o.txid).collect();
                persisted.lock_parents.insert(txid, parents);
            }
            // The store ignores a lock row for another transaction too.
            Some(lock) => {
                tracing::warn!(%txid, lock_txid = %lock.txid, "a stored lock names another transaction; ignored");
            }
            None => {
                let oversize = blob.is_none();
                tracing::warn!(%txid, oversize, "a stored InstantSend lock is unreadable");
                persisted.locks_unread = true;
            }
        }
    }
    Ok(out)
}

/// Verified lock rows with no record, their blob NULL when over `?1` bytes.
const LOCKS_WITHOUT_RECORD: &str = "SELECT l.wallet_id, l.txid, \
     CASE WHEN length(l.islock_blob) <= ?1 THEN l.islock_blob END \
     FROM core_instant_locks l WHERE l.verified = 1 AND NOT EXISTS \
     (SELECT 1 FROM core_transactions t WHERE t.wallet_id = l.wallet_id \
     AND t.txid = l.txid AND t.record_blob IS NOT NULL)";

pub(crate) fn lock_size_limit() -> i64 {
    i64::try_from(platform_wallet_storage::SIZE_LIMIT_BYTES).unwrap_or(i64::MAX)
}

/// A stored blob, decoded as the store encodes it, every byte used.
pub(crate) fn decode_stored<T: serde::de::DeserializeOwned>(blob: &[u8]) -> Option<T> {
    let config =
        bincode::config::standard().with_limit::<{ platform_wallet_storage::SIZE_LIMIT_BYTES }>();
    match bincode::serde::decode_from_slice(blob, config) {
        Ok((value, used)) if used == blob.len() => Some(value),
        _ => None,
    }
}

/// A stored record's blob, decoded as the store encodes it.
pub(crate) fn decode_record(blob: &[u8]) -> Option<TransactionRecord> {
    decode_stored(blob)
}

/// A stored lock row's blob, decoded as the store encodes it.
pub(crate) fn decode_lock(blob: &[u8]) -> Option<dashcore::InstantLock> {
    decode_stored(blob)
}

/// Of `txids`, those with an output of theirs stored for `wallet`, and
/// those with an InstantSend lock stored for them.
pub(crate) fn txids_with_outputs_and_locks(
    db_path: &Path,
    wallet: WalletId,
    txids: impl IntoIterator<Item = Txid>,
) -> Result<(HashSet<Txid>, HashSet<Txid>), rusqlite::Error> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut outputs = conn.prepare(
        "SELECT 1 FROM core_utxos WHERE wallet_id = ?1 AND substr(outpoint, 2, 32) = ?2 LIMIT 1",
    )?;
    let mut locks =
        conn.prepare("SELECT 1 FROM core_instant_locks WHERE wallet_id = ?1 AND txid = ?2")?;
    let (mut with_outputs, mut locked) = (HashSet::new(), HashSet::new());
    for txid in txids {
        let key = rusqlite::params![wallet.0.as_slice(), AsRef::<[u8]>::as_ref(&txid)];
        if outputs.exists(key)? {
            with_outputs.insert(txid);
        }
        if locks.exists(key)? {
            locked.insert(txid);
        }
    }
    Ok((with_outputs, locked))
}

/// The parts of a [`ChainView`] read from the library's state.
pub(crate) struct ViewParts {
    pub owned: HashMap<ScriptBuf, Owned>,
    pub payees: Payees,
    pub tip: u32,
    pub chainlock_height: Option<u32>,
    pub drops: crate::history::Drops,
}

impl ViewParts {
    pub(crate) fn view<'a>(
        &'a self,
        history: &'a HashMap<Txid, TxEntry>,
        network: dashcore::Network,
    ) -> ChainView<'a> {
        ChainView {
            owned: &self.owned,
            history,
            tip: self.tip,
            chainlock_height: self.chainlock_height,
            network,
            payees: &self.payees,
            drops: &self.drops,
        }
    }
}

/// The spend claims a load chooses for `h` from the payments this app sent
/// (`sent`), under the guard that installs them, and the claimants the
/// history holds locked. Spends key-wallet holds a record of this session,
/// or swept, are left to it.
fn claim_choice(
    h: &crate::history::WalletHistory,
    sent: &crate::tx_actions::Sent,
    chainlock: Option<u32>,
) -> (Vec<(OutPoint, Txid)>, HashSet<Txid>) {
    let mut claims = crate::history::unmined_own_spends(&h.txs, sent, chainlock);
    claims.retain(|(_, spender)| !h.session_held.contains(spender) && !h.swept.contains(spender));
    let locked = crate::history::locked_claimants(&h.txs, &claims);
    (claims, locked)
}

/// The last load's sent marks of `h` that the app database still holds
/// (`current`, when it read) and that were not dropped since, and the
/// payments `txid`'s lock can revive: `txid` and its unlocked descendants
/// out of a block. None when the load restored no claim or none of them
/// has a mark.
fn revived_marks(
    h: &crate::history::WalletHistory,
    txid: Txid,
    current: Option<&HashSet<Txid>>,
) -> Option<(crate::tx_actions::Sent, HashSet<Txid>)> {
    let marks = h.claim_marks.as_ref()?;
    let mut revived: HashSet<Txid> = crate::history::unmined_children(&h.txs, &[txid])
        .into_iter()
        .collect();
    revived.insert(txid);
    let kept = |t: &Txid| current.is_none_or(|c| c.contains(t)) && !h.drops.contains_key(t);
    if !revived.iter().any(|t| marks.contains_key(t) && kept(t)) {
        return None;
    }
    let sent = (marks.iter())
        .filter(|(t, _)| kept(t))
        .map(|(t, m)| (*t, m.clone()))
        .collect();
    Some((sent, revived))
}

/// Stores `lock` for `wallet` when its store holds `txid`'s record out of a
/// block, or holds none and `recordless` (the payment the history holds
/// from its sent mark alone): the history load reads that record, or the
/// mark's entry, locked. False when the store failed.
async fn store_lock(
    live: &crate::session::Live,
    wallet: WalletId,
    txid: Txid,
    lock: InstantLock,
    recordless: bool,
) -> bool {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let (persister, store) = (Arc::clone(&live.persister), Arc::clone(&live.store));
    let stored = tokio::task::spawn_blocking(move || {
        match persister.get_core_tx_record(wallet.0, &txid)? {
            Some(record) if record.block_info().is_none() => {}
            None if recordless => {}
            _ => return Ok(false),
        }
        let changeset = platform_wallet::changeset::PlatformWalletChangeSet {
            core: Some(platform_wallet::changeset::CoreChangeSet {
                instant_locks_for_non_final_records: BTreeMap::from([(txid, lock)]),
                ..Default::default()
            }),
            ..Default::default()
        };
        store.store(wallet.0, changeset)?;
        store.flush(wallet.0)?;
        Ok::<_, platform_wallet::changeset::PersistenceError>(true)
    })
    .await;
    let ok = matches!(stored, Ok(Ok(_)));
    if !ok {
        tracing::warn!(%wallet, %txid, ?stored, "could not store an InstantSend lock no event named");
    }
    ok
}

/// A wallet's stored history and the payments this app sent, read before
/// the wallet is loaded.
pub(crate) type StoredSpends = (crate::history::WalletHistory, crate::tx_actions::Sent);

/// Persisted records, first-seen times and abandoned transactions of one
/// wallet.
struct LoadedHistory {
    wallet: WalletId,
    records: Vec<TransactionRecord>,
    seen: Vec<(String, u64)>,
    abandoned: Vec<(Txid, dashcore::Transaction)>,
    /// [`crate::tx_actions::load_drop_inputs`].
    drop_inputs: BTreeMap<Txid, crate::tx_actions::OwnInputs>,
    /// [`crate::rescan_hold::load_for_open`].
    rescan_holds: BTreeMap<OutPoint, crate::rescan_hold::RescanHold>,
    /// The payments this app sent (#88).
    sent: crate::tx_actions::Sent,
    /// Every stored record was listed and read, and so were the abandoned
    /// and sent marks: the pruning of marks needs all three.
    complete: bool,
    /// As `complete`, but the sent marks' saved copies may not have read
    /// (`SentRead::CoinsOnly`): enough to restore the spend claims.
    claims: bool,
    /// Every stored transaction of the wallet, with a record or not, when
    /// every stored record was listed and read; else none.
    stored: HashSet<Txid>,
    /// Of `stored`, those that settle a sent payment: a record, or a
    /// height-only row with a height.
    settled: HashSet<Txid>,
    /// [`Persisted::lock_parents`].
    lock_parents: HashMap<Txid, Vec<Txid>>,
    /// The transactions the store dropped as unsigned claims
    /// ([`Persisted::dropped`]), those key-wallet swept as claims
    /// (`tx_actions::claims_swept`) and what the claim rule dropped
    /// ([`crate::tx_actions::load_dropped`]): their children are the claim
    /// rule's, never orphans. Not `dropped`, which seeds the claim rule's
    /// own drop at this load.
    orphan_exempt: HashSet<Txid>,
    /// [`Persisted::dropped`] (never-valid claims) and what this app
    /// dropped ([`crate::tx_actions::load_dropped`]).
    dropped: crate::tx_actions::Dead,
    /// [`crate::store_repair::store_restored`]; `None` when it didn't read.
    put_back: Option<HashSet<Txid>>,
    /// The restore holds (`restore_gate`), and whether a build waits for
    /// this load; `None` when they didn't read.
    restore_holds: Option<(std::collections::BTreeMap<Txid, BTreeSet<OutPoint>>, bool)>,
}

impl NetworkSession {
    /// Fills the history store at open: persisted records first (they
    /// include chainlocked transactions platform-wallet no longer keeps in
    /// memory), then the in-memory records, then first-seen times.
    pub(crate) async fn load_history(&self) -> Result<(), EngineError> {
        let ids: Vec<WalletId> = self
            .manager()?
            .list_wallet_ids_blocking()
            .into_iter()
            .map(WalletId)
            .collect();
        self.load_history_for(ids).await
    }

    /// [`Self::load_history`] for some wallets (a wallet just loaded).
    pub(crate) async fn load_history_for(&self, ids: Vec<WalletId>) -> Result<(), EngineError> {
        #[cfg(test)]
        let fail = (self.hub.fail_history_read).swap(false, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let fail = false;
        #[cfg(test)]
        let fail_dropped =
            (self.hub.fail_dropped_read).swap(false, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let fail_dropped = false;
        #[cfg(test)]
        let fail_restored =
            (self.hub.fail_restored_read).swap(false, std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let fail_restored = false;
        #[cfg(test)]
        let fault = self.hub.record_read_fault.lock().unwrap().take();
        #[cfg(test)]
        let unreadable = self.hub.unreadable_record.lock().unwrap().0.take();
        let live = self.live()?;
        let db_path = self.data_dir().join(WALLET_DB_FILE);
        let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
        let wallet_ids = ids.clone();
        let network = self.network.core_network();
        let loaded = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            let mut fault = fault;
            let listing = if fail {
                Err(rusqlite::Error::InvalidQuery)
            } else {
                persisted_txids(&db_path)
            };
            let (txids, listed) = match listing {
                Ok(txids) => (txids, true),
                Err(e) => {
                    tracing::warn!(error = %e, "could not list persisted transactions; history starts empty");
                    (HashMap::new(), false)
                }
            };
            wallet_ids
                .into_iter()
                .map(|wallet| {
                    let mut persisted = txids.get(&wallet.0).cloned().unwrap_or_default();
                    let store_read = persisted.add_store_dropped(&appdb, wallet);
                    let (records, read) = read_records(wallet, &persisted.records, |t| {
                        let read = persister.get_core_tx_record(wallet.0, t);
                        #[cfg(test)]
                        let read = match fault.take() {
                            Some(f) => f.apply(read),
                            None => read,
                        };
                        #[cfg(test)]
                        let read = match unreadable == Some(*t) {
                            true => RecordFault::Error.apply(read),
                            false => read,
                        };
                        read
                    });
                    let clean = listed && read;
                    let seen = appdb.tx_seen_times(&wallet.to_string()).unwrap_or_else(|e| {
                        tracing::warn!(%wallet, error = %e, "could not read first-seen times");
                        Vec::new()
                    });
                    let (abandoned, marks_read) = crate::tx_actions::load_abandoned(&appdb, wallet);
                    let drop_inputs = crate::tx_actions::load_drop_inputs(&appdb, wallet, network);
                    let (rescan_holds, holds_read) =
                        match crate::rescan_hold::load_for_open(&appdb, wallet) {
                            Ok(holds) => (holds, true),
                            Err(e) => {
                                tracing::warn!(%wallet, error = %e, "could not read the coins waiting for a rescan: every coin an abandoned payment spent waits");
                                let spent = (abandoned.copies.iter())
                                    .flat_map(|(_, tx)| tx.input.iter().map(|i| i.previous_output));
                                (spent.map(|o| (o, crate::rescan_hold::RescanHold::unread())).collect(), false)
                            }
                        };
                    let (mut sent, sent_read) = crate::tx_actions::read_sent(&appdb, wallet);
                    let (dropped_here, dropped_read) = match fail_dropped {
                        true => (crate::tx_actions::Dead::new(), false),
                        false => crate::tx_actions::load_dropped(&appdb, wallet),
                    };
                    sent.retain(|t, _| {
                        persisted.keeps_mark(t, &abandoned.unread) && !dropped_here.contains_key(t)
                    });
                    let abandoned = abandoned.copies;
                    // Unread, no load is complete: nothing is dropped.
                    let (swept_claims, claims_read) =
                        match crate::tx_actions::claims_swept(&appdb, wallet) {
                            Ok(swept) => (swept, true),
                            Err(e) => {
                                tracing::warn!(%wallet, error = %e, "could not read the claims swept");
                                (HashSet::new(), false)
                            }
                        };
                    let put_back = match fail_restored {
                        true => Err(dw_appdb::AppDbError::Corrupt("test".into())),
                        false => crate::store_repair::store_restored(&appdb, wallet),
                    };
                    let put_back = put_back
                        .inspect_err(|e| tracing::warn!(%wallet, error = %e, "could not read the claims the store's repair put back: no other spender proves a coin"))
                        .ok();
                    let restore_holds = crate::restore_gate::read_holds(&appdb, wallet)
                        .and_then(|h| Ok((h, crate::restore_gate::build_pending(&appdb, wallet)?)))
                        .inspect_err(|e| {
                            tracing::warn!(%wallet, error = %e, "could not read the restore holds")
                        })
                        .ok();
                    let stored: HashSet<Txid> = match clean {
                        true => (persisted.records.into_iter())
                            .chain(persisted.recordless)
                            .collect(),
                        false => HashSet::new(),
                    };
                    let settled = (stored.iter())
                        .filter(|t| !persisted.unconfirmed.contains(*t))
                        .copied()
                        .collect();
                    LoadedHistory {
                        wallet,
                        records,
                        seen,
                        abandoned,
                        drop_inputs,
                        rescan_holds,
                        sent,
                        complete: clean
                            && holds_read
                            && dropped_read
                            && store_read
                            && !persisted.locks_unread
                            && claims_read
                            && marks_read
                            && sent_read == crate::tx_actions::SentRead::Complete,
                        claims: clean
                            && marks_read
                            && sent_read != crate::tx_actions::SentRead::Failed,
                        stored,
                        settled,
                        lock_parents: persisted.lock_parents,
                        orphan_exempt: (persisted.dropped.iter().copied())
                            .chain(swept_claims)
                            .chain(dropped_here.keys().copied())
                            .collect(),
                        dropped: {
                            let mut dead: crate::tx_actions::Dead = (persisted.dropped.iter())
                                .filter(|t| !persisted.dropped_doubtful.contains(*t))
                                .map(|t| (*t, crate::history::DropCause::InvalidClaim))
                                .collect();
                            for (t, cause) in dropped_here {
                                dead.entry(t).or_insert(cause);
                            }
                            dead
                        },
                        put_back,
                        restore_holds,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await?;

        #[cfg(test)]
        let pause = self.hub.pause_history.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let wm = live.manager.wallet_manager_arc();
        let wm = wm.read().await;
        let mut sends = Vec::new();
        let mut buildable = Vec::new();
        for LoadedHistory {
            wallet,
            records,
            seen,
            abandoned,
            drop_inputs,
            rescan_holds,
            sent,
            complete,
            claims,
            stored,
            settled,
            lock_parents,
            orphan_exempt,
            dropped,
            put_back,
            restore_holds,
        } in loaded
        {
            let info = wm.get_wallet_info(&wallet.0);
            let in_memory: Vec<TransactionRecord> = info
                .map(|info| {
                    info.core_wallet
                        .transaction_history()
                        .into_iter()
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            // The wallet's coins whose transaction is stored (with its
            // record, or a height-only row), read under the same guard as
            // the records: a coin credited after them, or not stored yet,
            // waits for its own event ([`NetworkSession::coin_snapshot`]).
            // A load that failed to read a record, or read one missing or
            // another's, keeps every coin waiting. A contact's record comes
            // only from a store written before platform-wallet stopped
            // persisting those; a contact's ProRegTx is never left to this
            // hold alone: `upsert` keeps it out of sight for its collateral,
            // locked as an own one's.
            let loaded_coins: HashSet<Txid> = match info {
                Some(info) => (crate::coins::read_coins(&info.core_wallet).coins.iter())
                    .map(|c| c.utxo.outpoint.txid)
                    .filter(|t| stored.contains(t))
                    .collect(),
                None => HashSet::new(),
            };
            let seen: HashMap<String, u64> = seen.into_iter().collect();
            self.hub.history.with_wallet_records(wallet, |h, locks| {
                h.set_loaded_coins(loaded_coins);
                h.load_incomplete = !complete;
                h.loaded = true;
                h.put_back_unread = put_back.is_none();
                h.put_back = put_back.unwrap_or_default();
                h.lock_parents = lock_parents;
                for r in records.iter().chain(&in_memory) {
                    h.upsert_with_locks(r, None, locks);
                }
                h.session_held.extend(in_memory.iter().map(|r| r.txid));
                h.mark_abandoned(&abandoned);
                h.fill_dropped_inputs(&drop_inputs);
                for (coin, hold) in rescan_holds {
                    h.rescan_holds.entry(coin).or_insert(hold);
                }
                for (t, cause) in &dropped {
                    (h.drops).insert(*t, crate::history::DropInfo::new(*cause));
                }
                h.restore_holds_unread = restore_holds.as_ref().is_none_or(|(_, pending)| *pending);
                if let Some((holds, _)) = &restore_holds {
                    h.restore_holds = holds.clone();
                }
                if complete {
                    h.add_unrecorded_sends(&sent, &settled, self.network.core_network());
                    for txid in sent.keys() {
                        h.remember_if_buffered(txid, locks);
                    }
                }
                // Stored apart from the records, which can still say mempool.
                for t in info
                    .into_iter()
                    .flat_map(|i| i.core_wallet.instant_send_locks())
                {
                    h.set_instant_locked(t);
                }
                for (txid, entry) in h.txs.iter_mut() {
                    if let Some(t) = seen.get(&txid.to_string()) {
                        entry.first_seen = Some(*t);
                    }
                }
            });
            sends.push((
                wallet,
                sent,
                complete,
                claims,
                settled,
                (stored, orphan_exempt),
                dropped,
            ));
            if let (true, Some((_, pending))) = (complete, &restore_holds) {
                buildable.push((wallet, *pending));
            }
        }
        drop(wm);
        for (wallet, sent, complete, claims, settled, (stored, orphan_exempt), dropped) in sends {
            self.restore_unmined_spends(
                &live.manager,
                wallet,
                &sent,
                (complete, claims),
                (&settled, (&stored, &orphan_exempt)),
            )
            .await;
            // What the history holds built on a claim the store's open
            // dropped as unsigned, or on what this app dropped, is dropped
            // with it (#739).
            if complete && !dropped.is_empty() {
                let _ = self
                    .abandon_on_dead(&live.manager, wallet, &dropped, true)
                    .await;
            }
            // The excluded coins (`crate::withhold`) read the history: the
            // balance read before it loaded counts them.
            self.reread_balances(wallet).await;
        }
        let restored = self.hub.restore.is_restored();
        for wallet in ids {
            let build =
                (buildable.iter()).any(|(w, pending)| *w == wallet && (restored || *pending));
            if build {
                let (hub, appdb) = (Arc::clone(&self.hub), Arc::clone(&live.appdb));
                tokio::task::spawn_blocking(move || hub.build_restore_holds(&appdb, wallet, true))
                    .await?;
            } else {
                self.hub.refresh_restore_held(wallet);
            }
        }
        self.hub.restore.wake.notify_one();
        Ok(())
    }

    /// key-wallet rebuilds no records at load, so it no longer knows which
    /// of its coins the payments this app sent and that are still out of a
    /// block spent: a scan reaching such a coin's funding block would
    /// credit it again beside the change the store gave back (#88). Each
    /// one is restored as a spent-output claim held by its payment. An
    /// abandon or a competing spend that beats the payment settles the
    /// claim. A block or InstantSend lock beats an unlocked payment; only a
    /// ChainLock-covered block beats an InstantSend-locked payment.
    /// Spends key-wallet holds a record of this session are left to it.
    /// Then the marks of payments gone from the history go, and of those
    /// the SPV header chain proves final: it holds their block, at or below the
    /// block of this wallet's own applied ChainLock, which it holds too. A
    /// load that did not read every stored record and mark does neither,
    /// except that one whose sent marks read without their saved copies
    /// still restores the claims. A complete load then drops what can never
    /// confirm (`NetworkSession::drop_dead`), with `all_stored`, every
    /// stored transaction, and `orphan_exempt` ([`LoadedHistory`]), for the
    /// orphans.
    async fn restore_unmined_spends(
        &self,
        manager: &Manager,
        wallet: WalletId,
        sent: &crate::tx_actions::Sent,
        (complete, claims): (bool, bool),
        (stored, (all_stored, orphan_exempt)): (&HashSet<Txid>, (&HashSet<Txid>, &HashSet<Txid>)),
    ) {
        if !claims {
            tracing::warn!(%wallet, "history load incomplete: no spend claim restored");
            self.hub
                .history
                .with_wallet(wallet, |h| h.claim_marks = None);
            self.apply_spend_claims(manager, wallet, |_, _| (Vec::new(), HashSet::new()))
                .await;
            self.refresh_held(manager, wallet).await;
            return;
        }
        let (mut candidates, mut own) = (Vec::new(), None);
        self.apply_spend_claims(manager, wallet, |core, chainlock| {
            self.hub.history.with_wallet(wallet, |h| {
                for t in core.instant_send_locks() {
                    h.set_instant_locked(t);
                }
                let choice = claim_choice(h, sent, chainlock);
                own = (core.metadata.last_applied_chain_lock.as_ref())
                    .map(|c| (c.block_height, c.block_hash));
                candidates =
                    crate::history::settled_candidates(&h.txs, sent, stored, own.map(|c| c.0));
                h.claim_marks = Some(sent.clone());
                choice
            })
        })
        .await;
        if !complete {
            tracing::warn!(%wallet, "history load incomplete: no sent mark settled");
            self.refresh_held(manager, wallet).await;
            return;
        }
        self.drop_dead(manager, wallet, Some((all_stored, orphan_exempt)))
            .await;
        let settled = self.proven_settled(candidates, own).await;
        if !settled.is_empty() {
            let gone = settled.clone();
            let forgotten = self
                .appdb_op(move |db| crate::tx_actions::forget_sent(db, wallet, &settled))
                .await;
            match forgotten {
                Ok(()) => self.hub.history.drop_claims(&wallet, gone),
                Err(e) => {
                    tracing::warn!(%wallet, error = %e, "could not drop the marks of settled payments")
                }
            }
        }
    }

    /// Hands the hub what follows a lock that wallets learned of without an
    /// event of theirs ([`Self::rerun_locked_claims`]).
    pub(crate) fn attach_lock_rerun(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        #[cfg(test)]
        let hub = Arc::downgrade(&self.hub);
        self.hub
            .attach_lock_rerun(Box::new(move |txid, lock, wallets| {
                let task = weak.clone();
                let run = rt.spawn(async move {
                    if let Some(session) = task.upgrade() {
                        session.rerun_locked_claims(txid, lock, wallets).await;
                    }
                });
                #[cfg(test)]
                if let Some(hub) = hub.upgrade() {
                    hub.lock_reruns
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .push(run);
                }
                #[cfg(not(test))]
                drop(run);
            }));
    }

    /// `txid`'s InstantSend lock reached `wallets`' histories with no event
    /// of theirs (#828): each chooses again, under the guard that installs
    /// them, the spend claims of the payments the lock can revive (`txid`
    /// and its unlocked descendants out of a block) as its last load would
    /// have with the lock known, from that load's sent marks still held and
    /// not dropped since, and stores the lock for the next load. A wallet whose
    /// last load restored no claim chooses none. A lock whose store failed
    /// runs again at the next lock heard while the history still shows it;
    /// the session's end or the wallet's unload drops it.
    async fn rerun_locked_claims(&self, txid: Txid, lock: InstantLock, wallets: Vec<WalletId>) {
        let Ok(live) = self.live() else {
            return;
        };
        let history = &self.hub.history;
        let mut runs = self.hub.take_unstored_locks();
        runs.retain(|(wallet, txid), _| {
            history.read_wallet(wallet, |h| {
                h.loaded
                    && (h.txs.get(txid)).is_some_and(|e| {
                        e.context.block_info().is_none() && crate::history::instant_locked(e)
                    })
            })
        });
        runs.extend(wallets.into_iter().map(|w| ((w, txid), lock.clone())));
        for ((wallet, txid), lock) in runs {
            let current = self
                .appdb_op(move |db| Ok(crate::tx_actions::read_sent(db, wallet)))
                .await;
            let current: Option<HashSet<Txid>> = match current {
                Ok((sent, read)) if read != crate::tx_actions::SentRead::Failed => {
                    Some(sent.into_keys().collect())
                }
                _ => {
                    tracing::warn!(%wallet, %txid, "sent marks unread: the load's are used after a lock");
                    None
                }
            };
            let current = current.as_ref();
            let mut claimants = BTreeSet::new();
            if history.read_wallet(&wallet, |h| revived_marks(h, txid, current).is_some()) {
                claimants = self
                    .apply_spend_claims(&live.manager, wallet, |_, chainlock| {
                        history.with_wallet(wallet, |h| match revived_marks(h, txid, current) {
                            Some((sent, revived)) => {
                                let (mut claims, locked) = claim_choice(h, &sent, chainlock);
                                claims.retain(|(_, spender)| revived.contains(spender));
                                (claims, locked)
                            }
                            None => (Vec::new(), HashSet::new()),
                        })
                    })
                    .await;
            }
            // With no record, the lock is stored only for a payment it
            // gave a claim to, one that isn't abandoned (#832).
            let recordless = claimants.contains(&txid)
                && history.read_wallet(&wallet, |h| {
                    (h.txs.get(&txid)).is_some_and(|e| !e.abandoned)
                });
            if !store_lock(&live, wallet, txid, lock.clone(), recordless).await {
                self.hub.note_unstored_lock(wallet, txid, lock);
            }
        }
    }

    /// [`crate::history::proven_settled`] against the SPV header chain,
    /// read after the claims' guard: the lock's header first, and the
    /// candidates' only when it holds the lock's block.
    async fn proven_settled(
        &self,
        candidates: Vec<(Txid, Option<(u32, dashcore::BlockHash)>)>,
        chainlock: Option<(u32, dashcore::BlockHash)>,
    ) -> Vec<Txid> {
        let heights: BTreeSet<u32> = (candidates.iter())
            .filter_map(|(_, b)| b.map(|b| b.0))
            .collect();
        let headers = match chainlock {
            Some(cl) if !heights.is_empty() => self.headers_under(cl, heights).await,
            _ => None,
        };
        crate::history::proven_settled(candidates, chainlock, &headers.unwrap_or_default())
    }

    /// The SPV header chain's hashes at `heights` and at the ChainLock's,
    /// read after any guard, if the chain holds the ChainLock's block
    /// before and after (a Rebuild Index keeps the lock but not the
    /// headers): what [`crate::history::proven_final`] reads.
    pub(crate) async fn headers_under(
        &self,
        (height, hash): (u32, dashcore::BlockHash),
        heights: BTreeSet<u32>,
    ) -> Option<HashMap<u32, dashcore::BlockHash>> {
        let holds = || async { self.hub.header_hash(height).await == Some(hash) };
        if !holds().await {
            return None;
        }
        let mut headers = self.hub.header_hashes(heights).await;
        if !holds().await {
            return None;
        }
        headers.insert(height, hash);
        Some(headers)
    }

    /// Restores the claims `select` chooses, given the wallet and the best
    /// ChainLock height any loaded wallet applied or SPV reported (one
    /// applied while this wallet was closed counts), under the write lock
    /// that installs them, as key-wallet spent-output claims of `wallet`. A
    /// claimed coin the wallet still holds (a block processed for a wallet
    /// opened later before its claims were set, or a store written while
    /// the claim was not restored) is spent by a live payment this app
    /// sent: it leaves the funds accounts, is stored as spent and the
    /// balance is announced. The coins dead transactions' drops hold for
    /// good, as the wallet's open read them (`tx_actions::dropped_holds`),
    /// are claimed with no claimant, as the drops left them. Returns the
    /// payments given a claim.
    pub(crate) async fn apply_spend_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        select: impl FnOnce(
            &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
            Option<u32>,
        ) -> (Vec<(OutPoint, Txid)>, HashSet<Txid>),
    ) -> BTreeSet<Txid> {
        let Ok(live) = self.live() else {
            return BTreeSet::new();
        };
        let holds = self.hub.dropped_holds(wallet);
        #[cfg(test)]
        let pause = self.hub.pause_spend_claims.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let chainlock = (wm.get_all_wallet_infos().values())
            .filter_map(applied_chainlock)
            .max()
            .max(self.hub.tracker().chainlock_height())
            .max(self.hub.best_chainlock().map(|(h, _)| h));
        let Some(info) = wm.get_wallet_info_mut(&wallet.0) else {
            return BTreeSet::new();
        };
        let core = &mut info.core_wallet;
        let (claims, locked) = select(core, chainlock);
        let claims = claims.as_slice();
        if claims.is_empty() && holds.is_empty() {
            return BTreeSet::new();
        }
        (self.hub.history).with_wallet(wallet, |h| {
            h.claimed_at_load.extend(claims.iter().map(|c| c.1))
        });
        // A block this process saw spend a claimed coin mined the payment or
        // beat it: either way it claims nothing. Only a block the ChainLock
        // covers beats an InstantSend-locked one.
        let observed = core.observed_spent_outpoints();
        let beaten: BTreeSet<Txid> = (claims.iter())
            .filter(|(o, t)| {
                observed
                    .get(o)
                    .is_some_and(|h| !locked.contains(t) || chainlock.is_some_and(|cl| cl >= *h))
            })
            .map(|(_, t)| *t)
            .collect();
        let restore: Vec<(OutPoint, Option<Txid>)> = (claims.iter())
            .filter(|(_, t)| !beaten.contains(t))
            .map(|(o, t)| (*o, Some(*t)))
            .chain(holds.iter().map(|o| (*o, None)))
            .collect();
        let restored: BTreeSet<Txid> = restore.iter().filter_map(|(_, t)| *t).collect();
        let held = restore_spent_claims(info, &restore);
        let core = &mut info.core_wallet;
        (self.hub.history).with_wallet(wallet, |h| {
            h.claimed
                .extend(restore.iter().filter_map(|(o, t)| Some((*o, (*t)?))))
        });
        if held.is_empty() {
            return restored;
        }
        let mut removed: Vec<key_wallet::Utxo> = Vec::new();
        for account in core.accounts.all_funding_accounts_mut() {
            removed.extend(held.iter().filter_map(|o| account.utxos.remove(o)));
        }
        core.update_balance();
        self.hub.publish_core_balance(wallet, core);
        for utxo in &removed {
            let spenders: Vec<Txid> = (claims.iter())
                .filter(|(o, _)| *o == utxo.outpoint)
                .map(|(_, t)| *t)
                .collect();
            tracing::warn!(%wallet, outpoint = %utxo.outpoint, value = utxo.value(), ?spenders,
                "dropped a coin an unmined own transaction spends");
        }
        if removed.is_empty() {
            return restored;
        }
        // Stored under the lock that dropped them, so no write of a later
        // change to the same coins lands before this one.
        let persister = Arc::clone(&live.persister);
        let stored = tokio::task::spawn_blocking(move || {
            let changeset = platform_wallet::changeset::PlatformWalletChangeSet {
                core: Some(platform_wallet::changeset::CoreChangeSet {
                    spent_utxos: removed,
                    ..Default::default()
                }),
                ..Default::default()
            };
            persister.store(wallet.0, changeset)?;
            persister.flush(wallet.0)
        })
        .await;
        drop(wm);
        if !matches!(stored, Ok(Ok(()))) {
            tracing::warn!(%wallet, ?stored, "could not store the dropped coins as spent");
        }
        restored
    }

    /// The stored history of `wallet` and the payments this app sent, read
    /// from wallet.sqlite before the wallet is loaded, so its spend claims
    /// can be restored before a block is processed for it
    /// ([`Self::apply_stored_spends`]). No payment when a stored record or
    /// mark could not be read, so none claims.
    pub(crate) async fn stored_spends(
        &self,
        wallet: WalletId,
    ) -> Result<StoredSpends, EngineError> {
        let live = self.live()?;
        let db_path = self.data_dir().join(WALLET_DB_FILE);
        let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
        #[cfg(test)]
        let unreadable = self.hub.unreadable_record.lock().unwrap().1.take();
        let network = self.network.core_network();
        let stored = tokio::task::spawn_blocking(move || {
            let mut history = crate::history::WalletHistory::default();
            let mut persisted = match persisted_txids(&db_path) {
                Ok(mut all) => all.remove(&wallet.0).unwrap_or_default(),
                Err(e) => {
                    tracing::warn!(%wallet, error = %e, "could not list stored transactions for spend claims");
                    return (history, crate::tx_actions::Sent::new());
                }
            };
            // Read and marked abandoned as the history load does.
            let (records, read) = read_records(wallet, &persisted.records, |t| {
                let read = persister.get_core_tx_record(wallet.0, t);
                #[cfg(test)]
                let read = match unreadable == Some(*t) {
                    true => RecordFault::Error.apply(read),
                    false => read,
                };
                read
            });
            for r in &records {
                history.upsert(r, None);
            }
            let (abandoned, marks_read) = crate::tx_actions::load_abandoned(&appdb, wallet);
            history.mark_abandoned(&abandoned.copies);
            let (mut sent, sent_read) = crate::tx_actions::read_sent(&appdb, wallet);
            let (dropped_here, dropped_read) = crate::tx_actions::load_dropped(&appdb, wallet);
            let store_read = persisted.add_store_dropped(&appdb, wallet);
            sent.retain(|t, _| {
                persisted.keeps_mark(t, &abandoned.unread) && !dropped_here.contains_key(t)
            });
            if !(read && marks_read && dropped_read && store_read && sent_read != crate::tx_actions::SentRead::Failed) {
                tracing::warn!(%wallet, "stored spends incomplete: none claimed before the history load");
                return (history, crate::tx_actions::Sent::new());
            }
            let stored: HashSet<Txid> = (persisted.records.iter())
                .chain(&persisted.recordless)
                .filter(|t| !persisted.unconfirmed.contains(*t))
                .copied()
                .collect();
            history.add_unrecorded_sends(&sent, &stored, network);
            (history, sent)
        })
        .await?;
        Ok(stored)
    }

    /// Restores the spend claims of `stored` once `wallet` is registered,
    /// with the InstantSend locks its store holds apart from the records.
    pub(crate) async fn apply_stored_spends(
        &self,
        manager: &Manager,
        wallet: WalletId,
        (mut history, sent): StoredSpends,
    ) {
        self.apply_spend_claims(manager, wallet, |core, chainlock| {
            for t in core.instant_send_locks() {
                history.set_instant_locked(t);
            }
            let claims = crate::history::unmined_own_spends(&history.txs, &sent, chainlock);
            let locked = crate::history::locked_claimants(&history.txs, &claims);
            (claims, locked)
        })
        .await;
    }

    /// Transaction and address labels of a wallet (dw-appdb).
    pub(crate) async fn labels(
        appdb: &Arc<AppDb>,
        wallet: WalletId,
    ) -> Result<Labels, EngineError> {
        let appdb = Arc::clone(appdb);
        tokio::task::spawn_blocking(move || {
            let w = wallet.to_string();
            Ok::<_, dw_appdb::AppDbError>(Labels {
                tx: appdb.labels(&w, LabelKind::Tx)?.into_iter().collect(),
                address: appdb.labels(&w, LabelKind::Address)?.into_iter().collect(),
                fiat: appdb
                    .tx_fiat(&w)?
                    .into_iter()
                    .filter_map(|s| {
                        let source = crate::rates::RateSourceId::from_stored(&s.source)?;
                        Some((
                            s.txid,
                            crate::rates::TxFiat {
                                currency: s.currency,
                                price: s.price,
                                rate_at: s.rate_at,
                                source,
                            },
                        ))
                    })
                    .collect(),
                purpose: username_purposes(&appdb.registration_rows(&w)?),
                coinjoin_sends: appdb
                    .settings_with_prefix(&w, crate::send::COINJOIN_SEND_PREFIX)?
                    .into_iter()
                    .filter_map(|(k, _)| {
                        k.strip_prefix(crate::send::COINJOIN_SEND_PREFIX)
                            .map(str::to_owned)
                    })
                    .collect(),
            })
        })
        .await?
        .map_err(|e| EngineError::Storage(e.to_string()))
    }

    /// What a chain view needs from the library's state, read under its
    /// lock: the owned scripts, the DashPay payees, the tip and the best
    /// ChainLock height.
    pub(crate) fn view_parts(&self, info: &PlatformWalletInfo, wallet: &WalletId) -> ViewParts {
        let meta = &info.core_wallet.metadata;
        let owned = owned_scripts(info);
        let drops = self.drop_states(info, wallet, &owned);
        ViewParts {
            owned,
            payees: self.payees(info, wallet),
            tip: meta
                .last_processed_height
                .max(meta.synced_height)
                .max(self.hub.wallet_state(wallet).map_or(0, |s| s.tip())),
            chainlock_height: self.chainlock_height(&info.core_wallet),
            drops,
        }
    }

    /// The state of each transaction of `wallet` this app dropped
    /// ([`crate::history::drop_states`]), read under the manager's lock
    /// `info` is held under. Empty, and no read of the history, when the
    /// wallet dropped nothing.
    fn drop_states(
        &self,
        info: &PlatformWalletInfo,
        wallet: &WalletId,
        owned: &HashMap<ScriptBuf, Owned>,
    ) -> crate::history::Drops {
        if (self.hub.history).read_wallet(wallet, |h| h.drops.is_empty()) {
            return crate::history::Drops::default();
        }
        let core = &info.core_wallet;
        let funding = core.accounts.all_funding_accounts();
        // A coin given back that waits for its rescan is not back yet.
        let held = self.hub.rescan_held(wallet);
        let unspent =
            |o: &OutPoint| !held.contains(o) && funding.iter().any(|a| a.utxos.contains_key(o));
        let spent = core.observed_spent_outpoints();
        let block_spent = |o: &OutPoint| spent.contains_key(o);
        let state = self.hub.wallet_state(wallet);
        let (scan_at, pending) =
            crate::history::scan_facts(state.as_ref(), self.hub.abandon_batch_pending(wallet));
        let facts = crate::history::DropFacts {
            unspent: &unspent,
            block_spent: &block_spent,
            scan_at,
            pending,
            birth: core.birth_height(),
        };
        (self.hub.history).read_wallet(wallet, |h| crate::history::Drops {
            states: crate::history::drop_states(&h.txs, &h.drops, owned, &facts),
            inputs: h.drop_inputs.clone(),
        })
    }

    /// Height of the best ChainLock the wallet applied or SPV reported.
    pub(crate) fn chainlock_height(
        &self,
        core: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    ) -> Option<u32> {
        (core.metadata.last_applied_chain_lock.as_ref())
            .map(|c| c.block_height)
            .max(self.hub.tracker().chainlock_height())
    }

    /// Runs `f` with the wallet's chain view, over the history store's own
    /// entries under its read lock, with no copy (H-06). `f` must be a pure
    /// function of the view: no lock, task or store a history writer may
    /// wait on. `WalletNotFound` for an unknown wallet.
    pub(crate) async fn with_view<R>(
        &self,
        manager: &Manager,
        wallet: WalletId,
        f: impl FnOnce(&ChainView<'_>) -> R,
    ) -> Result<R, EngineError> {
        self.view_of(manager, wallet, false, f).await
    }

    /// [`Self::with_view`] for a read that shows payees: their names are
    /// fetched first, outside the history store's lock.
    pub(crate) async fn with_view_naming_payees<R>(
        &self,
        manager: &Manager,
        wallet: WalletId,
        f: impl FnOnce(&ChainView<'_>) -> R,
    ) -> Result<R, EngineError> {
        self.view_of(manager, wallet, true, f).await
    }

    async fn view_of<R>(
        &self,
        manager: &Manager,
        wallet: WalletId,
        name_payees: bool,
        f: impl FnOnce(&ChainView<'_>) -> R,
    ) -> Result<R, EngineError> {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm
            .get_wallet_info(&wallet.0)
            .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
        let parts = self.view_parts(info, &wallet);
        drop(wm);
        if name_payees {
            self.fetch_payee_names(wallet, &parts.payees);
        }
        let network = self.network.core_network();
        Ok(self
            .hub
            .history
            .read(&wallet, |history| f(&parts.view(history, network))))
    }

    /// One page of dash-qt history records (QT-086…089, IOS-027/028).
    pub async fn history_page(
        self: &Arc<Self>,
        wallet: WalletId,
        query: HistoryQuery,
    ) -> Result<HistoryPage, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            validate_query(&query)?;
            let records = this.wallet_records(wallet).await?;
            page(records, &query)
        })
        .await
    }

    /// Every dash-qt record of a wallet, unsorted. Records of a watch-only
    /// wallet are marked `involves_watch_only` (m2-engine.md §5). Caller
    /// holds the operation guard.
    pub(crate) async fn wallet_records(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<crate::TxRecord>, EngineError> {
        let live = self.live()?;
        let labels = Self::labels(&live.appdb, wallet).await?;
        let watch_only = !self.vault.has_wallet_secret(&wallet.0);
        let mut records = self
            .with_view_naming_payees(&live.manager, wallet, |view| {
                view.history
                    .iter()
                    .flat_map(|(txid, entry)| records_of(txid, entry, view, &labels).0)
                    .collect::<Vec<_>>()
            })
            .await?;
        if watch_only {
            for r in &mut records {
                r.involves_watch_only = true;
            }
        }
        Ok(records)
    }

    /// The transaction details dialog (QT-092, IOS-031). `txid` is display
    /// hex.
    pub async fn tx_detail(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: String,
    ) -> Result<TxDetail, EngineError> {
        let parsed = parse_txid(&txid)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let labels = Self::labels(&live.appdb, wallet).await?;
            let appdb = Arc::clone(&live.appdb);
            let message = tokio::task::spawn_blocking(move || {
                appdb.tx_message(&wallet.to_string(), &parsed.to_string())
            })
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?;
            let watch_only = !this.vault.has_wallet_secret(&wallet.0);
            let mut detail = this
                .with_view_naming_payees(&live.manager, wallet, |view| {
                    view.history
                        .get(&parsed)
                        .map(|entry| detail_of(&parsed, entry, view, &labels, message))
                })
                .await?
                .ok_or(EngineError::TxNotFound(txid))?;
            for r in &mut detail.records {
                r.involves_watch_only = watch_only;
            }
            Ok(detail)
        })
        .await
    }
}

/// Parses a display-order txid: 64 lower-case hex characters.
pub(crate) fn parse_txid(s: &str) -> Result<Txid, EngineError> {
    if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(EngineError::InvalidArgument(
            "txid must be 64 lower-case hex characters".into(),
        ));
    }
    Txid::from_str(s).map_err(|e| EngineError::InvalidArgument(format!("txid: {e}")))
}

#[cfg(test)]
mod lock_row_tests {
    use super::*;

    fn encode(lock: &dashcore::InstantLock) -> Vec<u8> {
        let config = bincode::config::standard()
            .with_limit::<{ platform_wallet_storage::SIZE_LIMIT_BYTES }>();
        bincode::serde::encode_to_vec(lock, config).unwrap()
    }

    /// A verified lock row with no record links its transaction to the
    /// lock's inputs; one that doesn't decode, is over the store's size
    /// limit or has a malformed txid leaves its wallet's locks unread; one
    /// with a record, an unverified one, and one naming another transaction
    /// are skipped.
    #[test]
    fn lock_rows_without_a_record_link_their_inputs_or_leave_locks_unread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_DB_FILE);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE core_transactions (wallet_id BLOB, txid BLOB, record_blob BLOB, height INTEGER);
             CREATE TABLE core_instant_locks (wallet_id BLOB, txid BLOB, islock_blob BLOB, verified INTEGER);
             CREATE TABLE core_dropped_unsigned_spends (wallet_id BLOB, txid BLOB);",
        )
        .unwrap();
        let txid = |n: u8| Txid::from_byte_array([n; 32]);
        let lock = |t: Txid, parent: Txid| dashcore::InstantLock {
            txid: t,
            inputs: vec![OutPoint::new(parent, 0)],
            ..Default::default()
        };
        let add_lock = |wallet: u8, t: Txid, blob: Vec<u8>, verified: bool| {
            conn.execute(
                "INSERT INTO core_instant_locks VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![
                    [wallet; 32].as_slice(),
                    t.as_byte_array().as_slice(),
                    blob,
                    verified
                ],
            )
            .unwrap();
        };
        add_lock(1, txid(10), encode(&lock(txid(10), txid(11))), true);
        add_lock(1, txid(12), encode(&lock(txid(12), txid(13))), true);
        conn.execute(
            "INSERT INTO core_transactions VALUES (?1, ?2, x'00', 5)",
            rusqlite::params![[1u8; 32].as_slice(), txid(12).as_byte_array().as_slice()],
        )
        .unwrap();
        add_lock(1, txid(14), encode(&lock(txid(14), txid(15))), false);
        add_lock(1, txid(16), encode(&lock(txid(17), txid(18))), true);
        add_lock(2, txid(20), vec![0xff; 8], true);
        let mut oversize = encode(&lock(txid(30), txid(31)));
        oversize.resize(platform_wallet_storage::SIZE_LIMIT_BYTES + 1, 0);
        add_lock(3, txid(30), oversize, true);
        conn.execute(
            "INSERT INTO core_instant_locks VALUES (?1, x'0102', ?2, 1)",
            rusqlite::params![[4u8; 32].as_slice(), encode(&lock(txid(40), txid(41)))],
        )
        .unwrap();
        drop(conn);

        let conn = rusqlite::Connection::open(&path).unwrap();
        let blob_of = |wallet: u8| -> Option<Vec<u8>> {
            let mut stmt = conn.prepare(LOCKS_WITHOUT_RECORD).unwrap();
            let mut rows = stmt.query([lock_size_limit()]).unwrap();
            while let Some(r) = rows.next().unwrap() {
                if r.get::<_, Vec<u8>>(0).unwrap() == [wallet; 32] {
                    return r.get(2).unwrap();
                }
            }
            panic!("no row of wallet {wallet}")
        };
        assert!(blob_of(3).is_none(), "an oversized blob isn't returned");
        assert!(blob_of(2).is_some());
        drop(conn);

        let out = persisted_txids(&path).unwrap();
        let one = &out[&[1u8; 32]];
        assert_eq!(
            one.lock_parents,
            HashMap::from([(txid(10), vec![txid(11)])])
        );
        assert!(!one.locks_unread);
        for wallet in [2u8, 3, 4] {
            assert!(out[&[wallet; 32]].lock_parents.is_empty());
            assert!(out[&[wallet; 32]].locks_unread, "wallet {wallet}");
        }
    }
}

#[cfg(test)]
mod claim_restore_tests {
    use std::path::{Path, PathBuf};

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// No engine code restores spent claims on key-wallet's wallet
    /// directly: every caller goes through [`super::restore_spent_claims`].
    #[test]
    fn only_the_helper_restores_spent_claims() {
        let call = concat!("restore_spent", "_outpoints(");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let helper = root.join("src").join("history_ops.rs");
        let mut files = Vec::new();
        for dir in ["src", "tests", "examples"] {
            if root.join(dir).is_dir() {
                sources(&root.join(dir), &mut files);
            }
        }
        let (mut found, mut exempt) = (Vec::new(), 0);
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if !line.contains(call) {
                    continue;
                }
                if *file == helper
                    && line.trim()
                        == concat!("info.core_wallet.restore_spent", "_outpoints(claims)")
                {
                    exempt += 1;
                } else {
                    found.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                }
            }
        }
        assert_eq!(exempt, 1, "the helper's own call");
        assert!(
            found.is_empty(),
            "spent claims restored past the helper: {found:#?}"
        );
    }
}
