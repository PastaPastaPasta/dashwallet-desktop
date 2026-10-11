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
    assert_eq!(launch["fixture"], false);

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
    // The engine's own refusal crosses with its code and typed data.
    let e = demo.err(
        "dashpay_status",
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
