use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
    #[cfg(all(debug_assertions, feature = "test-support"))]
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
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Root data directory; each network lives in `<data_root>/<network>/`.
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
    /// The data root's instance lock (`dw_fs::instance`): another process's
    /// engine, dwcli's or an app's, is refused the root while this one has
    /// it.
    _instance: dw_fs::instance::InstanceLock,
}

/// Owns the tokio runtime and the open network sessions.
pub struct Engine {
    runtime: Option<Runtime>,
    handle: Handle,
    shared: Arc<Shared>,
}

impl Engine {
    pub fn new(config: EngineConfig, sink: Arc<dyn EventSink>) -> Result<Self, EngineError> {
        if config.data_root.as_os_str().is_empty() {
            return Err(EngineError::InvalidConfig("data_root is empty".into()));
        }
        if config.worker_threads == Some(0) {
            return Err(EngineError::InvalidConfig(
                "worker_threads must be > 0".into(),
            ));
        }
        create_private_dir(&config.data_root)?;
        let instance =
            dw_fs::instance::InstanceLock::share(&config.data_root).map_err(|e| match e {
                dw_fs::instance::InstanceError::Busy => EngineError::StorageInUse(format!(
                    "{} is in use by another process",
                    config.data_root.display()
                )),
                dw_fs::instance::InstanceError::Io(e) => EngineError::from(e),
            })?;
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
        Ok(Self {
            runtime: Some(runtime),
            handle,
            shared: Arc::new(Shared {
                config,
                rates,
                sink,
                sessions: Mutex::new(HashMap::new()),
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

    /// Directory used for `network`.
    pub fn network_dir(&self, network: &DashNetwork) -> PathBuf {
        self.shared.config.data_root.join(network.dir_name())
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
        let dir = self.network_dir(&network);
        self.handle
            .spawn(async move {
                let mut sessions = shared.sessions.lock().await;
                if let Some(existing) = sessions.get(&network) {
                    return Ok(Arc::clone(existing));
                }
                let session = NetworkSession::open(
                    network.clone(),
                    dir,
                    Arc::clone(&shared.sink),
                    opts,
                    shared.config.vault.clone(),
                )
                .await?;
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
            Err(CloseRefused::Draining) => unreachable!("no drain without a pause"),
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
            // From the check to the commit no wallet loads and no
            // signing shielded call starts (E1 r2 R2-3). A load in
            // flight is waited for, briefly, without the sessions lock,
            // so opens, lookups and quit go on meanwhile; a call running
            // refuses the close.
            let loads = session.platform.wallet_loads.write();
            let Ok(_loads) = tokio::time::timeout(close_load_wait(&session), loads).await else {
                return Err(CloseRefused::Engine(EngineError::Wallet(
                    "a wallet is still opening; close the network once it is open".into(),
                )));
            };
            // After the load wait: a load takes `wallet_loads` before it
            // asks the gate, so none waits on the pause here.
            let paused = match &pause {
                Some((drain, busy)) => Some(
                    session
                        .gate
                        .pause(*drain, || busy(&session))
                        .await
                        .map_err(|refused| match refused {
                            PauseRefused::Busy(b) => CloseRefused::Busy(b),
                            PauseRefused::Draining => CloseRefused::Draining,
                        })?,
                ),
                None => None,
            };
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
                return Err(CloseRefused::Engine(EngineError::ShieldedSpendUnresolved));
            };
            if session.holds_shielded_spends().await {
                return Err(CloseRefused::Engine(EngineError::ShieldedSpendUnresolved));
            }
            if let Some(paused) = paused {
                paused.seal();
            }
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
                for (network, session) in sessions.drain() {
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
                    for (_, session) in sessions.drain() {
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
