//! DP1-02's registration funding ownership model, the [`Ledger`]
//! (`docs/design/DP1-02-ownership-model.md` rev2, §3 to §5, with the
//! manager's rulings of §13 and §13.1). Not part of the facade.
//!
//! Every path that decides which asset lock is whose (the run's funding
//! step before and after its library call, the changeset tap, "Finish
//! transfers") and every gate (discard, "in progress", resume, the status's
//! `funds_committed`) reads one ledger, built in one job of the wallet's
//! registration actor, and asks only its functions (I8). Ownership is read
//! from the wallet's funding steps (P2b's markers, signed records and
//! tombstones), never from the rows alone, so deleting a row changes no
//! lock's owner (I2), and a marked step whose signed txid is not known is
//! `Unknown`, never "no funding" (I1).

use dashcore::{OutPoint, Txid};
use dw_appdb::{AppDb, AppDbError, RegistrationRow};
use platform_wallet::AssetLockFundingType;

use super::errors::{PlatformError, RegistrationError};
use super::flows::DispatchState;
use super::lease::mode_b::{FundingStepView, FundingStepsUnavailable};
use super::lease::{FundingKey, LeaseTable};
use super::registration_flow::{
    Flows, LockInfo, LockStatus, RegistrationLib, b58, broadcast_unknown, failure_of, identity_for,
    interpret, is_core_balance, is_unsupported, ledger_op, may_own_an_unrecorded_lock, outpoint_of,
};
use super::registration_rows::{Funding, Phase};
use crate::{NetworkSession, WalletId};

/// What a funding step says of its own lock (§3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepClaim {
    /// Never marked, or resolved `NotSent` (P2b's reading of a flagged
    /// step with tombstones too, C4 and Q13): nothing of the step can still
    /// be out.
    None,
    /// A call was marked and signed these txids.
    Exact(Vec<Txid>),
    /// A call was marked and may have signed, but no txid is known: a call
    /// before its record, a `NotSent` write in flight, an unflagged marker.
    /// Never read as "no funding" (I1).
    Unknown,
}

impl StepClaim {
    fn of(step: &FundingStepView) -> Self {
        match step.status {
            None | Some(DispatchState::NotSent) => Self::None,
            Some(_) if step.signed.is_empty() => Self::Unknown,
            Some(_) => Self::Exact(step.signed.clone()),
        }
    }
}

/// A row's standing (§3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// This build cannot read the row (DEC-171 (3)).
    Unreadable { committed: bool },
    /// No funding of the row's is out: it may fund, if it is a Core-balance
    /// row with a free index.
    Unfunded,
    /// It holds a live lock that is not consumed.
    Funded(OutPoint),
    /// An outstanding step of its own or inherited, and it can still take
    /// that step's lock.
    Awaiting,
    /// An outstanding step whose lock can no longer come back to it.
    Closed,
    /// Its identity exists, or its live lock is consumed, and nothing of its
    /// funding is outstanding.
    Settled,
    /// Its intent is closed for good with this failure code (rows 7 and 8):
    /// another row holds its funding, or its funding was cancelled.
    Cancelled(&'static str),
}

/// Who owns an unheld, unconsumed registration lock of account 0 (§3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Owner {
    /// Signed by the funding step of this draft, which is outstanding.
    Step(i64),
    /// A row this build cannot read may own it (DEC-173 (2)).
    Parked,
    /// It may be the lock of an `Unknown` step (I5).
    Contested,
    /// A tombstone, or signed by a step that is no longer outstanding: it
    /// cannot land.
    Dead,
    Free,
}

/// Who may take a lock (§5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Taker {
    Row(i64),
    /// A new unnamed row "Finish transfers" makes for the lock: it has no
    /// step and never funds.
    Recovery,
}

/// Terminal evidence's one write for one row (§5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reconcile {
    /// (a) The row's step signed `lock`, which is consumed by `identity`,
    /// discovered at `index`: the row records both and goes on to its name
    /// and profile, with no funding and no identity creation.
    Attach {
        row: i64,
        lock: OutPoint,
        identity: String,
        index: u32,
    },
    /// (b) `clear` drops a voided outpoint (back to `KeysPrepared`); `code`
    /// closes the row for good, and its backup is taken again (MN-2).
    Close {
        row: i64,
        clear: bool,
        code: Option<&'static str>,
    },
}

impl Reconcile {
    pub(crate) fn row(&self) -> i64 {
        match self {
            Self::Attach { row, .. } | Self::Close { row, .. } => *row,
        }
    }

