//! The commands over Tauri's IPC against a real engine (E0-13 acceptance):
//! the app built as `run` builds it, on the mock runtime, with an engine in a
//! private temporary data root, an in-memory OS store and cheap KDF
//! parameters. Requests go through the webview's IPC handler with camelCase
//! JSON arguments, as the generated wrappers send them; results and errors
//! come back as the renderer sees them.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use dw_engine::DashNetwork;
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use serde_json::{Value, json};
use tauri::test::{MockRuntime, mock_context, noop_assets};
use tauri::webview::InvokeRequest;
use tauri::{App, Manager, WebviewWindow};

use crate::error::ErrorCode;
use crate::launch::LaunchOptions;

pub(crate) struct Demo {
    // Fields drop in order: the webview and the app (with its engine) before
    // the data root is deleted.
    webview: WebviewWindow<MockRuntime>,
    app: App<MockRuntime>,
    _dir: tempfile::TempDir,
}

/// Unreachable local endpoints: nothing leaves the machine.
pub(crate) fn offline_settings() -> Value {
    json!({
        "dapiAddresses": ["http://127.0.0.1:1"],
        "quorumUrl": "http://127.0.0.1:1",
        "spvPeers": ["127.0.0.1:1"],
        "noPlatform": true,
    })
}

impl Demo {
    pub(crate) fn new() -> Self {
        Self::launched(|_, _| {})
    }

    /// A demo whose launch options `edit` changes (given the private data
    /// root), as a command line would: `--selftest <file>`, for one.
    pub(crate) fn launched(edit: impl FnOnce(&mut LaunchOptions, &std::path::Path)) -> Self {
        Self::launched_in(dw_testutil::private_tempdir(), edit)
    }

    fn launched_in(
        dir: tempfile::TempDir,
        edit: impl FnOnce(&mut LaunchOptions, &std::path::Path),
    ) -> Self {
        Self::start(dir, edit, true)
    }

    /// Like [`Self::launched_in`], without waiting for the engine. A test that
    /// holds the build uses this so `run_iteration` can return first.
    pub(crate) fn open_unready(edit: impl FnOnce(&mut LaunchOptions, &std::path::Path)) -> Self {
        Self::start(dw_testutil::private_tempdir(), edit, false)
    }

    fn start(
        dir: tempfile::TempDir,
        edit: impl FnOnce(&mut LaunchOptions, &std::path::Path),
        wait_engine: bool,
    ) -> Self {
        let mut launch = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            network: DashNetwork::Regtest,
            ..LaunchOptions::default()
        };
        edit(&mut launch, dir.path());
        let vault = VaultConfig {
            kdf: KdfPolicy::Fixed(KdfParams::TEST),
            os_store: Arc::new(MemoryOsStore::new()),
            ..VaultConfig::default()
        };
        // No default menu: muda creates macOS menu items on the main thread
        // only, and the test harness runs tests on worker threads.
        let mut app = crate::builder_with_vault::<MockRuntime>(launch, Instant::now(), vault)
            .enable_macos_default_menu(false)
            .build(mock_context(noop_assets()))
            .expect("app");
        // The first iteration runs `setup`, which creates the engine. Called
        // once, not in a loop (the deprecation is about busy-looping).
        #[allow(deprecated)]
        app.run_iteration(|_, _| {});
        // Setup returns before the engine is built. Wait, so a command and
        // the data-root drop do not race the build thread.
        if wait_engine {
            let state = app.state::<crate::state::AppState>();
            tauri::async_runtime::block_on(state.engine.wait()).expect("the engine starts");
        }
        let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
            .build()
            .expect("webview");
        Demo {
            webview,
            app,
            _dir: dir,
        }
    }

    /// Invokes `cmd` with `args` (a JSON object, camelCase keys).
    pub(crate) fn call(&self, cmd: &str, args: Value) -> Result<Value, Value> {
        invoke(&self.webview, self.app.invoke_key(), cmd, args)
    }

    /// `call`, which must succeed.
    pub(crate) fn ok(&self, cmd: &str, args: Value) -> Value {
        self.call(cmd, args)
            .unwrap_or_else(|e| panic!("{cmd} failed: {e}"))
    }

    /// `call`, which must fail with an `AppError` of `code`.
    pub(crate) fn err(&self, cmd: &str, args: Value, code: &str) -> Value {
        let e = match self.call(cmd, args) {
            Ok(v) => panic!("{cmd} succeeded with {v}; expected {code}"),
            Err(e) => e,
        };
        assert_app_error(&e);
        assert_eq!(e["code"], code, "{cmd}: {e}");
        e
    }

    pub(crate) fn handle(&self) -> &tauri::AppHandle<MockRuntime> {
        self.app.handle()
    }

    /// Opens the launch network (regtest) offline.
    pub(crate) fn open(&self) -> Value {
        self.ok("open_network", json!({ "settings": offline_settings() }))
    }

    /// Every engine and shell event payload sent from now on.
    pub(crate) fn events(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
        use tauri::Listener;
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        for event in [crate::bridge::EVENT, crate::shell::EVENT] {
            let seen = Arc::clone(&seen);
            self.app.listen_any(event, move |e| {
                seen.lock().unwrap().push(e.payload().to_owned());
            });
        }
        seen
    }
}

/// [`Demo::call`] without the app, so several threads can invoke at once.
pub(crate) fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    invoke_key: &str,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let response = tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: tauri::ipc::CallbackFn(0),
            error: tauri::ipc::CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: tauri::ipc::InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: invoke_key.to_string(),
        },
    );
    response.map(|b| b.deserialize::<Value>().expect("JSON result"))
}

/// `e` has the `AppError` shape and a code of the generated union.
pub(crate) fn assert_app_error(e: &Value) {
    let codes: HashSet<&str> = ErrorCode::all().iter().map(|c| c.as_str()).collect();
    let code = e["code"]
        .as_str()
        .unwrap_or_else(|| panic!("not an AppError: {e}"));
    assert!(codes.contains(code), "{code} is not in ErrorCode");
    assert!(e["message"].is_string(), "{e}");
    assert!(e.get("data").is_some(), "{e}");
}

/// Setup (`run_iteration` on the app `builder_with_vault` builds) returns
/// while the engine hook is still held. The gate is released before the
/// assertions, so a failed assert cannot leave the build thread parked.
/// `spv_state` over IPC is `starting` for that window, and the window-ready
/// mark is earlier than the engine.
#[test]
fn the_setup_hook_returns_before_the_engine_is_built() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    struct Release(Option<mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    let (release_tx, release_rx) = mpsc::channel();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (snap_tx, snap_rx) = mpsc::channel::<(bool, Value, Option<Instant>, Option<Instant>)>();
    let (go_tx, go_rx) = mpsc::channel();
    let gate = Release(Some(release_tx));
    let finish = Release(Some(go_tx));

    let worker = std::thread::spawn(move || {
        crate::state::set_engine_start_hook(move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv();
        });
        let demo = Demo::open_unready(|_, _| {});
        let (ready, window_at, engine_at) = {
            let state = demo.app.state::<crate::state::AppState>();
            (
                state.engine.is_ready(),
                state.engine.window_ready_at(),
                state.engine.engine_ready_at(),
            )
        };
        let spv = demo.ok("spv_state", json!({}));
        // Still held: closing must not wait for the build, and a bad
        // network name is rejected before the wait or a retry.
        assert_eq!(demo.ok("close_network", json!({})), json!(false));
        demo.err(
            "open_network",
            json!({ "network": "moon" }),
            "invalid_argument",
        );
        let _ = snap_tx.send((ready, spv, window_at, engine_at));
        let _ = go_rx.recv();
        {
            let state = demo.app.state::<crate::state::AppState>();
            let engine = tauri::async_runtime::block_on(state.engine.wait())
                .expect("the engine starts after the hold");
            let window_at = state.engine.window_ready_at().expect("window mark");
            let engine_at = state.engine.engine_ready_at().expect("engine mark");
            assert!(window_at < engine_at);
            let _ = engine;
        }
        assert_eq!(demo.open()["network"]["dirName"], "regtest");
    });

    let entered = entered_rx.recv_timeout(Duration::from_secs(20));
    let snap = snap_rx.recv_timeout(Duration::from_secs(20));
    drop(gate);
    assert!(entered.is_ok(), "the build thread did not start");
    let (ready, spv, window_at, engine_at) = snap.expect("setup waited for the engine");
    assert!(!ready, "setup built the engine before returning");
    assert_eq!(spv, json!("starting"));
    assert!(window_at.is_some(), "setup did not mark the window");
    assert!(
        engine_at.is_none(),
        "the engine was marked before setup returned"
    );
    drop(finish);
    worker.join().expect("setup thread");
}

#[test]
fn a_failed_engine_build_rejects_bad_network_without_retrying_and_closes_false() {
    crate::state::set_engine_start_hook(|| panic!("engine build failed"));
    let demo = Demo::open_unready(|_, _| {});
    {
        let state = demo.app.state::<crate::state::AppState>();
        assert!(tauri::async_runtime::block_on(state.engine.wait()).is_err());
        assert_eq!(state.engine.builds_started(), 1);
    }
    assert_eq!(demo.ok("close_network", json!({})), json!(false));
    demo.err(
        "open_network",
        json!({ "network": "moon" }),
        "invalid_argument",
    );
    {
        let state = demo.app.state::<crate::state::AppState>();
        assert_eq!(state.engine.builds_started(), 1);
    }
    assert_eq!(demo.open()["network"]["dirName"], "regtest");
    {
        let state = demo.app.state::<crate::state::AppState>();
        assert_eq!(state.engine.builds_started(), 2);
    }
}

/// About and Information read the stamped build version from
/// `launch_info`'s `buildVersion`.
#[test]
fn launch_info_returns_the_stamped_build_version() {
    let demo = Demo::new();
    let launch = demo.ok("launch_info", json!({}));
    assert_eq!(launch["buildVersion"], dw_engine::BUILD_VERSION, "{launch}");
}

#[test]
fn engine_and_session_commands_over_ipc() {
    let demo = Demo::new();
    // Session commands before a network is open.
    demo.err("list_wallets", json!({}), "network_not_open");
    assert_eq!(demo.ok("active_network", json!({})), Value::Null);

    let launch = demo.ok("launch_info", json!({}));
    assert_eq!(launch["network"]["dirName"], "regtest");

    // Engine::open_network, with the renderer's settings over the launch's.
    let opened = demo.open();
    assert_eq!(opened["network"]["dirName"], "regtest");
    assert_eq!(opened["network"]["unitName"], "tDASH");
    assert_eq!(demo.ok("active_network", json!({}))["dirName"], "regtest");
    // Already active: the same session, settings ignored.
    assert_eq!(demo.ok("open_network", json!({})), opened);
    // Trust settings on mainnet and testnet come from launch flags only;
    // refused before the active session is touched.
    demo.err(
        "open_network",
        json!({ "network": "testnet", "settings": { "quorumUrl": "https://evil.example" } }),
        "invalid_argument",
    );
    assert_eq!(demo.ok("active_network", json!({}))["dirName"], "regtest");
    // An argument the binding parses itself.
    demo.err(
        "open_network",
        json!({ "network": "moon" }),
        "invalid_argument",
    );

    // Engine::existing_networks
    let networks = demo.ok("existing_networks", json!({}));
    assert!(
        networks
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["network"]["dirName"] == "regtest"),
        "{networks}"
    );

    // NetworkSession and Vault reads.
    assert_eq!(demo.ok("list_wallets", json!({})), json!([]));
    assert_eq!(demo.ok("lock_status", json!({})), opened["lock"]);
    assert!(demo.ok("sync_status", json!({}))["phases"].is_array());
    demo.err(
        "balance",
        json!({ "walletId": "not hex" }),
        "invalid_argument",
    );

    // Closing drops the active session.
    assert_eq!(demo.ok("close_network", json!({})), true);
    assert_eq!(demo.ok("close_network", json!({})), false);
    demo.err("lock_status", json!({}), "network_not_open");
}

#[test]
fn recover_in_fresh_folder_reaches_the_engine_only_after_a_refused_open() {
    let demo = Demo::new();
    let root = demo.app.state::<crate::state::AppState>().data_root.clone();
    demo.err("recover_in_fresh_folder", json!({}), "invalid_argument");
    demo.open();
    demo.err("recover_in_fresh_folder", json!({}), "invalid_argument");
    assert!(!root.join("network-folders.json").exists());
    assert!(!root.join("regtest-recovered-1").exists());
}

