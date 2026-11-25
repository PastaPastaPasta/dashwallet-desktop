//! The shielded holds a network directory's stores keep on disk, read
//! without the library and without writing: what "Restore earlier data"
//! would replace (R-1).
//!
//! The same holds Close and the removal gate refuse on
//! ([`shielded_pool::holds_unresolved`], [`shielded_pool::removal_hold`]),
//! as they stand on disk:
//! - `shielded.sqlite`: a `shielded_pending_spends` row (the library's
//!   record of a spend of unknown result, and the durable form of its
//!   reservation), and a fork disposition that holds notes (what the
//!   open's [`shielded_pool::import_fork_holds`] marks);
//! - `dispatch.sqlite`, the journal: an artifact not settled or archived,
//!   a DEC-1057 marker (`shielded_unrecorded`, the fork's imported holds
//!   among them), and, in a journal from before artifacts, a stored
//!   redrive (what the 10 → 11 step imports `unclassified`).
//!
//! A drained fork hold (`shielded_drained_hold`) does not hold here, as for
//! the removal: the fork's rows it came from stay in the before set, and the
//! open after a restore drains them again.
//!
//! Anything that does not read holds: a store or journal SQLite can't open
//! or query, a row it can't decode, a journal newer than this build.

use std::path::Path;
use std::time::Duration;

use dw_appdb::dispatch::{DISPATCH_DB_FILE, DISPATCH_SCHEMA};
use rusqlite::{Connection, OpenFlags};

use super::shielded_pool::{self, SHIELDED_DB_FILE};
use crate::{ShieldedHold, WalletId};

/// What the journal on disk holds of shielded spends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct JournalHolds {
    /// An artifact waiting for the library's outcome: registered, or
    /// unresolved and recorded, with the transition it names.
    pub(crate) open: bool,
    /// Any other artifact not settled or archived, or a redrive a journal
    /// from before artifacts keeps.
    pub(crate) unsettled: bool,
    /// A DEC-1057 marker.
    pub(crate) marked: bool,
}

/// Why a restore of `network_dir`'s earlier data is refused, if it is: a
/// spend that may still go through ([`ShieldedHold::InFlight`]: a stored
/// spend, a registered artifact), or one a confirmation alone does not
/// clear ([`ShieldedHold::CarriedOver`]: a marker, a fork hold, any other
/// open artifact, or a store or journal that does not read).
pub(crate) fn restore_hold(network_dir: &Path) -> Option<ShieldedHold> {
    match read_restore_hold(network_dir) {
        Ok(hold) => hold,
        Err(e) => {
            tracing::warn!(folder = %network_dir.display(), error = %e,
                "the shielded holds on disk could not be read; the restore is refused");
            Some(ShieldedHold::CarriedOver)
        }
    }
}

fn read_restore_hold(network_dir: &Path) -> Result<Option<ShieldedHold>, String> {
    #[cfg(test)]
    if crate::keys::take_test_failure(&shielded_pool::FAIL_DISK_READ_IN, network_dir) {
        return Err("test: the shielded store does not read".into());
    }
    let mut held = None;
    if let Some(db) = open_read_only(&network_dir.join(SHIELDED_DB_FILE))? {
        let (forks, unreadable) = shielded_pool::fork_holds_in(&db)?;
        if !forks.is_empty() || !unreadable.is_empty() {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
        if shielded_pool::pending_rows_in(&db, None)? {
            held = Some(ShieldedHold::InFlight);
        }
    }
    if let Some(db) = open_read_only(&network_dir.join(DISPATCH_DB_FILE))? {
        let journal = journal_holds_in(&db, None)?;
        if journal.marked || journal.unsettled {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
        if journal.open {
            held = Some(ShieldedHold::InFlight);
        }
    }
    Ok(held)
}

/// `path` opened read only, or `None` when there is no such file. With no
/// `-wal` or `-journal` beside it, opened `immutable=1`: it creates no side
/// file in the folder it reads.
fn open_read_only(path: &Path) -> Result<Option<Connection>, String> {
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let logged = ["-wal", "-journal"].iter().any(|suffix| {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        Path::new(&side).exists()
    });
    let db = if logged {
        Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
    } else {
        dw_store_upgrade::open_immutable(path)
    }
    .map_err(|e| e.to_string())?;
    db.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    Ok(Some(db))
}

fn has_table(db: &Connection, name: &str) -> Result<bool, String> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [name],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// What the journal open on `db` holds of `wallet`'s shielded spends, or
/// every wallet's. A file with no table at all (an open that stopped once
/// it created the journal) holds nothing, as the journal's open reads it
/// as new; one with tables but no schema, or newer than this build, does
/// not read.
fn journal_holds_in(db: &Connection, wallet: Option<WalletId>) -> Result<JournalHolds, String> {
    let tables: bool = db
        .query_row(
            "SELECT EXISTS (SELECT 1 FROM sqlite_master WHERE type = 'table')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !tables {
        return Ok(JournalHolds::default());
    }
    let schema: i64 = db
        .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |r| r.get(0))
        .map_err(|e| format!("the journal's schema: {e}"))?;
    if schema > i64::from(DISPATCH_SCHEMA) {
        return Err(format!(
            "the journal's schema {schema} is newer than this build's"
        ));
    }
    let wallet = wallet.map(|w| w.0.to_vec());
    let any = |table: &str| -> Result<bool, String> {
        if !has_table(db, table)? {
            return Ok(false);
        }
        db.query_row(
            &format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE ?1 IS NULL OR wallet = ?1)"),
            [&wallet],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())
    };
    let mut holds = JournalHolds {
        marked: any("shielded_unrecorded")?,
        ..Default::default()
    };
    if !has_table(db, "shielded_artifact")? {
        holds.unsettled = any("shielded_redrive")?;
        return Ok(holds);
    }
    let mut stmt = db
        .prepare(
            "SELECT status, status_reason, st_hash IS NOT NULL FROM shielded_artifact \
             WHERE ?1 IS NULL OR wallet = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&wallet], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        match row.map_err(|e| e.to_string())? {
            (status, _, _)
                if matches!(status.as_str(), "archived" | "executed" | "not_executed") => {}
            (status, None, true) if matches!(status.as_str(), "registered" | "unresolved") => {
                holds.open = true;
            }
            _ => holds.unsettled = true,
        }
    }
    Ok(holds)
}
