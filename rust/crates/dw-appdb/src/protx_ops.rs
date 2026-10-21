//! The single writer of `masternode_ops` (MG-04 §5): a wallet's ProTx
//! operations. Every delete is `... AND sent_at IS NULL`, so a row whose
//! bytes may have been handed to the network is never deleted (MG-06 r1
//! M-1, MG-04 D6); the writes a crash must not lose are synced
//! ([`AppDb::durably`]).

use rusqlite::{OptionalExtension, Row, params};

use crate::{AppDb, Result};

/// One `masternode_ops` row. `kind`, `terms` and `request` are the
/// engine's encodings; this crate does not interpret them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeOpRow {
    pub id: String,
    pub kind: String,
    pub terms: String,
    pub request: String,
    pub unsigned_tx: Option<Vec<u8>>,
    pub sign_address: Option<String>,
    pub sign_message: Option<String>,
    /// JSON array of `"txid:vout"`.
    pub fee_inputs: String,
    pub signed_tx: Option<Vec<u8>>,
    pub txid: Option<String>,
    pub pro_tx_hash: Option<String>,
    pub locked_collateral: bool,
    pub fee: u64,
    pub created_at: u64,
    pub signed_at: Option<u64>,
    pub sent_at: Option<u64>,
    pub last_error: Option<String>,
}

/// How one hand-off attempt ended, as [`AppDb::end_masternode_op_attempt`]
/// records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttemptEnd {
    /// Accepted, or the outcome is unknown: `sent_at` stays.
    Out { error: Option<String> },
    /// Definitely not sent. `first`: no earlier attempt may have sent the
    /// bytes, so `sent_at` goes back to null; otherwise it stays.
    NotSent { first: bool, error: String },
}

const COLUMNS: &str = "id, kind, terms, request, unsigned_tx, sign_address, sign_message, \
     fee_inputs, signed_tx, txid, pro_tx_hash, locked_collateral, fee, created_at, signed_at, \
     sent_at, last_error";

fn opt_u64(v: Option<i64>) -> Option<u64> {
    v.and_then(|v| u64::try_from(v).ok())
}

fn read_row(r: &Row<'_>) -> rusqlite::Result<MasternodeOpRow> {
    Ok(MasternodeOpRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        terms: r.get(2)?,
        request: r.get(3)?,
        unsigned_tx: r.get(4)?,
        sign_address: r.get(5)?,
        sign_message: r.get(6)?,
        fee_inputs: r.get(7)?,
        signed_tx: r.get(8)?,
        txid: r.get(9)?,
        pro_tx_hash: r.get(10)?,
        locked_collateral: r.get::<_, i64>(11)? != 0,
        fee: u64::try_from(r.get::<_, i64>(12)?).unwrap_or(0),
        created_at: u64::try_from(r.get::<_, i64>(13)?).unwrap_or(0),
        signed_at: opt_u64(r.get(14)?),
        sent_at: opt_u64(r.get(15)?),
        last_error: r.get(16)?,
    })
}

