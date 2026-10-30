//! DP1-05 offline: the main identity and main-name selection, and the pass
//! count of a recovery against a mocked Platform (the two library calls of
//! `recovery.rs`'s [`MockPlatform`]), and the testnet acceptance (ignored,
//! at the end).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dpp::identity::Identity;
use dpp::identity::accessors::IdentityGettersV0;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig,
};
use platform_wallet::changeset::{DpnsNameSaleStatus, DpnsNameStateEntry};
use platform_wallet::manager::startup::{WalletStartupOutcome, WalletStartupStatus};
use platform_wallet::wallet::identity::{ContactProfileEntry, DashPayProfile};
use platform_wallet::{ContactRequest, DpnsNameInfo, EstablishedContact};
use tokio::sync::Notify;
use zeroize::Zeroizing;

use super::bringup::NO_IDENTITY_KEY;
use super::contacts::{PrivateDetails, PublishState};
use super::errors::PlatformError;
use super::names::{MainNamePrefs, resolve_main_name};
use super::names_check::NamesChecks;
use super::private_details::InfoSource;
use super::private_details_tests::FakeInfos;
use super::recovery::{
    BoxedFuture, IdentityChoices, MockPlatform, OwnedIdentity, Pause, owned_names, summaries,
};
use super::reverify::{Held, Recorded, ShownProfile, confirmed, held_unverified, record_step};
use super::runtime::{TestPause, guard};
use super::{DashPay, IdentitySummary, NamesCheck, SpvState, StartupStatus};
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
        balance_as_of: None,
        has_dashpay_keys: true,
        profile: None,
        entities: Vec::new(),
        owner: Identifier::new([0; 32]),
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
        summaries(list(), choices, true, |_| false)
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
    assert!(summaries(list(), &stale, true, |_| false)[0].is_main);
    assert!(summaries(Vec::new(), &chosen, true, |_| true).is_empty());
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
        let s = summaries(vec![o.clone()], c, true, |_| false).remove(0);
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
    let shown = |current| summaries(vec![alice.clone()], &choices, current, |_| false).remove(0);
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

