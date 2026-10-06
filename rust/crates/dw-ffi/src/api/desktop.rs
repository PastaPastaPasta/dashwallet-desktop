//! M2 desktop OS services implemented in Rust (`dw-desktop`) for Windows and
//! Linux (DESIGN-opus §1.13): single instance + URI hand-off, autostart,
//! tray, notifications, URI scheme registration, capture exclusion, QR
//! decoding from images, log export. macOS uses AppKit/SwiftUI for the same
//! protocols; there these calls return `desktop.unsupported` except
//! `decode_qr_codes` and `Engine.export_logs`, which work everywhere.
//! Owner: S1 (desktop services). Contract: docs/contracts/m2-engine.md §2.10.
//!
//! The surface is the same on every OS so the generated bindings do not
//! depend on the build host. Windows code paths in dw-desktop are written
//! but not built or run yet (no Windows runner).

use std::path::PathBuf;
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

impl From<dw_desktop::DesktopError> for DesktopError {
    fn from(e: dw_desktop::DesktopError) -> Self {
        use dw_desktop::DesktopError as D;
        match e {
            D::Unsupported(feature) => Self::Unsupported { feature },
            D::OsError(detail) => Self::OsError { detail },
            D::NoQrCode => Self::NoQrCode,
            D::ImageUnreadable(detail) => Self::ImageUnreadable { detail },
            D::InvalidArgument(detail) => Self::InvalidArgument { detail },
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

/// `desktop.unsupported` on macOS, where the Swift side serves `feature`.
fn refuse_on_macos(feature: &str) -> Result<(), DesktopError> {
    if cfg!(target_os = "macos") {
        return Err(DesktopError::Unsupported {
            feature: format!("{feature} (served by PlatformServicesMac on macOS)"),
        });
    }
    Ok(())
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
    inner: dw_desktop::instance::InstanceGuard,
}

#[uniffi::export]
impl InstanceGuard {
    /// Stops listening and frees the key. Idempotent; dropping does the same.
    pub fn release(&self) {
        self.inner.release();
    }
}

/// Becomes the primary instance for `key` (`DashWallet-<network>`; Linux: a
/// lock file and socket in `$XDG_RUNTIME_DIR`; Windows: a named mutex and
/// pipe) and delivers later launches' forwarded arguments to `observer`.
/// `None`: another instance holds the key; the caller forwards with
/// `forward_to_primary` and exits 0. macOS: `desktop.unsupported`
/// (LaunchServices keeps one instance).
#[uniffi::export]
pub fn acquire_single_instance(
    key: String,
    observer: Arc<dyn InstanceObserver>,
) -> Result<Option<Arc<InstanceGuard>>, DesktopError> {
    refuse_on_macos("single instance")?;
    let handler: dw_desktop::instance::ForwardHandler =
        Arc::new(move |args| observer.on_forwarded(args));
    Ok(
        dw_desktop::instance::acquire(&key, handler)?
            .map(|inner| Arc::new(InstanceGuard { inner })),
    )
}

/// Sends `args` to the primary instance for `key`. `false` when none
/// listens (the caller becomes primary instead). Waits up to 3 s for a
/// primary that is still starting.
#[uniffi::export]
pub fn forward_to_primary(key: String, args: Vec<String>) -> Result<bool, DesktopError> {
    refuse_on_macos("single instance")?;
    Ok(dw_desktop::instance::forward(&key, &args)?)
}

/// Registers the URI schemes (`dash`, `pay`, `dashwallet`, …) for the
/// current user where the installer did not (Linux tarball:
/// `x-scheme-handler` in a user `.desktop` file plus `mimeapps.list`
/// defaults; Windows: HKCU). Packaged builds (Flatpak, MSI, macOS bundle)
/// register at install time; macOS returns `desktop.unsupported`.
#[uniffi::export]
pub fn register_uri_schemes(
    app_id: String,
    exec_path: String,
    schemes: Vec<String>,
) -> Result<(), DesktopError> {
    Ok(dw_desktop::uri_schemes::register(
        &app_id, &exec_path, &schemes,
    )?)
}

// ---- Autostart (QT-009) ----

/// "Start on system login" entry: Windows `Run` value, Linux XDG
/// autostart `.desktop` (`<app_id>.desktop`), launched with `args`
/// (dash-qt: `--min --chain=<network>`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AutostartEntry {
    pub app_id: String,
    pub display_name: String,
    pub exec_path: String,
    pub args: Vec<String>,
}

/// Whether the entry exists and is enabled. `desktop.unsupported` on
/// macOS and inside Flatpak (Background portal not implemented).
#[uniffi::export]
pub fn autostart_enabled(app_id: String) -> Result<bool, DesktopError> {
    Ok(dw_desktop::autostart::enabled(&app_id)?)
}

/// Writes (`enabled`) or deletes the entry. Idempotent.
#[uniffi::export]
pub fn set_autostart(entry: AutostartEntry, enabled: bool) -> Result<(), DesktopError> {
    let entry = dw_desktop::autostart::AutostartEntry {
        app_id: entry.app_id,
        display_name: entry.display_name,
        exec_path: entry.exec_path,
        args: entry.args,
    };
    Ok(dw_desktop::autostart::set(&entry, enabled)?)
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
/// StatusNotifierItem. Neither backend is built yet (the SNI needs a D-Bus
/// stack, the Windows icon a Win32 message loop), so `new` reports
/// `desktop.unsupported` on every OS and the host keeps the window: "Show
/// tray icon" and "Minimize to tray" are hidden, as on a session without a
/// tray host.
#[derive(Debug, uniffi::Object)]
pub struct TrayIcon {
    _private: (),
}

const TRAY_UNBUILT: &str = "tray icon (no StatusNotifierItem / Shell_NotifyIcon backend yet)";

#[uniffi::export]
impl TrayIcon {
    /// Shows the icon. `desktop.unsupported` when the session has no tray
    /// host, and in this build on every OS (see the type doc).
    #[uniffi::constructor]
    pub fn new(spec: TraySpec, observer: Arc<dyn TrayObserver>) -> Result<Arc<Self>, DesktopError> {
        let _ = (spec, observer);
        Err(DesktopError::Unsupported {
            feature: TRAY_UNBUILT.into(),
        })
    }

    pub fn set_tooltip(&self, tooltip: String) -> Result<(), DesktopError> {
        let _ = tooltip;
        Err(DesktopError::Unsupported {
            feature: TRAY_UNBUILT.into(),
        })
    }

    /// Replaces the menu (dash-qt disables it while a modal dialog is open).
    pub fn set_items(&self, items: Vec<TrayMenuItem>) -> Result<(), DesktopError> {
        let _ = items;
        Err(DesktopError::Unsupported {
            feature: TRAY_UNBUILT.into(),
        })
    }

    /// "Show tray icon" off hides it without dropping the object.
    pub fn set_visible(&self, visible: bool) -> Result<(), DesktopError> {
        let _ = visible;
        Err(DesktopError::Unsupported {
            feature: TRAY_UNBUILT.into(),
        })
    }
}

// ---- Notifications (QT-031…033, IOS-105/116) ----

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DesktopNotification {
    /// Identifies the notification in `NotificationObserver::on_activated`.
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

/// Linux: `notify-send` (libnotify, D-Bus `org.freedesktop.Notifications`),
/// timeout 10 s as dash-qt, clicks reported where `notify-send` supports
/// `--action`/`--wait`; `desktop.unsupported` without `notify-send`.
/// Windows: a toast through PowerShell, clicks not reported (unverified).
/// macOS: `desktop.unsupported` (UserNotifications in Swift).
#[derive(Debug, uniffi::Object)]
pub struct DesktopNotifier {
    inner: dw_desktop::notify::Notifier,
}

#[uniffi::export]
impl DesktopNotifier {
    #[uniffi::constructor]
    pub fn new(
        app_id: String,
        observer: Arc<dyn NotificationObserver>,
    ) -> Result<Arc<Self>, DesktopError> {
        let handler: dw_desktop::notify::ActivationHandler =
            Arc::new(move |id, deep_link| observer.on_activated(id, deep_link));
        Ok(Arc::new(Self {
            inner: dw_desktop::notify::Notifier::new(&app_id, handler)?,
        }))
    }

    pub fn notify(&self, notification: DesktopNotification) -> Result<(), DesktopError> {
        Ok(self.inner.notify(&dw_desktop::notify::Notification {
            id: notification.id,
            title: notification.title,
            body: notification.body,
            deep_link: notification.deep_link,
        })?)
    }
}

// ---- Screen capture (IOS-006), QR images (IOS-043), quick unlock ----

/// Windows `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` on the window
/// `hwnd` while a phrase is visible. Returns whether exclusion is in effect;
/// `desktop.unsupported` on Linux (the host shows a warning banner) and
/// macOS (`NSWindow.sharingType` in Swift).
#[uniffi::export]
pub fn set_window_capture_excluded(hwnd: u64, excluded: bool) -> Result<bool, DesktopError> {
    Ok(dw_desktop::capture::set_excluded(hwnd, excluded)?)
}

/// Decodes every QR code in an image file's bytes (PNG, JPEG, BMP) or a
/// clipboard image, in reading order. Works on every OS (pure Rust). At
/// most 32 MiB and 40 megapixels (`desktop.image_unreadable` above).
#[uniffi::export]
pub fn decode_qr_codes(image: Vec<u8>) -> Result<Vec<String>, DesktopError> {
    Ok(dw_desktop::qr::decode(&image)?)
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
    /// Log files in the zip, not counting its `manifest.txt`.
    pub file_count: u32,
    pub size_bytes: u64,
}

#[uniffi::export]
impl Engine {
    /// Zips the log files of the data root (`logs/` and `<network>/logs/`)
    /// plus `extra_files` (the Swift log) and a `manifest.txt` into
    /// `dest_path`, which must be absolute and must not exist (IOS-112).
    /// Missing extra files are listed in the manifest as skipped. Logs never
    /// hold secrets (the secret-in-log test guards this). The engine writes
    /// no log files yet, so today the zip holds the host's files and the
    /// manifest.
    pub async fn export_logs(
        &self,
        dest_path: String,
        extra_files: Vec<String>,
    ) -> Result<LogExport, DesktopError> {
        let extras: Vec<PathBuf> = extra_files.into_iter().map(PathBuf::from).collect();
        let export = dw_desktop::logs::export(
            &self.inner.config().data_root,
            &PathBuf::from(dest_path),
            &extras,
            &crate::core_version(),
        )?;
        Ok(LogExport {
            path: export.path.to_string_lossy().into_owned(),
            file_count: export.file_count,
            size_bytes: export.size_bytes,
        })
    }
}
