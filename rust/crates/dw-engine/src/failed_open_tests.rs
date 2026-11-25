//! Q104 104-F1 and Q86 (b): an open that fails once its wallet database is
//! open unloads no wallet. A loaded wallet keeps the database held, so the
//! next open in this process is refused, not retryable, while it is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

use crate::session::failed_open_probe;
use crate::session::{FAIL_HISTORY_LOAD_IN, FAIL_LOAD_IN, KEPT_AFTER_FAILED_OPEN_REFUSAL};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
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
        Arc::new(NoEvents),
    )
    .unwrap()
}

fn open(engine: &Engine) -> Result<Arc<NetworkSession>, EngineError> {
    engine.block_on(engine.open_network(
        DashNetwork::Regtest,
        SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        },
    ))
}

fn close(engine: &Engine) {
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// An open session with a vault and one imported wallet.
fn imported(engine: &Engine) -> (Arc<NetworkSession>, WalletId) {
    let session = open(engine).unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                    .as_bytes()
                    .to_vec(),
            ),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    (session, wallet)
}

/// The wallet ids registered in `data_dir`'s wallet database, read from a
/// copy so the held database is not opened again.
fn registered_on_copy(data_dir: &Path) -> Vec<[u8; 32]> {
    let copy = dw_testutil::private_tempdir();
    let name = crate::session::WALLET_DB_FILE;
    for suffix in ["", "-wal", "-shm"] {
        let from = data_dir.join(format!("{name}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, copy.path().join(format!("{name}{suffix}"))).unwrap();
        }
    }
    crate::store::registered_wallet_ids(&copy.path().join(name)).unwrap()
}

/// After a failed open: `wallet` is still registered, the wallet database
/// is still held, and the next open is refused, not retryable.
pub(crate) fn assert_held(engine: &Engine, data_dir: &Path, wallet: WalletId) {
    assert!(registered_on_copy(data_dir).contains(&wallet.0));
    let refused = open(engine).err().expect("the store is still held");
    assert!(
        matches!(
            &refused,
            EngineError::StorageInUse { detail, retryable: false }
                if detail == KEPT_AFTER_FAILED_OPEN_REFUSAL
        ),
        "{refused:?}"
    );
    assert_eq!(refused.code(), "storage_in_use");
    let held = crate::store_upgrade::open_persister(
        data_dir,
        crate::store_upgrade::persister_config(data_dir),
    );
    assert!(
        matches!(held, Err(EngineError::StorageInUse { .. })),
        "the wallet database is still held: {:?}",
        held.err()
    );
    assert!(registered_on_copy(data_dir).contains(&wallet.0));
}

fn failing_in<T>(
    list: &std::sync::Mutex<Vec<PathBuf>>,
    data_dir: &Path,
    f: impl FnOnce() -> T,
) -> T {
    list.lock().unwrap().push(data_dir.to_path_buf());
    let out = f();
    list.lock().unwrap().retain(|d| d != data_dir);
    out
}

/// Q86 F1: the failed journal open finds no shielded coordinator on its
/// manager, even when the session this process closed before it had one.
#[test]
fn a_failed_journal_open_keeps_its_wallets_and_refuses_the_retry_until_a_restart() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let (session, wallet) = imported(&engine);
    let data_dir = session.data_dir().to_path_buf();
    let manager = session.manager().unwrap();
    engine
        .block_on(
            manager.configure_shielded(
                data_dir.join(crate::platform::shielded_pool::SHIELDED_DB_FILE),
            ),
        )
        .unwrap();
    assert!(engine.block_on(manager.shielded_coordinator()).is_some());
    drop((manager, session));
    close(&engine);

    let journal = data_dir.join(dw_appdb::dispatch::DISPATCH_DB_FILE);
    rusqlite::Connection::open(&journal)
        .unwrap()
        .execute(
            "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, status, at) \
             VALUES (?1, ?2, 'imported_redrive', ?3, 'unclassified', 1)",
            rusqlite::params![[0x5a_u8; 32], [0x77_u8; 32], [0x88_u8; 32]],
        )
        .unwrap();
    dw_appdb::fault::arm_fault(&journal, dw_appdb::fault::Fault::ClassifyWrite);
    let failed = open(&engine).err().expect("the journal read fails once");
    assert!(
        matches!(&failed, EngineError::Storage(e) if e.contains("imported shielded spends")),
        "{failed:?}"
    );
    assert_eq!(failed_open_probe::seen(&data_dir), [(false, 1)]);

    assert_held(&engine, &data_dir, wallet);
    assert_eq!(failed_open_probe::seen(&data_dir), [(false, 1)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// An open that fails after its wallets loaded unloads none of them.
#[test]
fn an_open_failing_after_the_wallets_load_keeps_them() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let (session, wallet) = imported(&engine);
    let data_dir = session.data_dir().to_path_buf();
    drop(session);
    close(&engine);

    let failed = failing_in(&FAIL_LOAD_IN, &data_dir, || open(&engine));

    assert!(
        matches!(&failed, Err(EngineError::Storage(e)) if e == "test: load failed"),
        "{:?}",
        failed.err()
    );
    assert_held(&engine, &data_dir, wallet);
    engine.block_on(engine.shutdown()).unwrap();
}

/// An open whose history load fails unloads no wallet.
#[test]
fn an_open_failing_to_load_the_history_keeps_its_wallets() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let (session, wallet) = imported(&engine);
    let data_dir = session.data_dir().to_path_buf();
    drop(session);
    close(&engine);

    let failed = failing_in(&FAIL_HISTORY_LOAD_IN, &data_dir, || open(&engine));

    assert!(
        matches!(&failed, Err(EngineError::Storage(e)) if e == "test: history load failed"),
        "{:?}",
        failed.err()
    );
    assert_held(&engine, &data_dir, wallet);
    engine.block_on(engine.shutdown()).unwrap();
}

/// The refusal lasts only while the failed open's database is held: once
/// its last holder lets go, the next open opens.
#[test]
fn the_refusal_clears_once_the_held_database_is_released() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let data_dir = open(&engine).unwrap().data_dir().to_path_buf();
    close(&engine);
    let (holder, _) = crate::store_upgrade::open_persister(
        &data_dir,
        crate::store_upgrade::persister_config(&data_dir),
    )
    .unwrap();
    engine.block_on(crate::session::release_after_failed_open(
        &data_dir,
        Arc::downgrade(&holder),
        false,
    ));

    let refused = open(&engine).err().expect("held by the failed open");
    assert!(
        matches!(
            &refused,
            EngineError::StorageInUse {
                retryable: false,
                ..
            }
        ),
        "{refused:?}"
    );
    drop(holder);

    open(&engine).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
}

/// The refusal follows the wallet database, not the spelling of its path:
/// an engine reaching it through a symlinked root is refused too.
#[cfg(unix)]
#[test]
fn the_refusal_holds_through_a_symlinked_root() {
    let dir = dw_testutil::private_tempdir();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let linked = engine(&link);
    let (session, wallet) = imported(&linked);
    let data_dir = session.data_dir().to_path_buf();
    drop(session);
    close(&linked);
    let failed = failing_in(&FAIL_LOAD_IN, &data_dir, || open(&linked));
    assert!(failed.is_err());
    linked.block_on(linked.shutdown()).unwrap();
    drop(linked);

    let direct = engine(&real);
    let real_dir = data_dir.canonicalize().unwrap();
    assert_ne!(real_dir, data_dir);
    assert_held(&direct, &real_dir, wallet);
    direct.block_on(direct.shutdown()).unwrap();
}
