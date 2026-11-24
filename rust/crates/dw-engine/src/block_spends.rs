//! The durable "spent in a block" fact (#268, #400): the wallet store's
//! `core_block_spends` rows, as this session last read or wrote them, and
//! the hold that keeps coins under a block-spend check out of every build.
//!
//! A row says a processed block spent the coin. It outlives key-wallet's
//! observed-spend note, which the ChainLock prune drops, so a release (an
//! abandon, Remove unconfirmed) never gives back a coin a block spent, and
//! a load treats the row as a spend that beats the claim of a payment this
//! app sent. Rows go only on a chain truncation above their height, a
//! load-time check that finds their block off the header chain, or Reset
//! chain data.
//!
//! The hold is the engine's own set of outpoints: the coins the one-time
//! check is still looking at, and row-spent coins key-wallet still holds
//! in memory. Each is locked with key-wallet's `Utxo::lock`, so no
//! selector picks it (the library's asset-lock and CoinJoin builders
//! included), and only a lock the hold set itself is ever lifted by it.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock};

use dashcore::OutPoint;
use key_wallet::Utxo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use platform_wallet::PlatformWalletInfo;
use platform_wallet::changeset::{BlockSpend, CoreChangeSet};

use crate::WalletId;

/// Every caller restores spent claims through this one helper: the
/// platform wrapper remembers each claimed outpoint, so a block that later
/// spends it is captured as a row even though the wallet holds neither the
/// coin nor the claimant's record.
pub(crate) fn restore_spent_claims(
    info: &mut PlatformWalletInfo,
    claims: &[(OutPoint, Option<dashcore::Txid>)],
) -> Vec<OutPoint> {
    info.restore_spent_outpoints(claims)
}

#[derive(Default)]
pub(crate) struct BlockSpends {
    rows: RwLock<HashMap<WalletId, BTreeMap<OutPoint, BlockSpend>>>,
    /// Coins the one-time check holds: not offered until it clears them.
    checking: RwLock<HashMap<WalletId, BTreeSet<OutPoint>>>,
    /// Of the hold's coins, those whose key-wallet lock the hold set.
    locked: RwLock<HashMap<WalletId, BTreeSet<OutPoint>>>,
    /// Bumped by every change of the rows, so a read of the store that
    /// overlapped one is not installed over it.
    generation: std::sync::atomic::AtomicU64,
    /// Tests: the stored chain the check searches in place of SPV's.
    #[cfg(test)]
    pub(crate) fake_chain: RwLock<Option<Arc<dyn crate::block_spend_check::StoredChain>>>,
    /// Tests: the next check start ignores the wallet store's mark.
    #[cfg(test)]
    pub(crate) ignore_mark: std::sync::atomic::AtomicBool,
    /// Applies the hold again once rows land for a wallet
    /// ([`crate::NetworkSession::attach_block_spend_hold`]).
    on_rows: std::sync::OnceLock<Box<dyn Fn(WalletId) + Send + Sync>>,
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|p| p.into_inner())
}

fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|p| p.into_inner())
}

impl BlockSpends {
    #[cfg(test)]
    pub(crate) fn stored_chain(&self) -> Option<Arc<dyn crate::block_spend_check::StoredChain>> {
        read(&self.fake_chain).clone()
    }

