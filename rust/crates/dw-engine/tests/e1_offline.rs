//! Offline tests of the E1 calls: wallet registry (names, rename, remove),
//! receive addresses and requests, history queries, sync state, and the
//! session lifecycle rules (M1 close admission, M7 directory modes, events).

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::secp256k1::Secp256k1;
use dw_engine::{
    AddressChain, AddressFilter, DashNetwork, Engine, EngineConfig, EngineError, EngineEvent,
    EventSink, HistoryFilter, HistoryQuery, HistorySort, ImportOptions, NetworkSession, RescanFrom,
    SessionOptions, WalletId,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
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
    }
}

fn new_engine(root: &std::path::Path, sink: Arc<Recorder>) -> Engine {
    new_engine_with(root, sink, Arc::new(MemoryOsStore::new()))
}

/// `os_store` stands in for the OS keyring; share it to model a restart.
fn new_engine_with(
    root: &std::path::Path,
    sink: Arc<Recorder>,
    os_store: Arc<MemoryOsStore>,
) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store,
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

fn unencrypted_vault(engine: &Engine, s: &Arc<NetworkSession>) {
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
}

fn import(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    phrase: &str,
    options: ImportOptions,
) -> Result<WalletId, EngineError> {
    engine.block_on(s.import_wallet(
        Zeroizing::new(phrase.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        options,
    ))
}

fn genesis() -> ImportOptions {
    ImportOptions {
        birth_height: Some(0),
        ..ImportOptions::default()
    }
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

#[test]
fn names_rename_and_remove() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());
    let store = Arc::new(MemoryOsStore::new());
    let engine = new_engine_with(&root, Arc::clone(&rec), Arc::clone(&store));
    let s = open(&engine);
    unencrypted_vault(&engine, &s);

    let a = import(&engine, &s, ABANDON_12, genesis()).unwrap();
    let b = import(
        &engine,
        &s,
        LEGAL_12,
        ImportOptions {
            name: Some("  Savings  ".into()),
            ..genesis()
        },
    )
    .unwrap();
    let rejected = import(
        &engine,
        &s,
        LEGAL_12,
        ImportOptions {
            name: Some(" ".into()),
            ..genesis()
        },
    );
    assert!(
        matches!(rejected, Err(EngineError::NameRejected(_))),
        "{rejected:?}"
    );

    let infos = s.wallet_infos().unwrap();
    assert_eq!(
        infos
            .iter()
            .map(|i| (i.wallet_id, i.name.as_str()))
            .collect::<Vec<_>>(),
        vec![(a, "Wallet 1"), (b, "Savings")]
    );
    assert!(infos.iter().all(|i| i.hd && i.created_at.is_some()));
    assert_eq!(infos[0].birth_height, Some(0));
    assert_eq!(s.wallet_info(&b).unwrap().name, "Savings");

    engine
        .block_on(s.rename_wallet(a, "Spending".into()))
        .unwrap();
    assert!(matches!(
        engine.block_on(s.rename_wallet(a, "x".repeat(65))),
        Err(EngineError::NameRejected(_))
    ));
    let unknown: WalletId = "00".repeat(32).parse().unwrap();
    assert!(matches!(
        engine.block_on(s.rename_wallet(unknown, "x".into())),
        Err(EngineError::WalletNotFound(_))
    ));
    assert_eq!(s.wallet_info(&a).unwrap().name, "Spending");

    // Remove needs a Wipe grant.
    let wrong = engine
        .block_on(s.vault_op(move |v| {
            v.authorize(GrantPurpose::SignMessage, Some(&b.0), Credential::None)
        }))
        .unwrap();
    assert!(matches!(
        engine.block_on(s.remove_wallet(b, wrong.id)),
        Err(EngineError::Vault(
            dw_vault::VaultError::GrantPurposeMismatch
        ))
    ));
    assert!(matches!(
        engine.block_on(s.remove_wallet(b, "nope".into())),
        Err(EngineError::Vault(dw_vault::VaultError::GrantInvalid))
    ));
    let wipe = engine
        .block_on(
            s.vault_op(move |v| v.authorize(GrantPurpose::Wipe, Some(&b.0), Credential::None)),
        )
        .unwrap();
    engine.block_on(s.remove_wallet(b, wipe.id)).unwrap();
    assert!(!s.vault().has_wallet_secret(&b.0));
    assert_eq!(
        s.wallet_infos()
            .unwrap()
            .into_iter()
            .map(|i| i.wallet_id)
            .collect::<Vec<_>>(),
        vec![a]
    );
    assert!(rec.events().contains(&EngineEvent::WalletRemoved {
        network: DashNetwork::Regtest,
        wallet_id: b
    }));
    engine.block_on(engine.shutdown()).unwrap();
    drop(s);
    drop(engine);

    // After a restart the removal and the rename are still in effect.
    let engine = new_engine_with(&root, Arc::new(Recorder::default()), store);
    let s = open(&engine);
    let infos = s.wallet_infos().unwrap();
    assert_eq!(infos.len(), 1);
    assert_eq!(
        (infos[0].wallet_id, infos[0].name.as_str()),
        (a, "Spending")
    );
    // The removed wallet can be imported again and gets a fresh default name.
    let again = import(&engine, &s, LEGAL_12, genesis()).unwrap();
    assert_eq!(again, b);
    assert_eq!(s.wallet_info(&b).unwrap().name, "Wallet 2");
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn receive_addresses_requests_and_lookahead() {
    let dir = tempfile::tempdir().unwrap();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = open(&engine);
    unencrypted_vault(&engine, &s);
    let id = import(&engine, &s, ABANDON_12, genesis()).unwrap();

    let current = engine.block_on(s.current_receive_address(id)).unwrap();
    assert_eq!(current.address, address_at(ABANDON_12, "m/44'/1'/0'/0/0"));
    assert_eq!(current.derivation_path, "m/44'/1'/0'/0/0");
    assert_eq!(
        (current.chain, current.index, current.used),
        (AddressChain::Receiving, 0, false)
    );
    assert_eq!(current.balance, Some(0));

    let first = engine
        .block_on(s.next_receive_address(id, Some("Shop".into())))
        .unwrap();
    assert_eq!(first.index, 0);
    assert_eq!(first.label.as_deref(), Some("Shop"));
    let second = engine.block_on(s.next_receive_address(id, None)).unwrap();
    assert_eq!(second.index, 1, "an issued address is never issued again");

    let request = engine
        .block_on(s.create_receive_request(
            id,
            Some(150_000_000),
            Some("Rent".into()),
            Some("May".into()),
        ))
        .unwrap();
    assert_eq!(request.address, address_at(ABANDON_12, "m/44'/1'/0'/0/2"));
    assert_eq!(
        request.uri,
        format!(
            "dash:{}?amount=1.50000000&label=Rent&message=May",
            request.address
        )
    );
    let any = engine
        .block_on(s.create_receive_request(id, Some(0), None, None))
        .unwrap();
    assert_eq!(any.amount, None, "0 means any amount");
    let listed = engine.block_on(s.receive_requests(id)).unwrap();
    assert_eq!(
        listed.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![any.id, request.id]
    );
    engine
        .block_on(s.delete_receive_request(id, any.id))
        .unwrap();
    assert!(matches!(
        engine.block_on(s.delete_receive_request(id, any.id)),
        Err(EngineError::RequestNotFound(_))
    ));

    let all = engine
        .block_on(s.addresses(id, AddressFilter::default()))
        .unwrap();
    let receiving = all
        .iter()
        .filter(|a| a.chain == AddressChain::Receiving)
        .count();
    let change = all
        .iter()
        .filter(|a| a.chain == AddressChain::Change)
        .count();
    assert!(receiving >= 30 && change >= 30, "{receiving} {change}");
    let labelled = all
        .iter()
        .find(|a| a.index == 2 && a.chain == AddressChain::Receiving)
        .unwrap();
    assert_eq!(labelled.label.as_deref(), Some("Rent"));
    let only_change = engine
        .block_on(s.addresses(
            id,
            AddressFilter {
                chain: Some(AddressChain::Change),
                used: Some(false),
            },
        ))
        .unwrap();
    assert_eq!(only_change.len(), change);

    // Issuing every address inside the gap ends in GapLimit.
    let mut result = Ok(first);
    for _ in 0..receiving {
        result = engine.block_on(s.next_receive_address(id, None));
        if result.is_err() {
            break;
        }
    }
    assert!(matches!(result, Err(EngineError::GapLimit)), "{result:?}");

    // A Core-compatible restore scans 1000 addresses ahead.
    let restored = import(
        &engine,
        &s,
        LEGAL_12,
        ImportOptions {
            core_compat: true,
            ..genesis()
        },
    )
    .unwrap();
    let wide = engine
        .block_on(s.addresses(
            restored,
            AddressFilter {
                chain: Some(AddressChain::Receiving),
                used: None,
            },
        ))
        .unwrap();
    assert_eq!(wide.len(), 1000);
    assert!(matches!(
        import(
            &engine,
            &s,
            LEGAL_12,
            ImportOptions {
                lookahead: Some(1001),
                ..genesis()
            }
        ),
        Err(EngineError::InvalidArgument(_))
    ));
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn empty_history_and_query_errors() {
    let dir = tempfile::tempdir().unwrap();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = open(&engine);
    unencrypted_vault(&engine, &s);
    let id = import(&engine, &s, ABANDON_12, genesis()).unwrap();
    let query = HistoryQuery {
        filter: HistoryFilter::default(),
        sort: HistorySort::NewestFirst,
        cursor: None,
        limit: 50,
    };
    let page = engine.block_on(s.history_page(id, query.clone())).unwrap();
    assert!(page.records.is_empty());
    assert_eq!(page.total_matching, Some(0));
    assert_eq!(page.next_cursor, None);

    let bad_limit = HistoryQuery {
        limit: 0,
        ..query.clone()
    };
    assert!(matches!(
        engine.block_on(s.history_page(id, bad_limit)),
        Err(EngineError::InvalidQuery(_))
    ));
    let stale = HistoryQuery {
        cursor: Some("v1.0.0.x.0".into()),
        ..query.clone()
    };
    assert!(matches!(
        engine.block_on(s.history_page(id, stale)),
        Err(EngineError::StaleCursor)
    ));
    let unknown: WalletId = "00".repeat(32).parse().unwrap();
    assert!(matches!(
        engine.block_on(s.history_page(unknown, query)),
        Err(EngineError::WalletNotFound(_))
    ));
    assert!(matches!(
        engine.block_on(s.tx_detail(id, "ab".repeat(32))),
        Err(EngineError::TxNotFound(_))
    ));
    assert!(matches!(
        engine.block_on(s.tx_detail(id, "AB".repeat(32))),
        Err(EngineError::InvalidArgument(_))
    ));
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn sync_state_events_and_spv_requirements() {
    let dir = tempfile::tempdir().unwrap();
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(dir.path(), Arc::clone(&rec));
    let s = open(&engine);

    let snap = s.sync_snapshot().unwrap();
    assert!(!snap.running && !snap.caught_up);
    assert_eq!(snap.phases.len(), 4);
    assert_eq!(snap.connected_peers, 0);
    assert!(s.peers().unwrap().is_empty());
    assert!(matches!(
        engine.block_on(s.rescan(RescanFrom::Genesis)),
        Err(EngineError::SpvNotRunning)
    ));
    assert!(matches!(
        engine.block_on(s.rotate_peers()),
        Err(EngineError::SpvNotRunning)
    ));

    engine.block_on(s.start_spv()).unwrap();
    assert!(s.sync_snapshot().unwrap().running);
    // The pump delivers a Sync event for the start within its interval.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rec
        .events()
        .iter()
        .any(|e| matches!(e, EngineEvent::Sync { snapshot, .. } if snapshot.running))
    {
        assert!(
            Instant::now() < deadline,
            "no Sync event: {:?}",
            rec.events()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(
        engine.block_on(s.rescan(RescanFrom::Height(1_000_000))),
        Err(EngineError::HeightOutOfRange(1_000_000))
    ));
    engine.block_on(s.rescan(RescanFrom::WalletBirth)).unwrap();
    engine.block_on(s.rotate_peers()).unwrap();
    assert!(s.spv_running().unwrap());
    engine.block_on(s.stop_spv()).unwrap();
    assert!(!s.sync_snapshot().unwrap().running);
    engine.block_on(engine.shutdown()).unwrap();
    // Review M1: calls after close are refused.
    assert!(matches!(
        s.sync_snapshot(),
        Err(EngineError::NetworkNotOpen(_))
    ));
    assert!(matches!(
        engine.block_on(s.start_spv()),
        Err(EngineError::NetworkNotOpen(_))
    ));
}

/// Review M7: a data root the user chose keeps its permissions; only the
/// directories the engine creates are restricted.
#[cfg(unix)]
#[test]
fn chosen_data_root_keeps_its_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("chosen");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let engine = new_engine(&root, Arc::new(Recorder::default()));
    let s = open(&engine);
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o755);
    assert_eq!(mode(&root.join("regtest")), 0o700);
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
}
