//! Mode B's call permits (E0-04 §2a.3, §2a.4 item 5).

use std::sync::Arc;

use tokio::time::Instant;

use super::table::LeaseState;
use super::tests::{H, W, begin, status, table};
use super::*;
use crate::platform::flows::{BudgetPurpose, RevokeCause};

fn credits(amount: u64) -> Option<CallQuote> {
    Some(CallQuote {
        purpose: BudgetPurpose::Credits,
        amount,
    })
}

fn flow(report: &LockReport, lease: &Lease) -> FlowReport {
    report
        .flows
        .iter()
        .find(|f| f.lease == lease.id_string())
        .cloned()
        .expect("the lease is in the report")
}

#[tokio::test(start_paused = true)]
async fn a_call_charges_its_quote_and_shows_c2_while_it_runs() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    let view = l.view().unwrap();
    assert!(view.call_running, "C2 while the call runs");
    assert_eq!(view.budgets[0].spent, 400);
    assert_eq!(
        l.call_permit("again", credits(601)).unwrap_err(),
        LeaseError::Exceeded {
            purpose: BudgetPurpose::Credits,
            needed: 601,
            remaining: 600,
        }
    );
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    let view = l.view().unwrap();
    assert!(!view.call_running);
    assert_eq!(view.budgets[0].spent, 400, "a sent call keeps its charge");
}

#[tokio::test(start_paused = true)]
async fn a_lock_before_the_call_cancels_it_and_records_nothing() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let report = t.lock(RevokeCause::Lock, status).await;
    assert_eq!(
        l.call_permit("DashPay.register_name", credits(400))
            .unwrap_err(),
        LeaseError::Revoked(RevokeCause::Lock)
    );
    let f = flow(&report, &l);
    assert_eq!(f.outcome, FlowOutcome::Cancelled);
    assert!(f.artifacts.is_empty(), "nothing recorded");
}

#[tokio::test(start_paused = true)]
async fn the_lock_waits_for_a_running_call_and_reports_it_sent() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    p.signatures().note_released();
    let start = Instant::now();
    let lock = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.lock(RevokeCause::Lock, status).await }
    });
    tokio::time::sleep(H / 2).await;
    assert!(!lock.is_finished(), "the drain waits for the call");
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    let report = lock.await.unwrap();
    assert!(start.elapsed() < H);
    let f = flow(&report, &l);
    assert_eq!(f.outcome, FlowOutcome::Sent);
    assert_eq!(
        f.artifacts,
        vec![("DashPay.register_name".to_owned(), FlowOutcome::Sent)]
    );
}

#[tokio::test(start_paused = true)]
async fn a_call_that_outruns_h_is_maybe_sent_and_the_lock_returns_at_its_deadline() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    let deadline = p.deadline();
    let report = t.lock(RevokeCause::Lock, status).await;
    assert!(Instant::now() >= deadline, "the drain waited until H");
    assert!(Instant::now() < deadline + H / 10);
    let f = flow(&report, &l);
    assert_eq!(f.outcome, FlowOutcome::MaybeSent, "still running");
    // It ends later, failed: a signer refused Locked after the lock, with
    // no signature released, so it is Cancelled.
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
}

#[tokio::test(start_paused = true)]
async fn a_locked_signer_before_the_first_signature_cancels_and_refunds() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    assert_eq!(l.view().unwrap().budgets[0].spent, 0, "refunded");
    assert_eq!(l.state(), Some(LeaseState::Parked), "the flow parks");
}

#[tokio::test(start_paused = true)]
async fn a_locked_signer_after_a_released_one_is_maybe_sent_and_parks() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    // Step 2's ChainLock-height retry: the first attempt was signed and
    // submitted, the re-sign met the lock.
    p.signatures().note_released();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(l.view().unwrap().budgets[0].spent, 400, "kept");
    assert_eq!(l.state(), Some(LeaseState::Parked));
}

