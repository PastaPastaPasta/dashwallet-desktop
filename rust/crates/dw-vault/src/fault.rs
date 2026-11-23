//! Debug builds with `fault-inject` (and this crate's tests): one-shot
//! failures of a vault write, armed for one vault directory, after the new
//! file was renamed into place. The write then fails although the new file
//! is on disk, so a caller can't tell what the vault holds (MG-03 D31).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where the armed write fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The directory's fsync after the rename (the file is on disk, the
    /// vault's memory still has the old one).
    DirSync,
    /// The read-back check after the write (the file is on disk and in
    /// memory).
    ReadBack,
}

static ARMED: Mutex<Vec<(PathBuf, Fault)>> = Mutex::new(Vec::new());

/// Makes the next write of the vault in `dir` fail at `fault`, once.
pub fn arm(dir: &Path, fault: Fault) {
    ARMED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((dir.to_path_buf(), fault));
}

/// Whether `fault` was armed for `dir`; disarms it.
pub(crate) fn take(dir: &Path, fault: Fault) -> bool {
    let mut armed = ARMED.lock().unwrap_or_else(|p| p.into_inner());
    match armed.iter().position(|(d, f)| d == dir && *f == fault) {
        Some(i) => {
            armed.remove(i);
            true
        }
        None => false,
    }
}
