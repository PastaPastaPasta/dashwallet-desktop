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
use tauri::{App, WebviewWindow};

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
        let dir = dw_testutil::private_tempdir();
        let launch = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            network: DashNetwork::Regtest,
            ..LaunchOptions::default()
        };
        let vault = VaultConfig {
            kdf: KdfPolicy::Fixed(KdfParams::TEST),
            os_store: Arc::new(MemoryOsStore::new()),
            ..VaultConfig::default()
        };
        let mut app = crate::builder_with_vault::<MockRuntime>(launch, Instant::now(), vault)
            .build(mock_context(noop_assets()))
            .expect("app");
        // The first iteration runs `setup`, which creates the engine. Called
        // once, not in a loop (the deprecation is about busy-looping).
        #[allow(deprecated)]
        app.run_iteration(|_, _| {});
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
        let response = tauri::test::get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: tauri::ipc::CallbackFn(0),
                error: tauri::ipc::CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: tauri::ipc::InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: self.app.invoke_key().to_string(),
            },
        );
        response.map(|b| b.deserialize::<Value>().expect("JSON result"))
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

    /// Opens the launch network (regtest) offline.
    pub(crate) fn open(&self) -> Value {
        self.ok("open_network", json!({ "settings": offline_settings() }))
    }
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

#[test]
fn engine_and_session_commands_over_ipc() {
    let demo = Demo::new();
    // Session commands before a network is open.
    demo.err("list_wallets", json!({}), "app.network_not_open");
    assert_eq!(demo.ok("active_network", json!({})), Value::Null);

    let launch = demo.ok("launch_info", json!({}));
    assert_eq!(launch["network"]["dirName"], "regtest");
    assert_eq!(launch["fixture"], false);

    // Engine::open_network, with the renderer's settings over the launch's.
    let opened = demo.open();
    assert_eq!(opened["network"]["dirName"], "regtest");
    assert_eq!(opened["network"]["unitName"], "tDASH");
    assert_eq!(demo.ok("active_network", json!({}))["dirName"], "regtest");
    // Already active: the same session, settings ignored.
    assert_eq!(demo.ok("open_network", json!({})), opened);
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
    demo.err("lock_status", json!({}), "app.network_not_open");
}

#[test]
fn the_binding_refuses_with_app_codes() {
    let demo = Demo::new();
    demo.err(
        "selftest_checkpoint",
        json!({ "screen": "x" }),
        "app.not_selftest",
    );
    // A missing argument is Tauri's own refusal (a string, not an AppError).
    let e = demo.call("startup_mark", json!({})).unwrap_err();
    assert!(e.is_string(), "{e}");
    assert!(demo.ok("startup_mark", json!({ "mark": "test" })).is_u64());
}