#[tokio::test(start_paused = true)]
async fn a_failed_call_with_a_released_signature_is_maybe_sent() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(l.state(), Some(LeaseState::Active), "no Locked: not parked");
    let report = t.lock(RevokeCause::Lock, status).await;
    assert_eq!(flow(&report, &l).outcome, FlowOutcome::MaybeSent);
}

#[tokio::test(start_paused = true)]
async fn a_dropped_permit_counts_as_a_failed_call() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    drop(p);
    assert!(!l.view().unwrap().call_running);
    assert_eq!(
        l.view().unwrap().budgets[0].spent,
        0,
        "no signature: refunded"
    );
    let p = l
        .call_permit("DashPay.register_name", credits(400))
        .unwrap();
    p.signatures().note_released();
    drop(p);
    let report = t.lock(RevokeCause::Lock, status).await;
    assert_eq!(
        flow(&report, &l).artifacts,
        vec![
            ("DashPay.register_name".to_owned(), FlowOutcome::Cancelled),
            ("DashPay.register_name".to_owned(), FlowOutcome::MaybeSent),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn no_call_starts_during_a_drain() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let running = l.call_permit("first", credits(1)).unwrap();
    let lock = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.lock(RevokeCause::Lock, status).await }
    });
    tokio::task::yield_now().await;
    // The revoked lease refuses at once; nothing waits on the drain.
    assert!(matches!(
        l.call_permit("second", credits(1)).unwrap_err(),
        LeaseError::Locked | LeaseError::Revoked(RevokeCause::Lock)
    ));
    drop(running);
    lock.await.unwrap();
    let fresh = begin(&t, W, 0, 1000, 0).await.unwrap();
    assert!(fresh.call_permit("after", credits(1)).is_ok());
}

// --- Durable markers first (§2a.3, P2b) -----------------------------------

use std::sync::atomic::Ordering;
use std::time::Duration;

use dw_appdb::dispatch::{Resolution, StepEvent};
use platform_wallet::AssetLockFundingType;

use super::fence::{JournalBackend, Mark};
use super::tests::{FakeJournal, W2, with_journal};

const SUBMIT: &str = "withdrawal/w/submit";
const FUNDING: &str = "registration/d/funding";

fn funding_key() -> FundingKey {
    FundingKey {
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index: 3,
        account: 0,
    }
}

fn funding_step(step: &str) -> CallStep {
    CallStep::Funding {
        step: step.into(),
        key: funding_key(),
    }
}

fn resumable(step: &str) -> CallStep {
    CallStep::Resumable(step.into())
}

/// Everything the lease's budgets were charged.
fn spent(l: &Lease) -> u64 {
    l.view().unwrap().budgets.iter().map(|b| b.spent).sum()
}

/// A funding call's quote (§2a.3): `amount`, no worst-case fee.
fn fund(amount: u64) -> Option<CallQuote> {
    Some(
        FundingQuote {
            amount,
            fee_bound_worst: 0,
        }
        .into(),
    )
}

/// The table has read W's and W2's tracked rows: none (§2a.3's gate).
fn rows_read(t: &LeaseTable) {
    for w in [W, W2] {
        t.commit_snapshot(w, Default::default());
    }
}

fn step_status(t: &LeaseTable, wallet: crate::WalletId, step: &str) -> CallStepStatus {
    t.call_step(wallet, step).status
}

/// The step's synthetic artifact's events on disk, in order.
fn events(j: &FakeJournal, wallet: crate::WalletId, step: &str) -> Vec<StepEvent> {
    let id = ArtifactId::of_step(&wallet, step);
    j.log()
        .into_iter()
        .filter(|r| (r.wallet, r.artifact) == (wallet.0, id.0))
        .map(|r| r.event)
        .collect()
}

fn marked(step: &str) -> StepEvent {
    StepEvent::Marked(step.into())
}

const NOT_SENT: StepEvent = StepEvent::Resolved(Resolution::NotSent);
const SENT: StepEvent = StepEvent::Resolved(Resolution::Sent);

