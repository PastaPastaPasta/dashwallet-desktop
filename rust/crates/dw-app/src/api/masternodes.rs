//! My masternodes (MG-10, `docs/contracts/mn-engine.md`): the SPV list's
//! state, the wallet's masternodes with their payouts, tracking and attached
//! keys, an evonode's credits, and the masternode keychain (IOS-083). A
//! change sends a `masternodes` event. Failures carry the `masternodes.*`
//! codes; the keychain's the `masternode.*` ones.

use zeroize::Zeroizing;

use dw_engine::DashNetwork;

use crate::dto::governance::MasternodeTypeName;
use crate::dto::masternodes::{
    AttachedKeyRoleName, DissolvePreview, DissolveRequest, EvonodeCredits, JoinSharedRequest,
    KeyChangeRequest, KeyCheckName, ListedMasternode, MaintenanceSent, MasternodeDetail,
    MasternodeKeyInfo, MasternodeKeyRoleName, MasternodeListState, MasternodeOverview,
    MasternodePayout, ProTxOperation, RevealedMasternodeKey, SharedCapability, SharedImport,
    SharedKindName, SharedMessage, SharedOutcome, SharedSession, SharedShares, Standby,
    StandbyVariantName, StartSharedRequest, UnreadableSession, UntrackOutcome,
    UpdateRegistrarPreview, UpdateRegistrarRequest,
};
use crate::dto::protx::{MnRegistrationPreview, MnRegistrationRequest, ProTxKeys};
use crate::error::{AppCode, AppError, ErrorData};
use crate::ipc::commands;
use crate::shell::files::FilePurpose;
use crate::state::{AppState, parse_wallet_id};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "NetworkSession::masternodes",
        "internal: every masternodes command obtains it via state.masternodes",
    ),
    ("Masternodes::mark_withdrawal", "deferred (MG-10 S4)"),
    ("Masternodes::preview_update_share", "deferred (MG-10 S6)"),
    ("Masternodes::recheck_withdrawal", "deferred (MG-10 S4)"),
    (
        "Masternodes::resolve_collateral",
        "not bound: always refuses collateral_lookup (MG-03 D11)",
    ),
    ("Masternodes::retry_withdrawal", "deferred (MG-10 S4)"),
    ("Masternodes::update_share", "deferred (MG-10 S6)"),
    ("Masternodes::withdraw_credits", "deferred (MG-10 S4)"),
    ("Masternodes::withdrawal_quote", "deferred (MG-10 S4)"),
    ("Masternodes::withdrawals", "deferred (MG-10 S4)"),
];

