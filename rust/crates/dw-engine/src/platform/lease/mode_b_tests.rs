//! Mode B's derived funding statuses and its funding gate (E0-04 §2a.5,
//! §2a.3, §2a.4 item 7, §16.5–§16.7), against a fake evidence source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use dashcore::{OutPoint, Txid};
use platform_wallet::AssetLockFundingType;

use super::fence::JournalBackend;
use super::mode_b::{
    FundingStepView, FundingStepsUnavailable, Snapshot, TrackedLocks, derive_funding, index_trusted,
};
use super::tests::{
    DURABLE, FakeJournal, REG, Recorder, W, W2, begin, key, quote, row, sign_lock, sign_lost_lock,
    table, txid, until, with_journal,
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

/// The txids of a step's self-spends.
fn spent(info: &CallStepInfo) -> Vec<[u8; 32]> {
    info.self_spends.iter().map(|s| s.txid).collect()
}

/// A conflict for a self-spend row a test downgrades to a schema before 5,
/// which drops it: before then no self-spend recorded one (review Opus O-8a).
const LEGACY_CONFLICT: dw_appdb::dispatch::SelfSpendConflict =
    dw_appdb::dispatch::SelfSpendConflict {
        lock: [0; 32],
        input: dw_appdb::dispatch::StoredOutPoint {
            txid: [0; 32],
            vout: 0,
        },
    };

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

/// What a self-spend taking `lock`'s first input records of it (§2a.6).
fn conflict(lock: &TrackedRow) -> dw_appdb::dispatch::SelfSpendConflict {
    use dashcore::hashes::Hash as _;
    dw_appdb::dispatch::SelfSpendConflict {
        lock: lock.txid.to_byte_array(),
        input: dw_appdb::dispatch::StoredOutPoint {
            txid: lock.inputs[0].txid.to_byte_array(),
            vout: lock.inputs[0].vout,
        },
    }
}

/// A call of `step` that signed `lock` (§2a.6) and ended `MaybeSent`.
async fn signed_maybe_sent(t: &LeaseTable, l: &Lease, step: &str, index: u32, lock: &TrackedRow) {
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(step, index))
        .await
        .unwrap();
    sign_lock(t, &p, W, lock);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
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

/// A funding call of `step` that signed a lock, recorded it and failed,
/// the lock's row gone ([`sign_lost_lock`]): the marker stands, MaybeSent.
async fn maybe_sent(t: &LeaseTable, l: &Lease, step: &str, index: u32) -> Txid {
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(step, index))
        .await
        .unwrap();
    let lock = sign_lost_lock(t, &p, W);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    lock
}

/// A funding call of `step` under a build before schema 7 (OD-02): it
/// released a signature and failed, recording no lock. `j` keeps its marker
/// unflagged, so a reload reads the step `MaybeSent`, no lock known.
async fn legacy_maybe_sent(l: &Lease, j: &FakeJournal, step: &str, index: u32) {
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(step, index))
        .await
        .unwrap();
    p.note_released();
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    j.unflag_markers();
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
    maybe_sent(&t, &l, REG, 7).await;
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
    maybe_sent(&t, &l, TOPUP, 5).await;
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
    maybe_sent(&t, &l, REG, 7).await;
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
    maybe_sent(&t, &l, ADOPTED, 8).await;
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
    let lock = row(3, 9, R::Built);
    signed_maybe_sent(&t, &l, TOPUP, 9, &lock).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    assert!(
        j.insert_self_spend(&W.0, TOPUP, &id.0, &[0xee; 32], &[conflict(&lock)])
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
    p.note_released();
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
    maybe_sent(&t, &l, TOPUP, 8).await;
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
    maybe_sent(&t, &l, REG, 7).await;
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
/// it (`NotSent`), and then only under a new step and only by taking an
/// input of T1, whose signed record is durable before the library stores
/// or sends it (§2a.7; reviews R1-01, P2b r4 R4-01).
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
    let t1 = sign_lost_lock(&t, &p, W);
    // Power loss: the marker and T1's record are durable, the row was
    // never persisted.
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
        t2.record_self_spend(W, REG, [1; 32], &[]).await,
        Err(StuckFundingRefusal::MissesSignedLock(t1)),
        "Repair takes an input of T1"
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
    // The durable unsigned resolution took the marker, and left no
    // tombstone: the step is no standing one, and its retry is a fresh
    // call (DP1-02 StepCancelled).
    assert_eq!(t.call_step(W, REG).funding, None);
    assert!(j.tombstone_rows().is_empty());
    assert!(t.registration_funding_steps(W).unwrap().is_empty());
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    let v = step_view(&t, 7);
    assert_eq!((v.signed, v.dead), (vec![], vec![]));
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
    let lock = row(4, 3, R::Built);
    signed_maybe_sent(&t, &l, ORPHAN, 3, &lock).await;
    let id = ArtifactId::of_step(&W, ORPHAN);
    assert!(
        j.insert_self_spend(&W.0, ORPHAN, &id.0, &[0xaa; 32], &[conflict(&lock)])
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
    // Repair resolved it: its step is not funded again (a call under it
    // could not be told from the orphan; E0-04 H12, DP1-02 C5).
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(ORPHAN, 3))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::StepCancelled)
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
    p.note_released();
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
    sign_lost_lock(&t, &p, W);
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
    maybe_sent(&t, &l, C, 5).await;
    assert_eq!(t.funding_status(W, C), Some(MaybeSent));
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, C), Some(Sent));
}

/// DP1-06: a top-up's step (its marker under an `IdentityTopUp` key) reads
/// only the locks its calls signed, across a restart too: an earlier
/// top-up's `Consumed` row under the same key never answers for it (a
/// step under another key reads it `Sent` after a restart, above).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_top_up_step_reads_only_its_own_signed_locks_across_a_restart() {
    const A: &str = "topup/5-0/funding";
    const B: &str = "topup/5-1/funding";
    let key = FundingKey {
        funding_type: AssetLockFundingType::IdentityTopUp,
        identity_index: 5,
        account: 0,
    };
    let step = |step: &str| CallStep::Funding {
        step: step.into(),
        key,
    };
    let lock = |txid: Txid, status| TrackedRow {
        txid,
        key,
        status,
        trusted: index_trusted(&key, status),
        ..row(0, 5, status)
    };
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();

    // The first top-up's lock went out and was consumed.
    let p = l
        .call_permit_step("funding", quote(10, 0), step(A))
        .await
        .unwrap();
    let a = sign_lost_lock(&t, &p, W);
    assert_eq!(p.finish(true), FlowOutcome::Sent);
    src.set_rows(W, vec![lock(a, R::Consumed)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, A), Some(Sent));

    // The second signed a lock no row shows, and failed.
    let p = l
        .call_permit_step("funding", quote(10, 0), step(B))
        .await
        .unwrap();
    let b = sign_lost_lock(&t, &p, W);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, B), Some(MaybeSent));

    // A restart: the first's row is still not the second's lock.
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, A), Some(Sent));
    assert_eq!(t2.funding_status(W, B), Some(MaybeSent));
    // Its own lock, once tracked, is.
    src.set_rows(W, vec![lock(a, R::Consumed), lock(b, R::Broadcast)]);
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, B), Some(WillBeSent));
    src.set_rows(W, vec![lock(a, R::Consumed), lock(b, R::InstantSendLocked)]);
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, B), Some(Sent));
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
    p.note_released();
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
    p.note_released();
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
    maybe_sent(&t, &l, REG, 7).await;
    maybe_sent(&t, &l, ADOPTED, 8).await;

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
    let lock = row(3, 9, R::Built);
    signed_maybe_sent(&t, &l, TOPUP, 9, &lock).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    assert!(
        j.insert_self_spend(&W.0, TOPUP, &id.0, &[0xee; 32], &[conflict(&lock)])
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
/// index 0. For an unflagged step (§2a.7) at another index, that row is
/// unattributed: the step keeps reading `MaybeSent` (a ChainLocked
/// self-spend proves nothing over it) and Repair refuses. A flagged step's
/// self-spend resolves it all the same (DP1-02 C4). A step at index 0
/// still matches it by key, and a row re-keyed by its identity reads for
/// that identity's step. Each step's call signed a lock whose row is lost
/// (`own3`, `own0`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unattributed_row_blocks_the_self_spends_not_sent_and_repair() {
    use StuckFundingRefusal as F;
    const AT3: &str = "registration/3/funding";
    const AT0: &str = "registration/4/funding";
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let (own3, own0) = (row(5, 3, R::Built), row(6, 0, R::Built));
    signed_maybe_sent(&t, &l, AT3, 3, &own3).await;
    signed_maybe_sent(&t, &l, AT0, 0, &own0).await;
    // A self-spend of each step, recorded before any row came back.
    t.record_self_spend(W, AT3, [0xa3; 32], &own3.inputs)
        .await
        .unwrap();
    t.record_self_spend(W, AT0, [0xa0; 32], &own0.inputs)
        .await
        .unwrap();
    // The same journal written before schema 6 (OD-02): its markers'
    // calls may have sent a lock no record names.
    let legacy = j.snapshot();
    legacy.unflag_markers();

    // A lock that went out, lost its row and was re-added from the chain
    // at index 0: it may be any registration's.
    let lost = row(1, 0, R::RecoveredFromChain);
    assert!(!lost.trusted);
    src.set_rows(W, vec![lost.clone()]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    // A flagged step: the one lock its call signed is recorded, so the row
    // is not its lock, and the ChainLocked self-spend resolves it (DP1-02
    // C4).
    src.locked.lock().unwrap().insert([0xa3; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(NotSent));
    src.locked.lock().unwrap().clear();

    // Unflagged, the row may be its lock.
    let (t, rec) = reload(&legacy, &src).await;
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    assert_eq!(
        t.record_self_spend(W, AT3, [0xb3; 32], &[]).await,
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
    // At index 0 it matches by key; it is not the lock that step's call
    // signed, which the ChainLocked self-spend shows dead.
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
            sighashes: None,
            ..lost
        }],
    );
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, AT3), Some(Sent));
    assert_eq!(t.funding_status(W, AT0), Some(NotSent));
    let of = |step: &str| -> Vec<_> {
        rec.resolved()
            .into_iter()
            .filter(|(s, _)| s == step)
            .map(|(_, r)| r)
            .collect()
    };
    assert_eq!(
        of(AT3),
        vec![DispatchResolution::Sent],
        "the step at index 3 never read NotSent"
    );
    // The index-0 row's `Sent` was never journaled (untrusted, not AT0's
    // signed lock), so it gives way to the self-spend's evidence. (It was
    // the step's first reading after the reload: a baseline, announced by
    // no event.)
    assert_eq!(of(AT0), vec![DispatchResolution::NotSent]);
}

