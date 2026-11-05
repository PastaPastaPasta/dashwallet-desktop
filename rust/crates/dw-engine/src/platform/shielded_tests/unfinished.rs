//! Core → Shielded locks as unfinished transfers ([`ShieldLocks`]): listed
//! and finished through the shield flow's own apply, on the same outpoint,
//! with nothing funded.

use super::*;
use crate::platform::unfinished_locks::{
    self as locks, LockFinish, LockKindId, LockOrigin, UnfinishedLock, UnfinishedState,
};
use crate::platform::unfinished_shields::ShieldLocks;

/// A lease of `grant` for the shield flow, noting in `begun` that it was
/// asked for.
fn lease_of(
    s: &Arc<NetworkSession>,
    wallet: crate::WalletId,
    grant: String,
    begun: Arc<AtomicBool>,
) -> impl FnOnce() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Lease, PlatformError>> + Send>,
> {
    let s = Arc::clone(s);
    move || {
        Box::pin(async move {
            begun.store(true, Ordering::SeqCst);
            Ok(s.begin_lease(wallet, FlowKind::Shield, &[grant]).await?)
        })
    }
}

/// Finishes the lock at `outpoint` under a lease of a new grant, and
/// whether that lease was asked for.
fn finish(
    fx: &Fixture,
    lib: &Arc<FakeShielded>,
    outpoint: OutPoint,
) -> (Result<LockFinish, PlatformError>, bool) {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    let begun = Arc::new(AtomicBool::new(false));
    let asked = Arc::clone(&begun);
    let finished = run(fx, lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        locks::finish(&kind, outpoint, lease_of(&s, wallet, grant, asked)).await
    });
    (finished, begun.load(Ordering::SeqCst))
}

fn listed(fx: &Fixture, lib: &Arc<FakeShielded>) -> Result<Vec<UnfinishedLock>, PlatformError> {
    let wallet = fx.wallet;
    run(fx, lib, move |s, lib| async move {
        locks::detect(&ShieldLocks::new(&s, wallet, &*lib), &HashSet::new()).await
    })
}

fn state_of(fx: &Fixture, lib: &Arc<FakeShielded>) -> Vec<(LockOrigin, UnfinishedState)> {
    (listed(fx, lib).unwrap().into_iter())
        .map(|l| (l.origin, l.state))
        .collect()
}

fn answer(lib: &FakeShielded, answer: Result<Applied, PlatformError>) {
    *lib.applies.lock().unwrap() = Some(answer);
}

/// A lock no shield step here signed (a restore's, read back from the
/// chain) is history to 0.1's pending shields, and an unfinished transfer
/// here: finishing it applies the same outpoint, funds nothing, and only
/// the proved apply settles it, once.
#[test]
fn a_restored_shield_lock_is_listed_and_finished_on_its_outpoint() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    assert!(pending_of(&fx, &lib).is_empty(), "0.1 shows it as history");
    assert_eq!(
        listed(&fx, &lib).unwrap(),
        [UnfinishedLock {
            txid: restored.outpoint.txid.to_string(),
            vout: 0,
            kind: LockKindId::Shielded,
            duffs: restored.amount,
            origin: LockOrigin::Restored,
            state: UnfinishedState::Ready,
        }]
    );

    answer(&lib, Ok(Applied::Done));
    let before = funding_counts(&lib);
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(finished.unwrap(), LockFinish::Applied);
    assert!(begun);
    assert_eq!(*lib.applied.lock().unwrap(), [restored.outpoint]);
    assert_eq!(*lib.settles.lock().unwrap(), [restored.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert!(listed(&fx, &lib).unwrap().is_empty());

    let (again, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(again.unwrap(), LockFinish::Applied);
    assert!(!begun, "a finished lock asks for no lease");
    assert_eq!(lib.applied.lock().unwrap().len(), 1, "applied once");
}

/// Platform's unproved "already consumed" and a failure after the lock's
/// key signed both leave the lock unfinished, "Checking", with no settle
/// (SR1).
#[test]
fn an_unproved_answer_leaves_the_lock_checking() {
    let fx = Fixture::new(0);
    let own = lock(1, LockStatus::InstantSendLocked);
    let restored = lock(2, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone(), restored.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });

    lib.consumed.store(true, Ordering::SeqCst);
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, restored.outpoint).0.unwrap(),
        LockFinish::Checking
    );

    assert!(lib.settles.lock().unwrap().is_empty(), "no settle");
    assert_eq!(
        *lib.unknown.lock().unwrap(),
        HashSet::from([own.outpoint.txid, restored.outpoint.txid])
    );
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::ThisDevice, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Checking),
        ]
    );
}

