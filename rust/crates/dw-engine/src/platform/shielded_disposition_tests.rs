//! The engine's reading of the spend outcomes the library reports, into
//! the dispatch journal by transition hash (`shielded_pool::settle_outcomes`),
//! at each pass's end and at the shielded store's open. Upstream keeps no
//! disposition of its own: a spend's outcome is its history row (confirmed
//! once the scan finds its notes spent, failed once it is refused before
//! delivery or its anchor is proven pruned), and an unknown one keeps its
//! record and reservations. The spends are [`spends`], stored under the hook
//! with their artifacts ([`store_spends`]).
//!
//! The fork's tests of its dispositions are kept below, not compiled
//! ([`fork`]). Its executed-spend holds are replaced by upstream's durable
//! reservation: a spend's record (`shielded_pending_spends`) holds its
//! notes until the scan marks them spent, across a restart too (M2-a,
//! [`an_executed_spend_whose_mark_spent_failed_stays_reserved_across_a_restart`]).
//! The same hold with no mark at all, released by the library's own scan
//! over a mock SDK, is in `shielded_executed_tests.rs`.

use platform_wallet::wallet::shielded::ShieldedNote;

use super::*;

/// One shielded pass (offline: its scan fails, and its end still runs).
fn pass(engine: &Engine, session: &Arc<NetworkSession>) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    assert!(
        engine.block_on(async move { pool::guarded_pass(&s, &m, true).await }),
        "the pass ran"
    );
}

/// What the journal holds of spend `n`: its status, the disposition strings
/// and whether it was acknowledged.
type Settlement = (ArtifactStatus, Option<(String, Option<String>)>, bool);

fn settlement(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    n: u8,
) -> Settlement {
    let artifact = engine
        .block_on(session.leases.shielded_artifact(wallet, [n; 32]))
        .unwrap()
        .unwrap();
    (artifact.status, artifact.disposition, artifact.acked)
}

fn refused<T: std::fmt::Debug>(r: Result<T, EngineError>) {
    assert!(
        matches!(r, Err(EngineError::ShieldedSpendUnresolved)),
        "{r:?}"
    );
}

/// The library's reports settle the journal: spend 1's row confirmed by the
/// scan (which drops its record and reservations) reads `executed`, spend
/// 2's row failed by a refusal before delivery (its record retired, its
/// notes freed) reads `not_executed`, neither acknowledged. Then nothing
/// holds the wallet.
#[test]
fn reported_outcomes_settle_the_journal() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let [executed, failed]: [PendingRedrive; 2] = spends().try_into().unwrap();
        for n in &executed.nullifiers {
            store.clear_pending(subwallet(wallet), n).unwrap();
        }
        store
            .clear_redrive(subwallet(wallet), &executed.activity_id)
            .unwrap();
        history_row(
            &mut *store,
            subwallet(wallet),
            &executed,
            ShieldedActivityStatus::Confirmed,
        );
        refuse_undelivered(&mut *store, subwallet(wallet), &failed);
    });
    assert_eq!(
        settlement(&engine, &session, wallet, 1).0,
        ArtifactStatus::Registered
    );

    pass(&engine, &session);
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (
            ArtifactStatus::Executed,
            Some(("executed".into(), None)),
            false
        )
    );
    assert_eq!(
        settlement(&engine, &session, wallet, 2),
        (ArtifactStatus::NotExecuted, None, false)
    );
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// An outcome the library reports and the journal has not read holds the
/// wallet: the library keeps it in memory only, so a removal would lose it.
/// With spend 1's row confirmed and spend 2's failed (record and
/// reservations gone for both) Close Wallet, the network close and the
/// restore's rollback are refused until a pass reads both; then the close
/// proceeds.
#[test]
fn an_unread_outcome_holds_the_close_until_a_pass_reads_it() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let [executed, failed]: [PendingRedrive; 2] = spends().try_into().unwrap();
        for n in &executed.nullifiers {
            store.clear_pending(subwallet(wallet), n).unwrap();
        }
        store
            .clear_redrive(subwallet(wallet), &executed.activity_id)
            .unwrap();
        history_row(
            &mut *store,
            subwallet(wallet),
            &executed,
            ShieldedActivityStatus::Confirmed,
        );
        refuse_undelivered(&mut *store, subwallet(wallet), &failed);
        assert!(
            store
                .pending_redrives(subwallet(wallet))
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .stale_pending_spends(subwallet(wallet))
                .unwrap()
                .is_empty()
        );
    });

    refused(engine.block_on(session.unload_wallet(wallet)));
    refused(engine.block_on(engine.close_network(DashNetwork::Regtest)));
    assert!(session.live().is_ok(), "the session stays open");
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    refused(
        engine
            .block_on(async move { (s.platform.remove_from_manager(&s.leases, &m, wallet)).await }),
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");
    for n in [1, 2] {
        assert_eq!(
            settlement(&engine, &session, wallet, n).0,
            ArtifactStatus::Registered
        );
    }

    pass(&engine, &session);
    assert_eq!(
        settlement(&engine, &session, wallet, 1).0,
        ArtifactStatus::Executed
    );
    assert_eq!(
        settlement(&engine, &session, wallet, 2).0,
        ArtifactStatus::NotExecuted
    );
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// A failed first send under the hook leaves the spend's record and its
/// reservations (upstream's `ShieldedSpendUnconfirmed`): it may still
/// execute. Its artifact stays open (registered: upstream reports no
/// reason to settle it `unresolved` with), and even with its notes seen
/// spent by the host they stay held and Close Wallet stays refused.
#[test]
fn cancelled_first_send_holds_notes_and_reads_checking() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let first = spends().remove(0);
    spent_seen(&session, wallet, &first.nullifiers);

    pass(&engine, &session);
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (ArtifactStatus::Registered, None, false)
    );
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    let (stored, reserved) = engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        (
            store.pending_redrives(subwallet(wallet)).unwrap(),
            store.stale_pending_spends(subwallet(wallet)).unwrap(),
        )
    });
    assert!(
        stored.iter().any(|r| r.activity_id == first.activity_id),
        "{stored:?}"
    );
    for n in &first.nullifiers {
        assert!(reserved.iter().any(|(r, ..)| r == n), "{n:?} held");
    }
    refused(engine.block_on(session.unload_wallet(wallet)));
}

/// L4: a failed history row settles `not_executed` only once the library
/// holds neither a record nor a reserved note of the spend: with both, then
/// with either alone (spend 2's record retired first; spend 1's
/// reservations released, which drops its record, and its record saved
/// again alone), it stays open; with neither, it settles.
#[test]
fn a_failed_row_settles_only_once_nothing_of_the_spend_is_held() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let [first, second]: [PendingRedrive; 2] = spends().try_into().unwrap();
    // `(spend, retire first)`; each step's effect, then what the library
    // still holds of the spend: `(record, reservations)`.
    for (spend, retire_first) in [(second, true), (first, false)] {
        let n = spend.activity_id[0];
        let step = |what: &'static str| {
            let (s, m, spend) = (
                Arc::clone(&session),
                session.manager().unwrap(),
                spend.clone(),
            );
            engine.block_on(async move {
                let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
                let mut store = coordinator.store().write().await;
                let id = subwallet(wallet);
                match what {
                    "fail" => history_row(&mut *store, id, &spend, ShieldedActivityStatus::Failed),
                    "retire" => assert!(
                        store
                            .retire_undelivered_spend(id, &spend.activity_id, &[])
                            .unwrap()
                    ),
                    "release" => {
                        for nullifier in &spend.nullifiers {
                            store.clear_pending(id, nullifier).unwrap();
                        }
                    }
                    "record alone" => {
                        for nullifier in &spend.nullifiers {
                            store.clear_pending(id, nullifier).unwrap();
                        }
                        store.arm_redrive(id, spend.clone()).unwrap();
                    }
                    other => unreachable!("{other}"),
                }
                let stored = (store.pending_redrives(id).unwrap().iter())
                    .any(|r| r.activity_id == spend.activity_id);
                let held = (store.stale_pending_spends(id).unwrap().iter())
                    .any(|(_, _, a, _)| *a == Some(spend.activity_id));
                (stored, held)
            })
        };
        let (one, two) = if retire_first {
            (("retire", (false, true)), ("release", (false, false)))
        } else {
            (("record alone", (true, false)), ("retire", (false, false)))
        };
        assert_eq!(step("fail"), (true, true), "spend {n}");
        pass(&engine, &session);
        assert_eq!(
            settlement(&engine, &session, wallet, n).0,
            ArtifactStatus::Registered,
            "spend {n}: record and reservations held"
        );
        assert_eq!(step(one.0), one.1, "spend {n}");
        pass(&engine, &session);
        assert_eq!(
            settlement(&engine, &session, wallet, n).0,
            ArtifactStatus::Registered,
            "spend {n}: {} done, the rest held",
            one.0
        );
        assert_eq!(step(two.0), two.1, "spend {n}");
        pass(&engine, &session);
        assert_eq!(
            settlement(&engine, &session, wallet, n),
            (ArtifactStatus::NotExecuted, None, false),
            "spend {n}"
        );
    }
}

/// L1: while the dispatch journal is not read, an outcome the library may
/// report cannot be compared with it, so Close Wallet and the rollback are
/// refused; read again, they proceed.
#[test]
fn an_unread_journal_holds_the_close() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let journal = session.leases.journal.get();
    session.leases.load_journal(None, Vec::new(), Vec::new());
    assert!(!session.leases.journal_ready());
    refused(engine.block_on(session.unload_wallet(wallet)));
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    refused(
        engine
            .block_on(async move { (s.platform.remove_from_manager(&s.leases, &m, wallet)).await }),
    );
    assert!(session.require_wallet(&wallet).is_ok(), "still loaded");

    session.leases.load_journal(journal, Vec::new(), Vec::new());
    assert!(session.leases.journal_ready());
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// A row of the fork's `shielded_spend_dispositions`: transition `n`
/// (activity `[n; 32]`), its disposition and reason, the notes it holds
/// and `released_by_scan`.
pub(crate) type ForkRow = (u8, &'static str, Option<&'static str>, Vec<[u8; 32]>, i64);

