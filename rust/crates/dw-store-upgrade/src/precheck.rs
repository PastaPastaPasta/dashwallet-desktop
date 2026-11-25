//! The refusal check before anything in `{folder}` is written, renamed or
//! locked (Q78 R4, Q78-A R1 amendment §2, MG (f)).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use platform_wallet_storage::{
    LoadPolicy, SqlitePersister, SqlitePersisterConfig, WalletStorageError,
};

use crate::adopters::Adopters;
use crate::classify::{Classification, ForkState, classify};
use crate::error::{Result, StoreUpgradeError};
use crate::snapshot::STORE_FILES;
use crate::sql::{map_sqlite, open_existing, open_immutable};

/// What a network directory's `wallet.sqlite` is to this build.
#[derive(Debug)]
pub enum Precheck {
    /// No fork history: a fresh directory, an upstream file, or one already
    /// retired. Opens as usual.
    NotForked,
    /// A fork file this build upgrades.
    Fork { versions: BTreeSet<u32> },
    /// This build can't open it. Nothing in `{folder}` was changed.
    Refuse { reason: StoreUpgradeError },
    /// The file has a write-ahead log or hot journal the immutable read
    /// can't see: run [`precheck_locked`].
    NeedsLockedCheck,
}

fn wallet_path(network_dir: &Path) -> std::path::PathBuf {
    network_dir.join(STORE_FILES[0])
}

fn side(network_dir: &Path, suffix: &str) -> std::path::PathBuf {
    network_dir.join(format!("{}{suffix}", STORE_FILES[0]))
}

fn non_empty(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

fn verdict(c: Classification) -> Precheck {
    match c {
        Classification::NotForked => Precheck::NotForked,
        Classification::Fork(state) => Precheck::Fork {
            versions: state.versions,
        },
        Classification::Refuse(reason) => Precheck::Refuse { reason },
    }
}

/// [`precheck_with`] for this build's probed adopters; the probe runs once
/// per process under the data root (`network_dir`'s parent).
pub fn precheck(network_dir: &Path) -> Result<Precheck> {
    precheck_with(network_dir, Adopters::for_build(parent_dir(network_dir)?))
}

/// [`precheck_locked_with`] for this build's probed adopters.
pub fn precheck_locked(network_dir: &Path) -> Result<Precheck> {
    precheck_locked_with(network_dir, Adopters::for_build(parent_dir(network_dir)?))
}

/// The directory holding `path`: a network directory's data root, or a
/// store file's network directory. Never the working directory.
pub(crate) fn parent_dir(path: &Path) -> Result<&Path> {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| StoreUpgradeError::Io {
            context: "find the parent directory",
            unchanged: true,
            source: io::Error::new(
                io::ErrorKind::NotFound,
                format!("{} has no parent directory", path.display()),
            ),
        })
}

/// Name prefix of the private copies the checks make under the data root.
pub(crate) const SCRATCH_PREFIX: &str = ".store-upgrade-precheck-";

fn refuse_early(network_dir: &Path, adopters: &Adopters) -> Result<Option<Precheck>> {
    crate::token::refuse_if_held(network_dir)?;
    crate::restore::refuse_if_restore_pending(network_dir)?;
    if let crate::adopters::P9aStatus::Broken { detail } = adopters.p9a_status() {
        return Ok(Some(Precheck::Refuse {
            reason: StoreUpgradeError::P9aBroken {
                detail: detail.clone(),
            },
        }));
    }
    if !wallet_path(network_dir).exists() {
        return Ok(Some(Precheck::NotForked));
    }
    Ok(None)
}

/// The settings the public checks rehearse with: upstream's defaults under
/// `LoadPolicy::Strict`. [`crate::reconcile_fork_store`] rehearses with the
/// caller's own config before it writes.
fn default_open(network_dir: &Path) -> SqlitePersisterConfig {
    SqlitePersisterConfig::new(wallet_path(network_dir)).with_load_policy(LoadPolicy::Strict)
}

/// Read `wallet.sqlite` with `immutable=1`: no lock, no `-wal`/`-shm`, no
/// file created. A non-empty `-wal` or `-journal` means the main file alone
/// may be stale, so the answer is [`Precheck::NeedsLockedCheck`]. A fork file
/// the upgrade will write is then rehearsed on a private copy
/// ([`StoreUpgradeError::UpgradeRehearsalFailed`]).
pub fn precheck_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
    check(network_dir, adopters, &default_open(network_dir))
}

