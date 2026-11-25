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

const NEW_WALLET: &str = "INSERT INTO wallets (wallet_id, network, birth_height) \
     VALUES (X'00000000000000000000000000000000000000000000000000000000000000AA', 'regtest', 0)";

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

/// M3: the named snapshot of other data in a directory that can't be
/// written to is left as it is, no longer named, and never offered.
#[cfg(unix)]
#[test]
fn a_snapshot_of_other_data_that_cant_be_marked_is_no_longer_named() {
    use std::os::unix::fs::PermissionsExt;
    let (f, s) = failed_open();
    exec(&f.wallet(), BUMP);
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o500)).unwrap();
    let untouched = tree(s.dir());
    start_opens(
        &f,
        Some(crate::reconcile::UpgradeNotice::SnapshotOther),
        false,
    );
    let unmarked = !s.dir().join(NOT_THIS_WALLET).exists();
    let same = tree(s.dir()) == untouched;
    let record = record(&f);
    let state = snapshot_state(&f.net).unwrap();
    let request = crate::restore::request_restore(&f.net, &s);
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(unmarked && same, "left as it is");
    assert!(record.contains("snapshot none\n"), "{record}");
    assert!(!matches!(state, SnapshotState::Verified(_)), "{state:?}");
    assert!(
        matches!(request, Err(StoreUpgradeError::SnapshotInvalid { .. })),
        "{request:?}"
    );
    assert!(s.dir().join("wallet.sqlite").is_file());
}

/// M3: a named snapshot that can't be read for its permissions is
/// unverified: the start goes on with the damaged notice, never offering it.
#[test]
fn a_snapshot_unreadable_for_its_permissions_is_unverified() {
    let (f, s) = failed_open();
    let untouched = tree(s.dir());
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| {
        *h.borrow_mut() = Some((s.dir().to_path_buf(), std::io::ErrorKind::PermissionDenied))
    });
    let state = std::panic::catch_unwind(|| {
        start_opens(
            &f,
            Some(crate::reconcile::UpgradeNotice::SnapshotDamaged),
            false,
        );
        snapshot_state(&f.net).unwrap()
    });
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| *h.borrow_mut() = None);
    assert!(matches!(state.unwrap(), SnapshotState::Unverified { .. }));
    assert_eq!(tree(s.dir()), untouched, "never marked");
}

/// R1 committed, then upstream migrated the file.
fn migrated() -> (Fixture, SnapshotInfo) {
    let (f, s) = failed_open();
    start_opens(&f, None, true);
    assert!(refinery_max(&f) > crate::fork::FORK_REFINERY_MAX);
    (f, s)
}

/// The true snapshot is no longer offered, and is left as it is.
fn assert_not_offered(f: &Fixture, s: &SnapshotInfo, at: &str) {
    assert!(
        !matches!(snapshot_state(&f.net).unwrap(), SnapshotState::Verified(_)),
        "{at}"
    );
    assert_ne!(
        ProvedState::read(&f.net, false).backup,
        Some(BackupState::Verified),
        "{at}"
    );
    assert!(
        matches!(
            crate::restore::request_restore(&f.net, s),
            Err(StoreUpgradeError::SnapshotInvalid { .. })
        ),
        "{at}"
    );
}

#[test]
fn the_true_wallet_unchanged_once_migrated_restores() {
    let (f, s) = migrated();
    assert_backup(&f, &s, "migrated");
    crate::restore::request_restore(&f.net, &s).unwrap();
    assert!(start(&f, later(300)).unwrap().restored.is_some());
    assert!(column_present(&f));
}

#[test]
fn a_wallet_added_once_migrated_ends_the_offer() {
    let (f, s) = migrated();
    let untouched = tree(s.dir());
    exec(&f.wallet(), NEW_WALLET);
    assert_not_offered(&f, &s, "a wallet added");
    assert_eq!(tree(s.dir()), untouched);
    exec(
        &f.wallet(),
        "DELETE FROM wallets WHERE wallet_id = \
         X'00000000000000000000000000000000000000000000000000000000000000AA'",
    );
    assert_backup(&f, &s, "the same wallets again");
}

