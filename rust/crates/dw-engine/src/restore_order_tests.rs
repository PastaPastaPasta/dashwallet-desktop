//! #359: a restore stores a wallet's backed-up app rows before its seed and
//! its registration, as a Dash Core file import stores its range, locks and
//! salt. A child process (this test binary, running [`import_child`]) is
//! stopped at an import fault point by `DW_FAULT`, which aborts it there; the
//! parent opens the same data folder again and checks what it finds.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_appdb::fault::Stage;
use dw_appdb::{BookPurpose, LabelKind, TableRows};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};
use zeroize::Zeroizing;

use crate::backup::BackupFailure;
use crate::keys::{IMPORT_REGISTER, IMPORT_ROWS, IMPORT_SECRET};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const OTHER_PHRASE: &[u8] = b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
const VAULT_PASS: &[u8] = b"vault pw";
const ROOT: &str = "DWD_RESTORE_ORDER_ROOT";
const OP: &str = "DWD_RESTORE_ORDER_OP";
const CHILD: &str = "restore_order_tests::import_child";
/// How long the child may take to reach its point (a cold debug build
/// opens the engine slowly).
const REACH: Duration = Duration::from_secs(180);
const TXID: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_TXID: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const SALT_KEY: &str = "coinjoin.salt";

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

/// An engine on `root` with its regtest session open and its vault, made
/// with [`VAULT_PASS`] if there is none, unlocked.
fn launch(root: &Path) -> (Engine, Arc<NetworkSession>) {
    let engine = Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    let made = engine.block_on(session.vault_op(|v| v.create(Some(VAULT_PASS))));
    if made.is_err() {
        engine
            .block_on(session.vault_op(|v| v.unlock(VAULT_PASS, UnlockScope::Full)))
            .unwrap();
    }
    (engine, session)
}

/// Shuts `engine` down, so the next [`launch`] on its data folder in this
/// process can open it: a dropped engine closes its sessions in the
/// background.
fn close(engine: Engine, session: Arc<NetworkSession>) {
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
}

fn import_phrase(engine: &Engine, session: &Arc<NetworkSession>, phrase: &[u8]) -> WalletId {
    engine
        .block_on(session.import_wallet(
            Zeroizing::new(phrase.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..Default::default()
            },
        ))
        .unwrap()
}

/// A coin lock, CoinJoin salt, label and address-book entry of `id`.
fn store_rows(engine: &Engine, session: &NetworkSession, id: WalletId, txid: &'static str, n: u8) {
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| {
            db.lock_manual(&scope, txid, 3, 7)?;
            db.set_setting(&scope, SALT_KEY, Some(&format!("{n:02x}").repeat(32)))?;
            db.set_label(&scope, LabelKind::Tx, txid, Some(&format!("note {n}")), 8)?;
            db.upsert_book_entry(&scope, "yAddr", BookPurpose::Send, &format!("payee {n}"), 9)
        }))
        .unwrap();
}

/// A backup at `dest` of `phrase`'s wallet holding [`store_rows`]` (txid, n)`,
/// made in its own data folder; the wallet's id.
fn backup_of(dir: &Path, phrase: &[u8], txid: &'static str, n: u8, dest: &Path) -> WalletId {
    let (engine, session) = launch(&dir.join(format!("src-{n}")));
    let id = import_phrase(&engine, &session, phrase);
    store_rows(&engine, &session, id, txid, n);
    engine
        .block_on(session.backup_wallet(id, dest.to_path_buf(), None))
        .unwrap();
    close(engine, session);
    id
}

fn restore(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    backup: &Path,
) -> Result<Vec<WalletId>, EngineError> {
    engine.block_on(session.restore_backup(
        backup.to_path_buf(),
        Some(Zeroizing::new(VAULT_PASS.to_vec())),
    ))
}

/// The coin locks, salt, labels and address book of `id`.
#[derive(Debug, Clone, PartialEq)]
struct Rows {
    locks: Vec<(String, u32)>,
    salt: Option<String>,
    labels: Vec<TableRows>,
}

