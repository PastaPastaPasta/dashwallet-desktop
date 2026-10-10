//! Mode B's call permits (E0-04 §2a.3, H13).
//!
//! Without the platform PR the engine cannot see a library call's hand-offs,
//! so it fences the call as a whole. Each library write call runs under a
//! [`CallPermit`], taken in one J step that checks the lease is live and
//! charges the call's quote. The permit's deadline is its grant + H, and a
//! lock's drain waits for it as for a First permit (H3). The signers handed
//! to the call count the signatures they release ([`SignatureCount`]); that
//! count decides the call's outcome:
//! - `Sent` when the call returned Ok;
//! - `Cancelled` only when it failed, released no signature at all, and no
//!   earlier marker of its step exists: no signature means no artifact could
//!   be formed, whatever the call did inside;
//! - `MaybeSent` otherwise, a call still running at the report included.
//!
//! A call that met a `Locked` signer parks its flow, whatever its outcome.
//!
//! **Durable markers first** (§2a.3, review Opus r2 4a). A funding call or
//! a resumable step's call takes its permit with
//! [`Lease::call_permit_step`]: the J step that takes the permit also sets
//! the step's marker `Committing` and spawns its durable write, and the
//! permit is handed back only once that write returned, so the library
//! call starts after the marker is durable. The marker is an ordinary step
//! marker of a synthetic artifact
//! ([`ArtifactId::of_step`](super::ArtifactId::of_step)). The call's end
//! writes its definite resolution: Ok a `Sent` row (the DEC-163 bundle),
//! a definite Cancelled a `NotSent` row, and MaybeSent nothing, so the
//! marker stands.
//!
//! **Funding calls** (§2a.3 "Funding cap", "No funding build over a
//! possibly-sent row"): a [`CallStep::Funding`] permit is charged a
//! [`FundingQuote`], `amount + fee_bound_worst`, against the Funding budget,
//! and passes the gate of [`super::mode_b`] first. Its signer is
//! [`CallPermit::funding_signer`], which counts like the identity signer.
//!
//! Mode B residual (D5, design Q8): the cap is not re-checked against the
//! call's own coin selection. key-wallet signs input by input inside
//! `build_asset_lock_with_signer`, with no transaction-level hook before
//! the first signature where the engine could read the selected inputs and
//! the fee, so the exact debit is checked only in Mode A (at `register`).
//! `fee_bound_worst` is an upper bound instead: the fee of spending every
//! coin of the funding accounts, at most key-wallet's 500 inputs (it
//! refuses a larger selection). It counts the coins when the permit is
//! taken: a coin received before the build that the selection then picks
//! is not in it, so below 500 coins the bound can fall short by that
//! coin's input (149 duffs each).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use dw_appdb::dispatch::StoredFundingKey;
use platform_wallet::AssetLockFundingType;
use tokio::time::Instant;

use super::budget::Charge;
use super::fence::CallGate;
use super::mode_b::funding_gate;
#[cfg(test)]
use super::stress_tests::Mutation;
use super::table::{Effects, Inner, Lease, LeaseTable};
use super::{FlowOutcome, LeaseError, LeaseId};
use crate::platform::flows::BudgetPurpose;
use crate::platform::signers::{CountingFundingSigner, VaultIdentitySigner};

/// A funding step's key (§2a.3, §2a.5): what platform-wallet's tracked
/// asset-lock rows carry (`TrackedAssetLock`'s `funding_type`,
/// `identity_index` and `account_index`), so a row is matched to its flow
/// by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FundingKey {
    pub funding_type: AssetLockFundingType,
    pub identity_index: u32,
    /// The row's `account_index`.
    pub account: u32,
}

/// `funding_marker.funding_type`'s codes. The journal outlives builds: a
/// code is never renumbered or reused.
const FUNDING_TYPES: [(AssetLockFundingType, u8); 6] = [
    (AssetLockFundingType::IdentityRegistration, 0),
    (AssetLockFundingType::IdentityTopUp, 1),
    (AssetLockFundingType::IdentityTopUpNotBound, 2),
    (AssetLockFundingType::IdentityInvitation, 3),
    (AssetLockFundingType::AssetLockAddressTopUp, 4),
    (AssetLockFundingType::AssetLockShieldedAddressTopUp, 5),
];

