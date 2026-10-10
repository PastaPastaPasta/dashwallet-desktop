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
//! **What waits for E0-04.P2b (Mode B).** P2a has no call permits, no
//! funding or step markers and no released-signature counting. Each place
//! that needs them is marked `P2b seam:` below:
//! - the funding cap is checked against the lease's Funding budget before
//!   the call; P2b's call permit charges it, with a deadline the lock's
//!   drain waits for;
//! - the funding marker (`registration/<draft>/funding`) and the identity
//!   step's marker are not written; the calls run in their step scope
//!   (`Lease::scope_step`) already, and the funding gate reads the tracked
//!   rows by the draft's funding key, which survives a deleted journal;
//! - without the signature count, a failed funding call is read from the
//!   tracked rows and the error's class (H12): a tracked row is a
//!   committed lock; a definite rejection or a failure before anything was
//!   built, with no tracked row, is `NotSent` and the row is retryable
//!   again; any other failure with no tracked row is `MaybeSent`, and the
//!   row waits (or fails `platform.broadcast_unknown`) until the lock is
//!   tracked again or positive evidence says `NotSent`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use dashcore::OutPoint;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::state_transition::asset_lock_proof::chain::ChainAssetLockProof;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_appdb::{
    AppDb, AppDbError, LockOutpoint, RegistrationChange, RegistrationRow, RegistrationUpdate,
};
use dw_vault::LockState;
use tokio::time::Instant;

use super::errors::{NameError, PlatformError, RegistrationError};
use super::flows::{BudgetPurpose, DispatchState, LeaseStateView};
use super::lease::{Lease, LeaseId};
use super::names::NameOutcome;
use super::registration::RegistrationWait;
use super::registration_rows::{Decoded, Funding, Phase, ProfileRecord};
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

