//! R3 restore on restart (design v2.1, Q92 v2.1): every stop point resumes
//! to the before set, files another build wrote are copied aside before
//! anything live changes (V1), S4 keeps what is live, and a crashed attempt
//! removes only the dirs its record may name (92-4, 92-5).

use std::sync::OnceLock;

use super::*;
use crate::copy;
use crate::error::StoreUpgradeError;
use crate::fsx::{arm_fault, disarm_fault};
use crate::precheck::{Precheck, precheck_with};
use crate::reconcile::reconcile_fork_store;
use crate::restore::{
    FAIL_CLEANUP, IncompleteCause, KEPT, MOVE_SIDES_ON_KEEP, RESTORE_ATTEMPT, RESTORE_JOURNAL,
    RESTORE_REQUESTED, RESTORED_MARK, RefusalCause, RestoreInfo, S4_FOR_TEST, S4Policy,
    SAFE_TO_DELETE, SIDE_BEFORE_REPLACE, keep_current_files, request_restore, restored_mark,
    run_pending_restore,
};
use crate::snapshot::{
    STORE_FILES, SnapshotInfo, SnapshotState, UPGRADE_BACKUPS_DIR, latest_snapshot,
    snapshot_present, snapshot_state, take_snapshot,
};

type Hashes = BTreeMap<String, Option<String>>;

/// A v0.2.0 fixture after one upgrade start (snapshot, reconcile, open),
/// every store's log folded in, built once and copied per test.
struct Upgraded {
    _tmp: tempfile::TempDir,
    net: PathBuf,
    snapshot: SnapshotInfo,
}

impl Upgraded {
    fn root(&self) -> PathBuf {
        self.net.join(UPGRADE_BACKUPS_DIR)
    }

    fn wallet(&self) -> PathBuf {
        self.net.join("wallet.sqlite")
    }

    fn request(&self) {
        request_restore(&self.net, &self.snapshot).unwrap();
    }

    fn run(&self, at: SystemTime) -> crate::Result<Option<RestoreInfo>> {
        run_pending_restore(&self.net, at)
    }

    fn live(&self) -> Hashes {
        hashes_in(&self.net)
    }

    fn before(&self) -> Hashes {
        STORE_FILES
            .iter()
            .map(|n| {
                let sha = self
                    .snapshot
                    .files()
                    .iter()
                    .find(|f| f.name == *n)
                    .map(|f| f.sha256.clone());
                (n.to_string(), sha)
            })
            .collect()
    }

    fn sides(&self) -> Vec<String> {
        side_names(&self.net)
    }

    /// Stop with every store replaced, the request removed and the journal
    /// still there: the S4 stop point.
    fn stop_at_s4(&self) {
        self.request();
        arm_fault("restore.before_journal_removal", 0);
        let err = self.run(now()).unwrap_err();
        disarm_fault();
        assert!(matches!(
            err,
            StoreUpgradeError::InjectedFault("restore.before_journal_removal")
        ));
        assert_eq!(self.live(), self.before());
        assert!(self.root().join(RESTORE_JOURNAL).exists());
    }
}

fn hashes_in(dir: &Path) -> Hashes {
    STORE_FILES
        .iter()
        .map(|n| {
            let p = dir.join(n);
            (
                n.to_string(),
                p.exists().then(|| crate::sql::sha256_file(&p).unwrap()),
            )
        })
        .collect()
}

fn side_names(net: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for n in STORE_FILES {
        for s in crate::snapshot::SIDE_SUFFIXES {
            if net.join(format!("{n}{s}")).exists() {
                out.push(format!("{n}{s}"));
            }
        }
    }
    out
}

fn fold_logs(net: &Path) {
    for n in STORE_FILES {
        let p = net.join(n);
        if p.exists() {
            let conn = Connection::open(&p).unwrap();
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
                .unwrap();
            conn.close().unwrap();
        }
    }
    assert!(side_names(net).is_empty(), "{:?}", side_names(net));
}

/// Upgrade a v0.2.0 fixture after `prep` changed its live set.
fn upgrade_fixture(prep: impl FnOnce(&Path)) -> (tempfile::TempDir, PathBuf) {
    let f = fixture("v0.2.0");
    prep(&f.wallet());
    assert!(matches!(
        precheck_with(&f.net, build()).unwrap(),
        Precheck::Fork { .. }
    ));
    let snapshot = take_snapshot(&f.net, now()).unwrap();
    reconcile_fork_store(&config(&f.net), &snapshot, build()).unwrap();
    drop(open_and_load(&f.net));
    crate::token::reset_opened_for_test();
    fold_logs(&f.net);
    let tmp = private_tempdir();
    let net = tmp.path().join("regtest");
    copy_tree(&f.net, &net);
    (tmp, net)
}

fn copy_of(base: &Path) -> Upgraded {
    let tmp = private_tempdir();
    let net = tmp.path().join("regtest");
    copy_tree(base, &net);
    let snapshot = latest_snapshot(&net).unwrap().expect("a snapshot");
    let u = Upgraded {
        _tmp: tmp,
        net,
        snapshot,
    };
    assert_ne!(u.live(), u.before(), "the fixture's live set is upgraded");
    u
}

fn upgraded() -> Upgraded {
    static BASE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    copy_of(&BASE.get_or_init(|| upgrade_fixture(|_| {})).1)
}

