//! Dissolve now (MG-04b §5, §6.3, slice 2a): the unilateral ProDisTx by
//! one of this wallet's shares of a confirmed session, Core's
//! `BuildDissolution` output with the penalty `DissolveShared` picks
//! (rc.3 `providertx_service.cpp:639-681,1242-1279`), previewed, or signed
//! by the share's keychain owner key and sent as an MG-04 operation of kind
//! `dissolve`. It spends the collateral only: no fee inputs, no change.

use std::collections::HashMap;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, ScriptBuf, TxOut, Txid};
use dw_appdb::MasternodeOpRow;
use dw_protx::shared::dissolve::{
    BuildError, Dissolution, build_dissolution, check_fee, is_early, penalty_free_from,
};
use dw_protx::shared::rules::{self as shared_rules, SharedMn};
use dw_protx::shared::session::funding::decode_frozen;
use dw_protx::shared::standby::early_penalty;
use dw_protx::shared::{CollateralShare, ProDisTx, SpecialTx};
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use platform_wallet::masternode::MasternodeListSummary;
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
use crate::coins::{CoinFence, now_secs};
use crate::platform::{FlowKind, PlatformError};
use crate::send::plan::{dust_threshold, fee_for};
use crate::send::{MIN_FEE_PER_KB, SpendCharge};
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
    /// The actor share isn't this wallet's (an imported standby, §9.4):
    /// the wallet gives up nothing (§6.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub foreign: bool,
    /// A dissolution every owner signed (`send_maintenance`, MG-04b D95):
    /// the shares gave up their fee at approval, so sending and resending
    /// it charge nothing (§6.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub together: bool,
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
    /// What the wallet's share gives up: the penalty and the fee, or
    /// nothing when the actor share isn't the wallet's or every owner
    /// signed (its approval was charged).
    pub(crate) fn debit(&self) -> u64 {
        match self.foreign || self.together {
            true => 0,
            false => self.penalty.saturating_add(self.request.fee),
        }
    }
}

/// A signed dissolution to save and hand off.
pub(super) struct HandOff<'a> {
    pub id: &'a str,
    pub signed: SpecialTx,
    pub record: DissolveRecord,
    pub terms: &'a ProTxTerms,
    pub hash: [u8; 32],
    pub charge: SpendCharge,
    pub fence: CoinFence,
}

/// Core's builder's refusals are `INVALID_PARAMETER`s, with its texts.
pub(super) fn build_refusal(e: BuildError) -> MasternodesError {
    invalid(&e.to_string())
}

/// The fee of `dissolution` is at least the relay minimum for its size,
/// which with its one signature is exact.
pub(super) fn check_relay_fee(dissolution: &Dissolution, fee: u64) -> Result<(), MasternodesError> {
    check_relay_fee_of_size(dissolution.signed(vec![[0; 65]]).to_bytes().len(), fee)
}

/// `fee` is at least the relay minimum for a transaction of `size` bytes:
/// below it no node relays it (`MIN_FEE_PER_KB`).
pub(super) fn check_relay_fee_of_size(size: usize, fee: u64) -> Result<(), MasternodesError> {
    let floor = relay_floor(size)?;
    if fee < floor {
        return Err(invalid(&format!(
            "the fee is below the relay minimum of {floor} duffs"
        )));
    }
    Ok(())
}

