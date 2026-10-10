//! E0-05: the engine-owned bring-up (DASHPAY §2.5, §3.2). Offline: regtest
//! with SPV peers that refuse, and DAPI plus the quorum service on a local
//! listener that accepts connections and never answers, so every Platform
//! request hangs until the bring-up's budget cuts it ("DAPI blackholed").

use std::net::TcpListener;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use dw_engine::{
    CREATED_HERE_BUDGET, DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink,
    ImportOptions, NetworkSession, NoticeCode, SessionOptions, SpvState, StartupStatus, SyncLoop,
    WalletId, WatchOnlyOptions,
};
use dw_vault::{
    KdfParams, KdfPolicy, MemoryOsStore, OsSecretStore, UnlockScope, VaultConfig, VaultError,
};
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
        Self::with_store(encrypted, Arc::new(MemoryOsStore::new()))
    }

    /// As [`Self::new`], with `os_store` as the OS secret store.
    fn with_store(encrypted: bool, os_store: Arc<dyn OsSecretStore>) -> Self {
        let dir = dw_testutil::private_tempdir();
        let dapi = Blackhole::new();
        let rec = Arc::new(Recorder::default());
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(4),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store,
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

    // With DAPI blackholed a loop's first pass can still be waiting on it:
    // stop then says so (review r1 M1) and stops SPV anyway.
    let stopped = f.engine.block_on(f.session.stop_spv());
    assert!(
        matches!(&stopped, Ok(()) | Err(EngineError::Sdk(_))),
        "{stopped:?}"
    );
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
    f.session.lock_vault_sync().unwrap();
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
    f.session.lock_vault_sync().unwrap();
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
    let master = ExtendedPrivKey::new_master(dashcore::Network::Regtest, &secret.seed[..]).unwrap();
    let account = master
        .derive_priv(&DerivationPath::from_str("m/44'/1'/0'").unwrap())
        .unwrap();
    let tpub = ExtendedPubKey::from_priv(&account).to_string();
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

/// An OS secret store whose reads wait at a barrier while `hold` is set
/// (the technique of review DW-E0-05-r1-gpt's probe): a keyring read that
/// is slow or waits for the user.
#[derive(Default)]
struct BarrierStore {
    memory: MemoryOsStore,
    hold: AtomicBool,
    entered: AtomicBool,
    completed: AtomicBool,
    released: Mutex<bool>,
    wake: Condvar,
}

impl OsSecretStore for BarrierStore {
    fn put(&self, service: &[u8; 32], label: &str, secret: &[u8]) -> Result<(), VaultError> {
        self.memory.put(service, label, secret)
    }

    fn get(
        &self,
        service: &[u8; 32],
        label: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        if self.hold.load(Ordering::SeqCst) {
            self.entered.store(true, Ordering::SeqCst);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.wake.wait(released).unwrap();
            }
            self.completed.store(true, Ordering::SeqCst);
        }
        self.memory.get(service, label)
    }

    fn delete(&self, service: &[u8; 32], label: &str) -> Result<bool, VaultError> {
        self.memory.delete(service, label)
    }

    fn name(&self) -> &'static str {
        "test-barrier"
    }
}

