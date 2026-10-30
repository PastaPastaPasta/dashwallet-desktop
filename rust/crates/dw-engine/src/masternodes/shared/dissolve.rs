//! Dissolve now (MG-04b §5, §6.3, slice 2a): the unilateral ProDisTx by
//! one of this wallet's shares of a confirmed session, Core's
//! `BuildDissolution` output with the penalty `DissolveShared` picks
//! (rc.3 `providertx_service.cpp:639-681,1242-1279`), previewed, or signed
//! by the share's keychain owner key and sent as an MG-04 operation of kind
//! `dissolve`. It spends the collateral only: no fee inputs, no change.

use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, ScriptBuf, Txid};
use dw_appdb::MasternodeOpRow;
use dw_protx::shared::dissolve::{
    BuildError, Dissolution, build_dissolution, check_fee, is_early, penalty_free_from,
};
use dw_protx::shared::rules::{self as shared_rules, SharedMn};
use dw_protx::shared::session::funding::decode_frozen;
use dw_protx::shared::{CollateralShare, ProDisTx};
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use serde::{Deserialize, Serialize};

use super::super::MasternodesError;
use super::super::follow::ListView;
use super::super::protx::build::{Context, invalid, refused};
use super::super::protx::ops::{self, Delivery, kind_name, kind_of};
use super::super::protx::{charge_fee, json};
use super::super::protx_records::{
    ProTxOperation, ProTxOperationKind, ProTxOperationState, ProTxTerms,
};
use super::super::records::MasternodeType;
use super::super::shared_records::{DissolveRequest, ShareReturn, SharedWarning};
use super::flow::signer_refusal;
use super::store::envelope_refusal;
use crate::coins::now_secs;
use crate::platform::{FlowKind, PlatformError};
use crate::send::MIN_FEE_PER_KB;
use crate::send::plan::{dust_threshold, fee_for};
use crate::{EngineError, NetworkSession, WalletId};

/// A `dissolve` row's `request`: the caller's, with what the share gives
/// up beyond the fee and what it gets back, which the row's signed bytes
/// alone don't tell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DissolveRecord {
    #[serde(flatten)]
    pub request: DissolveRequest,
    pub penalty: u64,
    /// The actor's output, 0 when the transaction omits it (D27).
    pub actor_output: u64,
}

/// What a row's resend is charged: a `dissolve` row what its share gives
/// up, the penalty and the fee (§6.3); any other its fee.
pub(crate) fn row_debit(row: &MasternodeOpRow) -> Result<u64, MasternodesError> {
    if kind_of(&row.kind) != ProTxOperationKind::Dissolve {
        return Ok(row.fee);
    }
    let record: DissolveRecord = serde_json::from_str(&row.request)
        .map_err(|e| EngineError::Internal(format!("stored dissolution: {e}")))?;
    Ok(record.debit())
}

impl DissolveRecord {
    /// What the share gives up: the penalty and the fee.
    pub(crate) fn debit(&self) -> u64 {
        self.penalty.saturating_add(self.request.fee)
    }
}

/// Core's builder's refusals are `INVALID_PARAMETER`s, with its texts.
fn build_refusal(e: BuildError) -> MasternodesError {
    invalid(&e.to_string())
}

/// The argument checks before the wallet's (§7's order).
pub(super) fn check_request(request: &DissolveRequest) -> Result<(), MasternodesError> {
    check_fee(request.fee).map_err(build_refusal)
}

/// A dissolution planned from the session's registration and the list.
pub(super) struct DissolvePlan {
    pub dissolution: Dissolution,
    pub penalty: u64,
    pub early: bool,
    pub valid_from_height: u32,
    pub actor_output: u64,
    pub returns: Vec<ShareReturn>,
    pub warnings: Vec<SharedWarning>,
    /// The keychain owner key index of the share (D10), and the key id
    /// its signature must recover to.
    pub owner_index: u32,
    pub owner_key_id: [u8; 20],
    pub collateral: OutPoint,
    pub mn_terms: ProTxTerms,
    shares: Vec<CollateralShare>,
    registered_height: u32,
    early_period_blocks: u32,
    early_penalty: i64,
    voting_key_id: [u8; 20],
    spend_height: u32,
}

