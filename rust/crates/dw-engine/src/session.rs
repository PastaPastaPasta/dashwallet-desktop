use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};

use dash_sdk::SdkBuilder;
use dash_sdk::sdk::AddressList;
use dash_spv::{ClientConfig, DevnetConfig};
use dw_appdb::{APP_DB_FILE, AppDb};
use dw_vault::{VAULT_DIR, Vault, VaultConfig};
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletManager;
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::context::{LazyTrustedContext, SharedContext};
use crate::events::{SessionHub, SessionPump, WalletName, WalletState};
use crate::fsutil::create_private_dir;
use crate::gate::{OpGate, OpGuard};
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NoticeCode};

pub(crate) type Manager = PlatformWalletManager<crate::store::WalletStore>;

/// File name of the platform-wallet SQLite database inside a network dir.
pub const WALLET_DB_FILE: &str = "wallet.sqlite";
/// dash-spv storage directory inside a network dir.
pub const SPV_DIR: &str = "spv";

/// Network-scoped wallet id (key-wallet folds the network into the digest,
/// so the same mnemonic has a different id per network).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WalletId(pub [u8; 32]);

impl fmt::Display for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl fmt::Debug for WalletId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WalletId({self})")
    }
}

impl FromStr for WalletId {
    type Err = EngineError;
    /// Accepts exactly 64 lower-case hex characters (the contract's id form).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64 || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(EngineError::InvalidArgument(
                "wallet id must be 64 lower-case hex characters".into(),
            ));
        }
        let bytes = hex::decode(s)
            .map_err(|e| EngineError::InvalidArgument(format!("wallet id is not hex: {e}")))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| EngineError::InvalidArgument("wallet id must be 32 bytes".into()))?;
        Ok(WalletId(arr))
    }
}

/// Core balance buckets in duffs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
    /// Spendable balance (confirmed + unconfirmed) of the wallet's DIP9
    /// CoinJoin accounts. dash-qt's "fully mixed" rounds rule is not applied
    /// here (CoinJoin mixing is WS-06).
    pub coinjoin: u64,
}

/// Result of [`NetworkSession::create_wallet`]: the wallet's seed is in the
/// vault; the phrase is returned for headless hosts (dwcli, tests) that have
/// no other way to show it.
pub struct CreatedWallet {
    pub wallet_id: WalletId,
    pub mnemonic: Zeroizing<String>,
}

impl fmt::Debug for CreatedWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedWallet")
            .field("wallet_id", &self.wallet_id)
            .field("mnemonic", &"<redacted>")
            .finish()
    }
}

/// How to reach the network. Empty lists mean "network defaults"; devnet and
/// regtest have no defaults and need explicit DAPI addresses.
#[derive(Debug, Clone, Default)]
pub struct SessionOptions {
    /// DAPI endpoints (`https://host:port`). Empty = SDK defaults (mainnet/testnet only).
    pub dapi_addresses: Vec<String>,
    /// Override for the trusted quorum service. Must be https on mainnet/testnet.
    pub quorum_url: Option<String>,
    /// SPV peers (`ip:port`). When non-empty, SPV connects to these only.
    pub spv_peers: Vec<String>,
}

/// What a session holds while it is open. Taken out on close so the
/// persister (and its process-wide open-path claim) is released even while
/// hosts still hold `Arc<NetworkSession>`.
#[derive(Clone)]
pub(crate) struct Live {
    pub manager: Arc<Manager>,
    pub persister: Arc<SqlitePersister>,
    /// The persister platform-wallet uses; knows which wallets are closed.
    pub store: Arc<crate::store::WalletStore>,
    pub appdb: Arc<AppDb>,
}

