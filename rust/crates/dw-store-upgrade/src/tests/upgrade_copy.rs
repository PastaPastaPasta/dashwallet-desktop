//! [`crate::upgrade_copy_present`]: which upgrade copies of a store count.

use super::*;
use crate::snapshot::{NOT_THIS_WALLET, SnapshotInfo, UPGRADE_BACKUPS_DIR, upgrade_copy_present};

const SHIELDED: &str = "shielded.sqlite";

fn present(f: &Fixture, store: &str) -> bool {
    let before = tree(&f.root);
    let found = upgrade_copy_present(&f.net, store).unwrap();
    assert_eq!(tree(&f.root), before, "{store}: reads only");
    found
}

fn snapshot(f: &Fixture) -> SnapshotInfo {
    crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap()
}

/// `s` renamed as a `part-upgraded` copy, recorded as this upgrade's when
/// `recorded`.
fn as_part_upgraded(f: &Fixture, s: &SnapshotInfo, recorded: bool) -> std::path::PathBuf {
    let part = f
        .net
        .join(UPGRADE_BACKUPS_DIR)
        .join("part-upgraded-20991231T000000Z");
    fs::rename(s.dir(), &part).unwrap();
    if recorded {
        let info = crate::snapshot::read_snapshot(&part, false).unwrap();
        crate::snapshot::record_part(&f.net, &info).unwrap();
    }
    part
}

fn corrupt(dir: &std::path::Path) {
    let app = dir.join("app.sqlite");
    let mut bytes = fs::read(&app).unwrap();
    bytes[100] ^= 1;
    fs::write(&app, bytes).unwrap();
}

#[test]
fn a_verified_before_upgrade_snapshot_that_lists_the_store_counts() {
    let f = fixture("v0.1.0");
    assert!(!present(&f, SHIELDED), "no copy yet");
    let s = snapshot(&f);
    assert!(s.files().iter().any(|x| x.name == SHIELDED));
    assert!(present(&f, SHIELDED));
    corrupt(s.dir());
    assert!(!present(&f, SHIELDED), "unverified");
}

#[test]
fn a_verified_part_upgraded_copy_that_lists_the_store_counts() {
    let f = fixture("v0.1.0");
    let s = snapshot(&f);
    let part = as_part_upgraded(&f, &s, true);
    assert!(present(&f, SHIELDED));
    corrupt(&part);
    assert!(!present(&f, SHIELDED), "unverified");
}

#[test]
fn a_copy_without_the_store_never_counts() {
    let f = fixture("v0.1.0");
    fs::remove_file(f.net.join(SHIELDED)).unwrap();
    let s = snapshot(&f);
    assert!(s.absent().iter().any(|x| x == SHIELDED));
    assert!(!present(&f, SHIELDED));
    assert!(present(&f, "wallet.sqlite"));
    as_part_upgraded(&f, &s, true);
    assert!(!present(&f, SHIELDED));
    assert!(present(&f, "wallet.sqlite"));
}

#[test]
fn a_copy_marked_not_this_wallet_never_counts() {
    let f = fixture("v0.1.0");
    let s = snapshot(&f);
    fs::write(s.dir().join(NOT_THIS_WALLET), b"").unwrap();
    assert!(!present(&f, SHIELDED));
    assert!(crate::snapshot::read_snapshot(s.dir(), true).is_err());
    fs::remove_file(s.dir().join(NOT_THIS_WALLET)).unwrap();
    let part = as_part_upgraded(&f, &s, true);
    assert!(present(&f, SHIELDED));
    fs::write(part.join(NOT_THIS_WALLET), b"").unwrap();
    assert!(!present(&f, SHIELDED));
}

#[test]
fn a_part_upgraded_copy_this_upgrade_didnt_take_never_counts() {
    let f = fixture("v0.1.0");
    let s = snapshot(&f);
    let part = as_part_upgraded(&f, &s, false);
    assert!(crate::snapshot::read_snapshot(&part, true).is_ok());
    let untouched = tree(&part);
    assert!(!present(&f, SHIELDED), "copied in, not recorded");
    assert_eq!(tree(&part), untouched);
}

