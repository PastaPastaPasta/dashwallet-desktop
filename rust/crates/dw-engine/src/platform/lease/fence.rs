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

use std::collections::{BTreeSet, HashMap, HashSet};
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
use dw_appdb::dispatch::Resolution;

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
    /// Appends the artifact's resolution (DEC-154): `NotSent` supersedes
    /// its markers so far. The engine writes `Sent` only through
    /// `resolve_sent`.
    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String>;
    /// A marked artifact's `Sent` row with its marker set, in one
    /// transaction (DEC-163).
    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String>;
    /// A wallet's removal (DEC-160): `dispatch` rows only with `dispatch`,
    /// and the compactable artifacts' `step_log` rows, the predicate
    /// evaluated inside the delete. Returns the artifacts it erased.
    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String>;
    /// The `EraseAll` mutation's unconditional delete.
    #[cfg(test)]
    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String>;
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

    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String> {
        dw_appdb::dispatch::DispatchJournal::resolve(
            self,
            wallet,
            artifact,
            r,
            crate::events::unix_now(),
        )
        .map_err(|e| e.to_string())
    }

    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
    ) -> Result<(), String> {
        self.resolve_sent(wallet, artifact, steps, crate::events::unix_now())
            .map_err(|e| e.to_string())
    }

    fn erase_wallet(&self, wallet: &[u8; 32], dispatch: bool) -> Result<Vec<[u8; 32]>, String> {
        self.erase_wallet(wallet, dispatch)
            .map_err(|e| e.to_string())
    }

    #[cfg(test)]
    fn erase_wallet_unconditionally(&self, wallet: &[u8; 32]) -> Result<Vec<[u8; 32]>, String> {
        self.erase_wallet_unconditionally(wallet)
            .map_err(|e| e.to_string())
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
    pub(crate) state: RowlessState,
    kind: ArtifactKind,
    /// The charge the settlement refunds: (lease, purpose, charge).
    charge: Option<(LeaseId, BudgetPurpose, Charge)>,
}