    fn bump(&self) {
        (self.generation).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Installs `rows`, read from the store, unless the rows changed since
    /// `generation` was read: then the read may predate the change.
    pub(crate) fn set_rows_read_at(
        &self,
        wallet: WalletId,
        rows: BTreeMap<OutPoint, BlockSpend>,
        generation: u64,
    ) -> bool {
        let mut all = write(&self.rows);
        if self.generation() != generation {
            return false;
        }
        all.insert(wallet, rows);
        true
    }

    pub(crate) fn rows(&self, wallet: &WalletId) -> BTreeMap<OutPoint, BlockSpend> {
        read(&self.rows).get(wallet).cloned().unwrap_or_default()
    }

    pub(crate) fn has_row(&self, wallet: &WalletId, outpoint: &OutPoint) -> bool {
        read(&self.rows)
            .get(wallet)
            .is_some_and(|rows| rows.contains_key(outpoint))
    }

    /// What a stored changeset did to the rows: a truncation first, then
    /// its captured spends, upserted in order.
    pub(crate) fn applied(&self, wallet: WalletId, core: &CoreChangeSet) {
        if core.block_spends.is_empty() && core.block_spends_truncated_above.is_none() {
            return;
        }
        {
            let mut rows = write(&self.rows);
            let rows = rows.entry(wallet).or_default();
            if let Some(height) = core.block_spends_truncated_above {
                rows.retain(|_, s| s.height <= height);
            }
            for spend in &core.block_spends {
                rows.insert(spend.outpoint, *spend);
            }
            self.bump();
        }
        if !core.block_spends.is_empty()
            && let Some(hold) = self.on_rows.get()
        {
            hold(wallet);
        }
    }

    pub(crate) fn attach_on_rows(&self, run: Box<dyn Fn(WalletId) + Send + Sync>) {
        let _ = self.on_rows.set(run);
    }

    /// A fork replaced the blocks above `height`.
    pub(crate) fn truncate(&self, wallet: &WalletId, height: u32) {
        if let Some(rows) = write(&self.rows).get_mut(wallet) {
            rows.retain(|_, s| s.height <= height);
            self.bump();
        }
    }

    pub(crate) fn insert(&self, wallet: WalletId, spend: BlockSpend) {
        write(&self.rows)
            .entry(wallet)
            .or_default()
            .insert(spend.outpoint, spend);
        self.bump();
    }

    pub(crate) fn clear(&self, wallet: &WalletId) {
        write(&self.rows).remove(wallet);
        self.bump();
    }

    pub(crate) fn set_checking(&self, wallet: WalletId, outpoints: BTreeSet<OutPoint>) {
        let mut checking = write(&self.checking);
        if outpoints.is_empty() {
            checking.remove(&wallet);
        } else {
            checking.insert(wallet, outpoints);
        }
    }

    pub(crate) fn checking(&self, wallet: &WalletId) -> BTreeSet<OutPoint> {
        read(&self.checking)
            .get(wallet)
            .cloned()
            .unwrap_or_default()
    }

    /// The hold's own outpoints: the coins under check and the row-spent
    /// ones.
    pub(crate) fn held(&self, wallet: &WalletId) -> BTreeSet<OutPoint> {
        let mut held = self.checking(wallet);
        held.extend(read(&self.rows).get(wallet).into_iter().flat_map(|r| r.keys()));
        held
    }

    /// Applies the hold to `core`: a coin a row at or below `chain_lock`
    /// spent leaves the funds accounts (the block is final; returned so
    /// the caller stores it spent), every other coin of the hold is locked,
    /// and the hold's own lock is lifted from a coin that left it. Returns
    /// the coins taken out and whether a lock changed.
    pub(crate) fn apply_hold(
        &self,
        wallet: WalletId,
        core: &mut ManagedWalletInfo,
        chain_lock: Option<u32>,
    ) -> (Vec<Utxo>, bool) {
        let final_rows: BTreeSet<OutPoint> = read(&self.rows)
            .get(&wallet)
            .into_iter()
            .flat_map(|rows| rows.values())
            .filter(|s| chain_lock.is_some_and(|cl| s.height <= cl))
            .map(|s| s.outpoint)
            .collect();
        let held = self.held(&wallet);
        let mut locked = write(&self.locked);
        let own = locked.entry(wallet).or_default();
        let (mut removed, mut changed) = (Vec::new(), false);
        for account in core.accounts.all_funding_accounts_mut() {
            removed.extend(final_rows.iter().filter_map(|o| account.utxos.remove(o)));
            for (outpoint, utxo) in account.utxos.iter_mut() {
                if held.contains(outpoint) {
                    if !utxo.is_locked {
                        utxo.lock();
                        own.insert(*outpoint);
                        changed = true;
                    }
                } else if own.remove(outpoint) && utxo.is_locked {
                    utxo.unlock();
                    changed = true;
                }
            }
        }
        own.retain(|o| held.contains(o));
        (removed, changed)
    }

    /// Locks every coin of the hold `core` holds that is not locked yet
    /// (under the caller's write guard); returns whether one was.
    pub(crate) fn lock_held(&self, wallet: WalletId, core: &mut ManagedWalletInfo) -> bool {
        let held = self.held(&wallet);
        if held.is_empty() {
            return false;
        }
        let mut locked = write(&self.locked);
        let own = locked.entry(wallet).or_default();
        let mut changed = false;
        for account in core.accounts.all_funding_accounts_mut() {
            for (outpoint, utxo) in account.utxos.iter_mut() {
                if held.contains(outpoint) && !utxo.is_locked {
                    utxo.lock();
                    own.insert(*outpoint);
                    changed = true;
                }
            }
        }
        changed
    }

    /// Forgets that the hold set `outpoint`'s lock: another holder (a
    /// fence's sweep) took the flag over.
    pub(crate) fn disown(&self, wallet: &WalletId, outpoint: &OutPoint) {
        if let Some(own) = write(&self.locked).get_mut(wallet) {
            own.remove(outpoint);
        }
    }

    /// Whether the hold keeps `outpoint` (a fence's sweep of stale locks
    /// passes over it).
    pub(crate) fn holds(&self, wallet: &WalletId, outpoint: &OutPoint) -> bool {
        self.has_row(wallet, outpoint)
            || read(&self.checking)
                .get(wallet)
                .is_some_and(|c| c.contains(outpoint))
    }

    pub(crate) fn forget(&self, wallet: &WalletId) {
        write(&self.rows).remove(wallet);
        write(&self.checking).remove(wallet);
        write(&self.locked).remove(wallet);
    }
}

impl crate::NetworkSession {
    /// Reads `wallet`'s block-spend rows into the session, after the
    /// load-time check: a row above the wallet's ChainLock whose block the
    /// SPV header chain names differently is off the best chain and goes.
    /// A height whose header is unknown keeps its row; it is checked again
    /// on a later pass, before a release reads the rows, and by the
    /// session's periodic check.
    /// Returns whether the session's rows changed; `Err` when the rows
    /// cannot be read or checked, so a release refuses rather than read a
    /// stale set.
    pub(crate) async fn check_block_spends(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
    ) -> Result<bool, crate::EngineError> {
        let live = self.live()?;
        let chain_lock = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(info) = wm.get_wallet_info(&wallet.0) else {
                return Ok(false);
            };
            crate::history_ops::applied_chainlock(info).unwrap_or(0)
        };
        let storage = |e: &dyn std::fmt::Display| {
            crate::EngineError::Storage(format!("the wallet's block spends: {e}"))
        };
        for _ in 0..3 {
            let generation = self.hub.block_spends.generation();
            let persister = Arc::clone(&live.persister);
            let rows = tokio::task::spawn_blocking(move || persister.block_spent_outpoints(&wallet.0))
                .await?
                .map_err(|e| storage(&e))?;
            let heights: BTreeSet<u32> = (rows.values())
                .map(|s| s.height)
                .filter(|h| *h > chain_lock)
                .collect();
            let rows = if heights.is_empty() {
                rows
            } else {
                let mut chain = BTreeMap::new();
                for h in heights {
                    chain.insert(h, self.hub.header_hash(h).await);
                }
                let persister = Arc::clone(&live.persister);
                tokio::task::spawn_blocking(move || {
                    persister.delete_block_spends_off_chain(&wallet.0, chain_lock, &chain)?;
                    persister
                        .block_spent_outpoints(&wallet.0)
                        .map_err(platform_wallet::changeset::PersistenceError::from)
                })
                .await?
                .map_err(|e| storage(&e))?
            };
            let before = self.hub.block_spends.rows(&wallet);
            if self.hub.block_spends.set_rows_read_at(wallet, rows.clone(), generation) {
                return Ok(before != rows);
            }
        }
        Ok(false)
    }

