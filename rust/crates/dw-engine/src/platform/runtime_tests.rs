//! Bring-up internals: the status mapping, the local markers and the
//! cadence. The end-to-end behaviour is in `tests/e0_05_bringup.rs`.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use dw_vault::{KdfParams, KdfPolicy, LockState, MemoryOsStore, VaultConfig};
use platform_wallet::changeset::PendingContactCryptoOp;
use platform_wallet::manager::startup::WalletStartupStatus;
use zeroize::Zeroizing;

use super::bringup::{CREATED_HERE_KEY, NO_IDENTITY_KEY};
use super::runtime::{TestPause, guard, prompt_free};
use super::{PlatformCadence, SpvState, StartupStatus};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};

/// Held for the whole run by the tests that start SPV and wait on it or time
/// it: a bring-up competing with the other tests' bring-ups for CPU and disk
/// misses their wall-clock bounds on a loaded machine. A failing test poisons
/// the lock; the next one takes it anyway.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
}

const ABANDON_12: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn engine(dir: &std::path::Path, worker_threads: usize) -> Engine {
    engine_with(dir, worker_threads, Arc::new(NoEvents))
}

fn engine_with(dir: &std::path::Path, worker_threads: usize, sink: Arc<dyn EventSink>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: dir.join("data"),
            worker_threads: Some(worker_threads),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        sink,
    )
    .unwrap()
}

fn session(dir: &std::path::Path) -> (Engine, Arc<NetworkSession>) {
    let engine = engine(dir, 2);
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    (engine, s)
}

fn marker(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId, key: &'static str) -> bool {
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(s.appdb_op(move |db| db.setting(&scope, key)))
        .unwrap()
        .is_some()
}

#[test]
fn startup_status_maps_the_library_and_serializes_as_the_facade() {
    let cases = [
        (WalletStartupStatus::Ready, StartupStatus::Ready, "ready"),
        (
            WalletStartupStatus::NoIdentity,
            StartupStatus::NoIdentity,
            "no_identity",
        ),
        (
            WalletStartupStatus::PartialNoIdentity,
            StartupStatus::PartialNoIdentity,
            "partial_no_identity",
        ),
        (
            WalletStartupStatus::DiscoveryFailed,
            StartupStatus::DiscoveryFailed,
            "discovery_failed",
        ),
        (
            WalletStartupStatus::PartialAccountsPending,
            StartupStatus::PartialAccountsPending,
            "partial_accounts_pending",
        ),
        (
            WalletStartupStatus::SeedBindingUnverified,
            StartupStatus::SeedBindingUnverified,
            "seed_binding_unverified",
        ),
        (
            WalletStartupStatus::IdentityScanIncomplete,
            StartupStatus::IdentityScanIncomplete,
            "identity_scan_incomplete",
        ),
    ];
    for (library, ours, json) in cases {
        assert_eq!(StartupStatus::from(library), ours);
        assert_eq!(serde_json::to_value(ours).unwrap(), json);
        assert_eq!(ours.as_str(), json);
    }
    for (ours, json) in [
        (StartupStatus::NotRun, "not_run"),
        (StartupStatus::Starting, "starting"),
        (StartupStatus::IdentityUnsettled, "identity_unsettled"),
    ] {
        assert_eq!(serde_json::to_value(ours).unwrap(), json);
        assert_eq!(ours.as_str(), json);
        assert!(!ours.is_incomplete());
    }
    assert!(!StartupStatus::Ready.is_incomplete());
    assert!(!StartupStatus::NoIdentity.is_incomplete());
    assert!(StartupStatus::PartialNoIdentity.is_incomplete());
    assert!(StartupStatus::SeedBindingUnverified.is_incomplete());
}

#[test]
fn only_the_states_with_a_prompt_free_key_build_providers() {
    for (state, free) in [
        (LockState::NoVault, false),
        (LockState::NoKeys, true),
        (LockState::Unencrypted, true),
        (LockState::Locked, false),
        (LockState::UnlockedMixingOnly, false),
        (LockState::Unlocked, true),
    ] {
        assert_eq!(prompt_free(state), free, "{state:?}");
    }
}

/// A wallet created here is marked; an imported one is not; neither marker
/// leaves the installation.
#[test]
fn create_marks_the_wallet_created_here() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let created = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    let imported = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    assert!(marker(&engine, &s, created, CREATED_HERE_KEY));
    assert!(!marker(&engine, &s, imported, CREATED_HERE_KEY));
    let rows = engine
        .block_on(s.appdb_op(move |db| db.export_wallet_rows(&created.to_string())))
        .unwrap();
    let exported = format!("{rows:?}");
    assert!(!exported.contains(CREATED_HERE_KEY), "{exported}");
    engine.block_on(engine.shutdown()).unwrap();
}

/// Platform proved this seed owns no identity and none is on file: the
/// bring-up is skipped and SPV starts at once.
#[test]
fn a_proven_absence_skips_the_bring_up() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    // Imported: without the marker it would get the 20 s budget.
    let id = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    set_no_identity(&engine, &s, id, Duration::from_secs(24 * 3600));
    let called = Instant::now();
    engine.block_on(s.start_spv()).unwrap();
    while s.spv_state().unwrap() != SpvState::Running {
        assert!(called.elapsed() < Duration::from_secs(2), "SPV waited");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        s.dashpay_startup(&id).unwrap().startup,
        StartupStatus::NotRun
    );
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn the_cadence_paces_the_loops() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let manager = s.manager().unwrap();
    s.set_platform_cadence(PlatformCadence {
        window_visible: false,
        contest_ending_soon: true,
    })
    .unwrap();
    assert_eq!(manager.dashpay_sync().interval(), Duration::from_secs(60));
    assert_eq!(manager.dpns_sync().interval(), Duration::from_secs(60));
    s.set_platform_cadence(PlatformCadence::default()).unwrap();
    assert_eq!(manager.dashpay_sync().interval(), Duration::from_secs(15));
    assert_eq!(manager.dpns_sync().interval(), Duration::from_secs(600));
    // Nothing runs before SPV does.
    s.dashpay_sync_soon().unwrap();
    assert!(!manager.dashpay_sync().is_syncing());
    engine.block_on(engine.shutdown()).unwrap();
}