/// A `Built` row is the live pipeline's own build, whatever its index: it
/// leaves another index's ChainLocked self-spend reading `NotSent`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_built_row_at_index_0_is_attributed() {
    const AT3: &str = "registration/3/funding";
    let (t, _, _, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let own = row(5, 3, R::Built);
    signed_maybe_sent(&t, &l, AT3, 3, &own).await;
    t.record_self_spend(W, AT3, [0xa3; 32], &own.inputs)
        .await
        .unwrap();
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
    let own = row(5, 7, R::Built);
    signed_maybe_sent(&t, &l, REG, 7, &own).await;
    t.record_self_spend(W, REG, [0xa7; 32], &own.inputs)
        .await
        .unwrap();
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
            sighashes: None,
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
    maybe_sent(&t, &l, AT0, 0).await;
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

/// With no lock of the step known (a marker of a build before schema 7,
/// OD-02), Repair refuses it, whatever a scan of the chain finds (reviews
/// R1-01, P2b r4 R4-01). In the process that marked it, the marker is
/// flagged, but its call released a signature and recorded no lock: a lock
/// may have been sent unstored (§2a.7, P2b r6 R6-01).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repair_refuses_a_legacy_step_with_no_known_lock() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    legacy_maybe_sent(&l, &j, TOPUP, 11).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    let (t, _) = reload(&j, &src).await;
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    assert_eq!(
        t.record_self_spend(W, TOPUP, [0xed; 32], &[]).await,
        Err(StuckFundingRefusal::NoKnownLock)
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
    let lock = row(3, 9, R::Built);
    signed_maybe_sent(&t, &l, TOPUP, 9, &lock).await;
    let id = ArtifactId::of_step(&W, TOPUP);
    let marker = j
        .log()
        .iter()
        .find(|r| r.artifact == id.0)
        .map(|r| r.seq)
        .unwrap();
    t.record_self_spend(W, TOPUP, [0xee; 32], &lock.inputs)
        .await
        .unwrap();
    assert_eq!(spent(&t.call_step(W, TOPUP)), vec![[0xee; 32]]);
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
    t.record_self_spend(W, TOPUP, [0xef; 32], &lock.inputs)
        .await
        .unwrap();
    assert_eq!(j.load_calls().self_spends.len(), 2);

    // A restart keeps both rows with their marker.
    let (t2, rec2) = reload(&j, &src).await;
    assert_eq!(spent(&t2.call_step(W, TOPUP)), vec![[0xee; 32], [0xef; 32]]);
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
        t2.record_self_spend(W, TOPUP, [0xf0; 32], &lock.inputs)
            .await,
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
    t.record_self_spend(W, TOPUP, [0xee; 32], &[OutPoint::new(txid(101), 0)])
        .await
        .unwrap();
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
        t.record_self_spend(W, REG, [1; 32], &[]).await,
        Err(F::NotStuck(None)),
        "never started"
    );
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap()
        .finish(true);
    assert_eq!(
        t.record_self_spend(W, REG, [1; 32], &[]).await,
        Err(F::NotStuck(Some(Sent)))
    );
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 8))
        .await
        .unwrap();
    assert_eq!(
        t.record_self_spend(W, TOPUP, [1; 32], &[]).await,
        Err(F::CallRunning)
    );
    p.signatures().note_locked();
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, TOPUP) == Some(NotSent)).await;
    assert_eq!(
        t.record_self_spend(W, TOPUP, [1; 32], &[]).await,
        Err(F::NotStuck(Some(NotSent)))
    );

    // A draft's own row with no marker (a lost journal).
    const LOST: &str = "registration/5/funding";
    src.set_drafts(W, vec![DraftKey::registration(5, 5)]);
    src.set_rows(W, vec![row(2, 5, R::Broadcast)]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, LOST), Some(WillBeSent));
    assert_eq!(
        t.record_self_spend(W, LOST, [1; 32], &[]).await,
        Err(F::NoMarker)
    );

    // A failed write records nothing, on disk or in memory.
    const ORPHAN: &str = "topup/o/funding";
    src.set_rows(W, Vec::new());
    t.refresh_funding(W).await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let lock = row(3, 11, R::Built);
    signed_maybe_sent(&t, &l, ORPHAN, 11, &lock).await;
    j.fail_self_spend.store(true, Ordering::SeqCst);
    assert_eq!(
        t.record_self_spend(W, ORPHAN, [1; 32], &lock.inputs).await,
        Err(F::JournalUnavailable)
    );
    assert!(t.call_step(W, ORPHAN).self_spends.is_empty());
    assert!(j.load_calls().self_spends.is_empty());
    j.fail_self_spend.store(false, Ordering::SeqCst);
    t.record_self_spend(W, ORPHAN, [1; 32], &lock.inputs)
        .await
        .unwrap();

    // Rows never read this session.
    let (t2, _) = table();
    let _j2 = with_journal(&t2);
    t2.install_tracked(src.clone());
    assert_eq!(
        t2.record_self_spend(W, ORPHAN, [1; 32], &lock.inputs).await,
        Err(F::RowsUnknown)
    );
    // No journal at all.
    let (t3, _) = table();
    assert_eq!(
        t3.record_self_spend(W, ORPHAN, [1; 32], &lock.inputs).await,
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
/// restart, so Repair refuses it and so does the funding gate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proved_funding_is_journaled_sent_and_outlives_its_row() {
    use StuckFundingRefusal as F;
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    maybe_sent(&t, &l, TOPUP, 9).await;
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
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, TOPUP), Some(Sent));
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, TOPUP), Some(Sent));
    assert_eq!(
        t2.check_stuck_funding(W, TOPUP).err(),
        Some(F::NotStuck(Some(Sent)))
    );
    assert_eq!(
        t2.record_self_spend(W, TOPUP, [1; 32], &[]).await,
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
    legacy_maybe_sent(&l, &j, REG, 0).await;
    let (t, _) = reload(&j, &src).await;
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
    sign_lost_lock(&t, &pa, W);
    sign_lost_lock(&t, &pb, W);
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
    p.note_released();
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
/// gone too, it reads `MaybeSent` and Repair must take the lost lock's
/// input.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_proved_fundings_failed_sent_write_stays_owed() {
    use StuckFundingRefusal as F;
    for panics in [false, true] {
        let (t, _, j, src) = setup().await;
        let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
        let lost = maybe_sent(&t, &l, TOPUP, 9).await;
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
            t2.record_self_spend(W, TOPUP, [1; 32], &[]).await,
            Err(F::MissesSignedLock(lost))
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
        maybe_sent(&t, &l, TOPUP, 9).await;
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
    maybe_sent(&t, &l, TOPUP, 9).await;
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
        Some(NotSent),
        "the marker stands, with no signed record (§2a.7)"
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
/// reopens, and the process restarts, with 78's row lost: the unrelated
/// row never authorizes Repair.
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
    p.note_released();
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
    p.note_released();
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
        t.record_self_spend(W, CURRENT, [9; 32], &[]).await,
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
/// `MaybeSent` and Repair refuses. Since schema 6 a call's lock has a
/// durable record before it can be sent (§2a.7), so this needs a marker
/// from before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_index_zero_first_proved_after_restart_fails_closed_on_row_loss() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    legacy_maybe_sent(&l, &j, REG, 0).await;
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
    assert_eq!(t.signed_lock_txids(W, REG), vec![txid(1)]);
    assert!(t.signed_lock_txids(W, TOPUP).is_empty());
    // The next process, on its own copy of the file.
    let file = j.snapshot();
    let (reopened, _) = table();
    file.load_into(&reopened);
    assert_eq!(reopened.signed_lock_txids(W, REG), vec![txid(1)]);
    reopened.install_tracked(src.clone());
    assert!(reopened.refresh_funding(W).await);
    src.set_rows(W, vec![row(1, 0, R::ChainLocked)]);
    reopened.refresh_funding(W).await;
    assert_eq!(reopened.funding_status(W, REG), Some(Sent));
    until(|| sent_on_disk(&file, REG)).await;
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

/// `f` stops the process (§2a.7: `fail_closed` panics in
/// tests); the reason it gives.
fn fails_closed(f: impl FnOnce()) -> String {
    let e = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("fails closed");
    let why = e.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(why.starts_with("fail closed: "), "{why}");
    why
}

/// A signed lock's failing append is retried before the store returns,
/// and written once (§2a.7). One that keeps failing stops the process
/// inside the store, before the library's send (PA-2; review DP1-02 r8
/// R8-01, whose lost record let a possibly sent funding read unfunded).
/// The restarted process reads the step `MaybeSent`, no lock known: its
/// call released a signature (its release row), and the restart cannot
/// tell this stop from a send no store reached (P2b r6 R6-01; the
/// liveness cost of §2a.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_locks_failing_write_is_retried_then_stops_the_process() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let durable = |t: &LeaseTable| {
        t.with_j(|i, _| {
            (i.fence.signed_locks(&W, REG).iter())
                .map(|l| l.durable)
                .collect::<Vec<_>>()
        })
    };
    j.fail_signed_lock.store(2, Ordering::SeqCst);
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert_eq!(durable(&t), vec![true]);
    sign_lock(&t, &p, W, &own);
    assert_eq!(j.signed_lock_rows().len(), 1);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(reopened.signed_lock_txids(W, REG), vec![own.txid]);
    assert_eq!(reopened.funding_status(W, REG), Some(MaybeSent));

    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    j.fail_signed_lock.store(u64::MAX, Ordering::SeqCst);
    let why = fails_closed(|| sign_lock(&t, &p, W, &own));
    assert!(why.contains("keeps failing"), "{why}");
    // Until the process stops, the pending record keeps the step from
    // reading `NotSent`.
    assert_eq!(durable(&t), vec![false]);
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    let killed = j.snapshot();
    drop(p);
    assert!(killed.signed_lock_rows().is_empty());
    let (restarted, _) = reload(&killed, &src).await;
    assert!(restarted.call_step(W, REG).durable_record);
    assert!(restarted.signed_lock_txids(W, REG).is_empty());
    assert!(restarted.call_step(W, REG).released.is_some());
    assert_eq!(restarted.funding_status(W, REG), Some(MaybeSent));
}

