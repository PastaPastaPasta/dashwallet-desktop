//! The reconciler (design §4 with Q78 R1): runs before the persister opens,
//! after the snapshot is durable.

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::Connection;

use crate::adopters::Adopters;
use crate::classify::{Classification, ForkState, classify};
use crate::error::{Result, StoreUpgradeError};
use crate::fork;
use crate::fsx::fault;
use crate::precheck::{Precheck, precheck_locked_with, precheck_with};
use crate::snapshot::{self, SnapshotInfo};
use crate::sql::{has_column, open_existing};

/// What the reconciler did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// No fork history: nothing to do.
    NotForked,
    /// Fork file accepted, R1's column dropped if it was still there; the
    /// persister's migrations adopt the fork objects. The fork history is
    /// retired on a later start.
    Pending { versions: BTreeSet<u32> },
    /// Upstream had migrated it on an earlier start: the fork history is now
    /// `dw_fork_schema_history_retired`.
    Retired { versions: BTreeSet<u32> },
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

/// Reconcile `wallet_db` (a network directory's `wallet.sqlite`) for this
/// build. Every refusal is decided by a read that writes nothing, the upgrade
/// rehearsed on a private copy first; the writes (R1's drop, the retire) run
/// in one transaction after the snapshot is checked to be of the data held
/// under that transaction's lock, so a crash leaves the file as before or as
/// after. Never opens with `LoadPolicy::Recovery`.
pub fn reconcile_fork_store(
    wallet_db: &Path,
    snapshot: &SnapshotInfo,
    adopters: &Adopters,
) -> Result<Outcome> {
    let network_dir = crate::precheck::parent_dir(wallet_db)?;
    crate::token::refuse_if_held(network_dir)?;
    crate::restore::refuse_if_restore_pending(network_dir)?;
    let read = match precheck_with(network_dir, adopters)? {
        Precheck::NeedsLockedCheck => precheck_locked_with(network_dir, adopters)?,
        other => other,
    };
    let versions = match read {
        Precheck::NotForked => return Ok(Outcome::NotForked),
        Precheck::Refuse { reason } => return Err(reason),
        Precheck::NeedsLockedCheck => unreachable!("precheck_locked never asks again"),
        Precheck::Fork { versions } => versions,
    };
    check_snapshot(network_dir, snapshot, &versions)?;

    let conn =
        open_existing(wallet_db).map_err(StoreUpgradeError::sqlite("open wallet.sqlite", true))?;
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(StoreUpgradeError::sqlite("lock wallet.sqlite", true))?;
    let written = snapshot::require_reusable(snapshot, &conn, network_dir)
        .and_then(|()| write_in_transaction(&conn, adopters, &versions));
    match written {
        Ok(outcome) => {
            conn.execute_batch("COMMIT")
                .map_err(StoreUpgradeError::sqlite("commit the reconcile", false))?;
            tracing::info!(?outcome, "store upgrade: reconciled");
            Ok(outcome)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn write_in_transaction(
    conn: &Connection,
    adopters: &Adopters,
    versions: &BTreeSet<u32>,
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
        });
    }
    conn.execute_batch(RETIRE_SQL)
        .map_err(StoreUpgradeError::sqlite("retire the fork history", true))?;
    fault("reconcile.after_retire")?;
    Ok(Outcome::Retired {
        versions: state.versions,
    })
}
