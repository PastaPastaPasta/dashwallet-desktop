//! Unix single instance: `<runtime dir>/<key>.lock` (held with `flock` by
//! the primary for its lifetime) and `<runtime dir>/<key>.sock` (a stream
//! socket the primary listens on).
//!
//! The runtime directory is `$XDG_RUNTIME_DIR` (per user, mode 0700, as the
//! XDG spec requires; inside Flatpak it is the sandbox's own), else
//! `<temp dir>/dashwallet-<uid>`, created with mode 0700 and refused when
//! another user owns it or others can enter it. The socket is mode 0600.
//! The lock file is never deleted: deleting it while another process opens
//! it could let two processes each lock a different file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::ForwardHandler;
use crate::{DesktopError, frame};

/// How long a peer may take to send its frame.
const PEER_TIMEOUT: Duration = Duration::from_secs(2);
/// How long `forward` waits for a primary that holds the lock but is not
/// accepting yet (it is starting).
const STARTUP_WAIT: Duration = Duration::from_secs(3);

pub(crate) fn runtime_dir() -> Result<PathBuf, DesktopError> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)
        && dir.is_absolute()
        && dir.is_dir()
    {
        return Ok(dir);
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("dashwallet-{uid}"));
    match std::fs::create_dir(&dir) {
        Ok(()) => std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(DesktopError::OsError(format!(
            "{} is not a private directory of this user",
            dir.display()
        )));
    }
    Ok(dir)
}

fn paths(key: &str) -> Result<(PathBuf, PathBuf), DesktopError> {
    let dir = runtime_dir()?;
    Ok((
        dir.join(format!("{key}.lock")),
        dir.join(format!("{key}.sock")),
    ))
}

fn open_lock(path: &Path) -> Result<File, DesktopError> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?)
}

/// Whether some process holds the lock of `path` now.
fn lock_held(path: &Path) -> Result<bool, DesktopError> {
    let file = open_lock(path)?;
    match file.try_lock() {
        Ok(()) => Ok(false),
        Err(std::fs::TryLockError::WouldBlock) => Ok(true),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
    // `file` closes here, which drops a lock this call took.
}

pub(crate) struct Listener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    socket: PathBuf,
    /// Holding the open, locked file keeps this process the primary.
    _lock: File,
}

impl Listener {
    pub(crate) fn start(key: &str, handler: ForwardHandler) -> Result<Option<Self>, DesktopError> {
        let (lock_path, socket) = paths(key)?;
        let lock = open_lock(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        // We hold the lock, so a socket file is a leftover of a dead primary.
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("dw-desktop-instance".into())
                .spawn(move || serve(listener, stop, handler))?
        };
        Ok(Some(Self {
            stop,
            thread: Some(thread),
            socket,
            _lock: lock,
        }))
    }

    /// Stops the listener thread, removes the socket and drops the lock.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking accept.
        let _ = UnixStream::connect(&self.socket);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.socket);
    }
}

fn serve(listener: UnixListener, stop: Arc<AtomicBool>, handler: ForwardHandler) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(mut stream) = stream else { continue };
        let _ = stream.set_read_timeout(Some(PEER_TIMEOUT));
        let _ = stream.set_write_timeout(Some(PEER_TIMEOUT));
        match frame::read(&mut stream) {
            Some(args) => {
                handler(args);
                let _ = stream.write_all(&[frame::ACK]);
            }
            None => tracing::warn!("ignored a malformed single-instance message"),
        }
    }
}

pub(crate) fn forward(key: &str, args: &[String]) -> Result<bool, DesktopError> {
    let (lock_path, socket) = paths(key)?;
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        match UnixStream::connect(&socket) {
            Ok(mut stream) => {
                stream.set_write_timeout(Some(PEER_TIMEOUT))?;
                stream.set_read_timeout(Some(PEER_TIMEOUT))?;
                frame::write(&mut stream, args)?;
                let mut ack = [0u8; 1];
                stream.read_exact(&mut ack)?;
                return Ok(ack[0] == frame::ACK);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) =>
            {
                if !lock_held(&lock_path)? {
                    return Ok(false);
                }
                if Instant::now() >= deadline {
                    return Err(DesktopError::OsError(format!(
                        "the primary instance holds {key} but does not accept connections"
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.into()),
        }
    }
}
