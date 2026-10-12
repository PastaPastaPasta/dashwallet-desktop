//! The registration state machine (DASHPAY §3.4, ROADMAP DP1-02): one
//! `dp_registration` row per flow, advanced one step at a time. Not part of
//! the facade; `registration.rs` calls it.
//!
//! **Idempotent steps.** Every step first asks what an earlier run (in this
//! process, before a kill, or on the machine a backup came from) already
//! did, and adopts it instead of acting again:
//! - an identity at the row's index, or the identity whose id the asset
//!   lock's outpoint derives (Platform derives it the same way);
//! - a tracked asset lock matching the draft's own funding key
//!   (`IdentityRegistration`, the identity index, account 0), whatever its
//!   status, so a lock is never funded twice for one index;
//! - the names: DP1-03's `register_name` asks Platform first.
//!
//! So a kill at any transition leaves a row that ends registered or
//! resumable, and funds are committed only from `FundingSent` on.
//!
//! **The library behind a seam.** [`RegistrationLib`] is what a flow asks of
//! platform-wallet; [`PwLib`] asks it, and the tests script a fake (the
//! DP1-03 `NameNet` pattern).
//!
//! **On E0-04.P2b (Mode B).** The funding call holds P2b's funding permit
//! (`funding_call_permit`): its marker (`registration/<draft>/funding`) is
//! durable before the library starts, the permit charges the funding cap,
//! keeps the wallet's coin fence until the library tracks the lock, and
//! refuses a step that may be out already. The call signs through the
//! permit's funding signer, so P2b records the lock it signed as the step's
//! own (`signed_lock`) before the persister takes it: the one exact
//! provenance a row adopts a lock by, besides its index and the call's own
//! return (Sol r6 R6-01, R6-03). A failed call is read from the tracked
//! rows: a tracked row of the row's is a committed lock; with none, the
//! step's status decides: a call that signed nothing is `NotSent` and the
//! row is retryable again, any other is `MaybeSent`, and the row waits (or
//! fails `platform.broadcast_unknown`) until the lock is tracked again or
//! positive evidence says `NotSent`. The identity step runs under its own
//! resumable step permit (its call in `create_identity`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dashcore::OutPoint;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{
    AppDb, AppDbError, LockOutpoint, NewRegistration, RegistrationChange, RegistrationRow,
    RegistrationUpdate,
};
use dw_vault::LockState;
use tokio::time::Instant;

use super::errors::{NameError, PlatformError, RegistrationError};
use super::flows::{BudgetPurpose, LeaseStateView};
use super::identity_writes::identity_locks;
use super::lease::mode_b::TrackedRow;
use super::lease::{CallPermit, CallStep, DraftKey, FundingKey, Lease, LeaseTable};
use super::names::NameOutcome;
use super::registration::RegistrationWait;
use super::registration_ledger::{
    IndexOwner, Ledger, Owner, Reconcile, Standing, StepClaim, Taker,
};
use super::registration_rows::{Decoded, Funding, Phase, ProfileRecord};
use crate::coins::CoinFence;
use crate::{NetworkSession, WalletId};

/// The asset lock of a registration with a plain name: 0.03 DASH, as the
/// iOS wallet locks (`DWDashPayConstants.m`).
pub(crate) const LOCK_DUFFS: u64 = 3_000_000;
/// With a contested name: 0.25 DASH (iOS), which covers the contest fund.
pub(crate) const CONTESTED_LOCK_DUFFS: u64 = 25_000_000;
/// One duff locked is this many credits on Platform.
pub(crate) const CREDITS_PER_DUFF: u64 = 1_000;
/// The IdentityCreate fee bound until DP1-06's cost table: the pin's base
/// cost (2 000 000 credits, rs-platform-version
/// `state_transition_min_fees/v1.rs`) plus the six keys stored, bounded
/// at 10 KiB like `flows.rs`'s documents (27 400 credits a byte).
pub(crate) const IDENTITY_CREATE_CREDITS: u64 = 300_000_000;
/// The profile document's bound (`flows.rs` `UpdateProfile`).
pub(crate) const PROFILE_CREDITS: u64 = 300_000_000;
/// The asset-lock fee rate at the pin (platform-wallet
/// `asset_lock/manager.rs` `DEFAULT_FEE_PER_KB`), duffs per 1000 bytes.
const FEE_PER_KB: u64 = 1_000;
/// A P2PKH input, signed, at most.
const INPUT_BYTES: u64 = 149;
/// Version, counts, lock time, the change and credit outputs and the
/// asset-lock payload, generously.
const FIXED_BYTES: u64 = 300;
/// The library's own InstantSend wait in `create_funded_asset_lock_proof`
/// (`build.rs:1265-1300`): the in-memory key window (§2.6).
pub(crate) const IS_WINDOW: Duration = Duration::from_secs(300);
/// One bounded proof wait of the ChainLock wait, which has no limit: the
/// library's unbounded wait gives up after 180 s with a do-not-retry error
/// (`resume_asset_lock`), a bounded one returns `FinalityTimeout` and the
/// flow waits again.
pub(crate) const CHAINLOCK_POLL: Duration = Duration::from_secs(120);
/// "Finish transfers" waits for no proof, but a `Built` lock is resumed
/// this long so the library broadcasts it again.
pub(crate) const REBROADCAST_WAIT: Duration = Duration::from_secs(5);
/// The pause before a run that waits for the sync, or for the network,
/// steps again (the run keeps its lease, §2.6's one prompt).
const SYNC_POLL: Duration = Duration::from_secs(5);
const NETWORK_RETRY_MIN: Duration = Duration::from_secs(2);
/// A lock's outpoint is written this many times before the run gives up
/// with the storage error, backing off by this much more each time.
const LOCK_WRITE_TRIES: u32 = 3;
const LOCK_WRITE_RETRY: Duration = Duration::from_millis(100);
const NETWORK_RETRY_MAX: Duration = Duration::from_secs(60);

/// Mode B's worst-case funding fee (E0-04 §2a.3): spending every
/// spendable coin of the funding sources, a true upper bound on what the
/// library's coin selection can pay.
pub(crate) fn fee_bound_worst(inputs: usize) -> u64 {
    let bytes = FIXED_BYTES.saturating_add(INPUT_BYTES.saturating_mul(inputs as u64));
    bytes.saturating_mul(FEE_PER_KB).div_ceil(1000)
}

/// The funding step id (E0-04 §7.6), which `dispatch_status` and
/// `DispatchResolved` name.
pub(crate) fn funding_step(draft: i64) -> String {
    format!("registration/{draft}/funding")
}

pub(crate) fn identity_step(draft: i64) -> String {
    format!("registration/{draft}/identity")
}

/// The identity an asset lock creates: Platform's id for a proof of
/// `outpoint` (rs-dpp `create_identifier`, the same for both proof kinds).
pub(crate) fn identity_for(outpoint: OutPoint) -> Identifier {
    ChainAssetLockProof::new(0, outpoint.into()).create_identifier()
}

/// A tracked asset lock's status, as platform-wallet's `AssetLockStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockStatus {
    Built,
    Broadcast,
    InstantSendLocked,
    ChainLocked,
    Consumed,
    RecoveredFromChain,
}

impl LockStatus {
    /// The row holds a proof: step 2 (`FromExistingAssetLock`) resolves to
    /// it and waits for nothing (E0-04 §4.4).
    pub(crate) fn has_proof(self) -> bool {
        matches!(
            self,
            Self::InstantSendLocked | Self::ChainLocked | Self::RecoveredFromChain
        )
    }
}

/// A tracked asset lock as the flow reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockInfo {
    pub(crate) outpoint: OutPoint,
    /// Its funding type is `IdentityRegistration`.
    pub(crate) registration: bool,
    pub(crate) identity_index: u32,
    pub(crate) account: u32,
    pub(crate) amount: u64,
    pub(crate) status: LockStatus,
}

impl LockInfo {
    /// A registration lock of account 0: the kind a flow funds.
    pub(crate) fn funds_registration(&self) -> bool {
        self.registration && self.account == 0
    }

    /// The identity index the lock was funded for, when it is known
    /// ([`known_index`]).
    pub(crate) fn known_index(&self) -> Option<u32> {
        known_index(self.status, self.identity_index)
    }
}

/// A registration lock's identity index, when it is the one the lock was
/// funded for. The library's reconstruction from the chain (a restore, a
/// lost row) cannot recover a registration lock's index and reports 0
/// (`asset_lock/sync/reconstruction.rs` `funding_family`) with a status past
/// `Built`; only the live pipeline tracks a `Built` row, with the index it
/// was given. So a `Built` lock's index is known, and so is a non-zero one;
/// a 0 past `Built` may be either (Sol r1 R1-01).
pub(crate) fn known_index(status: LockStatus, identity_index: u32) -> Option<u32> {
    (status == LockStatus::Built || identity_index != 0).then_some(identity_index)
}

/// The coins of the library's pooled funding sources (BIP44, BIP32,
/// DashPay receiving).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Spendable {
    /// What the engine lets a funding spend: the coins it auto-selects.
    pub(crate) duffs: u64,
    /// Every coin of the sources, user-locked ones too: the library does
    /// not know the engine's locks, so the fee bound counts them all.
    pub(crate) inputs: usize,
    /// Coins of the sources the engine holds back but the pinned asset-lock
    /// builder would select (a user's or the dust protection's lock, a
    /// send's reservation: `held_funding_coins`), so a funding with any
    /// present is refused (Sol r1 R1-07). A tracked lock's inputs are not
    /// among them: the library reserves those itself.
    pub(crate) protected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FundError {
    /// The engine refused before the library call started (a lease that
    /// no longer signs): nothing was signed, so nothing can be out.
    Refused(PlatformError),
    /// Refused before the library call: a funding coin is held back (locked,
    /// or reserved by a send), read under the coin fence the call then
    /// holds (DEC-173 (3)).
    CoinsLocked,
    /// The proof wait ended without a proof (the 300 s InstantSend wait,
    /// or one bounded ChainLock wait); the lock is still tracked.
    IsTimeout,
    /// The library failed before anything reached the wire: a definite
    /// broadcast rejection, or no lock built at all (coin selection, the
    /// build, a key). Positive evidence of `NotSent` (H12).
    NotSent(PlatformError),
    /// Any other failure after the call started.
    Failed(PlatformError),
}