/// A second seed (a BIP39 test vector).
const LEGAL_12: &[u8] =
    b"legal winner thank year wave sausage worth useful legal winner thank yellow";

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
    /// (identity, contact) pairs the bring-up's DashPay pass establishes.
    contacts: Vec<([u8; 32], [u8; 32])>,
    /// The first this many bring-ups end `PartialNoIdentity` and find
    /// nothing: their discovery did not answer in the budget.
    partial_bring_ups: u32,
    bring_ups: Arc<AtomicU32>,
    hold_bring_up: Option<Arc<Notify>>,
    hold_names: Option<Arc<Notify>>,
    /// While set, Platform answers nothing: the names pass fetches nothing
    /// and no question about an identity's names is answered.
    fail_names: AtomicBool,
    /// The names pass skips every identity (the library logs a page that
    /// failed and goes on), while Platform still answers questions.
    skip_names: bool,
    /// Holds each question about an identity's names (only those of
    /// `hold_only`'s wallet, once set).
    hold_check: Option<Arc<Notify>>,
    hold_only: Mutex<Option<WalletId>>,
    /// The questions about names asked, per wallet.
    questions: Mutex<HashMap<WalletId, u32>>,
    /// Identities whose questions take `slow_for`, then go unanswered (a
    /// question's budget run out).
    slow: Vec<[u8; 32]>,
    slow_for: Duration,
    /// Identities whose listed labels have no domain document on Platform
    /// (a refused registration, a contest still open): Platform says they
    /// own no name.
    no_documents: Vec<[u8; 32]>,
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
        let (contacts, partial) = (self.contacts.clone(), self.partial_bring_ups);
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            let call = calls.fetch_add(1, Ordering::SeqCst);
            if call < partial {
                return Ok(WalletStartupOutcome {
                    status: WalletStartupStatus::PartialNoIdentity,
                    identity_id: None,
                    discovery_attempts: 1,
                    dashpay_sync_ran: false,
                    seed_binding_unverified: false,
                    identity_scan_incomplete: false,
                    contact_accounts_drained: 0,
                    contact_accounts_pending: 0,
                    elapsed: Duration::ZERO,
                });
            }
            if call > 0 {
                set_names(&manager, id, &replayed).await;
            }
            // As the library: the identities on file, an earlier explicit
            // discovery's included, and those its discovery finds (review r1
            // N1: not a fabricated absence).
            let on_file = add_identities(&manager, id, &identities).await;
            establish(&manager, id, &contacts).await;
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
        let fetches = !self.skip_names && !self.fail_names.load(Ordering::SeqCst);
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            if !fetches {
                return;
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

    fn owns_a_name(&self, id: WalletId, identity: String) -> BoxedFuture<Option<bool>> {
        *guard(&self.questions).entry(id).or_default() += 1;
        let answer = (!self.fail_names.load(Ordering::SeqCst)).then(|| {
            (self.names.iter()).any(|(raw, list)| {
                base58(*raw) == identity && !list.is_empty() && !self.no_documents.contains(raw)
            })
        });
        if self.slow.iter().any(|raw| base58(*raw) == identity) {
            let slow_for = self.slow_for;
            return Box::pin(async move {
                tokio::time::sleep(slow_for).await;
                None
            });
        }
        let only = *guard(&self.hold_only);
        let hold = (self.hold_check.clone()).filter(|_| only.is_none_or(|w| w == id));
        Box::pin(async move {
            if let Some(hold) = hold {
                hold.notified().await;
            }
            answer
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

/// Establishes the `contacts` the wallet lacks, as the library's DashPay
/// pass does from the pairs' requests (no private details).
async fn establish(manager: &Manager, id: WalletId, contacts: &[([u8; 32], [u8; 32])]) {
    let wm = manager.wallet_manager_arc();
    let mut wm = wm.write().await;
    let info = wm.get_wallet_info_mut(&id.0).expect("wallet info");
    for (owner, contact) in contacts {
        let (owner, contact) = (Identifier::from(*owner), Identifier::from(*contact));
        let m = (info.identity_manager.managed_identity_mut(&owner)).expect("on file");
        if m.dashpay().established_contacts().contains_key(&contact) {
            continue;
        }
        let request = |from, to, at| ContactRequest::new(from, to, 0, 0, 0, vec![1; 96], 1, at);
        m.apply_established_contact(EstablishedContact::new(
            contact,
            request(owner, contact, 1_000),
            request(contact, owner, 2_000),
        ));
    }
}

pub(super) fn engine(dir: &std::path::Path) -> Engine {
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
    let s = open_offline(&engine);
    engine
        .block_on(s.vault_op(move |v| v.create(passphrase)))
        .unwrap();
    *guard(&s.platform.recovery.mock) = Some(platform);
    (engine, s)
}

/// The network opened, offline.
fn open_offline(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap()
}

/// The network of an engine opened again on `dir` (after a shutdown), its
/// vault as left, its Platform `platform`.
fn reopen(dir: &std::path::Path, platform: Arc<Platform>) -> (Engine, Arc<NetworkSession>) {
    let engine = engine(dir);
    let s = open_offline(&engine);
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
    // After pass 1: the identity, as main, without its names yet, which
    // it does not claim to lack (DP1-08 D23).
    assert_eq!(shown(&s, id), vec![(base58(ALICE), vec![], None, true)]);
    assert_eq!(names_check(&s, id), Some(NamesCheck::Pending));
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
    assert_eq!(names_check(&s, id), None);
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
    provenance.fallback_in_use().unwrap();
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

    // DEC-183: the record of the identity as the library holds it.
    let identity = guard(&s.platform.recovery.snapshots)[&id][0].entities[0].clone();
    assert_eq!(identity.kind, super::provenance::kind::IDENTITY);
    provenance.record_verified(&identity).unwrap();
    assert_eq!(unverified(), vec![(base58(ALICE), false)]);

    // The library replaces the identity's keys (from any source): the
    // record no longer vouches for what is shown.
    let manager = s.manager().unwrap();
    engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        let managed = (info.identity_manager)
            .managed_identity_mut(&Identifier::new(ALICE))
            .unwrap();
        let key = IdentityPublicKeyV0 {
            id: 0,
            purpose: Purpose::AUTHENTICATION,
            security_level: SecurityLevel::MASTER,
            contract_bounds: None,
            key_type: KeyType::ECDSA_SECP256K1,
            read_only: false,
            data: vec![2; 33].into(),
            disabled_at: None,
        };
        let keys = managed.identity.public_keys_mut();
        assert!(keys.insert(0, IdentityPublicKey::V0(key)).is_none());
    });
    assert_eq!(
        unverified(),
        vec![(base58(ALICE), true)],
        "a record of other content verifies nothing"
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// Sol r3 R3-3's order (DEC-135): with SPV stopped, a discovery's queued
/// bring-up signal has no receiver; the bring-up of the next start replaces
/// it, and as a bring-up of a wallet with an identity on file it advances
/// the boundary when it starts. A name Platform dates after the discovery
/// but before that start is stored read.
#[test]
fn a_discovery_while_spv_is_stopped_advances_at_the_next_bring_up() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        replayed_names: vec![(ALICE, "between")],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::NoIdentity
    });
    engine.block_on(s.stop_spv()).unwrap();

    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let found = engine
        .block_on(s.dashpay(id).discover_identities(scan))
        .unwrap();
    assert_eq!(found, 1);
    let discovered_at = boundary(&engine, &s, id).unwrap();
    // After the discovery's boundary, a second before the next start's.
    let between_ms = (discovered_at + 1) * 1_000 + 1;
    while crate::events::unix_now() < discovered_at + 3 {
        std::thread::sleep(Duration::from_millis(50));
    }
    persist_name_row(&s, id, ALICE, "between", between_ms);

    start(&engine, &s);
    wait_until("the replacement bring-up", || {
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

/// Sol r4 R4-2's order (DEC-143): with SPV stopped, a discovery stores an
/// identity; the next start's bring-up is stopped after its keys, before its
/// Platform pass. The discovery marker is consumed only once a bring-up has
/// advanced the boundary at its pass, so it survives, and the start after
/// that advances before replaying: a name Platform dates after the stopped
/// attempt is stored read.
#[test]
fn a_recovery_stopped_before_its_platform_pass_advances_at_the_next_start() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        replayed_names: vec![(ALICE, "between")],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::NoIdentity
    });
    engine.block_on(s.stop_spv()).unwrap();
    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let found = engine
        .block_on(s.dashpay(id).discover_identities(scan))
        .unwrap();
    assert_eq!(found, 1);

    let pause = Arc::new(TestPause {
        reached: AtomicBool::new(false),
        release: Notify::new(),
    });
    *guard(&s.platform.pause_after_keys) = Some(Arc::clone(&pause));
    engine.block_on(s.start_spv()).unwrap();
    wait_until("the keys", || pause.reached.load(Ordering::SeqCst));
    engine.block_on(s.stop_spv()).unwrap();
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1, "no pass ran");
    // The boundary as the stopped attempt left it (the discovery's: the
    // attempt stopped before its advance). The name is dated after it and a
    // second before the next start.
    let attempted = boundary(&engine, &s, id).unwrap();
    let between_ms = (attempted + 1) * 1_000 + 1;
    while crate::events::unix_now() < attempted + 3 {
        std::thread::sleep(Duration::from_millis(50));
    }
    persist_name_row(&s, id, ALICE, "between", between_ms);

    start(&engine, &s);
    wait_until("the replacement bring-up", || {
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

/// Sol r4 R4-1's probe (DEC-143): a reader pins app.sqlite's WAL, so the
/// latch commits but its checkpoint is refused. Reopened with the reader
/// still there, the session reads the latch back (its data counts as
/// unverified) but does not take it for durable: the fallback is refused
/// again until a checkpoint succeeds, and then the latch is in the database
/// file itself.
#[test]
fn a_reopen_after_a_refused_checkpoint_checkpoints_before_the_fallback() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform::default());
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    restore(&engine, &s);
    let app_db = s.data_dir().join(dw_appdb::APP_DB_FILE);
    let latch_in_file_alone = || {
        let copy = dir.path().join("file-alone.sqlite");
        std::fs::copy(&app_db, &copy).unwrap();
        let found = rusqlite::Connection::open(&copy)
            .unwrap()
            .query_row(
                "SELECT 1 FROM settings_kv WHERE scope = ?1 AND key = ?2",
                [
                    dw_appdb::GLOBAL_SCOPE,
                    super::provenance::TRUST_FALLBACK_USED_KEY,
                ],
                |_| Ok(()),
            )
            .is_ok();
        std::fs::remove_file(&copy).unwrap();
        found
    };
    engine
        .block_on(s.appdb_op(|db| db.checkpoint_full()))
        .unwrap();
    let reader = rusqlite::Connection::open(&app_db).unwrap();
    reader
        .execute_batch("BEGIN; SELECT * FROM settings_kv;")
        .unwrap();
    let provenance = s.live().unwrap().provenance;
    assert!(provenance.fallback_in_use().is_err(), "checkpoint refused");
    assert!(!latch_in_file_alone());
    drop(provenance);
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    let (engine, s) = reopen(dir.path(), platform);
    let provenance = s.live().unwrap().provenance;
    assert!(provenance.is_unverified(&super::provenance::Entity {
        kind: super::provenance::kind::IDENTITY,
        key: "x".into(),
        digest: Some("v1:00".into()),
    }));
    assert!(
        provenance.fallback_in_use().is_err(),
        "a committed row is not a durable one"
    );
    assert!(!latch_in_file_alone());
    reader.execute_batch("ROLLBACK").unwrap();
    provenance.fallback_in_use().unwrap();
    assert!(latch_in_file_alone());
    drop(provenance);
    engine.block_on(engine.shutdown()).unwrap();
}

/// DEC-139: a plain warm start is no recovery phase. A contact request
/// Platform dates after the last session's boundary, while the app was
/// closed, arrives after the next start's bring-up and stays news (it
/// notifies); the start did not move the boundary.
#[test]
fn a_warm_start_keeps_events_from_while_closed_as_news() {
    use platform_wallet::changeset::PlatformWalletPersistence;

    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the restore's recovery", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
            && s.platform.recovery.names_passes.load(Ordering::SeqCst) == 1
    });
    let closed_at = boundary(&engine, &s, id).unwrap();
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);
    // Received while the app was closed, a second after the boundary.
    let while_closed_ms = (closed_at + 1) * 1_000 + 1;
    while crate::events::unix_now() < closed_at + 3 {
        std::thread::sleep(Duration::from_millis(50));
    }

    let (engine, s) = reopen(dir.path(), Arc::clone(&platform));
    start(&engine, &s);
    wait_until("the warm start's bring-up", || {
        platform.bring_ups.load(Ordering::SeqCst) == 2
            && s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    assert_eq!(boundary(&engine, &s, id), Some(closed_at), "not moved");
    let request = contacts(incoming(ALICE[0], 9, while_closed_ms));
    s.live().unwrap().store.store(id.0, request).unwrap();
    assert_eq!(
        journal(&engine, &s, id),
        vec![(
            "request_received".into(),
            while_closed_ms.to_string(),
            false
        )]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// Sol r3 R3-1's session probe (DEC-135): the latch is on file before the
/// fallback serves anything, so an identity it fetched stays unverified
/// after the session closes and the network is opened again, with no
/// verification record. While app.sqlite is held the latch cannot be
/// written and the fallback is refused (nothing is fetched through it).
#[test]
fn a_fallback_identity_stays_unverified_after_a_reopen() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        discovered: vec![(ALICE, 3)],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let provenance = s.live().unwrap().provenance;
    let holder = rusqlite::Connection::open(s.data_dir().join(dw_appdb::APP_DB_FILE)).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert!(
        provenance.fallback_in_use().is_err(),
        "the fallback is refused"
    );
    holder.execute_batch("ROLLBACK").unwrap();
    provenance.fallback_in_use().unwrap();
    drop(provenance);

    let scan = grant(&engine, &s, id, GrantPurpose::IdentityScan);
    let found = engine
        .block_on(s.dashpay(id).discover_identities(scan))
        .unwrap();
    assert_eq!(found, 1);
    let unverified = |s: &Arc<NetworkSession>| {
        s.dashpay(id)
            .identities()
            .unwrap()
            .into_iter()
            .map(|i| (i.identity, i.unverified))
            .collect::<Vec<_>>()
    };
    assert_eq!(unverified(&s), vec![(base58(ALICE), true)]);
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    let (engine, s) = reopen(dir.path(), platform);
    wait_until("the wallet's identity", || !unverified(&s).is_empty());
    assert_eq!(unverified(&s), vec![(base58(ALICE), true)]);
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
    // It shows no name: nameless once Platform says it owns none, with no
    // names pass (DP1-08 D23).
    wait_until("the answer", || names_check(&s, id).is_none());
    engine.block_on(engine.shutdown()).unwrap();
}

/// Each identity's `names_check` on a live read.
fn names_checks(s: &Arc<NetworkSession>, id: WalletId) -> Vec<(String, Option<NamesCheck>)> {
    live_identities(&s.dashpay(id))
        .into_iter()
        .map(|i| (i.identity, i.names_check))
        .collect()
}

/// The main identity's `names_check` on a live read.
fn names_check(s: &Arc<NetworkSession>, id: WalletId) -> Option<NamesCheck> {
    let read = live_identities(&s.dashpay(id));
    read.into_iter()
        .find(|i| i.is_main)
        .expect("an identity")
        .names_check
}

/// DP1-08 D23 (Sol r1 finding 1): a restored identity that owns no name
/// reads `Pending`, never nameless, for as long as the names pass that
/// fetches its names is held: no clock settles it. Once the pass ends and
/// Platform says it owns none, it is nameless.
#[test]
fn a_nameless_identity_reads_pending_until_its_delayed_names_pass_ends() {
    let dir = dw_testutil::private_tempdir();
    let hold_names = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        names: vec![(ALICE, vec![])],
        hold_names: Some(Arc::clone(&hold_names)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("pass 1", || {
        !shown(&s, id).is_empty() && s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    for _ in 0..10 {
        assert_eq!(names_check(&s, id), Some(NamesCheck::Pending));
        std::thread::sleep(Duration::from_millis(100));
    }

    hold_names.notify_one();
    wait_until("pass 2", || names_check(&s, id).is_none());
    assert_eq!(shown(&s, id), vec![(base58(ALICE), vec![], None, true)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D23: while Platform answers nothing, an identity's names are
/// unknown (`Failed`), not absent, whether it owns a name or not; once it
/// answers, the nameless one is nameless, and the one that owns a name the
/// session has not loaded stays unknown.
#[test]
fn a_failed_names_pass_leaves_the_names_unknown_until_platform_answers() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0), (SECOND, 1)],
        names: vec![(ALICE, vec![("alice", Some(100))]), (SECOND, vec![])],
        fail_names: AtomicBool::new(true),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    let unknown = vec![
        (base58(ALICE), Some(NamesCheck::Failed)),
        (base58(SECOND), Some(NamesCheck::Failed)),
    ];
    wait_until("the failed names pass", || names_checks(&s, id) == unknown);
    assert!(
        shown(&s, id)
            .iter()
            .all(|(_, names, _, _)| names.is_empty())
    );

    platform.fail_names.store(false, Ordering::SeqCst);
    // The minute's check, now.
    engine.block_on(s.check_names(&s.manager().unwrap(), id));
    assert_eq!(
        names_checks(&s, id),
        vec![
            (base58(ALICE), Some(NamesCheck::Failed)),
            (base58(SECOND), None),
        ]
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D23 (Opus delta M1): the library's names pass succeeds while it
/// skips an identity whose names it could not fetch. Platform's answer
/// that the identity owns a name keeps it unknown, never nameless.
#[test]
fn an_identity_the_names_pass_skipped_is_not_nameless() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        names: vec![(ALICE, vec![("alice", Some(100))])],
        skip_names: true,
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the names pass", || {
        s.platform.recovery.names_passes.load(Ordering::SeqCst) == 1
            && names_check(&s, id) == Some(NamesCheck::Failed)
    });
    assert_eq!(shown(&s, id), vec![(base58(ALICE), vec![], None, true)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D23 (Opus delta r2 M1): an identity whose only listed label was
/// refused shows no main name though the library lists one: it is asked
/// about too, and is nameless once Platform says it owns no name.
#[test]
fn an_identity_whose_only_label_was_refused_is_asked_and_nameless() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        names: vec![(ALICE, vec![("taken", None)])],
        no_documents: vec![ALICE],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let refused = s.set_name_pref(id, base58(ALICE), "refused_name", Some("taken".into()));
    engine.block_on(refused).unwrap();
    start(&engine, &s);
    wait_until("the answer", || {
        s.platform.recovery.names_passes.load(Ordering::SeqCst) == 1
            && names_check(&s, id).is_none()
    });
    assert_eq!(shown(&s, id), vec![(base58(ALICE), vec![], None, true)]);
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D23 (Opus delta M2): an identity a bring-up stores without a
/// names pass (it was on file) is pending, not nameless, until Platform
/// answers for it; nothing else settles it.
#[test]
fn an_identity_on_file_reads_pending_until_platform_answers() {
    let dir = dw_testutil::private_tempdir();
    let hold_check = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        hold_check: Some(Arc::clone(&hold_check)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(platform.bring_up(manager, id)).unwrap();

    start(&engine, &s);
    wait_until("the bring-up", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::Ready
    });
    for _ in 0..5 {
        assert_eq!(names_check(&s, id), Some(NamesCheck::Pending));
        std::thread::sleep(Duration::from_millis(100));
    }
    hold_check.notify_one();
    wait_until("the answer", || names_check(&s, id).is_none());
    assert_eq!(s.platform.recovery.names_passes.load(Ordering::SeqCst), 0);
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D23: a pass owed again (a discovery, or a pass cut off before a
/// restart) keeps an identity Platform said is nameless nameless, and an
/// unknown one reads pending until that pass ends and a check settles it.
#[test]
fn owing_a_names_pass_again_keeps_the_answers() {
    let checks = NamesChecks::default();
    let (id, a, b) = (WalletId([1; 32]), base58(ALICE), base58(SECOND));
    assert_eq!(checks.of(&id, &a), Some(NamesCheck::Pending));
    let before = checks.begin(&id).expect("no pass owed");
    checks.owe(id);
    assert!(checks.begin(&id).is_none(), "the pass asks");
    assert!(checks.answered(
        id,
        before,
        vec![(a.clone(), Some(true)), (b.clone(), Some(false))]
    ));
    assert_eq!(checks.of(&id, &a), None);
    assert_eq!(checks.of(&id, &b), Some(NamesCheck::Pending));
    checks.pass_ended(id, checks.generation(&id));
    // A check begun before the pass ended does not settle it.
    assert!(!checks.answered(id, before, vec![]));
    assert_eq!(checks.of(&id, &b), Some(NamesCheck::Pending));
    let after = checks.begin(&id).expect("the pass ended");
    assert!(checks.answered(id, after, vec![]));
    assert_eq!(checks.of(&id, &b), Some(NamesCheck::Failed));

    checks.owe(id);
    assert_eq!(checks.of(&id, &a), None);
    assert_eq!(checks.of(&id, &b), Some(NamesCheck::Pending));
    assert!(!checks.answered(id, after, vec![(a.clone(), Some(true))]));
}

/// DP1-08 D28: an answer asked before the wallet was forgotten (removed or
/// unloaded) is dropped, as is the end of a pass begun before it.
#[test]
fn an_answer_for_a_forgotten_wallet_is_dropped() {
    let checks = NamesChecks::default();
    let (id, a) = (WalletId([1; 32]), base58(ALICE));
    let asked = checks.begin(&id).expect("no pass owed");
    let before = checks.generation(&id);
    checks.forget(&id);
    checks.owe(id);
    checks.pass_ended(id, before);
    assert!(checks.begin(&id).is_none(), "the pass is still owed");
    assert!(!checks.answered(id, asked, vec![(a.clone(), Some(true))]));
    assert_eq!(checks.of(&id, &a), Some(NamesCheck::Pending));

    checks.pass_ended(id, checks.generation(&id));
    let asked = checks.begin(&id).expect("the pass ended");
    assert!(checks.answered(id, asked, vec![(a.clone(), Some(true))]));
    assert_eq!(checks.of(&id, &a), None);
}

/// The names questions asked about `id`'s identities so far.
fn questions(platform: &Platform, id: WalletId) -> u32 {
    guard(&platform.questions)
        .get(&id)
        .copied()
        .unwrap_or_default()
}

/// DP1-08 D27 (Sol r2 finding 1): a names check whose Platform answers are
/// held stalls nothing else: a wallet added meanwhile is brought up while
/// it waits. Triggers while it runs make one more run follow, not one each.
#[test]
fn a_held_names_check_stalls_no_other_wallet() {
    let dir = dw_testutil::private_tempdir();
    let hold_check = Arc::new(Notify::new());
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        // Platform says it owns a name the session has not loaded: asked
        // again by every check.
        names: vec![(ALICE, vec![("alice", Some(100))])],
        hold_check: Some(Arc::clone(&hold_check)),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let first = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(platform.bring_up(manager, first)).unwrap();
    // The second wallet's questions are answered at once.
    *guard(&platform.hold_only) = Some(first);

    start(&engine, &s);
    wait_until("the first wallet's question", || {
        questions(&platform, first) == 1
    });
    let second = engine
        .block_on(s.import_wallet(
            Zeroizing::new(LEGAL_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    // Unbounded before D27: the supervisor awaited the held check.
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.dashpay_startup(&second).unwrap().startup != StartupStatus::Ready {
        assert!(
            Instant::now() < deadline,
            "the second wallet's bring-up waited on the check"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(questions(&platform, first), 1);
    assert_eq!(names_check(&s, first), Some(NamesCheck::Pending));

    // Two triggers while it runs: one more run.
    let epoch = s.platform.recovery.names_runs.epoch();
    s.kick_names_check(first, epoch);
    s.kick_names_check(first, epoch);
    hold_check.notify_one();
    wait_until("the rerun's question", || questions(&platform, first) == 2);
    assert_eq!(names_check(&s, first), Some(NamesCheck::Failed));
    hold_check.notify_one();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(questions(&platform, first), 2);
    assert_eq!(names_check(&s, first), Some(NamesCheck::Failed));
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D28 (Sol r2 finding 2): an answer that comes back after its wallet
/// was removed is dropped, even with the same wallet imported again
/// meanwhile: it never inherits a stale "nameless".
#[test]
fn an_answer_landing_after_a_removal_is_dropped() {
    let (_dir, engine, s, id, wipe) = removable();
    let (reached, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
    *guard(&s.platform.recovery.pause_before_answer) = Some((reached.0, release.1));
    let (session, manager) = (Arc::clone(&s), s.manager().unwrap());
    let check =
        s.rt.spawn(async move { session.check_names(&manager, id).await });
    reached
        .1
        .recv_timeout(Duration::from_secs(20))
        .expect("the check answered");

    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    assert_eq!(restore(&engine, &s), id);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    release.0.send(()).unwrap();
    engine.block_on(check).unwrap();
    assert_eq!(names_check(&s, id), Some(NamesCheck::Pending));
    engine.block_on(engine.shutdown()).unwrap();
}

/// The names check of `identity`, one of `id`'s, on a live read.
fn names_check_of(s: &Arc<NetworkSession>, id: WalletId, identity: [u8; 32]) -> Option<NamesCheck> {
    names_checks(s, id)
        .into_iter()
        .find(|(i, _)| *i == base58(identity))
        .expect("the identity")
        .1
}

/// DP1-08 D31 (Sol r3 finding 1): eight identities whose questions each
/// take a question's whole budget, ahead of a ninth that Platform answers
/// at once. The first check's budget runs out before the ninth is asked; the
/// next ones ask first what has not been asked (or longest ago), so within
/// two retries the ninth has its answer and no longer reads "Username not
/// loaded". The times are scaled down (1 s per question, 1.5 s per check,
/// against the real 10 s and 20 s).
#[test]
fn a_slow_prefix_starves_no_identity() {
    let dir = dw_testutil::private_tempdir();
    let slow: Vec<[u8; 32]> = (0..8u8).map(|n| [20 + n; 32]).collect();
    const NINTH: [u8; 32] = [40; 32];
    let mut identities: Vec<([u8; 32], u32)> =
        slow.iter().zip(0..).map(|(raw, n)| (*raw, n)).collect();
    identities.push((NINTH, 8));
    let platform = Arc::new(Platform {
        identities,
        slow: slow.clone(),
        slow_for: Duration::from_secs(1),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine
        .block_on(platform.bring_up(Arc::clone(&manager), id))
        .unwrap();
    // Off the boundary where the second four would end (2 s), which a
    // loaded scheduler can cross either way.
    *guard(&s.platform.recovery.check_budget) = Some(Duration::from_millis(1500));

    engine.block_on(s.check_names(&manager, id));
    assert_eq!(
        names_check_of(&s, id, NINTH),
        Some(NamesCheck::Failed),
        "not reached"
    );
    for _ in 0..2 {
        engine.block_on(s.check_names(&manager, id));
        if names_check_of(&s, id, NINTH).is_none() {
            break;
        }
    }
    assert_eq!(
        names_check_of(&s, id, NINTH),
        None,
        "starved behind the slow ones"
    );
    assert!(
        slow.iter()
            .all(|raw| names_check_of(&s, id, *raw) == Some(NamesCheck::Failed))
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D32 (Sol r3 finding 2): the minute's retry held between two kicks
/// (no yield, so its abort cannot land) while `stop_spv` stops the
/// supervisor: once released, its second kick starts no check, and none is
/// alive after `stop_spv` returned.
#[test]
fn a_retry_held_across_a_stop_starts_no_check() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        // Asked again by every check, and each question held.
        names: vec![(ALICE, vec![("alice", Some(100))])],
        hold_check: Some(Arc::new(Notify::new())),
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let first = restore(&engine, &s);
    let second = engine
        .block_on(s.import_wallet(
            Zeroizing::new(LEGAL_12.to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let manager = s.manager().unwrap();
    for id in [first, second] {
        engine
            .block_on(platform.bring_up(Arc::clone(&manager), id))
            .unwrap();
    }
    *guard(&s.platform.recovery.retry_every) = Some(Duration::from_millis(200));

    start(&engine, &s);
    wait_until("both wallets' checks", || {
        questions(&platform, first) >= 1 && questions(&platform, second) >= 1
    });
    let (reached, release, kicked) = (
        std::sync::mpsc::channel(),
        std::sync::mpsc::channel(),
        std::sync::mpsc::channel(),
    );
    *guard(&s.platform.recovery.pause_between_kicks) = Some((reached.0, release.1, kicked.0));
    reached
        .1
        .recv_timeout(Duration::from_secs(20))
        .expect("the retry's first kick");

    engine.block_on(s.stop_spv()).unwrap();
    let asked = questions(&platform, first) + questions(&platform, second);
    // Aborted: gone at their next poll.
    wait_until("the stop's aborts", || {
        s.platform.recovery.names_runs.alive() == 0
    });
    release.0.send(()).unwrap();
    kicked
        .1
        .recv_timeout(Duration::from_secs(20))
        .expect("the retry's second kick");
    assert_eq!(
        s.platform.recovery.names_runs.alive(),
        0,
        "the held retry started a check"
    );
    assert_eq!(
        questions(&platform, first) + questions(&platform, second),
        asked
    );
    engine.block_on(engine.shutdown()).unwrap();
}

/// DP1-08 D33 (Sol r3 finding 3): a failed restore's rollback forgets the
/// wallet before it leaves the manager. A check begun in between (as the
/// minute's retry would) reads the wallet still there, at its new
/// generation; its answer, held until the same wallet is imported again,
/// is dropped: the identity reads pending, not a stale "nameless".
#[test]
fn a_check_begun_during_a_rollback_leaves_nothing() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    let wallet = engine.block_on(manager.get_wallet(&id.0)).unwrap();
    let checks = &s.platform.recovery.names_checks;
    let before = checks.generation(&id);
    std::thread::scope(|scope| {
        let restoring = s.platform.restoring(vec![id]);
        // Holds the rollback before the wallet leaves the manager.
        let payment = engine.block_on(wallet.generation().payment_guard());
        let rollback = scope.spawn(|| {
            engine.block_on(async {
                let held = s.wallet_lifecycle.hold().await;
                s.roll_back_restore(&held, &[(id, true)]).await
            })
        });
        wait_until("the rollback's forget", || checks.generation(&id) > before);
        let (reached, release) = (std::sync::mpsc::channel(), std::sync::mpsc::channel());
        *guard(&s.platform.recovery.pause_before_answer) = Some((reached.0, release.1));
        let (session, manager) = (Arc::clone(&s), Arc::clone(&manager));
        let check =
            s.rt.spawn(async move { session.check_names(&manager, id).await });
        reached
            .1
            .recv_timeout(Duration::from_secs(20))
            .expect("the check answered");

        drop(payment);
        rollback.join().unwrap();
        drop(restoring);
        assert_eq!(restore(&engine, &s), id);
        let manager = s.manager().unwrap();
        engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
        release.0.send(()).unwrap();
        engine.block_on(check).unwrap();
    });
    assert_eq!(names_check(&s, id), Some(NamesCheck::Pending));
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
    s.lock_vault_sync().unwrap();
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
        s.lock_vault_sync().unwrap();
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
        let rollback = scope.spawn(|| {
            engine.block_on(async {
                let held = s.wallet_lifecycle.hold().await;
                s.roll_back_restore(&held, &[(id, true)]).await
            })
        });
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

/// A settle requires a verified seed binding after an unlock too (Sol r1
/// S1): with keys of another seed in the vault, an empty read changes no
/// recorded digest, and the wallet reports `SeedBindingUnverified`. (Here
/// the drain has a DashPay pass's refresh queued and reports the mismatch
/// too; with nothing queued it checks nothing, which
/// `an_unlock_with_nothing_queued_still_checks_the_binding` covers.)
#[test]
fn an_unlock_settle_needs_a_verified_seed_binding() {
    const BOB: [u8; 32] = [9; 32];
    const CAROL: [u8; 32] = [10; 32];
    let bob = PrivateDetails {
        alias: Some("Bobby".into()),
        ..PrivateDetails::default()
    };
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        contacts: vec![(ALICE, BOB), (ALICE, CAROL)],
        ..Platform::default()
    });
    let (engine, s) = session_with(dir.path(), platform, Some(PASSPHRASE));
    let infos = Arc::new(FakeInfos::default());
    *guard(&s.platform.contacts.info_source) = Some(Arc::clone(&infos) as Arc<dyn InfoSource>);
    let id = restore(&engine, &s);
    engine.block_on(async {
        let crypto = s.prompt_free_crypto(id).await.unwrap();
        infos
            .seal(&crypto, (ALICE.into(), 0, BOB.into()), &bob)
            .await;
    });
    start(&engine, &s);
    let dp = s.dashpay(id);
    let state = |contact| {
        let d = dp.contact(base58(ALICE), base58(contact)).ok().flatten()?;
        Some(d.publish_state)
    };
    wait_until("the details", || {
        state(BOB) == Some(PublishState::Published)
    });
    wait_until("an empty contact-crypto queue", || {
        let startup = s.dashpay_startup(&id).unwrap();
        startup.startup == StartupStatus::Ready && startup.contact_accounts_pending == 0
    });

    infos.remove(ALICE.into(), BOB.into());
    engine
        .block_on(s.vault_op(move |v| {
            v.delete_wallet_secret(&id.0)?;
            v.store_wallet_secret(&id.0, &super::private_details_tests::other_seed())
        }))
        .unwrap();
    s.lock_vault_sync().unwrap();
    engine
        .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, UnlockScope::Full)))
        .unwrap();
    wait_until("the settle's check", || {
        s.dashpay_startup(&id).unwrap().startup == StartupStatus::SeedBindingUnverified
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(state(BOB), Some(PublishState::Published));
    engine.block_on(engine.shutdown()).unwrap();
}

/// ROADMAP DP2-04's acceptance: a restored wallet gets its contacts'
/// private details back. Another device of the seed published them (its
/// own engine and vault); a restore into a fresh datadir reads them after
/// its bring-up and shows them `Published`, or with the vault locked then,
/// after the unlock.
#[test]
fn a_restored_wallet_gets_its_private_details_back() {
    const BOB: [u8; 32] = [9; 32];
    const CAROL: [u8; 32] = [10; 32];
    let bob = PrivateDetails {
        alias: Some("Bobby".into()),
        note: Some("met at the meetup".into()),
        hidden: false,
    };
    let carol = PrivateDetails {
        hidden: true,
        ..PrivateDetails::default()
    };
    let infos = Arc::new(FakeInfos::default());
    {
        let old = dw_testutil::private_tempdir();
        let engine = super::journal_tests::engine(old.path(), Arc::new(NoEvents));
        let (s, wallet) = super::journal_tests::session(&engine, true);
        engine.block_on(async {
            let crypto = s.prompt_free_crypto(wallet).await.unwrap();
            infos
                .seal(&crypto, (ALICE.into(), 0, BOB.into()), &bob)
                .await;
            infos
                .seal(&crypto, (ALICE.into(), 0, CAROL.into()), &carol)
                .await;
        });
        engine.block_on(engine.shutdown()).unwrap();
    }
    for locked in [false, true] {
        let dir = dw_testutil::private_tempdir();
        let platform = Arc::new(Platform {
            identities: vec![(ALICE, 0)],
            contacts: vec![(ALICE, BOB), (ALICE, CAROL)],
            ..Platform::default()
        });
        let (engine, s) = session_with(dir.path(), platform, Some(PASSPHRASE));
        *guard(&s.platform.contacts.info_source) = Some(Arc::clone(&infos) as Arc<dyn InfoSource>);
        let id = restore(&engine, &s);
        let dp = s.dashpay(id);
        let detail = |contact| {
            let d = dp.contact(base58(ALICE), base58(contact)).ok().flatten()?;
            Some((d.private_details, d.publish_state))
        };
        if locked {
            s.lock_vault_sync().unwrap();
        }
        start(&engine, &s);
        if locked {
            wait_until("the bring-up", || {
                s.dashpay_startup(&id).unwrap().startup != StartupStatus::Starting
            });
            std::thread::sleep(Duration::from_millis(300));
            let shown = detail(BOB).map(|(d, _)| d);
            assert!(
                shown
                    .as_ref()
                    .is_none_or(|d| *d == PrivateDetails::default()),
                "{shown:?}"
            );
            engine
                .block_on(s.vault_op(|v| v.unlock(PASSPHRASE, UnlockScope::Full)))
                .unwrap();
        }
        wait_until("the details", || {
            detail(BOB) == Some((bob.clone(), PublishState::Published))
                && detail(CAROL) == Some((carol.clone(), PublishState::Published))
        });
        engine.block_on(engine.shutdown()).unwrap();
    }
}

// ---- a flow held on the bring-up (dw-tdash B2) ----

fn startup(s: &NetworkSession, id: WalletId) -> StartupStatus {
    s.dashpay_startup(&id).unwrap().startup
}

/// dw-tdash B2: a bring-up that left the seed's identity indexes unanswered
/// holds a funding at `Startup` (not `Network`), and nothing in the session
/// ran it again. A held flow's request now does, at once, which lifts the
/// hold without a restart; once it has answered, no flow asks again.
#[test]
fn a_held_flow_has_a_partial_bring_up_run_again() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        partial_bring_ups: 1,
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the partial bring-up", || {
        startup(&s, id) == StartupStatus::PartialNoIdentity
    });
    let hold = |s: &NetworkSession| super::registration_flow::startup_hold(startup(s, id));
    assert_eq!(hold(&s), Some(super::RegistrationWait::Startup));
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);

    assert!(s.platform.retry_startup(id));
    wait_until("the bring-up again", || {
        startup(&s, id) == StartupStatus::Ready
    });
    assert_eq!(hold(&s), None);
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
    assert_eq!(
        s.dashpay_startup(&id).unwrap().identity,
        Some(base58(ALICE))
    );
    assert!(!s.platform.retry_startup(id), "answered: nothing to run");
    engine.block_on(engine.shutdown()).unwrap();
}

/// dw-tdash B2: reruns a held flow asks for back off. The first runs at
/// once; then 30 s, doubling, never more than 10 minutes apart, for as long
/// as the outcome holds; and none while SPV is stopped.
#[test]
fn bring_up_reruns_back_off_to_ten_minutes() {
    use super::runtime::{STARTUP_RETRY_MAX, STARTUP_RETRY_MIN, startup_retry_pause};

    let pauses: Vec<u64> = (1..=8).map(|n| startup_retry_pause(n).as_secs()).collect();
    assert_eq!(pauses, [30, 60, 120, 240, 480, 600, 600, 600]);
    assert_eq!(startup_retry_pause(1), STARTUP_RETRY_MIN);
    assert_eq!(startup_retry_pause(u32::MAX), STARTUP_RETRY_MAX);

    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform {
        partial_bring_ups: u32::MAX,
        ..Platform::default()
    });
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    let runs = || platform.bring_ups.load(Ordering::SeqCst);
    wait_until("the partial bring-up", || {
        startup(&s, id) == StartupStatus::PartialNoIdentity
    });
    let t0 = Instant::now();
    let mut at = Duration::ZERO;
    for (n, pause) in [0u64, 30, 60, 120, 240, 480, 600, 600]
        .into_iter()
        .enumerate()
    {
        at += Duration::from_secs(pause);
        let ran = runs();
        if n > 0 {
            let early = t0 + at - Duration::from_millis(1);
            assert!(
                !s.platform.retry_startup_at(id, early),
                "rerun {n}: before its pause"
            );
        }
        assert!(s.platform.retry_startup_at(id, t0 + at), "rerun {n}");
        wait_until("the rerun", || {
            runs() == ran + 1 && startup(&s, id) == StartupStatus::PartialNoIdentity
        });
    }
    // An outcome that holds nothing ends the schedule: the next hold's
    // first request runs at once.
    assert!(!s.platform.retry_startup_at(id, t0 + at));
    let outcome = |s| super::DashPayStartup::new(s, false);
    s.platform.record(id, outcome(StartupStatus::NoIdentity));
    assert!(!s.platform.retry_startup_at(id, t0 + at));
    s.platform
        .record(id, outcome(StartupStatus::PartialNoIdentity));
    let ran = runs();
    assert!(s.platform.retry_startup_at(id, t0 + at), "a fresh schedule");
    wait_until("the rerun", || {
        runs() == ran + 1 && startup(&s, id) == StartupStatus::PartialNoIdentity
    });
    // Stopped, nothing runs it; the next start does, and the schedule
    // starts over.
    engine.block_on(s.stop_spv()).unwrap();
    assert!(!s.platform.retry_startup_at(id, t0 + at + STARTUP_RETRY_MAX));
    start(&engine, &s);
    wait_until("the start's bring-up", || {
        startup(&s, id) == StartupStatus::PartialNoIdentity
    });
    let ran = runs();
    assert!(s.platform.retry_startup(id));
    wait_until("the rerun", || runs() == ran + 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// dw-tdash B2: a wallet whose bring-up Platform's recent proof of no
/// identity skipped (`NotRun`) answers no funding (§3.4 Restore rule 1):
/// it held a registration at `Sync` until the proof was 7 days old. A held
/// flow's rerun runs the discovery anyway, and Platform proves it again.
#[test]
fn a_rerun_runs_the_discovery_a_proven_absence_skipped() {
    let dir = dw_testutil::private_tempdir();
    let platform = Arc::new(Platform::default());
    let (engine, s) = session(dir.path(), Arc::clone(&platform));
    let id = restore(&engine, &s);
    start(&engine, &s);
    wait_until("the bring-up", || {
        startup(&s, id) == StartupStatus::NoIdentity
    });
    engine.block_on(s.stop_spv()).unwrap();
    start(&engine, &s);
    wait_until("the skipped bring-up", || {
        startup(&s, id) == StartupStatus::NotRun
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 1);
    assert_eq!(
        super::registration_flow::startup_hold(StartupStatus::NotRun),
        Some(super::RegistrationWait::Startup)
    );

    assert!(s.platform.retry_startup(id));
    wait_until("the discovery", || {
        startup(&s, id) == StartupStatus::NoIdentity
    });
    assert_eq!(platform.bring_ups.load(Ordering::SeqCst), 2);
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

/// E0-10b fix r1 (DP2-01): the re-fetch walks the contacts' cached profiles
/// and the contact labels the read models show. A contact's profile the
/// library replaced after verification is unverified until the re-fetch
/// records the new content; a label verified as one contact's reads
/// unverified when shown for another (the digest binds the owner). The
/// fetch itself is `trust_tests`'; here the walk and the record.
#[test]
fn contact_profiles_and_shown_labels_are_refetched_until_verified() {
    const BOB: [u8; 32] = [9; 32];
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    let set_profile = |name: &str| {
        engine.block_on(async {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm.get_wallet_info_mut(&id.0).unwrap();
            let managed = (info.identity_manager)
                .managed_identity_mut(&Identifier::new(ALICE))
                .unwrap();
            let entry = ContactProfileEntry {
                profile: Some(DashPayProfile {
                    display_name: Some(name.into()),
                    ..Default::default()
                }),
                checked_at_ms: 1,
            };
            managed
                .dashpay_contact_profiles_mut()
                .insert(Identifier::new(BOB), entry);
        })
    };
    let held = || {
        engine.block_on(async {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            held_unverified(&wm, &provenance)
        })
    };
    // The re-fetch's record step for `h`, once its fetch verified.
    let record = |h: &Held| {
        let records = confirmed(&[h], &held());
        assert_eq!(records.len(), 1, "{h:?} is held and unverified");
        provenance.record_verified(&records[0]).unwrap();
    };
    let profile = |name: &str| Held::Profile {
        id: Identifier::new(BOB),
        shown: ShownProfile {
            display_name: Some(name.into()),
            ..Default::default()
        },
    };
    let label = |spelling: &str, owner: [u8; 32]| Held::label(spelling, Identifier::new(owner));

    set_profile("Bob");
    assert!(held().contains(&profile("Bob")), "a contact's profile");
    record(&profile("Bob"));
    assert!(!held().contains(&profile("Bob")), "verified");
    set_profile("Mallory");
    assert!(held().contains(&profile("Mallory")), "replaced: unverified");
    record(&profile("Mallory"));
    assert!(!held().contains(&profile("Mallory")), "re-verified");

    provenance.show_label(Identifier::new(BOB), Some("Bob"));
    assert!(held().contains(&label("Bob", BOB)), "a shown contact label");
    record(&label("Bob", BOB));
    assert!(!held().contains(&label("Bob", BOB)));
    assert!(!provenance.is_unverified(&label("Bob", BOB).entity()));
    provenance.show_label(Identifier::new(BOB), Some("B0b"));
    assert!(held().contains(&label("B0b", BOB)), "another spelling");
    assert!(provenance.is_unverified(&label("B0b", BOB).entity()));
    provenance.show_label(Identifier::new(SECOND), Some("Bob"));
    assert!(
        held().contains(&label("Bob", SECOND)),
        "shown for another owner"
    );
    assert!(provenance.is_unverified(&label("Bob", SECOND).entity()));
    provenance.show_label(Identifier::new(SECOND), None);
    assert!(!held().contains(&label("Bob", SECOND)), "no longer shown");
}

/// Opus-high N2: the re-fetch walks every contact request the library holds
/// for an identity: sent, incoming, and both directions of an established
/// contact. Nothing else is a contact request.
#[test]
fn the_refetch_walks_sent_incoming_and_established_requests() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    let request = |sender: [u8; 32], recipient: [u8; 32], created_at| {
        let (sender, recipient) = (Identifier::new(sender), Identifier::new(recipient));
        ContactRequest::new(sender, recipient, 0, 0, 0, vec![1; 96], 1, created_at)
    };
    let (bob, carol, dave) = ([9; 32], [10; 32], [11; 32]);
    let sent = request(ALICE, bob, 1);
    let incoming = request(carol, ALICE, 2);
    let (ours, theirs) = (request(ALICE, dave, 3), request(dave, ALICE, 4));
    engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        let managed = (info.identity_manager)
            .managed_identity_mut(&Identifier::new(ALICE))
            .unwrap();
        managed.apply_sent_contact_request(sent.clone());
        managed.apply_incoming_contact_request(incoming.clone());
        managed.apply_established_contact(EstablishedContact::new(
            Identifier::new(dave),
            ours.clone(),
            theirs.clone(),
        ));
    });
    let held = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        held_unverified(&wm, &provenance)
    });
    let mut requests: Vec<_> = (held.into_iter())
        .filter_map(|h| match h {
            Held::ContactRequest(r) => Some(r),
            _ => None,
        })
        .collect();
    requests.sort_by_key(|r| r.created_at);
    assert_eq!(requests, vec![sent, incoming, ours, theirs]);
}

/// Sol r1 R1, the reviewer's probe: an identity whose name verified as
/// `Alice` and that the library then shows as `Al1ce` (same normalized
/// label, same owner, no new record) is unverified.
#[test]
fn reviewer_dpns_spelling_replacement_must_be_unverified() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let wallet_id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, wallet_id, &[(ALICE, 0)]));
    engine.block_on(set_names(&manager, wallet_id, &[(ALICE, "Alice")]));
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    let summary = || s.dashpay(wallet_id).identities().unwrap().remove(0);
    assert!(summary().unverified);
    let entities = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&wallet_id.0).unwrap();
        let m = (info.identity_manager)
            .managed_identity(&Identifier::from(ALICE))
            .unwrap();
        (super::reverify::held_of(m, info).iter())
            .map(Held::entity)
            .collect::<Vec<_>>()
    });
    for e in &entities {
        provenance.record_verified(e).unwrap();
    }
    let before = summary();
    assert_eq!(before.names, vec!["Alice"]);
    assert!(!before.unverified);
    // The library replaces the spelling (from any source), no new record.
    engine.block_on(set_names(&manager, wallet_id, &[(ALICE, "Al1ce")]));
    let after = summary();
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(after.names, vec!["Al1ce"]);
    assert!(
        after.unverified,
        "unproved displayed spelling retained the verified flag"
    );
}

/// Manager ruling (Sol r1 confirmation): records live in the network's own
/// app.sqlite, so a record a testnet session writes is invisible to a
/// mainnet session on the same data root. Keeps that true if storage moves.
#[test]
fn a_testnet_record_is_invisible_to_a_mainnet_session() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(dir.path());
    let open = |network| {
        let s = engine
            .block_on(engine.open_network(network, SessionOptions::default()))
            .unwrap();
        let provenance = s.live().unwrap().provenance;
        provenance.fallback_in_use().unwrap();
        (s, provenance)
    };
    let (testnet, on_testnet) = open(DashNetwork::Testnet);
    let (mainnet, on_mainnet) = open(DashNetwork::Mainnet);
    let entities = [
        Held::Identity {
            id: Identifier::new(ALICE),
            keys: BTreeMap::new(),
        },
        Held::label("Alice", Identifier::new(ALICE)),
    ]
    .map(|h| h.entity());
    for e in &entities {
        on_testnet.record_verified(e).unwrap();
        assert!(!on_testnet.is_unverified(e));
        assert!(on_mainnet.is_unverified(e), "{e:?} on mainnet");
    }
    drop((on_testnet, on_mainnet, testnet, mainnet));
    engine.block_on(engine.shutdown()).unwrap();
    let engine = self::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Mainnet, SessionOptions::default()))
        .unwrap();
    let reopened = s.live().unwrap().provenance;
    for e in &entities {
        assert!(reopened.is_unverified(e), "{e:?} after a reopen");
    }
}

/// Sol r2 R3: two identities cache different revisions of one contact's
/// profile and both fetches verified (Platform changed between them). Both
/// writes succeed under one record key, the second replacing the first, so
/// one revision is still unverified: the step counts that from the library,
/// not from the writes.
#[test]
fn the_record_step_counts_what_is_left_from_the_library() {
    const BOB: [u8; 32] = [9; 32];
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0), (SECOND, 1)]));
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    let revision = |name: &str| ShownProfile {
        display_name: Some(name.into()),
        ..Default::default()
    };
    let (held_before, step) = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&id.0).unwrap();
        for (holder, name) in [(ALICE, "Bob"), (SECOND, "Bobby")] {
            let entry = ContactProfileEntry {
                profile: Some(DashPayProfile {
                    display_name: Some(name.into()),
                    ..Default::default()
                }),
                checked_at_ms: 1,
            };
            (info.identity_manager)
                .managed_identity_mut(&Identifier::new(holder))
                .unwrap()
                .dashpay_contact_profiles_mut()
                .insert(Identifier::new(BOB), entry);
        }
        let wm = wm.downgrade();
        let held = held_unverified(&wm, &provenance);
        let copies =
            |h: &&Held| matches!(h, Held::Profile { id, .. } if *id == Identifier::new(BOB));
        let verified: Vec<&Held> = held.iter().filter(copies).collect();
        assert_eq!(verified.len(), 2, "both revisions are held");
        let step = record_step(&wm, &provenance, &verified).await.unwrap();
        (held.len(), step)
    });
    // The identities and one of the two revisions are still unverified;
    // `held - written` would have said both revisions were done.
    assert_eq!(
        step,
        Recorded {
            written: 2,
            left: held_before - 1
        }
    );
    let unverified = |name: &str| {
        let h = Held::Profile {
            id: Identifier::new(BOB),
            shown: revision(name),
        };
        provenance.is_unverified(&h.entity())
    };
    assert_ne!(unverified("Bob"), unverified("Bobby"), "one record per key");
}

