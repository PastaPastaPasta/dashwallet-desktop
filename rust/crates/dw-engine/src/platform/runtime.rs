//! The session's Platform runtime (DASHPAY §3.1 `PlatformRuntime`; E0-05):
//! the supervisor slot, each wallet's bring-up status, the vault's lock
//! state as the bring-up sees it, and the Platform sync loops with their
//! cadence. The bring-up itself is in `bringup.rs`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockWriteGuard};
use std::time::{Duration, Instant};

use dw_vault::{LockState, Vault};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};

use super::registration::RegistrationWait;
use super::startup::{StartupStatus, SyncLoop, SyncLoopStatus};
use super::startup_status::{DashPayStartup, PlatformCadence, SpvState};
use crate::events::SessionHub;
use crate::session::Manager;
use crate::{EngineError, NetworkSession, PlatformChange, WalletId};

/// The drained notes held in upstream's store, by wallet: `(account, nullifier)`.
pub(crate) type DrainedHeld = HashMap<[u8; 32], HashSet<(u32, [u8; 32])>>;

/// `dashpay_sync` while a window is visible, and while hidden (§3.2).
const DASHPAY_VISIBLE: Duration = Duration::from_secs(15);
const DASHPAY_HIDDEN: Duration = Duration::from_secs(60);
/// The DPNS marketplace loop (`dpns_sync`), and while the host follows a
/// contest's last hour (`PlatformCadence::contest_ending_soon`). The own
/// contests have their watch (`contest_watch.rs`).
const MARKETPLACE: Duration = Duration::from_secs(600);
const MARKETPLACE_ENDING: Duration = Duration::from_secs(60);
/// The Platform-address balance pass (release 0.1): a window showing it,
/// none (the library's default is 15 s; a pass is a proof query per
/// address-tree branch).
const PLATFORM_ADDRESSES_VISIBLE: Duration = Duration::from_secs(60);
const PLATFORM_ADDRESSES_HIDDEN: Duration = Duration::from_secs(300);

/// How long a stop or close waits for the bring-up's blocking vault work (a
/// keyring read) after cancelling it. A read that takes longer is stuck, for
/// example behind a keyring prompt nobody answers: the stop or close then
/// reports it instead of hanging.
pub(super) const KEY_WORK_JOIN: Duration = Duration::from_secs(5);

/// The pause after a flow's first request to run a wallet's bring-up again
/// ([`PlatformRuntime::retry_startup`]), doubling with each request up to
/// [`STARTUP_RETRY_MAX`]. The first request runs it at once.
pub(super) const STARTUP_RETRY_MIN: Duration = Duration::from_secs(30);
pub(super) const STARTUP_RETRY_MAX: Duration = Duration::from_secs(600);

/// How long a queued-account drain that left the work queued holds back the
/// next one a changeset asks for (#84). An unlock drains whenever.
pub(super) const CONTACT_DRAIN_BACKOFF: Duration = Duration::from_secs(300);

/// The queued-account drains asked of the supervisor (#84): a sweep stores
/// the queue again for every contact still without its accounts.
#[derive(Default)]
pub(super) struct ContactDrains {
    /// A drain is asked for and its job has not started.
    requested: HashSet<WalletId>,
    /// When the wallet's last such drain left work queued.
    failed: HashMap<WalletId, Instant>,
}

/// Work the supervisor takes after SPV has started.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PlatformSignal {
    /// The vault became prompt-free (unlocked with scope Full, unencrypted).
    Unlocked,
    /// A wallet was registered, got keys or was opened.
    WalletAdded(WalletId),
    /// A restore's last mark on a wallet cleared after an entry point was
    /// refused for it (review r3 M4-R3): bring it up if it is still there.
    Readmit(WalletId),
    /// A flow waits on the wallet's bring-up, whose outcome only a run
    /// again changes (`StartupStatus::awaits_rerun`): run it again, with
    /// the library's budget and whatever Platform proved before.
    RetryStartup(WalletId),
    /// A sync queued a contact's account build for the wallet: drain it now
    /// if the vault is prompt-free, else it waits for the next unlock.
    ContactAccountsQueued(WalletId),
    /// A contest watch row was written (DP1-04): a pass now, and the
    /// schedule again.
    Contests,
}

/// A signal and when its event happened, on [`PlatformRuntime::stamp`]'s
/// clock.
pub(super) type Stamped = (u64, PlatformSignal);

pub(super) struct Supervisor {
    pub(super) cancel: watch::Sender<bool>,
    pub(super) signals: mpsc::UnboundedSender<Stamped>,
    /// Set right after the slot is installed (`start_spv`).
    pub(super) task: Option<JoinHandle<()>>,
    /// SPV has started: the supervisor is past its step 2.
    pub(super) running: bool,
}

/// The vault's lock state as the session last reported it, and how many
/// times it stopped being prompt-free: a bring-up holding keys watches the
/// count, which two reports racing each other cannot set back.
#[derive(Debug, Clone, Copy)]
pub(super) struct LockView {
    pub(super) state: LockState,
    pub(super) locks: u64,
}