/// What a registration asks of platform-wallet.
pub(crate) trait RegistrationLib: Send + Sync {
    /// Every asset lock the wallet tracks.
    fn tracked_locks(&self) -> impl Future<Output = Vec<LockInfo>> + Send;
    /// The wallet's identities: (index, base58 id).
    fn identities(&self) -> impl Future<Output = Vec<(u32, String)>> + Send;
    fn spendable(&self) -> impl Future<Output = Result<Spendable, PlatformError>> + Send;
    /// What holds a funding (§3.4 Restore rule 1, §2.2 rule 5's hold):
    /// `None` once SPV has synced and the wallet's DashPay bring-up (DP1-05's
    /// same-seed discovery; the library's asset-lock reconstruction runs
    /// with the sync) answered which identity indexes the seed uses. `Sync`
    /// while that is under way; `Unlock` for a bring-up that runs again at
    /// the first unlock; `Network` for one that runs again only at the next
    /// start.
    fn funding_hold(&self) -> Option<RegistrationWait>;
    /// P2b's funding call permit for `step` under `key`, charging
    /// `lock_duffs` and the worst-case fee (E0-04 §2a.6,
    /// `funding_call_permit`): taken under the wallet's coin fence, which it
    /// keeps until the library tracks the lock the call signed. Refused
    /// (`CoinsLocked`) while a funding coin is held back.
    fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        lock_duffs: u64,
    ) -> impl Future<Output = Result<CallPermit, FundError>> + Send;
    /// `create_funded_asset_lock_proof` for `IdentityRegistration` at
    /// `identity_index`, signed by `permit`'s funding signer, so P2b records
    /// the lock it signs as the step's own before the persister takes it:
    /// builds, tracks, broadcasts, and waits up to 300 s for an InstantSend
    /// lock.
    fn fund(
        &self,
        lock_duffs: u64,
        identity_index: u32,
        permit: &CallPermit,
    ) -> impl Future<Output = Result<OutPoint, FundError>> + Send;
    /// `resume_asset_lock`: waits for the tracked lock's proof, at most
    /// `timeout` (`IsTimeout` then), broadcasting a `Built` lock again
    /// first. Always bounded: the library's unbounded wait ends after
    /// 180 s with an error that says nothing about the lock. Needs no key.
    fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Duration,
    ) -> impl Future<Output = Result<(), FundError>> + Send;
    /// The tracked lock at `outpoint` as P2b reads it ([`TrackedRow::of`]),
    /// for the resume gate ([`LeaseTable::unrecorded_signer_of`]).
    fn tracked_row(&self, outpoint: OutPoint) -> impl Future<Output = Option<TrackedRow>> + Send;
    /// Whether SPV is caught up with the network (DEC-197: a start past an
    /// unknown funding needs the wallet's history current).
    fn spv_synced(&self) -> impl Future<Output = bool> + Send;
    /// The identity the lock created, if Platform (or the wallet) has it.
    fn find_identity(
        &self,
        outpoint: OutPoint,
    ) -> impl Future<Output = Result<Option<String>, PlatformError>> + Send;
    /// IdentityCreate from the tracked lock (`FromExistingAssetLock`),
    /// with keys 0–5 of `identity_index`: the transition signed by
    /// `permit`'s counting identity signer (step 2's retries re-sign inside
    /// the one call, E0-04 §2a.3), the lock's proof by the lease's
    /// uncounted funding signer (no lock is built).
    fn register_identity(
        &self,
        outpoint: OutPoint,
        identity_index: u32,
        permit: &CallPermit,
    ) -> impl Future<Output = Result<String, PlatformError>> + Send;
    /// DP1-03's `register_name` under the lease.
    fn register_name(
        &self,
        identity: &str,
        label: &str,
        lease: &Lease,
    ) -> impl Future<Output = Result<NameOutcome, NameError>> + Send;
    /// The identity's DashPay profile as Platform has it (its display name,
    /// public message and avatar URL), `None` if it has none.
    fn platform_profile(
        &self,
        identity: &str,
    ) -> impl Future<Output = Result<Option<ProfileRecord>, PlatformError>> + Send;
    /// Creates the identity's DashPay profile document. Platform refuses a
    /// second one (the profile's unique `$ownerId` index), so the step asks
    /// [`Self::platform_profile`] first.
    fn create_profile(
        &self,
        identity: &str,
        identity_index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// Revises the profile Platform has: the fields `profile` sets replace
    /// its own, the others stay (the library's read-modify-write update).
    fn update_profile(
        &self,
        identity: &str,
        identity_index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
    /// What `register_name`'s grant asks for `label`, in credits.
    fn name_credits(&self, label: &str) -> impl Future<Output = Result<u64, NameError>> + Send;
    /// The identity's credits; `None` when Platform does not know it.
    fn credit_balance(
        &self,
        identity: &str,
    ) -> impl Future<Output = Result<Option<u64>, PlatformError>> + Send;
}

/// One flow's state in this process (the session's Platform runtime),
/// none of it persisted: the claim's generation lives in the row's
/// `owner`.
#[derive(Debug, Default, Clone)]
pub(crate) struct FlowState {
    /// The claim that drives or decides the flow now (DEC-171): one at a
    /// time, and only its run may call the library for the flow.
    holder: Option<Holder>,
    /// A library call of the holder's run runs: no decision takes the flow.
    pub(crate) in_call: bool,
    /// What the flow waits for, set when it parks or starts a wait.
    pub(crate) wait: Option<RegistrationWait>,
    /// The InstantSend window's end, from an in-memory instant taken at the
    /// funding call (never `updated_at`, §3.4).
    pub(crate) is_until: Option<Instant>,
    /// The lease the running flow signs with.
    pub(crate) lease: Option<Held>,
    /// A lease `resume_registration` handed to a run that has none (or one
    /// that no longer signs); the run takes it at its next keyed step.
    offered: Option<Held>,
    /// The automatic backup of `FundingSent` was scheduled.
    pub(crate) backed_up: bool,
    pub(crate) task: Option<tokio::task::AbortHandle>,
    /// The next this many lock records of the run fail (a test's storage
    /// fault).
    #[cfg(test)]
    lock_write_faults: u32,
    /// Storage faults a test set for the run's next writes.
    #[cfg(test)]
    write_faults: u32,
    /// Automatic backups scheduled for the flow (a test's count).
    #[cfg(test)]
    pub(crate) backups: u32,
}

impl FlowState {
    /// A driver runs the flow (a task's, or "Finish transfers"').
    pub(crate) fn running(&self) -> bool {
        self.holder.is_some_and(|h| h.kind == Kind::Run)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Holder {
    /// Unique in the process, so a claim of a flow that was forgotten and
    /// claimed again is never taken for the new one.
    run: u64,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A driver: a decision may take the flow from it between its calls.
    Run,
    /// A discard deciding: nothing takes the flow from it.
    Decide,
}

#[derive(Clone)]
pub(crate) struct Held(pub(crate) Lease);

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Held").field(&self.0.id()).finish()
    }
}

/// What a decision took from a run: the run's task (aborted) and lease,
/// which a refused decision hands to a new run.
pub(crate) struct Preempted {
    /// A run held the flow.
    pub(crate) ran: bool,
    pub(crate) lease: Option<Lease>,
}

/// A job of a wallet's registration actor.
type Job = Box<dyn FnOnce() + Send>;

/// Every flow's [`FlowState`], keyed by wallet and draft, and each wallet's
/// registration actor (DEC-173): one thread that runs, one at a time and in
/// the order sent, every claim of the wallet's flows, every read of its
/// `dp_registration` rows that leads to a write, and every write. A job
/// runs to its end whatever happens to the task that sent it, so no claim
/// or write of a task that was aborted lands after a newer one.
#[derive(Debug, Default)]
pub(crate) struct Flows {
    flows: Mutex<HashMap<(WalletId, i64), FlowState>>,
    /// The next claim's `run`.
    runs: AtomicU64,
    /// Each wallet's actor's queue.
    actors: Mutex<HashMap<WalletId, std::sync::mpsc::Sender<Job>>>,
    /// Storage faults a test set for the tap's next lock records.
    #[cfg(test)]
    tap_write_faults: std::sync::atomic::AtomicU32,
}

impl Flows {
    fn with<R>(&self, key: (WalletId, i64), f: impl FnOnce(&mut FlowState) -> R) -> R {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        f(map.entry(key).or_default())
    }

    /// Like [`Self::with`], but leaves a forgotten flow forgotten.
    fn with_existing<R>(
        &self,
        key: (WalletId, i64),
        f: impl FnOnce(&mut FlowState) -> R,
    ) -> Option<R> {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        map.get_mut(&key).map(f)
    }

    /// `f` on the flow's state while `claim` still holds it.
    fn with_held<R>(&self, claim: &Claim, f: impl FnOnce(&mut FlowState) -> R) -> Option<R> {
        self.with_run(claim.key, claim.run, f)
    }

    /// `f` on the flow's state while `run` still holds it.
    fn with_run<R>(
        &self,
        key: (WalletId, i64),
        run: u64,
        f: impl FnOnce(&mut FlowState) -> R,
    ) -> Option<R> {
        self.with_existing(key, |s| {
            (s.holder.map(|h| h.run) == Some(run)).then(|| f(s))
        })
        .flatten()
    }

    pub(crate) fn get(&self, key: (WalletId, i64)) -> FlowState {
        self.flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// Sends `job` to `wallet`'s actor, which starts with the first.
    fn send(&self, wallet: WalletId, job: Job) {
        let mut actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        let actor = actors.entry(wallet).or_insert_with(|| {
            let (tx, rx) = std::sync::mpsc::channel::<Job>();
            let started = std::thread::Builder::new()
                .name("dw-registrations".into())
                .spawn(move || {
                    // Ends with the session's `Flows`, the last sender.
                    for job in rx {
                        // A job that panics fails its own sender only.
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                    }
                });
            if let Err(e) = started {
                tracing::error!(error = %e, "could not start the registration actor");
            }
            tx
        });
        let _ = actor.send(job);
    }

    /// Runs `f` on `wallet`'s actor, after every job sent before it.
    async fn act<R: Send + 'static>(
        &self,
        wallet: WalletId,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Result<R, PlatformError> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(
            wallet,
            Box::new(move || {
                let _ = tx.send(f());
            }),
        );
        rx.await.map_err(|_| PlatformError::Internal {
            detail: "the registration actor stopped".into(),
        })
    }

    /// [`Self::act`] for the changeset tap, which runs inside the
    /// persister and cannot await. No job calls the library, so waiting
    /// for one there cannot wait for the library's own locks.
    fn act_blocking<R: Send + 'static>(
        &self,
        wallet: WalletId,
        f: impl FnOnce() -> R + Send + 'static,
    ) -> Option<R> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.send(
            wallet,
            Box::new(move || {
                let _ = tx.send(f());
            }),
        );
        rx.recv().ok()
    }

    fn new_claim(self: &Arc<Self>, key: (WalletId, i64), s: &mut FlowState, kind: Kind) -> Claim {
        let run = self.runs.fetch_add(1, Ordering::Relaxed) + 1;
        s.holder = Some(Holder { run, kind });
        s.offered = None;
        s.wait = None;
        Claim {
            flows: Arc::clone(self),
            key,
            run,
            owner: 0,
        }
    }

    /// The in-memory half of a claim: a run's while no one holds the flow;
    /// a decision's (a discard) while no call runs and no other decision
    /// holds it, taken from a run between its calls, whose task ends.
    fn admit(self: &Arc<Self>, key: (WalletId, i64), kind: Kind) -> Option<(Claim, Preempted)> {
        let (claim, task, preempted) = self.with(key, |s| {
            let free = match kind {
                Kind::Run => s.holder.is_none(),
                Kind::Decide => !s.in_call && s.holder.is_none_or(|h| h.kind == Kind::Run),
            };
            if !free {
                return None;
            }
            let preempted = Preempted {
                ran: s.running(),
                // A lease offered to the run is the newer authority.
                lease: s.offered.take().or(s.lease.take()).map(|h| h.0),
            };
            let task = s.task.take();
            Some((self.new_claim(key, s, kind), task, preempted))
        })?;
        if let Some(task) = task {
            task.abort();
        }
        Some((claim, preempted))
    }

    /// A claim of the flow and its row, in one job of the actor: admitted,
    /// then the row's `owner` raised, so every write of an earlier claim is
    /// refused and no earlier claim's can follow it. `None` while another
    /// holds the flow (see [`Self::admit`]); a run's also once the row is
    /// gone (a decision's then finds it gone).
    async fn acquire(
        self: &Arc<Self>,
        session: &NetworkSession,
        key: (WalletId, i64),
        kind: Kind,
    ) -> Result<Option<(Claim, Preempted)>, PlatformError> {
        rows_op(session, key.0, move |db, flows| {
            let Some((mut claim, preempted)) = flows.admit(key, kind) else {
                return Ok(None);
            };
            let owner = db.claim_registration(&key.0.to_string(), key.1)?;
            if owner.is_none() && kind == Kind::Run {
                return Ok(None);
            }
            claim.owner = owner.unwrap_or(0);
            Ok(Some((claim, preempted)))
        })
        .await
    }

    /// A driver's claim of the flow and its row; `None` while another holds
    /// the flow, or once the row is gone.
    pub(crate) async fn claim(
        self: &Arc<Self>,
        session: &NetworkSession,
        key: (WalletId, i64),
    ) -> Result<Option<Claim>, PlatformError> {
        Ok(self
            .acquire(session, key, Kind::Run)
            .await?
            .map(|(claim, _)| claim))
    }

    /// A decision's claim (a discard); `None` while a call runs or another
    /// decision holds the flow.
    pub(crate) async fn take(
        self: &Arc<Self>,
        session: &NetworkSession,
        key: (WalletId, i64),
    ) -> Result<Option<(Claim, Preempted)>, PlatformError> {
        self.acquire(session, key, Kind::Decide).await
    }

    /// Marks a library call of `claim`'s run, atomic with a decision's
    /// [`Self::admit`]: `Err(Stop)` once the flow is no longer its.
    fn enter_call<'a>(&'a self, claim: &'a Claim) -> Result<InCall<'a>, Step> {
        self.with_held(claim, |s| s.in_call = true)
            .map(|()| InCall(self, claim))
            .ok_or(Step::Stop)
    }

    /// The flow is still `claim`'s.
    fn holds(&self, claim: &Claim) -> bool {
        self.with_held(claim, |_| ()).is_some()
    }

    /// Hands `lease` to the flow's running driver (see
    /// [`FlowState::offered`]); `Err` gives it back when no run is left to
    /// take it.
    pub(crate) fn offer(&self, key: (WalletId, i64), lease: Lease) -> Result<(), Lease> {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        match map.get_mut(&key) {
            Some(s) if s.running() => {
                if let Some(task) = &s.task {
                    lease.register_task(task.clone());
                }
                s.offered = Some(Held(lease));
                Ok(())
            }
            _ => Err(lease),
        }
    }

    pub(crate) fn forget(&self, key: (WalletId, i64)) {
        let state = self
            .flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        if let Some(task) = state.and_then(|s| s.task) {
            task.abort();
        }
    }

    pub(crate) fn abort_all(&self) {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        for task in map.values_mut().filter_map(|s| s.task.take()) {
            task.abort();
        }
    }

    /// An unloaded wallet's flows: their tasks end now, and their states go
    /// in a job of the actor, so a claim sent before it is forgotten with
    /// them and none sent after it (the wallet loaded again) finds them.
    pub(crate) fn forget_wallet(self: &Arc<Self>, wallet: WalletId) {
        {
            let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
            for (_, s) in map.iter_mut().filter(|((w, _), _)| *w == wallet) {
                if let Some(task) = s.task.take() {
                    task.abort();
                }
            }
        }
        let flows = Arc::clone(self);
        let clear = move || {
            let mut map = flows.flows.lock().unwrap_or_else(PoisonError::into_inner);
            map.retain(|(w, _), s| {
                if *w == wallet
                    && let Some(task) = s.task.take()
                {
                    task.abort();
                }
                *w != wallet
            });
        };
        let actors = self.actors.lock().unwrap_or_else(PoisonError::into_inner);
        match actors.get(&wallet) {
            Some(actor) => {
                let _ = actor.send(Box::new(clear));
            }
            // No job is queued, and none can be while the queues are held.
            None => clear(),
        }
    }

    /// The drafts of `wallet` whose library call runs (the ledger's
    /// `calling`).
    pub(crate) fn in_call_drafts(&self, wallet: WalletId) -> Vec<i64> {
        self.flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|((w, _), s)| *w == wallet && s.in_call)
            .map(|((_, d), _)| *d)
            .collect()
    }

    /// Fails the next `n` lock records of `key`'s run with a storage error.
    #[cfg(test)]
    pub(crate) fn fail_lock_writes(&self, key: (WalletId, i64), n: u32) {
        self.with(key, |s| s.lock_write_faults = n);
    }

    /// Fails the next `n` writes of `key`'s run with a storage error.
    #[cfg(test)]
    pub(crate) fn fail_writes(&self, key: (WalletId, i64), n: u32) {
        self.with(key, |s| s.write_faults = n);
    }

    /// Fails the tap's next `n` lock records with a storage error.
    #[cfg(test)]
    pub(crate) fn fail_tap_writes(&self, n: u32) {
        self.tap_write_faults
            .store(n, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn tap_write_fault(&self) -> bool {
        self.tap_write_faults
            .fetch_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |n| n.checked_sub(1),
            )
            .is_ok()
    }
}

