//! Q107: a start that finds the store upgraded never refuses for a snapshot
//! reason (107-1), offers no way out that needs the earlier data (107-2),
//! and never retires a snapshot while a restore owns `upgrade-backups/`.

use super::review_fixes::column_present;
use super::*;
use crate::copy::{ProvedState, RefusedCause, WayOut};
use crate::error::StoreUpgradeError;
use crate::reconcile::{Outcome, UpgradeNotice, reconcile_fork_store};
use crate::restore::{RESTORE_JOURNAL, RESTORE_REQUESTED, RefusalCause};
use crate::snapshot::{
    NOT_THIS_WALLET, SnapshotInfo, SnapshotState, UPGRADE_BACKUPS_DIR, snapshot_state,
};

pub(super) fn refinery_max(f: &Fixture) -> i64 {
    crate::sql::open_immutable(&f.wallet())
        .unwrap()
        .query_row(
            "SELECT MAX(version) FROM refinery_schema_history",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

fn named(f: &Fixture, prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(f.net.join(UPGRADE_BACKUPS_DIR))
        .map(|it| {
            it.map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

/// A new process: no store opened in it yet.
pub(super) fn start(f: &Fixture, at: SystemTime) -> crate::Result<crate::Prepared> {
    crate::token::reset_opened_for_test();
    crate::prepare_with(&f.net, build(), at)
}

/// Upstream's backup step can't write: `backups/` is a file.
fn block_backups(f: &Fixture) {
    let backups = f.net.join("backups");
    if backups.is_dir() {
        fs::remove_dir_all(&backups).unwrap();
    }
    fs::write(&backups, b"").unwrap();
}

fn unblock_backups(f: &Fixture) {
    fs::remove_file(f.net.join("backups")).unwrap();
}

/// R1 commits, then the upstream open fails in its backup step: the
/// between state, with the snapshot this start took.
pub(super) fn failed_open() -> (Fixture, SnapshotInfo) {
    let f = fixture("v0.1.0");
    let snapshot = start(&f, now()).unwrap().snapshot.unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    block_backups(&f);
    let err = PersisterOpened::open(config(&f.net))
        .err()
        .expect("the backup step fails");
    unblock_backups(&f);
    let retry = crate::copy::retry_copy_for(&err).expect("retried, with its screen");
    super::copy_table::assert_retry_copy(&retry);
    assert!(!column_present(&f));
    assert!(refinery_max(&f) <= crate::fork::FORK_REFINERY_MAX);
    (f, snapshot)
}

/// 107-2: with the file part-upgraded or migrated, no refusal offers the
/// earlier version or a restore.
fn assert_no_way_back(f: &Fixture, at: &str) {
    for cause in RefusedCause::ALL {
        for wrote in [false, true] {
            let way = WayOut::for_cause(cause, ProvedState::read(&f.net, wrote));
            assert!(
                !matches!(way, WayOut::Previous | WayOut::Restore),
                "{at}: {cause:?} offers {way:?}"
            );
        }
    }
}

/// The start after the failed open: no new before-upgrade snapshot, the
/// upstream open and load, and `notice` in the outcome. Without the true
/// snapshot (`notice` or not `kept`), no way back before or after.
pub(super) fn start_opens(f: &Fixture, notice: Option<UpgradeNotice>, kept: bool) -> SnapshotInfo {
    if !kept {
        assert_no_way_back(f, "between");
    }
    let befores = named(f, "before-upgrade-");
    let snapshot = start(f, later(60)).unwrap().snapshot.unwrap();
    let outcome = reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    assert!(matches!(outcome, Outcome::Pending { .. }), "{outcome:?}");
    assert_eq!(outcome.notice(), notice);
    drop(open_and_load(&f.net));
    assert_eq!(named(f, "before-upgrade-"), befores, "no new snapshot");
    assert!(refinery_max(f) > crate::fork::FORK_REFINERY_MAX);
    if !kept {
        assert_no_way_back(f, "upgraded");
    }
    snapshot
}

/// T5: the next start is a plain upgraded open.
pub(super) fn assert_plain_reopen(f: &Fixture) {
    let before = tree(&f.net.join(UPGRADE_BACKUPS_DIR));
    let snapshot = start(f, later(120)).unwrap().snapshot.unwrap();
    let outcome = reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    assert!(matches!(outcome, Outcome::Retired { .. }), "{outcome:?}");
    assert_eq!(outcome.notice(), None);
    drop(open_and_load(&f.net));
    assert!(refinery_max(f) > crate::fork::FORK_REFINERY_MAX);
    assert_eq!(
        tree(&f.net.join(UPGRADE_BACKUPS_DIR)),
        before,
        "no new snapshot"
    );
}

fn assert_restore_refused(f: &Fixture, snapshot: &SnapshotInfo) {
    let err = crate::restore::request_restore(&f.net, snapshot).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotInvalid { .. }),
        "{err:?}"
    );
    assert!(
        !f.net
            .join(UPGRADE_BACKUPS_DIR)
            .join(RESTORE_REQUESTED)
            .exists()
    );
}

#[test]
fn t1_an_absent_snapshot_opens_without_a_notice_or_a_restore() {
    let (f, s) = failed_open();
    crate::restore::request_restore(&f.net, &s).unwrap();
    let backups = f.net.join(UPGRADE_BACKUPS_DIR);
    let request = fs::read(backups.join(RESTORE_REQUESTED)).unwrap();
    fs::remove_dir_all(&backups).unwrap();
    start_opens(&f, None, false);
    assert_eq!(snapshot_state(&f.net).unwrap(), SnapshotState::None);
    assert!(
        fs::read_dir(f.net.join("backups").join("auto"))
            .unwrap()
            .next()
            .is_some(),
        "upstream's own backup"
    );
    fs::write(backups.join(RESTORE_REQUESTED), request).unwrap();
    let err = start(&f, later(90)).unwrap_err();
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreRefused {
                cause: RefusalCause::MissingBackup
            }
        ),
        "{err:?}"
    );
    assert_plain_reopen(&f);
}

#[test]
fn t2_an_unverified_snapshot_opens_with_the_damaged_notice_once() {
    let (f, s) = failed_open();
    let app = s.dir().join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[100] ^= 1;
    fs::write(&app, bytes).unwrap();
    start_opens(&f, Some(UpgradeNotice::SnapshotDamaged), false);
    assert!(matches!(
        snapshot_state(&f.net).unwrap(),
        SnapshotState::Unverified { .. }
    ));
    assert!(!crate::snapshot::snapshot_present(&f.net).unwrap());
    assert!(!s.dir().join(NOT_THIS_WALLET).exists());
    assert_restore_refused(&f, &s);
    assert_plain_reopen(&f);
}

#[test]
fn t3_a_snapshot_of_other_data_is_retired_and_opens_with_the_other_notice_once() {
    let (f, s) = failed_open();
    exec(
        &f.wallet(),
        "UPDATE core_utxos SET value = value + 1 WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)",
    );
    assert!(crate::snapshot::read_snapshot(s.dir(), true).is_ok());
    start_opens(&f, Some(UpgradeNotice::SnapshotOther), false);
    assert!(s.dir().join(NOT_THIS_WALLET).is_file(), "retired");
    assert!(s.dir().join("wallet.sqlite").is_file(), "never deleted");
    assert!(crate::snapshot::read_snapshot(s.dir(), true).is_err());
    assert!(!matches!(
        snapshot_state(&f.net).unwrap(),
        SnapshotState::Verified(_)
    ));
    assert_restore_refused(&f, &s);
    assert_plain_reopen(&f);
}

#[test]
fn t4_the_true_snapshot_is_reused_and_restores() {
    let (f, s) = failed_open();
    let reused = start_opens(&f, None, true);
    assert_eq!(reused.dir(), s.dir());
    assert_plain_reopen(&f);
    assert_eq!(
        ProvedState::read(&f.net, false).backup,
        Some(crate::copy::BackupState::Verified),
        "still this wallet's backup once the fork history is retired"
    );
    let SnapshotState::Verified(newest) = snapshot_state(&f.net).unwrap() else {
        panic!("the true snapshot stays verified");
    };
    assert_eq!(newest.manifest_sha256(), s.manifest_sha256());
    crate::restore::request_restore(&f.net, &newest).unwrap();
    let restored = start(&f, later(300)).unwrap().restored;
    assert!(restored.is_some());
    assert!(column_present(&f));
    assert!(refinery_max(&f) <= crate::fork::FORK_REFINERY_MAX);
    let state = ProvedState::read(&f.net, true);
    assert_eq!((state.migrated, state.unchanged), (Some(false), None));
    assert!(crate::restore::restored_mark_present(&f.net));
    assert_eq!(
        WayOut::for_cause(RefusedCause::ForkUnsupported, state),
        WayOut::Previous,
        "the way out goes by the file, not the restored mark"
    );
}

#[test]
fn failing_opens_never_stack_upgrade_copies() {
    let (f, s) = failed_open();
    fs::remove_dir_all(s.dir()).unwrap();
    for at in [30, 60] {
        let part = start(&f, later(at)).unwrap().snapshot.unwrap();
        reconcile_fork_store(&config(&f.net), &part, build()).unwrap();
        block_backups(&f);
        assert!(PersisterOpened::open(config(&f.net)).is_err());
        unblock_backups(&f);
    }
    assert_eq!(named(&f, "part-upgraded-").len(), 1);
    start_opens(&f, None, false);
    assert_eq!(named(&f, "part-upgraded-").len(), 1);
    assert_plain_reopen(&f);
}

#[test]
fn a_restore_journal_keeps_a_snapshot_of_other_data_unmarked() {
    let (f, s) = failed_open();
    exec(
        &f.wallet(),
        "UPDATE core_utxos SET value = value + 1 WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)",
    );
    let journal = f.net.join(UPGRADE_BACKUPS_DIR).join(RESTORE_JOURNAL);
    fs::write(&journal, b"not a journal\n").unwrap();
    let before = tree(s.dir());

    let err = crate::snapshot::take_snapshot(&f.net, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::RestorePending { .. }),
        "{err:?}"
    );
    assert_eq!(tree(s.dir()), before);

    crate::token::reset_opened_for_test();
    let ordered = format!(
        "{:?}",
        crate::restore::run_pending_restore(&f.net, later(60)).unwrap_err()
    );
    let started = start(&f, later(60)).unwrap_err();
    assert_eq!(format!("{started:?}"), ordered, "the restore goes first");
    assert!(!s.dir().join(NOT_THIS_WALLET).exists());
    assert_eq!(tree(s.dir()), before);
    assert!(journal.exists());
}

