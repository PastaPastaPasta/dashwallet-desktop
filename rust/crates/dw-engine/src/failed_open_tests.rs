//! Q104 104-F1 and Q86 (b): an open that fails once its wallet database is
//! open unloads no wallet and deletes none of its rows. At the platform pin
//! that releases the database once its manager drops (P16), nothing holds
//! it after the failed open, so the next open in this process opens, with
//! the same wallets, balances and history.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashcore::{Address, Block, BlockHash, OutPoint, Transaction, TxIn, TxOut, Txid};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

use crate::history::{HistoryFilter, HistoryQuery, HistorySort};
use crate::session::failed_open_probe;
use crate::session::{FAIL_HISTORY_LOAD_IN, FAIL_LOAD_IN};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletBalances, WalletId,
};

const PAID: u64 = 25 * 100_000_000;

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
    pay(engine, &session, wallet);
    (session, wallet)
}

/// Pays `wallet` [`PAID`] in a block at height 1 that its scan reached, as
/// dash-spv delivers it, and commits what the wallet stored of it.
fn pay(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    use dash_spv::EventHandler;
    use platform_wallet::changeset::PlatformWalletPersistence;
    use platform_wallet::key_wallet_manager::{WalletEvent, WalletInterface};
    let to = engine
        .block_on(session.current_receive_address(wallet))
        .unwrap()
        .address
        .parse::<Address<dashcore::address::NetworkUnchecked>>()
        .unwrap()
        .assume_checked();
    let payment = Transaction {
        version: 2,
        lock_time: 0,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
            ..Default::default()
        }],
        output: vec![TxOut {
            value: PAID,
            script_pubkey: to.script_pubkey(),
        }],
        special_transaction_payload: None,
    };
    let mut block = Block {
        header: dashcore::blockdata::block::Header {
            version: dashcore::blockdata::block::Version::from_consensus(1),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: dashcore::TxMerkleNode::all_zeros(),
            time: 1_793_598_712,
            bits: dashcore::CompactTarget::from_consensus(0x207f_ffff),
            nonce: 0,
        },
        txdata: vec![payment],
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    let manager = session.manager().unwrap();
    let events: Vec<WalletEvent> = engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let mut rx = wm.subscribe_events();
        wm.process_block_for_wallets(&block, block.block_hash(), 1, &BTreeSet::from([wallet.0]))
            .await;
        wm.update_wallet_synced_height(&wallet.0, 1);
        std::iter::from_fn(|| rx.try_recv().ok()).collect()
    });
    for event in &events {
        session.hub.on_wallet_event(event);
    }
    session.live().unwrap().persister.flush(wallet.0).unwrap();
}

/// What an open session shows: its loaded wallets, and `wallet`'s balances
/// and history `(txid, amount)`.
#[derive(Debug, PartialEq)]
pub(crate) struct Shown {
    wallets: BTreeSet<[u8; 32]>,
    balances: Option<WalletBalances>,
    history: Vec<(String, i64)>,
}

impl Shown {
    /// As a session shows it while the wallet's scan replays from its
    /// birth height: catching up.
    pub(crate) fn catching_up(mut self) -> Self {
        if let Some(b) = self.balances.as_mut() {
            b.catching_up = true;
        }
        self
    }
}

pub(crate) fn shown(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Shown {
    let page = engine
        .block_on(session.history_page(
            wallet,
            HistoryQuery {
                filter: HistoryFilter::default(),
                sort: HistorySort::NewestFirst,
                cursor: None,
                limit: 100,
            },
        ))
        .unwrap();
    Shown {
        wallets: (session.manager().unwrap().list_wallet_ids_blocking())
            .into_iter()
            .collect(),
        balances: session.balances(&wallet).unwrap(),
        history: (page.records.into_iter())
            .map(|r| (r.txid, r.amount))
            .collect(),
    }
}

/// `data_dir`'s wallet database files copied into a new directory, so a
/// read opens no database a session may be using.
fn wallet_db_copy(data_dir: &Path) -> (tempfile::TempDir, PathBuf) {
    let copy = dw_testutil::private_tempdir();
    let name = crate::session::WALLET_DB_FILE;
    for suffix in ["", "-wal", "-shm"] {
        let from = data_dir.join(format!("{name}{suffix}"));
        if from.exists() {
            std::fs::copy(&from, copy.path().join(format!("{name}{suffix}"))).unwrap();
        }
    }
    let db = copy.path().join(name);
    (copy, db)
}

/// `wallet`'s row count in each table of `data_dir`'s wallet database that
/// has a `wallet_id` column, read from a copy.
pub(crate) fn stored_rows(data_dir: &Path, wallet: WalletId) -> BTreeMap<String, i64> {
    let (_copy, db) = wallet_db_copy(data_dir);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) c \
             WHERE m.type = 'table' AND c.name = 'wallet_id' ORDER BY m.name",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let rows: BTreeMap<String, i64> = (tables.into_iter())
        .map(|table| {
            let count = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM \"{table}\" WHERE wallet_id = ?1"),
                    [&wallet.0[..]],
                    |r| r.get(0),
                )
                .unwrap();
            (table, count)
        })
        .collect();
    assert!(
        crate::store::registered_wallet_ids(&db)
            .unwrap()
            .contains(&wallet.0),
        "{wallet} is registered"
    );
    rows
}

