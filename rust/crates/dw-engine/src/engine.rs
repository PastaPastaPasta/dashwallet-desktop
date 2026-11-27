use std::collections::HashMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::runtime::{Handle, Runtime};
use tokio::sync::Mutex;

use crate::fsutil::create_private_dir;
use crate::gate::PauseRefused;
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NetworkSession, SessionOptions};

/// Stack size of the engine runtime's threads: the async workers and the
/// blocking pool. tokio's default is 2 MiB, which proof descent in the
/// Platform SDK overflows; platform-wallet's own DashPay loop and both
/// platform FFI runtimes use 8 MiB (DASHPAY §2.5). The `dw-engine-drop` thread
/// gets it too. The `dw-quorum-context` retry thread does no proof work and
/// keeps the platform default, and a future polled by [`Engine::block_on`]
/// runs on the caller's thread, so Platform calls must hop onto the runtime
/// (`on_runtime`).
pub const ENGINE_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

/// The name of the engine runtime's threads.
const ENGINE_THREAD_NAME: &str = "dw-engine";

/// Chains a panic hook that marks the engine poisoned
/// ([`crate::engine_poisoned`]) for a panic on its threads, whatever task it
/// was, then calls the hook it replaced. Once per process.
fn install_poison_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if std::thread::current().name() == Some(ENGINE_THREAD_NAME) {
                crate::error::poison();
            }
            previous(info);
        }));
    });
}

/// How long an in-app close waits for a wallet load in flight
/// (`Engine::close_network`): a load reads local storage only.
#[cfg(not(test))]
const CLOSE_LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
#[cfg(test)]
pub(crate) const CLOSE_LOAD_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

fn close_load_wait(_session: &NetworkSession) -> Duration {
    #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
    if let Some(wait) = *_session.close_load_wait.lock().unwrap() {
        return wait;
    }
    CLOSE_LOAD_WAIT
}

/// Why [`Engine::close_network_unless_busy`] left the session open.
#[derive(Debug)]
pub enum CloseRefused<B> {
    /// The close's own refusals, as [`Engine::close_network`]'s.
    Engine(EngineError),
    /// What the check found running.
    Busy(B),
    /// The session's operations did not finish within the drain.
    Draining,
    /// A transfer waits on its wallet's start-up to go on.
    Preparing,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Root data directory; each network lives in `<data_root>/<network>/`,
    /// or in the folder `network-folders.json` names for it
    /// ([`Engine::network_dir`]).
    pub data_root: PathBuf,
    /// tokio worker threads; `None` = tokio's default (one per core).
    pub worker_threads: Option<usize>,
    /// Vault options (KDF policy, OS secret store, clock) every session's
    /// vault is opened with. `VaultConfig::default()` is the production
    /// choice: calibrated Argon2id and the OS keyring.
    pub vault: dw_vault::VaultConfig,
}

struct Shared {
    config: EngineConfig,
    /// Prices (MP-01): the engine's, not a network's.
    rates: Arc<crate::rates::RatesHub>,
    sink: Arc<dyn EventSink>,
    /// Lifecycle operations (open/close) are serialised by this lock, the
    /// desktop analogue of iOS's `SerialAsyncLifecycleQueue` at engine level.
    sessions: Mutex<HashMap<DashNetwork, Arc<NetworkSession>>>,
    /// How the store-upgrade check refused each network's last open in
    /// this engine, where that way out allows starting over in a new folder
    /// ([`Engine::recover_in_fresh_folder`]).
    refused: std::sync::Mutex<HashMap<DashNetwork, Refused>>,
    /// The data root's instance lock (`dw_fs::instance`): another process's
    /// engine, dwcli's or an app's, is refused the root while this one has
    /// it.
    _instance: dw_fs::instance::InstanceLock,
}

