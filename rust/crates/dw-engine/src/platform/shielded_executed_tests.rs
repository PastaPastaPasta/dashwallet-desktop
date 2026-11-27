//! M2: a spend the library reports executed but whose notes it could
//! not mark spent. Each spend's success arm (operations.rs) hands the notes
//! over, confirms the history row and then calls `finalize_pending`, whose
//! failure only warns. The spend's record and its reservations stay, and
//! upstream restores the reservations from the record at every open. So
//! across a restart, a Close or a quit, the notes are neither selectable
//! nor counted spendable, and the next scan that finds their nullifiers
//! marks them spent and drops the record. The balance the desktop shows is
//! the spendable one. The disposition tests cover the same hold with a
//! failed record delete and a scan the test writes itself
//! (`shielded_disposition_tests.rs`, M2-a); these run the library's own
//! scan, answered by a mock SDK.

use dash_sdk::query_types::{
    NoParamQuery, ShieldedAnchors, ShieldedEncryptedNote, ShieldedEncryptedNotes,
    ShieldedEncryptedNotesQuery, ShieldedNotesCount,
};
use platform_wallet::wallet::shielded::ShieldedNote;

use super::*;

/// Where the quitting child process keeps its data root.
const CHILD_ROOT: &str = "DW_M2_QUIT_ROOT";
/// How the child's launch ends ([`launch_with_an_executed_spend_child`]).
const CHILD_MODE: &str = "DW_M2_CHILD_MODE";

const VALUE: u64 = 300_000_000;
/// The notes the spend consumed, and one it left.
const SPENT: [u64; 2] = [1, 2];
const LEFT: u64 = 3;
const ANCHOR: [u8; 32] = [0xa7; 32];
/// The height Platform's answers to a pass are proved at: past the spend's
/// anchor height, with its anchor still recorded, so no prune releases it.
const SET_HEIGHT: u64 = 2 * ANCHOR_HEIGHT;

fn held(position: u64) -> ShieldedNote {
    ShieldedNote {
        position,
        cmx: [position as u8; 32],
        nullifier: [position as u8 + 100; 32],
        block_height: 1,
        is_spent: false,
        value: VALUE,
        note_data: Vec::new(),
    }
}

fn nf(position: u64) -> [u8; 32] {
    held(position).nullifier
}

/// The executed spend of [`SPENT`]: a transfer built on [`ANCHOR`].
fn spend() -> PendingRedrive {
    let nullifiers: Vec<[u8; 32]> = SPENT.iter().map(|p| nf(*p)).collect();
    PendingRedrive {
        activity_id: [0xe1; 32],
        st_bytes: transition(1, ANCHOR, &nullifiers),
        anchor: ANCHOR,
        nullifiers,
        attempts: 0,
        identity_nonce_finalized: false,
        identity_user_abandoned: false,
        anchor_height: Some(ANCHOR_HEIGHT),
    }
}

/// Gives `wallet`'s account the notes [`SPENT`] and [`LEFT`], as a scan
/// does. The store keeps notes for the process only (the host's store
/// holds the viewing key), so a relaunch saves them again, as its scan
/// would rediscover them.
fn save_notes(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        for p in SPENT.iter().chain([&LEFT]) {
            store.save_note(subwallet(wallet), &held(*p)).unwrap();
        }
    });
}

/// The spend executed and its mark failed: the library's steps up to
/// `finalize_pending` (reserved, its record saved and its artifact in the
/// journal before the broadcast, its history row confirmed), with no
/// `mark_spent`.
fn executed_unmarked(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let r = spend();
        {
            let mut store = coordinator.store().write().await;
            reserve_and_record(&mut *store, subwallet(wallet), r.clone());
        }
        s.leases
            .record_shielded_artifact(
                wallet,
                NewShieldedArtifact {
                    activity: r.activity_id,
                    origin: ArtifactOrigin::Dispatched,
                    st_hash: Some(Sha256::digest(&r.st_bytes).into()),
                    kind: Some(ArtifactKind::Transfer),
                    anchor: Some(r.anchor),
                    identity_id: None,
                    selected: r.nullifiers.clone(),
                    published: r.nullifiers.clone(),
                },
            )
            .await
            .unwrap();
        let mut store = coordinator.store().write().await;
        history_row(
            &mut *store,
            subwallet(wallet),
            &r,
            ShieldedActivityStatus::Confirmed,
        );
    });
}