/// The session's Platform state (DASHPAY §3.1 `PlatformRuntime`).
pub(crate) struct PlatformRuntime {
    /// `false` for a chain without Platform (`SessionOptions::no_platform`):
    /// SPV starts without a bring-up and no loop runs.
    pub(super) enabled: bool,
    /// Unit tests drive Platform flows over fakes in sessions without
    /// Platform; [`Self::funds_off`] refuses them there only when a test
    /// sets this.
    #[cfg(test)]
    pub(crate) funds_off_in_tests: AtomicBool,
    pub(super) supervisor: Mutex<Option<Supervisor>>,
    /// Orders `start_spv`, `stop_spv` and `reset_chain_data`.
    pub(crate) lifecycle: tokio::sync::Mutex<()>,
    startup: RwLock<HashMap<WalletId, DashPayStartup>>,
    /// Where a changed bring-up outcome is signalled ([`StartupWrite`]).
    hub: Arc<SessionHub>,
    /// Bumped when a wallet's bring-up outcome changes or a rerun a flow
    /// asked for ends: what [`Self::await_bring_up`] waits on.
    startup_seen: watch::Sender<u64>,
    /// Flows inside [`Self::await_bring_up`]: an in-app close they refuse
    /// names them ([`Self::preparing`]).
    bring_up_waiters: AtomicUsize,
    cadence: Mutex<PlatformCadence>,
    pub(super) lock: watch::Sender<LockView>,
    /// Each wallet's bring-up or unlock task in flight, ended when the
    /// wallet is removed or closed.
    pub(super) tasks: Mutex<HashMap<WalletId, AbortHandle>>,
    /// The loops were started and not stopped since; read by passes that
    /// were scheduled earlier.
    pub(super) loops_on: Arc<AtomicBool>,
    /// Held to read by each shielded pass from its admission to its end; to
    /// write by the drain of a lock, a revocation or the close
    /// (`shielded_pool::guarded_pass`, 0.1 E1), and by every change to the
    /// wallets a pass covers: a wallet's load, its shielded bind, its
    /// removal from the manager, the coordinator's configuration (E1 r1
    /// R1-3). So the wallets the library's pass syncs and re-sends for are
    /// the ones its admission checked.
    pub(crate) shielded_passes: Arc<tokio::sync::RwLock<()>>,
    /// Shuts the dispatch hook from the close's start and during each drain
    /// of `shielded_passes`.
    pub(crate) hook_fence: Arc<super::shielded_pool::HookFence>,
    /// The coordinator the engine's dispatch hook is set on: the library
    /// takes a hook once and does not say whether one is set.
    pub(crate) shielded_hooked:
        Mutex<std::sync::Weak<platform_wallet::wallet::shielded::NetworkShieldedCoordinator>>,
    /// This start's drain of the network's shielded store (Q110 110-2),
    /// set once the session's journal open ran it: upstream's shielded
    /// store opens only on `Ok`, and stays closed while this is unset.
    pub(crate) shielded_drain: std::sync::OnceLock<Result<(), super::ShieldedStoreClosed>>,
    /// The drained notes the engine holds in upstream's store, by wallet:
    /// `(account, nullifier)` ([`super::shielded_pool::sync_drained_holds`]).
    pub(crate) drained_held: Mutex<DrainedHeld>,
    /// Unix seconds a periodic pass last proved the shielded pool unchanged
    /// and skipped its scan; `0` before one did.
    pub(crate) shielded_checked_at: AtomicU64,
    pub(crate) shielded_bind_warnings: Mutex<HashSet<&'static str>>,
    /// App Connect responses sealed and not yet published.
    pub(crate) app_connect: super::app_connect::PendingSends,
    /// A test's pause of the next shielded pass, after its admission and
    /// before the library's pass (`shielded_pool::guarded_pass`).
    #[cfg(test)]
    pub(crate) pass_pause: Mutex<Option<super::shielded_pool::PassPause>>,
    /// A test's pause of the next settle of drained spends, between the
    /// pass lock's read and its write (`shielded_pool::pass`).
    #[cfg(test)]
    pub(crate) settle_pause: Mutex<Option<super::shielded_pool::PassPause>>,
    /// The last evidence wish given to the coordinator
    /// (`shielded_pool::want_drained_evidence`).
    #[cfg(test)]
    pub(crate) evidence_wanted: std::sync::atomic::AtomicBool,
    /// A test's proved note count for the periodic pass's check
    /// (`Some(None)`: the read fails); the network's while `None`.
    #[cfg(test)]
    pub(crate) notes_count: Mutex<Option<Option<u64>>>,
    /// A test's pause of the next periodic pass after its count read, while
    /// it holds nothing of the session (`shielded_pool::periodic_pass`).
    #[cfg(test)]
    pub(crate) probe_pause: Mutex<Option<super::shielded_pool::PassPause>>,
    /// A test's block of the next count read's thread until a send or drop,
    /// as a quorum wait's `block_in_place` does.
    #[cfg(test)]
    pub(crate) probe_block: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// Told when the next shielded pass waits for its locks.
    #[cfg(test)]
    pub(crate) spends_queued: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Told when the next shielded pass, its locks taken, is admitted.
    #[cfg(test)]
    pub(crate) admitting: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// How many shielded passes reached the library's pass.
    #[cfg(test)]
    pub(crate) passes_run: std::sync::atomic::AtomicUsize,
    /// A test's proved reads for the dispatch hook's anchors read
    /// (`shielded_release`); the network's while `None`.
    #[cfg(test)]
    pub(crate) release_net: Mutex<Option<Arc<dyn super::shielded_release::ReleaseNet>>>,
    /// Held to read by each shielded pass, before `shielded_passes`; to
    /// write by a spend from its library call to its record, so no pass
    /// runs meanwhile. Apart from `shielded_passes` so a lock's drain waits
    /// for the passes only, not for a spend's network call.
    pub(crate) shielded_spends: tokio::sync::RwLock<()>,
    /// Held to read by each shielded call that signs (a send, an unshield,
    /// a shield, its "Check again") from before it enters the session to
    /// its end; to write by an in-app close from its check of the wallets
    /// to its commit (`Engine::close_network`, E1 r2 R2-3), which a call
    /// running refuses. So no spend starts between that check and the
    /// close.
    pub(crate) close_preflight: tokio::sync::RwLock<()>,
    /// Held to read by a wallet's load from before it enters the session
    /// to its end; to write by an in-app close, which waits (bounded) for
    /// the loads in flight rather than refusing, then holds it to its
    /// commit. So no wallet holding a stored spend loads between the
    /// close's check and the close.
    pub(crate) wallet_loads: tokio::sync::RwLock<()>,
    /// A test's pause of the close's check after its first wallet, with
    /// that wallet (`NetworkSession::holds_shielded_spends`).
    #[cfg(test)]
    pub(crate) close_check_pause: Mutex<Option<super::shielded_pool::CheckPause>>,
    /// Tests: holds the next `NetworkSession::refresh_protocol_version`
    /// before it asks the gate (row 74 r3 M1).
    #[cfg(test)]
    pub(crate) hold_refresh: std::sync::Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next shielded store's first open between its two
    /// reads of the load fence.
    #[cfg(test)]
    pub(crate) hold_between_fence_reads: std::sync::Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: what the library answers `keyset_spends` instead of this
    /// build's answer (`shielded_pool::PwShielded`); this build's while
    /// `None`.
    #[cfg(test)]
    pub(crate) keyset_spends: std::sync::Mutex<Option<bool>>,
    /// Tests: the next invitation-hold settle reports here inside its
    /// admitted vault work, and waits for the receiver (or its sender's
    /// drop) before it retains.
    #[cfg(test)]
    pub(crate) hold_settle_retain:
        std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    /// Each wallet's shielded call lock (`shielded_pool::wallet_lock`).
    pub(crate) shielded_calls: std::sync::Mutex<
        std::collections::HashMap<WalletId, std::sync::Weak<tokio::sync::Mutex<()>>>,
    >,
    /// The addresses each wallet's running fund applies hold
    /// (`platform_moves::FundHold`).
    pub(crate) fund_holds: super::platform_moves::FundHolds,
    /// Each wallet's unshield to a Platform address about to sign
    /// (`shielded_pool::UnshieldIntent`).
    pub(crate) unshield_intents:
        std::sync::Mutex<std::collections::HashMap<WalletId, super::shielded_pool::UnshieldIntent>>,
    /// Each wallet's Shielded-funded registration create about to sign
    /// (`registration_shielded::CreateIntent`).
    pub(crate) registration_intents: std::sync::Mutex<
        std::collections::HashMap<WalletId, super::registration_shielded::CreateIntent>,
    >,
    /// A test's faults of a Shielded-funded create's hook steps.
    #[cfg(test)]
    pub(crate) registration_faults:
        std::sync::Mutex<super::registration_shielded::RegistrationFaults>,
    /// Platform's last proved shielded note count and when it was proved
    /// (unix seconds), for the registration's pool check (C-funding F16).
    pub(crate) pool_notes: std::sync::Mutex<Option<(u64, u64)>>,
    /// The shield locks of unknown consumption marked this session, kept
    /// also in `app.sqlite` (`shielded_pool::ShieldedLib::unknown_locks`).
    pub(crate) shield_unknown: std::sync::Mutex<std::collections::HashSet<dashcore::Txid>>,
    pub(super) key_work: Arc<KeyWork>,
    /// Wallets a restore has begun storing and has not committed or rolled
    /// back (review r2 M4).
    restoring: Mutex<HashMap<WalletId, RestoreMark>>,
    /// The clock that orders signals' events against bring-up admissions
    /// (review r4 M4-R4).
    events: AtomicU64,
    /// Each wallet's requests to run its bring-up again since its outcome
    /// last stopped holding flows ([`Self::retry_startup`]).
    startup_retries: Mutex<HashMap<WalletId, StartupRetry>>,
    /// The identity read model and the names passes (DP1-05).
    pub(crate) recovery: super::recovery::Recovery,
    /// The contacts read model's snapshots and usernames (DP2-01).
    pub(crate) contacts: super::contacts_model::ContactsRuntime,
    /// Search, links and the scans waiting for a send (DP2-03).
    pub(crate) users: super::users::UsersRuntime,
    /// Avatar candidates, the fetch pipeline and the pictures toggle (DP4-02).
    pub(crate) avatars: super::avatar::AvatarRuntime,
    /// The registration flows running in this process (DP1-02).
    pub(crate) registrations: Arc<super::registration_flow::Flows>,
    /// The quoted Platform sends not yet sent (`platform_moves::SendOps`).
    pub(crate) send_ops: super::platform_moves::SendOps,
    /// A test's network for the invitation checks (DP5-02).
    #[cfg(test)]
    pub(crate) invitation_net: Mutex<Option<Arc<dyn super::invitation_check::InvitationNet>>>,
    /// When the contest watch last checked each row (DP1-04).
    pub(crate) contests: super::contest_watch::ContestsRuntime,
    /// The balances spent since their refresh, and the balance pass
    /// (DP1-06, `balances.rs`).
    pub(crate) balances: Arc<super::balances::Balances>,
    /// The network's proved Platform protocol version (`protocol.rs`).
    pub(crate) protocol: Arc<super::protocol::ProtocolRuntime>,
    /// The per-contact payment locks (DP3-01, `payment_locks.rs`).
    pub(crate) payments: super::payment_locks::PaymentLocks,
    /// The timers that signal a relied-on proof of no identity's expiry
    /// (BL-153 D21, D23).
    pub(crate) absence_expiries: super::banner::AbsenceExpiries,
    /// The money-move trust gate's waits (DP3-04, `money_gate.rs`).
    pub(crate) money_gate: super::money_gate::MoneyGate,
    /// Tests: the Identities page's network in place of the SDK
    /// (`identity_reads.rs`).
    #[cfg(test)]
    pub(crate) identity_net: Mutex<Option<Arc<dyn super::identity_reads::IdentityNet>>>,
    /// Tests: holds the next bring-up between building its keys and
    /// starting the library call (`runtime_tests.rs`).
    #[cfg(test)]
    pub(super) pause_after_keys: Mutex<Option<Arc<TestPause>>>,
    /// Tests: holds the next supervisor between starting SPV and marking it
    /// running (`runtime_tests.rs`).
    #[cfg(test)]
    pub(super) pause_before_running: Mutex<Option<Arc<TestPause>>>,
    /// Tests: [`Self::await_bring_up`]'s queued and running waits in place
    /// of [`super::bringup::FLOW_QUEUE_WAIT`] and [`super::bringup::FLOW_WAIT`].
    #[cfg(test)]
    pub(super) bring_up_waits: Mutex<Option<(Duration, Duration)>>,
    /// Tests: holds the next [`Self::await_bring_up`] after it read the
    /// hold and before it waits for a change.
    #[cfg(test)]
    pub(super) pause_bring_up_wait: Mutex<Option<Arc<TestPause>>>,
    /// The queued-account drains asked of the supervisor (#84).
    contact_drains: Mutex<ContactDrains>,
    /// Tests: how many contact-crypto drains ran (`bringup.rs`).
    #[cfg(test)]
    pub(super) contact_drains_run: AtomicU64,
    /// Tests: holds the next queued-account drain job before it starts.
    #[cfg(test)]
    pub(super) pause_contact_drain: Mutex<Option<Arc<TestPause>>>,
}

