//! Per-user URI scheme registration for unpackaged builds (QT-150,
//! IOS-048). Packages register at install time (macOS `CFBundleURLTypes`,
//! Flatpak and the MSI); this covers the Linux tarball and a Windows build
//! run from a folder.
//!
//! - Linux: `$XDG_DATA_HOME/applications/<app_id>.desktop` with
//!   `MimeType=x-scheme-handler/<scheme>;…` and `Exec=<exec> %u`, the
//!   defaults in `$XDG_CONFIG_HOME/mimeapps.list`, then
//!   `update-desktop-database` when it is installed.
//! - Windows: `HKCU\Software\Classes\<scheme>` with `URL Protocol` and
//!   `shell\open\command = "<exec>" "%1"`, written with `reg.exe`.
//!   UNVERIFIED: not run on Windows yet.
//! - macOS: `Unsupported` (the bundle's Info.plist registers them).

use crate::DesktopError;

/// The name the Linux handler entry shows in "Open with" lists.
pub const HANDLER_NAME: &str = "Dash Wallet";

fn check_scheme(scheme: &str) -> Result<(), DesktopError> {
    let mut chars = scheme.chars();
    let ok = scheme.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '+' | '.' | '-'));
    if ok {
        Ok(())
    } else {
        Err(DesktopError::InvalidArgument(format!(
            "URI scheme {scheme:?} is not a lower-case RFC 3986 scheme"
        )))
    }
}

/// Registers this executable as the handler of `schemes` for the user.
pub fn register(app_id: &str, exec_path: &str, schemes: &[String]) -> Result<(), DesktopError> {
    crate::check_name("app id", app_id)?;
    if schemes.is_empty() {
        return Err(DesktopError::InvalidArgument("no URI schemes".into()));
    }
    for scheme in schemes {
        check_scheme(scheme)?;
    }
    if !std::path::Path::new(exec_path).is_absolute() {
        return Err(DesktopError::InvalidArgument(format!(
            "exec path {exec_path:?} is not absolute"
        )));
    }
    #[cfg(target_os = "linux")]
    {
        use crate::desktop_entry::{config_home, data_home};
        let apps = data_home()?.join("applications");
        linux::register_in(&apps, &config_home()?, app_id, exec_path, schemes)?;
        linux::update_database(&apps);
        Ok(())
    }
    #[cfg(windows)]
    {
        let _ = app_id;
        windows::register(exec_path, schemes)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Err(DesktopError::Unsupported(
            "URI scheme registration (the app bundle's Info.plist registers them)".into(),
        ))
    }
}

#[cfg(any(target_os = "linux", test))]
pub(crate) mod linux {
    use std::path::Path;

    use super::HANDLER_NAME;
    use crate::DesktopError;
    use crate::desktop_entry::{exec_line, set_mime_defaults, write_atomic};

    pub(crate) fn register_in(
        applications: &Path,
        config: &Path,
        app_id: &str,
        exec_path: &str,
        schemes: &[String],
    ) -> Result<(), DesktopError> {
        let desktop_file = format!("{app_id}.desktop");
        let mime_types: Vec<String> = schemes
            .iter()
            .map(|s| format!("x-scheme-handler/{s}"))
            .collect();
        let entry = format!(
            "[Desktop Entry]\nType=Application\nVersion=1.5\nName={HANDLER_NAME}\nExec={} %u\nTerminal=false\nNoDisplay=true\nMimeType={};\n",
            exec_line([exec_path]),
            mime_types.join(";")
        );
        write_atomic(&applications.join(&desktop_file), &entry)?;

        let list = config.join("mimeapps.list");
        let existing = match std::fs::read_to_string(&list) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        write_atomic(
            &list,
            &set_mime_defaults(&existing, &mime_types, &desktop_file),
        )
    }

    /// Refreshes the MIME cache of `applications` when the tool exists.
    /// Desktops that read `mimeapps.list` directly do not need it, so a
    /// missing tool or a failure is logged, not returned.
    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn update_database(applications: &Path) {
        let Some(tool) = crate::find_program("update-desktop-database") else {
            tracing::info!("update-desktop-database not installed; mimeapps.list is set");
            return;
        };
        match std::process::Command::new(tool).arg(applications).output() {
            Ok(out) if out.status.success() => {}
            Ok(out) => tracing::warn!(
                stderr = %String::from_utf8_lossy(&out.stderr),
                "update-desktop-database failed"
            ),
            Err(e) => tracing::warn!(error = %e, "update-desktop-database did not run"),
        }
    }
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    use crate::DesktopError;
    use crate::autostart::windows_command_line;

    fn reg_add(key: &str, args: &[&str]) -> Result<(), DesktopError> {
        let output = Command::new("reg")
            .arg("add")
            .arg(key)
            .args(args)
            .arg("/f")
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(DesktopError::OsError(format!(
                "reg.exe add {key} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    pub(crate) fn register(exec_path: &str, schemes: &[String]) -> Result<(), DesktopError> {
        let command = format!("{} \"%1\"", windows_command_line(&[exec_path]));
        for scheme in schemes {
            let key = format!(r"HKCU\Software\Classes\{scheme}");
            reg_add(&key, &["/ve", "/d", &format!("URL:{scheme} Protocol")])?;
            reg_add(&key, &["/v", "URL Protocol", "/d", ""])?;
            reg_add(
                &format!(r"{key}\shell\open\command"),
                &["/ve", "/d", &command],
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_150_linux_handler_entry_and_defaults() {
        let data = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let apps = data.path().join("applications");
        let schemes = vec!["dash".to_string(), "pay".into()];
        linux::register_in(
            &apps,
            config.path(),
            "org.dash.DashWallet",
            "/opt/Dash Wallet/dash-wallet",
            &schemes,
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(apps.join("org.dash.DashWallet.desktop")).unwrap(),
            "[Desktop Entry]\nType=Application\nVersion=1.5\nName=Dash Wallet\nExec=\"/opt/Dash Wallet/dash-wallet\" %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/dash;x-scheme-handler/pay;\n"
        );
        assert_eq!(
            std::fs::read_to_string(config.path().join("mimeapps.list")).unwrap(),
            "[Default Applications]\nx-scheme-handler/dash=org.dash.DashWallet.desktop;\nx-scheme-handler/pay=org.dash.DashWallet.desktop;\n"
        );
    }

    #[test]
    fn test_QT_150_schemes_and_paths_are_checked() {
        for bad in ["", "Dash", "1dash", "da sh", "dash:", &"d".repeat(33)] {
            assert!(check_scheme(bad).is_err(), "{bad:?}");
        }
        for good in ["dash", "dash-key", "dash-st", "web+dash"] {
            check_scheme(good).unwrap();
        }
        assert!(matches!(
            register(
                "org.dash.DashWallet",
                "relative/dash-wallet",
                &["dash".into()]
            ),
            Err(DesktopError::InvalidArgument(_))
        ));
        assert!(matches!(
            register("org.dash.DashWallet", "/x", &[]),
            Err(DesktopError::InvalidArgument(_))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_QT_150_macos_is_unsupported() {
        assert!(matches!(
            register("org.dash.DashWallet", "/x", &["dash".into()]),
            Err(DesktopError::Unsupported(_))
        ));
    }
}
