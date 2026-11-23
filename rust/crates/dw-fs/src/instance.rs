//! One process per data root (U-01 D2): the exclusive advisory lock
//! (`flock` on Unix, `LockFileEx` on Windows) on `<root>/instance.lock`,
//! held while a value lives and released by the OS when the process ends,
//! however it ends. Every client that opens a data root's wallets takes it:
//! the engine on open ([`InstanceLock::share`]), so the desktop app, the
//! Swift app and dwcli never open one root from two processes; the desktop
//! app before its engine ([`InstanceLock::acquire`]), so a second launch
//! stops before it opens anything.

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

/// The roots this process holds, by canonical path.
static HELD: LazyLock<Mutex<HashMap<PathBuf, Weak<Held>>>> = LazyLock::new(Mutex::default);

struct Held {
    key: PathBuf,
    file: Option<File>,
}

impl Drop for Held {
    /// Closes the file (the OS releases the lock) and forgets the root under
    /// the registry's lock, so a [`InstanceLock::share`] holding it sees the
    /// holder alive, mid-drop (its entry dead: it waits), or gone.
    fn drop(&mut self) {
        let mut roots = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        drop(self.file.take());
        if roots.get(&self.key).is_some_and(|w| w.strong_count() == 0) {
            roots.remove(&self.key);
        }
    }
}

/// How long [`InstanceLock::share`] waits for this process's last holder of
/// a root to finish dropping: closing one file under the registry's lock.
const DROP_WAIT: Duration = Duration::from_secs(3);

/// How long [`InstanceLock::share`] retries a root no holder of this
/// process has but whose lock is taken: a child forked from this process
/// keeps the lock until it execs, past the last holder's close; another
/// process's lock outlasts the wait and is refused. The wait a test gives
/// [`InstanceLock::acquire`] to take a root back once its holder is gone.
pub const BUSY_WAIT: Duration = Duration::from_millis(250);

/// The lock of one data root, shared by every value of this process that
/// holds it.
#[derive(Clone)]
pub struct InstanceLock {
    _held: Arc<Held>,
}

impl std::fmt::Debug for InstanceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceLock").finish_non_exhaustive()
    }
}

/// Why a root's lock was not taken.
#[derive(Debug)]
pub enum InstanceError {
    /// Another holder has it: another process, or (for
    /// [`InstanceLock::acquire`]) this one.
    Busy,
    /// The lock file could not be opened (not naming the path) or locked.
    Io(io::Error),
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("the data root is in use by another process"),
            Self::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for InstanceError {}

impl InstanceLock {
    /// The lock file's name in the data root.
    pub const FILE: &str = "instance.lock";

    /// Takes the lock of the existing data root `root`, retrying for up to
    /// `wait`; [`InstanceError::Busy`] while any other holder has it, in
    /// this process too. The file is opened as the one name of a regular
    /// file ([`crate::open_sole_file`]: never through a symlink, 0600 when
    /// made).
    pub fn acquire(root: &Path, wait: Duration) -> Result<Self, InstanceError> {
        let key = std::fs::canonicalize(root).map_err(InstanceError::Io)?;
        let file = locked(&key, wait)?;
        let mut roots = HELD.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Self::held(&mut roots, key, file))
    }