#[test]
fn a_missing_argument_is_tauris_refusal() {
    let demo = Demo::new();
    // A string, not an AppError.
    let e = demo.call("startup_mark", json!({})).unwrap_err();
    assert!(e.is_string(), "{e}");
    assert!(demo.ok("startup_mark", json!({ "mark": "test" })).is_u64());
}

/// Without the `selftest` feature (every release and packaged build) the
/// self-test's commands are not registered, even in an app whose launch
/// options ask for the self-test: Tauri answers "not found" (E0-13 D17).
#[cfg(not(feature = "selftest"))]
#[test]
fn a_build_without_the_selftest_feature_registers_no_selftest_command() {
    let demo = Demo::launched(|launch, dir| {
        launch.selftest = Some(dir.join("report.json"));
        launch.selftest_passphrase_file = Some(dir.join("passphrase"));
    });
    assert_eq!(crate::api::selftest::NAMES.len(), 6);
    for name in crate::api::selftest::NAMES {
        let e = demo.call(name, json!({})).unwrap_err();
        assert!(
            e.as_str().is_some_and(|e| e.contains("not found")),
            "{name}: {e}"
        );
    }
}

#[cfg(feature = "selftest")]
#[test]
fn the_selftest_commands_refuse_without_selftest() {
    let demo = Demo::new();
    demo.err(
        "selftest_checkpoint",
        json!({ "screen": "x" }),
        "app.not_selftest",
    );
    demo.err("ipc_bench_rows", json!({ "rows": 2 }), "app.not_selftest");
    demo.err("selftest_unlock", json!({}), "app.not_selftest");
}

#[cfg(feature = "selftest")]
#[test]
fn the_selftest_commands_run_under_selftest() {
    let demo = Demo::launched(|launch, dir| launch.selftest = Some(dir.join("report.json")));
    let page = demo.ok("ipc_bench_rows", json!({ "rows": 2 }));
    assert_eq!(page["rows"].as_array().map(Vec::len), Some(2), "{page}");
    assert_eq!(
        demo.ok("selftest_checkpoint", json!({ "screen": "x" })),
        Value::Null
    );
    // No passphrase file: the self-test cannot unlock.
    demo.err("selftest_unlock", json!({}), "app.selftest");
}

/// `--selftest-passphrase-file`: `selftest_unlock` unlocks the vault with
/// the file's first line, and a passphrase that no longer opens it fails as
/// `unlock` does.
#[cfg(feature = "selftest")]
#[test]
fn the_selftest_unlocks_with_the_passphrase_file() {
    let demo = Demo::launched(|launch, dir| {
        launch.selftest = Some(dir.join("report.json"));
        launch.selftest_passphrase_file = Some(dir.join("passphrase"));
        write_passphrase_file(&dir.join("passphrase"), "selftest pass\nignored\n", 0o600);
    });
    assert_eq!(demo.ok("launch_info", json!({}))["selftestUnlock"], true);
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "selftest pass" }));
    demo.ok("lock", json!({}));
    assert_eq!(demo.ok("selftest_unlock", json!({}))["state"], "unlocked");
    demo.ok(
        "change_passphrase",
        json!({ "oldPassphrase": "selftest pass", "newPassphrase": "other" }),
    );
    demo.ok("lock", json!({}));
    demo.err("selftest_unlock", json!({}), "vault.wrong_passphrase");
}

/// The passphrase file at `path`, with `mode` on Unix.
#[cfg(feature = "selftest")]
fn write_passphrase_file(path: &std::path::Path, text: &str, mode: u32) {
    std::fs::write(path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = mode;
}

/// A passphrase file others can read is refused before the vault is
/// touched, with a code the self-test's report carries (E0-13 D16).
#[cfg(all(unix, feature = "selftest"))]
#[test]
fn the_selftest_refuses_a_passphrase_file_others_can_read() {
    let demo = Demo::launched(|launch, dir| {
        launch.selftest = Some(dir.join("report.json"));
        launch.selftest_passphrase_file = Some(dir.join("passphrase"));
        write_passphrase_file(&dir.join("passphrase"), "selftest pass\n", 0o644);
    });
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "selftest pass" }));
    demo.ok("lock", json!({}));
    let e = demo.err("selftest_unlock", json!({}), "app.selftest_passphrase_file");
    let message = e["message"].as_str().unwrap();
    assert!(message.contains("has mode 644"), "{message}");
    assert_eq!(demo.ok("lock_status", json!({}))["state"], "locked");
}

/// A representative command of each facade, through the IPC handler: the
/// free functions, `Vault`, `NetworkSession`, `TxDraft`, `PreparedTx` and
/// `DashPay`.
#[test]
fn a_command_of_every_facade_over_ipc() {
    let demo = Demo::new();
    // Free functions; they need no session.
    let phrase = demo.ok(
        "generate_mnemonic",
        json!({ "wordCount": 12, "language": "english" }),
    );
    let phrase = phrase.as_str().unwrap().to_owned();
    assert_eq!(phrase.split(' ').count(), 12);
    let check = demo.ok("check_mnemonic", json!({ "phrase": phrase }));
    assert_eq!(check["checksum"], "valid", "{check}");
    assert_eq!(check["language"], "english", "{check}");
    assert!(demo.ok("fee_policy", json!({}))["minRelayPerKb"].is_u64());
    assert_eq!(
        demo.ok("check_username", json!({ "label": "alice" }))["valid"],
        true
    );
    assert_eq!(
        demo.ok(
            "is_coinjoin_internal",
            json!({ "txType": "coinJoinMixing" })
        ),
        true
    );
    let missing = demo.call(
        "read_backup_header",
        json!({ "path": "/nonexistent/dw-e0-13.backup" }),
    );
    assert_app_error(&missing.unwrap_err());

    demo.open();

    // Vault
    let vault = demo.ok("create_vault", json!({ "passphrase": "correct horse" }));
    assert_eq!(vault["encrypted"], true, "{vault}");

    // NetworkSession
    let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
    let wallet = created["walletId"].as_str().unwrap().to_owned();
    assert_eq!(created["mnemonic"].as_str().unwrap().split(' ').count(), 12);
    let wallets = demo.ok("list_wallets", json!({}));
    assert_eq!(wallets.as_array().unwrap().len(), 1, "{wallets}");
    let address = demo.ok("current_receive_address", json!({ "walletId": wallet }));
    assert!(address["address"].is_string(), "{address}");

    // TxDraft, by handle
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet }));
    assert_eq!(
        demo.ok("draft_wallet_id", json!({ "draft": draft })),
        wallet
    );
    demo.ok(
        "draft_set_recipients",
        json!({ "draft": draft, "recipients": [{
            "address": address["address"],
            "amount": 100_000,
            "subtractFeeFromAmount": false,
            "label": null,
            "message": null,
        }] }),
    );
    // An empty wallet cannot fund it: an engine code, not a panic.
    assert_app_error(
        &demo
            .call("draft_estimate", json!({ "draft": draft }))
            .unwrap_err(),
    );
    assert_eq!(demo.ok("close_draft", json!({ "draft": draft })), true);
    demo.err(
        "draft_wallet_id",
        json!({ "draft": draft }),
        "app.unknown_handle",
    );

    // PreparedTx: only `draft_prepare` makes one, which needs funds; an
    // unknown handle is refused by the table.
    demo.err(
        "prepared_summary",
        json!({ "prepared": 1 }),
        "app.unknown_handle",
    );

    // NetworkSession::run_on_engine, through the console.
    let help = demo.ok(
        "console_execute",
        json!({ "walletId": null, "line": "help", "grantId": null }),
    );
    assert_eq!(help["isJson"], false, "{help}");
    demo.err(
        "console_execute",
        json!({ "line": "getbalance(\"" }),
        "console.parse_error",
    );
    let redacted = demo.ok(
        "console_redact",
        json!({ "line": "walletpassphrase \"secret\" 60" }),
    );
    assert!(!redacted.as_str().unwrap().contains("secret"), "{redacted}");

    // DashPay, of the new wallet; local state only (no Platform here).
    assert_eq!(
        demo.ok("identities", json!({ "walletId": wallet })),
        json!([])
    );
    // A wallet created here has no identity; Join waits for sync (BL-153).
    assert_eq!(
        demo.ok("dashpay_status", json!({ "walletId": wallet })),
        json!({ "kind": "noIdentity", "reason": "waitingForSync" })
    );
    // The engine's own refusal crosses with its code and typed data.
    let e = demo.err(
        "dashpay_sync_now",
        json!({ "walletId": wallet }),
        "platform.not_implemented",
    );
    assert_eq!(e["data"]["kind"], "call", "{e}");
    // The contacts read model (DP2-01) answers from memory.
    assert_eq!(
        demo.ok("dashpay_pending_setup_count", json!({ "walletId": wallet })),
        0
    );
    // The quorum provider (E0-10b): before masternode sync, the trusted
    // service. The developer toggle round-trips (a debug build takes it).
    assert_eq!(demo.ok("quorum_source", json!({})), "trusted");
    let compat = demo.ok("protocol_compatibility", json!({}));
    assert_eq!(compat["networkVersion"], serde_json::Value::Null);
    assert!(compat["supportedVersion"].as_u64().unwrap() >= 13);
    assert_eq!(demo.ok("trusted_quorums_only", json!({})), false);
    demo.ok("set_trusted_quorums_only", json!({ "on": true }));
    assert_eq!(demo.ok("trusted_quorums_only", json!({})), true);
    demo.ok("set_trusted_quorums_only", json!({ "on": false }));
}

/// The vault and wallet flows whose arguments are tagged records or
/// several secrets of one type: a swap or a dropped field would compile.
#[test]
fn vault_and_wallet_flows_over_ipc() {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "first" }));
    let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
    let wallet = created["walletId"].as_str().unwrap().to_owned();
    let phrase = created["mnemonic"].as_str().unwrap().to_owned();
    let other_phrase = demo.ok(
        "generate_mnemonic",
        json!({ "wordCount": 12, "language": "english" }),
    );
    let imported = demo.ok(
        "import_wallet",
        json!({
            "mnemonic": other_phrase,
            "bip39Passphrase": "extra",
            "options": { "name": "Imported", "birthHeight": null, "coreCompat": false, "lookahead": null },
        }),
    );
    let imported = imported.as_str().unwrap().to_owned();
    assert_eq!(
        demo.ok("wallet_info", json!({ "walletId": imported }))["name"],
        "Imported"
    );

    // A wrong passphrase carries the attempt count.
    demo.ok("lock", json!({}));
    let e = demo.err(
        "unlock",
        json!({ "passphrase": "wrong" }),
        "vault.wrong_passphrase",
    );
    assert_eq!(e["data"]["kind"], "wrongPassphrase", "{e}");
    assert_eq!(e["data"]["failedAttempts"], 1, "{e}");
    demo.ok("unlock", json!({ "passphrase": "first" }));

    // authorize (tagged purpose and credential) → revealMnemonic.
    let reveal = |wallet: &str, passphrase: &str| {
        let grant = demo.ok(
            "authorize",
            json!({
                "purpose": { "kind": "revealSecret" },
                "walletId": wallet,
                "credential": { "kind": "passphrase", "passphrase": passphrase },
            }),
        );
        assert_eq!(grant["purpose"]["kind"], "revealSecret", "{grant}");
        demo.ok(
            "reveal_mnemonic",
            json!({ "walletId": wallet, "grantId": grant["id"] }),
        )
    };
    let shown = reveal(&wallet, "first");
    assert_eq!(shown["phrase"], phrase.as_str());
    assert_eq!(shown["bip39Passphrase"], "");
    let shown = reveal(&imported, "first");
    assert_eq!(shown["phrase"], other_phrase);
    assert_eq!(shown["bip39Passphrase"], "extra");

    // Forgot passphrase: the recovery phrase sets a new vault passphrase.
    demo.ok("lock", json!({}));
    let recovered = demo.ok(
        "recover_with_mnemonic",
        json!({
            "walletId": wallet,
            "mnemonic": phrase,
            "bip39Passphrase": "",
            "newPassphrase": "second",
        }),
    );
    assert!(
        recovered["status"]["encrypted"].as_bool().unwrap(),
        "{recovered}"
    );
    demo.ok("lock", json!({}));
    demo.err(
        "unlock",
        json!({ "passphrase": "first" }),
        "vault.wrong_passphrase",
    );
    demo.ok("unlock", json!({ "passphrase": "second" }));

    // removeWallet with a wipe grant.
    let wipe = demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "wipe" },
            "walletId": imported,
            "credential": { "kind": "passphrase", "passphrase": "second" },
        }),
    );
    demo.ok(
        "remove_wallet",
        json!({ "walletId": imported, "grantId": wipe["id"] }),
    );
    assert_eq!(
        demo.ok("list_wallets", json!({})).as_array().unwrap().len(),
        1
    );
}

