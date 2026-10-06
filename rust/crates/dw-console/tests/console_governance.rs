//! Governance console commands against an offline regtest session (QT-145,
//! M3 R2): the budget needs only the height, the rest needs synced data or
//! is not offered in SPV mode.

use std::sync::Arc;

use dw_console::{ConsoleContext, ConsoleFailure};
use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

struct Null;
impl EventSink for Null {
    fn emit(&self, _: EngineEvent) {}
}

#[test]
fn test_qt_145_governance_console_commands_offline() {
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
    let mut ctx = ConsoleContext {
        session: Arc::clone(&s),
        wallet: None,
        grant_id: None,
    };
    let mut run = |line: &str| engine.block_on(ctx.run(line));

    // Core's formula from the height alone: regtest superblock 1500, 20
    // blocks a cycle (budget > 0); 1501 is no superblock.
    let budget = run("getsuperblockbudget 1500").unwrap().result;
    assert!(budget.parse::<f64>().unwrap() > 0.0, "{budget}");
    assert_eq!(
        run("getsuperblockbudget 1501").unwrap().result,
        "0.00000000"
    );
    assert!(matches!(
        run("getsuperblockbudget"),
        Err(ConsoleFailure::Rpc { code: -1, .. })
    ));
    // No tip yet.
    assert!(matches!(
        run("getgovernanceinfo"),
        Err(ConsoleFailure::Rpc { code: -1, .. })
    ));
    // Governance sync is off.
    match run("gobject list") {
        Err(ConsoleFailure::Rpc { message, .. }) => {
            assert!(message.contains("sync is off"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        run("gobject prepare 0 1 0 00"),
        Err(ConsoleFailure::NotAvailable(_))
    ));
    assert!(matches!(
        run(
            "gobject vote-many 0101010101010101010101010101010101010101010101010101010101010101 delete yes"
        ),
        Err(ConsoleFailure::Rpc { code: -8, .. })
    ));
    engine.block_on(engine.shutdown()).unwrap();
}
