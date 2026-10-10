//! The dispatch journal: `dispatch.sqlite`, one per network directory
//! (E0-04 design §6). Only the engine's dispatch fence writes it.
//!
//! - `dispatch` holds registered artifacts (asset locks, Mode A) with their
//!   recovery payload. On disk a row moves 0 (`Unsent`) → 1 (`Dispatching`)
//!   only; 2 (`PreFence`) is written only by the seeding. No statement sets
//!   `state` to 0, and the only delete is [`DispatchJournal::erase_wallet`].
//! - `step_log` is append-only (DEC-154): the write-ahead markers of
//!   resumable row-less steps (§7.6), each "possibly dispatched", and the
//!   artifacts' resolutions (`NotSent`, `Sent`, `MaybeSent`), in one
//!   sequence. A `NotSent` row supersedes the artifact's earlier markers; a
//!   `Sent` or `MaybeSent` row is final, so no later `NotSent` revokes them
//!   ([`standing`]). No row is changed. Rows are deleted under one
//!   predicate only (DEC-160): an artifact's rows go once it has no standing
//!   marker and no `Sent` or `MaybeSent` row ([`compactable`]). The sweep at
//!   open and [`DispatchJournal::erase_wallet`] evaluate it inside their
//!   `DELETE`; whatever it keeps outlives a wallet's removal.
//! - `funding_marker` and `self_spend` (schema 3, Mode B, §2a.3) are
//!   append-only too. A funding step's marker row is written with its
//!   `funding_marker` row (the funding key) in one transaction, which shares
//!   the marker row's `seq`, so it stands exactly while that marker stands
//!   ([`standing_calls`]). A `self_spend` row records Repair's self-spend of
//!   a funding against the marker standing when it was written (its
//!   `marker`). Both tables' rows of an artifact go exactly when its
//!   `step_log` rows are compacted, in the same `DELETE`'s transaction.
//! - `meta` holds the schema version, the creation time and the per-wallet
//!   `seeded:<wallet hex>` marks.
//!
//! The file is separate from `app.sqlite` (§6.2): it runs
//! `synchronous=FULL` (and `fullfsync` on Apple platforms), it stays out of
//! `.dwbackup`'s row export, and its rows outlive the wallet rows they
//! describe. A schema newer than this build is reported as
//! [`JournalOpen::NewerSchema`] instead of failing a CHECK later; the
//! engine then disables its fence (§6.4).
//!
//! Like [`crate::AppDb`], every method is a short synchronous SQLite call on
//! one connection behind a mutex; async callers run them on the blocking
//! pool.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{AppDbError, Result};

/// File name of the journal inside a network directory.
pub const DISPATCH_DB_FILE: &str = "dispatch.sqlite";

/// The schema this build reads and writes. Version 1 (a `step` table whose
/// rows a definite resolution deleted) is migrated at open (DEC-154), and
/// so is version 2 (no Mode B tables); both migrations only add.
pub const DISPATCH_SCHEMA: u32 = 3;

const SCHEMA_BASE: &str = "
CREATE TABLE meta (k TEXT PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID;
CREATE TABLE dispatch (
  wallet        BLOB NOT NULL CHECK (length(wallet) = 32),
  txid          BLOB NOT NULL CHECK (length(txid) = 32),
  origin_lease  BLOB CHECK (origin_lease IS NULL OR length(origin_lease) = 16),
  process       BLOB CHECK (process IS NULL OR length(process) = 16),
  state         INTEGER NOT NULL CHECK (state IN (0, 1, 2)),
  payload       BLOB NOT NULL,
  registered_at INTEGER NOT NULL,
  dispatched_at INTEGER,
  PRIMARY KEY (wallet, txid)
) WITHOUT ROWID;
";

/// Version 1's marker table, for the migration's tests.
#[cfg(test)]
const STEP_V1: &str = "
CREATE TABLE step (
  wallet    BLOB NOT NULL CHECK (length(wallet) = 32),
  step_id   TEXT NOT NULL,
  artifact  BLOB NOT NULL CHECK (length(artifact) = 32),
  state     INTEGER NOT NULL CHECK (state = 1),
  at        INTEGER NOT NULL,
  PRIMARY KEY (wallet, step_id, artifact)
) WITHOUT ROWID;
";

/// `kind`: 0 a marker of `step_id`, 1 `NotSent`, 2 `Sent`, 3 `MaybeSent`.
const STEP_LOG: &str = "
CREATE TABLE step_log (
  seq       INTEGER PRIMARY KEY AUTOINCREMENT,
  wallet    BLOB NOT NULL CHECK (length(wallet) = 32),
  artifact  BLOB NOT NULL CHECK (length(artifact) = 32),
  kind      INTEGER NOT NULL CHECK (kind IN (0, 1, 2, 3)),
  step_id   TEXT CHECK ((kind = 0) = (step_id IS NOT NULL)),
  at        INTEGER NOT NULL
);
CREATE INDEX step_log_artifact ON step_log (wallet, artifact, seq);
";

/// Version 1 → 2: the markers move to `step_log` in a stable order. A
/// version 1 file holds no resolution (its definite resolutions deleted
/// their markers), so every marker stands.
const MIGRATE_V1: &str = "
INSERT INTO step_log (wallet, artifact, kind, step_id, at)
  SELECT wallet, artifact, 0, step_id, at FROM step ORDER BY at, wallet, step_id, artifact;
DROP TABLE step;
UPDATE meta SET v = 2 WHERE k = 'schema';
";

/// Version 3 (Mode B, E0-04 P2b): a funding step's key, and Repair's
/// self-spends (§2a.3, §2a.5). Both append-only. A `funding_marker` row's
/// `seq` is the `seq` of the `step_log` marker row written with it; a
/// `self_spend` row's `marker` is the `seq` of the marker it answers.
const MODE_B: &str = "
CREATE TABLE funding_marker (
  seq            INTEGER PRIMARY KEY AUTOINCREMENT,
  wallet         BLOB NOT NULL CHECK (length(wallet) = 32),
  step_id        TEXT NOT NULL,
  artifact       BLOB NOT NULL CHECK (length(artifact) = 32),
  funding_type   INTEGER NOT NULL CHECK (funding_type BETWEEN 0 AND 255),
  identity_index INTEGER NOT NULL CHECK (identity_index BETWEEN 0 AND 4294967295),
  account        INTEGER NOT NULL CHECK (account BETWEEN 0 AND 4294967295),
  at             INTEGER NOT NULL
);
CREATE INDEX funding_marker_artifact ON funding_marker (wallet, artifact);
CREATE TABLE self_spend (
  seq      INTEGER PRIMARY KEY AUTOINCREMENT,
  wallet   BLOB NOT NULL CHECK (length(wallet) = 32),
  step_id  TEXT NOT NULL,
  artifact BLOB NOT NULL CHECK (length(artifact) = 32),
  txid     BLOB NOT NULL CHECK (length(txid) = 32),
  marker   INTEGER NOT NULL,
  at       INTEGER NOT NULL
);
CREATE INDEX self_spend_artifact ON self_spend (wallet, artifact);
";

/// Version 2 → 3: the Mode B tables are new and empty.
const MIGRATE_V2: &str = "UPDATE meta SET v = 3 WHERE k = 'schema';";

/// A registered artifact's state on disk (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskState {
    /// 0: registered, never possibly sent.
    Unsent,
    /// 1: possibly sent; only Resends from now on.
    Dispatching,
    /// 2: the row predates the journal; possibly sent.
    PreFence,
}

impl DiskState {
    fn from_db(v: i64) -> Result<Self> {
        match v {
            0 => Ok(Self::Unsent),
            1 => Ok(Self::Dispatching),
            2 => Ok(Self::PreFence),
            other => Err(AppDbError::Corrupt(format!("dispatch.state {other}"))),
        }
    }
}

/// One `dispatch` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRow {
    pub wallet: [u8; 32],
    pub txid: [u8; 32],
    /// `None` for `PreFence`.
    pub origin_lease: Option<[u8; 16]>,
    pub process: Option<[u8; 16]>,
    pub state: DiskState,
    pub payload: Vec<u8>,
    pub registered_at: u64,
    pub dispatched_at: Option<u64>,
}

