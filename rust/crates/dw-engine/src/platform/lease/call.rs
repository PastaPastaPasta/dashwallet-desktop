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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dw_appdb::dispatch::StoredFundingKey;
use dw_vault::SignerError;
use platform_wallet::AssetLockFundingType;
use tokio::time::Instant;

use super::budget::Charge;
use super::fence::CallGate;
use super::mode_b::funding_gate;
#[cfg(test)]
use super::stress_tests::Mutation;
use super::table::{Effects, Inner, Lease, LeaseTable};
use super::{FlowOutcome, LeaseError, LeaseId};
use crate::WalletId;
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
///
/// Only the permit's own signers count ([`CallPermit::identity_signer`],
/// [`CallPermit::funding_signer`]); `Lease::identity_signer` and
/// `Lease::funding_signer` do not, and no Mode B library call may use them.
///
/// **A signer cannot outlive its permit** (review M-1). A counting signer
/// may be cloned and kept past the permit's end, so the count closes when
/// the permit ends ([`Self::close`]), and every signature is counted before
/// the vault is asked for it: the signer first adds it to `signing`, then
/// reads `closed`, and refuses (`Locked`) when the permit has ended; the
/// permit's end sets `closed` first, then reads `signing` and `released`
/// (all `SeqCst`). So either the signer sees `closed` and asks the vault
/// for nothing, or the end sees the signature, released or still being
/// signed, and the call reads `MaybeSent`. A signing the vault refused is
/// taken back out of `signing`: no signature exists, so it never turns a
/// `Cancelled` into a `MaybeSent`. A refusal after the close leaves its
/// count standing (the end may have read it: fail closed).
///
/// A funding signer also keeps the sighash of each signature it released
/// ([`Signing::released_with`]), so the engine can tell exactly which
/// transaction the call signed (§2a.6, [`Self::signed_all`]).
#[derive(Debug, Default)]
pub struct SignatureCount {
    released: AtomicU64,
    /// The sighashes the funding signer released a signature over.
    signed: Mutex<Vec<[u8; 32]>>,
    /// Signatures asked of the vault and not yet returned.
    signing: AtomicU64,
    locked: AtomicBool,
    /// The permit has ended: its signers sign nothing more.
    closed: AtomicBool,
    /// A lock the call signed was recorded: its funding signer signs no
    /// other sighash ([`Self::admits`]).
    recorded: AtomicBool,
}

/// One signature asked of the vault under a [`SignatureCount`] (counted in
/// `signing`). [`Self::released`] when the vault signed; dropped
/// otherwise, which takes it back out.
#[must_use]
pub(crate) struct Signing<'a>(&'a SignatureCount);

impl Signing<'_> {
    /// The vault signed: counted released (before it leaves `signing`, so
    /// a reader of `signing` then `released` never misses it).
    pub(crate) fn released(self) {
        self.0.released.fetch_add(1, Ordering::SeqCst);
    }

    /// [`Self::released`], keeping the `sighash` signed: it is kept before
    /// it is counted, so a reader that sees it counted sees it kept too.
    pub(crate) fn released_with(self, sighash: [u8; 32]) {
        self.0
            .signed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(sighash);
        self.released();
    }
}

impl Drop for Signing<'_> {
    fn drop(&mut self) {
        self.0.signing.fetch_sub(1, Ordering::SeqCst);
    }
}

impl SignatureCount {
    pub fn released(&self) -> u64 {
        self.released.load(Ordering::SeqCst)
    }

    /// Whether a signer of the call refused `Locked`.
    pub fn locked(&self) -> bool {
        self.locked.load(Ordering::SeqCst)
    }

    /// Whether the permit has ended: its signers sign nothing more.
    pub(crate) fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// A released signature, as a counting signer reports one (lease's
    /// tests; a funding call's goes through [`CallPermit::note_released`],
    /// which records its release first).
    #[cfg(test)]
    pub(super) fn note_released(&self) {
        self.released.fetch_add(1, Ordering::SeqCst);
    }