/// ROADMAP E0-05 acceptance on public testnet: a wallet restored from a seed
/// that owns an identity with an established DashPay contact reports `Ready`
/// and has its contact (receival) accounts when SPV starts, so the first
/// filter scan watches them. SPV starts only after the bring-up returns, and
/// nothing else builds contact accounts before then (the loops start after
/// SPV and hold no signer), so accounts present at SPV's start come from the
/// bring-up.
///
/// `DW_E005_TESTNET_FIXTURE` names a directory with `A.mnemonic` (the seed)
/// and `fixture.json` (`A.identity_id`); see the E0-05 report.
#[test]
#[ignore = "public testnet; set DW_E005_TESTNET_FIXTURE"]
fn a_restored_testnet_wallet_with_an_identity_is_ready_before_spv() {
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_E005_TESTNET_FIXTURE").expect("DW_E005_TESTNET_FIXTURE"),
    );
    let phrase = Zeroizing::new(std::fs::read_to_string(fixture.join("A.mnemonic")).unwrap());
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let identity = meta["A"]["identity_id"].as_str().unwrap().to_owned();

    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path(), 4);
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let id = engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.trim().as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();

    let called = Instant::now();
    engine.block_on(s.start_spv()).unwrap();
    let took = called.elapsed();
    while s.spv_state().unwrap() != SpvState::Running {
        assert!(
            called.elapsed() < Duration::from_secs(25),
            "SPV did not start"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let spv_after = called.elapsed();
    // Read at once: what the first filter scan starts with.
    let manager = s.manager().unwrap();
    let receival = {
        let wm = manager.wallet_manager_arc();
        let wm = engine.block_on(async move { wm.read_owned().await });
        wm.get_wallet_info(&id.0)
            .unwrap()
            .core_wallet
            .accounts
            .dashpay_receival_accounts
            .len()
    };
    let startup = s.dashpay_startup(&id).unwrap();
    eprintln!(
        "testnet restore: start_spv took {took:?}; SPV running {spv_after:?} after the call; \
         startup {:?}, identity {:?}, pending {}, receival accounts at SPV start {receival}",
        startup.startup, startup.identity, startup.contact_accounts_pending
    );
    assert_eq!(startup.startup, StartupStatus::Ready);
    assert_eq!(startup.identity.as_deref(), Some(identity.as_str()));
    assert_eq!(startup.contact_accounts_pending, 0);
    assert!(receival > 0, "no contact accounts when SPV started");
    assert!(took < Duration::from_millis(100), "{took:?}");
    engine.block_on(engine.shutdown()).unwrap();
}

const PASSPHRASE: &[u8] = b"runtime tests vault passphrase";

/// Listeners that never accept: every DAPI request to them hangs. Several,
/// so a request the DAPI client gives up on one address for (after about
/// 10 s) moves on to the next instead of ending the pass.
fn blackhole() -> Vec<std::net::TcpListener> {
    (0..4)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect()
}

/// A regtest session with DAPI and the quorum service on `hole`, and a vault
/// encrypted with [`PASSPHRASE`] (left unlocked) or unencrypted.
fn session_on(
    dir: &std::path::Path,
    hole: &[std::net::TcpListener],
    encrypted: bool,
) -> (Engine, Arc<NetworkSession>) {
    let urls = hole
        .iter()
        .map(|l| format!("http://{}", l.local_addr().unwrap()))
        .collect();
    session_at(dir, urls, encrypted)
}

/// As [`session_on`], with DAPI and the quorum service on `urls`.
fn session_at(
    dir: &std::path::Path,
    urls: Vec<String>,
    encrypted: bool,
) -> (Engine, Arc<NetworkSession>) {
    session_with(engine(dir, 4), urls, encrypted)
}

fn session_with(
    engine: Engine,
    urls: Vec<String>,
    encrypted: bool,
) -> (Engine, Arc<NetworkSession>) {
    session_with_timeout(engine, urls, encrypted, None)
}

/// As [`session_on`], with DAPI attempt timeouts disabled: a request to
/// `hole` stays open until the hole is released.
fn session_held(dir: &std::path::Path, hole: &HeldHole) -> (Engine, Arc<NetworkSession>) {
    session_with_timeout(
        engine(dir, 4),
        hole.urls.clone(),
        false,
        Some(Duration::ZERO),
    )
}

/// Four DAPI addresses that accept every connection and never answer.
/// Released, each held connection is shut down and each later one is shut
/// down as it arrives, so a request on it fails at once on every OS: a
/// closed listener's unaccepted connections and a refused connect end at
/// an OS's own pace (#182: Windows outlived the test's wait). Dropped, it
/// releases first, so a test that fails cannot leave a pass waiting.
struct HeldHole {
    urls: Vec<String>,
    held: Arc<Mutex<(bool, Vec<std::net::TcpStream>)>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl HeldHole {
    fn new() -> Self {
        let held = Arc::new(Mutex::new((false, Vec::new())));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut urls = Vec::new();
        let mut threads = Vec::new();
        for _ in 0..4 {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            urls.push(format!("http://{}", listener.local_addr().unwrap()));
            let (held, stop) = (Arc::clone(&held), Arc::clone(&stop));
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let mut held = guard(&held);
                            if held.0 {
                                let _ = stream.shutdown(std::net::Shutdown::Both);
                            } else {
                                held.1.push(stream);
                            }
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            }));
        }
        Self {
            urls,
            held,
            stop,
            threads,
        }
    }

    fn release(&self) {
        let mut held = guard(&self.held);
        held.0 = true;
        for stream in held.1.drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

impl Drop for HeldHole {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn session_with_timeout(
    engine: Engine,
    urls: Vec<String>,
    encrypted: bool,
    dapi_request_timeout: Option<Duration>,
) -> (Engine, Arc<NetworkSession>) {
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                quorum_url: Some(urls[0].clone()),
                dapi_addresses: urls,
                spv_peers: vec!["127.0.0.1:1".into()],
                dapi_request_timeout,
                ..Default::default()
            },
        ))
        .unwrap();
    engine
        .block_on(s.vault_op(move |v| v.create(encrypted.then_some(PASSPHRASE))))
        .unwrap();
    engine.block_on(s.adopt_signing_window_default()).unwrap();
    (engine, s)
}

