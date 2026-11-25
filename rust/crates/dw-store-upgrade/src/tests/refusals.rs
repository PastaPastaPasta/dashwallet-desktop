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

/// A store the snapshot reads that SQLite reads as no database is refused
/// with the damaged-store screen, not retried: a retry fails the same way.
#[test]
fn a_damaged_store_stops_the_snapshot_with_a_refusal() {
    let f = fixture("v0.1.0");
    super::copy_table::damage_header(&f, "dispatch.sqlite");
    for _ in 0..2 {
        crate::token::reset_opened_for_test();
        let err = crate::prepare_with(&f.net, build(), now()).unwrap_err();
        assert!(
            matches!(&err, StoreUpgradeError::SnapshotSourceDamaged { store, .. } if store == "dispatch.sqlite"),
            "{err:?}"
        );
        assert!(err.is_refusal() && err.file_unchanged());
        assert!(crate::copy::retry_copy_for(&err).is_none());
        assert!(
            !f.net.join(crate::snapshot::UPGRADE_BACKUPS_DIR).exists(),
            "refused reading the live stores, before any copy"
        );
        for state in [
            crate::copy::ProvedState::default(),
            crate::copy::ProvedState::read(&f.net, false),
        ] {
            super::copy_table::assert_damaged_screen(&err, state);
        }
    }
}

/// A store whose tables read but whose index is damaged gets past the live
/// read; its copy fails the integrity check, the store fails it too, and
/// the start is refused with the damaged-store screen.
#[test]
fn a_damaged_index_found_by_the_copy_is_refused_with_the_damaged_screen() {
    let f = fixture("v0.1.0");
    let dispatch = f.net.join("dispatch.sqlite");
    let (root, page_size): (i64, i64) = {
        let conn = rusqlite::Connection::open(&dispatch).unwrap();
        let root = conn
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE type = 'index' AND name = 'step_log_artifact'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let page_size = conn
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .unwrap();
        (root, page_size)
    };
    let mut bytes = fs::read(&dispatch).unwrap();
    bytes[((root - 1) * page_size) as usize] = 0;
    fs::write(&dispatch, bytes).unwrap();
    let err = crate::prepare_with(&f.net, build(), now()).unwrap_err();
    assert!(
        matches!(&err, StoreUpgradeError::SnapshotSourceDamaged { store, detail }
            if store == "dispatch.sqlite" && detail == "integrity_check of the store and its copy failed"),
        "{err:?}"
    );
    let backups = f.net.join(crate::snapshot::UPGRADE_BACKUPS_DIR);
    assert!(
        fs::read_dir(&backups)
            .unwrap()
            .any(|d| d.unwrap().path().join("dispatch.sqlite").exists()),
        "refused by the copy"
    );
    assert!(crate::copy::retry_copy_for(&err).is_none());
    for state in [
        crate::copy::ProvedState::default(),
        crate::copy::ProvedState::read(&f.net, false),
    ] {
        super::copy_table::assert_damaged_screen(&err, state);
    }
}

/// A copied `wallet.sqlite` that reads as no database once taken is
/// refused with the damaged-store screen.
#[test]
fn a_copy_read_as_damaged_once_taken_is_refused_with_the_damaged_screen() {
    let f = fixture("v0.1.0");
    crate::fsx::arm_fault("snapshot.copy_read", 0);
    let err = crate::prepare_with(&f.net, build(), now()).unwrap_err();
    crate::fsx::disarm_fault();
    assert!(
        matches!(&err, StoreUpgradeError::SnapshotSourceDamaged { store, detail }
            if store == "wallet.sqlite" && detail.starts_with("its copy: ")),
        "{err:?}"
    );
    assert!(crate::copy::retry_copy_for(&err).is_none());
    for state in [
        crate::copy::ProvedState::default(),
        crate::copy::ProvedState::read(&f.net, false),
    ] {
        super::copy_table::assert_damaged_screen(&err, state);
    }
}

/// A copy that fails its integrity check while its source is sound is the
/// machine's fault: retried, never a refusal.
#[test]
fn a_copy_that_fails_its_check_beside_a_sound_store_is_retried() {
    let f = fixture("v0.1.0");
    crate::fsx::arm_fault("copy.integrity", 0);
    let err = crate::prepare_with(&f.net, build(), now()).unwrap_err();
    crate::fsx::disarm_fault();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotFailed { .. }),
        "{err:?}"
    );
    assert!(crate::copy::retry_copy_for(&err).is_some());
}