    /// A funding signature released over `sighash`, as the funding signer
    /// reports one (lease's tests; see [`CallPermit::note_signed`]).
    #[cfg(test)]
    pub(super) fn note_signed(&self, sighash: [u8; 32]) {
        self.signed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(sighash);
        self.note_released();
    }

    /// Whether the call's funding signer released a signature over one of
    /// `sighashes` (§2a.7): a transaction carrying it may be the call's.
    pub(crate) fn signed_any(&self, sighashes: &[[u8; 32]]) -> bool {
        let signed = self.signed.lock().unwrap_or_else(|p| p.into_inner());
        sighashes.iter().any(|s| signed.contains(s))
    }

    /// Whether the call's funding signer released a signature over every
    /// one of `sighashes`, and there is one (§2a.6). A transaction each of
    /// whose inputs carries a valid signature over its own sighash, every
    /// one released by the call, is the call's own: a `SIGHASH_ALL` sighash
    /// commits to all the transaction's inputs and outputs, so only the
    /// scriptSigs could differ, and they are what was released.
    pub(crate) fn signed_all(&self, sighashes: &[[u8; 32]]) -> bool {
        let signed = self.signed.lock().unwrap_or_else(|p| p.into_inner());
        !sighashes.is_empty() && sighashes.iter().all(|s| signed.contains(s))
    }

    /// Whether the call's funding signer released signatures over exactly
    /// `sighashes`, and no other, with nothing still being signed (review
    /// P2b r6 R6-02): the lock carrying them is the one lock the call
    /// signed. Read after [`Self::note_recorded`], so a signature it misses
    /// is refused ([`Self::admits`]), or still counted in `signing` here.
    pub(crate) fn signed_only(&self, sighashes: &[[u8; 32]]) -> bool {
        if self.signing.load(Ordering::SeqCst) > 0 {
            return false;
        }
        let signed = self.signed.lock().unwrap_or_else(|p| p.into_inner());
        !sighashes.is_empty()
            && sighashes.iter().all(|s| signed.contains(s))
            && signed.iter().all(|s| sighashes.contains(s))
    }

    pub(crate) fn note_locked(&self) {
        self.locked.store(true, Ordering::SeqCst);
    }

    /// Whether the call released a signature and no lock it signed was
    /// recorded (review P2b r7): nothing covers its signatures but its
    /// release row.
    pub(crate) fn uncovered(&self) -> bool {
        self.released() > 0 && !self.recorded.load(Ordering::SeqCst)
    }

    /// A lock the call signed was recorded as its step's (§2a.6, O-1).
    pub(crate) fn note_recorded(&self) {
        self.recorded.store(true, Ordering::SeqCst);
    }

    /// Whether the call's funding signer may sign `sighash`: one lock per
    /// call (O-1). Once a lock the call signed was recorded, which is after
    /// every input of it was signed and before its send, only a sighash
    /// the call already signed is (an identical retry): a second lock's
    /// signatures are refused. One signed before the record (two locks
    /// built under one permit) stops the process at the record instead
    /// (review P2b r6 R6-02, [`Self::signed_only`]): the signer sees bare
    /// sighashes, so it cannot tell two locks apart sooner. The premise
    /// that Repair's self-spend need take an input of each recorded lock
    /// only rests on it.
    pub(crate) fn admits(&self, sighash: [u8; 32]) -> bool {
        !self.recorded.load(Ordering::SeqCst) || self.signed_any(&[sighash])
    }

    /// A signer's error under this count: a `Locked` refusal is reported
    /// ([`Self::locked`]). Touches neither `signing` nor `closed`.
    pub(crate) fn observe_err(&self, e: &SignerError) {
        if matches!(e, SignerError::Locked) {
            self.note_locked();
        }
    }

    /// A signer is about to ask the vault for one signature: counted
    /// first, then refused (`None`) when the permit has ended (type doc).
    pub(crate) fn begin(&self) -> Option<Signing<'_>> {
        self.signing.fetch_add(1, Ordering::SeqCst);
        if self.closed.load(Ordering::SeqCst) {
            // Left counted: the end may have read it (fail closed).
            return None;
        }
        Some(Signing(self))
    }

    /// The permit's end: no signer signs from here on. Returns the
    /// signatures that were released or may still be (type doc).
    pub(crate) fn close(&self) -> u64 {
        self.closed.store(true, Ordering::SeqCst);
        // `signing` first: a signing that left it was counted released
        // before.
        let signing = self.signing.load(Ordering::SeqCst);
        signing.saturating_add(self.released.load(Ordering::SeqCst))
    }
}

