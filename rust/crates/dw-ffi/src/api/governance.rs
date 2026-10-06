//! M3 governance: govsync state, the proposal list and details, voting with
//! masternode voting keys, Create / Resume Proposal, the info panel and the
//! governance clock (QT-026, QT-128…134). Owner: R2 (`dw-governance`,
//! `dw-engine/src/governance.rs`). Contract: docs/contracts/m3-engine.md
//! §2.2.
//!
//! Data comes from full-node peers over P2P `govsync` (DESIGN-opus §1.14);
//! nothing is shown as known before it synced: counts and tallies of a
//! running sync come with its `GovernanceSyncState`.

use crate::api::common::{parse_txid, parse_wallet_id};
use crate::{DashNetwork, NetworkSession};
use dw_governance::params;

/// Governance constants of a network (QT-132, QT-134).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct GovernanceParams {
    pub superblock_start_height: u32,
    pub superblock_cycle: u32,
    /// Voting closes this many blocks before a superblock.
    pub maturity_window: u32,
    pub min_quorum: u32,
    /// 1 DASH, duffs.
    pub proposal_fee: u64,
    pub fee_confirmations: u32,
    pub max_name_len: u32,
    pub max_payload_bytes: u32,
    pub max_payments: u32,
    pub evonode_vote_weight: u32,
    pub vote_update_min_secs: u64,
    pub target_spacing_secs: u32,
}

/// The governance constants of `network`. **Works** (constants of
/// `dw-governance`; regtest values are Core's defaults, which
/// `-budgetparams` can change on a node).
#[uniffi::export]
pub fn governance_params(network: DashNetwork) -> GovernanceParams {
    let core = dw_engine::DashNetwork::from(network).core_network();
    let p = params::governance_params(core);
    GovernanceParams {
        superblock_start_height: p.superblock_start_height,
        superblock_cycle: p.superblock_cycle,
        maturity_window: p.maturity_window,
        min_quorum: p.min_quorum,
        proposal_fee: p.proposal_fee,
        fee_confirmations: params::PROPOSAL_FEE_CONFIRMATIONS,
        max_name_len: params::PROPOSAL_NAME_MAX_LEN as u32,
        max_payload_bytes: params::PROPOSAL_MAX_PAYLOAD_BYTES as u32,
        max_payments: params::PROPOSAL_MAX_PAYMENTS,
        evonode_vote_weight: params::EVONODE_VOTE_WEIGHT,
        vote_update_min_secs: params::VOTE_UPDATE_MIN_SECS,
        target_spacing_secs: p.target_spacing_secs,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum GovernanceSyncPhase {
    /// Sync is off (Governance tab and clock hidden).
    Disabled,
    /// Waiting for the chain and masternode list to sync, or for peers.
    Waiting,
    SyncingObjects,
    SyncingVotes,
    /// Caught up; new objects and votes keep arriving.
    Synced,
    /// The last attempt failed; it is retried.
    Failed,
}

/// Governance sync state ("waiting for sync…", progress, G7 counters).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GovernanceSyncState {
    pub phase: GovernanceSyncPhase,
    pub objects: u32,
    pub votes: u64,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
}

/// dash-qt `ProposalStatus`, evaluated in dash-qt's order (research 02
/// §11.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum VoteOutcome {
    Yes,
    No,
    Abstain,
}

/// Which proposals the list shows (the Source combo).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ProposalSource {
    /// "Active Proposals": every valid proposal not yet past its end.
    Active,
    /// "My Proposals": the wallet's proposals, broadcast or pending.
    Mine { wallet_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProposalQuery {
    pub source: ProposalSource,
    /// Case-insensitive substring of the title only (dash-qt).
    pub title_filter: Option<String>,
}

/// "My Votes": weighted votes of the masternodes whose voting keys the
/// wallets hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MyVotes {
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    pub unvoted: u32,
}

/// One row of the proposal list (QT-129).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProposalRow {
    /// Object hash, display order.
    pub hash: String,
    pub name: String,
    pub url: String,
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    pub start_epoch: u64,
    pub end_epoch: u64,
    pub status: ProposalStatus,
    /// Collateral confirmations while `Confirming`/`Pending`.
    pub collateral_confirmations: Option<u32>,
    /// Weighted funding votes.
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    /// `(yes − no) − threshold`: the "(%4%5)" margin of the Votes column.
    pub margin: i64,
    /// `None` = "No voting keys".
    pub my_votes: Option<MyVotes>,
}