fn wait_until(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn startup(s: &NetworkSession, id: &WalletId) -> StartupStatus {
    s.dashpay_startup(id).unwrap().startup
}

/// Puts an identity (no keys) on file for `id`: its DashPay pass then asks
/// DAPI for the identity's contact requests.
fn give_identity(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) {
    use dpp::identity::Identity;
    use dpp::identity::v0::IdentityV0;
    let manager = s.manager().unwrap();
    engine.block_on(async move {
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        let identity = Identity::V0(IdentityV0 {
            id: [7u8; 32].into(),
            public_keys: Default::default(),
            balance: 0,
            revision: 0,
        });
        info.identity_manager
            .add_identity(identity, 0, id.0, wallet.persister())
            .unwrap();
    });
}

/// The Platform loops a session on [`blackhole`] points at it: each pass
/// any of them has in flight at a stop may still be waiting at the deadline.
const BLACKHOLED_LOOPS: [&str; 5] = [
    "identity_sync",
    "dashpay_sync",
    "dpns_sync",
    "platform_address_sync",
    "shielded_sync",
];

/// Whether `stopped` is the stop's refusal naming the loops still in flight
/// at its deadline (`bringup.rs` `stop_platform`): at least one, and only
/// [`BLACKHOLED_LOOPS`]. Which of them are still in flight depends on timing.
pub(super) fn names_only_stuck_loops(stopped: &Result<(), EngineError>) -> bool {
    let Err(EngineError::Sdk(detail)) = stopped else {
        return false;
    };
    let Some(names) = detail.strip_prefix("SPV stopped, but this Platform work did not end: ")
    else {
        return false;
    };
    !names.is_empty() && names.split(", ").all(|n| BLACKHOLED_LOOPS.contains(&n))
}

/// 0.1.x row 66: the stop names whichever loops are still in flight, and on
/// CI that was `dpns_sync` alone, with the DashPay pass already ended.
#[test]
fn the_stop_check_takes_whichever_blackholed_loops_are_stuck() {
    let refused = |names: &str| -> Result<(), EngineError> {
        Err(EngineError::Sdk(format!(
            "SPV stopped, but this Platform work did not end: {names}"
        )))
    };
    for names in [
        "dpns_sync",
        "dashpay_sync",
        "shielded_sync",
        "identity_sync, dpns_sync, platform_address_sync",
    ] {
        assert!(names_only_stuck_loops(&refused(names)), "{names}");
    }
    for names in [
        "",
        "a vault key read",
        "dpns_sync, a vault key read",
        "dpns",
    ] {
        assert!(!names_only_stuck_loops(&refused(names)), "{names:?}");
    }
    assert!(!names_only_stuck_loops(&Ok(())));
    assert!(!names_only_stuck_loops(&Err(EngineError::Sdk(
        "dpns_sync".into()
    ))));
    assert!(!names_only_stuck_loops(&Err(EngineError::Wallet(
        "SPV stopped, but this Platform work did not end: dpns_sync".into()
    ))));
}

/// Review DW-E0-05-r1-gpt M1: a loop pass that outlives the drain deadline
/// makes `stop_spv` fail visibly (SPV is stopped anyway), and the next
/// `start_spv` does not wait for that pass: the drain is retried inside the
/// start task. Before the fix the stop returned `Ok`, and the start waited
/// another full drain budget (10 s).
///
/// The pass and that budget are the same length. A DAPI attempt ends at
/// 10 s (rs-dapi-client `request_settings.rs:9`, applied in
/// `dapi_client.rs:1198`) and `quiesce` waits
/// `COORDINATOR_DRAIN_BUDGET`, also 10 s (rs-platform-wallet
/// `manager/mod.rs:95`, `dashpay_sync.rs:287`). On a loaded runner the
/// attempt finishes as the budget runs out, the stop finds nothing left,
/// and it returns `Ok(())`. This session disables the attempt deadline
/// (`dapi_request_timeout: Some(0)`) and holds the passes' connections
/// until after the stop and the next start, then releases them
/// ([`HeldHole`]).
#[test]
fn a_stop_that_cannot_drain_a_pass_says_so_and_the_next_start_is_immediate() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let hole = HeldHole::new();
    let (engine, s) = session_held(dir.path(), &hole);
    // After the engine, so a panic — the stop, restart, and shutdown
    // bounds below included — drops this first and releases the passes
    // before the engine's drop thread joins them.
    let release = hole;
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    give_identity(&engine, &s, id);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV running", Duration::from_secs(10), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let manager = s.manager().unwrap();
    wait_until("a DashPay pass in flight", Duration::from_secs(10), || {
        manager.dashpay_sync().is_syncing()
    });

    let stopping = Instant::now();
    // `block_on` drives this on the engine's tokio runtime. The outer
    // bound fails the test if a stop stops draining, instead of hanging
    // the job. With no attempt deadline the pass does not end on its own.
    let stopped = engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(60), s.stop_spv()).await })
        .expect("the stop returned within 60 s");
    eprintln!(
        "stop with a stuck pass took {:?}: {stopped:?}",
        stopping.elapsed()
    );
    assert!(names_only_stuck_loops(&stopped), "{stopped:?}");
    assert_eq!(s.spv_state().unwrap(), SpvState::Stopped);

    // Allow scheduler delay, but not a full 10 s coordinator drain. A
    // restart that waits for the held pass hits this bound and drops
    // `release` on the way out, so the test fails instead of hanging.
    engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(5), s.start_spv()).await })
        .expect("restart returned before the drain budget")
        .unwrap();
    assert!(
        manager.dashpay_sync().is_syncing(),
        "restart returned while the DashPay pass was still in flight"
    );
    assert_ne!(s.spv_state().unwrap(), SpvState::Stopped);
    release.release();
    wait_until("the held passes end", Duration::from_secs(10), || {
        !manager.dashpay_sync().is_syncing()
            && !manager.identity_sync().is_syncing()
            && !manager.dpns_sync().is_syncing()
            && !manager.platform_address_sync().is_syncing()
            && !manager.shielded_sync().is_syncing()
    });
    engine
        .block_on(async { tokio::time::timeout(Duration::from_secs(60), engine.shutdown()).await })
        .expect("the shutdown returned within 60 s")
        .unwrap();
}

/// Reads `spv_state` as each `SpvStateChanged{running: true}` is emitted.
#[derive(Default)]
struct StateAtRunning {
    session: OnceLock<Weak<NetworkSession>>,
    seen: Mutex<Vec<SpvState>>,
}

impl EventSink for StateAtRunning {
    fn emit(&self, event: EngineEvent) {
        if let EngineEvent::SpvStateChanged { running: true, .. } = event
            && let Some(s) = self.session.get().and_then(Weak::upgrade)
        {
            guard(&self.seen).push(s.spv_state().unwrap());
        }
    }
}

