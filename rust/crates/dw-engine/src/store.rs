//! The persister platform-wallet is given: `SqlitePersister` with two
//! changes. Its `load` leaves out wallets the user closed (dash-qt "Close
//! Wallet", QT-101), so `load_from_persistor` at open, and again when a
//! wallet is opened, registers only the wallets that should be in memory.
//! Its `store` is the changeset tap (DASHPAY §3.5): what the SQLite
//! persister accepted goes on to `platform::journal`. Every other call goes
//! straight to the SQLite persister.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use dashcore::Txid;
use dpp::prelude::Identifier;
use key_wallet::managed_account::transaction_record::TransactionRecord;
use platform_wallet::changeset::changeset::DpnsNameStateEntry;
use platform_wallet::changeset::{
    ClientStartState, ListedCoreTxid, PersistenceCapabilities, PersistenceError,
    PlatformWalletChangeSet, PlatformWalletPersistence,
};
use platform_wallet_storage::SqlitePersister;

use crate::WalletId;
use crate::platform::journal::{ChangesetTap, classify};

type RawWalletId = [u8; 32];

pub(crate) struct WalletStore {
    inner: Arc<SqlitePersister>,
    /// Wallets `load` leaves out.
    unloaded: RwLock<HashSet<RawWalletId>>,
    tap: Arc<ChangesetTap>,
    /// The session's lease table, whose journal keeps the shielded spend
    /// markers (DEC-1057): installed once the session is assembled (the
    /// table is made after this store). Weak: the table outlives no store.
    leases: std::sync::OnceLock<std::sync::Weak<crate::platform::lease::LeaseTable>>,
    /// The session's replay records, told of each committed checkpoint.
    replay: std::sync::OnceLock<std::sync::Weak<crate::replay_record::ReplayRecords>>,
    /// Tests: a store the predicate matches fails (`LockPoisoned`) and
    /// reaches neither the SQLite persister nor the tap.
    #[cfg(test)]
    pub(crate) fail_store: std::sync::Mutex<Option<StoreFailure>>,
}

#[cfg(test)]
pub(crate) type StoreFailure = Box<dyn Fn(&PlatformWalletChangeSet) -> bool + Send>;

impl WalletStore {
    pub(crate) fn new(
        inner: Arc<SqlitePersister>,
        unloaded: HashSet<RawWalletId>,
        tap: Arc<ChangesetTap>,
    ) -> Self {
        Self {
            inner,
            unloaded: RwLock::new(unloaded),
            tap,
            leases: std::sync::OnceLock::new(),
            replay: std::sync::OnceLock::new(),
            #[cfg(test)]
            fail_store: Default::default(),
        }
    }

    /// Installs the session's lease table (once; a second call is a no-op).
    pub(crate) fn install_leases(&self, leases: &Arc<crate::platform::lease::LeaseTable>) {
        let _ = self.leases.set(Arc::downgrade(leases));
    }

    /// Installs the session's replay records (once).
    pub(crate) fn install_replay(&self, replay: &Arc<crate::replay_record::ReplayRecords>) {
        let _ = self.replay.set(Arc::downgrade(replay));
    }

    pub(crate) fn is_unloaded(&self, id: &RawWalletId) -> bool {
        self.unloaded
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .contains(id)
    }

    /// Marks a wallet unloaded (`true`) or loadable. Returns whether the
    /// state changed.
    pub(crate) fn set_unloaded(&self, id: RawWalletId, unloaded: bool) -> bool {
        let mut set = self.unloaded.write().unwrap_or_else(|p| p.into_inner());
        if unloaded {
            set.insert(id)
        } else {
            set.remove(&id)
        }
    }

    pub(crate) fn unloaded(&self) -> Vec<RawWalletId> {
        let mut v: Vec<RawWalletId> = self
            .unloaded
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .copied()
            .collect();
        v.sort_unstable();
        v
    }
}

impl PlatformWalletPersistence for WalletStore {
    fn store_commits_inline(&self) -> bool {
        self.inner.store_commits_inline()
    }

    fn persistence_capabilities(&self) -> PersistenceCapabilities {
        self.inner.persistence_capabilities()
    }

    fn store_transient_is_reissuable(&self) -> bool {
        self.inner.store_transient_is_reissuable()
    }

    fn persists_durably(&self) -> bool {
        self.inner.persists_durably()
    }

