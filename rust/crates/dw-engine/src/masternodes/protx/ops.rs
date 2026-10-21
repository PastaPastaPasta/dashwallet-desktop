//! ProTx operations (MG-04 §5): the `masternode_ops` rows, their state
//! derived on every read, the in-flight mark, and the one hand-off every
//! signing call and resend takes: the row saved durably with its locks,
//! the attempt recorded durably, the `PreparedTx` through the dispatch
//! fence, the outcome recorded.
//!
//! A row whose bytes may be out is never deleted (D6): the only delete is
//! `discard_operation`'s `... WHERE sent_at IS NULL`, and `sent_at` is
//! durable before each hand-off. A row's inputs stay persistently locked
//! from its save until its transaction is in a block or conflicted, or the
//! row is discarded (D8, D25), so no build after a restart can spend them
//! in a second transaction.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::{OutPoint, Transaction, Txid};
use dw_appdb::{AttemptEnd, MasternodeOpRow};

use super::super::MasternodesError;
use super::super::protx_records::{
    ProTxOperation, ProTxOperationKind, ProTxOperationState, ProTxTerms, SignRequest,
};
use crate::coins::{CoinFence, now_secs, read_coins};
use crate::history::TxEntry;
use crate::send::{PreparedSummary, SpendCharge};
use crate::{EngineError, NetworkSession, WalletId};

/// `masternodes.collateral_released:<outpoint>`, wallet-scoped: the user
/// released this collateral, so the follower doesn't lock it again (D9).
pub(crate) const RELEASED: &str = "masternodes.collateral_released:";

pub(crate) fn new_id() -> String {
    hex::encode(rand::random::<[u8; 16]>())
}

pub(crate) fn kind_name(kind: ProTxOperationKind) -> &'static str {
    match kind {
        ProTxOperationKind::Registration => "registration",
        ProTxOperationKind::UpdateRegistrar => "update_registrar",
    }
}

fn kind_of(name: &str) -> ProTxOperationKind {
    match name {
        "update_registrar" => ProTxOperationKind::UpdateRegistrar,
        _ => ProTxOperationKind::Registration,
    }
}

/// The snake-case name of a state, as `OperationState{state}` carries it.
pub(crate) fn state_name(state: ProTxOperationState) -> &'static str {
    match state {
        ProTxOperationState::NeedsSignature => "needs_signature",
        ProTxOperationState::InputsSpent => "inputs_spent",
        ProTxOperationState::Signed => "signed",
        ProTxOperationState::NotSent => "not_sent",
        ProTxOperationState::Sent => "sent",
        ProTxOperationState::Confirmed { .. } => "confirmed",
        ProTxOperationState::Conflicted => "conflicted",
    }
}

/// `fee_inputs`: a JSON array of `"txid:vout"`.
/// The code `last_error` keeps (§6): a send failure's `send.*` wire code
/// (the strings of dw-ffi's and dw-app's `SendCode`), else the engine
/// error's.
pub(crate) fn error_code(e: &EngineError) -> String {
    use crate::send::SendFailure as F;
    let EngineError::Send(f) = e else {
        return e.code().to_string();
    };
    let name = match f {
        F::NoRecipients => "no_recipients",
        F::InvalidAddress { .. } => "invalid_address",
        F::PlatformAddress { .. } => "platform_address",
        F::InvalidAmount { .. } => "invalid_amount",
        F::DustAmount { .. } => "dust_amount",
        F::DuplicateAddress { .. } => "duplicate_address",
        F::AmountExceedsBalance { .. } => "amount_exceeds_balance",
        F::AmountWithFeeExceedsBalance { .. } => "amount_with_fee_exceeds_balance",
        F::AmountTooSmallAfterFee { .. } => "amount_too_small_after_fee",
        F::InsufficientMixedFunds { .. } => "insufficient_mixed_funds",
        F::OutpointUnavailable(_) => "outpoint_unavailable",
        F::AbsurdFee { .. } => "absurd_fee",
        F::TxTooLarge => "tx_too_large",
        F::InvalidChangeAddress => "invalid_change_address",
        F::WatchOnly => "watch_only",
        F::VaultLocked => "vault_locked",
        F::GrantInvalid => "grant_invalid",
        F::GrantExceeded { .. } => "grant_exceeded",
        F::PreparedTxSpent => "prepared_tx_spent",
        F::NoPeers => "no_peers",
        F::BroadcastRejected { .. } => "broadcast_rejected",
        F::BroadcastUnknown { .. } => "broadcast_unknown",
        F::Cancelled => "cancelled",
    };
    format!("send.{name}")
}