/// One lock per call (O-1): once a lock the call signed is recorded, the
/// call signs no other sighash; the lock's own again (an identical retry)
/// it does. So Repair's self-spend need take an input of the recorded
/// lock only (Opus review O-1's probe: a second lock under one permit,
/// its record lost, let a self-spend of the first read the step
/// `NotSent`). The real signer is `session_tests`'
/// `mode_b_a_funding_signer_signs_one_lock_per_call`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_signs_no_second_lock_once_its_lock_is_recorded() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let first = row(1, 0, R::Built);
    let second_sighash = [2; 32];
    assert!(
        p.signatures().admits(second_sighash),
        "nothing recorded yet"
    );
    sign_lock(&t, &p, W, &first);
    assert!(!p.signatures().closed());
    assert!(!p.signatures().admits(second_sighash), "a second lock");
    assert!(p.signatures().admits([1; 32]), "the first lock's again");
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert_eq!(j.signed_lock_rows().len(), 1);
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(
        reopened.check_stuck_funding(W, REG).unwrap(),
        vec![(first.txid, first.inputs)]
    );
}

/// A lock signed while the journal is not ready cannot be recorded
/// durably: the process stops before the library's send, and nothing is
/// written (§2a.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lock_signed_while_the_journal_is_closed_stops_the_process() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    // The call's signer recorded its release before the close: one signing
    // after it is refused at the signer (R6-01); this is the store's own
    // check behind it.
    t.record_release(W, REG).await.unwrap();
    t.close_journal().await;
    let why = fails_closed(|| sign_lock(&t, &p, W, &row(1, 0, R::Built)));
    assert!(why.contains("not ready"), "{why}");
    assert!(j.signed_lock_rows().is_empty());
}

/// The step's marker no longer stands when its signed lock is recorded
/// (in memory, or on disk at the append): the process stops, nothing is
/// written (§2a.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signed_lock_whose_marker_no_longer_stands_stops_the_process() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    j.signed_lock_unmarked.store(true, Ordering::SeqCst);
    let why = fails_closed(|| sign_lock(&t, &p, W, &row(1, 0, R::Built)));
    assert!(why.contains("no longer stands on disk"), "{why}");
    assert!(j.signed_lock_rows().is_empty());
}

/// Only a lock whose every input signs a sighash a running call signed is
/// its own (§2a.6). One that may carry a call's signature without being
/// provably its own stops the process before its send (§2a.7): one with
/// only some of a call's signatures, a `Built` one whose signatures cannot
/// be read while a call that released one runs, and one of a call that
/// ended reaching the store unrecorded. Another call's lock, another
/// wallet's, or an unreadable one not `Built` is not the call's.
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
    pa.note_signed([1; 32]);
    pa.note_signed([3; 32]);
    pb.note_signed([2; 32]);
    let lock = |n: u8, sighashes: Option<Vec<[u8; 32]>>, built: bool| StoredLock {
        txid: txid(n),
        inputs: vec![OutPoint::new(txid(100 + n), 0), OutPoint::new(txid(200), 0)],
        sighashes,
        built,
    };
    let signed = |step: &str| {
        t.with_j(|i, _| {
            (i.fence.signed_locks(&W, step).iter())
                .map(|l| l.txid)
                .collect::<Vec<_>>()
        })
    };
    let record = |l: StoredLock| t.record_signed_locks(W, &[l]);
    for (partial, n) in [(vec![[1; 32], [9; 32]], 3), (vec![[1; 32], [2; 32]], 4)] {
        let why = fails_closed(|| record(lock(n, Some(partial), true)));
        assert!(why.contains("only some"), "{why}");
    }
    let why = fails_closed(|| record(lock(5, None, true)));
    assert!(why.contains("unverifiable built"), "{why}");
    record(lock(5, None, false));
    record(lock(6, Some(Vec::new()), true));
    record(lock(7, Some(vec![[1; 32], [3; 32]]), true));
    assert_eq!(signed(REG), vec![txid(7)]);
    assert!(signed(TOPUP).is_empty());
    t.record_signed_locks(W2, &[lock(8, Some(vec![[1; 32]]), true)]);
    assert_eq!(signed(REG), vec![txid(7)], "another wallet's lock");
    assert_eq!(pa.finish(false), FlowOutcome::MaybeSent);
    // The ended call's recorded lock stored again; then a lock with its
    // signature that was never recorded.
    record(lock(7, Some(vec![[1; 32], [3; 32]]), false));
    let why = fails_closed(|| record(lock(9, Some(vec![[1; 32]]), true)));
    assert!(why.contains("ended funding call"), "{why}");
    assert_eq!(signed(REG), vec![txid(7)]);
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

/// Sol r3's probe, on a real journal file (review P2b r3 R3-01): a schema 3
/// journal holds a self-spend the old engine admitted on heuristic
/// ownership, conflicting only with an unrelated lock B, while the step's
/// own lock A paid and lost its row. Upgraded, that self-spend is legacy:
/// ChainLocked, it must not read the step `NotSent`, and nothing later
/// lets it (review P2b r4 R4-01).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r3_schema3_heuristic_repair_must_not_survive_as_negative_evidence() {
    use dw_appdb::dispatch::{DispatchJournal, JournalOpen};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.sqlite");
    let artifact = ArtifactId::of_step(&W, REG);
    {
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 1).unwrap() else {
            panic!("schema")
        };
        j.insert_funding_step(&W.0, REG, &artifact.0, &key(0).stored(), 2)
            .unwrap();
        // The old engine admitted this using unrelated B's heuristic
        // ownership.
        j.insert_self_spend(&W.0, REG, &artifact.0, &[9; 32], &[LEGACY_CONFLICT], 3)
            .unwrap();
        drop(j);
        // Back to schema 3's layout: no provenance, no `signed_lock`, no
        // `durable_record`.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "ALTER TABLE self_spend DROP COLUMN conflicts;
             ALTER TABLE self_spend DROP COLUMN provenance;
             ALTER TABLE funding_marker DROP COLUMN durable_record;
             DROP TABLE funding_tombstone;
             DROP TABLE funding_release;
             DROP TABLE top_up_settled;
             DROP TABLE signed_lock;
             UPDATE meta SET v = 3 WHERE k = 'schema'",
        )
        .unwrap();
    }
    let JournalOpen::Ready(j) = DispatchJournal::open(&path, 4).unwrap() else {
        panic!("migration")
    };
    let calls = j.load_calls().unwrap();
    assert_eq!(calls.self_spends.len(), 1);
    assert!(calls.signed_locks.is_empty());
    let (rows, steps) = j.load().unwrap();
    let (t, _) = table();
    t.load_journal_with(Some(Arc::new(j)), rows, steps, calls);
    let src = Arc::new(FakeTracked::default());
    src.set_drafts(W, vec![DraftKey::registration(7, 0)]);
    // A was proved/paid then lost its tracked row; B remains, and the
    // self-spend conflicting only with B is ChainLocked.
    src.set_rows(W, vec![row(77, 0, R::Broadcast)]);
    src.locked.lock().unwrap().insert([9; 32]);
    t.install_tracked(src.clone());
    t.refresh_funding(W).await;
    let status = t.funding_status(W, REG);
    assert_ne!(
        status,
        Some(NotSent),
        "legacy heuristic ownership is not proof of a conflict with A"
    );
    assert_eq!(status, Some(WillBeSent));
    // Nor once B's row is gone: the step's marker stands with no row.
    src.set_rows(W, Vec::new());
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
}

