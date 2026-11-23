//! Issue 340: a backup of a wallet holding a TEXT value that is not UTF-8
//! succeeds and restores the value byte for byte; a failed automatic backup
//! is recorded until one succeeds; the first unlock backs every wallet up.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dw_vault::{MemoryOsStore, UnlockScope};

use super::*;
use crate::coins::TestPause;
use crate::{Engine, EngineConfig, EventSink, SessionOptions};

const PHRASE: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const OTHER_PHRASE: &[u8] = b"zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
const KEY: &[u8] = b"abandoned:\xff";
const VALUE: &[u8] = b"v\xfe1";
const VAULT_PASSPHRASE: &[u8] = b"vault pw";

#[derive(Default)]
struct Notices {
    notices: Mutex<Vec<(NoticeCode, String)>>,
    backups: AtomicUsize,
}

impl EventSink for Notices {
    fn emit(&self, event: EngineEvent) {
        match event {
            EngineEvent::Notice { code, detail, .. } => {
                self.notices.lock().unwrap().push((code, detail));
            }
            EngineEvent::Backups { .. } => {
                self.backups.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < Duration::from_secs(60), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

impl Notices {
    fn backup_failed(&self) -> usize {
        let seen = self.notices.lock().unwrap();
        seen.iter()
            .filter(|(c, _)| *c == NoticeCode::BackupFailed)
            .count()
    }

    fn wait_for_backup_failed(&self, n: usize) {
        wait_until(&format!("{n} BackupFailed notices"), || {
            self.backup_failed() >= n
        });
    }

    /// `Backups` events: automatic backups written.
    fn written(&self) -> usize {
        self.backups.load(Ordering::SeqCst)
    }

    fn wait_for_written(&self, n: usize) {
        wait_until(&format!("{n} Backups events"), || self.written() >= n);
    }
}

fn open(data_root: PathBuf, sink: Arc<dyn EventSink>) -> (Engine, Arc<NetworkSession>) {
    let (engine, session) = launch(data_root, sink, &Arc::new(MemoryOsStore::new()));
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    (engine, session)
}

/// An engine on `data_root` with its regtest session open.
fn launch(
    data_root: PathBuf,
    sink: Arc<dyn EventSink>,
    os_store: &Arc<MemoryOsStore>,
) -> (Engine, Arc<NetworkSession>) {
    use dw_vault::{KdfParams, KdfPolicy, VaultConfig};
    let engine = Engine::new(
        EngineConfig {
            data_root,
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: os_store.clone(),
                ..Default::default()
            },
        },
        sink,
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    (engine, session)
}

/// An encrypted vault holding the wallets of `phrases`, keeping `keep`
/// automatic backups, launched again: locked, its imports' automatic
/// backups renamed to 2020 (older than any backup made now).
fn relaunched_locked(
    root: &Path,
    phrases: &[&[u8]],
    keep: u32,
    sink: Arc<Notices>,
) -> (Engine, Arc<NetworkSession>, Vec<WalletId>) {
    let os_store = Arc::new(MemoryOsStore::new());
    let first = Arc::new(Notices::default());
    let (engine, session) = launch(root.to_path_buf(), first.clone(), &os_store);
    engine
        .block_on(session.vault_op(|v| v.create(Some(VAULT_PASSPHRASE))))
        .unwrap();
    let ids: Vec<_> = phrases
        .iter()
        .map(|p| import(&engine, &session, p))
        .collect();
    wait_for_listed(&engine, &session, ids.len());
    engine.block_on(session.set_backup_policy(keep)).unwrap();
    let dir = session.backup_directory();
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        let old = dir.join(format!("{}.2020-01-01-00-00.{EXTENSION}", b.wallet_id));
        std::fs::rename(&b.path, old).unwrap();
    }
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    let (engine, session) = launch(root.to_path_buf(), sink, &os_store);
    assert_eq!(session.vault().lock_state(), LockState::Locked);
    (engine, session, ids)
}

/// Waits until `n` automatic backups are listed.
fn wait_for_listed(engine: &Engine, session: &Arc<NetworkSession>, n: usize) {
    wait_until(&format!("{n} automatic backups"), || {
        engine
            .block_on(session.automatic_backups(None))
            .unwrap()
            .len()
            >= n
    });
}

fn unlock(engine: &Engine, session: &Arc<NetworkSession>) {
    engine
        .block_on(session.vault_op(|v| v.unlock(VAULT_PASSPHRASE, UnlockScope::Full)))
        .unwrap();
}

/// Puts a file where the automatic backups folder is.
fn block_backups(session: &NetworkSession) {
    let backups = session.backup_directory();
    std::fs::remove_dir_all(&backups).ok();
    std::fs::write(&backups, b"not a folder").unwrap();
}

fn import(engine: &Engine, session: &Arc<NetworkSession>, phrase: &[u8]) -> WalletId {
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

fn app_db(session: &NetworkSession) -> rusqlite::Connection {
    rusqlite::Connection::open(session.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap()
}

/// Stores the settings row `KEY` = `VALUE` of `id`, both TEXT that is not
/// UTF-8.
fn store_damaged_row(session: &NetworkSession, id: WalletId) {
    app_db(session)
        .execute(
            "INSERT INTO settings_kv (scope, key, value) \
             VALUES (?1, CAST(?2 AS TEXT), CAST(?3 AS TEXT))",
            rusqlite::params![id.to_string(), KEY, VALUE],
        )
        .unwrap();
}

/// The type and bytes of each key and value of `id`'s settings rows whose
/// key is not UTF-8.
fn damaged_rows(session: &NetworkSession, id: WalletId) -> Vec<(String, Vec<u8>, String, Vec<u8>)> {
    let db = app_db(session);
    let mut stmt = db
        .prepare(
            "SELECT typeof(key), CAST(key AS BLOB), typeof(value), CAST(value AS BLOB) \
             FROM settings_kv WHERE scope = ?1 ORDER BY key",
        )
        .unwrap();
    stmt.query_map([id.to_string()], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<(String, Vec<u8>, String, Vec<u8>)>>>()
    .unwrap()
    .into_iter()
    .filter(|(_, key, _, _)| std::str::from_utf8(key).is_err())
    .collect()
}

fn backup(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId, dest: &Path) {
    let info = engine
        .block_on(session.backup_wallet(
            id,
            dest.to_path_buf(),
            Some(Zeroizing::new(b"bk".to_vec())),
        ))
        .unwrap();
    assert_eq!(info.path, dest);
}

/// The file's header and its one bundle's payload, opened with `bk`.
fn open_backup(session: &NetworkSession, path: &Path) -> (BackupHeader, serde_json::Value) {
    let bytes = std::fs::read(path).unwrap();
    let (header, header_line, body) = split_header(&bytes).unwrap();
    let body = parse_body(body).unwrap();
    let (_, plain) =
        open_bundle(session.vault(), &body.bundles[0], Some(b"bk"), header_line).unwrap();
    (header, serde_json::from_slice(&plain).unwrap())
}

fn expected_damaged_row() -> Vec<(String, Vec<u8>, String, Vec<u8>)> {
    vec![("text".into(), KEY.to_vec(), "text".into(), VALUE.to_vec())]
}

/// The backup succeeds as version 2, carrying the key and value as `tb`,
/// and a restore into another data folder stores the same TEXT bytes.
#[test]
fn a_backup_of_a_non_utf8_key_restores_byte_identical() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, PHRASE);
    store_damaged_row(&session, id);
    let file = dir.path().join("w.dwbackup");

    backup(&engine, &session, id, &file);

    assert!(std::fs::read(&file).unwrap().starts_with(b"DWBACKUP 2\n"));
    let (header, payload) = open_backup(&session, &file);
    assert_eq!(header.format_version, 2);
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let settings = payload["app_rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["table"] == "settings_kv")
        .unwrap();
    assert!(
        settings["rows"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!([
                { "t": id.to_string() }, { "tb": b64(KEY) }, { "tb": b64(VALUE) }
            ])),
        "{settings}"
    );

    let (engine_b, session_b) = open(dir.path().join("b"), Arc::new(Notices::default()));
    let restored = engine_b
        .block_on(session_b.restore_backup(file, Some(Zeroizing::new(b"bk".to_vec()))))
        .unwrap();

    assert_eq!(restored, [id]);
    assert_eq!(damaged_rows(&session_b, id), expected_damaged_row());
}

/// A backup whose TEXT values are all UTF-8 is still version 1, which
/// older apps read, and carries no `tb` value.
#[test]
fn a_backup_without_non_utf8_text_stays_format_1() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, PHRASE);
    let file = dir.path().join("w.dwbackup");

    backup(&engine, &session, id, &file);

    assert!(std::fs::read(&file).unwrap().starts_with(b"DWBACKUP 1\n"));
    let (header, payload) = open_backup(&session, &file);
    assert_eq!(header.format_version, 1);
    assert!(!payload.to_string().contains("\"tb\""), "{payload}");
}

/// The backup of a damaged row panics nowhere, so the engine is not marked
/// poisoned (DEC-110), and the next backup still succeeds.
#[test]
fn a_backup_of_a_non_utf8_row_leaves_the_engine_unpoisoned() {
    let name = "backup::backup_tests::a_backup_of_a_non_utf8_row_leaves_the_engine_unpoisoned";
    if !crate::platform::avatar_tests::in_child(name, "DW_BACKUP_NON_UTF8_CHILD") {
        return;
    }
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, PHRASE);
    store_damaged_row(&session, id);

    backup(&engine, &session, id, &dir.path().join("1.dwbackup"));

    assert!(!crate::engine_poisoned());
    backup(&engine, &session, id, &dir.path().join("2.dwbackup"));
    assert!(!crate::engine_poisoned());
}

/// A restore that attaches keys to a watch-only wallet and then fails
/// leaves the bundle's non-UTF-8 key row behind (rows of an existing wallet
/// are put back by key, and the key is not text). A backup of the wallet
/// still succeeds once its keys are back.
#[test]
fn a_damaged_row_a_rolled_back_restore_leaves_does_not_break_a_backup() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let a = import(&engine, &session, PHRASE);
    let b = import(&engine, &session, OTHER_PHRASE);
    let file = dir.path().join("two.dwbackup");
    let b64 = |v: &[u8]| base64::engine::general_purpose::STANDARD.encode(v);
    let header = serde_json::json!({
        "format": "dwbackup", "format_version": 2, "network": "regtest", "created_at": 1,
        "wallet_ids": [a.to_string(), b.to_string()], "automatic": false, "app_version": "test",
    });
    let header_line = serde_json::to_vec(&header).unwrap();
    let bundles: Vec<_> = [(a, true), (b, false)]
        .into_iter()
        .map(|(id, damaged)| {
            let rows: Vec<_> = damaged
                .then(|| serde_json::json!([{ "t": id.to_string() }, { "tb": b64(KEY) }, { "tb": b64(VALUE) }]))
                .into_iter()
                .collect();
            let payload = serde_json::json!({
                "name": "A", "birth_height": 0, "created_at": null,
                "app_rows": [{ "table": "settings_kv", "columns": ["scope", "key", "value"], "rows": rows }],
            });
            session
                .vault()
                .backup_bundle(&id.0, Some(b"bk"), &serde_json::to_vec(&payload).unwrap(), &header_line)
                .unwrap()
        })
        .collect();
    let mut out = b"DWBACKUP 2\n".to_vec();
    out.extend_from_slice(&header_line);
    out.push(b'\n');
    out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": bundles })).unwrap());
    out.push(b'\n');
    std::fs::write(&file, out).unwrap();
    assert!(session.vault().delete_wallet_secret(&a.0).unwrap());

    let restored =
        engine.block_on(session.restore_backup(file, Some(Zeroizing::new(b"bk".to_vec()))));

    assert!(
        matches!(restored, Err(EngineError::Backup(BackupFailure::AlreadyExists(id))) if id == b),
        "{restored:?}"
    );
    assert!(!session.vault().has_wallet_secret(&a.0), "rolled back");
    assert_eq!(
        damaged_rows(&session, a),
        expected_damaged_row(),
        "left behind"
    );
    assert_eq!(import(&engine, &session, PHRASE), a);
    let again = dir.path().join("again.dwbackup");
    backup(&engine, &session, a, &again);
    assert!(std::fs::read(&again).unwrap().starts_with(b"DWBACKUP 2\n"));
}

/// The automatic backups of the v0.3.0 release (encrypted vault, `DWBACKUP
/// 1`, written at import: the wallet's name and fingerprint) restore with
/// its vault passphrase into a new data folder.
#[test]
fn a_release_backup_still_restores() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade/v0.3.0");
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("expected.json")).unwrap()).unwrap();
    let passphrase = expected["passphrase"].as_str().unwrap().as_bytes().to_vec();
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("data"), Arc::new(Notices::default()));
    let mut files: Vec<_> = std::fs::read_dir(root.join("data/regtest/backups"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 2);

    for file in files {
        assert!(std::fs::read(&file).unwrap().starts_with(b"DWBACKUP 1\n"));
        let restored = engine
            .block_on(
                session.restore_backup(file.clone(), Some(Zeroizing::new(passphrase.clone()))),
            )
            .unwrap();
        let id = restored[0];
        assert_eq!(
            restored,
            [automatic_wallet(&file.file_name().unwrap().to_string_lossy()).unwrap()]
        );
        let wallet = expected["wallets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["walletId"] == id.to_string().as_str())
            .unwrap();
        assert_eq!(
            session.hub.name_of(&id).map(|n| n.name).as_deref(),
            wallet["name"].as_str()
        );
    }
    for wallet in expected["wallets"].as_array().unwrap() {
        let id = wallet["walletId"].as_str().unwrap().to_owned();
        let fingerprint = engine
            .block_on(session.appdb_op(move |db| db.setting(&id, "bip32.master_fingerprint")))
            .unwrap();
        assert!(fingerprint.is_some_and(|f| f.len() == 8), "{wallet}");
    }
}

/// A failed automatic backup is recorded and sends `BackupFailed`; the
/// record goes when one succeeds, when automatic backups are turned off,
/// and with the wallet.
#[test]
fn a_failed_automatic_backup_is_recorded_and_announced_until_one_succeeds() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let backups = session.backup_directory();
    let block = || {
        std::fs::remove_dir_all(&backups).ok();
        std::fs::write(&backups, b"not a folder").unwrap();
    };
    let failures = || {
        engine
            .block_on(session.automatic_backup_failures())
            .unwrap()
    };
    block();
    let before = unix_now();

    // The import's own automatic backup fails.
    let id = import(&engine, &session, PHRASE);
    notices.wait_for_backup_failed(1);

    let recorded = failures();
    assert_eq!(recorded.len(), 1, "{recorded:?}");
    assert_eq!(recorded[0].0, id);
    assert!(
        (before..=unix_now()).contains(&recorded[0].1),
        "{recorded:?}"
    );

    std::fs::remove_file(&backups).unwrap();
    engine.block_on(session.automatic_backup(id));
    assert_eq!(failures(), []);
    assert_eq!(notices.backup_failed(), 1);
    assert_eq!(
        engine
            .block_on(session.automatic_backups(Some(id)))
            .unwrap()
            .len(),
        1
    );

    let fail_again = || {
        block();
        let n = notices.backup_failed();
        engine.block_on(session.automatic_backup(id));
        assert_eq!(notices.backup_failed(), n + 1);
        assert_eq!(failures().len(), 1);
        std::fs::remove_file(&backups).unwrap();
    };
    fail_again();
    engine.block_on(session.set_backup_policy(0)).unwrap();
    assert_eq!(failures(), []);

    engine.block_on(session.set_backup_policy(3)).unwrap();
    fail_again();
    let wipe = session
        .vault()
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(id, wipe)).unwrap();
    assert_eq!(failures(), []);
}

