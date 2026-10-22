//! A `shared_mn_sessions` row as the engine reads it (MG-04b §9): the
//! envelope parsed back into dw-protx's `Session`, this wallet's recorded
//! contribution (§4, m-1), and the view whose stage is derived on every read
//! from the row, the wallet's history and its coins (§9.2, D27).

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use dashcore::{OutPoint, Txid};
use dw_appdb::SharedSessionRow;
use dw_protx::shared::SpecialTx;
use dw_protx::shared::envelope::EnvelopeError;
use dw_protx::shared::json::Json;
use dw_protx::shared::session::consent::{FundingMismatch, OwnSide};
use dw_protx::shared::session::funding::{decode_frozen, verify_consent_signature};
use dw_protx::shared::session::merge::Refusal;
use dw_protx::shared::session::{Change, Contribution, Input, Session, Share, Stage};
use serde::{Deserialize, Serialize};

use super::super::MasternodesError;
use super::super::shared_records::{
    DeadReason, KeyBase, ShareRow, ShareTermsView, SharedKind, SharedNext, SharedRole,
    SharedSession, SharedStage, SharedWarning,
};
use crate::EngineError;
use crate::history::TxEntry;

/// `nSequence` of every input the wallet contributes (D16).
pub(crate) const FINAL_SEQUENCE: u32 = u32::MAX;

/// This wallet's side of a session as it recorded it at `start_shared` or
/// `join_shared` (§4, m-1): its row, its reserved inputs and their total
/// `I`, its change `C`, and the coordinator's fee. The base of every later
/// consent and funding check; never re-selected (D17) except by the
/// coordinator's draft-only `set_shared_fee` (D33). The `contribution`
/// column's JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recorded {
    pub share_index: usize,
    /// The keychain owner key index 0–19 of the share (D10).
    pub owner_index: u32,
    pub label: String,
    pub amount: u64,
    pub owner_address: String,
    pub refund_address: String,
    /// `""`: rewards go to the refund address.
    pub reward_address: String,
    /// `"txid:vout"`, in the contribution's order.
    pub inputs: Vec<String>,
    pub input_total: u64,
    pub change: Option<RecordedChange>,
    /// The registration fee it pays: the coordinator's (D9).
    pub fee: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecordedChange {
    pub address: String,
    pub amount: u64,
}

impl Recorded {
    pub fn outpoints(&self) -> Vec<OutPoint> {
        decode_outpoints(&self.inputs)
    }

    pub fn row(&self) -> Share {
        Share {
            label: self.label.clone(),
            amount: self.amount as i64,
            owner_address: self.owner_address.clone(),
            refund_address: self.refund_address.clone(),
            reward_address: self.reward_address.clone(),
        }
    }

    /// The envelope's contribution: the inputs as `Input`s (display txid,
    /// final sequence), the change.
    pub fn contribution(&self) -> Contribution {
        Contribution {
            label: self.label.clone(),
            inputs: self
                .outpoints()
                .iter()
                .map(|o| Input {
                    txid: o.txid.to_string(),
                    vout: o.vout,
                    sequence: FINAL_SEQUENCE,
                })
                .collect(),
            change: self.change.as_ref().map(|c| Change {
                address: c.address.clone(),
                amount: c.amount as i64,
            }),
            change_index: -1,
        }
    }

    pub fn own_side(&self) -> OwnSide {
        OwnSide {
            share_index: self.share_index,
            row: self.row(),
            contribution: self.contribution(),
            input_total: self.input_total,
            fee: self.fee,
        }
    }

    /// `I − C`.
    pub fn debit(&self) -> u64 {
        self.input_total
            .saturating_sub(self.change.as_ref().map_or(0, |c| c.amount))
    }
}

/// The `approved_terms` column: what this wallet last approved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Approved {
    pub terms: ShareTermsView,
    pub consent_hash: String,
}

/// `"txid:vout"` strings, as `reserved` and [`Recorded::inputs`] hold them.
pub(crate) fn encode_outpoints(outpoints: &[OutPoint]) -> Vec<String> {
    outpoints
        .iter()
        .map(|o| format!("{}:{}", o.txid, o.vout))
        .collect()
}

pub(crate) fn decode_outpoints(items: &[String]) -> Vec<OutPoint> {
    items
        .iter()
        .filter_map(|s| {
            let (txid, vout) = s.rsplit_once(':')?;
            Some(OutPoint {
                txid: Txid::from_str(txid).ok()?,
                vout: vout.parse().ok()?,
            })
        })
        .collect()
}

