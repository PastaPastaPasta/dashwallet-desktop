//! Q107 C1's way out of a store refused as damaged: the network starts over
//! in a new folder beside the refused one, with the restore's RESTORED mark
//! set, and the refused folder stays byte for byte as it was through the
//! whole setup. Every other refusal keeps its own way out. Its own test
//! binary: the probe is cached reading P9a Absent, so a retired contact-pool
//! row is a real refusal.

mod store_upgrade_common;

use std::path::Path;
use std::sync::{Arc, OnceLock};

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, KeyMaterial, NetworkSession,
    WalletId, WatchOnlyOptions,
};
use dw_store_upgrade::copy::WayOut;
use dw_store_upgrade::{Adopters, P9aStatus, RESTORED_MARK, UPGRADE_BACKUPS_DIR};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use store_upgrade_common::*;
use zeroize::Zeroizing;

const FIRST: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const SECOND: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// An engine on `root` whose OS store outlives it, as the keychain does.
fn engine_on(root: &Path, os_store: &Arc<MemoryOsStore>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: os_store.clone(),
                ..VaultConfig::default()
            },
        },
        Arc::new(NoEvents),
    )
    .unwrap()
}

/// The data root's entries, by name.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn restored_mark_set(dir: &Path) -> bool {
    dir.join(UPGRADE_BACKUPS_DIR).join(RESTORED_MARK).is_file()
}

/// Caches the probe once for this binary, reading P9a Absent.
fn probe_p9a_absent() {
    static PROBED: OnceLock<()> = OnceLock::new();
    PROBED.get_or_init(|| {
        let probe_root = dw_testutil::private_tempdir();
        Adopters::cache_with_p9a_for_test(probe_root.path(), P9aStatus::Absent);
    });
}

/// A store that can't be read as SQLite: its header overwritten, as
/// dw-store-upgrade's own damaged-store tests do.
fn damage_header(f: &Fixture, name: &str) {
    let path = f.net.join(name);
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[..100].fill(0x5a);
    std::fs::write(&path, bytes).unwrap();
}