commands! {
    /// Whether the SPV masternode list is synced, and its height.
    #[facade("Masternodes::list_state")]
    fn mn_list_state(state) -> MasternodeListState {
        Ok(state.masternodes().await?.list_state()?.into())
    }

    /// The wallet's own masternodes, then the tracked ones, with the
    /// collateral-shaped coins and the 30-day payouts.
    #[facade("Masternodes::list")]
    fn mn_list(state, wallet_id: String) -> MasternodeOverview {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.list(wallet).await?.into())
    }

    /// One masternode the wallet owns or tracks, or the synced list holds.
    #[facade("Masternodes::masternode")]
    fn mn_masternode(state, wallet_id: String, pro_tx_hash: String) -> MasternodeDetail {
        let wallet = parse_wallet_id(&wallet_id)?;
        let detail = state.masternodes().await?.masternode(wallet, pro_tx_hash).await?;
        Ok(detail.into())
    }

    /// Its payouts to the wallet, newest first; with `since` (Unix
    /// seconds), only those at or after it.
    #[facade("Masternodes::payouts")]
    fn mn_payouts(
        state,
        wallet_id: String,
        pro_tx_hash: String,
        since: Option<u64>,
    ) -> Vec<MasternodePayout> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let payouts = state
            .masternodes()
            .await?
            .payouts(wallet, pro_tx_hash, since)
            .await?;
        Ok(payouts.into_iter().map(Into::into).collect())
    }

    /// The synced list's entries matching a ProTx hash or `ip[:port]`.
    #[facade("Masternodes::find")]
    fn mn_find(state, query: String) -> Vec<ListedMasternode> {
        let found = state.masternodes().await?.find(query).await?;
        Ok(found.into_iter().map(Into::into).collect())
    }

    /// An evonode's claimable credits, read from Platform with a proof.
    #[facade("Masternodes::evonode_credits")]
    fn mn_evonode_credits(state, pro_tx_hash: String) -> EvonodeCredits {
        Ok(state.masternodes().await?.evonode_credits(pro_tx_hash).await?.into())
    }

    /// Tracks a masternode of the synced list, for every wallet.
    #[facade("Masternodes::track")]
    fn mn_track(state, pro_tx_hash: String, label: Option<String>) -> ListedMasternode {
        Ok(state.masternodes().await?.track(pro_tx_hash, label).await?.into())
    }

    /// Stops tracking; its attached keys are deleted first.
    #[facade("Masternodes::untrack")]
    fn mn_untrack(state, pro_tx_hash: String) -> UntrackOutcome {
        Ok(state.masternodes().await?.untrack(pro_tx_hash).await?.into())
    }

    /// Names a masternode; `null` or blank clears the name.
    #[facade("Masternodes::set_label")]
    fn mn_set_label(state, pro_tx_hash: String, label: Option<String>) -> () {
        Ok(state.masternodes().await?.set_label(pro_tx_hash, label).await?)
    }

    /// Attaches a private key (WIF or 64 hex) of `role` to a tracked
    /// masternode, checked as far as the wallet can. Needs an unlocked
    /// vault; no grant. The key is never echoed.
    #[facade("Masternodes::attach_key")]
    fn mn_attach_key(
        state,
        pro_tx_hash: String,
        role: AttachedKeyRoleName,
        key: String,
    ) -> KeyCheckName {
        let key = Zeroizing::new(key);
        let masternodes = state.masternodes().await?;
        Ok(masternodes.attach_key(pro_tx_hash, role.into(), key).await?.into())
    }

    /// Deletes the key attached for `role`; `false` when none was.
    #[facade("Masternodes::detach_key")]
    fn mn_detach_key(state, pro_tx_hash: String, role: AttachedKeyRoleName) -> bool {
        Ok(state.masternodes().await?.detach_key(pro_tx_hash, role.into()).await?)
    }

    /// The wallet's derived provider keys of `role`, `start..start+count`
    /// (count ≤ 100). Public data only.
    #[facade("NetworkSession::masternode_keys")]
    fn mn_keys(
        state,
        wallet_id: String,
        role: MasternodeKeyRoleName,
        start: u32,
        count: u32,
    ) -> Vec<MasternodeKeyInfo> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let keys = state
            .session()
            .await?
            .masternode_keys(wallet, role.into(), start, count)
            .await?;
        Ok(keys.into_iter().map(Into::into).collect())
    }

    /// One provider private key of the wallet. Needs a `revealSecret`
    /// grant for it, which the call consumes. The result is secret.
    #[facade("NetworkSession::reveal_masternode_key")]
    fn mn_reveal_key(
        state,
        wallet_id: String,
        role: MasternodeKeyRoleName,
        index: u32,
        grant: String,
    ) -> RevealedMasternodeKey {
        let wallet = parse_wallet_id(&wallet_id)?;
        let revealed = state
            .session()
            .await?
            .reveal_masternode_key(wallet, role.into(), index, grant)
            .await?;
        RevealedMasternodeKey::try_from(revealed)
    }

    /// Plans a ProUpRegTx (owner-signed: the voting key, the operator's
    /// public key and the payout) against the synced list: its fee, the
    /// owner key that would sign and what changes. Signs and locks nothing.
    #[facade("Masternodes::preview_update_registrar")]
    fn mn_preview_update_registrar(
        state,
        wallet_id: String,
        request: UpdateRegistrarRequest,
    ) -> UpdateRegistrarPreview {
        let wallet = parse_wallet_id(&wallet_id)?;
        let preview = state
            .masternodes()
            .await?
            .preview_update_registrar(wallet, request.into())
            .await?;
        Ok(preview.into())
    }

    /// Signs the ProUpRegTx with the masternode's owner key and sends it,
    /// under a `masternodeOp` grant covering the preview's `totalDebit`
    /// (a `key_change` lease). Never signs with an operator key.
    #[facade("Masternodes::update_registrar")]
    fn mn_update_registrar(
        state,
        wallet_id: String,
        request: UpdateRegistrarRequest,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let operation = state
            .masternodes()
            .await?
            .update_registrar(wallet, request.into(), grant)
            .await?;
        Ok(operation.into())
    }

    /// The wallet's ProTx operations (registrations and registrar
    /// updates), each with its state derived on this read.
    #[facade("Masternodes::operations")]
    fn mn_operations(state, wallet_id: String) -> Vec<ProTxOperation> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let ops = state.masternodes().await?.operations(wallet).await?;
        Ok(ops.into_iter().map(Into::into).collect())
    }

    /// Hands a signed operation that didn't go out to the network again,
    /// under a `masternodeOp` grant covering its `debit`.
    #[facade("Masternodes::resend_operation")]
    fn mn_resend_operation(
        state,
        wallet_id: String,
        operation: String,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let op = state
            .masternodes()
            .await?
            .resend_operation(wallet, operation, grant)
            .await?;
        Ok(op.into())
    }

    /// Deletes an operation that was never handed to the network and
    /// releases its coins.
    #[facade("Masternodes::discard_operation")]
    fn mn_discard_operation(state, wallet_id: String, operation: String) -> () {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state
            .masternodes()
            .await?
            .discard_operation(wallet, operation)
            .await?)
    }

    /// The keychain keys a registration of `kind` would use by default.
    /// Public keys and addresses only; needs the synced list.
    #[facade("Masternodes::registration_keys")]
    fn mn_registration_keys(state, wallet_id: String, kind: MasternodeTypeName) -> ProTxKeys {
        let wallet = parse_wallet_id(&wallet_id)?;
        let keys = state.masternodes().await?.registration_keys(wallet, kind.into()).await?;
        Ok(keys.into())
    }

    /// Checks a registration with Core's rules and plans its transaction.
    /// Locks, reserves and issues nothing.
    #[facade("Masternodes::preview_registration")]
    fn mn_preview_registration(
        state,
        wallet_id: String,
        request: MnRegistrationRequest,
    ) -> MnRegistrationPreview {
        let wallet = parse_wallet_id(&wallet_id)?;
        let masternodes = state.masternodes().await?;
        Ok(masternodes.preview_registration(wallet, request.into()).await?.into())
    }

    /// Registers a masternode with a fund-new or existing-coin collateral
    /// under a `masternodeOp` grant, which the call consumes. The result is
    /// the saved operation, `sent` or `not_sent` with its `lastError`.
    #[facade("Masternodes::register")]
    fn mn_register(
        state,
        wallet_id: String,
        request: MnRegistrationRequest,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let masternodes = state.masternodes().await?;
        Ok(masternodes.register(wallet, request.into(), grant).await?.into())
    }

    /// Prepares an external-collateral registration: locks its fee inputs
    /// and saves it as `needs_signature` with the message the collateral's
    /// key signs. No grant; signs nothing.
    #[facade("Masternodes::prepare_external_registration")]
    fn mn_prepare_external_registration(
        state,
        wallet_id: String,
        request: MnRegistrationRequest,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let masternodes = state.masternodes().await?;
        let op = masternodes.prepare_external_registration(wallet, request.into()).await?;
        Ok(op.into())
    }

    /// Re-plans an external registration whose fee inputs were spent: new
    /// inputs and a new message, the same terms.
    #[facade("Masternodes::refresh_external_registration")]
    fn mn_refresh_external_registration(
        state,
        wallet_id: String,
        operation: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let masternodes = state.masternodes().await?;
        let op = masternodes.refresh_external_registration(wallet, operation).await?;
        Ok(op.into())
    }

    /// Finishes an external registration with the collateral key's
    /// `signmessage` signature (base64), under a `masternodeOp` grant.
    #[facade("Masternodes::submit_external_registration")]
    fn mn_submit_external_registration(
        state,
        wallet_id: String,
        operation: String,
        signature: String,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        let masternodes = state.masternodes().await?;
        let op = masternodes
            .submit_external_registration(wallet, operation, signature, grant)
            .await?;
        Ok(op.into())
    }

    /// Unlocks a masternode's collateral coin and remembers it. Sends nothing.
    #[facade("Masternodes::release_collateral")]
    fn mn_release_collateral(state, wallet_id: String, pro_tx_hash: String) -> () {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.release_collateral(wallet, pro_tx_hash).await?)
    }

    /// Locks a released collateral coin again.
    #[facade("Masternodes::lock_collateral")]
    fn mn_lock_collateral(state, wallet_id: String, pro_tx_hash: String) -> () {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.lock_collateral(wallet, pro_tx_hash).await?)
    }
    /// What the wallet can do with shared masternodes: v24 as far as it can
    /// tell, shared evonodes (always off), and whether sending is on.
    #[facade("Masternodes::shared_capability")]
    fn mn_shared_capability(state, wallet_id: String) -> SharedCapability {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.shared_capability(wallet).await?.into())
    }

    /// Every shared-masternode session of the wallet, newest first.
    #[facade("Masternodes::shared_sessions")]
    fn mn_shared_sessions(state, wallet_id: String) -> Vec<SharedSession> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let sessions = state.masternodes().await?.shared_sessions(wallet).await?;
        Ok(sessions.into_iter().map(Into::into).collect())
    }

    /// Sessions whose stored record no longer reads, newest first: each can
    /// be discarded while nothing of it may be out.
    #[facade("Masternodes::unreadable_shared_sessions")]
    fn mn_shared_unreadable(state, wallet_id: String) -> Vec<UnreadableSession> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let sessions = state.masternodes().await?.unreadable_shared_sessions(wallet).await?;
        Ok(sessions.into_iter().map(Into::into).collect())
    }

    /// One session, its stage derived now.
    #[facade("Masternodes::shared_session")]
    fn mn_shared_session(state, wallet_id: String, session: String) -> SharedSession {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.shared_session(wallet, session).await?.into())
    }

    /// A shared masternode's shares from the wallet's history: amounts,
    /// refund and reward addresses, the early period and whether a share
    /// may dissolve or change keys now. No session is needed.
    #[facade("Masternodes::shared_shares")]
    fn mn_shared_shares(state, wallet_id: String, pro_tx_hash: String) -> SharedShares {
        let wallet = parse_wallet_id(&wallet_id)?;
        let shares = state.masternodes().await?.shared_shares(wallet, pro_tx_hash).await?;
        Ok(shares.into())
    }

    /// The wallet's stored standbys of a shared masternode, by share and
    /// fee; their signed bytes stay in Rust.
    #[facade("Masternodes::standby")]
    fn mn_shared_standby(state, wallet_id: String, pro_tx_hash: String) -> Vec<Standby> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let standbys = state.masternodes().await?.standby(wallet, pro_tx_hash).await?;
        Ok(standbys.into_iter().map(Into::into).collect())
    }

    /// Starts a session as its coordinator: reserves the coordinator's share
    /// and the fee, signs nothing, and returns the Invitation.
    #[facade("Masternodes::start_shared")]
    fn mn_shared_start(state, wallet_id: String, request: StartSharedRequest) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.start_shared(wallet, request.into()).await?.into())
    }

    /// Changes a draft's network fee before anyone replied.
    #[facade("Masternodes::set_shared_fee")]
    fn mn_shared_set_fee(state, wallet_id: String, session: String, fee: u64) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state.masternodes().await?.set_shared_fee(wallet, session, fee).await?;
        Ok(outcome.into())
    }

    /// Joins an imported Invitation: reserves exactly the share, signs
    /// nothing, and returns the reply.
    #[facade("Masternodes::join_shared")]
    fn mn_shared_join(
        state,
        wallet_id: String,
        session: String,
        request: JoinSharedRequest,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .join_shared(wallet, session, request.into())
            .await?;
        Ok(outcome.into())
    }

    /// The message to copy or save now.
    #[facade("Masternodes::shared_message")]
    fn mn_shared_message(state, wallet_id: String, session: String) -> SharedMessage {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.shared_message(wallet, session).await?.into())
    }

    /// Locks the terms at the `revision` the coordinator reviewed and signs
    /// its consent. Takes a `masternodeOp` grant of 0.
    #[facade("Masternodes::lock_shared_terms")]
    fn mn_shared_lock(
        state,
        wallet_id: String,
        session: String,
        revision: u32,
        grant: String,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .lock_shared_terms(wallet, session, revision, grant)
            .await?;
        Ok(outcome.into())
    }

    /// Unlocks the terms; every approval is dropped.
    #[facade("Masternodes::unlock_shared_terms")]
    fn mn_shared_unlock(state, wallet_id: String, session: String) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.unlock_shared_terms(wallet, session).await?.into())
    }

    /// Approves the locked terms of `consent_hash`, the one the participant
    /// reviewed. Takes a `masternodeOp` grant of 0.
    #[facade("Masternodes::approve_shared")]
    fn mn_shared_approve(
        state,
        wallet_id: String,
        session: String,
        consent_hash: String,
        grant: String,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .approve_shared(wallet, session, consent_hash, grant)
            .await?;
        Ok(outcome.into())
    }

    /// Spends every reserved coin of a session that may be out back to the
    /// wallet, so it can't confirm (`SharedSession.abandonCharge`). Takes a
    /// `masternodeOp` grant of exactly that charge. Every network.
    #[facade("Masternodes::abandon_shared_session")]
    fn mn_shared_abandon(state, wallet_id: String, session: String, grant: String) -> SharedSession {
        let wallet = parse_wallet_id(&wallet_id)?;
        let session = state
            .masternodes()
            .await?
            .abandon_shared_session(wallet, session, grant)
            .await?;
        Ok(session.into())
    }

    /// The latest abandon spend's saved bytes handed off again, while the
    /// spend may be out (`SharedSession.resendAbandonCharge`). Takes a
    /// `masternodeOp` grant of exactly that charge. Every network.
    #[facade("Masternodes::resend_shared_abandon")]
    fn mn_shared_resend_abandon(
        state,
        wallet_id: String,
        session: String,
        grant: String,
    ) -> SharedSession {
        let wallet = parse_wallet_id(&wallet_id)?;
        let session = state
            .masternodes()
            .await?
            .resend_shared_abandon(wallet, session, grant)
            .await?;
        Ok(session.into())
    }

    /// The same for a session whose stored record no longer reads
    /// (`UnreadableSession.abandonCharge`).
    #[facade("Masternodes::abandon_unreadable_shared_session")]
    fn mn_shared_abandon_unreadable(
        state,
        wallet_id: String,
        session: String,
        grant: String,
    ) -> UnreadableSession {
        let wallet = parse_wallet_id(&wallet_id)?;
        let session = state
            .masternodes()
            .await?
            .abandon_unreadable_shared_session(wallet, session, grant)
            .await?;
        Ok(session.into())
    }

    /// A participant at `combined` signs its funding inputs into the
    /// registration, charged its `debit`. Takes a `masternodeOp` grant of
    /// exactly that `debit`. Refused on mainnet before the engine is called
    /// (`app.shared_send_off`).
    #[facade("Masternodes::sign_shared_funding")]
    fn mn_shared_sign(state, wallet_id: String, session: String, grant: String) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        require_shared_send(&state).await?;
        let outcome = state
            .masternodes()
            .await?
            .sign_shared_funding(wallet, session, grant)
            .await?;
        Ok(outcome.into())
    }

    /// The coordinator signs its own inputs last and sends the registration
    /// at `combined`, or sends the saved registration again at `sent`,
    /// charged its `debit`. Takes a `masternodeOp` grant of exactly that
    /// `debit`. Refused on mainnet before the engine is called
    /// (`app.shared_send_off`).
    #[facade("Masternodes::send_shared_registration")]
    fn mn_shared_send(state, wallet_id: String, session: String, grant: String) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        require_shared_send(&state).await?;
        let outcome = state
            .masternodes()
            .await?
            .send_shared_registration(wallet, session, grant)
            .await?;
        Ok(outcome.into())
    }

    /// Deletes a session nothing of which may be out and releases its coins
    /// (`SharedSession.discardable`).
    #[facade("Masternodes::discard_shared_session")]
    fn mn_shared_discard(state, wallet_id: String, session: String) -> () {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(state.masternodes().await?.discard_shared_session(wallet, session).await?)
    }

    /// Reads a pasted message, or one standby transaction line. A refusal
    /// names its reason, never the text.
    #[facade("Masternodes::import_shared_message")]
    fn mn_shared_import(
        state,
        wallet_id: String,
        text: String,
        target: Option<String>,
    ) -> SharedImport {
        let wallet = parse_wallet_id(&wallet_id)?;
        let imported = state
            .masternodes()
            .await?
            .import_shared_message(wallet, text, target)
            .await?;
        Ok(imported.into())
    }

    /// Shows the open dialog and imports the chosen file: a message whole,
    /// or each transaction of a standby file once. `null` when the user
    /// cancelled. The file's text never reaches the renderer.
    #[facade(
        "Masternodes::import_shared_message",
        "dw_engine::masternodes::standby_file_transactions"
    )]
    fn mn_shared_import_file(
        state,
        wallet_id: String,
        target: Option<String>,
    ) -> Option<Vec<SharedImport>> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let Some(text) = open_shared_file(&state).await? else {
            return Ok(None);
        };
        let masternodes = state.masternodes().await?;
        let mut out = Vec::new();
        for text in file_messages(text) {
            let imported = masternodes
                .import_shared_message(wallet, text, target.clone())
                .await?;
            out.push(imported.into());
        }
        Ok(Some(out))
    }

    /// Plans a dissolution alone by one of this wallet's shares: what each
    /// share gets back, the penalty, the fee and `totalDebit`. Signs nothing.
    #[facade("Masternodes::preview_dissolve")]
    fn mn_shared_preview_dissolve(
        state,
        wallet_id: String,
        request: DissolveRequest,
    ) -> DissolvePreview {
        let wallet = parse_wallet_id(&wallet_id)?;
        let preview = state
            .masternodes()
            .await?
            .preview_dissolve(wallet, request.into())
            .await?;
        Ok(preview.into())
    }

    /// Signs the preview's dissolution with the share's owner key and sends
    /// it, under a `masternodeOp` grant covering its `totalDebit`. Refused on
    /// mainnet before the engine is called (`app.shared_send_off`).
    #[facade("Masternodes::dissolve")]
    fn mn_shared_dissolve(
        state,
        wallet_id: String,
        request: DissolveRequest,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        require_shared_send(&state).await?;
        let op = state
            .masternodes()
            .await?
            .dissolve(wallet, request.into(), grant)
            .await?;
        Ok(op.into())
    }

    /// Signs and stores the standby dissolutions paying `fee` of each of this
    /// wallet's shares, under a `masternodeOp` grant covering each share's
    /// penalty and fee. Sends nothing.
    #[facade("Masternodes::create_standby")]
    fn mn_shared_create_standby(
        state,
        wallet_id: String,
        pro_tx_hash: String,
        fee: u64,
        grant: String,
    ) -> Vec<Standby> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let standbys = state
            .masternodes()
            .await?
            .create_standby(wallet, pro_tx_hash, fee, grant)
            .await?;
        Ok(standbys.into_iter().map(Into::into).collect())
    }

    /// Shows the save dialog and writes the stored standby of `share` paying
    /// `fee` there as Core's file, readable by the user only, under a
    /// `masternodeOp` grant of 0. Returns the file's name, `null` when the
    /// user cancelled (the grant unused). The file's text never reaches the
    /// renderer.
    #[facade("Masternodes::export_standby")]
    fn mn_shared_export_standby(
        state,
        wallet_id: String,
        pro_tx_hash: String,
        share: u16,
        fee: u64,
        grant: String,
    ) -> Option<String> {
        let wallet = parse_wallet_id(&wallet_id)?;
        let name = standby_file_name(&pro_tx_hash, share);
        let Some(path) =
            crate::api::files::choose_save(&state, FilePurpose::StandbyDissolution, name).await?
        else {
            return Ok(None);
        };
        let file = state
            .masternodes()
            .await?
            .export_standby(wallet, pro_tx_hash, share, fee, grant)
            .await?;
        let text = Zeroizing::new(file.text.into_bytes());
        crate::api::files::write_chosen(path.clone(), text).await?;
        Ok(path.file_name().map(|n| n.to_string_lossy().into_owned()))
    }

    /// Sends a variant of the stored standby of `share` paying `fee`, under a
    /// `masternodeOp` grant covering the fee and, for the immediate variant,
    /// the penalty (0 for another share's). Refused on mainnet before the
    /// engine is called (`app.shared_send_off`).
    #[facade("Masternodes::send_standby")]
    fn mn_shared_send_standby(
        state,
        wallet_id: String,
        pro_tx_hash: String,
        share: u16,
        fee: u64,
        variant: StandbyVariantName,
        grant: String,
    ) -> ProTxOperation {
        let wallet = parse_wallet_id(&wallet_id)?;
        require_shared_send(&state).await?;
        let op = state
            .masternodes()
            .await?
            .send_standby(wallet, pro_tx_hash, share, fee, variant.into(), grant)
            .await?;
        Ok(op.into())
    }

    /// Prepares a dissolution together with this wallet's share the actor
    /// paying `fee`, and signs this wallet's shares under a `masternodeOp`
    /// grant covering the fee. Returns the session and its request.
    #[facade("Masternodes::start_dissolve_together")]
    fn mn_shared_start_dissolve_together(
        state,
        wallet_id: String,
        request: DissolveRequest,
        grant: String,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .start_dissolve_together(wallet, request.into(), grant)
            .await?;
        Ok(outcome.into())
    }

    /// Prepares a change of the registrar keys together, this wallet the
    /// funder: reserves the fee coins and signs this wallet's shares under a
    /// `masternodeOp` grant of 0. Returns the session and its request.
    #[facade("Masternodes::start_key_change")]
    fn mn_shared_start_key_change(
        state,
        wallet_id: String,
        request: KeyChangeRequest,
        grant: String,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .start_key_change(wallet, request.into(), grant)
            .await?;
        Ok(outcome.into())
    }

    /// Signs this wallet's unsigned shares of a dissolution together or a
    /// key change. A dissolution's grant covers `actorFee` when the request
    /// takes the fee from this wallet's share (`accept_actor_fee`), else 0; a
    /// key change's is 0.
    #[facade("Masternodes::approve_maintenance")]
    fn mn_shared_approve_maintenance(
        state,
        wallet_id: String,
        session: String,
        grant: String,
        accept_actor_fee: bool,
    ) -> SharedOutcome {
        let wallet = parse_wallet_id(&wallet_id)?;
        let outcome = state
            .masternodes()
            .await?
            .approve_maintenance(wallet, session, grant, accept_actor_fee)
            .await?;
        Ok(outcome.into())
    }

    /// Sends a complete dissolution together, under a `masternodeOp` grant of
    /// 0. Refused before the engine is called on mainnet
    /// (`app.shared_send_off`) and for any other kind of session
    /// (`app.shared_send_unsupported`): this version sends no key change.
    #[facade("Masternodes::send_maintenance", "Masternodes::shared_session")]
    fn mn_shared_send_maintenance(
        state,
        wallet_id: String,
        session: String,
        grant: String,
    ) -> MaintenanceSent {
        let wallet = parse_wallet_id(&wallet_id)?;
        require_shared_send(&state).await?;
        let masternodes = state.masternodes().await?;
        let kind = masternodes.shared_session(wallet, session.clone()).await?.kind;
        maintenance_send_allowed(kind.into())?;
        Ok(masternodes.send_maintenance(wallet, session, grant).await?.into())
    }
}