/// What `DashPay::status` reads of a wallet's bring-up.
fn seen(s: &DashPayStartup) -> (StartupStatus, Option<String>) {
    (s.startup, s.identity.clone())
}

/// [`PlatformRuntime::write_startup`]'s guard: on drop, after the lock is
/// released, it signals the wallets whose outcome changed.
pub(super) struct StartupWrite<'a> {
    map: Option<RwLockWriteGuard<'a, HashMap<WalletId, DashPayStartup>>>,
    before: HashMap<WalletId, (StartupStatus, Option<String>)>,
    hub: &'a SessionHub,
    seen: &'a watch::Sender<u64>,
}

impl std::ops::Deref for StartupWrite<'_> {
    type Target = HashMap<WalletId, DashPayStartup>;

    fn deref(&self) -> &Self::Target {
        self.map.as_ref().expect("held until drop")
    }
}

impl std::ops::DerefMut for StartupWrite<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.map.as_mut().expect("held until drop")
    }
}

impl Drop for StartupWrite<'_> {
    fn drop(&mut self) {
        let map = self.map.take().expect("held until drop");
        let mut changed: Vec<WalletId> = (map.iter())
            .filter(|(id, s)| self.before.get(id) != Some(&seen(s)))
            .map(|(id, _)| *id)
            .collect();
        changed.extend(self.before.keys().filter(|id| !map.contains_key(id)));
        drop(map);
        if !changed.is_empty() {
            self.seen.send_modify(|n| *n = n.wrapping_add(1));
        }
        for id in changed {
            self.hub.pump.mark_platform(id, [PlatformChange::Startup]);
        }
    }
}

/// One flow counted in [`PlatformRuntime::bring_up_waiters`] while it lives.
pub(crate) struct Waiting<'a>(&'a AtomicUsize);

