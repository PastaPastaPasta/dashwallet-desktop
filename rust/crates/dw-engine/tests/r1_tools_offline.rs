//! Offline tests of the M2 R1 calls (docs/contracts/m2-engine.md §2.1–2.4):
//! wallet load/unload and the load-on-startup list, watch-only wallets,
//! account xpubs, the data inventory, Tools information and repair.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, HistoryFilter,
    HistorySort, ImportOptions, NetworkSession, SessionOptions, WalletId, WarningCode,
    WatchOnlyOptions,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
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
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let xprv = master
        .derive_priv(&DerivationPath::from_str("m/44'/1'/0'").unwrap())
        .unwrap();
    ExtendedPubKey::from_priv(&xprv).to_string()
}

fn address_at(phrase: &str, path: &str) -> String {
    let secret = dw_vault::mnemonic::derive_secret(phrase.as_bytes(), b"", false).unwrap();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let child = master
        .derive_priv(&DerivationPath::from_str(path).unwrap())
        .unwrap();
    let pubkey = dashcore::PublicKey::new(child.private_key.public_key());
    dashcore::Address::p2pkh(&pubkey, dashcore::Network::Regtest).to_string()
}

fn loaded_ids(s: &NetworkSession) -> Vec<WalletId> {
    s.wallet_infos()
        .unwrap()
        .into_iter()
        .map(|w| w.wallet_id)
        .collect()
}

const ZOO_12: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";

fn restart(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    open(engine)
}

fn on_startup(s: &NetworkSession) -> Vec<(WalletId, bool)> {
    (s.wallet_load_states().unwrap().into_iter())
        .map(|w| (w.wallet_id, w.load_on_startup))
        .collect()
}

/// #87: a wallet created, imported or added watch-only after a Close
/// Wallet is on the load-on-startup list, and opens at the next start.
#[test]
fn a_wallet_added_after_a_close_opens_at_the_next_start() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    engine.block_on(s.unload_wallet(b)).unwrap();
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();

    let created = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    let watched = engine
        .block_on(s.import_watch_only(account_tpub(ZOO_12), WatchOnlyOptions::default()))
        .unwrap();
    let listed = |s: &NetworkSession, id: WalletId| on_startup(s).contains(&(id, true));
    assert!(listed(&s, created) && listed(&s, watched));

    let s = restart(&engine);
    let mut open_now = loaded_ids(&s);
    open_now.sort();
    let mut expected = vec![a, created, watched];
    expected.sort();
    assert_eq!(open_now, expected);
    assert!(on_startup(&s).contains(&(b, false)));
    engine.block_on(engine.shutdown()).unwrap();
}

/// #113: the last wallet on the list cannot be taken off it
/// (`LastStartupWallet`, nothing changed): a start with no wallet open would
/// leave the closed ones out of reach.
#[test]
fn the_last_listed_wallet_stays_listed() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    engine.block_on(s.unload_wallet(b)).unwrap();
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();
    assert!(matches!(
        engine.block_on(s.set_load_on_startup(a, false)),
        Err(EngineError::LastStartupWallet)
    ));
    assert_eq!(on_startup(&s), vec![(a, true), (b, false)]);

    let s = restart(&engine);
    assert_eq!(on_startup(&s), vec![(a, true), (b, false)]);
    assert_eq!(loaded_ids(&s), vec![a]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// #113: with no list yet every wallet is listed, so the lone wallet cannot
/// be taken off either.
#[test]
fn a_lone_wallet_stays_listed() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(
        &dir.path().join("data"),
        Arc::default(),
        Arc::new(MemoryOsStore::new()),
    );
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    assert!(matches!(
        engine.block_on(s.set_load_on_startup(a, false)),
        Err(EngineError::LastStartupWallet)
    ));
    assert_eq!(on_startup(&s), vec![(a, true)]);
    engine.block_on(engine.shutdown()).unwrap();
}

fn remove(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) {
    let wipe = engine
        .block_on(
            s.vault_op(move |v| v.authorize(GrantPurpose::Wipe, Some(&id.0), Credential::None)),
        )
        .unwrap();
    engine.block_on(s.remove_wallet(id, wipe.id)).unwrap();
}

fn stored_list(root: &std::path::Path) -> Option<String> {
    let db = dw_appdb::AppDb::open(&root.join("regtest").join(dw_appdb::APP_DB_FILE)).unwrap();
    db.setting(dw_appdb::GLOBAL_SCOPE, "load_on_startup")
        .unwrap()
}