/// The details view (double-click) and "Copy Raw JSON" (QT-130).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProposalDetail {
    pub row: ProposalRow,
    pub parent_hash: String,
    pub collateral_txid: String,
    pub created_at: u64,
    /// Payments requested: `ceil((end − start) / cycle time)`.
    pub payments: u32,
    /// The object's data JSON, exactly as signed.
    pub raw_json: String,
}

/// A masternode a wallet can vote with (QT-131 table).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VotingMasternode {
    pub pro_tx_hash: String,
    /// `txid-n`.
    pub collateral: String,
    pub voting_address: String,
    /// 1, or 4 for an EvoNode.
    pub weight: u32,
    pub current_vote: Option<VoteOutcome>,
    pub vote_time: Option<u64>,
    /// Earliest time a changed vote is accepted (1 h rule).
    pub next_vote_at: Option<u64>,
    /// Tracked masternode label, when it is one.
    pub label: Option<String>,
}

/// The result of one masternode's vote ("Voted successfully %n time(s)").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VoteResult {
    pub pro_tx_hash: String,
    /// `None` = relayed; otherwise a `governance.*` code.
    pub error_code: Option<String>,
    /// Diagnostic text for logs (Core's reject reason).
    pub detail: Option<String>,
}

/// A superblock the first payment can be in (Create Proposal "Payment
/// date").
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SuperblockDate {
    pub height: u32,
    pub estimated_time: u64,
}

/// The Create Proposal wizard's input (QT-132).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProposalDraft {
    pub name: String,
    pub url: String,
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    /// 1–12.
    pub payment_count: u32,
    /// One of `superblock_dates()`. dash-qt ignores the chosen date (a bug,
    /// research 02 §11.3); the engine honours it: `start_epoch` is half a
    /// cycle before that superblock and `end_epoch` half a cycle after the
    /// last payment.
    pub first_superblock_height: u32,
}

/// The proposal field a validation issue concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ProposalField {
    Name,
    Url,
    PaymentAddress,
    PaymentAmount,
    PaymentCount,
    FirstPayment,
    Payload,
}

/// Collateral state of a pending proposal (Resume Proposals).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CollateralStatus {
    /// The collateral transaction is not known to the wallet's view.
    Unknown,
    /// Not yet one confirmation.
    Pending,
    /// ≥ 1 confirmation: "Broadcast" is enabled.
    Ready,
}

/// A created, not yet submitted proposal (QT-133).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
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

/// Governance info panel (QT-134).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GovernanceInfo {
    pub sync: GovernanceSyncState,
    pub superblock_cycle: u32,
    pub last_superblock: Option<u32>,
    pub next_superblock: Option<u32>,
    pub next_superblock_eta: Option<u64>,
    /// `next_superblock − maturity_window`.
    pub voting_cutoff: Option<u32>,
    /// Masternodes that voted on any active proposal / eligible.
    pub masternodes_voting: Option<u32>,
    pub masternodes_eligible: Option<u32>,
    pub evonodes_voting: Option<u32>,
    pub evonodes_eligible: Option<u32>,
    /// `max(min_quorum, weighted_valid / 10)`.
    pub passing_threshold: Option<u32>,
    /// Masternodes whose voting keys the wallets hold, and their weight.
    pub masternodes_controlled: u32,
    pub votes_controlled: u32,
    pub proposal_count: Option<u32>,
    pub passing: Option<u32>,
    pub failing: Option<u32>,
    pub unfunded: Option<u32>,
    /// How much more budget the unfunded ones need ("(short X)").
    pub unfunded_short: Option<u64>,
    /// Superblock budget available, from the height (no node needed).
    pub budget_available: Option<u64>,
    /// Requested by passing proposals that fit.
    pub budget_allocated: Option<u64>,
}

