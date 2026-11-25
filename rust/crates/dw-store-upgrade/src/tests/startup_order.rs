//! The startup order lib.rs documents: reconcile straight after prepare,
//! before any store (vault, `app.sqlite`) is written. The snapshot
//! fingerprints all four stores, so a write in between makes reconcile refuse
//! the snapshot with the wallet file untouched.

use super::*;
use crate::error::StoreUpgradeError;
use crate::reconcile::{Outcome, reconcile_fork_store};
use crate::sql::{has_column, open_immutable, sha256_file};

fn column_present(f: &Fixture) -> bool {
    has_column(
        &open_immutable(&f.wallet()).unwrap(),
        crate::fork::CONTACTS,
        crate::fork::EXTERNAL_REF,
    )
    .unwrap()
}

#[test]
fn an_app_sqlite_write_between_prepare_and_reconcile_is_refused_unchanged() {
    let f = fixture("v0.3.0");
    let prepared = crate::prepare_with(&f.net, build(), now()).unwrap();
    let snapshot = prepared.snapshot.unwrap();
    exec(
        &f.net.join("app.sqlite"),
        "INSERT INTO settings_kv (scope, key, value) VALUES ('', 'written.after.prepare', '1')",
    );
    let wallet_sha = sha256_file(&f.wallet()).unwrap();
    let before = tree(&f.root);

    let err = reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap_err();
    assert!(
        matches!(&err, StoreUpgradeError::SnapshotInvalid { detail, .. }
            if detail == "taken of other data than the stores hold now"),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(sha256_file(&f.wallet()).unwrap(), wallet_sha);
    assert_eq!(tree_diff(&tree(&f.root), &before), Vec::<String>::new());
    assert!(column_present(&f), "R1's drop did not run");
    assert_eq!(
        crate::snapshot::completed(&f.net).len(),
        1,
        "not silently re-snapshotted"
    );

    let next = crate::prepare_with(&f.net, build(), later(60))
        .unwrap()
        .snapshot
        .unwrap();
    assert_ne!(next.dir(), snapshot.dir(), "the next start snapshots anew");
    assert!(matches!(
        reconcile_fork_store(&config(&f.net), &next, build()).unwrap(),
        Outcome::Pending { .. }
    ));
}

#[test]
fn reconcile_straight_after_prepare_proceeds() {
    let f = fixture("v0.3.0");
    let snapshot = crate::prepare_with(&f.net, build(), now())
        .unwrap()
        .snapshot
        .unwrap();
    assert!(column_present(&f));
    assert_eq!(
        reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap(),
        Outcome::Pending {
            versions: [1, 2, 3].into_iter().collect(),
            notice: None,
        }
    );
    assert!(!column_present(&f), "R1's drop ran");
}

#[test]
fn the_sweep_removes_a_crashed_probe_dir_only_under_the_lock() {
    let tmp = private_tempdir();
    let root = tmp.path();
    let probe = root.join(format!("{}crashed", crate::adopters::PROBE_PREFIX));
    fs::create_dir_all(probe.join("backups/auto")).unwrap();
    fs::write(probe.join("wallet.sqlite"), b"probe store").unwrap();
    let unrelated = root.join(".store-upgrade-unrelated");
    fs::create_dir(&unrelated).unwrap();
    fs::create_dir(root.join("regtest")).unwrap();

    let other_process = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(dw_fs::instance::InstanceLock::FILE))
        .unwrap();
    other_process.lock().unwrap();
    let err = crate::sweep_stale_scratch(root).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    assert!(probe.exists(), "not swept while another process holds it");
    drop(other_process);

    assert_eq!(crate::sweep_stale_scratch(root).unwrap(), 1);
    assert!(!probe.exists());
    assert!(unrelated.is_dir() && root.join("regtest").is_dir());

    let probed = crate::adopters::Adopters::probe(root);
    assert!(
        !probed.observations().is_empty(),
        "the probe ran in its directory: {:?}",
        probed.p9a_status()
    );
    assert_eq!(
        crate::sweep_stale_scratch(root).unwrap(),
        0,
        "this process's probe removed its own directory"
    );
}
