//! Screen-capture exclusion while a recovery phrase is visible (IOS-006).
//!
//! - Windows: `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
//!   (Windows 10 2004+; older versions refuse it and the call returns
//!   `false`). UNVERIFIED: not run on Windows yet.
//! - Linux: X11 and Wayland offer no such flag: `Unsupported`; the host
//!   shows a warning banner instead.
//! - macOS: `Unsupported` here (`NSWindow.sharingType = .none` in Swift).

use crate::DesktopError;

/// Turns capture exclusion of the window `hwnd` on or off. Returns whether
/// the requested state is in effect.
pub fn set_excluded(hwnd: u64, excluded: bool) -> Result<bool, DesktopError> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        };
        if hwnd == 0 {
            return Err(DesktopError::InvalidArgument("window handle 0".into()));
        }
        let affinity = if excluded {
            WDA_EXCLUDEFROMCAPTURE
        } else {
            WDA_NONE
        };
        // SAFETY: an invalid handle makes the call fail; it does not touch
        // memory through it.
        let ok = unsafe { SetWindowDisplayAffinity(hwnd as usize as _, affinity) } != 0;
        Ok(ok)
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, excluded);
        Err(DesktopError::Unsupported(
            "screen-capture exclusion on this OS".into(),
        ))
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[test]
    fn test_IOS_006_unsupported_off_windows() {
        assert!(matches!(
            set_excluded(1, true),
            Err(DesktopError::Unsupported(_))
        ));
    }
}
