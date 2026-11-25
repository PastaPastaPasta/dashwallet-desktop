//! The coordinated pre-upgrade snapshot (Q78 R3): the four stores copied with
//! SQLite's online backup API into `{folder}/upgrade-backups/before-upgrade-<UTC>/`,
//! a marker written last. Never under upstream's `backups/auto` (which its
//! retention prunes), never deleted.

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
/// wallet before the upgrade: the snapshot is retired, never read as a
/// backup again, and never deleted.
pub const NOT_THIS_WALLET: &str = "NOT-THIS-WALLET";
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

/// Why a snapshot directory doesn't check out.
enum SnapshotFault {
    /// Reading it failed (`EIO`, `EMFILE`, `EACCES`): retried, never taken
    /// for a defect of the copy.
    Io(io::Error),
    /// A marker or file is missing or is not what the marker says.
    Defect(String),
}

fn snapshot_fault(what: impl std::fmt::Display) -> impl FnOnce(io::Error) -> SnapshotFault {
    move |e| match e.kind() {
        io::ErrorKind::NotFound
        | io::ErrorKind::InvalidData
        | io::ErrorKind::IsADirectory
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::InvalidFilename => SnapshotFault::Defect(format!("{what}: {e}")),
        kind => SnapshotFault::Io(io::Error::new(kind, format!("{what}: {e}"))),
    }
}

