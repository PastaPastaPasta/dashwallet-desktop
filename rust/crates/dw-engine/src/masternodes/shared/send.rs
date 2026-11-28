//! What puts a session's bytes on the network (MG-04b §3.1 step 5, §6.3,
//! §9.3, slice 1b): the coordinator's registration, signed last (D24) and
//! saved with `out_at` before its hand-off (D17); its resend; and the exit
//! from an out session, a spend of one of its reserved coins (D26). Each
//! holds the wallet's coin fence from its coin read through its hand-off,
//! as MG-04's sends do, and hands off as `deliver` does: a `PreparedTx`
//! assembled from the signed bytes, through the dispatch fence.

use std::collections::HashSet;
use std::sync::Arc;

use dashcore::{OutPoint, ScriptBuf, Transaction, TxIn, TxOut};
use dw_appdb::AbandonOutcome;
use dw_protx::shared::session::Stage;
use dw_protx::shared::{ProRegTxV3, SpecialTx};

use super::super::MasternodesError;
use super::super::protx::build::{Context, SEQUENCE, to_sign};
use super::super::protx::charge_fee;
use super::super::protx::ops::{encode_inputs, error_code};
use super::super::shared_records::SharedStage;
use super::build::sign_own_inputs;
use super::recovery::{AbandonCoins, CHANGE_SCRIPT_LEN, abandon_log, latest, spend_state};
use super::store::{
    ChainView, Loaded, Recorded, contested, message_refusal, out_stage, role_of, stage_refusal,
};
use crate::coinjoin::{SIGHASH_ALL, sign_p2pkh_inputs};
use crate::coins::{CoinFence, now_secs};
use crate::masternodes::shared_records::AbandonSpendState;
use crate::platform::lease::Outcome;
use crate::send::{
    BroadcastOutcome, PreparedSummary, SendFailure, SpendCharge, TxDraft, attempt_outcome,
    l1_address,
};
use crate::{EngineError, NetworkSession, WalletId};

/// A refusal past `out_at` names the stage it reads (D17).
fn sent() -> MasternodesError {
    stage_refusal(SharedStage::Sent)
}

impl NetworkSession {
    /// Hands `tx`, saved with session `id` first, to the network (§6.3),
    /// as MG-04's `deliver` does but for a session row: reserved and charged
    /// as a `PreparedTx` (the fence is released there), through `TxDraft`'s
    /// fenced hand-off, the outcome's code kept as the row's `last_error`.
    /// `out_at` is never cleared: whoever holds the other signatures may
    /// have the bytes too (D17), so a failed first attempt is resent or
    /// abandoned, never discarded. `maybe_out`: a resend (MG-04 D7), whose
    /// row reads [`dw_appdb::RESEND_IN_PROGRESS`] from just before its
    /// hand-off until its outcome is written (MG-10 §4.4 change 4).
    /// `own_inputs`: this wallet's inputs of a registration others fund too.
    #[expect(clippy::too_many_arguments, reason = "one hand-off")]
    pub(super) async fn deliver_shared(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        tx: Transaction,
        summary: PreparedSummary,
        charge: SpendCharge,
        fence: CoinFence,
        maybe_out: bool,
        own_inputs: Option<Vec<OutPoint>>,
    ) -> Result<(), MasternodesError> {
        let draft = self.new_draft(wallet, HashSet::new(), false)?;
        if maybe_out {
            let (w, id) = (wallet.to_string(), id.to_string());
            if !self
                .appdb_op(move |db| db.mark_shared_session_resend_durable(&w, &id))
                .await?
            {
                return Err(MasternodesError::SharedSessionNotFound);
            }
        }
        let (outcome, _) = self
            .hand_off(&draft, tx, summary, charge, fence, maybe_out, own_inputs)
            .await;
        #[cfg(test)]
        super::super::protx::ops::crash_at(self, super::super::protx::ops::CrashPoint::HandedOff)?;
        let error = outcome.as_ref().err().map(error_code);
        let (w, id) = (wallet.to_string(), id.to_string());
        self.appdb_op(move |db| db.record_shared_session_attempt(&w, &id, error.as_deref()))
            .await?;
        self.masternodes.local_change();
        self.hub.pump.mark_balances(wallet);
        Ok(())
    }