    /// Applies the hold again, on the session's runtime, whenever rows land
    /// for a wallet: a coin the release gave back in memory before its row
    /// was stored is locked or taken out then.
    pub(crate) fn attach_block_spend_hold(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.block_spends.attach_on_rows(Box::new(move |wallet| {
            let weak = weak.clone();
            rt.spawn(async move {
                let Some(session) = weak.upgrade() else { return };
                if let Ok(manager) = session.manager() {
                    session.hold_block_spends(&manager, wallet).await;
                }
            });
        }));
    }

    /// [`BlockSpends::apply_hold`] on `info`, held under the manager's write
    /// lock: the balance is recomputed and published when a coin left or a
    /// lock changed. Returns the coins taken out, for the caller to store
    /// spent under the same lock.
    pub(crate) fn hold_block_spends_locked(
        &self,
        wallet: WalletId,
        info: &mut PlatformWalletInfo,
    ) -> Vec<Utxo> {
        let chain_lock = crate::history_ops::applied_chainlock(info);
        let core = &mut info.core_wallet;
        let (removed, changed) = self.hub.block_spends.apply_hold(wallet, core, chain_lock);
        if changed || !removed.is_empty() {
            core.update_balance();
            self.hub
                .publish_core_balance(wallet, &core.balance(), &core.account_balances());
        }
        for utxo in &removed {
            tracing::warn!(%wallet, outpoint = %utxo.outpoint, value = utxo.value(),
                "dropped a coin a ChainLocked block spent");
        }
        removed
    }

