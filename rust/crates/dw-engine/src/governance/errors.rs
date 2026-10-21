//! `GovernanceError`, the facade's error with its stable codes
//! (`docs/contracts/mg-engine.md` §4). The `Display` text is diagnostic
//! detail for logs; the UI picks its copy by `code()`. The m1 common codes
//! (`network_not_open`, `wallet_not_found`, …) come through `Engine`
//! unchanged, so no m1–m3 table gains a code (MG-02 D8).

use crate::error::EngineError;
use crate::platform::PlatformError;

use super::records::{GovernanceSyncPhase, ProposalField, SubmitRefusal};

#[derive(Debug, thiserror::Error)]
pub enum GovernanceError {
    /// Governance sync is off and the call needs it.
    #[error("governance sync is disabled")]
    SyncDisabled,
    /// No sync run reached `synced` yet (MG-00 D21), or no tip is known.
    #[error("governance is not synced ({phase:?})")]
    NotSynced { phase: GovernanceSyncPhase },
    /// Unknown, unconfirmed (MG-02 D3) or not a proposal.
    #[error("proposal not found: {hash}")]
    ProposalNotFound { hash: String },
    /// A relayed vote failed a check; `reason` is the store's rejection in
    /// snake case.
    #[error("vote rejected: {reason}")]
    VoteRejected { reason: String },
    /// No full node to talk to.
    #[error("no governance peers")]
    NoPeers,
    /// The call needs the masternode list and SPV has none yet.
    #[error("masternode list unavailable")]
    ListUnavailable,
    /// A proposal draft failed validation: every failing field, in wizard
    /// order (MG-06 D11).
    #[error("invalid proposal: {fields:?}")]
    InvalidProposal { fields: Vec<ProposalField> },
    /// `submit_proposal` on a proposal past submitting.
    #[error("submit refused: {}", reason.as_str())]
    SubmitRefused { reason: SubmitRefusal },
    /// No DPNS poll for the label: no contender and no stored result.
    #[error("no contest for {label}")]
    ContestNotFound { label: String },
    /// The poll resolved, or its voting ended (MG-07 D8).
    #[error("the contest for {label} is closed")]
    ContestClosed { label: String },
    /// A vote towards an identity the poll does not list (MG-07 D9).
    #[error("{identity} is not a contender")]
    NotAContender { identity: String },
    /// A contract stub (`call` is `"Governance.<method>"`).
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    #[error(transparent)]
    Engine(#[from] EngineError),
    /// The grant's lease refused (`vote`, MG-05 §5): its `platform.*` code.
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl GovernanceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::SyncDisabled => "governance.sync_disabled",
            Self::NotSynced { .. } => "governance.not_synced",
            Self::ProposalNotFound { .. } => "governance.proposal_not_found",
            Self::VoteRejected { .. } => "governance.vote_rejected",
            Self::NoPeers => "governance.no_peers",
            Self::ListUnavailable => "governance.list_unavailable",
            Self::InvalidProposal { .. } => "governance.invalid_proposal",
            Self::SubmitRefused { .. } => "governance.submit_refused",
            Self::ContestNotFound { .. } => "governance.contest_not_found",
            Self::ContestClosed { .. } => "governance.contest_closed",
            Self::NotAContender { .. } => "governance.not_a_contender",
            Self::NotImplemented { .. } => "governance.not_implemented",
            Self::Engine(e) => e.code(),
            Self::Platform(e) => e.code(),
        }
    }

    /// The wrapped engine error, if any.
    pub fn engine(&self) -> Option<&EngineError> {
        match self {
            Self::Engine(e) => Some(e),
            _ => None,
        }
    }
}
