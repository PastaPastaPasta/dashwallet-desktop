//! The coordinated pre-upgrade snapshot (Q78 R3): the four stores copied with
//! SQLite's online backup API into `{folder}/upgrade-backups/before-upgrade-<UTC>/`,
//! a marker written last. Never under upstream's `backups/auto` (which its
//! retention prunes), never deleted.

use std::borrow::Borrow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};

use crate::error::{Result, StoreUpgradeError};
use crate::fork;
use crate::fsx::{self, fault};
use crate::sql::{self, has_table, hex, sha256_file};

/// The four stores of a network directory, in snapshot and restore order.
pub const STORE_FILES: [&str; 4] = [
    "wallet.sqlite",
    "shielded.sqlite",
    "dispatch.sqlite",
    "app.sqlite",
];

/// The folder under `{folder}` holding every upgrade copy. A literal,
/// untranslated name: user copy names it (M3-UX (e)).
pub const UPGRADE_BACKUPS_DIR: &str = "upgrade-backups";
/// SQLite's side files of a store.
pub(crate) const SIDE_SUFFIXES: [&str; 3] = ["-wal", "-shm", "-journal"];
pub(crate) const BEFORE_PREFIX: &str = "before-upgrade";
/// A copy of an upgraded store taken when no before-upgrade snapshot is
/// kept ([`MissingSnapshot`]): never listed as a backup, never restored.
pub(crate) const UNRESTORABLE_PREFIX: &str = "part-upgraded";
pub(crate) const MARKER: &str = "SNAPSHOT-COMPLETE";
/// Written into a before-upgrade snapshot that verifies but is not of this
/// wallet before the upgrade, or when a wallet it contains is removed. A
/// retired snapshot is never read as a backup again or deleted.
pub const NOT_THIS_WALLET: &str = "NOT-THIS-WALLET";
const REMOVED_WALLET_MARK: &[u8] = b"dw-store-upgrade retired for wallet removal\nformat 1\n";
/// Written in `upgrade-backups/` by R1 under its lock, before the drop: the
/// before-upgrade snapshot the file is upgraded from (its directory name and
/// manifest sha256) and the file's `wallets.wallet_id` set, or `none`. Once
/// the file is upgraded, a before-upgrade snapshot is its backup only if
/// this names it and the live file holds exactly those wallets.
pub const UPGRADED_FROM: &str = "UPGRADED-FROM";
const UPGRADED_FROM_HEADER: &str = "dw-store-upgrade upgraded from";
/// Written in `upgrade-backups/` by the start that takes a `part-upgraded`
/// copy of an upgraded store: the copy's directory name and manifest
/// sha256. The only `part-upgraded` copy kept or counted
/// ([`upgrade_copy_present`]); one copied in never is.
pub const PART_UPGRADED_BY: &str = "PART-UPGRADED-BY";
const PART_UPGRADED_BY_HEADER: &str = "dw-store-upgrade part-upgraded copy";
const MARKER_HEADER: &str = "dw-store-upgrade snapshot";
const MARKER_FORMAT: u32 = 1;

/// One store in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// A completed snapshot, as its marker records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotInfo {
    dir: PathBuf,
    created: String,
    build: String,
    refinery_max: i64,
    fork_versions: BTreeSet<u32>,
    fingerprint: String,
    files: Vec<SnapshotFile>,
    absent: Vec<String>,
    manifest_sha256: String,
    missing: Option<MissingSnapshot>,
    between: bool,
}

impl SnapshotInfo {
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    /// The UTC stamp the snapshot was taken at.
    pub fn created(&self) -> &str {
        &self.created
    }
    /// The build that took it.
    pub fn build(&self) -> &str {
        &self.build
    }
    pub fn files(&self) -> &[SnapshotFile] {
        &self.files
    }
    /// Stores the network directory did not have.
    pub fn absent(&self) -> &[String] {
        &self.absent
    }
    pub fn fork_versions(&self) -> &BTreeSet<u32> {
        &self.fork_versions
    }
    /// The highest refinery version in the before set's wallet.sqlite.
    pub(crate) fn refinery_max(&self) -> i64 {
        self.refinery_max
    }

    /// For the `part-upgraded` copy [`take_snapshot`] returns for an upgraded
    /// store: why no before-upgrade snapshot was kept. Not in the marker.
    pub fn missing(&self) -> Option<MissingSnapshot> {
        self.missing
    }
    /// Taken or kept in the between state (R1's drop committed, upstream
    /// not yet migrated): the only state that judges the before-upgrade
    /// snapshot, so the only one whose [`Self::missing`] gets a notice.
    pub(crate) fn between(&self) -> bool {
        self.between
    }
    /// sha256 of the marker file: names this exact snapshot.
    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }
}

pub(crate) fn backups_dir(network_dir: &Path) -> PathBuf {
    network_dir.join(UPGRADE_BACKUPS_DIR)
}

fn render_marker(info: &SnapshotInfo) -> String {
    let mut s = format!("{MARKER_HEADER}\nformat {MARKER_FORMAT}\n");
    s += &format!("created {}\nbuild {}\n", info.created, info.build);
    s += &format!("refinery-max {}\n", info.refinery_max);
    let versions: Vec<String> = info.fork_versions.iter().map(u32::to_string).collect();
    s += &format!(
        "fork-versions {}\n",
        if versions.is_empty() {
            "-".into()
        } else {
            versions.join(",")
        }
    );
    s += &format!("fingerprint {}\n", info.fingerprint);
    for f in &info.files {
        s += &format!("file {} {} {}\n", f.name, f.bytes, f.sha256);
    }
    for a in &info.absent {
        s += &format!("absent {a}\n");
    }
    s
}

fn parse_marker(dir: &Path, text: &str) -> std::result::Result<SnapshotInfo, String> {
    let mut lines = text.lines();
    if lines.next() != Some(MARKER_HEADER) {
        return Err("not a snapshot marker".into());
    }
    let mut info = SnapshotInfo {
        dir: dir.to_path_buf(),
        created: String::new(),
        build: String::new(),
        refinery_max: -1,
        fork_versions: BTreeSet::new(),
        fingerprint: String::new(),
        files: Vec::new(),
        absent: Vec::new(),
        manifest_sha256: hex(&Sha256::digest(text.as_bytes())),
        missing: None,
        between: false,
    };
    let mut format = None;
    for line in lines {
        let (key, rest) = line
            .split_once(' ')
            .ok_or_else(|| format!("bad line {line:?}"))?;
        match key {
            "format" => format = rest.parse::<u32>().ok(),
            "created" => info.created = rest.into(),
            "build" => info.build = rest.into(),
            "refinery-max" => info.refinery_max = rest.parse().map_err(|_| "bad refinery-max")?,
            "fork-versions" if rest == "-" => {}
            "fork-versions" => {
                info.fork_versions = rest
                    .split(',')
                    .map(|v| v.parse::<u32>().map_err(|_| "bad fork version".to_string()))
                    .collect::<std::result::Result<_, _>>()?;
            }
            "fingerprint" => info.fingerprint = rest.into(),
            "file" => {
                let parts: Vec<&str> = rest.split(' ').collect();
                let [name, bytes, sha] = parts[..] else {
                    return Err(format!("bad file line {line:?}"));
                };
                info.files.push(SnapshotFile {
                    name: name.into(),
                    bytes: bytes.parse().map_err(|_| "bad size")?,
                    sha256: sha.into(),
                });
            }
            "absent" => info.absent.push(rest.into()),
            other => return Err(format!("unknown key {other}")),
        }
    }
    if format != Some(MARKER_FORMAT) {
        return Err(format!(
            "marker format {format:?}, this build reads {MARKER_FORMAT}"
        ));
    }
    let named: BTreeSet<&str> = info
        .files
        .iter()
        .map(|f| f.name.as_str())
        .chain(info.absent.iter().map(String::as_str))
        .collect();
    let want: BTreeSet<&str> = STORE_FILES.iter().copied().collect();
    if named != want || info.files.len() + info.absent.len() != STORE_FILES.len() {
        return Err("marker does not name each store exactly once".into());
    }
    if info.created.is_empty() || info.fingerprint.is_empty() || info.refinery_max < 0 {
        return Err("marker is missing a field".into());
    }
    Ok(info)
}