/// Writes `rows` of `wallet` into the fork's table in the shielded store,
/// created with the fork's schema.
pub(crate) fn fork_dispositions(path: &Path, wallet: WalletId, rows: &[ForkRow]) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS shielded_spend_dispositions (
            wallet_id     BLOB    NOT NULL
                CHECK (typeof(wallet_id) = 'blob' AND length(wallet_id) = 32),
            account_index INTEGER NOT NULL
                CHECK (typeof(account_index) = 'integer'
                       AND account_index BETWEEN 0 AND 4294967295),
            st_hash       BLOB    NOT NULL
                CHECK (typeof(st_hash) = 'blob' AND length(st_hash) = 32),
            activity_id   BLOB    NOT NULL
                CHECK (typeof(activity_id) = 'blob' AND length(activity_id) = 32),
            disposition   TEXT    NOT NULL
                CHECK (disposition IN ('executed', 'not_executed', 'unresolved')),
            reason        TEXT
                CHECK ((disposition = 'executed') = (reason IS NULL)),
            at            INTEGER NOT NULL,
            nullifiers    BLOB    NOT NULL DEFAULT x''
                CHECK (typeof(nullifiers) = 'blob' AND length(nullifiers) % 32 = 0
                       AND (disposition IN ('executed', 'unresolved')
                            OR length(nullifiers) = 0)),
            released_by_scan INTEGER NOT NULL DEFAULT 0
                CHECK (released_by_scan IN (0, 1)),
            PRIMARY KEY (wallet_id, account_index, st_hash)
        )",
    )
    .unwrap();
    for (n, disposition, reason, nullifiers, released) in rows {
        db.execute(
            "INSERT INTO shielded_spend_dispositions (wallet_id, account_index, st_hash, \
             activity_id, disposition, reason, at, nullifiers, released_by_scan) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
            rusqlite::params![
                &wallet.0[..],
                dw_vault::SHIELDED_ACCOUNT,
                &[*n; 32][..],
                &[*n; 32][..],
                disposition,
                reason,
                nullifiers.concat(),
                released,
            ],
        )
        .unwrap();
    }
}

/// Every row of the fork's table, as stored.
pub(crate) fn fork_rows(path: &Path) -> Vec<(Vec<u8>, Vec<u8>, i64)> {
    let db = rusqlite::Connection::open(path).unwrap();
    let mut rows = db
        .prepare(
            "SELECT st_hash, nullifiers, released_by_scan FROM shielded_spend_dispositions \
             ORDER BY st_hash",
        )
        .unwrap();
    rows.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Closes the network, runs `between` on its shielded store's path, and
/// opens it again, unlocked.
fn reopen_with(
    engine: &Engine,
    session: Arc<NetworkSession>,
    between: impl FnOnce(&Path),
) -> Arc<NetworkSession> {
    let path = session.data_dir().join(pool::SHIELDED_DB_FILE);
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    between(&path);
    let session = open(engine);
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    session
}

/// M1: the fork held the notes of an unacknowledged `executed` or
/// `unresolved` disposition until its acknowledgement, which upstream
/// neither reads nor offers. After the upgrade each such row is a marked
/// spend (DEC-1057): Close Wallet, the network close and the rollback are
/// refused until a scan finds all its notes spent. The fork's table is left
/// as it was, and a cleared marker is not marked again at the next open.
#[test]
fn a_fork_held_spend_holds_the_close_after_the_upgrade() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let (unresolved, executed) = (vec![[61; 32], [62; 32]], vec![[63; 32]]);
    let rows: Vec<ForkRow> = vec![
        (1, "unresolved", Some("cancelled"), unresolved.clone(), 0),
        (2, "executed", None, executed.clone(), 0),
    ];
    let mut before = Vec::new();
    let session = reopen_with(&engine, session, |path| {
        fork_dispositions(path, wallet, &rows);
        before = fork_rows(path);
    });
    assert_eq!(markers(&engine, &session, wallet), [[1; 32], [2; 32]]);
    refused(engine.block_on(session.unload_wallet(wallet)));
    refused(engine.block_on(engine.close_network(DashNetwork::Regtest)));
    assert!(session.live().is_ok(), "the session stays open");
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    refused(
        engine
            .block_on(async move { (s.platform.remove_from_manager(&s.leases, &m, wallet)).await }),
    );

    spent_seen(&session, wallet, &unresolved);
    assert_eq!(markers(&engine, &session, wallet), [[2; 32]]);
    refused(engine.block_on(session.unload_wallet(wallet)));
    spent_seen(&session, wallet, &executed);
    assert!(markers(&engine, &session, wallet).is_empty());
    engine.block_on(session.unload_wallet(wallet)).unwrap();

    let path = session.data_dir().join(pool::SHIELDED_DB_FILE);
    let session = reopen_with(&engine, session, |_| {});
    assert_eq!(fork_rows(&path), before, "the fork's rows as they were");
    engine.block_on(session.load_wallet(wallet)).unwrap();
    assert!(markers(&engine, &session, wallet).is_empty(), "not again");
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// M1: a fork row that holds no note (acknowledged, so its nullifiers were
/// cleared; `not_executed`; or left to a scan by a purge) marks nothing,
/// nor does a store without the fork's table: the close proceeds.
#[test]
fn fork_rows_holding_no_note_and_a_store_without_them_leave_the_close() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let session = reopen_with(&engine, session, |path| {
        assert!(path.exists(), "the bind opened the store");
        let db = rusqlite::Connection::open(path).unwrap();
        let tables: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'shielded_spend_dispositions'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "upstream's store has no such table");
    });
    assert!(markers(&engine, &session, wallet).is_empty());
    engine.block_on(session.unload_wallet(wallet)).unwrap();

    let rows: Vec<ForkRow> = vec![
        (1, "executed", None, Vec::new(), 0),
        (2, "unresolved", Some("cancelled"), Vec::new(), 0),
        (3, "not_executed", Some("refused"), Vec::new(), 0),
        (4, "unresolved", Some("cancelled"), vec![[64; 32]], 1),
        (5, "executed", None, vec![[65; 32]], 1),
    ];
    let session = reopen_with(&engine, session, |path| {
        fork_dispositions(path, wallet, &rows)
    });
    engine.block_on(session.load_wallet(wallet)).unwrap();
    assert!(markers(&engine, &session, wallet).is_empty());
    engine.block_on(session.unload_wallet(wallet)).unwrap();
    drop(session);
    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
}

/// The fork's table as a build that wrote it without its checks would
/// leave it, with one row of `wallet_id`, `st_hash` and `nullifiers`.
fn unchecked_fork_row(path: &Path, wallet_id: &[u8], st_hash: &[u8], nullifiers: &[u8]) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE shielded_spend_dispositions (wallet_id, account_index, st_hash, \
         activity_id, disposition, reason, at, nullifiers, released_by_scan)",
    )
    .unwrap();
    db.execute(
        "INSERT INTO shielded_spend_dispositions VALUES (?1, 0, ?2, ?3, 'unresolved', \
         'cancelled', 1, ?4, 0)",
        rusqlite::params![wallet_id, st_hash, &[1u8; 32][..], nullifiers],
    )
    .unwrap();
}

/// M1, fail closed: a fork row that does not read holds its wallet for the
/// session (a held-note list that is no list of nullifiers, a malformed
/// transition), and every wallet when its wallet does not read either;
/// nothing is marked. Another wallet is held only in the latter case.
#[test]
fn an_unreadable_fork_row_holds_its_wallet_or_every_wallet() {
    let other = WalletId([3; 32]);
    type Row = fn(WalletId) -> (Vec<u8>, Vec<u8>, Vec<u8>);
    let cases: [(Row, bool); 3] = [
        (|w| (w.0.to_vec(), vec![1; 32], vec![61; 33]), false),
        (|w| (w.0.to_vec(), vec![1; 31], vec![61; 32]), false),
        (|w| (w.0[..31].to_vec(), vec![1; 32], vec![61; 32]), true),
    ];
    for (row, every) in cases {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        let (wallet_id, st_hash, nullifiers) = row(wallet);
        let session = reopen_with(&engine, session, |path| {
            unchecked_fork_row(path, &wallet_id, &st_hash, &nullifiers)
        });
        assert!(markers(&engine, &session, wallet).is_empty());
        assert!(engine.block_on(session.leases.marks_unrecorded(wallet)));
        assert_eq!(
            engine.block_on(session.leases.marks_unrecorded(other)),
            every
        );
        refused(engine.block_on(session.unload_wallet(wallet)));
        refused(engine.block_on(engine.close_network(DashNetwork::Regtest)));
    }
}

/// M1, fail closed: with the dispatch journal unreadable at the open (here
/// of a newer schema) no marker can be written, so the wallet of a fork row
/// that holds notes is held for the session; another wallet is not.
#[test]
fn a_fork_hold_with_no_journal_holds_its_wallet_for_the_session() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let journal = session
        .data_dir()
        .join(dw_appdb::dispatch::DISPATCH_DB_FILE);
    let rows: Vec<ForkRow> = vec![(1, "unresolved", Some("cancelled"), vec![[61; 32]], 0)];
    let session = reopen_with(&engine, session, |path| {
        fork_dispositions(path, wallet, &rows);
        let db = rusqlite::Connection::open(&journal).unwrap();
        db.execute("UPDATE meta SET v = 99 WHERE k = 'schema'", [])
            .unwrap();
    });
    assert!(!session.leases.journal_ready());
    assert!(engine.block_on(session.leases.marks_unrecorded(wallet)));
    assert!(!engine.block_on(session.leases.marks_unrecorded(WalletId([3; 32]))));
    refused(engine.block_on(session.unload_wallet(wallet)));
}

/// Saves a note of `wallet`'s account with `nullifier` in the library's
/// store, read spent when `spent`, with no changeset: the persister's tap
/// does not see it, as with a note restored spent or one the store read
/// spent before the marker was written.
fn store_note(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    nullifier: [u8; 32],
    spent: bool,
) {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let note = platform_wallet::wallet::shielded::ShieldedNote {
            position: u64::from(nullifier[0]),
            cmx: [nullifier[0].wrapping_add(1); 32],
            nullifier,
            block_height: 1,
            is_spent: false,
            value: 1_000,
            note_data: vec![0; 115],
        };
        store.save_note(subwallet(wallet), &note).unwrap();
        if spent {
            assert!(store.mark_spent(subwallet(wallet), &nullifier).unwrap());
        }
    });
}