/// A kill now: the next process loads the file as it is (§6.5).
fn reload(j: &FakeJournal) -> (Arc<LeaseTable>, Arc<FakeJournal>) {
    let snap = j.snapshot();
    let (t, _) = table();
    let (rows, steps) = snap.load();
    t.load_journal_with(
        Some(snap.clone() as Arc<dyn JournalBackend>),
        rows,
        steps,
        snap.load_calls(),
    );
    rows_read(&t);
    (t, snap)
}

async fn until(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition never held");
}

/// A call that fails having released no signature: Cancelled unless an
/// earlier marker of its step exists, MaybeSent then (§2a.3).
async fn fail_unsigned(l: &Lease, step: CallStep) -> FlowOutcome {
    let quote = match step {
        CallStep::Funding { .. } => fund(10),
        CallStep::Resumable(_) => credits(10),
    };
    l.call_permit_step("call", quote, step)
        .await
        .unwrap()
        .finish(false)
}

/// §2a.3 "Durable markers first": the permit comes back only once the
/// marker's write returned. Under `DW_LEASE_MUTATION=CallBeforeMarker` the
/// permit returns at once and this fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_steps_marker_is_durable_before_its_permit_returns() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    j.stall_ms.store(300, Ordering::SeqCst);
    let pending = tokio::spawn({
        let l = l.clone();
        async move {
            l.call_permit_step("submit", credits(10), resumable(SUBMIT))
                .await
        }
    });
    let id = ArtifactId::of_step(&W, SUBMIT);
    until(|| t.inspect(|i| matches!(i.fence.mark(&W, SUBMIT, &id), Some(Mark::Committing { .. }))))
        .await;
    assert_eq!(
        step_status(&t, W, SUBMIT),
        CallStepStatus::Marked { durable: false }
    );
    assert_eq!(spent(&l), 10, "charged in the marker's J step");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !pending.is_finished(),
        "no permit while the marker's write runs"
    );
    assert!(!j.stands(&W, &id, SUBMIT));
    let permit = pending.await.unwrap().unwrap();
    assert!(j.stands(&W, &id, SUBMIT), "durable before the call starts");
    assert_eq!(
        step_status(&t, W, SUBMIT),
        CallStepStatus::Marked { durable: true }
    );
    j.stall_ms.store(0, Ordering::SeqCst);
    assert_eq!(permit.finish(true), FlowOutcome::Sent);
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::Sent);
    until(|| events(&j, W, SUBMIT).contains(&SENT)).await;
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT), SENT]);
}

