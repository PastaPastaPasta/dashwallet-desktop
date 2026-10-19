//! The governance facade (MG-02, `docs/contracts/mg-engine.md`): plain Rust
//! that `dwcli` and the console drive directly and E0-13 binds for the UI.
//! One [`Governance`] handle per call site, all on the session's shared
//! state (`sync.rs`): the store govsync fills, the list snapshot it was fed
//! and the sync task.
//!
//! No call signs with, takes or returns an operator key (MG-01's guard).

pub(crate) mod collateral;
mod errors;
mod reads;
mod records;
pub(crate) mod sync;
mod voting;

use std::sync::Arc;

pub use errors::GovernanceError;
pub use records::{
    CurrentVoteRow, GovernanceClock, GovernanceInfo, GovernanceSyncPhase, GovernanceSyncState,
    MasternodeType, ProposalCounts, ProposalDetail, ProposalQuery, ProposalRow, ProposalScope,
    ProposalStatus, RawVote, SignalTallies, SuperblockDate, VoteOutcome, VoteRelayed, VoteSignal,
    VoteTally, VotingMasternode, WeightRange,
};

use crate::{DashNetwork, EngineError, NetworkSession};

/// Governance of one network session. A cheap handle: the state lives in
/// the session, so any number of handles may exist (mg-engine §0).
#[derive(Clone)]
pub struct Governance {
    pub(super) session: Arc<NetworkSession>,
}

impl NetworkSession {
    /// The governance facade of this session.
    pub fn governance(self: &Arc<Self>) -> Governance {
        Governance {
            session: Arc::clone(self),
        }
    }
}

impl Governance {
    /// Whether governance sync is on (`governance.sync_enabled`, read at
    /// session open; default off).
    pub fn sync_enabled(&self) -> Result<bool, GovernanceError> {
        Ok(self.runtime()?.enabled())
    }

    /// Persists the setting, then starts or stops the sync task. Stopping
    /// keeps the store. Idempotent.
    pub async fn set_sync_enabled(&self, enabled: bool) -> Result<(), GovernanceError> {
        let session = Arc::clone(&self.session);
        self.session
            .on_runtime(async move {
                let _op = session.enter().await?;
                let appdb = session.live()?.appdb;
                let value = if enabled { "1" } else { "0" };
                tokio::task::spawn_blocking(move || {
                    appdb.set_setting(dw_appdb::GLOBAL_SCOPE, sync::SYNC_ENABLED_KEY, Some(value))
                })
                .await??;
                session.governance.set_enabled(&session, enabled);
                Ok::<_, EngineError>(())
            })
            .await?;
        Ok(())
    }

    /// The sync phase (mg-engine §5) and its counters.
    pub fn sync_state(&self) -> Result<GovernanceSyncState, GovernanceError> {
        let p = self.runtime()?.shared.progress().clone();
        Ok(GovernanceSyncState {
            phase: p.phase.into(),
            objects: p.objects,
            unconfirmed_objects: p.unconfirmed_objects,
            votes: p.votes,
            orphan_votes: p.orphan_votes,
            peers: p.peers,
            bytes_received: p.bytes_received,
            last_synced_at: p.last_synced_at,
            last_error: p.last_error,
            next_retry_at: p.next_retry_at,
            objects_secs: p.objects_secs,
            votes_secs: p.votes_secs,
        })
    }

    /// Starts a new run now (after `synced` or during a backoff).
    pub fn resync(&self) -> Result<(), GovernanceError> {
        let rt = self.runtime()?;
        if !rt.enabled() {
            return Err(GovernanceError::SyncDisabled);
        }
        rt.resync();
        Ok(())
    }
}

/// Core's `CSuperblock::GetPaymentsLimit` in duffs: the budget of the
/// superblock at `height`, 0 for a height that is not one.
pub fn superblock_budget(network: &DashNetwork, height: u32) -> u64 {
    dw_governance::clock::superblock_budget(network.core_network(), height)
}

/// A contract stub's body (`governance.not_implemented{call}`).
fn stub<T>(call: &str) -> Result<T, GovernanceError> {
    Err(GovernanceError::NotImplemented { call: call.into() })
}
