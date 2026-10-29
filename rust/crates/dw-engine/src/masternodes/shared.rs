//! Shared masternodes, slice 1a (MG-04b §7, mn-engine §2.4): v24's
//! multi-party registration, as dash-qt's session envelope runs it. This
//! wallet takes part as coordinator or participant up to its consent
//! signature; slice 1b adds the calls that produce bytes the network
//! accepts, each refusing while [`SHARED_SENDING`] is off (D1, DEC-678).
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
mod send;
mod store;
#[cfg(test)]
#[allow(non_snake_case)]
mod tests;
mod update;

use std::sync::Arc;

use dw_protx::shared::envelope::{SharedMessage as Message, decode_message, encode_base64};
use dw_protx::shared::session::funding::{Frozen, verify_consent_signature};
use dw_protx::shared::session::merge::add_signature;
use dw_protx::shared::session::{Session, Share, Stage, Terms};
use dw_protx::shared::{ProRegTxV3, SpecialTx};

use self::build::{ServiceProblem, coordinator_services, participant_services_ok};
use self::flow::{
    OwnRow, check_placeholder, check_reward, put_own_side, recorded_of, signer_refusal,
};
use self::store::{
    Approved, Loaded, bad_signature, envelope_refusal, message_name, message_refusal,
    stage_refusal, terms_view, view,
};
use super::protx::build::{Context, invalid, shared_voting};
use super::protx::ops::InFlight;
use super::protx_records::{OperatorChoice, ProTxOperation, VotingChoice};
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

/// The one switch for shared-masternode sending (DEC-678): on since slice
/// 1c closed D39 (every net-info entry of the list, D75) and M9 (the
/// dash-qt envelope oracle, D78). Turned off, `can_send` is false and every
/// call that produces bytes the network accepts (`sign_shared_funding`,
/// `send_shared_registration`, `abandon_shared_session`, `update_share`,
/// and `resend_operation` of an `update_share` row) refuses
/// `unsupported{feature: "shared_masternodes"}` after the wallet check.
pub(super) const SHARED_SENDING: bool = true;

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

impl Masternodes {
    /// Whether a call may produce bytes the network accepts:
    /// [`SHARED_SENDING`], or the test-support override.
    fn shared_can_send(&self) -> bool {
        #[cfg(any(test, all(debug_assertions, feature = "test-support")))]
        {
            self.shared_send
        }
        #[cfg(not(any(test, all(debug_assertions, feature = "test-support"))))]
        {
            SHARED_SENDING
        }
    }

    /// `unsupported{feature: "shared_masternodes"}` unless the call may
    /// produce bytes the network accepts.
    pub(super) fn require_sending(&self) -> Result<(), MasternodesError> {
        if self.shared_can_send() {
            Ok(())
        } else {
            Err(unsupported(None))
        }
    }
}