/// Read and check a snapshot directory's marker. `verify_hashes` re-hashes
/// every file.
pub(crate) fn read_snapshot(
    dir: &Path,
    verify_hashes: bool,
) -> std::result::Result<SnapshotInfo, String> {
    check_snapshot(dir, verify_hashes).map_err(|f| match f {
        SnapshotFault::Io(e) => e.to_string(),
        SnapshotFault::Defect(detail) => detail,
    })
}

/// Read a retired snapshot for migration-only consumers. Retirement blocks
/// restore eligibility while its verified files remain usable for migration.
pub(crate) fn read_snapshot_for_migration(dir: &Path) -> std::result::Result<SnapshotInfo, String> {
    check_snapshot_with_retirement(dir, true, true).map_err(|f| match f {
        SnapshotFault::Io(e) => e.to_string(),
        SnapshotFault::Defect(detail) => detail,
    })
}

/// Why a snapshot directory doesn't check out.
pub(crate) enum SnapshotFault {
    /// Reading it failed for a reason that may pass (`EIO`, `EMFILE`,
    /// `EINTR`, `ENOSPC`): retried, never taken for a defect of the copy.
    Io(io::Error),
    /// A marker or file is missing, is not what the marker says, or can't
    /// be read for its permissions (`EACCES`, `EPERM`): never a backup,
    /// and never waited for.
    Defect(String),
}

fn snapshot_fault(what: impl std::fmt::Display) -> impl FnOnce(io::Error) -> SnapshotFault {
    move |e| match e.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::InvalidData
        | io::ErrorKind::IsADirectory
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::InvalidFilename => SnapshotFault::Defect(format!("{what}: {e}")),
        kind => SnapshotFault::Io(io::Error::new(kind, format!("{what}: {e}"))),
    }
}

pub(crate) fn check_snapshot(
    dir: &Path,
    verify_hashes: bool,
) -> std::result::Result<SnapshotInfo, SnapshotFault> {
    check_snapshot_with_retirement(dir, verify_hashes, false)
}

fn check_snapshot_with_retirement(
    dir: &Path,
    verify_hashes: bool,
    allow_retired: bool,
) -> std::result::Result<SnapshotInfo, SnapshotFault> {
    #[cfg(test)]
    if let Some(kind) = SNAPSHOT_READ_FOR_TEST.with(|k| {
        k.borrow()
            .as_ref()
            .and_then(|(d, kind)| (d == dir).then_some(*kind))
    }) {
        return Err(snapshot_fault("read marker")(io::Error::from(kind)));
    }
    match fs::symlink_metadata(dir.join(NOT_THIS_WALLET)) {
        Ok(_) => {
            if !allow_retired {
                return Err(SnapshotFault::Defect(
                    "retired: not of this wallet before the upgrade".into(),
                ));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(snapshot_fault(NOT_THIS_WALLET)(e)),
    }
    let text = fs::read_to_string(dir.join(MARKER)).map_err(snapshot_fault("read marker"))?;
    let info = parse_marker(dir, &text).map_err(SnapshotFault::Defect)?;
    for f in &info.files {
        let path = dir.join(&f.name);
        let len = fs::metadata(&path).map_err(snapshot_fault(&f.name))?.len();
        if len != f.bytes {
            return Err(SnapshotFault::Defect(format!(
                "{} is {len} bytes, marker says {}",
                f.name, f.bytes
            )));
        }
        if verify_hashes && sha256_file(&path).map_err(snapshot_fault(&f.name))? != f.sha256 {
            return Err(SnapshotFault::Defect(format!(
                "{} does not match its sha256",
                f.name
            )));
        }
    }
    for a in &info.absent {
        if dir.join(a).exists() {
            return Err(SnapshotFault::Defect(format!(
                "{a} is present but the marker says absent"
            )));
        }
    }
    if verify_hashes {
        require_lineage(dir, &info)?;
    }
    Ok(info)
}

/// What [`UPGRADED_FROM`] records.
#[derive(Debug, PartialEq, Eq)]
enum Lineage {
    /// The snapshot and the `wallets.wallet_id` set (hex, sorted) of the
    /// file R1 upgraded.
    Snapshot {
        name: String,
        manifest: String,
        wallets: BTreeSet<String>,
    },
    None,
}

fn lineage_path(network_dir: &Path) -> PathBuf {
    backups_dir(network_dir).join(UPGRADED_FROM)
}

/// Retires the named backup before deleting a wallet it contains. Older
/// builds already honor the retirement marker.
pub fn retire_snapshot_for_wallet_removal(network_dir: &Path, wallet: &[u8; 32]) -> Result<()> {
    use std::io::Write;

    crate::restore::refuse_if_restore_pending(network_dir)?;
    let failure =
        StoreUpgradeError::io("retire the upgrade backup before removing the wallet", true);
    fsx::check_private_dir(&backups_dir(network_dir)).map_err(&failure)?;
    let Some(Lineage::Snapshot { name, wallets, .. }) =
        read_lineage(network_dir).map_err(&failure)?
    else {
        return Ok(());
    };
    if !wallets.contains(&hex(wallet).to_ascii_uppercase()) {
        return Ok(());
    }
    let dir = backups_dir(network_dir).join(name);
    let meta = match fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            write_lineage(network_dir, None)?;
            return Ok(());
        }
        Err(e) => return Err(failure(e)),
    };
    if !meta.is_dir() {
        return Err(failure(io::Error::new(
            io::ErrorKind::InvalidData,
            "the named backup is not a directory",
        )));
    }
    fsx::check_private_dir(&dir).map_err(&failure)?;
    let path = dir.join(NOT_THIS_WALLET);
    match fs::symlink_metadata(&path) {
        Ok(_) => return fsx::fsync_dir(&dir).map_err(failure),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(failure(e)),
    }
    let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(&failure)?;
    temp.write_all(REMOVED_WALLET_MARK).map_err(&failure)?;
    temp.as_file().sync_all().map_err(&failure)?;
    temp.persist_noclobber(&path)
        .map_err(|e| failure(e.error))?;
    fsx::fsync_dir(&dir).map_err(failure)
}

