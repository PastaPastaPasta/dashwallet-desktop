//! ProTx management (MG-04, mn-engine §2.3): the owner's ProRegTx flows.
//! Each signing call begins its own lease from a `MasternodeOp` grant,
//! signs with its `MasternodeOwner` signer (the collateral message, the
//! inputs; never as operator), saves the row durably, and hands the
//! transaction to `TxDraft`'s fenced path (§2). The lease ends when the
//! call returns.

mod build;
mod keys;
pub(crate) mod ops;
#[cfg(test)]
mod tests;
mod update;

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::OutPoint;
use dashcore::hashes::Hash;
use dw_appdb::MasternodeOpRow;
use dw_protx::params::collateral_amount;

use self::build::{
    Collateral, Funding, decode_signature, existing_coin, finish_registration, plan_funding,
    sign_message, summarize, to_sign, unsigned_registration,
};
use self::ops::{Delivery, encode_inputs, kind_name, operation_of, state_name, state_of};
use super::protx_records::{
    CollateralChoice, ProTxKeys, ProTxOperation, ProTxOperationKind, ProTxOperationState,
    RegistrationPreview, RegistrationRequest, UpdateRegistrarPreview, UpdateRegistrarRequest,
};
use super::records::MasternodeType;
use super::{Masternodes, MasternodesError, parse_pro_tx_hash};
use crate::coins::{now_secs, wallet_owns};
use crate::platform::{BudgetPurpose, FlowKind, PlatformError};
use crate::send::{SendFailure, SpendCharge, lease_failure};
use crate::{EngineError, NetworkSession, WalletId};

/// Change pays a P2PKH script (a fresh BIP44 internal address, D2).
const CHANGE_LEN: usize = 25;

impl Masternodes {
    /// [`Self::op`], with a `wallet_not_found` that doesn't repeat the
    /// caller's 64 hex (MG-03 D23).
    async fn protx_op<T, F, Fut>(&self, f: F) -> Result<T, MasternodesError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<NetworkSession>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, MasternodesError>> + Send,
    {
        self.op(f).await.map_err(|e| match e {
            MasternodesError::Engine(EngineError::WalletNotFound(_)) => {
                EngineError::WalletNotFound("the requested wallet".into()).into()
            }
            e => e,
        })
    }
}

