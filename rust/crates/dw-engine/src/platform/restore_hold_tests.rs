//! R-1 (a), (c): "Restore earlier data" is refused while the stores on disk
//! keep a shielded hold the restore would replace, or do not read, whether
//! the network is open or not; nothing is written then.

use std::path::Path;

use dw_appdb::dispatch::{DISPATCH_DB_FILE, DispatchJournal, JournalOpen};
use platform_wallet::wallet::shielded::ShieldedStore;

use super::close_unload_gate_tests::{ANCHOR, NULLIFIER, engine_at, launch, subwallet, with_store};
use super::restore_hold::restore_hold;
use super::shielded_close_tests::ANCHOR_HEIGHT;
use super::shielded_pool::SHIELDED_DB_FILE;
use crate::store_upgrade::{RestoreState, request_restore};
use crate::{DashNetwork, EngineError, ShieldedHold};

const WALLET: &str = "X'0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD'";
const KEY: &str = "X'5151515151515151515151515151515151515151515151515151515151515151'";

const PENDING: &str = "CREATE TABLE IF NOT EXISTS shielded_pending_spends (
    wallet_id BLOB NOT NULL, account_index INTEGER NOT NULL, activity_id BLOB NOT NULL,
    anchor BLOB NOT NULL, nullifiers BLOB NOT NULL, st_bytes BLOB NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (wallet_id, account_index, activity_id))";

fn shielded(dir: &Path, sql: &str) {
    let db = rusqlite::Connection::open(dir.join(SHIELDED_DB_FILE)).unwrap();
    db.execute_batch(PENDING).unwrap();
    db.execute_batch(sql).unwrap();
}

/// A journal at this build's schema, then `sql` on it.
fn journal(dir: &Path, sql: &str) {
    let path = dir.join(DISPATCH_DB_FILE);
    match DispatchJournal::open(&path, 1).unwrap() {
        JournalOpen::Ready(j) => drop(j),
        JournalOpen::NewerSchema(v) => panic!("newer schema {v}"),
    }
    rusqlite::Connection::open(path)
        .unwrap()
        .execute_batch(sql)
        .unwrap();
}

fn artifact(status: &str, reason: &str) -> String {
    format!(
        "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, kind, anchor, status, \
         status_reason, at) VALUES ({WALLET}, {KEY}, 'dispatched', {KEY}, 'transfer', {KEY}, \
         '{status}', {reason}, 1)"
    )
}

/// Every file under `dir` with its bytes.
fn tree(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            out.extend(tree(&path));
        } else {
            out.insert(path.clone(), std::fs::read(&path).unwrap());
        }
    }
    out
}

/// What `request_restore` says for `dir`, with nothing written by it.
fn requested(dir: &Path) -> Result<(), EngineError> {
    let before = tree(dir);
    let got = request_restore(dir);
    assert_eq!(tree(dir), before, "nothing is written");
    got
}

fn refused_with(dir: &Path, want: ShieldedHold, what: &str) {
    assert_eq!(restore_hold(dir), Some(want), "{what}");
    let got = requested(dir);
    assert!(
        matches!(&got, Err(EngineError::ShieldedSpendHeld(h)) if *h == want),
        "{what}: {got:?}"
    );
    assert_eq!(got.unwrap_err().code(), "shielded_spend_unresolved");
}

/// Past the hold check: with no snapshot, the snapshot's own refusal.
fn passes(dir: &Path, what: &str) {
    assert_eq!(restore_hold(dir), None, "{what}");
    match requested(dir) {
        Err(EngineError::StoreRestore(r)) => {
            assert_eq!(
                (r.state, r.cause),
                (RestoreState::Refused, "missingBackup"),
                "{what}"
            )
        }
        other => panic!("{what}: {other:?}"),
    }
}

fn case(prep: impl FnOnce(&Path)) -> tempfile::TempDir {
    let dir = dw_testutil::private_tempdir();
    prep(dir.path());
    dir
}

