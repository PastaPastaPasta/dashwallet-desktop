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
use dw_store_upgrade::copy::WayOut;
use store_upgrade_common::*;

const CHILD_ROOT: &str = "DW_RESTORE_CHILD_ROOT";
/// Set: the child's probe reads P9a Absent, as a build without it does.
const CHILD_WITHOUT_P9A: &str = "DW_RESTORE_CHILD_WITHOUT_P9A";
const RESULT: &str = "CHILD-RESULT: ";
/// The steps [`child_steps`] runs, comma-separated.
const CHILD_STEPS: &str = "DW_RESTORE_CHILD_STEPS";
/// Where `heal` copies `wallet.sqlite` back from, in the data root.
const GOOD_WALLET: &str = "wallet.good";

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
    let p9a = [(CHILD_WITHOUT_P9A, "1")];
    child_result("child_open", root, if without_p9a { &p9a } else { &[] })
}

/// Runs the test `name` in a child process on `root`, with `env`; what it
/// printed after [`RESULT`].
fn child_result(name: &str, root: &Path, env: &[(&str, &str)]) -> String {
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env(CHILD_ROOT, root)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap();
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

/// Makes `f`'s schema history no longer match, keeping a good copy of its
/// wallet file in the root for [`heal`]: the next open is refused with
/// Restore earlier data.
fn break_history(f: &Fixture) {
    std::fs::copy(f.wallet(), f.root.join(GOOD_WALLET)).unwrap();
    exec(
        &f.wallet(),
        "UPDATE refinery_schema_history SET checksum = '1' WHERE version = 1",
    );
}

/// Puts [`GOOD_WALLET`] back as `f`'s wallet file.
fn heal(f: &Fixture) {
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", f.wallet().display()));
    }
    std::fs::copy(f.root.join(GOOD_WALLET), f.wallet()).unwrap();
}

/// An engine on `f`'s root showing the Restore screen: [`break_history`],
/// then its open of regtest refused with Restore earlier data, the only
/// screen that asks for a restore. The refused open opens no store.
fn on_restore_screen(f: &Fixture) -> Engine {
    break_history(f);
    let engine = engine(&f.root);
    match engine.block_on(engine.open_network(DashNetwork::Regtest, local_opts())) {
        Err(EngineError::StoreRefused(r)) => assert_eq!(r.screen.way_out, WayOut::Restore),
        Err(other) => panic!("not refused with Restore earlier data: {other:?}"),
        Ok(_) => panic!("opened"),
    }
    engine
}

/// M1: Restore earlier data is only for a network whose open was refused
/// with it. On a healthy open network, and once it is closed, the request
/// is refused and nothing is asked for, though a verified snapshot is
/// there.
#[test]
fn a_request_on_an_open_network_is_refused() {
    let f = upgraded();
    let got = steps_in_child(&f.root, &["open", "request", "close", "request"]);
    assert_eq!(
        got,
        [
            "ok",
            "err invalid_argument",
            "closed true",
            "err invalid_argument"
        ]
    );
    assert!(!backups(&f).join("RESTORE-REQUESTED").exists());
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    let engine = engine(&f.root);
    assert!(
        engine
            .block_on(engine.restore_available(&DashNetwork::Regtest))
            .unwrap()
    );
    close(engine);
}

/// M1: a network this engine never opened, its snapshot verified, is
/// refused too, and nothing is written.
#[test]
fn a_request_without_a_refused_open_is_refused_and_writes_nothing() {
    let f = upgraded();
    let before = tree(&f.net);
    let engine = engine(&f.root);
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    assert!(
        matches!(got, Err(EngineError::InvalidArgument(_))),
        "{got:?}"
    );
    close(engine);
    assert_eq!(tree_diff(&before, &tree(&f.net)), Vec::<String>::new());
}

#[test]
fn an_old_snapshot_cannot_restore_a_wallet_missing_from_live_data() {
    let f = upgraded();
    request(&f);
    exec(&f.wallet(), "DELETE FROM wallets");
    let result = open_in_child(&f.root);
    assert!(result.starts_with("restore Refused "), "{result}");
    assert_eq!(after_dirs(&f), 0);
}

fn fixture_passphrase() -> Vec<u8> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/upgrade/v0.1.0/expected.json"
    )))
    .unwrap();
    fixture["passphrase"].as_str().unwrap().as_bytes().to_vec()
}