/// #113: removing the only listed wallet lists the first open one, the
/// wallet the app shows next, in the removal's own write; removing the last
/// wallet leaves nothing listed.
#[test]
fn removing_the_only_listed_wallet_lists_the_next_open_one() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    let c = import(&engine, &s, ZOO_12);
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();
    engine.block_on(s.set_load_on_startup(c, false)).unwrap();

    remove(&engine, &s, a);
    assert_eq!(on_startup(&s), vec![(b, true), (c, false)]);
    let s = restart(&engine);
    assert_eq!(loaded_ids(&s), vec![b]);
    assert_eq!(on_startup(&s), vec![(b, true), (c, false)]);

    engine.block_on(s.load_wallet(c)).unwrap();
    remove(&engine, &s, b);
    assert_eq!(on_startup(&s), vec![(c, true)]);
    remove(&engine, &s, c);
    assert!(on_startup(&s).is_empty());
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(stored_list(&root).as_deref(), Some("none"));

    let s = open(&engine);
    assert!(loaded_ids(&s).is_empty());
    let d = import(&engine, &s, ABANDON_12);
    assert_eq!(on_startup(&s), vec![(d, true)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// #113: removing the last wallet while no list was ever stored stores
/// "nothing to load": the next wallet added is the only one listed.
#[test]
fn removing_the_last_wallet_with_no_list_stores_nothing_to_load() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    remove(&engine, &s, a);
    assert_eq!(on_startup(&s), vec![(b, true)]);
    remove(&engine, &s, b);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(stored_list(&root).as_deref(), Some("none"));

    let s = open(&engine);
    let c = import(&engine, &s, ZOO_12);
    assert_eq!(on_startup(&s), vec![(c, true)]);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(stored_list(&root), Some(c.to_string()));
    engine.block_on(engine.shutdown()).unwrap();
}

/// #113: removing a listed wallet while another stays listed lists no
/// other wallet.
#[test]
fn removing_one_of_two_listed_wallets_lists_no_other() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    let c = import(&engine, &s, ZOO_12);
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();

    remove(&engine, &s, a);
    assert_eq!(on_startup(&s), vec![(b, false), (c, true)]);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(stored_list(&root), Some(c.to_string()));
    engine.block_on(engine.shutdown()).unwrap();
}

/// #113: a stored list that names no wallet of this network (an older
/// build's, a hand-edited one, "nothing to load") opens every wallet, and
/// is stored again with every wallet on it, so Settings shows what the
/// start did.
#[test]
fn a_list_naming_no_wallet_opens_every_one_and_is_repaired() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    let mut every = [a.to_string(), b.to_string()];
    every.sort();
    for stale in [
        WalletId([9; 32]).to_string(),
        "none".to_string(),
        String::new(),
    ] {
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
        let db = dw_appdb::AppDb::open(&root.join("regtest").join(dw_appdb::APP_DB_FILE)).unwrap();
        db.set_setting(dw_appdb::GLOBAL_SCOPE, "load_on_startup", Some(&stale))
            .unwrap();
        drop(db);

        let s = open(&engine);
        assert_eq!(loaded_ids(&s), vec![a, b], "{stale}");
        assert_eq!(on_startup(&s), vec![(a, true), (b, true)], "{stale}");
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap();
        assert_eq!(stored_list(&root), Some(every.join(",")), "{stale}");
        let s = open(&engine);
        engine.block_on(s.set_load_on_startup(a, false)).unwrap();
        engine.block_on(s.set_load_on_startup(a, true)).unwrap();
    }
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(stored_list(&root), Some(every.join(",")));
    let s = open(&engine);
    assert_eq!(loaded_ids(&s), vec![a, b]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// #87, #113: an empty list an earlier build stored opens every wallet, and
/// is stored again with every wallet on it.
#[test]
fn an_empty_list_from_an_earlier_build_loads_every_wallet() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let engine = new_engine(&root, Arc::default(), Arc::new(MemoryOsStore::new()));
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let a = import(&engine, &s, ABANDON_12);
    let b = import(&engine, &s, LEGAL_12);
    engine.block_on(s.set_load_on_startup(b, false)).unwrap();
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let db = dw_appdb::AppDb::open(&root.join("regtest").join(dw_appdb::APP_DB_FILE)).unwrap();
    db.set_setting(dw_appdb::GLOBAL_SCOPE, "load_on_startup", Some(""))
        .unwrap();
    drop(db);

    let s = open(&engine);
    assert_eq!(loaded_ids(&s), vec![a, b]);
    assert_eq!(on_startup(&s), vec![(a, true), (b, true)]);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let mut every = [a.to_string(), b.to_string()];
    every.sort();
    assert_eq!(stored_list(&root), Some(every.join(",")));
    engine.block_on(engine.shutdown()).unwrap();
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

/// DEC-210: a watch-only wallet imported again after its removal, in the
/// same run, is written as any wallet: its new name survives a reopen.
#[test]
fn a_watch_only_wallet_imported_again_after_its_removal_keeps_its_name() {
    let dir = dw_testutil::private_tempdir();
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(
        &dir.path().join("data"),
        rec,
        Arc::new(MemoryOsStore::new()),
    );
    let s = open(&engine);
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let tpub = account_tpub(ABANDON_12);
    let named = |name: &str| WatchOnlyOptions {
        name: Some(name.into()),
        birth_height: Some(0),
        lookahead: None,
    };
    let id = engine
        .block_on(s.import_watch_only(tpub.clone(), named("Cold")))
        .unwrap();
    let wipe = engine
        .block_on(
            s.vault_op(move |v| v.authorize(GrantPurpose::Wipe, Some(&id.0), Credential::None)),
        )
        .unwrap();
    engine.block_on(s.remove_wallet(id, wipe.id)).unwrap();
    let again = engine
        .block_on(s.import_watch_only(tpub, named("Again")))
        .unwrap();
    assert_eq!(again, id);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    let s = open(&engine);
    assert_eq!(s.wallet_info(&id).unwrap().name, "Again");
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
    let txid = dashcore::Txid::from_byte_array([1; 32]);
    let r = engine.block_on(s.abandon_transaction(id, txid));
    assert!(matches!(r, Err(EngineError::SpvNotRunning)), "{r:?}");
    let other = WalletId([9; 32]);
    let r = engine.block_on(s.abandon_transaction(other, txid));
    assert!(matches!(r, Err(EngineError::WalletNotFound(_))), "{r:?}");
}