    fn store(
        &self,
        wallet_id: RawWalletId,
        changeset: PlatformWalletChangeSet,
    ) -> Result<(), PersistenceError> {
        let replay = || self.replay.get().and_then(std::sync::Weak::upgrade);
        #[cfg(test)]
        if (self.fail_store.lock().unwrap().as_ref()).is_some_and(|fails| fails(&changeset)) {
            if changeset.core.is_some()
                && let Some(replay) = replay()
            {
                replay.note_store_failed(WalletId(wallet_id));
            }
            return Err(PersistenceError::LockPoisoned);
        }
        // The wallet as the call began: a removal (and a reimport) that
        // overtakes the record below leaves it unjournaled.
        let epoch = self.tap.epoch(WalletId(wallet_id));
        // Classified before the persister takes it, recorded once it has;
        // a funding call's own locks are recorded before (§2a.6).
        let classified = classify(&changeset);
        if let Some(classified) = &classified {
            self.tap.note_signed(WalletId(wallet_id), classified);
        }
        let core = changeset.core.is_some();
        // Committed when the call returns `Ok` (SQLite's `Immediate` mode).
        let committed = (changeset.core.as_ref())
            .and_then(|c| c.synced_height)
            .filter(|_| self.inner.store_commits_inline());
        // The nullifiers a proved scan, or a spend's proved result, found
        // spent (the library's only two sources: sync.rs:761-777,
        // operations.rs:2847-2854): they clear the shielded spend markers
        // once all of a marker's are (DEC-1057), after the persister took
        // them.
        let spent: Vec<[u8; 32]> = (changeset.shielded.as_ref())
            .map(|cs| {
                (cs.nullifiers_spent.iter())
                    .filter(|(id, _)| id.wallet_id == wallet_id)
                    .flat_map(|(_, nullifiers)| nullifiers.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        if let Err(e) = self.inner.store(wallet_id, changeset) {
            if core && let Some(replay) = replay() {
                replay.note_store_failed(WalletId(wallet_id));
            }
            return Err(e);
        }
        if let Some(height) = committed
            && let Some(replay) = replay()
        {
            replay.note(crate::replay_record::ReplayNote::Committed {
                wallet: WalletId(wallet_id),
                height,
            });
        }
        if !spent.is_empty()
            && let Some(leases) = self.leases.get().and_then(std::sync::Weak::upgrade)
        {
            leases.note_spent_nullifiers(WalletId(wallet_id), &spent);
        }
        if core {
            self.tap.core_changed(WalletId(wallet_id));
        }
        if let Some(classified) = classified {
            #[cfg(test)]
            self.tap.hold_record();
            (self.tap).record(WalletId(wallet_id), epoch, classified, self.inner.as_ref());
        }
        Ok(())
    }

    fn flush(&self, wallet_id: RawWalletId) -> Result<(), PersistenceError> {
        self.inner.flush(wallet_id)
    }

    fn persist_tracked_masternodes(
        &self,
        network: dashcore::Network,
        records: &[platform_wallet::masternode::TrackedMasternode],
    ) -> Result<(), PersistenceError> {
        self.inner.persist_tracked_masternodes(network, records)
    }

    fn load_tracked_masternodes(
        &self,
        network: dashcore::Network,
    ) -> Result<Vec<platform_wallet::masternode::TrackedMasternode>, PersistenceError> {
        self.inner.load_tracked_masternodes(network)
    }

    /// The SQLite start state without the unloaded wallets.
    fn load(&self) -> Result<ClientStartState, PersistenceError> {
        let mut state = self.inner.load()?;
        let unloaded = self.unloaded.read().unwrap_or_else(|p| p.into_inner());
        state.wallets.retain(|id, _| !unloaded.contains(id));
        state
            .platform_addresses
            .retain(|id, _| !unloaded.contains(id));
        Ok(state)
    }

    fn get_core_tx_record(
        &self,
        wallet_id: RawWalletId,
        txid: &Txid,
    ) -> Result<Option<TransactionRecord>, PersistenceError> {
        self.inner.get_core_tx_record(wallet_id, txid)
    }

    fn list_wallet_core_txids(
        &self,
        wallet_id: RawWalletId,
    ) -> Result<Option<Vec<ListedCoreTxid>>, PersistenceError> {
        self.inner.list_wallet_core_txids(wallet_id)
    }

    fn get_dpns_name_state(
        &self,
        wallet_id: RawWalletId,
        wallet_identity_id: &Identifier,
        normalized_label: &str,
    ) -> Result<Option<DpnsNameStateEntry>, PersistenceError> {
        self.inner
            .get_dpns_name_state(wallet_id, wallet_identity_id, normalized_label)
    }
}

/// Ids of every wallet registered in `wallet.sqlite` (read-only).
pub(crate) fn registered_wallet_ids(
    db_path: &std::path::Path,
) -> Result<Vec<RawWalletId>, rusqlite::Error> {
    if !db_path.exists() {
        return Ok(Vec::new());
    }
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut stmt = conn.prepare("SELECT wallet_id FROM wallets")?;
    let rows = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))?;
    let mut out = Vec::new();
    for row in rows {
        if let Ok(id) = RawWalletId::try_from(row?) {
            out.push(id);
        }
    }
    Ok(out)
}