    /// Stores the block spends platform-wallet captured for `wallet` and
    /// its event bridge has not drained yet, so a release computed under
    /// the same lock sees them. The session takes them first, and a failed
    /// store refuses the release: the spends are no longer queued anywhere
    /// else.
    pub(crate) async fn store_pending_block_spends(
        &self,
        wallet: WalletId,
        info: &PlatformWalletInfo,
    ) -> Result<(), crate::EngineError> {
        let spends = info.drain_block_spends();
        if spends.is_empty() {
            return Ok(());
        }
        let core = CoreChangeSet {
            block_spends: spends,
            ..Default::default()
        };
        self.hub.block_spends.applied(wallet, &core);
        let persister = Arc::clone(&self.live()?.persister);
        tokio::task::spawn_blocking(move || {
            use platform_wallet::changeset::PlatformWalletPersistence;
            let changeset = platform_wallet::changeset::PlatformWalletChangeSet {
                core: Some(core),
                ..Default::default()
            };
            persister.store(wallet.0, changeset)?;
            persister.flush(wallet.0)
        })
        .await?
        .map_err(|e| crate::EngineError::Storage(format!("the captured block spends: {e}")))
    }

    /// [`Self::hold_block_spends_locked`] under the manager's write lock,
    /// storing the coins it takes out as spent.
    pub(crate) async fn hold_block_spends(&self, manager: &crate::session::Manager, wallet: WalletId) {
        let Ok(live) = self.live() else {
            return;
        };
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some(info) = wm.get_wallet_info_mut(&wallet.0) else {
            return;
        };
        let removed = self.hold_block_spends_locked(wallet, info);
        if removed.is_empty() {
            return;
        }
        let persister = Arc::clone(&live.persister);
        let stored = tokio::task::spawn_blocking(move || {
            store_spent(&persister, wallet, removed)
        })
        .await;
        drop(wm);
        if !matches!(stored, Ok(Ok(()))) {
            tracing::warn!(%wallet, ?stored, "could not store the block-spent coins as spent");
        }
    }
}

/// Stores `spent` as spent coins of `wallet`.
pub(crate) fn store_spent(
    persister: &platform_wallet_storage::SqlitePersister,
    wallet: WalletId,
    spent: Vec<Utxo>,
) -> Result<(), platform_wallet::changeset::PersistenceError> {
    use platform_wallet::changeset::PlatformWalletPersistence;
    let changeset = platform_wallet::changeset::PlatformWalletChangeSet {
        core: Some(CoreChangeSet {
            spent_utxos: spent,
            ..Default::default()
        }),
        ..Default::default()
    };
    persister.store(wallet.0, changeset)?;
    persister.flush(wallet.0)
}

#[cfg(test)]
mod tests {
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

    /// C1: no engine code restores spent claims on key-wallet's wallet
    /// directly. Every caller goes through [`super::restore_spent_claims`],
    /// whose platform wrapper remembers the claims for the block-spend
    /// capture.
    #[test]
    fn only_the_helper_restores_spent_claims() {
        let call = concat!(".restore_spent", "_outpoints(");
        let helper = Path::new(file!()).file_name().unwrap();
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut found = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).unwrap();
            for (n, line) in text.lines().enumerate() {
                if !line.contains(call) {
                    continue;
                }
                let in_helper = file.file_name() == Some(helper)
                    && line.trim() == concat!("info.restore_spent", "_outpoints(claims)");
                if !in_helper {
                    found.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
                }
            }
        }
        assert!(found.is_empty(), "spent claims restored past the helper: {found:#?}");
    }
}