/// The coins of the library's pooled funding sources (BIP44, BIP32,
/// DashPay receiving).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Spendable {
    /// What the engine lets a funding spend: the coins it auto-selects.
    pub(crate) duffs: u64,
    /// Every coin of the sources, user-locked ones too: the library does
    /// not know the engine's locks, so the fee bound counts them all.
    pub(crate) inputs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FundError {
    /// The engine refused before the library call started (a lease that
    /// no longer signs): nothing was signed, so nothing can be out.
    Refused(PlatformError),
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
    fn spendable(&self) -> impl Future<Output = Spendable> + Send;
    /// SPV has synced and the wallet's DashPay bring-up (DP1-05's
    /// same-seed discovery; the library's asset-lock reconstruction runs
    /// with the sync) has run: §3.4 Restore rule 1, and §2.2 rule 5's hold.
    fn ready_to_fund(&self) -> bool;
    /// `create_funded_asset_lock_proof` for `IdentityRegistration` at
    /// `identity_index`, signed by the lease's funding signer: builds,
    /// tracks, broadcasts, and waits up to 300 s for an InstantSend lock.
    fn fund(
        &self,
        lock_duffs: u64,
        identity_index: u32,
        lease: &Lease,
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
    /// The identity the lock created, if Platform (or the wallet) has it.
    fn find_identity(
        &self,
        outpoint: OutPoint,
    ) -> impl Future<Output = Result<Option<String>, PlatformError>> + Send;
    /// IdentityCreate from the tracked lock (`FromExistingAssetLock`),
    /// with keys 0–5 of `identity_index`, under the lease's signers.
    fn register_identity(
        &self,
        outpoint: OutPoint,
        identity_index: u32,
        lease: &Lease,
    ) -> impl Future<Output = Result<String, PlatformError>> + Send;
    /// DP1-03's `register_name` under the lease.
    fn register_name(
        &self,
        identity: &str,
        label: &str,
        lease: &Lease,
    ) -> impl Future<Output = Result<NameOutcome, NameError>> + Send;
    /// Creates the identity's DashPay profile, or does nothing if it has
    /// one.
    fn create_profile(
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

/// One flow's state in this process (the session's Platform runtime):
/// nothing here is persisted.
#[derive(Debug, Default, Clone)]
pub(crate) struct FlowState {
    /// A task drives the flow now.
    pub(crate) running: bool,
    /// Which claim runs it: a task handle is stored for its own claim only.
    generation: u64,
    /// A library call of the flow runs: discard is refused.
    pub(crate) in_call: bool,
    /// A discard is deciding: no library call of the flow may start.
    stopping: bool,
    /// What the flow waits for, set when it parks or starts a wait.
    pub(crate) wait: Option<RegistrationWait>,
    /// The InstantSend window's end, from an in-memory instant taken at the
    /// funding call (never `updated_at`, §3.4).
    pub(crate) is_until: Option<Instant>,
    /// The lease the running flow signs with.
    pub(crate) lease: Option<LeaseId>,
    /// A lease `resume_registration` handed to a run that has none (or one
    /// that no longer signs); the run takes it at its next keyed step.
    offered: Option<Offered>,
    /// The automatic backup of `FundingSent` was scheduled.
    pub(crate) backed_up: bool,
    pub(crate) task: Option<tokio::task::AbortHandle>,
}

#[derive(Clone)]
struct Offered(Lease);

impl std::fmt::Debug for Offered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Offered").field(&self.0.id()).finish()
    }
}

/// Every flow's [`FlowState`], keyed by wallet and draft.
#[derive(Debug, Default)]
pub(crate) struct Flows {
    flows: Mutex<HashMap<(WalletId, i64), FlowState>>,
    /// Orders `start_registration`'s in-progress check and its insert.
    pub(crate) start: tokio::sync::Mutex<()>,
    /// Orders the choice of an identity index and its write, so no two
    /// flows take one index.
    index: tokio::sync::Mutex<()>,
}

impl Flows {
    pub(crate) fn with<R>(&self, key: (WalletId, i64), f: impl FnOnce(&mut FlowState) -> R) -> R {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        f(map.entry(key).or_default())
    }

    /// Like [`Self::with`], but leaves a forgotten flow forgotten.
    fn with_existing(&self, key: (WalletId, i64), f: impl FnOnce(&mut FlowState)) {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(s) = map.get_mut(&key) {
            f(s);
        }
    }

    pub(crate) fn get(&self, key: (WalletId, i64)) -> FlowState {
        self.flows
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned()
            .unwrap_or_default()
    }

    /// Claims the flow for one driver; `None` while another runs it.
    pub(crate) fn claim(self: &Arc<Self>, key: (WalletId, i64)) -> Option<Claim> {
        self.with(key, |s| {
            if s.running {
                return None;
            }
            s.running = true;
            s.generation = s.generation.wrapping_add(1);
            Some(Claim {
                flows: Arc::clone(self),
                key,
                generation: s.generation,
            })
        })
    }

    /// Marks a library call of the flow, unless a discard is deciding:
    /// atomic with [`Self::begin_stop`].
    fn enter_call(&self, key: (WalletId, i64)) -> Option<InCall<'_>> {
        self.with(key, |s| {
            if s.stopping {
                return false;
            }
            s.in_call = true;
            true
        })
        .then_some(InCall(self, key))
    }

    /// A discard starts deciding: `false` while a library call of the flow
    /// runs; otherwise none starts until [`Self::end_stop`] or
    /// [`Self::forget`].
    pub(crate) fn begin_stop(&self, key: (WalletId, i64)) -> bool {
        self.with(key, |s| {
            if s.in_call {
                return false;
            }
            s.stopping = true;
            true
        })
    }

    /// The discard was refused: the flow runs on, untouched.
    pub(crate) fn end_stop(&self, key: (WalletId, i64)) {
        self.with_existing(key, |s| s.stopping = false);
    }

    /// Hands `lease` to the flow's run (see [`FlowState::offered`]).
    pub(crate) fn offer(&self, key: (WalletId, i64), lease: Lease) {
        self.with(key, |s| {
            if let Some(task) = &s.task {
                lease.register_task(task.clone());
            }
            s.offered = Some(Offered(lease));
        });
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

    pub(crate) fn forget_wallet(&self, wallet: WalletId) {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        map.retain(|(w, _), s| {
            if *w == wallet
                && let Some(task) = s.task.take()
            {
                task.abort();
            }
            *w != wallet
        });
    }
}

/// One driver's hold on a flow; released when dropped.
pub(crate) struct Claim {
    flows: Arc<Flows>,
    key: (WalletId, i64),
    generation: u64,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.flows.with_existing(self.key, |s| {
            s.running = false;
            s.in_call = false;
            s.task = None;
            s.lease = None;
            s.offered = None;
        });
    }
}

/// Marks a library call of the flow; cleared when dropped.
struct InCall<'a>(&'a Flows, (WalletId, i64));

impl Drop for InCall<'_> {
    fn drop(&mut self) {
        self.0.with_existing(self.1, |s| s.in_call = false);
    }
}

/// What one step decided.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// The row changed; read it and step again.
    Again,
    /// Waits for `RegistrationWait`; the row keeps its phase.
    Park(RegistrationWait),
    /// Fails the flow with a code.
    Fail(&'static str, bool),
    /// Nothing left to do.
    Stop,
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

/// What the funding gates read: the wallet's tracked locks, and the locks
/// rows hold already.
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    pub(crate) locks: Vec<LockInfo>,
    /// Every row's `asset_lock_outpoint`.
    held: Vec<OutPoint>,
}

