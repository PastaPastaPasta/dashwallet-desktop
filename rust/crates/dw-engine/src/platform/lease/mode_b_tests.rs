//! Mode B's derived funding statuses and its funding gate (E0-04 §2a.5,
//! §2a.3, §2a.4 item 7, §16.5–§16.7), against a fake evidence source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dashcore::OutPoint;
use platform_wallet::AssetLockFundingType;

use super::fence::JournalBackend;
use super::mode_b::{Snapshot, TrackedLocks, derive_funding, index_trusted};
use super::tests::{
    DURABLE, FakeJournal, REG, Recorder, W, W2, begin, key, quote, row, sign_lock, table, txid,
    until, with_journal,
};
use super::*;
use crate::WalletId;
use crate::platform::PlatformError;
use crate::platform::flows::{BudgetPurpose, DispatchResolution, DispatchState};

use CallStepStatus as S;
use DispatchState::{MaybeSent, NotSent, Sent, WillBeSent};
use TrackedStatus as R;

const TOPUP: &str = "topup/i/funding";

fn funding(step: &str, identity_index: u32) -> CallStep {
    CallStep::Funding {
        step: step.into(),
        key: key(identity_index),
    }
}

/// The fake library and `app.sqlite`: what `refresh_funding` reads.
#[derive(Default)]
struct FakeTracked {
    rows: Mutex<HashMap<WalletId, Vec<TrackedRow>>>,
    drafts: Mutex<HashMap<WalletId, Vec<DraftKey>>>,
    locked: Mutex<HashSet<[u8; 32]>>,
    /// [`TrackedLocks::recovery_complete`]: DP1-05's scan finished.
    complete: AtomicBool,
}

impl FakeTracked {
    fn set_rows(&self, wallet: WalletId, rows: Vec<TrackedRow>) {
        self.rows.lock().unwrap().insert(wallet, rows);
    }

    fn set_drafts(&self, wallet: WalletId, drafts: Vec<DraftKey>) {
        self.drafts.lock().unwrap().insert(wallet, drafts);
    }
}

#[async_trait]
impl TrackedLocks for FakeTracked {
    async fn rows(&self, wallet: WalletId) -> Option<Vec<TrackedRow>> {
        Some(
            self.rows
                .lock()
                .unwrap()
                .get(&wallet)
                .cloned()
                .unwrap_or_default(),
        )
    }

    async fn drafts(&self, wallet: WalletId) -> Option<Vec<DraftKey>> {
        Some(
            self.drafts
                .lock()
                .unwrap()
                .get(&wallet)
                .cloned()
                .unwrap_or_default(),
        )
    }

    async fn chain_locked(&self, _: WalletId, txid: &[u8; 32]) -> bool {
        self.locked.lock().unwrap().contains(txid)
    }

    async fn recovery_complete(&self, _: WalletId) -> bool {
        self.complete.load(Ordering::SeqCst)
    }
}

/// A source whose row read waits for `release`, to hold a refresh in
/// flight; `entered` is notified once the read began.
struct Gated {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    rows: Vec<TrackedRow>,
}

impl Gated {
    fn new(rows: Vec<TrackedRow>) -> Arc<Self> {
        Arc::new(Self {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
            rows,
        })
    }
}

#[async_trait]
impl TrackedLocks for Gated {
    async fn rows(&self, _: WalletId) -> Option<Vec<TrackedRow>> {
        self.entered.notify_one();
        self.release.notified().await;
        Some(self.rows.clone())
    }

    async fn drafts(&self, _: WalletId) -> Option<Vec<DraftKey>> {
        Some(Vec::new())
    }

    async fn chain_locked(&self, _: WalletId, _: &[u8; 32]) -> bool {
        false
    }
}

/// A table with a journal and the fake source, its wallets' rows read.
async fn setup() -> (
    Arc<LeaseTable>,
    Arc<Recorder>,
    Arc<FakeJournal>,
    Arc<FakeTracked>,
) {
    let (t, rec) = table();
    let j = with_journal(&t);
    let src = Arc::new(FakeTracked::default());
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    assert!(t.refresh_funding(W2).await);
    (t, rec, j, src)
}

/// DP1-05's scan of W finished ([`TrackedLocks::recovery_complete`]):
/// Repair may resolve a step with no known lock (review R1-01).
async fn recovery_complete(t: &Arc<LeaseTable>, src: &FakeTracked) {
    src.complete.store(true, Ordering::SeqCst);
    assert!(t.refresh_funding(W).await);
}

/// A kill now: the next process loads the file as it is (§6.5), with the
/// same library state, and reads the wallet's rows.
async fn reload(j: &FakeJournal, src: &Arc<FakeTracked>) -> (Arc<LeaseTable>, Arc<Recorder>) {
    let snap = j.snapshot();
    let (t, rec) = table();
    snap.load_into(&t);
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    (t, rec)
}

fn funds_committed(l: &Lease) -> bool {
    l.view().unwrap().funds_committed
}

fn funding_spent(l: &Lease) -> u64 {
    l.view()
        .unwrap()
        .budgets
        .iter()
        .find(|b| b.purpose == BudgetPurpose::Funding)
        .map_or(0, |b| b.spent)
}

/// A funding call of `step` that released a signature and failed: the
/// marker stands, MaybeSent.
async fn maybe_sent(l: &Lease, step: &str, index: u32) {
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(step, index))
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
}

// --- §2a.5's table, row by row (the pure derivation) ----------------------

#[test]
fn row_1_a_chain_locked_self_spend_reads_not_sent() {
    let marked = DURABLE;
    assert_eq!(
        derive_funding(marked, Some(&[]), true, false),
        Some(NotSent)
    );
    // Repair spends the inputs of every Built and Broadcast row too.
    assert_eq!(
        derive_funding(marked, Some(&[R::Built]), true, false),
        Some(NotSent)
    );
    assert_eq!(
        derive_funding(marked, Some(&[R::Broadcast]), true, false),
        Some(NotSent)
    );
    // A lock that reached a proof was not conflicted by it.
    assert_eq!(
        derive_funding(marked, Some(&[R::ChainLocked]), true, false),
        Some(Sent)
    );
    // An unattributed row may be the step's lock (review H-1).
    for rows in [&[][..], &[R::Built], &[R::Broadcast]] {
        assert_eq!(
            derive_funding(marked, Some(rows), true, true),
            Some(MaybeSent),
            "{rows:?}"
        );
    }
    assert_eq!(
        derive_funding(marked, Some(&[R::ChainLocked]), true, true),
        Some(Sent)
    );
}

#[test]
fn row_2_a_row_with_a_proof_reads_sent_with_or_without_a_marker() {
    for status in [
        R::InstantSendLocked,
        R::ChainLocked,
        R::Consumed,
        R::RecoveredFromChain,
    ] {
        for step in [S::None, DURABLE, S::NotSent] {
            assert_eq!(
                derive_funding(step, Some(&[status]), false, false),
                Some(Sent),
                "{status:?} {step:?}"
            );
        }
        // A proof beats a pending row of the same key.
        assert_eq!(
            derive_funding(S::None, Some(&[R::Built, status]), false, false),
            Some(Sent)
        );
    }
}

#[test]
fn row_3_a_built_or_broadcast_row_reads_will_be_sent_with_or_without_a_marker() {
    for status in [R::Built, R::Broadcast] {
        for step in [S::None, S::Marked { durable: false }, DURABLE, S::NotSent] {
            assert_eq!(
                derive_funding(step, Some(&[status]), false, false),
                Some(WillBeSent),
                "{status:?} {step:?}"
            );
        }
    }
}

#[test]
fn row_4_no_row_and_a_definite_resolution_reads_not_sent() {
    assert_eq!(
        derive_funding(S::NotSent, Some(&[]), false, false),
        Some(NotSent)
    );
}

#[test]
fn row_5_no_marker_and_no_row_reads_none() {
    assert_eq!(derive_funding(S::None, Some(&[]), false, false), None);
}

#[test]
fn row_6_a_marker_with_no_row_and_no_resolution_reads_maybe_sent() {
    for durable in [false, true] {
        assert_eq!(
            derive_funding(S::Marked { durable }, Some(&[]), false, false),
            Some(MaybeSent)
        );
    }
}

#[test]
fn a_call_that_returned_ok_reads_sent_and_unknown_rows_never_read_none_or_not_sent() {
    assert_eq!(derive_funding(S::Sent, Some(&[]), false, false), Some(Sent));
    assert_eq!(derive_funding(S::Sent, None, false, false), Some(Sent));
    for step in [S::None, S::NotSent, DURABLE] {
        assert_eq!(
            derive_funding(step, None, false, false),
            Some(MaybeSent),
            "{step:?}"
        );
    }
}

// --- The derivation against the table ------------------------------------