fn check_snapshot(
    dir: &Path,
    verify_hashes: bool,
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
            return Err(SnapshotFault::Defect(
                "retired: not of this wallet before the upgrade".into(),
            ));
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
    Ok(info)
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
/// newest verified before-upgrade snapshot lists it, or a `part-upgraded`
/// copy with its marker that verifies does. A copy marked
/// [`NOT_THIS_WALLET`] never counts. Reads only.
pub fn upgrade_copy_present(network_dir: &Path, store: &str) -> io::Result<bool> {
    let lists = |info: &SnapshotInfo| info.files.iter().any(|f| f.name == store);
    if let SnapshotState::Verified(info) = snapshot_state(network_dir)?
        && lists(&info)
    {
        return Ok(true);
    }
    Ok(dirs_named(network_dir, UNRESTORABLE_PREFIX)?
        .iter()
        .any(|dir| dir.join(MARKER).exists() && read_snapshot(dir, true).is_ok_and(|i| lists(&i))))
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
pub(crate) fn live_state(
    wallet: &Connection,
    dir: &Path,
    open: impl Fn(&Path) -> rusqlite::Result<Connection>,
) -> rusqlite::Result<LiveState> {
    let refinery_max = if has_table(wallet, "refinery_schema_history")? {
        wallet.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let fork_versions = if has_table(wallet, fork::FORK_HISTORY)? {
        wallet
            .prepare(&format!("SELECT version FROM {}", fork::FORK_HISTORY))?
            .query_map([], |r| r.get::<_, u32>(0))?
            .collect::<rusqlite::Result<_>>()?
    } else {
        BTreeSet::new()
    };
    let between = !fork_versions.is_empty()
        && refinery_max <= fork::FORK_REFINERY_MAX
        && has_table(wallet, fork::CONTACTS)?
        && !sql::has_column(wallet, fork::CONTACTS, fork::EXTERNAL_REF)?;
    let wallet_fingerprint = fingerprint(wallet)?;
    let mut h = Sha256::new();
    h.update(format!("{}={wallet_fingerprint}\n", STORE_FILES[0]).as_bytes());
    for name in &STORE_FILES[1..] {
        let path = dir.join(name);
        let digest = if path.exists() {
            let conn = open(&path)?;
            let mut d = Sha256::new();
            for (table, digest) in sql::table_digests(&conn, &[])? {
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

/// A connection that reads a live store without changing it: with no log,
/// `immutable=1` (no lock, no `-wal`/`-shm` created); with a non-empty
/// `-wal` or `-journal`, read-only, never checkpointing (not even on close),
/// waiting at most `busy`. In that second case SQLite still opens `-shm`
/// read-write, creating it if missing, and a hot rollback journal fails as
/// `SQLITE_READONLY_ROLLBACK`; the stores run in WAL mode. The check for a
/// log and the open are not atomic: they rely on running under the instance
/// lock before any store opens, and M1 re-checks under the write lock.
fn open_source(path: &Path, busy: std::time::Duration) -> rusqlite::Result<Connection> {
    let logged = ["-wal", "-journal"].iter().any(|suffix| {
        let mut side = path.as_os_str().to_owned();
        side.push(suffix);
        fs::metadata(PathBuf::from(side)).is_ok_and(|m| m.len() > 0)
    });
    if !logged {
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
/// ([`reusable`]), read with `immutable=1`. `None` when that can't be read.
pub(crate) fn of_live(network_dir: &Path, info: &SnapshotInfo) -> Option<bool> {
    let conn = sql::open_immutable(&network_dir.join(STORE_FILES[0])).ok()?;
    let live = live_state(&conn, network_dir, sql::open_immutable).ok()?;
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
/// before-upgrade snapshot to keep. The start goes on all the same, with a
/// `part-upgraded` copy (Q107 107-1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissingSnapshot {
    /// No before-upgrade directory at all.
    Absent,
    /// Before-upgrade directories exist (one without its marker counts), and
    /// none verifies.
    Unverified,
    /// Verified ones exist, and none is of this wallet before the upgrade:
    /// each is marked [`NOT_THIS_WALLET`].
    Mismatch,
}

#[cfg(test)]
thread_local! {
    /// Fails every check of this snapshot directory with this I/O error.
    pub(crate) static SNAPSHOT_READ_FOR_TEST: std::cell::RefCell<Option<(PathBuf, io::ErrorKind)>> =
        const { std::cell::RefCell::new(None) };
}

/// Retire a verified before-upgrade snapshot that is not of this wallet:
/// [`NOT_THIS_WALLET`] written atomically into its directory, which is never
/// deleted. Never while a restore journal exists: the restore owns
/// `upgrade-backups/` until it is done.
fn mark_not_this_wallet(network_dir: &Path, dir: &Path, now: SystemTime) -> Result<()> {
    if fs::symlink_metadata(backups_dir(network_dir).join(crate::restore::RESTORE_JOURNAL)).is_ok()
    {
        return Err(StoreUpgradeError::RestorePending {
            dir: network_dir.to_path_buf(),
        });
    }
    let text = format!(
        "dw-store-upgrade not this wallet\nformat 1\nretired {}\n",
        fsx::utc_stamp(now)
    );
    tracing::warn!(dir = %dir.display(), "store upgrade: snapshot is not of this wallet; retired");
    fsx::write_durable(&dir.join(NOT_THIS_WALLET), text.as_bytes())
        .map_err(|e| failed(format!("mark {}: {e}", dir.display())))
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
/// start (Q107 107-1): a verified one from before the upgrade is kept, and
/// with none ([`MissingSnapshot`]) a verified `part-upgraded` copy is kept
/// or one is taken, never offered for restore. A directory without its
/// marker is then the true snapshot with its marker lost (the start that
/// took it checked the marker before the drop) and counts as `Unverified`;
/// it is never deleted. A verified one of other data is marked
/// [`NOT_THIS_WALLET`]. Only before a new snapshot of a store that is not
/// upgraded is a directory an interrupted run left without its marker
/// discarded and redone.
///
/// A fault reading a `before-upgrade` directory never hides an older one
/// that is this upgrade's. With none, it is retried once the store is
/// upgraded (that directory may be the true snapshot) and passed over
/// before.
pub fn take_snapshot(network_dir: &Path, now: SystemTime) -> Result<SnapshotInfo> {
    let mut missing = None;
    crate::token::refuse_if_held(network_dir)?;
    let wallet = network_dir.join(STORE_FILES[0]);
    let live = if wallet.exists() {
        let read = |e: rusqlite::Error| match e.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                StoreUpgradeError::StoreHeld {
                    path: network_dir.to_path_buf(),
                }
            }
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
        let mut state = MissingSnapshot::Absent;
        let mut fault = None;
        let mut others = Vec::new();
        for dir in list(BEFORE_PREFIX)?.iter().rev() {
            match inspect(dir, live) {
                Ok(Found::Reusable(info)) => return Ok(*info),
                Ok(Found::Other { marked }) => {
                    state = MissingSnapshot::Mismatch;
                    if !marked {
                        others.push(dir.clone());
                    }
                }
                Ok(Found::Defective) if state == MissingSnapshot::Absent => {
                    state = MissingSnapshot::Unverified;
                }
                Ok(Found::Defective) => {}
                Err(e) => {
                    fault.get_or_insert(e);
                }
            }
        }
        if live.between || live.refinery_max > fork::FORK_REFINERY_MAX {
            if let Some(e) = fault {
                return Err(e);
            }
            for dir in &others {
                mark_not_this_wallet(network_dir, dir, now)?;
            }
            for dir in list(UNRESTORABLE_PREFIX)?.iter().rev() {
                if let Ok(Found::Reusable(mut info)) = inspect(dir, live) {
                    info.missing = Some(state);
                    return Ok(*info);
                }
            }
            tracing::warn!(
                ?state,
                "store upgrade: upgraded store without a snapshot to keep"
            );
            prefix = UNRESTORABLE_PREFIX;
            missing = Some(state);
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
        let conn = sql::open_immutable(&dir.join(STORE_FILES[0]))
            .map_err(|e| failed(format!("reopen copy: {e}")))?;
        snapshot_state = Some(
            live_state(&conn, &dir, sql::open_immutable)
                .map_err(|e| failed(format!("read copy: {e}")))?,
        );
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
    };
    let text = render_marker(&info);
    info.manifest_sha256 = hex(&Sha256::digest(text.as_bytes()));
    fault("snapshot.before_marker")?;
    fsx::write_durable(&dir.join(MARKER), text.as_bytes())
        .map_err(|e| failed(format!("write marker: {e}")))?;
    fsx::fsync_dir(&root).map_err(|e| failed(format!("fsync {UPGRADE_BACKUPS_DIR}: {e}")))?;
    tracing::info!(dir = %dir.display(), "store upgrade: snapshot complete");
    Ok(info)
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

/// Busy or locked backup steps tolerated before the source counts as held.
const BUSY_STEPS: u32 = 100;

/// The online backup of one store into a single rollback-journal file,
/// integrity-checked and fsynced. A source kept busy past [`BUSY_STEPS`] is
/// [`StoreUpgradeError::StoreHeld`].
pub(crate) fn copy_store(src_path: &Path, dest: &Path) -> Result<()> {
    let name = src_path.file_name().unwrap_or_default().to_string_lossy();
    let fail = |e: &dyn std::fmt::Display| failed(format!("copy {name}: {e}"));
    let src = open_source(src_path, std::time::Duration::ZERO).map_err(|e| fail(&e))?;
    let mut dst = Connection::open(dest).map_err(|e| fail(&e))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst).map_err(|e| fail(&e))?;
        let mut busy = 0;
        loop {
            match backup.step(-1).map_err(|e| fail(&e))? {
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
    if !sql::integrity_ok(&dst).map_err(|e| fail(&e))? {
        return Err(fail(&"integrity_check of the copy failed"));
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
