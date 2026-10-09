//! Lease table, lock and fence tests (E0-04 §12 [A+B], P2a), on the table
//! alone with paused tokio time unless noted. The real-vault and session
//! tests are in `session_tests`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dw_appdb::dispatch::{DiskState, DispatchRow, Registered, StepRow};
use tokio::runtime::Handle;
use tokio::time::Instant;

use super::fence::JournalBackend;
use super::lock::Scope;
use super::table::{Issued, LeaseState, LeaseTable};
use super::*;
use crate::platform::flows::{
    BudgetPurpose, DispatchResolution, DispatchState, FlowKind, LeaseStateView, RevokeCause,
};
use crate::{DashNetwork, EngineEvent, EventSink, NoticeCode, WalletId};

pub(super) const W: WalletId = WalletId([1; 32]);
pub(super) const W2: WalletId = WalletId([2; 32]);
/// The permit lifetime H of the default config.
pub(super) const H: Duration = Duration::from_secs(10);

/// Records every event the table emits.
#[derive(Default)]
pub(super) struct Recorder(Mutex<Vec<EngineEvent>>);

impl EventSink for Recorder {
    fn emit(&self, e: EngineEvent) {
        self.0.lock().unwrap().push(e);
    }
}

impl Recorder {
    pub(super) fn resolved(&self) -> Vec<(String, DispatchResolution)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::DispatchResolved { resolved, .. } => {
                    Some((resolved.artifact.clone(), resolved.resolution))
                }
                _ => None,
            })
            .collect()
    }

    pub(super) fn notices(&self) -> Vec<NoticeCode> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::Notice { code, .. } => Some(*code),
                _ => None,
            })
            .collect()
    }

    pub(super) fn done(&self) -> Vec<LockReport> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                EngineEvent::LockProgress {
                    phase: LockPhase::Done(r),
                    ..
                } => Some(r.clone()),
                _ => None,
            })
            .collect()
    }
}

pub(super) fn status() -> dw_vault::VaultStatus {
    dw_vault::VaultStatus {
        state: dw_vault::LockState::Locked,
        encrypted: true,
        quick_unlock_enrolled: false,
        failed_attempts: 0,
        retry_after_secs: None,
        wallets_with_secrets: Vec::new(),
    }
}

pub(super) fn table_with(config: LeaseConfig) -> (Arc<LeaseTable>, Arc<Recorder>) {
    let rec = Arc::new(Recorder::default());
    let t = LeaseTable::new(
        DashNetwork::Regtest,
        rec.clone(),
        Handle::current(),
        config,
        0,
    );
    (t, rec)
}

pub(super) fn table() -> (Arc<LeaseTable>, Arc<Recorder>) {
    table_with(LeaseConfig::default())
}

fn issued(funding: u64, credits: u64, spend: u64, epoch: u64) -> Issued {
    Issued {
        funding,
        credits,
        spend,
        crypto: true,
        epoch_before: epoch,
        epoch_after: epoch,
        ..Default::default()
    }
}

pub(super) async fn begin(
    t: &Arc<LeaseTable>,
    wallet: WalletId,
    funding: u64,
    credits: u64,
    spend: u64,
) -> Result<Lease, LeaseError> {
    let epoch = t.inspect(|i| i.observed_epoch);
    t.begin(
        wallet,
        FlowKind::AcceptAndPay,
        move || Ok(issued(funding, credits, spend, epoch)),
        move || epoch,
    )
    .await
}

pub(super) fn art(n: u8) -> ArtifactId {
    ArtifactId([n; 32])
}

fn scope_of(lease: &Lease, step: Option<&str>) -> Option<DispatchScope> {
    Some(DispatchScope {
        wallet: lease.wallet(),
        origin: Origin::Lease(lease.id()),
        step: step.map(str::to_owned),
    })
}

pub(super) fn core(lease: &Lease, a: ArtifactId) -> AdmitRequest {
    AdmitRequest {
        wallet: lease.wallet(),
        artifact: a,
        kind: ArtifactKind::CoreTx { asset_lock: false },
        tracked_row: false,
        scope: scope_of(lease, None),
    }
}

pub(super) fn transition(lease: &Lease, a: ArtifactId, credits: u64) -> AdmitRequest {
    AdmitRequest {
        wallet: lease.wallet(),
        artifact: a,
        kind: ArtifactKind::Transition {
            credits,
            slot: None,
        },
        tracked_row: false,
        scope: scope_of(lease, None),
    }
}

/// A registered asset lock's request (its row is tracked).
fn asset_lock(lease: &Lease, a: ArtifactId) -> AdmitRequest {
    AdmitRequest {
        wallet: lease.wallet(),
        artifact: a,
        kind: ArtifactKind::CoreTx { asset_lock: true },
        tracked_row: true,
        scope: scope_of(lease, None),
    }
}

/// Charges `amount` to the lease's Spend budget and binds it to `a`, as
/// `TxDraft.prepare` does.
pub(super) fn draft(lease: &Lease, a: ArtifactId, amount: u64) {
    let c = lease.charge_spend(amount).unwrap();
    lease.table.bind_spend(lease.id(), lease.wallet(), a, c);
}

pub(super) fn first(v: Verdict) -> DispatchPermit {
    match v {
        Verdict::First(p) => p,
        other => panic!("expected First, got {other:?}"),
    }
}

fn resend(v: Verdict) -> AttemptGuard {
    match v {
        Verdict::Resend(g) => g,
        other => panic!("expected Resend, got {other:?}"),
    }
}

fn budget(lease: &Lease, purpose: BudgetPurpose) -> Option<(u64, u64)> {
    lease
        .view()?
        .budgets
        .iter()
        .find(|b| b.purpose == purpose)
        .map(|b| (b.ceiling, b.spent))
}

/// A row: (wallet, txid) → (origin lease, 0 unsent / 1 dispatching).
type Rows = HashMap<([u8; 32], [u8; 32]), (LeaseId, u8)>;
/// A step marker: (wallet, step id, artifact).
type Step = ([u8; 32], String, [u8; 32]);

/// A journal with injected faults (§12 "Journal").
#[derive(Default)]
pub(super) struct FakeJournal {
    pub(super) fail_register: AtomicBool,
    /// 0: ok; 1: lost (nothing written, error); 2: durable, then an error.
    pub(super) dispatch_fault: AtomicU64,
    pub(super) fail_step: AtomicBool,
    /// Real-time stall of every write (row, `Dispatching`, step), in ms.
    pub(super) stall_ms: AtomicU64,
    rows: Mutex<Rows>,
    steps: Mutex<Vec<Step>>,
}