/// The restored end state: the before set live with no side file, the
/// request and journal gone, the mark set, and `after-upgrade` holding the
/// set that was live.
fn assert_restored(u: &Upgraded, was_live: &Hashes, info: &RestoreInfo) {
    assert_eq!(u.live(), u.before());
    assert!(u.sides().is_empty(), "{:?}", u.sides());
    assert!(!u.root().join(RESTORE_REQUESTED).exists());
    assert!(!u.root().join(RESTORE_JOURNAL).exists());
    assert!(!u.root().join(RESTORE_ATTEMPT).exists());
    assert!(u.root().join(RESTORED_MARK).exists());
    assert_eq!(&hashes_in(&info.after_dir), was_live);
    assert!(!info.kept_live);
}

/// No live store's main file is ever missing (D2, V1).
fn assert_all_live(u: &Upgraded) {
    for n in STORE_FILES {
        assert!(u.net.join(n).exists(), "{n} is missing");
    }
}

fn during_dirs(u: &Upgraded) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(u.root())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| file_name(p).starts_with("during-restore-"))
        .collect();
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Every file's bytes under `dir`, by sha256.
fn contents(dir: &Path) -> std::collections::BTreeSet<String> {
    tree(dir).into_values().filter(|v| v != "dir").collect()
}

#[test]
fn fresh_restore_puts_back_the_before_set() {
    let u = upgraded();
    let was_live = u.live();
    u.request();
    let snapshot_tree = tree(u.snapshot.dir());
    let info = u.run(now()).unwrap().expect("a restore ran");
    assert_restored(&u, &was_live, &info);
    assert!(!info.resumed);
    assert!(info.during_dirs.is_empty());
    assert_eq!(
        tree(u.snapshot.dir()),
        snapshot_tree,
        "the snapshot is kept"
    );
    assert!(restored_mark(&u.net).unwrap().is_some());
    assert!(matches!(u.run(later(60)), Ok(None)), "nothing left to do");
}

/// Step 6 and H3's cross-check: the mark lists the before set's unmined
/// payments with the coins they spend, in the store's own outpoint encoding.
#[test]
fn fresh_restore_marks_the_before_sets_pending_payments() {
    let mut paid = None;
    let (_tmp, base) = upgrade_fixture(|wallet| {
        let conn = Connection::open(wallet).unwrap();
        paid = Some(pay(&conn));
    });
    let (txid, outpoint) = paid.unwrap();
    let u = copy_of(&base);
    u.request();
    u.run(now()).unwrap().unwrap();
    assert!(mark_lists(&u, &txid, &outpoint));
    assert_eq!(restored_mark(&u.net).unwrap().unwrap().pending.len(), 1);
}