impl Masternodes {
    /// The keychain keys a registration of `kind` would use by default
    /// (D13): public keys and addresses only. Needs the synced list.
    pub async fn registration_keys(
        &self,
        wallet: WalletId,
        kind: MasternodeType,
    ) -> Result<ProTxKeys, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let ctx = s.protx_context(wallet).await?;
            s.default_keys(wallet, kind == MasternodeType::Evonode, &ctx.used)
                .await
        })
        .await
    }

    /// Validates `request` with Core's rules and plans its transaction;
    /// locks, reserves and issues nothing (§2). A `NewAddress` payout and a
    /// fund-new collateral address are issued at registration, so the
    /// preview leaves them empty.
    pub async fn preview_registration(
        &self,
        wallet: WalletId,
        request: RegistrationRequest,
    ) -> Result<RegistrationPreview, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            build::check_request(&request, s.network.core_network())?;
            let ctx = s.protx_context(wallet).await?;
            let reg = s
                .resolve_registration(wallet, &request, &ctx, false)
                .await?;
            let fence = s.coin_fence(wallet).await?;
            let w = s.wallet(&wallet).await?;
            let exclude = s.check_collateral(&fence, &w, &reg).await?;
            let coins = s.protx_candidates(&fence, &w, &exclude).await?;
            let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
            let funding = plan_funding(&utxos, &reg.outputs(), reg.sized_payload(), CHANGE_LEN)
                .map_err(EngineError::from)?;
            Ok(RegistrationPreview {
                fee: funding.fee,
                total_debit: funding.fee,
                collateral_amount: collateral_amount(reg.evo()),
                terms: reg.display,
            })
        })
        .await
    }

    /// Registers a masternode with a `FundNew` or `ExistingUtxo`
    /// collateral under a `MasternodeOp` grant (§2, §4). The row is saved
    /// before the hand-off; the result is the operation, whatever the
    /// hand-off's outcome (`sent`, or `not_sent` with its `last_error`).
    pub async fn register(
        &self,
        wallet: WalletId,
        request: RegistrationRequest,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        if matches!(request.collateral, CollateralChoice::External { .. }) {
            return Err(EngineError::InvalidArgument(
                "an external collateral is registered with prepare_external_registration".into(),
            )
            .into());
        }
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            build::check_request(&request, s.network.core_network())?;
            let ctx = s.protx_context(wallet).await?;
            let reg = s.resolve_registration(wallet, &request, &ctx, true).await?;
            let lease = s
                .begin_lease(wallet, FlowKind::MasternodeRegistration, &[grant])
                .await
                .map_err(PlatformError::from)?;
            let id = ops::new_id();
            let result = async {
                let _flight = s.claim_operation(wallet, &id)?;
                let network = s.network.core_network();
                let fence = s.coin_fence(wallet).await?;
                let w = s.wallet(&wallet).await?;
                let snapshot = s.coin_snapshot(&fence, &w).await?;
                let existing = match &reg.collateral {
                    Collateral::Existing { outpoint } => {
                        let state = w.state().await;
                        Some(existing_coin(
                            &snapshot,
                            &state.core_wallet,
                            *outpoint,
                            reg.evo(),
                            network,
                        )?)
                    }
                    _ => None,
                };
                let exclude: HashSet<OutPoint> =
                    existing.iter().map(|(u, _, _)| u.outpoint).collect();
                let coins = s.protx_candidates(&fence, &w, &exclude).await?;
                let utxos: Vec<_> = coins.iter().map(|(u, _)| u.clone()).collect();
                let funding = plan_funding(&utxos, &reg.outputs(), reg.sized_payload(), CHANGE_LEN)
                    .map_err(EngineError::from)?;
                let (charge, signer) = lease
                    .charge_tx_signer(BudgetPurpose::Masternode, funding.fee)
                    .map_err(|e| lease_failure(e, funding.fee))?;
                let charge = SpendCharge::new(lease.clone(), BudgetPurpose::Masternode, charge);
                let change = match funding.change {
                    Some(_) => Some(
                        (w.core().next_change_address_for_account(0).await)
                            .map_err(EngineError::from)?,
                    ),
                    None => None,
                };
                let tx = unsigned_registration(&reg, &funding, change.as_ref())?;
                let signature = match &existing {
                    Some((_, path, _)) => {
                        let message = sign_message(&tx, network)?;
                        let b64 =
                            dw_vault::WalletSigner::sign_message(&signer, path, message.as_bytes())
                                .await
                                .map_err(|e| {
                                    EngineError::Internal(format!("collateral message: {e}"))
                                })?;
                        Some(decode_signature(&b64).ok_or_else(|| {
                            EngineError::Internal("the collateral signature is not 65 bytes".into())
                        })?)
                    }
                    None => None,
                };
                let collateral = existing
                    .as_ref()
                    .map(|(u, _, _)| (&u.txout.script_pubkey, Some(u.value())));
                let tx = finish_registration(
                    tx,
                    signature,
                    collateral,
                    &coins,
                    &ctx.listed,
                    &signer,
                    network,
                )
                .await?;
                #[cfg(test)]
                ops::crash_at(&s, ops::CrashPoint::Signed)?;
                let txid = tx.txid();
                let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
                let mut terms = reg.display.clone();
                let fund_new = match &reg.collateral {
                    Collateral::FundNew { .. } => {
                        build::registration_payload(&tx).map(|p| OutPoint {
                            txid,
                            vout: p.collateral_outpoint.vout,
                        })
                    }
                    _ => None,
                };
                if let Some(o) = fund_new {
                    terms.collateral_outpoint = Some(super::outpoint_string(&o));
                }
                let lock_collateral = existing.as_ref().filter(|(_, _, locked)| !locked);
                let mut locks = inputs.clone();
                locks.extend(lock_collateral.map(|(u, _, _)| u.outpoint));
                let now = now_secs();
                let row = MasternodeOpRow {
                    id: id.clone(),
                    kind: kind_name(ProTxOperationKind::Registration).into(),
                    terms: json(&terms)?,
                    request: json(&request)?,
                    unsigned_tx: None,
                    sign_address: None,
                    sign_message: None,
                    fee_inputs: encode_inputs(&inputs),
                    signed_tx: Some(dashcore::consensus::serialize(&tx)),
                    txid: Some(txid.to_string()),
                    pro_tx_hash: Some(txid.to_string()),
                    locked_collateral: lock_collateral.is_some(),
                    fee: funding.fee,
                    created_at: now,
                    signed_at: Some(now),
                    sent_at: None,
                    last_error: None,
                };
                s.save_operation(wallet, row, locks).await?;
                #[cfg(test)]
                ops::crash_at(&s, ops::CrashPoint::Saved)?;
                let summary = s
                    .summary_of(&w, &tx, &utxos, &funding, change.as_ref())
                    .await;
                s.deliver(
                    wallet,
                    Delivery {
                        id: id.clone(),
                        tx,
                        summary,
                        charge,
                        fence,
                        maybe_out: false,
                        lock_after: fund_new,
                    },
                )
                .await?;
                s.operation(wallet, &id).await
            }
            .await;
            lease.end();
            result
        })
        .await
    }

    /// Prepares an `External` registration (D4, D8, D19): plans the
    /// transaction, locks its fee inputs, and saves the row as
    /// `needs_signature` with Core's message for the collateral's key.
    /// Takes no grant and signs nothing.
    pub async fn prepare_external_registration(
        &self,
        wallet: WalletId,
        request: RegistrationRequest,
    ) -> Result<ProTxOperation, MasternodesError> {
        if !matches!(request.collateral, CollateralChoice::External { .. }) {
            return Err(EngineError::InvalidArgument(
                "only an external collateral is prepared; register the others".into(),
            )
            .into());
        }
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            build::check_request(&request, s.network.core_network())?;
            let ctx = s.protx_context(wallet).await?;
            let reg = s.resolve_registration(wallet, &request, &ctx, true).await?;
            let id = ops::new_id();
            let _flight = s.claim_operation(wallet, &id)?;
            let fence = s.coin_fence(wallet).await?;
            let row = s.prepared_row(wallet, &fence, &reg, &id, &request).await?;
            let inputs = ops::decode_inputs(&row.fee_inputs);
            s.save_operation(wallet, row, inputs).await?;
            drop(fence);
            s.operation(wallet, &id).await
        })
        .await
    }

    /// Finishes an `External` registration (§2): checks `signature`
    /// (base64, Core's `signmessage`) by the collateral address over the
    /// saved message, then signs the inputs under a `MasternodeOp` grant and
    /// sends as [`Self::register`] does.
    pub async fn submit_external_registration(
        &self,
        wallet: WalletId,
        operation: String,
        signature: String,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let _flight = s.claim_operation(wallet, &operation)?;
            let row = s.op_row(wallet, &operation).await?;
            let state = s.op_state(wallet, &row).await?;
            if state != ProTxOperationState::NeedsSignature {
                return Err(MasternodesError::OperationState {
                    state: state_name(state).into(),
                });
            }
            let network = s.network.core_network();
            let (Some(unsigned), Some(address), Some(message)) =
                (&row.unsigned_tx, &row.sign_address, &row.sign_message)
            else {
                return Err(
                    EngineError::Internal("a prepared row without its message".into()).into(),
                );
            };
            let key_id = keys::p2pkh_id(address, network)
                .ok_or_else(|| EngineError::Internal("stored collateral address".into()))?;
            let sig = decode_signature(&signature)
                .filter(|sig| {
                    let hash = dashcore::sign_message::signed_msg_hash(message);
                    dw_protx::rules::signed_by(sig, hash.to_byte_array(), key_id)
                })
                .ok_or(MasternodesError::SignatureMismatch)?;
            let tx: dashcore::Transaction = dashcore::consensus::deserialize(unsigned)
                .map_err(|e| EngineError::Internal(format!("stored transaction: {e}")))?;
            let ctx = s.protx_context(wallet).await?;
            let lease = s
                .begin_lease(wallet, FlowKind::MasternodeRegistration, &[grant])
                .await
                .map_err(PlatformError::from)?;
            let result = async {
                let fence = s.coin_fence(wallet).await?;
                let w = s.wallet(&wallet).await?;
                let coins = s.input_coins(&fence, &w, &tx, true).await?;
                let (charge, signer) = lease
                    .charge_tx_signer(BudgetPurpose::Masternode, row.fee)
                    .map_err(|e| lease_failure(e, row.fee))?;
                let charge = SpendCharge::new(lease.clone(), BudgetPurpose::Masternode, charge);
                let script =
                    dashcore::ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array(key_id));
                let tx = finish_registration(
                    tx,
                    Some(sig),
                    Some((&script, None)),
                    &coins,
                    &ctx.listed,
                    &signer,
                    network,
                )
                .await?;
                let txid = tx.txid();
                let (w_id, id, bytes, now) = (
                    wallet.to_string(),
                    operation.clone(),
                    dashcore::consensus::serialize(&tx),
                    now_secs(),
                );
                let signed = s
                    .appdb_op(move |db| {
                        db.sign_masternode_op_durable(
                            &w_id,
                            &id,
                            &bytes,
                            &txid.to_string(),
                            &txid.to_string(),
                            now,
                        )
                    })
                    .await?;
                if !signed {
                    return Err(MasternodesError::OperationNotFound);
                }
                let utxos: Vec<_> = coins.iter().map(|(u, _)| u.clone()).collect();
                let funding = Funding {
                    inputs: utxos.clone(),
                    fee: row.fee,
                    change: None,
                };
                let summary = s.summary_of(&w, &tx, &utxos, &funding, None).await;
                s.deliver(
                    wallet,
                    Delivery {
                        id: operation.clone(),
                        tx,
                        summary,
                        charge,
                        fence,
                        maybe_out: false,
                        lock_after: None,
                    },
                )
                .await?;
                s.operation(wallet, &operation).await
            }
            .await;
            lease.end();
            result
        })
        .await
    }

    /// Re-plans an `External` row whose fee inputs were spent
    /// (`inputs_spent`): new inputs, a new message, the same terms. The
    /// old inputs are unlocked and the new ones locked.
    pub async fn refresh_external_registration(
        &self,
        wallet: WalletId,
        operation: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let _flight = s.claim_operation(wallet, &operation)?;
            let row = s.op_row(wallet, &operation).await?;
            let state = s.op_state(wallet, &row).await?;
            if !matches!(
                state,
                ProTxOperationState::NeedsSignature | ProTxOperationState::InputsSpent
            ) {
                return Err(MasternodesError::OperationState {
                    state: state_name(state).into(),
                });
            }
            let request: RegistrationRequest = serde_json::from_str(&row.request)
                .map_err(|e| EngineError::Internal(format!("stored request: {e}")))?;
            let old = ops::decode_inputs(&row.fee_inputs);
            let w_id = wallet.to_string();
            // The old inputs leave the row's locks first, so the new plan
            // may take the ones still unspent.
            s.appdb_op({
                let w_id = w_id.clone();
                let old = old.clone();
                move |db| {
                    let now = now_secs();
                    for o in &old {
                        db.unlock(&w_id, &o.txid.to_string(), o.vout, now)?;
                    }
                    Ok(())
                }
            })
            .await?;
            let terms: super::protx_records::ProTxTerms = serde_json::from_str(&row.terms)
                .map_err(|e| EngineError::Internal(format!("stored ProTx terms: {e}")))?;
            let reg = build::registration_of(&terms, s.network.core_network())?;
            let fence = s.coin_fence(wallet).await?;
            let fresh = match s
                .prepared_row(wallet, &fence, &reg, &operation, &request)
                .await
            {
                Ok(fresh) => fresh,
                Err(e) => {
                    // The row still lists the old inputs: keep them locked
                    // with it (D25).
                    let w_id = w_id.clone();
                    s.appdb_op(move |db| {
                        let now = now_secs();
                        for o in &old {
                            db.lock_manual(&w_id, &o.txid.to_string(), o.vout, now)?;
                        }
                        Ok(())
                    })
                    .await?;
                    return Err(e);
                }
            };
            let new_inputs = ops::decode_inputs(&fresh.fee_inputs);
            let id = operation.clone();
            let updated = s
                .appdb_op(move |db| {
                    let now = now_secs();
                    for o in &new_inputs {
                        db.lock_manual(&w_id, &o.txid.to_string(), o.vout, now)?;
                    }
                    db.reprepare_masternode_op(
                        &w_id,
                        &id,
                        fresh.unsigned_tx.as_deref().unwrap_or_default(),
                        fresh.sign_message.as_deref().unwrap_or_default(),
                        &fresh.fee_inputs,
                        fresh.fee,
                    )
                })
                .await?;
            drop(fence);
            if !updated {
                return Err(MasternodesError::OperationNotFound);
            }
            s.masternodes.local_change();
            s.operation(wallet, &operation).await
        })
        .await
    }

    /// Validates `request` against the synced list and the wallet and
    /// plans its ProUpRegTx (§2): the fee, the owner key that would sign,
    /// what changes. Issues, locks and signs nothing.
    pub async fn preview_update_registrar(
        &self,
        wallet: WalletId,
        request: UpdateRegistrarRequest,
    ) -> Result<UpdateRegistrarPreview, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            update::check_update_request(&request, s.network.core_network())?;
            let ctx = s.protx_context(wallet).await?;
            let up = s.resolve_update(wallet, &request, &ctx, false).await?;
            let fence = s.coin_fence(wallet).await?;
            let w = s.wallet(&wallet).await?;
            let coins = s.protx_candidates(&fence, &w, &HashSet::new()).await?;
            let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
            let funding = plan_funding(&utxos, &up.outputs(), up.sized_payload(), CHANGE_LEN)
                .map_err(EngineError::from)?;
            Ok(UpdateRegistrarPreview {
                version: up.placeholder.version,
                fee: funding.fee,
                total_debit: funding.fee,
                owner_key: up.owner_source,
                operator_changes: up.operator_changes,
                voting_changes: up.voting_changes,
                payout_changes: up.payout_changes,
                // Core resets the operator's fields and bans the masternode
                // when the operator key changes (rc.3
                // `evo/specialtxman.cpp:538,557-561`).
                pose_bans: up.operator_changes,
                terms: up.display,
            })
        })
        .await
    }

    /// A ProUpRegTx under a `key_change` lease from a `MasternodeOp` grant,
    /// signed by the masternode's owner key (keychain or attached, D12) and
    /// sent as [`Self::register`] sends: saved before the hand-off, through
    /// the fence and its reservations. The lease is charged the fee.
    pub async fn update_registrar(
        &self,
        wallet: WalletId,
        request: UpdateRegistrarRequest,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            update::check_update_request(&request, s.network.core_network())?;
            let ctx = s.protx_context(wallet).await?;
            let up = s.resolve_update(wallet, &request, &ctx, true).await?;
            let lease = s
                .begin_lease(wallet, FlowKind::KeyChange, &[grant])
                .await
                .map_err(PlatformError::from)?;
            let id = ops::new_id();
            let result = async {
                let _flight = s.claim_operation(wallet, &id)?;
                let fence = s.coin_fence(wallet).await?;
                let w = s.wallet(&wallet).await?;
                let coins = s.protx_candidates(&fence, &w, &HashSet::new()).await?;
                let utxos: Vec<_> = coins.iter().map(|(u, _)| u.clone()).collect();
                let funding = plan_funding(&utxos, &up.outputs(), up.sized_payload(), CHANGE_LEN)
                    .map_err(EngineError::from)?;
                let (charge, signer) = lease
                    .charge_tx_signer(BudgetPurpose::Masternode, funding.fee)
                    .map_err(|e| lease_failure(e, funding.fee))?;
                let charge = SpendCharge::new(lease.clone(), BudgetPurpose::Masternode, charge);
                let change = match funding.change {
                    Some(_) => Some(
                        (w.core().next_change_address_for_account(0).await)
                            .map_err(EngineError::from)?,
                    ),
                    None => None,
                };
                let tx = update::unsigned_update(&up, &funding, change.as_ref())?;
                let tx = update::finish_update(tx, &up, &coins, &ctx, &signer).await?;
                #[cfg(test)]
                ops::crash_at(&s, ops::CrashPoint::Signed)?;
                let txid = tx.txid();
                let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
                let now = now_secs();
                let row = MasternodeOpRow {
                    id: id.clone(),
                    kind: kind_name(ProTxOperationKind::UpdateRegistrar).into(),
                    terms: json(&up.display)?,
                    request: json(&request)?,
                    unsigned_tx: None,
                    sign_address: None,
                    sign_message: None,
                    fee_inputs: encode_inputs(&inputs),
                    signed_tx: Some(dashcore::consensus::serialize(&tx)),
                    txid: Some(txid.to_string()),
                    pro_tx_hash: Some(up.placeholder.pro_tx_hash.to_string()),
                    locked_collateral: false,
                    fee: funding.fee,
                    created_at: now,
                    signed_at: Some(now),
                    sent_at: None,
                    last_error: None,
                };
                s.save_operation(wallet, row, inputs).await?;
                #[cfg(test)]
                ops::crash_at(&s, ops::CrashPoint::Saved)?;
                let summary = s
                    .summary_of(&w, &tx, &utxos, &funding, change.as_ref())
                    .await;
                s.deliver(
                    wallet,
                    Delivery {
                        id: id.clone(),
                        tx,
                        summary,
                        charge,
                        fence,
                        maybe_out: false,
                        lock_after: None,
                    },
                )
                .await?;
                s.operation(wallet, &id).await
            }
            .await;
            lease.end();
            result
        })
        .await
    }

    /// Every ProTx operation of the wallet, newest first, its state derived
    /// now (§5). Settled rows' input locks are released on the way (D25).
    pub async fn operations(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<ProTxOperation>, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            if let Err(e) = s.settle_operation_locks(wallet).await {
                tracing::debug!(error = %e, "releasing settled ProTx input locks");
            }
            let w = wallet.to_string();
            let rows = s.appdb_op(move |db| db.masternode_ops(&w)).await?;
            let (history, unspent) = s.op_view(wallet).await?;
            rows.iter()
                .map(|r| operation_of(r, state_of(r, &history, &unspent)))
                .collect()
        })
        .await
    }

    /// Hands a signed, unsettled row's saved bytes to the fence again (D7):
    /// under its own lease from a `MasternodeOp` grant, charged the row's
    /// fee, its inputs reserved again unless another owner holds one.
    pub async fn resend_operation(
        &self,
        wallet: WalletId,
        operation: String,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let _flight = s.claim_operation(wallet, &operation)?;
            let row = s.op_row(wallet, &operation).await?;
            let state = s.op_state(wallet, &row).await?;
            if !matches!(
                state,
                ProTxOperationState::Signed
                    | ProTxOperationState::NotSent
                    | ProTxOperationState::Sent
            ) {
                return Err(MasternodesError::OperationState {
                    state: state_name(state).into(),
                });
            }
            let tx = ops::signed_tx(&row)
                .ok_or_else(|| EngineError::Internal("stored signed transaction".into()))?;
            let flow = match row.kind.as_str() {
                "update_registrar" => FlowKind::KeyChange,
                _ => FlowKind::MasternodeRegistration,
            };
            let lease = s
                .begin_lease(wallet, flow, &[grant])
                .await
                .map_err(PlatformError::from)?;
            let result = async {
                let fence = s.coin_fence(wallet).await?;
                let w = s.wallet(&wallet).await?;
                let maybe_out = row.sent_at.is_some();
                let others =
                    (s.spends).held_by_others(&wallet, crate::send::SpendOwner::Tx(tx.txid()));
                if let Some(i) = tx
                    .input
                    .iter()
                    .find(|i| others.contains(&i.previous_output))
                {
                    return Err(EngineError::from(SendFailure::OutpointUnavailable(
                        i.previous_output,
                    ))
                    .into());
                }
                let coins = s.input_coins(&fence, &w, &tx, false).await?;
                let (charge, _) = lease
                    .charge_tx_signer(BudgetPurpose::Masternode, row.fee)
                    .map_err(|e| lease_failure(e, row.fee))?;
                let charge = SpendCharge::new(lease.clone(), BudgetPurpose::Masternode, charge);
                let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
                let funding = Funding {
                    inputs: utxos.clone(),
                    fee: row.fee,
                    change: None,
                };
                let summary = s.summary_of(&w, &tx, &utxos, &funding, None).await;
                s.deliver(
                    wallet,
                    Delivery {
                        id: operation.clone(),
                        tx,
                        summary,
                        charge,
                        fence,
                        maybe_out,
                        lock_after: None,
                    },
                )
                .await?;
                s.operation(wallet, &operation).await
            }
            .await;
            lease.end();
            result
        })
        .await
    }

    /// Ends a row nothing of which may be out: `needs_signature`,
    /// `inputs_spent`, `signed` or `not_sent` (D6), unlocking what it
    /// locked. `OperationState` for any other.
    pub async fn discard_operation(
        &self,
        wallet: WalletId,
        operation: String,
    ) -> Result<(), MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            s.discard(wallet, &operation).await
        })
        .await
    }

    /// Unlocks the collateral coin of one of the wallet's masternodes, and
    /// remembers the release so the follower leaves it unlocked (D9,
    /// MG-00 D38). Sends nothing.
    pub async fn release_collateral(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
    ) -> Result<(), MasternodesError> {
        self.protx_op(
            move |s| async move { s.set_collateral_lock(wallet, &pro_tx_hash, false).await },
        )
        .await
    }

    /// Locks it again and clears the release.
    pub async fn lock_collateral(
        &self,
        wallet: WalletId,
        pro_tx_hash: String,
    ) -> Result<(), MasternodesError> {
        self.protx_op(
            move |s| async move { s.set_collateral_lock(wallet, &pro_tx_hash, true).await },
        )
        .await
    }
}

