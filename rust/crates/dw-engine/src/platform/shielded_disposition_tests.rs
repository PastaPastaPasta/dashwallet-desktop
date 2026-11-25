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

#[test]
#[ignore = "GAP G6: upstream reports no Unattributable outcome; a confirmed row settles executed (reported_outcomes_settle_the_journal)"]
fn unattributable_with_confirmed_activity_reads_done_and_offers_no_retry() {
    panic!("GAP G6: no upstream Unattributable outcome");
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
