//! Offline engine tests: real PlatformWalletManager + SqlitePersister in a temp
//! dir, regtest/devnet with loopback endpoints that nothing listens on.

use std::sync::{Arc, Mutex};

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, SessionOptions,
    WalletBalances,
};
use zeroize::Zeroizing;

/// BIP39 test vector phrase (all-zero entropy).
const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

#[derive(Default)]
struct Recorder(Mutex<Vec<EngineEvent>>);

impl EventSink for Recorder {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Recorder {
    fn events(&self) -> Vec<EngineEvent> {
        self.0.lock().unwrap().clone()
    }
}

fn local_opts() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
    }
}

fn new_engine(root: &std::path::Path, sink: Arc<Recorder>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
        },
        sink,
    )
    .unwrap()
}

#[test]
fn created_wallet_survives_engine_restart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());

    let engine = new_engine(&root, Arc::clone(&rec));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    assert!(root.join("regtest").join("wallet.sqlite").exists());

    let created = engine.block_on(session.create_wallet(12)).unwrap();
    assert_eq!(created.mnemonic.split_whitespace().count(), 12);

    let wallets = session.list_wallets().unwrap();
    assert_eq!(wallets.len(), 1);
    assert_eq!(wallets[0].wallet_id, created.wallet_id);
    assert_eq!(wallets[0].balances, WalletBalances::default());
    assert_eq!(
        session.balances(&created.wallet_id).unwrap(),
        WalletBalances::default()
    );

    let events = rec.events();
    assert!(events.contains(&EngineEvent::SessionOpened {
        network: DashNetwork::Regtest
    }));
    assert!(events.contains(&EngineEvent::WalletCreated {
        network: DashNetwork::Regtest,
        wallet_id: created.wallet_id
    }));

    engine.block_on(engine.shutdown()).unwrap();
    assert!(!session.is_open());
    assert!(matches!(
        session.list_wallets(),
        Err(EngineError::NetworkNotOpen(_))
    ));
    assert!(rec.events().contains(&EngineEvent::SessionClosed {
        network: DashNetwork::Regtest
    }));
    drop(session);
    drop(engine);

    let engine = new_engine(&root, Arc::new(Recorder::default()));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    let ids: Vec<_> = session
        .list_wallets()
        .unwrap()
        .into_iter()
        .map(|w| w.wallet_id)
        .collect();
    assert_eq!(ids, vec![created.wallet_id]);
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn imported_wallet_id_is_deterministic_and_network_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));

    let regtest = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    let a = engine
        .block_on(regtest.import_wallet(Zeroizing::new(ABANDON_12.into()), Some(0)))
        .unwrap();
    // Registering the same wallet twice is refused by platform-wallet.
    let dup = engine.block_on(regtest.import_wallet(Zeroizing::new(ABANDON_12.into()), Some(0)));
    assert!(matches!(dup, Err(EngineError::Wallet(_))), "{dup:?}");

    let devnet = DashNetwork::Devnet {
        name: "dwtest".into(),
    };
    let dev = engine
        .block_on(engine.open_network(devnet.clone(), local_opts()))
        .unwrap();
    assert!(
        dir.path()
            .join("devnet-dwtest")
            .join("wallet.sqlite")
            .exists()
    );
    let b = engine
        .block_on(dev.import_wallet(Zeroizing::new(ABANDON_12.into()), Some(0)))
        .unwrap();
    assert_ne!(a, b, "same mnemonic must yield distinct ids per network");

    // Close and reopen the regtest session inside the same engine: the
    // persister claim is released by close, and the id is unchanged.
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    assert!(
        !engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let regtest = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    let ids: Vec<_> = regtest
        .list_wallets()
        .unwrap()
        .into_iter()
        .map(|w| w.wallet_id)
        .collect();
    assert_eq!(ids, vec![a]);
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn dropping_engine_without_shutdown_releases_storage() {
    let dir = tempfile::tempdir().unwrap();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    let id = engine
        .block_on(s.import_wallet(Zeroizing::new(ABANDON_12.into()), Some(0)))
        .unwrap();
    drop(s);
    drop(engine);

    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    assert_eq!(
        s.list_wallets()
            .unwrap()
            .into_iter()
            .map(|w| w.wallet_id)
            .collect::<Vec<_>>(),
        vec![id]
    );
}

#[test]
fn second_engine_on_same_data_dir_gets_storage_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let first = new_engine(dir.path(), Arc::new(Recorder::default()));
    let _s = first
        .block_on(first.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();

    let second = new_engine(dir.path(), Arc::new(Recorder::default()));
    let err = second
        .block_on(second.open_network(DashNetwork::Regtest, local_opts()))
        .err();
    assert!(matches!(err, Some(EngineError::StorageInUse(_))), "{err:?}");
    first.block_on(first.shutdown()).unwrap();
}

#[test]
fn rejects_bad_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    assert!(matches!(
        engine.block_on(s.create_wallet(13)),
        Err(EngineError::InvalidArgument(_))
    ));
    let bad = engine.block_on(s.import_wallet(Zeroizing::new("not a mnemonic".into()), None));
    assert!(matches!(bad, Err(EngineError::Wallet(_))), "{bad:?}");
    assert!(matches!(
        s.balances(&"00".repeat(32).parse().unwrap()),
        Err(EngineError::WalletNotFound(_))
    ));
    // Regtest has no default DAPI endpoints.
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let err = engine
        .block_on(engine.open_network(DashNetwork::Regtest, SessionOptions::default()))
        .err();
    assert!(
        matches!(err, Some(EngineError::InvalidConfig(_))),
        "{err:?}"
    );
    let err = engine
        .block_on(engine.open_network(
            DashNetwork::Devnet {
                name: "../etc".into(),
            },
            local_opts(),
        ))
        .err();
    assert!(
        matches!(err, Some(EngineError::InvalidArgument(_))),
        "{err:?}"
    );
}

#[test]
fn spv_starts_and_stops_without_reachable_peers() {
    let dir = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(dir.path(), Arc::clone(&rec));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    assert!(!s.spv_running().unwrap());
    engine.block_on(s.start_spv()).unwrap();
    assert!(s.spv_running().unwrap());
    engine.block_on(s.stop_spv()).unwrap();
    assert!(!s.spv_running().unwrap());
    let events = rec.events();
    assert!(events.contains(&EngineEvent::SpvStateChanged {
        network: DashNetwork::Regtest,
        running: true
    }));
    assert!(events.contains(&EngineEvent::SpvStateChanged {
        network: DashNetwork::Regtest,
        running: false
    }));
    engine.block_on(engine.shutdown()).unwrap();
}
