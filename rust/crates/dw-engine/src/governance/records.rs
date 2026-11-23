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

/// One masternode's vote of a `vote` batch (MG-05 §5, D6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteCast {
    /// The masternode, display order, lower case.
    pub pro_tx_hash: String,
    /// The vote's hash (display order) once the store took it.
    pub hash: Option<String>,
    /// The peers it was announced to: 0 with none connected, and the vote
    /// is announced on the next connection (D7).
    pub peers: u32,
    /// Why no vote was cast or taken: `unknown_masternode`,
    /// `collateral_unresolved`, `no_voting_key`, `cancelled` (a lock ended
    /// the batch), `signer_unavailable`, or a store refusal in `relay_vote`'s
    /// vocabulary (`too_often`, `same_timestamp`, …).
    pub rejected: Option<String>,
}

/// A masternode's choice in a contested DPNS name's poll (MG-07 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NameChoice {
    /// For the contender with this identity id (base58).
    TowardsIdentity {
        identity: String,
    },
    Abstain,
    /// Nobody gets the name.
    Lock,
}

/// One contender of a contested name, with its tally.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameContender {
    /// The contender's identity id, base58.
    pub identity: String,
    /// Its weighted votes; `None` when Platform gave no tally.
    pub votes: Option<u32>,
}

/// An open DPNS contest (MG-07 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestedName {
    /// The normalized label (`a-hj-km-np-z01-`).
    pub label: String,
    /// When voting ends (Unix s): the poll is resolved in the first block
    /// after it.
    pub ends_at: u64,
    /// The poll's first page of contenders, in identity order (D9).
    pub contenders: Vec<NameContender>,
    pub lock_votes: Option<u32>,
    pub abstain_votes: Option<u32>,
    /// False for a full page: later contenders may exist.
    pub contenders_complete: bool,
}

/// Which of the wallet's keys a masternode votes with (MG-07 D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum NameVotingKey {
    /// The keychain's voting key at `index` (`m/9'/coin'/3'/1'/index`).
    Keychain { index: u32 },
    /// The masternode's attached `voting` key.
    Attached,
}

/// A voter's standing in one listed contest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoterState {
    pub label: String,
    /// Its present choice, as Platform proved it.
    pub choice: Option<NameChoice>,
    /// The votes this app saw Platform take from it on the label, a lower
    /// bound of Platform's own count (D6).
    pub votes_used: u32,
}

/// One of the wallet's masternodes that can vote on names (MG-07 §1, D3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoter {
    pub pro_tx_hash: String,
    pub kind: MasternodeType,
    /// Platform's strength of its vote: 1, or 4 for an evonode.
    pub weight: u32,
    pub valid: bool,
    pub key: NameVotingKey,
    /// The listed contests it has a choice or a vote in.
    pub current: Vec<NameVoterState>,
}

/// `contested_names`'s answer (MG-07 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContestedNames {
    /// Soonest ending first.
    pub contests: Vec<ContestedName>,
    /// In list order.
    pub voters: Vec<NameVoter>,
    /// The votes a masternode has per poll: one and its changes (§4).
    pub votes_allowed: u32,
}

/// One masternode's vote of a `vote_name` batch (MG-07 §1, §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVoteCast {
    /// The masternode, display order, lower case.
    pub pro_tx_hash: String,
    /// The transition's hash (hex) once it was broadcast.
    pub transition: Option<String>,
    /// Why the vote does not count (mg-engine §4): `unknown_masternode`,
    /// `no_voting_key`, `cancelled`, `signer_unavailable`, `in_flight`,
    /// `vote_limit`, `platform_unavailable`, `unconfirmed`, or Platform's
    /// refusal (`already_voted`, `contest_closed`, …).
    pub rejected: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
    pub budget: u64,
    /// The height votes count to for this superblock: its height minus the
    /// maturity window, the clock's cutoff (MG-06 D13).
    pub voting_cutoff_height: u32,
    pub voting_cutoff_estimate: u64,
    /// The estimated cutoff is at most two days away: the proposal wizard
    /// offers the next superblock first (UX-SPEC §4.25.3).
    pub cutoff_soon: bool,
}

/// The Create Proposal wizard's input (mg-engine §2.5, MG-06 §2.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalDraft {
    pub name: String,
    pub url: String,
    /// P2PKH or P2SH of the session's network (MG-06 D12).
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    pub payment_count: u32,
    /// One of the next 12 superblocks (`upcoming_superblocks(12)`).
    pub first_superblock_height: u32,
    /// Coin control for the fee, `"<txid>-<n>"`: exactly these coins; empty
    /// for automatic selection.
    pub coins: Vec<String>,
}

/// A field of a [`ProposalDraft`] that failed validation, in wizard order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalField {
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

/// What the review step shows before the fee is paid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalPreview {
    /// The object hash, as it would be if created now.
    pub hash: String,
    pub data_json: String,
    pub data_hex: String,
    pub start_epoch: i64,
    pub end_epoch: i64,
    /// The proposal fee (`proposal_fee`).
    pub fee: u64,
    /// The fee transaction's own network fee, estimated.
    pub network_fee: u64,
}

/// One of the wallet's own proposals (mg-engine §2.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The last hand-off of its fee's failure, if it failed, while the
    /// wallet has not seen the fee: why a fee reads unsent (review r1 m-3).
    pub last_error: Option<String>,
}

/// Where one of the wallet's proposals is (MG-06 §2.3, MG-00 D16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MyProposalState {
    /// Saved, and the wallet has not seen its fee on the network.
    FeeUnsent,
    /// The fee is out; not submitted yet.
    FeePaid { confirmations: u32 },
    /// The fee has a confirmation, and no peer took the proposal yet.
    ReadyToSubmit { reason: SubmitWait },
    /// A peer took it; public once the fee has 6 confirmations.
    Confirming { confirmations: u32 },
    /// Peers announce it (MG-02 D3). `status` once governance is synced.
    Public { status: Option<ProposalStatus> },
    /// Its fee's coins were spent by another confirmed transaction.
    FeeFailed,
    /// Its end epoch passed before it went public.
    Expired,
}

/// Why a proposal whose fee confirmed is not submitted (yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitWait {
    /// Announced to a synced run's peers, and none fetched it in 60 s.
    NoPeerAccepted,
    /// Governance sync is off (MG-06 D9).
    SyncDisabled,
    /// No run is synced, so nothing is announced yet (MG-02 D24).
    NotSynced,
}

/// Why `submit_proposal` refused (`governance.submit_refused{reason}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitRefusal {
    Public,
    Expired,
    FeeFailed,
    /// Its fee is being handed to the network (a create's or another
    /// [Submit]'s, MG-06 D10).
    InFlight,
}

impl SubmitRefusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Expired => "expired",
            Self::FeeFailed => "fee_failed",
            Self::InFlight => "in_flight",
        }
    }
}