/// A definite Cancelled appends `NotSent`, which supersedes the marker, so
/// the step's next call is a fresh one; a failed call that may have sent
/// leaves the marker standing, and every later failure of the step reads
/// MaybeSent.
#[tokio::test(start_paused = true)]
async fn cancelled_supersedes_the_marker_and_maybe_sent_keeps_it() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::None);
    assert_eq!(
        fail_unsigned(&l, resumable(SUBMIT)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(spent(&l), 0, "refunded");
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT), NOT_SENT]);
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::NotSent);
    // Not prior-marked: a fresh marker, and Cancelled again.
    assert_eq!(
        fail_unsigned(&l, resumable(SUBMIT)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(
        events(&j, W, SUBMIT),
        vec![marked(SUBMIT), NOT_SENT, marked(SUBMIT), NOT_SENT]
    );
    // A call that released a signature and failed: MaybeSent, the marker
    // stands, no resolution is written.
    let p = l
        .call_permit_step("submit", credits(10), resumable(SUBMIT))
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(spent(&l), 10, "kept");
    assert_eq!(
        step_status(&t, W, SUBMIT),
        CallStepStatus::Marked { durable: true }
    );
    // Its earlier marker makes the next unsigned failure MaybeSent too, and
    // the marker is not written again.
    assert_eq!(
        fail_unsigned(&l, resumable(SUBMIT)).await,
        FlowOutcome::MaybeSent
    );
    let id = ArtifactId::of_step(&W, SUBMIT);
    assert!(j.stands(&W, &id, SUBMIT));
    assert_eq!(events(&j, W, SUBMIT).len(), 5);
}

/// §2a.4 item 6 at table level: the kill matrix of a funding call step.
/// At each point the next process loads the file as it is then.
#[tokio::test(start_paused = true)]
async fn the_kill_matrix_of_a_funding_call_step() {
    let (t, _) = table();
    let j = with_journal(&t);
    rows_read(&t);
    let l = begin(&t, W, 1000, 1000, 0).await.unwrap();

    // Killed before the marker write: nothing on disk, the step never
    // started, and the next process's call of it is a fresh one.
    let (t2, _) = reload(&j);
    assert_eq!(step_status(&t2, W, FUNDING), CallStepStatus::None);
    let l2 = begin(&t2, W, 1000, 1000, 0).await.unwrap();
    assert_eq!(
        fail_unsigned(&l2, funding_step(FUNDING)).await,
        FlowOutcome::Cancelled
    );

    // Killed after the marker, while the call runs: the marker and its
    // funding key stand; a later call of the step is prior-marked, so a
    // failure that released nothing reads MaybeSent, not Cancelled.
    let p = l
        .call_permit_step("funding", fund(10), funding_step(FUNDING))
        .await
        .unwrap();
    let kills_running = reload(&j);
    // Killed once the call returned, before its resolution's write.
    p.signatures().note_released();
    let kills_returned = reload(&j);
    for (t2, _) in [kills_running, kills_returned] {
        let info = t2.call_step(W, FUNDING);
        assert_eq!(info.status, CallStepStatus::Marked { durable: true });
        assert_eq!(info.funding, Some(funding_key()));
        let l2 = begin(&t2, W, 1000, 1000, 0).await.unwrap();
        // A standing funding marker refuses a second funding call of its
        // step (§2a.5, commit 3's gate): no second lock is built.
        assert_eq!(
            l2.call_permit_step("funding", fund(10), funding_step(FUNDING))
                .await
                .unwrap_err(),
            LeaseError::FundingBlocked(FundingBlock::Step(Some(
                crate::platform::flows::DispatchState::MaybeSent
            )))
        );
        assert_eq!(spent(&l2), 0, "refused before the charge");
        assert_eq!(
            t2.call_step(W, FUNDING).status,
            CallStepStatus::Marked { durable: true }
        );
    }

    // After the resolution: Sent, through the DEC-163 bundle with the
    // funding marker in it.
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    until(|| events(&j, W, FUNDING).contains(&SENT)).await;
    let id = ArtifactId::of_step(&W, FUNDING);
    let bundles = j.bundles.lock().unwrap().clone();
    assert_eq!(
        bundles,
        vec![(
            id.0,
            vec![FUNDING.to_owned()],
            vec![(FUNDING.to_owned(), funding_key().stored())]
        )]
    );
    let (t2, _) = reload(&j);
    let info = t2.call_step(W, FUNDING);
    assert_eq!(info.status, CallStepStatus::Sent);
    assert_eq!(info.funding, Some(funding_key()));

    // After a definite Cancelled's resolution: superseded, so the next
    // process reads no marker, and its call is a fresh one.
    const TOPUP: &str = "topup/i/funding";
    let p = l
        .call_permit_step("topup", fund(10), funding_step(TOPUP))
        .await
        .unwrap();
    let (t2, _) = reload(&j);
    assert_eq!(
        t2.call_step(W, TOPUP).status,
        CallStepStatus::Marked { durable: true }
    );
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    let (t2, j2) = reload(&j);
    assert_eq!(t2.call_step(W, TOPUP).status, CallStepStatus::None);
    assert_eq!(t2.call_step(W, TOPUP).funding, None);
    let l2 = begin(&t2, W, 1000, 1000, 0).await.unwrap();
    assert_eq!(
        fail_unsigned(&l2, funding_step(TOPUP)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(
        events(&j2, W, TOPUP),
        vec![marked(TOPUP), NOT_SENT, marked(TOPUP), NOT_SENT]
    );
    // Each marker row had its funding row; append-only.
    let rows = j2.funding_rows();
    assert_eq!(rows.iter().filter(|f| f.step_id == TOPUP).count(), 2);
}

/// §6.4 for a call step: a marker write that fails refuses the call, which
/// never starts and is refunded. The marker may have landed, so the step
/// reads MaybeSent until its `NotSent` row lands; the step's next call
/// retries that first and is then a fresh one.
#[tokio::test(start_paused = true)]
async fn a_failed_marker_write_refuses_the_call_and_reads_maybe_sent_until_not_sent_lands() {
    let (t, _) = table();
    let j = with_journal(&t);
    rows_read(&t);
    let l = begin(&t, W, 1000, 1000, 0).await.unwrap();
    j.fail_step.store(true, Ordering::SeqCst);
    j.fail_resolve.store(true, Ordering::SeqCst);
    assert_eq!(
        l.call_permit_step("funding", fund(10), funding_step(FUNDING))
            .await
            .unwrap_err(),
        LeaseError::JournalUnavailable
    );
    assert_eq!(spent(&l), 0, "refunded");
    assert!(!l.view().unwrap().call_running);
    assert_eq!(
        step_status(&t, W, FUNDING),
        CallStepStatus::Marked { durable: false }
    );
    assert!(events(&j, W, FUNDING).is_empty());
    assert_eq!(
        crate::platform::errors::PlatformError::from(LeaseError::JournalUnavailable).code(),
        "storage"
    );
    // The journal recovers: the next call lands the `NotSent` row first,
    // then writes its own marker, and is not prior-marked.
    j.fail_step.store(false, Ordering::SeqCst);
    j.fail_resolve.store(false, Ordering::SeqCst);
    assert_eq!(
        fail_unsigned(&l, funding_step(FUNDING)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(
        events(&j, W, FUNDING),
        vec![NOT_SENT, marked(FUNDING), NOT_SENT]
    );
    assert_eq!(step_status(&t, W, FUNDING), CallStepStatus::NotSent);

    // Only the marker write fails: its `NotSent` row lands at once.
    j.fail_step.store(true, Ordering::SeqCst);
    assert_eq!(
        l.call_permit_step("submit", credits(10), resumable(SUBMIT))
            .await
            .unwrap_err(),
        LeaseError::JournalUnavailable
    );
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::NotSent);
    assert_eq!(events(&j, W, SUBMIT), vec![NOT_SENT]);
}

/// A journal that is not ready refuses a call step before charging it:
/// nothing is marked or recorded.
#[tokio::test(start_paused = true)]
async fn a_call_step_needs_a_ready_journal() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    assert_eq!(
        l.call_permit_step("submit", credits(10), resumable(SUBMIT))
            .await
            .unwrap_err(),
        LeaseError::JournalUnavailable
    );
    t.load_journal(None, Vec::new(), Vec::new());
    assert_eq!(
        l.call_permit_step("submit", credits(10), resumable(SUBMIT))
            .await
            .unwrap_err(),
        LeaseError::JournalUnavailable
    );
    assert_eq!(spent(&l), 0);
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::None);
    // A plain call permit needs no journal.
    assert!(l.call_permit("query", credits(1)).is_ok());
}

/// Two calls of one step: the second is prior-marked, and an unsigned
/// failure of the first then supersedes nothing (the second may have
/// sent).
#[tokio::test(start_paused = true)]
async fn a_concurrent_call_of_the_step_keeps_its_marker() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let a = l
        .call_permit_step("a", credits(10), resumable(SUBMIT))
        .await
        .unwrap();
    let b = l
        .call_permit_step("b", credits(10), resumable(SUBMIT))
        .await
        .unwrap();
    b.signatures().note_released();
    assert_eq!(b.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(a.finish(false), FlowOutcome::Cancelled, "its own outcome");
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT)], "no NotSent");
    assert_eq!(
        step_status(&t, W, SUBMIT),
        CallStepStatus::Marked { durable: true }
    );
    // Another wallet's identical step id is another artifact.
    let other = begin(&t, W2, 0, 1000, 0).await.unwrap();
    assert_eq!(
        fail_unsigned(&other, resumable(SUBMIT)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(step_status(&t, W2, SUBMIT), CallStepStatus::NotSent);
}

/// A permit dropped while its marker is written never started: refunded,
/// and its `NotSent` row follows the marker's write, not before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_step_dropped_during_its_marker_write_resolves_after_it() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    j.stall_ms.store(200, Ordering::SeqCst);
    let pending = tokio::spawn({
        let l = l.clone();
        async move {
            l.call_permit_step("submit", credits(10), resumable(SUBMIT))
                .await
        }
    });
    let id = ArtifactId::of_step(&W, SUBMIT);
    until(|| t.inspect(|i| matches!(i.fence.mark(&W, SUBMIT, &id), Some(Mark::Committing { .. }))))
        .await;
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    assert_eq!(spent(&l), 0, "never started: refunded");
    until(|| step_status(&t, W, SUBMIT) == CallStepStatus::NotSent).await;
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT), NOT_SENT]);
    // A lock finds nothing in flight.
    t.lock(RevokeCause::Lock, status).await;
}

