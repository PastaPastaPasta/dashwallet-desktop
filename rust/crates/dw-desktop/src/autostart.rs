//! "Start on system login" (QT-009). dash-qt starts the wallet minimized
//! with its network: the host passes `--min --chain=<network>` in `args`.
//!
//! - Linux: `$XDG_CONFIG_HOME/autostart/<app_id>.desktop` (XDG Autostart
//!   Specification). Inside Flatpak the entry must come from the Background
//!   portal, which is not implemented: `Unsupported` there.
//! - Windows: the per-user `HKCU\…\CurrentVersion\Run` value `<app_id>`,
//!   written with `reg.exe`. dash-qt uses a Startup-folder shortcut, which
//!   needs COM (`IShellLink`); the Run value starts the program the same
//!   way at logon. UNVERIFIED: not run on Windows yet.
//! - macOS: `Unsupported` (dash-qt hides the option; the app is a login
//!   item through `SMAppService` if ever offered).

use crate::DesktopError;

/// The entry to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutostartEntry {
    /// File / registry value name; 1–64 of `[A-Za-z0-9._-]`.
    pub app_id: String,
    pub display_name: String,
    /// Absolute path of the executable.
    pub exec_path: String,
    pub args: Vec<String>,
}

/// Whether the entry for `app_id` exists and is enabled.
pub fn enabled(app_id: &str) -> Result<bool, DesktopError> {
    crate::check_name("app id", app_id)?;
    #[cfg(target_os = "linux")]
    {
        linux::refuse_flatpak()?;
        linux::enabled_in(&linux::autostart_dir()?, app_id)
    }
    #[cfg(windows)]
    {
        windows::enabled(app_id)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(DesktopError::Unsupported("autostart".into()))
    }
}

/// Writes (`enabled`) or deletes the entry. Idempotent.
pub fn set(entry: &AutostartEntry, enabled: bool) -> Result<(), DesktopError> {
    crate::check_name("app id", &entry.app_id)?;
    if enabled && !std::path::Path::new(&entry.exec_path).is_absolute() {
        return Err(DesktopError::InvalidArgument(format!(
            "exec path {:?} is not absolute",
            entry.exec_path
        )));
    }
    #[cfg(target_os = "linux")]
    {
        linux::refuse_flatpak()?;
        linux::set_in(&linux::autostart_dir()?, entry, enabled)
    }
    #[cfg(windows)]
    {
        windows::set(entry, enabled)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = enabled;
        Err(DesktopError::Unsupported("autostart".into()))
    }
}

#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod linux {
    use std::path::{Path, PathBuf};

    use super::AutostartEntry;
    use crate::DesktopError;
    use crate::desktop_entry::{config_home, exec_line, value, write_atomic};

    pub(crate) fn autostart_dir() -> Result<PathBuf, DesktopError> {
        Ok(config_home()?.join("autostart"))
    }

    /// Inside Flatpak only the Background portal may add autostart entries.
    pub(crate) fn refuse_flatpak() -> Result<(), DesktopError> {
        if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
            return Err(DesktopError::Unsupported(
                "autostart inside Flatpak (needs the Background portal)".into(),
            ));
        }
        Ok(())
    }

    fn file(dir: &Path, app_id: &str) -> PathBuf {
        dir.join(format!("{app_id}.desktop"))
    }

    /// Enabled when the file exists and neither `Hidden=true` nor
    /// `X-GNOME-Autostart-enabled=false` turns it off (both are how desktop
    /// settings panels disable an entry without deleting it).
    pub(crate) fn enabled_in(dir: &Path, app_id: &str) -> Result<bool, DesktopError> {
        let text = match std::fs::read_to_string(file(dir, app_id)) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let off = text.lines().map(str::trim).any(|l| {
            l.eq_ignore_ascii_case("Hidden=true")
                || l.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        });
        Ok(!off)
    }

    pub(crate) fn contents(entry: &AutostartEntry) -> String {
        let exec = exec_line(
            std::iter::once(entry.exec_path.as_str()).chain(entry.args.iter().map(String::as_str)),
        );
        format!(
            "[Desktop Entry]\nType=Application\nVersion=1.5\nName={}\nExec={}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
            value(&entry.display_name),
            exec
        )
    }

    pub(crate) fn set_in(
        dir: &Path,
        entry: &AutostartEntry,
        enabled: bool,
    ) -> Result<(), DesktopError> {
        let path = file(dir, &entry.app_id);
        if enabled {
            write_atomic(&path, &contents(entry))
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            }
        }
    }
}