/// Runs `f` with the app database and the flows on `wallet`'s registration
/// actor (DEC-173).
pub(crate) async fn rows_op<T: Send + 'static>(
    session: &NetworkSession,
    wallet: WalletId,
    f: impl FnOnce(&AppDb, &Arc<Flows>) -> Result<T, AppDbError> + Send + 'static,
) -> Result<T, PlatformError> {
    let db = session.live()?.appdb;
    let flows = Arc::clone(&session.platform.registrations);
    let actor = Arc::clone(&flows);
    actor
        .act(wallet, move || f(&db, &flows))
        .await?
        .map_err(|e| crate::EngineError::from(e).into())
}

/// [`rows_op`] for a job that reads a [`Ledger`], whose errors are
/// [`PlatformError`]s.
pub(crate) async fn ledger_op<T: Send + 'static>(
    session: &NetworkSession,
    wallet: WalletId,
    f: impl FnOnce(&AppDb, &Arc<Flows>) -> Result<T, PlatformError> + Send + 'static,
) -> Result<T, PlatformError> {
    rows_op(session, wallet, move |db, flows| Ok(f(db, flows))).await?
}

/// An [`AppDbError`] as the [`PlatformError`] [`rows_op`] makes of it.
pub(crate) fn db_error(e: AppDbError) -> PlatformError {
    crate::EngineError::from(e).into()
}

/// One holder's claim of a flow (DEC-171); released when dropped. Its
/// cleanup and its writes act only while the flow and the row are still
/// its: a claim taken over, or one of a flow forgotten and claimed again,
/// changes nothing.
pub(crate) struct Claim {
    flows: Arc<Flows>,
    key: (WalletId, i64),
    run: u64,
    /// The row's `owner` this claim raised it to.
    owner: u64,
}

impl Claim {
    pub(crate) fn draft(&self) -> i64 {
        self.key.1
    }

    /// The row's generation this claim holds.
    pub(crate) fn owner(&self) -> u64 {
        self.owner
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.flows.with_held(self, |s| {
            s.holder = None;
            s.in_call = false;
            s.task = None;
            s.lease = None;
            s.offered = None;
            // A row no task drives reads its wait from the row and the
            // world (`idle_wait`), never from a run that ended.
            s.wait = None;
        });
    }
}

/// Marks a library call of a run; cleared when dropped, if the flow is
/// still the run's.
struct InCall<'a>(&'a Flows, &'a Claim);

impl Drop for InCall<'_> {
    fn drop(&mut self) {
        self.0.with_held(self.1, |s| s.in_call = false);
    }
}

/// What one step decided.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// The row changed; read it and step again.
    Again,
    /// A bounded wait ended without the proof: step again at once, still
    /// waiting for it.
    Rewait,
    /// Waits for `RegistrationWait` in the run, if it is a task's (it
    /// keeps its lease); otherwise as `Park`.
    Wait(RegistrationWait),
    /// Waits for `RegistrationWait` with the run ended (an authority it
    /// lacks, a bring-up outcome of the next start); the row keeps its
    /// phase.
    Park(RegistrationWait),
    /// Fails the flow with a code.
    Fail(&'static str, bool),
    /// Nothing left to do.
    Stop,
}

/// What a driver's write found ([`Driver::write`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Written {
    Applied,
    /// The row moved since it was read (evidence written meanwhile, or a
    /// lock another flow took); the claim still holds it.
    Moved,
    /// A newer claim took the row, or it is gone: the run stops (DEC-173).
    Lost,
}

fn unsupported() -> &'static str {
    RegistrationError::Unsupported.code()
}

fn internal() -> &'static str {
    PlatformError::Internal {
        detail: String::new(),
    }
    .code()
}

pub(crate) fn broadcast_unknown() -> &'static str {
    PlatformError::BroadcastUnknown {
        artifact: String::new(),
    }
    .code()
}

pub(crate) fn outpoint_of(row: &RegistrationRow) -> Option<OutPoint> {
    row.asset_lock_outpoint.as_deref()?.parse().ok()
}

pub(crate) fn canonical(outpoint: OutPoint) -> LockOutpoint {
    LockOutpoint::parse(&outpoint.to_string()).expect("OutPoint displays canonically")
}

/// What this build makes of a row (DEC-171 (3)): its phase and funding, or
/// the code it fails with when this build cannot run it. A phase, funding,
/// profile or outpoint this build does not know is
/// `registration.unsupported`: such a row is never funded, discarded or
/// advanced. An invitation row is `invitation.invalid` (DP5-02 seam: the
/// link is a bearer credential in the vault of the machine that stashed it
/// only, so a restored row cannot use it, and none can before DP5-02); a
/// faucet row is `platform.feature_off` (developer builds only, and this
/// build has none).
pub(crate) fn interpret(row: &RegistrationRow) -> Result<(Phase, Funding), &'static str> {
    let profile_known = row
        .initial_profile
        .as_deref()
        .is_none_or(|p| matches!(ProfileRecord::decode(p), Decoded::Known(_)));
    let outpoint_known = row.asset_lock_outpoint.is_none() || outpoint_of(row).is_some();
    let (Some(phase), Decoded::Known(funding), true, true) = (
        Phase::parse(&row.phase),
        Funding::decode(&row.funding),
        profile_known,
        outpoint_known,
    ) else {
        return Err(unsupported());
    };
    match funding {
        Funding::Invitation { .. } => Err(super::errors::InvitationError::Invalid.code()),
        Funding::FaucetAssetLock { .. } => Err(PlatformError::FeatureOff {
            feature: String::new(),
        }
        .code()),
        Funding::CoreBalance { .. } | Funding::ExistingIdentity => Ok((phase, funding)),
    }
}

/// A row's failure: the code it cannot be run with, else its stored one;
/// with whether it is retryable.
pub(crate) fn failure_of(row: &RegistrationRow) -> Option<(&str, bool)> {
    match interpret(row) {
        Err(code) => Some((code, false)),
        Ok(_) => row.error.as_deref().map(|code| (code, row.retryable)),
    }
}

pub(crate) fn is_core_balance(row: &RegistrationRow) -> bool {
    matches!(interpret(row), Ok((_, Funding::CoreBalance { .. })))
}

/// A row this build cannot read (DEC-171 (3)).
pub(crate) fn is_unsupported(row: &RegistrationRow) -> bool {
    interpret(row) == Err(unsupported())
}

/// A row this build cannot read that may have funded a lock it records no
/// outpoint of (DEC-173 (2)): none it can parse, no identity, and a phase
/// past `Draft` as far as it reads.
pub(crate) fn may_own_an_unrecorded_lock(row: &RegistrationRow) -> bool {
    is_unsupported(row)
        && outpoint_of(row).is_none()
        && row.identity.is_none()
        && row.phase != Phase::Draft.as_str()
}

/// The changeset tap's report of a registration lock the library tracked,
/// from inside the persister, before the library broadcasts it. The lock
/// goes to the row the [`Ledger`] lets take it at its known index (§3.4
/// Restore rule 3, DP1-02 §6), in a job of the wallet's actor; a lock whose
/// index is not known is no row's by index. Returns the draft moved to
/// `FundingSent`. (The funding call's coin fence is P2b's: its permit
/// releases it when the library tracks the lock it signed, recorded in
/// `leases` before this runs.)
pub(crate) fn lock_tracked(
    flows: &Arc<Flows>,
    appdb: &Arc<AppDb>,
    leases: &Arc<LeaseTable>,
    wallet: WalletId,
    lock: LockInfo,
) -> Result<Option<i64>, AppDbError> {
    if lock.known_index().is_none() {
        return Ok(None);
    }
    let now = crate::events::unix_now();
    // A failed write is tried again (it reads the row afresh, so a retry
    // never writes twice). One that keeps failing leaves the lock to the
    // funding call, which attributes it when the call ends (MN-1).
    let mut tries = 1;
    loop {
        let (db, job_flows, leases, lock) = (
            Arc::clone(appdb),
            Arc::clone(flows),
            Arc::clone(leases),
            lock.clone(),
        );
        #[cfg(test)]
        let fault = flows.tap_write_fault();
        #[cfg(not(test))]
        let fault = false;
        let noted = match fault {
            true => Err(AppDbError::Corrupt("a test's fault".into())),
            false => flows
                .act_blocking(wallet, move || {
                    note_lock_tracked(&db, &job_flows, &leases, wallet, &lock, now)
                })
                .unwrap_or(Ok(None)),
        };
        match noted {
            Err(e) if tries < LOCK_WRITE_TRIES => {
                tracing::warn!(error = %e, "retrying a registration lock's record");
                // Inside the library's store: a short, blocking pause.
                std::thread::sleep(LOCK_WRITE_RETRY * tries);
                tries += 1;
            }
            noted => return noted,
        }
    }
}

/// [`lock_tracked`]'s write, on the actor: one [`Ledger`] read and the
/// write in the one job. The candidates are the rows at the lock's known
/// index holding no outpoint that [`Ledger::may_take`] lets take it; the
/// row whose funding call runs first, else the oldest.
fn note_lock_tracked(
    db: &AppDb,
    flows: &Flows,
    leases: &LeaseTable,
    wallet: WalletId,
    lock: &LockInfo,
    now: u64,
) -> Result<Option<i64>, AppDbError> {
    let Some(index) = lock.known_index() else {
        return Ok(None);
    };
    // Consumed value goes only through §5.2a (D4).
    if lock.status == LockStatus::Consumed {
        return Ok(None);
    }
    let w = wallet.to_string();
    let outpoint = canonical(lock.outpoint);
    if db.registration_for_lock(&w, outpoint.as_str())?.is_some() {
        return Ok(None);
    }
    // The library's other locks and identities cannot be read from inside
    // its store: this lock is the one the tap decides. A journal PA-1
    // cannot read fails closed: the lock is left to the funding call and
    // "Finish transfers".
    let Ok(ledger) = Ledger::read_in(db, flows, leases, wallet, vec![lock.clone()], Vec::new())?
    else {
        tracing::warn!("the dispatch journal is unavailable: a tracked lock waits");
        return Ok(None);
    };
    #[cfg(test)]
    note_pause(lock.outpoint);
    // OD-05 is `may_take`'s: a calling row takes only its own `Step` lock.
    let candidates = || {
        ledger.rows().iter().filter(|r| {
            r.identity_index == Some(index)
                && ledger.live(r).is_none()
                && ledger.may_take(Taker::Row(r.id), lock)
        })
    };
    let row = candidates()
        .find(|r| ledger.calling(r.id))
        .or_else(|| candidates().next());
    let Some(row) = row else {
        return Ok(None);
    };
    attach_lock(db, &w, row, &outpoint, now)
}