fn rows(engine: &Engine, session: &NetworkSession, id: WalletId) -> Rows {
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| {
            let locks = (db.locks(&scope)?.into_iter())
                .map(|l| (l.txid, l.vout))
                .collect();
            let salt = db.setting(&scope, SALT_KEY)?;
            let labels = (db.export_wallet_rows(&scope)?.into_iter())
                .filter(|t| t.table == "labels" || t.table == "address_book")
                .collect();
            Ok(Rows {
                locks,
                salt,
                labels,
            })
        }))
        .unwrap()
}

/// The label and address-book rows of `rows` (two labels, one entry from
/// [`store_rows`]).
fn label_rows(rows: &Rows) -> usize {
    rows.labels.iter().map(|t| t.rows.len()).sum()
}

fn no_rows() -> Rows {
    Rows {
        locks: vec![],
        salt: None,
        labels: vec![],
    }
}

fn registered(engine: &Engine, session: &NetworkSession, id: WalletId) -> bool {
    engine
        .block_on(session.manager().unwrap().get_wallet(&id.0))
        .is_some()
}

/// What `backup` restores to in a data folder of its own.
fn restored_rows(dir: &Path, backup: &Path, id: WalletId) -> Rows {
    let (engine, session) = launch(&dir.join("control"));
    assert_eq!(restore(&engine, &session, backup).unwrap(), vec![id]);
    let rows = rows(&engine, &session, id);
    assert!(rows.locks.len() == 1 && rows.salt.is_some() && label_rows(&rows) == 3);
    rows
}

/// Run by [`stopped_at`] in a child process: `DWD_RESTORE_ORDER_OP`
/// (`restore:<file>` or `core:<file>`) into `DWD_RESTORE_ORDER_ROOT`, which
/// `DW_FAULT` aborts at its point.
#[test]
#[ignore = "the child process of the restore order kill tests"]
fn import_child() {
    let (Some(root), Ok(op)) = (std::env::var_os(ROOT), std::env::var(OP)) else {
        return;
    };
    let (engine, session) = launch(Path::new(&root));
    let done = match op.split_once(':') {
        Some(("restore", file)) => restore(&engine, &session, Path::new(file)).map(|_| ()),
        Some(("core", file)) => engine
            .block_on(session.import_wallet_dat(file.into(), None, ImportOptions::default()))
            .map(|_| ()),
        _ => panic!("no operation {op:?}"),
    };
    panic!("not stopped: {done:?}");
}

/// Runs `op` in [`import_child`] at `root` until `DW_FAULT` aborts it after
/// `point`.
fn stopped_at(root: &Path, op: &str, point: &str) {
    std::fs::create_dir_all(root).unwrap();
    let log_path = root.with_extension("log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([CHILD, "--exact", "--ignored", "--test-threads=1"])
        .env(ROOT, root)
        .env(OP, op)
        .env("DW_FAULT", dw_appdb::fault::name(point, Stage::After))
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > REACH {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{op} never reached {point}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let log = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert_eq!(
        status.signal(),
        Some(libc::SIGABRT),
        "{op} was not stopped at {point} ({status:?}): {log}"
    );
}

fn restore_op(backup: &Path) -> String {
    format!("restore:{}", backup.display())
}

/// A restore stopped once its rows are stored leaves the wallet with them,
/// unregistered and without its seed; the restore run again succeeds with
/// the backup's rows.
#[test]
fn a_restore_stopped_after_its_rows_restores_fully_when_run_again() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let root = dir.path().join("data");

    stopped_at(&root, &restore_op(&backup), IMPORT_ROWS);

    let (engine, session) = launch(&root);
    assert!(!registered(&engine, &session, id));
    assert!(!session.vault().has_wallet_secret(&id.0));
    assert_eq!(
        rows(&engine, &session, id),
        expected,
        "stored before the seed"
    );
    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);
    assert_eq!(rows(&engine, &session, id), expected);
}

/// A restore stopped once its seed is stored, before it registers the
/// wallet, has stored its rows already; run again, it succeeds.
#[test]
fn a_restore_stopped_after_its_seed_restores_fully_when_run_again() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let root = dir.path().join("data");

    stopped_at(&root, &restore_op(&backup), IMPORT_SECRET);

    let (engine, session) = launch(&root);
    assert!(!registered(&engine, &session, id));
    assert!(session.vault().has_wallet_secret(&id.0));
    assert_eq!(
        rows(&engine, &session, id),
        expected,
        "stored before the seed"
    );
    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);
    assert_eq!(rows(&engine, &session, id), expected);
}