/// Each injected stop, started again, ends where an uninterrupted run does,
/// with every live store present at the stop.
#[test]
fn resume_at_each_stop_point_ends_restored() {
    let points: &[(&'static str, u32)] = &[
        ("restore.after_quiesce", 0),
        ("restore.copy", 0),
        ("restore.copy", 5),
        ("restore.after_journal", 0),
        ("restore.rename", 0),
        ("restore.rename", 3),
        ("restore.replaced", 0),
        ("restore.replaced", 2),
        ("restore.replaced", 3),
        ("restore.before_journal_removal", 0),
    ];
    for &(point, skip) in points {
        let u = upgraded();
        let was_live = u.live();
        u.request();
        arm_fault(point, skip);
        let err = u.run(now()).unwrap_err();
        disarm_fault();
        assert_all_live(&u);
        let journal = u.root().join(RESTORE_JOURNAL).exists();
        if !journal {
            assert_eq!(u.live(), was_live, "{point}/{skip}: nothing changed");
            assert!(
                !u.root().join(RESTORE_ATTEMPT).exists(),
                "{point}/{skip}: {err}"
            );
        }
        if !u.root().join(RESTORE_REQUESTED).exists() && !journal {
            assert!(
                matches!(err, StoreUpgradeError::RestoreNoSpace { .. }),
                "{point}/{skip}: {err}"
            );
            u.request();
        }
        let info = u.run(later(60)).unwrap().expect("resumed");
        assert_restored(&u, &was_live, &info);
        assert_eq!(info.resumed, journal, "{point}/{skip}");
    }
}

/// Garbage another build left beside the after set mid-restore.
fn write_sides_mid_restore(u: &Upgraded) -> Vec<Vec<u8>> {
    let mut written = Vec::new();
    for (i, name) in ["app.sqlite-wal", "wallet.sqlite-wal"].iter().enumerate() {
        let bytes = format!("written by another build {i}").into_bytes();
        fs::write(u.net.join(name), &bytes).unwrap();
        written.push(bytes);
    }
    written
}

/// V1: at every sub-step of a resume that finds side files, each live store
/// is present and every byte another build wrote is live or in a
/// `during-restore-*` copy; the second interrupted resume uses a new dir.
#[test]
fn v1_copy_first_keeps_every_write_at_each_sub_step() {
    let points: &[(&'static str, u32)] = &[
        ("restore.settle", 0),
        ("restore.settle", 1),
        ("restore.settle", 2),
        ("restore.rename", 0),
        ("restore.rename", 1),
        ("restore.rename", 2),
        ("restore.replaced", 0),
        ("restore.copy", 1),
    ];
    for &(point, skip) in points {
        let u = upgraded();
        let was_live = u.live();
        u.request();
        arm_fault("restore.after_journal", 0);
        u.run(now()).unwrap_err();
        disarm_fault();
        let written = write_sides_mid_restore(&u);
        let wanted: Vec<String> = written
            .iter()
            .map(|b| {
                use sha2::Digest;
                sha2::Sha256::digest(b)
                    .iter()
                    .map(|x| format!("{x:02x}"))
                    .collect()
            })
            .collect();
        arm_fault(point, skip);
        let err = u.run(later(60)).unwrap_err();
        disarm_fault();
        assert!(
            matches!(
                err,
                StoreUpgradeError::RestoreIncomplete {
                    journal_written: true,
                    ..
                }
            ),
            "{point}/{skip}: {err}"
        );
        assert_all_live(&u);
        let mut kept = contents(&u.net);
        for d in during_dirs(&u) {
            kept.extend(contents(&d));
        }
        for w in &wanted {
            assert!(kept.contains(w), "{point}/{skip}: a write was lost");
        }
        let first = during_dirs(&u);
        let info = u.run(later(60)).unwrap().expect("resumed");
        assert_restored(&u, &was_live, &info);
        let all = during_dirs(&u);
        let mut kept = std::collections::BTreeSet::new();
        for d in &all {
            kept.extend(contents(d));
            assert!(d.join(KEPT).exists());
        }
        for w in &wanted {
            assert!(
                kept.contains(w),
                "{point}/{skip}: a write was lost at the end"
            );
        }
        if !first.is_empty() && !info.during_dirs.is_empty() {
            assert!(
                !first.contains(&info.during_dirs[0]),
                "{point}/{skip}: the second resume uses its own dir"
            );
        }
    }
}

/// The fail-first for V1's side handling: with the side files left in place
/// the resume can't finish, and no live main file is touched.
#[test]
fn skipping_the_side_files_leaves_the_restore_unfinished() {
    let u = upgraded();
    u.request();
    arm_fault("restore.after_journal", 0);
    u.run(now()).unwrap_err();
    disarm_fault();
    write_sides_mid_restore(&u);
    crate::restore::SKIP_SIDE_MOVE.with(|s| s.set(true));
    let live = u.run(later(60));
    crate::restore::SKIP_SIDE_MOVE.with(|s| s.set(false));
    assert!(
        matches!(
            live,
            Err(StoreUpgradeError::RestoreIncomplete {
                journal_written: true,
                ..
            })
        ),
        "{live:?}"
    );
    assert_eq!(u.sides().len(), 2, "the old build's logs are still live");
    assert_all_live(&u);
    assert!(during_dirs(&u).is_empty(), "and no copy of them was kept");
}

/// S4-4: a side file that appears beside a store just before its replace
/// stops the fresh run with the journal kept; the resume copies it aside.
#[test]
fn a_side_file_before_a_replace_is_kept_aside() {
    for store in [
        "app.sqlite",
        "dispatch.sqlite",
        "shielded.sqlite",
        "wallet.sqlite",
    ] {
        let u = upgraded();
        let was_live = u.live();
        u.request();
        SIDE_BEFORE_REPLACE.with(|s| s.set(Some(store)));
        let err = u.run(now());
        SIDE_BEFORE_REPLACE.with(|s| s.set(None));
        assert!(
            matches!(
                err,
                Err(StoreUpgradeError::RestoreIncomplete {
                    journal_written: true,
                    ..
                })
            ),
            "{store}: {err:?}"
        );
        assert_all_live(&u);
        let info = u.run(later(60)).unwrap().unwrap();
        assert_restored(&u, &was_live, &info);
        let kept: std::collections::BTreeSet<String> =
            info.during_dirs.iter().flat_map(|d| contents(d)).collect();
        let want = {
            use sha2::Digest;
            sha2::Sha256::digest(b"written mid-replace")
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>()
        };
        assert!(kept.contains(&want), "{store}: the side file was kept");
    }
}

#[test]
fn s4_untouched_finishes_normally() {
    let u = upgraded();
    let was_live = u.live();
    u.stop_at_s4();
    let info = u.run(later(60)).unwrap().unwrap();
    assert_restored(&u, &was_live, &info);
    assert!(info.resumed);
}

/// One unmined payment spending a wallet coin: its txid and the coin.
fn pay(conn: &Connection) -> (Vec<u8>, Vec<u8>) {
    let (wid, outpoint): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT wallet_id, outpoint FROM core_utxos ORDER BY spent, outpoint LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let txid: Vec<u8> = (1u8..=32).collect();
    conn.execute(
        "INSERT INTO core_transactions (wallet_id, txid, height, finalized) VALUES (?1, ?2, NULL, 0)",
        (&wid, &txid),
    )
    .unwrap();
    conn.execute(
        "UPDATE core_utxos SET spent = 1, spent_in_txid = ?1 WHERE wallet_id = ?2 AND outpoint = ?3",
        (&txid, &wid, &outpoint),
    )
    .unwrap();
    (txid, outpoint)
}

/// Write one pending payment into the live wallet's WAL only, as an old
/// build that stopped before a checkpoint leaves it. Returns its txid and
/// the coin it spends.
fn pay_in_wal_only(wallet: &Path) -> (Vec<u8>, Vec<u8>) {
    let wal = wallet.with_file_name("wallet.sqlite-wal");
    let conn = Connection::open(wallet).unwrap();
    conn.pragma_update(None, "journal_mode", "WAL").unwrap();
    conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    let main_before = fs::read(wallet).unwrap();
    let (txid, outpoint) = pay(&conn);
    let main = fs::read(wallet).unwrap();
    let log = fs::read(&wal).unwrap();
    assert_eq!(main, main_before, "the payment is in the WAL only");
    drop(conn);
    fs::write(wallet, main).unwrap();
    fs::write(&wal, log).unwrap();
    let _ = fs::remove_file(wallet.with_file_name("wallet.sqlite-shm"));
    (txid, outpoint)
}

fn payment_visible(wallet: &Path, txid: &[u8]) -> bool {
    let conn = Connection::open(wallet).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM core_transactions WHERE txid = ?1 AND height IS NULL",
        [txid],
        |r| r.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn mark_lists(u: &Upgraded, txid: &[u8], outpoint: &[u8]) -> bool {
    let mark = restored_mark(&u.net).unwrap().expect("the mark stays");
    mark.pending.iter().any(|p| {
        dashcore::consensus::serialize(&p.txid) == txid
            && p.inputs.iter().any(|o| {
                bincode::serde::encode_to_vec(o, bincode::config::standard()).unwrap() == outpoint
            })
    })
}

/// 92-1, S4-3, S4-5: KeepLive is the default. A payment the old build left
/// in the wallet's WAL only stays visible, the side file is never moved, the
/// payment joins the mark, and a KEPT.txt line records the keep.
#[test]
fn s4_keep_live_keeps_a_wal_only_payment() {
    let u = upgraded();
    u.stop_at_s4();
    let (txid, outpoint) = pay_in_wal_only(&u.wallet());
    let live_tree = tree(&u.net);
    let info = u.run(later(60)).unwrap().unwrap();
    assert!(info.kept_live && info.resumed);
    assert!(!u.root().join(RESTORE_REQUESTED).exists());
    assert!(!u.root().join(RESTORE_JOURNAL).exists());
    assert!(u.root().join(RESTORED_MARK).exists(), "the mark stays (H4)");
    let changed = tree_diff(&live_tree, &tree(&u.net));
    let during = file_name(&info.during_dirs[0]);
    for path in &changed {
        let ok = path.starts_with(&format!("{UPGRADE_BACKUPS_DIR}/{during}"))
            || path == &format!("{UPGRADE_BACKUPS_DIR}/{RESTORE_JOURNAL}")
            || path == &format!("{UPGRADE_BACKUPS_DIR}/{RESTORED_MARK}");
        assert!(ok, "Keep changed {path}");
    }
    let kept = fs::read_to_string(info.during_dirs[0].join(KEPT)).unwrap();
    assert!(
        kept.lines()
            .any(|l| l == "kept live: opened by another build after the swap finished"),
        "{kept}"
    );
    assert!(u.net.join("wallet.sqlite-wal").exists(), "the WAL is live");
    assert!(mark_lists(&u, &txid, &outpoint), "the payment is held");
    assert!(
        payment_visible(&u.wallet(), &txid),
        "the payment is visible"
    );
}

/// The S4-5 fail-first: moving the side files aside loses the payment.
#[test]
fn s4_moving_the_side_files_aside_loses_the_payment() {
    let u = upgraded();
    u.stop_at_s4();
    let (txid, outpoint) = pay_in_wal_only(&u.wallet());
    MOVE_SIDES_ON_KEEP.with(|m| m.set(true));
    let info = u.run(later(60));
    MOVE_SIDES_ON_KEEP.with(|m| m.set(false));
    assert!(info.unwrap().unwrap().kept_live);
    assert!(!mark_lists(&u, &txid, &outpoint));
    assert!(!payment_visible(&u.wallet(), &txid));
}

#[test]
fn s4_refuse_policy_changes_nothing() {
    let u = upgraded();
    u.stop_at_s4();
    pay_in_wal_only(&u.wallet());
    let before = tree(&u.net);
    S4_FOR_TEST.with(|p| p.set(Some(S4Policy::Refuse)));
    let err = u.run(later(60));
    S4_FOR_TEST.with(|p| p.set(None));
    assert!(matches!(
        err,
        Err(StoreUpgradeError::RestoreIncomplete {
            cause: IncompleteCause::S4OldBuildWrote,
            journal_written: true,
            ..
        })
    ));
    assert_eq!(tree_diff(&before, &tree(&u.net)), Vec::<String>::new());
}

/// A wallet with the before set's version and wallets ids, but another
/// main file, is in the lineage (S4-2): kept live.
#[test]
fn s4_rewritten_wallet_with_the_same_ids_is_kept() {
    let u = upgraded();
    u.stop_at_s4();
    exec(&u.wallet(), "CREATE TABLE written_by_old_build (a)");
    fold_logs(&u.net);
    assert_ne!(u.live(), u.before());
    let info = u.run(later(60)).unwrap().unwrap();
    assert!(info.kept_live);
}

/// 92-2: a live wallet outside the before lineage is refused with nothing
/// moved, and the keep action then runs KeepLive with every existing file
/// in place (MG Q93 condition (2)).
#[test]
fn s4_other_lineage_refuses_with_keep_action() {
    for foreign in ["another wallet id", "the after file"] {
        let u = upgraded();
        u.stop_at_s4();
        match foreign {
            "another wallet id" => {
                exec(
                    &u.wallet(),
                    "INSERT INTO wallets (wallet_id, network, birth_height) \
                     VALUES (x'00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff', \
                     'regtest', 0)",
                );
                fold_logs(&u.net);
            }
            _ => {
                let after = fs::read_dir(u.root())
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .find(|p| file_name(p).starts_with("after-upgrade-"))
                    .unwrap();
                fs::copy(after.join("wallet.sqlite"), u.wallet()).unwrap();
            }
        }
        fs::write(u.net.join("app.sqlite-wal"), b"another build's log").unwrap();
        let before = tree(&u.net);
        let err = u.run(later(60)).unwrap_err();
        assert!(
            matches!(
                err,
                StoreUpgradeError::RestoreIncomplete {
                    cause: IncompleteCause::S4OtherLineage,
                    journal_written: true,
                    ..
                }
            ),
            "{foreign}: {err}"
        );
        assert_eq!(tree_diff(&before, &tree(&u.net)), Vec::<String>::new());
        let screen = crate::copy::restore_screen_for(&err).expect("a screen");
        assert_eq!(screen, crate::copy::restore_foreign::screen(), "{foreign}");
        assert_eq!(screen.buttons[0].key, "storeUpgrade.restore.foreign.keep");
        assert!(
            screen
                .buttons
                .iter()
                .all(|b| !b.key.starts_with("storeUpgrade.cantOpen.")),
            "no key the cantOpen family may retire"
        );
        assert!(
            !screen.buttons[0].default_focus,
            "Keep is not the default focus"
        );
        let info = keep_current_files(&u.net, later(120))
            .unwrap()
            .expect("kept");
        assert!(info.kept_live, "{foreign}");
        let after_keep = tree(&u.net);
        for (path, sha) in &before {
            if path == &format!("{UPGRADE_BACKUPS_DIR}/{RESTORE_JOURNAL}")
                || path == &format!("{UPGRADE_BACKUPS_DIR}/{RESTORED_MARK}")
            {
                continue;
            }
            assert_eq!(
                after_keep.get(path),
                Some(sha),
                "{foreign}: Keep changed {path}"
            );
        }
        assert!(!u.root().join(RESTORE_JOURNAL).exists());
        assert!(u.root().join(RESTORED_MARK).exists());
        assert!(matches!(u.run(later(180)), Ok(None)));
        assert!(matches!(keep_current_files(&u.net, later(240)), Ok(None)));
    }
}

#[test]
fn keep_action_does_nothing_before_s4() {
    let u = upgraded();
    assert!(matches!(keep_current_files(&u.net, now()), Ok(None)));
    u.request();
    arm_fault("restore.replaced", 0);
    u.run(now()).unwrap_err();
    disarm_fault();
    let before = tree(&u.net);
    assert!(matches!(keep_current_files(&u.net, later(60)), Ok(None)));
    assert_eq!(tree_diff(&before, &tree(&u.net)), Vec::<String>::new());
}

fn write_attempt_record(u: &Upgraded, entries: &[String]) {
    let mut text = "dw-store-upgrade restore-attempt\nformat 1\n".to_string();
    for e in entries {
        text += e;
        text.push('\n');
    }
    fs::write(u.root().join(RESTORE_ATTEMPT), text).unwrap();
}

/// 92-4: a record naming anything but a fresh-name attempt dir directly
/// under upgrade-backups, or a symlink, is refused; nothing outside is
/// removed and the restore still runs.
#[test]
fn restore_attempt_record_outside_upgrade_backups_is_refused() {
    let u = upgraded();
    let was_live = u.live();
    let outside = u.net.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("wallet.sqlite"), b"not ours").unwrap();
    let link = "after-upgrade-20261125T004000Z";
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, u.root().join(link)).unwrap();
    let snapshot_name = file_name(u.snapshot.dir());
    write_attempt_record(
        &u,
        &[
            "path ../outside".into(),
            "path ../../outside".into(),
            format!("path {}", outside.display()),
            format!("path {snapshot_name}"),
            format!("path {link}"),
            "path restore-staging-20261125T004000Z/../../outside".into(),
            "path RESTORE-IN-PROGRESS".into(),
            "path RESTORE-REQUESTED".into(),
            "path during-restore-20261125T004000Z".into(),
            "path wallet.sqlite".into(),
        ],
    );
    let snapshot_tree = tree(u.snapshot.dir());
    u.request();
    let info = u.run(now()).unwrap().unwrap();
    assert_restored(&u, &was_live, &info);
    assert_eq!(
        fs::read(outside.join("wallet.sqlite")).unwrap(),
        b"not ours".to_vec()
    );
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(u.root().join(link))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink is refused, not followed or removed"
    );
    assert_eq!(tree(u.snapshot.dir()), snapshot_tree);
}

