//! The governance facade's records (`docs/contracts/mg-engine.md` §3), as
//! the renderer sees them (BL-232). Hashes are 64-character lower-case hex
//! in Core's display order; outpoints are `"<txid>-<n>"`; amounts are duffs;
//! times are Unix seconds. The epochs, `createdAt` and the vote times are
//! i64s a peer chose, unbounded below or above, so they may pass 2^53 and
//! reach JS rounded: MG-09 range-checks them (MG-02 D97(b)). The engine
//! records carry no binding derives
//! (mg-engine §1), so each has its twin here, converted field by field.

use dw_engine::governance as engine;
use dw_engine::governance::ProposalScope;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::mirror_enum;

/// Where governance sync is (mg-engine §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum GovernanceSyncPhaseName {
    Disabled,
    Waiting,
    SyncingObjects,
    SyncingVotes,
    Synced,
    Failed,
}

mirror_enum!(engine::GovernanceSyncPhase => GovernanceSyncPhaseName {
    Disabled, Waiting, SyncingObjects, SyncingVotes, Synced, Failed,
});

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhaseName,
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

impl From<engine::GovernanceSyncState> for GovernanceSyncState {
    fn from(s: engine::GovernanceSyncState) -> Self {
        let engine::GovernanceSyncState {
            phase,
            objects,
            unconfirmed_objects,
            votes,
            orphan_votes,
            peers,
            bytes_received,
            last_synced_at,
            last_error,
            next_retry_at,
            objects_secs,
            votes_secs,
        } = s;
        GovernanceSyncState {
            phase: phase.into(),
            objects,
            unconfirmed_objects,
            votes,
            orphan_votes,
            peers,
            bytes_received,
            last_synced_at,
            last_error,
            next_retry_at,
            objects_secs,
            votes_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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

impl From<engine::GovernanceClock> for GovernanceClock {
    fn from(c: engine::GovernanceClock) -> Self {
        let engine::GovernanceClock {
            tip_height,
            cycle_progress,
            last_superblock,
            next_superblock,
            blocks_to_superblock,
            superblock_eta,
            voting_cutoff_estimate,
            voting_cutoff_height,
            voting_open,
        } = c;
        GovernanceClock {
            tip_height,
            cycle_progress,
            last_superblock,
            next_superblock,
            blocks_to_superblock,
            superblock_eta,
            voting_cutoff_estimate,
            voting_cutoff_height,
            voting_open,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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
    /// `null` until a run reached `synced` (MG-00 D21).
    pub proposals: Option<ProposalCounts>,
    pub allocated_estimate: Option<u64>,
    pub unallocated_estimate: Option<u64>,
}

impl From<engine::GovernanceInfo> for GovernanceInfo {
    fn from(i: engine::GovernanceInfo) -> Self {
        let engine::GovernanceInfo {
            clock,
            superblock_cycle,
            maturity_window,
            target_spacing,
            proposal_fee,
            budget,
            passing_threshold,
            valid_masternodes,
            valid_evonodes,
            valid_weight,
            proposals,
            allocated_estimate,
            unallocated_estimate,
        } = i;
        GovernanceInfo {
            clock: clock.into(),
            superblock_cycle,
            maturity_window,
            target_spacing,
            proposal_fee,
            budget,
            passing_threshold,
            valid_masternodes,
            valid_evonodes,
            valid_weight,
            proposals: proposals.map(Into::into),
            allocated_estimate,
            unallocated_estimate,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProposalCounts {
    pub passing: u32,
    pub failing: u32,
    pub unfunded: u32,
    pub total: u32,
    /// Proposals whose status the weight ranges could flip (MG-00 D45).
    pub close_call: u32,
}

impl From<engine::ProposalCounts> for ProposalCounts {
    fn from(c: engine::ProposalCounts) -> Self {
        let engine::ProposalCounts {
            passing,
            failing,
            unfunded,
            total,
            close_call,
        } = c;
        ProposalCounts {
            passing,
            failing,
            unfunded,
            total,
            close_call,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProposalScopeName {
    /// End epoch not past.
    Current,
    All,
}

mirror_enum!(ProposalScopeName => ProposalScope { Current, All });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProposalQuery {
    pub scope: ProposalScopeName,
    /// A case-folded substring of the name or URL.
    pub text: Option<String>,
}

impl From<ProposalQuery> for engine::ProposalQuery {
    fn from(q: ProposalQuery) -> Self {
        let ProposalQuery { scope, text } = q;
        engine::ProposalQuery {
            scope: scope.into(),
            text,
        }
    }
}

/// A weight sum the list cannot pin down (MG-02 D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WeightRange {
    pub min: u32,
    pub max: u32,
    pub estimate: u32,
}

impl From<engine::WeightRange> for WeightRange {
    fn from(r: engine::WeightRange) -> Self {
        let engine::WeightRange { min, max, estimate } = r;
        WeightRange { min, max, estimate }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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

impl From<engine::VoteTally> for VoteTally {
    fn from(t: engine::VoteTally) -> Self {
        let engine::VoteTally {
            yes,
            no,
            abstain,
            net_yes_estimate,
            voters,
            ranged_votes,
        } = t;
        VoteTally {
            yes: yes.into(),
            no: no.into(),
            abstain: abstain.into(),
            net_yes_estimate,
            voters,
            ranged_votes,
        }
    }
}

/// The tallies of the signals besides funding (MG-02 D82), for Core's
/// sentinel flags (D83).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignalTallies {
    pub valid: VoteTally,
    pub delete: VoteTally,
    pub endorsed: VoteTally,
}

impl From<engine::SignalTallies> for SignalTallies {
    fn from(s: engine::SignalTallies) -> Self {
        let engine::SignalTallies {
            valid,
            delete,
            endorsed,
        } = s;
        SignalTallies {
            valid: valid.into(),
            delete: delete.into(),
            endorsed: endorsed.into(),
        }
    }
}

/// dash-qt's proposal status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProposalStatusName {
    Funded,
    Lapsed,
    Confirming,
    Pending,
    Passing,
    Failing,
    Voting,
    Unfunded,
}

mirror_enum!(engine::ProposalStatus => ProposalStatusName {
    Funded, Lapsed, Confirming, Pending, Passing, Failing, Voting, Unfunded,
});

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProposalRow {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub payments_requested: u32,
    pub status: ProposalStatusName,
    /// The ranges could change the status (MG-00 D45 "Too close to call").
    pub close_call: bool,
    pub tally: VoteTally,
    pub funded_height: Option<u32>,
}

impl From<engine::ProposalRow> for ProposalRow {
    fn from(r: engine::ProposalRow) -> Self {
        let engine::ProposalRow {
            hash,
            name,
            url,
            payment_address,
            payment_amount,
            start_epoch,
            end_epoch,
            payments_requested,
            status,
            close_call,
            tally,
            funded_height,
        } = r;
        ProposalRow {
            hash,
            name,
            url,
            payment_address,
            payment_amount,
            start_epoch,
            end_epoch,
            payments_requested,
            status: status.into(),
            close_call,
            tally: tally.into(),
            funded_height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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

impl From<engine::ProposalDetail> for ProposalDetail {
    fn from(d: engine::ProposalDetail) -> Self {
        let engine::ProposalDetail {
            row,
            collateral_txid,
            created_at,
            data_json,
            signals,
            deletion_marked_at,
        } = d;
        ProposalDetail {
            row: row.into(),
            collateral_txid,
            created_at,
            data_json,
            signals: signals.into(),
            deletion_marked_at,
        }
    }
}

/// A vote's signal (Core's `vote_signal_enum_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VoteSignalName {
    Funding,
    Valid,
    Delete,
    Endorsed,
}

mirror_enum!(engine::VoteSignal => VoteSignalName { Funding, Valid, Delete, Endorsed });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum VoteOutcomeName {
    Yes,
    No,
    Abstain,
}

mirror_enum!(engine::VoteOutcome => VoteOutcomeName { Yes, No, Abstain });

impl From<VoteOutcomeName> for engine::VoteOutcome {
    fn from(o: VoteOutcomeName) -> Self {
        match o {
            VoteOutcomeName::Yes => Self::Yes,
            VoteOutcomeName::No => Self::No,
            VoteOutcomeName::Abstain => Self::Abstain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CurrentVoteRow {
    pub outpoint: String,
    pub signal: VoteSignalName,
    pub time: i64,
    pub outcome: VoteOutcomeName,
    pub weight: WeightRange,
    /// The outpoint was resolved to its masternode.
    pub exact: bool,
}

impl From<engine::CurrentVoteRow> for CurrentVoteRow {
    fn from(v: engine::CurrentVoteRow) -> Self {
        let engine::CurrentVoteRow {
            outpoint,
            signal,
            time,
            outcome,
            weight,
            exact,
        } = v;
        CurrentVoteRow {
            outpoint,
            signal: signal.into(),
            time,
            outcome: outcome.into(),
            weight: weight.into(),
            exact,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MasternodeTypeName {
    Regular,
    Evonode,
}

mirror_enum!(engine::MasternodeType => MasternodeTypeName { Regular, Evonode });

/// A masternode of the tip list whose voting key the wallet derives. Show a
/// shared key's vote neutrally ("Last vote seen with this shared key"),
/// never as each masternode's own (mg-engine §2.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VotingMasternode {
    pub pro_tx_hash: String,
    pub kind: MasternodeTypeName,
    pub weight: u32,
    /// PoSe-banned masternodes still vote (MG-02 D16).
    pub valid: bool,
    pub key_index: u32,
    /// This masternode's own current vote on the proposal (MG-02 D49).
    pub current_vote: Option<VoteOutcomeName>,
    pub vote_time: Option<i64>,
    /// Masternodes of the tip list holding the voting key, this one included.
    pub key_holders: u32,
    /// A shared key's latest vote seen on the proposal, by whichever of its
    /// masternodes; `null` for a key one masternode holds.
    pub key_last_vote: Option<VoteOutcomeName>,
    pub key_last_vote_time: Option<i64>,
}

impl From<engine::VotingMasternode> for VotingMasternode {
    fn from(m: engine::VotingMasternode) -> Self {
        let engine::VotingMasternode {
            pro_tx_hash,
            kind,
            weight,
            valid,
            key_index,
            current_vote,
            vote_time,
            key_holders,
            key_last_vote,
            key_last_vote_time,
        } = m;
        VotingMasternode {
            pro_tx_hash,
            kind: kind.into(),
            weight,
            valid,
            key_index,
            current_vote: current_vote.map(Into::into),
            vote_time,
            key_holders,
            key_last_vote: key_last_vote.map(Into::into),
            key_last_vote_time,
        }
    }
}

/// One masternode's result of `governance_vote` (MG-05 §5): `hash` and
/// `peers` when its vote was taken (0 peers: kept and announced on the next
/// connection), else the `rejected` reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct VoteCast {
    pub pro_tx_hash: String,
    pub hash: Option<String>,
    pub peers: u32,
    pub rejected: Option<String>,
}

impl From<engine::VoteCast> for VoteCast {
    fn from(c: engine::VoteCast) -> Self {
        let engine::VoteCast {
            pro_tx_hash,
            hash,
            peers,
            rejected,
        } = c;
        VoteCast {
            pro_tx_hash,
            hash,
            peers,
            rejected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
    pub budget: u64,
    /// The height votes count to for this superblock (MG-06 D13).
    pub voting_cutoff_height: u32,
    pub voting_cutoff_estimate: u64,
    /// The estimated cutoff is at most two days away: the proposal wizard
    /// offers the next superblock first (UX-SPEC §4.25.3).
    pub cutoff_soon: bool,
}

impl From<engine::SuperblockDate> for SuperblockDate {
    fn from(d: engine::SuperblockDate) -> Self {
        let engine::SuperblockDate {
            height,
            estimated_time,
            budget,
            voting_cutoff_height,
            voting_cutoff_estimate,
            cutoff_soon,
        } = d;
        SuperblockDate {
            height,
            estimated_time,
            budget,
            voting_cutoff_height,
            voting_cutoff_estimate,
            cutoff_soon,
        }
    }
}

/// The Create Proposal wizard's input (mg-engine §2.5, MG-06 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProposalDraft {
    pub name: String,
    pub url: String,
    /// P2PKH or P2SH of the session's network (MG-06 D12).
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    pub payment_count: u32,
    /// One of the next 12 superblocks (`governance_upcoming_superblocks`).
    pub first_superblock_height: u32,
    /// Coin control for the fee, `"<txid>-<n>"`: exactly these coins; empty
    /// for automatic selection.
    pub coins: Vec<String>,
}

impl From<ProposalDraft> for engine::ProposalDraft {
    fn from(d: ProposalDraft) -> Self {
        let ProposalDraft {
            name,
            url,
            payment_address,
            payment_amount,
            payment_count,
            first_superblock_height,
            coins,
        } = d;
        engine::ProposalDraft {
            name,
            url,
            payment_address,
            payment_amount,
            payment_count,
            first_superblock_height,
            coins,
        }
    }
}

/// A draft field that failed validation (`governance.invalid_proposal`),
/// named as the contract names them (snake_case, mg-engine §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProposalFieldName {
    Name,
    Url,
    PaymentAddress,
    PaymentAmount,
    PaymentCount,
    FirstPayment,
    /// The JSON exceeds Core's 512 bytes, or Core's checks refuse the
    /// built object.
    Payload,
}

mirror_enum!(engine::ProposalField => ProposalFieldName {
    Name, Url, PaymentAddress, PaymentAmount, PaymentCount, FirstPayment, Payload,
});

/// What the review step shows before the fee is paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProposalPreview {
    /// The object hash, as it would be if created now.
    pub hash: String,
    pub data_json: String,
    pub data_hex: String,
    pub start_epoch: i64,
    pub end_epoch: i64,
    /// The proposal fee, duffs.
    pub fee: u64,
    /// The fee transaction's own network fee, estimated.
    pub network_fee: u64,
}

impl From<engine::ProposalPreview> for ProposalPreview {
    fn from(p: engine::ProposalPreview) -> Self {
        let engine::ProposalPreview {
            hash,
            data_json,
            data_hex,
            start_epoch,
            end_epoch,
            fee,
            network_fee,
        } = p;
        ProposalPreview {
            hash,
            data_json,
            data_hex,
            start_epoch,
            end_epoch,
            fee,
            network_fee,
        }
    }
}

/// Why a proposal whose fee confirmed is not submitted (yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SubmitWaitName {
    /// Announced to a synced run's peers, and none fetched it in 60 s.
    NoPeerAccepted,
    /// Governance sync is off (MG-06 D9).
    SyncDisabled,
    /// No run is synced, so nothing is announced yet (MG-02 D24).
    NotSynced,
}

mirror_enum!(engine::SubmitWait => SubmitWaitName { NoPeerAccepted, SyncDisabled, NotSynced });

/// Where one of the wallet's proposals is (MG-06 §2.3, MG-00 D16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum MyProposalState {
    /// Saved, and the wallet has not seen its fee on the network.
    FeeUnsent,
    /// The fee is out; not submitted yet.
    FeePaid { confirmations: u32 },
    /// The fee has a confirmation, and no peer took the proposal yet.
    ReadyToSubmit { reason: SubmitWaitName },
    /// A peer took it; public once the fee has 6 confirmations.
    Confirming { confirmations: u32 },
    /// Peers announce it (MG-02 D3). `status` once governance is synced.
    Public { status: Option<ProposalStatusName> },
    /// Its fee's coins were spent by another confirmed transaction.
    FeeFailed,
    /// Its end epoch passed before it went public.
    Expired,
}

impl From<engine::MyProposalState> for MyProposalState {
    fn from(s: engine::MyProposalState) -> Self {
        use engine::MyProposalState as E;
        match s {
            E::FeeUnsent => Self::FeeUnsent,
            E::FeePaid { confirmations } => Self::FeePaid { confirmations },
            E::ReadyToSubmit { reason } => Self::ReadyToSubmit {
                reason: reason.into(),
            },
            E::Confirming { confirmations } => Self::Confirming { confirmations },
            E::Public { status } => Self::Public {
                status: status.map(Into::into),
            },
            E::FeeFailed => Self::FeeFailed,
            E::Expired => Self::Expired,
        }
    }
}

/// One of the wallet's own proposals (mg-engine §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MyProposal {
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    pub payment_amount: u64,
    pub payment_count: u32,
    pub start_epoch: i64,
    pub end_epoch: i64,
    pub fee_txid: String,
    pub created_at: u64,
    /// When a peer first fetched it (MG-06 D6).
    pub submitted_at: Option<u64>,
    pub state: MyProposalState,
    /// The last hand-off of its fee's failure, if it failed: why a fee
    /// reads unsent (MG-06 D10).
    pub last_error: Option<String>,
}

impl From<engine::MyProposal> for MyProposal {
    fn from(p: engine::MyProposal) -> Self {
        let engine::MyProposal {
            hash,
            name,
            url,
            payment_address,
            payment_amount,
            payment_count,
            start_epoch,
            end_epoch,
            fee_txid,
            created_at,
            submitted_at,
            state,
            last_error,
        } = p;
        MyProposal {
            hash,
            name,
            url,
            payment_address,
            payment_amount,
            payment_count,
            start_epoch,
            end_epoch,
            fee_txid,
            created_at,
            submitted_at,
            state: state.into(),
            last_error,
        }
    }
}
