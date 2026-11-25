//! Restore on restart (R3-RESTORE-DESIGN-v2.md rev 2.1, Q92 v2.1). The user's
//! request is a marker; the next start, before any store opens, copies the
//! live set into `after-upgrade-<UTC>/` and verifies it (D1), stages the
//! before set, writes the money-gate mark and the journal, then renames each
//! staged store over its live file, wallet.sqlite last (D2). A journal makes
//! every stop point resumable; files another build wrote in between are
//! copied into a new `during-restore-<UTC>/` before anything live changes
//! (V1). Once the wallet is replaced (S4), what another build wrote is kept
//! live. Before the journal, a failure removes only the dirs this attempt
//! recorded (92-4).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use dashcore::{OutPoint, Txid};

use crate::error::{Result, StoreUpgradeError};
use crate::fsx::{self, fault};
use crate::snapshot::{self, SIDE_SUFFIXES, STORE_FILES, SnapshotInfo, UPGRADE_BACKUPS_DIR};
use crate::sql::sha256_file;
use crate::token::PersisterOpened;

/// The user's request, written by [`request_restore`].
pub const RESTORE_REQUESTED: &str = "RESTORE-REQUESTED";
/// The journal: present from just before the first replace until the
/// restore is complete.
pub const RESTORE_JOURNAL: &str = "RESTORE-IN-PROGRESS";
/// The money gate's mark (M3-UX), with the before set's pending payments
/// (Q89 §3 option 2). Set before the first replace; cleared by the engine.
pub const RESTORED_MARK: &str = "RESTORED";
/// The paths an attempt creates before its journal (design §3).
pub const RESTORE_ATTEMPT: &str = "RESTORE-ATTEMPT";
/// Written into each dir of an attempt whose cleanup failed.
pub const SAFE_TO_DELETE: &str = "INCOMPLETE-SAFE-TO-DELETE.txt";
/// What a resume moved into its `during-restore-<UTC>/`, and why.
pub const KEPT: &str = "KEPT.txt";
pub(crate) const AFTER_PREFIX: &str = "after-upgrade";
pub(crate) const DURING_PREFIX: &str = "during-restore";
pub(crate) const STAGING_PREFIX: &str = "restore-staging";
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

/// What a resume does when the live wallet.sqlite is already the before
/// file and another build has written since (S4, design §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum S4Policy {
    /// Refuse with `RestoreIncomplete{S4OldBuildWrote}`; nothing moves.
    Refuse,
    /// Keep whatever is live, side files included; finish without moving.
    KeepLive,
}

/// The one S4 switch: KeepLive, the shipped default (Q92 v2.1, 92-1). Side
/// files are never moved at S4 in either mode. A live set outside the before
/// lineage is refused in both modes, with [`keep_current_files`] as the way
/// out.
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
    /// S4 under [`S4Policy::Refuse`]: the wallet was already replaced and
    /// another build wrote since.
    S4OldBuildWrote,
    /// S4 with a live set outside the before lineage (Q92 v2.1 S4-2):
    /// nothing moves; [`keep_current_files`] keeps it as it is.
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
            Self::S4OtherLineage => "the wallet data in place is not the restored wallet data",
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
    /// Files another build wrote or recreated mid-restore, if any.
    pub during_dirs: Vec<PathBuf>,
    /// Finished from a journal left by an earlier start.
    pub resumed: bool,
    /// S4 under [`S4Policy::KeepLive`]: finished keeping what was live.
    pub kept_live: bool,
}

/// One pending payment of the restored set: its txid and the wallet coins it
/// spends (Q89 §3 option 2: the engine holds these inputs).
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
        && valid_dir_name(&staging, STAGING_PREFIX)
        && valid_dir_name(&after, AFTER_PREFIX)
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

fn side_path(dir: &Path, name: &str, suffix: &str) -> PathBuf {
    dir.join(format!("{name}{suffix}"))
}