/// #231: a renderer that refreshes on `SpvStateChanged{running: true}` must
/// read `Running`, from the bring-up's start and from `rotate_peers`'
/// restart. Before the fix the supervisor announced the start before
/// marking itself running, so the read was `Starting` and no later event
/// corrected it. The pause holds the supervisor between the two.
#[test]
fn spv_reads_running_when_its_running_event_arrives() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let rec = Arc::new(StateAtRunning::default());
    let (engine, s) = session_with(
        engine_with(dir.path(), 4, Arc::clone(&rec) as Arc<dyn EventSink>),
        vec!["http://127.0.0.1:1".into()],
        false,
    );
    rec.session.set(Arc::downgrade(&s)).unwrap();
    let pause = Arc::new(TestPause::default());
    *guard(&s.platform.pause_before_running) = Some(Arc::clone(&pause));
    engine.block_on(s.start_spv()).unwrap();
    wait_until("the start, held", Duration::from_secs(5), || {
        pause.reached.load(Ordering::SeqCst)
    });
    pause.release.notify_one();
    wait_until("the running event", Duration::from_secs(5), || {
        !guard(&rec.seen).is_empty()
    });
    assert_eq!(*guard(&rec.seen), [SpvState::Running]);

    engine.block_on(s.rotate_peers()).unwrap();
    assert_eq!(*guard(&rec.seen), [SpvState::Running, SpvState::Running]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// E0-04 Q15, ruling (c), DEC-1032: the library re-sends a stored
/// unconfirmed shielded spend only from its coordinator's sync pass
/// (`coordinator.rs:1431`). The library's own loop never runs; the
/// supervisor drives the passes (`shielded_pool::guarded_pass`) while the
/// session is open, and the close seals the coordinator: no pass starts
/// after it, the engine's nor a direct one.
#[test]
fn the_close_ends_the_shielded_passes_that_redrive_spends() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let hole = blackhole();
    let (engine, s) = session_on(dir.path(), &hole, false);
    // The pool's count unread at once: the pass goes on to the silent nodes
    // before the other bring-up calls give up on them.
    *s.platform.notes_count.lock().unwrap() = Some(None);
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    give_identity(&engine, &s, id);
    engine.block_on(s.start_spv()).unwrap();
    let manager = s.manager().unwrap();
    // The supervisor's pass, stuck on the silent nodes.
    wait_until("a shielded pass in flight", Duration::from_secs(30), || {
        manager.shielded_sync().is_syncing()
    });
    assert!(
        !manager.shielded_sync().is_running(),
        "the library's loop never runs"
    );

    let network = s.network.clone();
    assert!(engine.block_on(engine.close_network(network)).unwrap());
    assert!(engine.block_on(s.enter()).is_err(), "no engine pass starts");
    assert!(!manager.shielded_sync().is_running());
    let pass = engine.block_on(manager.shielded_sync().sync_now(true));
    assert!(pass.is_empty() && pass.sync_unix_seconds == 0, "{pass:?}");
    engine.block_on(engine.shutdown()).unwrap();
}

/// Writes a `.dwbackup` of `wallets` sealed by `s`'s vault (backup
/// passphrase `bk`), as `r2_compat_offline.rs` does.
fn craft_backup(s: &Arc<NetworkSession>, path: &std::path::Path, wallets: &[WalletId]) {
    let header = serde_json::json!({
        "format": "dwbackup", "format_version": 1, "network": "regtest", "created_at": 1,
        "wallet_ids": wallets.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
        "automatic": false, "app_version": "test",
    });
    let header_line = serde_json::to_vec(&header).unwrap();
    let payload = serde_json::json!({
        "name": "A", "birth_height": 0, "created_at": null, "app_rows": []
    });
    let bundles: Vec<_> = wallets
        .iter()
        .map(|id| {
            s.vault()
                .backup_bundle(
                    &id.0,
                    Some(b"bk"),
                    &serde_json::to_vec(&payload).unwrap(),
                    &header_line,
                )
                .unwrap()
        })
        .collect();
    let mut out = b"DWBACKUP 1\n".to_vec();
    out.extend_from_slice(&header_line);
    out.push(b'\n');
    out.extend(serde_json::to_vec(&serde_json::json!({ "bundles": bundles })).unwrap());
    out.push(b'\n');
    std::fs::write(path, out).unwrap();
}

/// Review DW-E0-05-r1-gpt M4: while SPV runs, a restore that fails on a
/// later bundle starts no bring-up for the wallet it rolls back (one would
/// resolve that wallet's scan key and hold it until its budget ended); a
/// restore that succeeds brings its wallets up once it has committed.
/// Before the fix each bundle's import signalled the bring-up at once.
#[test]
fn a_failed_restore_starts_no_bring_up_for_what_it_rolls_back() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (src_engine, src) = session(&dir.path().join("src"));
    let a = src_engine
        .block_on(src.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let failing = dir.path().join("failing.dwbackup");
    let good = dir.path().join("good.dwbackup");
    // The second bundle is the same wallet again: `AlreadyExists`.
    craft_backup(&src, &failing, &[a, a]);
    craft_backup(&src, &good, &[a]);

    let hole = blackhole();
    let (engine, s) = session_on(&dir.path().join("dst"), &hole, false);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV running", Duration::from_secs(5), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let bk = || Some(Zeroizing::new(b"bk".to_vec()));
    let r = engine.block_on(s.restore_backup(failing, bk()));
    assert!(r.is_err(), "{r:?}");
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        s.platform.startup_of(&a).is_none(),
        "a bring-up ran for a rolled-back wallet: {:?}",
        s.platform.startup_of(&a)
    );
    assert!(!guard(&s.platform.tasks).contains_key(&a));

    assert_eq!(
        engine.block_on(s.restore_backup(good, bk())).unwrap(),
        vec![a]
    );
    wait_until(
        "the committed wallet's bring-up",
        Duration::from_secs(5),
        || {
            s.platform
                .startup_of(&a)
                .is_some_and(|st| st.startup == StartupStatus::Starting)
        },
    );
    engine.block_on(engine.shutdown()).unwrap();
    src_engine.block_on(src_engine.shutdown()).unwrap();
}

/// Review DW-E0-05-r1-gpt M5: the vault locks after the bring-up built its
/// keys and before the library call is polled. The stale scan key then
/// answers `Unavailable` at once, so the call and the lock are ready
/// together; the outcome must still be `IdentityUnsettled`, so the next
/// unlock runs discovery again. Before the fix the unbiased select took the
/// call about half the time and recorded `PartialNoIdentity`, and the unlock
/// only drained. Eight rounds make a lucky pass unlikely (1 in 256).
#[test]
fn a_lock_after_the_keys_were_built_leaves_the_identity_unsettled() {
    let _serial = serial();
    for round in 0..8 {
        let dir = dw_testutil::private_tempdir();
        // DAPI refuses at once: the race needs no network wait (the stale
        // scan key answers by itself), and the passes then end quickly.
        let (engine, s) = session_at(dir.path(), vec!["http://127.0.0.1:1".into()], true);
        let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
        let pause = Arc::new(TestPause::default());
        *guard(&s.platform.pause_after_keys) = Some(Arc::clone(&pause));
        engine.block_on(s.start_spv()).unwrap();
        wait_until("the keys", Duration::from_secs(5), || {
            pause.reached.load(std::sync::atomic::Ordering::SeqCst)
        });
        s.lock_vault_sync().unwrap();
        pause.release.notify_one();
        wait_until("the outcome", Duration::from_secs(5), || {
            startup(&s, &id) != StartupStatus::Starting
        });
        assert_eq!(
            startup(&s, &id),
            StartupStatus::IdentityUnsettled,
            "round {round}"
        );

        engine
            .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, dw_vault::UnlockScope::Full)))
            .unwrap();
        wait_until(
            "discovery again after the unlock",
            Duration::from_secs(5),
            || startup(&s, &id) == StartupStatus::Starting,
        );
        engine.block_on(engine.shutdown()).unwrap();
    }
}