#[test]
fn nothing_held_lets_the_request_through() {
    passes(case(|_| {}).path(), "no stores");
    passes(
        case(|d| {
            shielded(d, "");
            journal(d, &artifact("executed", "NULL"));
        })
        .path(),
        "empty stores, a settled artifact",
    );
    passes(
        case(|d| journal(d, &artifact("archived", "NULL"))).path(),
        "an archived artifact",
    );
    passes(
        case(|d| {
            rusqlite::Connection::open(d.join(DISPATCH_DB_FILE))
                .unwrap()
                .execute_batch("PRAGMA user_version = 0; VACUUM")
                .unwrap()
        })
        .path(),
        "a journal file with no table",
    );
}

#[test]
fn a_pending_row_refuses_the_request() {
    let row = format!(
        "INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, anchor, \
         nullifiers, st_bytes) VALUES ({WALLET}, 0, {KEY}, {KEY}, {KEY}, X'00')"
    );
    refused_with(
        case(|d| shielded(d, &row)).path(),
        ShieldedHold::InFlight,
        "a stored spend",
    );
}

#[test]
fn a_journal_artifact_or_marker_refuses_the_request() {
    refused_with(
        case(|d| journal(d, &artifact("registered", "NULL"))).path(),
        ShieldedHold::InFlight,
        "a registered artifact",
    );
    refused_with(
        case(|d| journal(d, &artifact("unresolved", "NULL"))).path(),
        ShieldedHold::InFlight,
        "an unresolved artifact",
    );
    refused_with(
        case(|d| journal(d, &artifact("unclassified", "NULL"))).path(),
        ShieldedHold::CarriedOver,
        "an unclassified import",
    );
    let marker = format!(
        "INSERT INTO shielded_unrecorded (wallet, activity, anchor, at) \
         VALUES ({WALLET}, {KEY}, {KEY}, 1)"
    );
    refused_with(
        case(|d| journal(d, &marker)).path(),
        ShieldedHold::CarriedOver,
        "a DEC-1057 marker",
    );
}

/// A journal from before artifacts: its stored redrives import as
/// unclassified artifacts, which hold.
#[test]
fn a_redrive_in_a_journal_from_before_artifacts_refuses_the_request() {
    let dir = case(|d| {
        let db = rusqlite::Connection::open(d.join(DISPATCH_DB_FILE)).unwrap();
        db.execute_batch(&format!(
            "CREATE TABLE meta (k TEXT PRIMARY KEY, v BLOB NOT NULL) WITHOUT ROWID;
             INSERT INTO meta VALUES ('schema', 10);
             CREATE TABLE shielded_redrive (wallet BLOB, activity BLOB, transition BLOB, at INTEGER);
             CREATE TABLE shielded_unrecorded (wallet BLOB, activity BLOB, anchor BLOB, at INTEGER);
             INSERT INTO shielded_redrive VALUES ({WALLET}, {KEY}, {KEY}, 1);"
        ))
        .unwrap();
    });
    refused_with(dir.path(), ShieldedHold::CarriedOver, "a 0.1 redrive");
}

/// The fork's held disposition, which the open marks (fork-era: it refuses
/// too); one with no notes held, or released by a scan, holds nothing.
#[test]
fn a_fork_disposition_that_holds_notes_refuses_the_request() {
    let table = "CREATE TABLE shielded_spend_dispositions (wallet_id BLOB NOT NULL, \
        account_index INTEGER NOT NULL, st_hash BLOB NOT NULL, activity_id BLOB NOT NULL, \
        disposition TEXT NOT NULL, reason TEXT, at INTEGER NOT NULL, \
        nullifiers BLOB NOT NULL DEFAULT x'', released_by_scan INTEGER NOT NULL DEFAULT 0)";
    let row = |nullifiers: &str, released: u8| {
        format!(
            "{table}; INSERT INTO shielded_spend_dispositions (wallet_id, account_index, st_hash, \
             activity_id, disposition, reason, at, nullifiers, released_by_scan) VALUES \
             ({WALLET}, 0, {KEY}, {KEY}, 'unresolved', 'purged', 1, {nullifiers}, {released})"
        )
    };
    refused_with(
        case(|d| shielded(d, &row(KEY, 0))).path(),
        ShieldedHold::CarriedOver,
        "a fork-held spend",
    );
    refused_with(
        case(|d| shielded(d, &row("X'0102'", 0))).path(),
        ShieldedHold::CarriedOver,
        "a fork-held spend that does not read",
    );
    passes(
        case(|d| shielded(d, &row("x''", 0))).path(),
        "no notes held",
    );
    passes(
        case(|d| shielded(d, &row(KEY, 1))).path(),
        "released by a scan",
    );
}

