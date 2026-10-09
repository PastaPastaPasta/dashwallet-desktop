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
#[cfg(test)]
use super::stress_tests::{GrantKind, LogEvent, Mutation};
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
    /// The artifact settled definitely unsent: its step markers go.
    fn resolve_unsent(&self, wallet: &[u8; 32], artifact: &[u8; 32]) -> Result<(), String>;
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

    fn resolve_unsent(&self, wallet: &[u8; 32], artifact: &[u8; 32]) -> Result<(), String> {
        dw_appdb::dispatch::DispatchJournal::resolve_unsent(self, wallet, artifact)
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
    /// Settled definitely unsent; the journal write superseding its step
    /// markers runs (review P2a r1 F2), or `failed` and waits for the next
    /// `admit` of it to retry. Every copy defers meanwhile: the bytes are
    /// known unsent, so nothing may resend them on their old evidence.
    Resolving { slot_taken: bool, failed: bool },
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
    pub(super) registering: HashMap<(WalletId, ArtifactId), bool>,
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

    /// Ends this attempt. A definite `NotSent` that settles a marked
    /// artifact deletes its markers in the journal before it returns
    /// (`block_in_place` on a multi-thread worker, inline elsewhere), so a
    /// current-thread runtime blocks for that short write.
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

    /// Ends this attempt. A definite `NotSent` that settles a marked
    /// artifact deletes its markers in the journal before it returns
    /// (`block_in_place` on a multi-thread worker, inline elsewhere), so a
    /// current-thread runtime blocks for that short write.
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

    /// Whether `id` has a step marker of `wallet`, in any step.
    #[cfg(test)]
    pub(crate) fn resolving(&self, id: &ArtifactId) -> bool {
        self.rowless
            .get(id)
            .is_some_and(|r| matches!(r.state, RowlessState::Resolving { .. }))
    }

    fn marked(&self, wallet: &WalletId, id: &ArtifactId) -> bool {
        self.steps
            .iter()
            .any(|((w, _), m)| w == wallet && m.contains_key(id))
    }

    /// H10 for one copy of `id` (review P2a r1 F1): every marker of the
    /// artifact, and the marker of the copy's own `step`, must be durable.
    fn obligation(&self, wallet: &WalletId, id: &ArtifactId, step: Option<&str>) -> Obligation {
        let mut write = Vec::new();
        for ((w, s), m) in &self.steps {
            match m.get(id) {
                _ if w != wallet => {}
                Some(Mark::Committing { .. }) => return Obligation::Pending,
                Some(Mark::Ambiguous) => write.push(s.clone()),
                Some(Mark::Durable) | None => {}
            }
        }
        if let Some(s) = step
            && !self
                .steps
                .get(&(*wallet, s.to_owned()))
                .is_some_and(|m| m.contains_key(id))
        {
            write.push(s.to_owned());
        }
        if write.is_empty() {
            Obligation::Met
        } else {
            Obligation::Write(write)
        }
    }

    #[cfg(test)]
    pub(crate) fn mark(&self, wallet: &WalletId, step: &str, id: &ArtifactId) -> Option<Mark> {
        self.steps
            .get(&(*wallet, step.to_owned()))
            .and_then(|m| m.get(id))
            .copied()
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
    let charge = r.charge.take();
    #[cfg(test)]
    i.note(LogEvent::Resolved {
        artifact: *id,
        refund: charge.map_or(0, |(_, _, c)| c.amount),
    });
    if let Some((lease, purpose, charge)) = charge
        && let Some(e) = i.leases.get_mut(&lease)
    {
        e.refund(purpose, charge);
        fx.changed(lease);
    }
    i.fence.spend.remove(id);
    resolved(t, fx, &wallet, id, DispatchResolution::NotSent);
}

/// How a deadline-bound hand-off ended ([`LeaseTable::hand_off`]).
#[derive(Debug)]
pub(crate) enum HandOff<T> {
    /// The library returned in time.
    Done(T),
    /// The deadline came before the library was entered: never sent.
    NotEntered,
    /// The deadline came during the library call: possibly sent.
    Cut,
}

/// How a row-less settlement proceeds.
enum Settling {
    /// Settled in this J step: the artifact has no step marker.
    Now,
    /// `Resolving`: `LeaseTable::resolve_unsent` settles it once the
    /// journal dropped its markers.
    Durably(WalletId),
    /// A marker write of it runs: the write's own J step settles it
    /// (`spawn_write`) unless a copy joined meanwhile.
    Blocked,
}

/// Starts settling the row-less `id` definitely unsent (§5.5). A marked
/// artifact's resolution must be durable before it is seen (review P2a r1
/// F2).
fn start_settle(t: &LeaseTable, i: &mut Inner, fx: &mut Effects, id: &ArtifactId) -> Settling {
    let Some(wallet) = i.fence.rowless.get(id).map(|r| r.wallet) else {
        return Settling::Now;
    };
    #[cfg(test)]
    let in_memory = i.mutation == Some(Mutation::MarkerOutlives);
    #[cfg(not(test))]
    let in_memory = false;
    // The mutation settles in memory, its markers kept in the journal.
    if in_memory || !i.fence.marked(&wallet, id) {
        settle_unsent(t, i, fx, id);
        return Settling::Now;
    }
    if matches!(i.fence.obligation(&wallet, id, None), Obligation::Pending) {
        return Settling::Blocked;
    }
    let Some(r) = i.fence.rowless.get_mut(id) else {
        return Settling::Now;
    };
    let slot_taken = matches!(
        r.state,
        RowlessState::Admitted {
            slot_taken: true,
            ..
        }
    );
    r.state = RowlessState::Resolving {
        slot_taken,
        failed: false,
    };
    Settling::Durably(wallet)
}

/// Whether an `Admitted` row-less entry with no attempt running settles:
/// every attempt ended definitely unsent, or another transition took its
/// nonce slot (H16). The caller has checked it was never seen sent.
fn settles_idle(possibly_out: bool, slot_taken: bool) -> bool {
    !possibly_out || slot_taken
}

/// A `Resolving` entry seen sent meanwhile is possibly out again, its
/// charge and markers kept: a sent artifact is never refunded.
fn keep_sent(i: &mut Inner, id: &ArtifactId) -> bool {
    if !i.fence.sent.contains(id) {
        return false;
    }
    if let Some(r) = i.fence.rowless.get_mut(id)
        && let RowlessState::Resolving { slot_taken, .. } = r.state
    {
        r.state = RowlessState::Admitted {
            running: 0,
            possibly_out: true,
            slot_taken,
        };
    }
    true
}

/// Runs a short journal call from synchronous code on an async thread:
/// `block_in_place` on a multi-thread worker, inline otherwise (a
/// `spawn_blocking` thread, or a current-thread runtime, which it blocks).
fn blocking<R>(f: impl FnOnce() -> R) -> R {
    match tokio::runtime::Handle::try_current() {
        Ok(h) if h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
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
    /// Durable writes were spawned; `then` decides once all returned.
    Write {
        writes: Vec<tokio::task::JoinHandle<bool>>,
        then: After,
    },
}

/// What an artifact's step markers still need before a copy transports.
enum Obligation {
    Met,
    /// A marker write runs.
    Pending,
    /// These steps' markers are missing or ambiguous: write them first.
    Write(Vec<String>),
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

/// Whether every write succeeded.
async fn all_written(writes: Vec<tokio::task::JoinHandle<bool>>) -> bool {
    let mut ok = true;
    for w in writes {
        ok &= w.await.unwrap_or(false);
    }
    ok
}

/// Inserts `r` for `id`, or joins an entry already admitted. A join adds
/// an attempt and nothing else: the entry's attempts say whether it may be
/// out, and an entry made from a marker starts possibly out.
fn admit_rowless(i: &mut Inner, id: ArtifactId, r: Rowless) {
    match i.fence.rowless.get_mut(&id).map(|e| &mut e.state) {
        Some(RowlessState::Admitted { running, .. }) => {
            *running += 1;
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
        {
            let charge = i
                .fence
                .rowless
                .get(&artifact)
                .filter(|_| !registered)
                .and_then(|r| r.charge)
                .map_or(0, |(_, _, c)| c.amount);
            i.note(LogEvent::Grant {
                attempt: id,
                lease: Some(lease),
                artifact,
                deadline: Some(deadline),
                kind: GrantKind::First,
                charge,
            });
        }
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
        i.note(LogEvent::Grant {
            attempt: id,
            lease: None,
            artifact,
            deadline: None,
            kind: GrantKind::Resend,
            charge: 0,
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
    /// Single-flight per artifact: a call that finds the artifact's write
    /// running waits for it and decides again, under the barrier, so one
    /// writer at most runs and a removal's erase waits for every write
    /// (DEC-134).
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
        let key = (wallet, txid);
        let charge = loop {
            let decided = self.register_step(&backend, key, lease, debit)?;
            match decided {
                Some(charge) => break charge,
                // Another call's write runs: wait for it to resolve.
                None => {
                    self.wait_until(|i| (!i.fence.registering.contains_key(&key)).then_some(()))
                        .await;
                }
            }
        };
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
                // Its J step below always runs, even after a panicking
                // write: a removal's erase waits for it (`erase_wallet_rows`).
                let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    backend.register(&wallet.0, &txid.0, &lease, &process, &payload)
                }))
                .unwrap_or_else(|_| Err("the journal write panicked".into()));
                table.with_j(|i, fx| {
                    let key = (wallet, txid);
                    let abandoned = i.fence.registering.remove(&key).unwrap_or(false);
                    #[cfg(test)]
                    i.note(LogEvent::Registered {
                        wallet,
                        artifact: txid,
                    });
                    fx.notify();
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

    /// `register`'s J step: `None` while another call's write for the
    /// artifact runs; `Some(None)` when it is registered already under this
    /// lease; `Some(Some(charge))` when this call charged and must write.
    fn register_step(
        &self,
        backend: &Option<Arc<dyn JournalBackend>>,
        key: (WalletId, ArtifactId),
        lease: LeaseId,
        debit: u64,
    ) -> Result<Option<Option<Charge>>, RegisterError> {
        let wallet = key.0;
        self.with_j(|i, fx| {
            if i.fence.journal != JournalState::Ready || backend.is_none() {
                return Err(RegisterError::Journal("unavailable".into()));
            }
            match i.fence.entries.get(&key) {
                Some(_) if i.fence.origins.get(&key) == Some(&lease) => {
                    return Ok(Some(None));
                }
                Some(_) => return Err(RegisterError::OtherOrigin),
                None => {}
            }
            #[cfg(test)]
            let single_flight = i.mutation != Some(Mutation::DuplicateRegister);
            #[cfg(not(test))]
            let single_flight = true;
            if single_flight && i.fence.registering.contains_key(&key) {
                return Ok(None);
            }
            if i.barrier.set_for(&wallet) {
                return Err(LeaseError::Locked.into());
            }
            let e = i.leases.get_mut(&lease).ok_or(LeaseError::Invalid)?;
            if e.wallet == wallet && e.state.live() && !e.funding.granted() {
                // A zero debit is no authority: Funding must be granted.
                return Err(LeaseError::NeedsGrant(BudgetPurpose::Funding).into());
            }
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
            i.fence.registering.insert(key, false);
            #[cfg(test)]
            i.note(LogEvent::Registering {
                wallet,
                artifact: key.1,
            });
            Ok(Some(Some(charge)))
        })
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
    /// released, so the tombstone gives way to it. A `Resolving` entry
    /// (only step-scoped Core transactions, none in P2a) does not: the
    /// draft's admit retries the resolution and is refused.
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
        if self.with_j(|i, _| i.mutation) == Some(Mutation::CheckThenAct)
            && let Some(v) = self.admit_check_then_act(&req).await
        {
            return v;
        }
        if req.scope.is_none() {
            // Outside J: a subscriber may do I/O (H1).
            tracing::error!(artifact = %req.artifact, "a hand-off without a dispatch scope");
        }
        // A definite resolution the journal refused is retried before any
        // decision about the artifact (review P2a r1 F2).
        let retry = self.with_j(|i, _| match i.fence.rowless.get_mut(&req.artifact) {
            Some(r) if r.wallet == req.wallet => match r.state {
                RowlessState::Resolving {
                    slot_taken,
                    failed: true,
                } => {
                    r.state = RowlessState::Resolving {
                        slot_taken,
                        failed: false,
                    };
                    true
                }
                _ => false,
            },
            _ => false,
        });
        if retry {
            let table = Arc::clone(self);
            let (wallet, id) = (req.wallet, req.artifact);
            let _ = self
                .rt
                .spawn_blocking(move || table.resolve_unsent(wallet, id))
                .await;
        }
        let step = self.with_j(|i, fx| self.decide(i, fx, &req, backend));
        let (writes, then) = match step {
            Step::Done(v) => return v,
            Step::Write { writes, then } => (writes, then),
        };
        #[cfg(test)]
        if self.with_j(|i, _| i.mutation) == Some(Mutation::TransportFirst)
            && let After::First { permit, .. } = then
        {
            // The mutation: hand off before the record is durable.
            drop(writes);
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
                let ok = all_written(writes).await;
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
                if !all_written(writes).await {
                    return Verdict::Deferred;
                }
                self.with_j(|i, _| {
                    if let Some(r) = rowless {
                        // Settled meanwhile: the definite answer wins; the
                        // caller's next admit reads it.
                        if i.fence
                            .rowless
                            .get(&req.artifact)
                            .is_some_and(|e| !matches!(e.state, RowlessState::Admitted { .. }))
                        {
                            return Verdict::Deferred;
                        }
                        // Another copy may have begun a marker meanwhile:
                        // this one owes it too (F1).
                        let step = req.scope.as_ref().and_then(|s| s.step.as_deref());
                        #[cfg(test)]
                        let recheck = i.mutation != Some(Mutation::CopyBeforeMarker);
                        #[cfg(not(test))]
                        let recheck = true;
                        if recheck
                            && !matches!(
                                i.fence.obligation(&req.wallet, &req.artifact, step),
                                Obligation::Met
                            )
                        {
                            return Verdict::Deferred;
                        }
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
            let settling = table.with_j(|i, fx| {
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
                    i.note(LogEvent::Durable {
                        artifact,
                        step: step.clone(),
                    });
                }
                if let Some(lease) = i.fence.origins.get(&(wallet, artifact)) {
                    fx.changed(*lease);
                }
                // A settlement this write blocked (`Settling::Blocked`, by
                // `finish` or `note_executed`) proceeds now, whether the
                // write landed or not: a copy that has not joined yet
                // defers to it (review P2a r1 F2).
                let idle = step.is_some()
                    && !i.fence.sent.contains(&artifact)
                    && i.fence.rowless.get(&artifact).is_some_and(|r| {
                        r.wallet == wallet
                            && matches!(
                                r.state,
                                RowlessState::Admitted {
                                    running: 0,
                                    possibly_out,
                                    slot_taken,
                                } if settles_idle(possibly_out, slot_taken)
                            )
                    });
                if idle {
                    fx.notify();
                    start_settle(&table, i, fx, &artifact)
                } else {
                    Settling::Now
                }
            });
            if let Settling::Durably(wallet) = settling {
                table.resolve_unsent(wallet, artifact);
            }
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
                        writes: vec![self.spawn_write(backend, wallet, id, None, owner)],
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
                                writes: vec![self.spawn_write(backend, wallet, id, None, owner)],
                                then: After::First {
                                    permit,
                                    placeholder: false,
                                },
                            }
                        }
                        None => {
                            revoke_unsent(self, i, fx, key, charge);
                            #[cfg(test)]
                            i.note(LogEvent::Refused {
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
        let entry = i
            .fence
            .rowless
            .get(&id)
            .filter(|r| r.wallet == wallet)
            .map(|r| r.state);
        let joins = match entry {
            Some(RowlessState::Revoked) => {
                return Step::Done(Verdict::Refused {
                    cleanup: false,
                    step_possibly_dispatched: false,
                });
            }
            // Its definite resolution is being made durable.
            Some(RowlessState::Resolving { .. }) => return Step::Done(Verdict::Deferred),
            Some(RowlessState::Admitted { .. }) => true,
            // A definite resolution supersedes every marker: an identical
            // signature is a fresh First under a live lease (§5.5).
            Some(RowlessState::NotSent) => false,
            None => false,
        };
        if !joins && req.kind == (ArtifactKind::CoreTx { asset_lock: true }) {
            return Step::Done(Verdict::Refused {
                cleanup: false,
                step_possibly_dispatched: false,
            });
        }
        // H10 (review P2a r1 F1): every copy of an artifact, whatever its
        // scope, transports only once each step marker of the artifact,
        // and its own step's, is durable. A copy of an admitted artifact
        // joins it; a marked one from this or an earlier process resends.
        #[cfg(test)]
        let superseded =
            entry == Some(RowlessState::NotSent) && i.mutation != Some(Mutation::MarkerOutlives);
        #[cfg(not(test))]
        let superseded = entry == Some(RowlessState::NotSent);
        let marked = !superseded && i.fence.marked(&wallet, &id);
        if joins || marked {
            #[cfg(test)]
            let obligation = if i.mutation == Some(Mutation::CopyBeforeMarker) {
                // The mutation: a copy goes out whatever its markers' state.
                Obligation::Met
            } else {
                i.fence.obligation(&wallet, &id, step.as_deref())
            };
            #[cfg(not(test))]
            let obligation = i.fence.obligation(&wallet, &id, step.as_deref());
            match obligation {
                Obligation::Pending => return Step::Done(Verdict::Deferred),
                Obligation::Met => {
                    admit_rowless(i, id, Rowless::admitted(wallet, req.kind, None, true));
                    return Step::Done(Verdict::Resend(self.guard(i, key, false)));
                }
                Obligation::Write(steps) => {
                    let Some(backend) = backend else {
                        return Step::Done(Verdict::Deferred);
                    };
                    let writes = steps
                        .into_iter()
                        .map(|s| {
                            i.fence.next += 1;
                            let owner = i.fence.next;
                            i.fence
                                .steps
                                .entry((wallet, s.clone()))
                                .or_default()
                                .insert(id, Mark::Committing { owner });
                            #[cfg(test)]
                            i.note(LogEvent::Marking {
                                artifact: id,
                                step: s.clone(),
                            });
                            self.spawn_write(Arc::clone(&backend), wallet, id, Some(s), owner)
                        })
                        .collect();
                    return Step::Write {
                        writes,
                        then: After::Resend {
                            registered: false,
                            rowless: Some(Rowless::admitted(wallet, req.kind, None, true)),
                        },
                    };
                }
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
                let guard = self.guard(i, key, false);
                #[cfg(test)]
                if let Some(LogEvent::Grant { kind, .. }) = i.log.last_mut() {
                    *kind = GrantKind::Unleased;
                }
                Step::Done(Verdict::FirstUnleased(guard))
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
                        // A zero cost is no authority: Credits must be
                        // granted (§3.1).
                        ArtifactKind::Transition { .. } => {
                            i.leases.get(&lease).is_some_and(|e| e.credits.granted())
                        }
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
                        #[cfg(test)]
                        i.note(LogEvent::Marking {
                            artifact: id,
                            step: s.clone(),
                        });
                        // The entry exists from here on, so a copy scoped
                        // another way joins it instead of charging again.
                        i.fence
                            .rowless
                            .insert(id, Rowless::admitted(wallet, req.kind, charge, false));
                        let permit = self.permit(i, fx, key, lease, false);
                        Step::Write {
                            writes: vec![self.spawn_write(
                                backend,
                                wallet,
                                id,
                                Some(s.clone()),
                                owner,
                            )],
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
        i.note(LogEvent::Refused {
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
        let (settlement, durably) = self.with_j(|i, fx| {
            fx.notify();
            let Some(a) = i.fence.attempts.remove(&attempt) else {
                return (Settlement::MaybeOut, None);
            };
            #[cfg(test)]
            i.note(LogEvent::Finish { attempt, outcome });
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
                let s = if i.fence.sent.contains(&id) {
                    Settlement::Sent
                } else {
                    Settlement::MaybeOut
                };
                return (s, None);
            }
            let Some(r) = i.fence.rowless.get_mut(&id) else {
                return (Settlement::MaybeOut, None);
            };
            let RowlessState::Admitted {
                running,
                possibly_out,
                slot_taken,
            } = &mut r.state
            else {
                return (Settlement::MaybeOut, None);
            };
            *running = running.saturating_sub(1);
            if outcome != Outcome::NotSent {
                *possibly_out = true;
            }
            if i.fence.sent.contains(&id) {
                return (Settlement::Sent, None);
            }
            let definite = !*possibly_out && outcome == Outcome::NotSent;
            // H16: another transition used the slot; this one cannot
            // execute. Tombstoned, but the bytes may be out.
            if *running > 0 || !settles_idle(*possibly_out, *slot_taken) {
                return (Settlement::MaybeOut, None);
            }
            match start_settle(self, i, fx, &id) {
                Settling::Now if definite => (Settlement::DefinitelyUnsent, None),
                Settling::Now | Settling::Blocked => (Settlement::MaybeOut, None),
                Settling::Durably(wallet) => (Settlement::MaybeOut, Some((wallet, id, definite))),
            }
        });
        match durably {
            Some((wallet, id, definite)) => {
                if self.resolve_unsent(wallet, id) && definite {
                    Settlement::DefinitelyUnsent
                } else {
                    Settlement::MaybeOut
                }
            }
            None => settlement,
        }
    }

    /// Makes the definite resolution of a `Resolving` `id` durable, then
    /// settles it (review P2a r1 F2): the journal loses its step markers
    /// before the refund and `NotSent` are seen, so no reload or resume
    /// can take it for possibly dispatched. When the write fails it stays
    /// `Resolving{failed}`, its charge spent, until the next `admit` of it
    /// retries; a panicking write counts as failed. An artifact seen sent
    /// before the delete keeps its markers; one seen sent during it gets
    /// them written again. Outside J; blocks on the journal (inline on a
    /// blocking thread).
    fn resolve_unsent(&self, wallet: WalletId, id: ArtifactId) -> bool {
        let backend = self.journal.get();
        let resolving = self.with_j(|i, fx| {
            fx.notify();
            !keep_sent(i, &id)
                && i.fence
                    .rowless
                    .get(&id)
                    .is_some_and(|r| matches!(r.state, RowlessState::Resolving { .. }))
        });
        if !resolving {
            return false;
        }
        let written = backend.as_ref().is_some_and(|backend| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                blocking(|| backend.resolve_unsent(&wallet.0, &id.0))
            }))
            .unwrap_or_else(|_| Err("the journal delete panicked".into()))
            .inspect_err(
                |e| tracing::warn!(artifact = %id, error = %e, "resolving a step marker failed"),
            )
            .is_ok()
        });
        let (settled, rewrite) = self.with_j(|i, fx| {
            fx.notify();
            if keep_sent(i, &id) {
                let steps: Vec<String> = i
                    .fence
                    .steps
                    .iter()
                    .filter(|((w, _), m)| *w == wallet && m.contains_key(&id))
                    .map(|((_, s), _)| s.clone())
                    .collect();
                return (false, if written { steps } else { Vec::new() });
            }
            let Some(r) = i.fence.rowless.get_mut(&id) else {
                return (false, Vec::new());
            };
            let RowlessState::Resolving { slot_taken, .. } = r.state else {
                return (false, Vec::new());
            };
            if !written {
                r.state = RowlessState::Resolving {
                    slot_taken,
                    failed: true,
                };
                return (false, Vec::new());
            }
            for ((w, _), m) in i.fence.steps.iter_mut() {
                if *w == wallet {
                    m.remove(&id);
                }
            }
            i.fence.steps.retain(|_, m| !m.is_empty());
            settle_unsent(self, i, fx, &id);
            (true, Vec::new())
        });
        if let Some(backend) = backend {
            for step in rewrite {
                if let Err(e) = blocking(|| backend.insert_step(&wallet.0, &step, &id.0)) {
                    tracing::warn!(artifact = %id, error = %e, "restoring a sent artifact's marker failed");
                }
            }
        }
        settled
    }

    fn discard(&self, attempt: u64) {
        self.with_j(|i, fx| {
            fx.notify();
            let a = i.fence.attempts.remove(&attempt);
            // Ended without its transport ever being called.
            #[cfg(test)]
            if a.is_some() {
                i.note(LogEvent::Finish {
                    attempt,
                    outcome: Outcome::NotSent,
                });
            }
            if let Some(a) = a
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
        let durably = self.with_j(|i, fx| {
            let signed_here = i.fence.rowless.get(&executed).is_some_and(|r| {
                r.wallet == wallet
                    && matches!(r.kind, ArtifactKind::Transition { slot: Some(s), .. } if s == slot)
            });
            if !signed_here {
                return Vec::new();
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
            let mut durably = Vec::new();
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
                    if *running == 0
                        && let Settling::Durably(w) = start_settle(self, i, fx, &id)
                    {
                        durably.push((w, id));
                    }
                }
            }
            durably
        });
        for (w, id) in durably {
            self.resolve_unsent(w, id);
        }
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
                    RowlessState::Admitted { .. } | RowlessState::Resolving { .. } => {
                        DispatchState::MaybeSent
                    }
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

    /// Erases a removed wallet's journal rows (§6.5) unless one of them may
    /// be on the wire. Runs under the removal's barrier, which refuses new
    /// registrations, and first waits for the running ones to land
    /// (DEC-134): no `Unsent` row of the wallet is written after the erase.
    pub(crate) async fn erase_wallet_rows(&self, wallet: WalletId) {
        self.wait_until(|i| (!i.fence.registering.keys().any(|(w, _)| *w == wallet)).then_some(()))
            .await;
        #[cfg(test)]
        self.with_j(|i, _| {
            i.note(LogEvent::Erase {
                wallet,
                done: false,
            })
        });
        if self.wallet_possibly_sent(&wallet) {
            tracing::warn!(wallet_id = %wallet, "keeping the dispatch records of a possibly sent asset lock");
            return;
        }
        let Some(backend) = self.journal.get() else {
            return;
        };
        let erased = self
            .rt
            .spawn_blocking(move || backend.erase_wallet(&wallet.0))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r);
        #[cfg(test)]
        self.with_j(|i, _| i.note(LogEvent::Erase { wallet, done: true }));
        match erased {
            Ok(()) => self.forget_wallet_entries(&wallet),
            Err(e) => {
                tracing::warn!(wallet_id = %wallet, error = %e, "erasing dispatch records failed")
            }
        }
    }

    /// Whether `wallet` has an entry that may be on the wire; a wiping
    /// removal keeps the journal's rows then (§6.5).
    fn wallet_possibly_sent(&self, wallet: &WalletId) -> bool {
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
    fn forget_wallet_entries(&self, wallet: &WalletId) {
        self.with_j(|i, _| {
            i.fence.entries.retain(|(w, _), _| w != wallet);
            i.fence.origins.retain(|(w, _), _| w != wallet);
            i.fence.steps.retain(|(w, _), _| w != wallet);
            i.fence.rowless.retain(|_, r| r.wallet != *wallet);
            i.fence.spend.retain(|_, s| s.wallet != *wallet);
        });
    }

    /// L5 (review P2a r1 F3): a hand-off bounded by a First's permit
    /// `deadline`. Every wait before the library (`ready`) runs under it,
    /// the deadline is checked last, right before `send` enters the
    /// library, and before each later poll of `send`. `None` (a Resend, an
    /// unleased First): unbounded.
    pub(crate) async fn hand_off<R, E, T, Fs>(
        &self,
        deadline: Option<Instant>,
        ready: impl Future<Output = Result<R, E>>,
        send: impl FnOnce(R) -> Fs,
    ) -> Result<HandOff<T>, E>
    where
        Fs: Future<Output = T>,
    {
        #[cfg(test)]
        let deadline =
            deadline.filter(|_| self.with_j(|i, _| i.mutation) != Some(Mutation::LateEntry));
        let Some(d) = deadline else {
            return Ok(HandOff::Done(send(ready.await?).await));
        };
        let Ok(ready) = tokio::time::timeout_at(d, ready).await else {
            return Ok(HandOff::NotEntered);
        };
        let ready = ready?;
        if Instant::now() >= d {
            return Ok(HandOff::NotEntered);
        }
        // The deadline is checked before every poll of the library, not
        // after it as `timeout_at` does: nothing of it runs at or past `d`.
        let mut send = std::pin::pin!(send(ready));
        let mut expiry = std::pin::pin!(tokio::time::sleep_until(d));
        Ok(std::future::poll_fn(|cx| {
            if Instant::now() >= d || expiry.as_mut().poll(cx).is_ready() {
                return std::task::Poll::Ready(HandOff::Cut);
            }
            send.as_mut().poll(cx).map(HandOff::Done)
        })
        .await)
    }

    /// Logs a transport call in J order (the stress checker's record):
    /// `step` is the scope's step of the copy that made it.
    #[cfg(test)]
    pub(crate) fn log_transport(&self, artifact: ArtifactId, attempt: u64, step: Option<String>) {
        self.with_j(|i, _| {
            i.note(LogEvent::Transport {
                artifact,
                attempt,
                at: Instant::now(),
                step,
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