    /// The lock this process already holds on `root`, shared, else the
    /// root's lock taken as [`Self::acquire`] does: what an engine takes, so
    /// engines of one process (and the app's own lock) share a root while
    /// another process is refused. It waits for this process's last holder
    /// to finish dropping ([`DROP_WAIT`]) and retries a taken lock briefly
    /// ([`BUSY_WAIT`]).
    pub fn share(root: &Path) -> Result<Self, InstanceError> {
        let key = std::fs::canonicalize(root).map_err(InstanceError::Io)?;
        let started = Instant::now();
        let mut busy_since = None;
        loop {
            let mut roots = HELD.lock().unwrap_or_else(PoisonError::into_inner);
            let waited = match roots.get(&key).map(Weak::upgrade) {
                Some(Some(held)) => return Ok(Self { _held: held }),
                Some(None) => started.elapsed() >= DROP_WAIT,
                None => match locked(&key, Duration::ZERO) {
                    Ok(file) => return Ok(Self::held(&mut roots, key, file)),
                    Err(InstanceError::Busy) => {
                        busy_since.get_or_insert_with(Instant::now).elapsed() >= BUSY_WAIT
                    }
                    Err(e) => return Err(e),
                },
            };
            drop(roots);
            if waited {
                return Err(InstanceError::Busy);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn held(roots: &mut HashMap<PathBuf, Weak<Held>>, key: PathBuf, file: File) -> Self {
        let held = Arc::new(Held {
            key: key.clone(),
            file: Some(file),
        });
        roots.insert(key, Arc::downgrade(&held));
        Self { _held: held }
    }
}

/// `<root>/instance.lock` opened ([`crate::open_sole_file`]) and locked,
/// retrying for up to `wait`.
fn locked(root: &Path, wait: Duration) -> Result<File, InstanceError> {
    let file = crate::open_sole_file(&root.join(InstanceLock::FILE)).map_err(InstanceError::Io)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(TryLockError::WouldBlock) => return Err(InstanceError::Busy),
            Err(TryLockError::Error(e)) => return Err(InstanceError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One process's engines share a root with each other and with the
    /// app's own lock; a second `acquire` (a second launch's) is refused
    /// while any of them holds it, and the root is free once all are gone.
    #[test]
    fn engines_share_a_root_and_a_second_acquire_is_refused() {
        let dir = dw_testutil::private_tempdir();
        let app = InstanceLock::acquire(dir.path(), Duration::ZERO).unwrap();
        let engine = InstanceLock::share(dir.path()).unwrap();
        let other = InstanceLock::share(dir.path()).unwrap();
        drop(app);
        assert!(matches!(
            InstanceLock::acquire(dir.path(), Duration::ZERO),
            Err(InstanceError::Busy)
        ));
        drop((engine, other));
        InstanceLock::acquire(dir.path(), BUSY_WAIT).unwrap();
    }

    /// The last holder dropped on another thread while this one shares the
    /// root again (an engine made again in one process, as a host does after
    /// dropping its last): the share gets the lock still held or, once it is
    /// gone, the root again; never `Busy`.
    #[test]
    fn a_share_racing_the_last_holders_drop_is_never_refused() {
        let dir = dw_testutil::private_tempdir();
        for _ in 0..5000 {
            let held = InstanceLock::share(dir.path()).unwrap();
            let dropper = std::thread::spawn(move || drop(held));
            let again = InstanceLock::share(dir.path());
            dropper.join().unwrap();
            assert!(again.is_ok(), "{:?}", again.err());
        }
    }

    /// A root locked by no holder of this process, released a moment later
    /// (as a child forked before the last holder's close keeps it until it
    /// execs): the share waits for it. One held past the wait (another
    /// process's) is refused.
    #[test]
    fn a_share_waits_briefly_for_a_lock_this_process_does_not_hold() {
        let dir = dw_testutil::private_tempdir();
        let path = dir.path().join(InstanceLock::FILE);
        let other = crate::open_sole_file(&path).unwrap();
        other.try_lock().unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            drop(other);
        });
        InstanceLock::share(dir.path()).unwrap();
        release.join().unwrap();

        let dir = dw_testutil::private_tempdir();
        let other = crate::open_sole_file(&dir.path().join(InstanceLock::FILE)).unwrap();
        other.try_lock().unwrap();
        let started = Instant::now();
        assert!(matches!(
            InstanceLock::share(dir.path()),
            Err(InstanceError::Busy)
        ));
        assert!(started.elapsed() >= BUSY_WAIT);
    }
}