pub(crate) fn reserved_of(row: &SharedSessionRow) -> Vec<OutPoint> {
    decode_outpoints(&serde_json::from_str::<Vec<String>>(&row.reserved).unwrap_or_default())
}

pub(crate) fn json<T: Serialize>(value: &T) -> Result<String, MasternodesError> {
    serde_json::to_string(value)
        .map_err(|e| EngineError::Internal(format!("encoding a shared session: {e}")).into())
}

pub(crate) fn role_name(role: SharedRole) -> &'static str {
    match role {
        SharedRole::Coordinator => "coordinator",
        SharedRole::Participant => "participant",
    }
}

/// The snake-case name `SharedStage{stage}` carries.
pub(crate) fn stage_name(stage: SharedStage) -> &'static str {
    match stage {
        SharedStage::Invited => "invited",
        SharedStage::Draft => "draft",
        SharedStage::Signing => "signing",
        SharedStage::Combined => "combined",
        SharedStage::FundingSigned => "funding_signed",
        SharedStage::Sent => "sent",
        SharedStage::Confirmed { .. } => "confirmed",
        SharedStage::Conflicted => "conflicted",
        SharedStage::Stale => "stale",
        SharedStage::Dead { .. } => "dead",
    }
}

pub(crate) fn stage_refusal(stage: SharedStage) -> MasternodesError {
    MasternodesError::SharedStage {
        stage: stage_name(stage).into(),
    }
}

pub(crate) fn message_refusal(reason: &str) -> MasternodesError {
    MasternodesError::SharedMessage {
        reason: reason.into(),
        share: None,
    }
}

pub(crate) fn funding_refusal(reason: &str) -> MasternodesError {
    MasternodesError::SharedFundingMismatch {
        reason: reason.into(),
        needed: None,
    }
}

/// An envelope's refusal (§10): the reason only, never the text.
pub(crate) fn envelope_refusal(e: EnvelopeError) -> MasternodesError {
    match e {
        EnvelopeError::NotShared => message_refusal("not_shared"),
        EnvelopeError::TooLarge => message_refusal("too_large"),
        EnvelopeError::Parse(_) => message_refusal("parse"),
        EnvelopeError::Network => message_refusal("network"),
        EnvelopeError::Labels => message_refusal("labels"),
        EnvelopeError::BadSignature { share } => MasternodesError::SharedMessage {
            reason: "bad_signature".into(),
            share: Some(share),
        },
    }
}

/// A merge step's refusal: a change to this wallet's own row, contribution
/// or inputs is a consent failure; the rest are the message's.
pub(crate) fn merge_refusal(e: Refusal) -> MasternodesError {
    match e {
        Refusal::OtherSession => message_refusal("other_session"),
        Refusal::TermsChanged => message_refusal("terms_changed"),
        Refusal::Older => message_refusal("older"),
        Refusal::Conflict => message_refusal("conflict"),
        Refusal::RowChanged => funding_refusal("row_changed"),
        Refusal::ContributionChanged => funding_refusal("contribution_changed"),
        Refusal::ForeignInput => funding_refusal("foreign_input"),
        Refusal::BadSignature { share } => MasternodesError::SharedMessage {
            reason: "bad_signature".into(),
            share: Some(share),
        },
    }
}

pub(crate) fn consent_refusal(e: FundingMismatch) -> MasternodesError {
    funding_refusal(match e {
        FundingMismatch::Template => "template",
        FundingMismatch::TimeLock => "time_lock",
        FundingMismatch::RowChanged => "row_changed",
        FundingMismatch::ContributionChanged => "contribution_changed",
        FundingMismatch::ForeignInput => "foreign_input",
        FundingMismatch::Overdraw => "overdraw",
    })
}

/// A session id: 64 lower-case hex (D28). The refusal repeats nothing.
pub(crate) fn check_session_id(id: &str) -> Result<(), MasternodesError> {
    if id.len() == 64 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(
            EngineError::InvalidArgument("not a shared session id (64 lower-case hex)".into())
                .into(),
        )
    }
}

/// A stored session, read back.
#[derive(Debug, Clone)]
pub(crate) struct Loaded {
    pub row: SharedSessionRow,
    pub session: Session,
    pub role: SharedRole,
    pub recorded: Option<Recorded>,
    pub approved: Option<Approved>,
}

