//! The single writer of `shared_mn_sessions` (MG-04b §9.1): a wallet's
//! shared-masternode sessions. Every delete is `... AND out_at IS NULL`, so
//! a row whose signed bytes may have left the wallet is never deleted
//! (MG-04b §9.2, MG-04 D6); every write a crash must not lose is synced
//! ([`AppDb::durably`]). A session's coins are persistent manual locks
//! written, replaced and released in the same transaction as its row
//! (MG-04 D53): the outpoints are the `reserved` column's, a JSON array of
//! `"txid:vout"`, which is the only record of what the session locked.

use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{AppDb, AppDbError, Holder, Result, sql_u64};

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
    /// Consensus bytes of the latest abandon spend, `abandon_txid`'s.
    pub abandon_tx: Option<Vec<u8>>,
    /// JSON array: every abandon spend the session signed, oldest first.
    pub abandon_log: Option<String>,
    /// A spend back must take the whole `reserved` (an older build's abandon).
    pub legacy_bound: bool,
    /// A resend of the session's own bytes was handed off.
    pub resend_marked: bool,
}

/// What one hand-off of an abandon spend says about its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonOutcome {
    Sent,
    /// A definite refusal: the bytes didn't leave.
    NotSent,
    MaybeSent,
}

impl AbandonOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::NotSent => "not_sent",
            Self::MaybeSent => "maybe_sent",
        }
    }

    fn of(text: &str) -> Option<Self> {
        [Self::Sent, Self::NotSent, Self::MaybeSent]
            .into_iter()
            .find(|o| o.as_str() == text)
    }
}

/// The codes an older build stored on an out row that read as a refusal:
/// the migration rewrites each to `send.broadcast_unknown`.
pub const INHERITED_REFUSED_CODES: &[&str] = &[
    "send.no_peers",
    "send.broadcast_rejected",
    "send.prepared_tx_spent",
    "send.cancelled",
    "network_not_open",
    "wallet_not_found",
];

/// The `last_error` a resend of the session's own bytes holds until its
/// outcome is written.
pub const RESEND_IN_PROGRESS: &str = "send.resend_in_progress";

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
     last_error, abandon_tx, abandon_log, legacy_bound, resend_marked";

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
        abandon_tx: r.get(16)?,
        abandon_log: r.get(17)?,
        legacy_bound: r.get::<_, i64>(18)? != 0,
        resend_marked: r.get::<_, i64>(19)? != 0,
    })
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

