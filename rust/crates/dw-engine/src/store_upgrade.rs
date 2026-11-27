//! The M3 store upgrade at a network's open (dw-store-upgrade): a requested
//! or part-done restore, then the refusal check, snapshot and reconcile
//! before any store opens, the persister opened only through
//! `PersisterOpened`, each refusal as a typed error carrying the crate's own
//! screen (`copy::screen_for`), and a restore that stopped the open by its
//! state (DTO contract v3).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dw_store_upgrade::copy::{ProvedState, RefusedScreen, restore_foreign, screen_for};
use dw_store_upgrade::{
    Adopters, IncompleteCause, PendingRestore, PersisterOpened, Precheck, RefusalCause,
    RestoreInfo, SnapshotState, StoreUpgradeError, UpgradeNotice, prepare_with,
    reconcile_fork_store,
};
use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};

use crate::error::EngineError;

/// The store upgrade's screens (`screen_for`), for the hosts.
pub use dw_store_upgrade::copy;

/// The start of the `internal` message a refusal without a screen maps to,
/// followed by its `StoreUpgradeError` variant name.
pub const NO_SCREEN: &str = "store refusal without a screen: ";

/// Where in the open the refusal came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreUpgradeStage {
    /// `run_pending_restore`, before the refusal check, or one of the
    /// restore commands (request, keep, cancel).
    Restore,
    /// The refusal check and snapshot, before anything in the folder is
    /// written.
    Prepare,
    /// The reconcile, before any store opens.
    Reconcile,
    /// `PersisterOpened::open`.
    Open,
}

/// A store-upgrade refusal: nothing in `folder` was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreRefusal {
    /// `copy::screen_for(e, ProvedState::read(folder, wrote))`, `{folder}`
    /// not yet filled in.
    pub screen: RefusedScreen,
    /// The network directory.
    pub folder: PathBuf,
    /// The `StoreUpgradeError` variant, for tests and logs.
    pub cause: &'static str,
    pub stage: StoreUpgradeStage,
    /// The crate's error text, for logs.
    pub detail: String,
}

/// Where a restore that stopped the open, or a refused restore command,
/// stands: the `storeUpgrade.restore.<state>.*` screen
/// `copy::restore_screen_for` gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreState {
    /// The request can't be honoured; it was removed and nothing changed.
    Refused,
    /// Not enough disk space for the restore's copies; nothing changed.
    NoSpace,
    /// Stopped before its journal: nothing changed, the request stays.
    NotStarted,
    /// Stopped after its journal: no store opens until a start finishes it.
    Incomplete,
    /// The live files are not the restored lineage (or, under the unshipped
    /// Refuse switch, another build wrote to them): nothing moved; Keep runs
    /// `keep_current_files`.
    Foreign,
}

/// An open whose store upgrade stopped on the machine and runs again at the
/// next start (`copy::retry_copy_for`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetryFailure {
    /// The network directory.
    pub folder: PathBuf,
    /// The crate's error text, for logs.
    pub detail: String,
}

/// A restore that stopped the open, or a refused restore command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreFailure {
    pub state: RestoreState,
    /// `refused`: missingBackup, mismatchedBackup or garbledMarker;
    /// `noSpace`: diskFull; `notStarted` and `incomplete`: diskFull,
    /// permissionDenied, fileVanished or other; `notStarted` also
    /// shieldedSpendInFlight or shieldedSpendCarriedOver (a shielded hold
    /// the restore would replace, which the screen names); `incomplete`
    /// also holdsUnchecked; `foreign`: otherLineage or oldBuildWrote.
    pub cause: &'static str,
    /// The extra space the copies need (`noSpace` only).
    pub needed_bytes: Option<u64>,
    /// The network directory.
    pub folder: PathBuf,
    /// The crate's error text, for logs.
    pub detail: String,
}

/// The persister config of a network directory: the one the reconcile
/// rehearses with and [`open_persister`] opens with. Strict (Q78 R6), with
/// upstream's auto-backup directory.
pub(crate) fn persister_config(network_dir: &Path) -> SqlitePersisterConfig {
    SqlitePersisterConfig::new(network_dir.join(crate::session::WALLET_DB_FILE))
        .with_load_policy(LoadPolicy::Strict)
}

