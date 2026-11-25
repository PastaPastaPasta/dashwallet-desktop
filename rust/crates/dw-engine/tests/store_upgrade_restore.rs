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
fn a_refusal_after_a_restore_says_the_files_changed() {
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