/// On a fresh journal, a ChainLocked self-spend reads the step `NotSent`
/// only while its record names an input of every signed lock of the step
/// (review P2b r3 R3-01): one naming another lock or input shows nothing;
/// the step reads `MaybeSent`. One with no conflicts is refused by the
/// journal (review Opus O-8a; the engine's guard against such a row is
/// `an_empty_recorded_self_spend_without_a_signed_lock_never_reads_not_sent`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_spend_without_its_conflict_evidence_never_reads_not_sent() {
    use dashcore::hashes::Hash as _;
    use dw_appdb::dispatch::{DispatchJournal, JournalOpen, SelfSpendConflict, StoredOutPoint};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.sqlite");
    let artifact = ArtifactId::of_step(&W, REG);
    let lock = txid(1).to_byte_array();
    let input = StoredOutPoint {
        txid: txid(101).to_byte_array(),
        vout: 0,
    };
    let other = StoredOutPoint { vout: 1, ..input };
    let JournalOpen::Ready(j) = DispatchJournal::open(&path, 1).unwrap() else {
        panic!("schema")
    };
    j.insert_funding_step(&W.0, REG, &artifact.0, &key(0).stored(), 2)
        .unwrap();
    assert!(
        j.insert_signed_lock(&W.0, REG, &artifact.0, &lock, &[input], 3)
            .unwrap()
    );
    assert!(
        j.insert_self_spend(&W.0, REG, &artifact.0, &[0xa1; 32], &[], 4)
            .is_err()
    );
    let spends: [(u8, Vec<SelfSpendConflict>); 2] = [
        (
            0xa2,
            vec![SelfSpendConflict {
                lock: [2; 32],
                input,
            }],
        ),
        (0xa3, vec![SelfSpendConflict { lock, input: other }]),
    ];
    for (n, conflicts) in &spends {
        assert!(
            j.insert_self_spend(&W.0, REG, &artifact.0, &[*n; 32], conflicts, 4)
                .unwrap()
        );
    }
    let open = |j: DispatchJournal| {
        let calls = j.load_calls().unwrap();
        let (rows, steps) = j.load().unwrap();
        let (t, _) = table();
        t.load_journal_with(Some(Arc::new(j)), rows, steps, calls);
        t
    };
    let t = open(j);
    let src = Arc::new(FakeTracked::default());
    for (n, _) in &spends {
        src.locked.lock().unwrap().insert([*n; 32]);
    }
    t.install_tracked(src.clone());
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    // The record that shows the conflict.
    let JournalOpen::Ready(j) = DispatchJournal::open(&path, 5).unwrap() else {
        panic!("schema")
    };
    let conflict = SelfSpendConflict { lock, input };
    assert!(
        j.insert_self_spend(&W.0, REG, &artifact.0, &[0xa4; 32], &[conflict], 6)
            .unwrap()
    );
    src.locked.lock().unwrap().insert([0xa4; 32]);
    let t = open(j);
    t.install_tracked(src.clone());
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
}

/// Repair records the input its self-spend takes of each signed lock of
/// the step (review P2b r3 R3-01), and refuses a self-spend that misses
/// one; the record survives a restart, so the ChainLocked self-spend reads
/// the step `NotSent` there too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_spend_records_the_input_it_takes_of_each_signed_lock() {
    use StuckFundingRefusal as F;
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let own = row(1, 0, R::Built);
    sign_lock(&t, &p, W, &own);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    src.set_rows(W, vec![own.clone()]);
    t.refresh_funding(W).await;
    assert_eq!(
        t.record_self_spend(W, REG, [0xee; 32], &[OutPoint::new(txid(9), 0)])
            .await,
        Err(F::MissesSignedLock(own.txid))
    );
    assert!(t.call_step(W, REG).self_spends.is_empty());
    let inputs = [OutPoint::new(txid(9), 0), own.inputs[0]];
    t.record_self_spend(W, REG, [0xee; 32], &inputs)
        .await
        .unwrap();
    assert_eq!(
        t.call_step(W, REG).self_spends,
        vec![SelfSpend {
            txid: [0xee; 32],
            conflicts: Some(vec![(own.txid, own.inputs[0])]),
        }]
    );
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    src.set_rows(W, Vec::new());
    let (reopened, _) = reload(&j, &src).await;
    assert!(
        reopened.call_step(W, REG).self_spends[0]
            .conflicts
            .is_some()
    );
    assert_eq!(reopened.funding_status(W, REG), Some(NotSent));
}

/// A ChainLocked self-spend's record shows the conflict only when it names
/// an input of every signed lock of the step: one covering some of them,
/// or none, shows nothing (reviews P2b r3 R3-01, r4 R4-01).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_self_spend_shows_the_conflict_only_over_every_signed_lock() {
    use dashcore::hashes::Hash as _;
    use dw_appdb::dispatch::{SelfSpendConflict, StoredOutPoint};
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    let (a, b) = (row(1, 0, R::Built), row(2, 0, R::Built));
    sign_lock(&t, &p, W, &a);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    // A second signed lock of the step (one call signs one, R6-02; a
    // journal of an earlier build may hold two), on disk.
    let id = ArtifactId::of_step(&W, REG);
    let inputs = [dw_appdb::dispatch::StoredOutPoint {
        txid: b.inputs[0].txid.to_byte_array(),
        vout: b.inputs[0].vout,
    }];
    assert!(
        j.insert_signed_lock(&W.0, REG, &id.0, &b.txid.to_byte_array(), &inputs)
            .unwrap()
    );
    let covers = |r: &TrackedRow| SelfSpendConflict {
        lock: r.txid.to_byte_array(),
        input: StoredOutPoint {
            txid: r.inputs[0].txid.to_byte_array(),
            vout: r.inputs[0].vout,
        },
    };
    assert!(
        j.insert_self_spend(&W.0, REG, &id.0, &[0xa1; 32], &[covers(&a)])
            .unwrap()
    );
    j.push_empty_self_spend(W, REG, [0xa2; 32]);
    src.locked.lock().unwrap().extend([[0xa1; 32], [0xa2; 32]]);
    let (t, _) = reload(&j, &src).await;
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    src.set_rows(W, vec![a.clone()]);
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(WillBeSent));
    // A record over both locks shows it.
    src.set_rows(W, Vec::new());
    assert!(
        j.insert_self_spend(&W.0, REG, &id.0, &[0xa3; 32], &[covers(&a), covers(&b)])
            .unwrap()
    );
    src.locked.lock().unwrap().insert([0xa3; 32]);
    let (t, _) = reload(&j, &src).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
}

