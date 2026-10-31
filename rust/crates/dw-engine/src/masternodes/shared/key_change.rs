//! Change keys together (MG-04b §3.3, D25, slice 2d): the shared
//! registrar update every owner signs, run as a maintenance session over
//! dash-qt's sigs envelope of kind `registrar` (`RotateSharedKeysDialog`,
//! rc.3 `qt/sharedmndialogs.cpp:1756-2140`). The funding wallet prepares
//! the request (`PrepareSharedRegistrarUpdate`,
//! `providertx_service.cpp:1336-1371`) on fee inputs it reserves and locks,
//! records the list's keys as its base and signs its shares; every other
//! owner imports it, approves and copies back a reply; the funder alone
//! inserts the signatures and signs its fee inputs again (`CombineShared`,
//! `:1224-1236`), so it alone sends (D99).
//!
//! The session is a `shared_mn_sessions` row of kind `key_change`: its
//! envelope is the request and the signatures verified so far, `my_shares`
//! this wallet's shares, `base_keys` the list's keys when it was prepared or
//! first imported (D25), and for the funder `reserved` its fee inputs and
//! `contribution` its [`Funded`] record. The row is the record of the send
//! (D99): `signed_tx`, `txid` and `out_at` are written before the bytes
//! leave, its locks stay until a block settles it, and
//! `abandon_shared_session` is its exit (§9.3, D102). Its stage is derived
//! on every read (D103).

use std::collections::{BTreeMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::hashes::{Hash, HashEngine, sha256};
use dashcore::{OutPoint, Transaction, TxOut, Txid};
use dw_appdb::{SharedSessionRow, SharedSessionUpdate};
use dw_protx::shared::envelope::{
    ReceivedSigs, SigEntry, SigsEnvelope, SigsKind, decode_message, fingerprint,
};
use dw_protx::shared::key_change::{self, Checked, Keys, Refusal};
use dw_protx::shared::rules::{self as shared_rules, SharedMn};
use dw_protx::shared::{CompactSignature, ProUpSharedRegTx, SpecialTx, payload_bytes};
use platform_wallet::masternode::MasternodeListSummary;
use serde::{Deserialize, Serialize};

use super::super::follow::ListView;
use super::super::protx::build::{Context, invalid, plan_funding, refused, shared_voting};
use super::super::protx::keys::{Claim, InUse};
use super::super::protx::ops::conflict_height;
use super::super::protx::{CHANGE_LEN, change_address, charge_fee};
use super::super::protx_records::KeepOr;
use super::super::shared_records::{
    DeadReason, KeyBase, KeyChangeRequest, KeyChangeView, MaintenanceSent, ShareRow,
    ShareTermsView, SharedImport, SharedKind, SharedMessage, SharedNext, SharedOutcome, SharedRole,
    SharedSession, SharedStage, SharedWarning,
};
use super::super::{MasternodesError, display_hex, outpoint_string};
use super::build::sign_own_inputs;
use super::dissolve::address_of;
use super::flow::signer_refusal;
use super::send::typed;
use super::standby::{in_history, p2pkh_address};
use super::store::{
    decode_outpoints, encode_outpoints, json, message_refusal, reserved_of, role_name, role_of,
    stage_refusal,
};
use super::together::{TogetherView, absorb, known_mn, unsigned};
use crate::coins::now_secs;
use crate::platform::BudgetPurpose;
use crate::send::lease_failure;
use crate::{EngineError, NetworkSession, WalletId};

/// The `kind` column of a key-change session.
pub(crate) const KIND: &str = "key_change";

/// The maintenance session id (m-17): SHA-256 of the envelope's `kind`
/// (`registrar`), the ProTx hash and the sign hash, both in internal (wire)
/// order, as 64 lower-case hex of the digest.
pub(crate) fn session_id(hash: [u8; 32], sign_hash: [u8; 32]) -> String {
    let mut engine = sha256::Hash::engine();
    engine.input(SigsKind::Registrar.key().as_bytes());
    engine.input(&hash);
    engine.input(&sign_hash);
    hex::encode(sha256::Hash::from_engine(engine).to_byte_array())
}

/// The funder's record of its request (the `contribution` column, D99):
/// what the send is charged, the fee inputs it reserved (kept after their
/// locks are released, so a settled session still reads its spend), its
/// change address, and when it abandoned the request before sending it
/// (D102).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Funded {
    pub fee: u64,
    pub inputs: Vec<String>,
    pub change: Option<String>,
    pub abandoned_at: Option<u64>,
}

impl Funded {
    fn inputs(&self) -> Vec<OutPoint> {
        decode_outpoints(&self.inputs)
    }

    /// The change address the record names.
    fn change(&self, network: dashcore::Network) -> Option<dashcore::Address> {
        crate::send::l1_address(self.change.as_deref()?, network).ok()
    }
}

/// A received request's refusal: `shared_message{reason}`, or Core's
/// reason for its trivial rules.
fn request_refusal(e: Refusal) -> MasternodesError {
    match e {
        Refusal::NotKeyChange => message_refusal("not_key_change"),
        Refusal::OtherMasternode => message_refusal("other_masternode"),
        Refusal::Rules(reason) => refused(reason),
        Refusal::InputsHash => message_refusal("inputs_hash"),
    }
}

/// A call's refusal at `stage`: `stale` as D25's `shared_message{stale}`,
/// any other as `shared_stage{stage}`.
fn key_change_refusal(stage: SharedStage) -> MasternodesError {
    match stage {
        SharedStage::Stale => message_refusal("stale"),
        stage => stage_refusal(stage),
    }
}

/// An approver's refusal of the funder's calls: only the funder can sign
/// the fee inputs again (`providertx_service.cpp:1229-1232`).
fn not_funder() -> MasternodesError {
    MasternodesError::SharedStage {
        stage: role_name(SharedRole::Participant).into(),
    }
}

