use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::runtime::{Handle, Runtime};
use tokio::sync::Mutex;

use crate::fsutil::create_private_dir;
use crate::{DashNetwork, EngineError, EngineEvent, EventSink, NetworkSession, SessionOptions};

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
    sink: Arc<dyn EventSink>,
    /// Lifecycle operations (open/close) are serialised by this lock, the
    /// desktop analogue of iOS's `SerialAsyncLifecycleQueue` at engine level.
    sessions: Mutex<HashMap<DashNetwork, Arc<NetworkSession>>>,
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
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all().thread_name("dw-engine");
        if let Some(n) = config.worker_threads {
            builder.worker_threads(n);
        }
        let runtime = builder
            .build()
            .map_err(|e| EngineError::Internal(format!("tokio runtime: {e}")))?;
        let handle = runtime.handle().clone();
        Ok(Self {
            runtime: Some(runtime),
            handle,
            shared: Arc::new(Shared {
                config,
                sink,
                sessions: Mutex::new(HashMap::new()),
            }),
        })
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
    pub async fn close_network(&self, network: DashNetwork) -> Result<bool, EngineError> {
        let shared = Arc::clone(&self.shared);
        self.handle
            .spawn(async move {
                let mut sessions = shared.sessions.lock().await;
                let Some(session) = sessions.remove(&network) else {
                    return Ok(false);
                };
                session.close().await;
                Ok(true)
            })
            .await?
    }

    /// Closes every open session.
    pub async fn shutdown(&self) -> Result<(), EngineError> {
        let shared = Arc::clone(&self.shared);
        self.handle
            .spawn(async move {
                let mut sessions = shared.sessions.lock().await;
                for (_, session) in sessions.drain() {
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