impl JournalBackend for FakeJournal {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        _process: &[u8; 16],
        _payload: &[u8],
    ) -> Result<Registered, String> {
        self.stall();
        if self.fail_register.load(Ordering::SeqCst) {
            return Err("injected register failure".into());
        }
        let mut rows = self.rows.lock().unwrap();
        match rows.get(&(*wallet, *txid)) {
            Some((o, _)) if o != origin => Ok(Registered::OtherOrigin),
            Some(_) => Ok(Registered::Ok),
            None => {
                rows.insert((*wallet, *txid), (*origin, 0));
                Ok(Registered::Ok)
            }
        }
    }

    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String> {
        self.stall();
        let fault = self.dispatch_fault.load(Ordering::SeqCst);
        if fault == 1 {
            return Err("injected: lost".into());
        }
        let mut rows = self.rows.lock().unwrap();
        let Some(row) = rows.get_mut(&(*wallet, *txid)) else {
            return Ok(false);
        };
        row.1 = 1;
        if fault == 2 {
            return Err("injected: durable, then an error".into());
        }
        Ok(true)
    }

    fn insert_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
    ) -> Result<(), String> {
        self.stall();
        if self.fail_step.load(Ordering::SeqCst) {
            return Err("injected step failure".into());
        }
        self.steps
            .lock()
            .unwrap()
            .push((*wallet, step.to_owned(), *artifact));
        Ok(())
    }

    fn erase_wallet(&self, wallet: &[u8; 32]) -> Result<(), String> {
        self.rows.lock().unwrap().retain(|(w, _), _| w != wallet);
        self.steps.lock().unwrap().retain(|(w, _, _)| w != wallet);
        Ok(())
    }
}

impl FakeJournal {
    fn stall(&self) {
        std::thread::sleep(Duration::from_millis(self.stall_ms.load(Ordering::SeqCst)));
    }

    /// What a reopen would load (§6.5).
    pub(super) fn load(&self) -> (Vec<DispatchRow>, Vec<StepRow>) {
        let rows = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .map(|((wallet, txid), (origin, state))| DispatchRow {
                wallet: *wallet,
                txid: *txid,
                origin_lease: Some(*origin),
                process: None,
                state: if *state == 1 {
                    DiskState::Dispatching
                } else {
                    DiskState::Unsent
                },
                payload: Vec::new(),
                registered_at: 0,
                dispatched_at: None,
            })
            .collect();
        let steps = self
            .steps
            .lock()
            .unwrap()
            .iter()
            .map(|(wallet, step_id, artifact)| StepRow {
                wallet: *wallet,
                step_id: step_id.clone(),
                artifact: *artifact,
                at: 0,
            })
            .collect();
        (rows, steps)
    }
}

pub(super) fn with_journal(t: &Arc<LeaseTable>) -> Arc<FakeJournal> {
    let j = Arc::new(FakeJournal::default());
    t.load_journal(
        Some(j.clone() as Arc<dyn JournalBackend>),
        Vec::new(),
        Vec::new(),
    );
    j
}

async fn register(lease: &Lease, a: ArtifactId, debit: u64) -> Result<(), fence::RegisterError> {
    let t = Arc::clone(&lease.table);
    let w = lease.wallet();
    lease
        .scope(async move { t.register(w, a, debit, Vec::new()).await })
        .await
}

/// Runs a lock in a task; the handle yields its report.
fn spawn_lock(t: &Arc<LeaseTable>) -> tokio::task::JoinHandle<LockReport> {
    let t = Arc::clone(t);
    tokio::spawn(async move { t.lock(RevokeCause::Lock, status).await })
}

fn outcome(report: &LockReport, lease: &Lease) -> FlowOutcome {
    report
        .flows
        .iter()
        .find(|f| f.lease == lease.id_string())
        .map(|f| f.outcome)
        .unwrap_or_else(|| panic!("{} missing from {report:?}", lease.id_string()))
}

/// Verdicts compare by their decision; permits and guards never compare
/// equal, so a test matches those with [`first`] and [`resend`].
impl PartialEq for Verdict {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Verdict::Deferred, Verdict::Deferred) => true,
            (
                Verdict::Refused {
                    cleanup: a,
                    step_possibly_dispatched: b,
                },
                Verdict::Refused {
                    cleanup: c,
                    step_possibly_dispatched: d,
                },
            ) => a == c && b == d,
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// The state machine (§4.3)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_state_machine_transition_by_transition() {
    let (t, _) = table();
    let a = begin(&t, W, 0, 1_000, 1_000).await.unwrap();
    assert_eq!(a.state(), Some(LeaseState::Active));
    assert_eq!(a.view().unwrap().state, LeaseStateView::Active);
    assert_eq!(budget(&a, BudgetPurpose::Credits), Some((1_000, 0)));
    assert_eq!(budget(&a, BudgetPurpose::Funding), None, "not granted");

    // Active → Parked: the signers refuse with the purpose.
    a.park();
    assert_eq!(a.state(), Some(LeaseState::Parked));
    assert_eq!(
        a.spend_signer().unwrap_err(),
        LeaseError::Parked(BudgetPurpose::Spend)
    );
    a.park();

    // Active → NeedsGrant on an epoch change; rebind → Active.
    let b = begin(&t, W, 0, 1_000, 0).await.unwrap();
    t.epoch_changed(1);
    assert_eq!(b.state(), Some(LeaseState::NeedsGrant));
    assert_eq!(
        a.state(),
        Some(LeaseState::NeedsGrant),
        "Parked → NeedsGrant"
    );
    assert_eq!(
        b.funding_signer().unwrap_err(),
        LeaseError::NeedsGrant(BudgetPurpose::Funding)
    );
    b.rebind(move || Ok(issued(0, 500, 0, 1)), || 1)
        .await
        .unwrap();
    assert_eq!(b.state(), Some(LeaseState::Active));
    assert_eq!(budget(&b, BudgetPurpose::Credits), Some((500, 0)));
    // Only a lease in NeedsGrant rebinds.
    assert_eq!(
        b.rebind(move || Ok(issued(0, 500, 0, 1)), || 1).await,
        Err(LeaseError::Invalid)
    );

    // A redemption that raced an epoch change is inserted NeedsGrant.
    let c = t
        .begin(W, FlowKind::Withdraw, move || Ok(issued(0, 1, 0, 1)), || 2)
        .await
        .unwrap();
    assert_eq!(c.state(), Some(LeaseState::NeedsGrant));
    t.observe_epoch(2);

    // Any live state → Revoked at a freeze, with the cause.
    t.freeze(RevokeCause::PassphraseChange, Scope::All, false)
        .finish(status());
    for l in [&a, &b, &c] {
        assert_eq!(
            l.state(),
            Some(LeaseState::Revoked(RevokeCause::PassphraseChange))
        );
    }
    assert_eq!(
        b.identity_signer(&[0]).unwrap_err(),
        LeaseError::Revoked(RevokeCause::PassphraseChange)
    );

    // → Ended, idempotently, from any state.
    let d = begin(&t, W, 0, 1, 0).await.unwrap();
    d.end();
    d.end();
    assert_eq!(d.state(), Some(LeaseState::Ended));
    assert_eq!(d.funding_signer().unwrap_err(), LeaseError::Ended);
    b.end();
    assert_eq!(b.state(), Some(LeaseState::Ended));
}

