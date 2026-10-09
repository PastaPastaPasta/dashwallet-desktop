//! The dispatch fence's engine side (E0-04 §5, §6, §7.6, §16.6).
//!
//! Every decision about an artifact is one J step. The durable writes (a
//! `Dispatching` record, a step marker) are spawned on the blocking pool
//! from inside that step, and the spawned task owns the resolution of the
//! `Committing` entry it was spawned for, so a dropped `admit` never leaves
//! an entry without its writer (§5.4).
//!
//! P2a installs it for engine-owned hand-offs (`TxDraft` under a lease) and
//! tests the library's side against a fake library; Mode A's PR (P3/P4)
//! and Mode B's call permits (P2b) call the same `register`, `admit`,
//! `abandon`, `proof_wait_started` and `chainlock_fallback`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::time::Instant;

use super::budget::Charge;
use super::table::{Effects, Entry, Inner, LeaseTable};
use super::{ArtifactId, DispatchScope, FlowOutcome, LeaseError, LeaseId, Origin};
use crate::platform::flows::{BudgetPurpose, DispatchResolution, DispatchResolved, DispatchState};
use crate::{EngineEvent, NoticeCode, WalletId};

/// The durable medium of the journal (§6.2): `dispatch.sqlite`, or a fake
/// with injected faults in tests. Every call is a short blocking write.
pub(crate) trait JournalBackend: Send + Sync + 'static {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        process: &[u8; 16],
        payload: &[u8],
    ) -> Result<dw_appdb::dispatch::Registered, String>;
    /// `Ok(true)`: the record is at `Dispatching` (or `PreFence`).
    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String>;
    fn insert_step(&self, wallet: &[u8; 32], step: &str, artifact: &[u8; 32])
    -> Result<(), String>;
    fn erase_wallet(&self, wallet: &[u8; 32]) -> Result<(), String>;
}

impl JournalBackend for dw_appdb::dispatch::DispatchJournal {
    fn register(
        &self,
        wallet: &[u8; 32],
        txid: &[u8; 32],
        origin: &LeaseId,
        process: &[u8; 16],
        payload: &[u8],
    ) -> Result<dw_appdb::dispatch::Registered, String> {
        self.register(
            wallet,
            txid,
            origin,
            process,
            payload,
            crate::events::unix_now(),
        )
        .map_err(|e| e.to_string())
    }

    fn mark_dispatching(&self, wallet: &[u8; 32], txid: &[u8; 32]) -> Result<bool, String> {
        self.mark_dispatching(wallet, txid, crate::events::unix_now())
            .map_err(|e| e.to_string())
    }

    fn insert_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
    ) -> Result<(), String> {
        self.insert_step(wallet, step, artifact, crate::events::unix_now())
            .map_err(|e| e.to_string())
    }

    fn erase_wallet(&self, wallet: &[u8; 32]) -> Result<(), String> {
        self.erase_wallet(wallet).map_err(|e| e.to_string())
    }
}

/// The open journal; `None` before the load and after close. Read outside
/// J, never under it (H1).
#[derive(Default)]
pub(crate) struct JournalSlot(Mutex<Option<Arc<dyn JournalBackend>>>);

impl JournalSlot {
    pub(crate) fn get(&self) -> Option<Arc<dyn JournalBackend>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    pub(crate) fn set(&self, backend: Option<Arc<dyn JournalBackend>>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = backend;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum JournalState {
    /// `admit` answers `Deferred` for tracked rows, `register` fails.
    #[default]
    NotLoaded,
    Ready,
    /// Unopenable or a newer schema (§6.4).
    Unavailable,
}

/// A registered artifact's entry (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reg {
    PreFence,
    /// `origin` is `None` only for an entry loaded without one.
    Unsent {
        origin: Option<LeaseId>,
        charge: Option<Charge>,
    },
    Committing {
        owner: u64,
    },
    Dispatching,
    Ambiguous,
    Revoked,
}

/// A resumable step's marker for one artifact (§7.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Committing { owner: u64 },
    Durable,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowlessState {
    Admitted {
        running: u32,
        possibly_out: bool,
        /// H16: a different transition this engine signed was proved
        /// executed in this one's nonce slot.
        slot_taken: bool,
    },
    /// A Core send settled definitely unsent: later copies are refused.
    Revoked,
    /// A state transition settled definitely unsent.
    NotSent,
}

#[derive(Debug, Clone)]
pub(crate) struct Rowless {
    wallet: WalletId,
    pub(crate) state: RowlessState,
    kind: ArtifactKind,
    /// The charge the settlement refunds: (lease, purpose, charge).
    charge: Option<(LeaseId, BudgetPurpose, Charge)>,
}

impl Rowless {
    fn admitted(
        wallet: WalletId,
        kind: ArtifactKind,
        charge: Option<(LeaseId, BudgetPurpose, Charge)>,
        possibly_out: bool,
    ) -> Self {
        Rowless {
            wallet,
            state: RowlessState::Admitted {
                running: 1,
                possibly_out,
                slot_taken: false,
            },
            kind,
            charge,
        }
    }
}

/// A `TxDraft`'s Spend charge, bound to its txid at signing (§4.2).
#[derive(Debug, Clone, Copy)]
struct SpendCharge {
    lease: LeaseId,
    wallet: WalletId,
    charge: Charge,
}

#[derive(Debug, Clone, Copy)]
struct Attempt {
    wallet: WalletId,
    artifact: ArtifactId,
    lease: Option<LeaseId>,
    /// A permit's deadline; guards have none.
    deadline: Option<Instant>,
    registered: bool,
}

#[derive(Default)]
pub(crate) struct FenceState {
    pub(crate) journal: JournalState,
    entries: HashMap<(WalletId, ArtifactId), Reg>,
    /// The lease that registered each entry of this process.
    origins: HashMap<(WalletId, ArtifactId), LeaseId>,
    /// Registrations whose write runs; `true` once abandoned meanwhile.
    registering: HashMap<(WalletId, ArtifactId), bool>,
    rowless: HashMap<ArtifactId, Rowless>,
    steps: HashMap<(WalletId, String), HashMap<ArtifactId, Mark>>,
    spend: HashMap<ArtifactId, SpendCharge>,
    /// Every artifact an attempt finished `Sent` for; only grows.
    sent: HashSet<ArtifactId>,
    attempts: HashMap<u64, Attempt>,
    next: u64,
}

/// What kind of artifact a hand-off carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactKind {
    /// `asset_lock`: special type 8, which must be registered (§4.4).
    CoreTx { asset_lock: bool },
    /// `credits`: the explicit credits it moves plus its fee bound;
    /// `slot`: its identity, nonce space and nonce (H16).
    Transition {
        credits: u64,
        slot: Option<NonceSlot>,
    },
}