/// Opus-high probe A (M2): a label the summary hides (a contested name the
/// library lists as soon as the write returns) does not count against the
/// identity: everything it shows verified, so it reads verified. The
/// re-fetch still walks the hidden label, which verifies once Platform has
/// its domain.
#[test]
fn opus_probe_a_a_hidden_contested_label_leaves_the_identity_verified() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let wallet_id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, wallet_id, &[(ALICE, 0)]));
    engine.block_on(async {
        let wallet = manager.get_wallet(&wallet_id.0).await.unwrap();
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let info = wm.get_wallet_info_mut(&wallet_id.0).unwrap();
        let m = (info.identity_manager)
            .managed_identity_mut(&Identifier::from(ALICE))
            .unwrap();
        let name = |l: &str| DpnsNameInfo {
            label: l.into(),
            acquired_at: Some(FETCHED_AT),
        };
        m.set_dpns_names(vec![name("Alice"), name("bob")], wallet.persister());
        m.set_contested_dpns_names(vec!["bob".into()], wallet.persister());
    });
    let provenance = s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    let contested = Held::label("bob", Identifier::new(ALICE));
    let entities = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let info = wm.get_wallet_info(&wallet_id.0).unwrap();
        let m = (info.identity_manager)
            .managed_identity(&Identifier::from(ALICE))
            .unwrap();
        (super::reverify::held_of(m, info).iter())
            .map(Held::entity)
            .collect::<Vec<_>>()
    });
    // Everything the summary shows verified (as the re-fetch records it).
    for e in entities.iter().filter(|e| **e != contested.entity()) {
        provenance.record_verified(e).unwrap();
    }
    let summary = s.dashpay(wallet_id).identities().unwrap().remove(0);
    let still_fetched = engine.block_on(async {
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        held_unverified(&wm, &provenance)
    });
    engine.block_on(engine.shutdown()).unwrap();
    assert_eq!(summary.names, vec!["Alice"], "bob is not shown");
    assert!(
        !summary.unverified,
        "everything shown verified, yet the identity reads unverified"
    );
    assert_eq!(still_fetched, vec![contested], "the re-fetch walks it");
}

