//! Restore on restart (R3-RESTORE-DESIGN-v2.md rev 2.1, Q92 v2.1): the
//! fresh run, its refusals and out-of-space path, every resume stop point,
//! S4, the attempt record, the mark, and the copy.

use super::*;
use crate::copy::{self, BackupState, CantOpenCause};
use crate::error::StoreUpgradeError;
use crate::fsx::{arm_fault, disarm_fault};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::restore::{
    self as r, IncompleteCause, RefusalCause, RestoreInfo, S4_FOR_TEST, S4Policy,
};
use crate::snapshot::{
    STORE_FILES, SnapshotInfo, SnapshotState, UPGRADE_BACKUPS_DIR, snapshot_state, take_snapshot,
};
use crate::sql::sha256_file;

const REPLACE_ORDER: [&str; 4] = [
    "app.sqlite",
    "dispatch.sqlite",
    "shielded.sqlite",
    "wallet.sqlite",
];
const FOLDER: &str = "/home/u/.local/share/Dash Wallet/regtest";
const REQUEST_KEY: &str = "regtest/upgrade-backups/RESTORE-REQUESTED";

type Hashes = BTreeMap<String, Option<String>>;

/// `DWSU_FAIL_FIRST=<name>` turns on a fail-first switch for one run.
fn fail_first(name: &str) -> bool {
    std::env::var("DWSU_FAIL_FIRST").is_ok_and(|v| v == name)
}

fn hashes(dir: &Path) -> Hashes {
    STORE_FILES
        .iter()
        .map(|n| (n.to_string(), sha256_file(&dir.join(n)).ok()))
        .collect()
}

fn before_set(s: &SnapshotInfo) -> Hashes {
    STORE_FILES
        .iter()
        .map(|n| {
            let sha = s.files().iter().find(|f| f.name == *n);
            (n.to_string(), sha.map(|f| f.sha256.clone()))
        })
        .collect()
}

fn side(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn sides_left(net: &Path) -> Vec<String> {
    let mut v = Vec::new();
    for n in STORE_FILES {
        for s in ["-wal", "-shm", "-journal"] {
            if side(&net.join(n), s).exists() {
                v.push(format!("{n}{s}"));
            }
        }
    }
    v
}

/// Fold every store's log into its main file, as a clean close does.
fn settle_stores(net: &Path) {
    for n in STORE_FILES {
        let p = net.join(n);
        if p.exists() {
            exec(&p, "PRAGMA wal_checkpoint(TRUNCATE)");
        }
    }
}

/// A fork file upgraded and used: snapshot, reconcile, the upstream open,
/// then a change to app.sqlite since the upgrade.
fn upgraded() -> (Fixture, SnapshotInfo) {
    let f = fixture("v0.1.0");
    settle_stores(&f.net);
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    drop(open_and_load(&f.net));
    exec(
        &f.net.join("app.sqlite"),
        "CREATE TABLE dwsu_since (a INTEGER); INSERT INTO dwsu_since VALUES (1)",
    );
    settle_stores(&f.net);
    crate::token::reset_opened_for_test();
    (f, snapshot)
}

/// [`upgraded`] with a restore requested: the tree before the request, and
/// the live (after) set.
fn requested() -> (Fixture, SnapshotInfo, BTreeMap<String, String>, Hashes) {
    let (f, s) = upgraded();
    let pre = tree(&f.root);
    r::request_restore(&f.net, &s).unwrap();
    let after = hashes(&f.net);
    assert_ne!(after, before_set(&s));
    (f, s, pre, after)
}

/// Upstream names its pre-migration backup by the wall-clock second, so a
/// second upgrade of one store within a second collides.
fn next_wall_second() {
    let secs = || {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    };
    let start = secs();
    while secs() == start {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn backups(f: &Fixture) -> PathBuf {
    f.net.join(UPGRADE_BACKUPS_DIR)
}

fn present(f: &Fixture, name: &str) -> bool {
    fs::symlink_metadata(backups(f).join(name)).is_ok()
}

fn dirs(f: &Fixture, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(backups(f))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("{prefix}-"))
        })
        .collect();
    v.sort();
    v
}

fn run(f: &Fixture, at: SystemTime) -> crate::Result<Option<RestoreInfo>> {
    r::run_pending_restore(&f.net, at)
}

fn run_with_fault(
    f: &Fixture,
    point: &'static str,
    skip: u32,
    at: SystemTime,
) -> StoreUpgradeError {
    arm_fault(point, skip);
    let found = run(f, at);
    disarm_fault();
    found.expect_err(point)
}

fn without_request(mut t: BTreeMap<String, String>) -> BTreeMap<String, String> {
    t.remove(REQUEST_KEY);
    t
}

/// What an old build leaves: a change held only in the store's log, the
/// log and an `-shm` beside the main file it never checkpointed.
fn leave_hot_wal(path: &Path, sql: &str) {
    let pristine = fs::read(path).unwrap();
    let conn = Connection::open(path).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    conn.execute_batch(&format!("PRAGMA wal_autocheckpoint = 0; {sql}"))
        .unwrap();
    let held = fs::read(side(path, "-wal")).unwrap();
    drop(conn);
    fs::write(path, &pristine).unwrap();
    fs::write(side(path, "-wal"), held).unwrap();
    fs::write(side(path, "-shm"), vec![0u8; 32768]).unwrap();
}