/// 92-5: a crash after the record but before the create leaves names that
/// don't exist; the sweep counts them as cleaned.
#[test]
fn crash_after_record_before_create_cleans_cleanly() {
    let u = upgraded();
    let was_live = u.live();
    let made = u.root().join("after-upgrade-20261125T004000Z");
    crate::fsx::create_private_dir(&made).unwrap();
    fs::write(made.join("ATTEMPT-ID"), "t1\n").unwrap();
    fs::write(made.join("app.sqlite.partial"), b"partial").unwrap();
    write_attempt_record(
        &u,
        &[
            "id t1".into(),
            "mark-created".into(),
            "path after-upgrade-20261125T004000Z".into(),
            "path restore-staging-20261125T004000Z".into(),
        ],
    );
    u.request();
    let info = u.run(now()).unwrap().unwrap();
    assert_restored(&u, &was_live, &info);
    assert!(!made.exists(), "the dir it did create is removed");
    assert!(!u.root().join("restore-staging-20261125T004000Z").exists());
}

/// A crash mid-copy leaves its dirs, a partial copy and a mark it created;
/// the next start removes exactly those.
#[test]
fn a_crashed_attempt_is_swept_by_the_next_start() {
    let u = upgraded();
    let was_live = u.live();
    let left = u.root().join("after-upgrade-20261125T004000Z");
    crate::fsx::create_private_dir(&left).unwrap();
    fs::write(left.join("ATTEMPT-ID"), "t1\n").unwrap();
    fs::write(left.join("app.sqlite"), b"partial").unwrap();
    fs::write(left.join("dispatch.sqlite.partial"), b"partial").unwrap();
    fs::write(
        u.root().join(RESTORED_MARK),
        "dw-store-upgrade restored\nformat 1\nattempt t1\n",
    )
    .unwrap();
    fs::write(u.root().join("RESTORED.partial"), b"partial").unwrap();
    write_attempt_record(
        &u,
        &[
            "id t1".into(),
            "mark-created".into(),
            "path after-upgrade-20261125T004000Z".into(),
        ],
    );
    u.request();
    let info = u.run(now()).unwrap().unwrap();
    assert_restored(&u, &was_live, &info);
    assert!(!left.exists());
    assert!(!u.root().join("RESTORED.partial").exists());
}