fn failures(engine: &Engine, session: &Arc<NetworkSession>) -> Vec<(WalletId, u64)> {
    let mut f = engine
        .block_on(session.automatic_backup_failures())
        .unwrap();
    f.sort();
    f
}

fn record_failure_at(session: &NetworkSession, id: WalletId, at: &str) {
    let scope = dw_appdb::local_scope(&id.to_string());
    session
        .live()
        .unwrap()
        .appdb
        .set_setting(&scope, AUTO_FAILED_SETTING, Some(at))
        .unwrap();
}

/// A manual backup that succeeds runs an automatic backup when the last
/// one failed. With the folder still unwritable the record stays, dated
/// now; once the folder is free the record goes, also after a restart.
#[test]
fn a_manual_backup_retries_a_failed_automatic_backup() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let os_store = Arc::new(MemoryOsStore::new());
    let root = dir.path().join("a");
    let (engine, session) = launch(root.clone(), notices.clone(), &os_store);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    block_backups(&session);
    let id = import(&engine, &session, PHRASE);
    notices.wait_for_backup_failed(1);
    record_failure_at(&session, id, "1000");
    let before = unix_now();

    backup(&engine, &session, id, &dir.path().join("1.dwbackup"));

    let still = failures(&engine, &session);
    assert_eq!(still.len(), 1, "{still:?}");
    assert_eq!(still[0].0, id);
    assert!((before..=unix_now()).contains(&still[0].1), "{still:?}");
    assert_eq!(notices.backup_failed(), 2);

    std::fs::remove_file(session.backup_directory()).unwrap();
    backup(&engine, &session, id, &dir.path().join("2.dwbackup"));

    assert_eq!(failures(&engine, &session), []);
    assert_eq!(
        engine
            .block_on(session.automatic_backups(Some(id)))
            .unwrap()
            .len(),
        1
    );
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
    let (engine, session) = launch(root, Arc::new(Notices::default()), &os_store);
    assert_eq!(failures(&engine, &session), []);
}