/// Whether this network's named backup was retired by Remove wallet.
pub fn snapshot_retired_for_wallet_removal(network_dir: &Path) -> bool {
    use std::io::Read;

    let Ok(Some(Lineage::Snapshot { name, .. })) = read_lineage(network_dir) else {
        return false;
    };
    let path = backups_dir(network_dir).join(name).join(NOT_THIS_WALLET);
    let Ok(file) = dw_fs::open_owner_only(&path) else {
        return false;
    };
    let mut marker = Vec::new();
    file.take(REMOVED_WALLET_MARK.len() as u64 + 1)
        .read_to_end(&mut marker)
        .is_ok()
        && marker == REMOVED_WALLET_MARK
}

/// [`UPGRADED_FROM`], `None` when there is none. A record that doesn't
/// parse is `InvalidData`.
fn read_lineage(network_dir: &Path) -> io::Result<Option<Lineage>> {
    let text = match fs::read_to_string(lineage_path(network_dir)) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    parse_lineage(&text).map(Some).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{UPGRADED_FROM} garbled"),
        )
    })
}

fn parse_lineage(text: &str) -> Option<Lineage> {
    let mut lines = text.lines();
    if lines.next()? != UPGRADED_FROM_HEADER || lines.next()? != "format 1" {
        return None;
    }
    let record = match lines.next()?.strip_prefix("snapshot ")? {
        "none" => Lineage::None,
        rest => {
            let (name, manifest) = rest.split_once(' ')?;
            let wallets = match lines.next()?.strip_prefix("wallets ")? {
                "-" => BTreeSet::new(),
                ids => ids.split(',').map(str::to_string).collect(),
            };
            let ok = name.starts_with(&format!("{BEFORE_PREFIX}-"))
                && !name.contains(['/', '\\', ' '])
                && manifest.len() == 64
                && wallets
                    .iter()
                    .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_hexdigit()));
            if !ok {
                return None;
            }
            Lineage::Snapshot {
                name: name.into(),
                manifest: manifest.into(),
                wallets,
            }
        }
    };
    lines.next().is_none().then_some(record)
}

/// Write [`UPGRADED_FROM`]. Never while a restore is requested or part-done:
/// the restore reads the snapshot it names at every resume.
fn write_lineage(
    network_dir: &Path,
    named: Option<(&SnapshotInfo, &BTreeSet<String>)>,
) -> Result<()> {
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let mut text = format!("{UPGRADED_FROM_HEADER}\nformat 1\n");
    match named {
        Some((info, wallets)) => {
            let ids: Vec<&str> = wallets.iter().map(String::as_str).collect();
            text += &format!(
                "snapshot {} {}\nwallets {}\n",
                name_of(&info.dir),
                info.manifest_sha256,
                if ids.is_empty() {
                    "-".into()
                } else {
                    ids.join(",")
                }
            );
        }
        None => text += "snapshot none\n",
    }
    fsx::write_durable(&lineage_path(network_dir), text.as_bytes())
        .map_err(StoreUpgradeError::io("record the upgrade snapshot", true))
}

/// R1, under its lock (`wallet` holds it) and before the drop: record
/// `snapshot` and the file's wallet ids as what it is upgraded from, or
/// `none` for a `part-upgraded` copy. A new before-upgrade snapshot starts a
/// new upgrade, so an earlier one's [`PART_UPGRADED_BY`] goes.
pub(crate) fn record_lineage(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    wallet: &Connection,
) -> Result<()> {
    if !restorable(snapshot) {
        return write_lineage(network_dir, None);
    }
    let wallets =
        wallet_ids(wallet).map_err(StoreUpgradeError::sqlite("read the wallet ids", true))?;
    write_lineage(network_dir, Some((snapshot, &wallets)))?;
    fsx::remove_durable(&backups_dir(network_dir).join(PART_UPGRADED_BY)).map_err(
        StoreUpgradeError::io("remove an earlier part-upgraded copy record", true),
    )
}

/// The `wallets.wallet_id` set, as upper-case hex.
fn wallet_ids(wallet: &Connection) -> rusqlite::Result<BTreeSet<String>> {
    if !has_table(wallet, "wallets")? {
        return Ok(BTreeSet::new());
    }
    wallet
        .prepare("SELECT hex(wallet_id) FROM wallets")?
        .query_map([], |r| r.get::<_, String>(0))?
        .collect()
}

fn names(record: &Lineage, info: &SnapshotInfo) -> bool {
    matches!(record, Lineage::Snapshot { name, manifest, .. }
        if info.dir.file_name().is_some_and(|n| n == name.as_str())
            && *manifest == info.manifest_sha256)
}

/// `network_dir`'s `wallet.sqlite` as the lineage gate reads it.
enum LiveWallet {
    Absent,
    /// It can't be read for what it holds (corrupt, not a database, or a log
    /// beside it unreadable for its permissions), so whether it is upgraded
    /// is not known. A file SQLite can't open at all is retried.
    Unreadable,
    /// Upgraded (R1's drop committed, or refinery past V018), with its
    /// wallet ids.
    Upgraded(BTreeSet<String>),
    NotUpgraded,
}

/// Read through [`open_copy`]. A fault that may pass (`SQLITE_IOERR`,
/// `SQLITE_FULL`, `SQLITE_CANTOPEN`, busy, an I/O error copying it) is
/// returned, never taken for an unreadable file.
fn live_wallet(network_dir: &Path) -> io::Result<LiveWallet> {
    let path = network_dir.join(STORE_FILES[0]);
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(LiveWallet::Absent),
        Err(e) => {
            return match snapshot_fault(STORE_FILES[0])(e) {
                SnapshotFault::Defect(_) => Ok(LiveWallet::Unreadable),
                SnapshotFault::Io(e) => Err(e),
            };
        }
        Ok(_) => {}
    }
    let read = open_copy(&path).and_then(|copy| {
        let conn: &Connection = copy.borrow();
        let (refinery_max, _, between) = upgrade_state(conn)?;
        if between || refinery_max > fork::FORK_REFINERY_MAX {
            wallet_ids(conn).map(LiveWallet::Upgraded)
        } else {
            Ok(LiveWallet::NotUpgraded)
        }
    });
    match read {
        Ok(live) => Ok(live),
        Err(e) if transient(&e) => Err(io::Error::other(format!("read {}: {e}", STORE_FILES[0]))),
        Err(_) => Ok(LiveWallet::Unreadable),
    }
}

/// An SQLite failure that may pass on a retry.
fn transient(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode as C;
    matches!(
        e.sqlite_error_code(),
        Some(
            C::SystemIoFailure
                | C::DiskFull
                | C::CannotOpen
                | C::DatabaseBusy
                | C::DatabaseLocked
                | C::OutOfMemory
                | C::OperationInterrupted
                | C::FileLockingProtocolFailed
        )
    )
}

/// A store read by [`open_copy`]; its scratch copy, if any, goes once the
/// connection has closed.
struct CopyRead {
    conn: Connection,
    _scratch: Option<tempfile::TempDir>,
}

impl Borrow<Connection> for CopyRead {
    fn borrow(&self) -> &Connection {
        &self.conn
    }
}

