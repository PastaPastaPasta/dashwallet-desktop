//! A restore through the real `Engine`: asked for in one process, run by
//! the next start's open before the refusal check and before any store
//! opens. Every open runs in its own child process, as a restart does: a
//! store opened in a process makes the restore refuse there
//! (`RestoreAfterOpen`), so this process opens none.

mod store_upgrade_common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use dw_engine::store_upgrade::RestoreState;
use dw_engine::{DashNetwork, Engine, EngineError};
use dw_store_upgrade::UPGRADE_BACKUPS_DIR;
use store_upgrade_common::*;

const CHILD_ROOT: &str = "DW_RESTORE_CHILD_ROOT";
/// Set: the child's probe reads P9a Absent, as a build without it does.
const CHILD_WITHOUT_P9A: &str = "DW_RESTORE_CHILD_WITHOUT_P9A";
const RESULT: &str = "CHILD-RESULT: ";

/// Run by [`open_in_child`] only: opens the root's regtest network and
/// prints how it went.
#[test]
fn child_open() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    let probe = dw_testutil::private_tempdir();
    if std::env::var_os(CHILD_WITHOUT_P9A).is_some() {
        dw_store_upgrade::Adopters::cache_with_p9a_for_test(
            probe.path(),
            dw_store_upgrade::P9aStatus::Absent,
        );
    }
    let engine = engine(Path::new(&root));
    let outcome = match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
        Ok(_) => "ok".to_string(),
        Err(EngineError::StoreRestore(r)) => format!("restore {:?} {}", r.state, r.cause),
        Err(EngineError::StoreRefused(r)) => {
            format!("refused {} {}", r.cause, r.screen.unchanged.key)
        }
        Err(e) => format!("err {} {e}", e.code()),
    };
    close(engine);
    println!("{RESULT}{outcome}");
}

/// Shuts `engine` down and drops it, so its instance lock is free for the
/// next start.
fn close(engine: Engine) {
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
}

/// Opens `root`'s regtest network in a child process, as a fresh start
/// does; what [`child_open`] printed.
fn open_in_child(root: &Path) -> String {
    open_in_child_with(root, false)
}

fn open_in_child_with(root: &Path, without_p9a: bool) -> String {
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "child_open", "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, root);
    if without_p9a {
        child.env(CHILD_WITHOUT_P9A, "1");
    }
    let out = child.stdin(Stdio::null()).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    match stdout
        .lines()
        .find_map(|l| l.split_once(RESULT).map(|(_, r)| r))
    {
        Some(r) if out.status.success() => r.to_string(),
        _ => panic!(
            "the child failed: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        ),
    }
}

/// A fork fixture upgraded by a first start, so it has a verified
/// before-upgrade snapshot.
fn upgraded() -> Fixture {
    let f = fixture("v0.1.0");
    assert_eq!(open_in_child(&f.root), "ok");
    f
}

fn backups(f: &Fixture) -> PathBuf {
    f.net.join(UPGRADE_BACKUPS_DIR)
}

fn after_dirs(f: &Fixture) -> usize {
    std::fs::read_dir(backups(f))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("after-upgrade-")
        })
        .count()
}

/// Restore earlier data: the request writes only RESTORE-REQUESTED; the
/// next start's open runs it before any store opens and opens the restored
/// data, with the RESTORED mark set. Run after a store opened in that
/// process, or after the refusal check, the restore would stop the open.
#[test]
fn a_requested_restore_runs_at_the_next_open() {
    let f = upgraded();
    let engine = engine(&f.root);
    assert!(
        engine
            .block_on(engine.restore_available(&DashNetwork::Regtest))
            .unwrap()
    );
    let before = tree(&f.net);
    engine
        .block_on(engine.request_restore(&DashNetwork::Regtest))
        .unwrap();
    assert_eq!(
        tree_diff(&before, &tree(&f.net)),
        [format!("{UPGRADE_BACKUPS_DIR}/RESTORE-REQUESTED")]
    );
    close(engine);
    assert_eq!(after_dirs(&f), 0);

    assert_eq!(open_in_child(&f.root), "ok");
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 1);
}