/// The list's keys for `hash` now: `shared_too_early` while it doesn't
/// hold it.
fn list_keys_of(ctx: &Context, hash: [u8; 32]) -> Result<Keys, MasternodesError> {
    ctx.entries
        .iter()
        .find(|e| e.pro_tx_hash == hash)
        .map(listed_keys)
        .ok_or(MasternodesError::SharedTooEarly)
}

fn corrupt() -> MasternodesError {
    EngineError::Internal("a stored key-change session does not parse".into()).into()
}

/// `k` as a [`KeyBase`].
fn keys_view(k: Keys, network: dashcore::Network) -> KeyBase {
    KeyBase {
        operator_public_key: hex::encode(k.operator_public_key),
        voting_address: p2pkh_address(k.voting_key_id, network),
    }
}

/// The keys a list entry holds, its operator key in the basic
/// serialization a ProUpSharedRegTx writes (`basic_form`, as a kept key is
/// copied, `providertx_service.cpp:1352`). A key that isn't one, as the
/// null key a revocation leaves (`ResetOperatorFields`,
/// `evo/dmnstate.h:127-135`), stays as listed: it differs from every base
/// that held a key, and a request keeping it fails Core's rules.
fn listed_keys(entry: &MasternodeListSummary) -> Keys {
    Keys {
        operator_public_key: dw_protx::bls::basic_form(&entry.operator_public_key)
            .unwrap_or(entry.operator_public_key),
        voting_key_id: entry.voting_key_id,
    }
}

/// A stored session, read back.
#[derive(Debug, Clone)]
pub(crate) struct KeyChange {
    pub row: SharedSessionRow,
    pub hash: [u8; 32],
    /// The request, its owner signatures not inserted.
    pub request: SpecialTx,
    /// Its payload.
    pub payload: ProUpSharedRegTx,
    /// The verified signatures by share.
    pub sigs: BTreeMap<u16, CompactSignature>,
    /// This wallet's shares and their keychain owner keys, as written.
    pub mine: BTreeMap<u16, u32>,
    /// The list's keys when it was prepared or first imported (D25).
    pub base: KeyBase,
    /// The funder's record; `None` for an approver.
    pub funded: Option<Funded>,
}

impl KeyChange {
    /// Parses `row`. The envelope is the wallet's own rendering, so a
    /// failure is corruption (named without the text).
    pub fn of(row: SharedSessionRow, network: dashcore::Network) -> Result<Self, MasternodesError> {
        let Ok(dw_protx::shared::envelope::SharedMessage::Sigs(r)) =
            decode_message(&row.envelope, network)
        else {
            return Err(corrupt());
        };
        let (Some(request), SigsKind::Registrar) = (r.tx, r.kind) else {
            return Err(corrupt());
        };
        let payload = dashcore::consensus::deserialize(&request.payload).map_err(|_| corrupt())?;
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
        let base = row
            .base_keys
            .as_deref()
            .and_then(|b| serde_json::from_str::<KeyBase>(b).ok())
            .ok_or_else(corrupt)?;
        let funded = match (role_of(&row.role), row.contribution.as_deref()) {
            (SharedRole::Coordinator, Some(c)) => {
                Some(serde_json::from_str::<Funded>(c).map_err(|_| corrupt())?)
            }
            _ => None,
        };
        Ok(Self {
            hash: r.pro_tx_hash.to_byte_array(),
            row,
            request,
            payload,
            sigs,
            mine,
            base,
            funded,
        })
    }

    fn keys(&self) -> Keys {
        Keys {
            operator_public_key: self.payload.operator_public_key,
            voting_key_id: self.payload.voting_key_id,
        }
    }

    fn envelope(&self, network: dashcore::Network) -> SigsEnvelope {
        envelope(network, self.hash, &self.request, &self.sigs)
    }

    /// The message to copy: the request with every signature held. The
    /// funder's is the Request, an approver's its Reply.
    pub fn message(&self, network: dashcore::Network) -> SharedMessage {
        let json = self.envelope(network).to_json();
        let code: String = self.row.id.chars().take(6).collect();
        SharedMessage {
            text: String::from_utf8(json.write(2)).expect("the writer emits UTF-8"),
            code: fingerprint(&json),
            name: match role_of(&self.row.role) {
                SharedRole::Coordinator => "Request",
                SharedRole::Participant => "Reply",
            }
            .into(),
            file_name: format!("shared-mn-session-{}.json", code.to_ascii_uppercase()),
        }
    }

    /// The funder's fee inputs (none for an approver).
    fn own_inputs(&self) -> Vec<OutPoint> {
        self.funded.as_ref().map(Funded::inputs).unwrap_or_default()
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
        kind: SigsKind::Registrar,
        pro_tx_hash: Txid::from_byte_array(hash),
        tx: request.to_bytes(),
        signatures: sigs.iter().map(|(i, s)| (*i, *s)).collect(),
    }
}

/// The synced list's keys for the masternode now: `None` while the list
/// isn't synced or doesn't hold it.
fn current_keys(list: Option<&ListView>, hash: &[u8; 32]) -> Option<Keys> {
    let l = list.filter(|l| l.synced)?;
    l.entry(hash).map(listed_keys)
}

fn same_keys(k: Keys, base: &KeyBase, network: dashcore::Network) -> bool {
    keys_view(k, network) == *base
}