impl ArtifactKind {
    /// What a definitely-unsent artifact of this kind leaves behind.
    fn tombstone(self) -> RowlessState {
        match self {
            ArtifactKind::CoreTx { .. } => RowlessState::Revoked,
            ArtifactKind::Transition { .. } => RowlessState::NotSent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NonceSpace {
    /// Withdrawals, transfers, key updates.
    Identity,
    /// Documents of one contract (DashPay, DPNS).
    Contract([u8; 32]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NonceSlot {
    pub identity: [u8; 32],
    pub space: NonceSpace,
    pub nonce: u64,
}

/// One hand-off request (§5.3).
#[derive(Debug, Clone)]
pub struct AdmitRequest {
    pub wallet: WalletId,
    pub artifact: ArtifactId,
    pub kind: ArtifactKind,
    /// The artifact has a persisted row (an asset lock).
    pub tracked_row: bool,
    /// The task-local scope; `None` when unscoped.
    pub scope: Option<DispatchScope>,
}

/// An attempt's own outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Sent,
    MaybeSent,
    /// A definite transport rejection: these bytes did not leave.
    NotSent,
}

/// The artifact's settlement, which every release decision follows (L6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settlement {
    DefinitelyUnsent,
    MaybeOut,
    Sent,
}

#[derive(Debug)]
pub enum Verdict {
    /// Hand off now, under the permit, until its deadline.
    First(DispatchPermit),
    /// An unleased origin: hand off now, no permit.
    FirstUnleased(AttemptGuard),
    /// A recorded possible dispatch: hand off now, no permit.
    Resend(AttemptGuard),
    /// Provably never sent. `cleanup`: this caller releases (the first
    /// refusal only). `step_possibly_dispatched`: an earlier artifact of the
    /// same resumable step may have been (H10), so the flow reports
    /// MaybeSent.
    Refused {
        cleanup: bool,
        step_possibly_dispatched: bool,
    },
    /// Not now, outcome unknown (MaybeSent): keep the row and the
    /// reservation.
    Deferred,
}

/// `abandon`'s answer (§5.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Abandon {
    Revoked { cleanup: bool },
    Committed,
}

/// Why `register` refused (§5.5); the build aborts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegisterError {
    #[error(transparent)]
    Lease(#[from] LeaseError),
    #[error("registered under another origin")]
    OtherOrigin,
    #[error("dispatch journal: {0}")]
    Journal(String),
}

/// A First's permit (§5.3). Dropping it is `finish(MaybeSent)`.
pub struct DispatchPermit {
    table: Arc<LeaseTable>,
    id: u64,
    deadline: Instant,
    done: bool,
}

impl std::fmt::Debug for DispatchPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchPermit")
            .field("id", &self.id)
            .finish()
    }
}

impl DispatchPermit {
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    #[cfg(test)]
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub fn finish(mut self, outcome: Outcome) -> Settlement {
        self.done = true;
        self.table.finish(self.id, outcome)
    }

    /// Ends the permit without an attempt (its transport was never called).
    fn discard(mut self) {
        self.done = true;
        self.table.discard(self.id);
    }
}

impl Drop for DispatchPermit {
    fn drop(&mut self) {
        if !self.done {
            self.table.finish(self.id, Outcome::MaybeSent);
        }
    }
}

/// An unpermitted attempt (an unleased First or a Resend). Dropping it is
/// `finish(MaybeSent)`.
pub struct AttemptGuard {
    table: Arc<LeaseTable>,
    id: u64,
    done: bool,
}

impl std::fmt::Debug for AttemptGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AttemptGuard")
            .field("id", &self.id)
            .finish()
    }
}

impl AttemptGuard {
    #[cfg(test)]
    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    pub fn finish(mut self, outcome: Outcome) -> Settlement {
        self.done = true;
        self.table.finish(self.id, outcome)
    }
}

impl Drop for AttemptGuard {
    fn drop(&mut self) {
        if !self.done {
            self.table.finish(self.id, Outcome::MaybeSent);
        }
    }
}

impl FenceState {
    fn attempt(
        &mut self,
        (wallet, artifact): (WalletId, ArtifactId),
        lease: Option<LeaseId>,
        deadline: Option<Instant>,
        registered: bool,
    ) -> u64 {
        self.next += 1;
        self.attempts.insert(
            self.next,
            Attempt {
                wallet,
                artifact,
                lease,
                deadline,
                registered,
            },
        );
        self.next
    }

    /// §16.5: a funding of the flow committed and not definitely unsent.
    pub(crate) fn funds_committed(&self, e: &Entry) -> bool {
        e.history.iter().any(|id| {
            matches!(
                self.entries.get(&(e.wallet, *id)),
                Some(Reg::Committing { .. } | Reg::Dispatching | Reg::Ambiguous | Reg::PreFence)
            )
        })
    }

    /// A committed artifact's outcome, from the history and the journal
    /// (§8.4).
    pub(crate) fn outcome(&self, wallet: &WalletId, id: &ArtifactId) -> FlowOutcome {
        if self.sent.contains(id) {
            return FlowOutcome::Sent;
        }
        match self.entries.get(&(*wallet, *id)) {
            Some(Reg::Dispatching | Reg::PreFence | Reg::Ambiguous) => {
                return FlowOutcome::WillBeSent;
            }
            Some(Reg::Committing { .. }) => return FlowOutcome::MaybeSent,
            Some(Reg::Unsent { .. } | Reg::Revoked) => return FlowOutcome::Cancelled,
            None => {}
        }
        match self.rowless.get(id).map(|r| r.state) {
            Some(RowlessState::Revoked | RowlessState::NotSent) => FlowOutcome::Cancelled,
            // Admitted, a step marker, or a commit whose write never
            // resolved: possibly out.
            _ => FlowOutcome::MaybeSent,
        }
    }