/// A live store read with its log applied and nothing written beside it:
/// with no log, `immutable=1`; with a non-empty `-wal` or `-journal`, the
/// file and its log copied into a private 0700 directory under the data
/// root (swept as precheck scratch) and opened there. [`open_live`] would
/// make or change `-shm` beside the live file, whoever owns its side files
/// then (a restore reads snapshots while it does). An I/O error copying is
/// `SQLITE_PERM` for its permissions, `SQLITE_FULL` for space and
/// `SQLITE_IOERR` otherwise.
fn open_copy(path: &Path) -> rusqlite::Result<CopyRead> {
    #[cfg(test)]
    fault("snapshot.open_copy").map_err(|_| {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_IOERR),
            Some("injected".into()),
        )
    })?;
    if !logged(path) {
        return Ok(CopyRead {
            conn: sql::open_immutable(path)?,
            _scratch: None,
        });
    }
    let ioerr = |e: io::Error| {
        let code = match e.kind() {
            io::ErrorKind::PermissionDenied => rusqlite::ffi::SQLITE_PERM,
            io::ErrorKind::StorageFull => rusqlite::ffi::SQLITE_FULL,
            _ => rusqlite::ffi::SQLITE_IOERR,
        };
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some(format!("copy {} to read it: {e}", path.display())),
        )
    };
    let root = crate::precheck::parent_dir(path)
        .and_then(crate::precheck::parent_dir)
        .map_err(|_| ioerr(io::Error::from(io::ErrorKind::NotFound)))?;
    let scratch = tempfile::Builder::new()
        .prefix(crate::precheck::SCRATCH_PREFIX)
        .tempdir_in(root)
        .map_err(ioerr)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700)).map_err(ioerr)?;
    }
    let name = path.file_name().unwrap_or_default();
    let copy = scratch.path().join(name);
    fs::copy(path, &copy).map_err(ioerr)?;
    for suffix in ["-wal", "-journal"] {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        let side = PathBuf::from(side);
        if side.exists() {
            let mut to = copy.as_os_str().to_owned();
            to.push(suffix);
            fs::copy(&side, PathBuf::from(to)).map_err(ioerr)?;
        }
    }
    Ok(CopyRead {
        conn: Connection::open(&copy)?,
        _scratch: Some(scratch),
    })
}

/// Once `wallet.sqlite` is upgraded, a before-upgrade snapshot is its backup
/// only if [`UPGRADED_FROM`] names it with this manifest and the live wallet
/// ids are exactly those R1 recorded: a restore replaces every store, so a
/// wallet added since would be lost and another wallet's file would be
/// rolled back to this one. With a record, a missing `wallet.sqlite` is held
/// to the named snapshot and one that can't be read proves nothing; with
/// none, either leaves the snapshot as it verified. A fault reading the live
/// file that may pass is [`SnapshotFault::Io`]. While a restore's journal
/// names this snapshot and manifest the gate is not applied: the restore
/// owns the stores, and the live file may be any it is replacing.
fn require_lineage(dir: &Path, info: &SnapshotInfo) -> std::result::Result<(), SnapshotFault> {
    let Some(network_dir) = dir
        .parent()
        .filter(|p| p.file_name().is_some_and(|n| n == UPGRADE_BACKUPS_DIR))
        .and_then(Path::parent)
    else {
        return Ok(());
    };
    if !restorable(info) || journal_names(network_dir, info) {
        return Ok(());
    }
    let live = live_wallet(network_dir).map_err(SnapshotFault::Io)?;
    if matches!(live, LiveWallet::NotUpgraded) {
        return Ok(());
    }
    let record = read_lineage(network_dir).map_err(snapshot_fault(UPGRADED_FROM))?;
    let held = match (live, &record) {
        (LiveWallet::Absent | LiveWallet::Unreadable, None) => true,
        (LiveWallet::Absent, Some(record)) => names(record, info),
        (LiveWallet::Upgraded(ids), Some(record)) => {
            names(record, info)
                && matches!(record, Lineage::Snapshot { wallets, .. } if *wallets == ids)
        }
        _ => false,
    };
    if held {
        Ok(())
    } else {
        Err(SnapshotFault::Defect(
            "not the snapshot this wallet was upgraded from".into(),
        ))
    }
}

/// Whether the restore journal (`RESTORE-IN-PROGRESS`) names `info`'s
/// directory and manifest.
fn journal_names(network_dir: &Path, info: &SnapshotInfo) -> bool {
    let Ok(text) =
        fs::read_to_string(backups_dir(network_dir).join(crate::restore::RESTORE_JOURNAL))
    else {
        return false;
    };
    let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(key));
    field("snapshot ") == Some(name_of(&info.dir).as_str())
        && field("manifest ") == Some(info.manifest_sha256.as_str())
}

/// Every `before-upgrade-*` directory, oldest name first.
fn before_dirs(network_dir: &Path) -> io::Result<Vec<PathBuf>> {
    dirs_named(network_dir, BEFORE_PREFIX)
}

/// Every `<prefix>-*` directory in `upgrade-backups/`, oldest name first.
fn dirs_named(network_dir: &Path, prefix: &str) -> io::Result<Vec<PathBuf>> {
    let root = backups_dir(network_dir);
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&format!("{prefix}-")) && entry.file_type()?.is_dir()
        {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// What [`snapshot_state`] found in `upgrade-backups/`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotState {
    /// The newest snapshot whose marker parses and whose every file matches
    /// its size and sha256: a backup.
    Verified(SnapshotInfo),
    /// A completed snapshot (its marker exists) that fails verification, and
    /// no verified one. Never described as a backup.
    Unverified { dir: PathBuf, detail: String },
    /// No completed snapshot.
    None,
}

/// The backup state of `network_dir`: a verified snapshot wins over an
/// unverified one; among each, the newest.
pub fn snapshot_state(network_dir: &Path) -> io::Result<SnapshotState> {
    let mut unverified = None;
    for dir in before_dirs(network_dir)?.into_iter().rev() {
        if !dir.join(MARKER).exists() {
            continue;
        }
        match read_snapshot(&dir, true) {
            Ok(info) => return Ok(SnapshotState::Verified(info)),
            Err(detail) => {
                unverified.get_or_insert(SnapshotState::Unverified { dir, detail });
            }
        }
    }
    Ok(unverified.unwrap_or(SnapshotState::None))
}

/// Whether a verified before-upgrade snapshot exists: only then does a
/// refusal screen offer Restore earlier data.
pub fn snapshot_present(network_dir: &Path) -> io::Result<bool> {
    Ok(matches!(
        snapshot_state(network_dir)?,
        SnapshotState::Verified(_)
    ))
}

