//! The single writer of `mn_payout_barriers` (MG-13 P16): app-wide
//! payout-change barriers, one per signed ProUpRegTx whose terms change the
//! payout. A barrier is written in the transaction that writes its
//! operation's signed bytes (`masternode_ops`, [`crate::protx_ops`]), deleted
//! by a clearing term ([`AppDb::clear_payout_barrier_durable`]) or with its
//! unsent operation in one transaction
//! ([`AppDb::discard_unsent_masternode_op_durable`], in `protx_ops`), and
//! never by a wallet
//! removal (fail closed). This crate does not read `terms`: the upgrade's
//! backfill from existing operations is the engine's
//! ([`AppDb::backfill_payout_barriers_durable`]).

use rusqlite::{Connection, Row, params};

use crate::fault_points::{MN_BARRIER_BACKFILL, MN_BARRIER_CLEAR};
use crate::{AppDb, GLOBAL_SCOPE, Result, fault_before, write_transaction};

/// The settings key (global scope) of the backfill's "done" marker, set in
/// the backfill's transaction.
pub const PAYOUT_BARRIERS_BACKFILLED: &str = "mg13.payout_barriers_backfilled";

/// A barrier to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPayoutBarrier {
    /// 64 lower-case hex, as displayed.
    pub pro_tx_hash: String,
    /// The `masternode_ops` row: its `id` and `wallet_id`.
    pub op_id: String,
    pub created_by: String,
    /// The operation's txid, 64 lower-case hex.
    pub txid: String,
    pub payout_script: Vec<u8>,
}

/// One `mn_payout_barriers` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutBarrierRow {
    pub id: i64,
    pub barrier: NewPayoutBarrier,
    pub created_at: u64,
}

const COLUMNS: &str = "id, pro_tx_hash, op_id, created_by, txid, payout_script, created_at";

fn read_row(r: &Row<'_>) -> rusqlite::Result<PayoutBarrierRow> {
    Ok(PayoutBarrierRow {
        id: r.get(0)?,
        barrier: NewPayoutBarrier {
            pro_tx_hash: r.get(1)?,
            op_id: r.get(2)?,
            created_by: r.get(3)?,
            txid: r.get(4)?,
            payout_script: r.get(5)?,
        },
        created_at: u64::try_from(r.get::<_, i64>(6)?).unwrap_or(0),
    })
}

fn sql_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// Writes `barrier` in the caller's transaction. With `keep_existing`, an
/// existing barrier of the same operation is kept (the backfill); else it is
/// an error. Only that conflict is skipped (not `OR IGNORE`, which would
/// drop a row its CHECKs refuse). The number of rows written.
pub(crate) fn write_barrier(
    conn: &Connection,
    barrier: &NewPayoutBarrier,
    now: u64,
    keep_existing: bool,
) -> rusqlite::Result<usize> {
    conn.execute(
        &format!(
            "INSERT INTO mn_payout_barriers
                 (pro_tx_hash, op_id, created_by, txid, payout_script, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6){}",
            if keep_existing {
                " ON CONFLICT (created_by, op_id) DO NOTHING"
            } else {
                ""
            }
        ),
        params![
            barrier.pro_tx_hash,
            barrier.op_id,
            barrier.created_by,
            barrier.txid,
            barrier.payout_script,
            sql_u64(now),
        ],
    )
}

impl AppDb {
    /// Every barrier, oldest first.
    pub fn payout_barriers(&self) -> Result<Vec<PayoutBarrierRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM mn_payout_barriers ORDER BY id"
        ))?;
        let rows = stmt.query_map([], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The masternode's barriers, oldest first (P4 (b)).
    pub fn payout_barriers_of(&self, pro_tx_hash: &str) -> Result<Vec<PayoutBarrierRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM mn_payout_barriers WHERE pro_tx_hash = ?1 ORDER BY id"
        ))?;
        let rows = stmt.query_map([pro_tx_hash], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Clears barrier `id` on a clearing term (Platform evidence, confirmed
    /// and covered, conflicted; P16), synced. Whether it existed.
    pub fn clear_payout_barrier_durable(&self, id: i64) -> Result<bool> {
        let cleared = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let cleared = tx.execute("DELETE FROM mn_payout_barriers WHERE id = ?1", [id])? == 1;
            fault_before(&tx, MN_BARRIER_CLEAR)?;
            tx.commit()?;
            Ok(cleared)
        })?;
        self.fault_after(MN_BARRIER_CLEAR)?;
        Ok(cleared)
    }

    /// The upgrade's backfill (MG-13 P16): one synced transaction that writes
    /// a barrier for each of `barriers` (the engine's choice of the existing
    /// payout-changing operations with signed bytes), keeping any barrier an
    /// operation already has, and sets the "done" marker
    /// ([`PAYOUT_BARRIERS_BACKFILLED`]). The number of barriers written;
    /// `None`, with nothing written, when the marker was already set (by
    /// another process, say).
    pub fn backfill_payout_barriers_durable(
        &self,
        barriers: &[NewPayoutBarrier],
        now: u64,
    ) -> Result<Option<usize>> {
        let written = self.durably(|conn| {
            let tx = write_transaction(conn)?;
            let done: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM settings_kv WHERE scope = ?1 AND key = ?2)",
                params![GLOBAL_SCOPE, PAYOUT_BARRIERS_BACKFILLED],
                |r| r.get(0),
            )?;
            if done {
                return Ok(None);
            }
            let mut written = 0;
            for barrier in barriers {
                written += write_barrier(&tx, barrier, now, true)?;
            }
            Self::put_setting(
                &tx,
                GLOBAL_SCOPE,
                PAYOUT_BARRIERS_BACKFILLED,
                Some(&now.to_string()),
            )?;
            fault_before(&tx, MN_BARRIER_BACKFILL)?;
            tx.commit()?;
            Ok(Some(written))
        })?;
        self.fault_after(MN_BARRIER_BACKFILL)?;
        Ok(written)
    }

    /// Whether the backfill ran ([`Self::backfill_payout_barriers_durable`]).
    pub fn payout_barriers_backfilled(&self) -> Result<bool> {
        Ok(self
            .setting(GLOBAL_SCOPE, PAYOUT_BARRIERS_BACKFILLED)?
            .is_some())
    }
}
