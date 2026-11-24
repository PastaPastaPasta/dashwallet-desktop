//! My masternodes (MG-10, `docs/contracts/mn-engine.md`): the SPV list's
//! state, the wallet's masternodes with their payouts, tracking and attached
//! keys, an evonode's credits, and the masternode keychain (IOS-083). A
//! change sends a `masternodes` event. Failures carry the `masternodes.*`
//! codes; the keychain's the `masternode.*` ones.

use zeroize::Zeroizing;

use crate::dto::masternodes::{
    AttachedKeyRoleName, EvonodeCredits, JoinSharedRequest, KeyCheckName, ListedMasternode,
    MasternodeDetail, MasternodeKeyInfo, MasternodeKeyRoleName, MasternodeListState,
    MasternodeOverview, MasternodePayout, ProTxOperation, RevealedMasternodeKey,
    SharedCapability, SharedImport, SharedMessage, SharedOutcome, SharedSession, SharedShares,
    Standby, StartSharedRequest, UntrackOutcome, UpdateRegistrarPreview, UpdateRegistrarRequest,
};
use crate::error::AppError;
use crate::ipc::commands;
use crate::shell::files::FilePurpose;
use crate::state::{AppState, parse_wallet_id};

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "NetworkSession::masternodes",
        "internal: every masternodes command obtains it via state.masternodes",
    ),
    (
        "Masternodes::abandon_shared_session",
        "deferred (MG-10 S5b): needs the recovery read (EP 4)",
    ),
    ("Masternodes::approve_maintenance", "deferred (MG-10 S6)"),
    ("Masternodes::create_standby", "deferred (MG-10 S6)"),
    ("Masternodes::dissolve", "deferred (MG-10 S6)"),
    ("Masternodes::export_standby", "deferred (MG-10 S6)"),
    ("Masternodes::lock_collateral", "deferred (MG-10 S2)"),
    ("Masternodes::mark_withdrawal", "deferred (MG-10 S4)"),
    (
        "Masternodes::prepare_external_registration",
        "deferred (MG-10 S2)",
    ),
    ("Masternodes::preview_dissolve", "deferred (MG-10 S6)"),
    ("Masternodes::preview_registration", "deferred (MG-10 S2)"),
    ("Masternodes::preview_update_share", "deferred (MG-10 S6)"),
    ("Masternodes::recheck_withdrawal", "deferred (MG-10 S4)"),
    (
        "Masternodes::refresh_external_registration",
        "deferred (MG-10 S2)",
    ),
    ("Masternodes::register", "deferred (MG-10 S2)"),
    ("Masternodes::registration_keys", "deferred (MG-10 S2)"),
    ("Masternodes::release_collateral", "deferred (MG-10 S2)"),
    (
        "Masternodes::resolve_collateral",
        "not bound: always refuses collateral_lookup (MG-03 D11)",
    ),
    ("Masternodes::retry_withdrawal", "deferred (MG-10 S4)"),
    ("Masternodes::send_maintenance", "deferred (MG-10 S6)"),
    (
        "Masternodes::send_shared_registration",
        "deferred (MG-10 S5b): needs the recovery read (EP 4)",
    ),
    ("Masternodes::send_standby", "deferred (MG-10 S6)"),
    (
        "Masternodes::sign_shared_funding",
        "deferred (MG-10 S5b): needs the recovery read (EP 4)",
    ),
    (
        "Masternodes::start_dissolve_together",
        "deferred (MG-10 S6)",
    ),
    ("Masternodes::start_key_change", "deferred (MG-10 S6)"),
    (
        "Masternodes::submit_external_registration",
        "deferred (MG-10 S2)",
    ),
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