impl Loaded {
    /// Parses `row`. The stored envelope is the wallet's own rendering, so
    /// a failure is corruption (named without the text).
    pub fn of(row: SharedSessionRow, network: dashcore::Network) -> Result<Self, MasternodesError> {
        let corrupt = || {
            MasternodesError::from(EngineError::Internal(
                "a stored shared session does not parse".into(),
            ))
        };
        let json = Json::read(row.envelope.as_bytes()).ok_or_else(corrupt)?;
        let mut session = Session::from_json(&json, network).map_err(|_| corrupt())?;
        let role = match row.role.as_str() {
            "coordinator" => SharedRole::Coordinator,
            _ => SharedRole::Participant,
        };
        let recorded: Option<Recorded> = match &row.contribution {
            Some(c) => Some(serde_json::from_str(c).map_err(|_| corrupt())?),
            None => None,
        };
        let approved = row
            .approved_terms
            .as_deref()
            .and_then(|a| serde_json::from_str(a).ok());
        if let (Some(rec), Some(signed)) = (&recorded, &row.signed_tx) {
            overlay_own_signatures(&mut session, rec, signed);
        }
        Ok(Self {
            row,
            session,
            role,
            recorded,
            approved,
        })
    }

    pub fn own_inputs(&self) -> Vec<OutPoint> {
        self.recorded
            .as_ref()
            .map(Recorded::outpoints)
            .unwrap_or_default()
    }

    pub fn out(&self) -> bool {
        self.row.out_at.is_some()
    }

    /// Whether this wallet's consent signature is in the session, and
    /// verifies over the frozen registration.
    pub fn own_approval(&self) -> bool {
        let Some(r) = &self.recorded else {
            return false;
        };
        let index = r.share_index as i32;
        let Some(sig) = self.session.signature_for(index) else {
            return false;
        };
        decode_frozen(&self.session)
            .is_ok_and(|frozen| verify_consent_signature(&frozen, index, &sig.signature))
    }
}

/// A participant's signed copy written by `sign_shared_funding` (out first,
/// then the envelope): a crash between the two writes leaves the envelope
/// without this wallet's scriptSigs, so they are taken from `signed_tx`.
fn overlay_own_signatures(session: &mut Session, rec: &Recorded, signed: &[u8]) {
    if session.stage < Stage::Combined {
        return;
    }
    let (Ok(signed), Ok(mut frozen)) = (SpecialTx::from_bytes(signed), decode_frozen(session))
    else {
        return;
    };
    let own = rec.outpoints();
    let mut changed = false;
    for (input, theirs) in frozen.tx.input.iter_mut().zip(&signed.input) {
        if input.previous_output == theirs.previous_output
            && own.contains(&input.previous_output)
            && input.script_sig.is_empty()
            && !theirs.script_sig.is_empty()
        {
            input.script_sig.clone_from(&theirs.script_sig);
            changed = true;
        }
    }
    if changed {
        session.protx = hex::encode(frozen.tx.to_bytes());
        if frozen.tx.input.iter().all(|i| !i.script_sig.is_empty()) {
            session.stage = session.stage.max(Stage::FundingSigned);
        }
    }
}

/// What the wallet sees of the chain for a session's state (§9.2): its
/// history and unspent coins, and in tests the registrations the pin
/// can't decode (BL-187), as confirmed txids.
pub(crate) struct ChainView {
    pub history: HashMap<Txid, TxEntry>,
    pub unspent: HashSet<OutPoint>,
}

impl ChainView {
    fn confirmed_height(&self, txid: &Txid) -> Option<u32> {
        self.history
            .get(txid)
            .and_then(|e| e.context.block_info())
            .map(|b| b.height())
    }

    /// The session's registration in the wallet's history: the
    /// transaction spending exactly the frozen template's inputs, in order,
    /// to its outputs. A participant holds only its own signed copy, whose
    /// txid isn't the ProTx hash unless it signed last, so the txid can't
    /// find it (§3.2 step 6, "or see the ProRegTx"); and the pinned library
    /// reads a version-3 payload with the version-2 layout (BL-187), so the
    /// payload isn't compared.
    fn registration(&self, session: &Session) -> Option<Txid> {
        if session.stage < Stage::Frozen {
            return None;
        }
        let template = decode_frozen(session).ok()?.tx;
        self.history.iter().find_map(|(id, e)| {
            let same_inputs = e.tx.input.len() == template.input.len()
                && (e.tx.input.iter().zip(&template.input))
                    .all(|(a, b)| a.previous_output == b.previous_output);
            (same_inputs && e.tx.output == template.output).then_some(*id)
        })
    }

    /// Another transaction in a block spends one of `inputs`.
    fn spent_in_block(&self, inputs: &[OutPoint], except: Option<&Txid>) -> bool {
        self.history.iter().any(|(id, e)| {
            Some(id) != except
                && e.context.block_info().is_some()
                && e.tx
                    .input
                    .iter()
                    .any(|i| inputs.contains(&i.previous_output))
        })
    }
}

