//! Transaction notifications (QT-031, IOS-116).
//!
//! - Linux: the `notify-send` program (libnotify), which talks to the
//!   session's `org.freedesktop.Notifications` service. When it supports
//!   `--action` and `--wait` (libnotify ≥ 0.7.10), a click on a
//!   notification with a deep link calls the activation handler. Without
//!   the program: `Unsupported`, and the host says notifications are not
//!   available. Timeout 10 s, as dash-qt.
//! - Windows: a toast shown through PowerShell's registered app identity
//!   (no AUMID of our own until the MSI installs a Start-menu shortcut).
//!   Clicks are not reported back. UNVERIFIED: not run on Windows yet.
//! - macOS: `Unsupported` (UserNotifications in Swift).

use std::sync::Arc;

use crate::DesktopError;

/// One notification. `id` comes back with the activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    /// Returned on click (e.g. a transaction route).
    pub deep_link: Option<String>,
}

/// Called with `(id, deep_link)` when the user clicks a notification, on a
/// notifier thread. Must return quickly.
pub type ActivationHandler = Arc<dyn Fn(String, Option<String>) + Send + Sync>;

/// Shows notifications for one app.
pub struct Notifier {
    #[cfg_attr(not(any(unix, windows)), allow(dead_code))]
    app_id: String,
    backend: Backend,
}

impl std::fmt::Debug for Notifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Notifier")
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

enum Backend {
    #[cfg(unix)]
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    NotifySend(notify_send::NotifySend),
    #[cfg(windows)]
    Toast,
    #[cfg(not(any(unix, windows)))]
    #[allow(dead_code)]
    None,
}

impl Notifier {
    /// `Unsupported` when this session has no way to show notifications.
    pub fn new(app_id: &str, handler: ActivationHandler) -> Result<Self, DesktopError> {
        crate::check_name("app id", app_id)?;
        #[cfg(target_os = "linux")]
        {
            let program = crate::find_program("notify-send").ok_or_else(|| {
                DesktopError::Unsupported("notifications (notify-send is not installed)".into())
            })?;
            Ok(Self {
                app_id: app_id.to_owned(),
                backend: Backend::NotifySend(notify_send::NotifySend::new(program, handler)),
            })
        }
        #[cfg(windows)]
        {
            let _ = handler;
            Ok(Self {
                app_id: app_id.to_owned(),
                backend: Backend::Toast,
            })
        }
        #[cfg(not(any(target_os = "linux", windows)))]
        {
            let _ = handler;
            Err(DesktopError::Unsupported(
                "notifications (UserNotifications in the app)".into(),
            ))
        }
    }

    /// A notifier over a given `notify-send` program (tests).
    #[cfg(all(test, unix))]
    fn with_notify_send(
        app_id: &str,
        program: std::path::PathBuf,
        handler: ActivationHandler,
    ) -> Self {
        Self {
            app_id: app_id.to_owned(),
            backend: Backend::NotifySend(notify_send::NotifySend::new(program, handler)),
        }
    }

    pub fn notify(&self, notification: &Notification) -> Result<(), DesktopError> {
        match &self.backend {
            #[cfg(unix)]
            Backend::NotifySend(n) => n.notify(&self.app_id, notification),
            #[cfg(windows)]
            Backend::Toast => toast::show(notification),
            #[cfg(not(any(unix, windows)))]
            Backend::None => {
                let _ = notification;
                Err(DesktopError::Unsupported("notifications".into()))
            }
        }
    }
}