    /// Permits in flight: (id, deadline, lease).
    pub(crate) fn permits(&self) -> impl Iterator<Item = (u64, Instant, Option<LeaseId>)> + '_ {
        self.attempts
            .iter()
            .filter_map(|(id, a)| a.deadline.map(|d| (*id, d, a.lease)))
    }

    pub(crate) fn has_attempt(&self, id: u64) -> bool {
        self.attempts.contains_key(&id)
    }

    fn step_marked(&self, wallet: &WalletId, step: &str) -> bool {
        self.steps
            .get(&(*wallet, step.to_owned()))
            .is_some_and(|m| !m.is_empty())
    }

    fn live(inner_leases: &HashMap<LeaseId, Entry>, lease: &LeaseId, wallet: &WalletId) -> bool {
        inner_leases
            .get(lease)
            .is_some_and(|e| e.wallet == *wallet && e.state.live())
    }
}

/// Emits `DispatchResolved` for `id`.
fn resolved(
    t: &LeaseTable,
    fx: &mut Effects,
    wallet: &WalletId,
    id: &ArtifactId,
    r: DispatchResolution,
) {
    fx.emit(EngineEvent::DispatchResolved {
        network: t.network.clone(),
        resolved: DispatchResolved {
            wallet_id: wallet.to_string(),
            artifact: id.to_string(),
            resolution: r,
        },
    });
}

/// The row-less artifact `id` settles definitely unsent: refund, tombstone
/// (§5.5), `DispatchResolved(NotSent)`.
fn settle_unsent(t: &LeaseTable, i: &mut Inner, fx: &mut Effects, id: &ArtifactId) {
    let Some(r) = i.fence.rowless.get_mut(id) else {
        return;
    };
    r.state = r.kind.tombstone();
    let wallet = r.wallet;
    if let Some((lease, purpose, charge)) = r.charge.take()
        && let Some(e) = i.leases.get_mut(&lease)
    {
        e.refund(purpose, charge);
        fx.changed(lease);
    }
    i.fence.spend.remove(id);
    resolved(t, fx, &wallet, id, DispatchResolution::NotSent);
}

/// A registered `Unsent` artifact that will never be sent: `Revoked`, its
/// Funding charge refunded to the origin lease, and its provisional outcome
/// resolved `NotSent` (§4.6, also for a reload's refusal).
fn revoke_unsent(
    t: &LeaseTable,
    i: &mut Inner,
    fx: &mut Effects,
    key: (WalletId, ArtifactId),
    charge: Option<Charge>,
) {
    i.fence.entries.insert(key, Reg::Revoked);
    resolved(t, fx, &key.0, &key.1, DispatchResolution::NotSent);
    if let (Some(charge), Some(lease)) = (charge, i.fence.origins.get(&key))
        && let Some(e) = i.leases.get_mut(lease)
    {
        e.refund(BudgetPurpose::Funding, charge);
        fx.changed(*lease);
    }
}

/// A decision `admit` reached in its J step.
enum Step {
    Done(Verdict),
    /// A durable write was spawned; `then` decides once it returns.
    Write {
        write: tokio::task::JoinHandle<bool>,
        then: After,
    },
}

enum After {
    /// A First: in time → `First(permit)`; else `Deferred`.
    /// `placeholder`: `decide` inserted its row-less entry already.
    First {
        permit: DispatchPermit,
        placeholder: bool,
    },
    /// A retry of an ambiguous write: ok → `Resend`.
    Resend {
        registered: bool,
        rowless: Option<Rowless>,
    },
}

/// Inserts `r` for `id`, or joins an entry already admitted.
fn admit_rowless(i: &mut Inner, id: ArtifactId, r: Rowless) {
    match i.fence.rowless.get_mut(&id).map(|e| &mut e.state) {
        Some(RowlessState::Admitted {
            running,
            possibly_out,
            ..
        }) => {
            *running += 1;
            *possibly_out = true;
        }
        _ => {
            i.fence.rowless.insert(id, r);
        }
    }
}

/// A First waiting for its durable write. Dropped before `hand_out` (a
/// late or failed write, or the admit future dropped), the attempt never
/// ran: the permit is discarded and a step-scoped First's row-less entry
/// released. The entry goes unless another attempt joined it; a joined
/// entry may be resent by a resume of the step (its marker may be
/// durable), so it can no longer settle definitely unsent. Either way the
/// charge stays spent: a resume sends without charging.
struct PendingFirst {
    table: Arc<LeaseTable>,
    artifact: ArtifactId,
    permit: Option<DispatchPermit>,
    placeholder: bool,
}

impl PendingFirst {
    fn deadline(&self) -> Instant {
        self.permit
            .as_ref()
            .map_or_else(Instant::now, |p| p.deadline)
    }

    fn hand_out(mut self) -> DispatchPermit {
        self.placeholder = false;
        self.permit.take().expect("held until handed out")
    }
}

impl Drop for PendingFirst {
    fn drop(&mut self) {
        if let Some(permit) = self.permit.take() {
            permit.discard();
        }
        if !self.placeholder {
            return;
        }
        self.table.with_j(|i, _| {
            let Some(r) = i.fence.rowless.get_mut(&self.artifact) else {
                return;
            };
            let RowlessState::Admitted {
                running,
                possibly_out,
                ..
            } = &mut r.state
            else {
                return;
            };
            *running = running.saturating_sub(1);
            if *running == 0 && !*possibly_out {
                i.fence.rowless.remove(&self.artifact);
            } else {
                *possibly_out = true;
                r.charge = None;
            }
        });
    }
}

impl LeaseTable {
    fn permit(
        self: &Arc<Self>,
        i: &mut Inner,
        fx: &mut Effects,
        (wallet, artifact): (WalletId, ArtifactId),
        lease: LeaseId,
        registered: bool,
    ) -> DispatchPermit {
        let deadline = Instant::now() + self.config.permit_ttl;
        let id = i
            .fence
            .attempt((wallet, artifact), Some(lease), Some(deadline), registered);
        if let Some(e) = i.leases.get_mut(&lease) {
            e.permits += 1;
            e.last_use = Instant::now();
            if !e.history.contains(&artifact) {
                e.history.push(artifact);
            }
            fx.changed(lease);
        }
        #[cfg(test)]
        i.note(super::stress_tests::LogEvent::Grant {
            attempt: id,
            lease: Some(lease),
            artifact,
            deadline: Some(deadline),
        });
        DispatchPermit {
            table: Arc::clone(self),
            id,
            deadline,
            done: false,
        }
    }

