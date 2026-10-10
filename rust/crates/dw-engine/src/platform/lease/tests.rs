//! Lease table, lock and fence tests (E0-04 §12 [A+B], P2a), on the table
//! alone with paused tokio time unless noted. The real-vault and session
//! tests are in `session_tests`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dw_appdb::dispatch::{
    DiskState, DispatchRow, Registered, Resolution, StepEvent, StepLogRow, StepRow,
};
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

/// A journal with injected faults (§12 "Journal").
#[derive(Default)]
pub(super) struct FakeJournal {
    pub(super) fail_register: AtomicBool,
    /// 0: ok; 1: lost (nothing written, error); 2: durable, then an error.
    pub(super) dispatch_fault: AtomicU64,
    pub(super) fail_step: AtomicBool,
    /// A `NotSent` row: lost (nothing written, error).
    pub(super) fail_resolve: AtomicBool,
    pub(super) panic_resolve: AtomicBool,
    /// A `NotSent` row: durable, then an error.
    pub(super) resolve_durable_err: AtomicBool,
    /// A `Sent` row: lost.
    pub(super) fail_sent: AtomicBool,
    /// A `Sent` row: durable, then an error.
    pub(super) sent_durable_err: AtomicBool,
    pub(super) panic_sent: AtomicBool,
    /// Real-time stall of every write (row, `Dispatching`, step), in ms.
    pub(super) stall_ms: AtomicU64,
    rows: Mutex<Rows>,
    /// `step_log`, append-only (DEC-154).
    log: Mutex<Vec<StepLogRow>>,
    /// Every artifact a marker row was ever appended for.
    ever_marked: Mutex<std::collections::HashSet<([u8; 32], [u8; 32])>>,
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
        self.append_marker(&mut self.log.lock().unwrap(), wallet, artifact, step);
        Ok(())
    }

    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String> {
        self.resolve_rows(wallet, artifact, Resolution::Sent, steps)
    }

    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String> {
        self.resolve_rows(wallet, artifact, r, &[])
    }

    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String> {
        if dispatch {
            self.rows.lock().unwrap().retain(|(w, _), _| w != wallet);
        }
        // DEC-160's predicate, under the log's lock like a transaction.
        let mut log = self.log.lock().unwrap();
        let gone: Vec<[u8; 32]> = dw_appdb::dispatch::compactable(&log)
            .into_iter()
            .filter(|(w, _)| w == wallet)
            .map(|(_, a)| a)
            .collect();
        log.retain(|r| r.wallet != *wallet || !gone.contains(&r.artifact));
        Ok(gone)
    }

    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
        self.rows.lock().unwrap().retain(|(w, _), _| w != wallet);
        let mut log = self.log.lock().unwrap();
        let gone: std::collections::BTreeSet<[u8; 32]> = log
            .iter()
            .filter(|r| r.wallet == *wallet)
            .map(|r| r.artifact)
            .collect();
        log.retain(|r| r.wallet != *wallet);
        Ok(gone.into_iter().collect())
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
        let steps: Vec<StepRow> = dw_appdb::dispatch::standing(&self.log.lock().unwrap());
        (rows, steps)
    }

    /// A marker of `step`, appended unless one stands.
    /// The resolution, after `steps`' markers in the same lock span.
    fn resolve_rows(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        r: Resolution,
        steps: &[String],
    ) -> Result<(), String> {
        self.stall();
        let not_sent = r == Resolution::NotSent;
        if not_sent && self.fail_resolve.load(Ordering::SeqCst)
            || !not_sent && self.fail_sent.load(Ordering::SeqCst)
        {
            return Err("injected resolve failure".into());
        }
        let panic = if not_sent {
            &self.panic_resolve
        } else {
            &self.panic_sent
        };
        assert!(!panic.load(Ordering::SeqCst), "injected resolve panic");
        // One transaction: under the log's lock throughout.
        let mut log = self.log.lock().unwrap();
        for step in steps {
            self.append_marker(&mut log, wallet, artifact, step);
        }
        log.push(StepLogRow {
            wallet: *wallet,
            artifact: *artifact,
            event: StepEvent::Resolved(r),
            at: 0,
        });
        drop(log);
        let durable_err = if not_sent {
            &self.resolve_durable_err
        } else {
            &self.sent_durable_err
        };
        if durable_err.load(Ordering::SeqCst) {
            return Err("injected: durable, then an error".into());
        }
        Ok(())
    }

    fn append_marker(
        &self,
        log: &mut Vec<StepLogRow>,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        step: &str,
    ) {
        let stands = dw_appdb::dispatch::standing(log)
            .iter()
            .any(|s| s.wallet == *wallet && s.artifact == *artifact && s.step_id == step);
        if !stands {
            self.ever_marked
                .lock()
                .unwrap()
                .insert((*wallet, *artifact));
            log.push(StepLogRow {
                wallet: *wallet,
                artifact: *artifact,
                event: StepEvent::Marked(step.to_owned()),
                at: 0,
            });
        }
    }

    /// Whether `step`'s marker of `artifact` stands on disk now, as the
    /// next process would read it.
    pub(super) fn stands(&self, wallet: &WalletId, artifact: &ArtifactId, step: &str) -> bool {
        dw_appdb::dispatch::standing(&self.log.lock().unwrap())
            .iter()
            .any(|r| r.wallet == wallet.0 && r.artifact == artifact.0 && r.step_id == step)
    }

    /// An artifact whose marker rows are gone from under its final row:
    /// evidence that recovery reads as never handed off. (A marker that
    /// never landed leaves a final row bare too, rightly: no copy went
    /// out on it.)
    pub(super) fn bare_final_row(&self) -> Option<ArtifactId> {
        let log = self.log.lock().unwrap();
        let ever = self.ever_marked.lock().unwrap();
        log.iter()
            .filter(|r| {
                matches!(
                    r.event,
                    StepEvent::Resolved(Resolution::Sent | Resolution::MaybeSent)
                )
            })
            .find(|f| {
                ever.contains(&(f.wallet, f.artifact))
                    && !log.iter().any(|r| {
                        (r.wallet, r.artifact) == (f.wallet, f.artifact)
                            && matches!(r.event, StepEvent::Marked(_))
                    })
            })
            .map(|f| ArtifactId(f.artifact))
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

/// Review O-3: a lease a lock revoked is ended by the reaper once idle
/// (its cause readable until then), and forgotten an idle period later.
#[tokio::test(start_paused = true)]
async fn the_reaper_collects_leases_a_lock_revoked() {
    let (t, _) = table();
    let idle = LeaseConfig::default().idle;
    let a = begin(&t, W, 0, 1, 0).await.unwrap();
    t.lock(RevokeCause::Lock, status).await;
    assert_eq!(a.state(), Some(LeaseState::Revoked(RevokeCause::Lock)));
    t.reap();
    assert_eq!(a.state(), Some(LeaseState::Revoked(RevokeCause::Lock)));
    tokio::time::advance(idle).await;
    t.reap();
    assert_eq!(a.state(), Some(LeaseState::Ended));
    tokio::time::advance(idle).await;
    t.reap();
    assert_eq!(a.state(), None, "forgotten");
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
    t.unbind_spend(l.id(), W, art(9));
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
    t.unbind_spend(l.id(), W, art(1));
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
    t.unbind_spend(l.id(), W, art(2));
    assert_eq!(
        budget(&l, BudgetPurpose::Spend),
        Some((1_000, 700)),
        "300 + 400 kept"
    );
    let g = resend(t.admit(core(&l, art(2))).await);
    assert_eq!(g.finish(Outcome::Sent), Settlement::Sent);
    assert_eq!(t.dispatch_status(W, art(2)), Some(DispatchState::Sent));
}

/// Review O-1 (Opus's probe, inverted): the same bytes admitted for a second
/// wallet while the first wallet's attempt runs wait for it. Neither settles
/// definitely unsent while the other's attempt may put them out, and each
/// wallet's Spend charge is refunded once, by its own settlement.
#[tokio::test(start_paused = true)]
async fn o1_the_same_bytes_under_two_wallets_never_settle_unsent_while_one_runs() {
    let (t, _rec) = table();
    with_journal(&t);
    let l1 = begin(&t, W, 0, 0, 1_000).await.unwrap();
    let l2 = begin(&t, W2, 0, 0, 1_000).await.unwrap();
    draft(&l2, art(5), 500);
    let p2 = first(t.admit(core(&l2, art(5))).await);
    draft(&l1, art(5), 400);
    assert_eq!(t.admit(core(&l1, art(5))).await, Verdict::Deferred);
    assert_eq!(budget(&l2, BudgetPurpose::Spend), Some((1_000, 500)));
    assert_eq!(p2.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(budget(&l2, BudgetPurpose::Spend), Some((1_000, 0)));
    // W2's tombstone is its own: W's draft of the bytes goes now.
    let p1 = first(t.admit(core(&l1, art(5))).await);
    assert_eq!(t.admit(core(&l2, art(5))).await, Verdict::Deferred);
    assert_eq!(p1.finish(Outcome::NotSent), Settlement::DefinitelyUnsent);
    assert_eq!(budget(&l1, BudgetPurpose::Spend), Some((1_000, 0)));
    assert_eq!(budget(&l2, BudgetPurpose::Spend), Some((1_000, 0)));
    assert_eq!(t.dispatch_status(W, art(5)), Some(DispatchState::NotSent));
    assert_eq!(t.dispatch_status(W2, art(5)), Some(DispatchState::NotSent));

    // Possibly out under one wallet, the bytes stay held for the other,
    // and each wallet's binding is its own to release.
    draft(&l2, art(6), 100);
    let p = first(t.admit(core(&l2, art(6))).await);
    assert_eq!(p.finish(Outcome::MaybeSent), Settlement::MaybeOut);
    draft(&l1, art(6), 200);
    assert_eq!(t.admit(core(&l1, art(6))).await, Verdict::Deferred);
    t.unbind_spend(l2.id(), W, art(6));
    assert_eq!(budget(&l1, BudgetPurpose::Spend), Some((1_000, 200)));
    t.unbind_spend(l1.id(), W, art(6));
    assert_eq!(budget(&l1, BudgetPurpose::Spend), Some((1_000, 0)));
    assert_eq!(budget(&l2, BudgetPurpose::Spend), Some((1_000, 100)));
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
    assert_eq!(
        rec2.resolved(),
        vec![(art(5).to_string(), DispatchResolution::NotSent)],
        "the reload's refusal resolves the row once"
    );
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

/// Waits (real time, at most 5 s) until `f` holds.
async fn until(mut f: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !f() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition never held");
}

/// Whether `a`'s marker in `step` is being written.
fn committing(t: &LeaseTable, step: &str, a: ArtifactId) -> bool {
    t.with_j(|i, _| {
        i.fence
            .mark(&W, step, &a)
            .is_some_and(|m| matches!(m, fence::Mark::Committing { .. }))
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_copy_of_a_step_artifact_waits_for_every_marker() {
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
    until(|| committing(&t, "registration/d/identity", art(1))).await;
    // H10 (review P2a r1 F1): while the marker is written, no copy goes,
    // whatever its scope.
    for copy in [
        step("registration/d/identity"),
        step("withdrawal/d/submit"),
        transition(&l, art(1), 10),
    ] {
        assert_eq!(t.admit(copy).await, Verdict::Deferred);
    }
    let permit = first(pending.await.unwrap());
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    // Durable: an unscoped copy joins without charging.
    let joined = resend(t.admit(transition(&l, art(1), 10)).await);
    // A copy of another step writes its own marker first; meanwhile every
    // copy waits again.
    let other = tokio::spawn({
        let (t, req) = (Arc::clone(&t), step("withdrawal/d/submit"));
        async move { t.admit(req).await }
    });
    until(|| committing(&t, "withdrawal/d/submit", art(1))).await;
    assert_eq!(t.admit(transition(&l, art(1), 10)).await, Verdict::Deferred);
    let other = resend(other.await.unwrap());
    assert_eq!(j.load().1.len(), 2, "both markers are durable");
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    // One attempt's definite rejection settles nothing while others run.
    assert_eq!(joined.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(other.finish(Outcome::NotSent), Settlement::MaybeOut);
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
async fn a_failed_step_write_hands_off_no_copy_until_it_is_repaired() {
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
    until(|| committing(&t, "topup/x/st", art(1))).await;
    assert_eq!(t.admit(transition(&l, art(1), 10)).await, Verdict::Deferred);
    assert_eq!(pending.await.unwrap(), Verdict::Deferred);
    // Still no marker: an unscoped copy is not handed off.
    assert_eq!(t.admit(transition(&l, art(1), 10)).await, Verdict::Deferred);
    assert!(j.load().1.is_empty());
    // The journal works again: the copy first repairs the marker, then
    // resends uncharged (the First's charge stays spent).
    j.fail_step.store(false, Ordering::SeqCst);
    j.stall_ms.store(0, Ordering::SeqCst);
    let joined = resend(t.admit(transition(&l, art(1), 10)).await);
    assert_eq!(j.load().1.len(), 1);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_removal_erase_waits_for_an_in_flight_register() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    let other = begin(&t, W2, 1_000, 0, 0).await.unwrap();
    register(&other, art(9), 1).await.unwrap();
    // A register whose row write is still running when the removal starts.
    j.stall_ms.store(300, Ordering::SeqCst);
    let registering = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(1), 100).await }
    });
    // Its marker stays for the stalled write; inserting it notifies no one.
    while !t.with_j(|i, _| i.fence.registering.contains_key(&(W, art(1)))) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    // DEC-134: the removal holds its barrier until it is done, so a lease
    // begun meanwhile waits it out and registers nothing; its erase waits
    // for the running write to land.
    let mut removal = t.freeze(RevokeCause::WalletRemoved, Scope::Wallet(W), false);
    removal.drained().await;
    let fresh = tokio::spawn({
        let t = Arc::clone(&t);
        async move { begin(&t, W, 1_000, 0, 0).await }
    });
    t.erase_wallet_rows(W).await;
    assert!(
        t.with_j(|i, _| i.fence.registering.is_empty()),
        "the erase ran after the write"
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!fresh.is_finished(), "the barrier is still held");
    drop(removal);
    fresh.await.unwrap().unwrap();
    registering.await.unwrap().unwrap();
    let (rows, steps) = j.load();
    assert!(
        rows.iter().all(|r| r.wallet != W.0),
        "no row of the removed wallet: {rows:?}"
    );
    assert!(steps.is_empty());
    assert!(rows.iter().any(|r| r.wallet == W2.0), "W2 keeps its row");
}

// Review P2a r1 regressions: Sol's interleavings, each failing on 270665a.

/// A real journal whose `register` and step writes wait at gates.
struct GatedJournal {
    db: dw_appdb::dispatch::DispatchJournal,
    /// Open gates; a closed one holds its writes.
    open: Mutex<(bool, bool)>,
    opened: std::sync::Condvar,
    registers: AtomicU64,
    steps: AtomicU64,
    fail_step: AtomicBool,
}

impl GatedJournal {
    fn new() -> Arc<Self> {
        let dw_appdb::dispatch::JournalOpen::Ready(db) =
            dw_appdb::dispatch::DispatchJournal::open_in_memory(0).unwrap()
        else {
            panic!("a fresh journal");
        };
        Arc::new(Self {
            db,
            open: Mutex::new((false, false)),
            opened: std::sync::Condvar::new(),
            registers: AtomicU64::new(0),
            steps: AtomicU64::new(0),
            fail_step: AtomicBool::new(false),
        })
    }

    fn wait(&self, gate: fn(&(bool, bool)) -> bool) {
        let mut open = self.open.lock().unwrap();
        while !gate(&open) {
            open = self.opened.wait(open).unwrap();
        }
    }

    fn open_registers(&self) {
        self.open.lock().unwrap().0 = true;
        self.opened.notify_all();
    }

    fn open_steps(&self) {
        self.open.lock().unwrap().1 = true;
        self.opened.notify_all();
    }
}

/// Opens every gate when dropped: a failing test must not leave a write
/// parked on a blocking thread, which would hang the runtime's shutdown.
struct OpenOnDrop(Arc<GatedJournal>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        self.0.open_registers();
        self.0.open_steps();
    }
}

impl JournalBackend for GatedJournal {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        process: &[u8; 16],
        payload: &[u8],
    ) -> Result<Registered, String> {
        self.registers.fetch_add(1, Ordering::SeqCst);
        self.wait(|o| o.0);
        self.db
            .register(wallet, txid, origin, process, payload, 0)
            .map_err(|e| e.to_string())
    }

    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String> {
        self.db
            .mark_dispatching(wallet, txid, 0)
            .map_err(|e| e.to_string())
    }

    fn insert_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
    ) -> Result<(), String> {
        self.steps.fetch_add(1, Ordering::SeqCst);
        self.wait(|o| o.1);
        if self.fail_step.load(Ordering::SeqCst) {
            return Err("injected pre-write failure".into());
        }
        self.db
            .insert_step(wallet, step, artifact, 0)
            .map_err(|e| e.to_string())
    }

    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String> {
        self.db
            .resolve(wallet, artifact, r, 0)
            .map_err(|e| e.to_string())
    }

    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String> {
        self.db
            .resolve_sent(wallet, artifact, steps, 0)
            .map_err(|e| e.to_string())
    }

    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String> {
        self.db
            .erase_wallet(wallet, dispatch)
            .map_err(|e| e.to_string())
    }

    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
        self.db
            .erase_wallet_unconditionally(wallet)
            .map_err(|e| e.to_string())
    }
}

