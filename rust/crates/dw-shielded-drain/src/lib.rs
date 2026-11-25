//! Drains a fork-written `shielded.sqlite` into the dispatch journal before
//! upstream's store first opens it.
//!
//! A fork build's store keeps three things upstream never reads: the
//! `shielded_spend_dispositions` table, whose executed and unresolved rows
//! hold their notes out of selection until the engine acknowledges them; the
//! `unsent` column, which proves a record was never broadcast; and
//! `armed_recording`. Without a drain every unacknowledged hold would become
//! selectable on upstream's first open, and a never-sent record a redrive
//! candidate. The drain moves the holds and the never-sent proof into the
//! journal ([`DispatchJournal::record_shielded_drain`]), then retires the
//! fork rows in the store, each step safe to repeat until the journal's
//! marker reads done.
//!
//! The session calls [`drain_fork_shielded_store`] once per network
//! directory, after this start's persister open and its journal open, and
//! before anything configures the shielded store. On `Err` the shielded
//! store stays closed.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

use dw_appdb::dispatch::{
    DispatchJournal, DrainedDisposition, DrainedOutcome, NeverSentSpend, ShieldedDrainPlan,
    ShieldedDrainState,
};
use dw_store_upgrade::{PersisterOpened, upgrade_copy_present};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

/// The shielded store's file in a network directory, and the name its drain
/// marker carries in that directory's journal.
pub const SHIELDED_STORE_FILE: &str = "shielded.sqlite";

const LIVE: &str = "shielded_spend_dispositions";
const RETIRED: &str = "dw_fork_dispositions_retired";

/// A pending record's key: `(wallet_id, account_index, activity_id)`.
type PendingKey = ([u8; 32], u32, [u8; 32]);

/// What a drain did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainOutcome {
    /// No fork rows to drain: a fresh, upstream-only or drained store.
    NotForkFormat,
    /// The fork rows went to the journal and the store retired them.
    Drained(DrainReport),
    /// A drain that stopped after the store retired its rows was marked done.
    Finished,
}

/// The counts of one drain, for its log line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DrainReport {
    pub dispositions: usize,
    pub held_notes: usize,
    pub unreadable: usize,
    pub never_sent: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ShieldedDrainError {
    /// No copy of the store from this directory's upgrade, or the
    /// persister opened was another directory's. Nothing was written.
    #[error(
        "the shielded store drain needs a copy of the store from this upgrade and its persister"
    )]
    Precondition,
    /// Another store holds `shielded.sqlite`.
    #[error("another store holds shielded.sqlite")]
    StoreInUse,
    /// A fork row's key, held list or record could not be decoded.
    #[error("corrupt fork row in shielded.sqlite: {detail}")]
    Corrupt { detail: String },
    #[error("shielded.sqlite: {0}")]
    Io(String),
    #[error("dispatch journal: {0}")]
    Journal(String),
}

type Result<T> = std::result::Result<T, ShieldedDrainError>;

/// A test seam: the drain fails right after the step it names.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailAfter {
    Read,
    Journal,
    Retire,
}

/// Drains `network_dir`'s `shielded.sqlite` into `journal`, that
/// directory's open dispatch journal.
///
/// A store with no fork rows returns [`DrainOutcome::NotForkFormat`]
/// before any check. Otherwise the drain takes the store's owner lock,
/// requires a copy of the store from this directory's upgrade and `opened`
/// for this directory, reads every fork row in one read transaction,
/// writes the journal in one transaction (marker `journaled`), retires the
/// fork rows in one store transaction, then marks the journal `done`.
pub fn drain_fork_shielded_store(
    network_dir: &Path,
    opened: &PersisterOpened,
    journal: &DispatchJournal,
) -> Result<DrainOutcome> {
    run(network_dir, opened, journal, None)
}