/// A call that joins the step while a dropped call's marker write runs
/// keeps the marker: the dropped call's held-back `NotSent` row no longer
/// applies once another call runs under it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_joining_a_dropped_calls_marker_write_keeps_the_marker() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    j.stall_ms.store(300, Ordering::SeqCst);
    let call = || {
        let l = l.clone();
        tokio::spawn(async move {
            l.call_permit_step("submit", credits(10), resumable(SUBMIT))
                .await
        })
    };
    let dropped = call();
    let id = ArtifactId::of_step(&W, SUBMIT);
    until(|| t.inspect(|i| matches!(i.fence.mark(&W, SUBMIT, &id), Some(Mark::Committing { .. }))))
        .await;
    dropped.abort();
    assert!(dropped.await.unwrap_err().is_cancelled());
    let joined = call().await.unwrap().unwrap();
    // Both writes returned; nothing superseded the marker.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT)]);
    assert!(j.stands(&W, &id, SUBMIT));
    j.stall_ms.store(0, Ordering::SeqCst);
    // It joined a marker begun before it: prior-marked.
    assert_eq!(joined.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT)]);
}

/// A lock that lands while the marker is written refuses the call with the
/// lease's own refusal: it never started, so it is refunded and its
/// `NotSent` row supersedes the marker, and the lock reports it Cancelled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_during_the_marker_write_refuses_the_call() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    j.stall_ms.store(300, Ordering::SeqCst);
    let pending = tokio::spawn({
        let l = l.clone();
        async move {
            l.call_permit_step("submit", credits(10), resumable(SUBMIT))
                .await
        }
    });
    let id = ArtifactId::of_step(&W, SUBMIT);
    until(|| t.inspect(|i| matches!(i.fence.mark(&W, SUBMIT, &id), Some(Mark::Committing { .. }))))
        .await;
    let lock = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.lock(RevokeCause::Lock, status).await }
    });
    assert_eq!(pending.await.unwrap().unwrap_err(), LeaseError::Locked);
    let report = lock.await.unwrap();
    assert_eq!(flow(&report, &l).outcome, FlowOutcome::Cancelled);
    assert_eq!(events(&j, W, SUBMIT), vec![marked(SUBMIT), NOT_SENT]);
    assert_eq!(step_status(&t, W, SUBMIT), CallStepStatus::NotSent);
}