/// The running event pump: its stop signal and task.
struct PumpTask {
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

/// One open network: SDK + PlatformWalletManager<SqlitePersister>, the
/// network's vault and app database, and the in-memory read models.
pub struct NetworkSession {
    pub(crate) network: DashNetwork,
    data_dir: PathBuf,
    pub(crate) rt: Handle,
    pub(crate) sink: Arc<dyn EventSink>,
    context: Arc<LazyTrustedContext>,
    /// Holds every wallet's seed (`<network dir>/vault`). platform-wallet
    /// registers wallets external-signable, so this is the only key material.
    pub(crate) vault: Vault,
    spv_peers: Vec<SocketAddr>,
    pub(crate) hub: Arc<SessionHub>,
    /// Admission of operations vs close (review M1).
    gate: OpGate,
    live: RwLock<Option<Live>>,
    pump: Mutex<Option<PumpTask>>,
    /// Inputs of prepared, not yet settled transactions (E2 send).
    pub(crate) spends: crate::send::PendingSpends,
    /// When this session opened, UNIX seconds ("Startup time").
    pub(crate) opened_at: u64,
    /// The previous session of this network did not close cleanly (its
    /// marker file was still there at open).
    pub(crate) unclean_previous: bool,
    /// dash-qt's load-on-startup wallet list; `None` = load every wallet.
    pub(crate) startup_list: Mutex<Option<std::collections::BTreeSet<WalletId>>>,
}

impl NetworkSession {
    /// Opens the session. Must run on the engine runtime: the manager spawns
    /// its persistence adapter task with `tokio::spawn`.
    pub(crate) async fn open(
        network: DashNetwork,
        data_dir: PathBuf,
        sink: Arc<dyn EventSink>,
        opts: SessionOptions,
        vault_config: VaultConfig,
    ) -> Result<Arc<Self>, EngineError> {
        network.validate()?;
        let spv_peers = opts
            .spv_peers
            .iter()
            .map(|p| {
                p.parse::<SocketAddr>()
                    .map_err(|e| EngineError::InvalidArgument(format!("bad SPV peer {p:?}: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        create_private_dir(&data_dir)?;
        let marker = data_dir.join(crate::tools::SESSION_MARKER);
        let unclean_previous = marker.exists();

        let vault_dir = data_dir.join(VAULT_DIR);
        let (core_network, tag) = (network.core_network(), network.dir_name());
        let vault = tokio::task::spawn_blocking(move || {
            Vault::open(vault_dir, core_network, &tag, vault_config)
        })
        .await??;

        let appdb_path = data_dir.join(APP_DB_FILE);
        let (appdb, names) = tokio::task::spawn_blocking(move || {
            let db = AppDb::open(&appdb_path)?;
            let names = db.wallets()?;
            Ok::<_, dw_appdb::AppDbError>((db, names))
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("app database: {e}")))?;
        let appdb = Arc::new(appdb);

        let context = Arc::new(LazyTrustedContext::new(
            network.core_network(),
            network.devnet_name().map(str::to_string),
            opts.quorum_url.clone(),
        ));
        // First attempt off the async workers: construction does a blocking DNS check.
        let ctx = Arc::clone(&context);
        match tokio::task::spawn_blocking(move || ctx.get_or_try_init(true)).await? {
            Ok(provider) => {
                // Prefetch quorums, as the iOS SDK does after building a trusted SDK.
                tokio::spawn(async move {
                    if let Err(e) = provider.update_quorum_caches().await {
                        tracing::warn!(error = %e, "quorum prefetch failed; continuing");
                    }
                });
            }
            Err(detail) => sink.emit(EngineEvent::Notice {
                network: Some(network.clone()),
                code: NoticeCode::PlatformContextUnavailable,
                detail,
            }),
        }

        let sdk = build_sdk(&network, &opts, Arc::clone(&context))?;

        let db_path = data_dir.join(WALLET_DB_FILE);
        let persister = Arc::new(
            tokio::task::spawn_blocking(move || {
                SqlitePersister::open(SqlitePersisterConfig::new(db_path))
            })
            .await??,
        );
        // dash-qt loads only the wallets of its settings list; an empty or
        // missing list loads every wallet.
        let startup_list = {
            let appdb = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || crate::multiwallet::read_startup_list(&appdb))
                .await??
        };
        let unloaded = match &startup_list {
            Some(list) => {
                let db_path = data_dir.join(WALLET_DB_FILE);
                let registered = tokio::task::spawn_blocking(move || {
                    crate::store::registered_wallet_ids(&db_path)
                })
                .await?
                .map_err(|e| EngineError::Storage(format!("wallet list: {e}")))?;
                registered
                    .into_iter()
                    .filter(|id| !list.contains(&WalletId(*id)))
                    .collect()
            }
            None => Default::default(),
        };
        let store = Arc::new(crate::store::WalletStore::new(
            Arc::clone(&persister),
            unloaded,
        ));

        let hub = Arc::new(SessionHub::new(network.clone(), Arc::clone(&sink)));
        for (order, (id, name, created_at)) in (0u64..).zip(names) {
            match id.parse::<WalletId>() {
                Ok(id) => hub.set_name(
                    id,
                    WalletName {
                        name,
                        created_at: Some(created_at),
                        order,
                    },
                ),
                Err(_) => tracing::warn!(wallet_id = %id, "ignoring a malformed wallet name row"),
            }
        }
        let manager = Arc::new(PlatformWalletManager::new(
            Arc::new(sdk),
            Arc::clone(&store),
            Arc::clone(&hub) as Arc<dyn platform_wallet::PlatformEventHandler>,
        ));
        if let Err(e) = manager.load_from_persistor().await {
            let report = manager.shutdown().await;
            if !report.all_clean() {
                tracing::warn!(?report, "manager shutdown after failed load was not clean");
            }
            return Err(e.into());
        }

        let session = Arc::new(Self {
            network,
            data_dir,
            rt: Handle::current(),
            sink,
            context,
            vault,
            spv_peers,
            hub,
            gate: OpGate::default(),
            live: RwLock::new(Some(Live {
                manager: Arc::clone(&manager),
                persister,
                store,
                appdb: Arc::clone(&appdb),
            })),
            pump: Mutex::new(None),
            spends: Default::default(),
            opened_at: crate::events::unix_now(),
            unclean_previous,
            startup_list: Mutex::new(startup_list),
        });
        if let Err(e) = std::fs::write(&marker, b"") {
            tracing::warn!(error = %e, "could not write the open-session marker");
        }
        for id in manager.list_wallet_ids_blocking() {
            session.refresh_wallet_state(&manager, WalletId(id)).await;
        }
        session.load_history().await?;
        session.start_pump(&manager, appdb);
        Ok(session)
    }

    fn start_pump(self: &Arc<Self>, manager: &Arc<Manager>, appdb: Arc<AppDb>) {
        let (stop, stop_rx) = watch::channel(false);
        let target = SessionPump {
            hub: Arc::clone(&self.hub),
            spv: manager.spv_arc(),
            appdb,
        };
        let hub = Arc::clone(&self.hub);
        let task = tokio::spawn(async move { hub.pump.run(&target, stop_rx).await });
        *self.pump.lock().unwrap_or_else(|p| p.into_inner()) = Some(PumpTask { stop, task });
    }

    /// Reads a wallet's balance and scan heights into the hub.
    pub(crate) async fn refresh_wallet_state(&self, manager: &Manager, id: WalletId) {
        let balance = manager.get_wallet(&id.0).await.map(|w| {
            let b = w.balance();
            WalletCoreBalance::new(b.confirmed(), b.unconfirmed(), b.immature(), b.locked())
        });
        self.store_wallet_state(manager, id, balance).await;
    }

    /// As [`Self::refresh_wallet_state`], with key-wallet's own balance
    /// instead of platform-wallet's event-fed mirror (after the engine
    /// changed key-wallet state directly, e.g. an abandon).
    pub(crate) async fn refresh_wallet_state_from_core(&self, manager: &Manager, id: WalletId) {
        self.store_wallet_state(manager, id, None).await;
    }

    async fn store_wallet_state(
        &self,
        manager: &Manager,
        id: WalletId,
        balance: Option<WalletCoreBalance>,
    ) {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&id.0) else {
            return;
        };
        let core = &info.core_wallet;
        if let Some(cl) = &core.metadata.last_applied_chain_lock {
            self.hub.note_chainlock(cl.block_height, cl.block_hash);
        }
        self.hub.set_wallet_state(
            id,
            WalletState {
                birth_height: core.birth_height(),
                synced_height: core.synced_height(),
                processed_height: core.last_processed_height(),
                balance: balance.unwrap_or_else(|| core.balance()),
                accounts: core.account_balances(),
            },
        );
    }

    pub fn network(&self) -> &DashNetwork {
        &self.network
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Runs `fut` on the engine runtime (for callers outside dw-engine whose
    /// work spawns tasks or uses tokio timers, e.g. the console).
    pub async fn run_on_engine<T, F>(&self, fut: F) -> Result<T, EngineError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        Ok(self.rt.spawn(fut).await?)
    }

    pub fn is_open(&self) -> bool {
        !self.gate.is_closed()
            && self
                .live
                .read()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
    }

    /// Whether the trusted quorum provider has been constructed.
    pub fn platform_context_ready(&self) -> bool {
        self.context.is_ready()
    }

    fn not_open(&self) -> EngineError {
        EngineError::NetworkNotOpen(self.network.to_string())
    }

    /// Admits an async operation; `NetworkNotOpen` once close has started.
    /// Public entry points take one guard each; helpers they call do not
    /// (a nested admission behind a waiting close would deadlock).
    pub(crate) async fn enter(&self) -> Result<OpGuard<'_>, EngineError> {
        self.gate.enter().await.ok_or_else(|| self.not_open())
    }

    /// Admits a synchronous operation without waiting.
    pub(crate) fn try_enter(&self) -> Result<OpGuard<'_>, EngineError> {
        self.gate.try_enter().ok_or_else(|| self.not_open())
    }

    pub(crate) fn live(&self) -> Result<Live, EngineError> {
        self.live
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .cloned()
            .ok_or_else(|| self.not_open())
    }

    pub(crate) fn manager(&self) -> Result<Arc<Manager>, EngineError> {
        Ok(self.live()?.manager)
    }

    /// Runs `fut` on the engine runtime; a panic becomes `EngineError::Internal`.
    pub(crate) async fn on_runtime<T, F>(&self, fut: F) -> Result<T, EngineError>
    where
        F: Future<Output = Result<T, EngineError>> + Send + 'static,
        T: Send + 'static,
    {
        self.rt.spawn(fut).await?
    }

    /// The configuration dash-spv is started with: storage under
    /// `<network dir>/spv`, masternode sync on (InstantSend/ChainLock
    /// handling depends on it), restricted to the configured peers when any
    /// were given. Counterpart: `platform_wallet_manager_spv_start`
    /// (rs-platform-wallet-ffi/src/spv.rs:408, config built at :519-551).
    fn spv_config(&self) -> Result<ClientConfig, EngineError> {
        let mut config = ClientConfig::new(self.network.core_network())
            .with_storage_path(self.data_dir.join(SPV_DIR))
            .with_user_agent(crate::tools::USER_AGENT);
        config.enable_masternodes = true;
        for peer in &self.spv_peers {
            config.add_peer(*peer);
        }
        if !self.spv_peers.is_empty() {
            config = config.with_restrict_to_configured_peers(true);
        }
        if let Some(name) = self.network.devnet_name() {
            config = config.with_devnet(DevnetConfig::new(name));
        }
        config.validate().map_err(EngineError::InvalidConfig)?;
        Ok(config)
    }

    /// Starts dash-spv and spawns its run loop.
    pub(crate) async fn start_spv_inner(&self, manager: &Manager) -> Result<(), EngineError> {
        let config = self.spv_config()?;
        let spv = manager.spv_arc();
        spv.start(config).await?;
        spv.spawn_run_loop();
        self.hub.set_spv_running(true);
        self.sink.emit(EngineEvent::SpvStateChanged {
            network: self.network.clone(),
            running: true,
        });
        Ok(())
    }

    pub(crate) async fn stop_spv_inner(&self, manager: &Manager) -> Result<(), EngineError> {
        let stopped = manager.spv().stop().await;
        self.hub.set_spv_running(false);
        self.sink.emit(EngineEvent::SpvStateChanged {
            network: self.network.clone(),
            running: false,
        });
        Ok(stopped?)
    }

    pub async fn start_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            this.start_spv_inner(&manager).await
        })
        .await
    }

