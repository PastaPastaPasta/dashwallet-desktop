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
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::wallet::root_extended_keys::RootExtendedPubKey;
use platform_wallet::changeset::{
    AccountAddressPoolEntry, AccountRegistrationEntry, PlatformWalletChangeSet,
    PlatformWalletPersistence, WalletMetadataEntry,
};

use crate::gate::Lifecycle;
use crate::keys::MAX_LOOKAHEAD;
use crate::platform::runtime::PlatformSignal;
use crate::session::WALLET_DB_FILE;
use crate::wallets::validate_name;
use crate::{DashNetwork, EngineError, EngineEvent, NetworkSession, WalletId};

/// Global setting: dash-qt's load-on-startup list, comma-separated ids.
const STARTUP_LIST_KEY: &str = "load_on_startup";
/// The stored list that loads no wallet. An absent value is no list (every
/// wallet loads); an empty one, as earlier builds stored an emptied list,
/// loads no wallet, so a start with wallets repairs it.
const NOTHING_ON_STARTUP: &str = "none";
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

/// The stored load-on-startup list; `None` when absent (no list yet). An
/// empty value (an earlier build's emptied list) or [`NOTHING_ON_STARTUP`]
/// is an empty list, which a start with wallets repairs.
pub(crate) fn read_startup_list(db: &AppDb) -> Result<Option<BTreeSet<WalletId>>, EngineError> {
    let Some(value) = db.setting(GLOBAL_SCOPE, STARTUP_LIST_KEY)? else {
        return Ok(None);
    };
    if value == NOTHING_ON_STARTUP {
        return Ok(Some(BTreeSet::new()));
    }
    Ok(Some(
        value
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse().ok())
            .collect(),
    ))
}

fn startup_list_value(list: &BTreeSet<WalletId>) -> String {
    let joined: Vec<String> = list.iter().map(ToString::to_string).collect();
    if joined.is_empty() {
        NOTHING_ON_STARTUP.to_string()
    } else {
        joined.join(",")
    }
}

pub(crate) fn write_startup_list(db: &AppDb, list: &BTreeSet<WalletId>) -> Result<(), AppDbError> {
    db.set_setting(
        GLOBAL_SCOPE,
        STARTUP_LIST_KEY,
        Some(&startup_list_value(list)),
    )
}

