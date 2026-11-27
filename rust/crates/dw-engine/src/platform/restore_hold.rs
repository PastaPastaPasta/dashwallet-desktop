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
//! An unresolved create the journal marks consumed, or whose registration's
//! proof in `app.sqlite` reads consumed (its notes spent with no identity,
//! nothing owed), does not hold either, as for
//! [`shielded_pool::holds_unresolved`]; with neither, or a mark or proof
//! that does not read, it holds.
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

use dw_appdb::APP_DB_FILE;
use dw_appdb::dispatch::{
    DISPATCH_DB_FILE, DISPATCH_SCHEMA, DiskState, ForkHold, unresolved_signed_locks_in,
};
use dw_store_upgrade::{SnapshotInfo, SnapshotState};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

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
pub(crate) fn restore_hold(
    network_dir: &Path,
    snapshot: Option<&SnapshotInfo>,
) -> Option<ShieldedHold> {
    match read_restore_hold(network_dir, snapshot) {
        Ok(hold) => hold,
        Err(e) => {
            tracing::warn!(folder = %network_dir.display(), error = %e,
                "the shielded holds on disk could not be read; the restore is refused");
            Some(ShieldedHold::CarriedOver)
        }
    }
}

pub(crate) fn check_core_dispatch(network_dir: &Path) -> Result<(), String> {
    let Some(db) = open_read_only(&network_dir.join(DISPATCH_DB_FILE))? else {
        return Ok(());
    };
    let unresolved = || "An asset lock may have been sent and is still unresolved.".to_string();
    if unresolved_signed_locks_in(&db).map_err(|e| e.to_string())? {
        return Err(unresolved());
    }
    if !has_table(&db, "dispatch")? {
        return Ok(());
    }
    let mut stmt = db
        .prepare("SELECT DISTINCT state FROM dispatch")
        .map_err(|e| e.to_string())?;
    let states = stmt
        .query_map([], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    for state in states {
        let state =
            DiskState::from_db(state.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if super::lease::fence::Reg::from_disk(state, None).possibly_sent() {
            return Err(unresolved());
        }
    }
    Ok(())
}

fn read_restore_hold(
    network_dir: &Path,
    snapshot: Option<&SnapshotInfo>,
) -> Result<Option<ShieldedHold>, String> {
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
    let mut snapshot = snapshot.cloned();
    if let Some(db) = open_read_only(&network_dir.join(DISPATCH_DB_FILE))? {
        let mut journal = journal_holds_in(&db, network_dir, None, None)?;
        if snapshot.is_none() && (journal.unsettled || journal.open || journal.marked) {
            snapshot = verified_snapshot(network_dir).ok();
        }
        if let Some(before) = &snapshot
            && let Some(backup) = open_read_only(&before.dir().join(DISPATCH_DB_FILE))?
            && has_table(&backup, "meta")?
            && backup
                .query_row("SELECT v FROM meta WHERE k = 'schema'", [], |row| {
                    row.get::<_, i64>(0)
                })
                .map_err(|e| e.to_string())?
                < 11
        {
            journal = journal_holds_in(&db, network_dir, None, Some(&backup))?;
            if journal.marked {
                let backed = if has_table(&backup, "shielded_unrecorded")? {
                    marker_notes_in(&backup)?
                } else {
                    BTreeSet::new()
                };
                carried.extend(
                    marker_notes_in(&db)?
                        .into_iter()
                        .filter(|notes| !covers_notes(&backed, notes)),
                );
            }
        } else if journal.marked {
            carried.extend(marker_notes_in(&db)?);
        }
        if journal.unsettled {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
        if journal.open {
            held = Some(ShieldedHold::InFlight);
        }
    }
    if !carried.is_empty() {
        let snapshot = match snapshot {
            Some(snapshot) => snapshot,
            None => verified_snapshot(network_dir)?,
        };
        let backed = snapshot_fork_notes(snapshot.dir())?;
        if carried.iter().any(|notes| !covers_notes(&backed, notes)) {
            return Ok(Some(ShieldedHold::CarriedOver));
        }
    }
    Ok(held)
}

/// A hold's wallet and activity, with the nullifiers it keeps.
type HeldNotes = ([u8; 32], [u8; 32], BTreeSet<[u8; 32]>);

fn covers_notes(backed: &BTreeSet<HeldNotes>, (wallet, activity, notes): &HeldNotes) -> bool {
    !notes.is_empty()
        && backed
            .iter()
            .any(|(w, a, n)| w == wallet && a == activity && notes.is_subset(n))
}

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
            .prepare("SELECT wallet, activity, nullifier FROM shielded_unrecorded_nullifier WHERE spent = 0")
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
fn verified_snapshot(network_dir: &Path) -> Result<dw_store_upgrade::SnapshotInfo, String> {
    match dw_store_upgrade::snapshot_state(network_dir).map_err(|e| e.to_string())? {
        SnapshotState::Verified(info) => Ok(info),
        _ => Err("no verified snapshot to restore".into()),
    }
}

fn snapshot_fork_notes(snapshot: &Path) -> Result<BTreeSet<HeldNotes>, String> {
    let Some(db) = open_read_only(&snapshot.join(SHIELDED_DB_FILE))? else {
        return Ok(BTreeSet::new());
    };
    let (holds, _) = shielded_pool::fork_holds_in(&db)?;
    Ok(holds.into_iter().map(fork_notes).collect())
}

/// What the journal at `network_dir` holds of `wallet`'s shielded spends,
/// read only; nothing with no journal.
pub(crate) fn journal_holds(network_dir: &Path, wallet: WalletId) -> Result<JournalHolds, String> {
    match open_read_only(&network_dir.join(DISPATCH_DB_FILE))? {
        Some(db) => journal_holds_in(&db, network_dir, Some(wallet), None),
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
/// not read. An unresolved create is open unless the journal marks it
/// consumed ([`unmarked_creates`]) or its proof in `network_dir`'s
/// `app.sqlite` reads consumed ([`creates_consumed`]); the mark outlives
/// the proof a removal stopped before its archive deleted.
struct Import<'a> {
    origin: &'a str,
    wallet: &'a [u8],
    activity: &'a [u8],
    transition: Option<&'a [u8]>,
    anchor: Option<&'a [u8]>,
}

fn reimported(db: &Connection, backup: &Connection, import: Import<'_>) -> Result<bool, String> {
    let (table, field, key) = match import.origin {
        "imported_redrive" => ("shielded_redrive", "transition", import.transition),
        "imported_marker" => ("shielded_unrecorded", "anchor", import.anchor),
        _ => return Ok(false),
    };
    if key.is_none_or(|key| key.len() != 32) || !has_table(backup, table)? {
        return Ok(false);
    }
    let present: bool = backup
        .query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM {table}
         WHERE wallet = ?1 AND activity = ?2 AND {field} = ?3)"
            ),
            rusqlite::params![import.wallet, import.activity, key],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !present {
        return Ok(false);
    }
    let notes =
        |db: &Connection, table: &str| -> Result<BTreeSet<[u8; 32]>, String> {
            if !has_table(db, table)? {
                return Ok(BTreeSet::new());
            }
            let mut stmt = db.prepare(&format!(
            "SELECT nullifier FROM {table} WHERE wallet = ?1 AND activity = ?2 AND spent = 0"
        )).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([import.wallet, import.activity], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            rows.map(|row| row.map_err(|e| e.to_string())).collect()
        };
    Ok(notes(db, "shielded_artifact_note")?
        .is_subset(&notes(backup, "shielded_unrecorded_nullifier")?))
}

fn journal_holds_in(
    db: &Connection,
    network_dir: &Path,
    wallet: Option<WalletId>,
    backup: Option<&Connection>,
) -> Result<JournalHolds, String> {
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
            "SELECT status, status_reason, st_hash, kind, wallet, origin, activity, anchor FROM shielded_artifact \
             WHERE ?1 IS NULL OR wallet = ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([&wallet], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Vec<u8>>(6)?,
                r.get::<_, Option<Vec<u8>>>(7)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut spent_creates = Vec::new();
    for row in rows {
        let (status, reason, st_hash, kind, wallet, origin, activity, anchor) =
            row.map_err(|e| e.to_string())?;
        if matches!(status.as_str(), "archived" | "executed" | "not_executed") {
            continue;
        }
        if matches!(status.as_str(), "unclassified" | "unresolved")
            && let Some(backup) = backup
            && reimported(
                db,
                backup,
                Import {
                    origin: &origin,
                    wallet: &wallet,
                    activity: &activity,
                    transition: st_hash.as_deref(),
                    anchor: anchor.as_deref(),
                },
            )?
        {
            continue;
        }
        match (status, reason, st_hash, kind, wallet) {
            (status, None, Some(st_hash), kind, wallet)
                if status == "unresolved" && kind.as_deref() == Some("identity_create") =>
            {
                spent_creates.push((wallet, st_hash));
            }
            (status, None, Some(_), ..)
                if matches!(status.as_str(), "registered" | "unresolved") =>
            {
                holds.open = true;
            }
            _ => holds.unsettled = true,
        }
    }
    if !holds.open && !spent_creates.is_empty() {
        let unmarked = unmarked_creates(db, spent_creates)?;
        if !unmarked.is_empty() && !creates_consumed(network_dir, &unmarked)? {
            holds.open = true;
        }
    }
    Ok(holds)
}

/// A create's wallet and transition hash, as the journal stores them.
type CreateKey = (Vec<u8>, Vec<u8>);

/// Those of `creates`, by wallet and transition hash, the journal open on
/// `db` does not mark consumed ([`dw_appdb::dispatch::consumed_creates_in`]).
/// A mark that does not read, or a malformed key, is an error.
fn unmarked_creates(db: &Connection, creates: Vec<CreateKey>) -> Result<Vec<CreateKey>, String> {
    let mut unmarked = Vec::new();
    for (wallet, st_hash) in creates {
        let (Ok(w), Ok(h)) = (
            <[u8; 32]>::try_from(wallet.as_slice()),
            <[u8; 32]>::try_from(st_hash.as_slice()),
        ) else {
            return Err("an artifact's wallet or transition is malformed".into());
        };
        let marked = dw_appdb::dispatch::consumed_creates_in(db, &w, &[h])
            .map_err(|e| format!("the journal's consumed creates: {e}"))?;
        if marked.is_empty() {
            unmarked.push((wallet, st_hash));
        }
    }
    Ok(unmarked)
}

/// Whether the registration proof (`dp_registration_proved`) of every
/// create in `creates`, by wallet and transition hash, reads consumed in
/// `network_dir`'s `app.sqlite`. No file, no settings table or no proof is
/// `false`; a file, table or row that does not read is an error.
fn creates_consumed(network_dir: &Path, creates: &[CreateKey]) -> Result<bool, String> {
    let Some(db) = open_read_only(&network_dir.join(APP_DB_FILE))? else {
        return Ok(false);
    };
    if !has_table(&db, "settings_kv")? {
        return Ok(false);
    }
    for (wallet, st_hash) in creates {
        let wallet = <[u8; 32]>::try_from(wallet.as_slice())
            .map_err(|_| "an artifact's wallet is malformed".to_string())?;
        let scope = dw_appdb::local_scope(&WalletId(wallet).to_string());
        let key = dw_appdb::dp_registration_proved(&hex::encode(st_hash));
        let proof: Option<String> = db
            .query_row(
                "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
                [&scope, &key],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        if proof.as_deref() != Some("consumed") {
            return Ok(false);
        }
    }
    Ok(true)
}
