//! The masternodes commands (MG-10 S1) over Tauri's IPC against a real,
//! offline regtest engine: no masternode list is ever synced here, so every
//! list-backed call answers as it does before the list arrives, and the
//! keychain answers in full.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

use serde_json::{Value, json};

use crate::ipc_tests::Demo;

const PW: &str = "pw";

fn hash(byte: &str) -> String {
    byte.repeat(32)
}

/// An open demo with an encrypted vault and one seed wallet.
fn demo_with_wallet() -> (Demo, String) {
    let demo = Demo::new();
    demo.open();
    with_wallet(demo, json!(PW))
}

/// `demo`, opened, with a vault of `passphrase` (`null`: unencrypted) and
/// one seed wallet.
fn with_wallet(demo: Demo, passphrase: Value) -> (Demo, String) {
    demo.ok("create_vault", json!({ "passphrase": passphrase }));
    let wallet = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"]
        .as_str()
        .unwrap()
        .to_owned();
    (demo, wallet)
}

/// A watch-only wallet of `wallet`'s first account.
fn watch_only(demo: &Demo, wallet: &str) -> String {
    let xpub = demo.ok("account_xpub", json!({ "walletId": wallet, "account": 0 }))["xpub"].clone();
    demo.ok(
        "import_watch_only",
        json!({ "xpub": xpub, "options": { "name": "Watched" } }),
    )
    .as_str()
    .unwrap()
    .to_owned()
}

