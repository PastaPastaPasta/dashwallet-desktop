//! Wallet lifecycle (QT-101, QT-114, IOS-009, IOS-110, IOS-111;
//! docs/contracts/m2-engine.md §2.1): dash-qt Open/Close Wallet, the
//! load-on-startup list, watch-only wallets from an account xpub, account
//! xpub export and the per-network data inventory.
//!
//! Closing a wallet unloads it from platform-wallet (in memory only) and
//! marks it in [`crate::store::WalletStore`], whose `load` then leaves it
//! out; opening it clears the mark and runs platform-wallet's idempotent
//! `load_from_persistor`, which registers just the missing wallet from
//! wallet.sqlite, where its scan resumes from its last checkpoint.
//!
//! A watch-only wallet is registered the way platform-wallet registers any
//! wallet (metadata, account xpub and address pools stored, then loaded from
//! the store) with no vault record, so it can never sign.

use std::collections::BTreeSet;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE};
use key_wallet::account::account_collection::AccountCollection;
use key_wallet::account::{Account, AccountType, StandardAccountType};
use key_wallet::bip32::{ChildNumber, ExtendedPubKey};
use key_wallet::wallet::Wallet;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::transaction_building::AccountTypePreference;
use key_wallet::wallet::root_extended_keys::RootExtendedPubKey;
use platform_wallet::changeset::{
    AccountAddressPoolEntry, AccountRegistrationEntry, PlatformWalletChangeSet,
    PlatformWalletPersistence, WalletMetadataEntry,
};

use crate::keys::MAX_LOOKAHEAD;
use crate::platform::runtime::PlatformSignal;
use crate::session::WALLET_DB_FILE;
use crate::wallets::validate_name;
use crate::{DashNetwork, EngineError, EngineEvent, NetworkSession, WalletId};

/// Global setting: dash-qt's load-on-startup list, comma-separated ids.
const STARTUP_LIST_KEY: &str = "load_on_startup";
/// Wallet-scope setting: the account xpub a watch-only wallet came from.
const WATCH_ONLY_XPUB_KEY: &str = "watch_only_xpub";
/// Default gap limit of a watch-only wallet's chains.
const WATCH_ONLY_DEFAULT_LOOKAHEAD: u32 = 30;

/// Load state of one registered wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletLoadState {
    pub wallet_id: WalletId,
    pub name: String,
    pub loaded: bool,
    pub load_on_startup: bool,
    pub watch_only: bool,
}

/// Options of [`NetworkSession::import_watch_only`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchOnlyOptions {
    pub name: Option<String>,
    pub birth_height: Option<u32>,
    pub lookahead: Option<u32>,
}

/// An account-level extended public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountXpub {
    pub account: u32,
    pub derivation_path: String,
    pub xpub: String,
}

/// What the data root holds for one network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDataInfo {
    pub network: DashNetwork,
    pub directory: String,
    pub has_wallet_state: bool,
    pub has_vault: bool,
    pub has_os_store_key: bool,
}

/// The stored load-on-startup list; `None` when absent or empty.
pub(crate) fn read_startup_list(db: &AppDb) -> Result<Option<BTreeSet<WalletId>>, EngineError> {
    let value = db.setting(GLOBAL_SCOPE, STARTUP_LIST_KEY)?;
    let list: BTreeSet<WalletId> = value
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    Ok((!list.is_empty()).then_some(list))
}

fn write_startup_list(db: &AppDb, list: &BTreeSet<WalletId>) -> Result<(), AppDbError> {
    let joined: Vec<String> = list.iter().map(ToString::to_string).collect();
    db.set_setting(GLOBAL_SCOPE, STARTUP_LIST_KEY, Some(&joined.join(",")))
}