fn json<T: serde::Serialize>(value: &T) -> Result<String, MasternodesError> {
    serde_json::to_string(value)
        .map_err(|e| EngineError::Internal(format!("encoding a ProTx record: {e}")).into())
}

impl NetworkSession {
    /// Under the fence: an `ExistingUtxo` collateral must be one of the
    /// wallet's coins of exactly the type's amount (§6); returned so the
    /// plan doesn't spend it.
    async fn check_collateral(
        &self,
        fence: &crate::coins::CoinFence,
        w: &platform_wallet::wallet::platform_wallet::PlatformWallet,
        reg: &build::Registration,
    ) -> Result<HashSet<OutPoint>, MasternodesError> {
        let Collateral::Existing { outpoint } = &reg.collateral else {
            return Ok(HashSet::new());
        };
        let snapshot = self.coin_snapshot(fence, w).await?;
        let state = w.state().await;
        let (coin, _, _) = existing_coin(
            &snapshot,
            &state.core_wallet,
            *outpoint,
            reg.evo(),
            self.network.core_network(),
        )?;
        Ok(HashSet::from([coin.outpoint]))
    }

    /// An `External` row, planned under `fence`: the unsigned transaction
    /// with its inputs hash, Core's message, the fee inputs to lock.
    async fn prepared_row(
        &self,
        wallet: WalletId,
        fence: &crate::coins::CoinFence,
        reg: &build::Registration,
        id: &str,
        request: &RegistrationRequest,
    ) -> Result<MasternodeOpRow, MasternodesError> {
        let Collateral::External { address, .. } = &reg.collateral else {
            return Err(EngineError::Internal("not an external collateral".into()).into());
        };
        let network = self.network.core_network();
        let w = self.wallet(&wallet).await?;
        let coins = self.protx_candidates(fence, &w, &HashSet::new()).await?;
        let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
        let funding = plan_funding(&utxos, &reg.outputs(), reg.sized_payload(), CHANGE_LEN)
            .map_err(EngineError::from)?;
        let change = match funding.change {
            Some(_) => Some(
                (w.core().next_change_address_for_account(0).await).map_err(EngineError::from)?,
            ),
            None => None,
        };
        let tx = unsigned_registration(reg, &funding, change.as_ref())?;
        let message = sign_message(&tx, network)?;
        let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
        Ok(MasternodeOpRow {
            id: id.to_string(),
            kind: kind_name(ProTxOperationKind::Registration).into(),
            terms: json(&reg.display)?,
            request: json(request)?,
            unsigned_tx: Some(dashcore::consensus::serialize(&tx)),
            sign_address: Some(address.to_string()),
            sign_message: Some(message),
            fee_inputs: encode_inputs(&inputs),
            signed_tx: None,
            txid: None,
            pro_tx_hash: None,
            locked_collateral: false,
            fee: funding.fee,
            created_at: now_secs(),
            signed_at: None,
            sent_at: None,
            last_error: None,
        })
    }