/// An old build's start: app.sqlite written and checkpointed, then a change
/// held in its log; dispatch and shielded written; logs left beside each.
/// Returns app.sqlite's main file sha256 as the old build left it.
fn old_build_writes(net: &Path) -> String {
    let app = net.join("app.sqlite");
    exec(
        &app,
        "CREATE TABLE IF NOT EXISTS dwsu_old (a INTEGER); INSERT INTO dwsu_old VALUES (1)",
    );
    leave_hot_wal(&app, "INSERT INTO dwsu_old VALUES (2)");
    for n in ["dispatch.sqlite", "shielded.sqlite"] {
        let p = net.join(n);
        exec(&p, "CREATE TABLE IF NOT EXISTS dwsu_old (a INTEGER)");
        leave_hot_wal(&p, "INSERT INTO dwsu_old VALUES (3)");
    }
    sha256_file(&app).unwrap()
}

const PENDING_TXID: [u8; 32] = [0xAA; 32];

/// An unmined payment of the first coin in `wallet.sqlite`.
fn pending_sql() -> String {
    let txid = "AA".repeat(32);
    format!(
        "INSERT INTO core_transactions (wallet_id, txid, height, finalized, record_blob) \
           SELECT wallet_id, x'{txid}', NULL, 0, x'00' FROM core_utxos ORDER BY outpoint LIMIT 1; \
         UPDATE core_utxos SET spent_in_txid = x'{txid}' \
           WHERE rowid = (SELECT rowid FROM core_utxos ORDER BY outpoint LIMIT 1)"
    )
}

fn first_outpoint(wallet: &Path) -> dashcore::OutPoint {
    let blob: Vec<u8> = crate::sql::open_immutable(wallet)
        .unwrap()
        .query_row(
            "SELECT outpoint FROM core_utxos ORDER BY outpoint LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let (op, used): (dashcore::OutPoint, usize) =
        bincode::serde::decode_from_slice(&blob, bincode::config::standard()).unwrap();
    assert_eq!(used, blob.len());
    assert_eq!(&op.txid.to_byte_array()[..], &blob[1..33]);
    op
}

fn assert_pending_is_the_one_payment(f: &Fixture, wallet_for_coin: &Path) {
    let mark = r::restored_mark(&f.net).unwrap().expect("mark set");
    assert_eq!(mark.pending.len(), 1, "{mark:?}");
    assert_eq!(mark.pending[0].txid.to_byte_array(), PENDING_TXID);
    assert_eq!(
        mark.pending[0].inputs,
        vec![first_outpoint(wallet_for_coin)]
    );
}

#[test]
fn restore_runs_keeps_both_sets_and_the_restored_store_reopens_strict() {
    let (f, s, _, after) = requested();
    let prepared = crate::prepare_with(&f.net, build(), later(60)).unwrap();
    let info = prepared.restored.expect("restored");
    assert!(!info.resumed && !info.kept_live && info.during_dirs.is_empty());
    assert_eq!(info.snapshot_dir, s.dir());
    assert_eq!(info.after_dir.parent().unwrap(), backups(&f));
    assert!(
        info.after_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("after-upgrade-")
    );
    assert_eq!(hashes(&f.net), before_set(&s));
    assert_eq!(hashes(&info.after_dir), after);
    assert!(sides_left(&f.net).is_empty());
    assert!(!present(&f, r::RESTORE_REQUESTED) && !present(&f, r::RESTORE_JOURNAL));
    assert!(!present(&f, r::RESTORE_ATTEMPT));
    assert!(dirs(&f, "restore-staging").is_empty());
    assert!(r::restored_mark_present(&f.net));
    assert!(matches!(prepared.precheck, crate::Precheck::Fork { .. }));
    let again = prepared.snapshot.unwrap();
    assert_eq!(again.dir(), s.dir(), "the restored set is the snapshot");
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &again, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    next_wall_second();
    let (p, token) = open_and_load(&f.net);
    assert!(r::clear_restored_mark_on_first_all_synced(&token).unwrap());
    assert!(!r::restored_mark_present(&f.net));
    assert!(!r::clear_restored_mark_on_first_all_synced(&token).unwrap());
    drop(p);
    assert_eq!(hashes(&info.after_dir), after, "after-upgrade untouched");
}

#[test]
fn restore_unreachable_without_explicit_request() {
    let (f, s) = upgraded();
    let pre = tree(&f.root);
    assert!(run(&f, later(60)).unwrap().is_none());
    let err = r::restore(&f.net, &s, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotInvalid { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), pre);
    r::request_restore(&f.net, &s).unwrap();
    let info = r::restore(&f.net, &s, later(60)).unwrap();
    assert_eq!(hashes(&f.net), before_set(&s));
    assert!(info.after_dir.starts_with(backups(&f)));
}

#[test]
fn request_names_only_a_completed_snapshot_in_this_directory() {
    let (f, s) = upgraded();
    let other = upgraded().1;
    let err = r::request_restore(&f.net, &other).unwrap_err();
    assert!(matches!(err, StoreUpgradeError::SnapshotInvalid { .. }));
    fs::write(s.dir().join("app.sqlite"), b"x").unwrap();
    assert!(r::request_restore(&f.net, &s).is_err());
    assert!(!present(&f, r::RESTORE_REQUESTED));
}

fn assert_refused_once(f: &Fixture, want_tree: BTreeMap<String, String>, cause: RefusalCause) {
    let err = run(f, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::RestoreRefused { cause: c } if c == cause),
        "{cause:?}: {err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(
        copy::screen_for(&err, BackupState::Verified),
        Some(copy::restore_refused(cause))
    );
    assert_eq!(tree(&f.root), want_tree, "{cause:?}");
    assert!(run(f, later(120)).unwrap().is_none(), "no loop");
}

