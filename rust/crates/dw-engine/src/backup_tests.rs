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

/// Waits until `n` automatic backups are listed and none is being written.
fn wait_for_listed(engine: &Engine, session: &Arc<NetworkSession>, n: usize) {
    wait_until(&format!("{n} automatic backups"), || {
        engine
            .block_on(session.automatic_backups(None))
            .unwrap()
            .len()
            >= n
            && session.backups.in_flight().is_empty()
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

/// A version 2 backup of `a` (with the damaged row) and `b`, both with
/// passphrase `bk`: restored into a vault that has `b`'s keys, it attaches
/// `a`'s and then fails `AlreadyExists(b)`, rolling `a` back.
fn write_two_wallet_backup(session: &NetworkSession, a: WalletId, b: WalletId, file: &Path) {
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
    std::fs::write(file, out).unwrap();
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
    write_two_wallet_backup(&session, a, b, &file);
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
    let mut complete = Vec::new();
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        complete = std::fs::read(&b.path).unwrap();
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
        std::fs::write(path, &complete).unwrap();
    }

    pause.release.notify_one();
    engine.block_on(task).unwrap();

    assert_eq!(failures(&engine, &session), []);
    assert_eq!(notices.backup_failed(), 0);
    notices.wait_for_written(1);
    assert_eq!(notices.written(), 1);
    for path in &taken {
        assert_eq!(std::fs::read(path).unwrap(), complete);
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

/// The unlock returns while its automatic backup is still held: run on the
/// runtime with a bounded wait, so an unlock that waited for its backups
/// fails here instead of hanging.
#[test]
fn unlock_returns_before_its_backups_finish() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, ids) = relaunched_locked(dir.path(), &[PHRASE], 10, notices.clone());
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());

    let (sent, unlocked) = std::sync::mpsc::channel();
    let s = Arc::clone(&session);
    session.rt.spawn(async move {
        let out = s
            .vault_op(|v| v.unlock(VAULT_PASSPHRASE, UnlockScope::Full))
            .await;
        sent.send(out.is_ok()).unwrap();
    });

    assert_eq!(unlocked.recv_timeout(Duration::from_secs(60)), Ok(true));
    assert!(pause.wait_reached(Duration::from_secs(60)));
    assert_eq!(notices.written(), 0);
    pause.release.notify_one();
    notices.wait_for_written(1);
    let listed = engine
        .block_on(session.automatic_backups(Some(ids[0])))
        .unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
}

fn wipe_grant(session: &NetworkSession, id: WalletId) -> String {
    session
        .vault()
        .authorize(
            dw_vault::GrantPurpose::Wipe,
            Some(&id.0),
            dw_vault::Credential::None,
        )
        .unwrap()
        .id
}

/// Waits until `task` ended or someone waits for the wallet lifecycle lock.
fn wait_finished_or_held<T>(session: &NetworkSession, task: &tokio::task::JoinHandle<T>) {
    wait_until("the call ends or waits for the lifecycle lock", || {
        task.is_finished() || session.wallet_lifecycle.waiting() > 0
    });
}

/// "Backups to keep" None set while an automatic backup is being written
/// waits for it, then deletes what it wrote, or the failure it recorded.
/// Unix only: it fails the backup by taking write permission away.
#[cfg(unix)]
#[test]
fn turning_backups_off_during_an_automatic_backup_leaves_no_backup_and_no_record() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(b.path).unwrap();
    }
    let during_a_backup = |block: bool| {
        let pause = Arc::new(TestPause::default());
        *session.backups.pause.lock().unwrap() = Some(pause.clone());
        let s = Arc::clone(&session);
        let backup = session.rt.spawn(async move {
            s.automatic_backup(id).await;
        });
        assert!(pause.wait_reached(Duration::from_secs(60)));
        if block {
            set_mode(&session.backup_directory(), 0o500);
        }
        let s = Arc::clone(&session);
        let off = session
            .rt
            .spawn(async move { s.set_backup_policy(0).await });
        wait_until("turning off ends or waits for the policy lock", || {
            off.is_finished() || session.backups.policy_waiting() > 0
        });
        pause.release.notify_one();
        engine.block_on(backup).unwrap();
        engine.block_on(off).unwrap().unwrap();
    };

    during_a_backup(false);

    assert_eq!(session.backup_policy().unwrap().keep, 0);
    assert_eq!(
        engine.block_on(session.automatic_backups(None)).unwrap(),
        []
    );
    assert_eq!(failures(&engine, &session), []);

    engine.block_on(session.set_backup_policy(3)).unwrap();
    during_a_backup(true);

    set_mode(&session.backup_directory(), 0o700);
    assert_eq!(notices.backup_failed(), 1);
    assert_eq!(failures(&engine, &session), []);
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// With "Backups to keep" None, a user backup deletes a failure record that
/// is still there: the Settings row it shows says to make one.
#[test]
fn a_backup_with_automatic_backups_off_clears_a_stale_failure_record() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    engine.block_on(session.set_backup_policy(0)).unwrap();
    record_failure_at(&session, id, "1000");

    backup(&engine, &session, id, &dir.path().join("1.dwbackup"));

    assert_eq!(failures(&engine, &session), []);
}

/// A recovery ("Forgot passphrase") of a vault locked at launch backs every
/// wallet up under the new passphrase, and so does a passphrase change
/// later in the same session.
#[test]
fn a_credential_change_backs_every_wallet_up_again() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, ids) = relaunched_locked(dir.path(), &[PHRASE], 10, notices.clone());

    engine
        .block_on(session.recover_vault(
            ids[0].0,
            Zeroizing::new(PHRASE.to_vec()),
            Zeroizing::new(Vec::new()),
            Zeroizing::new(b"new pw".to_vec()),
        ))
        .unwrap();
    notices.wait_for_written(1);

    let listed = || {
        engine
            .block_on(session.automatic_backups(Some(ids[0])))
            .unwrap()
    };
    let newest = || {
        listed()
            .into_iter()
            .find(|b| {
                let name = b.path.to_string_lossy();
                !name.contains(".2020-") && !name.contains(".2021-")
            })
            .unwrap()
            .path
    };
    let sealed_with = |file: PathBuf, folder: &str, old: &[u8], new: &[u8]| {
        let (engine_b, session_b) = open(dir.path().join(folder), Arc::new(Notices::default()));
        let restore = |pw: &[u8]| {
            engine_b
                .block_on(session_b.restore_backup(file.clone(), Some(Zeroizing::new(pw.to_vec()))))
        };
        assert!(
            matches!(
                restore(old),
                Err(EngineError::Backup(BackupFailure::WrongPassphrase))
            ),
            "{folder}: the old passphrase"
        );
        assert_eq!(
            restore(new).unwrap(),
            [ids[0]],
            "{folder}: the new passphrase"
        );
    };
    assert_eq!(listed().len(), 2);
    sealed_with(newest(), "b", VAULT_PASSPHRASE, b"new pw");
    let backups = session.backup_directory();
    std::fs::rename(
        newest(),
        backups.join(format!("{}.2021-01-01-00-00.{EXTENSION}", ids[0])),
    )
    .unwrap();

    engine
        .block_on(session.change_passphrase(
            Zeroizing::new(b"new pw".to_vec()),
            Zeroizing::new(b"newer pw".to_vec()),
        ))
        .unwrap();
    notices.wait_for_written(2);

    assert_eq!(listed().len(), 3);
    sealed_with(newest(), "c", b"new pw", b"newer pw");
    assert_eq!(notices.backup_failed(), 0);
}