/// Whether an upgrade copy of `store` (one of [`STORE_FILES`]) exists: the
/// newest verified before-upgrade snapshot lists it, or the `part-upgraded`
/// copy [`PART_UPGRADED_BY`] records verifies with that manifest and does.
/// A copy marked [`NOT_THIS_WALLET`] never counts. Reads only.
pub fn upgrade_copy_present(network_dir: &Path, store: &str) -> io::Result<bool> {
    let lists = |info: &SnapshotInfo| info.files.iter().any(|f| f.name == store);
    if let SnapshotState::Verified(info) = snapshot_state(network_dir)?
        && lists(&info)
    {
        return Ok(true);
    }
    if let Ok(Some(Lineage::Snapshot { name, manifest, .. })) = read_lineage(network_dir) {
        let dir = backups_dir(network_dir).join(name);
        if dir.join(MARKER).exists()
            && read_snapshot_for_migration(&dir)
                .is_ok_and(|i| i.manifest_sha256 == manifest && lists(&i))
        {
            return Ok(true);
        }
    }
    let Some((name, manifest)) = recorded_part(network_dir)? else {
        return Ok(false);
    };
    let dir = backups_dir(network_dir).join(name);
    Ok(dir.join(MARKER).exists()
        && read_snapshot(&dir, true).is_ok_and(|i| i.manifest_sha256 == manifest && lists(&i)))
}

/// The copy [`PART_UPGRADED_BY`] records: `None` with no record, or one
/// that doesn't parse or can't be read for its permissions. A fault that
/// may pass is returned.
fn recorded_part(network_dir: &Path) -> io::Result<Option<(String, String)>> {
    let text = match fs::read_to_string(backups_dir(network_dir).join(PART_UPGRADED_BY)) {
        Ok(text) => text,
        Err(e) => {
            return match snapshot_fault(PART_UPGRADED_BY)(e) {
                SnapshotFault::Defect(_) => Ok(None),
                SnapshotFault::Io(e) => Err(e),
            };
        }
    };
    let mut lines = text.lines();
    let record = (lines.next() == Some(PART_UPGRADED_BY_HEADER)
        && lines.next() == Some("format 1"))
    .then(|| lines.next()?.strip_prefix("copy ")?.split_once(' '))
    .flatten()
    .filter(|(name, manifest)| {
        name.starts_with(&format!("{UNRESTORABLE_PREFIX}-"))
            && !name.contains(['/', '\\', ' '])
            && manifest.len() == 64
    })
    .map(|(name, manifest)| (name.to_string(), manifest.to_string()));
    Ok(record.filter(|_| lines.next().is_none()))
}

/// Record `info`, a `part-upgraded` copy just taken and verified, as this
/// upgrade's. Never while a restore is requested or part-done.
pub(crate) fn record_part(network_dir: &Path, info: &SnapshotInfo) -> Result<()> {
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let text = format!(
        "{PART_UPGRADED_BY_HEADER}\nformat 1\ncopy {} {}\n",
        name_of(&info.dir),
        info.manifest_sha256
    );
    fsx::write_durable(
        &backups_dir(network_dir).join(PART_UPGRADED_BY),
        text.as_bytes(),
    )
    .map_err(StoreUpgradeError::io("record the part-upgraded copy", true))
}

/// The newest completed snapshot, its files checked against their sha256.
pub fn latest_snapshot(network_dir: &Path) -> Result<Option<SnapshotInfo>> {
    let dirs =
        before_dirs(network_dir).map_err(StoreUpgradeError::io("list upgrade backups", true))?;
    Ok(dirs.iter().rev().find_map(|d| read_snapshot(d, true).ok()))
}

/// What identifies the wallet data a snapshot was taken of.
pub(crate) struct LiveState {
    refinery_max: i64,
    fork_versions: BTreeSet<u32>,
    fingerprint: String,
    /// `wallet.sqlite`'s own part of `fingerprint`.
    wallet_fingerprint: String,
    /// R1's drop committed and upstream has not migrated yet: fork history
    /// present, the column gone, refinery not past the fork's last version.
    between: bool,
}

/// Table digests minus what the upgrade itself changes: the dropped column
/// and the history tables.
fn fingerprint(conn: &Connection) -> rusqlite::Result<String> {
    let digests = sql::table_digests(
        conn,
        &[
            (fork::CONTACTS, fork::EXTERNAL_REF),
            (fork::FORK_HISTORY, "*"),
            (fork::FORK_HISTORY_RETIRED, "*"),
            ("refinery_schema_history", "*"),
        ],
    )?;
    let mut h = Sha256::new();
    for (table, digest) in digests {
        h.update(format!("{table}={digest}\n").as_bytes());
    }
    Ok(hex(&h.finalize()))
}

/// The state of a directory's stores: `wallet` is a connection on its
/// `wallet.sqlite`, the other stores are opened with `open`. The
/// fingerprint covers all four (an absent store as `-`).
pub(crate) fn live_state<C: Borrow<Connection>>(
    wallet: &Connection,
    dir: &Path,
    open: impl Fn(&Path) -> rusqlite::Result<C>,
) -> rusqlite::Result<LiveState> {
    let (refinery_max, fork_versions, between) = upgrade_state(wallet)?;
    let wallet_fingerprint = fingerprint(wallet)?;
    let mut h = Sha256::new();
    h.update(format!("{}={wallet_fingerprint}\n", STORE_FILES[0]).as_bytes());
    for name in &STORE_FILES[1..] {
        let path = dir.join(name);
        let digest = if path.exists() {
            let conn = open(&path)?;
            let mut d = Sha256::new();
            for (table, digest) in sql::table_digests(conn.borrow(), &[])? {
                d.update(format!("{table}={digest}\n").as_bytes());
            }
            hex(&d.finalize())
        } else {
            "-".into()
        };
        h.update(format!("{name}={digest}\n").as_bytes());
    }
    Ok(LiveState {
        refinery_max,
        fork_versions,
        fingerprint: hex(&h.finalize()),
        wallet_fingerprint,
        between,
    })
}