#[cfg(windows)]
pub(crate) use windows::command_line as windows_command_line;

#[cfg(windows)]
mod windows {
    use std::process::Command;

    use super::AutostartEntry;
    use crate::DesktopError;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

    /// A Windows command line: each part double-quoted, inner quotes and
    /// the backslashes before them escaped (CommandLineToArgvW rules).
    pub(crate) fn command_line(parts: &[&str]) -> String {
        parts
            .iter()
            .map(|p| {
                let mut out = String::from("\"");
                let mut backslashes = 0;
                for c in p.chars() {
                    match c {
                        '\\' => backslashes += 1,
                        '"' => {
                            out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                            backslashes = 0;
                        }
                        _ => {
                            out.extend(std::iter::repeat_n('\\', backslashes));
                            backslashes = 0;
                        }
                    }
                    if c != '\\' {
                        out.push(c);
                    }
                }
                out.extend(std::iter::repeat_n('\\', backslashes * 2));
                out.push('"');
                out
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub(crate) fn enabled(app_id: &str) -> Result<bool, DesktopError> {
        let status = Command::new("reg")
            .args(["query", RUN_KEY, "/v", app_id])
            .output()?
            .status;
        Ok(status.success())
    }

    pub(crate) fn set(entry: &AutostartEntry, enabled: bool) -> Result<(), DesktopError> {
        let output = if enabled {
            let mut parts = vec![entry.exec_path.as_str()];
            parts.extend(entry.args.iter().map(String::as_str));
            Command::new("reg")
                .args(["add", RUN_KEY, "/v", &entry.app_id, "/t", "REG_SZ", "/d"])
                .arg(command_line(&parts))
                .arg("/f")
                .output()?
        } else {
            if !self::enabled(&entry.app_id)? {
                return Ok(());
            }
            Command::new("reg")
                .args(["delete", RUN_KEY, "/v", &entry.app_id, "/f"])
                .output()?
        };
        if output.status.success() {
            Ok(())
        } else {
            Err(DesktopError::OsError(format!(
                "reg.exe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> AutostartEntry {
        AutostartEntry {
            app_id: "org.dash.DashWallet-testnet".into(),
            display_name: "Dash Wallet (testnet)".into(),
            exec_path: "/home/u/Dash Wallet/dash-wallet".into(),
            args: vec!["--min".into(), "--chain=test".into()],
        }
    }

    #[test]
    fn test_QT_009_xdg_autostart_entry_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let e = entry();
        assert!(!linux::enabled_in(dir.path(), &e.app_id).unwrap());
        linux::set_in(dir.path(), &e, true).unwrap();
        assert!(linux::enabled_in(dir.path(), &e.app_id).unwrap());
        let text = std::fs::read_to_string(dir.path().join("org.dash.DashWallet-testnet.desktop"))
            .unwrap();
        assert_eq!(
            text,
            "[Desktop Entry]\nType=Application\nVersion=1.5\nName=Dash Wallet (testnet)\nExec=\"/home/u/Dash Wallet/dash-wallet\" --min --chain=test\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        );
        // Idempotent both ways.
        linux::set_in(dir.path(), &e, true).unwrap();
        linux::set_in(dir.path(), &e, false).unwrap();
        linux::set_in(dir.path(), &e, false).unwrap();
        assert!(!linux::enabled_in(dir.path(), &e.app_id).unwrap());

        // A settings panel that switched it off.
        std::fs::write(
            dir.path().join("org.dash.DashWallet-testnet.desktop"),
            "[Desktop Entry]\nHidden=true\n",
        )
        .unwrap();
        assert!(!linux::enabled_in(dir.path(), &e.app_id).unwrap());
    }

    #[test]
    fn test_QT_009_arguments_are_checked() {
        let mut e = entry();
        e.exec_path = "dash-wallet".into();
        assert!(matches!(
            set(&e, true),
            Err(DesktopError::InvalidArgument(_))
        ));
        e.app_id = "../evil".into();
        assert!(matches!(
            set(&e, false),
            Err(DesktopError::InvalidArgument(_))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_QT_009_macos_is_unsupported() {
        assert!(matches!(
            enabled("org.dash.DashWallet"),
            Err(DesktopError::Unsupported(_))
        ));
        assert!(matches!(
            set(&entry(), true),
            Err(DesktopError::Unsupported(_))
        ));
    }
}