fn run(
    network_dir: &Path,
    opened: &PersisterOpened,
    journal: &DispatchJournal,
    fault: Option<FailAfter>,
) -> Result<DrainOutcome> {
    let path = network_dir.join(SHIELDED_STORE_FILE);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => {
            return Err(ShieldedDrainError::Io(
                "shielded.sqlite is not a regular file".into(),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DrainOutcome::NotForkFormat),
        Err(e) => return Err(io_err("inspect", e)),
    }
    let _lock = lock_store(network_dir)?;
    let marker = journal
        .shielded_drain_state(SHIELDED_STORE_FILE)
        .map_err(journal_err)?;
    let fork = read_fork_rows(&path, marker)?;
    let Some(fork) = fork else {
        return Ok(DrainOutcome::NotForkFormat);
    };
    let ForkRows::Drain {
        plan,
        never_sent_keys,
        retire_live,
    } = fork
    else {
        if !same_dir(network_dir, opened) {
            return Err(ShieldedDrainError::Precondition);
        }
        journal
            .finish_shielded_drain(SHIELDED_STORE_FILE)
            .map_err(journal_err)?;
        tracing::info!("shielded drain: finished a drain the store had retired");
        return Ok(DrainOutcome::Finished);
    };
    precondition(network_dir, opened)?;
    trip(fault, FailAfter::Read)?;
    journal
        .record_shielded_drain(SHIELDED_STORE_FILE, &plan)
        .map_err(journal_err)?;
    trip(fault, FailAfter::Journal)?;
    retire(&path, &never_sent_keys, retire_live)?;
    trip(fault, FailAfter::Retire)?;
    journal
        .finish_shielded_drain(SHIELDED_STORE_FILE)
        .map_err(journal_err)?;
    let report = report(&plan);
    tracing::info!(?report, "shielded drain: drained the fork store");
    Ok(DrainOutcome::Drained(report))
}

fn trip(fault: Option<FailAfter>, at: FailAfter) -> Result<()> {
    if fault == Some(at) {
        return Err(ShieldedDrainError::Io(format!("fault {at:?}")));
    }
    Ok(())
}

fn report(plan: &ShieldedDrainPlan) -> DrainReport {
    let holds = |d: &&DrainedDisposition| !matches!(d.outcome, DrainedOutcome::NotExecuted(_));
    DrainReport {
        dispositions: plan.dispositions.len(),
        held_notes: plan
            .dispositions
            .iter()
            .filter(holds)
            .map(|d| d.held.len())
            .sum(),
        unreadable: plan
            .dispositions
            .iter()
            .filter(|d| d.outcome == DrainedOutcome::Unreadable)
            .count(),
        never_sent: plan.never_sent.len(),
    }
}

fn io_err(what: &str, e: impl std::fmt::Display) -> ShieldedDrainError {
    ShieldedDrainError::Io(format!("{what}: {e}"))
}

fn journal_err(e: dw_appdb::AppDbError) -> ShieldedDrainError {
    ShieldedDrainError::Journal(e.to_string())
}

fn corrupt(detail: impl Into<String>) -> ShieldedDrainError {
    ShieldedDrainError::Corrupt {
        detail: detail.into(),
    }
}

/// The store's owner lock, the one fork and upstream stores take at open,
/// so no store is open on the file while the drain runs.
fn lock_store(network_dir: &Path) -> Result<File> {
    let dir = network_dir
        .canonicalize()
        .map_err(|e| io_err("resolve the network directory", e))?;
    let lock_path = dir.join(format!("{SHIELDED_STORE_FILE}.lock"));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let lock = options
        .open(&lock_path)
        .map_err(|e| io_err("open the store lock", e))?;
    match lock.try_lock() {
        Ok(()) => Ok(lock),
        Err(TryLockError::WouldBlock) => Err(ShieldedDrainError::StoreInUse),
        Err(TryLockError::Error(e)) => Err(io_err("lock the store", e)),
    }
}

fn same_dir(network_dir: &Path, opened: &PersisterOpened) -> bool {
    match (
        network_dir.canonicalize(),
        opened.network_dir().canonicalize(),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The drain writes only beside this start's persister and a copy of the
/// store from this upgrade: a verified before-upgrade snapshot, or a
/// completed part-upgraded copy that includes it. The retire keeps every
/// fork row but the proved never-sent ones, so a copy, not a restorable
/// backup, is what it needs. A marker-only finish needs the persister
/// alone: its `journaled` marker proves an earlier drain met this, and the
/// copy may since have been pruned.
fn precondition(network_dir: &Path, opened: &PersisterOpened) -> Result<()> {
    if !same_dir(network_dir, opened)
        || !upgrade_copy_present(network_dir, SHIELDED_STORE_FILE).unwrap_or(false)
    {
        return Err(ShieldedDrainError::Precondition);
    }
    Ok(())
}

/// What the store's fork rows ask of the drain.
enum ForkRows {
    /// Only the journal's marker is left to set.
    Finish,
    Drain {
        plan: ShieldedDrainPlan,
        /// `(wallet_id, account_index, activity_id)` of each never-sent row.
        never_sent_keys: Vec<PendingKey>,
        /// Whether a live dispositions table is retired.
        retire_live: bool,
    },
}

/// Reads every fork row in one read transaction, or `None` when there is
/// nothing to drain.
fn read_fork_rows(path: &Path, marker: Option<ShieldedDrainState>) -> Result<Option<ForkRows>> {
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| io_err("open", e))?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|e| io_err("begin the read", e))?;
    let live = table_exists(&tx, LIVE)?;
    let retired = table_exists(&tx, RETIRED)?;
    let never_sent = read_never_sent(&tx)?;
    let any_unsent = !never_sent.is_empty();
    let (read_live, read_retired) = match (live, retired, any_unsent, marker) {
        (false, false, false, _) => return Ok(None),
        (false, false, true, _) => (false, false),
        (true, false, _, _) => (true, false),
        (false, true, _, None) => (false, true),
        (false, true, false, Some(ShieldedDrainState::Journaled)) => {
            return Ok(Some(ForkRows::Finish));
        }
        (false, true, false, Some(ShieldedDrainState::Done)) => return Ok(None),
        (false, true, true, Some(_)) => (false, false),
        (true, true, _, _) => (true, true),
    };
    let mut by_key: BTreeMap<([u8; 32], u32, [u8; 32]), DrainedDisposition> = BTreeMap::new();
    for (table, read) in [(RETIRED, read_retired), (LIVE, read_live)] {
        if read {
            for d in read_dispositions(&tx, table)? {
                let key = (d.wallet, d.account, d.st_hash);
                let d = match by_key.remove(&key) {
                    Some(retired) => shadow(retired, d),
                    None => d,
                };
                by_key.insert(key, d);
            }
        }
    }
    let never_sent_keys = never_sent.iter().map(|(key, _)| *key).collect();
    let plan = ShieldedDrainPlan {
        dispositions: by_key.into_values().collect(),
        never_sent: never_sent.into_iter().map(|(_, spend)| spend).collect(),
    };
    Ok(Some(ForkRows::Drain {
        plan,
        never_sent_keys,
        retire_live: live,
    }))
}

fn table_exists(tx: &Transaction<'_>, name: &str) -> Result<bool> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![name],
        |r| r.get(0),
    )
    .map_err(|e| io_err("read the schema", e))
}