/// What the hook answers the library's pass asking to re-send spend `n`.
fn admit(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    n: u8,
) -> Result<(), String> {
    use platform_wallet::wallet::shielded::ShieldedDispatch;
    let st_hash: [u8; 32] = Sha256::digest(&spends()[usize::from(n) - 1].st_bytes).into();
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let hook = pool::EngineDispatch::new(&s, &coordinator);
        (hook
            .admit(wallet.0, dw_vault::SHIELDED_ACCOUNT, [n; 32], st_hash)
            .await)
            .map_err(|refusal| refusal.reason)
    })
}

/// The status each of `wallet`'s history rows shows.
fn shown(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
) -> Vec<(String, crate::platform::shielded::ShieldedActivityStatus)> {
    engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            let network = pool::ShieldedLib::network(&lib);
            let rows = pool::ShieldedLib::activity(&lib, 0, 10).await?;
            Ok((rows.into_iter())
                .map(|e| pool::activity(e, network))
                .map(|a| (a.id, a.status))
                .collect())
        }))
        .unwrap()
}

/// Upstream reports no `Unattributable` outcome: a spend whose history row
/// it confirmed executed (its success arm, or the scan that finds its notes
/// spent) reads done, and nothing offers it again. Spend 1's row is
/// confirmed with its record and reservations gone (the scan's path), spend
/// 2's with both kept (a success arm whose mark-spent failed). Before a pass
/// reads them and after, the hook admits no re-send of either; the pass
/// settles both with the `executed` disposition; each history row shows
/// confirmed, never failed.
#[test]
fn a_confirmed_row_reads_done_and_offers_no_retry() {
    use crate::platform::shielded::ShieldedActivityStatus as Shown;
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    for n in [1, 2] {
        assert_eq!(
            admit(&engine, &session, wallet, n),
            Ok(()),
            "spend {n} held"
        );
    }
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let [scanned, kept]: [PendingRedrive; 2] = spends().try_into().unwrap();
        for n in &scanned.nullifiers {
            store.clear_pending(subwallet(wallet), n).unwrap();
        }
        store
            .clear_redrive(subwallet(wallet), &scanned.activity_id)
            .unwrap();
        for spend in [&scanned, &kept] {
            history_row(
                &mut *store,
                subwallet(wallet),
                spend,
                ShieldedActivityStatus::Confirmed,
            );
        }
        let held = store.pending_redrives(subwallet(wallet)).unwrap();
        assert!(held.iter().any(|r| r.activity_id == kept.activity_id));
    });
    let confirmed = |n: u8| (hex::encode([n; 32]), Shown::Confirmed);
    for n in [1, 2] {
        assert_eq!(
            admit(&engine, &session, wallet, n),
            Err("the spend's history row reads confirmed".into()),
            "spend {n}"
        );
        assert_eq!(
            settlement(&engine, &session, wallet, n).0,
            ArtifactStatus::Registered
        );
    }

    pass(&engine, &session);
    for n in [1, 2] {
        assert_eq!(
            settlement(&engine, &session, wallet, n),
            (
                ArtifactStatus::Executed,
                Some(("executed".into(), None)),
                false
            ),
            "spend {n}"
        );
        assert!(admit(&engine, &session, wallet, n).is_err(), "spend {n}");
    }
    let mut rows = shown(&engine, &session, wallet);
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(rows, [confirmed(1), confirmed(2)]);
}

// ---- M2-a: upstream's durable reservation across a restart ----

/// The credits of each note of [`scanned_notes`].
const NOTE_VALUE: u64 = 10_000_000_000;

/// The nullifier of the one note of [`scanned_notes`] no spend holds.
const FREE: [u8; 32] = [0x30; 32];

/// Where the child of the relaunch tests keeps its data root, and how it
/// ends ([`an_executed_spend_child`]).
const EXECUTED_ROOT: &str = "DW_M2A_EXECUTED_ROOT";
const EXECUTED_CUT: &str = "DW_M2A_EXECUTED_CUT";

/// The account's notes as a scan finds them: the two of each of [`spends`]
/// at positions 0 to 3, then [`FREE`]'s at 4, each [`NOTE_VALUE`].
fn scanned_notes() -> Vec<ShieldedNote> {
    (spends().into_iter())
        .flat_map(|r| r.nullifiers)
        .chain([FREE])
        .enumerate()
        .map(|(position, nullifier)| ShieldedNote {
            position: position as u64,
            cmx: [0x80 + position as u8; 32],
            nullifier,
            block_height: 1,
            is_spent: false,
            value: NOTE_VALUE,
            note_data: Vec::new(),
        })
        .collect()
}

/// A scan of `wallet`'s account as the library's pass commits one
/// (`sync_notes_across`, under one write of the store): it saves the notes
/// it decrypts, marks spent the account's notes whose nullifiers it saw,
/// then moves the watermark to `to`.
async fn scan(
    s: &Arc<NetworkSession>,
    wallet: WalletId,
    notes: &[ShieldedNote],
    spent: &[[u8; 32]],
    to: u64,
) {
    let m = s.manager().unwrap();
    let coordinator = pool::coordinator(s, &m, None).await.unwrap();
    let mut store = coordinator.store().write().await;
    for note in notes {
        store.save_note(subwallet(wallet), note).unwrap();
    }
    for nullifier in spent {
        store.mark_spent(subwallet(wallet), nullifier).unwrap();
    }
    store
        .set_last_synced_note_index(subwallet(wallet), to)
        .unwrap();
}

/// What upstream's success arm does with spend `r` once Platform proved it
/// executed (operations.rs, `Ok((proof, metadata))`): its notes handed over
/// to the outcome, its history row confirmed, then, with `finalize`,
/// `finalize_pending` → `mark_notes_spent`, the store's `mark_spent` of
/// each of its notes. Without `finalize`, the cut before it. The nullifiers
/// `mark_notes_spent` also hands the persister are not staged here
/// ([`spent_seen`] stages them where a test needs them).
async fn executed(s: &Arc<NetworkSession>, wallet: WalletId, r: &PendingRedrive, finalize: bool) {
    let m = s.manager().unwrap();
    let coordinator = pool::coordinator(s, &m, None).await.unwrap();
    let mut store = coordinator.store().write().await;
    history_row(
        &mut *store,
        subwallet(wallet),
        r,
        ShieldedActivityStatus::Confirmed,
    );
    if finalize {
        for nullifier in &r.nullifiers {
            store.mark_spent(subwallet(wallet), nullifier).unwrap();
        }
    }
}

/// Makes every delete of a spend record in the shielded store at `path`
/// fail (`on`), as a failed durable write in the store's `mark_spent` does
/// (it only warns, `file_store.rs`), or lets them succeed again.
fn fail_record_deletes(path: &Path, on: bool) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute_batch(if on {
        "CREATE TRIGGER m2a_fail_delete BEFORE DELETE ON shielded_pending_spends \
         BEGIN SELECT RAISE(FAIL, 'injected delete failure'); END;"
    } else {
        "DROP TRIGGER IF EXISTS m2a_fail_delete;"
    })
    .unwrap();
}

/// The positions of the notes a spend of `wallet` may select.
fn selectable(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Vec<u64> {
    let mut positions = engine
        .block_on(on_pool(session, wallet, |_, lib| async move {
            Ok((pool::ShieldedLib::unspent(&lib).await?.iter())
                .map(|n| n.position)
                .collect::<Vec<_>>())
        }))
        .unwrap();
    positions.sort_unstable();
    positions
}

/// The balance the desktop shows for `wallet` (`DashPay::shielded_status`),
/// which must be a reading.
fn shown_balance(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> u64 {
    let status = engine
        .block_on(session.dashpay(wallet).shielded_status())
        .unwrap();
    assert!(status.active && status.ready, "{status:?}");
    status.balance
}

/// The reservations of spend `r` in `wallet`'s store.
fn reserved_for(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    r: &PendingRedrive,
) -> Vec<[u8; 32]> {
    let (s, m, activity) = (
        Arc::clone(session),
        session.manager().unwrap(),
        r.activity_id,
    );
    let mut held: Vec<[u8; 32]> = engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        (store
            .stale_pending_spends(subwallet(wallet))
            .unwrap()
            .into_iter())
        .filter(|(_, _, a, _)| *a == Some(activity))
        .map(|(n, ..)| n)
        .collect()
    });
    held.sort_unstable();
    held
}

/// The first launch of the relaunch tests, in a child process: [`spends`]
/// stored with their artifacts, a scan that found their notes and
/// [`FREE`], then spend 1 executed. `crash`: its mark-spent ran with every
/// record delete failing, and the process exits at once, with no quit.
/// `quit`: cut before the mark-spent, then a quit (`Engine::shutdown`).
#[test]
#[ignore = "the child process of the relaunch tests after an executed spend"]
fn an_executed_spend_child() {
    let Some(root) = std::env::var_os(EXECUTED_ROOT) else {
        return;
    };
    let cut = std::env::var(EXECUTED_CUT).unwrap();
    let engine = engine_at(Path::new(&root));
    let (session, wallet) = first_launch(&engine);
    store_spends(&engine, &session, wallet);
    let s = Arc::clone(&session);
    engine.block_on(async move { scan(&s, wallet, &scanned_notes(), &[], 5).await });
    let first = spends().remove(0);
    let crash = cut == "crash";
    if crash {
        fail_record_deletes(&session.data_dir().join(pool::SHIELDED_DB_FILE), true);
    }
    let (s, r) = (Arc::clone(&session), first.clone());
    engine.block_on(async move { executed(&s, wallet, &r, crash).await });
    // Spend 1's notes are spent (crash) or held (quit) in this process,
    // and its record is on disk either way.
    assert_eq!(selectable(&engine, &session, wallet), [4]);
    let mut stored = stored_rows(&session, wallet);
    stored.sort_unstable();
    assert_eq!(stored, [[1; 32], [2; 32]]);
    let held = reserved_for(&engine, &session, wallet, &first);
    assert_eq!(held.is_empty(), crash, "{held:?}");
    std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
    if !crash {
        engine.block_on(engine.shutdown()).unwrap();
    }
    std::process::exit(0);
}