/// The status-bar governance clock (QT-026).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct GovernanceClock {
    /// Share of the cycle elapsed, 0–1 (the moon phase).
    pub cycle_progress: f64,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    pub voting_cutoff: u32,
    pub voting_open: bool,
    /// `budget_allocated / budget_available`, 0–1; `None` until synced.
    pub budget_committed: Option<f64>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum GovernanceError {
    /// Code `governance.sync_disabled`.
    #[error("governance sync is disabled")]
    SyncDisabled,
    /// Code `governance.not_synced`.
    #[error("governance data is not synced")]
    NotSynced,
    /// Code `governance.proposal_not_found`.
    #[error("proposal {hash} not found")]
    ProposalNotFound { hash: String },
    /// Code `governance.invalid_proposal`.
    #[error("invalid proposal field {field:?}")]
    InvalidProposal { field: ProposalField },
    /// Code `governance.no_voting_keys`.
    #[error("no voting keys")]
    NoVotingKeys,
    /// Code `governance.vote_too_often`.
    #[error("voting too often; retry in {retry_after_secs} s")]
    VoteTooOften { retry_after_secs: u64 },
    /// Code `governance.insufficient_funds`.
    #[error("needs {needed} duffs, {available} available")]
    InsufficientFunds { needed: u64, available: u64 },
    /// Code `governance.collateral_unconfirmed`.
    #[error("collateral has {confirmations} confirmations")]
    CollateralUnconfirmed { confirmations: u32 },
    /// Code `governance.proposal_expired`.
    #[error("proposal expired")]
    ProposalExpired,
    /// Code `governance.watch_only`.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `governance.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `governance.grant_invalid`.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `governance.no_peers`.
    #[error("no connected peers")]
    NoPeers,
    /// Code `governance.broadcast_rejected`.
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
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

crate::api::common::domain_error_common!(@not_implemented GovernanceError);
crate::api::common::export_error_code!(GovernanceError);

impl GovernanceError {
    /// Stable code (docs/contracts/m3-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::SyncDisabled => "governance.sync_disabled",
            Self::NotSynced => "governance.not_synced",
            Self::ProposalNotFound { .. } => "governance.proposal_not_found",
            Self::InvalidProposal { .. } => "governance.invalid_proposal",
            Self::NoVotingKeys => "governance.no_voting_keys",
            Self::VoteTooOften { .. } => "governance.vote_too_often",
            Self::InsufficientFunds { .. } => "governance.insufficient_funds",
            Self::CollateralUnconfirmed { .. } => "governance.collateral_unconfirmed",
            Self::ProposalExpired => "governance.proposal_expired",
            Self::WatchOnly => "governance.watch_only",
            Self::VaultLocked => "governance.vault_locked",
            Self::GrantInvalid => "governance.grant_invalid",
            Self::NoPeers => "governance.no_peers",
            Self::BroadcastRejected { .. } => "governance.broadcast_rejected",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<dw_engine::ProposalField> for ProposalField {
    fn from(f: dw_engine::ProposalField) -> Self {
        use dw_engine::ProposalField as F;
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

impl From<dw_engine::EngineError> for GovernanceError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        use dw_engine::GovernanceFailure as F;
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::Governance(f) => match f {
                F::SyncDisabled => Self::SyncDisabled,
                F::NotSynced => Self::NotSynced,
                F::ProposalNotFound(hash) => Self::ProposalNotFound { hash },
                F::InvalidProposal(field) => Self::InvalidProposal {
                    field: field.into(),
                },
                F::NoVotingKeys => Self::NoVotingKeys,
                F::VoteTooOften { retry_after_secs } => Self::VoteTooOften { retry_after_secs },
                F::InsufficientFunds { needed, available } => {
                    Self::InsufficientFunds { needed, available }
                }
                F::CollateralUnconfirmed { confirmations } => {
                    Self::CollateralUnconfirmed { confirmations }
                }
                F::ProposalExpired => Self::ProposalExpired,
                F::WatchOnly => Self::WatchOnly,
                F::VaultLocked => Self::VaultLocked,
                F::GrantInvalid => Self::GrantInvalid,
                F::NoPeers => Self::NoPeers,
                F::BroadcastRejected(reason) => Self::BroadcastRejected { reason },
            },
            E::Vault(V::NoVault | V::Locked | V::MixingOnly) => Self::VaultLocked,
            E::Vault(V::GrantInvalid | V::GrantPurposeMismatch) => Self::GrantInvalid,
            E::NoPeers => Self::NoPeers,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

/// Parses a governance object hash (64 lowercase hex, display order).
fn parse_hash(hash: &str) -> Result<(), GovernanceError> {
    parse_txid(hash).map(|_| ()).map_err(Into::into)
}

use dw_engine::governance as g;

impl From<g::GovernanceSyncPhase> for GovernanceSyncPhase {
    fn from(p: g::GovernanceSyncPhase) -> Self {
        match p {
            g::GovernanceSyncPhase::Disabled => Self::Disabled,
            g::GovernanceSyncPhase::Waiting => Self::Waiting,
            g::GovernanceSyncPhase::SyncingObjects => Self::SyncingObjects,
            g::GovernanceSyncPhase::SyncingVotes => Self::SyncingVotes,
            g::GovernanceSyncPhase::Synced => Self::Synced,
            g::GovernanceSyncPhase::Failed => Self::Failed,
        }
    }
}

impl From<g::GovernanceSyncState> for GovernanceSyncState {
    fn from(s: g::GovernanceSyncState) -> Self {
        Self {
            phase: s.phase.into(),
            objects: s.objects,
            votes: s.votes,
            peers: s.peers,
            bytes_received: s.bytes_received,
            last_synced_at: s.last_synced_at,
        }
    }
}

impl From<g::ProposalStatus> for ProposalStatus {
    fn from(s: g::ProposalStatus) -> Self {
        match s {
            g::ProposalStatus::Funded => Self::Funded,
            g::ProposalStatus::Lapsed => Self::Lapsed,
            g::ProposalStatus::Confirming => Self::Confirming,
            g::ProposalStatus::Pending => Self::Pending,
            g::ProposalStatus::Passing => Self::Passing,
            g::ProposalStatus::Failing => Self::Failing,
            g::ProposalStatus::Voting => Self::Voting,
            g::ProposalStatus::Unfunded => Self::Unfunded,
        }
    }
}

impl From<g::VoteOutcome> for VoteOutcome {
    fn from(o: g::VoteOutcome) -> Self {
        match o {
            g::VoteOutcome::Yes => Self::Yes,
            g::VoteOutcome::No => Self::No,
            g::VoteOutcome::Abstain => Self::Abstain,
        }
    }
}

impl From<VoteOutcome> for g::VoteOutcome {
    fn from(o: VoteOutcome) -> Self {
        match o {
            VoteOutcome::Yes => Self::Yes,
            VoteOutcome::No => Self::No,
            VoteOutcome::Abstain => Self::Abstain,
        }
    }
}

impl From<g::ProposalRow> for ProposalRow {
    fn from(r: g::ProposalRow) -> Self {
        Self {
            hash: r.hash,
            name: r.name,
            url: r.url,
            payment_address: r.payment_address,
            payment_amount: r.payment_amount,
            start_epoch: r.start_epoch,
            end_epoch: r.end_epoch,
            status: r.status.into(),
            collateral_confirmations: r.collateral_confirmations,
            yes: r.yes,
            no: r.no,
            abstain: r.abstain,
            margin: r.margin,
            my_votes: r.my_votes.map(|m| MyVotes {
                yes: m.yes,
                no: m.no,
                abstain: m.abstain,
                unvoted: m.unvoted,
            }),
        }
    }
}

impl From<ProposalDraft> for g::ProposalDraft {
    fn from(d: ProposalDraft) -> Self {
        Self {
            name: d.name,
            url: d.url,
            payment_address: d.payment_address,
            payment_amount: d.payment_amount,
            payment_count: d.payment_count,
            first_superblock_height: d.first_superblock_height,
        }
    }
}

impl From<g::CollateralStatus> for CollateralStatus {
    fn from(s: g::CollateralStatus) -> Self {
        match s {
            g::CollateralStatus::Unknown => Self::Unknown,
            g::CollateralStatus::Pending => Self::Pending,
            g::CollateralStatus::Ready => Self::Ready,
        }
    }
}

impl From<g::PendingProposal> for PendingProposal {
    fn from(p: g::PendingProposal) -> Self {
        Self {
            hash: p.hash,
            name: p.name,
            url: p.url,
            payment_amount: p.payment_amount,
            payment_count: p.payment_count,
            collateral_txid: p.collateral_txid,
            collateral_status: p.collateral_status.into(),
            confirmations: p.confirmations,
            created_at: p.created_at,
            end_epoch: p.end_epoch,
        }
    }
}

/// The stable code of a per-masternode vote failure (§4).
fn vote_failure_code(f: &g::GovernanceFailure) -> String {
    GovernanceError::from(dw_engine::EngineError::Governance(f.clone()))
        .code_str()
        .to_string()
}

#[uniffi::export]
impl NetworkSession {
    /// In-memory read; re-query on `Governance` events.
    pub fn governance_sync_state(&self) -> Result<GovernanceSyncState, GovernanceError> {
        Ok(self.inner.governance_sync_state()?.into())
    }

    /// Turns govsync on or off. The host turns it on while the Governance
    /// tab or the governance clock is shown (both opt-in in dash-qt), so a
    /// wallet that never shows governance never downloads it. Persisted.
    pub async fn set_governance_sync_enabled(&self, enabled: bool) -> Result<(), GovernanceError> {
        Ok(self.inner.set_governance_sync_enabled(enabled).await?)
    }

    /// The proposal list (QT-128/129), sorted by dash-qt's status order.
    /// `Active` needs sync enabled (`governance.sync_disabled`); `Mine`
    /// works without it and shows pending proposals.
    pub async fn proposals(
        &self,
        query: ProposalQuery,
    ) -> Result<Vec<ProposalRow>, GovernanceError> {
        let source = match &query.source {
            ProposalSource::Mine { wallet_id } => {
                g::ProposalSource::Mine(parse_wallet_id(wallet_id)?)
            }
            ProposalSource::Active => g::ProposalSource::Active,
        };
        let rows = self
            .inner
            .proposals(g::ProposalQuery {
                source,
                title_filter: query.title_filter,
            })
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn proposal_detail(&self, hash: String) -> Result<ProposalDetail, GovernanceError> {
        parse_hash(&hash)?;
        let d = self.inner.proposal_detail(hash).await?;
        Ok(ProposalDetail {
            row: d.row.into(),
            parent_hash: d.parent_hash,
            collateral_txid: d.collateral_txid,
            created_at: d.created_at,
            payments: d.payments,
            raw_json: d.raw_json,
        })
    }

    /// Masternodes that can vote on `hash`: those whose voting key a
    /// wallet holds (all wallets, or `wallet_id`'s). Owner keys are never
    /// used (dash-qt §11.2). Tracked masternodes with an attached voting key
    /// are not offered: attached keys are R3's (`attach_masternode_key`).
    pub async fn voting_masternodes(
        &self,
        hash: String,
        wallet_id: Option<String>,
    ) -> Result<Vec<VotingMasternode>, GovernanceError> {
        parse_hash(&hash)?;
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let mns = self.inner.voting_masternodes(hash, wallet).await?;
        Ok(mns
            .into_iter()
            .map(|m| VotingMasternode {
                pro_tx_hash: m.pro_tx_hash,
                collateral: m.collateral,
                voting_address: m.voting_address,
                weight: m.weight,
                current_vote: m.current_vote.map(Into::into),
                vote_time: m.vote_time,
                next_vote_at: m.next_vote_at,
                label: m.label,
            })
            .collect())
    }

    /// Signs a funding vote for each masternode and relays it (QT-131).
    /// `grant_id`: a `Governance` grant. One result per masternode; a
    /// masternode that voted on this proposal less than an hour ago gets
    /// `governance.vote_too_often` without being sent.
    pub async fn cast_votes(
        &self,
        hash: String,
        outcome: VoteOutcome,
        pro_tx_hashes: Vec<String>,
        grant_id: String,
    ) -> Result<Vec<VoteResult>, GovernanceError> {
        parse_hash(&hash)?;
        for h in &pro_tx_hashes {
            parse_hash(h)?;
        }
        let results = self
            .inner
            .cast_votes(hash, outcome.into(), pro_tx_hashes, grant_id)
            .await?;
        Ok(results
            .into_iter()
            .map(|r| VoteResult {
                pro_tx_hash: r.pro_tx_hash,
                error_code: r.failure.as_ref().map(vote_failure_code),
                detail: r.detail.or_else(|| r.failure.map(|f| f.to_string())),
            })
            .collect())
    }

    /// The next superblocks with estimated times (Create Proposal
    /// "Payment date"), `count` ≤ 12. Needs the chain tip
    /// (`governance.not_synced` before headers synced).
    pub fn superblock_dates(&self, count: u32) -> Result<Vec<SuperblockDate>, GovernanceError> {
        Ok(self
            .inner
            .superblock_dates(count)?
            .into_iter()
            .map(|d| SuperblockDate {
                height: d.height,
                estimated_time: d.estimated_time,
            })
            .collect())
    }

    /// Every rule of QT-132 that fails, in field order; empty = valid.
    pub fn validate_proposal(
        &self,
        draft: ProposalDraft,
    ) -> Result<Vec<ProposalField>, GovernanceError> {
        Ok(self
            .inner
            .validate_proposal(&draft.into())?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// "View JSON": the data JSON with dash-qt's key order
    /// `name, payment_address, payment_amount, url, start_epoch, end_epoch,
    /// type`.
    pub fn proposal_json(&self, draft: ProposalDraft) -> Result<String, GovernanceError> {
        Ok(self.inner.proposal_json(&draft.into())?)
    }

    /// "View Payload": the hex-encoded object data (≤ 512 bytes).
    pub fn proposal_payload_hex(&self, draft: ProposalDraft) -> Result<String, GovernanceError> {
        Ok(self.inner.proposal_payload_hex(&draft.into())?)
    }

    /// Creates the proposal (`gobject prepare`): validates, builds and
    /// broadcasts the 1 DASH `OP_RETURN <hash>` collateral transaction from
    /// the wallet and stores the proposal as pending (app.sqlite).
    /// `grant_id`: a `Spend` grant of at least 1 DASH + fee.
    pub async fn create_proposal(
        &self,
        wallet_id: String,
        draft: ProposalDraft,
        grant_id: String,
    ) -> Result<PendingProposal, GovernanceError> {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .create_proposal(wallet, draft.into(), grant_id)
            .await?
            .into())
    }

    /// Created, not yet submitted, unexpired proposals (Resume Proposals).
    pub async fn pending_proposals(
        &self,
        wallet_id: String,
    ) -> Result<Vec<PendingProposal>, GovernanceError> {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .pending_proposals(wallet)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// "Broadcast" (`gobject submit`): relays the object once its collateral
    /// has ≥ 1 confirmation. Returns the object hash.
    pub async fn submit_proposal(
        &self,
        wallet_id: String,
        hash: String,
    ) -> Result<String, GovernanceError> {
        let wallet = parse_wallet_id(&wallet_id)?;
        parse_hash(&hash)?;
        Ok(self.inner.submit_proposal(wallet, hash).await?)
    }

    /// The info panel (QT-134); `None` fields are not synced yet.
    pub async fn governance_info(&self) -> Result<GovernanceInfo, GovernanceError> {
        let i = self.inner.governance_info().await?;
        Ok(GovernanceInfo {
            sync: i.sync.into(),
            superblock_cycle: i.superblock_cycle,
            last_superblock: i.last_superblock,
            next_superblock: i.next_superblock,
            next_superblock_eta: i.next_superblock_eta,
            voting_cutoff: i.voting_cutoff,
            masternodes_voting: i.masternodes_voting,
            masternodes_eligible: i.masternodes_eligible,
            evonodes_voting: i.evonodes_voting,
            evonodes_eligible: i.evonodes_eligible,
            passing_threshold: i.passing_threshold,
            masternodes_controlled: i.masternodes_controlled,
            votes_controlled: i.votes_controlled,
            proposal_count: i.proposal_count,
            passing: i.passing,
            failing: i.failing,
            unfunded: i.unfunded,
            unfunded_short: i.unfunded_short,
            budget_available: i.budget_available,
            budget_allocated: i.budget_allocated,
        })
    }

    /// The clock (QT-026). Needs the chain tip only; `budget_committed`
    /// needs governance sync. In-memory read.
    pub fn governance_clock(&self) -> Result<GovernanceClock, GovernanceError> {
        let c = self.inner.governance_clock()?;
        Ok(GovernanceClock {
            cycle_progress: c.cycle_progress,
            next_superblock: c.next_superblock,
            blocks_to_superblock: c.blocks_to_superblock,
            superblock_eta: c.superblock_eta,
            voting_cutoff: c.voting_cutoff,
            voting_open: c.voting_open,
            budget_committed: c.budget_committed,
        })
    }
}