/// Each row of the table through `funding_status`, keyed by the marker's
/// funding key and by the draft's own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn funding_status_derives_each_row_from_the_rows_the_marker_and_the_drafts() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    // No marker, no row, no draft: never registered.
    assert_eq!(t.funding_status(W, REG), None);
    // A marker, no row, no resolution.
    maybe_sent(&l, REG, 7).await;
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    // A row under the marker's key: Built, then IS-locked.
    src.set_rows(W, vec![row(1, 7, R::Built)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(WillBeSent));
    src.set_rows(W, vec![row(1, 7, R::InstantSendLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(Sent));
    // Another wallet's rows say nothing of W's steps.
    assert_eq!(t.funding_status(W2, REG), None);
    // The draft's own key finds a row with no marker (a lost journal), at
    // account 0 only (DP1-02's).
    const LOST: &str = "registration/9/funding";
    src.set_drafts(W, vec![DraftKey::registration(9, 4)]);
    src.set_rows(W, vec![row(2, 4, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), Some(WillBeSent));
    src.set_rows(
        W,
        vec![TrackedRow {
            key: FundingKey {
                account: 3,
                ..key(4)
            },
            ..row(2, 4, R::Broadcast)
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), None);
    // A row of another funding type does not match the draft.
    src.set_rows(
        W,
        vec![TrackedRow {
            key: FundingKey {
                funding_type: AssetLockFundingType::IdentityTopUp,
                ..key(4)
            },
            ..row(2, 4, R::Broadcast)
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), None);
}

/// §2a.5: a row that matches several drafts counts for each (fail closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_matching_several_drafts_counts_for_each() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_drafts(
        W,
        vec![DraftKey::registration(1, 5), DraftKey::registration(2, 5)],
    );
    src.set_rows(W, vec![row(1, 5, R::Built)]);
    t.refresh_funding(W).await;
    for step in ["registration/1/funding", "registration/2/funding"] {
        assert_eq!(t.funding_status(W, step), Some(WillBeSent), "{step}");
    }
    // And a marker's key and another draft's key at once (the catch-up
    // resumed the Built row, so it no longer blocks funding). Row 1
    // preceded the marker, so it is not the top-up's; row 2 came after.
    t.note_resumed(W, txid(1));
    maybe_sent(&l, TOPUP, 5).await;
    src.set_rows(W, vec![row(1, 5, R::Built), row(2, 5, R::Consumed)]);
    t.refresh_funding(W).await;
    for step in ["registration/1/funding", "registration/2/funding", TOPUP] {
        assert_eq!(t.funding_status(W, step), Some(Sent), "{step}");
    }
}

/// §2a.5 `DispatchResolved`: a move to `Sent` (a row reaching a proof,
/// DP1-05 adopting a `RecoveredFromChain` lock, a call returning Ok) or to
/// `NotSent` (a definite resolution, Repair's ChainLocked self-spend), from
/// an earlier derivation; never on a move to `WillBeSent` or `MaybeSent`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_resolved_fires_on_moves_to_sent_and_not_sent_only() {
    let (t, rec, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();

    // A row transition: MaybeSent → WillBeSent (silent) → Sent.
    maybe_sent(&l, REG, 7).await;
    src.set_rows(W, vec![row(1, 7, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert!(rec.resolved().is_empty(), "WillBeSent is no resolution");
    src.set_rows(W, vec![row(1, 7, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(
        rec.resolved(),
        vec![(REG.to_owned(), DispatchResolution::Sent)]
    );
    // Unchanged: no second event.
    t.refresh_funding(W).await;
    assert_eq!(rec.resolved().len(), 1);

    // DP1-05's adoption: a lost lock found on the chain.
    const ADOPTED: &str = "registration/8/funding";
    maybe_sent(&l, ADOPTED, 8).await;
    src.set_rows(
        W,
        vec![row(1, 7, R::ChainLocked), row(2, 8, R::RecoveredFromChain)],
    );
    t.refresh_funding(W).await;
    assert_eq!(
        rec.resolved()[1],
        (ADOPTED.to_owned(), DispatchResolution::Sent)
    );

    // Repair's self-spend, ChainLocked later.
    maybe_sent(&l, TOPUP, 9).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    assert!(
        j.insert_self_spend(&W.0, TOPUP, &id.0, &[0xee; 32])
            .unwrap()
    );
    let (t, rec) = reload(&j, &src).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(NotSent));
    assert_eq!(
        rec.resolved(),
        vec![(TOPUP.to_owned(), DispatchResolution::NotSent)],
        "only the move; the reload's first derivation is a baseline"
    );

    // A definite resolution: a call that released nothing.
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    const LOCKED: &str = "topup/j/funding";
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(LOCKED, 10))
        .await
        .unwrap();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, LOCKED) == Some(NotSent)).await;
    until(|| rec.resolved().len() == 2).await;
    assert_eq!(
        rec.resolved()[1],
        (LOCKED.to_owned(), DispatchResolution::NotSent)
    );

    // A call that returned Ok (the Lock above parked that lease).
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    const OK: &str = "topup/k/funding";
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(OK, 11))
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    assert_eq!(t.funding_status(W, OK), Some(Sent));
    assert_eq!(rec.resolved()[2], (OK.to_owned(), DispatchResolution::Sent));
    assert_eq!(rec.resolved().len(), 3);
}

// --- funds_committed (§16.5) ----------------------------------------------

/// From the J step that takes the funding call's permit until its status
/// is `NotSent`; a possible send keeps it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn funds_committed_holds_from_the_permit_until_not_sent() {
    let (t, rec, _, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    assert!(!funds_committed(&l));
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    assert!(funds_committed(&l), "set with the permit and its marker");
    // The permit's J step announced it.
    assert!(rec.leases_changed().iter().any(|v| v.funds_committed));
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| !funds_committed(&l)).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));

    // A possible send keeps it (the Lock above parked that lease).
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, TOPUP, 8).await;
    assert!(funds_committed(&l));
}

/// §16.5: a tracked row matching the draft's own key sets it with no
/// marker, for the flow bound to the draft.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_matching_the_drafts_key_sets_funds_committed_with_no_marker() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_drafts(W, vec![DraftKey::registration(7, 7)]);
    t.refresh_funding(W).await;
    l.bind_funding_step(REG);
    assert!(!funds_committed(&l), "never registered");
    src.set_rows(W, vec![row(1, 7, R::Built)]);
    t.refresh_funding(W).await;
    assert!(funds_committed(&l));
    assert_eq!(t.call_step(W, REG).status, CallStepStatus::None);
}

// --- The funding gate (§2a.3) ---------------------------------------------

/// The cap: `amount + fee_bound_worst` against the Funding budget, charged
/// as the sum; a funding call needs a Funding quote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_funding_cap_refuses_amount_plus_the_worst_fee_and_charges_the_sum() {
    let (t, _, _, _) = setup().await;
    let l = begin(&t, W, 1_000, 1_000, 0).await.unwrap();
    assert_eq!(
        l.call_permit_step("funding", quote(900, 101), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::Exceeded {
            purpose: BudgetPurpose::Funding,
            needed: 1_001,
            remaining: 1_000,
        }
    );
    assert_eq!(
        t.call_step(W, REG).status,
        CallStepStatus::None,
        "no marker"
    );
    let overflow = quote(u64::MAX, 1);
    assert!(matches!(
        l.call_permit_step("funding", overflow, funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::Exceeded { .. }
    ));
    let credits = Some(CallQuote {
        purpose: BudgetPurpose::Credits,
        amount: 1,
    });
    assert_eq!(
        l.call_permit_step("funding", credits, funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::NeedsGrant(BudgetPurpose::Funding)
    );
    let p = l
        .call_permit_step("funding", quote(900, 100), funding(REG, 7))
        .await
        .unwrap();
    assert_eq!(funding_spent(&l), 1_000, "the sum");
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    assert_eq!(funding_spent(&l), 1_000);
}

/// No funding build over a possibly-sent row: any `Built` or `Broadcast`
/// row blocks the wallet's funding until the catch-up resumed it; rows
/// with a proof never do; rows never read block too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_built_or_broadcast_row_blocks_funding_until_it_is_resumed() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_rows(
        W,
        vec![
            row(1, 1, R::Built),
            row(2, 2, R::Broadcast),
            row(3, 3, R::ChainLocked),
        ],
    );
    t.refresh_funding(W).await;
    let refused = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        LeaseError::FundingBlocked(FundingBlock::Row(ArtifactId::from_txid(&txid(1))))
    );
    assert_eq!(
        PlatformError::from(refused),
        PlatformError::WillBeSent {
            artifact: txid(1).to_string()
        }
    );
    assert_eq!(funding_spent(&l), 0, "refused before the charge");
    assert_eq!(t.call_step(W, REG).status, CallStepStatus::None);
    t.note_resumed(W, txid(1));
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Row(ArtifactId::from_txid(&txid(2))))
    );
    // Another wallet's resume does not count.
    t.note_resumed(W2, txid(2));
    assert!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .is_err()
    );
    t.note_resumed(W, txid(2));
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    // A resumable step is not a funding call: no gate.
    src.set_rows(W, vec![row(4, 4, R::Built)]);
    t.refresh_funding(W).await;
    l.call_permit_step(
        "submit",
        None,
        CallStep::Resumable("withdrawal/w/submit".into()),
    )
    .await
    .unwrap()
    .finish(true);

    // A wallet whose rows were never read: refused, fail closed.
    let (t2, _) = table();
    let _j2 = with_journal(&t2);
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    let refused = l2
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        LeaseError::FundingBlocked(FundingBlock::RowsUnknown)
    );
    assert_eq!(PlatformError::from(refused).code(), "platform.unavailable");
    // Before its rows are read, a step never reads None or NotSent.
    assert_eq!(t2.funding_status(W, REG), Some(MaybeSent));
}

// --- No double pay (§2a.4 item 7, against the fake library) ---------------

/// Retry and discard only on `NotSent`: a possibly-sent funding step is
/// refused a second funding call (no T2), with its status' code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_possibly_sent_funding_step_refuses_a_second_funding_call() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, REG, 7).await;
    let spent = funding_spent(&l);
    let refused = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap_err();
    assert_eq!(
        refused,
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent)))
    );
    assert_eq!(PlatformError::from(refused).code(), "invalid_argument");
    assert_eq!(funding_spent(&l), spent, "no charge");
    // Its row appears: WillBeSent, still refused (the catch-up resends it).
    src.set_rows(W, vec![row(1, 7, R::Built)]);
    t.refresh_funding(W).await;
    t.note_resumed(W, txid(1));
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(WillBeSent)))
    );
    // Sent: never funded again.
    src.set_rows(W, vec![row(1, 7, R::Consumed)]);
    t.refresh_funding(W).await;
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(Sent)))
    );
}