fn sides_of(network_dir: &Path, name: &str) -> Vec<PathBuf> {
    SIDE_SUFFIXES
        .iter()
        .map(|s| side_path(network_dir, name, s))
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
        let conn = crate::sql::open_existing(&path)
            .map_err(|e| stopped(format!("open {name} to quiesce: {e}")))?;
        let mut stmt = conn
            .prepare("PRAGMA wal_checkpoint(TRUNCATE)")
            .map_err(|e| stopped(format!("quiesce {name}: {e}")))?;
        let busy: i64 = stmt
            .query_row([], |r| r.get(0))
            .map_err(|e| stopped(format!("quiesce {name}: {e}")))?;
        drop(stmt);
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

fn partial_of(dest: &Path) -> PathBuf {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".partial");
    PathBuf::from(tmp)
}

/// Copy `src` to `dest` through `dest.partial`, owner-only, fsynced, checked
/// against `sha`. `Ok(false)` when the copy does not match (the partial is
/// removed).
fn verified_copy(src: &Path, dest: &Path, sha: &str) -> io::Result<bool> {
    let tmp = partial_of(dest);
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

/// The dirs a fresh attempt creates before its journal, recorded durably in
/// `RESTORE-ATTEMPT` before each is created (design §3), and whether it
/// created the mark, so a failure or a crash removes exactly these.
struct Attempt {
    root: PathBuf,
    created: Vec<String>,
    mark_created: bool,
}

const MARK_CREATED: &str = "mark-created";

/// The text of [`SAFE_TO_DELETE`] (MG, Q93 §2).
const SAFE_TO_DELETE_TEXT: &str = "This folder is an unfinished copy that Dash Wallet made while \
trying to restore wallet data, and it couldn't remove it.\nYour wallet files are elsewhere in this \
data folder, and this attempt didn't change them.\nYou can delete this folder. Don't delete \
anything else in upgrade-backups: the other folders may hold the only record of recent payments.\n";

/// `{prefix}-YYYYMMDDTHHMMSSZ`, with an optional `-N`: a name
/// [`fsx::fresh_name`] makes.
fn is_fresh_name(name: &str, prefix: &str) -> bool {
    let Some(rest) = name.strip_prefix(prefix).and_then(|r| r.strip_prefix('-')) else {
        return false;
    };
    let b = rest.as_bytes();
    if b.len() < 16 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let stamp = digits(0..8) && b[8] == b'T' && digits(9..15) && b[15] == b'Z';
    stamp
        && match &rest[16..] {
            "" => true,
            n => n
                .strip_prefix('-')
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit())),
        }
}

/// 92-4: a record entry is one of an attempt's own dirs, a single component
/// directly under `upgrade-backups/` in [`fsx::fresh_name`]'s pattern. That
/// rules out separators, `.` and `..`, absolute paths, `before-upgrade-*`,
/// `during-restore-*` and every marker name.
fn attempt_path_allowed(rel: &str) -> bool {
    is_fresh_name(rel, AFTER_PREFIX) || is_fresh_name(rel, STAGING_PREFIX)
}

