//! The shielded holds a network directory's stores keep on disk, read
//! without the library and without writing: what "Restore earlier data"
//! would replace (R-1), and what a removal with no shielded store open
//! cannot ask the library about (#925).
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
//! open after a restore drains them again. For a restore, neither does a
//! fork disposition still in the live store, nor a marker, when the verified
//! snapshot's `shielded.sqlite` holds a disposition of the same wallet,
//! activity and nullifiers (the activity the open's import gives a fork
//! hold's marker): the restore brings it back and its first open holds it
//! again. With no such disposition, or a snapshot that does not read, it
//! holds.
//!
//! Anything that does not read holds: a store or journal SQLite can't open
//! or query, a row it can't decode, a journal newer than this build.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

use dw_appdb::dispatch::{DISPATCH_DB_FILE, DISPATCH_SCHEMA, ForkHold};
use dw_store_upgrade::SnapshotState;
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
    let mut carried = BTreeSet::new();
    if let Some(db) = open_read_only(&network_dir.join(SHIELDED_DB_FILE))? {
        let (forks, unreadable) = shielded_pool::fork_holds_in(&db)?;
        if !unreadable.is_empty() {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
        carried.extend(forks.into_iter().map(fork_notes));
        if shielded_pool::pending_rows_in(&db, None)? {
            held = Some(ShieldedHold::InFlight);
        }
    }
    if let Some(db) = open_read_only(&network_dir.join(DISPATCH_DB_FILE))? {
        let journal = journal_holds_in(&db, None)?;
        if journal.unsettled {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
        if journal.marked {
            carried.extend(marker_notes_in(&db)?);
        }
        if journal.open {
            held = Some(ShieldedHold::InFlight);
        }
    }
    if !carried.is_empty() && !carried.is_subset(&snapshot_fork_notes(network_dir)?) {
        return Ok(Some(ShieldedHold::CarriedOver));
    }
    Ok(held)
}

/// A hold's wallet and activity, with the nullifiers it keeps.
type HeldNotes = ([u8; 32], [u8; 32], BTreeSet<[u8; 32]>);

fn fork_notes(hold: ForkHold) -> HeldNotes {
    (
        hold.wallet,
        hold.activity,
        hold.nullifiers.into_iter().collect(),
    )
}

/// Every DEC-1057 marker of the journal open on `db`, with its nullifiers.
fn marker_notes_in(db: &Connection) -> Result<BTreeSet<HeldNotes>, String> {
    let bytes32 = |v: Vec<u8>| {
        <[u8; 32]>::try_from(v.as_slice()).map_err(|_| "a marker's key is malformed".to_string())
    };
    let mut markers = BTreeMap::<_, BTreeSet<[u8; 32]>>::new();
    let mut stmt = db
        .prepare("SELECT wallet, activity FROM shielded_unrecorded")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (wallet, activity) = row.map_err(|e| e.to_string())?;
        markers.insert((bytes32(wallet)?, bytes32(activity)?), BTreeSet::new());
    }
    if has_table(db, "shielded_unrecorded_nullifier")? {
        let mut stmt = db
            .prepare("SELECT wallet, activity, nullifier FROM shielded_unrecorded_nullifier")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let (wallet, activity, nullifier) = row.map_err(|e| e.to_string())?;
            let key = (bytes32(wallet)?, bytes32(activity)?);
            if let Some(notes) = markers.get_mut(&key) {
                notes.insert(bytes32(nullifier)?);
            }
        }
    }
    Ok(markers
        .into_iter()
        .map(|((wallet, activity), notes)| (wallet, activity, notes))
        .collect())
}

/// The fork dispositions that hold notes in the verified snapshot's
/// `shielded.sqlite`, the before set a restore puts back; none when the
/// snapshot has no shielded store. No verified snapshot, or a store in it
/// that does not read, is an error.
fn snapshot_fork_notes(network_dir: &Path) -> Result<BTreeSet<HeldNotes>, String> {
    let info = match dw_store_upgrade::snapshot_state(network_dir).map_err(|e| e.to_string())? {
        SnapshotState::Verified(info) => info,
        _ => return Err("no verified snapshot to restore".into()),
    };
    let Some(db) = open_read_only(&info.dir().join(SHIELDED_DB_FILE))? else {
        return Ok(BTreeSet::new());
    };
    let (holds, _) = shielded_pool::fork_holds_in(&db)?;
    Ok(holds.into_iter().map(fork_notes).collect())
}

/// What the journal at `network_dir` holds of `wallet`'s shielded spends,
/// read only; nothing with no journal.
pub(crate) fn journal_holds(network_dir: &Path, wallet: WalletId) -> Result<JournalHolds, String> {
    match open_read_only(&network_dir.join(DISPATCH_DB_FILE))? {
        Some(db) => journal_holds_in(&db, Some(wallet)),
        None => Ok(JournalHolds::default()),
    }
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