fn set_no_identity(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId, age: Duration) {
    let scope = dw_appdb::local_scope(&id.to_string());
    let at = crate::events::unix_now() - age.as_secs();
    engine
        .block_on(
            s.appdb_op(move |db| db.set_setting(&scope, NO_IDENTITY_KEY, Some(&at.to_string()))),
        )
        .unwrap();
}

/// pasta's ruling (E0-05 r1): a proven absence is trusted for 7 days, then
/// discovery runs again.
#[test]
fn a_proven_absence_expires_after_seven_days() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let id = engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    set_no_identity(&engine, &s, id, Duration::from_secs(8 * 24 * 3600));
    engine.block_on(s.start_spv()).unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        startup(&s, &id),
        StartupStatus::Starting,
        "an 8-day-old absence skipped discovery"
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP6-01's "find" forgets the proven absence, so the next start runs
/// discovery.
#[test]
fn find_forgets_the_proven_absence() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    set_no_identity(&engine, &s, id, Duration::from_secs(60));
    assert!(marker(&engine, &s, id, NO_IDENTITY_KEY));
    engine.block_on(s.forget_proven_absence(id)).unwrap();
    assert!(!marker(&engine, &s, id, NO_IDENTITY_KEY));
}

/// Holds the thread that emits `WalletCreated` while `armed`: a restore
/// stopped right after it registered a wallet, before it committed.
#[derive(Default)]
struct HoldWalletCreated {
    armed: std::sync::atomic::AtomicBool,
    held: std::sync::atomic::AtomicBool,
    release: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}

impl EventSink for HoldWalletCreated {
    fn emit(&self, event: EngineEvent) {
        use std::sync::atomic::Ordering::SeqCst;
        if matches!(event, EngineEvent::WalletCreated { .. }) && self.armed.swap(false, SeqCst) {
            self.held.store(true, SeqCst);
            let mut release = self.release.lock().unwrap();
            while !*release {
                release = self.wake.wait(release).unwrap();
            }
            *release = false;
            self.held.store(false, SeqCst);
        }
    }
}

impl HoldWalletCreated {
    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn wait_held(&self) {
        wait_until(
            "the restore to register its wallet",
            Duration::from_secs(10),
            || self.held.load(std::sync::atomic::Ordering::SeqCst),
        );
    }

    fn let_go(&self) {
        *self.release.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

/// Review DW-E0-05-r2-gpt M4: a start that lists the wallets while a
/// restore has registered one but not committed must not admit it; neither
/// may anything else until the restore commits. Deterministic: the restore is
/// held at its first `WalletCreated`, after registration, and SPV is started
/// then. Before the fix that start admitted the wallet (`Starting`), and the
/// restore then failed on its second bundle and rolled the wallet back.
#[test]
fn a_start_during_an_uncommitted_restore_skips_its_wallet() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (src_engine, src) = session(&dir.path().join("src"));
    let a = src_engine
        .block_on(src.create_wallet(12))
        .unwrap()
        .wallet_id;
    let failing = dir.path().join("failing.dwbackup");
    let good = dir.path().join("good.dwbackup");
    craft_backup(&src, &failing, &[a, a]);
    craft_backup(&src, &good, &[a]);

    let hold = Arc::new(HoldWalletCreated::default());
    // DAPI refuses: a restored wallet's discovery keeps retrying through
    // its 20 s budget, and close does not wait on stuck passes.
    let (engine, s) = session_with(
        engine_with(
            &dir.path().join("dst"),
            4,
            Arc::clone(&hold) as Arc<dyn EventSink>,
        ),
        vec!["http://127.0.0.1:1".into()],
        false,
    );
    let engine = Arc::new(engine);
    let restore = |path: std::path::PathBuf| {
        let (engine, s) = (Arc::clone(&engine), Arc::clone(&s));
        std::thread::spawn(move || {
            engine.block_on(s.restore_backup(path, Some(Zeroizing::new(b"bk".to_vec()))))
        })
    };
    // Admitted: a bring-up status recorded for it, or the catch-up mark its
    // first bring-up writes (`note_first_bring_up`). A task alone is not:
    // the contest watch runs one for every wallet of the manager, a held
    // restore's included, and that pass does nothing for a wallet with no
    // catch-up (`contest_watch.rs` `watch_contests`). Counting it failed
    // this test whenever the start's first contest pass was in flight at an
    // assertion (CI 41289102488).
    let admitted = |s: &NetworkSession| {
        s.platform.startup_of(&a).is_some() || engine.block_on(s.has_catch_up(a))
    };

    // A failing restore, held after registering `a`; SPV starts meanwhile.
    hold.arm();
    let restoring = restore(failing);
    hold.wait_held();
    engine.block_on(s.start_spv()).unwrap();
    assert!(
        !admitted(&s),
        "a start admitted an uncommitted restore's wallet: {:?}",
        s.platform.startup_of(&a)
    );
    // Nothing held SPV for it.
    wait_until("SPV running", Duration::from_secs(5), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    assert!(!admitted(&s));
    hold.let_go();
    assert!(restoring.join().unwrap().is_err());
    std::thread::sleep(Duration::from_secs(1));
    assert!(!admitted(&s), "a rolled-back wallet was brought up");

    // The same wallet restored for good: not before the commit, then once.
    hold.arm();
    let restoring = restore(good);
    hold.wait_held();
    std::thread::sleep(Duration::from_millis(300));
    assert!(!admitted(&s), "brought up before the restore committed");
    hold.let_go();
    assert_eq!(restoring.join().unwrap().unwrap(), vec![a]);
    wait_until(
        "the committed wallet's bring-up",
        Duration::from_secs(5),
        || {
            s.platform
                .startup_of(&a)
                .is_some_and(|st| st.startup == StartupStatus::Starting)
        },
    );
    engine.block_on(engine.shutdown()).unwrap();
    src_engine.block_on(src_engine.shutdown()).unwrap();
}

/// One successful restore of a wallet a start refused brings it up once: the
/// readmission when its mark clears and its commit's signal report the same
/// event (review r4 M4-R4).
#[test]
fn a_successful_restore_is_brought_up_once() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (src_engine, src) = session(&dir.path().join("src"));
    let a = src_engine
        .block_on(src.create_wallet(12))
        .unwrap()
        .wallet_id;
    let backup = dir.path().join("a.dwbackup");
    craft_backup(&src, &backup, &[a]);
    let hold = Arc::new(HoldWalletCreated::default());
    hold.arm();
    let (engine, s) = session_with(
        engine_with(
            &dir.path().join("dst"),
            4,
            Arc::clone(&hold) as Arc<dyn EventSink>,
        ),
        vec!["http://127.0.0.1:1".into()],
        true,
    );
    let engine = Arc::new(engine);
    let (e, cloned) = (Arc::clone(&engine), Arc::clone(&s));
    let restoring = std::thread::spawn(move || {
        e.block_on(cloned.restore_backup(backup, Some(Zeroizing::new(b"bk".to_vec()))))
    });
    // The start is refused `a` while the restore marks it.
    hold.wait_held();
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV running", Duration::from_secs(5), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let first = Arc::new(TestPause::default());
    *guard(&s.platform.pause_after_keys) = Some(Arc::clone(&first));
    hold.let_go();
    assert_eq!(restoring.join().unwrap().unwrap(), vec![a]);
    wait_until("the bring-up", Duration::from_secs(5), || {
        first.reached.load(Ordering::SeqCst)
    });

    let second = Arc::new(TestPause::default());
    *guard(&s.platform.pause_after_keys) = Some(Arc::clone(&second));
    // Locking ends the pass early, and signals nothing.
    s.lock_vault_sync().unwrap();
    first.release.notify_one();
    // Queued after every signal of the restore: once it is recorded, they
    // were all handled.
    let sentinel = WalletId([95; 32]);
    s.platform
        .signal(super::runtime::PlatformSignal::WalletAdded(sentinel));
    wait_until(
        "the restore's signals handled",
        Duration::from_secs(10),
        || second.reached.load(Ordering::SeqCst) || s.platform.startup_of(&sentinel).is_some(),
    );
    let twice = second.reached.load(Ordering::SeqCst);
    second.release.notify_one();
    assert!(!twice, "one restore brought its wallet up twice");
    engine.block_on(engine.shutdown()).unwrap();
    src_engine.block_on(src_engine.shutdown()).unwrap();
}

/// Signals for a wallet whose bring-up is running make one follow-up pass,
/// however many arrive (review r4 M4-R4).
#[test]
fn signals_during_a_bring_up_make_one_follow_up() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session_with(
        engine(dir.path(), 4),
        vec!["http://127.0.0.1:1".into()],
        true,
    );
    let a = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    let pause = |s: &NetworkSession| {
        let pause = Arc::new(TestPause::default());
        *guard(&s.platform.pause_after_keys) = Some(Arc::clone(&pause));
        pause
    };
    let first = pause(&s);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("the start's bring-up", Duration::from_secs(5), || {
        first.reached.load(Ordering::SeqCst)
    });
    for _ in 0..2 {
        s.platform
            .signal(super::runtime::PlatformSignal::WalletAdded(a));
    }
    // Locking ends each pass early, and signals nothing.
    s.lock_vault_sync().unwrap();
    let second = pause(&s);
    first.release.notify_one();
    wait_until("the follow-up", Duration::from_secs(10), || {
        second.reached.load(Ordering::SeqCst)
    });
    let third = pause(&s);
    second.release.notify_one();
    let sentinel = WalletId([96; 32]);
    s.platform
        .signal(super::runtime::PlatformSignal::WalletAdded(sentinel));
    wait_until("the signals handled", Duration::from_secs(10), || {
        third.reached.load(Ordering::SeqCst) || s.platform.startup_of(&sentinel).is_some()
    });
    let again = third.reached.load(Ordering::SeqCst);
    third.release.notify_one();
    assert!(!again, "two signals made two follow-up passes");
    engine.block_on(engine.shutdown()).unwrap();
}

