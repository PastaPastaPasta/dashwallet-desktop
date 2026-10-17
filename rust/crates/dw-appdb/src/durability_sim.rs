//! Debug builds' tests only (`durability-sim`): power loss, modelled as
//! losing every commit after the last one that was synced. An armed
//! database's durable writes ([`crate::AppDb::set_setting_durable`] and the
//! others on `durably`) save its files as its durable state, and
//! [`power_loss`] puts them back. With `skip_barriers`, a durable write
//! commits like any other, unsynced and unsaved: the test of a caller that
//! needs the barrier then sees what it loses without it (MG-03 D34).
//! Checkpoints aren't saved, so the model loses at least what a real power
//! cut can.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::Connection;

struct Armed {
    db: PathBuf,
    skip_barriers: bool,
}

static ARMED: Mutex<Vec<Armed>> = Mutex::new(Vec::new());

fn armed() -> std::sync::MutexGuard<'static, Vec<Armed>> {
    ARMED.lock().unwrap_or_else(|p| p.into_inner())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The database file and its WAL.
fn files(db: &Path) -> [PathBuf; 2] {
    [db.to_path_buf(), with_suffix(db, "-wal")]
}

/// Saves `db`'s files as its durable state.
fn save(db: &Path) {
    for file in files(db) {
        let saved = with_suffix(&file, ".durable");
        if file.exists() {
            std::fs::copy(&file, &saved).expect("save the durable state");
        } else {
            let _ = std::fs::remove_file(&saved);
        }
    }
}

/// Arms `db`, whose files as they are now (no connection open, so all of
/// it is durable) are its durable state.
pub fn arm(db: &Path, skip_barriers: bool) {
    let db = std::fs::canonicalize(db).expect("an existing database");
    save(&db);
    let mut armed = armed();
    armed.retain(|a| a.db != db);
    armed.push(Armed { db, skip_barriers });
}

/// Power loss on `db`, which no connection holds open: its files go back to
/// its last durable state, and it is disarmed.
pub fn power_loss(db: &Path) {
    let db = std::fs::canonicalize(db).expect("an existing database");
    armed().retain(|a| a.db != db);
    for file in files(&db) {
        let saved = with_suffix(&file, ".durable");
        if saved.exists() {
            std::fs::rename(&saved, &file).expect("restore the durable state");
        } else {
            let _ = std::fs::remove_file(&file);
        }
    }
    let _ = std::fs::remove_file(with_suffix(&db, "-shm"));
}

fn armed_db(conn: &Connection) -> Option<(PathBuf, bool)> {
    let db = std::fs::canonicalize(conn.path()?).ok()?;
    armed()
        .iter()
        .find(|a| a.db == db)
        .map(|a| (a.db.clone(), a.skip_barriers))
}

/// Whether a durable write on `conn` skips its barrier.
pub(crate) fn skips_barrier(conn: &Connection) -> bool {
    armed_db(conn).is_some_and(|(_, skip)| skip)
}

/// After a durable write on `conn`: its database's files are durable.
pub(crate) fn synced(conn: &Connection) {
    if let Some((db, _)) = armed_db(conn) {
        save(&db);
    }
}