    /// The write closes the row: its backup is taken again.
    pub(crate) fn closes(&self) -> bool {
        matches!(self, Self::Close { code: Some(_), .. })
    }
}

/// Who an identity index belongs to, for an `Unfunded` row (§6
/// `index_owner`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexOwner {
    /// Nothing else: the row may fund it.
    Free,
    /// A lock the row may take.
    Lock(OutPoint),
    /// Another flow's, an identity's that is not the row's, or a settled or
    /// cancelled step's: the row moves to a new index.
    Taken,
    /// Not known yet: the row waits; "Finish transfers" decides.
    Unknown,
}

/// The `registration.funded_elsewhere` failure (§3.3 row 7).
pub(crate) fn funded_elsewhere() -> &'static str {
    RegistrationError::FundedElsewhere.code()
}

/// The `registration.funding_cancelled` failure (§3.3 row 8).
pub(crate) fn funding_cancelled() -> &'static str {
    RegistrationError::FundingCancelled.code()
}

/// `row` failed with a code §5.2b writes: closed for good.
fn closed(row: &RegistrationRow) -> bool {
    failure_of(row).is_some_and(|(c, _)| c == funded_elsewhere() || c == funding_cancelled())
}

/// One funding step as the ledger reads it.
#[derive(Debug, Clone)]
struct Step {
    view: FundingStepView,
    claim: StepClaim,
    /// `signer(S)`: the draft's row, else its heir.
    signer: Option<i64>,
    /// `holder(S)`: the row whose live outpoint is a lock of `own(S)`.
    holder: Option<i64>,
    /// `row_of(S)`: exact possession beats draft id and heir.
    row_of: Option<i64>,
    /// `own(S)`.
    own: Vec<Txid>,
    /// `outstanding(S)`.
    outstanding: bool,
}

/// One wallet's registration evidence, read once per decision (§5): its
/// tracked locks and identities, its rows, its funding steps and the drafts
/// whose library call runs.
#[derive(Debug, Clone)]
pub(crate) struct Ledger {
    locks: Vec<LockInfo>,
    identities: Vec<(u32, String)>,
    /// Oldest first.
    rows: Vec<RegistrationRow>,
    steps: Vec<Step>,
    /// Every step's tombstones (`voided`).
    dead: Vec<Txid>,
    calling: Vec<i64>,
}

impl Ledger {
    /// The ledger of `wallet`, in a job of its registration actor: the rows
    /// and the drafts in a call read there, the funding steps in one J step
    /// of `leases` (PA-1), with the library's `locks` and `identities` read
    /// before the job. A journal PA-1 cannot read is [`FundingStepsUnavailable`] (the
    /// inner error): the decision fails closed.
    pub(crate) fn read_in(
        db: &AppDb,
        flows: &Flows,
        leases: &LeaseTable,
        wallet: WalletId,
        locks: Vec<LockInfo>,
        identities: Vec<(u32, String)>,
    ) -> Result<Result<Self, FundingStepsUnavailable>, AppDbError> {
        let rows = db.registration_rows(&wallet.to_string())?;
        let steps = leases.registration_funding_steps(wallet);
        #[cfg(test)]
        let steps = steps.map(|steps| test_views::overlay(leases, wallet, steps));
        Ok(steps
            .map(|steps| Self::new(locks, identities, rows, steps, flows.in_call_drafts(wallet))))
    }

    /// [`Self::read_in`] as one error: a storage error, and no write.
    pub(crate) fn read(
        db: &AppDb,
        flows: &Flows,
        leases: &LeaseTable,
        wallet: WalletId,
        locks: Vec<LockInfo>,
        identities: Vec<(u32, String)>,
    ) -> Result<Self, PlatformError> {
        match Self::read_in(db, flows, leases, wallet, locks, identities) {
            Ok(Ok(ledger)) => Ok(ledger),
            Ok(Err(FundingStepsUnavailable)) => Err(PlatformError::Storage {
                detail: "the dispatch journal is unavailable".into(),
            }),
            Err(e) => Err(crate::EngineError::from(e).into()),
        }
    }

    /// [`Self::read`] from a task: the library's locks and identities, then
    /// one job of the wallet's actor.
    pub(crate) async fn load<L: RegistrationLib>(
        session: &NetworkSession,
        wallet: WalletId,
        lib: &L,
    ) -> Result<Self, PlatformError> {
        let (locks, identities) = (lib.tracked_locks().await, lib.identities().await);
        let leases = std::sync::Arc::clone(&session.leases);
        ledger_op(session, wallet, move |db, flows| {
            Self::read(db, flows, &leases, wallet, locks, identities)
        })
        .await
    }

