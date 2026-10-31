//! MG-04's side of MG-13's payout-change barriers (MG-13 P16): the app-wide
//! `mn_payout_barriers` rows that hold a masternode withdrawal while a
//! payout change the app signed may not have reached Platform.
//!
//! - A ProUpRegTx that may change the payout writes its barrier in the
//!   transaction that saves its signed row (`update_registrar`, through
//!   [`NetworkSession::save_operation_with_barrier`]).
//! - `discard` deletes the unsent row and its barrier in one transaction
//!   (MG-04 D6: only while `sent_at IS NULL`).
//! - Rows signed before barriers existed get theirs once per app database,
//!   at session open ([`backfill_payout_barriers`]).
//!
//! Clearing is the withdrawal quote's (MG-13 P16's terms);
//! [`NetworkSession::barrier_operation_state`] is what it reads of MG-04.

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::{Network, ScriptBuf};
use dw_appdb::{AppDb, MasternodeOpRow, NewPayoutBarrier, PayoutBarrierRow};

use super::super::MasternodesError;
use super::super::protx_records::{ProTxOperationKind, ProTxOperationState, ProTxTerms};
use super::ops::{kind_name, signed_tx};
use crate::send::l1_address;
use crate::{EngineError, NetworkSession, WalletId};

/// The payout script of a signed `update_registrar` row's ProUpRegTx: the
/// new payout its barrier holds. `None` for any other row, an unsigned
/// one, or bytes that don't decode as a ProUpRegTx.
pub(crate) fn signed_payout_script(row: &MasternodeOpRow) -> Option<ScriptBuf> {
    if row.kind != kind_name(ProTxOperationKind::UpdateRegistrar) {
        return None;
    }
    match signed_tx(row)?.special_transaction_payload? {
        TransactionPayload::ProviderUpdateRegistrarPayloadType(p) => Some(p.script_payout),
        _ => None,
    }
}

/// The barrier the backfill writes for `wallet_id`'s `row`: one for every
/// signed `update_registrar` row, since a stored row doesn't say what the
/// payout was before it, so no kept payout can be ruled in (MG-13 P16; a
/// settled one clears on the quote's first check). The script is the
/// signed payload's, else the stored terms' payout address. `Err` names
/// what is missing when a barrier is due and can't be built.
fn backfill_barrier(
    wallet_id: &str,
    row: &MasternodeOpRow,
    network: Network,
) -> Result<Option<NewPayoutBarrier>, String> {
    if row.kind != kind_name(ProTxOperationKind::UpdateRegistrar) || row.signed_tx.is_none() {
        return Ok(None);
    }
    let script = signed_payout_script(row).or_else(|| {
        let terms: ProTxTerms = serde_json::from_str(&row.terms).ok()?;
        let address = l1_address(&terms.payout_address, network).ok()?;
        Some(address.script_pubkey())
    });
    let missing = match (&script, &row.txid, &row.pro_tx_hash) {
        (None, ..) => "its new payout script",
        (_, None, _) => "its txid",
        (_, _, None) => "its ProTx hash",
        (Some(script), Some(txid), Some(pro_tx_hash)) => {
            return Ok(Some(NewPayoutBarrier {
                pro_tx_hash: pro_tx_hash.clone(),
                op_id: row.id.clone(),
                created_by: wallet_id.to_owned(),
                txid: txid.clone(),
                payout_script: script.to_bytes(),
            }));
        }
    };
    Err(format!(
        "the barrier of signed update {} of wallet {wallet_id} lacks {missing}",
        row.id
    ))
}

/// The barriers of every wallet's ProUpRegTxs signed before barriers
/// existed, written once per app database behind its marker
/// (`backfill_payout_barriers_durable`, MG-13 P16, r3 n3). Runs at session
/// open, before anything can quote a withdrawal; a marker already set
/// writes nothing. Whether barriers are complete: on any failure (a row a
/// barrier is due for and can't be built included) it logs, writes
/// nothing, leaves the marker unset so the next open runs it again, and
/// returns `false`, which the quote reads as `payout_change_pending`.
pub(crate) fn backfill_payout_barriers(db: &AppDb, network: Network, now: u64) -> bool {
    let run = || -> Result<Option<usize>, String> {
        if db.payout_barriers_backfilled().map_err(|e| e.to_string())? {
            return Ok(None);
        }
        let mut barriers = Vec::new();
        for wallet_id in db.masternode_op_wallets().map_err(|e| e.to_string())? {
            for row in db.masternode_ops(&wallet_id).map_err(|e| e.to_string())? {
                barriers.extend(backfill_barrier(&wallet_id, &row, network)?);
            }
        }
        db.backfill_payout_barriers_durable(&barriers, now)
            .map_err(|e| e.to_string())
    };
    match run() {
        Ok(Some(written)) => {
            tracing::info!(written, "payout-change barriers backfilled (MG-13 P16)");
            true
        }
        Ok(None) => true,
        Err(error) => {
            tracing::error!(%error, "payout-change barrier backfill failed; masternode \
                withdrawals stay held until it succeeds at a later open (MG-13 P16)");
            false
        }
    }
}

impl NetworkSession {
    /// The state of `barrier`'s operation in its creating wallet's history,
    /// read as MG-04 reads every row (`Confirmed { height }`, `Conflicted`
    /// after D49's abandon, ...), for the quote's clearing terms (MG-13
    /// P16). `None` when that can't be read: the wallet isn't loaded (or
    /// was deleted), or no longer holds the row; then only Platform
    /// evidence clears the barrier.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "MG-13's quote reads it (commit 4)")
    )]
    pub(crate) async fn barrier_operation_state(
        &self,
        barrier: &PayoutBarrierRow,
    ) -> Result<Option<ProTxOperationState>, MasternodesError> {
        let Ok(wallet) = barrier.barrier.created_by.parse::<WalletId>() else {
            return Ok(None);
        };
        let (w, id) = (wallet.to_string(), barrier.barrier.op_id.clone());
        let Some(row) = self.appdb_op(move |db| db.masternode_op(&w, &id)).await? else {
            return Ok(None);
        };
        match self.op_state(wallet, &row).await {
            Err(MasternodesError::Engine(EngineError::WalletNotFound(_))) => Ok(None),
            state => state.map(Some),
        }
    }
}
