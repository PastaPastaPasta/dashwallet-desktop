//! Data roots under umask 002, the default of Ubuntu and Fedora desktops
//! (user-private groups): `mkdir` gives 0775 there, and SqlitePersister
//! refuses a database below any group-writable directory. The umask is
//! process-wide, so this file holds one test and its binary runs nothing else.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;

use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

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

    // An existing install whose network directory and app database an older
    // build left group-writable: the engine owns both and restricts them.
    chmod(&net, 0o775);
    chmod(&net.join(APP_DB_FILE), 0o664);
    open_regtest(&root).expect("an existing data root reopens");
    assert_eq!(mode(&net), 0o700);
    assert_eq!(mode(&net.join(APP_DB_FILE)), 0o600);

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
