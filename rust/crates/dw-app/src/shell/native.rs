//! The plugins `run` adds before the app's own: one instance per data root
//! and the saved window geometry.

use std::path::Path;

use tauri::Runtime;
use tauri::plugin::TauriPlugin;
use tauri_plugin_window_state::StateFlags;

use crate::launch::LaunchOptions;

/// A second launch hands its command line to the running app, which comes
/// forward and queues the links in it, and exits.
///
/// The instance is the data root's, as in dash-qt (its IPC server name
/// hashes the data directory): on Linux each canonical root (`root`, from
/// [`crate::instance::data_root`]) takes a D-Bus name of its own, the
/// default root too, so wallets in two data roots (and parallel test runs)
/// do not meet. On Windows the plugin keys by the bundle identifier only:
/// one instance per user session. It joins the forwarded arguments with
/// `|` and splits them there again, so [`rejoin_split_args`] puts a link
/// back together (D22).
///
/// Linux without a D-Bus session bus: no plugin (`None`, logged), since the
/// plugin panics on a malformed bus address and does nothing without a bus.
/// A second launch then stops at the data root's instance lock
/// ([`crate::instance::InstanceLock`]) without opening the wallet.
///
/// Not on macOS (`None`): LaunchServices already keeps one app instance
/// and hands links over as `RunEvent::Opened`, and the plugin's macOS
/// socket has a fixed name in the shared `/tmp`, which another local user
/// could take first and so swallow every launch and its arguments. A direct
/// launch of the binary, which LaunchServices does not see, stops at the
/// instance lock.
pub fn single_instance<R: Runtime>(root: &Path) -> Option<TauriPlugin<R>> {
    #[cfg(target_os = "macos")]
    {
        let _ = root;
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        if let Err(e) = zbus::blocking::Connection::session() {
            eprintln!(
                "dw-app: no D-Bus session bus ({e}): a second launch on this data root \
                 stops at its instance lock instead of handing its links over"
            );
            return None;
        }
        let builder = tauri_plugin_single_instance::Builder::new().callback(|app, args, _cwd| {
            #[cfg(windows)]
            let args = rejoin_split_args(args);
            super::reveal(app);
            super::deliver_links(app, args, super::links::LinkSource::Forwarded);
        });
        #[cfg(target_os = "linux")]
        let builder = builder.dbus_id(dbus_id(root));
        #[cfg(not(target_os = "linux"))]
        let _ = root;
        Some(builder.build())
    }
}

/// Debug builds only: set, a refused launch prints its refusal and shows no
/// dialog (the e2e harness's refused launches, which no one answers).
#[cfg(debug_assertions)]
pub const NO_LAUNCH_DIALOG: &str = "DW_NO_LAUNCH_DIALOG";

/// Gives up the single-instance plugin's claim (its D-Bus name, or its
/// mutex and message window), so a launch made while this one shows its
/// refusal no longer hands its links to a process about to exit. Nothing
/// without the plugin.
pub fn release_single_instance<R: Runtime, M: tauri::Manager<R>>(app: &M) {
    #[cfg(not(target_os = "macos"))]
    tauri_plugin_single_instance::destroy(app);
    #[cfg(target_os = "macos")]
    let _ = app;
}

/// Shows the dialog of a launch the instance lock refused and waits for its
/// button; the launch exits after it. It runs before the event loop (and,
/// without the single-instance plugin, before Tauri starts), so it is the
/// OS's own message box (GTK's on Linux), not the dialog plugin's.
pub fn show_refusal(dialog: crate::instance::RefusalDialog) {
    #[cfg(debug_assertions)]
    if std::env::var_os(NO_LAUNCH_DIALOG).is_some() {
        return;
    }
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::{DialogExt, GtkWindowExt};
        // A dialog on its own GTK thread (rfd's) never shows once Tauri has
        // started GTK on this one.
        if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
            return;
        }
        let message = gtk::MessageDialog::new(
            None::<&gtk::Window>,
            gtk::DialogFlags::MODAL,
            gtk::MessageType::Info,
            gtk::ButtonsType::None,
            dialog.body,
        );
        message.set_title(dialog.title);
        message.add_button(dialog.button, gtk::ResponseType::Ok);
        message.run();
        message.close();
    }
    #[cfg(not(target_os = "linux"))]
    let _ = rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Info)
        .set_title(dialog.title)
        .set_description(dialog.body)
        .set_buttons(rfd::MessageButtons::OkCustom(dialog.button.to_owned()))
        .show();
}

/// Puts back together the link that the Windows plugin's `|` split cut up
/// (D22): every piece from the first one with a link scheme on is one link,
/// joined again with `|`. So no piece of a split ever becomes a link of its
/// own (`pay:…?label=a|dash:…` stays one payment link, its label holding
/// the rest). A forwarded Windows launch thus carries at most one link,
/// which is what the registered `"%1"` handler passes; arguments after a
/// link are not supported there.
pub fn rejoin_split_args(mut args: Vec<String>) -> Vec<String> {
    if let Some(at) = args
        .iter()
        .position(|a| super::links::link_scheme(a).is_some())
    {
        let link = args.split_off(at).join("|");
        args.push(link);
    }
    args
}