/// Deletes a removed wallet's app rows and stores `list` as the
/// load-on-startup list in the same transaction.
pub(crate) fn delete_wallet_listing(
    db: &AppDb,
    id: WalletId,
    list: Option<&BTreeSet<WalletId>>,
) -> Result<(), AppDbError> {
    let value = list.map(startup_list_value);
    db.delete_wallet_and_set(
        &id.to_string(),
        value
            .as_deref()
            .map(|v| (GLOBAL_SCOPE, STARTUP_LIST_KEY, v)),
    )
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
    /// `DashNetwork` order, devnets by name, each in its folder
    /// ([`Self::network_dir`]): a network that started over is listed by its
    /// new folder, and the folder it left is not listed. Reads files and the
    /// OS secret store only.
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
            let redirects = crate::network_folders::redirects(&root)?;
            let mut names: BTreeSet<String> = entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !redirects.values().any(|folder| folder == name))
                .collect();
            names.extend(redirects.keys().cloned());
            for name in names {
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
                let dir = root.join(redirects.get(&name).unwrap_or(&name));
                if !dir.is_dir() {
                    continue;
                }
                let info = inspect_network_dir(&dir, network, &*os_store);
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
    /// `ShieldedSpendUnresolved` while it holds a shielded spend whose
    /// result is not known (DEC-1046).
    pub async fn unload_wallet(self: &Arc<Self>, id: WalletId) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let live = this.live()?;
            if live.store.is_unloaded(&id.0) {
                return Ok(());
            }
            this.require_wallet(&id)?;
            let synced = {
                let wm = live.manager.wallet_manager_arc();
                let wm = wm.read().await;
                (wm.get_wallet_info(&id.0)).map(|info| info.core_wallet.synced_height())
            };
            // A shielded call of the wallet in flight (a send, an unshield,
            // a shield) ends first and none starts until the unload is
            // done: a spend's library call outlives its permit's drain, and
            // may store its transition up to its end.
            let _shielded = crate::platform::shielded_pool::wallet_lock(&this, id)
                .lock_owned()
                .await;
            // DEC-1046: unloading a wallet that holds a shielded spend of
            // unknown result would delete its stored transition (no detach
            // at the pin); refused before anything of it changes, and again
            // at the removal (fail closed).
            if crate::platform::shielded_pool::holds_unresolved(
                &this.leases,
                &live.manager,
                &live.appdb,
                id,
            )
            .await
            {
                return Err(EngineError::ShieldedSpendUnresolved);
            }
            // E0-04 §8.6: the wallet's leases end and its permits drain.
            let _barrier = this
                .revoke_wallet(id, crate::platform::RevokeCause::WalletClosed)
                .await;
            // Its next load publishes narrow CoinJoin pools again; what SPV
            // matches above this checkpoint before they are widened is
            // replayed, across a crash too. Put back if it stays loaded; it
            // stays loaded if that cannot be stored.
            let noted = async {
                let marked = this.wide_scan(id).await?;
                if let Some(synced) = synced {
                    this.note_narrow_from(id, Some(synced), None).await?;
                }
                Ok::<_, EngineError>(marked)
            }
            .await;
            let marked = match noted {
                Ok(marked) => marked,
                Err(e) => {
                    this.spawn_ensure_background();
                    return Err(e);
                }
            };
            // Marked first, so a concurrent load cannot bring it back.
            live.store.set_unloaded(id.0, true);
            // Its discoveries end first, as for a removal: none may apply
            // what it finds to the wallet once it is loaded again.
            let _discoveries = this.platform.recovery.end_discoveries(id).await;
            if let Err(e) = (this.platform)
                .remove_from_manager(&this.leases, &live.manager, &live.appdb, id)
                .await
            {
                live.store.set_unloaded(id.0, false);
                this.set_wide_scan(id, marked).await;
                // The revocation dropped its background lease, without
                // which every shielded pass of the network is held back.
                this.spawn_ensure_background();
                return Err(e);
            }
            #[cfg(test)]
            let pause = this.hub.pause_unload.lock().unwrap().take();
            #[cfg(test)]
            if let Some(pause) = pause {
                pause
                    .reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                pause.release.notified().await;
            }
            this.hub.unload_wallet(&id);
            this.hub.restore.forget_wallet_checks(id, false);
            this.spends.forget_wallet(&id);
            this.coin_fences.forget(&id);
            this.platform.forget(&id);
            this.leases.wallet_closed(id);
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
            // A wallet loads with its stored shielded spends: not between an
            // in-app close's check and its commit (E1 r2 R2-3).
            let _admitted = this.platform.wallet_loads.read().await;
            let _op = this.enter().await?;
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let live = this.live()?;
            if !live.store.is_unloaded(&id.0) {
                return this.require_wallet(&id);
            }
            // The wallet joins the shielded passes' set: not while one is in
            // flight (R1-3, `PlatformRuntime::shielded_passes`).
            let passes = this.platform.shielded_passes.write().await;
            // Spends wait from before the wallet is published until its
            // stored replay is resumed (`WalletState::loading`); a store
            // SPV commits for it meanwhile ends no record
            // (`ReplayRecords::committed`).
            // Read first so they are restored as soon as the wallet is
            // registered, not after the history load (#88).
            {
                let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
                let db_path = this.data_dir().join(crate::session::WALLET_DB_FILE);
                let holds = tokio::task::spawn_blocking(move || {
                    crate::tx_actions::finish_marked_abandons(&persister, &appdb, &db_path, &[id])
                })
                .await??;
                this.hub.note_dropped_holds(holds);
            }
            let stored = this.stored_spends(id).await?;
            this.hub.admit(id);
            this.hub.update_wallet_state(id, |s| s.loading = true);
            // A wallet the session opened unloaded (not on the startup list)
            // has had no unload to note its checkpoint.
            // Not published unless that is stored.
            let noted = async {
                let db_path = this.data_dir().join(crate::session::WALLET_DB_FILE);
                let (synced, birth) = tokio::task::spawn_blocking(move || {
                    crate::store::stored_heights(&db_path, &id.0)
                })
                .await?
                .map_err(|e| EngineError::Storage(format!("the stored checkpoint: {e}")))?;
                // An unfinished replay's start too: SPV may finish it with
                // narrow pools. A resume lowers the birth to it; a cancelled
                // one's hold comes back no lower than the birth's checkpoint.
                // One that cannot be read may start anywhere: from the birth.
                let floor = birth.map_or(0, |b| b.saturating_sub(1));
                let checkpoint = match live.replay.stored(id).await {
                    Ok(record) => (synced.into_iter())
                        .chain(record.map(|r| if r.cancelled { r.reached.max(floor) } else { r.reached }))
                        .min(),
                    Err(e) => {
                        tracing::warn!(wallet_id = %id, error = %e, "could not read the wallet's replay record; marking it from its birth");
                        None
                    }
                };
                (this.note_narrow_from(id, checkpoint, birth.map(|b| b.saturating_sub(1)))).await
            }
            .await;
            if let Err(e) = noted {
                this.hub.unload_wallet(&id);
                return Err(e);
            }
            let in_flight = live.manager.platform_address_sync_arc().is_syncing();
            this.hub.restore.forget_wallet_checks(id, in_flight);
            live.store.set_unloaded(id.0, false);
            if let Err(e) = live.manager.load_from_persistor().await {
                live.store.set_unloaded(id.0, true);
                this.hub.unload_wallet(&id);
                return Err(e.into());
            }
            #[cfg(test)]
            let pause = this.hub.pause_claims.lock().unwrap().take();
            #[cfg(test)]
            if let Some(pause) = pause {
                pause
                    .reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                pause.release.notified().await;
            }
            this.apply_stored_spends(&live.manager, id, stored).await;
            this.admit_loaded(&live.manager, id).await;
            if live.manager.get_wallet(&id.0).await.is_none() {
                live.store.set_unloaded(id.0, true);
                this.hub.unload_wallet(&id);
                return Err(EngineError::WalletNotFound(id.to_string()));
            }
            drop(passes);
            #[cfg(test)]
            if this.hub.run_on_published(id) {
                return Err(EngineError::Internal("test: quit after the publish".into()));
            }
            // Its replay was noted before the publish; a failure here is
            // logged with it.
            let _ = this.restore_loaded_wallet(&live.manager, id).await;
            #[cfg(test)]
            let pause = this.hub.pause_load.lock().unwrap().take();
            #[cfg(test)]
            if let Some(pause) = pause {
                pause
                    .reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                pause.release.notified().await;
            }
            this.resume_replay(&live.manager, id).await;
            // Before spends are let in (refused meanwhile, as is every
            // build): a coin read holds back a collateral-sized coin whose
            // transaction the history doesn't know.
            let history = this.load_history_for(vec![id]).await;
            // A history that did not load is not whole: no drop reads it as
            // such once the open is over.
            if history.is_err() {
                (this.hub.history).with_wallet(id, |h| h.load_incomplete = true);
            }
            this.hub.update_wallet_state(id, |s| s.loading = false);
            // The drops that waited for this open go now (#818, S1).
            this.drain_deferred_drops(&live.manager).await;
            this.refresh_wallet_state(&live.manager, id).await;
            this.load_identity_choices(id).await;
            this.load_payment_locks(id).await;
            // Its contacts' profiles are fetched on its next pass (DP4-01 D12).
            this.expire_contact_profiles(&live.manager, id).await;
            history?;
            this.hub.pump.mark_history(id, None);
            this.hub.pump.mark_balances(id);
            this.sink.emit(EngineEvent::WalletLoadChanged {
                network: this.network.clone(),
                wallet_id: id,
                loaded: true,
            });
            this.platform.signal(PlatformSignal::WalletAdded(id));
            // E0-04 §8.6: H6 runs again for the reopened wallet.
            this.leases.wallet_opened(id);
            Ok(())
        })
        .await
    }

    /// Adds or removes the wallet from the load-on-startup list (dash-qt's
    /// settings.json `"wallet"` list), stored in app.sqlite.
    /// `LastStartupWallet`, nothing changed, when the removal would leave no
    /// registered wallet on the list.
    pub async fn set_load_on_startup(
        self: &Arc<Self>,
        id: WalletId,
        load_on_startup: bool,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // Not between a new wallet's registration and its listing.
            let _lifecycle = this.wallet_lifecycle.hold().await;
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
                if list.is_empty() {
                    return Err(EngineError::LastStartupWallet);
                }
            }
            let stored = list.clone();
            this.appdb_op(move |db| write_startup_list(db, &stored))
                .await?;
            *this.startup_list.lock().unwrap_or_else(|p| p.into_inner()) = Some(list);
            Ok(())
        })
        .await
    }

    /// The load-on-startup list once `removed` is gone: without it and
    /// wallets that no longer exist, and with the first open wallet in
    /// `wallet_infos` order (the one the app shows next) when nothing else
    /// would be on it. With no list yet, `None` while a wallet remains, and
    /// "nothing to load" once none does.
    pub(crate) fn startup_list_after_removal(
        &self,
        live: &crate::session::Live,
        removed: WalletId,
    ) -> Option<BTreeSet<WalletId>> {
        let cached = self
            .startup_list
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut open: Vec<WalletId> = (live.manager.list_wallet_ids_blocking().into_iter())
            .map(WalletId)
            .filter(|w| *w != removed)
            .collect();
        let order = |w: &WalletId| self.hub.name_of(w).map(|n| n.order);
        open.sort_by_key(|w| (order(w).is_none(), order(w), *w));
        let mut closed: Vec<WalletId> = live.store.unloaded().into_iter().map(WalletId).collect();
        closed.sort();
        let Some(mut list) = cached else {
            return (open.is_empty() && closed.is_empty()).then(BTreeSet::new);
        };
        list.retain(|w| *w != removed && (open.contains(w) || closed.contains(w)));
        if list.is_empty()
            && let Some(next) = open.first().or(closed.first())
        {
            list.insert(*next);
        }
        Some(list)
    }

    /// Stores `list` as the load-on-startup list on its own; a failure is
    /// logged only.
    pub(crate) async fn store_startup_list(
        &self,
        live: &crate::session::Live,
        list: &BTreeSet<WalletId>,
    ) {
        let (appdb, stored) = (Arc::clone(&live.appdb), list.clone());
        #[cfg(test)]
        let fail =
            crate::keys::take_test_failure(&crate::wallets::FAIL_LISTING_RETRY_IN, self.data_dir());
        let written = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if fail {
                return Err(AppDbError::Corrupt(
                    "test: load-on-startup write failed".into(),
                ));
            }
            write_startup_list(&appdb, &stored)
        })
        .await
        .map_err(|e| e.to_string())
        .and_then(|w| w.map_err(|e| e.to_string()));
        if let Err(e) = written {
            tracing::warn!(error = %e, "could not store the load-on-startup list");
        }
    }

    /// Puts a wallet registered here on the load-on-startup list (`listed`),
    /// when there is one, so it loads at the next start whatever was closed
    /// before it; or takes off one a restore's rollback removed. A failed
    /// write is logged only and the wallet's registration stands. The list
    /// in memory then changes only when it named no wallet: the stored one
    /// names none either, so the next start opens every wallet, this one
    /// included. Otherwise it stays as stored.
    pub(crate) async fn list_on_startup(&self, _held: &Lifecycle<'_>, id: WalletId, listed: bool) {
        let Some(mut list) = self
            .startup_list
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        else {
            return;
        };
        let named_none = list.is_empty();
        let changed = if listed {
            list.insert(id)
        } else {
            list.remove(&id)
        };
        if !changed {
            return;
        }
        let stored = list.clone();
        #[cfg(test)]
        let fail =
            crate::keys::take_test_failure(&crate::wallets::FAIL_LISTING_RETRY_IN, self.data_dir());
        let written = self
            .appdb_op(move |db| {
                #[cfg(test)]
                if fail {
                    return Err(AppDbError::Corrupt(
                        "test: load-on-startup write failed".into(),
                    ));
                }
                write_startup_list(db, &stored)
            })
            .await;
        if let Err(e) = &written {
            tracing::warn!(wallet_id = %id, error = %e, "could not update the load-on-startup list");
        }
        if written.is_ok() || named_none {
            *self.startup_list.lock().unwrap_or_else(|p| p.into_inner()) = Some(list);
        }
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
    /// id. Refused (`Storage`) while the dispatch journal is unavailable,
    /// and `RestoreOnly` while the network's RESTORED mark is set.
    pub async fn import_watch_only(
        self: &Arc<Self>,
        xpub: String,
        options: WatchOnlyOptions,
    ) -> Result<WalletId, EngineError> {
        crate::store_upgrade::refuse_while_restored(self.data_dir())?;
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
            let held = this.wallet_lifecycle.hold().await;
            let live = this.live()?;
            this.require_journal()?;
            if live.store.is_unloaded(&id.0) || live.manager.get_wallet(&id.0).await.is_some() {
                return Err(EngineError::WalletAlreadyExists(id.to_string()));
            }
            let account_type = AccountType::Standard {
                index: account,
                standard_account_type: StandardAccountType::BIP44Account,
            };
            let changeset = watch_only_registration(id, account_type, key, network, birth_height)?;
            // DEC-210: before the first write, as an import of a seed.
            let readmit = id.to_string();
            let readmitted = this.appdb_op(move |db| db.readmit_wallet(&readmit)).await?;
            // Before SPV can match a batch with the pools it publishes: the
            // lookahead it is to scan with and the marker, put back if
            // nothing is registered.
            let pending = async {
                let previous = this.wide_scan(id).await?;
                this.mark_pending(id, crate::keys::WideScan::Birth).await?;
                // A row an earlier attempt left does not narrow this one.
                let previous_gap = this.lookahead_row(id).await?;
                if lookahead == WATCH_ONLY_DEFAULT_LOOKAHEAD {
                    if !this.stored_registration(id).await? {
                        this.set_lookahead_row(id, None).await?;
                    }
                } else {
                    this.write_lookahead(id, lookahead).await?;
                }
                Ok::<_, EngineError>((previous, previous_gap))
            }
            .await;
            let (previous, previous_gap) = match pending {
                Ok(previous) => previous,
                Err(e) => {
                    if readmitted {
                        this.record_removed_again(&held, id).await;
                    }
                    return Err(e);
                }
            };
            let was_unloaded = this.hub.admit(id);
            let in_flight = live.manager.platform_address_sync_arc().is_syncing();
            this.hub.restore.forget_wallet_checks(id, in_flight);
            let registered = async {
                let store = Arc::clone(&live.store);
                let refused = crate::keys::registration_refused(this.data_dir());
                let stored = tokio::task::spawn_blocking(move || {
                    if refused {
                        return Err("test: registration refused".to_owned());
                    }
                    store
                        .store(id.0, changeset)
                        .and_then(|()| store.flush(id.0))
                        .map_err(|e| e.to_string())
                })
                .await?;
                if let Err(e) = stored {
                    // Not registered: removed again, as a failed seed import,
                    // with the lookahead as it was.
                    this.set_wide_scan(id, previous).await;
                    if let Err(e) = this.set_lookahead_row(id, previous_gap).await {
                        tracing::warn!(wallet_id = %id, error = %e, "could not put the lookahead back");
                    }
                    if readmitted {
                        this.record_removed_again(&held, id).await;
                    }
                    return Err(EngineError::Storage(format!("watch-only registration: {e}")));
                }
                live.manager.load_from_persistor().await?;
                this.admit_loaded(&live.manager, id).await;
                live.manager
                    .get_wallet(&id.0)
                    .await
                    .ok_or_else(|| EngineError::Internal(format!("watch-only wallet {id} did not load")))
            }
            .await;
            let wallet = match registered {
                Ok(wallet) => wallet,
                Err(e) => {
                    this.unadmit(&live.manager, id, was_unloaded).await;
                    return Err(e);
                }
            };
            #[cfg(test)]
            this.hub.run_on_published(id);
            let mut raised = true;
            if lookahead != WATCH_ONLY_DEFAULT_LOOKAHEAD
                && let Err(e) = crate::keys::apply_lookahead(&wallet, lookahead, false).await
            {
                raised = false;
                tracing::warn!(wallet_id = %id, error = %e, "could not set the watch-only lookahead");
            }
            // A batch SPV committed since the load published it matched the
            // default gap.
            let published_at = match live.store.loaded_checkpoint(&id.0) {
                Some(checkpoint) => checkpoint,
                None => crate::keys::birth_checkpoint(&live.manager, id).await,
            };
            if this.widen_published_wallet(&live.manager, id, published_at).await && raised {
                this.set_wide_scan(id, Some(crate::keys::WideScan::Done)).await;
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
            this.list_on_startup(&held, id, true).await;
            this.load_added_wallet(id).await;
            this.sink.emit(EngineEvent::WalletCreated {
                network: this.network.clone(),
                wallet_id: id,
            });
            Ok(id)
        })
        .await
    }
}

