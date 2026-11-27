//! Process-wide app state, managed by Tauri, and the handle tables that let
//! engine objects (a `TxDraft`, its `PreparedTx`s) outlive one command.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant};

use dw_engine::governance::Governance;
use dw_engine::masternodes::Masternodes;
use dw_engine::platform::DashPay;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, NetworkSession, PreparedTx, TxDraft, WalletId,
};
use dw_vault::VaultConfig;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{Mutex, Notify};

use crate::bridge::{EventMemory, TauriSink};
use crate::dto::ThemeName;
use crate::error::{AppCode, AppError, EngineCode};
use crate::instance::InstanceLock;
use crate::launch::LaunchOptions;
use crate::shell::Shell;

/// Debug builds only: milliseconds [`AppState::shutdown_engine`] waits
/// before it shuts the engine down, to stand in for a slow shutdown when
/// quitting and relaunching is checked by hand (issue 399).
#[cfg(debug_assertions)]
pub const DEBUG_QUIT_DELAY: &str = "DW_DEBUG_QUIT_DELAY_MS";

#[cfg(test)]
thread_local! {
    static ENGINE_START_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce() + Send>>> =
        const { std::cell::RefCell::new(None) };
}

/// The next `setup` runs this on the engine thread before `Engine::new`,
/// so a test can hold the build across `run_iteration`.
#[cfg(test)]
pub(crate) fn set_engine_start_hook(hook: impl FnOnce() + Send + 'static) {
    ENGINE_START_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
pub(crate) fn take_engine_start_hook() -> Option<Box<dyn FnOnce() + Send>> {
    ENGINE_START_HOOK.with(|slot| slot.borrow_mut().take())
}

#[cfg(test)]
thread_local! {
    static UPDATES_SEAM: std::cell::RefCell<Option<crate::updates::Updates>> =
        const { std::cell::RefCell::new(None) };
}

/// The next `AppState` on this thread uses `updates` instead of the app's.
#[cfg(test)]
pub(crate) fn set_updates(updates: crate::updates::Updates) {
    UPDATES_SEAM.with(|slot| *slot.borrow_mut() = Some(updates));
}

fn updates_for<R: Runtime>(app: &AppHandle<R>) -> crate::updates::Updates {
    #[cfg(test)]
    if let Some(updates) = UPDATES_SEAM.with(|slot| slot.borrow_mut().take()) {
        return updates;
    }
    crate::updates::Updates::for_app(app)
}

/// Builds the engine. Called again when a build failed and the user retries:
/// the splash's Retry must not repeat a consumed closure.
type EngineBuild = std::sync::Arc<dyn Fn() -> Result<Engine, AppError> + Send + Sync>;

pub struct AppState {
    pub launch: LaunchOptions,
    /// Taken at the top of `main`, for the cold-start measurement (U5).
    pub started: Instant,
    /// The engine. Empty until [`AppState::new`] or [`AppState::start_engine`]
    /// finishes building it; commands wait, except a status read, which
    /// reports starting.
    pub engine: EngineHandle,
    engine_build: EngineBuild,
    /// The data root (`--datadir` or the platform default), where
    /// `settings.json` lives (U-04 D2).
    pub data_root: PathBuf,
    /// Gives the main window an appearance (`None`: the system's).
    pub apply_theme: Box<dyn Fn(Option<ThemeName>) + Send + Sync>,
    /// Titles the main window.
    set_title: Box<dyn Fn(&str) + Send + Sync>,
    /// The network the window shows: the launch network until
    /// `switch_network` opens another. `open_network(null)` opens it, and
    /// `launch_info` reports it to a renderer that boots again.
    window_network: StdMutex<DashNetwork>,
    /// Broadcasts handed to the engine that have not returned
    /// ([`AppState::broadcasting`]): a network switch refuses meanwhile.
    /// Taken, once the session is found, by `draft_broadcast`,
    /// `broadcast_psbt`, `move_mixed_coins`, `consolidate_mixed_coins`,
    /// `resend_mixed_coins`, `resend_transaction`,
    /// `governance_submit_proposal` and `platform_send`. `shielded_send`
    /// takes none: a shielded spend in flight already refuses the close
    /// (`shielded_spend_unresolved`).
    broadcasts: Arc<AtomicUsize>,
    /// Tests: every broadcast guard taken.
    #[cfg(test)]
    pub(crate) broadcasts_taken: AtomicUsize,
    /// The network the window shows (D-row "one active session"). Session
    /// commands act on it; `open_network` and `close_network` change it.
    session: Mutex<Option<Arc<NetworkSession>>>,
    /// Recovery redirects that committed before their restart preference.
    pub(crate) recovery_choice_pending: Mutex<HashMap<DashNetwork, PathBuf>>,
    /// Ends the app (`AppHandle::exit`); `selftest_report` and `quit_app`
    /// call it.
    exit: Box<dyn Fn(i32) + Send + Sync>,
    /// Ends the app and starts it again (`AppHandle::request_restart`);
    /// `restart_app` calls it.
    restart: Box<dyn Fn() + Send + Sync>,
    /// The drafts and prepared transactions of the active session.
    pub sends: SendHandles,
    /// What an overflow cannot lose: the recent notices (`recentNotices`)
    /// and each network's last lock report (`lastLockReport`).
    pub memory: Arc<EventMemory>,
    /// Orders "require authentication for every payment" against the
    /// grants issued under it (`api::vault`, D11).
    pub payment_policy: Arc<PaymentPolicy>,
    /// Deep links and the native menus' state (U-01). The same `Arc` is
    /// Tauri-managed, for the callbacks that run before this state exists.
    pub shell: Arc<Shell>,
    /// Updates (PK-03): unavailable unless the build, the install and the
    /// user allow them.
    pub updates: crate::updates::Updates,
    /// The data root's instance lock, when `run` did not take it already
    /// (it does, before any window exists, and keeps it Tauri-managed; the
    /// mock-runtime builders do not). Declared after `engine`, so it is
    /// released after the engine is dropped.
    _instance: Option<InstanceLock>,
    /// Test seam: a payment `authorize` passes it between reading the
    /// setting and issuing the grant.
    #[cfg(test)]
    pub(crate) payment_seam: StdMutex<Option<Arc<Seam>>>,
    /// Test seam: a payment `authorize` passes it just before it takes
    /// [`PaymentPolicy::gate`].
    #[cfg(test)]
    pub(crate) payment_gate_seam: StdMutex<Option<Arc<Seam>>>,
    /// Test seam: `setPaymentAuthentication` passes it at this point,
    /// holding [`PaymentPolicy::gate`].
    #[cfg(test)]
    pub(crate) payment_set_pause: StdMutex<Option<(SetPause, Arc<Seam>)>>,
    /// Test seam: `authorizePlatformSend` passes it between issuing the
    /// grant (its id kept in `send_grant_issued`) and binding it.
    #[cfg(test)]
    pub(crate) send_bind_seam: StdMutex<Option<Arc<Seam>>>,
    #[cfg(test)]
    pub(crate) send_grant_issued: StdMutex<Option<String>>,
    /// Test seam: a payment `authorize` runs it on the vault after the
    /// payment check and before the grant is issued.
    #[cfg(test)]
    pub(crate) before_payment_issue: StdMutex<Option<BeforePaymentIssue>>,
    /// Test seam: the endpoints `switch_network` and `open_network(null)`
    /// open the window network with, instead of the launch's (tests reach
    /// no public DAPI or quorum service).
    #[cfg(test)]
    pub(crate) test_endpoints: StdMutex<Option<crate::api::network::SessionSettings>>,
    /// Test seam: `switch_network` and `finish_without_platform` run it
    /// after a failed open, before they reopen the network they left.
    #[cfg(test)]
    pub(crate) before_rollback: StdMutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Test seam: `finish_without_platform` runs it after the close, before
    /// it opens the network without Platform.
    #[cfg(test)]
    pub(crate) before_platform_off_open: StdMutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// Test seam: the networks the window started SPV on, in order.
    #[cfg(test)]
    pub(crate) spv_started: StdMutex<Vec<dw_engine::DashNetwork>>,
}

/// The single source of "require authentication for every payment" for
/// the grants issued under it (Sol r3 F1b). The setter holds `gate` across
/// advancing `generation` and then writing the setting; `authorize` holds it
/// across reading both, so no read sees the new value with the old
/// generation. Once the grant is issued, `authorize` compares `generation`
/// again: any write that may be readable by then has advanced it, so a
/// grant issued once "on" is readable was not judged under "off".
#[derive(Default)]
pub struct PaymentPolicy {
    pub gate: Mutex<()>,
    pub generation: AtomicU64,
}

/// A point the code under test passes and a test can watch: it reports
/// each arrival and, if it pauses, waits until the test resumes it. When
/// the test's [`SeamHandle`] is dropped (a failing test unwinds), a waiting
/// pass goes on, so a failure never leaves a thread parked.
///
/// A pause awaits; it never blocks its thread. The commands that pass a
/// seam run on Tauri's global runtime, which has one worker per core and is
/// shared by every test in the process. A blocking pause held a worker for
/// as long as the test kept it paused: three tests paused at once took all
/// three workers of a macOS runner, and the commands they waited for never
/// ran (main CI 38608334643, `authorize_waits_for_a_policy_write_in_progress`
/// timed out at its gate seam; with `TOKIO_WORKER_THREADS=1` it hung alone).
#[cfg(test)]
pub(crate) struct Seam {
    arrived: std::sync::mpsc::Sender<()>,
    resume: Option<Mutex<tokio::sync::mpsc::UnboundedReceiver<()>>>,
}

/// The test's side of a [`Seam`].
#[cfg(test)]
pub(crate) struct SeamHandle {
    arrived: std::sync::mpsc::Receiver<()>,
    resume: Option<tokio::sync::mpsc::UnboundedSender<()>>,
}

#[cfg(test)]
impl Seam {
    pub(crate) fn new(pauses: bool) -> (Arc<Seam>, SeamHandle) {
        let (arrived_tx, arrived) = std::sync::mpsc::channel();
        let (resume, resume_rx) = if pauses {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            (Some(tx), Some(Mutex::new(rx)))
        } else {
            (None, None)
        };
        (
            Arc::new(Seam {
                arrived: arrived_tx,
                resume: resume_rx,
            }),
            SeamHandle { arrived, resume },
        )
    }

    pub(crate) async fn pass(&self) {
        let _ = self.arrived.send(());
        if let Some(resume) = &self.resume {
            let _ = resume.lock().await.recv().await;
        }
    }

    /// Passes the seam in `slot`, if a test set one.
    pub(crate) async fn pass_slot(slot: &StdMutex<Option<Arc<Seam>>>) {
        let seam = slot.lock().unwrap().clone();
        if let Some(seam) = seam {
            seam.pass().await;
        }
    }
}

#[cfg(test)]
impl SeamHandle {
    /// Waits for the code under test to reach the seam; fails the test if
    /// it does not within two minutes (it returned early, say). The panic
    /// names the caller's line, so a failure says which seam it was.
    #[track_caller]
    pub(crate) fn arrived(&self) {
        if let Err(e) = self
            .arrived
            .recv_timeout(std::time::Duration::from_secs(120))
        {
            panic!("the code under test never reached its seam: {e:?}");
        }
    }

    pub(crate) fn resume(&self) {
        if let Some(resume) = &self.resume {
            let _ = resume.send(());
        }
    }
}

/// [`AppState::before_payment_issue`]'s hook.
#[cfg(test)]
pub(crate) type BeforePaymentIssue = Box<dyn FnOnce(&dw_vault::Vault) + Send>;

/// Where [`AppState::payment_set_pause`] stops the setter.
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SetPause {
    /// The generation is advanced; the setting is not written yet.
    AfterBump,
    /// The setting is written and readable.
    AfterWrite,
}

/// The engine, filled when its build finishes. Waiters sleep on `notify`;
/// a status read does not wait.
#[derive(Clone)]
pub struct EngineHandle {
    inner: Arc<EngineSlot>,
}

/// Single lifecycle state for engine construction, completion, retry, and exit.
enum BuildState {
    Idle,
    Starting,
    Failed(AppError),
    Ready,
    Abandoned(AppError),
}

enum ShutdownStep<'a> {
    Ready(&'a Engine),
    PendingAbandoned,
    Failed(AppError),
    AlreadyAbandoned,
}

struct EngineSlot {
    cell: OnceLock<Engine>,
    state: StdMutex<BuildState>,
    notify: Notify,
    /// Set when the window may paint, before the build runs.
    #[cfg(test)]
    window_ready_at: StdMutex<Option<Instant>>,
    /// Set when the build has installed the engine.
    #[cfg(test)]
    engine_ready_at: StdMutex<Option<Instant>>,
    /// Waiters blocked in [`EngineHandle::wait`], so a test can fail the
    /// build only after they are parked.
    #[cfg(test)]
    parked: AtomicUsize,
    /// Builds admitted to start.
    #[cfg(test)]
    builds_started: AtomicUsize,
    /// Build threads that have finished, published or not.
    #[cfg(test)]
    builds_settled: AtomicUsize,
}

impl EngineHandle {
    fn pending() -> Self {
        Self {
            inner: Arc::new(EngineSlot {
                cell: OnceLock::new(),
                state: StdMutex::new(BuildState::Idle),
                notify: Notify::new(),
                #[cfg(test)]
                window_ready_at: StdMutex::new(None),
                #[cfg(test)]
                engine_ready_at: StdMutex::new(None),
                #[cfg(test)]
                parked: AtomicUsize::new(0),
                #[cfg(test)]
                builds_started: AtomicUsize::new(0),
                #[cfg(test)]
                builds_settled: AtomicUsize::new(0),
            }),
        }
    }

    #[cfg(test)]
    fn mark_window(&self) {
        *self
            .inner
            .window_ready_at
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    #[cfg(test)]
    pub(crate) fn window_ready_at(&self) -> Option<Instant> {
        *self
            .inner
            .window_ready_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn engine_ready_at(&self) -> Option<Instant> {
        *self
            .inner
            .engine_ready_at
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The engine, once its build has finished.
    pub(crate) fn ready(&self) -> Option<&Engine> {
        self.inner.cell.get()
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.ready().is_some()
    }

    fn try_begin_initial(&self) -> bool {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, BuildState::Idle) {
            return false;
        }
        *state = BuildState::Starting;
        #[cfg(test)]
        self.inner.builds_started.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn try_begin_retry(&self) -> bool {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, BuildState::Failed(_)) {
            return false;
        }
        *state = BuildState::Starting;
        #[cfg(test)]
        self.inner.builds_started.fetch_add(1, Ordering::SeqCst);
        true
    }

    fn begin_shutdown(&self) -> ShutdownStep<'_> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        match &*state {
            BuildState::Ready => {
                let engine = self.inner.cell.get().expect("ready engine installed");
                ShutdownStep::Ready(engine)
            }
            BuildState::Idle | BuildState::Starting => {
                *state = BuildState::Abandoned(AppError::from(EngineError::Internal(
                    "quit while the engine was still starting".into(),
                )));
                drop(state);
                self.inner.notify.notify_waiters();
                ShutdownStep::PendingAbandoned
            }
            BuildState::Failed(err) => {
                let err = err.clone();
                *state = BuildState::Abandoned(err.clone());
                ShutdownStep::Failed(err)
            }
            BuildState::Abandoned(_) => ShutdownStep::AlreadyAbandoned,
        }
    }

    #[cfg(test)]
    pub(crate) fn builds_started(&self) -> usize {
        self.inner.builds_started.load(Ordering::SeqCst)
    }

    #[cfg(test)]
    pub(crate) fn builds_settled(&self) -> usize {
        self.inner.builds_settled.load(Ordering::SeqCst)
    }

    /// Pending, the recorded failure, or ready.
    pub(crate) fn build_view(&self) -> EngineBuildView {
        match &*self.inner.state.lock().unwrap_or_else(|e| e.into_inner()) {
            BuildState::Idle | BuildState::Starting => EngineBuildView::Pending,
            BuildState::Failed(err) | BuildState::Abandoned(err) => {
                EngineBuildView::Failed(err.clone())
            }
            BuildState::Ready => EngineBuildView::Ready,
        }
    }

    #[cfg(test)]
    pub(crate) fn parked_waiters(&self) -> usize {
        self.inner.parked.load(Ordering::SeqCst)
    }

    /// Publishes the build's result unless startup was abandoned. A result
    /// that arrives late is handed back, so the caller drops the engine (and
    /// shuts its runtime down) after the state lock is released.
    fn install(&self, result: Result<Engine, AppError>) -> Result<(), Result<Engine, AppError>> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*state, BuildState::Starting) {
            return Err(result);
        }
        match result {
            Ok(engine) => {
                #[cfg(test)]
                {
                    *self
                        .inner
                        .engine_ready_at
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                }
                let _ = self.inner.cell.set(engine);
                *state = BuildState::Ready;
            }
            Err(e) => {
                *state = BuildState::Failed(e);
            }
        }
        drop(state);
        self.inner.notify.notify_waiters();
        Ok(())
    }

    /// Waits until the build installs the engine or records its failure.
    /// Already finished: ready on the first poll, no runtime required.
    pub(crate) async fn wait(&self) -> Result<&Engine, AppError> {
        if let Some(engine) = self.ready() {
            return Ok(engine);
        }
        loop {
            let notified = self.inner.notify.notified();
            match self.build_view() {
                EngineBuildView::Ready => {
                    if let Some(engine) = self.ready() {
                        return Ok(engine);
                    }
                }
                EngineBuildView::Failed(err) => return Err(err),
                EngineBuildView::Pending => {}
            }
            #[cfg(test)]
            self.inner.parked.fetch_add(1, Ordering::SeqCst);
            notified.await;
            #[cfg(test)]
            self.inner.parked.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

/// What [`EngineHandle::build_view`] saw.
pub(crate) enum EngineBuildView {
    Pending,
    Failed(AppError),
    Ready,
}

/// If the build thread panics, waiters hear it instead of sleeping forever.
/// Constructed before the test hook runs, so a panic there still records.
struct FinishEngine {
    handle: EngineHandle,
    started: Instant,
    done: bool,
}

impl Drop for FinishEngine {
    fn drop(&mut self) {
        if !self.done
            && self
                .handle
                .install(Err(AppError::from(EngineError::Internal(
                    "the engine stopped while starting".into(),
                ))))
                .is_ok()
        {
            crate::startup_phase(self.started, "engine-failed");
        }
    }
}

impl AppState {
    /// `vault` is the production `VaultConfig::default()` except in tests
    /// (an in-memory OS store, cheap KDF parameters). Builds the engine
    /// before returning. The window uses [`Self::pending`] and
    /// [`Self::start_engine`] so the event loop can paint first.
    #[cfg(test)]
    pub fn new<R: Runtime>(
        app: &AppHandle<R>,
        launch: LaunchOptions,
        started: Instant,
        vault: VaultConfig,
        default_data_dir: impl FnOnce() -> Result<PathBuf, AppError>,
    ) -> Result<Self, AppError> {
        let state = Self::pending(app, launch, started, vault, default_data_dir)?;
        state.start_engine_blocking()?;
        Ok(state)
    }

    /// The window's state with the engine not built yet. [`Self::start_engine`]
    /// builds it off the event-loop thread. The window-ready mark is this
    /// return: nothing here opens a wallet or constructs the runtime.
    pub(crate) fn pending<R: Runtime>(
        app: &AppHandle<R>,
        mut launch: LaunchOptions,
        started: Instant,
        vault: VaultConfig,
        default_data_dir: impl FnOnce() -> Result<PathBuf, AppError>,
    ) -> Result<Self, AppError> {
        let memory = Arc::new(EventMemory::default());
        let data_root = match &launch.data_dir {
            Some(dir) => dir.clone(),
            None => default_data_dir()?,
        };
        // The network onboarding or Settings chose, unless `--network`
        // named one (release 0.1).
        launch.apply_network_choice(&data_root);
        // The lock before the engine: a second process on this root stops
        // here, before it opens any wallet database.
        let instance = match app.try_state::<InstanceLock>() {
            Some(_) => None,
            None => {
                dw_fs::create_private_dir(&data_root).map_err(|e| {
                    AppError::app(
                        AppCode::DataDir,
                        format!("cannot create {}: {e}", data_root.display()),
                    )
                })?;
                Some(InstanceLock::acquire(&data_root, Duration::ZERO)?)
            }
        };
        let config = EngineConfig {
            data_root: data_root.clone(),
            worker_threads: None,
            vault,
        };
        let sink_app = app.clone();
        let sink_memory = Arc::clone(&memory);
        let engine_build: EngineBuild = std::sync::Arc::new(move || {
            let sink = Arc::new(TauriSink::new(sink_app.clone(), Arc::clone(&sink_memory))?);
            Engine::new(config.clone(), sink).map_err(AppError::from)
        });
        let handle = app.clone();
        let restarting = app.clone();
        let window = app.clone();
        let titled = app.clone();
        let window_network = StdMutex::new(launch.network.clone());
        let shell = crate::shell::shell(app)
            .unwrap_or_else(|| Arc::new(Shell::new(launch.selftest_link_trace.clone())));
        let engine = EngineHandle::pending();
        // Before any engine work: the caller shows the window, then starts
        // the build. The mark is this instant, not the paint.
        #[cfg(test)]
        engine.mark_window();
        Ok(AppState {
            launch,
            started,
            engine,
            engine_build,
            data_root,
            apply_theme: Box::new(move |theme| {
                if let Some(w) = window.get_webview_window("main") {
                    let _ = w.set_theme(theme.map(Into::into));
                }
            }),
            set_title: Box::new(move |title| {
                if let Some(w) = titled.get_webview_window("main") {
                    let _ = w.set_title(title);
                }
            }),
            window_network,
            broadcasts: Arc::default(),
            #[cfg(test)]
            broadcasts_taken: AtomicUsize::new(0),
            session: Mutex::new(None),
            recovery_choice_pending: Mutex::new(HashMap::new()),
            exit: Box::new(move |code| handle.exit(code)),
            restart: Box::new(move || restarting.request_restart()),
            sends: SendHandles::default(),
            memory,
            payment_policy: Arc::default(),
            shell,
            updates: updates_for(app),
            _instance: instance,
            #[cfg(test)]
            payment_seam: StdMutex::default(),
            #[cfg(test)]
            payment_gate_seam: StdMutex::default(),
            #[cfg(test)]
            payment_set_pause: StdMutex::default(),
            #[cfg(test)]
            send_bind_seam: StdMutex::default(),
            #[cfg(test)]
            send_grant_issued: StdMutex::default(),
            #[cfg(test)]
            before_payment_issue: StdMutex::default(),
            #[cfg(test)]
            test_endpoints: StdMutex::default(),
            #[cfg(test)]
            before_rollback: StdMutex::default(),
            #[cfg(test)]
            before_platform_off_open: StdMutex::default(),
            #[cfg(test)]
            spv_started: StdMutex::default(),
        })
    }

    /// Builds the engine on this thread. Tests use it; the window does not.
    #[cfg(test)]
    fn start_engine_blocking(&self) -> Result<(), AppError> {
        if !self.engine.try_begin_initial() {
            return Err(AppError::from(EngineError::Internal(
                "the engine was already started".into(),
            )));
        }
        let result = (self.engine_build)();
        let out = match &result {
            Ok(_) => Ok(()),
            Err(e) => Err(e.clone()),
        };
        let _ = self.engine.install(result);
        out
    }

    /// Builds the engine on `dw-engine-start` and returns before it finishes.
    /// The event loop's Ready hook calls this after the window is shown, so
    /// the window can paint while the runtime, the rates client and the
    /// first touch of the engine code run here.
    pub(crate) fn start_engine(&self) {
        self.spawn_engine(false, || {});
    }

    /// [`Self::start_engine`], running `before` on the build thread ahead of
    /// `Engine::new`. The call returns while `before` is still blocked.
    #[cfg(test)]
    pub(crate) fn start_engine_after(&self, before: impl FnOnce() + Send + 'static) {
        self.spawn_engine(false, before);
    }

    /// Starts another build when the last one failed, so Retry is not a dead end.
    pub(crate) fn retry_engine_if_failed(&self) {
        self.spawn_engine(true, || {});
    }

    fn spawn_engine(&self, retry_only: bool, before: impl FnOnce() + Send + 'static) {
        let admitted = if retry_only {
            self.engine.try_begin_retry()
        } else {
            self.engine.try_begin_initial()
        };
        if !admitted {
            return;
        }
        let build = Arc::clone(&self.engine_build);
        let handle = self.engine.clone();
        let started = self.started;
        let spawned = std::thread::Builder::new()
            .name("dw-engine-start".into())
            .spawn(move || {
                // Before `before`: a panic in the test hook has to record
                // the failure, or waiters stay parked.
                let mut finish = FinishEngine {
                    handle: handle.clone(),
                    started,
                    done: false,
                };
                before();
                let build_started = Instant::now();
                let result = build();
                // Duration, not a timestamp from `run`. `engine-ready` is the
                // timestamp a timeline should use.
                eprintln!(
                    "dw-app: engine-build {}",
                    build_started.elapsed().as_millis()
                );
                tracing::info!(
                    elapsed_ms = build_started.elapsed().as_millis() as u64,
                    "engine build"
                );
                if let Err(ref e) = result {
                    tracing::warn!(error = %e, "the engine did not start");
                }
                let ok = result.is_ok();
                finish.done = true;
                if handle.install(result).is_ok() {
                    crate::startup_phase(
                        started,
                        if ok { "engine-ready" } else { "engine-failed" },
                    );
                }
                #[cfg(test)]
                handle.inner.builds_settled.fetch_add(1, Ordering::SeqCst);
            });
        if let Err(e) = spawned
            && self
                .engine
                .install(Err(AppError::from(EngineError::Internal(format!(
                    "could not start the engine thread: {e}"
                )))))
                .is_ok()
        {
            crate::startup_phase(self.started, "engine-failed");
        }
    }

    /// Shuts a ready engine down. If startup has not produced an engine, marks
    /// the slot abandoned so late completion drops the result without releasing
    /// queued commands, and returns without waiting on the build.
    pub(crate) async fn shutdown_engine(&self) -> Result<(), AppError> {
        #[cfg(debug_assertions)]
        if let Some(ms) = std::env::var(DEBUG_QUIT_DELAY)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        }
        match self.engine.begin_shutdown() {
            ShutdownStep::Ready(engine) => engine.shutdown().await.map_err(AppError::from),
            ShutdownStep::PendingAbandoned => {
                eprintln!("dash-wallet: quit while the engine was still starting");
                Ok(())
            }
            ShutdownStep::Failed(e) => {
                eprintln!("dash-wallet: engine did not start: {e}");
                Ok(())
            }
            ShutdownStep::AlreadyAbandoned => Ok(()),
        }
    }

    #[cfg(test)]
    pub(crate) async fn pause_payment_set(&self, at: SetPause) {
        let pause = self.payment_set_pause.lock().unwrap().clone();
        if let Some((stage, seam)) = pause
            && stage == at
        {
            seam.pass().await;
        }
    }

    /// The active session, unless an open or a close holds it right now;
    /// never waits.
    pub(crate) fn session_now(&self) -> Option<Arc<NetworkSession>> {
        self.session.try_lock().ok().and_then(|slot| slot.clone())
    }

    /// The active session; `network_not_open` before `open_network`.
    pub async fn session(&self) -> Result<Arc<NetworkSession>, AppError> {
        Ok(self.session_and_epoch().await?.0)
    }

    /// The active session with the handle epoch it belongs to, read
    /// together: `open_network` and `close_network` clear the handles while
    /// they hold the session, so a handle made from this session can be
    /// stored only under this epoch.
    pub async fn session_and_epoch(&self) -> Result<(Arc<NetworkSession>, u64), AppError> {
        // An early command waits out the start instead of failing it.
        self.engine.wait().await?;
        let slot = self.session.lock().await;
        let session = slot
            .clone()
            .ok_or_else(|| AppError::new(EngineCode::NetworkNotOpen, "call open_network first"))?;
        Ok((session, self.sends.epoch()))
    }

    /// The slot `open_network` and `close_network` hold while they change
    /// the active session, so two of them never interleave.
    pub(crate) async fn session_slot(
        &self,
    ) -> tokio::sync::MutexGuard<'_, Option<Arc<NetworkSession>>> {
        self.session.lock().await
    }

    /// The DashPay facade of one wallet of the active session.
    pub async fn dashpay(&self, wallet_id: &str) -> Result<Arc<DashPay>, AppError> {
        let id = parse_wallet_id(wallet_id)?;
        Ok(self.session().await?.dashpay(id))
    }

    /// The governance facade of the active session.
    pub async fn governance(&self) -> Result<Governance, AppError> {
        Ok(self.session().await?.governance())
    }

    /// The masternodes facade of the active session.
    pub async fn masternodes(&self) -> Result<Masternodes, AppError> {
        Ok(self.session().await?.masternodes())
    }

    /// The `--selftest` report path; the self-test commands refuse to run without it.
    pub fn selftest_path(&self) -> Result<&Path, AppError> {
        self.launch.selftest.as_deref().ok_or_else(|| {
            AppError::app(
                AppCode::NotSelftest,
                "the app was not launched with --selftest",
            )
        })
    }

    /// Developer-only commands (the trusted-quorums toggle, m4 contract)
    /// refuse in release builds.
    pub fn developer_build(&self) -> Result<(), AppError> {
        if cfg!(debug_assertions) {
            Ok(())
        } else {
            Err(AppError::app(
                AppCode::NotDeveloperBuild,
                "a developer-only command in a release build",
            ))
        }
    }

    pub fn exit(&self, code: i32) {
        (self.exit)(code)
    }

    /// Ends the app through its normal exit path and starts it again.
    pub fn restart(&self) {
        (self.restart)()
    }

    /// The network the window shows ([`AppState::window_network`]).
    pub fn window_network(&self) -> DashNetwork {
        self.window_network
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Makes `network` the window's, and titles the window for it.
    pub(crate) fn set_window_network(&self, network: DashNetwork) {
        (self.set_title)(&crate::window_title(&network));
        *self
            .window_network
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = network;
    }

    /// Counts a broadcast in flight until the guard drops.
    pub(crate) fn broadcasting(&self) -> BroadcastGuard<'_> {
        #[cfg(test)]
        self.broadcasts_taken.fetch_add(1, Ordering::SeqCst);
        self.broadcasts.fetch_add(1, Ordering::SeqCst);
        BroadcastGuard(&self.broadcasts)
    }

    /// How many broadcasts were handed to the engine and have not
    /// returned; read by a network switch's close.
    pub(crate) fn broadcasts(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.broadcasts)
    }

    /// Drops every handle: they belong to the session that is closing.
    pub(crate) fn clear_handles(&self) {
        self.sends.clear();
    }
}