/// Whether this version sends shared-masternode transactions on `network`:
/// everywhere but mainnet (MG's ruling for this release). [`SHARED_SEND_GATE`]
/// lists what it gates: `mn_shared_sign` (a participant's funding signature
/// lets the coordinator send), `mn_shared_send`, `mn_shared_dissolve`,
/// `mn_shared_send_standby` and `mn_shared_send_maintenance` refuse before
/// the engine is called, and `update_share` must check it once a command
/// binds it. Reads, standby create and export, approvals, the maintenance
/// starts and the abandon calls (`abandon_shared_session`,
/// `resend_shared_abandon`, the unreadable abandon) send nothing of a share
/// and are never gated.
pub(crate) fn shared_sends_allowed(network: &DashNetwork) -> bool {
    !matches!(network, DashNetwork::Mainnet)
}

/// [`shared_sends_allowed`] for the open session's network, as an error.
async fn require_shared_send(state: &AppState) -> Result<(), AppError> {
    let session = state.session().await?;
    let network = session.network();
    if shared_sends_allowed(network) {
        return Ok(());
    }
    Err(AppError::app(
        AppCode::SharedSendOff,
        "this version sends shared-masternode transactions on test networks only",
    )
    .with(ErrorData::SharedSendNetwork {
        network: network.dir_name(),
    }))
}

