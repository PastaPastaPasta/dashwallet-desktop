//! Offline tests of the R2 calls (docs/contracts/m2-engine.md §2.6–2.7):
//! Dash Core file imports against dashd-made vectors (testdata/compat,
//! testdata/dumpwallet), exports for dash-qt read back by our own parsers,
//! `.dwbackup` round trips between vaults and automatic backup rotation.
//! The dashd side of the round trips is the regtest `restore` suite.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_engine::{
    AddressChain, AddressFilter, BackupFailure, BookPurpose, CompatFailure, CoreExportFormat,
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    KeyMaterial, NetworkSession, SessionOptions, WalletFileKind, WalletId, inspect_wallet_file,
};
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig,
};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use zeroize::Zeroizing;

struct Quiet;

impl EventSink for Quiet {
    fn emit(&self, _event: EngineEvent) {}
}

fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata")
        .join(rel)
}

fn manifest() -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(testdata("compat/manifest.json")).unwrap()).unwrap()
}

fn engine(root: &Path) -> Engine {
    engine_with(root, Arc::new(Quiet))
}

fn engine_with(root: &Path, sink: Arc<dyn EventSink>) -> Engine {
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
        sink,
    )
    .unwrap()
}

fn session(e: &Engine, network: DashNetwork) -> Arc<NetworkSession> {
    e.block_on(e.open_network(
        network,
        SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        },
    ))
    .unwrap()
}

fn open(e: &Engine, passphrase: Option<&[u8]>) -> Arc<NetworkSession> {
    let s = session(e, DashNetwork::Regtest);
    let pw = passphrase.map(<[u8]>::to_vec);
    e.block_on(s.vault_op(move |v| v.create(pw.as_deref())))
        .unwrap();
    s
}

/// Dash Core's wallet id of a phrase + passphrase on regtest.
fn core_id(phrase: &str, passphrase: &str) -> WalletId {
    let s =
        dw_vault::mnemonic::derive_secret(phrase.as_bytes(), passphrase.as_bytes(), true).unwrap();
    WalletId(dw_vault::mnemonic::wallet_id_for_seed(&s.seed, dashcore::Network::Regtest).unwrap())
}

fn receive_addresses(e: &Engine, s: &Arc<NetworkSession>, id: WalletId, n: usize) -> Vec<String> {
    chain_addresses(e, s, id, AddressChain::Receiving, n)
}

fn chain_addresses(
    e: &Engine,
    s: &Arc<NetworkSession>,
    id: WalletId,
    chain: AddressChain,
    n: usize,
) -> Vec<String> {
    let mut a = e
        .block_on(s.addresses(
            id,
            AddressFilter {
                chain: Some(chain),
                used: None,
            },
        ))
        .unwrap();
    a.sort_by_key(|x| x.index);
    a.into_iter().take(n).map(|x| x.address).collect()
}

fn strings(v: &serde_json::Value) -> Vec<String> {
    serde_json::from_value(v.clone()).unwrap()
}

