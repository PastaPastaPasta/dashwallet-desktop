//! Data roots under umask 002, the default of Ubuntu and Fedora desktops
//! (user-private groups): `mkdir` gives 0775 there, and SqlitePersister
//! refuses a database below any group-writable directory. The umask is
//! process-wide, so this file holds one test and its binary runs nothing else.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, SessionOptions,
};
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

const APP_DB_FILE: &str = "app.sqlite";
const WALLET_DB_FILE: &str = "wallet.sqlite";

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn local_opts() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
    }
}

fn engine(root: &Path) -> Engine {
    engine_with(root, Arc::new(MemoryOsStore::new()))
}

/// An engine whose vault keeps its OS-store slot across restarts.
fn engine_with(root: &Path, os_store: Arc<MemoryOsStore>) -> Engine {
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
        Arc::new(NullSink),
    )
    .unwrap()
}

/// Opens regtest on `root`, then shuts the engine down.
fn open_regtest(root: &Path) -> Result<(), String> {
    let engine = engine(root);
    let result = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .map(drop)
        .map_err(|e| e.to_string());
    engine.block_on(engine.shutdown()).unwrap();
    result
}

/// Opens regtest on `root` and reports the session's avatar directory.
fn open_regtest_avatars(root: &Path) -> Result<Option<std::path::PathBuf>, String> {
    let engine = engine(root);
    let result = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .map(|session| session.avatars_dir())
        .map_err(|e| e.to_string());
    engine.block_on(engine.shutdown()).unwrap();
    result
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn data_roots_open_under_umask_002() {
    // SAFETY: umask only swaps the process file-mode mask and cannot fail.
    // Nothing else runs in this test binary.
    unsafe { libc::umask(0o002) };
    let probe = tempfile::tempdir().unwrap();
    assert_eq!(mode(probe.path()) & 0o020, 0o020, "umask 002 is in effect");
    drop(probe);

    let base = dw_testutil::private_tempdir();

    // A fresh root with missing parents (a new `~/.local/share/dashwallet`).
    let root = base.path().join("local/share/dashwallet");
    open_regtest(&root).expect("a fresh data root opens");
    let net = root.join("regtest");
    for dir in [base.path().join("local"), base.path().join("local/share")] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(&net), 0o700);
    assert_eq!(mode(&net.join(APP_DB_FILE)), 0o600);
    assert_eq!(mode(&net.join(WALLET_DB_FILE)), 0o600);
    assert_eq!(mode(&net.join("backups")), 0o700);
    assert_eq!(mode(&net.join("backups/auto")), 0o700);
    assert_eq!(mode(&net.join("avatars")), 0o700);

    // An existing install whose network directory and app database an older
    // build left group-writable: the engine owns both and restricts them.
    // The avatar directory is the engine's too.
    chmod(&net, 0o775);
    chmod(&net.join(APP_DB_FILE), 0o664);
    chmod(&net.join("avatars"), 0o775);
    open_regtest(&root).expect("an existing data root reopens");
    assert_eq!(mode(&net), 0o700);
    assert_eq!(mode(&net.join(APP_DB_FILE)), 0o600);
    assert_eq!(mode(&net.join("avatars")), 0o700);

    // A symlink swapped in for `avatars` is not followed to change a mode:
    // what it points at keeps its mode.
    let elsewhere = base.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    chmod(&elsewhere, 0o755);
    std::fs::remove_dir(net.join("avatars")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, net.join("avatars")).unwrap();
    open_regtest(&root).expect("a symlinked avatars directory opens");
    assert_eq!(mode(&elsewhere), 0o755);
    std::fs::remove_file(net.join("avatars")).unwrap();
    open_regtest(&root).expect("a missing avatars directory is recreated");
    assert_eq!(mode(&net.join("avatars")), 0o700);

    // The cache is disposable (review m4): an entry named `avatars` that is
    // not a directory disables avatars for the session, and the wallet opens.
    assert_eq!(open_regtest_avatars(&root), Ok(Some(net.join("avatars"))));
    std::fs::remove_dir(net.join("avatars")).unwrap();
    std::fs::write(net.join("avatars"), b"stray").unwrap();
    assert_eq!(open_regtest_avatars(&root), Ok(None));
    std::fs::remove_file(net.join("avatars")).unwrap();
    // A symlink that loops cannot be resolved.
    std::os::unix::fs::symlink(net.join("avatars"), net.join("avatars")).unwrap();
    assert_eq!(open_regtest_avatars(&root), Ok(None));
    std::fs::remove_file(net.join("avatars")).unwrap();
    assert_eq!(open_regtest_avatars(&root), Ok(Some(net.join("avatars"))));
    assert_eq!(mode(&net.join("avatars")), 0o700);

    // An install whose `backups` and `backups/auto` an older build left
    // group-writable (review D1-r2). The storage refuses to write an automatic
    // backup below a group-writable directory, so the engine restricts both
    // when it opens the network, before any storage operation needs one:
    // removing a wallet takes a pre-delete backup.
    let legacy = base.path().join("legacy");
    let store = Arc::new(MemoryOsStore::new());
    let wallet = {
        let engine = engine_with(&legacy, Arc::clone(&store));
        let s = engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .unwrap();
        engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
        let id = engine
            .block_on(s.import_wallet(
                Zeroizing::new(b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    ..ImportOptions::default()
                },
            ))
            .unwrap();
        engine.block_on(engine.shutdown()).unwrap();
        id
    };
    let backups = legacy.join("regtest/backups");
    chmod(&backups, 0o775);
    chmod(&backups.join("auto"), 0o775);
    let engine = engine_with(&legacy, store);
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("a legacy layout opens");
    assert_eq!(mode(&backups), 0o700);
    assert_eq!(mode(&backups.join("auto")), 0o700);
    let wipe = engine
        .block_on(
            s.vault_op(move |v| v.authorize(GrantPurpose::Wipe, Some(&wallet.0), Credential::None)),
        )
        .unwrap();
    engine
        .block_on(s.remove_wallet(wallet, wipe.id))
        .expect("the pre-delete backup is written");
    assert!(
        std::fs::read_dir(backups.join("auto")).unwrap().count() > 0,
        "storage wrote its automatic backup"
    );
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();

    // A root the user chose and left group-writable is not the engine's to
    // change: the open fails and names it.
    chmod(&root, 0o775);
    let err = open_regtest(&root).expect_err("a group-writable root is refused");
    assert!(
        err.contains(&root.display().to_string()) && err.contains("group/other-writable"),
        "{err}"
    );
    assert_eq!(mode(&root), 0o775);
    chmod(&root, 0o700);
}