/// Sol r4's probe (review P2b r4 R4-01): a schema 3 journal holds a legacy
/// self-spend S of an unrelated lock's input, while the step's own lock A,
/// signed and held by a peer, spends another coin (`withheld`; A itself is
/// never built here: the journal knows nothing of it) and has no row. Upgraded,
/// the wallet's rows are read empty, as a complete scan of the chain reads
/// them (A is not on it), before anything else: ChainLocked, S never reads
/// the step `NotSent`, read after read, and Repair refuses it, as no lock
/// of the step is known.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r4_legacy_self_spend_and_complete_chain_scan_do_not_kill_a_withheld_lock() {
    use dashcore::blockdata::script::{Builder, PushBytesBuf};
    use dashcore::hashes::Hash as _;
    use dashcore::secp256k1::{Message, SecretKey, ecdsa};
    use dw_appdb::dispatch::{DispatchJournal, JournalOpen, SelfSpendProvenance};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.sqlite");
    let id = ArtifactId::of_step(&W, REG);
    // A spends `withheld`; S spends only `unrelated`.
    let (withheld, unrelated) = (OutPoint::new(txid(78), 0), OutPoint::new(txid(77), 0));
    let sk = SecretKey::from_secret_bytes([23; 32]).unwrap();
    let pk = dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&sk));
    let code = dashcore::ScriptBuf::new_p2pkh(&pk.pubkey_hash());
    let mut s = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: unrelated,
            ..Default::default()
        }],
        output: vec![dashcore::TxOut {
            value: 10_000,
            script_pubkey: code.clone(),
        }],
        special_transaction_payload: None,
    };
    let digest = dashcore::sighash::SighashCache::new(&s)
        .legacy_signature_hash(0, &code, 1)
        .unwrap()
        .to_byte_array();
    let mut sig = ecdsa::sign(Message::from_digest(digest), &sk)
        .serialize_der()
        .to_vec();
    sig.push(1);
    s.input[0].script_sig = Builder::new()
        .push_slice(PushBytesBuf::try_from(sig).unwrap())
        .push_slice(pk.inner.serialize())
        .into_script();
    assert!(StoredLock::of(&s).sighashes.is_some());
    assert!(!s.input.iter().any(|i| i.previous_output == withheld));
    let s_id = ArtifactId::from_txid(&s.txid()).0;
    {
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 1).unwrap() else {
            panic!("schema")
        };
        j.insert_funding_step(&W.0, REG, &id.0, &key(0).stored(), 2)
            .unwrap();
        j.insert_self_spend(&W.0, REG, &id.0, &s_id, &[LEGACY_CONFLICT], 3)
            .unwrap();
        drop(j);
        // Back to schema 3's layout: no provenance, no `signed_lock`, no
        // `durable_record`.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "ALTER TABLE self_spend DROP COLUMN conflicts;
             ALTER TABLE self_spend DROP COLUMN provenance;
             ALTER TABLE funding_marker DROP COLUMN durable_record;
             DROP TABLE funding_tombstone;
             DROP TABLE funding_release;
             DROP TABLE top_up_settled;
             DROP TABLE signed_lock;
             UPDATE meta SET v = 3 WHERE k = 'schema'",
        )
        .unwrap();
    }
    let JournalOpen::Ready(j) = DispatchJournal::open(&path, 4).unwrap() else {
        panic!("migration")
    };
    let calls = j.load_calls().unwrap();
    assert_eq!(calls.self_spends[0].provenance, SelfSpendProvenance::Legacy);
    assert!(calls.signed_locks.is_empty());
    let (rows, steps) = j.load().unwrap();
    let (t, _) = table();
    t.load_journal_with(Some(Arc::new(j)), rows, steps, calls);
    let src = Arc::new(FakeTracked::default());
    src.set_drafts(W, vec![DraftKey::registration(7, 0)]);
    // The empty snapshot is established first, S already ChainLocked.
    src.set_rows(W, Vec::new());
    src.locked.lock().unwrap().insert(s_id);
    t.install_tracked(src.clone());
    for _ in 0..2 {
        assert!(t.refresh_funding(W).await);
        assert_eq!(
            t.funding_status(W, REG),
            Some(MaybeSent),
            "chain absence and an unrelated legacy self-spend cannot prove A unsendable"
        );
    }
    assert_eq!(
        t.record_self_spend(W, REG, [0xa5; 32], &[withheld]).await,
        Err(StuckFundingRefusal::NoKnownLock)
    );
}

/// A legacy self-spend of a step whose call's lock is recorded (schema 4)
/// shows nothing (review P2b r4 R4-01): the way out is a fresh Repair,
/// recorded with the input it takes of that lock. Its row is new, beside
/// the legacy one, and resolves the step once ChainLocked, across a
/// restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_step_resolves_only_by_a_fresh_recorded_self_spend() {
    use dashcore::hashes::Hash as _;
    use dw_appdb::dispatch::{DispatchJournal, JournalOpen, SelfSpendProvenance, StoredOutPoint};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dispatch.sqlite");
    let id = ArtifactId::of_step(&W, REG);
    let own = row(5, 0, R::Built);
    {
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 1).unwrap() else {
            panic!("schema")
        };
        j.insert_funding_step(&W.0, REG, &id.0, &key(0).stored(), 2)
            .unwrap();
        let input = StoredOutPoint {
            txid: own.inputs[0].txid.to_byte_array(),
            vout: own.inputs[0].vout,
        };
        j.insert_signed_lock(&W.0, REG, &id.0, &own.txid.to_byte_array(), &[input], 3)
            .unwrap();
        j.insert_self_spend(&W.0, REG, &id.0, &[0xa1; 32], &[LEGACY_CONFLICT], 4)
            .unwrap();
        drop(j);
        // Back to schema 4's layout: no provenance, no `durable_record`.
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "ALTER TABLE self_spend DROP COLUMN conflicts;
             ALTER TABLE self_spend DROP COLUMN provenance;
             ALTER TABLE funding_marker DROP COLUMN durable_record;
             DROP TABLE funding_tombstone;
             DROP TABLE funding_release;
             DROP TABLE top_up_settled;
             UPDATE meta SET v = 4 WHERE k = 'schema'",
        )
        .unwrap();
    }
    let open = || {
        let JournalOpen::Ready(j) = DispatchJournal::open(&path, 4).unwrap() else {
            panic!("migration")
        };
        let calls = j.load_calls().unwrap();
        let (rows, steps) = j.load().unwrap();
        let (t, _) = table();
        t.load_journal_with(Some(Arc::new(j)), rows, steps, calls);
        t
    };
    let t = open();
    let src = Arc::new(FakeTracked::default());
    src.set_drafts(W, vec![DraftKey::registration(7, 0)]);
    src.locked.lock().unwrap().insert([0xa1; 32]);
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent));
    t.record_self_spend(W, REG, [0xa2; 32], &own.inputs)
        .await
        .unwrap();
    assert_eq!(
        t.funding_status(W, REG),
        Some(MaybeSent),
        "not ChainLocked yet"
    );
    src.locked.lock().unwrap().insert([0xa2; 32]);
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    drop(t);
    let t = open();
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    let JournalOpen::Ready(j) = DispatchJournal::open(&path, 4).unwrap() else {
        panic!("schema")
    };
    let spends = j.load_calls().unwrap().self_spends;
    assert_eq!(spends.len(), 2);
    assert_eq!(
        (spends[0].txid, &spends[0].provenance),
        ([0xa1; 32], &SelfSpendProvenance::Legacy)
    );
    assert_eq!(spends[1].txid, [0xa2; 32]);
    assert!(matches!(&spends[1].provenance, SelfSpendProvenance::Conflicts(c) if c.len() == 1));
}

// --- Repair's resolution and DP1-02's view (ownership rev2 §9, §13.1) -----

/// What [`LeaseTable::registration_funding_steps`] shows of `draft`.
fn step_view(t: &LeaseTable, draft: i64) -> FundingStepView {
    (t.registration_funding_steps(W).unwrap().into_iter())
        .find(|v| v.draft == draft)
        .unwrap()
}

/// DP1-02's DEC-197 and DEC-198 am.1 read a step's release time in its own
/// view: J keeps each `funding_release` row's `at` beside its step, as the
/// release is recorded and as it loads again after a kill. So each step
/// shows its own time, never another step's; a step with no release shows
/// none; and a second record under the same marker keeps the first time,
/// as the row on disk does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_registration_step_shows_its_own_release_time() {
    const AT3: &str = "registration/3/funding";
    const AT5: &str = "registration/5/funding";
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    for (step, index, at) in [(AT3, 3, 1_000), (REG, 7, 7_000)] {
        let p = l
            .call_permit_step("funding", quote(10, 0), funding(step, index))
            .await
            .unwrap();
        t.record_release_at(W, step, at).unwrap();
        p.note_released();
        assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    }
    // A call that has released nothing yet.
    let running = l
        .call_permit_step("funding", quote(10, 0), funding(AT5, 5))
        .await
        .unwrap();
    let times = |t: &LeaseTable| {
        (t.registration_funding_steps(W).unwrap().into_iter())
            .map(|v| (v.draft, v.released_at))
            .collect::<Vec<_>>()
    };
    let expected = vec![(3, Some(1_000)), (5, None), (7, Some(7_000))];
    assert_eq!(times(&t), expected);
    assert_eq!(
        (j.release_rows().iter())
            .map(|r| (r.step_id.as_str(), r.at))
            .collect::<Vec<_>>(),
        vec![(AT3, 1_000), (REG, 7_000)],
        "J's time is the row's"
    );
    t.record_release_at(W, AT3, 9_000).unwrap();
    assert_eq!(times(&t), expected, "the marker's first release stands");
    let (reopened, _) = reload(&j, &src).await;
    assert_eq!(times(&reopened), expected, "loaded from each row's `at`");
    drop(running);
}