/// One standing step marker, as recovery reads it ([`standing`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub wallet: [u8; 32],
    pub step_id: String,
    pub artifact: [u8; 32],
    pub at: u64,
    /// The artifact has a `Sent` row: it was seen sent (DEC-154).
    pub sent: bool,
}

/// An artifact's resolution, appended to `step_log` (DEC-154).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Definitely unsent: supersedes the artifact's earlier markers.
    NotSent,
    /// Seen sent: final.
    Sent,
    /// Possibly sent: final. Nothing in P2a writes it.
    MaybeSent,
}

impl Resolution {
    fn kind(self) -> i64 {
        match self {
            Self::NotSent => 1,
            Self::Sent => 2,
            Self::MaybeSent => 3,
        }
    }
}

/// One `step_log` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEvent {
    Marked(String),
    Resolved(Resolution),
}

/// One `step_log` row, in sequence order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepLogRow {
    /// The row's `seq`: what a `funding_marker` or `self_spend` row links.
    pub seq: u64,
    pub wallet: [u8; 32],
    pub artifact: [u8; 32],
    pub event: StepEvent,
    pub at: u64,
}

/// A funding step's key as stored (§2a.3): the funding type's stable code
/// (the engine owns the mapping), the identity index and the account, as
/// platform-wallet's tracked asset-lock rows carry them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StoredFundingKey {
    pub funding_type: u8,
    pub identity_index: u32,
    pub account: u32,
}

/// One `funding_marker` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FundingRow {
    /// The `seq` of the `step_log` marker row it was written with.
    pub seq: u64,
    pub wallet: [u8; 32],
    pub step_id: String,
    pub artifact: [u8; 32],
    pub key: StoredFundingKey,
    pub at: u64,
}

/// One `self_spend` row: Repair's self-spend `txid` of a funding (§6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfSpendRow {
    pub seq: u64,
    pub wallet: [u8; 32],
    pub step_id: String,
    pub artifact: [u8; 32],
    pub txid: [u8; 32],
    /// The `seq` of the marker that stood when it was written.
    pub marker: u64,
    pub at: u64,
}

/// The Mode B rows recovery reads ([`standing_calls`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallRows {
    pub funding: Vec<FundingRow>,
    pub self_spends: Vec<SelfSpendRow>,
}

/// The (wallet, artifact) pairs whose `step_log` rows may be deleted
/// (DEC-160): no standing marker and no `Sent` or `MaybeSent` row. The
/// journal evaluates the same predicate in SQL ([`COMPACTABLE`]); this is
/// its reading for in-memory journals and the tests that hold them equal.
pub fn compactable(log: &[StepLogRow]) -> HashSet<([u8; 32], [u8; 32])> {
    let keep: HashSet<_> = standing(log)
        .into_iter()
        .map(|s| (s.wallet, s.artifact))
        .chain(log.iter().filter_map(|r| {
            matches!(
                r.event,
                StepEvent::Resolved(Resolution::Sent | Resolution::MaybeSent)
            )
            .then_some((r.wallet, r.artifact))
        }))
        .collect();
    log.iter()
        .map(|r| (r.wallet, r.artifact))
        .filter(|k| !keep.contains(k))
        .collect()
}

/// The in-memory reading of [`compact`] (DEC-160): deletes the rows of
/// every artifact [`compactable`] selects (of `wallet`, or of all wallets),
/// its `funding_marker` and `self_spend` rows with them, and returns the
/// pairs it erased. For in-memory journals and the tests that hold it
/// equal to the SQL.
pub fn compact_in_memory(
    log: &mut Vec<StepLogRow>,
    funding: &mut Vec<FundingRow>,
    self_spends: &mut Vec<SelfSpendRow>,
    wallet: Option<&[u8; 32]>,
) -> BTreeSet<([u8; 32], [u8; 32])> {
    let gone: BTreeSet<_> = compactable(log)
        .into_iter()
        .filter(|(w, _)| wallet.is_none_or(|x| x == w))
        .collect();
    log.retain(|r| !gone.contains(&(r.wallet, r.artifact)));
    funding.retain(|r| !gone.contains(&(r.wallet, r.artifact)));
    self_spends.retain(|r| !gone.contains(&(r.wallet, r.artifact)));
    gone
}

/// A step's marker, appended unless one of the same step already stands.
const APPEND_MARKER: &str = "
INSERT INTO step_log (wallet, artifact, kind, step_id, at)
SELECT ?1, ?2, 0, ?3, ?4 WHERE NOT EXISTS (
  SELECT 1 FROM step_log m WHERE m.wallet = ?1 AND m.artifact = ?2
    AND m.kind = 0 AND m.step_id = ?3
    AND (EXISTS (SELECT 1 FROM step_log f WHERE f.wallet = ?1
           AND f.artifact = ?2 AND f.kind IN (2, 3))
         OR m.seq > (SELECT coalesce(max(n.seq), 0) FROM step_log n
           WHERE n.wallet = ?1 AND n.artifact = ?2 AND n.kind = 1)))";

/// A resolution of kind ?3: a `NotSent` row unless it is already the
/// artifact's latest, a final row once.
const APPEND_RESOLUTION: &str = "
INSERT INTO step_log (wallet, artifact, kind, step_id, at)
SELECT ?1, ?2, ?3, NULL, ?4 WHERE CASE WHEN ?3 = 1
  THEN coalesce((SELECT kind FROM step_log WHERE wallet = ?1 AND artifact = ?2
                 ORDER BY seq DESC LIMIT 1), -1) <> 1
  ELSE NOT EXISTS (SELECT 1 FROM step_log
                   WHERE wallet = ?1 AND artifact = ?2 AND kind = ?3) END";

fn append_marker(
    c: &Connection,
    wallet: &[u8; 32],
    artifact: &[u8; 32],
    step_id: &str,
    now: u64,
) -> Result<()> {
    c.execute(
        APPEND_MARKER,
        params![&wallet[..], &artifact[..], step_id, now as i64],
    )?;
    Ok(())
}

fn append_resolution(
    c: &Connection,
    wallet: &[u8; 32],
    artifact: &[u8; 32],
    r: Resolution,
    now: u64,
) -> Result<()> {
    c.execute(
        APPEND_RESOLUTION,
        params![&wallet[..], &artifact[..], r.kind(), now as i64],
    )?;
    Ok(())
}

/// The `seq` of the latest marker of a step (?1 wallet, ?2 artifact, ?3
/// step), if it stands: a marker of the step stands exactly when its latest
/// one does.
const LATEST_STANDING: &str = "
SELECT m.seq FROM step_log m
WHERE m.seq = (SELECT max(seq) FROM step_log
               WHERE wallet = ?1 AND artifact = ?2 AND kind = 0 AND step_id = ?3)
  AND (EXISTS (SELECT 1 FROM step_log f WHERE f.wallet = ?1
         AND f.artifact = ?2 AND f.kind IN (2, 3))
       OR m.seq > (SELECT coalesce(max(n.seq), 0) FROM step_log n
         WHERE n.wallet = ?1 AND n.artifact = ?2 AND n.kind = 1))";

fn latest_standing(
    c: &Connection,
    wallet: &[u8; 32],
    artifact: &[u8; 32],
    step_id: &str,
) -> Result<Option<i64>> {
    Ok(c.query_row(
        LATEST_STANDING,
        params![&wallet[..], &artifact[..], step_id],
        |r| r.get(0),
    )
    .optional()?)
}

/// A funding step's key, with the standing marker of the step that has
/// none yet (the one the caller just appended, or one that stood already).
/// Its `seq` is the marker row's, so the two stand and go together. The
/// caller runs it right after the marker statement, in its transaction.
fn append_funding(
    c: &Connection,
    wallet: &[u8; 32],
    artifact: &[u8; 32],
    step_id: &str,
    key: &StoredFundingKey,
    now: u64,
) -> Result<()> {
    let Some(marker) = latest_standing(c, wallet, artifact, step_id)? else {
        return Ok(());
    };
    c.execute(
        "INSERT INTO funding_marker
           (seq, wallet, step_id, artifact, funding_type, identity_index, account, at)
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8
         WHERE NOT EXISTS (SELECT 1 FROM funding_marker WHERE seq = ?1)",
        params![
            marker,
            &wallet[..],
            step_id,
            &artifact[..],
            key.funding_type,
            key.identity_index,
            key.account,
            now as i64
        ],
    )?;
    Ok(())
}

