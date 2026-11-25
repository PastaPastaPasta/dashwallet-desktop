//! The reconciler (design §4 with Q78 R1): runs before the persister opens,
//! after the snapshot is durable.

use std::collections::BTreeSet;
use std::path::Path;

use platform_wallet_storage::{LoadPolicy, SqlitePersisterConfig};
use rusqlite::Connection;

use crate::adopters::Adopters;
use crate::classify::{Classification, ForkState, classify};
use crate::error::{Result, StoreUpgradeError};
use crate::fork;
use crate::fsx::fault;
use crate::precheck::{Precheck, check_all};
use crate::snapshot::{self, MissingSnapshot, SnapshotInfo};
use crate::sql::{has_column, open_existing};

/// What the reconciler did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No fork history: nothing to do.
    NotForked,
    /// Fork file accepted, R1's column dropped if it was still there; the
    /// persister's migrations adopt the fork objects. The fork history is
    /// retired on a later start. `notice` is for the engine to show once
    /// the persister has opened and loaded.
    Pending {
        versions: BTreeSet<u32>,
        notice: Option<UpgradeNotice>,
    },
    /// Upstream had migrated it on an earlier start: the fork history is now
    /// `dw_fork_schema_history_retired`.
    Retired { versions: BTreeSet<u32> },
}

impl Outcome {
    /// The one-time notice this outcome carries: only a `Pending` start
    /// whose snapshot is a `part-upgraded` copy because the before-upgrade
    /// one doesn't check out or isn't this wallet's.
    pub fn notice(&self) -> Option<UpgradeNotice> {
        match self {
            Self::Pending { notice, .. } => *notice,
            _ => None,
        }
    }
}

/// What the engine tells the user once after the upgrade opened the wallet
/// (Q107 107-1, [`crate::copy::notice_line`]). It blocks nothing. A missing
/// before-upgrade copy gets none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpgradeNotice {
    /// The before-upgrade snapshot doesn't verify.
    SnapshotDamaged,
    /// The before-upgrade snapshot verifies but is of other data; it is now
    /// marked [`crate::NOT_THIS_WALLET`].
    SnapshotOther,
}

impl UpgradeNotice {
    fn of(snapshot: &SnapshotInfo) -> Option<Self> {
        match snapshot.missing()? {
            MissingSnapshot::Absent => None,
            MissingSnapshot::Unverified => Some(Self::SnapshotDamaged),
            MissingSnapshot::Mismatch => Some(Self::SnapshotOther),
        }
    }
}

const DROP_SQL: &str = "ALTER TABLE contacts DROP COLUMN external_account_reference";
const RETIRE_SQL: &str =
    "ALTER TABLE dw_fork_schema_history RENAME TO dw_fork_schema_history_retired";

/// R1 alone, in its own transaction: the probe's path through the same SQL.
pub(crate) fn drop_external_reference(conn: &Connection) -> rusqlite::Result<()> {
    if !has_column(conn, fork::CONTACTS, fork::EXTERNAL_REF)? {
        return Ok(());
    }
    conn.execute_batch(&format!("BEGIN IMMEDIATE; {DROP_SQL}; COMMIT;"))
}

/// The snapshot handed in is a completed snapshot of this directory, its
/// files matching the marker, taken of this fork state.
fn check_snapshot(
    network_dir: &Path,
    snapshot: &SnapshotInfo,
    versions: &BTreeSet<u32>,
) -> Result<()> {
    let invalid = |detail: String| StoreUpgradeError::SnapshotInvalid {
        dir: snapshot.dir().to_path_buf(),
        detail,
    };
    let root = snapshot::backups_dir(network_dir)
        .canonicalize()
        .map_err(|e| invalid(format!("{}: {e}", snapshot::UPGRADE_BACKUPS_DIR)))?;
    let dir = snapshot
        .dir()
        .canonicalize()
        .map_err(|e| invalid(e.to_string()))?;
    if dir.parent() != Some(root.as_path()) {
        return Err(invalid("not in this directory's upgrade-backups".into()));
    }
    let on_disk = snapshot::read_snapshot(&dir, true).map_err(invalid)?;
    if on_disk.manifest_sha256() != snapshot.manifest_sha256() {
        return Err(invalid("marker differs from the snapshot handed in".into()));
    }
    if on_disk.fork_versions() != versions {
        return Err(invalid(format!(
            "taken of fork versions {:?}, the file has {versions:?}",
            on_disk.fork_versions()
        )));
    }
    Ok(())
}

/// The config the persister will open with can carry the upgrade: Strict,
/// with auto-backups on (upstream backs up before its pending migrations and
/// fails without a directory, after R1's drop has committed).
fn refuse_unsupported_config(config: &SqlitePersisterConfig) -> Result<()> {
    if config.load_policy != LoadPolicy::Strict {
        return Err(StoreUpgradeError::NonStrictOpen {
            policy: format!("{:?}", config.load_policy),
        });
    }
    if config.auto_backup_dir.is_none() {
        return Err(StoreUpgradeError::OpenConfigUnsupported {
            detail: "auto_backup_dir is None; the upgrade's migrations need it".into(),
        });
    }
    Ok(())
}