/// The relaunch after [`an_executed_spend_child`] ended by `cut`: spend 1's
/// record is back from disk with its reservations. Its notes, found again
/// by a scan that has not reached its actions, are neither selectable nor
/// counted in the balance the desktop shows, a send that needs them is
/// refused before its grant, and Close Wallet is refused. The scan that
/// reaches its actions marks them spent, and its record goes from disk
/// too. Its artifact never reads not executed.
fn relaunch_after_an_executed_spend(cut: &str) {
    let dir = dw_testutil::private_tempdir();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::disposition_tests::an_executed_spend_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(EXECUTED_ROOT, dir.path())
        .env(EXECUTED_CUT, cut)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the first launch failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
        .parse()
        .unwrap();
    let engine = engine_at(dir.path());
    let session = open(&engine);
    pool::force_keyset_spends(&session, true);
    // The failure was that write's: the relaunch's deletes succeed.
    fail_record_deletes(&session.data_dir().join(pool::SHIELDED_DB_FILE), false);
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    assert!(bind(&engine, &session, wallet));
    let [first, second]: [PendingRedrive; 2] = spends().try_into().unwrap();

    let mut stored = stored_rows(&session, wallet);
    stored.sort_unstable();
    assert_eq!(stored, [[1; 32], [2; 32]], "{cut}: the records");
    assert_eq!(
        reserved_for(&engine, &session, wallet, &first),
        first.nullifiers,
        "{cut}: spend 1's reservations"
    );
    refused(engine.block_on(session.unload_wallet(wallet)));

    let s = Arc::clone(&session);
    engine.block_on(async move { scan(&s, wallet, &scanned_notes(), &[], 5).await });
    assert_eq!(selectable(&engine, &session, wallet), [4], "{cut}");
    assert_eq!(
        shown_balance(&engine, &session, wallet),
        NOTE_VALUE,
        "{cut}"
    );
    let dp = session.dashpay(wallet);
    let to = engine
        .block_on(dp.shielded_receive_address())
        .unwrap()
        .address;
    let sent = engine.block_on(dp.shielded_send(to, 2 * NOTE_VALUE, None, "g".into(), false));
    assert_eq!(
        sent.unwrap_err().code(),
        "shielded.insufficient_funds",
        "{cut}"
    );
    refused(engine.block_on(session.unload_wallet(wallet)));

    let s = Arc::clone(&session);
    let spent = first.nullifiers.clone();
    engine.block_on(async move { scan(&s, wallet, &[], &spent, 7).await });
    spent_seen(&session, wallet, &first.nullifiers);
    assert_eq!(stored_rows(&session, wallet), [second.activity_id], "{cut}");
    assert!(reserved_for(&engine, &session, wallet, &first).is_empty());
    assert_eq!(selectable(&engine, &session, wallet), [4], "{cut}");
    assert_eq!(
        shown_balance(&engine, &session, wallet),
        NOTE_VALUE,
        "{cut}"
    );
    engine.block_on(session.ensure_background());
    pass(&engine, &session);
    assert_ne!(
        settlement(&engine, &session, wallet, 1).0,
        ArtifactStatus::NotExecuted,
        "{cut}"
    );
    refused(engine.block_on(session.unload_wallet(wallet)));
}

/// M2-a (i): an executed spend whose mark-spent failed to delete its record
/// keeps its notes reserved across a restart (here a crash right after
/// it), until the scan marks them spent
/// ([`relaunch_after_an_executed_spend`]).
#[test]
fn an_executed_spend_whose_mark_spent_failed_stays_reserved_across_a_restart() {
    relaunch_after_an_executed_spend("crash");
}

/// M2-a (ii): a quit right after a spend's executed outcome, before its
/// mark-spent, then a relaunch: the same as (i).
#[test]
fn a_quit_right_after_an_executed_spend_keeps_it_reserved_for_the_relaunch() {
    relaunch_after_an_executed_spend("quit");
}

/// M2-a (ii): Close Wallet right after a spend's executed outcome is
/// refused, before its mark-spent (the record and reservations hold it) and
/// after (the confirmed row a pass has not read holds it), and so is the
/// network close; nothing of the spend is let go. Once a pass reads the
/// row the close proceeds. After the relaunch no record comes back and the
/// artifact still reads executed; a scan that covers the spend (finding
/// its notes and their nullifiers together) leaves them out of the
/// selection and the balance.
#[test]
fn close_wallet_right_after_an_executed_spend_is_refused_until_it_is_read() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let first = spends().remove(0);
    let (s, r) = (Arc::clone(&session), first.clone());
    engine.block_on(async move {
        let m = s.manager().unwrap();
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        reserve_and_record(
            &mut *coordinator.store().write().await,
            subwallet(wallet),
            r.clone(),
        );
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
        scan(&s, wallet, &scanned_notes(), &[], 5).await;
    });
    // Spend 2's notes are unreserved here: four selectable notes, three of
    // them not spend 1's.
    let others = 3 * NOTE_VALUE;
    assert_eq!(shown_balance(&engine, &session, wallet), others);

    for finalize in [false, true] {
        let (s, r) = (Arc::clone(&session), first.clone());
        engine.block_on(async move { executed(&s, wallet, &r, finalize).await });
        refused(engine.block_on(session.unload_wallet(wallet)));
        refused(engine.block_on(engine.close_network(DashNetwork::Regtest)));
        assert!(session.live().is_ok(), "the session stays open");
        assert_eq!(
            stored_rows(&session, wallet).is_empty(),
            finalize,
            "the record is kept until the mark-spent"
        );
        assert_eq!(selectable(&engine, &session, wallet), [2, 3, 4]);
        assert_eq!(shown_balance(&engine, &session, wallet), others);
    }
    pass(&engine, &session);
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (
            ArtifactStatus::Executed,
            Some(("executed".into(), None)),
            false
        )
    );
    engine.block_on(session.unload_wallet(wallet)).unwrap();

    let session = reopen_with(&engine, session, |_| {});
    engine.block_on(session.load_wallet(wallet)).unwrap();
    assert!(bind(&engine, &session, wallet));
    assert!(stored_rows(&session, wallet).is_empty());
    let s = Arc::clone(&session);
    let spent = first.nullifiers.clone();
    engine.block_on(async move { scan(&s, wallet, &scanned_notes(), &spent, 7).await });
    assert_eq!(selectable(&engine, &session, wallet), [2, 3, 4]);
    assert_eq!(shown_balance(&engine, &session, wallet), others);
    assert_eq!(
        settlement(&engine, &session, wallet, 1).0,
        ArtifactStatus::Executed
    );
}

/// A spend the 0.1 build stored (its record with no anchor height, its
/// artifact imported and classified): each pass saves it a pending history
/// row before the library's pass, for the library's release to fail. Then
/// the release, as the library's (its notes freed, which drops its record,
/// and its row failed only if it has one), and the pass's end settles it
/// `not_executed`; a later pass saves it no row again.
#[test]
fn a_pass_keeps_an_imported_spends_row_for_its_release() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let spend = PendingRedrive {
        anchor_height: None,
        ..spends().remove(0)
    };
    let st_hash: [u8; 32] = Sha256::digest(&spend.st_bytes).into();
    let (s, m, r) = (
        Arc::clone(&session),
        session.manager().unwrap(),
        spend.clone(),
    );
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        reserve_and_record(
            &mut *coordinator.store().write().await,
            subwallet(wallet),
            r.clone(),
        );
        let imported = NewShieldedArtifact {
            activity: r.activity_id,
            origin: ArtifactOrigin::ImportedRedrive,
            st_hash: Some(st_hash),
            kind: None,
            anchor: Some(r.anchor),
            identity_id: None,
            selected: Vec::new(),
            published: Vec::new(),
        };
        assert!(
            s.leases
                .record_shielded_artifact(wallet, imported)
                .await
                .unwrap()
        );
        s.ensure_background().await;
    });
    let changed = rusqlite::Connection::open(
        session
            .data_dir()
            .join(dw_appdb::dispatch::DISPATCH_DB_FILE),
    )
    .unwrap()
    .execute(
        "UPDATE shielded_artifact SET status = 'unresolved' WHERE wallet = ?1 AND activity = ?2",
        rusqlite::params![&wallet.0[..], &spend.activity_id[..]],
    )
    .unwrap();
    assert_eq!(changed, 1);
    let activity = spend.activity_id;
    let row = |engine: &Engine| {
        let (s, m) = (Arc::clone(&session), session.manager().unwrap());
        engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            let store = coordinator.store().read().await;
            (store.get_activity_by_entry_id(subwallet(wallet), &activity))
                .unwrap()
                .map(|e| e.status)
        })
    };
    assert_eq!(row(&engine), None);

    pass(&engine, &session);
    assert_eq!(row(&engine), Some(ShieldedActivityStatus::Pending));
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (
            ArtifactStatus::Unresolved { unrecorded: false },
            None,
            false
        )
    );

    let (s, m, r) = (Arc::clone(&session), session.manager().unwrap(), spend);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        let id = subwallet(wallet);
        for nullifier in &r.nullifiers {
            assert!(store.clear_pending(id, nullifier).unwrap());
        }
        assert!(store.pending_redrives(id).unwrap().is_empty());
        if let Some(mut entry) = store.get_activity_by_entry_id(id, &r.activity_id).unwrap() {
            entry.status = ShieldedActivityStatus::Failed;
            store.save_activity(id, &entry).unwrap();
        }
    });
    pass(&engine, &session);
    assert_eq!(row(&engine), Some(ShieldedActivityStatus::Failed));
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (ArtifactStatus::NotExecuted, None, false)
    );
    pass(&engine, &session);
    assert_eq!(row(&engine), Some(ShieldedActivityStatus::Failed));
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// The history rows of `wallet`'s spend `activity` (all of its account's
/// with that id), by status, the shielded store opened as the engine opens
/// it if it is not yet.
fn import_rows(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    activity: [u8; 32],
) -> Vec<ShieldedActivityStatus> {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        (store
            .get_activity(subwallet(wallet), 0, usize::MAX)
            .unwrap()
            .into_iter())
        .filter(|e| e.id == activity)
        .map(|e| e.status)
        .collect()
    })
}