/// A restore stopped once it registered the wallet leaves it fully
/// restored: its seed and its rows are there.
#[test]
fn a_restore_stopped_after_registering_leaves_the_wallet_fully_restored() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let root = dir.path().join("data");

    stopped_at(&root, &restore_op(&backup), IMPORT_REGISTER);

    let (engine, session) = launch(&root);
    assert!(registered(&engine, &session, id));
    assert!(session.vault().has_wallet_secret(&id.0));
    assert_eq!(rows(&engine, &session, id), expected);
    assert!(matches!(
        restore(&engine, &session, &backup),
        Err(EngineError::Backup(BackupFailure::AlreadyExists(w))) if w == id
    ));
}

/// The rows a stopped restore left are its wallet's only: a retry from
/// another backup of the wallet replaces them with that backup's, and
/// another wallet restored meanwhile gets none of them.
#[test]
fn rows_a_stopped_restore_left_are_replaced_by_a_retry_and_stay_with_their_wallet() {
    let dir = dw_testutil::private_tempdir();
    let first = dir.path().join("first.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &first);
    let second = dir.path().join("second.dwbackup");
    assert_eq!(backup_of(dir.path(), PHRASE, OTHER_TXID, 2, &second), id);
    let other_backup = dir.path().join("other.dwbackup");
    let other = backup_of(dir.path(), OTHER_PHRASE, OTHER_TXID, 3, &other_backup);
    let expected_other = restored_rows(&dir.path().join("o"), &other_backup, other);
    let expected = restored_rows(dir.path(), &second, id);
    let root = dir.path().join("data");
    stopped_at(&root, &restore_op(&first), IMPORT_ROWS);
    let (engine, session) = launch(&root);
    let left = rows(&engine, &session, id);
    assert_eq!(left.locks, vec![(TXID.to_owned(), 3)]);

    assert_eq!(
        restore(&engine, &session, &other_backup).unwrap(),
        vec![other]
    );
    assert_eq!(rows(&engine, &session, other), expected_other);
    assert_eq!(
        rows(&engine, &session, id),
        left,
        "still the stopped wallet's"
    );
    assert!(!registered(&engine, &session, id));

    assert_eq!(restore(&engine, &session, &second).unwrap(), vec![id]);
    assert_eq!(rows(&engine, &session, id), expected);
}

/// The rows a stopped restore left stay when the wallet is restored from
/// its recovery phrase instead: they are the wallet's own (its id is the
/// seed's), from its own backup, and its coin locks keep coins it holds
/// for another use (collateral) out of a spend.
#[test]
fn rows_a_stopped_restore_left_stay_for_a_restore_from_the_phrase() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let root = dir.path().join("data");
    stopped_at(&root, &restore_op(&backup), IMPORT_ROWS);
    let (engine, session) = launch(&root);

    assert_eq!(import_phrase(&engine, &session, PHRASE), id);

    assert_eq!(rows(&engine, &session, id), expected);
    let kept = dashcore::OutPoint::new(TXID.parse().unwrap(), 3);
    assert_eq!(
        engine.block_on(session.locked_outpoints(id)).unwrap(),
        vec![kept],
        "listed as any lock"
    );
    engine
        .block_on(session.unlock_outpoints(id, vec![kept]))
        .unwrap();
    assert!(
        engine
            .block_on(session.locked_outpoints(id))
            .unwrap()
            .is_empty()
    );
}

/// A restore failing after any of its steps, in this process, stores
/// nothing of the wallet; run again it succeeds.
#[test]
fn a_restore_failing_after_a_step_stores_nothing_and_succeeds_when_run_again() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let (engine, session) = launch(&dir.path().join("data"));
    let db = session.data_dir().join(dw_appdb::APP_DB_FILE);

    for point in [IMPORT_ROWS, IMPORT_SECRET] {
        dw_appdb::fault::arm(&db, point, Stage::After);
        let failed = restore(&engine, &session, &backup);
        assert!(failed.is_err(), "{point}: {failed:?}");
        assert!(!registered(&engine, &session, id), "{point}");
        assert!(!session.vault().has_wallet_secret(&id.0), "{point}");
        assert_eq!(rows(&engine, &session, id), no_rows(), "{point}");
    }
    dw_appdb::fault::disarm(&db);
    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);
    assert_eq!(rows(&engine, &session, id), expected);
}