/// A wallet removed while its automatic backup waits gets nothing written,
/// recorded or announced after the removal returned.
#[test]
fn a_wallet_removed_during_its_automatic_backup_gets_nothing_after_the_removal() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    for b in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(b.path).unwrap();
    }
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    let s = Arc::clone(&session);
    let backup = session.rt.spawn(async move {
        s.automatic_backup(id).await;
    });
    assert!(pause.wait_reached(Duration::from_secs(60)));
    let wipe = wipe_grant(&session, id);
    let (s, n, backups) = (
        Arc::clone(&session),
        notices.clone(),
        session.backup_directory(),
    );
    let removal = session.rt.spawn(async move {
        s.remove_wallet(id, wipe).await.unwrap();
        let files = list_automatic(&backups, "regtest", None).unwrap().len();
        (files, n.written(), n.backup_failed())
    });
    wait_finished_or_held(&session, &removal);

    pause.release.notify_one();
    engine.block_on(backup).unwrap();
    let (files, written, failed) = engine.block_on(removal).unwrap();

    assert_eq!(
        list_automatic(&session.backup_directory(), "regtest", None)
            .unwrap()
            .len(),
        files
    );
    assert_eq!(notices.written(), written);
    assert_eq!((failed, notices.backup_failed()), (0, 0));
    assert_eq!(failures(&engine, &session), []);
}

/// An automatic backup of a wallet whose keys a restore attached and then
/// rolled back writes nothing: it waits for the restore to end.
#[test]
fn a_restore_rolled_back_during_an_automatic_backup_leaves_none() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let a = import(&engine, &session, PHRASE);
    let b = import(&engine, &session, OTHER_PHRASE);
    wait_for_listed(&engine, &session, 2);
    for f in engine.block_on(session.automatic_backups(None)).unwrap() {
        std::fs::remove_file(f.path).unwrap();
    }
    let file = dir.path().join("two.dwbackup");
    write_two_wallet_backup(&session, a, b, &file);
    assert!(session.vault().delete_wallet_secret(&a.0).unwrap());
    let pause = Arc::new(TestPause::default());
    *session.backups.pause_restore.lock().unwrap() = Some(pause.clone());
    let s = Arc::clone(&session);
    let restore = session.rt.spawn(async move {
        s.restore_backup(file, Some(Zeroizing::new(b"bk".to_vec())))
            .await
    });
    assert!(pause.wait_reached(Duration::from_secs(60)));
    assert!(session.vault().has_wallet_secret(&a.0), "attached");
    let s = Arc::clone(&session);
    let backup = session.rt.spawn(async move {
        s.automatic_backup(a).await;
    });
    wait_finished_or_held(&session, &backup);

    pause.release.notify_one();
    let restored = engine.block_on(restore).unwrap();
    engine.block_on(backup).unwrap();

    assert!(
        matches!(restored, Err(EngineError::Backup(BackupFailure::AlreadyExists(id))) if id == b),
        "{restored:?}"
    );
    assert!(!session.vault().has_wallet_secret(&a.0), "rolled back");
    assert_eq!(
        engine.block_on(session.automatic_backups(None)).unwrap(),
        []
    );
    assert_eq!(notices.backup_failed(), 0);
    assert_eq!(failures(&engine, &session), []);
}

/// A lock while the first unlock's backups run ends them; the next unlock
/// starts them over and backs every wallet up.
#[test]
fn a_lock_during_the_first_unlocks_backups_lets_the_next_unlock_finish_them() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, _) =
        relaunched_locked(dir.path(), &[PHRASE, OTHER_PHRASE], 10, notices.clone());
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    unlock(&engine, &session);
    assert!(pause.wait_reached(Duration::from_secs(60)));

    session.lock_vault_sync().unwrap();
    pause.release.notify_one();
    wait_until("the first run ends", || {
        session.backups.runs_done.load(Ordering::SeqCst) == 1
    });
    assert_eq!(notices.written(), 0);
    unlock(&engine, &session);

    notices.wait_for_written(2);
    assert_eq!(session.backups.runs.load(Ordering::SeqCst), 2);
    assert_eq!(notices.backup_failed(), 0);
}

/// An unlock that comes while a run that found the vault locked is ending
/// still gets every wallet backed up.
#[test]
fn an_unlock_while_a_locked_run_ends_backs_every_wallet_up() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, _) =
        relaunched_locked(dir.path(), &[PHRASE, OTHER_PHRASE], 10, notices.clone());
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    let ending = Arc::new(TestPause::default());
    *session.backups.pause_locked.lock().unwrap() = Some(ending.clone());
    unlock(&engine, &session);
    assert!(pause.wait_reached(Duration::from_secs(60)));
    session.lock_vault_sync().unwrap();
    pause.release.notify_one();
    assert!(ending.wait_reached(Duration::from_secs(60)));

    unlock(&engine, &session);
    ending.release.notify_one();

    notices.wait_for_written(2);
    assert_eq!(session.backups.runs.load(Ordering::SeqCst), 2);
    assert_eq!(notices.backup_failed(), 0);
}

// Issue 357: a backup appears under its name only once complete, and a file
// that is not a complete backup is never listed, counted or taken for one.

/// The bytes of a complete automatic backup of a new wallet, its id and the
/// session, its automatic backups deleted.
fn automatic_bytes(dir: &Path, phrase: &[u8]) -> (Engine, Arc<NetworkSession>, WalletId, Vec<u8>) {
    let (engine, session) = open(dir.join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, phrase);
    wait_for_listed(&engine, &session, 1);
    let listed = list_automatic(&session.backup_directory(), "regtest", None).unwrap();
    let bytes = std::fs::read(&listed[0].path).unwrap();
    for b in listed {
        std::fs::remove_file(b.path).unwrap();
    }
    (engine, session, id, bytes)
}