#[test]
fn test_qt_107_import_dump_wallet_restores_dashd_wallet() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, None);
    let path = testdata("dumpwallet/dump_hd_basic.txt");
    let report = e
        .block_on(s.import_dump_wallet(path.clone(), ImportOptions::default()))
        .unwrap();
    let dumps: serde_json::Value =
        serde_json::from_slice(&std::fs::read(testdata("dumpwallet/manifest.json")).unwrap())
            .unwrap();
    let d = &dumps["dumps"][0];
    assert_eq!(
        report.wallet_id,
        core_id(
            d["mnemonic"].as_str().unwrap(),
            d["mnemonic_passphrase"].as_str().unwrap()
        )
    );
    assert!(report.core_compat_seed);
    assert_eq!(report.keys_not_imported, 2);
    assert_eq!(report.scripts_not_imported, 2);
    let labelled: Vec<String> = d["labelled_addresses"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["address"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(receive_addresses(&e, &s, report.wallet_id, 4), labelled);
    assert_eq!(report.labels_imported, 4);
    let book = e
        .block_on(s.address_book(report.wallet_id, Some(BookPurpose::Receive), None))
        .unwrap();
    assert!(book.iter().any(|b| b.label == "spaces and % and # and é"));
    assert!(book.iter().any(|b| b.label == "tab\there"));
    // The same file again: the wallet exists with its keys.
    let again = e.block_on(s.import_dump_wallet(path, ImportOptions::default()));
    assert!(
        matches!(again, Err(EngineError::WalletAlreadyExists(_))),
        "{again:?}"
    );
    // Loose keys only: nothing to rebuild an HD wallet from.
    let loose = e.block_on(s.import_dump_wallet(
        testdata("dumpwallet/dump_loose_keys.txt"),
        ImportOptions::default(),
    ));
    assert!(
        matches!(loose, Err(EngineError::Compat(CompatFailure::NoHdChain))),
        "{loose:?}"
    );
    let not_dump = e
        .block_on(s.import_dump_wallet(testdata("compat/manifest.json"), ImportOptions::default()));
    assert!(matches!(
        not_dump,
        Err(EngineError::Compat(CompatFailure::UnsupportedFormat(_)))
    ));
}

#[test]
fn test_qt_106_import_wallet_dat() {
    let m = manifest();
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, Some(b"vault pass"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pass", UnlockScope::Full)))
        .unwrap();
    let enc = &m["wallets"][1];
    let path = testdata(&format!("compat/{}", enc["file"].as_str().unwrap()));
    let opts = ImportOptions::default;
    let none = e.block_on(s.import_wallet_dat(path.clone(), None, opts()));
    assert!(
        matches!(
            none,
            Err(EngineError::Compat(CompatFailure::PassphraseRequired))
        ),
        "{none:?}"
    );
    let wrong = e.block_on(s.import_wallet_dat(
        path.clone(),
        Some(Zeroizing::new(b"nope".to_vec())),
        opts(),
    ));
    assert!(matches!(
        wrong,
        Err(EngineError::Compat(CompatFailure::WrongPassphrase))
    ));
    let pass = enc["wallet_passphrase"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec();
    let report = e
        .block_on(s.import_wallet_dat(path, Some(Zeroizing::new(pass)), opts()))
        .unwrap();
    assert_eq!(
        report.wallet_id,
        core_id(
            enc["mnemonic"].as_str().unwrap(),
            enc["mnemonic_passphrase"].as_str().unwrap()
        )
    );
    for (chain, key) in [
        (AddressChain::Receiving, "external"),
        (AddressChain::Change, "internal"),
    ] {
        let want = strings(&enc["addresses"][key]);
        assert_eq!(
            chain_addresses(&e, &s, report.wallet_id, chain, want.len()),
            want,
            "{key}"
        );
    }
    assert_eq!(report.labels_imported, 1);

    // The weak-checksum phrase with a 300-byte passphrase (Core cuts the salt).
    let weak = &m["wallets"][2];
    let report = e
        .block_on(s.import_wallet_dat(
            testdata(&format!("compat/{}", weak["file"].as_str().unwrap())),
            None,
            opts(),
        ))
        .unwrap();
    let want = strings(&weak["addresses"]["external"]);
    assert_eq!(
        receive_addresses(&e, &s, report.wallet_id, want.len()),
        want
    );

    let bdb = e.block_on(s.import_wallet_dat(
        testdata("compat/walletdat/bdb_legacy_head.dat"),
        None,
        opts(),
    ));
    assert!(
        matches!(&bdb, Err(EngineError::Compat(CompatFailure::Corrupt(_)))),
        "{bdb:?}"
    );
    let xprv_only = e.block_on(s.import_wallet_dat(
        testdata("compat/walletdat/desc_nomnemonic.dat"),
        None,
        opts(),
    ));
    assert!(
        matches!(&xprv_only, Err(EngineError::NotImplemented(c)) if c.starts_with("import_wallet_dat.xprv")),
        "{xprv_only:?}"
    );

    // A synthetic legacy wallet (the test builder) restores the phrase,
    // labels and loose-key count. Real Dash Core files are
    // `test_qt_106_import_dash_core_bdb`.
    let phrase = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
    let plain = e
        .block_on(s.import_wallet_dat(testdata("compat/walletdat/bdb_hd_plain.dat"), None, opts()))
        .unwrap();
    assert_eq!(plain.wallet_id, core_id(phrase, "TREZOR"));
    assert_eq!(plain.labels_imported, 1);
    assert_eq!(plain.keys_not_imported, 2);
    assert_eq!(plain.scripts_not_imported, 2);
    assert!(plain.core_compat_seed);
    let got = receive_addresses(&e, &s, plain.wallet_id, 1);
    assert_eq!(got.len(), 1);

    let enc_path = testdata("compat/walletdat/bdb_hd_encrypted.dat");
    assert_eq!(
        inspect_wallet_file(&enc_path).unwrap(),
        WalletFileKind::WalletDatBdb {
            encrypted: Some(true)
        }
    );
    let missing = e.block_on(s.import_wallet_dat(enc_path.clone(), None, opts()));
    assert!(matches!(
        missing,
        Err(EngineError::Compat(CompatFailure::PassphraseRequired))
    ));
    let opened = e
        .block_on(s.import_wallet_dat(enc_path, Some(Zeroizing::new(b"bdb-pass".to_vec())), opts()))
        .unwrap();
    assert_eq!(opened.wallet_id, core_id(phrase, "second"));

    // A locked vault cannot take the seed.
    s.lock_vault_sync().unwrap();
    let locked =
        e.block_on(s.import_wallet_dat(testdata("compat/walletdat/desc_plain.dat"), None, opts()));
    assert!(
        matches!(
            locked,
            Err(EngineError::Vault(dw_vault::VaultError::Locked))
        ),
        "{locked:?}"
    );
}

/// Dash Core v24.0.2 regtest wallets (`createwallet` with `descriptors=false`,
/// `upgradetohd`, `encryptwallet`). The first receive addresses are the ones
/// Core's `dumpwallet` recorded for `m/44'/1'/0'/0/0` and `/1`.
#[test]
fn test_qt_106_import_dash_core_bdb() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, Some(b"vault pass"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pass", UnlockScope::Full)))
        .unwrap();
    let opts = ImportOptions::default;
    let zoo = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
    let plain = e
        .block_on(s.import_wallet_dat(
            testdata("compat/walletdat/bdb_core24_hd_mnem.dat"),
            None,
            opts(),
        ))
        .unwrap();
    assert_eq!(plain.wallet_id, core_id(zoo, "TREZOR"));
    assert_eq!(plain.labels_imported, 2);
    assert_eq!(plain.keys_not_imported, 0);
    assert_eq!(
        receive_addresses(&e, &s, plain.wallet_id, 2),
        [
            "yfzF8VHskautet5pKeKo1aoK1EgkyJpeU1",
            "yhjzbZYPH8hXV9xMfgi1g9927o5kxfXfHa",
        ]
    );

    let enc = testdata("compat/walletdat/bdb_core24_hd_encrypted.dat");
    assert!(matches!(
        e.block_on(s.import_wallet_dat(enc.clone(), None, opts())),
        Err(EngineError::Compat(CompatFailure::PassphraseRequired))
    ));
    assert!(matches!(
        e.block_on(s.import_wallet_dat(
            enc.clone(),
            Some(Zeroizing::new(b"nope".to_vec())),
            opts()
        )),
        Err(EngineError::Compat(CompatFailure::WrongPassphrase))
    ));
    let opened = e
        .block_on(s.import_wallet_dat(
            enc,
            Some(Zeroizing::new(b"core-wallet-pass".to_vec())),
            opts(),
        ))
        .unwrap();
    assert_eq!(
        opened.wallet_id,
        core_id(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            ""
        )
    );
    assert_eq!(
        receive_addresses(&e, &s, opened.wallet_id, 2),
        [
            "yRd4FhXfVGHXpsuZXPNkMrfD9GVj46pnjt",
            "yfd64jEpzzTLrHnR1wq3iiYXh68AiU8mcw",
        ]
    );
}

fn regtest_address(phrase: &str, pass: &str, path: &str) -> String {
    let secret =
        dw_vault::mnemonic::derive_secret(phrase.as_bytes(), pass.as_bytes(), true).unwrap();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let child = master
        .derive_priv(&DerivationPath::from_str(path).unwrap())
        .unwrap();
    dashcore::Address::p2pkh(
        &dashcore::PublicKey::new(child.private_key.public_key()),
        dashcore::Network::Regtest,
    )
    .to_string()
}

/// A label-free keypool is still checked against the session network, loose
/// keys are reported, coin locks are stored before any spend, and a chain
/// past the gap limit is actually derived.
#[test]
fn test_qt_106_import_checks_network_range_locks_and_loose_keys() {
    let zoo = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
    let main_dir = dw_testutil::private_tempdir();
    let main = engine(main_dir.path());
    let main_session = session(&main, DashNetwork::Mainnet);
    let refused = main.block_on(main_session.import_wallet_dat(
        testdata("compat/walletdat/bdb_label_free.dat"),
        None,
        ImportOptions::default(),
    ));
    assert!(
        matches!(
            refused,
            Err(EngineError::Compat(CompatFailure::NetworkMismatch {
                mainnet: false
            }))
        ),
        "{refused:?}"
    );

    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, Some(b"vault pass"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pass", UnlockScope::Full)))
        .unwrap();
    let opts = ImportOptions::default;
    let plain = e
        .block_on(s.import_wallet_dat(
            testdata("compat/walletdat/bdb_label_free.dat"),
            None,
            opts(),
        ))
        .unwrap();
    assert_eq!(plain.wallet_id, core_id(zoo, ""));
    assert_eq!(plain.labels_imported, 0);
    assert_eq!(
        receive_addresses(&e, &s, plain.wallet_id, 1),
        [regtest_address(zoo, "", "m/44'/1'/0'/0/0")]
    );

    let wide = e
        .block_on(s.import_wallet_dat(
            testdata("compat/walletdat/bdb_wide_range.dat"),
            None,
            opts(),
        ))
        .unwrap();
    assert_eq!(wide.keys_not_imported, 1);
    let receive = e
        .block_on(s.addresses(
            wide.wallet_id,
            AddressFilter {
                chain: Some(AddressChain::Receiving),
                used: None,
            },
        ))
        .unwrap();
    assert!(
        receive.iter().any(|a| a.index == 1000),
        "receive index 1000 was not derived"
    );
    let change = e
        .block_on(s.addresses(
            wide.wallet_id,
            AddressFilter {
                chain: Some(AddressChain::Change),
                used: None,
            },
        ))
        .unwrap();
    assert!(change.iter().any(|a| a.index == 1));
    let locks = e.block_on(s.locked_outpoints(wide.wallet_id)).unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].vout, 7);
    assert_eq!(locks[0].txid.to_string(), "ab".repeat(32));

    e.block_on(e.close_network(DashNetwork::Regtest)).unwrap();
    let s = session(&e, DashNetwork::Regtest);
    let chain_reach = |chain| {
        e.block_on(s.addresses(
            wide.wallet_id,
            AddressFilter {
                chain: Some(chain),
                used: None,
            },
        ))
        .unwrap()
        .iter()
        .map(|a| a.index)
        .max()
    };
    assert!(
        chain_reach(AddressChain::Receiving) >= Some(1000),
        "the receive range was not restored after a restart"
    );
    assert!(chain_reach(AddressChain::Change) >= Some(1));
    assert_eq!(
        e.block_on(s.locked_outpoints(wide.wallet_id))
            .unwrap()
            .len(),
        1
    );

    let account = e.block_on(s.import_wallet_dat(
        testdata("compat/walletdat/bdb_account1.dat"),
        None,
        opts(),
    ));
    assert!(
        matches!(
            &account,
            Err(EngineError::Compat(CompatFailure::ExtraHdAccount {
                account: 1
            }))
        ),
        "{account:?}"
    );
    let inspected = dw_engine::inspect_wallet_file(&testdata("compat/walletdat/bdb_account1.dat"));
    assert!(
        matches!(
            &inspected,
            Err(EngineError::Compat(CompatFailure::ExtraHdAccount {
                account: 1
            }))
        ),
        "{inspected:?}"
    );
}

const ABANDON: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn app_db(s: &Arc<NetworkSession>) -> rusqlite::Connection {
    let conn = rusqlite::Connection::open(s.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    conn.busy_timeout(Duration::from_secs(30)).unwrap();
    conn
}

/// A wallet's setting `key` as stored, read past the engine.
fn stored_setting(s: &Arc<NetworkSession>, id: WalletId, key: &str) -> Option<String> {
    app_db(s)
        .query_row(
            "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
            [id.to_string().as_str(), key],
            |r| r.get(0),
        )
        .ok()
}

/// Makes every CoinJoin salt write fail, or work again: the last write of
/// a wallet.dat import, after its range and locks are stored.
fn refuse_salt_writes(s: &Arc<NetworkSession>, refuse: bool) {
    let sql = if refuse {
        "CREATE TRIGGER refuse_salt_insert BEFORE INSERT ON settings_kv
           WHEN NEW.key = 'coinjoin.salt' BEGIN SELECT RAISE(ABORT, 'test: salt refused'); END;
         CREATE TRIGGER refuse_salt_update BEFORE UPDATE ON settings_kv
           WHEN NEW.key = 'coinjoin.salt' BEGIN SELECT RAISE(ABORT, 'test: salt refused'); END;"
    } else {
        "DROP TRIGGER refuse_salt_insert; DROP TRIGGER refuse_salt_update;"
    };
    app_db(s).execute_batch(sql).unwrap();
}

fn import_wide(e: &Engine, s: &Arc<NetworkSession>) -> Result<WalletId, EngineError> {
    e.block_on(s.import_wallet_dat(
        testdata("compat/walletdat/bdb_wide_range.dat"),
        None,
        ImportOptions::default(),
    ))
    .map(|r| r.wallet_id)
}

fn restore_abandon(e: &Engine, s: &Arc<NetworkSession>) -> WalletId {
    e.block_on(s.import_wallet(
        Zeroizing::new(ABANDON.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            core_compat: true,
            ..ImportOptions::default()
        },
    ))
    .unwrap()
}

fn reaches_index_1000(e: &Engine, s: &Arc<NetworkSession>, id: WalletId) -> bool {
    e.block_on(s.addresses(
        id,
        AddressFilter {
            chain: Some(AddressChain::Receiving),
            used: None,
        },
    ))
    .unwrap()
    .iter()
    .any(|a| a.index == 1000)
}

fn unlocked_session(e: &Engine) -> Arc<NetworkSession> {
    let s = open(e, Some(b"vault pass"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pass", UnlockScope::Full)))
        .unwrap();
    s
}

/// An import whose last write fails is refused; importing the file again
/// restores the range, Core's lock and its salt.
#[test]
fn test_qt_106_a_failed_wallet_dat_import_can_be_retried() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = unlocked_session(&e);
    let id = core_id(ABANDON, "");
    refuse_salt_writes(&s, true);
    let failed = import_wide(&e, &s);
    assert!(failed.is_err(), "{failed:?}");
    refuse_salt_writes(&s, false);
    assert_eq!(import_wide(&e, &s).unwrap(), id);
    assert!(
        reaches_index_1000(&e, &s, id),
        "the retry did not restore the receive range"
    );
    let locks = e.block_on(s.locked_outpoints(id)).unwrap();
    assert_eq!(locks.len(), 1, "the retry did not install Core's lock");
    assert_eq!(locks[0].vout, 7);
    assert_eq!(e.block_on(s.coinjoin_salt(id)).unwrap(), CORE_SALT);
}

/// A failed import leaves nothing of the file: restoring the same seed from
/// its phrase gets no Core lock, salt or range.
#[test]
fn test_qt_106_a_failed_wallet_dat_import_leaves_nothing_for_a_phrase_restore() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = unlocked_session(&e);
    refuse_salt_writes(&s, true);
    let failed = import_wide(&e, &s);
    assert!(failed.is_err(), "{failed:?}");
    refuse_salt_writes(&s, false);
    let id = restore_abandon(&e, &s);
    assert!(e.block_on(s.locked_outpoints(id)).unwrap().is_empty());
    assert_eq!(stored_setting(&s, id, "bip44.legacy_issued"), None);
    assert_ne!(e.block_on(s.coinjoin_salt(id)).unwrap(), CORE_SALT);
    assert!(!reaches_index_1000(&e, &s, id));
}

/// Importing the wallet.dat of a wallet already there, open or closed,
/// says it exists and changes nothing of it: its own salt, the locks the
/// user left, its range.
#[test]
fn test_qt_106_an_import_of_a_wallet_already_there_changes_nothing() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = unlocked_session(&e);
    let id = restore_abandon(&e, &s);
    let own_salt = e.block_on(s.coinjoin_salt(id)).unwrap();
    assert_ne!(own_salt, CORE_SALT);
    let exists = |again: Result<WalletId, EngineError>| {
        assert!(
            matches!(&again, Err(EngineError::WalletAlreadyExists(w)) if *w == id.to_string()),
            "{again:?}"
        );
    };
    exists(import_wide(&e, &s));
    assert_eq!(e.block_on(s.coinjoin_salt(id)).unwrap(), own_salt);
    assert!(e.block_on(s.locked_outpoints(id)).unwrap().is_empty());
    assert_eq!(stored_setting(&s, id, "bip44.legacy_issued"), None);
    assert!(!reaches_index_1000(&e, &s, id));
    e.block_on(s.unload_wallet(id)).unwrap();
    exists(import_wide(&e, &s));
    assert_eq!(
        stored_setting(&s, id, "coinjoin.salt").as_deref(),
        Some(own_salt.as_str())
    );
    assert_eq!(stored_setting(&s, id, "bip44.legacy_issued"), None);

    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = unlocked_session(&e);
    let id = import_wide(&e, &s).unwrap();
    let locks = e.block_on(s.locked_outpoints(id)).unwrap();
    assert_eq!(locks.len(), 1);
    e.block_on(s.unlock_outpoints(id, locks)).unwrap();
    exists(import_wide(&e, &s));
    assert!(
        e.block_on(s.locked_outpoints(id)).unwrap().is_empty(),
        "the coin the user unlocked was locked again"
    );
}

/// A watch-only wallet: the `abandon` wallet restored from its phrase,
/// with its own salt, then its seed deleted from the vault.
fn watch_only_abandon(e: &Engine, s: &Arc<NetworkSession>) -> (WalletId, String) {
    let id = restore_abandon(e, s);
    let own_salt = e.block_on(s.coinjoin_salt(id)).unwrap();
    assert_ne!(own_salt, CORE_SALT);
    assert!(s.vault().delete_wallet_secret(&id.0).unwrap());
    (id, own_salt)
}

/// A wallet.dat of a watch-only wallet's seed attaches its keys with Core's
/// range, locks and salt, all in place when the wallet is announced.
#[test]
fn test_qt_106_a_wallet_dat_attaches_keys_to_a_watch_only_wallet() {
    let dir = dw_testutil::private_tempdir();
    let probe = Arc::new(CreatedProbe::default());
    let e = engine_with(dir.path(), Arc::clone(&probe) as Arc<dyn EventSink>);
    let s = unlocked_session(&e);
    probe
        .db
        .set(s.data_dir().join(dw_appdb::APP_DB_FILE))
        .unwrap();
    let (id, _) = watch_only_abandon(&e, &s);
    assert_eq!(import_wide(&e, &s).unwrap(), id);
    assert!(s.vault().has_wallet_secret(&id.0));
    assert!(reaches_index_1000(&e, &s, id));
    assert_eq!(e.block_on(s.locked_outpoints(id)).unwrap().len(), 1);
    assert_eq!(e.block_on(s.coinjoin_salt(id)).unwrap(), CORE_SALT);
    let seen = probe.seen.lock().unwrap().clone();
    assert_eq!(
        seen.last(),
        Some(&(
            id.to_string(),
            1,
            Some("1001,2".to_owned()),
            Some(CORE_SALT.to_owned())
        )),
        "what the app database held when the keys were announced: {seen:?}"
    );
}

/// Attaching keys whose last write fails leaves the watch-only wallet as it
/// was: no seed, its own salt, no Core lock or range setting.
#[test]
fn test_qt_106_a_failed_key_attachment_leaves_the_watch_only_wallet_as_it_was() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = unlocked_session(&e);
    let (id, own_salt) = watch_only_abandon(&e, &s);
    refuse_salt_writes(&s, true);
    let failed = import_wide(&e, &s);
    assert!(failed.is_err(), "{failed:?}");
    refuse_salt_writes(&s, false);
    assert!(!s.vault().has_wallet_secret(&id.0));
    assert_eq!(stored_setting(&s, id, "coinjoin.salt"), Some(own_salt));
    assert_eq!(stored_setting(&s, id, "bip44.legacy_issued"), None);
    assert!(e.block_on(s.locked_outpoints(id)).unwrap().is_empty());
}

/// A `.dwbackup` of a legacy wallet restores its range before the wallet
/// is announced, in that session, without a reload.
#[test]
fn test_qt_110_a_restored_legacy_wallet_has_its_range_at_once() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("a"));
    let s = unlocked_session(&e);
    let id = import_wide(&e, &s).unwrap();
    let dest = dir.path().join("legacy.dwbackup");
    e.block_on(s.backup_wallet(id, dest.clone(), None)).unwrap();

    let e2 = engine(&dir.path().join("b"));
    let s2 = open(&e2, None);
    let ids = e2
        .block_on(s2.restore_backup(dest, Some(Zeroizing::new(b"vault pass".to_vec()))))
        .unwrap();
    assert_eq!(ids, vec![id]);
    assert!(
        reaches_index_1000(&e2, &s2, id),
        "the range was not restored"
    );
    assert_eq!(e2.block_on(s2.locked_outpoints(id)).unwrap().len(), 1);
}

