//! Dissolve together (MG-04b §3.3, §4, slice 2c): the unanimous ProDisTx
//! every owner signs, run as a maintenance session over dash-qt's sigs
//! envelope (`DissolveDialog`'s together tab, rc.3
//! `qt/sharedmndialogs.cpp:1354-1548`). One wallet prepares the request
//! (`PrepareSharedDissolution`, `providertx_service.cpp:1281-1296`) and
//! signs its shares; every other owner imports it, approves and copies back
//! a reply; any wallet holding every signature combines them in share order
//! (`CombineShared`, `:1212-1223`) and sends the result as an MG-04
//! operation of kind `dissolve` (D95).
//!
//! The session is a `shared_mn_sessions` row of kind `dissolve_together`
//! (D93): its envelope is the request and the signatures verified so far,
//! `my_shares` this wallet's shares of the masternode, and `signed_tx`,
//! `txid` and `out_at` are written before the send hands it off. Its stage
//! is derived on every read (D94): `signing` while a signature is missing,
//! `combined` once every share signed, then `sent`, `confirmed` or
//! `conflicted` as the operation it was sent as, and `dead{delisted}` when
//! the masternode left the list without a send from this wallet. The
//! registration is read from the wallet's history (D29), as the standby's.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dashcore::hashes::{Hash, HashEngine, sha256};
use dashcore::{OutPoint, Transaction, Txid};
use dw_appdb::{MasternodeOpRow, SharedSessionRow, SharedSessionUpdate};
use dw_protx::shared::envelope::{
    ReceivedSigs, SigEntry, SigsEnvelope, SigsKind, decode_message, fingerprint,
};
use dw_protx::shared::json::Json;
use dw_protx::shared::registration::ProRegTxV3;
use dw_protx::shared::rules::{self as shared_rules, SharedMn};
use dw_protx::shared::together::{self, Checked, Refusal};
use dw_protx::shared::{CompactSignature, ProDisTx, SpecialTx};

use super::super::follow::ListView;
use super::super::protx::build::{invalid, refused};
use super::super::protx::charge_fee;
use super::super::protx::ops::{self, signed_tx};
use super::super::protx_records::{ProTxOperation, ProTxOperationState};
use super::super::shared_records::{
    DeadReason, DissolveRequest, ShareRow, ShareTermsView, SharedImport, SharedKind, SharedMessage,
    SharedNext, SharedOutcome, SharedRole, SharedSession, SharedStage, SharedWarning,
};
use super::super::{MasternodesError, display_hex};
use super::dissolve::{
    DUST_REFUSAL, DissolveRecord, HandOff, actor_warnings, address_of, build_refusal,
    check_relay_fee_of_size, relay_floor,
};
use super::flow::signer_refusal;
use super::standby::{Registered, in_history, p2pkh_address};
use super::store::{ChainView, json, message_refusal, role_name, role_of, stage_refusal};
use crate::coins::now_secs;
use crate::platform::{BudgetPurpose, FlowKind, PlatformError};
use crate::send::lease_failure;
use crate::{EngineError, NetworkSession, WalletId};

/// The `kind` column of a dissolve-together session.
pub(crate) const KIND: &str = "dissolve_together";

/// The maintenance session id (m-17): SHA-256 of the envelope's `kind`
/// (`dissolve`), the ProTx hash and the sign hash, both in internal (wire)
/// order, as 64 lower-case hex of the digest.
pub(crate) fn session_id(hash: [u8; 32], sign_hash: [u8; 32]) -> String {
    let mut engine = sha256::Hash::engine();
    engine.input(SigsKind::Dissolve.key().as_bytes());
    engine.input(&hash);
    engine.input(&sign_hash);
    hex::encode(sha256::Hash::from_engine(engine).to_byte_array())
}

/// A received request's refusal (D92): `shared_message{reason}`.
fn request_refusal(e: Refusal) -> MasternodesError {
    message_refusal(match e {
        Refusal::NotDissolution => "not_dissolution",
        Refusal::OtherMasternode => "other_masternode",
        Refusal::Signed => "signed",
        Refusal::NotBuilt => "outputs",
    })
}

fn corrupt() -> MasternodesError {
    EngineError::Internal("a stored dissolve-together session does not parse".into()).into()
}

/// The size of the combined transaction: the request with one signature
/// per share, exact because every signature is 65 bytes.
fn combined_size(checked: &Checked, mn: &SharedMn) -> usize {
    let n = usize::from(together::sig_count(mn));
    checked
        .dissolution
        .signed(vec![[0; 65]; n])
        .to_bytes()
        .len()
}

/// The relay minimum for the combined transaction (D96): below it no node
/// would take it, so it is never imported, signed or sent.
fn check_relay(checked: &Checked, mn: &SharedMn) -> Result<(), MasternodesError> {
    check_relay_fee_of_size(combined_size(checked, mn), checked.fee)
}