#[test]
fn refused_restart_leaves_every_file_and_listing_unchanged_and_clears_the_request_once() {
    let (f, s, _, _) = requested();
    fs::remove_dir_all(s.dir()).unwrap();
    assert_refused_once(
        &f,
        without_request(tree(&f.root)),
        RefusalCause::MissingBackup,
    );

    let (f, s, _, _) = requested();
    fs::remove_file(s.dir().join("dispatch.sqlite")).unwrap();
    assert_refused_once(
        &f,
        without_request(tree(&f.root)),
        RefusalCause::MissingBackup,
    );

    let (f, s, _, _) = requested();
    let app = s.dir().join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[200] ^= 1;
    fs::write(&app, bytes).unwrap();
    assert_refused_once(
        &f,
        without_request(tree(&f.root)),
        RefusalCause::MismatchedBackup,
    );

    let (f, _, _, _) = requested();
    let req = backups(&f).join(r::RESTORE_REQUESTED);
    let text = fs::read_to_string(&req).unwrap();
    let manifest = text.lines().last().unwrap().to_string();
    fs::write(
        &req,
        text.replace(&manifest, &format!("manifest {}", "0".repeat(64))),
    )
    .unwrap();
    assert_refused_once(
        &f,
        without_request(tree(&f.root)),
        RefusalCause::MismatchedBackup,
    );

    for garbled in [b"garbage".to_vec(), vec![0xff, 0xfe], Vec::new()] {
        let (f, _, _, _) = requested();
        fs::write(backups(&f).join(r::RESTORE_REQUESTED), garbled).unwrap();
        assert_refused_once(
            &f,
            without_request(tree(&f.root)),
            RefusalCause::GarbledMarker,
        );
    }
}

#[test]
fn restore_refuses_older_and_newer_marker_versions() {
    for format in ["format 0", "format 2"] {
        let (f, _, _, _) = requested();
        let req = backups(&f).join(r::RESTORE_REQUESTED);
        let text = fs::read_to_string(&req)
            .unwrap()
            .replace("format 1", format);
        fs::write(&req, text).unwrap();
        assert_refused_once(
            &f,
            without_request(tree(&f.root)),
            RefusalCause::GarbledMarker,
        );
    }
}

#[test]
fn restore_refuses_after_a_store_opened_in_process() {
    let (f, _, _, _) = requested();
    let want = tree(&f.root);
    crate::token::note_stores_opening();
    let err = run(&f, later(60)).unwrap_err();
    crate::token::reset_opened_for_test();
    assert!(
        matches!(err, StoreUpgradeError::RestoreAfterOpen),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(tree(&f.root), want);
    assert!(run(&f, later(60)).unwrap().is_some());
}

#[test]
fn restore_refuses_while_wallet_store_is_held() {
    let (f, s) = upgraded();
    let (p, token) = open_and_load(&f.net);
    r::request_restore(&f.net, &s).unwrap();
    crate::token::reset_opened_for_test();
    let live = hashes(&f.net);
    let err = run(&f, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    assert!(present(&f, r::RESTORE_REQUESTED) && !present(&f, r::RESTORE_JOURNAL));
    assert!(dirs(&f, "after-upgrade").is_empty());
    assert_eq!(hashes(&f.net), live);
    let err = keep_current_err(&f);
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    drop(p);
    drop(token);
}

fn keep_current_err(f: &Fixture) -> StoreUpgradeError {
    r::keep_current_files(&f.net, later(60)).unwrap_err()
}

#[test]
fn restore_refuses_while_app_db_lock_is_held() {
    let (f, _, _, _) = requested();
    let want = tree(&f.root);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.net.join("app.sqlite.lock"))
        .unwrap();
    lock.lock().unwrap();
    let err = run(&f, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), want);
    drop(lock);
    assert!(run(&f, later(60)).unwrap().is_some());
}