/// Core's CoinJoin salt in `bdb_wide_range.dat`.
const CORE_SALT: &str = "5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c";

type Seen = (String, i64, Option<String>, Option<String>);

/// What the app database held for a wallet when `WalletCreated` was
/// emitted: its open locks, stored range and CoinJoin salt.
#[derive(Default)]
struct CreatedProbe {
    db: std::sync::OnceLock<PathBuf>,
    seen: std::sync::Mutex<Vec<Seen>>,
}

impl EventSink for CreatedProbe {
    fn emit(&self, event: EngineEvent) {
        let EngineEvent::WalletCreated { wallet_id, .. } = event else {
            return;
        };
        let Some(db) = self.db.get() else {
            return;
        };
        let conn =
            rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        let id = wallet_id.to_string();
        let locks: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM utxo_locks WHERE wallet_id = ?1 AND released_at IS NULL",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let setting = |key: &str| {
            conn.query_row(
                "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
                [id.as_str(), key],
                |r| r.get::<_, String>(0),
            )
            .ok()
        };
        let issued = setting("bip44.legacy_issued");
        let salt = setting("coinjoin.salt");
        self.seen.lock().unwrap().push((id, locks, issued, salt));
    }
}

/// Nothing hears of an imported legacy wallet before Core's range, coin
/// locks and salt are stored: `WalletCreated` finds them already there.
#[test]
fn test_qt_106_a_legacy_wallet_is_announced_with_its_locks_in_place() {
    let dir = dw_testutil::private_tempdir();
    let probe = Arc::new(CreatedProbe::default());
    let e = engine_with(dir.path(), Arc::clone(&probe) as Arc<dyn EventSink>);
    let s = open(&e, Some(b"vault pass"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pass", UnlockScope::Full)))
        .unwrap();
    probe
        .db
        .set(s.data_dir().join(dw_appdb::APP_DB_FILE))
        .unwrap();
    let report = e
        .block_on(s.import_wallet_dat(
            testdata("compat/walletdat/bdb_wide_range.dat"),
            None,
            ImportOptions::default(),
        ))
        .unwrap();
    let seen = probe.seen.lock().unwrap().clone();
    assert_eq!(
        seen,
        [(
            report.wallet_id.to_string(),
            1,
            Some("1001,2".to_owned()),
            Some(CORE_SALT.to_owned())
        )],
        "what the app database held when the wallet was announced"
    );
}

#[test]
fn test_qt_108_import_key_material() {
    let m = manifest();
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, None);
    let json = std::fs::read(testdata("compat/listdescriptors_plain.json")).unwrap();
    let report = e
        .block_on(s.import_key_material(
            KeyMaterial::Descriptors(Zeroizing::new(json)),
            ImportOptions::default(),
        ))
        .unwrap();
    let plain = &m["wallets"][0];
    assert_eq!(
        report.wallet_id,
        core_id(plain["mnemonic"].as_str().unwrap(), "TREZOR")
    );
    assert!(s.wallet_info(&report.wallet_id).unwrap().has_mnemonic);

    // dashd's `dumphdinfo` seed of the no-passphrase dump.
    let dumps: serde_json::Value =
        serde_json::from_slice(&std::fs::read(testdata("dumpwallet/manifest.json")).unwrap())
            .unwrap();
    let nopass = &dumps["dumps"][1];
    let seed = hex::decode(nopass["hd_seed"].as_str().unwrap()).unwrap();
    let report = e
        .block_on(s.import_key_material(
            KeyMaterial::HdSeed(Zeroizing::new(seed)),
            ImportOptions::default(),
        ))
        .unwrap();
    let info = s.wallet_info(&report.wallet_id).unwrap();
    assert!(!info.has_mnemonic && !info.watch_only);
    assert!(!report.core_compat_seed);
    assert_eq!(
        receive_addresses(&e, &s, report.wallet_id, 1),
        vec![
            nopass["labelled_addresses"][0]["address"]
                .as_str()
                .unwrap()
                .to_owned()
        ]
    );

    let short = e.block_on(s.import_key_material(
        KeyMaterial::HdSeed(Zeroizing::new(vec![1; 32])),
        ImportOptions::default(),
    ));
    assert!(
        matches!(&short, Err(EngineError::NotImplemented(c)) if c.starts_with("import_key_material.seed_length")),
        "{short:?}"
    );
    let tiny = e.block_on(s.import_key_material(
        KeyMaterial::HdSeed(Zeroizing::new(vec![1; 10])),
        ImportOptions::default(),
    ));
    assert!(matches!(
        tiny,
        Err(EngineError::Compat(CompatFailure::InvalidKeyMaterial(_)))
    ));
    let xprv = m["wallets"][3]["xprv"]
        .as_str()
        .unwrap()
        .as_bytes()
        .to_vec();
    let x = e.block_on(s.import_key_material(
        KeyMaterial::Xprv(Zeroizing::new(xprv)),
        ImportOptions::default(),
    ));
    assert!(
        matches!(&x, Err(EngineError::NotImplemented(c)) if c.starts_with("import_key_material.xprv")),
        "{x:?}"
    );
    let mainnet = e.block_on(s.import_key_material(
        KeyMaterial::Xprv(Zeroizing::new(b"xprv9s21ZrQH143K3QTDL4LXw2F7HEK3wJUD2nW2nRk4stbPy6cq3jPPqjiChkVvvNKmPGJxWUtg6LnF5kejMRNNU3TGtRBeJgk33yuGBxrMPHi".to_vec())),
        ImportOptions::default(),
    ));
    assert!(
        matches!(
            mainnet,
            Err(EngineError::Compat(CompatFailure::NetworkMismatch {
                mainnet: true
            }))
        ),
        "{mainnet:?}"
    );
}

fn reveal_grant(
    e: &Engine,
    s: &Arc<NetworkSession>,
    id: WalletId,
    cred: Credential<'static>,
) -> String {
    e.block_on(s.vault_op(move |v| v.authorize(GrantPurpose::RevealSecret, Some(&id.0), cred)))
        .unwrap()
        .id
}

#[test]
fn test_qt_109_exports_for_dash_qt_read_back() {
    const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("a"));
    let s = open(&e, None);
    let id = e
        .block_on(s.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(b"TREZOR".to_vec()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: true,
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let compat = e.block_on(s.core_mnemonic_compatibility(id)).unwrap();
    assert!(compat.core_compatible);
    assert!(compat.warnings.is_empty());

    let dump_path = dir.path().join("export.txt");
    let no_grant = e.block_on(s.export_for_core(
        id,
        CoreExportFormat::DumpWallet,
        dump_path.clone(),
        "missing".into(),
    ));
    assert!(
        matches!(no_grant, Err(EngineError::Vault(_))),
        "{no_grant:?}"
    );
    assert!(!dump_path.exists());
    let grant = reveal_grant(&e, &s, id, Credential::None);
    let report = e
        .block_on(s.export_for_core(id, CoreExportFormat::DumpWallet, dump_path.clone(), grant))
        .unwrap();
    assert!(report.key_count > 0);
    let text = std::fs::read_to_string(&dump_path).unwrap();
    let dump = dw_compat::dump::parse(&text).unwrap();
    let hd = dump.hd.as_ref().unwrap();
    assert_eq!(hd.mnemonic.as_str(), PHRASE);
    assert_eq!(hd.mnemonic_passphrase.as_str(), "TREZOR");
    // Every key line's WIF is the key of its address and path.
    let master = ExtendedPrivKey::from_str(&hd.xprv).unwrap();
    for k in dump.keys.iter().take(5) {
        let path = DerivationPath::from_str(k.hdkeypath.as_deref().unwrap()).unwrap();
        let child = master.derive_priv(&path).unwrap();
        let secret = k.secret(dashcore::Network::Regtest).unwrap();
        assert_eq!(&secret.key[..], &child.private_key.to_secret_bytes()[..]);
        let pk = dashcore::PublicKey::new(child.private_key.public_key());
        assert_eq!(
            dashcore::Address::p2pkh(&pk, dashcore::Network::Regtest).to_string(),
            k.address
        );
    }
    // Byte-identical when rewritten by Core's writer rules.
    assert_eq!(
        dw_compat::dump::write(&dump, dashcore::Network::Regtest).as_str(),
        text
    );
    // Exports replace no file.
    let grant = reveal_grant(&e, &s, id, Credential::None);
    let again =
        e.block_on(s.export_for_core(id, CoreExportFormat::DumpWallet, dump_path.clone(), grant));
    assert!(matches!(
        again,
        Err(EngineError::Compat(CompatFailure::DestinationUnwritable(_)))
    ));

    let desc_path = dir.path().join("import.json");
    let grant = reveal_grant(&e, &s, id, Credential::None);
    e.block_on(s.export_for_core(
        id,
        CoreExportFormat::ImportDescriptorsJson,
        desc_path.clone(),
        grant,
    ))
    .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&desc_path).unwrap()).unwrap();
    let first = json[0]["desc"].as_str().unwrap();
    let d = dw_compat::descriptor::PkhDescriptor::parse(first).unwrap();
    assert_eq!(d.key.as_str(), hd.xprv.as_str());
    assert_eq!(d.path, "44h/1h/0h/0");
    assert!(
        json[2]["desc"]
            .as_str()
            .unwrap()
            .contains("/9h/1h/4h/0h/0/*")
    );

    // The exported dump imports into another engine as the same wallet.
    let e2 = engine(&dir.path().join("b"));
    let s2 = open(&e2, None);
    let r = e2
        .block_on(s2.import_dump_wallet(dump_path, ImportOptions::default()))
        .unwrap();
    assert_eq!(r.wallet_id, id);
    assert_eq!(r.keys_not_imported, 0);
}