/// D83: a dust actor output is never relayed, so nothing signs it.
fn check_dust(checked: &Checked, mn: &SharedMn) -> Result<(), MasternodesError> {
    let actor = &mn.shares[usize::from(checked.actor)];
    match actor_warnings(checked.dissolution.actor_output, &actor.refund_script)
        .contains(&SharedWarning::ActorOutputDust)
    {
        true => Err(invalid(DUST_REFUSAL)),
        false => Ok(()),
    }
}

/// This wallet's shares (share, keychain owner key) without a signature in
/// `sigs`.
fn unsigned<'a>(
    mine: &'a BTreeMap<u16, u32>,
    sigs: &'a BTreeMap<u16, CompactSignature>,
) -> impl Iterator<Item = (u16, u32)> + 'a {
    mine.iter()
        .filter(|(share, _)| !sigs.contains_key(share))
        .map(|(s, o)| (*s, *o))
}

/// A stored session, read back.
#[derive(Debug, Clone)]
pub(crate) struct Together {
    pub row: SharedSessionRow,
    pub hash: [u8; 32],
    /// The prepared transaction, without signatures.
    pub request: SpecialTx,
    /// The verified signatures by share.
    pub sigs: BTreeMap<u16, CompactSignature>,
    /// This wallet's shares and their keychain owner keys, as written.
    pub mine: BTreeMap<u16, u32>,
}

impl Together {
    /// Parses `row`. The envelope is the wallet's own rendering, so a
    /// failure is corruption (named without the text).
    pub fn of(row: SharedSessionRow, network: dashcore::Network) -> Result<Self, MasternodesError> {
        let Ok(dw_protx::shared::envelope::SharedMessage::Sigs(r)) =
            decode_message(&row.envelope, network)
        else {
            return Err(corrupt());
        };
        let (Some(request), SigsKind::Dissolve) = (r.tx, r.kind) else {
            return Err(corrupt());
        };
        let sigs = r
            .entries
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .map(|e| match e {
                SigEntry::Signature { index, sig } => {
                    Ok((u16::try_from(index).map_err(|_| corrupt())?, sig))
                }
                _ => Err(corrupt()),
            })
            .collect::<Result<_, _>>()?;
        let mine =
            serde_json::from_str::<BTreeMap<u16, u32>>(&row.my_shares).map_err(|_| corrupt())?;
        Ok(Self {
            hash: r.pro_tx_hash.to_byte_array(),
            row,
            request,
            sigs,
            mine,
        })
    }

    fn envelope(&self, network: dashcore::Network) -> SigsEnvelope {
        envelope(network, self.hash, &self.request, &self.sigs)
    }

    /// The message to copy: the request with every signature held. The
    /// preparer's is the Request, an approver's its Reply (dash-qt's
    /// "Copy Request", "Approve and Copy Reply").
    pub fn message(&self, network: dashcore::Network) -> SharedMessage {
        let json = self.envelope(network).to_json();
        SharedMessage {
            text: String::from_utf8(json.write(2)).expect("the writer emits UTF-8"),
            code: fingerprint(&json),
            name: match role_of(&self.row.role) {
                SharedRole::Coordinator => "Request",
                SharedRole::Participant => "Reply",
            }
            .into(),
            file_name: format!("shared-mn-session-{}.json", self.session_code()),
        }
    }

    /// The id's first six characters, upper-case: the session's code.
    fn session_code(&self) -> String {
        let code: String = self.row.id.chars().take(6).collect();
        code.to_ascii_uppercase()
    }

    /// The combined transaction, once every share signed.
    fn combined(&self, mn: &SharedMn) -> Option<SpecialTx> {
        let checked = together::check(&self.request, mn).ok()?;
        together::combine(&checked, mn, &self.sigs).ok()
    }
}

fn envelope(
    network: dashcore::Network,
    hash: [u8; 32],
    request: &SpecialTx,
    sigs: &BTreeMap<u16, CompactSignature>,
) -> SigsEnvelope {
    SigsEnvelope {
        network,
        kind: SigsKind::Dissolve,
        pro_tx_hash: Txid::from_byte_array(hash),
        tx: request.to_bytes(),
        signatures: sigs.iter().map(|(i, s)| (*i, *s)).collect(),
    }
}

/// What a session's stage is derived from (D94), read once per call.
pub(crate) struct TogetherView {
    pub chain: ChainView,
    pub list: Option<ListView>,
    pub ops: Vec<MasternodeOpRow>,
}

/// The masternode as the history holds its registration at `height` (D29):
/// no list needed.
fn known_mn(hash: [u8; 32], height: u32, p: &ProRegTxV3) -> SharedMn<'_> {
    let txid = Txid::from_byte_array(hash);
    SharedMn {
        pro_tx_hash: txid,
        collateral: OutPoint::new(txid, p.collateral.vout),
        shares: &p.shares,
        registered_height: height,
        early_period_blocks: p.early_period_blocks,
        early_penalty: p.early_penalty,
        voting_key_id: p.voting_key_id,
    }
}

