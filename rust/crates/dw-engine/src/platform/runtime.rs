//! The session's Platform runtime (DASHPAY §3.1 `PlatformRuntime`; E0-05):
//! the supervisor slot, each wallet's bring-up status, the vault's lock
//! state as the bring-up sees it, and the Platform sync loops with their
//! cadence. The bring-up itself is in `bringup.rs`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::Duration;

use dw_vault::{LockState, Vault};
use tokio::sync::{mpsc, watch};
use tokio::task::{AbortHandle, JoinHandle};

use super::startup::{StartupStatus, SyncLoop, SyncLoopStatus};
use super::startup_status::{DashPayStartup, PlatformCadence, SpvState};
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};

/// `dashpay_sync` while a window is visible, and while hidden (§3.2).
const DASHPAY_VISIBLE: Duration = Duration::from_secs(15);
const DASHPAY_HIDDEN: Duration = Duration::from_secs(60);
/// The contest watch (`dpns_sync`), and in a contest's last hour.
const CONTEST_WATCH: Duration = Duration::from_secs(600);
const CONTEST_WATCH_ENDING: Duration = Duration::from_secs(60);

/// How long a stop or close waits for the bring-up's blocking vault work (a
/// keyring read) after cancelling it. A read that takes longer is stuck, for
/// example behind a keyring prompt nobody answers: the stop or close then
/// reports it instead of hanging.
pub(super) const KEY_WORK_JOIN: Duration = Duration::from_secs(5);

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
    pub(super) supervisor: Mutex<Option<Supervisor>>,
    /// Orders `start_spv`, `stop_spv` and `reset_chain_data`.
    pub(crate) lifecycle: tokio::sync::Mutex<()>,
    startup: RwLock<HashMap<WalletId, DashPayStartup>>,
    cadence: Mutex<PlatformCadence>,
    pub(super) lock: watch::Sender<LockView>,
    /// Each wallet's bring-up or unlock task in flight, ended when the
    /// wallet is removed or closed.
    pub(super) tasks: Mutex<HashMap<WalletId, AbortHandle>>,
    /// The loops were started and not stopped since; read by passes that
    /// were scheduled earlier.
    pub(super) loops_on: Arc<AtomicBool>,
    pub(super) key_work: Arc<KeyWork>,
    /// Wallets a restore has begun storing and has not committed or rolled
    /// back (review r2 M4).
    restoring: Mutex<HashMap<WalletId, RestoreMark>>,
    /// The clock that orders signals' events against bring-up admissions
    /// (review r4 M4-R4).
    events: AtomicU64,
    /// The identity read model and the names passes (DP1-05).
    pub(crate) recovery: super::recovery::Recovery,
    /// Tests: holds the next bring-up between building its keys and
    /// starting the library call (`runtime_tests.rs`).
    #[cfg(test)]
    pub(super) pause_after_keys: Mutex<Option<Arc<TestPause>>>,
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
    pub(crate) fn new(lock: LockState, enabled: bool) -> Self {
        Self {
            enabled,
            supervisor: Mutex::new(None),
            lifecycle: tokio::sync::Mutex::new(()),
            startup: RwLock::new(HashMap::new()),
            cadence: Mutex::new(PlatformCadence::default()),
            lock: watch::Sender::new(LockView {
                state: lock,
                locks: 0,
            }),
            tasks: Mutex::new(HashMap::new()),
            loops_on: Arc::new(AtomicBool::new(false)),
            key_work: Arc::default(),
            restoring: Mutex::new(HashMap::new()),
            events: AtomicU64::new(0),
            recovery: Default::default(),
            #[cfg(test)]
            pause_after_keys: Mutex::new(None),
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
        if !self.enabled {
            return;
        }
        if let Some(s) = &*guard(&self.supervisor) {
            let _ = s.signals.send((at, signal));
        }
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
        for entry in self.write_startup().values_mut() {
            if entry.startup == StartupStatus::Starting {
                entry.startup = StartupStatus::NotRun;
            }
        }
    }

    /// Waits for the bring-up's blocking key work, at most
    /// [`KEY_WORK_JOIN`]; `false` when some still runs.
    pub(crate) async fn key_work_idle(&self) -> bool {
        self.key_work.wait_idle().await
    }

    pub(super) fn write_startup(
        &self,
    ) -> std::sync::RwLockWriteGuard<'_, HashMap<WalletId, DashPayStartup>> {
        self.startup.write().unwrap_or_else(|p| p.into_inner())
    }

    pub(super) fn startup_of(&self, id: &WalletId) -> Option<DashPayStartup> {
        self.startup
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    pub(super) fn record(&self, id: WalletId, startup: DashPayStartup) {
        self.write_startup().insert(id, startup);
    }

    pub(super) fn set_status(&self, id: WalletId, status: StartupStatus) {
        self.write_startup()
            .entry(id)
            .or_insert_with(|| DashPayStartup::new(status, false))
            .startup = status;
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
        self.recovery.forget(id);
    }

    pub(super) fn untrack(&self, id: WalletId, task: tokio::task::Id) {
        let mut tasks = guard(&self.tasks);
        if tasks.get(&id).is_some_and(|t| t.id() == task) {
            tasks.remove(&id);
        }
    }
}

impl NetworkSession {
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
    }

    pub(super) fn apply_cadence(&self, manager: &Manager) {
        let c = *guard(&self.platform.cadence);
        manager.dashpay_sync().set_interval(if c.window_visible {
            DASHPAY_VISIBLE
        } else {
            DASHPAY_HIDDEN
        });
        manager.dpns_sync().set_interval(if c.contest_ending_soon {
            CONTEST_WATCH_ENDING
        } else {
            CONTEST_WATCH
        });
    }
}
