//! The re-review of ffbc69cf: a snapshot whose marker was lost after R1's
//! drop (M-A), snapshots of an upgraded file (M-B), a fault reading a
//! snapshot (L-1), the wallet file's side files (L-5), the file checked
//! again under the write lock (L-4), and tests of a snapshot past V018 and
//! of the rehearsal's upstream open (L-7).

use std::io::ErrorKind;

use platform_wallet_storage::WalletStorageError;

#[cfg(unix)]
use super::review_fixes::column_present;
#[cfg(unix)]
use super::review_fixes_3::assert_file_refused;
use super::review_fixes_3::{assert_missing, between, checked};
use super::*;
use crate::copy::{self, ProvedState, RefusedCause};
use crate::error::StoreUpgradeError;
use crate::precheck::{OPEN_FOR_TEST, Precheck};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::{
    ABSENT_POLICY_FOR_TEST, AbsentSnapshotPolicy, MARKER, MissingSnapshot, SNAPSHOT_READ_FOR_TEST,
    SnapshotInfo, SnapshotState, UPGRADE_BACKUPS_DIR, snapshot_state, take_snapshot,
};

#[test]
fn a_lost_marker_after_the_drop_is_unverified_and_the_copy_is_kept() {
    for policy in [
        AbsentSnapshotPolicy::Refuse,
        AbsentSnapshotPolicy::ProceedUnrestorable,
    ] {
        let (f, s) = between();
        fs::remove_file(s.dir().join(MARKER)).unwrap();
        ABSENT_POLICY_FOR_TEST.with(|p| p.set(Some(policy)));
        assert_missing(&f, MissingSnapshot::Unverified);
        ABSENT_POLICY_FOR_TEST.with(|p| p.set(None));
        assert!(s.dir().join("wallet.sqlite").is_file(), "{policy:?}");
    }
}

fn prepare_under(
    f: &Fixture,
    policy: AbsentSnapshotPolicy,
    at: SystemTime,
) -> crate::Result<crate::Prepared> {
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(Some(policy)));
    let got = crate::prepare_with(&f.net, build(), at);
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(None));
    got
}

fn before_upgrade_dirs(f: &Fixture) -> Vec<String> {
    fs::read_dir(f.net.join(UPGRADE_BACKUPS_DIR))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("before-upgrade-"))
        .collect()
}

fn assert_absent(got: &crate::Result<crate::Prepared>) {
    assert!(
        matches!(
            got,
            Err(StoreUpgradeError::UpgradeSnapshotMissing {
                state: MissingSnapshot::Absent,
                ..
            })
        ),
        "{got:?}"
    );
}

/// The before-upgrade snapshot is gone after R1's drop; upstream migrates
/// the file (the persister opens), then a third start prepares.
fn migrated_without_a_snapshot(policy: AbsentSnapshotPolicy) -> (Fixture, Option<SnapshotInfo>) {
    let (f, s) = between();
    fs::remove_dir_all(s.dir()).unwrap();
    let first = prepare_under(&f, policy, later(60));
    let again = prepare_under(&f, policy, later(120));
    let part = match policy {
        AbsentSnapshotPolicy::Refuse => {
            assert_absent(&first);
            assert_absent(&again);
            None
        }
        AbsentSnapshotPolicy::ProceedUnrestorable => {
            let part = first.unwrap().snapshot.unwrap();
            assert!(
                part.dir()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("part-upgraded-")
            );
            assert_eq!(again.unwrap().snapshot.unwrap().dir(), part.dir(), "reused");
            assert!(crate::restore::request_restore(&f.net, &part).is_err());
            assert!(matches!(
                reconcile_fork_store(&config(&f.net), &part, build()).unwrap(),
                Outcome::Pending { .. }
            ));
            Some(part)
        }
    };
    drop(open_and_load(&f.net));
    assert_eq!(copy::migrated(&f.net), Some(true));
    (f, part)
}

#[test]
fn a_migrated_file_without_a_snapshot_is_refused_and_never_copied_as_a_backup() {
    let (f, _) = migrated_without_a_snapshot(AbsentSnapshotPolicy::Refuse);
    let before = tree(&f.root);
    let third = prepare_under(&f, AbsentSnapshotPolicy::Refuse, later(180));
    assert_absent(&third);
    assert_eq!(tree(&f.root), before, "nothing written");
    assert_eq!(snapshot_state(&f.net).unwrap(), SnapshotState::None);
    assert_eq!(before_upgrade_dirs(&f), Vec::<String>::new());
}