/// A pause of [`note_lock_tracked`] between its read and its write, for
/// the lock a test names: (that lock, "paused", "go on").
#[cfg(test)]
type NotePause = (
    OutPoint,
    std::sync::mpsc::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[cfg(test)]
pub(crate) static NOTE_PAUSE: Mutex<Option<NotePause>> = Mutex::new(None);

#[cfg(test)]
fn note_pause(outpoint: OutPoint) {
    let mut pause = NOTE_PAUSE.lock().unwrap_or_else(PoisonError::into_inner);
    if pause.as_ref().is_some_and(|(op, _, _)| *op == outpoint)
        && let Some((_, paused, go)) = pause.take()
    {
        drop(pause);
        let _ = paused.send(());
        let _ = go.recv();
    }
}

/// Moves `row` (as read on the actor, with no live outpoint) to
/// `FundingSent` with `lock`, clearing a failure; the draft if it moved.
fn attach_lock(
    db: &AppDb,
    wallet: &str,
    row: &RegistrationRow,
    lock: &LockOutpoint,
    now: u64,
) -> Result<Option<i64>, AppDbError> {
    let failed = row.error.is_some();
    let change = RegistrationChange {
        phase: Some(Phase::FundingSent.as_str().into()),
        outpoint: Some(lock.clone()),
        failure: failed.then_some(None),
        ..Default::default()
    };
    let written =
        db.update_registration(wallet, row.id, row.owner, &row.phase, failed, &change, now)?;
    Ok((written == RegistrationUpdate::Applied).then_some(row.id))
}

/// Applies one [`Reconcile`] to `row` (as read on the actor), as one
/// compare-and-set under `owner` (§5.2): whether it was written.
pub(crate) fn apply_reconcile(
    db: &AppDb,
    wallet: &str,
    row: &RegistrationRow,
    owner: u64,
    reconcile: &Reconcile,
    now: u64,
) -> Result<bool, AppDbError> {
    let failed = row.error.is_some();
    let change = match reconcile {
        // (a): the identity's own index replaces the row's (its keys are
        // there); no funding and no identity creation follow.
        Reconcile::Attach {
            lock,
            identity,
            index,
            ..
        } => RegistrationChange {
            phase: Some(Phase::IdentityRegistered.as_str().into()),
            identity_index: Some(*index),
            identity: Some(identity.clone()),
            outpoint: Some(canonical(*lock)),
            failure: failed.then_some(None),
            ..Default::default()
        },
        // (b)
        Reconcile::Close { clear, code, .. } => RegistrationChange {
            phase: clear.then(|| Phase::KeysPrepared.as_str().into()),
            clear_outpoint: *clear,
            failure: code.map(|c| Some((c.into(), false))),
            ..Default::default()
        },
    };
    let written =
        db.update_registration(wallet, row.id, owner, &row.phase, failed, &change, now)?;
    Ok(written == RegistrationUpdate::Applied)
}

/// A row §5.2b closed: the backup taken for its funding no longer holds
/// it, so it is taken again (MN-2) and a later restore carries the close.
pub(crate) fn backup_again(session: &Arc<NetworkSession>, wallet: WalletId, draft: i64) {
    session
        .platform
        .registrations
        .with((wallet, draft), |s| s.backed_up = false);
    backup_once(session, wallet, draft);
}

/// "Finish transfers"' import, on the actor (DEC-171 (2), DEC-173, DP1-02
/// §6): first every [`Reconcile`] of the ledger is written (§5.2); then, on
/// a fresh ledger, every unheld unconsumed registration lock goes where its
/// [`Owner`] and [`Ledger::may_take`] let it. A `Step` lock goes to its
/// step's open row, calling or not, else to a recovery row when
/// `may_take(Recovery)` lets it; a `Free` lock to the row at its known
/// index, else to the oldest row waiting for a lock whose amount it covers,
/// else to a recovery row (a new unnamed row at an unused index); a
/// `Contested` lock to a recovery row when only wedged steps contest it
/// (DEC-198). `Parked` and `Dead` locks go nowhere. `held`: the funding
/// hold is on, and until discovery has answered which indexes the seed
/// uses, a fresh index may be another device's identity's, so nothing is
/// written. Returns [`Ledger::pending`], read afresh after the writes, and
/// the drafts §5.2b closed (their backups are taken again).
#[allow(clippy::too_many_arguments)]
pub(crate) fn import_stranded(
    db: &AppDb,
    flows: &Flows,
    leases: &LeaseTable,
    wallet: WalletId,
    locks: Vec<LockInfo>,
    identities: Vec<(u32, String)>,
    held: bool,
    now: u64,
) -> Result<(u32, Vec<i64>), PlatformError> {
    let w = wallet.to_string();
    let read = || Ledger::read(db, flows, leases, wallet, locks.clone(), identities.clone());
    let ledger = read()?;
    if held {
        return Ok((ledger.pending(), Vec::new()));
    }
    let mut closed = Vec::new();
    let mut wrote = false;
    for reconcile in ledger.reconcile() {
        let Some(row) = ledger.row(reconcile.row()) else {
            continue;
        };
        if apply_reconcile(db, &w, row, row.owner, &reconcile, now).map_err(db_error)? {
            wrote = true;
            if reconcile.closes() {
                closed.push(row.id);
            }
        }
    }
    let ledger = if wrote { read()? } else { ledger };
    let mut next = ledger.next_index();
    let mut given: Vec<i64> = Vec::new();
    for lock in ledger.locks() {
        let may = |r: &&RegistrationRow| {
            !given.contains(&r.id) && ledger.may_take(Taker::Row(r.id), lock)
        };
        let row = match ledger.owner(lock) {
            Some(Owner::Step(_)) => ledger.rows().iter().find(may),
            Some(Owner::Free) => match lock.known_index() {
                Some(index) => ledger
                    .rows()
                    .iter()
                    .filter(|r| r.identity_index == Some(index))
                    .find(may),
                // D3: a row waiting for a lock (no failure, or a retryable
                // one) whose amount the lock covers.
                None => ledger.rows().iter().filter(may).find(|r| {
                    r.identity_index.is_some()
                        && (r.error.is_none() || r.retryable)
                        && matches!(interpret(r), Ok((_, Funding::CoreBalance { lock_duffs }))
                            if lock_duffs <= lock.amount)
                }),
            },
            // A row never; Recovery when only wedged steps contest it.
            Some(Owner::Contested) => None,
            Some(Owner::Parked | Owner::Dead) | None => continue,
        };
        let outpoint = canonical(lock.outpoint);
        if let Some(row) = row {
            if attach_lock(db, &w, row, &outpoint, now)
                .map_err(db_error)?
                .is_some()
            {
                given.push(row.id);
            }
            continue;
        }
        if !ledger.may_take(Taker::Recovery, lock) {
            continue;
        }
        let row = NewRegistration {
            identity_index: Some(next),
            identity: None,
            // A recovered lock has no name to register.
            label: String::new(),
            temp_label: None,
            funding: Funding::CoreBalance {
                lock_duffs: lock.amount,
            }
            .encode(),
            initial_profile: None,
            phase: if lock.status.has_proof() {
                Phase::ProofWaiting
            } else {
                Phase::FundingSent
            }
            .as_str()
            .into(),
        };
        if db
            .adopt_registration_lock(&w, &outpoint, &row, now)
            .map_err(db_error)?
            .1
        {
            next = next.saturating_add(1);
        }
    }
    Ok((read()?.pending(), closed))
}

/// One run of one flow: steps the row until it parks, fails or is done.
/// It writes only while its claim holds the flow (DEC-171).
pub(crate) struct Driver<'a, L> {
    session: &'a Arc<NetworkSession>,
    wallet: WalletId,
    lib: &'a L,
    claim: &'a Claim,
    /// The authority the run signs with; `None` runs only the steps that
    /// need no key (the proof wait) and parks at the first that does,
    /// unless `resume_registration` offers one meanwhile.
    lease: Mutex<Option<Lease>>,
    /// A task's run: it waits for a missing proof, for the sync and for
    /// the network itself, keeping its lease (one prompt, §2.6). `false`
    /// parks at each, as "Finish transfers" does.
    waits: bool,
}

impl<'a, L: RegistrationLib> Driver<'a, L> {
    /// `claim` from [`Flows::claim`].
    pub(crate) fn new(
        session: &'a Arc<NetworkSession>,
        lib: &'a L,
        claim: &'a Claim,
        lease: Option<Lease>,
        waits: bool,
    ) -> Self {
        Self {
            session,
            wallet: claim.key.0,
            lib,
            claim,
            lease: Mutex::new(lease),
            waits,
        }
    }

    fn flows(&self) -> &Flows {
        &self.session.platform.registrations
    }

    fn draft(&self) -> i64 {
        self.claim.draft()
    }

    /// The run's lease; one offered meanwhile replaces it.
    fn lease(&self) -> Option<Lease> {
        let mut lease = self.lease.lock().unwrap_or_else(PoisonError::into_inner);
        let offered = self
            .flows()
            .with_held(self.claim, |s| {
                let offered = s.offered.take()?;
                s.lease = Some(offered.clone());
                Some(offered.0)
            })
            .flatten();
        if offered.is_some() {
            *lease = offered;
        }
        lease.clone()
    }

    /// Steps the flow until it parks, fails or ends. An error is a storage
    /// failure: the row keeps the phase it had, and the flow is resumable.
    pub(crate) async fn run(&self) -> Result<(), PlatformError> {
        let lease = self
            .lease
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        self.flows()
            .with_held(self.claim, |s| s.lease = lease.map(Held));
        // The caller drops the claim after the run, which clears its wait.
        self.steps().await
    }

    async fn steps(&self) -> Result<(), PlatformError> {
        let draft = self.draft();
        let mut retry = NETWORK_RETRY_MIN;
        loop {
            // A decision took the flow (a discard), or it was forgotten.
            if !self.flows().holds(self.claim) {
                return Ok(());
            }
            let Some(row) = self.row().await? else {
                return Ok(());
            };
            if row.error.is_some() {
                return Ok(());
            }
            match self.step(&row).await? {
                Step::Again => {
                    retry = NETWORK_RETRY_MIN;
                    self.flows().with_held(self.claim, |s| s.wait = None);
                }
                Step::Rewait => {}
                Step::Wait(wait) if self.waits => {
                    self.flows().with_held(self.claim, |s| s.wait = Some(wait));
                    let pause = match wait {
                        RegistrationWait::Sync => SYNC_POLL,
                        RegistrationWait::ChainLock => CHAINLOCK_POLL,
                        _ => {
                            tracing::info!(draft, phase = %row.phase, "registration retries");
                            let pause = retry;
                            retry = (retry * 2).min(NETWORK_RETRY_MAX);
                            pause
                        }
                    };
                    tokio::time::sleep(pause).await;
                }
                Step::Wait(wait) | Step::Park(wait) => {
                    tracing::info!(draft, phase = %row.phase, ?wait, "registration parked");
                    self.flows().with_held(self.claim, |s| s.wait = Some(wait));
                    return Ok(());
                }
                Step::Fail(code, retryable) => {
                    tracing::info!(draft, phase = %row.phase, code, retryable, "registration failed");
                    self.fail(&row, code, retryable).await?;
                    return Ok(());
                }
                Step::Stop => return Ok(()),
            }
        }
    }

    async fn fail(
        &self,
        row: &RegistrationRow,
        code: &str,
        retryable: bool,
    ) -> Result<(), PlatformError> {
        let change = RegistrationChange {
            failure: Some(Some((code.into(), retryable))),
            ..Default::default()
        };
        self.write(row, change).await?;
        Ok(())
    }

    async fn ledger(&self) -> Result<Ledger, PlatformError> {
        Ledger::load(self.session, self.wallet, self.lib).await
    }

    async fn row(&self) -> Result<Option<RegistrationRow>, PlatformError> {
        let (wallet, draft) = (self.wallet.to_string(), self.draft());
        rows_op(self.session, self.wallet, move |db, _| {
            db.registration_row(&wallet, draft)
        })
        .await
    }

    async fn write(
        &self,
        row: &RegistrationRow,
        change: RegistrationChange,
    ) -> Result<Written, PlatformError> {
        self.write_from(row, move |_, _| Ok(change)).await
    }

    /// Compare-and-set from the row as read, under the run's claim, of the
    /// change `change` makes on the actor (DEC-173).
    async fn write_from(
        &self,
        row: &RegistrationRow,
        change: impl FnOnce(&AppDb, &Flows) -> Result<RegistrationChange, PlatformError>
        + Send
        + 'static,
    ) -> Result<Written, PlatformError> {
        let (wallet, id, owner, phase, failed) = (
            self.wallet.to_string(),
            row.id,
            self.claim.owner,
            row.phase.clone(),
            row.error.is_some(),
        );
        let now = crate::events::unix_now();
        let written = ledger_op(self.session, self.wallet, move |db, flows| {
            let change = change(db, flows)?;
            let update = db.update_registration(&wallet, id, owner, &phase, failed, &change, now);
            Ok(match update.map_err(db_error)? {
                RegistrationUpdate::Applied => Written::Applied,
                RegistrationUpdate::LockTaken => Written::Moved,
                RegistrationUpdate::Stale => match db.registration_row(&wallet, id) {
                    Ok(Some(r)) if r.owner == owner => Written::Moved,
                    Ok(_) => Written::Lost,
                    Err(e) => return Err(db_error(e)),
                },
            })
        })
        .await?;
        match written {
            Written::Applied => signal(self.session, self.wallet, id),
            Written::Moved => {}
            Written::Lost => tracing::info!(draft = id, "a newer claim took the registration"),
        }
        Ok(written)
    }

    /// Writes `change`, then steps again; a run whose row a newer claim
    /// took stops.
    async fn advance(
        &self,
        row: &RegistrationRow,
        change: RegistrationChange,
    ) -> Result<Step, PlatformError> {
        self.advance_from(row, move |_, _| Ok(change)).await
    }

    /// [`Self::advance`] with the change [`Self::write_from`] makes.
    async fn advance_from(
        &self,
        row: &RegistrationRow,
        change: impl FnOnce(&AppDb, &Flows) -> Result<RegistrationChange, PlatformError>
        + Send
        + 'static,
    ) -> Result<Step, PlatformError> {
        #[cfg(test)]
        if self
            .flows()
            .with_held(self.claim, |s| {
                s.write_faults = s.write_faults.checked_sub(1)?;
                Some(())
            })
            .flatten()
            .is_some()
        {
            return Err(PlatformError::Storage {
                detail: "a test's fault".into(),
            });
        }
        Ok(match self.write_from(row, change).await? {
            Written::Lost => Step::Stop,
            Written::Applied | Written::Moved => Step::Again,
        })
    }

    async fn to_phase(&self, row: &RegistrationRow, phase: Phase) -> Result<Step, PlatformError> {
        self.advance(
            row,
            RegistrationChange {
                phase: Some(phase.as_str().into()),
                ..Default::default()
            },
        )
        .await
    }

