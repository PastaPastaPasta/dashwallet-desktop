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
//!
//! Mode B's call steps (§2a.3 "Durable markers first") are ordinary step
//! markers of a synthetic artifact ([`ArtifactId::of_step`]), so DEC-154,
//! DEC-160 and DEC-163 hold for them unchanged. A funding step's marker
//! carries its funding key, written in the marker's transaction and in
//! every `Sent` bundle that re-appends it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use tokio::time::Instant;

use super::budget::Charge;
use super::call::{CallFunding, CallStep, FundingKey, SignatureCount};
use super::mode_b::{SelfSpend, SignedLock, StoredLock};
#[cfg(test)]
use super::stress_tests::{GrantKind, LogEvent, Mutation};
use super::table::{Effects, Entry, Inner, LeaseTable};
use super::{ArtifactId, DispatchScope, FlowOutcome, LeaseError, LeaseId, Origin};
use crate::platform::flows::{BudgetPurpose, DispatchResolution, DispatchResolved, DispatchState};
use crate::{EngineEvent, NoticeCode, WalletId};
use dw_appdb::dispatch::{
    CallRows, Resolution, SelfSpendConflict, SelfSpendProvenance, StoredFundingKey, StoredOutPoint,
};

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
    /// A funding step's marker with its funding key, in one transaction
    /// (§2a.3). Returns the standing marker's `seq`.
    fn insert_funding_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        key: &StoredFundingKey,
    ) -> Result<u64, String>;
    /// Repair's self-spend `txid` of a funding step (§6.5), recorded against
    /// the step's standing marker with the signed locks it spends an input
    /// of (`conflicts`); `Ok(false)` when none stands
    /// ([`LeaseTable::record_self_spend`]).
    fn insert_self_spend(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        txid: &[u8; 32],
        conflicts: &[SelfSpendConflict],
    ) -> Result<bool, String>;
    /// An asset lock `txid` (internal byte order) a call of funding `step`
    /// signed, with its `inputs` (§2a.6), recorded against the step's
    /// standing marker; `Ok(false)` when none stands
    /// ([`LeaseTable::record_signed_locks`]).
    fn insert_signed_lock(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        txid: &[u8; 32],
        inputs: &[StoredOutPoint],
    ) -> Result<bool, String>;
    /// Funding `step` was resolved by Repair: its ChainLocked self-spend
    /// `self_spend` takes an input of every lock signed under the step's
    /// standing marker, which the tombstone answers; `Ok(false)` when none
    /// stands ([`LeaseTable::write_tombstones`]).
    fn insert_tombstone(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        self_spend: &[u8; 32],
    ) -> Result<bool, String>;
    /// A funding call of `step` under the marker `marker` (its `seq`) is
    /// about to release its first signature (§2a.7, review R6-01),
    /// recorded against that marker at `at` (UNIX seconds) only while it
    /// stands (DP1-02 f1). Returns the row's time (the first write's);
    /// `Ok(None)` when `marker` does not stand
    /// ([`LeaseTable::record_release`]).
    fn insert_release(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        marker: u64,
        at: u64,
    ) -> Result<Option<u64>, String>;
    /// Appends the artifact's resolution (DEC-154): `NotSent` supersedes
    /// its markers so far. The engine writes `Sent` only through
    /// `resolve_sent`.
    fn resolve(&self, wallet: &[u8; 32], artifact: &[u8; 32], r: Resolution) -> Result<(), String>;
    /// A marked artifact's `Sent` row with its marker set, each funding
    /// step's key with its marker, in one transaction (DEC-163).
    fn resolve_sent(
        &self,
        wallet: &[u8; 32],
        artifact: &[u8; 32],
        steps: &[String],
        funding: &[(String, StoredFundingKey)],
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

    fn insert_funding_step(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        key: &StoredFundingKey,
    ) -> Result<u64, String> {
        self.insert_funding_step(wallet, step, artifact, key, crate::events::unix_now())
            .map_err(|e| e.to_string())
    }

    fn insert_self_spend(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        txid: &[u8; 32],
        conflicts: &[SelfSpendConflict],
    ) -> Result<bool, String> {
        self.insert_self_spend(
            wallet,
            step,
            artifact,
            txid,
            conflicts,
            crate::events::unix_now(),
        )
        .map_err(|e| e.to_string())
    }

    fn insert_signed_lock(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        txid: &[u8; 32],
        inputs: &[StoredOutPoint],
    ) -> Result<bool, String> {
        self.insert_signed_lock(
            wallet,
            step,
            artifact,
            txid,
            inputs,
            crate::events::unix_now(),
        )
        .map_err(|e| e.to_string())
    }

    fn insert_tombstone(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        self_spend: &[u8; 32],
    ) -> Result<bool, String> {
        self.insert_tombstone(
            wallet,
            step,
            artifact,
            self_spend,
            crate::events::unix_now(),
        )
        .map_err(|e| e.to_string())
    }

    fn insert_release(
        &self,
        wallet: &[u8; 32],
        step: &str,
        artifact: &[u8; 32],
        marker: u64,
        at: u64,
    ) -> Result<Option<u64>, String> {
        self.insert_release(wallet, step, artifact, marker, at)
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
        funding: &[(String, StoredFundingKey)],
    ) -> Result<(), String> {
        self.resolve_sent(wallet, artifact, steps, funding, crate::events::unix_now())
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

impl Mark {
    /// Its write is running.
    fn is_committing(self) -> bool {
        matches!(self, Self::Committing { .. })
    }
}

/// A Mode B call step's calls under its marker (§2a.3).
#[derive(Debug, Default)]
struct CallStepRun {
    /// Calls running under the marker.
    running: u32,
    /// A call under the marker may have sent, or an earlier marker of the
    /// step existed: no `NotSent` row may supersede it.
    possibly_out: bool,
    /// Every call ended definitely unsent while the marker's write ran:
    /// that write's J step begins the `NotSent` row.
    after_write: bool,
    /// The `NotSent` row is being written: a new call of the step waits.
    resolving: bool,
    /// The `NotSent` row failed, and may have landed: the next call of the
    /// step retries it before its own marker.
    owed: bool,
}

/// A Mode B call step's journal state (§2a.3), which `dispatch_status`
/// reads (§2a.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStepStatus {
    /// No marker: no call of the step ever started, or a definite
    /// resolution was compacted away with it.
    None,
    /// A marker stands, or may (a write that failed may have committed);
    /// `durable` once its write returned.
    Marked { durable: bool },
    /// A `NotSent` row superseded the marker in this process.
    NotSent,
    /// A call of the step returned Ok, or its `Sent` row was loaded.
    Sent,
}

/// What the fence holds of one call step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallStepInfo {
    pub status: CallStepStatus,
    /// A funding step's key, as its marker carries it.
    pub funding: Option<FundingKey>,
    /// The marker carries the durable-record guarantee (§2a.7, review
    /// OD-02): every lock a call under it signed was recorded durably
    /// before anything could send it.
    pub durable_record: bool,
    /// A call under the marker may have released a funding signature: its
    /// `funding_release` row (schema 7, §2a.7, review P2b r6 R6-01) is
    /// written before the vault signs anything. The row's `at`, UNIX
    /// seconds.
    pub released: Option<u64>,
    /// Repair's self-spends standing against the marker.
    pub self_spends: Vec<SelfSpend>,
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

/// A journal write's outcome, in its J step (§6.4): the first write that
/// succeeds after one failed runs H6's resume pass again (review Opus 11).
fn note_write(i: &mut Inner, fx: &mut Effects, ok: bool) {
    if !ok {
        i.fence.write_failed = true;
    } else if std::mem::take(&mut i.fence.write_failed) {
        fx.resume();
    }
}

/// A journal write's end, in its J step: one write fewer in flight, and
/// its outcome ([`note_write`]).
fn end_write(i: &mut Inner, fx: &mut Effects, ok: bool) {
    i.fence.writes -= 1;
    note_write(i, fx, ok);
}

/// A journal write, a panic in it taken as its failure.
fn guarded<T>(write: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(write))
        .unwrap_or_else(|_| Err("the journal write panicked".into()))
}

/// How often a signed lock's append is tried before the process stops
/// (§2a.7), the first retry after [`SIGNED_WRITE_BACKOFF`], each next one
/// after twice the last wait.
const SIGNED_WRITE_ATTEMPTS: u32 = 5;
const SIGNED_WRITE_BACKOFF: Duration = Duration::from_millis(50);