/// The data roots this process has swept.
static SWEPT: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Remove the private copies a crashed store-upgrade check left under
/// `data_root`, under its instance lock, once per data root per process: a
/// second engine on the same root shares the lock, and its sweep would
/// delete a check the first engine is running. Returns whether this call
/// swept.
pub(crate) fn sweep_once(data_root: &Path) -> bool {
    let key = data_root
        .canonicalize()
        .unwrap_or_else(|_| data_root.to_path_buf());
    let mut swept = SWEPT.lock().unwrap_or_else(|p| p.into_inner());
    order::record(data_root, order::Step::Sweep);
    if swept.contains(&key) {
        return false;
    }
    match dw_store_upgrade::sweep_stale_scratch(data_root) {
        Ok(0) => {}
        Ok(n) => tracing::info!(removed = n, "store upgrade: stale scratch copies removed"),
        Err(e) => tracing::warn!(error = %e, "store upgrade: stale scratch copies not removed"),
    }
    swept.push(key);
    true
}

/// The restore step, blocking, under the data root's instance lock and
/// before the refusal check and any store of `network_dir` opens: a
/// requested restore runs, a part-done one finishes, or nothing is pending.
/// Any error stops the open. A requested restore first reads the live
/// stores' shielded holds ([`restore_hold`](crate::platform::restore_hold));
/// one held, or a store that does not read, stops it before anything moves
/// (`notStarted`, `shieldedSpendInFlight` or `shieldedSpendCarriedOver`),
/// the request kept.
fn run_pending_restore(network_dir: &Path) -> Result<Option<RestoreInfo>, EngineError> {
    order::record(network_dir, order::Step::Restore);
    let found = std::cell::Cell::new(None);
    let holds = |dir: &Path| match crate::platform::restore_hold::restore_hold(dir) {
        Some(hold) => {
            found.set(Some(hold));
            Err(format!("a shielded spend is held ({hold:?})"))
        }
        None => Ok(()),
    };
    let info = dw_store_upgrade::run_pending_restore(network_dir, SystemTime::now(), &holds)
        .map_err(|e| {
            let held = matches!(
                e,
                StoreUpgradeError::RestoreIncomplete {
                    cause: IncompleteCause::HoldsOpen,
                    ..
                }
            );
            match map_error(network_dir, StoreUpgradeStage::Restore, e, false) {
                EngineError::StoreRestore(mut r) if held => {
                    r.cause = hold_cause(found.get());
                    EngineError::StoreRestore(r)
                }
                e => e,
            }
        })?;
    if let Some(info) = &info {
        tracing::info!(?info, "store upgrade: wallet data restored");
    }
    Ok(info)
}

/// Steps 1 and 2 of the crate's startup order, blocking, under the data
/// root's instance lock and before any store of `network_dir` opens: a
/// pending restore, this build's probe, the refusal check and snapshot, and
/// the reconcile straight after with `config`. A refusal changes nothing in
/// `network_dir`. The notice is for the session to carry once the
/// persister has opened and the wallets have loaded ([`copy::notice_line`]).
pub(crate) fn prepare_and_reconcile(
    network_dir: &Path,
    config: &SqlitePersisterConfig,
) -> Result<Option<UpgradeNotice>, EngineError> {
    let data_root = network_dir
        .parent()
        .ok_or_else(|| EngineError::Internal(format!("{} has no parent", network_dir.display())))?;
    let restored = run_pending_restore(network_dir)?;
    let adopters = Adopters::for_build(data_root);
    let prepared = prepare_with(network_dir, adopters, SystemTime::now()).map_err(|e| {
        map_error(
            network_dir,
            StoreUpgradeStage::Prepare,
            e,
            restored.is_some(),
        )
    })?;
    let wrote = restored.is_some() || prepared.restored.is_some();
    let failed = |stage, e| map_error(network_dir, stage, e, wrote);
    order::record(network_dir, order::Step::Prepare);
    match prepared.precheck {
        Precheck::NotForked => Ok(None),
        Precheck::Refuse { reason } => Err(failed(StoreUpgradeStage::Prepare, reason)),
        Precheck::NeedsLockedCheck => Err(EngineError::Internal(
            "store upgrade: prepare left the locked check undone".into(),
        )),
        Precheck::Fork { versions } => {
            let snapshot = prepared.snapshot.ok_or_else(|| {
                EngineError::Internal("store upgrade: a fork file without its snapshot".into())
            })?;
            let outcome = reconcile_fork_store(config, &snapshot, adopters)
                .map_err(|e| failed(StoreUpgradeStage::Reconcile, e))?;
            order::record(network_dir, order::Step::Reconcile);
            tracing::info!(?versions, ?outcome, snapshot = %snapshot.dir().display(),
                "store upgrade: fork store reconciled");
            Ok(outcome.notice())
        }
    }
}