#[test]
fn absent_can_be_switched_to_proceed_without_a_restorable_snapshot() {
    let policy = AbsentSnapshotPolicy::ProceedUnrestorable;
    let (f, part) = migrated_without_a_snapshot(policy);
    let part = part.unwrap();
    let before = tree(&f.root);
    let kept = prepare_under(&f, policy, later(180))
        .unwrap()
        .snapshot
        .unwrap();
    assert_eq!(kept.dir(), part.dir(), "the part-upgraded copy is kept");
    assert_eq!(tree(&f.root), before, "nothing written");
    assert_eq!(snapshot_state(&f.net).unwrap(), SnapshotState::None);
    assert_eq!(before_upgrade_dirs(&f), Vec::<String>::new());
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &kept, build()).unwrap(),
        Outcome::Retired { .. }
    ));
    assert_eq!(snapshot_state(&f.net).unwrap(), SnapshotState::None);
}

#[test]
fn a_part_upgraded_copy_renamed_after_the_migration_is_never_kept() {
    let (f, part) = migrated_without_a_snapshot(AbsentSnapshotPolicy::ProceedUnrestorable);
    let posing = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("before-upgrade-20991231T000000Z");
    fs::rename(part.unwrap().dir(), &posing).unwrap();
    assert!(crate::snapshot::read_snapshot(&posing, true).is_ok());
    for policy in [
        AbsentSnapshotPolicy::Refuse,
        AbsentSnapshotPolicy::ProceedUnrestorable,
    ] {
        let before = tree(&f.root);
        let got = prepare_under(&f, policy, later(180));
        assert!(
            matches!(
                got,
                Err(StoreUpgradeError::UpgradeSnapshotMissing {
                    state: MissingSnapshot::Mismatch,
                    ..
                })
            ),
            "{policy:?}: {got:?}"
        );
        assert_eq!(tree(&f.root), before, "{policy:?}");
    }
}

fn fail_reads_of(dir: &std::path::Path) {
    SNAPSHOT_READ_FOR_TEST.with(|k| *k.borrow_mut() = Some((dir.to_path_buf(), ErrorKind::Other)));
}

fn read_snapshots() {
    SNAPSHOT_READ_FOR_TEST.with(|k| *k.borrow_mut() = None);
}

#[test]
fn a_fault_reading_the_snapshot_is_retried_and_a_missing_file_is_unverified() {
    let (f, s) = between();
    let before = tree(&f.root);
    fail_reads_of(s.dir());
    let err = crate::prepare_with(&f.net, build(), later(60)).unwrap_err();
    read_snapshots();
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
    assert_eq!(copy::screen_for(&err, ProvedState::default()), None);
    assert_eq!(tree(&f.root), before, "nothing written");

    fs::remove_file(s.dir().join("app.sqlite")).unwrap();
    assert_missing(&f, MissingSnapshot::Unverified);
}

#[test]
fn a_snapshot_that_fails_to_read_never_hides_the_true_one() {
    let (f, s) = between();
    let newer = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("before-upgrade-20991231T000000Z");
    fs::create_dir(&newer).unwrap();
    fail_reads_of(&newer);
    let kept = crate::prepare_with(&f.net, build(), later(60));
    read_snapshots();
    assert_eq!(kept.unwrap().snapshot.unwrap().dir(), s.dir());
    assert!(newer.is_dir());

    let g = fixture("v0.1.0");
    let first = crate::prepare_with(&g.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    fail_reads_of(first.dir());
    let again = crate::prepare_with(&g.net, build(), later(60));
    read_snapshots();
    let again = again.unwrap().snapshot.unwrap();
    assert_ne!(again.dir(), first.dir(), "not upgraded: a new snapshot");
    assert!(first.dir().join(MARKER).is_file());
}

#[cfg(unix)]
#[test]
fn a_symlinked_log_or_shared_memory_file_is_refused_before_the_drop() {
    for suffix in ["-wal", "-shm"] {
        let f = fixture("v0.1.0");
        let side = f.net.join(format!("wallet.sqlite{suffix}"));
        let _ = fs::remove_file(&side);
        let real = f.root.join(format!("elsewhere{suffix}"));
        fs::write(&real, b"").unwrap();
        std::os::unix::fs::symlink(&real, &side).unwrap();
        assert_file_refused(&f, &format!("wallet.sqlite{suffix} is a symbolic link"));
    }
    let f = fixture("v0.1.0");
    exec(&f.wallet(), "PRAGMA wal_checkpoint(TRUNCATE)");
    super::review_fixes::hot_wal(&f, "CREATE TABLE dwsu_side (a INTEGER)");
    let wal = f.net.join("wallet.sqlite-wal");
    let real = f.root.join("elsewhere-wal");
    fs::rename(&wal, &real).unwrap();
    std::os::unix::fs::symlink(&real, &wal).unwrap();
    assert_file_refused(&f, "wallet.sqlite-wal is a symbolic link");
}

#[cfg(unix)]
fn swap_for_a_symlink(net: &std::path::Path) {
    let wallet = net.join("wallet.sqlite");
    let real = net.parent().unwrap().join("swapped.sqlite");
    fs::rename(&wallet, &real).unwrap();
    std::os::unix::fs::symlink(&real, &wallet).unwrap();
}

#[cfg(unix)]
#[test]
fn a_wallet_file_swapped_for_a_symlink_under_the_lock_is_refused() {
    let f = fixture("v0.1.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    crate::reconcile::LOCKED_FOR_TEST.with(|h| h.set(Some(swap_for_a_symlink)));
    let got = reconcile_fork_store(&config(&f.net), &snapshot, build());
    crate::reconcile::LOCKED_FOR_TEST.with(|h| h.set(None));
    let err = got.unwrap_err();
    assert!(
        matches!(&err, StoreUpgradeError::WalletFileUnsupported { detail } if detail.contains("symbolic link")),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert!(column_present(&f), "the drop was rolled back");
}

#[test]
fn a_restorable_snapshot_past_v018_is_never_kept_as_the_backup() {
    let g = fixture("v0.1.0");
    exec(
        &g.wallet(),
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (23, 'V23__posing', '', '0')",
    );
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(Some(AbsentSnapshotPolicy::ProceedUnrestorable)));
    let past = take_snapshot(&g.net, now());
    ABSENT_POLICY_FOR_TEST.with(|p| p.set(None));
    let past = past.unwrap();
    assert!(past.refinery_max() > 18);

    let (f, s) = between();
    fs::remove_dir_all(s.dir()).unwrap();
    let posing = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("before-upgrade-20991231T000000Z");
    fs::create_dir(&posing).unwrap();
    for entry in fs::read_dir(past.dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), posing.join(entry.file_name())).unwrap();
    }
    assert!(crate::snapshot::read_snapshot(&posing, true).is_ok());
    let copy = crate::sql::open_immutable(&posing.join("wallet.sqlite")).unwrap();
    assert!(crate::sql::has_column(&copy, "contacts", "external_account_reference").unwrap());
    drop(copy);
    assert_missing(&f, MissingSnapshot::Mismatch);
}