    async fn step(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        // DEC-171 (3): a row this build cannot run is left as it is; its
        // status reads the failure.
        let Ok((phase, funding)) = interpret(row) else {
            return Ok(Step::Stop);
        };
        // §5.2: terminal evidence for the row first; a write steps again.
        if matches!(
            phase,
            Phase::KeysPrepared | Phase::FundingSent | Phase::ProofWaiting
        ) && matches!(funding, Funding::CoreBalance { .. })
            && self.reconcile().await?
        {
            return Ok(Step::Again);
        }
        // DEC-171 (4): a lock's outpoint makes the row funded, whatever
        // phase an import or a restore left it in.
        if phase == Phase::KeysPrepared
            && matches!(funding, Funding::CoreBalance { .. })
            && outpoint_of(row).is_some()
        {
            return self.to_phase(row, Phase::FundingSent).await;
        }
        match phase {
            Phase::Draft => self.prepare_keys(row, &funding).await,
            Phase::KeysPrepared => match funding {
                Funding::CoreBalance { lock_duffs } => self.fund(row, lock_duffs).await,
                _ => Ok(Step::Fail(internal(), false)),
            },
            Phase::FundingSent | Phase::ProofWaiting => self.create_identity(row, phase).await,
            Phase::IdentityRegistered if row.label.is_empty() => self.profile(row).await,
            Phase::IdentityRegistered => self.to_phase(row, Phase::NameRequested).await,
            // The name step settled without the name (MJ-1): the profile next.
            Phase::NameRequested if row.name_error.is_some() => self.profile(row).await,
            Phase::NameRequested => self.names(row).await,
            Phase::NameRegistered | Phase::Contested => self.profile(row).await,
            Phase::ProfileCreated => self.to_phase(row, Phase::Done).await,
            Phase::Done => Ok(Step::Stop),
        }
    }

    /// "Unlock to finish" on a vault without its full key, otherwise
    /// "Confirm to finish".
    fn authority_wait(&self) -> RegistrationWait {
        authority_wait(self.session)
    }

    /// What a failed library call means for the flow (not the funding call:
    /// see [`Self::fund`]). Identity creation and names are idempotent by
    /// construction (the lock creates one identity; names ask Platform
    /// first), so their other failures may be retried.
    fn failed(&self, e: &PlatformError) -> Step {
        match e {
            PlatformError::NeedsGrant { .. }
            | PlatformError::Cancelled
            | PlatformError::LeaseRevoked { .. }
            | PlatformError::LeaseExpired
            | PlatformError::GrantInvalid
            | PlatformError::SignerUnavailable => Step::Park(self.authority_wait()),
            PlatformError::Unavailable
            | PlatformError::Timeout
            | PlatformError::ContextUnavailable => Step::Wait(RegistrationWait::Network),
            other => Step::Fail(other.code(), true),
        }
    }

    /// The funding hold as a step: the sync is waited for in the run, the
    /// others park it.
    fn funding_hold(&self) -> Option<Step> {
        Some(match self.lib.funding_hold()? {
            RegistrationWait::Sync => Step::Wait(RegistrationWait::Sync),
            other => Step::Park(other),
        })
    }