/// A refusal after the start's restore put the before-upgrade files back
/// never says Dash Wallet changed nothing (ProvedState's `wrote`): the
/// restored file isn't upgraded, so the screen only asks to keep the folder.
/// Here the restored fork file holds a rotated contact and the build has no
/// P9a.
#[test]
fn a_refusal_after_a_restore_never_says_nothing_changed() {
    let f = fixture("v0.1.0");
    add_retired_row(&f.wallet());
    assert_eq!(open_in_child(&f.root), "ok");
    let engine = engine(&f.root);
    engine
        .block_on(engine.request_restore(&DashNetwork::Regtest))
        .unwrap();
    close(engine);
    assert_eq!(
        open_in_child_with(&f.root, true),
        "refused RetiredPoolWithoutAdopter storeUpgrade.refused.keep"
    );
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(
        open_in_child_with(&f.root, true),
        "refused RetiredPoolWithoutAdopter storeUpgrade.refused.unchanged"
    );
}

/// An unreadable request on a fork file that was never upgraded: the open
/// stops before any store opens, and only the request is gone; every other
/// file and directory is byte-identical.
#[test]
fn an_unreadable_request_keeps_every_store_closed_and_unchanged() {
    let f = fixture("v0.1.0");
    let before = tree(&f.net);
    std::fs::create_dir(backups(&f)).unwrap();
    std::fs::write(backups(&f).join("RESTORE-REQUESTED"), b"").unwrap();
    assert_eq!(open_in_child(&f.root), "restore Refused garbledMarker");
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    let mut after = tree(&f.net);
    after.retain(|k, _| !k.starts_with(UPGRADE_BACKUPS_DIR));
    assert_eq!(tree_diff(&before, &after), Vec::<String>::new());
}

/// A request the next start can't honour stops that open as
/// `storage_restore` refused, once: the request is gone, and the start
/// after opens the data as it was.
#[test]
fn a_garbled_request_stops_the_open_once() {
    let f = upgraded();
    std::fs::write(backups(&f).join("RESTORE-REQUESTED"), b"not a request").unwrap();
    assert_eq!(open_in_child(&f.root), "restore Refused garbledMarker");
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(!dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 0);
}

/// Cancel restore drops a request that hasn't begun: the next start opens
/// the data as it was.
#[test]
fn a_cancelled_request_never_runs() {
    let f = upgraded();
    let engine = engine(&f.root);
    engine
        .block_on(engine.request_restore(&DashNetwork::Regtest))
        .unwrap();
    engine
        .block_on(engine.cancel_restore(&DashNetwork::Regtest))
        .unwrap();
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    engine
        .block_on(engine.keep_current_files(&DashNetwork::Regtest))
        .unwrap();
    close(engine);
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(!dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 0);
}

/// A restore already replacing files can only be finished: asking for
/// another is the incomplete-restore screen (never the foreign one, never
/// internal), and no request is written.
#[test]
fn a_request_while_a_restore_is_replacing_files_is_the_incomplete_screen() {
    let f = upgraded();
    std::fs::write(
        backups(&f).join(dw_store_upgrade::RESTORE_JOURNAL),
        b"journal",
    )
    .unwrap();
    let before = tree(&f.net);
    let engine = engine(&f.root);
    match engine.block_on(engine.request_restore(&DashNetwork::Regtest)) {
        Err(EngineError::StoreRestore(r)) => {
            assert_eq!((r.state, r.cause), (RestoreState::Incomplete, "other"));
            assert_eq!(r.folder, f.net);
        }
        other => panic!("{other:?}"),
    }
    close(engine);
    assert_eq!(tree_diff(&before, &tree(&f.net)), Vec::<String>::new());
}

/// The incomplete screen wins over the snapshot's state: beside a torn
/// snapshot, a restore replacing files is still the incomplete screen.
#[test]
fn a_request_while_a_restore_is_replacing_files_beside_a_torn_snapshot_is_incomplete() {
    let f = upgraded();
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    std::fs::write(
        snapshot.dir().join(dw_store_upgrade::STORE_FILES[0]),
        b"torn",
    )
    .unwrap();
    std::fs::write(
        backups(&f).join(dw_store_upgrade::RESTORE_JOURNAL),
        b"journal",
    )
    .unwrap();
    let before = tree(&f.net);
    let engine = engine(&f.root);
    assert!(
        !engine
            .block_on(engine.restore_available(&DashNetwork::Regtest))
            .unwrap()
    );
    match engine.block_on(engine.request_restore(&DashNetwork::Regtest)) {
        Err(EngineError::StoreRestore(r)) => {
            assert_eq!((r.state, r.cause), (RestoreState::Incomplete, "other"))
        }
        other => panic!("{other:?}"),
    }
    close(engine);
    assert_eq!(tree_diff(&before, &tree(&f.net)), Vec::<String>::new());
}