#[test]
fn prepare_runs_pending_restore_before_precheck_and_any_open() {
    let (f, s, _, _) = requested();
    assert!(matches!(
        crate::precheck_with(&f.net, build()).unwrap_err(),
        StoreUpgradeError::RestorePending { .. }
    ));
    let err = PersisterOpened::open(config(&f.net))
        .err()
        .expect("refused");
    assert!(
        matches!(err, StoreUpgradeError::RestorePending { .. }),
        "{err:?}"
    );
    crate::token::reset_opened_for_test();
    let prepared = crate::prepare_with(&f.net, build(), later(60)).unwrap();
    assert!(prepared.restored.is_some());
    assert!(matches!(prepared.precheck, crate::Precheck::Fork { .. }));
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn after_set_complete_and_mark_written_before_the_first_replace() {
    let (f, s, _, after) = requested();
    let err = run_with_fault(&f, "restore.after_journal", 0, later(60));
    assert!(
        matches!(err, StoreUpgradeError::InjectedFault(_)),
        "{err:?}"
    );
    assert_eq!(hashes(&f.net), after, "nothing replaced yet");
    assert!(present(&f, r::RESTORE_JOURNAL) && r::restored_mark_present(&f.net));
    assert!(!present(&f, r::RESTORE_ATTEMPT));
    let [after_dir] = &dirs(&f, "after-upgrade")[..] else {
        panic!("one after-upgrade dir")
    };
    assert_eq!(hashes(after_dir), after);
    let [staging] = &dirs(&f, "restore-staging")[..] else {
        panic!("one staging dir")
    };
    assert_eq!(hashes(staging), before_set(&s));
    let info = run(&f, later(120)).unwrap().unwrap();
    assert!(info.resumed && info.during_dirs.is_empty());
    assert_eq!(&info.after_dir, after_dir);
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn marker_removed_only_after_swap_and_fsync() {
    let (f, s, _, _) = requested();
    let err = run_with_fault(&f, "restore.before_journal_removal", 0, later(60));
    assert!(
        matches!(err, StoreUpgradeError::InjectedFault(_)),
        "{err:?}"
    );
    assert_eq!(hashes(&f.net), before_set(&s));
    assert!(present(&f, r::RESTORE_JOURNAL));
    let info = run(&f, later(120)).unwrap().unwrap();
    assert!(info.resumed && !info.kept_live && info.during_dirs.is_empty());
    assert!(!present(&f, r::RESTORE_JOURNAL));
}

#[test]
fn quiesced_but_unstaged_restore_runs_again_at_the_next_start() {
    let (f, s, _, after) = requested();
    let err = run_with_fault(&f, "restore.after_quiesce", 0, later(60));
    assert!(
        matches!(err, StoreUpgradeError::InjectedFault(_)),
        "{err:?}"
    );
    assert_eq!(hashes(&f.net), after);
    assert!(present(&f, r::RESTORE_REQUESTED) && !present(&f, r::RESTORE_JOURNAL));
    assert!(dirs(&f, "after-upgrade").is_empty());
    run(&f, later(120)).unwrap().unwrap();
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn wallet_is_replaced_last() {
    for k in 0..4 {
        let (f, s, _, after) = requested();
        let before = before_set(&s);
        let err = run_with_fault(&f, "restore.replaced", k, later(60));
        assert!(
            matches!(
                err,
                StoreUpgradeError::RestoreIncomplete {
                    journal_written: true,
                    ..
                }
            ),
            "{err:?}"
        );
        let live = hashes(&f.net);
        for (i, name) in REPLACE_ORDER.iter().enumerate() {
            let want = if i <= k as usize { &before } else { &after };
            assert_eq!(live[*name], want[*name], "stop {k}: {name}");
        }
        let info = run(&f, later(120)).unwrap().unwrap();
        assert!(info.resumed && !info.kept_live && info.during_dirs.is_empty());
        assert_eq!(hashes(&f.net), before);
    }
}

#[test]
fn enospc_before_journal_shows_no_space_not_incomplete() {
    for skip in 0..8 {
        let (f, _, pre, _) = requested();
        let err = run_with_fault(&f, "restore.copy", skip, later(60));
        assert!(
            matches!(err, StoreUpgradeError::RestoreNoSpace { needed: Some(n) } if n > 0),
            "copy {skip}: {err:?}"
        );
        assert!(err.file_unchanged());
        assert_eq!(
            copy::screen_for(&err, BackupState::Verified),
            Some(copy::restore_no_space())
        );
        assert_eq!(tree(&f.root), pre, "copy {skip}");
        assert!(run(&f, later(120)).unwrap().is_none(), "request cleared");
    }
}

#[test]
fn enospc_at_the_journal_removes_this_attempts_mark() {
    let (f, _, pre, _) = requested();
    let err = run_with_fault(&f, "restore.journal", 0, later(60));
    assert!(
        matches!(err, StoreUpgradeError::RestoreNoSpace { .. }),
        "{err:?}"
    );
    assert!(!r::restored_mark_present(&f.net));
    assert_eq!(tree(&f.root), pre);
}

#[test]
fn enospc_cleanup_removes_only_this_attempts_copies() {
    let (f, s) = upgraded();
    let unrelated = backups(&f).join("after-upgrade-20000101T000000Z");
    crate::fsx::create_private_dir(&unrelated).unwrap();
    fs::write(unrelated.join("app.sqlite"), b"an older restore's copy").unwrap();
    let pre = tree(&f.root);
    r::request_restore(&f.net, &s).unwrap();
    let err = run_with_fault(&f, "restore.copy", 5, later(60));
    assert!(
        matches!(err, StoreUpgradeError::RestoreNoSpace { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), pre);
}

#[test]
fn enospc_cleanup_failure_leaves_a_safe_to_delete_manifest() {
    let (f, s, _, after) = requested();
    r::CLEANUP_FAILS.with(|c| c.set(true));
    let err = run_with_fault(&f, "restore.copy", 2, later(60));
    r::CLEANUP_FAILS.with(|c| c.set(false));
    assert!(
        matches!(err, StoreUpgradeError::RestoreNoSpace { .. }),
        "{err:?}"
    );
    assert_eq!(hashes(&f.net), after);
    assert!(!present(&f, r::RESTORE_ATTEMPT) && !present(&f, r::RESTORE_REQUESTED));
    let [left] = &dirs(&f, "after-upgrade")[..] else {
        panic!("one partial dir")
    };
    let manifest = fs::read_to_string(left.join(r::SAFE_TO_DELETE)).unwrap();
    assert!(
        manifest.starts_with("incomplete, safe to delete\n"),
        "{manifest}"
    );
    assert!(manifest.contains("file app.sqlite\n") && manifest.contains("file dispatch.sqlite\n"));
    assert!(manifest.contains("cause: "));
    let left_tree = tree(left);
    r::request_restore(&f.net, &s).unwrap();
    let info = run(&f, later(120)).unwrap().unwrap();
    assert_ne!(&info.after_dir, left);
    assert_eq!(tree(left), left_tree, "a later run never removes it");
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn restore_attempt_record_outside_upgrade_backups_is_refused() {
    let (f, s, _, _) = requested();
    let b = backups(&f);
    let outside = f.root.join("outside");
    crate::fsx::create_private_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), b"keep").unwrap();
    let snapshot_name = s.dir().file_name().unwrap().to_string_lossy().into_owned();
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut entries = vec![
        "..".to_string(),
        outside.display().to_string(),
        snapshot_name.clone(),
        r::RESTORE_JOURNAL.to_string(),
        "after-upgrade-20000101T000000Z/../../../outside".to_string(),
        "restore-staging-bogus".to_string(),
        "during-restore-20000101T000000Z".to_string(),
    ];
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, b.join("after-upgrade-20000102T000000Z")).unwrap();
        entries.push("after-upgrade-20000102T000000Z".into());
    }
    let during = b.join("during-restore-20000101T000000Z");
    crate::fsx::create_private_dir(&during).unwrap();
    fs::write(during.join("app.sqlite"), b"kept mid-restore").unwrap();
    let mut text = "dw-store-upgrade restore-attempt\nformat 1\n".to_string();
    for e in &entries {
        text += &format!("dir {e}\n");
    }
    fs::write(b.join(r::RESTORE_ATTEMPT), text).unwrap();

    run(&f, later(60)).unwrap().unwrap();
    assert_eq!(fs::read(outside.join("keep.txt")).unwrap(), b"keep");
    assert!(crate::snapshot::read_snapshot(s.dir(), true).is_ok());
    assert!(during.join("app.sqlite").exists());
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(b.join("after-upgrade-20000102T000000Z"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!present(&f, r::RESTORE_ATTEMPT));
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn crash_after_record_before_create_cleans_cleanly() {
    let (f, s, _, _) = requested();
    let b = backups(&f);
    let partial = b.join("restore-staging-20000101T000000Z");
    crate::fsx::create_private_dir(&partial).unwrap();
    fs::write(partial.join("app.sqlite.partial"), b"half a copy").unwrap();
    fs::write(
        b.join(r::RESTORE_ATTEMPT),
        "dw-store-upgrade restore-attempt\nformat 1\n\
         dir after-upgrade-20000101T000000Z\ndir restore-staging-20000101T000000Z\n",
    )
    .unwrap();
    run(&f, later(60)).unwrap().unwrap();
    assert!(!partial.exists());
    assert!(!present(&f, r::RESTORE_ATTEMPT));
    assert_eq!(dirs(&f, "after-upgrade").len(), 1);
    assert_eq!(hashes(&f.net), before_set(&s));
}

#[test]
fn resume_crash_between_move_and_rename_leaves_a_live_file() {
    let (f, s, _, after) = requested();
    let err = run_with_fault(&f, "restore.rename", 0, later(60));
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::PermissionDenied,
                journal_written: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(hashes(&f.net), after);
    let modified = old_build_writes(&f.net);

    let err = run_with_fault(&f, "restore.rename", 0, later(120));
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreIncomplete {
                journal_written: true,
                ..
            }
        ),
        "{err:?}"
    );
    let app = f.net.join("app.sqlite");
    assert_eq!(
        sha256_file(&app).unwrap(),
        modified,
        "the live file is still there"
    );
    assert!(!side(&app, "-wal").exists() && !side(&app, "-shm").exists());
    let [first] = &dirs(&f, "during-restore")[..] else {
        panic!("one during-restore dir")
    };
    assert_eq!(sha256_file(&first.join("app.sqlite")).unwrap(), modified);
    assert!(first.join("app.sqlite-wal").exists());
    let kept = fs::read_to_string(first.join(r::KEPT)).unwrap();
    assert!(
        kept.contains("app.sqlite: written by another build"),
        "{kept}"
    );

    let info = run(&f, later(180)).unwrap().unwrap();
    let during = dirs(&f, "during-restore");
    assert_eq!(
        during.len(),
        2,
        "a second interrupted resume uses a new dir"
    );
    assert_eq!(info.during_dirs, vec![during[1].clone()]);
    assert_eq!(&during[0], first);
    assert_eq!(hashes(&f.net), before_set(&s));
    assert!(sides_left(&f.net).is_empty());
    assert!(r::restored_mark_present(&f.net));
}

#[test]
fn leftover_old_build_wal_is_moved_aside_never_replayed() {
    let (f, s, _, _) = requested();
    run_with_fault(&f, "restore.rename", 0, later(60));
    let app = f.net.join("app.sqlite");
    leave_hot_wal(
        &app,
        "CREATE TABLE dwsu_old_build (a INTEGER); INSERT INTO dwsu_old_build VALUES (7)",
    );
    let wal = fs::read(side(&app, "-wal")).unwrap();
    r::SKIP_SIDE_UNLINK.with(|x| x.set(fail_first("skip-side-unlink")));
    let found = run(&f, later(120));
    r::SKIP_SIDE_UNLINK.with(|x| x.set(false));
    let info = found.unwrap().unwrap();
    assert_eq!(hashes(&f.net), before_set(&s));
    let conn = Connection::open(&app).unwrap();
    assert!(
        !crate::sql::has_table(&conn, "dwsu_old_build").unwrap(),
        "the old log was replayed"
    );
    assert!(crate::sql::integrity_ok(&conn).unwrap());
    drop(conn);
    let [during] = &info.during_dirs[..] else {
        panic!("one during-restore dir")
    };
    assert_eq!(fs::read(during.join("app.sqlite-wal")).unwrap(), wal);
}

#[test]
fn resume_after_old_build_touched_folder_mid_restore() {
    for point in ["restore.replaced", "restore.rename"] {
        for k in 0..4u32 {
            if point == "restore.replaced" && k == 3 {
                continue;
            }
            let (f, s, _, after) = requested();
            run_with_fault(&f, point, k, later(60));
            let modified = old_build_writes(&f.net);
            let info = run(&f, later(120))
                .unwrap_or_else(|e| panic!("{point} {k}: {e:?}"))
                .unwrap();
            let at = format!("{point} {k}");
            assert!(info.resumed && !info.kept_live, "{at}");
            assert_eq!(hashes(&info.after_dir), after, "{at}");
            assert_eq!(hashes(&f.net), before_set(&s), "{at}");
            assert!(sides_left(&f.net).is_empty(), "{at}");
            let [during] = &info.during_dirs[..] else {
                panic!("{at}: one during-restore dir")
            };
            assert_eq!(
                sha256_file(&during.join("app.sqlite")).unwrap(),
                modified,
                "{at}"
            );
            for n in STORE_FILES.iter().filter(|n| **n != "wallet.sqlite") {
                assert!(during.join(format!("{n}-wal")).exists(), "{at}: {n}-wal");
            }
            assert!(r::restored_mark_present(&f.net), "{at}");
            assert!(!present(&f, r::RESTORE_JOURNAL), "{at}");
        }
    }
}

/// S4: every store replaced (a crash right after wallet.sqlite's rename),
/// then an old build writes a payment held only in wallet.sqlite's log.
fn at_s4_with_old_build_payment() -> (Fixture, SnapshotInfo) {
    let (f, s, _, _) = requested();
    run_with_fault(&f, "restore.replaced", 3, later(60));
    assert_eq!(hashes(&f.net), before_set(&s));
    assert!(present(&f, r::RESTORE_JOURNAL));
    leave_hot_wal(&f.wallet(), &pending_sql());
    (f, s)
}

#[test]
fn s4_keeps_live_and_side_files_by_default() {
    assert_eq!(r::S4_POLICY, S4Policy::KeepLive);
    let (f, s) = at_s4_with_old_build_payment();
    let live = tree(&f.net);
    r::MOVE_SIDES_AT_S4.with(|x| x.set(fail_first("move-sides-at-s4")));
    let found = run(&f, later(120));
    r::MOVE_SIDES_AT_S4.with(|x| x.set(false));
    let info = found.unwrap().unwrap();
    assert!(info.kept_live && info.resumed);
    assert_pending_is_the_one_payment(&f, &s.dir().join("wallet.sqlite"));
    for n in ["wallet.sqlite", "wallet.sqlite-wal", "wallet.sqlite-shm"] {
        assert_eq!(
            live.get(n),
            tree(&f.net).get(n),
            "{n} kept in place, unopened"
        );
    }
    assert!(!present(&f, r::RESTORE_JOURNAL) && !present(&f, r::RESTORE_REQUESTED));
    assert!(r::restored_mark_present(&f.net));
    let kept = fs::read_to_string(info.after_dir.join(r::KEPT)).unwrap();
    assert!(kept.contains("kept the live files"), "{kept}");
    let visible: i64 = Connection::open(f.wallet())
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM core_transactions WHERE height IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(visible, 1, "the payment held in the log is visible");
}

#[test]
fn s4_lineage_by_schema_and_wallet_ids_keeps_live() {
    let (f, s, _, _) = requested();
    run_with_fault(&f, "restore.replaced", 3, later(60));
    exec(&f.wallet(), &pending_sql());
    assert_ne!(hashes(&f.net), before_set(&s));
    let info = run(&f, later(120)).unwrap().unwrap();
    assert!(info.kept_live);
    assert_pending_is_the_one_payment(&f, &f.wallet());
}

#[test]
fn s4_refuses_when_switched_and_touches_nothing() {
    let (f, _) = at_s4_with_old_build_payment();
    let want = tree(&f.root);
    S4_FOR_TEST.with(|p| p.set(Some(S4Policy::Refuse)));
    let found = run(&f, later(120));
    S4_FOR_TEST.with(|p| p.set(None));
    let err = found.unwrap_err();
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::S4OldBuildWrote,
                journal_written: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), want);
    assert_eq!(
        copy::screen_for(&err, BackupState::Verified),
        Some(copy::restore_keep_current())
    );
    let info = r::keep_current_files(&f.net, later(180)).unwrap();
    assert!(info.kept_live);
    assert!(!present(&f, r::RESTORE_JOURNAL));
}

