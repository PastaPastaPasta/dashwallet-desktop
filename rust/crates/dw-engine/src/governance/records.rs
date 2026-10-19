//! The records the governance facade returns and takes
//! (`docs/contracts/mg-engine.md` §3). Hashes are 64-character lower-case
//! hex in Core's display order; outpoints are `"<txid>-<n>"`; amounts are
//! duffs; times are Unix seconds.

use serde::{Deserialize, Serialize};

/// Where governance sync is (`mg-engine.md` §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GovernanceSyncPhase {
    Disabled,
    Waiting,
    SyncingObjects,
    SyncingVotes,
    Synced,
    Failed,
}

impl From<dw_governance::sync::Phase> for GovernanceSyncPhase {
    fn from(p: dw_governance::sync::Phase) -> Self {
        use dw_governance::sync::Phase as P;
        match p {
            P::Disabled => Self::Disabled,
            P::Waiting => Self::Waiting,
            P::SyncingObjects => Self::SyncingObjects,
            P::SyncingVotes => Self::SyncingVotes,
            P::Synced => Self::Synced,
            P::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhase,
    /// Confirmed objects.
    pub objects: u32,
    /// Objects announced by one peer only (MG-02 D3).
    pub unconfirmed_objects: u32,
    pub votes: u64,
    pub orphan_votes: u32,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
    pub last_error: Option<String>,
    pub next_retry_at: Option<u64>,
    pub objects_secs: Option<f64>,
    pub votes_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceClock {
    pub tip_height: u32,
    pub cycle_progress: f64,
    pub last_superblock: u32,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    /// The estimated time voting closes (MG-00 D19).
    pub voting_cutoff_estimate: u64,
    pub voting_cutoff_height: u32,
    pub voting_open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GovernanceInfo {
    pub clock: GovernanceClock,
    pub superblock_cycle: u32,
    pub maturity_window: u32,
    /// Core's consensus target spacing (150 s), as `getgovernanceinfo`.
    pub target_spacing: u32,
    pub proposal_fee: u64,
    /// The next superblock's budget.
    pub budget: u64,
    pub passing_threshold: u32,
    pub valid_masternodes: u32,
    pub valid_evonodes: u32,
    pub valid_weight: u32,
    /// `None` until a run reached `synced` (MG-00 D21).
    pub proposals: Option<ProposalCounts>,
    pub allocated_estimate: Option<u64>,
    pub unallocated_estimate: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalCounts {
    pub passing: u32,
    pub failing: u32,
    pub unfunded: u32,
    pub total: u32,
    /// Proposals whose status the weight ranges could flip (MG-00 D45).
    pub close_call: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalScope {
    /// End epoch not past.
    Current,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalQuery {
    pub scope: ProposalScope,
    /// A case-folded substring of the name or URL.
    pub text: Option<String>,
}

/// A weight sum the list cannot pin down (MG-02 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WeightRange {
    pub min: u32,
    pub max: u32,
    pub estimate: u32,
}

impl From<dw_governance::tally::WeightRange> for WeightRange {
    fn from(r: dw_governance::tally::WeightRange) -> Self {
        Self {
            min: r.min,
            max: r.max,
            estimate: r.estimate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VoteTally {
    pub yes: WeightRange,
    pub no: WeightRange,
    pub abstain: WeightRange,
    pub net_yes_estimate: i64,
    /// Outpoints seen voting.
    pub voters: u32,
    /// Votes whose weight is not exact.
    pub ranged_votes: u32,
}

impl From<dw_governance::tally::RangedTally> for VoteTally {
    fn from(t: dw_governance::tally::RangedTally) -> Self {
        Self {
            yes: t.yes.into(),
            no: t.no.into(),
            abstain: t.abstain.into(),
            net_yes_estimate: t.estimate.absolute_yes(),
            voters: t.voters,
            ranged_votes: t.ranged_votes,
        }
    }
}

/// The tallies of the signals besides funding (MG-02 D82), for Core's
/// sentinel flags (D83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SignalTallies {
    pub valid: VoteTally,
    pub delete: VoteTally,
    pub endorsed: VoteTally,
}

/// dash-qt's proposal status (`tally::Status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Funded,
    Lapsed,
    Confirming,
    Pending,
    Passing,
    Failing,
    Voting,
    Unfunded,
}

impl From<dw_governance::tally::Status> for ProposalStatus {
    fn from(s: dw_governance::tally::Status) -> Self {
        use dw_governance::tally::Status as S;
        match s {
            S::Funded => Self::Funded,
            S::Lapsed => Self::Lapsed,
            S::Confirming => Self::Confirming,
            S::Pending => Self::Pending,
            S::Passing => Self::Passing,
            S::Failing => Self::Failing,
            S::Voting => Self::Voting,
            S::Unfunded => Self::Unfunded,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalRow {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub payments_requested: u32,
    pub status: ProposalStatus,
    /// The ranges could change the status (MG-00 D45 "Too close to call").
    pub close_call: bool,
    pub tally: VoteTally,
    pub funded_height: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDetail {
    pub row: ProposalRow,
    /// As the first sender gave it (MG-02 D18).
    pub collateral_txid: String,
    pub created_at: i64,
    pub data_json: String,
    pub signals: SignalTallies,
    /// When the wallet marked the proposal deleted by vote; it leaves 10
    /// minutes later (MG-02 D84).
    pub deletion_marked_at: Option<i64>,
}

/// A vote's signal (`vote_signal_enum_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteSignal {
    Funding,
    Valid,
    Delete,
    Endorsed,
}

impl From<dw_governance::vote::SentinelSignal> for VoteSignal {
    fn from(s: dw_governance::vote::SentinelSignal) -> Self {
        use dw_governance::vote::SentinelSignal as S;
        match s {
            S::Valid => Self::Valid,
            S::Delete => Self::Delete,
            S::Endorsed => Self::Endorsed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoteOutcome {
    Yes,
    No,
    Abstain,
}

impl From<dw_governance::vote::VoteOutcome> for VoteOutcome {
    fn from(o: dw_governance::vote::VoteOutcome) -> Self {
        use dw_governance::vote::VoteOutcome as O;
        match o {
            O::Yes => Self::Yes,
            O::No => Self::No,
            O::Abstain => Self::Abstain,
        }
    }
}

impl From<VoteOutcome> for dw_governance::vote::VoteOutcome {
    fn from(o: VoteOutcome) -> Self {
        match o {
            VoteOutcome::Yes => Self::Yes,
            VoteOutcome::No => Self::No,
            VoteOutcome::Abstain => Self::Abstain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurrentVoteRow {
    pub outpoint: String,
    pub signal: VoteSignal,
    pub time: i64,
    pub outcome: VoteOutcome,
    pub weight: WeightRange,
    /// The outpoint was resolved to its masternode.
    pub exact: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MasternodeType {
    Regular,
    Evonode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VotingMasternode {
    pub pro_tx_hash: String,
    pub kind: MasternodeType,
    pub weight: u32,
    /// PoSe-banned masternodes still vote (MG-02 D16).
    pub valid: bool,
    pub key_index: u32,
    /// This masternode's own current vote on the proposal: a vote under its
    /// key whose collateral is resolved to it, or any vote under a key no
    /// other masternode of the list holds (MG-02 D49).
    pub current_vote: Option<VoteOutcome>,
    pub vote_time: Option<i64>,
    /// Masternodes of the tip list holding the voting key, this one
    /// included. Past one, the key's votes are not told apart by
    /// masternode until their collateral is resolved.
    pub key_holders: u32,
    /// A shared key's latest vote seen on the proposal, by whichever of its
    /// masternodes; `None` for a key one masternode holds.
    pub key_last_vote: Option<VoteOutcome>,
    pub key_last_vote_time: Option<i64>,
}

/// Core `voteraw`'s arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawVote {
    /// `"<txid>-<n>"`.
    pub collateral: String,
    pub proposal: String,
    pub outcome: VoteOutcome,
    pub time: i64,
    /// Base64 of the 65-byte compact signature.
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteRelayed {
    pub hash: String,
    pub peers: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
    pub budget: u64,
}