/// What the store holds of the spend: the nullifiers a spend may select,
/// the spendable balance, the nullifiers marked spent and whether its
/// record is kept.
#[derive(Debug, PartialEq)]
struct Held {
    selectable: Vec<[u8; 32]>,
    spendable: u64,
    spent: Vec<[u8; 32]>,
    recorded: bool,
}

fn held_state(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Held {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        let id = subwallet(wallet);
        let mut selectable: Vec<_> = (store.get_unspent_notes(id).unwrap().into_iter())
            .map(|n| n.nullifier)
            .collect();
        selectable.sort();
        let mut spent: Vec<_> = (store.get_all_notes(id).unwrap().into_iter())
            .filter(|n| n.is_spent)
            .map(|n| n.nullifier)
            .collect();
        spent.sort();
        Held {
            selectable,
            spendable: store.spendable_balance(id).unwrap(),
            spent,
            recorded: (store.pending_redrives(id).unwrap().iter())
                .any(|r| r.activity_id == spend().activity_id),
        }
    })
}

/// Rule 1: the executed spend's notes are neither selectable nor counted.
fn unmarked() -> Held {
    Held {
        selectable: vec![nf(LEFT)],
        spendable: VALUE,
        spent: Vec::new(),
        recorded: true,
    }
}

/// The scan found the spend: its notes spent, its record gone.
fn marked() -> Held {
    Held {
        selectable: vec![nf(LEFT)],
        spendable: VALUE,
        spent: SPENT.iter().map(|p| nf(*p)).collect(),
        recorded: false,
    }
}

/// A pass's Platform answers: the notes of the pool's first chunk, whose
/// actions spent `spent` (garbage ciphertexts no key opens), and the
/// recorded anchor set holding [`ANCHOR`], each proved at [`SET_HEIGHT`].
fn scan_sdk(spent: &[[u8; 32]]) -> dash_sdk::Sdk {
    let notes: Vec<ShieldedEncryptedNote> = (spent.iter().enumerate())
        .map(|(i, nullifier)| ShieldedEncryptedNote {
            cmx: vec![0x30 + i as u8; 32],
            nullifier: nullifier.to_vec(),
            cv_net: vec![0; 32],
            encrypted_note: vec![0; 32 + 104 + 80],
        })
        .collect();
    let total = notes.len() as u64;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let mut sdk = dash_sdk::Sdk::new_mock();
        let count = 2048
            * u32::from(
                sdk.version()
                    .drive_abci
                    .query
                    .shielded_queries
                    .max_query_chunks,
            );
        let mut first = Some(notes);
        for chunk in 0..16 {
            sdk.mock()
                .expect_fetch(
                    ShieldedEncryptedNotesQuery {
                        start_index: u64::from(count) * chunk,
                        count,
                    },
                    Some(ShieldedEncryptedNotes {
                        notes: first.take().unwrap_or_default(),
                        total_count: total,
                    }),
                )
                .await
                .unwrap();
        }
        sdk.mock()
            .expect_fetch(NoParamQuery {}, Some(ShieldedAnchors(vec![ANCHOR])))
            .await
            .unwrap();
        sdk.mock()
            .expect_fetch(NoParamQuery {}, Some(ShieldedNotesCount(total)))
            .await
            .unwrap();
        sdk.mock()
            .set_response_height_of::<ShieldedAnchors>(SET_HEIGHT)
            .set_response_height_of::<ShieldedNotesCount>(SET_HEIGHT)
            .set_response_height_of::<ShieldedEncryptedNotes>(SET_HEIGHT);
        sdk
    })
}

/// The relaunch: the session answered by `sdk`, the vault unlocked, the
/// wallet bound and its notes found again.
fn relaunch(engine: &Engine, root: &Path, sdk: dash_sdk::Sdk) -> (Arc<NetworkSession>, WalletId) {
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                sdk: Some(sdk),
                ..options()
            },
        ))
        .unwrap();
    let wallet: WalletId = (std::fs::read_to_string(root.join("wallet")).unwrap())
        .parse()
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert!(bind(engine, &session, wallet));
    save_notes(engine, &session, wallet);
    (session, wallet)
}