/// A restore failing once the wallet is registered, in this process,
/// leaves it in the manager without its seed (as any import failing there
/// does) but with its rows; run again, it attaches the seed.
#[test]
fn a_restore_failing_after_registering_keeps_its_rows_and_succeeds_when_run_again() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let (engine, session) = launch(&dir.path().join("data"));
    let db = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(&db, IMPORT_REGISTER, Stage::After);

    assert!(restore(&engine, &session, &backup).is_err());

    assert!(!session.vault().has_wallet_secret(&id.0));
    assert_eq!(rows(&engine, &session, id), expected);
    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);
    assert!(session.vault().has_wallet_secret(&id.0));
    assert_eq!(rows(&engine, &session, id), expected);
}

fn wallet_dat() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/compat/walletdat/bdb_wide_range.dat")
}

/// The Dash Core file import's order, which the restore now follows: its
/// coin locks and salt are stored before its seed, and both before the
/// wallet is registered.
#[test]
fn a_dash_core_file_import_stores_its_locks_and_salt_before_its_seed() {
    let dir = dw_testutil::private_tempdir();
    let op = format!("core:{}", wallet_dat().display());
    let (engine, session) = launch(&dir.path().join("control"));
    let id = engine
        .block_on(session.import_wallet_dat(wallet_dat(), None, ImportOptions::default()))
        .unwrap()
        .wallet_id;
    let expected = rows(&engine, &session, id);
    assert!(expected.locks.len() == 1 && expected.salt.is_some());
    let only_core = |r: Rows| (r.locks, r.salt);

    for (point, seed) in [(IMPORT_ROWS, false), (IMPORT_SECRET, true)] {
        let root = dir.path().join(point);
        stopped_at(&root, &op, point);
        let (engine, session) = launch(&root);
        assert!(!registered(&engine, &session, id), "{point}");
        assert_eq!(session.vault().has_wallet_secret(&id.0), seed, "{point}");
        assert_eq!(
            only_core(rows(&engine, &session, id)),
            only_core(expected.clone()),
            "{point}"
        );
    }
}