/// "Require authentication for every payment" is enforced in Rust (Sol r1
/// F1): on an encrypted, unlocked vault, `spend`, `platformOp`,
/// `masternodeOp` and `governance` grants (MG-05 D11) need a credential
/// while it is on, and turning it either way needs one.
#[test]
fn payment_authentication_is_enforced_over_ipc() {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let none = json!({ "kind": "unencrypted" });
    let pw = json!({ "kind": "passphrase", "passphrase": "pw" });
    let spend = json!({ "kind": "spend", "maxDuffs": 1000 });
    let platform = json!({ "kind": "platformOp", "maxDuffs": 1000, "maxCredits": "1000" });
    let masternode = json!({ "kind": "masternodeOp", "maxDuffs": 1000 });
    let governance = json!({ "kind": "governance" });
    let change = json!({ "kind": "changeCredential" });
    let authorize = |purpose: &Value, wallet: &Value, credential: &Value| json!({ "purpose": purpose, "walletId": wallet, "credential": credential });
    let refused = |demo: &Demo| {
        for purpose in [&spend, &platform, &masternode, &governance] {
            demo.err(
                "authorize",
                authorize(purpose, &wallet, &none),
                "vault.credential_required",
            );
        }
        demo.err(
            "authorize_set",
            json!({ "purposes": [platform, spend], "walletId": wallet, "credential": none }),
            "vault.credential_required",
        );
    };

    // Default is the signing window create_vault just opened. A payment at
    // or under the spend limit needs no passphrase; one over it does. A
    // masternode owner operation and a vote are not payments: they ask.
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok("authorize", authorize(&spend, &wallet, &none));
    demo.ok("authorize", authorize(&platform, &wallet, &none));
    for purpose in [&masternode, &governance] {
        demo.err(
            "authorize",
            authorize(purpose, &wallet, &none),
            "vault.credential_required",
        );
    }
    demo.ok(
        "authorize_set",
        json!({ "purposes": [platform, spend], "walletId": wallet, "credential": none }),
    );
    let over = json!({ "kind": "spend", "maxDuffs": 50_000_001 });
    demo.err(
        "authorize",
        authorize(&over, &wallet, &none),
        "vault.credential_required",
    );
    // Reveal and a credential change always need the passphrase.
    demo.err(
        "authorize",
        authorize(&json!({ "kind": "revealSecret" }), &wallet, &none),
        "vault.credential_required",
    );
    demo.err(
        "authorize",
        authorize(&change, &Value::Null, &none),
        "vault.credential_required",
    );
    // The passphrase still issues what the window does not.
    demo.ok("authorize", authorize(&over, &wallet, &pw));
    demo.ok("authorize", authorize(&spend, &wallet, &pw));
    demo.ok("authorize", authorize(&platform, &wallet, &pw));
    demo.ok("authorize", authorize(&masternode, &wallet, &pw));
    demo.ok("authorize", authorize(&governance, &wallet, &pw));
    // Sign message keeps the vault's own rule: no passphrase while unlocked.
    demo.ok(
        "authorize",
        authorize(&json!({ "kind": "signMessage" }), &wallet, &none),
    );

    // Sleep ends the window and leaves the vault unlocked.
    {
        use tauri::Manager;
        let session =
            tauri::async_runtime::block_on(demo.app.state::<crate::state::AppState>().session())
                .unwrap();
        session.vault().end_signing_window();
    }
    assert_eq!(demo.ok("lock_status", json!({}))["state"], "unlocked");
    demo.err(
        "authorize",
        authorize(&spend, &wallet, &none),
        "vault.credential_required",
    );
    // A passphrase unlock opens it again.
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    demo.ok("authorize", authorize(&spend, &wallet, &none));

    // Turning "ask every payment" on needs a credential: no changeCredential
    // grant without one, and no other grant stands in for it.
    demo.err(
        "authorize",
        authorize(&change, &Value::Null, &none),
        "vault.credential_required",
    );
    let spend_grant = demo.ok("authorize", authorize(&spend, &wallet, &pw))["id"].clone();
    demo.err(
        "set_payment_authentication",
        json!({ "on": false, "grantId": spend_grant }),
        "vault.grant_purpose_mismatch",
    );
    demo.err(
        "set_payment_authentication",
        json!({ "on": false, "grantId": "made-up" }),
        "vault.grant_invalid",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    let grant = demo.ok("authorize", authorize(&change, &Value::Null, &pw))["id"].clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
    );

    // On: every payment needs the passphrase, including one the window would cover.
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    refused(&demo);

    // Turning it off needs a credential too, and the window applies again.
    demo.err(
        "set_payment_authentication",
        json!({ "on": false, "grantId": "made-up" }),
        "vault.grant_invalid",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    let grant = demo.ok("authorize", authorize(&change, &Value::Null, &pw))["id"].clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": false, "grantId": grant }),
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    // Turning it on ended the window; a passphrase unlock opens one.
    demo.err(
        "authorize",
        authorize(&spend, &wallet, &none),
        "vault.credential_required",
    );
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    demo.ok("authorize", authorize(&spend, &wallet, &none));

    // Locked, the vault answers first, as without the setting.
    demo.ok("lock", json!({}));
    demo.err(
        "authorize",
        authorize(&spend, &wallet, &none),
        "vault.locked",
    );
}

/// A vault that stored no row (the old "ask every time" default) still asks
/// after the window exists. `"0"` was an explicit choice to stop asking, and
/// now means the window. Anything else asks. A vault this build creates
/// writes the window, so only that one starts without asking.
#[test]
fn an_absent_payment_auth_row_keeps_asking_after_upgrade() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let spend = json!({
        "purpose": { "kind": "spend", "maxDuffs": 1000 },
        "walletId": wallet,
        "credential": { "kind": "unencrypted" },
    });
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok("authorize", spend.clone());

    let session =
        tauri::async_runtime::block_on(demo.app.state::<crate::state::AppState>().session())
            .unwrap();
    let write = |value: Option<&str>| {
        tauri::async_runtime::block_on(
            session.set_payment_auth_row_for_test(value.map(str::to_owned)),
        )
        .unwrap();
    };

    write(None);
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    demo.err("authorize", spend.clone(), "vault.credential_required");

    write(Some("0"));
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    demo.ok("authorize", spend.clone());

    write(Some("garbage"));
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    demo.err("authorize", spend.clone(), "vault.credential_required");

    write(Some("1"));
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    write(Some("strict"));
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    write(Some("window"));
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    demo.ok("authorize", spend);
}

/// "Ask every payment" holds in the vault, not only in `authorize`: a
/// caller that asks the vault directly, past the IPC check, gets no
/// credential-free payment, after an unlock and after the network is
/// opened again from the stored choice.
#[test]
fn ask_every_payment_holds_for_a_caller_that_skips_the_ipc_check() {
    use dw_vault::{Credential, GrantPurpose, VaultError};
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"]
        .as_str()
        .unwrap()
        .to_owned();
    let wallet = crate::state::parse_wallet_id(&wallet).unwrap().0;
    let grant = demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "changeCredential" },
            "walletId": null,
            "credential": { "kind": "passphrase", "passphrase": "pw" },
        }),
    )["id"]
        .clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
    );
    let direct = || {
        let session =
            tauri::async_runtime::block_on(demo.app.state::<crate::state::AppState>().session())
                .unwrap();
        let v = session.vault();
        v.unlock(b"pw", dw_vault::UnlockScope::Full).unwrap();
        for purposes in [
            vec![GrantPurpose::Spend { max_duffs: 1000 }],
            vec![
                GrantPurpose::PlatformOp {
                    max_duffs: 0,
                    max_credits: 1000,
                },
                GrantPurpose::IdentityScan,
            ],
        ] {
            assert_eq!(
                v.authorize_set(&purposes, Some(&wallet), Credential::None)
                    .map(drop),
                Err(VaultError::CredentialRequired),
                "{purposes:?}"
            );
        }
        assert!(!v.signing_window_open());
    };
    direct();
    assert_eq!(demo.ok("close_network", json!({})), true);
    demo.open();
    direct();
}

/// logind's PrepareForSleep(true) ends the window and leaves the vault
/// unlocked. false does not. A passphrase unlock is what opens it again.
#[test]
fn prepare_for_sleep_ends_the_signing_window() {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let spend = json!({
        "purpose": { "kind": "spend", "maxDuffs": 1000 },
        "walletId": wallet,
        "credential": { "kind": "unencrypted" },
    });
    demo.ok("authorize", spend.clone());
    crate::shell::sleep::apply_prepare_for_sleep(false, demo.handle());
    demo.ok("authorize", spend.clone());
    crate::shell::sleep::apply_prepare_for_sleep(true, demo.handle());
    assert_eq!(demo.ok("lock_status", json!({}))["state"], "unlocked");
    demo.err("authorize", spend.clone(), "vault.credential_required");
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    demo.ok("authorize", spend);
}

/// "Ask every payment" on: a no-credential payment that passed the check
/// while the vault was locked, then met an unlock before its grant was
/// issued, is refused. The unlock opened the signing window, but the
/// window does not stand in for the setting.
#[test]
fn an_unlock_between_check_and_issue_does_not_bypass_ask_every_payment() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let grant = demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "changeCredential" },
            "walletId": null,
            "credential": { "kind": "passphrase", "passphrase": "pw" },
        }),
    )["id"]
        .clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
    );
    demo.ok("lock", json!({}));
    *demo
        .app
        .state::<crate::state::AppState>()
        .before_payment_issue
        .lock()
        .unwrap() = Some(Box::new(|v: &dw_vault::Vault| {
        v.unlock(b"pw", dw_vault::UnlockScope::Full).unwrap();
    }));
    demo.err(
        "authorize",
        json!({
            "purpose": { "kind": "spend", "maxDuffs": 1000 },
            "walletId": wallet,
            "credential": { "kind": "unencrypted" },
        }),
        "vault.credential_required",
    );
    assert_eq!(demo.ok("lock_status", json!({}))["state"], "unlocked");
}

/// `authorize_platform_send` sizes the grant from the engine's quote: an
/// operation the engine never quoted is refused before the credential is
/// checked or any grant issued.
#[test]
fn a_platform_send_grant_needs_a_quoted_operation() {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    for credential in [
        json!({ "kind": "passphrase", "passphrase": "pw" }),
        json!({ "kind": "passphrase", "passphrase": "wrong" }),
    ] {
        demo.err(
            "authorize_platform_send",
            json!({ "walletId": wallet, "operation": format!("op-{:032x}", 7), "credential": credential }),
            "invalid_argument",
        );
    }
}

/// A vault, a wallet and a quoted send of it (no network): the wallet's id,
/// the session and the operation.
fn quoted_send(
    demo: &Demo,
) -> (
    String,
    dw_engine::WalletId,
    std::sync::Arc<dw_engine::NetworkSession>,
    String,
) {
    use tauri::Manager;
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let id = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"]
        .as_str()
        .unwrap()
        .to_owned();
    let wallet: dw_engine::WalletId = id.parse().unwrap();
    let state = demo.app.state::<crate::state::AppState>();
    let session = tauri::async_runtime::block_on(state.session()).unwrap();
    let operation = session.quote_send_for_test(wallet, 1000);
    (id, wallet, session, operation)
}

fn send_grant_ask(wallet: &str, operation: &str) -> Value {
    json!({ "walletId": wallet, "operation": operation, "credential": { "kind": "passphrase", "passphrase": "pw" } })
}

/// A second `authorize_platform_send` for one operation revokes the grant
/// the first bound: it fails even an ordinary `platformOp` check, and the
/// new one passes.
#[test]
fn a_second_platform_send_grant_revokes_the_first() {
    let demo = Demo::new();
    let (id, wallet, session, operation) = quoted_send(&demo);
    let first = demo.ok("authorize_platform_send", send_grant_ask(&id, &operation))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let second = demo.ok("authorize_platform_send", send_grant_ask(&id, &operation))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let check = |g: &str| {
        session
            .vault()
            .check_grant(g, dw_vault::GrantKind::PlatformOp, Some(&wallet.0))
    };
    assert!(check(&first).is_err(), "the replaced grant is revoked");
    check(&second).expect("the bound grant is live");
}