fn forced_pass(engine: &Engine, session: &Arc<NetworkSession>) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    assert!(
        engine.block_on(async move { pool::guarded_pass(&s, &m, true).await }),
        "the pass ran"
    );
}

fn refused<T: std::fmt::Debug>(r: Result<T, EngineError>) {
    assert!(
        matches!(r, Err(EngineError::ShieldedSpendUnresolved)),
        "{r:?}"
    );
}

/// After the relaunch: the notes are still held, and the first pass whose
/// scan finds their nullifiers marks them spent, which drops the record,
/// and the wallet closes. The pool's set still records the spend's anchor,
/// so nothing but the scan resolves it.
fn relaunch_then_scan(root: &Path) {
    let engine = engine_at(root);
    let (session, wallet) = relaunch(&engine, root, scan_sdk(&spend().nullifiers));
    assert_eq!(
        held_state(&engine, &session, wallet),
        unmarked(),
        "restored"
    );
    refused(engine.block_on(session.unload_wallet(wallet)));

    forced_pass(&engine, &session);
    assert_eq!(held_state(&engine, &session, wallet), marked(), "scanned");
    assert!(
        stored_rows(&session, wallet).is_empty(),
        "the record is gone from disk"
    );
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// The first launch, run in a child process (a launch's stores stay open
/// until its process ends): right after the executed spend whose mark
/// failed, its notes are held in this process. Then, by [`CHILD_MODE`]:
/// `exit` ends the process with no quit (a restart); `quit` quits (the
/// app's quit, `Engine::shutdown`); `close` first tries Close Wallet and the
/// network close, both refused, then quits. The process exits at once.
#[test]
#[ignore = "the child process of the executed-spend relaunch tests"]
fn launch_with_an_executed_spend_child() {
    let (Some(root), Some(mode)) = (std::env::var_os(CHILD_ROOT), std::env::var(CHILD_MODE).ok())
    else {
        return;
    };
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    save_notes(&engine, &session, wallet);
    executed_unmarked(&engine, &session, wallet);
    if mode == "finalized" {
        let s = Arc::clone(&session);
        engine.block_on(async move {
            let m = s.manager().unwrap();
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            {
                let mut store = coordinator.store().write().await;
                for nullifier in spend().nullifiers {
                    assert!(store.mark_spent(subwallet(wallet), &nullifier).unwrap());
                }
            }
            assert!(stored_rows(&s, wallet).is_empty());
        });
    } else {
        assert_eq!(held_state(&engine, &session, wallet), unmarked());
    }
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    match mode.as_str() {
        "exit" | "finalized" => {}
        "quit" => engine.block_on(engine.shutdown()).unwrap(),
        "close" => {
            refused(engine.block_on(session.unload_wallet(wallet)));
            refused(engine.block_on(engine.close_network(DashNetwork::Regtest)));
            assert_eq!(held_state(&engine, &session, wallet), unmarked());
            engine.block_on(engine.shutdown()).unwrap();
        }
        other => panic!("unknown mode {other}"),
    }
    std::process::exit(0);
}

/// Runs [`launch_with_an_executed_spend_child`] in `mode` over a new data
/// root, then relaunches over it ([`relaunch_then_scan`]).
fn launch_then_relaunch(mode: &str) {
    let dir = dw_testutil::private_tempdir();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::executed_tests::launch_with_an_executed_spend_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, dir.path())
        .env(CHILD_MODE, mode)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "the first launch failed ({mode}): {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(dir.path().join("wallet").exists(), "the first launch ran");
    relaunch_then_scan(dir.path());
}

/// M2-a (i): an executed spend whose mark failed holds its notes in
/// its process and across a restart (rule 1), until the next scan marks
/// them spent.
#[test]
fn an_executed_spend_whose_mark_failed_holds_its_notes_until_a_scan_marks_them() {
    launch_then_relaunch("exit");
}

/// M2-a (ii): Close Wallet (refused) or the quit right after an
/// executed spend whose mark failed, then a relaunch: the notes stay held
/// until the next scan marks them spent.
#[test]
fn a_close_or_quit_after_an_executed_spend_keeps_its_notes_held() {
    launch_then_relaunch("close");
    launch_then_relaunch("quit");
}