/// A broadcast in flight ([`AppState::broadcasting`]).
pub(crate) struct BroadcastGuard<'a>(&'a AtomicUsize);

impl Drop for BroadcastGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Wallet ids cross the IPC boundary as lower-case hex.
pub fn parse_wallet_id(s: &str) -> Result<WalletId, AppError> {
    s.parse().map_err(AppError::from)
}

/// Engine objects the renderer refers to by a number, because they hold
/// state between calls (a draft's recipients, a signed transaction) and
/// cannot cross the IPC boundary themselves: `TxDraft`s, and `PreparedTx`s
/// with the draft that made each.
///
/// Ids are never reused within a process. Each kind is capped: a renderer
/// that leaks handles gets `app.too_many_handles` instead of growing the
/// process. Closing the session drops them all and starts a new epoch. One
/// lock covers both kinds, so a prepared transaction is stored only while
/// its draft is open, and a draft only in the epoch of its session; one
/// that arrives late is dropped (a pending prepared transaction then
/// releases its inputs).
///
/// A prepare reserves its prepared handle before it redeems its grant
/// ([`SendHandles::reserve_prepared`]); reservations count against the cap,
/// so a full table refuses a prepare before its grant is spent.
#[derive(Default)]
pub struct SendHandles {
    inner: StdMutex<Sends>,
    /// Test seam: every `reserve_prepared` waits here after its attempt,
    /// so concurrent prepares all attempt before any of them goes on.
    #[cfg(test)]
    pub(crate) reserve_barrier: StdMutex<Option<Arc<std::sync::Barrier>>>,
}