/// Step 3: the stores may open from here, recorded for the startup-order
/// pin.
pub(crate) fn stores_opening(network_dir: &Path) {
    dw_store_upgrade::note_stores_opening();
    order::record(network_dir, order::Step::StoresOpening);
}

/// Step 4: the persister, opened only through the crate's token, which the
/// caller keeps as long as the persister.
pub(crate) fn open_persister(
    network_dir: &Path,
    config: SqlitePersisterConfig,
) -> Result<(Arc<SqlitePersister>, PersisterOpened), EngineError> {
    order::record(network_dir, order::Step::Persister);
    PersisterOpened::open(config)
        .map_err(|e| map_error(network_dir, StoreUpgradeStage::Open, e, false))
}

/// Whether `network_dir`'s wallet data is a restore the engine has not yet
/// cleared (the RESTORED mark). A mark that can't be checked reads as set,
/// so the holds stay on (Q102 102-3).
pub fn restored(network_dir: &Path) -> bool {
    dw_store_upgrade::restored_mark_present(network_dir)
}

/// [`EngineError::RestoreOnly`] while [`restored`] reads the mark set: a
/// new wallet, a file or key import and a watch-only wallet wait for the
/// mark to clear; a wallet comes only from its recovery phrase.
pub fn refuse_while_restored(network_dir: &Path) -> Result<(), EngineError> {
    if restored(network_dir) {
        return Err(EngineError::RestoreOnly);
    }
    Ok(())
}

/// Whether `network_dir` has a verified before-upgrade snapshot to restore.
pub(crate) fn restore_available(network_dir: &Path) -> Result<bool, EngineError> {
    dw_store_upgrade::snapshot_present(network_dir)
        .map_err(|e| EngineError::RetryableIo(format!("read the upgrade backups: {e}")))
}

/// Ask for the restore of the verified snapshot at the next start. While a
/// restore is replacing files, `storage_restore` incomplete: that one can
/// only be finished, whatever the snapshot reads as. While the live stores
/// keep a shielded hold the restore would replace, or don't read
/// ([`restore_hold`](crate::platform::restore_hold)),
/// [`EngineError::ShieldedSpendHeld`]. Without a verified snapshot, refused
/// (missingBackup); a snapshot that changed since it was read, refused
/// (mismatchedBackup). Nothing else is written.
pub(crate) fn request_restore(network_dir: &Path) -> Result<(), EngineError> {
    let refused = |cause: RefusalCause, detail: String| {
        EngineError::StoreRestore(Box::new(RestoreFailure {
            state: RestoreState::Refused,
            cause: refusal_cause(cause),
            needed_bytes: None,
            folder: network_dir.to_path_buf(),
            detail,
        }))
    };
    let replacing = dw_store_upgrade::pending_restore(network_dir)
        .map_err(|e| EngineError::RetryableIo(format!("read the restore marker: {e}")))?;
    if replacing == Some(PendingRestore::InProgress) {
        return Err(map_error(
            network_dir,
            StoreUpgradeStage::Restore,
            StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::Other,
                journal_written: true,
                detail: "the restore has begun replacing files and can only be finished".into(),
            },
            false,
        ));
    }
    if let Some(hold) = crate::platform::restore_hold::restore_hold(network_dir) {
        return Err(EngineError::ShieldedSpendHeld(hold));
    }
    match dw_store_upgrade::snapshot_state(network_dir)
        .map_err(|e| EngineError::RetryableIo(format!("read the upgrade backups: {e}")))?
    {
        SnapshotState::Verified(info) => dw_store_upgrade::request_restore(network_dir, &info)
            .map_err(|e| match e {
                StoreUpgradeError::SnapshotInvalid { .. } => {
                    refused(RefusalCause::MismatchedBackup, e.to_string())
                }
                e => map_error(network_dir, StoreUpgradeStage::Restore, e, false),
            }),
        SnapshotState::Unverified { detail, .. } => {
            Err(refused(RefusalCause::MissingBackup, detail))
        }
        SnapshotState::None => Err(refused(
            RefusalCause::MissingBackup,
            "no before-upgrade snapshot".into(),
        )),
    }
}

