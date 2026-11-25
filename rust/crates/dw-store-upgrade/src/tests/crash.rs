//! Crash injection: each interrupted run, started again, ends where an
//! uninterrupted run does (table-by-table checksums).

use super::*;
use crate::error::StoreUpgradeError;
use crate::fsx::{arm_fault, disarm_fault};
use crate::precheck::{Precheck, precheck_with};
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::snapshot::{SnapshotInfo, take_snapshot};

/// One start: precheck, snapshot, reconcile, open the persister.
fn start(
    f: &Fixture,
    at: SystemTime,
) -> std::result::Result<(SnapshotInfo, Outcome), StoreUpgradeError> {
    match precheck_with(&f.net, build())? {
        Precheck::Fork { .. } => {}
        other => panic!("expected a fork file, got {other:?}"),
    }
    let snapshot = take_snapshot(&f.net, at)?;
    let outcome = reconcile_fork_store(&f.wallet(), &snapshot, build())?;
    drop(open_and_load(&f.net));
    Ok((snapshot, outcome))
}

/// Two clean starts: Pending, then Retired.
fn uninterrupted(version: &str) -> (BTreeMap<String, String>, usize) {
    let f = fixture(version);
    let (_, first) = start(&f, now()).unwrap();
    assert!(matches!(first, Outcome::Pending { .. }));
    let (_, second) = start(&f, later(60)).unwrap();
    assert!(matches!(second, Outcome::Retired { .. }));
    (tables(&f.wallet()), snapshot_count(&f))
}

fn snapshot_count(f: &Fixture) -> usize {
    crate::snapshot::completed(&f.net).len()
}

fn finish(f: &Fixture) {
    loop {
        if matches!(precheck_with(&f.net, build()).unwrap(), Precheck::NotForked) {
            break;
        }
        start(f, later(120)).unwrap();
    }
}

#[test]
fn crash_during_the_drop_transaction_converges() {
    for version in ["v0.1.0", "v0.3.0"] {
        let (want, snapshots) = uninterrupted(version);
        let f = fixture(version);
        let original = tables(&f.wallet());
        arm_fault("reconcile.after_drop", 0);
        let err = start(&f, now()).unwrap_err();
        disarm_fault();
        assert!(matches!(
            err,
            StoreUpgradeError::InjectedFault("reconcile.after_drop")
        ));
        assert_eq!(tables(&f.wallet()), original, "the drop rolled back");
        let conn = Connection::open(f.wallet()).unwrap();
        assert!(crate::sql::has_column(&conn, "contacts", "external_account_reference").unwrap());
        drop(conn);
        finish(&f);
        assert_eq!(tables(&f.wallet()), want, "{version}");
        assert_eq!(snapshot_count(&f), snapshots);
    }
}

#[test]
fn crash_between_the_upstream_migration_and_the_retire_converges() {
    let (want, snapshots) = uninterrupted("v0.2.0");
    let f = fixture("v0.2.0");
    let (_, first) = start(&f, now()).unwrap();
    assert!(matches!(first, Outcome::Pending { .. }));
    let migrated = tables(&f.wallet());
    arm_fault("reconcile.after_retire", 0);
    let err = start(&f, later(60)).unwrap_err();
    disarm_fault();
    assert!(matches!(
        err,
        StoreUpgradeError::InjectedFault("reconcile.after_retire")
    ));
    assert_eq!(tables(&f.wallet()), migrated, "the retire rolled back");
    finish(&f);
    assert_eq!(tables(&f.wallet()), want);
    assert_eq!(snapshot_count(&f), snapshots);
}

#[test]
fn crash_mid_snapshot_converges() {
    let (want, snapshots) = uninterrupted("v0.1.0");
    let f = fixture("v0.1.0");
    arm_fault("snapshot.after_file", 1);
    let err = start(&f, now()).unwrap_err();
    disarm_fault();
    assert!(matches!(
        err,
        StoreUpgradeError::InjectedFault("snapshot.after_file")
    ));
    let left: Vec<_> = fs::read_dir(f.net.join("upgrade-backups"))
        .unwrap()
        .collect();
    assert_eq!(left.len(), 1, "one incomplete snapshot dir");
    assert_eq!(snapshot_count(&f), 0);
    for name in crate::snapshot::STORE_FILES {
        assert_eq!(
            tables(&f.net.join(name)),
            tables(&fixture("v0.1.0").net.join(name)),
            "{name}"
        );
    }
    finish(&f);
    assert_eq!(tables(&f.wallet()), want);
    assert_eq!(snapshot_count(&f), snapshots);
}

#[test]
fn crash_before_the_snapshot_marker_converges() {
    let (want, snapshots) = uninterrupted("v0.3.0");
    let f = fixture("v0.3.0");
    arm_fault("snapshot.before_marker", 0);
    assert!(start(&f, now()).is_err());
    disarm_fault();
    assert_eq!(snapshot_count(&f), 0);
    finish(&f);
    assert_eq!(tables(&f.wallet()), want);
    assert_eq!(snapshot_count(&f), snapshots);
}