    /// Stops dash-spv. Idempotent.
    pub async fn stop_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            this.stop_spv_inner(&manager).await
        })
        .await
    }

    pub fn spv_running(&self) -> Result<bool, EngineError> {
        let _op = self.try_enter()?;
        Ok(self.manager()?.spv().is_started())
    }

    /// Closes the session: waits for admitted operations (review M1), stops
    /// the event pump, shuts the manager down (SPV, coordinators,
    /// persistence adapter drain) and releases the databases. Must run on
    /// the engine runtime.
    pub(crate) async fn close(&self) {
        let _closing = self.gate.close().await;
        let pump = self.pump.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(pump) = pump {
            let _ = pump.stop.send(true);
            if let Err(e) = pump.task.await {
                tracing::warn!(error = %e, "event pump task ended abnormally");
            }
        }
        let taken = self.live.write().unwrap_or_else(|p| p.into_inner()).take();
        let Some(live) = taken else { return };
        let manager = live.manager;
        let report = manager.shutdown().await;
        self.hub.set_spv_running(false);
        if report.all_clean() {
            let marker = self.data_dir.join(crate::tools::SESSION_MARKER);
            if let Err(e) = std::fs::remove_file(&marker)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, "could not remove the open-session marker");
            }
        } else {
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code: NoticeCode::UncleanShutdown,
                detail: format!("{:?}", report.per_worker),
            });
        }
        // Unload every wallet before dropping the manager. Upstream has a
        // reference cycle that otherwise leaks each registered wallet — and
        // with it the persister and its process-wide open-path claim, so the
        // database could never be reopened in this process:
        //   PlatformWallet -> SpvBroadcaster -> Arc<SpvRuntime>
        //     (rs-platform-wallet/src/broadcaster.rs:219-226, manager/wallet_lifecycle.rs:557)
        //   SpvRuntime -> PlatformEventManager -> BalanceUpdateHandler
        //     -> Arc<ArcSwap<wallets map>> -> Arc<PlatformWallet>
        //     (manager/mod.rs:522, wallet/core/balance_handler.rs:43-45)
        // `remove_wallet` is an in-memory unload (no persister writes:
        // manager/wallet_lifecycle.rs:676-900, key-wallet-manager accessors.rs:75).
        for id in manager.list_wallet_ids_blocking() {
            if let Err(e) = manager.remove_wallet(&id).await {
                tracing::warn!(wallet_id = %hex::encode(id), error = %e, "unload on close failed");
            }
        }
        drop(manager);
        drop(live.store);
        drop(live.persister);
        drop(live.appdb);
        self.sink.emit(EngineEvent::SessionClosed {
            network: self.network.clone(),
        });
    }
}