#[test]
fn s4_other_lineage_refuses_with_keep_action() {
    let (f, _, _, _) = requested();
    run_with_fault(&f, "restore.replaced", 3, later(60));
    let (_fresh_dir, fresh) = fresh_store();
    fs::copy(&fresh, f.wallet()).unwrap();
    let want = tree(&f.root);
    let err = run(&f, later(120)).unwrap_err();
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::S4OtherLineage,
                journal_written: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), want, "nothing moved");
    let screen = copy::screen_for(&err, BackupState::Verified).unwrap();
    assert_eq!(screen.title_key, "storeUpgrade.restore.keepCurrent.title");
    assert_eq!(
        screen.buttons[0].key,
        "storeUpgrade.restore.keepCurrent.keep"
    );
    assert!(
        run(&f, later(150)).is_err(),
        "refused again until the user acts"
    );

    let wallet = sha256_file(&f.wallet()).unwrap();
    let info = r::keep_current_files(&f.net, later(180)).unwrap();
    assert!(info.kept_live);
    assert_eq!(sha256_file(&f.wallet()).unwrap(), wallet);
    assert!(!present(&f, r::RESTORE_JOURNAL) && !present(&f, r::RESTORE_REQUESTED));
    assert!(r::restored_mark_present(&f.net), "the mark stays");
    let kept = fs::read_to_string(info.after_dir.join(r::KEPT)).unwrap();
    assert_eq!(kept.lines().count(), 1, "{kept}");
    assert!(run(&f, later(200)).unwrap().is_none());
}