/// The same check with the log applied: `wallet.sqlite` and its `-wal` or
/// `-journal` are copied into a private directory beside `{folder}` (under
/// the data root, never inside `{folder}`) and the copy is opened with a
/// normal connection, which replays the log. The originals stay
/// byte-identical: a normal connection on them would checkpoint the log or
/// write `-shm`.
pub fn precheck_locked_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
    check_locked(network_dir, adopters, &default_open(network_dir))
}

/// [`precheck_with`], then [`precheck_locked_with`] when it asks, the
/// rehearsal opening the copy with `open`'s settings.
pub(crate) fn check_all(
    network_dir: &Path,
    adopters: &Adopters,
    open: &SqlitePersisterConfig,
) -> Result<Precheck> {
    match check(network_dir, adopters, open)? {
        Precheck::NeedsLockedCheck => check_locked(network_dir, adopters, open),
        other => Ok(other),
    }
}

fn check(
    network_dir: &Path,
    adopters: &Adopters,
    open: &SqlitePersisterConfig,
) -> Result<Precheck> {
    if let Some(early) = refuse_early(network_dir, adopters)? {
        return Ok(early);
    }
    if non_empty(&side(network_dir, "-wal")) || non_empty(&side(network_dir, "-journal")) {
        return Ok(Precheck::NeedsLockedCheck);
    }
    let path = wallet_path(network_dir);
    let conn = open_immutable(&path).map_err(map_sqlite("open wallet.sqlite read-only"))?;
    let found = classify(&conn, adopters)?;
    drop(conn);
    match found {
        Classification::Fork(state) if state.upgrade_writes() => {
            ScratchCopy::of(network_dir)?.rehearse(state, open)
        }
        other => Ok(verdict(other)),
    }
}

fn check_locked(
    network_dir: &Path,
    adopters: &Adopters,
    open: &SqlitePersisterConfig,
) -> Result<Precheck> {
    if let Some(early) = refuse_early(network_dir, adopters)? {
        return Ok(early);
    }
    let copy = ScratchCopy::of(network_dir)?;
    let conn = open_existing(&copy.wallet).map_err(map_sqlite("open the wallet.sqlite copy"))?;
    let found = classify(&conn, adopters);
    drop(conn);
    match found? {
        Classification::Fork(state) if state.upgrade_writes() => copy.rehearse(state, open),
        other => Ok(verdict(other)),
    }
}

/// `wallet.sqlite` and its log, copied into a private 0700 directory under
/// the data root; removed on drop.
struct ScratchCopy {
    dir: tempfile::TempDir,
    wallet: PathBuf,
}

impl ScratchCopy {
    fn of(network_dir: &Path) -> Result<Self> {
        let io = |ctx| StoreUpgradeError::io(ctx, true);
        let dir = tempfile::Builder::new()
            .prefix(SCRATCH_PREFIX)
            .tempdir_in(parent_dir(network_dir)?)
            .map_err(io("create precheck scratch dir"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))
                .map_err(io("chmod precheck scratch dir"))?;
        }
        let wallet = dir.path().join(STORE_FILES[0]);
        fs::copy(wallet_path(network_dir), &wallet).map_err(io("copy wallet.sqlite"))?;
        for suffix in ["-wal", "-journal"] {
            let src = side(network_dir, suffix);
            if src.exists() {
                fs::copy(&src, dir.path().join(format!("{}{suffix}", STORE_FILES[0])))
                    .map_err(io("copy wallet.sqlite log"))?;
            }
        }
        Ok(Self { dir, wallet })
    }

    /// M4: R1's drop, then the upstream open under `LoadPolicy::Strict`, on
    /// the copy, with `open`'s settings (journal mode, synchronous, busy
    /// timeout, flush mode, whether auto-backups are on) and its backup
    /// directory moved into scratch. A failure on the data is a refusal
    /// before the real file is touched; a failure of the machine is a
    /// retryable [`StoreUpgradeError::Io`].
    fn rehearse(&self, state: ForkState, open: &SqlitePersisterConfig) -> Result<Precheck> {
        let failed = |detail: String| {
            Ok(Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { detail },
            })
        };
        let conn = match open_existing(&self.wallet) {
            Ok(c) => c,
            Err(e) if sqlite_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("open the copy: {e}")),
        };
        match crate::reconcile::drop_external_reference(&conn) {
            Ok(()) => {}
            Err(e) if sqlite_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("drop contacts.external_account_reference: {e}")),
        }
        drop(conn);
        let mut config = open.clone();
        config.path = self.wallet.clone();
        config.load_policy = LoadPolicy::Strict;
        if config.auto_backup_dir.is_some() {
            let auto = self.dir.path().join("auto");
            #[cfg(test)]
            if BLOCK_REHEARSAL_BACKUP.with(|b| b.get()) {
                fs::write(&auto, b"")
                    .map_err(StoreUpgradeError::io("block the backup dir", true))?;
            }
            config.auto_backup_dir = Some(auto);
        }
        match SqlitePersister::open(config) {
            Ok(persister) => drop(persister),
            Err(e) if storage_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("upstream open: {e}")),
        }
        Ok(Precheck::Fork {
            versions: state.versions,
        })
    }
}