/// The `funding_release` row of one funding call's signer (§2a.7, review P2b
/// r6 R6-01): written once, before its first signature, shared by its
/// clones (a concurrent first signature waits for the one write).
pub(crate) struct FundingRelease {
    table: Arc<LeaseTable>,
    wallet: WalletId,
    step: String,
    done: tokio::sync::OnceCell<()>,
}

impl std::fmt::Debug for FundingRelease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FundingRelease")
            .field("step", &self.step)
            .field("recorded", &self.done.initialized())
            .finish_non_exhaustive()
    }
}

impl FundingRelease {
    /// The release is durable: written now, or by an earlier signature. A
    /// failed write is tried again by the next signature.
    pub(crate) async fn record(&self) -> Result<(), String> {
        self.done
            .get_or_try_init(|| self.table.record_release(self.wallet, &self.step))
            .await
            .map(|_| ())
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
    /// A funding call's step, key and signature count ([`CallFunding`]).
    pub(crate) funding: Option<CallFunding>,
    /// A funding call's coin fence (`coins.rs`, E0-04 §2a.6): the one its
    /// caller's policy check read the coins under. Released, outside J, at
    /// the first of: the changeset tap reporting a tracked row of the wallet
    /// under the call's key, of any status, that is not foreign to its
    /// step, once the call released a signature (the library tracks and
    /// persists its lock `Built` after signing it and before broadcasting
    /// it, and from then on the lock's inputs read reserved; the persister
    /// may fold that `Built` into a later status before the tap sees it, so
    /// any status counts; a row before the call's first signature is not
    /// its lock); or the call's end. Never held through the library's proof
    /// wait past that row.
    pub(crate) fence: Option<crate::coins::CoinFence>,
}

/// What the changeset tap matches a funding call's tracked rows by
/// ([`LeaseTable::note_tracked`]): a row it matches for no other running
/// call is the step's own lock (review P2b r1 F1).
#[derive(Debug)]
pub(crate) struct CallFunding {
    pub(crate) wallet: WalletId,
    pub(crate) step: String,
    pub(crate) key: FundingKey,
    /// The call's own count ([`CallPermit::signatures`]).
    pub(crate) signatures: Arc<SignatureCount>,
}

/// A finished call of a lease, for the lock's report (§8.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallRecord {
    pub(crate) label: String,
    pub(crate) outcome: FlowOutcome,
    /// A funding call's step: the lock's report reads the step's derived
    /// status instead of this outcome (review R1-02).
    pub(crate) funding_step: Option<String>,
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
    step: Option<CallStep>,
    /// Handed back to the caller: the library call may have started.
    started: bool,
    deadline: Instant,
    done: bool,
}

/// A permit's J step: the call's id, its charge and its deadline.
struct Took {
    id: u64,
    charge: Option<(BudgetPurpose, Charge)>,
    deadline: Instant,
}

/// What a call step's J step decided: the permit's [`Took`], whether an
/// earlier marker of the step exists, and the marker's write to await.
struct Taken {
    took: Took,
    prior: bool,
    write: Option<tokio::task::JoinHandle<bool>>,
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
        let took = self
            .table
            .with_j(|i, fx| self.take_call(i, fx, label.into(), quote))?;
        Ok(self.permit(took, false, None))
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
    ///
    /// A funding step's step id and key are not checked here, nor is its
    /// coin fence taken: a Mode B funding call takes its permit through
    /// [`crate::NetworkSession::funding_call_permit`], which refuses a
    /// step that is no registration funding step, or a key that is not
    /// its draft's own (reviews M-3, M-4), and so this is crate-private
    /// (review Opus O-11).
    pub(crate) async fn call_permit_step(
        &self,
        label: impl Into<String>,
        quote: Option<CallQuote>,
        step: CallStep,
    ) -> Result<CallPermit, LeaseError> {
        self.call_permit_step_fenced(label, quote, step, None).await
    }

