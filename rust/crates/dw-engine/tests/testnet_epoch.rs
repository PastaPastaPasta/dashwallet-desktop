use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dw_engine::platform::QuorumSource;
use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{MemoryOsStore, VaultConfig};

struct Sink;

impl EventSink for Sink {
    fn emit(&self, _event: EngineEvent) {}
}

#[test]
#[ignore = "read-only public testnet check; requires testnet SPV and Platform access"]
fn testnet_epoch_sets_network_version() {
    let expected: u32 = std::env::var("DW_TESTNET_PROTOCOL_VERSION")
        .expect(
            "set DW_TESTNET_PROTOCOL_VERSION to public testnet's current Platform protocol version",
        )
        .parse()
        .unwrap();
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Sink),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(session.start_spv()).unwrap();

    let sync_deadline = Instant::now() + Duration::from_secs(900);
    loop {
        let sync = session.sync_snapshot().unwrap();
        if sync.caught_up && session.quorum_source().unwrap() == QuorumSource::Spv {
            break;
        }
        assert!(
            Instant::now() < sync_deadline,
            "testnet SPV sync timed out: {sync:?}"
        );
        thread::sleep(Duration::from_secs(2));
    }

    let refresh_deadline = Instant::now() + Duration::from_secs(90);
    let observed = loop {
        let version = session.protocol_compatibility().unwrap().network_version;
        if version.is_some() || Instant::now() >= refresh_deadline {
            break version;
        }
        thread::sleep(Duration::from_secs(1));
    };
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(observed, Some(expected));
    eprintln!("SPV-verified testnet epoch reported protocol version {expected}");
}