#[cfg(test)]
thread_local! {
    /// Puts a file where the rehearsal's backup directory goes.
    pub(crate) static BLOCK_REHEARSAL_BACKUP: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

/// SQLite failing for the machine, not the data.
fn sqlite_fault(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode::*;
    matches!(
        e.sqlite_error_code(),
        Some(
            DiskFull
                | SystemIoFailure
                | CannotOpen
                | ReadOnly
                | PermissionDenied
                | DatabaseBusy
                | DatabaseLocked
                | OutOfMemory
        )
    )
}

/// Upstream's open failing for the machine: I/O, the backup directory, or
/// a SQLite fault.
fn storage_fault(e: &WalletStorageError) -> bool {
    match e {
        WalletStorageError::Io(_) | WalletStorageError::AutoBackupDirUnwritable { .. } => true,
        WalletStorageError::Sqlite(s) => sqlite_fault(s),
        other => other.is_transient(),
    }
}

fn machine_fault(e: impl std::fmt::Display) -> StoreUpgradeError {
    StoreUpgradeError::Io {
        context: "rehearse the upgrade on a copy",
        unchanged: true,
        source: io::Error::other(e.to_string()),
    }
}

/// Remove the private directories a crash left under `data_root`: the
/// checks' copies (`.store-upgrade-precheck-*`) and the probe's store
/// (`.store-upgrade-probe-*`).
/// Sweeps only when no other process holds the data root's instance lock
/// ([`dw_fs::instance::InstanceLock::share`]: this process's own lock, or the
/// lock taken for the sweep); another holder is
/// [`StoreUpgradeError::StoreHeld`]. Call it once per start, after the
/// instance lock and before any check: a check or probe running in another
/// thread of this process would lose its directory. This process's own
/// probe ([`crate::Adopters::for_build`]) removes its directory before it
/// returns, so a probe that ran earlier leaves nothing live to sweep.
/// Returns how many were removed.
pub fn sweep_stale_scratch(data_root: &Path) -> Result<usize> {
    let io = |ctx| StoreUpgradeError::io(ctx, true);
    let lock_path = data_root.join(dw_fs::instance::InstanceLock::FILE);
    let _lock = match dw_fs::instance::InstanceLock::share(data_root) {
        Ok(lock) => lock,
        Err(dw_fs::instance::InstanceError::Busy) => {
            return Err(StoreUpgradeError::StoreHeld { path: lock_path });
        }
        Err(dw_fs::instance::InstanceError::Io(e)) => return Err(io("take the instance lock")(e)),
    };
    let mut removed = 0;
    for entry in fs::read_dir(data_root).map_err(io("list the data root"))? {
        let entry = entry.map_err(io("list the data root"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_scratch =
            name.starts_with(SCRATCH_PREFIX) || name.starts_with(crate::adopters::PROBE_PREFIX);
        if is_scratch
            && entry
                .file_type()
                .map_err(io("stat a scratch dir"))?
                .is_dir()
        {
            fs::remove_dir_all(entry.path()).map_err(io("remove a stale scratch dir"))?;
            removed += 1;
        }
    }
    Ok(removed)
}
