//! Shared masternodes, slice 1a (MG-04b §7, mn-engine §2.4): v24's
//! multi-party registration, as dash-qt's session envelope runs it. This
//! wallet takes part as coordinator or participant up to its consent
//! signature; nothing here produces bytes the network accepts while
//! `can_send` is false (D1), and the one call that would,
//! `sign_shared_funding`, refuses then.
//!
//! Every signing call begins its own `SharedMasternode` lease from a
//! `MasternodeOp` grant and signs with the vault's shared owner signer
//! (keychain owner keys only, D11); nothing signs as operator. Refusals
//! come in §7's order: arguments, `wallet_not_found`, `unsupported`, the
//! list, the session (its stage, the copy the caller reviewed, D56, and
//! its reservation, D17), the message, Core's rules, the consent checks,
//! the lease.

pub(super) use self::store::check_session_id;

mod build;
mod flow;
mod import;
mod store;
#[cfg(test)]
#[allow(non_snake_case)]
mod tests;

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::hashes::Hash;
use dashcore::{Address, OutPoint, ScriptBuf, TxIn, TxOut, Txid};
use dw_protx::shared::envelope::{SharedMessage as Message, decode_message, encode_base64};
use dw_protx::shared::rawtx::SPECIAL_VERSION;
use dw_protx::shared::rules::{self as shared_rules, ShareReward, SharedMn};
use dw_protx::shared::session::funding::{
    Frozen, decode_frozen, script_for, verify_consent_signature,
};
use dw_protx::shared::session::merge::add_signature;
use dw_protx::shared::session::{Session, Share, Stage, Terms};
use dw_protx::shared::{ProUpShareTx, SpecialTx, TRANSACTION_PROVIDER_UPDATE_SHARE, payload_bytes};

use self::build::{ServiceProblem, coordinator_services, participant_services_ok};
use self::flow::{
    OwnRow, check_placeholder, check_reward, put_own_side, recorded_of, signer_refusal,
};
use self::store::{
    Approved, Loaded, bad_signature, envelope_refusal, message_name, message_refusal,
    stage_refusal, terms_view, view,
};
use super::protx::CHANGE_LEN;
use super::protx::build::{
    Context, Funding, SEQUENCE, invalid, plan_funding, preview_script, refused, shared_voting,
};
use super::protx::ops::InFlight;
use super::protx_records::{OperatorChoice, VotingChoice};
use super::shared_records::{
    Activation, JoinSharedRequest, RewardChoice, SharedCapability, SharedImport, SharedMessage,
    SharedOutcome, SharedRole, SharedSession, SharedStage, SharedWarning, StartSharedRequest,
    UpdateSharePreview, UpdateShareRequest,
};
use super::{Masternodes, MasternodesError, display_hex, parse_pro_tx_hash};
use crate::platform::lease::Lease;
use crate::platform::{BudgetPurpose, FlowKind, PlatformError};
use crate::send::lease_failure;
use crate::{EngineError, NetworkSession, WalletId};

/// A joined or started session without its recorded side: a row this
/// version didn't write.
fn unrecorded() -> MasternodesError {
    EngineError::Internal("a shared session without its recorded contribution".into()).into()
}

/// `masternodes.unsupported{feature: "shared_masternodes"}`.
fn unsupported(field: Option<&str>) -> MasternodesError {
    MasternodesError::Unsupported {
        feature: "shared_masternodes".into(),
        field: field.map(Into::into),
    }
}

/// D3: creation needs v24 `active`.
async fn require_v24(s: &NetworkSession) -> Result<(), MasternodesError> {
    if s.v24_active().await? {
        Ok(())
    } else {
        Err(unsupported(None))
    }
}