    /// A ledger of explicit evidence: [`Self::read`]'s, and the tests'.
    pub(crate) fn new(
        locks: Vec<LockInfo>,
        identities: Vec<(u32, String)>,
        mut rows: Vec<RegistrationRow>,
        mut views: Vec<FundingStepView>,
        calling: Vec<i64>,
    ) -> Self {
        rows.sort_by_key(|r| r.id);
        views.sort_by_key(|s| s.draft);
        let dead = views.iter().flat_map(|s| s.dead.iter().copied()).collect();
        let mut ledger = Self {
            locks,
            identities,
            rows,
            steps: Vec::new(),
            dead,
            calling,
        };
        let drafts: Vec<i64> = views.iter().map(|s| s.draft).collect();
        let steps = views
            .into_iter()
            .map(|view| {
                let claim = StepClaim::of(&view);
                // The heir (F10, Q5): the oldest open row at the key's index
                // whose own funding key is exactly the marker's, and whose
                // id drafts no step.
                let signer = ledger.row(view.draft).map(|r| r.id).or_else(|| {
                    ledger
                        .rows
                        .iter()
                        .find(|r| !drafts.contains(&r.id) && ledger.may_inherit(r, &view.key))
                        .map(|r| r.id)
                });
                let own = if view.signed.is_empty() {
                    signer
                        .and_then(|d| ledger.row(d))
                        .and_then(|r| ledger.live(r))
                        .map(|o| vec![o.txid])
                        .unwrap_or_default()
                } else {
                    view.signed.clone()
                };
                let holder = ledger
                    .rows
                    .iter()
                    .find(|r| ledger.live(r).is_some_and(|o| own.contains(&o.txid)))
                    .map(|r| r.id);
                // D15: the holder, else the signer; none if that row is closed
                // with a cancel code (never another candidate), so its close
                // sticks whatever later evidence reads (§5.2b); the step then
                // settles in place, or its lock goes to `Recovery`.
                let row_of = holder
                    .or(signer)
                    .filter(|d| ledger.row(*d).is_none_or(|r| !closed(r)));
                let mut step = Step {
                    view,
                    claim,
                    signer,
                    holder,
                    row_of,
                    own,
                    outstanding: false,
                };
                step.outstanding = step.claim != StepClaim::None && !ledger.settled(&step);
                step
            })
            .collect();
        ledger.steps = steps;
        ledger
    }

    pub(crate) fn rows(&self) -> &[RegistrationRow] {
        &self.rows
    }

    pub(crate) fn locks(&self) -> &[LockInfo] {
        &self.locks
    }

    pub(crate) fn row(&self, id: i64) -> Option<&RegistrationRow> {
        self.rows.iter().find(|r| r.id == id)
    }

    fn step(&self, draft: i64) -> Option<&Step> {
        self.steps.iter().find(|s| s.view.draft == draft)
    }

    /// A library call of `draft`'s run runs (`calling(r)`).
    pub(crate) fn calling(&self, draft: i64) -> bool {
        self.calling.contains(&draft)
    }

    // --- §3.2 predicates ------------------------------------------------

    /// `voided(o)`: a ChainLocked self-spend took the inputs of a lock of
    /// this txid.
    fn voided(&self, txid: Txid) -> bool {
        self.dead.contains(&txid)
    }

    /// `live(r)`: the row's recorded outpoint, unless it is voided.
    pub(crate) fn live(&self, row: &RegistrationRow) -> Option<OutPoint> {
        outpoint_of(row).filter(|o| !self.voided(o.txid))
    }

    /// The row records an outpoint that is voided (§5.2b's `clear`).
    fn records_voided(&self, row: &RegistrationRow) -> bool {
        outpoint_of(row).is_some_and(|o| self.voided(o.txid))
    }

    /// `consumed(o)`: the lock is tracked `Consumed`, the identity it
    /// creates is discovered, or a row holds it with an identity.
    fn consumed(&self, outpoint: OutPoint) -> bool {
        if self
            .locks
            .iter()
            .any(|l| l.outpoint == outpoint && l.status == LockStatus::Consumed)
        {
            return true;
        }
        let id = b58(&identity_for(outpoint));
        self.identities.iter().any(|(_, i)| *i == id)
            || self
                .rows
                .iter()
                .any(|r| outpoint_of(r) == Some(outpoint) && r.identity.is_some())
    }

    /// `lock(t)` (F9): the tracked lock of txid `t`, else its credit output
    /// 0.
    fn lock_of(&self, txid: Txid) -> OutPoint {
        self.locks
            .iter()
            .find(|l| l.outpoint.txid == txid)
            .map_or(OutPoint::new(txid, 0), |l| l.outpoint)
    }

