//! ProTx operations (MG-04 §5): the `masternode_ops` rows, their state
//! derived on every read, the in-flight mark, and the one hand-off every
//! signing call and resend takes: the row saved durably with its locks,
//! the attempt recorded durably, the `PreparedTx` through the dispatch
//! fence, the outcome recorded.
//!
//! A row whose bytes may be out is never deleted (D6): the only delete is
//! `discard_operation`'s `... WHERE sent_at IS NULL`, which takes the row's
//! payout-change barrier with it in one transaction (MG-13 P16), and
//! `sent_at` is durable before each hand-off. A row's inputs stay persistently locked
//! from its save until its transaction is in a block, conflicted or
//! abandoned in Activity (MG-10 §1.10, D49), or the row is discarded (D8,
//! D25), so no build after a restart can spend them in a second
//! transaction.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::{OutPoint, Transaction, Txid};
use dw_appdb::{AttemptEnd, Holder, MasternodeOpRow};
use dw_protx::params::collateral_amount;

use super::super::MasternodesError;
use super::super::protx_records::{
    ProTxOperation, ProTxOperationKind, ProTxOperationState, ProTxTerms, SignRequest,
};
use super::super::shared::list_settled;
use crate::coins::{CoinFence, WalletCoin, now_secs, read_coins};
use crate::history::{Finality, TxEntry, chain_locked, instant_locked, spender_heights};
use crate::send::{PreparedSummary, SpendCharge};
use crate::{EngineError, NetworkSession, WalletId};

/// Discard's refusal while the history lists its payment pending.
const DISCARD_LISTED: &str = "discarding the operation: its payment is still listed in History; abandon it there first, then discard again";

/// Discard's refusal when a sent mark may be all that claims the payment's
/// coins (not this session's held hand-off, not listed).
const DISCARD_UNCERTAIN: &str = "discarding the operation: its payment may still be on its way; restart Dash Wallet, abandon the payment in History, then discard again; if History doesn't list it, run Tools ▸ Repair ▸ Remove unconfirmed transactions, then discard again";

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
        ProTxOperationKind::UpdateShare => "update_share",
        ProTxOperationKind::Dissolve => "dissolve",
    }
}

pub(crate) fn kind_of(name: &str) -> ProTxOperationKind {
    match name {
        "update_registrar" => ProTxOperationKind::UpdateRegistrar,
        "update_share" => ProTxOperationKind::UpdateShare,
        "dissolve" => ProTxOperationKind::Dissolve,
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
        ProTxOperationState::Abandoned => "abandoned",
    }
}

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
        F::HeldUntilRestart => "held_until_restart",
    };
    format!("send.{name}")
}

/// `fee_inputs`: a JSON array of `"txid:vout"`.
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

/// What [`state_of`] reads of the wallet: [`NetworkSession::op_view`].
pub(crate) struct OpView {
    pub history: HashMap<Txid, TxEntry>,
    pub unspent: HashSet<OutPoint>,
    pub chainlock: Option<u32>,
    pub finality: Finality,
}

impl OpView {
    /// The history, the unspent coins and the [`Beats`] they read, for
    /// [`state_of`].
    pub(crate) fn with_beats(self) -> (HashMap<Txid, TxEntry>, HashSet<OutPoint>, Beats) {
        let beats = Beats::of(&self.history, self.chainlock, self.finality);
        (self.history, self.unspent, beats)
    }
}

/// What may beat an operation: only a block the SPV header chain proves
/// final under a ChainLock ([`Finality`]), as a reorg can undo any other
/// and a record kept from a fork a restart reloads names a block the chain
/// no longer holds. A block that beats one the history holds
/// InstantSend-locked is also ChainLocked (DIP-10, key-wallet's order).
/// Each release that a beaten operation allows (its inputs and collateral,
/// its payout-change barrier, the hold on a later update) reads it.
pub(crate) struct Beats {
    /// The history's [`crate::history::conflicted`] blocks, of those
    /// `finality` proves.
    pub conflicted: HashMap<Txid, (u32, dashcore::BlockHash)>,
    pub finality: Finality,
    pub chainlock: Option<u32>,
}

impl Beats {
    pub(crate) fn of(
        history: &HashMap<Txid, TxEntry>,
        chainlock: Option<u32>,
        finality: Finality,
    ) -> Self {
        let conflicted = crate::history::conflicted(history, chainlock, |b| finality.proves(b));
        Self {
            conflicted,
            finality,
            chainlock,
        }
    }

    /// The height of `rival`'s block when it beats a transaction the
    /// history holds InstantSend-locked (`locked`) or not.
    fn height(&self, rival: &TxEntry, locked: bool) -> Option<u32> {
        let block = rival.context.block_info()?;
        (self.finality.proves(block) && (!locked || chain_locked(&rival.context, self.chainlock)))
            .then(|| block.height())
    }

    fn parent(&self, txid: &Txid) -> Option<u32> {
        self.conflicted.get(txid).map(|(height, _)| *height)
    }

    /// Tests: [`Beats`] of `history` on a header chain that holds each of
    /// its blocks, under a ChainLock at `chainlock` (none proves nothing).
    #[cfg(test)]
    pub(crate) fn on_chain(history: &HashMap<Txid, TxEntry>, chainlock: Option<u32>) -> Self {
        use dashcore::hashes::Hash;
        let mut active: HashMap<u32, dashcore::BlockHash> = (history.values())
            .filter_map(|e| e.context.block_info())
            .map(|b| (b.height(), b.block_hash()))
            .collect();
        let lock = chainlock.map(|h| {
            let hash = *active.entry(h).or_insert(dashcore::BlockHash::all_zeros());
            (h, hash)
        });
        let finality = Finality {
            chainlock: lock,
            active,
        };
        Self::of(history, chainlock, finality)
    }
}