/// The stage (D103), first match. The funder: abandoned before its send;
/// its sent transaction in a block (`confirmed`); out, a fee input spent in
/// a block by another transaction (`conflicted`); not out, a fee input
/// spent (`dead{inputs_spent}`). Then for every wallet: the masternode out
/// of the synced list once the wallet has scanned to its height
/// (`dead{delisted}`, D81's guard); for an approver, the list's keys the
/// requested ones (`confirmed` at the list's height, D27); the list's keys
/// other than the base (`stale`, D25), for a request out other than its
/// requested ones too; then `sent` for one out, else the signatures. A
/// request out stays valid in Core whatever keys the list took since (the
/// owners' signatures don't commit to the base), so its resend would undo
/// a later change: `stale` refuses it.
fn derive_stage(
    k: &KeyChange,
    mn: Option<&SharedMn>,
    v: &TogetherView,
    network: dashcore::Network,
) -> SharedStage {
    let out = k.funded.is_some() && k.row.out_at.is_some();
    if let Some(f) = &k.funded {
        if f.abandoned_at.is_some() && !out {
            return SharedStage::Dead {
                reason: DeadReason::Abandoned,
            };
        }
        let txid = k.row.txid.as_deref().and_then(|t| Txid::from_str(t).ok());
        if let Some(height) = txid.as_ref().and_then(|t| v.chain.confirmed_height(t)) {
            return SharedStage::Confirmed { height };
        }
        let own = f.inputs();
        if out {
            if conflict_height(&v.chain.history, &own, txid.as_ref()).is_some() {
                return SharedStage::Conflicted;
            }
        } else if own.iter().any(|o| !v.chain.unspent.contains(o)) {
            return SharedStage::Dead {
                reason: DeadReason::InputsSpent,
            };
        }
    }
    let Some(mn) = mn else {
        return match out {
            true => SharedStage::Sent,
            false => SharedStage::Signing,
        };
    };
    let list = v.list.as_ref();
    let gone = list.is_some_and(|l| {
        l.synced && l.height.is_some_and(|h| h > mn.registered_height) && l.entry(&k.hash).is_none()
    });
    if gone && v.scanned >= v.list_bound {
        return SharedStage::Dead {
            reason: DeadReason::Delisted,
        };
    }
    if let Some(current) = current_keys(list, &k.hash) {
        let requested = k.keys();
        if k.funded.is_none() && current == requested && !same_keys(requested, &k.base, network) {
            let height = list.and_then(|l| l.height).unwrap_or(0);
            return SharedStage::Confirmed { height };
        }
        if !same_keys(current, &k.base, network) && !(out && current == requested) {
            return SharedStage::Stale;
        }
    }
    if out {
        return SharedStage::Sent;
    }
    match k.sigs.len() == mn.shares.len() {
        true => SharedStage::Combined,
        false => SharedStage::Signing,
    }
}

