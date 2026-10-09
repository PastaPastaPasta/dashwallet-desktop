//! The dispatch journal: `dispatch.sqlite`, one per network directory
//! (E0-04 design §6). Only the engine's dispatch fence writes it.
//!
//! - `dispatch` holds registered artifacts (asset locks, Mode A) with their
//!   recovery payload. On disk a row moves 0 (`Unsent`) → 1 (`Dispatching`)
//!   only; 2 (`PreFence`) is written only by the seeding. No statement sets
//!   `state` to 0, and the only delete is [`DispatchJournal::erase_wallet`].
//! - `step` holds the write-ahead markers of resumable row-less steps (§7.6),
//!   written once as "possibly dispatched" and never changed.
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

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{AppDbError, Result};

/// File name of the journal inside a network directory.
pub const DISPATCH_DB_FILE: &str = "dispatch.sqlite";

/// The schema this build reads and writes.
pub const DISPATCH_SCHEMA: u32 = 1;

const SCHEMA_V1: &str = "
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
CREATE TABLE step (
  wallet    BLOB NOT NULL CHECK (length(wallet) = 32),
  step_id   TEXT NOT NULL,
  artifact  BLOB NOT NULL CHECK (length(artifact) = 32),
  state     INTEGER NOT NULL CHECK (state = 1),
  at        INTEGER NOT NULL,
  PRIMARY KEY (wallet, step_id, artifact)
) WITHOUT ROWID;
";

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

/// One `step` marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub wallet: [u8; 32],
    pub step_id: String,
    pub artifact: [u8; 32],
    pub at: u64,
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
            tx.execute_batch(SCHEMA_V1)?;
            tx.execute(
                "INSERT INTO meta (k, v) VALUES ('schema', ?1), ('created_at', ?2)",
                params![i64::from(DISPATCH_SCHEMA), now as i64],
            )?;
        }
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

    /// A resumable step's write-ahead marker (§7.6). Idempotent.
    pub fn insert_step(
        &self,
        wallet: &[u8; 32],
        step_id: &str,
        artifact: &[u8; 32],
        now: u64,
    ) -> Result<()> {
        self.conn().execute(
            "INSERT INTO step (wallet, step_id, artifact, state, at) VALUES (?1, ?2, ?3, 1, ?4)
             ON CONFLICT DO NOTHING",
            params![&wallet[..], step_id, &artifact[..], now as i64],
        )?;
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
        let mut stmt = conn.prepare("SELECT wallet, step_id, artifact, at FROM step")?;
        let steps = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?
            .map(|row| {
                let (wallet, step_id, artifact, at) = row?;
                Ok(StepRow {
                    wallet: fixed(wallet, "step.wallet")?,
                    step_id,
                    artifact: fixed(artifact, "step.artifact")?,
                    at: at as u64,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((rows, steps))
    }

    /// Erases `wallet`'s rows, markers and `seeded:` mark with
    /// `secure_delete` on, then truncates the WAL (§6.5). The caller checks
    /// that the wallet tracks no row any more.
    pub fn erase_wallet(&self, wallet: &[u8; 32]) -> Result<()> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM dispatch WHERE wallet = ?1",
            params![&wallet[..]],
        )?;
        tx.execute("DELETE FROM step WHERE wallet = ?1", params![&wallet[..]])?;
        tx.execute("DELETE FROM meta WHERE k = ?1", params![seeded_key(wallet)])?;
        tx.commit()?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
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
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
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
        j.erase_wallet(&W).unwrap();
        let (rows, steps) = j.load().unwrap();
        assert!(rows.iter().all(|r| r.wallet == other) && rows.len() == 1);
        assert!(steps.iter().all(|s| s.wallet == other) && steps.len() == 1);
        assert!(!j.is_seeded(&W).unwrap());
        assert!(j.is_seeded(&other).unwrap());
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