/// The onboarding restore: a wallet from its recovery phrase.
fn restore(engine: &Engine, session: &Arc<NetworkSession>, phrase: &str) -> WalletId {
    engine
        .block_on(session.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap()
}

fn wallet_count(session: &NetworkSession) -> usize {
    session.wallet_load_states().unwrap().len()
}

/// Whether each way in but the recovery phrase is refused with
/// `RestoreOnly`: a new wallet, the file restores (a `.dwbackup`, a
/// dumpwallet file, a wallet.dat), key material and a watch-only wallet.
/// `missing` names no file and the key material and xpub are not valid, so
/// a way that is not refused fails on its input instead.
fn other_ways_in(engine: &Engine, session: &Arc<NetworkSession>, missing: &Path) -> [bool; 6] {
    let only = |r: Result<(), EngineError>| matches!(r, Err(EngineError::RestoreOnly));
    let path = || missing.to_path_buf();
    let options = ImportOptions::default;
    let seed = KeyMaterial::HdSeed(Zeroizing::new(vec![1]));
    [
        only(engine.block_on(session.create_wallet(12)).map(drop)),
        only(
            engine
                .block_on(session.restore_backup(path(), None))
                .map(drop),
        ),
        only(
            engine
                .block_on(session.import_dump_wallet(path(), options()))
                .map(drop),
        ),
        only(
            engine
                .block_on(session.import_wallet_dat(path(), None, options()))
                .map(drop),
        ),
        only(
            engine
                .block_on(session.import_key_material(seed, options()))
                .map(drop),
        ),
        only(
            engine
                .block_on(session.import_watch_only("xpub".into(), WatchOnlyOptions::default()))
                .map(drop),
        ),
    ]
}

/// Takes every permission off `dir` and returns its mode, or `None` when
/// this process reads it anyway (root, CAP_DAC_READ_SEARCH): then the
/// check proves nothing and the folder is left as it is.
#[cfg(unix)]
fn make_unreadable(dir: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o7777;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(dir).is_ok() {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        eprintln!("unreadable refused folder: skipped, this process reads it anyway");
        return None;
    }
    eprintln!("unreadable refused folder: yes");
    Some(mode)
}

#[test]
fn a_damaged_network_starts_over_in_a_new_folder_and_never_touches_the_refused_one() {
    probe_p9a_absent();
    let f = fixture("v0.1.0");
    damage_header(&f, "dispatch.sqlite");
    let os_store = Arc::new(MemoryOsStore::new());
    let pristine = tree(&f.net);

    let engine = engine_on(&f.root, &os_store);
    let not_yet = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(not_yet, Err(EngineError::InvalidArgument(_))),
        "only after a refused open: {not_yet:?}"
    );
    let refused = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .err()
        .expect("a damaged store is refused");
    match &refused {
        EngineError::StoreRefused(r) => {
            assert_eq!(r.cause, "SnapshotSourceDamaged");
            assert_eq!(r.screen.way_out, WayOut::Damaged);
        }
        other => panic!("not a store refusal: {other:?}"),
    }
    let before = tree(&f.net);
    assert_eq!(refused_open_diff(&pristine, &before), Vec::<String>::new());
    assert!(
        before.values().filter(|v| !v.ends_with(" dir")).count() >= 3,
        "every file in the refused folder is hashed: {before:?}"
    );
    let root_before = listing(&f.root);
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), f.net);

    let fresh = engine
        .block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(fresh, f.root.join("regtest-recovered-1"));
    assert_ne!(fresh, f.net);
    assert_eq!(
        tree_diff(&before, &tree(&f.net)),
        Vec::<String>::new(),
        "the refused folder is byte-identical, listing included"
    );
    let mut root_after = root_before.clone();
    root_after.extend(["network-folders.json".into(), "regtest-recovered-1".into()]);
    root_after.sort();
    assert_eq!(
        listing(&f.root),
        root_after,
        "nothing else in the data root"
    );
    assert_eq!(
        listing(&fresh),
        [UPGRADE_BACKUPS_DIR],
        "empty but for the mark"
    );
    assert_eq!(listing(&fresh.join(UPGRADE_BACKUPS_DIR)), [RESTORED_MARK]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), fresh);
    let again = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(again, Err(EngineError::InvalidArgument(_))),
        "once per refusal: {again:?}"
    );
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    // From here on nothing may read the refused folder.
    #[cfg(unix)]
    let locked = make_unreadable(&f.net);

    // The restart: the new folder opens, empty, with the gate's mark; the
    // first wallet is restored from its phrase, and a second through Add a
    // wallet, both under the same mark.
    let engine = engine_on(&f.root, &os_store);
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), fresh);
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("the new folder opens");
    assert_eq!(session.data_dir(), fresh.as_path());
    assert_eq!(wallet_count(&session), 0);
    assert!(restored_mark_set(&fresh), "the first open keeps the mark");
    assert!(
        dw_engine::store_upgrade::restored(session.data_dir()),
        "NetworkInfo.restored reads true on the first open"
    );
    let mark = dw_store_upgrade::restored_mark(&fresh)
        .expect("the gate reads the mark")
        .expect("the money gate is closed");
    assert!(mark.pending.is_empty(), "no payment to hold: {mark:?}");
    let busy = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(busy, Err(EngineError::InvalidArgument(_))),
        "never while open: {busy:?}"
    );
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let missing = f.root.join("no-such-file");
    assert_eq!(
        other_ways_in(&engine, &session, &missing),
        [true; 6],
        "under the mark a wallet comes only from its recovery phrase"
    );
    assert_eq!(wallet_count(&session), 0);
    let first = restore(&engine, &session, FIRST);
    let second = restore(&engine, &session, SECOND);
    assert_ne!(first, second);
    assert_eq!(wallet_count(&session), 2);
    assert!(
        restored_mark_set(&fresh),
        "the mark covers every restored wallet"
    );
    let _ = engine.block_on(session.start_spv());
    let listed = engine.block_on(engine.existing_networks()).unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|n| (n.network.clone(), n.directory.clone()))
            .collect::<Vec<_>>(),
        [(DashNetwork::Regtest, fresh.to_string_lossy().into_owned())],
        "listed by its new folder, never the refused one"
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    // The second restart opens the new folder again, never the refusal.
    let engine = engine_on(&f.root, &os_store);
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("the set-up folder opens again");
    assert_eq!(session.data_dir(), fresh.as_path());
    assert_eq!(wallet_count(&session), 2);
    assert!(restored_mark_set(&fresh), "held until its first full sync");
    assert_eq!(other_ways_in(&engine, &session, &missing), [true; 6]);
    assert_eq!(wallet_count(&session), 2);

    // The mark removed directly (not by the clear rule): every way in is back.
    std::fs::remove_file(fresh.join(UPGRADE_BACKUPS_DIR).join(RESTORED_MARK)).unwrap();
    assert!(!dw_engine::store_upgrade::restored(session.data_dir()));
    assert_eq!(
        other_ways_in(&engine, &session, &missing),
        [false; 6],
        "no way in is refused once the mark is gone"
    );
    assert_eq!(wallet_count(&session), 3, "the new wallet was added");
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    #[cfg(unix)]
    if let Some(mode) = locked {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f.net, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    assert_eq!(
        tree_diff(&before, &tree(&f.net)),
        Vec::<String>::new(),
        "the refused folder is byte-identical after the whole setup: every name, mode and hash"
    );
    assert_eq!(
        refused_open_diff(&pristine, &tree(&f.net)),
        Vec::<String>::new(),
        "only the refused open's own snapshot was ever added"
    );
}

