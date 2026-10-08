//! dashwallet-desktop on Tauri 2 (gate G-UI spike, DASHPAY §2.1).
//!
//! The engine is linked in-process: commands call dw-engine directly and
//! engine events reach the renderer through [`bridge`]. The renderer is
//! `apps/desktop` (React + TypeScript); its IPC types are generated from
//! [`dto`].

pub mod bridge;
pub mod commands;
pub mod dto;
pub mod launch;

use std::time::Instant;

use tauri::plugin::TauriPlugin;
use tauri::{Manager, RunEvent, Runtime, Url};

use crate::commands::AppState;
use crate::dto::AppError;
use crate::launch::{LaunchOptions, ParseOutcome};

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
                eprintln!("dw-app: navigation refused: {url}");
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

/// Entry point of the `dash-wallet` binary.
pub fn run() {
    let started = Instant::now();
    let launch = match launch::parse(std::env::args().skip(1)) {
        Ok(ParseOutcome::Run(o)) => o,
        Ok(ParseOutcome::Help) => {
            println!("{}", launch::USAGE);
            return;
        }
        Err(e) => {
            eprintln!("dash-wallet: {e}\n{}", launch::USAGE);
            std::process::exit(2);
        }
    };
    let app = builder::<tauri::Wry>(launch, started)
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            eprintln!("dash-wallet: could not start: {e}");
            std::process::exit(1);
        });
    app.run(|handle, event| {
        if let RunEvent::Exit = event
            && let Some(engine) = handle
                .try_state::<AppState>()
                .and_then(|s| s.inner().engine.as_ref())
        {
            // Flush and release every wallet database before the process ends.
            if let Err(e) = tauri::async_runtime::block_on(engine.shutdown()) {
                eprintln!("dash-wallet: engine shutdown: {e}");
            }
        }
    });
}

/// The app minus its context, so tests can build it on the mock runtime.
pub fn builder<R: Runtime>(launch: LaunchOptions, started: Instant) -> tauri::Builder<R> {
    tauri::Builder::<R>::new()
        .plugin(navigation_guard())
        // Start-up breakdown for U5, next to the renderer's own marks (`startup_mark`).
        .on_page_load(move |_, payload| {
            let mark = match payload.event() {
                tauri::webview::PageLoadEvent::Started => "page-load-started",
                tauri::webview::PageLoadEvent::Finished => "page-load-finished",
            };
            eprintln!("dw-app: startup {mark} {}", started.elapsed().as_millis());
        })
        .invoke_handler(tauri::generate_handler![
            commands::launch_info,
            commands::open_network,
            commands::list_wallets,
            commands::lock_status,
            commands::unlock,
            commands::lock,
            commands::balance,
            commands::sync_status,
            commands::history_page,
            commands::tx_detail,
            commands::startup_mark,
            commands::selftest_report,
            commands::selftest_checkpoint,
            commands::ipc_bench_rows,
            commands::probe_unlisted,
        ])
        .setup(move |app| {
            eprintln!("dw-app: startup setup {}", started.elapsed().as_millis());
            let theme = launch.theme;
            let title = window_title(&launch.network);
            let handle = app.handle();
            let state = AppState::new(handle, launch, started, || {
                handle
                    .path()
                    .app_data_dir()
                    .map_err(|e| AppError::app("data_dir", e.to_string()))
            })?;
            app.manage(state);
            if let Some(window) = app.get_webview_window("main") {
                window.set_title(&title)?;
                if let Some(theme) = theme {
                    window.set_theme(Some(theme.into()))?;
                }
            }
            exit_on_signal(handle.clone());
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::*;

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

    fn generated_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../apps/desktop/src/api/generated")
    }

    fn read_dir_sorted(dir: &Path) -> Vec<(String, String)> {
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(Result::ok)
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().into_owned();
                        (name, std::fs::read_to_string(e.path()).unwrap_or_default())
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    /// The committed TypeScript bindings equal what the Rust records
    /// generate. Regenerate with `DW_UPDATE_BINDINGS=1 cargo test -p dw-app bindings`.
    #[test]
    fn bindings_are_current() {
        let fresh = tempfile::tempdir().unwrap();
        dto::export_all(fresh.path()).unwrap();
        let committed = generated_dir();
        if std::env::var_os("DW_UPDATE_BINDINGS").is_some() {
            let _ = std::fs::remove_dir_all(&committed);
            std::fs::create_dir_all(&committed).unwrap();
            for (name, text) in read_dir_sorted(fresh.path()) {
                std::fs::write(committed.join(name), text).unwrap();
            }
        }
        assert_eq!(
            read_dir_sorted(&committed),
            read_dir_sorted(fresh.path()),
            "apps/desktop/src/api/generated is stale; run DW_UPDATE_BINDINGS=1 cargo test -p dw-app bindings"
        );
    }

    #[test]
    fn fixture_mode_has_no_engine() {
        let app = tauri::test::mock_app();
        let opts = LaunchOptions {
            fixture: true,
            ..LaunchOptions::default()
        };
        let state = AppState::new(app.handle(), opts, Instant::now(), || {
            unreachable!("no data dir in fixture mode")
        })
        .unwrap();
        assert!(state.engine.is_none());
    }

    #[test]
    fn engine_mode_opens_the_data_root() {
        let app = tauri::test::mock_app();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("data");
        let opts = LaunchOptions {
            data_dir: Some(root.clone()),
            ..LaunchOptions::default()
        };
        let state = AppState::new(app.handle(), opts, Instant::now(), || {
            unreachable!("--datadir given")
        })
        .unwrap();
        assert_eq!(state.engine.as_ref().unwrap().config().data_root, root);
        assert!(root.is_dir());
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
