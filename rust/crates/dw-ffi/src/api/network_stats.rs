//! M3 Tools → Information → Network sub-tab (QT-144). Owner: R1. Values an
//! SPV client cannot know are `None` (DESIGN-opus §1.14): the credit pool
//! and the InstantSend counters need a full node (or the dashd RPC data
//! source, post-M3); the mempool rows stay `None` in `NodeInfo` (M2).
//! Contract: docs/contracts/m3-engine.md §2.6.

use crate::api::common::{ensure_open, not_implemented};
use crate::{ChainLockInfo, MasternodeCount, NetworkSession, SyncError};

/// "Credit Pool" (Core `getcreditpoolinfo`): full node only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CreditPoolInfo {
    pub last_block_change: u32,
    pub total_locked: u64,
    pub pending_unlocks: u64,
    pub withdrawal_limit: u64,
}

/// "InstantSend" counters (Core `getislocks`/mempool counters): full node
/// only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct InstantSendCounters {
    pub verified: u32,
    pub unverified: u32,
    pub awaiting_tx: u32,
    pub unprotected_txs: u32,
}

/// One LLMQ type in the masternode list ("N active (x.x% health)").
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct QuorumSummary {
    /// Core name, e.g. `llmq_400_60`.
    pub llmq_name: String,
    pub llmq_type: u8,
    pub active: u32,
    /// Share of the type's quorums that are valid, 0–100; `None` when the
    /// list does not say.
    pub health_percent: Option<f64>,
    pub rotated: bool,
}

/// The Network sub-tab.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct NetworkStats {
    /// `None` on SPV.
    pub credit_pool: Option<CreditPoolInfo>,
    /// `None` on SPV.
    pub instantsend: Option<InstantSendCounters>,
    /// From the SML, as `NodeInfo` (M2).
    pub masternodes: Option<MasternodeCount>,
    pub evonodes: Option<MasternodeCount>,
    pub best_chainlock: Option<ChainLockInfo>,
    /// From the quorum list of the synced masternode list; empty before it
    /// synced.
    pub quorums: Vec<QuorumSummary>,
}

#[uniffi::export]
impl NetworkSession {
    /// The Network sub-tab. In-memory read; re-query on `Sync` and
    /// `Masternodes` events.
    pub fn network_stats(&self) -> Result<NetworkStats, SyncError> {
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.network_stats")
    }
}