/// The operation the session was handed off as: the one whose signed bytes
/// carry its `txid`.
fn handed_off<'a>(t: &Together, v: &'a TogetherView) -> Option<&'a MasternodeOpRow> {
    let txid = t.row.txid.as_deref()?;
    v.ops
        .iter()
        .find(|r| signed_tx(r).is_some_and(|tx| tx.txid().to_string() == txid))
}

/// Whether `tx` is `request` signed: the same transaction once its
/// ProDisTx's `vchSigs` are emptied. A wallet missing a signature knows the
/// dissolution's bytes only this way.
fn signs_request(tx: &Transaction, request: &SpecialTx) -> bool {
    let Ok(special) = SpecialTx::from_bytes(&dashcore::consensus::serialize(tx)) else {
        return false;
    };
    let Ok(mut p) = dashcore::consensus::deserialize::<ProDisTx>(&special.payload) else {
        return false;
    };
    p.sigs.clear();
    let unsigned = SpecialTx {
        payload: dashcore::consensus::serialize(&p),
        ..special
    };
    unsigned == *request
}

/// The first transaction of the history spending `collateral` in a block,
/// and its height.
fn collateral_spend(chain: &ChainView, collateral: OutPoint) -> Option<(u32, &Transaction)> {
    chain
        .history
        .values()
        .filter(|e| e.tx.input.iter().any(|i| i.previous_output == collateral))
        .filter_map(|e| Some((e.context.block_info()?.height(), &e.tx)))
        .min_by_key(|(height, _)| *height)
}

/// D94: the combined transaction in a block of the history; else the
/// operation the session was sent as (its txid's); else the collateral
/// spent in a block, by the request signed (`confirmed`) or by another
/// transaction, or the masternode out of the synced list (`dead`); then
/// the signatures.
fn derive_stage(
    t: &Together,
    mn: Option<&SharedMn>,
    v: &TogetherView,
    op_state: Option<ProTxOperationState>,
) -> SharedStage {
    let txid = mn.and_then(|mn| t.combined(mn)).map(|tx| tx.txid());
    if let Some(height) = txid.as_ref().and_then(|t| v.chain.confirmed_height(t)) {
        return SharedStage::Confirmed { height };
    }
    if t.row.out_at.is_some() {
        match op_state {
            Some(ProTxOperationState::Confirmed { height }) => {
                return SharedStage::Confirmed { height };
            }
            Some(ProTxOperationState::Conflicted) => return SharedStage::Conflicted,
            Some(_) => return SharedStage::Sent,
            // Marked out, but no operation saved: nothing was handed off.
            None => {}
        }
    }
    let Some(mn) = mn else {
        return SharedStage::Signing;
    };
    let dead = SharedStage::Dead {
        reason: DeadReason::Delisted,
    };
    match collateral_spend(&v.chain, mn.collateral) {
        Some((height, tx)) if signs_request(tx, &t.request) => {
            return SharedStage::Confirmed { height };
        }
        Some(_) => return dead,
        None => {}
    }
    let delisted = v.list.as_ref().is_some_and(|l| {
        l.synced && l.height.is_some_and(|h| h > mn.registered_height) && l.entry(&t.hash).is_none()
    });
    match (delisted, t.sigs.len() == mn.shares.len()) {
        (true, _) => dead,
        (false, true) => SharedStage::Combined,
        (false, false) => SharedStage::Signing,
    }
}

