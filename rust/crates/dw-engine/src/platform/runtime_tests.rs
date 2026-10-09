//! Bring-up internals: the status mapping, the local markers and the
//! cadence. The end-to-end behaviour is in `tests/e0_05_bringup.rs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_vault::{KdfParams, KdfPolicy, LockState, MemoryOsStore, VaultConfig};
use platform_wallet::manager::startup::WalletStartupStatus;
use zeroize::Zeroizing;

use super::bringup::{CREATED_HERE_KEY, NO_IDENTITY_KEY};
use super::runtime::prompt_free;
use super::{PlatformCadence, SpvState, StartupStatus};
use crate::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, NetworkSession,
    SessionOptions, WalletId,
};

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
}

const ABANDON_12: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

fn engine(dir: &std::path::Path, worker_threads: usize) -> Engine {
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
        Arc::new(NoEvents),
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
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(s.appdb_op(move |db| db.set_setting(&scope, NO_IDENTITY_KEY, Some("1"))))
        .unwrap();
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
