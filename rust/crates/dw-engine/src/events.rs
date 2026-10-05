use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dash_spv::network::NetworkEvent;
use dash_spv::sync::SyncProgress;
use dash_spv::EventHandler;
use platform_wallet::events::WalletEvent;
use platform_wallet::PlatformEventHandler;

use crate::{DashNetwork, WalletId};

/// Engine → host signal. Events say *what changed*; hosts pull the data again
/// (DESIGN-opus §1.5 rule 4).
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    SessionOpened {
        network: DashNetwork,
    },
    SessionClosed {
        network: DashNetwork,
    },
    WalletCreated {
        network: DashNetwork,
        wallet_id: WalletId,
    },
    /// Something about this wallet changed (tx seen, IS lock, block processed).
    WalletChanged {
        network: DashNetwork,
        wallet_id: WalletId,
    },
    SpvStateChanged {
        network: DashNetwork,
        running: bool,
    },
    /// Throttled to at most [`PROGRESS_MIN_INTERVAL`] per network.
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
}

/// Non-fatal conditions the UI may surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeCode {
    /// The trusted quorum service could not be reached; Platform proof
    /// verification retries lazily.
    PlatformContextUnavailable,
    /// dash-spv reported a fatal sync error.
    SpvError,
    /// Session shutdown left a worker running (see the detail text).
    UncleanShutdown,
}

/// Receives engine events. Called from engine threads; must not block.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: EngineEvent);
}

/// Minimum spacing between `SyncProgress` events (≤4 Hz, DESIGN-opus §1.5).
pub const PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Bridges platform-wallet / dash-spv callbacks of one session to the sink.
pub(crate) struct SessionEventBridge {
    network: DashNetwork,
    sink: Arc<dyn EventSink>,
    last_progress: Mutex<Option<Instant>>,
}

impl SessionEventBridge {
    pub(crate) fn new(network: DashNetwork, sink: Arc<dyn EventSink>) -> Self {
        Self {
            network,
            sink,
            last_progress: Mutex::new(None),
        }
    }

    fn progress_due(&self) -> bool {
        let mut last = self.last_progress.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < PROGRESS_MIN_INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }
}

impl EventHandler for SessionEventBridge {
    fn on_progress(&self, progress: &SyncProgress) {
        if !self.progress_due() {
            return;
        }
        self.sink.emit(EngineEvent::SyncProgress {
            network: self.network.clone(),
            header_tip_height: progress.headers().ok().map(|h| h.tip_height()),
            synced: progress.is_synced(),
        });
    }

    fn on_network_event(&self, event: &NetworkEvent) {
        if let NetworkEvent::PeersUpdated {
            connected_count, ..
        } = event
        {
            self.sink.emit(EngineEvent::PeersChanged {
                network: self.network.clone(),
                connected: u32::try_from(*connected_count).unwrap_or(u32::MAX),
            });
        }
    }

    fn on_wallet_event(&self, event: &WalletEvent) {
        self.sink.emit(EngineEvent::WalletChanged {
            network: self.network.clone(),
            wallet_id: WalletId(event.wallet_id()),
        });
    }

    fn on_error(&self, error: &str) {
        self.sink.emit(EngineEvent::Notice {
            network: Some(self.network.clone()),
            code: NoticeCode::SpvError,
            detail: error.to_string(),
        });
    }
}

impl PlatformEventHandler for SessionEventBridge {}