    /// A row's live outpoint is a lock of `txid`.
    fn held(&self, txid: Txid) -> bool {
        self.rows
            .iter()
            .any(|r| self.live(r).is_some_and(|o| o.txid == txid))
    }

    /// `settled(S)`: every lock of `own(S)` is consumed, and is held, or no
    /// open row of the step still has to receive its identity (§5.2a's
    /// in-place arm).
    fn settled(&self, s: &Step) -> bool {
        let row_of = s.row_of.and_then(|d| self.row(d));
        !s.own.is_empty()
            && s.own.iter().all(|t| {
                self.consumed(self.lock_of(*t))
                    && (self.held(*t) || row_of.is_none_or(|r| !self.open(r)))
            })
    }

    /// `open(r)`: a readable Core-balance row with an index, no live
    /// outpoint and no identity, whatever its phase or failure.
    pub(crate) fn open(&self, row: &RegistrationRow) -> bool {
        matches!(interpret(row), Ok((_, Funding::CoreBalance { .. })))
            && row.identity_index.is_some()
            && self.live(row).is_none()
            && row.identity.is_none()
    }

    /// `row` may inherit an orphan step of `key` (§3.2's heir, Q5): an
    /// open row whose own funding key is exactly the marker's.
    fn may_inherit(&self, row: &RegistrationRow, key: &FundingKey) -> bool {
        self.open(row)
            && row.identity_index.is_some_and(|index| {
                *key == FundingKey {
                    funding_type: AssetLockFundingType::IdentityRegistration,
                    identity_index: index,
                    account: 0,
                }
            })
    }