/// A `wallet.sqlite`'s refinery `MAX(version)`, its fork versions (from the
/// retired history once retired) and whether it is between R1's drop and
/// upstream's migration.
fn upgrade_state(wallet: &Connection) -> rusqlite::Result<(i64, BTreeSet<u32>, bool)> {
    let refinery_max = if has_table(wallet, "refinery_schema_history")? {
        wallet.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let history = if has_table(wallet, fork::FORK_HISTORY)? {
        Some(fork::FORK_HISTORY)
    } else if has_table(wallet, fork::FORK_HISTORY_RETIRED)? {
        Some(fork::FORK_HISTORY_RETIRED)
    } else {
        None
    };
    let fork_versions = match history {
        Some(table) => wallet
            .prepare(&format!("SELECT version FROM {table}"))?
            .query_map([], |r| r.get::<_, u32>(0))?
            .collect::<rusqlite::Result<_>>()?,
        None => BTreeSet::new(),
    };
    let between = !fork_versions.is_empty()
        && refinery_max <= fork::FORK_REFINERY_MAX
        && has_table(wallet, fork::CONTACTS)?
        && !sql::has_column(wallet, fork::CONTACTS, fork::EXTERNAL_REF)?;
    Ok((refinery_max, fork_versions, between))
}

/// A connection that reads a live store without changing it: with no log,
/// `immutable=1` (no lock, no `-wal`/`-shm` created); with a non-empty
/// `-wal` or `-journal`, read-only, never checkpointing (not even on close),
/// waiting at most `busy`. In that second case SQLite still opens `-shm`
/// read-write, creating it if missing, and a hot rollback journal fails as
/// `SQLITE_READONLY_ROLLBACK`; the stores run in WAL mode. The check for a
/// log and the open are not atomic: they rely on running under the instance
/// lock before any store opens, and M1 re-checks under the write lock.
fn open_source(path: &Path, busy: std::time::Duration) -> rusqlite::Result<Connection> {
    if !logged(path) {
        return sql::open_immutable(path);
    }
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.set_db_config(
        rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
        true,
    )?;
    conn.busy_timeout(busy)?;
    Ok(conn)
}

/// A non-empty `-wal` or `-journal` beside `path`.
fn logged(path: &Path) -> bool {
    ["-wal", "-journal"].iter().any(|suffix| {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        fs::metadata(PathBuf::from(side)).is_ok_and(|m| m.len() > 0)
    })
}

fn open_live(path: &Path) -> rusqlite::Result<Connection> {
    open_source(path, std::time::Duration::from_secs(5))
}

/// M1: `snapshot` is of the data `wallet` (a connection holding the write
/// lock on `network_dir`'s `wallet.sqlite`) and the other stores hold now.
pub(crate) fn require_reusable(
    snapshot: &SnapshotInfo,
    wallet: &Connection,
    network_dir: &Path,
) -> Result<()> {
    let live = live_state(wallet, network_dir, open_live)
        .map_err(StoreUpgradeError::sqlite("read the live state", true))?;
    if reusable(snapshot, &live).map_err(StoreUpgradeError::sqlite("read the snapshot", true))? {
        return Ok(());
    }
    Err(StoreUpgradeError::SnapshotInvalid {
        dir: snapshot.dir().to_path_buf(),
        detail: "taken of other data than the stores hold now".into(),
    })
}

/// A completed snapshot is this upgrade's when the fork versions match and
/// either upstream has migrated since (the upgrade is in flight) or the data
/// is the same apart from what the upgrade changes. A `before-upgrade` one
/// must also be a file a released build opens ([`released`]): a
/// `part-upgraded` copy renamed to pose as one never is.
pub(crate) fn reusable(info: &SnapshotInfo, live: &LiveState) -> rusqlite::Result<bool> {
    if live.between {
        return reusable_between(info, live);
    }
    Ok(info.fork_versions == live.fork_versions
        && (live.refinery_max > info.refinery_max || live.fingerprint == info.fingerprint)
        && (!restorable(info) || released(info)?))
}

/// Between R1's drop and upstream's migration the other stores may have
/// changed (a start that stopped before the persister opened), but a snapshot
/// taken now would hold the file without the column, which no released build
/// opens. Only the true pre-upgrade snapshot will do: verified, the same fork
/// versions, its `wallet.sqlite` the live one apart from the column, and
/// itself [`released`]. A `part-upgraded` copy is matched on the wallet alone.
fn reusable_between(info: &SnapshotInfo, live: &LiveState) -> rusqlite::Result<bool> {
    Ok(info.fork_versions == live.fork_versions
        && (!restorable(info) || released(info)?)
        && fingerprint(&sql::open_immutable(&info.dir.join(STORE_FILES[0]))?)?
            == live.wallet_fingerprint)
}

/// Whether `info` is of `network_dir`'s data before the upgrade
/// ([`reusable`]), each store read by [`open_copy`] with its log applied.
/// `None` when that can't be read.
pub(crate) fn of_live(network_dir: &Path, info: &SnapshotInfo) -> Option<bool> {
    let wallet = open_copy(&network_dir.join(STORE_FILES[0])).ok()?;
    let live = live_state(wallet.borrow(), network_dir, open_copy).ok()?;
    reusable(info, &live).ok()
}

fn restorable(info: &SnapshotInfo) -> bool {
    info.dir.file_name().is_some_and(|n| {
        n.to_string_lossy()
            .starts_with(&format!("{BEFORE_PREFIX}-"))
    })
}

/// A copy a released build opens: refinery at most V018 and
/// `contacts.external_account_reference` present.
fn released(info: &SnapshotInfo) -> rusqlite::Result<bool> {
    Ok(info.refinery_max <= fork::FORK_REFINERY_MAX
        && sql::has_column(
            &sql::open_immutable(&info.dir.join(STORE_FILES[0]))?,
            fork::CONTACTS,
            fork::EXTERNAL_REF,
        )?)
}

/// Why an upgraded store (R1's drop committed, or upstream migrated) has no
/// before-upgrade snapshot to keep: the one [`UPGRADED_FROM`] names is the
/// only candidate. The start goes on all the same, with a `part-upgraded`
/// copy (Q107 107-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingSnapshot {
    /// No record, a record of `none`, or the named directory is gone.
    Absent,
    /// The named directory is there and doesn't verify (its marker lost, or
    /// live wallet ids other than those R1 recorded, count), or the record
    /// doesn't parse.
    Unverified,
    /// The named snapshot is marked [`NOT_THIS_WALLET`], or verifies and is
    /// not this upgrade's: judged against the data only in the between
    /// state, which marks it.
    Mismatch,
}

#[cfg(test)]
thread_local! {
    /// Fails every check of this snapshot directory with this I/O error.
    pub(crate) static SNAPSHOT_READ_FOR_TEST: std::cell::RefCell<Option<(PathBuf, io::ErrorKind)>> =
        const { std::cell::RefCell::new(None) };
}

/// Retire the named snapshot when it verifies and is not of this wallet:
/// [`NOT_THIS_WALLET`] written atomically into its directory, which is never
/// deleted. A directory that can't be written to (`EACCES`, `EPERM`,
/// `EROFS`) is left as it is and [`UPGRADED_FROM`] is rewritten as `none`,
/// which keeps it from being offered all the same; only a failure to write
/// that is retried. Never while a restore is requested or part-done: the
/// restore owns `upgrade-backups/` until it is done, and may name this
/// snapshot.
fn mark_not_this_wallet(network_dir: &Path, dir: &Path, now: SystemTime) -> Result<()> {
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let text = format!(
        "dw-store-upgrade not this wallet\nformat 1\nretired {}\n",
        fsx::utc_stamp(now)
    );
    match fsx::write_durable(&dir.join(NOT_THIS_WALLET), text.as_bytes()) {
        Ok(()) => {
            tracing::warn!(dir = %dir.display(), "store upgrade: snapshot is not of this wallet; retired");
            Ok(())
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            tracing::warn!(dir = %dir.display(), error = %e, "store upgrade: snapshot is not of this wallet and can't be marked; no longer named");
            write_lineage(network_dir, None)
        }
        Err(e) => Err(StoreUpgradeError::io("mark a snapshot of other data", true)(e)),
    }
}

fn failed(detail: impl std::fmt::Display) -> StoreUpgradeError {
    StoreUpgradeError::SnapshotFailed {
        detail: detail.to_string(),
    }
}

