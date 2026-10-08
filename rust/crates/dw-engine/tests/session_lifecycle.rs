//! Offline engine tests: real PlatformWalletManager + SqlitePersister in a temp
//! dir, regtest/devnet with loopback endpoints that nothing listens on. Vaults
//! use cheap Argon2id parameters and an in-process OS store.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use dashcore::secp256k1::Secp256k1;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, LockState, MemoryOsStore, UnlockScope,
    VaultConfig, VaultError,
};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
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
        ..Default::default()
    }
}

/// Vault passphrase of the tests' encrypted vaults.
const PASSPHRASE: &[u8] = b"correct horse battery staple";

fn new_engine(root: &std::path::Path, sink: Arc<Recorder>) -> Engine {
    new_engine_with_store(root, sink, Arc::new(MemoryOsStore::new()))
}

/// `os_store` stands in for the OS keyring; share it between engines to model
/// an app restart on the same machine.
fn new_engine_with_store(
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

/// Creates the session's vault: encrypted with [`PASSPHRASE`], or unencrypted.
fn create_vault(engine: &Engine, session: &Arc<NetworkSession>, encrypted: bool) {
    engine
        .block_on(session.vault_op(move |v| v.create(encrypted.then_some(PASSPHRASE))))
        .unwrap();
}

fn import(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    phrase: &str,
) -> Result<WalletId, EngineError> {
    engine.block_on(session.import_wallet(
        Zeroizing::new(phrase.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(0),
            ..ImportOptions::default()
        },
    ))
}

/// The regtest P2PKH address at `m/44'/1'/0'/0/0` of `phrase`.
fn first_receive_address(phrase: &str) -> String {
    let secret = dw_vault::mnemonic::derive_secret(phrase.as_bytes(), b"", false).unwrap();
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let child = master
        .derive_priv(&secp, &DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
        .unwrap();
    let pubkey = dashcore::PublicKey::new(child.private_key.public_key(&secp));
    dashcore::Address::p2pkh(&pubkey, dashcore::Network::Regtest).to_string()
}

#[test]
fn created_wallet_survives_engine_restart() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("data");
    let rec = Arc::new(Recorder::default());

    let engine = new_engine(&root, Arc::clone(&rec));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    assert!(root.join("regtest").join("wallet.sqlite").exists());
    create_vault(&engine, &session, true);

    let created = engine.block_on(session.create_wallet(12)).unwrap();
    assert!(session.vault().has_wallet_secret(&created.wallet_id.0));
    assert_eq!(created.mnemonic.split_whitespace().count(), 12);

    let wallets = session.wallet_infos().unwrap();
    assert_eq!(wallets.len(), 1);
    assert_eq!(wallets[0].wallet_id, created.wallet_id);
    assert_eq!(wallets[0].name, "Wallet 1");
    assert!(wallets[0].has_mnemonic && !wallets[0].watch_only);
    // Nothing scanned yet: the balance is unknown, not zero (review M-3).
    assert_eq!(wallets[0].balances, None);
    assert_eq!(session.balances(&created.wallet_id).unwrap(), None);

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
        session.wallet_infos(),
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
    let infos = session.wallet_infos().unwrap();
    assert_eq!(
        infos.iter().map(|w| w.wallet_id).collect::<Vec<_>>(),
        vec![created.wallet_id]
    );
    // Review H-2: the keys are still there after the restart.
    assert!(infos[0].has_mnemonic && !infos[0].watch_only);
    assert_eq!(infos[0].name, "Wallet 1");
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn imported_wallet_id_is_deterministic_and_network_scoped() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));

    let regtest = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    create_vault(&engine, &regtest, false);
    let a = import(&engine, &regtest, ABANDON_12).unwrap();
    // Importing a wallet whose keys the vault already holds is refused.
    let dup = import(&engine, &regtest, ABANDON_12);
    assert!(
        matches!(dup, Err(EngineError::WalletAlreadyExists(_))),
        "{dup:?}"
    );

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
    create_vault(&engine, &dev, false);
    let b = import(&engine, &dev, ABANDON_12).unwrap();
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
        .wallet_infos()
        .unwrap()
        .into_iter()
        .map(|w| w.wallet_id)
        .collect();
    assert_eq!(ids, vec![a]);
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn dropping_engine_without_shutdown_releases_storage() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    create_vault(&engine, &s, true);
    let id = import(&engine, &s, ABANDON_12).unwrap();
    drop(s);
    // Review L2: dropping returns at once; the sessions are closed on a
    // background thread, so the storage is free shortly after.
    let started = std::time::Instant::now();
    drop(engine);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));

    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let s = loop {
        match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
            Ok(s) => break s,
            Err(EngineError::StorageInUse(_)) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => panic!("{e:?}"),
        }
    };
    assert_eq!(
        s.wallet_infos()
            .unwrap()
            .into_iter()
            .map(|w| w.wallet_id)
            .collect::<Vec<_>>(),
        vec![id]
    );
}