#[tokio::test(start_paused = true)]
async fn the_reaper_ends_idle_vault_key_leases_and_forgets_them_later() {
    let (t, _) = table();
    let idle = LeaseConfig::default().idle;
    let a = begin(&t, W, 0, 1, 0).await.unwrap();
    let busy = begin(&t, W, 0, 1, 0).await.unwrap();
    let call = busy.call_running();
    tokio::time::advance(idle).await;
    t.reap();
    assert_eq!(a.state(), Some(LeaseState::Ended));
    assert_eq!(busy.state(), Some(LeaseState::Active), "a call runs");
    drop(call);
    tokio::time::advance(idle).await;
    t.reap();
    assert_eq!(a.state(), None, "forgotten");
    assert_eq!(t.lease(&a.id(), &W).unwrap_err(), LeaseError::Invalid);
    assert_eq!(busy.state(), Some(LeaseState::Ended));
    // Another wallet's lease id is invalid for this one (N-6).
    let other = begin(&t, W2, 0, 1, 0).await.unwrap();
    assert_eq!(t.lease(&other.id(), &W).unwrap_err(), LeaseError::Invalid);
    assert!(t.lease(&other.id(), &W2).is_ok());
}

// ---------------------------------------------------------------------------
// Budgets and rebind (§4.2, §4.3)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn a_rebind_caps_each_purpose_and_old_refunds_never_lift_it() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 1_000, 1_000, 0).await.unwrap();
    let p1 = first(t.admit(transition(&l, art(1), 700)).await);
    assert_eq!(p1.finish(Outcome::Sent), Settlement::Sent);
    let held = first(t.admit(transition(&l, art(2), 200)).await);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 900)));

    // Smaller, partial and wrong-purpose fresh grants.
    t.epoch_changed(1);
    l.rebind(move || Ok(issued(5_000, 1, 777, 1)), || 1)
        .await
        .unwrap();
    assert_eq!(
        budget(&l, BudgetPurpose::Funding),
        Some((1_000, 0)),
        "min(1000, 5000)"
    );
    assert_eq!(
        budget(&l, BudgetPurpose::Credits),
        Some((1, 0)),
        "min(100, 1)"
    );
    assert_eq!(
        budget(&l, BudgetPurpose::Spend),
        None,
        "never granted: stays 0"
    );
    assert!(l.spend_signer().is_err());

    // A new 100-credit transition under the fresh 1-credit grant.
    assert!(matches!(
        t.admit(transition(&l, art(3), 100)).await,
        Verdict::Refused { .. }
    ));
    // The older charge refunded after the rebind lifts nothing.
    assert_eq!(held.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1, 0)));

    // A second rebind with zero: the purpose is gone.
    t.epoch_changed(2);
    l.rebind(move || Ok(issued(0, 0, 0, 2)), || 2)
        .await
        .unwrap();
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((0, 0)));
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((0, 0)));
    assert_eq!(
        l.identity_signer(&[0]).unwrap_err(),
        LeaseError::NeedsGrant(BudgetPurpose::Credits)
    );
}

#[tokio::test(start_paused = true)]
async fn spend_charges_bind_to_the_txid_and_refund_on_a_definite_not_sent() {
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 0, 1_000).await.unwrap();
    // Over the cap: refused before signing.
    assert_eq!(
        l.charge_spend(1_001).unwrap_err(),
        LeaseError::Exceeded {
            purpose: BudgetPurpose::Spend,
            needed: 1_001,
            remaining: 1_000
        }
    );
    // Prepared and abandoned before any admission: refunded.
    draft(&l, art(9), 900);
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 900)));
    t.unbind_spend(art(9));
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 0)));

    // Two holders of the same bytes: O's NotSent while R runs is not a
    // settlement; the second NotSent settles once and refunds once.
    draft(&l, art(1), 600);
    let o = first(t.admit(core(&l, art(1))).await);
    let r = resend(t.admit(core(&l, art(1))).await);
    assert_eq!(o.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 600)));
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::MaybeSent));
    assert_eq!(r.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 0)));
    assert_eq!(
        rec.resolved(),
        vec![(art(1).to_string(), DispatchResolution::NotSent)]
    );
    // The tombstone: a repeat is refused without cleanup.
    assert_eq!(
        t.admit(core(&l, art(1))).await,
        Verdict::Refused {
            cleanup: false,
            step_possibly_dispatched: false
        }
    );
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::NotSent));
    t.unbind_spend(art(1));
    assert_eq!(
        budget(&l, BudgetPurpose::Spend),
        Some((1_000, 0)),
        "no second refund"
    );
    // A new prepare of the same bytes is a new draft: admitted, and its
    // own refusal (a lock first) refunds its own charge.
    draft(&l, art(1), 300);
    first(t.admit(core(&l, art(1))).await).finish(Outcome::Sent);
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 300)));
    let l2 = begin(&t, W2, 0, 0, 1_000).await.unwrap();
    draft(&l2, art(3), 200);
    t.freeze(RevokeCause::WalletClosed, Scope::Wallet(W2), false)
        .finish(status());
    assert!(matches!(
        t.admit(core(&l2, art(3))).await,
        Verdict::Refused { cleanup: true, .. }
    ));
    assert_eq!(budget(&l2, BudgetPurpose::Spend), Some((1_000, 0)));

    // A MaybeSent attempt keeps the charge.
    draft(&l, art(2), 400);
    let p = first(t.admit(core(&l, art(2))).await);
    assert_eq!(p.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    t.unbind_spend(art(2));
    assert_eq!(
        budget(&l, BudgetPurpose::Spend),
        Some((1_000, 700)),
        "300 + 400 kept"
    );
    let g = resend(t.admit(core(&l, art(2))).await);
    assert_eq!(g.finish(Outcome::Sent), Settlement::Sent);
    assert_eq!(t.dispatch_status(W, art(2)), Some(DispatchState::Sent));
}

