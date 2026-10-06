//! OS services of dashwallet-desktop on Windows and Linux (DESIGN-opus
//! §1.13, docs/contracts/m2-engine.md §2.10). macOS uses AppKit and
//! SwiftUI for the same jobs, so the dw-ffi wrappers report `Unsupported`
//! there for everything except [`qr`] and [`logs`], which are pure Rust and
//! work on every OS.
//!
//! - [`instance`]: one primary process per key; later launches forward
//!   their arguments (URIs) to it and exit (QT-001). Unix: a socket in the
//!   user's runtime directory guarded by a lock file; Windows: a named
//!   mutex and a named pipe (not built or run on Windows yet).
//! - [`autostart`]: "Start on system login" (QT-009). Linux: XDG autostart
//!   entry; Windows: the per-user `Run` registry value (not run yet).
//! - [`uri_schemes`]: per-user URI scheme registration for unpackaged
//!   builds (QT-150, IOS-048). Linux: a `.desktop` file plus
//!   `mimeapps.list` defaults; Windows: `HKCU\Software\Classes`.
//! - [`notify`]: transaction notifications (QT-031). Linux: `notify-send`;
//!   Windows: a toast through PowerShell (not run yet).
//! - [`qr`]: every QR code in a PNG/JPEG/BMP image (IOS-043).
//! - [`logs`]: zips the log files for a support request (IOS-112).
//! - [`capture`]: Windows screen-capture exclusion (IOS-006).
//!
//! There is no tray icon here: a StatusNotifierItem needs a D-Bus stack
//! (`ksni`/`zbus`) and `Shell_NotifyIcon` a Win32 message loop; neither is
//! built yet, so the hosts keep the window (see the contract).

#![cfg_attr(test, allow(non_snake_case))] // test names carry checklist ids

pub mod autostart;
pub mod capture;
mod desktop_entry;
mod frame;
pub mod instance;
pub mod logs;
pub mod notify;
pub mod qr;
pub mod uri_schemes;

/// Errors of this crate. Variants mirror the `desktop.*` codes of
/// docs/contracts/m2-engine.md §4 plus `invalid_argument`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DesktopError {
    /// This OS or desktop session does not offer the feature; the string
    /// names it for the log.
    #[error("{0} unsupported here")]
    Unsupported(String),
    /// An OS call, file operation or helper program failed.
    #[error("os error: {0}")]
    OsError(String),
    /// The image holds no readable QR code.
    #[error("no QR code found")]
    NoQrCode,
    /// Not a PNG/JPEG/BMP image the decoder reads, or too large.
    #[error("image unreadable: {0}")]
    ImageUnreadable(String),
    /// A caller argument is malformed.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl DesktopError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unsupported(_) => "desktop.unsupported",
            Self::OsError(_) => "desktop.os_error",
            Self::NoQrCode => "desktop.no_qr_code",
            Self::ImageUnreadable(_) => "desktop.image_unreadable",
            Self::InvalidArgument(_) => "invalid_argument",
        }
    }
}

impl From<std::io::Error> for DesktopError {
    fn from(e: std::io::Error) -> Self {
        Self::OsError(e.to_string())
    }
}

/// Checks a name that becomes part of a file, pipe or registry path:
/// 1–64 ASCII letters, digits, `.`, `_` or `-`, not starting with `.`.
pub(crate) fn check_name(what: &str, name: &str) -> Result<(), DesktopError> {
    let ok = (1..=64).contains(&name.len())
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(DesktopError::InvalidArgument(format!(
            "{what} {name:?} must be 1-64 of [A-Za-z0-9._-] and not start with '.'"
        )))
    }
}

/// Searches `PATH` for an executable named `program`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn find_program(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        for good in ["DashWallet-mainnet", "org.dash.Wallet", "a_b"] {
            check_name("key", good).unwrap();
        }
        for bad in ["", ".hidden", "a/b", "a b", "../x", "é", &"x".repeat(65)] {
            assert!(check_name("key", bad).is_err(), "{bad:?}");
        }
    }
}
