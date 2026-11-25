//! The M3 store upgrade at a network's open (dw-store-upgrade): the refusal
//! check, snapshot and reconcile before any store opens, the persister
//! opened only through `PersisterOpened`, and each refusal as a typed error
//! carrying the crate's own screen (`copy::screen_for`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use dw_store_upgrade::copy::{ScreenCopy, screen_for};
use dw_store_upgrade::{
    Adopters, PersisterOpened, Precheck, StoreUpgradeError, latest_snapshot, prepare_with,
    reconcile_fork_store,
};
use platform_wallet_storage::{LoadPolicy, SqlitePersister, SqlitePersisterConfig};

use crate::error::EngineError;

/// The store upgrade's screens (`screen_for`), for the hosts.
pub use dw_store_upgrade::copy;

/// Whether `storage_in_use` may clear without a restart. False at this
/// platform pin: a closed session's persister stays held in the process
/// until it ends. True from the pin carrying the persister-release fix
/// (P16); `storage_in_use_retryable_matches_an_in_process_reopen` ties it
/// to that behaviour.
pub const STORAGE_IN_USE_RETRYABLE: bool = false;

/// Where in the open the refusal came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreUpgradeStage {
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
    /// `screen_for(e, backup_present)`, `{folder}` not yet filled in.
    pub screen: ScreenCopy,
    /// The network directory.
    pub folder: PathBuf,
    /// The `StoreUpgradeError` variant, for tests and logs.
    pub cause: &'static str,
    pub stage: StoreUpgradeStage,
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

/// Steps 1 and 2 of the crate's startup order, blocking, under the data
/// root's instance lock and before any store of `network_dir` opens: this
/// build's probe, the refusal check and snapshot, and the reconcile straight
/// after with `config`. A refusal changes nothing in `network_dir`.
pub(crate) fn prepare_and_reconcile(
    network_dir: &Path,
    config: &SqlitePersisterConfig,
) -> Result<(), EngineError> {
    let data_root = network_dir
        .parent()
        .ok_or_else(|| EngineError::Internal(format!("{} has no parent", network_dir.display())))?;
    let adopters = Adopters::for_build(data_root);
    let failed = |stage| move |e| map_error(network_dir, stage, e);
    let prepared = prepare_with(network_dir, adopters, SystemTime::now())
        .map_err(failed(StoreUpgradeStage::Prepare))?;
    order::record(network_dir, order::Step::Prepare);
    match prepared.precheck {
        Precheck::NotForked => Ok(()),
        Precheck::Refuse { reason } => Err(failed(StoreUpgradeStage::Prepare)(reason)),
        Precheck::NeedsLockedCheck => Err(EngineError::Internal(
            "store upgrade: prepare left the locked check undone".into(),
        )),
        Precheck::Fork { versions } => {
            let snapshot = prepared.snapshot.ok_or_else(|| {
                EngineError::Internal("store upgrade: a fork file without its snapshot".into())
            })?;
            let outcome = reconcile_fork_store(config, &snapshot, adopters)
                .map_err(failed(StoreUpgradeStage::Reconcile))?;
            order::record(network_dir, order::Step::Reconcile);
            tracing::info!(?versions, ?outcome, snapshot = %snapshot.dir().display(),
                "store upgrade: fork store reconciled");
            Ok(())
        }
    }
}

/// Step 4: the persister, opened only through the crate's token, which the
/// caller keeps as long as the persister.
pub(crate) fn open_persister(
    config: SqlitePersisterConfig,
) -> Result<(Arc<SqlitePersister>, PersisterOpened), EngineError> {
    let network_dir = config
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    order::record(&network_dir, order::Step::Persister);
    PersisterOpened::open(config).map_err(|e| map_error(&network_dir, StoreUpgradeStage::Open, e))
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
        E::StoreHeld { .. } => "StoreHeld",
        E::NonStrictOpen { .. } => "NonStrictOpen",
        E::OpenConfigUnsupported { .. } => "OpenConfigUnsupported",
        E::ProbeNotRun => "ProbeNotRun",
        E::PersisterOpen { .. } => "PersisterOpen",
        E::Sqlite { .. } => "Sqlite",
        E::Io { .. } => "Io",
    }
}

/// A crate error as the engine's:
/// - a refusal ([`StoreUpgradeError::is_refusal`]) as
///   [`EngineError::StoreRefused`] with the crate's screen, its backup
///   sentence only for a verified snapshot; a refusal without a screen as
///   `Internal`, never a generic screen;
/// - a held store as `StorageInUse`;
/// - a failure of the machine (disk, I/O, the snapshot, the rehearsal's
///   I/O) as a retryable io error, never a refusal;
/// - upstream's own open error by its usual mapping;
/// - a caller-side state (non-Strict open, probe not run, a config the
///   reconcile didn't rehearse, a snapshot that doesn't fit, a pending
///   restore) as `Internal`.
pub(crate) fn map_error(
    network_dir: &Path,
    stage: StoreUpgradeStage,
    e: StoreUpgradeError,
) -> EngineError {
    use StoreUpgradeError as E;
    let cause = variant_name(&e);
    tracing::warn!(cause, ?stage, error = %e, folder = %network_dir.display(),
        "store upgrade: open stopped");
    if e.is_refusal() {
        let backup_present = latest_snapshot(network_dir).is_ok_and(|s| s.is_some());
        return match screen_for(&e, backup_present) {
            Some(screen) => EngineError::StoreRefused(StoreRefusal {
                screen,
                folder: network_dir.to_path_buf(),
                cause,
                stage,
                detail: e.to_string(),
            }),
            None => EngineError::Internal(format!("store refusal without a screen: {cause}")),
        };
    }
    match e {
        E::StoreHeld { .. } => EngineError::StorageInUse(e.to_string()),
        E::PersisterOpen { source, .. } => EngineError::from(*source),
        E::SnapshotFailed { .. } | E::Sqlite { .. } | E::Io { .. } => {
            EngineError::RetryableIo(e.to_string())
        }
        _ => EngineError::Internal(format!("store upgrade: {cause}: {e}")),
    }
}

/// The open's store steps in the order they ran, per data root (tests
/// only): the startup-order pin.
pub(crate) mod order {
    use std::path::Path;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Step {
        Instance,
        Sweep,
        Prepare,
        Reconcile,
        StoresOpening,
        Vault,
        AppDb,
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