    async fn identity_at(&self, index: u32) -> Option<String> {
        self.lib
            .identities()
            .await
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, id)| id)
    }

    /// Draft → KeysPrepared: the identity index, chosen only after the
    /// sync hold, so DP1-05's discovery has found every index the seed
    /// used. An existing identity skips the funding.
    async fn prepare_keys(
        &self,
        row: &RegistrationRow,
        funding: &Funding,
    ) -> Result<Step, PlatformError> {
        if let Funding::ExistingIdentity = funding {
            let identity = row.identity.as_deref().unwrap_or_default();
            let found = self
                .lib
                .identities()
                .await
                .into_iter()
                .find(|(_, id)| id == identity);
            let Some((index, id)) = found else {
                return Ok(Step::Fail(
                    PlatformError::Identity(super::errors::IdentityError::NotFound).code(),
                    false,
                ));
            };
            return self
                .advance(
                    row,
                    RegistrationChange {
                        phase: Some(Phase::IdentityRegistered.as_str().into()),
                        identity_index: Some(index),
                        identity: Some(id),
                        ..Default::default()
                    },
                )
                .await;
        }
        if let Some(step) = self.funding_hold() {
            return Ok(step);
        }
        self.assign_index(row).await
    }

    /// Writes the run's own row's [`Reconcile`] (§5.2), under the run's
    /// claim, in one job with the ledger it reads: whether it wrote.
    async fn reconcile(&self) -> Result<bool, PlatformError> {
        let (locks, identities) = (self.lib.tracked_locks().await, self.lib.identities().await);
        let (wallet, draft, owner) = (self.wallet, self.draft(), self.claim.owner);
        let leases = Arc::clone(&self.session.leases);
        let now = crate::events::unix_now();
        let (wrote, closes) = ledger_op(self.session, wallet, move |db, flows| {
            let ledger = Ledger::read(db, flows, &leases, wallet, locks, identities)?;
            let Some(row) = ledger.row(draft) else {
                return Ok((false, false));
            };
            let Some(reconcile) = ledger.reconcile().into_iter().find(|r| r.row() == draft) else {
                return Ok((false, false));
            };
            let wrote = apply_reconcile(db, &wallet.to_string(), row, owner, &reconcile, now)
                .map_err(db_error)?;
            Ok((wrote, wrote && reconcile.closes()))
        })
        .await?;
        if closes {
            backup_again(self.session, wallet, draft);
        }
        if wrote {
            signal(self.session, wallet, draft);
        }
        Ok(wrote)
    }

    /// Writes `KeysPrepared` with a new identity index, one past every used
    /// one, chosen from the rows read in the write's own job: no two flows
    /// take one index.
    async fn assign_index(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        let (locks, identities) = (self.lib.tracked_locks().await, self.lib.identities().await);
        let (wallet, leases) = (self.wallet, Arc::clone(&self.session.leases));
        self.advance_from(row, move |db, flows| {
            // The steps too: no new index lands on a step's slot (I12).
            let ledger = Ledger::read(db, flows, &leases, wallet, locks, identities)?;
            Ok(RegistrationChange {
                phase: Some(Phase::KeysPrepared.as_str().into()),
                identity_index: Some(ledger.next_index()),
                ..Default::default()
            })
        })
        .await
    }

    /// A storage fault a test set for the next lock record.
    #[cfg(test)]
    fn lock_write_fault(&self) -> Option<PlatformError> {
        self.flows()
            .with_held(self.claim, |s| {
                s.lock_write_faults = s.lock_write_faults.checked_sub(1)?;
                Some(PlatformError::Storage {
                    detail: "a test's fault".into(),
                })
            })
            .flatten()
    }

    /// Records the flow's lock: the outpoint and `phase`, then the
    /// automatic backup of §3.4 Restore rule 4. A failed write is retried;
    /// one that keeps failing ends the run as a storage error, the row
    /// unfailed at its index, where resume and "Finish transfers" find the
    /// lock again (MN-1).
    async fn record_lock(
        &self,
        row: &RegistrationRow,
        outpoint: OutPoint,
        phase: Phase,
    ) -> Result<Step, PlatformError> {
        let change = RegistrationChange {
            phase: Some(phase.as_str().into()),
            outpoint: Some(canonical(outpoint)),
            ..Default::default()
        };
        let mut tries = 1;
        let step = loop {
            #[cfg(test)]
            let written = match self.lock_write_fault() {
                Some(e) => Err(e),
                None => self.advance(row, change.clone()).await,
            };
            #[cfg(not(test))]
            let written = self.advance(row, change.clone()).await;
            match written {
                Ok(step) => break step,
                Err(e) if tries < LOCK_WRITE_TRIES => {
                    tracing::warn!(draft = row.id, error = %e, "retrying the lock's record");
                    tokio::time::sleep(LOCK_WRITE_RETRY * tries).await;
                    tries += 1;
                }
                Err(e) => return Err(e),
            }
        };
        backup_once(self.session, self.wallet, row.id);
        Ok(step)
    }

    /// Marks the funding call of the run, on the actor with the row read
    /// there (DEC-173): unchanged since the funding gate read it, so no
    /// evidence written meanwhile ("Finish transfers" giving it a lock) is
    /// funded over, and none is written to it while the call runs
    /// ([`import_stranded`]). Otherwise the step: `Again` for a changed
    /// row of the run's generation, `Stop` once the generation is lost.
    async fn enter_funding(
        &self,
        row: &RegistrationRow,
    ) -> Result<Result<InCall<'_>, Step>, PlatformError> {
        let (wallet, key, run, owner, phase) = (
            self.wallet.to_string(),
            self.claim.key,
            self.claim.run,
            self.claim.owner,
            row.phase.clone(),
        );
        let entered = rows_op(self.session, self.wallet, move |db, flows| {
            let Some(r) = db
                .registration_row(&wallet, key.1)?
                .filter(|r| r.owner == owner)
            else {
                return Ok(Err(Step::Stop));
            };
            if r.phase != phase || r.asset_lock_outpoint.is_some() || r.error.is_some() {
                return Ok(Err(Step::Again));
            }
            Ok(flows
                .with_run(key, run, |s| s.in_call = true)
                .ok_or(Step::Stop))
        })
        .await?;
        Ok(entered.map(|()| InCall(self.flows(), self.claim)))
    }

    /// KeysPrepared → FundingSent: the one step that commits funds.
    async fn fund(&self, row: &RegistrationRow, lock_duffs: u64) -> Result<Step, PlatformError> {
        if let Some(step) = self.funding_hold() {
            return Ok(step);
        }
        let Some(index) = row.identity_index else {
            return Ok(Step::Fail(internal(), false));
        };
        let ledger = self.ledger().await?;
        match ledger.standing(row) {
            Standing::Unfunded => {}
            // The lock of a step of the row's (its own, or one it
            // inherited), whatever its status or index says (DP1-02 §6);
            // else it is away or not known: the row waits for it and never
            // funds again (I1, I4).
            Standing::Awaiting => {
                return match ledger.own_lock(row) {
                    Some(lock) => {
                        self.record_lock(row, lock.outpoint, Phase::FundingSent)
                            .await
                    }
                    // §5.2a: its consumed lock waits for discovery; the
                    // reconcile at the top of the step attaches it.
                    None if ledger.awaits_discovery(row) => {
                        Ok(Step::Wait(RegistrationWait::Network))
                    }
                    None => self.maybe_sent().await,
                };
            }
            Standing::Closed => return self.maybe_sent().await,
            // `step` records an outpoint first, and §5.2b closes a cancelled
            // row; nothing else funds.
            Standing::Funded(_)
            | Standing::Settled
            | Standing::Cancelled(_)
            | Standing::Unreadable { .. } => {
                return Ok(Step::Stop);
            }
        }
        match ledger.index_owner(row, index) {
            IndexOwner::Free => {}
            // §3.4 Restore rule 2: what an earlier run left.
            IndexOwner::Lock(outpoint) => {
                return self.record_lock(row, outpoint, Phase::FundingSent).await;
            }
            // Another flow funded this index (a restored row behind
            // reality whose lock "Finish transfers" adopted): this one
            // takes a new index rather than a second lock for it.
            IndexOwner::Taken => return self.assign_index(row).await,
            // What is at the index cannot be attributed yet: it waits for
            // "Finish transfers".
            IndexOwner::Unknown => return Ok(Step::Wait(ledger.unattributed_wait(row))),
        }
        // P2b's permit refuses a step its marker says may be out: the
        // backstop of the standing above.
        let Some(lease) = self.lease() else {
            return Ok(Step::Park(self.authority_wait()));
        };
        // The funding cap (E0-04 §2a.3): the lock plus the worst-case fee
        // must fit the Funding budget, which the call permit charges.
        // Checked first, so a lock past the cap fails retryable with its
        // figures.
        let spendable = match self.lib.spendable().await {
            Ok(spendable) => spendable,
            Err(e) => return Ok(self.failed(&e)),
        };
        let needed = lock_duffs.saturating_add(fee_bound_worst(spendable.inputs));
        let remaining = match lease.available(BudgetPurpose::Funding) {
            Ok(remaining) => remaining,
            Err(e) => return Ok(self.failed(&e.into())),
        };
        if needed > remaining {
            return Ok(Step::Fail(
                PlatformError::GrantExceeded {
                    purpose: BudgetPurpose::Funding,
                    needed,
                    remaining,
                }
                .code(),
                true,
            ));
        }
        if spendable.protected > 0 {
            return Ok(Step::Fail(RegistrationError::CoinsLocked.code(), true));
        }
        if spendable.duffs < needed {
            return Ok(Step::Fail(
                RegistrationError::FundingInsufficient {
                    needed,
                    available: spendable.duffs,
                }
                .code(),
                true,
            ));
        }
        // Held until the outcome is written: a discard deciding meanwhile
        // would read an unfunded row whose lock may be out.
        let _in_call = match self.enter_funding(row).await? {
            Ok(in_call) => in_call,
            Err(step) => return Ok(step),
        };
        self.flows().with_held(self.claim, |s| {
            s.is_until = Some(Instant::now() + IS_WINDOW);
            s.wait = Some(RegistrationWait::InstantSend);
        });
        // The call holds P2b's funding permit (E0-04 §2a.6): its marker is
        // durable before the library starts, and the lock its funding
        // signer signs is recorded as the step's own before the persister
        // takes it.
        let step = funding_step(row.id);
        let funded = lease
            .scope_step(Some(step.clone()), async {
                let key = DraftKey::registration(row.id, index).key;
                let permit = self
                    .lib
                    .funding_permit(&lease, step.clone(), key, lock_duffs)
                    .await?;
                let funded = self.lib.fund(lock_duffs, index, &permit).await;
                permit.finish(funded.is_ok());
                funded
            })
            .await;
        // platform-wallet tracks the lock (`Built`, stored through the tap)
        // before it hands it to the broadcaster, and untracks it only on a
        // rejection (`build.rs` `broadcast_funded_asset_lock_with_funding`).
        // After the call a fresh ledger decides (DP1-02 §6, the post-call
        // rule), the row still calling: its live outpoint (the tap or
        // "Finish transfers" wrote it), else its own lock, else its step's
        // claim, and only for a `None` claim the call's error. Never the
        // call's result alone (I3).
        let Some(current) = self.row().await? else {
            return Ok(Step::Stop);
        };
        let ledger = self.ledger().await?;
        let signed = ledger.signed_of(row.id);
        if let Ok(returned) = &funded
            && !signed.contains(&returned.txid)
        {
            // I3: a lock the row's step did not sign is never recorded.
            tracing::error!(
                draft = row.id,
                %returned,
                "the funding call returned a lock its step did not sign"
            );
        }
        let phase = || match funded {
            Err(FundError::IsTimeout) => {
                // The window is over. A call that ended sooner (an unknown
                // broadcast outcome) keeps the rest of it for the proof
                // wait.
                self.flows().with_held(self.claim, |s| s.is_until = None);
                self.drop_own_key(&lease);
                Phase::ProofWaiting
            }
            _ => Phase::FundingSent,
        };
        if let Some(live) = ledger.live(&current) {
            if signed.contains(&live.txid) {
                return self.record_lock(row, live, phase()).await;
            }
            // D13: a live outpoint the row's step did not sign was written
            // before the call by a `may_take` path (trusted by induction,
            // I3): the run goes on with it.
            tracing::warn!(draft = row.id, %live, "the row holds a lock its call did not sign");
            return Ok(Step::Again);
        }
        if let Some(own) = ledger.own_lock(&current) {
            let own = own.outpoint;
            return self.record_lock(row, own, phase()).await;
        }
        if ledger.claim_of(row.id) != StepClaim::None {
            return self.maybe_sent().await;
        }
        // `cancelled(S)` (§3.2): Repair resolved the step once its call's
        // permit ended (Repair refuses while it runs), or P2b refused the
        // call `StepCancelled`. It is never called again whatever the call
        // returned (a rejection alone would make the row retryable, H12);
        // the next step, the call over, closes the row (§3.3 row 8).
        if ledger.step_cancelled(row.id) {
            return Ok(Step::Again);
        }
        match funded {
            // Refused before the call: nothing was signed. A step that may
            // be out already (an earlier call of the draft) waits.
            Err(FundError::Refused(
                PlatformError::BroadcastUnknown { artifact }
                | PlatformError::WillBeSent { artifact },
            )) if artifact == step => self.maybe_sent().await,
            // P2b's gate named another tracked lock the catch-up has not
            // resent yet (`FundingBlock::Row`, by its txid): nothing of the
            // step's is out, and the funding waits for the catch-up.
            Err(FundError::Refused(PlatformError::WillBeSent { .. })) => {
                Ok(Step::Wait(RegistrationWait::Sync))
            }
            Err(FundError::Refused(e)) => Ok(self.failed(&e)),
            Err(FundError::CoinsLocked) => {
                Ok(Step::Fail(RegistrationError::CoinsLocked.code(), true))
            }
            // H12: P2b's `NotSent` row superseded the marker and nothing was
            // signed, so the same step may retry.
            Err(FundError::NotSent(e)) => {
                tracing::info!(draft = row.id, "the funding never reached the wire: {e}");
                self.unfund(e.code()).await
            }
            // An `Ok` the step cannot back, or another failure: fail closed.
            other => {
                tracing::warn!(
                    draft = row.id,
                    "the funding call ended untracked: {other:?}"
                );
                self.maybe_sent().await
            }
        }
    }

    /// Moves a flow whose lock never reached the wire back to
    /// `KeysPrepared`, retryable, clearing the outpoint the tap may have
    /// recorded while the lock was tracked. The automatic backup taken for
    /// that lock no longer holds the flow's funding: the next lock's is
    /// scheduled again (MN-2).
    async fn unfund(&self, code: &'static str) -> Result<Step, PlatformError> {
        let Some(row) = self.row().await? else {
            return Ok(Step::Stop);
        };
        if row.error.is_some() {
            return Ok(Step::Stop);
        }
        let change = RegistrationChange {
            phase: Some(Phase::KeysPrepared.as_str().into()),
            clear_outpoint: true,
            failure: Some(Some((code.into(), true))),
            ..Default::default()
        };
        if self.write(&row, change).await? == Written::Applied {
            self.flows().with_held(self.claim, |s| s.backed_up = false);
        }
        Ok(Step::Stop)
    }

    /// A funding that may be out with no tracked row behind it: a row with
    /// the outpoint waits for the lock to be tracked again (its outpoint
    /// reads `MaybeSent`); one without fails `platform.broadcast_unknown`,
    /// not retryable, until the lock is tracked (the tap adopts it) or
    /// positive evidence says `NotSent`.
    async fn maybe_sent(&self) -> Result<Step, PlatformError> {
        let Some(row) = self.row().await? else {
            return Ok(Step::Stop);
        };
        if row.error.is_some() {
            return Ok(Step::Stop);
        }
        if row.asset_lock_outpoint.is_some() {
            return Ok(Step::Again);
        }
        self.fail(&row, broadcast_unknown(), false).await?;
        Ok(Step::Stop)
    }

    /// An own-key lease drops its key when the InstantSend window ends
    /// (§2.6): the flow parks keyless and asks again at identity creation.
    fn drop_own_key(&self, lease: &Lease) {
        if holds_own_key(lease) {
            lease.park();
        }
    }

    /// FundingSent / ProofWaiting → IdentityRegistered.
    async fn create_identity(
        &self,
        row: &RegistrationRow,
        phase: Phase,
    ) -> Result<Step, PlatformError> {
        let (Some(index), Some(outpoint)) = (row.identity_index, outpoint_of(row)) else {
            return Ok(Step::Fail(internal(), false));
        };
        let registered = |id: String| RegistrationChange {
            phase: Some(Phase::IdentityRegistered.as_str().into()),
            identity: Some(id),
            ..Default::default()
        };
        // Only the identity this lock makes: another at the index is not
        // this flow's.
        let own = b58(&identity_for(outpoint));
        if self.identity_at(index).await.as_ref() == Some(&own) {
            return self.advance(row, registered(own)).await;
        }
        match self.lib.find_identity(outpoint).await {
            Ok(Some(id)) => return self.advance(row, registered(id)).await,
            Ok(None) => {}
            Err(e) => return Ok(self.failed(&e)),
        }
        let locks = self.lib.tracked_locks().await;
        let Some(lock) = locks.iter().find(|l| l.outpoint == outpoint) else {
            // Its row was lost, or untracked as swept (Mode B residual):
            // DP1-05's reconstruction tracks it again once it is on the
            // chain, and the run looks again.
            return Ok(Step::Wait(RegistrationWait::ChainLock));
        };
        if lock.status == LockStatus::Consumed {
            // Platform took the lock; its identity is not visible yet.
            return Ok(Step::Wait(RegistrationWait::Network));
        }
        if !lock.status.has_proof() {
            if phase == Phase::FundingSent {
                return self.to_phase(row, Phase::ProofWaiting).await;
            }
            return Ok(self.wait_proof(outpoint, lock.status).await);
        }
        let Some(lease) = self.lease() else {
            return Ok(Step::Park(self.authority_wait()));
        };
        // The call holds P2b's step permit (E0-04 §2a.3): the step's marker
        // is durable before the library starts, and the permit's identity
        // signer counts what it releases. Step 2's ChainLock-height and
        // IS→CL retries re-sign inside the one call, so an earlier attempt
        // may execute: a call that released a signature and then failed
        // reads `MaybeSent`, and one whose signer met the lock parks the
        // flow. Either way the identity is looked for first (it is the
        // lock's: Platform creates it once), so the next run never pays
        // twice.
        let in_call = match self.flows().enter_call(self.claim) {
            Ok(in_call) => in_call,
            Err(step) => return Ok(step),
        };
        let step = identity_step(row.id);
        let (created, locked) = lease
            .scope_step(Some(step.clone()), async {
                let call = (lease.call_permit_step(&step, None, CallStep::Resumable(step.clone())))
                    .await
                    .map_err(PlatformError::from)?;
                let created = self.lib.register_identity(outpoint, index, &call).await;
                let locked = call.signatures().locked();
                call.finish(created.is_ok());
                Ok::<_, PlatformError>((created, locked))
            })
            .await
            .unwrap_or_else(|e| (Err(e), false));
        drop(in_call);
        match created {
            Ok(id) => self.advance(row, registered(id)).await,
            Err(e) => {
                if let Ok(Some(id)) = self.lib.find_identity(outpoint).await {
                    return self.advance(row, registered(id)).await;
                }
                // A signer that met the lock parks the flow: the permit's
                // counted signers say so, the proof's uncounted one only
                // by the vault's state.
                if locked || self.authority_wait() == RegistrationWait::Unlock {
                    return Ok(Step::Park(self.authority_wait()));
                }
                Ok(self.failed(&e))
            }
        }
    }

    /// A funding call of this process signed the tracked lock at `outpoint`
    /// and its record is not durable yet (P2b's resume gate).
    async fn unrecorded(&self, outpoint: OutPoint) -> bool {
        let Some(row) = self.lib.tracked_row(outpoint).await else {
            return false;
        };
        (self.session.leases)
            .unrecorded_signer_of(self.wallet, &row)
            .is_some()
    }

    /// The proof wait: InstantSend while the in-memory window lasts, then
    /// the ChainLock with no limit, in bounded waits. No key is needed; an
    /// own key is dropped when the window ends (§2.6), and a key given
    /// later lasts its own hold.
    async fn wait_proof(&self, outpoint: OutPoint, status: LockStatus) -> Step {
        let (key, draft) = (self.claim.key, self.draft());
        // `wait_proof` broadcasts a `Built` lock again: never one a funding
        // call of this process signed before its record is durable (E0-04
        // §2a.7). The record lands, or the call ends, and the run looks
        // again.
        if status == LockStatus::Built && self.unrecorded(outpoint).await {
            return Step::Wait(RegistrationWait::FundingUnknown);
        }
        let now = Instant::now();
        let window = self
            .flows()
            .get(key)
            .is_until
            .filter(|until| *until > now)
            .map(|until| until - now);
        let wait = match window {
            Some(_) => RegistrationWait::InstantSend,
            None => RegistrationWait::ChainLock,
        };
        if !self.waits {
            // A `Built` lock is broadcast again, the proof not waited for.
            if status == LockStatus::Built {
                match self.lib.wait_proof(outpoint, REBROADCAST_WAIT).await {
                    Ok(()) => return Step::Again,
                    Err(FundError::IsTimeout) => {}
                    Err(e) => tracing::info!(draft, "the lock's re-broadcast failed: {e:?}"),
                }
            }
            return Step::Park(wait);
        }
        self.flows().with_held(self.claim, |s| s.wait = Some(wait));
        match self
            .lib
            .wait_proof(outpoint, window.unwrap_or(CHAINLOCK_POLL))
            .await
        {
            Ok(()) => Step::Again,
            Err(FundError::IsTimeout) => {
                if window.is_some() {
                    self.flows().with_held(self.claim, |s| s.is_until = None);
                    if let Some(lease) = self.lease() {
                        self.drop_own_key(&lease);
                    }
                }
                Step::Rewait
            }
            Err(FundError::Refused(e) | FundError::NotSent(e) | FundError::Failed(e)) => {
                self.failed(&e)
            }
            Err(FundError::CoinsLocked) => Step::Fail(RegistrationError::CoinsLocked.code(), true),
        }
    }

    /// NameRequested → NameRegistered | Contested, then the temporary name.
    /// The identity exists, so a name Platform refuses for good settles the
    /// step (MJ-1, [`Self::after_identity`]): its code goes to `name_error`
    /// and the flow goes on to the profile; the identity claims a name
    /// through DP1-03's `register_name`. Any other failure waits.
    async fn names(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        let Some(identity) = row.identity.as_deref() else {
            return Ok(Step::Fail(internal(), false));
        };
        let Some(lease) = self.lease() else {
            return Ok(Step::Park(self.authority_wait()));
        };
        let _in_call = match self.flows().enter_call(self.claim) {
            Ok(in_call) => in_call,
            Err(step) => return Ok(step),
        };
        let failed = match self.lib.register_name(identity, &row.label, &lease).await {
            Ok(NameOutcome::Registered) => return self.to_phase(row, Phase::NameRegistered).await,
            Ok(NameOutcome::ContestStarted { .. }) => {
                if let Some(temp) = row.temp_label.as_deref() {
                    match self.lib.register_name(identity, temp, &lease).await {
                        Ok(_) => {}
                        Err(NameError::Platform(e)) => {
                            let step = self.after_identity(&e)?;
                            if !matches!(step, Step::Fail(..)) {
                                return Ok(step);
                            }
                            tracing::info!(draft = row.id, "no temporary name: {e}");
                        }
                        // The temporary name is a courtesy: losing it to
                        // another owner leaves the contest as it is.
                        Err(e) => tracing::info!(draft = row.id, "no temporary name: {e}"),
                    }
                }
                return self.to_phase(row, Phase::Contested).await;
            }
            Err(NameError::Platform(e)) => self.after_identity(&e)?,
            // DP1-03's typed refusals: a retry meets them again.
            Err(
                e @ (NameError::Taken
                | NameError::ContestOpen
                | NameError::Locked
                | NameError::Invalid { .. }
                | NameError::UnavailableForInvite),
            ) => Step::Fail(e.code(), false),
        };
        self.settle(row, failed, |code| RegistrationChange {
            name_error: Some(code.into()),
            ..Default::default()
        })
        .await
    }

    /// The initial profile, if any, then Done. Idempotent against Platform
    /// (MN-3), which refuses a second profile document: one Platform has
    /// that already says what the row's does counts as published, a
    /// different one is revised under the update rules. A refusal positively
    /// known to repeat settles the step (MJ-1, [`Self::after_identity`]):
    /// the flow ends `Done` with its code in `profile_error`. A failed read
    /// never settles it.
    async fn profile(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        let Some(text) = row.initial_profile.as_deref() else {
            return self.to_phase(row, Phase::Done).await;
        };
        let Decoded::Known(profile) = ProfileRecord::decode(text) else {
            return Ok(Step::Fail(unsupported(), false));
        };
        let (Some(identity), Some(index)) = (row.identity.as_deref(), row.identity_index) else {
            return Ok(Step::Fail(internal(), false));
        };
        let settled = |code: &'static str| RegistrationChange {
            phase: Some(Phase::Done.as_str().into()),
            profile_error: Some(code.into()),
            ..Default::default()
        };
        let existing = match self.lib.platform_profile(identity).await {
            Ok(existing) => existing,
            Err(e) => return self.after_read(&e),
        };
        if existing.as_ref().is_some_and(|p| profile_says(p, &profile)) {
            return self.profile_created(row).await;
        }
        let Some(lease) = self.lease() else {
            return Ok(Step::Park(self.authority_wait()));
        };
        let _in_call = match self.flows().enter_call(self.claim) {
            Ok(in_call) => in_call,
            Err(step) => return Ok(step),
        };
        let put = match existing {
            None => {
                self.lib
                    .create_profile(identity, index, &profile, &lease)
                    .await
            }
            Some(_) => {
                self.lib
                    .update_profile(identity, index, &profile, &lease)
                    .await
            }
        };
        match put {
            Ok(()) => self.profile_created(row).await,
            Err(e) => {
                // An earlier put may have landed meanwhile (one whose
                // outcome was unknown): Platform's answer decides, and a
                // failed read decides nothing (Sol r6 R6-04).
                match self.lib.platform_profile(identity).await {
                    Ok(Some(p)) if profile_says(&p, &profile) => {
                        return self.profile_created(row).await;
                    }
                    Ok(_) => {}
                    Err(read) => return self.after_read(&read),
                }
                // Only a refusal positively known to repeat settles: a
                // duplicate or a revision race (an absence read before the
                // put landed) is no such refusal, so it waits for a read
                // that reconciles it.
                self.settle(row, self.after_identity(&e)?, settled).await
            }
        }
    }

    async fn profile_created(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        self.advance(
            row,
            RegistrationChange {
                phase: Some(Phase::ProfileCreated.as_str().into()),
                clear_initial_profile: true,
                ..Default::default()
            },
        )
        .await
    }

    /// A library error at a step after the identity exists (MJ-1). Only a
    /// refusal positively known to repeat ([`refused_for_good`]) fails the
    /// step, which `settle` settles. A closed session stops the run and a
    /// storage error ends it, the row as it was; an authority the run lacks
    /// parks it. Anything else waits for the network (both steps read
    /// Platform before they write, so a retry finds what landed): a write
    /// whose outcome is unknown, a proof or trust failure, the network, and
    /// any error the engine cannot classify (`internal`: an execution the
    /// SDK could not prove, a nonce a lagging node lacks, the library's
    /// flattened DPNS error, Sol r6 R6-02).
    fn after_identity(&self, e: &PlatformError) -> Result<Step, PlatformError> {
        Ok(match e {
            PlatformError::NetworkNotOpen | PlatformError::WalletNotFound => Step::Stop,
            PlatformError::Storage { .. } => return Err(e.clone()),
            e if refused_for_good(e) => Step::Fail(e.code(), false),
            e => match self.failed(e) {
                Step::Fail(..) => Step::Wait(RegistrationWait::Network),
                step => step,
            },
        })
    }

    /// A failed read of Platform at a step after the identity exists: as
    /// [`Self::after_identity`], except that it never settles the step
    /// (Sol r6 R6-04): what the read would have said is unknown, so the step
    /// waits to read again.
    fn after_read(&self, e: &PlatformError) -> Result<Step, PlatformError> {
        Ok(match self.after_identity(e)? {
            Step::Fail(..) => Step::Wait(RegistrationWait::Network),
            step => step,
        })
    }

    /// A step after the identity exists ended with `step`. A failure
    /// settles it (MJ-1): the identity's funds are spent, so nothing holds
    /// the flow; `settle` records the code and the flow goes on. A wait or
    /// a park is the step's own.
    async fn settle(
        &self,
        row: &RegistrationRow,
        step: Step,
        settle: impl FnOnce(&'static str) -> RegistrationChange,
    ) -> Result<Step, PlatformError> {
        match step {
            Step::Fail(code, _) => {
                tracing::info!(draft = row.id, phase = %row.phase, code, "registration step settled");
                self.advance(row, settle(code)).await
            }
            step => Ok(step),
        }
    }
}

