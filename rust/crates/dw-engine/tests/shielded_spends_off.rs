//! The shipped engine spends from Shielded on testnet only: built as the app
//! links it (not `cfg(test)`, so with no test override of the switch),
//! mainnet's and regtest's status and sync read report `spends_available`
//! false and their receive address and spend quotes are refused, while
//! testnet reports spends available and issues the receive address.

use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, SessionOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

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
fn shielded_spends_are_off_on_mainnet_until_their_funds_review() {
    for network in [
        DashNetwork::Mainnet,
        DashNetwork::Testnet,
        DashNetwork::Regtest,
    ] {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(&dir.path().join("data"));
        let session = engine
            .block_on(engine.open_network(network.clone(), opts()))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        let wallet = engine
            .block_on(session.import_wallet(
                Zeroizing::new(PHRASE.to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions::default(),
            ))
            .unwrap();
        let dp = session.dashpay(wallet);
        let on = network == DashNetwork::Testnet;
        let info = engine.block_on(dp.shielded_sync_info()).unwrap();
        assert_eq!(info.spends_available, on, "{network:?}: sync read");
        let status = engine.block_on(dp.shielded_status()).unwrap();
        assert_eq!(status.spends_available, on, "{network:?}: status");
        if on {
            let address = engine.block_on(dp.shielded_receive_address());
            assert!(address.is_ok(), "{network:?}: receive address");
            engine.block_on(engine.close_network(network)).unwrap();
            continue;
        }
        let address = engine.block_on(dp.shielded_receive_address());
        assert_eq!(
            address.unwrap_err().code(),
            "platform.unavailable",
            "{network:?}: no receive address"
        );
        let quotes = [
            engine.block_on(dp.shielded_send_quote("tdash1z".into(), 1)),
            engine.block_on(dp.unshield_quote(1, None)),
        ];
        for quote in quotes {
            assert_eq!(
                quote.unwrap_err().code(),
                "platform.unavailable",
                "{network:?}: no quote"
            );
        }
        engine.block_on(engine.close_network(network)).unwrap();
    }
}
