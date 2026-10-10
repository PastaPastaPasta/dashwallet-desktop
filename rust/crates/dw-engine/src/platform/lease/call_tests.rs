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