pub(crate) fn signed_tx(row: &MasternodeOpRow) -> Option<Transaction> {
    dashcore::consensus::deserialize(row.signed_tx.as_deref()?).ok()
}

/// Tests: answers `session`'s header lookups with the zero hash the
/// fixtures' blocks carry, but at a height `forks` names, with its hash:
/// a block a fork replaced.
#[cfg(test)]
pub(crate) fn flat_headers(
    session: &NetworkSession,
    forks: &Arc<std::sync::Mutex<HashMap<u32, dashcore::BlockHash>>>,
) {
    use dashcore::hashes::Hash;
    let forks = Arc::clone(forks);
    session.hub.attach_headers(Arc::new(move |height| {
        let hash = (forks.lock().unwrap().get(&height).copied())
            .unwrap_or(dashcore::BlockHash::all_zeros());
        Box::pin(async move { Some(hash) })
    }));
}

/// A row's state (§5), from the row, the wallet's history, its unspent
/// coins and what may beat it ([`Beats`]). Undecodable
/// signed bytes read as `sent` or `signed`, never as a state that would let
/// the row go.
pub(crate) fn state_of(
    row: &MasternodeOpRow,
    history: &HashMap<Txid, TxEntry>,
    unspent: &HashSet<OutPoint>,
    beats: &Beats,
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
    match tx_beaten_height(&tx, history, beats) {
        Some(_) => ProTxOperationState::Conflicted,
        None => unsettled,
    }
}

/// The height of the lowest block of [`Beats`] that beats the transaction
/// `txid` spending `inputs`, so that it can never confirm: a block spending
/// one of `inputs` ([`conflict_height`]), or the history's descendant rule
/// ([`crate::history::conflicted`], which reads an abandoned parent too)
/// for it or for a transaction it spends. One the history holds
/// InstantSend-locked is never beaten through its parent.
pub(crate) fn beaten_height(
    history: &HashMap<Txid, TxEntry>,
    beats: &Beats,
    inputs: &[OutPoint],
    txid: Option<&Txid>,
) -> Option<u32> {
    let locked = locked_in(history, txid);
    let parents = (inputs.iter())
        .filter(|_| !locked)
        .filter_map(|o| beats.parent(&o.txid));
    (conflict_height(history, inputs, txid, beats).into_iter())
        .chain(txid.and_then(|t| beats.parent(t)))
        .chain(parents)
        .min()
}

fn locked_in(history: &HashMap<Txid, TxEntry>, txid: Option<&Txid>) -> bool {
    txid.and_then(|t| history.get(t))
        .is_some_and(instant_locked)
}

/// `state` once Activity's abandon is applied (MG-10 §1.10, D49): a `sent`
/// registration or key change whose txid the wallet's abandoned marks name
/// (an Abandon's durable commit point), and that isn't InstantSend-locked,
/// is `abandoned`. Terminal: its fee inputs are released and it is never
/// resent. Core may still mine it, so its payout-change barrier stays
/// (MG-13 P16), it holds the signing of a later update of its masternode,
/// and the key checks (D51) read [`state_of`], where it is `sent`.
pub(crate) fn abandon_settled(
    row: &MasternodeOpRow,
    state: ProTxOperationState,
    history: &HashMap<Txid, TxEntry>,
    abandoned: &HashSet<Txid>,
) -> ProTxOperationState {
    if state != ProTxOperationState::Sent
        || !matches!(
            kind_of(&row.kind),
            ProTxOperationKind::Registration | ProTxOperationKind::UpdateRegistrar
        )
    {
        return state;
    }
    // An InstantSend lock seen since outranks the mark: it will be mined.
    let marked = (row.txid.as_deref())
        .and_then(|t| Txid::from_str(t).ok())
        .is_some_and(|t| abandoned.contains(&t) && !history.get(&t).is_some_and(instant_locked));
    if marked {
        ProTxOperationState::Abandoned
    } else {
        state
    }
}

/// The lowest height of a block of [`Beats`] holding another transaction
/// than `except` that spends one of `inputs`: MG-04's operations and the
/// shared sessions alike (D70). One the history holds InstantSend-locked
/// is beaten only by a ChainLocked block.
pub(crate) fn conflict_height(
    history: &HashMap<Txid, TxEntry>,
    inputs: &[OutPoint],
    except: Option<&Txid>,
    beats: &Beats,
) -> Option<u32> {
    conflict_of(history, inputs, except, &[], beats).map(|(_, height)| height)
}

/// [`conflict_height`] with the transaction in that block, `preferred`
/// ones first at the same height.
pub(crate) fn conflict_of(
    history: &HashMap<Txid, TxEntry>,
    inputs: &[OutPoint],
    except: Option<&Txid>,
    preferred: &[Txid],
    beats: &Beats,
) -> Option<(Txid, u32)> {
    let locked = locked_in(history, except);
    history
        .iter()
        .filter(|(id, entry)| {
            Some(*id) != except
                && entry
                    .tx
                    .input
                    .iter()
                    .any(|input| inputs.contains(&input.previous_output))
        })
        .filter_map(|(id, entry)| Some((*id, beats.height(entry, locked)?)))
        .min_by_key(|(id, height)| (*height, !preferred.contains(id), *id))
}

/// [`beaten_height`] of `tx`.
fn tx_beaten_height(
    tx: &Transaction,
    history: &HashMap<Txid, TxEntry>,
    beats: &Beats,
) -> Option<u32> {
    let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    beaten_height(history, beats, &inputs, Some(&tx.txid()))
}

