//! A reward change (MG-04b §5.1, D29, D31): the ProUpShareTx for one of
//! this wallet's shares of a confirmed session, previewed, or signed by the
//! share's keychain owner key and sent as an MG-04 operation of kind
//! `update_share` (slice 1b, §6.3).

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, ScriptBuf, TxIn, TxOut, Txid};
use dw_appdb::MasternodeOpRow;
use dw_protx::shared::rawtx::SPECIAL_VERSION;
use dw_protx::shared::rules::{self as shared_rules, ShareReward, SharedMn};
use dw_protx::shared::session::funding::{decode_frozen, script_for};
use dw_protx::shared::{ProUpShareTx, SpecialTx, TRANSACTION_PROVIDER_UPDATE_SHARE, payload_bytes};
use key_wallet::{DerivationPath, Utxo};

use super::super::MasternodesError;
use super::super::protx::build::{
    Context, Funding, SEQUENCE, plan_funding, preview_script, refused,
};
use super::super::protx::ops::{self, Delivery, encode_inputs, kind_name};
use super::super::protx::{CHANGE_LEN, change_address, charge_fee, json};
use super::super::protx_records::{
    ProTxOperation, ProTxOperationKind, ProTxOperationState, ProTxTerms,
};
use super::super::records::MasternodeType;
use super::super::shared_records::{RewardChoice, UpdateShareRequest};
use super::build::sign_own_inputs;
use super::flow::{REWARD_LABEL, signer_refusal};
use super::store::envelope_refusal;
use crate::coins::{CoinFence, now_secs};
use crate::{EngineError, NetworkSession, WalletId};

/// A reward change planned under a coin fence.
pub(super) struct ShareUpdate {
    pub payload: ProUpShareTx,
    pub tx: SpecialTx,
    pub funding: Funding,
    pub coins: Vec<(Utxo, DerivationPath)>,
    pub change: Option<Address>,
    /// The keychain owner key index of the share (D10).
    pub owner_index: u32,
    pub reward_script: ScriptBuf,
    pub refund_script: ScriptBuf,
    /// The share's owner key id, which its signature must recover to.
    pub owner_key_id: [u8; 20],
    pub mn_terms: ProTxTerms,
}

/// The ProUpShareTx spending `funding`, its change to `change`: `p` takes
/// the inputs hash, the transaction its payload.
fn update_share_tx(p: &mut ProUpShareTx, funding: &Funding, change: ScriptBuf) -> SpecialTx {
    let mut tx = SpecialTx {
        version: SPECIAL_VERSION,
        tx_type: TRANSACTION_PROVIDER_UPDATE_SHARE,
        input: funding
            .inputs
            .iter()
            .map(|u| TxIn {
                previous_output: u.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: SEQUENCE,
                witness: Default::default(),
            })
            .collect(),
        output: funding
            .change
            .map(|value| TxOut {
                value,
                script_pubkey: change,
            })
            .into_iter()
            .collect(),
        lock_time: 0,
        payload: Vec::new(),
    };
    p.inputs_hash = tx.inputs_hash();
    tx.payload = payload_bytes(p);
    tx
}