    /// `steps(r)`: the outstanding steps whose row is `row`.
    fn steps_of(&self, row: i64) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(move |s| s.outstanding && s.row_of == Some(row))
    }

    /// The signed txids of `draft`'s own step (`signed(S_r)`).
    pub(crate) fn signed_of(&self, draft: i64) -> &[Txid] {
        self.step(draft).map_or(&[], |s| s.view.signed.as_slice())
    }

    /// `cancelled(S)` for `draft`'s own step (§3.2): it is in the view with
    /// claim `None`, so it is never called again.
    pub(crate) fn step_cancelled(&self, draft: i64) -> bool {
        self.step(draft).is_some_and(|s| s.claim == StepClaim::None)
    }

    /// The claim of `draft`'s own step: `None` once it left the view.
    pub(crate) fn claim_of(&self, draft: i64) -> StepClaim {
        self.step(draft)
            .map_or(StepClaim::None, |s| s.claim.clone())
    }

    /// An outstanding step of `row` is `Unknown` (the run cannot know its
    /// lock: `RegistrationWait::FundingUnknown`).
    pub(crate) fn funding_unknown(&self, row: &RegistrationRow) -> bool {
        self.steps_of(row.id).any(|s| s.claim == StepClaim::Unknown)
    }

    /// The open `row` waits for discovery to report the identity its step's
    /// consumed lock created (§5.2a: `RegistrationWait::Network`).
    pub(crate) fn awaits_discovery(&self, row: &RegistrationRow) -> bool {
        self.open(row)
            && self
                .steps_of(row.id)
                .flat_map(|s| s.view.signed.iter())
                .any(|t| {
                    let o = self.lock_of(*t);
                    !self.held(*t) && self.consumed(o) && self.discovered(o).is_none()
                })
    }

    /// The identity `lock` created, if discovery reports it: (index, id).
    fn discovered(&self, lock: OutPoint) -> Option<&(u32, String)> {
        let id = b58(&identity_for(lock));
        self.identities.iter().find(|(_, i)| *i == id)
    }

    /// The outstanding steps with no row (orphans, F6): their drafts, and
    /// whether each is `Unknown`.
    pub(crate) fn orphans(&self) -> Vec<(i64, bool)> {
        self.steps
            .iter()
            .filter(|s| s.outstanding && s.row_of.is_none())
            .map(|s| (s.view.draft, s.claim == StepClaim::Unknown))
            .collect()
    }

    /// An orphan step's signed txid, if one is known.
    pub(crate) fn orphan_txid(&self, draft: i64) -> Option<Txid> {
        self.step(draft)
            .and_then(|s| s.view.signed.first().copied())
    }

    // --- §3.3 -----------------------------------------------------------

    /// `standing(r)` (§3.3, the first matching line).
    pub(crate) fn standing(&self, row: &RegistrationRow) -> Standing {
        if is_unsupported(row) {
            return Standing::Unreadable {
                committed: self.unreadable_committed(row),
            };
        }
        if !is_core_balance(row) {
            return Standing::Unfunded;
        }
        let live = self.live(row);
        if let Some(o) = live
            && !self.consumed(o)
        {
            return Standing::Funded(o);
        }
        if self.steps_of(row.id).next().is_some() {
            return if self.open(row) {
                Standing::Awaiting
            } else {
                Standing::Closed
            };
        }
        if row.identity.is_some() || live.is_some() {
            return Standing::Settled;
        }
        if self.open(row)
            && !self.calling(row.id)
            && let Some(code) = self.cancelled(row)
        {
            return Standing::Cancelled(code);
        }
        Standing::Unfunded
    }

    /// Rows 7 and 8's evidence for an open row with no outstanding step;
    /// `funded_elsewhere` first.
    fn cancelled(&self, row: &RegistrationRow) -> Option<&'static str> {
        let failed_with = |code: &str| failure_of(row).is_some_and(|(c, _)| c == code);
        let signs = |s: &&Step| s.signer == Some(row.id);
        if failed_with(funded_elsewhere())
            || self
                .steps
                .iter()
                .filter(signs)
                .any(|s| s.holder.is_some_and(|h| h != row.id))
        {
            return Some(funded_elsewhere());
        }
        if failed_with(funding_cancelled())
            || self.records_voided(row)
            || self
                .steps
                .iter()
                .filter(signs)
                .any(|s| s.claim == StepClaim::None)
        {
            return Some(funding_cancelled());
        }
        None
    }

    /// DEC-171 (3)'s may-own rule, unchanged: a row this build cannot read
    /// is committed once anything says it may have funded: its outpoint, its
    /// identity, a step of its own, or a lock no row holds at its index or
    /// whose index the library lost.
    fn unreadable_committed(&self, row: &RegistrationRow) -> bool {
        row.asset_lock_outpoint.is_some()
            || row.identity.is_some()
            || self.steps_of(row.id).next().is_some()
            || self.locks.iter().any(|l| {
                matches!(
                    self.owner(l),
                    Some(Owner::Free | Owner::Parked | Owner::Contested)
                ) && (l.known_index().is_none() || l.known_index() == row.identity_index)
            })
    }

    /// `committed(r)` (§3.3), or its funding call runs. The gates'
    /// predicate, never the status's display field.
    pub(crate) fn committed(&self, row: &RegistrationRow) -> bool {
        match self.standing(row) {
            Standing::Unreadable { committed } => committed,
            Standing::Funded(_) | Standing::Awaiting | Standing::Closed => true,
            Standing::Settled | Standing::Cancelled(_) => false,
            Standing::Unfunded => is_core_balance(row) && self.calling(row.id),
        }
    }

    // --- §3.4 -----------------------------------------------------------

    /// `lock`'s owner whatever its consumption: `None` for a lock that is no
    /// registration lock of account 0, or that a row holds live.
    fn class(&self, lock: &LockInfo) -> Option<Owner> {
        let txid = lock.outpoint.txid;
        if !lock.funds_registration() || self.held(txid) {
            return None;
        }
        let mut signers = self.steps.iter().filter(|s| s.view.signed.contains(&txid));
        if self.voided(txid) || signers.clone().any(|s| !s.outstanding) {
            return Some(Owner::Dead);
        }
        if let Some(s) = signers.next() {
            return Some(Owner::Step(s.view.draft));
        }
        if self.may_be_unreadables(lock) {
            return Some(Owner::Parked);
        }
        if self.contested(lock) {
            return Some(Owner::Contested);
        }
        Some(Owner::Free)
    }

    /// `owner(L)` (§3.4): `None` for a lock a row holds live, and for a
    /// consumed one (consumed signed value is §5.2a's).
    pub(crate) fn owner(&self, lock: &LockInfo) -> Option<Owner> {
        if lock.status == LockStatus::Consumed || self.consumed(lock.outpoint) {
            return None;
        }
        self.class(lock)
    }

    /// A row this build cannot read may own `lock` (DEC-173 (2)): one at
    /// its index, or any such row once the library lost the lock's index.
    fn may_be_unreadables(&self, lock: &LockInfo) -> bool {
        let index = lock.known_index();
        self.rows.iter().any(|r| {
            may_own_an_unrecorded_lock(r) && (index.is_none() || r.identity_index == index)
        })
    }

    /// Row 4: an outstanding `Unknown` step with no lock of its own may have
    /// built `lock` (its index is the step's, or lost), unless the amount
    /// differs from the one its row locks (Q6).
    fn contested(&self, lock: &LockInfo) -> bool {
        self.steps.iter().any(|s| {
            let row = s.row_of.and_then(|d| self.row(d));
            s.outstanding
                && s.claim == StepClaim::Unknown
                && s.own.is_empty()
                && lock
                    .known_index()
                    .is_none_or(|i| i == s.view.key.identity_index)
                && !row.is_some_and(|r| {
                    matches!(interpret(r), Ok((_, Funding::CoreBalance { lock_duffs }))
                        if lock_duffs != lock.amount)
                })
        })
    }

    // --- §5.1 -----------------------------------------------------------

    /// `may_take` (§5.1): whether `taker` may take `lock`, which no row
    /// holds live. Read whatever its consumption, for `index_owner`'s
    /// identity arm; the allocating paths skip consumed locks.
    pub(crate) fn may_take(&self, taker: Taker, lock: &LockInfo) -> bool {
        match self.class(lock) {
            Some(Owner::Step(draft)) => {
                let row_of = self
                    .step(draft)
                    .and_then(|s| s.row_of)
                    .and_then(|d| self.row(d));
                match taker {
                    // OD-05: calling or not.
                    Taker::Row(id) => row_of.is_some_and(|r| r.id == id && self.open(r)),
                    Taker::Recovery => row_of.is_none_or(|r| !is_unsupported(r) && !self.open(r)),
                }
            }
            Some(Owner::Free) => match taker {
                Taker::Row(id) => self.row(id).is_some_and(|r| {
                    self.standing(r) == Standing::Unfunded && self.open(r) && !self.calling(id)
                }),
                Taker::Recovery => true,
            },
            Some(Owner::Parked | Owner::Contested | Owner::Dead) | None => false,
        }
    }

    /// `own_lock(r)`: an unconsumed tracked lock of one of `row`'s steps
    /// that it may take now.
    pub(crate) fn own_lock(&self, row: &RegistrationRow) -> Option<&LockInfo> {
        let drafts: Vec<i64> = self.steps_of(row.id).map(|s| s.view.draft).collect();
        self.locks.iter().find(|l| {
            matches!(self.owner(l), Some(Owner::Step(d)) if drafts.contains(&d))
                && self.may_take(Taker::Row(row.id), l)
        })
    }

    // --- §5.2 -----------------------------------------------------------

    /// `reconcile()` (§5.2): at most one write per row that isn't calling.
    /// (a) attaches consumed signed value to its open row once discovery
    /// reports the identity; (b) clears a voided outpoint and closes a
    /// `Cancelled` row whose failure isn't written yet. (c) needs no write:
    /// a recorded outpoint stands unless a tombstone voids it.
    pub(crate) fn reconcile(&self) -> Vec<Reconcile> {
        let mut out = Vec::new();
        // DEC-171 (3): a row this build cannot read is never written.
        for row in self
            .rows
            .iter()
            .filter(|r| !self.calling(r.id) && !is_unsupported(r))
        {
            if let Some(attach) = self.attach(row) {
                out.push(attach);
                continue;
            }
            // D11: a row with an identity is never moved back to
            // `KeysPrepared`.
            let clear = self.records_voided(row) && row.identity.is_none();
            let code = match self.standing(row) {
                Standing::Cancelled(code) if failure_of(row) != Some((code, false)) => Some(code),
                _ => None,
            };
            if clear || code.is_some() {
                out.push(Reconcile::Close {
                    row: row.id,
                    clear,
                    code,
                });
            }
        }
        out
    }

    /// §5.2a for `row`.
    fn attach(&self, row: &RegistrationRow) -> Option<Reconcile> {
        if !self.open(row) {
            return None;
        }
        self.steps_of(row.id)
            .flat_map(|s| s.view.signed.iter())
            .find_map(|t| {
                let lock = self.lock_of(*t);
                if self.held(*t) || !self.consumed(lock) {
                    return None;
                }
                let (index, identity) = self.discovered(lock)?;
                Some(Reconcile::Attach {
                    row: row.id,
                    lock,
                    identity: identity.clone(),
                    index: *index,
                })
            })
    }

    // --- §5.3 -----------------------------------------------------------

    /// `pending` (§5.3): the outstanding steps no row holds a lock of, plus
    /// the locks that are `Parked`, `Contested` or `Free`. An `Unknown` step
    /// and a `Contested` lock that may be its count as two.
    pub(crate) fn pending(&self) -> u32 {
        let steps = self
            .steps
            .iter()
            .filter(|s| s.outstanding && !s.own.iter().any(|t| self.held(*t)))
            .count();
        let locks = self
            .locks
            .iter()
            .filter(|l| {
                matches!(
                    self.owner(l),
                    Some(Owner::Parked | Owner::Contested | Owner::Free)
                )
            })
            .count();
        u32::try_from(steps + locks).unwrap_or(u32::MAX)
    }

    // --- §6 index_owner and next_index ----------------------------------

    /// Who owns identity index `index` for the `Unfunded` `row` (§6).
    pub(crate) fn index_owner(&self, row: &RegistrationRow, index: u32) -> IndexOwner {
        if let Some(lock) = self.locks.iter().find(|l| {
            l.known_index() == Some(index)
                && self.owner(l) == Some(Owner::Free)
                && self.may_take(Taker::Row(row.id), l)
        }) {
            return IndexOwner::Lock(lock.outpoint);
        }
        if let Some((_, id)) = self.identities.iter().find(|(i, _)| *i == index) {
            // Restore rule 2: what the seed did at the index, adopted by its
            // lock, by the one ownership rule (R1-05, R7-02).
            if let Some(lock) = self
                .locks
                .iter()
                .find(|l| l.funds_registration() && b58(&identity_for(l.outpoint)) == *id)
            {
                return match self.class(lock) {
                    _ if self.may_take(Taker::Row(row.id), lock) => IndexOwner::Lock(lock.outpoint),
                    Some(Owner::Contested | Owner::Parked) => IndexOwner::Unknown,
                    _ => IndexOwner::Taken,
                };
            }
            if self
                .rows
                .iter()
                .any(|r| r.id != row.id && r.identity.as_deref() == Some(id.as_str()))
            {
                return IndexOwner::Taken;
            }
            return IndexOwner::Unknown;
        }
        // Rows restored behind reality may share an index: a funded,
        // committed or older one keeps it.
        let shared = self.rows.iter().any(|r| {
            r.id != row.id
                && r.identity_index == Some(index)
                && (r.asset_lock_outpoint.is_some()
                    || r.identity.is_some()
                    || self.committed(r)
                    || r.id < row.id)
        });
        if shared {
            return IndexOwner::Taken;
        }
        // I12, C3: another draft's step at the index, standing, settled or
        // cancelled (D14: after the rows, so a step whose committed row is
        // at the index sends this row on; an orphan's slot waits).
        let mut others = self
            .steps
            .iter()
            .filter(|s| s.view.draft != row.id && s.view.key.identity_index == index)
            .peekable();
        if others.peek().is_some() {
            return if others.any(|s| s.outstanding) {
                IndexOwner::Unknown
            } else {
                IndexOwner::Taken
            };
        }
        if self.unindexed() {
            return IndexOwner::Unknown;
        }
        IndexOwner::Free
    }

    /// An unheld `Free`, `Contested` or `Parked` lock whose index the
    /// library lost: it may be any row's.
    fn unindexed(&self) -> bool {
        self.locks.iter().any(|l| {
            l.known_index().is_none()
                && matches!(
                    self.owner(l),
                    Some(Owner::Free | Owner::Contested | Owner::Parked)
                )
        })
    }

    /// The `Unfunded` `row` cannot fund before "Finish transfers" or the
    /// library attributes what is at its index.
    pub(crate) fn unattributed(&self, row: &RegistrationRow) -> bool {
        match row.identity_index {
            Some(index) => self.index_owner(row, index) == IndexOwner::Unknown,
            None => self.unindexed(),
        }
    }

    /// One past every identity index the wallet's identities, its tracked
    /// registration locks, its rows and every step in the view (standing,
    /// settled or cancelled) use, so no new row lands on a step's slot (I12).
    pub(crate) fn next_index(&self) -> u32 {
        let used = self
            .identities
            .iter()
            .map(|(i, _)| *i)
            .chain(
                self.locks
                    .iter()
                    .filter(|l| l.registration)
                    .map(|l| l.identity_index),
            )
            .chain(self.rows.iter().filter_map(|r| r.identity_index))
            .chain(self.steps.iter().map(|s| s.view.key.identity_index))
            .max();
        used.map_or(0, |i| i.saturating_add(1))
    }

    // --- §5.4 gates -----------------------------------------------------

    /// "In progress" (§5.4) for one row: (no failure and not Done), or
    /// retryable, or (committed and not Done). A `Cancelled` row never is;
    /// an unreadable one while committed.
    pub(crate) fn in_progress(&self, row: &RegistrationRow) -> bool {
        let done = row.phase == Phase::Done.as_str();
        match self.standing(row) {
            Standing::Cancelled(_) => false,
            Standing::Unreadable { committed } => committed,
            _ => match failure_of(row) {
                None => !done,
                Some((_, retryable)) => retryable || (self.committed(row) && !done),
            },
        }
    }

    /// What `start_registration` waits for (§5.4, I6): a row in progress
    /// (its draft), else an orphan step (its funding step id).
    pub(crate) fn blocking(&self) -> Option<String> {
        if let Some(row) = self.rows.iter().find(|r| self.in_progress(r)) {
            return Some(row.id.to_string());
        }
        self.orphans()
            .first()
            .map(|(draft, _)| super::registration_flow::funding_step(*draft))
    }

    /// Why a discard of `row` is refused (§5.4), if it is.
    pub(crate) fn discard_refused(&self, row: &RegistrationRow) -> Option<&'static str> {
        if is_unsupported(row) {
            // DEC-171 (3): what this build cannot read, it does not delete.
            return Some("the registration was written by a newer version");
        }
        self.committed(row)
            .then_some("the registration's funds are committed")
    }

    /// A failed row may run again (§5.4 "resume").
    pub(crate) fn resumable(&self, row: &RegistrationRow) -> bool {
        match self.standing(row) {
            Standing::Unfunded => {
                row.retryable || row.error.as_deref() == Some(broadcast_unknown())
            }
            Standing::Funded(_) => true,
            Standing::Awaiting | Standing::Unreadable { .. } | Standing::Cancelled(_) => false,
            Standing::Closed | Standing::Settled => row.retryable,
        }
    }

    /// The status's `funds_committed` (E0-04 §16.5, Q4): committed, or a
    /// lock of the row's was committed or consumed once, so a `Settled` or
    /// `Done` row still reads `true`; a `Cancelled` one reads `false`.
    pub(crate) fn funds_committed(&self, row: &RegistrationRow) -> bool {
        match self.standing(row) {
            Standing::Unreadable { committed } => committed,
            Standing::Cancelled(_) => false,
            _ => {
                self.committed(row)
                    || (is_core_balance(row)
                        && (row.asset_lock_outpoint.is_some()
                            || row.identity.is_some()
                            || Phase::parse(&row.phase).is_some_and(|p| !p.unfunded())))
            }
        }
    }
}