/// A bind that fails after its grant was issued (the operation sent or
/// evicted in between) revokes that grant.
#[test]
fn a_failed_platform_send_bind_revokes_the_grant_it_issued() {
    use tauri::Manager;
    let demo = Demo::new();
    let (id, wallet, session, operation) = quoted_send(&demo);
    let state = demo.app.state::<crate::state::AppState>();
    let (seam, at_bind) = crate::state::Seam::new(true);
    *state.send_bind_seam.lock().unwrap() = Some(seam);
    let key = demo.app.invoke_key().to_string();
    let ask = send_grant_ask(&id, &operation);
    let (webview, key, state, session, operation) =
        (&demo.webview, &key, &state, &session, &operation);
    let refused = std::thread::scope(move |s| {
        let call = s.spawn(move || invoke(webview, key, "authorize_platform_send", ask));
        at_bind.arrived();
        *state.send_bind_seam.lock().unwrap() = None;
        session.forget_send_for_test(operation);
        at_bind.resume();
        call.join().unwrap()
    });
    assert_eq!(refused.unwrap_err()["code"], "invalid_argument");
    let issued = state
        .send_grant_issued
        .lock()
        .unwrap()
        .clone()
        .expect("a grant was issued");
    let live =
        session
            .vault()
            .check_grant(&issued, dw_vault::GrantKind::PlatformOp, Some(&wallet.0));
    assert!(live.is_err(), "the grant the bind refused is revoked");
}

/// On an unencrypted vault there is no credential to ask for: the setting
/// changes nothing and cannot be changed, so no value stored without a
/// credential carries over to the vault once encrypted. Creating it stores
/// no choice either, so it reads as "ask every payment".
#[test]
fn payment_authentication_needs_an_encrypted_vault() {
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": null }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "spend", "maxDuffs": 1000 },
            "walletId": wallet,
            "credential": { "kind": "unencrypted" },
        }),
    );
    let grant = demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "changeCredential" },
            "walletId": null,
            "credential": { "kind": "unencrypted" },
        }),
    )["id"]
        .clone();
    demo.err(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
        "vault.not_encrypted",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
}

/// Encrypts the unencrypted vault of `demo` with passphrase "pw" and
/// unlocks it.
fn encrypt_and_unlock(demo: &Demo) {
    let grant = demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "changeCredential" },
            "walletId": null,
            "credential": { "kind": "unencrypted" },
        }),
    )["id"]
        .clone();
    demo.ok(
        "encrypt_vault",
        json!({ "newPassphrase": "pw", "grantId": grant }),
    );
    demo.ok("unlock", json!({ "passphrase": "pw" }));
}

/// A no-credential payment of `wallet`, as the renderer asks it.
fn no_credential_spend(wallet: &Value) -> Value {
    json!({
        "purpose": { "kind": "spend", "maxDuffs": 1000 },
        "walletId": wallet,
        "credential": { "kind": "unencrypted" },
    })
}

/// A vault made without a passphrase and encrypted later starts on "ask
/// every payment": the user never chose the window for it. A window an
/// earlier build stored for it is not kept either.
#[test]
fn encrypting_a_vault_made_without_a_passphrase_asks_every_payment() {
    use tauri::Manager;
    for earlier_row in [None, Some("window")] {
        let demo = Demo::new();
        demo.open();
        demo.ok("create_vault", json!({ "passphrase": null }));
        let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
        if let Some(row) = earlier_row {
            let session = tauri::async_runtime::block_on(
                demo.app.state::<crate::state::AppState>().session(),
            )
            .unwrap();
            tauri::async_runtime::block_on(
                session.set_payment_auth_row_for_test(Some(row.to_owned())),
            )
            .unwrap();
        }
        encrypt_and_unlock(&demo);
        assert_eq!(
            demo.ok("payment_authentication", json!({})),
            true,
            "{earlier_row:?}"
        );
        demo.err(
            "authorize",
            no_credential_spend(&wallet),
            "vault.credential_required",
        );
        assert_eq!(
            demo.ok("quick_unlock_policy", json!({}))["signingWindowOpen"],
            false
        );
    }
}

/// A refused encrypt changes no payment choice: an encrypted vault on the
/// window stays on it after `vault.already_encrypted` (whatever the grant),
/// and a vault without a passphrase keeps its stored row after a grant that
/// is not a live change-credential grant.
#[test]
fn a_refused_encrypt_keeps_the_payment_choice() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.err(
        "encrypt_vault",
        json!({ "newPassphrase": "other", "grantId": "no such grant" }),
        "vault.already_encrypted",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), false);

    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": null }));
    let session =
        tauri::async_runtime::block_on(demo.app.state::<crate::state::AppState>().session())
            .unwrap();
    tauri::async_runtime::block_on(session.set_payment_auth_row_for_test(Some("window".into())))
        .unwrap();
    demo.err(
        "encrypt_vault",
        json!({ "newPassphrase": "pw", "grantId": "no such grant" }),
        "vault.grant_invalid",
    );
    let stored = tauri::async_runtime::block_on(session.payment_authentication()).unwrap();
    assert!(!stored.on);
}

/// A recovery with the recovery phrase keeps "ask every payment" and the
/// window as the user had them, for the new vault it makes.
#[test]
fn a_recovery_keeps_the_payment_setting() {
    for on in [true, false] {
        let demo = Demo::new();
        demo.open();
        demo.ok("create_vault", json!({ "passphrase": "pw" }));
        let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
        let wallet = created["walletId"].clone();
        let grant = demo.ok(
            "authorize",
            json!({
                "purpose": { "kind": "changeCredential" },
                "walletId": null,
                "credential": { "kind": "passphrase", "passphrase": "pw" },
            }),
        )["id"]
            .clone();
        demo.ok(
            "set_payment_authentication",
            json!({ "on": on, "grantId": grant }),
        );
        demo.ok("lock", json!({}));
        demo.ok(
            "recover_with_mnemonic",
            json!({
                "walletId": wallet,
                "mnemonic": created["mnemonic"],
                "bip39Passphrase": "",
                "newPassphrase": "second",
            }),
        );
        assert_eq!(demo.ok("payment_authentication", json!({})), on, "{on}");
        if on {
            demo.err(
                "authorize",
                no_credential_spend(&wallet),
                "vault.credential_required",
            );
        } else {
            // A recovery without the old data key cannot read the sealed
            // limit, so the new vault's limit is 0 until it is set again.
            demo.err(
                "authorize",
                no_credential_spend(&wallet),
                "vault.credential_required",
            );
            let grant = demo.ok(
                "authorize",
                json!({
                    "purpose": { "kind": "changeCredential" },
                    "walletId": null,
                    "credential": { "kind": "passphrase", "passphrase": "second" },
                }),
            )["id"]
                .clone();
            demo.ok(
                "set_quick_unlock_spend_limit",
                json!({ "grantId": grant, "spendLimitDuffs": 10_000_000u64 }),
            );
            demo.ok("authorize", no_credential_spend(&wallet));
        }
    }
}

/// The window default is written after the vault exists: a failed write
/// does not fail the creation (a retry would find the vault there), and
/// the vault asks every payment.
#[test]
fn a_failed_default_write_does_not_fail_vault_creation() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    let session =
        tauri::async_runtime::block_on(demo.app.state::<crate::state::AppState>().session())
            .unwrap();
    session.fail_next_payment_auth_write_for_test();
    let status = demo.ok("create_vault", json!({ "passphrase": "pw" }));
    assert_eq!(status["state"], "unlocked", "{status}");
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    demo.err(
        "authorize",
        no_credential_spend(&wallet),
        "vault.credential_required",
    );
    assert!(!session.vault().signing_window_allowed());
}

/// Sol r2 F1: the setting belongs to the vault. "Ask every payment", set
/// with the passphrase, does not carry over to a vault created after
/// `destroyVault`. The new one starts on the signing window.
#[test]
fn payment_authentication_does_not_outlive_its_vault() {
    let demo = Demo::new();
    demo.open();
    let pw = |p: &str| json!({ "kind": "passphrase", "passphrase": p });
    let none = json!({ "kind": "unencrypted" });
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let change = |credential: &Value| {
        demo.ok(
            "authorize",
            json!({ "purpose": { "kind": "changeCredential" }, "walletId": null, "credential": credential }),
        )["id"]
            .clone()
    };
    let grant = change(&pw("pw"));
    demo.ok(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), true);

    // "Delete All": the wallet, then the vault.
    let wipe = demo.ok(
        "authorize",
        json!({ "purpose": { "kind": "wipe" }, "walletId": wallet, "credential": pw("pw") }),
    );
    demo.ok(
        "remove_wallet",
        json!({ "walletId": wallet, "grantId": wipe["id"] }),
    );
    demo.ok("destroy_vault", json!({ "credential": pw("pw") }));

    // A new vault starts on its own default, the window, not on the old
    // vault's choice. (One made without a passphrase and encrypted later
    // asks: `encrypting_a_vault_made_without_a_passphrase_asks_every_payment`.)
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    demo.ok("lock", json!({}));
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok(
        "authorize",
        json!({ "purpose": { "kind": "spend", "maxDuffs": 1000 }, "walletId": wallet, "credential": none }),
    );
}

/// Runs a no-credential spend `authorize` on its own thread up to its test
/// seam (the setting read, the grant not yet issued), runs `meanwhile`,
/// then lets it finish and returns its result. If `meanwhile` panics, the
/// seam's handle drops with it and `authorize` goes on, so the scope can
/// end; if `authorize` fails before the seam, the wait for it times out.
fn authorize_across(demo: &Demo, wallet: &Value, meanwhile: impl FnOnce()) -> Result<Value, Value> {
    use tauri::Manager;
    let state = demo.app.state::<crate::state::AppState>();
    let (seam, at_seam) = crate::state::Seam::new(true);
    *state.payment_seam.lock().unwrap() = Some(seam);
    let key = demo.app.invoke_key().to_string();
    let (webview, key, state) = (&demo.webview, &key, &state);
    std::thread::scope(move |s| {
        let call = s.spawn(move || {
            invoke(
                webview,
                key,
                "authorize",
                json!({
                    "purpose": { "kind": "spend", "maxDuffs": 1000 },
                    "walletId": wallet,
                    "credential": { "kind": "unencrypted" },
                }),
            )
        });
        at_seam.arrived();
        *state.payment_seam.lock().unwrap() = None;
        meanwhile();
        at_seam.resume();
        call.join().unwrap()
    })
}

/// Sol r2 F1b: a switch to "on" that completes while a no-credential
/// payment grant is between reading "off" and being issued refuses it.
#[test]
fn a_policy_change_during_authorize_applies_to_it() {
    let demo = Demo::new();
    demo.open();
    let pw = json!({ "kind": "passphrase", "passphrase": "pw" });
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let change = || {
        demo.ok(
            "authorize",
            json!({ "purpose": { "kind": "changeCredential" }, "walletId": null, "credential": pw }),
        )["id"]
            .clone()
    };
    let grant = change();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": false, "grantId": grant }),
    );
    // Unchanged meanwhile: off, the grant is issued.
    authorize_across(&demo, &wallet, || {}).expect("off: no credential needed");

    let grant = change();
    let refused = authorize_across(&demo, &wallet, || {
        demo.ok(
            "set_payment_authentication",
            json!({ "on": true, "grantId": grant }),
        );
    })
    .expect_err("issued under the old setting");
    assert_eq!(refused["code"], "vault.credential_required", "{refused}");
}

/// "Off", then a `changeCredential` grant for the switch back: the setup of
/// the Sol r3 F1b tests.
fn payment_auth_off(demo: &Demo) -> (Value, Value) {
    let pw = json!({ "kind": "passphrase", "passphrase": "pw" });
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let change = || {
        demo.ok(
            "authorize",
            json!({ "purpose": { "kind": "changeCredential" }, "walletId": null, "credential": pw }),
        )["id"]
            .clone()
    };
    let grant = change();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": false, "grantId": grant }),
    );
    (wallet, change())
}