/// Take the pre-upgrade snapshot of `network_dir`, or return this upgrade's
/// completed one. The wallet files are not changed: the sources are read
/// through read-only connections that never checkpoint.
///
/// Once the store is upgraded (R1's drop committed, or refinery past V018)
/// no `before-upgrade` snapshot is taken and no snapshot reason refuses the
/// start (Q107 107-1). Only the snapshot [`UPGRADED_FROM`] names is looked
/// at; every other `before-upgrade` directory is left as it is. In the
/// between state the named one is kept if it is of the live data, and one
/// that verifies and isn't is marked [`NOT_THIS_WALLET`]. Once upstream has
/// migrated it is kept if it verifies, never judged against the data. With
/// none to keep ([`MissingSnapshot`]) a verified `part-upgraded` copy is
/// kept or one is taken, never offered for restore. The named directory
/// without its marker is the true snapshot with its marker lost and counts
/// as `Unverified`; it is never deleted. Only the `part-upgraded` copy
/// [`PART_UPGRADED_BY`] records is kept; any other with its marker is left
/// as it is. Before a new copy, a directory of its kind without its marker
/// (an interrupted run's) is discarded. A new copy is verified once taken:
/// one that doesn't verify is a retried
/// [`StoreUpgradeError::SnapshotFailed`] and no other is taken that start,
/// so faulty copies add up one per fault, never one per start. A
/// `part-upgraded` copy that verifies is then recorded.
///
/// Before the upgrade, a fault reading a `before-upgrade` directory never
/// hides an older one that is this upgrade's, and is passed over. Once
/// upgraded, a fault reading the named one is retried.
pub fn take_snapshot(network_dir: &Path, now: SystemTime) -> Result<SnapshotInfo> {
    let mut missing = None;
    let mut between = false;
    crate::token::refuse_if_held(network_dir)?;
    let wallet = network_dir.join(STORE_FILES[0]);
    let live = if wallet.exists() {
        let read = |e: rusqlite::Error| match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                StoreUpgradeError::StoreHeld {
                    path: network_dir.to_path_buf(),
                }
            }
            _ if damaged(&e) => StoreUpgradeError::SnapshotSourceDamaged {
                store: damaged_store(network_dir),
                detail: e.to_string(),
            },
            _ => failed(format!("read the live stores: {e}")),
        };
        let conn = open_live(&wallet).map_err(read)?;
        Some(live_state(&conn, network_dir, open_live).map_err(read)?)
    } else {
        None
    };
    let list = |prefix| {
        dirs_named(network_dir, prefix).map_err(|e| failed(format!("list snapshots: {e}")))
    };
    let mut prefix = BEFORE_PREFIX;
    if let Some(live) = &live {
        if live.between || live.refinery_max > fork::FORK_REFINERY_MAX {
            let state = match upgraded_from(network_dir, live, now)? {
                Ok(info) => return Ok(*info),
                Err(state) => state,
            };
            let recorded = recorded_part(network_dir).map_err(StoreUpgradeError::io(
                "read the part-upgraded copy record",
                true,
            ))?;
            if let Some((name, manifest)) = recorded
                && let Ok(Found::Reusable(mut info)) =
                    inspect(&backups_dir(network_dir).join(name), live)
                && info.manifest_sha256 == manifest
            {
                info.missing = Some(state);
                info.between = live.between;
                return Ok(*info);
            }
            tracing::warn!(
                ?state,
                "store upgrade: upgraded store without a snapshot to keep"
            );
            prefix = UNRESTORABLE_PREFIX;
            missing = Some(state);
            between = live.between;
        } else {
            for dir in list(BEFORE_PREFIX)?.iter().rev() {
                if let Ok(Found::Reusable(info)) = inspect(dir, live) {
                    return Ok(*info);
                }
            }
        }
    }
    for dir in list(prefix)? {
        if !dir.join(MARKER).exists() {
            tracing::warn!(dir = %dir.display(), "store upgrade: discarding an incomplete snapshot");
            fs::remove_dir_all(&dir)
                .map_err(|e| failed(format!("discard {}: {e}", dir.display())))?;
        }
    }
    let root = backups_dir(network_dir);
    fsx::ensure_private_dir(&root)
        .map_err(|e| failed(format!("create {UPGRADE_BACKUPS_DIR}: {e}")))?;
    let dir = root.join(fsx::fresh_name(&root, prefix, now));
    fsx::create_private_dir(&dir).map_err(|e| failed(format!("create {}: {e}", dir.display())))?;
    fsx::fsync_dir(&root).map_err(|e| failed(format!("fsync {UPGRADE_BACKUPS_DIR}: {e}")))?;
    let mut files = Vec::new();
    let mut absent = Vec::new();
    let mut snapshot_state = None;
    for name in STORE_FILES {
        let src_path = network_dir.join(name);
        if !src_path.exists() {
            absent.push(name.to_string());
            continue;
        }
        let dest = dir.join(name);
        copy_store(&src_path, &dest)?;
        files.push(SnapshotFile {
            name: name.into(),
            bytes: fs::metadata(&dest)
                .map_err(|e| failed(format!("{name}: {e}")))?
                .len(),
            sha256: sha256_file(&dest).map_err(|e| failed(format!("{name}: {e}")))?,
        });
        fault("snapshot.after_file")?;
    }
    if files.iter().any(|f| f.name == STORE_FILES[0]) {
        let copy = |e: rusqlite::Error| {
            if damaged(&e) {
                StoreUpgradeError::SnapshotSourceDamaged {
                    store: damaged_store(&dir),
                    detail: format!("its copy: {e}"),
                }
            } else {
                failed(format!("read copy: {e}"))
            }
        };
        #[cfg(test)]
        if fault("snapshot.copy_read").is_err() {
            let copied = dir.join(STORE_FILES[0]);
            let mut bytes = fs::read(&copied).unwrap();
            bytes[..100].fill(0x5a);
            fs::write(&copied, bytes).unwrap();
        }
        let conn = sql::open_immutable(&dir.join(STORE_FILES[0])).map_err(copy)?;
        snapshot_state = Some(live_state(&conn, &dir, sql::open_immutable).map_err(copy)?);
    }
    let state = snapshot_state.unwrap_or(LiveState {
        refinery_max: 0,
        fork_versions: BTreeSet::new(),
        fingerprint: "-".into(),
        wallet_fingerprint: "-".into(),
        between: false,
    });
    let mut info = SnapshotInfo {
        dir: dir.clone(),
        created: fsx::utc_stamp(now),
        build: env!("CARGO_PKG_VERSION").into(),
        refinery_max: state.refinery_max,
        fork_versions: state.fork_versions,
        fingerprint: state.fingerprint,
        files,
        absent,
        manifest_sha256: String::new(),
        missing,
        between,
    };
    let text = render_marker(&info);
    info.manifest_sha256 = hex(&Sha256::digest(text.as_bytes()));
    fault("snapshot.before_marker")?;
    fsx::write_durable(&dir.join(MARKER), text.as_bytes())
        .map_err(|e| failed(format!("write marker: {e}")))?;
    fsx::fsync_dir(&root).map_err(|e| failed(format!("fsync {UPGRADE_BACKUPS_DIR}: {e}")))?;
    #[cfg(test)]
    if fault("snapshot.corrupt_copy").is_err() {
        let first = dir.join(&info.files[0].name);
        let mut bytes = fs::read(&first).unwrap();
        bytes[100] ^= 1;
        fs::write(&first, bytes).unwrap();
    }
    match check_snapshot(&dir, true) {
        Ok(_) => {}
        Err(SnapshotFault::Defect(detail)) => {
            return Err(failed(format!(
                "{} does not verify once taken: {detail}",
                dir.display()
            )));
        }
        Err(SnapshotFault::Io(e)) => {
            return Err(StoreUpgradeError::io(
                "verify the new upgrade snapshot",
                true,
            )(e));
        }
    }
    if prefix == UNRESTORABLE_PREFIX {
        record_part(network_dir, &info)?;
    }
    tracing::info!(dir = %dir.display(), "store upgrade: snapshot complete");
    Ok(info)
}

