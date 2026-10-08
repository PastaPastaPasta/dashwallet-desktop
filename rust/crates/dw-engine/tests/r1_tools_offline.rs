//! Offline tests of the M2 R1 calls (docs/contracts/m2-engine.md §2.1–2.4):
//! wallet load/unload and the load-on-startup list, watch-only wallets,
//! account xpubs, the data inventory, Tools information and repair.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dashcore::secp256k1::Secp256k1;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, HistoryFilter,
    HistorySort, ImportOptions, NetworkSession, SessionOptions, WalletId, WarningCode,
    WatchOnlyOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use zeroize::Zeroizing;

const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const LEGAL_12: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";

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
        ..Default::default()
    }
}

fn new_engine(root: &std::path::Path, sink: Arc<Recorder>, os: Arc<MemoryOsStore>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: os,
                ..VaultConfig::default()
            },
        },
        sink,
    )
    .unwrap()
}

fn open(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap()
}

fn import(engine: &Engine, s: &Arc<NetworkSession>, phrase: &str) -> WalletId {
    engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap()
}

/// The BIP44 account-0 tpub of `phrase` on regtest.
fn account_tpub(phrase: &str) -> String {
    let secret = dw_vault::mnemonic::derive_secret(phrase.as_bytes(), b"", false).unwrap();
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let xprv = master
        .derive_priv(&secp, &DerivationPath::from_str("m/44'/1'/0'").unwrap())
        .unwrap();
    ExtendedPubKey::from_priv(&secp, &xprv).to_string()
}

fn address_at(phrase: &str, path: &str) -> String {
    let secret = dw_vault::mnemonic::derive_secret(phrase.as_bytes(), b"", false).unwrap();
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let child = master
        .derive_priv(&secp, &DerivationPath::from_str(path).unwrap())
        .unwrap();
    let pubkey = dashcore::PublicKey::new(child.private_key.public_key(&secp));
    dashcore::Address::p2pkh(&pubkey, dashcore::Network::Regtest).to_string()
}

fn loaded_ids(s: &NetworkSession) -> Vec<WalletId> {
    s.wallet_infos()
        .unwrap()
        .into_iter()
        .map(|w| w.wallet_id)
        .collect()
}