impl FundingKey {
    /// A tracked row's key.
    pub fn of_tracked(lock: &platform_wallet::TrackedAssetLock) -> Self {
        Self {
            funding_type: lock.funding_type,
            identity_index: lock.identity_index,
            account: lock.account_index,
        }
    }

    /// The funding type's stored code ([`FUNDING_TYPES`]). Exhaustive, so
    /// a new type fails to build until it has a code.
    pub(crate) fn type_code(t: AssetLockFundingType) -> u8 {
        use AssetLockFundingType as T;
        match t {
            T::IdentityRegistration => 0,
            T::IdentityTopUp => 1,
            T::IdentityTopUpNotBound => 2,
            T::IdentityInvitation => 3,
            T::AssetLockAddressTopUp => 4,
            T::AssetLockShieldedAddressTopUp => 5,
        }
    }

    pub(crate) fn type_of_code(code: u8) -> Option<AssetLockFundingType> {
        FUNDING_TYPES
            .iter()
            .find(|(_, c)| *c == code)
            .map(|(t, _)| *t)
    }

    pub(crate) fn stored(self) -> StoredFundingKey {
        StoredFundingKey {
            funding_type: Self::type_code(self.funding_type),
            identity_index: self.identity_index,
            account: self.account,
        }
    }

    /// `None` for a code this build does not know.
    pub(crate) fn from_stored(k: StoredFundingKey) -> Option<Self> {
        Some(Self {
            funding_type: Self::type_of_code(k.funding_type)?,
            identity_index: k.identity_index,
            account: k.account,
        })
    }
}

/// The step a Mode B call's marker records (§2a.3, §7.6). Keyed by flow
/// and step, never by an artifact the engine cannot name yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallStep {
    /// A resumable step, e.g. `withdrawal/<id>/submit`.
    Resumable(String),
    /// A funding call, `registration/<draft>/funding` or
    /// `topup/<id>/funding`, with its funding key.
    Funding { step: String, key: FundingKey },
}

impl CallStep {
    pub fn id(&self) -> &str {
        match self {
            Self::Resumable(step) | Self::Funding { step, .. } => step,
        }
    }

    pub fn funding(&self) -> Option<FundingKey> {
        match self {
            Self::Resumable(_) => None,
            Self::Funding { key, .. } => Some(*key),
        }
    }
}

/// The signatures released during one call permit (§2a.3): host-visible,
/// and it only grows.
#[derive(Debug, Default)]
pub struct SignatureCount {
    released: AtomicU64,
    locked: AtomicBool,
}

impl SignatureCount {
    pub fn released(&self) -> u64 {
        self.released.load(Ordering::SeqCst)
    }

    /// Whether a signer of the call refused `Locked`.
    pub fn locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    pub(crate) fn note_released(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn note_locked(&self) {
        self.locked.store(true, Ordering::SeqCst);
    }
}

/// What a library write call charges: its quote, against one purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallQuote {
    pub purpose: BudgetPurpose,
    pub amount: u64,
}

/// A funding call's quote (§2a.3, §16.2): the amount locked and the
/// wallet's `fee_bound_worst` ([`crate::NetworkSession::fee_bound_worst`]).
/// The permit refuses when their sum exceeds the Funding budget's
/// available amount, and charges the sum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FundingQuote {
    pub amount: u64,
    pub fee_bound_worst: u64,
}

impl From<FundingQuote> for CallQuote {
    fn from(q: FundingQuote) -> Self {
        Self {
            purpose: BudgetPurpose::Funding,
            // An overflowing sum exceeds every budget.
            amount: q.amount.saturating_add(q.fee_bound_worst),
        }
    }
}

/// A call running under a permit (in J).
#[derive(Debug)]
pub(crate) struct CallRun {
    pub(crate) lease: LeaseId,
    pub(crate) deadline: Instant,
    pub(crate) label: String,
}

/// A finished call of a lease, for the lock's report (§8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallRecord {
    pub(crate) label: String,
    pub(crate) outcome: FlowOutcome,
}