/// Parses a BIP44 account xpub for `network`: `xpub` on mainnet, `tpub`
/// elsewhere, depth 3, hardened child number. Returns the key (stamped
/// with `network`) and its account index.
pub(crate) fn parse_account_xpub(
    text: &str,
    network: dashcore::Network,
) -> Result<(ExtendedPubKey, u32), EngineError> {
    let mut xpub = ExtendedPubKey::from_str(text.trim())
        .map_err(|e| EngineError::InvalidXpub(format!("not an extended public key: {e}")))?;
    let mainnet_key = xpub.network == dashcore::Network::Mainnet;
    if mainnet_key != (network == dashcore::Network::Mainnet) {
        return Err(EngineError::InvalidXpub(format!(
            "key is for {:?}, session is {network:?}",
            xpub.network
        )));
    }
    if xpub.depth != 3 {
        return Err(EngineError::InvalidXpub(format!(
            "depth {} is not an account key (m/44'/coin'/account' has depth 3)",
            xpub.depth
        )));
    }
    let account = match xpub.child_number {
        ChildNumber::Hardened { index } => index,
        other => {
            return Err(EngineError::InvalidXpub(format!(
                "child number {other} is not a hardened account index"
            )));
        }
    };
    xpub.network = network;
    Ok((xpub, account))
}

/// The id of a watch-only wallet: key-wallet's network-scoped digest of
/// the account key (a seed wallet's id digests its root key instead).
pub(crate) fn watch_only_wallet_id(xpub: &ExtendedPubKey, network: dashcore::Network) -> WalletId {
    WalletId(Wallet::compute_wallet_id_from_root_extended_pub_key(
        &RootExtendedPubKey::from_extended_pub_key(xpub),
        Some(network),
    ))
}

impl crate::Engine {
    /// Every network with data under the data root (IOS-009), in
    /// `DashNetwork` order, devnets by name. Reads files and the OS secret
    /// store only.
    pub async fn existing_networks(&self) -> Result<Vec<NetworkDataInfo>, EngineError> {
        let root = self.config().data_root.clone();
        let os_store = Arc::clone(&self.config().vault.os_store);
        self.run_blocking(move || -> Result<Vec<NetworkDataInfo>, EngineError> {
            let mut out = Vec::new();
            let mut devnets = Vec::new();
            let entries = match std::fs::read_dir(&root) {
                Ok(e) => e,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
                Err(e) => return Err(e.into()),
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let network = match name.as_str() {
                    "mainnet" => DashNetwork::Mainnet,
                    "testnet" => DashNetwork::Testnet,
                    "regtest" => DashNetwork::Regtest,
                    n => match n.strip_prefix("devnet-") {
                        Some(d) if !d.is_empty() => DashNetwork::Devnet {
                            name: d.to_string(),
                        },
                        _ => continue,
                    },
                };
                if !entry.path().is_dir() {
                    continue;
                }
                let info = inspect_network_dir(&entry.path(), network, &*os_store);
                if info.has_wallet_state || info.has_vault || info.has_os_store_key {
                    match info.network {
                        DashNetwork::Devnet { .. } => devnets.push(info),
                        _ => out.push(info),
                    }
                }
            }
            let rank = |n: &DashNetwork| match n {
                DashNetwork::Mainnet => 0,
                DashNetwork::Testnet => 1,
                DashNetwork::Devnet { .. } => 2,
                DashNetwork::Regtest => 3,
            };
            out.sort_by_key(|i| rank(&i.network));
            devnets.sort_by(|a, b| a.directory.cmp(&b.directory));
            let regtest_at = out
                .iter()
                .position(|i| matches!(i.network, DashNetwork::Regtest))
                .unwrap_or(out.len());
            out.splice(regtest_at..regtest_at, devnets);
            Ok(out)
        })
        .await
    }
}

fn inspect_network_dir(
    dir: &Path,
    network: DashNetwork,
    os_store: &dyn dw_vault::OsSecretStore,
) -> NetworkDataInfo {
    let vault = dw_vault::inspect_vault_dir(&dir.join(dw_vault::VAULT_DIR), os_store);
    NetworkDataInfo {
        network,
        directory: dir.to_string_lossy().into_owned(),
        has_wallet_state: dir.join(WALLET_DB_FILE).is_file(),
        has_vault: vault.has_vault,
        has_os_store_key: vault.has_os_store_key,
    }
}