impl DissolvePlan {
    fn mn(&self) -> SharedMn<'_> {
        SharedMn {
            pro_tx_hash: self.dissolution.payload.pro_tx_hash,
            collateral: self.collateral,
            shares: &self.shares,
            registered_height: self.registered_height,
            early_period_blocks: self.early_period_blocks,
            early_penalty: self.early_penalty,
            voting_key_id: self.voting_key_id,
        }
    }

    /// Core's ProDisTx rules (§5.1) on `sigs`' copy, mined at the next
    /// height; the signature checked when `check_sigs`.
    fn check(&self, sigs: Vec<[u8; 65]>, check_sigs: bool) -> Result<(), MasternodesError> {
        let tx = self.dissolution.signed(sigs.clone());
        let p = ProDisTx {
            sigs,
            ..self.dissolution.payload.clone()
        };
        shared_rules::check_dissolution(
            &tx,
            &p,
            Some(&self.mn()),
            self.spend_height,
            true,
            check_sigs,
        )
        .map_err(refused)
    }
}

impl NetworkSession {
    /// The dissolution `request` asks for: from the registration of a
    /// confirmed session of the wallet holding the share, one block deep
    /// and in the synced list (m-16); the penalty `DissolveShared` picks
    /// at the next height; Core's builder output, its fee at least the
    /// relay minimum for its size, and Core's rules run on it.
    pub(super) async fn plan_dissolve(
        &self,
        wallet: WalletId,
        request: &DissolveRequest,
        hash: [u8; 32],
        ctx: &Context,
    ) -> Result<DissolvePlan, MasternodesError> {
        let network = self.network.core_network();
        let txid = Txid::from_byte_array(hash);
        let (l, height) = self.shared_confirmed(wallet, txid).await?;
        let listed = ctx.entries.iter().find(|e| e.pro_tx_hash == hash);
        let (Some(list_height), Some(listed)) = (ctx.list_height.filter(|h| *h > height), listed)
        else {
            return Err(MasternodesError::SharedTooEarly);
        };
        let index = usize::from(request.share_index);
        let recorded = l
            .recorded
            .clone()
            .filter(|r| r.share_index == index)
            .ok_or(MasternodesError::ShareNotMine)?;
        let frozen = decode_frozen(&l.session).map_err(envelope_refusal)?;
        let p = frozen.payload;
        let actor = p
            .shares
            .get(index)
            .ok_or(MasternodesError::ShareNotMine)?
            .clone();
        let mn = SharedMn {
            pro_tx_hash: txid,
            collateral: OutPoint::new(txid, p.collateral.vout),
            shares: &p.shares,
            registered_height: height,
            early_period_blocks: p.early_period_blocks,
            early_penalty: p.early_penalty,
            voting_key_id: listed.voting_key_id,
        };
        let spend_height = list_height + 1;
        let early = is_early(&mn, spend_height);
        let penalty = if early {
            mn.early_penalty.max(0) as u64
        } else {
            0
        };
        let dissolution = build_dissolution(&mn, request.share_index, penalty, request.fee)
            .map_err(build_refusal)?;
        // The size with its one signature is exact.
        let size = dissolution.signed(vec![[0; 65]]).to_bytes().len();
        let floor = fee_for(FeeRate::new(MIN_FEE_PER_KB), size)
            .ok_or(EngineError::Send(crate::send::SendFailure::TxTooLarge))?;
        if request.fee < floor {
            return Err(invalid(&format!(
                "the fee is below the relay minimum of {floor} duffs"
            )));
        }
        let actor_output = dissolution.actor_output;
        let warnings = actor_warnings(actor_output, &actor.refund_script);
        // Outputs are the non-actors in share order, then the actor's.
        let mut paid = dissolution.tx.output.iter();
        let returns = p
            .shares
            .iter()
            .enumerate()
            .map(|(i, s)| ShareReturn {
                share_index: i as u16,
                amount: match i == index {
                    true => actor_output,
                    false => paid.next().map_or(0, |o| o.value),
                },
                refund_address: Address::from_script(&s.refund_script, network)
                    .ok()
                    .map(|a| a.to_string()),
                mine: i == index,
            })
            .collect();
        let terms = &l.session.terms;
        let mn_terms = ProTxTerms {
            masternode_type: MasternodeType::Regular,
            version: ProDisTx::CURRENT_VERSION,
            service: terms.core_p2p_addrs.clone(),
            owner_address: recorded.owner_address.clone(),
            owner_index: Some(recorded.owner_index),
            voting_address: terms.voting_address.clone(),
            voting_index: None,
            operator_public_key: terms.operator_pub_key.clone(),
            operator_index: None,
            operator_reward: u16::try_from(terms.operator_reward).unwrap_or(0),
            payout_address: Address::from_script(&actor.refund_script, network)
                .map(|a| a.to_string())
                .unwrap_or_default(),
            collateral: None,
            collateral_outpoint: Some(format!("{txid}-{}", p.collateral.vout)),
            collateral_address: None,
            platform: None,
            platform_node_index: None,
        };
        let plan = DissolvePlan {
            valid_from_height: penalty_free_from(&mn),
            collateral: mn.collateral,
            dissolution,
            penalty,
            early,
            actor_output,
            returns,
            warnings,
            owner_index: recorded.owner_index,
            owner_key_id: actor.owner_key_id,
            mn_terms,
            registered_height: height,
            early_period_blocks: p.early_period_blocks,
            early_penalty: p.early_penalty,
            voting_key_id: listed.voting_key_id,
            spend_height,
            shares: p.shares,
        };
        plan.check(vec![[0; 65]], false)?;
        Ok(plan)
    }