/// The RAII proof that a library call may run. Dropping it unfinished
/// counts as a failed call.
pub struct CallPermit {
    table: Arc<LeaseTable>,
    lease: Lease,
    id: u64,
    charge: Option<(BudgetPurpose, Charge)>,
    signatures: Arc<SignatureCount>,
    /// An earlier marker of the call's step exists (§2a.3).
    prior_marker: bool,
    /// The call step whose marker it holds ([`Lease::call_permit_step`]).
    step: Option<String>,
    /// The step is a funding step: its end refreshes the wallet's tracked
    /// rows (§2a.5).
    funding: bool,
    /// Handed back to the caller: the library call may have started.
    started: bool,
    deadline: Instant,
    done: bool,
}

/// A permit's J step: the call's id, its charge and its deadline.
type Took = (u64, Option<(BudgetPurpose, Charge)>, Instant);

/// What a call step's J step decided.
type Taken = (
    u64,
    Option<(BudgetPurpose, Charge)>,
    Instant,
    bool,
    Option<tokio::task::JoinHandle<bool>>,
);

impl std::fmt::Debug for CallPermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallPermit")
            .field("id", &self.id)
            .field("released", &self.signatures.released())
            .finish_non_exhaustive()
    }
}

impl Lease {
    /// Takes a call permit for `label` (the library call, or its step id),
    /// charging `quote` in the same J step. Refused when the lease does not
    /// sign: a lock or a revocation before the call means nothing started.
    pub fn call_permit(
        &self,
        label: impl Into<String>,
        quote: Option<CallQuote>,
    ) -> Result<CallPermit, LeaseError> {
        self.call_permit_with(label.into(), quote, false)
    }

    pub(crate) fn call_permit_with(
        &self,
        label: String,
        quote: Option<CallQuote>,
        prior_marker: bool,
    ) -> Result<CallPermit, LeaseError> {
        let (id, charge, deadline) = self
            .table
            .with_j(|i, fx| self.take_call(i, fx, label, quote))?;
        Ok(self.permit(id, charge, deadline, prior_marker, None))
    }

    /// Takes a call permit for a funding call or a resumable step's call
    /// (§2a.3 "Durable markers first"). In one J step: the checks and the
    /// charge of [`Self::call_permit`], whether an earlier marker of the
    /// step exists (`prior_marker`), and the step's marker set `Committing`
    /// with its durable write spawned. The permit comes back only once that
    /// write returned, so the library call starts after the marker is
    /// durable. A journal that is not ready refuses before charging; a
    /// write that failed, or came after the permit's deadline, refuses the
    /// call as never started (refunded), and the step reads MaybeSent until
    /// its `NotSent` row lands. Both are `JournalUnavailable` (`storage`).
    /// A lock, revocation or close that lands during the write refuses the
    /// call the same way, with the lease's own refusal. A step whose marker
    /// stands under another funding key (or none) is refused before the
    /// charge: `StepKeyMismatch`.
    pub async fn call_permit_step(
        &self,
        label: impl Into<String>,
        quote: Option<CallQuote>,
        step: CallStep,
    ) -> Result<CallPermit, LeaseError> {
        let label = label.into();
        let purpose = quote.map_or(BudgetPurpose::Crypto, |q| q.purpose);
        let table = &self.table;
        let key = (self.wallet, step.id().to_owned());
        let mut retry = true;
        let (id, charge, deadline, prior, write) = loop {
            let backend = table.journal.get();
            let taken = table.with_j(|i, fx| -> Result<Result<Taken, CallGate>, LeaseError> {
                if let Some(gate) = table.call_step_gate(i, &key, backend.is_some(), retry)? {
                    return Ok(Err(gate));
                }
                if i.fence.call_key_differs(self.wallet, &step) {
                    return Err(LeaseError::StepKeyMismatch);
                }
                if step.funding().is_some() {
                    self.check_funding(i, quote, step.id())?;
                }
                let backend = backend.expect("checked by the gate");
                let (id, charge, deadline) = self.take_call(i, fx, label.clone(), quote)?;
                let (prior, write) = table.call_step_mark(i, backend, self.wallet, &step);
                if let Some(fkey) = step.funding() {
                    super::mode_b::note_marked(i, self.wallet, step.id(), fkey);
                    // `funds_committed` from this J step on (§16.5).
                    if let Some(e) = i.leases.get_mut(&self.id)
                        && !e.funding_steps.iter().any(|s| s == step.id())
                    {
                        e.funding_steps.push(step.id().to_owned());
                    }
                    fx.changed(self.id);
                    super::mode_b::rederive(table, i, fx, self.wallet);
                }
                Ok(Ok((id, charge, deadline, prior, write)))
            })?;
            match taken {
                Ok(taken) => break taken,
                Err(CallGate::Wait) => {
                    table
                        .wait_until(|i| (!i.fence.call_resolving(&key)).then_some(()))
                        .await;
                }
                Err(CallGate::Retry) => {
                    retry = false;
                    table.retry_call_not_sent(key.0, key.1.clone()).await;
                }
            }
        };
        let mut permit = self.permit(id, charge, deadline, prior, Some(key.1));
        permit.funding = step.funding().is_some();
        // Before the write returns the permit is not handed out: dropped
        // here, it ends as a call that never started.
        permit.started = false;
        #[cfg(test)]
        if table.inspect(|i| i.mutation) == Some(Mutation::CallBeforeMarker) {
            // The mutation: the call may start before its marker is durable.
            drop(write);
            permit.started = true;
            return Ok(permit);
        }
        let durable = match write {
            Some(write) => write.await.unwrap_or(false),
            None => true,
        };
        let refusal = if !durable || Instant::now() >= permit.deadline {
            Some(LeaseError::JournalUnavailable)
        } else {
            // A lock, revocation or close during the write: the call never
            // started (the lock's drain waits for it until it ends here).
            table.with_j(|i, _| self.refusal(i, purpose))
        };
        match refusal {
            None => {
                permit.started = true;
                Ok(permit)
            }
            Some(e) => {
                permit.done = true;
                permit.end(false);
                Err(e)
            }
        }
    }