pub(crate) const OVERDUE_SECS: u64 = 10_800;

pub(crate) fn tip_overdue(sent_at: u64, tip_time: Option<u64>) -> bool {
    tip_time.is_some_and(|time| time.saturating_sub(sent_at) >= OVERDUE_SECS)
}

pub(crate) fn overdue(
    sent_at: u64,
    list_synced: bool,
    list_height: Option<u32>,
    tip_height: Option<u32>,
    tip_time: Option<u64>,
) -> bool {
    list_synced
        && matches!((list_height, tip_height), (Some(list), Some(tip)) if list.saturating_add(1) >= tip)
        && tip_overdue(sent_at, tip_time)
}

pub(crate) fn operation_of(
    row: &MasternodeOpRow,
    state: ProTxOperationState,
    tracker: &crate::sync::SyncTracker,
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
        debit: crate::masternodes::shared::row_debit(row)?,
        overdue: state == ProTxOperationState::Sent
            && row.sent_at.is_some_and(|sent_at| {
                overdue(
                    sent_at,
                    tracker.masternodes_synced(),
                    tracker.masternode_list_height(),
                    tracker.tip_height(),
                    tracker.current_tip_time(),
                )
            }),
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

/// [`InFlight`] for a caller that holds the session by reference (the
/// follower's pass), released when it is dropped.
pub(crate) struct InFlightRef<'a> {
    session: &'a NetworkSession,
    key: (WalletId, String),
}

impl Drop for InFlightRef<'_> {
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
    /// A shared session's signed copy saved with `out_at`, not returned
    /// (MG-04b D17).
    SharedOut,
    /// The follower's settling of operation locks fails.
    Settle,
    /// The follower's pass over shared sessions fails.
    SharedFollow,
    /// The collateral lock step fails, past the coin fence.
    LockStep,
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
    /// Shared registrations in a block at a height, added to the wallet's
    /// history by their ProTx hash: the pin can't decode a version-3
    /// ProRegTx into it (BL-187).
    pub shared_mined: std::sync::Mutex<Vec<(Txid, dashcore::Transaction, u32)>>,
    /// Which of `shared_mined` are in a ChainLocked block.
    pub shared_locked: std::sync::Mutex<std::collections::HashSet<Txid>>,
    /// Stands in for the SPV client's state as the shared recovery read
    /// sees it: running, and connected peers.
    pub spv: std::sync::Mutex<Option<(bool, u32)>>,
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

/// A saved row's hand-off.
pub(crate) struct Delivery {
    pub id: String,
    pub tx: Transaction,
    pub summary: PreparedSummary,
    pub charge: SpendCharge,
    pub fence: CoinFence,
    /// The row's `sent_at` was set: the bytes may already be out (D7).
    pub maybe_out: bool,
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

    /// Marks shared session `(wallet, id)` in flight (MG-04b §9.2): its
    /// 64-hex id never names an operation (32 hex).
    pub(crate) fn claim_shared_session(
        self: &Arc<Self>,
        wallet: WalletId,
        id: &str,
    ) -> Result<InFlight, MasternodesError> {
        crate::masternodes::shared::check_session_id(id)?;
        let key = (wallet, id.to_string());
        if !self.masternodes.protx_in_flight().insert(key.clone()) {
            return Err(MasternodesError::InFlight);
        }
        Ok(InFlight {
            session: Arc::clone(self),
            key,
        })
    }