impl Evidence {
    pub(crate) fn new(locks: Vec<LockInfo>, rows: &[RegistrationRow]) -> Self {
        Self {
            locks,
            held: rows.iter().filter_map(outpoint_of).collect(),
        }
    }

    fn tracked(&self, outpoint: OutPoint) -> Option<&LockInfo> {
        self.locks.iter().find(|l| l.outpoint == outpoint)
    }

    /// The tracked lock behind `row`: its outpoint's, or before one reached
    /// the row, the unheld lock of its funding key.
    pub(crate) fn lock_of(&self, row: &RegistrationRow) -> Option<&LockInfo> {
        match outpoint_of(row) {
            Some(op) => self.tracked(op),
            None => row.identity_index.and_then(|i| self.unheld_lock(i)),
        }
    }

    /// A registration lock at `index` (account 0) that no row holds: the
    /// one a `CoreBalance` draft at `index` funds, by the draft's own
    /// funding key, so it is found even when no outpoint reached the row
    /// (E0-04 §2a.5, review Opus r3 F-2). A lock another row holds is that
    /// row's.
    pub(crate) fn unheld_lock(&self, index: u32) -> Option<&LockInfo> {
        self.at_index(index)
            .find(|l| !self.held.contains(&l.outpoint))
    }

    /// A registration lock at `index` that another row than `row` holds:
    /// the index is that flow's, and `row` must take another.
    fn held_elsewhere(&self, row: &RegistrationRow, index: u32) -> bool {
        let own = outpoint_of(row);
        self.at_index(index)
            .any(|l| self.held.contains(&l.outpoint) && Some(l.outpoint) != own)
    }

    fn at_index(&self, index: u32) -> impl Iterator<Item = &LockInfo> {
        self.locks
            .iter()
            .filter(move |l| l.registration && l.identity_index == index && l.account == 0)
    }
}

fn is_core_balance(row: &RegistrationRow) -> bool {
    matches!(
        Funding::decode(&row.funding),
        Decoded::Known(Funding::CoreBalance { .. })
    )
}

/// What the engine's funding gate reads for a row's funding (E0-04 §16.6):
/// `None` and `Some(NotSent)` allow a discard or a second funding.
///
/// P2b seam: §2a.5's derivation from the funding marker and its definite
/// resolutions joins here; P2a has no markers, so this reads the row and
/// the tracked locks only.
pub(crate) fn funding_status(row: &RegistrationRow, ev: &Evidence) -> Option<DispatchState> {
    if !is_core_balance(row) {
        // No funding of the user's: an existing identity, or a lock that is
        // the inviter's or the faucet's.
        return None;
    }
    if let Some(lock) = ev.lock_of(row) {
        return Some(
            if lock.status.has_proof() || lock.status == LockStatus::Consumed {
                DispatchState::Sent
            } else {
                DispatchState::WillBeSent
            },
        );
    }
    if row.asset_lock_outpoint.is_some()
        || row.error.as_deref() == Some(broadcast_unknown())
        || Phase::parse(&row.phase).is_some_and(|p| !p.unfunded())
    {
        // Funded once, its row now lost (a power loss, a journal deleted by
        // hand, a lock the library untracked as swept): possibly out, until
        // positive evidence settles it. A phase this build does not know
        // counts by its outpoint alone: every build writes it before the
        // broadcast (Restore rule 3).
        return Some(DispatchState::MaybeSent);
    }
    None
}

/// `funds_committed` (E0-04 §16.5) for a row: a funding committed and not
/// definitely unsent, or a funding call running now.
pub(crate) fn funds_committed(row: &RegistrationRow, ev: &Evidence, in_call: bool) -> bool {
    let funded = matches!(
        funding_status(row, ev),
        Some(DispatchState::WillBeSent | DispatchState::MaybeSent | DispatchState::Sent)
    );
    funded || (is_core_balance(row) && in_call)
}