/// The message to copy now for `l`'s session (§2.3's names by stage).
fn message_of(l: &Loaded) -> Result<SharedMessage, MasternodesError> {
    let session = &l.session;
    if l.role == SharedRole::Participant && l.recorded.is_none() && session.stage == Stage::Draft {
        return Err(stage_refusal(SharedStage::Invited));
    }
    let code = session.session_code();
    Ok(SharedMessage {
        text: session.to_text(),
        code: session.message_code(),
        name: message_name(session.stage, l.role).into(),
        file_name: format!("shared-mn-session-{code}.json"),
    })
}

/// The session record and its message after a write.
async fn outcome(
    s: &NetworkSession,
    wallet: WalletId,
    id: &str,
) -> Result<SharedOutcome, MasternodesError> {
    let l = s.shared_loaded(wallet, id).await?;
    let chain = s.shared_chain(wallet).await?;
    Ok(SharedOutcome {
        message: message_of(&l)?,
        session: view(&l, &chain),
    })
}

/// D3, then the list a call's rules run on.
async fn v24_context(s: &NetworkSession, wallet: WalletId) -> Result<Context, MasternodesError> {
    require_v24(s).await?;
    s.protx_context(wallet).await
}

/// The request's terms in the envelope's form, with the keys picked.
fn terms_of(request: &StartSharedRequest, operator: [u8; 48], voting: &str) -> Terms {
    let t = &request.terms;
    Terms {
        core_p2p_addrs: t.service.trim().to_string(),
        operator_pub_key: hex::encode(operator),
        voting_address: voting.to_string(),
        operator_reward: i32::from(t.operator_reward),
        early_period_blocks: t.early_period_blocks,
        early_penalty: i64::try_from(t.early_penalty).unwrap_or(i64::MAX),
    }
}