/// The withheld-T1 power-loss trace (§2a.4 item 7, GPT r1 5): the lock T1
/// was signed and handed to a peer that withholds it, the process lost
/// power before the row was persisted, and DP1-05 finds nothing on the
/// chain. The marker survives: the step reads MaybeSent indefinitely, so
/// - discard is refused (§16.6 allows it only on `NotSent` or `None`), and
///   DP1-02's "Register again" discards first: no new step either;
/// - a second funding call of the same step is refused by the gate.
///
/// So no T2 is built. Only Repair's self-spend, once ChainLocked, resolves
/// it (`NotSent`), and then only under a new step; with no lock of the
/// step known, Repair waits for DP1-05's scan to complete (review R1-01).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_withheld_t1_power_loss_trace_builds_no_t2() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_drafts(W, vec![DraftKey::registration(7, 7)]);
    t.refresh_funding(W).await;
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    p.signatures().note_released();
    // Power loss: the marker is durable, the row was never persisted.
    let (t2, _) = reload(&j, &src).await;
    drop(p);
    let status = t2.funding_status(W, REG);
    assert_eq!(status, Some(MaybeSent));
    let discard_allowed = matches!(status, None | Some(NotSent));
    assert!(!discard_allowed, "discard is refused");
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent))),
        "no T2"
    );
    assert!(funds_committed_after_bind(&l2, REG));
    // DP1-05 finds nothing: still MaybeSent, with no time bound.
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, REG), Some(MaybeSent));
    assert_eq!(
        t2.record_self_spend(W, REG, [1; 32]).await,
        Err(StuckFundingRefusal::NoKnownLock),
        "the scan is not shown complete"
    );
}

fn funds_committed_after_bind(l: &Lease, step: &str) -> bool {
    l.bind_funding_step(step);
    funds_committed(l)
}

/// A definitely-unsent funding allows funding again: a Lock before the
/// first signature (Cancelled → `NotSent`, the same step funds afresh) and
/// Repair's ChainLocked self-spend (`NotSent`; discard is allowed, and a
/// new step funds). So does a funding the gate refused before its marker
/// (here over an unresumed `Built` row): it never registered (`None`).
/// A not-ready rejection after signing is not covered: Mode B at the pin
/// cannot tell it (§2a.6, review M-2).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unsent_or_never_started_funding_allows_funding_again() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();

    // A Lock before signing.
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, REG) == Some(NotSent)).await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    assert_eq!(p.finish(true), FlowOutcome::Sent);

    // Refused by the gate before any marker, so never registered.
    src.set_rows(W, vec![row(1, 1, R::Built)]);
    t.refresh_funding(W).await;
    assert!(
        l.call_permit_step("funding", quote(10, 0), funding(TOPUP, 8))
            .await
            .is_err()
    );
    assert_eq!(t.funding_status(W, TOPUP), None);
    src.set_rows(W, Vec::new());
    t.refresh_funding(W).await;
    l.call_permit_step("funding", quote(10, 0), funding(TOPUP, 8))
        .await
        .unwrap()
        .finish(true);

    // Repair's self-spend resolves an orphan.
    const ORPHAN: &str = "registration/3/funding";
    maybe_sent(&l, ORPHAN, 3).await;
    let id = ArtifactId::of_step(&W, ORPHAN);
    assert!(
        j.insert_self_spend(&W.0, ORPHAN, &id.0, &[0xaa; 32])
            .unwrap()
    );
    let (t2, rec2) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, ORPHAN), Some(MaybeSent));
    src.locked.lock().unwrap().insert([0xaa; 32]);
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, ORPHAN), Some(NotSent));
    assert_eq!(
        rec2.resolved(),
        vec![(ORPHAN.to_owned(), DispatchResolution::NotSent)]
    );
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    // The marker still stands, answered by the self-spend: its step is not
    // funded again (a call under it could not be told from the orphan).
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(ORPHAN, 3))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(NotSent)))
    );
    // "Register again" under a new draft: allowed.
    l2.call_permit_step(
        "funding",
        quote(10, 0),
        funding("registration/4/funding", 3),
    )
    .await
    .unwrap()
    .finish(true);
}

/// A snapshot that never goes back: refreshes are serialized, and an
/// unreadable source leaves the last snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_that_reads_nothing_keeps_the_last_snapshot() {
    struct Unloaded;
    #[async_trait]
    impl TrackedLocks for Unloaded {
        async fn rows(&self, _: WalletId) -> Option<Vec<TrackedRow>> {
            None
        }
        async fn drafts(&self, _: WalletId) -> Option<Vec<DraftKey>> {
            None
        }
        async fn chain_locked(&self, _: WalletId, _: &[u8; 32]) -> bool {
            false
        }
    }
    let (t, _) = table();
    let _j = with_journal(&t);
    assert!(!t.refresh_funding(W).await, "no source installed");
    t.commit_snapshot(
        W,
        Snapshot {
            rows: vec![row(1, 7, R::Built)],
            ..Default::default()
        },
    );
    t.install_tracked(Arc::new(Unloaded));
    assert!(!t.refresh_funding(W).await);
    assert!(t.has_tracked_snapshot(W));
    assert_eq!(t.funding_status(W, REG), None, "no key matches");
    assert!(!t.has_tracked_snapshot(W2));
}

// --- Rows of earlier fundings under the same key ---------------------------

/// Every top-up of an identity carries one funding key, and a `Consumed`
/// row stays: a later step marked under the key does not read an earlier
/// funding's row as its own (in its process; after a restart it does,
/// failing closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_funding_under_the_same_key_does_not_read_an_earlier_ones_row() {
    const A: &str = "topup/a/funding";
    const B: &str = "topup/b/funding";
    let (t, rec, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();

    // The first top-up went out and its lock was consumed.
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(A, 5))
        .await
        .unwrap();
    p.signatures().note_released();
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    src.set_rows(W, vec![row(1, 5, R::Consumed)]);
    t.refresh_funding(W).await;
    assert_eq!(
        rec.resolved(),
        vec![(A.to_owned(), DispatchResolution::Sent)]
    );

    // The second: its marker's key matches the first's row, which is not
    // its lock.
    assert_eq!(t.funding_status(W, B), None);
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(B, 5))
        .await
        .unwrap();
    assert_eq!(t.funding_status(W, B), Some(MaybeSent));
    p.signatures().note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, B), Some(MaybeSent));
    assert_eq!(rec.resolved().len(), 1, "no Sent for B from A's row");

    // B's own row: it reads B's progress, and A stays Sent.
    src.set_rows(W, vec![row(1, 5, R::Consumed), row(2, 5, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, B), Some(WillBeSent));
    assert_eq!(t.funding_status(W, A), Some(Sent));
    src.set_rows(W, vec![row(1, 5, R::Consumed), row(2, 5, R::Consumed)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, B), Some(Sent));
    assert_eq!(rec.resolved()[1], (B.to_owned(), DispatchResolution::Sent));

    // A third, possibly sent, then a restart: the earlier rows count again
    // (the residual), which reads Sent, never None or NotSent.
    const C: &str = "topup/c/funding";
    maybe_sent(&l, C, 5).await;
    assert_eq!(t.funding_status(W, C), Some(MaybeSent));
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, C), Some(Sent));
}

/// A funding call's coin fence (E0-04 §2a.6) is released by the first
/// tracked row the changeset tap reports under the call's key in its
/// wallet that is not foreign to its step, once the call released a
/// signature: not by a row that preceded its marker (an earlier
/// funding's, stored again), one of another key or one of another wallet,
/// nor by its own key's row before its first signature. A waiter for the
/// fence ("Lock unspent") gets it then; the call itself keeps running.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_funding_calls_fence_is_released_by_its_own_tracked_row_only() {
    const A: &str = "topup/a/funding";
    const B: &str = "topup/b/funding";
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    // An earlier funding's lock under the same key.
    l.call_permit_step("funding", quote(10, 0), funding(A, 5))
        .await
        .unwrap()
        .finish(true);
    src.set_rows(W, vec![row(1, 5, R::Consumed)]);
    t.refresh_funding(W).await;

    let fences = Arc::new(crate::coins::CoinFences::default());
    let fence = fences.acquire(W).await;
    let p = l
        .call_permit_step_fenced("funding", quote(10, 0), funding(B, 5), Some(fence))
        .await
        .unwrap();
    assert!(t.call_holds_fence(W));
    let waiter = tokio::spawn({
        let fences = Arc::clone(&fences);
        async move { fences.acquire(W).await }
    });
    // Before the call's first signature no row is its lock.
    t.note_tracked(W, &[(txid(2), key(5))]);
    p.signatures().note_released();
    t.note_tracked(W, &[(txid(1), key(5))]);
    t.note_tracked(W, &[(txid(2), key(6))]);
    t.note_tracked(W, &[]);
    t.note_tracked(W2, &[(txid(2), key(5))]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiter.is_finished(), "still fenced");
    assert!(t.call_holds_fence(W));
    // The call's own lock.
    t.note_tracked(W, &[(txid(1), key(5)), (txid(2), key(5))]);
    let got = tokio::time::timeout(Duration::from_secs(5), waiter)
        .await
        .expect("released")
        .unwrap();
    assert_eq!(got.wallet_id(), W);
    assert!(!t.call_holds_fence(W));
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
}