/// The length of the magic and header lines of `bytes`.
fn header_end(bytes: &[u8]) -> usize {
    let first = bytes.iter().position(|b| *b == b'\n').unwrap();
    first + 1 + bytes[first + 1..].iter().position(|b| *b == b'\n').unwrap() + 1
}

fn at_minute(id: WalletId, minute: u32) -> String {
    format!("{id}.2020-01-01-00-{minute:02}.{EXTENSION}")
}

fn private_dir(path: &Path) -> PathBuf {
    std::fs::create_dir(path).unwrap();
    #[cfg(unix)]
    set_mode(path, 0o700);
    path.to_path_buf()
}

/// Cut at 0, in the magic line, in the header, just after the header line,
/// mid-body and just before the body's last `}`, a backup is not listed;
/// without just its final newline it is.
#[test]
fn only_a_complete_file_is_listed_as_an_automatic_backup() {
    let dir = dw_testutil::private_tempdir();
    let (_engine, _session, id, bytes) = automatic_bytes(dir.path(), PHRASE);
    let end = header_end(&bytes);
    let cuts = [
        ("empty", 0),
        ("in the magic line", 4),
        ("in the header", 20),
        ("after the header", end),
        ("mid-body", (end + bytes.len()) / 2),
        ("before the last brace", bytes.len() - 2),
    ];
    for (i, (what, cut)) in cuts.into_iter().enumerate() {
        let folder = private_dir(&dir.path().join(format!("cut{i}")));
        std::fs::write(folder.join(at_minute(id, 0)), &bytes[..cut]).unwrap();
        assert!(
            list_automatic(&folder, "regtest", None).unwrap().is_empty(),
            "{what}"
        );
    }
    let folder = private_dir(&dir.path().join("no-newline"));
    std::fs::write(folder.join(at_minute(id, 0)), &bytes[..bytes.len() - 1]).unwrap();
    assert_eq!(list_automatic(&folder, "regtest", None).unwrap().len(), 1);
}

/// Swaps the header line of `bytes` for `edit` of it; the body is left as
/// it is (completeness is checked without the passphrase).
fn with_header(bytes: &[u8], edit: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let first = bytes.iter().position(|b| *b == b'\n').unwrap() + 1;
    let end = header_end(bytes);
    let mut header: serde_json::Value = serde_json::from_slice(&bytes[first..end - 1]).unwrap();
    edit(&mut header);
    let mut out = bytes[..first].to_vec();
    out.extend(serde_json::to_vec(&header).unwrap());
    out.push(b'\n');
    out.extend_from_slice(&bytes[end..]);
    out
}

/// A version 2 automatic backup is listed and rotated. A file whose body
/// is whole JSON is not a backup when it holds a bundle more than its
/// header names, a bundle of another wallet or of a version this build
/// does not read, or anything after the body; nor is a complete backup
/// under another wallet's name, or of another network. Both v0.3.0 release
/// backups are.
#[test]
fn an_automatic_backup_must_be_of_the_wallet_and_network_it_is_listed_for() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let a = import(&engine, &session, PHRASE);
    let b = import(&engine, &session, OTHER_PHRASE);
    wait_for_listed(&engine, &session, 2);
    let backups = session.backup_directory();
    let mut of_b = Vec::new();
    for f in list_automatic(&backups, "regtest", None).unwrap() {
        if f.wallet_id == b {
            of_b = std::fs::read(&f.path).unwrap();
        }
        std::fs::remove_file(f.path).unwrap();
    }
    store_damaged_row(&session, a);
    let s = Arc::clone(&session);
    engine
        .block_on(session.rt.spawn(async move { s.automatic_backup(a).await }))
        .unwrap();
    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    let bytes = std::fs::read(&listed[0].path).unwrap();
    assert!(bytes.starts_with(b"DWBACKUP 2\n"));
    prune(&backups, "regtest", None, 0, None).unwrap();
    assert!(!listed[0].path.exists(), "a version 2 backup is rotated");

    let folder = private_dir(&dir.path().join("files"));
    let listed_as = |id: WalletId, file: &[u8], network: &str| {
        let path = folder.join(at_minute(id, 0));
        std::fs::write(&path, file).unwrap();
        let n = list_automatic(&folder, network, None).unwrap().len();
        std::fs::remove_file(path).unwrap();
        n
    };
    assert_eq!(listed_as(a, &bytes, "regtest"), 1);

    let end = header_end(&bytes);
    let mut body: serde_json::Value = serde_json::from_slice(&bytes[end..]).unwrap();
    let of_b_body: serde_json::Value = serde_json::from_slice(&of_b[header_end(&of_b)..]).unwrap();
    body["bundles"]
        .as_array_mut()
        .unwrap()
        .push(of_b_body["bundles"][0].clone());
    let mut one_more = bytes[..end].to_vec();
    one_more.extend(serde_json::to_vec(&body).unwrap());
    one_more.push(b'\n');
    assert_eq!(listed_as(a, &one_more, "regtest"), 0, "a bundle more");

    let mut trailing = bytes.clone();
    trailing.extend(b"{}\n");
    assert_eq!(
        listed_as(a, &trailing, "regtest"),
        0,
        "something after the body"
    );
    let mut v3: serde_json::Value = serde_json::from_slice(&bytes[end..]).unwrap();
    v3["bundles"][0]["version"] = 3.into();
    let mut version_3 = bytes[..end].to_vec();
    version_3.extend(serde_json::to_vec(&v3).unwrap());
    version_3.push(b'\n');
    assert_eq!(
        listed_as(a, &version_3, "regtest"),
        0,
        "a bundle version this build does not read"
    );

    let names_b = with_header(&bytes, |h| {
        h["wallet_ids"] = serde_json::json!([b.to_string()])
    });
    assert_eq!(
        listed_as(b, &names_b, "regtest"),
        0,
        "a bundle of another wallet"
    );
    assert_eq!(listed_as(b, &bytes, "regtest"), 0, "another wallet's name");
    let testnet = with_header(&bytes, |h| h["network"] = "testnet".into());
    assert_eq!(listed_as(a, &testnet, "regtest"), 0, "another network");
    assert_eq!(
        listed_as(a, &bytes, "testnet"),
        0,
        "listed for another network"
    );

    let release = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/upgrade/v0.3.0/data/regtest/backups");
    assert_eq!(list_automatic(&release, "regtest", None).unwrap().len(), 2);
}