/// The registration's txid, once every input is signed: its ProTx hash.
pub(crate) fn complete_txid(session: &Session) -> Option<Txid> {
    if session.stage < Stage::Combined {
        return None;
    }
    let frozen = decode_frozen(session).ok()?;
    frozen
        .tx
        .input
        .iter()
        .all(|i| !i.script_sig.is_empty())
        .then(|| frozen.tx.txid())
}

/// The session's ProTx hash: its complete registration's txid, from the
/// envelope or the wallet's history.
pub(crate) fn session_hash(l: &Loaded, chain: &ChainView) -> Option<Txid> {
    complete_txid(&l.session).or_else(|| chain.registration(&l.session))
}

/// The session's stage (§3, §9.2), first match: confirmed, conflicted or
/// out; dead (a reserved input spent before `out_at`); then the envelope's.
pub(crate) fn derive_stage(l: &Loaded, chain: &ChainView) -> SharedStage {
    let txid = session_hash(l, chain)
        .or_else(|| l.row.txid.as_deref().and_then(|t| Txid::from_str(t).ok()));
    if let Some(height) = txid.as_ref().and_then(|t| chain.confirmed_height(t)) {
        return SharedStage::Confirmed { height };
    }
    let own = l.own_inputs();
    if l.out() {
        if chain.spent_in_block(&own, txid.as_ref()) {
            return SharedStage::Conflicted;
        }
        return match l.role {
            SharedRole::Coordinator => SharedStage::Sent,
            SharedRole::Participant => SharedStage::FundingSigned,
        };
    }
    if !own.is_empty() && own.iter().any(|o| !chain.unspent.contains(o)) {
        return SharedStage::Dead {
            reason: DeadReason::InputsSpent,
        };
    }
    match l.session.stage {
        Stage::Draft if l.recorded.is_none() => SharedStage::Invited,
        Stage::Draft => SharedStage::Draft,
        Stage::Frozen | Stage::Signing => SharedStage::Signing,
        Stage::Combined => SharedStage::Combined,
        Stage::FundingSigned => SharedStage::FundingSigned,
        Stage::Broadcast => SharedStage::Sent,
    }
}

pub(crate) fn terms_view(session: &Session) -> ShareTermsView {
    let t = &session.terms;
    ShareTermsView {
        service: t.core_p2p_addrs.clone(),
        operator_public_key: t.operator_pub_key.clone(),
        voting_address: t.voting_address.clone(),
        operator_reward: t.operator_reward,
        early_period_blocks: t.early_period_blocks,
        early_penalty: t.early_penalty,
    }
}

fn opt(s: &str) -> Option<String> {
    (!s.is_empty()).then(|| s.to_string())
}

/// Whether every input of `label`'s contribution is signed in the frozen
/// transaction.
fn contribution_signed(session: &Session, label: &str) -> bool {
    let Some(c) = session.contributions.iter().find(|c| c.label == label) else {
        return false;
    };
    let Ok(frozen) = decode_frozen(session) else {
        return false;
    };
    c.inputs.iter().all(|input| {
        frozen.tx.input.iter().any(|i| {
            i.previous_output
                .txid
                .to_string()
                .eq_ignore_ascii_case(&input.txid)
                && i.previous_output.vout == input.vout
                && !i.script_sig.is_empty()
        })
    })
}

/// Core's warnings a session carries (§8): the zero penalty, the empty
/// service (D5), the contributions this wallet can't value (D26).
fn warnings(l: &Loaded) -> Vec<SharedWarning> {
    let s = &l.session;
    let mut out = Vec::new();
    if s.terms.early_penalty == 0 {
        out.push(SharedWarning::ZeroPenalty);
    }
    if s.terms.core_p2p_addrs.trim().is_empty() {
        out.push(SharedWarning::EmptyService);
    }
    let own_label = l.recorded.as_ref().map(|r| r.label.as_str());
    let foreign: Vec<&Contribution> = s
        .contributions
        .iter()
        .filter(|c| Some(c.label.as_str()) != own_label)
        .collect();
    if !foreign.is_empty() {
        out.push(SharedWarning::UnvaluedInputs {
            shares: foreign
                .iter()
                .filter_map(|c| s.share_index_of(&c.label))
                .map(|i| i as u16)
                .collect(),
            count: foreign.iter().map(|c| c.inputs.len() as u32).sum(),
        });
    }
    out
}

