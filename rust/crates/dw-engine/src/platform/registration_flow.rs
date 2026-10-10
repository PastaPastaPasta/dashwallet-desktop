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
//! - without the signature count, a funding call that failed after it
//!   started and left no tracked row is `MaybeSent`: the row fails, not
//!   retryable, until positive evidence says `NotSent` (H12).

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

/// The coins the library's pooled funding may select (BIP44, BIP32,
/// DashPay receiving).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Spendable {
    pub(crate) duffs: u64,
    pub(crate) inputs: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FundError {
    /// The engine refused before the library call started (a lease that
    /// no longer signs): nothing was signed, so nothing can be out.
    Refused(PlatformError),
    /// The 300 s InstantSend wait ended without a lock; the row waits for
    /// a ChainLock.
    IsTimeout,
    /// The library call failed after it started.
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
    /// `timeout` (`None`: the ChainLock wait, unbounded). Needs no key.
    fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Option<Duration>,
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
    /// A library call of the flow runs: discard is refused.
    pub(crate) in_call: bool,
    /// What the flow waits for, set when it parks or starts a wait.
    pub(crate) wait: Option<RegistrationWait>,
    /// The InstantSend window's end, from an in-memory instant taken at the
    /// funding call (never `updated_at`, §3.4).
    pub(crate) is_until: Option<Instant>,
    /// The lease the flow last ran under.
    pub(crate) lease: Option<LeaseId>,
    /// The automatic backup of `FundingSent` was scheduled.
    pub(crate) backed_up: bool,
    pub(crate) task: Option<tokio::task::AbortHandle>,
}

/// Every flow's [`FlowState`], keyed by wallet and draft.
#[derive(Debug, Default)]
pub(crate) struct Flows {
    flows: Mutex<HashMap<(WalletId, i64), FlowState>>,
    /// Orders `start_registration`'s in-progress check and its insert.
    pub(crate) start: tokio::sync::Mutex<()>,
}

impl Flows {
    pub(crate) fn with<R>(&self, key: (WalletId, i64), f: impl FnOnce(&mut FlowState) -> R) -> R {
        let mut map = self.flows.lock().unwrap_or_else(PoisonError::into_inner);
        f(map.entry(key).or_default())
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
            Some(Claim {
                flows: Arc::clone(self),
                key,
            })
        })
    }

    /// Aborts the flow's task unless a library call of it runs; `false`
    /// when one does. Atomic with the driver's marking of its calls.
    pub(crate) fn stop_unless_in_call(&self, key: (WalletId, i64)) -> bool {
        self.with(key, |s| {
            if s.in_call {
                return false;
            }
            if let Some(task) = s.task.take() {
                task.abort();
            }
            true
        })
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
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.flows.with(self.key, |s| {
            s.running = false;
            s.in_call = false;
            s.task = None;
        });
    }
}

/// Marks a library call of the flow; cleared when dropped.
struct InCall<'a>(&'a Flows, (WalletId, i64));

