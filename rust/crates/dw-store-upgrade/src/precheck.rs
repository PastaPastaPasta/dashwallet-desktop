//! The refusal check before anything in `{folder}` is written, renamed or
//! locked (Q78 R4, Q78-A R1 amendment §2, MG (f)).

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};

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

/// Read `wallet.sqlite` with `immutable=1`: no lock, no `-wal`/`-shm`, no
/// file created. A non-empty `-wal` or `-journal` means the main file alone
/// may be stale, so the answer is [`Precheck::NeedsLockedCheck`]. A fork file
/// the upgrade will write is then rehearsed on a private copy
/// ([`StoreUpgradeError::UpgradeRehearsalFailed`]).
pub fn precheck_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
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
            let copy = ScratchCopy::of(network_dir)?;
            copy.rehearse(state)
        }
        other => Ok(verdict(other)),
    }
}

/// The same check with the log applied: `wallet.sqlite` and its `-wal` or
/// `-journal` are copied into a private directory beside `{folder}` (under
/// the data root, never inside `{folder}`) and the copy is opened with a
/// normal connection, which replays the log. The originals stay
/// byte-identical: a normal connection on them would checkpoint the log or
/// write `-shm`.
pub fn precheck_locked_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
    if let Some(early) = refuse_early(network_dir, adopters)? {
        return Ok(early);
    }
    let copy = ScratchCopy::of(network_dir)?;
    let conn = open_existing(&copy.wallet).map_err(map_sqlite("open the wallet.sqlite copy"))?;
    let found = classify(&conn, adopters);
    drop(conn);
    match found? {
        Classification::Fork(state) if state.upgrade_writes() => copy.rehearse(state),
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
    /// the copy. A failure is a refusal before the real file is touched.
    fn rehearse(&self, state: ForkState) -> Result<Precheck> {
        let failed = |detail: String| {
            Ok(Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { detail },
            })
        };
        let conn = match open_existing(&self.wallet) {
            Ok(c) => c,
            Err(e) => return failed(format!("open the copy: {e}")),
        };
        if let Err(e) = crate::reconcile::drop_external_reference(&conn) {
            return failed(format!("drop contacts.external_account_reference: {e}"));
        }
        drop(conn);
        let config = SqlitePersisterConfig::new(&self.wallet)
            .with_load_policy(LoadPolicy::Strict)
            .with_auto_backup_dir(Some(self.dir.path().join("auto")));
        match SqlitePersister::open(config) {
            Ok(persister) => drop(persister),
            Err(e) => return failed(format!("upstream open: {e}")),
        }
        Ok(Precheck::Fork {
            versions: state.versions,
        })
    }
}

/// Remove private copies left under `data_root` by a check that crashed.
/// `instance_lock` must be the caller's locked handle on
/// `<data_root>/instance.lock`: only the instance holding it may sweep, so no
/// running check loses its copy. Returns how many were removed.
pub fn sweep_stale_scratch(data_root: &Path, instance_lock: &fs::File) -> Result<usize> {
    let io = |ctx| StoreUpgradeError::io(ctx, true);
    let lock_path = data_root.join("instance.lock");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held = instance_lock
            .metadata()
            .map_err(io("stat the instance lock"))?;
        let named = fs::metadata(&lock_path).map_err(io("stat the instance lock"))?;
        if (held.dev(), held.ino()) != (named.dev(), named.ino()) {
            return Err(StoreUpgradeError::StoreHeld { path: lock_path });
        }
    }
    let probe = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(io("open the instance lock"))?;
    match probe.try_lock() {
        Err(fs::TryLockError::WouldBlock) => {}
        Ok(()) => return Err(StoreUpgradeError::StoreHeld { path: lock_path }),
        Err(fs::TryLockError::Error(e)) => return Err(io("probe the instance lock")(e)),
    }
    drop(probe);
    let mut removed = 0;
    for entry in fs::read_dir(data_root).map_err(io("list the data root"))? {
        let entry = entry.map_err(io("list the data root"))?;
        let is_scratch = entry
            .file_name()
            .to_string_lossy()
            .starts_with(SCRATCH_PREFIX);
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