/// F1: a copy scoped another way never transports before the step marker
/// is durable; after the marker write fails, nothing was handed off and
/// no marker exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f1_no_copy_transports_before_its_step_marker_is_durable() {
    let (t, _) = table();
    let j = GatedJournal::new();
    let _open = OpenOnDrop(Arc::clone(&j));
    j.fail_step.store(true, Ordering::SeqCst);
    t.load_journal(Some(j.clone()), vec![], vec![]);
    let l = begin(&t, W, 0, 1000, 0).await.unwrap();
    let req = AdmitRequest {
        scope: scope_of(&l, Some("registration/d/identity")),
        ..transition(&l, art(78), 10)
    };
    let pending = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.admit(req).await }
    });
    until(|| j.steps.load(Ordering::SeqCst) == 1).await;
    let copy = t.admit(transition(&l, art(78), 10)).await;
    assert_eq!(copy, Verdict::Deferred, "H10: a copy before the marker");
    j.open_steps();
    assert_eq!(pending.await.unwrap(), Verdict::Deferred);
    assert!(j.db.load().unwrap().1.is_empty());
    // A later copy repairs the marker before it goes.
    j.fail_step.store(false, Ordering::SeqCst);
    resend(t.admit(transition(&l, art(78), 10)).await).finish(Outcome::MaybeSent);
    assert_eq!(j.db.load().unwrap().1.len(), 1);
}

