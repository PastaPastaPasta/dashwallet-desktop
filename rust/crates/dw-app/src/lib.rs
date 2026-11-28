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
mod governance_vectors_tests;
#[cfg(test)]
mod history_vectors_tests;
pub mod instance;
mod ipc;
#[cfg(test)]
mod ipc_tests;
pub mod launch;
#[cfg(test)]
mod masternodes_tests;
#[cfg(target_os = "linux")]
mod portal;
pub mod settings;
pub mod shell;
pub mod state;
pub mod updates;
#[cfg(test)]
mod upgrade_path_tests;

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

/// "DashPay Desktop", with " — Testnet" (or another network name) off mainnet (DASHPAY R11).
pub fn window_title(network: &dw_engine::DashNetwork) -> String {
    use dw_engine::DashNetwork as N;
    match network {
        N::Mainnet => "DashPay Desktop".into(),
        N::Testnet => "DashPay Desktop — Testnet".into(),
        N::Regtest => "DashPay Desktop — Regtest".into(),
        N::Devnet { name } => format!("DashPay Desktop — Devnet {name}"),
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

/// One startup phase, milliseconds from `run`. The same line the cold-start
/// harness reads (`dw-app: startup <phase> <ms>`), and an info log when
/// `DW_LOG` is set.
pub(crate) fn startup_phase(started: Instant, phase: &str) {
    let ms = started.elapsed().as_millis();
    tracing::info!(phase, elapsed_ms = ms as u64, "startup");
    eprintln!("dw-app: startup {phase} {ms}");
}

/// The line an argument error prints: redacted, though no launch error
/// quotes a value (D30), in case one ever does: the OS may pass a link
/// after an option or inside one.
pub fn launch_error(e: &str) -> String {
    format!("dash-wallet: {}", dw_uri::redact::redact_links(e))
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
            eprintln!("{}\n{}", launch_error(&e), launch::usage());
            std::process::exit(2);
        }
    };
    startup_phase(started, "args");
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
        // The path came from an argument: redacted like one (D30).
        let line = format!("data directory {}: {e}", dir.display());
        eprintln!("{}", launch_error(&line));
        std::process::exit(1);
    });
    launch.data_dir = Some(root.clone());
    startup_phase(started, "data-root");
    // The instance lock ends a second launch on this root that cannot hand
    // its links over; it is taken before the main window, webview or tray
    // icon exists (those come with `run`). The setup hook shows the window
    // and returns before the engine is built: it runs on the event loop's
    // Ready, and the window cannot paint until that hook returns. With the
    // single-instance plugin it waits for the build, during which the plugin forwards and
    // exits where it can; a refused launch gives the plugin's claim up
    // before its dialog, so later launches do not hand their links to it.
    // Without it (macOS, or Linux without a session bus) it comes first, so
    // a refused launch ends before Tauri starts.
    let take_lock = |release: &dyn Fn()| {
        instance::InstanceLock::acquire(&root, instance::InstanceLock::LAUNCH_WAIT).unwrap_or_else(
            |e| {
                eprintln!("dash-wallet: {e}");
                instance::refuse(&e, release, shell::native::show_refusal);
                std::process::exit(1);
            },
        )
    };
    let mut base = tauri::Builder::<tauri::Wry>::new();
    let single_instance = shell::native::single_instance(&root);
    let lock_and_mark = |release: &dyn Fn()| {
        let lock = take_lock(release);
        startup_phase(started, "instance");
        lock
    };
    let early_lock = single_instance.is_none().then(|| lock_and_mark(&|| ()));
    if let Some(single_instance) = single_instance {
        base = base.plugin(single_instance);
    }
    // Before `build`, which registers the GTK application. A portal that
    // cannot start would otherwise be activated from that register and the
    // window would not exist until the activation gave up.
    #[cfg(target_os = "linux")]
    portal::skip_portal_autostart_if_unavailable();
    #[cfg(target_os = "linux")]
    startup_phase(started, "portal");
    if let Some(window_state) = shell::native::window_state(&launch) {
        base = base.plugin(window_state);
    }
    base = base.plugin(tauri_plugin_dialog::init());
    if updates::build_has_updater(context.config()) {
        base = base.plugin(updates::plugin());
    }
    let app = app_builder(base, launch, started, dw_vault::VaultConfig::default())
        .menu(shell::menu::build)
        .on_menu_event(shell::menu::handle)
        .build(context)
        .unwrap_or_else(|e| {
            eprintln!("dash-wallet: could not start: {e}");
            std::process::exit(1);
        });
    startup_phase(started, "built");
    let lock = match early_lock {
        Some(lock) => lock,
        None => lock_and_mark(&|| shell::native::release_single_instance(&app)),
    };
    app.manage(lock);
    // The self-test's grant, outside `capabilities/` so that no other build
    // embeds it (build.rs, E0-13 D17). Tauri panics on one it cannot parse
    // or resolve (`api::selftest` tests it parses; every permission exists).
    #[cfg(feature = "selftest")]
    app.add_capability(api::selftest::CAPABILITY)
        .expect("the self-test capability");
    shell::sleep::install(app.handle());
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
            shell::quit::hide_for_exit(handle);
            // Flush and release every wallet database before the process ends.
            if let Err(e) = tauri::async_runtime::block_on(state.shutdown_engine()) {
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
            startup_phase(started, mark);
        })
        .invoke_handler(api::invoke)
        .setup(move |app| {
            startup_phase(started, "setup");
            // `--theme` wins; otherwise the appearance Settings saved (U-04 D3).
            let theme = launch.theme.or_else(|| {
                launch
                    .data_dir
                    .as_deref()
                    .and_then(|root| settings::read(root).theme)
            });
            let window_size = launch.window_size;
            let handle = app.handle().clone();
            // No engine yet. This hook is the event loop's Ready: it has
            // already created the window, and nothing paints until it
            // returns. The build (runtime, rates client) runs afterwards.
            let state = AppState::pending(&handle, launch, started, vault, || {
                handle
                    .path()
                    .app_data_dir()
                    .map_err(|e| AppError::app(AppCode::DataDir, e.to_string()))
            })?;
            // After the state: it resolves the saved network choice.
            let title = window_title(&state.window_network());
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
                window.show()?;
            }
            startup_phase(started, "window-ready");
            app.manage(state);
            #[cfg(test)]
            if let Some(hook) = crate::state::take_engine_start_hook() {
                app.state::<AppState>().start_engine_after(hook);
            } else {
                app.state::<AppState>().start_engine();
            }
            #[cfg(not(test))]
            app.state::<AppState>().start_engine();
            exit_on_signal(handle);
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
        assert_eq!(state.engine.ready().unwrap().config().data_root, root);
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
        // The engine releases its storage, and with it the root, on its
        // drop thread, shortly after the state is dropped.
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match open() {
                Ok(_) => break,
                Err(e)
                    if e.code_str() == AppCode::AlreadyRunning.as_str()
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("the root opens again once its holder is gone: {e:?}"),
            }
        }
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

    /// A panic in the build hook reaches every waiter that was already
    /// parked, `spv_state` stops saying `starting`, and Retry builds the engine.
    #[test]
    fn a_panicked_engine_build_fails_then_retries() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use crate::api::chain::spv_state_of;

        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let opts = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            ..LaunchOptions::default()
        };
        let state = AppState::pending(
            app.handle(),
            opts,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        state.start_engine_after(move || {
            let _ = entered_tx.send(());
            let _ = go_rx.recv();
            panic!("engine build failed");
        });
        assert!(
            entered_rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the build thread did not start"
        );

        let mut waiters = Vec::new();
        for _ in 0..3 {
            let engine = state.engine.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let result = tauri::async_runtime::block_on(engine.wait())
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
            waiters.push(rx);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while state.engine.parked_waiters() < 3 {
            assert!(
                Instant::now() < deadline,
                "waiters did not park before the failure"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        go_tx.send(()).unwrap();

        for rx in waiters {
            let err = rx
                .recv_timeout(Duration::from_secs(5))
                .expect("a parked waiter did not hear the failure")
                .expect_err("a panicked build installed an engine");
            assert!(err.contains("stopped while starting"), "{err}");
        }
        let spv = tauri::async_runtime::block_on(spv_state_of(&state));
        assert!(spv.is_err(), "spv_state still reports a build: {spv:?}");
        state.retry_engine_if_failed();
        let engine = tauri::async_runtime::block_on(state.engine.wait()).expect("retried");
        assert!(tauri::async_runtime::block_on(engine.existing_networks()).is_ok());
        assert!(tauri::async_runtime::block_on(state.shutdown_engine()).is_ok());
    }

    /// Idle does not admit a retry, and two retries of one failure admit one build.
    #[test]
    fn two_retries_of_a_failed_build_start_one_engine() {
        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let opts = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            ..LaunchOptions::default()
        };
        let state = AppState::pending(
            app.handle(),
            opts,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .unwrap();
        state.retry_engine_if_failed();
        assert_eq!(state.engine.builds_started(), 0);
        state.start_engine_after(|| panic!("engine build failed"));
        let _ = tauri::async_runtime::block_on(state.engine.wait());
        let started = state.engine.builds_started();
        std::thread::scope(|scope| {
            scope.spawn(|| state.retry_engine_if_failed());
            scope.spawn(|| state.retry_engine_if_failed());
        });
        tauri::async_runtime::block_on(state.engine.wait()).expect("one retry");
        assert_eq!(state.engine.builds_started(), started + 1);
    }

    /// Quit while the build is held wakes parked waiters and returns without
    /// waiting on the build or publishing a late engine.
    #[test]
    fn a_quit_during_startup_does_not_wait_for_the_build() {
        use std::sync::mpsc;
        use std::time::Duration;

        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let opts = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            ..LaunchOptions::default()
        };
        let state = AppState::pending(
            app.handle(),
            opts,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (go_tx, go_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        state.start_engine_after(move || {
            let _ = entered_tx.send(());
            let _ = go_rx.recv();
            let _ = done_tx.send(());
        });
        assert!(entered_rx.recv_timeout(Duration::from_secs(5)).is_ok());

        let waiter_engine = state.engine.clone();
        let (waiter_tx, waiter_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let res = tauri::async_runtime::block_on(waiter_engine.wait())
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = waiter_tx.send(res);
        });
        let park_deadline = Instant::now() + Duration::from_secs(5);
        while state.engine.parked_waiters() < 1 {
            assert!(Instant::now() < park_deadline, "waiter did not park");
            std::thread::sleep(Duration::from_millis(10));
        }
        // The build is still held: a quit that waited on it would never
        // return here.
        let quit = std::thread::scope(|scope| {
            let (quit_tx, quit_rx) = mpsc::channel();
            let state = &state;
            scope.spawn(move || {
                let _ = quit_tx.send(tauri::async_runtime::block_on(state.shutdown_engine()));
            });
            let quit = quit_rx.recv_timeout(Duration::from_secs(10));
            go_tx.send(()).unwrap();
            quit
        });
        assert!(
            quit.expect("quit waited on the build").is_ok(),
            "quit during startup failed"
        );
        let waiter_err = waiter_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("parked waiter was not woken on quit")
            .expect_err("waiter succeeded after quit");
        assert!(waiter_err.contains("quit while the engine was still starting"));

        assert!(done_rx.recv_timeout(Duration::from_secs(5)).is_ok());
        let settle_deadline = Instant::now() + Duration::from_secs(60);
        while state.engine.builds_settled() < 1 {
            assert!(Instant::now() < settle_deadline, "the build did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !state.engine.is_ready(),
            "a late build was published after quit"
        );
        state.retry_engine_if_failed();
        assert_eq!(state.engine.builds_started(), 1);
    }

    #[test]
    fn window_title_names_the_network_off_mainnet() {
        use dw_engine::DashNetwork;
        assert_eq!(window_title(&DashNetwork::Mainnet), "DashPay Desktop");
        assert_eq!(
            window_title(&DashNetwork::Testnet),
            "DashPay Desktop — Testnet"
        );
        assert_eq!(
            window_title(&DashNetwork::Devnet {
                name: "paloma".into()
            }),
            "DashPay Desktop — Devnet paloma"
        );
    }
}