/// An earlier restore's after set and during dir (design §3: a failed
/// attempt must leave them alone).
fn seed_earlier_restore(u: &Upgraded) {
    for dir in [
        "after-upgrade-20261124T000000Z",
        "during-restore-20261124T000000Z",
    ] {
        let d = u.root().join(dir);
        crate::fsx::create_private_dir(&d).unwrap();
        fs::write(d.join("wallet.sqlite"), format!("earlier {dir}")).unwrap();
    }
}

/// Q102 102-1: a record that can't be removed stops the run with the journal
/// still there, wherever a run removes it after the journal (finish, a
/// resume's stale record, keep); the next start finishes and leaves no record
/// behind.
#[test]
fn a_record_removal_failure_is_restore_incomplete() {
    let fails = |u: &Upgraded, skip: u32, at: SystemTime, what: &str| {
        arm_fault("restore.remove_record", skip);
        let err = u.run(at).unwrap_err();
        disarm_fault();
        assert!(
            matches!(
                err,
                StoreUpgradeError::RestoreIncomplete {
                    journal_written: true,
                    ..
                }
            ),
            "{what}: {err:?}"
        );
        assert!(u.root().join(RESTORE_JOURNAL).exists(), "{what}");
    };
    let finishes = |u: &Upgraded, what: &str| {
        let info = u.run(later(180)).unwrap().unwrap();
        assert!(info.resumed, "{what}");
        for gone in [RESTORE_JOURNAL, RESTORE_ATTEMPT, RESTORE_REQUESTED] {
            assert!(!u.root().join(gone).exists(), "{what}: {gone}");
        }
        assert!(u.root().join(RESTORED_MARK).exists(), "{what}");
        assert!(!info.after_dir.join("ATTEMPT-ID").exists(), "{what}");
        info
    };

    let u = upgraded();
    u.request();
    fails(&u, 0, now(), "finish");
    write_attempt_record(&u, &[]);
    fails(&u, 0, later(60), "a resume's stale record");
    assert!(u.root().join(RESTORE_ATTEMPT).exists(), "resume");
    let info = finishes(&u, "after finish and resume");
    assert_eq!(u.live(), u.before());
    assert!(!info.kept_live);

    let u = upgraded();
    u.stop_at_s4();
    pay_in_wal_only(&u.wallet());
    fails(&u, 1, later(60), "keep");
    let info = finishes(&u, "after keep");
    assert!(info.kept_live);
}