/// A launch over a spend the 0.1 build stored (its record with no anchor
/// height, its artifact imported and classified) saves its pending history
/// row at the shielded store's open, before any pass. Once the release
/// fails it and a pass settles it `not_executed`, each later launch saves
/// its failed row again at the open, exactly one, and settles nothing again.
#[test]
fn an_imported_spends_row_is_kept_at_the_stores_open() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let spend = PendingRedrive {
        anchor_height: None,
        ..spends().remove(0)
    };
    let st_hash: [u8; 32] = Sha256::digest(&spend.st_bytes).into();
    let r = spend.clone();
    let session = reopen_with(&engine, session, move |path| {
        let mut store =
            platform_wallet::wallet::shielded::FileBackedShieldedStore::open_path(path, 100)
                .unwrap();
        reserve_and_record(&mut store, subwallet(wallet), r.clone());
        drop(store);
        let journal = path.with_file_name(dw_appdb::dispatch::DISPATCH_DB_FILE);
        let inserted = rusqlite::Connection::open(journal)
            .unwrap()
            .execute(
                "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, status, at) \
                 VALUES (?1, ?2, 'imported_redrive', ?3, 'unresolved', 1)",
                rusqlite::params![&wallet.0[..], &r.activity_id[..], &st_hash[..]],
            )
            .unwrap();
        assert_eq!(inserted, 1);
    });
    engine.block_on(session.load_wallet(wallet)).unwrap();
    assert!(
        (session.manager().unwrap().list_wallet_ids_blocking()).contains(&wallet.0),
        "the wallet is loaded"
    );
    let open = ArtifactStatus::Unresolved { unrecorded: false };
    assert_eq!(settlement(&engine, &session, wallet, 1).0, open);
    let activity = spend.activity_id;
    assert_eq!(
        import_rows(&engine, &session, wallet, activity),
        [ShieldedActivityStatus::Pending]
    );

    let (s, m, r) = (Arc::clone(&session), session.manager().unwrap(), spend);
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        {
            let mut store = coordinator.store().write().await;
            let id = subwallet(wallet);
            for nullifier in &r.nullifiers {
                assert!(store.clear_pending(id, nullifier).unwrap());
            }
            let mut entry = (store.get_activity_by_entry_id(id, &r.activity_id))
                .unwrap()
                .unwrap();
            entry.status = ShieldedActivityStatus::Failed;
            store.save_activity(id, &entry).unwrap();
        }
        s.ensure_background().await;
    });
    pass(&engine, &session);
    let settled = (ArtifactStatus::NotExecuted, None, false);
    assert_eq!(settlement(&engine, &session, wallet, 1), settled);

    let mut session = session;
    for _ in 0..2 {
        session = reopen_with(&engine, session, |_| {});
        engine.block_on(session.load_wallet(wallet)).unwrap();
        assert_eq!(
            import_rows(&engine, &session, wallet, activity),
            [ShieldedActivityStatus::Failed]
        );
        assert_eq!(settlement(&engine, &session, wallet, 1).0, settled.0);
    }
    engine.block_on(session.ensure_background());
    pass(&engine, &session);
    assert_eq!(
        import_rows(&engine, &session, wallet, activity),
        [ShieldedActivityStatus::Failed]
    );
    assert_eq!(settlement(&engine, &session, wallet, 1).0, settled.0);
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// A launch over a spend the 0.1 build stored, its record with no anchor
/// height and its artifact imported and classified, the wallet loaded: its
/// pending history row is saved at the shielded store's open.
fn launch_with_import(engine: &Engine) -> (Arc<NetworkSession>, WalletId, PendingRedrive) {
    let (session, wallet) = first_launch(engine);
    let spend = PendingRedrive {
        anchor_height: None,
        ..spends().remove(0)
    };
    let st_hash: [u8; 32] = Sha256::digest(&spend.st_bytes).into();
    let r = spend.clone();
    let session = reopen_with(engine, session, move |path| {
        let mut store =
            platform_wallet::wallet::shielded::FileBackedShieldedStore::open_path(path, 100)
                .unwrap();
        reserve_and_record(&mut store, subwallet(wallet), r.clone());
        drop(store);
        let journal = path.with_file_name(dw_appdb::dispatch::DISPATCH_DB_FILE);
        let inserted = rusqlite::Connection::open(journal)
            .unwrap()
            .execute(
                "INSERT INTO shielded_artifact (wallet, activity, origin, st_hash, status, at) \
                 VALUES (?1, ?2, 'imported_redrive', ?3, 'unresolved', 1)",
                rusqlite::params![&wallet.0[..], &r.activity_id[..], &st_hash[..]],
            )
            .unwrap();
        assert_eq!(inserted, 1);
    });
    engine.block_on(session.load_wallet(wallet)).unwrap();
    assert_eq!(
        import_rows(engine, &session, wallet, spend.activity_id),
        [ShieldedActivityStatus::Pending]
    );
    (session, wallet, spend)
}

/// Frees the notes of imported spend `r` in the library's store, which
/// drops its record, leaving its history row as it is; then whether the
/// library reports an outcome the journal has not read
/// (`shielded_pool::reported_outcome`).
fn free_notes(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    r: &PendingRedrive,
) -> Option<bool> {
    let (s, m, r) = (Arc::clone(session), session.manager().unwrap(), r.clone());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        {
            let mut store = coordinator.store().write().await;
            let id = subwallet(wallet);
            for nullifier in &r.nullifiers {
                assert!(store.clear_pending(id, nullifier).unwrap());
            }
            assert!(store.pending_redrives(id).unwrap().is_empty());
        }
        s.ensure_background().await;
        pool::reported_outcome(&s.leases, &coordinator, wallet).await
    })
}

/// Whether the library reports an outcome of `wallet`'s spends the journal
/// has not read (`shielded_pool::reported_outcome`).
fn reported(engine: &Engine, session: &Arc<NetworkSession>, wallet: WalletId) -> Option<bool> {
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        pool::reported_outcome(&s.leases, &coordinator, wallet).await
    })
}

/// The journal's settlement of an imported spend freed with no proof of
/// its outcome: open, under the library's `Unattributable` strings.
fn freed_unknown() -> Settlement {
    (
        ArtifactStatus::Unresolved { unrecorded: false },
        Some(("unresolved".to_owned(), Some("unattributable".to_owned()))),
        false,
    )
}

/// An imported spend's pending row with nothing of it held is an outcome
/// the journal has not read: the close and an idle pass wait for it. A
/// pass reads it unknown (its notes are not spent), which holds them until
/// the journal marks it; the mark lifts the wait for good.
#[test]
fn a_freed_imports_pending_row_holds_until_the_journal_marks_it() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet, spend) = launch_with_import(&engine);
    assert_eq!(reported(&engine, &session, wallet), Some(false));

    assert_eq!(free_notes(&engine, &session, wallet, &spend), Some(true));
    let (s, m, activity) = (
        Arc::clone(&session),
        session.manager().unwrap(),
        spend.activity_id,
    );
    let artifact = engine
        .block_on(session.leases.shielded_artifact(wallet, activity))
        .unwrap()
        .unwrap();
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        pool::spend_outcome(&mut *store, subwallet(wallet), &artifact).unwrap();
    });
    assert_eq!(
        import_rows(&engine, &session, wallet, activity),
        [ShieldedActivityStatus::Unknown]
    );
    assert_eq!(reported(&engine, &session, wallet), Some(true));

    pass(&engine, &session);
    assert_eq!(settlement(&engine, &session, wallet, 1), freed_unknown());
    assert_eq!(reported(&engine, &session, wallet), Some(false));
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// M1: an imported spend whose notes came free with its row pending reads
/// unknown and the journal marks it so. The library's delete of its record
/// never reached disk, so the record comes back at the next launch, and
/// the notes it took are then spent by another payment, which drops the
/// record. The marked spend gets no row at the open, and the passes read
/// nothing of it: never pending, never done, never settled executed, and
/// nothing holds the close.
#[test]
fn a_freed_imports_returning_record_never_settles_executed() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet, spend) = launch_with_import(&engine);
    let activity = spend.activity_id;
    free_notes(&engine, &session, wallet, &spend);
    pass(&engine, &session);
    assert_eq!(
        import_rows(&engine, &session, wallet, activity),
        [ShieldedActivityStatus::Unknown]
    );
    let marked = settlement(&engine, &session, wallet, 1);

    let r = spend.clone();
    let session = reopen_with(&engine, session, move |path| {
        let mut store =
            platform_wallet::wallet::shielded::FileBackedShieldedStore::open_path(path, 100)
                .unwrap();
        reserve_and_record(&mut store, subwallet(wallet), r);
    });
    engine.block_on(session.load_wallet(wallet)).unwrap();
    let rows_at_open = import_rows(&engine, &session, wallet, activity);
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, true);
    }
    let (s, m) = (Arc::clone(&session), session.manager().unwrap());
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let store = coordinator.store().read().await;
        assert!(
            store
                .pending_redrives(subwallet(wallet))
                .unwrap()
                .is_empty()
        );
        drop(store);
        s.ensure_background().await;
    });
    pass(&engine, &session);
    assert_eq!(
        (
            rows_at_open,
            import_rows(&engine, &session, wallet, activity),
            settlement(&engine, &session, wallet, 1),
        ),
        (Vec::new(), Vec::new(), freed_unknown())
    );
    assert_eq!(marked, freed_unknown());
    assert_eq!(reported(&engine, &session, wallet), Some(false));
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// The activity of [`spend_freed_notes`]' spend.
const AGAIN: [u8; 32] = [0x5a; 32];

/// Where the child of the #931 tests keeps its data root, and where it
/// stops ([`an_undecided_import_child`]).
const UNDECIDED_ROOT: &str = "DW_D1_UNDECIDED_ROOT";
const UNDECIDED_CUT: &str = "DW_D1_UNDECIDED_CUT";