impl<'a> Waiting<'a> {
    fn new(count: &'a AtomicUsize) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count)
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A one-shot pause point for tests.
#[cfg(test)]
#[derive(Default)]
pub(super) struct TestPause {
    pub(super) reached: AtomicBool,
    pub(super) release: tokio::sync::Notify,
}

/// The bring-up's blocking vault work: building its keys can read the OS
/// keyring, on the blocking pool, where cancelling the task that waits for it
/// does not stop it (review DW-E0-05-r1-gpt M2). The work is counted so a stop
/// or close can wait for it; what it builds after a teardown is dropped where
/// it was built, and after close it also locks the vault again, so a data key
/// it loaded does not stay in memory.
#[derive(Debug)]
pub(super) struct KeyWork {
    pending: watch::Sender<usize>,
    /// Bumped by every teardown.
    generation: AtomicU64,
    closed: AtomicBool,
}

impl Default for KeyWork {
    fn default() -> Self {
        Self {
            pending: watch::Sender::new(0),
            generation: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }
}

/// Counts one piece of key work down when it ends, panics included.
struct Counted(Arc<KeyWork>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.pending.send_modify(|n| *n -= 1);
    }
}

impl KeyWork {
    /// Runs `work` on the blocking pool, counted. `None` when it failed or
    /// a teardown came while it ran.
    pub(super) async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        vault: Vault,
        work: impl FnOnce(&Vault) -> Result<T, EngineError> + Send + 'static,
    ) -> Option<T> {
        let generation = self.generation.load(Ordering::SeqCst);
        self.pending.send_modify(|n| *n += 1);
        let counted = Counted(Arc::clone(self));
        let joined = tokio::task::spawn_blocking(move || {
            let this = &counted.0;
            let built = work(&vault);
            if this.closed.load(Ordering::SeqCst) {
                vault.lock();
            }
            if this.generation.load(Ordering::SeqCst) != generation {
                return None;
            }
            built
                .map_err(|e| tracing::debug!(error = %e, "Platform key work refused"))
                .ok()
        })
        .await;
        joined.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Platform key work failed");
            None
        })
    }

    /// Makes work in flight hand nothing back; `closing` also has it lock the
    /// vault when it ends.
    pub(super) fn teardown(&self, closing: bool) {
        if closing {
            self.closed.store(true, Ordering::SeqCst);
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Waits until no key work runs, at most [`KEY_WORK_JOIN`]. `false` when
    /// some is still running.
    pub(super) async fn wait_idle(&self) -> bool {
        let mut pending = self.pending.subscribe();
        tokio::time::timeout(
            KEY_WORK_JOIN,
            super::bringup::until(&mut pending, |n| *n == 0),
        )
        .await
        .is_ok()
    }
}

/// One wallet's restore mark.
#[derive(Debug, Default)]
struct RestoreMark {
    /// Restores holding the mark.
    holds: usize,
    /// An entry point was refused while it was marked; when the last hold
    /// goes, the wallet is readmitted (review r3 M4-R3).
    refused: bool,
}

/// A wallet's bring-up reruns ([`PlatformRuntime::retry_startup`]).
#[derive(Debug, Clone, Copy)]
struct StartupRetry {
    /// Reruns requested so far.
    runs: u32,
    /// None is requested before this.
    next: Instant,
    /// The last rerun asked for has not ended.
    asked: bool,
}

/// The pause after the `runs`th rerun of a bring-up before the next:
/// [`STARTUP_RETRY_MIN`] doubled per rerun after the first, at most
/// [`STARTUP_RETRY_MAX`].
pub(super) fn startup_retry_pause(runs: u32) -> Duration {
    let doublings = runs.saturating_sub(1).min(16);
    (STARTUP_RETRY_MIN * 2u32.pow(doublings)).min(STARTUP_RETRY_MAX)
}

/// Wallets of a restore in progress ([`PlatformRuntime::restoring`]).
pub(crate) struct Restoring<'a> {
    runtime: &'a PlatformRuntime,
    ids: Vec<WalletId>,
}

impl Drop for Restoring<'_> {
    fn drop(&mut self) {
        let mut readmit = Vec::new();
        {
            let mut restoring = guard(&self.runtime.restoring);
            for id in &self.ids {
                if let Some(mark) = restoring.get_mut(id) {
                    mark.holds -= 1;
                    if mark.holds == 0 && restoring.remove(id).is_some_and(|m| m.refused) {
                        readmit.push(*id);
                    }
                }
            }
        }
        // A wallet a rollback removed is skipped by the supervisor.
        for id in readmit {
            self.runtime.signal(PlatformSignal::Readmit(id));
        }
    }
}

/// Whether the vault serves its full key without a prompt (dw-vault
/// `prompt_free_signer`): the states that build the bring-up's providers.
pub(crate) fn prompt_free(state: LockState) -> bool {
    matches!(
        state,
        LockState::NoKeys | LockState::Unencrypted | LockState::Unlocked
    )
}

