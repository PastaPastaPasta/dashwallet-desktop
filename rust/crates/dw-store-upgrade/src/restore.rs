//! Restore on restart (R3-RESTORE-DESIGN-v2.md rev 2.1 with the Q92 v2.1
//! ruling). The user's request is a marker; the next start, before any store
//! opens, copies the live set into `after-upgrade-<UTC>/` and verifies it
//! (D1), stages the before set, writes the money-gate mark and the journal,
//! then renames each staged store over its live file, wallet.sqlite last
//! (D2). A journal makes every stop point resumable. Before the journal, a
//! failure removes only the dirs this attempt made.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dashcore::{OutPoint, Txid};

use crate::error::{Result, StoreUpgradeError};
use crate::fsx::{self, fault};
use crate::snapshot::{self, SIDE_SUFFIXES, STORE_FILES, SnapshotInfo, UPGRADE_BACKUPS_DIR};
use crate::sql::{open_existing, open_immutable, sha256_file};
use crate::token::PersisterOpened;

/// The user's request, written by [`request_restore`].
pub const RESTORE_REQUESTED: &str = "RESTORE-REQUESTED";
/// The journal: present from just before the first replace until the
/// restore is complete.
pub const RESTORE_JOURNAL: &str = "RESTORE-IN-PROGRESS";
/// The money gate's mark (M3-UX), with the pending payments to hold (Q89 §3
/// option 2). Set before the first replace; cleared by the engine.
pub const RESTORED_MARK: &str = "RESTORED";
/// The dirs an attempt creates before its journal (design §3).
pub const RESTORE_ATTEMPT: &str = "RESTORE-ATTEMPT";
/// Written into each dir of an attempt whose cleanup failed.
pub const SAFE_TO_DELETE: &str = "INCOMPLETE-SAFE-TO-DELETE.txt";
/// What a resume kept, and why.
pub const KEPT: &str = "KEPT.txt";
pub(crate) const AFTER_PREFIX: &str = "after-upgrade";
pub(crate) const DURING_PREFIX: &str = "during-restore";
pub(crate) const STAGING_PREFIX: &str = "restore-staging";
const READ_PREFIX: &str = ".restore-read-";
const REQUEST_HEADER: &str = "dw-store-upgrade restore-request";
const JOURNAL_HEADER: &str = "dw-store-upgrade restore-journal";
const ATTEMPT_HEADER: &str = "dw-store-upgrade restore-attempt";
const MARK_HEADER: &str = "dw-store-upgrade restored";
const FORMAT: u32 = 1;

/// D2: the replace order; wallet.sqlite is always last.
const REPLACE_ORDER: [&str; 4] = [
    "app.sqlite",
    "dispatch.sqlite",
    "shielded.sqlite",
    "wallet.sqlite",
];

/// Lock files of stores that may be open in this or another process.
const STORE_LOCKS: [&str; 2] = ["app.sqlite.lock", "shielded.sqlite.lock"];

/// What a resume does at S4 (the wallet already replaced) when another build
/// has written since and the live wallet is in the before lineage (Q92 S4-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum S4Policy {
    /// Keep whatever is live, side files included, and finish.
    KeepLive,
    /// Stop with `RestoreIncomplete{S4OldBuildWrote}`; nothing moves.
    Refuse,
}

/// The shipped S4 behaviour (Q92 92-1). Side files are never moved at S4.
pub const S4_POLICY: S4Policy = S4Policy::KeepLive;

fn s4_policy() -> S4Policy {
    #[cfg(test)]
    if let Some(p) = S4_FOR_TEST.with(|p| p.get()) {
        return p;
    }
    S4_POLICY
}

/// Why a requested restore was refused before anything moved
/// (`storeUpgrade.restore.refused.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalCause {
    MissingBackup,
    MismatchedBackup,
    GarbledMarker,
}

impl std::fmt::Display for RefusalCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingBackup => "the before-upgrade backup is missing",
            Self::MismatchedBackup => "the before-upgrade backup does not match the request",
            Self::GarbledMarker => "the restore request is unreadable or from another build",
        })
    }
}

/// Why a restore stopped (`storeUpgrade.restore.incomplete.*`, or
/// `restore.notStarted.*` before the journal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncompleteCause {
    DiskFull,
    PermissionDenied,
    FileVanished,
    /// S4 under [`S4Policy::Refuse`].
    S4OldBuildWrote,
    /// S4 with a live wallet outside the before lineage (Q92 S4-2): the user
    /// may keep the current files ([`keep_current_files`]).
    S4OtherLineage,
    Other,
}

impl IncompleteCause {
    pub fn of(e: &io::Error) -> Self {
        match e.kind() {
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::DiskFull,
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
                Self::PermissionDenied
            }
            io::ErrorKind::NotFound => Self::FileVanished,
            _ => Self::Other,
        }
    }
}

impl std::fmt::Display for IncompleteCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DiskFull => "disk full",
            Self::PermissionDenied => "permission denied",
            Self::FileVanished => "a file vanished",
            Self::S4OldBuildWrote => "another build wrote to the restored wallet data",
            Self::S4OtherLineage => "the wallet data is not the one the restore put back",
            Self::Other => "file system error",
        })
    }
}

/// A completed restore.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreInfo {
    pub snapshot_dir: PathBuf,
    /// The verified copy of the replaced set.
    pub after_dir: PathBuf,
    /// Copies of files another build wrote mid-restore, if any.
    pub during_dirs: Vec<PathBuf>,
    /// Finished from a journal left by an earlier start.
    pub resumed: bool,
    /// S4: finished keeping what was live.
    pub kept_live: bool,
}

/// One pending payment to hold: its txid and the wallet coins it spends
/// (Q89 §3 option 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPayment {
    pub txid: Txid,
    pub inputs: Vec<OutPoint>,
}

/// The money gate's mark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredMark {
    pub pending: Vec<PendingPayment>,
}

/// What [`pending_restore`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingRestore {
    Requested,
    InProgress,
}

