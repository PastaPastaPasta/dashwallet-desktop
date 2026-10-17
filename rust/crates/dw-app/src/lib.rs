//! dashwallet-desktop on Tauri 2 (G-04, ADR 0003; E0-13 binds the facades).
//!
//! The engine is linked in-process: the commands in [`api`] call the
//! dw-engine facades directly, and engine events reach the renderer through
//! [`bridge`]. The renderer is `apps/desktop` (React + TypeScript); its IPC
//! types and command wrappers are generated ([`codegen`]).

pub mod api;
pub mod bridge;
pub mod codegen;
pub mod coverage;
pub mod dto;
pub mod error;
#[cfg(test)]
mod history_vectors_tests;
pub mod instance;
mod ipc;
#[cfg(test)]
mod ipc_tests;
pub mod launch;
pub mod settings;
pub mod shell;
pub mod state;

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tauri::plugin::TauriPlugin;
use tauri::{Manager, RunEvent, Runtime, Url};

use crate::error::{AppCode, AppError};
use crate::launch::{LaunchOptions, ParseOutcome};
use crate::shell::Shell;
use crate::shell::links::LinkSource;
use crate::state::AppState;

/// Origins the main webview may show: the bundled assets (`tauri://localhost`
/// on Linux and macOS, `http://tauri.localhost` on Windows) and, in debug
/// builds, the Vite dev server.
pub fn is_app_url(url: &Url) -> bool {
    match (url.scheme(), url.host_str()) {
        ("tauri", Some("localhost")) => true,
        ("http" | "https", Some("tauri.localhost")) => true,
        ("http", Some("localhost")) if cfg!(debug_assertions) => url.port() == Some(1420),
        _ => false,
    }
}

/// "Dash Wallet", with " — Testnet" (or another network name) off mainnet (DASHPAY R11).
pub fn window_title(network: &dw_engine::DashNetwork) -> String {
    use dw_engine::DashNetwork as N;
    match network {
        N::Mainnet => "Dash Wallet".into(),
        N::Testnet => "Dash Wallet — Testnet".into(),
        N::Regtest => "Dash Wallet — Regtest".into(),
        N::Devnet { name } => format!("Dash Wallet — Devnet {name}"),
    }
}

/// Refuses every navigation away from the app (U6): a link in
/// attacker-supplied text can never load a remote page into the window that
/// holds the IPC bridge.
fn navigation_guard<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("navigation-guard")
        .on_navigation(|_, url| {
            let allowed = is_app_url(url);
            if !allowed {
                let url = url.to_string();
                eprintln!(
                    "dw-app: navigation refused: {}",
                    dw_uri::redact::redact_links(&url)
                );
            }
            allowed
        })
        .build()
}

/// SIGTERM and SIGINT end the app through `AppHandle::exit`, so the engine
/// shutdown in `run` (`RunEvent::Exit`) still flushes the wallet databases.
fn exit_on_signal<R: Runtime>(app: tauri::AppHandle<R>) {
    #[cfg(unix)]
    tauri::async_runtime::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            eprintln!("dw-app: could not install the signal handlers");
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        app.exit(0);
    });
    #[cfg(not(unix))]
    let _ = app;
}

/// Without a DRM render node (a VM, a CI runner, remote X) WebKitGTK's DMA-BUF
/// renderer falls back to software buffers that cost about a second before the
/// first paint; the shared-memory renderer does not (G-02: first paint 0.9–1.8 s
/// instead of 2.4–3.0 s on agentbox). Machines with a GPU keep the default, and
/// a value the user set is kept. A render node the user cannot open counts as
/// one (the check is for the device, not for access to it).
///
/// # Safety
///
/// It sets an environment variable (which WebKitGTK reads when it creates the
/// web view), so no other thread may exist: call it first in `main`.
#[cfg(target_os = "linux")]
pub unsafe fn webkit_renderer_without_gpu() {
    const VAR: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
    let render_node = std::fs::read_dir("/dev/dri").is_ok_and(|dir| {
        dir.flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
    });
    if !render_node && std::env::var_os(VAR).is_none() {
        // SAFETY: the caller guarantees that no other thread exists.
        unsafe { std::env::set_var(VAR, "1") };
    }
}

