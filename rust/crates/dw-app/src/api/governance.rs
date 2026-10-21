//! Governance (MG-02, `docs/contracts/mg-engine.md`, BL-232): the sync
//! task, the clock and budget, proposals and their votes, and the
//! masternodes the wallet votes with. Reads answer from memory; a change of
//! what they show sends a `governance` event (≤ 1 Hz). Failures carry the
//! `governance.*` codes.

use crate::dto::governance::{
    CurrentVoteRow, GovernanceClock, GovernanceInfo, GovernanceSyncState, MyProposal,
    ProposalDetail, ProposalDraft, ProposalPreview, ProposalQuery, ProposalRow, SuperblockDate,
    VoteCast, VoteOutcomeName, VotingMasternode,
};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "NetworkSession::governance",
        "internal: every governance command obtains it via state.governance",
    ),
    (
        "Governance::relay_vote",
        "internal: reached through the console's `voteraw` (`console_execute`); the UI votes with `governance_vote` (MG-05)",
    ),
    (
        "Governance::contested_names",
        "deferred (MG-11): the contested-name voting UI binds it (MG-07 D11)",
    ),
    (
        "Governance::vote_name",
        "deferred (MG-11): the contested-name voting UI binds it (MG-07 D11)",
    ),
];

commands! {
    /// Whether governance sync is on (off by default).
    #[facade("Governance::sync_enabled")]
    fn governance_sync_enabled(state) -> bool {
        Ok(state.governance().await?.sync_enabled()?)
    }

    /// Saves the setting, then starts or stops the sync task. Stopping keeps
    /// what was synced. Idempotent.
    #[facade("Governance::set_sync_enabled")]
    fn governance_set_sync_enabled(state, enabled: bool) -> () {
        Ok(state.governance().await?.set_sync_enabled(enabled).await?)
    }

    /// The sync phase and its counters.
    #[facade("Governance::sync_state")]
    fn governance_sync_state(state) -> GovernanceSyncState {
        Ok(state.governance().await?.sync_state()?.into())
    }

    /// Starts a new sync run now; `governance.sync_disabled` while sync is off.
    #[facade("Governance::resync")]
    fn governance_resync(state) -> () {
        Ok(state.governance().await?.resync()?)
    }

    /// The superblock cycle at the tip: progress, the next superblock and its
    /// ETA, and the estimated voting cutoff.
    #[facade("Governance::clock")]
    fn governance_clock(state) -> GovernanceClock {
        Ok(state.governance().await?.clock()?.into())
    }

    /// The clock, the next superblock's budget, Core's `getgovernanceinfo`
    /// numbers, and (once synced) the proposal counts and allocation.
    #[facade("Governance::info")]
    fn governance_info(state) -> GovernanceInfo {
        Ok(state.governance().await?.info()?.into())
    }

    /// The next `count` (at most 24) superblocks with their ETAs and budgets.
    #[facade("Governance::upcoming_superblocks")]
    fn governance_upcoming_superblocks(state, count: u32) -> Vec<SuperblockDate> {
        let dates = state.governance().await?.upcoming_superblocks(count)?;
        Ok(dates.into_iter().map(Into::into).collect())
    }

    /// Confirmed proposals, sorted by status group then net yes;
    /// `governance.not_synced` until a run reached `synced`.
    #[facade("Governance::proposals")]
    fn governance_proposals(state, query: ProposalQuery) -> Vec<ProposalRow> {
        let rows = state.governance().await?.proposals(query.into())?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// One proposal with its object fields and the valid, delete and
    /// endorsed tallies.
    #[facade("Governance::proposal")]
    fn governance_proposal(state, hash: String) -> ProposalDetail {
        Ok(state.governance().await?.proposal(hash)?.into())
    }

    /// Core's `gobject getcurrentvotes`: every current vote on the proposal,
    /// of one collateral outpoint (`"<txid>-<n>"`, parsed as Core does: 64
    /// hex characters in any case, then the index; MG-05 D10) when given.
    #[facade("Governance::current_votes")]
    fn governance_current_votes(
        state,
        hash: String,
        outpoint: Option<String>,
    ) -> Vec<CurrentVoteRow> {
        let votes = state.governance().await?.current_votes(hash, outpoint)?;
        Ok(votes.into_iter().map(Into::into).collect())
    }

    /// The masternodes whose voting key the wallet derives, each with its own
    /// vote on `proposal` as seen; `governance.list_unavailable` while SPV has
    /// no masternode list.
    #[facade("Governance::voting_masternodes")]
    fn governance_voting_masternodes(
        state,
        wallet_id: String,
        proposal: String,
    ) -> Vec<VotingMasternode> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let rows = state
            .governance()
            .await?
            .voting_masternodes(wallet, proposal)
            .await?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// Signs and relays a funding vote on `proposal` with each masternode
    /// (proTxHash) under one `Governance` grant of the wallet: one result per
    /// masternode, in order (MG-05 §5). Argument and sync errors fail the
    /// call before the grant is used; lease errors are `platform.*`.
    #[facade("Governance::vote")]
    fn governance_vote(
        state,
        wallet_id: String,
        proposal: String,
        outcome: VoteOutcomeName,
        masternodes: Vec<String>,
        grant: String,
    ) -> Vec<VoteCast> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let cast = state
            .governance()
            .await?
            .vote(wallet, proposal, outcome.into(), masternodes, grant)
            .await?;
        Ok(cast.into_iter().map(Into::into).collect())
    }

    /// The review step of the Create Proposal wizard: the proposal's hash,
    /// data, epochs and fees as if created now. Reserves, signs and saves
    /// nothing. `governance.invalid_proposal` lists every failing field
    /// (wizard order); `governance.not_synced` before a tip is known.
    #[facade("Governance::preview_proposal")]
    fn governance_preview_proposal(
        state,
        wallet_id: String,
        draft: ProposalDraft,
    ) -> ProposalPreview {
        let wallet = parse_wallet_id(&wallet_id)?;
        let preview = state
            .governance()
            .await?
            .preview_proposal(wallet, draft.into())
            .await?;
        Ok(preview.into())
    }

    /// Creates the proposal under one `Spend` grant of the wallet covering
    /// the proposal fee and its network fee: pays the fee and saves the
    /// proposal, which then waits for its fee to confirm before peers are
    /// offered it (MG-06). Argument errors fail the call before the grant
    /// is used; lease errors are `platform.*`.
    #[facade("Governance::create_proposal")]
    fn governance_create_proposal(
        state,
        wallet_id: String,
        draft: ProposalDraft,
        grant: String,
    ) -> MyProposal {
        let wallet = parse_wallet_id(&wallet_id)?;
        let created = state
            .governance()
            .await?
            .create_proposal(wallet, draft.into(), grant)
            .await?;
        Ok(created.into())
    }

    /// The wallet's own proposals with where each is (mg-engine §2.5).
    #[facade("Governance::my_proposals")]
    fn governance_my_proposals(state, wallet_id: String) -> Vec<MyProposal> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let rows = state.governance().await?.my_proposals(wallet)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    /// The Submit action: resends the saved fee of a `feeUnsent` proposal or
    /// an unmined `feePaid` one, or has a `readyToSubmit` one looked for by
    /// a new sync run at once. Returns the
    /// proposal after; `governance.submit_refused` for a public, expired or
    /// fee-failed one.
    #[facade("Governance::submit_proposal")]
    fn governance_submit_proposal(state, wallet_id: String, hash: String) -> MyProposal {
        let wallet = parse_wallet_id(&wallet_id)?;
        let submitted = state
            .governance()
            .await?
            .submit_proposal(wallet, hash)
            .await?;
        Ok(submitted.into())
    }
}