/// Remove `path` if it is there and not a folder; a symlink is removed as a
/// link, never followed. ENOENT counts as removed (92-5).
fn remove_if_file(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => Err(io::Error::other(format!("{} is a folder", path.display()))),
        Ok(_) => match fs::remove_file(path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Remove one of an attempt's dirs: only the store copies and their partial
/// files in it, then the dir. A symlink or a non-folder is refused and left.
fn remove_attempt_dir(dir: &Path) -> io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => {}
        Ok(_) => {
            tracing::warn!(path = %dir.display(),
                "store upgrade: refused to remove a restore attempt entry that is not a plain folder");
            return Ok(());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    for name in STORE_FILES {
        let file = dir.join(name);
        remove_if_file(&partial_of(&file))?;
        remove_if_file(&file)?;
    }
    match fs::remove_dir(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

impl Attempt {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            created: Vec::new(),
            mark_created: false,
        }
    }

    /// Read the record a crashed attempt left. An entry 92-4 doesn't allow
    /// is refused and logged.
    fn left_over(root: &Path) -> Option<Self> {
        let text = fs::read_to_string(root.join(RESTORE_ATTEMPT)).ok()?;
        let mut attempt = Self::new(root);
        let mut lines = text.lines();
        if lines.next() != Some(ATTEMPT_HEADER) || lines.next() != Some("format 1") {
            tracing::warn!("store upgrade: refused an unreadable restore attempt record");
            return Some(attempt);
        }
        for line in lines {
            if line == MARK_CREATED {
                attempt.mark_created = true;
                continue;
            }
            match line.strip_prefix("path ") {
                Some(rel) if attempt_path_allowed(rel) => attempt.created.push(rel.to_string()),
                _ => tracing::warn!(
                    entry = line,
                    "store upgrade: refused a restore attempt record entry"
                ),
            }
        }
        Some(attempt)
    }

    fn save(&self) -> io::Result<()> {
        let mut text = format!("{ATTEMPT_HEADER}\nformat {FORMAT}\n");
        if self.mark_created {
            text += MARK_CREATED;
            text.push('\n');
        }
        for p in &self.created {
            text += &format!("path {p}\n");
        }
        fsx::write_durable(&self.root.join(RESTORE_ATTEMPT), text.as_bytes())
    }

    /// Record the dir `rel` (directly under `upgrade-backups/`) durably
    /// before it is created.
    fn will_create(&mut self, rel: String) -> io::Result<()> {
        debug_assert!(attempt_path_allowed(&rel), "{rel}");
        self.created.push(rel);
        self.save()
    }

    /// Record durably that this attempt is about to create the mark.
    fn will_create_mark(&mut self) -> io::Result<()> {
        self.mark_created = true;
        self.save()
    }

    /// Remove every recorded dir, newest first, the partial mark and journal
    /// files, a mark this attempt created, then the record. Once a journal
    /// exists nothing is removed but the record: its dirs are the journal's.
    fn cleanup(&self) -> io::Result<()> {
        if exists(&self.root.join(RESTORE_JOURNAL)) {
            tracing::warn!("store upgrade: a restore journal exists; left the attempt's folders");
            return fsx::remove_durable(&self.root.join(RESTORE_ATTEMPT));
        }
        for rel in self.created.iter().rev() {
            fault_cleanup()?;
            remove_attempt_dir(&self.root.join(rel))?;
        }
        for name in ["RESTORED.partial", "RESTORE-IN-PROGRESS.partial"] {
            remove_if_file(&self.root.join(name))?;
        }
        if self.mark_created {
            remove_if_file(&self.root.join(RESTORED_MARK))?;
        }
        fsx::fsync_dir(&self.root)?;
        fsx::remove_durable(&self.root.join(RESTORE_ATTEMPT))
    }

    /// The cleanup failed: mark each of this attempt's dirs safe to delete
    /// (never when a journal exists), then drop the record so no later run
    /// removes them.
    fn leave_safe_to_delete(&self, cause: &str) {
        if !exists(&self.root.join(RESTORE_JOURNAL)) {
            for rel in &self.created {
                let dir = self.root.join(rel);
                if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                let text = format!("{SAFE_TO_DELETE_TEXT}\ncause: {cause}\n");
                if let Err(e) = fsx::write_durable(&dir.join(SAFE_TO_DELETE), text.as_bytes()) {
                    tracing::warn!(dir = %dir.display(), error = %e,
                        "store upgrade: could not mark a partial restore copy safe to delete");
                }
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
        attempt.will_create(dir_name.clone()).map_err(io(what))?;
        fsx::create_private_dir(&dir).map_err(io(what))?;
        for e in &j.stores {
            let (src, sha) = match (staging, &e.before, &e.after) {
                (true, Some(sha), _) => (snapshot.dir().join(&e.name), sha),
                (false, _, Some(sha)) => (network_dir.join(&e.name), sha),
                _ => continue,
            };
            let dest = dir.join(&e.name);
            match verified_copy(&src, &dest, sha) {
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
    let marked = write_mark_for(network_dir, snapshot, now, Some(&mut attempt)).and_then(|()| {
        fsx::write_durable(
            &root.join(RESTORE_JOURNAL),
            render_journal(&journal).as_bytes(),
        )
    });
    if let Err(e) = marked {
        if exists(&root.join(RESTORE_JOURNAL)) {
            return Err(incomplete(&e, "write the restore journal"));
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

type Payments = Vec<(Vec<u8>, Vec<Vec<u8>>)>;

/// The before set's pending payments: unmined transactions that spend a
/// wallet coin, read immutable from the before set's wallet.sqlite.
fn pending_payments(wallet: &Path) -> rusqlite::Result<Payments> {
    if !wallet.exists() {
        return Ok(Vec::new());
    }
    pending_payments_in(&crate::sql::open_immutable(wallet)?)
}

fn pending_payments_in(conn: &rusqlite::Connection) -> rusqlite::Result<Payments> {
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
    let mut out: Payments = Vec::new();
    for row in rows {
        let (txid, outpoint) = row?;
        match out.last_mut() {
            Some((t, inputs)) if *t == txid => inputs.push(outpoint),
            _ => out.push((txid, vec![outpoint])),
        }
    }
    Ok(out)
}

/// S4-5: the live wallet's pending payments with its log applied. The main
/// file and its `-wal`/`-journal` are copied into a private scratch dir and
/// the copy is opened there; the live files are never opened or moved.
fn live_pending_payments(root: &Path, wallet: &Path) -> io::Result<Payments> {
    let scratch = tempfile::Builder::new()
        .prefix(".keep-read-")
        .tempdir_in(root)?;
    let name = file_name_of(wallet);
    let copy = scratch.path().join(&name);
    fs::copy(wallet, &copy)?;
    for suffix in ["-wal", "-journal"] {
        let side = side_path(wallet.parent().unwrap_or(root), &name, suffix);
        if exists(&side) {
            fs::copy(&side, side_path(scratch.path(), &name, suffix))?;
        }
    }
    let read = rusqlite::Connection::open(&copy).and_then(|c| pending_payments_in(&c));
    read.map_err(|e| io::Error::other(format!("read the live wallet's pending payments: {e}")))
}

/// An outpoint blob as platform-wallet-storage writes it (bincode, standard
/// config); trailing bytes are refused.
fn decode_outpoint(bytes: &[u8]) -> Option<OutPoint> {
    let (op, used) = bincode::serde::decode_from_slice(bytes, bincode::config::standard()).ok()?;
    (used == bytes.len()).then_some(op)
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

/// Step 6: the mark, listing the before set's pending payments.
fn write_mark_for(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    now: SystemTime,
    attempt: Option<&mut Attempt>,
) -> io::Result<()> {
    let payments = pending_payments(&snapshot.dir().join(STORE_FILES[0]))
        .map_err(|e| io::Error::other(format!("read the pending payments: {e}")))?;
    write_mark(network_dir, snapshot, now, payments, attempt)
}

/// Write the mark with `payments` added. An existing mark (an earlier
/// restore not yet synced, or this one) keeps its entries.
fn write_mark(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    now: SystemTime,
    payments: Payments,
    attempt: Option<&mut Attempt>,
) -> io::Result<()> {
    let root = root(network_dir);
    let path = root.join(RESTORED_MARK);
    let mut lines: Vec<String> = fs::read_to_string(&path)
        .map(|t| {
            t.lines()
                .filter(|l| l.starts_with("pending "))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if let (Some(attempt), false) = (attempt, exists(&path)) {
        attempt.will_create_mark()?;
    }
    for (txid, inputs) in payments {
        let inputs: Vec<String> = inputs.iter().map(|o| hex(o)).collect();
        let line = format!("pending {} {}", hex(&txid), inputs.join(","));
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    let mut text = format!(
        "{MARK_HEADER}\nformat {FORMAT}\nrestored {} from {}\n",
        fsx::utc_stamp(now),
        file_name_of(snapshot.dir())
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

/// The money gate's mark and the before set's pending payments, if set. An
/// entry that can't be read fails the call: the holds must never be
/// silently short.
pub fn restored_mark(network_dir: &Path) -> Result<Option<RestoredMark>> {
    let text = match fs::read_to_string(root(network_dir).join(RESTORED_MARK)) {
        Ok(t) => t,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StoreUpgradeError::io("read the restored mark", true)(e)),
    };
    let bad = |line: &str| StoreUpgradeError::SnapshotInvalid {
        dir: root(network_dir).join(RESTORED_MARK),
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
    if FAIL_CLEANUP.with(|f| f.get()) {
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
    /// The fail-first switch: skip moving side files aside.
    pub(crate) static SKIP_SIDE_MOVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The S4 switch, flipped by its own test.
    pub(crate) static S4_FOR_TEST: std::cell::Cell<Option<S4Policy>> =
        const { std::cell::Cell::new(None) };
    /// How long the last fresh run's replaces took.
    pub(crate) static LAST_REPLACE: std::cell::Cell<Option<std::time::Duration>> =
        const { std::cell::Cell::new(None) };
    /// The S4-5 fail-first switch: KeepLive moves the wallet's side files
    /// aside before reading.
    pub(crate) static MOVE_SIDES_ON_KEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Every attempt cleanup fails, alongside an armed fault point.
    pub(crate) static FAIL_CLEANUP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn skip_side_move() -> bool {
    #[cfg(test)]
    return SKIP_SIDE_MOVE.with(|s| s.get());
    #[cfg(not(test))]
    false
}

fn move_sides_on_keep() -> bool {
    #[cfg(test)]
    return MOVE_SIDES_ON_KEEP.with(|s| s.get());
    #[cfg(not(test))]
    false
}

/// This run's `during-restore-<UTC>/`, created on first use; each run gets
/// its own.
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

    fn dest_for(&mut self, path: &Path) -> io::Result<PathBuf> {
        let dir = self.dir()?;
        let name = file_name_of(path);
        let mut dest = dir.join(&name);
        let mut n = 2;
        while exists(&dest) || exists(&partial_of(&dest)) {
            dest = dir.join(format!("{name}.{n}"));
            n += 1;
        }
        Ok(dest)
    }

    /// Add `line` to `KEPT.txt`.
    fn note(&mut self, line: String) -> io::Result<()> {
        let dir = self.dir()?;
        self.kept.push(line);
        fsx::write_durable(&dir.join(KEPT), (self.kept.join("\n") + "\n").as_bytes())
    }

    /// V1 step 1: a verified copy of `path` in, unopened, with `why` in
    /// `KEPT.txt`. The original stays until the caller unlinks it.
    fn copy_in(&mut self, path: &Path, why: &str) -> io::Result<()> {
        let dest = self.dest_for(path)?;
        let sha = sha256_file(path)?;
        if !verified_copy(path, &dest, &sha)? {
            return Err(io::Error::other(format!(
                "{} changed while it was copied",
                file_name_of(path)
            )));
        }
        fsx::fsync_dir(dest.parent().unwrap_or(&self.root))?;
        self.note(format!("{}: {why}", file_name_of(&dest)))?;
        tracing::warn!(from = %path.display(), to = %dest.display(),
            "store upgrade: kept a copy of a file written mid-restore");
        fault_io("restore.settle")
    }
}

const WRITTEN_MID_RESTORE: &str = "written by another build while a restore was part-done";

/// The `KEPT.txt` line for S4 KeepLive (MG, Q92 §2 support doc).
const KEPT_LIVE: &str = "kept live: opened by another build after the swap finished";

/// The staged before copy of one store, verified, re-staged from the
/// snapshot when it is missing or doesn't match.
fn ensure_staged(
    staging_dir: &Path,
    snapshot_dir: &Path,
    name: &str,
    sha: &str,
) -> io::Result<PathBuf> {
    let staged = staging_dir.join(name);
    if hash_opt(&staged)?.as_deref() == Some(sha) {
        return Ok(staged);
    }
    remove_if_file(&staged)?;
    fsx::ensure_private_dir(staging_dir)?;
    if !verified_copy(&snapshot_dir.join(name), &staged, sha)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{name} in the before-upgrade backup no longer matches"),
        ));
    }
    fsx::fsync_dir(staging_dir)?;
    Ok(staged)
}

/// Resume, incomplete phase (design §5, V1 copy-first): bring one store to
/// its before file from whatever is there, never opening it. The staged copy
/// is verified first; anything another build wrote (side files, or a main
/// file that is neither the before nor the after file) is copied into a
/// `during-restore-*` dir and verified; only then are the live side files
/// unlinked and the staged file renamed over the live one. A live main file
/// is in place at every step.
fn settle(
    network_dir: &Path,
    j: &Journal,
    snapshot_dir: &Path,
    e: &StoreEntry,
    during: &mut During,
) -> io::Result<()> {
    let live = network_dir.join(&e.name);
    let sides = if skip_side_move() {
        Vec::new()
    } else {
        sides_of(network_dir, &e.name)
    };
    let found = hash_opt(&live)?;
    let at_before = found.is_some() && found == e.before;
    let at_after = found.is_some() && found == e.after;
    let staged = match (&e.before, at_before) {
        (Some(sha), false) => Some(ensure_staged(
            &root(network_dir).join(&j.staging),
            snapshot_dir,
            &e.name,
            sha,
        )?),
        _ => None,
    };
    if found.is_some() && (!sides.is_empty() || (!at_before && !at_after)) {
        during.copy_in(&live, WRITTEN_MID_RESTORE)?;
    }
    for side in &sides {
        during.copy_in(side, WRITTEN_MID_RESTORE)?;
    }
    if !sides.is_empty() {
        for side in &sides {
            fault_rename()?;
            remove_if_file(side)?;
        }
        fsx::fsync_dir(network_dir)?;
    }
    match (staged, found) {
        (Some(staged), _) => {
            fault_rename()?;
            fs::rename(&staged, &live)?;
        }
        (None, Some(_)) if e.before.is_none() => {
            fault_rename()?;
            fs::remove_file(&live)?;
        }
        _ => return Ok(()),
    }
    fsx::fsync_dir(network_dir)?;
    fault_replaced()
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
        if live != e.before || (side_left && !skip_side_move()) {
            return Err(incomplete_other(format!(
                "{} is not the before-upgrade file",
                e.name
            )));
        }
    }
    for dir in [network_dir, &after_dir, &root] {
        fsx::fsync_dir(dir).map_err(fail)?;
    }
    let staging = root.join(&j.staging);
    if staging.exists() {
        for e in &j.stores {
            let staged = staging.join(&e.name);
            if e.before.is_some() && hash_opt(&staged).map_err(fail)? == e.before {
                fs::remove_file(&staged).map_err(fail)?;
            }
        }
        fs::remove_dir(&staging).map_err(fail)?;
        fsx::fsync_dir(&root).map_err(fail)?;
    }
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

/// Where a resume stands against S4 (design §4, Q92 v2.1 S4-2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum S4 {
    /// The wallet is not replaced yet: the incomplete phase runs.
    No,
    /// Replaced and untouched since: the normal finish.
    Clean,
    /// Replaced, and another build wrote within the before lineage.
    InLineage,
    /// Replaced, and the live set is not in the before lineage.
    OtherLineage,
}

fn refinery_max_of(path: &Path) -> rusqlite::Result<i64> {
    crate::sql::open_immutable(path)?.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM refinery_schema_history",
        [],
        |r| r.get(0),
    )
}

fn wallet_ids_of(path: &Path) -> rusqlite::Result<Vec<Vec<u8>>> {
    let conn = crate::sql::open_immutable(path)?;
    let mut stmt = conn.prepare("SELECT wallet_id FROM wallets ORDER BY wallet_id")?;
    stmt.query_map([], |r| r.get(0))?.collect()
}

/// S4 is the wallet's staged copy gone with a live wallet in place: the
/// replace renames the staged file atomically, so from then on whatever is
/// live came after the swap. Only main files are read, the wallet's
/// immutable; no side file is opened, replayed or moved. The live set is in
/// the before lineage when no store is at its after file and the wallet is
/// the before file, or has the before set's refinery version and `wallets`
/// ids.
fn s4_state(network_dir: &Path, j: &Journal, snapshot: &SnapshotInfo) -> io::Result<S4> {
    let Some(wallet) = j.stores.last() else {
        return Ok(S4::No);
    };
    let Some(before) = &wallet.before else {
        return Ok(S4::No);
    };
    if exists(&root(network_dir).join(&j.staging).join(&wallet.name)) {
        return Ok(S4::No);
    }
    let live = network_dir.join(&wallet.name);
    let Some(h) = hash_opt(&live)? else {
        return Ok(S4::No);
    };
    let mut untouched = true;
    let mut at_after = false;
    for e in &j.stores {
        let found = hash_opt(&network_dir.join(&e.name))?;
        untouched &= found == e.before && sides_of(network_dir, &e.name).is_empty();
        at_after |= found.is_some() && found == e.after && e.after != e.before;
    }
    if untouched {
        return Ok(S4::Clean);
    }
    if at_after {
        return Ok(S4::OtherLineage);
    }
    if &h == before {
        return Ok(S4::InLineage);
    }
    let before_wallet = snapshot.dir().join(&wallet.name);
    let same_ids = matches!(
        (wallet_ids_of(&live), wallet_ids_of(&before_wallet)),
        (Ok(a), Ok(b)) if a == b
    );
    let same_version = refinery_max_of(&live).ok() == Some(snapshot.refinery_max());
    Ok(if same_ids && same_version {
        S4::InLineage
    } else {
        S4::OtherLineage
    })
}

/// S4-3 KeepLive: what is live stays, side files included, unopened and
/// unmoved. The live wallet's pending payments, read with its log applied
/// (S4-5), join the mark; a `KEPT.txt` line records the keep; then the
/// request goes, then the journal. The mark stays.
fn keep_live(
    network_dir: &Path,
    j: &Journal,
    snapshot: &SnapshotInfo,
    now: SystemTime,
) -> Result<RestoreInfo> {
    let root = root(network_dir);
    let fail = |e: io::Error| incomplete(&e, "keep the wallet data in place");
    let mut during = During::new(&root, now);
    let wallet = network_dir.join(STORE_FILES[0]);
    if move_sides_on_keep() {
        for side in sides_of(network_dir, STORE_FILES[0]) {
            let dest = during.dest_for(&side).map_err(fail)?;
            fs::rename(&side, dest).map_err(fail)?;
        }
    }
    let payments = live_pending_payments(&root, &wallet).map_err(fail)?;
    write_mark(network_dir, snapshot, now, payments, None).map_err(fail)?;
    during.note(KEPT_LIVE.into()).map_err(fail)?;
    for e in &j.stores {
        let sha = hash_opt(&network_dir.join(&e.name)).map_err(fail)?;
        let sides: Vec<String> = sides_of(network_dir, &e.name)
            .iter()
            .map(|p| file_name_of(p))
            .collect();
        during
            .note(format!(
                "{}: sha256 {}, side files {}",
                e.name,
                sha.as_deref().unwrap_or("absent"),
                if sides.is_empty() {
                    "none".into()
                } else {
                    sides.join(" ")
                }
            ))
            .map_err(fail)?;
    }
    fsx::remove_durable(&root.join(RESTORE_REQUESTED)).map_err(fail)?;
    fsx::remove_durable(&root.join(RESTORE_JOURNAL)).map_err(fail)?;
    tracing::warn!("store upgrade: restore finished keeping what another build wrote");
    Ok(RestoreInfo {
        snapshot_dir: root.join(&j.snapshot),
        after_dir: root.join(&j.after),
        during_dirs: during.dir.into_iter().collect(),
        resumed: true,
        kept_live: true,
    })
}

/// Read and check the journal, its after-upgrade set and its snapshot, and
/// make sure the mark is set.
fn load_journal(network_dir: &Path, now: SystemTime) -> Result<(Journal, SnapshotInfo)> {
    let root = root(network_dir);
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
    if !restored_mark_present(network_dir) {
        write_mark_for(network_dir, &snapshot, now, None)
            .map_err(|e| incomplete(&e, "write the restored mark"))?;
    }
    Ok((journal, snapshot))
}

/// Resume from the journal, before any store opens (design §5).
fn resume(network_dir: &Path, now: SystemTime) -> Result<RestoreInfo> {
    let root = root(network_dir);
    if let Err(e) = fsx::remove_durable(&root.join(RESTORE_ATTEMPT)) {
        tracing::warn!(error = %e, "store upgrade: could not remove a stale attempt record");
    }
    let (journal, snapshot) = load_journal(network_dir, now)?;
    match s4_state(network_dir, &journal, &snapshot)
        .map_err(|e| incomplete(&e, "read the wallet"))?
    {
        S4::Clean => return finish(network_dir, &journal, Vec::new(), true),
        S4::InLineage => {
            return match s4_policy() {
                S4Policy::Refuse => Err(StoreUpgradeError::RestoreIncomplete {
                    cause: IncompleteCause::S4OldBuildWrote,
                    journal_written: true,
                    detail: "the wallet data was already restored and another build wrote to it"
                        .into(),
                }),
                S4Policy::KeepLive => keep_live(network_dir, &journal, &snapshot, now),
            };
        }
        S4::OtherLineage => {
            return Err(StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::S4OtherLineage,
                journal_written: true,
                detail: "the wallet data in place is not the restored wallet data; nothing was \
                         changed"
                    .into(),
            });
        }
        S4::No => {}
    }
    let snapshot_dir = root.join(&journal.snapshot);
    let mut during = During::new(&root, now);
    for e in &journal.stores {
        settle(network_dir, &journal, &snapshot_dir, e, &mut during)
            .map_err(|err| incomplete(&err, "finish the restore"))?;
    }
    finish(
        network_dir,
        &journal,
        during.dir.into_iter().collect(),
        true,
    )
}

/// "Keep the current files" (MG Q93 §1): from an S4 refusal, keep what is
/// live exactly as KeepLive does (S4-3), whatever its lineage. `Ok(None)`
/// when no restore is part-done or it is not at S4: nothing is kept, and the
/// next start resumes as before.
pub fn keep_current_files(network_dir: &Path, now: SystemTime) -> Result<Option<RestoreInfo>> {
    gates(network_dir)?;
    let pending = pending_restore(network_dir)
        .map_err(StoreUpgradeError::io("read the restore marker", true))?;
    if pending != Some(PendingRestore::InProgress) {
        return Ok(None);
    }
    let (journal, snapshot) = load_journal(network_dir, now)?;
    match s4_state(network_dir, &journal, &snapshot)
        .map_err(|e| incomplete(&e, "read the wallet"))?
    {
        S4::No => Ok(None),
        S4::Clean => finish(network_dir, &journal, Vec::new(), true).map(Some),
        S4::InLineage | S4::OtherLineage => {
            keep_live(network_dir, &journal, &snapshot, now).map(Some)
        }
    }
}

/// Every path restore touches in a network directory, in order, for support
/// docs (design §9).
pub fn touched_paths() -> Vec<String> {
    let b = UPGRADE_BACKUPS_DIR;
    let mut v = vec![format!("{b}/{RESTORE_REQUESTED}")];
    v.push(format!("{b}/{}-<UTC>/ (read)", snapshot::BEFORE_PREFIX));
    for name in STORE_FILES {
        v.push(format!(
            "{name}, {name}-wal, {name}-shm, {name}-journal (quiesce)"
        ));
    }
    v.push(format!("{b}/{RESTORE_ATTEMPT}"));
    v.push(format!("{b}/{AFTER_PREFIX}-<UTC>/"));
    v.push(format!("{b}/{STAGING_PREFIX}-<UTC>/"));
    v.push(format!("{b}/{RESTORED_MARK}"));
    v.push(format!("{b}/{RESTORE_JOURNAL}"));
    v.push(format!("{b}/{DURING_PREFIX}-<UTC>/{KEPT} (resume only)"));
    v.push(format!(
        "{b}/{AFTER_PREFIX}-<UTC>/{SAFE_TO_DELETE}, {b}/{STAGING_PREFIX}-<UTC>/{SAFE_TO_DELETE} (failed cleanup only)"
    ));
    v
}