/// Makes every settlement the dispatch journal at `path` writes fail
/// (`on`), as a failed write of an imported spend's [`freed_unknown`] mark
/// does, or lets them succeed again.
fn fail_journal_settlements(path: &Path, on: bool) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    db.execute_batch(if on {
        "CREATE TRIGGER d1_fail_settle BEFORE UPDATE ON shielded_artifact \
         BEGIN SELECT RAISE(FAIL, 'injected settlement failure'); END;"
    } else {
        "DROP TRIGGER IF EXISTS d1_fail_settle;"
    })
    .unwrap();
}

/// Another spend taking the notes of imported spend `r`, as the library
/// stores it and asks the dispatch hook before its first broadcast. What
/// the library then does: an accepted spend executes (its notes are
/// marked spent), and either way its record goes. The hook's answer.
fn spend_freed_notes(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    r: &PendingRedrive,
) -> Result<(), String> {
    use platform_wallet::wallet::shielded::{
        DispatchedNote, ShieldedDispatch, ShieldedDispatchKind, ShieldedDispatchPayload,
    };
    let (s, m) = (Arc::clone(session), session.manager().unwrap());
    let again = PendingRedrive {
        activity_id: AGAIN,
        st_bytes: spends().remove(1).st_bytes,
        attempts: 0,
        ..r.clone()
    };
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let id = subwallet(wallet);
        let selected_notes = {
            let mut store = coordinator.store().write().await;
            store.arm_redrive(id, again.clone()).unwrap();
            (store.get_all_notes(id).unwrap().into_iter())
                .filter(|n| again.nullifiers.contains(&n.nullifier))
                .map(|n| DispatchedNote {
                    nullifier: n.nullifier,
                    cmx: n.cmx,
                    value: n.value,
                    position: n.position,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(selected_notes.len(), again.nullifiers.len());
        let payload = ShieldedDispatchPayload {
            kind: ShieldedDispatchKind::Transfer,
            wallet_id: wallet.0,
            account_index: dw_vault::SHIELDED_ACCOUNT,
            activity_id: AGAIN,
            st_hash: Sha256::digest(&again.st_bytes).into(),
            st_bytes: again.st_bytes.clone(),
            anchor: again.anchor,
            selected_notes,
            published_nullifiers: again.nullifiers.clone(),
        };
        let hook = pool::EngineDispatch::new(&s, &coordinator);
        let answer = hook.dispatch(payload).await.map_err(|e| e.reason);
        let mut store = coordinator.store().write().await;
        if answer.is_ok() {
            for nullifier in &again.nullifiers {
                store.mark_spent(id, nullifier).unwrap();
            }
        }
        store.clear_redrive(id, &AGAIN).unwrap();
        answer
    })
}

/// The refusal of a spend taking an undecided import's notes.
const UNDECIDED: &str =
    "the spend takes a note of an imported spend whose outcome is not recorded yet";

/// #931's first launch, in a child process: an imported spend's notes come
/// free. Its record's delete fails, so the record stays on disk, except at
/// `deleted`, where it reaches disk.
/// - `unmarked`: a pass reads its row unknown and the journal's write of
///   the mark fails;
/// - `unread` and `deleted`: its row is read unknown, the cut before the
///   mark's write.
///
/// Then, but at `deleted`, another spend takes the freed notes in the same
/// session. The process exits with no quit, leaving the hook's answer
/// (`accepted`, its refusal, or `none`) and the wallet in the data root.
#[test]
#[ignore = "the child process of the #931 tests"]
fn an_undecided_import_child() {
    let Some(root) = std::env::var_os(UNDECIDED_ROOT) else {
        return;
    };
    let cut = std::env::var(UNDECIDED_CUT).unwrap();
    let engine = engine_at(Path::new(&root));
    let (session, wallet, spend) = launch_with_import(&engine);
    pool::force_keyset_spends(&session, true);
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, false);
    }
    let shielded = session.data_dir().join(pool::SHIELDED_DB_FILE);
    let journal = shielded.with_file_name(dw_appdb::dispatch::DISPATCH_DB_FILE);
    fail_record_deletes(&shielded, cut != "deleted");
    assert_eq!(free_notes(&engine, &session, wallet, &spend), Some(true));
    fail_record_deletes(&shielded, false);
    if cut == "unmarked" {
        fail_journal_settlements(&journal, true);
        pass(&engine, &session);
    } else {
        let (s, m) = (Arc::clone(&session), session.manager().unwrap());
        let artifact = engine
            .block_on(session.leases.shielded_artifact(wallet, spend.activity_id))
            .unwrap()
            .unwrap();
        engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            let mut store = coordinator.store().write().await;
            pool::spend_outcome(&mut *store, subwallet(wallet), &artifact).unwrap();
        });
    }
    assert_eq!(
        import_rows(&engine, &session, wallet, spend.activity_id),
        [ShieldedActivityStatus::Unknown]
    );
    assert_eq!(settlement(&engine, &session, wallet, 1).1, None);
    let answer = match cut.as_str() {
        "deleted" => "none".to_owned(),
        _ => match spend_freed_notes(&engine, &session, wallet, &spend) {
            Ok(()) => "accepted".to_owned(),
            Err(why) => why,
        },
    };
    fail_journal_settlements(&journal, false);
    let root = Path::new(&root);
    std::fs::write(root.join("answer"), answer).unwrap();
    std::fs::write(root.join("wallet"), wallet.to_string()).unwrap();
    std::process::exit(0);
}

/// The relaunch after [`an_undecided_import_child`] cut at `cut`, spends
/// forced on and the wallet loaded: the engine, the session, the wallet,
/// the imported spend and the hook's answer in the first launch. Its data
/// root is `root`.
fn relaunch_after_an_undecided_import(
    root: &Path,
    cut: &str,
) -> (
    Engine,
    Arc<NetworkSession>,
    WalletId,
    PendingRedrive,
    String,
) {
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "platform::shielded_close_tests::disposition_tests::an_undecided_import_child",
            "--exact",
            "--ignored",
            "--test-threads=1",
        ])
        .env(UNDECIDED_ROOT, root)
        .env(UNDECIDED_CUT, cut)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "the first launch failed: {}",
        String::from_utf8_lossy(&child.stderr)
    );
    let answer = std::fs::read_to_string(root.join("answer")).unwrap();
    let wallet: WalletId = (std::fs::read_to_string(root.join("wallet")).unwrap())
        .parse()
        .unwrap();
    let engine = engine_at(root);
    let session = open(&engine);
    pool::force_keyset_spends(&session, true);
    engine
        .block_on(session.vault_op(|v| v.unlock(PASS, UnlockScope::Full)))
        .unwrap();
    engine.block_on(session.load_wallet(wallet)).unwrap();
    let spend = PendingRedrive {
        anchor_height: None,
        ..spends().remove(0)
    };
    (engine, session, wallet, spend, answer)
}

/// #931 (D1): the relaunch after [`an_undecided_import_child`] cut at
/// `cut`, the hook having answered the spend of the import's freed notes.
/// The import's notes are found again, spent if that spend went (its notes
/// were spent on Platform). The hook refused it, so they are unspent: the
/// import's record is back from disk with its notes held, and a pass
/// settles nothing. The import never reads executed.
fn an_undecided_import_never_reads_executed(cut: &str) {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, wallet, spend, answer) =
        relaunch_after_an_undecided_import(dir.path(), cut);
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, answer == "accepted");
    }
    pass(&engine, &session);
    let (status, disposition, _) = settlement(&engine, &session, wallet, 1);
    assert_eq!(
        (
            answer.as_str(),
            status == ArtifactStatus::Executed,
            disposition,
            stored_rows(&session, wallet),
        ),
        (UNDECIDED, false, None, vec![spend.activity_id]),
        "{cut}"
    );
    assert_eq!(
        reserved_for(&engine, &session, wallet, &spend),
        spend.nullifiers,
        "{cut}"
    );
}

/// #931's four steps: an imported spend's notes come free, the journal's
/// write of its unknown mark fails, the library's delete of its record
/// fails, and another spend takes the freed notes in the same session,
/// then a restart. The hook refuses that spend, so the import's returning
/// record never reads executed.
#[test]
fn a_spend_of_an_imports_freed_notes_waits_for_its_mark() {
    an_undecided_import_never_reads_executed("unmarked");
}

/// #931: the window between an imported spend's unknown row and the
/// journal's write of its mark, then a crash. A spend of its freed notes in
/// that window is refused, and the relaunch never reads the import
/// executed.
#[test]
fn a_crash_before_an_imports_mark_never_settles_it_executed() {
    an_undecided_import_never_reads_executed("unread");
}

/// #931, no lockout: a crash before an imported spend's mark, after the
/// library's delete of its record reached disk. The relaunch has no record
/// and no row of it, nothing reads it again, and a spend of its notes is
/// accepted though the journal never marked it.
#[test]
fn an_unmarked_import_with_no_record_left_does_not_hold_its_notes() {
    let dir = dw_testutil::private_tempdir();
    let (engine, session, wallet, spend, answer) =
        relaunch_after_an_undecided_import(dir.path(), "deleted");
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, false);
    }
    assert_eq!(
        (
            answer.as_str(),
            import_rows(&engine, &session, wallet, spend.activity_id),
            settlement(&engine, &session, wallet, 1).1,
            stored_rows(&session, wallet),
        ),
        ("none", Vec::new(), None, Vec::new())
    );
    assert_eq!(spend_freed_notes(&engine, &session, wallet, &spend), Ok(()));
}