pub(super) fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl PlatformRuntime {
    pub(crate) fn new(
        lock: LockState,
        enabled: bool,
        hub: Arc<SessionHub>,
        protocol: Arc<super::protocol::ProtocolRuntime>,
    ) -> Self {
        Self {
            enabled,
            #[cfg(test)]
            funds_off_in_tests: AtomicBool::new(false),
            supervisor: Mutex::new(None),
            lifecycle: tokio::sync::Mutex::new(()),
            startup: RwLock::new(HashMap::new()),
            hub,
            startup_seen: watch::Sender::new(0),
            bring_up_waiters: AtomicUsize::new(0),
            cadence: Mutex::new(PlatformCadence::default()),
            lock: watch::Sender::new(LockView {
                state: lock,
                locks: 0,
            }),
            tasks: Mutex::new(HashMap::new()),
            loops_on: Arc::new(AtomicBool::new(false)),
            shielded_passes: Arc::default(),
            hook_fence: Arc::default(),
            shielded_hooked: Mutex::new(std::sync::Weak::new()),
            shielded_drain: std::sync::OnceLock::new(),
            drained_held: Mutex::default(),
            shielded_checked_at: AtomicU64::new(0),
            shielded_bind_warnings: Mutex::default(),
            app_connect: Default::default(),
            #[cfg(test)]
            pass_pause: Mutex::new(None),
            #[cfg(test)]
            settle_pause: Mutex::new(None),
            #[cfg(test)]
            evidence_wanted: Default::default(),
            #[cfg(test)]
            notes_count: Mutex::new(None),
            #[cfg(test)]
            probe_pause: Mutex::new(None),
            #[cfg(test)]
            probe_block: Mutex::new(None),
            #[cfg(test)]
            spends_queued: Mutex::new(None),
            #[cfg(test)]
            admitting: Mutex::new(None),
            #[cfg(test)]
            passes_run: Default::default(),
            #[cfg(test)]
            release_net: Mutex::new(None),
            shielded_spends: tokio::sync::RwLock::new(()),
            close_preflight: tokio::sync::RwLock::new(()),
            wallet_loads: tokio::sync::RwLock::new(()),
            #[cfg(test)]
            close_check_pause: Mutex::new(None),
            #[cfg(test)]
            hold_refresh: std::sync::Mutex::default(),
            #[cfg(test)]
            hold_between_fence_reads: std::sync::Mutex::default(),
            #[cfg(test)]
            keyset_spends: std::sync::Mutex::default(),
            #[cfg(test)]
            hold_settle_retain: std::sync::Mutex::default(),
            shielded_calls: std::sync::Mutex::default(),
            fund_holds: Arc::default(),
            unshield_intents: std::sync::Mutex::default(),
            registration_intents: std::sync::Mutex::default(),
            #[cfg(test)]
            registration_faults: std::sync::Mutex::default(),
            pool_notes: std::sync::Mutex::default(),
            shield_unknown: std::sync::Mutex::default(),
            key_work: Arc::default(),
            restoring: Mutex::new(HashMap::new()),
            events: AtomicU64::new(0),
            startup_retries: Mutex::new(HashMap::new()),
            recovery: Default::default(),
            contacts: Default::default(),
            users: Default::default(),
            avatars: Default::default(),
            registrations: Default::default(),
            send_ops: Default::default(),
            #[cfg(test)]
            invitation_net: Mutex::new(None),
            contests: Default::default(),
            balances: Default::default(),
            protocol,
            payments: Default::default(),
            absence_expiries: Default::default(),
            money_gate: Default::default(),
            #[cfg(test)]
            identity_net: Mutex::new(None),
            #[cfg(test)]
            pause_after_keys: Mutex::new(None),
            #[cfg(test)]
            pause_before_running: Mutex::new(None),
            #[cfg(test)]
            bring_up_waits: Mutex::new(None),
            #[cfg(test)]
            pause_bring_up_wait: Mutex::new(None),
            contact_drains: Mutex::default(),
            #[cfg(test)]
            contact_drains_run: AtomicU64::new(0),
            #[cfg(test)]
            pause_contact_drain: Mutex::new(None),
        }
    }

    /// `Starting` while a supervisor has not started SPV; a supervisor whose
    /// SPV stopped underneath it (a failed `rotate_peers` restart) is
    /// `Stopped`, and the next `start_spv` replaces it.
    fn spv_state(&self, spv_started: bool) -> SpvState {
        match &*guard(&self.supervisor) {
            Some(s) if !s.running => SpvState::Starting,
            _ if spv_started => SpvState::Running,
            _ => SpvState::Stopped,
        }
    }

    /// The session runs with Platform (not `SessionOptions::no_platform`).
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Funds flows that need Platform are refused up front (C7): the
    /// session runs without Platform.
    pub(crate) fn funds_off(&self) -> bool {
        #[cfg(test)]
        if !self.funds_off_in_tests.load(Ordering::SeqCst) {
            return false;
        }
        !self.enabled
    }

    /// SPV runs under a supervisor, and with it the Platform loops.
    pub(super) fn is_running(&self) -> bool {
        self.enabled && guard(&self.supervisor).as_ref().is_some_and(|s| s.running)
    }

    /// Hands `signal` to the supervisor, its event stamped now; dropped
    /// while SPV is stopped (the next start brings every wallet up anyway).
    pub(crate) fn signal(&self, signal: PlatformSignal) {
        self.signal_at(self.stamp(), signal);
    }

    /// [`Self::signal`] for an event stamped earlier.
    pub(crate) fn signal_at(&self, at: u64, signal: PlatformSignal) {
        self.send_signal(at, signal);
    }

    /// [`Self::signal_at`]; whether a supervisor took it.
    fn send_signal(&self, at: u64, signal: PlatformSignal) -> bool {
        self.enabled
            && guard(&self.supervisor)
                .as_ref()
                .is_some_and(|s| s.signals.send((at, signal)).is_ok())
    }

    /// A changeset queued a contact's account build for `id` (#84): one
    /// drain is asked of the supervisor, unless one is asked for already
    /// or the wallet's last such drain left work queued less than
    /// [`CONTACT_DRAIN_BACKOFF`] ago. Called inside the library's store.
    pub(crate) fn contact_accounts_queued(&self, id: WalletId) {
        {
            let mut drains = guard(&self.contact_drains);
            if (drains.failed.get(&id)).is_some_and(|at| at.elapsed() < CONTACT_DRAIN_BACKOFF) {
                return;
            }
            if !drains.requested.insert(id) {
                return;
            }
        }
        if !self.send_signal(self.stamp(), PlatformSignal::ContactAccountsQueued(id)) {
            guard(&self.contact_drains).requested.remove(&id);
        }
    }

    /// The drain job for `id` starts: a changeset from now on asks again.
    pub(super) fn contact_drain_started(&self, id: WalletId) {
        guard(&self.contact_drains).requested.remove(&id);
    }

    /// The job's drain ended; `drained`: it left nothing queued.
    pub(super) fn contact_drain_ended(&self, id: WalletId, drained: bool) {
        let mut drains = guard(&self.contact_drains);
        if drained {
            drains.failed.remove(&id);
        } else {
            drains.failed.insert(id, Instant::now());
        }
    }

    /// Tests: whether `id`'s last queued-account drain left work queued.
    #[cfg(test)]
    pub(super) fn contact_drain_backed_off(&self, id: &WalletId) -> bool {
        guard(&self.contact_drains).failed.contains_key(id)
    }

    /// An unlock drains every wallet: the back-offs end.
    pub(super) fn contact_drains_unlocked(&self) {
        guard(&self.contact_drains).failed.clear();
    }

    /// A new supervisor: nothing is asked of it yet.
    pub(super) fn contact_drains_reset(&self) {
        *guard(&self.contact_drains) = ContactDrains::default();
    }

    /// The time on the clock that orders events against admissions. Stamp
    /// an event after its change is made, and an admission before its pass
    /// reads anything: a pass admitted after an event's stamp sees it.
    pub(crate) fn stamp(&self) -> u64 {
        self.events.fetch_add(1, Ordering::SeqCst)
    }

    /// Called by the session whenever it reports a lock-state change.
    pub(crate) fn note_lock_state(&self, before: LockState, after: LockState) {
        self.lock.send_modify(|view| {
            view.state = after;
            if prompt_free(before) && !prompt_free(after) {
                view.locks += 1;
            }
        });
        if prompt_free(after) && !prompt_free(before) {
            self.signal(PlatformSignal::Unlocked);
        }
    }

    /// A lock the session made itself. Counted even when the state it read
    /// before already said locked: an unlock reported in between would
    /// otherwise hide this lock from a bring-up that holds keys.
    pub(crate) fn note_lock(&self) {
        self.lock.send_modify(|view| view.locks += 1);
    }

    /// Whether the loops the supervisor of `epoch` started still run: no
    /// stop or close since (`NamesRuns::runs`).
    pub(super) fn loops_run(&self, epoch: u64) -> bool {
        self.loops_on.load(Ordering::Acquire) && self.recovery.names_runs.runs(epoch)
    }

    /// Marks SPV started; `false` when a stop or close took the supervisor
    /// meanwhile (it waits for the supervisor, then stops SPV).
    pub(super) fn mark_running(&self) -> bool {
        match &mut *guard(&self.supervisor) {
            Some(s) => {
                s.running = true;
                true
            }
            None => false,
        }
    }

    /// Cancels the supervisor and waits for it (not for its key work: see
    /// [`KeyWork::wait_idle`]). Statuses it left at `Starting` become
    /// `NotRun`. `closing`: the session is closing.
    pub(crate) async fn deactivate(&self, closing: bool) {
        self.key_work.teardown(closing);
        let supervisor = guard(&self.supervisor).take();
        if let Some(s) = supervisor {
            let _ = s.cancel.send(true);
            if let Some(task) = s.task
                && let Err(e) = task.await
            {
                tracing::warn!(error = %e, "Platform supervisor ended abnormally");
            }
        }
        // Work its tasks started before they saw the cancel.
        self.key_work.teardown(closing);
        self.loops_on.store(false, Ordering::Release);
        // A background balance refresh or names check holds the wallet:
        // none outlives a close (DP1-06, DP1-08 D27).
        self.recovery.names_runs.stop();
        self.protocol.stop();
        if closing {
            self.balances.close().await;
            self.protocol.close().await;
            self.recovery.names_runs.close().await;
        }
        for entry in self.write_startup().values_mut() {
            if entry.startup == StartupStatus::Starting {
                entry.startup = StartupStatus::NotRun;
            }
        }
        // The next start brings every wallet up anyway.
        guard(&self.startup_retries).clear();
    }

    /// Waits for the bring-up's blocking key work, at most
    /// [`KEY_WORK_JOIN`]; `false` when some still runs.
    pub(crate) async fn key_work_idle(&self) -> bool {
        self.key_work.wait_idle().await
    }

    /// The outcomes, to change: each wallet whose outcome or named identity
    /// differs when the guard drops gets `Platform{Startup}` (BL-153 D8).
    pub(super) fn write_startup(&self) -> StartupWrite<'_> {
        let map = self.startup.write().unwrap_or_else(|p| p.into_inner());
        let before = map.iter().map(|(id, s)| (*id, seen(s))).collect();
        StartupWrite {
            map: Some(map),
            before,
            hub: &self.hub,
            seen: &self.startup_seen,
        }
    }

    pub(super) fn startup_of(&self, id: &WalletId) -> Option<DashPayStartup> {
        self.startup
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    pub(super) fn record(&self, id: WalletId, startup: DashPayStartup) {
        let unasked = {
            let mut retries = guard(&self.startup_retries);
            if !startup.startup.awaits_rerun() {
                retries.remove(&id).is_some_and(|r| r.asked)
            } else if let Some(retry) = retries.get_mut(&id) {
                std::mem::replace(&mut retry.asked, false)
            } else {
                false
            }
        };
        self.write_startup().insert(id, startup);
        if unasked {
            self.startup_seen.send_modify(|n| *n = n.wrapping_add(1));
        }
    }

    /// The rerun of wallet `id`'s bring-up that a flow asked for ended, or
    /// the supervisor dropped it: whatever it recorded, it no longer runs.
    pub(super) fn rerun_ended(&self, id: WalletId) {
        if let Some(retry) = guard(&self.startup_retries).get_mut(&id) {
            retry.asked = false;
        }
        self.startup_seen.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// A rerun of wallet `id`'s bring-up was asked for and has not ended,
    /// or a bring-up of it runs.
    pub(super) fn bring_up_pending(&self, id: &WalletId) -> bool {
        guard(&self.startup_retries)
            .get(id)
            .is_some_and(|r| r.asked)
            || self.starting(id)
    }

    fn starting(&self, id: &WalletId) -> bool {
        self.startup_of(id)
            .is_some_and(|s| s.startup == StartupStatus::Starting)
    }

    /// A flow about to act on wallet `id` whose library holds it at
    /// `hold()`: on a [`RegistrationWait::Startup`] hold, waits for the
    /// bring-up's rerun ([`Self::await_bring_up`]). The caller reads its
    /// hold again after: this decides nothing.
    pub(crate) async fn settle_startup_hold(
        &self,
        id: WalletId,
        hold: impl Fn() -> Option<RegistrationWait>,
    ) {
        let on_startup = || hold() == Some(RegistrationWait::Startup);
        if on_startup() {
            self.await_bring_up(id, on_startup).await;
        }
    }

    /// `hold`, a read of wallet `id`'s funding hold, after asking for the
    /// bring-up's rerun ([`Self::retry_startup`]) when it is
    /// [`RegistrationWait::Startup`]: nothing else runs it again before the
    /// next start.
    pub(crate) fn ask_on_startup_hold(
        &self,
        id: WalletId,
        hold: Option<RegistrationWait>,
    ) -> Option<RegistrationWait> {
        if hold == Some(RegistrationWait::Startup) {
            self.retry_startup(id);
        }
        hold
    }

    /// A flow held on wallet `id`'s bring-up (`held`): asks for the rerun
    /// ([`Self::retry_startup`]) and waits while one is asked for or runs,
    /// until `held` lifts or the supervisor stops; at most
    /// [`super::bringup::FLOW_QUEUE_WAIT`] for the rerun to start, then
    /// [`super::bringup::FLOW_WAIT`] from its start. Nothing while SPV is
    /// stopped.
    async fn await_bring_up(&self, id: WalletId, held: impl Fn() -> bool) {
        // Subscribed before any read below: a change from here on wakes it.
        let mut seen = self.startup_seen.subscribe();
        let Some(mut cancel) = guard(&self.supervisor)
            .as_ref()
            .filter(|s| s.running)
            .map(|s| s.cancel.subscribe())
        else {
            return;
        };
        let _waiting = Waiting::new(&self.bring_up_waiters);
        let (queued, running) = self.bring_up_waits();
        let mut deadline = Instant::now() + queued;
        let mut started = false;
        self.retry_startup(id);
        while held() && self.bring_up_pending(&id) && !*cancel.borrow() {
            if !started && self.starting(&id) {
                started = true;
                deadline = Instant::now() + running;
            }
            #[cfg(test)]
            {
                let pause = guard(&self.pause_bring_up_wait).take();
                if let Some(pause) = pause {
                    pause.reached.store(true, Ordering::SeqCst);
                    pause.release.notified().await;
                }
            }
            tokio::select! {
                biased;
                () = super::bringup::until(&mut cancel, |c| *c) => return,
                changed = tokio::time::timeout_at(deadline.into(), seen.changed()) => {
                    if !matches!(changed, Ok(Ok(()))) {
                        return;
                    }
                }
            }
        }
    }

    /// A flow waits on a wallet's bring-up ([`Self::await_bring_up`]).
    pub(crate) fn preparing(&self) -> bool {
        self.bring_up_waiters.load(Ordering::SeqCst) > 0
    }

    /// Counts as a flow waiting on a bring-up while it lives.
    #[cfg(all(debug_assertions, feature = "test-support"))]
    pub(crate) fn waiting(&self) -> Waiting<'_> {
        Waiting::new(&self.bring_up_waiters)
    }

    /// [`Self::await_bring_up`]'s waits: queued, then running.
    fn bring_up_waits(&self) -> (Duration, Duration) {
        #[cfg(test)]
        if let Some(waits) = *guard(&self.bring_up_waits) {
            return waits;
        }
        (super::bringup::FLOW_QUEUE_WAIT, super::bringup::FLOW_WAIT)
    }

    /// A flow is held on wallet `id`'s bring-up (`StartupStatus::awaits_rerun`;
    /// DP1-02 §3.4 Restore rule 1, a top-up): has the supervisor run it again,
    /// at once the first time, then at most once per
    /// [`startup_retry_pause`], from 30 s doubling to 10 minutes, for as long
    /// as flows keep asking and the outcome keeps holding them. Nothing while
    /// SPV is stopped (the next start runs it), for a wallet without keys,
    /// while a bring-up of it runs, or while one asked for has not ended.
    /// `true` when it asked.
    pub(crate) fn retry_startup(&self, id: WalletId) -> bool {
        self.retry_startup_at(id, Instant::now())
    }

    /// Wallet `id`'s last bring-up outcome holds flows until it runs again:
    /// one of [`StartupStatus::awaits_rerun`] for a wallet with keys, or none
    /// on record (it never ran, or the library refused it), which reads as
    /// `NotRun`.
    pub(super) fn awaits_rerun(&self, id: &WalletId) -> bool {
        self.startup_of(id)
            .is_none_or(|s| s.startup.awaits_rerun() && !s.read_only)
    }

    /// [`Self::retry_startup`] at `now`.
    pub(super) fn retry_startup_at(&self, id: WalletId, now: Instant) -> bool {
        if !self.awaits_rerun(&id) || !self.is_running() {
            return false;
        }
        {
            let mut retries = guard(&self.startup_retries);
            let retry = retries.entry(id).or_insert(StartupRetry {
                runs: 0,
                next: now,
                asked: false,
            });
            if retry.asked || now < retry.next {
                return false;
            }
            retry.runs += 1;
            retry.next = now + startup_retry_pause(retry.runs);
            // Before the send: a rerun that ends at once clears it after.
            retry.asked = true;
        }
        tracing::info!(wallet_id = %id, "a flow waits on the DashPay bring-up: running it again");
        if !self.send_signal(self.stamp(), PlatformSignal::RetryStartup(id)) {
            self.rerun_ended(id);
        }
        true
    }

    /// Sets wallet `id`'s startup status; the one it replaced, if any.
    pub(super) fn set_status(&self, id: WalletId, status: StartupStatus) -> Option<StartupStatus> {
        let mut startup = self.write_startup();
        if let Some(entry) = startup.get_mut(&id) {
            return Some(std::mem::replace(&mut entry.startup, status));
        }
        startup.insert(id, DashPayStartup::new(status, false));
        None
    }

    /// Whether bring-up or unlock work may start for `id` now: `None` while a
    /// restore of it has not committed, else `Some(has_seed())`. One lock
    /// covers both reads, and a restore marks its wallets under it before it
    /// stores a seed, so no entry point sees a restored seed before the
    /// commit (review r2 M4).
    /// A refusal is remembered: the wallet is readmitted when the last
    /// restore marking it ends.
    pub(super) fn admit(&self, id: &WalletId, has_seed: impl FnOnce() -> bool) -> Option<bool> {
        let mut restoring = guard(&self.restoring);
        match restoring.get_mut(id) {
            Some(mark) => {
                mark.refused = true;
                None
            }
            None => Some(has_seed()),
        }
    }

    /// Marks `ids` as being restored until the returned guard drops: drop it
    /// once the restore has committed, before signalling the bring-up, or
    /// after its rollback.
    pub(crate) fn restoring(&self, ids: Vec<WalletId>) -> Restoring<'_> {
        let mut restoring = guard(&self.restoring);
        for id in &ids {
            restoring.entry(*id).or_default().holds += 1;
        }
        Restoring { runtime: self, ids }
    }

    /// Forgets a removed or closed wallet and ends its bring-up.
    pub(crate) fn forget(&self, id: &WalletId) {
        if let Some(task) = guard(&self.tasks).remove(id) {
            task.abort();
        }
        self.write_startup().remove(id);
        guard(&self.startup_retries).remove(id);
        {
            let mut drains = guard(&self.contact_drains);
            drains.requested.remove(id);
            drains.failed.remove(id);
        }
        self.recovery.forget(id);
        self.contacts.forget(id);
        self.users.scans.forget(id);
        self.registrations.forget_wallet(*id);
        self.contests.forget(id);
        self.balances.forget(id);
        self.payments.forget(id);
        self.absence_expiries.forget(id);
    }

    /// Takes wallet `id` out of `manager` (a removal, an unload, a restore's
    /// rollback, the close): its fenced writes in flight (private details,
    /// the bell's `mark_read`) land first and none lands afterwards ([`super::private_details::WriteFences`]), and its
    /// digests leave memory.
    ///
    /// The engine's only route to platform-wallet's `remove_wallet`, which
    /// at the pin always purges the wallet's shielded store, its stored
    /// spends included (`wallet_lifecycle.rs:1011-1013` →
    /// `coordinator.rs:1041-1068` → `file_store.rs:1734-1761`). So a
    /// wallet holding a spend whose result is not known stays
    /// (`ShieldedSpendUnresolved`, DEC-1046), as does one with a marked
    /// spend (`leases`' journal, DEC-1057), on every route: the user's
    /// removal too (Q86 Q-PURGE C2), which checks first
    /// (`shielded_pool::removal_hold`). The removal runs with no shielded
    /// pass in flight (`shielded_passes`, R1-3): the coordinator's set of
    /// wallets changes only between passes.
    /// Every caller ended the wallet's permits first (its revocation, the
    /// close's), so no spend of it stores a transition after the check.
    pub(crate) async fn remove_from_manager(
        &self,
        leases: &super::lease::LeaseTable,
        manager: &Manager,
        appdb: &Arc<dw_appdb::AppDb>,
        id: WalletId,
    ) -> Result<(), EngineError> {
        let _fence = self.contacts.fences.removal(id).await;
        let _passes = self.shielded_passes.write().await;
        if super::shielded_pool::holds_unresolved(leases, manager, appdb, id).await {
            return Err(EngineError::ShieldedSpendUnresolved);
        }
        manager.remove_wallet(&id.0).await?;
        self.contacts.digests.forget(&id);
        Ok(())
    }

    pub(super) fn untrack(&self, id: WalletId, task: tokio::task::Id) {
        let mut tasks = guard(&self.tasks);
        if tasks.get(&id).is_some_and(|t| t.id() == task) {
            tasks.remove(&id);
        }
    }
}