/// A completed scan that does not find the spend's nullifiers frees
/// nothing, and its set, still recording the spend's anchor, proves no
/// prune: the notes stay held and unmarked.
#[test]
fn a_scan_without_the_spends_nullifiers_keeps_its_notes_held() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = unbound_launch_with(
        &engine,
        crate::SessionOptions {
            sdk: Some(scan_sdk(&[[0x55; 32]])),
            ..options()
        },
    );
    assert!(bind(&engine, &session, wallet));
    save_notes(&engine, &session, wallet);
    executed_unmarked(&engine, &session, wallet);
    forced_pass(&engine, &session);
    let status = engine
        .block_on(session.dashpay(wallet).shielded_status())
        .unwrap();
    assert_eq!(
        status.synced_index,
        Some(1),
        "the scan completed: {status:?}"
    );
    assert_eq!(held_state(&engine, &session, wallet), unmarked());
    refused(engine.block_on(session.unload_wallet(wallet)));
}

/// M2-b: the shielded balance the desktop shows (`ShieldedStatus`
/// `balance`, the Home card's) counts a reserved note as not spendable.
#[test]
fn the_shown_balance_leaves_out_reserved_notes() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    save_notes(&engine, &session, wallet);
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        (coordinator.store().write().await)
            .set_last_synced_note_index(subwallet(wallet), LEFT + 1)
            .unwrap();
    });
    let shown = || {
        let status = engine
            .block_on(session.dashpay(wallet).shielded_status())
            .unwrap();
        assert!(status.ready, "{status:?}");
        status.balance
    };
    assert_eq!(shown(), 3 * VALUE, "nothing reserved");
    executed_unmarked(&engine, &session, wallet);
    assert_eq!(shown(), VALUE, "the spend's notes are reserved");
}

#[test]
fn a_finalized_spend_stays_unselectable_after_a_behind_rescan() {
    let dir = dw_testutil::private_tempdir();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::executed_tests::launch_with_an_executed_spend_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, dir.path())
        .env(CHILD_MODE, "finalized")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    let engine = engine_at(dir.path());
    let (session, wallet) = relaunch(&engine, dir.path(), scan_sdk(&[[0x55; 32]]));
    assert!(stored_rows(&session, wallet).is_empty());
    forced_pass(&engine, &session);

    let dp = session.dashpay(wallet);
    let balance = engine.block_on(dp.shielded_status()).unwrap().balance;
    assert_eq!(balance, VALUE);
    let to = engine
        .block_on(dp.shielded_receive_address())
        .unwrap()
        .address;
    assert_eq!(
        engine
            .block_on(dp.shielded_send_quote(to, 2 * VALUE))
            .unwrap_err()
            .code(),
        "shielded.insufficient_funds"
    );
    assert_eq!(held_state(&engine, &session, wallet).selectable, [nf(LEFT)]);

    let path = session
        .data_dir()
        .join(dw_appdb::dispatch::DISPATCH_DB_FILE);
    let journal = rusqlite::Connection::open(path).unwrap();
    journal.busy_timeout(Duration::from_secs(1)).unwrap();
    journal
        .execute_batch("PRAGMA ignore_check_constraints = ON")
        .unwrap();
    let activity = spend().activity_id;
    let invalid = [9u8; 31];
    journal
        .execute(
            "INSERT INTO shielded_artifact_note (wallet, activity, nullifier) VALUES (?1, ?2, ?3)",
            rusqlite::params![&wallet.0[..], &activity[..], &invalid[..]],
        )
        .unwrap();
    assert!(engine.block_on(dp.shielded_status()).is_err());
    assert_eq!(held_state(&engine, &session, wallet).selectable, [nf(LEFT)]);
    journal
        .execute(
            "DELETE FROM shielded_artifact_note WHERE length(nullifier) <> 32",
            [],
        )
        .unwrap();

    let s = Arc::clone(&session);
    engine.block_on(async move {
        let m = s.manager().unwrap();
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        for nullifier in spend().nullifiers {
            assert!(store.mark_spent(subwallet(wallet), &nullifier).unwrap());
        }
    });
    assert_eq!(held_state(&engine, &session, wallet), marked());
}
