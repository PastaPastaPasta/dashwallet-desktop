//! Wallet registry: the wallet list with names and balances, rename and
//! remove (QT-014, QT-021, QT-101, IOS-109/110).
//!
//! The synchronous reads use only in-memory state: names cached from
//! dw-appdb at open, balances and scan heights kept from wallet events, the
//! vault's in-memory record index.

use std::sync::Arc;

use dw_vault::GrantKind;

use crate::events::{WalletName, unix_now};
use crate::platform::RevokeCause;
use crate::{EngineError, EngineEvent, NetworkSession, WalletBalances, WalletId};

/// Tests: the next wallet removal of a session on one of these data
/// directories fails to delete the wallet's store.
#[cfg(test)]
pub(crate) static FAIL_DELETE_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: the next wallet removal of a session on one of these data
/// directories fails its app-database transaction, after its store is gone.
#[cfg(test)]
pub(crate) static FAIL_LISTING_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: the next wallet removal of a session on one of these data
/// directories stops once its app rows are gone, before the journal's
/// erase, as a process stopped there would.
#[cfg(test)]
pub(crate) static STOP_BEFORE_ERASE_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Tests: the next load-on-startup write on its own fails: the one after
/// a failed removal transaction, or the one that lists a registered wallet.
#[cfg(test)]
pub(crate) static FAIL_LISTING_RETRY_IN: std::sync::Mutex<Vec<std::path::PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// Longest wallet name, in characters, after trimming.
pub const MAX_WALLET_NAME: usize = 64;

/// One registered wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletInfo {
    pub wallet_id: WalletId,
    pub name: String,
    /// No signing keys are available (the vault holds no seed for it).
    pub watch_only: bool,
    /// The vault holds the wallet's recovery phrase.
    pub has_mnemonic: bool,
    /// The wallet was imported from a Dash Core wallet.dat with legacy
    /// issued address ranges, which a recovery phrase doesn't restore. A
    /// failed read counts as `true`.
    pub legacy_issued: bool,
    /// Every platform-wallet wallet is an HD wallet.
    pub hd: bool,
    pub birth_height: Option<u32>,
    /// When the wallet was added on this device; `None` for wallets added
    /// before names were stored.
    pub created_at: Option<u64>,
    /// The balance known so far (`WalletBalances::catching_up` until the scan
    /// has processed the birth block, review M-3); `None` before the wallet's
    /// state was first read.
    pub balances: Option<WalletBalances>,
}

/// Trims and checks a wallet name (1–64 characters).
pub(crate) fn validate_name(name: &str) -> Result<String, EngineError> {
    let trimmed = name.trim();
    let n = trimmed.chars().count();
    if n == 0 || n > MAX_WALLET_NAME {
        return Err(EngineError::NameRejected(format!(
            "name must be 1-{MAX_WALLET_NAME} characters after trimming, got {n}"
        )));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(EngineError::NameRejected(
            "name contains control characters".into(),
        ));
    }
    Ok(trimmed.to_string())
}

impl NetworkSession {
    fn info_of(&self, id: WalletId) -> WalletInfo {
        let name = self.hub.name_of(&id);
        let state = self.hub.wallet_state(&id);
        let has_secret = self.vault.has_wallet_secret(&id.0);
        WalletInfo {
            wallet_id: id,
            name: name
                .as_ref()
                .map(|n| n.name.clone())
                .unwrap_or_else(|| default_name_for(&id)),
            watch_only: !has_secret,
            // A wallet imported from a raw seed has keys but no phrase.
            has_mnemonic: self.vault.has_wallet_mnemonic(&id.0),
            legacy_issued: self.live().map_or(true, |live| {
                (live.appdb)
                    .setting(&id.to_string(), crate::compat::ISSUED_SETTING)
                    .map_or(true, |v| v.is_some())
            }),
            hd: true,
            birth_height: state.as_ref().map(|s| s.birth_height),
            created_at: name.and_then(|n| n.created_at),
            balances: state.map(|s| s.balances()),
        }
    }