/// A refusal racing the last mark's release is never lost, and readmits
/// once (review r4 probe).
#[test]
fn a_refusal_racing_the_last_release_readmits_once() {
    use super::runtime::{PlatformSignal, Supervisor};
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let (cancel, _) = tokio::sync::watch::channel(false);
    let (signals, mut rx) = tokio::sync::mpsc::unbounded_channel();
    *guard(&s.platform.supervisor) = Some(Supervisor {
        cancel,
        signals,
        task: None,
        running: false,
    });
    for n in 0..1000u32 {
        let mut bytes = [0; 32];
        bytes[..4].copy_from_slice(&n.to_le_bytes());
        let id = WalletId(bytes);
        let mark = s.platform.restoring(vec![id]);
        let barrier = std::sync::Barrier::new(2);
        let answer = std::thread::scope(|scope| {
            let t = scope.spawn(|| {
                barrier.wait();
                s.platform.admit(&id, || true)
            });
            barrier.wait();
            drop(mark);
            t.join().unwrap()
        });
        match answer {
            None => assert!(
                matches!(rx.try_recv(), Ok((_, PlatformSignal::Readmit(found))) if found == id)
            ),
            Some(true) => {}
            other => panic!("{other:?}"),
        }
        assert!(rx.try_recv().is_err(), "a readmission too many");
    }
    guard(&s.platform.supervisor).take();
    engine.block_on(engine.shutdown()).unwrap();
}

/// Routes `WalletCreated` of `a` and `b` to their own holds (review r3).
struct HoldTwo {
    a: WalletId,
    b: WalletId,
    a_hold: HoldWalletCreated,
    b_hold: HoldWalletCreated,
}

impl EventSink for HoldTwo {
    fn emit(&self, event: EngineEvent) {
        if let EngineEvent::WalletCreated { wallet_id, .. } = &event {
            if *wallet_id == self.a {
                self.a_hold.emit(event);
            } else if *wallet_id == self.b {
                self.b_hold.emit(event);
            }
        }
    }
}