// ---------------------------------------------------------------------------
// Row-less guards, tombstones and H16 (§5.4, §5.5)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_fence_row_less_guards() {
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 1_000, 1_000, 1_000).await.unwrap();
    // A row-less asset lock is refused.
    let mut req = core(&l, art(1));
    req.kind = ArtifactKind::CoreTx { asset_lock: true };
    assert!(matches!(t.admit(req).await, Verdict::Refused { .. }));
    // A leased Core send with no TxDraft Spend charge for its txid.
    assert_eq!(
        t.admit(core(&l, art(2))).await,
        Verdict::Refused {
            cleanup: true,
            step_possibly_dispatched: false
        }
    );
    // Another wallet's lease for this wallet's bytes.
    let other = begin(&t, W2, 0, 1_000, 0).await.unwrap();
    let mut req = transition(&other, art(3), 1);
    req.wallet = W;
    assert_eq!(t.admit(req).await, Verdict::Deferred);
    assert_eq!(rec.notices(), vec![NoticeCode::UnscopedDispatch]);

    // An unscoped call: Deferred and a Notice.
    let mut req = transition(&l, art(4), 1);
    req.scope = None;
    assert_eq!(t.admit(req.clone()).await, Verdict::Deferred);
    assert_eq!(rec.notices().len(), 2, "{:?}", rec.notices());
    // An unscoped repeat of admitted bytes: Resend and a Notice.
    let g = DispatchScope::unleased(W, UnleasedKind::Rebroadcast, {
        let (t, req) = (Arc::clone(&t), req.clone());
        async move {
            t.admit(AdmitRequest {
                scope: DispatchScope::current(),
                ..req
            })
            .await
        }
    })
    .await;
    let Verdict::FirstUnleased(g) = g else {
        panic!("expected FirstUnleased, got {g:?}");
    };
    let r = resend(t.admit(req).await);
    assert_eq!(rec.notices().len(), 3);
    drop(g);
    drop(r);
}

#[tokio::test(start_paused = true)]
async fn a_definite_rejection_tombstones_and_the_identical_retry_is_admitted() {
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let p = first(t.admit(transition(&l, art(1), 100)).await);
    assert_eq!(p.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::NotSent));
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 0)));
    // The host's retry signs identical bytes and is admitted.
    let p = first(t.admit(transition(&l, art(1), 100)).await);
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::MaybeSent));
    assert_eq!(p.finish(Outcome::Sent), Settlement::Sent);
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::Sent));
    assert_eq!(
        rec.resolved(),
        vec![
            (art(1).to_string(), DispatchResolution::NotSent),
            (art(1).to_string(), DispatchResolution::Sent),
        ]
    );
    // Unknown bytes: no answer (a restart forgets row-less artifacts).
    assert_eq!(t.dispatch_status(W, art(7)), None);
}

#[tokio::test(start_paused = true)]
async fn h16_only_a_proved_transition_in_the_same_slot_settles_another() {
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 10_000, 0).await.unwrap();
    let slot = |nonce| NonceSlot {
        identity: [5; 32],
        space: NonceSpace::Identity,
        nonce,
    };
    let at = |a, nonce| AdmitRequest {
        kind: ArtifactKind::Transition {
            credits: 10,
            slot: Some(slot(nonce)),
        },
        ..transition(&l, a, 10)
    };
    // W1 (nonce n) ends broadcast_unknown.
    let w1 = first(t.admit(at(art(1), 7)).await);
    assert_eq!(w1.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    // A later transition takes n + 1 and executes: W1 stays MaybeSent.
    let w2 = first(t.admit(at(art(2), 8)).await);
    w2.finish(Outcome::MaybeSent);
    t.note_executed(W, art(2), slot(8));
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::MaybeSent));
    // Bytes this engine never signed prove nothing.
    t.note_executed(W, art(9), slot(7));
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::MaybeSent));
    // A transition this engine signed with nonce n, proved executed.
    let w3 = first(t.admit(at(art(3), 7)).await);
    w3.finish(Outcome::MaybeSent);
    t.note_executed(W, art(3), slot(7));
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::NotSent));
    assert_eq!(t.dispatch_status(W, art(3)), Some(DispatchState::Sent));
    assert!(
        rec.resolved()
            .contains(&(art(1).to_string(), DispatchResolution::NotSent))
    );
    // A slot-taken artifact with an attempt still running settles when it
    // ends, as MaybeOut: its bytes may be out.
    let w4 = first(t.admit(at(art(4), 9)).await);
    let w5 = first(t.admit(at(art(5), 9)).await);
    w5.finish(Outcome::Sent);
    t.note_executed(W, art(5), slot(9));
    assert_eq!(t.dispatch_status(W, art(4)), Some(DispatchState::MaybeSent));
    assert_eq!(w4.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    assert_eq!(t.dispatch_status(W, art(4)), Some(DispatchState::NotSent));
}

// ---------------------------------------------------------------------------
// The journal (§6) and registered artifacts
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn funds_committed_and_dispatch_status_of_registered_artifacts() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    register(&l, art(1), 600).await.unwrap();
    register(&l, art(1), 600).await.unwrap();
    assert_eq!(
        budget(&l, BudgetPurpose::Funding),
        Some((1_000, 600)),
        "idempotent"
    );
    // Over the remaining Funding budget.
    assert!(matches!(
        register(&l, art(2), 401).await,
        Err(fence::RegisterError::Lease(LeaseError::Exceeded { .. }))
    ));
    // A live Unsent funding: MaybeSent, funds not committed.
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::MaybeSent));
    assert!(!l.view().unwrap().funds_committed);
    // Abandoned before admit: Revoked, NotSent, refunded.
    assert_eq!(
        t.abandon(W, art(1)),
        fence::Abandon::Revoked { cleanup: true }
    );
    assert_eq!(
        t.abandon(W, art(1)),
        fence::Abandon::Revoked { cleanup: false }
    );
    assert_eq!(t.dispatch_status(W, art(1)), Some(DispatchState::NotSent));
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((1_000, 0)));
    assert!(!l.view().unwrap().funds_committed);
    // Another origin's register of the same txid.
    let other = begin(&t, W, 1_000, 0, 0).await.unwrap();
    assert_eq!(
        register(&other, art(1), 1).await,
        Err(fence::RegisterError::OtherOrigin)
    );
    // From Committing on: committed.
    register(&l, art(3), 500).await.unwrap();
    let p = first(t.admit(asset_lock(&l, art(3))).await);
    assert!(l.view().unwrap().funds_committed);
    assert_eq!(
        t.dispatch_status(W, art(3)),
        Some(DispatchState::WillBeSent)
    );
    assert_eq!(t.abandon(W, art(3)), fence::Abandon::Committed);
    // L13: a rejection after Dispatching keeps the row.
    assert_eq!(p.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(
        t.dispatch_status(W, art(3)),
        Some(DispatchState::WillBeSent)
    );
    resend(t.admit(asset_lock(&l, art(3))).await).finish(Outcome::Sent);
    assert_eq!(t.dispatch_status(W, art(3)), Some(DispatchState::Sent));
}

