use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};

use dash_sdk::SdkBuilder;
use dash_sdk::dapi_grpc::tonic::transport::{Certificate, ClientTlsConfig, Endpoint};
use dash_sdk::sdk::AddressList;
use dash_spv::{ClientConfig, DevnetConfig};
use dpp::version::PlatformVersion;
use dw_appdb::{APP_DB_FILE, AppDb};
use dw_vault::{VAULT_DIR, Vault, VaultConfig};
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformWalletManager;
use platform_wallet_storage::{SqlitePersister, SqlitePersisterConfig};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::{Error as PemError, PemObject};
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::context::{LazyTrustedContext, SharedContext};
use crate::events::{SessionHub, SessionPump, WalletName, WalletState};
use crate::fsutil::{create_owned_dir, create_owned_file};
use crate::gate::{OpGate, OpGuard};
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NoticeCode};

pub(crate) type Manager = PlatformWalletManager<crate::store::WalletStore>;

/// File name of the platform-wallet SQLite database inside a network dir.
pub const WALLET_DB_FILE: &str = "wallet.sqlite";
/// dash-spv storage directory inside a network dir.
pub const SPV_DIR: &str = "spv";
/// DashPay avatar thumbnails inside a network dir (DASHPAY §3.4).
pub const AVATARS_DIR: &str = "avatars";

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
    /// Fully mixed CoinJoin balance (QT-043, `CoinJoinBalances.fully_mixed`)
    /// once the wallet's CoinJoin status was computed; before that, the
    /// spendable balance (confirmed + unconfirmed) of the DIP9 CoinJoin
    /// accounts.
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
    /// PEM CA certificate the SDK trusts for DAPI TLS, in addition to the
    /// system roots (a dashmate devnet gateway serves a self-signed
    /// certificate). Every certificate in the file must parse; the file may
    /// be a CA or a self-signed leaf with `CA:FALSE`, and the DAPI host name
    /// must match its subject alternative names. Replacing the system roots
    /// (pinning) needs an SDK change and is not offered.
    pub ca_cert_path: Option<PathBuf>,
    /// Protocol version the SDK starts at, used as given (a seed below the
    /// network's floor is not clamped). Auto-detect stays on and ratchets it
    /// up from what the network reports. `None` = the per-network floor.
    pub initial_protocol_version: Option<u32>,
    /// The chain has no Platform (a plain dashd regtest): `start_spv` runs
    /// no DashPay bring-up and no Platform sync loops, and every wallet's
    /// `dashpay_startup` stays `NotRun`.
    pub no_platform: bool,
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
    /// The changeset tap the store feeds: DashPay signals and the journal
    /// (DASHPAY §3.5). Here, not on the session, so close releases its
    /// `app.sqlite` handle.
    pub tap: Arc<crate::platform::journal::ChangesetTap>,
    /// Positive provenance (DEC-125): which Platform data counts as verified.
    pub provenance: Arc<crate::platform::provenance::Provenance>,
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
    /// The avatar cache directory could be created at open; without it
    /// avatars are off for the session (`avatars_dir`).
    avatars_ready: bool,
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
    /// The console `walletpassphrase` relock timer (review L1): one per
    /// session, replaced by a later one, cancelled by `lock_vault` and
    /// `close`. (generation, task).
    pub(crate) relock: Mutex<Option<(u64, tokio::task::AbortHandle)>>,
    /// Mixing state, CoinJoin options and queues (M3, `coinjoin.rs`).
    pub(crate) coinjoin: crate::coinjoin::CoinJoinRuntime,
    /// The bring-up, the Platform sync loops and their status (E0-05).
    pub(crate) platform: crate::platform::runtime::PlatformRuntime,
    /// Leases, the lock coordinator and the dispatch fence (E0-04).
    pub(crate) leases: Arc<crate::platform::lease::LeaseTable>,
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
        // Everything the caller can get wrong, before any directory, vault or
        // network I/O exists to clean up.
        let sdk_options = SdkOptions::validate(&opts)?;
        create_owned_dir(&data_dir, Path::new(""))?;
        let marker = data_dir.join(crate::tools::SESSION_MARKER);
        let unclean_previous = marker.exists();

        let vault_dir = data_dir.join(VAULT_DIR);
        let (core_network, tag) = (network.core_network(), network.dir_name());
        let vault = tokio::task::spawn_blocking(move || {
            Vault::open(vault_dir, core_network, &tag, vault_config)
        })
        .await??;

        let appdb_path = data_dir.join(APP_DB_FILE);
        create_owned_file(&data_dir, Path::new(APP_DB_FILE))?;
        let (appdb, names) = tokio::task::spawn_blocking(move || {
            let db = AppDb::open(&appdb_path)?;
            let names = db.wallets()?;
            Ok::<_, dw_appdb::AppDbError>((db, names))
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("app database: {e}")))?;
        let appdb = Arc::new(appdb);
        // Owner-only whatever the umask (DASHPAY §3.4). The cache is
        // disposable, so a stray file, a symlink that cannot be resolved, or
        // an entry owned by another user (or a symlink loop) named `avatars`
        // turns avatars off for this session instead of failing the open. A
        // dangling symlink whose target can be created is followed and the
        // target created (mode 0700); avatars stay on.
        let avatars_ready = match create_owned_dir(&data_dir, Path::new(AVATARS_DIR)) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, dir = %data_dir.join(AVATARS_DIR).display(),
                    "avatar cache directory unusable; avatars are disabled for this session");
                false
            }
        };
        let coinjoin_settings = {
            let db = Arc::clone(&appdb);
            tokio::task::spawn_blocking(move || crate::coinjoin::load_settings(&db)).await?
        };

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

        let sdk = build_sdk(&network, &opts, sdk_options, Arc::clone(&context))?;

        let db_path = data_dir.join(WALLET_DB_FILE);
        // SqlitePersister creates its backup directory with the umask's mode
        // and refuses one below a group-writable directory: `backups` and
        // `backups/auto` are owner-only, legacy 0775 ones included, before
        // it needs a pre-migration backup.
        let auto_backups = platform_wallet_storage::default_auto_backup_dir(&db_path);
        let auto_backups = auto_backups.strip_prefix(&data_dir).map_err(|_| {
            EngineError::Internal(format!(
                "{} is outside the network directory",
                auto_backups.display()
            ))
        })?;
        create_owned_dir(&data_dir, auto_backups)?;
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
        let hub = Arc::new(SessionHub::new(network.clone(), Arc::clone(&sink)));
        let tap = Arc::new(crate::platform::journal::ChangesetTap::new(
            Arc::clone(&hub),
            Arc::clone(&appdb),
        ));
        let provenance = {
            let appdb = Arc::clone(&appdb);
            Arc::new(
                tokio::task::spawn_blocking(move || {
                    crate::platform::provenance::Provenance::open(appdb)
                })
                .await?,
            )
        };
        let store = Arc::new(crate::store::WalletStore::new(
            Arc::clone(&persister),
            unloaded,
            Arc::clone(&tap),
        ));
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

        let lock_state = vault.lock_state();
        let journal = crate::platform::lease::session::open_journal(&data_dir).await;
        let leases = crate::platform::lease::LeaseTable::new(
            network.clone(),
            Arc::clone(&sink),
            Handle::current(),
            Default::default(),
            vault.epoch(),
        );
        let session = Arc::new(Self {
            network,
            data_dir,
            avatars_ready,
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
                tap,
                provenance,
            })),
            pump: Mutex::new(None),
            spends: Default::default(),
            opened_at: crate::events::unix_now(),
            unclean_previous,
            startup_list: Mutex::new(startup_list),
            relock: Mutex::new(None),
            coinjoin: crate::coinjoin::CoinJoinRuntime::new(coinjoin_settings),
            platform: crate::platform::runtime::PlatformRuntime::new(lock_state, !opts.no_platform),
            leases,
        });
        match journal {
            Some((backend, rows, steps)) => session.leases.load_journal(Some(backend), rows, steps),
            None => session.leases.load_journal(None, Vec::new(), Vec::new()),
        }
        session.leases.start_reaper();
        if let Err(e) =
            create_owned_file(&session.data_dir, Path::new(crate::tools::SESSION_MARKER))
        {
            tracing::warn!(error = %e, "could not write the open-session marker");
        }
        session.apply_stored_lookaheads(&manager).await;
        for id in manager.list_wallet_ids_blocking() {
            session.refresh_wallet_state(&manager, WalletId(id)).await;
            session.load_identity_choices(WalletId(id)).await;
        }
        session.load_history().await?;
        session.start_pump(&manager, appdb);
        // dash-qt backs a wallet up when it loads (QT-116); skipped while
        // the data key is not available.
        for id in manager.list_wallet_ids_blocking() {
            session.schedule_automatic_backup(WalletId(id));
        }
        session.spawn_ensure_background();
        Ok(session)
    }

    fn start_pump(self: &Arc<Self>, manager: &Arc<Manager>, appdb: Arc<AppDb>) {
        let (stop, stop_rx) = watch::channel(false);
        let target = SessionPump {
            hub: Arc::clone(&self.hub),
            spv: manager.spv_arc(),
            appdb,
            vault: self.vault.clone(),
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
                fully_mixed: self.hub.wallet_state(&id).and_then(|s| s.fully_mixed),
            },
        );
    }

    pub fn network(&self) -> &DashNetwork {
        &self.network
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The avatar thumbnail directory (DASHPAY §3.4), or `None` when it could
    /// not be created at open: avatars are then disabled for this session.
    pub fn avatars_dir(&self) -> Option<PathBuf> {
        self.avatars_ready.then(|| self.data_dir.join(AVATARS_DIR))
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
    pub(crate) fn spv_config(&self) -> Result<ClientConfig, EngineError> {
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
        // Owner-only before dash-spv creates it with the umask's mode.
        create_owned_dir(&self.data_dir, Path::new(SPV_DIR))?;
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

    /// Closes the session: cancels a running bring-up, waits for admitted
    /// operations (review M1), stops the event pump, shuts the manager down
    /// (SPV, coordinators, persistence adapter drain) and releases the
    /// databases. Must run on the engine runtime.
    pub(crate) async fn close(&self) {
        // A running bring-up ends first (DASHPAY §3.2). It holds no
        // operation guard, so it is cancelled before waiting for the guards,
        // and again after, for a start admitted meanwhile; then its blocking
        // key work is waited for (bounded). The loops drain once, sealed, in
        // the manager's shutdown below (m1-engine §3.2 interpretations).
        self.platform.deactivate(true).await;
        // E0-04 §8.5: revoke, drain and abort the flows before the gate,
        // which waits for the flows' operation guards.
        self.close_leases().await;
        let _closing = self.gate.close().await;
        self.platform.deactivate(true).await;
        let keys_idle = self.platform.key_work_idle().await;
        self.cancel_relock();
        self.coinjoin.shutdown();
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
        if report.all_clean() && keys_idle {
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
                detail: if keys_idle {
                    format!("{:?}", report.per_worker)
                } else {
                    format!(
                        "{:?}; a DashPay bring-up key read did not end",
                        report.per_worker
                    )
                },
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
        drop(live.tap);
        drop(live.provenance);
        // Per network, so the last to close (§8.5).
        self.leases.close_journal();
        self.sink.emit(EngineEvent::SessionClosed {
            network: self.network.clone(),
        });
    }
}

/// The SDK options that need parsing, validated once at `open`.
struct SdkOptions {
    ca: Option<Certificate>,
    initial_version: Option<&'static PlatformVersion>,
}

impl SdkOptions {
    fn validate(opts: &SessionOptions) -> Result<Self, EngineError> {
        let ca = opts.ca_cert_path.as_deref().map(load_ca).transpose()?;
        let initial_version = opts
            .initial_protocol_version
            .map(|version| {
                PlatformVersion::get(version).map_err(|e| {
                    EngineError::InvalidConfig(format!("initial protocol version {version}: {e}"))
                })
            })
            .transpose()?;
        Ok(Self {
            ca,
            initial_version,
        })
    }
}

/// Reads and checks a PEM CA file the way the SDK will use it. The SDK parses
/// it at its first DAPI call and panics the calling task on a bad one
/// (`rs-dapi-client` `create_channel`: `expect("Failed to set TLS config")`),
/// so a damaged file has to be rejected here.
fn load_ca(path: &Path) -> Result<Certificate, EngineError> {
    let bad_ca = |e: &dyn fmt::Display| {
        EngineError::InvalidConfig(format!("CA certificate {}: {e}", path.display()))
    };
    let pem = std::fs::read(path).map_err(|e| bad_ca(&e))?;
    // Every PEM block must decode (truncated and bad-base64 ones fail here)...
    let certs = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| match e {
            PemError::MissingSectionEnd { .. } => {
                bad_ca(&"truncated: a CERTIFICATE block has no END line")
            }
            PemError::Base64Decode(detail) => {
                bad_ca(&format_args!("a CERTIFICATE block is not base64: {detail}"))
            }
            other => bad_ca(&format_args!("not a PEM certificate: {other:?}")),
        })?;
    if certs.is_empty() {
        return Err(bad_ca(&"no -----BEGIN CERTIFICATE----- block"));
    }
    // ...and be a certificate: tonic silently drops a block whose DER does not
    // parse, which would only show up later as an unknown issuer.
    for (index, cert) in certs.iter().enumerate() {
        webpki::anchor_from_trusted_cert(cert)
            .map_err(|e| bad_ca(&format_args!("certificate {} of the file: {e}", index + 1)))?;
    }
    let cert = Certificate::from_pem(pem);
    // Last, the exact call the SDK makes with it. Cheap, and it holds even if
    // tonic's parsing changes.
    Endpoint::from_static("https://localhost")
        .tls_config(ClientTlsConfig::new().ca_certificate(cert.clone()))
        .map_err(|e| bad_ca(&e))?;
    Ok(cert)
}

fn build_sdk(
    network: &DashNetwork,
    opts: &SessionOptions,
    sdk_options: SdkOptions,
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
    let mut builder = builder.with_context_provider(SharedContext(context));
    if let Some(ca) = sdk_options.ca {
        builder = builder.with_ca_certificate(ca);
    }
    if let Some(version) = sdk_options.initial_version {
        builder = builder.with_initial_version(version);
    }
    Ok(builder.build()?)
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

    fn sdk_with(opts: SessionOptions) -> Result<dash_sdk::Sdk, EngineError> {
        let context = Arc::new(LazyTrustedContext::new(
            dashcore::Network::Regtest,
            None,
            Some("http://127.0.0.1:1".into()),
        ));
        let opts = SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            ..opts
        };
        let sdk_options = SdkOptions::validate(&opts)?;
        build_sdk(&DashNetwork::Regtest, &opts, sdk_options, context)
    }

    #[test]
    fn initial_protocol_version_seeds_the_sdk() {
        let sdk = sdk_with(SessionOptions {
            initial_protocol_version: Some(PlatformVersion::first().protocol_version),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            sdk.protocol_version_number(),
            PlatformVersion::first().protocol_version
        );
        let err = sdk_with(SessionOptions {
            initial_protocol_version: Some(u32::MAX),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, EngineError::InvalidConfig(_)), "{err:?}");
    }

    #[test]
    fn missing_ca_certificate_is_a_config_error() {
        let err = sdk_with(SessionOptions {
            ca_cert_path: Some("/nonexistent/dwd-ca.pem".into()),
            ..Default::default()
        })
        .unwrap_err();
        assert!(matches!(err, EngineError::InvalidConfig(_)), "{err:?}");
    }

    /// A self-signed `CA:FALSE` certificate (CN=localhost), as a dashmate
    /// gateway serves; validity is not checked at load.
    const GOOD_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
MIIDHzCCAgegAwIBAgIUC0UrfHVX7RAdqsyyadadvEUJ9ZMwDQYJKoZIhvcNAQEL\n\
BQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MTAwODE1NTU0MloXDTI2MTAx\n\
MDE1NTU0MlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\n\
AAOCAQ8AMIIBCgKCAQEAzLIVPB7Jw8N3Hl0OwSMiZA9r1gdBGJ7rKFTNtCwZkLU5\n\
uMm1w9f1YHQcYAWCnLeKBxgvgQrHVGifhrXlVttyJnhiZiE15huWyE1a7AON/7Nx\n\
vOpWy1j2cidZMou6r2UiBLJ33fjpWyXEWbshgZk3SNL7NSJ+2iANqCcKL2TaInfZ\n\
v081VzcHMyheMQpxvncR36n0GjvtV9HH7ApX/Mih/DXnkhG6LTvGqiTRKyPcPsHb\n\
7gLUcJYbQbd4aDdlkoQSJfc+zs3N6Fb2CsZM1zTHZBZ21brxWVS5yuqVVk7EEd+A\n\
KHm8kRNZMnu2NPAg8ATcpdZ7e3Ns5yTZx1Jm+efCcwIDAQABo2kwZzAdBgNVHQ4E\n\
FgQUfJ8EmVR6rAgSGiRcGql6Qn8lUyUwHwYDVR0jBBgwFoAUfJ8EmVR6rAgSGiRc\n\
Gql6Qn8lUyUwGgYDVR0RBBMwEYIJbG9jYWxob3N0hwR/AAABMAkGA1UdEwQCMAAw\n\
DQYJKoZIhvcNAQELBQADggEBAJWftMJtNa+9Xn29ESCmkq7D7mpDUemmPi1A6QTS\n\
HyKU1JvXWyiqbwe+TK+LmTR5vyzgWY29pMFkuZ44hTVF8XlIIMt7OU9u8R9Msq8/\n\
M3DLDx5xFLWpl53Chu0EkZOrq3Zo/hC/OLGbLSQIElqa/t/+CByg2dT4OtmG2KMb\n\
IYyg/zrxLd6ckSamTS/peM81T5R92Qa9PnDGusYDGWurZEEm6ZQtCVmtu7joC+NE\n\
J62k8Vm0RL24O48uZ1hN5XHIBBu8bVYm3zm3k+GS2dLwZna3ciWpEjqwXMZyUiBC\n\
PRihiNYRunWtfhIwgcXNS8k8Aw/lDZNOwwt4ykNeDyMuBUM=\n\
-----END CERTIFICATE-----\n\
";

    fn ca_error(contents: &[u8]) -> EngineError {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("ca.pem");
        std::fs::write(&path, contents).unwrap();
        sdk_with(SessionOptions {
            ca_cert_path: Some(path),
            ..Default::default()
        })
        .unwrap_err()
    }

    #[test]
    fn a_valid_ca_certificate_is_accepted() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join("ca.pem");
        // A chain: the certificate twice, with a comment line between.
        std::fs::write(&path, format!("{GOOD_CA_PEM}# again\n{GOOD_CA_PEM}")).unwrap();
        sdk_with(SessionOptions {
            ca_cert_path: Some(path),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn a_damaged_ca_file_is_a_config_error() {
        // Each of these used to pass the open-time check and panic an engine
        // task at the first DAPI call (or, for the last, trust nothing).
        let cases: [(&str, Vec<u8>); 7] = [
            ("empty", Vec::new()),
            (
                "key, not a certificate",
                b"-----BEGIN PRIVATE KEY-----\n".to_vec(),
            ),
            ("DER bytes", vec![0x30, 0x82, 0x03, 0x1f, 0x00, 0xff]),
            (
                "truncated mid-block",
                GOOD_CA_PEM.as_bytes()[..300].to_vec(),
            ),
            (
                "no END line",
                b"-----BEGIN CERTIFICATE-----\nZm9vYmFy\n".to_vec(),
            ),
            (
                "bad base64",
                b"-----BEGIN CERTIFICATE-----\n!!!!not base64!!!!\n-----END CERTIFICATE-----\n"
                    .to_vec(),
            ),
            (
                "base64 that is not a certificate",
                b"-----BEGIN CERTIFICATE-----\nZm9vYmFyZm9vYmFy\n-----END CERTIFICATE-----\n"
                    .to_vec(),
            ),
        ];
        for (name, contents) in cases {
            let err = ca_error(&contents);
            assert!(
                matches!(err, EngineError::InvalidConfig(_)),
                "{name}: {err:?}"
            );
        }
        // A good certificate followed by a truncated one is damaged too.
        let mut chain = GOOD_CA_PEM.as_bytes().to_vec();
        chain.extend_from_slice(&GOOD_CA_PEM.as_bytes()[..300]);
        let err = ca_error(&chain);
        assert!(
            matches!(err, EngineError::InvalidConfig(_)),
            "chain: {err:?}"
        );
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
