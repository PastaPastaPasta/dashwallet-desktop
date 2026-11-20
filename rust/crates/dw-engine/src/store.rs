//! The persister platform-wallet is given: `SqlitePersister` with two
//! changes. Its `load` leaves out wallets the user closed (dash-qt "Close
//! Wallet", QT-101), so `load_from_persistor` at open, and again when a
//! wallet is opened, registers only the wallets that should be in memory.
//! Its `store` is the changeset tap (DASHPAY §3.5): what the SQLite
//! persister accepted goes on to `platform::journal`. Every other call goes
//! straight to the SQLite persister.

use std::collections::{HashMap, HashSet};
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
    /// Each wallet's filter checkpoint as `load` last read it.
    loaded_at: std::sync::Mutex<HashMap<RawWalletId, u32>>,
    /// Tests: a store the predicate matches fails (`LockPoisoned`) and
    /// reaches neither the SQLite persister nor the tap.
    #[cfg(test)]
    pub(crate) fail_store: std::sync::Mutex<Option<StoreFailure>>,
    /// Tests: runs once, after the store of a wallet's registration.
    #[cfg(test)]
    pub(crate) on_register: std::sync::Mutex<Option<RegisterHook>>,
}

#[cfg(test)]
pub(crate) type StoreFailure = Box<dyn Fn(&PlatformWalletChangeSet) -> bool + Send>;

