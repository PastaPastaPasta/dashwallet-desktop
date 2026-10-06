//! M2 wallet lifecycle: load/unload (dash-qt Open/Close wallet), the
//! load-on-startup list, watch-only wallets from an xpub, account xpub
//! export and the per-network data inventory. Owner: R1 (engine-tools).
//! Contract: docs/contracts/m2-engine.md §2.1.

use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
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

#[uniffi::export]
impl Engine {
    /// Every network that has data under the data root, in `DashNetwork`
    /// order (devnets by name). Reads the file system and the OS secret
    /// store only; opens nothing. Owner R1.
    pub async fn existing_networks(&self) -> Result<Vec<NetworkDataInfo>, EngineError> {
        Err(EngineError::NotImplemented {
            detail: "Engine.existing_networks".into(),
        })
    }
}

#[uniffi::export]
impl NetworkSession {
    /// Every registered wallet with its load state, in creation order.
    /// In-memory read.
    pub fn wallet_load_states(&self) -> Result<Vec<WalletLoadState>, WalletError> {
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.wallet_load_states")
    }

    /// dash-qt "Open Wallet": loads a registered, unloaded wallet and resumes
    /// its scan from its last processed height. Idempotent. Emits
    /// `WalletLoadChanged{loaded: true}`.
    pub async fn load_wallet(&self, wallet_id: String) -> Result<(), WalletError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.load_wallet")
    }

    /// dash-qt "Close Wallet": stops tracking the wallet and drops it from
    /// memory; its data, vault records and app metadata stay. Prepared,
    /// unsent payments of the wallet are abandoned. Idempotent. Emits
    /// `WalletLoadChanged{loaded: false}`.
    pub async fn unload_wallet(&self, wallet_id: String) -> Result<(), WalletError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.unload_wallet")
    }

    /// Adds or removes the wallet from the load-on-startup list (dash-qt adds
    /// on Create/Open/Restore and removes on Close; the host calls this with
    /// the same rule). Persisted in dw-appdb.
    pub async fn set_load_on_startup(
        &self,
        wallet_id: String,
        load_on_startup: bool,
    ) -> Result<(), WalletError> {
        let _ = load_on_startup;
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.set_load_on_startup")
    }

    /// Registers a watch-only wallet from a BIP44 account xpub (QT-114;
    /// interim for platform-wallet `WatchOnly` registration, DESIGN-opus §2
    /// PSBT row): no vault record, `WalletInfo.watch_only = true`, sends are
    /// `send.watch_only` and PSBTs are created unsigned. Returns the wallet
    /// id. A later `import_wallet` of the matching phrase attaches the keys.
    pub async fn import_watch_only(
        &self,
        xpub: String,
        options: WatchOnlyOptions,
    ) -> Result<String, WalletError> {
        let _ = (xpub, options);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.import_watch_only")
    }

    /// The BIP44 account xpub (IOS-111). Public data, no grant; watch-only
    /// wallets return the xpub they were imported from.
    pub async fn account_xpub(
        &self,
        wallet_id: String,
        account: u32,
    ) -> Result<AccountXpub, WalletError> {
        let _ = account;
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.account_xpub")
    }
}