impl NetworkSession {
    /// Every registered wallet with its load state, in the order wallets
    /// were added. In-memory read.
    pub fn wallet_load_states(&self) -> Result<Vec<WalletLoadState>, EngineError> {
        let _op = self.try_enter()?;
        let live = self.live()?;
        let loaded: BTreeSet<[u8; 32]> = live
            .manager
            .list_wallet_ids_blocking()
            .into_iter()
            .collect();
        let startup = self
            .startup_list
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut ids: Vec<WalletId> = loaded
            .iter()
            .copied()
            .chain(live.store.unloaded())
            .map(WalletId)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let order = |w: &WalletId| self.hub.name_of(w).map(|n| n.order);
        ids.sort_by_key(|w| (order(w).is_none(), order(w), *w));
        Ok(ids
            .into_iter()
            .map(|id| WalletLoadState {
                name: self
                    .hub
                    .name_of(&id)
                    .map(|n| n.name)
                    .unwrap_or_else(|| format!("Wallet {}", &id.to_string()[..8])),
                loaded: loaded.contains(&id.0),
                load_on_startup: startup.as_ref().is_none_or(|l| l.contains(&id)),
                watch_only: !self.vault.has_wallet_secret(&id.0),
                wallet_id: id,
            })
            .collect())
    }