#[test]
fn test_qt_101_close_open_and_load_on_startup() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let os = Arc::new(MemoryOsStore::new());
    let engine = new_engine(&root, Arc::clone(&rec), Arc::clone(&os));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    assert_eq!(loaded_ids(&s), vec![a, b]);

    // Close B: gone from wallet_infos and wallet-scoped calls, listed as
    // unloaded with its name.
    engine.block_on(s.unload_wallet(b)).unwrap();
    engine.block_on(s.unload_wallet(b)).unwrap(); // idempotent
    assert_eq!(loaded_ids(&s), vec![a]);
    assert!(matches!(
        s.wallet_info(&b),
        Err(EngineError::WalletNotFound(_))
    ));
    let states = s.wallet_load_states().unwrap();
    assert_eq!(states.len(), 2);
    assert!(states[0].loaded && !states[1].loaded);
    assert_eq!(states[1].wallet_id, b);
    assert_eq!(states[1].name, "Wallet 2");
    assert!(
        states.iter().all(|w| w.load_on_startup),
        "no list yet = all"
    );
    assert!(rec.events().contains(&EngineEvent::WalletLoadChanged {
        network: DashNetwork::Regtest,
        wallet_id: b,
        loaded: false,
    }));

    // Open it again: back, with its history store reloaded.
    engine.block_on(s.load_wallet(b)).unwrap();
    engine.block_on(s.load_wallet(b)).unwrap(); // idempotent
    assert_eq!(loaded_ids(&s), vec![a, b]);
    assert!(rec.events().contains(&EngineEvent::WalletLoadChanged {
        network: DashNetwork::Regtest,
        wallet_id: b,
        loaded: true,
    }));
    assert!(matches!(
        engine.block_on(s.load_wallet(WalletId([9; 32]))),
        Err(EngineError::WalletNotFound(_))
    ));

    // dash-qt Close removes from the startup list: a restart loads only A.
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();
    let states = s.wallet_load_states().unwrap();
    assert!(states[0].load_on_startup && !states[1].load_on_startup);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let s = open(&engine);
    assert_eq!(loaded_ids(&s), vec![a]);
    let states = s.wallet_load_states().unwrap();
    assert_eq!(states.len(), 2);
    assert!(!states[1].loaded && states[1].wallet_id == b);
    engine.block_on(s.load_wallet(b)).unwrap();
    assert_eq!(loaded_ids(&s), vec![a, b]);
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn test_qt_114_watch_only_wallet_from_an_account_xpub() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let os = Arc::new(MemoryOsStore::new());
    let engine = new_engine(&root, Arc::clone(&rec), Arc::clone(&os));
    let s = open(&engine);
    let tpub = account_tpub(ABANDON_12);

    // No vault is needed: a watch-only wallet has no secret.
    let id = engine
        .block_on(s.import_watch_only(
            tpub.clone(),
            WatchOnlyOptions {
                name: Some("Cold".into()),
                birth_height: Some(0),
                lookahead: None,
            },
        ))
        .unwrap();
    let info = s.wallet_info(&id).unwrap();
    assert!(info.watch_only && !info.has_mnemonic);
    assert_eq!(info.name, "Cold");
    assert!(matches!(
        engine.block_on(s.import_watch_only(tpub.clone(), WatchOnlyOptions::default())),
        Err(EngineError::WalletAlreadyExists(_))
    ));
    assert!(matches!(
        engine.block_on(s.import_watch_only("xpub-nonsense".into(), WatchOnlyOptions::default())),
        Err(EngineError::InvalidXpub(_))
    ));

    // IOS-111: the account key comes back; the first receive address is
    // the seed wallet's m/44'/1'/0'/0/0.
    let x = engine.block_on(s.account_xpub(id, 0)).unwrap();
    assert_eq!(x.xpub, tpub);
    assert_eq!(x.derivation_path, "m/44'/1'/0'");
    assert!(matches!(
        engine.block_on(s.account_xpub(id, 5)),
        Err(EngineError::InvalidArgument(_))
    ));

    // It watches the seed wallet's addresses.
    let first = engine.block_on(s.current_receive_address(id)).unwrap();
    assert_eq!(first.derivation_path, "m/44'/1'/0'/0/0");
    assert_eq!(first.address, address_at(ABANDON_12, "m/44'/1'/0'/0/0"));

    // Survives a restart (rebuilt from wallet.sqlite like any wallet).
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let s = open(&engine);
    let info = s.wallet_info(&id).unwrap();
    assert!(info.watch_only);
    assert_eq!(engine.block_on(s.account_xpub(id, 0)).unwrap().xpub, tpub);

    // The CSV of a watch-only wallet has the Watch-only column.
    let csv = engine
        .block_on(s.export_history_csv(
            id,
            HistoryFilter::default(),
            HistorySort::NewestFirst,
            dw_units::Unit::Dash,
            Vec::new(),
            0,
        ))
        .unwrap();
    assert_eq!(
        csv,
        "\"Confirmed\",\"Watch-only\",\"Date\",\"Type\",\"Label\",\"Address\",\"Amount (tDASH)\",\"ID\"\n"
    );
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn test_ios_111_seed_wallet_account_xpub_matches_derivation() {
    let dir = dw_testutil::private_tempdir();
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(&dir.path().join("d"), rec, Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let x = engine.block_on(s.account_xpub(a, 0)).unwrap();
    assert_eq!(x.xpub, account_tpub(ABANDON_12));
    assert!(x.xpub.starts_with("tpub"));
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn test_ios_009_existing_networks_reads_the_data_root() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let os = Arc::new(MemoryOsStore::new());
    let engine = new_engine(&root, rec, Arc::clone(&os));
    assert!(
        engine
            .block_on(engine.existing_networks())
            .unwrap()
            .is_empty()
    );
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let nets = engine.block_on(engine.existing_networks()).unwrap();
    assert_eq!(nets.len(), 1);
    assert_eq!(nets[0].network, DashNetwork::Regtest);
    assert!(nets[0].has_wallet_state && nets[0].has_vault);
    assert!(
        nets[0].has_os_store_key,
        "an unencrypted vault keeps its key in slot O"
    );
    // A stray directory that is not a network is ignored.
    std::fs::create_dir_all(root.join("not-a-network")).unwrap();
    assert_eq!(
        engine.block_on(engine.existing_networks()).unwrap().len(),
        1
    );
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn test_qt_143_qt_148_information_warnings_and_repair() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(&root, rec, Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    let info = s.node_info().unwrap();
    assert_eq!(info.network, DashNetwork::Regtest);
    assert!(info.user_agent.starts_with("/dashwallet-desktop:"));
    assert_eq!(info.connections_in, 0);
    assert!(info.local_addresses.is_empty());
    assert_eq!(
        info.mempool_tx_count, None,
        "SPV never reports mempool data"
    );
    assert_eq!(info.mempool_usage_bytes, None);
    assert_eq!(info.masternodes, None, "no masternode list synced");
    assert!(info.startup_time > 0);
    let codes: Vec<WarningCode> = s.warnings().unwrap().into_iter().map(|w| w.code).collect();
    assert!(codes.contains(&WarningCode::PrereleaseBuild));
    assert!(!codes.contains(&WarningCode::UncleanShutdown));
    assert_eq!(s.rescan_progress().unwrap(), None);
    assert!(!engine.block_on(s.cancel_rescan()).unwrap());

    // QT-148: chain data goes, wallets stay.
    std::fs::create_dir_all(s.data_dir().join("spv").join("headers")).unwrap();
    engine.block_on(s.reset_chain_data()).unwrap();
    assert!(!s.data_dir().join("spv").exists());

    // A session dropped without close leaves its marker: the next open
    // reports an unclean shutdown.
    let marker = s.data_dir().join(".session-open");
    assert!(marker.exists());
    engine.block_on(engine.shutdown()).unwrap();
    assert!(!marker.exists(), "a clean close removes the marker");
    std::fs::write(&marker, b"").unwrap();
    let s = open(&engine);
    let codes: Vec<WarningCode> = s.warnings().unwrap().into_iter().map(|w| w.code).collect();
    assert_eq!(codes.first(), Some(&WarningCode::UncleanShutdown));
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn test_ios_113_birth_height_is_stored_and_survives_a_restart() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let os = Arc::new(MemoryOsStore::new());
    let engine = new_engine(&root, rec, Arc::clone(&os));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    engine.block_on(s.set_birth_height(a, 1234)).unwrap();
    assert_eq!(s.wallet_info(&a).unwrap().birth_height, Some(1234));
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let s = open(&engine);
    assert_eq!(s.wallet_info(&a).unwrap().birth_height, Some(1234));
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review M2 (QT-091): abandoning needs a running SPV client, because the
/// spent coins only come back through its filter rescan. Offline it is
/// refused before anything changes.
#[test]
fn test_qt_091_abandon_needs_a_running_spv_client() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(
        &dir.path().join("data"),
        Arc::new(Recorder::default()),
        Arc::new(MemoryOsStore::new()),
    );
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let id = import(&engine, &s, ABANDON_12);
    let txid = <dashcore::Txid as dashcore::hashes::Hash>::from_byte_array([1; 32]);
    let r = engine.block_on(s.abandon_transaction(id, txid));
    assert!(matches!(r, Err(EngineError::SpvNotRunning)), "{r:?}");
    let other = WalletId([9; 32]);
    let r = engine.block_on(s.abandon_transaction(other, txid));
    assert!(matches!(r, Err(EngineError::WalletNotFound(_))), "{r:?}");
}