/// Upstream's check of the auto-backup directory, before the drop: created
/// owner-only and written to. A failure is a retryable I/O error with the
/// wallet file unchanged.
fn ensure_backup_dir(dir: &Path) -> Result<()> {
    let io = StoreUpgradeError::io("check the auto-backup directory", true);
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir).map_err(&io)?;
    tempfile::NamedTempFile::new_in(dir).map(drop).map_err(io)
}

/// Reconcile `config.path` (a network directory's `wallet.sqlite`) for this
/// build. `config` must be the one [`crate::PersisterOpened::open`] gets next:
/// the upgrade is rehearsed on a private copy with its settings, and the open
/// refuses any other. Every refusal is decided by a read that writes nothing;
/// the writes (R1's drop, the retire) run in one transaction after the
/// file is checked again to be one upstream opens (not a symbolic link, not
/// another user's) and the snapshot to be of the data held under that
/// transaction's lock, so a crash leaves the file as before or as after. Never opens with
/// `LoadPolicy::Recovery`.
pub fn reconcile_fork_store(
    config: &SqlitePersisterConfig,
    snapshot: &SnapshotInfo,
    adopters: &Adopters,
) -> Result<Outcome> {
    let wallet_db = config.path.as_path();
    let network_dir = crate::precheck::parent_dir(wallet_db)?;
    crate::token::refuse_if_held(network_dir)?;
    crate::restore::refuse_if_restore_pending(network_dir)?;
    refuse_unsupported_config(config)?;
    let versions = match check_all(network_dir, adopters, config)? {
        Precheck::NotForked => return Ok(Outcome::NotForked),
        Precheck::Refuse { reason } => return Err(reason),
        Precheck::NeedsLockedCheck => unreachable!("check_all never asks again"),
        Precheck::Fork { versions } => versions,
    };
    check_snapshot(network_dir, snapshot, &versions)?;

    let conn =
        open_existing(wallet_db).map_err(StoreUpgradeError::sqlite("open wallet.sqlite", true))?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(StoreUpgradeError::sqlite("lock wallet.sqlite", true))?;
    #[cfg(test)]
    if let Some(hook) = LOCKED_FOR_TEST.with(|h| h.take()) {
        hook(network_dir);
    }
    let written = crate::precheck::require_openable_file(network_dir)
        .and_then(|()| snapshot::require_reusable(snapshot, &conn, network_dir))
        .and_then(|()| {
            write_in_transaction(
                &conn,
                adopters,
                &versions,
                config.auto_backup_dir.as_deref(),
            )
        })
        .map(|outcome| match outcome {
            Outcome::Pending { versions, .. } => Outcome::Pending {
                versions,
                notice: UpgradeNotice::of(snapshot),
            },
            other => other,
        });
    match written {
        Ok(outcome) => {
            conn.execute_batch("COMMIT")
                .map_err(StoreUpgradeError::sqlite("commit the reconcile", false))?;
            crate::token::note_reconciled(network_dir, config);
            tracing::info!(?outcome, "store upgrade: reconciled");
            Ok(outcome)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Runs once on the network directory right after `BEGIN IMMEDIATE`.
    pub(crate) static LOCKED_FOR_TEST: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

/// The writes under the lock. `backup_dir` is checked only when upstream
/// will still migrate, and so back up, at the open that follows.
fn write_in_transaction(
    conn: &Connection,
    adopters: &Adopters,
    versions: &BTreeSet<u32>,
    backup_dir: Option<&Path>,
) -> Result<Outcome> {
    let state: ForkState = match classify(conn, adopters)? {
        Classification::Fork(state) if &state.versions == versions => state,
        Classification::Fork(state) => {
            return Err(StoreUpgradeError::ForkStoreShapeMismatch {
                object: format!(
                    "fork versions changed to {:?} during the upgrade",
                    state.versions
                ),
            });
        }
        Classification::Refuse(reason) => return Err(reason),
        Classification::NotForked => {
            return Err(StoreUpgradeError::ForkStoreShapeMismatch {
                object: "fork history vanished during the upgrade".into(),
            });
        }
    };
    if !state.history_complete
        && let Some(dir) = backup_dir
    {
        ensure_backup_dir(dir)?;
    }
    if state.column_present {
        conn.execute_batch(DROP_SQL)
            .map_err(StoreUpgradeError::sqlite(
                "drop contacts.external_account_reference",
                true,
            ))?;
        fault("reconcile.after_drop")?;
        let reference = crate::shapes::Reference::build(adopters)?;
        if !reference.contacts_is_fresh(conn)? {
            return Err(StoreUpgradeError::ForkStoreShapeMismatch {
                object: "contacts after the drop differs from this build's".into(),
            });
        }
    }
    if !state.history_complete {
        return Ok(Outcome::Pending {
            versions: state.versions,
            notice: None,
        });
    }
    conn.execute_batch(RETIRE_SQL)
        .map_err(StoreUpgradeError::sqlite("retire the fork history", true))?;
    fault("reconcile.after_retire")?;
    Ok(Outcome::Retired {
        versions: state.versions,
    })
}