/// Makes `setPaymentAuthentication` stop at `at` until the returned
/// handle resumes it (or is dropped).
fn pause_payment_set(demo: &Demo, at: crate::state::SetPause) -> crate::state::SeamHandle {
    use tauri::Manager;
    let state = demo.app.state::<crate::state::AppState>();
    let (seam, handle) = crate::state::Seam::new(true);
    *state.payment_set_pause.lock().unwrap() = Some((at, seam));
    handle
}

/// `setPaymentAuthentication` to "on" with `grant`, on its own thread.
fn switch_on<'s>(
    s: &'s std::thread::Scope<'s, '_>,
    webview: &'s WebviewWindow<MockRuntime>,
    key: &'s str,
    grant: &'s Value,
) -> std::thread::ScopedJoinHandle<'s, Result<Value, Value>> {
    s.spawn(move || {
        invoke(
            webview,
            key,
            "set_payment_authentication",
            json!({ "on": true, "grantId": grant }),
        )
    })
}

/// Sol r3 F1b, the reviewer's two-stage probe: a no-credential payment
/// grant read "off", then the switch to "on" is written and readable (the
/// setter stopped right after its write) before the grant is issued. It is
/// refused: the generation moved before the write.
#[test]
fn a_grant_issued_once_the_strict_value_is_readable_is_refused() {
    let demo = Demo::new();
    demo.open();
    let (wallet, grant) = payment_auth_off(&demo);
    let written = pause_payment_set(&demo, crate::state::SetPause::AfterWrite);
    let key = demo.app.invoke_key().to_string();
    let (demo, webview, key, grant) = (&demo, &demo.webview, key.as_str(), &grant);
    // `written` moves into the scope: a failure drops it, which lets the
    // setter go on, before the scope waits for its threads.
    std::thread::scope(move |s| {
        let mut setter = None;
        let result = authorize_across(demo, &wallet, || {
            setter = Some(switch_on(s, webview, key, grant));
            // Written; the setter has not returned.
            written.arrived();
        });
        written.resume();
        setter.unwrap().join().unwrap().expect("the switch");
        let refused = result.expect_err("issued once \"on\" was readable");
        assert_eq!(refused["code"], "vault.credential_required", "{refused}");
    });
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
}

/// Sol r3 F1b: `authorize` reads the setting under the setter's gate. While
/// the setter has advanced the generation but not yet written "on", a
/// no-credential payment `authorize` that reached the gate waits, then
/// reads "on" and is refused; it cannot read "off" with the new generation
/// and keep its grant.
#[test]
fn authorize_waits_for_a_policy_write_in_progress() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    let (wallet, grant) = payment_auth_off(&demo);
    let bumped = pause_payment_set(&demo, crate::state::SetPause::AfterBump);
    let (seam, at_gate) = crate::state::Seam::new(false);
    *demo
        .app
        .state::<crate::state::AppState>()
        .payment_gate_seam
        .lock()
        .unwrap() = Some(seam);
    let key = demo.app.invoke_key().to_string();
    let (webview, key, grant, wallet) = (&demo.webview, key.as_str(), &grant, &wallet);
    std::thread::scope(move |s| {
        let setter = switch_on(s, webview, key, grant);
        bumped.arrived();
        let call = s.spawn(move || {
            invoke(
                webview,
                key,
                "authorize",
                json!({
                    "purpose": { "kind": "spend", "maxDuffs": 1000 },
                    "walletId": wallet,
                    "credential": { "kind": "unencrypted" },
                }),
            )
        });
        // At the gate. Only a wrong outcome depends on the sleep: a correct
        // `authorize` cannot pass the gate while the setter holds it.
        at_gate.arrived();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let early = call.is_finished();
        bumped.resume();
        setter.join().unwrap().expect("the switch");
        let result = call.join().unwrap();
        assert!(!early, "authorize read the setting mid-write: {result:?}");
        let refused = result.expect_err("read \"on\"");
        assert_eq!(refused["code"], "vault.credential_required", "{refused}");
    });
}

/// Sol r2 F1: a no-credential payment grant that read "off" for one vault
/// is refused when, by the time it is issued, another vault replaced it.
#[test]
fn a_vault_replaced_during_authorize_is_judged_by_its_own_setting() {
    let demo = Demo::new();
    demo.open();
    let pw = json!({ "kind": "passphrase", "passphrase": "pw" });
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let grant = demo.ok(
        "authorize",
        json!({ "purpose": { "kind": "changeCredential" }, "walletId": null, "credential": pw }),
    )["id"]
        .clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": false, "grantId": grant }),
    );
    let refused = authorize_across(&demo, &wallet, || {
        let wipe = demo.ok(
            "authorize",
            json!({ "purpose": { "kind": "wipe" }, "walletId": wallet, "credential": pw }),
        );
        demo.ok(
            "remove_wallet",
            json!({ "walletId": wallet, "grantId": wipe["id"] }),
        );
        demo.ok("destroy_vault", json!({ "credential": pw }));
        // Encrypted and unlocked: it would take no credential if "off"
        // still applied.
        demo.ok("create_vault", json!({ "passphrase": "pw" }));
    })
    .expect_err("issued by another vault");
    assert_eq!(refused["code"], "vault.credential_required", "{refused}");
}

/// Sol r2 F3b: `lock` returns its report, and `lastLockReport` returns it
/// again from the app's memory of the `done` event (which the bridge keeps
/// before its queue, so an overflow cannot lose it; `bridge::tests`).
#[test]
fn the_last_lock_report_is_kept_for_its_network() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    assert_eq!(demo.ok("last_lock_report", json!({})), Value::Null);
    let report = demo.ok("lock", json!({}));
    assert_eq!(report["status"]["state"], "locked");
    assert_eq!(report["flows"], json!([]));
    assert_eq!(report["flowsOmitted"], 0);
    assert_eq!(demo.ok("last_lock_report", json!({})), report);

    // A lock report the engine sends, with flows, is what the query reads.
    let state = demo.app.state::<crate::state::AppState>();
    let sink = crate::bridge::TauriSink::new(demo.app.handle().clone(), Arc::clone(&state.memory))
        .unwrap();
    dw_engine::EventSink::emit(
        &sink,
        dw_engine::EngineEvent::LockProgress {
            network: dw_engine::DashNetwork::Regtest,
            phase: dw_engine::platform::LockPhase::Done(dw_engine::platform::LockReport {
                status: tauri::async_runtime::block_on(state.session())
                    .unwrap()
                    .vault()
                    .status(),
                flows: vec![dw_engine::platform::FlowReport {
                    lease: "0123abcd".into(),
                    wallet_id: "ab".repeat(32),
                    flow: dw_engine::platform::FlowKind::TopUp,
                    outcome: dw_engine::platform::FlowOutcome::MaybeSent,
                    artifacts: vec![(
                        "topup/1/funding".into(),
                        dw_engine::platform::FlowOutcome::MaybeSent,
                    )],
                }],
            }),
        },
    );
    let kept = demo.ok("last_lock_report", json!({}));
    assert_eq!(kept["flows"][0]["outcome"], "maybeSent", "{kept}");
    assert_eq!(
        kept["flows"][0]["artifacts"][0]["artifact"],
        "topup/1/funding"
    );
    // A new session of the network starts without one.
    demo.ok("close_network", json!({}));
    demo.open();
    assert_eq!(demo.ok("last_lock_report", json!({})), Value::Null);
}

/// Sol r1 F2: two prepares race for the last prepared handle (63 taken).
/// Exactly one gets past the cap; the other fails with
/// `app.too_many_handles` before its grant is redeemed. Reservations of
/// prepares that fail, of closed drafts and of an ended session are given
/// back.
#[test]
fn the_last_prepared_handle_goes_to_one_prepare() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": null }));
    let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
    let wallet: dw_engine::WalletId = created["walletId"].as_str().unwrap().parse().unwrap();
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet.to_string() }));
    let draft_id = draft.as_u64().unwrap();
    let state = demo.app.state::<crate::state::AppState>();
    let grant = || {
        demo.ok(
            "authorize",
            json!({
                "purpose": { "kind": "spend", "maxDuffs": 1000 },
                "walletId": wallet.to_string(),
                "credential": { "kind": "unencrypted" },
            }),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let grants = [grant(), grant()];
    // 63 handles taken: reservations count against the cap as open
    // handles do (this wallet has no coins to prepare real ones).
    let taken: Vec<_> = (1..crate::state::SendHandles::MAX)
        .map(|_| state.sends.reserve_prepared(draft_id).unwrap())
        .collect();
    // Both prepares wait at the barrier inside their async commands, each
    // on its own runtime worker (Tauri's runtime has one per core).
    *state.sends.reserve_barrier.lock().unwrap() = Some(Arc::new(std::sync::Barrier::new(2)));
    let key = demo.app.invoke_key().to_string();
    let results: Vec<Result<Value, Value>> = std::thread::scope(|s| {
        let calls: Vec<_> = grants
            .iter()
            .map(|g| {
                let (webview, key) = (&demo.webview, &key);
                s.spawn(move || {
                    invoke(
                        webview,
                        key,
                        "draft_prepare",
                        json!({ "draft": draft_id, "grantId": g }),
                    )
                })
            })
            .collect();
        calls.into_iter().map(|c| c.join().unwrap()).collect()
    });
    *state.sends.reserve_barrier.lock().unwrap() = None;
    let full: Vec<bool> = results
        .iter()
        .map(|r| r.as_ref().unwrap_err()["code"] == "app.too_many_handles")
        .collect();
    assert_eq!(full.iter().filter(|f| **f).count(), 1, "{results:?}");
    // The loser's grant was not redeemed: it is still good for a spend.
    let session = tauri::async_runtime::block_on(state.session()).unwrap();
    let loser = &grants[full.iter().position(|f| *f).unwrap()];
    session
        .vault()
        .check_grant(loser, dw_vault::GrantKind::Spend, Some(&wallet.0))
        .expect("the loser's grant is unspent");
    // The winner's prepare failed (no coins): its reservation went back.
    let last = state.sends.reserve_prepared(draft_id).expect("the 64th");
    assert_eq!(
        state
            .sends
            .reserve_prepared(draft_id)
            .err()
            .unwrap()
            .code_str(),
        "app.too_many_handles"
    );
    // Closing the draft gives back its reservations; the session's end too.
    drop(last);
    demo.ok("close_draft", json!({ "draft": draft }));
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet.to_string() }));
    let draft_id = draft.as_u64().unwrap();
    let again: Vec<_> = (0..crate::state::SendHandles::MAX)
        .map(|_| state.sends.reserve_prepared(draft_id).unwrap())
        .collect();
    drop(taken);
    demo.ok("close_network", json!({}));
    demo.open();
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet.to_string() }));
    let fresh = state.sends.reserve_prepared(draft.as_u64().unwrap());
    assert!(fresh.is_ok(), "the ended session's reservations are gone");
    drop(again);
}

/// A draft made from a session that closed meanwhile is not stored.
#[test]
fn handles_belong_to_their_session() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    demo.ok("create_vault", json!({ "passphrase": null }));
    let created = demo.ok("create_wallet", json!({ "wordCount": 12 }));
    let wallet: dw_engine::WalletId = created["walletId"].as_str().unwrap().parse().unwrap();
    let state = demo.app.state::<crate::state::AppState>();
    let (session, epoch) =
        tauri::async_runtime::block_on(state.session_and_epoch()).expect("session");
    let draft = session.new_tx_draft(wallet).expect("draft");
    let handle = demo.ok("new_tx_draft", json!({ "walletId": wallet.to_string() }));
    // Switching the network drops the handles and starts a new epoch.
    demo.ok("close_network", json!({}));
    demo.open();
    let e = state.sends.insert_draft(epoch, draft).unwrap_err();
    assert_eq!(e.code_str(), "network_not_open");
    demo.err(
        "draft_wallet_id",
        json!({ "draft": handle }),
        "app.unknown_handle",
    );
}

/// The `.dwl` files of the link stash under `root`, with their text.
fn stash_files(root: &std::path::Path) -> Vec<(std::path::PathBuf, String)> {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "dwl") {
                let text = std::fs::read_to_string(&path).unwrap();
                found.push((path, text));
            }
        }
    }
    found
}