fn named_wallets(path: &Path) -> Vec<String> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut stmt = db
        .prepare("SELECT name FROM wallets ORDER BY name")
        .unwrap();
    stmt.query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn removal_keeps_the_old_snapshot_retired_after_a_same_seed_reimport() {
    let f = upgraded();
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    let before = tree(snapshot.dir());

    assert_eq!(steps_in_child(&f.root, &["remove-main"]), ["removed"]);
    assert_eq!(named_wallets(&f.net.join("app.sqlite")), ["Fixture second"]);
    assert!(!dw_store_upgrade::snapshot_present(&f.net).unwrap());

    assert_eq!(steps_in_child(&f.root, &["import-main"]), ["imported"]);
    assert_eq!(
        named_wallets(&f.net.join("app.sqlite")),
        ["Fixture second", "Reimported main"]
    );
    assert!(dw_store_upgrade::request_restore(&f.net, &snapshot).is_err());
    assert_eq!(open_in_child(&f.root), "ok");
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    assert_eq!(
        tree_diff(&before, &tree(snapshot.dir())),
        [dw_store_upgrade::NOT_THIS_WALLET]
    );
}

#[test]
fn restore_stays_retired_if_removal_stops_after_the_mark() {
    let f = upgraded();
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    dw_store_upgrade::retire_snapshot_for_wallet_removal(&f.net, &main_wallet()).unwrap();

    // The next process finds the durable mark with the original wallet still present.
    assert_eq!(
        named_wallets(&f.net.join("app.sqlite")),
        ["Fixture main", "Fixture second"]
    );
    assert!(dw_store_upgrade::request_restore(&f.net, &snapshot).is_err());
    break_history(&f);
    assert_eq!(
        steps_in_child(&f.root, &["open", "request", "recover"]),
        [
            RETIRED_SCREEN,
            "err invalid_argument",
            "recovered regtest-recovered-1",
        ]
    );
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);
    assert_eq!(
        named_wallets(&f.net.join("app.sqlite")),
        ["Fixture main", "Fixture second"]
    );
}

#[test]
fn removal_succeeds_when_the_named_snapshot_is_missing_and_stays_retired_if_returned() {
    let f = upgraded();
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    let archived = snapshot.dir().with_file_name("archived-before-removal");
    std::fs::rename(snapshot.dir(), &archived).unwrap();
    assert_eq!(
        steps_in_child(&f.root, &["remove-main"]),
        ["removed"]
    );
    assert_eq!(named_wallets(&f.net.join("app.sqlite")), ["Fixture second"]);
    std::fs::rename(&archived, snapshot.dir()).unwrap();
    assert_eq!(steps_in_child(&f.root, &["import-main"]), ["imported"]);
    assert!(!dw_store_upgrade::snapshot_present(&f.net).unwrap());
}

#[cfg(unix)]
#[test]
fn removal_refuses_a_redirected_backup_root_without_changing_the_wallet_or_backup() {
    let f = upgraded();
    let before = tree(&backups(&f));
    assert_eq!(
        steps_in_child(&f.root, &["remove-main-with-redirected-backup"]),
        ["err storage"]
    );
    assert_eq!(
        named_wallets(&f.net.join("app.sqlite")),
        ["Fixture main", "Fixture second"]
    );
    assert_eq!(tree(&backups(&f)), before);
}

/// Restore earlier data: from the Restore screen, the request writes only
/// RESTORE-REQUESTED; the next start's open runs it before any store opens
/// and opens the restored data, with the RESTORED mark set. Run after a
/// store opened in that process, or after the refusal check, the restore
/// would stop the open.
#[test]
fn a_requested_restore_runs_at_the_next_open() {
    let f = upgraded();
    let engine = on_restore_screen(&f);
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
    request(&f);
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
    let engine = on_restore_screen(&f);
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
    heal(&f);
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
    let engine = on_restore_screen(&f);
    std::fs::write(
        backups(&f).join(dw_store_upgrade::RESTORE_JOURNAL),
        b"journal",
    )
    .unwrap();
    let before = tree(&f.net);
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
    let engine = on_restore_screen(&f);
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

/// Without a verified snapshot there is nothing to restore: from the
/// Restore screen, once its snapshot no longer verifies, the request is
/// refused (missingBackup) and nothing is written.
#[test]
fn a_request_without_a_snapshot_is_refused() {
    let f = upgraded();
    let engine = on_restore_screen(&f);
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    std::fs::write(
        snapshot.dir().join(dw_store_upgrade::STORE_FILES[0]),
        b"torn",
    )
    .unwrap();
    let before = tree(&f.net);
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
    request_got(f).unwrap();
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
    let engine = on_restore_screen(&f);
    let before = tree(&f.net);
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
    let f = fork_fixture();
    assert_eq!(
        drained_holds(&f),
        [vec![102u8; 32]],
        "drained at the upgrade"
    );
    request(&f);
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 1);
    assert_eq!(
        drained_holds(&f),
        [vec![102u8; 32]],
        "held again after the restore"
    );
}

