//! The funds review of the P9a commit: Strict-only opens (H1), a snapshot of
//! the data held under the write lock (M1), the probe gate on open (M2), the
//! data root (M3), the rehearsal on a copy (M4), and the lower items.

use super::*;
use crate::adopters::{FreshObject, P9aStatus};
use crate::error::StoreUpgradeError;
use crate::precheck::{Precheck, precheck_with};
use crate::reconcile::reconcile_fork_store;
use crate::snapshot::take_snapshot;
use crate::sql::{has_column, open_immutable, sha256_file};
use crate::token::PROBE_FOR_OPEN;

fn column_present(f: &Fixture) -> bool {
    let conn = open_immutable(&f.wallet()).unwrap();
    has_column(&conn, "contacts", "external_account_reference").unwrap()
}

#[test]
fn non_strict_open_is_refused_and_the_file_is_unchanged() {
    let f = fixture("v0.1.0");
    let before = tree(&f.root);
    let wallet_sha = sha256_file(&f.wallet()).unwrap();
    let err = PersisterOpened::open(config(&f.net).with_load_policy(LoadPolicy::Recovery))
        .err()
        .expect("Recovery is refused");
    assert!(
        matches!(err, StoreUpgradeError::NonStrictOpen { .. }),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert_eq!(sha256_file(&f.wallet()).unwrap(), wallet_sha);
    assert_eq!(tree(&f.root), before);
    assert!(!crate::token::is_held(&f.net));
}

#[test]
fn reconcile_refuses_a_snapshot_of_other_data() {
    let f = fixture("v0.1.0");
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    exec(
        &f.wallet(),
        "DELETE FROM core_utxos WHERE rowid = (SELECT min(rowid) FROM core_utxos); \
         UPDATE core_utxos SET spent = 1 - spent",
    );
    let before = tree(&f.net);
    let err = reconcile_fork_store(&f.wallet(), &snapshot, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotInvalid { .. }),
        "{err:?}"
    );
    assert!(err.file_unchanged());
    assert!(column_present(&f), "no drop against a stale snapshot");
    assert_eq!(tree(&f.net), before);
}

#[test]
fn snapshot_fingerprint_covers_every_store() {
    let f = fixture("v0.1.0");
    let first = take_snapshot(&f.net, now()).unwrap();
    exec(
        &f.net.join("dispatch.sqlite"),
        "CREATE TABLE dwsu_changed (a INTEGER); INSERT INTO dwsu_changed VALUES (1)",
    );
    let second = take_snapshot(&f.net, later(1)).unwrap();
    assert_ne!(first.dir(), second.dir(), "dispatch.sqlite changed");
    let before = tree(&f.net);
    let err = reconcile_fork_store(&f.wallet(), &first, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::SnapshotInvalid { .. }),
        "{err:?}"
    );
    let after = tree(&f.net);
    assert!(after == before, "{:?}", tree_diff(&before, &after));
    assert!(column_present(&f));
}

fn open_with_probe(f: &Fixture, probe: Option<P9aStatus>) -> StoreUpgradeError {
    PROBE_FOR_OPEN.with(|p| *p.borrow_mut() = Some(probe));
    let result = PersisterOpened::open(config(&f.net));
    PROBE_FOR_OPEN.with(|p| *p.borrow_mut() = None);
    result.err().expect("refused")
}

#[test]
fn open_refuses_when_the_probe_is_missing_or_broken() {
    let f = fixture("v0.1.0");
    let before = tree(&f.root);
    let err = open_with_probe(&f, None);
    assert!(matches!(err, StoreUpgradeError::ProbeNotRun), "{err:?}");
    let err = open_with_probe(
        &f,
        Some(P9aStatus::Broken {
            detail: "test".into(),
        }),
    );
    assert!(
        matches!(err, StoreUpgradeError::P9aBroken { .. }),
        "{err:?}"
    );
    assert_eq!(tree(&f.root), before);
}

#[test]
fn dropping_the_token_early_keeps_the_store_held() {
    let f = fixture("v0.1.0");
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    reconcile_fork_store(&f.wallet(), &snapshot, build()).unwrap();
    let (persister, token) = open_and_load(&f.net);
    drop(token);
    assert!(crate::token::is_held(&f.net));
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap_err(),
        StoreUpgradeError::StoreHeld { .. }
    ));
    let clone = std::sync::Arc::clone(&persister);
    drop(persister);
    assert!(crate::token::is_held(&f.net), "a clone still holds it");
    drop(clone);
    assert!(!crate::token::is_held(&f.net));
}