/// Without a verified snapshot there is nothing to restore: the request is
/// refused (missingBackup) and nothing is written.
#[test]
fn a_request_without_a_snapshot_is_refused() {
    let f = fixture("v0.1.0");
    let before = tree(&f.net);
    let engine = engine(&f.root);
    assert!(
        !engine
            .block_on(engine.restore_available(&DashNetwork::Regtest))
            .unwrap()
    );
    match engine.block_on(engine.request_restore(&DashNetwork::Regtest)) {
        Err(EngineError::StoreRestore(r)) => {
            assert_eq!((r.state, r.cause), (RestoreState::Refused, "missingBackup"));
            assert_eq!(r.folder, f.net);
        }
        other => panic!("{other:?}"),
    }
    close(engine);
    assert!(tree_diff(&before, &tree(&f.net)).is_empty());
}

/// "Fixture main" of the v0.1.0 fixture, as SQL.
const MAIN: &str = "X'0FF35F248C69653EFCE8336CE14B78E29CD3BBAF4162C42246259850AF2C2ADD'";
const KEY: &str = "X'5151515151515151515151515151515151515151515151515151515151515151'";

/// A `shielded_pending_spends` row of [`MAIN`] in the live shielded store:
/// a spend of unknown result the library stored.
fn add_pending_row(net: &Path) {
    exec(
        &net.join("shielded.sqlite"),
        &format!(
            "CREATE TABLE IF NOT EXISTS shielded_pending_spends (wallet_id BLOB NOT NULL, \
             account_index INTEGER NOT NULL, activity_id BLOB NOT NULL, anchor BLOB NOT NULL, \
             nullifiers BLOB NOT NULL, st_bytes BLOB NOT NULL, attempts INTEGER NOT NULL \
             DEFAULT 0, PRIMARY KEY (wallet_id, account_index, activity_id)); \
             INSERT INTO shielded_pending_spends (wallet_id, account_index, activity_id, anchor, \
             nullifiers, st_bytes) VALUES ({MAIN}, 0, {KEY}, {KEY}, {KEY}, X'00')"
        ),
    );
}

/// Each of the four stores' sha256, `None` for one not there.
fn stores(net: &Path) -> Vec<Option<String>> {
    let all = tree(net);
    dw_store_upgrade::STORE_FILES
        .iter()
        .map(|s| all.get(*s).cloned())
        .collect()
}

/// The restore stopped before anything moved, its request kept: no copy,
/// no mark, no journal, every store as it was.
fn nothing_moved(f: &Fixture, live: &[Option<String>]) {
    assert_eq!(stores(&f.net), live);
    assert_eq!(
        dw_store_upgrade::pending_restore(&f.net).unwrap(),
        Some(dw_store_upgrade::PendingRestore::Requested)
    );
    assert_eq!(after_dirs(f), 0);
    assert!(!dw_store_upgrade::restored_mark_present(&f.net));
}

fn request(f: &Fixture) {
    let engine = engine(&f.root);
    engine
        .block_on(engine.request_restore(&DashNetwork::Regtest))
        .unwrap();
    close(engine);
}

/// R-1 (b): a spend dispatched after the request (here its stored record)
/// stops the next start's restore before anything moves, in the notStarted
/// state with the request kept; once it clears, the same request runs.
#[test]
fn a_hold_that_appears_after_the_request_stops_the_restore() {
    let f = upgraded();
    request(&f);
    add_pending_row(&f.net);
    let live = stores(&f.net);
    assert_eq!(
        open_in_child(&f.root),
        "restore NotStarted shieldedSpendInFlight"
    );
    nothing_moved(&f, &live);
    assert_eq!(
        open_in_child(&f.root),
        "restore NotStarted shieldedSpendInFlight",
        "again at every start while it holds"
    );
    exec(
        &f.net.join("shielded.sqlite"),
        "DELETE FROM shielded_pending_spends",
    );
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 1);
}

