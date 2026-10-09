//! E0-05: the engine-owned bring-up (DASHPAY §2.5, §3.2). Offline: regtest
//! with SPV peers that refuse, and DAPI plus the quorum service on a local
//! listener that accepts connections and never answers, so every Platform
//! request hangs until the bring-up's budget cuts it ("DAPI blackholed").

use std::net::TcpListener;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::secp256k1::Secp256k1;
use dw_engine::{
    CREATED_HERE_BUDGET, DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink,
    ImportOptions, NetworkSession, NoticeCode, SessionOptions, SpvState, StartupStatus, SyncLoop,
    WalletId, WatchOnlyOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use zeroize::Zeroizing;

const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASSPHRASE: &[u8] = b"correct horse battery staple";
/// platform-wallet's `DEFAULT_STARTUP_BUDGET`, for a restored wallet.
const RESTORE_BUDGET: Duration = Duration::from_secs(20);
/// The acceptance bound past the budget for SPV's start (ROADMAP E0-05).
const SPV_SLACK: Duration = Duration::from_secs(1);

/// Events with the instant they arrived.
#[derive(Default)]
struct Recorder(Mutex<Vec<(Instant, EngineEvent)>>);

impl EventSink for Recorder {
    fn emit(&self, event: EngineEvent) {
        self.0.lock().unwrap().push((Instant::now(), event));
    }
}

impl Recorder {
    fn events(&self) -> Vec<EngineEvent> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// When SPV reported its start, waiting up to `timeout` for it.
    fn spv_started_at(&self, timeout: Duration) -> Option<Instant> {
        let deadline = Instant::now() + timeout;
        loop {
            let found = self.0.lock().unwrap().iter().find_map(|(at, e)| {
                matches!(e, EngineEvent::SpvStateChanged { running: true, .. }).then_some(*at)
            });
            if found.is_some() || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn notices(&self, code: NoticeCode) -> Vec<String> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                EngineEvent::Notice {
                    code: c, detail, ..
                } if c == code => Some(detail),
                _ => None,
            })
            .collect()
    }
}

/// A TCP listener that never accepts: connections complete in the kernel's
/// backlog and every request waits forever.
struct Blackhole(TcpListener);

impl Blackhole {
    fn new() -> Self {
        Self(TcpListener::bind("127.0.0.1:0").unwrap())
    }

    fn url(&self) -> String {
        format!("http://{}", self.0.local_addr().unwrap())
    }

    /// Regtest with DAPI and the quorum service here, and an SPV peer that
    /// refuses.
    fn options(&self) -> SessionOptions {
        SessionOptions {
            dapi_addresses: vec![self.url()],
            quorum_url: Some(self.url()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        }
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    dapi: Blackhole,
    rec: Arc<Recorder>,
    engine: Engine,
    session: Arc<NetworkSession>,
}

impl Fixture {
    /// A regtest session whose vault is unencrypted (prompt-free) or
    /// encrypted and unlocked.
    fn new(encrypted: bool) -> Self {
        let dir = dw_testutil::private_tempdir();
        let dapi = Blackhole::new();
        let rec = Arc::new(Recorder::default());
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(4),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::clone(&rec) as Arc<dyn EventSink>,
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, dapi.options()))
            .unwrap();
        engine
            .block_on(session.vault_op(move |v| v.create(encrypted.then_some(PASSPHRASE))))
            .unwrap();
        Self {
            _dir: dir,
            dapi,
            rec,
            engine,
            session,
        }
    }

    fn create_wallet(&self) -> WalletId {
        self.engine
            .block_on(self.session.create_wallet(12))
            .unwrap()
            .wallet_id
    }