/// A funding call's fence is released at the call's end and with a
/// refusal; a resumable step's call holds none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_funding_calls_fence_is_released_at_its_end_or_refusal() {
    let (t, _, _, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let fences = crate::coins::CoinFences::default();
    let reacquired = |what: &'static str| {
        let fences = &fences;
        async move {
            tokio::time::timeout(Duration::from_secs(5), fences.acquire(W))
                .await
                .expect(what)
        }
    };
    let p = l
        .call_permit_step_fenced(
            "funding",
            quote(10, 0),
            funding(REG, 7),
            Some(fences.acquire(W).await),
        )
        .await
        .unwrap();
    assert!(t.call_holds_fence(W));
    p.signatures().note_released();
    drop(p);
    assert!(!t.call_holds_fence(W));
    drop(reacquired("released at the call's end").await);
    // Refused by the gate (the step reads MaybeSent): released with it.
    assert!(
        l.call_permit_step_fenced(
            "funding",
            quote(10, 0),
            funding(REG, 7),
            Some(fences.acquire(W).await),
        )
        .await
        .is_err()
    );
    drop(reacquired("released with the refusal").await);
    // A resumable step's call holds none.
    let p = l
        .call_permit_step_fenced(
            "submit",
            None,
            CallStep::Resumable("withdrawal/w/submit".into()),
            Some(fences.acquire(W).await),
        )
        .await
        .unwrap();
    assert!(!t.call_holds_fence(W));
    drop(reacquired("never held by a resumable step's call").await);
    p.finish(true);
}

// --- DispatchResolved's baseline ------------------------------------------

/// A step's first derivation is silent, unless its status was reported
/// before the wallet's first snapshot: then the snapshot's resolution is a
/// move from that answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_status_reported_before_the_first_snapshot_is_the_baseline() {
    const ADOPTED: &str = "registration/8/funding";
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, REG, 7).await;
    maybe_sent(&l, ADOPTED, 8).await;

    // The next process, its rows not read yet.
    let snap = j.snapshot();
    let (t2, rec2) = table();
    snap.load_into(&t2);
    t2.install_tracked(src.clone());
    assert_eq!(t2.funding_status(W, REG), Some(MaybeSent), "reported");
    src.set_rows(
        W,
        vec![row(1, 7, R::ChainLocked), row(2, 8, R::RecoveredFromChain)],
    );
    assert!(t2.refresh_funding(W).await);
    assert_eq!(t2.funding_status(W, ADOPTED), Some(Sent));
    assert_eq!(
        rec2.resolved(),
        vec![(REG.to_owned(), DispatchResolution::Sent)],
        "the reported step moved; the unreported one has its baseline"
    );
}

// --- The refresh hooks -----------------------------------------------------

/// A Core change refreshes a wallet only while one of Repair's self-spends
/// stands unseen ChainLocked; the spawned refresh reads the ChainLock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wallet_awaits_a_self_spend_lock_until_its_snapshot_sees_it() {
    let (t, _, j, src) = setup().await;
    assert!(!t.awaits_self_spend_lock(W));
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, TOPUP, 9).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    assert!(
        j.insert_self_spend(&W.0, TOPUP, &id.0, &[0xee; 32])
            .unwrap()
    );
    let (t, rec) = reload(&j, &src).await;
    assert!(t.awaits_self_spend_lock(W));
    assert!(!t.awaits_self_spend_lock(W2));
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.request_refresh(W);
    until(|| t.funding_status(W, TOPUP) == Some(NotSent)).await;
    assert!(!t.awaits_self_spend_lock(W));
    assert_eq!(
        rec.resolved(),
        vec![(TOPUP.to_owned(), DispatchResolution::NotSent)]
    );
}

/// A read that the wallet's removal overtook is dropped, not stored for a
/// removed wallet.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_overtaken_by_the_wallets_removal_is_dropped() {
    let (t, _) = table();
    let _j = with_journal(&t);
    let src = Gated::new(vec![row(1, 7, R::Built)]);
    t.install_tracked(src.clone());
    let refresh = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.refresh_funding(W).await }
    });
    src.entered.notified().await;
    t.with_j(|i, _| i.mode_b.forget(&W));
    src.release.notify_one();
    assert!(!refresh.await.unwrap());
    assert!(!t.has_tracked_snapshot(W));
}

/// Review L-5: the removal epoch is per wallet, so one wallet's removal
/// drops no other wallet's refresh in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_wallets_removal_keeps_a_refresh_in_flight() {
    let (t, _) = table();
    let _j = with_journal(&t);
    let src = Gated::new(Vec::new());
    t.install_tracked(src.clone());
    let refresh = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.refresh_funding(W2).await }
    });
    src.entered.notified().await;
    t.with_j(|i, _| i.mode_b.forget(&W));
    src.release.notify_one();
    assert!(refresh.await.unwrap());
    assert!(t.has_tracked_snapshot(W2));
}

// --- Unattributed rows (review H-1) ----------------------------------------

/// A tracked lock at the pin for the tests below.
fn tracked_lock(
    out_point: OutPoint,
    identity_index: u32,
    status: platform_wallet::AssetLockStatus,
) -> platform_wallet::TrackedAssetLock {
    platform_wallet::TrackedAssetLock {
        out_point,
        transaction: dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: OutPoint::new(txid(200), 0),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        },
        account_index: 0,
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index,
        amount: 1_000,
        status,
        proof: None,
    }
}

/// Which rows' identity index is trusted as it stands: the pin's
/// reconstruction gives every singleton family index 0 and a bound
/// top-up its exact index, and only the live pipeline builds.
#[test]
fn a_rows_identity_index_is_trusted_when_built_non_zero_or_a_bound_top_up() {
    use AssetLockFundingType as T;
    let at = |funding_type, identity_index| FundingKey {
        funding_type,
        identity_index,
        account: 0,
    };
    for t in [
        T::IdentityRegistration,
        T::IdentityTopUpNotBound,
        T::IdentityInvitation,
        T::AssetLockAddressTopUp,
        T::AssetLockShieldedAddressTopUp,
    ] {
        assert!(index_trusted(&at(t, 0), R::Built), "{t:?}");
        assert!(index_trusted(&at(t, 3), R::RecoveredFromChain), "{t:?}");
        for status in [
            R::Broadcast,
            R::InstantSendLocked,
            R::ChainLocked,
            R::Consumed,
            R::RecoveredFromChain,
        ] {
            assert!(!index_trusted(&at(t, 0), status), "{t:?} {status:?}");
        }
    }
    assert!(index_trusted(
        &at(T::IdentityTopUp, 0),
        R::RecoveredFromChain
    ));
}

/// The real source re-keys an untrusted registration row by the identity
/// its outpoint created, when that identity is one of the wallet's.
#[test]
fn an_untrusted_registration_row_is_re_keyed_by_the_identity_it_created() {
    use super::mode_b::{attributed_row, identity_for};
    use platform_wallet::AssetLockStatus as A;
    let out_point = OutPoint::new(txid(1), 1);
    let lock = tracked_lock(out_point, 0, A::RecoveredFromChain);
    let mine = HashMap::from([(identity_for(out_point), 3)]);
    let r = attributed_row(&lock, &mine);
    assert_eq!((r.key, r.trusted), (key(3), true));
    // Another outpoint's identity (vout 0 of the same transaction): left
    // as it is, unattributed.
    let other = HashMap::from([(identity_for(OutPoint::new(txid(1), 0)), 3)]);
    let r = attributed_row(&lock, &other);
    assert_eq!((r.key, r.trusted), (key(0), false));
    // A trusted row keeps its key, whatever identity it created.
    let built = tracked_lock(out_point, 0, A::Built);
    let r = attributed_row(&built, &mine);
    assert_eq!((r.key, r.trusted), (key(0), true));
    // Another funding type is not re-keyed by a registration's identity.
    let mut invitation = tracked_lock(out_point, 0, A::RecoveredFromChain);
    invitation.funding_type = AssetLockFundingType::IdentityInvitation;
    assert!(!attributed_row(&invitation, &mine).trusted);
}