    /// A funding call's checks (§2a.3), before its charge: the lease signs,
    /// the quote is a Funding quote ([`FundingQuote`]), and the Mode B gate
    /// passes. The budget check itself is the charge's.
    fn check_funding(
        &self,
        i: &Inner,
        quote: Option<CallQuote>,
        step: &str,
    ) -> Result<(), LeaseError> {
        if let Some(refused) = self.refusal(i, BudgetPurpose::Funding) {
            return Err(refused);
        }
        if quote.map(|q| q.purpose) != Some(BudgetPurpose::Funding) {
            return Err(LeaseError::NeedsGrant(BudgetPurpose::Funding));
        }
        match funding_gate(i, self.wallet, step) {
            Some(block) => Err(LeaseError::FundingBlocked(block)),
            None => Ok(()),
        }
    }

    /// Why the lease would not sign a call now, if it would not.
    fn refusal(&self, i: &Inner, purpose: BudgetPurpose) -> Option<LeaseError> {
        if i.closed || i.barrier.set_for(&self.wallet) {
            return Some(LeaseError::Locked);
        }
        match i.leases.get(&self.id) {
            None => Some(LeaseError::Invalid),
            Some(e) if !e.state.signs() => Some(e.state.refusal(purpose)),
            Some(_) => None,
        }
    }

    /// A call permit's J step: the lease checks and the charge (§2a.3).
    fn take_call(
        &self,
        i: &mut Inner,
        fx: &mut Effects,
        label: String,
        quote: Option<CallQuote>,
    ) -> Result<Took, LeaseError> {
        let ttl = self.table.config.permit_ttl;
        let purpose = quote.map_or(BudgetPurpose::Crypto, |q| q.purpose);
        if let Some(refused) = self.refusal(i, purpose) {
            return Err(refused);
        }
        let e = i.leases.get_mut(&self.id).ok_or(LeaseError::Invalid)?;
        let charge = match quote {
            Some(q) => {
                let budget = e
                    .budget_mut(q.purpose)
                    .filter(|b| b.granted())
                    .ok_or(LeaseError::NeedsGrant(q.purpose))?;
                let c = budget.charge(q.amount).map_err(|x| LeaseError::Exceeded {
                    purpose: q.purpose,
                    needed: x.needed,
                    remaining: x.remaining,
                })?;
                Some((q.purpose, c))
            }
            None => None,
        };
        let now = Instant::now();
        e.calls += 1;
        e.last_use = now;
        fx.changed(self.id);
        i.next_call += 1;
        let deadline = now + ttl;
        i.calls.insert(
            i.next_call,
            CallRun {
                lease: self.id,
                deadline,
                label,
            },
        );
        fx.notify();
        Ok((i.next_call, charge, deadline))
    }