/// DP5-01's acceptance: links opened before any wallet (or vault) exists
/// are kept, survive a restart, are sealed when the vault is created, and
/// are offered after onboarding: a payment once, an invitation by id only,
/// never its text, until it is dismissed.
#[test]
fn links_opened_before_a_wallet_survive_onboarding() {
    const ADDR: &str = "yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV";
    const SECRET: &str = "cPlantedInvitationSecret1";
    let pay = format!("dash:{ADDR}?amount=0.25&label=Coffee");
    let invite = format!("dashpay://invite?du=alice&pk={SECRET}&assetlocktx=00");
    let regtest = json!({ "network": "regtest" });
    let demo = Demo::launched(|launch, _| launch.links = vec![pay.clone(), invite.clone()]);
    demo.open();
    let routes = demo.ok("take_links", regtest.clone());
    assert_eq!(routes[0]["kind"], "saved", "{routes}");
    assert_eq!(routes[0]["link"]["kind"], "payment");
    assert_eq!(routes[1]["kind"], "invitation", "{routes}");
    let pay_id = routes[0]["link"]["id"].as_str().unwrap().to_owned();
    let invite_id = routes[1]["linkId"].as_str().unwrap().to_owned();
    assert!(!routes.to_string().contains(SECRET) && !routes.to_string().contains(ADDR));
    // The same link again is the same entry.
    let again = demo.ok("route_link", json!({ "network": "regtest", "text": pay }));
    assert_eq!(again["link"]["id"], pay_id);

    // Before a vault exists, a 0600 file holds the text (DASHPAY §2.9).
    let root = demo._dir.path().to_owned();
    let files = stash_files(&root);
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.iter().any(|(_, t)| t.contains(ADDR)));
    #[cfg(unix)]
    for (path, _) in &files {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{path:?}");
    }

    // A restart: a new app on a data root holding only the stash. (A mock
    // app's state, and with it the instance lock, outlives its drop.)
    let first = demo;
    let dir = dw_testutil::private_tempdir();
    for (path, _) in &files {
        let to = dir.path().join(path.strip_prefix(&root).unwrap());
        let mut mkdir = std::fs::DirBuilder::new();
        mkdir.recursive(true);
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut mkdir, 0o700);
        mkdir.create(to.parent().unwrap()).unwrap();
        std::fs::copy(path, &to).unwrap();
    }
    let root = dir.path().to_owned();
    let demo = Demo::launched_in(dir, |_, _| {});
    drop(first);
    demo.open();
    let kept = demo.ok("stashed_links", json!({}));
    assert_eq!(kept.as_array().unwrap().len(), 2, "{kept}");
    demo.ok("create_vault", json!({ "passphrase": "pw" }));
    for (path, text) in stash_files(&root) {
        assert!(
            !text.contains(ADDR) && !text.contains(SECRET),
            "{path:?}: {text}"
        );
    }
    demo.ok("create_wallet", json!({ "wordCount": 12 }));

    // Locked, nothing is handed out and nothing is lost.
    demo.ok("lock", json!({}));
    assert_eq!(demo.ok("stashed_links", json!({})), kept);
    demo.err(
        "open_stashed_link",
        json!({ "linkId": pay_id }),
        "platform.signer_unavailable",
    );
    demo.ok("unlock", json!({ "passphrase": "pw" }));

    let opened = demo.ok("open_stashed_link", json!({ "linkId": pay_id }));
    assert_eq!(opened["kind"], "pay", "{opened}");
    assert_eq!(opened["uri"]["address"], ADDR);
    assert_eq!(opened["uri"]["amount"], 25_000_000);
    // Taken once.
    assert_eq!(
        demo.ok("open_stashed_link", json!({ "linkId": pay_id })),
        Value::Null
    );
    // An invitation is offered by id and stays.
    let invitation = json!({ "kind": "invitation", "linkId": invite_id });
    assert_eq!(
        demo.ok("open_stashed_link", json!({ "linkId": invite_id })),
        invitation
    );
    assert_eq!(
        demo.ok("open_stashed_link", json!({ "linkId": invite_id })),
        invitation
    );
    let left = demo.ok("stashed_links", json!({}));
    assert_eq!(left.as_array().unwrap().len(), 1, "{left}");
    assert_eq!(left[0]["kind"], "invitation");
    // Locked: an invitation is still offered by id (nothing is opened),
    // and Remove still works (review r1 #6, #7).
    demo.ok("lock", json!({}));
    assert_eq!(
        demo.ok("open_stashed_link", json!({ "linkId": invite_id })),
        invitation
    );
    demo.ok("dismiss_stashed_link", json!({ "linkId": invite_id }));
    assert_eq!(demo.ok("stashed_links", json!({})), json!([]));
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    assert!(stash_files(&root).is_empty());
    demo.err(
        "open_stashed_link",
        json!({ "linkId": "nope" }),
        "invalid_argument",
    );
    // With a wallet, a payment link routes at once.
    let routed = demo.ok("route_link", json!({ "network": "regtest", "text": pay }));
    assert_eq!(routed["kind"], "pay", "{routed}");
}

/// U-01: links among the launch arguments wait in the shell until the
/// renderer takes them, routed for its network; `route_link` routes typed
/// text the same way; `shell_sync` reaches the native menus.
#[test]
fn launch_links_wait_for_take_links() {
    const ADDR: &str = "yMsgnH1xKGa85n4bq2imrZbG2KgrmGttAV";
    let demo = Demo::launched(|launch, _| {
        launch.links = vec![
            format!("dash:{ADDR}?amount=0.5&label=Launch"),
            "dashpay://invite?du=alice&pk=secret".into(),
        ];
    });
    demo.err(
        "take_links",
        json!({ "network": "moon" }),
        "invalid_argument",
    );
    let routes = demo.ok("take_links", json!({ "network": "regtest" }));
    assert_eq!(
        routes,
        json!([
            {
                "kind": "pay",
                "uri": { "address": ADDR, "amount": 50_000_000, "label": "Launch", "message": null },
                "username": null,
            },
        ])
    );
    // No network is open, so there is no stash to keep the invitation in:
    // it never reaches the renderer bare (DP5-01 D3) and waits in the queue
    // until a take has the network's session (review r1 #8).
    assert_eq!(
        demo.ok("take_links", json!({ "network": "regtest" })),
        json!([])
    );
    demo.open();
    let routes = demo.ok("take_links", json!({ "network": "regtest" }));
    assert_eq!(routes[0]["kind"], "invitation", "{routes}");
    assert_eq!(routes[0]["linkId"].as_str().map(str::len), Some(32));
    assert_eq!(routes.as_array().map(Vec::len), Some(1));
    assert_eq!(
        demo.ok("take_links", json!({ "network": "regtest" })),
        json!([])
    );
    assert_eq!(
        demo.ok(
            "route_link",
            json!({ "network": "regtest", "text": "dashwallet://scanqr" })
        ),
        json!({ "kind": "scanQr" })
    );
    let long = format!("dash:{ADDR}?label={}", "x".repeat(5000));
    assert_eq!(
        demo.ok("route_link", json!({ "network": "regtest", "text": long }))["reason"]["kind"],
        "tooLong"
    );
    use tauri::Manager;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    demo.app
        .state::<crate::state::AppState>()
        .shell
        .on_sync(Box::new(move |s| sink.lock().unwrap().push(s)));
    demo.ok(
        "shell_sync",
        json!({ "sync": { "discreet": true, "canLock": false, "secretShown": true } }),
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [crate::shell::ShellSync {
            discreet: true,
            can_lock: false,
            secret_shown: true,
        }]
    );
}

/// U-02 D27: `shell_sync` answers only once the content protection is in
/// effect, and refuses with `app.screen_protection` when it could not be
/// applied: the renderer shows no secret before, or without, it.
#[test]
fn shell_sync_answers_once_the_protection_is_in_effect() {
    use tauri::Manager;
    let demo = Demo::new();
    let shell = Arc::clone(&demo.app.state::<crate::state::AppState>().shell);
    let applied = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&applied);
    shell.on_protect(Box::new(move |on| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sink = Arc::clone(&sink);
        // The platform takes its time: the answer still waits for it.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            sink.lock().unwrap().push(on);
            let _ = tx.send(Ok(()));
        });
        rx
    }));
    let sync = |secret_shown: bool| json!({ "sync": { "discreet": false, "canLock": false, "secretShown": secret_shown } });
    demo.ok("shell_sync", sync(true));
    assert_eq!(*applied.lock().unwrap(), [true]);
    demo.ok("shell_sync", sync(false));
    assert_eq!(*applied.lock().unwrap(), [true, false]);

    // The main thread never ran the change, or the platform refused it.
    shell.on_protect(Box::new(|_| tokio::sync::oneshot::channel().1));
    demo.err("shell_sync", sync(true), "app.screen_protection");
    shell.on_protect(Box::new(|_| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(Err("refused".to_owned()));
        rx
    }));
    demo.err("shell_sync", sync(true), "app.screen_protection");
}

/// U-05 D2: files come from the user's choice in the OS dialog, which the
/// shell shows: the commands read and write only the path it answered with,
/// and `null` is a cancel.
#[test]
fn files_go_through_the_dialog_the_user_answers() {
    use crate::shell::files::{FilePurpose, FileRequest};
    use tauri::Manager;
    let demo = Demo::new();
    let shell = Arc::clone(&demo.app.state::<crate::state::AppState>().shell);
    // No dialog at all: an error, never a silent cancel.
    demo.err("open_file", json!({ "purpose": "psbt" }), "app.file_dialog");

    let dir = dw_testutil::private_tempdir();
    let answer = Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let asked = Arc::new(std::sync::Mutex::new(Vec::<FileRequest>::new()));
    let (a, q) = (Arc::clone(&answer), Arc::clone(&asked));
    shell.on_choose(Box::new(move |request| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        q.lock().unwrap().push(request);
        let _ = tx.send(a.lock().unwrap().clone());
        rx
    }));
    let choose = |path: Option<&std::path::Path>| *answer.lock().unwrap() = path.map(Into::into);

    // A shared-masternode file never reaches the renderer: refused before any dialog.
    demo.err(
        "open_file",
        json!({ "purpose": "sharedMasternodeMessage" }),
        "invalid_argument",
    );
    assert!(asked.lock().unwrap().is_empty());
    // Cancelled.
    assert_eq!(
        demo.ok("open_file", json!({ "purpose": "psbt" })),
        Value::Null
    );
    let psbt = dir.path().join("in.psbt");
    std::fs::write(&psbt, b"psbt\xffxyz").unwrap();
    choose(Some(&psbt));
    assert_eq!(
        demo.ok("open_file", json!({ "purpose": "psbt" })),
        json!({ "name": "in.psbt", "data": "cHNidP94eXo=" })
    );

    let out = dir.path().join("out.csv");
    choose(Some(&out));
    let saved = demo.ok(
        "save_file_as",
        json!({ "purpose": "csv", "suggestedName": "Addresses.csv", "data": "YSxi" }),
    );
    assert_eq!(saved, json!(out.to_str().unwrap()));
    assert_eq!(std::fs::read(&out).unwrap(), b"a,b");
    demo.err(
        "save_file_as",
        json!({ "purpose": "csv", "suggestedName": "x.csv", "data": "not base64!" }),
        "invalid_argument",
    );
    assert_eq!(
        demo.ok(
            "choose_file_path",
            json!({ "purpose": "walletBackup", "save": true, "suggestedName": "Everyday.dwbackup" })
        ),
        json!(out.to_str().unwrap())
    );
    // Activity's export: its own purpose, and a typed extension the dialog doubled is collapsed.
    let doubled = dir.path().join("history.csv.csv");
    choose(Some(&doubled));
    let saved = demo.ok(
        "save_file_as",
        json!({ "purpose": "historyCsv", "suggestedName": "Dash Wallet history 2026-11-03.csv", "data": "YSxi" }),
    );
    let collapsed = dir.path().join("history.csv");
    assert_eq!(saved, json!(collapsed.to_str().unwrap()));
    assert_eq!(std::fs::read(&collapsed).unwrap(), b"a,b");
    assert!(!doubled.exists());
    // The dialog asked nothing about `history.csv.csv`, which was not there: collapsing onto the
    // `history.csv` that is there would replace it without the OS's question, so the path stays.
    choose(Some(&doubled));
    let saved = demo.ok(
        "save_file_as",
        json!({ "purpose": "historyCsv", "suggestedName": "Dash Wallet history 2026-11-03.csv", "data": "eCx5" }),
    );
    assert_eq!(saved, json!(doubled.to_str().unwrap()));
    assert_eq!(std::fs::read(&collapsed).unwrap(), b"a,b");
    assert_eq!(std::fs::read(&doubled).unwrap(), b"x,y");
    // The same through the dialog-alone command the engine's writers use.
    let backup = dir.path().join("a.dwbackup");
    std::fs::write(&backup, b"keep").unwrap();
    let backup_doubled = dir.path().join("a.dwbackup.dwbackup");
    choose(Some(&backup_doubled));
    assert_eq!(
        demo.ok(
            "choose_file_path",
            json!({ "purpose": "walletBackup", "save": true, "suggestedName": "a.dwbackup" })
        ),
        json!(backup_doubled.to_str().unwrap())
    );
    assert_eq!(std::fs::read(&backup).unwrap(), b"keep");
    let asked = asked.lock().unwrap();
    let seen: Vec<_> = asked
        .iter()
        .map(|r| (r.purpose, r.save, r.suggested_name.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            (FilePurpose::Psbt, false, None),
            (FilePurpose::Psbt, false, None),
            (FilePurpose::Csv, true, Some("Addresses.csv")),
            (FilePurpose::WalletBackup, true, Some("Everyday.dwbackup")),
            (
                FilePurpose::HistoryCsv,
                true,
                Some("Dash Wallet history 2026-11-03.csv")
            ),
            (
                FilePurpose::HistoryCsv,
                true,
                Some("Dash Wallet history 2026-11-03.csv")
            ),
            (FilePurpose::WalletBackup, true, Some("a.dwbackup")),
        ],
        "a bad payload is refused before any dialog"
    );
}