/// A manual backup with no failed automatic one before it writes no
/// automatic backup.
#[test]
fn a_manual_backup_without_a_failure_writes_no_automatic_backup() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(b.path).unwrap();
    }

    backup(&engine, &session, id, &dir.path().join("1.dwbackup"));

    assert_eq!(
        engine.block_on(session.automatic_backups(None)).unwrap(),
        []
    );
}

/// An automatic backup whose file of the same minute appears while it is
/// built (another automatic backup of that minute) is skipped: nothing is
/// recorded or announced, and the file is left as it is.
#[test]
fn an_automatic_backup_of_a_minute_already_written_is_skipped() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    let backups = session.backup_directory();
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(b.path).unwrap();
    }
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    let s = Arc::clone(&session);
    let task = session
        .rt
        .spawn(async move { s.automatic_backup(id).await });
    assert!(pause.wait_reached(Duration::from_secs(60)));
    let now = unix_now();
    let taken: Vec<_> = [now - 60, now, now + 60]
        .into_iter()
        .map(|t| backups.join(automatic_name(&id, t)))
        .collect();
    for path in &taken {
        std::fs::write(path, b"taken").unwrap();
    }

    pause.release.notify_one();
    engine.block_on(task).unwrap();

    assert_eq!(failures(&engine, &session), []);
    assert_eq!(notices.backup_failed(), 0);
    notices.wait_for_written(1);
    assert_eq!(notices.written(), 1);
    for path in &taken {
        assert_eq!(std::fs::read(path).unwrap(), b"taken");
    }
}