    /// Hands an abandon spend off, its bytes and `abandon_log` entry saved
    /// first (MG-10 §4.4 change 4): as [`Self::deliver_shared`], but the
    /// attempt's outcome is folded into that entry with its code as
    /// `last_error`, synced, and returned. `first`: the spend's first
    /// hand-off, the only one whose definite refusal shows its bytes never
    /// left; an error before any hand-off is such a refusal. A replay is
    /// run as a resend of bytes that may be out.
    #[expect(clippy::too_many_arguments, reason = "one hand-off")]
    pub(super) async fn deliver_abandon(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        tx: Transaction,
        summary: PreparedSummary,
        charge: SpendCharge,
        fence: CoinFence,
        first: bool,
    ) -> Result<Outcome, MasternodesError> {
        let txid = tx.txid().to_string();
        let (result, pending) = match self.new_shared_abandon_draft(wallet) {
            Ok(draft) => {
                self.hand_off(&draft, tx, summary, charge, fence, !first, None)
                    .await
            }
            Err(e) => (Err(e), false),
        };
        #[cfg(test)]
        super::super::protx::ops::crash_at(self, super::super::protx::ops::CrashPoint::HandedOff)?;
        let outcome = match attempt_outcome(&result) {
            Outcome::MaybeSent if pending => Outcome::NotSent,
            outcome => outcome,
        };
        let code = result.as_ref().err().map(error_code);
        let logged = match outcome {
            Outcome::Sent => AbandonOutcome::Sent,
            Outcome::NotSent => AbandonOutcome::NotSent,
            Outcome::MaybeSent => AbandonOutcome::MaybeSent,
        };
        let (w, id) = (wallet.to_string(), id.to_string());
        let recorded = self
            .appdb_op(move |db| {
                db.record_shared_abandon_outcome_durable(
                    &w,
                    &id,
                    &txid,
                    logged,
                    code.as_deref(),
                    first,
                    now_secs(),
                )
            })
            .await?;
        self.masternodes.local_change();
        self.hub.pump.mark_balances(wallet);
        if !recorded {
            return Err(MasternodesError::SharedSessionNotFound);
        }
        match result {
            Err(e @ EngineError::Send(SendFailure::OutpointUnavailable(_))) => Err(e.into()),
            _ => Ok(outcome),
        }
    }