#[test]
fn another_wallets_file_swapped_in_once_migrated_is_never_rolled_back() {
    let (f, s) = migrated();
    let g = fixture("v0.2.0");
    let gs = start(&g, now()).unwrap().snapshot.unwrap();
    reconcile_fork_store(&config(&g.net), &gs, build()).unwrap();
    drop(open_and_load(&g.net));
    for side in ["", "-wal", "-shm"] {
        let name = format!("wallet.sqlite{side}");
        let _ = fs::remove_file(f.net.join(&name));
        if g.net.join(&name).exists() {
            fs::copy(g.net.join(&name), f.net.join(&name)).unwrap();
        }
    }
    let untouched = tree(s.dir());
    assert_not_offered(&f, &s, "swapped");
    start(&f, later(300)).unwrap();
    assert_not_offered(&f, &s, "after a start");
    assert_eq!(tree(s.dir()), untouched, "never marked, never changed");
}

#[test]
fn live_wallet_ids_only_in_the_wal_are_read() {
    let (f, s) = migrated();
    super::review_fixes::hot_wal(&f, NEW_WALLET);
    let wal = f.net.join("wallet.sqlite-wal");
    assert!(fs::metadata(&wal).unwrap().len() > 0);
    let logged = crate::sql::sha256_file(&wal).unwrap();
    assert_not_offered(&f, &s, "a wallet added in the -wal only");
    assert_eq!(
        crate::sql::sha256_file(&wal).unwrap(),
        logged,
        "never checkpointed"
    );
}

/// L3: the backup check fingerprints live rows that are only in the
/// `-wal`, never checkpoints it and makes no `-shm` beside the live file.
#[test]
fn the_backup_check_reads_rows_only_in_the_wal_and_writes_nothing_beside_them() {
    let (f, s) = failed_open();
    assert_eq!(crate::snapshot::of_live(&f.net, &s), Some(true));
    super::review_fixes::hot_wal(&f, BUMP);
    let wal = f.net.join("wallet.sqlite-wal");
    let shm = f.net.join("wallet.sqlite-shm");
    assert!(fs::metadata(&wal).unwrap().len() > 0 && !shm.exists());
    let before = tree(&f.root);
    assert_eq!(
        crate::snapshot::of_live(&f.net, &s),
        Some(false),
        "the row in the -wal is read"
    );
    assert_eq!(
        ProvedState::read(&f.net, false).backup,
        Some(BackupState::Unverified)
    );
    assert!(!shm.exists(), "no -shm beside the live file");
    assert_eq!(tree(&f.root), before, "nothing written, never checkpointed");
}

fn part_dirs(f: &Fixture) -> Vec<std::path::PathBuf> {
    let mut dirs: Vec<_> = fs::read_dir(f.net.join(UPGRADE_BACKUPS_DIR))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("part-upgraded-")
        })
        .collect();
    dirs.sort();
    dirs
}

/// L5: a new copy that doesn't verify once taken is retried and kept as it
/// is; the next start takes one more and later starts keep that one.
#[test]
fn a_copy_that_fails_to_verify_once_taken_adds_one_copy_per_fault() {
    let (f, s) = failed_open();
    fs::remove_dir_all(s.dir()).unwrap();
    arm_fault("snapshot.corrupt_copy", 0);
    let err = start(&f, later(30)).unwrap_err();
    disarm_fault();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotFailed { .. }),
        "{err:?}"
    );
    assert!(crate::copy::retry_copy_for(&err).is_some());
    let [faulty] = &part_dirs(&f)[..] else {
        panic!("one copy");
    };
    let kept = tree(faulty);
    let part = start(&f, later(60)).unwrap().snapshot.unwrap();
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &part, build()).unwrap(),
        Outcome::Pending { .. }
    ));
    let again = start(&f, later(90)).unwrap().snapshot.unwrap();
    assert_eq!(again.dir(), part.dir());
    assert_eq!(part_dirs(&f).len(), 2);
    assert_eq!(&tree(faulty), &kept, "never deleted, never changed");
    assert!(read_snapshot(part.dir(), true).is_ok());
}

