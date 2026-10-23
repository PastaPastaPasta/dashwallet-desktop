//! The single writer of `shared_mn_sessions` (MG-04b §9.1): a wallet's
//! shared-masternode sessions. Every delete is `... AND out_at IS NULL`, so
//! a row whose signed bytes may have left the wallet is never deleted
//! (MG-04b §9.2, MG-04 D6); every write a crash must not lose is synced
//! ([`AppDb::durably`]). A session's coins are persistent manual locks
//! written, replaced and released in the same transaction as its row
//! (MG-04 D53): the outpoints are the `reserved` column's, a JSON array of
//! `"txid:vout"`, which is the only record of what the session locked.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{AppDb, AppDbError, Result};

/// One `shared_mn_sessions` row. `kind`, `role` and the JSON columns are
/// the engine's encodings; this crate interprets only `reserved`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSessionRow {
    /// 64 lower-case hex.
    pub id: String,
    /// `registration`, `dissolve_together` or `key_change`.
    pub kind: String,
    /// `coordinator` or `participant`.
    pub role: String,
    /// The latest canonical message JSON.
    pub envelope: String,
    /// JSON: share index to keychain owner index.
    pub my_shares: String,
    /// JSON: this wallet's recorded inputs, their total and its change.
    pub contribution: Option<String>,
    /// JSON array of `"txid:vout"`: the outpoints locked for the session.
    pub reserved: String,
    pub approved_terms: Option<String>,
    pub base_keys: Option<String>,
    pub signed_tx: Option<Vec<u8>>,
    pub txid: Option<String>,
    pub out_at: Option<u64>,
    pub abandon_txid: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_error: Option<String>,
}

/// The mutable fields [`AppDb::update_shared_session_durable`] writes.
/// Every field replaces the stored one, except `signed_tx` and `txid`: a
/// `None` there keeps what is stored, so a write that does not know the
/// bytes cannot lose them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSessionUpdate {
    pub envelope: String,
    pub my_shares: String,
    pub contribution: Option<String>,
    pub approved_terms: Option<String>,
    pub signed_tx: Option<Vec<u8>>,
    pub txid: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: u64,
}

const COLUMNS: &str = "id, kind, role, envelope, my_shares, contribution, reserved, \
     approved_terms, base_keys, signed_tx, txid, out_at, abandon_txid, created_at, updated_at, \
     last_error";

fn opt_u64(v: Option<i64>) -> Option<u64> {
    v.and_then(|v| u64::try_from(v).ok())
}

fn read_row(r: &Row<'_>) -> rusqlite::Result<SharedSessionRow> {
    Ok(SharedSessionRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        role: r.get(2)?,
        envelope: r.get(3)?,
        my_shares: r.get(4)?,
        contribution: r.get(5)?,
        reserved: r.get(6)?,
        approved_terms: r.get(7)?,
        base_keys: r.get(8)?,
        signed_tx: r.get(9)?,
        txid: r.get(10)?,
        out_at: opt_u64(r.get(11)?),
        abandon_txid: r.get(12)?,
        created_at: u64::try_from(r.get::<_, i64>(13)?).unwrap_or(0),
        updated_at: u64::try_from(r.get::<_, i64>(14)?).unwrap_or(0),
        last_error: r.get(15)?,
    })
}

fn sql_u64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn corrupt(what: &str, value: &str) -> AppDbError {
    AppDbError::Corrupt(format!("shared_mn_sessions.{what} {value}"))
}

/// The outpoints of a `reserved` value: a JSON array of `"txid:vout"`.
fn outpoints(conn: &Connection, reserved: &str) -> Result<Vec<(String, u32)>> {
    let kind: Option<String> = conn
        .query_row("SELECT json_type(?1)", [reserved], |r| r.get(0))
        .map_err(|_| corrupt("reserved", reserved))?;
    if kind.as_deref() != Some("array") {
        return Err(corrupt("reserved", reserved));
    }
    let mut stmt = conn.prepare("SELECT value FROM json_each(?1)")?;
    let items: Vec<String> = stmt
        .query_map([reserved], |r| r.get(0))
        .map_err(|_| corrupt("reserved", reserved))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|_| corrupt("reserved", reserved))?;
    items
        .into_iter()
        .map(|item| {
            let (txid, vout) = item
                .rsplit_once(':')
                .ok_or_else(|| corrupt("reserved", &item))?;
            let vout = vout.parse().map_err(|_| corrupt("reserved", &item))?;
            Ok((txid.to_owned(), vout))
        })
        .collect()
}

