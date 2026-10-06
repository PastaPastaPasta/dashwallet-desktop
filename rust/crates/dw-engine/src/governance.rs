//! Governance in the engine (M3, owner R2; docs/contracts/m3-engine.md
//! §2.2): the govsync lifecycle over `dw-governance`, proposal and vote
//! queries, voting with wallet or tracked voting keys, proposal creation
//! and submission, and the `Governance` event. Today this module holds the
//! failure type of the contract; the calls return `NotImplemented` from
//! the FFI until R2 lands them.

use crate::EngineError;

/// The proposal field a validation failure concerns (Create Proposal
/// wizard, QT-132).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalField {
    /// 1–40 characters of `[-_a-z0-9]`.
    Name,
    /// No spaces, at least 4 characters.
    Url,
    /// P2PKH or P2SH of this network.
    PaymentAddress,
    /// Above zero.
    PaymentAmount,
    /// 1–12.
    PaymentCount,
    /// Not one of the next 12 superblocks.
    FirstPayment,
    /// The serialized proposal exceeds 512 bytes.
    Payload,
}

/// Why a governance call failed (`governance.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GovernanceFailure {
    /// Governance sync is turned off (Governance tab and clock hidden).
    SyncDisabled,
    /// The call needs governance data that has not finished syncing.
    NotSynced,
    /// No governance object with this hash is known.
    ProposalNotFound(String),
    InvalidProposal(ProposalField),
    /// None of the chosen masternodes has its voting key in a wallet or
    /// attached to a tracked masternode.
    NoVotingKeys,
    /// The masternode changed its vote on this proposal less than an hour
    /// ago (Core `GOVERNANCE_UPDATE_MIN`).
    VoteTooOften {
        retry_after_secs: u64,
    },
    /// The wallet cannot pay the 1 DASH collateral plus fee.
    InsufficientFunds {
        needed: u64,
        available: u64,
    },
    /// Submitting needs at least one collateral confirmation.
    CollateralUnconfirmed {
        confirmations: u32,
    },
    /// The pending proposal's payment period has passed.
    ProposalExpired,
    WatchOnly,
    VaultLocked,
    GrantInvalid,
    NoPeers,
    /// A peer rejected the object, vote or collateral; Core's reason.
    BroadcastRejected(String),
}

impl std::fmt::Display for GovernanceFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SyncDisabled => f.write_str("governance sync is disabled"),
            Self::NotSynced => f.write_str("governance data is not synced"),
            Self::ProposalNotFound(h) => write!(f, "proposal {h} not found"),
            Self::InvalidProposal(field) => write!(f, "invalid proposal field {field:?}"),
            Self::NoVotingKeys => f.write_str("no voting keys"),
            Self::VoteTooOften { retry_after_secs } => {
                write!(f, "voting too often; retry in {retry_after_secs} s")
            }
            Self::InsufficientFunds { needed, available } => {
                write!(f, "needs {needed} duffs, {available} available")
            }
            Self::CollateralUnconfirmed { confirmations } => {
                write!(f, "collateral has {confirmations} confirmations")
            }
            Self::ProposalExpired => f.write_str("proposal expired"),
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::VaultLocked => f.write_str("vault locked"),
            Self::GrantInvalid => f.write_str("grant invalid"),
            Self::NoPeers => f.write_str("no connected peers"),
            Self::BroadcastRejected(r) => write!(f, "broadcast rejected: {r}"),
        }
    }
}

impl From<GovernanceFailure> for EngineError {
    fn from(f: GovernanceFailure) -> Self {
        EngineError::Governance(f)
    }
}