impl BarrierStore {
    fn wait_entered(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !self.entered.load(Ordering::SeqCst) {
            assert!(
                Instant::now() < deadline,
                "the bring-up never read the OS store"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}

/// An unencrypted vault whose data key must be read from the OS store again
/// (`lock_vault` dropped the cached one), with that read held: the bring-up
/// of a wallet created here is waiting in its key acquisition once SPV is
/// starting. No automatic backups, which read the store too.
fn held_key_read() -> (Fixture, Arc<BarrierStore>, WalletId) {
    let store = Arc::new(BarrierStore::default());
    let f = Fixture::with_store(false, Arc::clone(&store) as Arc<dyn OsSecretStore>);
    f.engine.block_on(f.session.set_backup_policy(0)).unwrap();
    let id = f.create_wallet();
    f.session.lock_vault_sync().unwrap();
    store.hold.store(true, Ordering::SeqCst);
    (f, store, id)
}

/// Review DW-E0-05-r1-gpt M2: a task abort cannot stop a `spawn_blocking`
/// key read, so close must wait for it; before the fix, close returned in
/// about 4 ms and removed the open-session marker while the read was still
/// paused.
#[test]
fn close_waits_for_a_bring_up_key_read() {
    let (f, store, _) = held_key_read();
    f.start_spv();
    store.wait_entered();
    let engine = Arc::new(f.engine);
    let closing = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap();
        })
    };
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !closing.is_finished(),
        "close returned while the bring-up's key read was still running"
    );
    store.release();
    closing.join().unwrap();
    assert!(store.completed.load(Ordering::SeqCst));
    assert!(f.rec.notices(NoticeCode::UncleanShutdown).is_empty());
    let marker = engine
        .network_dir(&DashNetwork::Regtest)
        .join(".session-open");
    assert!(!marker.exists(), "the close was not clean");
    engine.block_on(engine.shutdown()).unwrap();
}

/// M2 for `stop_spv`: it returns only once the key read has ended.
#[test]
fn stop_spv_waits_for_a_bring_up_key_read() {
    let (f, store, _) = held_key_read();
    f.start_spv();
    store.wait_entered();
    let session = Arc::clone(&f.session);
    let engine = Arc::new(f.engine);
    let stopping = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.block_on(session.stop_spv()))
    };
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !stopping.is_finished(),
        "stop_spv returned while the bring-up's key read was still running"
    );
    store.release();
    stopping.join().unwrap().unwrap();
    assert!(store.completed.load(Ordering::SeqCst));
    assert_eq!(f.session.spv_state().unwrap(), SpvState::Stopped);
    engine.block_on(engine.shutdown()).unwrap();
}

/// M2, bounded: a key read that never ends cannot hold close forever.
/// Close stops waiting after `KEY_WORK_JOIN` (5 s) and reports itself
/// unclean, so the open-session marker stays.
#[test]
fn close_reports_a_key_read_it_could_not_wait_for() {
    let (f, store, _) = held_key_read();
    f.start_spv();
    store.wait_entered();
    let closing = Instant::now();
    assert!(
        f.engine
            .block_on(f.engine.close_network(DashNetwork::Regtest))
            .unwrap()
    );
    let took = closing.elapsed();
    eprintln!("close with a stuck key read took {took:?}");
    assert!(took >= Duration::from_secs(4), "{took:?}");
    assert!(took < Duration::from_secs(8), "{took:?}");
    assert!(!store.completed.load(Ordering::SeqCst));
    assert_eq!(f.rec.notices(NoticeCode::UncleanShutdown).len(), 1);
    let marker = f
        .engine
        .network_dir(&DashNetwork::Regtest)
        .join(".session-open");
    assert!(marker.exists(), "an unclean close removed the marker");
    store.release();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !store.completed.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    f.engine.block_on(f.engine.shutdown()).unwrap();
}

/// Review DW-E0-05-r1-gpt M3: the budget bounds the whole bring-up, key
/// acquisition included. With the key read held, SPV still starts within the
/// created-here budget + 1 s; before the fix it stayed `Starting` for as long
/// as the read was held.
#[test]
fn a_held_key_read_does_not_hold_spv_past_the_budget() {
    let (f, store, id) = held_key_read();
    let (_, called) = f.start_spv();
    store.wait_entered();
    let started = f
        .rec
        .spv_started_at(CREATED_HERE_BUDGET + SPV_SLACK + Duration::from_secs(2));
    assert!(
        !store.completed.load(Ordering::SeqCst),
        "the read was released early"
    );
    let started = started.expect("SPV did not start while the key read was held");
    let after = started - called;
    eprintln!("held key read: SPV started {after:?} after the call");
    assert!(after <= CREATED_HERE_BUDGET + SPV_SLACK, "{after:?}");
    assert_eq!(f.startup(&id), StartupStatus::PartialNoIdentity);
    store.release();
    f.engine.block_on(f.engine.shutdown()).unwrap();
    assert!(f.rec.notices(NoticeCode::UncleanShutdown).is_empty());
}