/// Unlocks the outpoints of `reserved`, inside a caller's transaction.
fn unlock_reserved(conn: &Connection, wallet_id: &str, reserved: &str, now: u64) -> Result<()> {
    for (txid, vout) in outpoints(conn, reserved)? {
        AppDb::unlock_on(conn, wallet_id, &txid, vout, now)?;
    }
    Ok(())
}

/// Locks the outpoints of `reserved`, inside a caller's transaction.
fn lock_reserved(conn: &Connection, wallet_id: &str, reserved: &str, now: u64) -> Result<()> {
    for (txid, vout) in outpoints(conn, reserved)? {
        AppDb::lock_manual_on(conn, wallet_id, &txid, vout, now)?;
    }
    Ok(())
}

impl AppDb {
    /// Inserts a new session and locks the outpoints of its `reserved`
    /// column (locked by the user), in one synced transaction: neither the
    /// row nor a lock is ever kept without the other (MG-04 D53). The
    /// row's `out_at` must be null. A second session with the same id is an
    /// error and writes nothing.
    pub fn insert_shared_session_durable(
        &self,
        wallet_id: &str,
        row: &SharedSessionRow,
        now: u64,
    ) -> Result<()> {
        debug_assert!(row.out_at.is_none());
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            lock_reserved(&tx, wallet_id, &row.reserved, now)?;
            tx.execute(
                &format!(
                    "INSERT INTO shared_mn_sessions (wallet_id, {COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                             ?16, ?17)"
                ),
                params![
                    wallet_id,
                    row.id,
                    row.kind,
                    row.role,
                    row.envelope,
                    row.my_shares,
                    row.contribution,
                    row.reserved,
                    row.approved_terms,
                    row.base_keys,
                    row.signed_tx,
                    row.txid,
                    row.out_at.map(sql_u64),
                    row.abandon_txid,
                    sql_u64(row.created_at),
                    sql_u64(row.updated_at),
                    row.last_error,
                ],
            )?;
            Ok(tx.commit()?)
        })
    }

    /// The wallet's sessions, newest first (by `updated_at`, then id).
    pub fn shared_sessions(&self, wallet_id: &str) -> Result<Vec<SharedSessionRow>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM shared_mn_sessions WHERE wallet_id = ?1
             ORDER BY updated_at DESC, id"
        ))?;
        let rows = stmt.query_map([wallet_id], read_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn shared_session(&self, wallet_id: &str, id: &str) -> Result<Option<SharedSessionRow>> {
        let conn = self.conn();
        Ok(conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM shared_mn_sessions WHERE wallet_id = ?1 AND id = ?2"
                ),
                params![wallet_id, id],
                read_row,
            )
            .optional()?)
    }

    /// Writes a session's mutable fields, synced (see
    /// [`SharedSessionUpdate`]). `false`, and nothing written, when the
    /// session is gone, or is out and the write would give it other signed
    /// bytes than the ones that may have left.
    pub fn update_shared_session_durable(
        &self,
        wallet_id: &str,
        id: &str,
        update: &SharedSessionUpdate,
    ) -> Result<bool> {
        self.durably(|conn| {
            Ok(conn.execute(
                "UPDATE shared_mn_sessions SET envelope = ?3, my_shares = ?4, contribution = ?5,
                     approved_terms = ?6, signed_tx = COALESCE(?7, signed_tx),
                     txid = COALESCE(?8, txid), last_error = ?9, updated_at = ?10
                 WHERE wallet_id = ?1 AND id = ?2
                   AND (out_at IS NULL OR ?7 IS NULL OR signed_tx = ?7)",
                params![
                    wallet_id,
                    id,
                    update.envelope,
                    update.my_shares,
                    update.contribution,
                    update.approved_terms,
                    update.signed_tx,
                    update.txid,
                    update.last_error,
                    sql_u64(update.updated_at),
                ],
            )? == 1)
        })
    }

    /// Replaces a session's reservation (MG-04b D33, `set_shared_fee`, and
    /// a participant's join), in one synced transaction: the old outpoints
    /// are unlocked, the new ones (`reserved`, a JSON array of
    /// `"txid:vout"`) locked, and the row's `reserved`, `contribution`,
    /// `my_shares` and `envelope` replaced. An
    /// outpoint in both stays locked. `false`, and nothing written, when
    /// the session is gone or is out: its coins are never released then.
    #[expect(clippy::too_many_arguments, reason = "one replaced reservation")]
    pub fn replace_shared_session_reservation_durable(
        &self,
        wallet_id: &str,
        id: &str,
        reserved: &str,
        contribution: Option<&str>,
        my_shares: &str,
        envelope: &str,
        updated_at: u64,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let old: Option<String> = tx
                .query_row(
                    "SELECT reserved FROM shared_mn_sessions
                     WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NULL",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(old) = old else {
                return Ok(false);
            };
            unlock_reserved(&tx, wallet_id, &old, now)?;
            lock_reserved(&tx, wallet_id, reserved, now)?;
            tx.execute(
                "UPDATE shared_mn_sessions SET reserved = ?3, contribution = ?4, my_shares = ?5,
                     envelope = ?6, updated_at = ?7
                 WHERE wallet_id = ?1 AND id = ?2",
                params![
                    wallet_id,
                    id,
                    reserved,
                    contribution,
                    my_shares,
                    envelope,
                    sql_u64(updated_at)
                ],
            )?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Marks a session out, synced, before its signed bytes can leave the
    /// wallet: `signed_tx`, `txid` and `out_at` are written together. The
    /// first `out_at` stays; writing the same bytes again is a no-op that
    /// returns `true`. `false`, and nothing written, when the session is
    /// gone or already out with other bytes.
    pub fn mark_shared_session_out_durable(
        &self,
        wallet_id: &str,
        id: &str,
        signed_tx: &[u8],
        txid: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            Ok(conn.execute(
                "UPDATE shared_mn_sessions SET signed_tx = ?3, txid = ?4,
                     out_at = COALESCE(out_at, ?5), updated_at = MAX(updated_at, ?5)
                 WHERE wallet_id = ?1 AND id = ?2 AND (out_at IS NULL OR signed_tx = ?3)",
                params![wallet_id, id, signed_tx, txid, sql_u64(now)],
            )? == 1)
        })
    }

    /// Deletes a session nothing of which may be out and unlocks its
    /// coins, in one synced transaction: `false` (and nothing deleted or
    /// unlocked) when its `out_at` is set or it is gone.
    pub fn delete_unsent_shared_session_durable(
        &self,
        wallet_id: &str,
        id: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let reserved: Option<String> = tx
                .query_row(
                    "DELETE FROM shared_mn_sessions
                     WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NULL RETURNING reserved",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(reserved) = reserved else {
                return Ok(false);
            };
            unlock_reserved(&tx, wallet_id, &reserved, now)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Unlocks a session's coins and forgets that it held them (`reserved`
    /// becomes `[]`), keeping the row, in one synced transaction: for a
    /// session that is confirmed, conflicted or dead. `expected` is the
    /// `reserved` value the caller judged the session by: when the row's
    /// is another now (a fee change re-reserved it since), nothing is
    /// unlocked or written. Idempotent; `false` when the session is gone
    /// or its `reserved` isn't `expected`.
    pub fn release_shared_session_locks_durable(
        &self,
        wallet_id: &str,
        id: &str,
        expected: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let reserved: Option<String> = tx
                .query_row(
                    "SELECT reserved FROM shared_mn_sessions WHERE wallet_id = ?1 AND id = ?2",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            if reserved.as_deref() != Some(expected) {
                return Ok(false);
            }
            unlock_reserved(&tx, wallet_id, expected, now)?;
            tx.execute(
                "UPDATE shared_mn_sessions SET reserved = '[]' WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id],
            )?;
            tx.commit()?;
            Ok(true)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "aa";
    const W2: &str = "bb";

    fn id(n: u8) -> String {
        format!("{n:02x}").repeat(32)
    }

    fn op(txid: &str, vout: u32) -> String {
        format!("{txid}:{vout}")
    }

    fn reserved(ops: &[(&str, u32)]) -> String {
        let items: Vec<_> = ops
            .iter()
            .map(|(t, v)| format!("\"{}\"", op(t, *v)))
            .collect();
        format!("[{}]", items.join(","))
    }

    fn row(n: u8, ops: &[(&str, u32)]) -> SharedSessionRow {
        SharedSessionRow {
            id: id(n),
            kind: "registration".into(),
            role: "coordinator".into(),
            envelope: "{\"rev\":1}".into(),
            my_shares: "{\"0\":0}".into(),
            contribution: None,
            reserved: reserved(ops),
            approved_terms: None,
            base_keys: None,
            signed_tx: None,
            txid: None,
            out_at: None,
            abandon_txid: None,
            created_at: 10,
            updated_at: 10,
            last_error: None,
        }
    }

    fn update(envelope: &str, updated_at: u64) -> SharedSessionUpdate {
        SharedSessionUpdate {
            envelope: envelope.into(),
            my_shares: "{\"0\":0}".into(),
            contribution: None,
            approved_terms: None,
            signed_tx: None,
            txid: None,
            last_error: None,
            updated_at,
        }
    }

    fn active_locks(db: &AppDb) -> Vec<(String, u32)> {
        let mut locks: Vec<_> = (db.locks(W).unwrap().into_iter())
            .filter(|r| r.is_locked())
            .map(|r| (r.txid, r.vout))
            .collect();
        locks.sort();
        locks
    }

    fn pairs(ops: &[(&str, u32)]) -> Vec<(String, u32)> {
        ops.iter().map(|(t, v)| ((*t).to_owned(), *v)).collect()
    }

    #[test]
    fn an_insert_locks_its_outpoints_and_reads_back() {
        let db = AppDb::open_in_memory().unwrap();
        let mut r = row(1, &[("t", 0), ("u", 3)]);
        r.contribution = Some("{\"total\":5}".into());
        db.insert_shared_session_durable(W, &r, 11).unwrap();
        assert_eq!(db.shared_session(W, &id(1)).unwrap(), Some(r));
        assert_eq!(active_locks(&db), pairs(&[("t", 0), ("u", 3)]));
        assert_eq!(db.shared_session(W2, &id(1)).unwrap(), None);
        assert_eq!(db.shared_session(W, &id(2)).unwrap(), None);
        // The same id again is refused and locks nothing more.
        assert!(
            db.insert_shared_session_durable(W, &row(1, &[("v", 1)]), 12)
                .is_err()
        );
        assert_eq!(active_locks(&db), pairs(&[("t", 0), ("u", 3)]));
        // Another wallet may use the id.
        db.insert_shared_session_durable(W2, &row(1, &[]), 12)
            .unwrap();
    }

    /// MG-04 D53: a row and its locks are one write. A failure writing
    /// either leaves neither.
    #[test]
    fn a_row_and_its_locks_are_written_together() {
        let db = AppDb::open_in_memory().unwrap();
        let r = row(1, &[("t", 0), ("u", 1)]);
        db.conn()
            .execute_batch(
                "CREATE TEMP TRIGGER no_lock BEFORE INSERT ON utxo_locks
                 BEGIN SELECT RAISE(ABORT, 'lock refused'); END;",
            )
            .unwrap();
        assert!(db.insert_shared_session_durable(W, &r, 1).is_err());
        assert!(db.shared_session(W, &id(1)).unwrap().is_none());
        db.conn()
            .execute_batch(
                "DROP TRIGGER no_lock;
                 CREATE TEMP TRIGGER no_row BEFORE INSERT ON shared_mn_sessions
                 BEGIN SELECT RAISE(ABORT, 'row refused'); END;",
            )
            .unwrap();
        assert!(db.insert_shared_session_durable(W, &r, 1).is_err());
        assert!(active_locks(&db).is_empty(), "no lock without its row");
        db.conn().execute_batch("DROP TRIGGER no_row;").unwrap();
        db.insert_shared_session_durable(W, &r, 1).unwrap();
        assert_eq!(active_locks(&db), pairs(&[("t", 0), ("u", 1)]));
    }

    #[test]
    fn a_malformed_reservation_writes_nothing() {
        let db = AppDb::open_in_memory().unwrap();
        for bad in ["not json", "{}", "[1]", "[\"novout\"]", "[\"t:x\"]"] {
            let mut r = row(1, &[("t", 0)]);
            r.reserved = bad.into();
            assert!(
                matches!(
                    db.insert_shared_session_durable(W, &r, 1),
                    Err(AppDbError::Corrupt(_))
                ),
                "{bad}"
            );
        }
        assert!(db.shared_sessions(W).unwrap().is_empty());
        assert!(active_locks(&db).is_empty());
    }

    #[test]
    fn an_update_writes_the_mutable_fields_and_keeps_the_rest() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        let u = SharedSessionUpdate {
            envelope: "{\"rev\":2}".into(),
            my_shares: "{\"0\":0,\"1\":4}".into(),
            contribution: Some("{\"total\":9}".into()),
            approved_terms: Some("{\"terms\":1}".into()),
            signed_tx: Some(vec![1, 2]),
            txid: Some("tx".into()),
            last_error: Some("shared_message.older".into()),
            updated_at: 20,
        };
        assert!(db.update_shared_session_durable(W, &id(1), &u).unwrap());
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(r.envelope, u.envelope);
        assert_eq!(r.my_shares, u.my_shares);
        assert_eq!(r.contribution, u.contribution);
        assert_eq!(r.approved_terms, u.approved_terms);
        assert_eq!(r.signed_tx, u.signed_tx);
        assert_eq!(r.txid, u.txid);
        assert_eq!(r.last_error, u.last_error);
        assert_eq!((r.updated_at, r.created_at), (20, 10));
        assert_eq!((r.reserved, r.out_at), (reserved(&[("t", 0)]), None));
        // `None` signed bytes and txid keep the stored ones; the other
        // fields are replaced.
        assert!(
            db.update_shared_session_durable(W, &id(1), &update("{\"rev\":3}", 21))
                .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(
            (r.signed_tx, r.txid, r.contribution, r.last_error),
            (Some(vec![1, 2]), Some("tx".into()), None, None)
        );
        assert_eq!((r.envelope.as_str(), r.updated_at), ("{\"rev\":3}", 21));
        // A missing session is not created.
        assert!(!db.update_shared_session_durable(W, &id(2), &u).unwrap());
        assert!(!db.update_shared_session_durable(W2, &id(1), &u).unwrap());
    }

    #[test]
    fn an_out_session_keeps_the_bytes_that_left() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        assert!(
            db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
                .unwrap()
        );
        let mut u = update("{\"rev\":2}", 31);
        u.signed_tx = Some(vec![8]);
        assert!(!db.update_shared_session_durable(W, &id(1), &u).unwrap());
        // The same bytes, or none, are fine.
        u.signed_tx = Some(vec![7]);
        assert!(db.update_shared_session_durable(W, &id(1), &u).unwrap());
        assert!(
            db.update_shared_session_durable(W, &id(1), &update("{\"rev\":3}", 32))
                .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(
            (r.signed_tx, r.envelope.as_str()),
            (Some(vec![7]), "{\"rev\":3}")
        );
    }

    #[test]
    fn replacing_a_reservation_swaps_the_locks_and_the_row_together() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0), ("u", 1)]), 1)
            .unwrap();
        // A lock outside the session is not touched.
        db.lock_manual(W, "mine", 0, 1).unwrap();
        let new = reserved(&[("u", 1), ("v", 2)]);
        assert!(
            db.replace_shared_session_reservation_durable(
                W,
                &id(1),
                &new,
                Some("{\"total\":8}"),
                "{\"1\":3}",
                "{\"rev\":2}",
                40,
                2
            )
            .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(r.reserved, new);
        assert_eq!(r.contribution.as_deref(), Some("{\"total\":8}"));
        assert_eq!(r.my_shares, "{\"1\":3}");
        assert_eq!(r.envelope, "{\"rev\":2}");
        assert_eq!(r.updated_at, 40);
        assert_eq!(active_locks(&db), pairs(&[("mine", 0), ("u", 1), ("v", 2)]));

        // A failure locking the new outpoints keeps the old reservation.
        db.conn()
            .execute_batch(
                "CREATE TEMP TRIGGER no_lock BEFORE INSERT ON utxo_locks
                 BEGIN SELECT RAISE(ABORT, 'lock refused'); END;",
            )
            .unwrap();
        assert!(
            db.replace_shared_session_reservation_durable(
                W,
                &id(1),
                &reserved(&[("w", 0)]),
                None,
                "{}",
                "{\"rev\":3}",
                41,
                3
            )
            .is_err()
        );
        db.conn().execute_batch("DROP TRIGGER no_lock;").unwrap();
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!((r.reserved, r.envelope.as_str()), (new, "{\"rev\":2}"));
        assert_eq!(active_locks(&db), pairs(&[("mine", 0), ("u", 1), ("v", 2)]));

        // A missing session changes nothing.
        assert!(
            !db.replace_shared_session_reservation_durable(
                W,
                &id(2),
                &reserved(&[("w", 0)]),
                None,
                "{}",
                "{}",
                42,
                4
            )
            .unwrap()
        );
        assert_eq!(active_locks(&db), pairs(&[("mine", 0), ("u", 1), ("v", 2)]));
    }

    #[test]
    fn an_out_session_keeps_its_reservation() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
            .unwrap();
        assert!(
            !db.replace_shared_session_reservation_durable(
                W,
                &id(1),
                &reserved(&[("w", 0)]),
                None,
                "{}",
                "{}",
                42,
                4
            )
            .unwrap()
        );
        assert_eq!(active_locks(&db), pairs(&[("t", 0)]));
        assert_eq!(
            db.shared_session(W, &id(1)).unwrap().unwrap().reserved,
            reserved(&[("t", 0)])
        );
    }

    #[test]
    fn marking_out_writes_the_bytes_and_the_time_together() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        assert!(
            !db.mark_shared_session_out_durable(W, &id(2), &[7], "tx", 30)
                .unwrap()
        );
        assert!(
            db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
                .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(
            (r.signed_tx, r.txid, r.out_at, r.updated_at),
            (Some(vec![7]), Some("tx".into()), Some(30), 30)
        );
        // The same bytes again keep the first time; other bytes are refused.
        assert!(
            db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 35)
                .unwrap()
        );
        assert!(
            !db.mark_shared_session_out_durable(W, &id(1), &[8], "ty", 36)
                .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(
            (r.signed_tx, r.txid, r.out_at),
            (Some(vec![7]), Some("tx".into()), Some(30))
        );
    }

    #[test]
    fn a_session_that_is_out_is_never_deleted() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0), ("u", 1)]), 1)
            .unwrap();
        db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
            .unwrap();
        assert!(
            !db.delete_unsent_shared_session_durable(W, &id(1), 31)
                .unwrap()
        );
        assert!(db.shared_session(W, &id(1)).unwrap().is_some());
        assert_eq!(active_locks(&db), pairs(&[("t", 0), ("u", 1)]));
    }

    #[test]
    fn deleting_a_session_before_it_is_out_unlocks_its_coins() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0), ("u", 1)]), 1)
            .unwrap();
        db.insert_shared_session_durable(W, &row(2, &[("v", 2)]), 1)
            .unwrap();
        assert!(
            db.delete_unsent_shared_session_durable(W, &id(1), 5)
                .unwrap()
        );
        assert!(db.shared_session(W, &id(1)).unwrap().is_none());
        assert_eq!(active_locks(&db), pairs(&[("v", 2)]));
        // Gone: nothing to delete, other sessions untouched.
        assert!(
            !db.delete_unsent_shared_session_durable(W, &id(1), 6)
                .unwrap()
        );
        assert!(
            !db.delete_unsent_shared_session_durable(W2, &id(2), 6)
                .unwrap()
        );
        assert!(db.shared_session(W, &id(2)).unwrap().is_some());
        assert_eq!(active_locks(&db), pairs(&[("v", 2)]));
    }

    #[test]
    fn releasing_the_locks_keeps_the_row() {
        let db = AppDb::open_in_memory().unwrap();
        let held = reserved(&[("t", 0), ("u", 1)]);
        db.insert_shared_session_durable(W, &row(1, &[("t", 0), ("u", 1)]), 1)
            .unwrap();
        db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
            .unwrap();
        assert!(
            db.release_shared_session_locks_durable(W, &id(1), &held, 40)
                .unwrap()
        );
        assert!(active_locks(&db).is_empty());
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!((r.reserved.as_str(), r.out_at), ("[]", Some(30)));
        // Idempotent: a later lock of the same outpoint is not released.
        db.lock_manual(W, "t", 0, 41).unwrap();
        assert!(
            db.release_shared_session_locks_durable(W, &id(1), "[]", 42)
                .unwrap()
        );
        assert_eq!(active_locks(&db), pairs(&[("t", 0)]));
        assert!(
            !db.release_shared_session_locks_durable(W, &id(2), "[]", 42)
                .unwrap()
        );
    }

    /// fixes-1a M4: a release judged on a `reserved` that has since been
    /// replaced (a fee change re-reserved the session's coins) changes
    /// nothing: neither the new locks nor the row.
    #[test]
    fn a_release_of_a_replaced_reservation_changes_nothing() {
        let db = AppDb::open_in_memory().unwrap();
        let read = reserved(&[("t", 0)]);
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        let now = reserved(&[("t", 0), ("v", 2)]);
        assert!(
            db.replace_shared_session_reservation_durable(W, &id(1), &now, None, "{}", "{}", 2, 2)
                .unwrap()
        );
        assert!(
            !db.release_shared_session_locks_durable(W, &id(1), &read, 3)
                .unwrap()
        );
        assert_eq!(active_locks(&db), pairs(&[("t", 0), ("v", 2)]));
        assert_eq!(db.shared_session(W, &id(1)).unwrap().unwrap().reserved, now);
    }

    #[test]
    fn sessions_list_newest_first() {
        let db = AppDb::open_in_memory().unwrap();
        for (n, at) in [(3, 20), (1, 30), (2, 20), (4, 5)] {
            let mut r = row(n, &[]);
            r.updated_at = at;
            db.insert_shared_session_durable(W, &r, 1).unwrap();
        }
        db.insert_shared_session_durable(W2, &row(9, &[]), 1)
            .unwrap();
        let ids: Vec<_> = db
            .shared_sessions(W)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(ids, [id(1), id(2), id(3), id(4)]);
        // An update moves a session to the front.
        db.update_shared_session_durable(W, &id(4), &update("{}", 99))
            .unwrap();
        assert_eq!(db.shared_sessions(W).unwrap()[0].id, id(4));
    }

    #[test]
    fn a_wallets_removal_deletes_its_sessions_and_skips_later_inserts() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        db.insert_shared_session_durable(W2, &row(1, &[]), 1)
            .unwrap();
        db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
            .unwrap();
        db.delete_wallet(W).unwrap();
        assert!(db.shared_sessions(W).unwrap().is_empty());
        assert_eq!(db.shared_sessions(W2).unwrap().len(), 1);
        db.insert_shared_session_durable(W, &row(2, &[]), 2)
            .unwrap();
        assert!(db.shared_sessions(W).unwrap().is_empty());
    }

    #[test]
    fn the_schema_checks_hold() {
        let db = AppDb::open_in_memory().unwrap();
        let conn = db.conn();
        let insert = |id: &str, kind: &str, role: &str, extra: &str| {
            conn.execute(
                &format!(
                    "INSERT INTO shared_mn_sessions
                         (id, wallet_id, kind, role, envelope, my_shares, created_at, updated_at
                          {})
                     VALUES ('{id}', 'aa', '{kind}', '{role}', '{{}}', '{{}}', 1, 1 {})",
                    if extra.is_empty() { "" } else { ", out_at" },
                    if extra.is_empty() { "" } else { ", 5" },
                ),
                [],
            )
        };
        let good = id(1);
        // Out needs signed bytes.
        assert!(insert(&good, "registration", "coordinator", "out").is_err());
        assert!(insert(&good, "registration", "coordinator", "").is_ok());
        // The id is 64 lower-case hex; kind and role are the known ones.
        assert!(insert(&id(2)[1..], "registration", "coordinator", "").is_err());
        assert!(insert(&id(0xab).to_uppercase(), "registration", "coordinator", "").is_err());
        assert!(insert(&"g".repeat(64), "registration", "coordinator", "").is_err());
        assert!(insert(&id(2), "update", "coordinator", "").is_err());
        assert!(insert(&id(2), "key_change", "observer", "").is_err());
        assert!(insert(&id(2), "dissolve_together", "participant", "").is_ok());
    }
}