/// A store or journal that does not read holds: not a database, a shape
/// the reader doesn't know, a journal newer than this build.
#[test]
fn an_unreadable_store_or_journal_refuses_the_request() {
    let garbage = |name: &'static str| {
        move |d: &Path| std::fs::write(d.join(name), b"not a database, not at all").unwrap()
    };
    refused_with(
        case(garbage(SHIELDED_DB_FILE)).path(),
        ShieldedHold::CarriedOver,
        "a shielded store that is not a database",
    );
    refused_with(
        case(garbage(DISPATCH_DB_FILE)).path(),
        ShieldedHold::CarriedOver,
        "a journal that is not a database",
    );
    refused_with(
        case(|d| {
            let db = rusqlite::Connection::open(d.join(SHIELDED_DB_FILE)).unwrap();
            db.execute_batch("CREATE TABLE shielded_pending_spends (other BLOB)")
                .unwrap();
        })
        .path(),
        ShieldedHold::CarriedOver,
        "a pending table of another shape",
    );
    refused_with(
        case(|d| journal(d, "UPDATE meta SET v = 99 WHERE k = 'schema'")).path(),
        ShieldedHold::CarriedOver,
        "a newer journal",
    );
    refused_with(
        case(|d| journal(d, "DROP TABLE meta")).path(),
        ShieldedHold::CarriedOver,
        "a journal with tables and no schema",
    );
    let dir = case(|_| {});
    let fail =
        || (super::shielded_pool::FAIL_DISK_READ_IN.lock().unwrap()).push(dir.path().to_path_buf());
    fail();
    assert_eq!(restore_hold(dir.path()), Some(ShieldedHold::CarriedOver));
    fail();
    assert!(matches!(
        requested(dir.path()),
        Err(EngineError::ShieldedSpendHeld(ShieldedHold::CarriedOver))
    ));
    passes(dir.path(), "once it reads");
}

/// (a) with the network open: the removal gate's hold of a loaded wallet
/// refuses the request, a reservation the library keeps in memory alone
/// included; once it clears, the request gets past the check.
#[test]
fn a_reservation_in_the_open_session_refuses_the_request() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = launch(&engine);
    let data_dir = session.data_dir().to_path_buf();
    with_store(&engine, &session, move |store| {
        assert!(store.mark_pending(subwallet(wallet), &NULLIFIER).unwrap());
        (store.set_pending_spend(
            subwallet(wallet),
            &NULLIFIER,
            ANCHOR,
            super::close_unload_gate_tests::ACTIVITY,
            Some(ANCHOR_HEIGHT),
        ))
        .unwrap();
    });
    assert_eq!(restore_hold(&data_dir), None, "nothing on disk");
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    assert!(
        matches!(
            &got,
            Err(EngineError::ShieldedSpendHeld(ShieldedHold::InFlight))
        ),
        "{got:?}"
    );
    assert_eq!(dw_store_upgrade::pending_restore(&data_dir).unwrap(), None);

    with_store(&engine, &session, move |store| {
        assert!(store.clear_pending(subwallet(wallet), &NULLIFIER).unwrap());
    });
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    assert!(
        matches!(&got, Err(EngineError::StoreRestore(r)) if r.cause == "missingBackup"),
        "{got:?}"
    );
    engine.block_on(engine.shutdown()).unwrap();
}