/// F2: a refunded, definitely unsent step-scoped transition is never
/// revived by its marker: not after a Lock, not after a reload.
#[tokio::test(start_paused = true)]
async fn r1_f2_a_definite_not_sent_supersedes_its_step_marker() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let req = AdmitRequest {
        scope: scope_of(&l, Some("withdrawal/d/submit")),
        ..transition(&l, art(80), 10)
    };
    let permit = first(t.admit(req.clone()).await);
    assert_eq!(
        j.load().1.len(),
        1,
        "the marker is durable before transport"
    );
    assert_eq!(
        permit.finish(Outcome::NotSent),
        Settlement::DefinitelyUnsent
    );
    assert!(
        j.load().1.is_empty(),
        "the resolution reached the journal first"
    );
    assert_eq!(t.dispatch_status(W, art(80)), Some(DispatchState::NotSent));
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((100, 0)));
    assert_eq!(
        rec.resolved(),
        vec![(art(80).to_string(), DispatchResolution::NotSent)]
    );
    t.lock(RevokeCause::Lock, status).await;
    let refused = Verdict::Refused {
        cleanup: false,
        step_possibly_dispatched: false,
    };
    assert_eq!(t.admit(req.clone()).await, refused);
    assert_eq!(t.dispatch_status(W, art(80)), Some(DispatchState::NotSent));

    // A reload: no marker revives it either.
    let (rows, steps) = j.load();
    let (t2, _) = table();
    t2.load_journal(Some(j.clone() as Arc<dyn JournalBackend>), rows, steps);
    let dead = begin(&t2, W, 0, 100, 0).await.unwrap();
    t2.lock(RevokeCause::Lock, status).await;
    let resume = |l: &Lease| AdmitRequest {
        scope: scope_of(l, Some("withdrawal/d/submit")),
        ..transition(l, art(80), 10)
    };
    assert_eq!(t2.admit(resume(&dead)).await, refused);
    // Under a live lease it is a fresh First, charged again.
    let live = begin(&t2, W, 0, 100, 0).await.unwrap();
    first(t2.admit(resume(&live)).await).finish(Outcome::Sent);
    assert_eq!(budget(&live, BudgetPurpose::Credits), Some((100, 10)));
}

/// F2: the definite resolution is seen only once it is durable; when the
/// journal cannot record it, it stays possibly out, charged.
#[tokio::test(start_paused = true)]
async fn r1_f2_a_resolution_the_journal_refuses_stays_possibly_out() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let req = AdmitRequest {
        scope: scope_of(&l, Some("withdrawal/d/submit")),
        ..transition(&l, art(81), 10)
    };
    let permit = first(t.admit(req.clone()).await);
    j.fail_resolve.store(true, Ordering::SeqCst);
    assert_eq!(permit.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(j.load().1.len(), 1, "the marker stays");
    assert_eq!(
        t.dispatch_status(W, art(81)),
        Some(DispatchState::MaybeSent)
    );
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((100, 10)));
    assert!(rec.resolved().is_empty());
    // Known unsent, it is never resent on its old marker: a copy retries
    // the resolution first, and defers while the journal refuses it.
    assert_eq!(t.admit(req.clone()).await, Verdict::Deferred);
    assert_eq!(j.load().1.len(), 1);
    j.fail_resolve.store(false, Ordering::SeqCst);
    // The retry lands: refunded, NotSent, and the copy is a fresh First.
    let again = first(t.admit(req).await);
    assert!(j.load().1.len() == 1, "the new First's own marker");
    assert_eq!(rec.resolved().len(), 1);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((100, 10)));
    again.finish(Outcome::Sent);
}

/// F2: a nonce-slot resolution (H16) supersedes the loser's marker, not
/// the winner's.
#[tokio::test(start_paused = true)]
async fn r1_f2_a_slot_resolution_supersedes_the_losers_marker() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let slot = NonceSlot {
        identity: [3; 32],
        space: NonceSpace::Identity,
        nonce: 7,
    };
    let signed = |a| AdmitRequest {
        kind: ArtifactKind::Transition {
            credits: 10,
            slot: Some(slot),
        },
        scope: scope_of(&l, Some("topup/x/st")),
        ..transition(&l, a, 10)
    };
    first(t.admit(signed(art(82))).await).finish(Outcome::MaybeSent);
    first(t.admit(signed(art(83))).await).finish(Outcome::MaybeSent);
    t.note_executed(W, art(83), slot);
    assert_eq!(t.dispatch_status(W, art(82)), Some(DispatchState::NotSent));
    let steps = j.load().1;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].artifact, art(83).0);
    t.lock(RevokeCause::Lock, status).await;
    assert!(matches!(
        t.admit(signed(art(82))).await,
        Verdict::Refused { .. }
    ));
}

/// F4: duplicate registrations are single-flight, so a removal's erase
/// waits for every write and no row of the wallet lands after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f4_duplicate_registers_cannot_outlive_a_removal() {
    let (t, _) = table();
    let j = GatedJournal::new();
    let _open = OpenOnDrop(Arc::clone(&j));
    t.load_journal(Some(j.clone()), vec![], vec![]);
    let l = begin(&t, W, 1000, 0, 0).await.unwrap();
    let other = begin(&t, W, 1000, 0, 0).await.unwrap();
    let a = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(77), 10).await }
    });
    until(|| j.registers.load(Ordering::SeqCst) == 1).await;
    let b = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(77), 10).await }
    });
    let c = tokio::spawn({
        let other = other.clone();
        async move { register(&other, art(77), 10).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !b.is_finished() && !c.is_finished(),
        "both wait for the write"
    );
    let mut removal = t.freeze(RevokeCause::WalletRemoved, Scope::Wallet(W), false);
    removal.drained().await;
    let erase = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.erase_wallet_rows(W).await }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !erase.is_finished(),
        "the erase waits for the running write"
    );
    j.open_registers();
    a.await.unwrap().unwrap();
    erase.await.unwrap();
    // The duplicates never wrote: one writer ran. The same-origin one may
    // still have read the entry before the erase forgot it.
    let _ = b.await.unwrap();
    assert!(c.await.unwrap().is_err());
    assert_eq!(j.registers.load(Ordering::SeqCst), 1);
    drop(removal);
    assert!(j.db.load().unwrap().0.is_empty(), "no row after the erase");
    assert_eq!(budget(&l, BudgetPurpose::Funding), Some((1000, 10)));
    assert_eq!(budget(&other, BudgetPurpose::Funding), Some((1000, 0)));
}