#[test]
fn the_probe_creates_a_missing_data_root_private() {
    let tmp = private_tempdir();
    let root = tmp.path().join("fresh-install").join("data");
    let adopters = crate::adopters::Adopters::probe_new_root(&root);
    assert!(root.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert!(
        !matches!(adopters.p9a_status(), P9aStatus::Broken { .. }),
        "{:?}",
        adopters.p9a_status()
    );
}

#[test]
fn a_directory_without_a_parent_is_an_error_never_the_working_directory() {
    for path in ["wallet.sqlite", "regtest"] {
        let err = crate::precheck::parent_dir(Path::new(path)).unwrap_err();
        assert!(err.to_string().contains(path), "{err}");
    }
    assert!(crate::precheck::precheck(Path::new("regtest")).is_err());
    assert!(crate::prepare(Path::new("regtest"), now()).is_err());
}

/// A historical transaction upstream's V021 cannot decode under Strict.
fn add_undecodable_record(path: &Path) {
    exec(
        path,
        "INSERT INTO core_transactions (wallet_id, txid, finalized, record_blob) \
         SELECT wallet_id, randomblob(32), 0, x'00' FROM wallets LIMIT 1",
    );
}

#[test]
fn an_upgrade_that_fails_on_a_copy_is_refused_unchanged() {
    let f = fixture("v0.1.0");
    add_undecodable_record(&f.wallet());
    let before = tree(&f.root);
    let found = precheck_with(&f.net, build()).unwrap();
    let Precheck::Refuse { reason } = found else {
        panic!("expected a refusal, got {found:?}");
    };
    assert!(
        matches!(reason, StoreUpgradeError::UpgradeRehearsalFailed { .. }),
        "{reason:?}"
    );
    assert!(reason.file_unchanged() && reason.is_refusal());
    assert_eq!(tree(&f.root), before, "the rehearsal changed the data root");
    let prepared = crate::prepare_with(&f.net, build(), now()).unwrap();
    assert!(prepared.snapshot.is_none(), "refused before the snapshot");
    assert_eq!(tree(&f.root), before);
}

#[test]
fn reconcile_rehearses_before_the_drop() {
    let f = fixture("v0.1.0");
    add_undecodable_record(&f.wallet());
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    let before = tree(&f.net);
    let err = reconcile_fork_store(&f.wallet(), &snapshot, build()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::UpgradeRehearsalFailed { .. }),
        "{err:?}"
    );
    assert!(
        column_present(&f),
        "the real file is never left part-upgraded"
    );
    assert_eq!(tree(&f.net), before);
}

/// Leave `-wal` holding a change the main file lacks, as a crash does.
fn hot_wal(f: &Fixture) {
    let wallet = f.wallet();
    let pristine = fs::read(&wallet).unwrap();
    let wal = f.net.join("wallet.sqlite-wal");
    let conn = Connection::open(&wallet).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    conn.execute_batch(
        "PRAGMA wal_autocheckpoint = 0; \
         CREATE TABLE dwsu_hot (a INTEGER); INSERT INTO dwsu_hot VALUES (42)",
    )
    .unwrap();
    let held = fs::read(&wal).unwrap();
    drop(conn);
    fs::write(&wallet, &pristine).unwrap();
    fs::write(&wal, held).unwrap();
    let _ = fs::remove_file(f.net.join("wallet.sqlite-shm"));
}

#[test]
fn snapshot_reads_a_hot_wal_without_checkpointing_it() {
    let f = fixture("v0.1.0");
    hot_wal(&f);
    let main = sha256_file(&f.wallet()).unwrap();
    let wal = sha256_file(&f.net.join("wallet.sqlite-wal")).unwrap();
    let app_main = sha256_file(&f.net.join("app.sqlite")).unwrap();
    let app_wal = sha256_file(&f.net.join("app.sqlite-wal")).unwrap();
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    assert_eq!(
        sha256_file(&f.wallet()).unwrap(),
        main,
        "main file untouched"
    );
    assert_eq!(
        sha256_file(&f.net.join("wallet.sqlite-wal")).unwrap(),
        wal,
        "-wal kept, not checkpointed"
    );
    assert_eq!(sha256_file(&f.net.join("app.sqlite")).unwrap(), app_main);
    assert_eq!(sha256_file(&f.net.join("app.sqlite-wal")).unwrap(), app_wal);
    let copy = open_immutable(&snapshot.dir().join("wallet.sqlite")).unwrap();
    let hot: i64 = copy
        .query_row("SELECT a FROM dwsu_hot", [], |r| r.get(0))
        .unwrap();
    assert_eq!(hot, 42, "the copy holds the logged change");
}