/// The ProUpShareTx spending `funding`, its change to a stand-in script:
/// `p` takes the inputs hash, the transaction its payload.
fn update_share_tx(p: &mut ProUpShareTx, funding: &Funding) -> SpecialTx {
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
                script_pubkey: preview_script(),
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

impl Masternodes {
    /// Whether a call may produce bytes the network accepts (D1): `false`
    /// at the pin; only the test-support constructor sets it.
    fn shared_can_send(&self) -> bool {
        #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
        {
            self.shared_send
        }
        #[cfg(not(any(test, all(debug_assertions, feature = "test-support"))))]
        {
            false
        }
    }
}

/// The capability override (r2-m4): compiled only in tests and in a debug
/// build with `test-support`, reached only through
/// `test_support::shared_masternodes_with_send_override`.
#[cfg(any(test, all(debug_assertions, feature = "test-support")))]
impl Masternodes {
    pub(crate) fn with_shared_send(session: &Arc<NetworkSession>, can_send: bool) -> Self {
        Self {
            session: Arc::clone(session),
            shared_send: can_send,
        }
    }
}

impl Masternodes {
    /// What this session can do with shared masternodes (D1, D3, D4).
    pub async fn shared_capability(
        &self,
        wallet: WalletId,
    ) -> Result<SharedCapability, MasternodesError> {
        let can_send = self.shared_can_send();
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            Ok(SharedCapability {
                v24: if s.v24_active().await? {
                    Activation::Active
                } else {
                    Activation::Unknown
                },
                evonode_shares: Activation::Inactive,
                can_send,
            })
        })
        .await
    }

    /// The wallet's sessions, newest first, each stage derived now (§9.2).
    pub async fn shared_sessions(
        &self,
        wallet: WalletId,
    ) -> Result<Vec<SharedSession>, MasternodesError> {
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let rows = s.shared_rows(wallet).await?;
            let chain = s.shared_chain(wallet).await?;
            let network = s.network.core_network();
            // A row that doesn't parse is skipped, as every other reader
            // does; `shared_session` of its id reports it.
            Ok(flow::loaded_all(rows, network)
                .iter()
                .map(|l| view(l, &chain))
                .collect())
        })
        .await
    }

    /// One session of the wallet.
    pub async fn shared_session(
        &self,
        wallet: WalletId,
        session: String,
    ) -> Result<SharedSession, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            s.shared_view(wallet, &session).await
        })
        .await
    }

    /// Coordinator (§3.1 step 1): checks the request (§5, D5, D28), picks
    /// this wallet's keys (D10), reserves share + fee with persistent locks
    /// (D9), records its contribution and saves the draft. Returns the
    /// session and its Invitation.
    pub async fn start_shared(
        &self,
        wallet: WalletId,
        request: StartSharedRequest,
    ) -> Result<SharedOutcome, MasternodesError> {
        build::check_slots(request.shares.len())?;
        let service = coordinator_services(&request.terms.service);
        if service == Err(ServiceProblem::Invalid) {
            return Err(invalid("the service is one or more IPv4 a.b.c.d:port"));
        }
        build::check_fee(request.fee)?;
        if request.terms.early_penalty > i64::MAX as u64 {
            return Err(invalid("the penalty is out of range"));
        }
        self.protx_op(move |s| async move {
            let network = s.network.core_network();
            check_reward(&request.reward, network)?;
            if let VotingChoice::Address { address } = &request.terms.voting
                && super::protx::keys::p2pkh_id(address, network).is_none()
            {
                return Err(invalid("the voting key is not a P2PKH address"));
            }
            s.require_wallet(&wallet)?;
            require_v24(&s).await?;
            if service.is_err() {
                return Err(unsupported(Some("service")));
            }
            let mut ctx = s.protx_context(wallet).await?;

            // The draft's table and terms, labels first (D28).
            let id = hex::encode(rand::random::<[u8; 32]>());
            let mut session = Session::new(network, id.clone());
            session.shares = request
                .shares
                .iter()
                .map(|slot| Share {
                    label: slot.label.clone(),
                    amount: i64::try_from(slot.amount).unwrap_or(i64::MAX),
                    ..Share::default()
                })
                .collect();
            session.check_labels().map_err(envelope_refusal)?;

            // Keys (D10): the owner first, so voting never takes it.
            let own = s.own_row(wallet, &mut ctx.used, &request.reward).await?;
            let voting = s
                .pick_voting(wallet, &request.terms.voting, &ctx.used)
                .await?;
            if voting.address == own.owner_address {
                return Err(shared_voting());
            }
            let operator: OperatorChoice = request.terms.operator.clone();
            let operator = s.pick_operator(wallet, &operator, &ctx.used).await?;
            session.terms = terms_of(&request, operator.public_key, &voting.address);
            session.operator_secret_holder = request.node_run_by.clone().unwrap_or_default();
            session.coordinator_label = session.shares[0].label.clone();
            own.fill(&mut session.shares[0]);

            // Core's rules on the terms, stand-ins in the empty rows (§5).
            check_placeholder(&session, &ctx, network)?;

            // The reservation: share + fee, under the fence (D9).
            let target = request.shares[0].amount.saturating_add(request.fee);
            let (fence, selected, change) = s.shared_reserve(wallet, &[], target, None).await?;
            let rec = recorded_of(
                0,
                &own,
                &session.shares[0],
                &selected,
                change,
                Some(request.fee),
            );
            put_own_side(&mut session, &rec)?;
            let reserved = rec.outpoints();
            s.insert_shared(
                wallet,
                &session,
                SharedRole::Coordinator,
                Some(&rec),
                &reserved,
                None,
            )
            .await?;
            drop(fence);
            outcome(&s, wallet, &id).await
        })
        .await
    }

    /// Coordinator, `draft`, before any other party's contribution (D33):
    /// its contribution re-made for share + `fee`, its coins re-reserved
    /// in the same write, `revision` + 1.
    pub async fn set_shared_fee(
        &self,
        wallet: WalletId,
        session: String,
        fee: u64,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        build::check_fee(fee)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Coordinator,
                    SharedStage::Draft,
                )
                .await?;
            let rec = l
                .recorded
                .clone()
                .ok_or_else(|| EngineError::Internal("a coordinator without its side".into()))?;
            if l.session.contributions.iter().any(|c| c.label != rec.label) {
                return Err(MasternodesError::SharedStage {
                    stage: "replied".into(),
                });
            }
            let target = rec.amount.saturating_add(fee);
            let reuse = rec.change.as_ref().map(|c| c.address.clone());
            let (fence, selected, change) = s
                .shared_reserve(wallet, &rec.outpoints(), target, reuse.as_deref())
                .await?;
            let new = recorded_of(
                rec.share_index,
                &OwnRow::from(&rec),
                &rec.row(),
                &selected,
                change,
                Some(fee),
            );
            let mut next = l.session.clone();
            put_own_side(&mut next, &new)?;
            next.revision += 1;
            s.replace_contribution(wallet, &l, &next, &new).await?;
            drop(fence);
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Detects a pasted or opened message (§2.1) and applies it by §3.0's
    /// table. Signs nothing; never repeats the text in a refusal (MG-00
    /// D36). `target`: the session a bare signatures array is for (part 2:
    /// in 1a such arrays, sigs envelopes and standbys are `unsupported`).
    pub async fn import_shared_message(
        &self,
        wallet: WalletId,
        text: String,
        target: Option<String>,
    ) -> Result<SharedImport, MasternodesError> {
        if let Some(t) = &target {
            check_session_id(t)?;
        }
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let network = s.network.core_network();
            let (theirs, received) = match decode_message(&text, network) {
                Ok(Message::Session { session, received }) => (*session, received),
                Ok(Message::BareSigs(_)) if target.is_none() => {
                    return Err(message_refusal("parse"));
                }
                // Maintenance and standbys are part 2.
                Ok(Message::BareSigs(_) | Message::Sigs(_) | Message::Standby(_)) => {
                    return Err(unsupported(None));
                }
                Err(e) => return Err(envelope_refusal(e)),
            };
            let id = theirs.session_id.clone();
            let _flight = s.claim_shared_session(wallet, &id)?;
            let ours = s
                .shared_row(wallet, &id)
                .await?
                .map(|r| Loaded::of(r, network))
                .transpose()?;
            let changed = match &ours {
                None => {
                    let new = import::first(&theirs)?;
                    s.insert_shared(wallet, &new, SharedRole::Participant, None, &[], None)
                        .await?;
                    true
                }
                Some(l) => match import::apply(l, &theirs)? {
                    Some(new) => {
                        s.save_shared(wallet, l, &new, l.approved.as_ref()).await?;
                        true
                    }
                    None => false,
                },
            };
            let session = s.shared_view(wallet, &id).await?;
            Ok(SharedImport::Session {
                session,
                changed,
                received_code: received.code,
                warnings: if received.edited {
                    vec![SharedWarning::EditedMessage]
                } else {
                    Vec::new()
                },
            })
        })
        .await
    }

    /// Participant (§3.2 step 2): fills its row (D10), reserves exactly its
    /// share (D9), records its contribution, returns the Details
    /// (`revision` + 1).
    pub async fn join_shared(
        &self,
        wallet: WalletId,
        session: String,
        request: JoinSharedRequest,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            let network = s.network.core_network();
            check_reward(&request.reward, network)?;
            s.require_wallet(&wallet)?;
            let mut ctx = v24_context(&s, wallet).await?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Participant,
                    SharedStage::Invited,
                )
                .await?;
            if !participant_services_ok(&l.session.terms.core_p2p_addrs) {
                return Err(unsupported(Some("service")));
            }
            // `inferMyShare`: the requested row, else the single empty one.
            let empty: Vec<usize> = (l.session.shares.iter().enumerate())
                .filter(|(_, r)| r.owner_address.is_empty())
                .map(|(i, _)| i)
                .collect();
            let index = match request.share_index {
                Some(i) => usize::from(i),
                None if empty.len() == 1 => empty[0],
                None => return Err(invalid("name the share to join")),
            };
            if !empty.contains(&index) {
                return Err(invalid("that share is not open"));
            }
            let mut next = l.session.clone();
            // Core's rules on the terms with this row as it would be.
            let own = s.own_row(wallet, &mut ctx.used, &request.reward).await?;
            own.fill(&mut next.shares[index]);
            check_placeholder(&next, &ctx, network)?;

            let row = next.shares[index].clone();
            let target = u64::try_from(row.amount).unwrap_or(0);
            let (fence, selected, change) = s.shared_reserve(wallet, &[], target, None).await?;
            let rec = recorded_of(index, &own, &row, &selected, change, None);
            put_own_side(&mut next, &rec)?;
            next.revision += 1;
            next.check_labels().map_err(envelope_refusal)?;
            s.replace_contribution(wallet, &l, &next, &rec).await?;
            drop(fence);
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// The message to copy or save now: its text, code, name and Core's
    /// file name `shared-mn-session-<CODE>.json`.
    pub async fn shared_message(
        &self,
        wallet: WalletId,
        session: String,
    ) -> Result<SharedMessage, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let l = s.shared_loaded(wallet, &session).await?;
            message_of(&l)
        })
        .await
    }

    /// Coordinator (§3.1 step 3): builds the registration (§2.4), runs
    /// Core's rules, the fee floor (D26) and its own consent checks (§4),
    /// signs its consent under a `SharedMasternode` lease (charged 0) and
    /// returns Locked Terms, `revision` unchanged. `revision` is the
    /// session record's the coordinator reviewed: another one (a reply
    /// absorbed since) is `shared_message{conflict}` (D56).
    pub async fn lock_shared_terms(
        &self,
        wallet: WalletId,
        session: String,
        revision: u32,
        grant: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let ctx = v24_context(&s, wallet).await?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Coordinator,
                    SharedStage::Draft,
                )
                .await?;
            if i64::from(l.session.revision) != i64::from(revision) {
                return Err(message_refusal("conflict"));
            }
            let complete = l.session.shares.iter().all(|r| {
                !r.owner_address.is_empty()
                    && !r.refund_address.is_empty()
                    && l.session.contributions.iter().any(|c| c.label == r.label)
            });
            if !complete {
                return Err(MasternodesError::SharedStage {
                    stage: "incomplete".into(),
                });
            }
            let rec = l.recorded.clone().ok_or_else(unrecorded)?;
            s.reserved_inputs(wallet, &l, &rec.outpoints()).await?;
            let mut frozen_session = l.session.clone();
            build::freeze(&mut frozen_session)?;
            let locked = Loaded {
                session: frozen_session,
                ..l.clone()
            };
            let (_, unspent) = s.op_view(wallet).await?;
            let frozen = s.consent_checks(&locked, &ctx, &unspent, false, Some(&rec))?;
            s.consent_and_save(wallet, &grant, &l, locked.session, &frozen, &rec)
                .await?;
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Coordinator: back to `draft` before `combined` (§3.1), `revision`
    /// + 1, every approval discarded.
    pub async fn unlock_shared_terms(
        &self,
        wallet: WalletId,
        session: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Coordinator,
                    SharedStage::Signing,
                )
                .await?;
            let mut next = l.session.clone();
            build::unfreeze(&mut next);
            s.save_shared(wallet, &l, &next, None).await?;
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Participant (§3.2 step 4): §4's checks on the locked registration,
    /// then its consent signature under a `SharedMasternode` lease
    /// (charged 0). Returns the Approval, `revision` unchanged; an approved
    /// session returns its Approval again without signing. `consent_hash`
    /// is the session record's the participant reviewed: another one (a
    /// re-locked copy imported since) is `shared_message{conflict}` (D56).
    pub async fn approve_shared(
        &self,
        wallet: WalletId,
        session: String,
        consent_hash: String,
        grant: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let ctx = v24_context(&s, wallet).await?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Participant,
                    SharedStage::Signing,
                )
                .await?;
            if consent_hash != l.session.consent_hash {
                return Err(message_refusal("conflict"));
            }
            let rec = l.recorded.clone().ok_or_else(unrecorded)?;
            s.reserved_inputs(wallet, &l, &rec.outpoints()).await?;
            if l.own_approval() {
                return outcome(&s, wallet, &session).await;
            }
            if !participant_services_ok(&l.session.terms.core_p2p_addrs) {
                return Err(unsupported(Some("service")));
            }
            let (_, unspent) = s.op_view(wallet).await?;
            let frozen = s.consent_checks(&l, &ctx, &unspent, false, None)?;
            s.consent_and_save(wallet, &grant, &l, l.session.clone(), &frozen, &rec)
                .await?;
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Participant (§3.2 step 5) †: refuses
    /// `unsupported{feature: "shared_masternodes"}` while `can_send` is
    /// false (D1). Otherwise re-checks the reservation (D17) and §4, charges
    /// `I − C` (D18), signs only its own inputs (SIGHASH_ALL), saves the
    /// signed copy with `out_at` before returning the Signed Contribution.
    pub async fn sign_shared_funding(
        &self,
        wallet: WalletId,
        session: String,
        grant: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        let can_send = self.shared_can_send();
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            if !can_send {
                return Err(unsupported(None));
            }
            let ctx = v24_context(&s, wallet).await?;
            let (_flight, l) = s
                .claim_loaded(
                    wallet,
                    &session,
                    SharedRole::Participant,
                    SharedStage::Combined,
                )
                .await?;
            let rec = l.recorded.clone().ok_or_else(unrecorded)?;
            let own = s.reserved_inputs(wallet, &l, &rec.outpoints()).await?;
            let (_, unspent) = s.op_view(wallet).await?;
            let frozen = s.consent_checks(&l, &ctx, &unspent, true, None)?;
            s.sign_funding(wallet, &grant, &l, &frozen.tx, &own, rec.debit())
                .await?;
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Deletes a session nothing of which may be out (D17) and unlocks its
    /// coins; one that may be out is `shared_stage{stage}`. Both are read
    /// from the stored row, so a session whose envelope no longer parses
    /// can still be discarded (D57).
    pub async fn discard_shared_session(
        &self,
        wallet: WalletId,
        session: String,
    ) -> Result<(), MasternodesError> {
        check_session_id(&session)?;
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            let _flight = s.claim_shared_session(wallet, &session)?;
            let row = s
                .shared_row(wallet, &session)
                .await?
                .ok_or(MasternodesError::SharedSessionNotFound)?;
            if row.out_at.is_some() {
                let role = store::role_of(&row.role);
                let stage = match Loaded::of(row, s.network.core_network()) {
                    Ok(l) => store::derive_stage(&l, &s.shared_chain(wallet).await?),
                    Err(_) => store::out_stage(role),
                };
                return Err(stage_refusal(stage));
            }
            let (w, id) = (wallet.to_string(), session.clone());
            let deleted = s
                .appdb_op(move |db| {
                    db.delete_unsent_shared_session_durable(&w, &id, crate::coins::now_secs())
                })
                .await?;
            if !deleted {
                return Err(MasternodesError::SharedSessionNotFound);
            }
            s.masternodes.local_change();
            s.hub.pump.mark_balances(wallet);
            Ok(())
        })
        .await
    }

    /// The ProUpShareTx this wallet would build for one of its shares
    /// (§5.1, D29, D31): from the registration of a confirmed session of
    /// the wallet, Core's rules run on it. Issues, locks and signs nothing.
    pub async fn preview_update_share(
        &self,
        wallet: WalletId,
        request: UpdateShareRequest,
    ) -> Result<UpdateSharePreview, MasternodesError> {
        let hash = parse_pro_tx_hash(&request.pro_tx_hash)?;
        self.protx_op(move |s| async move {
            let network = s.network.core_network();
            check_reward(&request.reward, network)?;
            s.require_wallet(&wallet)?;
            let ctx = v24_context(&s, wallet).await?;
            let txid = Txid::from_byte_array(hash);
            let (l, height) = s.shared_confirmed(wallet, txid).await?;
            // One block deep, and in the synced list (m-16).
            let listed = ctx.entries.iter().find(|e| e.pro_tx_hash == hash);
            if ctx.list_height.is_none_or(|h| h <= height) || listed.is_none() {
                return Err(MasternodesError::SharedTooEarly);
            }
            let index = usize::from(request.share_index);
            if l.recorded.as_ref().map(|r| r.share_index) != Some(index) {
                return Err(MasternodesError::ShareNotMine);
            }
            let frozen = decode_frozen(&l.session).map_err(envelope_refusal)?;
            let share = frozen
                .payload
                .shares
                .get(index)
                .ok_or(MasternodesError::ShareNotMine)?;
            let reward = match &request.reward {
                RewardChoice::Refund => ShareReward::Refund,
                RewardChoice::NewAddress => ShareReward::Script(preview_script()),
                RewardChoice::Address { address } => {
                    ShareReward::Script(script_for(address.trim(), network))
                }
            };
            let reward_script = reward.script(share);
            let voting_key_id = listed.map_or(frozen.payload.voting_key_id, |e| e.voting_key_id);
            let mut p = ProUpShareTx {
                version: ProUpShareTx::CURRENT_VERSION,
                pro_tx_hash: txid,
                share_index: request.share_index,
                reward_script: reward_script.clone(),
                inputs_hash: [0; 32],
                sig: vec![0; 65],
            };
            let fence = s.coin_fence(wallet).await?;
            let w = s.wallet(&wallet).await?;
            let coins = s.protx_candidates(&fence, &w, &HashSet::new()).await?;
            drop(fence);
            let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
            let funding = plan_funding(&utxos, &[], payload_bytes(&p).len(), CHANGE_LEN)
                .map_err(EngineError::from)?;
            let tx = update_share_tx(&mut p, &funding);
            let mn = SharedMn {
                pro_tx_hash: txid,
                collateral: OutPoint::new(txid, frozen.payload.collateral.vout),
                shares: &frozen.payload.shares,
                registered_height: height,
                early_period_blocks: frozen.payload.early_period_blocks,
                early_penalty: frozen.payload.early_penalty,
                voting_key_id,
            };
            shared_rules::check_update_share(&tx, &p, Some(&mn), true, false).map_err(refused)?;
            let reward_address = match &request.reward {
                RewardChoice::NewAddress => None,
                _ => Address::from_script(&reward_script, network)
                    .ok()
                    .map(|a| a.to_string()),
            };
            Ok(UpdateSharePreview {
                pro_tx_hash: display_hex(&hash),
                share_index: request.share_index,
                reward_address,
                reward_is_refund: reward_script == share.refund_script,
                fee: funding.fee,
                total_debit: funding.fee,
            })
        })
        .await
    }
}