fn column_exists(tx: &Transaction<'_>, table: &str, column: &str) -> Result<bool> {
    tx.query_row(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        params![table, column],
        |r| r.get(0),
    )
    .map_err(|e| io_err("read the schema", e))
}

fn blob32(row: &Row<'_>, i: usize, what: &str) -> Result<[u8; 32]> {
    match row.get_ref(i).map_err(|e| io_err("read", e))? {
        ValueRef::Blob(b) => b
            .try_into()
            .map_err(|_| corrupt(format!("{what} is {} bytes", b.len()))),
        other => Err(corrupt(format!("{what} is a {:?}", other.data_type()))),
    }
}

fn account(row: &Row<'_>, i: usize) -> Result<u32> {
    match row.get_ref(i).map_err(|e| io_err("read", e))? {
        ValueRef::Integer(n) => {
            u32::try_from(n).map_err(|_| corrupt(format!("account_index {n} is out of range")))
        }
        other => Err(corrupt(format!(
            "account_index is a {:?}",
            other.data_type()
        ))),
    }
}

fn text(row: &Row<'_>, i: usize, what: &str) -> Result<Option<String>> {
    match row.get_ref(i).map_err(|e| io_err("read", e))? {
        ValueRef::Null => Ok(None),
        ValueRef::Text(t) => String::from_utf8(t.to_vec())
            .map(Some)
            .map_err(|_| corrupt(format!("{what} is not UTF-8"))),
        other => Err(corrupt(format!("{what} is a {:?}", other.data_type()))),
    }
}