#[test]
fn a_store_kept_locked_is_held_not_waited_on_forever() {
    let f = fixture("v0.1.0");
    let dispatch = f.net.join("dispatch.sqlite");
    let holder = Connection::open(&dispatch).unwrap();
    let mode: String = holder
        .query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    holder
        .execute_batch("BEGIN EXCLUSIVE; CREATE TABLE dwsu_lock (a INTEGER)")
        .unwrap();
    let tmp = private_tempdir();
    let err = crate::snapshot::copy_store(&dispatch, &tmp.path().join("copy.sqlite")).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    let err = take_snapshot(&f.net, now()).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    holder.execute_batch("ROLLBACK").unwrap();
}

#[test]
fn stale_scratch_is_swept_only_by_the_instance_lock_holder() {
    let tmp = private_tempdir();
    let root = tmp.path();
    let stale = root.join(format!("{}crashed", crate::precheck::SCRATCH_PREFIX));
    fs::create_dir(&stale).unwrap();
    fs::write(stale.join("wallet.sqlite"), b"copy").unwrap();
    fs::create_dir(root.join("regtest")).unwrap();
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("instance.lock"))
        .unwrap();
    let err = crate::precheck::sweep_stale_scratch(root, &lock).unwrap_err();
    assert!(
        matches!(err, StoreUpgradeError::StoreHeld { .. }),
        "{err:?}"
    );
    assert!(stale.exists(), "not swept without the lock");
    let other = fs::File::create(root.join("other.lock")).unwrap();
    lock.lock().unwrap();
    assert!(crate::precheck::sweep_stale_scratch(root, &other).is_err());
    assert!(stale.exists(), "not swept with another file's handle");
    assert_eq!(
        crate::precheck::sweep_stale_scratch(root, &lock).unwrap(),
        1
    );
    assert!(!stale.exists());
    assert!(root.join("regtest").is_dir());
}

/// `adopters` with this fixture's fork-shaped retired table as its own fresh
/// one, plus `extra` objects on it.
fn with_fresh_retired(f: &Fixture, extra: &[(&str, &str)]) -> crate::adopters::Adopters {
    let conn = open_immutable(&f.wallet()).unwrap();
    let mut objects: Vec<FreshObject> = conn
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE tbl_name = ?1")
        .unwrap()
        .query_map([crate::fork::RETIRED_POOL], |r| {
            Ok(FreshObject {
                kind: r.get(0)?,
                name: r.get(1)?,
                table: r.get(2)?,
                sql: r.get(3)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for (name, sql) in extra {
        objects.push(FreshObject {
            kind: "index".into(),
            name: (*name).into(),
            table: crate::fork::RETIRED_POOL.into(),
            sql: Some((*sql).into()),
        });
    }
    let mut adopters = build().with_p9a(P9aStatus::Present);
    adopters.fresh.extend(objects);
    adopters
}

#[test]
fn a_build_whose_retired_table_differs_from_fork_v1_refuses() {
    let f = fixture("v0.1.0");
    let same = with_fresh_retired(&f, &[]);
    assert!(matches!(
        precheck_with(&f.net, &same).unwrap(),
        Precheck::Fork { .. }
    ));
    let differs = with_fresh_retired(
        &f,
        &[(
            "idx_extra",
            "CREATE INDEX idx_extra ON core_address_pool_retired(used)",
        )],
    );
    let before = tree(&f.root);
    let found = precheck_with(&f.net, &differs).unwrap();
    assert!(
        matches!(
            &found,
            Precheck::Refuse { reason: StoreUpgradeError::ForkStoreShapeMismatch { object } }
                if object.starts_with("this build's")
        ),
        "{found:?}"
    );
    assert_eq!(tree(&f.root), before);
}