impl NetworkSession {
    /// Whether a loaded wallet holds a shielded spend whose result is not
    /// known: an in-app close of the session is refused then (DEC-1046,
    /// `Engine::close_network`). A signing shielded call in flight, which
    /// may store one before the close's gate lets it end, holds
    /// `close_preflight` to read and refuses the close there; an unload in
    /// flight is no payment, and the close's gate waits for it. `false`
    /// once closed. Only under [`PlatformRuntime::close_preflight`] held to
    /// write does the answer hold until the close commits.
    pub(crate) async fn holds_shielded_spends(&self) -> bool {
        let Ok(live) = self.live() else {
            return false;
        };
        for id in live.manager.list_wallet_ids_blocking() {
            let id = WalletId(id);
            if super::shielded_pool::holds_unresolved(&self.leases, &live.manager, &live.appdb, id)
                .await
            {
                return true;
            }
            #[cfg(test)]
            {
                let pause = guard(&self.platform.close_check_pause).take();
                if let Some(pause) = pause {
                    let _ = pause.reached.send(id);
                    let _ = pause.resume.await;
                }
            }
        }
        false
    }

    /// The removal gate's hold ([`super::shielded_pool::removal_hold`]) of
    /// any loaded wallet, carried over first: why a restore of this
    /// network's earlier data is refused while it is open.
    pub(crate) async fn restore_hold(&self) -> Option<crate::ShieldedHold> {
        let live = self.live().ok()?;
        let mut held = None;
        for id in live.manager.list_wallet_ids_blocking() {
            match super::shielded_pool::removal_hold(self, &live.manager, &live.appdb, WalletId(id))
                .await
            {
                Some(crate::ShieldedHold::CarriedOver) => {
                    return Some(crate::ShieldedHold::CarriedOver);
                }
                Some(hold) => held = Some(hold),
                None => {}
            }
        }
        held
    }