/// The foreign screen's Keep: keep the live files as they are and end the
/// restore. Nothing part-done at S4 is a no-op; the open is tried again.
pub(crate) fn keep_current_files(network_dir: &Path) -> Result<(), EngineError> {
    dw_store_upgrade::keep_current_files(network_dir, SystemTime::now())
        .map(|kept| {
            if let Some(info) = kept {
                tracing::info!(?info, "store upgrade: kept the current wallet files");
            }
        })
        .map_err(|e| map_error(network_dir, StoreUpgradeStage::Restore, e, false))
}

/// Drop a restore that hasn't begun replacing files.
pub(crate) fn cancel_restore(network_dir: &Path) -> Result<(), EngineError> {
    dw_store_upgrade::cancel_restore(network_dir)
        .map_err(|e| map_error(network_dir, StoreUpgradeStage::Restore, e, false))
}

/// The `StoreUpgradeError` variant's name. Exhaustive: a new variant must
/// be named, and mapped by [`map_error`], before this compiles.
pub(crate) fn variant_name(e: &StoreUpgradeError) -> &'static str {
    use StoreUpgradeError as E;
    match e {
        E::ForkStoreUnsupported { .. } => "ForkStoreUnsupported",
        E::ForkStoreShapeMismatch { .. } => "ForkStoreShapeMismatch",
        E::RetiredPoolWithoutAdopter { .. } => "RetiredPoolWithoutAdopter",
        E::P9aBroken { .. } => "P9aBroken",
        E::UpgradeRehearsalFailed { .. } => "UpgradeRehearsalFailed",
        E::PreHistoryFork => "PreHistoryFork",
        E::SchemaTooNew { .. } => "SchemaTooNew",
        E::SchemaHistoryMismatch { .. } => "SchemaHistoryMismatch",
        E::SnapshotInvalid { .. } => "SnapshotInvalid",
        E::SnapshotFailed { .. } => "SnapshotFailed",
        E::RestorePending { .. } => "RestorePending",
        E::RestoreRefused { .. } => "RestoreRefused",
        E::RestoreNoSpace { .. } => "RestoreNoSpace",
        E::RestoreIncomplete { .. } => "RestoreIncomplete",
        E::RestoreAfterOpen => "RestoreAfterOpen",
        E::UpgradeCopyUnwritable { .. } => "UpgradeCopyUnwritable",
        E::SnapshotSourceDamaged { .. } => "SnapshotSourceDamaged",
        E::WalletFileUnsupported { .. } => "WalletFileUnsupported",
        E::StoreHeld { .. } => "StoreHeld",
        E::NonStrictOpen { .. } => "NonStrictOpen",
        E::OpenConfigUnsupported { .. } => "OpenConfigUnsupported",
        E::ProbeNotRun => "ProbeNotRun",
        E::PersisterOpen { .. } => "PersisterOpen",
        E::Sqlite { .. } => "Sqlite",
        E::Io { .. } => "Io",
    }
}

