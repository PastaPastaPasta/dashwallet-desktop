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
            versions: [1, 2, 3].into_iter().collect()
        }
    );
    assert!(!column_present(&f), "R1's drop ran");
}