/// A refusal at a step after the identity exists that a retry would meet
/// again, positively identified by its kind (Sol r6 R6-02): the allow-list
/// that settles a step. A budget or credits short of the step, an argument
/// the library or Platform refuses as invalid, a seed that does not match,
/// a feature off or not built, an identity without the key the step signs
/// with. Nothing flattened into `internal` is among them.
pub(crate) fn refused_for_good(e: &PlatformError) -> bool {
    matches!(
        e,
        PlatformError::GrantExceeded { .. }
            | PlatformError::InsufficientCredits { .. }
            | PlatformError::InvalidArgument { .. }
            | PlatformError::SeedMismatch
            | PlatformError::FeatureOff { .. }
            | PlatformError::NotImplemented { .. }
            | PlatformError::Identity(super::errors::IdentityError::KeysMissing { .. })
    )
}

/// Platform's profile already says what `wanted` sets; the fields `wanted`
/// leaves out are not compared (an update would leave them as they are).
fn profile_says(platform: &ProfileRecord, wanted: &ProfileRecord) -> bool {
    let says =
        |wanted: &Option<String>, platform: &Option<String>| wanted.is_none() || wanted == platform;
    says(&wanted.display_name, &platform.display_name)
        && says(&wanted.public_message, &platform.public_message)
        && says(&wanted.avatar_url, &platform.avatar_url)
}

/// `Registration{draft}` for hosts that show the row (DASHPAY §3.5).
pub(crate) fn signal(session: &NetworkSession, wallet: WalletId, draft: i64) {
    session.hub.pump.mark_platform(
        wallet,
        [crate::PlatformChange::Registration {
            draft: Some(draft.to_string()),
        }],
    );
}

/// "Unlock to finish" on a vault without its full key, otherwise
/// "Confirm to finish".
pub(crate) fn authority_wait(session: &NetworkSession) -> RegistrationWait {
    match session.vault.lock_state() {
        LockState::Locked | LockState::UnlockedMixingOnly => RegistrationWait::Unlock,
        _ => RegistrationWait::Authorize,
    }
}

/// Claims `draft` and runs it in a task; nothing while another holds the
/// flow, or once the row is gone.
pub(crate) async fn start_run<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    draft: i64,
    lease: Option<Lease>,
) -> Result<(), PlatformError> {
    let flows = &session.platform.registrations;
    if let Some(claim) = flows.claim(session, (wallet, draft)).await? {
        spawn(session, lib, claim, lease);
    }
    Ok(())
}

/// Runs `claim`'s flow (from [`Flows::claim`]) in a task.
pub(crate) fn spawn<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    lib: Arc<L>,
    claim: Claim,
    lease: Option<Lease>,
) {
    let (key, run) = (claim.key, claim.run);
    let task = {
        let session = Arc::clone(session);
        let lease = lease.clone();
        session.rt.clone().spawn(async move {
            let driver = Driver::new(&session, &*lib, &claim, lease, true);
            if let Err(e) = driver.run().await {
                tracing::warn!(draft = key.1, error = %e, "registration stopped");
            }
        })
    };
    let handle = task.abort_handle();
    session.platform.registrations.with_existing(key, |s| {
        if s.holder.map(|h| h.run) == Some(run) {
            s.task = Some(handle.clone());
        }
    });
    if let Some(lease) = &lease {
        lease.register_task(handle);
    }
}

/// Schedules the automatic backup of a flow that reached `FundingSent`
/// (§3.4 Restore rule 4), once per flow and process.
pub(crate) fn backup_once(session: &Arc<NetworkSession>, wallet: WalletId, draft: i64) {
    let first = session.platform.registrations.with((wallet, draft), |s| {
        let first = !std::mem::replace(&mut s.backed_up, true);
        #[cfg(test)]
        if first {
            s.backups += 1;
        }
        first
    });
    if first {
        session.schedule_automatic_backup(wallet);
    }
}

/// The lease holds its own vault key now (§2.6).
pub(crate) fn holds_own_key(lease: &Lease) -> bool {
    lease.view().is_some_and(|v| {
        v.own_key
            && matches!(
                v.state,
                LeaseStateView::Active | LeaseStateView::AwaitingProof
            )
    })
}

fn parse_identity(identity: &str) -> Result<Identifier, PlatformError> {
    Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
        PlatformError::InvalidArgument {
            detail: "identity is not a base58 identifier".into(),
        }
    })
}

/// The base58 text of an identity id.
pub(crate) fn b58(id: &Identifier) -> String {
    id.to_string(Encoding::Base58)
}

/// `RegistrationLib` over platform-wallet and the SDK.
pub(crate) struct PwLib {
    pub(crate) session: Arc<NetworkSession>,
    pub(crate) wallet_id: WalletId,
    pub(crate) wallet: Arc<platform_wallet::PlatformWallet>,
    pub(crate) sdk: Arc<dash_sdk::Sdk>,
}

pub(crate) fn lock_status(
    s: &platform_wallet::wallet::asset_lock::tracked::AssetLockStatus,
) -> LockStatus {
    use platform_wallet::wallet::asset_lock::tracked::AssetLockStatus as S;
    match s {
        S::Built => LockStatus::Built,
        S::Broadcast => LockStatus::Broadcast,
        S::InstantSendLocked => LockStatus::InstantSendLocked,
        S::ChainLocked => LockStatus::ChainLocked,
        S::Consumed => LockStatus::Consumed,
        S::RecoveredFromChain => LockStatus::RecoveredFromChain,
    }
}

fn fund_error(e: platform_wallet::PlatformWalletError) -> FundError {
    use platform_wallet::PlatformWalletError as E;
    match e {
        E::FinalityTimeout(_) => FundError::IsTimeout,
        // The definite-rejection contract (`build.rs`: the row is gone, the
        // inputs are free), and the failures before the lock is built and
        // tracked.
        e @ (E::TransactionBroadcast(_)
        | E::AssetLockTransaction(_)
        | E::AssetLockInsufficientFunds { .. }
        | E::InputMidBroadcast { .. }
        | E::TransactionBuild(_)) => FundError::NotSent(e.into()),
        e => FundError::Failed(e.into()),
    }
}

/// The fields of `profile` the library publishes. Avatar bytes are DP4-02's;
/// a registration's profile names its avatar by URL only.
fn profile_update(profile: &ProfileRecord) -> platform_wallet::wallet::identity::ProfileUpdate {
    platform_wallet::wallet::identity::ProfileUpdate {
        display_name: profile.display_name.clone(),
        public_message: profile.public_message.clone(),
        avatar_url: profile.avatar_url.clone(),
        avatar_bytes: None,
    }
}

/// The DashPay system contract, parsed once, at the version platform-wallet
/// loads it (`PlatformVersion::latest()`).
fn dashpay_contract() -> Result<Arc<dpp::prelude::DataContract>, PlatformError> {
    static CONTRACT: std::sync::OnceLock<Arc<dpp::prelude::DataContract>> =
        std::sync::OnceLock::new();
    if let Some(contract) = CONTRACT.get() {
        return Ok(Arc::clone(contract));
    }
    let contract = dpp::system_data_contracts::load_system_data_contract(
        dpp::system_data_contracts::SystemDataContract::Dashpay,
        dpp::version::PlatformVersion::latest(),
    )
    .map_err(|e| PlatformError::Internal {
        detail: format!("the DashPay contract: {e}"),
    })?;
    Ok(Arc::clone(CONTRACT.get_or_init(|| Arc::new(contract))))
}

/// What a funding may spend of the engine's coin view. `protected` counts
/// the funding coins the library's builder would not skip
/// ([`crate::coins::CoinSnapshot::held_funding_coins`]: the users' and the
/// dust protection's locks, the sends' reservations); a tracked lock's
/// inputs are the library's own reservation, not among them.
fn spendable_of(snapshot: &crate::coins::CoinSnapshot) -> Spendable {
    let mut out = Spendable {
        protected: snapshot.held_funding_coins().count(),
        ..Spendable::default()
    };
    for c in snapshot.coins.iter().filter(|c| c.send_account) {
        out.inputs += 1;
        if c.auto_selectable(snapshot.height) {
            out.duffs = out.duffs.saturating_add(c.utxo.value());
        }
    }
    out
}