#[test]
fn a_restore_request_keeps_a_snapshot_of_other_data_unmarked() {
    let (f, s) = failed_open();
    crate::restore::request_restore(&f.net, &s).unwrap();
    exec(
        &f.wallet(),
        "UPDATE core_utxos SET value = value + 1 WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)",
    );
    let before = tree(s.dir());
    let err = crate::snapshot::take_snapshot(&f.net, later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::RestorePending { .. }),
        "{err:?}"
    );
    assert!(!s.dir().join(NOT_THIS_WALLET).exists());
    assert_eq!(tree(s.dir()), before);
}

/// A fault reading the snapshot under R1's lock, once upgraded, is retried
/// with its screen: never a refusal, never the snapshot called invalid.
#[test]
fn a_fault_rereading_the_snapshot_under_the_lock_is_retried() {
    let (f, _) = failed_open();
    let part = start(&f, later(60)).unwrap().snapshot.unwrap();
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| {
        *h.borrow_mut() = Some((part.dir().to_path_buf(), std::io::ErrorKind::Interrupted))
    });
    let err = reconcile_fork_store(&config(&f.net), &part, build()).unwrap_err();
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| *h.borrow_mut() = None);
    assert!(
        matches!(
            err,
            StoreUpgradeError::Io {
                unchanged: true,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!err.is_refusal() && RefusedCause::of(&err).is_none());
    assert!(crate::copy::retry_copy_for(&err).is_some());
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &part, build()).unwrap(),
        Outcome::Pending { .. }
    ));
}