/// The pin re-adds a registration lock that lost its row at identity
/// index 0. For a step at another index, that row is unattributed: the
/// step keeps reading `MaybeSent` (a ChainLocked self-spend proves nothing
/// over it) and Repair refuses. A step at index 0 still matches it by key,
/// and a row re-keyed by its identity reads for that identity's step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattributed_row_blocks_the_self_spends_not_sent_and_repair() {
    use StuckFundingRefusal as F;
    const AT3: &str = "registration/3/funding";
    const AT0: &str = "registration/4/funding";
    let (t, rec, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, AT3, 3).await;
    maybe_sent(&l, AT0, 0).await;
    // A self-spend of each step, recorded before any row came back.
    recovery_complete(&t, &src).await;
    t.record_self_spend(W, AT3, [0xa3; 32]).await.unwrap();
    t.record_self_spend(W, AT0, [0xa0; 32]).await.unwrap();

    // T1 of the step at index 3 went out, lost its row and was re-added
    // from the chain at index 0.
    let lost = row(1, 0, R::RecoveredFromChain);
    assert!(!lost.trusted);
    src.set_rows(W, vec![lost.clone()]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    assert_eq!(
        t.record_self_spend(W, AT3, [0xb3; 32]).await,
        Err(F::UnattributedLock(txid(1)))
    );
    src.locked.lock().unwrap().insert([0xa3; 32]);
    t.refresh_funding(W).await;
    assert_eq!(
        t.funding_status(W, AT3),
        Some(MaybeSent),
        "a ChainLocked self-spend does not read NotSent"
    );
    // The step at index 0 matches the row by key: Sent.
    assert_eq!(t.funding_status(W, AT0), Some(Sent));

    // A pre-finality reconstruction (Broadcast) is unattributed too.
    src.set_rows(W, vec![row(1, 0, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    // At index 0 it matches by key: the ChainLocked self-spend conflicts
    // with it as before.
    src.locked.lock().unwrap().insert([0xa0; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT0), Some(NotSent));
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));

    // The same row, re-keyed by the identity at index 3 it created: the
    // step at index 3 reads Sent, and the one at index 0 no longer sees it.
    src.set_rows(
        W,
        vec![TrackedRow {
            key: key(3),
            trusted: true,
            ..lost
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(Sent));
    assert_eq!(t.funding_status(W, AT0), Some(NotSent));
    let at3: Vec<_> = rec
        .resolved()
        .into_iter()
        .filter(|(s, _)| s == AT3)
        .collect();
    assert_eq!(
        at3,
        vec![(AT3.to_owned(), DispatchResolution::Sent)],
        "the step at index 3 never read NotSent"
    );
}

/// A `Built` row is the live pipeline's own build, whatever its index: it
/// leaves another index's ChainLocked self-spend reading `NotSent`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_built_row_at_index_0_is_attributed() {
    const AT3: &str = "registration/3/funding";
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, AT3, 3).await;
    recovery_complete(&t, &src).await;
    t.record_self_spend(W, AT3, [0xa3; 32]).await.unwrap();
    src.set_rows(W, vec![row(1, 0, R::Built)]);
    src.locked.lock().unwrap().insert([0xa3; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(NotSent));
}

/// An unattributed row of another funding type says nothing of a step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattributed_row_of_another_funding_type_does_not_block() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, REG, 7).await;
    recovery_complete(&t, &src).await;
    t.record_self_spend(W, REG, [0xa7; 32]).await.unwrap();
    let invitation = TrackedRow {
        key: FundingKey {
            funding_type: AssetLockFundingType::IdentityInvitation,
            ..key(0)
        },
        ..row(1, 0, R::RecoveredFromChain)
    };
    assert!(!invitation.trusted);
    src.set_rows(W, vec![invitation]);
    src.locked.lock().unwrap().insert([0xa7; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
}

/// The pure derivation: with an unattributed row that may be its lock, a
/// step with no standing marker reads `MaybeSent` instead of `None` or
/// `NotSent`; its matching rows still read as before.
#[test]
fn an_unattributed_row_makes_a_markerless_step_read_maybe_sent() {
    for step in [S::None, S::NotSent] {
        assert_eq!(
            derive_funding(step, Some(&[]), false, true),
            Some(MaybeSent),
            "{step:?}"
        );
        assert_eq!(
            derive_funding(step, Some(&[R::RecoveredFromChain]), false, true),
            Some(Sent)
        );
        assert_eq!(
            derive_funding(step, Some(&[R::Broadcast]), false, true),
            Some(WillBeSent)
        );
    }
}

/// A lost or rolled-back journal: draft 3's funding call ran, its marker
/// is gone, and its lock came back from the chain at identity index 0
/// (unattributed). The markerless step reads `MaybeSent`, so the gate
/// refuses a second funding, until the row is re-keyed by the identity it
/// created (index 3): then it reads `Sent`. A step at index 0 matches the
/// row by key and reads by its status; a markerless draft with no
/// unattributed row reads `None`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_markerless_registration_step_reads_maybe_sent_over_an_unattributed_row() {
    const AT3: &str = "registration/3/funding";
    const AT0: &str = "registration/4/funding";
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_drafts(
        W,
        vec![DraftKey::registration(3, 3), DraftKey::registration(4, 0)],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), None, "no unattributed row");
    assert_eq!(t.funding_status(W, AT0), None);

    let lost = row(1, 0, R::RecoveredFromChain);
    assert!(!lost.trusted);
    src.set_rows(W, vec![lost.clone()]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    let spent = funding_spent(&l);
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(AT3, 3))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent))),
        "no second funding"
    );
    assert_eq!(funding_spent(&l), spent, "no charge");
    // The step at index 0 matches the row by key: by its status.
    assert_eq!(t.funding_status(W, AT0), Some(Sent));
    src.set_rows(W, vec![row(1, 0, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT0), Some(WillBeSent));
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));

    // An unattributed row of another account is no lock of the draft
    // (DP1-02 funds every registration from account 0).
    src.set_rows(
        W,
        vec![TrackedRow {
            key: FundingKey {
                account: 1,
                ..key(0)
            },
            ..row(1, 0, R::RecoveredFromChain)
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), None);

    // Re-keyed by the managed identity at index 3 it created.
    src.set_rows(
        W,
        vec![TrackedRow {
            key: key(3),
            trusted: true,
            ..lost
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(Sent));
    assert_eq!(t.funding_status(W, AT0), None);
}

/// A rolled-back journal kept draft 0's marker and lost draft 3's: the
/// unattributed row is not attributed to draft 0's marker alone, so the
/// markerless step at index 3 still reads `MaybeSent`. A step whose
/// definite `NotSent` stands reads `MaybeSent` too while such a row may be
/// the lock of a later call of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattributed_row_is_not_given_to_another_drafts_marker() {
    const AT3: &str = "registration/3/funding";
    const AT0: &str = "registration/4/funding";
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    src.set_drafts(
        W,
        vec![DraftKey::registration(3, 3), DraftKey::registration(4, 0)],
    );
    t.refresh_funding(W).await;
    maybe_sent(&l, AT0, 0).await;
    // A definite NotSent of draft 3 (a Lock before its first signature).
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(AT3, 3))
        .await
        .unwrap();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, AT3) == Some(NotSent)).await;

    src.set_rows(W, vec![row(1, 0, R::RecoveredFromChain)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT0), Some(Sent));
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(AT3, 3))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent)))
    );
}

// --- Repair's self-spend (§6.5, §2a.5 row 1; commit 7) --------------------

/// Repair records its self-spend against the step's standing marker in the
/// journal (append-only), and in memory once that write landed. Recording
/// is no resolution: the step stays `MaybeSent` while the self-spend is
/// not ChainLocked (it may never confirm), across a restart too, and reads
/// `NotSent`, with `DispatchResolved(NotSent)`, once it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recorded_self_spend_resolves_only_once_chain_locked() {
    use dw_appdb::dispatch::{Resolution, StepEvent};
    let (t, rec, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, TOPUP, 9).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    let marker = j
        .log()
        .iter()
        .find(|r| r.artifact == id.0)
        .map(|r| r.seq)
        .unwrap();

    // With no lock of the step known, only a complete recovery lets Repair
    // resolve it (review R1-01).
    assert_eq!(
        t.record_self_spend(W, TOPUP, [0xee; 32]).await,
        Err(StuckFundingRefusal::NoKnownLock)
    );
    recovery_complete(&t, &src).await;
    t.record_self_spend(W, TOPUP, [0xee; 32]).await.unwrap();
    assert_eq!(t.call_step(W, TOPUP).self_spends, vec![[0xee; 32]]);
    let rows = j.load_calls().self_spends;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].step_id.as_str(), rows[0].artifact, rows[0].txid),
        (TOPUP, id.0, [0xee; 32])
    );
    assert_eq!(rows[0].marker, marker, "it answers the standing marker");
    assert!(
        !j.log()
            .iter()
            .any(|r| r.artifact == id.0 && r.event == StepEvent::Resolved(Resolution::NotSent)),
        "no NotSent row is written for it"
    );
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    assert!(t.awaits_self_spend_lock(W));

    // Never confirmed: MaybeSent, refresh after refresh.
    t.refresh_funding(W).await;
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    assert!(rec.resolved().is_empty());

    // A second self-spend (the first one rejected and abandoned, so its
    // coins are the second's too: `repair.rs`) is appended as well.
    t.record_self_spend(W, TOPUP, [0xef; 32]).await.unwrap();
    assert_eq!(j.load_calls().self_spends.len(), 2);

    // A restart keeps both rows with their marker.
    let (t2, rec2) = reload(&j, &src).await;
    assert_eq!(
        t2.call_step(W, TOPUP).self_spends,
        vec![[0xee; 32], [0xef; 32]]
    );
    assert_eq!(t2.funding_status(W, TOPUP), Some(MaybeSent));
    assert!(rec2.resolved().is_empty());

    // The second one is ChainLocked: NotSent, announced once.
    src.locked.lock().unwrap().insert([0xef; 32]);
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, TOPUP), Some(NotSent));
    assert_eq!(
        rec2.resolved(),
        vec![(TOPUP.to_owned(), DispatchResolution::NotSent)]
    );
    t2.refresh_funding(W).await;
    assert_eq!(rec2.resolved().len(), 1);
    // Resolved, so nothing is stuck any more.
    assert_eq!(
        t2.record_self_spend(W, TOPUP, [0xf0; 32]).await,
        Err(StuckFundingRefusal::NotStuck(Some(NotSent)))
    );
}

/// A tracked `Built` row of the step (`WillBeSent`, no proof) is stuck too;
/// the self-spend spends its inputs, so once ChainLocked it reads `NotSent`
/// over the row (§2a.5 row 1 before row 3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_will_be_sent_funding_resolves_by_its_self_spend() {
    let (t, rec, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    let built = row(1, 9, R::Built);
    sign_lock(&t, &p, W, &built);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    src.set_rows(W, vec![built]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(WillBeSent));
    assert_eq!(
        t.check_stuck_funding(W, TOPUP).unwrap(),
        vec![(txid(1), vec![OutPoint::new(txid(101), 0)])]
    );
    t.record_self_spend(W, TOPUP, [0xee; 32]).await.unwrap();
    assert_eq!(t.funding_status(W, TOPUP), Some(WillBeSent));
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(NotSent));
    assert_eq!(
        rec.resolved(),
        vec![(TOPUP.to_owned(), DispatchResolution::NotSent)]
    );
}