/// Milliseconds from the process's creation to now, from `/proc/self/stat`'s
/// `starttime` (field 22, in 1/100 s since boot) and `/proc/uptime`: the part
/// of a cold start before `run` (exec, dynamic linking), which the marks
/// measured from `run` cannot see. Both are CLOCK_BOOTTIME, in 10 ms steps.
#[cfg(any(target_os = "linux", test))]
fn ms_since_process_start(stat: &str, uptime: &str) -> Option<u64> {
    // The command name, in parentheses, may itself contain spaces and ')'.
    let start: u64 = stat
        .rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()?;
    let (secs, hundredths) = uptime.split_whitespace().next()?.split_once('.')?;
    let now = secs.parse::<u64>().ok()? * 100 + hundredths.parse::<u64>().ok()?;
    Some(now.checked_sub(start)? * 10)
}

/// Installs the process's logger: `filter` (EnvFilter syntax) to stderr,
/// every line through [`dw_uri::redact::RedactingWriter`], so no link or
/// part of one reaches a log (DASHPAY §3.8, DP5-01 D14).
pub fn install_logging(filter: &str) -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
        // No colour: the redaction reads escape sequences as delimiters,
        // and plain text leaves it nothing to get wrong (review r1 #1).
        .with_ansi(false)
        .with_writer(|| dw_uri::redact::RedactingWriter::new(std::io::stderr()))
        .try_init()
        .map_err(|e| e.to_string())
}

/// Entry point of the `dash-wallet` binary.
pub fn run() {
    let started = Instant::now();
    // A panic message may quote a link: printed redacted (DP5-01 D14).
    dw_uri::redact::redact_panics();
    // `DW_LOG=info,dw_engine=debug` (EnvFilter syntax) logs to stderr, as
    // dwcli's `DWCLI_LOG` does (BL-162). Off by default, without colour,
    // and a logger already set is reported, not a panic.
    if let Ok(filter) = std::env::var("DW_LOG")
        && let Err(e) = install_logging(&filter)
    {
        eprintln!("DW_LOG ignored: {e}");
    }
    // U5 measures cold start from spawn: harnesses add this to the marks.
    #[cfg(target_os = "linux")]
    if let (Ok(stat), Ok(uptime)) = (
        std::fs::read_to_string("/proc/self/stat"),
        std::fs::read_to_string("/proc/uptime"),
    ) && let Some(ms) = ms_since_process_start(&stat, &uptime)
    {
        eprintln!("dw-app: startup before-run {ms}");
    }
    let mut launch = match launch::parse(std::env::args().skip(1)) {
        Ok(ParseOutcome::Run(o)) => *o,
        Ok(ParseOutcome::Help) => {
            println!("{}", launch::usage());
            return;
        }
        Err(e) => {
            // An option's value may be a link the OS passed after it.
            let e = dw_uri::redact::redact_links(&e).into_owned();
            eprintln!("dash-wallet: {e}\n{}", launch::usage());
            std::process::exit(2);
        }
    };
    let context = tauri::generate_context!();
    // The data root first, created and canonical: the instance name, its
    // lock, the window-state file and the engine all take this one path.
    let identifier = context.config().identifier.clone();
    let root = instance::data_root(launch.data_dir.as_deref(), || {
        dirs::data_dir().map(|dir| dir.join(identifier))
    })
    .unwrap_or_else(|e| {
        let dir = launch
            .data_dir
            .as_deref()
            .unwrap_or(Path::new("the app-data directory"));
        eprintln!("dash-wallet: data directory {}: {e}", dir.display());
        std::process::exit(1);
    });
    launch.data_dir = Some(root.clone());
    // The instance lock ends a second launch on this root that cannot hand
    // its links over; it is taken before the main window, webview or tray
    // icon exists (those, and the setup hook that opens the engine, come with
    // `run`, where a setup error would panic). With the single-instance
    // plugin it waits for the build, during which the plugin forwards and
    // exits where it can. Without it (macOS, or Linux without a session
    // bus) it comes first, so a refused launch ends before GTK starts and
    // GLib autolaunches a session bus of its own.
    let take_lock = || {
        instance::InstanceLock::acquire(&root, instance::InstanceLock::LAUNCH_WAIT).unwrap_or_else(
            |e| {
                eprintln!("dash-wallet: {e}");
                std::process::exit(1);
            },
        )
    };
    let mut base = tauri::Builder::<tauri::Wry>::new();
    let single_instance = shell::native::single_instance(&root);
    let early_lock = single_instance.is_none().then(take_lock);
    if let Some(single_instance) = single_instance {
        base = base.plugin(single_instance);
    }
    if let Some(window_state) = shell::native::window_state(&launch) {
        base = base.plugin(window_state);
    }
    base = base.plugin(tauri_plugin_dialog::init());
    let app = app_builder(base, launch, started, dw_vault::VaultConfig::default())
        .menu(shell::menu::build)
        .on_menu_event(shell::menu::handle)
        .build(context)
        .unwrap_or_else(|e| {
            eprintln!("dash-wallet: could not start: {e}");
            std::process::exit(1);
        });
    app.manage(early_lock.unwrap_or_else(take_lock));
    // The self-test's grant, outside `capabilities/` so that no other build
    // embeds it (build.rs, E0-13 D17). Tauri panics on one it cannot parse
    // or resolve (`api::selftest` tests it parses; every permission exists).
    #[cfg(feature = "selftest")]
    app.add_capability(api::selftest::CAPABILITY)
        .expect("the self-test capability");
    shell::protect_secrets(app.handle());
    shell::files::install(app.handle());
    if let Err(e) = shell::tray::install(app.handle()) {
        eprintln!("dw-app: tray icon: {e}");
    }
    app.run(|handle, event| {
        // macOS hands a clicked link to the running app as an event.
        #[cfg(target_os = "macos")]
        if let RunEvent::Opened { urls } = &event {
            shell::deliver_links(
                handle,
                urls.iter().map(|u| u.to_string()),
                LinkSource::Opened,
            );
        }
        if let RunEvent::Exit = event
            && let Some(state) = handle.try_state::<AppState>()
        {
            // Flush and release every wallet database before the process ends.
            if let Err(e) = tauri::async_runtime::block_on(state.engine.shutdown()) {
                eprintln!("dash-wallet: engine shutdown: {e}");
            }
        }
    });
}

