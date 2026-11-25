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
        E::RestoreRefused {
            cause: RefusalCause::GarbledMarker,
        },
        E::RestoreNoSpace {
            needed: Some(1 << 31),
        },
        E::RestoreIncomplete {
            cause: IncompleteCause::PermissionDenied,
            journal_written: true,
            detail: "rename".into(),
        },
        E::RestoreAfterOpen,
        E::UpgradeCopyUnwritable {
            detail: "readonly".into(),
        },
        E::SnapshotSourceDamaged {
            store: "wallet.sqlite".into(),
            detail: "corrupt".into(),
        },
        E::WalletFileUnsupported {
            detail: "is a symbolic link".into(),
        },
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
    assert_eq!(listed, 25, "a variant is missing from every_variant");
}

/// Every state the engine may prove of a refused folder.
fn proved_states() -> Vec<ProvedState> {
    use dw_store_upgrade::copy::BackupState as B;
    let mut out = Vec::new();
    for migrated in [None, Some(false), Some(true)] {
        for backup in [None, Some(B::None), Some(B::Unverified), Some(B::Verified)] {
            for unchanged in [None, Some(false), Some(true)] {
                out.push(ProvedState {
                    migrated,
                    backup,
                    unchanged,
                });
            }
        }
    }
    out
}

/// Q98 98-2: every refusal has the store upgrade's own screen, whatever the
/// folder's proved state. No generic screen stands in: this stays red
/// until dw-store-upgrade's `screen_for` covers every refusal.
#[test]
fn every_refusal_has_a_screen() {
    let dir = dw_testutil::private_tempdir();
    let mut missing = Vec::new();
    for e in every_variant(dir.path()) {
        if !e.is_refusal() {
            continue;
        }
        for state in proved_states() {
            if screen_for(&e, state).is_none() {
                missing.push(format!("{} ({state:?})", variant_name(&e)));
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
        let screen = screen_for(&e, ProvedState::read(dir.path(), false));
        let mapped = map_error(dir.path(), StoreUpgradeStage::Prepare, e, false);
        match (&mapped, name) {
            (EngineError::StoreRefused(r), _) if refusal => {
                assert_eq!(Some(&r.screen), screen.as_ref(), "{name}");
                assert_eq!((r.cause, r.folder.as_path()), (name, dir.path()));
            }
            (EngineError::StoreRestore(r), "RestoreRefused") => assert_eq!(
                (r.state, r.cause, r.needed_bytes),
                (RestoreState::Refused, "garbledMarker", None)
            ),
            (EngineError::StoreRestore(r), "RestoreNoSpace") => assert_eq!(
                (r.state, r.cause, r.needed_bytes),
                (RestoreState::NoSpace, "diskFull", Some(1 << 31))
            ),
            (EngineError::StoreRestore(r), "RestoreIncomplete") => assert_eq!(
                (r.state, r.cause, r.needed_bytes, r.folder.as_path()),
                (
                    RestoreState::Incomplete,
                    "permissionDenied",
                    None,
                    dir.path()
                )
            ),
            (EngineError::Internal(m), _) if refusal => {
                assert!(screen.is_none(), "{name}");
                assert_eq!(m, &format!("{NO_SCREEN}{name}"));
            }
            (
                EngineError::StorageInUse {
                    retryable: true, ..
                },
                "StoreHeld",
            ) => {}
            (EngineError::RetryableIo(_), "SnapshotFailed" | "Sqlite" | "Io") => {}
            (EngineError::StorageTooNew(_), "PersisterOpen") => {}
            (
                EngineError::Internal(m),
                "NonStrictOpen"
                | "ProbeNotRun"
                | "OpenConfigUnsupported"
                | "SnapshotInvalid"
                | "RestorePending"
                | "RestoreAfterOpen",
            ) => assert!(m.contains(name), "{m}"),
            (other, _) => panic!("{name}: {other:?}"),
        }
    }
}

/// A wallet file that isn't a database, or is corrupt, is `storage` (no
/// retry helps); any other SQLite failure stays retryable `io`.
#[test]
fn a_damaged_wallet_file_is_storage_not_retryable_io() {
    let dir = dw_testutil::private_tempdir();
    let sqlite = |code| StoreUpgradeError::Sqlite {
        context: "read the wallet file's tables",
        unchanged: true,
        source: rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None),
    };
    let map = |e| map_error(dir.path(), StoreUpgradeStage::Prepare, e, false);
    for code in [rusqlite::ffi::SQLITE_NOTADB, rusqlite::ffi::SQLITE_CORRUPT] {
        assert!(
            matches!(map(sqlite(code)), EngineError::Storage(_)),
            "{code}"
        );
    }
    assert!(matches!(
        map(sqlite(rusqlite::ffi::SQLITE_IOERR)),
        EngineError::RetryableIo(_)
    ));
}

/// The way out a refusal of `dir` offers, and whether it has Restore earlier
/// data.
fn way_out(dir: &Path, e: StoreUpgradeError) -> (dw_store_upgrade::copy::WayOut, bool) {
    match map_error(dir, StoreUpgradeStage::Prepare, e, false) {
        EngineError::StoreRefused(r) => (
            r.screen.way_out,
            (r.screen.buttons.iter()).any(|b| b.key == "storeUpgrade.refused.restore"),
        ),
        other => panic!("{other:?}"),
    }
}

/// No backup is claimed without a verified snapshot (98-5): an incomplete
/// or torn `before-upgrade-*` directory reads as no backup, and no screen
/// offers Restore earlier data for it.
#[test]
fn a_refusal_offers_no_restore_without_a_verified_snapshot() {
    use dw_store_upgrade::copy::WayOut;
    let shape = || StoreUpgradeError::ForkStoreShapeMismatch {
        object: "contacts".into(),
    };
    let dir = dw_testutil::private_tempdir();
    assert_eq!(way_out(dir.path(), shape()), (WayOut::Support, false));
    let torn = dir
        .path()
        .join("upgrade-backups/before-upgrade-20261125T000000Z");
    fs::create_dir_all(&torn).unwrap();
    fs::write(
        torn.join("SNAPSHOT-COMPLETE"),
        b"dw-store-upgrade snapshot\nformat 1\n",
    )
    .unwrap();
    assert_eq!(way_out(dir.path(), shape()), (WayOut::Support, false));
}

/// `wrote`: a refusal after this start changed the files (a restore it
/// finished) says Dash Wallet had started changing them, never that it
/// changed nothing.
#[test]
fn a_refusal_after_this_start_wrote_never_says_nothing_changed() {
    let dir = dw_testutil::private_tempdir();
    let unchanged = |wrote| match map_error(
        dir.path(),
        StoreUpgradeStage::Prepare,
        StoreUpgradeError::RetiredPoolWithoutAdopter { rows: 1 },
        wrote,
    ) {
        EngineError::StoreRefused(r) => r.screen.unchanged.key,
        other => panic!("{other:?}"),
    };
    assert_eq!(unchanged(false), "storeUpgrade.refused.keep");
    assert_eq!(unchanged(true), "storeUpgrade.refused.changedByUpgrade");
}

/// Every error with a restore screen, and the state its screen is for.
fn restore_errors() -> Vec<StoreUpgradeError> {
    use StoreUpgradeError as E;
    let mut v: Vec<E> = [
        RefusalCause::MissingBackup,
        RefusalCause::MismatchedBackup,
        RefusalCause::GarbledMarker,
    ]
    .into_iter()
    .map(|cause| E::RestoreRefused { cause })
    .collect();
    v.push(E::RestoreNoSpace { needed: Some(7) });
    v.push(E::RestoreNoSpace { needed: None });
    for cause in [
        IncompleteCause::DiskFull,
        IncompleteCause::PermissionDenied,
        IncompleteCause::FileVanished,
        IncompleteCause::S4OldBuildWrote,
        IncompleteCause::S4OtherLineage,
        IncompleteCause::Other,
    ] {
        for journal_written in [false, true] {
            v.push(E::RestoreIncomplete {
                cause,
                journal_written,
                detail: "x".into(),
            });
        }
    }
    v
}

fn state_key(s: RestoreState) -> &'static str {
    match s {
        RestoreState::Refused => "refused",
        RestoreState::NoSpace => "noSpace",
        RestoreState::NotStarted => "notStarted",
        RestoreState::Incomplete => "incomplete",
        RestoreState::Foreign => "foreign",
    }
}