#[test]
fn keep_current_files_refuses_before_the_wallet_is_put_back() {
    let (f, _, _, _) = requested();
    run_with_fault(&f, "restore.replaced", 1, later(60));
    let want = tree(&f.root);
    let err = keep_current_err(&f);
    assert!(
        matches!(err, StoreUpgradeError::RestoreIncomplete { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), want);
}

#[test]
fn restored_mark_lists_before_set_pending_payments() {
    let f = fixture("v0.1.0");
    settle_stores(&f.net);
    exec(&f.wallet(), &pending_sql());
    settle_stores(&f.net);
    let s = take_snapshot(&f.net, now()).unwrap();
    exec(
        &f.wallet(),
        &format!(
            "DELETE FROM core_transactions WHERE txid = x'{}'",
            "AA".repeat(32)
        ),
    );
    settle_stores(&f.net);
    r::request_restore(&f.net, &s).unwrap();
    run(&f, later(60)).unwrap().unwrap();
    assert_pending_is_the_one_payment(&f, &f.wallet());
    let text = fs::read_to_string(backups(&f).join(r::RESTORED_MARK)).unwrap();
    assert!(text.starts_with("dw-store-upgrade restored\nformat 1\nrestored "));

    fs::write(
        backups(&f).join(r::RESTORED_MARK),
        format!("{text}pending zz 00\n"),
    )
    .unwrap();
    assert!(r::restored_mark(&f.net).is_err(), "never silently short");
}

#[test]
fn snapshot_state_reports_a_corrupt_snapshot_as_unverified() {
    let f = fixture("v0.1.0");
    assert_eq!(snapshot_state(&f.net).unwrap(), SnapshotState::None);
    let s = take_snapshot(&f.net, now()).unwrap();
    assert_eq!(
        snapshot_state(&f.net).unwrap(),
        SnapshotState::Verified(s.clone())
    );
    let app = s.dir().join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[100] ^= 1;
    fs::write(&app, bytes).unwrap();
    let state = snapshot_state(&f.net).unwrap();
    assert!(
        matches!(&state, SnapshotState::Unverified { dir, .. } if dir == s.dir()),
        "{state:?}"
    );
    assert!(!crate::snapshot_present(&f.net).unwrap());
    let backup = BackupState::from(&state);
    assert_eq!(backup, BackupState::Unverified);
    let screen = copy::cant_open(CantOpenCause::P9aBroken, backup);
    assert_eq!(screen.text_key, "storeUpgrade.cantOpen.textWithoutBackup");
    assert!(screen.backup_unverified);
    let err = StoreUpgradeError::P9aBroken { detail: "x".into() };
    assert_eq!(copy::screen_for(&err, backup), Some(screen));
    assert!(!copy::cant_open(CantOpenCause::P9aBroken, BackupState::None).backup_unverified);
}

#[test]
fn restore_confirm_copy_is_verbatim_and_cancel_has_default_focus() {
    let c = copy::restore_confirm();
    assert_eq!(c.title_key, "storeUpgrade.restore.confirm.title");
    assert_eq!(c.title, "Restore wallet data from before the upgrade?");
    assert!(c.text.starts_with(
        "Dash Wallet restarts and puts back the wallet data it saved before the upgrade, so the wallet won't show anything recorded since then. "
    ));
    assert!(c.text.contains(&format!(
        "kept in the {UPGRADE_BACKUPS_DIR} folder inside {{folder}}. Don't delete it"
    )));
    assert!(c.text.contains(
        "An earlier version may offer coins this version is holding back, and a payment that uses them can fail.\n\n"
    ));
    assert!(c.text.ends_with(
        "\n\nIf Dash Wallet closes before the restore finishes, start this version again before you open an earlier one."
    ));
    let [restore, cancel] = &c.buttons[..] else {
        panic!("two buttons")
    };
    assert_eq!(
        (restore.label, restore.destructive, restore.default_focus),
        ("Restart and restore", true, false)
    );
    assert_eq!(
        (cancel.label, cancel.destructive, cancel.default_focus),
        ("Cancel", false, true)
    );
    assert!(
        c.render(FOLDER)
            .contains(&format!("folder inside {FOLDER}."))
    );
}

#[test]
fn restore_refusal_copy_renders_for_each_cause() {
    for cause in [
        RefusalCause::MissingBackup,
        RefusalCause::MismatchedBackup,
        RefusalCause::GarbledMarker,
    ] {
        let err = StoreUpgradeError::RestoreRefused { cause };
        let shown = copy::screen_for(&err, BackupState::None)
            .unwrap()
            .render(FOLDER);
        assert!(shown.starts_with("The earlier wallet data couldn't be restored\n\n"));
        assert!(shown.contains("so nothing was changed."));
        assert!(shown.ends_with("[OK]"));
    }
}

#[test]
fn restore_incomplete_copy_renders_for_each_cause() {
    for cause in [
        IncompleteCause::DiskFull,
        IncompleteCause::PermissionDenied,
        IncompleteCause::FileVanished,
        IncompleteCause::Other,
    ] {
        let err = StoreUpgradeError::RestoreIncomplete {
            cause,
            journal_written: true,
            detail: String::new(),
        };
        let screen = copy::screen_for(&err, BackupState::Verified).unwrap();
        assert_eq!(screen.title_key, "storeUpgrade.restore.incomplete.title");
        assert_eq!(screen.hint.unwrap().0, "settings.links.issuesHint");
        let shown = screen.render(FOLDER);
        assert!(shown.contains(&format!("hasn't opened the wallet data in {FOLDER}.")));
        assert!(shown.contains("don't delete anything there"));
        assert!(shown.contains("[Restart][Report a problem][Quit]"));
        assert!(!err.file_unchanged());

        let err = StoreUpgradeError::RestoreIncomplete {
            cause,
            journal_written: false,
            detail: String::new(),
        };
        let screen = copy::screen_for(&err, BackupState::Verified).unwrap();
        assert_eq!(screen.title_key, "storeUpgrade.restore.notStarted.title");
        assert!(err.file_unchanged());
    }
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::StorageFull)),
        IncompleteCause::DiskFull
    );
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        IncompleteCause::PermissionDenied
    );
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::NotFound)),
        IncompleteCause::FileVanished
    );
}

