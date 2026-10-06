//! Governance in the engine (M3, owner R2; docs/contracts/m3-engine.md
//! §2.2): the govsync lifecycle over `dw-governance`, the proposal list,
//! details, info panel and clock, voting with the wallets' DIP-3 voting
//! keys, and creating / submitting proposals (QT-026, QT-128…134).
//!
//! - [`state`]: per-session state, the sync task and the coalesced
//!   `Governance` event.
//! - [`view`]: list rows, details, info panel and clock from the synced
//!   objects, the SPV masternode list and the tip.
//! - [`voting`]: which masternodes a wallet votes for, and casting votes.
//! - [`proposals`]: the 1 DASH collateral transaction, pending proposals in
//!   app.sqlite, submitting.
//!
//! Every value comes from synced objects, the SPV masternode list or the
//! wallets; what is not known is `None` (DESIGN-opus §1.14).

mod proposals;
pub(crate) mod state;
mod view;
mod voting;

use crate::EngineError;

pub use dw_governance::proposal::Draft as ProposalDraft;
pub use dw_governance::sync::Phase as GovernanceSyncPhase;
pub use dw_governance::tally::Status as ProposalStatus;
pub use dw_governance::vote::VoteOutcome;
pub(crate) use state::GovernanceState;

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

impl From<dw_governance::proposal::Field> for ProposalField {
    fn from(f: dw_governance::proposal::Field) -> Self {
        use dw_governance::proposal::Field as F;
        match f {
            F::Name => Self::Name,
            F::Url => Self::Url,
            F::PaymentAddress => Self::PaymentAddress,
            F::PaymentAmount => Self::PaymentAmount,
            F::PaymentCount => Self::PaymentCount,
            F::FirstPayment => Self::FirstPayment,
            F::Payload => Self::Payload,
        }
    }
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

/// Governance sync state ("waiting for sync…", progress, G7 counters).
#[derive(Debug, Clone, PartialEq)]
pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhase,
    pub objects: u32,
    pub votes: u64,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
    /// Why the last attempt failed (logs, `dwcli gov-sync`).
    pub last_error: Option<String>,
    /// Wall time of the last full sync's objects / votes phases (G7).
    pub objects_secs: Option<f64>,
    pub votes_secs: Option<f64>,
}

/// Which proposals the list shows (the Source combo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProposalSource {
    /// "Active Proposals": every synced proposal not lapsed for longer than
    /// one superblock cycle.
    Active,
    /// "My Proposals": the wallet's proposals, broadcast or pending.
    Mine(crate::WalletId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalQuery {
    pub source: ProposalSource,
    /// Case-insensitive substring of the title only (dash-qt).
    pub title_filter: Option<String>,
}

/// "My Votes": weighted votes of the masternodes whose voting keys the
/// wallets hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MyVotes {
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    pub unvoted: u32,
}

/// One row of the proposal list (QT-129).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalRow {
    /// Display order.
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    pub start_epoch: u64,
    pub end_epoch: u64,
    pub status: ProposalStatus,
    /// Collateral confirmations of the wallet's own proposals (`None` for
    /// others: the peers relay objects only after 6).
    pub collateral_confirmations: Option<u32>,
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    /// `(yes − no) − threshold`.
    pub margin: i64,
    /// `None` = "No voting keys".
    pub my_votes: Option<MyVotes>,
    /// Superblock height of the trigger that paid it.
    pub funded_height: Option<u32>,
}

/// The details view and "Copy Raw JSON" (QT-130).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalDetail {
    pub row: ProposalRow,
    pub parent_hash: String,
    pub collateral_txid: String,
    pub created_at: u64,
    pub payments: u32,
    /// The object's data JSON, exactly as signed.
    pub raw_json: String,
}

/// A masternode a wallet can vote with (QT-131 table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VotingMasternode {
    /// Display order.
    pub pro_tx_hash: String,
    /// `txid-n`.
    pub collateral: String,
    pub voting_address: String,
    pub weight: u32,
    pub current_vote: Option<VoteOutcome>,
    pub vote_time: Option<u64>,
    pub next_vote_at: Option<u64>,
    pub label: Option<String>,
}

/// One masternode's vote result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteResult {
    pub pro_tx_hash: String,
    /// `None` = relayed.
    pub failure: Option<GovernanceFailure>,
    pub detail: Option<String>,
}

/// A superblock the first payment can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
}

/// Collateral state of a pending proposal (Resume Proposals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollateralStatus {
    Unknown,
    Pending,
    Ready,
}

/// A created, not yet submitted proposal (QT-133).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProposal {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_amount: u64,
    pub payment_count: u32,
    pub collateral_txid: String,
    pub collateral_status: CollateralStatus,
    pub confirmations: u32,
    pub created_at: u64,
    pub end_epoch: u64,
}

/// Governance info panel (QT-134); `None` fields are not synced yet.
#[derive(Debug, Clone, PartialEq)]
pub struct GovernanceInfo {
    pub sync: GovernanceSyncState,
    pub superblock_cycle: u32,
    pub last_superblock: Option<u32>,
    pub next_superblock: Option<u32>,
    pub next_superblock_eta: Option<u64>,
    pub voting_cutoff: Option<u32>,
    pub masternodes_voting: Option<u32>,
    pub masternodes_eligible: Option<u32>,
    pub evonodes_voting: Option<u32>,
    pub evonodes_eligible: Option<u32>,
    pub passing_threshold: Option<u32>,
    pub masternodes_controlled: u32,
    pub votes_controlled: u32,
    pub proposal_count: Option<u32>,
    pub passing: Option<u32>,
    pub failing: Option<u32>,
    pub unfunded: Option<u32>,
    pub unfunded_short: Option<u64>,
    pub budget_available: Option<u64>,
    pub budget_allocated: Option<u64>,
}

/// The status-bar governance clock (QT-026).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GovernanceClock {
    pub cycle_progress: f64,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    pub voting_cutoff: u32,
    pub voting_open: bool,
    pub budget_committed: Option<f64>,
}
