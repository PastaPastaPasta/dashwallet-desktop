//! M2 desktop OS services implemented in Rust (`dw-desktop`) for Windows and
//! Linux (DESIGN-opus §1.13): single instance + URI hand-off, autostart,
//! tray, notifications, URI scheme registration, capture exclusion, QR
//! decoding from images, log export. macOS uses AppKit/SwiftUI for the same
//! protocols; there these calls return `desktop.unsupported` unless noted.
//! Owner: S1 (desktop services). Contract: docs/contracts/m2-engine.md §2.10.
//!
//! The surface is the same on every OS so the generated bindings do not
//! depend on the build host.

use std::sync::Arc;

use crate::Engine;
use crate::api::common::not_implemented;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum DesktopError {
    /// Code `desktop.unsupported`: this OS (or desktop session) does not
    /// offer the feature, e.g. no StatusNotifierItem host on GNOME without
    /// the AppIndicator extension. `feature` names it for the log.
    #[error("{feature} unsupported here")]
    Unsupported { feature: String },
    /// Code `desktop.os_error`: the OS call failed.
    #[error("os error: {detail}")]
    OsError { detail: String },
    /// Code `desktop.no_qr_code`: the image holds no readable QR code.
    #[error("no QR code found")]
    NoQrCode,
    /// Code `desktop.image_unreadable`: not a PNG/JPEG/BMP the decoder reads.
    #[error("image unreadable: {detail}")]
    ImageUnreadable { detail: String },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

impl crate::api::common::NotImplementedError for DesktopError {
    fn not_implemented(call: &'static str) -> Self {
        Self::NotImplemented {
            call: call.to_string(),
        }
    }
}

impl From<dw_engine::EngineError> for DesktopError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            E::Io(_) | E::Storage(_) => Self::OsError { detail },
            _ => Self::Internal { detail },
        }
    }
}

crate::api::common::export_error_code!(DesktopError);

impl DesktopError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::Unsupported { .. } => "desktop.unsupported",
            Self::OsError { .. } => "desktop.os_error",
            Self::NoQrCode => "desktop.no_qr_code",
            Self::ImageUnreadable { .. } => "desktop.image_unreadable",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

// ---- Single instance and URI hand-off (QT-001, QT-150) ----

/// Receives the arguments a second launch forwarded (URIs, file paths).
/// Called on a dw-desktop thread; must return quickly.
#[uniffi::export(with_foreign)]
pub trait InstanceObserver: Send + Sync {
    fn on_forwarded(&self, args: Vec<String>);
}

/// Held by the primary instance; releasing it frees the instance key.
#[derive(Debug, uniffi::Object)]
pub struct InstanceGuard {
    _private: (),
}

#[uniffi::export]
impl InstanceGuard {
    /// Stops listening and frees the key. Idempotent; dropping does the same.
    pub fn release(&self) {}
}

/// Becomes the primary instance for `key` (`DashWallet-<network>`; a local
/// socket on Windows, an abstract Unix socket on Linux) and delivers later
/// launches' forwarded arguments to `observer`. `None`: another instance
/// holds the key; the caller forwards with `forward_to_primary` and exits 0.
#[uniffi::export]
pub fn acquire_single_instance(
    key: String,
    observer: Arc<dyn InstanceObserver>,
) -> Result<Option<Arc<InstanceGuard>>, DesktopError> {
    let _ = (key, observer);
    not_implemented("acquire_single_instance")
}

/// Sends `args` to the primary instance for `key`. `false` when none
/// listens (the caller becomes primary instead).
#[uniffi::export]
pub fn forward_to_primary(key: String, args: Vec<String>) -> Result<bool, DesktopError> {
    let _ = (key, args);
    not_implemented("forward_to_primary")
}

/// Registers the URI schemes (`dash`, `pay`, `dashwallet`, …) for the
/// current user where the installer did not (Linux tarball:
/// `x-scheme-handler` in a user `.desktop` file; Windows: HKCU). Packaged
/// builds (Flatpak, MSI, macOS bundle) register at install time.
#[uniffi::export]
pub fn register_uri_schemes(
    app_id: String,
    exec_path: String,
    schemes: Vec<String>,
) -> Result<(), DesktopError> {
    let _ = (app_id, exec_path, schemes);
    not_implemented("register_uri_schemes")
}

// ---- Autostart (QT-009) ----

/// "Start on system login" entry: Windows Startup shortcut, Linux XDG
/// autostart `.desktop` (`dashwallet[-<network>].desktop`), launched with
/// `args` (dash-qt: `--min --chain=<network>`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AutostartEntry {
    pub app_id: String,
    pub display_name: String,
    pub exec_path: String,
    pub args: Vec<String>,
}

#[uniffi::export]
pub fn autostart_enabled(app_id: String) -> Result<bool, DesktopError> {
    let _ = app_id;
    not_implemented("autostart_enabled")
}

/// Writes (`enabled`) or deletes the entry. Idempotent.
#[uniffi::export]
pub fn set_autostart(entry: AutostartEntry, enabled: bool) -> Result<(), DesktopError> {
    let _ = (entry, enabled);
    not_implemented("set_autostart")
}