/// F6: dropping the last owning handle ends the lease; a lookup handle
/// never does.
#[tokio::test(start_paused = true)]
async fn r1_f6_the_last_owning_handle_ends_the_lease() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let id = l.id();
    // Lookups and their drops leave it active, and so does a dropped
    // owning clone while another owner lives.
    drop(t.lease(&id, &W).unwrap());
    let clone = l.clone();
    drop(clone);
    assert!(!t.lease(&id, &W).unwrap().is_owner());
    assert_eq!(t.lease(&id, &W).unwrap().state(), Some(LeaseState::Active));
    let req = transition(&l, art(84), 10);
    drop(l);
    assert_eq!(t.lease(&id, &W).unwrap().state(), Some(LeaseState::Ended));
    assert!(!matches!(t.admit(req).await, Verdict::First(_)));
}

/// F8: no purpose, no hand-off: a zero cost does not stand in for a
/// Credits or Funding grant.
#[tokio::test(start_paused = true)]
async fn r1_f8_a_zero_cost_needs_the_purpose() {
    let (t, _) = table();
    with_journal(&t);
    let l = begin(&t, W, 0, 0, 0).await.unwrap();
    assert!(matches!(
        t.admit(transition(&l, art(85), 0)).await,
        Verdict::Refused { .. }
    ));
    assert_eq!(
        register(&l, art(86), 0).await,
        Err(LeaseError::NeedsGrant(BudgetPurpose::Funding).into())
    );
    // With the purpose, zero is a valid charge.
    let credits = begin(&t, W, 0, 1, 0).await.unwrap();
    first(t.admit(transition(&credits, art(87), 0)).await).finish(Outcome::Sent);
}

/// Polls `f` once.
fn poll_once<F: Future + Unpin>(f: &mut F) -> std::task::Poll<F::Output> {
    let waker = std::task::Waker::noop();
    std::pin::Pin::new(f).poll(&mut std::task::Context::from_waker(waker))
}

fn step_copy(l: &Lease, a: ArtifactId, step: &str) -> AdmitRequest {
    AdmitRequest {
        scope: scope_of(l, Some(step)),
        ..transition(l, a, 10)
    }
}

/// Review of r1 (finding 2): a copy whose own marker landed still owes a
/// marker another copy began before it joined.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f1_a_copy_owes_a_marker_begun_before_it_joined() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(90);
    let running = first(t.admit(step_copy(&l, a, "s")).await);
    // C1 (step t) writes its marker; its continuation has not run.
    j.stall_ms.store(100, Ordering::SeqCst);
    let mut c1 = Box::pin(t.admit(step_copy(&l, a, "t")));
    assert!(poll_once(&mut c1).is_pending());
    until(|| t.with_j(|i, _| i.fence.mark(&W, "t", &a)) == Some(fence::Mark::Durable)).await;
    // C2 (step u) begins its marker, slowly.
    j.stall_ms.store(300, Ordering::SeqCst);
    let mut c2 = Box::pin(t.admit(step_copy(&l, a, "u")));
    assert!(poll_once(&mut c2).is_pending());
    assert!(committing(&t, "u", a));
    assert_eq!(c1.await, Verdict::Deferred, "u is not durable yet");
    resend(c2.await).finish(Outcome::Sent);
    running.finish(Outcome::Sent);
}

/// Review of r1 (finding 3): a settlement a marker write blocked still
/// happens when that write fails, so the known-unsent bytes are never
/// resent after a Lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f2_a_blocked_settlement_lands_when_the_write_fails() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(91);
    let req = step_copy(&l, a, "s");
    let running = first(t.admit(req.clone()).await);
    j.stall_ms.store(300, Ordering::SeqCst);
    j.fail_step.store(true, Ordering::SeqCst);
    let mut copy = Box::pin(t.admit(step_copy(&l, a, "t")));
    assert!(poll_once(&mut copy).is_pending());
    assert!(committing(&t, "t", a));
    // Blocked by the copy's write: not settled yet.
    assert_eq!(running.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(copy.await, Verdict::Deferred);
    until(|| t.dispatch_status(W, a) == Some(DispatchState::NotSent)).await;
    assert!(j.load().1.is_empty(), "the resolution reached the journal");
    assert_eq!(rec.resolved().len(), 1);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 0)));
    j.stall_ms.store(0, Ordering::SeqCst);
    j.fail_step.store(false, Ordering::SeqCst);
    t.lock(RevokeCause::Lock, status).await;
    assert!(matches!(t.admit(req).await, Verdict::Refused { .. }));
}

/// Review of r1 (finding 1): the library is never polled at or past the
/// deadline, even when it would finish in that very poll.
#[tokio::test(start_paused = true)]
async fn r1_f3_the_library_is_not_polled_at_the_deadline() {
    let (t, _) = table();
    let d = Instant::now() + H;
    let polled_late = Arc::new(AtomicBool::new(false));
    let late = Arc::clone(&polled_late);
    let ended = t
        .hand_off(Some(d), async { Ok::<_, ()>(()) }, |()| async move {
            tokio::time::sleep_until(d).await;
            late.store(true, Ordering::SeqCst);
        })
        .await
        .unwrap();
    assert!(matches!(ended, fence::HandOff::Cut));
    assert!(!polled_late.load(Ordering::SeqCst));
    // Before the deadline it runs as usual; past it, it is never entered.
    let ok = t.hand_off(Some(d + H), async { Ok::<_, ()>(()) }, |()| async { 7 });
    assert!(matches!(ok.await.unwrap(), fence::HandOff::Done(7)));
    let entered = Arc::new(AtomicBool::new(false));
    let e = Arc::clone(&entered);
    let none = t.hand_off(
        Some(Instant::now()),
        async { Ok::<_, ()>(()) },
        |()| async move { e.store(true, Ordering::SeqCst) },
    );
    assert!(matches!(none.await.unwrap(), fence::HandOff::NotEntered));
    assert!(!entered.load(Ordering::SeqCst));
}

fn resolving(t: &LeaseTable, a: ArtifactId) -> bool {
    t.with_j(|i, _| i.fence.resolving(&(W, a)))
}

/// Review of r1 (fix round): a marker write that returns never settles
/// an artifact already seen sent, so its charge is never refunded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f2_a_marker_write_never_settles_a_sent_artifact() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(92);
    let running = first(t.admit(step_copy(&l, a, "s")).await);
    j.stall_ms.store(300, Ordering::SeqCst);
    let copy = tokio::spawn({
        let (t, req) = (Arc::clone(&t), step_copy(&l, a, "t"));
        async move { t.admit(req).await }
    });
    until(|| committing(&t, "t", a)).await;
    t.note_seen(W, a);
    assert_eq!(running.finish(Outcome::NotSent), Settlement::Sent);
    // DEC-154: the copy waits for the artifact's Sent row; it is handed
    // off only once the fence has acknowledged it Durable, which follows
    // the row reaching disk (review r5 N1).
    let v = match copy.await.unwrap() {
        Verdict::Deferred => {
            until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable))
                .await;
            t.admit(step_copy(&l, a, "t")).await
        }
        v => v,
    };
    assert!(j.stands(&W, &a, "s") && j.stands(&W, &a, "t"));
    assert!(j.load().1.iter().all(|s| s.sent));
    resend(v).finish(Outcome::Sent);
    assert_eq!(j.load().1.len(), 2, "both markers are kept");
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    assert_eq!(
        rec.resolved(),
        vec![(a.to_string(), DispatchResolution::Sent)]
    );
}

