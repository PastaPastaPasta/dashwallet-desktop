//! DP1-05 offline: the main identity and main-name selection, and the pass
//! count of a recovery against a mocked Platform (the two library calls of
//! `recovery.rs`'s [`MockPlatform`]), and the testnet acceptance (ignored,
//! at the end).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_vault::{Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use platform_wallet::DpnsNameInfo;
use platform_wallet::changeset::{DpnsNameSaleStatus, DpnsNameStateEntry};
use platform_wallet::manager::startup::{WalletStartupOutcome, WalletStartupStatus};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use super::bringup::NO_IDENTITY_KEY;
use super::errors::PlatformError;
use super::recovery::{
    BoxedFuture, IdentityChoices, MockPlatform, OwnedIdentity, main_name, owned_names, summaries,
};
use super::{SpvState, StartupStatus};
use crate::session::Manager;
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};

fn names(list: &[(&str, Option<u64>)]) -> Vec<(String, Option<u64>)> {
    list.iter().map(|(l, at)| ((*l).to_owned(), *at)).collect()
}

#[test]
fn the_main_name_is_the_choice_while_owned_else_the_first_acquired() {
    let owned = names(&[("zed", Some(300)), ("alice", Some(100)), ("bob", Some(100))]);
    // Earliest first; a tie keeps the library's order.
    assert_eq!(main_name(&owned, None).as_deref(), Some("alice"));
    assert_eq!(main_name(&owned, Some("zed")).as_deref(), Some("zed"));
    // A choice the identity no longer owns (sold, transferred) is ignored.
    assert_eq!(main_name(&owned, Some("carol")).as_deref(), Some("alice"));
    // Unknown acquisition times come last, in the library's order.
    let unknown = names(&[("x", None), ("y", Some(5)), ("w", None)]);
    assert_eq!(main_name(&unknown, None).as_deref(), Some("y"));
    assert_eq!(
        main_name(&names(&[("x", None), ("w", None)]), None).as_deref(),
        Some("x")
    );
    assert_eq!(main_name(&[], Some("alice")), None);
    // DPNS compares labels homograph-normalized; the label shown is the
    // identity's own.
    let owned = names(&[("Al1ce", Some(300)), ("bob", Some(100))]);
    assert_eq!(main_name(&owned, Some("alice")).as_deref(), Some("Al1ce"));
}

fn row(identity: [u8; 32], label: &str, status: DpnsNameSaleStatus, at: u64) -> DpnsNameStateEntry {
    DpnsNameStateEntry {
        document_id: Identifier::from([at as u8; 32]),
        wallet_identity_id: identity.into(),
        label: label.into(),
        normalized_label: convert_to_homograph_safe_chars(label),
        normalized_parent_domain_name: "dash".into(),
        price: None,
        status,
        created_at_ms: Some(at),
        updated_at_ms: None,
        transferred_at_ms: None,
        last_synced_at_ms: FETCHED_AT,
    }
}

#[test]
fn names_carry_platforms_times_and_drop_once_sold() {
    let listed: Vec<DpnsNameInfo> = ["late", "Early", "sold", "given", "fresh"]
        .into_iter()
        .map(|label| DpnsNameInfo {
            label: label.into(),
            acquired_at: Some(FETCHED_AT),
        })
        .collect();
    let mut transferred = row(
        ALICE,
        "given",
        DpnsNameSaleStatus::Transferred { to: SECOND.into() },
        4,
    );
    transferred.transferred_at_ms = Some(5);
    let rows = BTreeMap::from_iter(
        [
            row(ALICE, "late", DpnsNameSaleStatus::Owned, 300),
            // Matched as DPNS does, normalized.
            row(ALICE, "early", DpnsNameSaleStatus::Owned, 100),
            row(
                ALICE,
                "sold",
                DpnsNameSaleStatus::Sold { to: SECOND.into() },
                50,
            ),
            transferred,
            // Another identity's row says nothing about ours.
            row(SECOND, "fresh", DpnsNameSaleStatus::Owned, 1),
        ]
        .map(|r| (r.document_id, r)),
    );
    let owned = owned_names(ALICE.into(), &listed, &rows);
    assert_eq!(
        owned,
        names(&[
            ("late", Some(300)),
            ("Early", Some(100)),
            ("fresh", Some(FETCHED_AT))
        ])
    );
    assert_eq!(main_name(&owned, None).as_deref(), Some("Early"));
    // A choice of a sold name is no longer the identity's.
    assert_eq!(main_name(&owned, Some("sold")).as_deref(), Some("Early"));
}

