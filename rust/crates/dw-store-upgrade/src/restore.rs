//! The restore markers the open path checks. A requested or part-done
//! restore keeps every store closed until the next start runs it.

use std::fs;
use std::io;
use std::path::Path;

use crate::error::{Result, StoreUpgradeError};
use crate::snapshot;

/// The user's request to restore the before-upgrade set at the next start.
pub const RESTORE_REQUESTED: &str = "RESTORE-REQUESTED";
/// A restore's journal: present while the swap is part-done.
pub const RESTORE_JOURNAL: &str = "RESTORE-IN-PROGRESS";

/// What [`pending_restore`] found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingRestore {
    Requested,
    InProgress,
}

/// Whether a restore is requested or part-done in `network_dir`.
pub fn pending_restore(network_dir: &Path) -> io::Result<Option<PendingRestore>> {
    let root = snapshot::backups_dir(network_dir);
    if fs::symlink_metadata(root.join(RESTORE_JOURNAL)).is_ok() {
        return Ok(Some(PendingRestore::InProgress));
    }
    match fs::symlink_metadata(root.join(RESTORE_REQUESTED)) {
        Ok(_) => Ok(Some(PendingRestore::Requested)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn refuse_if_restore_pending(network_dir: &Path) -> Result<()> {
    match pending_restore(network_dir) {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(StoreUpgradeError::RestorePending {
            dir: network_dir.to_path_buf(),
        }),
        Err(e) => Err(StoreUpgradeError::io("read the restore marker", true)(e)),
    }
}