    fn permit(
        &self,
        id: u64,
        charge: Option<(BudgetPurpose, Charge)>,
        deadline: Instant,
        prior_marker: bool,
        step: Option<String>,
    ) -> CallPermit {
        CallPermit {
            table: Arc::clone(&self.table),
            lease: self.clone(),
            id,
            charge,
            signatures: Arc::default(),
            prior_marker,
            step,
            funding: false,
            started: true,
            deadline,
            done: false,
        }
    }
}

impl CallPermit {
    /// The count the call's signers report to.
    pub fn signatures(&self) -> Arc<SignatureCount> {
        Arc::clone(&self.signatures)
    }

    /// The grant + H: past it, a lock's drain stops waiting for the call.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn lease(&self) -> &Lease {
        &self.lease
    }

    /// The lease's identity signer, counting into this permit. Issued while
    /// the lease signs, even with the Credits budget at 0: the permit's J
    /// step charged the call's quote already.
    pub fn identity_signer(
        &self,
        identity_indices: &[u32],
    ) -> Result<VaultIdentitySigner, LeaseError> {
        Ok(self
            .lease
            .call_identity_signer(identity_indices)?
            .counting(self.signatures()))
    }

    /// The lease's funding signer for `create_funded_asset_lock_proof`,
    /// counting into this permit: every released signature and a `Locked`
    /// refusal, as [`Self::identity_signer`]'s.
    pub fn funding_signer(&self) -> Result<CountingFundingSigner, LeaseError> {
        Ok(CountingFundingSigner::new(
            self.lease.call_funding_signer()?,
            self.signatures(),
        ))
    }

    /// Ends the call: `ok` when the library returned Ok. Returns the call's
    /// outcome; a definite `Cancelled` refunds the quote. A call step's
    /// definite resolution is written before it returns: `Sent` begins the
    /// step's `Sent` row, and a `Cancelled` that leaves no call of the step
    /// possibly out appends its `NotSent` row (`block_in_place` on a
    /// multi-thread worker, inline elsewhere). MaybeSent leaves the marker
    /// standing.
    pub fn finish(mut self, ok: bool) -> FlowOutcome {
        self.done = true;
        self.end(ok)
    }

    fn end(&mut self, ok: bool) -> FlowOutcome {
        let released = self.signatures.released();
        let locked = self.signatures.locked();
        // A call never handed out never started: nothing of it was sent.
        let outcome = if ok {
            FlowOutcome::Sent
        } else if !self.started || (released == 0 && !self.prior_marker) {
            FlowOutcome::Cancelled
        } else {
            FlowOutcome::MaybeSent
        };
        let (id, lease, wallet) = (self.id, self.lease.id, self.lease.wallet);
        let charge = self.charge.take();
        let step = self.step.clone();
        // Read outside J (H1).
        let backend = step.as_ref().and_then(|_| self.table.journal.get());
        let table = Arc::clone(&self.table);
        let resolve = table.with_j(|i, fx| {
            let run = i.calls.remove(&id);
            if let Some(e) = i.leases.get_mut(&lease) {
                e.calls = e.calls.saturating_sub(1);
                e.last_use = Instant::now();
                if let Some(run) = run {
                    e.calls_done.push(CallRecord {
                        label: run.label,
                        outcome,
                    });
                }
                if outcome == FlowOutcome::Cancelled
                    && let Some((purpose, charge)) = charge
                {
                    e.refund(purpose, charge);
                }
                if locked {
                    e.park(fx);
                }
                fx.changed(lease);
            }
            fx.notify();
            let resolve = step.as_deref().is_some_and(|s| {
                table.call_step_end(i, fx, backend, wallet, (s, self.funding), outcome)
            });
            if step.is_some() {
                // The call's end: `Sent` on Ok (§2a.5).
                super::mode_b::rederive(&table, i, fx, wallet);
            }
            resolve
        });
        if resolve && let Some(step) = &self.step {
            table.resolve_call_not_sent(wallet, step);
        }
        if self.funding {
            // The call may have left a tracked row.
            table.request_refresh(wallet);
        }
        outcome
    }
}

impl Drop for CallPermit {
    fn drop(&mut self) {
        if !self.done {
            self.end(false);
        }
    }
}