    /// The outcome, and whether nothing was handed off: the transaction
    /// is still pending under its lease (its sent mark failed).
    #[expect(clippy::too_many_arguments, reason = "one hand-off")]
    async fn hand_off(
        &self,
        draft: &Arc<TxDraft>,
        tx: Transaction,
        summary: PreparedSummary,
        charge: SpendCharge,
        fence: CoinFence,
        maybe_out: bool,
        own_inputs: Option<Vec<OutPoint>>,
    ) -> (Result<BroadcastOutcome, EngineError>, bool) {
        #[cfg(test)]
        if let Some(seam) = self.masternodes.protx_seam() {
            *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&seam.broadcaster));
        }
        let prepared = draft.assembled(tx, summary, charge, fence, maybe_out, own_inputs);
        let outcome = draft.broadcast_inner(Arc::clone(&prepared)).await;
        if maybe_out {
            prepared.refund_unadmitted();
        }
        (outcome, prepared.pending())
    }

    /// The summary a hand-off shows, read under `fence`: this wallet's
    /// inputs of `tx`, its fee, its change.
    pub(super) async fn shared_summary(
        &self,
        fence: &CoinFence,
        tx: &Transaction,
        fee: u64,
        change: Option<&dashcore::Address>,
    ) -> Result<PreparedSummary, MasternodesError> {
        let w = self.wallet(&fence.wallet_id()).await?;
        let coins = self.input_coins(fence, &w, tx, false).await?;
        let inputs: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
        Ok(self.summary_of(&w, tx, &inputs, fee, change).await?)
    }

    /// The coordinator's send (§3.1 step 5, D24): once every other party's
    /// inputs are signed, §4's checks and the D65 consent hash again, then
    /// its debit charged (D18), its inputs signed, the complete registration
    /// saved with `out_at` before the hand-off (D17), and handed off.
    pub(super) async fn send_registration(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        l: &Loaded,
        ctx: &Context,
        fence: CoinFence,
    ) -> Result<(), MasternodesError> {
        let rec = l.recorded.as_ref().ok_or_else(super::unrecorded)?;
        let own = self
            .reserved_inputs(wallet, &l.row, &rec.outpoints())
            .await?;
        let unspent = self.op_view(wallet).await?.unspent;
        let frozen = self.consent_checks(l, ctx, &unspent, true, None)?;
        let approved = l.approved.as_ref().map(|a| a.consent_hash.as_str());
        if approved != Some(ProRegTxV3::display_hash(&frozen.consent_hash).as_str()) {
            return Err(message_refusal("conflict"));
        }
        let mine: HashSet<OutPoint> = own.iter().map(|(o, _, _)| *o).collect();
        if frozen
            .tx
            .input
            .iter()
            .any(|i| !mine.contains(&i.previous_output) && i.script_sig.is_empty())
        {
            return Err(MasternodesError::SharedStage {
                stage: "unsigned".into(),
            });
        }
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, signer) = charge_fee(&lease, rec.debit())?;
            let signed = sign_own_inputs(&frozen.tx, &own, &signer).await?;
            let bytes = signed.to_bytes();
            let tx = typed(&signed)?;
            let txid = tx.txid().to_string();
            let (w, id, out) = (wallet.to_string(), l.row.id.clone(), bytes.clone());
            let saved = self
                .appdb_op(move |db| {
                    db.mark_shared_session_out_durable(&w, &id, &out, &txid, now_secs())
                })
                .await?;
            if !saved {
                return Err(sent());
            }
            #[cfg(test)]
            super::super::protx::ops::crash_at(
                self,
                super::super::protx::ops::CrashPoint::SharedOut,
            )?;
            // The Final Record (§2.3).
            let mut next = l.session.clone();
            next.protx = hex::encode(&bytes);
            next.stage = Stage::Broadcast;
            self.save_shared(wallet, l, &next, l.approved.as_ref())
                .await?;
            let change = change_of(rec, self.network.core_network());
            let summary = self
                .shared_summary(&fence, &tx, rec.fee.unwrap_or(0), change.as_ref())
                .await?;
            let own = Some(rec.outpoints());
            self.deliver_shared(wallet, &l.row.id, tx, summary, charge, fence, false, own)
                .await
        }
        .await;
        lease.end();
        result
    }

    /// A resend of the coordinator's saved registration (§7's "resend the
    /// same way"): the same bytes under a new lease charged the debit, since
    /// a first hand-off that never left was released and refunded; the
    /// fence refunds it again when the bytes may be out already (D71).
    /// Refused while another pending spend holds one of its inputs, as
    /// MG-04's resend.
    pub(super) async fn resend_registration(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        l: &Loaded,
        chain: &ChainView,
        fence: CoinFence,
    ) -> Result<(), MasternodesError> {
        let rec = l.recorded.as_ref().ok_or_else(super::unrecorded)?;
        let tx = self.registration_resend_checks(wallet, l, chain)?;
        let fee = rec.fee.unwrap_or(0);
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, _) = charge_fee(&lease, rec.debit())?;
            let change = change_of(rec, self.network.core_network());
            let summary = self
                .shared_summary(&fence, &tx, fee, change.as_ref())
                .await?;
            let own = Some(rec.outpoints());
            self.deliver_shared(wallet, &l.row.id, tx, summary, charge, fence, true, own)
                .await
        }
        .await;
        lease.end();
        result
    }

    /// The resend's checks before its lease: a spend of the session's coins
    /// in a block that may still become final refuses `shared_stage
    /// {conflicting}` ([`contested`], #954), and no other pending spend may
    /// hold an input of the saved registration (MG-04 D7). The saved bytes.
    pub(super) fn registration_resend_checks(
        &self,
        wallet: WalletId,
        l: &Loaded,
        chain: &ChainView,
    ) -> Result<Transaction, MasternodesError> {
        if contested(l, chain, &SharedStage::Sent) {
            return Err(MasternodesError::SharedStage {
                stage: "conflicting".into(),
            });
        }
        let tx: Transaction = l
            .row
            .signed_tx
            .as_deref()
            .and_then(|b| dashcore::consensus::deserialize(b).ok())
            .ok_or_else(|| EngineError::Internal("the saved registration".into()))?;
        self.check_resend_inputs(wallet, &tx)?;
        Ok(tx)
    }

    /// The exit from an out session (§9.3, D26; MG-10 §4.4 changes 2 and
    /// 4): every reserved coin the wallet holds unspent, spent to one fresh
    /// change address of its own (charged the relay minimum of that spend),
    /// only when [`super::recovery::AbandonCoins::Usable`], so it takes every
    /// input an earlier abandon spend took and conflicts with all of them.
    /// `abandon_txid`, its bytes and its `abandon_log` entry are saved
    /// before the hand-off. The row and its locks stay until a block
    /// settles the session (`confirmed`, or `conflicted` by this spend).
    /// Any arm but `Usable` and `TooSmall` refuses `shared_stage` naming the
    /// caller's stage while out (`sent` or `funding_signed`, review r1 of
    /// 1b n-2): a coordinator whose registration the wallet recorded holds
    /// none unspent until that transaction is abandoned (QT-091, as MG-04
    /// D49). `TooSmall` is `send.amount_with_fee_exceeds_balance`.
    pub(super) async fn abandon_session(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        row: &dw_appdb::SharedSessionRow,
        fence: CoinFence,
    ) -> Result<(), MasternodesError> {
        let chain = self.shared_chain(wallet).await?;
        let facts = self.coin_facts(wallet, row, &chain).await?;
        let (coins, fee) = match facts.test(&chain.history) {
            AbandonCoins::Usable { coins, fee } => (coins, fee),
            AbandonCoins::TooSmall { fee, available } => {
                return Err(EngineError::Send(SendFailure::AmountWithFeeExceedsBalance {
                    fee,
                    available,
                })
                .into());
            }
            _ => return Err(stage_refusal(out_stage(role_of(&row.role)))),
        };
        let w = self.wallet(&wallet).await?;
        let address = w
            .core()
            .next_change_address_for_account(0)
            .await
            .map_err(EngineError::from)?;
        let script = address.script_pubkey();
        if script.len() != CHANGE_SCRIPT_LEN {
            return Err(EngineError::Internal("a change script that isn't P2PKH".into()).into());
        }
        let available: u64 = coins.iter().map(|(u, _)| u.value()).sum();
        let mut tx = Transaction {
            version: 3,
            lock_time: 0,
            input: coins
                .iter()
                .map(|(u, _)| TxIn {
                    previous_output: u.outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: SEQUENCE,
                    witness: Default::default(),
                })
                .collect(),
            output: vec![TxOut {
                value: available - fee,
                script_pubkey: script,
            }],
            special_transaction_payload: None,
        };
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, signer) = charge_fee(&lease, fee)?;
            let inputs = to_sign(&tx, &coins)?;
            #[cfg(test)]
            let pause = self.hub.pause_shared_abandon_sign.lock().unwrap().take();
            #[cfg(test)]
            if let Some(pause) = pause {
                pause.mark_reached();
                pause.release.notified().await;
            }
            {
                let state = crate::wallet_read::info_of(&w).await?;
                let core = &state.core_wallet;
                let excluded = self.hub.recovery_exclusions(wallet, core);
                let available = crate::coins::read_coins(core);
                if tx.input.iter().any(|input| {
                    let coin = &input.previous_output;
                    excluded.contains_key(coin)
                        || !available.coins.iter().any(|c| c.utxo.outpoint == *coin)
                }) {
                    return Err(stage_refusal(out_stage(role_of(&row.role))));
                }
                sign_p2pkh_inputs(&signer, &mut tx, &inputs, SIGHASH_ALL).await?;
            }
            let txid = tx.txid().to_string();
            let (w_id, id) = (wallet.to_string(), row.id.clone());
            let outpoints: Vec<OutPoint> = coins.iter().map(|(u, _)| u.outpoint).collect();
            let (bytes, inputs) = (
                dashcore::consensus::serialize(&tx),
                encode_inputs(&outpoints),
            );
            let logged = self
                .appdb_op(move |db| {
                    db.set_shared_session_abandon_durable(
                        &w_id,
                        &id,
                        &txid,
                        &bytes,
                        fee,
                        &inputs,
                        now_secs(),
                    )
                })
                .await?
                .ok_or(MasternodesError::SharedSessionNotFound)?;
            let utxos: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
            let summary = self
                .summary_of(&w, &tx, &utxos, fee, Some(&address))
                .await?;
            self.deliver_abandon(wallet, &row.id, tx, summary, charge, fence, !logged)
                .await
                .map(drop)
        }
        .await;
        lease.end();
        result
    }

    /// `resend_shared_abandon`'s hand-off (MG-10 §4.4 change 4), once the
    /// caller refused a session settled or not out: the latest abandon
    /// spend's saved bytes again, charged its fee, refused while it can't
    /// be out or may be replaced (`shared_stage`: `sent` / `funding_signed`
    /// with none signed, `legacy` with no bytes kept, `abandon_cancelled`
    /// once abandoned in Activity, also while that Abandon's mark is stored
    /// before its sweep or can't be read, `abandon_refused` after a definite
    /// refusal), `legacy` too when neither its log entry nor the wallet's
    /// coins show its fee ([`Self::abandon_fee`]), `abandon_cancelled`
    /// again for a mark read once the coins are fenced, then while another
    /// pending spend holds one of its inputs. Its entry reads `maybe_sent`
    /// before the hand-off, which runs as a replay. Activity's Abandon takes
    /// neither the fence nor the shared claim: one that marks the spend
    /// after that last read doesn't stop the hand-off.
    pub(super) async fn resend_abandon(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        row: &dw_appdb::SharedSessionRow,
    ) -> Result<(), MasternodesError> {
        let Some(txid) = row.abandon_txid.clone() else {
            return Err(stage_refusal(out_stage(role_of(&row.role))));
        };
        let chain = self.shared_chain(wallet).await?;
        let log = abandon_log(row)?;
        let entry = latest(row, &log);
        let refusal = |stage: &str| MasternodesError::SharedStage {
            stage: stage.into(),
        };
        let Some(bytes) = row.abandon_tx.as_deref() else {
            return Err(refusal("legacy"));
        };
        match spend_state(row, entry, &chain.history) {
            Some(AbandonSpendState::AbandonedInActivity) => {
                return Err(refusal("abandon_cancelled"));
            }
            Some(AbandonSpendState::Refused) => return Err(refusal("abandon_refused")),
            _ if self.spend_back_marked(wallet, row).await => {
                return Err(refusal("abandon_cancelled"));
            }
            _ => {}
        }
        let tx: Transaction = dashcore::consensus::deserialize(bytes)
            .map_err(|_| EngineError::Internal("the saved abandon spend".into()))?;
        let Some(fee) = self.abandon_fee(wallet, row, entry, &chain.history).await? else {
            return Err(refusal("legacy"));
        };
        let fence = self.coin_fence(wallet).await?;
        if self.spend_back_marked(wallet, row).await {
            return Err(refusal("abandon_cancelled"));
        }
        self.check_resend_inputs(wallet, &tx)?;
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, _) = charge_fee(&lease, fee)?;
            let (w, id, marked) = (wallet.to_string(), row.id.clone(), txid.clone());
            if !self
                .appdb_op(move |db| {
                    db.mark_shared_abandon_replay_durable(&w, &id, &marked, now_secs())
                })
                .await?
            {
                return Err(MasternodesError::SharedSessionNotFound);
            }
            let change = tx.output.first().and_then(|o| {
                dashcore::Address::from_script(&o.script_pubkey, self.network.core_network()).ok()
            });
            let summary = self
                .shared_summary(&fence, &tx, fee, change.as_ref())
                .await?;
            self.deliver_abandon(wallet, &row.id, tx, summary, charge, fence, false)
                .await
                .map(drop)
        }
        .await;
        lease.end();
        result
    }
}

/// `signed` as the pin's typed transaction, which the hand-off and the
/// wallet's history take: it must read back to the same bytes, or the txid
/// the row saves wouldn't be the one that goes out.
pub(super) fn typed(signed: &SpecialTx) -> Result<Transaction, MasternodesError> {
    let bytes = signed.to_bytes();
    dashcore::consensus::deserialize::<Transaction>(&bytes)
        .ok()
        .filter(|tx| dashcore::consensus::serialize(tx) == bytes)
        .ok_or_else(|| {
            EngineError::Internal("the pin doesn't read back a shared transaction".into()).into()
        })
}

/// The change address a recorded contribution names.
fn change_of(rec: &Recorded, network: dashcore::Network) -> Option<dashcore::Address> {
    l1_address(&rec.change.as_ref()?.address, network).ok()
}