    /// [`Self::claim_shared_session`] without waiting or refusing: `None`
    /// while a call holds the session.
    pub(crate) fn try_claim_shared_session(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Option<InFlightRef<'_>> {
        let key = (wallet, id.to_string());
        let claimed = self.masternodes.protx_in_flight().insert(key.clone());
        // Built only when claimed: dropping one releases the key.
        claimed.then(|| InFlightRef { session: self, key })
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

    /// The wallet's history, unspent coins, best ChainLock height and the
    /// [`Finality`] of its blocks, for [`state_of`].
    pub(crate) async fn op_view(&self, wallet: WalletId) -> Result<OpView, MasternodesError> {
        let w = self.wallet(&wallet).await?;
        self.op_view_of(wallet, &w).await
    }

    /// [`Self::op_view`] of the wallet `w`, already looked up: a wallet that
    /// left since is an error.
    pub(crate) async fn op_view_of(
        &self,
        wallet: WalletId,
        w: &platform_wallet::wallet::platform_wallet::PlatformWallet,
    ) -> Result<OpView, MasternodesError> {
        let (unspent, applied) = {
            let state = crate::wallet_read::info_of(w).await?;
            let unspent = (read_coins(&state.core_wallet).coins.into_iter())
                .map(|c| c.utxo.outpoint)
                .collect();
            (unspent, crate::history_ops::applied_chainlock(&state))
        };
        let chainlock = (applied.max(self.hub.tracker().chainlock_height()))
            .max(self.hub.best_chainlock().map(|(h, _)| h));
        let history = self.hub.history.read(&wallet, |h| h.clone());
        let finality = self
            .finality(|at_most| spender_heights([&history], at_most))
            .await;
        Ok(OpView {
            history,
            unspent,
            chainlock,
            finality,
        })
    }

    /// The [`Finality`] of the blocks at the heights `heights` names at or
    /// below the best ChainLock: the SPV header chain's hashes there, read
    /// while it holds the ChainLock's block. Nothing is proven without a
    /// ChainLock or that block.
    pub(crate) async fn finality(&self, heights: impl FnOnce(u32) -> BTreeSet<u32>) -> Finality {
        let Some(chainlock) = self.hub.best_chainlock() else {
            return Finality::default();
        };
        match self.headers_under(chainlock, heights(chainlock.0)).await {
            Some(active) => Finality {
                chainlock: Some(chainlock),
                active,
            },
            None => Finality::default(),
        }
    }

    /// The txids the wallet's abandoned marks name, for [`abandon_settled`].
    pub(crate) async fn abandoned_txids(
        &self,
        wallet: WalletId,
    ) -> Result<HashSet<Txid>, MasternodesError> {
        Ok(self
            .appdb_op(move |db| crate::tx_actions::abandoned_txids(db, wallet))
            .await?)
    }

    pub(crate) async fn op_state(
        &self,
        wallet: WalletId,
        row: &MasternodeOpRow,
    ) -> Result<ProTxOperationState, MasternodesError> {
        let (history, unspent, beats) = self.op_view(wallet).await?.with_beats();
        let abandoned = self.abandoned_txids(wallet).await?;
        let view = self
            .dissolution_view(wallet, std::slice::from_ref(row))
            .await;
        let state = list_settled(row, state_of(row, &history, &unspent, &beats), &view);
        Ok(abandon_settled(row, state, &history, &abandoned))
    }

    /// `Ok` when a resend of `row` may hand its bytes off: it reads
    /// `signed`, `not_sent` or `sent`, and a registration's or key change's
    /// txid carries no abandoned mark, whatever its state reads (an
    /// InstantSend lock seen since the abandon, or a hand-off recovered
    /// unsent and abandoned in Activity). `OperationState` otherwise,
    /// `abandoned` for a marked one. The shared kinds keep their own rules.
    pub(crate) async fn require_resendable(
        &self,
        wallet: WalletId,
        row: &MasternodeOpRow,
    ) -> Result<(), MasternodesError> {
        let state = self.op_state(wallet, row).await?;
        let abandoned = self.abandoned_txids(wallet).await?;
        let covered = matches!(
            kind_of(&row.kind),
            ProTxOperationKind::Registration | ProTxOperationKind::UpdateRegistrar
        );
        let marked = covered
            && (row.txid.as_deref())
                .and_then(|t| Txid::from_str(t).ok())
                .is_some_and(|t| abandoned.contains(&t));
        let state = match state {
            ProTxOperationState::Signed
            | ProTxOperationState::NotSent
            | ProTxOperationState::Sent
                if !marked =>
            {
                return Ok(());
            }
            ProTxOperationState::Signed
            | ProTxOperationState::NotSent
            | ProTxOperationState::Sent => ProTxOperationState::Abandoned,
            state => state,
        };
        Err(MasternodesError::OperationState {
            state: state_name(state).into(),
        })
    }

    pub(crate) async fn operation(
        &self,
        wallet: WalletId,
        id: &str,
    ) -> Result<ProTxOperation, MasternodesError> {
        let row = self.op_row(wallet, id).await?;
        let state = self.op_state(wallet, &row).await?;
        operation_of(&row, state, &self.hub.tracker())
    }

    /// Saves a new row durably with the coin locks it holds (its inputs,
    /// and a collateral it locked) in one transaction, so a power loss
    /// never keeps the row without them, nor them without the row (D53).
    pub(crate) async fn save_operation(
        &self,
        wallet: WalletId,
        row: MasternodeOpRow,
        locks: Vec<OutPoint>,
    ) -> Result<(), MasternodesError> {
        self.save_operation_with_barrier(wallet, row, locks, None)
            .await
    }

    /// [`Self::save_operation`] of a signed ProUpRegTx that may change the
    /// payout to `new_payout`: its payout-change barrier is written in the
    /// same transaction as the row (MG-13 P16), so no crash keeps either
    /// without the other.
    pub(crate) async fn save_operation_with_barrier(
        &self,
        wallet: WalletId,
        row: MasternodeOpRow,
        locks: Vec<OutPoint>,
        new_payout: Option<Vec<u8>>,
    ) -> Result<(), MasternodesError> {
        let w = wallet.to_string();
        let locks: Vec<(String, u32)> =
            locks.iter().map(|o| (o.txid.to_string(), o.vout)).collect();
        #[cfg(test)]
        crate::coins::pause_at(&self.coin_fences.pause_row_write).await;
        self.appdb_op(move |db| {
            db.insert_masternode_op_with_barrier_durable(
                &w,
                &row,
                &locks,
                now_secs(),
                new_payout.as_deref(),
            )
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
        let draft = self.new_draft(wallet, HashSet::new(), false)?;
        #[cfg(test)]
        if let Some(seam) = self.masternodes.protx_seam() {
            *draft.broadcaster.lock().unwrap() = Some(Arc::clone(&seam.broadcaster));
        }
        let prepared = draft.assembled(d.tx, d.summary, d.charge, d.fence, d.maybe_out, None);
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
        // An error before the hand-off (the sent mark's write) leaves the
        // transaction pending: nothing was handed off.
        let held = matches!(
            &outcome,
            Err(EngineError::Send(
                crate::send::SendFailure::HeldUntilRestart
            ))
        );
        let end = if prepared.released() || (outcome.is_err() && prepared.pending()) || held {
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
        let (w, id) = (wallet.to_string(), d.id.clone());
        self.appdb_op(move |db| db.end_masternode_op_attempt(&w, &id, &end))
            .await?;
        self.masternodes.local_change();
        Ok(())
    }

    /// Whether a block at `height` is one a reorg won't undo (MG-04b D70,
    /// Opus-h NIT-4): at or under the ChainLock, or
    /// `RECOMMENDED_CONFIRMATIONS` deep at the tip.
    pub(crate) fn buried(&self) -> impl Fn(u32) -> bool + use<> {
        let (tip, chainlock) = {
            let t = self.hub.tracker();
            (t.tip_height(), t.chainlock_height())
        };
        move |height| {
            chainlock.is_some_and(|c| height <= c)
                || tip.is_some_and(|t| {
                    t.saturating_sub(height) + 1 >= crate::history::RECOMMENDED_CONFIRMATIONS
                })
        }
    }

    /// Releases the inputs of settled rows: in a block, abandoned in
    /// Activity (MG-10 §1.10, D49), or conflicted by a spend
    /// [`Self::buried`] says a reorg won't undo (MG-04b D70, review r1 of 1b
    /// n-5). The coins are spent, or come back to be spent (D49), so the
    /// locks only clutter coin control (D25).
    pub(crate) async fn settle_operation_locks(
        &self,
        wallet: WalletId,
    ) -> Result<(), MasternodesError> {
        #[cfg(test)]
        crash_at(self, CrashPoint::Settle)?;
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        if rows.iter().all(|r| r.fee_inputs == "[]") {
            return Ok(());
        }
        let (history, unspent, beats) = self.op_view(wallet).await?.with_beats();
        let abandoned = self.abandoned_txids(wallet).await?;
        let buried = self.buried();
        let settled: Vec<(String, Vec<OutPoint>, bool)> = rows
            .iter()
            .filter(|r| r.fee_inputs != "[]")
            .filter_map(|r| {
                let state = state_of(r, &history, &unspent, &beats);
                let state = abandon_settled(r, state, &history, &abandoned);
                let settled = match state {
                    ProTxOperationState::Confirmed { .. } | ProTxOperationState::Abandoned => true,
                    ProTxOperationState::Conflicted => signed_tx(r)
                        .and_then(|tx| tx_beaten_height(&tx, &history, &beats))
                        .is_some_and(&buried),
                    _ => false,
                };
                settled.then(|| {
                    let by_abandon = state == ProTxOperationState::Abandoned;
                    (r.id.clone(), decode_inputs(&r.fee_inputs), by_abandon)
                })
            })
            .collect();
        if settled.is_empty() {
            return Ok(());
        }
        let by_abandon = settled.iter().any(|(_, _, a)| *a);
        let w = wallet.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            for (id, inputs, abandoned) in &settled {
                if *abandoned {
                    // Its inputs and their locks in one transaction; its
                    // payout-change barrier stays (MG-13 P16).
                    let inputs: Vec<(String, u32)> = inputs
                        .iter()
                        .map(|o| (o.txid.to_string(), o.vout))
                        .collect();
                    db.release_abandoned_masternode_op_durable(&w, id, &inputs, now)?;
                    continue;
                }
                for o in inputs {
                    let holder = Holder::Operation(id);
                    db.release_lock(&w, holder, &o.txid.to_string(), o.vout, now)?;
                }
                db.clear_masternode_op_inputs(&w, id)?;
            }
            Ok(())
        })
        .await?;
        if by_abandon {
            self.hub.pump.mark_balances(wallet);
            self.masternodes.local_change();
        }
        Ok(())
    }

    /// Unlocks the collateral each conflicted row locked, unless another
    /// row names the coin (D54). The caller holds the wallet's coin fence,
    /// so no registration saves a row naming it meanwhile.
    async fn release_conflicted_collaterals(
        &self,
        wallet: WalletId,
    ) -> Result<(), MasternodesError> {
        let w = wallet.to_string();
        let rows = self.appdb_op(move |db| db.masternode_ops(&w)).await?;
        if !rows.iter().any(|r| r.locked_collateral) {
            return Ok(());
        }
        let (history, unspent, beats) = self.op_view(wallet).await?.with_beats();
        let (beaten, live): (Vec<_>, Vec<_>) = rows.iter().partition(|r| {
            state_of(r, &history, &unspent, &beats) == ProTxOperationState::Conflicted
        });
        let named: HashSet<OutPoint> = live.iter().filter_map(|r| row_collateral(r)).collect();
        let release: Vec<(String, OutPoint)> = beaten
            .iter()
            .filter(|r| r.locked_collateral)
            .filter_map(|r| Some((r.id.clone(), row_collateral(r)?)))
            .filter(|(_, o)| !named.contains(o))
            .collect();
        if release.is_empty() {
            return Ok(());
        }
        let w = wallet.to_string();
        self.appdb_op(move |db| {
            let now = now_secs();
            for (id, o) in &release {
                db.release_masternode_op_collateral(&w, id, &o.txid.to_string(), o.vout, now)?;
            }
            Ok(())
        })
        .await?;
        self.hub.pump.mark_balances(wallet);
        self.masternodes.local_change();
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
        // A first hand-off refused before it left whose sent marks could not
        // be dropped (`send.held_until_restart`) still has them: they go
        // first, or a restart would list its transaction for Resend.
        let marked = (state == ProTxOperationState::NotSent)
            .then(|| row.txid.as_deref().and_then(|t| Txid::from_str(t).ok()))
            .flatten();
        // A payment listed from its mark after a restart has its coins
        // claimed: it is abandoned first, never dropped here. Its marks go
        // only for this session's held hand-off, whose coins no load
        // claimed, or once it is abandoned. Elsewhere a mark may be all that
        // claims its coins (a load still running, one whose saved copies
        // didn't read): Discard waits.
        let listed = marked.and_then(|t| self.hub.history.get(&wallet, &t));
        if listed.as_ref().is_some_and(|e| !e.is_abandoned()) {
            return Err(EngineError::Wallet(DISCARD_LISTED.into()).into());
        }
        let droppable = listed.is_some()
            || marked.is_some_and(|t| {
                (self.hub.history).read_wallet(&wallet, |h| {
                    h.held_here.contains(&t) && !h.claimed_at_load.contains(&t)
                })
            });
        let (w, id) = (wallet.to_string(), id.to_string());
        let deleted = self
            .appdb_op(move |db| {
                if let Some(txid) = marked
                    && crate::tx_actions::has_sent_mark(db, wallet, txid)?
                {
                    if !droppable {
                        return Ok(Err(None));
                    }
                    if let Err(e) = crate::tx_actions::forget_sent(db, wallet, &[txid]) {
                        return Ok(Err(Some(e.to_string())));
                    }
                }
                // The row and its payout-change barrier go in one
                // transaction, only while `sent_at IS NULL` (MG-13 P16,
                // r4 m1): a row that may be out keeps both.
                if !db.discard_unsent_masternode_op_durable(&w, &id)? {
                    return Ok(Ok(false));
                }
                let now = now_secs();
                for o in &unlock {
                    let holder = Holder::Operation(&id);
                    db.release_lock(&w, holder, &o.txid.to_string(), o.vout, now)?;
                }
                Ok(Ok(true))
            })
            .await?
            .map_err(|e| match e {
                None => EngineError::Wallet(DISCARD_UNCERTAIN.into()),
                Some(e) => {
                    tracing::warn!(%wallet, error = %e, "could not drop a discarded operation's sent marks");
                    EngineError::Storage(
                        "discarding the operation: its payment's saved copy could not be removed; restart Dash Wallet, then discard it again".into(),
                    )
                }
            })?;
        if let Some(t) = marked {
            self.hub.history.drop_claims(&wallet, [t]);
        }
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
    /// locks released, then the wallet's collaterals locked
    /// ([`Self::lock_own_collaterals`]). A failed earlier step is logged
    /// and skips nothing after it.
    pub(crate) async fn protx_follow_pass(&self) -> Result<(), MasternodesError> {
        let manager = self.manager()?;
        let ids = {
            let m = Arc::clone(&manager);
            tokio::task::spawn_blocking(move || m.list_wallet_ids_blocking()).await?
        };
        for id in ids {
            let wallet = WalletId(id);
            if let Err(e) = self.settle_operation_locks(wallet).await {
                tracing::warn!(%wallet, error = %e, "settling ProTx operation locks");
            }
            if let Err(e) = self.shared_follow_pass(wallet).await {
                tracing::warn!(%wallet, error = %e, "following shared masternode sessions");
            }
            self.collateral_step(wallet).await;
        }
        Ok(())
    }

    /// Locks the collaterals of the wallets whose history gained a ProRegTx
    /// since the last call, whether or not the masternode list is synced.
    pub(crate) async fn protx_collateral_pass(&self) {
        for wallet in self.hub.history.take_collateral_pending() {
            self.collateral_step(wallet).await;
        }
    }

    /// One wallet's [`Self::lock_own_collaterals`]. A wallet whose coin
    /// fence a build holds, or whose step failed, is marked pending again
    /// for the next poll, unless it or the session is gone.
    async fn collateral_step(&self, wallet: WalletId) {
        match self.lock_own_collaterals(wallet).await {
            Ok(true) => {}
            Ok(false) => self.hub.history.rearm_collateral(wallet),
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "locking masternode collaterals");
                if !matches!(
                    e,
                    MasternodesError::Engine(
                        EngineError::WalletNotFound(_) | EngineError::NetworkNotOpen(_)
                    )
                ) {
                    self.hub.history.rearm_collateral(wallet);
                }
            }
        }
    }

    /// Locks each of the wallet's masternodes' collateral that the wallet
    /// holds unspent ([`Self::lock_collaterals`]), after unlocking what
    /// conflicted rows locked. Core locks a ProRegTx's own collateral when
    /// the transaction is added and every listed collateral at load
    /// (v24.0.0 `wallet/wallet.cpp:1141`, `:2580-2590`). An external
    /// collateral holding its type's amount is locked while its ProRegTx is
    /// unconfirmed too, sooner than Core's load; one that never confirms
    /// leaves it locked until `release-collateral`, or "Unlock unspent"
    /// once it is abandoned. That is the safe side. The collateral of a
    /// ProRegTx the history keeps out of sight (a contact's record) is
    /// locked with them. `false` when a build holds the wallet's coin
    /// fence, so nothing ran.
    async fn lock_own_collaterals(&self, wallet: WalletId) -> Result<bool, MasternodesError> {
        let Some(fence) = self.try_coin_fence(wallet).await? else {
            return Ok(false);
        };
        #[cfg(test)]
        crash_at(self, CrashPoint::LockStep)?;
        self.release_conflicted_collaterals(wallet).await?;
        let (m, hub) = (self.manager()?, Arc::clone(&self.hub));
        let collaterals: Vec<(OutPoint, u64)> = tokio::task::spawn_blocking(move || {
            super::super::follow::wallet_masternodes(&m, &hub.history, &wallet)
                .map(|(owned, _)| {
                    (owned.iter())
                        .filter_map(|o| {
                            Some((o.collateral?, collateral_amount(o.record.is_evonode)))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
                .into_iter()
                .chain(hub.history.hidden_registration_collaterals(&wallet))
                .collect()
        })
        .await?;
        if !collaterals.is_empty() {
            let w = self.wallet(&wallet).await?;
            let coins = read_coins(&crate::wallet_read::info_of(&w).await?.core_wallet).coins;
            self.lock_collaterals(&fence, collaterals, &coins).await?;
        }
        Ok(true)
    }

    /// Locks each of `collaterals` (an outpoint and the amount its type
    /// needs) that is one of the fenced wallet's unspent `coins` holding that
    /// amount, unless locked already or released by the user
    /// (`release-collateral`, D9). Core accepts no other amount, so a
    /// ProRegTx naming another coin locks nothing. The follower's step and
    /// every build's coin read ([`Self::coin_snapshot`]) run it under the
    /// fence they hold.
    pub(crate) async fn lock_collaterals(
        &self,
        fence: &CoinFence,
        collaterals: Vec<(OutPoint, u64)>,
        coins: &[WalletCoin],
    ) -> Result<(), EngineError> {
        let wallet = fence.wallet_id();
        let held: Vec<OutPoint> = (collaterals.into_iter())
            .filter(|(o, amount)| {
                (coins.iter()).any(|c| c.utxo.outpoint == *o && c.utxo.value() == *amount)
            })
            .map(|(o, _)| o)
            .collect();
        if held.is_empty() {
            return Ok(());
        }
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
            state_of(
                &r,
                &history,
                &HashSet::from([o]),
                &Beats::on_chain(&history, None)
            ),
            ProTxOperationState::NeedsSignature
        );
        assert_eq!(
            state_of(
                &r,
                &history,
                &HashSet::new(),
                &Beats::on_chain(&history, None)
            ),
            ProTxOperationState::InputsSpent
        );
    }

    #[test]
    fn signed_rows_read_signed_not_sent_or_sent() {
        let mut r = row();
        r.signed_tx = Some(vec![0xff]);
        r.txid = Some("x".into());
        let (h, u) = (HashMap::new(), HashSet::new());
        assert_eq!(
            state_of(&r, &h, &u, &Beats::on_chain(&h, None)),
            ProTxOperationState::Signed
        );
        r.last_error = Some("send.no_peers".into());
        assert_eq!(
            state_of(&r, &h, &u, &Beats::on_chain(&h, None)),
            ProTxOperationState::NotSent
        );
        r.sent_at = Some(1);
        assert_eq!(
            state_of(&r, &h, &u, &Beats::on_chain(&h, None)),
            ProTxOperationState::Sent
        );
    }

    #[test]
    fn ep_t2_overdue_uses_only_current_tip_and_a_current_list() {
        let sent = 20_000;
        let check = |delta, synced, list, tip| overdue(sent, synced, list, tip, Some(sent + delta));
        assert!(!check(10_799, true, Some(100), Some(100)));
        assert!(check(10_800, true, Some(100), Some(100)));
        assert!(!check(7_200, true, Some(100), Some(100)));
        assert!(!overdue(sent, true, Some(100), Some(100), None));
        assert!(!check(10_800, true, Some(98), Some(100)));
        assert!(check(10_800, true, Some(99), Some(100)));
        assert!(!check(10_800, false, Some(100), Some(100)));
        assert!(!check(10_800, true, None, Some(100)));
        assert!(!check(10_800, true, Some(100), None));
        assert!(
            tip_overdue(sent, Some(sent + 10_800)),
            "abandon does not wait on a list"
        );
        assert!(
            !overdue(sent, true, Some(90), Some(90), Some(sent - 1)),
            "reorg time is authoritative"
        );
        assert!(check(10_800, true, Some(u32::MAX), Some(u32::MAX)));
    }

    #[test]
    fn ep_conflict_of_preserves_height_state_and_exceptions() {
        use dashcore::hashes::Hash;
        use dashcore::{ScriptBuf, TxIn, TxOut};
        use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
        let input = OutPoint::new(Txid::from_byte_array([7; 32]), 0);
        let tx = |value| Transaction {
            version: 3,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: input,
                script_sig: ScriptBuf::new(),
                sequence: u32::MAX,
                witness: Default::default(),
            }],
            output: vec![TxOut {
                value,
                script_pubkey: ScriptBuf::new(),
            }],
            special_transaction_payload: None,
        };
        let own = tx(1);
        let candidates = [tx(2), tx(3), tx(4)];
        let entry = |tx: Transaction, height: Option<u32>| TxEntry {
            tx,
            context: height.map_or(TransactionContext::Mempool, |h| {
                TransactionContext::InBlock(BlockInfo::new(h, dashcore::BlockHash::all_zeros(), 0))
            }),
            instant_locked: false,
            first_seen: None,
            own_inputs: Default::default(),
            abandoned: false,
        };
        let original = |history: &HashMap<Txid, TxEntry>, except: Option<&Txid>| {
            history
                .iter()
                .filter(|(id, e)| {
                    Some(*id) != except && e.tx.input.iter().any(|i| i.previous_output == input)
                })
                .filter_map(|(_, e)| e.context.block_info().map(|b| b.height()))
                .min()
        };
        for order in [[0, 1, 2], [2, 1, 0], [1, 0, 2]] {
            let mut history = HashMap::new();
            for i in order {
                history.insert(
                    candidates[i].txid(),
                    entry(candidates[i].clone(), Some(if i == 2 { 8 } else { 7 })),
                );
            }
            history.insert(own.txid(), entry(own.clone(), None));
            let final_ = Beats::on_chain(&history, Some(9));
            for except in [
                None,
                Some(own.txid()),
                Some(candidates[0].txid()),
                Some(candidates[2].txid()),
            ] {
                assert_eq!(
                    conflict_height(&history, &[input], except.as_ref(), &final_),
                    original(&history, except.as_ref())
                );
            }
            let preferred = candidates[1].txid();
            assert_eq!(
                conflict_of(&history, &[input], Some(&own.txid()), &[preferred], &final_),
                Some((preferred, 7))
            );
            assert_eq!(
                conflict_of(
                    &history,
                    &[input],
                    Some(&own.txid()),
                    &[candidates[2].txid()],
                    &final_
                )
                .unwrap()
                .1,
                7
            );
            assert_eq!(
                conflict_of(&history, &[input], Some(&own.txid()), &[], &final_)
                    .unwrap()
                    .0,
                candidates[0].txid().min(candidates[1].txid())
            );
            let r = MasternodeOpRow {
                signed_tx: Some(dashcore::consensus::serialize(&own)),
                sent_at: Some(1),
                ..row()
            };
            assert_eq!(
                state_of(&r, &history, &HashSet::new(), &final_),
                ProTxOperationState::Conflicted
            );
            assert_eq!(
                state_of(
                    &r,
                    &history,
                    &HashSet::new(),
                    &Beats::on_chain(&history, None)
                ),
                ProTxOperationState::Sent,
                "no ChainLock"
            );
            history.insert(own.txid(), entry(own.clone(), Some(9)));
            assert_eq!(
                state_of(&r, &history, &HashSet::new(), &final_),
                ProTxOperationState::Confirmed { height: 9 }
            );
        }
    }

    /// #740: a row is beaten through a parent the history's descendant
    /// rule beats, at the lowest beating block, whether the history holds
    /// the row's transaction or not and whether the parent is abandoned;
    /// never by a rival only in the mempool, never through its parent when
    /// the row is InstantSend-locked, and through an InstantSend-locked
    /// parent only once a ChainLock covers the block. #954: only by a block
    /// the header chain holds under a ChainLock, and an InstantSend-locked
    /// row's own input spent in a plain block beats it only once that
    /// block is ChainLocked.
    #[test]
    fn mg740_a_row_is_beaten_through_a_beaten_parent_by_the_history_rule() {
        use dashcore::hashes::Hash;
        use dashcore::{ScriptBuf, TxIn, TxOut};
        use key_wallet::transaction_checking::{BlockInfo, TransactionContext};
        let spend = |input: OutPoint, value| Transaction {
            version: 3,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: input,
                ..Default::default()
            }],
            output: vec![TxOut {
                value,
                script_pubkey: ScriptBuf::new(),
            }],
            special_transaction_payload: None,
        };
        let entry = |tx: &Transaction, height: Option<u32>, locked: bool| TxEntry {
            tx: tx.clone(),
            context: height.map_or(TransactionContext::Mempool, |h| {
                TransactionContext::InBlock(BlockInfo::new(h, dashcore::BlockHash::all_zeros(), 0))
            }),
            instant_locked: locked,
            first_seen: None,
            own_inputs: Default::default(),
            abandoned: false,
        };
        let input = OutPoint::new(Txid::from_byte_array([5; 32]), 0);
        let p = spend(input, 10);
        let k = spend(OutPoint::new(p.txid(), 0), 9);
        let (rival, early) = (spend(input, 8), spend(input, 7));
        let r = MasternodeOpRow {
            signed_tx: Some(dashcore::consensus::serialize(&k)),
            sent_at: Some(1),
            ..row()
        };
        let k_inputs = [OutPoint::new(p.txid(), 0)];
        let read_with = |history: &HashMap<Txid, TxEntry>, beats: &Beats| {
            (
                beaten_height(history, beats, &k_inputs, Some(&k.txid())),
                state_of(&r, history, &HashSet::new(), beats),
            )
        };
        let read = |history: &HashMap<Txid, TxEntry>, chainlock| {
            read_with(history, &Beats::on_chain(history, chainlock))
        };
        let sent = (None, ProTxOperationState::Sent);
        let beaten = |h| (Some(h), ProTxOperationState::Conflicted);

        let mut history = HashMap::from([(p.txid(), entry(&p, None, false))]);
        history.insert(rival.txid(), entry(&rival, None, false));
        let cl = Some(20);
        assert_eq!(read(&history, cl), sent, "a rival in the mempool");
        history.insert(rival.txid(), entry(&rival, Some(9), false));
        assert_eq!(read(&history, None), sent, "no ChainLock");
        assert_eq!(read(&history, Some(8)), sent, "above the ChainLock");
        assert_eq!(read(&history, cl), beaten(9), "K not in the history");
        let mut stale = Beats::on_chain(&history, cl).finality;
        stale
            .active
            .insert(9, dashcore::BlockHash::from_byte_array([9; 32]));
        let stale = Beats::of(&history, cl, stale);
        assert_eq!(
            read_with(&history, &stale),
            sent,
            "a block the chain replaced"
        );
        history.insert(k.txid(), entry(&k, None, false));
        assert_eq!(read(&history, cl), beaten(9));
        history.insert(early.txid(), entry(&early, Some(7), false));
        assert_eq!(read(&history, cl), beaten(7), "the lowest block");
        history.get_mut(&p.txid()).unwrap().abandoned = true;
        assert_eq!(read(&history, cl), beaten(7), "P abandoned");

        history.insert(k.txid(), entry(&k, None, true));
        assert_eq!(read(&history, cl), sent, "K InstantSend-locked");
        let direct = spend(OutPoint::new(p.txid(), 0), 6);
        history.insert(direct.txid(), entry(&direct, Some(12), false));
        assert_eq!(read(&history, Some(11)), sent, "K locked, a plain block");
        assert_eq!(
            read(&history, Some(12)),
            beaten(12),
            "K locked, ChainLocked"
        );
        history.remove(&direct.txid());

        history.insert(k.txid(), entry(&k, None, false));
        history.insert(p.txid(), entry(&p, None, true));
        assert_eq!(read(&history, None), sent, "P locked, no ChainLock");
        assert_eq!(read(&history, Some(6)), sent, "below the ChainLock");
        assert_eq!(read(&history, Some(7)), beaten(7), "P locked, ChainLocked");
    }
}