fn root(network_dir: &Path) -> PathBuf {
    snapshot::backups_dir(network_dir)
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether a restore is requested or part-done in `network_dir`.
pub fn pending_restore(network_dir: &Path) -> io::Result<Option<PendingRestore>> {
    let root = root(network_dir);
    if exists(&root.join(RESTORE_JOURNAL)) {
        return Ok(Some(PendingRestore::InProgress));
    }
    match fs::symlink_metadata(root.join(RESTORE_REQUESTED)) {
        Ok(_) => Ok(Some(PendingRestore::Requested)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn refuse_if_restore_pending(network_dir: &Path) -> Result<()> {
    match pending_restore(network_dir) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(StoreUpgradeError::RestorePending {
            dir: network_dir.to_path_buf(),
        }),
        Err(e) => Err(StoreUpgradeError::io("read the restore marker", true)(e)),
    }
}

fn valid_dir_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|r| r.strip_prefix('-'))
        .is_some_and(|r| !r.is_empty() && r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// `name` is exactly what [`fsx::fresh_name`] makes for `prefix`:
/// `<prefix>-YYYYMMDDTHHMMSSZ`, optionally `-<n>`.
fn fresh_name_of(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix).and_then(|r| r.strip_prefix('-')) else {
        return false;
    };
    let (stamp, n) = match rest.split_at_checked(16) {
        Some((stamp, "")) => (stamp, None),
        Some((stamp, tail)) => match tail.strip_prefix('-') {
            Some(n) => (stamp, Some(n)),
            None => return false,
        },
        None => return false,
    };
    let b = stamp.as_bytes();
    let stamp_ok = b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'T'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'Z';
    stamp_ok && n.is_none_or(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Record the user's "Restart and restore": the marker names `snapshot`'s
/// directory and manifest sha256. Writes nothing else; the restore runs at
/// the next start.
pub fn request_restore(network_dir: &Path, snapshot: &SnapshotInfo) -> Result<()> {
    let root = root(network_dir);
    let invalid = || StoreUpgradeError::SnapshotInvalid {
        dir: snapshot.dir().to_path_buf(),
        detail: "not a completed snapshot in this directory's upgrade-backups".into(),
    };
    let in_root =
        snapshot.dir().parent().and_then(|p| p.canonicalize().ok()) == root.canonicalize().ok();
    let name = file_name_of(snapshot.dir());
    if !in_root || !valid_dir_name(&name, snapshot::BEFORE_PREFIX) {
        return Err(invalid());
    }
    let on_disk = snapshot::read_snapshot(snapshot.dir(), true).map_err(|_| invalid())?;
    if on_disk.manifest_sha256() != snapshot.manifest_sha256() {
        return Err(invalid());
    }
    let text = format!(
        "{REQUEST_HEADER}\nformat {FORMAT}\nsnapshot {name}\nmanifest {}\n",
        snapshot.manifest_sha256()
    );
    fsx::write_durable(&root.join(RESTORE_REQUESTED), text.as_bytes())
        .map_err(StoreUpgradeError::io("write the restore request", true))
}

struct Request {
    snapshot: String,
    manifest: String,
}

fn parse_request(text: &str) -> Option<Request> {
    let mut lines = text.lines();
    if lines.next()? != REQUEST_HEADER || lines.next()? != format!("format {FORMAT}") {
        return None;
    }
    let snapshot = lines.next()?.strip_prefix("snapshot ")?.to_string();
    let manifest = lines.next()?.strip_prefix("manifest ")?.to_string();
    if lines.next().is_some()
        || !valid_dir_name(&snapshot, snapshot::BEFORE_PREFIX)
        || manifest.len() != 64
    {
        return None;
    }
    Some(Request { snapshot, manifest })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoreEntry {
    name: String,
    before: Option<String>,
    after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Journal {
    snapshot: String,
    manifest: String,
    staging: String,
    after: String,
    stores: Vec<StoreEntry>,
}

impl Journal {
    fn wallet(&self) -> &StoreEntry {
        &self.stores[REPLACE_ORDER.len() - 1]
    }
}

fn render_journal(j: &Journal) -> String {
    let mut s = format!(
        "{JOURNAL_HEADER}\nformat {FORMAT}\nsnapshot {}\nmanifest {}\nstaging {}\nafter {}\n",
        j.snapshot, j.manifest, j.staging, j.after
    );
    for e in &j.stores {
        s += &format!(
            "store {} before {} after {}\n",
            e.name,
            e.before.as_deref().unwrap_or("-"),
            e.after.as_deref().unwrap_or("-")
        );
    }
    s
}

fn parse_journal(text: &str) -> Option<Journal> {
    let mut lines = text.lines();
    if lines.next()? != JOURNAL_HEADER || lines.next()? != format!("format {FORMAT}") {
        return None;
    }
    let mut field = |key: &str| {
        lines
            .next()?
            .strip_prefix(key)?
            .strip_prefix(' ')
            .map(str::to_string)
    };
    let snapshot = field("snapshot")?;
    let manifest = field("manifest")?;
    let staging = field("staging")?;
    let after = field("after")?;
    let mut stores = Vec::new();
    for line in lines {
        let parts: Vec<&str> = line.split(' ').collect();
        let ["store", name, "before", before, "after", after] = parts[..] else {
            return None;
        };
        let opt = |v: &str| (v != "-").then(|| v.to_string());
        stores.push(StoreEntry {
            name: name.into(),
            before: opt(before),
            after: opt(after),
        });
    }
    let names: Vec<&str> = stores.iter().map(|e| e.name.as_str()).collect();
    let ok = valid_dir_name(&snapshot, snapshot::BEFORE_PREFIX)
        && fresh_name_of(&staging, STAGING_PREFIX)
        && fresh_name_of(&after, AFTER_PREFIX)
        && names == REPLACE_ORDER;
    ok.then_some(Journal {
        snapshot,
        manifest,
        staging,
        after,
        stores,
    })
}

/// The startup entry for a pending restore: resume a journal, run a
/// requested restore, or do nothing. Runs before any store opens in the
/// process (F4-a, Q86 (f)).
pub fn run_pending_restore(network_dir: &Path, now: SystemTime) -> Result<Option<RestoreInfo>> {
    let pending = pending_restore(network_dir)
        .map_err(StoreUpgradeError::io("read the restore marker", true))?;
    let Some(pending) = pending else {
        return Ok(None);
    };
    gates(network_dir)?;
    match pending {
        PendingRestore::InProgress => resume(network_dir, now).map(Some),
        PendingRestore::Requested => {
            let snapshot = verify_request(network_dir).map_err(|c| refuse(network_dir, c))?;
            fresh(network_dir, &snapshot, now).map(Some)
        }
    }
}

/// Run the restore the request names: the verified `snapshot` back in, the
/// replaced set kept in `upgrade-backups/after-upgrade-<UTC>/`. Reachable
/// only through a request naming `snapshot`.
pub fn restore(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    now: SystemTime,
) -> Result<RestoreInfo> {
    gates(network_dir)?;
    let pending = pending_restore(network_dir)
        .map_err(StoreUpgradeError::io("read the restore marker", true))?;
    if pending != Some(PendingRestore::Requested) {
        return Err(StoreUpgradeError::SnapshotInvalid {
            dir: snapshot.dir().to_path_buf(),
            detail: "no restore was requested".into(),
        });
    }
    let verified = verify_request(network_dir).map_err(|c| refuse(network_dir, c))?;
    if verified.manifest_sha256() != snapshot.manifest_sha256() || verified.dir() != snapshot.dir()
    {
        return Err(refuse(network_dir, RefusalCause::MismatchedBackup));
    }
    fresh(network_dir, &verified, now)
}

/// Read the request and the snapshot it names, every file checked against
/// the manifest.
fn verify_request(network_dir: &Path) -> std::result::Result<SnapshotInfo, RefusalCause> {
    let root = root(network_dir);
    let text = fs::read(root.join(RESTORE_REQUESTED)).map_err(|_| RefusalCause::GarbledMarker)?;
    let text = String::from_utf8(text).map_err(|_| RefusalCause::GarbledMarker)?;
    let request = parse_request(&text).ok_or(RefusalCause::GarbledMarker)?;
    let dir = root.join(&request.snapshot);
    if !dir.is_dir() || !dir.join(snapshot::MARKER).exists() {
        return Err(RefusalCause::MissingBackup);
    }
    let info = snapshot::read_snapshot(&dir, true).map_err(|_| {
        if snapshot_files_missing(&dir) {
            RefusalCause::MissingBackup
        } else {
            RefusalCause::MismatchedBackup
        }
    })?;
    if info.manifest_sha256() != request.manifest {
        return Err(RefusalCause::MismatchedBackup);
    }
    Ok(info)
}

fn snapshot_files_missing(dir: &Path) -> bool {
    fs::read_to_string(dir.join(snapshot::MARKER)).is_ok_and(|text| {
        text.lines()
            .filter_map(|l| l.strip_prefix("file "))
            .filter_map(|r| r.split(' ').next())
            .any(|name| !dir.join(name).exists())
    })
}

/// MG (k): the refusal shows once. The request is removed; nothing else is
/// touched.
fn refuse(network_dir: &Path, cause: RefusalCause) -> StoreUpgradeError {
    if let Err(e) = fsx::remove_durable(&root(network_dir).join(RESTORE_REQUESTED)) {
        tracing::warn!(error = %e, "store upgrade: could not remove the refused restore request");
    }
    tracing::warn!(%cause, "store upgrade: restore refused");
    StoreUpgradeError::RestoreRefused { cause }
}

/// F4-a/F4-b: no store has opened in this process, none is held, and no
/// store lock is taken. Touches nothing.
fn gates(network_dir: &Path) -> Result<()> {
    if crate::token::stores_opened_in_process() {
        return Err(StoreUpgradeError::RestoreAfterOpen);
    }
    crate::token::refuse_if_held(network_dir)?;
    for lock in STORE_LOCKS {
        let path = network_dir.join(lock);
        let file = match fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(StoreUpgradeError::io("open a store lock", true)(e)),
        };
        match file.try_lock() {
            Ok(()) => drop(file),
            Err(fs::TryLockError::WouldBlock) => {
                return Err(StoreUpgradeError::StoreHeld { path });
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(StoreUpgradeError::io("probe a store lock", true)(e));
            }
        }
    }
    Ok(())
}

fn not_started(e: &io::Error, what: &str) -> StoreUpgradeError {
    StoreUpgradeError::RestoreIncomplete {
        cause: IncompleteCause::of(e),
        journal_written: false,
        detail: format!("{what}: {e}"),
    }
}

fn incomplete(e: &io::Error, what: &str) -> StoreUpgradeError {
    StoreUpgradeError::RestoreIncomplete {
        cause: IncompleteCause::of(e),
        journal_written: true,
        detail: format!("{what}: {e}"),
    }
}

fn incomplete_other(detail: String) -> StoreUpgradeError {
    StoreUpgradeError::RestoreIncomplete {
        cause: IncompleteCause::Other,
        journal_written: true,
        detail,
    }
}

fn sides_of(network_dir: &Path, name: &str) -> Vec<PathBuf> {
    SIDE_SUFFIXES
        .iter()
        .map(|s| network_dir.join(format!("{name}{s}")))
        .filter(|p| exists(p))
        .collect()
}

/// Step 2: fold each live store's log into its main file and close it, so
/// the main file is the whole store and no side file remains. Stores keep
/// their journal mode.
fn quiesce(network_dir: &Path) -> Result<()> {
    let stopped = |detail: String| StoreUpgradeError::RestoreIncomplete {
        cause: IncompleteCause::Other,
        journal_written: false,
        detail,
    };
    for name in STORE_FILES {
        let path = network_dir.join(name);
        if !path.exists() {
            continue;
        }
        let conn =
            open_existing(&path).map_err(|e| stopped(format!("open {name} to quiesce: {e}")))?;
        let busy: i64 = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
            .map_err(|e| stopped(format!("quiesce {name}: {e}")))?;
        if busy != 0 {
            return Err(stopped(format!("{name} is busy and could not be quiesced")));
        }
        conn.close()
            .map_err(|(_, e)| stopped(format!("close {name}: {e}")))?;
        fsx::fsync_file(&path).map_err(|e| not_started(&e, "fsync a quiesced store"))?;
        if let Some(side) = sides_of(network_dir, name).first() {
            return Err(stopped(format!(
                "{} remains after quiesce",
                file_name_of(side)
            )));
        }
    }
    Ok(())
}

fn hash_opt(path: &Path) -> io::Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(_) => sha256_file(path).map(Some),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Copy `src` to `dest` through `dest.partial`, owner-only, fsynced, checked
/// against `sha`. `Ok(false)` when the copy does not match (the partial is
/// removed).
fn verified_copy(src: &Path, dest: &Path, sha: &str) -> io::Result<bool> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".partial");
    let tmp = PathBuf::from(tmp);
    fault_io("restore.copy")?;
    fs::copy(src, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fsx::fsync_file(&tmp)?;
    if sha256_file(&tmp)? != sha {
        fs::remove_file(&tmp)?;
        return Ok(false);
    }
    fs::rename(&tmp, dest)?;
    Ok(true)
}

/// The dirs a fresh attempt creates before its journal, each recorded
/// durably in `RESTORE-ATTEMPT` before it is created (design §3), so a
/// failure or a crash removes exactly these.
struct Attempt {
    root: PathBuf,
    created: Vec<String>,
}

/// Q92 92-4: a recorded name is one path component under `upgrade-backups/`
/// that [`fsx::fresh_name`] makes for an attempt's own dirs, and is not a
/// symlink. Anything else is refused and logged, never removed.
fn attempt_entry_ok(root: &Path, name: &str) -> bool {
    let shaped = fresh_name_of(name, AFTER_PREFIX) || fresh_name_of(name, STAGING_PREFIX);
    let ok = shaped
        && match fs::symlink_metadata(root.join(name)) {
            Ok(m) => m.is_dir() && !m.file_type().is_symlink(),
            Err(e) => e.kind() == io::ErrorKind::NotFound,
        };
    if !ok {
        tracing::warn!(
            entry = name,
            "store upgrade: refused a restore attempt record entry"
        );
    }
    ok
}

impl Attempt {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            created: Vec::new(),
        }
    }

    /// The record a crashed attempt left, keeping only entries that pass
    /// [`attempt_entry_ok`].
    fn left_over(root: &Path) -> Option<Self> {
        let text = fs::read_to_string(root.join(RESTORE_ATTEMPT)).ok()?;
        let mut lines = text.lines();
        let mut created = Vec::new();
        if lines.next() == Some(ATTEMPT_HEADER) && lines.next() == Some("format 1") {
            created = lines
                .filter_map(|l| l.strip_prefix("dir "))
                .filter(|name| attempt_entry_ok(root, name))
                .map(str::to_string)
                .collect();
        }
        Some(Self {
            root: root.to_path_buf(),
            created,
        })
    }

    /// Record `name` durably before it is created.
    fn will_create(&mut self, name: &str) -> io::Result<()> {
        debug_assert!(fresh_name_of(name, AFTER_PREFIX) || fresh_name_of(name, STAGING_PREFIX));
        self.created.push(name.to_string());
        let mut text = format!("{ATTEMPT_HEADER}\nformat {FORMAT}\n");
        for p in &self.created {
            text += &format!("dir {p}\n");
        }
        fsx::write_durable(&self.root.join(RESTORE_ATTEMPT), text.as_bytes())
    }

    /// Remove every recorded dir, newest first, then the record. A dir or
    /// file that is already gone counts as removed (Q92 92-5).
    fn cleanup(&self) -> io::Result<()> {
        for name in self.created.iter().rev() {
            let dir = self.root.join(name);
            for e in read_dir_or_empty(&dir)? {
                fault_cleanup()?;
                remove_file_if_present(&e)?;
            }
            match fs::remove_dir(&dir) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        fsx::fsync_dir(&self.root)?;
        fsx::remove_durable(&self.root.join(RESTORE_ATTEMPT))
    }

    /// The cleanup failed: mark each of this attempt's dirs safe to delete,
    /// then drop the record so no later run removes them.
    fn leave_safe_to_delete(&self, cause: &str) {
        for name in &self.created {
            let dir = self.root.join(name);
            if !dir.is_dir() {
                continue;
            }
            let mut files: Vec<String> = read_dir_or_empty(&dir)
                .unwrap_or_default()
                .iter()
                .map(|p| file_name_of(p))
                .filter(|n| n != SAFE_TO_DELETE)
                .collect();
            files.sort();
            let mut text = "incomplete, safe to delete\n\nA restore attempt stopped before it \
                            changed anything. Every file here is a partial copy of a file that \
                            is still in place.\n"
                .to_string();
            for f in files {
                text += &format!("file {f}\n");
            }
            text += &format!("cause: {cause}\n");
            if let Err(e) = fsx::write_durable(&dir.join(SAFE_TO_DELETE), text.as_bytes()) {
                tracing::warn!(dir = %dir.display(), error = %e,
                    "store upgrade: could not mark a partial restore copy safe to delete");
            }
        }
        if let Err(e) = fsx::remove_durable(&self.root.join(RESTORE_ATTEMPT)) {
            tracing::warn!(error = %e, "store upgrade: could not remove the restore attempt record");
        }
    }

    /// Remove the copies, or mark them safe to delete when that fails.
    fn undo(&self, cause: &str) {
        if let Err(e) = self.cleanup() {
            tracing::warn!(error = %e, "store upgrade: restore attempt cleanup failed");
            self.leave_safe_to_delete(&format!("{cause}; cleanup: {e}"));
        }
    }
}

fn read_dir_or_empty(dir: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(dir) {
        Ok(it) => it.map(|e| e.map(|e| e.path())).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A crash before an earlier attempt's journal left its record: remove what
/// it lists, and only that.
fn sweep_left_over_attempt(root: &Path) {
    if let Some(attempt) = Attempt::left_over(root) {
        attempt.undo("left by an attempt that stopped before its journal");
    }
}

/// Before the journal: ENOSPC is `RestoreNoSpace` with the request cleared
/// (MG (s)); anything else is `RestoreIncomplete{journal_written: false}`
/// with the request kept (MG (o)).
fn before_journal_error(
    network_dir: &Path,
    e: &io::Error,
    what: &str,
    needed: u64,
) -> StoreUpgradeError {
    if IncompleteCause::of(e) == IncompleteCause::DiskFull {
        if let Err(e) = fsx::remove_durable(&root(network_dir).join(RESTORE_REQUESTED)) {
            tracing::warn!(error = %e, "store upgrade: could not clear the restore request");
        }
        return StoreUpgradeError::RestoreNoSpace {
            needed: Some(needed),
        };
    }
    not_started(e, what)
}

enum CopyStop {
    Io(io::Error, &'static str),
    Mismatch,
}

/// Steps 4 and 5: the after set copied and verified into `after-upgrade`,
/// then the before set into `restore-staging`.
fn copy_sets(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    j: &Journal,
    attempt: &mut Attempt,
) -> std::result::Result<(), CopyStop> {
    let root = root(network_dir);
    let io = |what: &'static str| move |e: io::Error| CopyStop::Io(e, what);
    for (dir_name, what) in [
        (&j.after, "copy the live set into after-upgrade"),
        (&j.staging, "stage the before-upgrade set"),
    ] {
        let staging = dir_name == &j.staging;
        let dir = root.join(dir_name);
        attempt.will_create(dir_name).map_err(io(what))?;
        fsx::create_private_dir(&dir).map_err(io(what))?;
        for e in &j.stores {
            let (src, sha) = match (staging, &e.before, &e.after) {
                (true, Some(sha), _) => (snapshot.dir().join(&e.name), sha),
                (false, _, Some(sha)) => (network_dir.join(&e.name), sha),
                _ => continue,
            };
            match verified_copy(&src, &dir.join(&e.name), sha) {
                Ok(true) => {}
                Ok(false) if staging => return Err(CopyStop::Mismatch),
                Ok(false) => {
                    return Err(CopyStop::Io(
                        io::Error::other(format!("{} changed while it was copied", e.name)),
                        what,
                    ));
                }
                Err(err) => return Err(CopyStop::Io(err, what)),
            }
        }
        fsx::fsync_dir(&dir).map_err(io(what))?;
    }
    fsx::fsync_dir(&root).map_err(io("fsync upgrade-backups"))
}

/// The extra space a restore's copies take: the live set plus the before set.
fn space_needed(network_dir: &Path, snapshot: &SnapshotInfo) -> u64 {
    let live: u64 = STORE_FILES
        .iter()
        .filter_map(|n| fs::metadata(network_dir.join(n)).ok())
        .map(|m| m.len())
        .sum();
    live + snapshot.files().iter().map(|f| f.bytes).sum::<u64>()
}

/// Steps 0–8 of a fresh run (design §2).
fn fresh(network_dir: &Path, snapshot: &SnapshotInfo, now: SystemTime) -> Result<RestoreInfo> {
    let root = root(network_dir);
    sweep_left_over_attempt(&root);
    quiesce(network_dir)?;
    fault("restore.after_quiesce")?;
    let mut stores = Vec::new();
    for name in REPLACE_ORDER {
        let after =
            hash_opt(&network_dir.join(name)).map_err(|e| not_started(&e, "hash a live store"))?;
        let before = snapshot
            .files()
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.sha256.clone());
        stores.push(StoreEntry {
            name: name.into(),
            before,
            after,
        });
    }
    let journal = Journal {
        snapshot: file_name_of(snapshot.dir()),
        manifest: snapshot.manifest_sha256().to_string(),
        after: fsx::fresh_name(&root, AFTER_PREFIX, now),
        staging: fsx::fresh_name(&root, STAGING_PREFIX, now),
        stores,
    };
    let needed = space_needed(network_dir, snapshot);
    let mut attempt = Attempt::new(&root);
    match copy_sets(network_dir, snapshot, &journal, &mut attempt) {
        Ok(()) => {}
        Err(CopyStop::Mismatch) => {
            attempt.undo("the before-upgrade backup changed");
            return Err(refuse(network_dir, RefusalCause::MismatchedBackup));
        }
        Err(CopyStop::Io(e, what)) => {
            attempt.undo(&e.to_string());
            return Err(before_journal_error(network_dir, &e, what, needed));
        }
    }
    let mark = root.join(RESTORED_MARK);
    let mark_was_set = exists(&mark);
    let written = staged_pending(&root.join(&journal.staging))
        .and_then(|pending| add_to_mark(network_dir, &journal.snapshot, &pending, now))
        .and_then(|()| {
            fault_io("restore.journal")?;
            fsx::write_durable(
                &root.join(RESTORE_JOURNAL),
                render_journal(&journal).as_bytes(),
            )
        });
    if let Err(e) = written {
        if !mark_was_set && let Err(e) = fsx::remove_durable(&mark) {
            tracing::warn!(error = %e, "store upgrade: could not remove this attempt's mark");
        }
        attempt.undo(&e.to_string());
        return Err(before_journal_error(
            network_dir,
            &e,
            "write the restore journal",
            needed,
        ));
    }
    if let Err(e) = fsx::remove_durable(&root.join(RESTORE_ATTEMPT)) {
        tracing::warn!(error = %e, "store upgrade: could not remove the restore attempt record");
    }
    fault("restore.after_journal")?;
    #[cfg(test)]
    let started = std::time::Instant::now();
    replace(network_dir, &journal).map_err(|e| incomplete(&e, "replace a store"))?;
    #[cfg(test)]
    LAST_REPLACE.with(|t| t.set(Some(started.elapsed())));
    finish(network_dir, &journal, Vec::new(), false)
}

/// Step 7 (D2): each staged store renamed over its live file in
/// [`REPLACE_ORDER`], wallet.sqlite last. A store the before set lacks is
/// removed (its verified copy is in `after-upgrade`).
fn replace(network_dir: &Path, j: &Journal) -> io::Result<()> {
    let staging = root(network_dir).join(&j.staging);
    for e in &j.stores {
        let live = network_dir.join(&e.name);
        match (&e.before, &e.after) {
            (Some(_), _) => {
                fault_rename()?;
                fs::rename(staging.join(&e.name), &live)?;
            }
            (None, Some(_)) => {
                fault_rename()?;
                fs::remove_file(&live)?;
            }
            (None, None) => continue,
        }
        fault_replaced()?;
    }
    fsx::fsync_dir(network_dir)
}

type RawPayment = (Vec<u8>, Vec<Vec<u8>>);

/// Unmined transactions that spend a wallet coin, from `conn`.
fn pending_payments(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<RawPayment>> {
    if !crate::sql::has_table(conn, "core_transactions")?
        || !crate::sql::has_table(conn, "core_utxos")?
    {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT t.txid, u.outpoint FROM core_transactions t \
         JOIN core_utxos u ON u.wallet_id = t.wallet_id AND u.spent_in_txid = t.txid \
         WHERE t.height IS NULL ORDER BY t.txid, u.outpoint",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out: Vec<RawPayment> = Vec::new();
    for row in rows {
        let (txid, outpoint) = row?;
        match out.last_mut() {
            Some((t, inputs)) if *t == txid => inputs.push(outpoint),
            _ => out.push((txid, vec![outpoint])),
        }
    }
    Ok(out)
}

/// The before set's pending payments, from the staged wallet.sqlite (read
/// immutable: the staged copy has no log).
fn staged_pending(staging: &Path) -> io::Result<Vec<RawPayment>> {
    let wallet = staging.join(STORE_FILES[0]);
    if !wallet.exists() {
        return Ok(Vec::new());
    }
    open_immutable(&wallet)
        .and_then(|c| pending_payments(&c))
        .map_err(|e| io::Error::other(format!("read the pending payments: {e}")))
}

/// The pending payments of a live wallet.sqlite with its log applied, read
/// from a private copy of the main file and its log, so nothing beside the
/// live file is written or replayed into it.
fn live_pending(network_dir: &Path) -> io::Result<Vec<RawPayment>> {
    let wallet = network_dir.join(STORE_FILES[0]);
    if !wallet.exists() {
        return Ok(Vec::new());
    }
    let scratch = tempfile::Builder::new()
        .prefix(READ_PREFIX)
        .tempdir_in(root(network_dir))?;
    let copy = scratch.path().join(STORE_FILES[0]);
    fs::copy(&wallet, &copy)?;
    for suffix in ["-wal", "-journal"] {
        let side = network_dir.join(format!("{}{suffix}", STORE_FILES[0]));
        if side.exists() {
            fs::copy(
                &side,
                scratch.path().join(format!("{}{suffix}", STORE_FILES[0])),
            )?;
        }
    }
    open_existing(&copy)
        .and_then(|c| pending_payments(&c))
        .map_err(|e| io::Error::other(format!("read the pending payments: {e}")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Step 6: set the mark, adding `pending` to any entries an earlier restore
/// left (not yet synced).
fn add_to_mark(
    network_dir: &Path,
    from: &str,
    pending: &[RawPayment],
    now: SystemTime,
) -> io::Result<()> {
    let path = root(network_dir).join(RESTORED_MARK);
    let mut lines: Vec<String> = match fs::read_to_string(&path) {
        Ok(t) => t
            .lines()
            .filter(|l| l.starts_with("pending "))
            .map(str::to_string)
            .collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    for (txid, inputs) in pending {
        let inputs: Vec<String> = inputs.iter().map(|o| hex(o)).collect();
        let line = format!("pending {} {}", hex(txid), inputs.join(","));
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    let mut text = format!(
        "{MARK_HEADER}\nformat {FORMAT}\nrestored {} from {from}\n",
        fsx::utc_stamp(now)
    );
    for l in lines {
        text += &l;
        text.push('\n');
    }
    fsx::write_durable(&path, text.as_bytes())
}

/// Whether the money gate's mark is set.
pub fn restored_mark_present(network_dir: &Path) -> bool {
    exists(&root(network_dir).join(RESTORED_MARK))
}

/// The money gate's mark and the pending payments to hold, if set. An entry
/// that can't be read fails the call: the holds must never be silently
/// short.
pub fn restored_mark(network_dir: &Path) -> Result<Option<RestoredMark>> {
    let path = root(network_dir).join(RESTORED_MARK);
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreUpgradeError::io("read the restored mark", true)(e)),
    };
    let bad = |line: &str| StoreUpgradeError::SnapshotInvalid {
        dir: path.clone(),
        detail: format!("unreadable pending entry {line:?}"),
    };
    let mut pending = Vec::new();
    for line in text.lines().filter_map(|l| l.strip_prefix("pending ")) {
        let (txid, inputs) = line.split_once(' ').ok_or_else(|| bad(line))?;
        let txid: [u8; 32] = unhex(txid)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| bad(line))?;
        let inputs = inputs
            .split(',')
            .map(|o| {
                unhex(o)
                    .and_then(|b| decode_outpoint(&b))
                    .ok_or_else(|| bad(line))
            })
            .collect::<Result<Vec<_>>>()?;
        pending.push(PendingPayment {
            txid: Txid::from_byte_array(txid),
            inputs,
        });
    }
    Ok(Some(RestoredMark { pending }))
}

/// The `core_utxos.outpoint` encoding: platform-wallet-storage's bincode
/// blob, every byte consumed.
fn decode_outpoint(bytes: &[u8]) -> Option<OutPoint> {
    let (op, used): (OutPoint, usize) =
        bincode::serde::decode_from_slice(bytes, bincode::config::standard()).ok()?;
    (used == bytes.len()).then_some(op)
}

/// Clear the mark on the first all-Synced (Core, Platform, Shielded; off
/// counts as satisfied). Returns whether a mark was cleared. The token
/// proves this process opened the restored store.
pub fn clear_restored_mark_on_first_all_synced(opened: &PersisterOpened) -> Result<bool> {
    let mark = root(opened.network_dir()).join(RESTORED_MARK);
    if !exists(&mark) {
        return Ok(false);
    }
    fsx::remove_durable(&mark).map_err(StoreUpgradeError::io("clear the restored mark", true))?;
    Ok(true)
}

#[cfg(test)]
fn fault_io(point: &'static str) -> io::Result<()> {
    fault(point)
        .map_err(|_| io::Error::new(io::ErrorKind::StorageFull, format!("injected at {point}")))
}

#[cfg(not(test))]
#[inline(always)]
fn fault_io(_point: &'static str) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn fault_cleanup() -> io::Result<()> {
    if CLEANUP_FAILS.with(|c| c.get()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "injected cleanup failure",
        ));
    }
    Ok(())
}

#[cfg(not(test))]
#[inline(always)]
fn fault_cleanup() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn fault_rename() -> io::Result<()> {
    fault("restore.rename")
        .map_err(|_| io::Error::new(io::ErrorKind::PermissionDenied, "injected rename failure"))
}

#[cfg(not(test))]
#[inline(always)]
fn fault_rename() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
fn fault_replaced() -> io::Result<()> {
    fault("restore.replaced").map_err(|_| io::Error::other("injected crash after a replace"))
}

#[cfg(not(test))]
#[inline(always)]
fn fault_replaced() -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// Fail-first switch: leave side files beside a store the resume puts back.
    pub(crate) static SKIP_SIDE_UNLINK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Fail-first switch: move side files aside at S4.
    pub(crate) static MOVE_SIDES_AT_S4: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Every removal in an attempt's cleanup fails.
    pub(crate) static CLEANUP_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The S4 switch, flipped by its own test.
    pub(crate) static S4_FOR_TEST: std::cell::Cell<Option<S4Policy>> =
        const { std::cell::Cell::new(None) };
    /// How long the last fresh run's replaces took.
    pub(crate) static LAST_REPLACE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
fn skip_side_unlink() -> bool {
    SKIP_SIDE_UNLINK.with(|s| s.get())
}

#[cfg(not(test))]
fn skip_side_unlink() -> bool {
    false
}

#[cfg(test)]
fn move_sides_at_s4() -> bool {
    MOVE_SIDES_AT_S4.with(|s| s.get())
}

#[cfg(not(test))]
fn move_sides_at_s4() -> bool {
    false
}

/// This resume run's `during-restore-<UTC>/`, created on first use; each run
/// gets its own.
struct During {
    root: PathBuf,
    now: SystemTime,
    dir: Option<PathBuf>,
    kept: Vec<String>,
}

impl During {
    fn new(root: &Path, now: SystemTime) -> Self {
        Self {
            root: root.to_path_buf(),
            now,
            dir: None,
            kept: Vec::new(),
        }
    }

    fn dir(&mut self) -> io::Result<PathBuf> {
        if let Some(d) = &self.dir {
            return Ok(d.clone());
        }
        let d = self
            .root
            .join(fsx::fresh_name(&self.root, DURING_PREFIX, self.now));
        fsx::create_private_dir(&d)?;
        fsx::fsync_dir(&self.root)?;
        self.dir = Some(d.clone());
        Ok(d)
    }

    /// Q92 V1 step 1: copy `path` in, unopened, fsynced and verified, with
    /// `why` in `KEPT.txt`. The live file stays where it is.
    fn keep_copy(&mut self, path: &Path, why: &str) -> io::Result<()> {
        let dir = self.dir()?;
        let name = file_name_of(path);
        let mut dest = dir.join(&name);
        let mut n = 2;
        while exists(&dest) {
            dest = dir.join(format!("{name}.{n}"));
            n += 1;
        }
        let sha = sha256_file(path)?;
        if !verified_copy(path, &dest, &sha)? {
            return Err(io::Error::other(format!(
                "{name} changed while it was kept"
            )));
        }
        fsx::fsync_dir(&dir)?;
        self.kept.push(format!("{}: {why}", file_name_of(&dest)));
        fsx::write_durable(&dir.join(KEPT), (self.kept.join("\n") + "\n").as_bytes())?;
        tracing::warn!(from = %path.display(), to = %dest.display(),
            "store upgrade: kept a copy of a file written mid-restore");
        Ok(())
    }
}

const WRITTEN_MID_RESTORE: &str = "written by another build while a restore was part-done";

/// Resume, incomplete phase (design §5 with Q92 V1): bring one store to its
/// before file from whatever is there, never opening it. Anything another
/// build wrote is copied and verified into `during-restore` first, then the
/// side files are unlinked and the staged file is renamed over the live one,
/// so the live file is never missing.
fn settle(
    network_dir: &Path,
    j: &Journal,
    snapshot_dir: &Path,
    e: &StoreEntry,
    during: &mut During,
) -> io::Result<()> {
    let root = root(network_dir);
    let live = network_dir.join(&e.name);
    let staged = root.join(&j.staging).join(&e.name);
    let sides = sides_of(network_dir, &e.name);
    let found = hash_opt(&live)?;
    let modified = found.is_some() && found != e.before && found != e.after;
    if !sides.is_empty() || modified {
        if found.is_some() {
            during.keep_copy(&live, WRITTEN_MID_RESTORE)?;
        }
        for side in &sides {
            during.keep_copy(side, WRITTEN_MID_RESTORE)?;
        }
    }
    if !skip_side_unlink() {
        for side in &sides {
            fs::remove_file(side)?;
        }
        if !sides.is_empty() {
            fsx::fsync_dir(network_dir)?;
        }
    }
    match (&e.before, &found) {
        (Some(b), Some(f)) if b == f => Ok(()),
        (Some(sha), _) => {
            if hash_opt(&staged)?.as_ref() != Some(sha) {
                remove_file_if_present(&staged)?;
                fsx::ensure_private_dir(&root.join(&j.staging))?;
                if !verified_copy(&snapshot_dir.join(&e.name), &staged, sha)? {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{} in the before-upgrade backup no longer matches", e.name),
                    ));
                }
            }
            fault_rename()?;
            fs::rename(&staged, &live)?;
            fsx::fsync_dir(network_dir)?;
            fault_replaced()
        }
        (None, Some(_)) => {
            fault_rename()?;
            fs::remove_file(&live)?;
            fsx::fsync_dir(network_dir)
        }
        (None, None) => Ok(()),
    }
}

/// Step 8, also the end of every resume: each live store is its before file
/// with no side file, then the staging dir, the request and the journal go.
fn finish(
    network_dir: &Path,
    j: &Journal,
    during_dirs: Vec<PathBuf>,
    resumed: bool,
) -> Result<RestoreInfo> {
    let root = root(network_dir);
    let after_dir = root.join(&j.after);
    let fail = |e: io::Error| incomplete(&e, "finish the restore");
    for e in &j.stores {
        let live = hash_opt(&network_dir.join(&e.name)).map_err(fail)?;
        let side_left = !sides_of(network_dir, &e.name).is_empty();
        if live != e.before || (side_left && !skip_side_unlink()) {
            return Err(incomplete_other(format!(
                "{} is not the before-upgrade file",
                e.name
            )));
        }
    }
    for dir in [network_dir, &after_dir, &root] {
        fsx::fsync_dir(dir).map_err(fail)?;
    }
    remove_staging(&root, j).map_err(fail)?;
    fsx::remove_durable(&root.join(RESTORE_REQUESTED)).map_err(fail)?;
    fault("restore.before_journal_removal")?;
    fsx::remove_durable(&root.join(RESTORE_JOURNAL)).map_err(fail)?;
    tracing::info!(after = %after_dir.display(), resumed, "store upgrade: restore complete");
    Ok(RestoreInfo {
        snapshot_dir: root.join(&j.snapshot),
        after_dir,
        during_dirs,
        resumed,
        kept_live: false,
    })
}

/// The staging dir, holding at most copies of before files.
fn remove_staging(root: &Path, j: &Journal) -> io::Result<()> {
    let staging = root.join(&j.staging);
    if !staging.exists() {
        return Ok(());
    }
    for e in &j.stores {
        let staged = staging.join(&e.name);
        if e.before.is_some() && hash_opt(&staged)? == e.before {
            fs::remove_file(&staged)?;
        }
    }
    match fs::remove_dir(&staging) {
        Ok(()) => fsx::fsync_dir(root),
        Err(e) => {
            tracing::warn!(dir = %staging.display(), error = %e,
                "store upgrade: left a restore staging dir that is not empty");
            Ok(())
        }
    }
}

/// Where a resume stands for wallet.sqlite.
#[derive(Debug, PartialEq, Eq)]
enum WalletPhase {
    /// Not yet replaced: the incomplete phase.
    Incomplete,
    /// Replaced (S4), the live wallet in the before lineage (Q92 S4-2).
    S4Lineage,
    /// Replaced, the live wallet something else.
    S4Other,
}

fn wallet_ids(path: &Path) -> rusqlite::Result<Vec<Vec<u8>>> {
    let conn = open_immutable(path)?;
    let mut stmt = conn.prepare("SELECT wallet_id FROM wallets ORDER BY wallet_id")?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

fn refinery_max(path: &Path) -> rusqlite::Result<i64> {
    open_immutable(path)?.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM refinery_schema_history",
        [],
        |r| r.get(0),
    )
}

/// The staged wallet is gone (the replace renames it) and a live wallet
/// exists: S4. Its lineage (Q92 S4-2), from the main file read immutable
/// (no side file is opened or replayed): not the after file, and either the
/// before file or the before set's schema version with the same wallet ids.
fn wallet_phase(
    network_dir: &Path,
    j: &Journal,
    snapshot: &SnapshotInfo,
) -> io::Result<WalletPhase> {
    let wallet = j.wallet();
    if wallet.before.is_none() || exists(&root(network_dir).join(&j.staging).join(&wallet.name)) {
        return Ok(WalletPhase::Incomplete);
    }
    let live = network_dir.join(&wallet.name);
    let found = hash_opt(&live)?;
    if found.is_none() || found == wallet.after {
        return Ok(WalletPhase::Incomplete);
    }
    if found == wallet.before {
        return Ok(WalletPhase::S4Lineage);
    }
    let before = snapshot.dir().join(&wallet.name);
    let same = refinery_max(&live).ok() == Some(snapshot.refinery_max())
        && wallet_ids(&live)
            .ok()
            .is_some_and(|ids| wallet_ids(&before).ok() == Some(ids));
    Ok(if same {
        WalletPhase::S4Lineage
    } else {
        WalletPhase::S4Other
    })
}

/// Resume from the journal, before any store opens (design §5).
fn resume(network_dir: &Path, now: SystemTime) -> Result<RestoreInfo> {
    let (journal, snapshot) = read_journal(network_dir)?;
    let root = root(network_dir);
    if !restored_mark_present(network_dir) {
        let pending = open_immutable(&snapshot.dir().join(STORE_FILES[0]))
            .and_then(|c| pending_payments(&c))
            .map_err(|e| {
                incomplete(
                    &io::Error::other(e.to_string()),
                    "read the pending payments",
                )
            })?;
        add_to_mark(network_dir, &journal.snapshot, &pending, now)
            .map_err(|e| incomplete(&e, "write the restored mark"))?;
    }
    let phase = wallet_phase(network_dir, &journal, &snapshot)
        .map_err(|e| incomplete(&e, "read the wallet"))?;
    if phase != WalletPhase::Incomplete {
        let touched = journal.stores.iter().any(|e| {
            !sides_of(network_dir, &e.name).is_empty()
                || hash_opt(&network_dir.join(&e.name)).ok().flatten() != e.before
        });
        return match (phase, s4_policy()) {
            (WalletPhase::S4Lineage, _) if !touched => {
                finish(network_dir, &journal, Vec::new(), true)
            }
            (WalletPhase::S4Lineage, S4Policy::KeepLive) => keep_live(network_dir, &journal, now),
            (WalletPhase::S4Lineage, S4Policy::Refuse) => {
                Err(StoreUpgradeError::RestoreIncomplete {
                    cause: IncompleteCause::S4OldBuildWrote,
                    journal_written: true,
                    detail: "the wallet data was already restored and another build wrote to it"
                        .into(),
                })
            }
            _ => Err(StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::S4OtherLineage,
                journal_written: true,
                detail: "the live wallet data is neither the restored nor the replaced data".into(),
            }),
        };
    }
    let mut during = During::new(&root, now);
    for e in &journal.stores {
        settle(network_dir, &journal, snapshot.dir(), e, &mut during)
            .map_err(|err| incomplete(&err, "finish the restore"))?;
    }
    finish(
        network_dir,
        &journal,
        during.dir.into_iter().collect(),
        true,
    )
}

/// The journal, its after-upgrade copies checked, and the verified snapshot
/// it names. A stale attempt record beside it is dropped (its dirs are the
/// journal's).
fn read_journal(network_dir: &Path) -> Result<(Journal, SnapshotInfo)> {
    let root = root(network_dir);
    if exists(&root.join(RESTORE_ATTEMPT))
        && let Err(e) = fsx::remove_durable(&root.join(RESTORE_ATTEMPT))
    {
        tracing::warn!(error = %e, "store upgrade: could not remove a stale attempt record");
    }
    let text = fs::read_to_string(root.join(RESTORE_JOURNAL))
        .map_err(|e| incomplete(&e, "read the restore journal"))?;
    let journal = parse_journal(&text)
        .ok_or_else(|| incomplete_other("the restore journal is unreadable".into()))?;
    let after_dir = root.join(&journal.after);
    for e in &journal.stores {
        if let Some(sha) = &e.after {
            let found = hash_opt(&after_dir.join(&e.name))
                .map_err(|err| incomplete(&err, "read after-upgrade"))?;
            if found.as_ref() != Some(sha) {
                return Err(incomplete_other(format!(
                    "after-upgrade {} does not match the journal",
                    e.name
                )));
            }
        }
    }
    let snapshot = snapshot::read_snapshot(&root.join(&journal.snapshot), true)
        .map_err(|d| incomplete_other(format!("the before-upgrade backup: {d}")))?;
    if snapshot.manifest_sha256() != journal.manifest {
        return Err(incomplete_other(
            "the before-upgrade backup changed since the restore began".into(),
        ));
    }
    Ok((journal, snapshot))
}

/// S4, kept (Q92 S4-3, S4-5): whatever is live stays, side files included;
/// the live wallet's pending payments, read with its log from a private
/// copy, join the mark's; a `KEPT.txt` line in `after-upgrade`; then the
/// request, then the journal, go. The mark stays.
fn keep_live(network_dir: &Path, j: &Journal, now: SystemTime) -> Result<RestoreInfo> {
    let root = root(network_dir);
    let fail = |e: io::Error| incomplete(&e, "finish keeping the current files");
    if move_sides_at_s4() {
        let dir = root.join(fsx::fresh_name(&root, DURING_PREFIX, now));
        fsx::create_private_dir(&dir).map_err(fail)?;
        for side in sides_of(network_dir, STORE_FILES[0]) {
            fs::rename(&side, dir.join(file_name_of(&side))).map_err(fail)?;
        }
    }
    let pending = live_pending(network_dir).map_err(fail)?;
    add_to_mark(network_dir, &j.snapshot, &pending, now).map_err(fail)?;
    let after_dir = root.join(&j.after);
    let kept = after_dir.join(KEPT);
    let mut text = match fs::read_to_string(&kept) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(fail(e)),
    };
    text += &format!(
        "{}: kept the live files, side files included, that another build wrote after the \
         restore put the before-upgrade set back\n",
        fsx::utc_stamp(now)
    );
    fsx::write_durable(&kept, text.as_bytes()).map_err(fail)?;
    remove_staging(&root, j).map_err(fail)?;
    fsx::remove_durable(&root.join(RESTORE_REQUESTED)).map_err(fail)?;
    fsx::remove_durable(&root.join(RESTORE_JOURNAL)).map_err(fail)?;
    tracing::warn!("store upgrade: restore finished keeping what another build wrote");
    Ok(RestoreInfo {
        snapshot_dir: root.join(&j.snapshot),
        after_dir,
        during_dirs: Vec::new(),
        resumed: true,
        kept_live: true,
    })
}

/// The "keep the current files" action of an S4 refusal
/// ([`IncompleteCause::S4OtherLineage`], [`IncompleteCause::S4OldBuildWrote`]):
/// finishes the restore keeping whatever is live (Q92 S4-2). Only at S4,
/// before any store opens.
pub fn keep_current_files(network_dir: &Path, now: SystemTime) -> Result<RestoreInfo> {
    gates(network_dir)?;
    let (journal, snapshot) = read_journal(network_dir)?;
    let phase = wallet_phase(network_dir, &journal, &snapshot)
        .map_err(|e| incomplete(&e, "read the wallet"))?;
    if phase == WalletPhase::Incomplete {
        return Err(incomplete_other(
            "the wallet data has not been put back yet; restart to finish the restore".into(),
        ));
    }
    keep_live(network_dir, &journal, now)
}

/// Every path restore touches in a network directory, for support docs
/// (design §9).
pub fn touched_paths() -> Vec<String> {
    let b = UPGRADE_BACKUPS_DIR;
    let mut v = vec![
        format!("{b}/{RESTORE_REQUESTED}"),
        format!("{b}/{}-<UTC>/ (read)", snapshot::BEFORE_PREFIX),
    ];
    for name in STORE_FILES {
        v.push(format!("{name}, {name}-wal, {name}-shm, {name}-journal"));
    }
    v.extend([
        format!("{b}/{RESTORE_ATTEMPT}"),
        format!("{b}/{AFTER_PREFIX}-<UTC>/"),
        format!("{b}/{STAGING_PREFIX}-<UTC>/"),
        format!("{b}/{RESTORED_MARK}"),
        format!("{b}/{RESTORE_JOURNAL}"),
        format!("{b}/{DURING_PREFIX}-<UTC>/ with {KEPT} (resume only)"),
        format!("{b}/{AFTER_PREFIX}-<UTC>/{KEPT} (kept the current files only)"),
        format!("{b}/{READ_PREFIX}* (removed after reading)"),
        format!("{b}/<dir>/{SAFE_TO_DELETE} (failed cleanup only)"),
    ]);
    v
}