/// Run by [`steps_in_child`] only: one engine on the root runs each step on
/// regtest and prints how each went. `open`; `close`; `request` (Restore
/// earlier data); `cancel` (Cancel restore); `recover` (Set up from
/// recovery phrase); `heal` puts [`GOOD_WALLET`] back as `wallet.sqlite`;
/// `unhold` deletes [`add_pending_row`]'s row.
#[test]
fn child_steps() {
    let (Some(root), Some(steps)) = (
        std::env::var_os(CHILD_ROOT),
        std::env::var(CHILD_STEPS).ok(),
    ) else {
        return;
    };
    let root = PathBuf::from(root);
    let net = DashNetwork::Regtest;
    let engine = engine(&root);
    let err = |e: EngineError| format!("err {}", e.code());
    let mut out = Vec::new();
    for step in steps.split(',') {
        out.push(match step {
            "open" => match engine.block_on(engine.open_network(net.clone(), local_opts())) {
                Ok(_) => "ok".to_string(),
                Err(EngineError::StoreRestore(r)) => format!("restore {:?} {}", r.state, r.cause),
                Err(EngineError::StoreRefused(r)) => {
                    let keys: Vec<&str> = r.screen.buttons.iter().map(|b| b.key).collect();
                    if r.screen.way_out == WayOut::Unverified {
                        format!("refused Unverified {} {}", r.screen.way_out_line.key, keys.join(" "))
                    } else {
                        format!("refused {:?} {}", r.screen.way_out, keys.join(" "))
                    }
                }
                Err(e) => err(e),
            },
            "close" => engine
                .block_on(engine.close_network(net.clone()))
                .map_or_else(err, |closed| format!("closed {closed}")),
            "request" => match engine.block_on(engine.request_restore(&net)) {
                Ok(()) => "requested".to_string(),
                Err(EngineError::ShieldedSpendHeld(hold)) => format!("held {hold:?}"),
                Err(e) => err(e),
            },
            "cancel" => engine
                .block_on(engine.cancel_restore(&net))
                .map_or_else(err, |()| "cancelled".to_string()),
            "recover" => engine
                .block_on(engine.recover_in_fresh_folder(net.clone()))
                .map_or_else(err, |dir| {
                    format!("recovered {}", dir.file_name().unwrap().to_string_lossy())
                }),
            "heal" => {
                let wallet = root.join("regtest").join("wallet.sqlite");
                for suffix in ["-wal", "-shm", "-journal"] {
                    let _ = std::fs::remove_file(format!("{}{suffix}", wallet.display()));
                }
                std::fs::copy(root.join(GOOD_WALLET), &wallet).unwrap();
                "healed".to_string()
            }
            "unhold" => {
                exec(
                    &root.join("regtest").join("shielded.sqlite"),
                    "DELETE FROM shielded_pending_spends",
                );
                "unheld".to_string()
            }
            "remove-main" | "remove-main-without-backup" | "remove-main-with-redirected-backup" | "import-main" => {
                use dw_vault::{Credential, GrantPurpose, UnlockScope};
                use zeroize::Zeroizing;
                let session = engine.block_on(engine.open_network(net.clone(), local_opts())).unwrap();
                let wallet = dw_engine::WalletId(main_wallet());
                let passphrase = Zeroizing::new(fixture_passphrase());
                if step.starts_with("remove-main") {
                    let moved = if step == "remove-main-without-backup" {
                        let net_dir = session.data_dir();
                        let snapshot = dw_store_upgrade::latest_snapshot(net_dir).unwrap().unwrap();
                        let path = snapshot.dir().to_path_buf();
                        let moved = path.with_file_name("moved-backup");
                        std::fs::rename(&path, &moved).unwrap();
                        Some((path, moved))
                    } else {
                        None
                    };
                    #[cfg(unix)]
                    let redirected = (step == "remove-main-with-redirected-backup").then(|| {
                        let path = session.data_dir().join(UPGRADE_BACKUPS_DIR);
                        let target = root.join("redirected-backup");
                        std::fs::rename(&path, &target).unwrap();
                        std::os::unix::fs::symlink(&target, &path).unwrap();
                        (path, target)
                    });
                    let grant = engine.block_on(session.vault_op(move |vault| vault.authorize(
                        GrantPurpose::Wipe, Some(&wallet.0), Credential::Passphrase(&passphrase),
                    ))).unwrap();
                    let result = engine.block_on(session.remove_wallet(wallet, grant.id));
                    if let Some((path, moved)) = moved {
                        std::fs::rename(moved, path).unwrap();
                    }
                    #[cfg(unix)]
                    if let Some((path, target)) = redirected {
                        std::fs::remove_file(&path).unwrap();
                        std::fs::rename(target, path).unwrap();
                    }
                    if result.is_err() {
                        assert!(session.vault().has_wallet_secret(&wallet.0));
                    }
                    result.map_or_else(err, |()| "removed".to_string())
                } else {
                    engine.block_on(session.vault_op(move |vault| vault.unlock(&passphrase, UnlockScope::Full))).unwrap();
                    let id = engine.block_on(session.import_wallet(
                        Zeroizing::new(b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about".to_vec()),
                        Zeroizing::new(Vec::new()),
                        dw_engine::ImportOptions {name: Some("Reimported main".into()), birth_height: Some(0), ..Default::default()},
                    )).unwrap();
                    assert_eq!(id, wallet);
                    "imported".to_string()
                }
            }
            other => panic!("no step {other}"),
        });
    }
    close(engine);
    println!("{RESULT}{}", out.join(" | "));
}

/// Runs `steps` in a child process on `root` ([`child_steps`]).
fn steps_in_child(root: &Path, steps: &[&str]) -> Vec<String> {
    child_result("child_steps", root, &[(CHILD_STEPS, &steps.join(","))])
        .split(" | ")
        .map(str::to_string)
        .collect()
}

const RESTORE_SCREEN: &str = "refused Restore storeUpgrade.refused.restore storeUpgrade.refused.report storeUpgrade.refused.quit";
const RESTORE_HELD_SCREEN: &str = "refused Restore storeUpgrade.refused.restore \
     storeUpgrade.refused.recover storeUpgrade.refused.report storeUpgrade.refused.quit";
const RETIRED_SCREEN: &str = "refused Unverified storeUpgrade.refused.wayOut.retired \
     storeUpgrade.refused.recover \
     storeUpgrade.refused.report storeUpgrade.refused.quit";

/// An upgraded fixture whose open is refused with Restore earlier data (its
/// schema history no longer matches), with a good copy of its wallet file
/// in the root for `heal`.
fn refused_with_restore() -> Fixture {
    let f = upgraded();
    break_history(&f);
    f
}

/// R-1 rule 4 (a): on the Restore screen with a shielded hold on disk,
/// Restore earlier data is refused, and Set up from recovery phrase then
/// starts the network over in a new folder that holds only the RESTORED
/// mark; the refused folder stays byte-identical.
#[test]
fn a_restore_refused_for_a_hold_on_the_restore_screen_lets_the_network_start_over() {
    let f = refused_with_restore();
    add_pending_row(&f.net);
    let pristine = tree(&f.net);
    let got = steps_in_child(
        &f.root,
        &["recover", "open", "recover", "request", "recover"],
    );
    assert_eq!(
        got,
        [
            "err invalid_argument",
            RESTORE_SCREEN,
            "err invalid_argument",
            "held InFlight",
            "recovered regtest-recovered-1",
        ]
    );
    assert_eq!(
        refused_open_diff(&pristine, &tree(&f.net)),
        Vec::<String>::new(),
        "the refused folder is as it was: only the refused open's own snapshot is new"
    );
    let fresh = f.root.join("regtest-recovered-1");
    assert_eq!(
        tree(&fresh).into_keys().collect::<Vec<_>>(),
        [
            ".".to_string(),
            UPGRADE_BACKUPS_DIR.to_string(),
            format!("{UPGRADE_BACKUPS_DIR}/{}", dw_store_upgrade::RESTORED_MARK)
        ],
        "the new folder holds only the mark"
    );
    let engine = engine(&f.root);
    assert_eq!(engine.network_dir(&DashNetwork::Regtest).unwrap(), fresh);
    close(engine);
}

/// The same Restore screen without the record keeps its own way out:
/// nothing held, the request goes through and recover is refused; and a
/// later open that is not refused withdraws the record.
#[test]
fn recover_needs_the_hold_record_and_a_later_open_withdraws_it() {
    let f = refused_with_restore();
    let got = steps_in_child(&f.root, &["open", "request", "recover", "cancel"]);
    assert_eq!(
        got,
        [
            RESTORE_SCREEN,
            "requested",
            "err invalid_argument",
            "cancelled"
        ]
    );
    assert_eq!(dw_store_upgrade::pending_restore(&f.net).unwrap(), None);

    add_pending_row(&f.net);
    let got = steps_in_child(
        &f.root,
        &[
            "open", "request", "open", "heal", "open", "close", "recover",
        ],
    );
    assert_eq!(
        got,
        [
            RESTORE_SCREEN,
            "held InFlight",
            RESTORE_HELD_SCREEN,
            "healed",
            "ok",
            "closed true",
            "err invalid_argument",
        ]
    );
    assert!(!f.root.join("regtest-recovered-1").exists());
}

/// M1: Restore earlier data refused for a hold keeps the Restore screen's
/// request: once the hold is gone, the same engine's request goes through.
#[test]
fn a_request_after_a_held_one_goes_through_once_the_hold_is_gone() {
    let f = refused_with_restore();
    add_pending_row(&f.net);
    let got = steps_in_child(&f.root, &["open", "request", "unhold", "request"]);
    assert_eq!(
        got,
        [RESTORE_SCREEN, "held InFlight", "unheld", "requested"]
    );
    assert_eq!(
        dw_store_upgrade::pending_restore(&f.net).unwrap(),
        Some(dw_store_upgrade::PendingRestore::Requested)
    );
}

/// R-1 rule 4 (a) at the swap: a hold that appears after the request stops
/// the next start's restore (notStarted); after Cancel restore, the open's
/// Restore screen offers Set up from recovery phrase, and it starts over.
#[test]
fn a_swap_refused_for_a_hold_offers_starting_over_after_cancel_restore() {
    let f = refused_with_restore();
    let got = steps_in_child(&f.root, &["open", "request"]);
    assert_eq!(got, [RESTORE_SCREEN, "requested"]);
    add_pending_row(&f.net);
    let got = steps_in_child(&f.root, &["open", "cancel", "open", "recover"]);
    assert_eq!(
        got,
        [
            "restore NotStarted shieldedSpendInFlight",
            "cancelled",
            RESTORE_HELD_SCREEN,
            "recovered regtest-recovered-1",
        ]
    );
    assert_eq!(after_dirs(&f), 0, "nothing was restored");
    assert!(!dw_store_upgrade::restored_mark_present(&f.net));
}

/// The live fork disposition of [`fork_fixture`]'s wallet, nullifier
/// `[n; 32]`, written as a fresh fork store over the live one: the store as
/// an open that stopped before its drain leaves it.
fn live_fork_store(f: &Fixture, n: u8) {
    use dw_shielded_drain::test_support::{ForkRow, fork_store};
    let shielded = f.net.join("shielded.sqlite");
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let _ = std::fs::remove_file(f.net.join(format!("shielded.sqlite{suffix}")));
    }
    fork_store(
        &shielded,
        &main_wallet(),
        0,
        &[ForkRow::Disposition {
            st: 2,
            kind: "unresolved",
            reason: Some("purged"),
            nullifiers: vec![[n; 32]],
        }],
    )
    .unwrap();
}

