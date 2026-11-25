//! The re-review of the review fixes: the rehearsal runs with the caller's
//! config (1), a start that stopped after R1's drop keeps the true snapshot
//! (3), a fault of the machine in the rehearsal is retried, not refused (5),
//! and the locked check rehearses a change held only in the log (13).

use super::review_fixes::{column_present, hot_wal};
use super::*;
use crate::error::StoreUpgradeError;
use crate::precheck::{BLOCK_REHEARSAL_BACKUP, Precheck, precheck_locked_with, precheck_with};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::sql::{has_column, open_immutable, sha256_file};

fn checked(f: &Fixture) -> crate::error::Result<Precheck> {
    match precheck_with(&f.net, build())? {
        Precheck::NeedsLockedCheck => precheck_locked_with(&f.net, build()),
        other => Ok(other),
    }
}

fn before_upgrade_dirs(f: &Fixture) -> usize {
    fs::read_dir(f.net.join(crate::snapshot::UPGRADE_BACKUPS_DIR))
        .map(|it| {
            it.filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("before-upgrade-")
            })
            .count()
        })
        .unwrap_or(0)
}

#[test]
fn reconcile_refuses_before_the_drop_when_the_backup_dir_cannot_be_written() {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    fs::write(f.root.join("not-a-dir"), b"").unwrap();
    let unwritable =
        config(&f.net).with_auto_backup_dir(Some(f.root.join("not-a-dir").join("auto")));
    let wallet = sha256_file(&f.wallet()).unwrap();
    let err = reconcile_fork_store(&unwritable, &snapshot, build()).unwrap_err();
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
    assert!(!err.is_refusal());
    assert!(
        column_present(&f),
        "nothing dropped for an open that would fail"
    );
    assert_eq!(sha256_file(&f.wallet()).unwrap(), wallet);

    let mut no_backups = config(&f.net);
    no_backups.auto_backup_dir = None;
    let err = reconcile_fork_store(&no_backups, &snapshot, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::OpenConfigUnsupported { .. }),
        "{err:?}"
    );
    assert!(column_present(&f));

    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    drop(open_and_load(&f.net));
}

#[test]
fn open_refuses_a_config_other_than_the_reconciled_one() {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    let mut other = config(&f.net);
    other.busy_timeout = Duration::from_millis(1234);
    let err = PersisterOpened::open(other).err().expect("refused");
    assert!(
        matches!(err, StoreUpgradeError::OpenConfigUnsupported { .. }),
        "{err:?}"
    );
    let mut no_backups = config(&f.net);
    no_backups.auto_backup_dir = None;
    let err = PersisterOpened::open(no_backups).err().expect("refused");
    assert!(
        matches!(err, StoreUpgradeError::OpenConfigUnsupported { .. }),
        "{err:?}"
    );
    assert!(!crate::token::is_held(&f.net));
    drop(open_and_load(&f.net));
}

#[test]
fn a_start_that_stopped_after_the_drop_keeps_the_true_snapshot() {
    let f = fixture("v0.1.0");
    let first = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &first, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    assert!(!column_present(&f));
    exec(
        &f.net.join("app.sqlite"),
        "CREATE TABLE dwsu_after_drop (a INTEGER); INSERT INTO dwsu_after_drop VALUES (1)",
    );
    let again = crate::prepare_with(&f.net, build(), later(60))
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(again.dir(), first.dir(), "no snapshot of the dropped file");
    assert_eq!(before_upgrade_dirs(&f), 1);
    let copy = open_immutable(&first.dir().join("wallet.sqlite")).unwrap();
    assert!(has_column(&copy, "contacts", "external_account_reference").unwrap());
    drop(copy);
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &again, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    drop(open_and_load(&f.net));
}

#[test]
fn a_part_done_upgrade_without_its_snapshot_is_refused_not_resnapshotted() {
    let f = fixture("v0.1.0");
    let first = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    reconcile_fork_store(&config(&f.net), &first, build()).unwrap();
    fs::remove_dir_all(first.dir()).unwrap();
    let before = tree(&f.root);
    let err = crate::prepare_with(&f.net, build(), later(60)).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotInvalid { .. }),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(before_upgrade_dirs(&f), 0);
    assert_eq!(tree(&f.root), before);
}

#[test]
fn a_machine_fault_in_the_rehearsal_is_retryable_not_a_refusal() {
    let f = fixture("v0.1.0");
    let before = tree(&f.root);
    BLOCK_REHEARSAL_BACKUP.with(|b| b.set(true));
    let found = checked(&f);
    BLOCK_REHEARSAL_BACKUP.with(|b| b.set(false));
    let err = found.unwrap_err();
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
    assert!(!err.is_refusal());
    assert!(crate::copy::screen_for(&err, crate::copy::ProvedState::default()).is_none());
    assert_eq!(tree(&f.root), before);
    assert!(matches!(checked(&f).unwrap(), Precheck::Fork { .. }));
}

#[test]
fn the_locked_check_rehearses_a_change_held_only_in_the_log() {
    let f = fixture("v0.1.0");
    exec(&f.wallet(), "PRAGMA wal_checkpoint(TRUNCATE)");
    hot_wal(
        &f,
        "INSERT INTO core_transactions (wallet_id, txid, finalized, record_blob) \
         SELECT wallet_id, randomblob(32), 0, x'00' FROM wallets LIMIT 1",
    );
    let before = tree(&f.root);
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::NeedsLockedCheck
    ));
    let found = precheck_locked_with(&f.net, build()).unwrap();
    assert!(
        matches!(
            &found,
            Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { .. }
            }
        ),
        "{found:?}"
    );
    assert_eq!(tree(&f.root), before);
}
