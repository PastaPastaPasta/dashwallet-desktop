//! DashPay identities, registration, names, credits, invitations and leases.
//!
//! dw-ffi does not bind DashPay, so these records are designed here from
//! the engine's (`dw-engine/src/platform`): camelCase mirrors with
//! exhaustive `From`s, credits as decimal strings, duffs as numbers. A
//! `grant` argument is a vault grant id or a lease id (`beginFlow`'s result);
//! neither is a secret. Invitation links are bearer secrets: they are wrapped
//! the moment they arrive and never logged.

mod credits;
mod flows;
mod invitations;
mod keys;
mod names;
mod registration;
mod startup;

use dw_engine::platform::{BearerSecret, FlowKind};

use self::credits::{
    CostTable, PendingTopUp, StuckTopUpFunding, TopUpOutcome, TopUpQuote, WithdrawAmount,
    WithdrawOutcome, WithdrawQuote,
};
use self::flows::{GrantAction, GrantRequest};
use self::invitations::InvitationStatus;
use self::keys::{IdentityDetail, IdentitySummary};
use self::names::{ContestStatus, NameAvailability, NameOutcome, UserHit, UsernameCheck};
use self::registration::{
    FaucetLockKey, FinishReport, RegistrationQuote, RegistrationRequest, RegistrationStatus,
};
use self::startup::{DashPayStatus, DashPaySyncStatus, QuorumSource, SyncPassReport};
use crate::api::send::BroadcastOutcome;
use crate::dto::platform::{DispatchStateName, FlowKindName, Lease};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "NetworkSession::dashpay",
        "internal: every DashPay command obtains it via state.dashpay",
    ),
    (
        "DashPay::wallet_id",
        "internal: the renderer passes the wallet id it already holds",
    ),
    (
        "NetworkSession::lease_table",
        "internal: the engine's lease table; the renderer reads `leases` and its views",
    ),
    (
        "NetworkSession::coin_fence",
        "internal: the wallet's coin fence, which the engine's own flows take and pass to `funding_call_permit`; a host never holds it",
    ),
    (
        "NetworkSession::funding_call_permit",
        "internal: Mode B funding calls take their permit inside the engine (E0-04 P2b); it takes the engine's owning `Lease` and a `CoinFence`, which a host never holds",
    ),
    (
        "NetworkSession::rebind_lease",
        "internal: takes the engine's owning `Lease`, which a host never holds (`beginFlow` returns only its id); a host recovers a `needsGrant` lease with `endFlow` and `beginFlow` (m4 §1)",
    ),
];

