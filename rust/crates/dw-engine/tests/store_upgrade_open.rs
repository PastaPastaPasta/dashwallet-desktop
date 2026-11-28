//! The M3 store upgrade through the real `Engine::open_network`: every
//! refusal class leaves the network directory byte-identical (MG condition
//! B), an accepted fork file upgrades and opens, and a network closed
//! cleanly reopens in this process.

mod store_upgrade_common;

use dw_engine::{DashNetwork, EngineError, ImportOptions};
use store_upgrade_common::*;
use zeroize::Zeroizing;

#[test]
fn a_fork_version_set_no_build_published_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "INSERT INTO dw_fork_schema_history VALUES (6, 'later', 'x', '1')",
    );
    assert_eq!(open_refused(&f), "ForkStoreUnsupported");
}

/// A refused open leaves every existing file and directory of the folder
/// byte-identical, the folder's own mode included (nothing chmods it before
/// the refusal check); only `upgrade-backups/` and `backups/auto` may be new.
#[cfg(unix)]
#[test]
fn a_refused_open_keeps_existing_files_and_the_folder_mode() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "INSERT INTO dw_fork_schema_history VALUES (6, 'later', 'x', '1')",
    );
    std::fs::set_permissions(&f.net, std::fs::Permissions::from_mode(0o750)).unwrap();
    std::fs::set_permissions(f.wallet(), std::fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(open_refused(&f), "ForkStoreUnsupported");
}

/// A data root the user left group-writable is refused before the open
/// reads or writes anything below it: the error names the root and why, the
/// root keeps its mode, and nothing in it is created or changed but the
/// instance lock the engine takes as it starts.
#[cfg(unix)]
#[test]
fn a_group_writable_data_root_is_refused_with_nothing_written() {
    use std::os::unix::fs::PermissionsExt;
    let chmod = |p: &std::path::Path, m| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap()
    };
    let existing = fixture("v0.3.0");
    let fresh = dw_testutil::private_tempdir();
    let fresh_root = fresh.path().join("data");
    std::fs::create_dir(&fresh_root).unwrap();
    for root in [&existing.root, &fresh_root] {
        chmod(root, 0o775);
        let before = tree(root);
        let engine = engine(root);
        let err = engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .err()
            .expect("a group-writable data root is refused");
        match &err {
            EngineError::DataFolderNotPrivate { folder, reason, .. } => {
                assert_eq!(
                    std::fs::canonicalize(folder).unwrap(),
                    std::fs::canonicalize(root).unwrap(),
                    "{err}"
                );
                assert_eq!(*reason, dw_fs::InsecureReason::GroupWritable, "{err}");
            }
            other => panic!("not a data-folder refusal: {other:?}"),
        }
        assert_eq!(err.code(), "storage_not_private");
        let changed = tree_diff(&before, &tree(root));
        let lock = dw_fs::instance::InstanceLock::FILE;
        assert!(
            changed.iter().all(|p| p == lock) && !before.contains_key(lock),
            "the refused open changed {changed:?}"
        );
        engine.block_on(engine.shutdown()).unwrap();
        assert_eq!(
            std::fs::metadata(root).unwrap().permissions().mode() & 0o7777,
            0o775
        );
    }
    assert!(!fresh_root.join("regtest").exists());
    chmod(&fresh_root, 0o700);
    let engine = engine(&fresh_root);
    engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .expect("the same root opens once only the user can change it");
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn a_fork_object_of_another_shape_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "ALTER TABLE core_address_pool_retired ADD COLUMN extra INTEGER",
    );
    assert_eq!(open_refused(&f), "ForkStoreShapeMismatch");
}

#[test]
fn an_upgrade_that_fails_on_a_copy_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "INSERT INTO core_transactions (wallet_id, txid, finalized, record_blob) \
         SELECT wallet_id, randomblob(32), 0, x'00' FROM wallets LIMIT 1",
    );
    assert_eq!(open_refused(&f), "UpgradeRehearsalFailed");
}

#[test]
fn a_pre_history_dev_build_file_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "DROP TABLE dw_fork_schema_history; \
         INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (19, 'dashpay_external_rotation', 'x', '1')",
    );
    assert_eq!(open_refused(&f), "PreHistoryFork");
}

#[test]
fn a_fork_file_with_a_newer_schema_is_refused_unchanged() {
    let f = fixture("v0.3.0");
    exec(
        &f.wallet(),
        "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
         VALUES (999, 'from_a_newer_build', 'x', '1')",
    );
    assert_eq!(open_refused(&f), "SchemaTooNew");
}