/// "Backups to keep" 2 with three cut-off backups newer than the one
/// complete backup keeps the complete one, and deletes none of the others.
#[test]
fn pruning_keeps_the_newest_complete_backup_whatever_incomplete_files_there_are() {
    let dir = dw_testutil::private_tempdir();
    let (_engine, session, id, bytes) = automatic_bytes(dir.path(), PHRASE);
    let backups = session.backup_directory();
    let cut = &bytes[..(header_end(&bytes) + bytes.len()) / 2];
    std::fs::write(backups.join(at_minute(id, 0)), &bytes).unwrap();
    for minute in 1..=3 {
        std::fs::write(backups.join(at_minute(id, minute)), cut).unwrap();
    }

    prune(&backups, "regtest", None, 2, None).unwrap();

    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, backups.join(at_minute(id, 0)));
    for minute in 1..=3 {
        assert_eq!(
            std::fs::read(backups.join(at_minute(id, minute))).unwrap(),
            cut
        );
    }
}

/// Waits until the current minute has at least 15 seconds left.
fn not_at_a_minute_end() {
    wait_until("a minute with time left", || unix_now() % 60 < 45);
}

/// A cut-off file at this minute's name is set aside as `.incomplete`
/// (kept) and that minute's backup is written; where it cannot be set
/// aside, the backup's failure is recorded and announced. Unix only: it
/// stops the setting aside by taking write permission away.
#[cfg(unix)]
#[test]
fn an_incomplete_file_at_this_minutes_name_neither_stops_its_backup_nor_hides_its_failure() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session) = open(dir.path().join("a"), notices.clone());
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    let backups = session.backup_directory();
    let listed = list_automatic(&backups, "regtest", None).unwrap();
    let bytes = std::fs::read(&listed[0].path).unwrap();
    std::fs::remove_file(&listed[0].path).unwrap();
    let cut = &bytes[..(header_end(&bytes) + bytes.len()) / 2];
    let run = || {
        let s = Arc::clone(&session);
        engine
            .block_on(
                session
                    .rt
                    .spawn(async move { s.automatic_backup(id).await }),
            )
            .unwrap();
    };

    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::fs::write(&name, cut).unwrap();
    run();

    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, name);
    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    assert_eq!(std::fs::read(&aside).unwrap(), cut);
    assert!(failures(&engine, &session).is_empty());

    not_at_a_minute_end();
    std::fs::remove_file(&listed[0].path).unwrap();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::fs::write(&name, cut).unwrap();
    set_mode(&backups, 0o500);
    run();
    set_mode(&backups, 0o700);

    assert_eq!(failures(&engine, &session).len(), 1);
    notices.wait_for_backup_failed(1);
    assert_eq!(std::fs::read(&name).unwrap(), cut);
    assert_eq!(std::fs::read(&aside).unwrap(), cut);
}

/// Runs this test binary's `name` in a child with `CHILD_DIR` set to `dir`
/// and `DW_FAULT` naming the point between a backup's sync and its publish;
/// `Err` with its standard error unless it was stopped there.
fn stopped_in_child(name: &str, dir: &Path) -> Result<(), String> {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_DIR, dir)
        .env("DW_FAULT", "backup.before_publish")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    match out.status.success() {
        false if stderr.contains("DW_FAULT=backup.before_publish: aborting") => Ok(()),
        _ => Err(stderr),
    }
}

const CHILD_DIR: &str = "DW_BACKUP_CHILD_DIR";

fn file_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// A user backup stopped between its sync and its publish leaves nothing
/// at the path the user chose, only a hidden temporary file beside it.
#[test]
fn a_user_backup_stopped_before_its_publish_leaves_nothing_at_its_path() {
    if let Some(root) = std::env::var_os(CHILD_DIR) {
        let root = PathBuf::from(root);
        let (engine, session) = open(root.join("data"), Arc::new(Notices::default()));
        engine.block_on(session.set_backup_policy(0)).unwrap();
        let id = import(&engine, &session, PHRASE);
        backup(&engine, &session, id, &root.join("chosen/w.dwbackup"));
        return;
    }
    let dir = dw_testutil::private_tempdir();
    let chosen = private_dir(&dir.path().join("chosen"));

    let stopped = stopped_in_child(
        "backup::backup_tests::a_user_backup_stopped_before_its_publish_leaves_nothing_at_its_path",
        dir.path(),
    );

    let names = file_names(&chosen);
    assert!(!names.contains(&"w.dwbackup".to_owned()), "{names:?}");
    stopped.unwrap();
    assert_eq!(names.len(), 1);
    assert!(
        names[0].starts_with(".dash-wallet-backup.tmp-"),
        "{names:?}"
    );
}

/// An automatic backup stopped between its sync and its publish leaves no
/// backup and its failure recorded: the next start shows it while the
/// vault is locked, and the backup its unlock writes clears it.
#[test]
fn an_automatic_backup_stopped_before_its_publish_shows_its_failure_until_the_next_one() {
    if let Some(root) = std::env::var_os(CHILD_DIR) {
        let root = PathBuf::from(root);
        let (engine, session) = launch(
            root,
            Arc::new(Notices::default()),
            &Arc::new(MemoryOsStore::new()),
        );
        engine
            .block_on(session.vault_op(|v| v.create(Some(VAULT_PASSPHRASE))))
            .unwrap();
        import(&engine, &session, PHRASE);
        wait_for_listed(&engine, &session, 1);
        return;
    }
    let dir = dw_testutil::private_tempdir();

    let stopped = stopped_in_child(
        "backup::backup_tests::an_automatic_backup_stopped_before_its_publish_shows_its_failure_until_the_next_one",
        dir.path(),
    );

    let backups = dir.path().join("regtest").join(BACKUP_DIR);
    let names = file_names(&backups);
    assert!(
        !names.iter().any(|n| n.ends_with(&format!(".{EXTENSION}"))),
        "{names:?}"
    );
    stopped.unwrap();
    let notices = Arc::new(Notices::default());
    let (engine, session) = launch(
        dir.path().to_path_buf(),
        notices.clone(),
        &Arc::new(MemoryOsStore::new()),
    );
    assert_eq!(session.vault().lock_state(), LockState::Locked);
    let failed = failures(&engine, &session);
    assert_eq!(failed.len(), 1);

    unlock(&engine, &session);
    notices.wait_for_written(1);

    assert!(failures(&engine, &session).is_empty());
    assert_eq!(
        engine
            .block_on(session.automatic_backups(Some(failed[0].0)))
            .unwrap()
            .len(),
        1
    );
}

