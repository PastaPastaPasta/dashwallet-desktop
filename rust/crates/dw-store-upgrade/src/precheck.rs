//! The refusal check before anything in `{folder}` is written, renamed or
//! locked (Q78 R4, Q78-A R1 amendment §2, MG (f)).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crate::adopters::Adopters;
use crate::classify::{Classification, classify};
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

fn from_classification(c: Classification) -> Precheck {
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
    precheck_with(network_dir, Adopters::for_build(data_root(network_dir)))
}

/// [`precheck_locked_with`] for this build's probed adopters.
pub fn precheck_locked(network_dir: &Path) -> Result<Precheck> {
    precheck_locked_with(network_dir, Adopters::for_build(data_root(network_dir)))
}

pub(crate) fn data_root(network_dir: &Path) -> &Path {
    network_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

/// Read `wallet.sqlite` with `immutable=1`: no lock, no `-wal`/`-shm`, no
/// file created. A non-empty `-wal` or `-journal` means the main file alone
/// may be stale, so the answer is [`Precheck::NeedsLockedCheck`].
pub fn precheck_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
    crate::token::refuse_if_held(network_dir)?;
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let path = wallet_path(network_dir);
    if let crate::adopters::P9aStatus::Broken { detail } = adopters.p9a_status() {
        return Ok(Precheck::Refuse {
            reason: StoreUpgradeError::P9aBroken {
                detail: detail.clone(),
            },
        });
    }
    if !path.exists() {
        return Ok(Precheck::NotForked);
    }
    if non_empty(&side(network_dir, "-wal")) || non_empty(&side(network_dir, "-journal")) {
        return Ok(Precheck::NeedsLockedCheck);
    }
    let conn = open_immutable(&path).map_err(map_sqlite("open wallet.sqlite read-only"))?;
    Ok(from_classification(classify(&conn, adopters)?))
}

/// The same check with the log applied: `wallet.sqlite` and its `-wal` or
/// `-journal` are copied into a private directory beside `{folder}` (under
/// the data root, never inside `{folder}`) and the copy is opened with a
/// normal connection, which replays the log. The originals stay
/// byte-identical: a normal connection on them would checkpoint the log or
/// write `-shm`.
pub fn precheck_locked_with(network_dir: &Path, adopters: &Adopters) -> Result<Precheck> {
    crate::token::refuse_if_held(network_dir)?;
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let path = wallet_path(network_dir);
    if let crate::adopters::P9aStatus::Broken { detail } = adopters.p9a_status() {
        return Ok(Precheck::Refuse {
            reason: StoreUpgradeError::P9aBroken {
                detail: detail.clone(),
            },
        });
    }
    if !path.exists() {
        return Ok(Precheck::NotForked);
    }
    let io = |ctx| StoreUpgradeError::io(ctx, true);
    let scratch = tempfile::Builder::new()
        .prefix(".store-upgrade-precheck-")
        .tempdir_in(data_root(network_dir))
        .map_err(io("create precheck scratch dir"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(scratch.path(), fs::Permissions::from_mode(0o700))
            .map_err(io("chmod precheck scratch dir"))?;
    }
    let copy = scratch.path().join(STORE_FILES[0]);
    fs::copy(&path, &copy).map_err(io("copy wallet.sqlite"))?;
    for suffix in ["-wal", "-journal"] {
        let src = side(network_dir, suffix);
        if src.exists() {
            fs::copy(
                &src,
                scratch.path().join(format!("{}{suffix}", STORE_FILES[0])),
            )
            .map_err(io("copy wallet.sqlite log"))?;
        }
    }
    let conn = open_existing(&copy).map_err(map_sqlite("open the wallet.sqlite copy"))?;
    let result = classify(&conn, adopters);
    drop(conn);
    Ok(from_classification(result?))
}
