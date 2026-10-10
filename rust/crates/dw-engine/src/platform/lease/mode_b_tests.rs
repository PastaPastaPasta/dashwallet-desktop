//! Mode B's derived funding statuses and its funding gate (E0-04 §2a.5,
//! §2a.3, §2a.4 item 7, §16.5–§16.7), against a fake evidence source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use platform_wallet::AssetLockFundingType;

use super::fence::JournalBackend;
use super::mode_b::{Snapshot, TrackedLocks, derive_funding, index_trusted};
use super::tests::{FakeJournal, Recorder, W, W2, begin, table, with_journal};
use super::*;
use crate::WalletId;
use crate::platform::PlatformError;
use crate::platform::flows::{BudgetPurpose, DispatchResolution, DispatchState};

use CallStepStatus as S;
use DispatchState::{MaybeSent, NotSent, Sent, WillBeSent};
use TrackedStatus as R;

const REG: &str = "registration/7/funding";
const TOPUP: &str = "topup/i/funding";

fn key(identity_index: u32) -> FundingKey {
    FundingKey {
        funding_type: AssetLockFundingType::IdentityRegistration,
        identity_index,
        account: 0,
    }
}

fn funding(step: &str, identity_index: u32) -> CallStep {
    CallStep::Funding {
        step: step.into(),
        key: key(identity_index),
    }
}

fn quote(amount: u64, fee_bound_worst: u64) -> Option<CallQuote> {
    Some(
        FundingQuote {
            amount,
            fee_bound_worst,
        }
        .into(),
    )
}

fn txid(n: u8) -> Txid {
    Txid::from_byte_array([n; 32])
}

fn row(n: u8, identity_index: u32, status: TrackedStatus) -> TrackedRow {
    TrackedRow {
        txid: txid(n),
        key: key(identity_index),
        status,
        inputs: vec![OutPoint::new(txid(100 + n), 0)],
        trusted: index_trusted(&key(identity_index), status),
    }
}

/// The fake library and `app.sqlite`: what `refresh_funding` reads.
#[derive(Default)]
struct FakeTracked {
    rows: Mutex<HashMap<WalletId, Vec<TrackedRow>>>,
    drafts: Mutex<HashMap<WalletId, Vec<DraftKey>>>,
    locked: Mutex<HashSet<[u8; 32]>>,
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

/// A kill now: the next process loads the file as it is (§6.5), with the
/// same library state, and reads the wallet's rows.
async fn reload(j: &FakeJournal, src: &Arc<FakeTracked>) -> (Arc<LeaseTable>, Arc<Recorder>) {
    let snap = j.snapshot();
    let (t, rec) = table();
    let (rows, steps) = snap.load();
    t.load_journal_with(
        Some(snap.clone() as Arc<dyn JournalBackend>),
        rows,
        steps,
        snap.load_calls(),
    );
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    (t, rec)
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
    let marked = S::Marked { durable: true };
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
        for step in [S::None, S::Marked { durable: true }, S::NotSent] {
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
        for step in [
            S::None,
            S::Marked { durable: false },
            S::Marked { durable: true },
            S::NotSent,
        ] {
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
    for step in [S::None, S::NotSent, S::Marked { durable: true }] {
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
/// it (`NotSent`), and then only under a new step.
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
    let (rows, steps) = snap.load();
    t2.load_journal_with(
        Some(snap.clone() as Arc<dyn JournalBackend>),
        rows,
        steps,
        snap.load_calls(),
    );
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
    struct Gated {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    #[async_trait]
    impl TrackedLocks for Gated {
        async fn rows(&self, _: WalletId) -> Option<Vec<TrackedRow>> {
            self.entered.notify_one();
            self.release.notified().await;
            Some(vec![row(1, 7, R::Built)])
        }
        async fn drafts(&self, _: WalletId) -> Option<Vec<DraftKey>> {
            Some(Vec::new())
        }
        async fn chain_locked(&self, _: WalletId, _: &[u8; 32]) -> bool {
            false
        }
    }
    let (t, _) = table();
    let _j = with_journal(&t);
    let src = Arc::new(Gated {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
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
    struct Gated {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    #[async_trait]
    impl TrackedLocks for Gated {
        async fn rows(&self, _: WalletId) -> Option<Vec<TrackedRow>> {
            self.entered.notify_one();
            self.release.notified().await;
            Some(Vec::new())
        }
        async fn drafts(&self, _: WalletId) -> Option<Vec<DraftKey>> {
            Some(Vec::new())
        }
        async fn chain_locked(&self, _: WalletId, _: &[u8; 32]) -> bool {
            false
        }
    }
    let (t, _) = table();
    let _j = with_journal(&t);
    let src = Arc::new(Gated {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
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
    maybe_sent(&l, TOPUP, 9).await;
    src.set_rows(W, vec![row(1, 9, R::Built)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(WillBeSent));
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