/// A finish cut after its signature resumes on the same outpoint: no new
/// lock, no funding, and the later proved apply settles it.
#[test]
fn an_interrupted_finish_resumes_on_the_same_outpoint() {
    let fx = Fixture::new(0);
    let own = lock(1, LockStatus::ChainLocked);
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });
    let before = funding_counts(&lib);
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, own.outpoint).0.unwrap(),
        LockFinish::Applied
    );

    assert_eq!(*lib.applied.lock().unwrap(), [own.outpoint, own.outpoint]);
    assert_eq!(*lib.settles.lock().unwrap(), [own.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert!(listed(&fx, &lib).unwrap().is_empty());
}

/// Below the activation the lock lists "not active", and a finish refuses
/// before asking for a lease: the grant is kept and nothing is applied.
#[test]
fn an_inactive_network_keeps_the_grant() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        version: 11,
        refreshed: 11,
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::NotActive)]
    );
    let (finished, begun) = finish(&fx, &lib, restored.outpoint);
    assert_eq!(
        finished.unwrap(),
        LockFinish::NotApplied {
            error: "shielded.not_active".into()
        }
    );
    assert!(!begun, "no lease, the grant kept");
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// A lock with no proof yet waits: no lease, no apply.
#[test]
fn a_lock_without_a_proof_waits() {
    let fx = Fixture::new(0);
    let unproved = lock(1, LockStatus::Broadcast);
    let lib = Arc::new(FakeShielded {
        locks: vec![unproved.clone()],
        ..FakeShielded::default()
    });
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Confirming)]
    );
    let (finished, begun) = finish(&fx, &lib, unproved.outpoint);
    assert_eq!(finished.unwrap(), LockFinish::ProofWaiting);
    assert!(!begun);
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// A refusal before the lock's key signed reached nothing: not applied,
/// not marked, still ready.
#[test]
fn a_finish_refused_before_its_signature_is_not_applied() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Err(PlatformError::Unavailable));
    assert_eq!(
        finish(&fx, &lib, restored.outpoint).0.unwrap(),
        LockFinish::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    assert!(lib.unknown.lock().unwrap().is_empty());
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::Restored, UnfinishedState::Ready)]
    );
}

/// Two finishes of one lock at once apply it once: the second reads it
/// settled under the wallet's shield lock.
#[test]
fn two_finishes_of_one_lock_apply_once() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Ok(Applied::Done));
    let wallet = fx.wallet;
    let (g1, g2) = (shield_grant(&fx, 1), shield_grant(&fx, 1));
    let begun = Arc::new(AtomicBool::new(false));
    let outpoint = restored.outpoint;
    let (a, b) = run(&fx, &lib, move |s, lib| async move {
        let kind = ShieldLocks::new(&s, wallet, &*lib);
        tokio::join!(
            locks::finish(
                &kind,
                outpoint,
                lease_of(&s, wallet, g1, Arc::clone(&begun))
            ),
            locks::finish(&kind, outpoint, lease_of(&s, wallet, g2, begun)),
        )
    });
    assert_eq!(
        (a.unwrap(), b.unwrap()),
        (LockFinish::Applied, LockFinish::Applied)
    );
    assert_eq!(lib.applied.lock().unwrap().len(), 1);
    assert_eq!(lib.settles.lock().unwrap().len(), 1);
}