/// What Repair refuses, before anything is written: unknown rows, a step
/// that is not stuck (never started, sent, definitely unsent), a step
/// stuck only by the draft's own key (no marker to answer), a call running
/// under the marker, and a journal that is not ready or fails the write
/// (then nothing is recorded in memory either).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_refuses_a_step_that_is_not_stuck() {
    use StuckFundingRefusal as F;
    let (t, _, j, src) = setup().await;
    assert_eq!(
        t.record_self_spend(W, REG, [1; 32]).await,
        Err(F::NotStuck(None)),
        "never started"
    );
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap()
        .finish(true);
    assert_eq!(
        t.record_self_spend(W, REG, [1; 32]).await,
        Err(F::NotStuck(Some(Sent)))
    );
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 8))
        .await
        .unwrap();
    assert_eq!(
        t.record_self_spend(W, TOPUP, [1; 32]).await,
        Err(F::CallRunning)
    );
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, TOPUP) == Some(NotSent)).await;
    assert_eq!(
        t.record_self_spend(W, TOPUP, [1; 32]).await,
        Err(F::NotStuck(Some(NotSent)))
    );

    // A draft's own row with no marker (a lost journal).
    const LOST: &str = "registration/5/funding";
    src.set_drafts(W, vec![DraftKey::registration(5, 5)]);
    src.set_rows(W, vec![row(2, 5, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), Some(WillBeSent));
    assert_eq!(
        t.record_self_spend(W, LOST, [1; 32]).await,
        Err(F::NoMarker)
    );

    // A failed write records nothing, on disk or in memory.
    const ORPHAN: &str = "topup/o/funding";
    src.set_rows(W, Vec::new());
    t.refresh_funding(W).await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, ORPHAN, 11).await;
    // No lock of it is known and the wallet's recovery is not complete.
    assert_eq!(
        t.record_self_spend(W, ORPHAN, [1; 32]).await,
        Err(F::NoKnownLock)
    );
    recovery_complete(&t, &src).await;
    j.fail_self_spend.store(true, Ordering::SeqCst);
    assert_eq!(
        t.record_self_spend(W, ORPHAN, [1; 32]).await,
        Err(F::JournalUnavailable)
    );
    assert!(t.call_step(W, ORPHAN).self_spends.is_empty());
    assert!(j.load_calls().self_spends.is_empty());
    j.fail_self_spend.store(false, Ordering::SeqCst);
    t.record_self_spend(W, ORPHAN, [1; 32]).await.unwrap();

    // Rows never read this session.
    let (t2, _) = table();
    let _j2 = with_journal(&t2);
    t2.install_tracked(src.clone());
    assert_eq!(
        t2.record_self_spend(W, ORPHAN, [1; 32]).await,
        Err(F::RowsUnknown)
    );
    // No journal at all.
    let (t3, _) = table();
    assert_eq!(
        t3.record_self_spend(W, ORPHAN, [1; 32]).await,
        Err(F::JournalUnavailable)
    );
}

// --- A proved funding is journaled `Sent` (review R1-01) ------------------

/// Whether `j` holds `step`'s `Sent` row.
fn sent_on_disk(j: &FakeJournal, step: &str) -> bool {
    let id = ArtifactId::of_step(&W, step);
    j.load().1.iter().any(|r| r.artifact == id.0 && r.sent)
}

/// A funding step a ChainLocked row proved gets its `Sent` row: once the
/// row is gone (a sweep, a restore) the step still reads `Sent`, across a
/// restart and with the wallet's recovery complete, so Repair refuses it
/// and so does the funding gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proved_funding_is_journaled_sent_and_outlives_its_row() {
    use StuckFundingRefusal as F;
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, TOPUP, 9).await;
    src.set_rows(W, vec![row(1, 9, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
    until(|| sent_on_disk(&j, TOPUP)).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    assert_eq!(
        j.bundles
            .lock()
            .unwrap()
            .iter()
            .filter(|b| b.0 == id.0)
            .count(),
        1,
        "one bundle"
    );

    src.set_rows(W, Vec::new());
    recovery_complete(&t, &src).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, TOPUP), Some(Sent));
    assert_eq!(
        t2.check_stuck_funding(W, TOPUP).err(),
        Some(F::NotStuck(Some(Sent)))
    );
    assert_eq!(
        t2.record_self_spend(W, TOPUP, [1; 32]).await,
        Err(F::NotStuck(Some(Sent)))
    );
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(Sent)))
    );
}

/// A step with no marker, proved by a row under its draft's own key (a
/// lost journal), is journaled with a marker and that key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_markerless_step_proved_by_its_drafts_key_is_journaled_sent() {
    const LOST: &str = "registration/9/funding";
    let (t, _, j, src) = setup().await;
    src.set_drafts(W, vec![DraftKey::registration(9, 4)]);
    src.set_rows(W, vec![row(2, 4, R::InstantSendLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), Some(Sent));
    until(|| sent_on_disk(&j, LOST)).await;
    let id = ArtifactId::of_step(&W, LOST);
    assert!(
        j.funding_rows()
            .iter()
            .any(|r| r.step_id == LOST && r.artifact == id.0)
    );
    src.set_rows(W, Vec::new());
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, LOST), Some(Sent));
}

/// A registration at identity index 0 (the wallet's first identity): its
/// lock's row is untrusted once it left `Built`, but the step's call signed
/// it (§2a.6), so Repair counts it as the step's lock and its proof is
/// journaled (review P2b r1 F1, F2; r2 R2-01).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_0_registrations_own_lock_is_journaled_sent() {
    const AT0: &str = "registration/4/funding";
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(AT0, 0))
        .await
        .unwrap();
    sign_lock(&t, &p, W, &row(1, 0, R::Built));
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    let broadcast = row(1, 0, R::Broadcast);
    assert!(!broadcast.trusted);
    src.set_rows(W, vec![broadcast]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT0), Some(WillBeSent));
    assert!(t.check_stuck_funding(W, AT0).is_ok(), "its own lock");
    src.set_rows(W, vec![row(1, 0, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT0), Some(Sent));
    until(|| sent_on_disk(&j, AT0)).await;
    src.set_rows(W, Vec::new());
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, AT0), Some(Sent));
}

/// An untrusted row the tap never attributed to the step (index 0,
/// re-added from the chain) may be another singleton step's lock: Repair
/// does not count it as the step's (review P2b r1 F2), and its proof reads
/// `Sent` while it stands but is never journaled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattributed_index_0_row_is_not_the_steps_own() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, REG, 0).await;
    src.set_rows(W, vec![row(1, 0, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(WillBeSent));
    assert_eq!(
        t.check_stuck_funding(W, REG).err(),
        Some(StuckFundingRefusal::NoKnownLock)
    );
    let lost = row(1, 0, R::RecoveredFromChain);
    assert!(!lost.trusted);
    src.set_rows(W, vec![lost]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(Sent));
    src.set_rows(W, Vec::new());
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    assert!(!sent_on_disk(&j, REG));
}

/// A row two running calls could have signed (the same key), stored while
/// both ran, is neither step's own: the tap's sighting is no evidence of
/// whose it is (§2a.6), and neither proof is journaled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_two_calls_could_have_signed_is_neither_steps_own() {
    const A: &str = "registration/4/funding";
    const B: &str = "registration/5/funding";
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let pa = l
        .call_permit_step("funding", quote(10, 0), funding(A, 0))
        .await
        .unwrap();
    let pb = l
        .call_permit_step("funding", quote(10, 0), funding(B, 0))
        .await
        .unwrap();
    pa.signatures().note_released();
    pb.signatures().note_released();
    t.note_tracked(W, &[(txid(1), key(0))]);
    assert_eq!(pa.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(pb.finish(false), FlowOutcome::MaybeSent);
    src.set_rows(W, vec![row(1, 0, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, A), Some(Sent));
    assert_eq!(t.funding_status(W, B), Some(Sent));
    src.set_rows(W, Vec::new());
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, A), Some(MaybeSent));
    assert_eq!(t.funding_status(W, B), Some(MaybeSent));
    assert!(!sent_on_disk(&j, A) && !sent_on_disk(&j, B));
}

/// A funding call that returned `Sent` while the journal was closed wrote
/// no `Sent` row: once a proved row is read with the journal back, the row
/// is owed and written (review P2b r1 F3).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_sent_call_without_its_row_is_journaled_once_proved() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    p.signatures().note_released();
    t.close_journal().await;
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
    assert!(!sent_on_disk(&j, TOPUP));
    j.load_into(&t);
    src.set_rows(W, vec![row(1, 9, R::ChainLocked)]);
    t.refresh_funding(W).await;
    until(|| sent_on_disk(&j, TOPUP)).await;
}

/// The `Sent` write of a proved funding fails or panics: the step reads
/// `Sent` in memory and its row stays owed, so the journal's close writes
/// it, even after the row is gone. A kill before that: the reopened
/// process derives the step from its row again and writes it; with the row
/// gone too, it reads `MaybeSent` and Repair refuses (no lock known, the
/// recovery incomplete).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proved_fundings_failed_sent_write_stays_owed() {
    use StuckFundingRefusal as F;
    for panics in [false, true] {
        let (t, _, j, src) = setup().await;
        let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
        maybe_sent(&l, TOPUP, 9).await;
        let id = ArtifactId::of_step(&W, TOPUP);
        let fault = if panics { &j.panic_sent } else { &j.fail_sent };
        fault.store(true, Ordering::SeqCst);
        let proved = vec![row(1, 9, R::ChainLocked)];
        src.set_rows(W, proved.clone());
        t.refresh_funding(W).await;
        assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
        until(|| {
            j.bundles.lock().unwrap().iter().any(|b| b.0 == id.0)
                && t.with_j(|i, _| i.fence.evidence_mark(&W, &id))
                    == Some(super::fence::Mark::Ambiguous)
        })
        .await;
        assert!(!sent_on_disk(&j, TOPUP), "panics: {panics}");
        let killed = j.snapshot();

        // In process: the row goes, the step stays Sent, the close writes it.
        src.set_rows(W, Vec::new());
        t.refresh_funding(W).await;
        assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
        fault.store(false, Ordering::SeqCst);
        t.close_journal().await;
        assert!(sent_on_disk(&j, TOPUP), "panics: {panics}");

        // A kill before the retry, and the row gone: fail closed.
        let (t2, _) = reload(&killed, &src).await;
        assert_eq!(t2.funding_status(W, TOPUP), Some(MaybeSent));
        assert_eq!(
            t2.record_self_spend(W, TOPUP, [1; 32]).await,
            Err(F::NoKnownLock)
        );
        // With the row read again, the reopened process writes it.
        src.set_rows(W, proved);
        let reopened = killed.snapshot();
        let (t3, _) = table();
        reopened.load_into(&t3);
        t3.install_tracked(src.clone());
        assert!(t3.refresh_funding(W).await);
        until(|| sent_on_disk(&reopened, TOPUP)).await;
        src.set_rows(W, Vec::new());
        let (t4, _) = reload(&reopened, &src).await;
        assert_eq!(t4.funding_status(W, TOPUP), Some(Sent));
    }
}