/// What this wallet does next (§3.1, §3.2).
fn next_of(l: &Loaded, stage: SharedStage) -> SharedNext {
    let s = &l.session;
    let complete = s.shares.iter().all(|r| {
        !r.owner_address.is_empty()
            && !r.refund_address.is_empty()
            && s.contributions.iter().any(|c| c.label == r.label)
    });
    match (l.role, stage) {
        (_, SharedStage::Invited) => SharedNext::Join,
        (SharedRole::Coordinator, SharedStage::Draft) if complete => SharedNext::Lock,
        (_, SharedStage::Draft) => SharedNext::Copy,
        (SharedRole::Participant, SharedStage::Signing) if !l.own_approval() => SharedNext::Approve,
        (_, SharedStage::Signing) => SharedNext::Copy,
        (SharedRole::Participant, SharedStage::Combined) => SharedNext::Sign,
        (SharedRole::Coordinator, SharedStage::Combined | SharedStage::FundingSigned) => {
            let own = l.own_inputs();
            let others_signed = decode_frozen(s).is_ok_and(|f| {
                f.tx.input
                    .iter()
                    .filter(|i| !own.contains(&i.previous_output))
                    .all(|i| !i.script_sig.is_empty())
            });
            if others_signed {
                SharedNext::Send
            } else {
                SharedNext::Copy
            }
        }
        (SharedRole::Participant, SharedStage::FundingSigned) => SharedNext::Copy,
        (_, SharedStage::Sent) => SharedNext::Wait,
        _ => SharedNext::Nothing,
    }
}

/// The record of a stored session, its stage derived now.
pub(crate) fn view(l: &Loaded, chain: &ChainView) -> SharedSession {
    let s = &l.session;
    let stage = derive_stage(l, chain);
    let mine = l.recorded.as_ref().map(|r| r.share_index);
    let frozen = decode_frozen(s).ok();
    let rows = s
        .shares
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let approved = s.signature_for(i as i32).is_some_and(|sig| {
                frozen
                    .as_ref()
                    .is_some_and(|f| verify_consent_signature(f, i as i32, &sig.signature))
            });
            ShareRow {
                index: i as u16,
                label: r.label.clone(),
                amount: u64::try_from(r.amount).unwrap_or(0),
                owner_address: opt(&r.owner_address),
                refund_address: opt(&r.refund_address),
                reward_address: opt(&r.reward_address),
                mine: mine == Some(i),
                contributed: s.contributions.iter().any(|c| c.label == r.label),
                approved,
                funding_signed: contribution_signed(s, &r.label),
            }
        })
        .collect();
    SharedSession {
        id: s.session_id.clone(),
        code: s.session_code(),
        message_code: s.message_code(),
        kind: match l.row.kind.as_str() {
            "dissolve_together" => SharedKind::DissolveTogether,
            "key_change" => SharedKind::KeyChange,
            _ => SharedKind::Registration,
        },
        role: l.role,
        stage,
        revision: u32::try_from(s.revision).unwrap_or(0),
        rows,
        terms: terms_view(s),
        node_run_by: s.operator_secret_holder.clone(),
        fee: l.recorded.as_ref().and_then(|r| r.fee),
        pro_tx_hash: session_hash(l, chain).map(|t| t.to_string()),
        txid: l.row.txid.clone(),
        reserved: reserved_of(&l.row)
            .iter()
            .map(super::super::outpoint_string)
            .collect(),
        next: next_of(l, stage),
        warnings: warnings(l),
        approved_terms: l.approved.as_ref().map(|a| a.terms.clone()),
        base_keys: l
            .row
            .base_keys
            .as_deref()
            .and_then(|b| serde_json::from_str::<KeyBase>(b).ok()),
        actor_fee: None,
        created_at: l.row.created_at,
        updated_at: l.row.updated_at,
        last_error: l.row.last_error.clone(),
    }
}

/// The message names by stage and role (§2.3, `sharedmncreatedialog.cpp:2693-2709`).
pub(crate) fn message_name(stage: Stage, role: SharedRole) -> &'static str {
    match (stage, role) {
        (Stage::Draft, SharedRole::Coordinator) => "Invitation",
        (Stage::Draft, SharedRole::Participant) => "Details",
        (Stage::Frozen | Stage::Signing, SharedRole::Coordinator) => "Locked Terms",
        (Stage::Frozen | Stage::Signing, SharedRole::Participant) => "Approval",
        (Stage::Combined, SharedRole::Coordinator) => "Signing Request",
        (Stage::Combined | Stage::FundingSigned, _) => "Signed Contribution",
        (Stage::Broadcast, _) => "Final Record",
    }
}