pub(crate) fn encode_inputs(outpoints: &[OutPoint]) -> String {
    let list: Vec<String> = outpoints
        .iter()
        .map(|o| format!("{}:{}", o.txid, o.vout))
        .collect();
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

pub(crate) fn decode_inputs(text: &str) -> Vec<OutPoint> {
    serde_json::from_str::<Vec<String>>(text)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| {
            let (txid, vout) = s.split_once(':')?;
            Some(OutPoint {
                txid: Txid::from_str(txid).ok()?,
                vout: vout.parse().ok()?,
            })
        })
        .collect()
}

pub(crate) fn signed_tx(row: &MasternodeOpRow) -> Option<Transaction> {
    dashcore::consensus::deserialize(row.signed_tx.as_deref()?).ok()
}

/// A row's state (§5), from the row, the wallet's history and its unspent
/// coins. Undecodable signed bytes read as `sent` or `signed`, never as a
/// state that would let the row go.
pub(crate) fn state_of(
    row: &MasternodeOpRow,
    history: &HashMap<Txid, TxEntry>,
    unspent: &HashSet<OutPoint>,
) -> ProTxOperationState {
    if row.signed_tx.is_none() {
        let inputs = decode_inputs(&row.fee_inputs);
        return if inputs.iter().all(|o| unspent.contains(o)) {
            ProTxOperationState::NeedsSignature
        } else {
            ProTxOperationState::InputsSpent
        };
    }
    let unsettled = if row.sent_at.is_some() {
        ProTxOperationState::Sent
    } else if row.last_error.is_some() {
        ProTxOperationState::NotSent
    } else {
        ProTxOperationState::Signed
    };
    let Some(tx) = signed_tx(row) else {
        return unsettled;
    };
    let txid = tx.txid();
    if let Some(block) = history.get(&txid).and_then(|e| e.context.block_info()) {
        return ProTxOperationState::Confirmed {
            height: block.height(),
        };
    }
    let inputs: HashSet<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    let conflicted = history.iter().any(|(id, e)| {
        *id != txid
            && e.context.block_info().is_some()
            && e.tx
                .input
                .iter()
                .any(|i| inputs.contains(&i.previous_output))
    });
    if conflicted {
        ProTxOperationState::Conflicted
    } else {
        unsettled
    }
}

pub(crate) fn operation_of(
    row: &MasternodeOpRow,
    state: ProTxOperationState,
) -> Result<ProTxOperation, MasternodesError> {
    let terms: ProTxTerms = serde_json::from_str(&row.terms)
        .map_err(|e| EngineError::Internal(format!("stored ProTx terms: {e}")))?;
    let sign_request = match (state, &row.sign_address, &row.sign_message) {
        (ProTxOperationState::NeedsSignature, Some(address), Some(message)) => Some(SignRequest {
            address: address.clone(),
            message: message.clone(),
        }),
        _ => None,
    };
    Ok(ProTxOperation {
        id: row.id.clone(),
        kind: kind_of(&row.kind),
        pro_tx_hash: row.pro_tx_hash.clone(),
        txid: row.txid.clone(),
        state,
        sign_request,
        fee: row.fee,
        created_at: row.created_at,
        signed_at: row.signed_at,
        sent_at: row.sent_at,
        last_error: row.last_error.clone(),
        terms,
    })
}

/// One attempt on an operation at a time (§5): held from the first read
/// of the row to the record of the outcome.
pub(crate) struct InFlight {
    session: Arc<NetworkSession>,
    key: (WalletId, String),
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.session.masternodes.protx_in_flight().remove(&self.key);
    }
}

/// Where a test stops a hand-off, as a crash there would (MG-04 §7, the
/// manager's slice-2 addition). Nothing after the point runs, so the row is
/// left as the crash would leave it.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashPoint {
    /// Signed, not saved.
    Signed,
    /// Saved, no attempt recorded.
    Saved,
    /// Handed off, its outcome not recorded.
    HandedOff,
}

/// The test seam of the ProTx hand-off: the library's broadcast faked, and
/// an optional crash point.
#[cfg(test)]
pub(crate) struct Seam {
    pub broadcaster: Arc<dyn crate::send::TestBroadcaster>,
    pub crash: std::sync::Mutex<Option<CrashPoint>>,
    /// Stands in for the synced SPV list.
    pub list: std::sync::Mutex<Option<Vec<platform_wallet::masternode::MasternodeListSummary>>>,
    /// The stand-in list's height.
    pub list_height: std::sync::Mutex<u32>,
}