// --- A lock's report of a funding step (review R1-02) ----------------------

/// The only flow `report` holds.
fn only_flow(report: &LockReport) -> &FlowReport {
    let [flow] = &report.flows[..] else {
        panic!("{report:?}")
    };
    flow
}

/// A fresh lease of a restarted process bound to a funding step reports
/// it under its step id by its status now: `Sent`, `WillBeSent`,
/// `MaybeSent`; `NotSent` and never started are `Cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_reports_a_resumed_flows_funding_by_its_status() {
    use FlowOutcome as O;
    for (rows, want) in [
        (vec![row(1, 9, R::ChainLocked)], O::Sent),
        (vec![row(1, 9, R::Built)], O::WillBeSent),
        (Vec::new(), O::MaybeSent),
    ] {
        let (t, _, j, src) = setup().await;
        let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
        maybe_sent(&l, TOPUP, 9).await;
        src.set_rows(W, rows);
        let (t2, _) = reload(&j, &src).await;
        let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
        l2.bind_funding_step(TOPUP);
        let report = t2.lock(RevokeCause::Lock, super::tests::status).await;
        let flow = only_flow(&report);
        assert_eq!(flow.artifacts, vec![(TOPUP.to_owned(), want)]);
        assert_eq!(flow.outcome, want);
    }

    // Locked before its first signature: NotSent.
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    p.signatures().note_locked();
    assert_eq!(p.finish(false), O::Cancelled);
    until(|| t.funding_status(W, TOPUP) == Some(NotSent)).await;
    let (t2, _) = reload(&j, &src).await;
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    l2.bind_funding_step(TOPUP);
    l2.bind_funding_step(REG);
    assert_eq!(t2.funding_status(W, REG), None, "never started");
    let report = t2.lock(RevokeCause::Lock, super::tests::status).await;
    let flow = only_flow(&report);
    assert_eq!(
        flow.artifacts,
        vec![
            (TOPUP.to_owned(), O::Cancelled),
            (REG.to_owned(), O::Cancelled)
        ]
    );
    assert_eq!(flow.outcome, O::Cancelled);
}

/// A funding call that failed locally (`MaybeSent`) and whose lock the
/// network later proved is reported `Sent`: the evidence supersedes the
/// call's own outcome, which is not reported beside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_funding_call_proved_later_is_reported_sent() {
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, TOPUP, 9).await;
    src.set_rows(W, vec![row(1, 9, R::ChainLocked)]);
    t.refresh_funding(W).await;
    let report = t.lock(RevokeCause::Lock, super::tests::status).await;
    let flow = only_flow(&report);
    assert_eq!(flow.artifacts, vec![(TOPUP.to_owned(), FlowOutcome::Sent)]);
    assert_eq!(flow.outcome, FlowOutcome::Sent);
}

/// A call the lock found ended definitely unsent, its `NotSent` row lost
/// (owed to the step's next call): the report reads `Cancelled`, not the
/// standing marker's `MaybeSent` (review P2b r1 F4).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_reports_a_cancelled_call_whose_not_sent_row_is_owed() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    p.signatures().note_locked();
    j.fail_resolve.store(true, Ordering::SeqCst);
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    let step = (W, TOPUP.to_owned());
    until(|| t.with_j(|i, _| !i.fence.call_resolving(&step))).await;
    assert_eq!(
        t.funding_status(W, TOPUP),
        Some(MaybeSent),
        "the marker stands"
    );
    let report = t.lock(RevokeCause::Lock, super::tests::status).await;
    let flow = only_flow(&report);
    assert_eq!(
        flow.artifacts,
        vec![(TOPUP.to_owned(), FlowOutcome::Cancelled)]
    );
    assert_eq!(flow.outcome, FlowOutcome::Cancelled);
}

// --- Exact provenance (§2a.6, review P2b r2 R2-01) -------------------------

/// Sol r2's probe, strengthened: a reconstruction of ANOTHER step's lock at
/// index 0 is stored while this call runs and after it signed. It is not
/// the call's lock (the call's own lock, 78, is proved before the wallet
/// closes, but no `Sent` row of it is written). The wallet closes and
/// reopens, and the process restarts, with 78's row lost and the recovery
/// incomplete: the unrelated row never authorizes Repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_unrelated_reconstruction_must_not_authorize_repair() {
    const CURRENT: &str = "registration/44/funding";
    let (t, _, j, src) = setup().await;
    src.set_drafts(W, vec![DraftKey::registration(44, 0)]);
    t.refresh_funding(W).await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(CURRENT, 0))
        .await
        .unwrap();
    p.signatures().note_released();
    let unrelated = row(77, 0, R::Broadcast);
    assert!(!unrelated.trusted);
    src.set_rows(W, vec![unrelated.clone()]);
    t.note_tracked(W, &[(unrelated.txid, unrelated.key)]);
    src.set_rows(W, vec![unrelated.clone(), row(78, 0, R::Built)]);
    p.finish(false);
    t.refresh_funding(W).await;
    src.set_rows(W, vec![unrelated.clone(), row(78, 0, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, CURRENT), Some(Sent));
    assert!(!sent_on_disk(&j, CURRENT));
    t.wallet_closed(W);
    src.set_rows(W, vec![unrelated.clone()]);
    t.wallet_opened(W);
    t.refresh_funding(W).await;
    assert_eq!(
        t.check_stuck_funding(W, CURRENT).err(),
        Some(StuckFundingRefusal::NoKnownLock),
        "a temporal coincidence is not evidence that this is CURRENT's lock"
    );
    let (restarted, _) = reload(&j, &src).await;
    assert_eq!(
        restarted.check_stuck_funding(W, CURRENT).err(),
        Some(StuckFundingRefusal::NoKnownLock)
    );
}

/// Sol r2's probe: the same admission through Repair's recording. A
/// ChainLocked conflict of only the unrelated lock never declares CURRENT
/// definitely unsent, in this process or the next.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_unrelated_conflict_must_not_resolve_current_not_sent() {
    const CURRENT: &str = "registration/44/funding";
    let (t, _, j, src) = setup().await;
    src.set_drafts(W, vec![DraftKey::registration(44, 0)]);
    t.refresh_funding(W).await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(CURRENT, 0))
        .await
        .unwrap();
    p.signatures().note_released();
    let unrelated = row(77, 0, R::Broadcast);
    src.set_rows(W, vec![unrelated.clone()]);
    t.note_tracked(W, &[(unrelated.txid, unrelated.key)]);
    src.set_rows(W, vec![unrelated.clone(), row(78, 0, R::Built)]);
    p.finish(false);
    t.refresh_funding(W).await;
    src.set_rows(W, vec![unrelated.clone(), row(78, 0, R::ChainLocked)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, CURRENT), Some(Sent));
    assert!(!sent_on_disk(&j, CURRENT));
    t.wallet_closed(W);
    src.set_rows(W, vec![unrelated.clone()]);
    t.wallet_opened(W);
    t.refresh_funding(W).await;
    assert_eq!(
        t.record_self_spend(W, CURRENT, [9; 32]).await,
        Err(StuckFundingRefusal::NoKnownLock)
    );
    src.locked.lock().unwrap().insert([9; 32]);
    t.refresh_funding(W).await;
    let (restarted, _) = reload(&j, &src).await;
    assert_ne!(t.funding_status(W, CURRENT), Some(NotSent));
    assert_ne!(restarted.funding_status(W, CURRENT), Some(NotSent));
}

/// Sol r2's probe: an index-0 lock no call was seen to sign, first proved
/// after a restart, is not journaled; with its row lost the step reads
/// `MaybeSent` and Repair refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_index_zero_first_proved_after_restart_fails_closed_on_row_loss() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&l, REG, 0).await;
    let (reopened, _) = reload(&j, &src).await;
    src.set_rows(W, vec![row(1, 0, R::ChainLocked)]);
    reopened.refresh_funding(W).await;
    assert_eq!(reopened.funding_status(W, REG), Some(Sent));
    assert!(!sent_on_disk(&j, REG));
    src.set_rows(W, Vec::new());
    reopened.refresh_funding(W).await;
    assert_eq!(reopened.funding_status(W, REG), Some(MaybeSent));
    assert_eq!(
        reopened.check_stuck_funding(W, REG).err(),
        Some(StuckFundingRefusal::NoKnownLock)
    );
}

/// The converse (the index-0 residual of review P2b r1): the call signed
/// the lock, so its record is on disk. First proved after a restart, it is
/// journaled `Sent`, which stands once its row is lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_0_signed_lock_first_proved_after_a_restart_is_journaled_sent() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    sign_lock(&t, &p, W, &row(1, 0, R::Built));
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(j.load_calls().signed_locks.len(), 1);
    let (reopened, _) = reload(&j, &src).await;
    src.set_rows(W, vec![row(1, 0, R::ChainLocked)]);
    reopened.refresh_funding(W).await;
    assert_eq!(reopened.funding_status(W, REG), Some(Sent));
    until(|| sent_on_disk(&j, REG)).await;
    src.set_rows(W, Vec::new());
    reopened.refresh_funding(W).await;
    assert_eq!(reopened.funding_status(W, REG), Some(Sent));
    assert_eq!(
        reopened.check_stuck_funding(W, REG).err(),
        Some(StuckFundingRefusal::NotStuck(Some(Sent)))
    );
}