#[derive(Default)]
struct Sends {
    epoch: u64,
    /// The last id given out.
    last: u64,
    drafts: HashMap<u64, Arc<TxDraft>>,
    /// Each with its draft, and whether a broadcast of it was attempted.
    prepared: HashMap<u64, Prepared>,
    /// Prepares in flight: reservation id → draft.
    reserved: HashMap<u64, u64>,
    /// The last reservation id given out.
    last_reservation: u64,
}

struct Prepared {
    draft: u64,
    tx: Arc<PreparedTx>,
    broadcast: bool,
}

/// `app.too_many_handles` when `open` handles of a kind are the cap.
fn full(open: usize) -> Result<(), AppError> {
    if open >= SendHandles::MAX {
        return Err(AppError::new(
            AppCode::TooManyHandles,
            format!(
                "more than {} open handles; close some first",
                SendHandles::MAX
            ),
        ));
    }
    Ok(())
}

impl Sends {
    fn next_id(&mut self, open: usize) -> Result<u64, AppError> {
        full(open)?;
        self.last += 1;
        Ok(self.last)
    }
}

fn unknown(id: u64) -> AppError {
    AppError::new(AppCode::UnknownHandle, format!("no handle {id}"))
}

impl SendHandles {
    /// Live handles per kind.
    pub const MAX: usize = 64;