fn readonly_migration() -> WalletStorageError {
    let tmp = private_tempdir();
    let path = tmp.path().join("ro.sqlite");
    drop(Connection::open(&path).unwrap());
    let mut ro =
        Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let migration = refinery::Migration::unapplied("V1__a", "CREATE TABLE a (x INTEGER)").unwrap();
    WalletStorageError::Migration(
        refinery::Runner::new(&[migration])
            .run(&mut ro)
            .expect_err("read-only"),
    )
}

fn integrity_run_disk_full() -> WalletStorageError {
    WalletStorageError::IntegrityCheckRunFailed {
        source: rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_FULL),
            None,
        ),
    }
}

fn integrity_failed() -> WalletStorageError {
    WalletStorageError::IntegrityCheckFailed {
        report: "row 1 missing from index".into(),
    }
}

#[test]
fn the_rehearsal_reads_a_failed_upstream_open_by_its_cause() {
    let f = fixture("v0.1.0");
    let before = tree(&f.root);
    let rehearse = |open: fn() -> WalletStorageError| {
        OPEN_FOR_TEST.with(|o| o.set(Some(open)));
        let got = checked(&f);
        OPEN_FOR_TEST.with(|o| o.set(None));
        got
    };

    let err = rehearse(integrity_run_disk_full).unwrap_err();
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

    let got = rehearse(readonly_migration).unwrap();
    let Precheck::Refuse { reason } = &got else {
        panic!("{got:?}")
    };
    assert!(
        matches!(reason, StoreUpgradeError::UpgradeCopyUnwritable { detail } if detail.contains("upstream open")),
        "{reason:?}"
    );
    assert!(reason.is_refusal() && reason.file_unchanged());
    assert_eq!(
        copy::screen_for(reason, ProvedState::default()).map(|s| s.cause),
        Some(RefusedCause::Unexpected)
    );

    let got = rehearse(integrity_failed).unwrap();
    assert!(
        matches!(
            &got,
            Precheck::Refuse {
                reason: StoreUpgradeError::UpgradeRehearsalFailed { .. }
            }
        ),
        "{got:?}"
    );

    assert_eq!(tree(&f.root), before, "nothing written");
    assert!(matches!(checked(&f).unwrap(), Precheck::Fork { .. }));
}

#[cfg(unix)]
#[test]
fn a_write_protected_wallet_file_is_refused_not_retried() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture("v0.1.0");
    fs::set_permissions(f.wallet(), fs::Permissions::from_mode(0o444)).unwrap();
    if fs::OpenOptions::new().write(true).open(f.wallet()).is_ok() {
        eprintln!("skipped: this process writes a 0444 file");
        return;
    }
    let before = tree(&f.root);
    let got = checked(&f).unwrap();
    let Precheck::Refuse { reason } = &got else {
        panic!("{got:?}")
    };
    assert!(
        matches!(reason, StoreUpgradeError::UpgradeCopyUnwritable { .. }),
        "{reason:?}"
    );
    assert_eq!(tree(&f.root), before, "nothing written");
}