/// `--selftest-link-trace`: the shell records each launch link's scheme
/// and the renderer each route, never the link's text.
#[cfg(feature = "selftest")]
#[test]
fn the_link_trace_records_schemes_and_routes() {
    let demo = Demo::launched(|launch, dir| {
        launch.selftest_link_trace = Some(dir.join("trace"));
        launch.links = vec!["dashpay://user?id=secretish&username=bob".into()];
    });
    assert_eq!(demo.ok("launch_info", json!({}))["selftestLinkTrace"], true);
    demo.ok(
        "selftest_link_routed",
        json!({ "route": "rejected notALink" }),
    );
    let trace = std::fs::read_to_string(demo._dir.path().join("trace")).unwrap();
    assert_eq!(trace, "received launch dashpay\nrouted rejected notALink\n");
    assert!(!trace.contains("secretish"));
}

#[cfg(feature = "selftest")]
#[test]
fn the_link_trace_command_refuses_without_the_option() {
    Demo::new().err(
        "selftest_link_routed",
        json!({ "route": "pay" }),
        "app.not_selftest",
    );
}

/// E1 r2: a close the engine refused leaves its session open, and the app
/// keeps it active with its handles, whichever the refusal
/// (`shielded_spend_unresolved`, or `wallet` while a wallet is still
/// opening); a close that ended the session drops both.
#[test]
fn a_refused_close_keeps_the_active_session() {
    use tauri::Manager;
    let demo = Demo::new();
    demo.open();
    let state = demo.app.state::<crate::state::AppState>();
    let session = tauri::async_runtime::block_on(state.session()).unwrap();
    let refusals = || {
        [
            dw_engine::EngineError::ShieldedSpendUnresolved,
            dw_engine::EngineError::Wallet(
                "a wallet is still opening; close the network once it is open".into(),
            ),
        ]
    };
    for (refusal, same) in refusals().into_iter().zip(refusals()) {
        let code = crate::error::AppError::from(same).code;
        let mut slot = Some(Arc::clone(&session));
        let closed = crate::api::network::close_active(&state, &mut slot, Err(refusal));
        assert_eq!(closed.unwrap_err().code, code);
        assert!(slot.is_some(), "the session stays active");
    }
    assert!(tauri::async_runtime::block_on(state.session()).is_ok());

    // Closed in the engine: a failure then still drops it.
    assert!(
        tauri::async_runtime::block_on(
            state
                .engine
                .ready()
                .unwrap()
                .close_network(DashNetwork::Regtest)
        )
        .unwrap()
    );
    assert!(!session.is_open());
    let mut slot = Some(session);
    let closed = crate::api::network::close_active(
        &state,
        &mut slot,
        Err(dw_engine::EngineError::Internal(
            "the close task ended".into(),
        )),
    );
    assert!(closed.is_err());
    assert!(slot.is_none());
}

/// A launch on mainnet with no network flag (so not pinned) and no SPV. The
/// windows' sessions open with unreachable local endpoints (a test seam): a
/// test reaches no public DAPI or quorum service.
fn launched_on_mainnet() -> Demo {
    launched_on_mainnet_with_sync(Some(false))
}

fn launched_on_mainnet_with_sync(sync: Option<bool>) -> Demo {
    let demo = Demo::launched(|launch, _| {
        launch.network = DashNetwork::Mainnet;
        launch.network_given = false;
        launch.sync = sync;
    });
    *demo.state().test_endpoints.lock().unwrap() = Some(crate::api::network::SessionSettings {
        dapi_addresses: vec!["https://127.0.0.1:1".into()],
        quorum_url: Some("https://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    });
    demo
}

impl Demo {
    pub(crate) fn state(&self) -> tauri::State<'_, crate::state::AppState> {
        use tauri::Manager;
        self.app.state::<crate::state::AppState>()
    }

    fn data_root(&self) -> std::path::PathBuf {
        self.state().data_root.clone()
    }

    fn active(&self) -> Value {
        self.ok("active_network", json!({}))["dirName"].clone()
    }
}

/// Row 74: choosing the other network switches the running window, with no
/// restart; the renderer's next boot (`launch_info`) and `open_network(null)`
/// follow it, and the choice is saved for the next start.
#[test]
fn a_network_choice_switches_the_running_window() {
    let demo = launched_on_mainnet();
    assert_eq!(
        demo.ok("open_network", json!({}))["network"]["dirName"],
        "mainnet"
    );
    demo.ok("create_vault", json!({ "passphrase": null }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet }));
    let switched = demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(
        switched["opened"]["network"]["dirName"], "testnet",
        "{switched}"
    );
    assert_eq!(switched["choice"]["chosen"], "testnet");
    assert_eq!(switched["choice"]["saved"], true);
    assert_eq!(switched["choice"]["active"]["dirName"], "testnet");
    assert_eq!(switched["choice"]["forcedByFlag"], false);
    assert_eq!(demo.active(), "testnet");
    // Nothing of mainnet is reachable from the testnet window.
    demo.err(
        "draft_wallet_id",
        json!({ "draft": draft }),
        "app.unknown_handle",
    );
    assert_eq!(
        demo.ok("list_wallets", json!({})).as_array().unwrap().len(),
        0
    );
    assert_eq!(
        demo.ok("launch_info", json!({}))["network"]["dirName"],
        "testnet"
    );
    assert_eq!(
        demo.ok("open_network", json!({}))["network"]["dirName"],
        "testnet"
    );
    assert_eq!(
        demo.ok("get_network_choice", json!({}))["chosen"],
        "testnet"
    );
    // Already the window's: kept as it is.
    let again = demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(again["opened"]["network"]["dirName"], "testnet");
    // And back, in the same process: its wallet is there.
    demo.ok("switch_network", json!({ "network": "mainnet" }));
    assert_eq!(demo.active(), "mainnet");
    assert_eq!(
        demo.ok("list_wallets", json!({})).as_array().unwrap().len(),
        1
    );
    assert_eq!(
        demo.ok("get_network_choice", json!({}))["chosen"],
        "mainnet"
    );
}

/// Row 74: the launch's trust flags (`--spv-peer`, `--dapi`, `--quorum-url`,
/// `--ca-cert`, …) belong to the network they were given for, so without
/// `--network` they pin the launch network: a choice is only saved.
#[test]
fn trust_flags_pin_the_launch_network() {
    let demo = Demo::launched(|launch, _| {
        launch.network = DashNetwork::Testnet;
        launch.network_given = false;
        launch.sync = Some(false);
        launch.no_platform = true;
        launch.spv_peers = vec!["127.0.0.1:1".into()];
        launch.dapi_addresses = vec!["https://127.0.0.1:1".into()];
        launch.quorum_url = Some("https://127.0.0.1:1".into());
    });
    assert_eq!(
        demo.ok("get_network_choice", json!({}))["forcedByFlag"],
        true
    );
    demo.ok("open_network", json!({}));
    let saved = demo.ok("switch_network", json!({ "network": "mainnet" }));
    assert_eq!(saved["opened"], Value::Null, "{saved}");
    assert_eq!(saved["choice"]["chosen"], "mainnet");
    assert_eq!(saved["choice"]["active"]["dirName"], "testnet");
    assert_eq!(demo.active(), "testnet");
}

/// Row 74: a network that does not open leaves the window on the one it
/// left, and the choice unsaved.
#[test]
fn a_failed_switch_rolls_back_and_saves_nothing() {
    let demo = launched_on_mainnet();
    demo.ok("open_network", json!({}));
    // A file where the network's directory goes: its open fails.
    std::fs::write(demo.data_root().join("testnet"), b"").unwrap();
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_switch_failed",
    );
    assert_eq!(e["data"]["kind"], "networkSwitch", "{e}");
    assert_eq!(e["data"]["network"]["dirName"], "testnet");
    assert_eq!(e["data"]["rolledBack"], true);
    assert_eq!(demo.active(), "mainnet");
    assert_eq!(
        demo.ok("launch_info", json!({}))["network"]["dirName"],
        "mainnet"
    );
    let choice = demo.ok("get_network_choice", json!({}));
    assert_eq!(choice["chosen"], "mainnet", "{choice}");
    assert_eq!(choice["saved"], false);
}

/// Row 74: when the network left does not reopen either, the window has
/// none; it stays the window's, so the splash's Retry (`open_network(null)`)
/// opens it once it can.
#[test]
fn a_failed_rollback_leaves_the_old_network_to_retry() {
    let demo = launched_on_mainnet();
    demo.ok("open_network", json!({}));
    let root = demo.data_root();
    std::fs::write(root.join("testnet"), b"").unwrap();
    let away = root.join("mainnet.away");
    {
        let (root, away) = (root.clone(), away.clone());
        *demo.state().before_rollback.lock().unwrap() = Some(Box::new(move || {
            std::fs::rename(root.join("mainnet"), &away).unwrap();
            std::fs::write(root.join("mainnet"), b"").unwrap();
        }));
    }
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_switch_failed",
    );
    assert_eq!(e["data"]["rolledBack"], false, "{e}");
    assert_eq!(demo.ok("active_network", json!({})), Value::Null);
    assert_eq!(
        demo.ok("launch_info", json!({}))["network"]["dirName"],
        "mainnet"
    );
    assert_eq!(demo.ok("get_network_choice", json!({}))["saved"], false);
    // Retry, once the old network can open again.
    *demo.state().before_rollback.lock().unwrap() = None;
    std::fs::remove_file(root.join("mainnet")).unwrap();
    std::fs::rename(&away, root.join("mainnet")).unwrap();
    assert_eq!(
        demo.ok("open_network", json!({}))["network"]["dirName"],
        "mainnet"
    );
}

/// Row 74: closing the network would cut what runs on it, so the switch
/// refuses and changes nothing (here a broadcast that has not returned).
#[test]
fn a_switch_refuses_while_a_broadcast_has_not_returned() {
    let demo = launched_on_mainnet();
    demo.ok("open_network", json!({}));
    let state = demo.state();
    let in_flight = state.broadcasting();
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_busy",
    );
    assert_eq!(e["data"]["kind"], "networkBusy", "{e}");
    assert_eq!(e["data"]["broadcast"], true);
    assert_eq!(e["data"]["mixing"], false);
    assert_eq!(e["data"]["flows"], json!([]));
    assert_eq!(demo.active(), "mainnet");
    assert_eq!(demo.ok("get_network_choice", json!({}))["saved"], false);
    drop(in_flight);
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(demo.active(), "testnet");
}