    fn lock(&self) -> std::sync::MutexGuard<'_, Sends> {
        // A panic while holding the lock leaves the maps themselves consistent.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn epoch(&self) -> u64 {
        self.lock().epoch
    }

    /// Stores a draft made from the session of `epoch`.
    pub fn insert_draft(&self, epoch: u64, draft: Arc<TxDraft>) -> Result<u64, AppError> {
        let mut s = self.lock();
        if s.epoch != epoch {
            return Err(AppError::new(
                EngineCode::NetworkNotOpen,
                "the network closed while the draft was made",
            ));
        }
        let open = s.drafts.len();
        let id = s.next_id(open)?;
        s.drafts.insert(id, draft);
        Ok(id)
    }

    pub fn draft(&self, id: u64) -> Result<Arc<TxDraft>, AppError> {
        self.lock()
            .drafts
            .get(&id)
            .cloned()
            .ok_or_else(|| unknown(id))
    }

    /// Drops a draft, its prepared transactions and the reservations of its
    /// prepares in flight; `false` when it was not open. Closing does not
    /// stop a broadcast already handed to the engine.
    pub fn close_draft(&self, id: u64) -> bool {
        let mut s = self.lock();
        s.prepared.retain(|_, p| p.draft != id);
        s.reserved.retain(|_, d| *d != id);
        s.drafts.remove(&id).is_some()
    }

    /// Reserves room for one prepared transaction of `draft`, before the
    /// prepare redeems its grant: `app.too_many_handles` when the open and
    /// reserved handles are the cap. The reservation is given back when the
    /// slot is dropped unfilled (any failure), its draft is closed, or the
    /// session ends.
    pub fn reserve_prepared(&self, draft: u64) -> Result<PreparedSlot<'_>, AppError> {
        let reserved = {
            let mut s = self.lock();
            if !s.drafts.contains_key(&draft) {
                Err(unknown(draft))
            } else {
                full(s.prepared.len() + s.reserved.len()).map(|()| {
                    s.last_reservation += 1;
                    let id = s.last_reservation;
                    s.reserved.insert(id, draft);
                    id
                })
            }
        };
        #[cfg(test)]
        {
            // Not under the mutex: the other prepare must reach the barrier.
            let barrier = self.reserve_barrier.lock().unwrap().clone();
            if let Some(barrier) = barrier {
                barrier.wait();
            }
        }
        Ok(PreparedSlot {
            handles: self,
            id: reserved?,
            draft,
        })
    }

    /// Stores a transaction under its reservation, if that still holds (the
    /// draft open, the session the same).
    fn fill(&self, slot: u64, draft: u64, tx: Arc<PreparedTx>) -> Result<u64, AppError> {
        let mut s = self.lock();
        if s.reserved.remove(&slot).is_none() {
            return Err(AppError::new(
                AppCode::UnknownHandle,
                format!("draft {draft} was closed while it was prepared"),
            ));
        }
        // The reservation held the room: no cap check.
        s.last += 1;
        let id = s.last;
        s.prepared.insert(
            id,
            Prepared {
                draft,
                tx,
                broadcast: false,
            },
        );
        Ok(id)
    }

    /// A prepared transaction with the draft that made it.
    pub fn prepared(&self, id: u64) -> Result<(u64, Arc<PreparedTx>), AppError> {
        let s = self.lock();
        let p = s.prepared.get(&id).ok_or_else(|| unknown(id))?;
        Ok((p.draft, Arc::clone(&p.tx)))
    }

    /// Marks a broadcast of `id` as attempted; `true` for the first one.
    pub fn start_broadcast(&self, id: u64) -> bool {
        self.lock()
            .prepared
            .get_mut(&id)
            .is_some_and(|p| !std::mem::replace(&mut p.broadcast, true))
    }

    pub fn remove_prepared(&self, id: u64) {
        self.lock().prepared.remove(&id);
    }

    fn clear(&self) {
        let mut s = self.lock();
        s.epoch += 1;
        s.drafts.clear();
        s.prepared.clear();
        s.reserved.clear();
    }
}