fn sql_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl AppDb {
    /// Inserts a new operation with the coin locks it holds (`locks`,
    /// `(txid, vout)`, locked by the user), in one synced transaction: an
    /// External row before its fee inputs are reported locked, a signed
    /// row before any hand-off (save before broadcast). Neither the row nor
    /// a lock is ever kept without the other (MG-04 D53). The row's
    /// `sent_at` must be null.
    pub fn insert_masternode_op_durable(
        &self,
        wallet_id: &str,
        row: &MasternodeOpRow,
        locks: &[(String, u32)],
        now: u64,
    ) -> Result<()> {
        debug_assert!(row.sent_at.is_none());
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            for (txid, vout) in locks {
                Self::lock_manual_on(&tx, wallet_id, txid, *vout, now)?;
            }
            tx.execute(
                &format!(
                    "INSERT INTO masternode_ops (wallet_id, {COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                             ?16, NULL, ?17)"
                ),
                params![
                    wallet_id,
                    row.id,
                    row.kind,
                    row.terms,
                    row.request,
                    row.unsigned_tx,
                    row.sign_address,
                    row.sign_message,
                    row.fee_inputs,
                    row.signed_tx,
                    row.txid,
                    row.pro_tx_hash,
                    i64::from(row.locked_collateral),
                    sql_u64(row.fee),
                    sql_u64(row.created_at),
                    row.signed_at.map(sql_u64),
                    row.last_error,
                ],
            )?;
            Ok(tx.commit()?)
        })
    }

    /// The wallet's operations, newest first.
    pub fn masternode_ops(&self, wallet_id: &str) -> Result<Vec<MasternodeOpRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM masternode_ops WHERE wallet_id = ?1
             ORDER BY created_at DESC, id"
        ))?;
        let rows = stmt.query_map([wallet_id], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn masternode_op(&self, wallet_id: &str, id: &str) -> Result<Option<MasternodeOpRow>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM masternode_ops WHERE wallet_id = ?1 AND id = ?2"),
                params![wallet_id, id],
                read_row,
            )
            .optional()?)
    }

    /// Writes an External row's signed bytes, synced, once: `false` when
    /// the row is gone or already signed.
    pub fn sign_masternode_op_durable(
        &self,
        wallet_id: &str,
        id: &str,
        signed_tx: &[u8],
        txid: &str,
        pro_tx_hash: &str,
        signed_at: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            Ok(conn.execute(
                "UPDATE masternode_ops SET signed_tx = ?3, txid = ?4, pro_tx_hash = ?5,
                     signed_at = ?6, last_error = NULL
                 WHERE wallet_id = ?1 AND id = ?2 AND signed_tx IS NULL",
                params![
                    wallet_id,
                    id,
                    signed_tx,
                    txid,
                    pro_tx_hash,
                    sql_u64(signed_at)
                ],
            )? == 1)
        })
    }

    /// An External row's new prepared transaction (refresh): only while
    /// it is unsigned. `locks` (its new inputs) are written with it in one
    /// transaction, and not at all when the row isn't updated (MG-04 D53).
    #[expect(clippy::too_many_arguments, reason = "one prepared transaction")]
    pub fn reprepare_masternode_op(
        &self,
        wallet_id: &str,
        id: &str,
        unsigned_tx: &[u8],
        sign_message: &str,
        fee_inputs: &str,
        fee: u64,
        locks: &[(String, u32)],
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let updated = tx.execute(
                "UPDATE masternode_ops SET unsigned_tx = ?3, sign_message = ?4, fee_inputs = ?5,
                     fee = ?6, last_error = NULL
                 WHERE wallet_id = ?1 AND id = ?2 AND signed_tx IS NULL",
                params![
                    wallet_id,
                    id,
                    unsigned_tx,
                    sign_message,
                    fee_inputs,
                    sql_u64(fee)
                ],
            )? == 1;
            if !updated {
                return Ok(false);
            }
            for (txid, vout) in locks {
                Self::lock_manual_on(&tx, wallet_id, txid, *vout, now)?;
            }
            tx.commit()?;
            Ok(true)
        })
    }

    /// Marks a hand-off as starting, synced, before the bytes can leave:
    /// `sent_at` is set unless an earlier attempt set it. Returns whether
    /// one had (`None` when the row is gone or unsigned).
    pub fn begin_masternode_op_attempt(
        &self,
        wallet_id: &str,
        id: &str,
        now: u64,
    ) -> Result<Option<bool>> {
        self.durably(|conn| {
            let prior: Option<Option<i64>> = conn
                .query_row(
                    "SELECT sent_at FROM masternode_ops
                     WHERE wallet_id = ?1 AND id = ?2 AND signed_tx IS NOT NULL",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(prior) = prior else {
                return Ok(None);
            };
            conn.execute(
                "UPDATE masternode_ops SET sent_at = COALESCE(sent_at, ?3)
                 WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id, sql_u64(now)],
            )?;
            Ok(Some(prior.is_some()))
        })
    }

    /// Records how an attempt ended. A first attempt that definitely did
    /// not send clears `sent_at` again; nothing else does.
    pub fn end_masternode_op_attempt(
        &self,
        wallet_id: &str,
        id: &str,
        end: &AttemptEnd,
    ) -> Result<()> {
        let conn = self.conn();
        match end {
            AttemptEnd::Out { error } => conn.execute(
                "UPDATE masternode_ops SET last_error = ?3 WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id, error],
            )?,
            AttemptEnd::NotSent { first: true, error } => conn.execute(
                "UPDATE masternode_ops SET sent_at = NULL, last_error = ?3
                 WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id, error],
            )?,
            AttemptEnd::NotSent {
                first: false,
                error,
            } => conn.execute(
                "UPDATE masternode_ops SET last_error = ?3 WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id, error],
            )?,
        };
        Ok(())
    }

    /// Deletes an operation nothing of which may be out: `false` (and
    /// nothing deleted) when its `sent_at` is set or it is gone.
    pub fn delete_unsent_masternode_op(&self, wallet_id: &str, id: &str) -> Result<bool> {
        let conn = self.conn();
        Ok(conn.execute(
            "DELETE FROM masternode_ops WHERE wallet_id = ?1 AND id = ?2 AND sent_at IS NULL",
            params![wallet_id, id],
        )? == 1)
    }

    /// Unlocks the collateral `txid:vout` operation `id` locked and
    /// forgets that it did, in one transaction: its transaction is
    /// conflicted, so it never confirms (MG-04 D54).
    pub fn release_masternode_op_collateral(
        &self,
        wallet_id: &str,
        id: &str,
        txid: &str,
        vout: u32,
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        Self::unlock_on(&tx, wallet_id, txid, vout, now)?;
        tx.execute(
            "UPDATE masternode_ops SET locked_collateral = 0 WHERE wallet_id = ?1 AND id = ?2",
            params![wallet_id, id],
        )?;
        Ok(tx.commit()?)
    }

    /// Forgets the inputs an operation locked, once the engine unlocked
    /// them (its transaction is in a block or conflicted).
    pub fn clear_masternode_op_inputs(&self, wallet_id: &str, id: &str) -> Result<()> {
        self.conn().execute(
            "UPDATE masternode_ops SET fee_inputs = '[]' WHERE wallet_id = ?1 AND id = ?2",
            params![wallet_id, id],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";

    fn row(id: &str, signed: bool) -> MasternodeOpRow {
        MasternodeOpRow {
            id: id.into(),
            kind: "registration".into(),
            terms: "{}".into(),
            request: "{}".into(),
            unsigned_tx: (!signed).then(|| vec![1]),
            sign_address: None,
            sign_message: None,
            fee_inputs: "[]".into(),
            signed_tx: signed.then(|| vec![2]),
            txid: signed.then(|| "t".into()),
            pro_tx_hash: None,
            locked_collateral: false,
            fee: 7,
            created_at: 1,
            signed_at: signed.then_some(2),
            sent_at: None,
            last_error: None,
        }
    }

    #[test]
    fn a_row_that_may_be_out_is_never_deleted() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_masternode_op_durable(W, &row("a", true), &[], 1)
            .unwrap();
        assert_eq!(
            db.begin_masternode_op_attempt(W, "a", 5).unwrap(),
            Some(false)
        );
        assert!(!db.delete_unsent_masternode_op(W, "a").unwrap());
        // A second attempt keeps the first's time.
        assert_eq!(
            db.begin_masternode_op_attempt(W, "a", 9).unwrap(),
            Some(true)
        );
        assert_eq!(db.masternode_op(W, "a").unwrap().unwrap().sent_at, Some(5));
        // A later definite refusal doesn't clear it.
        db.end_masternode_op_attempt(
            W,
            "a",
            &AttemptEnd::NotSent {
                first: false,
                error: "x".into(),
            },
        )
        .unwrap();
        assert_eq!(db.masternode_op(W, "a").unwrap().unwrap().sent_at, Some(5));
        assert!(!db.delete_unsent_masternode_op(W, "a").unwrap());
        assert!(db.masternode_op(W, "a").unwrap().is_some());
    }

    #[test]
    fn a_first_attempt_definitely_unsent_can_be_discarded() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_masternode_op_durable(W, &row("a", true), &[], 1)
            .unwrap();
        db.begin_masternode_op_attempt(W, "a", 5).unwrap();
        db.end_masternode_op_attempt(
            W,
            "a",
            &AttemptEnd::NotSent {
                first: true,
                error: "send.no_peers".into(),
            },
        )
        .unwrap();
        let r = db.masternode_op(W, "a").unwrap().unwrap();
        assert_eq!(
            (r.sent_at, r.last_error.as_deref()),
            (None, Some("send.no_peers"))
        );
        assert!(db.delete_unsent_masternode_op(W, "a").unwrap());
        assert!(db.masternode_op(W, "a").unwrap().is_none());
    }

    #[test]
    fn an_external_row_is_signed_once_and_only_unsigned_rows_reprepare() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_masternode_op_durable(W, &row("e", false), &[], 1)
            .unwrap();
        assert_eq!(db.begin_masternode_op_attempt(W, "e", 1).unwrap(), None);
        assert!(
            db.reprepare_masternode_op(W, "e", &[3], "m", "[]", 9, &[], 1)
                .unwrap()
        );
        assert!(
            db.sign_masternode_op_durable(W, "e", &[4], "t", "t", 3)
                .unwrap()
        );
        assert!(
            !db.sign_masternode_op_durable(W, "e", &[5], "u", "u", 4)
                .unwrap()
        );
        assert!(
            !db.reprepare_masternode_op(W, "e", &[6], "m", "[]", 9, &[], 1)
                .unwrap()
        );
        let r = db.masternode_op(W, "e").unwrap().unwrap();
        assert_eq!((r.signed_tx, r.fee), (Some(vec![4]), 9));
        // The CHECKs: sent needs signed bytes; signed bytes need a txid.
        let conn = db.conn();
        assert!(
            conn.execute(
                "INSERT INTO masternode_ops (id, wallet_id, kind, terms, request, fee, created_at,
                     sent_at) VALUES ('x', 'aa', 'k', '{}', '{}', 1, 1, 1)",
                [],
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "INSERT INTO masternode_ops (id, wallet_id, kind, terms, request, fee, created_at,
                     signed_tx) VALUES ('y', 'aa', 'k', '{}', '{}', 1, 1, x'00')",
                [],
            )
            .is_err()
        );
    }

    fn active_locks(db: &AppDb) -> Vec<(String, u32)> {
        let mut locks: Vec<_> = (db.locks(W).unwrap().into_iter())
            .filter(|r| r.is_locked())
            .map(|r| (r.txid, r.vout))
            .collect();
        locks.sort();
        locks
    }

    /// MG-04 D53 (r2 n-2): a row and its locks are one write. A failure
    /// writing either leaves neither, so no crash keeps a row without its
    /// locks, or locks with no row to settle them.
    #[test]
    fn a_row_and_its_locks_are_written_together() {
        let db = AppDb::open_in_memory().unwrap();
        let locks = [("t".to_string(), 0), ("u".to_string(), 1)];
        db.conn()
            .execute_batch(
                "CREATE TEMP TRIGGER no_lock BEFORE INSERT ON utxo_locks
                 BEGIN SELECT RAISE(ABORT, 'lock refused'); END;",
            )
            .unwrap();
        assert!(
            db.insert_masternode_op_durable(W, &row("a", true), &locks, 1)
                .is_err()
        );
        assert!(
            db.masternode_op(W, "a").unwrap().is_none(),
            "no row without its locks"
        );
        db.conn()
            .execute_batch(
                "DROP TRIGGER no_lock;
                 CREATE TEMP TRIGGER no_row BEFORE INSERT ON masternode_ops
                 BEGIN SELECT RAISE(ABORT, 'row refused'); END;",
            )
            .unwrap();
        assert!(
            db.insert_masternode_op_durable(W, &row("a", true), &locks, 1)
                .is_err()
        );
        assert!(active_locks(&db).is_empty(), "no lock without its row");
        db.conn().execute_batch("DROP TRIGGER no_row;").unwrap();
        db.insert_masternode_op_durable(W, &row("a", true), &locks, 1)
            .unwrap();
        assert!(db.masternode_op(W, "a").unwrap().is_some());
        assert_eq!(active_locks(&db), locks);
    }

    /// MG-04 D53 (r3 n-2): a refresh's new inputs are locked in the write
    /// that puts them in the row: a failure writing either, or a row that
    /// is signed by now, leaves neither.
    #[test]
    fn a_refresh_locks_its_inputs_with_the_row() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_masternode_op_durable(W, &row("e", false), &[], 1)
            .unwrap();
        let locks = [("n".to_string(), 0), ("o".to_string(), 3)];
        let reprepare =
            |db: &AppDb| db.reprepare_masternode_op(W, "e", &[3], "m", "[]", 9, &locks, 2);
        db.conn()
            .execute_batch(
                "CREATE TEMP TRIGGER no_lock BEFORE INSERT ON utxo_locks
                 BEGIN SELECT RAISE(ABORT, 'lock refused'); END;",
            )
            .unwrap();
        assert!(reprepare(&db).is_err());
        assert_eq!(
            db.masternode_op(W, "e").unwrap().unwrap().fee,
            row("e", false).fee
        );
        db.conn()
            .execute_batch(
                "DROP TRIGGER no_lock;
                 CREATE TEMP TRIGGER no_row BEFORE UPDATE ON masternode_ops
                 BEGIN SELECT RAISE(ABORT, 'row refused'); END;",
            )
            .unwrap();
        assert!(reprepare(&db).is_err());
        assert!(active_locks(&db).is_empty(), "no lock without its row");
        db.conn().execute_batch("DROP TRIGGER no_row;").unwrap();
        assert!(reprepare(&db).unwrap());
        assert_eq!(db.masternode_op(W, "e").unwrap().unwrap().fee, 9);
        assert_eq!(active_locks(&db), locks);

        db.unlock(W, "n", 0, 3).unwrap();
        db.unlock(W, "o", 3, 3).unwrap();
        db.sign_masternode_op_durable(W, "e", &[4], "t", "t", 3)
            .unwrap();
        assert!(!reprepare(&db).unwrap());
        assert!(
            active_locks(&db).is_empty(),
            "a signed row takes no new locks"
        );
    }

    /// MG-04 D54: a conflicted operation's collateral lock goes with its
    /// flag, and nothing else of the row or the wallet's locks.
    #[test]
    fn a_released_collateral_is_unlocked_and_forgotten() {
        let db = AppDb::open_in_memory().unwrap();
        let mut r = row("a", true);
        r.locked_collateral = true;
        let locks = [("c".to_string(), 0), ("f".to_string(), 2)];
        db.insert_masternode_op_durable(W, &r, &locks, 1).unwrap();
        db.release_masternode_op_collateral(W, "a", "c", 0, 2)
            .unwrap();
        let after = db.masternode_op(W, "a").unwrap().unwrap();
        assert!(!after.locked_collateral);
        assert_eq!(active_locks(&db), [("f".to_string(), 2)]);
    }
}
