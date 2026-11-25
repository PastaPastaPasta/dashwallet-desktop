//! Test support for other crates' tests (feature `test-support`, which the
//! app never enables): a real between-state store.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use platform_wallet_storage::{LoadPolicy, SqlitePersisterConfig};

use crate::error::{Result, StoreUpgradeError};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::SnapshotInfo;
use crate::{Adopters, PersisterOpened, prepare_with};

/// What becomes of the before-upgrade snapshot once R1's drop has committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeforeSnapshot {
    /// Left as taken: verified, and of the live data.
    Present,
    /// Its directory removed.
    Absent,
    /// One byte of its `app.sqlite` flipped.
    Unverified,
    /// Left as taken, and one live `core_utxos` value changed.
    Mismatch,
}

/// A between-state store [`between_state`] built.
#[derive(Debug)]
pub struct BetweenState {
    /// The config R1 was rehearsed with; open with this one.
    pub config: SqlitePersisterConfig,
    /// The before-upgrade snapshot R1 named, as taken.
    pub snapshot: SnapshotInfo,
    /// With `again`, the copy the next start kept: the snapshot itself when
    /// [`BeforeSnapshot::Present`], otherwise the `part-upgraded` copy it
    /// took.
    pub kept: Option<SnapshotInfo>,
}

/// The released v0.1.0 fork fixture's network directory (fork V1-V3 with
/// all four stores), read only.
pub fn fork_fixture() -> PathBuf {
    std::env::var_os("DWSU_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .join("testdata/upgrade/v0.1.0/data/regtest")
}

/// Build a real between-state store in `network_dir`, which must not exist
/// and whose parent (the data root) must, owner-only. The fork fixture
/// ([`fork_fixture`]) is copied in; then the real precheck and
/// [`crate::take_snapshot`] ([`prepare_with`]), R1's drop
/// ([`reconcile_fork_store`]), and the upstream open, made to fail in its
/// backup step (`backups/` a file for that open only) so upstream never
/// migrates. `before` is then applied to the snapshot. With `again`, one
/// more start runs its prepare and reconcile, keeping a copy, and stops
/// before the open. Starts are stamped `now`, then `now` + 60 s.
pub fn between_state(
    network_dir: &Path,
    before: BeforeSnapshot,
    again: bool,
    now: SystemTime,
) -> Result<BetweenState> {
    let io = |ctx| StoreUpgradeError::io(ctx, true);
    copy_tree(&fork_fixture(), network_dir).map_err(io("copy the fork fixture"))?;
    let adopters = Adopters::for_build(crate::precheck::parent_dir(network_dir)?);
    let config = SqlitePersisterConfig::new(network_dir.join("wallet.sqlite"))
        .with_load_policy(LoadPolicy::Strict)
        .with_auto_backup_dir(Some(network_dir.join("backups").join("auto")));
    let snapshot = start(network_dir, adopters, &config, now)?;
    let backups = network_dir.join("backups");
    if backups.is_dir() {
        fs::remove_dir_all(&backups).map_err(io("clear backups/"))?;
    }
    fs::write(&backups, b"").map_err(io("block backups/"))?;
    let opened = PersisterOpened::open(config.clone());
    fs::remove_file(&backups).map_err(io("unblock backups/"))?;
    if opened.is_ok() {
        return Err(StoreUpgradeError::OpenConfigUnsupported {
            detail: "the upstream open was expected to fail in its backup step".into(),
        });
    }
    match before {
        BeforeSnapshot::Present => {}
        BeforeSnapshot::Absent => {
            fs::remove_dir_all(snapshot.dir()).map_err(io("remove the snapshot"))?;
        }
        BeforeSnapshot::Unverified => {
            let app = snapshot.dir().join("app.sqlite");
            let mut bytes = fs::read(&app).map_err(io("read the snapshot"))?;
            bytes[100] ^= 1;
            fs::write(&app, bytes).map_err(io("damage the snapshot"))?;
        }
        BeforeSnapshot::Mismatch => {
            rusqlite::Connection::open(network_dir.join("wallet.sqlite"))
                .and_then(|c| {
                    c.execute_batch(
                        "UPDATE core_utxos SET value = value + 1 \
                         WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)",
                    )
                })
                .map_err(StoreUpgradeError::sqlite("change a live row", false))?;
        }
    }
    let kept = if again {
        Some(start(
            network_dir,
            adopters,
            &config,
            now + Duration::from_secs(60),
        )?)
    } else {
        None
    };
    Ok(BetweenState {
        config,
        snapshot,
        kept,
    })
}

/// A start up to the open: prepare, then reconcile, which must be pending.
fn start(
    network_dir: &Path,
    adopters: &Adopters,
    config: &SqlitePersisterConfig,
    now: SystemTime,
) -> Result<SnapshotInfo> {
    let snapshot = prepare_with(network_dir, adopters, now)?
        .snapshot
        .ok_or_else(|| StoreUpgradeError::ForkStoreShapeMismatch {
            object: "the fork fixture did not read as a fork store".into(),
        })?;
    match reconcile_fork_store(config, &snapshot, adopters)? {
        Outcome::Pending { .. } => Ok(snapshot),
        other => Err(StoreUpgradeError::ForkStoreShapeMismatch {
            object: format!("reconcile was {other:?}, not pending"),
        }),
    }
}

fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    crate::fsx::create_private_dir(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
