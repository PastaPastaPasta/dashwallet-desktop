//! Single instance and argument hand-off (QT-001, DESIGN-opus §1.13).
//!
//! The first process to [`acquire`] a key becomes the primary: it listens
//! for later launches and passes their arguments (URIs, file paths) to its
//! handler. A later launch finds the key taken, calls [`forward`] and exits.
//! A key is `DashWallet-<network>`, so each network has its own primary.
//!
//! Unix (Linux; also built and tested on macOS, where the app itself relies
//! on LaunchServices instead): a lock file and a stream socket in the
//! user's runtime directory. The `flock` on the lock file decides who is
//! primary, so a stale socket left by a crash never blocks a new primary.
//! Windows: a named mutex decides, a named pipe carries the arguments
//! (written against the Win32 API, not yet built or run on Windows).

use std::sync::Arc;

use crate::DesktopError;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

/// Receives the arguments of each later launch, on the listener thread.
/// Must return quickly.
pub type ForwardHandler = Arc<dyn Fn(Vec<String>) + Send + Sync>;

/// Held by the primary. Releasing it (or dropping it) stops the listener
/// and frees the key.
pub struct InstanceGuard {
    #[cfg(any(unix, windows))]
    inner: std::sync::Mutex<Option<platform::Listener>>,
}

impl std::fmt::Debug for InstanceGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceGuard").finish_non_exhaustive()
    }
}

impl InstanceGuard {
    /// Stops listening and frees the key. Idempotent.
    pub fn release(&self) {
        #[cfg(any(unix, windows))]
        {
            let listener = self
                .inner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .take();
            if let Some(listener) = listener {
                listener.stop();
            }
        }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        self.release();
    }
}

/// Becomes the primary for `key`, or returns `None` when another process
/// is. `key`: 1–64 of `[A-Za-z0-9._-]`.
pub fn acquire(key: &str, handler: ForwardHandler) -> Result<Option<InstanceGuard>, DesktopError> {
    crate::check_name("instance key", key)?;
    #[cfg(any(unix, windows))]
    {
        Ok(
            platform::Listener::start(key, handler)?.map(|listener| InstanceGuard {
                inner: std::sync::Mutex::new(Some(listener)),
            }),
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = handler;
        Err(DesktopError::Unsupported("single instance".into()))
    }
}

/// Sends `args` to the primary for `key`. `Ok(false)` when there is no
/// primary (the caller can become it). Waits up to a few seconds for a
/// primary that holds the key but is not listening yet.
pub fn forward(key: &str, args: &[String]) -> Result<bool, DesktopError> {
    crate::check_name("instance key", key)?;
    crate::frame::check_args(args)?;
    #[cfg(any(unix, windows))]
    {
        platform::forward(key, args)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(DesktopError::Unsupported("single instance".into()))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    /// Runs with a private runtime directory so tests do not meet each
    /// other or a real app.
    fn with_runtime_dir<T>(f: impl FnOnce() -> T) -> T {
        static LOCK: Mutex<()> = Mutex::new(());
        let _serial = LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: tests that touch the environment run one at a time under
        // `LOCK`, and nothing else in this crate's tests reads it.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", dir.path()) };
        let out = f();
        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        out
    }

    fn channel_handler() -> (ForwardHandler, mpsc::Receiver<Vec<String>>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (
            Arc::new(move |args| {
                let _ = tx.lock().unwrap().send(args);
            }),
            rx,
        )
    }

    #[test]
    fn test_QT_001_second_launch_forwards_its_uris_to_the_primary() {
        with_runtime_dir(|| {
            let (handler, rx) = channel_handler();
            let guard = acquire("DashWallet-test", handler)
                .unwrap()
                .expect("primary");
            let (other, _) = channel_handler();
            assert!(acquire("DashWallet-test", other).unwrap().is_none());

            let uris = vec!["dash:yQ1?amount=0.5".to_string(), "--min".into()];
            assert!(forward("DashWallet-test", &uris).unwrap());
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), uris);
            assert!(forward("DashWallet-test", &[]).unwrap());
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Vec::<String>::new()
            );

            // Another network is another key.
            assert!(!forward("DashWallet-testnet", &uris).unwrap());

            guard.release();
            guard.release();
            assert!(!forward("DashWallet-test", &uris).unwrap());
            // The key is free again.
            let (handler, _) = channel_handler();
            assert!(acquire("DashWallet-test", handler).unwrap().is_some());
        });
    }

    #[test]
    fn test_QT_001_a_stale_socket_does_not_block_a_new_primary() {
        with_runtime_dir(|| {
            let dir = unix::runtime_dir().unwrap();
            std::fs::write(dir.join("DashWallet-stale.sock"), b"left by a crash").unwrap();
            let (handler, rx) = channel_handler();
            let _guard = acquire("DashWallet-stale", handler)
                .unwrap()
                .expect("primary");
            assert!(forward("DashWallet-stale", &["dash:x".into()]).unwrap());
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                vec!["dash:x"]
            );
        });
    }

    #[test]
    fn test_QT_001_garbage_from_a_peer_is_ignored() {
        with_runtime_dir(|| {
            use std::io::Write;
            let (handler, rx) = channel_handler();
            let _guard = acquire("DashWallet-junk", handler)
                .unwrap()
                .expect("primary");
            let path = unix::runtime_dir().unwrap().join("DashWallet-junk.sock");
            let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
            s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
            drop(s);
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
            // The listener still serves well-formed peers.
            assert!(forward("DashWallet-junk", &["dash:y".into()]).unwrap());
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                vec!["dash:y"]
            );
        });
    }

    #[test]
    fn keys_and_arguments_are_checked() {
        let (handler, _) = channel_handler();
        assert!(matches!(
            acquire("../etc/passwd", handler),
            Err(DesktopError::InvalidArgument(_))
        ));
        assert!(matches!(
            forward("DashWallet-x", &["x".repeat(100_000)]),
            Err(DesktopError::InvalidArgument(_))
        ));
    }
}
