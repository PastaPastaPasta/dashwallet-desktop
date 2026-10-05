//! SPV sync status, peers and rescans. Owner: E1 (engine-core).
//! `start_spv` / `stop_spv` / `spv_running` live in `session.rs` (M0).
//! Contract: docs/contracts/m1-engine.md §sync.

use crate::NetworkSession;
use crate::api::common::{domain_error_common, not_implemented};

/// dash-spv sync phases, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum SyncPhase {
    Headers,
    FilterHeaders,
    Filters,
    Masternodes,
}

/// Progress of one phase. Heights are `None` until dash-spv reports them.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncPhaseProgress {
    pub phase: SyncPhase,
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub done: bool,
}

/// Whole-network sync state (QT-024/025/027, IOS-023). Raw values: damping
/// and the 45 s stall rule are the host's `SPVCoordinator` job.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SyncSnapshot {
    pub running: bool,
    /// One row per phase, in `SyncPhase` order.
    pub phases: Vec<SyncPhaseProgress>,
    /// The phase dash-spv is working on; `None` when idle or caught up.
    pub active_phase: Option<SyncPhase>,
    /// Best header height.
    pub tip_height: Option<u32>,
    /// Timestamp of the best header (UNIX seconds), for "x behind" text.
    pub tip_time: Option<u64>,
    pub chainlock_height: Option<u32>,
    pub connected_peers: u32,
    /// dash-spv reached its steady state (all phases done, waiting for
    /// events). Hosts gate on this, never on a progress fraction.
    pub caught_up: bool,
    /// Seconds since dash-spv last made progress while not caught up.
    pub seconds_since_progress: Option<u64>,
}

/// A connected SPV peer (QT-147, IOS-023).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerInfo {
    /// `ip:port`.
    pub address: String,
    pub user_agent: Option<String>,
    pub protocol_version: Option<u32>,
    pub best_height: Option<u32>,
    pub ping_ms: Option<u32>,
    /// UNIX seconds.
    pub connected_since: Option<u64>,
    pub inbound: bool,
    pub bytes_sent: Option<u64>,
    pub bytes_received: Option<u64>,
}

/// Where `rescan` starts (QT-117, QT-148, IOS-113).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum RescanFrom {
    /// The earliest birth height of the network's wallets.
    WalletBirth,
    Genesis,
    Height {
        height: u32,
    },
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SyncError {
    /// Code `sync.spv_not_running`: the call needs a running SPV client.
    #[error("spv not running")]
    SpvNotRunning,
    /// Code `sync.height_out_of_range`: above the tip or below the first checkpoint.
    #[error("height {height} out of range")]
    HeightOutOfRange { height: u32 },
    /// Code `sync.spv`: dash-spv failed; `detail` is diagnostic.
    #[error("spv: {detail}")]
    Spv { detail: String },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(SyncError);

impl SyncError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::SpvNotRunning => "sync.spv_not_running",
            Self::HeightOutOfRange { .. } => "sync.height_out_of_range",
            Self::Spv { .. } => "sync.spv",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// Current sync state. In-memory read; also pushed as `EngineEvent::Sync`.
    pub fn sync_snapshot(&self) -> Result<SyncSnapshot, SyncError> {
        not_implemented("NetworkSession.sync_snapshot")
    }

    /// Connected peers. In-memory read.
    pub fn peers(&self) -> Result<Vec<PeerInfo>, SyncError> {
        not_implemented("NetworkSession.peers")
    }

    /// Disconnects the current peers and connects to new ones (IOS-023
    /// "Change peers" after a stall).
    pub async fn rotate_peers(&self) -> Result<(), SyncError> {
        not_implemented("NetworkSession.rotate_peers")
    }

    /// Re-scans compact filters from `from` for every wallet on the network.
    /// Returns once the rescan is scheduled; progress arrives as `Sync` events.
    pub async fn rescan(&self, from: RescanFrom) -> Result<(), SyncError> {
        let _ = from;
        not_implemented("NetworkSession.rescan")
    }
}