fn build_sdk(
    network: &DashNetwork,
    opts: &SessionOptions,
    context: Arc<LazyTrustedContext>,
) -> Result<dash_sdk::Sdk, EngineError> {
    let builder = if opts.dapi_addresses.is_empty() {
        match network {
            DashNetwork::Mainnet => SdkBuilder::new_mainnet(),
            DashNetwork::Testnet => SdkBuilder::new_testnet(),
            DashNetwork::Devnet { .. } | DashNetwork::Regtest => {
                return Err(EngineError::InvalidConfig(format!(
                    "{network} has no default DAPI addresses; pass dapi_addresses"
                )));
            }
        }
    } else {
        let list = AddressList::from_str(&opts.dapi_addresses.join(","))
            .map_err(|e| EngineError::InvalidArgument(format!("bad DAPI address list: {e}")))?;
        SdkBuilder::new(list).with_network(network.core_network())
    };
    Ok(builder
        .with_context_provider(SharedContext(context))
        .build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_id_hex_round_trip() {
        let id = WalletId([0xab; 32]);
        let s = id.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(s.parse::<WalletId>().unwrap(), id);
        assert!("abcd".parse::<WalletId>().is_err());
        assert!("zz".repeat(32).parse::<WalletId>().is_err());
        // The contract's form is lower case only.
        assert!("AB".repeat(32).parse::<WalletId>().is_err());
    }

    #[test]
    fn created_wallet_debug_redacts_mnemonic() {
        let w = CreatedWallet {
            wallet_id: WalletId([1; 32]),
            mnemonic: Zeroizing::new("secret words".into()),
        };
        assert!(!format!("{w:?}").contains("secret"));
    }
}
