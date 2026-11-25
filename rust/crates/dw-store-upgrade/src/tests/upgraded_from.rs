//! [`crate::snapshot::UPGRADED_FROM`]: R1 names the snapshot the file is
//! upgraded from, and once upgraded no other before-upgrade directory is
//! ever its backup.

use super::q107::{assert_plain_reopen, failed_open, refinery_max, start, start_opens};
use super::review_fixes::column_present;
use super::*;
use crate::copy::{BackupState, ProvedState};
use crate::error::StoreUpgradeError;
use crate::fsx::{arm_fault, disarm_fault};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::{
    NOT_THIS_WALLET, SnapshotInfo, SnapshotState, UPGRADE_BACKUPS_DIR, UPGRADED_FROM,
    read_snapshot, snapshot_state, upgrade_copy_present,
};

const BUMP: &str =
    "UPDATE core_utxos SET value = value + 1 WHERE rowid = (SELECT MIN(rowid) FROM core_utxos)";

fn record(f: &Fixture) -> String {
    fs::read_to_string(f.net.join(UPGRADE_BACKUPS_DIR).join(UPGRADED_FROM)).unwrap()
}

fn names(f: &Fixture, s: &SnapshotInfo) -> bool {
    let name = s.dir().file_name().unwrap().to_string_lossy();
    record(f).contains(&format!("snapshot {name} {}\n", s.manifest_sha256()))
}

/// The true snapshot is the backup: verified, of the live data as
/// [`ProvedState`] reads it, never marked.
fn assert_backup(f: &Fixture, s: &SnapshotInfo, at: &str) {
    let SnapshotState::Verified(newest) = snapshot_state(&f.net).unwrap() else {
        panic!("{at}: the true snapshot stays verified");
    };
    assert_eq!(newest.manifest_sha256(), s.manifest_sha256(), "{at}");
    assert_eq!(
        ProvedState::read(&f.net, false).backup,
        Some(BackupState::Verified),
        "{at}"
    );
    assert!(
        upgrade_copy_present(&f.net, "wallet.sqlite").unwrap(),
        "{at}"
    );
    assert!(!s.dir().join(NOT_THIS_WALLET).exists(), "{at}");
}

/// A verified before-upgrade snapshot of another wallet with the same fork
/// versions, dropped into `f`'s `upgrade-backups/` under a newer name.
fn drop_in_foreign(f: &Fixture) -> std::path::PathBuf {
    let g = fixture("v0.1.0");
    exec(&g.wallet(), BUMP);
    exec(&g.wallet(), BUMP);
    let other = crate::snapshot::take_snapshot(&g.net, later(400)).unwrap();
    let foreign = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("before-upgrade-20991231T000000Z");
    fs::create_dir(&foreign).unwrap();
    for entry in fs::read_dir(other.dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), foreign.join(entry.file_name())).unwrap();
    }
    assert!(read_snapshot(&foreign, false).is_ok());
    foreign
}

fn assert_never_offered(f: &Fixture, foreign: &std::path::Path, at: &str) {
    assert!(read_snapshot(foreign, true).is_err(), "{at}");
    let info = read_snapshot(foreign, false).unwrap();
    assert!(
        matches!(
            crate::restore::request_restore(&f.net, &info),
            Err(StoreUpgradeError::SnapshotInvalid { .. })
        ),
        "{at}"
    );
    assert_eq!(
        crate::snapshot::latest_snapshot(&f.net)
            .unwrap()
            .map(|i| i.dir().to_path_buf()),
        snapshot_state_dir(f),
        "{at}"
    );
}

fn snapshot_state_dir(f: &Fixture) -> Option<std::path::PathBuf> {
    match snapshot_state(&f.net).unwrap() {
        SnapshotState::Verified(info) => Some(info.dir().to_path_buf()),
        _ => None,
    }
}

#[test]
fn r1_names_the_snapshot_before_its_drop() {
    let (f, s) = failed_open();
    assert!(names(&f, &s), "{}", record(&f));
}

#[test]
fn a_foreign_snapshot_dropped_in_once_migrated_is_never_the_backup() {
    let (f, s) = failed_open();
    start_opens(&f, None, true);
    assert!(refinery_max(&f) > crate::fork::FORK_REFINERY_MAX);
    let foreign = drop_in_foreign(&f);
    let untouched = tree(&foreign);
    assert_never_offered(&f, &foreign, "done");
    assert_backup(&f, &s, "done");
    assert_plain_reopen(&f);
    assert_never_offered(&f, &foreign, "retired");
    assert_backup(&f, &s, "retired");
    assert_eq!(tree(&foreign), untouched, "never marked, never changed");
}

#[test]
fn the_true_snapshot_stays_the_backup_once_migrated_whatever_the_rows() {
    let (f, s) = failed_open();
    start_opens(&f, None, true);
    exec(&f.wallet(), BUMP);
    assert_backup(&f, &s, "done, rows changed");
    assert_plain_reopen(&f);
    exec(&f.wallet(), BUMP);
    assert_backup(&f, &s, "retired, rows changed");
    start(&f, later(500)).unwrap();
    assert_backup(&f, &s, "a later start");
}

#[test]
fn a_crash_between_the_record_and_the_drop_leaves_a_start_that_works() {
    let f = fixture("v0.1.0");
    let s = start(&f, now()).unwrap().snapshot.unwrap();
    arm_fault("reconcile.after_lineage", 0);
    let err = reconcile_fork_store(&config(&f.net), &s, build()).unwrap_err();
    disarm_fault();
    assert!(
        matches!(err, StoreUpgradeError::InjectedFault(_)),
        "{err:?}"
    );
    assert!(column_present(&f), "the drop rolled back");
    assert!(names(&f, &s));

    let again = start(&f, later(60)).unwrap().snapshot.unwrap();
    assert_eq!(again.dir(), s.dir());
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &again, build()).unwrap(),
        Outcome::Pending { notice: None, .. }
    ));
    drop(open_and_load(&f.net));
    assert!(names(&f, &s));
    assert_backup(&f, &s, "migrated");
}

#[test]
fn a_record_that_cant_be_written_stops_before_the_drop_and_is_retried() {
    let f = fixture("v0.1.0");
    let s = start(&f, now()).unwrap().snapshot.unwrap();
    let path = f.net.join(UPGRADE_BACKUPS_DIR).join(UPGRADED_FROM);
    fs::create_dir(&path).unwrap();
    let err = reconcile_fork_store(&config(&f.net), &s, build()).unwrap_err();
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
    assert!(crate::copy::retry_copy_for(&err).is_some());
    assert!(column_present(&f));
    fs::remove_dir(&path).unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &s, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    assert!(names(&f, &s));
}
