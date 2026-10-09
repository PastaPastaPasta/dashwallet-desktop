//! DP1-05 offline: the main identity and main-name selection, and the pass
//! count of a recovery against a mocked Platform (the two library calls of
//! `recovery.rs`'s [`MockPlatform`]), and the testnet acceptance (ignored,
//! at the end).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig,
};
use platform_wallet::DpnsNameInfo;
use platform_wallet::changeset::{DpnsNameSaleStatus, DpnsNameStateEntry};
use platform_wallet::manager::startup::{WalletStartupOutcome, WalletStartupStatus};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use super::bringup::NO_IDENTITY_KEY;
use super::errors::PlatformError;
use super::names::{MainNamePrefs, resolve_main_name};
use super::recovery::{
    BoxedFuture, IdentityChoices, MockPlatform, OwnedIdentity, Pause, owned_names, summaries,
};
use super::runtime::guard;
use super::{DashPay, IdentitySummary, SpvState, StartupStatus};
use crate::session::Manager;
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    NetworkSession, SessionOptions, WalletId,
};

fn names(list: &[(&str, Option<u64>)]) -> Vec<(String, Option<u64>)> {
    list.iter().map(|(l, at)| ((*l).to_owned(), *at)).collect()
}

/// DP1-03's rule with only a pick: DP1-05's cases.
fn main_name(names: &[(String, Option<u64>)], preferred: Option<&str>) -> Option<String> {
    let prefs = MainNamePrefs {
        pick: preferred.map(str::to_owned),
        ..MainNamePrefs::default()
    };
    resolve_main_name(names, &[], &prefs)
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
        open_contests: Vec::new(),
        row_owned: Vec::new(),
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
        summaries(list(), choices, true, |_, _| false)
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
        names: HashMap::from([(
            "second".into(),
            MainNamePrefs {
                pick: Some("gone".into()),
                ..MainNamePrefs::default()
            },
        )]),
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
    assert!(summaries(list(), &stale, true, |_, _| false)[0].is_main);
    assert!(summaries(Vec::new(), &chosen, true, |_, _| true).is_empty());
}

/// Review DP1-03 R5: `identities()` shows what `DashPay::main_name` does. A
/// label whose write may be in flight is no name unless a marketplace row
/// says so, a label in a contest is none yet, and while that contest is
/// open the temporary name is the main name.
#[test]
fn identities_show_only_names_platform_shows_owned() {
    let mut alice = owned(
        "alice",
        0,
        &[("carol", Some(100)), ("pend", None), ("bob", Some(200))],
    );
    let choices = |prefs: MainNamePrefs| IdentityChoices {
        names: HashMap::from([("alice".into(), prefs)]),
        ..IdentityChoices::default()
    };
    let pending = MainNamePrefs {
        pick: Some("pend".into()),
        pending: vec!["pend".into()],
        ..MainNamePrefs::default()
    };
    let shown = |o: &OwnedIdentity, c: &IdentityChoices| {
        let s = summaries(vec![o.clone()], c, true, |_, _| false).remove(0);
        (s.names, s.main_name)
    };
    assert_eq!(
        shown(&alice, &choices(pending.clone())),
        (vec!["carol".into(), "bob".into()], Some("carol".into()))
    );
    alice.row_owned = vec!["pend".into()];
    assert_eq!(
        shown(&alice, &choices(pending)),
        (
            vec!["carol".into(), "pend".into(), "bob".into()],
            Some("pend".into())
        )
    );

    alice.open_contests = vec!["carol".into()];
    let temporary = MainNamePrefs {
        temporary: Some("bob".into()),
        ..MainNamePrefs::default()
    };
    assert_eq!(
        shown(&alice, &choices(temporary)),
        (vec!["pend".into(), "bob".into()], Some("bob".into()))
    );
}