/// The v0.3.0 release's automatic backups restore to the same wallets and
/// rows as they did before #359: `restored-rows.txt` was written by this
/// test (`DWD_WRITE_GOLDEN=1`) at the revision #359 branched from.
#[test]
fn the_release_backups_restore_to_the_same_rows() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade/v0.3.0");
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("expected.json")).unwrap()).unwrap();
    let passphrase = expected["passphrase"].as_str().unwrap().as_bytes().to_vec();
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = launch(&dir.path().join("data"));
    let mut files: Vec<_> = std::fs::read_dir(fixture.join("data/regtest/backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let start = crate::events::unix_now() as i64;
    let mut ids = Vec::new();
    for file in files {
        ids.extend(
            engine
                .block_on(session.restore_backup(file, Some(Zeroizing::new(passphrase.clone()))))
                .unwrap(),
        );
    }
    ids.sort();
    let mut dump = String::new();
    for id in ids {
        let info = session.wallet_info(&id).unwrap();
        let scope = id.to_string();
        let tables = engine
            .block_on(session.appdb_op(move |db| db.export_wallet_rows(&scope)))
            .unwrap();
        dump += &format!(
            "{id} registered={} seed={} name={:?} birth={:?}\n",
            registered(&engine, &session, id),
            session.vault().has_wallet_secret(&id.0),
            info.name,
            info.birth_height,
        );
        for t in tables {
            dump += &format!("  {} {:?}\n", t.table, t.columns);
            for mut row in t.rows {
                // The wallet list's order: the restore's time, not the
                // backup's.
                if t.table == "wallets"
                    && let Some(i) = t.columns.iter().position(|c| c == "created_at")
                    && matches!(row[i], dw_appdb::SqlValue::Integer(at) if at >= start)
                {
                    row[i] = dw_appdb::SqlValue::Text("restored now".into());
                }
                dump += &format!("    {row:?}\n");
            }
        }
    }
    let golden = fixture.join("restored-rows.txt");
    if std::env::var_os("DWD_WRITE_GOLDEN").is_some() {
        std::fs::write(&golden, &dump).unwrap();
    }
    assert_eq!(dump, std::fs::read_to_string(golden).unwrap());
}

/// What platform-wallet's `initialize_from_persisted` failing after the
/// registration write leaves (its registration on disk, its seed and rows
/// deleted by the failed import, the wallet recorded as removed): after a
/// relaunch the backup restored again attaches the seed, so the wallet
/// signs, and brings back its rows.
#[test]
fn a_registration_left_without_its_seed_or_rows_is_recovered_by_restoring_again() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let expected = restored_rows(dir.path(), &backup, id);
    let root = dir.path().join("data");
    {
        let (engine, session) = launch(&root);
        assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);
        assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
        let scope = id.to_string();
        engine
            .block_on(session.appdb_op(move |db| db.delete_wallet(&scope)))
            .unwrap();
        close(engine, session);
    }
    let (engine, session) = launch(&root);
    assert!(registered(&engine, &session, id));
    assert!(!session.vault().has_wallet_secret(&id.0));
    assert_eq!(rows(&engine, &session, id), no_rows());

    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);

    assert!(session.vault().has_wallet_secret(&id.0));
    assert_eq!(rows(&engine, &session, id), expected);
    let address = engine
        .block_on(session.next_receive_address(id, None))
        .unwrap()
        .address;
    let grant = (session.vault())
        .authorize(
            dw_vault::GrantPurpose::SignMessage,
            Some(&id.0),
            dw_vault::Credential::Passphrase(VAULT_PASS),
        )
        .unwrap()
        .id;
    let signed = engine.block_on(session.sign_message(id, address, b"m".to_vec(), grant));
    assert!(signed.is_ok_and(|s| !s.is_empty()));
}

/// `phrase`'s wallet imported in this data folder and made watch-only (its
/// seed deleted): registered, without keys.
fn watch_only(engine: &Engine, session: &Arc<NetworkSession>, phrase: &[u8]) -> WalletId {
    let id = import_phrase(engine, session, phrase);
    assert!(session.vault().delete_wallet_secret(&id.0).unwrap());
    id
}

fn masternode_op() -> dw_appdb::MasternodeOpRow {
    dw_appdb::MasternodeOpRow {
        id: "op-1".into(),
        kind: "registration".into(),
        terms: "{}".into(),
        request: "{}".into(),
        unsigned_tx: Some(vec![1]),
        sign_address: None,
        sign_message: None,
        fee_inputs: "[]".into(),
        signed_tx: None,
        txid: None,
        pro_tx_hash: None,
        locked_collateral: false,
        fee: 7,
        created_at: 1,
        signed_at: None,
        sent_at: None,
        last_error: None,
    }
}

/// Keys restored to a registered (watch-only) wallet only add the backup's
/// rows: its own coin lock, label and masternode operation, which the
/// backup lacks, stay.
#[test]
fn a_restore_to_a_registered_wallet_keeps_the_rows_it_has() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let (engine, session) = launch(&dir.path().join("data"));
    assert_eq!(watch_only(&engine, &session, PHRASE), id);
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| {
            db.lock_manual(&scope, OTHER_TXID, 5, 7)?;
            db.set_label(&scope, LabelKind::Tx, OTHER_TXID, Some("own"), 8)?;
            db.insert_masternode_op_durable(&scope, &masternode_op(), &[], 9)
        }))
        .unwrap();

    assert_eq!(restore(&engine, &session, &backup).unwrap(), vec![id]);

    let scope = id.to_string();
    let (locks, label, ops) = engine
        .block_on(session.appdb_op(move |db| {
            Ok((
                (db.locks(&scope)?.into_iter())
                    .map(|l| (l.txid, l.vout))
                    .collect::<Vec<_>>(),
                db.label(&scope, LabelKind::Tx, OTHER_TXID)?,
                db.masternode_ops(&scope)?,
            ))
        }))
        .unwrap();
    assert!(locks.contains(&(OTHER_TXID.to_owned(), 5)), "{locks:?}");
    assert!(locks.contains(&(TXID.to_owned(), 3)), "{locks:?}");
    assert_eq!(label.as_deref(), Some("own"));
    assert_eq!(ops, vec![masternode_op()]);
}