    /// dash-qt "Close Wallet": unloads a loaded wallet. Its data stays; its
    /// prepared, unsent payments lose their reservations. Idempotent.
    pub async fn unload_wallet(self: &Arc<Self>, id: WalletId) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            if live.store.is_unloaded(&id.0) {
                return Ok(());
            }
            this.require_wallet(&id)?;
            // Marked first, so a concurrent load cannot bring it back.
            live.store.set_unloaded(id.0, true);
            // Its discoveries end first, as for a removal: none may apply
            // what it finds to the wallet once it is loaded again.
            this.platform.recovery.end_discoveries(id).await;
            if let Err(e) = live.manager.remove_wallet(&id.0).await {
                live.store.set_unloaded(id.0, false);
                this.platform.recovery.resume_discoveries(id);
                return Err(e.into());
            }
            this.hub.unload_wallet(&id);
            this.spends.forget_wallet(&id);
            this.platform.forget(&id);
            this.sink.emit(EngineEvent::WalletLoadChanged {
                network: this.network.clone(),
                wallet_id: id,
                loaded: false,
            });
            Ok(())
        })
        .await
    }

    /// dash-qt "Open Wallet": loads a registered, unloaded wallet from
    /// wallet.sqlite; its scan resumes from its last checkpoint. Idempotent.
    pub async fn load_wallet(self: &Arc<Self>, id: WalletId) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            if !live.store.is_unloaded(&id.0) {
                return this.require_wallet(&id);
            }
            live.store.set_unloaded(id.0, false);
            if let Err(e) = live.manager.load_from_persistor().await {
                live.store.set_unloaded(id.0, true);
                return Err(e.into());
            }
            if live.manager.get_wallet(&id.0).await.is_none() {
                live.store.set_unloaded(id.0, true);
                return Err(EngineError::WalletNotFound(id.to_string()));
            }
            this.refresh_wallet_state(&live.manager, id).await;
            this.load_identity_choices(id).await;
            this.load_history_for(vec![id]).await?;
            this.hub.pump.mark_history(id, None);
            this.hub.pump.mark_balances(id);
            this.sink.emit(EngineEvent::WalletLoadChanged {
                network: this.network.clone(),
                wallet_id: id,
                loaded: true,
            });
            this.platform.signal(PlatformSignal::WalletAdded(id));
            Ok(())
        })
        .await
    }

    /// Adds or removes the wallet from the load-on-startup list (dash-qt's
    /// settings.json `"wallet"` list), stored in app.sqlite.
    pub async fn set_load_on_startup(
        self: &Arc<Self>,
        id: WalletId,
        load_on_startup: bool,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            if !live.store.is_unloaded(&id.0) {
                this.require_wallet(&id)?;
            }
            let registered: BTreeSet<WalletId> = live
                .manager
                .list_wallet_ids_blocking()
                .into_iter()
                .chain(live.store.unloaded())
                .map(WalletId)
                .collect();
            let mut list = this
                .startup_list
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                // No list yet: every wallet loads, so all are on it.
                .unwrap_or_else(|| registered.clone());
            list.retain(|w| registered.contains(w));
            if load_on_startup {
                list.insert(id);
            } else {
                list.remove(&id);
            }
            let stored = list.clone();
            this.appdb_op(move |db| write_startup_list(db, &stored))
                .await?;
            *this.startup_list.lock().unwrap_or_else(|p| p.into_inner()) =
                (!list.is_empty()).then_some(list);
            Ok(())
        })
        .await
    }

    /// The BIP44 account xpub (IOS-111). Public data, no grant. A
    /// watch-only wallet returns the key it was imported from.
    pub async fn account_xpub(
        self: &Arc<Self>,
        id: WalletId,
        account: u32,
    ) -> Result<AccountXpub, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            this.require_wallet(&id)?;
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let wallet = wm
                .get_wallet(&id.0)
                .ok_or_else(|| EngineError::WalletNotFound(id.to_string()))?;
            let acct = wallet
                .accounts
                .standard_bip44_accounts
                .get(&account)
                .ok_or_else(|| {
                    EngineError::InvalidArgument(format!("BIP44 account {account} is not derived"))
                })?;
            let path = acct
                .derivation_path()
                .map_err(|e| EngineError::Internal(format!("derivation path: {e}")))?;
            Ok(AccountXpub {
                account,
                derivation_path: path.to_string(),
                xpub: acct.account_xpub.to_string(),
            })
        })
        .await
    }

    /// Registers a watch-only wallet from a BIP44 account xpub (QT-114):
    /// no vault record, so sends fail with `send.watch_only`. Returns its
    /// id.
    pub async fn import_watch_only(
        self: &Arc<Self>,
        xpub: String,
        options: WatchOnlyOptions,
    ) -> Result<WalletId, EngineError> {
        let network = self.network.core_network();
        let (key, account) = parse_account_xpub(&xpub, network)?;
        let name = options.name.as_deref().map(validate_name).transpose()?;
        let lookahead = options.lookahead.unwrap_or(WATCH_ONLY_DEFAULT_LOOKAHEAD);
        if !(1..=MAX_LOOKAHEAD).contains(&lookahead) {
            return Err(EngineError::InvalidArgument(format!(
                "lookahead {lookahead} outside 1..={MAX_LOOKAHEAD}"
            )));
        }
        let birth_height = options.birth_height.unwrap_or(0);
        let id = watch_only_wallet_id(&key, network);
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            if live.store.is_unloaded(&id.0) || live.manager.get_wallet(&id.0).await.is_some() {
                return Err(EngineError::WalletAlreadyExists(id.to_string()));
            }
            let account_type = AccountType::Standard {
                index: account,
                standard_account_type: StandardAccountType::BIP44Account,
            };
            let changeset = watch_only_registration(id, account_type, key, network, birth_height)?;
            let store = Arc::clone(&live.store);
            tokio::task::spawn_blocking(move || {
                store.store(id.0, changeset)?;
                store.flush(id.0)
            })
            .await?
            .map_err(|e| EngineError::Storage(format!("watch-only registration: {e}")))?;
            live.manager.load_from_persistor().await?;
            let wallet = live
                .manager
                .get_wallet(&id.0)
                .await
                .ok_or_else(|| EngineError::Internal(format!("watch-only wallet {id} did not load")))?;
            if lookahead != WATCH_ONLY_DEFAULT_LOOKAHEAD
                && let Err(e) = wallet
                    .core()
                    .set_gap_limit(AccountTypePreference::BIP44, account, lookahead)
                    .await
            {
                tracing::warn!(wallet_id = %id, error = %e, "could not set the watch-only lookahead");
            }
            this.refresh_wallet_state(&live.manager, id).await;
            let default_name = this.next_default_name();
            if let Err(e) = this
                .store_name(id, name.unwrap_or(default_name), None)
                .await
            {
                tracing::warn!(wallet_id = %id, error = %e, "could not store the wallet name");
            }
            let scope = id.to_string();
            let text = key.to_string();
            if let Err(e) = this
                .appdb_op(move |db| db.set_setting(&scope, WATCH_ONLY_XPUB_KEY, Some(&text)))
                .await
            {
                tracing::warn!(wallet_id = %id, error = %e, "could not store the watch-only xpub");
            }
            this.sink.emit(EngineEvent::WalletCreated {
                network: this.network.clone(),
                wallet_id: id,
            });
            Ok(id)
        })
        .await
    }
}

