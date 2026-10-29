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
use dw_protx::shared::session::Stage;
use dw_protx::shared::{ProRegTxV3, SpecialTx};
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;

use super::super::MasternodesError;
use super::super::protx::build::{Context, SEQUENCE, to_sign, tx_size};
use super::super::protx::charge_fee;
use super::super::protx::ops::error_code;
use super::super::shared_records::SharedStage;
use super::build::sign_own_inputs;
use super::store::{
    Loaded, Recorded, message_refusal, out_stage, reserved_of, role_of, stage_refusal,
};
use crate::coinjoin::{SIGHASH_ALL, sign_p2pkh_inputs};
use crate::coins::{CoinFence, now_secs, own_address_info, read_coins};
use crate::send::plan::dust_threshold;
use crate::send::plan::fee_for;
use crate::send::{MIN_FEE_PER_KB, PreparedSummary, SendFailure, SpendCharge, l1_address};
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
    /// abandoned, never discarded. `maybe_out`: a resend (MG-04 D7).
    #[expect(clippy::too_many_arguments, reason = "one hand-off")]
    async fn deliver_shared(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
        tx: Transaction,
        summary: PreparedSummary,
        charge: SpendCharge,
        fence: CoinFence,
        maybe_out: bool,
    ) -> Result<(), MasternodesError> {
        let draft = self.new_draft(wallet, HashSet::new())?;
        #[cfg(test)]
        if let Some(seam) = self.masternodes.protx_seam() {
            *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&seam.broadcaster));
        }
        let prepared = draft.assembled(tx, summary, charge, fence, maybe_out);
        let outcome = draft.broadcast_inner(Arc::clone(&prepared)).await;
        if maybe_out {
            prepared.refund_unadmitted();
        }
        let error = outcome.as_ref().err().map(error_code);
        let (w, id) = (wallet.to_string(), id.to_string());
        self.appdb_op(move |db| db.record_shared_session_attempt(&w, &id, error.as_deref()))
            .await?;
        self.masternodes.local_change();
        self.hub.pump.mark_balances(wallet);
        Ok(())
    }

    /// The summary a hand-off shows, read under `fence`: this wallet's
    /// inputs of `tx`, its fee, its change.
    async fn shared_summary(
        &self,
        fence: &CoinFence,
        tx: &Transaction,
        fee: u64,
        change: Option<&dashcore::Address>,
    ) -> Result<PreparedSummary, MasternodesError> {
        let w = self.wallet(&fence.wallet_id()).await?;
        let coins = self.input_coins(fence, &w, tx, false).await?;
        let inputs: Vec<_> = coins.into_iter().map(|(u, _)| u).collect();
        Ok(self.summary_of(&w, tx, &inputs, fee, change).await)
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
        let own = self.reserved_inputs(wallet, l, &rec.outpoints()).await?;
        let (_, unspent) = self.op_view(wallet).await?;
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
            self.deliver_shared(wallet, &l.row.id, tx, summary, charge, fence, false)
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
        fence: CoinFence,
    ) -> Result<(), MasternodesError> {
        let rec = l.recorded.as_ref().ok_or_else(super::unrecorded)?;
        let tx: Transaction = l
            .row
            .signed_tx
            .as_deref()
            .and_then(|b| dashcore::consensus::deserialize(b).ok())
            .ok_or_else(|| EngineError::Internal("the saved registration".into()))?;
        self.check_resend_inputs(wallet, &tx)?;
        let fee = rec.fee.unwrap_or(0);
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, _) = charge_fee(&lease, rec.debit())?;
            let change = change_of(rec, self.network.core_network());
            let summary = self
                .shared_summary(&fence, &tx, fee, change.as_ref())
                .await?;
            self.deliver_shared(wallet, &l.row.id, tx, summary, charge, fence, true)
                .await
        }
        .await;
        lease.end();
        result
    }

    /// The exit from an out session (§9.3, D26): the largest of its
    /// reserved coins the wallet still holds unspent and no other pending
    /// spend holds, spent whole to a
    /// fresh change address of its own (charged the fee), `abandon_txid`
    /// saved before the hand-off. The row and its locks stay until a block
    /// settles the session (`confirmed`, or `conflicted` by this spend).
    /// A coordinator whose registration the wallet recorded holds none
    /// unspent until that transaction is abandoned (QT-091, as MG-04 D49).
    /// With no such coin it refuses `shared_stage` naming the caller's
    /// stage while out (`sent` or `funding_signed`, review r1 of 1b n-2);
    /// a coin worth no more than the fee and dust is
    /// `send.amount_with_fee_exceeds_balance`.
    pub(super) async fn abandon_session(
        self: &Arc<Self>,
        wallet: WalletId,
        grant: &str,
        row: &dw_appdb::SharedSessionRow,
        fence: CoinFence,
    ) -> Result<(), MasternodesError> {
        let reserved = reserved_of(row);
        // Every pending spend's coins (an earlier abandon's among them).
        let pending = self.spends.snapshot(&wallet);
        let w = self.wallet(&wallet).await?;
        let coin = {
            let state = w.state().await;
            read_coins(&state.core_wallet)
                .coins
                .into_iter()
                .filter(|c| {
                    reserved.contains(&c.utxo.outpoint) && !pending.contains(&c.utxo.outpoint)
                })
                .max_by_key(|c| (c.utxo.value(), c.utxo.outpoint))
                .and_then(|c| {
                    let path = own_address_info(&state.core_wallet, &c.utxo.address)?.path;
                    Some((c.utxo, path))
                })
        };
        let Some((utxo, path)) = coin else {
            return Err(stage_refusal(out_stage(role_of(&row.role))));
        };
        let outpoint = utxo.outpoint;
        let address = w
            .core()
            .next_change_address_for_account(0)
            .await
            .map_err(EngineError::from)?;
        let script = address.script_pubkey();
        let fee = fee_for(FeeRate::new(MIN_FEE_PER_KB), tx_size(1, &[script.len()], 0))
            .ok_or(EngineError::Send(SendFailure::TxTooLarge))?;
        let value = utxo
            .value()
            .checked_sub(fee)
            .filter(|v| *v > dust_threshold(script.len()))
            .ok_or(EngineError::Send(
                SendFailure::AmountWithFeeExceedsBalance {
                    fee,
                    available: utxo.value(),
                },
            ))?;
        let mut tx = Transaction {
            version: 3,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: outpoint,
                script_sig: ScriptBuf::new(),
                sequence: SEQUENCE,
                witness: Default::default(),
            }],
            output: vec![TxOut {
                value,
                script_pubkey: script,
            }],
            special_transaction_payload: None,
        };
        let lease = self.shared_lease(wallet, grant).await?;
        let result = async {
            let (charge, signer) = charge_fee(&lease, fee)?;
            let coins = [(utxo.clone(), path)];
            let inputs = to_sign(&tx, &coins)?;
            sign_p2pkh_inputs(&signer, &mut tx, &inputs, SIGHASH_ALL).await?;
            let txid = tx.txid().to_string();
            let (w_id, id) = (wallet.to_string(), row.id.clone());
            let saved = self
                .appdb_op(move |db| {
                    db.set_shared_session_abandon_durable(&w_id, &id, &txid, now_secs())
                })
                .await?;
            if !saved {
                return Err(MasternodesError::SharedSessionNotFound);
            }
            let summary = self
                .summary_of(&w, &tx, std::slice::from_ref(&utxo), fee, Some(&address))
                .await;
            self.deliver_shared(wallet, &row.id, tx, summary, charge, fence, false)
                .await
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
