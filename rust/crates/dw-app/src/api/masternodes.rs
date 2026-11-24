//! My masternodes (MG-10, `docs/contracts/mn-engine.md`): the SPV list's
//! state, the wallet's masternodes with their payouts, tracking and attached
//! keys, an evonode's credits, and the masternode keychain (IOS-083). A
//! change sends a `masternodes` event. Failures carry the `masternodes.*`
//! codes; the keychain's the `masternode.*` ones.

use zeroize::Zeroizing;

use crate::dto::masternodes::{
    AttachedKeyRoleName, EvonodeCredits, KeyCheckName, ListedMasternode, MasternodeDetail,
    MasternodeKeyInfo, MasternodeKeyRoleName, MasternodeListState, MasternodeOverview,
    MasternodePayout, ProTxOperation, RevealedMasternodeKey, UntrackOutcome,
    UpdateRegistrarPreview, UpdateRegistrarRequest,
};
use crate::dto::governance::MasternodeTypeName;
use crate::dto::protx::{MnRegistrationPreview, MnRegistrationRequest, ProTxKeys};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[
    (
        "NetworkSession::masternodes",
        "internal: every masternodes command obtains it via state.masternodes",
    ),
    ("Masternodes::abandon_shared_session", "deferred (MG-10 S5)"),
    ("Masternodes::approve_maintenance", "deferred (MG-10 S6)"),
    ("Masternodes::approve_shared", "deferred (MG-10 S5)"),
    ("Masternodes::create_standby", "deferred (MG-10 S6)"),
    ("Masternodes::discard_shared_session", "deferred (MG-10 S5)"),
    ("Masternodes::dissolve", "deferred (MG-10 S6)"),
    ("Masternodes::export_standby", "deferred (MG-10 S6)"),
    ("Masternodes::import_shared_message", "deferred (MG-10 S5)"),
    ("Masternodes::join_shared", "deferred (MG-10 S5)"),
    ("Masternodes::lock_shared_terms", "deferred (MG-10 S5)"),
    ("Masternodes::mark_withdrawal", "deferred (MG-10 S4)"),
    ("Masternodes::preview_dissolve", "deferred (MG-10 S6)"),
    ("Masternodes::preview_update_share", "deferred (MG-10 S6)"),
    ("Masternodes::recheck_withdrawal", "deferred (MG-10 S4)"),
    (
        "Masternodes::resolve_collateral",
        "not bound: always refuses collateral_lookup (MG-03 D11)",
    ),
    ("Masternodes::retry_withdrawal", "deferred (MG-10 S4)"),
    ("Masternodes::send_maintenance", "deferred (MG-10 S6)"),
    (
        "Masternodes::send_shared_registration",
        "deferred (MG-10 S5)",
    ),
    ("Masternodes::send_standby", "deferred (MG-10 S6)"),
    ("Masternodes::set_shared_fee", "deferred (MG-10 S5)"),
    ("Masternodes::shared_capability", "deferred (MG-10 S5)"),
    ("Masternodes::shared_message", "deferred (MG-10 S5)"),
    ("Masternodes::shared_session", "deferred (MG-10 S5)"),
    ("Masternodes::shared_sessions", "deferred (MG-10 S1)"),
    ("Masternodes::shared_shares", "deferred (MG-10 S6)"),
    ("Masternodes::sign_shared_funding", "deferred (MG-10 S5)"),
    ("Masternodes::standby", "deferred (MG-10 S6)"),
    (
        "Masternodes::start_dissolve_together",
        "deferred (MG-10 S6)",
    ),
    ("Masternodes::start_key_change", "deferred (MG-10 S6)"),
    ("Masternodes::start_shared", "deferred (MG-10 S5)"),
    ("Masternodes::unreadable_shared_sessions", "deferred (MG-10 S5)"),
    ("Masternodes::unlock_shared_terms", "deferred (MG-10 S5)"),
    ("Masternodes::update_share", "deferred (MG-10 S6)"),
    ("Masternodes::withdraw_credits", "deferred (MG-10 S4)"),
    ("Masternodes::withdrawal_quote", "deferred (MG-10 S4)"),
    ("Masternodes::withdrawals", "deferred (MG-10 S4)"),
    (
        "dw_engine::masternodes::standby_file_transactions",
        "deferred (MG-10 S5)",
    ),
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
}