/// While an automatic backup is being written its failure is already
/// recorded (a stop would leave it) but not listed.
#[test]
fn a_backup_being_written_shows_no_failure() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, _) = automatic_bytes(dir.path(), PHRASE);
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    let s = Arc::clone(&session);
    let backup = session
        .rt
        .spawn(async move { s.automatic_backup(id).await });
    assert!(pause.wait_reached(Duration::from_secs(60)));
    let recorded = || {
        session
            .live()
            .unwrap()
            .appdb
            .setting(&dw_appdb::local_scope(&id.to_string()), AUTO_FAILED_SETTING)
            .unwrap()
    };

    assert!(recorded().is_some());
    assert!(failures(&engine, &session).is_empty());

    pause.release.notify_one();
    engine.block_on(backup).unwrap();
    assert_eq!(recorded(), None);
    assert!(failures(&engine, &session).is_empty());
    assert_eq!(
        list_automatic(&session.backup_directory(), "regtest", None)
            .unwrap()
            .len(),
        1
    );
}

/// Sets `path`'s modification time to `ago` before now.
fn written_ago(path: &Path, ago: Duration) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - ago)
        .unwrap();
}

/// In the automatic backups folder, a temporary file over an hour old goes
/// once a complete backup of its wallet is newer than it; every other
/// temporary file is kept.
#[test]
fn a_stale_temporary_file_goes_only_once_a_newer_backup_of_its_wallet_is_there() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, a, bytes) = automatic_bytes(dir.path(), PHRASE);
    let b = import(&engine, &session, OTHER_PHRASE);
    wait_for_listed(&engine, &session, 1);
    let backups = session.backup_directory();
    for f in list_automatic(&backups, "regtest", None).unwrap() {
        std::fs::remove_file(f.path).unwrap();
    }
    let hour = Duration::from_secs(60 * 60);
    let end = header_end(&bytes);
    let mut header: serde_json::Value = serde_json::from_slice(
        &bytes[bytes.iter().position(|b| *b == b'\n').unwrap() + 1..end - 1],
    )
    .unwrap();
    header["created_at"] = (unix_now() - 3 * hour.as_secs()).into();
    let mut three_hours_old = bytes[..bytes.iter().position(|b| *b == b'\n').unwrap() + 1].to_vec();
    three_hours_old.extend(serde_json::to_vec(&header).unwrap());
    three_hours_old.push(b'\n');
    three_hours_old.extend_from_slice(&bytes[end..]);
    std::fs::write(backups.join(at_minute(a, 0)), three_hours_old).unwrap();
    assert_eq!(list_automatic(&backups, "regtest", None).unwrap().len(), 1);
    let temp = |id: WalletId, minute: u32, ago: Duration| {
        let path = backups.join(format!(".{id}.tmp-1-{minute:016x}"));
        std::fs::write(&path, &bytes).unwrap();
        written_ago(&path, ago);
        path
    };
    let older = temp(a, 1, 4 * hour);
    let newer = temp(a, 2, 2 * hour);
    let recent = temp(a, 3, hour / 2);
    let no_backup = temp(b, 4, 4 * hour);

    prune(&backups, "regtest", None, 10, None).unwrap();

    assert!(!older.exists());
    assert!(newer.exists() && recent.exists() && no_backup.exists());
}

/// Restoring a cut-off backup fails `backup.corrupt` before anything is
/// imported.
#[test]
fn restoring_a_cut_off_backup_fails_corrupt_and_imports_nothing() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session) = open(dir.path().join("a"), Arc::new(Notices::default()));
    let id = import(&engine, &session, PHRASE);
    let file = dir.path().join("w.dwbackup");
    backup(&engine, &session, id, &file);
    let bytes = std::fs::read(&file).unwrap();
    std::fs::write(&file, &bytes[..(header_end(&bytes) + bytes.len()) / 2]).unwrap();

    let (engine_b, session_b) = open(dir.path().join("b"), Arc::new(Notices::default()));
    let restored =
        engine_b.block_on(session_b.restore_backup(file, Some(Zeroizing::new(b"bk".to_vec()))));

    assert!(
        matches!(
            restored,
            Err(EngineError::Backup(BackupFailure::Corrupt(_)))
        ),
        "{restored:?}"
    );
    assert!(
        session_b
            .manager()
            .unwrap()
            .list_wallet_ids_blocking()
            .is_empty()
    );
    assert!(
        engine_b
            .block_on(session_b.automatic_backups(None))
            .unwrap()
            .is_empty()
    );
}

/// The session, one wallet, the bytes of its complete automatic backup,
/// and its backups folder emptied.
fn emptied(dir: &Path, notices: Arc<Notices>) -> (Engine, Arc<NetworkSession>, WalletId, Vec<u8>) {
    let (engine, session) = open(dir.join("a"), notices);
    let id = import(&engine, &session, PHRASE);
    wait_for_listed(&engine, &session, 1);
    let listed = list_automatic(&session.backup_directory(), "regtest", None).unwrap();
    let bytes = std::fs::read(&listed[0].path).unwrap();
    std::fs::remove_file(&listed[0].path).unwrap();
    (engine, session, id, bytes)
}

fn run_automatic(engine: &Engine, session: &Arc<NetworkSession>, id: WalletId) -> AutomaticRun {
    let s = Arc::clone(session);
    engine
        .block_on(
            session
                .rt
                .spawn(async move { s.automatic_backup(id).await }),
        )
        .unwrap()
}

/// A file at this minute's name that cannot be read fails the backup: it
/// is neither set aside nor taken for a backup. Unix only: it takes read
/// permission away.
#[cfg(unix)]
#[test]
fn an_unreadable_file_at_this_minutes_name_fails_the_backup() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, bytes) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::fs::write(&name, &bytes).unwrap();
    set_mode(&name, 0o000);

    run_automatic(&engine, &session, id);
    set_mode(&name, 0o600);

    assert_eq!(failures(&engine, &session).len(), 1);
    notices.wait_for_backup_failed(1);
    assert_eq!(std::fs::read(&name).unwrap(), bytes);
    let own = name.file_name().unwrap().to_string_lossy().into_owned();
    let ours: Vec<_> = file_names(&backups)
        .into_iter()
        .filter(|n| n.contains(&own))
        .collect();
    assert_eq!(ours, [own]);
}

/// A complete backup already at this minute's name counts as this
/// minute's backup: a failure recorded before it is cleared.
#[test]
fn a_complete_backup_at_this_minutes_name_clears_a_failure_record() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, bytes) = emptied(dir.path(), Arc::new(Notices::default()));
    record_failure_at(&session, id, "1000");
    not_at_a_minute_end();
    let name = session
        .backup_directory()
        .join(automatic_name(&id, unix_now()));
    std::fs::write(&name, &bytes).unwrap();

    run_automatic(&engine, &session, id);

    assert!(failures(&engine, &session).is_empty());
    assert_eq!(std::fs::read(&name).unwrap(), bytes);
}

