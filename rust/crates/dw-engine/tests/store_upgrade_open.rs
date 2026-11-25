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
