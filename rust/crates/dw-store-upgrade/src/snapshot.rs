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
pub(crate) const BEFORE_PREFIX: &str = "before-upgrade";
pub(crate) const MARKER: &str = "SNAPSHOT-COMPLETE";
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
    let text = fs::read_to_string(dir.join(MARKER)).map_err(|e| format!("read marker: {e}"))?;
    let info = parse_marker(dir, &text)?;
    for f in &info.files {
        let path = dir.join(&f.name);
        let len = fs::metadata(&path)
            .map_err(|e| format!("{}: {e}", f.name))?
            .len();
        if len != f.bytes {
            return Err(format!(
                "{} is {len} bytes, marker says {}",
                f.name, f.bytes
            ));
        }
        if verify_hashes && sha256_file(&path).map_err(|e| format!("{}: {e}", f.name))? != f.sha256
        {
            return Err(format!("{} does not match its sha256", f.name));
        }
    }
    for a in &info.absent {
        if dir.join(a).exists() {
            return Err(format!("{a} is present but the marker says absent"));
        }
    }
    Ok(info)
}

/// Every `before-upgrade-*` directory, oldest name first.
fn before_dirs(network_dir: &Path) -> io::Result<Vec<PathBuf>> {
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
        if name
            .to_string_lossy()
            .starts_with(&format!("{BEFORE_PREFIX}-"))
            && entry.file_type()?.is_dir()
        {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Whether a completed before-upgrade snapshot exists with its files present
/// (chooses `storeUpgrade.cantOpen.text` over `textWithoutBackup`).
pub fn snapshot_present(network_dir: &Path) -> io::Result<bool> {
    Ok(before_dirs(network_dir)?
        .iter()
        .any(|d| read_snapshot(d, false).is_ok()))
}

/// The newest completed snapshot, its files checked against their sha256.
pub fn latest_snapshot(network_dir: &Path) -> Result<Option<SnapshotInfo>> {
    let dirs =
        before_dirs(network_dir).map_err(StoreUpgradeError::io("list upgrade backups", true))?;
    Ok(dirs.iter().rev().find_map(|d| read_snapshot(d, true).ok()))
}

/// What identifies the wallet data a snapshot was taken of.
struct LiveState {
    refinery_max: i64,
    fork_versions: BTreeSet<u32>,
    fingerprint: String,
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

fn live_state(conn: &Connection) -> rusqlite::Result<LiveState> {
    let refinery_max = if has_table(conn, "refinery_schema_history")? {
        conn.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )?
    } else {
        0
    };
    let fork_versions = if has_table(conn, fork::FORK_HISTORY)? {
        conn.prepare(&format!("SELECT version FROM {}", fork::FORK_HISTORY))?
            .query_map([], |r| r.get::<_, u32>(0))?
            .collect::<rusqlite::Result<_>>()?
    } else {
        BTreeSet::new()
    };
    Ok(LiveState {
        refinery_max,
        fork_versions,
        fingerprint: fingerprint(conn)?,
    })
}

fn open_source(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

/// A completed snapshot is this upgrade's when the fork versions match and
/// either upstream has migrated since (the upgrade is in flight) or the data
/// is the same apart from what the upgrade changes.
fn reusable(info: &SnapshotInfo, live: &LiveState) -> bool {
    info.fork_versions == live.fork_versions
        && (live.refinery_max > info.refinery_max || live.fingerprint == info.fingerprint)
}

fn failed(detail: impl std::fmt::Display) -> StoreUpgradeError {
    StoreUpgradeError::SnapshotFailed {
        detail: detail.to_string(),
    }
}

/// Take the pre-upgrade snapshot of `network_dir`, or return this upgrade's
/// completed one. A directory left without its marker by an interrupted run
/// is discarded and redone. The wallet files are not changed (the source
/// connections only read, though closing one may checkpoint its WAL).
pub fn take_snapshot(network_dir: &Path, now: SystemTime) -> Result<SnapshotInfo> {
    crate::token::refuse_if_held(network_dir)?;
    let wallet = network_dir.join(STORE_FILES[0]);
    let live = if wallet.exists() {
        let conn = open_source(&wallet).map_err(|e| failed(format!("open wallet.sqlite: {e}")))?;
        Some(live_state(&conn).map_err(|e| failed(format!("read wallet.sqlite: {e}")))?)
    } else {
        None
    };
    let dirs = before_dirs(network_dir).map_err(|e| failed(format!("list snapshots: {e}")))?;
    for dir in &dirs {
        if !dir.join(MARKER).exists() {
            tracing::warn!(dir = %dir.display(), "store upgrade: discarding an incomplete snapshot");
            fs::remove_dir_all(dir)
                .map_err(|e| failed(format!("discard {}: {e}", dir.display())))?;
        }
    }
    if let Some(live) = &live {
        for dir in dirs.iter().rev().filter(|d| d.exists()) {
            if let Ok(info) = read_snapshot(dir, true)
                && reusable(&info, live)
            {
                return Ok(info);
            }
        }
    }
    let root = backups_dir(network_dir);
    fsx::ensure_private_dir(&root)
        .map_err(|e| failed(format!("create {UPGRADE_BACKUPS_DIR}: {e}")))?;
    let dir = root.join(fsx::fresh_name(&root, BEFORE_PREFIX, now));
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
        copy_store(&src_path, &dest).map_err(|e| failed(format!("copy {name}: {e}")))?;
        if name == STORE_FILES[0] {
            let conn =
                sql::open_immutable(&dest).map_err(|e| failed(format!("reopen copy: {e}")))?;
            snapshot_state =
                Some(live_state(&conn).map_err(|e| failed(format!("read copy: {e}")))?);
        }
        files.push(SnapshotFile {
            name: name.into(),
            bytes: fs::metadata(&dest)
                .map_err(|e| failed(format!("{name}: {e}")))?
                .len(),
            sha256: sha256_file(&dest).map_err(|e| failed(format!("{name}: {e}")))?,
        });
        fault("snapshot.after_file")?;
    }
    let state = snapshot_state.unwrap_or(LiveState {
        refinery_max: 0,
        fork_versions: BTreeSet::new(),
        fingerprint: "-".into(),
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

/// The online backup of one store into a single rollback-journal file,
/// integrity-checked and fsynced.
fn copy_store(src_path: &Path, dest: &Path) -> std::result::Result<(), String> {
    let src = open_source(src_path).map_err(|e| e.to_string())?;
    let mut dst = Connection::open(dest).map_err(|e| e.to_string())?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst).map_err(|e| e.to_string())?;
        loop {
            match backup.step(-1).map_err(|e| e.to_string())? {
                rusqlite::backup::StepResult::Done => break,
                rusqlite::backup::StepResult::More => continue,
                rusqlite::backup::StepResult::Busy | rusqlite::backup::StepResult::Locked => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                _ => return Err("unexpected backup step".into()),
            }
        }
    }
    drop(src);
    let mode: String = dst
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !mode.eq_ignore_ascii_case("delete") {
        return Err(format!("copy journal mode is {mode}"));
    }
    if !sql::integrity_ok(&dst).map_err(|e| e.to_string())? {
        return Err("integrity_check of the copy failed".into());
    }
    dst.close().map_err(|(_, e)| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    }
    fsx::fsync_file(dest).map_err(|e| e.to_string())
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