    /// [`Self::call_permit_step`] for a funding call that holds `fence`,
    /// its wallet's coin fence: the permit's J step moves it into the
    /// call, which keeps it until the library tracked its lock or the call
    /// ends ([`FundingFence`]). Refused, it is dropped with the refusal.
    pub(crate) async fn call_permit_step_fenced(
        &self,
        label: impl Into<String>,
        quote: Option<CallQuote>,
        step: CallStep,
        fence: Option<crate::coins::CoinFence>,
    ) -> Result<CallPermit, LeaseError> {
        let mut fence = fence;
        let signatures = Arc::<SignatureCount>::default();
        let label = label.into();
        let purpose = quote.map_or(BudgetPurpose::Crypto, |q| q.purpose);
        let table = &self.table;
        let key = (self.wallet, step.id().to_owned());
        let mut retry = true;
        let Taken { took, prior, write } = loop {
            let backend = table.journal.get();
            let attempt = table.with_j(|i, fx| -> Result<Result<Taken, CallGate>, LeaseError> {
                if let Some(gate) = table.call_step_gate(i, &key, backend.is_some(), retry)? {
                    return Ok(Err(gate));
                }
                // The funding gate first: a possibly-sent marker under
                // another key reads as that step's status, which the host
                // can act on (review Opus O-11).
                if step.funding().is_some() {
                    self.check_funding(i, quote, step.id())?;
                }
                if i.fence.call_key_differs(self.wallet, &step) {
                    return Err(LeaseError::StepKeyMismatch);
                }
                let backend = backend.expect("checked by the gate");
                let took = self.take_call(i, fx, label.clone(), quote)?;
                let run = i.calls.get_mut(&took.id);
                match (step.funding(), run) {
                    (Some(key), Some(run)) => {
                        run.funding = Some(CallFunding {
                            wallet: self.wallet,
                            step: step.id().to_owned(),
                            key,
                            signatures: Arc::clone(&signatures),
                        });
                        run.fence = fence.take();
                    }
                    // Only a funding call holds one.
                    _ => {
                        if let Some(fence) = fence.take() {
                            fx.drop_later(fence);
                        }
                    }
                }
                let (prior, write) = table.call_step_mark(i, backend, self.wallet, &step);
                if let Some(fkey) = step.funding() {
                    super::mode_b::note_marked(i, self.wallet, step.id(), fkey);
                    // `funds_committed` from this J step on (§16.5).
                    self.bind_funding_step_in(i, fx, step.id());
                }
                Ok(Ok(Taken { took, prior, write }))
            })?;
            match attempt {
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
        // Before the write returns the permit is not handed out: dropped
        // here, it ends as a call that never started.
        let mut permit = self.permit(took, prior, Some(step));
        permit.signatures = signatures;
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
                funding: None,
                fence: None,
            },
        );
        fx.notify();
        Ok(Took {
            id: i.next_call,
            charge,
            deadline,
        })
    }