/// A failure record that is not UTF-8 text (a crafted row) is skipped when
/// the failures are listed, and turning automatic backups off still
/// deletes it with the others, in the write of the policy.
#[test]
fn an_unreadable_failure_record_neither_hides_others_nor_stops_turning_backups_off() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let a = import(&engine, &session, PHRASE);
    let b = import(&engine, &session, OTHER_PHRASE);
    wait_for_listed(&engine, &session, 2);
    app_db(&session)
        .execute(
            "INSERT INTO settings_kv (scope, key, value) VALUES (?1, ?2, CAST(?3 AS TEXT))",
            rusqlite::params![
                dw_appdb::local_scope(&a.to_string()),
                AUTO_FAILED_SETTING,
                b"\xff".to_vec()
            ],
        )
        .unwrap();
    record_failure_at(&session, b, "5");

    assert_eq!(failures(&engine, &session), [(b, 5)]);

    engine.block_on(session.set_backup_policy(0)).unwrap();

    assert_eq!(session.backup_policy().unwrap().keep, 0);
    let left: i64 = app_db(&session)
        .query_row(
            "SELECT count(*) FROM settings_kv WHERE key = ?1",
            [AUTO_FAILED_SETTING],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0);
}

/// An automatic backup of a wallet with a damaged row is version 2, and a
/// restore of it stores the row's TEXT bytes as they were.
#[test]
fn an_automatic_backup_of_a_damaged_row_restores_byte_identical() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(b.path).unwrap();
    }
    store_damaged_row(&session, id);

    engine.block_on(session.automatic_backup(id));

    let written = engine
        .block_on(session.automatic_backups(Some(id)))
        .unwrap();
    assert_eq!(written.len(), 1);
    assert!(
        std::fs::read(&written[0].path)
            .unwrap()
            .starts_with(b"DWBACKUP 2\n")
    );
    assert!(read_header(&written[0].path).unwrap().automatic);
    assert_eq!(notices.backup_failed(), 0);
    let file = dir.path().join("auto.dwbackup");
    std::fs::copy(&written[0].path, &file).unwrap();
    let wipe = session
        .vault()
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id;
    engine.block_on(session.remove_wallet(id, wipe)).unwrap();
    assert_eq!(damaged_rows(&session, id), []);

    let restored = engine.block_on(session.restore_backup(file, None)).unwrap();

    assert_eq!(restored, [id]);
    assert_eq!(damaged_rows(&session, id), expected_damaged_row());
}