/// `mn_shared_send_maintenance` sends a dissolution together only.
fn maintenance_send_allowed(kind: SharedKindName) -> Result<(), AppError> {
    if kind == SharedKindName::DissolveTogether {
        return Ok(());
    }
    Err(AppError::app(
        AppCode::SharedSendUnsupported,
        "this version sends no shared-masternode session of this kind",
    )
    .with(ErrorData::SharedUnsupported { session_kind: kind }))
}

/// How a shared-masternode call stands against [`shared_sends_allowed`].
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendGate {
    /// The command refuses on mainnet before the engine is called.
    Gated,
    /// Not bound yet; its command must check the gate when it is.
    GatedWhenBound,
    /// The command never checks it.
    Never,
}

/// Every shared-masternode command or engine call that signs or reserves,
/// with its gate. `tests::the_send_gate_table_matches_the_source` checks
/// each command's body against it.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const SHARED_SEND_GATE: &[(&str, SendGate)] = &[
    ("Masternodes::dissolve", SendGate::Gated),
    ("Masternodes::send_standby", SendGate::Gated),
    ("Masternodes::send_maintenance", SendGate::Gated),
    ("mn_shared_sign", SendGate::Gated),
    ("mn_shared_send", SendGate::Gated),
    ("mn_shared_dissolve", SendGate::Gated),
    ("mn_shared_send_standby", SendGate::Gated),
    ("mn_shared_send_maintenance", SendGate::Gated),
    ("Masternodes::update_share", SendGate::GatedWhenBound),
    ("Masternodes::abandon_shared_session", SendGate::Never),
    ("Masternodes::resend_shared_abandon", SendGate::Never),
    ("mn_shared_abandon", SendGate::Never),
    ("mn_shared_resend_abandon", SendGate::Never),
    ("mn_shared_abandon_unreadable", SendGate::Never),
    ("mn_shared_lock", SendGate::Never),
    ("mn_shared_approve", SendGate::Never),
    ("mn_shared_preview_dissolve", SendGate::Never),
    ("mn_shared_create_standby", SendGate::Never),
    ("mn_shared_export_standby", SendGate::Never),
    ("mn_shared_start_dissolve_together", SendGate::Never),
    ("mn_shared_start_key_change", SendGate::Never),
    ("mn_shared_approve_maintenance", SendGate::Never),
    ("mn_shared_standby", SendGate::Never),
    ("mn_shared_shares", SendGate::Never),
];
/// Core's standby file name: the ProTx hash's first 8 characters, the share
/// numbered from 1 (the engine's `StandbyFile.file_name`).
fn standby_file_name(pro_tx_hash: &str, share: u16) -> String {
    let short = pro_tx_hash
        .chars()
        .take(8)
        .collect::<String>()
        .to_lowercase();
    format!(
        "standby-dissolution-{short}-share{}.txt",
        u32::from(share) + 1
    )
}