fn grant(demo: &Demo, wallet: &str, purpose: Value) -> String {
    demo.ok(
        "authorize",
        json!({
            "purpose": purpose,
            "walletId": wallet,
            "credential": { "kind": "passphrase", "passphrase": PW },
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn reveal_grant(demo: &Demo, wallet: &str) -> String {
    grant(demo, wallet, json!({ "kind": "revealSecret" }))
}

fn reveal(demo: &Demo, wallet: &str, role: &str, index: u32, grant: &str) -> Result<Value, Value> {
    demo.call(
        "mn_reveal_key",
        json!({ "walletId": wallet, "role": role, "index": index, "grant": grant }),
    )
}

fn keys(demo: &Demo, wallet: &str, role: &str, start: u32, count: u32) -> Result<Value, Value> {
    demo.call(
        "mn_keys",
        json!({ "walletId": wallet, "role": role, "start": start, "count": count }),
    )
}

/// The P2PKH address of a regtest WIF.
fn wif_address(wif: &str) -> String {
    let key = dashcore::PrivateKey::from_wif(wif).expect("a WIF");
    dashcore::Address::p2pkh(&key.public_key(), dashcore::Network::Regtest).to_string()
}

#[test]
fn reads_answer_before_the_list_is_synced() {
    let (demo, wallet) = demo_with_wallet();
    let watched = watch_only(&demo, &wallet);
    assert_eq!(
        demo.ok("mn_list_state", json!({})),
        json!({ "synced": false, "height": null })
    );
    let empty = json!({
        "list": { "synced": false, "height": null },
        "masternodes": [],
        "collateralCoins": [],
        "payouts30d": 0,
        "unattributed30d": 0,
        "payoutsIncomplete": false,
    });
    assert_eq!(demo.ok("mn_list", json!({ "walletId": wallet })), empty);
    assert_eq!(demo.ok("mn_list", json!({ "walletId": watched })), empty);
    demo.ok("lock", json!({}));
    assert_eq!(demo.ok("mn_list", json!({ "walletId": wallet })), empty);

    let other = hash("ef");
    for cmd in ["mn_list", "mn_masternode", "mn_payouts"] {
        let args = json!({ "walletId": other, "proTxHash": hash("ab"), "since": null });
        let e = demo.err(cmd, args, "wallet_not_found");
        assert!(!e["message"].as_str().unwrap().contains(&other), "{e}");
        demo.err(
            cmd,
            json!({ "walletId": "zz", "proTxHash": hash("ab"), "since": null }),
            "invalid_argument",
        );
    }
    for cmd in ["mn_masternode", "mn_payouts"] {
        let malformed = json!({ "walletId": wallet, "proTxHash": "ab", "since": null });
        demo.err(cmd, malformed, "invalid_argument");
        let unknown = json!({ "walletId": wallet, "proTxHash": hash("ab"), "since": 0 });
        demo.err(cmd, unknown, "masternodes.list_unavailable");
    }
    demo.err(
        "mn_find",
        json!({ "query": "not a query" }),
        "invalid_argument",
    );
    for query in [hash("ab"), "203.0.113.7:19999".into()] {
        demo.err(
            "mn_find",
            json!({ "query": query }),
            "masternodes.list_unavailable",
        );
    }
    demo.err(
        "mn_evonode_credits",
        json!({ "proTxHash": "ab" }),
        "invalid_argument",
    );
    demo.err(
        "mn_evonode_credits",
        json!({ "proTxHash": hash("ab") }),
        "masternodes.list_unavailable",
    );
}

#[test]
fn tracking_refuses_in_the_contract_order() {
    let (demo, _) = demo_with_wallet();
    let events = demo.events();
    let h = hash("ab");
    demo.err(
        "mn_track",
        json!({ "proTxHash": "ab", "label": null }),
        "invalid_argument",
    );
    demo.err(
        "mn_track",
        json!({ "proTxHash": h, "label": "x".repeat(1000) }),
        "invalid_argument",
    );
    demo.err(
        "mn_track",
        json!({ "proTxHash": h, "label": "Home node" }),
        "masternodes.list_unavailable",
    );
    demo.err(
        "mn_set_label",
        json!({ "proTxHash": h, "label": "Home node" }),
        "masternodes.list_unavailable",
    );
    demo.err(
        "mn_untrack",
        json!({ "proTxHash": h }),
        "masternodes.not_tracked",
    );
    for role in ["owner", "voting", "payout"] {
        demo.err(
            "mn_detach_key",
            json!({ "proTxHash": h, "role": role }),
            "masternodes.not_tracked",
        );
    }
    let operator = json!({ "proTxHash": h, "role": "operator" });
    assert!(demo.call("mn_detach_key", operator).is_err());

    // A pasted key is never echoed, refused or not.
    let pasted = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";
    let e = demo.err(
        "mn_attach_key",
        json!({ "proTxHash": h, "role": "owner", "key": "not a key" }),
        "invalid_argument",
    );
    assert!(!e.to_string().contains("not a key"), "{e}");
    for role in ["owner", "voting", "payout"] {
        let e = demo.err(
            "mn_attach_key",
            json!({ "proTxHash": h, "role": role, "key": pasted }),
            "masternodes.not_tracked",
        );
        assert!(!e.to_string().contains(pasted), "{e}");
    }
    assert!(
        events.lock().unwrap().iter().all(|e| !e.contains(pasted)),
        "an event carries the pasted key"
    );
}

/// `mn_keys` and `mn_reveal_key` name the same key: the revealed WIF's
/// address is the listed one, for the role and index asked.
#[test]
fn the_keychain_lists_and_reveals_the_same_keys() {
    let (demo, wallet) = demo_with_wallet();
    for (role, sub) in [("voting", 1), ("owner", 2)] {
        let listed = keys(&demo, &wallet, role, 3, 2).unwrap();
        let listed = listed.as_array().unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["role"], role);
        assert_eq!(listed[0]["index"], 3);
        assert_eq!(listed[0]["derivationPath"], format!("m/9'/1'/3'/{sub}'/3"));
        let shown = reveal(&demo, &wallet, role, 4, &reveal_grant(&demo, &wallet)).unwrap();
        assert_eq!(
            wif_address(shown["wif"].as_str().unwrap()),
            listed[1]["address"]
        );
        assert_eq!(shown["privateKeyHex"].as_str().unwrap().len(), 64);
        assert_eq!(shown["tenderdashKey"], Value::Null);
    }
    let operator = keys(&demo, &wallet, "operator", 0, 1).unwrap();
    assert_eq!(operator[0]["publicKeyHex"].as_str().unwrap().len(), 96);
    assert_eq!(
        operator[0]["legacyPublicKeyHex"].as_str().unwrap().len(),
        96
    );
    assert_eq!(operator[0]["address"], Value::Null);
    let node = keys(&demo, &wallet, "platform_node", 0, 1).unwrap();
    assert_eq!(node[0]["derivationPath"], "m/9'/1'/3'/4'/0'");
    assert_eq!(node[0]["platformNodeId"].as_str().unwrap().len(), 40);
    let shown = reveal(
        &demo,
        &wallet,
        "platform_node",
        0,
        &reveal_grant(&demo, &wallet),
    )
    .unwrap();
    assert_eq!(shown["tenderdashKey"].as_str().unwrap().len(), 88);

    // Public keys need no unlocked vault.
    demo.ok("lock", json!({}));
    assert_eq!(keys(&demo, &wallet, "operator", 0, 1).unwrap(), operator);

    demo.err(
        "mn_keys",
        json!({ "walletId": wallet, "role": "owner", "start": 0, "count": 101 }),
        "invalid_argument",
    );
    demo.err(
        "mn_keys",
        json!({ "walletId": hash("ef"), "role": "owner", "start": 0, "count": 1 }),
        "wallet_not_found",
    );
}

#[test]
fn a_watch_only_wallet_has_no_keychain() {
    let (demo, wallet) = demo_with_wallet();
    let watched = watch_only(&demo, &wallet);
    for role in ["owner", "voting", "operator", "platform_node"] {
        let e = keys(&demo, &watched, role, 0, 1).unwrap_err();
        assert_eq!(e["code"], "masternode.watch_only", "{role}: {e}");
    }
    let e = reveal(&demo, &watched, "owner", 0, &reveal_grant(&demo, &watched)).unwrap_err();
    assert_eq!(e["code"], "masternode.watch_only", "{e}");
    let e = reveal(&demo, &watched, "owner", 0, &reveal_grant(&demo, &wallet)).unwrap_err();
    assert_eq!(e["code"], "masternode.grant_invalid", "{e}");
}

/// Each refusal of a reveal: no grant, a grant of another purpose or
/// wallet, a spent grant, a locked vault (which revoked every grant and
/// issues one only for the passphrase). Nothing is revealed.
#[test]
fn a_reveal_needs_its_own_unspent_grant_and_an_unlocked_vault() {
    let (demo, wallet) = demo_with_wallet();
    let other = demo.ok("create_wallet", json!({ "wordCount": 12 }))["walletId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = |grant: &str| {
        let e = reveal(&demo, &wallet, "owner", 0, grant).unwrap_err();
        assert_eq!(e["code"], "masternode.grant_invalid", "{e}");
    };
    refused("no-such-grant");
    refused(&grant(&demo, &wallet, json!({ "kind": "signMessage" })));
    refused(&grant(
        &demo,
        &wallet,
        json!({ "kind": "masternodeOp", "maxDuffs": 0 }),
    ));
    refused(&reveal_grant(&demo, &other));

    let once = reveal_grant(&demo, &wallet);
    reveal(&demo, &wallet, "owner", 0, &once).unwrap();
    refused(&once);

    let before_lock = reveal_grant(&demo, &wallet);
    demo.ok("lock", json!({}));
    refused(&before_lock);
    demo.err(
        "authorize",
        json!({
            "purpose": { "kind": "revealSecret" },
            "walletId": wallet,
            "credential": { "kind": "unencrypted" },
        }),
        "vault.credential_required",
    );
    demo.err(
        "mn_reveal_key",
        json!({ "walletId": hash("ef"), "role": "owner", "index": 0, "grant": before_lock }),
        "wallet_not_found",
    );
}

/// The registrar update (MG-10 S3) takes the renderer's request shape and
/// refuses before anything is signed while the list is not synced.
#[test]
fn a_registrar_update_is_refused_before_the_list_is_synced() {
    let (demo, wallet) = demo_with_wallet();
    let request = |operator: Value, voting: Value, payout: Value| json!({ "proTxHash": hash("ab"), "operator": operator, "voting": voting, "payout": payout });
    let requests = [
        request(
            json!({ "kind": "keep" }),
            json!({ "kind": "keep" }),
            json!({ "kind": "new_address" }),
        ),
        request(
            json!({ "kind": "keychain", "index": 1 }),
            json!({ "kind": "next_unused" }),
            json!({ "kind": "new_address" }),
        ),
    ];
    for r in &requests {
        demo.err(
            "mn_preview_update_registrar",
            json!({ "walletId": wallet, "request": r }),
            "masternodes.list_unavailable",
        );
        let g = grant(
            &demo,
            &wallet,
            json!({ "kind": "masternodeOp", "maxDuffs": 10_000 }),
        );
        demo.err(
            "mn_update_registrar",
            json!({ "walletId": wallet, "request": r, "grant": g }),
            "masternodes.list_unavailable",
        );
    }
    let unknown_kind = request(
        json!({ "kind": "operator_secret" }),
        json!({ "kind": "keep" }),
        json!({ "kind": "new_address" }),
    );
    demo.call(
        "mn_preview_update_registrar",
        json!({ "walletId": wallet, "request": unknown_kind }),
    )
    .unwrap_err();
    assert_eq!(
        demo.ok("mn_operations", json!({ "walletId": wallet })),
        json!([])
    );
    demo.err(
        "mn_discard_operation",
        json!({ "walletId": wallet, "operation": "no-such-operation" }),
        "invalid_argument",
    );
    demo.err(
        "mn_preview_update_registrar",
        json!({ "walletId": hash("ef"), "request": requests[0] }),
        "wallet_not_found",
    );
}

static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: Mutex<Vec<u8>> = Mutex::new(Vec::new());

struct Capture;

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if CAPTURING.load(Ordering::SeqCst) {
            CAPTURED.lock().unwrap().extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Every log record of the process, at trace, while `CAPTURING` is set;
/// outside that window no record is even formatted.
fn capture_logs() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{Layer, filter};
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(|| Capture)
                    .with_filter(filter::filter_fn(|_| CAPTURING.load(Ordering::SeqCst))),
            )
            .try_init()
            .expect("no other global subscriber in dw-app's tests");
    });
}