/// Opus-high probe B (M1): an installation from before E0-10b holds library
/// data the trusted service's keys proved, but no latch (those builds never
/// set it). Upgrading runs `trust_digest` over a database with wallets,
/// which latches it: the data reads unverified and the re-fetch walks it.
#[test]
fn opus_probe_b_pre_e0_10b_data_starts_unverified_and_is_refetched() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let wallet_id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, wallet_id, &[(ALICE, 0)]));
    engine.block_on(set_names(&manager, wallet_id, &[(ALICE, "Alice")]));
    let unverified = |s: &Arc<NetworkSession>| {
        (s.dashpay(wallet_id).identities().unwrap().into_iter())
            .map(|i| (i.identity, i.unverified))
            .collect::<Vec<_>>()
    };
    assert_eq!(unverified(&s), vec![(base58(ALICE), false)], "not latched");
    let appdb = s.data_dir().join(dw_appdb::APP_DB_FILE);
    drop(manager);
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    // app.sqlite as a build from before E0-10b left it: without the later
    // migrations too (DP1-02's registration columns, DP2-04's digests and
    // removed wallets with their triggers, DP2-05's page index, DP3-01's
    // payment-lock inputs, BL-229's governance registrations, MG-06's
    // proposals, MG-04's ProTx operations, MG-04b's shared sessions and
    // standbys), which such a build never had, or the runner refuses the gap
    // below them.
    let conn = rusqlite::Connection::open(&appdb).unwrap();
    let drop_triggers: String = conn
        .query_row(
            "SELECT group_concat('DROP TRIGGER \"' || name || '\";', ' ')
             FROM sqlite_master WHERE type = 'trigger'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch(&drop_triggers).unwrap();
    conn.execute_batch(
        "DROP INDEX dp_events_page;
             DROP TABLE removed_wallets;
             ALTER TABLE dp_trust_verified DROP COLUMN digest;
             ALTER TABLE dp_registration DROP COLUMN owner;
             ALTER TABLE dp_registration DROP COLUMN name_error;
             ALTER TABLE dp_registration DROP COLUMN profile_error;
             DROP TABLE dp_contact_info;
             ALTER TABLE dp_payment_lock DROP COLUMN inputs;
             DROP TABLE governance_registrations;
             DROP TABLE masternode_ops;
             DROP TABLE shared_mn_sessions;
             DROP TABLE shared_mn_standby;
             DROP TABLE mg_proposals;
             DROP TABLE mg_proposals_kept;
             DELETE FROM refinery_schema_history WHERE version >= 2026101101;
             DELETE FROM settings_kv WHERE key = 'trust.fallback_used_at';",
    )
    .unwrap();

    let (engine, s) = reopen(dir.path(), Arc::new(Platform::default()));
    wait_until("the wallet's identity", || !unverified(&s).is_empty());
    assert_eq!(unverified(&s), vec![(base58(ALICE), true)]);
    let provenance = s.live().unwrap().provenance;
    let refetched = engine.block_on(async {
        let manager = s.manager().unwrap();
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        held_unverified(&wm, &provenance)
    });
    engine.block_on(engine.shutdown()).unwrap();
    assert!(refetched.contains(&Held::label("Alice", Identifier::new(ALICE))));
    assert!(
        (refetched.iter())
            .any(|h| matches!(h, Held::Identity { id, .. } if *id == Identifier::new(ALICE))),
        "{refetched:?}"
    );
}