impl PwLib {
    /// The wallet's coin fence for a funding (E0-04 §2a.6, DEC-173 (3)),
    /// refused while a funding coin is held back (`held_funding_coins`):
    /// the pinned builder cannot be told to exclude one
    /// (`ReservationToken::next` is crate-private). The check and the
    /// selection see one policy: "Lock unspent", the dust locks and every
    /// engine build wait for the fence, which the funding permit keeps
    /// until the library tracks the lock the call signed (whose inputs then
    /// read reserved, until it is spent or untracked), or the call ends.
    pub(crate) async fn fence(&self) -> Result<CoinFence, FundError> {
        let refused = |e: crate::EngineError| FundError::Refused(e.into());
        let fence = self
            .session
            .coin_fence(self.wallet_id)
            .await
            .map_err(refused)?;
        let snapshot = self.session.fenced_coins(&fence).await.map_err(refused)?;
        if spendable_of(&snapshot).protected > 0 {
            return Err(FundError::CoinsLocked);
        }
        Ok(fence)
    }
}

impl RegistrationLib for PwLib {
    async fn tracked_locks(&self) -> Vec<LockInfo> {
        use platform_wallet::AssetLockFundingType;
        self.wallet
            .asset_locks()
            .list_tracked_locks()
            .await
            .into_iter()
            .map(|l| LockInfo {
                outpoint: l.out_point,
                registration: l.funding_type == AssetLockFundingType::IdentityRegistration,
                identity_index: l.identity_index,
                account: l.account_index,
                amount: l.amount,
                status: lock_status(&l.status),
            })
            .collect()
    }

    async fn identities(&self) -> Vec<(u32, String)> {
        let state = self.wallet.state().await;
        state
            .identity_manager
            .wallet_managed_identities(&self.wallet_id.0)
            .filter_map(|m| Some((m.identity_index?, b58(&m.identity.id()))))
            .collect()
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        let fence = self.session.coin_fence(self.wallet_id).await?;
        let snapshot = self.session.fenced_coins(&fence).await?;
        Ok(spendable_of(&snapshot))
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        use super::startup::StartupStatus as S;
        let synced = self
            .session
            .hub
            .tracker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .caught_up();
        if !synced {
            return Some(RegistrationWait::Sync);
        }
        // Restore rule 1: the same-seed discovery answered which identity
        // indexes the seed uses. One that did not holds the funding: it
        // runs again at the first unlock (no signers), or at the next start
        // (it never reached Platform, failed locally, or left indexes
        // unanswered).
        match self
            .session
            .dashpay_startup(&self.wallet_id)
            .map(|s| s.startup)
        {
            Ok(S::Ready | S::NoIdentity | S::PartialAccountsPending | S::SeedBindingUnverified) => {
                None
            }
            Ok(S::IdentityUnsettled) => Some(RegistrationWait::Unlock),
            Ok(S::PartialNoIdentity | S::DiscoveryFailed | S::IdentityScanIncomplete) => {
                Some(RegistrationWait::Network)
            }
            Ok(S::NotRun | S::Starting) | Err(_) => Some(RegistrationWait::Sync),
        }
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        lock_duffs: u64,
    ) -> Result<CallPermit, FundError> {
        let fence = self.fence().await?;
        self.session
            .funding_call_permit(lease, step.clone(), lock_duffs, step, key, fence)
            .await
            .map_err(FundError::Refused)
    }

    async fn fund(
        &self,
        lock_duffs: u64,
        identity_index: u32,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        use platform_wallet::AssetLockFundingType;
        let signer = permit
            .funding_signer()
            .map_err(|e| FundError::Refused(e.into()))?;
        self.wallet
            .asset_locks()
            .create_funded_asset_lock_proof(
                lock_duffs,
                0,
                AssetLockFundingType::IdentityRegistration,
                identity_index,
                &signer,
            )
            .await
            .map(|(_, _, outpoint)| outpoint)
            .map_err(fund_error)
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        self.wallet
            .asset_locks()
            .resume_asset_lock(&outpoint, Some(timeout))
            .await
            .map(|_| ())
            .map_err(|e| match e {
                // A `Built` lock's re-broadcast that reached no peer: the
                // lock stays tracked; the network is waited for.
                platform_wallet::PlatformWalletError::TransactionBroadcastUnconfirmed(_) => {
                    FundError::Failed(PlatformError::Unavailable)
                }
                e => fund_error(e),
            })
    }

    async fn tracked_row(&self, outpoint: OutPoint) -> Option<TrackedRow> {
        (self.wallet.asset_locks().list_tracked_locks().await.iter())
            .find(|l| l.out_point == outpoint)
            .map(TrackedRow::of)
    }

    async fn spv_synced(&self) -> bool {
        self.session.hub.tracker().caught_up()
    }

    async fn find_identity(&self, outpoint: OutPoint) -> Result<Option<String>, PlatformError> {
        use dash_sdk::platform::Fetch;
        let id = identity_for(outpoint);
        let fetched = dpp::identity::Identity::fetch(&self.sdk, id).await?;
        Ok(fetched.map(|i| b58(&i.id())))
    }

    async fn register_identity(
        &self,
        outpoint: OutPoint,
        identity_index: u32,
        permit: &CallPermit,
    ) -> Result<String, PlatformError> {
        use platform_wallet::wallet::asset_lock::AssetLockFunding;
        let identity_signer = permit.identity_signer(&[identity_index])?;
        // Uncounted, and no funding release recorded (E0-04 §2a.7): this
        // call builds no lock. The library resumes the tracked one, which
        // `create_identity` passes only with a proof, so nothing is sent,
        // and the funding key signs only the transition's asset lock proof
        // (the credit output's key). The step's status rests on that lock's
        // own signed record, which no signature here can contradict.
        let funding_signer = permit.lease().uncounted_funding_signer()?;
        let keys = identity_signer
            .derive_public_keys(identity_index, &super::keys_policy::registration_specs())
            .await
            .map_err(PlatformError::from)?;
        let identity = self
            .wallet
            .identity()
            .register_identity_with_funding(
                AssetLockFunding::FromExistingAssetLock {
                    out_point: outpoint,
                    consume_invitation_voucher: false,
                },
                identity_index,
                keys,
                &identity_signer,
                &funding_signer,
                None,
            )
            .await?;
        Ok(b58(&identity.id()))
    }

    async fn register_name(
        &self,
        identity: &str,
        label: &str,
        lease: &Lease,
    ) -> Result<NameOutcome, NameError> {
        let identity = parse_identity(identity)?;
        super::dashpay::DashPay::register_name_for_flow(
            &self.session,
            self.wallet_id,
            identity,
            label,
            lease.clone(),
        )
        .await
    }

    async fn platform_profile(
        &self,
        identity: &str,
    ) -> Result<Option<ProfileRecord>, PlatformError> {
        use dash_sdk::drive::query::{WhereClause, WhereOperator};
        use dash_sdk::platform::{DocumentQuery, FetchMany};
        use dpp::document::{Document, DocumentV0Getters};
        let id = parse_identity(identity)?;
        let query = DocumentQuery::new(dashpay_contract()?, "profile")
            .map_err(|e| PlatformError::Internal {
                detail: format!("the profile query: {e}"),
            })?
            .with_where(WhereClause {
                field: "$ownerId".into(),
                operator: WhereOperator::Equal,
                value: dpp::platform_value::platform_value!(id),
            })
            .with_limit(1);
        let documents = Document::fetch_many(&self.sdk, query).await?;
        let Some(document) = documents.into_values().flatten().next() else {
            return Ok(None);
        };
        let text = |key: &str| {
            document
                .properties()
                .get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Ok(Some(ProfileRecord {
            display_name: text("displayName"),
            public_message: text("publicMessage"),
            avatar_url: text("avatarUrl"),
            ..ProfileRecord::default()
        }))
    }

    async fn create_profile(
        &self,
        identity: &str,
        identity_index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        let id = parse_identity(identity)?;
        let signer = lease.identity_signer(&[identity_index])?;
        // One write of the identity at a time (identity_writes.rs).
        let locks = identity_locks(self.wallet_id, id);
        let _write = locks.write.lock().await;
        self.wallet
            .identity()
            .dashpay()
            .create_profile_with_external_signer(&id, profile_update(profile), &signer)
            .await?;
        Ok(())
    }

    async fn update_profile(
        &self,
        identity: &str,
        identity_index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        let id = parse_identity(identity)?;
        let signer = lease.identity_signer(&[identity_index])?;
        // One write of the identity at a time (identity_writes.rs).
        let locks = identity_locks(self.wallet_id, id);
        let _write = locks.write.lock().await;
        self.wallet
            .identity()
            .dashpay()
            .update_profile_with_external_signer(&id, profile_update(profile), &signer)
            .await?;
        Ok(())
    }

    async fn name_credits(&self, label: &str) -> Result<u64, NameError> {
        super::dashpay::DashPay::name_credits_for_flow(Arc::clone(&self.sdk), label).await
    }

    async fn credit_balance(&self, identity: &str) -> Result<Option<u64>, PlatformError> {
        use dash_sdk::platform::Fetch;
        use dash_sdk::query_types::IdentityBalance;
        let id = parse_identity(identity)?;
        Ok(IdentityBalance::fetch(&self.sdk, id).await?)
    }
}

#[cfg(test)]
mod flows_tests {
    use super::*;

    const KEY: (WalletId, i64) = (WalletId([3; 32]), 1);

    fn hold(flows: &Arc<Flows>) -> Option<Claim> {
        flows.admit(KEY, Kind::Run).map(|(claim, _)| claim)
    }

    fn take(flows: &Arc<Flows>) -> Option<Claim> {
        flows.admit(KEY, Kind::Decide).map(|(claim, _)| claim)
    }

    /// A decision takes the flow from a run between its calls: the run's
    /// calls are refused from then on, whatever the decision ends with. A
    /// call running refuses a decision.
    #[test]
    fn a_decision_takes_the_flow_between_calls() {
        let flows = Arc::new(Flows::default());
        let run = hold(&flows).unwrap();
        let call = flows.enter_call(&run).ok().unwrap();
        assert!(take(&flows).is_none(), "a call runs");
        drop(call);
        let decision = take(&flows).unwrap();
        assert!(!flows.get(KEY).running());
        assert_eq!(flows.enter_call(&run).err(), Some(Step::Stop));
        drop(decision);
        assert_eq!(flows.enter_call(&run).err(), Some(Step::Stop));
        // The taken run's end does not release the next holder.
        let next = hold(&flows).unwrap();
        drop(run);
        assert!(flows.get(KEY).running());
        assert!(flows.enter_call(&next).is_ok());
    }

    /// Sol r1 R1-02: a second discard cannot decide while the first does,
    /// so the first one's end cannot let a call start under the second.
    #[test]
    fn two_discard_guards_keep_calls_excluded() {
        let flows = Arc::new(Flows::default());
        let run = hold(&flows).unwrap();
        let first = take(&flows).unwrap();
        assert!(take(&flows).is_none(), "one decision at a time");
        drop(first);
        assert_eq!(
            flows.enter_call(&run).err(),
            Some(Step::Stop),
            "a call entered after a discard decided"
        );
    }

    /// Sol r1 R1-08: a claim of a forgotten flow (its wallet closed) that
    /// ends after the flow was claimed again leaves the new claim alone.
    #[tokio::test]
    async fn a_retired_claim_cannot_clear_a_reopened_flow() {
        let flows = Arc::new(Flows::default());
        let retired = hold(&flows).unwrap();
        flows.forget_wallet(KEY.0);
        flows.act(KEY.0, || ()).await.unwrap();
        let reopened = hold(&flows).unwrap();
        drop(retired);
        assert!(flows.get(KEY).running(), "the reopened run was cleared");
        assert!(hold(&flows).is_none(), "a second driver was admitted");
        assert!(flows.enter_call(&reopened).is_ok());
    }

    /// DEC-173: a claim sent before an unload but run after it is
    /// forgotten with the wallet's flows, so the wallet loaded again admits
    /// its own claim.
    #[tokio::test]
    async fn a_claim_sent_before_an_unload_is_forgotten_with_it() {
        let flows = Arc::new(Flows::default());
        let (gate, wait) = std::sync::mpsc::channel::<()>();
        flows.send(
            KEY.0,
            Box::new(move || {
                let _ = wait.recv();
            }),
        );
        let early = {
            let flows = Arc::clone(&flows);
            tokio::spawn(async move {
                let job = Arc::clone(&flows);
                flows.act(KEY.0, move || hold(&job)).await.unwrap()
            })
        };
        // The current-thread runtime polls the early claim, which queues
        // it behind the gate, before the unload's job.
        tokio::task::yield_now().await;
        flows.forget_wallet(KEY.0);
        let job = Arc::clone(&flows);
        let reopened = flows.act(KEY.0, move || hold(&job));
        gate.send(()).unwrap();
        let early = early.await.unwrap();
        let reopened = reopened.await.unwrap();
        assert!(early.is_some(), "the early claim ran before the unload");
        let reopened = reopened.expect("the reloaded wallet admits its claim");
        assert!(flows.enter_call(&reopened).is_ok());
        assert_eq!(
            flows.enter_call(early.as_ref().unwrap()).err(),
            Some(Step::Stop)
        );
    }
}