#[tokio::test(start_paused = true)]
async fn journal_faults_give_ambiguous_then_resend() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 10_000, 0, 0).await.unwrap();
    // A register failure aborts the build and refunds.
    j.fail_register.store(true, Ordering::SeqCst);
    assert!(matches!(
        register(&l, art(1), 100).await,
        Err(fence::RegisterError::Journal(_))
    ));
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((10_000, 0)));
    j.fail_register.store(false, Ordering::SeqCst);

    // A lost Dispatching write and a durable one that errs.
    for (n, fault) in [(2u8, 1u64), (3, 2)] {
        register(&l, art(n), 100).await.unwrap();
        j.dispatch_fault.store(fault, Ordering::SeqCst);
        assert_eq!(t.admit(asset_lock(&l, art(n))).await, Verdict::Deferred);
        assert_eq!(
            t.dispatch_status(W, art(n)),
            Some(DispatchState::WillBeSent)
        );
        assert!(l.view().unwrap().funds_committed);
        j.dispatch_fault.store(0, Ordering::SeqCst);
        resend(t.admit(asset_lock(&l, art(n))).await).finish(Outcome::MaybeSent);
    }
    // A row with no entry: Deferred and a Notice, never sent.
    assert_eq!(t.admit(asset_lock(&l, art(4))).await, Verdict::Deferred);
    assert!(rec.notices().contains(&NoticeCode::DispatchRecordMissing));

    // Across a reopen: 0 is refused once with cleanup, 1 is a Resend.
    register(&l, art(5), 100).await.unwrap();
    let (rows, steps) = j.load();
    let (t2, rec2) = table();
    t2.load_journal(Some(j.clone() as Arc<dyn JournalBackend>), rows, steps);
    let l2 = begin(&t2, W, 10_000, 0, 0).await.unwrap();
    assert_eq!(
        t2.admit(asset_lock(&l2, art(5))).await,
        Verdict::Refused {
            cleanup: true,
            step_possibly_dispatched: false
        }
    );
    assert_eq!(
        t2.admit(asset_lock(&l2, art(5))).await,
        Verdict::Refused {
            cleanup: false,
            step_possibly_dispatched: false
        }
    );
    resend(t2.admit(asset_lock(&l2, art(2))).await);
    resend(t2.admit(asset_lock(&l2, art(3))).await);
    assert!(rec2.notices().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unavailable_journal_refuses_leased_firsts_and_raises_the_notice() {
    let (t, rec) = table();
    t.load_journal(None, Vec::new(), Vec::new());
    assert_eq!(rec.notices(), vec![NoticeCode::DispatchJournalUnavailable]);
    let l = begin(&t, W, 1_000, 1_000, 1_000).await.unwrap();
    assert!(matches!(
        t.admit(transition(&l, art(1), 1)).await,
        Verdict::Refused { .. }
    ));
    draft(&l, art(2), 10);
    assert!(matches!(
        t.admit(core(&l, art(2))).await,
        Verdict::Refused { cleanup: true, .. }
    ));
    assert_eq!(
        budget(&l, BudgetPurpose::Spend),
        Some((1_000, 0)),
        "refunded"
    );
    assert!(matches!(
        register(&l, art(3), 1).await,
        Err(fence::RegisterError::Journal(_))
    ));
    // Unleased hand-offs are not the journal's.
    let v = DispatchScope::unleased(W, UnleasedKind::Send, {
        let t = Arc::clone(&t);
        async move {
            t.admit(AdmitRequest {
                scope: DispatchScope::current(),
                ..transition(&l, art(4), 1)
            })
            .await
        }
    })
    .await;
    assert!(matches!(v, Verdict::FirstUnleased(_)));
}

// ---------------------------------------------------------------------------
// Resumable steps (§7.6)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn step_markers_make_the_next_process_resend_or_report_maybe_sent() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let step = |l: &Lease, a, s: &str| AdmitRequest {
        scope: scope_of(l, Some(s)),
        ..transition(l, a, 10)
    };
    // The marker is durable before the permit is returned.
    let p = first(t.admit(step(&l, art(1), "registration/d/identity")).await);
    assert_eq!(j.load().1.len(), 1);
    drop(p); // killed mid-attempt: MaybeSent
    // A failing marker write: Deferred, then a retry is a Resend.
    j.fail_step.store(true, Ordering::SeqCst);
    assert_eq!(
        t.admit(step(&l, art(2), "topup/x/st")).await,
        Verdict::Deferred
    );
    assert_eq!(t.dispatch_status(W, art(2)), Some(DispatchState::MaybeSent));
    j.fail_step.store(false, Ordering::SeqCst);
    resend(t.admit(step(&l, art(2), "topup/x/st")).await);

    // The next process: the identical signature is a Resend; a different
    // signature of the same step under a revoked lease reads MaybeSent.
    let (rows, steps) = j.load();
    let (t2, _) = table();
    t2.load_journal(Some(j.clone() as Arc<dyn JournalBackend>), rows, steps);
    let l2 = begin(&t2, W, 0, 1_000, 0).await.unwrap();
    resend(t2.admit(step(&l2, art(1), "registration/d/identity")).await);
    t2.freeze(RevokeCause::Lock, Scope::All, false)
        .finish(status());
    assert_eq!(
        t2.admit(step(&l2, art(3), "registration/d/identity")).await,
        Verdict::Refused {
            cleanup: false,
            step_possibly_dispatched: true
        }
    );
    assert_eq!(
        t2.dispatch_status(W, art(1)),
        Some(DispatchState::MaybeSent)
    );
}