#[cfg(test)]
pub(crate) fn crash_at(
    session: &NetworkSession,
    point: CrashPoint,
) -> Result<(), MasternodesError> {
    let seam = session.masternodes.protx_seam();
    if seam
        .as_ref()
        .is_some_and(|s| *s.crash.lock().unwrap() == Some(point))
    {
        return Err(EngineError::Internal(format!("crash seam: {point:?}")).into());
    }
    Ok(())
}

/// What a hand-off locks once the transaction may be out: a fund-new
/// collateral output (D9).
pub(crate) struct Delivery {
    pub id: String,
    pub tx: Transaction,
    pub summary: PreparedSummary,
    pub charge: SpendCharge,
    pub fence: CoinFence,
    /// The row's `sent_at` was set: the bytes may already be out (D7).
    pub maybe_out: bool,
    pub lock_after: Option<OutPoint>,
}

impl NetworkSession {
    /// Marks `(wallet, id)` in flight (`InFlight` if another call holds it).
    pub(crate) fn claim_operation(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
    ) -> Result<InFlight, MasternodesError> {
        // An id is `new_id()`'s 32 lower-case hex; anything else can't name
        // a row (and is never repeated in the error).
        if id.len() != 32 || !id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(EngineError::InvalidArgument("not an operation id".into()).into());
        }
        let key = (wallet, id.to_string());
        if !self.masternodes.protx_in_flight().insert(key.clone()) {
            return Err(MasternodesError::InFlight);
        }
        Ok(InFlight {
            session: Arc::clone(self),
            key,
        })
    }

    pub(crate) async fn op_row(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<MasternodeOpRow, MasternodesError> {
        let (w, id) = (wallet.to_string(), id.to_string());
        self.appdb_op(move |db| db.masternode_op(&w, &id))
            .await?
            .ok_or(MasternodesError::OperationNotFound)
    }

    /// The wallet's history and unspent coins, for [`state_of`].
    pub(crate) async fn op_view(
        &self,
        wallet: WalletId,
    ) -> Result<(HashMap<Txid, TxEntry>, HashSet<OutPoint>), MasternodesError> {
        let w = self.wallet(&wallet).await?;
        let unspent = {
            let state = w.state().await;
            read_coins(&state.core_wallet)
                .coins
                .into_iter()
                .map(|c| c.utxo.outpoint)
                .collect()
        };
        let history = self.hub.history.read(&wallet, |h| h.clone());
        Ok((history, unspent))
    }

    pub(crate) async fn op_state(
        &self,
        wallet: WalletId,
        row: &MasternodeOpRow,
    ) -> Result<ProTxOperationState, MasternodesError> {
        let (history, unspent) = self.op_view(wallet).await?;
        Ok(state_of(row, &history, &unspent))
    }

    pub(crate) async fn operation(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let row = self.op_row(wallet, id).await?;
        let state = self.op_state(wallet, &row).await?;
        operation_of(&row, state)
    }

    /// Saves a new row durably, after the coin locks it holds (its inputs,
    /// and a collateral it locked), so a power loss never keeps the row
    /// without them.
    pub(crate) async fn save_operation(
        &self,
        wallet: WalletId,
        row: MasternodeOpRow,
        locks: Vec<OutPoint>,
    ) -> Result<(), MasternodesError> {
        let w = wallet.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            for o in &locks {
                db.lock_manual(&w, &o.txid.to_string(), o.vout, now)?;
            }
            db.insert_masternode_op_durable(&w, &row)
        })
        .await?;
        self.masternodes.local_change();
        Ok(())
    }

    /// Hands a saved row's transaction to the network (§2): reserved and
    /// charged as a `PreparedTx` (the fence is released there), the attempt
    /// made durable, `TxDraft`'s fenced hand-off, the outcome recorded.
    /// A definite `NotSent` of a first attempt clears `sent_at` (the row
    /// reads `not_sent`); every other outcome keeps it (`sent`).
    pub(crate) async fn deliver(
        self: &Arc<Self>,
        wallet: WalletId,
        d: Delivery,
    ) -> Result<(), MasternodesError> {
        let draft = self.new_draft(wallet, HashSet::new())?;
        #[cfg(test)]
        if let Some(seam) = self.masternodes.protx_seam() {
            *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&seam.broadcaster));
        }
        let prepared = draft.assembled(d.tx, d.summary, d.charge, d.fence, d.maybe_out);
        let (w, id, now) = (wallet.to_string(), d.id.clone(), now_secs());
        let begun = self
            .appdb_op(move |db| db.begin_masternode_op_attempt(&w, &id, now))
            .await?;
        let Some(earlier) = begun else {
            // The row is gone or unsigned: nothing goes out (dropping the
            // pending transaction releases it).
            return Err(MasternodesError::OperationNotFound);
        };
        let outcome = draft.broadcast_inner(Arc::clone(&prepared)).await;
        #[cfg(test)]
        crash_at(self, CrashPoint::HandedOff)?;
        if d.maybe_out {
            prepared.refund_unadmitted();
        }
        let code = error_code;
        let end = if prepared.released() {
            AttemptEnd::NotSent {
                first: !earlier,
                error: outcome
                    .as_ref()
                    .err()
                    .map_or_else(|| "send.cancelled".into(), code),
            }
        } else {
            AttemptEnd::Out {
                error: outcome.as_ref().err().map(code),
            }
        };
        let out = matches!(end, AttemptEnd::Out { .. });
        let (w, id) = (wallet.to_string(), d.id.clone());
        let lock = d.lock_after.filter(|_| out);
        self.appdb_op(move |db| {
            db.end_masternode_op_attempt(&w, &id, &end)?;
            if let Some(o) = lock {
                db.lock_manual(&w, &o.txid.to_string(), o.vout, now_secs())?;
            }
            Ok(())
        })
        .await?;
        self.masternodes.local_change();
        Ok(())
    }

    /// Releases the inputs of settled rows (in a block, or conflicted): the
    /// coins are spent, so the locks only clutter coin control (D25).
    pub(crate) async fn settle_operation_locks(
        &self,
        wallet: WalletId,
    ) -> Result<(), MasternodesError> {
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        if rows.iter().all(|r| r.fee_inputs == "[]") {
            return Ok(());
        }
        let (history, unspent) = self.op_view(wallet).await?;
        let settled: Vec<(String, Vec<OutPoint>)> = rows
            .iter()
            .filter(|r| r.fee_inputs != "[]")
            .filter(|r| {
                matches!(
                    state_of(r, &history, &unspent),
                    ProTxOperationState::Confirmed { .. } | ProTxOperationState::Conflicted
                )
            })
            .map(|r| (r.id.clone(), decode_inputs(&r.fee_inputs)))
            .collect();
        if settled.is_empty() {
            return Ok(());
        }
        let w = wallet.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            for (id, inputs) in &settled {
                for o in inputs {
                    db.unlock(&w, &o.txid.to_string(), o.vout, now)?;
                }
                db.clear_masternode_op_inputs(&w, id)?;
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// Ends a row nothing of which may be out (D6) and unlocks what it
    /// locked. `OperationState` for any other state.
    pub(crate) async fn discard(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
    ) -> Result<(), MasternodesError> {
        let _flight = self.claim_operation(wallet, id)?;
        let row = self.op_row(wallet, id).await?;
        let state = self.op_state(wallet, &row).await?;
        if !matches!(
            state,
            ProTxOperationState::NeedsSignature
                | ProTxOperationState::InputsSpent
                | ProTxOperationState::Signed
                | ProTxOperationState::NotSent
        ) {
            return Err(MasternodesError::OperationState {
                state: state_name(state).into(),
            });
        }
        let mut unlock = decode_inputs(&row.fee_inputs);
        if row.locked_collateral
            && let Some(o) = row_collateral(&row)
        {
            unlock.push(o);
        }
        let (w, id) = (wallet.to_string(), id.to_string());
        let deleted = self
            .appdb_op(move |db| {
                if !db.delete_unsent_masternode_op(&w, &id)? {
                    return Ok(false);
                }
                let now = now_secs();
                for o in &unlock {
                    db.unlock(&w, &o.txid.to_string(), o.vout, now)?;
                }
                Ok(true)
            })
            .await?;
        if !deleted {
            // `sent_at` was set: the bytes may be out (D6).
            return Err(MasternodesError::OperationState {
                state: state_name(ProTxOperationState::Sent).into(),
            });
        }
        self.masternodes.local_change();
        Ok(())
    }
}

impl NetworkSession {
    /// The follower's pass over every wallet (D9, D25): settled rows' input
    /// locks released, and each of the wallet's masternodes' collateral
    /// that the wallet holds unspent locked, unless the user released it
    /// (Core's `AutoLockMasternodeCollaterals`, rc.3
    /// `wallet/wallet.cpp:2515-2525`). A wallet whose coin fence a build
    /// holds waits for the next pass.
    pub(crate) async fn protx_follow_pass(&self) -> Result<(), MasternodesError> {
        let manager = self.manager()?;
        let ids = {
            let m = Arc::clone(&manager);
            tokio::task::spawn_blocking(move || m.list_wallet_ids_blocking()).await?
        };
        for id in ids {
            let wallet = WalletId(id);
            self.settle_operation_locks(wallet).await?;
            let Some(_fence) = self.try_coin_fence(wallet).await? else {
                continue;
            };
            let (m, hub) = (Arc::clone(&manager), Arc::clone(&self.hub));
            let collaterals: Vec<OutPoint> = tokio::task::spawn_blocking(move || {
                super::super::follow::wallet_masternodes(&m, &hub.history, &wallet)
                    .map(|(owned, _)| owned.iter().filter_map(|o| o.collateral).collect())
                    .unwrap_or_default()
            })
            .await?;
            if collaterals.is_empty() {
                continue;
            }
            let (_, unspent) = self.op_view(wallet).await?;
            let held: Vec<OutPoint> = collaterals
                .into_iter()
                .filter(|o| unspent.contains(o))
                .collect();
            let w = wallet.to_string();
            let locked = self
                .appdb_op(move |db| {
                    let active: HashSet<(String, u32)> = db
                        .locks(&w)?
                        .into_iter()
                        .filter(|r| r.is_locked())
                        .map(|r| (r.txid, r.vout))
                        .collect();
                    let released: HashSet<String> = db
                        .settings_with_prefix(&w, RELEASED)?
                        .into_iter()
                        .map(|(k, _)| k)
                        .collect();
                    let now = now_secs();
                    let mut changed = false;
                    for o in &held {
                        let key = (o.txid.to_string(), o.vout);
                        let name = format!("{RELEASED}{}-{}", o.txid, o.vout);
                        if !active.contains(&key) && !released.contains(&name) {
                            db.lock_manual(&w, &key.0, key.1, now)?;
                            changed = true;
                        }
                    }
                    Ok(changed)
                })
                .await?;
            if locked {
                self.hub.pump.mark_balances(wallet);
            }
        }
        Ok(())
    }
}

/// The existing collateral a row names (its terms).
fn row_collateral(row: &MasternodeOpRow) -> Option<OutPoint> {
    let terms: ProTxTerms = serde_json::from_str(&row.terms).ok()?;
    super::keys::parse_outpoint(terms.collateral_outpoint.as_deref()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> MasternodeOpRow {
        MasternodeOpRow {
            id: "a".into(),
            kind: "registration".into(),
            terms: "{}".into(),
            request: "{}".into(),
            unsigned_tx: None,
            sign_address: None,
            sign_message: None,
            fee_inputs: "[]".into(),
            signed_tx: None,
            txid: None,
            pro_tx_hash: None,
            locked_collateral: false,
            fee: 0,
            created_at: 0,
            signed_at: None,
            sent_at: None,
            last_error: None,
        }
    }

    #[test]
    fn inputs_round_trip_as_txid_colon_vout() {
        let o = OutPoint {
            txid: Txid::from_str(&"ab".repeat(32)).unwrap(),
            vout: 7,
        };
        let text = encode_inputs(&[o]);
        assert_eq!(text, format!("[\"{}:7\"]", "ab".repeat(32)));
        assert_eq!(decode_inputs(&text), vec![o]);
    }

    #[test]
    fn unsigned_rows_read_from_their_inputs() {
        let o = OutPoint {
            txid: Txid::from_str(&"cd".repeat(32)).unwrap(),
            vout: 1,
        };
        let mut r = row();
        r.fee_inputs = encode_inputs(&[o]);
        let history = HashMap::new();
        assert_eq!(
            state_of(&r, &history, &HashSet::from([o])),
            ProTxOperationState::NeedsSignature
        );
        assert_eq!(
            state_of(&r, &history, &HashSet::new()),
            ProTxOperationState::InputsSpent
        );
    }

    #[test]
    fn signed_rows_read_signed_not_sent_or_sent() {
        let mut r = row();
        r.signed_tx = Some(vec![0xff]);
        r.txid = Some("x".into());
        let (h, u) = (HashMap::new(), HashSet::new());
        assert_eq!(state_of(&r, &h, &u), ProTxOperationState::Signed);
        r.last_error = Some("send.no_peers".into());
        assert_eq!(state_of(&r, &h, &u), ProTxOperationState::NotSent);
        r.sent_at = Some(1);
        assert_eq!(state_of(&r, &h, &u), ProTxOperationState::Sent);
    }
}