/// #931 for a release: an imported spend released (its row failed, its
/// notes freed) whose `not_executed` settlement fails to write. A spend of
/// its notes is refused until a pass writes it, then accepted.
#[test]
fn a_spend_of_a_released_imports_notes_waits_for_its_settlement() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet, spend) = launch_with_import(&engine);
    pool::force_keyset_spends(&session, true);
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, false);
    }
    free_notes(&engine, &session, wallet, &spend);
    let (s, m, r) = (
        Arc::clone(&session),
        session.manager().unwrap(),
        spend.clone(),
    );
    engine.block_on(async move {
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        let mut store = coordinator.store().write().await;
        history_row(
            &mut *store,
            subwallet(wallet),
            &r,
            ShieldedActivityStatus::Failed,
        );
    });
    let journal = (session.data_dir().join(pool::SHIELDED_DB_FILE))
        .with_file_name(dw_appdb::dispatch::DISPATCH_DB_FILE);
    fail_journal_settlements(&journal, true);
    pass(&engine, &session);
    let unwritten = settlement(&engine, &session, wallet, 1).0;
    let refused = spend_freed_notes(&engine, &session, wallet, &spend);
    fail_journal_settlements(&journal, false);
    pass(&engine, &session);
    assert_eq!(
        (
            unwritten == ArtifactStatus::NotExecuted,
            refused,
            settlement(&engine, &session, wallet, 1).0,
            spend_freed_notes(&engine, &session, wallet, &spend),
        ),
        (
            false,
            Err(UNDECIDED.to_owned()),
            ArtifactStatus::NotExecuted,
            Ok(())
        )
    );
}

/// Makes the dispatch journal's read of `wallet`'s artifacts fail (`on`):
/// a selected note of imported spend `activity` whose nullifier is not 32
/// bytes, written past the table's checks; or removes it.
fn damage_artifact_read(path: &Path, wallet: WalletId, activity: [u8; 32], on: bool) {
    let db = rusqlite::Connection::open(path).unwrap();
    db.busy_timeout(Duration::from_secs(10)).unwrap();
    if on {
        db.execute_batch("PRAGMA ignore_check_constraints = ON;")
            .unwrap();
        db.execute(
            "INSERT INTO shielded_artifact_note (wallet, activity, nullifier) VALUES (?1, ?2, ?3)",
            rusqlite::params![&wallet.0[..], &activity[..], &[7u8; 31][..]],
        )
        .unwrap();
    } else {
        db.execute(
            "DELETE FROM shielded_artifact_note WHERE length(nullifier) <> 32",
            [],
        )
        .unwrap();
    }
}

/// #931, the pass: one whose keep of the imported spends' history rows
/// cannot read the artifacts is held back before the library's pass while
/// the library holds a record or a reserved note of a spend with no row,
/// so no release or scan in it frees notes with no row to read the spend's
/// outcome by or to hold them from a new spend. A spend whose row was kept
/// keeps it, so with every held spend's row kept the pass runs.
#[test]
fn a_pass_is_held_while_a_held_spends_row_cannot_be_kept() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet, spend) = launch_with_import(&engine);
    let journal = session
        .data_dir()
        .join(dw_appdb::dispatch::DISPATCH_DB_FILE);
    let runs = || (session.platform.passes_run).load(std::sync::atomic::Ordering::SeqCst);
    let reserved = [0x5b; 32];
    let pass_with = |rowless: Option<bool>| {
        let (s, m) = (Arc::clone(&session), session.manager().unwrap());
        let again = PendingRedrive {
            activity_id: AGAIN,
            st_bytes: spends().remove(1).st_bytes,
            ..spend.clone()
        };
        let before = runs();
        let ran = engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            {
                let id = subwallet(wallet);
                let mut store = coordinator.store().write().await;
                store.clear_redrive(id, &AGAIN).unwrap();
                store.clear_pending(id, &reserved).unwrap();
                match rowless {
                    Some(true) => store.arm_redrive(id, again).unwrap(),
                    Some(false) => {
                        assert!(store.mark_pending(id, &reserved).unwrap());
                        (store.set_pending_spend(id, &reserved, again.anchor, AGAIN, None))
                            .unwrap();
                    }
                    None => {}
                }
            }
            pool::guarded_pass(&s, &m, true).await
        });
        (ran, runs() - before)
    };
    damage_artifact_read(&journal, wallet, spend.activity_id, true);
    assert_eq!(
        [
            pass_with(Some(true)),
            pass_with(Some(false)),
            pass_with(None)
        ],
        [(false, 0), (false, 0), (true, 1)]
    );
}

/// #931: the wait ends with the mark. An imported spend's pending row
/// with nothing of it held refuses a spend of its notes, as does its
/// unknown row; once a pass writes the mark, the same spend is accepted.
#[test]
fn a_spend_of_an_imports_freed_notes_is_accepted_once_it_is_marked() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet, spend) = launch_with_import(&engine);
    pool::force_keyset_spends(&session, true);
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, false);
    }
    assert_eq!(free_notes(&engine, &session, wallet, &spend), Some(true));
    let pending = spend_freed_notes(&engine, &session, wallet, &spend);
    pass(&engine, &session);
    assert_eq!(settlement(&engine, &session, wallet, 1), freed_unknown());
    assert_eq!(
        (
            pending,
            spend_freed_notes(&engine, &session, wallet, &spend)
        ),
        (Err(UNDECIDED.to_owned()), Ok(()))
    );
}

/// A spend the engine dispatched is never confirmed by a pass: its pending
/// row, with nothing of it held and every note it took read spent, settles
/// nothing (the library's own scan confirms its rows), and the row stays
/// pending.
#[test]
fn a_pass_never_confirms_a_dispatched_spends_pending_row() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine_at(dir.path());
    let (session, wallet) = first_launch(&engine);
    let spend = spends().remove(0);
    let (s, m, r) = (
        Arc::clone(&session),
        session.manager().unwrap(),
        spend.clone(),
    );
    engine.block_on(async move {
        let dispatched = NewShieldedArtifact {
            activity: r.activity_id,
            origin: ArtifactOrigin::Dispatched,
            st_hash: Some(Sha256::digest(&r.st_bytes).into()),
            kind: Some(ArtifactKind::Transfer),
            anchor: Some(r.anchor),
            identity_id: None,
            selected: r.nullifiers.clone(),
            published: r.nullifiers.clone(),
        };
        assert!(
            s.leases
                .record_shielded_artifact(wallet, dispatched)
                .await
                .unwrap()
        );
        let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
        history_row(
            &mut *coordinator.store().write().await,
            subwallet(wallet),
            &r,
            ShieldedActivityStatus::Pending,
        );
        s.ensure_background().await;
    });
    for nullifier in &spend.nullifiers {
        store_note(&engine, &session, wallet, *nullifier, true);
    }
    pass(&engine, &session);
    assert_eq!(
        settlement(&engine, &session, wallet, 1),
        (ArtifactStatus::Registered, None, false)
    );
    assert_eq!(
        import_rows(&engine, &session, wallet, spend.activity_id),
        [ShieldedActivityStatus::Pending]
    );
    engine.block_on(session.unload_wallet(wallet)).unwrap();
}

/// The fork's tests, against its dispositions (`resolve_redrive`,
/// `spend_dispositions`, `ack_spend_disposition`), which upstream does not
/// have.
#[cfg(any())]
mod fork {
    use std::sync::atomic::Ordering;

    use platform_wallet::wallet::shielded::{
        ShieldedActivityEntry, ShieldedActivityKind, ShieldedActivityStatus, ShieldedDirection,
        ShieldedDispatch,
    };

    use super::*;

    /// Where the crashing child process keeps its data root.
    const CRASH_ROOT: &str = "DW_A5_CRASH_ROOT";

    fn st_hash(n: u8) -> [u8; 32] {
        Sha256::digest(&spends()[usize::from(n) - 1].st_bytes).into()
    }

    fn nullifiers(n: u8) -> Vec<[u8; 32]> {
        spends()[usize::from(n) - 1].nullifiers.clone()
    }