// ---------------------------------------------------------------------------
// The barrier and the bound (§8.1, H3, H8)
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn begin_lease_during_a_drain_waits_then_succeeds() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let held = first(t.admit(transition(&l, art(1), 1)).await);
    let t0 = Instant::now();
    let lock = spawn_lock(&t);
    tokio::task::yield_now().await;
    let b = tokio::spawn({
        let t = Arc::clone(&t);
        async move { (begin(&t, W, 0, 1, 0).await, Instant::now()) }
    });
    tokio::time::sleep(H / 2).await;
    assert!(!b.is_finished(), "the barrier holds begin_lease");
    let report = lock.await.unwrap();
    assert_eq!(Instant::now() - t0, H, "the drain ends at the deadline");
    let (lease, at) = b.await.unwrap();
    assert_eq!(lease.unwrap().state(), Some(LeaseState::Active));
    assert!(at >= t0 + H);
    assert_eq!(outcome(&report, &l), FlowOutcome::MaybeSent);
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn begin_lease_paused_between_redemption_and_insert_is_refused() {
    let (t, _) = table();
    let (go, wait) = std::sync::mpsc::channel::<()>();
    let (redeemed, at_insert) = tokio::sync::oneshot::channel::<()>();
    let b = tokio::spawn({
        let t = Arc::clone(&t);
        async move {
            t.begin(
                W,
                FlowKind::Registration,
                move || {
                    redeemed.send(()).unwrap();
                    wait.recv().unwrap();
                    Ok(issued(1, 1, 0, 0))
                },
                || 0,
            )
            .await
        }
    });
    at_insert.await.unwrap();
    let report = t.lock(RevokeCause::Lock, status).await;
    assert!(report.flows.is_empty());
    go.send(()).unwrap();
    assert_eq!(b.await.unwrap().unwrap_err(), LeaseError::Locked);
    assert!(t.views().is_empty(), "nothing inserted");
}

#[tokio::test(start_paused = true)]
async fn lock_returns_at_h_with_one_two_and_eight_stalled_permits() {
    for n in [1u8, 2, 8] {
        let (t, _) = table();
        with_journal(&t);
        let mut permits = Vec::new();
        let mut leases = Vec::new();
        for k in 0..n {
            let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
            permits.push(first(t.admit(transition(&l, art(k), 1)).await));
            leases.push(l);
        }
        // A lease that admits nothing before the lock.
        let late = begin(&t, W, 0, 1_000, 0).await.unwrap();
        let t0 = Instant::now();
        let k1 = spawn_lock(&t);
        tokio::time::sleep(H * 9 / 10).await;
        // A First at 0.9 H is refused; a second lock joins.
        assert!(matches!(
            t.admit(transition(&late, art(100), 1)).await,
            Verdict::Refused { .. }
        ));
        let k2 = spawn_lock(&t);
        let r1 = k1.await.unwrap();
        assert_eq!(Instant::now() - t0, H, "n = {n}");
        let r2 = k2.await.unwrap();
        assert_eq!(
            Instant::now() - t0,
            H,
            "the joined lock ends with the drain"
        );
        for l in &leases {
            assert_eq!(outcome(&r1, l), FlowOutcome::MaybeSent);
            assert_eq!(outcome(&r2, l), FlowOutcome::MaybeSent);
        }
        assert_eq!(outcome(&r1, &late), FlowOutcome::Cancelled);
        drop(permits);
    }
}

#[tokio::test(start_paused = true)]
async fn a_dropped_lock_caller_revokes_everything_and_unblocks_at_the_deadline() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let held = first(t.admit(transition(&l, art(1), 1)).await);
    let t0 = Instant::now();
    let caller = spawn_lock(&t);
    tokio::time::sleep(Duration::from_secs(1)).await;
    caller.abort();
    assert_eq!(l.state(), Some(LeaseState::Revoked(RevokeCause::Lock)));
    let lease = begin(&t, W, 0, 1, 0).await.unwrap();
    assert_eq!(Instant::now() - t0, H);
    assert_eq!(lease.state(), Some(LeaseState::Active));
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn joined_locks_run_their_own_gates_and_an_unlock_waits_for_them() {
    let (t, rec) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let held = first(t.admit(transition(&l, art(1), 1)).await);
    let gates = Arc::new(AtomicU64::new(0));
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let k1 = tokio::spawn({
        let (t, gates) = (Arc::clone(&t), Arc::clone(&gates));
        async move {
            t.lock(RevokeCause::Lock, move || {
                gate.recv().unwrap();
                gates.fetch_add(1, Ordering::SeqCst);
                status()
            })
            .await
        }
    });
    tokio::task::yield_now().await;
    // An unlock queued before K1's gate runs after it (H14).
    let unlock = tokio::spawn({
        let (t, gates) = (Arc::clone(&t), Arc::clone(&gates));
        async move {
            t.wait_gates().await;
            gates.load(Ordering::SeqCst)
        }
    });
    tokio::task::yield_now().await;
    assert!(!unlock.is_finished());
    open.send(()).unwrap();
    assert_eq!(unlock.await.unwrap(), 1, "the unlock ran after K1's gate");

    // K2 during K1's drain runs its own gate, synchronously here.
    let g = Arc::clone(&gates);
    let k2 = t.lock_sync(RevokeCause::Lock, move || {
        g.fetch_add(1, Ordering::SeqCst);
        status()
    });
    assert_eq!(k2.state, dw_vault::LockState::Locked);
    assert_eq!(gates.load(Ordering::SeqCst), 2);
    assert!(rec.done().is_empty(), "the drain is still running");
    let r1 = k1.await.unwrap();
    assert_eq!(outcome(&r1, &l), FlowOutcome::MaybeSent);
    tokio::time::sleep(Duration::from_millis(1)).await;
    // Both locks' Done carry the lease.
    let done = rec.done();
    assert_eq!(done.len(), 2);
    assert!(
        done.iter()
            .all(|r| outcome(r, &l) == FlowOutcome::MaybeSent)
    );
    drop(held);
}

// ---------------------------------------------------------------------------
// Fence conformance (§12): barriers 1 and 2, row-less repeats
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn barrier_1_a_flow_paused_before_admit_is_cancelled() {
    let (t, _) = table();
    with_journal(&t);
    for kind in 0..2 {
        let l = begin(&t, W, 0, 1_000, 1_000).await.unwrap();
        let req = if kind == 0 {
            draft(&l, art(1), 500);
            core(&l, art(1))
        } else {
            transition(&l, art(2), 500)
        };
        let report = t.lock(RevokeCause::Lock, status).await;
        assert_eq!(outcome(&report, &l), FlowOutcome::Cancelled);
        assert!(matches!(t.admit(req).await, Verdict::Refused { .. }));
        assert_eq!(
            budget(&l, BudgetPurpose::Spend),
            Some((1_000, 0)),
            "released"
        );
        assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 0)));
    }
}