/// The app minus its context, so tests can build it on the mock runtime.
pub fn builder<R: Runtime>(launch: LaunchOptions, started: Instant) -> tauri::Builder<R> {
    builder_with_vault(launch, started, dw_vault::VaultConfig::default())
}

/// [`builder`] with the vault's configuration (tests: an in-memory OS store).
pub fn builder_with_vault<R: Runtime>(
    launch: LaunchOptions,
    started: Instant,
    vault: dw_vault::VaultConfig,
) -> tauri::Builder<R> {
    app_builder(tauri::Builder::<R>::new(), launch, started, vault)
}

/// The app on `base` (which `run` gives the native plugins).
fn app_builder<R: Runtime>(
    base: tauri::Builder<R>,
    mut launch: LaunchOptions,
    started: Instant,
    vault: dw_vault::VaultConfig,
) -> tauri::Builder<R> {
    // Managed before any plugin runs, so a link forwarded during start-up
    // waits in the queue. The launch's own links move into it.
    let shell = Arc::new(Shell::new(launch.selftest_link_trace.clone()));
    shell.queue(std::mem::take(&mut launch.links), LinkSource::Launch);
    base.manage(shell)
        .plugin(navigation_guard())
        // Start-up breakdown for U5, next to the renderer's own marks (`startup_mark`).
        .on_page_load(move |_, payload| {
            let mark = match payload.event() {
                tauri::webview::PageLoadEvent::Started => "page-load-started",
                tauri::webview::PageLoadEvent::Finished => "page-load-finished",
            };
            eprintln!("dw-app: startup {mark} {}", started.elapsed().as_millis());
        })
        .invoke_handler(api::invoke)
        .setup(move |app| {
            eprintln!("dw-app: startup setup {}", started.elapsed().as_millis());
            // `--theme` wins; otherwise the appearance Settings saved (U-04 D3).
            let theme = launch.theme.or_else(|| {
                launch
                    .data_dir
                    .as_deref()
                    .and_then(|root| settings::read(root).theme)
            });
            let window_size = launch.window_size;
            let title = window_title(&launch.network);
            let handle = app.handle();
            let state = AppState::new(handle, launch, started, vault, || {
                handle
                    .path()
                    .app_data_dir()
                    .map_err(|e| AppError::app(AppCode::DataDir, e.to_string()))
            })?;
            app.manage(state);
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(&title)?;
                if let Some(theme) = theme {
                    window.set_theme(Some(theme.into()))?;
                }
                if let Some((width, height)) = window_size {
                    window.set_size(tauri::LogicalSize::new(width, height))?;
                    // Centred for the configured size; a smaller desktop would
                    // otherwise leave the resized window partly off-screen.
                    window.center()?;
                }
            }
            exit_on_signal(handle.clone());
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_since_process_start_reads_the_kernel_clocks() {
        // Fields 3..: state, ppid, ..., starttime (22) = 4000 ticks (40.00 s).
        let stat =
            "123 (dash wallet) x) S 1 123 123 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 4000 0 0";
        assert_eq!(ms_since_process_start(stat, "41.37 99.00\n"), Some(1370));
        assert_eq!(ms_since_process_start(stat, "39.99 1.00"), None);
        assert_eq!(ms_since_process_start("123 (x) S", "41.37 1.00"), None);
        #[cfg(target_os = "linux")]
        {
            let read = |p| std::fs::read_to_string(p).unwrap();
            let ms = ms_since_process_start(&read("/proc/self/stat"), &read("/proc/uptime"));
            assert!(ms.is_some_and(|ms| ms < 3_600_000), "{ms:?}");
        }
    }

    #[test]
    fn only_app_origins_are_navigable() {
        let ok = |s: &str| is_app_url(&Url::parse(s).unwrap());
        assert!(ok("tauri://localhost/index.html"));
        assert!(ok("http://tauri.localhost/"));
        assert!(!ok("https://evil.example/"));
        assert!(!ok("file:///etc/passwd"));
        assert!(!ok("tauri://evil.example/"));
        assert!(!ok("http://localhost:8080/"));
    }

    #[test]
    fn engine_mode_opens_the_data_root() {
        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().join("data");
        let opts = LaunchOptions {
            data_dir: Some(root.clone()),
            ..LaunchOptions::default()
        };
        let state = AppState::new(
            app.handle(),
            opts,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .unwrap();
        assert_eq!(state.engine.config().data_root, root);
        assert!(root.is_dir());
    }

    #[test]
    fn a_locked_data_root_is_not_opened_twice() {
        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let root = dir.path().join("data");
        let opts = || LaunchOptions {
            data_dir: Some(root.clone()),
            ..LaunchOptions::default()
        };
        let open = || {
            AppState::new(
                app.handle(),
                opts(),
                Instant::now(),
                Default::default(),
                || unreachable!("--datadir given"),
            )
        };
        let first = open().unwrap();
        let entries = || {
            let mut names: Vec<_> = std::fs::read_dir(&root)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect();
            names.sort();
            names
        };
        let before = entries();
        let Err(second) = open() else {
            panic!("a second AppState opened a locked data root");
        };
        assert_eq!(second.code_str(), AppCode::AlreadyRunning.as_str());
        assert!(second.to_string().contains("already running"), "{second}");
        // The refused open wrote nothing into the root.
        assert_eq!(entries(), before);
        drop(first);
        open().expect("the root opens again once its holder is gone");
    }

    #[test]
    fn the_lock_run_took_is_the_one_the_state_keeps() {
        // `run` manages the lock before the setup hook builds the state,
        // which must then not try for a second one (it would be refused).
        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let root = instance::data_root(Some(&dir.path().join("data")), || unreachable!()).unwrap();
        let lock = instance::InstanceLock::acquire(&root, std::time::Duration::ZERO).unwrap();
        app.manage(lock);
        let opts = LaunchOptions {
            data_dir: Some(root.clone()),
            ..LaunchOptions::default()
        };
        AppState::new(
            app.handle(),
            opts,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .expect("the state opens under the lock run took");
    }

    #[test]
    fn window_title_names_the_network_off_mainnet() {
        use dw_engine::DashNetwork;
        assert_eq!(window_title(&DashNetwork::Mainnet), "Dash Wallet");
        assert_eq!(window_title(&DashNetwork::Testnet), "Dash Wallet — Testnet");
        assert_eq!(
            window_title(&DashNetwork::Devnet {
                name: "paloma".into()
            }),
            "Dash Wallet — Devnet paloma"
        );
    }
}