/// Review DW-E0-05-r3-gpt M4-R3 (its probe
/// `r3_overlapping_restore_loses_committed_wallet_bringup`, with the
/// assertions turned to the fixed behaviour): R1 = [a] commits while `a` is
/// still marked as being restored, so R1's bring-up signal is refused; when
/// the last mark clears, `a` (committed, surviving) must be brought up, and
/// `b` (rolled back) must not. Before the fix `a` stayed without a bring-up
/// until the next start.
///
/// Restores run one at a time since DEC-210 (3b): R2 = [b, a] waits at the
/// wallet lifecycle lock until R1 has committed, then fails on `a` and rolls
/// `b` back. The overlapping mark is still possible (R1's signal is handled
/// after R2 has marked `a`, a matter of timing); a mark the test holds on
/// [b, a] stands in for it.
#[test]
fn overlapping_restores_keep_a_committed_wallets_bring_up() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (src_engine, src) = session(&dir.path().join("src"));
    let a = src_engine
        .block_on(src.create_wallet(12))
        .unwrap()
        .wallet_id;
    let b = src_engine
        .block_on(src.create_wallet(12))
        .unwrap()
        .wallet_id;
    let first = dir.path().join("a.dwbackup");
    let second = dir.path().join("b-a.dwbackup");
    craft_backup(&src, &first, &[a]);
    craft_backup(&src, &second, &[b, a]);
    let hold = Arc::new(HoldTwo {
        a,
        b,
        a_hold: HoldWalletCreated::default(),
        b_hold: HoldWalletCreated::default(),
    });
    hold.a_hold.arm();
    hold.b_hold.arm();
    let (engine, s) = session_with(
        engine_with(
            &dir.path().join("dst"),
            4,
            Arc::clone(&hold) as Arc<dyn EventSink>,
        ),
        vec!["http://127.0.0.1:1".into()],
        false,
    );
    let engine = Arc::new(engine);
    let restore = |path: std::path::PathBuf| {
        let (engine, s) = (Arc::clone(&engine), Arc::clone(&s));
        std::thread::spawn(move || {
            engine.block_on(s.restore_backup(path, Some(Zeroizing::new(b"bk".to_vec()))))
        })
    };
    let r1 = restore(first);
    hold.a_hold.wait_held();
    let r2 = restore(second);
    wait_until("R2 at the lifecycle lock", Duration::from_secs(20), || {
        r2.is_finished() || s.wallet_lifecycle.waiting() == 1
    });
    assert!(!r2.is_finished(), "R2 ran beside R1");
    let marked = s.platform.restoring(vec![b, a]);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV running", Duration::from_secs(5), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    assert!(s.platform.startup_of(&a).is_none());

    hold.a_hold.let_go();
    assert_eq!(r1.join().unwrap().unwrap(), vec![a]);
    // A sentinel queued after R1's signal: once it is recorded, R1's signal
    // was handled, and refused, since `a` is still marked.
    let sentinel = WalletId([93; 32]);
    s.platform
        .signal(super::runtime::PlatformSignal::WalletAdded(sentinel));
    wait_until("R1's signal handled", Duration::from_secs(5), || {
        s.platform.startup_of(&sentinel).is_some()
    });
    assert!(s.platform.startup_of(&a).is_none());

    hold.b_hold.wait_held();
    hold.b_hold.let_go();
    assert!(r2.join().unwrap().is_err());
    assert!(s.platform.startup_of(&a).is_none());
    drop(marked);
    wait_until(
        "the committed wallet's bring-up",
        Duration::from_secs(5),
        || s.platform.startup_of(&a).is_some(),
    );
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        s.platform.startup_of(&b).is_none() && !guard(&s.platform.tasks).contains_key(&b),
        "the rolled-back wallet was brought up"
    );
    engine.block_on(engine.shutdown()).unwrap();
    src_engine.block_on(src_engine.shutdown()).unwrap();
}

/// DP1-04 through the supervisor: right after the loops start, a pass
/// finds the outcome of a contest that resolved while the app was closed;
/// a watch `register_name` starts later re-arms it at once.
#[test]
fn the_supervisor_runs_the_contest_watch() {
    use super::contest_watch::PollWinner;
    use super::contest_watch_tests::{FakePolls, poll};
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let hole = blackhole();
    let (engine, s) = session_on(dir.path(), &hole, false);
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    give_identity(&engine, &s, id);
    let me: dpp::prelude::Identifier = [7u8; 32].into();
    let (w, i) = (
        id.to_string(),
        me.to_string(dpp::platform_value::string_encoding::Encoding::Base58),
    );
    let (w2, i2) = (w.clone(), i.clone());
    engine
        .block_on(s.appdb_op(move |db| db.watch_contest(&w2, &i2, "alice", 0)))
        .unwrap();
    // Resolved after this installation's first bring-up: news.
    let later_ms = (crate::events::unix_now() + 3_600) * 1000;
    let net = Arc::new(FakePolls::default());
    net.set(
        "a11ce",
        poll(
            Some(PollWinner::Identity([9u8; 32].into())),
            later_ms,
            &[7, 9],
        ),
    );
    net.set("b0b", poll(Some(PollWinner::Locked), later_ms, &[7]));
    *s.platform.contests.net.lock().unwrap() = Some(Arc::clone(&net));

    engine.block_on(s.start_spv()).unwrap();
    let kinds = || {
        let w = w.clone();
        let rows = engine
            .block_on(s.appdb_op(move |db| db.journal(&w)))
            .unwrap();
        (rows.into_iter())
            .filter(|r| r.kind.starts_with("contest_"))
            .map(|r| (r.kind, r.reference))
            .collect::<Vec<_>>()
    };
    wait_until("the first pass's outcome", Duration::from_secs(60), || {
        !kinds().is_empty()
    });
    assert_eq!(
        kinds(),
        [("contest_lost".to_string(), "alice@0".to_string())]
    );

    let manager = s.manager().unwrap();
    let s2 = Arc::clone(&s);
    engine.block_on(async move {
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        s2.start_watching(&wallet, id, &me, "bob", Some(5)).await;
    });
    wait_until("the re-armed pass", Duration::from_secs(30), || {
        kinds().len() == 2
    });
    assert_eq!(
        kinds()[1],
        ("contest_locked".to_string(), "bob@5".to_string())
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// CI 41289102488 (macOS): a reopen right after a close was refused with
/// `StorageInUse`. The persister's in-process claim goes with its last
/// reference, and the close returned while another was still held
/// (platform-wallet's drop names a drain, a wallet handle or an in-flight
/// read). The close now waits for it, within `PERSISTER_RELEASE`; a holder
/// that never lets go still keeps the database.
#[test]
fn a_close_returns_once_the_wallet_database_is_released() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path());
    let open = || {
        engine.block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
    };
    let close = || {
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        )
    };

    // A reference held a moment past the close, as an in-flight read's.
    let held = s.live().unwrap().persister;
    drop(s);
    let reader = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(1));
        drop(held);
    });
    close();
    let s = open().expect("the close waited for the database");
    reader.join().unwrap();

    // One held for good: the close returns within its bound, and the
    // database stays in use until it goes.
    let held = s.live().unwrap().persister;
    drop(s);
    let started = Instant::now();
    close();
    assert!(
        started.elapsed() < crate::session::PERSISTER_RELEASE + Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(matches!(open(), Err(EngineError::StorageInUse { .. })));
    drop(held);
    open().unwrap();
    engine.block_on(engine.shutdown()).unwrap();
}