#[test]
fn test_qt_109_strict_non_english_phrase_is_not_core_compatible() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, None);
    let phrase = dw_vault::mnemonic::generate(12, dw_vault::mnemonic::Language::Spanish).unwrap();
    let id = e
        .block_on(s.import_wallet(phrase, Zeroizing::new(Vec::new()), ImportOptions::default()))
        .unwrap();
    let c = e.block_on(s.core_mnemonic_compatibility(id)).unwrap();
    assert!(!c.core_compatible);
    assert_eq!(
        c.warnings,
        vec![dw_engine::ExportWarning::MnemonicNotCoreCompatible]
    );
}

fn wait_for<T>(mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "condition not reached");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn test_qt_110_backup_restores_into_another_vault() {
    const PHRASE: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("a"));
    let s = open(&e, Some(b"vault pw"));
    e.block_on(s.vault_op(|v| v.unlock(b"vault pw", UnlockScope::Full)))
        .unwrap();
    let id = e
        .block_on(s.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(b"p".to_vec()),
            ImportOptions {
                birth_height: Some(7),
                name: Some("Savings".into()),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let first = receive_addresses(&e, &s, id, 1).remove(0);
    e.block_on(s.save_address_book_entry(
        id,
        first.clone(),
        "mine é".into(),
        BookPurpose::Receive,
        false,
    ))
    .unwrap();

    let dest = dir.path().join("w.dwbackup");
    let with_pw =
        e.block_on(s.backup_wallet(id, dest.clone(), Some(Zeroizing::new(b"x".to_vec()))));
    assert!(
        matches!(with_pw, Err(EngineError::InvalidArgument(_))),
        "{with_pw:?}"
    );
    let info = e.block_on(s.backup_wallet(id, dest.clone(), None)).unwrap();
    assert!(!info.automatic && info.size_bytes > 0);
    let again = e.block_on(s.backup_wallet(id, dest.clone(), None));
    assert!(matches!(
        again,
        Err(EngineError::Backup(BackupFailure::DestinationUnwritable(_)))
    ));
    match inspect_wallet_file(&dest).unwrap() {
        WalletFileKind::DwBackup {
            network,
            wallet_count,
            format_version,
            ..
        } => {
            assert_eq!(network, Some(DashNetwork::Regtest));
            assert_eq!((wallet_count, format_version), (1, 1));
        }
        other => panic!("{other:?}"),
    }
    let locked = {
        s.lock_vault_sync().unwrap();
        e.block_on(s.backup_wallet(id, dir.path().join("locked.dwbackup"), None))
    };
    assert!(matches!(
        locked,
        Err(EngineError::Backup(BackupFailure::VaultLocked))
    ));

    // Another engine with an unencrypted vault.
    let e2 = engine(&dir.path().join("b"));
    let s2 = open(&e2, None);
    let none = e2.block_on(s2.restore_backup(dest.clone(), None));
    assert!(
        matches!(
            none,
            Err(EngineError::Backup(BackupFailure::PassphraseRequired))
        ),
        "{none:?}"
    );
    let wrong = e2.block_on(s2.restore_backup(dest.clone(), Some(Zeroizing::new(b"no".to_vec()))));
    assert!(
        matches!(
            wrong,
            Err(EngineError::Backup(BackupFailure::WrongPassphrase))
        ),
        "{wrong:?}"
    );
    let ids = e2
        .block_on(s2.restore_backup(dest.clone(), Some(Zeroizing::new(b"vault pw".to_vec()))))
        .unwrap();
    assert_eq!(ids, vec![id]);
    let w = s2.wallet_info(&id).unwrap();
    assert_eq!(w.name, "Savings");
    assert!(w.has_mnemonic);
    assert_eq!(w.birth_height, Some(7));
    let book = e2.block_on(s2.address_book(id, None, None)).unwrap();
    assert!(
        book.iter()
            .any(|b| b.address == first && b.label == "mine é")
    );
    let dup = e2.block_on(s2.restore_backup(dest, Some(Zeroizing::new(b"vault pw".to_vec()))));
    assert!(
        matches!(
            dup,
            Err(EngineError::Backup(BackupFailure::AlreadyExists(_)))
        ),
        "{dup:?}"
    );

    // An unencrypted vault's backup needs its own passphrase.
    let none = e2.block_on(s2.backup_wallet(id, dir.path().join("u.dwbackup"), None));
    assert!(matches!(
        none,
        Err(EngineError::Backup(BackupFailure::PassphraseRequired))
    ));
    e2.block_on(s2.backup_wallet(
        id,
        dir.path().join("u.dwbackup"),
        Some(Zeroizing::new(b"bk".to_vec())),
    ))
    .unwrap();
}

#[test]
fn test_qt_116_automatic_backups_rotate() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, None);
    let policy = s.backup_policy().unwrap();
    assert_eq!(policy.keep, 10);
    assert_eq!(policy.directory, s.data_dir().join("backups"));
    let created = e.block_on(s.create_wallet(12)).unwrap();
    let list = wait_for(|| {
        let l = e
            .block_on(s.automatic_backups(Some(created.wallet_id)))
            .unwrap();
        (!l.is_empty()).then_some(l)
    });
    assert_eq!(list.len(), 1);
    assert!(list[0].automatic);
    assert_eq!(list[0].wallet_id, created.wallet_id);
    // The automatic backup of an unencrypted vault opens only in that vault.
    assert!(matches!(
        inspect_wallet_file(&list[0].path).unwrap(),
        WalletFileKind::DwBackup { .. }
    ));
    assert!(matches!(
        e.block_on(s.set_backup_policy(11)),
        Err(EngineError::InvalidArgument(_))
    ));
    e.block_on(s.set_backup_policy(0)).unwrap();
    assert!(e.block_on(s.automatic_backups(None)).unwrap().is_empty());
    assert_eq!(s.backup_policy().unwrap().keep, 0);
}