/// DP1-02 f1 (Sol r9 residual 2): a release write begun under one marker
/// that lands only after its call was cancelled (`NotSent` superseded the
/// marker, the signing dropped before any signature) and the step was
/// marked again answers no marker. The next call, which released nothing,
/// shows no release, in memory and after a restart, where its flagged
/// marker reads `NotSent` (C5), not `MaybeSent` with an hour started early.
/// Its own release then names its own marker and is written as usual.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_late_release_write_never_answers_the_next_marker() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let first = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    let (began, release) = j.hold_release();
    let record = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.record_release(W, REG).await }
    });
    tokio::task::spawn_blocking(move || began.recv_timeout(Duration::from_secs(60)))
        .await
        .unwrap()
        .expect("the release write began under the first marker");
    assert_eq!(first.finish(false), FlowOutcome::Cancelled);
    let second = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    release.send(()).unwrap();
    assert!(
        record.await.unwrap().is_err(),
        "the superseded marker's write"
    );
    assert!(j.release_rows().is_empty(), "no row answers the new marker");
    let step = |t: &LeaseTable| {
        (t.registration_funding_steps(W).unwrap().into_iter())
            .find(|v| v.draft == 7)
            .unwrap()
    };
    assert_eq!(step(&t).released_at, None);
    let (reopened, _) = reload(&j, &src).await;
    let reloaded = step(&reopened);
    assert_eq!(
        (reloaded.released_at, reloaded.status),
        (None, Some(NotSent))
    );
    t.record_release(W, REG).await.unwrap();
    let rows = j.release_rows();
    let standing = j.funding_rows().last().unwrap().seq;
    assert_eq!(
        rows.iter().map(|r| r.marker).collect::<Vec<_>>(),
        vec![standing],
        "the second marker's own release"
    );
    assert_eq!(
        step(&t).released_at,
        Some(rows[0].at),
        "J's time is the row's"
    );
    second.note_released();
    assert_eq!(second.finish(false), FlowOutcome::MaybeSent);
}

/// Sol f1 r1: a release that committed under the first marker but answers
/// only after the step went `NotSent`, the wallet's rows were erased and
/// it was opened again with a new marker of the same step answers that
/// marker neither in memory nor on disk; the new call's release then
/// writes its own row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_release_answering_after_erase_never_answers_the_new_marker() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let first = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    let old = j.funding_rows().last().unwrap().seq;
    let (stored, release) = j.hold_release_after();
    let record = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.record_release(W, REG).await }
    });
    tokio::task::spawn_blocking(move || stored.recv_timeout(Duration::from_secs(60)))
        .await
        .unwrap()
        .expect("the release row stored under the first marker");
    let markers = |j: &FakeJournal| {
        j.release_rows()
            .iter()
            .map(|r| r.marker)
            .collect::<Vec<_>>()
    };
    assert_eq!(markers(&j), vec![old]);
    assert_eq!(first.finish(false), FlowOutcome::Cancelled);
    t.erase_wallet_rows(W).await;
    assert!(j.release_rows().is_empty(), "compacted with its marker");
    t.wallet_opened(W);
    assert!(t.refresh_funding(W).await);
    let second = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    let new = j.funding_rows().last().unwrap().seq;
    assert_ne!(new, old);
    release.send(()).unwrap();
    assert!(record.await.unwrap().is_err(), "the first marker's answer");
    let step = |t: &LeaseTable| {
        (t.registration_funding_steps(W).unwrap().into_iter())
            .find(|v| v.draft == 7)
            .unwrap()
    };
    assert_eq!(step(&t).released_at, None, "no time for the new marker");
    assert!(j.release_rows().is_empty(), "no row for the new marker");
    t.record_release(W, REG).await.unwrap();
    assert_eq!(markers(&j), vec![new], "the new marker's own release");
    assert_eq!(step(&t).released_at, Some(j.release_rows()[0].at));
    second.note_released();
    assert_eq!(second.finish(false), FlowOutcome::MaybeSent);
}

/// Repair's resolution of a flagged step is durable (DP1-02 PA-3, review
/// OD-06): its tombstone outlives the self-spend's ChainLock in the
/// snapshot, the wallet's erase and a restart. The step reads `NotSent`
/// over any row, its own key's included (C4); its signed lock is dead in
/// the PA-1 view; no call of it starts again (`StepCancelled`, E0-04 H12).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repaired_step_stays_resolved_across_its_erase_and_a_restart() {
    const AT3: &str = "registration/3/funding";
    let (t, rec, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let (lock, own3) = (row(5, 7, R::Built), row(6, 3, R::Built));
    signed_maybe_sent(&t, &l, REG, 7, &lock).await;
    signed_maybe_sent(&t, &l, AT3, 3, &own3).await;
    signed_maybe_sent(&t, &l, TOPUP, 9, &row(8, 9, R::Built)).await;
    let view = |draft, key, signed| FundingStepView {
        draft,
        key,
        status: Some(MaybeSent),
        signed,
        dead: Vec::new(),
        durable_record: true,
        recording: false,
        // Released when signed; its time is checked by
        // `each_registration_step_shows_its_own_release_time`.
        released_at: Some(0),
    };
    assert_eq!(
        (t.registration_funding_steps(W).unwrap().into_iter())
            .map(|v| FundingStepView {
                released_at: v.released_at.map(|_| 0),
                ..v
            })
            .collect::<Vec<_>>(),
        vec![
            view(3, key(3), vec![own3.txid]),
            view(7, key(7), vec![lock.txid])
        ],
        "by draft; the top-up is no registration"
    );

    t.record_self_spend(W, REG, [0xee; 32], &lock.inputs)
        .await
        .unwrap();
    t.refresh_funding(W).await;
    assert!(j.tombstone_rows().is_empty(), "not ChainLocked yet");
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    until(|| j.tombstone_rows().len() == 1).await;
    let stone = &j.tombstone_rows()[0];
    assert_eq!(
        (stone.step_id.as_str(), stone.self_spend),
        (REG, [0xee; 32])
    );
    assert_eq!(
        rec.resolved(),
        vec![(REG.to_owned(), DispatchResolution::NotSent)],
        "announced once"
    );
    let v = step_view(&t, 7);
    assert_eq!(
        (v.status, v.signed, v.dead, v.durable_record),
        (Some(NotSent), vec![], vec![lock.txid], true)
    );
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::StepCancelled)
    );
    assert_eq!(
        t.record_self_spend(W, REG, [0xef; 32], &lock.inputs).await,
        Err(StuckFundingRefusal::NotStuck(Some(NotSent)))
    );
    // Nothing supersedes its marker: a call of the step under another key,
    // or one disguised as resumable, is refused before any J write (E0-04
    // F7).
    let log = j.log().len();
    for (step, refusal) in [
        (
            funding(REG, 8),
            LeaseError::FundingBlocked(FundingBlock::StepCancelled),
        ),
        (CallStep::Resumable(REG.into()), LeaseError::StepKeyMismatch),
    ] {
        assert_eq!(
            l.call_permit_step("funding", quote(10, 0), step)
                .await
                .unwrap_err(),
            refusal
        );
    }
    assert_eq!(j.log().len(), log);

    // The wallet's rows erased (§8.6): its marker stands, so its tombstone
    // does too; the wallet added again (a re-add, a backup restored: they
    // write no J row) reads it so.
    t.erase_wallet_rows(W).await;
    assert_eq!(j.tombstone_rows().len(), 1);
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    t.wallet_opened(W);
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    assert_eq!(step_view(&t, 7).dead, vec![lock.txid]);

    // A restart whose snapshot no longer shows the ChainLock, with an
    // unattributed row and a pending one under the step's own key: no row
    // is its lock (C4), and the resolution is never retracted.
    src.locked.lock().unwrap().clear();
    src.set_rows(
        W,
        vec![row(1, 0, R::RecoveredFromChain), row(2, 7, R::Broadcast)],
    );
    let (t2, rec2) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, REG), Some(NotSent));
    assert_eq!(t2.funding_status(W, AT3), Some(MaybeSent));
    let v = step_view(&t2, 7);
    assert_eq!(
        (v.status, v.signed, v.dead),
        (Some(NotSent), vec![], vec![lock.txid])
    );
    assert_eq!(step_view(&t2, 3).signed, vec![own3.txid]);
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::StepCancelled)
    );
    t2.refresh_funding(W).await;
    assert!(
        !rec2.resolved().iter().any(|(s, _)| s == REG),
        "nothing moves it: {:?}",
        rec2.resolved()
    );
    assert_eq!(j.snapshot().tombstone_rows().len(), 1);
}

/// A tombstone whose write fails stays owed: the step reads `NotSent` and
/// refuses its call meanwhile, though a later snapshot no longer shows the
/// ChainLock (a ChainLock is final), and the next refresh writes it (PA-3,
/// review P2b r5 validator).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_tombstone_write_stays_owed() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let lock = row(5, 7, R::Built);
    signed_maybe_sent(&t, &l, REG, 7, &lock).await;
    t.record_self_spend(W, REG, [0xee; 32], &lock.inputs)
        .await
        .unwrap();
    j.fail_tombstone.store(true, Ordering::SeqCst);
    src.locked.lock().unwrap().insert([0xee; 32]);
    t.refresh_funding(W).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    until(|| t.with_j(|i, _| i.fence.tombstoning.is_empty())).await;
    assert!(j.tombstone_rows().is_empty());
    src.locked.lock().unwrap().clear();
    t.refresh_funding(W).await;
    until(|| t.with_j(|i, _| i.fence.tombstoning.is_empty())).await;
    assert_eq!(t.funding_status(W, REG), Some(NotSent));
    assert!(j.tombstone_rows().is_empty());
    assert_eq!(
        l.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::StepCancelled)
    );
    j.fail_tombstone.store(false, Ordering::SeqCst);
    t.refresh_funding(W).await;
    until(|| j.tombstone_rows().len() == 1).await;
}