/// C1: a record that outlived a finished restore (a failed unlink, or put
/// there by hand) never removes that restore's after set or its mark.
#[test]
fn a_stale_record_never_removes_a_finished_restore() {
    let u = upgraded();
    u.request();
    let info = u.run(now()).unwrap().unwrap();
    let after = file_name(&info.after_dir);
    let after_tree = tree(&info.after_dir);
    assert!(!info.after_dir.join("ATTEMPT-ID").exists());
    let mark = fs::read_to_string(u.root().join(RESTORED_MARK)).unwrap();
    assert!(!mark.contains("attempt "), "{mark}");
    for id in ["", "id anything"] {
        let mut entries = vec!["mark-created".to_string(), format!("path {after}")];
        if !id.is_empty() {
            entries.insert(0, id.to_string());
        }
        write_attempt_record(&u, &entries);
        let snap = latest_snapshot(&u.net).unwrap().unwrap();
        request_restore(&u.net, &snap).unwrap();
        let again = u.run(later(3600)).unwrap().unwrap();
        assert_eq!(tree(&info.after_dir), after_tree, "{id:?}");
        assert!(u.root().join(RESTORED_MARK).exists(), "{id:?}");
        assert_ne!(again.after_dir, info.after_dir);
    }
}

/// ENOSPC before the journal: RestoreNoSpace, the request cleared, and
/// nothing else in the network dir changed.
#[test]
fn enospc_cleanup_removes_only_this_attempts_copies() {
    for skip in [0, 2, 6] {
        let u = upgraded();
        seed_earlier_restore(&u);
        u.request();
        let start = tree(&u.net);
        arm_fault("restore.copy", skip);
        let err = u.run(now()).unwrap_err();
        disarm_fault();
        assert!(
            matches!(err, StoreUpgradeError::RestoreNoSpace { needed: Some(_) }),
            "{skip}: {err}"
        );
        assert_eq!(
            tree_diff(&start, &tree(&u.net)),
            vec![format!("{UPGRADE_BACKUPS_DIR}/{RESTORE_REQUESTED}")],
            "{skip}"
        );
    }
}