impl NetworkSession {
    /// Whether `id` is a stored key-change session of the wallet.
    pub(crate) async fn is_key_change(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<bool, MasternodesError> {
        Ok(self
            .shared_row(wallet, id)
            .await?
            .is_some_and(|r| r.kind == KIND))
    }

    /// The wallet's key-change sessions, newest first.
    pub(crate) async fn key_change_rows(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<SharedSessionRow>, MasternodesError> {
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.shared_sessions(&w)).await?;
        Ok(rows.into_iter().filter(|r| r.kind == KIND).collect())
    }

    async fn key_change_stored(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<Option<KeyChange>, MasternodesError> {
        match self.shared_row(wallet, id).await? {
            Some(row) if row.kind == KIND => {
                Ok(Some(KeyChange::of(row, self.network.core_network())?))
            }
            Some(_) => Err(invalid("not a key-change session")),
            None => Ok(None),
        }
    }

    /// A stored key-change session: `shared_session_not_found`, or
    /// `invalid_argument` for a session of another kind.
    pub(crate) async fn key_change_loaded(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<KeyChange, MasternodesError> {
        self.key_change_stored(wallet, id)
            .await?
            .ok_or(MasternodesError::SharedSessionNotFound)
    }

    /// The session's record, its stage derived now (D103).
    pub(crate) fn key_change_session(&self, k: &KeyChange, v: &TogetherView) -> SharedSession {
        let network = self.network.core_network();
        let known = in_history(&v.chain, k.hash).ok();
        let mn = known
            .as_ref()
            .map(|(height, p)| known_mn(k.hash, *height, p));
        let stage = derive_stage(k, mn.as_ref(), v, network);
        view(k, mn.as_ref(), v, stage, network)
    }

    /// The session's stage, derived now.
    pub(crate) async fn key_change_stage(
        &self,
        wallet: WalletId,
        k: &KeyChange,
    ) -> Result<SharedStage, MasternodesError> {
        let v = self.together_read(wallet).await?;
        Ok(self.key_change_session(k, &v).stage)
    }

    /// The record of `id` with its message.
    pub(crate) async fn key_change_outcome(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let k = self.key_change_loaded(wallet, id).await?;
        let v = self.together_read(wallet).await?;
        Ok(SharedOutcome {
            session: self.key_change_session(&k, &v),
            message: k.message(self.network.core_network()),
        })
    }

    /// D23 for a key change sent from this wallet: one of masternode `hash`
    /// out (`signed_tx` saved) and not yet buried in a block is
    /// `update_pending{operation}`, naming the session, `stale` included:
    /// Core may still mine it until the abandon spends a fee input (D105).
    /// `except`: the session being resent.
    pub(crate) async fn check_no_key_change_out(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        except: Option<&str>,
    ) -> Result<(), MasternodesError> {
        let network = self.network.core_network();
        let rows: Vec<_> = self
            .key_change_rows(wallet)
            .await?
            .into_iter()
            .filter(|r| r.out_at.is_some() && Some(r.id.as_str()) != except)
            .collect();
        if rows.is_empty() {
            return Ok(());
        }
        let v = self.together_read(wallet).await?;
        let buried = self.buried();
        for row in rows {
            let id = row.id.clone();
            // Out but no longer read: held, as an out row is (D17).
            let Ok(k) = KeyChange::of(row, network) else {
                return Err(MasternodesError::UpdatePending { operation: id });
            };
            if k.hash != hash {
                continue;
            }
            let pending = match self.key_change_session(&k, &v).stage {
                SharedStage::Confirmed { height } => !buried(height),
                SharedStage::Sent | SharedStage::Stale => true,
                _ => false,
            };
            if pending {
                return Err(MasternodesError::UpdatePending { operation: id });
            }
        }
        Ok(())
    }

    /// A key change's keys for MG-04's checks (D99): each session's requested
    /// operator and voting keys count as in use until it is discarded, as a
    /// registration session's do (D10); and its operator key is a pending
    /// claim (MG-04 D51) while the request may still go out, or is out and
    /// not yet in the list, as Core's mempool indexes a ProUpSharedRegTx's
    /// (`txmempool.cpp:695-698`).
    pub(crate) async fn key_change_keys(
        &self,
        wallet: WalletId,
        used: &mut InUse,
        pending: &mut Vec<Claim>,
    ) -> Result<(), MasternodesError> {
        let rows = self.key_change_rows(wallet).await?;
        if rows.is_empty() {
            return Ok(());
        }
        let network = self.network.core_network();
        let v = self.together_read(wallet).await?;
        let list_height = v.list.as_ref().and_then(|l| l.height);
        for row in rows {
            let Ok(k) = KeyChange::of(row, network) else {
                continue;
            };
            used.operator.insert(k.payload.operator_public_key);
            used.voting.insert(k.payload.voting_key_id);
            let may_go_out = match self.key_change_session(&k, &v).stage {
                SharedStage::Dead { .. } | SharedStage::Conflicted => false,
                // The funder's, by its history: listed once the list is at
                // its block; an approver's is the list's own.
                SharedStage::Confirmed { height } => {
                    k.funded.is_some() && list_height.is_none_or(|l| l < height)
                }
                _ => true,
            };
            if may_go_out {
                pending.push(Claim {
                    id: Some(k.row.id.clone()),
                    operator: Some(k.payload.operator_public_key),
                    ..Claim::default()
                });
            }
        }
        Ok(())
    }

    /// Writes a new session, or the signatures and shares of a stored one
    /// (its other columns kept).
    #[expect(clippy::too_many_arguments, reason = "one row's columns")]
    async fn write_key_change(
        &self,
        wallet: WalletId,
        id: &str,
        hash: [u8; 32],
        request: &SpecialTx,
        sigs: &BTreeMap<u16, CompactSignature>,
        mine: &BTreeMap<u16, u32>,
        stored: Option<&SharedSessionRow>,
        new: Option<(SharedRole, &KeyBase, Option<&Funded>)>,
    ) -> Result<(), MasternodesError> {
        let network = self.network.core_network();
        let envelope = envelope(network, hash, request, sigs).to_text();
        let my_shares = json(mine)?;
        let now = now_secs();
        let w = wallet.to_string();
        match (stored, new) {
            (Some(row), _) => {
                let update = SharedSessionUpdate {
                    envelope,
                    my_shares,
                    contribution: row.contribution.clone(),
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
            (None, Some((role, base, funded))) => {
                let reserved = funded.map_or_else(Vec::new, |f| f.inputs.clone());
                let row = SharedSessionRow {
                    id: id.to_string(),
                    kind: KIND.into(),
                    role: role_name(role).into(),
                    envelope,
                    my_shares,
                    contribution: funded.map(json).transpose()?,
                    reserved: json(&reserved)?,
                    approved_terms: None,
                    base_keys: Some(json(base)?),
                    signed_tx: None,
                    txid: None,
                    out_at: None,
                    abandon_txid: None,
                    created_at: now,
                    updated_at: now,
                    last_error: None,
                };
                // The fee inputs are locked with the row (MG-04 D53).
                self.appdb_op(move |db| db.insert_shared_session_durable(&w, &row, now))
                    .await?;
            }
            (None, None) => return Err(MasternodesError::SharedSessionNotFound),
        }
        self.masternodes.local_change();
        Ok(())
    }

    /// Signs `checked` with each of `to_sign` (share, keychain owner key)
    /// under one `SharedMasternode` lease charged 0 (§6.3, D100: the funder
    /// pays the fee, charged at the send), each signature checked to be
    /// its share's, adding them to `sigs`.
    async fn sign_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        mn: &SharedMn<'_>,
        checked: &Checked,
        to_sign: &[(u16, u32)],
        sigs: &mut BTreeMap<u16, CompactSignature>,
    ) -> Result<(), MasternodesError> {
        let lease = self.shared_lease(wallet, grant).await?;
        let result = (|| {
            let (_, signer) = lease
                .charge_tx_signer(BudgetPurpose::Masternode, 0)
                .map_err(|e| lease_failure(e, 0))?;
            let payload = payload_bytes(&checked.payload);
            for (share, owner_index) in to_sign {
                let sig = signer
                    .sign_update_shared_registrar(*owner_index, &payload)
                    .map_err(signer_refusal)?;
                // The keychain key must be the share's (D11).
                if !key_change::verifies(mn, checked.sign_hash, *share, &sig) {
                    return Err(MasternodesError::ShareNotMine);
                }
                sigs.insert(*share, sig);
            }
            Ok(())
        })();
        lease.end();
        result
    }

    /// Prepares the request as `protx shared_update_registrar_prepare`
    /// does (D100): each key kept (the list's) or set (MG-04's pickers, a
    /// voting address P2PKH as dash-qt requires, `sharedmndialogs.cpp:
    /// 2030-2036`, and no share's owner key, MG-04 D52); the fee inputs picked and reserved under the coin
    /// fence; Core's rules on the request with empty signatures; the base
    /// recorded (D25); this wallet's shares signed under a lease charged 0;
    /// the row written with its locks. A request that changes no key is
    /// refused.
    pub(super) async fn start_key_change_session(
        self: &Arc<Self>,
        wallet: WalletId,
        request: KeyChangeRequest,
        hash: [u8; 32],
        grant: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let network = self.network.core_network();
        let ctx = super::v24_context(self, wallet).await?;
        // The fence from the coin read to the row's write (D68).
        let fence = self.coin_fence(wallet).await?;
        self.check_no_pending_maintenance(wallet, hash, None)
            .await?;
        let reg = self.registered(wallet, hash, &ctx).await?;
        let mn = reg.mn();
        let mine: BTreeMap<u16, u32> = self.my_shares(wallet, &reg).await?.into_iter().collect();
        if mine.is_empty() {
            return Err(MasternodesError::ShareNotMine);
        }
        let listed = list_keys_of(&ctx, hash)?;
        let operator = match &request.operator {
            KeepOr::Keep => listed.operator_public_key,
            KeepOr::Set(choice) => {
                let p = self.pick_operator(wallet, choice, &ctx.used).await?;
                ctx.check_pending(&Claim {
                    operator: Some(p.public_key),
                    ..Claim::default()
                })?;
                p.public_key
            }
        };
        let voting = match &request.voting {
            KeepOr::Keep => listed.voting_key_id,
            KeepOr::Set(choice) => {
                let key_id = self.pick_voting(wallet, choice, &ctx.used).await?.key_id;
                // MG-04 D52 (§6.1): voting never shares an owner's key.
                if mn.shares.iter().any(|s| s.owner_key_id == key_id) {
                    return Err(shared_voting());
                }
                key_id
            }
        };
        let keys = Keys {
            operator_public_key: operator,
            voting_key_id: voting,
        };
        if keys == listed {
            return Err(invalid("the key change changes no key"));
        }
        let w = self.wallet(&wallet).await?;
        let coins = self
            .protx_candidates(&fence, &w, &HashSet::new(), None)
            .await?;
        let utxos: Vec<_> = coins.iter().map(|(u, _)| u.clone()).collect();
        let size = payload_bytes(&key_change::placeholder(&mn, keys)).len();
        let funding = plan_funding(&utxos, &[], size, CHANGE_LEN).map_err(EngineError::from)?;
        let change = change_address(&w, &funding).await?;
        let change_out = funding.change.zip(change.as_ref()).map(|(value, a)| TxOut {
            value,
            script_pubkey: a.script_pubkey(),
        });
        let inputs: Vec<OutPoint> = funding.inputs.iter().map(|u| u.outpoint).collect();
        let tx = key_change::prepare(&mn, keys, &inputs, change_out);
        // Core's trivial rules: a kept null key is `bad-protx-key-null`.
        let checked = key_change::check(&tx, &mn).map_err(request_refusal)?;
        // Core's rules on the request: the operator key unique in the list,
        // the voting key no share's payee, `inputsHash` (D12).
        shared_rules::check_update_shared_registrar(
            &tx,
            &checked.payload,
            Some(&mn),
            &ctx.listed,
            true,
            false,
        )
        .map_err(refused)?;
        let id = session_id(hash, checked.sign_hash);
        let _flight = self.claim_shared_session(wallet, &id)?;
        // The same request as one abandoned before its send, whose coins
        // were released and selected again: the abandoned row gives way
        // (D102), once the new one is signed. Any other row holds its
        // coins, so its id can't recur.
        let replaces = match self.key_change_stored(wallet, &id).await? {
            None => false,
            Some(old) => {
                let abandoned = old
                    .funded
                    .as_ref()
                    .is_some_and(|f| f.abandoned_at.is_some());
                if old.row.out_at.is_some() || !abandoned {
                    return Err(stage_refusal(self.key_change_stage(wallet, &old).await?));
                }
                true
            }
        };
        let mut sigs = BTreeMap::new();
        let to_sign: Vec<(u16, u32)> = unsigned(&mine, &sigs).collect();
        self.sign_key_change(wallet, grant, &mn, &checked, &to_sign, &mut sigs)
            .await?;
        if replaces {
            let (w, old_id) = (wallet.to_string(), id.clone());
            self.appdb_op(move |db| {
                db.delete_unsent_shared_session_durable(&w, &old_id, now_secs())
            })
            .await?;
        }
        let funded = Funded {
            fee: funding.fee,
            inputs: encode_outpoints(&inputs),
            change: change.map(|a| a.to_string()),
            abandoned_at: None,
        };
        let base = keys_view(listed, network);
        self.write_key_change(
            wallet,
            &id,
            hash,
            &tx,
            &sigs,
            &mine,
            None,
            Some((SharedRole::Coordinator, &base, Some(&funded))),
        )
        .await?;
        drop(fence);
        self.key_change_outcome(wallet, &id).await
    }

    /// A `registrar` sigs envelope or a bare array for a key-change session
    /// (§2.5, D104): the request checked against the registration in the
    /// wallet's history and the list (D29, m-16); each signature verified
    /// and taken, or skipped with dash-qt's reason; the session created
    /// (this wallet an approver, its base the list's keys now) or updated.
    /// Signs nothing.
    pub(super) async fn import_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        received: Option<ReceivedSigs>,
        bare: Option<(Vec<SigEntry>, String)>,
        target: Option<String>,
    ) -> Result<SharedImport, MasternodesError> {
        let target = match &target {
            Some(id) => Some(self.key_change_loaded(wallet, id).await?),
            None => None,
        };
        let network = self.network.core_network();
        let (hash, tx, entries, code, (edited, no_approvals)) = match (received, bare) {
            (Some(r), _) => {
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
                let Some(t) = &target else {
                    return Err(message_refusal("parse"));
                };
                (t.hash, None, entries, code, (false, false))
            }
            (None, None) => return Err(message_refusal("parse")),
        };
        let request = match (tx, &target) {
            // Another request: the sign hash binds the keys and the fee
            // inputs; scriptSigs, sequences and outputs are the funder's.
            (Some(tx), Some(t)) if payload_of(&tx).sign_hash() != t.payload.sign_hash() => {
                return Err(message_refusal("other_session"));
            }
            (_, Some(t)) if t.hash != hash => return Err(message_refusal("other_session")),
            (Some(tx), _) => tx,
            (None, Some(t)) => t.request.clone(),
            (None, None) => return Err(message_refusal("parse")),
        };
        let ctx = super::v24_context(self, wallet).await?;
        let reg = match self.registered(wallet, hash, &ctx).await {
            // A late reply once the masternode has left the list: the stored
            // session's stage, past `signing` and `combined`.
            Err(MasternodesError::SharedTooEarly) => {
                return Err(self.key_change_late(wallet, hash, &request).await?);
            }
            r => r?,
        };
        let mn = reg.mn();
        let checked = key_change::check(&request, &mn).map_err(request_refusal)?;
        let id = session_id(hash, checked.sign_hash);
        if target.as_ref().is_some_and(|t| t.row.id != id) {
            return Err(message_refusal("other_session"));
        }
        let _flight = self.claim_shared_session(wallet, &id)?;
        let stored = self.key_change_stored(wallet, &id).await?;
        // A stored copy is kept as written: the funder's own request has
        // its scriptSigs empty, a received one may carry the funder's.
        let request = stored.as_ref().map_or(request, |k| k.request.clone());
        let mut sigs = stored.as_ref().map(|k| k.sigs.clone()).unwrap_or_default();
        let (taken, skipped) = absorb(&mn, checked.sign_hash, &mut sigs, &entries);
        let mine: BTreeMap<u16, u32> = self.my_shares(wallet, &reg).await?.into_iter().collect();
        let changed = match &stored {
            None => true,
            Some(k) => taken > 0 || k.mine != mine,
        };
        if changed {
            // The base: the list's keys when first imported (D25).
            let base = keys_view(list_keys_of(&ctx, hash)?, network);
            self.write_key_change(
                wallet,
                &id,
                hash,
                &request,
                &sigs,
                &mine,
                stored.as_ref().map(|k| &k.row),
                Some((SharedRole::Participant, &base, None)),
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
        let outcome = self.key_change_outcome(wallet, &id).await?;
        Ok(SharedImport::Session {
            session: Box::new(outcome.session),
            changed,
            received_code: code,
            warnings,
        })
    }

    /// The refusal of a message for `request` while the masternode isn't in
    /// the list above its registration: the stored session's stage once it
    /// is past `signing` and `combined`, else `shared_too_early`.
    async fn key_change_late(
        &self,
        wallet: WalletId,
        hash: [u8; 32],
        request: &SpecialTx,
    ) -> Result<MasternodesError, MasternodesError> {
        let network = self.network.core_network();
        let sign_hash = payload_of(request).sign_hash();
        for row in self.key_change_rows(wallet).await? {
            let Ok(k) = KeyChange::of(row, network) else {
                continue;
            };
            if k.hash != hash || k.payload.sign_hash() != sign_hash {
                continue;
            }
            let stage = self.key_change_stage(wallet, &k).await?;
            if !matches!(stage, SharedStage::Signing | SharedStage::Combined) {
                return Ok(stage_refusal(stage));
            }
        }
        Ok(MasternodesError::SharedTooEarly)
    }

    /// Signs this wallet's unsigned shares of a key change under a lease
    /// charged 0 (§6.3, D100): `signing`, the list's keys still the base
    /// (D25, `shared_message{stale}`), D23.
    pub(super) async fn approve_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        grant: &str,
    ) -> Result<SharedOutcome, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let _flight = self.claim_shared_session(wallet, id)?;
        let k = self.key_change_loaded(wallet, id).await?;
        let stage = self.key_change_stage(wallet, &k).await?;
        if stage != SharedStage::Signing {
            return Err(key_change_refusal(stage));
        }
        self.check_no_pending_maintenance(wallet, k.hash, None)
            .await?;
        let reg = self.registered(wallet, k.hash, &ctx).await?;
        let mn = reg.mn();
        let checked = key_change::check(&k.request, &mn).map_err(request_refusal)?;
        let mine: BTreeMap<u16, u32> = self.my_shares(wallet, &reg).await?.into_iter().collect();
        if mine.is_empty() {
            return Err(MasternodesError::ShareNotMine);
        }
        let to_sign: Vec<(u16, u32)> = unsigned(&mine, &k.sigs).collect();
        if !to_sign.is_empty() {
            let mut sigs = k.sigs.clone();
            self.sign_key_change(wallet, grant, &mn, &checked, &to_sign, &mut sigs)
                .await?;
            self.write_key_change(
                wallet,
                id,
                k.hash,
                &k.request,
                &sigs,
                &mine,
                Some(&k.row),
                None,
            )
            .await?;
        }
        self.key_change_outcome(wallet, id).await
    }

    /// The funder's send (D99, D100): `combined`, the list's keys still the
    /// base, D23 both ways; the signatures inserted in share order and the
    /// fee inputs signed again over the result (`CombineShared`, `:1224-
    /// 1236`); Core's rules with the signatures; the fee charged under a
    /// `SharedMasternode` lease; `signed_tx`, `txid` and `out_at` saved
    /// before the hand-off. A session already `sent` is resent: the same
    /// bytes, charged again (a hand-off that never left was refunded).
    pub(super) async fn send_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        grant: &str,
    ) -> Result<MaintenanceSent, MasternodesError> {
        let ctx = super::v24_context(self, wallet).await?;
        let _updating = self.masternodes.update_lock().await;
        let fence = self.coin_fence(wallet).await?;
        let _flight = self.claim_shared_session(wallet, id)?;
        let k = self.key_change_loaded(wallet, id).await?;
        let Some(funded) = k.funded.as_ref() else {
            return Err(not_funder());
        };
        let stage = self.key_change_stage(wallet, &k).await?;
        match stage {
            SharedStage::Combined => {}
            SharedStage::Sent => {
                self.resend_key_change(wallet, grant, &k, funded, fence)
                    .await?;
                return self.key_change_sent(wallet, id).await;
            }
            stage => return Err(key_change_refusal(stage)),
        }
        self.check_no_pending_maintenance(wallet, k.hash, None)
            .await?;
        let reg = self.registered(wallet, k.hash, &ctx).await?;
        let mn = reg.mn();
        let checked = key_change::check(&k.request, &mn).map_err(request_refusal)?;
        let combined = key_change::combine(&k.request, &checked, &mn, &k.sigs)
            .map_err(|e| EngineError::Internal(format!("combining a complete session: {e}")))?;
        let outpoints: Vec<OutPoint> = combined.input.iter().map(|i| i.previous_output).collect();
        let own = self.reserved_inputs(wallet, &k.row, &outpoints).await?;
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, signer) = charge_fee(&lease, funded.fee)?;
            let signed = sign_own_inputs(&combined, &own, &signer).await?;
            let p = payload_of(&signed);
            shared_rules::check_update_shared_registrar(
                &signed,
                &p,
                Some(&mn),
                &ctx.listed,
                true,
                true,
            )
            .map_err(refused)?;
            let tx = typed(&signed)?;
            let (w, sid, bytes, txid) = (
                wallet.to_string(),
                id.to_string(),
                signed.to_bytes(),
                tx.txid().to_string(),
            );
            // Out before the bytes can leave (D17).
            let out = self
                .appdb_op(move |db| {
                    db.mark_shared_session_out_durable(&w, &sid, &bytes, &txid, now_secs())
                })
                .await?;
            if !out {
                return Err(stage_refusal(SharedStage::Sent));
            }
            let change = funded.change(self.network.core_network());
            let summary = self
                .shared_summary(&fence, &tx, funded.fee, change.as_ref())
                .await?;
            self.deliver_shared(wallet, id, tx, summary, charge, fence, false)
                .await
        }
        .await;
        lease.end();
        result?;
        self.key_change_sent(wallet, id).await
    }