/// A backup that finds the vault locked while it is built puts back the
/// failure recorded before it.
#[test]
fn a_backup_the_vault_lock_ends_puts_back_an_earlier_failure() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, ids) = relaunched_locked(dir.path(), &[PHRASE], 10, notices.clone());
    record_failure_at(&session, ids[0], "1000");
    let pause = Arc::new(TestPause::default());
    *session.backups.pause.lock().unwrap() = Some(pause.clone());
    unlock(&engine, &session);
    assert!(pause.wait_reached(Duration::from_secs(60)));

    session.lock_vault_sync().unwrap();
    pause.release.notify_one();
    wait_until("the run ends", || {
        session.backups.runs_done.load(Ordering::SeqCst) == 1
    });

    assert_eq!(failures(&engine, &session), [(ids[0], 1000)]);
    assert_eq!(notices.backup_failed(), 0);
}

/// An incomplete file at this minute's name that cannot be set aside (every
/// `.incomplete` name is taken) fails the backup, and every file is kept.
#[test]
fn an_incomplete_file_that_cannot_be_set_aside_fails_the_backup() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, bytes) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    let cut = &bytes[..(header_end(&bytes) + bytes.len()) / 2];
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::fs::write(&name, cut).unwrap();
    for n in 1..=100 {
        let mut aside = name.clone().into_os_string();
        aside.push(if n == 1 {
            ".incomplete".to_owned()
        } else {
            format!(".incomplete-{n}")
        });
        std::fs::write(aside, b"aside").unwrap();
    }

    run_automatic(&engine, &session, id);

    assert_eq!(failures(&engine, &session).len(), 1);
    notices.wait_for_backup_failed(1);
    assert_eq!(std::fs::read(&name).unwrap(), cut);
    let own = name.file_name().unwrap().to_string_lossy().into_owned();
    let ours = file_names(&backups)
        .into_iter()
        .filter(|n| n.contains(&own))
        .count();
    assert_eq!(ours, 101);
}

/// A backup whose header time is later than now (written while the clock
/// was ahead) does not push out the backups written after it: with
/// "Backups to keep" 1 the new backup is kept and the future-dated one
/// goes.
#[test]
fn a_backup_dated_in_the_future_does_not_push_out_new_ones() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, bytes) = emptied(dir.path(), Arc::new(Notices::default()));
    engine.block_on(session.set_backup_policy(1)).unwrap();
    let backups = session.backup_directory();
    let future = backups.join(at_minute(id, 0));
    std::fs::write(
        &future,
        with_header(&bytes, |h| h["created_at"] = (unix_now() + 86_400).into()),
    )
    .unwrap();
    assert_eq!(list_automatic(&backups, "regtest", None).unwrap().len(), 1);

    run_automatic(&engine, &session, id);

    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_ne!(listed[0].path, future);
    assert!(!future.exists());
}

/// Runs `fut` on the session's runtime. If it takes over a minute, opens
/// each of `fifos` for writing, which ends an open blocked on it, and fails.
#[cfg(unix)]
fn unless_stuck_on<T: Send + 'static>(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    fifos: &[&Path],
    fut: impl std::future::Future<Output = T> + Send + 'static,
) -> T {
    use std::os::unix::fs::OpenOptionsExt;
    let task = session.rt.spawn(fut);
    match engine.block_on(async { tokio::time::timeout(Duration::from_secs(60), task).await }) {
        Ok(out) => out.unwrap(),
        Err(_) => {
            for fifo in fifos {
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(fifo);
            }
            panic!("blocked on a FIFO");
        }
    }
}

#[cfg(unix)]
fn mkfifo(path: &Path) {
    let status = std::process::Command::new("mkfifo")
        .arg("-m")
        .arg("600")
        .arg(path)
        .status()
        .unwrap();
    assert!(status.success());
}

/// A FIFO under an automatic backup's name is never opened: listing skips
/// it, and one at this minute's name is set aside and that minute's backup
/// written.
#[cfg(unix)]
#[test]
fn a_fifo_at_a_backups_name_neither_hangs_nor_stops_a_backup() {
    use std::os::unix::fs::FileTypeExt;
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, _) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    let old = backups.join(at_minute(id, 0));
    mkfifo(&old);
    let s = Arc::clone(&session);
    let listed = unless_stuck_on(&engine, &session, &[&old], async move {
        s.automatic_backups(None).await
    });
    assert!(listed.unwrap().is_empty());

    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    mkfifo(&name);
    let s = Arc::clone(&session);
    unless_stuck_on(&engine, &session, &[&old, &name], async move {
        s.automatic_backup(id).await
    });

    assert!(failures(&engine, &session).is_empty());
    assert_eq!(notices.backup_failed(), 0);
    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, name);
    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    assert!(std::fs::metadata(aside).unwrap().file_type().is_fifo());
    assert!(std::fs::metadata(&old).unwrap().file_type().is_fifo());
}

/// A complete backup at this minute's name whose header is older than a
/// recorded failure (a clock stepped back during a run, or a file copied
/// in) leaves the record.
#[test]
fn a_backup_at_this_minutes_name_older_than_a_failure_leaves_its_record() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, bytes) = emptied(dir.path(), Arc::new(Notices::default()));
    record_failure_at(&session, id, "3000");
    not_at_a_minute_end();
    let name = session
        .backup_directory()
        .join(automatic_name(&id, unix_now()));
    let old = with_header(&bytes, |h| h["created_at"] = 2000.into());
    std::fs::write(&name, &old).unwrap();

    run_automatic(&engine, &session, id);

    assert_eq!(failures(&engine, &session), [(id, 3000)]);
    assert_eq!(std::fs::read(&name).unwrap(), old);
}

/// With "Backups to keep" 2, a backup dated in the future ranks below an
/// older one: the new and the older backups are kept, the future-dated one
/// goes.
#[test]
fn a_backup_dated_in_the_future_ranks_below_an_older_one() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, bytes) = emptied(dir.path(), Arc::new(Notices::default()));
    engine.block_on(session.set_backup_policy(2)).unwrap();
    let backups = session.backup_directory();
    let older = backups.join(at_minute(id, 1));
    std::fs::write(&older, &bytes).unwrap();
    let future = backups.join(at_minute(id, 0));
    std::fs::write(
        &future,
        with_header(&bytes, |h| h["created_at"] = (unix_now() + 86_400).into()),
    )
    .unwrap();
    assert_eq!(list_automatic(&backups, "regtest", None).unwrap().len(), 2);

    run_automatic(&engine, &session, id);

    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|b| b.path == older));
    assert!(!future.exists());
}

fn recorded(session: &NetworkSession, id: WalletId) -> Option<String> {
    session
        .live()
        .unwrap()
        .appdb
        .setting(&dw_appdb::local_scope(&id.to_string()), AUTO_FAILED_SETTING)
        .unwrap()
}