    fn guard(
        self: &Arc<Self>,
        i: &mut Inner,
        (wallet, artifact): (WalletId, ArtifactId),
        registered: bool,
    ) -> AttemptGuard {
        let id = i.fence.attempt((wallet, artifact), None, None, registered);
        #[cfg(test)]
        i.note(super::stress_tests::LogEvent::Grant {
            attempt: id,
            lease: None,
            artifact,
            deadline: None,
        });
        AttemptGuard {
            table: Arc::clone(self),
            id,
            done: false,
        }
    }

    fn notice(&self, fx: &mut Effects, code: NoticeCode, detail: String) {
        fx.emit(EngineEvent::Notice {
            network: Some(self.network.clone()),
            code,
            detail,
        });
    }

    /// Loads the journal's rows (§6.5): at load every origin is dead, so an
    /// `Unsent` row is refused on its first `admit`.
    pub(crate) fn load_journal(
        &self,
        backend: Option<Arc<dyn JournalBackend>>,
        rows: Vec<dw_appdb::dispatch::DispatchRow>,
        steps: Vec<dw_appdb::dispatch::StepRow>,
    ) {
        use dw_appdb::dispatch::DiskState;
        let ready = backend.is_some();
        self.journal.set(backend);
        self.with_j(|i, fx| {
            if !ready {
                i.fence.journal = JournalState::Unavailable;
                self.notice(
                    fx,
                    NoticeCode::DispatchJournalUnavailable,
                    "the dispatch journal could not be opened".into(),
                );
                return;
            }
            for row in rows {
                let reg = match row.state {
                    DiskState::Unsent => Reg::Unsent {
                        origin: row.origin_lease,
                        charge: None,
                    },
                    DiskState::Dispatching => Reg::Dispatching,
                    DiskState::PreFence => Reg::PreFence,
                };
                i.fence
                    .entries
                    .insert((WalletId(row.wallet), ArtifactId(row.txid)), reg);
            }
            for s in steps {
                i.fence
                    .steps
                    .entry((WalletId(s.wallet), s.step_id))
                    .or_default()
                    .insert(ArtifactId(s.artifact), Mark::Durable);
            }
            i.fence.journal = JournalState::Ready;
            fx.notify();
        });
    }

    /// Closes the journal (last step of close, §8.5).
    pub(crate) fn close_journal(&self) {
        self.journal.set(None);
        self.with_j(|i, _| i.fence.journal = JournalState::NotLoaded);
    }

    /// `register` (§5.5): charges `debit` to the scope's lease's Funding
    /// budget and writes `Unsent{origin}` durably before it is in memory.
    pub async fn register(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: ArtifactId,
        debit: u64,
        payload: Vec<u8>,
    ) -> Result<(), RegisterError> {
        let Some(Origin::Lease(lease)) = DispatchScope::current()
            .filter(|s| s.wallet == wallet)
            .map(|s| s.origin)
        else {
            return Err(LeaseError::Invalid.into());
        };
        let backend = self.journal.get();
        let charge = self.with_j(|i, fx| {
            if i.fence.journal != JournalState::Ready || backend.is_none() {
                return Err(RegisterError::Journal("unavailable".into()));
            }
            match i.fence.entries.get(&(wallet, txid)) {
                Some(_) if i.fence.origins.get(&(wallet, txid)) == Some(&lease) => {
                    return Ok(None);
                }
                Some(_) => return Err(RegisterError::OtherOrigin),
                None => {}
            }
            if i.barrier.set_for(&wallet) {
                return Err(LeaseError::Locked.into());
            }
            let e = i.leases.get_mut(&lease).ok_or(LeaseError::Invalid)?;
            if e.wallet != wallet || !e.state.live() {
                return Err(match e.state {
                    super::table::LeaseState::Revoked(c) => LeaseError::Revoked(c),
                    super::table::LeaseState::Ended => LeaseError::Ended,
                    _ => LeaseError::Invalid,
                }
                .into());
            }
            let charge = e.funding.charge(debit).map_err(|x| LeaseError::Exceeded {
                purpose: BudgetPurpose::Funding,
                needed: x.needed,
                remaining: x.remaining,
            })?;
            fx.changed(lease);
            i.fence.registering.entry((wallet, txid)).or_insert(false);
            Ok(Some(charge))
        })?;
        let Some(charge) = charge else {
            return Ok(());
        };
        let backend = backend.expect("checked above");
        let process = self.process;
        let table = Arc::clone(self);
        // The task resolves the entry itself, so a caller dropped during
        // the write neither loses the entry nor leaks the charge.
        self.rt
            .spawn_blocking(move || {
                let written = backend.register(&wallet.0, &txid.0, &lease, &process, &payload);
                table.with_j(|i, fx| {
                    let key = (wallet, txid);
                    let abandoned = i.fence.registering.remove(&key).unwrap_or(false);
                    let kept = matches!(written, Ok(dw_appdb::dispatch::Registered::Ok))
                        && !i.fence.entries.contains_key(&key);
                    if kept {
                        // Abandoned during the write: refused from the start.
                        let entry = if abandoned {
                            Reg::Revoked
                        } else {
                            Reg::Unsent {
                                origin: Some(lease),
                                charge: Some(charge),
                            }
                        };
                        i.fence.entries.insert(key, entry);
                        i.fence.origins.insert(key, lease);
                    }
                    if (!kept || abandoned)
                        && let Some(e) = i.leases.get_mut(&lease)
                    {
                        // Failed, abandoned, or a concurrent register won.
                        e.refund(BudgetPurpose::Funding, charge);
                        fx.changed(lease);
                    }
                    match written {
                        Ok(dw_appdb::dispatch::Registered::Ok)
                            if i.fence.origins.get(&key) == Some(&lease) =>
                        {
                            Ok(())
                        }
                        Ok(_) => Err(RegisterError::OtherOrigin),
                        Err(e) => Err(RegisterError::Journal(e)),
                    }
                })
            })
            .await
            .map_err(|e| RegisterError::Journal(e.to_string()))?
    }