impl Rowless {
    fn admitted(
        kind: ArtifactKind,
        charge: Option<(LeaseId, BudgetPurpose, Charge)>,
        possibly_out: bool,
    ) -> Self {
        Rowless {
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
    /// Row-less artifacts, per wallet (review O-1): the same bytes under
    /// two wallets are two entries, and `decide` keeps one in flight.
    rowless: HashMap<(WalletId, ArtifactId), Rowless>,
    steps: HashMap<(WalletId, String), HashMap<ArtifactId, Mark>>,
    /// A marked artifact's `Sent` row (DEC-154): its markers stand on disk
    /// over any `NotSent` row only once it is durable.
    evidence: HashMap<(WalletId, ArtifactId), Mark>,
    /// Markers a durable `NotSent` row superseded in this process, with
    /// what was known of their own write; a `Sent` row brings them back as
    /// they were, so one never durable is written again.
    superseded: HashMap<(WalletId, ArtifactId), Vec<(String, Mark)>>,
    /// Journal writes running (`spawn_write`, a `NotSent` row, a removal's
    /// erase); closing the journal waits for them.
    writes: usize,
    spend: HashMap<(WalletId, ArtifactId), SpendCharge>,
    /// Every artifact an attempt finished `Sent` for, under any wallet:
    /// identical bytes are one transaction. Only grows.
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
    /// artifact appends its `NotSent` row to the journal before it returns
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
    /// artifact appends its `NotSent` row to the journal before it returns
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
        match self.rowless.get(&(*wallet, *id)).map(|r| r.state) {
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

    #[cfg(test)]
    pub(crate) fn resolving(&self, key: &(WalletId, ArtifactId)) -> bool {
        self.rowless
            .get(key)
            .is_some_and(|r| matches!(r.state, RowlessState::Resolving { .. }))
    }

    /// The artifact's current marker set (DEC-163): every step marked in
    /// memory, superseded or not, whatever its write did. A `Sent` row is
    /// written with all of them, since a `NotSent` row or a removal's
    /// compaction may have taken any.
    fn marker_set(&self, wallet: &WalletId, id: &ArtifactId) -> Vec<String> {
        let set: BTreeSet<&String> = self
            .steps
            .iter()
            .filter(|((w, _), m)| w == wallet && m.contains_key(id))
            .map(|((_, s), _)| s)
            .chain(
                self.superseded
                    .get(&(*wallet, *id))
                    .into_iter()
                    .flatten()
                    .map(|(s, _)| s),
            )
            .collect();
        set.into_iter().cloned().collect()
    }

    /// Whether `id` has a step marker of `wallet`, in any step.
    fn marked(&self, wallet: &WalletId, id: &ArtifactId) -> bool {
        self.steps
            .iter()
            .any(|((w, _), m)| w == wallet && m.contains_key(id))
    }

    /// H10 for one copy of `id` (review P2a r1 F1): every marker of the
    /// artifact, and the marker of the copy's own `step`, must be durable,
    /// and so must its `Sent` row once begun (DEC-154, Sol r2 R2-F1): a
    /// `NotSent` row may supersede the markers until then. A write that
    /// failed may have committed, so it is written again (`Ambiguous`).
    fn obligation(
        &self,
        wallet: &WalletId,
        id: &ArtifactId,
        step: Option<&str>,
        evidence: bool,
    ) -> Obligation {
        let mut write = Vec::new();
        match self.evidence.get(&(*wallet, *id)) {
            _ if !evidence => {}
            Some(Mark::Committing { .. }) => return Obligation::Pending,
            Some(Mark::Ambiguous) => write.push(Target::Sent(self.marker_set(wallet, id))),
            Some(Mark::Durable) | None => {}
        }
        for ((w, s), m) in &self.steps {
            match m.get(id) {
                _ if w != wallet => {}
                Some(Mark::Committing { .. }) => return Obligation::Pending,
                Some(Mark::Ambiguous) => write.push(Target::Step(s.clone())),
                Some(Mark::Durable) | None => {}
            }
        }
        if let Some(s) = step
            && !self
                .steps
                .get(&(*wallet, s.to_owned()))
                .is_some_and(|m| m.contains_key(id))
        {
            write.push(Target::Step(s.to_owned()));
        }
        if write.is_empty() {
            Obligation::Met
        } else {
            Obligation::Write(write)
        }
    }

    #[cfg(test)]
    pub(crate) fn evidence_mark(&self, wallet: &WalletId, id: &ArtifactId) -> Option<Mark> {
        self.evidence.get(&(*wallet, *id)).copied()
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

/// A row-less map key: the wallet and the artifact (review O-1).
type RowKey = (WalletId, ArtifactId);

/// `wallet`'s row-less key of `id`. The `ArtifactKeyed` mutation keys by
/// the artifact alone, as before review O-1.
fn row_key(i: &Inner, wallet: WalletId, id: ArtifactId) -> RowKey {
    #[cfg(test)]
    if i.mutation == Some(Mutation::ArtifactKeyed) {
        return (WalletId([0; 32]), id);
    }
    let _ = i;
    (wallet, id)
}

/// The row-less artifact `id` settles definitely unsent: refund, tombstone
/// (§5.5), `DispatchResolved(NotSent)`.
fn settle_unsent(t: &LeaseTable, i: &mut Inner, fx: &mut Effects, key: RowKey) {
    let Some(r) = i.fence.rowless.get_mut(&key) else {
        return;
    };
    r.state = r.kind.tombstone();
    let charge = r.charge.take();
    let (wallet, id) = key;
    #[cfg(test)]
    i.note(LogEvent::Resolved {
        artifact: id,
        refund: charge.map_or(0, |(_, _, c)| c.amount),
    });
    if let Some((lease, purpose, charge)) = charge
        && let Some(e) = i.leases.get_mut(&lease)
    {
        e.refund(purpose, charge);
        fx.changed(lease);
    }
    i.fence.spend.remove(&key);
    resolved(t, fx, &wallet, &id, DispatchResolution::NotSent);
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
    /// `Resolving`: `LeaseTable::resolve_not_sent` settles it once the
    /// journal holds its `NotSent` row.
    Durably,
    /// A marker write of it runs: the write's own J step settles it
    /// (`spawn_write`) unless a copy joined meanwhile.
    Blocked,
}

/// Starts settling the row-less `id` definitely unsent (§5.5). A marked
/// artifact's resolution must be durable before it is seen (review P2a r1
/// F2).
fn start_settle(t: &LeaseTable, i: &mut Inner, fx: &mut Effects, key: RowKey) -> Settling {
    if !i.fence.rowless.contains_key(&key) {
        return Settling::Now;
    }
    let (wallet, id) = &key;
    #[cfg(test)]
    let in_memory = i.mutation == Some(Mutation::MarkerOutlives);
    #[cfg(not(test))]
    let in_memory = false;
    // The mutation settles in memory, its markers kept in the journal.
    if in_memory || !i.fence.marked(wallet, id) {
        settle_unsent(t, i, fx, key);
        return Settling::Now;
    }
    if matches!(owed(i, wallet, id, None), Obligation::Pending) {
        return Settling::Blocked;
    }
    let Some(r) = i.fence.rowless.get_mut(&key) else {
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
    Settling::Durably
}

/// Whether an `Admitted` row-less entry with no attempt running settles:
/// every attempt ended definitely unsent, or another transition took its
/// nonce slot (H16). The caller has checked it was never seen sent.
fn settles_idle(possibly_out: bool, slot_taken: bool) -> bool {
    !possibly_out || slot_taken
}

/// A `Resolving` entry seen sent meanwhile is possibly out again, its
/// charge and markers kept: a sent artifact is never refunded, and its
/// `Sent` row makes the markers stand over any `NotSent` row (DEC-154).
fn keep_sent(i: &mut Inner, key: RowKey) -> bool {
    if !i.fence.sent.contains(&key.1) {
        return false;
    }
    if let Some(r) = i.fence.rowless.get_mut(&key)
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

/// Applies a removal's erase in memory (DEC-160, DEC-163): the wallet's
/// entries when its `dispatch` rows went, and each artifact whose
/// `step_log` rows went, but only while nothing newer may be known of it.
/// One with a `Sent` row begun or owed, a marker in `steps`, or an attempt
/// or `NotSent` write in flight (or failed) may have changed after the
/// delete read it, so it stays; what is forgotten is a settled artifact's
/// superseded marks and its tombstone. Under J, in the J step the delete's
/// result arrives in.
fn forget_erased(i: &mut Inner, wallet: WalletId, dispatch: bool, erased: Vec<[u8; 32]>) {
    #[cfg(test)]
    let all = i.mutation == Some(Mutation::ForgetNewer);
    #[cfg(not(test))]
    let all = false;
    let f = &mut i.fence;
    if dispatch {
        f.entries.retain(|(w, _), _| *w != wallet);
        f.origins.retain(|(w, _), _| *w != wallet);
        f.spend.retain(|(w, _), _| *w != wallet);
    }
    let gone: HashSet<ArtifactId> = erased
        .into_iter()
        .map(ArtifactId)
        .filter(|a| {
            all || !(f.evidence.contains_key(&(wallet, *a))
                || f.marked(&wallet, a)
                || f.rowless.get(&(wallet, *a)).is_some_and(|r| {
                    matches!(
                        r.state,
                        RowlessState::Admitted { .. } | RowlessState::Resolving { .. }
                    )
                }))
        })
        .collect();
    let forgotten = |w: &WalletId, a: &ArtifactId| *w == wallet && gone.contains(a);
    for ((w, _), m) in f.steps.iter_mut() {
        m.retain(|a, _| !forgotten(w, a));
    }
    f.steps.retain(|_, m| !m.is_empty());
    f.evidence.retain(|(w, a), _| !forgotten(w, a));
    f.superseded.retain(|(w, a), _| !forgotten(w, a));
    f.rowless.retain(|(w, a), _| !forgotten(w, a));
    #[cfg(test)]
    for artifact in gone {
        i.note(LogEvent::Erased { wallet, artifact });
    }
}

/// The bundle mutations (DEC-163): `BundleOwedOnly` drops the markers it
/// believes durable; `SplitBundle` writes them as separate statements.
#[cfg(test)]
fn mutate_bundle(i: &Inner, wallet: WalletId, artifact: ArtifactId, target: Target) -> Target {
    match target {
        Target::Sent(mut steps) if i.mutation == Some(Mutation::BundleOwedOnly) => {
            steps.retain(|s| i.fence.mark(&wallet, s, &artifact) != Some(Mark::Durable));
            Target::Sent(steps)
        }
        target => target,
    }
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
    /// A marker or `Sent` write runs.
    Pending,
    /// These are missing or ambiguous: write them first.
    Write(Vec<Target>),
}

/// What a durable write of `spawn_write` records.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    /// A registered artifact's `Dispatching`.
    Record,
    /// A step's write-ahead marker.
    Step(String),
    /// The artifact's `Sent` row (DEC-154) with its complete current
    /// marker set, in one SQLite transaction (DEC-163), so the row never
    /// lands without them.
    Sent(Vec<String>),
}

/// [`FenceState::obligation`] under J; the `SkipEvidence` mutation ignores
/// the `Sent` row.
fn owed(i: &Inner, wallet: &WalletId, id: &ArtifactId, step: Option<&str>) -> Obligation {
    #[cfg(test)]
    let evidence = i.mutation != Some(Mutation::SkipEvidence);
    #[cfg(not(test))]
    let evidence = true;
    i.fence.obligation(wallet, id, step, evidence)
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
fn admit_rowless(i: &mut Inner, key: RowKey, r: Rowless) {
    match i.fence.rowless.get_mut(&key).map(|e| &mut e.state) {
        Some(RowlessState::Admitted { running, .. }) => {
            *running += 1;
        }
        _ => {
            i.fence.rowless.insert(key, r);
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
    key: RowKey,
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
            let Some(r) = i.fence.rowless.get_mut(&self.key) else {
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
                i.fence.rowless.remove(&self.key);
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
                .get(&row_key(i, wallet, artifact))
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
                if s.sent {
                    i.fence.sent.insert(ArtifactId(s.artifact));
                    i.fence
                        .evidence
                        .insert((WalletId(s.wallet), ArtifactId(s.artifact)), Mark::Durable);
                }
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

    /// Closes the journal (last step of close, §8.5). No write begins once
    /// it is not `Ready`; the ones running land first, so none of this
    /// session lands after the next one's sweep (review r2 L1).
    pub(crate) async fn close_journal(self: &Arc<Self>) {
        let backend = self.journal.get();
        self.with_j(|i, _| {
            // A last try for each `Sent` row a failed write left owed
            // (review r2 M2).
            if let Some(backend) = backend.filter(|_| i.fence.journal == JournalState::Ready) {
                self.retry_owed_sent(i, &backend, None);
            }
            i.fence.journal = JournalState::NotLoaded;
        });
        // A `register` write is not counted in `writes`; its `registering`
        // marker stands until its J step runs (review O-5).
        self.wait_until(|i| (i.fence.writes == 0 && i.fence.registering.is_empty()).then_some(()))
            .await;
        self.journal.set(None);
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
            let key = row_key(i, wallet, txid);
            if i.fence
                .rowless
                .get(&key)
                .is_some_and(|r| r.state == RowlessState::Revoked)
            {
                i.fence.rowless.remove(&key);
            }
            i.fence.spend.insert(key, SpendCharge { lease, charge });
        });
    }

    /// A prepared `TxDraft` released before its charge was admitted:
    /// refunds it, if `lease` still holds the binding. An admission moves
    /// the charge to the row-less entry, whose settlement refunds it.
    pub(crate) fn unbind_spend(&self, lease: LeaseId, wallet: WalletId, txid: ArtifactId) {
        self.with_j(|i, fx| {
            let key = row_key(i, wallet, txid);
            if i.fence.spend.get(&key).is_some_and(|s| s.lease == lease)
                && let Some(s) = i.fence.spend.remove(&key)
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
        let key = self.with_j(|i, _| row_key(i, req.wallet, req.artifact));
        let retry = self.with_j(|i, _| {
            let Some(r) = i.fence.rowless.get_mut(&key) else {
                return false;
            };
            let RowlessState::Resolving {
                slot_taken,
                failed: true,
            } = r.state
            else {
                return false;
            };
            r.state = RowlessState::Resolving {
                slot_taken,
                failed: false,
            };
            true
        });
        if retry {
            let table = Arc::clone(self);
            let (wallet, id) = (req.wallet, req.artifact);
            let _ = self
                .rt
                .spawn_blocking(move || table.resolve_not_sent(wallet, id))
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
                    key,
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
                        // caller's next admit reads it. A sighting outranks
                        // a settled `NotSent`, as in `decide`; `Resolving`
                        // and `Revoked` still defer.
                        if i.fence.rowless.get(&key).is_some_and(|e| match e.state {
                            RowlessState::Admitted { .. } => false,
                            RowlessState::NotSent => !i.fence.sent.contains(&req.artifact),
                            _ => true,
                        }) {
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
                            && !matches!(owed(i, &req.wallet, &req.artifact, step), Obligation::Met)
                        {
                            return Verdict::Deferred;
                        }
                        admit_rowless(i, key, r);
                    }
                    Verdict::Resend(self.guard(i, (req.wallet, req.artifact), registered))
                })
            }
        }
    }

    /// Spawns the durable write of `target` for its `Committing` entry,
    /// inside the J step that set it. The task resolves the entry itself; a
    /// write that fails or panics may have committed, so its entry becomes
    /// `Ambiguous` and is written again before any copy (DEC-154 (3)).
    fn spawn_write(
        self: &Arc<Self>,
        i: &mut Inner,
        backend: Arc<dyn JournalBackend>,
        wallet: WalletId,
        artifact: ArtifactId,
        target: Target,
        owner: u64,
    ) -> tokio::task::JoinHandle<bool> {
        i.fence.writes += 1;
        #[cfg(test)]
        let split = i.mutation == Some(Mutation::SplitBundle);
        #[cfg(test)]
        let target = mutate_bundle(i, wallet, artifact, target);
        let table = Arc::clone(self);
        self.rt.spawn_blocking(move || {
            let (w, a) = (&wallet.0, &artifact.0);
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &target {
                Target::Record => matches!(backend.mark_dispatching(w, a), Ok(true)),
                Target::Step(s) => backend.insert_step(w, s, a).is_ok(),
                #[cfg(test)]
                Target::Sent(steps) if split => {
                    // The mutation: the bundle as separate statements.
                    steps.iter().all(|s| backend.insert_step(w, s, a).is_ok())
                        && backend.resolve(w, a, Resolution::Sent).is_ok()
                }
                Target::Sent(steps) => backend.resolve_sent(w, a, steps).is_ok(),
            }))
            .unwrap_or(false);
            let settling = table.with_j(|i, fx| {
                i.fence.writes -= 1;
                fx.notify();
                // A `Sent` row an earlier write failed is retried once the
                // journal takes a write again (review r2 M2).
                if ok && i.fence.journal == JournalState::Ready {
                    table.retry_owed_sent(i, &backend, Some(wallet));
                }
                let mark = match &target {
                    Target::Record => {
                        if let Some(e) = i.fence.entries.get_mut(&(wallet, artifact))
                            && *e == (Reg::Committing { owner })
                        {
                            *e = if ok { Reg::Dispatching } else { Reg::Ambiguous };
                        }
                        None
                    }
                    Target::Step(s) => i
                        .fence
                        .steps
                        .get_mut(&(wallet, s.clone()))
                        .and_then(|m| m.get_mut(&artifact)),
                    Target::Sent(_) => i.fence.evidence.get_mut(&(wallet, artifact)),
                };
                if let Some(m) = mark
                    && *m == (Mark::Committing { owner })
                {
                    *m = if ok { Mark::Durable } else { Mark::Ambiguous };
                }
                #[cfg(test)]
                if ok {
                    i.note(match &target {
                        Target::Record => LogEvent::Durable {
                            artifact,
                            step: None,
                        },
                        Target::Step(s) => LogEvent::Durable {
                            artifact,
                            step: Some(s.clone()),
                        },
                        Target::Sent(_) => LogEvent::Evidence { artifact },
                    });
                }
                // A bundle wrote its markers with its row (DEC-163).
                if ok && let Target::Sent(steps) = &target {
                    for s in steps {
                        if let Some(m) = i
                            .fence
                            .steps
                            .get_mut(&(wallet, s.clone()))
                            .and_then(|m| m.get_mut(&artifact))
                            .filter(|m| **m == Mark::Ambiguous)
                        {
                            *m = Mark::Durable;
                        }
                        #[cfg(test)]
                        i.note(LogEvent::Durable {
                            artifact,
                            step: Some(s.clone()),
                        });
                    }
                }
                if let Some(lease) = i.fence.origins.get(&(wallet, artifact)) {
                    fx.changed(*lease);
                }
                // A settlement this write blocked (`Settling::Blocked`, by
                // `finish` or `note_executed`) proceeds now, whether the
                // write landed or not: a copy that has not joined yet
                // defers to it (review P2a r1 F2).
                let idle = matches!(target, Target::Step(_))
                    && !i.fence.sent.contains(&artifact)
                    && i.fence
                        .rowless
                        .get(&row_key(i, wallet, artifact))
                        .is_some_and(|r| {
                            matches!(
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
                    start_settle(&table, i, fx, row_key(i, wallet, artifact))
                } else {
                    Settling::Now
                }
            });
            if let Settling::Durably = settling {
                table.resolve_not_sent(wallet, artifact);
            }
            ok
        })
    }

    /// Begins again each `Sent` row a failed write left owed, of `wallet` or
    /// of all wallets. Under J.
    fn retry_owed_sent(
        self: &Arc<Self>,
        i: &mut Inner,
        backend: &Arc<dyn JournalBackend>,
        wallet: Option<WalletId>,
    ) {
        let owed: Vec<(WalletId, ArtifactId)> = i
            .fence
            .evidence
            .iter()
            .filter(|((w, _), m)| wallet.is_none_or(|x| x == *w) && **m == Mark::Ambiguous)
            .map(|(k, _)| *k)
            .collect();
        for (w, a) in owed {
            self.begin_sent_bundle(i, Arc::clone(backend), w, a);
        }
    }

    /// Begins the artifact's `Sent` row with its whole marker set, as one
    /// write (DEC-163). Under J.
    fn begin_sent_bundle(
        self: &Arc<Self>,
        i: &mut Inner,
        backend: Arc<dyn JournalBackend>,
        wallet: WalletId,
        artifact: ArtifactId,
    ) {
        i.fence.next += 1;
        let owner = i.fence.next;
        i.fence
            .evidence
            .insert((wallet, artifact), Mark::Committing { owner });
        let steps = i.fence.marker_set(&wallet, &artifact);
        drop(self.spawn_write(i, backend, wallet, artifact, Target::Sent(steps), owner));
    }

    /// DEC-154: a marked artifact seen sent gets its `Sent` row, which makes
    /// its markers stand for good, over a `NotSent` row written or possibly
    /// written too. Its copies wait for the row (`obligation`). Markers a
    /// `NotSent` row superseded in this process come back with it, each as
    /// its write left it. One that failed is begun again by the next
    /// sighting or the next journal write that lands. Under J.
    fn begin_sent_row(
        self: &Arc<Self>,
        i: &mut Inner,
        backend: Option<Arc<dyn JournalBackend>>,
        wallet: WalletId,
        artifact: ArtifactId,
    ) {
        let key = (wallet, artifact);
        let Some(backend) = backend.filter(|_| i.fence.journal == JournalState::Ready) else {
            return;
        };
        for (s, mark) in i.fence.superseded.remove(&key).unwrap_or_default() {
            i.fence
                .steps
                .entry((wallet, s))
                .or_default()
                .entry(artifact)
                .or_insert(mark);
        }
        // A row whose write failed is begun again (review r2 M2).
        if !i.fence.marked(&wallet, &artifact)
            || matches!(
                i.fence.evidence.get(&key),
                Some(Mark::Committing { .. } | Mark::Durable)
            )
        {
            return;
        }
        #[cfg(test)]
        if i.mutation == Some(Mutation::NoSentRow) {
            // The mutation: memory alone says the markers stand.
            i.fence.evidence.insert(key, Mark::Durable);
            return;
        }
        self.begin_sent_bundle(i, backend, wallet, artifact);
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
                        writes: vec![self.spawn_write(
                            i,
                            backend,
                            wallet,
                            id,
                            Target::Record,
                            owner,
                        )],
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
                                writes: vec![self.spawn_write(
                                    i,
                                    backend,
                                    wallet,
                                    id,
                                    Target::Record,
                                    owner,
                                )],
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

        // Row-less artifacts. One artifact is one transaction (review O-1):
        // while another wallet's entry of the same bytes may be in flight
        // or resolving, a copy for this wallet waits, so neither settles
        // definitely unsent while the other's attempt may put them out.
        let rkey = row_key(i, wallet, id);
        #[cfg(test)]
        let artifact_keyed = i.mutation == Some(Mutation::ArtifactKeyed);
        #[cfg(not(test))]
        let artifact_keyed = false;
        if !artifact_keyed
            && i.fence.rowless.iter().any(|((w, a), r)| {
                *a == id
                    && *w != wallet
                    && matches!(
                        r.state,
                        RowlessState::Admitted { .. } | RowlessState::Resolving { .. }
                    )
            })
        {
            return Step::Done(Verdict::Deferred);
        }
        let entry = i.fence.rowless.get(&rkey).map(|r| r.state);
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
        // A `Sent` row outranks the `NotSent` tombstone (DEC-154).
        #[cfg(test)]
        let superseded = entry == Some(RowlessState::NotSent)
            && !i.fence.sent.contains(&id)
            && i.mutation != Some(Mutation::MarkerOutlives);
        #[cfg(not(test))]
        let superseded = entry == Some(RowlessState::NotSent) && !i.fence.sent.contains(&id);
        let marked = !superseded && i.fence.marked(&wallet, &id);
        if joins || marked {
            #[cfg(test)]
            let obligation = if i.mutation == Some(Mutation::CopyBeforeMarker) {
                // The mutation: a copy goes out whatever its markers' state.
                Obligation::Met
            } else {
                owed(i, &wallet, &id, step.as_deref())
            };
            #[cfg(not(test))]
            let obligation = owed(i, &wallet, &id, step.as_deref());
            match obligation {
                Obligation::Pending => return Step::Done(Verdict::Deferred),
                Obligation::Met => {
                    admit_rowless(i, rkey, Rowless::admitted(req.kind, None, true));
                    return Step::Done(Verdict::Resend(self.guard(i, key, false)));
                }
                Obligation::Write(steps) => {
                    let Some(backend) = backend else {
                        return Step::Done(Verdict::Deferred);
                    };
                    let writes = steps
                        .into_iter()
                        .map(|target| {
                            i.fence.next += 1;
                            let owner = i.fence.next;
                            match &target {
                                Target::Step(s) => {
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
                                }
                                Target::Sent(_) => {
                                    i.fence
                                        .evidence
                                        .insert((wallet, id), Mark::Committing { owner });
                                }
                                Target::Record => {}
                            }
                            self.spawn_write(i, Arc::clone(&backend), wallet, id, target, owner)
                        })
                        .collect();
                    return Step::Write {
                        writes,
                        then: After::Resend {
                            registered: false,
                            rowless: Some(Rowless::admitted(req.kind, None, true)),
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
                    .insert(rkey, Rowless::admitted(req.kind, None, false));
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
                        ArtifactKind::CoreTx { .. } => {
                            i.fence.spend.get(&rkey).is_some_and(|s| s.lease == lease)
                        }
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
                            .remove(&rkey)
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
                            .insert(rkey, Rowless::admitted(req.kind, charge, false));
                        let permit = self.permit(i, fx, key, lease, false);
                        Step::Write {
                            writes: vec![self.spawn_write(
                                i,
                                backend,
                                wallet,
                                id,
                                Target::Step(s.clone()),
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
                            .insert(rkey, Rowless::admitted(req.kind, charge, false));
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
        let key = row_key(i, req.wallet, id);
        let first = !i.fence.rowless.contains_key(&key);
        i.fence.rowless.insert(
            key,
            Rowless {
                state: req.kind.tombstone(),
                kind: req.kind,
                charge: None,
            },
        );
        if let Some(s) = i.fence.spend.remove(&key)
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
            let key = row_key(i, a.wallet, id);
            let Some(r) = i.fence.rowless.get_mut(&key) else {
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
            match start_settle(self, i, fx, key) {
                Settling::Now if definite => (Settlement::DefinitelyUnsent, None),
                Settling::Now | Settling::Blocked => (Settlement::MaybeOut, None),
                Settling::Durably => (Settlement::MaybeOut, Some((a.wallet, id, definite))),
            }
        });
        match durably {
            Some((wallet, id, definite)) => {
                if self.resolve_not_sent(wallet, id) && definite {
                    Settlement::DefinitelyUnsent
                } else {
                    Settlement::MaybeOut
                }
            }
            None => settlement,
        }
    }

    /// Makes the definite resolution of a `Resolving` `id` durable, then
    /// settles it (review P2a r1 F2): a `NotSent` row is appended before
    /// the refund and `NotSent` are seen, so no reload or resume takes the
    /// artifact for possibly dispatched (DEC-154). When the write fails or
    /// panics it may still have committed: the entry stays
    /// `Resolving{failed}`, its charge spent and its copies deferred, until
    /// the next `admit` retries. An artifact seen sent is not resolved: its
    /// `Sent` row (`begin_sent_row`) outranks the `NotSent` row, written or
    /// not, so nothing is restored. Outside J; blocks on the journal
    /// (inline on a blocking thread).
    fn resolve_not_sent(&self, wallet: WalletId, id: ArtifactId) -> bool {
        let backend = self.journal.get();
        let (resolving, write) = self.with_j(|i, fx| {
            fx.notify();
            let key = row_key(i, wallet, id);
            let go = !keep_sent(i, key)
                && i.fence
                    .rowless
                    .get(&key)
                    .is_some_and(|r| matches!(r.state, RowlessState::Resolving { .. }));
            // A closed journal takes no write: the resolution fails and the
            // next admit retries it.
            let write = go && i.fence.journal == JournalState::Ready;
            if write {
                i.fence.writes += 1;
                #[cfg(test)]
                i.note(LogEvent::Superseding { artifact: id });
            }
            (go, write)
        });
        if !resolving {
            return false;
        }
        let written = write
            && backend.as_ref().is_some_and(|backend| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                blocking(|| backend.resolve(&wallet.0, &id.0, Resolution::NotSent))
            }))
            .unwrap_or_else(|_| Err("the journal write panicked".into()))
            .inspect_err(
                |e| tracing::warn!(artifact = %id, error = %e, "resolving a step marker failed"),
            )
            .is_ok()
            });
        self.with_j(|i, fx| {
            fx.notify();
            if write {
                i.fence.writes -= 1;
            }
            let key = row_key(i, wallet, id);
            if keep_sent(i, key) {
                return false;
            }
            let Some(r) = i.fence.rowless.get_mut(&key) else {
                return false;
            };
            let RowlessState::Resolving { slot_taken, .. } = r.state else {
                return false;
            };
            if !written {
                #[cfg(test)]
                if i.mutation == Some(Mutation::FailedUncommitted) {
                    // The mutation: a failed write is taken as uncommitted,
                    // so the markers are read as standing.
                    r.state = RowlessState::Admitted {
                        running: 0,
                        possibly_out: true,
                        slot_taken,
                    };
                    return false;
                }
                r.state = RowlessState::Resolving {
                    slot_taken,
                    failed: true,
                };
                return false;
            }
            let mut gone = Vec::new();
            for ((w, s), m) in i.fence.steps.iter_mut() {
                if *w == wallet
                    && let Some(mark) = m.remove(&id)
                {
                    // Only a marker whose write returned is known on disk;
                    // any other may never have landed (review r2 H1).
                    let mark = if mark == Mark::Durable {
                        Mark::Durable
                    } else {
                        Mark::Ambiguous
                    };
                    gone.push((s.clone(), mark));
                }
            }
            i.fence.steps.retain(|_, m| !m.is_empty());
            let stash = i.fence.superseded.entry((wallet, id)).or_default();
            for (s, mark) in gone {
                match stash.iter_mut().find(|(t, _)| *t == s) {
                    Some((_, m)) if mark == Mark::Durable => *m = Mark::Durable,
                    Some(_) => {}
                    None => stash.push((s, mark)),
                }
            }
            settle_unsent(self, i, fx, key);
            true
        })
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
    pub fn note_executed(
        self: &Arc<Self>,
        wallet: WalletId,
        executed: ArtifactId,
        slot: NonceSlot,
    ) {
        let backend = self.journal.get();
        let durably = self.with_j(|i, fx| {
            let signed_here = i.fence.rowless.get(&row_key(i, wallet, executed)).is_some_and(|r| {
                matches!(r.kind, ArtifactKind::Transition { slot: Some(s), .. } if s == slot)
            });
            if !signed_here {
                return Vec::new();
            }
            if i.fence.sent.insert(executed) {
                resolved(self, fx, &wallet, &executed, DispatchResolution::Sent);
            }
            self.begin_sent_row(i, backend, wallet, executed);
            let executed_key = row_key(i, wallet, executed);
            let others: Vec<RowKey> = i
                .fence
                .rowless
                .iter()
                .filter(|((w, id), r)| {
                    (*w, *id) != executed_key
                        && *w == executed_key.0
                        && matches!(r.kind, ArtifactKind::Transition { slot: Some(s), .. } if s == slot)
                })
                .map(|(key, _)| *key)
                .collect();
            let mut durably = Vec::new();
            for key in others {
                if i.fence.sent.contains(&key.1) {
                    continue;
                }
                let Some(r) = i.fence.rowless.get_mut(&key) else {
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
                        && let Settling::Durably = start_settle(self, i, fx, key)
                    {
                        durably.push(key.1);
                    }
                }
            }
            durably
        });
        for id in durably {
            self.resolve_not_sent(wallet, id);
        }
    }

    /// The wallet saw the transaction (a Resend accepted, or it confirmed):
    /// `Sent` (§4.6).
    pub fn note_seen(self: &Arc<Self>, wallet: WalletId, artifact: ArtifactId) {
        let backend = self.journal.get();
        self.with_j(|i, fx| {
            // Another wallet's sighting says nothing of this one's entry
            // (review r2 L2).
            let known = i.fence.rowless.contains_key(&row_key(i, wallet, artifact))
                || i.fence.entries.contains_key(&(wallet, artifact));
            if !known {
                return;
            }
            if i.fence.sent.insert(artifact) {
                resolved(self, fx, &wallet, &artifact, DispatchResolution::Sent);
            }
            // Begins the row, or again after a failed write.
            self.begin_sent_row(i, backend, wallet, artifact);
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
            if let Some(r) = f.rowless.get(&row_key(i, wallet, artifact)) {
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

    /// Erases a removed wallet's journal rows (§6.5): its `dispatch` rows
    /// unless a registered entry may be on the wire, and of its `step_log`
    /// only what DEC-160's predicate finds compactable, inside the delete.
    /// Runs under the removal's barrier, which refuses new registrations,
    /// and first waits for the running ones to land (DEC-134): no `Unsent`
    /// row of the wallet is written after the erase. Other journal writes
    /// run on: a `Sent` row carries its markers (DEC-163), so either order
    /// against the delete keeps them.
    pub(crate) async fn erase_wallet_rows(self: &Arc<Self>, wallet: WalletId) {
        self.wait_until(|i| (!i.fence.registering.keys().any(|(w, _)| *w == wallet)).then_some(()))
            .await;
        let Some(backend) = self.journal.get() else {
            return;
        };
        let dispatch = !self.wallet_dispatch_possibly_sent(&wallet);
        if !dispatch {
            tracing::warn!(wallet_id = %wallet, "keeping the dispatch records of a possibly sent asset lock");
        }
        #[cfg(test)]
        let all = self.inspect(|i| i.mutation) == Some(Mutation::EraseAll);
        // The delete applies its own outcome, so a removal dropped meanwhile
        // does not skip it; closing the journal waits for it.
        self.with_j(|i, _| {
            i.fence.writes += 1;
            #[cfg(test)]
            i.note(LogEvent::Erase {
                wallet,
                done: false,
            });
        });
        let table = Arc::clone(self);
        let erase = self.rt.spawn_blocking(move || {
            let erased = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if all {
                    // The mutation: the unconditional delete.
                    return backend.erase_wallet_unconditionally(&wallet.0);
                }
                backend.erase_wallet(&wallet.0, dispatch)
            }))
            .unwrap_or_else(|_| Err("the erase panicked".into()));
            // A failed erase may have committed; memory keeps everything,
            // and a `Sent` row still carries every marker it knows.
            if let Err(e) = &erased {
                tracing::warn!(wallet_id = %wallet, error = %e, "erasing dispatch records failed");
            }
            table.with_j(|i, fx| {
                i.fence.writes -= 1;
                fx.notify();
                if let Ok(erased) = erased {
                    let forget = || forget_erased(i, wallet, dispatch, erased);
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(forget));
                }
                #[cfg(test)]
                i.note(LogEvent::Erase { wallet, done: true });
            });
        });
        let _ = erase.await;
    }

    /// Whether `wallet` has a registered entry that may be on the wire; a
    /// wiping removal keeps its `dispatch` rows then (§6.5).
    fn wallet_dispatch_possibly_sent(&self, wallet: &WalletId) -> bool {
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
            !i.fence.rowless.contains_key(&(req.wallet, req.artifact))
                && FenceState::live(&i.leases, &lease, &req.wallet)
        });
        if !live {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
        Some(self.with_j(|i, fx| {
            i.fence.rowless.insert(
                (req.wallet, req.artifact),
                Rowless::admitted(req.kind, None, false),
            );
            Verdict::First(self.permit(i, fx, (req.wallet, req.artifact), lease, false))
        }))
    }
}