/// Writes the outpoint of a lock the library just tracked into the
/// `KeysPrepared` row it funds, before the library broadcasts it (§3.4
/// Restore rule 3): platform-wallet stores the `Built` row inline, and the
/// changeset tap calls this from that store. Returns the draft moved to
/// `FundingSent`.
pub(crate) fn note_lock_tracked(
    appdb: &AppDb,
    wallet: &str,
    outpoint: OutPoint,
    identity_index: u32,
    now: u64,
) -> Result<Option<i64>, AppDbError> {
    let lock = canonical(outpoint);
    if appdb
        .registration_for_lock(wallet, lock.as_str())?
        .is_some()
    {
        return Ok(None);
    }
    let mut rows = appdb.registration_rows(wallet)?;
    // A live flow first; then one that failed with nothing committed (a
    // retryable failure, or `broadcast_unknown`, which this lock settles):
    // the lock is that flow's, so it resumes instead of a second row.
    rows.sort_by_key(|r| r.error.is_some());
    for row in rows {
        let failed = row.error.is_some();
        let funds_it =
            (!failed || row.retryable || row.error.as_deref() == Some(broadcast_unknown()))
                && row.phase == Phase::KeysPrepared.as_str()
                && row.identity_index == Some(identity_index)
                && row.asset_lock_outpoint.is_none()
                && is_core_balance(&row);
        if !funds_it {
            continue;
        }
        let change = RegistrationChange {
            phase: Some(Phase::FundingSent.as_str().into()),
            outpoint: Some(lock.clone()),
            failure: failed.then_some(None),
            ..Default::default()
        };
        let written = appdb.update_registration(
            wallet,
            row.id,
            Phase::KeysPrepared.as_str(),
            failed,
            &change,
            now,
        )?;
        if written == RegistrationUpdate::Applied {
            return Ok(Some(row.id));
        }
    }
    Ok(None)
}