/// A revealed key reaches the caller only: no log record at trace and no
/// event carries any form of it.
#[test]
fn a_revealed_key_is_in_no_log_and_no_event() {
    capture_logs();
    let (demo, wallet) = demo_with_wallet();
    let events = demo.events();
    CAPTURING.store(true, Ordering::SeqCst);
    tracing::info!("reveal-capture-probe");
    let mut secrets = Vec::new();
    let mut byte_forms = Vec::new();
    for (role, index) in [
        ("owner", 0),
        ("voting", 1),
        ("operator", 2),
        ("platform_node", 0),
    ] {
        let shown = reveal(&demo, &wallet, role, index, &reveal_grant(&demo, &wallet)).unwrap();
        for field in ["privateKeyHex", "wif", "tenderdashKey"] {
            if let Some(s) = shown[field].as_str() {
                secrets.push(s.to_owned());
                if field == "privateKeyHex" {
                    byte_forms.push(format!("{:?}", hex::decode(s).unwrap()));
                }
            }
        }
    }
    let spent = reveal_grant(&demo, &wallet);
    reveal(&demo, &wallet, "owner", 0, &spent).unwrap();
    let refusal = reveal(&demo, &wallet, "owner", 0, &spent).unwrap_err();
    CAPTURING.store(false, Ordering::SeqCst);
    assert_eq!(secrets.len(), 7, "{secrets:?}");
    assert!(!byte_forms.is_empty());
    secrets.extend(byte_forms);

    let logs = String::from_utf8_lossy(&CAPTURED.lock().unwrap()).into_owned();
    assert!(logs.contains("reveal-capture-probe"), "the capture is live");
    let events = events.lock().unwrap().join("\n");
    for secret in &secrets {
        assert!(
            !logs.contains(secret.as_str()),
            "a log record holds a revealed key"
        );
        assert!(
            !events.contains(secret.as_str()),
            "an event holds a revealed key"
        );
        assert!(!refusal.to_string().contains(secret.as_str()));
    }
}