/// Every restore screen (`copy::all_screens`, less the confirm the renderer
/// opens itself) is reached, each as `storage_restore` with the state its
/// keys name: the screen is `copy::restore_screen_for`'s, the foreign
/// lineage's included. Causes are the DTO's.
#[test]
fn every_restore_screen_is_storage_restore_by_its_state() {
    use dw_store_upgrade::copy::{all_screens, restore_confirm, restore_screen_for};
    let dir = dw_testutil::private_tempdir();
    let mut reached = Vec::new();
    for e in restore_errors() {
        let screen = restore_screen_for(&e).expect("a restore screen");
        let detail = format!("{e:?}");
        match map_error(dir.path(), StoreUpgradeStage::Restore, e, false) {
            EngineError::StoreRestore(r) => {
                let state = state_key(r.state);
                assert_eq!(
                    screen.title_key,
                    format!("storeUpgrade.restore.{state}.title"),
                    "{detail}"
                );
                let causes: &[&str] = match r.state {
                    RestoreState::Refused => {
                        &["missingBackup", "mismatchedBackup", "garbledMarker"]
                    }
                    RestoreState::NoSpace => &["diskFull"],
                    RestoreState::Foreign => &["otherLineage", "oldBuildWrote"],
                    _ => &["diskFull", "permissionDenied", "fileVanished", "other"],
                };
                assert!(causes.contains(&r.cause), "{detail}: {}", r.cause);
                assert_eq!(r.folder, dir.path());
                if !reached.contains(&screen) {
                    reached.push(screen);
                }
            }
            other => panic!("{detail}: {other:?}"),
        }
    }
    let mut every = all_screens();
    every.retain(|s| *s != restore_confirm());
    assert_eq!(reached.len(), every.len());
    for s in every {
        assert!(reached.contains(&s), "{} is never reached", s.title_key);
    }
    assert!(matches!(
        map_error(
            dir.path(),
            StoreUpgradeStage::Restore,
            StoreUpgradeError::RestoreIncomplete {
                cause: IncompleteCause::S4OtherLineage,
                journal_written: true,
                detail: "x".into(),
            },
            false
        ),
        EngineError::StoreRestore(r) if r.state == RestoreState::Foreign && r.cause == "otherLineage"
    ));
}