impl Drop for InCall<'_> {
    fn drop(&mut self) {
        self.0.with(self.1, |s| s.in_call = false);
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

/// The tracked lock a `CoreBalance` draft at `index` funds: by the draft's
/// own funding key, so it is found even when no outpoint reached the row
/// (E0-04 §2a.5, review Opus r3 F-2). A lock that matches several drafts
/// counts for each, which fails closed.
pub(crate) fn matching_lock(locks: &[LockInfo], index: u32) -> Option<&LockInfo> {
    locks
        .iter()
        .find(|l| l.registration && l.identity_index == index && l.account == 0)
}

/// What the engine's funding gate reads for a row's funding (E0-04 §16.6):
/// `None` and `Some(NotSent)` allow a discard or a second funding.
///
/// P2b seam: §2a.5's derivation from the funding marker and its definite
/// resolutions joins here; P2a has no markers, so this reads the row and
/// the tracked locks only.
pub(crate) fn funding_status(row: &RegistrationRow, locks: &[LockInfo]) -> Option<DispatchState> {
    let Decoded::Known(Funding::CoreBalance { .. }) = Funding::decode(&row.funding) else {
        // No funding of the user's: an existing identity, or a lock that is
        // the inviter's or the faucet's.
        return None;
    };
    let lock = outpoint_of(row)
        .and_then(|op| locks.iter().find(|l| l.outpoint == op))
        .or_else(|| row.identity_index.and_then(|i| matching_lock(locks, i)));
    if let Some(lock) = lock {
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
        // hand): possibly out, until positive evidence settles it. A phase
        // this build does not know counts by its outpoint alone: every
        // build writes it before the broadcast (Restore rule 3).
        return Some(DispatchState::MaybeSent);
    }
    None
}

/// `funds_committed` (E0-04 §16.5) for a row: a funding committed and not
/// definitely unsent, or a funding call running now.
pub(crate) fn funds_committed(row: &RegistrationRow, locks: &[LockInfo], in_call: bool) -> bool {
    let funded = matches!(
        funding_status(row, locks),
        Some(DispatchState::WillBeSent | DispatchState::MaybeSent | DispatchState::Sent)
    );
    let core = matches!(
        Funding::decode(&row.funding),
        Decoded::Known(Funding::CoreBalance { .. })
    );
    funded || (core && in_call)
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
    for row in appdb.registration_rows(wallet)? {
        let funds_it = row.error.is_none()
            && row.phase == Phase::KeysPrepared.as_str()
            && row.identity_index == Some(identity_index)
            && row.asset_lock_outpoint.is_none()
            && matches!(
                Funding::decode(&row.funding),
                Decoded::Known(Funding::CoreBalance { .. })
            );
        if !funds_it {
            continue;
        }
        let change = RegistrationChange {
            phase: Some(Phase::FundingSent.as_str().into()),
            outpoint: Some(lock.clone()),
            ..Default::default()
        };
        let written = appdb.update_registration(
            wallet,
            row.id,
            Phase::KeysPrepared.as_str(),
            false,
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
    pub(crate) session: &'a Arc<NetworkSession>,
    pub(crate) wallet: WalletId,
    pub(crate) lib: &'a L,
    /// The authority the run signs with; `None` runs only the steps that
    /// need no key (the proof wait) and parks at the first that does.
    pub(crate) lease: Option<Lease>,
    /// Wait for a missing proof in this run (`false`: park on it, as
    /// "Finish transfers" does).
    pub(crate) wait_for_proof: bool,
}

impl<L: RegistrationLib> Driver<'_, L> {
    fn flows(&self) -> &Flows {
        &self.session.platform.registrations
    }

    /// Steps `draft` until it parks, fails or ends. An error is a storage
    /// failure: the row keeps the phase it had, and the flow is resumable.
    pub(crate) async fn run(&self, draft: i64) -> Result<(), PlatformError> {
        let key = (self.wallet, draft);
        if let Some(lease) = &self.lease {
            self.flows().with(key, |s| s.lease = Some(lease.id()));
        }
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
                Step::Park(wait) => {
                    tracing::info!(draft, phase = %row.phase, ?wait, "registration parked");
                    self.flows().with(key, |s| s.wait = Some(wait));
                    return Ok(());
                }
                Step::Fail(code, retryable) => {
                    tracing::info!(draft, phase = %row.phase, code, retryable, "registration failed");
                    let change = RegistrationChange {
                        failure: Some(Some((code.into(), retryable))),
                        ..Default::default()
                    };
                    self.write(&row, change).await?;
                    return Ok(());
                }
                Step::Stop => return Ok(()),
            }
        }
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
            RegistrationUpdate::Applied | RegistrationUpdate::Stale => Step::Again,
            // Another flow holds the lock this one would adopt: two drafts
            // on one funding key, which the engine never makes.
            RegistrationUpdate::LockTaken => Step::Fail(internal(), false),
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
        let index = self.next_index().await?;
        self.advance(
            row,
            RegistrationChange {
                phase: Some(Phase::KeysPrepared.as_str().into()),
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
        let locks = self.lib.tracked_locks().await;
        if let Some(lock) = matching_lock(&locks, index) {
            return self
                .record_lock(row, lock.outpoint, Phase::FundingSent)
                .await;
        }
        // The funding gate (H11): a second funding only after `NotSent` or
        // `None`. P2b seam: the funding marker's status joins here.
        if let Some(state) = funding_status(row, &locks)
            && state != DispatchState::NotSent
        {
            return Ok(Step::Fail(broadcast_unknown(), false));
        }
        let Some(lease) = &self.lease else {
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
        self.flows().with(key, |s| {
            s.in_call = true;
            s.is_until = Some(Instant::now() + IS_WINDOW);
            s.wait = Some(RegistrationWait::InstantSend);
        });
        let in_call = InCall(self.flows(), key);
        let funded = lease
            .scope_step(Some(funding_step(row.id)), async {
                let call = lease
                    .call_permit(funding_step(row.id), None)
                    .map_err(|e| FundError::Refused(e.into()))?;
                let funded = self.lib.fund(lock_duffs, index, lease).await;
                call.finish(funded.is_ok());
                funded
            })
            .await;
        drop(in_call);
        self.flows().with(key, |s| s.is_until = None);
        // platform-wallet tracks the lock (`Built`, stored through the tap)
        // before it hands it to the broadcaster, and untracks it only on a
        // definitive rejection (`build.rs` `broadcast_funded_asset_lock_with_funding`).
        // So after the call, a tracked row is a committed lock, and no
        // tracked row for the draft's funding key is positive evidence that
        // nothing reached the wire: `NotSent` (H12), and the row is
        // unfunded and retryable again.
        let tracked = matching_lock(&self.lib.tracked_locks().await, index).map(|l| l.outpoint);
        match (funded, tracked) {
            (Ok(outpoint), _) => self.record_lock(row, outpoint, Phase::FundingSent).await,
            (Err(FundError::IsTimeout), Some(outpoint)) => {
                self.drop_own_key(lease);
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
            (Err(e), None) => {
                tracing::info!(draft = row.id, "the funding call failed untracked: {e:?}");
                let code = match &e {
                    FundError::Failed(e) => e.code(),
                    _ => RegistrationError::IslockTimeout.code(),
                };
                self.unfund(row.id, code).await
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

    /// An own-key lease drops its key when the InstantSend window ends
    /// (§2.6): the flow parks keyless and asks again at identity creation.
    fn drop_own_key(&self, lease: &Lease) {
        if lease.view().is_some_and(|v| {
            v.own_key
                && matches!(
                    v.state,
                    LeaseStateView::Active | LeaseStateView::AwaitingProof
                )
        }) {
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
        if let Some(id) = self.identity_at(index).await {
            return self.advance(row, registered(id)).await;
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
            return Ok(self.wait_proof(row.id, outpoint).await);
        }
        let Some(lease) = &self.lease else {
            return Ok(Step::Park(self.authority_wait()));
        };
        // The call holds a plain call permit. P2b seam: the step marker of
        // `identity_step` written before the call.
        let key = (self.wallet, row.id);
        self.flows().with(key, |s| s.in_call = true);
        let in_call = InCall(self.flows(), key);
        let created = lease
            .scope_step(Some(identity_step(row.id)), async {
                let call = lease.call_permit(identity_step(row.id), None)?;
                let created = self.lib.register_identity(outpoint, index, lease).await;
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
    /// the ChainLock with no limit. No key is needed; an own key is dropped
    /// when the window ends.
    async fn wait_proof(&self, draft: i64, outpoint: OutPoint) -> Step {
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
            && let Some(lease) = &self.lease
        {
            self.drop_own_key(lease);
        }
        if !self.wait_for_proof {
            return Step::Park(wait);
        }
        self.flows().with(key, |s| s.wait = Some(wait));
        match self.lib.wait_proof(outpoint, window).await {
            Ok(()) => Step::Again,
            Err(FundError::IsTimeout) => {
                self.flows().with(key, |s| s.is_until = None);
                Step::Again
            }
            Err(FundError::Refused(e) | FundError::Failed(e)) => self.failed(&e),
        }
    }

    /// NameRequested → NameRegistered | Contested, then the temporary name.
    async fn names(&self, row: &RegistrationRow) -> Result<Step, PlatformError> {
        let Some(identity) = row.identity.as_deref() else {
            return Ok(Step::Fail(internal(), false));
        };
        let Some(lease) = &self.lease else {
            return Ok(Step::Park(self.authority_wait()));
        };
        match self.lib.register_name(identity, &row.label, lease).await {
            Ok(NameOutcome::Registered) => self.to_phase(row, Phase::NameRegistered).await,
            Ok(NameOutcome::ContestStarted { .. }) => {
                if let Some(temp) = row.temp_label.as_deref() {
                    match self.lib.register_name(identity, temp, lease).await {
                        Ok(_) => {}
                        Err(NameError::Platform(e))
                            if !matches!(self.failed(&e), Step::Fail(..)) =>
                        {
                            return Ok(self.failed(&e));
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
        let Some(lease) = &self.lease else {
            return Ok(Step::Park(self.authority_wait()));
        };
        match self
            .lib
            .create_profile(identity, index, &profile, lease)
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
    wait_for_proof: bool,
) -> bool {
    let flows = &session.platform.registrations;
    let Some(claim) = flows.claim((wallet, draft)) else {
        return false;
    };
    let task = {
        let session = Arc::clone(session);
        let lease = lease.clone();
        session.rt.clone().spawn(async move {
            let _claim = claim;
            let driver = Driver {
                session: &session,
                wallet,
                lib: &*lib,
                lease,
                wait_for_proof,
            };
            if let Err(e) = driver.run(draft).await {
                tracing::warn!(draft, error = %e, "registration stopped");
            }
        })
    };
    let handle = task.abort_handle();
    flows.with((wallet, draft), |s| {
        if s.running {
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
    match e {
        platform_wallet::PlatformWalletError::FinalityTimeout(_) => FundError::IsTimeout,
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
        let coins = snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && c.auto_selectable(snapshot.height));
        let mut out = Spendable::default();
        for c in coins {
            out.duffs = out.duffs.saturating_add(c.utxo.value());
            out.inputs += 1;
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
        let brought_up = self
            .session
            .dashpay_startup(&self.wallet_id)
            .is_ok_and(|s| !matches!(s.startup, StartupStatus::NotRun | StartupStatus::Starting));
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

    async fn wait_proof(
        &self,
        outpoint: OutPoint,
        timeout: Option<Duration>,
    ) -> Result<(), FundError> {
        self.wallet
            .asset_locks()
            .resume_asset_lock(&outpoint, timeout)
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
        let identity = Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
            NameError::from(PlatformError::InvalidArgument {
                detail: "identity is not a base58 identifier".into(),
            })
        })?;
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
        let id = Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
            PlatformError::InvalidArgument {
                detail: "identity is not a base58 identifier".into(),
            }
        })?;
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
        let id = Identifier::from_string(identity, Encoding::Base58).map_err(|_| {
            PlatformError::InvalidArgument {
                detail: "identity is not a base58 identifier".into(),
            }
        })?;
        Ok(IdentityBalance::fetch(&self.sdk, id).await?)
    }
}