/// A record whose time cannot be read is deleted by a complete backup of
/// this minute at this minute's name, and kept by one with an older header.
#[test]
fn an_unreadable_record_is_cleared_only_by_a_backup_of_this_minute() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, bytes) = emptied(dir.path(), Arc::new(Notices::default()));
    let backups = session.backup_directory();
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));

    record_failure_at(&session, id, "not a time");
    let old = with_header(&bytes, |h| h["created_at"] = 2000.into());
    std::fs::write(&name, &old).unwrap();
    run_automatic(&engine, &session, id);
    assert_eq!(recorded(&session, id).as_deref(), Some("not a time"));

    std::fs::remove_file(&name).unwrap();
    let now = with_header(&bytes, |h| h["created_at"] = unix_now().into());
    std::fs::write(&name, &now).unwrap();
    run_automatic(&engine, &session, id);
    assert_eq!(recorded(&session, id), None);
    assert_eq!(std::fs::read(&name).unwrap(), now);
}

/// A directory at this minute's name is set aside as `.incomplete` and
/// that minute's backup is written.
#[test]
fn a_directory_at_this_minutes_name_is_set_aside() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, _) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::fs::create_dir(&name).unwrap();

    run_automatic(&engine, &session, id);

    assert!(failures(&engine, &session).is_empty());
    assert_eq!(notices.backup_failed(), 0);
    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, name);
    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    assert!(std::fs::metadata(aside).unwrap().is_dir());
}

/// A symlink to a FIFO at this minute's name is followed and never
/// opened: it is set aside and that minute's backup is written.
#[cfg(unix)]
#[test]
fn a_symlink_to_a_fifo_at_this_minutes_name_is_set_aside() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, _) = emptied(dir.path(), Arc::new(Notices::default()));
    let backups = session.backup_directory();
    let fifo = dir.path().join("fifo");
    mkfifo(&fifo);
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::os::unix::fs::symlink(&fifo, &name).unwrap();

    let s = Arc::clone(&session);
    unless_stuck_on(&engine, &session, &[&fifo], async move {
        s.automatic_backup(id).await
    });

    assert!(failures(&engine, &session).is_empty());
    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, name);
    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    assert!(
        std::fs::symlink_metadata(aside)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

/// `bytes`, a complete backup, with its bundle's sealed payload taken out:
/// its metadata still matches, but restore refuses it.
fn without_payload(bytes: &[u8]) -> Vec<u8> {
    let end = header_end(bytes);
    let mut body: serde_json::Value = serde_json::from_slice(&bytes[end..]).unwrap();
    body["bundles"][0]
        .as_object_mut()
        .unwrap()
        .remove("payload")
        .unwrap();
    let mut out = bytes[..end].to_vec();
    out.extend(serde_json::to_vec(&body).unwrap());
    out.push(b'\n');
    out
}

/// A newer file missing its payload, which restore refuses, is neither
/// counted as a backup nor deleted, and with one kept the older restorable
/// backup stays.
#[test]
fn a_newer_backup_missing_its_payload_costs_no_restorable_one() {
    let dir = dw_testutil::private_tempdir();
    let (_engine, session, id, bytes) = automatic_bytes(dir.path(), PHRASE);
    let backups = session.backup_directory();
    let older = backups.join(at_minute(id, 1));
    std::fs::write(&older, &bytes).unwrap();
    let damaged = backups.join(at_minute(id, 2));
    std::fs::write(&damaged, without_payload(&bytes)).unwrap();

    let listed = list_automatic(&backups, "regtest", None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].path, older);
    prune(&backups, "regtest", None, 1, None).unwrap();

    assert_eq!(std::fs::read(&older).unwrap(), bytes);
    assert!(damaged.exists());
}

/// A file missing its payload at this minute's name does not stand for
/// that minute's backup or clear a failure: it is set aside and the backup
/// written.
#[test]
fn a_backup_missing_its_payload_at_this_minutes_name_is_set_aside() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, bytes) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    record_failure_at(&session, id, "5");
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    let damaged = without_payload(&bytes);
    std::fs::write(&name, &damaged).unwrap();

    run_automatic(&engine, &session, id);

    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    assert_eq!(std::fs::read(&aside).unwrap(), damaged);
    assert!(matches!(
        check_automatic(&name, id, "regtest").unwrap(),
        AtName::Complete(_)
    ));
    assert!(failures(&engine, &session).is_empty());
}

/// A backup published but whose folder could not be synced fails: the
/// older copy is not pruned and the failure stays recorded. A retry in the
/// same minute takes the name for the backup only once it is durable.
#[test]
fn a_backup_not_made_durable_keeps_the_older_copy_and_its_failure() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, bytes) = emptied(dir.path(), notices.clone());
    engine.block_on(session.set_backup_policy(1)).unwrap();
    wait_until("no backup in flight", || {
        session.backups.in_flight().is_empty()
    });
    let backups = session.backup_directory();
    for f in list_automatic(&backups, "regtest", None).unwrap() {
        std::fs::remove_file(f.path).unwrap();
    }
    let older = backups.join(at_minute(id, 1));
    std::fs::write(&older, &bytes).unwrap();
    not_at_a_minute_end();
    let publish_after = || {
        dw_appdb::fault::arm(&backups, "backup.publish", dw_appdb::fault::Stage::After);
    };

    publish_after();
    run_automatic(&engine, &session, id);

    let name = backups.join(automatic_name(&id, unix_now()));
    assert!(name.exists());
    assert_eq!(std::fs::read(&older).unwrap(), bytes);
    assert_eq!(failures(&engine, &session).len(), 1);
    notices.wait_for_backup_failed(1);

    publish_after();
    run_automatic(&engine, &session, id);
    assert_eq!(failures(&engine, &session).len(), 1);

    run_automatic(&engine, &session, id);
    assert!(failures(&engine, &session).is_empty());
    assert_eq!(std::fs::read(&older).unwrap(), bytes);
}

/// The failure record an automatic backup writes as it starts is durable:
/// after a power loss, a backup that had started still shows as failed.
#[test]
fn a_started_automatic_backup_is_recorded_through_a_power_loss() {
    use dw_appdb::durability_sim::{arm, power_loss};
    let dir = dw_testutil::private_tempdir();
    let (engine, session, id, _) = emptied(dir.path(), Arc::new(Notices::default()));
    let db = session.data_dir().join(dw_appdb::APP_DB_FILE);
    drop(session);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    arm(&db, false);
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
    wait_for_listed(&engine, &session, 1);
    drop(session);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    power_loss(&db);

    let recorded: Option<String> = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT value FROM settings_kv WHERE scope = ?1 AND key = ?2",
            [
                dw_appdb::local_scope(&id.to_string()).as_str(),
                AUTO_FAILED_SETTING,
            ],
            |r| r.get(0),
        )
        .ok();
    assert!(recorded.is_some());
}