impl NetworkSession {
    /// Whether `id` is a stored dissolve-together session of the wallet.
    pub(crate) async fn is_together(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<bool, MasternodesError> {
        Ok(self
            .shared_row(wallet, id)
            .await?
            .is_some_and(|r| r.kind == KIND))
    }

    /// The wallet's dissolve-together sessions, newest first.
    pub(crate) async fn together_rows(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<SharedSessionRow>, MasternodesError> {
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.shared_sessions(&w)).await?;
        Ok(rows.into_iter().filter(|r| r.kind == KIND).collect())
    }

    /// The stored dissolve-together session `id`, if the wallet has one;
    /// `invalid_argument` for a session of another kind.
    async fn together_stored(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<Option<Together>, MasternodesError> {
        match self.shared_row(wallet, id).await? {
            Some(row) if row.kind == KIND => {
                Ok(Some(Together::of(row, self.network.core_network())?))
            }
            Some(_) => Err(invalid("not a dissolve-together session")),
            None => Ok(None),
        }
    }

    /// A stored dissolve-together session: `shared_session_not_found`, or
    /// `invalid_argument` for a session of another kind.
    pub(crate) async fn together_loaded(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<Together, MasternodesError> {
        self.together_stored(wallet, id)
            .await?
            .ok_or(MasternodesError::SharedSessionNotFound)
    }

    pub(crate) async fn together_read(
        &self,
        wallet: WalletId,
    ) -> Result<TogetherView, MasternodesError> {
        let chain = self.shared_chain(wallet).await?;
        let list = match self.manager() {
            Ok(manager) => Some(self.protx_list(&manager).await),
            Err(_) => None,
        };
        let w = wallet.to_string();
        let ops = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        Ok(TogetherView { chain, list, ops })
    }

    /// The session's record, its stage derived now (D94).
    pub(crate) async fn together_session(
        &self,
        wallet: WalletId,
        t: &Together,
        v: &TogetherView,
    ) -> Result<SharedSession, MasternodesError> {
        let known = in_history(&v.chain, t.hash).ok();
        let mn = known
            .as_ref()
            .map(|(height, p)| known_mn(t.hash, *height, p));
        let op_state = match handed_off(t, v) {
            Some(row) => Some(self.op_state(wallet, row).await?),
            None => None,
        };
        let stage = derive_stage(t, mn.as_ref(), v, op_state);
        Ok(view(t, mn.as_ref(), v, stage, self.network.core_network()))
    }

    /// Whether `row`, a dissolve-together session, may be discarded (D17):
    /// `Ok` with the `txid` it is marked out with, `None` when it isn't.
    /// Out and handed off (an operation holds its txid), its stage
    /// (`shared_stage`); out and never handed off, it may go: the send saves
    /// the operation before the bytes leave (D95). A row that no longer
    /// parses goes unless out (D57).
    pub(crate) async fn together_discard(
        &self,
        wallet: WalletId,
        row: SharedSessionRow,
    ) -> Result<Option<String>, MasternodesError> {
        let out = row.out_at.is_some();
        let Ok(t) = Together::of(row, self.network.core_network()) else {
            return match out {
                true => Err(stage_refusal(SharedStage::Sent)),
                false => Ok(None),
            };
        };
        let v = self.together_read(wallet).await?;
        if !out {
            return Ok(None);
        }
        if handed_off(&t, &v).is_some() {
            let stage = self.together_session(wallet, &t, &v).await?.stage;
            return Err(stage_refusal(stage));
        }
        match t.row.txid.clone() {
            Some(txid) => Ok(Some(txid)),
            None => Err(stage_refusal(SharedStage::Sent)),
        }
    }

    /// The session's stage, derived now (D94).
    pub(crate) async fn together_stage(
        &self,
        wallet: WalletId,
        t: &Together,
    ) -> Result<SharedStage, MasternodesError> {
        let v = self.together_read(wallet).await?;
        Ok(self.together_session(wallet, t, &v).await?.stage)
    }

    /// The record of `id` with its message.
    pub(crate) async fn together_outcome(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let t = self.together_loaded(wallet, id).await?;
        let v = self.together_read(wallet).await?;
        Ok(SharedOutcome {
            session: self.together_session(wallet, &t, &v).await?,
            message: t.message(self.network.core_network()),
        })
    }

    /// Writes a new session, or the signatures and shares of a stored one.
    #[expect(clippy::too_many_arguments, reason = "one row's columns")]
    async fn write_together(
        &self,
        wallet: WalletId,
        id: &str,
        hash: [u8; 32],
        request: &SpecialTx,
        sigs: &BTreeMap<u16, CompactSignature>,
        mine: &BTreeMap<u16, u32>,
        stored: Option<&SharedSessionRow>,
        role: SharedRole,
    ) -> Result<(), MasternodesError> {
        let network = self.network.core_network();
        let envelope = envelope(network, hash, request, sigs).to_text();
        let my_shares = json(mine)?;
        let now = now_secs();
        let w = wallet.to_string();
        match stored {
            Some(row) => {
                let update = SharedSessionUpdate {
                    envelope,
                    my_shares,
                    contribution: None,
                    approved_terms: None,
                    signed_tx: None,
                    txid: None,
                    last_error: row.last_error.clone(),
                    updated_at: now,
                };
                let id = id.to_string();
                let saved = self
                    .appdb_op(move |db| db.update_shared_session_durable(&w, &id, &update))
                    .await?;
                if !saved {
                    return Err(MasternodesError::SharedSessionNotFound);
                }
            }
            None => {
                let row = SharedSessionRow {
                    id: id.to_string(),
                    kind: KIND.into(),
                    role: role_name(role).into(),
                    envelope,
                    my_shares,
                    contribution: None,
                    reserved: "[]".into(),
                    approved_terms: None,
                    base_keys: None,
                    signed_tx: None,
                    txid: None,
                    out_at: None,
                    abandon_txid: None,
                    created_at: now,
                    updated_at: now,
                    last_error: None,
                };
                self.appdb_op(move |db| db.insert_shared_session_durable(&w, &row, now))
                    .await?;
            }
        }
        self.masternodes.local_change();
        Ok(())
    }

    /// This wallet's shares of `reg` (D10), as a map.
    async fn together_mine(
        &self,
        wallet: WalletId,
        reg: &Registered,
    ) -> Result<BTreeMap<u16, u32>, MasternodesError> {
        Ok(self.my_shares(wallet, reg).await?.into_iter().collect())
    }

    /// Signs `checked` with each of `to_sign` (share, keychain owner key)
    /// under one `Dissolution` lease charged `debit` (§6.3), each signature
    /// checked to be its share's, then writes them into the session; the
    /// charge goes back when nothing is written.
    #[expect(clippy::too_many_arguments, reason = "one signing step")]
    async fn sign_together(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        debit: u64,
        reg: &Registered,
        checked: &Checked,
        to_sign: &[(u16, u32)],
        mut sigs: BTreeMap<u16, CompactSignature>,
        mine: &BTreeMap<u16, u32>,
        stored: Option<&SharedSessionRow>,
        role: SharedRole,
    ) -> Result<(), MasternodesError> {
        let mn = reg.mn();
        let n = together::sig_count(&mn);
        let id = session_id(reg.hash, checked.sign_hash);
        let registration = self.registration_bytes(wallet, reg.hash).await?;
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let result = async {
            let (charge, signer) = lease
                .charge_tx_signer(BudgetPurpose::Masternode, debit)
                .map_err(|e| lease_failure(e, debit))?;
            let signed = (|| {
                let request = checked.dissolution.tx.to_bytes();
                for (share, owner_index) in to_sign {
                    let sig = signer
                        .sign_dissolution(*owner_index, &request, &registration, n)
                        .map_err(signer_refusal)?;
                    // The keychain key must be the share's (D11).
                    if !together::verifies(&mn, checked.sign_hash, *share, &sig) {
                        return Err(MasternodesError::ShareNotMine);
                    }
                    sigs.insert(*share, sig);
                }
                Ok(())
            })();
            let written = match signed {
                Ok(()) => {
                    self.write_together(
                        wallet,
                        &id,
                        reg.hash,
                        &checked.dissolution.tx,
                        &sigs,
                        mine,
                        stored,
                        role,
                    )
                    .await
                }
                Err(e) => Err(e),
            };
            if written.is_err() {
                lease.refund(BudgetPurpose::Masternode, charge);
            }
            written
        }
        .await;
        lease.end();
        result
    }

    /// Prepares the request as `protx shared_dissolve_prepare` does, the
    /// actor one of this wallet's shares as dash-qt's preparer picks it
    /// (`:1373-1375`), and signs every share of this wallet under a lease
    /// charged the fee its actor share pays (§6.3). Starting the same
    /// request again signs what is missing and returns the session.
    pub(super) async fn start_together(
        self: &Arc<Self>,
        wallet: WalletId,
        request: DissolveRequest,
        hash: [u8; 32],
        grant: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        self.check_no_pending_maintenance(wallet, hash, None)
            .await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let mine = self.together_mine(wallet, &reg).await?;
        if !mine.contains_key(&request.share_index) {
            return Err(MasternodesError::ShareNotMine);
        }
        let prepared =
            together::prepare(&mn, request.share_index, request.fee).map_err(build_refusal)?;
        let checked = together::check(&prepared.tx, &mn)
            .map_err(|_| EngineError::Internal("a prepared request reads as another".into()))?;
        check_relay(&checked, &mn)?;
        check_dust(&checked, &mn)?;
        // Core's rules on the combined shape, signatures not checked.
        let n = usize::from(together::sig_count(&mn));
        let shape = checked.dissolution.signed(vec![[0; 65]; n]);
        let p = ProDisTx {
            sigs: vec![[0; 65]; n],
            ..checked.dissolution.payload.clone()
        };
        shared_rules::check_dissolution(&shape, &p, Some(&mn), reg.spend_height, true, false)
            .map_err(refused)?;
        let id = session_id(hash, checked.sign_hash);
        let _flight = self.claim_shared_session(wallet, &id)?;
        let stored = self.together_stored(wallet, &id).await?;
        let sigs = stored.as_ref().map(|t| t.sigs.clone()).unwrap_or_default();
        if let Some(t) = &stored {
            let stage = self.together_stage(wallet, t).await?;
            if !matches!(stage, SharedStage::Signing | SharedStage::Combined) {
                return Err(stage_refusal(stage));
            }
        }
        let to_sign: Vec<(u16, u32)> = unsigned(&mine, &sigs).collect();
        if !to_sign.is_empty() {
            // The fee leaves the actor share: charged with its signature.
            let actor_signing = to_sign.iter().any(|(share, _)| *share == checked.actor);
            self.sign_together(
                wallet,
                grant,
                if actor_signing { checked.fee } else { 0 },
                &reg,
                &checked,
                &to_sign,
                sigs,
                &mine,
                stored.as_ref().map(|t| &t.row),
                SharedRole::Coordinator,
            )
            .await?;
        }
        self.together_outcome(wallet, &id).await
    }

    /// A sigs envelope or a targeted bare array (§2.5): the request checked
    /// against the registration in the wallet's history and the list (D92,
    /// D29, m-16) and the relay minimum (D96); each signature verified and
    /// taken, or skipped with dash-qt's reason (`addSignatures`,
    /// `sharedmndialogs.cpp:577-613`); the session created (this wallet a
    /// participant) or updated. Signs nothing.
    pub(super) async fn import_together(
        self: &Arc<Self>,
        wallet: WalletId,
        received: Option<ReceivedSigs>,
        bare: Option<(Vec<SigEntry>, String)>,
        target: Option<String>,
    ) -> Result<SharedImport, MasternodesError> {
        if received
            .as_ref()
            .is_some_and(|r| r.kind == SigsKind::Registrar)
        {
            // The key change is a later slice (D45, D97).
            return Err(super::unsupported(None));
        }
        let targeted = match &target {
            Some(id) => Some(self.together_loaded(wallet, id).await?),
            None => None,
        };
        let (hash, tx, entries, code, (edited, no_approvals)) = match (received, bare) {
            (Some(r), _) => {
                // A `signatures` value that isn't an array: dash-qt takes
                // the rest and says so (`sharedmndialogs.cpp:569-572`).
                let no_approvals = matches!(r.entries, Some(None));
                let entries = r.entries.flatten().unwrap_or_default();
                (
                    r.pro_tx_hash.to_byte_array(),
                    r.tx,
                    entries,
                    r.received.code,
                    (r.received.edited, no_approvals),
                )
            }
            (None, Some((entries, code))) => {
                let Some(t) = &targeted else {
                    return Err(message_refusal("parse"));
                };
                (t.hash, None, entries, code, (false, false))
            }
            (None, None) => return Err(message_refusal("parse")),
        };
        // The request: the message's, else the target's (dash-qt adopts a
        // `tx` only when it holds none or the same one, `:540-555`).
        let request = match (tx, &targeted) {
            (Some(tx), Some(t)) if tx != t.request => return Err(message_refusal("other_session")),
            (_, Some(t)) if t.hash != hash => return Err(message_refusal("other_session")),
            (Some(tx), _) => tx,
            (None, Some(t)) => t.request.clone(),
            (None, None) => return Err(message_refusal("parse")),
        };
        let ctx = super::v24_context(self, wallet).await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let checked = together::check(&request, &mn).map_err(request_refusal)?;
        check_relay(&checked, &mn)?;
        let id = session_id(hash, checked.sign_hash);
        if targeted.as_ref().is_some_and(|t| t.row.id != id) {
            return Err(message_refusal("other_session"));
        }
        let _flight = self.claim_shared_session(wallet, &id)?;
        let stored = self.together_stored(wallet, &id).await?;
        let mut sigs = stored.as_ref().map(|t| t.sigs.clone()).unwrap_or_default();
        let (taken, skipped) = absorb(&mn, checked.sign_hash, &mut sigs, &entries);
        let mine = self.together_mine(wallet, &reg).await?;
        let changed = match &stored {
            None => true,
            Some(t) => taken > 0 || t.mine != mine,
        };
        if changed {
            self.write_together(
                wallet,
                &id,
                hash,
                &request,
                &sigs,
                &mine,
                stored.as_ref().map(|t| &t.row),
                SharedRole::Participant,
            )
            .await?;
        }
        let mut warnings = Vec::new();
        if edited {
            warnings.push(SharedWarning::EditedMessage);
        }
        if no_approvals {
            warnings.push(SharedWarning::NoApprovals);
        }
        if let Some(w) = skipped {
            warnings.push(w);
        }
        let outcome = self.together_outcome(wallet, &id).await?;
        Ok(SharedImport::Session {
            session: Box::new(outcome.session),
            changed,
            received_code: code,
            warnings,
        })
    }

    /// Signs this wallet's shares of an imported request (§4's unanimous
    /// row) under a lease charged the fee when its actor share pays it
    /// (§6.3). A request this wallet didn't prepare whose actor share is
    /// this wallet's needs `accept_actor_fee` unless its fee is at most the
    /// relay minimum (m-15).
    pub(super) async fn approve_together(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        grant: &str,
        accept_actor_fee: bool,
    ) -> Result<SharedOutcome, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let _flight = self.claim_shared_session(wallet, id)?;
        let t = self.together_loaded(wallet, id).await?;
        // The stage first: a delisted masternode's session reads `dead`.
        let stage = self.together_stage(wallet, &t).await?;
        if stage != SharedStage::Signing {
            return Err(stage_refusal(stage));
        }
        // D23: a signature that can't be mined while another dissolution
        // or reward change of the masternode may be is never charged.
        self.check_no_pending_maintenance(wallet, t.hash, None)
            .await?;
        let reg = self.registered(wallet, t.hash, &ctx).await?;
        let mn = reg.mn();
        let checked = together::check(&t.request, &mn).map_err(request_refusal)?;
        let mine = self.together_mine(wallet, &reg).await?;
        if mine.is_empty() {
            return Err(MasternodesError::ShareNotMine);
        }
        let to_sign: Vec<(u16, u32)> = unsigned(&mine, &t.sigs).collect();
        if to_sign.is_empty() {
            return self.together_outcome(wallet, id).await;
        }
        check_relay(&checked, &mn)?;
        check_dust(&checked, &mn)?;
        // The fee leaves the actor share: charged with its signature (§6.3).
        let actor_signing = to_sign.iter().any(|(share, _)| *share == checked.actor);
        let prepared_here = role_of(&t.row.role) == SharedRole::Coordinator;
        if actor_signing
            && !prepared_here
            && !accept_actor_fee
            && checked.fee > relay_floor(combined_size(&checked, &mn))?
        {
            return Err(MasternodesError::SharedActorFee { fee: checked.fee });
        }
        let debit = if actor_signing { checked.fee } else { 0 };
        self.sign_together(
            wallet,
            grant,
            debit,
            &reg,
            &checked,
            &to_sign,
            t.sigs.clone(),
            &mine,
            Some(&t.row),
            role_of(&t.row.role),
        )
        .await?;
        self.together_outcome(wallet, id).await
    }

    /// Combines every signature in share order (`InsertUnanimousSignatures`)
    /// and hands the dissolution off as a `dissolve` operation (D95): D23
    /// both ways, Core's rules with the signatures, the relay minimum and
    /// D83; the session marked out first. Charged nothing: the approvals
    /// were (§6.3).
    pub(super) async fn send_together(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        grant: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let updating = self.masternodes.update_lock().await;
        let fence = self.coin_fence(wallet).await?;
        let _flight = self.claim_shared_session(wallet, id)?;
        let t = self.together_loaded(wallet, id).await?;
        // The stage first: a delisted masternode's session reads `dead`.
        let stage = self.together_stage(wallet, &t).await?;
        if stage != SharedStage::Combined {
            return Err(stage_refusal(stage));
        }
        self.check_no_pending_maintenance(wallet, t.hash, None)
            .await?;
        let reg = self.registered(wallet, t.hash, &ctx).await?;
        let mn = reg.mn();
        let checked = together::check(&t.request, &mn).map_err(request_refusal)?;
        let combined = together::combine(&checked, &mn, &t.sigs)
            .map_err(|e| EngineError::Internal(format!("combining a complete session: {e}")))?;
        let p = ProDisTx {
            sigs: (0..mn.shares.len() as u16).map(|i| t.sigs[&i]).collect(),
            ..checked.dissolution.payload.clone()
        };
        shared_rules::check_dissolution(&combined, &p, Some(&mn), reg.spend_height, true, true)
            .map_err(refused)?;
        check_relay(&checked, &mn)?;
        check_dust(&checked, &mn)?;
        let mine = self.together_mine(wallet, &reg).await?;
        let owner_index = mine.get(&checked.actor).copied();
        let record = DissolveRecord {
            request: DissolveRequest {
                pro_tx_hash: display_hex(&t.hash),
                share_index: checked.actor,
                fee: checked.fee,
            },
            penalty: 0,
            actor_output: checked.dissolution.actor_output,
            foreign: owner_index.is_none(),
            together: true,
        };
        let terms = reg.terms(checked.actor, owner_index, self.network.core_network());
        let (w, sid, bytes, txid) = (
            wallet.to_string(),
            id.to_string(),
            combined.to_bytes(),
            combined.txid().to_string(),
        );
        let lease = self
            .begin_lease(wallet, FlowKind::Dissolution, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?;
        let op = ops::new_id();
        let result = async {
            let _flight = self.claim_operation(wallet, &op)?;
            let (charge, _) = charge_fee(&lease, record.debit())?;
            // Out before the bytes can leave (D17's rule for sessions); the
            // same bytes again on a retry.
            let out = self
                .appdb_op(move |db| {
                    db.mark_shared_session_out_durable(&w, &sid, &bytes, &txid, now_secs())
                })
                .await?;
            if !out {
                return Err(stage_refusal(SharedStage::Sent));
            }
            let handed = HandOff {
                id: &op,
                signed: combined,
                record,
                terms: &terms,
                hash: t.hash,
                charge,
                fence,
            };
            self.hand_off_dissolution(wallet, handed, updating).await
        }
        .await;
        lease.end();
        result
    }
}

/// `addSignatures` (`sharedmndialogs.cpp:577-613`): each entry with a share
/// number in range and a 65-byte signature that verifies over `sign_hash`
/// by its share's owner key is taken; a byte-identical repeat is ignored; a
/// second, different signature of a share is skipped, the first kept. The
/// count taken, and the skipped entries' warning.
fn absorb(
    mn: &SharedMn,
    sign_hash: [u8; 32],
    sigs: &mut BTreeMap<u16, CompactSignature>,
    entries: &[SigEntry],
) -> (u32, Option<SharedWarning>) {
    let mut taken = 0;
    let mut skipped = 0u32;
    let mut shares = Vec::new();
    for entry in entries {
        let in_range = entry
            .index()
            .and_then(|i| u16::try_from(i).ok())
            .filter(|i| usize::from(*i) < mn.shares.len());
        let (Some(index), SigEntry::Signature { sig, .. }) = (in_range, entry) else {
            skipped += 1;
            shares.extend(in_range);
            continue;
        };
        if !together::verifies(mn, sign_hash, index, sig) {
            skipped += 1;
            shares.push(index);
            continue;
        }
        match sigs.get(&index) {
            Some(held) if held == sig => {}
            Some(_) => {
                skipped += 1;
                shares.push(index);
            }
            None => {
                sigs.insert(index, *sig);
                taken += 1;
            }
        }
    }
    let warning = (skipped > 0).then(|| {
        let shares: Vec<u16> = shares
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        SharedWarning::SkippedSignatures {
            shares,
            count: skipped,
        }
    });
    (taken, warning)
}

/// The fingerprint a bare array's sender reads out: its own JSON's.
pub(super) fn bare_code(text: &str) -> String {
    Json::read(text.trim().as_bytes())
        .map(|json| fingerprint(&json))
        .unwrap_or_default()
}

fn view(
    t: &Together,
    mn: Option<&SharedMn>,
    v: &TogetherView,
    stage: SharedStage,
    network: dashcore::Network,
) -> SharedSession {
    let checked = mn.and_then(|mn| together::check(&t.request, mn).ok());
    let fee = checked.as_ref().map(|c| c.fee);
    let actor_mine = checked
        .as_ref()
        .is_some_and(|c| t.mine.contains_key(&c.actor));
    let rows = mn
        .map(|mn| {
            (0u16..)
                .zip(mn.shares)
                .map(|(i, s)| ShareRow {
                    index: i,
                    label: format!("Share {}", i + 1),
                    amount: s.amount,
                    owner_address: Some(p2pkh_address(s.owner_key_id, network)),
                    refund_address: address_of(&s.refund_script, network),
                    reward_address: (!s.reward_script.is_empty())
                        .then(|| address_of(&s.reward_script, network))
                        .flatten(),
                    mine: t.mine.contains_key(&i),
                    contributed: false,
                    approved: t.sigs.contains_key(&i),
                    funding_signed: false,
                })
                .collect()
        })
        .unwrap_or_default();
    let listed = v.list.as_ref().and_then(|l| l.entry(&t.hash));
    let terms = ShareTermsView {
        service: listed
            .and_then(|e| e.service_address)
            .map(|a| a.to_string())
            .unwrap_or_default(),
        service_read: None,
        operator_public_key: mn
            .map(|_| hex::encode(listed.map_or([0; 48], |e| e.operator_public_key)))
            .unwrap_or_default(),
        voting_address: mn
            .map(|mn| {
                p2pkh_address(
                    listed.map_or(mn.voting_key_id, |e| e.voting_key_id),
                    network,
                )
            })
            .unwrap_or_default(),
        operator_reward: 0,
        early_period_blocks: mn.map_or(0, |mn| mn.early_period_blocks),
        early_penalty: mn.map_or(0, |mn| mn.early_penalty),
    };
    let own_unsigned = unsigned(&t.mine, &t.sigs).next().is_some();
    let next = match stage {
        SharedStage::Signing if own_unsigned => SharedNext::Approve,
        SharedStage::Signing => SharedNext::Copy,
        SharedStage::Combined => SharedNext::Send,
        SharedStage::Sent => SharedNext::Wait,
        _ => SharedNext::Nothing,
    };
    let mut warnings = Vec::new();
    if let (Some(c), Some(mn)) = (&checked, mn) {
        let refund = &mn.shares[usize::from(c.actor)].refund_script;
        warnings.extend(actor_warnings(c.dissolution.actor_output, refund));
    }
    let complete = mn
        .and_then(|mn| t.combined(mn))
        .map(|tx| tx.txid().to_string());
    SharedSession {
        id: t.row.id.clone(),
        code: t.session_code(),
        message_code: fingerprint(&t.envelope(network).to_json()),
        kind: SharedKind::DissolveTogether,
        role: role_of(&t.row.role),
        stage,
        revision: 0,
        consent_hash: None,
        rows,
        terms,
        node_run_by: String::new(),
        fee,
        debit: fee.map(|f| if actor_mine { f } else { 0 }),
        pro_tx_hash: Some(display_hex(&t.hash)),
        txid: t.row.txid.clone().or(complete),
        reserved: Vec::new(),
        next,
        warnings,
        approved_terms: None,
        base_keys: None,
        actor_fee: fee.filter(|_| actor_mine),
        created_at: t.row.created_at,
        updated_at: t.row.updated_at,
        last_error: t.row.last_error.clone(),
    }
}
