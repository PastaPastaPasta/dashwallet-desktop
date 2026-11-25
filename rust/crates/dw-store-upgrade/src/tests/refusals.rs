//! Every refusal writes, renames and locks nothing in `{folder}`: sha256 of
//! every file and the listing, before and after.

use super::*;
use crate::error::StoreUpgradeError;
use crate::precheck::{Precheck, precheck_locked_with, precheck_with};

/// Precheck (and the locked check when asked) refuses with `want`; the data
/// root and `{folder}` are byte-identical afterwards.
fn refused(f: &Fixture, want: fn(&StoreUpgradeError) -> bool) {
    let before = tree(&f.root);
    let mut found = precheck_with(&f.net, build()).unwrap();
    if matches!(found, Precheck::NeedsLockedCheck) {
        assert_eq!(tree(&f.root), before);
        found = precheck_locked_with(&f.net, build()).unwrap();
    }
    let Precheck::Refuse { reason } = found else {
        panic!("expected a refusal, got {found:?}");
    };
    assert!(want(&reason), "{reason:?}");
    assert!(reason.file_unchanged() && reason.is_refusal());
    assert_eq!(tree(&f.root), before, "the refusal changed the data root");
}

#[test]
fn refuses_unpublished_version_set_1_6() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "INSERT INTO dw_fork_schema_history VALUES (6, 'later', 'x', '1')",
    );
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreUnsupported { versions } if versions.iter().eq([1, 6].iter())),
    );
}

#[test]
fn refuses_fork_v4_without_its_adopter() {
    let f = fixture("v0.3.0");
    exec(
        &f.wallet(),
        "INSERT INTO dw_fork_schema_history VALUES (4, 'dismissed_txids', 'x', '1'); \
         CREATE TABLE core_dismissed_txids (wallet_id BLOB NOT NULL, txid BLOB NOT NULL)",
    );
    refused(&f, |e| {
        matches!(e, StoreUpgradeError::ForkStoreUnsupported { .. })
    });
}

#[test]
fn refuses_a_changed_column_type() {
    let f = fixture("v0.1.0");
    let conn = Connection::open(f.wallet()).unwrap();
    let sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'core_address_pool_retired'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let changed = sql.replacen(
        "address_index INTEGER NOT NULL",
        "address_index TEXT NOT NULL",
        1,
    );
    assert_ne!(sql, changed);
    conn.execute_batch(&format!(
        "DROP TABLE core_address_pool_retired; {changed}; \
         CREATE INDEX idx_core_address_pool_retired_script ON core_address_pool_retired(wallet_id, script);"
    ))
    .unwrap();
    drop(conn);
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { object } if object == "core_address_pool_retired"),
    );
}

#[test]
fn refuses_an_extra_column_on_the_retired_table() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "ALTER TABLE core_address_pool_retired ADD COLUMN extra INTEGER",
    );
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { object } if object == "core_address_pool_retired"),
    );
}

#[test]
fn refuses_a_missing_index() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "DROP INDEX idx_core_address_pool_retired_script",
    );
    refused(&f, |e| {
        matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { .. })
    });
}

#[test]
fn refuses_an_index_on_the_dropped_column() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "CREATE INDEX idx_ext ON contacts(external_account_reference)",
    );
    refused(&f, |e| {
        matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { .. })
    });
}

#[test]
fn refuses_a_view_naming_the_dropped_column() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "CREATE VIEW v_ext AS SELECT external_account_reference FROM contacts",
    );
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { object } if object.contains("view v_ext")),
    );
}

#[test]
fn refuses_pre_history_v19() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "DROP TABLE dw_fork_schema_history; \
         INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (19, 'dashpay_external_rotation', 'x', '1')",
    );
    refused(&f, |e| matches!(e, StoreUpgradeError::PreHistoryFork));
}

#[test]
fn refuses_a_schema_newer_than_this_build() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) VALUES (99, 'future', 'x', '1')",
    );
    refused(&f, |e| {
        matches!(e, StoreUpgradeError::SchemaTooNew { found: 99, .. })
    });
}

#[test]
fn refuses_a_wal_only_fork_v4_row_through_the_locked_check() {
    let f = fixture("v0.1.0");
    let wallet = f.wallet();
    let pristine = fs::read(&wallet).unwrap();
    let wal = f.net.join("wallet.sqlite-wal");
    {
        let conn = Connection::open(&wallet).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        conn.execute_batch(
            "PRAGMA wal_autocheckpoint = 0; \
             INSERT INTO dw_fork_schema_history VALUES (4, 'dismissed_txids', 'x', '1')",
        )
        .unwrap();
        let held = fs::read(&wal).unwrap();
        assert!(!held.is_empty());
        drop(conn);
        fs::write(&wallet, &pristine).unwrap();
        fs::write(&wal, held).unwrap();
        let _ = fs::remove_file(f.net.join("wallet.sqlite-shm"));
    }
    let main_only = crate::sql::open_immutable(&wallet).unwrap();
    let rows: i64 = main_only
        .query_row(
            "SELECT count(*) FROM dw_fork_schema_history WHERE version = 4",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 0, "the V4 row must be only in the WAL");
    drop(main_only);
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::NeedsLockedCheck
    ));
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreUnsupported { versions } if versions.contains(&4)),
    );
}

#[test]
fn reconcile_refusal_leaves_the_store_unchanged() {
    let f = fixture("v0.1.0");
    let snapshot = crate::snapshot::take_snapshot(&f.net, now()).unwrap();
    exec(
        &f.wallet(),
        "ALTER TABLE core_address_pool_retired ADD COLUMN extra INTEGER",
    );
    let before = tree(&f.net);
    let err =
        crate::reconcile::reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::ForkStoreShapeMismatch { .. }),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(tree(&f.net), before);
}

#[test]
fn refuses_an_unreleased_fork_table_no_upstream_migration_made() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        build().fresh_table_sql("core_block_spends").unwrap(),
    );
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { object } if object == "core_block_spends without fork version 5"),
    );
}

#[test]
fn refuses_an_upstream_made_table_that_is_not_this_builds() {
    let f = fixture("v0.1.0");
    let snapshot = crate::snapshot::take_snapshot(&f.net, now()).unwrap();
    crate::reconcile::reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    drop(open_and_load(&f.net));
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::Fork { .. }
    ));
    exec(
        &f.wallet(),
        "DROP TABLE core_dismissed_txids; \
         CREATE TABLE core_dismissed_txids (wallet_id BLOB NOT NULL, txid BLOB NOT NULL)",
    );
    refused(
        &f,
        |e| matches!(e, StoreUpgradeError::ForkStoreShapeMismatch { object } if object == "core_dismissed_txids without fork version 4"),
    );
}
