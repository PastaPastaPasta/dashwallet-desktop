//! M3 store upgrade: moves wallet files written by released fork-pinned
//! desktop builds onto upstream platform-wallet-storage.
//!
//! Startup order for one network directory `{folder}` (all before any store
//! opens, in `spawn_blocking`):
//! 1. [`prepare`]: the refusal check ([`precheck`], [`precheck_locked`]),
//!    then the snapshot ([`take_snapshot`]) for a fork file. A refusal
//!    writes, renames and locks nothing in `{folder}`. A pending restore
//!    keeps every store closed ([`StoreUpgradeError::RestorePending`]).
//! 2. [`note_stores_opening`], then vault and `app.sqlite`.
//! 3. [`reconcile_fork_store`] just before the persister.
//! 4. [`PersisterOpened::open`].

mod adopters;
mod classify;
pub mod copy;
mod error;
mod fork;
mod fsx;
mod precheck;
mod reconcile;
mod restore;
mod shapes;
mod snapshot;
mod sql;
mod token;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::SystemTime;

pub use adopters::{Adopters, EmbeddedMigration, P9aStatus, p9a_present, r1d_line_required};
pub use error::{Result, StoreUpgradeError};
pub use precheck::{Precheck, precheck, precheck_locked, precheck_locked_with, precheck_with};
pub use reconcile::{Outcome, reconcile_fork_store};
pub use restore::{PendingRestore, RESTORE_JOURNAL, RESTORE_REQUESTED, pending_restore};
pub use snapshot::{
    STORE_FILES, SnapshotFile, SnapshotInfo, UPGRADE_BACKUPS_DIR, latest_snapshot,
    snapshot_present, take_snapshot,
};
pub use token::{PersisterOpened, note_stores_opening, stores_opened_in_process};

/// What [`prepare`] found and did.
#[derive(Debug)]
pub struct Prepared {
    pub precheck: Precheck,
    /// The pre-upgrade snapshot, for a fork file.
    pub snapshot: Option<SnapshotInfo>,
}

/// Step 1 of the startup order, for this build's probed adopters.
pub fn prepare(network_dir: &Path, now: SystemTime) -> Result<Prepared> {
    prepare_with(
        network_dir,
        Adopters::for_build(precheck::data_root(network_dir)),
        now,
    )
}

/// Step 1 for `adopters`: the refusal check, then the snapshot. A refusal is
/// returned as [`Precheck::Refuse`] with nothing in `{folder}` written.
pub fn prepare_with(network_dir: &Path, adopters: &Adopters, now: SystemTime) -> Result<Prepared> {
    let mut checked = precheck_with(network_dir, adopters)?;
    if matches!(checked, Precheck::NeedsLockedCheck) {
        checked = precheck_locked_with(network_dir, adopters)?;
    }
    let snapshot = match &checked {
        Precheck::Fork { .. } => Some(take_snapshot(network_dir, now)?),
        _ => None,
    };
    Ok(Prepared {
        precheck: checked,
        snapshot,
    })
}