/// The note-spend records durably never broadcast, with their keys.
fn read_never_sent(tx: &Transaction<'_>) -> Result<Vec<(PendingKey, NeverSentSpend)>> {
    let table = "shielded_pending_spends";
    if !table_exists(tx, table)? || !column_exists(tx, table, "unsent")? {
        return Ok(Vec::new());
    }
    let mut stmt = tx
        .prepare(
            "SELECT wallet_id, account_index, activity_id, st_bytes FROM shielded_pending_spends \
             WHERE unsent = 1 AND length(nullifiers) > 0",
        )
        .map_err(|e| io_err("read never-sent records", e))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| io_err("read never-sent records", e))?;
    let mut out = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| io_err("read never-sent records", e))?
    {
        let wallet = blob32(row, 0, "a never-sent record's wallet_id")?;
        let account = account(row, 1)?;
        let activity = blob32(row, 2, "a never-sent record's activity_id")?;
        let st_hash = match row.get_ref(3).map_err(|e| io_err("read", e))? {
            ValueRef::Blob(b) => Sha256::digest(b).into(),
            other => {
                return Err(corrupt(format!(
                    "a never-sent record's st_bytes is a {:?}",
                    other.data_type()
                )));
            }
        };
        out.push((
            (wallet, account, activity),
            NeverSentSpend {
                wallet,
                activity,
                st_hash,
            },
        ));
    }
    Ok(out)
}

/// Every disposition in `table`, decoded by the fork's own rules.
fn read_dispositions(tx: &Transaction<'_>, table: &str) -> Result<Vec<DrainedDisposition>> {
    let mut stmt = tx
        .prepare(&format!(
            "SELECT wallet_id, account_index, st_hash, disposition, reason, nullifiers FROM {table}"
        ))
        .map_err(|e| io_err("read dispositions", e))?;
    let mut rows = stmt.query([]).map_err(|e| io_err("read dispositions", e))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| io_err("read dispositions", e))? {
        let wallet = blob32(row, 0, "a disposition's wallet_id")?;
        let account = account(row, 1)?;
        let st_hash = blob32(row, 2, "a disposition's st_hash")?;
        let kind = text(row, 3, "a disposition's kind")?
            .ok_or_else(|| corrupt("a disposition's kind is NULL"))?;
        let reason = text(row, 4, "a disposition's reason")?;
        let held = match row.get_ref(5).map_err(|e| io_err("read", e))? {
            ValueRef::Blob(b) if b.len() % 32 == 0 => b.as_chunks::<32>().0.to_vec(),
            ValueRef::Blob(b) => {
                return Err(corrupt(format!(
                    "a disposition's held list is {} bytes",
                    b.len()
                )));
            }
            other => {
                return Err(corrupt(format!(
                    "a disposition's held list is a {:?}",
                    other.data_type()
                )));
            }
        };
        let outcome = decode(&kind, reason.as_deref(), held.is_empty());
        out.push(DrainedDisposition {
            wallet,
            account,
            st_hash,
            kind,
            reason,
            held,
            outcome,
        });
    }
    Ok(out)
}

