//! The console against an offline engine session (QT-145).

use std::sync::Arc;

use dw_console::{ConsoleContext, ConsoleFailure};
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, SessionOptions,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

struct Null;
impl EventSink for Null {
    fn emit(&self, _: EngineEvent) {}
}

#[test]
fn test_qt_145_console_answers_wallet_commands_offline() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("d"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Null),
    )
    .unwrap();
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
            },
        ))
        .unwrap();
    engine
        .block_on(s.vault_op(|v| v.create(Some(b"console pass"))))
        .unwrap();
    engine
        .block_on(s.vault_op(|v| v.unlock(b"console pass", dw_vault::UnlockScope::Full)))
        .unwrap();
    let w = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let mut ctx = ConsoleContext {
        session: Arc::clone(&s),
        wallet: None,
        grant_id: None,
    };
    let run = |ctx: &mut ConsoleContext, line: &str| engine.block_on(ctx.run(line));

    // Wallet commands need a wallet.
    assert!(matches!(
        run(&mut ctx, "getwalletinfo"),
        Err(ConsoleFailure::WalletRequired)
    ));
    // Full-node commands answer honestly.
    assert!(matches!(
        run(&mut ctx, "getblockhash 0"),
        Err(ConsoleFailure::NotAvailable(c)) if c == "getblockhash"
    ));
    assert!(matches!(
        run(&mut ctx, "nosuchcommand"),
        Err(ConsoleFailure::NotAvailable(_))
    ));
    let help = run(&mut ctx, "help").unwrap();
    assert!(!help.is_json && help.result.contains("== Wallet =="));
    assert!(
        run(&mut ctx, "help-console")
            .unwrap()
            .result
            .contains("getblockhash(0)")
    );

    ctx.wallet = Some(w);
    let info = run(&mut ctx, "getwalletinfo").unwrap();
    assert!(info.is_json);
    assert!(
        info.result.starts_with("{\n  \"walletname\": \"Wallet 1\""),
        "{}",
        info.result
    );
    assert!(info.result.contains("\"private_keys_enabled\": true"));
    assert!(info.result.contains("\"scanning\": false"));
    assert_eq!(
        run(&mut ctx, "listwallets").unwrap().result,
        "[\n  \"Wallet 1\"\n]"
    );
    assert_eq!(run(&mut ctx, "listtransactions").unwrap().result, "[\n]");
    assert_eq!(run(&mut ctx, "listlockunspent").unwrap().result, "[\n]");

    // getnewaddress hands out the wallet's receive address; validateaddress
    // and nested calls work on it.
    let addr = run(&mut ctx, "getnewaddress \"first\"").unwrap();
    assert!(!addr.is_json && addr.result.starts_with('y'));
    let v = run(&mut ctx, "validateaddress(getnewaddress())[isvalid]").unwrap();
    assert_eq!(v.result, "true");

    // signmessage asks for a grant, then signs with it; verifymessage
    // checks the signature.
    let line = format!("signmessage {} hello", addr.result);
    match run(&mut ctx, &line) {
        Err(ConsoleFailure::AuthorizationRequired {
            purpose: GrantPurpose::SignMessage,
            wallet,
        }) => assert_eq!(wallet, Some(w)),
        other => panic!("{other:?}"),
    }
    let grant = engine
        .block_on(s.vault_op(move |v| {
            v.authorize(GrantPurpose::SignMessage, Some(&w.0), Credential::None)
        }))
        .unwrap();
    ctx.grant_id = Some(grant.id);
    let sig = run(&mut ctx, &line).unwrap().result;
    let ok = run(
        &mut ctx,
        &format!("verifymessage {} {sig} hello", addr.result),
    )
    .unwrap();
    assert_eq!(ok.result, "true");
    let bad = run(
        &mut ctx,
        &format!("verifymessage {} {sig} other", addr.result),
    )
    .unwrap();
    assert_eq!(bad.result, "false");

    // walletlock / walletpassphrase map to the vault.
    run(&mut ctx, "walletlock").unwrap();
    assert_eq!(s.vault().lock_state(), dw_vault::LockState::Locked);
    match run(&mut ctx, "walletpassphrase wrong 60") {
        Err(ConsoleFailure::Rpc { code: -14, .. }) => {}
        other => panic!("{other:?}"),
    }
    run(&mut ctx, "walletpassphrase \"console pass\" 600").unwrap();
    assert_eq!(s.vault().lock_state(), dw_vault::LockState::Unlocked);
    assert_eq!(
        dw_console::redact("walletpassphrase \"console pass\" 600").unwrap(),
        "walletpassphrase(…)"
    );

    // Review L1: one relock timer per session. A later walletpassphrase
    // replaces a shorter earlier one, walletlock cancels it, and the timer
    // does not keep the session alive.
    // The 1 s timer fires: waited for with a deadline, not a fixed sleep,
    // so a loaded machine only makes the test slower. Its observed delay
    // sizes the wait of the negative check below.
    run(&mut ctx, "walletpassphrase \"console pass\" 1").unwrap();
    let started = std::time::Instant::now();
    let deadline = started + std::time::Duration::from_secs(30);
    while s.vault().lock_state() != dw_vault::LockState::Locked || s.relock_pending() {
        assert!(
            std::time::Instant::now() < deadline,
            "the 1 s timer never locked the vault"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let fired_after = started.elapsed();
    run(&mut ctx, "walletpassphrase \"console pass\" 600").unwrap();

    let strong = Arc::strong_count(&s);
    run(&mut ctx, "walletpassphrase \"console pass\" 1").unwrap();
    run(&mut ctx, "walletpassphrase \"console pass\" 600").unwrap();
    assert_eq!(Arc::strong_count(&s), strong, "the timer holds the session");
    // Twice as long as the first timer took (at least 1.5 s): a replaced
    // timer that still ran would have locked the vault by then.
    std::thread::sleep((fired_after * 2).max(std::time::Duration::from_millis(1500)));
    assert_eq!(
        s.vault().lock_state(),
        dw_vault::LockState::Unlocked,
        "the replaced 1 s timer locked the vault"
    );
    assert!(s.relock_pending());
    run(&mut ctx, "walletlock").unwrap();
    assert!(!s.relock_pending());
    run(&mut ctx, "walletpassphrase \"console pass\" 600").unwrap();

    // Errors with Core codes.
    match run(
        &mut ctx,
        "gettransaction 0101010101010101010101010101010101010101010101010101010101010101",
    ) {
        Err(ConsoleFailure::Rpc { code: -5, message }) => {
            assert_eq!(message, "Invalid or non-wallet transaction id")
        }
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(
            run(&mut ctx, "getbalance"),
            Err(ConsoleFailure::Rpc { code: -4, .. })
        ),
        "balance unknown before any scan"
    );
    engine.block_on(engine.shutdown()).unwrap();
}