/// The D-Bus name prefix of the instance for the canonical data root
/// `root`: the bundle identifier and a 64-bit FNV-1a of the path. An
/// element may not start with a digit, hence the `d`.
pub fn dbus_id(root: &Path) -> String {
    let hash = root
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("org.dash.wallet.desktop.d{hash:016x}")
}

/// Saves the main window's size, position, maximised and full-screen state
/// at exit and restores them at launch, in `<data root>/window-state.json`
/// (`run` always sets `data_dir` to the resolved root; without one, the
/// platform config directory). `--window-size`
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
            std::path::absolute(dir)
                .unwrap_or_else(|_| dir.clone())
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

    /// The queued links as `take_links` routes them without a session.
    fn routed(shell: &super::super::Shell) -> Vec<super::super::links::LinkRoute> {
        shell
            .inbox
            .drain("regtest")
            .unwrap()
            .into_iter()
            .map(|l| crate::shell::links::route("regtest", &l.unwrap()).unwrap())
            .collect()
    }

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
    fn one_root_has_one_bus_name_however_it_is_spelled() {
        use crate::instance::data_root;
        let dir = dw_testutil::private_tempdir();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let via_link = dir.path().join("link/wallet");
        // The default root before it exists, then the same as `--datadir`
        // through a symlinked parent, and directly.
        let default = data_root(None, || Some(via_link.clone())).unwrap();
        let name = dbus_id(&default);
        for spelling in [
            &via_link,
            &real.join("wallet"),
            &dir.path().join("link/../real/wallet"),
        ] {
            assert_eq!(
                dbus_id(&data_root(Some(spelling), || unreachable!()).unwrap()),
                name
            );
        }
        assert_ne!(
            name,
            dbus_id(&data_root(Some(&real.join("other")), || unreachable!()).unwrap())
        );
    }

    #[test]
    fn a_split_link_is_rejoined_and_never_becomes_two() {
        const A: &str = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg";
        const B: &str = "Xr8vG1hSKgCjSD3zcEKGDzWmW5tVJsbQvr";
        let exe = r"C:\Program Files\Dash Wallet\dash-wallet.exe".to_string();
        // What the primary gets for `dash-wallet.exe "<link>"`: the plugin
        // joined the arguments with `|` and split them there.
        let split = |link: &str| -> Vec<String> {
            std::iter::once(exe.clone())
                .chain(link.split('|').map(String::from))
                .collect()
        };
        let label = format!("pay:{A}?label=a|b");
        assert_eq!(rejoin_split_args(split(&label)), [exe.clone(), label]);
        let two = format!("pay:{A}?label=a|dash:{B}");
        let rejoined = rejoin_split_args(split(&two));
        assert_eq!(rejoined, [exe.clone(), two]);
        // One link reaches the queue, not a `pay:` and a `dash:`.
        let shell = super::super::Shell::new(None);
        assert_eq!(
            shell.queue(rejoined, super::super::links::LinkSource::Forwarded),
            1
        );
        // No link: nothing changes.
        let flags = vec![exe.clone(), "--datadir".into(), "C:\\w".into()];
        assert_eq!(rejoin_split_args(flags.clone()), flags);
    }

    /// CI's Windows deep-link leg (`ci/github/deep-links.sh`), as the
    /// running app receives it. The OS starts `"dash-wallet.exe" "%1"` with
    /// the link after the Windows shell gave it a `/`; that launch's plugin
    /// sends `{cwd}|{argv joined with |}` (tauri-plugin-single-instance
    /// 2.5.3, windows.rs), and the running app's plugin splits it at every
    /// `|`, takes the first piece as the cwd and hands the rest over.
    #[test]
    fn windows_forwarded_links_route_as_ci_sends_them() {
        use super::super::Shell;
        use super::super::links::{LinkRoute, LinkSource};
        const ADDRESS: &str = "yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV";
        const IDENTITY: &str = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";
        let exe = r"C:\Users\runneradmin\AppData\Local\Dash Wallet\dash-wallet.exe";
        let forwarded = |link: &str| -> Vec<String> {
            let data = format!(r"C:\Windows\system32|{exe}|{link}");
            let mut pieces = data.split('|');
            let _cwd = pieces.next().unwrap();
            rejoin_split_args(pieces.map(String::from).collect())
        };
        let shell = Shell::new(None);
        for link in [
            format!("pay:{ADDRESS}?label=Second"),
            format!("dashpay://user/?id={IDENTITY}&username=bob"),
            "dashwallet://scanqr/".to_owned(),
        ] {
            assert_eq!(
                shell.queue_from(forwarded(&link), LinkSource::Forwarded, true),
                1
            );
        }
        let routes = routed(&shell);
        assert!(
            matches!(&routes[0], LinkRoute::Pay { uri, .. } if uri.address == ADDRESS),
            "{routes:?}"
        );
        assert_eq!(
            routes[1],
            LinkRoute::AddContact {
                identity: IDENTITY.into(),
                username: "bob".into()
            }
        );
        assert_eq!(routes[2], LinkRoute::ScanQr);
        // Without the shell's slash undone, the dashpay: link is no link
        // (CI run 38558102495: "routed rejected notALink").
        let slashed = format!("dashpay://user/?id={IDENTITY}&username=bob");
        assert_eq!(
            shell.queue_from(forwarded(&slashed), LinkSource::Forwarded, false),
            1
        );
        assert!(matches!(routed(&shell)[..], [LinkRoute::Rejected { .. }]));
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