#[test]
fn test_qt_105_core_lookahead_survives_a_restart() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, Some(b"pw"));
    let id = e
        .block_on(s.import_dump_wallet(
            testdata("dumpwallet/dump_hd_nopass.txt"),
            ImportOptions::default(),
        ))
        .unwrap()
        .wallet_id;
    let before = receive_addresses(&e, &s, id, 2000).len();
    assert!(before >= 1000, "{before}");
    e.block_on(e.close_network(DashNetwork::Regtest)).unwrap();
    // Same data directory, new session: the stored lookahead is applied again.
    let s = e
        .block_on(e.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    assert_eq!(receive_addresses(&e, &s, id, 2000).len(), before);
    assert!(chain_addresses(&e, &s, id, AddressChain::Change, 2000).len() >= 1000);
}

const PHRASE_A: &str =
    "legal winner thank year wave sausage worth useful legal winner thank yellow";
const PHRASE_B: &str =
    "letter advice cage absurd amount doctor acoustic avoid letter advice cage above";

fn import(e: &Engine, s: &Arc<NetworkSession>, phrase: &str, name: &str) -> WalletId {
    e.block_on(s.import_wallet(
        Zeroizing::new(phrase.as_bytes().to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions {
            birth_height: Some(3),
            name: Some(name.into()),
            ..ImportOptions::default()
        },
    ))
    .unwrap()
}

/// A `.dwbackup` file: (header line, body line).
fn backup_lines(path: &Path) -> (Vec<u8>, serde_json::Value) {
    let bytes = std::fs::read(path).unwrap();
    let mut lines = bytes.split(|b| *b == b'\n');
    assert_eq!(lines.next().unwrap(), b"DWBACKUP 1");
    let header = lines.next().unwrap().to_vec();
    let body = serde_json::from_slice(lines.next().unwrap()).unwrap();
    (header, body)
}

/// Review M4: a one-wallet backup carries that wallet's data only; the
/// whole-network wallet.sqlite snapshot (other wallets' xpubs and history)
/// is gone.
#[test]
fn a_backup_holds_only_its_wallets_data() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(dir.path());
    let s = open(&e, None);
    let a = import(&e, &s, PHRASE_A, "A");
    let b = import(&e, &s, PHRASE_B, "B");
    let dest = dir.path().join("a.dwbackup");
    e.block_on(s.backup_wallet(a, dest.clone(), Some(Zeroizing::new(b"bk".to_vec()))))
        .unwrap();
    let (header, body) = backup_lines(&dest);
    let bundle: dw_vault::WalletBackupBundle =
        serde_json::from_value(body["bundles"][0].clone()).unwrap();
    let (_, payload) = s
        .vault()
        .open_backup_bundle(&bundle, Some(b"bk"), None, &header)
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
    assert!(json.get("wallet_sqlite").is_none(), "{json}");
    assert_eq!(json["name"], "A");
    let text = String::from_utf8(payload.to_vec()).unwrap();
    assert!(!text.contains(&b.to_string()), "another wallet's rows");
    assert!(text.contains(&a.to_string()));
    // No temporary copy is left (or made) in the network directory.
    let stray: Vec<_> = std::fs::read_dir(s.data_dir())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(".backup-"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

/// Writes a `.dwbackup` with `bundles` (wallet ids in the header in order)
/// and the given payload per bundle, sealed by `s`'s vault.
fn craft_backup(s: &Arc<NetworkSession>, path: &Path, wallets: &[(WalletId, serde_json::Value)]) {
    let header = serde_json::json!({
        "format": "dwbackup", "format_version": 1, "network": "regtest",
        "created_at": 1, "wallet_ids": wallets.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>(),
        "automatic": false, "app_version": "test",
    });
    let header_line = serde_json::to_vec(&header).unwrap();
    let bundles: Vec<_> = wallets
        .iter()
        .map(|(id, payload)| {
            s.vault()
                .backup_bundle(
                    &id.0,
                    Some(b"bk"),
                    &serde_json::to_vec(payload).unwrap(),
                    &header_line,
                )
                .unwrap()
        })
        .collect();
    let mut out = b"DWBACKUP 1\n".to_vec();
    out.extend_from_slice(&header_line);
    out.push(b'\n');
    out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": bundles })).unwrap());
    out.push(b'\n');
    std::fs::write(path, out).unwrap();
}

/// Review L3: a restore that fails after registering a wallet removes what
/// it registered, so a failure leaves no half-restored wallet behind.
#[test]
fn a_failed_restore_rolls_back_the_wallets_it_registered() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("src"));
    let s = open(&e, None);
    let a = import(&e, &s, PHRASE_A, "A");
    let b = import(&e, &s, PHRASE_B, "B");
    let good = |name: &str| serde_json::json!({"name": name, "birth_height": 0, "created_at": null, "app_rows": []});

    let e2 = engine(&dir.path().join("dst"));
    let s2 = open(&e2, None);
    let pw = || Some(Zeroizing::new(b"bk".to_vec()));
    let gone = |id: &WalletId| s2.wallet_info(id).is_err() && !s2.vault().has_wallet_secret(&id.0);

    // Multi-bundle: A restores, then B's bundle fails (A twice).
    let multi = dir.path().join("multi.dwbackup");
    craft_backup(
        &s,
        &multi,
        &[(a, good("A")), (b, good("B")), (a, good("A"))],
    );
    let r = e2.block_on(s2.restore_backup(multi, pw()));
    assert!(
        matches!(r, Err(EngineError::Backup(BackupFailure::AlreadyExists(id))) if id == a),
        "{r:?}"
    );
    assert!(
        gone(&a) && gone(&b),
        "a failed restore left wallets registered"
    );

    // A bad app row value fails the restore; nothing stays registered.
    let bad = dir.path().join("bad.dwbackup");
    let rows = serde_json::json!({"name": "A", "birth_height": 0, "created_at": null,
        "app_rows": [{"table": "wallet_names", "columns": ["wallet_id"], "rows": [[{"x": 1}]]}]});
    craft_backup(&s, &bad, &[(a, rows)]);
    let r = e2.block_on(s2.restore_backup(bad, pw()));
    assert!(
        matches!(r, Err(EngineError::Backup(BackupFailure::Corrupt(_)))),
        "{r:?}"
    );
    assert!(gone(&a));

    // The same wallets restore once the file is good.
    let ok = dir.path().join("ok.dwbackup");
    craft_backup(&s, &ok, &[(a, good("A")), (b, good("B"))]);
    assert_eq!(
        e2.block_on(s2.restore_backup(ok, pw())).unwrap(),
        vec![a, b]
    );
    assert_eq!(s2.wallet_info(&b).unwrap().name, "B");
}