fn owned(identity: &str, index: u32, list: &[(&str, Option<u64>)]) -> OwnedIdentity {
    OwnedIdentity {
        identity: identity.into(),
        index,
        names: names(list),
        balance: Some(1),
        has_dashpay_keys: true,
        profile: None,
    }
}

#[test]
fn the_main_identity_is_the_choice_while_held_else_the_lowest_index() {
    let list = || {
        vec![
            owned("second", 1, &[("bob", Some(1))]),
            owned("first", 0, &[]),
        ]
    };
    let shown = |choices: &IdentityChoices| {
        summaries(list(), choices)
            .into_iter()
            .map(|s| (s.identity, s.index, s.is_main, s.main_name))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shown(&IdentityChoices::default()),
        vec![
            ("first".into(), 0, true, None),
            ("second".into(), 1, false, Some("bob".into())),
        ]
    );
    let chosen = IdentityChoices {
        main_identity: Some("second".into()),
        main_names: HashMap::from([("second".into(), "gone".into())]),
    };
    assert_eq!(
        shown(&chosen),
        vec![
            ("first".into(), 0, false, None),
            ("second".into(), 1, true, Some("bob".into())),
        ]
    );
    // A chosen identity that is no longer the wallet's.
    let stale = IdentityChoices {
        main_identity: Some("elsewhere".into()),
        ..IdentityChoices::default()
    };
    assert!(summaries(list(), &stale)[0].is_main);
    assert!(summaries(Vec::new(), &chosen).is_empty());
}

// ---- recovery against a mocked Platform ----

struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _: EngineEvent) {}
}

const ABANDON_12: &[u8] =
    b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

const ALICE: [u8; 32] = [7; 32];
const SECOND: [u8; 32] = [8; 32];
/// The library's stamp on every name one fetch finds.
const FETCHED_AT: u64 = 900;

fn base58(raw: [u8; 32]) -> String {
    Identifier::from(raw).to_string(Encoding::Base58)
}

/// A label and the `$createdAt` Platform holds for it.
type Name = (&'static str, Option<u64>);

/// What the mocked Platform has: identities the bring-up's discovery finds
/// (with no names: its enrichment ran out of budget), the names a later
/// DPNS pass returns, and identities past the bring-up's gap that only an
/// explicit discovery finds. The bring-up and the names pass can be held
/// until the test releases them.
#[derive(Default)]
struct Platform {
    identities: Vec<([u8; 32], u32)>,
    /// Each identity's labels (`None`: no marketplace row yet).
    names: Vec<([u8; 32], Vec<Name>)>,
    /// Identities only `discover_identities` finds.
    discovered: Vec<([u8; 32], u32)>,
    bring_ups: Arc<AtomicU32>,
    hold_bring_up: Option<Arc<Notify>>,
    hold_names: Option<Arc<Notify>>,
}

impl MockPlatform for Platform {
    fn bring_up(
        &self,
        manager: Arc<Manager>,
        id: WalletId,
    ) -> BoxedFuture<Result<WalletStartupOutcome, EngineError>> {
        let (identities, calls, hold) = (
            self.identities.clone(),
            Arc::clone(&self.bring_ups),
            self.hold_bring_up.clone(),
        );
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            calls.fetch_add(1, Ordering::SeqCst);
            add_identities(&manager, id, &identities).await;
            let status = if identities.is_empty() {
                WalletStartupStatus::NoIdentity
            } else {
                WalletStartupStatus::Ready
            };
            Ok(WalletStartupOutcome {
                status,
                identity_id: identities.first().map(|(raw, _)| (*raw).into()),
                discovery_attempts: 1,
                dashpay_sync_ran: true,
                seed_binding_unverified: false,
                identity_scan_incomplete: false,
                contact_accounts_drained: 0,
                contact_accounts_pending: 0,
                elapsed: Duration::ZERO,
            })
        })
    }

    fn names_pass(&self, manager: Arc<Manager>, id: WalletId) -> BoxedFuture<()> {
        let (names, hold) = (self.names.clone(), self.hold_names.clone());
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            let wallet = manager.get_wallet(&id.0).await.expect("wallet");
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm.get_wallet_info_mut(&id.0).expect("wallet info");
            for (raw, list) in names {
                // As the library does: the sweep's rows carry Platform's
                // `$createdAt`, the names its fetch time, the same for all.
                for (label, created_at_ms) in &list {
                    let Some(created_at_ms) = created_at_ms else {
                        continue;
                    };
                    let document_id = Identifier::from([info.dpns_name_states.len() as u8; 32]);
                    info.dpns_name_states.insert(
                        document_id,
                        DpnsNameStateEntry {
                            document_id,
                            wallet_identity_id: raw.into(),
                            label: (*label).into(),
                            normalized_label: convert_to_homograph_safe_chars(label),
                            normalized_parent_domain_name: "dash".into(),
                            price: None,
                            status: DpnsNameSaleStatus::Owned,
                            created_at_ms: Some(*created_at_ms),
                            updated_at_ms: None,
                            transferred_at_ms: None,
                            last_synced_at_ms: FETCHED_AT,
                        },
                    );
                }
                let managed = info
                    .identity_manager
                    .managed_identity_mut(&Identifier::from(raw))
                    .expect("discovered in pass 1");
                let list = list
                    .iter()
                    .map(|(label, _)| DpnsNameInfo {
                        label: (*label).into(),
                        acquired_at: Some(FETCHED_AT),
                    })
                    .collect();
                managed.set_dpns_names(list, wallet.persister());
            }
        })
    }

    fn discover(
        &self,
        manager: Arc<Manager>,
        id: WalletId,
    ) -> BoxedFuture<Result<usize, PlatformError>> {
        let found = self.discovered.clone();
        Box::pin(async move {
            add_identities(&manager, id, &found).await;
            Ok(found.len())
        })
    }
}