/// A mainnet window with an unencrypted vault, one wallet and a draft.
fn mainnet_wallet_and_draft(demo: &Demo) -> (Value, Value) {
    demo.ok("open_network", json!({}));
    demo.ok("create_vault", json!({ "passphrase": null }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    let draft = demo.ok("new_tx_draft", json!({ "walletId": wallet }));
    (wallet, draft)
}

fn platform_grant(demo: &Demo, wallet: &Value) -> Value {
    let purpose = json!({ "kind": "platformOp", "maxDuffs": 1000, "maxCredits": "1000" });
    let credential = json!({ "kind": "unencrypted" });
    demo.ok(
        "authorize",
        json!({ "purpose": purpose, "walletId": wallet, "credential": credential }),
    )["id"]
        .clone()
}

/// After a refused switch: the window, its session, its drafts and the
/// choice are as they were, and the vault was not locked (a grant issued
/// before still opens a flow).
fn nothing_changed(demo: &Demo, wallet: &Value, draft: &Value, grant: &Value) {
    assert_eq!(demo.active(), "mainnet");
    assert_eq!(
        demo.ok("launch_info", json!({}))["network"]["dirName"],
        "mainnet"
    );
    let choice = demo.ok("get_network_choice", json!({}));
    assert_eq!(
        (&choice["chosen"], &choice["saved"]),
        (&json!("mainnet"), &json!(false))
    );
    assert_eq!(
        &demo.ok("draft_wallet_id", json!({ "draft": draft })),
        wallet
    );
    assert_eq!(
        demo.ok("list_wallets", json!({})).as_array().unwrap().len(),
        1
    );
    let lease = demo.ok(
        "begin_flow",
        json!({ "walletId": wallet, "flow": "contactRequest", "grants": [grant] }),
    );
    demo.ok("end_flow", json!({ "lease": lease }));
}

/// Row 74 r1: the engine's own refusals of the close (a wallet still
/// opening, a shielded spend not resolved) refuse the switch, named, and
/// change nothing.
#[test]
fn a_close_that_refuses_itself_refuses_the_switch_and_changes_nothing() {
    use dw_engine::test_support::{CloseFence, hold_close_fence};
    let demo = launched_on_mainnet();
    let (wallet, draft) = mainnet_wallet_and_draft(&demo);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    for (fence, code, reason) in [
        (CloseFence::WalletLoad, "wallet", "walletOpening"),
        (
            CloseFence::ShieldedSpend,
            "shielded_spend_unresolved",
            "shieldedSpendUnresolved",
        ),
    ] {
        let grant = platform_grant(&demo, &wallet);
        let held = hold_close_fence(&session, fence, std::time::Duration::from_millis(100));
        let e = demo.err("switch_network", json!({ "network": "testnet" }), code);
        assert_eq!(e["data"]["kind"], "networkCloseRefused", "{e}");
        assert_eq!(e["data"]["reason"], reason);
        drop(held);
        nothing_changed(&demo, &wallet, &draft, &grant);
    }
    drop(session);
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(demo.active(), "testnet");
}

/// Row 74 r1: an operation still running once the switch's drain ends
/// refuses it as `draining`, and nothing changes.
#[test]
fn a_switch_refuses_while_an_operation_does_not_finish() {
    use dw_engine::test_support::{CloseFence, hold_close_fence};
    let demo = launched_on_mainnet();
    let (wallet, draft) = mainnet_wallet_and_draft(&demo);
    let grant = platform_grant(&demo, &wallet);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let held = hold_close_fence(
        &session,
        CloseFence::Operation,
        std::time::Duration::from_secs(30),
    );
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_busy",
    );
    assert_eq!(e["data"]["draining"], true, "{e}");
    assert_eq!(e["data"]["flows"], json!([]));
    assert_eq!(e["data"]["mixing"], false);
    assert_eq!(e["data"]["broadcast"], false);
    drop(held);
    nothing_changed(&demo, &wallet, &draft, &grant);
}

/// Row 74 r2 (test gap): a Platform send counts as a broadcast for as long
/// as its engine call runs. Held before the engine admits it, it refuses a
/// switch asked meanwhile.
#[test]
fn a_switch_refuses_while_a_platform_send_runs() {
    let demo = launched_on_mainnet();
    let (wallet, draft) = mainnet_wallet_and_draft(&demo);
    let grant = platform_grant(&demo, &wallet);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let (held, release) = dw_engine::test_support::hold_platform_send(&session);
    drop(session);
    let key = demo.app.invoke_key().to_string();
    let args = json!({ "walletId": wallet, "operation": "op", "grant": "g" });
    let (webview, key) = (&demo.webview, &key);
    let e = std::thread::scope(|s| {
        let sending = s.spawn(move || invoke(webview, key, "platform_send", args));
        held.recv_timeout(std::time::Duration::from_secs(30))
            .expect("the send reached the engine");
        let e = demo.err(
            "switch_network",
            json!({ "network": "testnet" }),
            "app.network_busy",
        );
        drop(release);
        let _ = sending.join().unwrap();
        e
    });
    assert_eq!(e["data"]["broadcast"], true, "{e}");
    assert_eq!(e["data"]["draining"], false);
    nothing_changed(&demo, &wallet, &draft, &grant);
}

/// Row 74 r2: a take of the links for the network the window left, which a
/// switch went ahead of, takes none; the new network's take gets them.
#[test]
fn a_take_for_the_network_a_switch_left_leaves_the_links_queued() {
    let demo = launched_on_mainnet();
    demo.ok("open_network", json!({}));
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert!(
        demo.state()
            .shell
            .inbox
            .deliver("dashwallet://scanqr".into())
    );
    assert_eq!(
        demo.ok("take_links", json!({ "network": "mainnet" })),
        json!([])
    );
    assert_eq!(
        demo.ok("take_links", json!({ "network": "testnet" })),
        json!([{ "kind": "scanQr" }])
    );
}

/// Row 74 r1: a Platform flow holding a lease refuses the switch; locking
/// the wallet ends its lease, and the switch goes through.
#[test]
fn a_switch_refuses_while_a_flow_holds_a_lease() {
    let demo = launched_on_mainnet();
    let (wallet, draft) = mainnet_wallet_and_draft(&demo);
    let grant = platform_grant(&demo, &wallet);
    demo.ok(
        "begin_flow",
        json!({ "walletId": wallet, "flow": "contactRequest", "grants": [grant] }),
    );
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_busy",
    );
    assert_eq!(e["data"]["flows"], json!(["contactRequest"]), "{e}");
    assert_eq!(e["data"]["stalled"], false);
    assert_eq!(e["data"]["mixing"], false);
    assert_eq!(e["data"]["broadcast"], false);
    assert_eq!(e["data"]["draining"], false);
    nothing_changed(&demo, &wallet, &draft, &platform_grant(&demo, &wallet));
    demo.ok("lock", json!({}));
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(demo.active(), "testnet");
}

/// A reset of chain data is refused, as a network switch is, while a wallet
/// mixes and while a mixing session is still open after mixing stopped. It
/// goes ahead once mixing has stopped and its sessions have closed.
#[test]
fn a_reset_of_chain_data_refuses_while_mixing_a_session_is_open_or_inputs_are_held() {
    let demo = launched_on_mainnet();
    let (wallet, _) = mainnet_wallet_and_draft(&demo);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let id = crate::state::parse_wallet_id(wallet.as_str().unwrap()).unwrap();
    demo.ok("stop_spv", json!({}));

    dw_engine::test_support::set_mixing(&session, id, true);
    let e = demo.err("reset_chain_data", json!({}), "app.network_busy");
    assert_eq!(e["data"]["mixing"], true, "{e}");

    dw_engine::test_support::set_mixing(&session, id, false);
    dw_engine::test_support::set_open_mixing_session(&session, id, true);
    let e = demo.err("reset_chain_data", json!({}), "app.network_busy");
    assert_eq!(e["data"]["mixing"], true, "{e}");

    dw_engine::test_support::set_open_mixing_session(&session, id, false);
    dw_engine::test_support::hold_signed_round_input(&session, id);
    assert!(!session.mixing_busy());
    demo.err("reset_chain_data", json!({}), "coinjoin.inputs_held");
}

#[test]
fn a_reset_of_chain_data_runs_once_mixing_stopped() {
    let demo = launched_on_mainnet();
    let (wallet, _) = mainnet_wallet_and_draft(&demo);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let id = crate::state::parse_wallet_id(wallet.as_str().unwrap()).unwrap();
    demo.ok("stop_spv", json!({}));
    dw_engine::test_support::set_mixing(&session, id, true);
    demo.err("reset_chain_data", json!({}), "app.network_busy");
    demo.ok("stop_mixing", json!({ "walletId": wallet }));
    demo.ok("reset_chain_data", json!({}));
}

/// Row 74 r1: a wallet mixing refuses the switch.
#[test]
fn a_switch_refuses_while_a_wallet_mixes() {
    let demo = launched_on_mainnet();
    let (wallet, draft) = mainnet_wallet_and_draft(&demo);
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let id = crate::state::parse_wallet_id(wallet.as_str().unwrap()).unwrap();
    dw_engine::test_support::set_mixing(&session, id, true);
    let e = demo.err(
        "switch_network",
        json!({ "network": "testnet" }),
        "app.network_busy",
    );
    assert_eq!(e["data"]["mixing"], true, "{e}");
    assert_eq!(e["data"]["flows"], json!([]));
    nothing_changed(&demo, &wallet, &draft, &platform_grant(&demo, &wallet));
    dw_engine::test_support::set_mixing(&session, id, false);
    drop(session);
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(demo.active(), "testnet");
}

/// Row 74 r1: with sync on, the switch starts SPV on the network it opens,
/// not only on the launch network.
#[test]
fn a_switch_starts_spv_on_the_network_it_opens() {
    let demo = launched_on_mainnet_with_sync(Some(true));
    let started = || demo.state().spv_started.lock().unwrap().clone();
    demo.ok("open_network", json!({}));
    assert_eq!(started(), [DashNetwork::Mainnet]);
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(started(), [DashNetwork::Mainnet, DashNetwork::Testnet]);
}

/// Row 74 r1: `recentNotices` reads the window network's notices and
/// those of none; a network the window left keeps its notices out.
#[test]
fn recent_notices_are_the_window_networks() {
    use dw_engine::{EngineEvent, NoticeCode};
    let demo = launched_on_mainnet();
    demo.ok("open_network", json!({}));
    let state = demo.state();
    for (network, detail) in [
        (Some(DashNetwork::Mainnet), "main"),
        (Some(DashNetwork::Testnet), "test"),
        (None, "none"),
    ] {
        let notice = EngineEvent::Notice {
            network,
            code: NoticeCode::SpvError,
            detail: detail.into(),
        };
        state.memory.record(&crate::bridge::AppEvent::from(notice));
    }
    let details = |demo: &Demo| -> Vec<Value> {
        let notices = demo.ok("recent_notices", json!({}));
        notices
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["detail"].clone())
            .collect()
    };
    assert_eq!(details(&demo), [json!("main"), json!("none")]);
    demo.ok("switch_network", json!({ "network": "testnet" }));
    assert_eq!(details(&demo), [json!("test"), json!("none")]);
}

/// Row 74 r1: every command that hands a transaction or a Platform
/// transition to the network counts as a broadcast in flight while it runs
/// (here each fails in the engine, after its count). `draft_broadcast`
/// counts once its prepared transaction is found, which needs funds.
#[test]
fn every_broadcast_command_counts_while_it_runs() {
    let demo = launched_on_mainnet();
    let (wallet, _) = mainnet_wallet_and_draft(&demo);
    let txid = "11".repeat(32);
    let commands = [
        (
            "broadcast_psbt",
            json!({ "psbt": "cHNidP8BAAoCAAAAAAAAAAAAAAAA" }),
        ),
        (
            "move_mixed_coins",
            json!({ "walletId": wallet, "destination": "wallet", "quoteId": "q", "grantId": "g" }),
        ),
        (
            "consolidate_mixed_coins",
            json!({ "walletId": wallet, "quoteId": "q", "grantId": "g" }),
        ),
        (
            "resend_mixed_coins",
            json!({ "walletId": wallet, "txid": txid }),
        ),
        (
            "resend_transaction",
            json!({ "walletId": wallet, "txid": txid }),
        ),
        (
            "governance_submit_proposal",
            json!({ "walletId": wallet, "hash": txid }),
        ),
        (
            "platform_send",
            json!({ "walletId": wallet, "operation": "op", "grant": "g" }),
        ),
    ];
    let taken = || (demo.state().broadcasts_taken).load(std::sync::atomic::Ordering::SeqCst);
    for (cmd, args) in commands {
        let before = taken();
        let _ = demo.call(cmd, args);
        assert_eq!(taken(), before + 1, "{cmd} took no broadcast guard");
        assert_eq!(
            demo.state()
                .broadcasts()
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }
}