#[test]
fn a_refinery_history_no_release_wrote_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    exec(
        &f.wallet(),
        "UPDATE refinery_schema_history SET checksum = '1' WHERE version = 1",
    );
    assert_eq!(open_refused(&f), "SchemaHistoryMismatch");
}

/// Each released fixture upgrades through the open: the pre-upgrade
/// snapshot is taken, R1's column is dropped, every wallet loads; the next
/// start retires the fork history and reuses the snapshot.
#[test]
fn every_released_fork_file_upgrades_through_the_open() {
    for version in ["v0.1.0", "v0.2.0", "v0.3.0"] {
        let f = fixture(version);
        let engine = engine(&f.root);
        let s = engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .unwrap_or_else(|e| panic!("{version}: {e:?}"));
        assert!(!s.wallet_infos().unwrap().is_empty(), "{version}");
        assert_eq!(s.upgrade_notice(), None, "{version}");
        drop(s);
        engine.block_on(engine.shutdown()).unwrap();
        drop(engine);
        let snapshot = dw_store_upgrade::latest_snapshot(&f.net)
            .unwrap()
            .expect("snapshot");

        let engine = self::engine(&f.root);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let s = loop {
            match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
                Ok(s) => break s,
                Err(EngineError::StorageInUse { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("{version} second start: {e:?}"),
            }
        };
        let conn = rusqlite::Connection::open_with_flags(
            f.wallet(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let retired: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'dw_fork_schema_history_retired')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            retired,
            "{version}: the second start retires the fork history"
        );
        drop(conn);
        drop(s);
        engine.block_on(engine.shutdown()).unwrap();
        assert_eq!(
            dw_store_upgrade::latest_snapshot(&f.net)
                .unwrap()
                .unwrap()
                .dir(),
            snapshot.dir(),
            "{version}"
        );
    }
}

/// Q107 107-1: the open of a between-state store whose before-upgrade
/// snapshot doesn't check out, or is of other data, carries the notice once
/// its wallets have loaded; one with a snapshot to restore, or none at all,
/// carries none, and so does the next start, past the between state.
#[test]
fn an_open_without_a_snapshot_to_restore_carries_its_notice() {
    use dw_store_upgrade::test_support::{BeforeSnapshot, between_state};
    for (before, key) in [
        (
            BeforeSnapshot::Unverified,
            Some("storeUpgrade.notice.snapshotDamaged"),
        ),
        (
            BeforeSnapshot::Mismatch,
            Some("storeUpgrade.notice.snapshotOther"),
        ),
        (BeforeSnapshot::Present, None),
        (BeforeSnapshot::Absent, None),
    ] {
        let tmp = dw_testutil::private_tempdir();
        let root = tmp.path().join("data");
        dw_testutil::private_dir(&root);
        between_state(
            &root.join("regtest"),
            before,
            false,
            std::time::SystemTime::now(),
        )
        .unwrap_or_else(|e| panic!("{before:?}: {e:?}"));

        let engine = self::engine(&root);
        let s = engine
            .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
            .unwrap_or_else(|e| panic!("{before:?}: {e:?}"));
        assert!(!s.wallet_infos().unwrap().is_empty(), "{before:?}");
        let notice = s.upgrade_notice();
        assert_eq!(notice.map(|l| l.key), key, "{before:?}");
        if let Some(line) = notice {
            assert!(line.text.contains("upgrade-backups"), "{before:?}");
        }
        assert_eq!(
            s.upgrade_notice(),
            notice,
            "{before:?}: the same all session"
        );
        drop(s);
        engine.block_on(engine.shutdown()).unwrap();
        drop(engine);

        let engine = self::engine(&root);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let s = loop {
            match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
                Ok(s) => break s,
                Err(EngineError::StorageInUse { .. }) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => panic!("{before:?} second start: {e:?}"),
            }
        };
        assert_eq!(s.upgrade_notice(), None, "{before:?} second start");
        drop(s);
        engine.block_on(engine.shutdown()).unwrap();
    }
}

/// A network closed cleanly with a wallet loaded reopens in the same
/// process: a held store's `storage_in_use` is retryable.
#[test]
fn a_clean_close_then_reopen_works_in_process() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Regtest, local_opts()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    engine
        .block_on(s.import_wallet(
            Zeroizing::new(
                "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
                    .as_bytes()
                    .to_vec(),
            ),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    drop(s);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let reopen = engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts()));
    assert!(reopen.is_ok(), "{:?}", reopen.as_ref().err());
    drop(reopen);
    engine.block_on(engine.shutdown()).unwrap();
}