/// Keys restored to a registered wallet whose seed then fails to store:
/// the settings the backup added go again and those it had stay as they
/// were (the salt it lacked is gone, its own value of a key the backup also
/// holds is kept), while the backup's other rows, added, stay.
#[test]
fn a_restore_to_a_registered_wallet_failing_after_its_rows_puts_its_settings_back() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = {
        let (engine, session) = launch(&dir.path().join("src"));
        let id = import_phrase(&engine, &session, PHRASE);
        store_rows(&engine, &session, id, TXID, 1);
        let scope = id.to_string();
        engine
            .block_on(session.appdb_op(move |db| db.set_setting(&scope, "own.key", Some("backup"))))
            .unwrap();
        engine
            .block_on(session.backup_wallet(id, backup.clone(), None))
            .unwrap();
        id
    };
    let (engine, session) = launch(&dir.path().join("data"));
    assert_eq!(watch_only(&engine, &session, PHRASE), id);
    let scope = id.to_string();
    engine
        .block_on(session.appdb_op(move |db| db.set_setting(&scope, "own.key", Some("mine"))))
        .unwrap();
    let db = session.data_dir().join(dw_appdb::APP_DB_FILE);
    dw_appdb::fault::arm(&db, IMPORT_ROWS, Stage::After);

    assert!(restore(&engine, &session, &backup).is_err());

    assert!(registered(&engine, &session, id));
    assert!(!session.vault().has_wallet_secret(&id.0));
    let scope = id.to_string();
    let own = engine
        .block_on(session.appdb_op(move |db| db.setting(&scope, "own.key")))
        .unwrap();
    assert_eq!(own.as_deref(), Some("mine"));
    let left = rows(&engine, &session, id);
    assert_eq!(left.salt, None);
    assert_eq!(left.locks, vec![(TXID.to_owned(), 3)]);
    assert_eq!(label_rows(&left), 3);
}

/// A bundle carrying no app rows still replaces the rows a stopped restore
/// left for its wallet.
#[test]
fn a_backup_without_rows_replaces_the_rows_a_stopped_restore_left() {
    let dir = dw_testutil::private_tempdir();
    let backup = dir.path().join("w.dwbackup");
    let id = backup_of(dir.path(), PHRASE, TXID, 1, &backup);
    let empty = dir.path().join("empty.dwbackup");
    {
        let (_engine, session) = launch(&dir.path().join("src-1"));
        let header = serde_json::json!({
            "format": "dwbackup", "format_version": 1, "network": "regtest", "created_at": 1,
            "wallet_ids": [id.to_string()], "automatic": false, "app_version": "test",
        });
        let header_line = serde_json::to_vec(&header).unwrap();
        let payload = serde_json::json!({
            "name": "A", "birth_height": 0, "created_at": null, "app_rows": [],
        });
        let bundle = (session.vault())
            .backup_bundle(
                &id.0,
                None,
                &serde_json::to_vec(&payload).unwrap(),
                &header_line,
            )
            .unwrap();
        let mut out = b"DWBACKUP 1\n".to_vec();
        out.extend_from_slice(&header_line);
        out.push(b'\n');
        out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": [bundle] })).unwrap());
        out.push(b'\n');
        std::fs::write(&empty, out).unwrap();
    }
    let root = dir.path().join("data");
    stopped_at(&root, &restore_op(&backup), IMPORT_ROWS);
    let (engine, session) = launch(&root);
    assert_eq!(rows(&engine, &session, id).locks.len(), 1);

    assert_eq!(restore(&engine, &session, &empty).unwrap(), vec![id]);

    assert_eq!(rows(&engine, &session, id), no_rows());
}