    /// Dissolve now (§6.3): planned under the fence, signed by the share's
    /// keychain owner key under one `Dissolution` lease charged the penalty
    /// and the fee, saved as a `dissolve` operation and handed off as
    /// MG-04's rows are. One at a time per masternode (D23):
    /// `update_pending{operation}` while an `update_share` or another
    /// `dissolve` of it may still be mined.
    pub(super) async fn send_dissolve(
        self: &Arc<Self>,
        wallet: WalletId,
        request: DissolveRequest,
        hash: [u8; 32],
        grant: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let updating = self.masternodes.update_lock().await;
        let fence = self.coin_fence(wallet).await?;
        self.check_no_pending_maintenance(wallet, hash).await?;
        let plan = self.plan_dissolve(wallet, &request, hash, &ctx).await?;
        // D83: the preview warns of a dust actor output; sending refuses
        // it, as no node would relay the transaction.
        if plan.warnings.contains(&SharedWarning::ActorOutputDust) {
            return Err(invalid(DUST_REFUSAL));
        }
        let registration = self.registration_bytes(wallet, hash).await?;
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let id = ops::new_id();
        let result = async {
            let _flight = self.claim_operation(wallet, &id)?;
            let (charge, signer) = charge_fee(&lease, plan.penalty + request.fee)?;
            let sig = signer
                .sign_dissolution(
                    plan.owner_index,
                    &plan.dissolution.tx.to_bytes(),
                    &registration,
                    1,
                )
                .map_err(signer_refusal)?;
            // The keychain key must be the actor share's (D11), checked as
            // Core checks it: else Core refuses the bytes.
            if !shared_rules::verify_hash_canonical(
                plan.dissolution.sign_hash(1),
                plan.owner_key_id,
                &sig,
            ) {
                return Err(MasternodesError::ShareNotMine);
            }
            plan.check(vec![sig], true)?;
            let signed = plan.dissolution.signed(vec![sig]);
            let tx = super::send::typed(&signed)?;
            let txid = tx.txid();
            let now = now_secs();
            let record = DissolveRecord {
                request: request.clone(),
                penalty: plan.penalty,
                actor_output: plan.actor_output,
            };
            let row = MasternodeOpRow {
                id: id.clone(),
                kind: kind_name(ProTxOperationKind::Dissolve).into(),
                terms: json(&plan.mn_terms)?,
                request: json(&record)?,
                unsigned_tx: None,
                sign_address: None,
                sign_message: None,
                // The collateral isn't the wallet's coin: no fee inputs,
                // no locks (§6.3).
                fee_inputs: ops::encode_inputs(&[]),
                signed_tx: Some(signed.to_bytes()),
                txid: Some(txid.to_string()),
                pro_tx_hash: Some(Txid::from_byte_array(hash).to_string()),
                locked_collateral: false,
                fee: request.fee,
                created_at: now,
                signed_at: Some(now),
                sent_at: None,
                last_error: None,
            };
            self.save_operation(wallet, row, Vec::new()).await?;
            drop(updating);
            let w = self.wallet(&wallet).await?;
            let summary = self.summary_of(&w, &tx, &[], request.fee, None).await;
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

    /// What [`list_settled`] reads: the synced list when `needed` (some
    /// row is an [`unseen_dissolution`]), else none, and the height the
    /// wallet has scanned to.
    pub(crate) async fn dissolution_list(&self, wallet: WalletId, needed: bool) -> (ListView, u32) {
        if !needed {
            return (ListView::default(), 0);
        }
        let list = match self.manager() {
            Ok(manager) => self.protx_list(&manager).await,
            Err(_) => ListView::default(),
        };
        let scanned = self.hub.wallet_state(&wallet).map_or(0, |s| s.tip());
        (list, scanned)
    }

    /// The masternode's registration from the wallet's history (D29), in
    /// the bytes the vault's dissolution signer reads.
    async fn registration_bytes(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
    ) -> Result<Vec<u8>, MasternodesError> {
        let chain = self.shared_chain(wallet).await?;
        chain
            .history
            .get(&Txid::from_byte_array(hash))
            .map(|e| dashcore::consensus::serialize(&e.tx))
            .ok_or(MasternodesError::SharedRegistrationUnknown)
    }
}

const DUST_REFUSAL: &str =
    "the actor's output is below the dust threshold and would not be relayed";

/// n-10: an actor output in (0, dust) is valid and Core's builder keeps
/// it, but it isn't relayed (`providertx_service.cpp:674-676`).
fn actor_warnings(actor_output: u64, refund: &ScriptBuf) -> Vec<SharedWarning> {
    match actor_output > 0 && actor_output < dust_threshold(refund.len()) {
        true => vec![SharedWarning::ActorOutputDust],
        false => Vec::new(),
    }
}

/// D27: the wallet sees its dissolution by the collateral leaving the
/// list. One that pays this wallet nothing (its actor output omitted) is
/// never in the history, so a `sent` one reads `confirmed`, at the list's
/// height, once its masternode is out of the synced list; every other
/// dissolution of it pays this wallet's refund and so is in the history
/// (`conflicted` there), but only once the wallet has scanned up to the
/// list's height: `scanned` below it, the masternode may have left by
/// such a dissolution the history doesn't hold yet, so it stays `sent`.
pub(crate) fn list_settled(
    row: &MasternodeOpRow,
    state: ProTxOperationState,
    list: &ListView,
    scanned: u32,
) -> ProTxOperationState {
    if state != ProTxOperationState::Sent || !unseen_dissolution(row) || !list.synced {
        return state;
    }
    let Some(height) = list.height.filter(|h| scanned >= *h) else {
        return state;
    };
    let gone = row
        .pro_tx_hash
        .as_deref()
        .and_then(|h| super::super::parse_pro_tx_hash(h).ok())
        .is_some_and(|hash| list.entry(&hash).is_none());
    match gone {
        true => ProTxOperationState::Confirmed { height },
        false => state,
    }
}

/// A `dissolve` row whose transaction omits its actor output.
pub(crate) fn unseen_dissolution(row: &MasternodeOpRow) -> bool {
    kind_of(&row.kind) == ProTxOperationKind::Dissolve
        && serde_json::from_str::<DissolveRecord>(&row.request).is_ok_and(|r| r.actor_output == 0)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use platform_wallet::masternode::MasternodeListSummary;

    use super::*;

    const HASH: [u8; 32] = [7; 32];

    fn row(actor_output: u64) -> MasternodeOpRow {
        let record = DissolveRecord {
            request: DissolveRequest {
                pro_tx_hash: Txid::from_byte_array(HASH).to_string(),
                share_index: 0,
                fee: 1_000,
            },
            penalty: 5,
            actor_output,
        };
        MasternodeOpRow {
            id: "a".into(),
            kind: "dissolve".into(),
            terms: "{}".into(),
            request: serde_json::to_string(&record).unwrap(),
            unsigned_tx: None,
            sign_address: None,
            sign_message: None,
            fee_inputs: "[]".into(),
            signed_tx: None,
            txid: None,
            pro_tx_hash: Some(Txid::from_byte_array(HASH).to_string()),
            locked_collateral: false,
            fee: 1_000,
            created_at: 0,
            signed_at: None,
            sent_at: None,
            last_error: None,
        }
    }

    fn list(synced: bool, entries: &[[u8; 32]]) -> ListView {
        ListView {
            synced,
            height: Some(120),
            entries: entries
                .iter()
                .map(|h| MasternodeListSummary {
                    pro_tx_hash: *h,
                    service_address: None,
                    platform_http_port: None,
                    operator_public_key: [1; 48],
                    voting_key_id: [1; 20],
                    platform_node_id: None,
                    is_valid: true,
                    is_evonode: false,
                    has_extended_net_info: true,
                    net_info: None,
                })
                .collect(),
        }
    }

    #[test]
    fn test_QT_127_an_unseen_sent_dissolution_confirms_when_its_masternode_leaves_the_list() {
        use ProTxOperationState::*;
        let gone = list(true, &[[8; 32]]);
        assert_eq!(
            list_settled(&row(0), Sent, &gone, 120),
            Confirmed { height: 120 }
        );
        // Still listed, the list unsynced, the wallet's scan behind the
        // list, or not sent: as derived.
        assert_eq!(list_settled(&row(0), Sent, &list(true, &[HASH]), 120), Sent);
        assert_eq!(list_settled(&row(0), Sent, &list(false, &[]), 120), Sent);
        assert_eq!(list_settled(&row(0), Sent, &gone, 119), Sent);
        for state in [Signed, NotSent, Conflicted, Confirmed { height: 3 }] {
            assert_eq!(list_settled(&row(0), state, &gone, 120), state);
        }
        // One paying this wallet is seen in the history, not the list.
        assert!(!unseen_dissolution(&row(1)));
        assert_eq!(list_settled(&row(1), Sent, &gone, 120), Sent);
        let update = MasternodeOpRow {
            kind: "update_share".into(),
            ..row(0)
        };
        assert_eq!(list_settled(&update, Sent, &gone, 120), Sent);
    }

    #[test]
    fn test_QT_127_a_resent_dissolution_is_charged_its_penalty_and_fee() {
        assert_eq!(row_debit(&row(0)).unwrap(), 1_005);
        let update = MasternodeOpRow {
            kind: "update_share".into(),
            request: "{}".into(),
            ..row(0)
        };
        assert_eq!(row_debit(&update).unwrap(), 1_000);
    }

    #[test]
    fn test_QT_127_an_actor_output_below_dust_warns() {
        let p2pkh = ScriptBuf::new_p2pkh(&dashcore::PubkeyHash::from_byte_array([1; 20]));
        let dust = dust_threshold(p2pkh.len());
        assert_eq!(dust, 546);
        assert!(actor_warnings(0, &p2pkh).is_empty(), "omitted, not dust");
        assert_eq!(
            actor_warnings(1, &p2pkh),
            vec![SharedWarning::ActorOutputDust]
        );
        assert_eq!(
            actor_warnings(dust - 1, &p2pkh),
            vec![SharedWarning::ActorOutputDust]
        );
        assert!(actor_warnings(dust, &p2pkh).is_empty());
    }
}