/// Review of r1 (fix round), DEC-154: seen sent while its resolution is
/// being written, an artifact keeps its charge, and its Sent row keeps its
/// marker standing over the NotSent row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f2_an_artifact_seen_sent_during_its_resolution_keeps_its_marker() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(95);
    let permit = first(t.admit(step_copy(&l, a, "s")).await);
    j.stall_ms.store(300, Ordering::SeqCst);
    let finish = tokio::task::spawn_blocking(move || permit.finish(Outcome::NotSent));
    until(|| resolving(&t, a)).await;
    t.note_seen(W, a);
    assert_eq!(finish.await.unwrap(), Settlement::MaybeOut);
    // DEC-154: the Sent row makes the marker stand over the NotSent row.
    until(|| j.load().1.len() == 1).await;
    assert!(j.load().1[0].sent);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    assert_eq!(
        rec.resolved(),
        vec![(a.to_string(), DispatchResolution::Sent)]
    );
    j.stall_ms.store(0, Ordering::SeqCst);
    resend(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
}

/// Review of r1 (fix round): an H16 settlement a marker write blocked
/// proceeds when that write fails, although the bytes may be out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f2_a_blocked_slot_settlement_lands_when_the_write_fails() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let slot = NonceSlot {
        identity: [4; 32],
        space: NonceSpace::Identity,
        nonce: 9,
    };
    let signed = |a, step: &str| AdmitRequest {
        kind: ArtifactKind::Transition {
            credits: 10,
            slot: Some(slot),
        },
        scope: scope_of(&l, Some(step)),
        ..transition(&l, a, 10)
    };
    first(t.admit(signed(art(93), "s")).await).finish(Outcome::MaybeSent);
    first(t.admit(signed(art(94), "s")).await).finish(Outcome::MaybeSent);
    j.stall_ms.store(300, Ordering::SeqCst);
    j.fail_step.store(true, Ordering::SeqCst);
    let copy = tokio::spawn({
        let (t, req) = (Arc::clone(&t), signed(art(93), "t"));
        async move { t.admit(req).await }
    });
    until(|| committing(&t, "t", art(93))).await;
    // Blocked by the copy's write.
    t.note_executed(W, art(94), slot);
    assert_eq!(copy.await.unwrap(), Verdict::Deferred);
    until(|| t.dispatch_status(W, art(93)) == Some(DispatchState::NotSent)).await;
    let steps = j.load().1;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].artifact, art(94).0);
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((100, 10)));
}

/// Review of r1 (fix round): a panicking resolution counts as failed, so
/// the next admit retries it instead of deferring forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_f2_a_panicking_resolution_is_retried() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(96);
    j.panic_resolve.store(true, Ordering::SeqCst);
    let permit = first(t.admit(step_copy(&l, a, "s")).await);
    assert_eq!(permit.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert_eq!(t.admit(step_copy(&l, a, "s")).await, Verdict::Deferred);
    j.panic_resolve.store(false, Ordering::SeqCst);
    first(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
    assert_eq!(
        rec.resolved()[0],
        (a.to_string(), DispatchResolution::NotSent)
    );
}

/// Sol r2 R2-F1's probes (DEC-154): a file-backed `dispatch.sqlite` whose
/// `NotSent` and `Sent` writes can be held, and which injects one fault.
struct R2Journal {
    db: dw_appdb::dispatch::DispatchJournal,
    path: std::path::PathBuf,
    fault: R2Fault,
    sent_writes: AtomicU64,
    resolving: AtomicBool,
    sending: AtomicBool,
    /// (`NotSent` released, `Sent` released).
    gate: Mutex<(bool, bool)>,
    wake: std::sync::Condvar,
    _dir: tempfile::TempDir,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum R2Fault {
    None,
    /// The first `Sent` write fails before writing.
    SentLost,
    /// The `NotSent` write commits, then reports an error.
    NotSentDurableThenError,
    /// The `Sent` write is held until released.
    SentHeld,
}

impl R2Journal {
    fn new(fault: R2Fault) -> Arc<Self> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(dw_appdb::dispatch::DISPATCH_DB_FILE);
        let dw_appdb::dispatch::JournalOpen::Ready(db) =
            dw_appdb::dispatch::DispatchJournal::open(&path, 0).unwrap()
        else {
            panic!("a fresh journal");
        };
        Arc::new(Self {
            db,
            path,
            fault,
            sent_writes: AtomicU64::new(0),
            resolving: AtomicBool::new(false),
            sending: AtomicBool::new(false),
            gate: Mutex::new((false, fault != R2Fault::SentHeld)),
            wake: std::sync::Condvar::new(),
            _dir: dir,
        })
    }

    fn release(&self, sent: bool) {
        let mut g = self.gate.lock().unwrap();
        if sent {
            g.1 = true;
        } else {
            g.0 = true;
        }
        self.wake.notify_all();
    }

    fn wait(&self, sent: bool) {
        let mut g = self.gate.lock().unwrap();
        while !(if sent { g.1 } else { g.0 }) {
            g = self.wake.wait(g).unwrap();
        }
    }

    /// What a reopen of the file loads.
    fn reopen(
        &self,
    ) -> (
        Vec<DispatchRow>,
        Vec<StepRow>,
        dw_appdb::dispatch::DispatchJournal,
    ) {
        let dw_appdb::dispatch::JournalOpen::Ready(db) =
            dw_appdb::dispatch::DispatchJournal::open(&self.path, 1).unwrap()
        else {
            panic!("the journal reopens");
        };
        let (rows, steps) = db.load().unwrap();
        (rows, steps, db)
    }
}

struct R2Release(Arc<R2Journal>);

impl Drop for R2Release {
    fn drop(&mut self) {
        self.0.release(false);
        self.0.release(true);
    }
}

impl JournalBackend for R2Journal {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        process: &[u8; 16],
        payload: &[u8],
    ) -> Result<Registered, String> {
        self.db
            .register(wallet, txid, origin, process, payload, 0)
            .map_err(|e| e.to_string())
    }

    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String> {
        self.db
            .mark_dispatching(wallet, txid, 0)
            .map_err(|e| e.to_string())
    }

    fn insert_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
    ) -> Result<(), String> {
        self.db
            .insert_step(wallet, step, artifact, 0)
            .map_err(|e| e.to_string())
    }

    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String> {
        if r == Resolution::NotSent {
            self.resolving.store(true, Ordering::SeqCst);
            self.wait(false);
            self.db
                .resolve(wallet, artifact, r, 0)
                .map_err(|e| e.to_string())?;
            if self.fault == R2Fault::NotSentDurableThenError {
                return Err("injected: durable, then an error".into());
            }
            return Ok(());
        }
        self.resolve_sent(wallet, artifact, &[])
    }

    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String> {
        let n = self.sent_writes.fetch_add(1, Ordering::SeqCst);
        self.sending.store(true, Ordering::SeqCst);
        if self.fault == R2Fault::SentLost && n == 0 {
            return Err("injected: lost before writing".into());
        }
        self.wait(true);
        self.db
            .resolve_sent(wallet, artifact, steps, 0)
            .map_err(|e| e.to_string())
    }

    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String> {
        self.db
            .erase_wallet(wallet, dispatch)
            .map_err(|e| e.to_string())
    }

    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
        self.db
            .erase_wallet_unconditionally(wallet)
            .map_err(|e| e.to_string())
    }
}

/// Sol r2's interleaving: a step First settles definitely unsent while its
/// `NotSent` write is held; the artifact is seen sent meanwhile; then the
/// write is released. Returns the table, journal and lease.
async fn r2_seen_during_resolution(
    fault: R2Fault,
) -> (
    Arc<LeaseTable>,
    Arc<Recorder>,
    Arc<R2Journal>,
    Lease,
    ArtifactId,
) {
    let (t, rec) = table();
    let j = R2Journal::new(fault);
    t.load_journal(Some(j.clone()), vec![], vec![]);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let a = art(240);
    let p = first(t.admit(step_copy(&l, a, "withdrawal/d/submit")).await);
    assert_eq!(j.db.load().unwrap().1.len(), 1);
    let finish = tokio::task::spawn_blocking(move || p.finish(Outcome::NotSent));
    until(|| j.resolving.load(Ordering::SeqCst)).await;
    t.note_seen(W, a);
    j.release(false);
    assert_eq!(finish.await.unwrap(), Settlement::MaybeOut);
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::Sent));
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((100, 10)));
    assert_eq!(
        rec.resolved(),
        vec![(a.to_string(), DispatchResolution::Sent)]
    );
    (t, rec, j, l, a)
}