/// DEC-160's predicate over `step_log l`, in SQL: [`compactable`].
const COMPACTABLE: &str = "
  NOT EXISTS (SELECT 1 FROM step_log f
              WHERE f.wallet = l.wallet AND f.artifact = l.artifact AND f.kind IN (2, 3))
  AND NOT EXISTS (SELECT 1 FROM step_log m
                  WHERE m.wallet = l.wallet AND m.artifact = l.artifact AND m.kind = 0
                    AND NOT EXISTS (SELECT 1 FROM step_log n
                                    WHERE n.wallet = m.wallet AND n.artifact = m.artifact
                                      AND n.kind = 1 AND n.seq > m.seq))";

/// Deletes the rows of every compactable artifact (of `wallet`, or of all
/// wallets) in one statement, so the predicate and the delete see the same
/// rows, and then those artifacts' `funding_marker` and `self_spend` rows;
/// returns the artifacts it erased. [`compact_in_memory`] is its reading.
fn compact(conn: &Connection, wallet: Option<&[u8; 32]>) -> Result<Vec<([u8; 32], [u8; 32])>> {
    let sql = format!(
        "DELETE FROM step_log WHERE seq IN (
           SELECT l.seq FROM step_log l WHERE (?1 IS NULL OR l.wallet = ?1) AND {COMPACTABLE})
         RETURNING wallet, artifact"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![wallet.map(|w| &w[..])], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = BTreeSet::new();
    for r in rows {
        let (w, a) = r?;
        out.insert((fixed(w, "step_log.wallet")?, fixed(a, "step_log.artifact")?));
    }
    drop(stmt);
    // The artifact's Mode B rows go exactly with its `step_log` rows, in
    // the caller's transaction.
    for (w, a) in &out {
        for table in ["funding_marker", "self_spend"] {
            conn.execute(
                &format!("DELETE FROM {table} WHERE wallet = ?1 AND artifact = ?2"),
                params![&w[..], &a[..]],
            )?;
        }
    }
    Ok(out.into_iter().collect())
}

/// Per row of `log`: whether it is a marker that stands (DEC-154). A
/// marker stands unless a later `NotSent` row of its artifact supersedes
/// it; any `Sent` or `MaybeSent` row of the artifact is final, so all its
/// markers stand whatever follows.
fn marker_stands(log: &[StepLogRow]) -> Vec<bool> {
    use std::collections::HashMap;
    type Key = ([u8; 32], [u8; 32]);
    let mut last_not_sent: HashMap<Key, usize> = HashMap::new();
    let mut final_: HashSet<Key> = HashSet::new();
    for (n, r) in log.iter().enumerate() {
        let key = (r.wallet, r.artifact);
        match r.event {
            StepEvent::Resolved(Resolution::NotSent) => {
                last_not_sent.insert(key, n);
            }
            StepEvent::Resolved(Resolution::Sent | Resolution::MaybeSent) => {
                final_.insert(key);
            }
            StepEvent::Marked(_) => {}
        }
    }
    log.iter()
        .enumerate()
        .map(|(n, r)| {
            let key = (r.wallet, r.artifact);
            matches!(r.event, StepEvent::Marked(_))
                && (final_.contains(&key) || last_not_sent.get(&key).is_none_or(|&m| n > m))
        })
        .collect()
}

/// Recovery's reading of `step_log` (DEC-154): the markers that stand, one
/// per (wallet, step, artifact), earliest first ([`marker_stands`]).
pub fn standing(log: &[StepLogRow]) -> Vec<StepRow> {
    let sent: HashSet<([u8; 32], [u8; 32])> = log
        .iter()
        .filter(|r| r.event == StepEvent::Resolved(Resolution::Sent))
        .map(|r| (r.wallet, r.artifact))
        .collect();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for (r, stands) in log.iter().zip(marker_stands(log)) {
        let StepEvent::Marked(step) = &r.event else {
            continue;
        };
        let key = (r.wallet, r.artifact);
        if stands && seen.insert((key, step.clone())) {
            out.push(StepRow {
                wallet: r.wallet,
                step_id: step.clone(),
                artifact: r.artifact,
                at: r.at,
                sent: sent.contains(&key),
            });
        }
    }
    out
}

/// Recovery's reading of the Mode B rows (§2a.3): a `funding_marker` row
/// stands exactly while the marker row it was written with stands, and a
/// `self_spend` row while the marker it answers does. A row whose marker
/// is gone, or names another step, does not stand.
pub fn standing_calls(
    log: &[StepLogRow],
    funding: &[FundingRow],
    self_spends: &[SelfSpendRow],
) -> CallRows {
    use std::collections::HashMap;
    let stands: HashMap<u64, (&[u8; 32], &[u8; 32], &str)> = log
        .iter()
        .zip(marker_stands(log))
        .filter_map(|(r, stands)| match &r.event {
            StepEvent::Marked(step) if stands => {
                Some((r.seq, (&r.wallet, &r.artifact, step.as_str())))
            }
            _ => None,
        })
        .collect();
    let of = |seq: u64, wallet: &[u8; 32], artifact: &[u8; 32], step: &str| {
        stands.get(&seq) == Some(&(wallet, artifact, step))
    };
    CallRows {
        funding: funding
            .iter()
            .filter(|f| of(f.seq, &f.wallet, &f.artifact, &f.step_id))
            .cloned()
            .collect(),
        self_spends: self_spends
            .iter()
            .filter(|s| of(s.marker, &s.wallet, &s.artifact, &s.step_id))
            .cloned()
            .collect(),
    }
}

/// What `register` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registered {
    /// The row was written now, or already existed with the same origin.
    Ok,
    /// A row for this txid exists with another origin (§5.5).
    OtherOrigin,
}

/// The result of opening the journal.
pub enum JournalOpen {
    Ready(DispatchJournal),
    /// The file's schema is newer than this build: the fence is disabled
    /// rather than failing a CHECK on an unknown state (§6.2).
    NewerSchema(u32),
}

/// The pragmas in effect, read back after opening (§12 "Journal").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalPragmas {
    pub journal_mode: String,
    /// 2 = FULL.
    pub synchronous: i64,
    pub secure_delete: i64,
    /// 1 on Apple platforms; not set elsewhere.
    pub fullfsync: i64,
}

pub struct DispatchJournal {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for DispatchJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DispatchJournal")
    }
}

impl DispatchJournal {
    /// Opens (creating if needed) the journal at `path`. The caller creates
    /// the file owner-only first.
    pub fn open(path: &Path, now: u64) -> Result<JournalOpen> {
        Self::init(Connection::open(path)?, now)
    }

    /// A private in-memory journal (tests).
    pub fn open_in_memory(now: u64) -> Result<JournalOpen> {
        Self::init(Connection::open_in_memory()?, now)
    }