/// A restore that fails after it stored the seed of a wallet already there
/// (watch-only) puts back that wallet's settings: after a reopen it has its
/// earlier range and no CoinJoin salt from the bundle.
#[test]
fn a_failed_restore_puts_back_the_settings_of_a_wallet_already_there() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("src"));
    let s = open(&e, None);
    let a = import(&e, &s, PHRASE_A, "A");
    let rows = serde_json::json!({"name": "A", "birth_height": 0, "created_at": null,
    "app_rows": [{"table": "settings_kv", "columns": ["scope", "key", "value"], "rows": [
        [{"t": a.to_string()}, {"t": "bip44.legacy_issued"}, {"t": "1001,2600"}],
        [{"t": a.to_string()}, {"t": "coinjoin.salt"}, {"t": CORE_SALT}],
    ]}]});
    let again =
        serde_json::json!({"name": "A", "birth_height": 0, "created_at": null, "app_rows": []});
    let multi = dir.path().join("multi.dwbackup");
    craft_backup(&s, &multi, &[(a, rows), (a, again)]);

    let e2 = engine(&dir.path().join("dst"));
    let s2 = open(&e2, None);
    assert_eq!(import(&e2, &s2, PHRASE_A, "A"), a);
    app_db(&s2)
        .execute(
            "INSERT INTO settings_kv (scope, key, value) VALUES (?1, 'bip44.legacy_issued', '3,2')",
            [a.to_string()],
        )
        .unwrap();
    assert_eq!(stored_setting(&s2, a, "coinjoin.salt"), None);
    assert!(s2.vault().delete_wallet_secret(&a.0).unwrap());

    let r = e2.block_on(s2.restore_backup(multi, Some(Zeroizing::new(b"bk".to_vec()))));
    assert!(
        matches!(r, Err(EngineError::Backup(BackupFailure::AlreadyExists(id))) if id == a),
        "{r:?}"
    );
    assert!(!s2.vault().has_wallet_secret(&a.0));
    let as_before = |s: &Arc<NetworkSession>| {
        assert_eq!(
            stored_setting(s, a, "bip44.legacy_issued").as_deref(),
            Some("3,2")
        );
        assert_eq!(stored_setting(s, a, "coinjoin.salt"), None);
    };
    as_before(&s2);
    drop(s2);
    e2.block_on(e2.close_network(DashNetwork::Regtest)).unwrap();
    let s2 = session(&e2, DashNetwork::Regtest);
    as_before(&s2);
    assert!(!reaches_index_1000(&e2, &s2, a));
}