/// After a reopen of the file, under a dead lease, the step still reads as
/// possibly dispatched: its marker stands and the artifact reads `Sent`.
async fn r2_evidence_survives_a_reopen(j: &R2Journal, a: ArtifactId) {
    let (rows, steps, db) = j.reopen();
    assert_eq!(steps.len(), 1, "the durable marker stands");
    assert!(steps[0].sent);
    let (reloaded, _) = table();
    reloaded.load_journal(Some(Arc::new(db)), rows, steps);
    let dead = begin(&reloaded, W, 0, 100, 0).await.unwrap();
    reloaded.lock(RevokeCause::Lock, status).await;
    assert_eq!(reloaded.dispatch_status(W, a), Some(DispatchState::Sent));
    resend(
        reloaded
            .admit(step_copy(&dead, a, "withdrawal/d/submit"))
            .await,
    )
    .finish(Outcome::Sent);
    // Different bytes for the step read as possibly dispatched.
    assert!(matches!(
        reloaded
            .admit(step_copy(&dead, art(241), "withdrawal/d/submit"))
            .await,
        Verdict::Refused {
            step_possibly_dispatched: true,
            ..
        }
    ));
}

/// Sol r2 R2-F1 probe 1: while the `Sent` row is held, with the `NotSent`
/// row durable and no marker standing on disk, a copy waits.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_f1_a_copy_waits_while_the_sent_row_is_held() {
    let (t, _, j, l, a) = r2_seen_during_resolution(R2Fault::SentHeld).await;
    let _release = R2Release(Arc::clone(&j));
    until(|| j.sending.load(Ordering::SeqCst)).await;
    assert!(
        j.db.load().unwrap().1.is_empty(),
        "the NotSent row is durable"
    );
    assert_eq!(
        t.admit(step_copy(&l, a, "withdrawal/d/submit")).await,
        Verdict::Deferred
    );
    j.release(true);
    // The row lands before the write's completion marks it durable.
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    assert_eq!(j.db.load().unwrap().1.len(), 1);
    let copy = resend(t.admit(step_copy(&l, a, "withdrawal/d/submit")).await);
    t.log_transport(a, copy.id(), Some("withdrawal/d/submit".into()));
    copy.finish(Outcome::Sent);
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
    r2_evidence_survives_a_reopen(&j, a).await;
}

/// Sol r2 R2-F1 probe 2: a `Sent` row lost before writing stays owed; the
/// next copy writes it before it transports, and a reopen finds the
/// evidence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_f1_a_lost_sent_row_is_written_before_any_copy() {
    let (t, _, j, l, a) = r2_seen_during_resolution(R2Fault::SentLost).await;
    let _release = R2Release(Arc::clone(&j));
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Ambiguous)).await;
    assert!(j.db.load().unwrap().1.is_empty());
    let copy = resend(t.admit(step_copy(&l, a, "withdrawal/d/submit")).await);
    assert_eq!(j.sent_writes.load(Ordering::SeqCst), 2, "the copy wrote it");
    assert_eq!(j.db.load().unwrap().1.len(), 1);
    t.log_transport(a, copy.id(), Some("withdrawal/d/submit".into()));
    copy.finish(Outcome::Sent);
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
    r2_evidence_survives_a_reopen(&j, a).await;
}

/// Sol r2 R2-F1 probe 3: a `NotSent` write that committed and then
/// reported an error loses nothing: the `Sent` row outranks it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_f1_a_not_sent_row_committed_then_failed_keeps_the_evidence() {
    let (t, _, j, l, a) = r2_seen_during_resolution(R2Fault::NotSentDurableThenError).await;
    let _release = R2Release(Arc::clone(&j));
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    assert_eq!(j.db.load().unwrap().1.len(), 1);
    let copy = resend(t.admit(step_copy(&l, a, "withdrawal/d/submit")).await);
    t.log_transport(a, copy.id(), Some("withdrawal/d/submit".into()));
    copy.finish(Outcome::Sent);
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
    r2_evidence_survives_a_reopen(&j, a).await;
}

/// The control: nothing fails; the evidence survives a reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_f1_sent_evidence_survives_a_reopen() {
    let (_t, _, j, _l, a) = r2_seen_during_resolution(R2Fault::None).await;
    let _release = R2Release(Arc::clone(&j));
    until(|| j.db.load().unwrap().1.len() == 1).await;
    r2_evidence_survives_a_reopen(&j, a).await;
}

/// DEC-154 (3): a `NotSent` write that failed may have committed, so its
/// markers no longer count as standing: copies wait until a retry lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_f1_a_failed_not_sent_write_counts_as_committed() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(97);
    j.resolve_durable_err.store(true, Ordering::SeqCst);
    let permit = first(t.admit(step_copy(&l, a, "s")).await);
    assert_eq!(permit.finish(Outcome::NotSent), Settlement::MaybeOut);
    assert!(j.load().1.is_empty(), "the NotSent row committed");
    // Each copy retries the resolution first, and defers while it fails.
    assert_eq!(t.admit(step_copy(&l, a, "s")).await, Verdict::Deferred);
    assert_eq!(t.admit(step_copy(&l, a, "t")).await, Verdict::Deferred);
    assert!(rec.resolved().is_empty());
    assert_eq!(budget(&l, BudgetPurpose::Credits), Some((1_000, 10)));
    j.resolve_durable_err.store(false, Ordering::SeqCst);
    first(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
    assert_eq!(
        rec.resolved()[0],
        (a.to_string(), DispatchResolution::NotSent)
    );
    assert_eq!(j.load().1.len(), 1, "the new First's marker stands");
}

/// Review r2 H1: a marker whose write failed is superseded by a `NotSent`
/// row and comes back with a sighting; the sighting's `Sent` row carries
/// it (DEC-163), so the next copy of its step finds it standing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_h1_a_superseded_marker_that_never_landed_is_written_again() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(98);
    let permit = first(t.admit(step_copy(&l, a, "s")).await);
    j.fail_step.store(true, Ordering::SeqCst);
    let _ = t.admit(step_copy(&l, a, "t")).await;
    j.fail_step.store(false, Ordering::SeqCst);
    assert_eq!(
        t.with_j(|i, _| i.fence.mark(&W, "t", &a)),
        Some(fence::Mark::Ambiguous)
    );
    permit.finish(Outcome::NotSent);
    until(|| {
        rec.resolved()
            .contains(&(a.to_string(), DispatchResolution::NotSent))
    })
    .await;
    assert!(j.load().1.is_empty(), "the NotSent row supersedes s");
    t.note_seen(W, a);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    assert!(j.stands(&W, &a, "s"), "the Sent row makes s stand");
    assert!(j.stands(&W, &a, "t"), "the Sent row carried t");
    assert_eq!(
        t.with_j(|i, _| i.fence.mark(&W, "t", &a)),
        Some(fence::Mark::Durable)
    );
    resend(t.admit(step_copy(&l, a, "t")).await).finish(Outcome::Sent);
}

/// Review r2 M2: a `Sent` row whose write failed (lost, landed and then an
/// error, or panicked) is begun again by the next journal write that
/// lands, or the next sighting, with no copy of the artifact needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_m2_a_failed_sent_row_is_retried_without_a_copy() {
    for (n, fault) in [0u8, 1, 2].into_iter().enumerate() {
        let (t, _) = table();
        let j = with_journal(&t);
        let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
        let a = art(100 + n as u8);
        let flag = match fault {
            0 => &j.fail_sent,
            1 => &j.sent_durable_err,
            _ => &j.panic_sent,
        };
        first(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
        flag.store(true, Ordering::SeqCst);
        t.note_seen(W, a);
        until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Ambiguous))
            .await;
        flag.store(false, Ordering::SeqCst);
        if fault == 2 {
            // A second sighting begins it again.
            t.note_seen(W, a);
        } else {
            // Another artifact's marker landing begins it again.
            first(t.admit(step_copy(&l, art(110 + n as u8), "u")).await).finish(Outcome::Sent);
        }
        until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable))
            .await;
        let sent = j
            .load()
            .1
            .into_iter()
            .find(|r| r.artifact == a.0)
            .is_some_and(|r| r.sent);
        assert!(sent, "fault {fault}: the Sent row is on disk");
    }
}