/// A live row over a retired row of the same key: the live row's evidence
/// wins, and the notes either held stay held. A `not_executed` live row
/// over a retired one that held notes reads unreadable, so they do.
fn shadow(retired: DrainedDisposition, mut live: DrainedDisposition) -> DrainedDisposition {
    for nullifier in retired.held {
        if !live.held.contains(&nullifier) {
            live.held.push(nullifier);
        }
    }
    if matches!(live.outcome, DrainedOutcome::NotExecuted(_)) && !live.held.is_empty() {
        live.outcome = DrainedOutcome::Unreadable;
    }
    live
}

/// The fork's `from_stored`, plus its schema's rule that a `not_executed`
/// row holds nothing: one that holds notes anyway is unreadable, so its
/// notes stay held.
fn decode(kind: &str, reason: Option<&str>, held_empty: bool) -> DrainedOutcome {
    match (kind, reason) {
        ("executed", None) => DrainedOutcome::Executed,
        ("not_executed", Some(r @ ("refused" | "abandoned" | "rejected" | "pruned")))
            if held_empty =>
        {
            DrainedOutcome::NotExecuted(r.to_owned())
        }
        (
            "unresolved",
            Some(r @ ("deserialize_dropped" | "purged" | "unattributable" | "cancelled")),
        ) => DrainedOutcome::Unresolved(r.to_owned()),
        _ => DrainedOutcome::Unreadable,
    }
}

/// The commit point: deletes the never-sent rows and retires the live
/// dispositions table, in one transaction with `synchronous=FULL`.
fn retire(path: &Path, never_sent_keys: &[PendingKey], retire_live: bool) -> Result<()> {
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| io_err("open", e))?;
    conn.pragma_update(None, "synchronous", "FULL")
        .map_err(|e| io_err("set synchronous", e))?;
    if cfg!(target_vendor = "apple") {
        conn.pragma_update(None, "fullfsync", "ON")
            .map_err(|e| io_err("set fullfsync", e))?;
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| io_err("begin the retire", e))?;
    for (wallet, account, activity) in never_sent_keys {
        tx.execute(
            "DELETE FROM shielded_pending_spends WHERE wallet_id = ?1 AND account_index = ?2 \
             AND activity_id = ?3 AND unsent = 1",
            params![&wallet[..], account, &activity[..]],
        )
        .map_err(|e| io_err("delete a never-sent record", e))?;
    }
    if retire_live {
        if table_exists(&tx, RETIRED)? {
            let columns = shared_columns(&tx)?;
            tx.execute_batch(&format!(
                "INSERT OR REPLACE INTO {RETIRED} ({columns}) SELECT {columns} FROM {LIVE};
                 DROP TABLE {LIVE};"
            ))
            .map_err(|e| io_err("retire the dispositions into the retired table", e))?;
        } else {
            tx.execute_batch(&format!("ALTER TABLE {LIVE} RENAME TO {RETIRED};"))
                .map_err(|e| io_err("retire the dispositions", e))?;
        }
    }
    tx.commit().map_err(|e| io_err("commit the retire", e))
}

/// The live table's columns the retired one also has, quoted, in the live
/// table's order: a table retired by an older fork build may lack one.
fn shared_columns(tx: &Transaction<'_>) -> Result<String> {
    let mut stmt = tx
        .prepare(
            "SELECT l.name FROM pragma_table_info(?1) AS l \
             JOIN pragma_table_info(?2) AS r ON r.name = l.name ORDER BY l.cid",
        )
        .map_err(|e| io_err("read the schema", e))?;
    let names = stmt
        .query_map(params![LIVE, RETIRED], |r| r.get::<_, String>(0))
        .map_err(|e| io_err("read the schema", e))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| io_err("read the schema", e))?;
    Ok(names
        .iter()
        .map(|n| format!("\"{}\"", n.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", "))
}