    /// The wallet coins `tx` spends, with their key paths, read under
    /// `fence`. `unspent`: each must still be an unspent coin of the wallet
    /// (a submit); a resend takes what the wallet still lists and signs
    /// nothing.
    async fn input_coins(
        &self,
        fence: &crate::coins::CoinFence,
        w: &platform_wallet::wallet::platform_wallet::PlatformWallet,
        tx: &dashcore::Transaction,
        unspent: bool,
    ) -> Result<Vec<(key_wallet::Utxo, key_wallet::DerivationPath)>, MasternodesError> {
        let snapshot = self.coin_snapshot(fence, w).await?;
        let state = w.state().await;
        let mut out = Vec::new();
        for input in &tx.input {
            let found = snapshot
                .coins
                .iter()
                .find(|c| c.utxo.outpoint == input.previous_output)
                .and_then(|c| {
                    let path =
                        crate::coins::own_address_info(&state.core_wallet, &c.utxo.address)?.path;
                    Some((c.utxo.clone(), path))
                });
            match found {
                Some(coin) => out.push(coin),
                None if unspent => {
                    return Err(MasternodesError::OperationState {
                        state: state_name(ProTxOperationState::InputsSpent).into(),
                    });
                }
                None => {}
            }
        }
        if unspent {
            to_sign(tx, &out)?;
        }
        Ok(out)
    }