// ---- DEC-210: writes after wallet removal ----

use platform_wallet::changeset::PlatformWalletPersistence;

use super::journal_tests::{contacts, incoming};
use crate::coins::TestPause as JobPause;
use crate::held_writes::{ReleaseOnDrop, a_write_held_past_the_removal, hold_first_job};

/// A session with the ABANDON_12 wallet restored and ALICE among its
/// identities, and a removal grant for the wallet.
fn removable() -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    WalletId,
    String,
) {
    let dir = dw_testutil::private_tempdir();
    let (engine, s, id, wipe) = removable_in(dir.path());
    (dir, engine, s, id, wipe)
}

/// [`removable`] in `dir`.
fn removable_in(dir: &std::path::Path) -> (Engine, Arc<NetworkSession>, WalletId, String) {
    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        ..Platform::default()
    });
    let (engine, s) = session(dir, Arc::clone(&platform));
    let id = restore(&engine, &s);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &platform.identities));
    let wipe = grant(&engine, &s, id, GrantPurpose::Wipe);
    (engine, s, id, wipe)
}

/// recovery.rs `set_main_identity_of`: its write held past a removal.
#[test]
fn a_main_identity_write_held_past_a_removal_leaves_nothing() {
    let (_dir, engine, s, id, wipe) = removable();
    let dp = s.dashpay(id);
    let (pause, task) = hold_first_job(&s, async move {
        let _ = dp.set_main_identity(base58(ALICE)).await;
    });
    let db = s.live().unwrap().appdb;
    a_write_held_past_the_removal(&engine, &s, (id, wipe), &pause, Some(task), || {
        db.main_identity(&id.to_string()).unwrap().is_none()
    });
    engine.block_on(engine.shutdown()).unwrap();
}