/// After a failed open of `data_dir`: the open ended with its wallets
/// still loaded, so it unloaded none; `wallet` is still registered with at
/// least the rows it had (`rows`, read before the open); and the next open
/// in this process opens and shows what the session before it showed
/// (`shown_before`). Returns that open's session.
pub(crate) fn assert_kept_and_reopened(
    engine: &Engine,
    data_dir: &Path,
    wallet: WalletId,
    rows: &BTreeMap<String, i64>,
    shown_before: &Shown,
) -> Arc<NetworkSession> {
    assert_eq!(
        failed_open_probe::ended(data_dir),
        [(true, true)],
        "the failed open ended with its wallet loaded and released its wallet database"
    );
    assert!(
        !crate::session::held_after_failed_open(data_dir),
        "no failed open still holds the wallet database"
    );
    let kept = stored_rows(data_dir, wallet);
    assert_eq!(
        kept.keys().collect::<Vec<_>>(),
        rows.keys().collect::<Vec<_>>()
    );
    for (table, had) in rows {
        assert!(
            kept[table] >= *had,
            "{table}: {} of {had} rows kept",
            kept[table]
        );
    }
    assert_eq!(rows.get("wallets"), Some(&1), "{rows:?}");

    let session = open(engine).expect("the next open opens");
    assert_eq!(&shown(engine, &session, wallet), shown_before);
    session
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
/// The retry opens with the wallet as it was.
#[test]
fn a_failed_journal_open_keeps_its_wallets_and_the_retry_opens() {
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
    let before = shown(&engine, &session, wallet);
    drop((manager, session));
    close(&engine);
    let rows = stored_rows(&data_dir, wallet);

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

    assert_kept_and_reopened(&engine, &data_dir, wallet, &rows, &before);
    assert_eq!(failed_open_probe::seen(&data_dir), [(false, 1)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// An open that fails after its wallets loaded unloads none of them, and
/// the retry opens with them as they were.
#[test]
fn an_open_failing_after_the_wallets_load_keeps_them() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let (session, wallet) = imported(&engine);
    let data_dir = session.data_dir().to_path_buf();
    let before = shown(&engine, &session, wallet);
    assert_eq!(before.balances.map(|b| b.total), Some(PAID));
    assert_eq!(before.history.len(), 1, "{before:?}");
    drop(session);
    close(&engine);
    let rows = stored_rows(&data_dir, wallet);

    let failed = failing_in(&FAIL_LOAD_IN, &data_dir, || open(&engine));

    assert!(
        matches!(&failed, Err(EngineError::Storage(e)) if e == "test: load failed"),
        "{:?}",
        failed.err()
    );
    assert_kept_and_reopened(&engine, &data_dir, wallet, &rows, &before);
    engine.block_on(engine.shutdown()).unwrap();
}

/// An open whose history load fails unloads no wallet, and the retry opens
/// with it as it was.
#[test]
fn an_open_failing_to_load_the_history_keeps_its_wallets() {
    let root = dw_testutil::private_tempdir();
    let engine = engine(root.path());
    let (session, wallet) = imported(&engine);
    let data_dir = session.data_dir().to_path_buf();
    let before = shown(&engine, &session, wallet);
    assert_eq!(before.balances.map(|b| b.total), Some(PAID));
    drop(session);
    close(&engine);
    let rows = stored_rows(&data_dir, wallet);

    let failed = failing_in(&FAIL_HISTORY_LOAD_IN, &data_dir, || open(&engine));

    assert!(
        matches!(&failed, Err(EngineError::Storage(e)) if e == "test: history load failed"),
        "{:?}",
        failed.err()
    );
    assert_kept_and_reopened(&engine, &data_dir, wallet, &rows, &before);
    engine.block_on(engine.shutdown()).unwrap();
}

/// A failed open whose database stays held refuses the next open, not
/// retryable, while it is held. At this platform pin a failed open leaves
/// nothing holding it, so only a holder kept here shows the refusal. It
/// lasts only while the database is held: once its last holder lets go,
/// the next open opens.
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

/// A failed open through a symlinked root keeps its wallet, and an engine
/// reaching the database by its real path opens it as it was.
#[cfg(unix)]
#[test]
fn a_failed_open_through_a_symlinked_root_keeps_its_wallets() {
    let dir = dw_testutil::private_tempdir();
    let real = dir.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let linked = engine(&link);
    let (session, wallet) = imported(&linked);
    let data_dir = session.data_dir().to_path_buf();
    let before = shown(&linked, &session, wallet);
    drop(session);
    close(&linked);
    let rows = stored_rows(&data_dir, wallet);
    let failed = failing_in(&FAIL_LOAD_IN, &data_dir, || open(&linked));
    assert!(
        matches!(&failed, Err(EngineError::Storage(e)) if e == "test: load failed"),
        "{:?}",
        failed.err()
    );
    linked.block_on(linked.shutdown()).unwrap();
    drop(linked);

    let direct = engine(&real);
    assert_ne!(data_dir.canonicalize().unwrap(), data_dir);
    let session = assert_kept_and_reopened(&direct, &data_dir, wallet, &rows, &before);
    assert_eq!(
        session.data_dir().canonicalize().unwrap(),
        data_dir.canonicalize().unwrap()
    );
    direct.block_on(direct.shutdown()).unwrap();
}