impl NetworkSession {
    /// The reward change `request` asks for, under `fence`: from the
    /// registration of a confirmed session of the wallet holding the share,
    /// one block deep and in the synced list (m-16), its fee inputs planned
    /// and Core's rules (§5.1) run on it. `commit`: issue a `NewAddress`
    /// reward and the change address; a preview stands in for them.
    pub(super) async fn plan_share_update(
        &self,
        wallet: WalletId,
        request: &UpdateShareRequest,
        hash: [u8; 32],
        ctx: &Context,
        fence: &CoinFence,
        commit: bool,
    ) -> Result<ShareUpdate, MasternodesError> {
        let network = self.network.core_network();
        let txid = Txid::from_byte_array(hash);
        let (l, height) = self.shared_confirmed(wallet, txid).await?;
        let listed = ctx.entries.iter().find(|e| e.pro_tx_hash == hash);
        if ctx.list_height.is_none_or(|h| h <= height) || listed.is_none() {
            return Err(MasternodesError::SharedTooEarly);
        }
        let index = usize::from(request.share_index);
        let recorded = l
            .recorded
            .clone()
            .filter(|r| r.share_index == index)
            .ok_or(MasternodesError::ShareNotMine)?;
        let frozen = decode_frozen(&l.session).map_err(envelope_refusal)?;
        let share = frozen
            .payload
            .shares
            .get(index)
            .ok_or(MasternodesError::ShareNotMine)?;
        let reward = match &request.reward {
            RewardChoice::Refund => ShareReward::Refund,
            RewardChoice::NewAddress if commit => {
                ShareReward::Script(self.issued(wallet, REWARD_LABEL).await?.script_pubkey())
            }
            RewardChoice::NewAddress => ShareReward::Script(preview_script()),
            RewardChoice::Address { address } => {
                ShareReward::Script(script_for(address.trim(), network))
            }
        };
        let reward_script = reward.script(share);
        let voting_key_id = listed.map_or(frozen.payload.voting_key_id, |e| e.voting_key_id);
        let mut payload = ProUpShareTx {
            version: ProUpShareTx::CURRENT_VERSION,
            pro_tx_hash: txid,
            share_index: request.share_index,
            reward_script: reward_script.clone(),
            inputs_hash: [0; 32],
            sig: vec![0; 65],
        };
        let w = self.wallet(&wallet).await?;
        let coins = self.protx_candidates(fence, &w, &HashSet::new(), None).await?;
        let utxos: Vec<_> = coins.iter().map(|(u, _)| u.clone()).collect();
        let funding = plan_funding(&utxos, &[], payload_bytes(&payload).len(), CHANGE_LEN)
            .map_err(EngineError::from)?;
        let change = match commit {
            true => change_address(&w, &funding).await?,
            false => None,
        };
        let change_script = change
            .as_ref()
            .map_or_else(preview_script, Address::script_pubkey);
        let tx = update_share_tx(&mut payload, &funding, change_script);
        let mn = SharedMn {
            pro_tx_hash: txid,
            collateral: OutPoint::new(txid, frozen.payload.collateral.vout),
            shares: &frozen.payload.shares,
            registered_height: height,
            early_period_blocks: frozen.payload.early_period_blocks,
            early_penalty: frozen.payload.early_penalty,
            voting_key_id,
        };
        shared_rules::check_update_share(&tx, &payload, Some(&mn), true, false).map_err(refused)?;
        let terms = &l.session.terms;
        let reward_address = Address::from_script(&reward_script, network)
            .map(|a| a.to_string())
            .unwrap_or_default();
        let mn_terms = ProTxTerms {
            masternode_type: MasternodeType::Regular,
            version: payload.version,
            service: terms.core_p2p_addrs.clone(),
            owner_address: recorded.owner_address.clone(),
            owner_index: Some(recorded.owner_index),
            voting_address: terms.voting_address.clone(),
            voting_index: None,
            operator_public_key: terms.operator_pub_key.clone(),
            operator_index: None,
            operator_reward: u16::try_from(terms.operator_reward).unwrap_or(0),
            payout_address: reward_address,
            collateral: None,
            collateral_outpoint: Some(format!("{txid}-{}", frozen.payload.collateral.vout)),
            collateral_address: None,
            platform: None,
            platform_node_index: None,
        };
        Ok(ShareUpdate {
            payload,
            tx,
            funding,
            coins,
            change,
            owner_index: recorded.owner_index,
            reward_script,
            refund_script: share.refund_script.clone(),
            owner_key_id: share.owner_key_id,
            mn_terms,
        })
    }

