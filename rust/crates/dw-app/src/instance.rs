//! One process per data root (U-01 D2). The root is resolved once, before
//! anything is named after it, and the process that opens its wallet holds
//! an exclusive lock in it until it exits.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::error::{AppCode, AppError};

/// The data root of this launch: `explicit` (`--datadir`), else `default`
/// (the platform's app-data directory, where Tauri's `app_data_dir` finds
/// it), created (0700 when new) and then canonicalized. The instance name,
/// the lock, the window-state file and the engine all take this one path,
/// so every spelling of a root meets the same instance: the default root
/// and the same directory given as `--datadir`, and a root under a
/// symlinked parent before and after it exists.
pub fn data_root(
    explicit: Option<&Path>,
    default: impl FnOnce() -> Option<PathBuf>,
) -> io::Result<PathBuf> {
    let dir = match explicit {
        Some(dir) => dir.to_path_buf(),
        None => default().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "the platform has no app-data directory",
            )
        })?,
    };
    dw_fs::create_private_dir(&dir)?;
    dunce::canonicalize(&dir)
}

/// The exclusive advisory lock (`flock` on Unix, `LockFileEx` on Windows)
/// on `<root>/instance.lock`, held while the value lives and released by
/// the OS when the process ends, however it ends. The single-instance
/// plugin hands a second launch's links to the first where it can; this
/// lock is the backstop where it cannot (no D-Bus session bus, a direct
/// macOS binary launch, another Windows session), so that two processes
/// never open one wallet.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// The lock file's name in the data root.
    pub const FILE: &str = "instance.lock";

    /// How long `run` waits for the lock: a launch during the previous
    /// instance's shutdown (its engine flush, after the plugin has given up
    /// the D-Bus name) gets it once that process is gone.
    pub const LAUNCH_WAIT: Duration = Duration::from_secs(3);

    /// Takes the lock of the existing data root `root`, retrying for up to
    /// `wait`, or fails with `app.already_running` while another process
    /// holds it. The file is opened as the one name of a regular file
    /// (`dw_fs::open_sole_file`: never through a symlink, 0600 when made).
    pub fn acquire(root: &Path, wait: Duration) -> Result<Self, AppError> {
        let path = root.join(Self::FILE);
        let file = dw_fs::open_sole_file(&path).map_err(|e| {
            AppError::app(
                AppCode::DataDir,
                format!("cannot open {}: {e}", path.display()),
            )
        })?;
        let deadline = Instant::now() + wait;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(InstanceLock { _file: file }),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(AppError::app(
                        AppCode::AlreadyRunning,
                        format!(
                            "another Dash Wallet is already running on {}",
                            root.display()
                        ),
                    ));
                }
                Err(TryLockError::Error(e)) => {
                    return Err(AppError::app(
                        AppCode::DataDir,
                        format!("cannot lock {}: {e}", path.display()),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Set to a data root: `lock_probe` tries its lock and exits 0 (taken)
    /// or 3 (`app.already_running`).
    const PROBE: &str = "DW_INSTANCE_LOCK_PROBE";

    #[test]
    fn the_default_root_and_the_same_datadir_are_one_root() {
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().join("Dash Wallet");
        // Before the root exists (the default), then after (`--datadir`).
        let default = data_root(None, || Some(root.clone())).unwrap();
        assert!(root.is_dir());
        let explicit = data_root(Some(&root), || unreachable!("--datadir given")).unwrap();
        assert_eq!(default, explicit);
        assert_eq!(default, dunce::canonicalize(&root).unwrap());
        assert!(data_root(None, || None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_parent_gives_one_root_before_and_after_creation() {
        use std::os::unix::fs::PermissionsExt;
        let dir = dw_testutil::private_tempdir();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let via_link = dir.path().join("link/wallet");
        let before = data_root(Some(&via_link), || unreachable!()).unwrap();
        let after = data_root(Some(&via_link), || unreachable!()).unwrap();
        let direct = data_root(Some(&real.join("wallet")), || unreachable!()).unwrap();
        assert_eq!(before, after);
        assert_eq!(before, direct);
        assert_eq!(before, dunce::canonicalize(real.join("wallet")).unwrap());
        let mode = std::fs::metadata(&before).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    }

    #[test]
    fn one_process_per_data_root() {
        let dir = dw_testutil::private_tempdir();
        let root = data_root(Some(&dir.path().join("root")), || unreachable!()).unwrap();
        // Another process, so that the lock is tested across processes as
        // the app takes it (this test binary, running only `lock_probe`).
        let probe = || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "instance::tests::lock_probe", "--test-threads=1"])
                .env(PROBE, &root)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap()
                .code()
        };
        let held = InstanceLock::acquire(&root, Duration::ZERO).unwrap();
        assert_eq!(probe(), Some(3), "a second process must not take the lock");
        // Within one process too: a second open of the file is refused.
        let again = InstanceLock::acquire(&root, Duration::ZERO).unwrap_err();
        assert_eq!(again.code_str(), AppCode::AlreadyRunning.as_str());
        drop(held);
        assert_eq!(probe(), Some(0), "the lock is free once its holder is gone");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(root.join(InstanceLock::FILE)).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn a_launch_waits_for_a_holder_that_is_shutting_down() {
        let dir = dw_testutil::private_tempdir();
        let root = data_root(Some(&dir.path().join("root")), || unreachable!()).unwrap();
        let held = InstanceLock::acquire(&root, Duration::ZERO).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(held);
        });
        InstanceLock::acquire(&root, Duration::from_secs(30))
            .expect("the lock, once its holder lets go");
        release.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_file_is_never_reached_through_a_symlink() {
        let dir = dw_testutil::private_tempdir();
        let root = data_root(Some(&dir.path().join("root")), || unreachable!()).unwrap();
        let elsewhere = dir.path().join("elsewhere");
        std::os::unix::fs::symlink(&elsewhere, root.join(InstanceLock::FILE)).unwrap();
        let e = InstanceLock::acquire(&root, Duration::ZERO).unwrap_err();
        assert_eq!(e.code_str(), AppCode::DataDir.as_str());
        assert!(!elsewhere.exists());
    }

    /// The other process of `one_process_per_data_root`; a no-op otherwise.
    #[test]
    fn lock_probe() {
        let Some(root) = std::env::var_os(PROBE) else {
            return;
        };
        let code = match InstanceLock::acquire(Path::new(&root), Duration::ZERO) {
            Ok(_) => 0,
            Err(e) if e.code_str() == AppCode::AlreadyRunning.as_str() => 3,
            Err(e) => {
                eprintln!("{e}");
                4
            }
        };
        std::process::exit(code);
    }
}