/// The fail-closed end of a signed lock's record (§2a.7, PA-2): the lock
/// could otherwise reach the wire with no durable record of its step. The
/// process stops inside the library's store call, before its send; after
/// a restart the step reads `NotSent`, its marker flagged and no lock of
/// it recorded or sent. Tests panic instead.
fn fail_closed(wallet: WalletId, why: &str) -> ! {
    tracing::error!(wallet_id = %wallet, why, "stopping: a signed asset lock could be sent without its durable record");
    // The log's writer may not flush before the abort.
    eprintln!("dw-engine: stopping, wallet {wallet}: {why}");
    #[cfg(test)]
    panic!("fail closed: {why}");
    #[cfg(not(test))]
    std::process::abort();
}

/// Appends the `NotSent` row of `id` (inline on a blocking thread; outside
/// J). `false` when there is no journal or the write failed or panicked,
/// which `what` names in the log.
fn write_not_sent(
    backend: Option<&Arc<dyn JournalBackend>>,
    wallet: &WalletId,
    id: &ArtifactId,
    what: &str,
) -> bool {
    backend.is_some_and(|backend| {
        guarded(|| blocking(|| backend.resolve(&wallet.0, &id.0, Resolution::NotSent)))
            .inspect_err(|e| tracing::warn!(artifact = %id, error = %e, "{what} failed"))
            .is_ok()
    })
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
    /// The last journal write failed (§6.4): the next one that succeeds
    /// runs H6's resume pass again.
    write_failed: bool,
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
    /// Mode B (§2a.3): each funding step's key, as its marker carries it.
    funding: HashMap<(WalletId, String), FundingKey>,
    /// Repair's self-spends standing against each funding step's marker.
    self_spends: HashMap<(WalletId, String), Vec<SelfSpend>>,
    /// The asset locks each funding step's calls signed (§2a.6), standing
    /// against its marker: on disk, or only here when the write failed.
    signed_locks: HashMap<(WalletId, String), Vec<SignedLock>>,
    /// The funding steps whose standing marker carries the durable-record
    /// guarantee (§2a.7, review OD-02): created by this build, or loaded
    /// with the schema 6 flag.
    durable_record: HashSet<(WalletId, String)>,
    /// The funding steps with a `funding_release` row against their
    /// standing marker (schema 7, §2a.7, review P2b r6 R6-01), each with
    /// the row's `at` (UNIX seconds, DP1-02's DEC-197): a call under it may
    /// have released a funding signature since then.
    released: HashMap<(WalletId, String), u64>,
    /// How often each funding step's marker was superseded in this process:
    /// a release write begun under one marker is not taken for the next
    /// one's ([`LeaseTable::record_release`]).
    supersedes: HashMap<(WalletId, String), u64>,
    /// Each funding step's standing marker's `seq` on disk, once its write
    /// in this process returned: a release names it, so a write that lands
    /// after the marker was superseded answers no other one (DP1-02 f1). A
    /// loaded marker needs none: the gate refuses a new call of a marked
    /// step, and a release without it is refused. Dropped with the marker
    /// when a `NotSent` row supersedes it; a `Sent` bundle that appends the
    /// step's marker again leaves the old one here, harmlessly (no call of
    /// a `Sent` step runs, and the journal refuses a release naming it).
    marker_seqs: HashMap<(WalletId, String), u64>,
    /// The funding steps Repair resolved, durably (DP1-02 PA-3, review
    /// OD-06): each with its ChainLocked self-spend, which takes an input
    /// of every lock the step's call signed. Its signed locks are dead for
    /// good, its marker stands, and no call of it runs again
    /// ([`FundingBlock::StepCancelled`](super::mode_b::FundingBlock::StepCancelled)).
    pub(super) tombstones: HashMap<(WalletId, String), [u8; 32]>,
    /// The funding steps Repair resolved in this process whose tombstone is
    /// not durable yet, each with its self-spend: the resolution holds from
    /// the first snapshot that showed it, whatever later ones show (a
    /// ChainLock is final), and its write is owed until it lands (review
    /// P2b r5 validator).
    pub(super) owed_tombstones: HashMap<(WalletId, String), [u8; 32]>,
    /// The tombstones whose write runs ([`LeaseTable::write_tombstones`]).
    pub(super) tombstoning: HashSet<(WalletId, String)>,
    /// The funding steps whose marker loaded with a funding type this
    /// build does not know: their claim is unknown (PA-1's view refuses).
    pub(super) unreadable_funding: HashSet<(WalletId, String)>,
    /// The funding calls that ended in this process after their signer
    /// released a signature (§2a.7): a lock of theirs that reaches the
    /// store unrecorded stops the process, and the resume pass sends none
    /// of theirs without its durable record.
    pub(super) ended_funding: Vec<CallFunding>,
    /// The call steps with a call running under their marker, or whose
    /// `NotSent` row is written, failed or owed.
    call_steps: HashMap<(WalletId, String), CallStepRun>,
    spend: HashMap<(WalletId, ArtifactId), SpendCharge>,
    /// Every artifact an attempt finished `Sent` for, under any wallet:
    /// identical bytes are one transaction. Only grows.
    sent: SentSet,
    attempts: HashMap<u64, Attempt>,
    next: u64,
    /// Tombstones whose `MaybeSent` (another wallet holds the same bytes)
    /// a host read: [`sweep_cross_wallet`] announces each when the hold
    /// ends (BL-76).
    held: HashSet<RowKey>,
}

/// The artifacts seen `Sent` (only grows), with the ones added since the
/// last [`sweep_cross_wallet`].
#[derive(Default)]
struct SentSet {
    set: HashSet<ArtifactId>,
    fresh: Vec<ArtifactId>,
}

impl SentSet {
    fn contains(&self, id: &ArtifactId) -> bool {
        self.set.contains(id)
    }

    /// Adds `id`; `true` when it is new, which the sweep announces to
    /// every wallet that holds a key of it.
    fn insert(&mut self, id: ArtifactId) -> bool {
        let new = self.set.insert(id);
        if new {
            self.fresh.push(id);
        }
        new
    }

    /// Adds `id` as the journal's load found it: nothing to announce.
    fn load(&mut self, id: ArtifactId) {
        self.set.insert(id);
    }
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
            // The same bytes may be out under another wallet (review O-1).
            Some(RowlessState::Revoked | RowlessState::NotSent)
                if self.held_elsewhere(wallet, id) =>
            {
                FlowOutcome::MaybeSent
            }
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