    /// The permit of `took`. A call step's permit is not handed out before
    /// its marker's write returned: it starts unstarted.
    fn permit(&self, took: Took, prior_marker: bool, step: Option<CallStep>) -> CallPermit {
        CallPermit {
            table: Arc::clone(&self.table),
            lease: self.clone(),
            id: took.id,
            charge: took.charge,
            signatures: Arc::default(),
            prior_marker,
            started: step.is_none(),
            step,
            deadline: took.deadline,
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
    /// step charged the call's quote already. It signs nothing once the
    /// permit ended, clones included ([`SignatureCount`]).
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
    /// refusal, as [`Self::identity_signer`]'s, and nothing signed once the
    /// permit ended. It signs one asset lock: once a lock it signed is
    /// recorded, it refuses any sighash it did not sign before (review Opus
    /// O-1), so it is no signer for the identity transition the library
    /// signs after the lock (`registration.rs`'s one-call path, which Mode B
    /// does not take): that one needs [`Self::identity_signer`].
    ///
    /// Before the vault releases its first signature, it records the
    /// release against the step's standing marker, durably
    /// ([`LeaseTable::record_release`], schema 7, review P2b r6 R6-01), and
    /// refuses when that fails: no funding signature leaves the vault, by
    /// whatever path it then reaches a broadcast, while its step could still
    /// read `NotSent`. Refused `StepKeyMismatch` on a permit of no funding
    /// step (no marker to record against).
    pub fn funding_signer(&self) -> Result<CountingFundingSigner, LeaseError> {
        let Some(step) = self.step.as_ref().filter(|s| s.funding().is_some()) else {
            return Err(LeaseError::StepKeyMismatch);
        };
        let release = Arc::new(FundingRelease {
            table: Arc::clone(&self.table),
            wallet: self.lease.wallet,
            step: step.id().to_owned(),
            done: tokio::sync::OnceCell::new(),
        });
        Ok(CountingFundingSigner::new(
            self.lease.call_funding_signer()?,
            self.signatures(),
            release,
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

    /// A released signature, as the permit's funding signer reports one
    /// (tests): under a funding step, its release recorded first.
    #[cfg(test)]
    pub(crate) fn note_released(&self) {
        self.note_release();
        self.signatures.note_released();
    }

    /// A funding signature released over `sighash`, as the permit's funding
    /// signer reports one (tests): its release recorded first.
    #[cfg(test)]
    pub(crate) fn note_signed(&self, sighash: [u8; 32]) {
        self.note_release();
        self.signatures.note_signed(sighash);
    }

    #[cfg(test)]
    fn note_release(&self) {
        if let Some(step) = self.step.as_ref().filter(|s| s.funding().is_some()) {
            (self.table.record_release_now(self.lease.wallet, step.id()))
                .expect("the funding release is recorded");
        }
    }

    /// The call's step is a funding step: its end refreshes the wallet's
    /// tracked rows (§2a.5).
    fn funding(&self) -> bool {
        self.step.as_ref().is_some_and(|s| s.funding().is_some())
    }

    fn end(&mut self, ok: bool) -> FlowOutcome {
        // Closed before it is read: a signer used after this signs nothing
        // (`SignatureCount`).
        let released = self.signatures.close();
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
        let funding = self.funding();
        // Read outside J (H1).
        let backend = step.as_ref().and_then(|_| self.table.journal.get());
        let table = Arc::clone(&self.table);
        let (resolve, resume) = table.with_j(|i, fx| {
            let mut run = i.calls.remove(&id);
            // Released outside J (J stays a leaf).
            if let Some(f) = run.as_mut().and_then(|r| r.fence.take()) {
                fx.drop_later(f);
            }
            // Its signer's locks stay recognised (§2a.7).
            if released > 0
                && let Some(f) = run.as_mut().and_then(|r| r.funding.take())
            {
                i.fence.ended_funding.push(f);
            }
            // A pass deferred a lock of the wallet's call: its record is
            // durable now, or the process stopped.
            let resume = funding && i.mode_b.passes.take_deferred(&wallet);
            if let Some(e) = i.leases.get_mut(&lease) {
                e.calls = e.calls.saturating_sub(1);
                e.last_use = Instant::now();
                if let Some(run) = run {
                    e.calls_done.push(CallRecord {
                        label: run.label,
                        outcome,
                        funding_step: step
                            .as_ref()
                            .filter(|s| s.funding().is_some())
                            .map(|s| s.id().to_owned()),
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
            let resolve = step
                .as_ref()
                .is_some_and(|s| table.call_step_end(i, fx, backend, wallet, s, outcome));
            if step.is_some() {
                // The call's end: `Sent` on Ok (§2a.5).
                super::mode_b::rederive(&table, i, fx, wallet);
            }
            (resolve, resume)
        });
        if resolve && let Some(step) = &step {
            table.resolve_call_not_sent(wallet, step.id());
        }
        if funding {
            // The call may have left a tracked row.
            table.request_refresh(wallet);
        }
        if resume {
            table.request_resume(Some(wallet));
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