    /// `abandon` (§5.5): one J step, no I/O; callable from `Drop`.
    pub fn abandon(&self, wallet: WalletId, txid: ArtifactId) -> Abandon {
        self.with_j(|i, fx| {
            let key = (wallet, txid);
            match i.fence.entries.get(&key).copied() {
                Some(Reg::Unsent { charge, .. }) => {
                    revoke_unsent(self, i, fx, key, charge);
                    Abandon::Revoked { cleanup: true }
                }
                None => match i.fence.registering.get_mut(&key) {
                    // Its write lands as `Revoked` (`register`).
                    Some(abandoned) => Abandon::Revoked {
                        cleanup: !std::mem::replace(abandoned, true),
                    },
                    None => Abandon::Revoked { cleanup: false },
                },
                Some(Reg::Revoked) => Abandon::Revoked { cleanup: false },
                Some(_) => Abandon::Committed,
            }
        })
    }

    /// The library's proof wait for this asset lock starts now (§4.4):
    /// the origin lease keeps its key exactly as long.
    pub fn proof_wait_started(&self, wallet: WalletId, txid: ArtifactId, timeout: Duration) {
        self.with_j(|i, fx| {
            let Some(lease) = i.fence.origins.get(&(wallet, txid)).copied() else {
                return;
            };
            if let Some(e) = i.leases.get_mut(&lease)
                && e.state.signs()
                && e.key.is_some()
            {
                e.key_until = Some(Instant::now() + timeout);
                e.state = super::table::LeaseState::AwaitingProof;
                fx.changed(lease);
            }
        });
    }

    /// A surfaced ChainLock fallback (L17): the origin lease parks at once.
    pub fn chainlock_fallback(&self, wallet: WalletId, txid: ArtifactId) {
        self.with_j(|i, fx| {
            if let Some(lease) = i.fence.origins.get(&(wallet, txid)).copied()
                && let Some(e) = i.leases.get_mut(&lease)
                && e.park(fx)
            {
                fx.changed(lease);
            }
        });
    }

    /// Binds a `TxDraft`'s Spend charge to its txid (§4.2). A prepare that
    /// signs the same bytes as a draft that settled definitely unsent is a
    /// new draft: its inputs are reserved again and the earlier holder was
    /// released, so the tombstone gives way to it.
    pub(crate) fn bind_spend(
        &self,
        lease: LeaseId,
        wallet: WalletId,
        txid: ArtifactId,
        charge: Charge,
    ) {
        self.with_j(|i, _| {
            if i.fence
                .rowless
                .get(&txid)
                .is_some_and(|r| r.wallet == wallet && r.state == RowlessState::Revoked)
            {
                i.fence.rowless.remove(&txid);
            }
            i.fence.spend.insert(
                txid,
                SpendCharge {
                    lease,
                    wallet,
                    charge,
                },
            );
        });
    }

    /// A prepared `TxDraft` released before its charge was admitted:
    /// refunds it. An admission moves the charge to the row-less entry,
    /// whose settlement refunds it.
    pub(crate) fn unbind_spend(&self, txid: ArtifactId) {
        self.with_j(|i, fx| {
            if let Some(s) = i.fence.spend.remove(&txid)
                && let Some(e) = i.leases.get_mut(&s.lease)
            {
                e.refund(BudgetPurpose::Spend, s.charge);
                fx.changed(s.lease);
            }
        });
    }

    /// `admit` (§5.4): the one decision. Never waits on another caller or
    /// on the journal's load; the only await is its own spawned write.
    pub async fn admit(self: &Arc<Self>, req: AdmitRequest) -> Verdict {
        let backend = self.journal.get();
        #[cfg(test)]
        if self.with_j(|i, _| i.mutation) == Some(super::stress_tests::Mutation::CheckThenAct)
            && let Some(v) = self.admit_check_then_act(&req).await
        {
            return v;
        }
        if req.scope.is_none() {
            // Outside J: a subscriber may do I/O (H1).
            tracing::error!(artifact = %req.artifact, "a hand-off without a dispatch scope");
        }
        let step = self.with_j(|i, fx| self.decide(i, fx, &req, backend));
        let (write, then) = match step {
            Step::Done(v) => return v,
            Step::Write { write, then } => (write, then),
        };
        #[cfg(test)]
        if self.with_j(|i, _| i.mutation) == Some(super::stress_tests::Mutation::TransportFirst)
            && let After::First { permit, .. } = then
        {
            // The mutation: hand off before the record is durable.
            drop(write);
            return Verdict::First(permit);
        }
        match then {
            After::First {
                permit,
                placeholder,
            } => {
                let pending = PendingFirst {
                    table: Arc::clone(self),
                    artifact: req.artifact,
                    permit: Some(permit),
                    placeholder,
                };
                let ok = write.await.unwrap_or(false);
                if ok && Instant::now() < pending.deadline() {
                    Verdict::First(pending.hand_out())
                } else {
                    Verdict::Deferred
                }
            }
            After::Resend {
                registered,
                rowless,
            } => {
                if !write.await.unwrap_or(false) {
                    return Verdict::Deferred;
                }
                self.with_j(|i, _| {
                    if let Some(r) = rowless {
                        admit_rowless(i, req.artifact, r);
                    }
                    Verdict::Resend(self.guard(i, (req.wallet, req.artifact), registered))
                })
            }
        }
    }

