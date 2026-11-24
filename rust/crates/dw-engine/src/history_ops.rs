//! Session operations over the history read model: loading persisted
//! history at open, `history_page` and `tx_detail`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dashcore::hashes::Hash;
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
            for info in pool.addresses.values() {
                map.insert(
                    info.script_pubkey.clone(),
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
    /// owner (`core_dropped_unsigned_spends`): never restored from a mark.
    pub(crate) dropped: HashSet<Txid>,
}

impl Persisted {
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
    type Row = (Vec<u8>, Vec<u8>);
    let dropped = || -> Result<Vec<Row>, rusqlite::Error> {
        let mut stmt = conn.prepare("SELECT wallet_id, txid FROM core_dropped_unsigned_spends")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    };
    match dropped() {
        Ok(rows) => {
            for (wallet, txid) in rows {
                let (Ok(wallet), Ok(txid)) =
                    (<[u8; 32]>::try_from(wallet), Txid::from_slice(&txid))
                else {
                    continue;
                };
                out.entry(wallet).or_default().dropped.insert(txid);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read the spends the store dropped as unsigned")
        }
    }
    Ok(out)
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
        }
    }
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
        let fault = self.hub.record_read_fault.lock().unwrap().take();
        #[cfg(test)]
        let unreadable = self.hub.unreadable_record.lock().unwrap().0.take();
        let live = self.live()?;
        let db_path = self.data_dir().join(WALLET_DB_FILE);
        let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
        let wallet_ids = ids.clone();
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
                    let persisted = txids.get(&wallet.0).cloned().unwrap_or_default();
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
                    let (mut sent, sent_read) = crate::tx_actions::read_sent(&appdb, wallet);
                    sent.retain(|t, _| persisted.keeps_mark(t, &abandoned.unread));
                    let abandoned = abandoned.copies;
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
                        sent,
                        complete: clean
                            && marks_read
                            && sent_read == crate::tx_actions::SentRead::Complete,
                        claims: clean
                            && marks_read
                            && sent_read != crate::tx_actions::SentRead::Failed,
                        stored,
                        settled,
                    }
                })
                .collect::<Vec<_>>()
        })
        .await?;
        for wallet in &ids {
            if let Err(e) = self.check_block_spends(&live.manager, *wallet).await {
                tracing::warn!(%wallet, error = %e, "could not check the wallet's block spends at load");
            }
        }

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
        for LoadedHistory {
            wallet,
            records,
            seen,
            abandoned,
            sent,
            complete,
            claims,
            stored,
            settled,
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
            self.hub.history.with_wallet(wallet, |h| {
                h.set_loaded_coins(loaded_coins);
                h.load_incomplete = !complete;
                for r in records.iter().chain(&in_memory) {
                    h.upsert(r, None);
                }
                h.session_held.extend(in_memory.iter().map(|r| r.txid));
                h.mark_abandoned(&abandoned);
                if complete {
                    h.add_unrecorded_sends(&sent, &settled, self.network.core_network());
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
            sends.push((wallet, sent, complete, claims, settled));
        }
        drop(wm);
        for (wallet, sent, complete, claims, stored) in sends {
            self.restore_unmined_spends(&live.manager, wallet, &sent, (complete, claims), &stored)
                .await;
            if let Err(e) = (self.start_block_spend_check(&live.manager, wallet, complete)).await {
                tracing::warn!(%wallet, error = %e, "could not start the block-spend check");
            }
        }
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
    /// still restores the claims.
    async fn restore_unmined_spends(
        &self,
        manager: &Manager,
        wallet: WalletId,
        sent: &crate::tx_actions::Sent,
        (complete, claims): (bool, bool),
        stored: &HashSet<Txid>,
    ) {
        if !claims {
            tracing::warn!(%wallet, "history load incomplete: no spend claim restored");
            self.hold_block_spends(manager, wallet).await;
            return;
        }
        let (mut candidates, mut own) = (Vec::new(), None);
        self.apply_spend_claims(manager, wallet, |core, chainlock| {
            self.hub.history.with_wallet(wallet, |h| {
                for t in core.instant_send_locks() {
                    h.set_instant_locked(t);
                }
                own = (core.metadata.last_applied_chain_lock.as_ref())
                    .map(|c| (c.block_height, c.block_hash));
                candidates =
                    crate::history::settled_candidates(&h.txs, sent, stored, own.map(|c| c.0));
                let mut claims = crate::history::unmined_own_spends(&h.txs, sent, chainlock);
                claims.retain(|(_, spender)| {
                    !h.session_held.contains(spender) && !h.swept.contains(spender)
                });
                let locked = crate::history::locked_claimants(&h.txs, &claims);
                (claims, locked)
            })
        })
        .await;
        if !complete {
            tracing::warn!(%wallet, "history load incomplete: no sent mark settled");
            return;
        }
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
    /// balance is announced.
    pub(crate) async fn apply_spend_claims(
        &self,
        manager: &Manager,
        wallet: WalletId,
        select: impl FnOnce(
            &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
            Option<u32>,
        ) -> (Vec<(OutPoint, Txid)>, HashSet<Txid>),
    ) {
        let Ok(live) = self.live() else {
            return;
        };
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
            return;
        };
        let (claims, locked) = select(&info.core_wallet, chainlock);
        let claims = claims.as_slice();
        let mut removed = self.hold_block_spends_locked(wallet, info);
        let block_spent = self.hub.block_spends.rows(&wallet);
        let core = &mut info.core_wallet;
        (self.hub.history).with_wallet(wallet, |h| {
            h.claimed_at_load.extend(claims.iter().map(|c| c.1))
        });
        // A block this process saw spend a claimed coin mined the payment or
        // beat it: either way it claims nothing. Only a block the ChainLock
        // covers beats an InstantSend-locked one.
        // A stored block-spend row is the same evidence, kept past the
        // ChainLock's prune of the observed note.
        let observed = core.observed_spent_outpoints();
        let beaten: BTreeSet<Txid> = (claims.iter())
            .filter(|(o, t)| {
                (observed.get(o).copied())
                    .or_else(|| block_spent.get(o).map(|s| s.height))
                    .is_some_and(|h| !locked.contains(t) || chainlock.is_some_and(|cl| cl >= h))
            })
            .map(|(_, t)| *t)
            .collect();
        let restore: Vec<(OutPoint, Option<Txid>)> = (claims.iter())
            .filter(|(_, t)| !beaten.contains(t))
            .map(|(o, t)| (*o, Some(*t)))
            .collect();
        let held = match restore.is_empty() {
            true => Vec::new(),
            false => crate::block_spends::restore_spent_claims(info, &restore),
        };
        let core = &mut info.core_wallet;
        (self.hub.history).with_wallet(wallet, |h| {
            h.claimed
                .extend(restore.iter().filter_map(|(o, t)| Some((*o, (*t)?))))
        });
        let claimed_from = removed.len();
        for account in core.accounts.all_funding_accounts_mut() {
            removed.extend(held.iter().filter_map(|o| account.utxos.remove(o)));
        }
        if removed.is_empty() {
            return;
        }
        core.update_balance();
        self.hub
            .publish_core_balance(wallet, &core.balance(), &core.account_balances());
        for utxo in &removed[claimed_from..] {
            let spenders: Vec<Txid> = (claims.iter())
                .filter(|(o, _)| *o == utxo.outpoint)
                .map(|(_, t)| *t)
                .collect();
            tracing::warn!(%wallet, outpoint = %utxo.outpoint, value = utxo.value(), ?spenders,
                "dropped a coin an unmined own transaction spends");
        }
        // Stored under the lock that dropped them, so no write of a later
        // change to the same coins lands before this one.
        let persister = Arc::clone(&live.persister);
        let stored = tokio::task::spawn_blocking(move || {
            crate::block_spends::store_spent(&persister, wallet, removed)
        })
        .await;
        drop(wm);
        if !matches!(stored, Ok(Ok(()))) {
            tracing::warn!(%wallet, ?stored, "could not store the dropped coins as spent");
        }
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
            let persisted = match persisted_txids(&db_path) {
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
            sent.retain(|t, _| persisted.keeps_mark(t, &abandoned.unread));
            if !(read && marks_read && sent_read != crate::tx_actions::SentRead::Failed) {
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
        ViewParts {
            owned: owned_scripts(info),
            payees: self.payees(info, wallet),
            tip: meta
                .last_processed_height
                .max(meta.synced_height)
                .max(self.hub.wallet_state(wallet).map_or(0, |s| s.tip())),
            chainlock_height: self.chainlock_height(&info.core_wallet),
        }
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