/// Q110 110-1: only the part-upgraded copy this upgrade took counts and is
/// kept; one copied in, verified and listing shielded.sqlite, never is.
#[test]
fn a_part_upgraded_copy_copied_in_is_never_kept_or_counted() {
    let (g, gs) = failed_open();
    fs::remove_dir_all(gs.dir()).unwrap();
    let other = start(&g, later(30)).unwrap().snapshot.unwrap();
    let (f, s) = failed_open();
    fs::remove_dir_all(s.dir()).unwrap();
    let foreign = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("part-upgraded-20991231T000000Z");
    fs::create_dir(&foreign).unwrap();
    for entry in fs::read_dir(other.dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), foreign.join(entry.file_name())).unwrap();
    }
    let copied = read_snapshot(&foreign, true).unwrap();
    assert!(copied.files().iter().any(|x| x.name == "shielded.sqlite"));
    let untouched = tree(&foreign);
    assert!(!upgrade_copy_present(&f.net, "shielded.sqlite").unwrap());
    let part = start(&f, later(60)).unwrap().snapshot.unwrap();
    assert_ne!(part.dir(), foreign.as_path());
    assert!(upgrade_copy_present(&f.net, "shielded.sqlite").unwrap());
    assert_eq!(tree(&foreign), untouched);
}

/// A fault reading the live file for the gate that may pass is retried,
/// never read as a damaged snapshot: no refusal, no notice, no extra copy.
#[test]
fn a_fault_reading_the_live_file_for_the_gate_is_retried() {
    let (f, s) = failed_open();
    arm_fault("snapshot.open_copy", 0);
    let err = reconcile_fork_store(&config(&f.net), &s, build()).unwrap_err();
    disarm_fault();
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
    assert!(!err.is_refusal() && crate::copy::retry_copy_for(&err).is_some());
    arm_fault("snapshot.open_copy", 0);
    let started = start(&f, later(60));
    disarm_fault();
    assert!(
        matches!(started, Err(StoreUpgradeError::Io { .. })),
        "{started:?}"
    );
    assert!(part_dirs(&f).is_empty(), "no part-upgraded copy");
    let kept = start(&f, later(90)).unwrap().snapshot.unwrap();
    assert_eq!(kept.dir(), s.dir());
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &kept, build()).unwrap(),
        Outcome::Pending { notice: None, .. }
    ));
}

/// A restore's journal lifts the gate for the snapshot it names only.
#[test]
fn a_restore_journal_lifts_the_gate_for_its_own_snapshot_only() {
    let (f, s) = migrated();
    let foreign = drop_in_foreign(&f);
    let journal = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join(crate::restore::RESTORE_JOURNAL);
    fs::write(
        &journal,
        format!(
            "snapshot {}\nmanifest {}\n",
            s.dir().file_name().unwrap().to_string_lossy(),
            s.manifest_sha256()
        ),
    )
    .unwrap();
    exec(&f.wallet(), NEW_WALLET);
    assert!(read_snapshot(s.dir(), true).is_ok(), "the journal's own");
    assert!(read_snapshot(&foreign, true).is_err(), "any other");
    fs::remove_file(&journal).unwrap();
    assert!(read_snapshot(s.dir(), true).is_err(), "gated again");
}

/// With a record and no `wallet.sqlite`, only the named snapshot verifies.
#[test]
fn a_missing_wallet_file_is_held_to_the_named_snapshot() {
    let (f, s) = migrated();
    let foreign = drop_in_foreign(&f);
    for side in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(f.net.join(format!("wallet.sqlite{side}")));
    }
    assert!(read_snapshot(s.dir(), true).is_ok());
    assert!(read_snapshot(&foreign, true).is_err());
}

/// R1 naming a new before-upgrade snapshot ends an earlier upgrade's
/// part-upgraded copy record, which then never counts.
#[test]
fn r1_ends_an_earlier_part_upgraded_copy_record() {
    let f = fixture("v0.1.0");
    let s = start(&f, now()).unwrap().snapshot.unwrap();
    let stale = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("part-upgraded-20991231T000000Z");
    fs::create_dir(&stale).unwrap();
    for entry in fs::read_dir(s.dir()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), stale.join(entry.file_name())).unwrap();
    }
    crate::snapshot::record_part(&f.net, &read_snapshot(&stale, false).unwrap()).unwrap();
    let record = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join(crate::snapshot::PART_UPGRADED_BY);
    assert!(record.is_file());
    reconcile_fork_store(&config(&f.net), &s, build()).unwrap();
    assert!(!record.exists());
    let app = s.dir().join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[100] ^= 1;
    fs::write(&app, bytes).unwrap();
    assert!(!upgrade_copy_present(&f.net, "shielded.sqlite").unwrap());
}
