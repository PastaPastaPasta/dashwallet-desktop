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
//! ([`fork`]); each has an ignored stand-in naming the gap.

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
    refused(engine.block_on(async move {
        (s.platform
            .remove_from_manager(&s.leases, &m, wallet, StoredSpends::Refuse))
        .await
    }));
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
    refused(engine.block_on(async move {
        (s.platform
            .remove_from_manager(&s.leases, &m, wallet, StoredSpends::Refuse))
        .await
    }));
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
type ForkRow = (u8, &'static str, Option<&'static str>, Vec<[u8; 32]>, i64);

/// Writes `rows` of `wallet` into the fork's table in the shielded store,
/// created with the fork's schema.
fn fork_dispositions(path: &Path, wallet: WalletId, rows: &[ForkRow]) {
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
fn fork_rows(path: &Path) -> Vec<(Vec<u8>, Vec<u8>, i64)> {
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
    refused(engine.block_on(async move {
        (s.platform
            .remove_from_manager(&s.leases, &m, wallet, StoredSpends::Refuse))
        .await
    }));

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

#[test]
#[ignore = "GAP G2: upstream keeps no executed-spend hold until the host holds the spent state; journal holds (DRAIN D1) not ported"]
fn executed_ack_lifts_the_close_refusal() {
    panic!("GAP G2: no upstream disposition to acknowledge");
}

#[test]
#[ignore = "GAP G2: upstream has no disposition and no acknowledgement to replay after a crash"]
fn disposition_replayed_after_crash_before_ack() {
    panic!("GAP G2: no upstream disposition to acknowledge");
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

#[test]
#[ignore = "GAP G2: upstream keeps no disposition rows to read or acknowledge"]
fn unreadable_disposition_row_is_acked_by_key_and_listing_resumes() {
    panic!("GAP G2: no upstream disposition rows");
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
