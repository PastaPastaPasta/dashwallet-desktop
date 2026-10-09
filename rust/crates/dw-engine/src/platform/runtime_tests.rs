//! Bring-up internals: the status mapping, the local markers and the
//! cadence. The end-to-end behaviour is in `tests/e0_05_bringup.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_vault::{KdfParams, KdfPolicy, LockState, MemoryOsStore, VaultConfig};
use platform_wallet::manager::startup::WalletStartupStatus;
use zeroize::Zeroizing;

use super::bringup::{CREATED_HERE_KEY, NO_IDENTITY_KEY};
use super::runtime::{TestPause, guard, prompt_free};
use super::{PlatformCadence, SpvState, StartupStatus};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};

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
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                quorum_url: Some(urls[0].clone()),
                dapi_addresses: urls,
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    engine
        .block_on(s.vault_op(move |v| v.create(encrypted.then_some(PASSPHRASE))))
        .unwrap();
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

/// Review DW-E0-05-r1-gpt M1: a loop pass that outlives the drain deadline
/// makes `stop_spv` fail visibly (SPV is stopped anyway), and the next
/// `start_spv` does not wait for that pass: the drain is retried inside the
/// start task. Before the fix the stop returned `Ok`, and the start waited
/// another full drain budget (10 s).
#[test]
fn a_stop_that_cannot_drain_a_pass_says_so_and_the_next_start_is_immediate() {
    let dir = dw_testutil::private_tempdir();
    let hole = blackhole();
    let (engine, s) = session_on(dir.path(), &hole, false);
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
    let stopped = engine.block_on(s.stop_spv());
    eprintln!(
        "stop with a stuck pass took {:?}: {stopped:?}",
        stopping.elapsed()
    );
    assert!(
        matches!(&stopped, Err(EngineError::Sdk(detail)) if detail.contains("dashpay_sync")),
        "{stopped:?}"
    );
    assert_eq!(s.spv_state().unwrap(), SpvState::Stopped);

    let starting = Instant::now();
    engine.block_on(s.start_spv()).unwrap();
    let took = starting.elapsed();
    eprintln!("the next start_spv took {took:?}");
    assert!(took < Duration::from_millis(100), "{took:?}");
    assert_eq!(s.spv_state().unwrap(), SpvState::Starting);
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
        s.lock_vault().unwrap();
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
    let admitted = |s: &NetworkSession| {
        s.platform.startup_of(&a).is_some() || guard(&s.platform.tasks).contains_key(&a)
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
/// assertions turned to the fixed behaviour): R1 = [a] commits while R2 =
/// [b, a] still marks `a`, so R1's bring-up signal is refused; when R2 fails
/// and its last mark clears, `a` (committed, surviving) must be brought up,
/// and `b` (rolled back) must not. Before the fix `a` stayed without a
/// bring-up until the next start.
#[test]
fn overlapping_restores_keep_a_committed_wallets_bring_up() {
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
    hold.b_hold.wait_held();
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV running", Duration::from_secs(5), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    assert!(s.platform.startup_of(&a).is_none());

    hold.a_hold.let_go();
    assert_eq!(r1.join().unwrap().unwrap(), vec![a]);
    // A sentinel queued after R1's signal: once it is recorded, R1's signal
    // was handled, and refused, since R2 still marks `a`.
    let sentinel = WalletId([93; 32]);
    s.platform
        .signal(super::runtime::PlatformSignal::WalletAdded(sentinel));
    wait_until("R1's signal handled", Duration::from_secs(5), || {
        s.platform.startup_of(&sentinel).is_some()
    });
    assert!(s.platform.startup_of(&a).is_none());

    hold.b_hold.let_go();
    assert!(r2.join().unwrap().is_err());
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