fn migrated_and_retired() -> (Fixture, SnapshotInfo) {
    let f = fixture("v0.1.0");
    let s = snapshot(&f);
    crate::reconcile_fork_store(&config(&f.net), &s, build()).unwrap();
    let conn = rusqlite::Connection::open(f.wallet()).unwrap();
    let id: Vec<u8> = conn
        .query_row("SELECT wallet_id FROM wallets LIMIT 1", [], |r| r.get(0))
        .unwrap();
    drop(conn);
    let wallet: [u8; 32] = id.try_into().unwrap();
    crate::retire_snapshot_for_wallet_removal(&f.net, &wallet).unwrap();
    assert!(present(&f, SHIELDED));
    (f, s)
}

#[test]
fn retired_snapshot_transient_io_fault_is_propagated() {
    let (f, s) = migrated_and_retired();
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| {
        *h.borrow_mut() = Some((s.dir().to_path_buf(), std::io::ErrorKind::Interrupted));
    });
    let res = upgrade_copy_present(&f.net, SHIELDED);
    crate::snapshot::SNAPSHOT_READ_FOR_TEST.with(|h| *h.borrow_mut() = None);
    let err = res.expect_err("transient I/O error on retired copy must propagate");
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
}

#[cfg(unix)]
#[test]
fn retired_snapshot_with_symlink_fifo_or_oversized_marker_does_not_verify() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::sync::mpsc;
    use std::time::Duration;

    let (f, s) = migrated_and_retired();

    // Non-private snapshot directory is rejected (#1184 F3).
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o770)).unwrap();
    assert!(
        !upgrade_copy_present(&f.net, SHIELDED).unwrap(),
        "group-writable retired snapshot dir must not verify"
    );
    fs::set_permissions(s.dir(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(upgrade_copy_present(&f.net, SHIELDED).unwrap());

    // Symlink at a listed store file is rejected rather than followed.
    let store_path = s.dir().join(SHIELDED);
    let outside = f.root.join("outside-shielded.sqlite");
    fs::rename(&store_path, &outside).unwrap();
    symlink(&outside, &store_path).unwrap();
    assert!(
        !upgrade_copy_present(&f.net, SHIELDED).unwrap(),
        "symlinked store file must not verify"
    );
    fs::remove_file(&store_path).unwrap();
    fs::rename(&outside, &store_path).unwrap();
    assert!(upgrade_copy_present(&f.net, SHIELDED).unwrap());

    // Oversized SNAPSHOT-COMPLETE is rejected cleanly.
    let marker_path = s.dir().join(crate::snapshot::MARKER);
    let original_marker = fs::read(&marker_path).unwrap();
    let mut huge = original_marker.clone();
    huge.resize(128 * 1024, b' ');
    fs::write(&marker_path, &huge).unwrap();
    assert!(
        !upgrade_copy_present(&f.net, SHIELDED).unwrap(),
        "oversized marker must not verify"
    );
    fs::write(&marker_path, &original_marker).unwrap();
    assert!(upgrade_copy_present(&f.net, SHIELDED).unwrap());

    // FIFO at SNAPSHOT-COMPLETE must fail immediately instead of blocking.
    fs::remove_file(&marker_path).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&marker_path)
            .status()
            .unwrap()
            .success()
    );
    let net = f.net.clone();
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let res = upgrade_copy_present(&net, SHIELDED);
        let _ = tx.send(res);
    });
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(res) => {
            handle.join().unwrap();
            assert!(!res.unwrap(), "FIFO marker must be rejected as absent");
        }
        Err(_) => {
            // Unblock a hanging open on the FIFO so cleanup does not stall.
            let _ = std::fs::OpenOptions::new().write(true).open(&marker_path);
            let _ = handle.join();
            panic!("upgrade_copy_present blocked on a FIFO at SNAPSHOT-COMPLETE");
        }
    }
}
