use std::path::PathBuf;
use std::sync::Arc;

use crate::{
    EngineError, NetworkSession, SessionOptions, SyncSnapshot, VaultLockState, WalletBalances,
};

/// Version string of the Rust core (crate version).
#[uniffi::export]
pub fn core_version() -> String {
    format!("dashwallet_core {}", env!("CARGO_PKG_VERSION"))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum DashNetwork {
    Mainnet,
    Testnet,
    Devnet { name: String },
    Regtest,
}

impl From<DashNetwork> for dw_engine::DashNetwork {
    fn from(n: DashNetwork) -> Self {
        match n {
            DashNetwork::Mainnet => Self::Mainnet,
            DashNetwork::Testnet => Self::Testnet,
            DashNetwork::Devnet { name } => Self::Devnet { name },
            DashNetwork::Regtest => Self::Regtest,
        }
    }
}

impl From<dw_engine::DashNetwork> for DashNetwork {
    fn from(n: dw_engine::DashNetwork) -> Self {
        match n {
            dw_engine::DashNetwork::Mainnet => Self::Mainnet,
            dw_engine::DashNetwork::Testnet => Self::Testnet,
            dw_engine::DashNetwork::Devnet { name } => Self::Devnet { name },
            dw_engine::DashNetwork::Regtest => Self::Regtest,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineConfig {
    /// Root data directory; networks live in `<data_root>/<network>/`.
    pub data_root: String,
    /// tokio worker threads; `None` = one per core.
    pub worker_threads: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum NoticeCode {
    PlatformContextUnavailable,
    SpvError,
    UncleanShutdown,
    /// SPV made no progress for 45 s while not caught up (IOS-023); the host
    /// may offer `rotate_peers`.
    SyncStalled,
    /// An automatic wallet backup failed (QT-116).
    BackupFailed,
}

impl From<dw_engine::NoticeCode> for NoticeCode {
    fn from(c: dw_engine::NoticeCode) -> Self {
        match c {
            dw_engine::NoticeCode::PlatformContextUnavailable => Self::PlatformContextUnavailable,
            dw_engine::NoticeCode::SpvError => Self::SpvError,
            dw_engine::NoticeCode::UncleanShutdown => Self::UncleanShutdown,
        }
    }
}

/// Engine → host signal; the host re-queries data when it arrives
/// (DESIGN-opus §1.5 rule 4). Each domain is debounced in Rust to at most
/// 4 Hz, and the last change of a burst is always delivered.
///
/// M0 variants (`SyncProgress`, `PeersChanged`, `WalletChanged`) stay until
/// E1 emits `Sync`, `Balances` and `HistoryChanged`; E1 then removes them.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum EngineEvent {
    SessionOpened {
        network: DashNetwork,
    },
    SessionClosed {
        network: DashNetwork,
    },
    WalletCreated {
        network: DashNetwork,
        wallet_id: String,
    },
    WalletChanged {
        network: DashNetwork,
        wallet_id: String,
    },
    SpvStateChanged {
        network: DashNetwork,
        running: bool,
    },
    SyncProgress {
        network: DashNetwork,
        header_tip_height: Option<u32>,
        synced: bool,
    },
    PeersChanged {
        network: DashNetwork,
        connected: u32,
    },
    Notice {
        network: Option<DashNetwork>,
        code: NoticeCode,
        detail: String,
    },
    /// New sync state; same value `NetworkSession::sync_snapshot` returns.
    Sync {
        network: DashNetwork,
        snapshot: SyncSnapshot,
    },
    /// The wallet's balance buckets changed.
    Balances {
        network: DashNetwork,
        wallet_id: String,
        balances: WalletBalances,
    },
    /// Transactions of the wallet were added or changed status; re-query
    /// `history_page`. `txids` lists the affected ones when known (empty =
    /// reload everything).
    HistoryChanged {
        network: DashNetwork,
        wallet_id: String,
        txids: Vec<String>,
    },
    WalletRemoved {
        network: DashNetwork,
        wallet_id: String,
    },
    /// The vault of `network` changed lock state.
    LockState {
        network: DashNetwork,
        state: VaultLockState,
    },
}

impl From<dw_engine::EngineEvent> for EngineEvent {
    fn from(e: dw_engine::EngineEvent) -> Self {
        use dw_engine::EngineEvent as E;
        match e {
            E::SessionOpened { network } => Self::SessionOpened {
                network: network.into(),
            },
            E::SessionClosed { network } => Self::SessionClosed {
                network: network.into(),
            },
            E::WalletCreated { network, wallet_id } => Self::WalletCreated {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
            },
            E::WalletChanged { network, wallet_id } => Self::WalletChanged {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
            },
            E::SpvStateChanged { network, running } => Self::SpvStateChanged {
                network: network.into(),
                running,
            },
            E::SyncProgress {
                network,
                header_tip_height,
                synced,
            } => Self::SyncProgress {
                network: network.into(),
                header_tip_height,
                synced,
            },
            E::PeersChanged { network, connected } => Self::PeersChanged {
                network: network.into(),
                connected,
            },
            E::Notice {
                network,
                code,
                detail,
            } => Self::Notice {
                network: network.map(Into::into),
                code: code.into(),
                detail,
            },
            E::VaultLockState { network, state } => Self::LockState {
                network: network.into(),
                state: state.into(),
            },
        }
    }
}

/// Implemented by the host (Swift `EventBus`). Called on engine threads; must
/// return quickly and must not call back into the engine synchronously.
#[uniffi::export(with_foreign)]
pub trait EngineObserver: Send + Sync {
    fn on_event(&self, event: EngineEvent);
}

struct ObserverSink(Arc<dyn EngineObserver>);

impl dw_engine::EventSink for ObserverSink {
    fn emit(&self, event: dw_engine::EngineEvent) {
        self.0.on_event(event.into());
    }
}

#[derive(uniffi::Object)]
pub struct Engine {
    inner: dw_engine::Engine,
}

#[uniffi::export]
impl Engine {
    #[uniffi::constructor]
    pub fn new(
        config: EngineConfig,
        observer: Arc<dyn EngineObserver>,
    ) -> Result<Arc<Self>, EngineError> {
        let inner = dw_engine::Engine::new(
            dw_engine::EngineConfig {
                data_root: PathBuf::from(config.data_root),
                worker_threads: config.worker_threads.map(|n| n as usize),
                // Calibrated Argon2id and the OS keyring for slot O.
                vault: dw_vault::VaultConfig::default(),
            },
            Arc::new(ObserverSink(observer)),
        )?;
        Ok(Arc::new(Self { inner }))
    }

    /// Opens (or returns the open) session for `network`.
    pub async fn open_network(
        &self,
        network: DashNetwork,
        options: SessionOptions,
    ) -> Result<Arc<NetworkSession>, EngineError> {
        let session = self
            .inner
            .open_network(network.into(), options.into())
            .await?;
        Ok(Arc::new(NetworkSession::wrap(session)))
    }

    /// Closes the session for `network`; `false` when none was open.
    pub async fn close_network(&self, network: DashNetwork) -> Result<bool, EngineError> {
        Ok(self.inner.close_network(network.into()).await?)
    }

    /// Closes every session. Call before releasing the engine.
    pub async fn shutdown(&self) -> Result<(), EngineError> {
        Ok(self.inner.shutdown().await?)
    }

    /// Data directory used for `network` ("Open data folder").
    pub fn network_dir(&self, network: DashNetwork) -> String {
        self.inner
            .network_dir(&network.into())
            .to_string_lossy()
            .into_owned()
    }
}