/// Review DP1-03 r5 (DEC-138): the older snapshot shows no names and no
/// main name, whatever it holds that looks like evidence (a marketplace
/// row, the stored pick), and says they are updating; the identity's other
/// fields still show. The current list shows the names.
#[test]
fn an_older_snapshot_shows_no_names() {
    let mut alice = owned("alice", 0, &[("carol", Some(100)), ("dash", Some(50))]);
    alice.row_owned = vec![convert_to_homograph_safe_chars("carol")];
    let choices = IdentityChoices {
        names: HashMap::from([(
            "alice".into(),
            MainNamePrefs {
                pick: Some("carol".into()),
                ..MainNamePrefs::default()
            },
        )]),
        ..IdentityChoices::default()
    };
    let shown = |current| summaries(vec![alice.clone()], &choices, current, |_, _| false).remove(0);
    let live = shown(true);
    assert_eq!(live.names, ["carol", "dash"]);
    assert_eq!(live.main_name.as_deref(), Some("carol"));
    assert!(!live.names_updating);
    let old = shown(false);
    assert!(old.names.is_empty());
    assert_eq!(old.main_name, None);
    assert!(old.names_updating);
    assert_eq!(
        (old.identity, old.index, old.balance, old.is_main),
        (live.identity, live.index, live.balance, live.is_main)
    );
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
/// until the test releases them, and the discovery once it has stored what
/// it found (the library's DPNS enrichment comes after that).
#[derive(Default)]
struct Platform {
    identities: Vec<([u8; 32], u32)>,
    /// Each identity's labels (`None`: no marketplace row yet).
    names: Vec<([u8; 32], Vec<Name>)>,
    /// Identities only `discover_identities` finds.
    discovered: Vec<([u8; 32], u32)>,
    /// Labels a later bring-up (not the first) finds for an identity.
    replayed_names: Vec<([u8; 32], &'static str)>,
    bring_ups: Arc<AtomicU32>,
    hold_bring_up: Option<Arc<Notify>>,
    hold_names: Option<Arc<Notify>>,
    hold_discovery: Option<Arc<Notify>>,
    /// Holds the discovery before it stores anything (a network response
    /// still awaited).
    hold_before_fold: Option<Arc<Notify>>,
    /// The discovery has started.
    discovery_started: Arc<AtomicBool>,
    /// The discovery has stored its identities (and is held, if held).
    discovery_stored: Arc<AtomicBool>,
    /// The discovery's future has ended or been dropped.
    discovery_ended: Arc<AtomicBool>,
}

/// Sets its flag when dropped.
struct Flag(Arc<AtomicBool>);

impl Drop for Flag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl MockPlatform for Platform {
    fn bring_up(
        &self,
        manager: Arc<Manager>,
        id: WalletId,
    ) -> BoxedFuture<Result<WalletStartupOutcome, EngineError>> {
        let (identities, calls, hold, replayed) = (
            self.identities.clone(),
            Arc::clone(&self.bring_ups),
            self.hold_bring_up.clone(),
            self.replayed_names.clone(),
        );
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            if calls.fetch_add(1, Ordering::SeqCst) > 0 {
                set_names(&manager, id, &replayed).await;
            }
            // As the library: the identities on file, an earlier explicit
            // discovery's included, and those its discovery finds (review r1
            // N1: not a fabricated absence).
            let on_file = add_identities(&manager, id, &identities).await;
            let status = if on_file.is_empty() {
                WalletStartupStatus::NoIdentity
            } else {
                WalletStartupStatus::Ready
            };
            Ok(WalletStartupOutcome {
                status,
                identity_id: on_file.first().copied(),
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
        let (found, hold, before_fold) = (
            self.discovered.clone(),
            self.hold_discovery.clone(),
            self.hold_before_fold.clone(),
        );
        let (started, stored, ended) = (
            Arc::clone(&self.discovery_started),
            Arc::clone(&self.discovery_stored),
            Flag(Arc::clone(&self.discovery_ended)),
        );
        Box::pin(async move {
            let _ended = ended;
            started.store(true, Ordering::SeqCst);
            if let Some(hold) = before_fold {
                hold.notified().await;
            }
            // By wallet id, as the library folds what it found.
            add_identities(&manager, id, &found).await;
            stored.store(true, Ordering::SeqCst);
            if let Some(hold) = hold {
                hold.notified().await;
            }
            Ok(found.len())
        })
    }
}

/// Gives each identity of `names` its label, stamped with the fetch time.
async fn set_names(manager: &Manager, id: WalletId, names: &[([u8; 32], &'static str)]) {
    let wallet = manager.get_wallet(&id.0).await.expect("wallet");
    let wm = manager.wallet_manager_arc();
    let mut wm = wm.write().await;
    let info = wm.get_wallet_info_mut(&id.0).expect("wallet info");
    for (raw, label) in names {
        let names = vec![DpnsNameInfo {
            label: (*label).into(),
            acquired_at: Some(FETCHED_AT),
        }];
        info.identity_manager
            .managed_identity_mut(&Identifier::from(*raw))
            .expect("on file")
            .set_dpns_names(names, wallet.persister());
    }
}

/// Adds `identities` the wallet lacks; the identities it then has.
async fn add_identities(
    manager: &Manager,
    id: WalletId,
    identities: &[([u8; 32], u32)],
) -> Vec<Identifier> {
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
    info.identity_manager.wallet_identity_ids(&id.0)
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
    session_with(dir, platform, None)
}

const PASSPHRASE: &[u8] = b"recovery tests vault passphrase";

/// [`session`] with a vault encrypted with `passphrase` (left unlocked).
fn session_with(
    dir: &std::path::Path,
    platform: Arc<Platform>,
    passphrase: Option<&'static [u8]>,
) -> (Engine, Arc<NetworkSession>) {
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
    engine
        .block_on(s.vault_op(move |v| v.create(passphrase)))
        .unwrap();
    *guard(&s.platform.recovery.mock) = Some(platform);
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

/// `identities()` from the live read. A read while a sync pass holds the
/// wallet manager comes from the older snapshot, which shows no names and
/// says so with `names_updating` (DEC-138); this reads again until it is
/// not, so a test comparing names never compares the snapshot's.
pub(super) fn live_identities(dp: &DashPay) -> Vec<IdentitySummary> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let read = dp.identities().unwrap();
        if !read.iter().any(|i| i.names_updating) {
            return read;
        }
        assert!(
            Instant::now() < deadline,
            "identities() still read the older snapshot (names_updating) after \
             30 s: something held the wallet manager throughout"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits until the live `identities()` shows `want`, comparing the read it
/// waited on.
fn assert_shown(
    s: &Arc<NetworkSession>,
    id: WalletId,
    want: Vec<(String, Vec<String>, Option<String>, bool)>,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut got = shown(s, id);
    while got != want && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
        got = shown(s, id);
    }
    assert_eq!(got, want);
}

/// (identity, names, main name, is main) as the live `identities()` shows
/// them.
fn shown(
    s: &Arc<NetworkSession>,
    id: WalletId,
) -> Vec<(String, Vec<String>, Option<String>, bool)> {
    live_identities(&s.dashpay(id))
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
    // Review r3 N1: the identity can show before pass 1 publishes `Ready`.
    wait_until("pass 1", || {
        !shown(&s, id).is_empty() && s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
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
    assert_shown(&s, id, alice("alice"));
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);

    // DP1-03's choice (or a contest's outcome) moves the main name.
    engine
        .block_on(s.set_main_name(id, base58(ALICE), Some("tmp-alice-1".into())))
        .unwrap();
    assert_shown(&s, id, alice("tmp-alice-1"));
    engine
        .block_on(s.set_main_name(id, base58(ALICE), None))
        .unwrap();
    assert_shown(&s, id, alice("alice"));

    // No third pass of ours.
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// The journal of `id` as (kind, ref, read).
fn journal(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) -> Vec<(String, String, bool)> {
    engine
        .block_on(s.appdb_op(move |db| db.journal(&id.to_string())))
        .unwrap()
        .into_iter()
        .map(|r| (r.kind, r.reference, r.read_at.is_some()))
        .collect()
}

/// Persists the marketplace row of `identity`'s `label`, created on
/// Platform at `created_at_ms`, as the library's sweep does.
fn persist_name_row(
    s: &Arc<NetworkSession>,
    id: WalletId,
    identity: [u8; 32],
    label: &str,
    created_at_ms: u64,
) {
    use platform_wallet::changeset::{
        DpnsNameStateChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
    };
    let document_id = Identifier::from([0xD0; 32]);
    let mut states = DpnsNameStateChangeSet::default();
    states.names.insert(
        document_id,
        DpnsNameStateEntry {
            document_id,
            wallet_identity_id: identity.into(),
            label: label.into(),
            normalized_label: convert_to_homograph_safe_chars(label),
            normalized_parent_domain_name: "dash".into(),
            price: None,
            status: DpnsNameSaleStatus::Owned,
            created_at_ms: Some(created_at_ms),
            updated_at_ms: None,
            transferred_at_ms: None,
            last_synced_at_ms: FETCHED_AT,
        },
    );
    let changeset = PlatformWalletChangeSet {
        dpns_name_states: Some(states),
        ..Default::default()
    };
    s.live().unwrap().store.store(id.0, changeset).unwrap();
}

/// The wallet's catch-up boundary on file (UNIX seconds).
fn boundary(engine: &Engine, s: &Arc<NetworkSession>, id: WalletId) -> Option<u64> {
    let scope = dw_appdb::local_scope(&id.to_string());
    engine
        .block_on(s.appdb_op(move |db| db.setting(&scope, super::journal::CATCH_UP_BEFORE_KEY)))
        .unwrap()
        .map(|at| at.parse().unwrap())
}

/// E0-06 / DEC-125: a restore's bring-up sets the boundary when it starts,
/// and its names pass moves it to its own start. A name Platform dates
/// before that is stored read; one with no marketplace row has no
/// authoritative time and is news (the declared residual).
#[test]
fn a_restores_names_pass_stores_platform_dated_names_read() {
    let dir = dw_testutil::private_tempdir();
    let hold_names = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        names: vec![(ALICE, vec![("alice", Some(100)), ("zed", None)])],
        hold_names: Some(Arc::clone(&hold_names)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("pass 1", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    assert!(boundary(&engine, &s, id).is_some(), "set by the restore");
    persist_name_row(&s, id, ALICE, "alice", 100);

    hold_names.notify_one();
    wait_until("pass 2", || journal(&engine, &s, id).len() == 2);
    let at = |label: &str| format!("{label}@{FETCHED_AT}");
    assert_eq!(
        journal(&engine, &s, id),
        vec![
            ("username_registered".into(), at("alice"), true),
            ("username_registered".into(), at("zed"), false),
        ]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// Sol r2 R2-2's scheduler order (DEC-125): the restore's bring-up is held
/// at Platform while an explicit discovery stores an identity and queues
/// its names pass and bring-up; the held bring-up completes, the names pass
/// runs, and only then the queued bring-up, which stores a name Platform
/// dates after the restore but before the discovery. The discovery moved
/// the boundary forward, so the name is stored read whatever ran in
/// between.
#[test]
fn a_bring_up_a_discovery_queued_behind_a_running_one_stores_history_read() {
    let dir = dw_testutil::private_tempdir();
    let hold_bring_up = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        replayed_names: vec![(ALICE, "between")],
        hold_bring_up: Some(Arc::clone(&hold_bring_up)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    engine.block_on(s.start_spv()).unwrap();
    wait_until("the restore's bring-up", || {
        boundary(&engine, &s, id).is_some()
    });
    let restored_at = boundary(&engine, &s, id).unwrap();
    // Dated after the restore's boundary, and a second before the
    // discovery's.
    let between_ms = (restored_at + 1) * 1_000 + 1;
    while crate::events::unix_now() < restored_at + 3 {
        std::thread::sleep(Duration::from_millis(50));
    }

    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let found = engine
        .block_on(s.dashpay(id).discover_identities(scan))
        .unwrap();
    assert_eq!(found, 1);
    assert!(boundary(&engine, &s, id).unwrap() * 1_000 > between_ms);
    persist_name_row(&s, id, ALICE, "between", between_ms);

    hold_bring_up.notify_one();
    wait_until("the names pass", || {
        s.platform.recovery.names_passes.load(Ordering::SeqCst) == 1
    });
    assert_eq!(
        platform.bring_ups.load(Ordering::SeqCst),
        1,
        "not yet queued"
    );
    hold_bring_up.notify_one();
    wait_until("the queued bring-up", || {
        !journal(&engine, &s, id).is_empty()
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
    assert_eq!(
        journal(&engine, &s, id),
        vec![(
            "username_registered".into(),
            format!("between@{FETCHED_AT}"),
            true
        )]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// Sol r2 R2-1's probe (DEC-125): once the trusted fallback has been used,
/// an identity a discovery finds is unverified although its store was
/// refused (wallet.sqlite held by another writer) and nothing of it is on
/// file; it is verified only once a verified fetch records it.
#[test]
fn a_refused_store_leaves_a_discovered_identity_unverified() {
    let dir = dw_testutil::private_tempdir();
    let response = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        hold_before_fold: Some(Arc::clone(&response)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use();
    let wallet_db = s.data_dir().join(crate::session::WALLET_DB_FILE);
    let stored_identities = || -> i64 {
        rusqlite::Connection::open(&wallet_db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM identities", [], |r| r.get(0))
            .unwrap()
    };

    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let holder = rusqlite::Connection::open(&wallet_db).unwrap();
    let found = std::thread::scope(|scope| {
        let call = scope.spawn(|| engine.block_on(s.dashpay(id).discover_identities(scan)));
        wait_until("the discovery's network wait", || {
            platform.discovery_started.load(Ordering::SeqCst)
        });
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        response.notify_one();
        call.join().unwrap()
    });
    assert_eq!(found.unwrap(), 1);
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(stored_identities(), 0, "the store was refused");
    let unverified = || {
        s.dashpay(id)
            .identities()
            .unwrap()
            .into_iter()
            .map(|i| (i.identity, i.unverified))
            .collect::<Vec<_>>()
    };
    assert_eq!(unverified(), vec![(base58(ALICE), true)]);

    provenance
        .record_verified(super::provenance::kind::IDENTITY, &base58(ALICE))
        .unwrap();
    assert_eq!(unverified(), vec![(base58(ALICE), false)]);
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
    // Review r3 N1: the supervisor may run the names pass before the
    // `Readmit` bring-up, so the names alone do not prove that bring-up
    // published `Ready`. Wait for both.
    // Review r1 N1: the bring-up after it sees the identity on file and
    // does not prove the absence again.
    wait_until("the names pass and the final Ready", || {
        shown(&s, id)
            == vec![(
                base58(ALICE),
                vec!["alice".into()],
                Some("alice".into()),
                true,
            )]
            && s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);
    assert!(marker().is_none());
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review r1 M1: a grant the user confirmed with the passphrase on a locked
/// or mixing-only vault carries its own key, and scans through its hold.
#[test]
fn a_passphrase_grant_discovers_on_a_locked_or_mixing_only_vault() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        ..Platform::default()
    });
    let (engine, s) = session_with(dir.path(), Arc::clone(&platform), Some(PASSPHRASE));
    let id = restore(&engine, &s);
    s.lock_vault().unwrap();
    for scope in [None, Some(UnlockScope::MixingOnly)] {
        if let Some(scope) = scope {
            engine
                .block_on(s.vault_op(move |v| v.unlock(PASSPHRASE, scope)))
                .unwrap();
        }
        let scan = engine
            .block_on(s.vault_op(move |v| {
                v.authorize(
                    GrantPurpose::IdentityScan,
                    Some(&id.0),
                    Credential::Passphrase(PASSPHRASE),
                )
            }))
            .unwrap()
            .id;
        let found = engine.block_on(s.dashpay(id).discover_identities(scan));
        assert_eq!(found.unwrap(), 1, "{scope:?}");
    }
    assert_eq!(shown(&s, id)[0].0, base58(ALICE));
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review r1 M2: the vault locks after the scan stored an identity, during
/// the enrichment that follows. The call is cancelled, but the identity
/// still gets its bring-up and names pass.
#[test]
fn a_lock_after_discovery_stored_an_identity_keeps_its_recovery() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        names: vec![(ALICE, vec![("alice", Some(1))])],
        hold_discovery: Some(Arc::new(Notify::new())),
        ..Platform::default()
    });
    let (engine, s) = session_with(dir.path(), Arc::clone(&platform), Some(PASSPHRASE));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::NoIdentity
    });
    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let ended = std::thread::scope(|scope| {
        let call = scope.spawn(|| engine.block_on(s.dashpay(id).discover_identities(scan)));
        wait_until("the identity stored", || {
            platform.discovery_stored.load(Ordering::SeqCst)
        });
        s.lock_vault().unwrap();
        call.join().unwrap()
    });
    assert_eq!(ended.unwrap_err().code(), "platform.cancelled");
    wait_until("the rest of the recovery", || {
        shown(&s, id)
            == vec![(
                base58(ALICE),
                vec!["alice".into()],
                Some("alice".into()),
                true,
            )]
            && s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review r1 M3: removing or unloading a wallet ends its discovery, whose
/// keys are then gone, before the call returns, and nothing of it applies
/// to the wallet afterwards (loaded again, here).
#[test]
fn removing_or_unloading_a_wallet_ends_its_discovery_first() {
    for remove in [true, false] {
        let dir = dw_testutil::private_tempdir();
        let hold = Arc::new(Notify::new());
        let platform = Arc::new(Platform {
            discovered: vec![(ALICE, 3)],
            hold_discovery: Some(Arc::clone(&hold)),
            ..Platform::default()
        });
        let (engine, s) = session(dir.path(), Arc::clone(&platform));
        let id = restore(&engine, &s);
        let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
        let wipe = grant(&engine, &s, id, GrantPurpose::Wipe);
        let (outlived, ended) = std::thread::scope(|scope| {
            let call = scope.spawn(|| engine.block_on(s.dashpay(id).discover_identities(scan)));
            wait_until("the discovery", || {
                platform.discovery_stored.load(Ordering::SeqCst)
            });
            if remove {
                engine.block_on(s.remove_wallet(id, wipe)).unwrap();
            } else {
                engine.block_on(s.unload_wallet(id)).unwrap();
            }
            let outlived = !platform.discovery_ended.load(Ordering::SeqCst);
            // A discovery still held would keep the scope open.
            hold.notify_one();
            (outlived, call.join().unwrap())
        });
        let what = if remove { "removal" } else { "unload" };
        assert!(!outlived, "the discovery outlived the {what}");
        assert_eq!(ended.unwrap_err().code(), "platform.cancelled", "{what}");
        if !remove {
            engine.block_on(s.load_wallet(id)).unwrap();
        }
        assert!(s.platform.recovery.take_names_due().is_empty());
        engine.block_on(engine.shutdown()).unwrap();
    }
}

/// Review r2 M3-R2 (Sol's reproduction): a restore's rollback keeps
/// discovery admission closed until the wallet is out of the manager and its
/// secret gone. A discovery asked for while the rollback waits on the
/// wallet's lifecycle gate (a payment holds it) is refused, and nothing of
/// it reaches the wallet when the same phrase is imported again.
#[test]
fn a_restore_rollback_admits_no_discovery_until_it_is_done() {
    let dir = dw_testutil::private_tempdir();
    let response = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        hold_before_fold: Some(Arc::clone(&response)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::NoIdentity
    });
    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let manager = s.manager().unwrap();
    let wallet = engine.block_on(manager.get_wallet(&id.0)).unwrap();
    let (late, outlived, replacement) = std::thread::scope(|scope| {
        let restoring = s.platform.restoring(vec![id]);
        let payment = engine.block_on(wallet.generation().payment_guard());
        let rollback = scope.spawn(|| engine.block_on(s.roll_back_restore(&[(id, true)])));
        wait_until("the rollback's forget", || {
            s.platform.startup_of(&id).is_none()
        });
        let call = scope.spawn(|| engine.block_on(s.dashpay(id).discover_identities(scan)));
        wait_until("the discovery's answer or its network wait", || {
            call.is_finished() || platform.discovery_started.load(Ordering::SeqCst)
        });
        drop(payment);
        rollback.join().unwrap();
        drop(restoring);
        assert!(!s.vault.has_wallet_secret(&id.0));
        let outlived = !call.is_finished();
        // The same phrase again; then the old call's response, if it lives.
        assert_eq!(restore(&engine, &s), id);
        response.notify_one();
        let late = call.join().unwrap();
        (late, outlived, shown(&s, id))
    });
    assert!(!outlived, "a discovery outlived the rollback: {late:?}");
    assert_eq!(late.unwrap_err().code(), "wallet_not_found");
    assert_eq!(replacement, vec![]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// Review r1 N2: two `set_main_identity` calls at once leave the cache as
/// the database: the first is held between its write and its cache update
/// while the second runs.
#[test]
fn concurrent_main_identity_writes_leave_the_cache_as_the_database() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0), (SECOND, 1)],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &platform.identities));
    let pause = Arc::new(Pause::default());
    *guard(&s.platform.recovery.pause_after_choice) = Some(Arc::clone(&pause));
    std::thread::scope(|scope| {
        let first = scope.spawn(|| engine.block_on(s.dashpay(id).set_main_identity(base58(ALICE))));
        wait_until("the first write", || pause.reached.load(Ordering::SeqCst));
        let second =
            scope.spawn(|| engine.block_on(s.dashpay(id).set_main_identity(base58(SECOND))));
        // Time for the second to finish, were it not serialized.
        std::thread::sleep(Duration::from_millis(300));
        pause.release.notify_one();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
    });
    let stored = engine
        .block_on(s.appdb_op(move |db| db.main_identity(&id.to_string())))
        .unwrap();
    let main: Vec<String> = shown(&s, id)
        .into_iter()
        .filter(|(_, _, _, main)| *main)
        .map(|(identity, ..)| identity)
        .collect();
    assert_eq!(stored, Some(base58(SECOND)));
    assert_eq!(main, vec![base58(SECOND)]);
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
            let shown = live_identities(&s.dashpay(id));
            let Some(first) = shown.first() else {
                return false;
            };
            let mut got = first.names.clone();
            got.sort();
            got == sorted && first.main_name.as_deref() == Some(main.as_str())
        });
        let shown = live_identities(&s.dashpay(id));
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
