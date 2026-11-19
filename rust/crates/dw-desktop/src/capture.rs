//! Screen-capture exclusion while a recovery phrase is visible (IOS-006).
//!
//! - Windows: `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`
//!   (Windows 10 2004+; older versions refuse it and the call returns
//!   `false`).
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, GetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
        WS_OVERLAPPEDWINDOW,
    };

    fn affinity(hwnd: u64) -> u32 {
        let mut value = u32::MAX;
        // SAFETY: a live window of this thread and a valid out pointer.
        assert_ne!(
            unsafe { GetWindowDisplayAffinity(hwnd as usize as _, &mut value) },
            0
        );
        value
    }

    #[test]
    fn test_IOS_006_windows_affinity_of_a_test_window() {
        let class: Vec<u16> = "STATIC\0".encode_utf16().collect();
        // SAFETY: a top-level window of the predefined STATIC class, owned
        // and destroyed by this thread.
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                200,
                100,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        assert!(!hwnd.is_null());
        let id = hwnd as usize as u64;
        assert_eq!(affinity(id), WDA_NONE);
        assert!(set_excluded(id, true).unwrap());
        assert_eq!(affinity(id), WDA_EXCLUDEFROMCAPTURE);
        assert!(set_excluded(id, false).unwrap());
        assert_eq!(affinity(id), WDA_NONE);
        // SAFETY: created above on this thread.
        unsafe { DestroyWindow(hwnd) };
        assert!(!set_excluded(id, true).unwrap());
        assert!(matches!(
            set_excluded(0, true),
            Err(DesktopError::InvalidArgument(_))
        ));
    }
}