/// The snapshot an upgraded store keeps: the one [`UPGRADED_FROM`] names,
/// or why there is none. Judged against the live data only in the between
/// state, where nothing but this crate wrote the wallet since R1 named it;
/// once upstream has migrated, the named one is kept if it verifies.
fn upgraded_from(
    network_dir: &Path,
    live: &LiveState,
    now: SystemTime,
) -> Result<std::result::Result<Box<SnapshotInfo>, MissingSnapshot>> {
    let retry = StoreUpgradeError::io("read the upgrade snapshot record", true);
    let name = match read_lineage(network_dir) {
        Ok(Some(Lineage::Snapshot { name, .. })) => name,
        Ok(Some(Lineage::None) | None) => return Ok(Err(MissingSnapshot::Absent)),
        Err(e) => {
            return match snapshot_fault(UPGRADED_FROM)(e) {
                SnapshotFault::Defect(_) => Ok(Err(MissingSnapshot::Unverified)),
                SnapshotFault::Io(e) => Err(retry(e)),
            };
        }
    };
    let dir = backups_dir(network_dir).join(name);
    match fs::symlink_metadata(&dir) {
        Ok(_) => {}
        Err(e) => {
            return match snapshot_fault(&name_of(&dir))(e) {
                SnapshotFault::Defect(_) => Ok(Err(MissingSnapshot::Absent)),
                SnapshotFault::Io(e) => Err(retry(e)),
            };
        }
    }
    Ok(match inspect(&dir, live)? {
        Found::Reusable(info) => Ok(info),
        Found::Defective => Err(MissingSnapshot::Unverified),
        Found::Other { marked: false } if live.between => {
            mark_not_this_wallet(network_dir, &dir, now)?;
            Err(MissingSnapshot::Mismatch)
        }
        Found::Other { .. } => Err(MissingSnapshot::Mismatch),
    })
}

fn name_of(dir: &Path) -> String {
    dir.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// What a snapshot directory is to the live stores.
enum Found {
    /// Verified, and this upgrade's ([`reusable`]).
    Reusable(Box<SnapshotInfo>),
    /// Verified, and of other data or not a file a released build opens;
    /// or already marked [`NOT_THIS_WALLET`].
    Other { marked: bool },
    /// No marker, or it fails verification.
    Defective,
}

/// Check `dir` against `live`. A fault reading it is a retryable
/// [`StoreUpgradeError::Io`] or [`StoreUpgradeError::Sqlite`]: it says
/// nothing about the copy.
fn inspect(dir: &Path, live: &LiveState) -> Result<Found> {
    if fs::symlink_metadata(dir.join(NOT_THIS_WALLET)).is_ok() {
        return Ok(Found::Other { marked: true });
    }
    let info = match check_snapshot(dir, true) {
        Ok(info) => info,
        Err(SnapshotFault::Defect(_)) => return Ok(Found::Defective),
        Err(SnapshotFault::Io(e)) => {
            return Err(StoreUpgradeError::io("verify an upgrade snapshot", true)(e));
        }
    };
    let reused = reusable(&info, live)
        .map_err(StoreUpgradeError::sqlite("read an upgrade snapshot", true))?;
    Ok(if reused {
        Found::Reusable(Box::new(info))
    } else {
        Found::Other { marked: false }
    })
}

/// SQLite read the store as corrupt or as not a database.
fn damaged(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

/// The first store in `dir` SQLite can't read whole, for
/// [`StoreUpgradeError::SnapshotSourceDamaged`].
fn damaged_store(dir: &Path) -> String {
    STORE_FILES
        .iter()
        .find(|name| {
            let path = dir.join(name);
            path.exists()
                && !open_source(&path, std::time::Duration::ZERO)
                    .and_then(|c| sql::integrity_ok(&c))
                    .unwrap_or(false)
        })
        .unwrap_or(&STORE_FILES[0])
        .to_string()
}

/// Busy or locked backup steps tolerated before the source counts as held.
const BUSY_STEPS: u32 = 100;

/// The online backup of one store into a single rollback-journal file,
/// integrity-checked and fsynced. A source kept busy past [`BUSY_STEPS`] is
/// [`StoreUpgradeError::StoreHeld`].
pub(crate) fn copy_store(src_path: &Path, dest: &Path) -> Result<()> {
    let name = src_path.file_name().unwrap_or_default().to_string_lossy();
    let fail = |e: &dyn std::fmt::Display| failed(format!("copy {name}: {e}"));
    let source_damaged = |detail: String| StoreUpgradeError::SnapshotSourceDamaged {
        store: name.to_string(),
        detail,
    };
    let read = |e: rusqlite::Error| {
        if damaged(&e) {
            source_damaged(e.to_string())
        } else {
            fail(&e)
        }
    };
    let src = open_source(src_path, std::time::Duration::ZERO).map_err(read)?;
    let mut dst = Connection::open(dest).map_err(|e| fail(&e))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst).map_err(read)?;
        let mut busy = 0;
        loop {
            match backup.step(-1).map_err(read)? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => continue,
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    busy += 1;
                    if busy >= BUSY_STEPS {
                        return Err(StoreUpgradeError::StoreHeld {
                            path: src_path.to_path_buf(),
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => return Err(fail(&"unexpected backup step")),
            }
        }
    }
    drop(src);
    let mode: String = dst
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .map_err(|e| fail(&e))?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(fail(&format!("copy journal mode is {mode}")));
    }
    let copied = match sql::integrity_ok(&dst) {
        Ok(ok) => ok,
        Err(e) if damaged(&e) => false,
        Err(e) => return Err(fail(&e)),
    };
    #[cfg(test)]
    let copied = copied && fault("copy.integrity").is_ok();
    if !copied {
        return Err(
            match open_source(src_path, std::time::Duration::ZERO)
                .and_then(|c| sql::integrity_ok(&c))
            {
                Ok(false) => {
                    source_damaged("integrity_check of the store and its copy failed".into())
                }
                Err(e) if damaged(&e) => source_damaged(e.to_string()),
                _ => fail(&"integrity_check of the copy failed"),
            },
        );
    }
    dst.close().map_err(|(_, e)| fail(&e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o600)).map_err(|e| fail(&e))?;
    }
    fsx::fsync_file(dest).map_err(|e| fail(&e))
}

/// Each completed snapshot's dir and marker sha256, for tests and the log.
#[cfg(test)]
pub(crate) fn completed(network_dir: &Path) -> BTreeMap<PathBuf, String> {
    before_dirs(network_dir)
        .unwrap()
        .into_iter()
        .filter_map(|d| {
            read_snapshot(&d, false)
                .ok()
                .map(|i| (d, i.manifest_sha256))
        })
        .collect()
}

#[cfg(not(test))]
#[allow(dead_code)]
fn _unused(_: BTreeMap<(), ()>) {}