/// Review r2 L2: another wallet's sighting of an artifact neither marks it
/// sent nor keeps this wallet's entry from settling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_l2_another_wallets_sighting_is_not_this_ones() {
    let (t, rec) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(99);
    let permit = first(t.admit(step_copy(&l, a, "s")).await);
    t.note_seen(W2, a);
    assert_ne!(t.dispatch_status(W, a), Some(DispatchState::Sent));
    assert_eq!(t.with_j(|i, _| i.fence.evidence_mark(&W2, &a)), None);
    permit.finish(Outcome::NotSent);
    until(|| {
        rec.resolved()
            .contains(&(a.to_string(), DispatchResolution::NotSent))
    })
    .await;
    assert!(j.load().1.is_empty());
}

/// Review r2 L1: closing the journal waits for the writes running, so none
/// lands after the next session's sweep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_l1_closing_the_journal_waits_for_its_writes() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(97);
    first(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
    j.stall_ms.store(300, Ordering::SeqCst);
    t.note_seen(W, a);
    assert!(matches!(
        t.with_j(|i, _| i.fence.evidence_mark(&W, &a)),
        Some(fence::Mark::Committing { .. })
    ));
    t.close_journal().await;
    assert!(
        j.load().1.iter().any(|r| r.artifact == a.0 && r.sent),
        "the Sent row landed before close returned"
    );
}

/// Review O-5: closing the journal waits for a running `register` write,
/// so no `Unsent` row lands after it returns.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn o5_closing_the_journal_waits_for_a_register_write() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 1_000, 0, 0).await.unwrap();
    j.stall_ms.store(300, Ordering::SeqCst);
    let registering = tokio::spawn({
        let l = l.clone();
        async move { register(&l, art(1), 100).await }
    });
    while !t.with_j(|i, _| i.fence.registering.contains_key(&(W, art(1)))) {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    t.close_journal().await;
    assert!(t.with_j(|i, _| i.fence.registering.is_empty()));
    assert_eq!(
        j.rows.lock().unwrap().len(),
        1,
        "the row landed before close returned"
    );
    registering.await.unwrap().unwrap();
}

/// Review O-8: a flow task registered after close's `abort_tasks` ran is
/// aborted at once.
#[tokio::test(start_paused = true)]
async fn o8_a_task_registered_after_close_is_aborted() {
    let (t, _) = table();
    let l = begin(&t, W, 0, 1, 0).await.unwrap();
    t.with_j(|i, _| i.closed = true);
    let task = tokio::spawn(std::future::pending::<()>());
    l.register_task(task.abort_handle());
    assert!(task.await.unwrap_err().is_cancelled());
}

/// Review r2 M2: a `Sent` row left owed with no later write or sighting
/// gets a last try when the journal closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_m2_closing_the_journal_retries_an_owed_sent_row() {
    let (t, _) = table();
    let j = with_journal(&t);
    let l = begin(&t, W, 0, 1_000, 0).await.unwrap();
    let a = art(95);
    first(t.admit(step_copy(&l, a, "s")).await).finish(Outcome::Sent);
    j.fail_sent.store(true, Ordering::SeqCst);
    t.note_seen(W, a);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Ambiguous)).await;
    j.fail_sent.store(false, Ordering::SeqCst);
    t.close_journal().await;
    assert!(j.load().1.iter().any(|r| r.artifact == a.0 && r.sent));
}

/// A file-backed journal for the removal probes (Sol r3, r4): the erase,
/// the `Sent` row and the `NotSent` row can each be held before they write.
struct R3Journal {
    db: dw_appdb::dispatch::DispatchJournal,
    path: std::path::PathBuf,
    erase_entered: AtomicBool,
    /// The erase commits, then reports an error.
    erase_err: AtomicBool,
    sending: AtomicBool,
    not_sending: AtomicBool,
    /// Which of [`ERASE`], [`SENT`], [`NOT_SENT`] may write.
    gate: Mutex<[bool; 3]>,
    wake: std::sync::Condvar,
    _dir: tempfile::TempDir,
}

const ERASE: usize = 0;
const SENT: usize = 1;
const NOT_SENT: usize = 2;

impl R3Journal {
    fn new(held: &[usize]) -> Arc<Self> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(dw_appdb::dispatch::DISPATCH_DB_FILE);
        let dw_appdb::dispatch::JournalOpen::Ready(db) =
            dw_appdb::dispatch::DispatchJournal::open(&path, 0).unwrap()
        else {
            panic!("a fresh journal");
        };
        Arc::new(Self {
            db,
            path,
            erase_entered: AtomicBool::new(false),
            erase_err: AtomicBool::new(false),
            sending: AtomicBool::new(false),
            not_sending: AtomicBool::new(false),
            gate: Mutex::new([ERASE, SENT, NOT_SENT].map(|g| !held.contains(&g))),
            wake: std::sync::Condvar::new(),
            _dir: dir,
        })
    }

    fn release(&self, g: usize) {
        self.gate.lock().unwrap()[g] = true;
        self.wake.notify_all();
    }

    fn wait(&self, g: usize) {
        let mut open = self.gate.lock().unwrap();
        while !open[g] {
            open = self.wake.wait(open).unwrap();
        }
    }

    /// The markers standing in a reopen of the file, and its log.
    fn reopened(&self) -> (Vec<StepRow>, Vec<StepLogRow>) {
        let dw_appdb::dispatch::JournalOpen::Ready(db) =
            dw_appdb::dispatch::DispatchJournal::open(&self.path, 1).unwrap()
        else {
            panic!("the journal reopens");
        };
        (db.load().unwrap().1, db.step_log().unwrap())
    }
}

struct R3Release(Arc<R3Journal>);

impl Drop for R3Release {
    fn drop(&mut self) {
        for g in [ERASE, SENT, NOT_SENT] {
            self.0.release(g);
        }
    }
}

impl JournalBackend for R3Journal {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        process: &[u8; 16],
        payload: &[u8],
    ) -> Result<Registered, String> {
        self.db
            .register(wallet, txid, origin, process, payload, 0)
            .map_err(|e| e.to_string())
    }

    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String> {
        self.db
            .mark_dispatching(wallet, txid, 0)
            .map_err(|e| e.to_string())
    }

    fn insert_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
    ) -> Result<(), String> {
        self.db
            .insert_step(wallet, step, artifact, 0)
            .map_err(|e| e.to_string())
    }

    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String> {
        if r != Resolution::NotSent {
            return self.resolve_sent(wallet, artifact, &[]);
        }
        self.not_sending.store(true, Ordering::SeqCst);
        self.wait(NOT_SENT);
        self.db
            .resolve(wallet, artifact, r, 0)
            .map_err(|e| e.to_string())
    }

    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String> {
        self.sending.store(true, Ordering::SeqCst);
        self.wait(SENT);
        self.db
            .resolve_sent(wallet, artifact, steps, 0)
            .map_err(|e| e.to_string())
    }

    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String> {
        self.erase_entered.store(true, Ordering::SeqCst);
        self.wait(ERASE);
        let erased = self
            .db
            .erase_wallet(wallet, dispatch)
            .map_err(|e| e.to_string())?;
        if self.erase_err.load(Ordering::SeqCst) {
            return Err("injected: erased, then an error".into());
        }
        Ok(erased)
    }

    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
        self.erase_entered.store(true, Ordering::SeqCst);
        self.wait(ERASE);
        self.db
            .erase_wallet_unconditionally(wallet)
            .map_err(|e| e.to_string())
    }
}

/// A table on a fresh [`R3Journal`] with `held` gates, a lease of `W`, and
/// a First of `a`'s step `s` that finished `outcome`, its resolution
/// landed unless held.
async fn r3_setup(
    held: &[usize],
    a: ArtifactId,
    outcome: Outcome,
) -> (Arc<LeaseTable>, Arc<R3Journal>, R3Release, Lease) {
    let (t, rec) = table();
    let j = R3Journal::new(held);
    let release = R3Release(Arc::clone(&j));
    t.load_journal(Some(j.clone() as Arc<dyn JournalBackend>), vec![], vec![]);
    let l = begin(&t, W, 0, 100, 0).await.unwrap();
    let p = first(t.admit(step_copy(&l, a, "s")).await);
    if outcome != Outcome::NotSent {
        t.log_transport(a, p.id(), Some("s".into()));
        p.finish(outcome);
    } else if held.contains(&NOT_SENT) {
        tokio::task::spawn_blocking(move || p.finish(outcome));
        until(|| j.not_sending.load(Ordering::SeqCst)).await;
    } else {
        p.finish(outcome);
        until(|| {
            rec.resolved()
                .contains(&(a.to_string(), DispatchResolution::NotSent))
        })
        .await;
    }
    (t, j, release, l)
}

