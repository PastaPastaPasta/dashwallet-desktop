//! The plugins `run` adds before the app's own: one instance per data root
//! and the saved window geometry.

use std::path::{Path, PathBuf};

use tauri::Runtime;
use tauri::plugin::TauriPlugin;
use tauri_plugin_window_state::StateFlags;

use crate::launch::LaunchOptions;

/// A second launch hands its command line to the running app, which comes
/// forward and queues the links in it, and exits.
///
/// The instance is the data root's, as in dash-qt (its IPC server name
/// hashes the data directory): on Linux a `--datadir` launch takes a D-Bus
/// name of its own, so wallets in two data roots (and parallel test runs)
/// do not meet. On Windows the plugin keys by the bundle identifier only:
/// one instance per user session; it joins the forwarded arguments with
/// `|`, so a link holding one arrives cut short. Without a D-Bus session
/// bus the plugin cannot register and every launch runs on its own.
///
/// Not on macOS (`None`): LaunchServices already keeps one app instance
/// and hands links over as `RunEvent::Opened`, and the plugin's macOS
/// socket has a fixed name in the shared `/tmp`, which another local user
/// could take first and so swallow every launch and its arguments.
pub fn single_instance<R: Runtime>(launch: &LaunchOptions) -> Option<TauriPlugin<R>> {
    #[cfg(target_os = "macos")]
    {
        let _ = launch;
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let builder = tauri_plugin_single_instance::Builder::new().callback(|app, args, _cwd| {
            super::reveal(app);
            super::deliver_links(app, args, super::links::LinkSource::Forwarded);
        });
        #[cfg(target_os = "linux")]
        let builder = match &launch.data_dir {
            Some(dir) => builder.dbus_id(dbus_id(dir)),
            None => builder,
        };
        #[cfg(not(target_os = "linux"))]
        let _ = launch;
        Some(builder.build())
    }
}

fn absolute(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// The D-Bus name prefix of the instance for data root `dir`: the bundle
/// identifier and a 64-bit FNV-1a of the canonical path (the absolute one
/// before the root exists), so a symlinked spelling meets the same
/// instance. An element may not start with a digit, hence the `d`.
pub fn dbus_id(dir: &Path) -> String {
    let path = std::fs::canonicalize(dir).unwrap_or_else(|_| absolute(dir));
    let hash = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("org.dash.wallet.desktop.d{hash:016x}")
}

/// Saves the main window's size, position, maximised and full-screen state
/// at exit and restores them at launch: in `<datadir>/window-state.json`
/// with `--datadir`, else in the platform config directory. `--window-size`
/// and the self-test, which need a known geometry, neither read nor write
/// it.
pub fn window_state<R: Runtime>(launch: &LaunchOptions) -> Option<TauriPlugin<R>> {
    if launch.window_size.is_some() || launch.selftest.is_some() {
        return None;
    }
    let mut builder = tauri_plugin_window_state::Builder::new().with_state_flags(
        StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED | StateFlags::FULLSCREEN,
    );
    if let Some(dir) = &launch.data_dir {
        // An absolute file name replaces the config directory it is joined to.
        builder = builder.with_filename(
            absolute(dir)
                .join("window-state.json")
                .to_string_lossy()
                .into_owned(),
        );
    }
    Some(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_data_root_has_its_own_bus_name() {
        let a = dbus_id(Path::new("/home/u/wallet-a"));
        let b = dbus_id(Path::new("/home/u/wallet-b"));
        assert_ne!(a, b);
        assert_eq!(a, dbus_id(Path::new("/home/u/wallet-a")));
        // D-Bus: elements of [A-Za-z0-9_], none starting with a digit.
        let last = a.rsplit('.').next().unwrap();
        assert!(last.starts_with('d') && last.len() == 17, "{a}");
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_data_root_meets_the_same_instance() {
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, dir.path().join("link")).unwrap();
        assert_eq!(dbus_id(&root), dbus_id(&dir.path().join("link")));
        assert_eq!(dbus_id(&root), dbus_id(&dir.path().join("link/../root")));
    }

    #[test]
    fn a_fixed_geometry_skips_the_saved_one() {
        let fixed = LaunchOptions {
            window_size: Some((1000, 620)),
            ..LaunchOptions::default()
        };
        assert!(window_state::<tauri::test::MockRuntime>(&fixed).is_none());
        assert!(window_state::<tauri::test::MockRuntime>(&LaunchOptions::default()).is_some());
    }
}