/// A signed lock whose row is lost (a restore) still names the inputs the
/// self-spend must conflict with, across a restart; another lock of the
/// step's key that no call of it signed is conflicted with too, but is
/// never what admits Repair.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_locks_inputs_outlive_its_row() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(reopened.funding_status(W, REG), Some(MaybeSent));
    assert_eq!(
        reopened.check_stuck_funding(W, REG).unwrap(),
        vec![(own.txid, own.inputs.clone())]
    );
    let other = row(2, 0, R::Broadcast);
    src.set_rows(W, vec![other.clone()]);
    reopened.refresh_funding(W).await;
    assert_eq!(
        reopened.check_stuck_funding(W, REG).unwrap(),
        vec![(own.txid, own.inputs), (other.txid, other.inputs)]
    );
}

/// A signed lock's write that failed is retried by the call's next store of
/// it, and written once (§2a.6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_locks_failed_write_is_retried_by_the_next_store() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let durable = || {
        t.with_j(|i, _| {
            (i.fence.signed_locks(&W, REG).iter())
                .map(|l| l.durable)
                .collect::<Vec<_>>()
        })
    };
    j.fail_signed_lock.store(true, Ordering::SeqCst);
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert_eq!(durable(), vec![false]);
    assert!(j.signed_lock_rows().is_empty());
    j.fail_signed_lock.store(false, Ordering::SeqCst);
    sign_lock(&t, &p, W, &own);
    sign_lock(&t, &p, W, &own);
    assert_eq!(durable(), vec![true]);
    assert_eq!(j.signed_lock_rows().len(), 1);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(
        reopened.check_stuck_funding(W, REG).unwrap(),
        vec![(own.txid, own.inputs)]
    );
}

/// Every lock one call signed is the step's, durably, and Repair's
/// self-spend must take an input of each (§2a.6).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_lock_a_call_signed_is_recorded() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let (first, second) = (row(1, 0, R::Built), row(2, 0, R::Built));
    sign_lock(&t, &p, W, &first);
    sign_lock(&t, &p, W, &second);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(j.signed_lock_rows().len(), 2);
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(
        reopened.check_stuck_funding(W, REG).unwrap(),
        vec![(first.txid, first.inputs), (second.txid, second.inputs)]
    );
}

/// A lock signed while the journal is closed is the step's in this process
/// only: Repair conflicts with it, nothing is written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_signed_while_the_journal_is_closed_is_kept_in_memory() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    t.close_journal().await;
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert!(j.signed_lock_rows().is_empty());
    let durable = t.with_j(|i, _| {
        (i.fence.signed_locks(&W, REG).iter())
            .map(|l| l.durable)
            .collect::<Vec<_>>()
    });
    assert_eq!(durable, vec![false]);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(
        t.check_stuck_funding(W, REG).unwrap(),
        vec![(own.txid, own.inputs)]
    );
}

/// The step's marker no longer stands on disk when its signed lock is
/// written: nothing is written, and the lock is known in this process
/// only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_lock_whose_marker_no_longer_stands_is_not_written() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    j.signed_lock_unmarked.store(true, Ordering::SeqCst);
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert!(j.signed_lock_rows().is_empty());
    let durable = t.with_j(|i, _| {
        (i.fence.signed_locks(&W, REG).iter())
            .map(|l| l.durable)
            .collect::<Vec<_>>()
    });
    assert_eq!(durable, vec![false]);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
}

/// The signed lock's write fails: it is the step's own in this process
/// (Repair conflicts with it), and after a restart no lock of the step is
/// known (fail closed).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_lock_the_journal_lost_is_known_in_its_process_only() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    j.fail_signed_lock.store(true, Ordering::SeqCst);
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    j.fail_signed_lock.store(false, Ordering::SeqCst);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert!(j.signed_lock_rows().is_empty());
    assert_eq!(
        t.check_stuck_funding(W, REG).unwrap(),
        vec![(own.txid, own.inputs)]
    );
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(
        reopened.check_stuck_funding(W, REG).err(),
        Some(StuckFundingRefusal::NoKnownLock)
    );
}

/// Only a lock whose every input signs a sighash the running call signed is
/// its own: one input short, another call's, or one stored after the call
/// ended is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_is_the_calls_only_when_every_input_carries_its_signature() {
    let (t, _, _, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let pa = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let pb = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    pa.signatures().note_signed([1; 32]);
    pa.signatures().note_signed([3; 32]);
    pb.signatures().note_signed([2; 32]);
    let lock = |n: u8, sighashes: Option<Vec<[u8; 32]>>| StoredLock {
        txid: txid(n),
        inputs: vec![OutPoint::new(txid(100 + n), 0), OutPoint::new(txid(200), 0)],
        sighashes,
    };
    let signed = |step: &str| {
        t.with_j(|i, _| {
            (i.fence.signed_locks(&W, step).iter())
                .map(|l| l.txid)
                .collect::<Vec<_>>()
        })
    };
    t.record_signed_locks(
        W,
        &[
            lock(3, Some(vec![[1; 32], [9; 32]])),
            lock(4, Some(vec![[1; 32], [2; 32]])),
            lock(5, None),
            lock(6, Some(Vec::new())),
            lock(7, Some(vec![[1; 32], [3; 32]])),
        ],
    );
    assert_eq!(signed(REG), vec![txid(7)]);
    assert!(signed(TOPUP).is_empty());
    t.record_signed_locks(W2, &[lock(8, Some(vec![[1; 32]]))]);
    assert_eq!(signed(REG), vec![txid(7)], "another wallet's lock");
    assert_eq!(pa.finish(false), FlowOutcome::MaybeSent);
    t.record_signed_locks(W, &[lock(9, Some(vec![[1; 32]]))]);
    assert_eq!(signed(REG), vec![txid(7)], "after the call ended");
    assert_eq!(pb.finish(false), FlowOutcome::MaybeSent);
}

/// A stored lock's sighashes are recomputed per input from the transaction
/// itself, and read only when each P2PKH scriptSig's signature verifies
/// over its own (either S): a signature copied to another input, or onto
/// a transaction with other outputs, reads none (§2a.6).
#[test]
fn a_stored_locks_signatures_are_read_from_its_p2pkh_script_sigs() {
    use dashcore::blockdata::script::{Builder, PushBytesBuf};
    use dashcore::hashes::Hash;
    use dashcore::secp256k1::{Message, SecretKey, ecdsa};
    let sk = SecretKey::from_secret_bytes([5; 32]).unwrap();
    let pk = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&sk));
    let code = dashcore::ScriptBuf::new_p2pkh(&pk.pubkey_hash());
    let unsigned = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: (0..2)
            .map(|n| dashcore::TxIn {
                previous_output: OutPoint::new(txid(40 + n), 0),
                ..Default::default()
            })
            .collect(),
        output: vec![dashcore::TxOut {
            value: 10_000,
            script_pubkey: dashcore::ScriptBuf::new_op_return(&[]),
        }],
        special_transaction_payload: None,
    };
    let sighash = |tx: &dashcore::Transaction, n: usize| {
        dashcore::sighash::SighashCache::new(tx)
            .legacy_signature_hash(n, &code, 1)
            .unwrap()
            .to_byte_array()
    };
    let sighashes = vec![sighash(&unsigned, 0), sighash(&unsigned, 1)];
    assert_ne!(sighashes[0], sighashes[1], "each input signs its own");
    let sign = |digest: [u8; 32]| ecdsa::sign(Message::from_digest(digest), &sk);
    let script = |sig: &ecdsa::Signature, hash_type: u8| {
        let mut push = sig.serialize_der().to_vec();
        push.push(hash_type);
        Builder::new()
            .push_slice(PushBytesBuf::try_from(push).unwrap())
            .push_slice(pk.inner.serialize())
            .into_script()
    };
    let with = |scripts: [dashcore::ScriptBuf; 2]| {
        let mut tx = unsigned.clone();
        for (input, script_sig) in tx.input.iter_mut().zip(scripts) {
            input.script_sig = script_sig;
        }
        tx
    };
    let (sig0, sig1) = (sign(sighashes[0]), sign(sighashes[1]));
    // Input 1's signature with a high S: (r, n - s).
    let mut compact = sig1.serialize_compact();
    let order = dashcore::secp256k1::constants::CURVE_ORDER;
    let mut borrow = 0i16;
    for i in (32..64).rev() {
        let d = order[i - 32] as i16 - compact[i] as i16 - borrow;
        compact[i] = d.rem_euclid(256) as u8;
        borrow = i16::from(d < 0);
    }
    let high = ecdsa::Signature::from_compact(&compact).unwrap();
    assert_ne!(high, sig1);
    let own = with([script(&sig0, 0x01), script(&sig1, 0x01)]);
    assert_eq!(StoredLock::of(&own).sighashes, Some(sighashes.clone()));
    assert_eq!(StoredLock::of(&own).txid, own.txid());
    let malleated = with([script(&sig0, 0x01), script(&high, 0x01)]);
    assert_eq!(StoredLock::of(&malleated).sighashes, Some(sighashes));
    let mut other_outputs = own.clone();
    other_outputs.output[0].value = 9_000;
    for bad in [
        // Input 0's signature copied onto input 1.
        with([script(&sig0, 0x01), script(&sig0, 0x01)]),
        other_outputs,
        with([script(&sig0, 0x01), script(&sig1, 0x81)]),
        with([
            script(&sig0, 0x01),
            Builder::new()
                .push_slice([0x30, 0x01, 0x01])
                .push_slice(pk.inner.serialize())
                .into_script(),
        ]),
        with([script(&sig0, 0x01), dashcore::ScriptBuf::new()]),
    ] {
        assert_eq!(StoredLock::of(&bad).sighashes, None);
    }
}