    /// Spawns the durable write for `key`'s `Committing` entry, inside the
    /// J step that set it. The task resolves the entry itself.
    fn spawn_write(
        self: &Arc<Self>,
        backend: Arc<dyn JournalBackend>,
        wallet: WalletId,
        artifact: ArtifactId,
        step: Option<String>,
        owner: u64,
    ) -> tokio::task::JoinHandle<bool> {
        let table = Arc::clone(self);
        self.rt.spawn_blocking(move || {
            let ok = match &step {
                None => matches!(backend.mark_dispatching(&wallet.0, &artifact.0), Ok(true)),
                Some(s) => backend.insert_step(&wallet.0, s, &artifact.0).is_ok(),
            };
            table.with_j(|i, fx| {
                match &step {
                    None => {
                        if let Some(e) = i.fence.entries.get_mut(&(wallet, artifact))
                            && *e == (Reg::Committing { owner })
                        {
                            *e = if ok { Reg::Dispatching } else { Reg::Ambiguous };
                        }
                    }
                    Some(s) => {
                        if let Some(m) = i
                            .fence
                            .steps
                            .get_mut(&(wallet, s.clone()))
                            .and_then(|m| m.get_mut(&artifact))
                            && *m == (Mark::Committing { owner })
                        {
                            *m = if ok { Mark::Durable } else { Mark::Ambiguous };
                        }
                    }
                }
                #[cfg(test)]
                if ok {
                    i.note(super::stress_tests::LogEvent::Durable { artifact });
                }
                if let Some(lease) = i.fence.origins.get(&(wallet, artifact)) {
                    fx.changed(*lease);
                }
            });
            ok
        })
    }

    fn decide(
        self: &Arc<Self>,
        i: &mut Inner,
        fx: &mut Effects,
        req: &AdmitRequest,
        backend: Option<Arc<dyn JournalBackend>>,
    ) -> Step {
        let wallet = req.wallet;
        let id = req.artifact;
        let key = (wallet, id);
        if req.scope.is_none() {
            self.notice(fx, NoticeCode::UnscopedDispatch, format!("artifact {id}"));
        }
        let origin = req
            .scope
            .as_ref()
            .filter(|s| s.wallet == wallet)
            .map(|s| s.origin);
        let step = req.scope.as_ref().and_then(|s| s.step.clone());
        let journal = i.fence.journal;
        let backend = backend.filter(|_| journal == JournalState::Ready);

        // Registered artifacts: the journal decides.
        if let Some(reg) = i.fence.entries.get(&key).copied() {
            let Some(backend) = backend else {
                return Step::Done(Verdict::Deferred);
            };
            return match reg {
                Reg::Revoked => Step::Done(Verdict::Refused {
                    cleanup: false,
                    step_possibly_dispatched: false,
                }),
                Reg::Dispatching | Reg::PreFence => {
                    Step::Done(Verdict::Resend(self.guard(i, key, true)))
                }
                Reg::Committing { .. } => Step::Done(Verdict::Deferred),
                Reg::Ambiguous => {
                    i.fence.next += 1;
                    let owner = i.fence.next;
                    i.fence.entries.insert(key, Reg::Committing { owner });
                    Step::Write {
                        write: self.spawn_write(backend, wallet, id, None, owner),
                        then: After::Resend {
                            registered: true,
                            rowless: None,
                        },
                    }
                }
                Reg::Unsent { origin: o, charge } => {
                    let live = o.filter(|l| FenceState::live(&i.leases, l, &wallet));
                    match live {
                        Some(lease) => {
                            i.fence.next += 1;
                            let owner = i.fence.next;
                            i.fence.entries.insert(key, Reg::Committing { owner });
                            let permit = self.permit(i, fx, key, lease, true);
                            Step::Write {
                                write: self.spawn_write(backend, wallet, id, None, owner),
                                then: After::First {
                                    permit,
                                    placeholder: false,
                                },
                            }
                        }
                        None => {
                            revoke_unsent(self, i, fx, key, charge);
                            #[cfg(test)]
                            i.note(super::stress_tests::LogEvent::Refused {
                                artifact: id,
                                cleanup: true,
                            });
                            Step::Done(Verdict::Refused {
                                cleanup: true,
                                step_possibly_dispatched: false,
                            })
                        }
                    }
                }
            };
        }
        if req.tracked_row {
            // Unknown provenance: never sent, never cleaned up (Q16).
            if journal == JournalState::Ready {
                self.notice(
                    fx,
                    NoticeCode::DispatchRecordMissing,
                    format!("asset lock {id}"),
                );
            }
            return Step::Done(Verdict::Deferred);
        }

        // Row-less artifacts.
        if let Some(r) = i.fence.rowless.get_mut(&id).filter(|r| r.wallet == wallet) {
            match &mut r.state {
                RowlessState::Revoked => {
                    return Step::Done(Verdict::Refused {
                        cleanup: false,
                        step_possibly_dispatched: false,
                    });
                }
                RowlessState::Admitted { .. }
                    if step.as_ref().is_some_and(|s| {
                        matches!(
                            i.fence
                                .steps
                                .get(&(wallet, s.clone()))
                                .and_then(|m| m.get(&id)),
                            Some(Mark::Committing { .. })
                        )
                    }) =>
                {
                    // This step's marker write is still running.
                    return Step::Done(Verdict::Deferred);
                }
                RowlessState::Admitted { running, .. } => {
                    *running += 1;
                    return Step::Done(Verdict::Resend(self.guard(i, key, false)));
                }
                RowlessState::NotSent => {}
            }
        }
        if req.kind == (ArtifactKind::CoreTx { asset_lock: true }) {
            return Step::Done(Verdict::Refused {
                cleanup: false,
                step_possibly_dispatched: false,
            });
        }
        // A resumable step's marker from this or an earlier process.
        if let Some(s) = &step {
            let mark = i
                .fence
                .steps
                .get(&(wallet, s.clone()))
                .and_then(|m| m.get(&id))
                .copied();
            match mark {
                Some(Mark::Durable) => {
                    i.fence
                        .rowless
                        .insert(id, Rowless::admitted(wallet, req.kind, None, true));
                    return Step::Done(Verdict::Resend(self.guard(i, key, false)));
                }
                Some(Mark::Committing { .. }) => return Step::Done(Verdict::Deferred),
                Some(Mark::Ambiguous) => {
                    let Some(backend) = backend else {
                        return Step::Done(Verdict::Deferred);
                    };
                    i.fence.next += 1;
                    let owner = i.fence.next;
                    i.fence
                        .steps
                        .entry((wallet, s.clone()))
                        .or_default()
                        .insert(id, Mark::Committing { owner });
                    return Step::Write {
                        write: self.spawn_write(backend, wallet, id, Some(s.clone()), owner),
                        then: After::Resend {
                            registered: false,
                            rowless: Some(Rowless::admitted(wallet, req.kind, None, true)),
                        },
                    };
                }
                None => {}
            }
        }
        match origin {
            None => {
                if req.scope.is_some() {
                    // A scope for another wallet: an engine bug, fail closed.
                    self.notice(fx, NoticeCode::UnscopedDispatch, format!("artifact {id}"));
                }
                Step::Done(Verdict::Deferred)
            }
            Some(Origin::Unleased(_)) => {
                i.fence
                    .rowless
                    .insert(id, Rowless::admitted(wallet, req.kind, None, false));
                Step::Done(Verdict::FirstUnleased(self.guard(i, key, false)))
            }
            Some(Origin::Lease(lease)) => {
                let authority = backend.is_some()
                    && FenceState::live(&i.leases, &lease, &wallet)
                    && match req.kind {
                        ArtifactKind::CoreTx { .. } => i
                            .fence
                            .spend
                            .get(&id)
                            .is_some_and(|s| s.lease == lease && s.wallet == wallet),
                        ArtifactKind::Transition { .. } => true,
                    };
                // Credits are charged with the permit, in the same step.
                let charge = match (authority, req.kind) {
                    (true, ArtifactKind::Transition { credits, .. }) => i
                        .leases
                        .get_mut(&lease)
                        .and_then(|e| e.credits.charge(credits).ok())
                        .map(|c| Some((lease, BudgetPurpose::Credits, c))),
                    // The Spend charge moves to the row-less entry, whose
                    // settlement refunds it.
                    (true, ArtifactKind::CoreTx { .. }) => Some(
                        i.fence
                            .spend
                            .remove(&id)
                            .map(|s| (lease, BudgetPurpose::Spend, s.charge)),
                    ),
                    (false, _) => None,
                };
                let Some(charge) = charge else {
                    return Step::Done(self.refuse_rowless(i, fx, req, step.as_deref()));
                };
                match (&step, backend) {
                    (Some(s), Some(backend)) => {
                        i.fence.next += 1;
                        let owner = i.fence.next;
                        i.fence
                            .steps
                            .entry((wallet, s.clone()))
                            .or_default()
                            .insert(id, Mark::Committing { owner });
                        // The entry exists from here on, so a copy scoped
                        // another way joins it instead of charging again.
                        i.fence
                            .rowless
                            .insert(id, Rowless::admitted(wallet, req.kind, charge, false));
                        let permit = self.permit(i, fx, key, lease, false);
                        Step::Write {
                            write: self.spawn_write(backend, wallet, id, Some(s.clone()), owner),
                            then: After::First {
                                permit,
                                placeholder: true,
                            },
                        }
                    }
                    _ => {
                        i.fence
                            .rowless
                            .insert(id, Rowless::admitted(wallet, req.kind, charge, false));
                        Step::Done(Verdict::First(self.permit(i, fx, key, lease, false)))
                    }
                }
            }
        }
    }