    /// Whether another wallet may have the bytes `id` out or about to go out
    /// (review O-1): its row-less entry is `Admitted` or `Resolving`, it
    /// holds a step marker of them, or their `Sent` row.
    pub(crate) fn held_elsewhere(&self, wallet: &WalletId, id: &ArtifactId) -> bool {
        self.rowless.iter().any(|((w, a), r)| {
            a == id
                && w != wallet
                && matches!(
                    r.state,
                    RowlessState::Admitted { .. } | RowlessState::Resolving { .. }
                )
        }) || self
            .steps
            .iter()
            .any(|((w, _), m)| w != wallet && m.contains_key(id))
            || self.evidence.keys().any(|(w, a)| a == id && w != wallet)
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

    /// `step`'s marker of `id` in memory.
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

    /// A Mode B call step's journal state (§2a.3).
    pub(crate) fn call_step_status(&self, wallet: &WalletId, step: &str) -> CallStepStatus {
        let id = ArtifactId::of_step(wallet, step);
        if self.sent.contains(&id) {
            return CallStepStatus::Sent;
        }
        match self.mark(wallet, step, &id) {
            Some(Mark::Durable) => CallStepStatus::Marked { durable: true },
            Some(_) => CallStepStatus::Marked { durable: false },
            None if self.superseded.contains_key(&(*wallet, id)) => CallStepStatus::NotSent,
            None => CallStepStatus::None,
        }
    }

    pub(super) fn call_step_info(&self, wallet: &WalletId, step: &str) -> CallStepInfo {
        let key = (*wallet, step.to_owned());
        CallStepInfo {
            status: self.call_step_status(wallet, step),
            funding: self.funding.get(&key).copied(),
            durable_record: self.durable_record.contains(&key),
            released: self.released.get(&key).copied(),
            self_spends: self.self_spends.get(&key).cloned().unwrap_or_default(),
        }
    }

    /// A funding step whose lock the wallet saw proved (review R1-01): it
    /// reads `Sent` for good, and its `Sent` row is owed (`Ambiguous`), to
    /// be begun after J ([`Effects::owe_sent`]) with the step's marker and
    /// funding key, as one DEC-163 bundle. Markers a `NotSent` row
    /// superseded come back with it; a step with no marker (found by its
    /// draft's own key) gets one, under `key`, the proving row's. A write
    /// that fails or panics leaves the row owed. `false` when the row is
    /// written, being written or owed already.
    pub(super) fn note_proved_funding(
        &mut self,
        wallet: WalletId,
        step: &str,
        key: FundingKey,
    ) -> bool {
        let id = ArtifactId::of_step(&wallet, step);
        // `sent` may hold it with no row begun (a call that returned `Sent`
        // while the journal was not ready, a removed wallet re-added):
        // owed then too (review P2b r1 F3).
        if !self.sent.insert(id) && self.evidence.contains_key(&(wallet, id)) {
            return false;
        }
        for (s, mark) in self.superseded.remove(&(wallet, id)).unwrap_or_default() {
            self.steps
                .entry((wallet, s))
                .or_default()
                .entry(id)
                .or_insert(mark);
        }
        let k = (wallet, step.to_owned());
        self.funding.entry(k.clone()).or_insert(key);
        self.steps
            .entry(k)
            .or_default()
            .entry(id)
            .or_insert(Mark::Ambiguous);
        // Durable or being written already, or owed now.
        self.evidence.entry((wallet, id)).or_insert(Mark::Ambiguous);
        true
    }

    /// The wallets with a funding step's marker.
    pub(super) fn funding_wallets(&self) -> BTreeSet<WalletId> {
        self.funding.keys().map(|(w, _)| *w).collect()
    }

    /// The wallet's funding steps with a standing marker (§2a.5).
    pub(super) fn funding_steps_of(&self, wallet: &WalletId) -> Vec<String> {
        self.funding
            .keys()
            .filter(|(w, _)| w == wallet)
            .map(|(_, s)| s.clone())
            .collect()
    }

    /// The asset locks the calls of funding `step` signed (§2a.6).
    pub(super) fn signed_locks(&self, wallet: &WalletId, step: &str) -> &[SignedLock] {
        self.signed_locks
            .get(&(*wallet, step.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    /// Whether lock `txid` of funding `step` has its durable record
    /// (§2a.7).
    pub(super) fn recorded_durably(&self, wallet: &WalletId, step: &str, txid: Txid) -> bool {
        self.signed_locks(wallet, step)
            .iter()
            .any(|l| l.txid == txid && l.durable)
    }

    /// Repair's self-spends standing against the wallet's funding markers.
    pub(super) fn self_spends_of(&self, wallet: &WalletId) -> Vec<[u8; 32]> {
        self.self_spends
            .iter()
            .filter(|((w, _), _)| w == wallet)
            .flat_map(|(_, spends)| spends.iter().map(|s| s.txid))
            .collect()
    }

    /// Whether every call of the step in this process ended definitely
    /// unsent and none runs, its `NotSent` row begun after the marker's
    /// write, being written, or owed after a failed write (the lock report,
    /// review P2b r1 F4).
    pub(super) fn call_ended_unsent(&self, key: &(WalletId, String)) -> bool {
        self.call_steps.get(key).is_some_and(|r| {
            r.running == 0 && !r.possibly_out && (r.after_write || r.resolving || r.owed)
        })
    }

    /// Whether a `NotSent` row of the call step is being written.
    pub(super) fn call_resolving(&self, key: &(WalletId, String)) -> bool {
        self.call_steps.get(key).is_some_and(|r| r.resolving)
    }

    /// Whether a call runs under the step's marker, or its `NotSent` row is
    /// being written.
    pub(super) fn call_busy(&self, key: &(WalletId, String)) -> bool {
        self.call_steps
            .get(key)
            .is_some_and(|r| r.running > 0 || r.resolving)
    }

    /// Whether the step's own marker stands in memory (any mark).
    fn call_marked(&self, wallet: &WalletId, step: &str) -> bool {
        self.mark(wallet, step, &ArtifactId::of_step(wallet, step))
            .is_some()
    }

    /// The funding keys a write of `id`'s `steps` carries: those of its
    /// funding steps, whose synthetic artifact `id` is (§2a.3).
    fn funding_of(
        &self,
        wallet: &WalletId,
        id: &ArtifactId,
        steps: &[String],
    ) -> Vec<(String, StoredFundingKey)> {
        steps
            .iter()
            .filter_map(|s| {
                let key = self.funding.get(&(*wallet, s.clone()))?;
                (ArtifactId::of_step(wallet, s) == *id).then(|| (s.clone(), key.stored()))
            })
            .collect()
    }

    /// Whether a call of `step` would carry another funding key (or none)
    /// than the marker of the step that exists: the journal keeps the
    /// key first recorded under a marker (§2a.3).
    pub(super) fn call_key_differs(&self, wallet: WalletId, step: &CallStep) -> bool {
        let key = (wallet, step.id().to_owned());
        self.call_marked(&wallet, step.id()) && self.funding.get(&key).copied() != step.funding()
    }

    /// A durable `NotSent` row superseded `id`'s markers of `wallet`: they
    /// leave `steps` for the stash a later `Sent` row restores them from,
    /// each as its own write left it.
    fn supersede(&mut self, wallet: WalletId, id: ArtifactId) {
        let mut gone = Vec::new();
        for ((w, s), m) in self.steps.iter_mut() {
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
        self.steps.retain(|_, m| !m.is_empty());
        // A call step's funding key, self-spends and signed locks stand
        // with its marker.
        for (s, _) in &gone {
            if id == ArtifactId::of_step(&wallet, s) {
                let key = (wallet, s.clone());
                // Only a call that released no signature writes a step's
                // `NotSent` row (`call.rs`), and a signed lock or a
                // self-spend needs one: none is ever dropped here (review
                // Opus O-9).
                debug_assert!(
                    !self.signed_locks.contains_key(&key)
                        && !self.self_spends.contains_key(&key)
                        && !self.tombstones.contains_key(&key),
                    "superseding funding step {s}, which has a signed lock or a self-spend"
                );
                self.funding.remove(&key);
                self.durable_record.remove(&key);
                self.released.remove(&key);
                self.marker_seqs.remove(&key);
                *self.supersedes.entry(key.clone()).or_default() += 1;
                self.self_spends.remove(&key);
                self.signed_locks.remove(&key);
            }
        }
        let stash = self.superseded.entry((wallet, id)).or_default();
        for (s, mark) in gone {
            match stash.iter_mut().find(|(t, _)| *t == s) {
                Some((_, m)) if mark == Mark::Durable => *m = Mark::Durable,
                Some(_) => {}
                None => stash.push((s, mark)),
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_read(&self, wallet: &WalletId, id: &ArtifactId) -> bool {
        self.held.contains(&(*wallet, *id))
    }

    fn live(inner_leases: &HashMap<LeaseId, Entry>, lease: &LeaseId, wallet: &WalletId) -> bool {
        inner_leases
            .get(lease)
            .is_some_and(|e| e.wallet == *wallet && e.state.live())
    }
}

impl LeaseTable {
    /// The `DispatchResolved` event of `wallet`'s `artifact` (a txid, a
    /// state-transition hash or a funding step id).
    pub(super) fn resolved_event(
        &self,
        wallet: &WalletId,
        artifact: impl ToString,
        resolution: DispatchResolution,
    ) -> EngineEvent {
        EngineEvent::DispatchResolved {
            network: self.network.clone(),
            resolved: DispatchResolved {
                wallet_id: wallet.to_string(),
                artifact: artifact.to_string(),
                resolution,
            },
        }
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
    fx.emit(t.resolved_event(wallet, id, r));
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

/// [`FenceState::held_elsewhere`], which the `ArtifactKeyed` mutation never
/// consults (one key per artifact has no other wallet's entry).
fn held_elsewhere(i: &Inner, wallet: &WalletId, id: &ArtifactId) -> bool {
    #[cfg(test)]
    if i.mutation == Some(Mutation::ArtifactKeyed) {
        return false;
    }
    i.fence.held_elsewhere(wallet, id)
}

/// BL-76, §5.5 "Per wallet, one transaction": announces what another
/// wallet's change resolved for this one, at the end of every J step.
/// - An artifact newly seen `Sent` (`sent` is keyed by artifact): every
///   wallet with an entry of it reads `Sent` now, and gets
///   `DispatchResolved(Sent)` unless this J step already emits it (the
///   sighting's own wallet does).
/// - A tombstone whose `MaybeSent` a host read while another wallet held
///   the same bytes ([`FenceState::note_hold_read`]): once no other wallet
///   holds them, it reads `NotSent` again, and its wallet gets
///   `DispatchResolved(NotSent)`. A host that never read the `MaybeSent`
///   still holds the tombstone's own `NotSent`, which stays true.
pub(super) fn sweep_cross_wallet(t: &LeaseTable, i: &mut Inner, fx: &mut Effects) {
    if i.fence.sent.fresh.is_empty() && i.fence.held.is_empty() {
        return;
    }
    let fresh = std::mem::take(&mut i.fence.sent.fresh);
    if !fresh.is_empty() {
        let f = &i.fence;
        let fresh: HashSet<ArtifactId> = fresh.into_iter().collect();
        // A call step's own synthetic artifact announces under the step's
        // id (`call_step_end`, `rederive`), not here.
        let marked = f.steps.iter().flat_map(|((w, s), m)| {
            let own = ArtifactId::of_step(w, s);
            m.keys().filter(move |a| **a != own).map(move |a| (*w, *a))
        });
        let keys: BTreeSet<RowKey> = f
            .rowless
            .keys()
            .chain(f.entries.keys())
            .copied()
            .chain(marked)
            .filter(|(_, a)| fresh.contains(a))
            .collect();
        for (wallet, id) in keys {
            resolved_once(t, fx, &wallet, &id, DispatchResolution::Sent);
        }
        i.fence.held.retain(|(_, a)| !fresh.contains(a));
    }
    let ended: Vec<RowKey> = i
        .fence
        .held
        .iter()
        .filter(|(w, a)| !tombstone_held(i, w, a))
        .copied()
        .collect();
    for (wallet, id) in ended {
        i.fence.held.remove(&(wallet, id));
        // A key forgotten, or admitted again by its own wallet, has no
        // hold to end.
        if is_tombstone(i, &wallet, &id) && !i.fence.sent.contains(&id) {
            resolved_once(t, fx, &wallet, &id, DispatchResolution::NotSent);
        }
    }
}

/// Whether `wallet`'s entry of `id` is a row-less tombstone (as
/// `dispatch_status` reads it: no registered entry first).
fn is_tombstone(i: &Inner, wallet: &WalletId, id: &ArtifactId) -> bool {
    !i.fence.entries.contains_key(&(*wallet, *id))
        && i.fence
            .rowless
            .get(&row_key(i, *wallet, *id))
            .is_some_and(|r| matches!(r.state, RowlessState::Revoked | RowlessState::NotSent))
}

/// Whether `wallet`'s tombstone of `id` reads `MaybeSent` because another
/// wallet holds the same bytes (§5.5).
fn tombstone_held(i: &Inner, wallet: &WalletId, id: &ArtifactId) -> bool {
    !i.fence.sent.contains(id) && is_tombstone(i, wallet, id) && held_elsewhere(i, wallet, id)
}

/// A host reads `wallet`'s status of `id` now: a held tombstone's
/// `MaybeSent` is announced again when the hold ends (BL-76).
pub(crate) fn note_hold_read(i: &mut Inner, wallet: &WalletId, id: &ArtifactId) {
    if tombstone_held(i, wallet, id) {
        i.fence.held.insert((*wallet, *id));
    }
}

/// [`resolved`], unless this J step already emits the same event.
fn resolved_once(
    t: &LeaseTable,
    fx: &mut Effects,
    wallet: &WalletId,
    id: &ArtifactId,
    r: DispatchResolution,
) {
    fx.emit_once(t.resolved_event(wallet, id, r));
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
    // The removed wallet's tracked rows and derived statuses (§2a.5).
    i.mode_b.forget(&wallet);
    let f = &mut i.fence;
    f.funding
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.self_spends
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.signed_locks
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.durable_record
        .retain(|(w, s)| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.released
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.supersedes
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.marker_seqs
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.tombstones
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.owed_tombstones
        .retain(|(w, s), _| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.unreadable_funding
        .retain(|(w, s)| !forgotten(w, &ArtifactId::of_step(w, s)));
    f.ended_funding
        .retain(|c| !forgotten(&c.wallet, &ArtifactId::of_step(&c.wallet, &c.step)));
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

/// Why a Mode B call step does not go on yet ([`LeaseTable::call_step_gate`]).
pub(super) enum CallGate {
    /// Its `NotSent` row is being written.
    Wait,
    /// Its `NotSent` row failed: retry it first.
    Retry,
}

/// In a step write's J step: whether the call step's `NotSent` row, held
/// back while the marker's write ran (`call_step_end`), begins now.
fn call_settles_after_write(
    f: &mut FenceState,
    wallet: WalletId,
    step: &str,
    id: ArtifactId,
) -> bool {
    let key = (wallet, step.to_owned());
    let writing = f.mark(&wallet, step, &id).is_some_and(Mark::is_committing);
    match f.call_steps.get_mut(&key) {
        Some(run) if run.after_write && !writing && run.running == 0 && !run.possibly_out => {
            run.after_write = false;
            run.resolving = true;
            true
        }
        _ => false,
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

    /// Loads the journal's rows (§6.5) with no Mode B rows.
    #[cfg(test)]
    pub(crate) fn load_journal(
        &self,
        backend: Option<Arc<dyn JournalBackend>>,
        rows: Vec<dw_appdb::dispatch::DispatchRow>,
        steps: Vec<dw_appdb::dispatch::StepRow>,
    ) {
        self.load_journal_with(backend, rows, steps, CallRows::default());
    }

    /// Loads the journal's rows (§6.5): at load every origin is dead, so an
    /// `Unsent` row is refused on its first `admit`. Of `calls`, only the
    /// rows that stand ([`dw_appdb::dispatch::standing_calls`]); a funding
    /// key of a type this build does not know is skipped, and its marker
    /// still reads possibly sent.
    pub(crate) fn load_journal_with(
        &self,
        backend: Option<Arc<dyn JournalBackend>>,
        rows: Vec<dw_appdb::dispatch::DispatchRow>,
        steps: Vec<dw_appdb::dispatch::StepRow>,
        calls: CallRows,
    ) {
        use dw_appdb::dispatch::DiskState;
        let ready = backend.is_some();
        self.journal.set(backend);
        // Outside J: a subscriber may do I/O (H1).
        let mut unreadable = Vec::new();
        let funding: Vec<_> = calls
            .funding
            .into_iter()
            .filter_map(|f| match FundingKey::from_stored(f.key) {
                Some(key) => Some(((WalletId(f.wallet), f.step_id), (key, f.durable_record))),
                None => {
                    tracing::warn!(step = %f.step_id, code = f.key.funding_type, "an unknown funding type in the dispatch journal");
                    unreadable.push((WalletId(f.wallet), f.step_id));
                    None
                }
            })
            .collect();
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
                    i.fence.sent.load(ArtifactId(s.artifact));
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
            // In `seq` order: the latest standing key of a step wins, and
            // its flag (§2a.7).
            i.fence.unreadable_funding.extend(unreadable);
            for (step, (key, durable_record)) in funding {
                if durable_record {
                    i.fence.durable_record.insert(step.clone());
                } else {
                    i.fence.durable_record.remove(&step);
                }
                i.fence.funding.insert(step, key);
            }
            for s in calls.self_spends {
                let conflicts = match s.provenance {
                    SelfSpendProvenance::Legacy => None,
                    SelfSpendProvenance::Conflicts(c) => Some(
                        c.iter()
                            .map(|c| {
                                (
                                    Txid::from_byte_array(c.lock),
                                    OutPoint::new(
                                        Txid::from_byte_array(c.input.txid),
                                        c.input.vout,
                                    ),
                                )
                            })
                            .collect(),
                    ),
                };
                i.fence
                    .self_spends
                    .entry((WalletId(s.wallet), s.step_id))
                    .or_default()
                    .push(SelfSpend {
                        txid: s.txid,
                        conflicts,
                    });
            }
            for l in calls.signed_locks {
                i.fence
                    .signed_locks
                    .entry((WalletId(l.wallet), l.step_id))
                    .or_default()
                    .push(SignedLock {
                        txid: Txid::from_byte_array(l.txid),
                        inputs: l
                            .inputs
                            .iter()
                            .map(|o| OutPoint::new(Txid::from_byte_array(o.txid), o.vout))
                            .collect(),
                        durable: true,
                    });
            }
            for t in calls.tombstones {
                (i.fence.tombstones).insert((WalletId(t.wallet), t.step_id), t.self_spend);
            }
            for r in calls.releases {
                // One row per standing marker: a step has one at most.
                (i.fence.released).insert((WalletId(r.wallet), r.step_id), r.at);
            }
            i.fence.journal = JournalState::Ready;
            fx.notify();
        });
    }

    /// Writes each owed tombstone of `wallets` (DP1-02 PA-3, review
    /// OD-06): a funding step whose ChainLocked self-spend a snapshot
    /// showed resolving it ([`resolving_spend`](super::mode_b::resolving_spend)),
    /// held in `owed_tombstones` until its write lands. A failed write is
    /// begun again by the next derivation of the wallet; one that finds no
    /// marker standing on disk is dropped (the step then reads from its
    /// evidence again). Each on a blocking thread, outside J.
    pub(super) fn write_tombstones(self: &Arc<Self>, wallets: &BTreeSet<WalletId>) {
        let Some(backend) = self.journal.get() else {
            return;
        };
        let jobs: Vec<(WalletId, String, [u8; 32])> = self.with_j(|i, _| {
            let mut jobs = Vec::new();
            if i.fence.journal != JournalState::Ready {
                return jobs;
            }
            for wallet in wallets {
                let owed: Vec<((WalletId, String), [u8; 32])> = (i.fence.owed_tombstones.iter())
                    .filter(|(k, _)| k.0 == *wallet && !i.fence.tombstoning.contains(*k))
                    .map(|(k, s)| (k.clone(), *s))
                    .collect();
                for (key, spend) in owed {
                    i.fence.tombstoning.insert(key.clone());
                    i.fence.writes += 1;
                    jobs.push((key.0, key.1, spend));
                }
            }
            jobs
        });
        for (wallet, step, spend) in jobs {
            let (table, backend) = (Arc::clone(self), Arc::clone(&backend));
            self.rt.spawn_blocking(move || {
                let id = ArtifactId::of_step(&wallet, &step);
                let written = guarded(|| backend.insert_tombstone(&wallet.0, &step, &id.0, &spend));
                if let Err(e) = &written {
                    tracing::warn!(step, error = %e, "recording a funding step's tombstone failed");
                }
                table.with_j(|i, fx| {
                    fx.notify();
                    end_write(i, fx, written.is_ok());
                    let key = (wallet, step);
                    i.fence.tombstoning.remove(&key);
                    match written {
                        Ok(true) => {
                            i.fence.owed_tombstones.remove(&key);
                            i.fence.tombstones.insert(key, spend);
                        }
                        Ok(false) => {
                            tracing::warn!(
                                step = key.1,
                                "no marker stands on disk for a funding step's tombstone"
                            );
                            i.fence.owed_tombstones.remove(&key);
                        }
                        Err(_) => {}
                    }
                });
            });
        }
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
                let written =
                    guarded(|| backend.register(&wallet.0, &txid.0, &lease, &process, &payload));
                table.with_j(|i, fx| {
                    note_write(i, fx, written.is_ok());
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
                        if (recheck
                            && !matches!(
                                owed(i, &req.wallet, &req.artifact, step),
                                Obligation::Met
                            ))
                            || held_elsewhere(i, &req.wallet, &req.artifact)
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
        // A funding step's marker carries its key, alone or in a bundle
        // (§2a.3, DEC-163).
        let funding = match &target {
            Target::Record => Vec::new(),
            Target::Step(s) => i
                .fence
                .funding_of(&wallet, &artifact, std::slice::from_ref(s)),
            Target::Sent(steps) => i.fence.funding_of(&wallet, &artifact, steps),
        };
        let table = Arc::clone(self);
        self.rt.spawn_blocking(move || {
            let (w, a) = (&wallet.0, &artifact.0);
            // A funding marker's `seq`, which its calls' releases name.
            let mut seq = None;
            let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &target {
                Target::Record => matches!(backend.mark_dispatching(w, a), Ok(true)),
                Target::Step(s) => match funding.first() {
                    Some((_, key)) => backend
                        .insert_funding_step(w, s, a, key)
                        .map(|n| seq = Some(n))
                        .is_ok(),
                    None => backend.insert_step(w, s, a).is_ok(),
                },
                #[cfg(test)]
                Target::Sent(steps) if split => {
                    // The mutation: the bundle as separate statements.
                    steps.iter().all(|s| backend.insert_step(w, s, a).is_ok())
                        && backend.resolve(w, a, Resolution::Sent).is_ok()
                }
                Target::Sent(steps) => backend.resolve_sent(w, a, steps, &funding).is_ok(),
            }))
            .unwrap_or(false);
            let (settling, call) = table.with_j(|i, fx| {
                fx.notify();
                end_write(i, fx, ok);
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
                    if let (Target::Step(s), Some(seq)) = (&target, seq) {
                        i.fence.marker_seqs.insert((wallet, s.clone()), seq);
                    }
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
                // A call step's `NotSent` row this write held back
                // (`call_step_end`) begins now.
                let call = match &target {
                    Target::Step(s) => call_settles_after_write(&mut i.fence, wallet, s, artifact)
                        .then(|| s.clone()),
                    _ => None,
                };
                let settling = if idle {
                    fx.notify();
                    start_settle(&table, i, fx, row_key(i, wallet, artifact))
                } else {
                    Settling::Now
                };
                (settling, call)
            });
            if let Settling::Durably = settling {
                table.resolve_not_sent(wallet, artifact);
            }
            if let Some(step) = call {
                table.resolve_call_not_sent(wallet, &step);
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

    /// Begins `wallets`' owed `Sent` rows ([`Effects::owe_sent`]), reading
    /// the journal outside J. A journal not ready leaves them owed: the
    /// next write that lands, or the journal's close, begins them.
    pub(super) fn write_owed_sent(self: &Arc<Self>, wallets: &BTreeSet<WalletId>) {
        let Some(backend) = self.journal.get() else {
            return;
        };
        self.with_j(|i, _| {
            if i.fence.journal == JournalState::Ready {
                for w in wallets {
                    self.retry_owed_sent(i, &backend, Some(*w));
                }
            }
        });
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
        // while another wallet may have the same bytes out or about to go
        // out, a copy for this wallet waits, so no entry of this wallet runs
        // alongside it and settles definitely unsent.
        let rkey = row_key(i, wallet, id);
        if held_elsewhere(i, &wallet, &id) {
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
        let written =
            write && write_not_sent(backend.as_ref(), &wallet, &id, "resolving a step marker");
        self.with_j(|i, fx| {
            fx.notify();
            if write {
                end_write(i, fx, written);
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
            i.fence.supersede(wallet, id);
            settle_unsent(self, i, fx, key);
            true
        })
    }

    /// The gate of a Mode B call step's J step (§2a.3): `None` when the call
    /// may go on to its lease checks. The journal must be ready, since the
    /// marker is written before the call; a running `NotSent` row of the
    /// step is waited for, and a failed one is retried first (once per
    /// call, `retry`).
    pub(super) fn call_step_gate(
        &self,
        i: &mut Inner,
        key: &(WalletId, String),
        ready: bool,
        retry: bool,
    ) -> Result<Option<CallGate>, LeaseError> {
        if !ready || i.fence.journal != JournalState::Ready {
            return Err(LeaseError::JournalUnavailable);
        }
        Ok(match i.fence.call_steps.get_mut(key) {
            Some(run) if run.resolving => Some(CallGate::Wait),
            Some(run) if run.owed && retry => {
                run.owed = false;
                run.resolving = true;
                Some(CallGate::Retry)
            }
            _ => None,
        })
    }

    /// In the J step that took the call's permit (§2a.3): the step's marker
    /// becomes `Committing` and its durable write is spawned, unless it is
    /// durable already. Returns whether an earlier marker of the step
    /// exists (the call's `prior_marker`), and the write to await before
    /// the call starts.
    pub(super) fn call_step_mark(
        self: &Arc<Self>,
        i: &mut Inner,
        backend: Arc<dyn JournalBackend>,
        wallet: WalletId,
        step: &CallStep,
    ) -> (bool, Option<tokio::task::JoinHandle<bool>>) {
        let id = ArtifactId::of_step(&wallet, step.id());
        let key = (wallet, step.id().to_owned());
        let mark = i.fence.mark(&wallet, step.id(), &id);
        let prior = mark.is_some();
        // A new marker records the call's key; under an existing one the
        // key is the same (refused otherwise, `call_key_differs`).
        if !prior && let Some(funding) = step.funding() {
            i.fence.funding.insert(key.clone(), funding);
            i.fence.durable_record.insert(key.clone());
        }
        let run = i.fence.call_steps.entry(key.clone()).or_default();
        run.running += 1;
        run.possibly_out |= prior;
        // This call runs under the marker now: a `NotSent` row an earlier
        // call held back or left owed no longer applies.
        run.owed = false;
        run.after_write = false;
        if mark == Some(Mark::Durable) {
            // Only a resumable step's call runs under a marker that stood:
            // the funding gate refuses a marked funding step, so a funding
            // call always writes its marker and learns its `seq`, which its
            // release names (DP1-02 f1).
            debug_assert!(
                step.funding().is_none(),
                "a funding call over a standing marker"
            );
            return (true, None);
        }
        i.fence.next += 1;
        let owner = i.fence.next;
        i.fence
            .steps
            .entry(key)
            .or_default()
            .insert(id, Mark::Committing { owner });
        let target = Target::Step(step.id().to_owned());
        let write = self.spawn_write(i, backend, wallet, id, target, owner);
        (prior, Some(write))
    }

    /// A call under the step's marker ended (in its permit's J step). Ok
    /// begins the step's `Sent` row, the DEC-163 bundle. Once no call runs
    /// under the marker and none may have sent, a `NotSent` row supersedes
    /// it (written with the marker, `FULL`); while the marker's own write
    /// runs, that write's J step begins it. Returns whether the caller
    /// writes the row now, outside J ([`Self::resolve_call_not_sent`]).
    pub(super) fn call_step_end(
        self: &Arc<Self>,
        i: &mut Inner,
        fx: &mut Effects,
        backend: Option<Arc<dyn JournalBackend>>,
        wallet: WalletId,
        step: &CallStep,
        outcome: FlowOutcome,
    ) -> bool {
        let id = ArtifactId::of_step(&wallet, step.id());
        let key = (wallet, step.id().to_owned());
        if outcome == FlowOutcome::Sent {
            // A resumable step's call that returned Ok holds the proved
            // execution (H12, BL-76): `Sent`, announced under the step's
            // id, as the facade reads it. A funding step's status is
            // derived, and `rederive` announces it.
            if i.fence.sent.insert(id) && step.funding().is_none() {
                fx.emit(self.resolved_event(&wallet, step.id(), DispatchResolution::Sent));
            }
            self.begin_sent_row(i, backend, wallet, id);
        }
        let writing = i
            .fence
            .mark(&wallet, step.id(), &id)
            .is_some_and(Mark::is_committing);
        let Some(run) = i.fence.call_steps.get_mut(&key) else {
            return false;
        };
        run.running = run.running.saturating_sub(1);
        if outcome != FlowOutcome::Cancelled {
            run.possibly_out = true;
        }
        if run.running > 0 {
            return false;
        }
        if run.possibly_out {
            i.fence.call_steps.remove(&key);
            return false;
        }
        if writing {
            run.after_write = true;
            return false;
        }
        run.resolving = true;
        true
    }

    /// Retries a call step's failed `NotSent` row ([`CallGate::Retry`]).
    pub(super) async fn retry_call_not_sent(self: &Arc<Self>, wallet: WalletId, step: String) {
        let table = Arc::clone(self);
        let _ = self
            .rt
            .spawn_blocking(move || table.resolve_call_not_sent(wallet, &step))
            .await;
    }

    /// Writes the `NotSent` row of a call step whose every call ended
    /// definitely unsent (§2a.3, §7.6): it supersedes the step's marker,
    /// and a new call of the step is a fresh one. A write that fails or
    /// panics may have landed: the marker stays, `Ambiguous`, so the step
    /// reads MaybeSent until a retry lands (the step's next call). Outside
    /// J; blocks on the journal (inline on a blocking thread).
    pub(super) fn resolve_call_not_sent(&self, wallet: WalletId, step: &str) -> bool {
        let backend = self.journal.get();
        let id = ArtifactId::of_step(&wallet, step);
        let key = (wallet, step.to_owned());
        let (resolving, write) = self.with_j(|i, _| {
            let resolving = i.fence.call_resolving(&key);
            let write = resolving && i.fence.journal == JournalState::Ready;
            if write {
                i.fence.writes += 1;
            }
            (resolving, write)
        });
        if !resolving {
            return false;
        }
        let written = write
            && write_not_sent(
                backend.as_ref(),
                &wallet,
                &id,
                "resolving a call step's marker",
            );
        self.with_j(|i, fx| {
            fx.notify();
            if write {
                end_write(i, fx, written);
            }
            let Some(run) = i.fence.call_steps.get_mut(&key) else {
                return false;
            };
            run.resolving = false;
            if !written {
                run.owed = true;
                if let Some(m) = i.fence.steps.get_mut(&key).and_then(|m| m.get_mut(&id)) {
                    *m = Mark::Ambiguous;
                }
                return false;
            }
            i.fence.call_steps.remove(&key);
            i.fence.supersede(wallet, id);
            // A definite resolution (§2a.5): the funding may read NotSent.
            super::mode_b::rederive(self, i, fx, wallet);
            true
        })
    }

    /// A Mode B call step's journal state, its funding key and Repair's
    /// self-spends (§2a.3, §2a.5).
    pub fn call_step(&self, wallet: WalletId, step: &str) -> CallStepInfo {
        self.with_j(|i, _| i.fence.call_step_info(&wallet, step))
    }

    /// Records that a funding call of `step` is about to release its first
    /// signature (schema 7, §2a.7, review P2b r6 R6-01): one append to the
    /// journal's `funding_release` table against the step's standing
    /// marker, named by its `seq` as J knew it when the record began, so a
    /// write that lands after that marker was superseded and the step
    /// marked again answers neither (DP1-02 f1); in memory, with the row's
    /// time, once it landed. The call's funding signer awaits
    /// it before the vault signs anything
    /// ([`CountingFundingSigner`](crate::platform::CountingFundingSigner)),
    /// so every lock that can reach a broadcast, by whatever path, has its
    /// step's release row on disk first, and the step never reads `NotSent`
    /// for want of a signed-lock record once a signature may be out. An
    /// error (no journal ready, no marker `seq` known in this process, the
    /// marker no longer standing on disk, a write that keeps failing or
    /// panics, the marker superseded meanwhile) refuses the signature. A failing write is retried ([`SIGNED_WRITE_ATTEMPTS`]).
    /// Idempotent per marker. Outside J.
    pub(crate) async fn record_release(
        self: &Arc<Self>,
        wallet: WalletId,
        step: &str,
    ) -> Result<(), String> {
        let backend = (self.journal.get()).ok_or("no dispatch journal")?;
        let key = (wallet, step.to_owned());
        let begun = self.with_j(|i, _| {
            if i.fence.journal != JournalState::Ready {
                return Err("the dispatch journal is not ready".to_owned());
            }
            if !i.fence.call_marked(&wallet, step) {
                return Err("the funding step's marker no longer stands".to_owned());
            }
            if i.fence.released.contains_key(&key) {
                return Ok(None);
            }
            let Some(&marker) = i.fence.marker_seqs.get(&key) else {
                return Err("the funding step's marker is not durable".to_owned());
            };
            i.fence.writes += 1;
            Ok(Some((
                i.fence.supersedes.get(&key).copied().unwrap_or(0),
                marker,
            )))
        })?;
        let Some((generation, marker)) = begun else {
            return Ok(());
        };
        // One time for every attempt, so a retry that finds an earlier
        // attempt's row (idempotent per marker) leaves J's time the row's.
        let at = crate::events::unix_now();
        // The write and the J step that ends it run on the blocking task,
        // so `writes` comes down even if this future is dropped.
        let table = Arc::clone(self);
        self.rt
            .spawn_blocking(move || {
                let id = ArtifactId::of_step(&wallet, &key.1);
                let mut delay = SIGNED_WRITE_BACKOFF;
                let mut written = Err(String::new());
                for attempt in 1..=SIGNED_WRITE_ATTEMPTS {
                    written =
                        guarded(|| backend.insert_release(&wallet.0, &key.1, &id.0, marker, at));
                    match &written {
                        Err(e) => {
                            tracing::warn!(step = key.1, attempt, error = %e, "recording a funding release failed");
                        }
                        Ok(_) => break,
                    }
                    if attempt < SIGNED_WRITE_ATTEMPTS {
                        std::thread::sleep(delay);
                        delay *= 2;
                    }
                }
                table.with_j(|i, fx| {
                    fx.notify();
                    end_write(i, fx, written.is_ok());
                    match written {
                        Err(e) => Err(e),
                        Ok(None) => Err("the funding step's marker no longer stands on disk".to_owned()),
                        // A `NotSent` row cannot supersede the marker while
                        // the call signs (`CallPermit::end`), unless this
                        // signing was dropped meanwhile: the row is then the
                        // superseded marker's (the journal wrote it only
                        // while that marker stood), and the next one's call
                        // writes its own.
                        Ok(Some(_))
                            if !i.fence.call_marked(&wallet, &key.1)
                                || i.fence.supersedes.get(&key).copied().unwrap_or(0)
                                    != generation =>
                        {
                            Err("the funding step's marker was superseded".to_owned())
                        }
                        // A permit that ended during this write read its
                        // step `NotSent` until now (no release, no call
                        // running), and `MaybeSent` from here: its
                        // resolution event may flap NotSent → MaybeSent
                        // (review Opus closure C-4). Safe: the signer
                        // re-checks the end after this write and signs
                        // nothing.
                        // The row's time: an earlier attempt's or another
                        // signer's write under the marker may have landed
                        // first.
                        Ok(Some(at)) => {
                            i.fence.released.insert(key, at);
                            Ok(())
                        }
                    }
                })
            })
            .await
            .unwrap_or_else(|_| Err("the funding release write was cancelled".to_owned()))
    }

    /// [`Self::record_release`] inline (tests): what a funding signer's
    /// first signature writes, for a test that releases one without it.
    #[cfg(test)]
    pub(crate) fn record_release_now(&self, wallet: WalletId, step: &str) -> Result<(), String> {
        self.record_release_at(wallet, step, crate::events::unix_now())
    }

    /// Sets J's time of `step`'s release to `at`, in memory only, as a
    /// `funding_release` row written at `at` would load (tests).
    #[cfg(test)]
    pub(crate) fn set_release_at(&self, wallet: WalletId, step: &str, at: u64) {
        self.with_j(|i, fx| {
            fx.notify();
            i.fence.released.insert((wallet, step.to_owned()), at);
        });
    }

    /// [`Self::record_release_now`] at `at` (UNIX seconds).
    #[cfg(test)]
    pub(crate) fn record_release_at(
        &self,
        wallet: WalletId,
        step: &str,
        at: u64,
    ) -> Result<(), String> {
        let key = (wallet, step.to_owned());
        let state = self.with_j(|i, _| {
            let ready =
                i.fence.journal == JournalState::Ready && i.fence.call_marked(&wallet, step);
            (ready, i.fence.released.contains_key(&key))
        });
        match state {
            (_, true) => return Ok(()),
            (false, false) => return Err("no journal ready, or no marker".into()),
            (true, false) => {}
        }
        let backend = (self.journal.get()).ok_or("no dispatch journal")?;
        let id = ArtifactId::of_step(&wallet, step);
        let marker = (self.with_j(|i, _| i.fence.marker_seqs.get(&key).copied()))
            .ok_or("the funding step's marker is not durable")?;
        let Some(at) = backend.insert_release(&wallet.0, step, &id.0, marker, at)? else {
            return Err("the funding step's marker no longer stands on disk".into());
        };
        self.with_j(|i, fx| {
            fx.notify();
            i.fence.released.insert(key, at);
        });
        Ok(())
    }

    /// Records Repair's self-spend `txid` (display order) of the stuck
    /// funding `step` (§6.5, §2a.5), which spends `inputs`, before the
    /// self-spend is broadcast: one append to the journal's `self_spend`
    /// table, against the step's standing marker, with the input it spends
    /// of each lock a call of the step signed (its provenance, review P2b
    /// r3 R3-01), then the step's in-memory self-spends. Never a `NotSent`
    /// row: the step reads `NotSent` only once the self-spend is
    /// ChainLocked and its record shows the conflict
    /// ([`super::mode_b::derive_funding`], [`SelfSpend`]), and after a
    /// restart the row and the wallet's ChainLock say so again.
    ///
    /// Refused unless the step is stuck now
    /// ([`super::mode_b::stuck_funding`]); refused with `MissesSignedLock`
    /// when `inputs` miss every input of a signed lock; with `NoMarker` when no
    /// marker of the step stands on disk (a `NotSent` row superseded it
    /// meanwhile). A write that fails or panics may have landed: the caller
    /// must then not broadcast, and a row of a self-spend never sent is
    /// harmless (it is never ChainLocked). Outside J.
    pub(crate) async fn record_self_spend(
        self: &Arc<Self>,
        wallet: WalletId,
        step: &str,
        txid: [u8; 32],
        inputs: &[OutPoint],
    ) -> Result<(), super::mode_b::StuckFundingRefusal> {
        use super::mode_b::StuckFundingRefusal as R;
        let backend = self.journal.get().ok_or(R::JournalUnavailable)?;
        let conflicts = self.with_j(|i, _| {
            if i.fence.journal != JournalState::Ready {
                return Err(R::JournalUnavailable);
            }
            super::mode_b::stuck_funding(i, wallet, step)?;
            let conflicts = super::mode_b::self_spend_conflicts(i, wallet, step, inputs)?;
            i.fence.writes += 1;
            Ok(conflicts)
        })?;
        let stored: Vec<SelfSpendConflict> = conflicts
            .iter()
            .map(|(lock, input)| SelfSpendConflict {
                lock: lock.to_byte_array(),
                input: StoredOutPoint {
                    txid: input.txid.to_byte_array(),
                    vout: input.vout,
                },
            })
            .collect();
        // The write and the J step that ends it run on the blocking task,
        // so `writes` comes down even if this future is dropped.
        let table = Arc::clone(self);
        let step = step.to_owned();
        self.rt
            .spawn_blocking(move || {
                let id = ArtifactId::of_step(&wallet, &step);
                let written =
                    guarded(|| backend.insert_self_spend(&wallet.0, &step, &id.0, &txid, &stored))
                        .inspect_err(
                            |e| tracing::warn!(step, error = %e, "recording a self-spend failed"),
                        )
                        .ok();
                table.with_j(|i, fx| {
                    fx.notify();
                    end_write(i, fx, written.is_some());
                    match written {
                        None => Err(R::JournalUnavailable),
                        Some(false) => Err(R::NoMarker),
                        Some(true) => {
                            // A `NotSent` row that superseded the marker
                            // since dropped the row with it, on disk as in
                            // memory.
                            if i.fence.call_marked(&wallet, &step) {
                                let spends = i.fence.self_spends.entry((wallet, step));
                                let spends = spends.or_default();
                                if !spends.iter().any(|s| s.txid == txid) {
                                    spends.push(SelfSpend {
                                        txid,
                                        conflicts: Some(conflicts),
                                    });
                                }
                            }
                            Ok(())
                        }
                    }
                })
            })
            .await
            .unwrap_or(Err(R::JournalUnavailable))
    }

    /// The changeset tap is about to store asset locks of `wallet`
    /// (`locks`), before the persister takes them (§2a.6, §2a.7, reviews
    /// P2b r2 R2-01, OD-01). A lock a running funding call signed (every
    /// input carries a valid signature over a sighash the call's funding
    /// signer signed: [`SignatureCount::signed_all`](super::call::SignatureCount::signed_all))
    /// is its step's own, exactly: it is recorded with its inputs against
    /// the step's standing marker, first here, pending (from then on the
    /// step cannot read `NotSent` for want of a record), then by one append
    /// to the journal's `signed_lock` table. This returns only once that
    /// append landed: the library sends a `Built` lock only after this
    /// store returns, and the resume pass sends none before its record is
    /// durable ([`unrecorded_signer`](super::mode_b::unrecorded_signer)), so no lock a call
    /// signed that reaches the store is sent without its durable record
    /// (§2a.7). One the library sends without storing it (an empty tracking
    /// changeset, review P2b r6 R6-01) is covered by its call's release row
    /// instead ([`Self::record_release`]): its step reads possibly sent.
    /// A failing append is retried ([`SIGNED_WRITE_ATTEMPTS`]). One that
    /// keeps failing, a marker gone, a journal not ready, a lock carrying
    /// only some of a call's signatures, a call that signed anything besides
    /// the lock (a second lock, review P2b r6 R6-02), an asset-lock
    /// changeset with no lock while a running call's released signatures
    /// have no recorded lock (review P2b r7), a lock of a call that
    /// already ended reaching the store unrecorded, or a `Built` lock whose
    /// signatures cannot be read
    /// stored while a call that released one runs stops the process
    /// ([`fail_closed`]), before the library's send. A recorded lock ends
    /// its call's signing of any other (one lock per call, O-1:
    /// [`SignatureCount::admits`]). Blocking, outside J: it runs inside the
    /// library's store call, which waits for it.
    pub(crate) fn record_signed_locks(&self, wallet: WalletId, locks: &[StoredLock]) {
        if locks.is_empty() {
            // An asset-lock changeset with no lock (review P2b r7): a
            // running call that released a signature no recorded lock of it
            // covers may have signed a lock the library sends unstored, a
            // second one among them (R6-02). It stops here too; its release
            // row keeps its step `MaybeSent`.
            let uncovered = self.with_j(|i, _| {
                let mut uncovered = false;
                for f in i.calls.values().filter_map(|r| r.funding.as_ref()) {
                    if f.wallet == wallet && f.signatures.uncovered() {
                        f.signatures.note_recorded();
                        uncovered = true;
                    }
                }
                uncovered
            });
            if uncovered {
                fail_closed(
                    wallet,
                    "a funding call's signatures reached a store with no lock",
                );
            }
            return;
        }
        let backend = self.journal.get();
        let classified = self.with_j(|i, fx| -> Result<_, &'static str> {
            let mut found: Vec<(String, SignedLock)> = Vec::new();
            for lock in locks {
                let Some(sighashes) = lock.sighashes.as_deref() else {
                    // Only the builder stores a `Built` lock, before its
                    // send: one no call can be shown to have signed may
                    // still be a running call's.
                    let signing = (i.calls.values().filter_map(|r| r.funding.as_ref()))
                        .chain(&i.fence.ended_funding)
                        .any(|f| f.wallet == wallet && f.signatures.released() > 0);
                    if lock.built && signing {
                        return Err(
                            "an unverifiable built asset lock is stored while a funding call signs",
                        );
                    }
                    continue;
                };
                let signers: Vec<(String, Arc<SignatureCount>)> = i
                    .calls
                    .values()
                    .filter_map(|r| r.funding.as_ref())
                    .filter(|f| f.wallet == wallet && f.signatures.signed_any(sighashes))
                    .map(|f| (f.step.clone(), f.signatures.clone()))
                    .collect();
                for (step, signatures) in signers {
                    if !signatures.signed_all(sighashes) {
                        return Err(
                            "an asset lock carries only some of a funding call's signatures",
                        );
                    }
                    // From here on the call signs no second lock (O-1); one
                    // it signed or is signing before is a second lock under
                    // the permit (R6-02), which no record covers.
                    signatures.note_recorded();
                    if !signatures.signed_only(sighashes) {
                        return Err("a funding call signed more than one asset lock");
                    }
                    if !i.fence.call_marked(&wallet, &step) {
                        return Err("a signing funding call's marker no longer stands");
                    }
                    let known = i
                        .fence
                        .signed_locks
                        .entry((wallet, step.clone()))
                        .or_default();
                    match known.iter().find(|l| l.txid == lock.txid) {
                        Some(l) if l.durable => continue,
                        Some(_) => {}
                        None => known.push(SignedLock {
                            txid: lock.txid,
                            inputs: lock.inputs.clone(),
                            durable: false,
                        }),
                    }
                    if !found.iter().any(|(s, l)| *s == step && l.txid == lock.txid) {
                        let pending = SignedLock {
                            txid: lock.txid,
                            inputs: lock.inputs.clone(),
                            durable: false,
                        };
                        found.push((step, pending));
                    }
                }
                // Its call ended before its store: the library call outlived
                // the permit, and its send would follow this store. Only the
                // builder stores a lock `Built`, before its send; a copy of
                // an ended call's lock stored otherwise (a reconstruction of
                // a malleated copy, whose signatures read the same sighashes)
                // was sent by someone else and costs liveness only (§2a.6).
                if lock.built
                    && i.fence.ended_funding.iter().any(|f| {
                        f.wallet == wallet
                            && f.signatures.signed_any(sighashes)
                            && !i.fence.recorded_durably(&wallet, &f.step, lock.txid)
                    })
                {
                    return Err("a lock of an ended funding call reached the store unrecorded");
                }
            }
            let ready = backend.is_some() && i.fence.journal == JournalState::Ready;
            if !found.is_empty() {
                if ready {
                    i.fence.writes += 1;
                }
                fx.notify();
                super::mode_b::rederive(self, i, fx, wallet);
            }
            Ok((found, ready))
        });
        let (mut found, ready) = match classified {
            Ok(c) => c,
            Err(why) => fail_closed(wallet, why),
        };
        if found.is_empty() {
            return;
        }
        let Some(backend) = backend.filter(|_| ready) else {
            fail_closed(wallet, "the journal is not ready to record a signed lock");
        };
        let mut delay = SIGNED_WRITE_BACKOFF;
        for attempt in 1..=SIGNED_WRITE_ATTEMPTS {
            let mut failed = false;
            for (step, lock) in found.iter_mut().filter(|(_, l)| !l.durable) {
                let id = ArtifactId::of_step(&wallet, step);
                let inputs: Vec<StoredOutPoint> = lock
                    .inputs
                    .iter()
                    .map(|o| StoredOutPoint {
                        txid: o.txid.to_byte_array(),
                        vout: o.vout,
                    })
                    .collect();
                let txid = lock.txid.to_byte_array();
                match guarded(|| {
                    blocking(|| backend.insert_signed_lock(&wallet.0, step, &id.0, &txid, &inputs))
                }) {
                    Ok(true) => lock.durable = true,
                    Ok(false) => {
                        self.with_j(|i, fx| end_write(i, fx, true));
                        fail_closed(
                            wallet,
                            "a signing funding call's marker no longer stands on disk",
                        );
                    }
                    Err(e) => {
                        failed = true;
                        tracing::warn!(step, attempt, error = %e, "recording a signed lock failed");
                    }
                }
            }
            if !failed {
                break;
            }
            if attempt == SIGNED_WRITE_ATTEMPTS {
                self.with_j(|i, fx| end_write(i, fx, false));
                fail_closed(wallet, "recording a signed lock keeps failing");
            }
            blocking(|| std::thread::sleep(delay));
            delay *= 2;
        }
        let resume = self.with_j(|i, fx| {
            fx.notify();
            end_write(i, fx, true);
            for (step, lock) in found {
                if let Some(known) = i.fence.signed_locks.get_mut(&(wallet, step))
                    && let Some(k) = known.iter_mut().find(|l| l.txid == lock.txid)
                {
                    k.durable = true;
                }
            }
            // A row already proved is journaled now.
            super::mode_b::rederive(self, i, fx, wallet);
            // A pass that skipped a row for want of this record runs again.
            i.mode_b.passes.take_deferred(&wallet)
        });
        if resume && let Some(me) = self.me.upgrade() {
            me.request_resume(Some(wallet));
        }
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

    /// The wallet holds positive network evidence that the transaction is
    /// out (BL-76, §4.6): an InstantSend lock, a block that holds it, or a
    /// ChainLock over that block (`SessionHub`): `Sent`.
    pub fn note_seen(self: &Arc<Self>, wallet: WalletId, artifact: ArtifactId) {
        self.note_seen_all(wallet, [artifact]);
    }

    /// [`Self::note_seen`] of every artifact in one J step (a block's or a
    /// ChainLock's transactions).
    pub fn note_seen_all(
        self: &Arc<Self>,
        wallet: WalletId,
        artifacts: impl IntoIterator<Item = ArtifactId>,
    ) {
        let backend = self.journal.get();
        self.with_j(|i, fx| {
            for artifact in artifacts {
                // Another wallet's sighting says nothing of this one's
                // entry (review r2 L2). A step marker alone is one: after a
                // restart a marked artifact has no row-less entry.
                let known = i.fence.rowless.contains_key(&row_key(i, wallet, artifact))
                    || i.fence.entries.contains_key(&(wallet, artifact))
                    || i.fence
                        .steps
                        .iter()
                        .any(|((w, _), m)| *w == wallet && m.contains_key(&artifact));
                if !known {
                    continue;
                }
                if i.fence.sent.insert(artifact) {
                    resolved(self, fx, &wallet, &artifact, DispatchResolution::Sent);
                }
                // Begins the row, or again after a failed write.
                self.begin_sent_row(i, backend.clone(), wallet, artifact);
            }
        });
    }

    /// `dispatch_status` (§16.6), first matching row wins. The tracked-row
    /// rows of the table are Mode A's (P4) and Mode B's (P2b).
    pub fn dispatch_status(&self, wallet: WalletId, artifact: ArtifactId) -> Option<DispatchState> {
        self.with_j(|i, _| {
            note_hold_read(i, &wallet, &artifact);
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
                    // The same bytes may be out under another wallet.
                    RowlessState::Revoked | RowlessState::NotSent
                        if f.held_elsewhere(&wallet, &artifact) =>
                    {
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
                fx.notify();
                end_write(i, fx, erased.is_ok());
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
