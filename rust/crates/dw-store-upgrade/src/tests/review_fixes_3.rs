//! The re-review of 2f0bdd20: a part-done upgrade with no snapshot to keep
//! (M1), machine faults inside a migration or load (M2, L8), a snapshot of
//! the dropped-column file (L3), the backup dir checked only when upstream
//! will migrate (L4), and files the rehearsal's copy can't show (L5).

use platform_wallet::changeset::{PersistenceError, PersistenceErrorKind};
use platform_wallet_storage::{Synchronous, WalletStorageError};

use super::review_fixes::column_present;
use super::*;
use crate::copy::{self, BackupState, ProvedState, RefusedCause, WayOut};
use crate::error::StoreUpgradeError;
use crate::precheck::{LOAD_FOR_TEST, Precheck, precheck_locked_with, precheck_with};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::{
    ABSENT_POLICY_FOR_TEST, AbsentSnapshotPolicy, MissingSnapshot, SnapshotInfo,
};
#[cfg(unix)]
use crate::sql::sha256_file;

pub(super) fn checked(f: &Fixture) -> crate::Result<Precheck> {
    match precheck_with(&f.net, build())? {
        Precheck::NeedsLockedCheck => precheck_locked_with(&f.net, build()),
        other => Ok(other),
    }
}

/// R1's drop committed, the persister never opened: the between state.
pub(super) fn between() -> (Fixture, SnapshotInfo) {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    assert!(!column_present(&f));
    (f, snapshot)
}

pub(super) fn assert_missing(f: &Fixture, want: MissingSnapshot) {
    let before = tree(&f.root);
    let err = crate::prepare_with(&f.net, build(), later(60)).unwrap_err();
    assert!(
        matches!(&err, StoreUpgradeError::UpgradeSnapshotMissing { state, .. } if *state == want),
        "{err:?}"
    );
    assert!(err.file_unchanged() && err.is_refusal());
    let claimed = ProvedState {
        migrated: Some(false),
        backup: Some(BackupState::Verified),
        unchanged: Some(true),
    };
    let screen = copy::screen_for(&err, claimed).unwrap();
    assert_eq!(screen.cause, RefusedCause::SnapshotMissing);
    assert_eq!(
        screen.unchanged.key,
        "storeUpgrade.refused.changedByUpgrade"
    );
    let way_out = match want {
        MissingSnapshot::Unverified => WayOut::Unverified,
        MissingSnapshot::Absent | MissingSnapshot::Mismatch => WayOut::SupportMigrated,
    };
    assert_eq!(screen.way_out, way_out, "{want:?}");
    assert_eq!(copy::restore_screen_for(&err), None);
    assert_eq!(tree(&f.root), before, "{want:?}: nothing written");
}

#[test]
fn a_part_done_upgrade_with_no_snapshot_at_all_is_refused_as_absent() {
    let (f, s) = between();
    fs::remove_dir_all(s.dir()).unwrap();
    assert_missing(&f, MissingSnapshot::Absent);
}

#[test]
fn a_part_done_upgrade_whose_snapshot_fails_verification_is_refused_as_unverified() {
    let (f, s) = between();
    let app = s.dir().join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[100] ^= 1;
    fs::write(&app, bytes).unwrap();
    assert_missing(&f, MissingSnapshot::Unverified);
}

#[test]
fn a_part_done_upgrade_whose_snapshot_is_of_other_data_is_refused_as_mismatch() {
    let (f, _) = between();
    exec(
        &f.wallet(),
        "UPDATE core_utxos SET value = value + 1 WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)",
    );
    assert_missing(&f, MissingSnapshot::Mismatch);
}

#[test]
fn a_snapshot_of_the_dropped_column_file_is_never_kept_as_the_backup() {
    let (f, s) = between();
    fs::remove_dir_all(s.dir()).unwrap();
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(Some(AbsentSnapshotPolicy::ProceedUnrestorable)));
    let made = crate::prepare_with(&f.net, build(), later(60));
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(None));
    let dropped = made.unwrap().snapshot.unwrap();
    let posing = f
        .net
        .join(crate::snapshot::UPGRADE_BACKUPS_DIR)
        .join("before-upgrade-20991231T000000Z");
    fs::rename(dropped.dir(), &posing).unwrap();
    assert!(crate::snapshot::read_snapshot(&posing, true).is_ok());
    assert_missing(&f, MissingSnapshot::Mismatch);
}

#[test]
fn a_fault_inside_a_migration_or_integrity_check_is_read_by_its_code() {
    let tmp = private_tempdir();
    let path = tmp.path().join("ro.sqlite");
    drop(Connection::open(&path).unwrap());
    let mut ro =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let migration = refinery::Migration::unapplied("V1__a", "CREATE TABLE a (x INTEGER)").unwrap();
    let err = refinery::Runner::new(&[migration])
        .run(&mut ro)
        .expect_err("read-only");
    let readonly = WalletStorageError::Migration(err);
    assert!(!crate::precheck::storage_fault(&readonly));
    assert!(crate::precheck::write_refused(&readonly));

    let full =
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL), None);
    assert!(crate::precheck::storage_fault(
        &WalletStorageError::IntegrityCheckRunFailed { source: full }
    ));
    assert!(!crate::precheck::storage_fault(
        &WalletStorageError::IntegrityCheckFailed {
            report: "row 1 missing from index".into()
        }
    ));
    assert!(!crate::precheck::storage_fault(
        &WalletStorageError::SchemaHistoryMissing
    ));
}