/// A dangling symlink at this minute's name is there: it is set aside and
/// the backup written.
#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_this_minutes_name_is_set_aside() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, _) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    not_at_a_minute_end();
    let name = backups.join(automatic_name(&id, unix_now()));
    std::os::unix::fs::symlink(dir.path().join("missing"), &name).unwrap();

    run_automatic(&engine, &session, id);

    let mut aside = name.clone().into_os_string();
    aside.push(".incomplete");
    let aside = PathBuf::from(aside);
    assert!(aside.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(matches!(
        check_automatic(&name, id, "regtest").unwrap(),
        AtName::Complete(_)
    ));
    assert!(failures(&engine, &session).is_empty());
}

/// A failure record cleared without a backup written (one already at this
/// minute's name, or backups off) still sends `Backups`.
#[test]
fn clearing_a_failure_sends_backups() {
    let dir = dw_testutil::private_tempdir();
    let notices = Arc::new(Notices::default());
    let (engine, session, id, bytes) = emptied(dir.path(), notices.clone());
    let backups = session.backup_directory();
    not_at_a_minute_end();
    std::fs::write(backups.join(automatic_name(&id, unix_now())), &bytes).unwrap();
    record_failure_at(&session, id, "5");
    let before = notices.written();
    run_automatic(&engine, &session, id);
    assert!(failures(&engine, &session).is_empty());
    assert_eq!(notices.written(), before + 1);

    engine.block_on(session.set_backup_policy(0)).unwrap();
    record_failure_at(&session, id, "6");
    let before = notices.written();
    run_automatic(&engine, &session, id);
    assert!(failures(&engine, &session).is_empty());
    assert_eq!(notices.written(), before + 1);
}

fn export_notices(notices: &Notices) -> Vec<(NoticeCode, String)> {
    let seen = notices.notices.lock().unwrap();
    seen.iter()
        .filter(|(c, _)| {
            matches!(
                c,
                NoticeCode::ExportStagingDeleted | NoticeCode::ExportStagingNotDeleted
            )
        })
        .cloned()
        .collect()
}

/// A Dash Core export stopped between its staging file's sync and its
/// publish leaves only that file, which holds keys in the clear; the next
/// start deletes it and says so.
#[test]
fn an_export_stopped_before_its_publish_is_cleaned_up_at_the_next_start() {
    if let Some(root) = std::env::var_os(CHILD_DIR) {
        let root = PathBuf::from(root);
        let (engine, session) = launch(
            root.join("data"),
            Arc::new(Notices::default()),
            &Arc::new(MemoryOsStore::new()),
        );
        engine
            .block_on(session.vault_op(|v| v.create(Some(VAULT_PASSPHRASE))))
            .unwrap();
        engine.block_on(session.set_backup_policy(0)).unwrap();
        let id = import(&engine, &session, PHRASE);
        let grant = session
            .vault()
            .authorize(
                dw_vault::GrantPurpose::RevealSecret,
                Some(&id.0),
                dw_vault::Credential::Passphrase(VAULT_PASSPHRASE),
            )
            .unwrap()
            .id;
        let _ = engine.block_on(session.export_for_core(
            id,
            crate::compat::CoreExportFormat::DumpWallet,
            root.join("chosen/wallet.txt"),
            grant,
        ));
        return;
    }
    let dir = dw_testutil::private_tempdir();
    let chosen = private_dir(&dir.path().join("chosen"));
    let stopped = stopped_in_child(
        "backup::backup_tests::an_export_stopped_before_its_publish_is_cleaned_up_at_the_next_start",
        dir.path(),
    );
    let names = file_names(&chosen);
    stopped.unwrap();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(
        names[0].starts_with(".dash-wallet-export.tmp-"),
        "{names:?}"
    );

    let notices = Arc::new(Notices::default());
    let (_engine, _session) = launch(
        dir.path().join("data"),
        notices.clone(),
        &Arc::new(MemoryOsStore::new()),
    );

    assert!(file_names(&chosen).is_empty());
    assert_eq!(
        export_notices(&notices),
        [(
            NoticeCode::ExportStagingDeleted,
            chosen.join(&names[0]).display().to_string()
        )]
    );
}

/// An export staging file the start cannot delete keeps its record and is
/// named in a notice at every start while it is there (the renderer's card;
/// a dismissal there changes nothing here); once it is gone, the next start
/// clears the record and sends nothing.
#[test]
fn an_export_staging_file_the_start_cannot_delete_is_named_at_every_start_until_it_is_gone() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join("a");
    let os_store = Arc::new(MemoryOsStore::new());
    let records = |session: &NetworkSession| {
        session
            .live()
            .unwrap()
            .appdb
            .settings_with_prefix(dw_appdb::GLOBAL_SCOPE, crate::compat::EXPORT_STAGING_PREFIX)
            .unwrap()
            .len()
    };
    let restart = |engine: Engine, session: Arc<NetworkSession>| {
        engine.block_on(engine.shutdown()).unwrap();
        drop(session);
        drop(engine);
        let notices = Arc::new(Notices::default());
        let (engine, session) = launch(root.clone(), notices.clone(), &os_store);
        (engine, session, notices)
    };
    let (engine, session) = launch(root.clone(), Arc::new(Notices::default()), &os_store);
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let chosen = private_dir(&dir.path().join("chosen"));
    let dest = chosen.join("wallet.txt");
    let stuck = crate::compat::staging_path(&dest, crate::compat::EXPORT_STAGING_TAG).unwrap();
    std::fs::create_dir(&stuck).unwrap();
    std::fs::write(stuck.join("x"), b"").unwrap();
    crate::compat::record_export_staging(&session.live().unwrap().appdb, &stuck, &dest).unwrap();
    let named = [(
        NoticeCode::ExportStagingNotDeleted,
        stuck.display().to_string(),
    )];

    let (engine, session, notices) = restart(engine, session);
    assert_eq!(export_notices(&notices), named);
    assert_eq!(records(&session), 1);

    let (engine, session, notices) = restart(engine, session);
    assert_eq!(export_notices(&notices), named);
    assert_eq!(records(&session), 1);

    std::fs::remove_dir_all(&stuck).unwrap();
    let (engine, session, notices) = restart(engine, session);
    assert_eq!(export_notices(&notices), []);
    assert_eq!(records(&session), 0);

    let (_engine, _session, notices) = restart(engine, session);
    assert_eq!(export_notices(&notices), []);
}