#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod notify_send {
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};

    use super::{ActivationHandler, Notification};
    use crate::DesktopError;

    /// Notifications waiting for a click at once; more are shown without
    /// click handling so a burst does not pile up processes.
    const MAX_WAITING: usize = 8;
    const TIMEOUT_MS: &str = "10000";

    pub(super) struct NotifySend {
        program: PathBuf,
        handler: ActivationHandler,
        /// Whether this `notify-send` has `--action` and `--wait`.
        actions: OnceLock<bool>,
        waiting: Arc<AtomicUsize>,
    }

    /// Notification bodies may be read as markup by the server.
    fn escape_markup(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }

    impl NotifySend {
        pub(super) fn new(program: PathBuf, handler: ActivationHandler) -> Self {
            Self {
                program,
                handler,
                actions: OnceLock::new(),
                waiting: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn supports_actions(&self) -> bool {
            *self.actions.get_or_init(|| {
                Command::new(&self.program)
                    .arg("--help")
                    .stdin(Stdio::null())
                    .output()
                    .map(|o| {
                        let help = String::from_utf8_lossy(&o.stdout);
                        help.contains("--action") && help.contains("--wait")
                    })
                    .unwrap_or(false)
            })
        }

        pub(super) fn notify(&self, app_id: &str, n: &Notification) -> Result<(), DesktopError> {
            let mut command = Command::new(&self.program);
            command
                .arg(format!("--app-name={app_id}"))
                .arg(format!("--expire-time={TIMEOUT_MS}"))
                .stdin(Stdio::null());
            let wait = n.deep_link.is_some()
                && self.supports_actions()
                && self.waiting.load(Ordering::SeqCst) < MAX_WAITING;
            if wait {
                command.arg("--action=default=Open").arg("--wait");
            }
            command.arg("--").arg(&n.title).arg(escape_markup(&n.body));

            if !wait {
                let out = command.output()?;
                return if out.status.success() {
                    Ok(())
                } else {
                    Err(DesktopError::OsError(format!(
                        "notify-send failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )))
                };
            }
            // `--wait` blocks until the notification closes and prints the
            // action the user chose; read it on a thread.
            let child = command
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()?;
            self.waiting.fetch_add(1, Ordering::SeqCst);
            let waiting = Arc::clone(&self.waiting);
            let handler = Arc::clone(&self.handler);
            let id = n.id.clone();
            let deep_link = n.deep_link.clone();
            let spawned = std::thread::Builder::new()
                .name("dw-desktop-notify".into())
                .spawn(move || {
                    let clicked = child.wait_with_output().is_ok_and(|o| {
                        o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "default"
                    });
                    waiting.fetch_sub(1, Ordering::SeqCst);
                    if clicked {
                        handler(id, deep_link);
                    }
                });
            if let Err(e) = spawned {
                self.waiting.fetch_sub(1, Ordering::SeqCst);
                return Err(e.into());
            }
            Ok(())
        }
    }
}

#[cfg(windows)]
mod toast {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use super::Notification;
    use crate::DesktopError;

    /// PowerShell's app identity, which Windows lets show toasts.
    const POWERSHELL_AUMID: &str =
        r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }

    /// Base64 of the UTF-16LE script, as `-EncodedCommand` expects, so no
    /// text reaches PowerShell's own parser unquoted.
    fn encode_command(script: &str) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub(super) fn show(n: &Notification) -> Result<(), DesktopError> {
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            xml_escape(&n.title),
            xml_escape(&n.body)
        );
        // Single-quoted PowerShell string: only `'` needs doubling.
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null;\
             [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null;\
             $x = New-Object Windows.Data.Xml.Dom.XmlDocument; $x.LoadXml('{}');\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{}').Show((New-Object Windows.UI.Notifications.ToastNotification $x))",
            xml.replace('\'', "''"),
            POWERSHELL_AUMID
        );
        let out = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-EncodedCommand"])
            .arg(encode_command(&script))
            .creation_flags(CREATE_NO_WINDOW)
            .output()?;
        if out.status.success() {
            Ok(())
        } else {
            Err(DesktopError::OsError(format!(
                "toast failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )))
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A stand-in `notify-send` that records its arguments in `log` and
    /// prints `answer` (what the real one prints after `--wait`).
    fn fake_notify_send(
        dir: &std::path::Path,
        answer: &str,
        with_actions: bool,
    ) -> std::path::PathBuf {
        let program = dir.join("notify-send");
        let log = dir.join("log");
        let help = if with_actions {
            "  -A, --action=[NAME=]Text...\n  -w, --wait"
        } else {
            "  -u, --urgency=LEVEL"
        };
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --help ]; then echo '{help}'; exit 0; fi\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\necho '---' >> '{}'\necho '{answer}'\n",
                log.display(),
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        program
    }

    fn handler() -> (ActivationHandler, mpsc::Receiver<(String, Option<String>)>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        (
            Arc::new(move |id, link| {
                let _ = tx.lock().unwrap().send((id, link));
            }),
            rx,
        )
    }

    #[test]
    fn test_QT_031_notify_send_shows_and_reports_clicks() {
        let dir = tempfile::tempdir().unwrap();
        let program = fake_notify_send(dir.path(), "default", true);
        let (h, rx) = handler();
        let notifier = Notifier::with_notify_send("org.dash.DashWallet", program, h);
        notifier
            .notify(&Notification {
                id: "tx-1".into(),
                title: "Incoming transaction".into(),
                body: "Amount: 1.00 DASH\n<b>Label</b> & co".into(),
                deep_link: Some("dashwallet://tx/abcd".into()),
            })
            .unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            ("tx-1".to_string(), Some("dashwallet://tx/abcd".to_string()))
        );
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        assert_eq!(
            log,
            "--app-name=org.dash.DashWallet\n--expire-time=10000\n--action=default=Open\n--wait\n--\nIncoming transaction\nAmount: 1.00 DASH\n&lt;b&gt;Label&lt;/b&gt; &amp; co\n---\n"
        );
    }

    #[test]
    fn test_QT_031_without_actions_or_link_no_click_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let program = fake_notify_send(dir.path(), "default", false);
        let (h, rx) = handler();
        let notifier = Notifier::with_notify_send("org.dash.DashWallet", program, h);
        let n = Notification {
            id: "tx-2".into(),
            title: "-starts with a dash".into(),
            body: "b".into(),
            deep_link: Some("x".into()),
        };
        notifier.notify(&n).unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        let log = std::fs::read_to_string(dir.path().join("log")).unwrap();
        assert!(!log.contains("--wait"));
        assert!(log.contains("--\n-starts with a dash\n"));
    }

    #[test]
    fn test_QT_031_a_failing_notify_send_is_an_os_error() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("notify-send");
        std::fs::write(&program, "#!/bin/sh\necho 'no daemon' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (h, _) = handler();
        let notifier = Notifier::with_notify_send("org.dash.DashWallet", program, h);
        let err = notifier
            .notify(&Notification {
                id: "x".into(),
                title: "t".into(),
                body: "b".into(),
                deep_link: None,
            })
            .unwrap_err();
        assert_eq!(
            err,
            DesktopError::OsError("notify-send failed: no daemon".into())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_QT_031_macos_is_unsupported() {
        let (h, _) = handler();
        assert!(matches!(
            Notifier::new("org.dash.DashWallet", h),
            Err(DesktopError::Unsupported(_))
        ));
    }
}