/// A test seam on [`Ledger::read_in`] only: PA-1 views a test sets for one
/// session's lease table, standing in for what the local PA-1 stub cannot
/// list yet (PA-3 tombstones, OD-02's flag, retention across
/// `erase_wallet`). A view set replaces the stub's view of its draft; the
/// others are added. Never in a build that ships.
#[cfg(test)]
pub(crate) mod test_views {
    use std::sync::{Arc, Mutex, PoisonError};

    use super::{FundingStepView, LeaseTable, WalletId};

    /// The views set, by lease table (its address, pinned by the guard's
    /// `Arc`) and wallet.
    static VIEWS: Mutex<Vec<(usize, WalletId, FundingStepView)>> = Mutex::new(Vec::new());

    fn key(leases: &LeaseTable) -> usize {
        std::ptr::from_ref(leases) as usize
    }

    /// The views of one session's wallet, for as long as it lives.
    pub(crate) struct Views {
        leases: Arc<LeaseTable>,
        wallet: WalletId,
    }

    impl Views {
        pub(crate) fn new(leases: &Arc<LeaseTable>, wallet: WalletId) -> Self {
            Self {
                leases: Arc::clone(leases),
                wallet,
            }
        }

        /// Sets the view of `view.draft`, replacing an earlier one.
        pub(crate) fn set(&self, view: FundingStepView) {
            let k = key(&self.leases);
            let mut views = VIEWS.lock().unwrap_or_else(PoisonError::into_inner);
            views.retain(|(l, w, v)| !(*l == k && *w == self.wallet && v.draft == view.draft));
            views.push((k, self.wallet, view));
        }
    }

    impl Drop for Views {
        fn drop(&mut self) {
            let k = key(&self.leases);
            VIEWS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|(l, w, _)| !(*l == k && *w == self.wallet));
        }
    }

    /// `steps` (the stub's) with the views set for `leases` and `wallet`.
    pub(super) fn overlay(
        leases: &LeaseTable,
        wallet: WalletId,
        mut steps: Vec<FundingStepView>,
    ) -> Vec<FundingStepView> {
        let k = key(leases);
        let views = VIEWS.lock().unwrap_or_else(PoisonError::into_inner);
        for (_, _, view) in views.iter().filter(|(l, w, _)| *l == k && *w == wallet) {
            steps.retain(|s| s.draft != view.draft);
            steps.push(view.clone());
        }
        steps
    }
}