/// Room for one prepared transaction ([`SendHandles::reserve_prepared`]).
/// Dropped unfilled, it gives the room back.
pub struct PreparedSlot<'a> {
    handles: &'a SendHandles,
    id: u64,
    draft: u64,
}

impl PreparedSlot<'_> {
    /// Stores the prepared transaction; fails with `app.unknown_handle` when
    /// the draft was closed or the network switched meanwhile (`tx` is then
    /// dropped and releases its inputs).
    pub fn fill(self, tx: Arc<PreparedTx>) -> Result<u64, AppError> {
        self.handles.fill(self.id, self.draft, tx)
    }
}

impl Drop for PreparedSlot<'_> {
    fn drop(&mut self) {
        // A no-op after `fill`, `close_draft` or a session switch.
        self.handles.lock().reserved.remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build that finishes after quit is not published; its engine comes
    /// back to the caller, and the state lock is free while the caller still
    /// holds it.
    #[test]
    fn a_build_finishing_after_quit_is_handed_back_unlocked() {
        let app = tauri::test::mock_app();
        let dir = dw_testutil::private_tempdir();
        let launch = LaunchOptions {
            data_dir: Some(dir.path().join("data")),
            ..LaunchOptions::default()
        };
        let state = AppState::pending(
            app.handle(),
            launch,
            Instant::now(),
            Default::default(),
            || unreachable!("--datadir given"),
        )
        .unwrap();
        assert!(state.engine.try_begin_initial());
        assert!(tauri::async_runtime::block_on(state.shutdown_engine()).is_ok());
        let engine = (state.engine_build)().expect("the engine builds");
        let late = state
            .engine
            .install(Ok(engine))
            .expect_err("a build after quit was published");
        assert!(matches!(
            state.engine.build_view(),
            EngineBuildView::Failed(_)
        ));
        assert!(late.is_ok(), "the late engine was not handed back");
        drop(late);
        assert!(state.engine.ready().is_none());
    }

    #[test]
    fn handles_are_capped_and_never_reused() {
        // Exercised with real drafts over IPC (`ipc_tests`); here only the
        // id and cap rules, which need no engine.
        let mut s = Sends::default();
        let ids: Vec<u64> = (0..SendHandles::MAX)
            .map(|n| s.next_id(n).unwrap())
            .collect();
        assert_eq!(ids, (1..=SendHandles::MAX as u64).collect::<Vec<_>>());
        assert_eq!(
            s.next_id(SendHandles::MAX).unwrap_err().code_str(),
            "app.too_many_handles"
        );
        let t = SendHandles::default();
        assert_eq!(t.draft(1).unwrap_err().code_str(), "app.unknown_handle");
        assert_eq!(t.prepared(1).unwrap_err().code_str(), "app.unknown_handle");
        assert!(!t.close_draft(1));
        let epoch = t.epoch();
        t.clear();
        assert_eq!(t.epoch(), epoch + 1);
    }

    /// A paused seam leaves its worker to other tasks: on a runtime with
    /// one worker, a task spawned while another is paused still runs. A
    /// blocking pause hangs here (main CI 38608334643, see [`Seam`]).
    #[test]
    fn a_paused_seam_does_not_hold_its_worker() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        let (seam, handle) = Seam::new(true);
        let paused = runtime.spawn(async move { seam.pass().await });
        handle.arrived();
        let (ran_tx, ran) = std::sync::mpsc::channel();
        runtime.spawn(async move { ran_tx.send(()) });
        // The spawned task either runs or never will; the bound only turns
        // a hang into a failure.
        ran.recv_timeout(std::time::Duration::from_secs(120))
            .expect("the paused seam held the only worker");
        assert!(!paused.is_finished());
        handle.resume();
        runtime.block_on(paused).unwrap();
    }
}