    /// Resolves the stored spend `n` as the library's scan or send does.
    fn resolve(
        engine: &Engine,
        session: &Arc<NetworkSession>,
        wallet: WalletId,
        n: u8,
        disposition: ShieldedSpendDisposition,
    ) {
        let (s, m) = (Arc::clone(session), session.manager().unwrap());
        engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            (coordinator.store().write().await)
                .resolve_redrive(subwallet(wallet), &[n; 32], disposition)
                .unwrap();
        });
    }

    /// One shielded pass (offline: its scan fails, and its end still runs).
    fn pass(engine: &Engine, session: &Arc<NetworkSession>) {
        let (s, m) = (Arc::clone(session), session.manager().unwrap());
        assert!(
            engine.block_on(async move { pool::guarded_pass(&s, &m, true).await }),
            "the pass ran"
        );
    }

    /// What the journal holds of spend `n`: its status, the library's
    /// disposition strings and whether it was acknowledged.
    type Settlement = (ArtifactStatus, Option<(String, Option<String>)>, bool);

    fn settlement(
        engine: &Engine,
        session: &Arc<NetworkSession>,
        wallet: WalletId,
        n: u8,
    ) -> Settlement {
        let artifact = engine
            .block_on(session.leases.shielded_artifact(wallet, [n; 32]))
            .unwrap()
            .unwrap();
        (artifact.status, artifact.disposition, artifact.acked)
    }

    fn stored(kind: &str, reason: Option<&str>) -> Option<(String, Option<String>)> {
        Some((kind.into(), reason.map(Into::into)))
    }

    /// An outcome the library lists: its `st_hash` and the notes it holds.
    type Held = ([u8; 32], Vec<[u8; 32]>);

    /// The outcomes the library lists for `wallet`.
    fn listing(
        engine: &Engine,
        session: &Arc<NetworkSession>,
        wallet: WalletId,
    ) -> Result<Vec<Held>, String> {
        let (s, m) = (Arc::clone(session), session.manager().unwrap());
        engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            let listed = coordinator.spend_dispositions(wallet.0).await;
            listed
                .map(|records| {
                    (records.into_iter())
                        .map(|d| (d.st_hash, d.held_nullifiers))
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
    }

    fn refused<T: std::fmt::Debug>(r: Result<T, EngineError>) {
        assert!(
            matches!(r, Err(EngineError::ShieldedSpendUnresolved)),
            "{r:?}"
        );
    }

    /// An executed spend lets the wallet go once it is acknowledged: after a
    /// pass, a spend whose notes the persister holds spent is settled
    /// `executed` and acknowledged, while one whose notes it has not seen
    /// spent yet is settled but keeps its hold (and Close Wallet stays
    /// refused) until a later pass finds them spent. Then Close Wallet and the
    /// network close succeed.
    #[test]
    fn executed_ack_lifts_the_close_refusal() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        store_spends(&engine, &session, wallet);
        resolve(
            &engine,
            &session,
            wallet,
            1,
            ShieldedSpendDisposition::Executed,
        );
        resolve(
            &engine,
            &session,
            wallet,
            2,
            ShieldedSpendDisposition::Executed,
        );
        spent_seen(&session, wallet, &nullifiers(1));
        refused(engine.block_on(session.unload_wallet(wallet)));

        pass(&engine, &session);
        let executed = (ArtifactStatus::Executed, stored("executed", None));
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (executed.0, executed.1.clone(), true)
        );
        assert_eq!(
            settlement(&engine, &session, wallet, 2),
            (executed.0, executed.1.clone(), false)
        );
        assert_eq!(
            listing(&engine, &session, wallet),
            Ok(vec![(st_hash(2), nullifiers(2))])
        );
        refused(engine.block_on(session.unload_wallet(wallet)));

        spent_seen(&session, wallet, &nullifiers(2));
        pass(&engine, &session);
        assert_eq!(listing(&engine, &session, wallet), Ok(Vec::new()));
        assert_eq!(
            settlement(&engine, &session, wallet, 2),
            (executed.0, executed.1, true)
        );
        engine.block_on(session.unload_wallet(wallet)).unwrap();
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
    }

    /// Run by [`disposition_replayed_after_crash_before_ack`] in a child
    /// process: an executed spend, its notes seen spent, and a pass that
    /// crashes between the journal's settlement and the acknowledgement; then
    /// the app quits. It quits rather than aborting so that it can report what
    /// it left: a quit runs no pass and does not configure the shielded store
    /// again, so it neither settles nor acknowledges an outcome, and the
    /// relaunch's open is the first to read it again.
    #[test]
    #[ignore = "the child process of disposition_replayed_after_crash_before_ack"]
    fn crash_before_ack_child() {
        let Some(root) = std::env::var_os(CRASH_ROOT) else {
            return;
        };
        let engine = engine_at(Path::new(&root));
        let (session, wallet) = first_launch(&engine);
        store_spends(&engine, &session, wallet);
        resolve(
            &engine,
            &session,
            wallet,
            1,
            ShieldedSpendDisposition::Executed,
        );
        spent_seen(&session, wallet, &nullifiers(1));
        session
            .platform
            .crash_before_ack
            .store(true, Ordering::SeqCst);
        pass(&engine, &session);
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (ArtifactStatus::Executed, stored("executed", None), true)
        );
        assert_eq!(
            listing(&engine, &session, wallet),
            Ok(vec![(st_hash(1), nullifiers(1))]),
            "not acknowledged before the crash"
        );
        std::fs::write(Path::new(&root).join("wallet"), wallet.to_string()).unwrap();
        engine.block_on(engine.shutdown()).unwrap();
        std::process::exit(0);
    }

    /// A crash after the journal's settlement and before the acknowledgement
    /// leaves the outcome listed; the next launch's open of the shielded
    /// store writes the same settlement again and then acknowledges it.
    #[test]
    fn disposition_replayed_after_crash_before_ack() {
        let dir = dw_testutil::private_tempdir();
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "platform::shielded_close_tests::disposition_tests::crash_before_ack_child",
                "--exact",
                "--ignored",
                "--test-threads=1",
            ])
            .env(CRASH_ROOT, dir.path())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "the crashing process failed: {}{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        let wallet: WalletId = (std::fs::read_to_string(dir.path().join("wallet")).unwrap())
            .parse()
            .unwrap();

        let engine = engine_at(dir.path());
        let session = open(&engine);
        assert!(bind(&engine, &session, wallet), "the store's open");
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (ArtifactStatus::Executed, stored("executed", None), true)
        );
        assert_eq!(listing(&engine, &session, wallet), Ok(Vec::new()));
    }

    /// W3: a spend the library resolves `Unresolved(Unattributable)` while its
    /// history row reads confirmed is done, never failed: settled `executed`,
    /// its disposition kept as the library wrote it. It is no proof, so it is
    /// not acknowledged (its notes stay held, Close Wallet stays refused, even
    /// with its notes seen spent), its row stays confirmed and nothing of it
    /// is armed again. Without the confirmed row it reads as being confirmed.
    #[test]
    fn unattributable_with_confirmed_activity_reads_done_and_offers_no_retry() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        store_spends(&engine, &session, wallet);
        let unattributable = ShieldedSpendDisposition::Unresolved(UnresolvedReason::Unattributable);
        resolve(&engine, &session, wallet, 1, unattributable);
        resolve(&engine, &session, wallet, 2, unattributable);
        let confirmed = ShieldedActivityEntry {
            id: [1; 32],
            kind: ShieldedActivityKind::Sent,
            direction: ShieldedDirection::Out,
            amount: 1_000,
            fee: None,
            counterparty: None,
            memo: None,
            block_height: Some(7),
            status: ShieldedActivityStatus::Confirmed,
            created_at_ms: 0,
            min_note_position: None,
            note_cmxs: Vec::new(),
            spent_nullifiers: nullifiers(1),
        };
        let (s, m) = (Arc::clone(&session), session.manager().unwrap());
        let entry = confirmed.clone();
        let coordinator = engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            (coordinator.store().write().await)
                .save_activity(subwallet(wallet), &entry)
                .unwrap();
            coordinator
        });
        spent_seen(&session, wallet, &[nullifiers(1), nullifiers(2)].concat());

        pass(&engine, &session);
        let kept = stored("unresolved", Some("unattributable"));
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (ArtifactStatus::Executed, kept.clone(), false)
        );
        assert_eq!(
            settlement(&engine, &session, wallet, 2),
            (
                ArtifactStatus::Unresolved { unrecorded: false },
                kept,
                false
            )
        );
        let mut held = vec![(st_hash(1), nullifiers(1)), (st_hash(2), nullifiers(2))];
        held.sort();
        assert_eq!(
            listing(&engine, &session, wallet),
            Ok(held),
            "both still hold their notes"
        );
        let c = Arc::clone(&coordinator);
        let (row, armed) = engine.block_on(async move {
            let store = c.store().read().await;
            (
                store
                    .get_activity_by_entry_id(subwallet(wallet), &[1; 32])
                    .unwrap(),
                store.pending_redrives(subwallet(wallet)).unwrap(),
            )
        });
        assert_eq!(row, Some(confirmed));
        assert!(armed.is_empty(), "{armed:?}");
        refused(engine.block_on(session.unload_wallet(wallet)));
    }

    /// An outcome the library cannot read fails the wallet's whole listing. It
    /// is acknowledged by its key only once each note it holds reads spent in
    /// the journal (so nothing it held becomes selectable), its artifact read
    /// as unknown; the listing then resumes and the next outcome (here a
    /// `NotExecuted` one) is settled and acknowledged.
    #[test]
    fn unreadable_disposition_row_is_acked_by_key_and_listing_resumes() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        store_spends(&engine, &session, wallet);
        resolve(
            &engine,
            &session,
            wallet,
            1,
            ShieldedSpendDisposition::Executed,
        );
        resolve(
            &engine,
            &session,
            wallet,
            2,
            ShieldedSpendDisposition::NotExecuted(NotExecutedReason::Pruned),
        );
        let db =
            rusqlite::Connection::open(session.data_dir().join(pool::SHIELDED_DB_FILE)).unwrap();
        let corrupted = db
            .execute(
                "UPDATE shielded_spend_dispositions SET at = -1 WHERE st_hash = ?1",
                [&st_hash(1)[..]],
            )
            .unwrap();
        assert_eq!(corrupted, 1);
        drop(db);
        assert!(listing(&engine, &session, wallet).is_err());

        pass(&engine, &session);
        assert!(
            listing(&engine, &session, wallet).is_err(),
            "its notes are not seen spent: it stays"
        );
        assert_eq!(
            settlement(&engine, &session, wallet, 2).0,
            ArtifactStatus::Registered
        );

        spent_seen(&session, wallet, &nullifiers(1));
        pass(&engine, &session);
        assert_eq!(listing(&engine, &session, wallet), Ok(Vec::new()));
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (ArtifactStatus::Unresolved { unrecorded: false }, None, true)
        );
        assert_eq!(
            settlement(&engine, &session, wallet, 2),
            (
                ArtifactStatus::NotExecuted,
                stored("not_executed", Some("pruned")),
                true
            )
        );

        engine.block_on(session.ensure_background());
        assert!(session.leases.background(&wallet).is_some());
        let (s, m) = (Arc::clone(&session), session.manager().unwrap());
        let readmitted = engine.block_on(async move {
            let coordinator = pool::coordinator(&s, &m, None).await.unwrap();
            let hook = pool::EngineDispatch::new(&s, &coordinator);
            let account = subwallet(wallet).account_index;
            (hook.admit(wallet.0, account, [1; 32], st_hash(1)).await).map_err(|e| e.reason)
        });
        assert_eq!(
            readmitted,
            Err("the spend's artifact does not allow a send".to_owned()),
            "an acknowledged artifact admits no send"
        );
    }

    /// A failed first send under the hook leaves the spend `Unresolved
    /// (Cancelled)`: it may still execute. It reads as being confirmed
    /// (`unresolved`, its disposition kept) and is never acknowledged here,
    /// even with its notes seen spent: they stay held and Close Wallet stays
    /// refused.
    #[test]
    fn cancelled_first_send_holds_notes_and_reads_checking() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine_at(dir.path());
        let (session, wallet) = first_launch(&engine);
        store_spends(&engine, &session, wallet);
        resolve(
            &engine,
            &session,
            wallet,
            1,
            ShieldedSpendDisposition::Unresolved(UnresolvedReason::Cancelled),
        );
        spent_seen(&session, wallet, &nullifiers(1));

        pass(&engine, &session);
        assert_eq!(
            settlement(&engine, &session, wallet, 1),
            (
                ArtifactStatus::Unresolved { unrecorded: false },
                stored("unresolved", Some("cancelled")),
                false
            )
        );
        assert_eq!(
            listing(&engine, &session, wallet),
            Ok(vec![(st_hash(1), nullifiers(1))])
        );
        refused(engine.block_on(session.unload_wallet(wallet)));
    }
}