#[cfg(test)]
pub(crate) type RegisterHook = Box<dyn FnOnce(RawWalletId) + Send>;

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
            loaded_at: Default::default(),
            #[cfg(test)]
            fail_store: Default::default(),
            #[cfg(test)]
            on_register: Default::default(),
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

    /// Wallet `id`'s filter checkpoint when it was last loaded.
    pub(crate) fn loaded_checkpoint(&self, id: &RawWalletId) -> Option<u32> {
        (self.loaded_at.lock().unwrap_or_else(|p| p.into_inner()))
            .get(id)
            .copied()
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

/// key-wallet records a transaction it sees again (a peer's relay after a
/// restart) with no `input_details` when the coins it spends are no longer
/// held, and the SQLite persister replaces the stored record with it: the
/// next load would no longer know which of the wallet's coins the
/// transaction spent, and restore no claim for them (#88). A record that
/// names no own input keeps the input side the stored record of the same
/// transaction names; its context and outputs are the new ones.
fn keep_known_inputs(
    changeset: &mut PlatformWalletChangeSet,
    stored: impl Fn(&Txid) -> Option<TransactionRecord>,
) {
    let Some(core) = changeset.core.as_mut() else {
        return;
    };
    for record in (core.records.iter_mut())
        .filter(|r| r.input_details.is_empty() && !r.transaction.is_coin_base())
    {
        let Some(old) = stored(&record.txid) else {
            continue;
        };
        if old.input_details.is_empty() || old.transaction.txid() != record.txid {
            continue;
        }
        record.input_details = old.input_details;
        record.net_amount = old.net_amount;
        record.fee = old.fee;
        record.direction = old.direction;
        record.transaction_type = old.transaction_type;
    }
}

/// platform-wallet hands the store every `Received` / `Change` output of a
/// record as a new coin and names in `utxo_credit_verdicts` the ones
/// key-wallet did not credit; the SQLite persister writes them unspent all
/// the same, so the next load gives back a coin the wallet does not hold
/// (#88: one an unmined own spend claims, scanned in a later process). They
/// are stored as spent instead: a later credit stores the coin unspent
/// again, as a sweep or an abandon releasing it does.
fn withhold_uncredited(changeset: &mut PlatformWalletChangeSet) {
    let Some(core) = changeset.core.as_mut() else {
        return;
    };
    if core.utxo_credit_verdicts.is_empty() {
        return;
    }
    let verdicts = &core.utxo_credit_verdicts;
    let (uncredited, credited) = std::mem::take(&mut core.new_utxos)
        .into_iter()
        .partition(|u| verdicts.contains_key(&u.outpoint));
    core.new_utxos = credited;
    core.spent_utxos.extend::<Vec<_>>(uncredited);
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
        #[cfg(test)]
        let registered = changeset.wallet_metadata.is_some();
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
        let mut changeset = changeset;
        withhold_uncredited(&mut changeset);
        keep_known_inputs(&mut changeset, |txid| {
            self.inner
                .get_core_tx_record(wallet_id, txid)
                .ok()
                .flatten()
        });
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
        #[cfg(test)]
        if registered && let Some(hook) = self.on_register.lock().unwrap().take() {
            hook(wallet_id);
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
        let mut loaded_at = self.loaded_at.lock().unwrap_or_else(|p| p.into_inner());
        for (id, wallet) in &state.wallets {
            loaded_at.insert(*id, wallet.wallet_info.metadata.synced_height);
        }
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

#[cfg(test)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::{OutPoint, TxOut, Txid};
    use platform_wallet::changeset::changeset::UtxoCreditVerdict;
    use platform_wallet::changeset::{CoreChangeSet, PlatformWalletChangeSet};

    use super::withhold_uncredited;

    fn coin(vout: u32) -> key_wallet::Utxo {
        let script =
            dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array([7; 20]));
        key_wallet::Utxo::new(
            OutPoint::new(Txid::from_byte_array([1; 32]), vout),
            TxOut {
                value: 1_000 + u64::from(vout),
                script_pubkey: script.clone(),
            },
            dashcore::Address::from_script(&script, dashcore::Network::Regtest).unwrap(),
            10,
            false,
        )
    }

    fn outpoints(utxos: &[key_wallet::Utxo]) -> Vec<OutPoint> {
        utxos.iter().map(|u| u.outpoint).collect()
    }

    /// Every output with a verdict moves from the new coins to the spent
    /// ones, whatever the verdict; the others stay new; a spent coin the
    /// batch already carried stays.
    #[test]
    fn outputs_the_wallet_did_not_credit_are_stored_spent() {
        let mut changeset = PlatformWalletChangeSet {
            core: Some(CoreChangeSet {
                new_utxos: (0..4).map(coin).collect(),
                spent_utxos: vec![coin(9)],
                utxo_credit_verdicts: [
                    (coin(1).outpoint, UtxoCreditVerdict::Uncredited),
                    (
                        coin(2).outpoint,
                        UtxoCreditVerdict::ObservedSpent { height: 9 },
                    ),
                    (coin(3).outpoint, UtxoCreditVerdict::Doomed),
                    (coin(5).outpoint, UtxoCreditVerdict::Uncredited),
                ]
                .into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        withhold_uncredited(&mut changeset);
        let core = changeset.core.unwrap();
        assert_eq!(outpoints(&core.new_utxos), [coin(0).outpoint]);
        assert_eq!(
            outpoints(&core.spent_utxos),
            [9, 1, 2, 3].map(|v| coin(v).outpoint)
        );
        assert_eq!(core.spent_utxos[1], coin(1), "the coin itself, unchanged");
    }

    /// Without verdicts, or without a core part, nothing changes.
    #[test]
    fn a_batch_without_verdicts_is_stored_as_given() {
        let mut changeset = PlatformWalletChangeSet {
            core: Some(CoreChangeSet {
                new_utxos: (0..2).map(coin).collect(),
                ..Default::default()
            }),
            ..Default::default()
        };
        withhold_uncredited(&mut changeset);
        let core = changeset.core.unwrap();
        assert_eq!(outpoints(&core.new_utxos), [0, 1].map(|v| coin(v).outpoint));
        assert!(core.spent_utxos.is_empty());

        let mut empty = PlatformWalletChangeSet::default();
        withhold_uncredited(&mut empty);
        assert!(empty.core.is_none());
    }
}
