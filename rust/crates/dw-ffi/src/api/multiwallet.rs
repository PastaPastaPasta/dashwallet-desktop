//! M2 wallet lifecycle: load/unload (dash-qt Open/Close wallet), the
//! load-on-startup list, watch-only wallets from an xpub, account xpub
//! export and the per-network data inventory. Owner: R1 (engine-tools).
//! Contract: docs/contracts/m2-engine.md §2.1.

use crate::api::common::parse_wallet_id;
use crate::{DashNetwork, Engine, EngineError, NetworkSession, WalletError};

/// Load state of one registered wallet (QT-101). Unloaded wallets keep their
/// data but are absent from `wallet_infos` and every wallet-scoped call
/// returns `wallet_not_found` for them, as a closed dash-qt wallet is not
/// in the wallet selector.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletLoadState {
    pub wallet_id: String,
    /// The wallet's display name (dw-appdb), so the "Open Wallet" menu can
    /// list unloaded wallets.
    pub name: String,
    pub loaded: bool,
    /// Loaded when the network opens (dash-qt settings.json `"wallet"` list).
    pub load_on_startup: bool,
    pub watch_only: bool,
}

/// Options for `import_watch_only` (QT-114).
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct WatchOnlyOptions {
    /// 1–64 characters after trimming; `None` = "Wallet N".
    pub name: Option<String>,
    /// First block to scan; `None` = genesis (an xpub has no birth date).
    pub birth_height: Option<u32>,
    /// Gap limit of the watched chains, 1..=1000; `None` = 30.
    pub lookahead: Option<u32>,
}

/// An account-level extended public key (IOS-111).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AccountXpub {
    /// BIP44 account index.
    pub account: u32,
    /// `m/44'/5'/0'` (mainnet) or `m/44'/1'/0'` (test networks).
    pub derivation_path: String,
    /// `xpub…` on mainnet, `tpub…` elsewhere.
    pub xpub: String,
}

/// What the data root holds for one network, read without opening it
/// (IOS-009 existing-wallet detection on first run or reinstall).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NetworkDataInfo {
    pub network: DashNetwork,
    /// The network directory.
    pub directory: String,
    /// `wallet.sqlite` exists.
    pub has_wallet_state: bool,
    /// A vault file exists in `vault/`.
    pub has_vault: bool,
    /// The OS secret store holds slot O of this network's vault (an
    /// unencrypted vault whose files may have been deleted).
    pub has_os_store_key: bool,
}

impl From<dw_engine::NetworkDataInfo> for NetworkDataInfo {
    fn from(i: dw_engine::NetworkDataInfo) -> Self {
        Self {
            network: i.network.into(),
            directory: i.directory,
            has_wallet_state: i.has_wallet_state,
            has_vault: i.has_vault,
            has_os_store_key: i.has_os_store_key,
        }
    }
}

#[uniffi::export]
impl Engine {
    /// Every network that has data under the data root, in `DashNetwork`
    /// order (devnets by name). Reads the file system and the OS secret
    /// store only; opens nothing. `has_os_store_key` is `false` when the
    /// vault file is gone (its OS-store address is in the file) or the
    /// store cannot be reached.
    pub async fn existing_networks(&self) -> Result<Vec<NetworkDataInfo>, EngineError> {
        Ok(self
            .inner
            .existing_networks()
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

#[uniffi::export]
impl NetworkSession {
    /// Every registered wallet with its load state, in creation order.
    /// In-memory read.
    pub fn wallet_load_states(&self) -> Result<Vec<WalletLoadState>, WalletError> {
        Ok(self
            .inner
            .wallet_load_states()?
            .into_iter()
            .map(|w| WalletLoadState {
                wallet_id: w.wallet_id.to_string(),
                name: w.name,
                loaded: w.loaded,
                load_on_startup: w.load_on_startup,
                watch_only: w.watch_only,
            })
            .collect())
    }

    /// dash-qt "Open Wallet": loads a registered, unloaded wallet and resumes
    /// its scan from its last processed height. Idempotent. Emits
    /// `WalletLoadChanged{loaded: true}`.
    pub async fn load_wallet(&self, wallet_id: String) -> Result<(), WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.load_wallet(id).await?)
    }

    /// dash-qt "Close Wallet": stops tracking the wallet and drops it from
    /// memory; its data, vault records and app metadata stay. Prepared,
    /// unsent payments of the wallet lose their input reservations (they can
    /// no longer be broadcast: `wallet_not_found`). Idempotent. Emits
    /// `WalletLoadChanged{loaded: false}`.
    pub async fn unload_wallet(&self, wallet_id: String) -> Result<(), WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.unload_wallet(id).await?)
    }

    /// Adds or removes the wallet from the load-on-startup list (dash-qt adds
    /// on Create/Open/Restore and removes on Close; the host calls this with
    /// the same rule). Persisted in dw-appdb.
    pub async fn set_load_on_startup(
        &self,
        wallet_id: String,
        load_on_startup: bool,
    ) -> Result<(), WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.set_load_on_startup(id, load_on_startup).await?)
    }

    /// Registers a watch-only wallet from a BIP44 account xpub (QT-114;
    /// interim for platform-wallet `WatchOnly` registration, DESIGN-opus §2
    /// PSBT row): no vault record, `WalletInfo.watch_only = true`, sends are
    /// `send.watch_only`. Returns the wallet id, a digest of the account key
    /// (a seed wallet's id digests its root key, so importing the matching
    /// phrase later adds a separate wallet).
    pub async fn import_watch_only(
        &self,
        xpub: String,
        options: WatchOnlyOptions,
    ) -> Result<String, WalletError> {
        let id = self
            .inner
            .import_watch_only(
                xpub,
                dw_engine::WatchOnlyOptions {
                    name: options.name,
                    birth_height: options.birth_height,
                    lookahead: options.lookahead,
                },
            )
            .await?;
        Ok(id.to_string())
    }

    /// The BIP44 account xpub (IOS-111). Public data, no grant; watch-only
    /// wallets return the xpub they were imported from.
    pub async fn account_xpub(
        &self,
        wallet_id: String,
        account: u32,
    ) -> Result<AccountXpub, WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        let x = self.inner.account_xpub(id, account).await?;
        Ok(AccountXpub {
            account: x.account,
            derivation_path: x.derivation_path,
            xpub: x.xpub,
        })
    }
}
