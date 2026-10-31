//! Debug builds with `fault-inject` (and this crate's tests): named one-shot
//! fault points at the durable writes of MG-13's withdrawals and payout
//! barriers (MG-13 §3 "Fault sweep"), modelled on dw-vault's `fault`.
//!
//! Each point has a [`Stage::Before`] variant, hit inside the write's
//! transaction just before its commit (for [`SUPERSEDE`], between the
//! refused record's move to `superseded` and the sibling's insert), and a
//! [`Stage::After`] variant, hit once the write committed. Armed for one
//! database file ([`arm`]), the write fails with
//! [`AppDbError::Fault`](crate::AppDbError::Fault) naming the point: before,
//! with nothing committed; after, with the write on disk while the caller
//! sees an error, as a crash right after it. The engine adds its own points
//! (network steps) with [`hit`].
//!
//! `DW_FAULT=<name>` (a full name, `mn_withdraw.after_insert`) aborts the
//! process at that point instead, whatever the database: the crash of MG-13
//! T2 (4). Release builds compile this module out.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

pub use crate::fault_points::*;

/// Every dw-appdb point.
pub const POINTS: [&str; 13] = [
    MN_WITHDRAW_INSERT,
    MN_WITHDRAW_SENT_AT,
    MN_WITHDRAW_STATE,
    MN_WITHDRAW_FINALIZE,
    MN_WITHDRAW_NOT_EXECUTED,
    MN_WITHDRAW_SUPERSEDE,
    MN_WITHDRAW_MARK,
    MN_WITHDRAW_FLOOR,
    MN_WITHDRAW_NOTE,
    MN_BARRIER_WRITE,
    MN_BARRIER_CLEAR,
    MN_BARRIER_DISCARD,
    MN_BARRIER_BACKFILL,
];

/// Where a point fails its write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// In the transaction, before its commit: nothing is written.
    Before,
    /// After the commit: the write is on disk.
    After,
}

impl Stage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Before => "before",
            Self::After => "after",
        }
    }
}

/// The point's full name, its stage before the operation:
/// `mn_withdraw.insert` before its commit is `mn_withdraw.before_insert`.
pub fn name(point: &str, stage: Stage) -> String {
    match point.split_once('.') {
        Some((area, op)) => format!("{area}.{}_{op}", stage.as_str()),
        None => format!("{}_{point}", stage.as_str()),
    }
}

static ARMED: Mutex<Vec<(PathBuf, String, Stage)>> = Mutex::new(Vec::new());

/// The database at `path`, by its canonical name, or `None` (in memory,
/// gone).
fn canonical(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// Makes the next hit of `point` at `stage` on the database at `db` fail,
/// once.
pub fn arm(db: &Path, point: &str, stage: Stage) {
    let db = canonical(db).expect("an existing database");
    ARMED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((db, point.to_owned(), stage));
}

/// Disarms every point of the database at `db`.
pub fn disarm(db: &Path) {
    let Some(db) = canonical(db) else { return };
    ARMED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|(d, ..)| *d != db);
}

/// Whether `point` at `stage` was armed for `db` (disarming it). Aborts the
/// process first when `DW_FAULT` names the point.
fn take(db: Option<&Path>, point: &str, stage: Stage) -> bool {
    let full = name(point, stage);
    if std::env::var_os("DW_FAULT").is_some_and(|v| v == full.as_str()) {
        eprintln!("DW_FAULT={full}: aborting");
        std::process::abort();
    }
    let Some(db) = db.and_then(canonical) else {
        return false;
    };
    let mut armed = ARMED.lock().unwrap_or_else(|p| p.into_inner());
    match (armed.iter()).position(|(d, p, s)| *d == db && p == point && *s == stage) {
        Some(i) => {
            armed.remove(i);
            true
        }
        None => false,
    }
}

/// A fault point on the database at `db`: `Err(AppDbError::Fault(name))`
/// when it was armed (once), and an abort when `DW_FAULT` names it.
pub fn hit(db: &Path, point: &str, stage: Stage) -> crate::Result<()> {
    hit_at(Some(db), point, stage)
}

fn hit_at(db: Option<&Path>, point: &str, stage: Stage) -> crate::Result<()> {
    if take(db, point, stage) {
        return Err(crate::AppDbError::Fault(name(point, stage)));
    }
    Ok(())
}

/// [`hit`] on the database `conn` has open (none in memory).
pub(crate) fn hit_on(conn: &Connection, point: &str, stage: Stage) -> crate::Result<()> {
    let path = conn.path().filter(|p| !p.is_empty()).map(Path::new);
    hit_at(path, point, stage)
}
