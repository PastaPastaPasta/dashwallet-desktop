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

    /// Opens the launch network (regtest) offline.
    pub(crate) fn open(&self) -> Value {
        self.ok("open_network", json!({ "settings": offline_settings() }))
    }
}

/// [`Demo::call`] without the app, so several threads can invoke at once.
fn invoke(
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
/// F1): on an encrypted, unlocked vault, `spend` and `platformOp` grants
/// need a credential while it is on, and turning it either way needs one.
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
    let change = json!({ "kind": "changeCredential" });
    let authorize = |purpose: &Value, wallet: &Value, credential: &Value| json!({ "purpose": purpose, "walletId": wallet, "credential": credential });
    let refused = |demo: &Demo| {
        for purpose in [&spend, &platform] {
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

    // On by default: no grant for a payment without the passphrase.
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    refused(&demo);
    demo.ok("authorize", authorize(&spend, &wallet, &pw));
    demo.ok("authorize", authorize(&platform, &wallet, &pw));
    // Other purposes keep the vault's own rules.
    demo.ok(
        "authorize",
        authorize(&json!({ "kind": "signMessage" }), &wallet, &none),
    );

    // Turning it off needs a credential: no changeCredential grant without
    // one, and no other grant stands in for it.
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
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    let grant = demo.ok("authorize", authorize(&change, &Value::Null, &pw))["id"].clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": false, "grantId": grant }),
    );

    // Off: dash-qt behaviour, an unlocked vault grants without a prompt.
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    demo.ok("authorize", authorize(&spend, &wallet, &none));
    demo.ok("authorize", authorize(&platform, &wallet, &none));
    demo.ok(
        "authorize_set",
        json!({ "purposes": [platform, spend], "walletId": wallet, "credential": none }),
    );

    // Turning it back on needs a credential too.
    demo.err(
        "set_payment_authentication",
        json!({ "on": true, "grantId": "made-up" }),
        "vault.grant_invalid",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), false);
    let grant = demo.ok("authorize", authorize(&change, &Value::Null, &pw))["id"].clone();
    demo.ok(
        "set_payment_authentication",
        json!({ "on": true, "grantId": grant }),
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    refused(&demo);

    // Locked, the vault answers first, as without the setting.
    demo.ok("lock", json!({}));
    demo.err(
        "authorize",
        authorize(&spend, &wallet, &none),
        "vault.locked",
    );
}

/// On an unencrypted vault there is no credential to ask for: the setting
/// changes nothing and cannot be changed, so no value stored without a
/// credential carries over to the vault once encrypted.
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
        json!({ "on": false, "grantId": grant }),
        "vault.not_encrypted",
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
}

/// Sol r2 F1: the setting belongs to the vault. "Off", set with the
/// passphrase, does not carry over to a vault created after `destroyVault`,
/// even one encrypted and unlocked the same way.
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
        json!({ "on": false, "grantId": grant }),
    );
    assert_eq!(demo.ok("payment_authentication", json!({})), false);

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

    // A new vault, encrypted later, then locked and unlocked.
    demo.ok("create_vault", json!({ "passphrase": null }));
    let grant = change(&none);
    demo.ok(
        "encrypt_vault",
        json!({ "newPassphrase": "pw", "grantId": grant }),
    );
    demo.ok("lock", json!({}));
    demo.ok("unlock", json!({ "passphrase": "pw" }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"].clone();
    assert_eq!(demo.ok("payment_authentication", json!({})), true);
    demo.err(
        "authorize",
        json!({ "purpose": { "kind": "spend", "maxDuffs": 1000 }, "walletId": wallet, "credential": none }),
        "vault.credential_required",
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
    demo.ok("dismiss_stashed_link", json!({ "linkId": invite_id }));
    assert_eq!(demo.ok("stashed_links", json!({})), json!([]));
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
            // No network is open, so there is no stash to keep it in:
            // an invitation never reaches the renderer bare (DP5-01 D3).
            { "kind": "rejected", "reason": { "kind": "notSaved" } },
        ])
    );
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
