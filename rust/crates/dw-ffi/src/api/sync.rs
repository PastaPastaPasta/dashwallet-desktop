//! SPV sync status, peers and rescans. Owner: E1 (engine-core).
//! `start_spv` / `stop_spv` / `spv_running` live in `session.rs` (M0).
//! Contract: docs/contracts/m1-engine.md §sync.

use crate::NetworkSession;
use crate::api::common::domain_error_common;

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
/// is the host's `SPVCoordinator` job. The 45 s stall rule is the engine's:
/// it sends `NoticeCode::SyncStalled` once per stall.
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
    /// Code `sync.spv_running` (M2, QT-148): the call needs SPV stopped
    /// (resetting chain data).
    #[error("spv running")]
    SpvRunning,
    /// Code `sync.rescan_in_progress` (M2, QT-117): a rescan is already
    /// running (dash-qt "Wallet is currently rescanning").
    #[error("rescan in progress")]
    RescanInProgress,
    /// Code `sync.peer_not_found` (M2, QT-147): no connected or banned peer
    /// with that address.
    #[error("peer {address} not found")]
    PeerNotFound { address: String },
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

domain_error_common!(@not_implemented SyncError);

impl From<dw_engine::EngineError> for SyncError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::SpvNotRunning => Self::SpvNotRunning,
            E::HeightOutOfRange(height) => Self::HeightOutOfRange { height },
            E::Spv(_) => Self::Spv { detail },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

impl From<dw_engine::SyncPhase> for SyncPhase {
    fn from(p: dw_engine::SyncPhase) -> Self {
        match p {
            dw_engine::SyncPhase::Headers => Self::Headers,
            dw_engine::SyncPhase::FilterHeaders => Self::FilterHeaders,
            dw_engine::SyncPhase::Filters => Self::Filters,
            dw_engine::SyncPhase::Masternodes => Self::Masternodes,
        }
    }
}

impl From<dw_engine::SyncSnapshot> for SyncSnapshot {
    fn from(s: dw_engine::SyncSnapshot) -> Self {
        Self {
            running: s.running,
            phases: s
                .phases
                .into_iter()
                .map(|p| SyncPhaseProgress {
                    phase: p.phase.into(),
                    current_height: p.current_height,
                    target_height: p.target_height,
                    done: p.done,
                })
                .collect(),
            active_phase: s.active_phase.map(Into::into),
            tip_height: s.tip_height,
            tip_time: s.tip_time,
            chainlock_height: s.chainlock_height,
            connected_peers: s.connected_peers,
            caught_up: s.caught_up,
            seconds_since_progress: s.seconds_since_progress,
        }
    }
}

impl From<dw_engine::PeerInfo> for PeerInfo {
    fn from(p: dw_engine::PeerInfo) -> Self {
        Self {
            address: p.address,
            user_agent: p.user_agent,
            protocol_version: p.protocol_version,
            best_height: p.best_height,
            ping_ms: p.ping_ms,
            connected_since: p.connected_since,
            inbound: p.inbound,
            bytes_sent: p.bytes_sent,
            bytes_received: p.bytes_received,
        }
    }
}

impl From<RescanFrom> for dw_engine::RescanFrom {
    fn from(r: RescanFrom) -> Self {
        match r {
            RescanFrom::WalletBirth => Self::WalletBirth,
            RescanFrom::Genesis => Self::Genesis,
            RescanFrom::Height { height } => Self::Height(height),
        }
    }
}

crate::api::common::export_error_code!(SyncError);

impl SyncError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
        match self {
            Self::SpvNotRunning => "sync.spv_not_running",
            Self::HeightOutOfRange { .. } => "sync.height_out_of_range",
            Self::Spv { .. } => "sync.spv",
            Self::SpvRunning => "sync.spv_running",
            Self::RescanInProgress => "sync.rescan_in_progress",
            Self::PeerNotFound { .. } => "sync.peer_not_found",
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
        Ok(self.inner.sync_snapshot()?.into())
    }

    /// Connected peers. In-memory read. dash-spv reports only addresses, so
    /// the other fields are `None` until it exposes per-peer data.
    pub fn peers(&self) -> Result<Vec<PeerInfo>, SyncError> {
        Ok(self.inner.peers()?.into_iter().map(Into::into).collect())
    }

    /// Disconnects the current peers and connects again (IOS-023 "Change
    /// peers" after a stall). Restarts the SPV client: with configured peers
    /// only, the same peers are dialled again.
    pub async fn rotate_peers(&self) -> Result<(), SyncError> {
        Ok(self.inner.rotate_peers().await?)
    }

    /// Re-scans compact filters from `from` for every wallet on the network.
    /// Returns once the rescan is scheduled; progress arrives as `Sync`
    /// events. The rewind is not persisted: after a restart, call it again.
    pub async fn rescan(&self, from: RescanFrom) -> Result<(), SyncError> {
        Ok(self.inner.rescan(from.into()).await?)
    }
}