    async fn summary_of(
        &self,
        w: &platform_wallet::wallet::platform_wallet::PlatformWallet,
        tx: &dashcore::Transaction,
        inputs: &[key_wallet::Utxo],
        funding: &Funding,
        change: Option<&dashcore::Address>,
    ) -> crate::send::PreparedSummary {
        let network = self.network.core_network();
        let state = w.state().await;
        let change = change.map(|a| a.script_pubkey());
        summarize(
            tx,
            inputs,
            funding.fee,
            change.as_ref(),
            |script| {
                dashcore::Address::from_script(script, network)
                    .is_ok_and(|a| wallet_owns(&state.core_wallet, &a))
            },
            network,
        )
    }

    async fn set_collateral_lock(
        self: &Arc<Self>,
        wallet: WalletId,
        pro_tx_hash: &str,
        lock: bool,
    ) -> Result<(), MasternodesError> {
        self.require_wallet(&wallet)?;
        let hash = parse_pro_tx_hash(pro_tx_hash)?;
        let manager = self.manager()?;
        let hub = Arc::clone(&self.hub);
        let owned = tokio::task::spawn_blocking(move || {
            super::follow::wallet_masternodes(&manager, &hub.history, &wallet).map(|(o, _)| o)
        })
        .await?
        .unwrap_or_default();
        let collateral = owned
            .iter()
            .find(|o| o.record.pro_tx_hash == hash)
            .ok_or(MasternodesError::NotFound)?
            .collateral
            .ok_or(MasternodesError::CollateralUnavailable)?;
        let _fence = self.coin_fence(wallet).await?;
        let (w, key) = (
            wallet.to_string(),
            format!("{}{}", ops::RELEASED, super::outpoint_string(&collateral)),
        );
        self.appdb_op(move |db| {
            let now = now_secs();
            let (txid, vout) = (collateral.txid.to_string(), collateral.vout);
            if lock {
                db.lock_manual(&w, &txid, vout, now)?;
                db.set_setting_durable(&w, &key, None)
            } else {
                db.set_setting_durable(&w, &key, Some(&now.to_string()))?;
                db.unlock(&w, &txid, vout, now).map(|_| ())
            }
        })
        .await?;
        self.masternodes.local_change();
        Ok(())
    }
}