/// Every refusal but a damaged store keeps its own way out: recover is
/// refused, and nothing is written in the data root.
#[test]
fn every_other_refusal_is_refused_for_recover() {
    probe_p9a_absent();
    let cases: [(&str, &str, &str); 8] = [
        (
            "v0.1.0",
            "INSERT INTO dw_fork_schema_history VALUES (6, 'later', 'x', '1')",
            "ForkStoreUnsupported",
        ),
        (
            "v0.1.0",
            "ALTER TABLE core_address_pool_retired ADD COLUMN extra INTEGER",
            "ForkStoreShapeMismatch",
        ),
        (
            "v0.1.0",
            "INSERT INTO core_transactions (wallet_id, txid, finalized, record_blob) \
             SELECT wallet_id, randomblob(32), 0, x'00' FROM wallets LIMIT 1",
            "UpgradeRehearsalFailed",
        ),
        (
            "v0.1.0",
            "DROP TABLE dw_fork_schema_history; \
             INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
             VALUES (19, 'dashpay_external_rotation', 'x', '1')",
            "PreHistoryFork",
        ),
        (
            "v0.3.0",
            "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
             VALUES (999, 'from_a_newer_build', 'x', '1')",
            "SchemaTooNew",
        ),
        (
            "v0.1.0",
            "UPDATE refinery_schema_history SET checksum = '1' WHERE version = 1",
            "SchemaHistoryMismatch",
        ),
        ("v0.1.0", "", "RetiredPoolWithoutAdopter"),
        ("v0.1.0", "-", "WalletFileUnsupported"),
    ];
    for (version, sql, cause) in cases {
        let f = fixture(version);
        match sql {
            "" => add_retired_row(&f.wallet()),
            "-" => {
                #[cfg(unix)]
                {
                    let moved = f.net.join("wallet.real");
                    std::fs::rename(f.wallet(), &moved).unwrap();
                    std::os::unix::fs::symlink(&moved, f.wallet()).unwrap();
                }
                #[cfg(not(unix))]
                continue;
            }
            sql => exec(&f.wallet(), sql),
        }
        let engine = engine(&f.root);
        let refused = engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .err()
            .expect("the open is refused");
        match &refused {
            EngineError::StoreRefused(r) => {
                assert_eq!(r.cause, cause);
                assert_ne!(r.screen.way_out, WayOut::Damaged, "{cause}");
            }
            other => panic!("{cause}: not a store refusal: {other:?}"),
        }
        let root_before = tree(&f.root);
        let recovered = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
        assert!(
            matches!(recovered, Err(EngineError::InvalidArgument(_))),
            "{cause}: {recovered:?}"
        );
        assert_eq!(
            tree_diff(&root_before, &tree(&f.root)),
            Vec::<String>::new(),
            "{cause}"
        );
        assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), f.net);
        engine.block_on(engine.shutdown()).unwrap();
    }
}