    /// A leased row-less First whose lease is not live, lacks the
    /// authority, or whose journal is unavailable: provably never sent.
    fn refuse_rowless(
        &self,
        i: &mut Inner,
        fx: &mut Effects,
        req: &AdmitRequest,
        step: Option<&str>,
    ) -> Verdict {
        let id = req.artifact;
        let first = !i.fence.rowless.contains_key(&id);
        i.fence.rowless.insert(
            id,
            Rowless {
                wallet: req.wallet,
                state: req.kind.tombstone(),
                kind: req.kind,
                charge: None,
            },
        );
        if let Some(s) = i.fence.spend.remove(&id)
            && let Some(e) = i.leases.get_mut(&s.lease)
        {
            e.refund(BudgetPurpose::Spend, s.charge);
            fx.changed(s.lease);
        }
        let cleanup = first && matches!(req.kind, ArtifactKind::CoreTx { .. });
        #[cfg(test)]
        i.note(super::stress_tests::LogEvent::Refused {
            artifact: id,
            cleanup,
        });
        Verdict::Refused {
            cleanup,
            step_possibly_dispatched: step.is_some_and(|s| i.fence.step_marked(&req.wallet, s)),
        }
    }

    /// `finish` of an attempt (§5.5): the artifact's settlement.
    pub(crate) fn finish(&self, attempt: u64, outcome: Outcome) -> Settlement {
        self.with_j(|i, fx| {
            fx.notify();
            let Some(a) = i.fence.attempts.remove(&attempt) else {
                return Settlement::MaybeOut;
            };
            #[cfg(test)]
            i.note(super::stress_tests::LogEvent::Finish { attempt });
            if let Some(lease) = a.lease
                && let Some(e) = i.leases.get_mut(&lease)
            {
                e.permits = e.permits.saturating_sub(1);
                fx.changed(lease);
            }
            let id = a.artifact;
            if outcome == Outcome::Sent && i.fence.sent.insert(id) {
                resolved(self, fx, &a.wallet, &id, DispatchResolution::Sent);
            }
            if a.registered {
                // L13: after `Dispatching` a rejection keeps the row.
                return if i.fence.sent.contains(&id) {
                    Settlement::Sent
                } else {
                    Settlement::MaybeOut
                };
            }
            let Some(r) = i.fence.rowless.get_mut(&id) else {
                return Settlement::MaybeOut;
            };
            let RowlessState::Admitted {
                running,
                possibly_out,
                slot_taken,
            } = &mut r.state
            else {
                return Settlement::MaybeOut;
            };
            *running = running.saturating_sub(1);
            if outcome != Outcome::NotSent {
                *possibly_out = true;
            }
            if i.fence.sent.contains(&id) {
                return Settlement::Sent;
            }
            if *running > 0 {
                return Settlement::MaybeOut;
            }
            if !*possibly_out && outcome == Outcome::NotSent {
                settle_unsent(self, i, fx, &id);
                return Settlement::DefinitelyUnsent;
            }
            if *slot_taken {
                // H16: another transition used the slot; this one cannot
                // execute. Tombstoned, but the bytes may be out.
                settle_unsent(self, i, fx, &id);
            }
            Settlement::MaybeOut
        })
    }

    fn discard(&self, attempt: u64) {
        self.with_j(|i, fx| {
            fx.notify();
            if let Some(a) = i.fence.attempts.remove(&attempt)
                && let Some(lease) = a.lease
                && let Some(e) = i.leases.get_mut(&lease)
            {
                e.permits = e.permits.saturating_sub(1);
                fx.changed(lease);
            }
        });
    }