/// A refused open's way out that [`Engine::recover_in_fresh_folder`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refused {
    /// [`EngineError::StoreRefused`] with
    /// [`WayOut::Damaged`](dw_store_upgrade::copy::WayOut::Damaged): may
    /// start over.
    Damaged,
    /// [`EngineError::StoreRefused`] with
    /// [`WayOut::Restore`](dw_store_upgrade::copy::WayOut::Restore), its
    /// restore not refused for a shielded hold: may not start over.
    Restore,
    /// Restore earlier data refused for a shielded hold (R-1): the request
    /// after a [`Self::Restore`] open, or the swap at the open. May start
    /// over, and keeps through later opens refused with
    /// [`WayOut::Restore`](dw_store_upgrade::copy::WayOut::Restore), whose
    /// screens then offer it.
    RestoreHeld,
}

impl Shared {
    fn refused(&self, network: &DashNetwork) -> Option<Refused> {
        let refused = self.refused.lock().unwrap_or_else(|p| p.into_inner());
        refused.get(network).copied()
    }

    fn set_refused(&self, network: &DashNetwork, state: Option<Refused>) {
        let mut refused = self.refused.lock().unwrap_or_else(|p| p.into_inner());
        match state {
            Some(state) => refused.insert(network.clone(), state),
            None => refused.remove(network),
        };
    }

    /// Records how `opened` was refused. A restore the swap refused for a
    /// shielded hold, and a later [`WayOut::Restore`] refusal after one, keep
    /// [`Refused::RestoreHeld`], whose screen then offers Set up from
    /// recovery phrase; any other result, an open that is not refused among
    /// them, withdraws it.
    fn note_open<T>(&self, network: &DashNetwork, opened: &mut Result<T, EngineError>) {
        use dw_store_upgrade::copy::WayOut;
        let state = match opened {
            Err(EngineError::StoreRefused(r)) if r.screen.way_out == WayOut::Damaged => {
                Some(Refused::Damaged)
            }
            Err(EngineError::StoreRefused(r)) if r.screen.way_out == WayOut::Restore => {
                if self.refused(network) == Some(Refused::RestoreHeld) {
                    r.screen.offer_recover();
                    Some(Refused::RestoreHeld)
                } else {
                    Some(Refused::Restore)
                }
            }
            Err(EngineError::StoreRestore(r))
                if r.state == crate::store_upgrade::RestoreState::NotStarted
                    && crate::store_upgrade::is_hold_cause(r.cause) =>
            {
                Some(Refused::RestoreHeld)
            }
            _ => None,
        };
        self.set_refused(network, state);
    }
}

/// Owns the tokio runtime and the open network sessions.
pub struct Engine {
    runtime: Option<Runtime>,
    handle: Handle,
    shared: Arc<Shared>,
}