commands! {
    // Identities.

    /// Same-seed discovery past the highest identity index on file; the
    /// number of identities found. `grant` is an identity-scan grant id (a
    /// lease id is `platform.grant_invalid`).
    #[facade("DashPay::discover_identities")]
    fn discover_identities(state, wallet_id: String, grant: String) -> u32 {
        Ok(state.dashpay(&wallet_id).await?.discover_identities(grant).await?)
    }

    /// The wallet's identities by index, from memory.
    #[facade("DashPay::identities")]
    fn identities(state, wallet_id: String) -> Vec<IdentitySummary> {
        let list = state.dashpay(&wallet_id).await?.identities()?;
        Ok(list.into_iter().map(Into::into).collect())
    }

    /// One identity with its public keys.
    #[facade("DashPay::identity_detail")]
    fn identity_detail(state, wallet_id: String, identity: String) -> IdentityDetail {
        Ok(state.dashpay(&wallet_id).await?.identity_detail(identity).await?.into())
    }

    /// Re-reads the identity's credit balance from Platform, as a decimal
    /// string; `null` when unknown.
    #[facade("DashPay::refresh_balance")]
    fn identity_refresh_balance(
        state,
        wallet_id: String,
        identity: String,
    ) -> Option<String> {
        let balance = state.dashpay(&wallet_id).await?.refresh_balance(identity).await?;
        Ok(balance.map(|credits| credits.to_string()))
    }

    /// Chooses the main identity: one of the wallet's, else `identity.not_found`.
    #[facade("DashPay::set_main_identity")]
    fn set_main_identity(state, wallet_id: String, identity: String) -> () {
        Ok(state.dashpay(&wallet_id).await?.set_main_identity(identity).await?)
    }

    // Registration.

    /// What a registration costs and the grant it needs.
    #[facade("DashPay::registration_quote")]
    fn registration_quote(
        state,
        wallet_id: String,
        request: RegistrationRequest,
    ) -> RegistrationQuote {
        let quote = state
            .dashpay(&wallet_id)
            .await?
            .registration_quote(request.into())
            .await?;
        Ok(quote.into())
    }

    /// Starts a registration under `grant` (a grant id or a lease id); the
    /// draft id. Progress is `registrations`.
    #[facade("DashPay::start_registration")]
    fn start_registration(
        state,
        wallet_id: String,
        request: RegistrationRequest,
        grant: String,
    ) -> String {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.start_registration(request.into(), grant).await?)
    }

    /// `startRegistration` past the earlier registrations `past` (as
    /// `registration.in_progress` names them) whose funding is unknown.
    /// Call it only after the user confirmed that each one's payment may
    /// still confirm and then shows up as a lock to recover later.
    #[facade("DashPay::start_registration_past_unknown_funding")]
    fn start_registration_past_unknown_funding(
        state,
        wallet_id: String,
        request: RegistrationRequest,
        grant: String,
        past: Vec<String>,
    ) -> String {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay
            .start_registration_past_unknown_funding(request.into(), grant, past)
            .await?)
    }

    /// Every registration on file, with what it waits for.
    #[facade("DashPay::registrations")]
    fn registrations(state, wallet_id: String) -> Vec<RegistrationStatus> {
        let list = state.dashpay(&wallet_id).await?.registrations().await?;
        Ok(list.into_iter().map(Into::into).collect())
    }

    /// Resumes a parked registration. `grant` is needed exactly when its
    /// `waiting` is `unlock` or `authorize`.
    #[facade("DashPay::resume_registration")]
    fn resume_registration(
        state,
        wallet_id: String,
        draft: String,
        grant: Option<String>,
    ) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.resume_registration(draft, grant).await?)
    }

    /// Discards a registration that has not committed funds.
    #[facade("DashPay::discard_registration")]
    fn discard_registration(state, wallet_id: String, draft: String) -> () {
        Ok(state.dashpay(&wallet_id).await?.discard_registration(draft).await?)
    }

    /// Tools ▸ Repair "Finish transfers": completes stranded asset locks.
    #[facade("DashPay::finish_asset_locks")]
    fn finish_asset_locks(state, wallet_id: String, grant: String) -> FinishReport {
        Ok(state.dashpay(&wallet_id).await?.finish_asset_locks(grant).await?.into())
    }

    /// Developer builds: derives the asset-lock key for the faucet's
    /// `POST /api/asset-lock-proof`.
    #[facade("DashPay::prepare_faucet_lock")]
    fn prepare_faucet_lock(state, wallet_id: String, grant: String) -> FaucetLockKey {
        Ok(state.dashpay(&wallet_id).await?.prepare_faucet_lock(grant).await?.into())
    }

    // Names.

    /// Validates a username label offline; needs no session.
    #[facade("dw_engine::check_username")]
    fn check_username(state, label: String) -> UsernameCheck {
        Ok(dw_engine::platform::check_username(&label)?.into())
    }

    /// The label's checklist verdict, then the network's.
    #[facade("DashPay::name_availability")]
    fn name_availability(state, wallet_id: String, label: String) -> NameAvailability {
        Ok(state.dashpay(&wallet_id).await?.name_availability(label).await?.into())
    }

    /// Registers `label` for `identity`, paid from its credits under `grant`.
    #[facade("DashPay::register_name")]
    fn register_name(
        state,
        wallet_id: String,
        identity: String,
        label: String,
        grant: String,
    ) -> NameOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.register_name(identity, label, grant).await?.into())
    }

    /// Picks `label` as the identity's main name, or clears the pick with `null`.
    #[facade("DashPay::set_main_name")]
    fn set_main_name(
        state,
        wallet_id: String,
        identity: String,
        label: Option<String>,
    ) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.set_main_name(identity, label).await?)
    }

    /// The identity's main name; `null` while it owns none.
    #[facade("DashPay::main_name")]
    fn main_name(state, wallet_id: String, identity: String) -> Option<String> {
        Ok(state.dashpay(&wallet_id).await?.main_name(identity).await?)
    }

    /// The identity's contest for `label`.
    #[facade("DashPay::contest_status")]
    fn contest_status(
        state,
        wallet_id: String,
        identity: String,
        label: String,
    ) -> ContestStatus {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.contest_status(identity, label).await?.into())
    }

    /// Users whose username starts with `prefix`.
    #[facade("DashPay::search_users")]
    fn search_users(
        state,
        wallet_id: String,
        prefix: String,
        limit: u32,
    ) -> Vec<UserHit> {
        let hits = state.dashpay(&wallet_id).await?.search_users(prefix, limit).await?;
        Ok(hits.into_iter().map(Into::into).collect())
    }

    /// The user with exactly this username, if any.
    #[facade("DashPay::resolve_user")]
    fn resolve_user(state, wallet_id: String, username: String) -> Option<UserHit> {
        let hit = state.dashpay(&wallet_id).await?.resolve_user(username).await?;
        Ok(hit.map(Into::into))
    }

    // Credits.

    /// Credit costs of the DashPay actions, for "about N contact requests"
    /// and the low-credit warnings.
    #[facade("DashPay::cost_table")]
    fn credits_cost_table(state, wallet_id: String) -> CostTable {
        Ok(state.dashpay(&wallet_id).await?.cost_table()?.into())
    }

    /// What a top-up of `duffs` costs and buys.
    #[facade("DashPay::top_up_quote")]
    fn credits_top_up_quote(
        state,
        wallet_id: String,
        identity: String,
        duffs: u64,
    ) -> TopUpQuote {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.top_up_quote(identity, duffs).await?.into())
    }

    /// Tops the identity up with `duffs` from the Core balance under `grant`.
    #[facade("DashPay::top_up")]
    fn credits_top_up(
        state,
        wallet_id: String,
        identity: String,
        duffs: u64,
        grant: String,
    ) -> TopUpOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.top_up(identity, duffs, grant).await?.into())
    }

    /// The wallet's top-up locks Platform has not taken.
    #[facade("DashPay::pending_top_ups")]
    fn credits_pending_top_ups(state, wallet_id: String) -> Vec<PendingTopUp> {
        let pending = state.dashpay(&wallet_id).await?.pending_top_ups().await?;
        Ok(pending.into_iter().map(Into::into).collect())
    }

    /// The wallet's top-up fundings whose outcome is unknown: Tools ▸ Repair
    /// resolves each by its `step`.
    #[facade("DashPay::stuck_top_up_fundings")]
    fn credits_stuck_top_up_fundings(state, wallet_id: String) -> Vec<StuckTopUpFunding> {
        let stuck = state.dashpay(&wallet_id).await?.stuck_top_up_fundings().await?;
        Ok(stuck.into_iter().map(Into::into).collect())
    }

    /// Applies the pending top-up lock `txid` under `grant`.
    #[facade("DashPay::resume_top_up")]
    fn credits_resume_top_up(
        state,
        wallet_id: String,
        txid: String,
        grant: String,
    ) -> TopUpOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.resume_top_up(txid, grant).await?.into())
    }

    /// Resolves the pending top-up `txid` whose consumption is unknown (the
    /// user saw its credits arrive): it no longer blocks the identity's
    /// top-ups.
    #[facade("DashPay::resolve_top_up")]
    fn credits_resolve_top_up(state, wallet_id: String, txid: String) -> () {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.resolve_top_up(txid).await?)
    }

    /// What a withdrawal takes and pays out.
    #[facade("DashPay::withdraw_quote")]
    fn credits_withdraw_quote(
        state,
        wallet_id: String,
        identity: String,
        amount: WithdrawAmount,
    ) -> WithdrawQuote {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.withdraw_quote(identity, amount.into()).await?.into())
    }

    /// Withdraws credits to the Core address `to` under `grant`.
    #[facade("DashPay::withdraw")]
    fn credits_withdraw(
        state,
        wallet_id: String,
        identity: String,
        to: String,
        amount: WithdrawAmount,
        grant: String,
    ) -> WithdrawOutcome {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.withdraw(identity, to, amount.into(), grant).await?.into())
    }

    // Grants, flows and leases.

    /// The caps of the grant `action` needs.
    #[facade("DashPay::grant_request")]
    fn grant_request(
        state,
        wallet_id: String,
        identity: String,
        action: GrantAction,
    ) -> GrantRequest {
        let dashpay = state.dashpay(&wallet_id).await?;
        Ok(dashpay.grant_request(identity, action.into()).await?.into())
    }

    /// What the engine knows about a handed-off artifact (a txid, a
    /// state-transition hash or a funding step id); `null`: unknown, which the
    /// UI reads like `maybeSent`.
    #[facade("DashPay::dispatch_status")]
    fn dispatch_status(
        state,
        wallet_id: String,
        artifact: String,
    ) -> Option<DispatchStateName> {
        let status = state.dashpay(&wallet_id).await?.dispatch_status(artifact).await?;
        Ok(status.map(Into::into))
    }

    /// Repair's "Resolve stuck funding: send all coins to yourself" for the
    /// funding step `step` of the wallet (`registration/<draft>/funding` or
    /// `topup/<id>/funding`, as `dispatchStatus` names it; E0-04 §6.5).
    /// `grant` is a `spend` grant or a lease with a Spend budget, as for
    /// `draftPrepare`: the self-spend's outflow is its fee. Refused
    /// `stuck_funding` with the refusal and whether the same Repair may
    /// succeed later (`retryLater`); on success the step keeps reading
    /// `maybeSent` (or `willBeSent`) until the wallet sees the self-spend
    /// ChainLocked.
    #[facade("NetworkSession::resolve_stuck_funding")]
    fn resolve_stuck_funding(
        state,
        wallet_id: String,
        step: String,
        grant: String,
    ) -> BroadcastOutcome {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session.resolve_stuck_funding(id, step, grant).await?.into())
    }

    /// Redeems `grants` (vault grant ids) into one lease for `flow` and
    /// returns its id, which the wallet's calls accept as their `grant` for
    /// the purposes the lease carries. `endFlow` releases it.
    #[facade("NetworkSession::begin_flow")]
    fn begin_flow(
        state,
        wallet_id: String,
        flow: FlowKindName,
        grants: Vec<String>,
    ) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session.begin_flow(id, FlowKind::from(flow), grants).await?)
    }

    /// Releases a lease. Idempotent.
    #[facade("NetworkSession::end_flow")]
    fn end_flow(state, lease: String) -> () {
        Ok(state.session().await?.end_flow(lease)?)
    }

    /// Every live lease of the session.
    #[facade("NetworkSession::leases")]
    fn leases(state) -> Vec<Lease> {
        let views = state.session().await?.leases()?;
        Ok(views.into_iter().map(Into::into).collect())
    }

    // Invitations.

    /// Stores an invitation link in the vault (or beside it before one
    /// exists); its id.
    #[facade("NetworkSession::stash_invitation")]
    fn stash_invitation(state, link: String) -> String {
        let link = BearerSecret::new(link);
        Ok(state.session().await?.stash_invitation(link).await?)
    }

    /// Whether a stashed link can still be claimed.
    #[facade("NetworkSession::invitation_status")]
    fn invitation_status(state, link_id: String) -> InvitationStatus {
        Ok(state.session().await?.invitation_status(link_id).await?.into())
    }

    /// The ids of the stashed links, oldest first.
    #[facade("NetworkSession::pending_invitations")]
    fn pending_invitations(state) -> Vec<String> {
        Ok(state.session().await?.pending_invitations().await?)
    }

    /// Deletes a stashed link: dismissed, or claimed.
    #[facade("NetworkSession::forget_invitation")]
    fn forget_invitation(state, link_id: String) -> () {
        Ok(state.session().await?.forget_invitation(link_id).await?)
    }

    // DashPay status and sync.

    /// The banner state of the Home card, the identity chip and Contacts.
    #[facade("DashPay::status")]
    fn dashpay_status(state, wallet_id: String) -> DashPayStatus {
        Ok(state.dashpay(&wallet_id).await?.status()?.into())
    }

    /// Runs a DashPay sync pass now.
    #[facade("DashPay::sync_now")]
    fn dashpay_sync_now(state, wallet_id: String) -> SyncPassReport {
        Ok(state.dashpay(&wallet_id).await?.sync_now().await?.into())
    }

    /// Tools ▸ Information's DashPay card.
    #[facade("DashPay::sync_status")]
    fn dashpay_sync_status(state, wallet_id: String) -> DashPaySyncStatus {
        Ok(state.dashpay(&wallet_id).await?.sync_status()?.into())
    }

    /// Where Platform proofs get their quorum keys now (Tools ▸ Information).
    #[facade("NetworkSession::quorum_source")]
    fn quorum_source(state) -> QuorumSource {
        Ok(state.session().await?.quorum_source()?.into())
    }

    /// The developer toggle: quorum keys from the trusted service only.
    #[facade("NetworkSession::trusted_quorums_only")]
    fn trusted_quorums_only(state) -> bool {
        Ok(state.session().await?.trusted_quorums_only()?)
    }

    /// Sets the developer toggle. It is global (every network, kept across
    /// launches) and turns SPV checks of quorum keys off, so only developer
    /// builds take it (m4 contract); a release build refuses with
    /// `app.not_developer_build`.
    #[facade("NetworkSession::set_trusted_quorums_only")]
    fn set_trusted_quorums_only(state, on: bool) -> () {
        state.developer_build()?;
        Ok(state.session().await?.set_trusted_quorums_only(on).await?)
    }
}