/// Unlocks the outpoints of session `id`'s `reserved`, inside a caller's
/// transaction, but those another row holds (BL-346).
fn unlock_reserved(
    conn: &Connection,
    wallet_id: &str,
    id: &str,
    reserved: &str,
    now: u64,
) -> Result<()> {
    for (txid, vout) in outpoints(conn, reserved)? {
        AppDb::release_lock_on(conn, wallet_id, Holder::Session(id), &txid, vout, now)?;
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

/// The `abandon_log` index and outcome of session `id`'s entry for `txid`.
fn abandon_entry(
    conn: &Connection,
    wallet_id: &str,
    id: &str,
    txid: &str,
) -> Result<Option<(i64, Option<AbandonOutcome>)>> {
    let found: Option<(i64, Option<String>)> = conn
        .query_row(
            "SELECT e.key, json_extract(e.value, '$.outcome')
             FROM shared_mn_sessions s, json_each(COALESCE(s.abandon_log, '[]')) e
             WHERE s.wallet_id = ?1 AND s.id = ?2 AND s.out_at IS NOT NULL
               AND json_extract(e.value, '$.txid') = ?3
             ORDER BY e.key LIMIT 1",
            params![wallet_id, id, txid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    found
        .map(|(index, outcome)| match outcome {
            None => Ok((index, None)),
            Some(text) => AbandonOutcome::of(&text)
                .map(|o| (index, Some(o)))
                .ok_or_else(|| corrupt("abandon_log", &text)),
        })
        .transpose()
}

fn set_entry_outcome(
    conn: &Connection,
    wallet_id: &str,
    id: &str,
    index: i64,
    outcome: AbandonOutcome,
    code: Option<&str>,
    now: u64,
) -> Result<()> {
    conn.execute(
        "UPDATE shared_mn_sessions SET abandon_log = json_set(abandon_log,
             '$[' || ?3 || '].outcome', ?4, '$[' || ?3 || '].code', ?5),
             updated_at = MAX(updated_at, ?6)
         WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NOT NULL",
        params![wallet_id, id, index, outcome.as_str(), code, sql_u64(now)],
    )?;
    Ok(())
}

/// The outcome an entry moves to after one hand-off, `None` to keep it.
fn fold(
    current: Option<AbandonOutcome>,
    attempt: AbandonOutcome,
    first: bool,
) -> Option<AbandonOutcome> {
    use AbandonOutcome::{MaybeSent, NotSent, Sent};
    match (current, attempt) {
        (None, NotSent) if first => Some(NotSent),
        (None | Some(NotSent), MaybeSent) | (None | Some(NotSent) | Some(MaybeSent), Sent) => {
            Some(attempt)
        }
        _ => None,
    }
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
                             ?16, ?17, ?18, ?19, ?20, ?21)"
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
                    row.abandon_tx,
                    row.abandon_log,
                    row.legacy_bound,
                    row.resend_marked,
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
            unlock_reserved(&tx, wallet_id, id, &old, now)?;
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

    /// Records how a hand-off of a session's bytes ended: `error`, its
    /// failure's code, or `None`. `false` when the session is gone.
    pub fn record_shared_session_attempt(
        &self,
        wallet_id: &str,
        id: &str,
        error: Option<&str>,
    ) -> Result<bool> {
        Ok(self.conn().execute(
            "UPDATE shared_mn_sessions SET last_error = ?3 WHERE wallet_id = ?1 AND id = ?2",
            params![wallet_id, id, error],
        )? == 1)
    }

    /// Records an out session's abandon spend (MG-04b §9.3, MG-10 §4.4),
    /// synced, before its bytes can leave: `abandon_txid` and `abandon_tx`
    /// set and an entry appended to `abandon_log` with no outcome. A txid
    /// already logged keeps its entry, whose missing or `not_sent` outcome
    /// becomes `maybe_sent`: its bytes leave again, after a hand-off that may
    /// be out or that a crash may follow. That entry takes `fee` and
    /// `inputs` where it has none (an older build's legacy entry): the same
    /// txid is the same spend. `legacy_bound` is cleared when
    /// `inputs`, a JSON array of `"txid:vout"`, hold every outpoint of the
    /// stored `reserved`. `Some(logged)`, whether the txid was already
    /// logged; `None`, and nothing written, when the session is gone or isn't
    /// out.
    #[expect(clippy::too_many_arguments, reason = "one abandon spend")]
    pub fn set_shared_session_abandon_durable(
        &self,
        wallet_id: &str,
        id: &str,
        txid: &str,
        signed_tx: &[u8],
        fee: u64,
        inputs: &str,
        now: u64,
    ) -> Result<Option<bool>> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let reserved: Option<String> = tx
                .query_row(
                    "SELECT reserved FROM shared_mn_sessions
                     WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NOT NULL",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(reserved) = reserved else {
                return Ok(None);
            };
            let spent = outpoints(&tx, inputs)?;
            let whole = outpoints(&tx, &reserved)?.iter().all(|o| spent.contains(o));
            let entry = abandon_entry(&tx, wallet_id, id, txid)?;
            let logged = entry.is_some();
            tx.execute(
                "UPDATE shared_mn_sessions SET abandon_txid = ?3, abandon_tx = ?4,
                     abandon_log = CASE WHEN ?8 THEN abandon_log ELSE json_insert(
                         COALESCE(abandon_log, '[]'), '$[#]',
                         json_object('txid', ?3, 'fee', ?5, 'inputs', json(?6),
                             'signed_at', ?7, 'outcome', NULL, 'code', NULL,
                             'legacy', json('false'))) END,
                     legacy_bound = CASE WHEN ?9 THEN 0 ELSE legacy_bound END,
                     updated_at = MAX(updated_at, ?7)
                 WHERE wallet_id = ?1 AND id = ?2",
                params![
                    wallet_id,
                    id,
                    txid,
                    signed_tx,
                    sql_u64(fee),
                    inputs,
                    sql_u64(now),
                    logged,
                    whole
                ],
            )?;
            if let Some((index, _)) = entry {
                tx.execute(
                    "UPDATE shared_mn_sessions SET abandon_log = json_set(abandon_log,
                         '$[' || ?3 || '].fee', COALESCE(
                             json_extract(abandon_log, '$[' || ?3 || '].fee'), ?4),
                         '$[' || ?3 || '].inputs', json(CASE
                             WHEN json_array_length(abandon_log, '$[' || ?3 || '].inputs') > 0
                             THEN json_extract(abandon_log, '$[' || ?3 || '].inputs')
                             ELSE ?5 END))
                     WHERE wallet_id = ?1 AND id = ?2",
                    params![wallet_id, id, index, sql_u64(fee), inputs],
                )?;
            }
            if let Some((index, None | Some(AbandonOutcome::NotSent))) = entry {
                set_entry_outcome(
                    &tx,
                    wallet_id,
                    id,
                    index,
                    AbandonOutcome::MaybeSent,
                    None,
                    now,
                )?;
            }
            tx.commit()?;
            Ok(Some(logged))
        })
    }

    /// Before a replay of a logged abandon spend, synced: its entry's
    /// missing outcome becomes `maybe_sent` (a replay's refusal can't show
    /// the first hand-off never left). Any recorded outcome stays. `false`
    /// when the session or the entry is gone or the session isn't out.
    pub fn mark_shared_abandon_replay_durable(
        &self,
        wallet_id: &str,
        id: &str,
        txid: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let Some((index, outcome)) = abandon_entry(conn, wallet_id, id, txid)? else {
                return Ok(false);
            };
            if outcome.is_none() {
                set_entry_outcome(
                    conn,
                    wallet_id,
                    id,
                    index,
                    AbandonOutcome::MaybeSent,
                    None,
                    now,
                )?;
            }
            Ok(true)
        })
    }

    /// Folds one hand-off of the logged abandon spend `txid` into its entry
    /// and writes `code` as `last_error`, synced. Forward only: a missing
    /// outcome becomes `not_sent` only on the spend's `first` hand-off;
    /// `maybe_sent` and `sent` replace a missing or `not_sent` one, `sent` a
    /// `maybe_sent` one. Any other result (a replay's refusal among them)
    /// leaves the entry as it is. `false` when the session or the entry is
    /// gone or the session isn't out.
    #[expect(clippy::too_many_arguments, reason = "one hand-off's result")]
    pub fn record_shared_abandon_outcome_durable(
        &self,
        wallet_id: &str,
        id: &str,
        txid: &str,
        outcome: AbandonOutcome,
        code: Option<&str>,
        first: bool,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let Some((index, current)) = abandon_entry(&tx, wallet_id, id, txid)? else {
                return Ok(false);
            };
            if let Some(next) = fold(current, outcome, first) {
                set_entry_outcome(&tx, wallet_id, id, index, next, code, now)?;
            }
            tx.execute(
                "UPDATE shared_mn_sessions SET last_error = ?3, updated_at = MAX(updated_at, ?4)
                 WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NOT NULL",
                params![wallet_id, id, code, sql_u64(now)],
            )?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Immediately before a resend of an out session's own bytes is handed
    /// off, synced: `last_error` becomes [`RESEND_IN_PROGRESS`] and
    /// `resend_marked` is set for good, so a crash in the hand-off never
    /// reads as the earlier attempt's refusal. `false` when the session is
    /// gone or isn't out.
    pub fn mark_shared_session_resend_durable(&self, wallet_id: &str, id: &str) -> Result<bool> {
        self.durably(|conn| {
            Ok(conn.execute(
                "UPDATE shared_mn_sessions SET last_error = ?3, resend_marked = 1
                 WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NOT NULL",
                params![wallet_id, id, RESEND_IN_PROGRESS],
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
        self.delete_shared_session_where(
            "DELETE FROM shared_mn_sessions
             WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NULL RETURNING reserved",
            params![wallet_id, id],
            wallet_id,
            id,
            now,
        )
    }

    /// Deletes a session marked out with `txid` whose bytes the caller has
    /// found were never handed off, and unlocks its coins, in one synced
    /// transaction: `false` (and nothing deleted or unlocked) when it isn't
    /// out, is out with another txid, or is gone.
    pub fn delete_out_shared_session_durable(
        &self,
        wallet_id: &str,
        id: &str,
        txid: &str,
        now: u64,
    ) -> Result<bool> {
        self.delete_shared_session_where(
            "DELETE FROM shared_mn_sessions
             WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NOT NULL AND txid = ?3
             RETURNING reserved",
            params![wallet_id, id, txid],
            wallet_id,
            id,
            now,
        )
    }

    fn delete_shared_session_where(
        &self,
        delete: &str,
        params: &[&dyn rusqlite::ToSql],
        wallet_id: &str,
        id: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let reserved: Option<String> = tx.query_row(delete, params, |r| r.get(0)).optional()?;
            let Some(reserved) = reserved else {
                return Ok(false);
            };
            unlock_reserved(&tx, wallet_id, id, &reserved, now)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Ends a session nothing of which is out by its own choice (MG-04b
    /// D102, a key change abandoned before its send): its coins unlocked,
    /// `reserved` emptied and `contribution` replaced (the record of the
    /// abandon), the row kept, in one synced transaction. `false`, and
    /// nothing written, when the session is gone or out.
    pub fn abandon_unsent_shared_session_durable(
        &self,
        wallet_id: &str,
        id: &str,
        contribution: &str,
        now: u64,
    ) -> Result<bool> {
        self.durably(|conn| {
            let tx = conn.unchecked_transaction()?;
            let reserved: Option<String> = tx
                .query_row(
                    "SELECT reserved FROM shared_mn_sessions
                     WHERE wallet_id = ?1 AND id = ?2 AND out_at IS NULL",
                    params![wallet_id, id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(reserved) = reserved else {
                return Ok(false);
            };
            unlock_reserved(&tx, wallet_id, id, &reserved, now)?;
            tx.execute(
                "UPDATE shared_mn_sessions SET reserved = '[]', contribution = ?3, updated_at = ?4
                 WHERE wallet_id = ?1 AND id = ?2",
                params![wallet_id, id, contribution, sql_u64(now)],
            )?;
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
            unlock_reserved(&tx, wallet_id, id, expected, now)?;
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
            abandon_tx: None,
            abandon_log: None,
            legacy_bound: false,
            resend_marked: false,
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
    fn a_session_out_with_its_txid_is_deleted_by_that_txid_only() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 1)
            .unwrap();
        db.insert_shared_session_durable(W, &row(2, &[("v", 2)]), 1)
            .unwrap();
        // Not out: the unsent delete's.
        assert!(
            !db.delete_out_shared_session_durable(W, &id(2), "tx", 5)
                .unwrap()
        );
        db.mark_shared_session_out_durable(W, &id(1), &[7], "tx", 30)
            .unwrap();
        assert!(
            !db.delete_out_shared_session_durable(W, &id(1), "ty", 31)
                .unwrap()
        );
        assert!(
            !db.delete_out_shared_session_durable(W2, &id(1), "tx", 31)
                .unwrap()
        );
        assert!(
            db.delete_out_shared_session_durable(W, &id(1), "tx", 31)
                .unwrap()
        );
        assert!(db.shared_session(W, &id(1)).unwrap().is_none());
        assert_eq!(active_locks(&db), pairs(&[("v", 2)]));
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

    /// MG-04b D102: a key change abandoned before its send unlocks its
    /// coins and keeps its row with the abandon's record; one out isn't
    /// touched.
    #[test]
    fn abandoning_an_unsent_session_unlocks_its_coins_and_keeps_the_row() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0), ("u", 1)]), 1)
            .unwrap();
        db.insert_shared_session_durable(W, &row(2, &[("v", 2)]), 1)
            .unwrap();
        assert!(
            db.abandon_unsent_shared_session_durable(W, &id(1), "{\"abandoned_at\":5}", 5)
                .unwrap()
        );
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(
            (
                r.reserved.as_str(),
                r.contribution.as_deref(),
                r.updated_at,
                r.out_at
            ),
            ("[]", Some("{\"abandoned_at\":5}"), 5, None)
        );
        assert_eq!(active_locks(&db), pairs(&[("v", 2)]));
        // Out: nothing written, its lock kept.
        db.mark_shared_session_out_durable(W, &id(2), &[7], "tx", 6)
            .unwrap();
        assert!(
            !db.abandon_unsent_shared_session_durable(W, &id(2), "{}", 7)
                .unwrap()
        );
        let r = db.shared_session(W, &id(2)).unwrap().unwrap();
        assert_eq!((r.reserved, r.contribution), (reserved(&[("v", 2)]), None));
        assert_eq!(active_locks(&db), pairs(&[("v", 2)]));
        assert!(
            !db.abandon_unsent_shared_session_durable(W, &id(3), "{}", 7)
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

    fn out(db: &AppDb, n: u8, ops: &[(&str, u32)]) {
        db.insert_shared_session_durable(W, &row(n, ops), 10)
            .unwrap();
        db.mark_shared_session_out_durable(W, &id(n), &[7], "own", 20)
            .unwrap();
    }

    fn inputs(ops: &[(&str, u32)]) -> String {
        reserved(ops)
    }

    /// `(txid, outcome, code)` of every `abandon_log` entry, in order.
    fn log(db: &AppDb, n: u8) -> Vec<(String, Option<String>, Option<String>)> {
        let text = db.shared_session(W, &id(n)).unwrap().unwrap().abandon_log;
        let conn = db.conn();
        let mut stmt = conn
            .prepare(
                "SELECT json_extract(value, '$.txid'), json_extract(value, '$.outcome'),
                     json_extract(value, '$.code')
                 FROM json_each(COALESCE(?1, '[]')) ORDER BY key",
            )
            .unwrap();
        stmt.query_map([text], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn abandon(db: &AppDb, n: u8, txid: &str, ops: &[(&str, u32)]) -> Option<bool> {
        db.set_shared_session_abandon_durable(W, &id(n), txid, &[9], 226, &inputs(ops), 40)
            .unwrap()
    }

    fn set_legacy_bound(db: &AppDb, n: u8) {
        db.conn()
            .execute(
                "UPDATE shared_mn_sessions SET legacy_bound = 1 WHERE id = ?1",
                [id(n)],
            )
            .unwrap();
    }

    /// MG-10 §4.4 storage: each spend back is saved with its bytes and
    /// appended to the log before it can leave; earlier entries stay.
    #[test]
    fn an_abandon_spend_is_saved_and_appended_to_the_log() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[("t", 0)]), 10)
            .unwrap();
        assert_eq!(abandon(&db, 1, "a", &[("t", 0)]), None, "not out");
        assert_eq!(abandon(&db, 2, "a", &[("t", 0)]), None, "gone");
        let before = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert_eq!(before.abandon_log, None);
        assert_eq!(before.abandon_txid, None);

        out(&db, 3, &[("t", 1), ("u", 1)]);
        assert_eq!(abandon(&db, 3, "a", &[("t", 1)]), Some(false));
        let r = db.shared_session(W, &id(3)).unwrap().unwrap();
        assert_eq!(r.abandon_txid.as_deref(), Some("a"));
        assert_eq!(r.abandon_tx, Some(vec![9]));
        assert_eq!(log(&db, 3), [("a".into(), None, None)]);
        let entry: (i64, String, i64, bool) = db
            .conn()
            .query_row(
                "SELECT json_extract(abandon_log, '$[0].fee'),
                     json_extract(abandon_log, '$[0].inputs'),
                     json_extract(abandon_log, '$[0].signed_at'),
                     json_extract(abandon_log, '$[0].legacy')
                 FROM shared_mn_sessions WHERE id = ?1",
                [id(3)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(entry, (226, "[\"t:1\"]".into(), 40, false));

        assert_eq!(abandon(&db, 3, "b", &[("t", 1), ("u", 1)]), Some(false));
        assert_eq!(
            abandon(&db, 3, "b", &[("t", 1), ("u", 1)]),
            Some(true),
            "logged once"
        );
        assert_eq!(
            log(&db, 3),
            [
                ("a".into(), None, None),
                ("b".into(), Some("maybe_sent".into()), None)
            ]
        );
        assert_eq!(
            db.shared_session(W, &id(3))
                .unwrap()
                .unwrap()
                .abandon_txid
                .as_deref(),
            Some("b")
        );
        assert!(
            matches!(
                db.set_shared_session_abandon_durable(W, &id(3), "c", &[9], 1, "[\"x\"]", 50),
                Err(AppDbError::Corrupt(_))
            ),
            "inputs are \"txid:vout\""
        );
        assert_eq!(log(&db, 3).len(), 2);
        assert!(
            !db.delete_unsent_shared_session_durable(W, &id(3), 60)
                .unwrap()
        );
        assert!(
            !db.delete_out_shared_session_durable(W, &id(3), "x", 60)
                .unwrap()
        );
    }

    /// A spend logged again under a legacy entry's txid fills the fee and
    /// inputs that entry lacks, and keeps a logged fee and inputs.
    #[test]
    fn a_legacy_entry_takes_the_fee_and_inputs_of_its_spend() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0), ("u", 1)]);
        db.conn()
            .execute(
                "UPDATE shared_mn_sessions SET abandon_txid = 'a', abandon_log = json_array(
                     json_object('txid', 'a', 'fee', NULL, 'inputs', json('[]'),
                         'signed_at', 5, 'outcome', 'maybe_sent', 'code', NULL,
                         'legacy', json('true')))
                 WHERE id = ?1",
                [id(1)],
            )
            .unwrap();
        let entry = |db: &AppDb| -> (Option<i64>, String, bool, String) {
            db.conn()
                .query_row(
                    "SELECT json_extract(abandon_log, '$[0].fee'),
                         json_extract(abandon_log, '$[0].inputs'),
                         json_extract(abandon_log, '$[0].legacy'),
                         json_extract(abandon_log, '$[0].outcome')
                     FROM shared_mn_sessions WHERE id = ?1",
                    [id(1)],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap()
        };
        assert_eq!(abandon(&db, 1, "a", &[("t", 0), ("u", 1)]), Some(true));
        let filled = (
            Some(226),
            "[\"t:0\",\"u:1\"]".to_string(),
            true,
            "maybe_sent".into(),
        );
        assert_eq!(entry(&db), filled);
        assert_eq!(log(&db, 1).len(), 1, "the same entry");
        db.set_shared_session_abandon_durable(W, &id(1), "a", &[9], 9, &inputs(&[("t", 0)]), 50)
            .unwrap();
        assert_eq!(entry(&db), filled, "kept");
    }

    /// Sol r6 M1: the legacy mark clears only with a spend taking every
    /// reserved outpoint, checked by the write itself.
    #[test]
    fn only_a_spend_of_the_whole_reservation_clears_the_legacy_mark() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0), ("u", 1)]);
        set_legacy_bound(&db, 1);
        assert_eq!(abandon(&db, 1, "a", &[("t", 0)]), Some(false));
        assert!(db.shared_session(W, &id(1)).unwrap().unwrap().legacy_bound);
        assert_eq!(abandon(&db, 1, "b", &[("u", 1), ("v", 2)]), Some(false));
        assert!(db.shared_session(W, &id(1)).unwrap().unwrap().legacy_bound);
        assert_eq!(
            abandon(&db, 1, "c", &[("u", 1), ("t", 0), ("v", 2)]),
            Some(false)
        );
        assert!(!db.shared_session(W, &id(1)).unwrap().unwrap().legacy_bound);
        assert_eq!(abandon(&db, 1, "d", &[("t", 0)]), Some(false));
        assert!(
            !db.shared_session(W, &id(1)).unwrap().unwrap().legacy_bound,
            "never set again"
        );
    }

    #[test]
    fn the_fold_is_forward_only() {
        use AbandonOutcome::{MaybeSent, NotSent, Sent};
        let cases = [
            (None, NotSent, true, Some(NotSent)),
            (None, NotSent, false, None),
            (None, MaybeSent, true, Some(MaybeSent)),
            (None, MaybeSent, false, Some(MaybeSent)),
            (None, Sent, false, Some(Sent)),
            (Some(NotSent), NotSent, true, None),
            (Some(NotSent), MaybeSent, false, Some(MaybeSent)),
            (Some(NotSent), Sent, false, Some(Sent)),
            (Some(MaybeSent), NotSent, true, None),
            (Some(MaybeSent), NotSent, false, None),
            (Some(MaybeSent), MaybeSent, false, None),
            (Some(MaybeSent), Sent, false, Some(Sent)),
            (Some(Sent), NotSent, true, None),
            (Some(Sent), MaybeSent, false, None),
            (Some(Sent), Sent, false, None),
        ];
        for (current, attempt, first, next) in cases {
            assert_eq!(
                fold(current, attempt, first),
                next,
                "{current:?} {attempt:?} {first}"
            );
        }
    }

    /// The fold through the store: the entry moves forward, a replay's
    /// refusal leaves it alone, and `last_error` takes every attempt's code.
    #[test]
    fn an_outcome_is_recorded_in_its_entry() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0)]);
        assert_eq!(abandon(&db, 1, "a", &[("t", 0)]), Some(false));
        assert_eq!(abandon(&db, 1, "b", &[("t", 0)]), Some(false));
        let record = |txid: &str, o, code: Option<&str>, first| {
            db.record_shared_abandon_outcome_durable(W, &id(1), txid, o, code, first, 50)
                .unwrap()
        };
        let last_error = || db.shared_session(W, &id(1)).unwrap().unwrap().last_error;
        let entry = |n: usize| log(&db, 1)[n].clone();

        assert!(!record("z", AbandonOutcome::Sent, None, true), "no entry");
        assert!(
            !db.record_shared_abandon_outcome_durable(
                W,
                &id(2),
                "a",
                AbandonOutcome::Sent,
                None,
                true,
                50
            )
            .unwrap()
        );

        assert!(record(
            "a",
            AbandonOutcome::NotSent,
            Some("send.no_peers"),
            false
        ));
        assert_eq!(entry(0), ("a".into(), None, None), "a replay's refusal");
        assert_eq!(last_error().as_deref(), Some("send.no_peers"));

        assert!(record(
            "a",
            AbandonOutcome::NotSent,
            Some("send.no_peers"),
            true
        ));
        assert_eq!(
            entry(0),
            (
                "a".into(),
                Some("not_sent".into()),
                Some("send.no_peers".into())
            )
        );
        assert!(record(
            "a",
            AbandonOutcome::MaybeSent,
            Some("send.broadcast_unknown"),
            false
        ));
        assert_eq!(
            entry(0),
            (
                "a".into(),
                Some("maybe_sent".into()),
                Some("send.broadcast_unknown".into())
            )
        );
        assert!(record(
            "a",
            AbandonOutcome::NotSent,
            Some("send.cancelled"),
            false
        ));
        assert_eq!(entry(0).1.as_deref(), Some("maybe_sent"), "never back");
        assert_eq!(last_error().as_deref(), Some("send.cancelled"));
        assert!(record("a", AbandonOutcome::Sent, None, false));
        assert_eq!(entry(0), ("a".into(), Some("sent".into()), None));
        assert_eq!(last_error(), None);
        assert!(record(
            "a",
            AbandonOutcome::MaybeSent,
            Some("send.broadcast_unknown"),
            false
        ));
        assert_eq!(entry(0).1.as_deref(), Some("sent"));
        assert_eq!(entry(1), ("b".into(), None, None), "only its own entry");
    }

    /// Sol r5 m1: a replay first makes a missing outcome `maybe_sent`, so
    /// its refusal can never read as the first hand-off's.
    #[test]
    fn a_replay_mark_moves_only_a_missing_outcome() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0)]);
        for txid in ["a", "b", "c", "d"] {
            assert_eq!(abandon(&db, 1, txid, &[("t", 0)]), Some(false));
        }
        let record = |txid: &str, o| {
            db.record_shared_abandon_outcome_durable(W, &id(1), txid, o, None, true, 50)
                .unwrap()
        };
        assert!(record("b", AbandonOutcome::NotSent));
        assert!(record("c", AbandonOutcome::Sent));
        assert!(record("d", AbandonOutcome::MaybeSent));
        for txid in ["a", "b", "c", "d"] {
            assert!(
                db.mark_shared_abandon_replay_durable(W, &id(1), txid, 60)
                    .unwrap()
            );
        }
        assert!(
            !db.mark_shared_abandon_replay_durable(W, &id(1), "z", 60)
                .unwrap()
        );
        let outcomes: Vec<_> = log(&db, 1).into_iter().map(|(_, o, _)| o).collect();
        assert_eq!(
            outcomes,
            [
                Some("maybe_sent".to_string()),
                Some("not_sent".into()),
                Some("sent".into()),
                Some("maybe_sent".into())
            ]
        );
    }

    /// Review r1 M1: a spend signed again with a logged txid leaves again,
    /// so a missing or refused outcome becomes `maybe_sent`; the write says
    /// it was logged. A later outcome stays.
    #[test]
    fn a_relogged_spend_reads_maybe_sent_and_reports_it_was_logged() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0)]);
        for txid in ["a", "b", "c", "d"] {
            assert_eq!(abandon(&db, 1, txid, &[("t", 0)]), Some(false));
        }
        let record = |txid: &str, o, code: Option<&str>| {
            db.record_shared_abandon_outcome_durable(W, &id(1), txid, o, code, true, 50)
                .unwrap()
        };
        assert!(record("b", AbandonOutcome::NotSent, Some("send.no_peers")));
        assert!(record(
            "c",
            AbandonOutcome::MaybeSent,
            Some("send.broadcast_unknown")
        ));
        assert!(record("d", AbandonOutcome::Sent, None));
        for txid in ["a", "b", "c", "d"] {
            assert_eq!(abandon(&db, 1, txid, &[("t", 0)]), Some(true), "{txid}");
        }
        assert_eq!(
            log(&db, 1),
            [
                ("a".into(), Some("maybe_sent".into()), None),
                ("b".into(), Some("maybe_sent".into()), None),
                (
                    "c".into(),
                    Some("maybe_sent".into()),
                    Some("send.broadcast_unknown".into())
                ),
                ("d".into(), Some("sent".into()), None),
            ]
        );
    }

    /// Review r1 n3: the entry writes bump `updated_at` like every other
    /// write, and never move it back.
    #[test]
    fn the_entry_writes_bump_updated_at() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[("t", 0)]);
        let at = || db.shared_session(W, &id(1)).unwrap().unwrap().updated_at;
        assert_eq!(abandon(&db, 1, "a", &[("t", 0)]), Some(false));
        assert_eq!(at(), 40);
        assert!(
            db.mark_shared_abandon_replay_durable(W, &id(1), "a", 60)
                .unwrap()
        );
        assert_eq!(at(), 60);
        assert!(
            db.record_shared_abandon_outcome_durable(
                W,
                &id(1),
                "a",
                AbandonOutcome::Sent,
                None,
                false,
                70
            )
            .unwrap()
        );
        assert_eq!(at(), 70);
        assert!(
            db.record_shared_abandon_outcome_durable(
                W,
                &id(1),
                "a",
                AbandonOutcome::Sent,
                None,
                false,
                65
            )
            .unwrap()
        );
        assert_eq!(at(), 70, "never back");
    }

    /// R2, Opus r6 n3: the resend mark, on out rows only, and never cleared.
    #[test]
    fn the_resend_mark_sets_the_marker_and_the_flag() {
        let db = AppDb::open_in_memory().unwrap();
        db.insert_shared_session_durable(W, &row(1, &[]), 10)
            .unwrap();
        assert!(!db.mark_shared_session_resend_durable(W, &id(1)).unwrap());
        assert!(!db.mark_shared_session_resend_durable(W, &id(2)).unwrap());
        let r = db.shared_session(W, &id(1)).unwrap().unwrap();
        assert!(!r.resend_marked && r.last_error.is_none());

        out(&db, 3, &[]);
        assert!(db.mark_shared_session_resend_durable(W, &id(3)).unwrap());
        let r = db.shared_session(W, &id(3)).unwrap().unwrap();
        assert!(r.resend_marked);
        assert_eq!(r.last_error.as_deref(), Some(RESEND_IN_PROGRESS));
        db.record_shared_session_attempt(W, &id(3), Some("send.no_peers"))
            .unwrap();
        let r = db.shared_session(W, &id(3)).unwrap().unwrap();
        assert!(r.resend_marked, "kept after the outcome");
        assert_eq!(r.last_error.as_deref(), Some("send.no_peers"));
    }

    #[test]
    fn the_abandon_columns_check_their_values() {
        let db = AppDb::open_in_memory().unwrap();
        out(&db, 1, &[]);
        let set = |sql: &str| {
            db.conn()
                .execute(
                    &format!("UPDATE shared_mn_sessions SET {sql} WHERE id = ?1"),
                    [id(1)],
                )
                .map(|_| ())
        };
        assert!(set("abandon_tx = x'01'").is_err(), "bytes need their id");
        assert!(set("abandon_log = 'not json'").is_err());
        assert!(set("legacy_bound = 2").is_err());
        assert!(set("resend_marked = -1").is_err());
        assert!(set("legacy_bound = NULL").is_err());
        assert!(set("abandon_txid = 'a', abandon_tx = x'01', abandon_log = '[]'").is_ok());
        assert!(set("abandon_txid = NULL").is_err());
    }
}