/// An attached key goes into the vault and never comes back: no read the
/// renderer makes (the overview, the detail, the vote and username voters)
/// and no event carries it in any form, whether or not the read answers.
#[test]
fn an_attached_key_is_in_no_read_and_no_event() {
    let (demo, wallet) = demo_with_wallet();
    let events = demo.events();
    let h = hash("c1");
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    dw_engine::test_support::track_unlisted(&session, [0xc1; 32]);
    drop(session);

    let wif = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";
    let owner_hex = "3a".repeat(32);
    let attach = |role: &str, key: &str| {
        demo.ok(
            "mn_attach_key",
            json!({ "proTxHash": h, "role": role, "key": key }),
        )
    };
    // A voting key is checked against the list, which this offline demo never has
    // (`list_unavailable`); the owner and payout keys go into the same vault records.
    assert_eq!(attach("owner", wif), json!("unchecked"));
    assert_eq!(attach("payout", &owner_hex), json!("unchecked"));

    let wif_secret = dashcore::PrivateKey::from_wif(wif)
        .unwrap()
        .inner
        .to_secret_bytes();
    let hex_secret = hex::decode(&owner_hex).unwrap();
    let mut forms = vec![wif.to_owned(), owner_hex.clone(), owner_hex.to_uppercase()];
    for bytes in [&wif_secret[..], &hex_secret[..]] {
        forms.push(hex::encode(bytes));
        forms.push(hex::encode_upper(bytes));
        forms.push(format!("{bytes:?}"));
    }

    let overview = demo.ok("mn_list", json!({ "walletId": wallet }));
    let row = (overview["masternodes"].as_array().unwrap().iter())
        .find(|r| r["proTxHash"] == json!(h))
        .expect("the tracked row");
    let roles: Vec<_> = (row["roles"]["attached"].as_array().unwrap().iter())
        .map(|k| k["role"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(roles.len(), 2, "both keys are read back: {row}");
    let reads = [
        Ok(overview),
        demo.call(
            "mn_masternode",
            json!({ "walletId": wallet, "proTxHash": h }),
        ),
        demo.call(
            "governance_voting_masternodes",
            json!({ "walletId": wallet, "proposal": hash("aa") }),
        ),
        demo.call("governance_contested_names", json!({ "walletId": wallet })),
    ];
    let events = events.lock().unwrap().join("\n");
    for read in reads {
        let text = match read {
            Ok(v) | Err(v) => v.to_string(),
        };
        for form in &forms {
            assert!(
                !text.contains(form.as_str()),
                "a read holds an attached key: {text}"
            );
        }
    }
    for form in &forms {
        assert!(
            !events.contains(form.as_str()),
            "an event holds an attached key"
        );
    }
}

/// An open demo on `network` with an unencrypted vault (mainnet and testnet
/// refuse the tests' weak KDF) and one seed wallet. Its session opens with
/// unreachable local endpoints (the test seam mainnet and testnet need) and
/// no SPV.
fn demo_on(network: dw_engine::DashNetwork) -> (Demo, String) {
    let demo = Demo::launched(|launch, _| {
        launch.network = network;
        launch.sync = Some(false);
    });
    *demo.state().test_endpoints.lock().unwrap() = Some(crate::api::network::SessionSettings {
        dapi_addresses: vec!["https://127.0.0.1:1".into()],
        quorum_url: Some("https://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        no_platform: true,
        ..Default::default()
    });
    demo.ok("open_network", json!({}));
    with_wallet(demo, Value::Null)
}

/// A `masternodeOp` grant of `max` on [`demo_on`]'s unencrypted vault.
fn op_grant(demo: &Demo, wallet: &str, max: u64) -> String {
    demo.ok(
        "authorize",
        json!({
            "purpose": { "kind": "masternodeOp", "maxDuffs": max },
            "walletId": wallet,
            "credential": { "kind": "unencrypted" },
        }),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Abandon is the way out of a session that may be out, so no network gates
/// it: on mainnet as on the test networks every abandon command reaches the engine,
/// which answers for the session itself.
#[test]
fn abandon_reaches_the_engine_on_every_network() {
    for network in [
        dw_engine::DashNetwork::Mainnet,
        dw_engine::DashNetwork::Testnet,
        dw_engine::DashNetwork::Regtest,
    ] {
        let (demo, wallet) = demo_on(network);
        let grant = op_grant(&demo, &wallet, 1_000);
        for cmd in [
            "mn_shared_abandon",
            "mn_shared_resend_abandon",
            "mn_shared_abandon_unreadable",
        ] {
            demo.err(
                cmd,
                json!({ "walletId": wallet, "session": hash("cd"), "grant": grant }),
                "masternodes.shared_session_not_found",
            );
        }
    }
}

/// `mn_shared_sign` and `mn_shared_send` with their arguments.
fn registration_sends(wallet: &str, grant: &str) -> [(&'static str, Value); 2] {
    let args = json!({ "walletId": wallet, "session": hash("cd"), "grant": grant });
    [("mn_shared_sign", args.clone()), ("mn_shared_send", args)]
}

/// Redeems `grant` in the vault directly, as the engine would.
fn redeem(demo: &Demo, wallet: &str, grant: &str) -> Result<(), dw_vault::VaultError> {
    let session = tauri::async_runtime::block_on(demo.state().session()).unwrap();
    let wallet = crate::state::parse_wallet_id(wallet).unwrap();
    session
        .vault()
        .redeem_grant(grant, dw_vault::GrantKind::MasternodeOp, Some(&wallet.0))
        .map(drop)
}

/// MG's ruling: on mainnet the binding refuses signing and sending a shared
/// registration before the engine, with the network named, and before
/// anything redeems the grant: the same grant still redeems afterwards.
#[test]
fn registration_sends_are_refused_on_mainnet_before_the_grant() {
    let (demo, wallet) = demo_on(dw_engine::DashNetwork::Mainnet);
    for cmd in ["mn_shared_sign", "mn_shared_send"] {
        let grant = op_grant(&demo, &wallet, 300 * 100_000_000);
        let args = json!({ "walletId": wallet, "session": hash("cd"), "grant": grant });
        let e = demo.err(cmd, args, "app.shared_send_off");
        assert_eq!(
            e["data"],
            json!({ "kind": "sharedSendNetwork", "network": "mainnet" }),
            "{cmd}"
        );
        redeem(&demo, &wallet, &grant).unwrap_or_else(|e| panic!("{cmd} used the grant: {e}"));
        assert!(
            redeem(&demo, &wallet, &grant).is_err(),
            "a grant redeems once"
        );
    }
}

/// Off mainnet the gate lets both calls through to the engine, which
/// refuses them for its own reasons (no masternode list offline, no such
/// session).
#[test]
fn registration_sends_reach_the_engine_off_mainnet() {
    for network in [
        dw_engine::DashNetwork::Testnet,
        dw_engine::DashNetwork::Regtest,
    ] {
        let (demo, wallet) = demo_on(network.clone());
        let grant = op_grant(&demo, &wallet, 0);
        for (cmd, args) in registration_sends(&wallet, &grant) {
            let e = demo.call(cmd, args).expect_err(cmd);
            assert!(
                !e["code"].as_str().unwrap().starts_with("app."),
                "{cmd} on {network:?}: {e}"
            );
        }
    }
}