#[test]
fn second_engine_on_same_data_dir_gets_storage_in_use() {
    let dir = dw_testutil::private_tempdir();
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
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    create_vault(&engine, &s, false);
    assert!(matches!(
        engine.block_on(s.create_wallet(13)),
        Err(EngineError::InvalidArgument(_))
    ));
    let bad = import(&engine, &s, "not a mnemonic");
    assert!(
        matches!(bad, Err(EngineError::InvalidMnemonic(_))),
        "{bad:?}"
    );
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
    let dir = dw_testutil::private_tempdir();
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

/// Review H-1: no path registers a wallet whose seed is not in the vault.
#[test]
fn import_without_usable_vault_registers_nothing() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();

    let err = import(&engine, &s, ABANDON_12);
    assert!(
        matches!(err, Err(EngineError::Vault(VaultError::NoVault))),
        "{err:?}"
    );
    let err = engine.block_on(s.create_wallet(12));
    assert!(
        matches!(err, Err(EngineError::Vault(VaultError::NoVault))),
        "{err:?}"
    );
    assert!(s.wallet_infos().unwrap().is_empty());

    create_vault(&engine, &s, true);
    s.lock_vault().unwrap();
    let err = import(&engine, &s, ABANDON_12);
    assert!(
        matches!(err, Err(EngineError::Vault(VaultError::Locked))),
        "{err:?}"
    );
    assert!(s.wallet_infos().unwrap().is_empty());
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review H-2: after a restart the wallet still has its keys: unlocking the
/// vault lets it sign, and the signature verifies against its address.
#[test]
fn imported_wallet_signs_after_restart_and_unlock() {
    let dir = dw_testutil::private_tempdir();
    let rec = Arc::new(Recorder::default());
    let engine = new_engine(dir.path(), Arc::clone(&rec));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    create_vault(&engine, &s, true);
    assert!(rec.events().contains(&EngineEvent::VaultLockState {
        network: DashNetwork::Regtest,
        state: LockState::Unlocked,
    }));
    let id = import(&engine, &s, ABANDON_12).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
    drop(s);
    drop(engine);

    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    let status = s.vault().status();
    assert_eq!(status.state, LockState::Locked);
    assert_eq!(status.wallets_with_secrets, vec![id.0]);
    assert_eq!(
        s.wallet_infos()
            .unwrap()
            .into_iter()
            .map(|w| w.wallet_id)
            .collect::<Vec<_>>(),
        vec![id]
    );

    // Locked: no grant without the passphrase.
    let locked =
        engine.block_on(s.vault_op(move |v| {
            v.authorize(GrantPurpose::SignMessage, Some(&id.0), Credential::None)
        }));
    assert!(
        matches!(locked, Err(EngineError::Vault(VaultError::Locked))),
        "{locked:?}"
    );

    engine
        .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, UnlockScope::Full)))
        .unwrap();
    let grant = engine
        .block_on(s.vault_op(move |v| {
            v.authorize(GrantPurpose::SignMessage, Some(&id.0), Credential::None)
        }))
        .unwrap();
    let address = first_receive_address(ABANDON_12);
    let message = b"dashwallet-desktop restart test".to_vec();
    let signature = engine
        .block_on(s.sign_message(id, address.clone(), message.clone(), grant.id.clone()))
        .unwrap();
    dw_message::verify_message(&address, &signature, &message, dashcore::Network::Regtest).unwrap();

    // Grants are single use.
    let reused = engine.block_on(s.sign_message(id, address, message, grant.id));
    assert!(
        matches!(reused, Err(EngineError::Vault(VaultError::GrantInvalid))),
        "{reused:?}"
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// Re-importing the phrase of a registered wallet whose vault records were
/// lost attaches the keys again instead of failing.
#[test]
fn import_attaches_keys_to_registered_wallet_without_secret() {
    let dir = dw_testutil::private_tempdir();
    let engine = new_engine(dir.path(), Arc::new(Recorder::default()));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    create_vault(&engine, &s, false);
    let id = import(&engine, &s, ABANDON_12).unwrap();
    assert!(s.vault().delete_wallet_secret(&id.0).unwrap());
    assert!(!s.vault().has_wallet_secret(&id.0));

    assert_eq!(import(&engine, &s, ABANDON_12).unwrap(), id);
    assert!(s.vault().has_wallet_secret(&id.0));
    assert_eq!(s.wallet_infos().unwrap().len(), 1);
    engine.block_on(engine.shutdown()).unwrap();
}
