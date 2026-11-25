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