    /// A resend of the funder's saved key change: the same bytes under a
    /// new lease charged the fee, as the registration's resend; D23 holds it
    /// while another operation or key change of the masternode is pending.
    async fn resend_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        k: &KeyChange,
        funded: &Funded,
        fence: crate::coins::CoinFence,
    ) -> Result<(), MasternodesError> {
        let tx: Transaction = k
            .row
            .signed_tx
            .as_deref()
            .and_then(|b| dashcore::consensus::deserialize(b).ok())
            .ok_or_else(|| EngineError::Internal("the saved key change".into()))?;
        self.check_no_pending_ops(wallet, k.hash, None).await?;
        self.check_no_key_change_out(wallet, k.hash, Some(&k.row.id))
            .await?;
        self.check_resend_inputs(wallet, &tx)?;
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, _) = charge_fee(&lease, funded.fee)?;
            let change = funded.change(self.network.core_network());
            let summary = self
                .shared_summary(&fence, &tx, funded.fee, change.as_ref())
                .await?;
            self.deliver_shared(wallet, &k.row.id, tx, summary, charge, fence, true)
                .await
        }
        .await;
        lease.end();
        result
    }

    async fn key_change_sent(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<MaintenanceSent, MasternodesError> {
        let session = self.key_change_outcome(wallet, id).await?.session;
        Ok(MaintenanceSent::Session {
            session: Box::new(session),
        })
    }

    /// The funder's abandon of a key change (D102). Before `out_at` nothing
    /// is spent: no other wallet can finish the request (its fee inputs'
    /// signatures are the funder's alone, made after the insert), so the
    /// reservation and its locks are released and the row kept, reading
    /// `dead{abandoned}`; charged 0. Out, it spends a fee input to the
    /// wallet as a registration's abandon does (§9.3). An approver has
    /// nothing to abandon (`shared_stage{participant}`); a settled session
    /// is `shared_stage{stage}`.
    pub(super) async fn abandon_key_change(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        row: SharedSessionRow,
    ) -> Result<SharedSession, MasternodesError> {
        let id = row.id.clone();
        let k = KeyChange::of(row, self.network.core_network())?;
        let Some(funded) = &k.funded else {
            return Err(not_funder());
        };
        let stage = self.key_change_stage(wallet, &k).await?;
        if matches!(
            stage,
            SharedStage::Confirmed { .. } | SharedStage::Conflicted | SharedStage::Dead { .. }
        ) {
            return Err(stage_refusal(stage));
        }
        if k.row.out_at.is_none() {
            let record = json(&Funded {
                abandoned_at: Some(now_secs()),
                ..funded.clone()
            })?;
            let (w, sid) = (wallet.to_string(), id.clone());
            let done = self
                .appdb_op(move |db| {
                    db.abandon_unsent_shared_session_durable(&w, &sid, &record, now_secs())
                })
                .await?;
            if !done {
                // Out since it was read: the stage it reads now.
                let k = self.key_change_loaded(wallet, &id).await?;
                return Err(stage_refusal(self.key_change_stage(wallet, &k).await?));
            }
            self.masternodes.local_change();
            self.hub.pump.mark_balances(wallet);
        } else {
            let fence = self.coin_fence(wallet).await?;
            self.abandon_session(wallet, grant, &k.row, fence).await?;
        }
        Ok(self.key_change_outcome(wallet, &id).await?.session)
    }

    /// Whether `row`, a key-change session, may be discarded (D17): `Ok`
    /// while it isn't out, `shared_stage{stage}` once it is.
    pub(crate) async fn key_change_discard(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
    ) -> Result<(), MasternodesError> {
        if row.out_at.is_none() {
            return Ok(());
        }
        let stage = match KeyChange::of(row.clone(), self.network.core_network()) {
            Ok(k) => self.key_change_stage(wallet, &k).await?,
            Err(_) => SharedStage::Sent,
        };
        Err(stage_refusal(stage))
    }

    /// The follower's pass over key-change sessions holding locks (§11):
    /// one `confirmed` in a buried block, `conflicted` by a buried spend,
    /// or `dead` releases them; one `sent`, `stale` or still collecting
    /// keeps them.
    pub(crate) async fn key_change_follow_pass(
        &self,
        wallet: WalletId,
    ) -> Result<bool, MasternodesError> {
        let rows: Vec<_> = self
            .key_change_rows(wallet)
            .await?
            .into_iter()
            .filter(|r| !reserved_of(r).is_empty())
            .collect();
        if rows.is_empty() {
            return Ok(false);
        }
        let network = self.network.core_network();
        let v = self.together_read(wallet).await?;
        let buried = self.buried();
        let mut released = false;
        for listed in rows {
            let Some(_flight) = self.try_claim_shared_session(wallet, &listed.id) else {
                continue;
            };
            let Some(row) = self.shared_row(wallet, &listed.id).await? else {
                continue;
            };
            if row != listed {
                continue;
            }
            let Ok(k) = KeyChange::of(row, network) else {
                continue;
            };
            let release = match self.key_change_session(&k, &v).stage {
                SharedStage::Confirmed { height } => buried(height),
                SharedStage::Conflicted => {
                    let txid = k.row.txid.as_deref().and_then(|t| Txid::from_str(t).ok());
                    conflict_height(&v.chain.history, &k.own_inputs(), txid.as_ref())
                        .is_some_and(&buried)
                }
                SharedStage::Dead { .. } => true,
                _ => false,
            };
            if !release {
                continue;
            }
            let (w, id, expected) = (wallet.to_string(), k.row.id, k.row.reserved);
            released |= self
                .appdb_op(move |db| {
                    db.release_shared_session_locks_durable(&w, &id, &expected, now_secs())
                })
                .await?;
        }
        Ok(released)
    }
}