/// The journal keeps the funding key first recorded under a marker: while
/// one stands, a call of the step with another key, or with none, is
/// refused before it is charged. Once superseded, the step takes any key.
#[tokio::test(start_paused = true)]
async fn a_call_step_under_another_funding_key_is_refused() {
    let (t, _) = table();
    let j = with_journal(&t);
    rows_read(&t);
    let l = begin(&t, W, 1000, 1000, 0).await.unwrap();
    let other = CallStep::Funding {
        step: FUNDING.into(),
        key: FundingKey {
            identity_index: 4,
            ..funding_key()
        },
    };
    // Superseded: the key goes with the marker, and another key is a
    // fresh call.
    assert_eq!(
        fail_unsigned(&l, funding_step(FUNDING)).await,
        FlowOutcome::Cancelled
    );
    assert_eq!(t.call_step(W, FUNDING).funding, None);
    let p = l
        .call_permit_step("funding", fund(10), other.clone())
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    let before = events(&j, W, FUNDING);
    // The marker stands under `other`'s key.
    for step in [funding_step(FUNDING), resumable(FUNDING)] {
        assert_eq!(
            l.call_permit_step("funding", fund(10), step)
                .await
                .unwrap_err(),
            LeaseError::StepKeyMismatch
        );
    }
    assert_eq!(spent(&l), 10, "only the MaybeSent call's charge");
    assert!(!l.view().unwrap().call_running);
    assert_eq!(events(&j, W, FUNDING), before);
    assert_eq!(t.call_step(W, FUNDING).funding, other.funding());
    assert_eq!(
        crate::platform::errors::PlatformError::from(LeaseError::StepKeyMismatch).code(),
        "internal"
    );
    // Under its own key, the step's next funding call is refused too: its
    // funding may have been sent (commit 3's gate).
    assert_eq!(
        l.call_permit_step("funding", fund(10), other)
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(
            crate::platform::flows::DispatchState::MaybeSent
        )))
    );
}