fn refusal_cause(c: RefusalCause) -> &'static str {
    match c {
        RefusalCause::MissingBackup => "missingBackup",
        RefusalCause::MismatchedBackup => "mismatchedBackup",
        RefusalCause::GarbledMarker => "garbledMarker",
    }
}

fn incomplete_cause(c: IncompleteCause) -> &'static str {
    match c {
        IncompleteCause::DiskFull => "diskFull",
        IncompleteCause::PermissionDenied => "permissionDenied",
        IncompleteCause::FileVanished => "fileVanished",
        IncompleteCause::S4OldBuildWrote => "oldBuildWrote",
        IncompleteCause::S4OtherLineage => "otherLineage",
        IncompleteCause::HoldsOpen => hold_cause(None),
        IncompleteCause::HoldsUnchecked => "holdsUnchecked",
        IncompleteCause::Other => "other",
    }
}

/// Whether `cause` is a `notStarted` restore's shielded hold ([`hold_cause`]).
pub(crate) fn is_hold_cause(cause: &str) -> bool {
    [Some(crate::ShieldedHold::InFlight), None]
        .into_iter()
        .any(|hold| hold_cause(hold) == cause)
}

/// The `notStarted` cause of a restore a shielded hold stopped; one not
/// known is carried over.
fn hold_cause(hold: Option<crate::ShieldedHold>) -> &'static str {
    match hold {
        Some(crate::ShieldedHold::InFlight) => "shieldedSpendInFlight",
        Some(crate::ShieldedHold::CarriedOver) | None => "shieldedSpendCarriedOver",
    }
}

/// The state and cause of an error that has a restore screen
/// (`copy::restore_screen_for`, the crate's one decision point, the foreign
/// screen included); `None` for every other error.
fn restore_state(e: &StoreUpgradeError) -> Option<(RestoreState, &'static str, Option<u64>)> {
    use StoreUpgradeError as E;
    let screen = dw_store_upgrade::copy::restore_screen_for(e)?;
    let foreign = screen == restore_foreign::screen();
    Some(match e {
        E::RestoreRefused { cause } => (RestoreState::Refused, refusal_cause(*cause), None),
        E::RestoreNoSpace { needed } => (RestoreState::NoSpace, "diskFull", *needed),
        E::RestoreIncomplete { cause, .. } if foreign => {
            (RestoreState::Foreign, incomplete_cause(*cause), None)
        }
        E::RestoreIncomplete {
            cause,
            journal_written: true,
            ..
        } => (RestoreState::Incomplete, incomplete_cause(*cause), None),
        E::RestoreIncomplete { cause, .. } => {
            (RestoreState::NotStarted, incomplete_cause(*cause), None)
        }
        _ => return None,
    })
}