// ---- Tray (QT-028…030, IOS-117) ----

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TrayMenuItem {
    /// Reported back in `TrayObserver::on_menu_item`; empty for separators.
    pub id: String,
    /// Localized by the host.
    pub title: String,
    pub enabled: bool,
    pub separator: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TraySpec {
    pub app_id: String,
    /// "Dash Wallet client" + network text.
    pub tooltip: String,
    /// PNG, network-tinted by the host.
    pub icon_png: Vec<u8>,
    pub items: Vec<TrayMenuItem>,
}

/// Tray events, delivered on the tray thread; must return quickly.
#[uniffi::export(with_foreign)]
pub trait TrayObserver: Send + Sync {
    /// Left click: show or hide the main window (not on macOS).
    fn on_activate(&self);
    fn on_menu_item(&self, id: String);
}

/// The Windows notification-area icon (`Shell_NotifyIcon` thread) or Linux
/// StatusNotifierItem (`ksni`).
#[derive(Debug, uniffi::Object)]
pub struct TrayIcon {
    _private: (),
}

#[uniffi::export]
impl TrayIcon {
    /// Shows the icon. `desktop.unsupported` when the session has no tray
    /// host; the host then hides "Show tray icon" / "Minimize to tray".
    #[uniffi::constructor]
    pub fn new(spec: TraySpec, observer: Arc<dyn TrayObserver>) -> Result<Arc<Self>, DesktopError> {
        let _ = (spec, observer);
        not_implemented("TrayIcon.new")
    }

    pub fn set_tooltip(&self, tooltip: String) -> Result<(), DesktopError> {
        let _ = tooltip;
        not_implemented("TrayIcon.set_tooltip")
    }

    /// Replaces the menu (dash-qt disables it while a modal dialog is open).
    pub fn set_items(&self, items: Vec<TrayMenuItem>) -> Result<(), DesktopError> {
        let _ = items;
        not_implemented("TrayIcon.set_items")
    }

    /// "Show tray icon" off hides it without dropping the object.
    pub fn set_visible(&self, visible: bool) -> Result<(), DesktopError> {
        let _ = visible;
        not_implemented("TrayIcon.set_visible")
    }
}

// ---- Notifications (QT-031…033, IOS-105/116) ----

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DesktopNotification {
    /// Replaces an earlier notification with the same id.
    pub id: String,
    pub title: String,
    pub body: String,
    /// Returned in `NotificationObserver::on_activated` (e.g. a txid route).
    pub deep_link: Option<String>,
}

/// Clicks on notifications; delivered on a dw-desktop thread.
#[uniffi::export(with_foreign)]
pub trait NotificationObserver: Send + Sync {
    fn on_activated(&self, id: String, deep_link: Option<String>);
}

/// WinRT toasts on Windows, `org.freedesktop.Notifications` (or the Flatpak
/// portal) on Linux, timeout 10 s as dash-qt.
#[derive(Debug, uniffi::Object)]
pub struct DesktopNotifier {
    _private: (),
}

#[uniffi::export]
impl DesktopNotifier {
    #[uniffi::constructor]
    pub fn new(
        app_id: String,
        observer: Arc<dyn NotificationObserver>,
    ) -> Result<Arc<Self>, DesktopError> {
        let _ = (app_id, observer);
        not_implemented("DesktopNotifier.new")
    }

    pub fn notify(&self, notification: DesktopNotification) -> Result<(), DesktopError> {
        let _ = notification;
        not_implemented("DesktopNotifier.notify")
    }
}

// ---- Screen capture (IOS-006), QR images (IOS-043), quick unlock ----

/// Windows `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the window
/// `hwnd` while a phrase is visible. Returns whether exclusion is in effect;
/// `desktop.unsupported` on Linux (the host shows a warning banner).
#[uniffi::export]
pub fn set_window_capture_excluded(hwnd: u64, excluded: bool) -> Result<bool, DesktopError> {
    let _ = (hwnd, excluded);
    not_implemented("set_window_capture_excluded")
}

/// Decodes every QR code in an image file's bytes (PNG, JPEG, BMP) or a
/// clipboard image, in reading order. Works on every OS (pure Rust).
#[uniffi::export]
pub fn decode_qr_codes(image: Vec<u8>) -> Result<Vec<String>, DesktopError> {
    let _ = image;
    not_implemented("decode_qr_codes")
}

/// The OS biometric provider for vault slot B on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum QuickUnlockProvider {
    /// macOS: implemented in Swift (`PlatformServicesMac`), not here.
    TouchId,
    /// Windows Hello `KeyCredential` (M6).
    WindowsHello,
    /// Linux and hosts without biometrics: quick unlock is hidden.
    Unavailable,
}

/// Which provider the Rust side offers on this host. Windows Hello lands in
/// M6: until then this returns `Unavailable` on Windows too.
#[uniffi::export]
pub fn desktop_quick_unlock_provider() -> QuickUnlockProvider {
    QuickUnlockProvider::Unavailable
}

/// Windows Hello: signs the vault's fixed challenge and derives the slot B
/// wrap key (HKDF of the signature). M6.
#[uniffi::export]
pub fn windows_hello_wrap_key(challenge: Vec<u8>) -> Result<Vec<u8>, DesktopError> {
    let _ = challenge;
    not_implemented("windows_hello_wrap_key")
}

// ---- Logs (IOS-112) ----

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LogExport {
    pub path: String,
    pub file_count: u32,
    pub size_bytes: u64,
}

#[uniffi::export]
impl Engine {
    /// Zips the Rust log files of every network plus `extra_files` (the
    /// Swift log) into `dest_path` (IOS-112). Logs never hold secrets (the
    /// secret-in-log test guards this).
    pub async fn export_logs(
        &self,
        dest_path: String,
        extra_files: Vec<String>,
    ) -> Result<LogExport, DesktopError> {
        let _ = (dest_path, extra_files);
        not_implemented("Engine.export_logs")
    }
}