fn main_wallet() -> [u8; 32] {
    let mut wallet = [0; 32];
    hex::decode_to_slice(&MAIN[2..66], &mut wallet).unwrap();
    wallet
}

/// The marker the open's import of a fork hold writes for [`live_fork_store`]'s
/// disposition: activity `[2; 32]`, a zero anchor, nullifier `[n; 32]`.
fn fork_marker(f: &Fixture, n: u8) {
    marker(f, 2, &[n]);
}

/// A DEC-1057 marker of [`MAIN`], activity `[activity; 32]`, with a
/// nullifier `[n; 32]` for each of `notes`.
fn marker(f: &Fixture, activity: u8, notes: &[u8]) {
    let hex = |b: u8| format!("X'{}'", hex::encode([b; 32]));
    let a = hex(activity);
    let mut sql = format!(
        "INSERT INTO shielded_unrecorded (wallet, activity, anchor, at) \
         VALUES ({MAIN}, {a}, {z}, 1);",
        z = hex(0)
    );
    for n in notes {
        sql += &format!(
            "INSERT INTO shielded_unrecorded_nullifier (wallet, activity, nullifier, spent) \
             VALUES ({MAIN}, {a}, {n}, 0);",
            n = hex(*n)
        );
    }
    exec(&f.net.join("dispatch.sqlite"), &sql);
}