/// A crate error as the engine's:
/// - a refusal ([`StoreUpgradeError::is_refusal`]) as
///   [`EngineError::StoreRefused`] with the crate's screen, its way out by
///   the folder's proved state (a backup only when verified, 98-5; `wrote`:
///   this start changed the files); a refusal without a screen as
///   `Internal`, never a generic screen;
/// - an error with a restore screen (`copy::restore_screen_for`) as
///   [`EngineError::StoreRestore`] by the screen's state, the foreign
///   lineage's included;
/// - a held store as `StorageInUse`, held until a restart when the restore
///   step met it;
/// - a wallet file that isn't a database, or is corrupt, as `Storage` when
///   SQLite reports it outside the snapshot step (the snapshot step reports
///   a damaged store as `SnapshotSourceDamaged`, a refusal);
/// - a failure of the machine (disk full or not writable, an I/O fault)
///   while the open upgrades the store (prepare, reconcile, the persister
///   open) as [`EngineError::StoreRetry`], the crate's retry screen
///   (`copy::retry_copy_for`); any other, and any in the restore commands,
///   as a retryable io error; never a refusal;
/// - upstream's own open error by its usual mapping;
/// - a caller-side state (non-Strict open, probe not run, a config the
///   reconcile didn't rehearse, a snapshot that doesn't fit, a pending
///   restore, a restore after a store opened) as `Internal`.
pub(crate) fn map_error(
    network_dir: &Path,
    stage: StoreUpgradeStage,
    e: StoreUpgradeError,
    wrote: bool,
) -> EngineError {
    use StoreUpgradeError as E;
    let cause = variant_name(&e);
    tracing::warn!(cause, ?stage, error = %e, folder = %network_dir.display(),
        "store upgrade: open stopped");
    if e.is_refusal() {
        return match screen_for(&e, ProvedState::read(network_dir, wrote)) {
            Some(mut screen) => {
                if screen.way_out == copy::WayOut::Unverified
                    && dw_store_upgrade::snapshot_retired_for_wallet_removal(network_dir)
                {
                    screen.offer_recover_after_wallet_removal();
                }
                EngineError::StoreRefused(Box::new(StoreRefusal {
                    screen,
                    folder: network_dir.to_path_buf(),
                    cause,
                    stage,
                    detail: e.to_string(),
                }))
            }
            None => EngineError::Internal(format!("{NO_SCREEN}{cause}")),
        };
    }
    if let Some((state, cause, needed_bytes)) = restore_state(&e) {
        return EngineError::StoreRestore(Box::new(RestoreFailure {
            state,
            cause,
            needed_bytes,
            folder: network_dir.to_path_buf(),
            detail: e.to_string(),
        }));
    }
    let damaged = matches!(&e, E::Sqlite { source, .. } if is_damaged(source));
    let upgrading = matches!(
        stage,
        StoreUpgradeStage::Prepare | StoreUpgradeStage::Reconcile | StoreUpgradeStage::Open
    );
    if upgrading && !damaged && dw_store_upgrade::copy::retry_copy_for(&e).is_some() {
        return EngineError::StoreRetry(Box::new(RetryFailure {
            folder: network_dir.to_path_buf(),
            detail: e.to_string(),
        }));
    }
    match e {
        E::StoreHeld { .. } if stage == StoreUpgradeStage::Restore => EngineError::StorageInUse {
            detail: e.to_string(),
            retryable: false,
        },
        E::StoreHeld { .. } => EngineError::storage_in_use(e.to_string()),
        E::PersisterOpen { source, .. } => EngineError::from(*source),
        E::Sqlite { ref source, .. } if is_damaged(source) => EngineError::Storage(e.to_string()),
        E::SnapshotFailed { .. } | E::Sqlite { .. } | E::Io { .. } => {
            EngineError::RetryableIo(e.to_string())
        }
        _ => EngineError::Internal(format!("store upgrade: {cause}: {e}")),
    }
}

/// A wallet file that isn't a database or is corrupt: retrying can't help.
fn is_damaged(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::NotADatabase | rusqlite::ErrorCode::DatabaseCorrupt)
    )
}

/// The open's store steps in the order they ran, per data root (tests
/// only): the startup-order pin.
pub(crate) mod order {
    use std::path::Path;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Step {
        Instance,
        Sweep,
        Restore,
        Prepare,
        Reconcile,
        StoresOpening,
        Vault,
        AppDb,
        StoreRepair,
        Persister,
    }

    #[cfg(test)]
    static STEPS: std::sync::Mutex<Vec<(std::path::PathBuf, Step)>> =
        std::sync::Mutex::new(Vec::new());

    /// Record `step` for the data root `dir` is, or is in.
    #[cfg_attr(not(test), allow(unused_variables))]
    pub(crate) fn record(dir: &Path, step: Step) {
        #[cfg(test)]
        {
            let root = match step {
                Step::Instance | Step::Sweep => dir,
                _ => dir.parent().unwrap_or(dir),
            };
            (STEPS.lock().unwrap_or_else(|p| p.into_inner())).push((root.to_path_buf(), step));
        }
    }

    #[cfg(test)]
    pub(crate) fn steps(root: &Path) -> Vec<Step> {
        (STEPS.lock().unwrap_or_else(|p| p.into_inner()).iter())
            .filter(|(r, _)| r == root)
            .map(|(_, s)| *s)
            .collect()
    }
}

#[cfg(test)]
#[path = "store_upgrade_tests.rs"]
mod tests;