impl NetworkSession {
    /// Admits the wallets other than `id` a load from the store brought
    /// back into `manager` ([`crate::events::SessionHub::admit_loaded`]);
    /// `id`, the wallet being loaded, was admitted before.
    pub(crate) async fn admit_loaded(&self, manager: &crate::session::Manager, id: WalletId) {
        let ids = manager.wallet_ids().await;
        (self.hub).admit_loaded(ids.into_iter().map(WalletId).filter(|w| *w != id));
    }

    /// A registration of `id` that failed after
    /// [`crate::events::SessionHub::admit`]: a wallet unloaded or removed
    /// before (`was_unloaded`) is again, unless `manager` holds it.
    pub(crate) async fn unadmit(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        was_unloaded: bool,
    ) {
        if was_unloaded && manager.get_wallet(&id.0).await.is_none() {
            self.hub.unload_wallet(&id);
        }
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
                pool.addresses().values().cloned().collect();
            if addresses.is_empty() {
                continue;
            }
            changeset
                .account_address_pools
                .push(AccountAddressPoolEntry {
                    account_type,
                    pool_type: pool.pool_type(),
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
        let master = ExtendedPrivKey::new_master(dashcore::Network::Testnet, &[7u8; 64]).unwrap();
        let path = DerivationPath::from_str(path).unwrap();
        let xprv = master.derive_priv(&path).unwrap();
        ExtendedPubKey::from_priv(&xprv).to_string()
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