    /// Whether SPV is starting or running ([`Self::spv_state`] tells which).
    /// While starting, calls that need a running SPV (`rescan`,
    /// `rotate_peers`, a broadcast) still fail as when it is stopped.
    pub fn spv_running(&self) -> Result<bool, EngineError> {
        Ok(self.spv_state()? != SpvState::Stopped)
    }

    /// SPV's state, `Starting` included. In-memory read.
    pub fn spv_state(&self) -> Result<SpvState, EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        Ok(self.platform_spv_state(&manager))
    }

    pub(crate) fn platform_spv_state(&self, manager: &Manager) -> SpvState {
        self.platform.spv_state(manager.spv().is_started())
    }

    /// The wallet's last DashPay bring-up. `NotRun` before the first one.
    /// In-memory read.
    pub fn dashpay_startup(&self, wallet: &WalletId) -> Result<DashPayStartup, EngineError> {
        let _op = self.try_enter()?;
        self.require_wallet(wallet)?;
        // Read-only is the vault's answer now: keys attached or rolled back
        // since the last bring-up change it.
        let read_only = !self.vault.has_wallet_secret(&wallet.0);
        let mut startup = self
            .platform
            .startup_of(wallet)
            .unwrap_or_else(|| DashPayStartup::new(StartupStatus::NotRun, read_only));
        startup.read_only = read_only;
        Ok(startup)
    }

    /// The Platform sync loops this session runs. In-memory read.
    pub fn platform_loops(&self) -> Result<Vec<SyncLoopStatus>, EngineError> {
        let _op = self.try_enter()?;
        let m = self.manager()?;
        let status =
            |sync_loop, running: bool, last: Option<u64>, every: Duration| SyncLoopStatus {
                sync_loop,
                running,
                last_run_at: last,
                next_run_at: last.filter(|_| running).map(|t| t + every.as_secs()),
            };
        let (identity, dashpay, dpns) = (m.identity_sync(), m.dashpay_sync(), m.dpns_sync());
        let addresses = m.platform_address_sync();
        Ok(vec![
            status(
                SyncLoop::IdentitySync,
                identity.is_running(),
                identity.last_sync_unix_seconds(),
                identity.interval(),
            ),
            status(
                SyncLoop::DashPaySync,
                dashpay.is_running(),
                dashpay.last_sync_unix_seconds(),
                dashpay.interval(),
            ),
            status(
                SyncLoop::DpnsSync,
                dpns.is_running(),
                dpns.last_sync_unix_seconds(),
                dpns.interval(),
            ),
            status(
                SyncLoop::PlatformAddressSync,
                addresses.is_running(),
                addresses.last_sync_unix_seconds(),
                addresses.interval(),
            ),
        ])
    }

    /// Paces the loops (§3.2 "Cadence"). A window becoming visible also runs
    /// a DashPay pass at once.
    pub fn set_platform_cadence(&self, cadence: PlatformCadence) -> Result<(), EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        let before = std::mem::replace(&mut *guard(&self.platform.cadence), cadence);
        self.apply_cadence(&manager);
        if cadence.window_visible && !before.window_visible {
            self.kick_dashpay_sync(&manager);
        }
        Ok(())
    }

    /// Runs a DashPay pass now, in the background (§3.2: Contacts or the
    /// bell opened, after an unlock, after a DashPay write). Nothing runs
    /// while SPV is not running; a pass already in flight is not repeated.
    pub fn dashpay_sync_soon(&self) -> Result<(), EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        self.kick_dashpay_sync(&manager);
        Ok(())
    }

    pub(super) fn kick_dashpay_sync(&self, manager: &Manager) {
        if !self.platform.is_running() {
            return;
        }
        let dashpay = manager.dashpay_sync_arc();
        let loops_on = Arc::clone(&self.platform.loops_on);
        self.rt.spawn(async move {
            // Not after a stop that came first.
            if loops_on.load(Ordering::Acquire) {
                dashpay.sync_now().await;
            }
        });
        // The pass's balances too (DP1-06). Protocol compatibility refreshes
        // when the loops start and on the 60 s timer, not on every pass.
        self.kick_balance_pass();
    }

    pub(super) fn apply_cadence(&self, manager: &Manager) {
        let c = *guard(&self.platform.cadence);
        manager.dashpay_sync().set_interval(if c.window_visible {
            DASHPAY_VISIBLE
        } else {
            DASHPAY_HIDDEN
        });
        manager.dpns_sync().set_interval(if c.contest_ending_soon {
            MARKETPLACE_ENDING
        } else {
            MARKETPLACE
        });
        manager
            .platform_address_sync()
            .set_interval(if c.window_visible {
                PLATFORM_ADDRESSES_VISIBLE
            } else {
                PLATFORM_ADDRESSES_HIDDEN
            });
    }
}
