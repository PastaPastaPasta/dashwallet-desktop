use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

use super::order::{Step, steps};
use super::*;
use crate::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};

/// One error of every `StoreUpgradeError` variant this crate's build has.
fn every_variant(dir: &Path) -> Vec<StoreUpgradeError> {
    use StoreUpgradeError as E;
    let io = || std::io::Error::other("disk full");
    vec![
        E::ForkStoreUnsupported {
            versions: [1, 6].into(),
        },
        E::ForkStoreShapeMismatch {
            object: "core_address_pool_retired".into(),
        },
        E::RetiredPoolWithoutAdopter { rows: 1 },
        E::P9aBroken {
            detail: "control FAILED".into(),
        },
        E::UpgradeRehearsalFailed {
            detail: "upstream open".into(),
        },
        E::PreHistoryFork,
        E::SchemaTooNew {
            found: 99,
            supported: 31,
        },
        E::SchemaHistoryMismatch {
            version: 1,
            detail: "checksum".into(),
        },
        E::SnapshotInvalid {
            dir: dir.into(),
            detail: "x".into(),
        },
        E::SnapshotFailed { detail: "x".into() },
        E::RestorePending { dir: dir.into() },
        E::StoreHeld { path: dir.into() },
        E::NonStrictOpen {
            policy: "Recovery".into(),
        },
        E::OpenConfigUnsupported { detail: "x".into() },
        E::ProbeNotRun,
        E::PersisterOpen {
            dir: dir.into(),
            source: Box::new(
                platform_wallet_storage::WalletStorageError::SchemaVersionUnsupported {
                    found: 99,
                    max_supported: 31,
                },
            ),
        },
        E::Sqlite {
            context: "commit the reconcile",
            unchanged: false,
            source: rusqlite::Error::InvalidQuery,
        },
        E::Io {
            context: "rehearse the upgrade on a copy",
            unchanged: true,
            source: io(),
        },
    ]
}

/// Every variant is in [`every_variant`] once: [`variant_name`]'s match is
/// exhaustive, so a new variant fails to compile until it is named there.
#[test]
fn every_variant_is_listed_once() {
    let dir = dw_testutil::private_tempdir();
    let mut names: Vec<&str> = every_variant(dir.path()).iter().map(variant_name).collect();
    let listed = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), listed, "a variant is listed twice");
    assert_eq!(listed, 18, "a variant is missing from every_variant");
}

/// Q98 98-2: every refusal has the store upgrade's own screen, with and
/// without the backup sentence. No generic screen stands in: this stays red
/// until dw-store-upgrade's `screen_for` covers every refusal.
#[test]
fn every_refusal_has_a_screen() {
    let dir = dw_testutil::private_tempdir();
    let mut missing = Vec::new();
    for e in every_variant(dir.path()) {
        if !e.is_refusal() {
            continue;
        }
        for backup_present in [true, false] {
            if screen_for(&e, backup_present).is_none() {
                missing.push(format!(
                    "{} (backup_present={backup_present})",
                    variant_name(&e)
                ));
            }
        }
    }
    assert!(missing.is_empty(), "refusals without a screen: {missing:?}");
}

/// Each crate error's engine error: a refusal with a screen as
/// `storage_refused`, one without as `internal` naming it (never a generic
/// screen); a held store as `storage_in_use`; the machine's failures (the
/// rehearsal's I/O included) as retryable `io`; upstream's open error by
/// its own mapping; the rest as `internal`.
#[test]
fn crate_errors_map_to_their_engine_errors() {
    let dir = dw_testutil::private_tempdir();
    for e in every_variant(dir.path()) {
        let name = variant_name(&e);
        let refusal = e.is_refusal();
        let screen = screen_for(&e, false);
        let mapped = map_error(dir.path(), StoreUpgradeStage::Prepare, e);
        match (&mapped, name) {
            (EngineError::StoreRefused(r), _) => {
                assert!(refusal, "{name}");
                assert_eq!(Some(&r.screen), screen.as_ref(), "{name}");
                assert_eq!((r.cause, r.folder.as_path()), (name, dir.path()));
            }
            (EngineError::Internal(m), _) if refusal => {
                assert!(screen.is_none(), "{name}");
                assert_eq!(m, &format!("store refusal without a screen: {name}"));
            }
            (EngineError::StorageInUse(_), "StoreHeld") => {}
            (EngineError::RetryableIo(_), "SnapshotFailed" | "Sqlite" | "Io") => {}
            (EngineError::StorageTooNew(_), "PersisterOpen") => {}
            (
                EngineError::Internal(m),
                "NonStrictOpen"
                | "ProbeNotRun"
                | "OpenConfigUnsupported"
                | "SnapshotInvalid"
                | "RestorePending",
            ) => assert!(m.contains(name), "{m}"),
            (other, _) => panic!("{name}: {other:?}"),
        }
    }
}