#[tokio::test(start_paused = true)]
async fn barrier_2_the_lock_waits_for_an_admitted_hand_off() {
    let (t, _) = table();
    with_journal(&t);
    for kind in 0..2 {
        let l = begin(&t, W, 0, 1_000, 1_000).await.unwrap();
        let req = if kind == 0 {
            draft(&l, art(1), 500);
            core(&l, art(1))
        } else {
            transition(&l, art(2), 500)
        };
        let p = first(t.admit(req).await);
        let lock = spawn_lock(&t);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!lock.is_finished(), "the lock waits for the permit");
        assert_eq!(p.finish(Outcome::Sent), Settlement::Sent);
        let report = lock.await.unwrap();
        assert_eq!(outcome(&report, &l), FlowOutcome::Sent);
        assert_eq!(report.flows[0].artifacts.len(), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn outcomes_come_from_history_never_from_what_was_in_flight() {
    let (t, _) = table();
    with_journal(&t);
    // A registration: funding Sent, identity transition uncommitted, locked
    // during the proof wait.
    let reg = begin(&t, W, 1_000, 1_000, 0).await.unwrap();
    register(&reg, art(1), 500).await.unwrap();
    first(t.admit(asset_lock(&reg, art(1))).await).finish(Outcome::Sent);
    // A rejected-after-commit asset lock.
    let other = begin(&t, W, 1_000, 0, 0).await.unwrap();
    register(&other, art(2), 500).await.unwrap();
    first(t.admit(asset_lock(&other, art(2))).await).finish(Outcome::NotSent);
    let report = t.lock(RevokeCause::Lock, status).await;
    assert_eq!(outcome(&report, &reg), FlowOutcome::Sent);
    let flow = report
        .flows
        .iter()
        .find(|f| f.lease == reg.id_string())
        .unwrap();
    assert_eq!(
        flow.artifacts,
        vec![(art(1).to_string(), FlowOutcome::Sent)]
    );
    assert_eq!(outcome(&report, &other), FlowOutcome::WillBeSent);
}

#[tokio::test(start_paused = true)]
async fn a_row_less_repeat_after_a_lock_is_a_resend_unless_it_definitely_failed() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 0, 1_000).await.unwrap();
    // Timed out (MaybeSent), then the lock: the repeat is a Resend.
    draft(&l, art(1), 100);
    first(t.admit(core(&l, art(1))).await).finish(Outcome::MaybeSent);
    let report = t.lock(RevokeCause::Lock, status).await;
    assert_eq!(outcome(&report, &l), FlowOutcome::MaybeSent);
    resend(t.admit(core(&l, art(1))).await).finish(Outcome::MaybeSent);
    assert_eq!(budget(&l, BudgetPurpose::Spend), Some((1_000, 100)), "kept");

    // A definite NotSent: the id leaves the set, the repeat is refused.
    let l = begin(&t, W, 0, 0, 1_000).await.unwrap();
    draft(&l, art(2), 100);
    first(t.admit(core(&l, art(2))).await).finish(Outcome::NotSent);
    t.lock(RevokeCause::Lock, status).await;
    assert!(matches!(
        t.admit(core(&l, art(2))).await,
        Verdict::Refused { cleanup: false, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn per_wallet_revocations_leave_other_wallets_alone() {
    let (t, _) = table();
    let a = begin(&t, W, 0, 1, 0).await.unwrap();
    let b = begin(&t, W2, 0, 1, 0).await.unwrap();
    let freeze = t.freeze(RevokeCause::WalletRemoved, Scope::Wallet(W), false);
    assert_eq!(
        a.state(),
        Some(LeaseState::Revoked(RevokeCause::WalletRemoved))
    );
    assert_eq!(b.state(), Some(LeaseState::Active));
    // The other wallet's begin is not barred; this one's waits.
    begin(&t, W2, 0, 1, 0).await.unwrap();
    let w = tokio::spawn({
        let t = Arc::clone(&t);
        async move { begin(&t, W, 0, 1, 0).await }
    });
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!w.is_finished());
    freeze.finish(status());
    w.await.unwrap().unwrap();
}

// ---------------------------------------------------------------------------
// The key window (§4.4), without a vault: the timer and its state
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn proof_wait_and_chainlock_fallback_follow_the_origin_lease() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    register(&l, art(1), 100).await.unwrap();
    // No own key: the window changes nothing.
    t.proof_wait_started(W, art(1), Duration::from_secs(300));
    assert_eq!(l.state(), Some(LeaseState::Active));
    t.chainlock_fallback(W, art(1));
    assert_eq!(l.state(), Some(LeaseState::Parked));
    assert_eq!(
        l.view().unwrap().state,
        LeaseStateView::Parked {
            reason: crate::platform::flows::ParkReason::ProofWaiting
        }
    );
}

// ---------------------------------------------------------------------------
// Real time (§12 "Real time: the same bounds, within 250 ms")
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_time_bounds_hold_within_250_ms() {
    let h = Duration::from_millis(300);
    let slack = Duration::from_millis(250);
    let (t, _) = table_with(LeaseConfig {
        permit_ttl: h,
        ..LeaseConfig::default()
    });
    let j = with_journal(&t);
    // Two stalled permits.
    let mut held = Vec::new();
    for k in 0..2 {
        let l = begin(&t, W, 1_000, 1_000, 0).await.unwrap();
        held.push(first(t.admit(transition(&l, art(k), 1)).await));
    }
    let t0 = std::time::Instant::now();
    t.lock(RevokeCause::Lock, status).await;
    let took = t0.elapsed();
    assert!(took >= h / 2 && took <= h + slack, "{took:?}");

    // A stalled Dispatching write: the lock keeps the bound and the
    // hand-off is Deferred, never handed to the transport late (L5).
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    register(&l, art(9), 100).await.unwrap();
    j.stall_ms.store(900, Ordering::SeqCst);
    let admit = tokio::spawn({
        let (t, req) = (Arc::clone(&t), asset_lock(&l, art(9)));
        async move { t.admit(req).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let t0 = std::time::Instant::now();
    t.lock(RevokeCause::Lock, status).await;
    assert!(t0.elapsed() <= h + slack, "{:?}", t0.elapsed());
    assert_eq!(admit.await.unwrap(), Verdict::Deferred);
    drop(held);
}

// ---------------------------------------------------------------------------
// Review fixes: epochs, close, rebind, cancel safety
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn the_observed_epoch_never_moves_back() {
    let (t, _) = table();
    // An unlock's stale reading lands after a lock recorded a newer epoch.
    t.observe_epoch(3);
    t.epoch_changed(2);
    t.observe_epoch(1);
    assert_eq!(t.inspect(|i| i.observed_epoch), 3);
    let l = begin(&t, W, 0, 1, 0).await.unwrap();
    assert_eq!(l.state(), Some(LeaseState::Active), "fresh at epoch 3");
    // A stale epoch change leaves live leases alone.
    t.epoch_changed(3);
    assert_eq!(l.state(), Some(LeaseState::Active));
    t.epoch_changed(4);
    assert_eq!(l.state(), Some(LeaseState::NeedsGrant));
}

#[tokio::test(start_paused = true)]
async fn nothing_is_created_after_close() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let held = first(t.admit(transition(&l, art(1), 1)).await);
    let mut close = t.freeze(RevokeCause::Close, Scope::All, false);
    // A begin that waits out close's drain must not insert afterwards.
    let b = tokio::spawn({
        let t = Arc::clone(&t);
        async move { begin(&t, W, 0, 1, 0).await }
    });
    tokio::task::yield_now().await;
    close.drain().await;
    close.finish(status());
    assert_eq!(b.await.unwrap().unwrap_err(), LeaseError::Locked);
    assert_eq!(
        begin(&t, W2, 0, 1, 0).await.unwrap_err(),
        LeaseError::Locked
    );
    assert_eq!(
        l.rebind(move || Ok(issued(0, 1, 0, 0)), || 0).await,
        Err(LeaseError::Locked)
    );
    assert_eq!(t.views().len(), 1, "only the lease begun before close");
    drop(held);
}

#[tokio::test(start_paused = true)]
async fn a_parked_lease_rebinds_under_a_fresh_grant() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1_000, 1_000).await.unwrap();
    l.park();
    assert_eq!(
        l.spend_signer().unwrap_err(),
        LeaseError::Parked(BudgetPurpose::Spend)
    );
    l.rebind(move || Ok(issued(0, 400, 0, 0)), || 0)
        .await
        .unwrap();
    assert_eq!(l.state(), Some(LeaseState::Active));
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((400, 0)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_admitted_during_a_step_write_joins_the_first() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let step = |s: &str| AdmitRequest {
        scope: scope_of(&l, Some(s)),
        ..transition(&l, art(1), 10)
    };
    j.stall_ms.store(200, Ordering::SeqCst);
    let pending = tokio::spawn({
        let (t, req) = (Arc::clone(&t), step("registration/d/identity"));
        async move { t.admit(req).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    // The same step waits for its marker; a copy scoped another way joins
    // the entry instead of charging again.
    assert_eq!(
        t.admit(step("registration/d/identity")).await,
        Verdict::Deferred
    );
    let joined = resend(t.admit(transition(&l, art(1), 10)).await);
    let permit = first(pending.await.unwrap());
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    // One attempt's definite rejection settles nothing while the other runs.
    assert_eq!(joined.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(permit.finish(Outcome::Sent), Settlement::Sent);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_admit_or_register_keeps_the_fence_consistent() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 1_000, 1_000, 0).await.unwrap();
    j.stall_ms.store(150, Ordering::SeqCst);

    // A step-scoped admit dropped during its marker write: the placeholder
    // goes; the durable marker makes the same step a Resend.
    let req = AdmitRequest {
        scope: scope_of(&l, Some("topup/x/st")),
        ..transition(&l, art(1), 10)
    };
    let dropped = tokio::spawn({
        let (t, req) = (Arc::clone(&t), req.clone());
        async move { t.admit(req).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    dropped.abort();
    tokio::time::sleep(Duration::from_millis(250)).await;
    j.stall_ms.store(0, Ordering::SeqCst);
    assert_eq!(j.load().1.len(), 1, "the marker is durable");
    resend(t.admit(req).await);

    // A register dropped during its write still enters the entry and
    // keeps its charge.
    j.stall_ms.store(150, Ordering::SeqCst);
    let dropped = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(2), 100).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    dropped.abort();
    tokio::time::sleep(Duration::from_millis(250)).await;
    j.stall_ms.store(0, Ordering::SeqCst);
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((1_000, 100)));
    first(t.admit(asset_lock(&l, art(2))).await).finish(Outcome::Sent);
    assert!(!rec.notices().contains(&NoticeCode::DispatchRecordMissing));

    // Two concurrent registers of one txid charge once.
    j.stall_ms.store(50, Ordering::SeqCst);
    let (a, b) = tokio::join!(register(&l, art(3), 100), register(&l, art(3), 100));
    a.unwrap();
    b.unwrap();
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((1_000, 200)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_joined_copy_never_settles_unsent_after_its_first_was_deferred() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    // The marker write fails: the step's First is Deferred, its marker
    // Ambiguous, so a resume may still send these bytes.
    j.stall_ms.store(200, Ordering::SeqCst);
    j.fail_step.store(true, Ordering::SeqCst);
    let pending = tokio::spawn({
        let (t, req) = (
            Arc::clone(&t),
            AdmitRequest {
                scope: scope_of(&l, Some("topup/x/st")),
                ..transition(&l, art(1), 10)
            },
        );
        async move { t.admit(req).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let joined = resend(t.admit(transition(&l, art(1), 10)).await);
    assert_eq!(pending.await.unwrap(), Verdict::Deferred);
    assert_eq!(joined.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    assert!(rec.resolved().is_empty(), "never reported NotSent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_abandon_during_the_register_write_lands_revoked_and_refunded() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    j.stall_ms.store(150, Ordering::SeqCst);
    let registering = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(1), 100).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        t.abandon(W, art(1)),
        fence::Abandon::Revoked { cleanup: true }
    );
    assert_eq!(
        t.abandon(W, art(1)),
        fence::Abandon::Revoked { cleanup: false }
    );
    registering.await.unwrap().unwrap();
    j.stall_ms.store(0, Ordering::SeqCst);
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((1_000, 0)));
    assert_eq!(
        t.admit(asset_lock(&l, art(1))).await,
        Verdict::Refused {
            cleanup: false,
            step_possibly_dispatched: false
        }
    );
}