    /// H16: the engine holds `executed`'s proved execution result, a
    /// transition it signed in `slot`. It is `Sent`; every other transition
    /// this engine signed for the same slot can no longer execute and
    /// settles `NotSent` once none of its attempts runs.
    pub fn note_executed(&self, wallet: WalletId, executed: ArtifactId, slot: NonceSlot) {
        self.with_j(|i, fx| {
            let signed_here = i.fence.rowless.get(&executed).is_some_and(|r| {
                r.wallet == wallet
                    && matches!(r.kind, ArtifactKind::Transition { slot: Some(s), .. } if s == slot)
            });
            if !signed_here {
                return;
            }
            if i.fence.sent.insert(executed) {
                resolved(self, fx, &wallet, &executed, DispatchResolution::Sent);
            }
            let others: Vec<ArtifactId> = i
                .fence
                .rowless
                .iter()
                .filter(|(id, r)| {
                    **id != executed
                        && r.wallet == wallet
                        && matches!(r.kind, ArtifactKind::Transition { slot: Some(s), .. } if s == slot)
                })
                .map(|(id, _)| *id)
                .collect();
            for id in others {
                if i.fence.sent.contains(&id) {
                    continue;
                }
                let Some(r) = i.fence.rowless.get_mut(&id) else {
                    continue;
                };
                if let RowlessState::Admitted {
                    running,
                    slot_taken,
                    ..
                } = &mut r.state
                {
                    *slot_taken = true;
                    if *running == 0 {
                        settle_unsent(self, i, fx, &id);
                    }
                }
            }
        });
    }

    /// The wallet saw the transaction (a Resend accepted, or it confirmed):
    /// `Sent` (§4.6).
    pub fn note_seen(&self, wallet: WalletId, artifact: ArtifactId) {
        self.with_j(|i, fx| {
            let known = i.fence.rowless.contains_key(&artifact)
                || i.fence.entries.contains_key(&(wallet, artifact));
            if known && i.fence.sent.insert(artifact) {
                resolved(self, fx, &wallet, &artifact, DispatchResolution::Sent);
            }
        });
    }

    /// `dispatch_status` (§16.6), first matching row wins. The tracked-row
    /// rows of the table are Mode A's (P4) and Mode B's (P2b).
    pub fn dispatch_status(&self, wallet: WalletId, artifact: ArtifactId) -> Option<DispatchState> {
        self.with_j(|i, _| {
            let f = &i.fence;
            if f.sent.contains(&artifact) {
                return Some(DispatchState::Sent);
            }
            match f.entries.get(&(wallet, artifact)) {
                Some(Reg::Dispatching | Reg::PreFence | Reg::Ambiguous) => {
                    return Some(DispatchState::WillBeSent);
                }
                Some(Reg::Committing { .. }) => return Some(DispatchState::MaybeSent),
                Some(Reg::Unsent { origin, .. }) => {
                    let live = origin.is_some_and(|l| FenceState::live(&i.leases, &l, &wallet));
                    return Some(if live {
                        DispatchState::MaybeSent
                    } else {
                        DispatchState::NotSent
                    });
                }
                Some(Reg::Revoked) => return Some(DispatchState::NotSent),
                None => {}
            }
            if let Some(r) = f.rowless.get(&artifact).filter(|r| r.wallet == wallet) {
                return Some(match r.state {
                    RowlessState::Admitted { .. } => DispatchState::MaybeSent,
                    RowlessState::Revoked | RowlessState::NotSent => DispatchState::NotSent,
                });
            }
            let marked = f
                .steps
                .iter()
                .any(|((w, _), m)| *w == wallet && m.contains_key(&artifact));
            marked.then_some(DispatchState::MaybeSent)
        })
    }

    /// Whether `wallet` has an entry that may be on the wire; a wiping
    /// removal keeps the journal's rows then (§6.5).
    pub(crate) fn wallet_possibly_sent(&self, wallet: &WalletId) -> bool {
        self.with_j(|i, _| {
            i.fence.entries.iter().any(|((w, _), r)| {
                w == wallet
                    && matches!(
                        r,
                        Reg::Committing { .. } | Reg::Dispatching | Reg::Ambiguous | Reg::PreFence
                    )
            })
        })
    }

    /// Forgets a wiped wallet's entries after its rows were erased.
    pub(crate) fn forget_wallet_entries(&self, wallet: &WalletId) {
        self.with_j(|i, _| {
            i.fence.entries.retain(|(w, _), _| w != wallet);
            i.fence.origins.retain(|(w, _), _| w != wallet);
            i.fence.steps.retain(|(w, _), _| w != wallet);
            for ((w, _), abandoned) in &mut i.fence.registering {
                *abandoned |= w == wallet;
            }
            i.fence.rowless.retain(|_, r| r.wallet != *wallet);
            i.fence.spend.retain(|_, s| s.wallet != *wallet);
        });
    }

    /// Logs a transport call in J order (the stress checker's record).
    #[cfg(test)]
    pub(crate) fn log_transport(&self, artifact: ArtifactId, attempt: u64) {
        self.with_j(|i, _| {
            i.note(super::stress_tests::LogEvent::Transport {
                artifact,
                attempt,
                at: Instant::now(),
            });
        });
    }

    #[cfg(test)]
    async fn admit_check_then_act(self: &Arc<Self>, req: &AdmitRequest) -> Option<Verdict> {
        // The mutation the stress must catch: the lease is checked in one
        // J step and the permit granted in another.
        let Some(Origin::Lease(lease)) = req.scope.as_ref().map(|s| s.origin) else {
            return None;
        };
        if req.tracked_row || req.scope.as_ref().is_some_and(|s| s.step.is_some()) {
            return None;
        }
        let live = self.with_j(|i, _| {
            !i.fence.rowless.contains_key(&req.artifact)
                && FenceState::live(&i.leases, &lease, &req.wallet)
        });
        if !live {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
        Some(self.with_j(|i, fx| {
            i.fence.rowless.insert(
                req.artifact,
                Rowless::admitted(req.wallet, req.kind, None, false),
            );
            Verdict::First(self.permit(i, fx, (req.wallet, req.artifact), lease, false))
        }))
    }
}