/// M1: an open refused as damaged offers no Restore earlier data, so a
/// request is refused and nothing in the refused folder is written.
#[test]
fn a_damaged_refusal_refuses_restore_earlier_data() {
    probe_p9a_absent();
    let f = fixture("v0.1.0");
    damage_header(&f, "dispatch.sqlite");
    let engine = engine(&f.root);
    match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
        Err(EngineError::StoreRefused(r)) => assert_eq!(r.screen.way_out, WayOut::Damaged),
        Err(other) => panic!("not refused as damaged: {other:?}"),
        Ok(_) => panic!("opened"),
    }
    let before = tree(&f.net);
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    assert!(
        matches!(got, Err(EngineError::InvalidArgument(_))),
        "{got:?}"
    );
    assert_eq!(tree_diff(&before, &tree(&f.net)), Vec::<String>::new());
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn damaged_wallet_precheck_offers_recovery() {
    probe_p9a_absent();
    let root = dw_testutil::private_tempdir();
    let net = root.path().join("regtest");
    dw_fs::create_owned_dir(root.path(), Path::new("regtest")).unwrap();
    let wallet = net.join("wallet.sqlite");
    let junk = [0x5a; 512];
    std::fs::write(&wallet, junk).unwrap();
    let engine = engine(root.path());
    match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
        Err(EngineError::StoreRefused(r)) => assert_eq!(r.screen.way_out, WayOut::Damaged),
        Err(other) => panic!("not refused as damaged: {other:?}"),
        Ok(_) => panic!("opened"),
    }
    let recovered = engine
        .block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest))
        .unwrap();
    assert!(restored_mark_set(&recovered));
    assert_eq!(std::fs::read(wallet).unwrap(), junk);
    assert!(
        engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .is_ok()
    );
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn missing_shielded_store_after_upgrade_offers_recovery() {
    probe_p9a_absent();
    let f = fixture("v0.3.0");
    let engine = engine(&f.root);
    let network = DashNetwork::Regtest;
    let session = engine
        .block_on(engine.open_network(network.clone(), local_opts()))
        .unwrap();
    drop(session);
    engine
        .block_on(engine.close_network(network.clone()))
        .unwrap();

    let shielded = f.net.join("shielded.sqlite");
    std::fs::remove_file(&shielded).unwrap();
    match engine.block_on(engine.open_network(network.clone(), local_opts())) {
        Err(EngineError::StoreRefused(r)) => assert_eq!(r.screen.way_out, WayOut::Damaged),
        Err(other) => panic!("not refused as damaged: {other:?}"),
        Ok(_) => panic!("opened without the shielded store"),
    }
    assert!(!shielded.exists());
    let recovered = engine
        .block_on(engine.recover_in_fresh_folder(network.clone()))
        .unwrap();
    assert!(restored_mark_set(&recovered));
    assert!(
        engine
            .block_on(engine.open_network(network, local_opts()))
            .is_ok()
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// A damaged refusal followed by an open that is not refused as damaged
/// (here, the store mended) takes the way out back.
#[test]
fn a_later_open_not_refused_as_damaged_withdraws_the_way_out() {
    probe_p9a_absent();
    let f = fixture("v0.1.0");
    let store = f.net.join("dispatch.sqlite");
    let good = std::fs::read(&store).unwrap();
    damage_header(&f, "dispatch.sqlite");
    let engine = engine(&f.root);
    assert!(matches!(
        engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())),
        Err(EngineError::StoreRefused(_))
    ));
    std::fs::write(&store, good).unwrap();
    add_retired_row(&f.wallet());
    let refused = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(
        matches!(&refused, Err(EngineError::StoreRefused(r)) if r.cause == "RetiredPoolWithoutAdopter"),
        "{:?}",
        refused.err()
    );
    let recovered = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(recovered, Err(EngineError::InvalidArgument(_))),
        "{recovered:?}"
    );
    assert!(!f.root.join("network-folders.json").exists());
    engine.block_on(engine.shutdown()).unwrap();
}

/// A damaged refusal followed by an open that fails before the store check
/// (the record names a folder that is gone) takes the way out back too:
/// only the last open counts.
#[test]
fn a_later_open_that_fails_on_its_folder_withdraws_the_way_out() {
    probe_p9a_absent();
    let f = fixture("v0.1.0");
    damage_header(&f, "dispatch.sqlite");
    let engine = engine(&f.root);
    assert!(matches!(
        engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())),
        Err(EngineError::StoreRefused(_))
    ));
    std::fs::write(
        f.root.join("network-folders.json"),
        br#"{"regtest":"regtest-recovered-1"}"#,
    )
    .unwrap();
    let opened = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(
        matches!(opened, Err(EngineError::Storage(_))),
        "{:?}",
        opened.err()
    );
    let recovered = engine.block_on(engine.recover_in_fresh_folder(DashNetwork::Regtest));
    assert!(
        matches!(recovered, Err(EngineError::InvalidArgument(_))),
        "{recovered:?}"
    );
    assert!(!f.root.join("regtest-recovered-1").exists());
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn a_damaged_folder_record_fails_every_open_closed() {
    probe_p9a_absent();
    let f = fixture("v0.1.0");
    std::fs::write(f.root.join("network-folders.json"), b"{\"regtest\": ").unwrap();
    let engine = engine(&f.root);
    assert!(matches!(
        engine.network_dir(&DashNetwork::Regtest),
        Err(EngineError::Storage(_))
    ));
    let opened = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(
        matches!(opened, Err(EngineError::Storage(_))),
        "never the default folder: {:?}",
        opened.err()
    );
    assert!(matches!(
        engine.block_on(engine.existing_networks()),
        Err(EngineError::Storage(_))
    ));
    engine.block_on(engine.shutdown()).unwrap();
}