    /// Every registered wallet, in the order they were added (wallets
    /// without a stored name last, by id). In-memory but for one settings
    /// read per wallet.
    pub fn wallet_infos(&self) -> Result<Vec<WalletInfo>, EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        let mut infos: Vec<WalletInfo> = manager
            .list_wallet_ids_blocking()
            .into_iter()
            .map(|id| self.info_of(WalletId(id)))
            .collect();
        // Named wallets in the order they were added, then the rest by id.
        let order = |w: &WalletInfo| self.hub.name_of(&w.wallet_id).map(|n| n.order);
        infos.sort_by_key(|w| (order(w).is_none(), order(w), w.wallet_id));
        Ok(infos)
    }

    /// One wallet. In-memory read.
    pub fn wallet_info(&self, id: &WalletId) -> Result<WalletInfo, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(id)?;
        Ok(self.info_of(*id))
    }

    /// `WalletNotFound` unless the wallet is registered (wait-free).
    pub(crate) fn require_wallet(&self, id: &WalletId) -> Result<(), EngineError> {
        if self.manager()?.list_wallet_ids_blocking().contains(&id.0) {
            Ok(())
        } else {
            Err(EngineError::WalletNotFound(id.to_string()))
        }
    }

    /// Height up to which the wallet's compact filters have been scanned
    /// (its filter checkpoint). A replay the user cancelled before it was
    /// back counts as scanned only to where it stopped, until a rewind
    /// replays what it skipped. In-memory read.
    pub fn wallet_scan_height(&self, id: &WalletId) -> Option<u32> {
        self.hub.wallet_state(id).map(|s| {
            s.replay_cancelled
                .map_or(s.synced_height, |reached| s.synced_height.min(reached))
        })
    }

    /// Balance buckets known so far (`WalletBalances::catching_up` while the
    /// scan has not reached the birth block); `None` before the wallet's
    /// state was first read. In-memory read.
    pub fn balances(&self, id: &WalletId) -> Result<Option<WalletBalances>, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(id)?;
        Ok(self.hub.wallet_state(id).map(|s| s.balances()))
    }

    /// Stores the name of a wallet in dw-appdb and the in-memory cache.
    pub(crate) async fn store_name(
        &self,
        id: WalletId,
        name: String,
        keep_created_at: Option<u64>,
    ) -> Result<(), EngineError> {
        let appdb = self.live()?.appdb;
        let now = unix_now();
        let order = match self.hub.name_of(&id) {
            Some(existing) => existing.order,
            None => self.hub.next_name_order(),
        };
        let stored = name.clone();
        tokio::task::spawn_blocking(move || appdb.set_wallet_name(&id.to_string(), &stored, now))
            .await?
            .map_err(|e| EngineError::Storage(e.to_string()))?;
        self.hub.set_name(
            id,
            WalletName {
                name,
                // The row keeps its first creation time.
                created_at: Some(keep_created_at.unwrap_or(now)),
                order,
            },
        );
        Ok(())
    }

    /// The default name of the next wallet: "Wallet N", N one more than the
    /// number of registered wallets, skipping names in use.
    pub(crate) fn next_default_name(&self) -> String {
        let count = self
            .manager()
            .map(|m| m.list_wallet_ids_blocking().len())
            .unwrap_or(0);
        let used: Vec<String> = self
            .hub
            .names
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|n| n.name.clone())
            .collect();
        (count + 1..)
            .map(|n| format!("Wallet {n}"))
            .find(|n| !used.contains(n))
            .unwrap_or_else(|| "Wallet".into())
    }

    /// Renames a wallet (1–64 characters after trimming).
    pub async fn rename_wallet(
        self: &Arc<Self>,
        id: WalletId,
        name: String,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let name = validate_name(&name)?;
            this.require_wallet(&id)?;
            let created_at = this.hub.name_of(&id).and_then(|n| n.created_at);
            this.store_name(id, name, created_at).await
        })
        .await
    }

    /// Removes a wallet: unloads it, deletes its wallet-state rows, its app
    /// metadata and finally its vault records (IOS-109, QT-101). Needs a
    /// `Wipe` grant, redeemed after the wallet is found. Refused (`Storage`)
    /// before anything changes while the dispatch journal is unavailable,
    /// and (`ShieldedSpendHeld`) while the wallet holds a shielded spend
    /// whose result is not known, as Close Wallet is refused, or has one
    /// stored in a shielded store not opened this session
    /// (`shielded_pool::removal_hold`).
    ///
    /// Deleting the vault records needs a full-scope data key: the vault's,
    /// or the grant's own key when it was issued with the passphrase on a
    /// locked or mixing-only vault. For a wallet with a seed the grant is
    /// checked up front without consuming it, so a locked vault with a grant
    /// that carries no key is refused (`Vault(Locked)`) before anything is
    /// consumed or deleted.
    ///
    /// The vault records go first and fail the removal closed (E0-04
    /// DEC-134): if they cannot be deleted (the vault was locked in the
    /// meantime, a write failed), the call returns that error, the wallet
    /// stays listed and usable, and nothing else of it is removed. The
    /// wallet's leases are revoked and the grant is consumed either way.
    /// Once the seed is gone the removal is committed: a later step's
    /// failure is returned and leaves the wallet without a seed. If its
    /// wallet rows could not be deleted, removing it again finishes the
    /// removal, after a reopen of the session when the wallet manager let it
    /// go. If only its app metadata could not be, the wallet is gone: an
    /// import of it in this session deletes the metadata first, or is
    /// refused (`Storage`) while it cannot, and so does the next lone open
    /// of the app database when the wallet was recorded as removed.
    /// The load-on-startup list loses the wallet in the transaction that
    /// deletes its app metadata; when nothing else would be on it, the first
    /// open wallet (the one the app shows next) is put on it. If that
    /// transaction fails, the list is stored on its own, and kept in memory
    /// even if that fails too (logged).
    /// `WalletRemoved` is
    /// sent once the wallet's rows are gone. The removal holds the wallet's
    /// barrier throughout, so a lease begun meanwhile finds no wallet.
    pub async fn remove_wallet(
        self: &Arc<Self>,
        id: WalletId,
        grant_id: String,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // Before its wallet check: an import of the wallet runs wholly
            // before or after the removal (DEC-210 (3b)).
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let live = this.live()?;
            this.require_wallet(&id)?;
            this.require_journal()?;
            if this.vault.has_wallet_secret(&id.0) {
                this.vault
                    .check_grant(&grant_id, GrantKind::Wipe, Some(&id.0))?;
            }
            // A shielded call of the wallet in flight ends first and none
            // starts until the removal is done, as for Close Wallet: none
            // stores a spend after the check below.
            let _shielded = crate::platform::shielded_pool::wallet_lock(&this, id)
                .lock_owned()
                .await;
            // Q86 Q-PURGE C2: a wallet holding a shielded spend whose result
            // is not known is not removed, as the removal would delete the
            // spend's record and its notes' hold; refused before anything of
            // it changes, and checked again below.
            let held = crate::platform::shielded_pool::removal_hold(&this, &live.manager, &live.appdb, id);
            if let Some(hold) = held.await {
                return Err(EngineError::ShieldedSpendHeld(hold));
            }
            // E0-04 §8.6: the wallet's leases end and its permits drain
            // before anything of it goes.
            let _barrier = this.revoke_wallet(id, RevokeCause::WalletRemoved).await;
            // MG-06 D23: a proposal whose fee is paid outlives the removal,
            // or the fee is lost with its object. Its failure fails the
            // removal before anything goes.
            let kept = this.governance().kept_on_removal(id)?;
            if !kept.is_empty() {
                tracing::warn!(wallet_id = %id, proposals = kept.len(), "keeping the paid proposals of a removed wallet");
            }
            // On the blocking pool as the deletion below is, not through
            // `appdb_op`: the tests' first-job hold is the deletion's.
            let (appdb, wallet_id) = (Arc::clone(&live.appdb), id.to_string());
            tokio::task::spawn_blocking(move || appdb.keep_proposals(&wallet_id, &kept)).await??;
            let vault = this.vault.clone();
            // The grant is redeemed (consumed) before anything is deleted. Its
            // token carries the key of a passphrase grant on a locked vault.
            let token = tokio::task::spawn_blocking(move || {
                vault.redeem_grant(&grant_id, GrantKind::Wipe, Some(&id.0))
            })
            .await??;

            // Its discoveries end first, and none starts until the removal
            // is done: none may keep its master key, or apply what it finds,
            // past the removal (review DP1-05 r1 M3).
            let _discoveries = this.platform.recovery.end_discoveries(id).await;
            // Again before the seed goes, now that its leases drained: a hold
            // that came up meanwhile keeps the seed. The leases come back.
            let held = crate::platform::shielded_pool::removal_hold(&this, &live.manager, &live.appdb, id);
            if let Some(hold) = held.await {
                this.spawn_ensure_background();
                return Err(EngineError::ShieldedSpendHeld(hold));
            }
            crate::platform::preserve_consumed_proofs(&this, id).await?;
            // DEC-134: the vault records first; their failure fails the
            // removal closed, before anything else of the wallet goes.
            let vault = this.vault.clone();
            tokio::task::spawn_blocking(move || vault.wipe_wallet_secret(&id.0, &token)).await??;
            // Its gate once more, fail closed (`ShieldedSpendUnresolved`,
            // the seed gone: removing it again finishes the removal).
            (this.platform)
                .remove_from_manager(&this.leases, &live.manager, &live.appdb, id)
                .await?;
            this.hub.forget_wallet(&id);
            this.coin_fences.forget(&id);
            this.platform.forget(&id);
            let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
            let was_listed = (this.startup_list.lock())
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .is_none_or(|list| list.contains(&id));
            let startup = this.startup_list_after_removal(&live, id);
            let stored = startup.clone();
            #[cfg(test)]
            let pause = crate::coins::held_appdb_job();
            #[cfg(test)]
            let fail_delete = crate::keys::take_test_failure(&FAIL_DELETE_IN, this.data_dir());
            #[cfg(test)]
            let fail_listing = crate::keys::take_test_failure(&FAIL_LISTING_IN, this.data_dir());
            let wallet_gone = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let gone = Arc::clone(&wallet_gone);
            let deleted = tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                #[cfg(test)]
                if let Some(pause) = pause {
                    pause.hold();
                }
                #[cfg(test)]
                if fail_delete {
                    return Err(EngineError::Storage("test: wallet delete failed".into()));
                }
                persister.delete_wallet(id.0)?;
                gone.store(true, std::sync::atomic::Ordering::SeqCst);
                #[cfg(test)]
                if fail_listing {
                    return Err(EngineError::Storage("test: app rows delete failed".into()));
                }
                crate::multiwallet::delete_wallet_listing(&appdb, id, stored.as_ref())
                    .map_err(|e| EngineError::Storage(e.to_string()))
            })
            .await;
            if let Some(mut list) = startup {
                // A listed wallet still in the store stays listed, beside
                // the next one: the next start loads it and the removal can
                // be retried, and a later list write keeps it.
                if was_listed && !wallet_gone.load(std::sync::atomic::Ordering::SeqCst) {
                    list.insert(id);
                }
                if !matches!(deleted, Ok(Ok(()))) {
                    this.store_startup_list(&live, &list).await;
                }
                *this.startup_list.lock().unwrap_or_else(|p| p.into_inner()) = Some(list);
            }
            // The wallet has left the manager: a journal write begun before
            // is not admitted from here on, even if a delete failed (review
            // DP2-05 r3 finding 1). An import of it deletes what a failed
            // delete left, or is refused (`AppDb::readmit_wallet`).
            live.tap.forget(&id);
            live.replay.forget(id).await;
            deleted??;
            this.sink.emit(EngineEvent::WalletRemoved {
                network: this.network.clone(),
                wallet_id: id,
            });
            #[cfg(test)]
            if crate::keys::take_test_failure(&STOP_BEFORE_ERASE_IN, this.data_dir()) {
                return Ok(());
            }
            this.erase_dispatch_rows(id).await;
            Ok(())
        })
        .await
    }
}

/// Name shown for a wallet that has no stored name.
fn default_name_for(id: &WalletId) -> String {
    format!("Wallet {}", &id.to_string()[..8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name("  Savings ").unwrap(), "Savings");
        assert!(validate_name("   ").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
        assert_eq!(validate_name(&"é".repeat(64)).unwrap().chars().count(), 64);
        assert!(validate_name("a\nb").is_err());
    }
}