    fn import(&self, phrase: &str) -> WalletId {
        self.engine
            .block_on(self.session.import_wallet(
                Zeroizing::new(phrase.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions::default(),
            ))
            .unwrap()
    }

    /// Starts SPV and returns how long `start_spv` took and when it was
    /// called.
    fn start_spv(&self) -> (Duration, Instant) {
        let called = Instant::now();
        self.engine.block_on(self.session.start_spv()).unwrap();
        (called.elapsed(), called)
    }

    fn startup(&self, id: &WalletId) -> StartupStatus {
        self.session.dashpay_startup(id).unwrap().startup
    }

    fn wait_startup(&self, id: &WalletId, want: StartupStatus, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.startup(id) != want {
            assert!(
                Instant::now() < deadline,
                "status {:?}, waiting for {want:?}",
                self.startup(id)
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// `start_spv` returns at once; SPV waits for the bring-up, which a
/// blackholed DAPI holds for the wallet's whole budget, and starts within
/// budget + 1 s; then the loops run.
#[test]
fn start_spv_returns_at_once_and_spv_starts_after_the_bring_up() {
    let f = Fixture::new(false);
    let id = f.create_wallet();
    let (took, called) = f.start_spv();
    assert!(took < Duration::from_millis(100), "start_spv took {took:?}");
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Starting);
    assert!(f.session.spv_running().unwrap());
    assert_eq!(f.startup(&id), StartupStatus::Starting);
    // A second start while starting is a no-op.
    f.engine.block_on(f.session.start_spv()).unwrap();

    let started = f
        .rec
        .spv_started_at(CREATED_HERE_BUDGET + SPV_SLACK + Duration::from_secs(2))
        .expect("SPV started");
    let after = started - called;
    eprintln!("created here: start_spv took {took:?}; SPV started {after:?} after the call");
    assert!(
        after >= CREATED_HERE_BUDGET - Duration::from_millis(200),
        "SPV started before the bring-up ended: {after:?}"
    );
    assert!(
        after <= CREATED_HERE_BUDGET + SPV_SLACK,
        "SPV started {after:?} after start_spv"
    );
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Running);
    let startup = f.session.dashpay_startup(&id).unwrap();
    assert_eq!(startup.startup, StartupStatus::PartialNoIdentity);
    assert!(!startup.read_only);
    assert!(startup.finished_at.is_some());
    let notices = f.rec.notices(NoticeCode::DashPayStartupIncomplete);
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains(&id.to_string()));

    let deadline = Instant::now() + Duration::from_secs(5);
    while !f
        .session
        .platform_loops()
        .unwrap()
        .iter()
        .all(|l| l.running)
    {
        assert!(Instant::now() < deadline, "the loops did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let loops: Vec<_> = f
        .session
        .platform_loops()
        .unwrap()
        .into_iter()
        .map(|l| l.sync_loop)
        .collect();
    assert_eq!(
        loops,
        [
            SyncLoop::IdentitySync,
            SyncLoop::DashPaySync,
            SyncLoop::DpnsSync
        ]
    );
    f.session.dashpay_sync_soon().unwrap();

    f.engine.block_on(f.session.stop_spv()).unwrap();
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Stopped);
    assert!(
        f.session
            .platform_loops()
            .unwrap()
            .iter()
            .all(|l| !l.running)
    );
    f.engine.block_on(f.engine.shutdown()).unwrap();
    assert!(f.rec.notices(NoticeCode::UncleanShutdown).is_empty());
}

/// A restored wallet gets the library's 20 s budget, and SPV still starts
/// within budget + 1 s.
#[test]
fn a_restored_wallet_gets_the_full_budget() {
    let f = Fixture::new(false);
    let id = f.import(ABANDON_12);
    let (took, called) = f.start_spv();
    assert!(took < Duration::from_millis(100), "start_spv took {took:?}");
    let started = f
        .rec
        .spv_started_at(RESTORE_BUDGET + SPV_SLACK + Duration::from_secs(5))
        .expect("SPV started");
    let after = started - called;
    eprintln!("restored: start_spv took {took:?}; SPV started {after:?} after the call");
    assert!(
        after >= RESTORE_BUDGET - Duration::from_millis(200),
        "{after:?}"
    );
    assert!(after <= RESTORE_BUDGET + SPV_SLACK, "{after:?}");
    assert_eq!(f.startup(&id), StartupStatus::PartialNoIdentity);
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// Closing the network during the bring-up cancels it, never starts SPV and
/// leaves a clean shutdown: no `UncleanShutdown` notice, the open-session
/// marker removed, and the databases free to open again.
#[test]
fn closing_during_the_bring_up_is_clean() {
    let f = Fixture::new(false);
    let id = f.import(ABANDON_12);
    f.start_spv();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(f.startup(&id), StartupStatus::Starting);

    let closing = Instant::now();
    assert!(
        f.engine
            .block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let took = closing.elapsed();
    eprintln!("close during the bring-up took {took:?}");
    assert!(took < Duration::from_secs(5), "close took {took:?}");
    let events = f.rec.events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, EngineEvent::SpvStateChanged { running: true, .. })),
        "SPV started after close: {events:?}"
    );
    assert!(f.rec.notices(NoticeCode::UncleanShutdown).is_empty());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::SessionClosed { .. }))
    );
    assert!(matches!(
        f.session.spv_state(),
        Err(EngineError::NetworkNotOpen(_))
    ));
    let marker = f
        .engine
        .network_dir(&DashNetwork::Regtest)
        .join(".session-open");
    assert!(!marker.exists(), "the close was not clean");

    // Everything was released: the network opens again in this process.
    let again = f
        .engine
        .block_on(
            f.engine
                .open_network(DashNetwork::Regtest, f.dapi.options()),
        )
        .unwrap();
    assert_eq!(
        again.dashpay_startup(&id).unwrap().startup,
        StartupStatus::NotRun
    );
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// `stop_spv` during the bring-up cancels it at once; a later start runs it
/// again.
#[test]
fn stop_spv_cancels_the_bring_up() {
    let f = Fixture::new(false);
    let id = f.import(ABANDON_12);
    f.start_spv();
    std::thread::sleep(Duration::from_millis(300));
    // Chain data cannot be reset while SPV is starting.
    assert!(matches!(
        f.engine.block_on(f.session.reset_chain_data()),
        Err(EngineError::SpvRunning)
    ));
    let stopping = Instant::now();
    f.engine.block_on(f.session.stop_spv()).unwrap();
    assert!(
        stopping.elapsed() < Duration::from_secs(3),
        "{:?}",
        stopping.elapsed()
    );
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Stopped);
    assert_eq!(f.startup(&id), StartupStatus::NotRun);
    assert!(f.rec.spv_started_at(Duration::ZERO).is_none());

    f.start_spv();
    assert_eq!(f.startup(&id), StartupStatus::Starting);
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// With a locked vault the bring-up has no keys: it answers at once with
/// `IdentityUnsettled` and SPV starts without waiting. The first unlock runs
/// it again with keys.
#[test]
fn a_locked_vault_defers_the_bring_up_to_the_unlock() {
    let f = Fixture::new(true);
    let id = f.create_wallet();
    f.session.lock_vault().unwrap();
    let (_, called) = f.start_spv();
    let started = f
        .rec
        .spv_started_at(Duration::from_secs(5))
        .expect("SPV started");
    assert!(
        started - called < Duration::from_secs(1),
        "{:?}",
        started - called
    );
    assert_eq!(f.startup(&id), StartupStatus::IdentityUnsettled);
    // Unsettled until the unlock: not reported as incomplete.
    assert!(
        f.rec
            .notices(NoticeCode::DashPayStartupIncomplete)
            .is_empty()
    );

    f.engine
        .block_on(
            f.session
                .vault_op(|v| v.unlock(PASSPHRASE, UnlockScope::Full)),
        )
        .unwrap();
    f.wait_startup(&id, StartupStatus::Starting, Duration::from_secs(5));
    f.wait_startup(
        &id,
        StartupStatus::PartialNoIdentity,
        CREATED_HERE_BUDGET + Duration::from_secs(3),
    );
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// A lock drops a running bring-up (its scan key is beyond the vault's
/// lock) and SPV starts right away.
#[test]
fn a_lock_drops_a_running_bring_up() {
    let f = Fixture::new(true);
    let id = f.import(ABANDON_12);
    let (_, called) = f.start_spv();
    std::thread::sleep(Duration::from_millis(300));
    // Still running with its keys: without them it would have ended at once.
    assert_eq!(f.startup(&id), StartupStatus::Starting);
    let locked = Instant::now();
    f.session.lock_vault().unwrap();
    let started = f
        .rec
        .spv_started_at(Duration::from_secs(5))
        .expect("SPV started");
    eprintln!("SPV started {:?} after the lock", started - locked);
    assert!(started > locked, "SPV started before the lock");
    assert!(
        started - called < Duration::from_secs(2),
        "SPV waited {:?}",
        started - called
    );
    assert_eq!(f.startup(&id), StartupStatus::IdentityUnsettled);
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// A watch-only wallet is DashPay read-only: no bring-up, nothing to wait
/// for.
#[test]
fn a_watch_only_wallet_is_read_only() {
    let f = Fixture::new(false);
    let secret = dw_vault::mnemonic::derive_secret(ABANDON_12.as_bytes(), b"", false).unwrap();
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let account = master
        .derive_priv(&secp, &DerivationPath::from_str("m/44'/1'/0'").unwrap())
        .unwrap();
    let tpub = ExtendedPubKey::from_priv(&secp, &account).to_string();
    let id = f
        .engine
        .block_on(
            f.session
                .import_watch_only(tpub, WatchOnlyOptions::default()),
        )
        .unwrap();
    let before = f.session.dashpay_startup(&id).unwrap();
    assert!(before.read_only);
    assert_eq!(before.startup, StartupStatus::NotRun);

    let (_, called) = f.start_spv();
    let started = f
        .rec
        .spv_started_at(Duration::from_secs(5))
        .expect("SPV started");
    assert!(started - called < Duration::from_secs(1));
    let after = f.session.dashpay_startup(&id).unwrap();
    assert!(after.read_only);
    assert_eq!(after.startup, StartupStatus::NotRun);
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// A wallet added while SPV runs gets its bring-up without holding SPV.
#[test]
fn a_wallet_added_while_running_is_brought_up() {
    let f = Fixture::new(false);
    f.start_spv();
    f.rec
        .spv_started_at(Duration::from_secs(5))
        .expect("SPV started");
    let id = f.create_wallet();
    f.wait_startup(&id, StartupStatus::Starting, Duration::from_secs(5));
    f.wait_startup(
        &id,
        StartupStatus::PartialNoIdentity,
        CREATED_HERE_BUDGET + Duration::from_secs(3),
    );
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Running);
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// Removing a wallet ends its bring-up (and with it any scan key it
/// resolved): SPV starts at once instead of after the budget.
#[test]
fn removing_a_wallet_ends_its_bring_up() {
    let f = Fixture::new(false);
    let id = f.import(ABANDON_12);
    let (_, called) = f.start_spv();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(f.startup(&id), StartupStatus::Starting);
    let grant = f
        .engine
        .block_on(f.session.vault_op(move |v| {
            v.authorize(
                dw_vault::GrantPurpose::Wipe,
                Some(&id.0),
                dw_vault::Credential::None,
            )
        }))
        .unwrap();
    f.engine
        .block_on(f.session.remove_wallet(id, grant.id))
        .unwrap();
    let started = f
        .rec
        .spv_started_at(Duration::from_secs(5))
        .expect("SPV started");
    assert!(
        started - called < Duration::from_secs(2),
        "SPV waited {:?}",
        started - called
    );
    assert!(matches!(
        f.session.dashpay_startup(&id),
        Err(EngineError::WalletNotFound(_))
    ));
    f.engine.block_on(f.engine.shutdown()).unwrap();
}