/// One run of one flow: steps the row until it parks, fails or is done.
pub(crate) struct Driver<'a, L> {
    session: &'a Arc<NetworkSession>,
    wallet: WalletId,
    lib: &'a L,
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
    pub(crate) fn new(
        session: &'a Arc<NetworkSession>,
        wallet: WalletId,
        lib: &'a L,
        lease: Option<Lease>,
        waits: bool,
    ) -> Self {
        Self {
            session,
            wallet,
            lib,
            lease: Mutex::new(lease),
            waits,
        }
    }

    fn flows(&self) -> &Flows {
        &self.session.platform.registrations
    }

    /// The run's lease; one offered meanwhile replaces it.
    fn lease(&self, draft: i64) -> Option<Lease> {
        let mut lease = self.lease.lock().unwrap_or_else(PoisonError::into_inner);
        let offered = self.flows().with((self.wallet, draft), |s| {
            let offered = s.offered.take()?;
            s.lease = Some(offered.0.id());
            Some(offered.0)
        });
        if offered.is_some() {
            *lease = offered;
        }
        lease.clone()
    }

    /// Steps `draft` until it parks, fails or ends. An error is a storage
    /// failure: the row keeps the phase it had, and the flow is resumable.
    pub(crate) async fn run(&self, draft: i64) -> Result<(), PlatformError> {
        let key = (self.wallet, draft);
        let lease = self
            .lease
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Lease::id);
        self.flows().with(key, |s| s.lease = lease);
        let ran = self.steps(draft).await;
        if ran.is_err() {
            self.flows().with_existing(key, |s| s.wait = None);
        }
        ran
    }

    async fn steps(&self, draft: i64) -> Result<(), PlatformError> {
        let key = (self.wallet, draft);
        let mut retry = Duration::from_secs(2);
        loop {
            let Some(row) = self.row(draft).await? else {
                return Ok(());
            };
            if row.error.is_some() {
                return Ok(());
            }
            self.flows().with(key, |s| s.wait = None);
            match self.step(&row).await? {
                Step::Again => continue,
                Step::Park(wait @ (RegistrationWait::Sync | RegistrationWait::Network))
                    if self.waits =>
                {
                    self.flows().with(key, |s| s.wait = Some(wait));
                    let pause = if wait == RegistrationWait::Sync {
                        SYNC_POLL
                    } else {
                        tracing::info!(draft, phase = %row.phase, "registration retries");
                        let pause = retry;
                        retry = (retry * 2).min(NETWORK_RETRY_MAX);
                        pause
                    };
                    tokio::time::sleep(pause).await;
                }
                Step::Park(wait) => {
                    tracing::info!(draft, phase = %row.phase, ?wait, "registration parked");
                    self.flows().with(key, |s| s.wait = Some(wait));
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

    async fn evidence(&self) -> Result<Evidence, PlatformError> {
        let wallet = self.wallet.to_string();
        let rows = self
            .session
            .appdb_op(move |db| db.registration_rows(&wallet))
            .await?;
        Ok(Evidence::new(self.lib.tracked_locks().await, &rows))
    }

    async fn row(&self, draft: i64) -> Result<Option<RegistrationRow>, PlatformError> {
        let wallet = self.wallet.to_string();
        Ok(self
            .session
            .appdb_op(move |db| db.registration_row(&wallet, draft))
            .await?)
    }

    /// Compare-and-set from the row as read.
    async fn write(
        &self,
        row: &RegistrationRow,
        change: RegistrationChange,
    ) -> Result<RegistrationUpdate, PlatformError> {
        let (wallet, id, phase, failed) = (
            self.wallet.to_string(),
            row.id,
            row.phase.clone(),
            row.error.is_some(),
        );
        let now = crate::events::unix_now();
        let written = self
            .session
            .appdb_op(move |db| db.update_registration(&wallet, id, &phase, failed, &change, now))
            .await?;
        if written == RegistrationUpdate::Applied {
            signal(self.session, self.wallet, id);
        }
        Ok(written)
    }

    async fn advance(
        &self,
        row: &RegistrationRow,
        change: RegistrationChange,
    ) -> Result<Step, PlatformError> {
        Ok(match self.write(row, change).await? {
            // `LockTaken`: another flow took the lock this one would adopt
            // meanwhile; the next step reads it as that flow's.
            RegistrationUpdate::Applied
            | RegistrationUpdate::Stale
            | RegistrationUpdate::LockTaken => Step::Again,
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
        let (Some(phase), Decoded::Known(funding)) =
            (Phase::parse(&row.phase), Funding::decode(&row.funding))
        else {
            return Ok(Step::Fail(unsupported(), false));
        };
        match funding {
            // DP5-02 seam: the invitation-claim path. The link is a bearer
            // credential in the vault of the machine that stashed it only,
            // so a restored row cannot use it, and none can before DP5-02:
            // the row fails for good and stops counting as in progress.
            Funding::Invitation { .. } => {
                return Ok(Step::Fail(
                    super::errors::InvitationError::Invalid.code(),
                    false,
                ));
            }
            // Developer builds only, and this build has none.
            Funding::FaucetAssetLock { .. } => {
                return Ok(Step::Fail(
                    PlatformError::FeatureOff {
                        feature: String::new(),
                    }
                    .code(),
                    false,
                ));
            }
            Funding::CoreBalance { .. } | Funding::ExistingIdentity => {}
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
            | PlatformError::ContextUnavailable => Step::Park(RegistrationWait::Network),
            other => Step::Fail(other.code(), true),
        }
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
        if !self.lib.ready_to_fund() {
            return Ok(Step::Park(RegistrationWait::Sync));
        }
        self.take_index(row, Phase::KeysPrepared).await
    }

    /// Writes `phase` with a new identity index, one past every used one;
    /// under the index lock, so no two flows take one index.
    async fn take_index(&self, row: &RegistrationRow, phase: Phase) -> Result<Step, PlatformError> {
        let _index = self.flows().index.lock().await;
        let index = self.next_index().await?;
        self.advance(
            row,
            RegistrationChange {
                phase: Some(phase.as_str().into()),
                identity_index: Some(index),
                ..Default::default()
            },
        )
        .await
    }

    /// One past every identity index the wallet's identities, its tracked
    /// registration locks and its flows use.
    async fn next_index(&self) -> Result<u32, PlatformError> {
        let wallet = self.wallet.to_string();
        let rows = self
            .session
            .appdb_op(move |db| db.registration_rows(&wallet))
            .await?;
        let used = self
            .lib
            .identities()
            .await
            .into_iter()
            .map(|(i, _)| i)
            .chain(
                self.lib
                    .tracked_locks()
                    .await
                    .into_iter()
                    .filter(|l| l.registration)
                    .map(|l| l.identity_index),
            )
            .chain(rows.iter().filter_map(|r| r.identity_index))
            .max();
        Ok(used.map_or(0, |i| i.saturating_add(1)))
    }

    /// Records the flow's lock: the outpoint and `phase`, then the
    /// automatic backup of §3.4 Restore rule 4.
    async fn record_lock(
        &self,
        row: &RegistrationRow,
        outpoint: OutPoint,
        phase: Phase,
    ) -> Result<Step, PlatformError> {
        let step = self
            .advance(
                row,
                RegistrationChange {
                    phase: Some(phase.as_str().into()),
                    outpoint: Some(canonical(outpoint)),
                    ..Default::default()
                },
            )
            .await?;
        self.backup_once(row.id);
        Ok(step)
    }

    fn backup_once(&self, draft: i64) {
        backup_once(self.session, self.wallet, draft);
    }

    /// KeysPrepared → FundingSent: the one step that commits funds.
    async fn fund(&self, row: &RegistrationRow, lock_duffs: u64) -> Result<Step, PlatformError> {
        if !self.lib.ready_to_fund() {
            return Ok(Step::Park(RegistrationWait::Sync));
        }
        let Some(index) = row.identity_index else {
            return Ok(Step::Fail(internal(), false));
        };
        // §3.4 Restore rule 2: adopt what an earlier run left.
        if let Some(id) = self.identity_at(index).await {
            return self
                .advance(
                    row,
                    RegistrationChange {
                        phase: Some(Phase::IdentityRegistered.as_str().into()),
                        identity: Some(id),
                        ..Default::default()
                    },
                )
                .await;
        }
        let ev = self.evidence().await?;
        if let Some(lock) = ev.unheld_lock(index) {
            return self
                .record_lock(row, lock.outpoint, Phase::FundingSent)
                .await;
        }
        if ev.held_elsewhere(row, index) {
            // Another flow funded this index (a restored row behind
            // reality whose lock "Finish transfers" adopted): this one
            // takes a new index rather than a second lock for it.
            return self.take_index(row, Phase::KeysPrepared).await;
        }
        // The funding gate (H11): a second funding only after `NotSent` or
        // `None`. P2b seam: the funding marker's status joins here.
        if let Some(state) = funding_status(row, &ev)
            && state != DispatchState::NotSent
        {
            return Ok(Step::Fail(broadcast_unknown(), false));
        }
        let Some(lease) = self.lease(row.id) else {
            return Ok(Step::Park(self.authority_wait()));
        };
        // The funding cap (E0-04 §2a.3): the lock plus the worst-case fee
        // must fit the Funding budget. P2b seam: the call permit charges it
        // in its J step instead of this check.
        let spendable = self.lib.spendable().await;
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
        // The call holds a plain call permit. P2b seam: the funding permit
        // (`funding_call_permit`) and its marker (`funding_step`, with the
        // funding key) before the call.
        let key = (self.wallet, row.id);
        let Some(in_call) = self.flows().enter_call(key) else {
            // A discard is deciding.
            return Ok(Step::Stop);
        };
        self.flows().with(key, |s| {
            s.is_until = Some(Instant::now() + IS_WINDOW);
            s.wait = Some(RegistrationWait::InstantSend);
        });
        let funded = lease
            .scope_step(Some(funding_step(row.id)), async {
                let call = lease
                    .call_permit(funding_step(row.id), None)
                    .map_err(|e| FundError::Refused(e.into()))?;
                let funded = self.lib.fund(lock_duffs, index, &lease).await;
                call.finish(funded.is_ok());
                funded
            })
            .await;
        drop(in_call);
        // platform-wallet tracks the lock (`Built`, stored through the tap)
        // before it hands it to the broadcaster, and untracks it only on a
        // definitive rejection (`build.rs` `broadcast_funded_asset_lock_with_funding`).
        // So after the call a tracked row is a committed lock. With none,
        // the error's class decides (H12): a definite rejection or a
        // failure before the build is `NotSent`, anything else (a lock the
        // library untracked as swept, a wallet closing) may be out.
        let current = self.row(row.id).await?;
        let ev = self.evidence().await?;
        let tracked = match &current {
            Some(r) => ev.lock_of(r),
            None => ev.unheld_lock(index),
        }
        .map(|l| l.outpoint);
        match (funded, tracked) {
            (Ok(outpoint), _) => self.record_lock(row, outpoint, Phase::FundingSent).await,
            (Err(FundError::IsTimeout), Some(outpoint)) => {
                // The window is over. A call that ended sooner (an unknown
                // broadcast outcome) keeps the rest of it for the proof wait.
                self.flows().with(key, |s| s.is_until = None);
                self.drop_own_key(&lease);
                self.record_lock(row, outpoint, Phase::ProofWaiting).await
            }
            (Err(e), Some(outpoint)) => {
                tracing::info!(
                    draft = row.id,
                    "the funding call failed after tracking: {e:?}"
                );
                self.record_lock(row, outpoint, Phase::FundingSent).await
            }
            // Refused before the call: nothing was signed.
            (Err(FundError::Refused(e)), None) => Ok(self.failed(&e)),
            (Err(FundError::NotSent(e)), None) => {
                tracing::info!(draft = row.id, "the funding never reached the wire: {e}");
                self.unfund(row.id, e.code()).await
            }
            (Err(e), None) => {
                tracing::warn!(draft = row.id, "the funding call failed untracked: {e:?}");
                self.maybe_sent(row.id).await
            }
        }
    }

    /// Moves a flow whose lock never reached the wire back to
    /// `KeysPrepared`, retryable: the tap may have recorded the lock's
    /// outpoint while it was tracked.
    async fn unfund(&self, draft: i64, code: &'static str) -> Result<Step, PlatformError> {
        let Some(row) = self.row(draft).await? else {
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
        self.write(&row, change).await?;
        Ok(Step::Stop)
    }

    /// A funding that may be out with no tracked row behind it: a row with
    /// the outpoint waits for the lock to be tracked again (its outpoint
    /// reads `MaybeSent`); one without fails `platform.broadcast_unknown`,
    /// not retryable, until the lock is tracked (the tap adopts it) or
    /// positive evidence says `NotSent`.
    async fn maybe_sent(&self, draft: i64) -> Result<Step, PlatformError> {
        let Some(row) = self.row(draft).await? else {
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
            // Its row was lost (Mode B residual): DP1-05's reconstruction
            // tracks it again once it is on the chain.
            return Ok(Step::Park(RegistrationWait::ChainLock));
        };
        if lock.status == LockStatus::Consumed {
            // Platform took the lock; its identity is not visible yet.
            return Ok(Step::Park(RegistrationWait::Network));
        }
        if !lock.status.has_proof() {
            if phase == Phase::FundingSent {
                return self.to_phase(row, Phase::ProofWaiting).await;
            }
            return Ok(self.wait_proof(row.id, outpoint, lock.status).await);
        }
        let Some(lease) = self.lease(row.id) else {
            return Ok(Step::Park(self.authority_wait()));
        };
        // The call holds a plain call permit. P2b seam: the step marker of
        // `identity_step` written before the call.
        let Some(in_call) = self.flows().enter_call((self.wallet, row.id)) else {
            return Ok(Step::Stop);
        };
        let created = lease
            .scope_step(Some(identity_step(row.id)), async {
                let call = lease.call_permit(identity_step(row.id), None)?;
                let created = self.lib.register_identity(outpoint, index, &lease).await;
                call.finish(created.is_ok());
                created
            })
            .await;
        drop(in_call);
        match created {
            Ok(id) => self.advance(row, registered(id)).await,
            Err(e) => {
                if let Ok(Some(id)) = self.lib.find_identity(outpoint).await {
                    return self.advance(row, registered(id)).await;
                }
                Ok(self.failed(&e))
            }
        }
    }

    /// The proof wait: InstantSend while the in-memory window lasts, then
    /// the ChainLock with no limit, in bounded waits. No key is needed; an
    /// own key is dropped when the window ends.
    async fn wait_proof(&self, draft: i64, outpoint: OutPoint, status: LockStatus) -> Step {
        let key = (self.wallet, draft);
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
        if window.is_none()
            && let Some(lease) = self.lease(draft)
        {
            self.drop_own_key(&lease);
        }
        if !self.waits {
            // A `Built` lock is broadcast again, the proof not waited for.
            if status == LockStatus::Built
                && let Err(e) = self.lib.wait_proof(outpoint, REBROADCAST_WAIT).await
                && e != FundError::IsTimeout
            {
                tracing::info!(draft, "the lock's re-broadcast failed: {e:?}");
            }
            return Step::Park(wait);
        }
        self.flows().with(key, |s| s.wait = Some(wait));
        match self
            .lib
            .wait_proof(outpoint, window.unwrap_or(CHAINLOCK_POLL))
            .await
        {
            Ok(()) => Step::Again,
            Err(FundError::IsTimeout) => {
                if window.is_some() {
                    self.flows().with(key, |s| s.is_until = None);
                }
                Step::Again
            }
            Err(FundError::Refused(e) | FundError::NotSent(e) | FundError::Failed(e)) => {
                self.failed(&e)
            }
        }
    }

    /// NameRequested → NameRegistered | Contested, then the temporary name.
    async fn names(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        let Some(identity) = row.identity.as_deref() else {
            return Ok(Step::Fail(internal(), false));
        };
        let Some(lease) = self.lease(row.id) else {
            return Ok(Step::Park(self.authority_wait()));
        };
        let Some(_in_call) = self.flows().enter_call((self.wallet, row.id)) else {
            return Ok(Step::Stop);
        };
        match self.lib.register_name(identity, &row.label, &lease).await {
            Ok(NameOutcome::Registered) => self.to_phase(row, Phase::NameRegistered).await,
            Ok(NameOutcome::ContestStarted { .. }) => {
                if let Some(temp) = row.temp_label.as_deref() {
                    match self.lib.register_name(identity, temp, &lease).await {
                        Ok(_) => {}
                        Err(NameError::Platform(e)) => {
                            let step = self.failed(&e);
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
                self.to_phase(row, Phase::Contested).await
            }
            Err(NameError::Platform(e)) => Ok(self.failed(&e)),
            // Taken, locked, a closed contest: the identity exists, and the
            // user picks another name with `register_name`.
            Err(e) => Ok(Step::Fail(e.code(), false)),
        }
    }

    /// The initial profile, if any, then Done.
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
        let Some(lease) = self.lease(row.id) else {
            return Ok(Step::Park(self.authority_wait()));
        };
        let Some(_in_call) = self.flows().enter_call((self.wallet, row.id)) else {
            return Ok(Step::Stop);
        };
        match self
            .lib
            .create_profile(identity, index, &profile, &lease)
            .await
        {
            Ok(()) => {
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
            Err(e) => Ok(self.failed(&e)),
        }
    }
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

/// Starts a task driving `draft` unless one already does (`false`).
pub(crate) fn spawn<L: RegistrationLib + 'static>(
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    lib: Arc<L>,
    draft: i64,
    lease: Option<Lease>,
) -> bool {
    let flows = &session.platform.registrations;
    let Some(claim) = flows.claim((wallet, draft)) else {
        return false;
    };
    let generation = claim.generation;
    let task = {
        let session = Arc::clone(session);
        let lease = lease.clone();
        session.rt.clone().spawn(async move {
            let _claim = claim;
            let driver = Driver::new(&session, wallet, &*lib, lease, true);
            if let Err(e) = driver.run(draft).await {
                tracing::warn!(draft, error = %e, "registration stopped");
            }
        })
    };
    let handle = task.abort_handle();
    flows.with_existing((wallet, draft), |s| {
        if s.running && s.generation == generation {
            s.task = Some(handle.clone());
        }
    });
    if let Some(lease) = &lease {
        lease.register_task(handle);
    }
    true
}

/// Schedules the automatic backup of a flow that reached `FundingSent`
/// (§3.4 Restore rule 4), once per flow and process.
pub(crate) fn backup_once(session: &Arc<NetworkSession>, wallet: WalletId, draft: i64) {
    let first = session.platform.registrations.with((wallet, draft), |s| {
        !std::mem::replace(&mut s.backed_up, true)
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

fn lock_status(s: &platform_wallet::wallet::asset_lock::tracked::AssetLockStatus) -> LockStatus {
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

    async fn spendable(&self) -> Spendable {
        let state = self.wallet.state().await;
        let snapshot = crate::coins::read_coins(&state.core_wallet);
        let mut out = Spendable::default();
        for c in snapshot.coins.iter().filter(|c| c.send_account) {
            out.inputs += 1;
            if c.auto_selectable(snapshot.height) {
                out.duffs = out.duffs.saturating_add(c.utxo.value());
            }
        }
        out
    }

    fn ready_to_fund(&self) -> bool {
        use super::startup::StartupStatus;
        let synced = self
            .session
            .hub
            .tracker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .caught_up();
        // Restore rule 1: the same-seed discovery answered which identity
        // indexes the seed uses. A scan that did not (it never reached
        // Platform, failed locally, left indexes unanswered, or ran without
        // signers) holds the funding.
        let brought_up = self
            .session
            .dashpay_startup(&self.wallet_id)
            .is_ok_and(|s| {
                matches!(
                    s.startup,
                    StartupStatus::Ready
                        | StartupStatus::NoIdentity
                        | StartupStatus::PartialAccountsPending
                        | StartupStatus::SeedBindingUnverified
                )
            });
        synced && brought_up
    }

    async fn fund(
        &self,
        lock_duffs: u64,
        identity_index: u32,
        lease: &Lease,
    ) -> Result<OutPoint, FundError> {
        use platform_wallet::AssetLockFundingType;
        let signer = lease
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
            .map_err(fund_error)
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
        lease: &Lease,
    ) -> Result<String, PlatformError> {
        use platform_wallet::wallet::asset_lock::AssetLockFunding;
        let identity_signer = lease.identity_signer(&[identity_index])?;
        let funding_signer = lease.funding_signer()?;
        let keys = super::keys_policy::derive_keys(
            identity_signer.vault_signer(),
            identity_index,
            &super::keys_policy::registration_specs(),
        )
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

    async fn create_profile(
        &self,
        identity: &str,
        identity_index: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        let id = parse_identity(identity)?;
        let has_profile = {
            let state = self.wallet.state().await;
            state
                .identity_manager
                .managed_identity(&id)
                .is_some_and(|m| m.dashpay().profile.is_some())
        };
        if has_profile {
            return Ok(());
        }
        let signer = lease.identity_signer(&[identity_index])?;
        self.wallet
            .identity()
            .dashpay()
            .create_profile_with_external_signer(
                &id,
                platform_wallet::wallet::identity::ProfileUpdate {
                    display_name: profile.display_name.clone(),
                    public_message: profile.public_message.clone(),
                    avatar_url: profile.avatar_url.clone(),
                    avatar_bytes: None,
                },
                &signer,
            )
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