/// Repair's self-spends stand with the funding marker they answer and
/// load with it (§6.5).
#[tokio::test(start_paused = true)]
async fn self_spends_load_with_their_funding_marker() {
    let (t, _) = table();
    let j = with_journal(&t);
    rows_read(&t);
    let l = begin(&t, W, 1000, 1000, 0).await.unwrap();
    let id = ArtifactId::of_step(&W, FUNDING);
    assert!(!j.insert_self_spend(&W.0, FUNDING, &id.0, &[7; 32]).unwrap());
    let p = l
        .call_permit_step("funding", fund(10), funding_step(FUNDING))
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert!(j.insert_self_spend(&W.0, FUNDING, &id.0, &[7; 32]).unwrap());
    let (t2, _) = reload(&j);
    let info = t2.call_step(W, FUNDING);
    assert_eq!(info.status, CallStepStatus::Marked { durable: true });
    assert_eq!(info.funding, Some(funding_key()));
    assert_eq!(info.self_spends, vec![[7; 32]]);
}

/// `funding_marker.funding_type`'s codes are stable and cover every type;
/// a key is what a tracked row carries.
#[test]
fn funding_types_map_to_stable_codes() {
    use AssetLockFundingType as T;
    let pinned = [
        (T::IdentityRegistration, 0),
        (T::IdentityTopUp, 1),
        (T::IdentityTopUpNotBound, 2),
        (T::IdentityInvitation, 3),
        (T::AssetLockAddressTopUp, 4),
        (T::AssetLockShieldedAddressTopUp, 5),
    ];
    for (t, code) in pinned {
        assert_eq!(FundingKey::type_code(t), code, "{t:?}");
        assert_eq!(FundingKey::type_of_code(code), Some(t));
        let key = FundingKey {
            funding_type: t,
            identity_index: 9,
            account: 2,
        };
        assert_eq!(FundingKey::from_stored(key.stored()), Some(key));
    }
    assert_eq!(FundingKey::type_of_code(6), None, "unknown");
    let lock = platform_wallet::TrackedAssetLock {
        out_point: dashcore::OutPoint::null(),
        transaction: dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input: Vec::new(),
            output: Vec::new(),
            special_transaction_payload: None,
        },
        account_index: 2,
        funding_type: T::IdentityTopUp,
        identity_index: 9,
        amount: 1,
        status: platform_wallet::AssetLockStatus::Built,
        proof: None,
    };
    assert_eq!(
        FundingKey::of_tracked(&lock),
        FundingKey {
            funding_type: T::IdentityTopUp,
            identity_index: 9,
            account: 2,
        }
    );
}