/// The cleanup itself fails: each of the attempt's dirs gets the MG note,
/// the record goes, and the live set is untouched.
#[test]
fn enospc_cleanup_failure_leaves_a_safe_to_delete_manifest() {
    let u = upgraded();
    let was_live = u.live();
    seed_earlier_restore(&u);
    u.request();
    arm_fault("restore.copy", 6);
    FAIL_CLEANUP.with(|f| f.set(true));
    let err = u.run(now());
    FAIL_CLEANUP.with(|f| f.set(false));
    disarm_fault();
    assert!(matches!(err, Err(StoreUpgradeError::RestoreNoSpace { .. })));
    assert_eq!(u.live(), was_live);
    assert!(!u.root().join(RESTORE_ATTEMPT).exists());
    let notes: Vec<PathBuf> = fs::read_dir(u.root())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join(SAFE_TO_DELETE).exists())
        .collect();
    assert_eq!(
        notes.len(),
        1,
        "the partial staging dir; never a complete after set"
    );
    for d in &notes {
        let name = file_name(d);
        assert!(name.starts_with("restore-staging-"), "{name}");
        let text = fs::read_to_string(d.join(SAFE_TO_DELETE)).unwrap();
        assert!(
            text.starts_with("This folder is an unfinished copy"),
            "{text}"
        );
    }
    u.request();
    let info = u.run(later(60)).unwrap().unwrap();
    assert_restored(&u, &was_live, &info);
    for d in &notes {
        assert!(d.exists(), "a later run leaves a noted dir alone");
    }
}

/// Q98 98-5: only a verified snapshot is a backup to the refusal screens; an
/// unverified one (a corrupt byte, a garbled marker) is Unverified, and none
/// at all (no snapshot, or one without its marker) is None.
#[test]
fn only_a_verified_snapshot_gets_the_backup_sentence() {
    use crate::copy::{BackupState, ProvedState};
    let key = |net: &Path| {
        let state = snapshot_state(net).unwrap();
        let backup = BackupState::from(&state);
        assert_eq!(ProvedState::read(net, false).backup, Some(backup));
        assert_eq!(
            snapshot_present(net).unwrap(),
            backup == BackupState::Verified
        );
        (state, backup)
    };
    let with = BackupState::Verified;
    let without = BackupState::Unverified;
    let none = BackupState::None;

    let u = upgraded();
    let (state, k) = key(&u.net);
    assert!(matches!(state, SnapshotState::Verified(_)));
    assert_eq!(k, with);

    let file = u.snapshot.dir().join("wallet.sqlite");
    let mut bytes = fs::read(&file).unwrap();
    bytes[100] ^= 0x80;
    fs::write(&file, bytes).unwrap();
    let (state, k) = key(&u.net);
    assert!(
        matches!(state, SnapshotState::Unverified { .. }),
        "{state:?}"
    );
    assert_eq!(k, without, "a corrupt byte");

    let u = upgraded();
    let marker = u.snapshot.dir().join(crate::snapshot::MARKER);
    let text = fs::read_to_string(&marker).unwrap();
    fs::write(&marker, text.replacen("refinery-max", "refinery-mix", 1)).unwrap();
    let (state, k) = key(&u.net);
    assert!(
        matches!(state, SnapshotState::Unverified { .. }),
        "{state:?}"
    );
    assert_eq!(k, without, "a garbled marker");

    fs::remove_file(&marker).unwrap();
    let (state, k) = key(&u.net);
    assert_eq!(
        state,
        SnapshotState::None,
        "no marker: not a completed snapshot"
    );
    assert_eq!(k, none);

    fs::remove_dir_all(u.snapshot.dir()).unwrap();
    let (state, k) = key(&u.net);
    assert_eq!(state, SnapshotState::None);
    assert_eq!(k, none, "no snapshot");
}

/// One corrupt byte in a snapshot file makes it Unverified: no backup
/// sentence, and a restore requested from it is refused with nothing moved.
#[test]
fn snapshot_state_one_corrupt_byte_is_unverified() {
    let u = upgraded();
    assert!(matches!(
        snapshot_state(&u.net).unwrap(),
        SnapshotState::Verified(_)
    ));
    u.request();
    let file = u.snapshot.dir().join("dispatch.sqlite");
    let mut bytes = fs::read(&file).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0x01;
    fs::write(&file, bytes).unwrap();
    assert!(matches!(
        snapshot_state(&u.net).unwrap(),
        SnapshotState::Unverified { .. }
    ));
    assert!(!snapshot_present(&u.net).unwrap());
    let live = u.live();
    let err = u.run(now()).unwrap_err();
    assert!(
        matches!(
            err,
            StoreUpgradeError::RestoreRefused {
                cause: RefusalCause::MismatchedBackup | RefusalCause::MissingBackup
            }
        ),
        "{err}"
    );
    assert_eq!(u.live(), live);
    assert!(!u.root().join(RESTORE_REQUESTED).exists(), "MG (k)");
}

const FOLDER: &str = "/home/u/.local/share/Dash Wallet/regtest";

#[test]
fn restore_confirm_copy_is_verbatim_and_cancel_has_default_focus() {
    let c = copy::restore_confirm();
    assert_eq!(c.title_key, "storeUpgrade.restore.confirm.title");
    assert_eq!(c.title, "Restore wallet data from before the upgrade?");
    assert!(c.text.starts_with(
        "Dash Wallet restarts and puts back the wallet data it saved before the upgrade, so the wallet won't show anything recorded since then. "
    ));
    assert!(c.text.contains(&format!(
        "kept in the {UPGRADE_BACKUPS_DIR} folder inside {{folder}}. Don't delete it"
    )));
    assert!(c.text.contains(
        "An earlier version may offer coins this version is holding back, and a payment that uses them can fail.\n\n"
    ));
    assert!(c.text.ends_with(
        "\n\nIf Dash Wallet closes before the restore finishes, start this version again before you open an earlier one."
    ));
    let [restore, cancel] = &c.buttons[..] else {
        panic!("two buttons")
    };
    assert_eq!(
        (restore.label, restore.destructive, restore.default_focus),
        ("Restart and restore", true, false)
    );
    assert_eq!(
        (cancel.label, cancel.destructive, cancel.default_focus),
        ("Cancel", false, true)
    );
    assert!(
        c.render(FOLDER)
            .contains(&format!("folder inside {FOLDER}."))
    );
}

