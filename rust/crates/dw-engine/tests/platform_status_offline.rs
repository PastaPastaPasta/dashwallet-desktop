//! `platform_status` against a loopback DAPI endpoint nothing listens on,
//! driven from a runtime that is not the engine's.

use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, SessionOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn engine(root: &std::path::Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap()
}

fn opts() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

#[test]
fn platform_status_reports_an_unreachable_dapi_and_a_closed_session() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts()))
        .unwrap();

    // A current-thread runtime without a reactor: the call must hop onto the
    // engine runtime for the SDK's networking to work at all.
    let bare = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let err = bare.block_on(session.platform_status()).unwrap_err();
    assert!(matches!(err, EngineError::Sdk(_)), "{err:?}");

    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let err = bare.block_on(session.platform_status()).unwrap_err();
    assert!(matches!(err, EngineError::NetworkNotOpen(_)), "{err:?}");
}