/// Nothing is told apart before the dispatch journal is read, and a lock
/// the wallet does not track is no transfer of it.
#[test]
fn an_unread_journal_or_an_unknown_lock_is_refused() {
    let fx = Fixture::new(0);
    let restored = lock(1, LockStatus::RecoveredFromChain);
    let unread = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        own: None,
        ..FakeShielded::default()
    });
    assert_eq!(
        listed(&fx, &unread).unwrap_err(),
        PlatformError::Unavailable
    );
    let (finished, begun) = finish(&fx, &unread, restored.outpoint);
    assert_eq!(finished.unwrap_err(), PlatformError::Unavailable);
    assert!(!begun);

    let lib = Arc::new(FakeShielded {
        locks: vec![restored.clone()],
        ..FakeShielded::default()
    });
    let other = OutPoint::new(restored.outpoint.txid, 1);
    let (finished, begun) = finish(&fx, &lib, other);
    assert!(
        matches!(finished, Err(PlatformError::InvalidArgument { .. })),
        "{finished:?}"
    );
    assert!(!begun);
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// The library's own rows: a restored shield lock the restore scan
/// rebuilt (`RecoveredFromChain`, ChainLock proof) lists as a restored
/// transfer, and its credit script, which the finish's signer is narrowed
/// to, is the lock payload's credit output of that outpoint and no other.
#[test]
fn a_restored_library_row_lists_with_its_credit_script() {
    let fx = Fixture::new(0);
    let lib = real_rows(&fx);
    let transaction = lock_transaction(5);
    let outpoint = OutPoint::new(transaction.txid(), 0);
    let row = TrackedAssetLock {
        out_point: outpoint,
        proof: Some(AssetLockProof::Chain(ChainAssetLockProof::new(
            100,
            outpoint.into(),
        ))),
        transaction,
        account_index: 0,
        funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
        identity_index: 0,
        amount: 1_000_000,
        status: AssetLockStatus::RecoveredFromChain,
    };
    fx.engine.block_on(lib.track(row, true));

    assert_eq!(
        listed(&fx, &lib).unwrap(),
        [UnfinishedLock {
            txid: outpoint.txid.to_string(),
            vout: 0,
            kind: LockKindId::Shielded,
            duffs: 1_000_000,
            origin: LockOrigin::Restored,
            state: UnfinishedState::Ready,
        }]
    );
    let scripts = fx.engine.block_on(async {
        let real = lib.real().await;
        (
            real.credit_script(outpoint).await,
            real.credit_script(OutPoint::new(outpoint.txid, 1)).await,
            real.credit_script(lock(5, LockStatus::Broadcast).outpoint)
                .await,
        )
    });
    assert_eq!(scripts, (Some(core_address(5).script_pubkey()), None, None));
}

/// Two credit outputs of one lock transaction are two locks: finishing one
/// settles nothing by txid (the library's row finishes it) and marks
/// nothing by txid, so the other stays listed and ready.
#[test]
fn finishing_one_output_of_a_lock_leaves_its_other_output_listed() {
    let fx = Fixture::new(0);
    let first = lock(1, LockStatus::RecoveredFromChain);
    let second = LockInfo {
        outpoint: OutPoint::new(first.outpoint.txid, 1),
        ..first.clone()
    };
    let lib = Arc::new(FakeShielded {
        locks: vec![first.clone(), second.clone()],
        ..FakeShielded::default()
    });
    answer(&lib, Ok(Applied::Done));
    assert_eq!(
        finish(&fx, &lib, first.outpoint).0.unwrap(),
        LockFinish::Applied
    );
    answer(&lib, Ok(Applied::Unknown));
    assert_eq!(
        finish(&fx, &lib, first.outpoint).0.unwrap(),
        LockFinish::Checking
    );
    assert!(lib.settles.lock().unwrap().is_empty(), "no settle by txid");
    assert!(lib.unknown.lock().unwrap().is_empty(), "no mark by txid");
    let second_listed = (listed(&fx, &lib).unwrap().into_iter())
        .find(|l| l.vout == 1)
        .map(|l| l.state);
    assert_eq!(second_listed, Some(UnfinishedState::Ready));
}

/// Repair's "Finish transfers" under its one lease: each unfinished lock
/// once, in order, an unproved answer leaving that lock pending while the
/// next runs; one with no proof is pending, not tried.
#[test]
fn finish_transfers_runs_each_lock_once_and_reports_the_kind() {
    let fx = Fixture::new(0);
    let (checking, applied, unproved) = (
        lock(1, LockStatus::RecoveredFromChain),
        lock(2, LockStatus::RecoveredFromChain),
        lock(3, LockStatus::Broadcast),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![unproved.clone(), applied.clone(), checking.clone()],
        ..FakeShielded::default()
    });
    // In txid order: the first apply hears "already consumed", the second
    // is proved.
    lib.consumed.store(true, Ordering::SeqCst);
    answer(&lib, Ok(Applied::Done));
    let before = funding_counts(&lib);
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let report = run(&fx, &lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    });
    assert_eq!(
        report,
        super::super::FinishReport {
            resumed: 2,
            completed: 1,
            still_pending: 2,
            already_used: 0,
            by_kind: vec![locks::TransfersReport {
                kind: LockKindId::Shielded,
                resumed: 2,
                completed: 1,
                still_pending: 2,
            }],
            unread: Vec::new(),
        }
    );
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [checking.outpoint, applied.outpoint]
    );
    assert_eq!(*lib.settles.lock().unwrap(), [applied.outpoint.txid]);
    assert_no_funding(&lib, before);
    assert_eq!(
        state_of(&fx, &lib),
        [
            (LockOrigin::Restored, UnfinishedState::Checking),
            (LockOrigin::Restored, UnfinishedState::Confirming),
        ]
    );
}