/// A kill in the middle of a call that signed nothing (DP1-02 C5; Opus
/// crash probe F1 window (a)): the flagged marker reads `NotSent`, as its
/// row would, and still refuses a call of the step. Repair has nothing to
/// resolve.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_mid_call_that_signed_nothing_reads_not_sent_and_refuses_the_step() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 7))
        .await
        .unwrap();
    assert_eq!(t.funding_status(W, REG), Some(MaybeSent), "its call runs");
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(t2.funding_status(W, REG), Some(NotSent));
    let v = step_view(&t2, 7);
    assert_eq!(
        (v.status, v.signed, v.dead, v.durable_record),
        (Some(NotSent), vec![], vec![], true)
    );
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    l2.bind_funding_step(REG);
    assert!(!funds_committed(&l2));
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(NotSent)))
    );
    assert_eq!(
        t2.record_self_spend(W, REG, [1; 32], &lock_inputs()).await,
        Err(StuckFundingRefusal::NotStuck(Some(NotSent)))
    );
    std::mem::forget(p); // the dead process never ends it
}

fn lock_inputs() -> Vec<OutPoint> {
    vec![OutPoint::new(txid(101), 0)]
}

/// A journal not read at start (unopenable, a newer schema) shows no
/// marker, which says nothing (review Opus O-3; its crash probe
/// `opus_crash_journal_unavailable_after_restart_reads_a_marked_step`): a
/// marked step reads `MaybeSent`, so a lease bound to it has committed
/// funds; the PA-1 view refuses rather than list nothing; no call starts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_journal_not_read_reads_a_marked_step_maybe_sent() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    signed_maybe_sent(&t, &l, REG, 7, &row(1, 7, R::Built)).await;
    assert!(!j.signed_lock_rows().is_empty());
    let (t2, rec2) = table();
    t2.load_journal_with(None, Vec::new(), Vec::new(), Default::default());
    t2.install_tracked(src.clone());
    assert!(t2.refresh_funding(W).await);
    assert_eq!(t2.funding_status(W, REG), Some(MaybeSent));
    assert_eq!(
        t2.registration_funding_steps(W),
        Err(FundingStepsUnavailable)
    );
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    l2.bind_funding_step(REG);
    assert!(funds_committed(&l2));
    assert!(matches!(
        l2.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await,
        Err(LeaseError::JournalUnavailable)
    ));
    // A row of its draft's key that proves it still reads `Sent`.
    src.set_drafts(W, vec![DraftKey::registration(7, 7)]);
    src.set_rows(W, vec![row(1, 7, R::ChainLocked)]);
    t2.refresh_funding(W).await;
    assert_eq!(t2.funding_status(W, REG), Some(Sent));
    assert!(
        rec2.resolved()
            .iter()
            .all(|(_, r)| *r != DispatchResolution::NotSent)
    );
}

/// The empty-record guard of `shows_conflict` (review Opus O-10; the
/// mutation sub-review's M1): a recorded self-spend showing the conflict
/// with nothing, under a legacy step no signed lock is known of, never
/// resolves it, ChainLocked or not. Without the guard, "every signed
/// lock" is vacuously all of none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_recorded_self_spend_without_a_signed_lock_never_reads_not_sent() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    legacy_maybe_sent(&l, &j, REG, 7).await;
    assert!(j.signed_lock_rows().is_empty());
    let id = ArtifactId::of_step(&W, REG);
    assert!(
        JournalBackend::insert_self_spend(&*j, &W.0, REG, &id.0, &[0xe1; 32], &[]).is_err(),
        "the journal refuses one now (O-8a)"
    );
    j.push_empty_self_spend(W, REG, [0xe1; 32]);
    src.locked.lock().unwrap().insert([0xe1; 32]);
    let (t2, _) = reload(&j, &src).await;
    assert_eq!(spent(&t2.call_step(W, REG)), vec![[0xe1; 32]]);
    assert_eq!(t2.funding_status(W, REG), Some(MaybeSent));
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        l2.call_permit_step("funding", quote(10, 0), funding(REG, 7))
            .await
            .unwrap_err(),
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent)))
    );
}

/// Each of Repair's refusals has its own code, and says whether the same
/// Repair may pass later unaided (review Opus O-5, O-13b).
#[test]
fn a_repair_refusal_has_a_code_and_says_whether_it_passes() {
    use StuckFundingRefusal as F;
    let all = [
        (F::RowsUnknown, "rows_unknown", true),
        (F::NotStuck(Some(Sent)), "not_stuck", false),
        (F::NoMarker, "no_marker", false),
        (F::CallRunning, "call_running", true),
        (F::JournalUnavailable, "journal_unavailable", true),
        (F::SpendUnsettled(txid(1)), "spend_unsettled", true),
        (F::LockInputsSpent(txid(1)), "lock_inputs_spent", false),
        (F::MissesSignedLock(txid(1)), "misses_signed_lock", false),
        (F::UnattributedLock(txid(1)), "unattributed_lock", false),
        (F::NoKnownLock, "no_known_lock", false),
    ];
    for (r, code, passes) in all {
        assert_eq!((r.code(), r.retry_later()), (code, passes), "{r:?}");
    }
    assert_eq!(
        F::LockInputsSpent(txid(1)).to_string(),
        format!(
            "asset lock {}: every coin it spends is already spent",
            txid(1)
        )
    );
}

/// A copy of an ended call's lock stored with another txid and not `Built`
/// (a reconstruction of a malleated copy, whose signatures read the same
/// sighashes) was sent by someone else: it is not the step's own, and the
/// process goes on. Stored `Built`, only the builder could have stored it,
/// before its send: the process stops (§2a.7, review P2b r5 validator).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_malleated_copy_of_an_ended_calls_lock_stops_nothing() {
    let (t, _, _, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let lock = row(5, 7, R::Built);
    signed_maybe_sent(&t, &l, REG, 7, &lock).await;
    let copy = StoredLock {
        txid: txid(9),
        inputs: lock.inputs.clone(),
        sighashes: Some(vec![[5; 32]]),
        built: false,
    };
    t.record_signed_locks(W, std::slice::from_ref(&copy));
    assert_eq!(t.signed_lock_txids(W, REG), vec![lock.txid]);
    let why = fails_closed(|| {
        t.record_signed_locks(
            W,
            &[StoredLock {
                built: true,
                ..copy
            }],
        )
    });
    assert!(why.contains("ended funding call"), "{why}");
}

/// DP1-02 PA-1 reads J alone (ownership r2 Sol item 1): the view of a step
/// is the same whatever app.sqlite's drafts and the library's rows say
/// (deleted, renumbered, re-keyed), before and after a restart. A closed
/// journal gives no view rather than an empty one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_funding_view_reads_the_journal_alone_and_refuses_a_closed_one() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let lock = row(5, 7, R::Built);
    src.set_drafts(W, vec![DraftKey::registration(7, 7)]);
    signed_maybe_sent(&t, &l, REG, 7, &lock).await;
    let before = t.registration_funding_steps(W).unwrap();
    assert!(before[0].released_at.is_some(), "the call released");
    assert_eq!(
        before,
        vec![FundingStepView {
            draft: 7,
            key: key(7),
            status: Some(MaybeSent),
            signed: vec![lock.txid],
            dead: Vec::new(),
            durable_record: true,
            recording: false,
            released_at: before[0].released_at,
        }]
    );
    for drafts in [
        Vec::new(),
        vec![DraftKey::registration(9, 7)],
        vec![DraftKey::registration(7, 2)],
    ] {
        src.set_drafts(W, drafts);
        src.set_rows(W, Vec::new());
        assert!(t.refresh_funding(W).await);
        assert_eq!(t.registration_funding_steps(W).unwrap(), before);
        let (t2, _) = reload(&j, &src).await;
        assert_eq!(t2.registration_funding_steps(W).unwrap(), before);
    }
    t.close_journal().await;
    assert_eq!(
        t.registration_funding_steps(W),
        Err(FundingStepsUnavailable)
    );
}