/// The wallet's removal: its freeze, drained, and the erase of its rows.
async fn r3_remove(t: &Arc<LeaseTable>) -> super::lock::Freeze {
    let mut removal = t.freeze(RevokeCause::WalletRemoved, Scope::Wallet(W), false);
    removal.drained().await;
    t.erase_wallet_rows(W).await;
    removal
}

/// `r3_remove` in a task, once its erase is held in the journal.
async fn r3_removal_erasing(
    t: &Arc<LeaseTable>,
    j: &R3Journal,
) -> tokio::task::JoinHandle<super::lock::Freeze> {
    let t = Arc::clone(t);
    let erase = tokio::spawn(async move { r3_remove(&t).await });
    until(|| j.erase_entered.load(Ordering::SeqCst)).await;
    erase
}

/// Sol r3 R3-F1 probe 1: a step that ended possibly sent and sits idle
/// keeps its evidence through the wallet's removal (DEC-160): its marker
/// stands, so the predicate keeps its rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_f1_removal_keeps_an_idle_possibly_out_step() {
    let a = art(242);
    let (t, j, _release, _l) = r3_setup(&[], a, Outcome::MaybeSent).await;
    r3_remove(&t).await;
    let (standing, log) = j.reopened();
    assert!(!log.is_empty(), "the removal kept the step's rows");
    assert_eq!(standing.len(), 1, "its marker stands on reopen");
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::MaybeSent));
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
}

/// Sol r3 R3-F1 probe 2: a copy admitted while the removal's erase runs
/// Resends on the marker the predicate keeps, which stands at its
/// transport and after a reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_f1_a_copy_during_the_erase_transports_on_standing_evidence() {
    let a = art(243);
    let (t, j, _release, l) = r3_setup(&[ERASE], a, Outcome::MaybeSent).await;
    let erase = r3_removal_erasing(&t, &j).await;
    let copy = resend(t.admit(step_copy(&l, a, "s")).await);
    j.release(ERASE);
    let _removal = erase.await.unwrap();
    let mut standing = 0;
    t.hand_off(None, async { Ok::<_, ()>(()) }, |()| async {
        standing = j.db.load().unwrap().1.len();
        t.log_transport(a, copy.id(), Some("s".into()));
    })
    .await
    .unwrap();
    copy.finish(Outcome::MaybeSent);
    assert!(standing >= 1, "a marker stands at the transport");
    assert_eq!(j.reopened().0.len(), 1, "and after a reopen");
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
}

/// DEC-163 (3): a sighting's `Sent` row still running when the removal's
/// delete erases the artifact's settled rows lands with its marker, and
/// memory keeps the newer evidence the delete did not see. `ForgetNewer`
/// forgets it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_f1_a_sent_row_running_across_the_erase_keeps_its_markers() {
    let a = art(244);
    let (t, j, _release, _l) = r3_setup(&[SENT], a, Outcome::NotSent).await;
    t.note_seen(W, a);
    until(|| j.sending.load(Ordering::SeqCst)).await;
    let _removal = r3_remove(&t).await;
    assert!(
        j.reopened().1.is_empty(),
        "the delete took the settled rows"
    );
    j.release(SENT);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    let (standing, _) = j.reopened();
    assert_eq!(standing.len(), 1, "the marker stands with its Sent row");
    assert!(standing[0].sent);
    assert_eq!(t.dispatch_status(W, a), Some(DispatchState::Sent));
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
}

/// A sighting while the removal's erase is held writes its bundle at once:
/// the delete then finds a final row and keeps everything, and a reopen
/// before any copy reads the marker sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r3_f1_a_sighting_during_the_erase_keeps_its_evidence() {
    let a = art(245);
    let (t, j, _release, _l) = r3_setup(&[ERASE], a, Outcome::NotSent).await;
    let erase = r3_removal_erasing(&t, &j).await;
    t.note_seen(W, a);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    j.release(ERASE);
    let _removal = erase.await.unwrap();
    let (standing, _) = j.reopened();
    assert_eq!(standing.len(), 1, "the marker stands on reopen");
    assert!(standing[0].sent);
    super::stress_tests::check(&t.inspect(|i| i.log.clone())).unwrap();
}

/// Sol r4 R4-F1 (`review_r4_failed_committed_erase_then_sighting_before_any_copy`):
/// an erase that commits and then reports an error, then a sighting. Its
/// `Sent` row carries the marker the delete took, though memory believes it
/// durable, so a reopen before any copy or repair reads the step sent: a
/// dead lease Resends identical bytes and refuses others as possibly
/// dispatched. `BundleOwedOnly` leaves the row bare.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_f1_a_sighting_after_a_failed_erase_keeps_its_step_before_any_copy() {
    let a = art(247);
    let (t, j, _release, _l) = r3_setup(&[], a, Outcome::NotSent).await;
    j.erase_err.store(true, Ordering::SeqCst);
    let _removal = r3_remove(&t).await;
    assert!(j.reopened().1.is_empty(), "the injected erase committed");
    t.note_seen(W, a);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    // Nothing admits, transports, repairs or closes before the reopen.
    let (standing, _) = j.reopened();
    assert_eq!(standing.len(), 1, "the Sent row carries its step");
    assert!(standing[0].sent);
    let (reload, _) = table();
    let dw_appdb::dispatch::JournalOpen::Ready(db) =
        dw_appdb::dispatch::DispatchJournal::open(&j.path, 2).unwrap()
    else {
        panic!("the journal reopens");
    };
    let (rows, steps) = db.load().unwrap();
    reload.load_journal(Some(Arc::new(db)), rows, steps);
    let dead = begin(&reload, W, 0, 100, 0).await.unwrap();
    reload.lock(RevokeCause::Lock, status).await;
    resend(reload.admit(step_copy(&dead, a, "s")).await).finish(Outcome::Sent);
    assert!(matches!(
        reload.admit(step_copy(&dead, art(248), "s")).await,
        Verdict::Refused {
            step_possibly_dispatched: true,
            ..
        }
    ));
}

/// DEC-163 (1): the bundle is one transaction. A sighting during the
/// `NotSent` write begins it while the marker still stands; the `NotSent`
/// row then lands and the removal's delete takes both before the bundle
/// writes. Written as one transaction it appends the marker again with its
/// row; as separate statements (`SplitBundle`) the marker statement found
/// it standing and the row lands bare.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_f1_a_bundle_overtaken_by_its_not_sent_row_and_the_erase_keeps_its_marker() {
    let a = art(249);
    let (t, j, _release, _l) = r3_setup(&[SENT, NOT_SENT], a, Outcome::NotSent).await;
    t.note_seen(W, a);
    until(|| j.sending.load(Ordering::SeqCst)).await;
    j.release(NOT_SENT);
    until(|| {
        j.db.step_log()
            .unwrap()
            .iter()
            .any(|r| r.event == StepEvent::Resolved(Resolution::NotSent))
    })
    .await;
    let _removal = r3_remove(&t).await;
    assert!(
        j.reopened().1.is_empty(),
        "the delete took marker and NotSent"
    );
    j.release(SENT);
    until(|| t.with_j(|i, _| i.fence.evidence_mark(&W, &a)) == Some(fence::Mark::Durable)).await;
    let (standing, _) = j.reopened();
    assert_eq!(standing.len(), 1, "the Sent row lands with its marker");
    assert!(standing[0].sent);
}

/// DEC-163 (3): a removal whose caller is dropped while its delete runs
/// still applies the delete's outcome, and closing the journal waits for
/// it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_closing_the_journal_waits_for_an_abandoned_erase() {
    let a = art(250);
    let (t, j, _release, _l) = r3_setup(&[ERASE], a, Outcome::MaybeSent).await;
    let erase = r3_removal_erasing(&t, &j).await;
    erase.abort();
    let _ = erase.await;
    let close = tokio::spawn({
        let t = Arc::clone(&t);
        async move { t.close_journal().await }
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!close.is_finished(), "close joins the erase");
    j.release(ERASE);
    close.await.unwrap();
    assert_eq!(j.reopened().0.len(), 1);
    let log = t.inspect(|i| i.log.clone());
    assert!(
        log.iter()
            .any(|e| matches!(e, super::stress_tests::LogEvent::Erase { done: true, .. }))
    );
}
