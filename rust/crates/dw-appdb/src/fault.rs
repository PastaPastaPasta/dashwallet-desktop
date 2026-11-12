//! Debug builds with `fault-inject` (and this crate's tests): one-shot
//! failures of a dispatch journal write, armed for one journal file.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Where the armed write fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The classification of the imported shielded artifacts, before its
    /// first write ([`crate::dispatch::DispatchJournal::classify_shielded_imports`]).
    ClassifyWrite,
}

static ARMED: Mutex<Vec<(PathBuf, Fault)>> = Mutex::new(Vec::new());

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Makes the next `fault` write of the journal at `path` fail, once.
pub fn arm(path: &Path, fault: Fault) {
    ARMED
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push((canonical(path), fault));
}

/// Whether `fault` was armed for the journal at `path` (`None` for an
/// in-memory one); disarms it.
pub(crate) fn take(path: Option<&str>, fault: Fault) -> bool {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return false;
    };
    let path = canonical(Path::new(path));
    let mut armed = ARMED.lock().unwrap_or_else(|p| p.into_inner());
    match armed.iter().position(|(p, f)| *p == path && *f == fault) {
        Some(i) => {
            armed.remove(i);
            true
        }
        None => false,
    }
}