/// The chosen file's text; `null` on cancel. Invalid UTF-8 is refused as a
/// message that isn't a shared-masternode one, without its bytes.
async fn open_shared_file(state: &AppState) -> Result<Option<String>, AppError> {
    let Some(file) = crate::api::files::open(state, FilePurpose::SharedMasternodeMessage).await?
    else {
        return Ok(None);
    };
    String::from_utf8(file.data).map(Some).map_err(|_| {
        dw_engine::masternodes::MasternodesError::SharedMessage {
            reason: "not_shared".into(),
            share: None,
        }
        .into()
    })
}

/// What one file holds to import: each distinct transaction line of a
/// standby file (this wallet's or Dash Core's), else the whole text as one
/// message.
fn file_messages(text: String) -> Vec<String> {
    dw_engine::masternodes::standby_file_transactions(&text).unwrap_or_else(|_| vec![text])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_sends_are_off_on_mainnet_only() {
        assert!(!shared_sends_allowed(&DashNetwork::Mainnet));
        assert!(shared_sends_allowed(&DashNetwork::Testnet));
        assert!(shared_sends_allowed(&DashNetwork::Regtest));
        assert!(shared_sends_allowed(&DashNetwork::Devnet {
            name: "paloma".into()
        }));
    }

    #[test]
    fn maintenance_sends_a_dissolution_together_only() {
        assert!(maintenance_send_allowed(SharedKindName::DissolveTogether).is_ok());
        for kind in [SharedKindName::KeyChange, SharedKindName::Registration] {
            let e = maintenance_send_allowed(kind).unwrap_err();
            assert_eq!(e.code_str(), "app.shared_send_unsupported");
            assert_eq!(
                e.data,
                Some(ErrorData::SharedUnsupported { session_kind: kind })
            );
        }
        let e =
            serde_json::to_value(maintenance_send_allowed(SharedKindName::KeyChange).unwrap_err())
                .unwrap();
        assert_eq!(
            e["data"],
            serde_json::json!({ "kind": "sharedUnsupported", "sessionKind": "key_change" })
        );
    }

    /// A command's body in this file, from its `fn` to the next command's
    /// doc comment.
    fn body<'a>(src: &'a str, name: &str) -> &'a str {
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{name} isn't declared here"));
        let rest = &src[start..];
        let end = rest.find("\n    ///").unwrap_or(rest.len());
        &rest[..end]
    }

    /// The engine calls that hand a shared-masternode transaction to the
    /// network, or sign what lets another wallet do it.
    const SEND_CALLS: &[&str] = &[
        ".dissolve(",
        ".send_standby(",
        ".send_maintenance(",
        ".sign_shared_funding(",
        ".send_shared_registration(",
        ".update_share(",
    ];

    #[test]
    fn the_send_gate_table_matches_the_source() {
        let src = include_str!("masternodes.rs");
        let commands = &src[..src.find("\n/// Whether this version sends").unwrap()];
        for (name, gate) in SHARED_SEND_GATE {
            if let Some(call) = name.strip_prefix("Masternodes::") {
                let callers: Vec<_> = NAMES
                    .iter()
                    .filter(|n| body(commands, n).contains(&format!(".{call}(")))
                    .collect();
                if callers.is_empty() {
                    // Unbound: excluded with a reason.
                    assert!(
                        EXCLUDED.iter().any(|(m, _)| m == name),
                        "{name} is neither bound nor excluded"
                    );
                }
                // Bound: every command calling it has a row of the same gate.
                for caller in callers {
                    assert!(
                        SHARED_SEND_GATE.contains(&(caller, *gate)),
                        "{caller} calls {name}: give it a row of the same gate"
                    );
                }
                continue;
            }
            assert!(NAMES.contains(name), "{name} isn't a command");
            let checks = body(commands, name).contains("require_shared_send(");
            assert_eq!(checks, *gate == SendGate::Gated, "{name}");
        }
        // Every command that makes a send call is in the table, gated.
        for name in NAMES {
            let b = body(commands, name);
            if SEND_CALLS.iter().any(|c| b.contains(c)) {
                assert!(
                    SHARED_SEND_GATE.contains(&(name, SendGate::Gated)),
                    "{name} sends but isn't gated"
                );
            }
        }
        // C1, C2: the funding signature and the registration's send are
        // gated; abandon never.
        assert!(SHARED_SEND_GATE.contains(&("mn_shared_sign", SendGate::Gated)));
        assert!(SHARED_SEND_GATE.contains(&("mn_shared_send", SendGate::Gated)));
        for send in [
            "mn_shared_dissolve",
            "mn_shared_send_standby",
            "mn_shared_send_maintenance",
        ] {
            assert!(
                SHARED_SEND_GATE.contains(&(send, SendGate::Gated)),
                "{send}"
            );
        }
        for abandon in [
            "Masternodes::abandon_shared_session",
            "Masternodes::resend_shared_abandon",
            "mn_shared_abandon",
            "mn_shared_resend_abandon",
            "mn_shared_abandon_unreadable",
        ] {
            assert!(SHARED_SEND_GATE.contains(&(abandon, SendGate::Never)), "{abandon}");
        }
    }

    #[test]
    fn the_standby_file_is_named_as_core_names_it() {
        assert_eq!(
            standby_file_name(&"4D1C3A3B".repeat(8), 1),
            "standby-dissolution-4d1c3a3b-share2.txt"
        );
    }
}