/// recovery.rs `set_name_pref`: its write held past a removal.
#[test]
fn a_name_pref_write_held_past_a_removal_leaves_nothing() {
    let (_dir, engine, s, id, wipe) = removable();
    let session = Arc::clone(&s);
    let (pause, task) = hold_first_job(&s, async move {
        let _ = session
            .set_main_name(id, base58(ALICE), Some("alice".into()))
            .await;
    });
    let db = s.live().unwrap().appdb;
    a_write_held_past_the_removal(&engine, &s, (id, wipe), &pause, Some(task), || {
        db.dp_prefs(&id.to_string(), super::recovery::MAIN_NAME_PREF)
            .unwrap()
            .is_empty()
    });
    engine.block_on(engine.shutdown()).unwrap();
}

/// The journal tap writes inside the persister's `store`, with no fence: a
/// write held there does not hold up a removal, and lands nothing after it.
#[test]
fn a_journal_write_held_past_a_removal_leaves_nothing() {
    let (_dir, engine, s, id, wipe) = removable();
    let live = s.live().unwrap();
    let pause = Arc::new(JobPause::default());
    *guard(&live.tap.pause_journal) = Some(Arc::clone(&pause));
    let store = Arc::clone(&live.store);
    let request = contacts(incoming(ALICE[0], 9, 1));
    let storing =
        s.rt.spawn_blocking(move || store.store(id.0, request).unwrap());
    a_write_held_past_the_removal(&engine, &s, (id, wipe), &pause, None, || {
        journal(&engine, &s, id).is_empty()
    });
    engine.block_on(storing).unwrap();
    engine.block_on(engine.shutdown()).unwrap();
}

