//! The refusal check before anything in `{folder}` is written, renamed or
//! locked (Q78 R4, Q78-A R1 amendment §2, MG (f)).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use platform_wallet::changeset::{PersistenceError, PlatformWalletPersistence};
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
            if let Some(refused) = refuse_unopenable_file(network_dir)? {
                return Ok(refused);
            }
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
        Classification::Fork(state) if state.upgrade_writes() => {
            match refuse_unopenable_file(network_dir)? {
                Some(refused) => Ok(refused),
                None => copy.rehearse(state, open),
            }
        }
        other => Ok(verdict(other)),
    }
}

/// What the rehearsal's copy can't show: upstream's real open refuses a
/// symlinked `wallet.sqlite` on unix (its `reject_symlink` has a unix body
/// only) and makes the file and its `-wal` and `-shm` owner-only, which
/// fails on another user's file. Either would fail every start after R1's
/// drop.
fn refuse_unopenable_file(network_dir: &Path) -> Result<Option<Precheck>> {
    for (suffix, context) in [
        ("", "stat wallet.sqlite"),
        ("-wal", "stat wallet.sqlite-wal"),
        ("-shm", "stat wallet.sqlite-shm"),
    ] {
        let meta = match fs::symlink_metadata(side(network_dir, suffix)) {
            Ok(meta) => meta,
            Err(e) if !suffix.is_empty() && e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(StoreUpgradeError::io(context, true)(e)),
        };
        if let Some(what) = unsupported(&meta) {
            return Ok(Some(Precheck::Refuse {
                reason: StoreUpgradeError::WalletFileUnsupported {
                    detail: format!("{}{suffix} {what}", STORE_FILES[0]),
                },
            }));
        }
    }
    Ok(None)
}

/// [`refuse_unopenable_file`] as an error: repeated under the write lock,
/// right before R1's drop or the retire.
pub(crate) fn require_openable_file(network_dir: &Path) -> Result<()> {
    match refuse_unopenable_file(network_dir)? {
        Some(Precheck::Refuse { reason }) => Err(reason),
        _ => Ok(()),
    }
}

#[cfg(unix)]
fn unsupported(meta: &fs::Metadata) -> Option<String> {
    if meta.file_type().is_symlink() {
        return Some("is a symbolic link".to_string());
    }
    foreign_owner(meta)
}

#[cfg(not(unix))]
fn unsupported(_meta: &fs::Metadata) -> Option<String> {
    None
}

#[cfg(unix)]
fn foreign_owner(meta: &fs::Metadata) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let me = effective_uid();
    (meta.uid() != me).then(|| format!("is owned by uid {}, not {me}", meta.uid()))
}

#[cfg(unix)]
fn effective_uid() -> u32 {
    #[cfg(test)]
    if let Some(uid) = EUID_FOR_TEST.with(|u| u.get()) {
        return uid;
    }
    rustix::process::geteuid().as_raw()
}

/// `wallet.sqlite` and its log, copied into a private 0700 directory under
/// the data root; removed on drop.
pub(crate) struct ScratchCopy {
    dir: tempfile::TempDir,
    wallet: PathBuf,
}

impl ScratchCopy {
    pub(crate) fn of(network_dir: &Path) -> Result<Self> {
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

    /// The copy of `wallet.sqlite`, its log beside it.
    pub(crate) fn wallet(&self) -> &Path {
        &self.wallet
    }

    /// M4: R1's drop, then the upstream open and load under
    /// `LoadPolicy::Strict`, on the copy, with `open`'s settings (journal mode, synchronous, busy
    /// timeout, flush mode, whether auto-backups are on) and its backup
    /// directory moved into scratch. A failure on the data is a refusal
    /// before the real file is touched; a failure of the machine is a
    /// retryable [`StoreUpgradeError::Io`], except SQLite refusing to write
    /// the copy ([`write_refused`]), which is refused.
    fn rehearse(&self, state: ForkState, open: &SqlitePersisterConfig) -> Result<Precheck> {
        let failed = |detail: String| {
            Ok(Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { detail },
            })
        };
        let unwritable = |detail: String| {
            Ok(Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeCopyUnwritable { detail },
            })
        };
        let conn = match open_existing(&self.wallet) {
            Ok(c) => c,
            Err(e) if write_refused(&e) => return unwritable(format!("open the copy: {e}")),
            Err(e) if sqlite_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("open the copy: {e}")),
        };
        match crate::reconcile::drop_external_reference(&conn) {
            Ok(()) => {}
            Err(e) if write_refused(&e) => {
                return unwritable(format!("drop contacts.external_account_reference: {e}"));
            }
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
        #[cfg(test)]
        let opened = OPEN_FOR_TEST
            .with(|o| o.take())
            .map_or_else(|| SqlitePersister::open(config), |f| Err(f()));
        #[cfg(not(test))]
        let opened = SqlitePersister::open(config);
        let persister = match opened {
            Ok(persister) => persister,
            Err(WalletStorageError::ConfigInvalid { reason }) => {
                return Err(StoreUpgradeError::OpenConfigUnsupported {
                    detail: reason.into(),
                });
            }
            Err(e) if write_refused(&e) => return unwritable(format!("upstream open: {e}")),
            Err(e) if storage_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("upstream open: {e}")),
        };
        #[cfg(test)]
        let loaded = LOAD_FOR_TEST
            .with(|l| l.take())
            .map_or_else(|| persister.load().map(drop), |f| Err(f()));
        #[cfg(not(test))]
        let loaded = persister.load().map(drop);
        match loaded {
            Ok(()) => {}
            Err(e) if !e.is_transient() && write_refused(&e) => {
                return unwritable(format!("upstream load: {e}"));
            }
            Err(e) if persistence_fault(&e) => return Err(machine_fault(e)),
            Err(e) => return failed(format!("upstream load: {e}")),
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
    /// Replaces the rehearsal's upstream open result with this error, once.
    pub(crate) static OPEN_FOR_TEST: std::cell::Cell<Option<fn() -> WalletStorageError>> =
        const { std::cell::Cell::new(None) };
    /// Replaces the rehearsal's load result with this error, once.
    pub(crate) static LOAD_FOR_TEST: std::cell::Cell<Option<fn() -> PersistenceError>> =
        const { std::cell::Cell::new(None) };
    /// Pretends this process runs as another user.
    pub(crate) static EUID_FOR_TEST: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
}