/// Fix-review L1: a restore schedules the automatic backups only after
/// every bundle succeeded, so a failed (rolled back) restore leaves no
/// automatic backup behind, and a good one backs up every restored wallet.
#[test]
fn test_qt_116_failed_restore_leaves_no_automatic_backup() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("src"));
    let s = open(&e, None);
    let a = import(&e, &s, PHRASE_A, "A");
    let b = import(&e, &s, PHRASE_B, "B");
    let good = |name: &str| serde_json::json!({"name": name, "birth_height": 0, "created_at": null, "app_rows": []});

    let e2 = engine(&dir.path().join("dst"));
    let s2 = open(&e2, None);
    let pw = || Some(Zeroizing::new(b"bk".to_vec()));

    // A registers, then the second copy of A fails: everything rolls back.
    let multi = dir.path().join("multi.dwbackup");
    craft_backup(&s, &multi, &[(a, good("A")), (a, good("A"))]);
    let r = e2.block_on(s2.restore_backup(multi, pw()));
    assert!(
        matches!(r, Err(EngineError::Backup(BackupFailure::AlreadyExists(_)))),
        "{r:?}"
    );
    // Any wrongly scheduled backup would run within this window.
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        let list = e2.block_on(s2.automatic_backups(None)).unwrap();
        assert!(list.is_empty(), "a failed restore left {list:?}");
        std::thread::sleep(Duration::from_millis(50));
    }

    let ok = dir.path().join("ok.dwbackup");
    craft_backup(&s, &ok, &[(a, good("A")), (b, good("B"))]);
    assert_eq!(
        e2.block_on(s2.restore_backup(ok, pw())).unwrap(),
        vec![a, b]
    );
    let list = wait_for(|| {
        let l = e2.block_on(s2.automatic_backups(None)).unwrap();
        (l.len() == 2).then_some(l)
    });
    let mut ids: Vec<_> = list.iter().map(|i| i.wallet_id).collect();
    ids.sort_by_key(|id| id.to_string());
    let mut want = vec![a, b];
    want.sort_by_key(|id| id.to_string());
    assert_eq!(ids, want);
}