    fn init(mut conn: Connection, now: u64) -> Result<JournalOpen> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        if cfg!(target_vendor = "apple") {
            // macOS fsync does not flush the disk cache.
            conn.pragma_update(None, "fullfsync", "ON")?;
            conn.pragma_update(None, "checkpoint_fullfsync", "ON")?;
        }
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        let tx = conn.transaction()?;
        let has_meta: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta')",
            [],
            |r| r.get(0),
        )?;
        if has_meta {
            let schema: Option<i64> = tx
                .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
                .optional()?;
            match schema {
                Some(v) if v == i64::from(DISPATCH_SCHEMA) => {}
                Some(v @ (1 | 2)) => {
                    if v == 1 {
                        tx.execute_batch(STEP_LOG)?;
                        tx.execute_batch(MIGRATE_V1)?;
                    }
                    tx.execute_batch(MODE_B)?;
                    tx.execute_batch(MIGRATE_V2)?;
                }
                Some(v) if v > i64::from(DISPATCH_SCHEMA) => {
                    return Ok(JournalOpen::NewerSchema(
                        u32::try_from(v).unwrap_or(u32::MAX),
                    ));
                }
                other => {
                    return Err(AppDbError::Corrupt(format!(
                        "dispatch meta.schema {other:?}"
                    )));
                }
            }
        } else {
            tx.execute_batch(SCHEMA_BASE)?;
            tx.execute_batch(STEP_LOG)?;
            tx.execute_batch(MODE_B)?;
            tx.execute(
                "INSERT INTO meta (k, v) VALUES ('schema', ?1), ('created_at', ?2)",
                params![i64::from(DISPATCH_SCHEMA), now as i64],
            )?;
        }
        sweep(&tx)?;
        tx.commit()?;
        Ok(JournalOpen::Ready(Self {
            conn: Mutex::new(conn),
        }))
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn pragmas(&self) -> Result<JournalPragmas> {
        let conn = self.conn();
        let get = |name: &str| conn.pragma_query_value(None, name, |r| r.get::<_, i64>(0));
        Ok(JournalPragmas {
            journal_mode: conn.pragma_query_value(None, "journal_mode", |r| r.get(0))?,
            synchronous: get("synchronous")?,
            secure_delete: get("secure_delete")?,
            fullfsync: get("fullfsync")?,
        })
    }

    /// `register` (§6.2): inserts an `Unsent` row, or finds the one already
    /// there. Same origin: a no-op. Another origin: [`Registered::OtherOrigin`].
    pub fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &[u8; 16],
        process: &[u8; 16],
        payload: &[u8],
        now: u64,
    ) -> Result<Registered> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO dispatch (wallet, txid, origin_lease, process, state, payload, registered_at)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6) ON CONFLICT DO NOTHING",
            params![&wallet[..], &txid[..], &origin[..], &process[..], payload, now as i64],
        )?;
        let stored: Option<Vec<u8>> = conn.query_row(
            "SELECT origin_lease FROM dispatch WHERE wallet = ?1 AND txid = ?2",
            params![&wallet[..], &txid[..]],
            |r| r.get(0),
        )?;
        Ok(if stored.as_deref() == Some(&origin[..]) {
            Registered::Ok
        } else {
            Registered::OtherOrigin
        })
    }

    /// The `Dispatching` write: `Unsent` → `Dispatching`. `Ok(true)` when the
    /// row is (now or already) at 1 or 2; `Ok(false)` when there is no row.
    pub fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32], now: u64) -> Result<bool> {
        let conn = self.conn();
        conn.execute(
            "UPDATE dispatch SET state = 1, dispatched_at = ?3
             WHERE wallet = ?1 AND txid = ?2 AND state = 0",
            params![&wallet[..], &txid[..], now as i64],
        )?;
        let state: Option<i64> = conn
            .query_row(
                "SELECT state FROM dispatch WHERE wallet = ?1 AND txid = ?2",
                params![&wallet[..], &txid[..]],
                |r| r.get(0),
            )
            .optional()?;
        Ok(matches!(state, Some(1 | 2)))
    }

    /// A resumable step's write-ahead marker (§7.6), appended unless one of
    /// the same step already stands (after a `NotSent` row, a later First
    /// of the same bytes appends it again). Idempotent.
    pub fn insert_step(
        &self,
        wallet: &[u8; 32],
        step_id: &str,
        artifact: &[u8; 32],
        now: u64,
    ) -> Result<()> {
        append_marker(&self.conn(), wallet, artifact, step_id, now)
    }

    /// A funding step's write-ahead marker with its funding key (§2a.3), in
    /// one transaction: the `step_log` marker, appended unless one of the
    /// step stands, and its `funding_marker` row, unless the standing
    /// marker has one already (the first key recorded under a marker
    /// stays). Idempotent.
    pub fn insert_funding_step(
        &self,
        wallet: &[u8; 32],
        step_id: &str,
        artifact: &[u8; 32],
        key: &StoredFundingKey,
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        append_marker(&tx, wallet, artifact, step_id, now)?;
        append_funding(&tx, wallet, artifact, step_id, key, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Repair's self-spend `txid` of the funding marked by `step_id`
    /// (§6.5, §2a.5), recorded against the step's standing marker; the
    /// engine writes the `NotSent` row only once the spend is ChainLocked.
    /// `Ok(false)`: no marker of the step stands, so nothing is recorded.
    /// Idempotent.
    pub fn insert_self_spend(
        &self,
        wallet: &[u8; 32],
        step_id: &str,
        artifact: &[u8; 32],
        txid: &[u8; 32],
        now: u64,
    ) -> Result<bool> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(marker) = latest_standing(&tx, wallet, artifact, step_id)? else {
            return Ok(false);
        };
        tx.execute(
            "INSERT INTO self_spend (wallet, step_id, artifact, txid, marker, at)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6
             WHERE NOT EXISTS (SELECT 1 FROM self_spend WHERE marker = ?5 AND txid = ?4)",
            params![
                &wallet[..],
                step_id,
                &artifact[..],
                &txid[..],
                marker,
                now as i64
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Appends `artifact`'s resolution (DEC-154). `NotSent` supersedes its
    /// markers so far: the engine refunds and reports it only after this
    /// returned (review P2a r1 F2). `Sent` is written once; a repeated
    /// `NotSent` with nothing in between is not written again. The engine
    /// writes a marked artifact's `Sent` row with [`Self::resolve_sent`].
    pub fn resolve(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        resolution: Resolution,
        now: u64,
    ) -> Result<()> {
        append_resolution(&self.conn(), wallet, artifact, resolution, now)
    }

    /// A marked artifact's `Sent` row with its complete marker set (DEC-163):
    /// each of `steps` appended unless it stands, with its funding key from
    /// `funding` for a funding step (§2a.3), then the row, in one
    /// transaction. Whatever order it takes against a `NotSent` row or a
    /// removal's compaction, the row never stands on disk without them.
    pub fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
        funding: &[(String, StoredFundingKey)],
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for step_id in steps {
            append_marker(&tx, wallet, artifact, step_id, now)?;
            if let Some((_, key)) = funding.iter().find(|(s, _)| s == step_id) {
                append_funding(&tx, wallet, artifact, step_id, key, now)?;
            }
        }
        append_resolution(&tx, wallet, artifact, Resolution::Sent, now)?;
        tx.commit()?;
        Ok(())
    }

    /// The seeding (§6.5, Mode A): a `PreFence` row per pre-fence asset lock
    /// of `wallet`, and its `seeded:` mark, in one transaction. Existing rows
    /// are kept as they are.
    pub fn seed_wallet(
        &self,
        wallet: &[u8; 32],
        rows: &[([u8; 32], Vec<u8>)],
        now: u64,
    ) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for (txid, payload) in rows {
            tx.execute(
                "INSERT INTO dispatch (wallet, txid, origin_lease, process, state, payload, registered_at)
                 VALUES (?1, ?2, NULL, NULL, 2, ?3, ?4) ON CONFLICT DO NOTHING",
                params![&wallet[..], &txid[..], payload, now as i64],
            )?;
        }
        tx.execute(
            "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            params![seeded_key(wallet), now as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn is_seeded(&self, wallet: &[u8; 32]) -> Result<bool> {
        Ok(self.conn().query_row(
            "SELECT EXISTS (SELECT 1 FROM meta WHERE k = ?1)",
            params![seeded_key(wallet)],
            |r| r.get(0),
        )?)
    }

    /// Every row, for the load (§6.5).
    pub fn load(&self) -> Result<(Vec<DispatchRow>, Vec<StepRow>)> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT wallet, txid, origin_lease, process, state, payload, registered_at, dispatched_at
             FROM dispatch",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Option<Vec<u8>>>(2)?,
                    r.get::<_, Option<Vec<u8>>>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Vec<u8>>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, Option<i64>>(7)?,
                ))
            })?
            .map(|row| {
                let (wallet, txid, origin, process, state, payload, at, dispatched) = row?;
                Ok(DispatchRow {
                    wallet: fixed(wallet, "dispatch.wallet")?,
                    txid: fixed(txid, "dispatch.txid")?,
                    origin_lease: origin
                        .map(|o| fixed(o, "dispatch.origin_lease"))
                        .transpose()?,
                    process: process.map(|p| fixed(p, "dispatch.process")).transpose()?,
                    state: DiskState::from_db(state)?,
                    payload,
                    registered_at: at as u64,
                    dispatched_at: dispatched.map(|d| d as u64),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((rows, standing(&step_log(&conn)?)))
    }

    /// The Mode B rows that stand, for the load (§2a.3, [`standing_calls`]).
    pub fn load_calls(&self) -> Result<CallRows> {
        let conn = self.conn();
        Ok(standing_calls(
            &step_log(&conn)?,
            &funding_rows(&conn)?,
            &self_spend_rows(&conn)?,
        ))
    }

    /// `step_log` in sequence order (tests and the sweep).
    pub fn step_log(&self) -> Result<Vec<StepLogRow>> {
        step_log(&self.conn())
    }

    /// Every `funding_marker` row, in sequence order (tests).
    pub fn funding_rows(&self) -> Result<Vec<FundingRow>> {
        funding_rows(&self.conn())
    }

    /// Every `self_spend` row, in sequence order (tests).
    pub fn self_spend_rows(&self) -> Result<Vec<SelfSpendRow>> {
        self_spend_rows(&self.conn())
    }

    /// A wallet's removal (§6.5), with `secure_delete` on, then a WAL
    /// truncation. With `dispatch` (the caller found no possibly sent
    /// entry) its `dispatch` rows and `seeded:` mark go. Of its `step_log`
    /// only the compactable artifacts' rows go, the predicate evaluated in
    /// the same transaction as the delete (DEC-160); the rest is evidence of
    /// something possibly on the wire and stays. Returns the artifacts whose
    /// rows were erased.
    pub fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>> {
        let mut conn = self.conn();
        // IMMEDIATE: the predicate and the delete see the rows no append
        // can slip between (DEC-160).
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if dispatch {
            tx.execute(
                "DELETE FROM dispatch WHERE wallet = ?1",
                params![&wallet[..]],
            )?;
            tx.execute("DELETE FROM meta WHERE k = ?1", params![seeded_key(wallet)])?;
        }
        let erased = compact(&tx, Some(wallet))?;
        tx.commit()?;
        // Best effort: the delete committed, and the caller forgets what it
        // erased, so no checkpoint error may report it as failed.
        let _ = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        Ok(erased.into_iter().map(|(_, a)| a).collect())
    }

    /// The unconditional delete DEC-160 removed, for the engine's `EraseAll`
    /// mutation test only.
    #[cfg(feature = "test-mutations")]
    #[doc(hidden)]
    pub fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>> {
        let mut conn = self.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let erased: BTreeSet<[u8; 32]> = step_log(&tx)?
            .into_iter()
            .filter(|r| r.wallet == *wallet)
            .map(|r| r.artifact)
            .collect();
        for table in ["step_log", "funding_marker", "self_spend"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE wallet = ?1"),
                params![&wallet[..]],
            )?;
        }
        tx.commit()?;
        Ok(erased.into_iter().collect())
    }
}

fn step_log(conn: &Connection) -> Result<Vec<StepLogRow>> {
    let mut stmt =
        conn.prepare("SELECT wallet, artifact, kind, step_id, at, seq FROM step_log ORDER BY seq")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })?
        .map(|row| {
            let (wallet, artifact, kind, step_id, at, seq) = row?;
            let event = match (kind, step_id) {
                (0, Some(s)) => StepEvent::Marked(s),
                (1, None) => StepEvent::Resolved(Resolution::NotSent),
                (2, None) => StepEvent::Resolved(Resolution::Sent),
                (3, None) => StepEvent::Resolved(Resolution::MaybeSent),
                (k, _) => return Err(AppDbError::Corrupt(format!("step_log.kind {k}"))),
            };
            Ok(StepLogRow {
                seq: seq as u64,
                wallet: fixed(wallet, "step_log.wallet")?,
                artifact: fixed(artifact, "step_log.artifact")?,
                event,
                at: at as u64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

fn funding_rows(conn: &Connection) -> Result<Vec<FundingRow>> {
    let mut stmt = conn.prepare(
        "SELECT seq, wallet, step_id, artifact, funding_type, identity_index, account, at
         FROM funding_marker ORDER BY seq",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, i64>(7)?,
            ))
        })?
        .map(|row| {
            let (seq, wallet, step_id, artifact, ty, index, account, at) = row?;
            let int = |v: i64, what: &str| {
                u32::try_from(v)
                    .map_err(|_| AppDbError::Corrupt(format!("funding_marker.{what} {v}")))
            };
            Ok(FundingRow {
                seq: seq as u64,
                wallet: fixed(wallet, "funding_marker.wallet")?,
                step_id,
                artifact: fixed(artifact, "funding_marker.artifact")?,
                key: StoredFundingKey {
                    funding_type: u8::try_from(ty).map_err(|_| {
                        AppDbError::Corrupt(format!("funding_marker.funding_type {ty}"))
                    })?,
                    identity_index: int(index, "identity_index")?,
                    account: int(account, "account")?,
                },
                at: at as u64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

fn self_spend_rows(conn: &Connection) -> Result<Vec<SelfSpendRow>> {
    let mut stmt = conn.prepare(
        "SELECT seq, wallet, step_id, artifact, txid, marker, at FROM self_spend ORDER BY seq",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .map(|row| {
            let (seq, wallet, step_id, artifact, txid, marker, at) = row?;
            Ok(SelfSpendRow {
                seq: seq as u64,
                wallet: fixed(wallet, "self_spend.wallet")?,
                step_id,
                artifact: fixed(artifact, "self_spend.artifact")?,
                txid: fixed(txid, "self_spend.txid")?,
                marker: marker as u64,
                at: at as u64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(rows)
}

/// The sweep at open (DEC-154 (4)): nothing of this process is on the
/// wire yet, so an artifact with no standing marker and no final row reads
/// the same with no rows at all; its rows go.
fn sweep(conn: &Connection) -> Result<()> {
    compact(conn, None).map(drop)
}

fn seeded_key(wallet: &[u8; 32]) -> String {
    let mut key = String::with_capacity(7 + 64);
    key.push_str("seeded:");
    for b in wallet {
        key.push_str(&format!("{b:02x}"));
    }
    key
}

fn fixed<const N: usize>(bytes: Vec<u8>, what: &str) -> Result<[u8; N]> {
    bytes
        .try_into()
        .map_err(|b: Vec<u8>| AppDbError::Corrupt(format!("{what}: {} bytes", b.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: [u8; 32] = [1; 32];
    const T: [u8; 32] = [2; 32];
    const L1: [u8; 16] = [3; 16];
    const L2: [u8; 16] = [4; 16];
    const P: [u8; 16] = [5; 16];

    fn journal() -> DispatchJournal {
        match DispatchJournal::open_in_memory(1).unwrap() {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
        }
    }

    #[test]
    fn register_is_idempotent_and_refuses_another_origin() {
        let j = journal();
        assert_eq!(
            j.register(&W, &T, &L1, &P, b"p", 1).unwrap(),
            Registered::Ok
        );
        assert_eq!(
            j.register(&W, &T, &L1, &P, b"q", 2).unwrap(),
            Registered::Ok
        );
        assert_eq!(
            j.register(&W, &T, &L2, &P, b"p", 3).unwrap(),
            Registered::OtherOrigin
        );
        let (rows, _) = j.load().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, DiskState::Unsent);
        assert_eq!(rows[0].payload, b"p", "the first payload is kept");
        assert_eq!(rows[0].origin_lease, Some(L1));
    }

    #[test]
    fn dispatching_moves_zero_to_one_only() {
        let j = journal();
        assert!(!j.mark_dispatching(&W, &T, 1).unwrap(), "no row");
        j.register(&W, &T, &L1, &P, b"p", 1).unwrap();
        assert!(j.mark_dispatching(&W, &T, 5).unwrap());
        assert!(j.mark_dispatching(&W, &T, 6).unwrap(), "already 1");
        let (rows, _) = j.load().unwrap();
        assert_eq!(rows[0].state, DiskState::Dispatching);
        assert_eq!(rows[0].dispatched_at, Some(5));
        // A re-register of a dispatched row leaves it at 1.
        j.register(&W, &T, &L1, &P, b"p", 7).unwrap();
        assert_eq!(j.load().unwrap().0[0].state, DiskState::Dispatching);
    }

    #[test]
    fn no_statement_moves_a_row_back_to_unsent() {
        // Schema test (§12 "Journal"): every statement that writes `state`
        // in this file writes 1 or 2; only the INSERT of `register` writes 0.
        let src = include_str!("dispatch.rs");
        let code = &src[..src.find("#[cfg(test)]\nmod tests").unwrap()];
        let lower = code.to_ascii_lowercase();
        for (i, _) in lower.match_indices("set state") {
            let rest = &lower[i..i + 16];
            assert!(rest.starts_with("set state = 1"), "{rest}");
        }
        assert_eq!(lower.matches("state, payload, registered_at)").count(), 2);
        assert_eq!(lower.matches("?4, 0, ?5").count(), 1, "one Unsent insert");
        assert!(!lower.contains("state = 0,"));
    }

    #[test]
    fn steps_are_write_once() {
        let j = journal();
        j.insert_step(&W, "registration/d/identity", &T, 1).unwrap();
        j.insert_step(&W, "registration/d/identity", &T, 2).unwrap();
        let (_, steps) = j.load().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].at, 1);
    }

    fn markers(j: &DispatchJournal) -> Vec<(String, [u8; 32], bool)> {
        j.load()
            .unwrap()
            .1
            .into_iter()
            .map(|s| (s.step_id, s.artifact, s.sent))
            .collect()
    }

    #[test]
    fn a_not_sent_row_supersedes_only_that_artifacts_earlier_markers() {
        let j = journal();
        let other = [9; 32];
        j.insert_step(&W, "registration/d/identity", &T, 1).unwrap();
        j.insert_step(&W, "withdrawal/d/submit", &T, 1).unwrap();
        j.insert_step(&W, "registration/d/identity", &other, 1)
            .unwrap();
        j.resolve(&W, &T, Resolution::NotSent, 2).unwrap();
        j.resolve(&W, &T, Resolution::NotSent, 3).unwrap();
        assert_eq!(
            markers(&j),
            vec![("registration/d/identity".into(), other, false)]
        );
        // DEC-154: nothing was deleted; one NotSent row was appended.
        assert_eq!(j.step_log().unwrap().len(), 4);
        // A later First of the same bytes appends its marker again, and
        // it stands until the next NotSent row.
        j.insert_step(&W, "registration/d/identity", &T, 4).unwrap();
        j.insert_step(&W, "registration/d/identity", &T, 5).unwrap();
        assert_eq!(markers(&j).len(), 2);
        assert_eq!(j.step_log().unwrap().len(), 5);
        j.resolve(&W, &T, Resolution::NotSent, 6).unwrap();
        assert_eq!(markers(&j).len(), 1);
    }

    #[test]
    fn a_sent_row_is_final_whatever_follows() {
        let j = journal();
        j.insert_step(&W, "s", &T, 1).unwrap();
        // Sent overtaking a NotSent resolution (Sol r2 R2-F1).
        j.resolve(&W, &T, Resolution::NotSent, 2).unwrap();
        assert!(markers(&j).is_empty());
        j.resolve(&W, &T, Resolution::Sent, 3).unwrap();
        assert_eq!(markers(&j), vec![("s".into(), T, true)]);
        // A later NotSent revokes nothing; Sent is written once.
        j.resolve(&W, &T, Resolution::NotSent, 4).unwrap();
        j.resolve(&W, &T, Resolution::Sent, 5).unwrap();
        assert_eq!(markers(&j), vec![("s".into(), T, true)]);
        assert_eq!(j.step_log().unwrap().len(), 4);
        // MaybeSent is final too, without reading as sent.
        let u = [8; 32];
        j.insert_step(&W, "s", &u, 6).unwrap();
        j.resolve(&W, &u, Resolution::MaybeSent, 7).unwrap();
        j.resolve(&W, &u, Resolution::NotSent, 8).unwrap();
        assert!(markers(&j).contains(&("s".into(), u, false)));
    }

    /// DEC-163: a `Sent` row written with its marker set in one transaction
    /// never stands bare, in any order against a `NotSent` row and the
    /// removal's compaction; written as separate statements, one order
    /// leaves it bare (Sol r4's control).
    #[test]
    fn a_sent_bundle_never_lands_without_its_markers() {
        #[derive(Clone, Copy, Debug)]
        enum Op {
            NotSent,
            Erase,
            Bundle,
            Marker,
            Sent,
        }
        fn orders(ops: &[Op]) -> Vec<Vec<Op>> {
            if ops.len() <= 1 {
                return vec![ops.to_vec()];
            }
            (0..ops.len())
                .flat_map(|i| {
                    let mut rest = ops.to_vec();
                    let op = rest.remove(i);
                    orders(&rest).into_iter().map(move |mut o| {
                        o.insert(0, op);
                        o
                    })
                })
                .collect()
        }
        let bare = |ops: &[Op]| {
            orders(ops)
                .into_iter()
                .filter(|order| {
                    let j = journal();
                    j.insert_step(&W, "s", &T, 1).unwrap();
                    for op in order {
                        match op {
                            Op::NotSent => j.resolve(&W, &T, Resolution::NotSent, 2).unwrap(),
                            Op::Erase => drop(j.erase_wallet(&W, false).unwrap()),
                            Op::Bundle => j.resolve_sent(&W, &T, &["s".into()], &[], 3).unwrap(),
                            Op::Marker => j.insert_step(&W, "s", &T, 3).unwrap(),
                            Op::Sent => j.resolve(&W, &T, Resolution::Sent, 3).unwrap(),
                        }
                    }
                    markers(&j) != vec![("s".into(), T, true)]
                })
                .count()
        };
        assert_eq!(bare(&[Op::NotSent, Op::Erase, Op::Bundle]), 0);
        assert!(bare(&[Op::NotSent, Op::Erase, Op::Marker, Op::Sent]) > 0);
    }

    #[test]
    fn the_sweep_at_open_keeps_every_standing_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        let open = |now| match DispatchJournal::open(&path, now).unwrap() {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
        };
        let j = open(1);
        let (gone, kept, sent) = ([7; 32], [8; 32], [9; 32]);
        for a in [gone, kept, sent] {
            j.insert_step(&W, "s", &a, 1).unwrap();
            j.resolve(&W, &a, Resolution::NotSent, 2).unwrap();
        }
        j.insert_step(&W, "s", &kept, 3).unwrap();
        j.resolve(&W, &sent, Resolution::Sent, 3).unwrap();
        let before = markers(&j);
        drop(j);
        let j = open(2);
        assert_eq!(markers(&j), before);
        let log = j.step_log().unwrap();
        assert!(log.iter().all(|r| r.artifact != gone));
        assert_eq!(log.len(), 6, "kept and sent keep every row");
    }

    #[test]
    fn a_version_1_journal_migrates_with_every_marker_standing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_BASE).unwrap();
        conn.execute_batch(STEP_V1).unwrap();
        conn.execute(
            "INSERT INTO meta (k, v) VALUES ('schema', 1), ('created_at', 1)",
            [],
        )
        .unwrap();
        for (step, at) in [("b", 2), ("a", 1)] {
            conn.execute(
                "INSERT INTO step (wallet, step_id, artifact, state, at) VALUES (?1, ?2, ?3, 1, ?4)",
                params![&W[..], step, &T[..], at],
            )
            .unwrap();
        }
        drop(conn);
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 3).unwrap() else {
            panic!("newer schema");
        };
        assert_eq!(
            markers(&j),
            vec![("a".into(), T, false), ("b".into(), T, false)]
        );
        let schema: i64 = j
            .conn()
            .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(schema, 3, "through version 2 to 3");
        let old: bool = j
            .conn()
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE name = 'step')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!old);
        assert_mode_b_tables(&j);
    }

    fn assert_mode_b_tables(j: &DispatchJournal) {
        for table in ["funding_marker", "self_spend"] {
            let there: bool = j
                .conn()
                .query_row(
                    "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(there, "{table}");
        }
    }

    /// Schema 3 only adds: a version 2 file keeps every row and reading.
    #[test]
    fn a_version_2_journal_migrates_to_3_keeping_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(SCHEMA_BASE).unwrap();
        conn.execute_batch(STEP_LOG).unwrap();
        conn.execute(
            "INSERT INTO meta (k, v) VALUES ('schema', 2), ('created_at', 1)",
            [],
        )
        .unwrap();
        for (artifact, kind, step) in [(T, 0, Some("a")), (T, 2, None), ([9; 32], 0, Some("b"))] {
            conn.execute(
                "INSERT INTO step_log (wallet, artifact, kind, step_id, at) VALUES (?1, ?2, ?3, ?4, 1)",
                params![&W[..], &artifact[..], kind, step],
            )
            .unwrap();
        }
        drop(conn);
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 3).unwrap() else {
            panic!("newer schema");
        };
        assert_eq!(
            markers(&j),
            vec![("a".into(), T, true), ("b".into(), [9; 32], false)]
        );
        assert_eq!(j.step_log().unwrap().len(), 3);
        let schema: i64 = j
            .conn()
            .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(schema, 3);
        assert_mode_b_tables(&j);
        assert_eq!(j.load_calls().unwrap(), CallRows::default());
        // The new tables take writes; a reopen keeps them.
        j.insert_funding_step(&W, "topup/i/funding", &[7; 32], &KEY, 4)
            .unwrap();
        drop(j);
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 5).unwrap() else {
            panic!("newer schema");
        };
        assert_eq!(j.load_calls().unwrap().funding.len(), 1);
    }

    const KEY: StoredFundingKey = StoredFundingKey {
        funding_type: 1,
        identity_index: 4,
        account: 0,
    };

    fn calls(j: &DispatchJournal) -> Vec<(String, StoredFundingKey)> {
        j.load_calls()
            .unwrap()
            .funding
            .into_iter()
            .map(|f| (f.step_id, f.key))
            .collect()
    }

    /// A funding marker is its `step_log` marker row and its key, written
    /// together, sharing the marker's `seq`; it stands exactly while the
    /// marker does.
    #[test]
    fn a_funding_marker_stands_and_goes_with_its_step_marker() {
        let j = journal();
        let step = "registration/d/funding";
        j.insert_funding_step(&W, step, &T, &KEY, 1).unwrap();
        j.insert_funding_step(&W, step, &T, &KEY, 2).unwrap();
        let log = j.step_log().unwrap();
        let funding = j.funding_rows().unwrap();
        assert_eq!(log.len(), 1, "one marker");
        assert_eq!(funding.len(), 1, "one key");
        assert_eq!(funding[0].seq, log[0].seq, "linked by seq");
        assert_eq!(calls(&j), vec![(step.into(), KEY)]);
        // While the marker stands, a later call's other key is not added:
        // the first key recorded under the marker stays.
        let other = StoredFundingKey {
            identity_index: 5,
            ..KEY
        };
        j.insert_funding_step(&W, step, &T, &other, 3).unwrap();
        assert_eq!(calls(&j), vec![(step.into(), KEY)]);
        // NotSent supersedes the marker and its key.
        j.resolve(&W, &T, Resolution::NotSent, 4).unwrap();
        assert!(calls(&j).is_empty());
        assert!(markers(&j).is_empty());
        // A new call of the step: a new marker with its own key.
        j.insert_funding_step(&W, step, &T, &other, 5).unwrap();
        assert_eq!(calls(&j), vec![(step.into(), other)]);
        assert_eq!(j.funding_rows().unwrap().len(), 2, "append-only");
        // A plain marker of the step (never written by the engine for a
        // funding step) carries no key and makes no row stand.
        j.insert_step(&W, "s", &[3; 32], 6).unwrap();
        assert_eq!(calls(&j).len(), 1);
    }

    /// DEC-163: a `Sent` bundle re-appends a funding step's key with its
    /// marker, so the row never stands without either, even after a
    /// `NotSent` row and the compaction took them.
    #[test]
    fn a_sent_bundle_carries_the_funding_marker() {
        let j = journal();
        let step = "topup/i/funding";
        j.insert_funding_step(&W, step, &T, &KEY, 1).unwrap();
        j.resolve(&W, &T, Resolution::NotSent, 2).unwrap();
        assert_eq!(j.erase_wallet(&W, false).unwrap(), vec![T]);
        assert!(j.funding_rows().unwrap().is_empty(), "compacted with it");
        j.resolve_sent(&W, &T, &[step.into()], &[(step.into(), KEY)], 3)
            .unwrap();
        assert_eq!(markers(&j), vec![(step.into(), T, true)]);
        assert_eq!(calls(&j), vec![(step.into(), KEY)]);
        // A bundle over a standing marker adds nothing.
        j.resolve_sent(&W, &T, &[step.into()], &[(step.into(), KEY)], 4)
            .unwrap();
        assert_eq!(j.funding_rows().unwrap().len(), 1);
        assert_eq!(j.step_log().unwrap().len(), 2);
        // Final: a later NotSent revokes neither.
        j.resolve(&W, &T, Resolution::NotSent, 5).unwrap();
        assert_eq!(calls(&j), vec![(step.into(), KEY)]);
    }

    /// A self-spend is recorded only against a standing marker of its
    /// step, and stands only while that marker does.
    #[test]
    fn a_self_spend_answers_the_marker_standing_when_written() {
        let j = journal();
        let step = "registration/d/funding";
        let (tx1, tx2) = ([5; 32], [6; 32]);
        assert!(!j.insert_self_spend(&W, step, &T, &tx1, 1).unwrap());
        assert!(j.self_spend_rows().unwrap().is_empty(), "no marker");
        j.insert_funding_step(&W, step, &T, &KEY, 2).unwrap();
        assert!(j.insert_self_spend(&W, step, &T, &tx1, 3).unwrap());
        assert!(j.insert_self_spend(&W, step, &T, &tx1, 4).unwrap());
        assert!(j.insert_self_spend(&W, step, &T, &tx2, 4).unwrap());
        let spends = |j: &DispatchJournal| -> Vec<[u8; 32]> {
            j.load_calls()
                .unwrap()
                .self_spends
                .into_iter()
                .map(|s| s.txid)
                .collect()
        };
        assert_eq!(spends(&j), vec![tx1, tx2], "idempotent per txid");
        // The spend ChainLocked: NotSent supersedes the marker, and the
        // self-spends with it. A new call's marker does not inherit them.
        j.resolve(&W, &T, Resolution::NotSent, 5).unwrap();
        assert!(spends(&j).is_empty());
        assert!(!j.insert_self_spend(&W, step, &T, &tx1, 6).unwrap());
        j.insert_funding_step(&W, step, &T, &KEY, 7).unwrap();
        assert!(spends(&j).is_empty());
        assert_eq!(j.self_spend_rows().unwrap().len(), 2, "append-only");
        // The compaction takes the side rows exactly with the artifact's.
        j.resolve(&W, &T, Resolution::NotSent, 8).unwrap();
        assert_eq!(j.erase_wallet(&W, false).unwrap(), vec![T]);
        assert!(j.self_spend_rows().unwrap().is_empty());
        assert!(j.funding_rows().unwrap().is_empty());
    }

    /// The sweep never takes a standing funding marker or its self-spend.
    #[test]
    fn the_sweep_keeps_a_standing_funding_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        let open = |now| match DispatchJournal::open(&path, now).unwrap() {
            JournalOpen::Ready(j) => j,
            JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
        };
        let j = open(1);
        let (kept, gone) = ([7; 32], [8; 32]);
        for a in [kept, gone] {
            j.insert_funding_step(&W, "s/funding", &a, &KEY, 1).unwrap();
            assert!(
                j.insert_self_spend(&W, "s/funding", &a, &[1; 32], 1)
                    .unwrap()
            );
        }
        j.resolve(&W, &gone, Resolution::NotSent, 2).unwrap();
        drop(j);
        let j = open(2);
        let rows = j.load_calls().unwrap();
        assert_eq!(rows.funding.len(), 1);
        assert_eq!(rows.funding[0].artifact, kept);
        assert_eq!(rows.self_spends.len(), 1);
        assert!(j.funding_rows().unwrap().iter().all(|f| f.artifact == kept));
        assert!(
            j.self_spend_rows()
                .unwrap()
                .iter()
                .all(|f| f.artifact == kept)
        );
    }

    #[test]
    fn seeding_writes_prefence_and_the_mark_once() {
        let j = journal();
        j.register(&W, &T, &L1, &P, b"p", 1).unwrap();
        j.mark_dispatching(&W, &T, 1).unwrap();
        assert!(!j.is_seeded(&W).unwrap());
        j.seed_wallet(&W, &[(T, b"x".to_vec()), ([9; 32], b"y".to_vec())], 2)
            .unwrap();
        assert!(j.is_seeded(&W).unwrap());
        let (rows, _) = j.load().unwrap();
        let state = |t: [u8; 32]| rows.iter().find(|r| r.txid == t).unwrap().state;
        assert_eq!(state(T), DiskState::Dispatching, "an existing row is kept");
        assert_eq!(state([9; 32]), DiskState::PreFence);
    }

    #[test]
    fn erase_removes_one_wallet_only() {
        let j = journal();
        let other = [7; 32];
        for w in [W, other] {
            j.register(&w, &T, &L1, &P, b"p", 1).unwrap();
            j.insert_step(&w, "s", &T, 1).unwrap();
            j.seed_wallet(&w, &[], 1).unwrap();
        }
        j.resolve(&W, &T, Resolution::NotSent, 2).unwrap();
        assert_eq!(j.erase_wallet(&W, true).unwrap(), vec![T]);
        let (rows, steps) = j.load().unwrap();
        assert!(rows.iter().all(|r| r.wallet == other) && rows.len() == 1);
        assert!(steps.iter().all(|s| s.wallet == other) && steps.len() == 1);
        assert!(!j.is_seeded(&W).unwrap());
        assert!(j.is_seeded(&other).unwrap());
    }

    /// DEC-160: a removal keeps every artifact with a standing marker or a
    /// final row, whole, and erases only the rest.
    #[test]
    fn erase_keeps_the_evidence_of_anything_possibly_sent() {
        let j = journal();
        let (out, settled, seen, maybe, kept) = ([1; 32], [2; 32], [3; 32], [4; 32], [5; 32]);
        for a in [out, settled, seen, maybe] {
            j.insert_step(&W, "s", &a, 1).unwrap();
        }
        for a in [settled, seen] {
            j.resolve(&W, &a, Resolution::NotSent, 2).unwrap();
        }
        j.resolve(&W, &seen, Resolution::Sent, 3).unwrap();
        j.resolve(&W, &maybe, Resolution::MaybeSent, 3).unwrap();
        j.register(&W, &kept, &L1, &P, b"p", 1).unwrap();
        assert_eq!(j.erase_wallet(&W, false).unwrap(), vec![settled]);
        let log = j.step_log().unwrap();
        assert!(log.iter().all(|r| r.artifact != settled));
        assert_eq!(log.len(), 6, "out, seen and maybe keep every row");
        let stands: Vec<[u8; 32]> = markers(&j).into_iter().map(|(_, a, _)| a).collect();
        assert_eq!(stands, vec![out, seen, maybe]);
        assert_eq!(j.load().unwrap().0.len(), 1, "no dispatch erase asked");
        assert!(j.erase_wallet(&W, false).unwrap().is_empty(), "idempotent");
    }

    /// The SQL predicate (`COMPACTABLE`, used by the sweep and the removal)
    /// and the Rust one (`compactable`, over `standing`) agree on random
    /// logs, including orders the write API would not produce; and so do
    /// their deletes of the Mode B rows (`compact` and
    /// `compact_in_memory`), orphans included.
    #[test]
    fn the_sql_and_rust_compaction_predicates_agree() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..300 {
            let j = journal();
            let rows = next(12);
            for _ in 0..rows {
                let (w, a) = ([next(2) as u8; 32], [next(3) as u8; 32]);
                let kind = next(4) as i64;
                let step = (kind == 0).then(|| format!("s{}", next(2)));
                j.conn()
                    .execute(
                        "INSERT INTO step_log (wallet, artifact, kind, step_id, at) VALUES (?1, ?2, ?3, ?4, 0)",
                        params![&w[..], &a[..], kind, step],
                    )
                    .unwrap();
            }
            let log = j.step_log().unwrap();
            // Side rows: linked to a marker, or orphans of any pair.
            for r in &log {
                if let StepEvent::Marked(step) = &r.event
                    && next(2) == 0
                {
                    j.conn()
                        .execute(
                            "INSERT INTO funding_marker (seq, wallet, step_id, artifact, funding_type, identity_index, account, at)
                             VALUES (?1, ?2, ?3, ?4, 0, 0, 0, 0)",
                            params![r.seq as i64, &r.wallet[..], step, &r.artifact[..]],
                        )
                        .unwrap();
                }
            }
            for n in 0..next(4) {
                let (w, a) = ([next(2) as u8; 32], [next(3) as u8; 32]);
                j.conn()
                    .execute(
                        "INSERT INTO funding_marker (seq, wallet, step_id, artifact, funding_type, identity_index, account, at)
                         VALUES (?1, ?2, 's0', ?3, 0, 0, 0, 0)",
                        params![1000 + n as i64, &w[..], &a[..]],
                    )
                    .unwrap();
                j.conn()
                    .execute(
                        "INSERT INTO self_spend (wallet, step_id, artifact, txid, marker, at)
                         VALUES (?1, 's0', ?2, ?3, ?4, 0)",
                        params![&w[..], &a[..], &[n as u8; 32][..], next(13) as i64],
                    )
                    .unwrap();
            }
            let (mut funding, mut spends) =
                (j.funding_rows().unwrap(), j.self_spend_rows().unwrap());
            let mut mem_log = log.clone();
            let mut want: Vec<_> = compactable(&log).into_iter().collect();
            want.sort();
            let mem = compact_in_memory(&mut mem_log, &mut funding, &mut spends, None);
            assert_eq!(mem.into_iter().collect::<Vec<_>>(), want);
            let mut got = compact(&j.conn(), None).unwrap();
            got.sort();
            assert_eq!(got, want, "{log:?}");
            let left = j.step_log().unwrap();
            assert_eq!(left, mem_log);
            assert_eq!(j.funding_rows().unwrap(), funding, "{log:?}");
            assert_eq!(j.self_spend_rows().unwrap(), spends, "{log:?}");
            assert_eq!(
                standing(&left),
                standing(&log),
                "compaction keeps the reading"
            );
            assert!(compactable(&left).is_empty());
        }
    }

    #[test]
    fn pragmas_are_full_and_secure_delete_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 1).unwrap() else {
            panic!("newer schema");
        };
        let p = j.pragmas().unwrap();
        assert_eq!(p.journal_mode, "wal");
        assert_eq!(p.synchronous, 2, "FULL");
        assert_eq!(p.secure_delete, 1);
        if cfg!(target_vendor = "apple") {
            assert_eq!(p.fullfsync, 1);
        }
        // Reopening keeps the rows.
        j.register(&W, &T, &L1, &P, b"p", 1).unwrap();
        drop(j);
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 2).unwrap() else {
            panic!("newer schema");
        };
        assert_eq!(j.load().unwrap().0.len(), 1);
    }

    #[test]
    fn a_newer_schema_is_reported_not_failed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(DISPATCH_DB_FILE);
        drop(DispatchJournal::open(&path, 1).unwrap());
        Connection::open(&path)
            .unwrap()
            .execute("UPDATE meta SET v = 99 WHERE k = 'schema'", [])
            .unwrap();
        match DispatchJournal::open(&path, 2).unwrap() {
            JournalOpen::NewerSchema(v) => assert_eq!(v, 99),
            JournalOpen::Ready(_) => panic!("opened a newer schema"),
        }
    }
}