/// R-1 (b): a DEC-1057 marker or an open artifact in the live journal, and
/// a shielded store that does not read, stop the restore the same way.
#[test]
fn a_journal_hold_or_an_unreadable_store_stops_the_restore() {
    let marker = format!(
        "INSERT INTO shielded_unrecorded (wallet, activity, anchor, at) \
         VALUES ({MAIN}, {KEY}, {KEY}, 1)"
    );
    let artifact = format!(
        "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, kind, anchor, status, \
         at) VALUES ({MAIN}, {KEY}, 'dispatched', {KEY}, 'transfer', {KEY}, 'registered', 1)"
    );
    for (what, want, write) in [
        ("marker", "shieldedSpendCarriedOver", Some(marker.as_str())),
        ("artifact", "shieldedSpendInFlight", Some(artifact.as_str())),
        ("unreadable", "shieldedSpendCarriedOver", None),
    ] {
        let f = upgraded();
        request(&f);
        match write {
            Some(sql) => exec(&f.net.join("dispatch.sqlite"), sql),
            None => std::fs::write(f.net.join("shielded.sqlite"), b"not a database").unwrap(),
        }
        let live = stores(&f.net);
        assert_eq!(
            open_in_child(&f.root),
            format!("restore NotStarted {want}"),
            "{what}"
        );
        nothing_moved(&f, &live);
    }
}

/// A before set without `shielded.sqlite`: the restore would remove the
/// live one. While it holds a spend, the restore stops first and the live
/// store stays, its row in it.
#[test]
fn a_snapshot_without_the_shielded_store_never_removes_a_held_one() {
    let f = fixture("v0.1.0");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(f.net.join(format!("shielded.sqlite{suffix}")));
    }
    assert_eq!(open_in_child(&f.root), "ok");
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    assert!(
        !snapshot.files().iter().any(|s| s.name == "shielded.sqlite"),
        "the before set has no shielded store"
    );
    request(&f);
    add_pending_row(&f.net);
    let live = stores(&f.net);
    assert!(live[1].is_some());
    assert_eq!(
        open_in_child(&f.root),
        "restore NotStarted shieldedSpendInFlight"
    );
    nothing_moved(&f, &live);
    let rows: i64 = rusqlite::Connection::open(f.net.join("shielded.sqlite"))
        .unwrap()
        .query_row("SELECT count(*) FROM shielded_pending_spends", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 1);
}

/// R-1 (a), (c): with no session open on the network, the stores on disk
/// are read: a hold refuses the request and nothing is written.
#[test]
fn a_hold_on_disk_refuses_the_request_with_no_session_open() {
    let f = upgraded();
    exec(
        &f.net.join("dispatch.sqlite"),
        &format!(
            "INSERT INTO shielded_unrecorded (wallet, activity, anchor, at) \
             VALUES ({MAIN}, {KEY}, {KEY}, 1)"
        ),
    );
    let before = tree(&f.net);
    let engine = engine(&f.root);
    assert!(
        engine
            .block_on(engine.session(&DashNetwork::Regtest))
            .is_none()
    );
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    assert!(
        matches!(
            got,
            Err(EngineError::ShieldedSpendHeld(
                dw_engine::ShieldedHold::CarriedOver
            ))
        ),
        "{got:?}"
    );
    close(engine);
    assert_eq!(tree_diff(&before, &tree(&f.net)), Vec::<String>::new());
}

/// B row 2: a fork disposition of the before set comes back with it. The
/// first open after the restore upgrades and drains the restored store
/// again, and the hold is in the journal again.
#[test]
fn a_restored_fork_disposition_is_held_again_on_the_next_open() {
    use dw_shielded_drain::test_support::{ForkRow, fork_store};
    let f = fixture("v0.1.0");
    let shielded = f.net.join("shielded.sqlite");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(f.net.join(format!("shielded.sqlite{suffix}")));
    }
    let mut wallet = [0; 32];
    hex::decode_to_slice(&MAIN[2..66], &mut wallet).unwrap();
    fork_store(
        &shielded,
        &wallet,
        0,
        &[ForkRow::Disposition {
            st: 2,
            kind: "unresolved",
            reason: Some("purged"),
            nullifiers: vec![[102; 32]],
        }],
    )
    .unwrap();
    let held = |f: &Fixture| -> Vec<Vec<u8>> {
        let db = rusqlite::Connection::open(f.net.join("dispatch.sqlite")).unwrap();
        let mut stmt = db
            .prepare("SELECT nullifier FROM shielded_drained_hold WHERE spent = 0")
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(open_in_child(&f.root), "ok");
    assert_eq!(held(&f), [vec![102u8; 32]], "drained at the upgrade");
    request(&f);
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 1);
    assert_eq!(held(&f), [vec![102u8; 32]], "held again after the restore");
}