#[test]
fn restore_no_space_copy_renders_with_and_without_size() {
    for needed in [Some(123_456), None] {
        let err = StoreUpgradeError::RestoreNoSpace { needed };
        let shown = copy::screen_for(&err, BackupState::Verified)
            .unwrap()
            .render(FOLDER);
        assert!(shown.starts_with("Not enough disk space to restore\n\n"));
        assert!(shown.contains(&format!("the disk that holds {FOLDER}, then")));
        assert!(shown.ends_with("[OK]"));
    }
}

#[test]
fn no_screen_says_funds_are_held_on_the_network() {
    let source = include_str!("../copy.rs");
    assert!(!source.contains("held on the network"));
    for s in copy::all_screens() {
        let all = [s.title, s.text]
            .into_iter()
            .chain(s.buttons.iter().map(|b| b.label))
            .chain(s.hint.map(|h| h.1));
        for text in all {
            assert!(!text.contains("held on the network"), "{text}");
            assert!(!text.contains("Your funds"), "{text}");
        }
    }
}

#[test]
fn placeholders_are_only_the_keys_mg_has_not_written() {
    let mut keys: Vec<&str> = copy::all_screens()
        .iter()
        .filter(|s| s.title.starts_with(copy::PLACEHOLDER))
        .map(|s| s.title_key)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "storeUpgrade.restore.keepCurrent.title",
            "storeUpgrade.restore.notStarted.title",
            "storeUpgrade.snapshotMissing.title"
        ]
    );
}

#[test]
fn touched_paths_name_every_marker_and_dir() {
    let all = r::touched_paths().join("\n");
    for name in [
        r::RESTORE_REQUESTED,
        r::RESTORE_JOURNAL,
        r::RESTORED_MARK,
        r::RESTORE_ATTEMPT,
        r::SAFE_TO_DELETE,
        r::KEPT,
        "after-upgrade-<UTC>",
        "restore-staging-<UTC>",
        "during-restore-<UTC>",
        "before-upgrade-<UTC>",
    ] {
        assert!(all.contains(name), "{name}");
    }
}

#[test]
#[ignore = "timing on a loaded box is noise; run alone"]
fn measure_rename_window() {
    let mut taken = Vec::new();
    for _ in 0..20 {
        let (f, _, _, _) = requested();
        run(&f, later(60)).unwrap().unwrap();
        taken.push(r::LAST_REPLACE.with(|t| t.get()).unwrap());
    }
    taken.sort();
    eprintln!(
        "rename window: median {:?}, min {:?}, max {:?}",
        taken[10], taken[0], taken[19]
    );
}