/// The tap storing changesets of the wallet all through its removal: the
/// removal returns, the tap's writes return, and none is left behind.
#[test]
fn the_tap_writing_during_a_removal_neither_blocks_nor_deadlocks() {
    let (_dir, engine, s, id, wipe) = removable();
    let store = Arc::clone(&s.live().unwrap().store);
    let stop = AtomicBool::new(false);
    let stored = AtomicU32::new(0);
    std::thread::scope(|scope| {
        // Stops the tap however the scope ends: its join waits for it.
        struct Stop<'a>(&'a AtomicBool);
        impl Drop for Stop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let _stop = Stop(&stop);
        let tap = scope.spawn(|| {
            for sender in (1..=u8::MAX).cycle() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let request = contacts(incoming(ALICE[0], sender, u64::from(sender)));
                // After the removal the persister may refuse the wallet.
                let _ = store.store(id.0, request);
                stored.fetch_add(1, Ordering::SeqCst);
            }
        });
        wait_until("the tap's first writes", || {
            stored.load(Ordering::SeqCst) > 3
        });
        engine
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(60), s.remove_wallet(id, wipe)).await
            })
            .expect("the removal returns while the tap writes")
            .unwrap();
        let after = stored.load(Ordering::SeqCst);
        wait_until("the tap's writes after the removal", || {
            stored.load(Ordering::SeqCst) > after + 3
        });
        stop.store(true, Ordering::SeqCst);
        wait_until("the tap's end", || tap.is_finished());
    });
    assert!(journal(&engine, &s, id).is_empty());
    engine.block_on(engine.shutdown()).unwrap();
}

/// The pump's first-seen write (events.rs `flush_history`), a job no one
/// waits for, held past a removal.
#[test]
fn a_first_seen_write_held_past_a_removal_leaves_nothing() {
    use dashcore::hashes::Hash;

    let (_dir, engine, s, id, wipe) = removable();
    let pause = Arc::new(JobPause::default());
    *guard(&s.hub.pause_seen) = Some(Arc::clone(&pause));
    s.hub
        .pump
        .mark_history(id, Some(&[dashcore::Txid::from_byte_array([5; 32])]));
    let db = s.live().unwrap().appdb;
    a_write_held_past_the_removal(&engine, &s, (id, wipe), &pause, None, || {
        db.tx_seen_times(&id.to_string()).unwrap().is_empty()
    });
    engine.block_on(engine.shutdown()).unwrap();
}

/// The same wallet imported again after its removal, in the same run, is
/// written as any wallet: the import readmits it before its first write.
#[test]
fn a_wallet_imported_again_after_its_removal_is_written() {
    let (_dir, engine, s, id, wipe) = removable();
    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    assert_eq!(restore(&engine, &s), id);
    let db = s.live().unwrap().appdb;
    let wallet = id.to_string();
    assert!(
        db.wallets().unwrap().iter().any(|(w, ..)| *w == wallet),
        "the import's name"
    );
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    engine
        .block_on(s.dashpay(id).set_main_identity(base58(ALICE)))
        .unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), Some(base58(ALICE)));
    let request = contacts(incoming(ALICE[0], 9, 1));
    s.live().unwrap().store.store(id.0, request).unwrap();
    assert_eq!(journal(&engine, &s, id).len(), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// The bring-up counts the bell (DP2-05): after a reopen the unread count
/// is in memory before any `unread_count` call, so the first sync read
/// answers it.
#[test]
fn the_bring_up_counts_the_bell() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s, id, _) = removable_in(dir.path());
    let request = contacts(incoming(ALICE[0], 9, u64::MAX / 2));
    s.live().unwrap().store.store(id.0, request).unwrap();
    drop(s);
    engine.block_on(engine.shutdown()).unwrap();
    drop(engine);

    let platform = Arc::new(Platform {
        identities: vec![(ALICE, 0)],
        ..Platform::default()
    });
    let (engine, s) = reopen(dir.path(), platform);
    start(&engine, &s);
    let alice = base58(ALICE);
    wait_until("the bring-up's count", || {
        s.live().unwrap().tap.unread(id, &alice) == Some(1)
    });
    assert_eq!(s.dashpay(id).unread_count(alice).unwrap(), 1);
    engine.block_on(engine.shutdown()).unwrap();
}

/// A removed wallet's bell goes with its tombstone (DP2-05, DEC-210): its
/// rows and counts are gone, the calls refuse it, and the same seed
/// imported again starts with an empty bell.
#[test]
fn a_removed_wallets_bell_goes_with_it() {
    let (_dir, engine, s, id, wipe) = removable();
    let alice = base58(ALICE);
    let request = || contacts(incoming(ALICE[0], 9, u64::MAX / 2));
    s.live().unwrap().store.store(id.0, request()).unwrap();
    let dp = s.dashpay(id);
    dp.unread_count(alice.clone()).unwrap();
    wait_until("the count", || dp.unread_count(alice.clone()).unwrap() == 1);

    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    assert!(journal(&engine, &s, id).is_empty());
    assert_eq!(
        dp.unread_count(alice.clone()).unwrap_err().code(),
        "wallet_not_found"
    );
    assert_eq!(s.live().unwrap().tap.unread(id, &alice), None);

    assert_eq!(restore(&engine, &s), id);
    let manager = s.manager().unwrap();
    engine.block_on(add_identities(&manager, id, &[(ALICE, 0)]));
    assert_eq!(dp.unread_count(alice.clone()).unwrap(), 0);
    let page = engine.block_on(dp.events(alice, None, 50)).unwrap();
    assert!(page.new.is_empty() && page.earlier.is_empty(), "{page:?}");
    engine.block_on(engine.shutdown()).unwrap();
}