/// DP1-02 C4 and C5 on a real `dispatch.sqlite` (ownership r2 Sol item
/// 6), with a flagged journal and the same file unflagged (OD-02): a step
/// whose call signed a lock and whose Repair self-spend is ChainLocked
/// (C4), and a step whose call was killed before it signed (C5), beside an
/// unattributed row and a row under the C4 step's own key that is not its
/// lock. Flagged, both read `NotSent`, the views and the gates agree, the
/// C4 step's resolution is announced and its tombstone written, and both
/// still read so after a reopen whose snapshot no longer shows the
/// ChainLock. Unflagged,
/// neither reads `NotSent` and none is announced: the C4 step's lock is
/// dead and its tombstone written all the same (§2a.6), but either row may
/// be a lock either call sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flagged_and_unflagged_markers_on_a_journal_file_read_as_their_flag_says() {
    use dw_appdb::dispatch::{DispatchJournal, JournalOpen};
    const AT3: &str = "registration/3/funding";
    let dir = tempfile::tempdir().unwrap();
    let (path, legacy) = (
        dir.path().join("dispatch.sqlite"),
        dir.path().join("v5.sqlite"),
    );
    let open = |path: &std::path::Path| {
        let JournalOpen::Ready(j) = DispatchJournal::open(path, 4).unwrap() else {
            panic!("schema")
        };
        let calls = j.load_calls().unwrap();
        let (rows, steps) = j.load().unwrap();
        let (t, rec) = table();
        t.load_journal_with(Some(Arc::new(j)), rows, steps, calls);
        (t, rec)
    };
    let tombstones = |path: &std::path::Path| {
        let JournalOpen::Ready(j) = DispatchJournal::open(path, 4).unwrap() else {
            panic!("schema")
        };
        j.load_calls().unwrap().tombstones.len()
    };
    let lock = row(5, 7, R::Built);
    {
        let (t, _) = open(&path);
        let src = Arc::new(FakeTracked::default());
        t.install_tracked(src.clone());
        assert!(t.refresh_funding(W).await);
        let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
        signed_maybe_sent(&t, &l, REG, 7, &lock).await;
        t.record_self_spend(W, REG, [0xee; 32], &lock.inputs)
            .await
            .unwrap();
        // Killed while running, before it signed.
        let p = l
            .call_permit_step("funding", quote(10, 0), funding(AT3, 3))
            .await
            .unwrap();
        std::mem::forget(p);
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute("VACUUM INTO ?1", [legacy.to_str().unwrap()])
            .unwrap();
    }
    rusqlite::Connection::open(&legacy)
        .unwrap()
        .execute("UPDATE funding_marker SET durable_record = 0", [])
        .unwrap();

    let src = Arc::new(FakeTracked::default());
    src.set_drafts(
        W,
        vec![DraftKey::registration(7, 7), DraftKey::registration(3, 3)],
    );
    src.set_rows(
        W,
        vec![row(1, 0, R::RecoveredFromChain), row(2, 7, R::Broadcast)],
    );
    src.locked.lock().unwrap().insert([0xee; 32]);
    let refused = |l: &Lease, step: &str, index: u32| {
        let l = l.clone();
        let step = funding(step, index);
        async move {
            l.call_permit_step("funding", quote(10, 0), step)
                .await
                .unwrap_err()
        }
    };

    // Unflagged: either row may be a lock either call sent.
    let (t, rec) = open(&legacy);
    t.install_tracked(src.clone());
    assert!(t.refresh_funding(W).await);
    assert_ne!(t.funding_status(W, REG), Some(NotSent));
    assert_eq!(t.funding_status(W, AT3), Some(MaybeSent));
    let views = t.registration_funding_steps(W).unwrap();
    assert_eq!(
        views
            .iter()
            .map(|v| (v.draft, v.signed.clone(), v.dead.clone(), v.durable_record))
            .collect::<Vec<_>>(),
        vec![
            (3, vec![], vec![], false),
            (7, vec![], vec![lock.txid], false)
        ]
    );
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        refused(&l, REG, 7).await,
        LeaseError::FundingBlocked(FundingBlock::StepCancelled)
    );
    assert_eq!(
        refused(&l, AT3, 3).await,
        LeaseError::FundingBlocked(FundingBlock::Step(Some(MaybeSent)))
    );
    assert!(rec.resolved().is_empty());
    until(|| t.with_j(|i, _| i.fence.tombstoning.is_empty())).await;
    assert_eq!(tombstones(&legacy), 1);
    drop((l, t));

    // Flagged. The first pass reads the C4 step before the self-spend's
    // ChainLock, then with it (the move is announced); the restart's reads
    // it with the ChainLock gone (a first reading is its baseline, no
    // event).
    for restart in [false, true] {
        let (t, rec) = open(&path);
        src.locked.lock().unwrap().clear();
        t.install_tracked(src.clone());
        if !restart {
            assert!(t.refresh_funding(W).await);
            assert_ne!(t.funding_status(W, REG), Some(NotSent));
            src.locked.lock().unwrap().insert([0xee; 32]);
        }
        assert!(t.refresh_funding(W).await);
        assert_eq!(t.funding_status(W, REG), Some(NotSent), "{restart}");
        assert_eq!(t.funding_status(W, AT3), Some(NotSent), "{restart}");
        let views = t.registration_funding_steps(W).unwrap();
        assert_eq!(
            views
                .iter()
                .map(|v| (
                    v.draft,
                    v.status,
                    v.signed.clone(),
                    v.dead.clone(),
                    v.durable_record
                ))
                .collect::<Vec<_>>(),
            vec![
                (3, Some(NotSent), vec![], vec![], true),
                (7, Some(NotSent), vec![], vec![lock.txid], true),
            ]
        );
        let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
        assert_eq!(
            refused(&l, REG, 7).await,
            LeaseError::FundingBlocked(FundingBlock::StepCancelled)
        );
        assert_eq!(
            refused(&l, AT3, 3).await,
            LeaseError::FundingBlocked(FundingBlock::Step(Some(NotSent)))
        );
        if restart {
            assert!(!rec.resolved().iter().any(|(s, _)| s == REG));
        } else {
            assert!(
                rec.resolved()
                    .contains(&(REG.to_owned(), DispatchResolution::NotSent))
            );
            until(|| t.with_j(|i, _| i.fence.tombstoning.is_empty())).await;
        }
        assert_eq!(tombstones(&path), 1);
    }
}

/// Review P2b r6 R6-01: a flagged marker reads `NotSent` for want of a
/// signed-lock record only when no call under it released a signature (no
/// `funding_release` row, schema 7). A call killed while asking for its
/// first signature wrote none: `NotSent`. One that released a signature and
/// recorded no lock (the library may have sent a lock no store of it
/// reached, an empty tracking changeset) reads `MaybeSent`, in process and
/// after a kill; Repair, knowing no lock, refuses (its liveness cost: a
/// new draft).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r6_01_a_released_call_with_no_recorded_lock_reads_maybe_sent() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(TOPUP, 9))
        .await
        .unwrap();
    let count = p.signatures();
    let in_flight = count.begin().expect("open");
    let killed = j.snapshot();
    drop(in_flight);
    let (t2, _) = reload(&killed, &src).await;
    assert_eq!(t2.funding_status(W, TOPUP), Some(NotSent));
    assert!(killed.release_rows().is_empty());

    p.note_released();
    assert!(t.call_step(W, TOPUP).released.is_some());
    assert_eq!(j.release_rows().len(), 1);
    assert_eq!(p.finish(false), FlowOutcome::MaybeSent);
    assert!(t.refresh_funding(W).await);
    assert_eq!(t.funding_status(W, TOPUP), Some(MaybeSent));
    let (t3, _) = reload(&j, &src).await;
    assert_eq!(t3.funding_status(W, TOPUP), Some(MaybeSent));
    assert!(t3.call_step(W, TOPUP).released.is_some());
    assert_eq!(
        t3.record_self_spend(W, TOPUP, [1; 32], &[]).await,
        Err(StuckFundingRefusal::NoKnownLock)
    );
}

/// Review P2b r6 R6-01: a release row answers its marker only. A `NotSent`
/// row (a call that released nothing, the vault refusing after the release
/// was recorded) supersedes the marker with its release, so the step's next
/// call starts clean.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r6_01_a_release_ends_with_its_marker() {
    let (t, _, j, src) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    t.record_release(W, REG).await.unwrap();
    assert!(t.call_step(W, REG).released.is_some());
    assert_eq!(p.finish(false), FlowOutcome::Cancelled);
    until(|| t.funding_status(W, REG) == Some(NotSent)).await;
    assert!(t.call_step(W, REG).released.is_none());
    // Reloaded, no marker stands, nor its release: never started.
    let (t2, _) = reload(&j, &src).await;
    assert!(t2.call_step(W, REG).released.is_none());
    assert_eq!(t2.call_step(W, REG).status, S::None);
    assert!(j.release_rows().len() == 1, "kept on disk until compacted");
    // No marker stands: nothing to record against.
    assert!(t.record_release(W, REG).await.is_err());
}

/// Review P2b r6 R6-01: a release write that keeps failing is retried,
/// then refused: nothing is recorded, in memory or on disk, so the signer
/// asks nothing of the vault and the call can end `Cancelled`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r6_01_a_release_write_that_keeps_failing_is_refused() {
    let (t, _, j, _) = setup().await;
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    let p = l
        .call_permit_step("funding", quote(10, 0), funding(REG, 0))
        .await
        .unwrap();
    j.fail_release.store(true, Ordering::SeqCst);
    assert!(t.record_release(W, REG).await.is_err());
    assert!(t.call_step(W, REG).released.is_none());
    assert!(j.release_rows().is_empty());
    j.fail_release.store(false, Ordering::SeqCst);
    t.record_release(W, REG).await.unwrap();
    assert_eq!(j.release_rows().len(), 1);
    assert_eq!(p.finish(false), FlowOutcome::Cancelled, "nothing released");
}
