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
    /// SPV made no progress for 45 s while not caught up (IOS-023). The
    /// engine owns the rule and sends one notice per stall; the host may
    /// offer `rotate_peers`.
    SyncStalled,
    /// An automatic wallet backup failed (QT-116); `detail` names the wallet
    /// and the cause.
    BackupFailed,
    /// `remove_wallet` removed the wallet but could not delete its seed from
    /// the vault (e.g. the vault was locked meanwhile). `detail` names the
    /// wallet id.
    WalletSecretNotDeleted,
}

impl NoticeCode {
    /// The binding's code; `None` for the engine-side codes the frozen
    /// bindings do not carry yet (E0-04 §16.8; E0-13 forwards them).
    fn forward(c: dw_engine::NoticeCode) -> Option<Self> {
        use dw_engine::NoticeCode as C;
        Some(match c {
            C::PlatformContextUnavailable => Self::PlatformContextUnavailable,
            C::SpvError => Self::SpvError,
            C::UncleanShutdown => Self::UncleanShutdown,
            C::SyncStalled => Self::SyncStalled,
            C::BackupFailed => Self::BackupFailed,
            C::WalletSecretNotDeleted => Self::WalletSecretNotDeleted,
            // Withheld until E0-13 binds them.
            C::DashPayStartupIncomplete
            | C::DispatchRecordMissing
            | C::UnscopedDispatch
            | C::DispatchJournalUnavailable => return None,
        })
    }
}

/// Engine → host signal; the host re-queries data when it arrives
/// (DESIGN-opus §1.5 rule 4). `Sync`, `Balances` and `HistoryChanged` are
/// debounced in Rust to at most 4 Hz per domain, and the last change of a
/// burst is always delivered.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum EngineEvent {
    SessionOpened {
        network: DashNetwork,
    },
    SessionClosed {
        network: DashNetwork,
    },
    /// A wallet was registered, or keys were attached to a registered
    /// wallet; reload the wallet list.
    WalletCreated {
        network: DashNetwork,
        wallet_id: String,
    },
    SpvStateChanged {
        network: DashNetwork,
        running: bool,
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
    /// The wallet's balance buckets changed. `None` while the balance is
    /// not known yet (the scan has not reached the birth height).
    Balances {
        network: DashNetwork,
        wallet_id: String,
        balances: Option<WalletBalances>,
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
    /// M2 (QT-031…033, IOS-116): transactions of the wallet seen for the
    /// first time, batched over 100 ms as dash-qt batches its popups. Sent
    /// once per transaction, never for status changes (those are
    /// `HistoryChanged`). `catch_up` is true while SPV is not caught up
    /// (dash-qt shows no popups during initial block download). The host
    /// reads the rows with `NetworkSession::tx_notices`.
    NewTransactions {
        network: DashNetwork,
        wallet_id: String,
        txids: Vec<String>,
        catch_up: bool,
    },
    /// M2 (QT-101): a wallet was loaded (opened) or unloaded (closed) without
    /// being removed. Reload the wallet list and `wallet_load_states`.
    WalletLoadChanged {
        network: DashNetwork,
        wallet_id: String,
        loaded: bool,
    },
    /// M3 (QT-041…050): the wallet's mixing status, CoinJoin balances or
    /// progress changed; re-query `coinjoin_status`. At most once a second
    /// per wallet (dash-qt's panel timer), trailing edge kept.
    CoinJoin {
        network: DashNetwork,
        wallet_id: String,
    },
}

impl EngineEvent {
    /// The binding's event; `None` for the engine-side variants the frozen
    /// bindings do not carry yet (E0-04 §13; E0-13 forwards them).
    pub(crate) fn forward(e: dw_engine::EngineEvent) -> Option<Self> {
        use dw_engine::EngineEvent as E;
        Some(match e {
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
            E::WalletRemoved { network, wallet_id } => Self::WalletRemoved {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
            },
            E::SpvStateChanged { network, running } => Self::SpvStateChanged {
                network: network.into(),
                running,
            },
            E::Sync { network, snapshot } => Self::Sync {
                network: network.into(),
                snapshot: snapshot.into(),
            },
            E::Balances {
                network,
                wallet_id,
                balances,
            } => Self::Balances {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
                balances: balances.map(Into::into),
            },
            E::HistoryChanged {
                network,
                wallet_id,
                txids,
            } => Self::HistoryChanged {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
                txids: txids.iter().map(ToString::to_string).collect(),
            },
            E::Notice {
                network,
                code,
                detail,
            } => Self::Notice {
                network: network.map(Into::into),
                code: NoticeCode::forward(code)?,
                detail,
            },
            E::VaultLockState { network, state } => Self::LockState {
                network: network.into(),
                state: state.into(),
            },
            E::NewTransactions {
                network,
                wallet_id,
                txids,
                catch_up,
            } => Self::NewTransactions {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
                txids: txids.iter().map(ToString::to_string).collect(),
                catch_up,
            },
            E::WalletLoadChanged {
                network,
                wallet_id,
                loaded,
            } => Self::WalletLoadChanged {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
                loaded,
            },
            E::CoinJoin { network, wallet_id } => Self::CoinJoin {
                network: network.into(),
                wallet_id: wallet_id.to_string(),
            },
            // Withheld until E0-13 binds them: the DashPay signals (E0-06)
            // and E0-04's lease, lock and dispatch events.
            E::Platform { .. }
            | E::LeaseChanged { .. }
            | E::LockProgress { .. }
            | E::DispatchResolved { .. } => return None,
        })
    }
}

/// Implemented by the host (Swift `EventBus`). Called on engine threads; must
/// return quickly and must not call back into the engine synchronously.
#[uniffi::export(with_foreign)]
pub trait EngineObserver: Send + Sync {
    fn on_event(&self, event: EngineEvent);
}

pub(crate) struct ObserverSink(pub(crate) Arc<dyn EngineObserver>);

impl dw_engine::EventSink for ObserverSink {
    fn emit(&self, event: dw_engine::EngineEvent) {
        if let Some(event) = EngineEvent::forward(event) {
            self.0.on_event(event);
        }
    }
}

#[derive(uniffi::Object)]
pub struct Engine {
    pub(crate) inner: dw_engine::Engine,
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