impl NetworkSession {
    /// What a call that signs or changes a stored session begins with, in
    /// §7's order: the session marked in flight, read back, the caller's
    /// role, and the stage it must be at.
    async fn claim_loaded(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        role: SharedRole,
        stage: SharedStage,
    ) -> Result<(InFlight, Loaded), MasternodesError> {
        let flight = self.claim_shared_session(wallet, id)?;
        let l = self.shared_loaded(wallet, id).await?;
        if l.role != role {
            return Err(MasternodesError::SharedStage {
                stage: store::role_name(l.role).into(),
            });
        }
        let chain = self.shared_chain(wallet).await?;
        let current = store::derive_stage(&l, &chain);
        if current != stage {
            return Err(stage_refusal(current));
        }
        Ok((flight, l))
    }

    /// A `SharedMasternode` lease on `grant`.
    async fn shared_lease(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
    ) -> Result<Lease, MasternodesError> {
        Ok(self
            .begin_lease(wallet, FlowKind::SharedMasternode, &[grant.to_string()])
            .await
            .map_err(PlatformError::from)?)
    }

    /// [`Self::sign_consent`] on `next` (a copy of the registration `l`
    /// holds), saved over `l` with the terms it now approves.
    async fn consent_and_save(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        l: &Loaded,
        mut next: Session,
        frozen: &Frozen,
        rec: &store::Recorded,
    ) -> Result<(), MasternodesError> {
        self.sign_consent(wallet, grant, &mut next, frozen, rec)
            .await?;
        let approved = Approved {
            terms: terms_view(&next),
            consent_hash: next.consent_hash.clone(),
        };
        self.save_shared(wallet, l, &next, Some(&approved)).await
    }

