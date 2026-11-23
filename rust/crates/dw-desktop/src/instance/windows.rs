//! Windows single instance: the named mutex `Local\<key>` decides who is
//! primary (session-local); the primary serves `\\.\pipe\<key>-<user>`.
//!
//! UNVERIFIED: written against the Win32 API (windows-sys 0.59) but not
//! built or run on Windows yet; no Windows runner exists (DESIGN.md R2 G1).
//!
//! The first pipe instance is created with `FILE_FLAG_FIRST_PIPE_INSTANCE`,
//! so a process that squatted the name makes `start` fail instead of
//! receiving our arguments; `PIPE_REJECT_REMOTE_CLIENTS` keeps it local.
//! The default pipe DACL gives other users read access only, so they
//! cannot send a frame.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

use super::ForwardHandler;
use crate::{DesktopError, frame};

const STARTUP_WAIT: Duration = Duration::from_secs(3);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The user name made safe for a pipe name.
fn user_tag() -> String {
    std::env::var("USERNAME")
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

fn pipe_name(key: &str) -> String {
    format!(r"\\.\pipe\{key}-{}", user_tag())
}

struct OwnedHandle(HANDLE);

// SAFETY: a mutex handle may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful Create* call and is
        // closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn create_pipe(name: &[u16], first: bool) -> Result<HANDLE, DesktopError> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    // SAFETY: `name` is NUL-terminated UTF-16; null security attributes
    // select the default DACL.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            4096,
            64 * 1024,
            0,
            std::ptr::null(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: no preconditions.
        let code = unsafe { GetLastError() };
        return Err(DesktopError::OsError(format!(
            "CreateNamedPipeW failed ({code})"
        )));
    }
    Ok(handle)
}

pub(crate) struct Listener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pipe: String,
    _mutex: OwnedHandle,
}

impl Listener {
    pub(crate) fn start(key: &str, handler: ForwardHandler) -> Result<Option<Self>, DesktopError> {
        let mutex_name = wide(&format!(r"Local\{key}"));
        // SAFETY: NUL-terminated name, default security, not initially owned.
        let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
        // SAFETY: no preconditions; read right after the call it describes.
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if mutex.is_null() {
            return Err(DesktopError::OsError("CreateMutexW failed".into()));
        }
        let mutex = OwnedHandle(mutex);
        if already {
            return Ok(None);
        }
        let pipe = pipe_name(key);
        let wide_pipe = wide(&pipe);
        let first = create_pipe(&wide_pipe, true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = Arc::clone(&stop);
            let first = first as usize;
            std::thread::Builder::new()
                .name("dw-desktop-instance".into())
                .spawn(move || serve(wide_pipe, first as HANDLE, stop, handler))?
        };
        Ok(Some(Self {
            stop,
            thread: Some(thread),
            pipe,
            _mutex: mutex,
        }))
    }

    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Wake the blocking ConnectNamedPipe.
        let _ = OpenOptions::new().read(true).write(true).open(&self.pipe);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(name: Vec<u16>, first: HANDLE, stop: Arc<AtomicBool>, handler: ForwardHandler) {
    let mut next = Some(first);
    loop {
        let handle = match next.take() {
            Some(h) => h,
            None => match create_pipe(&name, false) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(error = %e, "single-instance pipe stopped");
                    return;
                }
            },
        };
        // SAFETY: `handle` is a pipe instance we own; synchronous connect.
        let connected = unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) } != 0
            // SAFETY: no preconditions.
            || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        // SAFETY: the File takes ownership and closes the instance on drop.
        let mut file = unsafe { File::from_raw_handle(handle as RawHandle) };
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if connected {
            match frame::read(&mut file) {
                Some(args) => {
                    handler(args);
                    let _ = file.write_all(&[frame::ACK]);
                    let _ = file.flush();
                }
                None => tracing::warn!("ignored a malformed single-instance message"),
            }
        }
    }
}

pub(crate) fn forward(key: &str, args: &[String]) -> Result<bool, DesktopError> {
    let pipe = pipe_name(key);
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        match OpenOptions::new().read(true).write(true).open(&pipe) {
            Ok(mut file) => {
                frame::write(&mut file, args)?;
                let mut ack = [0u8; 1];
                file.read_exact(&mut ack)?;
                return Ok(ack[0] == frame::ACK);
            }
            Err(e) => {
                // No pipe and no mutex: no primary.
                let mutex_name = wide(&format!(r"Local\{key}"));
                // SAFETY: as in `Listener::start`.
                let probe = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
                // SAFETY: no preconditions.
                let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
                if !probe.is_null() {
                    drop(OwnedHandle(probe));
                }
                if !exists {
                    return Ok(false);
                }
                if Instant::now() >= deadline {
                    return Err(DesktopError::OsError(format!(
                        "the primary instance holds {key} but its pipe does not open: {e}"
                    )));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