/// Row 74: an in-app close (a network switch) locks the vault. A session
/// reference held past the close (a command still finishing) reads locked,
/// and a grant issued before the close is refused.
#[test]
fn a_close_locks_the_vault_of_a_retained_session() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session_at(dir.path(), vec!["http://127.0.0.1:1".into()], true);
    assert_eq!(s.vault().status().state, LockState::Unlocked);
    let grant = s
        .vault()
        .authorize(
            dw_vault::GrantPurpose::ChangeCredential,
            None,
            dw_vault::Credential::Passphrase(PASSPHRASE),
        )
        .unwrap();
    s.vault()
        .check_grant(&grant.id, dw_vault::GrantKind::ChangeCredential, None)
        .unwrap();

    assert!(
        engine
            .block_on(engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    assert_eq!(s.vault().status().state, LockState::Locked);
    assert!(
        s.vault()
            .check_grant(&grant.id, dw_vault::GrantKind::ChangeCredential, None)
            .is_err()
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// The account build a DashPay sync queues for a new contact of `id`'s
/// identity ([`give_identity`]), stored through the session's store and
/// its tap as the library stores it.
fn queue_contact_account(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) {
    queue_contact_op(engine, s, id, PendingContactCryptoOp::RegisterReceiving);
}

/// [`queue_contact_account`] with `op` for the contact.
fn queue_contact_op(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    id: WalletId,
    op: PendingContactCryptoOp,
) {
    use platform_wallet::changeset::{
        PendingContactCrypto, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let entry = PendingContactCrypto {
        owner_identity_id: [7u8; 32].into(),
        contact_id: [8u8; 32].into(),
        op,
        enqueued_at_ms: 0,
    };
    let manager = s.manager().unwrap();
    let queued = entry.clone();
    engine.block_on(async move {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        (info
            .identity_manager
            .managed_identity_mut(&[7u8; 32].into()))
        .unwrap()
        .dashpay_pending_contact_crypto_mut()
        .push(queued);
    });
    let store = s.live().unwrap().store;
    store
        .store(
            id.0,
            PlatformWalletChangeSet {
                pending_contact_crypto_added: vec![entry],
                ..Default::default()
            },
        )
        .unwrap();
}

fn queued_contact_work(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) -> usize {
    let manager = s.manager().unwrap();
    engine.block_on(async move {
        let wallet = manager.get_wallet(&id.0).await.unwrap();
        wallet
            .identity()
            .dashpay()
            .pending_contact_crypto_count()
            .await
    })
}

/// #84: a contact's account build that a sync queues while the vault is
/// unlocked is drained with no further action; while it is locked it stays
/// queued, and the unlock drains it.
#[test]
fn a_contact_account_queued_while_unlocked_is_built_at_once() {
    let _serial = serial();
    for locked in [false, true] {
        let dir = dw_testutil::private_tempdir();
        let (engine, s) = session_at(dir.path(), vec!["http://127.0.0.1:1".into()], true);
        let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
        give_identity(&engine, &s, id);
        engine.block_on(s.start_spv()).unwrap();
        wait_until("the bring-up", Duration::from_secs(30), || {
            !matches!(
                startup(&s, &id),
                StartupStatus::NotRun | StartupStatus::Starting
            )
        });
        if locked {
            s.lock_vault_sync().unwrap();
        }
        queue_contact_account(&engine, &s, id);
        if locked {
            std::thread::sleep(Duration::from_secs(2));
            assert_eq!(queued_contact_work(&engine, &s, id), 1, "kept while locked");
            engine
                .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, dw_vault::UnlockScope::Full)))
                .unwrap();
        }
        wait_until("the drain", Duration::from_secs(20), || {
            queued_contact_work(&engine, &s, id) == 0
        });
        engine.block_on(engine.shutdown()).unwrap();
    }
}

fn drains_run(s: &NetworkSession) -> u64 {
    s.platform.contact_drains_run.load(Ordering::SeqCst)
}

/// An unlocked session with SPV started and our identity on file, past
/// its bring-up.
fn started_with_identity(dir: &std::path::Path) -> (Engine, Arc<NetworkSession>, WalletId) {
    let (engine, s) = session_at(dir, vec!["http://127.0.0.1:1".into()], true);
    let id = engine.block_on(s.create_wallet(12)).unwrap().wallet_id;
    give_identity(&engine, &s, id);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("the bring-up", Duration::from_secs(30), || {
        !matches!(
            startup(&s, &id),
            StartupStatus::NotRun | StartupStatus::Starting
        )
    });
    (engine, s, id)
}

/// #84: every sweep stores the queue again for a contact still without its
/// accounts; the changesets that arrive before a requested drain starts
/// make that one drain.
#[test]
fn queued_changesets_before_a_drain_make_one_drain() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (engine, s, id) = started_with_identity(dir.path());
    std::thread::sleep(Duration::from_millis(500));
    let pause = Arc::new(TestPause::default());
    *guard(&s.platform.pause_contact_drain) = Some(Arc::clone(&pause));
    let before = drains_run(&s);
    queue_contact_account(&engine, &s, id);
    wait_until("the drain job", Duration::from_secs(10), || {
        pause.reached.load(Ordering::SeqCst)
    });
    queue_contact_account(&engine, &s, id);
    queue_contact_account(&engine, &s, id);
    pause.release.notify_one();
    wait_until("the drain", Duration::from_secs(20), || {
        queued_contact_work(&engine, &s, id) == 0
    });
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(drains_run(&s) - before, 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// #84: a drain that leaves the work queued is not run again for the next
/// sweep's changeset within the back-off; an unlock drains it, and after
/// the unlock a changeset asks for a drain again.
#[test]
fn a_failed_drain_waits_for_the_back_off_or_an_unlock() {
    let _serial = serial();
    let dir = dw_testutil::private_tempdir();
    let (engine, s, id) = started_with_identity(dir.path());
    std::thread::sleep(Duration::from_millis(500));
    // The contact's identity cannot be fetched: the build stays queued.
    let failing = || PendingContactCryptoOp::RegisterExternal {
        encrypted_public_key: vec![1; 96],
        our_decryption_key_index: 0,
        contact_encryption_key_index: 0,
    };
    let before = drains_run(&s);
    queue_contact_op(&engine, &s, id, failing());
    wait_until("the first drain", Duration::from_secs(30), || {
        drains_run(&s) > before
    });
    wait_until("the first drain's end", Duration::from_secs(60), || {
        s.platform.contact_drain_backed_off(&id)
    });
    assert_eq!(queued_contact_work(&engine, &s, id), 1, "left queued");
    let failed = drains_run(&s);

    queue_contact_op(&engine, &s, id, failing());
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(drains_run(&s), failed, "not again within the back-off");

    s.lock_vault_sync().unwrap();
    engine
        .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, dw_vault::UnlockScope::Full)))
        .unwrap();
    wait_until("the unlock's drain", Duration::from_secs(30), || {
        drains_run(&s) > failed
    });
    std::thread::sleep(Duration::from_secs(1));
    let unlocked = drains_run(&s);
    queue_contact_op(&engine, &s, id, failing());
    wait_until("a drain after the unlock", Duration::from_secs(30), || {
        drains_run(&s) > unlocked
    });
    engine.block_on(engine.shutdown()).unwrap();
}