/// A v0.1.0 fixture with a fork disposition of nullifier `[102; 32]`,
/// upgraded and drained by a first start: its before set holds it.
fn fork_fixture() -> Fixture {
    let f = fixture("v0.1.0");
    live_fork_store(&f, 102);
    assert_eq!(open_in_child(&f.root), "ok");
    f
}

fn drained_holds(f: &Fixture) -> Vec<Vec<u8>> {
    let db = rusqlite::Connection::open(f.net.join("dispatch.sqlite")).unwrap();
    let mut stmt = db
        .prepare("SELECT nullifier FROM shielded_drained_hold WHERE spent = 0")
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Restore earlier data asked for from `f`'s Restore screen
/// ([`on_restore_screen`]), the engine closed after.
fn request_got(f: &Fixture) -> Result<(), EngineError> {
    let engine = on_restore_screen(f);
    let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
    close(engine);
    got
}

/// R-1 (N): a live fork disposition and its imported marker that the
/// snapshot's shielded store carries (same wallet and nullifiers) don't hold
/// the restore: it runs, and its first open holds the notes again.
#[test]
fn a_fork_hold_the_snapshot_carries_does_not_hold_the_restore() {
    let f = fork_fixture();
    live_fork_store(&f, 102);
    fork_marker(&f, 102);
    request(&f);
    assert_eq!(open_in_child(&f.root), "ok");
    assert!(dw_store_upgrade::restored_mark_present(&f.net));
    assert_eq!(after_dirs(&f), 1);
    assert_eq!(
        drained_holds(&f),
        [vec![102u8; 32]],
        "held again after the restore"
    );
}

/// R-1 (N): a fork hold only the live stores have, or one the snapshot's
/// shielded store can't vouch for, holds the restore.
#[test]
fn a_fork_hold_the_snapshot_does_not_carry_holds_the_restore() {
    let held = |f: &Fixture, engine: Engine, what: &str| {
        let before = tree(&f.net);
        let got = engine.block_on(engine.request_restore(&DashNetwork::Regtest));
        close(engine);
        assert!(
            matches!(
                got,
                Err(EngineError::ShieldedSpendHeld(
                    dw_engine::ShieldedHold::CarriedOver
                ))
            ),
            "{what}: {got:?}"
        );
        assert_eq!(
            tree_diff(&before, &tree(&f.net)),
            Vec::<String>::new(),
            "{what}"
        );
    };
    let f = fork_fixture();
    live_fork_store(&f, 103);
    held(&f, on_restore_screen(&f), "a live disposition only");

    for (what, activity, notes) in [
        ("a marker only", 2, &[104][..]),
        ("a marker of another activity", 3, &[102]),
        ("a marker of more notes", 2, &[102, 103]),
        ("a marker with no notes", 2, &[]),
    ] {
        let f = fork_fixture();
        marker(&f, activity, notes);
        held(&f, on_restore_screen(&f), what);
    }

    let f = fork_fixture();
    live_fork_store(&f, 102);
    fork_marker(&f, 102);
    let engine = on_restore_screen(&f);
    let snapshot = dw_store_upgrade::latest_snapshot(&f.net).unwrap().unwrap();
    let copy = snapshot.dir().join("shielded.sqlite");
    let mut bytes = std::fs::read(&copy).unwrap();
    bytes[..100].fill(0x5a);
    std::fs::write(&copy, bytes).unwrap();
    held(&f, engine, "the snapshot's shielded store does not verify");
}