    /// This wallet's consent signature on `session`'s frozen registration
    /// (§4) under its own `SharedMasternode` lease, charged 0 (§6.3): the
    /// vault's shared signer by the share's keychain owner key, checked
    /// before it is added.
    async fn sign_consent(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        session: &mut Session,
        frozen: &Frozen,
        rec: &store::Recorded,
    ) -> Result<(), MasternodesError> {
        let lease = self.shared_lease(wallet, grant).await?;
        let result = (|| {
            let (_, signer) = lease
                .charge_tx_signer(BudgetPurpose::Masternode, 0)
                .map_err(|e| lease_failure(e, 0))?;
            let sig = signer
                .sign_shared_consent(rec.owner_index, &frozen.tx.to_bytes())
                .map_err(signer_refusal)?;
            let b64 = encode_base64(&sig);
            let share = rec.share_index as i32;
            if !verify_consent_signature(frozen, share, &b64) {
                return Err(MasternodesError::ShareNotMine);
            }
            add_signature(session, frozen, share, &b64)
                .map_err(|_| bad_signature(Some(share as u16)))?;
            Ok(())
        })();
        lease.end();
        result
    }

    /// The participant's funding signatures (D17, D18): `debit` charged to
    /// the lease's masternode budget before signing, this wallet's inputs
    /// signed, the signed copy saved with `out_at` before anything is
    /// returned, then the envelope.
    async fn sign_funding(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        l: &Loaded,
        tx: &SpecialTx,
        own: &[build::OwnInput],
        debit: u64,
    ) -> Result<(), MasternodesError> {
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, signer) = lease
                .charge_tx_signer(BudgetPurpose::Masternode, debit)
                .map_err(|e| lease_failure(e, debit))?;
            let signed = match build::sign_own_inputs(tx, own, &signer).await {
                Ok(signed) => signed,
                Err(e) => {
                    lease.refund(BudgetPurpose::Masternode, charge);
                    return Err(e);
                }
            };
            let bytes = signed.to_bytes();
            let txid = signed.txid().to_string();
            let (w, id) = (wallet.to_string(), l.row.id.clone());
            let out = self
                .appdb_op(move |db| {
                    db.mark_shared_session_out_durable(
                        &w,
                        &id,
                        &bytes,
                        &txid,
                        crate::coins::now_secs(),
                    )
                })
                .await;
            // Nothing saved, so nothing returned: the charge goes back.
            let saved = match out {
                Ok(true) => Ok(()),
                Ok(false) => Err(MasternodesError::SharedStage {
                    stage: "funding_signed".into(),
                }),
                Err(e) => Err(e.into()),
            };
            if let Err(e) = saved {
                lease.refund(BudgetPurpose::Masternode, charge);
                return Err(e);
            }
            #[cfg(test)]
            super::protx::ops::crash_at(self, super::protx::ops::CrashPoint::SharedOut)?;
            let mut next = l.session.clone();
            next.protx = hex::encode(signed.to_bytes());
            if signed.input.iter().all(|i| !i.script_sig.is_empty()) {
                next.stage = next.stage.max(Stage::FundingSigned);
            }
            self.save_shared(wallet, l, &next, l.approved.as_ref())
                .await
        }
        .await;
        lease.end();
        result
    }
}