#[test]
fn restore_refusal_copy_renders_for_each_cause() {
    for cause in [
        RefusalCause::MissingBackup,
        RefusalCause::MismatchedBackup,
        RefusalCause::GarbledMarker,
    ] {
        let err = StoreUpgradeError::RestoreRefused { cause };
        let shown = copy::restore_screen_for(&err).unwrap().render(FOLDER);
        assert!(shown.starts_with("The earlier wallet data couldn't be restored\n\n"));
        assert!(shown.contains("so nothing was changed."));
        assert!(shown.ends_with("[OK]"));
    }
}

#[test]
fn restore_incomplete_copy_renders_for_each_cause() {
    for cause in [
        IncompleteCause::DiskFull,
        IncompleteCause::PermissionDenied,
        IncompleteCause::FileVanished,
        IncompleteCause::Other,
    ] {
        let err = StoreUpgradeError::RestoreIncomplete {
            cause,
            journal_written: true,
            detail: String::new(),
        };
        let screen = copy::restore_screen_for(&err).unwrap();
        assert_eq!(screen.title_key, "storeUpgrade.restore.incomplete.title");
        assert_eq!(screen.hint.unwrap().0, "settings.about.links.issuesHint");
        let shown = screen.render(FOLDER);
        assert!(shown.contains(&format!("hasn't opened the wallet data in {FOLDER}.")));
        assert!(shown.contains("don't delete anything there"));
        assert!(shown.contains("[Restart][Report a problem][Quit]"));
        assert!(!err.file_unchanged());

        let err = StoreUpgradeError::RestoreIncomplete {
            cause,
            journal_written: false,
            detail: String::new(),
        };
        let screen = copy::restore_screen_for(&err).unwrap();
        assert_eq!(screen.title_key, "storeUpgrade.restore.notStarted.title");
        assert!(err.file_unchanged());
    }
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::StorageFull)),
        IncompleteCause::DiskFull
    );
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        IncompleteCause::PermissionDenied
    );
    assert_eq!(
        IncompleteCause::of(&std::io::Error::from(std::io::ErrorKind::NotFound)),
        IncompleteCause::FileVanished
    );
}

#[test]
fn restore_no_space_copy_renders_with_and_without_size() {
    for needed in [Some(123_456), None] {
        let err = StoreUpgradeError::RestoreNoSpace { needed };
        let shown = copy::restore_screen_for(&err).unwrap().render(FOLDER);
        assert!(shown.starts_with("Not enough disk space to restore\n\n"));
        assert!(shown.contains(&format!("the disk that holds {FOLDER}, then")));
        assert!(shown.ends_with("[OK]"));
    }
}

#[test]
fn no_screen_says_funds_are_held_on_the_network() {
    let source = include_str!("../copy.rs");
    assert!(!source.contains("held on the network"));
    for s in copy::all_screens() {
        let all = [s.title, s.text]
            .into_iter()
            .chain(s.buttons.iter().map(|b| b.label))
            .chain(s.hint.map(|h| h.1));
        for text in all {
            assert!(!text.contains("held on the network"), "{text}");
            assert!(!text.contains("Your funds"), "{text}");
        }
    }
}

#[test]
fn placeholders_are_only_the_keys_mg_has_not_written() {
    let mut keys: Vec<&str> = copy::all_screens()
        .iter()
        .filter(|s| s.title.starts_with(copy::PLACEHOLDER))
        .map(|s| s.title_key)
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "storeUpgrade.restore.keepCurrent.title",
            "storeUpgrade.restore.notStarted.title",
        ]
    );
}

#[test]
fn touched_paths_name_every_marker_and_dir() {
    let all = crate::restore::touched_paths().join("\n");
    for name in [
        crate::restore::RESTORE_REQUESTED,
        crate::restore::RESTORE_JOURNAL,
        crate::restore::RESTORED_MARK,
        crate::restore::RESTORE_ATTEMPT,
        crate::restore::SAFE_TO_DELETE,
        crate::restore::KEPT,
        "after-upgrade-<UTC>",
        "restore-staging-<UTC>",
        "during-restore-<UTC>",
        "before-upgrade-<UTC>",
    ] {
        assert!(all.contains(name), "{name}");
    }
}

#[test]
fn restore_foreign_screen_renders_the_folder_in_its_hint() {
    let s = copy::restore_foreign::screen();
    assert!(copy::all_screens().contains(&s));
    let keys: Vec<_> = s.buttons.iter().map(|b| b.key).collect();
    assert_eq!(
        keys,
        [
            "storeUpgrade.restore.foreign.keep",
            "settings.about.links.issues",
            "storeUpgrade.restore.foreign.quit",
        ]
    );
    assert!(s.buttons.iter().all(|b| !b.default_focus && !b.destructive));
    let shown = s.render(FOLDER);
    assert!(shown.contains(&format!("opens the files in {FOLDER} as they are now")));
    assert!(!shown.contains("{folder}"));
}