/// The sending override (r2-m4): compiled only in tests and in a debug
/// build with `test-support`, reached from other crates only through
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
            // The fence from the keys to the row's write (D63): another
            // creation call can't take the same owner key or coins.
            let fence = s.coin_fence(wallet).await?;
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
            let (selected, change) = s.shared_reserve(&fence, &id, &[], target, None).await?;
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
            #[cfg(test)]
            crate::coins::pause_at(&s.coin_fences.pause_row_write).await;
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
            let fence = s.coin_fence(wallet).await?;
            let (selected, change) = s
                .shared_reserve(&fence, &session, &rec.outpoints(), target, reuse.as_deref())
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
            require_v24(&s).await?;
            // The fence from the keys to the row's write (D63).
            let fence = s.coin_fence(wallet).await?;
            let mut ctx = s.protx_context(wallet).await?;
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
            let (selected, change) = s
                .shared_reserve(&fence, &session, &[], target, None)
                .await?;
            let rec = recorded_of(index, &own, &row, &selected, change, None);
            put_own_side(&mut next, &rec)?;
            next.revision += 1;
            next.check_labels().map_err(envelope_refusal)?;
            #[cfg(test)]
            crate::coins::pause_at(&s.coin_fences.pause_row_write).await;
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
        // The record's `consent_hash`: 64 hex, compared ignoring case.
        if consent_hash.len() != 64 || !consent_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(invalid("the consent hash is 64 hex digits"));
        }
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
            if !consent_hash.eq_ignore_ascii_case(&l.session.consent_hash) {
                return Err(message_refusal("conflict"));
            }
            let rec = l.recorded.clone().ok_or_else(unrecorded)?;
            // The fence from the reservation's re-check (D50) to the write.
            let _fence = s.coin_fence(wallet).await?;
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

    /// Participant (§3.2 step 5) †: `unsupported` while sending is off
    /// ([`SHARED_SENDING`]). Re-checks the reservation (D17) and §4,
    /// charges `I − C` (D18), signs only its own inputs (SIGHASH_ALL), saves
    /// the signed copy with `out_at` before returning the Signed
    /// Contribution, the coin fence held from the re-check to that write.
    pub async fn sign_shared_funding(
        &self,
        wallet: WalletId,
        session: String,
        grant: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        let sending = self.require_sending();
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            sending?;
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
            // The fence from the reservation's re-check to the `out_at`
            // write, as a send holds it from its coin read through signing.
            let _fence = s.coin_fence(wallet).await?;
            let own = s.reserved_inputs(wallet, &l, &rec.outpoints()).await?;
            let (_, unspent) = s.op_view(wallet).await?;
            let frozen = s.consent_checks(&l, &ctx, &unspent, true, None)?;
            // D56 at the funding step: the copy is the one this wallet
            // approved (D65).
            let approved = l.approved.as_ref().map(|a| a.consent_hash.as_str());
            if approved != Some(ProRegTxV3::display_hash(&frozen.consent_hash).as_str()) {
                return Err(message_refusal("conflict"));
            }
            s.sign_funding(wallet, &grant, &l, &frozen.tx, &own, rec.debit())
                .await?;
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// Coordinator (§3.1 step 5) †: `unsupported` while sending is off
    /// ([`SHARED_SENDING`]). At `combined`, once every other
    /// party's inputs are signed (`shared_stage{stage: "unsigned"}`
    /// otherwise), signs its own last (D24), charged its debit (D18), saves
    /// the complete registration with `out_at` and hands it off through the
    /// fence (§6.3). At `sent`, hands the saved bytes off again (a resend,
    /// charged the debit, D71). Returns the Final Record; the hand-off's failure is
    /// the session's `last_error`.
    pub async fn send_shared_registration(
        &self,
        wallet: WalletId,
        session: String,
        grant: String,
    ) -> Result<SharedOutcome, MasternodesError> {
        check_session_id(&session)?;
        let sending = self.require_sending();
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            sending?;
            let ctx = v24_context(&s, wallet).await?;
            let _flight = s.claim_shared_session(wallet, &session)?;
            let l = s.shared_loaded(wallet, &session).await?;
            if l.role != SharedRole::Coordinator {
                return Err(MasternodesError::SharedStage {
                    stage: store::role_name(l.role).into(),
                });
            }
            // The fence from the reservation's re-check through the
            // hand-off (§6.3).
            let fence = s.coin_fence(wallet).await?;
            match store::derive_stage(&l, &s.shared_chain(wallet).await?) {
                SharedStage::Combined => {
                    s.send_registration(wallet, &grant, &l, &ctx, fence).await?
                }
                // The user's exit stands: an abandoned registration isn't
                // handed off again (D72).
                SharedStage::Sent if l.row.abandon_txid.is_some() => {
                    return Err(MasternodesError::SharedStage {
                        stage: "abandoned".into(),
                    });
                }
                SharedStage::Sent => s.resend_registration(wallet, &grant, &l, fence).await?,
                stage => return Err(stage_refusal(stage)),
            }
            outcome(&s, wallet, &session).await
        })
        .await
    }

    /// The exit from a session that may be out (§9.3, D26) †: `unsupported`
    /// while sending is off ([`SHARED_SENDING`]). Spends the
    /// largest of its reserved coins the wallet holds unspent to a fresh
    /// address of its own under its own lease (charged the fee), through
    /// the fence, `abandon_txid` saved first. The row is kept and reads
    /// `conflicted` once that spend is in a block (`confirmed` if the
    /// registration won); its locks are released then (§11). A session not
    /// out is discarded instead (`shared_stage{stage}`), as is one already
    /// settled.
    pub async fn abandon_shared_session(
        &self,
        wallet: WalletId,
        session: String,
        grant: String,
    ) -> Result<SharedSession, MasternodesError> {
        check_session_id(&session)?;
        let sending = self.require_sending();
        self.protx_op(move |s| async move {
            s.require_wallet(&wallet)?;
            sending?;
            let _flight = s.claim_shared_session(wallet, &session)?;
            let row = s
                .shared_row(wallet, &session)
                .await?
                .ok_or(MasternodesError::SharedSessionNotFound)?;
            let stage = s.stored_stage(wallet, &row).await?;
            if row.out_at.is_none()
                || matches!(
                    stage,
                    SharedStage::Confirmed { .. } | SharedStage::Conflicted
                )
            {
                return Err(stage_refusal(stage));
            }
            let fence = s.coin_fence(wallet).await?;
            s.abandon_session(wallet, &grant, &row, fence).await?;
            s.shared_view(wallet, &session).await
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
                return Err(stage_refusal(s.stored_stage(wallet, &row).await?));
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
            check_reward(&request.reward, s.network.core_network())?;
            s.require_wallet(&wallet)?;
            let ctx = v24_context(&s, wallet).await?;
            let fence = s.coin_fence(wallet).await?;
            let plan = s
                .plan_share_update(wallet, &request, hash, &ctx, &fence, false)
                .await?;
            drop(fence);
            let reward_address = match &request.reward {
                RewardChoice::NewAddress => None,
                _ => Some(plan.mn_terms.payout_address.clone()).filter(|a| !a.is_empty()),
            };
            Ok(UpdateSharePreview {
                pro_tx_hash: display_hex(&hash),
                share_index: request.share_index,
                reward_address,
                reward_is_refund: plan.reward_script == plan.refund_script,
                fee: plan.funding.fee,
                total_debit: plan.funding.fee,
            })
        })
        .await
    }

    /// A reward change (§5.1, §6.3) †: `unsupported` while sending is off
    /// ([`SHARED_SENDING`]). [`Self::preview_update_share`]'s
    /// ProUpShareTx, signed by the share's keychain owner key and paid by
    /// the wallet's coins under one `SharedMasternode` lease charged the
    /// fee, then sent as an MG-04 operation of kind `update_share`: saved
    /// with its input locks before the hand-off, its outcome the
    /// operation's state. `masternodes.update_pending{operation}` while
    /// another reward change of the masternode may still be mined (D23).
    pub async fn update_share(
        &self,
        wallet: WalletId,
        request: UpdateShareRequest,
        grant: String,
    ) -> Result<ProTxOperation, MasternodesError> {
        let hash = parse_pro_tx_hash(&request.pro_tx_hash)?;
        let sending = self.require_sending();
        self.protx_op(move |s| async move {
            check_reward(&request.reward, s.network.core_network())?;
            s.require_wallet(&wallet)?;
            sending?;
            s.send_share_update(wallet, request, hash, &grant).await
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