/// The backup sentence only for a verified snapshot (98-5): an incomplete
/// or torn `before-upgrade-*` directory reads as no backup.
#[test]
fn a_refusal_screen_has_the_backup_sentence_only_for_a_verified_snapshot() {
    let refused = |dir: &Path| match map_error(
        dir,
        StoreUpgradeStage::Prepare,
        StoreUpgradeError::RetiredPoolWithoutAdopter { rows: 1 },
    ) {
        EngineError::StoreRefused(r) => r.screen.text_key,
        other => panic!("{other:?}"),
    };
    let dir = dw_testutil::private_tempdir();
    assert_eq!(
        refused(dir.path()),
        "storeUpgrade.cantOpen.textWithoutBackup"
    );
    let torn = dir
        .path()
        .join("upgrade-backups/before-upgrade-20261125T000000Z");
    fs::create_dir_all(&torn).unwrap();
    fs::write(
        torn.join("SNAPSHOT-COMPLETE"),
        b"dw-store-upgrade snapshot\nformat 1\n",
    )
    .unwrap();
    assert_eq!(
        refused(dir.path()),
        "storeUpgrade.cantOpen.textWithoutBackup"
    );
}

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
}

fn engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NoEvents),
    )
    .unwrap()
}

fn open(engine: &Engine) -> Result<Arc<crate::NetworkSession>, EngineError> {
    engine.block_on(engine.open_network(
        DashNetwork::Regtest,
        SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        },
    ))
}

fn copy_tree(src: &Path, dest: &Path) {
    fs::create_dir(dest).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let to = dest.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to);
        } else {
            fs::copy(entry.path(), &to).unwrap();
        }
    }
}

/// A 0700 data root holding a copy of a released fixture's `regtest`.
fn fixture_root(version: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = dw_testutil::private_tempdir();
    let root = tmp.path().join("data");
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/upgrade")
            .join(version)
            .join("data"),
        &root,
    );
    (tmp, root)
}

/// Q98 98-4: the open's order is instance lock → sweep → prepare →
/// reconcile → note_stores_opening → vault → app.sqlite → persister, for a
/// fork file the reconcile writes. A vault, app.sqlite or store open moved
/// ahead of the reconcile fails here; a write between prepare and reconcile
/// also changes the four-store fingerprint, and the reconcile refuses.
#[test]
fn the_open_runs_the_store_upgrade_before_any_store_opens() {
    let (_tmp, root) = fixture_root("v0.1.0");
    let engine = engine(&root);
    let session = open(&engine).expect("a v0.1.0 fork file upgrades and opens");
    assert_eq!(
        steps(&root),
        [
            Step::Instance,
            Step::Sweep,
            Step::Prepare,
            Step::Reconcile,
            Step::StoresOpening,
            Step::Vault,
            Step::AppDb,
            Step::Persister,
        ]
    );
    assert!(
        dw_store_upgrade::latest_snapshot(session.data_dir())
            .unwrap()
            .is_some()
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();

    let fresh = dw_testutil::private_tempdir();
    let engine = self::engine(fresh.path());
    drop(open(&engine).expect("a fresh directory opens"));
    assert_eq!(
        steps(fresh.path()),
        [
            Step::Instance,
            Step::Sweep,
            Step::Prepare,
            Step::StoresOpening,
            Step::Vault,
            Step::AppDb,
            Step::Persister,
        ]
    );
    engine.block_on(engine.shutdown()).unwrap();
}
