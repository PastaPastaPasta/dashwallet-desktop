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

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use tokio::time::Instant;

use super::budget::Charge;
use super::table::{Lease, LeaseTable};
use super::{FlowOutcome, LeaseError, LeaseId};
use crate::platform::flows::BudgetPurpose;
use crate::platform::signers::VaultIdentitySigner;

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
    deadline: Instant,
    done: bool,
}

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
        let table = &self.table;
        let ttl = table.config.permit_ttl;
        let (id, charge, deadline) = table.with_j(|i, fx| {
            let purpose = quote.map_or(BudgetPurpose::Crypto, |q| q.purpose);
            if i.closed || i.barrier.set_for(&self.wallet) {
                return Err(LeaseError::Locked);
            }
            let e = i.leases.get_mut(&self.id).ok_or(LeaseError::Invalid)?;
            if !e.state.signs() {
                return Err(e.state.refusal(purpose));
            }
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
        })?;
        Ok(CallPermit {
            table: Arc::clone(table),
            lease: self.clone(),
            id,
            charge,
            signatures: Arc::default(),
            prior_marker,
            deadline,
            done: false,
        })
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

    /// The lease's identity signer, counting into this permit.
    pub fn identity_signer(
        &self,
        identity_indices: &[u32],
    ) -> Result<VaultIdentitySigner, LeaseError> {
        Ok(self
            .lease
            .identity_signer(identity_indices)?
            .counting(self.signatures()))
    }

    /// Ends the call: `ok` when the library returned Ok. Returns the call's
    /// outcome; a definite `Cancelled` refunds the quote.
    pub fn finish(mut self, ok: bool) -> FlowOutcome {
        self.done = true;
        self.end(ok)
    }

    fn end(&mut self, ok: bool) -> FlowOutcome {
        let released = self.signatures.released();
        let locked = self.signatures.locked();
        let outcome = if ok {
            FlowOutcome::Sent
        } else if released == 0 && !self.prior_marker {
            FlowOutcome::Cancelled
        } else {
            FlowOutcome::MaybeSent
        };
        let (id, lease) = (self.id, self.lease.id);
        let charge = self.charge.take();
        self.table.with_j(|i, fx| {
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
        });
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
