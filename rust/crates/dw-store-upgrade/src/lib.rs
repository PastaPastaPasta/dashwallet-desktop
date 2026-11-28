//! M3 store upgrade: moves wallet files written by released fork-pinned
//! desktop builds onto upstream platform-wallet-storage.
//!
//! Startup order for one network directory `{folder}`, all under the data
//! root's instance lock and before any store opens, in `spawn_blocking`:
//! 0. [`sweep_stale_scratch`], once per start, before any check: removes the
//!    checks' and the probe's private directories a crash left under the
//!    data root. The probe ([`Adopters::for_build`]) runs inside the first
//!    check and removes its own directory before it returns.
//! 1. [`prepare`]: a requested or part-done restore first
//!    ([`run_pending_restore`]), then the refusal check ([`precheck`],
//!    [`precheck_locked`]),
//!    the upgrade rehearsed on a private copy, then the snapshot
//!    ([`take_snapshot`]) for a fork file. The refusal check itself writes,
//!    renames and locks nothing in `{folder}`; a restore before it may have.
//!    A restore that stops keeps every store closed until a start finishes
//!    it.
//! 2. [`reconcile_fork_store`] straight after, before any store opens, with
//!    the `SqlitePersisterConfig` step 4 will use: the upgrade is rehearsed
//!    again with those settings, and the snapshot must fingerprint all four
//!    stores as they are.
//! 3. [`note_stores_opening`], then vault and `app.sqlite`.
//! 4. [`PersisterOpened::open`] with that same config.
//!
//! A fork file's start copies `wallet.sqlite` (with its log) twice into
//! scratch, once per rehearsal, besides the snapshot of all four stores: it
//! needs about three times the wallet's size free under the data root.

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

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::SystemTime;

pub use adopters::{Adopters, EmbeddedMigration, P9aStatus, p9a_present, r1d_line_required};
pub use error::{Result, StoreUpgradeError};
pub use precheck::{
    Precheck, precheck, precheck_locked, precheck_locked_with, precheck_with, sweep_stale_scratch,
};
pub use reconcile::{Outcome, UpgradeNotice, reconcile_fork_store};
pub use restore::{
    HoldCheck, IncompleteCause, KEPT, PendingPayment, PendingRestore, RESTORE_ATTEMPT,
    RESTORE_JOURNAL, RESTORE_REQUESTED, RESTORED_MARK, RefusalCause, RestoreInfo, RestoredMark,
    S4_POLICY, S4Policy, SAFE_TO_DELETE, cancel_restore, clear_restored_mark_on_first_all_synced,
    keep_current_files, pending_restore, request_restore, restore, restored_mark,
    restored_mark_present, run_pending_restore, touched_paths,
};
pub use snapshot::{
    MissingSnapshot, NOT_THIS_WALLET, PART_UPGRADED_BY, STORE_FILES, SnapshotFile, SnapshotInfo,
    SnapshotState, UPGRADE_BACKUPS_DIR, UPGRADED_FROM, latest_snapshot,
    retire_snapshot_for_wallet_removal, snapshot_present, snapshot_retired_for_wallet_removal,
    snapshot_state, take_snapshot, upgrade_copy_present,
};
pub use sql::open_immutable;
pub use token::{PersisterOpened, note_stores_opening, stores_opened_in_process};

/// What [`prepare`] found and did.
#[derive(Debug)]
pub struct Prepared {
    /// A restore this start finished, before the check.
    pub restored: Option<RestoreInfo>,
    pub precheck: Precheck,
    /// The pre-upgrade snapshot, for a fork file.
    pub snapshot: Option<SnapshotInfo>,
}

/// Step 1 of the startup order, for this build's probed adopters.
pub fn prepare(network_dir: &Path, now: SystemTime) -> Result<Prepared> {
    prepare_with(
        network_dir,
        Adopters::for_build(precheck::parent_dir(network_dir)?),
        now,
    )
}

/// Step 1 for `adopters`: a pending restore, the refusal check, then the
/// snapshot. A refusal is returned as [`Precheck::Refuse`] with nothing in
/// `{folder}` written. A requested restore that has not begun is refused
/// here (`HoldsOpen`): the caller runs [`run_pending_restore`] with its
/// [`HoldCheck`] first, or [`prepare_with_holds`].
pub fn prepare_with(network_dir: &Path, adopters: &Adopters, now: SystemTime) -> Result<Prepared> {
    prepare_with_holds(network_dir, adopters, now, &|_, _| {
        Err("no hold check ran before the restore".into())
    })
}

/// [`prepare_with`], a requested restore run once `holds` passed.
pub fn prepare_with_holds(
    network_dir: &Path,
    adopters: &Adopters,
    now: SystemTime,
    holds: HoldCheck<'_>,
) -> Result<Prepared> {
    let restored = run_pending_restore(network_dir, now, holds)?;
    let mut checked = precheck_with(network_dir, adopters)?;
    if matches!(checked, Precheck::NeedsLockedCheck) {
        checked = precheck_locked_with(network_dir, adopters)?;
    }
    let snapshot = match &checked {
        Precheck::Fork { .. } => Some(take_snapshot(network_dir, now)?),
        _ => None,
    };
    Ok(Prepared {
        restored,
        precheck: checked,
        snapshot,
    })
}