/// The changeset platform-wallet's own registration stores for a wallet
/// (metadata, account xpub, address pools), for one watch-only BIP44
/// account. Loading it back rebuilds the wallet external-signable from the
/// xpub, as every wallet is rebuilt at open.
fn watch_only_registration(
    id: WalletId,
    account_type: AccountType,
    xpub: ExtendedPubKey,
    network: dashcore::Network,
    birth_height: u32,
) -> Result<PlatformWalletChangeSet, EngineError> {
    let account = Account::from_xpub(Some(id.0), account_type, xpub, network)
        .map_err(|e| EngineError::InvalidXpub(e.to_string()))?;
    let mut accounts = AccountCollection::new();
    accounts
        .insert(account)
        .map_err(|e| EngineError::Internal(format!("account collection: {e}")))?;
    let wallet = Wallet::new_watch_only(network, id.0, accounts);
    let info = ManagedWalletInfo::from_wallet(&wallet, birth_height);
    let mut changeset = PlatformWalletChangeSet {
        wallet_metadata: Some(WalletMetadataEntry {
            network,
            // Watch-only wallets have no root key: platform-wallet falls back
            // to the wallet id (a group of one).
            wallet_group_id: id.0,
            birth_height,
        }),
        account_registrations: vec![AccountRegistrationEntry {
            account_type,
            account_xpub: xpub,
        }],
        ..Default::default()
    };
    for managed in info.all_managed_accounts() {
        let account_type = managed.managed_account_type().to_account_type();
        for pool in managed.managed_account_type().address_pools() {
            let addresses: Vec<key_wallet::AddressInfo> =
                pool.addresses.values().cloned().collect();
            if addresses.is_empty() {
                continue;
            }
            changeset
                .account_address_pools
                .push(AccountAddressPoolEntry {
                    account_type,
                    pool_type: pool.pool_type,
                    addresses,
                });
        }
    }
    Ok(changeset)
}

#[cfg(test)]
mod tests {
    use super::*;

    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

    /// The public key at `path` of a fixed test seed, on testnet.
    fn tpub_at(path: &str) -> String {
        let secp = dashcore::secp256k1::Secp256k1::new();
        let master = ExtendedPrivKey::new_master(dashcore::Network::Testnet, &[7u8; 64]).unwrap();
        let path = DerivationPath::from_str(path).unwrap();
        let xprv = master.derive_priv(&secp, &path).unwrap();
        ExtendedPubKey::from_priv(&secp, &xprv).to_string()
    }

    #[test]
    fn test_qt_114_account_xpub_rules() {
        let net = dashcore::Network::Testnet;
        assert!(matches!(
            parse_account_xpub("nonsense", net),
            Err(EngineError::InvalidXpub(_))
        ));
        // Not an account key: depth 4, and depth 3 with a normal index.
        assert!(matches!(
            parse_account_xpub(&tpub_at("m/44'/1'/0'/0"), net),
            Err(EngineError::InvalidXpub(_))
        ));
        assert!(matches!(
            parse_account_xpub(&tpub_at("m/44'/1'/2"), net),
            Err(EngineError::InvalidXpub(_))
        ));
        let account_key = tpub_at("m/44'/1'/2'");
        assert!(account_key.starts_with("tpub"));
        match parse_account_xpub(&account_key, net) {
            Ok((key, account)) => {
                assert_eq!(key.depth, 3);
                assert_eq!(account, 2);
                // The same key is refused on mainnet.
                assert!(matches!(
                    parse_account_xpub(&account_key, dashcore::Network::Mainnet),
                    Err(EngineError::InvalidXpub(_))
                ));
                // A watch-only id never collides across networks.
                assert_ne!(
                    watch_only_wallet_id(&key, net),
                    watch_only_wallet_id(&key, dashcore::Network::Regtest)
                );
            }
            Err(e) => panic!("{e}"),
        }
    }
}
