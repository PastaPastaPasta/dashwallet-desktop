//! Wallet registry: the wallet list with names and balances, rename and
//! remove (QT-014, QT-021, QT-101, IOS-109/110).
//!
//! The synchronous reads use only in-memory state: names cached from
//! dw-appdb at open, balances and scan heights kept from wallet events, the
//! vault's in-memory record index.

use std::sync::Arc;

use dw_vault::GrantKind;

use crate::events::{WalletName, unix_now};
use crate::{EngineError, EngineEvent, NetworkSession, WalletBalances, WalletId};

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
    /// Every platform-wallet wallet is an HD wallet.
    pub hd: bool,
    pub birth_height: Option<u32>,
    /// When the wallet was added on this device; `None` for wallets added
    /// before names were stored.
    pub created_at: Option<u64>,
    /// `None` until the scan has processed the birth block (review M-3).
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
        let has_mnemonic = self.vault.has_wallet_secret(&id.0);
        WalletInfo {
            wallet_id: id,
            name: name
                .as_ref()
                .map(|n| n.name.clone())
                .unwrap_or_else(|| default_name_for(&id)),
            watch_only: !has_mnemonic,
            has_mnemonic,
            hd: true,
            birth_height: state.as_ref().map(|s| s.birth_height),
            created_at: name.and_then(|n| n.created_at),
            balances: state.and_then(|s| s.balances()),
        }
    }

    /// Every registered wallet, in creation order (wallets without a stored
    /// creation time last, by id). In-memory read.
    pub fn wallet_infos(&self) -> Result<Vec<WalletInfo>, EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        let mut infos: Vec<WalletInfo> = manager
            .list_wallet_ids_blocking()
            .into_iter()
            .map(|id| self.info_of(WalletId(id)))
            .collect();
        infos.sort_by(|a, b| {
            (a.created_at.is_none(), a.created_at, a.wallet_id).cmp(&(
                b.created_at.is_none(),
                b.created_at,
                b.wallet_id,
            ))
        });
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

    /// Balance buckets; `None` while not known yet. In-memory read.
    pub fn balances(&self, id: &WalletId) -> Result<Option<WalletBalances>, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(id)?;
        Ok(self.hub.wallet_state(id).and_then(|s| s.balances()))
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
    /// `Wipe` grant, redeemed after the wallet is found. Emits
    /// `WalletRemoved`.
    pub async fn remove_wallet(
        self: &Arc<Self>,
        id: WalletId,
        grant_id: String,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            this.require_wallet(&id)?;
            let vault = this.vault.clone();
            let token =
                tokio::task::spawn_blocking(move || vault.redeem_grant(&grant_id, GrantKind::Wipe))
                    .await??;

            live.manager.remove_wallet(&id.0).await?;
            this.hub.forget_wallet(&id);
            let (persister, appdb, vault) = (
                Arc::clone(&live.persister),
                Arc::clone(&live.appdb),
                this.vault.clone(),
            );
            tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                // Wallet rows first: until the vault records go, the seed
                // can still restore the wallet if a later step fails.
                persister.delete_wallet(id.0)?;
                appdb
                    .delete_wallet(&id.to_string())
                    .map_err(|e| EngineError::Storage(e.to_string()))?;
                drop(token);
                vault.delete_wallet_secret(&id.0)?;
                Ok(())
            })
            .await??;
            this.sink.emit(EngineEvent::WalletRemoved {
                network: this.network.clone(),
                wallet_id: id,
            });
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