/// The relay minimum for a transaction of `size` bytes (`MIN_FEE_PER_KB`).
pub(super) fn relay_floor(size: usize) -> Result<u64, MasternodesError> {
    Ok(fee_for(FeeRate::new(MIN_FEE_PER_KB), size)
        .ok_or(EngineError::Send(crate::send::SendFailure::TxTooLarge))?)
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
        let (list_height, listed) = listed_above(ctx, hash, height)?;
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
        let penalty = if early { early_penalty(&mn) } else { 0 };
        let dissolution = build_dissolution(&mn, request.share_index, penalty, request.fee)
            .map_err(build_refusal)?;
        check_relay_fee(&dissolution, request.fee)?;
        let actor_output = dissolution.actor_output;
        let warnings = actor_warnings(actor_output, &actor.refund_script);
        let returns = share_returns(
            &p.shares,
            &dissolution.tx.output,
            request.share_index,
            actor_output,
            |i| i == request.share_index,
            network,
        );
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
            payout_address: address_of(&actor.refund_script, network).unwrap_or_default(),
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
        self.check_no_pending_maintenance(wallet, hash, None)
            .await?;
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
            let record = DissolveRecord {
                request: request.clone(),
                penalty: plan.penalty,
                actor_output: plan.actor_output,
                foreign: false,
                together: false,
            };
            let handed = HandOff {
                id: &id,
                signed: plan.dissolution.signed(vec![sig]),
                record,
                terms: &plan.mn_terms,
                hash,
                charge,
                fence,
            };
            self.hand_off_dissolution(wallet, handed, updating).await
        }
        .await;
        lease.end();
        result
    }

    /// Saves a signed dissolution as a `dissolve` operation, then hands it
    /// off as MG-04's rows are; D23's lock is released once the row is
    /// saved.
    pub(super) async fn hand_off_dissolution(
        self: &Arc<Self>,
        wallet: WalletId,
        h: HandOff<'_>,
        updating: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<ProTxOperation, MasternodesError> {
        let tx = super::send::typed(&h.signed)?;
        let txid = tx.txid();
        let now = now_secs();
        let fee = h.record.request.fee;
        let row = MasternodeOpRow {
            id: h.id.to_string(),
            kind: kind_name(ProTxOperationKind::Dissolve).into(),
            terms: json(h.terms)?,
            request: json(&h.record)?,
            unsigned_tx: None,
            sign_address: None,
            sign_message: None,
            // The collateral isn't the wallet's coin: no fee inputs, no
            // locks (§6.3).
            fee_inputs: ops::encode_inputs(&[]),
            signed_tx: Some(h.signed.to_bytes()),
            txid: Some(txid.to_string()),
            pro_tx_hash: Some(Txid::from_byte_array(h.hash).to_string()),
            locked_collateral: false,
            fee,
            created_at: now,
            signed_at: Some(now),
            sent_at: None,
            last_error: None,
        };
        self.save_operation(wallet, row, Vec::new()).await?;
        drop(updating);
        let w = self.wallet(&wallet).await?;
        let summary = self.summary_of(&w, &tx, &[], fee, None).await;
        self.deliver(
            wallet,
            Delivery {
                id: h.id.to_string(),
                tx,
                summary,
                charge: h.charge,
                fence: h.fence,
                maybe_out: false,
            },
        )
        .await?;
        self.operation(wallet, h.id).await
    }

    /// D81's two readings for `list`, taken after it: a height its entries
    /// aren't above, and the wallet's filter-scan checkpoint. The pinned SPV
    /// library gives the entries without their height, and its engine takes
    /// a diff before its progress says so: bound the entries' height by the
    /// readings taken after them, the headers' included. The engine asks for
    /// no diff above a stored header, and the header reading is published
    /// before that request's round trip ends; it isn't certain to be (D81
    /// names the residual).
    pub(super) fn list_scan(&self, wallet: WalletId, list: &ListView) -> (u32, u32) {
        let list_bound = {
            let t = self.hub.tracker();
            [list.height, t.masternode_list_height(), t.tip_height()]
                .into_iter()
                .flatten()
                .max()
                .unwrap_or(0)
        };
        let scanned = self
            .hub
            .wallet_state(&wallet)
            .map_or(0, |s| s.synced_height);
        (list_bound, scanned)
    }

    /// What [`list_settled`] reads for `rows`: nothing unless one is an
    /// [`unseen_dissolution`].
    pub(crate) async fn dissolution_view(
        &self,
        wallet: WalletId,
        rows: &[MasternodeOpRow],
    ) -> DissolutionView {
        let hashes: Vec<[u8; 32]> = rows
            .iter()
            .filter(|r| unseen_dissolution(r))
            .filter_map(row_hash)
            .collect();
        if hashes.is_empty() {
            return DissolutionView::default();
        }
        let Ok(manager) = self.manager() else {
            return DissolutionView::default();
        };
        let list = self.protx_list(&manager).await;
        let (list_bound, scanned) = self.list_scan(wallet, &list);
        let chain = self.shared_chain(wallet).await.ok();
        let mut registered = HashMap::new();
        for hash in hashes {
            let txid = Txid::from_byte_array(hash);
            let at = match self.shared_confirmed(wallet, txid).await {
                Ok((_, at)) => Some(at),
                // A standby sent without the session (D86, D87): the
                // registration's block in the wallet's history.
                Err(_) => chain
                    .as_ref()
                    .and_then(|c| c.history.get(&txid))
                    .and_then(|e| e.context.block_info())
                    .map(|b| b.height()),
            };
            if let Some(at) = at {
                registered.insert(hash, at);
            }
        }
        DissolutionView {
            list,
            list_bound,
            scanned,
            registered,
        }
    }

    /// The masternode's registration from the wallet's history (D29), in
    /// the bytes the vault's dissolution signer reads.
    pub(super) async fn registration_bytes(
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

pub(super) const DUST_REFUSAL: &str =
    "the actor's output is below the dust threshold and would not be relayed";

/// n-10: an actor output in (0, dust) is valid and Core's builder keeps
/// it, but it isn't relayed (`providertx_service.cpp:674-676`).
pub(super) fn actor_warnings(actor_output: u64, refund: &ScriptBuf) -> Vec<SharedWarning> {
    match actor_output > 0 && actor_output < dust_threshold(refund.len()) {
        true => vec![SharedWarning::ActorOutputDust],
        false => Vec::new(),
    }
}

/// m-16: the list's height and its entry of `hash`, the list above the
/// registration's block `height`; else `shared_too_early`.
pub(super) fn listed_above(
    ctx: &Context,
    hash: [u8; 32],
    height: u32,
) -> Result<(u32, &MasternodeListSummary), MasternodesError> {
    let listed = ctx.entries.iter().find(|e| e.pro_tx_hash == hash);
    match (ctx.list_height.filter(|h| *h > height), listed) {
        (Some(list_height), Some(listed)) => Ok((list_height, listed)),
        _ => Err(MasternodesError::SharedTooEarly),
    }
}

/// Each share's return from a dissolution by `actor` paying `outputs`:
/// the non-actors in share order, then the actor's `actor_output`,
/// omitted at 0 (`BuildDissolution`).
pub(super) fn share_returns(
    shares: &[CollateralShare],
    outputs: &[TxOut],
    actor: u16,
    actor_output: u64,
    mine: impl Fn(u16) -> bool,
    network: dashcore::Network,
) -> Vec<ShareReturn> {
    let mut paid = outputs.iter();
    (0u16..)
        .zip(shares)
        .map(|(i, s)| ShareReturn {
            share_index: i,
            amount: match i == actor {
                true => actor_output,
                false => paid.next().map_or(0, |o| o.value),
            },
            refund_address: address_of(&s.refund_script, network),
            mine: mine(i),
        })
        .collect()
}

pub(super) fn address_of(script: &ScriptBuf, network: dashcore::Network) -> Option<String> {
    Address::from_script(script, network)
        .ok()
        .map(|a| a.to_string())
}

/// What [`list_settled`] reads (D81).
#[derive(Debug, Clone, Default)]
pub(crate) struct DissolutionView {
    pub list: ListView,
    /// A height the entries' list isn't above (D81).
    pub list_bound: u32,
    /// The wallet's filter-scan checkpoint (`synced_height`): every block
    /// up to it that pays the wallet is in its history. The best processed
    /// block isn't: filter batches commit with gaps.
    pub scanned: u32,
    /// The height of each unseen row's registration, while it is in a
    /// block of the wallet's history.
    pub registered: HashMap<[u8; 32], u32>,
}

/// D27: the wallet sees its dissolution by the collateral leaving the
/// list. One that pays this wallet nothing (its actor output omitted) is
/// never in the history, so a `sent` one reads `confirmed`, at the list's
/// height, once its masternode is out of the synced list (D81). Every
/// other dissolution of it pays this wallet's refund and so is in the
/// history (`conflicted` there), but only once the wallet has scanned to
/// the list's height: below it, another's dissolution may have emptied the
/// list unseen, so the row stays `sent`. So it does while the registration
/// isn't in a block at or below that height (a reorg took it out).
pub(crate) fn list_settled(
    row: &MasternodeOpRow,
    state: ProTxOperationState,
    view: &DissolutionView,
) -> ProTxOperationState {
    if state != ProTxOperationState::Sent || !unseen_dissolution(row) || !view.list.synced {
        return state;
    }
    let Some(hash) = row_hash(row) else {
        return state;
    };
    let gone = view.list.entry(&hash).is_none();
    let registered = view
        .registered
        .get(&hash)
        .is_some_and(|at| *at <= view.list_bound);
    match gone && registered && view.scanned >= view.list_bound {
        true => ProTxOperationState::Confirmed {
            height: view.list_bound,
        },
        false => state,
    }
}

fn row_hash(row: &MasternodeOpRow) -> Option<[u8; 32]> {
    let hash = row.pro_tx_hash.as_deref()?;
    super::super::parse_pro_tx_hash(hash).ok()
}

/// A `dissolve` row whose transaction may never be in the wallet's
/// history: its actor output omitted, or another share's standby
/// (`foreign`, D87), whose actor output pays another wallet.
fn unseen_dissolution(row: &MasternodeOpRow) -> bool {
    kind_of(&row.kind) == ProTxOperationKind::Dissolve
        && serde_json::from_str::<DissolveRecord>(&row.request)
            .is_ok_and(|r| r.actor_output == 0 || r.foreign)
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
            foreign: false,
            together: false,
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

    /// A synced list at 120 holding `entries`, the wallet scanned to
    /// `scanned`, the registration in a block at `registered`.
    fn view(entries: &[[u8; 32]], scanned: u32, registered: Option<u32>) -> DissolutionView {
        DissolutionView {
            list: list(true, entries),
            list_bound: 120,
            scanned,
            registered: registered.map(|at| (HASH, at)).into_iter().collect(),
        }
    }

    #[test]
    fn test_QT_127_an_unseen_sent_dissolution_confirms_when_its_masternode_leaves_the_list() {
        use ProTxOperationState::*;
        let gone = view(&[[8; 32]], 120, Some(50));
        assert_eq!(
            list_settled(&row(0), Sent, &gone),
            Confirmed { height: 120 }
        );
        // Still listed, the list unsynced, the wallet's scan behind the
        // list, the registration out of a block or above the list, or not
        // sent: as derived.
        assert_eq!(
            list_settled(&row(0), Sent, &view(&[HASH], 120, Some(50))),
            Sent
        );
        let unsynced = DissolutionView {
            list: list(false, &[]),
            ..gone.clone()
        };
        assert_eq!(list_settled(&row(0), Sent, &unsynced), Sent);
        assert_eq!(list_settled(&row(0), Sent, &view(&[], 119, Some(50))), Sent);
        assert_eq!(list_settled(&row(0), Sent, &view(&[], 120, None)), Sent);
        assert_eq!(
            list_settled(&row(0), Sent, &view(&[], 120, Some(121))),
            Sent
        );
        for state in [Signed, NotSent, Conflicted, Confirmed { height: 3 }] {
            assert_eq!(list_settled(&row(0), state, &gone), state);
        }
        // One paying this wallet is seen in the history, not the list.
        assert!(!unseen_dissolution(&row(1)));
        assert_eq!(list_settled(&row(1), Sent, &gone), Sent);
        let update = MasternodeOpRow {
            kind: "update_share".into(),
            ..row(0)
        };
        assert_eq!(list_settled(&update, Sent, &gone), Sent);
    }

    #[test]
    fn test_QT_127_a_resent_dissolution_is_charged_its_penalty_and_fee() {
        assert_eq!(row_debit(&row(0)).unwrap(), 1_005);
        // Another share's standby gives up nothing of the wallet's.
        let mut record: DissolveRecord = serde_json::from_str(&row(0).request).unwrap();
        record.foreign = true;
        let foreign = MasternodeOpRow {
            request: serde_json::to_string(&record).unwrap(),
            ..row(0)
        };
        assert_eq!(row_debit(&foreign).unwrap(), 0);
        // Every owner signed: the approvals were charged.
        record.foreign = false;
        record.together = true;
        let together = MasternodeOpRow {
            request: serde_json::to_string(&record).unwrap(),
            ..row(0)
        };
        assert_eq!(row_debit(&together).unwrap(), 0);
        assert!(
            !row(0).request.contains("foreign"),
            "the 2a row's JSON is unchanged"
        );
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