/// Nothing unfinished, nothing reported: no kind row.
#[test]
fn finish_transfers_with_nothing_unfinished_reports_no_kind() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded::default());
    let (wallet, grant) = (fx.wallet, shield_grant(&fx, 1));
    let report = run(&fx, &lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    });
    assert_eq!(report, super::super::FinishReport::default());
}

/// Finish transfers over `lib` under a Registration lease, as Repair runs
/// it.
fn finish_transfers(fx: &Fixture, lib: &Arc<FakeShielded>) -> super::super::FinishReport {
    let (wallet, grant) = (fx.wallet, shield_grant(fx, 1));
    run(fx, lib, move |s, lib| async move {
        let lease = (s
            .begin_lease(wallet, FlowKind::Registration, &[grant])
            .await)
            .unwrap();
        let mut report = super::super::FinishReport::default();
        locks::finish_kind(&ShieldLocks::new(&s, wallet, &*lib), &lease, &mut report).await;
        report
    })
}

fn totals(r: &super::super::FinishReport) -> (u32, u32, u32) {
    (r.resumed, r.completed, r.still_pending)
}

/// A lock refused before its signature (this device's here) stays pending,
/// unmarked, and the next lock still runs.
#[test]
fn finish_transfers_runs_the_next_lock_after_a_refusal() {
    let fx = Fixture::new(0);
    let (own, restored) = (
        lock(1, LockStatus::InstantSendLocked),
        lock(2, LockStatus::RecoveredFromChain),
    );
    let lib = Arc::new(FakeShielded {
        locks: vec![own.clone(), restored.clone()],
        own: Some(HashSet::from([own.outpoint.txid])),
        ..FakeShielded::default()
    });
    answer(&lib, Err(PlatformError::Unavailable));
    lib.queued.lock().unwrap().push_back(Ok(Applied::Done));
    let report = finish_transfers(&fx, &lib);
    assert_eq!(totals(&report), (1, 1, 1));
    assert_eq!(
        *lib.applied.lock().unwrap(),
        [own.outpoint, restored.outpoint]
    );
    assert!(
        lib.unknown.lock().unwrap().is_empty(),
        "a refusal marks nothing"
    );
    assert_eq!(*lib.settles.lock().unwrap(), [restored.outpoint.txid]);
    assert_eq!(
        state_of(&fx, &lib),
        [(LockOrigin::ThisDevice, UnfinishedState::Ready)]
    );
}

/// Below the activation every lock is pending, none tried.
#[test]
fn finish_transfers_tries_nothing_on_an_inactive_network() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        version: 11,
        refreshed: 11,
        locks: vec![lock(1, LockStatus::RecoveredFromChain)],
        ..FakeShielded::default()
    });
    let report = finish_transfers(&fx, &lib);
    assert_eq!(totals(&report), (0, 0, 1));
    assert!(lib.applied.lock().unwrap().is_empty());
}

/// Locks that cannot be read are not counted as "nothing pending": the
/// kind is reported unread.
#[test]
fn finish_transfers_reports_a_kind_it_could_not_read() {
    let fx = Fixture::new(0);
    let lib = Arc::new(FakeShielded {
        locks: vec![lock(1, LockStatus::RecoveredFromChain)],
        own: None,
        ..FakeShielded::default()
    });
    let report = finish_transfers(&fx, &lib);
    assert_eq!(
        report,
        super::super::FinishReport {
            unread: vec![LockKindId::Shielded],
            ..Default::default()
        }
    );
    assert!(lib.applied.lock().unwrap().is_empty());
}