/// SQLite failing for the machine, not the data, in a way a retry can
/// clear: retried as [`StoreUpgradeError::Io`].
/// - `SQLITE_FULL`, `SQLITE_IOERR`, `SQLITE_NOMEM`: the disk and memory.
/// - `SQLITE_BUSY`, `SQLITE_LOCKED`: a lock, as upstream's `is_transient`.
/// - `SQLITE_CANTOPEN`: the copy sits in a 0700 directory this process just
///   made and wrote it into, so what is left is the machine: descriptors
///   run out (`EMFILE`, `ENFILE`) or the directory went away, which a later
///   start clears. A copy that can't be made at all is already a retried
///   `Io`.
///
/// `SQLITE_READONLY` and `SQLITE_PERM` are [`write_refused`] instead.
fn sqlite_fault(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode::*;
    matches!(
        e.sqlite_error_code(),
        Some(DiskFull | SystemIoFailure | CannotOpen | DatabaseBusy | DatabaseLocked | OutOfMemory)
    )
}

/// SQLite refusing to write the copy (`SQLITE_READONLY`, `SQLITE_PERM`)
/// anywhere in the error's sources. The copy keeps `wallet.sqlite`'s own
/// mode, so a write-protected file ends here, and every retry would too:
/// a refusal ([`StoreUpgradeError::UpgradeCopyUnwritable`]), with no defect
/// of the data claimed.
pub(crate) fn write_refused(e: &(dyn std::error::Error + 'static)) -> bool {
    use rusqlite::ErrorCode::{PermissionDenied, ReadOnly};
    std::iter::successors(Some(e), |err| err.source()).any(|err| {
        err.downcast_ref::<rusqlite::Error>()
            .is_some_and(|s| matches!(s.sqlite_error_code(), Some(ReadOnly | PermissionDenied)))
    })
}

/// Upstream's open failing for the machine: I/O, the backup directory, or
/// a SQLite fault, found anywhere in the error's sources (a migration or an
/// integrity check that could not run carries its SQLite error inside).
pub(crate) fn storage_fault(e: &WalletStorageError) -> bool {
    machine_cause(e)
}

/// Upstream's load failing for the machine: marked transient, or a machine
/// cause in its sources.
fn persistence_fault(e: &PersistenceError) -> bool {
    e.is_transient() || machine_cause(e)
}

/// Upstream failing to decode stored bytes: a defect of the data, whatever
/// its sources say (a consensus decode reports a short blob as an
/// `UnexpectedEof` I/O error).
pub(crate) fn decode_defect(e: &WalletStorageError) -> bool {
    matches!(
        e,
        WalletStorageError::ConsensusCodec { .. }
            | WalletStorageError::BincodeDecode { .. }
            | WalletStorageError::HashDecode { .. }
            | WalletStorageError::AddressDecode { .. }
            | WalletStorageError::BlobDecode { .. }
    )
}

/// An I/O error of the machine, not a short or malformed read.
pub(crate) fn machine_io(e: &io::Error) -> bool {
    !matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::InvalidData
    )
}

fn machine_cause(e: &(dyn std::error::Error + 'static)) -> bool {
    let mut next = Some(e);
    while let Some(err) = next {
        if let Some(s) = err.downcast_ref::<WalletStorageError>() {
            if decode_defect(s) {
                return false;
            }
            if matches!(s, WalletStorageError::AutoBackupDirUnwritable { .. }) || s.is_transient() {
                return true;
            }
        }
        if err.downcast_ref::<io::Error>().is_some_and(machine_io)
            || err
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(sqlite_fault)
        {
            return true;
        }
        next = err.source();
    }
    false
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