#[test]
fn an_invalid_open_config_is_a_caller_error_not_a_refusal() {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    let mut bad = config(&f.net);
    bad.synchronous = Synchronous::Off;
    let err = reconcile_fork_store(&bad, &snapshot, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::OpenConfigUnsupported { .. }),
        "{err:?}"
    );
    assert!(!err.is_refusal());
    assert!(column_present(&f));
}

fn data_load_error() -> PersistenceError {
    PersistenceError::backend("wallet 1 could not be rehydrated")
}

fn transient_load_error() -> PersistenceError {
    PersistenceError::backend_with_kind(PersistenceErrorKind::Transient, "database is locked")
}

fn io_load_error() -> PersistenceError {
    PersistenceError::backend(WalletStorageError::Io(std::io::Error::other("EIO")))
}

#[test]
fn the_rehearsal_loads_the_copy_strict() {
    let f = fixture("v0.1.0");
    let before = tree(&f.root);
    LOAD_FOR_TEST.with(|l| l.set(Some(data_load_error)));
    let found = checked(&f);
    LOAD_FOR_TEST.with(|l| l.set(None));
    let found = found.unwrap();
    assert!(
        matches!(
            &found,
            Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { detail }
            } if detail.contains("upstream load")
        ),
        "{found:?}"
    );
    assert_eq!(tree(&f.root), before);

    for machine in [
        transient_load_error as fn() -> PersistenceError,
        io_load_error,
    ] {
        LOAD_FOR_TEST.with(|l| l.set(Some(machine)));
        let found = checked(&f);
        LOAD_FOR_TEST.with(|l| l.set(None));
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
    }
    assert_eq!(tree(&f.root), before);
    assert!(matches!(checked(&f).unwrap(), Precheck::Fork { .. }));
}

#[test]
fn an_unwritable_backup_dir_never_blocks_the_retire() {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    drop(open_and_load(&f.net));
    let again = crate::prepare_with(&f.net, build(), later(60))
        .unwrap()
        .snapshot
        .unwrap();
    fs::write(f.root.join("not-a-dir"), b"").unwrap();
    let unwritable =
        config(&f.net).with_auto_backup_dir(Some(f.root.join("not-a-dir").join("auto")));
    let outcome = reconcile_fork_store(&unwritable, &again, build()).unwrap();
    assert!(matches!(outcome, Outcome::Retired { .. }), "{outcome:?}");
    assert!(!f.root.join("not-a-dir").join("auto").exists());
}

#[cfg(unix)]
pub(super) fn assert_file_refused(f: &Fixture, needle: &str) {
    let before = tree(&f.root);
    let wallet = sha256_file(&f.wallet()).unwrap();
    let found = checked(f).unwrap();
    let Precheck::Refuse { reason } = &found else {
        panic!("{found:?}")
    };
    assert!(
        matches!(reason, StoreUpgradeError::WalletFileUnsupported { detail } if detail.contains(needle)),
        "{reason:?}"
    );
    assert!(reason.file_unchanged());
    assert_eq!(
        copy::screen_for(reason, ProvedState::default()).map(|s| s.cause),
        Some(RefusedCause::Unexpected)
    );
    assert_eq!(tree(&f.root), before);
    assert_eq!(sha256_file(&f.wallet()).unwrap(), wallet);
}

#[cfg(unix)]
#[test]
fn a_symlinked_wallet_file_is_refused_before_the_drop() {
    let f = fixture("v0.1.0");
    let real = f.root.join("elsewhere.sqlite");
    fs::rename(f.wallet(), &real).unwrap();
    std::os::unix::fs::symlink(&real, f.wallet()).unwrap();
    assert_file_refused(&f, "symbolic link");
    let err = crate::prepare_with(&f.net, build(), now()).unwrap();
    assert!(err.snapshot.is_none());
}

#[cfg(unix)]
#[test]
fn a_wallet_file_another_user_owns_is_refused_before_the_drop() {
    use std::os::unix::fs::MetadataExt;
    let f = fixture("v0.1.0");
    let mine = fs::metadata(f.wallet()).unwrap().uid();
    crate::precheck::EUID_FOR_TEST.with(|u| u.set(Some(mine + 1)));
    assert_file_refused(&f, "is owned by uid");
    exec(&f.wallet(), "PRAGMA wal_checkpoint(TRUNCATE)");
    super::review_fixes::hot_wal(&f, "CREATE TABLE dwsu_owner (a INTEGER)");
    assert_file_refused(&f, "is owned by uid");
    crate::precheck::EUID_FOR_TEST.with(|u| u.set(None));
    assert!(matches!(checked(&f).unwrap(), Precheck::Fork { .. }));
}