impl Engine {
    pub fn new(config: EngineConfig, sink: Arc<dyn EventSink>) -> Result<Self, EngineError> {
        let started = Instant::now();
        let mark = |phase: &'static str| {
            tracing::info!(
                phase,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "engine start"
            );
        };
        if config.data_root.as_os_str().is_empty() {
            return Err(EngineError::InvalidConfig("data_root is empty".into()));
        }
        if config.worker_threads == Some(0) {
            return Err(EngineError::InvalidConfig(
                "worker_threads must be > 0".into(),
            ));
        }
        create_private_dir(&config.data_root)?;
        mark("data-root");
        let instance =
            dw_fs::instance::InstanceLock::share(&config.data_root).map_err(|e| match e {
                dw_fs::instance::InstanceError::Busy => EngineError::storage_in_use(format!(
                    "{} is in use by another process",
                    config.data_root.display()
                )),
                dw_fs::instance::InstanceError::Io(e) => EngineError::from(e),
            })?;
        mark("instance");
        crate::store_upgrade::order::record(
            &config.data_root,
            crate::store_upgrade::order::Step::Instance,
        );
        crate::store_upgrade::sweep_once(&config.data_root);
        install_poison_hook();
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        // Applies to the async workers and the blocking pool alike.
        builder
            .enable_all()
            .thread_name(ENGINE_THREAD_NAME)
            .thread_stack_size(ENGINE_THREAD_STACK_SIZE);
        if let Some(n) = config.worker_threads {
            builder.worker_threads(n);
        }
        let runtime = builder
            .build()
            .map_err(|e| EngineError::Internal(format!("tokio runtime: {e}")))?;
        mark("runtime");
        let handle = runtime.handle().clone();
        #[cfg(not(test))]
        let rates_config = crate::rates::RatesConfig::production();
        #[cfg(test)]
        let rates_config = crate::rates::RatesConfig::offline_for_tests();
        let rates = crate::rates::RatesHub::start(
            &config.data_root,
            rates_config,
            Arc::clone(&sink),
            &handle,
        );
        mark("rates");
        Ok(Self {
            runtime: Some(runtime),
            handle,
            shared: Arc::new(Shared {
                config,
                rates,
                sink,
                sessions: Mutex::new(HashMap::new()),
                refused: std::sync::Mutex::new(HashMap::new()),
                _instance: instance,
            }),
        })
    }

    pub(crate) fn rates_hub(&self) -> Arc<crate::rates::RatesHub> {
        Arc::clone(&self.shared.rates)
    }

    pub fn config(&self) -> &EngineConfig {
        &self.shared.config
    }

    /// Directory used for `network`: `<data_root>/<network>`, or the folder
    /// `<data_root>/network-folders.json` names for it once it started over
    /// ([`Self::recover_in_fresh_folder`]). A record that can't be read or
    /// names anything else is a `Storage` error, never the default folder.
    pub fn network_dir(&self, network: &DashNetwork) -> Result<PathBuf, EngineError> {
        crate::network_folders::resolve(&self.shared.config.data_root, network)
    }

    /// The way out of a refused open (Q107 C1): `network` starts over in a
    /// new, empty, owner-only folder beside its own,
    /// `<dir name>-recovered-<n>` with the lowest `n` free, which
    /// `network-folders.json` records for every open after this one, this
    /// engine's and the next. Nothing in the network's current folder is
    /// written, and none of its stores is opened: the refused data stays as
    /// it is. Returns the new folder. `InvalidArgument` while the network
    /// is open, or unless its last open in this engine was refused by the
    /// store-upgrade check as damaged (`SnapshotSourceDamaged`, the
    /// `wayOut.damaged` screen), or Restore earlier data was refused for a
    /// shielded hold since an open refused with `wayOut.restore` or at that
    /// open's swap (R-1); every other refusal keeps its own way out.
    pub async fn recover_in_fresh_folder(
        &self,
        network: DashNetwork,
    ) -> Result<PathBuf, EngineError> {
        network.validate()?;
        let shared = Arc::clone(&self.shared);
        self.handle
            .spawn(async move {
                let sessions = shared.sessions.lock().await;
                if sessions.contains_key(&network) {
                    return Err(EngineError::InvalidArgument(format!(
                        "{network} is open; only a network whose open was refused starts over"
                    )));
                }
                if !matches!(
                    shared.refused(&network),
                    Some(Refused::Damaged | Refused::RestoreHeld)
                ) {
                    return Err(EngineError::InvalidArgument(format!(
                        "the last open of {network} was not refused as damaged, nor its restore for a shielded hold; nothing to start over from"
                    )));
                }
                let (root, net) = (shared.config.data_root.clone(), network.clone());
                let dir = tokio::task::spawn_blocking(move || {
                    crate::network_folders::recover(&root, &net)
                })
                .await??;
                shared.set_refused(&network, None);
                drop(sessions);
                tracing::info!(%network, folder = %dir.display(), "network starts over in a new folder");
                Ok(dir)
            })
            .await?
    }

    /// Opens (or returns the already-open) session for `network`. `opts` only
    /// applies when the session is created by this call.
    pub async fn open_network(
        &self,
        network: DashNetwork,
        opts: SessionOptions,
    ) -> Result<Arc<NetworkSession>, EngineError> {
        network.validate()?;
        let shared = Arc::clone(&self.shared);
        self.handle
            .spawn(async move {
                let mut sessions = shared.sessions.lock().await;
                if let Some(existing) = sessions.get(&network) {
                    return Ok(Arc::clone(existing));
                }
                let dir = match crate::network_folders::resolve(&shared.config.data_root, &network)
                {
                    Ok(dir) => dir,
                    Err(e) => {
                        shared.set_refused(&network, None);
                        return Err(e);
                    }
                };
                let mut opened = NetworkSession::open(
                    network.clone(),
                    dir,
                    Arc::clone(&shared.sink),
                    opts,
                    shared.config.vault.clone(),
                )
                .await;
                shared.note_open(&network, &mut opened);
                let session = opened?;
                sessions.insert(network.clone(), Arc::clone(&session));
                if network == DashNetwork::Mainnet {
                    session.hub.set_rates(Arc::downgrade(&shared.rates));
                    shared.rates.mainnet_opened();
                }
                shared.sink.emit(EngineEvent::SessionOpened { network });
                Ok(session)
            })
            .await?
    }

    /// Runs `f` on `network`'s directory on the blocking pool, while no
    /// session of this engine opens or closes.
    async fn with_network_dir<T: Send + 'static>(
        &self,
        network: &DashNetwork,
        f: fn(&Path) -> Result<T, EngineError>,
    ) -> Result<T, EngineError> {
        network.validate()?;
        let _sessions = self.shared.sessions.lock().await;
        let dir = self.network_dir(network)?;
        self.handle.spawn_blocking(move || f(&dir)).await?
    }

    /// Whether `network` has a verified before-upgrade snapshot to restore.
    pub async fn restore_available(&self, network: &DashNetwork) -> Result<bool, EngineError> {
        self.with_network_dir(network, crate::store_upgrade::restore_available)
            .await
    }

    /// Asks for the restore of `network`'s verified before-upgrade snapshot
    /// at the next start of the app; nothing is restored in this process.
    /// `InvalidArgument`, with nothing read or written, while `network` is
    /// open, or unless its last open in this engine was refused by the
    /// store-upgrade check with `wayOut.restore` (the screen that offers
    /// Restore earlier data), Restore earlier data refused for a shielded
    /// hold since then included. Without a verified snapshot it is refused
    /// with `storage_restore` (`refused`, missingBackup). Refused with
    /// `ShieldedSpendHeld` while a wallet of `network` holds a shielded
    /// spend whose result is not known on disk
    /// (`store_upgrade::request_restore`); the next start reads the stores
    /// again before the restore moves anything. After an open refused with
    /// `wayOut.restore`, that refusal lets `network` start over
    /// ([`Self::recover_in_fresh_folder`]).
    pub async fn request_restore(&self, network: &DashNetwork) -> Result<(), EngineError> {
        network.validate()?;
        let sessions = self.shared.sessions.lock().await;
        if sessions.contains_key(network) {
            return Err(EngineError::InvalidArgument(format!(
                "{network} is open; only a network whose open was refused restores earlier data"
            )));
        }
        if !matches!(
            self.shared.refused(network),
            Some(Refused::Restore | Refused::RestoreHeld)
        ) {
            return Err(EngineError::InvalidArgument(format!(
                "the last open of {network} was not refused with Restore earlier data; nothing to restore from"
            )));
        }
        let dir = self.network_dir(network)?;
        let requested = self
            .handle
            .spawn_blocking(move || crate::store_upgrade::request_restore(&dir))
            .await?;
        if matches!(requested, Err(EngineError::ShieldedSpendHeld(_)))
            && self.shared.refused(network) == Some(Refused::Restore)
        {
            self.shared.set_refused(network, Some(Refused::RestoreHeld));
        }
        requested
    }

    /// The foreign-lineage screen's Keep: ends `network`'s part-done
    /// restore keeping its live files as they are; then open it again.
    pub async fn keep_current_files(&self, network: &DashNetwork) -> Result<(), EngineError> {
        self.with_network_dir(network, crate::store_upgrade::keep_current_files)
            .await
    }

    /// Drops `network`'s restore request while it hasn't begun replacing
    /// files; once it has, `storage_restore` (`incomplete`).
    pub async fn cancel_restore(&self, network: &DashNetwork) -> Result<(), EngineError> {
        self.with_network_dir(network, crate::store_upgrade::cancel_restore)
            .await
    }

    /// The open session for `network`, if any.
    pub async fn session(&self, network: &DashNetwork) -> Option<Arc<NetworkSession>> {
        self.shared.sessions.lock().await.get(network).cloned()
    }

    /// Closes the session for `network`. Returns `false` when none was open.
    /// It waits up to 30 s for a wallet load in flight, then
    /// is refused with `wallet` ("a wallet is still opening").
    /// `ShieldedSpendUnresolved`, the session left open, while one of its
    /// wallets holds a shielded spend whose result is not known (DEC-1046):
    /// the close would have to keep that wallet loaded, and the network
    /// could not reopen in this process. [`Self::shutdown`] (app quit) is
    /// never refused.
    pub async fn close_network(&self, network: DashNetwork) -> Result<bool, EngineError> {
        let never = None::<(Duration, fn(&NetworkSession) -> Option<Infallible>)>;
        match self.close(network, never).await {
            Ok(closed) => Ok(closed),
            Err(CloseRefused::Engine(e)) => Err(e),
            Err(CloseRefused::Busy(never)) => match never {},
            Err(CloseRefused::Draining | CloseRefused::Preparing) => {
                unreachable!("no drain or wait refusal without a pause")
            }
        }
    }

    /// [`Self::close_network`], refused as well when it would cut what runs
    /// on the session. After the wallet-load wait, the session's operations
    /// are paused: new ones wait. `busy` is asked at once, and again once
    /// every admitted operation has finished, waiting up to `drain` for
    /// them; a session `busy` names, or one whose operations did not finish
    /// in time, refuses the close. Then the close's own refusals. On every
    /// refusal the session goes on unchanged and the operations that
    /// waited run; once the close commits they are refused with
    /// `NetworkNotOpen`. So an operation admitted before the close, such as
    /// a flow taking a lease or mixing starting, is either seen by `busy`
    /// or finished before the session is closed: none is cut.
    pub async fn close_network_unless_busy<B: Send + 'static>(
        &self,
        network: DashNetwork,
        drain: Duration,
        busy: impl Fn(&NetworkSession) -> Option<B> + Send + Sync + 'static,
    ) -> Result<bool, CloseRefused<B>> {
        self.close(network, Some((drain, busy))).await
    }

    async fn close<B: Send + 'static>(
        &self,
        network: DashNetwork,
        pause: Option<(
            Duration,
            impl Fn(&NetworkSession) -> Option<B> + Send + Sync + 'static,
        )>,
    ) -> Result<bool, CloseRefused<B>> {
        let shared = Arc::clone(&self.shared);
        let closing = self.handle.spawn(async move {
            let Some(session) = shared.sessions.lock().await.get(&network).cloned() else {
                return Ok(false);
            };
            // Until a switch's close is decided, a shielded store's first
            // open waits for it (`shielded_pool::coordinator`) rather than
            // failing; a refusal releases the load fence before it resumes
            // the gate.
            let mut hold = session.gate.hold(pause.is_some());
            let unresolved = || {
                if pause.is_some() && session.platform.preparing() {
                    CloseRefused::Preparing
                } else {
                    CloseRefused::Engine(EngineError::ShieldedSpendUnresolved)
                }
            };
            // From the check to the commit no wallet loads and no
            // signing shielded call starts (E1 r2 R2-3). A load in
            // flight is waited for, briefly, without the sessions lock,
            // so opens, lookups and quit go on meanwhile; a call running
            // refuses the close.
            let loads = session.platform.wallet_loads.write();
            let Ok(loads) = tokio::time::timeout(close_load_wait(&session), loads).await else {
                return Err(CloseRefused::Engine(EngineError::Wallet(
                    "a wallet is still opening; close the network once it is open".into(),
                )));
            };
            hold.fence(loads);
            // After the load wait: a load takes `wallet_loads` before it
            // asks the gate, so none waits on the pause here.
            if let Some((drain, busy)) = &pause {
                // A signing shielded call holds this to read, and an
                // operation guard, for its whole run: named, not drained.
                let shielded = || session.platform.close_preflight.try_write().is_err();
                if shielded() {
                    return Err(unresolved());
                }
                hold.pause();
                match hold.check(*drain, || busy(&session)).await {
                    Ok(()) => {}
                    Err(PauseRefused::Busy(b)) => return Err(CloseRefused::Busy(b)),
                    Err(PauseRefused::Draining) if shielded() => return Err(unresolved()),
                    Err(PauseRefused::Draining) => return Err(CloseRefused::Draining),
                }
            }
            #[cfg(test)]
            crate::coins::pause_at(&session.gate.hold_before_sessions).await;
            let mut sessions = shared.sessions.lock().await;
            // Closed (quit) or reopened while it waited: nothing of this
            // session is left to close.
            if !sessions
                .get(&network)
                .is_some_and(|open| Arc::ptr_eq(open, &session))
            {
                return Ok(false);
            }
            let Ok(_preflight) = session.platform.close_preflight.try_write() else {
                return Err(unresolved());
            };
            if session.holds_shielded_spends().await {
                return Err(CloseRefused::Engine(EngineError::ShieldedSpendUnresolved));
            }
            let _loads = hold.commit();
            sessions.remove(&network);
            if network == DashNetwork::Mainnet {
                shared.rates.mainnet_closed();
            }
            session.close().await;
            Ok(true)
        });
        closing.await.map_err(|e| CloseRefused::Engine(e.into()))?
    }

    /// Closes every open session.
    pub async fn shutdown(&self) -> Result<(), EngineError> {
        let shared = Arc::clone(&self.shared);
        self.handle
            .spawn(async move {
                let mut sessions = shared.sessions.lock().await;
                let drained: Vec<_> = sessions.drain().collect();
                for (_, session) in &drained {
                    session.platform.hook_fence.close();
                }
                for (network, session) in drained {
                    if network == DashNetwork::Mainnet {
                        shared.rates.mainnet_closed();
                    }
                    session.close().await;
                }
                Ok(())
            })
            .await?
    }

    /// Runs blocking file-system work on the engine's blocking pool.
    pub(crate) async fn run_blocking<T, F>(&self, f: F) -> Result<T, EngineError>
    where
        F: FnOnce() -> Result<T, EngineError> + Send + 'static,
        T: Send + 'static,
    {
        self.handle.spawn_blocking(f).await?
    }

    /// What a user-chosen file is (QT-106/107/110), detected from its
    /// content on the blocking pool. Opens no session.
    pub async fn inspect_wallet_file(
        &self,
        path: PathBuf,
    ) -> Result<crate::WalletFileKind, EngineError> {
        self.handle
            .spawn_blocking(move || crate::compat::inspect_wallet_file(&path))
            .await?
    }

    /// Runs `fut` to completion on the engine runtime from a non-async thread
    /// (CLI, tests). Panics if called from inside an async context.
    pub fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.handle.block_on(fut)
    }
}

impl Drop for Engine {
    /// Hosts should call [`Engine::shutdown`] first. Otherwise the open
    /// sessions are closed (databases flushed and released) on a background
    /// thread that then shuts the runtime down, so the dropping thread —
    /// possibly the UI thread — never blocks (review L2). Storage is released
    /// shortly after `drop` returns, not before.
    fn drop(&mut self) {
        let Some(rt) = self.runtime.take() else {
            return;
        };
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("dw-engine-drop".into())
            // It runs the sessions' close, which may end Platform work.
            .stack_size(ENGINE_THREAD_STACK_SIZE)
            .spawn(move || {
                rt.block_on(async move {
                    let mut sessions = shared.sessions.lock().await;
                    let drained: Vec<_> = sessions.drain().collect();
                    for (_, session) in &drained {
                        session.platform.hook_fence.close();
                    }
                    for (_, session) in drained {
                        session.close().await;
                    }
                });
                rt.shutdown_background();
            });
        if let Err(e) = spawned {
            // Without a thread the sessions cannot be closed without
            // blocking here; the runtime is dropped in the background.
            tracing::warn!(error = %e, "could not start the engine drop thread");
        }
    }
}