/// An encrypted vault is locked at open, where nothing is backed up; its
/// first unlock backs every wallet up once, in the background, and the
/// policy's rotation still holds. A second unlock backs nothing up.
#[test]
fn the_first_unlock_backs_up_every_wallet_once() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, ids) =
        relaunched_locked(dir.path(), &[PHRASE, OTHER_PHRASE], 1, notices.clone());
    let listed = || engine.block_on(session.automatic_backups(None)).unwrap();
    assert_eq!(session.backups.runs.load(Ordering::SeqCst), 0);
    assert_eq!(listed().len(), 2);

    unlock(&engine, &session);
    notices.wait_for_written(2);

    let after = listed();
    assert_eq!(after.len(), 2, "{after:?}");
    for id in &ids {
        let mine: Vec<_> = after.iter().filter(|b| b.wallet_id == *id).collect();
        assert_eq!(mine.len(), 1, "{after:?}");
        assert!(
            !mine[0].path.to_string_lossy().contains(".2020-"),
            "{after:?}"
        );
    }
    assert_eq!(notices.backup_failed(), 0);
    assert_eq!(failures(&engine, &session), []);

    engine.block_on(session.lock_vault()).unwrap();
    for b in after {
        std::fs::remove_file(b.path).unwrap();
    }
    unlock(&engine, &session);

    assert_eq!(session.backups.runs.load(Ordering::SeqCst), 1);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(listed(), []);
    assert_eq!(notices.written(), 2);
}

/// The first unlock's automatic backups that fail are recorded and
/// announced (`BackupFailed`, the app's toast).
#[test]
fn a_failed_backup_after_the_first_unlock_is_recorded_and_announced() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, mut ids) =
        relaunched_locked(dir.path(), &[PHRASE, OTHER_PHRASE], 10, notices.clone());
    block_backups(&session);

    unlock(&engine, &session);
    notices.wait_for_backup_failed(2);

    ids.sort();
    let recorded: Vec<_> = failures(&engine, &session)
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(recorded, ids);
    assert_eq!(notices.written(), 0);
}

/// The unlock returns while its automatic backup is still held.
#[test]
fn unlock_returns_before_its_backups_finish() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, ids) = relaunched_locked(dir.path(), &[PHRASE], 10, notices.clone());
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());

    unlock(&engine, &session);

    assert!(pause.wait_reached(Duration::from_secs(60)));
    assert_eq!(notices.written(), 0);
    pause.release.notify_one();
    notices.wait_for_written(1);
    let listed = engine
        .block_on(session.automatic_backups(Some(ids[0])))
        .unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
}