/// Fix-review L5: a bundle version this build does not read is
/// `backup.unsupported_version`, not `backup.corrupt`, even when the newer
/// bundle no longer parses as this build's bundle; nothing is registered.
#[test]
fn test_qt_110_unknown_bundle_version_is_unsupported() {
    let dir = dw_testutil::private_tempdir();
    let e = engine(&dir.path().join("src"));
    let s = open(&e, None);
    let a = import(&e, &s, PHRASE_A, "A");
    let good =
        serde_json::json!({"name": "A", "birth_height": 0, "created_at": null, "app_rows": []});
    let path = dir.path().join("a.dwbackup");
    craft_backup(&s, &path, &[(a, good)]);
    let (header, body) = backup_lines(&path);
    let write = |bundle: serde_json::Value| {
        let mut out = b"DWBACKUP 1\n".to_vec();
        out.extend_from_slice(&header);
        out.push(b'\n');
        out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": [bundle] })).unwrap());
        out.push(b'\n');
        let p = dir.path().join("v3.dwbackup");
        std::fs::write(&p, out).unwrap();
        p
    };

    let e2 = engine(&dir.path().join("dst"));
    let s2 = open(&e2, None);
    let pw = || Some(Zeroizing::new(b"bk".to_vec()));
    let mut same_shape = body["bundles"][0].clone();
    same_shape["version"] = 3.into();
    let new_shape = serde_json::json!({"version": 3, "wallet_id": a.to_string(), "sealed": "00"});
    for bundle in [same_shape, new_shape] {
        let r = e2.block_on(s2.restore_backup(write(bundle), pw()));
        assert!(
            matches!(
                r,
                Err(EngineError::Backup(BackupFailure::UnsupportedVersion(3)))
            ),
            "{r:?}"
        );
        assert!(s2.wallet_info(&a).is_err());
    }
    // A bundle without a version is corrupt.
    let mut unversioned = body["bundles"][0].clone();
    unversioned.as_object_mut().unwrap().remove("version");
    let r = e2.block_on(s2.restore_backup(write(unversioned), pw()));
    assert!(
        matches!(r, Err(EngineError::Backup(BackupFailure::Corrupt(_)))),
        "{r:?}"
    );
}