/// A network closed and opened again while the app database of the
/// closed session is still alive (a job of it holds it) keeps the wallet
/// it removed recorded, and a late write through it is skipped (DEC-210
/// (3b)).
fn a_removal_outlives_a_reopen(dir: &std::path::Path) {
    let (engine, s, id, wipe) = removable_in(dir);
    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    let old = s.live().unwrap().appdb;
    drop(s);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    // `old` is the only instance left.
    assert_eq!(Arc::strong_count(&old), 1);

    let s = open_offline(&engine);
    let wallet = id.to_string();
    old.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(
        s.live().unwrap().appdb.main_identity(&wallet).unwrap(),
        None
    );
    engine.block_on(engine.shutdown()).unwrap();
}

#[test]
fn a_removal_outlives_a_reopen_while_its_app_database_is_alive() {
    a_removal_outlives_a_reopen(dw_testutil::private_tempdir().path());
}

/// [`a_removal_outlives_a_reopen`] in a directory whose path is not UTF-8.
#[cfg(target_os = "linux")]
#[test]
fn a_removal_outlives_a_reopen_in_a_directory_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::DirBuilderExt;
    let dir = dw_testutil::private_tempdir();
    let root = dir.path().join(std::ffi::OsStr::from_bytes(b"w-\xff"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    assert!(root.to_str().is_none());
    a_removal_outlives_a_reopen(&root);
}

/// No instance of the app database at `path` is open.
fn assert_nothing_left_open(path: &std::path::Path) {
    let lock = path.with_file_name(dw_appdb::APP_DB_LOCK_FILE);
    let free = std::fs::File::open(lock).unwrap().try_lock().is_ok();
    assert!(free, "an instance of this process is left");
}

/// A network closed and opened again with nothing of the closed session
/// left forgets the wallets it removed (DEC-210 (3b)).
#[test]
fn a_clean_reopen_forgets_the_removed_wallets() {
    let (_dir, engine, s, id, wipe) = removable();
    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    let path = s.data_dir().join(dw_appdb::APP_DB_FILE);
    drop(s);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_nothing_left_open(&path);

    let s = open_offline(&engine);
    let (db, wallet) = (s.live().unwrap().appdb, id.to_string());
    db.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), Some(base58(ALICE)));
    engine.block_on(engine.shutdown()).unwrap();
}

/// A session opened while another process has the network's app database
/// open, after it removed a wallet, keeps that wallet recorded: the other
/// process's late write of it is skipped (DEC-210 (3b)).
#[test]
fn an_open_keeps_a_removal_by_another_process_with_the_database_open() {
    let dir = dw_testutil::private_tempdir();
    let (engine, s) = session(dir.path(), Arc::new(Platform::default()));
    let path = s.data_dir().join(dw_appdb::APP_DB_FILE);
    drop(s);
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_nothing_left_open(&path);
    let wallet = "ee".repeat(32);
    let other = crate::held_writes::OtherProcess::start(&path, &wallet);

    let s = open_offline(&engine);
    other.write_late();
    let names = s.live().unwrap().appdb.wallet_names().unwrap();
    assert!(names.iter().all(|(w, _)| *w != wallet), "{names:?}");
    engine.block_on(engine.shutdown()).unwrap();
}

/// A removal and an import of the same wallet run one after the other
/// (DEC-210 (3b)): an import begun while the removal is held, its wallet
/// already out of the manager, waits for it, then registers the wallet
/// with its writes accepted.
#[test]
fn an_import_during_a_removal_of_the_same_wallet_waits_for_it() {
    let (_dir, engine, s, id, wipe) = removable();
    let (pause, removal) = hold_removal(&engine, &s, id, wipe);
    let _release = ReleaseOnDrop(Arc::clone(&pause));
    let manager = s.manager().unwrap();

    let session = Arc::clone(&s);
    let import = s.rt.spawn(async move {
        session
            .import_wallet(
                Zeroizing::new(ABANDON_12.to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions::default(),
            )
            .await
    });
    wait_until("the import at the lifecycle lock", || {
        import.is_finished() || s.wallet_lifecycle.waiting() == 1
    });
    assert!(!import.is_finished(), "the import ran during the removal");
    pause.release.notify_one();
    engine.block_on(removal).unwrap();
    assert_eq!(engine.block_on(import).unwrap().unwrap(), id);

    assert!(engine.block_on(manager.get_wallet(&id.0)).is_some());
    let (db, wallet) = (s.live().unwrap().appdb, id.to_string());
    db.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), Some(base58(ALICE)));
    assert!(db.wallet_names().unwrap().iter().any(|(w, _)| *w == wallet));
    engine.block_on(engine.shutdown()).unwrap();
}

/// The removal of wallet `id`, held after the wallet left the manager and
/// before its rows are deleted.
fn hold_removal(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    id: WalletId,
    wipe: String,
) -> (Arc<JobPause>, tokio::task::JoinHandle<()>) {
    let session = Arc::clone(s);
    let (pause, removal) = hold_first_job(s, async move {
        session.remove_wallet(id, wipe).await.unwrap();
    });
    wait_until("the held removal", || pause.reached.load(Ordering::SeqCst));
    let manager = s.manager().unwrap();
    assert!(engine.block_on(manager.get_wallet(&id.0)).is_none());
    (pause, removal)
}

/// The other operations that register or remove a wallet wait for a held
/// removal too (DEC-210 (3b)): a watch-only import, a restore, and the
/// open and close of a wallet.
#[test]
fn the_other_lifecycle_operations_wait_for_a_held_removal() {
    let (dir, engine, s, id, wipe) = removable();
    let xpub = engine.block_on(s.account_xpub(id, 0)).unwrap().xpub;
    let backup = dir.path().join("wallet.dwbackup");
    let passphrase = || Some(Zeroizing::new(b"backup passphrase".to_vec()));
    engine
        .block_on(s.backup_wallet(id, backup.clone(), passphrase()))
        .unwrap();
    let (pause, removal) = hold_removal(&engine, &s, id, wipe);
    let _release = ReleaseOnDrop(Arc::clone(&pause));

    let (a, b, c, d) = (
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
        Arc::clone(&s),
    );
    let waiting = [
        (
            "a watch-only import",
            s.rt.spawn(async move {
                let _ = a.import_watch_only(xpub, Default::default()).await;
            }),
        ),
        (
            "a restore",
            s.rt.spawn(async move {
                let _ = b.restore_backup(backup, passphrase()).await;
            }),
        ),
        (
            "an open",
            s.rt.spawn(async move {
                let _ = c.load_wallet(id).await;
            }),
        ),
        (
            "a close",
            s.rt.spawn(async move {
                let _ = d.unload_wallet(id).await;
            }),
        ),
    ];
    wait_until("all four at the lifecycle lock", || {
        waiting.iter().any(|(_, task)| task.is_finished()) || s.wallet_lifecycle.waiting() == 4
    });
    for (what, task) in &waiting {
        assert!(!task.is_finished(), "{what} ran during the removal");
    }
    pause.release.notify_one();
    engine.block_on(removal).unwrap();
    for (_, task) in waiting {
        engine.block_on(task).unwrap();
    }
    engine.block_on(engine.shutdown()).unwrap();
}

/// An import that readmits a removed wallet and fails after registering it
/// leaves it registered, so not recorded as removed: its writes land.
#[test]
fn an_import_failing_after_registering_leaves_the_wallet_writable() {
    let (_dir, engine, s, id, wipe) = removable();
    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    guard(&crate::keys::FAIL_AFTER_REGISTRATION_IN).push(s.data_dir().to_path_buf());
    let failed = engine.block_on(s.import_wallet(
        Zeroizing::new(ABANDON_12.to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions::default(),
    ));
    assert!(failed.is_err(), "{failed:?}");
    let manager = s.manager().unwrap();
    assert!(engine.block_on(manager.get_wallet(&id.0)).is_some());
    let (db, wallet) = (s.live().unwrap().appdb, id.to_string());
    db.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), Some(base58(ALICE)));
    engine.block_on(engine.shutdown()).unwrap();
}

/// A watch-only import that readmits a removed wallet and then fails to
/// store it records the wallet as removed again (DEC-210 (3b)).
#[test]
fn a_failed_watch_only_import_records_the_wallet_as_removed_again() {
    let (_dir, engine, s, id, _) = removable();
    let xpub = engine.block_on(s.account_xpub(id, 0)).unwrap().xpub;
    let watch = engine
        .block_on(s.import_watch_only(xpub.clone(), Default::default()))
        .unwrap();
    let wipe = grant(&engine, &s, watch, GrantPurpose::Wipe);
    engine.block_on(s.remove_wallet(watch, wipe)).unwrap();
    guard(&crate::keys::FAIL_REGISTRATION_IN).push(s.data_dir().to_path_buf());
    let failed = engine.block_on(s.import_watch_only(xpub, Default::default()));
    assert!(failed.is_err(), "{failed:?}");
    let (db, wallet) = (s.live().unwrap().appdb, watch.to_string());
    db.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), None);
    engine.block_on(engine.shutdown()).unwrap();
}

/// An import that readmits a removed wallet and then fails to register it
/// records the wallet as removed again, with what the import wrote of it.
#[test]
fn a_failed_import_records_the_wallet_as_removed_again() {
    let (_dir, engine, s, id, wipe) = removable();
    engine.block_on(s.remove_wallet(id, wipe)).unwrap();
    guard(&crate::keys::FAIL_REGISTRATION_IN).push(s.data_dir().to_path_buf());
    let failed = engine.block_on(s.import_wallet(
        Zeroizing::new(ABANDON_12.to_vec()),
        Zeroizing::new(Vec::new()),
        ImportOptions::default(),
    ));
    assert!(failed.is_err(), "{failed:?}");
    let (db, wallet) = (s.live().unwrap().appdb, id.to_string());
    let fingerprint = crate::send::psbt::FINGERPRINT_SETTING;
    assert_eq!(db.setting(&wallet, fingerprint).unwrap(), None);
    db.set_main_identity(&wallet, &base58(ALICE)).unwrap();
    assert_eq!(db.main_identity(&wallet).unwrap(), None);
    engine.block_on(engine.shutdown()).unwrap();
}