async fn add_identities(manager: &Manager, id: WalletId, identities: &[([u8; 32], u32)]) {
    let wallet = manager.get_wallet(&id.0).await.expect("wallet");
    let wm = manager.wallet_manager_arc();
    let mut wm = wm.write().await;
    let info = wm.get_wallet_info_mut(&id.0).expect("wallet info");
    for (raw, index) in identities {
        if info
            .identity_manager
            .managed_identity(&Identifier::from(*raw))
            .is_none()
        {
            let identity = Identity::V0(IdentityV0 {
                id: (*raw).into(),
                public_keys: Default::default(),
                balance: 1_000,
                revision: 0,
            });
            info.identity_manager
                .add_identity(identity, *index, id.0, wallet.persister())
                .unwrap();
        }
    }
}

fn engine(dir: &std::path::Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: dir.join("data"),
            worker_threads: Some(4),
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

/// A regtest session whose Platform is `platform` (DAPI refuses, so the
/// real loops' passes fail and change nothing), with an unencrypted vault.
fn session(dir: &std::path::Path, platform: Arc<Platform>) -> (Engine, Arc<NetworkSession>) {
    let engine = engine(dir);
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
    *super::runtime::guard(&s.platform.recovery.mock) = Some(platform);
    (engine, s)
}

fn restore(engine: &Engine, s: &Arc<NetworkSession>) -> WalletId {
    engine
        .block_on(s.import_wallet(
            Zeroizing::new(ABANDON_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn start(engine: &Engine, s: &Arc<NetworkSession>) {
    engine.block_on(s.start_spv()).unwrap();
    wait_until("SPV", || s.spv_state().unwrap() == SpvState::Running);
}

/// (identity, names, main name, is main) as `identities()` shows them.
fn shown(
    s: &Arc<NetworkSession>,
    id: WalletId,
) -> Vec<(String, Vec<String>, Option<String>, bool)> {
    s.dashpay(id)
        .identities()
        .unwrap()
        .into_iter()
        .map(|i| (i.identity, i.names, i.main_name, i.is_main))
        .collect()
}

/// ROADMAP DP1-05: a restore into a fresh datadir shows the identity after
/// the bring-up (pass 1) and its names and main name after the names pass
/// (pass 2), which follows at once; nothing else runs.
#[test]
fn a_restore_recovers_identity_names_and_main_name_in_two_passes() {
    let dir = dw_testutil::private_tempdir();
    let hold_names = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        names: vec![(
            ALICE,
            vec![
                ("tmp-alice-1", Some(300)),
                ("alice", Some(100)),
                ("zed", None),
            ],
        )],
        hold_names: Some(Arc::clone(&hold_names)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    assert!(shown(&s, id).is_empty());

    start(&engine, &s);
    wait_until("pass 1", || !shown(&s, id).is_empty());
    assert_eq!(
        s.dashpay_startup(&id).unwrap().startup,
        StartupStatus::Ready
    );
    // After pass 1: the identity, as main, without its names yet.
    assert_eq!(shown(&s, id), vec![(base58(ALICE), vec![], None, true)]);
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);

    hold_names.notify_one();
    wait_until("pass 2", || !shown(&s, id)[0].1.is_empty());
    let alice = |main: &str| {
        vec![(
            base58(ALICE),
            vec!["tmp-alice-1".into(), "alice".into(), "zed".into()],
            Some(main.to_owned()),
            true,
        )]
    };
    assert_eq!(shown(&s, id), alice("alice"));
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);

    // DP1-03's choice (or a contest's outcome) moves the main name.
    engine
        .block_on(s.set_main_name(id, base58(ALICE), Some("tmp-alice-1".into())))
        .unwrap();
    assert_eq!(shown(&s, id), alice("tmp-alice-1"));
    engine
        .block_on(s.set_main_name(id, base58(ALICE), None))
        .unwrap();
    assert_eq!(shown(&s, id), alice("alice"));

    // No third pass of ours.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// A wallet restored while SPV runs (a `.dwbackup` carrying its main
/// identity, here the row written before its bring-up) recovers the same
/// way, and keeps the restored main identity; `set_main_identity` refuses
/// an identity that is not the wallet's.
#[test]
fn a_wallet_restored_while_spv_runs_keeps_its_restored_main_identity() {
    let dir = dw_testutil::private_tempdir();
    let hold_bring_up = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0), (SECOND, 1)],
        names: vec![
            (ALICE, vec![("alice", Some(1))]),
            (SECOND, vec![("bob", Some(2))]),
        ],
        hold_bring_up: Some(Arc::clone(&hold_bring_up)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    start(&engine, &s);
    let id = restore(&engine, &s);
    let (wallet, second) = (id.to_string(), base58(SECOND));
    engine
        .block_on(s.appdb_op(move |db| db.set_main_identity(&wallet, &second)))
        .unwrap();
    hold_bring_up.notify_one();

    wait_until("pass 2", || {
        let shown = shown(&s, id);
        !shown.is_empty() && shown.iter().all(|(_, names, _, _)| !names.is_empty())
    });
    assert_eq!(
        shown(&s, id),
        vec![
            (
                base58(ALICE),
                vec!["alice".into()],
                Some("alice".into()),
                false
            ),
            (base58(SECOND), vec!["bob".into()], Some("bob".into()), true),
        ]
    );
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);

    let dp = s.dashpay(id);
    engine
        .block_on(dp.set_main_identity(base58(ALICE)))
        .unwrap();
    assert!(shown(&s, id)[0].3);
    let refused = engine
        .block_on(dp.set_main_identity(base58([9; 32])))
        .unwrap_err();
    assert_eq!(refused.code(), "identity.not_found");
    let stored = engine
        .block_on(s.appdb_op(move |db| db.main_identity(&id.to_string())))
        .unwrap();
    assert_eq!(stored, Some(base58(ALICE)));
    engine.block_on(engine.shutdown()).unwrap();
}

/// An ordinary start of a wallet whose identity is on file discovers
/// nothing new: no names pass of ours (`dpns_sync` keeps its names fresh).
#[test]
fn a_start_with_the_identity_on_file_runs_no_names_pass() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    // As if an earlier session had recovered it.
    let manager = s.manager().unwrap();
    engine.block_on(platform.bring_up(manager, id)).unwrap();
    platform.bring_ups.store(0, Ordering::SeqCst);

    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 0);
    engine.block_on(engine.shutdown()).unwrap();
}

/// A grant of `purpose` for the wallet, as the UI's prompt gets it.
fn grant(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId, purpose: GrantPurpose) -> String {
    engine
        .block_on(s.vault_op(move |v| v.authorize(purpose, Some(&id.0), Credential::None)))
        .unwrap()
        .id
}

/// `discover_identities` takes an `IdentityScan` grant only.
#[test]
fn discovery_refuses_anything_but_an_identity_scan_grant() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let id = restore(&engine, &s);
    let dp = s.dashpay(id);
    for refused in [
        "lease-1".to_owned(),
        grant(
            &engine,
            &s,
            id,
            GrantPurpose::PlatformOp {
                max_duffs: 1,
                max_credits: 1,
            },
        ),
    ] {
        let refused = engine
            .block_on(dp.discover_identities(refused))
            .unwrap_err();
        assert_eq!(refused.code(), "platform.grant_invalid");
    }
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP6-01's "find": `discover_identities` forgets that Platform proved the
/// wallet has no identity, and what it finds gets the rest of a recovery
/// from the supervisor (a bring-up, then the names pass).
#[test]
fn discovery_forgets_the_proven_absence_and_recovers_what_it_finds() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        names: vec![(ALICE, vec![("alice", Some(1))])],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::NoIdentity
    });
    let scope = dw_appdb::local_scope(&id.to_string());
    let marker = || {
        let scope = scope.clone();
        engine
            .block_on(s.appdb_op(move |db| db.setting(&scope, NO_IDENTITY_KEY)))
            .unwrap()
    };
    assert!(marker().is_some());

    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let found = engine
        .block_on(s.dashpay(id).discover_identities(scan))
        .unwrap();
    assert_eq!(found, 1);
    assert!(marker().is_none());
    wait_until("the names pass", || {
        shown(&s, id)
            == vec![(
                base58(ALICE),
                vec!["alice".into()],
                Some("alice".into()),
                true,
            )]
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// ROADMAP DP1-05 on public testnet. `DW_DP105_TESTNET_FIXTURE` names the
/// fixture directory: `A.mnemonic`, `B.mnemonic` and `fixture.json` with
/// each party's identity, names and expected main name, and B's incoming
/// contact payment from A (`/work/scratch/dw-e005/testnet-fixture`).
///
/// B is restored before SPV starts (the startup path: its contact accounts
/// exist before the first filter scan); A while SPV runs, where `dpns_sync`
/// has had its first pass and waits 10 minutes, so A's names can only come
/// from the bring-up (pass 1) and the names pass after it (pass 2).
#[test]
#[ignore = "public testnet; set DW_DP105_TESTNET_FIXTURE"]
fn the_testnet_fixture_restores_identities_names_and_contact_payments() {
    use platform_wallet::wallet::identity::PaymentDirection;

    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP105_TESTNET_FIXTURE").expect("DW_DP105_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let party = |w: &str| {
        let names: Vec<String> = meta[w]["names"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["label"].as_str().unwrap().to_owned())
            .collect();
        (
            meta[w]["identity_id"].as_str().unwrap().to_owned(),
            names,
            meta[w]["expected_main_name"].as_str().unwrap().to_owned(),
        )
    };

    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let import = |w: &str| {
        let phrase = std::fs::read_to_string(fixture.join(format!("{w}.mnemonic"))).unwrap();
        engine
            .block_on(s.import_wallet(
                Zeroizing::new(phrase.trim().as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(birth),
                    ..ImportOptions::default()
                },
            ))
            .unwrap()
    };

    let t0 = Instant::now();
    let b = import("B");
    engine.block_on(s.start_spv()).unwrap();
    wait_for("SPV", Duration::from_secs(60), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let a = import("A");

    for (w, id) in [("B", b), ("A", a)] {
        let (identity, names, main) = party(w);
        let mut sorted = names.clone();
        sorted.sort();
        // The bring-up's enrichment may show the names first; the main name
        // settles once the names pass has Platform's timestamps.
        wait_for(&format!("{w}'s names"), Duration::from_secs(90), || {
            let shown = s.dashpay(id).identities().unwrap();
            let Some(first) = shown.first() else {
                return false;
            };
            let mut got = first.names.clone();
            got.sort();
            got == sorted && first.main_name.as_deref() == Some(main.as_str())
        });
        let shown = s.dashpay(id).identities().unwrap();
        eprintln!(
            "{w}: identity, names and main name after {:?}: {:?}",
            t0.elapsed(),
            shown
                .iter()
                .map(|i| (&i.identity, &i.names, &i.main_name, i.is_main))
                .collect::<Vec<_>>()
        );
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].identity, identity);
        assert_eq!(shown[0].main_name.as_deref(), Some(main.as_str()));
        assert!(shown[0].is_main);
        assert!(shown[0].has_dashpay_keys);
        assert_eq!(
            s.dashpay_startup(&id).unwrap().startup,
            StartupStatus::Ready
        );
    }
    // One names pass per wallet: each recovered in two passes.
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 2);

    // After SPV: B's contact payment history.
    let payment = &meta["contact_payments"][0];
    let txid = payment["txid"].as_str().unwrap().to_owned();
    let a_id = Identifier::from_string(&party("A").0, Encoding::Base58).unwrap();
    let b_id = Identifier::from_string(&party("B").0, Encoding::Base58).unwrap();
    let manager = s.manager().unwrap();
    let entry = || {
        let wm = manager.wallet_manager_arc();
        let wm = engine.block_on(async move { wm.read_owned().await });
        wm.get_wallet_info(&b.0)
            .and_then(|info| info.identity_manager.managed_identity(&b_id))
            .and_then(|m| m.dashpay().payments.get(&txid).cloned())
    };
    wait_for("B's contact payment", Duration::from_secs(1800), || {
        entry().is_some()
    });
    let entry = entry().unwrap();
    eprintln!("B's contact payment after {:?}: {entry:?}", t0.elapsed());
    assert_eq!(entry.counterparty_id, a_id);
    assert_eq!(entry.direction, PaymentDirection::Received);
    assert_eq!(entry.amount_duffs, payment["duffs"].as_u64().unwrap());
    engine.block_on(engine.shutdown()).unwrap();
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(250));
    }
}