/// A store the restore step finds held is held until a restart; the
/// persister's own open keeps its retryable `storage_in_use`.
#[test]
fn a_store_the_restore_finds_held_needs_a_restart() {
    let dir = dw_testutil::private_tempdir();
    let held = || StoreUpgradeError::StoreHeld {
        path: dir.path().into(),
    };
    for (stage, retry) in [
        (StoreUpgradeStage::Restore, false),
        (StoreUpgradeStage::Open, true),
    ] {
        match map_error(dir.path(), stage, held(), false) {
            EngineError::StorageInUse { retryable, .. } => {
                assert_eq!(retryable, retry, "{stage:?}")
            }
            other => panic!("{other:?}"),
        }
    }
}

/// Restore earlier data is asked for only with a verified snapshot: with
/// none, or an unverified one, `storage_restore` refused (missingBackup) and
/// nothing is written.
#[test]
fn a_restore_request_without_a_verified_snapshot_is_refused() {
    let dir = dw_testutil::private_tempdir();
    let refused = |dir: &Path| match request_restore(dir) {
        Err(EngineError::StoreRestore(r)) => (r.state, r.cause),
        other => panic!("{other:?}"),
    };
    assert!(!restore_available(dir.path()).unwrap());
    assert_eq!(
        refused(dir.path()),
        (RestoreState::Refused, "missingBackup")
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
    assert!(!restore_available(dir.path()).unwrap());
    assert_eq!(
        refused(dir.path()),
        (RestoreState::Refused, "missingBackup")
    );
    assert_eq!(dw_store_upgrade::pending_restore(dir.path()).unwrap(), None);
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

/// Q98 98-4: the open's order is instance lock → sweep →
/// run_pending_restore → prepare → reconcile → note_stores_opening → vault
/// → app.sqlite → persister, for a fork file the reconcile writes. The
/// restore runs before the refusal check (which refuses while one is
/// pending) and before any store opens (after which it refuses). A vault, app.sqlite or store open moved
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
            Step::Restore,
            Step::Prepare,
            Step::Reconcile,
            Step::StoresOpening,
            Step::Vault,
            Step::AppDb,
            Step::Persister,
        ]
    );
    let net = session.data_dir().to_path_buf();
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    assert!(dw_store_upgrade::latest_snapshot(&net).unwrap().is_some());

    use dw_store_upgrade::copy::WayOut;
    let shape = || StoreUpgradeError::ForkStoreShapeMismatch {
        object: "contacts".into(),
    };
    assert!(restore_available(&net).unwrap());
    assert_eq!(way_out(&net, shape()), (WayOut::Restore, true));
    let snapshot = dw_store_upgrade::latest_snapshot(&net).unwrap().unwrap();
    let torn = snapshot.dir().join(dw_store_upgrade::STORE_FILES[0]);
    fs::write(&torn, b"torn").unwrap();
    assert!(!restore_available(&net).unwrap());
    assert_eq!(way_out(&net, shape()), (WayOut::Unverified, false));

    let fresh = dw_testutil::private_tempdir();
    let engine = self::engine(fresh.path());
    drop(open(&engine).expect("a fresh directory opens"));
    assert_eq!(
        steps(fresh.path()),
        [
            Step::Instance,
            Step::Sweep,
            Step::Restore,
            Step::Prepare,
            Step::StoresOpening,
            Step::Vault,
            Step::AppDb,
            Step::Persister,
        ]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// A second engine on a root this process already swept shares the
/// instance lock, so it must not sweep: the copy a check of the first engine
/// is using stays.
#[test]
fn a_second_engine_on_a_swept_root_does_not_sweep() {
    let tmp = dw_testutil::private_tempdir();
    let stale = tmp.path().join(".store-upgrade-precheck-stale");
    fs::create_dir(&stale).unwrap();
    let first = engine(tmp.path());
    assert!(
        !stale.exists(),
        "the first engine sweeps a crashed check's copy"
    );
    let running = tmp.path().join(".store-upgrade-precheck-running");
    fs::create_dir(&running).unwrap();
    let second = engine(tmp.path());
    assert!(
        running.exists(),
        "the second engine swept a running check's copy"
    );
    assert_eq!(
        steps(tmp.path()),
        [Step::Instance, Step::Sweep, Step::Instance, Step::Sweep]
    );
    drop(second);
    first.block_on(first.shutdown()).unwrap();
}