/// The payload of a key-change transaction this wallet wrote or checked.
fn payload_of(tx: &SpecialTx) -> ProUpSharedRegTx {
    dashcore::consensus::deserialize(&tx.payload).unwrap_or(ProUpSharedRegTx {
        version: 0,
        pro_tx_hash: Txid::all_zeros(),
        operator_public_key: [0; 48],
        voting_key_id: [0; 20],
        inputs_hash: [0; 32],
        sigs: Vec::new(),
    })
}

fn view(
    k: &KeyChange,
    mn: Option<&SharedMn>,
    v: &TogetherView,
    stage: SharedStage,
    network: dashcore::Network,
) -> SharedSession {
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
                    mine: k.mine.contains_key(&i),
                    contributed: false,
                    approved: k.sigs.contains_key(&i),
                    funding_signed: false,
                })
                .collect()
        })
        .unwrap_or_default();
    let listed = v.list.as_ref().and_then(|l| l.entry(&k.hash));
    let current = current_keys(v.list.as_ref(), &k.hash);
    let requested = keys_view(k.keys(), network);
    let terms = ShareTermsView {
        service: listed
            .and_then(|e| e.service_address)
            .map(|a| a.to_string())
            .unwrap_or_default(),
        service_read: None,
        operator_public_key: requested.operator_public_key.clone(),
        voting_address: requested.voting_address.clone(),
        operator_reward: 0,
        early_period_blocks: mn.map_or(0, |mn| mn.early_period_blocks),
        early_penalty: mn.map_or(0, |mn| mn.early_penalty),
    };
    let funder = k.funded.is_some();
    let own_unsigned = unsigned(&k.mine, &k.sigs).next().is_some();
    let next = match stage {
        SharedStage::Signing if own_unsigned => SharedNext::Approve,
        SharedStage::Signing => SharedNext::Copy,
        SharedStage::Combined if funder => SharedNext::Send,
        // An approver holding every signature copies them to the funder.
        SharedStage::Combined => SharedNext::Copy,
        SharedStage::Sent => SharedNext::Wait,
        SharedStage::Stale if funder => SharedNext::Abandon,
        _ => SharedNext::Nothing,
    };
    let mut warnings = Vec::new();
    if requested.operator_public_key != k.base.operator_public_key {
        warnings.push(SharedWarning::OperatorReset);
    }
    warnings.push(SharedWarning::UsableByFunder);
    let fee = k.funded.as_ref().map(|f| f.fee);
    SharedSession {
        id: k.row.id.clone(),
        code: k
            .row
            .id
            .chars()
            .take(6)
            .collect::<String>()
            .to_ascii_uppercase(),
        message_code: fingerprint(&k.envelope(network).to_json()),
        kind: SharedKind::KeyChange,
        role: role_of(&k.row.role),
        stage,
        revision: 0,
        consent_hash: None,
        rows,
        terms,
        node_run_by: String::new(),
        fee,
        debit: fee,
        pro_tx_hash: Some(display_hex(&k.hash)),
        txid: k.row.txid.clone(),
        reserved: reserved_of(&k.row).iter().map(outpoint_string).collect(),
        next,
        warnings,
        approved_terms: None,
        base_keys: Some(k.base.clone()),
        key_change: Some(KeyChangeView {
            requested,
            current: current.map(|c| keys_view(c, network)),
            funder,
        }),
        actor_fee: None,
        created_at: k.row.created_at,
        updated_at: k.row.updated_at,
        last_error: k.row.last_error.clone(),
    }
}