    /// The reward change (§6.3): planned under the fence, the payload
    /// signed by the share's keychain owner key and the fee inputs by the
    /// wallet's, both under one `SharedMasternode` lease charged the fee,
    /// saved as an `update_share` operation with its input locks, and
    /// handed off as MG-04's rows are. One at a time per masternode (D23):
    /// `update_pending{operation}` while another `update_share` of it may
    /// still be mined.
    pub(super) async fn send_share_update(
        self: &Arc<Self>,
        wallet: WalletId,
        request: UpdateShareRequest,
        hash: [u8; 32],
        grant: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let updating = self.masternodes.update_lock().await;
        // The fence from the reward address and the coins to the row's
        // write and its hand-off (BL-352's order).
        let fence = self.coin_fence(wallet).await?;
        self.check_no_pending_share_update(wallet, hash).await?;
        let mut plan = self
            .plan_share_update(wallet, &request, hash, &ctx, &fence, true)
            .await?;
        let lease = self.shared_lease(wallet, grant).await?;
        let id = ops::new_id();
        let result = async {
            let _flight = self.claim_operation(wallet, &id)?;
            let fee = plan.funding.fee;
            let (charge, signer) = charge_fee(&lease, fee)?;
            let sig = signer
                .sign_update_share(plan.owner_index, &payload_bytes(&plan.payload))
                .map_err(signer_refusal)?;
            // The keychain key must be the share's (the vault signs what
            // it's given, D11), checked as Core checks every share
            // signature: else Core refuses the bytes.
            if !shared_rules::verify_hash_canonical(
                plan.payload.sign_hash(),
                plan.owner_key_id,
                &sig,
            ) {
                return Err(MasternodesError::ShareNotMine);
            }
            plan.payload.sig = sig.to_vec();
            plan.tx.payload = payload_bytes(&plan.payload);
            let own: Vec<_> = plan
                .coins
                .iter()
                .filter(|(u, _)| plan.funding.inputs.iter().any(|i| i.outpoint == u.outpoint))
                .map(|(u, p)| (u.outpoint, u.txout.script_pubkey.clone(), p.clone()))
                .collect();
            let signed = sign_own_inputs(&plan.tx, &own, &signer).await?;
            let tx = super::send::typed(&signed)?;
            let txid = tx.txid();
            let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
            let now = now_secs();
            let row = MasternodeOpRow {
                id: id.clone(),
                kind: kind_name(ProTxOperationKind::UpdateShare).into(),
                terms: json(&plan.mn_terms)?,
                request: json(&request)?,
                unsigned_tx: None,
                sign_address: None,
                sign_message: None,
                fee_inputs: encode_inputs(&inputs),
                signed_tx: Some(signed.to_bytes()),
                txid: Some(txid.to_string()),
                pro_tx_hash: Some(Txid::from_byte_array(hash).to_string()),
                locked_collateral: false,
                fee,
                created_at: now,
                signed_at: Some(now),
                sent_at: None,
                last_error: None,
            };
            self.save_operation(wallet, row, inputs).await?;
            drop(updating);
            let w = self.wallet(&wallet).await?;
            let summary = self
                .summary_of(
                    &w,
                    &tx,
                    &plan.funding.inputs,
                    plan.funding.fee,
                    plan.change.as_ref(),
                )
                .await;
            self.deliver(
                wallet,
                Delivery {
                    id: id.clone(),
                    tx,
                    summary,
                    charge,
                    fence,
                    maybe_out: false,
                },
            )
            .await?;
            self.operation(wallet, &id).await
        }
        .await;
        lease.end();
        result
    }

    /// D23: another `update_share` of masternode `hash` that may still be
    /// mined (`signed`, `not_sent`, `sent`) is `update_pending`.
    async fn check_no_pending_share_update(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
    ) -> Result<(), MasternodesError> {
        let target = Txid::from_byte_array(hash).to_string();
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        for row in rows.iter().filter(|r| {
            r.kind == kind_name(ProTxOperationKind::UpdateShare)
                && r.pro_tx_hash.as_deref() == Some(target.as_str())
        }) {
            if matches!(
                self.op_state(wallet, row).await?,
                ProTxOperationState::Signed
                    | ProTxOperationState::NotSent
                    | ProTxOperationState::Sent
            ) {
                return Err(MasternodesError::UpdatePending {
                    operation: row.id.clone(),
                });
            }
        }
        Ok(())
    }
}
