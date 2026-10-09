//! Tests of names.rs (DP1-03): the main-name order, the temporary-name
//! policy, the contest join deadline, the vote-state verdicts and
//! registration steps, and, on a session with no network, a main name that
//! survives DPNS sync (platform #4978) and a restart.

use std::path::Path;
use std::sync::Arc;

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{Identity, IdentityPublicKey};
use dw_vault::Credential;
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use platform_wallet::DpnsFetch;

use super::*;
use crate::{Engine, EngineConfig, EventSink, SessionOptions};

fn names(labels: &[&str]) -> Vec<DpnsNameInfo> {
    labels
        .iter()
        .map(|label| DpnsNameInfo {
            label: label.to_string(),
            acquired_at: None,
        })
        .collect()
}

fn contests(labels: &[&str]) -> Vec<String> {
    labels.iter().map(|l| l.to_string()).collect()
}

fn prefs(pick: Option<&str>, temporary: Option<&str>, contested: Option<&str>) -> MainNamePrefs {
    MainNamePrefs {
        pick: pick.map(str::to_string),
        temporary: temporary.map(str::to_string),
        contested: contested.map(str::to_string),
        pending: Vec::new(),
    }
}

fn untimed(labels: &[&str]) -> Vec<(String, Option<u64>)> {
    labels.iter().map(|l| (l.to_string(), None)).collect()
}

fn main_of(owned: &[&str], open: &[&str], prefs: MainNamePrefs) -> Option<String> {
    resolve_main_name(&untimed(owned), &contests(open), &prefs)
}

#[test]
fn the_pick_wins_while_owned_in_the_owned_spelling() {
    let main = main_of(
        &["alice2", "Bob-Builder"],
        &[],
        prefs(Some("b0b-bui1der"), Some("alice2"), None),
    );
    assert_eq!(main.as_deref(), Some("Bob-Builder"));
}

#[test]
fn a_pick_no_longer_owned_is_skipped_not_lost() {
    let pick = || prefs(Some("alice"), None, None);
    assert_eq!(
        main_of(&["carol", "dave"], &[], pick()).as_deref(),
        Some("carol")
    );
    assert_eq!(
        main_of(&["carol", "dave", "alice"], &[], pick()).as_deref(),
        Some("alice")
    );
}

#[test]
fn the_temporary_name_shows_while_the_contest_is_open() {
    // platform-wallet appends a contested label to the owned list; until
    // `record_contest` moves it, the open contests filter it out.
    let open = &["a11ce"];
    let main = main_of(
        &["alice2", "alice"],
        open,
        prefs(None, Some("alice2"), Some("alice")),
    );
    assert_eq!(main.as_deref(), Some("alice2"));
    // Picking the contested label does not show it before it is won.
    let main = main_of(
        &["carol", "alice2", "alice"],
        open,
        prefs(Some("alice"), Some("alice2"), Some("alice")),
    );
    assert_eq!(main.as_deref(), Some("alice2"));
}

#[test]
fn a_won_contest_shows_the_won_name_and_a_lost_one_the_oldest_name() {
    let after = || prefs(None, Some("alice2"), Some("alice"));
    assert_eq!(
        main_of(&["carol", "alice2", "alice"], &[], after()).as_deref(),
        Some("alice")
    );
    assert_eq!(
        main_of(&["alice2"], &[], after()).as_deref(),
        Some("alice2")
    );
    // A later extra name does not displace the oldest one after a lost
    // contest (review DP1-03 #5).
    assert_eq!(
        main_of(&["alice2", "bob"], &[], after()).as_deref(),
        Some("alice2")
    );
}

#[test]
fn no_name_and_no_temporary_name_is_none() {
    assert_eq!(main_of(&[], &[], prefs(Some("alice"), None, None)), None);
    // A temporary name the identity does not own is not shown.
    assert_eq!(
        main_of(&[], &["a11ce"], prefs(None, Some("alice2"), None)),
        None
    );
    // Without prefs the oldest owned name shows.
    assert_eq!(
        main_of(&["zed", "amy"], &[], MainNamePrefs::default()).as_deref(),
        Some("zed")
    );
}

fn id(byte: u8) -> Identifier {
    Identifier::from([byte; 32])
}

fn b58(byte: u8) -> Option<String> {
    Some(id(byte).to_string(Encoding::Base58))
}

#[test]
fn the_vote_state_verdicts() {
    use ContestedDocumentVotePollWinnerInfo as W;
    let available = NameAvailability::Available { contested: true };
    let taken = NameAvailability::Taken { owner: b58(1) };
    assert_eq!(vote_verdict(None, vec![]).availability, available);
    assert_eq!(
        vote_verdict(Some(W::NoWinner), vec![]).availability,
        available
    );
    assert_eq!(
        vote_verdict(Some(W::WonByIdentity(id(1))), vec![id(2)]).availability,
        taken
    );
    assert_eq!(
        vote_verdict(Some(W::Locked), vec![id(2)]).availability,
        NameAvailability::Locked
    );
    let open = vote_verdict(None, vec![id(2), id(3)]);
    assert_eq!(
        open.availability,
        NameAvailability::ContestOpen {
            ends_at: None,
            contenders: 2
        }
    );
    assert_eq!(open.contenders, vec![id(2), id(3)]);
}

#[test]
fn the_registration_steps() {
    let me = id(1);
    let step = |found: &Lookup, until: Option<u64>| registration_step(found, &me, until, 1_000);
    let code = |r: Result<Option<NameOutcome>, NameError>| r.unwrap_err().code();
    let verdict = Lookup::verdict;

    assert_eq!(
        step(
            &verdict(NameAvailability::Available { contested: false }),
            None
        ),
        Ok(None)
    );
    assert_eq!(
        step(&verdict(NameAvailability::Taken { owner: b58(1) }), None),
        Ok(Some(NameOutcome::Registered))
    );
    assert_eq!(
        code(step(
            &verdict(NameAvailability::Taken { owner: b58(2) }),
            None
        )),
        "name.taken"
    );
    assert_eq!(
        code(step(&verdict(NameAvailability::Locked), None)),
        "name.locked"
    );
    assert_eq!(
        code(step(&verdict(NameAvailability::Unknown), None)),
        "platform.unavailable"
    );

    let contest = |contenders: Vec<Identifier>| Lookup {
        availability: NameAvailability::ContestOpen {
            ends_at: Some(5_000),
            contenders: u32::try_from(contenders.len()).unwrap(),
        },
        contenders,
    };
    // Already contending: no second join, whatever the deadline.
    assert_eq!(
        step(&contest(vec![id(2), me]), Some(10)),
        Ok(Some(NameOutcome::ContestStarted {
            ends_at: Some(5_000)
        }))
    );
    assert_eq!(step(&contest(vec![id(2)]), Some(1_001)), Ok(None));
    // A new contender with no known deadline may be past the window: no
    // fee is spent on it (review DP1-03 R2).
    assert_eq!(
        code(step(&contest(vec![id(2)]), None)),
        "platform.unavailable"
    );
    assert_eq!(
        code(step(&contest(vec![id(2)]), Some(1_000))),
        "name.contest_open"
    );
    // A full vote-state page may hide the identity and cannot be priced.
    let crowded: Vec<Identifier> = (2..=101).map(id).collect();
    assert_eq!(code(step(&contest(crowded), None)), "name.contest_open");
}

#[test]
fn a_contested_registration_moves_to_the_open_contests() {
    let dir = dw_testutil::private_tempdir();
    // What platform-wallet leaves after registering a contested label.
    let f = Fixture::new(dir.path(), &["carol", "Alice"]);
    f.with_identity(|managed, persister| {
        record_contest(managed, "Alice", persister);
        let labels: Vec<&str> = managed
            .dpns_names
            .iter()
            .map(|n| n.label.as_str())
            .collect();
        assert_eq!(labels, ["carol"]);
        assert_eq!(managed.contested_dpns_names, ["a11ce"]);
    });
}

#[test]
fn the_temporary_name_policy() {
    assert!(check_temporary_name("alice", "alice2").is_ok());
    assert!(check_temporary_name("alice", "Alice-Temp-7").is_ok());
    let code = |r: Result<(), NameError>| r.unwrap_err().code();
    // Contested itself: `[a-z01-]` only after folding, 3–19 characters.
    assert_eq!(
        code(check_temporary_name("alice", "alice1")),
        "invalid_argument"
    );
    assert_eq!(
        code(check_temporary_name("alice", "a11ce")),
        "invalid_argument"
    );
    // The requested name under folding, when it is not contested.
    assert_eq!(
        code(check_temporary_name("alice2", "A1ice2")),
        "invalid_argument"
    );
    assert_eq!(code(check_temporary_name("alice", "a")), "name.invalid");
    assert_eq!(
        code(check_temporary_name("alice", "al--ice2")),
        "name.invalid"
    );
}

#[test]
fn the_join_deadline_is_the_poll_less_the_join_window_before_the_end() {
    let version = PlatformVersion::latest();
    let end = 1_800_000_000;
    // Mainnet: a two-week poll that takes contenders for its first week.
    assert_eq!(
        join_deadline(end, &DashNetwork::Mainnet, version),
        end - 7 * 24 * 3600
    );
    // Test networks: 90 minutes, of which the first 45 take contenders.
    for network in [
        DashNetwork::Testnet,
        DashNetwork::Devnet { name: "d".into() },
        DashNetwork::Regtest,
    ] {
        assert_eq!(join_deadline(end, &network, version), end - 45 * 60);
    }
}

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

const IDENTITY: [u8; 32] = [7; 32];

fn identity_id() -> String {
    Identifier::from(IDENTITY).to_string(Encoding::Base58)
}

/// A regtest session with no network, whose new wallet holds a keyless
/// identity at index 0 owning `labels`.
struct Fixture {
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

impl Fixture {
    fn new(dir: &Path, labels: &[&str]) -> Self {
        Self::build(dir, labels, false)
    }

    /// With a HIGH ECDSA authentication key, which may sign DPNS documents.
    fn signing(dir: &Path, labels: &[&str]) -> Self {
        Self::build(dir, labels, true)
    }

    fn build(dir: &Path, labels: &[&str], signing: bool) -> Self {
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.join("data"),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(NullSink),
        )
        .unwrap();
        let session = Self::open(&engine);
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        let wallet = engine
            .block_on(session.create_wallet(12))
            .unwrap()
            .wallet_id;
        let f = Self {
            engine,
            session,
            wallet,
        };
        f.give_identity(labels, signing);
        f
    }

    fn open(engine: &Engine) -> Arc<NetworkSession> {
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

    fn reopen(&mut self) {
        self.engine
            .block_on(self.engine.close_network(DashNetwork::Regtest))
            .unwrap();
        self.session = Self::open(&self.engine);
    }

    fn dp(&self) -> Arc<DashPay> {
        self.session.dashpay(self.wallet)
    }

    /// Runs `f` on the identity under the wallet's write lock.
    fn with_identity(
        &self,
        f: impl FnOnce(&mut ManagedIdentity, &WalletPersister) + Send + 'static,
    ) {
        let manager = self.session.manager().unwrap();
        let id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&id.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let managed = state
                .identity_manager
                .managed_identity_mut(&IDENTITY.into())
                .unwrap();
            f(managed, wallet.persister());
        });
    }

    fn give_identity(&self, labels: &[&str], signing: bool) {
        let manager = self.session.manager().unwrap();
        let id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&id.0).await.unwrap();
            let key = IdentityPublicKey::V0(IdentityPublicKeyV0 {
                id: 0,
                purpose: Purpose::AUTHENTICATION,
                security_level: SecurityLevel::HIGH,
                contract_bounds: None,
                key_type: KeyType::ECDSA_SECP256K1,
                read_only: false,
                data: vec![2; 33].into(),
                disabled_at: None,
            });
            let identity = Identity::V0(IdentityV0 {
                id: IDENTITY.into(),
                public_keys: signing.then_some((0, key)).into_iter().collect(),
                balance: 0,
                revision: 0,
            });
            wallet
                .state_mut()
                .await
                .identity_manager
                .add_identity(identity, 0, id.0, wallet.persister())
                .unwrap();
        });
        let labels = names(labels);
        self.with_identity(move |m, p| m.set_dpns_names(labels, p));
    }

    /// What a DPNS username sync does with a fetch of `labels`.
    fn sync_names(&self, labels: &[&str], fetch: DpnsFetch) {
        let labels = names(labels);
        self.with_identity(move |m, p| {
            m.apply_fetched_dpns_names(labels, fetch, p);
        });
    }

    fn main_name(&self) -> Option<String> {
        self.engine
            .block_on(self.dp().main_name(identity_id()))
            .unwrap()
    }

    fn code<T: std::fmt::Debug>(
        &self,
        call: impl Future<Output = Result<T, NameError>>,
    ) -> &'static str {
        self.engine.block_on(call).unwrap_err().code()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.engine.block_on(self.engine.shutdown());
    }
}

/// Platform #4978: the chosen name is the user's. A username sync that does
/// not list it (a partial page, or a complete fetch of a wallet identity,
/// which only merges) leaves it chosen, and so does a restart.
#[test]
fn the_main_name_survives_sync_and_a_restart() {
    let dir = dw_testutil::private_tempdir();
    let mut f = Fixture::new(dir.path(), &["carol", "alice"]);
    assert_eq!(f.main_name().as_deref(), Some("carol"));
    f.engine
        .block_on(f.dp().set_main_name(identity_id(), Some("A1ICE".into())))
        .unwrap();
    assert_eq!(f.main_name().as_deref(), Some("alice"));

    f.sync_names(&["carol"], DpnsFetch::Partial);
    assert_eq!(f.main_name().as_deref(), Some("alice"));
    f.sync_names(&["carol", "dave"], DpnsFetch::Complete);
    assert_eq!(f.main_name().as_deref(), Some("alice"));

    f.reopen();
    assert_eq!(f.main_name().as_deref(), Some("alice"));

    // Clearing the pick falls back to the oldest name.
    f.engine
        .block_on(f.dp().set_main_name(identity_id(), None))
        .unwrap();
    assert_eq!(f.main_name().as_deref(), Some("carol"));
}

#[test]
fn only_an_owned_name_can_be_the_main_name() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::new(dir.path(), &["carol"]);
    let dp = f.dp();
    assert_eq!(
        f.code(dp.set_main_name(identity_id(), Some("zed".into()))),
        "invalid_argument"
    );
    // Another identity, or another wallet, is not this wallet's identity.
    let other = Identifier::from([8; 32]).to_string(Encoding::Base58);
    assert_eq!(f.code(dp.set_main_name(other, None)), "identity.not_found");
    let stranger = f.session.dashpay(WalletId([9; 32]));
    assert_eq!(
        f.code(stranger.set_main_name(identity_id(), None)),
        "wallet_not_found"
    );
    assert_eq!(
        f.code(dp.main_name("not base58!".into())),
        "invalid_argument"
    );
}

/// The answers `register_name` gives without the network or the grant.
#[test]
fn register_name_answers_owned_names_and_keyless_identities_offline() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::new(dir.path(), &["carol"]);
    let dp = f.dp();
    let register = |label: &str| dp.register_name(identity_id(), label.into(), "no grant".into());
    assert_eq!(
        f.engine.block_on(register("CAR0L")),
        Ok(NameOutcome::Registered)
    );
    // A label it contends for is not owned: it goes on to the checks.
    f.with_identity(|m, p| record_contest(m, "carol", p));
    assert_eq!(f.code(register("carol")), "identity.keys_missing");
    assert_eq!(f.code(register("bob-the-builder")), "identity.keys_missing");
    assert_eq!(f.code(register("b--b")), "name.invalid");
}

// Review DP1-03 R4: the fallback is the first acquired name, by Platform's
// times, not list order.

#[test]
fn the_fallback_is_the_first_acquired_name() {
    let owned = |names: &[(&str, Option<u64>)]| -> Vec<(String, Option<u64>)> {
        names.iter().map(|(n, at)| (n.to_string(), *at)).collect()
    };
    let none = MainNamePrefs::default();
    let main = |names| resolve_main_name(&owned(names), &[], &none);
    assert_eq!(
        main(&[("new2", Some(200)), ("old2", Some(100))]).as_deref(),
        Some("old2")
    );
    // No time comes last; a tie keeps list order.
    assert_eq!(
        main(&[("untimed", None), ("late", Some(900))]).as_deref(),
        Some("late")
    );
    assert_eq!(
        main(&[("b-tie", Some(5)), ("a-tie", Some(5))]).as_deref(),
        Some("b-tie")
    );
    // The pick, the temporary name and the won name still come first.
    let late_pick = prefs(Some("new2"), None, None);
    let names = owned(&[("new2", Some(200)), ("old2", Some(100))]);
    assert_eq!(
        resolve_main_name(&names, &[], &late_pick).as_deref(),
        Some("new2")
    );
}

fn row(
    document: u8,
    label: &str,
    status: DpnsNameSaleStatus,
    created: Option<u64>,
    transferred: Option<u64>,
) -> (Identifier, DpnsNameStateEntry) {
    let entry = DpnsNameStateEntry {
        document_id: id(document),
        wallet_identity_id: IDENTITY.into(),
        label: label.into(),
        normalized_label: convert_to_homograph_safe_chars(label),
        normalized_parent_domain_name: "dash".into(),
        price: None,
        status,
        created_at_ms: created,
        updated_at_ms: None,
        transferred_at_ms: transferred,
        last_synced_at_ms: 0,
    };
    (id(document), entry)
}

/// A restore stamps every name with one fetch time, in DPNS query order;
/// the marketplace rows carry when the identity got each one.
#[test]
fn acquisition_times_come_from_the_marketplace_rows() {
    let fetched = |label: &str| DpnsNameInfo {
        label: label.into(),
        acquired_at: Some(7_000),
    };
    let names = [
        fetched("zed"),
        fetched("Amy"),
        fetched("sold1"),
        fetched("bob"),
    ];
    let rows: BTreeMap<_, _> = [
        row(1, "zed", DpnsNameSaleStatus::Owned, Some(3_000), None),
        // Bought later than zed was registered.
        row(
            2,
            "amy",
            DpnsNameSaleStatus::Owned,
            Some(1_000),
            Some(4_000),
        ),
        row(3, "amy-earlier", DpnsNameSaleStatus::Owned, Some(1), None),
        row(
            4,
            "so1d1",
            DpnsNameSaleStatus::Sold { to: id(9) },
            Some(1),
            None,
        ),
    ]
    .into_iter()
    .collect();
    let owned = owned_names(IDENTITY.into(), &names, &rows);
    assert_eq!(
        owned,
        [
            ("zed".to_string(), Some(3_000)),
            ("Amy".to_string(), Some(4_000)),
            ("bob".to_string(), Some(7_000)),
        ]
    );
    assert_eq!(
        resolve_main_name(&owned, &[], &MainNamePrefs::default()).as_deref(),
        Some("zed")
    );
}

#[test]
fn the_main_name_follows_the_rows_on_a_session() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::new(dir.path(), &["new2", "old2"]);
    assert_eq!(f.main_name().as_deref(), Some("new2"));
    f.give_rows(vec![
        row(1, "new2", DpnsNameSaleStatus::Owned, Some(200), None),
        row(2, "old2", DpnsNameSaleStatus::Owned, Some(100), None),
    ]);
    assert_eq!(f.main_name().as_deref(), Some("old2"));
}

// Review DP1-03 R1–R3: `register_name` on a scripted network.

struct FakeNet {
    /// Answers in order; the last one repeats.
    lookups: Mutex<VecDeque<Lookup>>,
    balance: Option<u64>,
    end: Option<u64>,
    /// The write fails, and the label's state is whatever the next lookup
    /// says; otherwise it lands as the library's does: the label joins the
    /// list.
    fails: bool,
    submits: AtomicU32,
}

impl FakeNet {
    fn new(lookups: Vec<Lookup>) -> Self {
        Self {
            lookups: Mutex::new(lookups.into()),
            balance: Some(u64::MAX / 2),
            end: None,
            fails: false,
            submits: AtomicU32::new(0),
        }
    }

    fn available(contested: bool) -> Self {
        Self::new(vec![Lookup::verdict(NameAvailability::Available {
            contested,
        })])
    }

    fn balance(mut self, credits: u64) -> Self {
        self.balance = Some(credits);
        self
    }

    fn failing(mut self) -> Self {
        self.fails = true;
        self
    }

    fn ending(mut self, at: u64) -> Self {
        self.end = Some(at);
        self
    }

    fn submits(&self) -> u32 {
        self.submits.load(Ordering::SeqCst)
    }
}

impl NameNet for FakeNet {
    fn version(&self) -> &PlatformVersion {
        PlatformVersion::latest()
    }

    async fn lookup(&self, _: &UsernameCheck) -> Result<Lookup, PlatformError> {
        let mut lookups = self.lookups.lock().unwrap();
        let found = match lookups.len() {
            0 => panic!("no lookup scripted"),
            1 => lookups[0].clone(),
            _ => lookups.pop_front().unwrap(),
        };
        Ok(found)
    }

    async fn contest_end(&self, _: &str) -> Option<u64> {
        self.end
    }

    async fn balance(&self, _: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(self.balance)
    }

    async fn submit(
        &self,
        wallet: &PlatformWallet,
        identity: Identifier,
        label: &str,
        _: Option<u64>,
        _: &VaultIdentitySigner,
    ) -> Result<(), PlatformError> {
        self.submits.fetch_add(1, Ordering::SeqCst);
        if self.fails {
            return Err(PlatformError::Unavailable);
        }
        let mut state = wallet.state_mut().await;
        let managed = state
            .identity_manager
            .managed_identity_mut(&identity)
            .unwrap();
        let mut names = managed.dpns_names.clone();
        names.push(DpnsNameInfo {
            label: label.into(),
            acquired_at: Some(crate::events::unix_now() * 1000),
        });
        managed.set_dpns_names(names, wallet.persister());
        Ok(())
    }
}

/// A network a call must not reach.
fn offline() -> FakeNet {
    FakeNet::new(vec![])
}

fn me() -> Option<String> {
    Some(identity_id())
}

fn contest_with(contenders: Vec<Identifier>, ends_at: Option<u64>) -> Lookup {
    Lookup {
        availability: NameAvailability::ContestOpen {
            ends_at,
            contenders: u32::try_from(contenders.len()).unwrap(),
        },
        contenders,
    }
}

impl Fixture {
    /// A `PlatformOp` grant for this wallet capped at `max_credits`.
    fn grant(&self, max_credits: u64) -> String {
        self.session
            .vault
            .authorize(
                GrantPurpose::PlatformOp {
                    max_duffs: 0,
                    max_credits,
                },
                Some(&self.wallet.0),
                Credential::None,
            )
            .unwrap()
            .id
    }

    fn grant_unused(&self, grant: &str) -> bool {
        self.session
            .vault
            .check_grant(grant, GrantKind::PlatformOp, Some(&self.wallet.0))
            .is_ok()
    }

    fn register(
        &self,
        net: &Arc<FakeNet>,
        label: &str,
        grant: &str,
    ) -> Result<NameOutcome, NameError> {
        let ask = Registration {
            wallet_id: self.wallet,
            identity: IDENTITY.into(),
            label: label.into(),
            check: valid_label(label).unwrap(),
        };
        let (net, grant) = (Arc::clone(net), grant.to_string());
        self.engine
            .block_on(self.dp().on_wallet(move |session, _, wallet| async move {
                DashPay::register(&*net, &session, &wallet, ask, grant).await
            }))
    }

    fn prefs(&self) -> MainNamePrefs {
        let (session, wallet) = (Arc::clone(&self.session), self.wallet);
        self.engine
            .block_on(
                async move { DashPay::main_name_prefs(&session, wallet, &IDENTITY.into()).await },
            )
            .unwrap()
    }

    fn set_pref(&self, key: &'static str, value: &str) {
        let (session, wallet, value) = (Arc::clone(&self.session), self.wallet, value.to_string());
        self.engine
            .block_on(async move {
                DashPay::set_main_name_pref(&session, wallet, &IDENTITY.into(), key, Some(value))
                    .await
            })
            .unwrap();
    }

    fn set_pending(&self, pending: &[(NameKind, &str)]) {
        let prefs = MainNamePrefs {
            pending: pending.iter().map(|(k, l)| (*k, l.to_string())).collect(),
            ..MainNamePrefs::default()
        };
        self.set_pref(PREF_PENDING_NAME, &prefs.pending_value().unwrap());
    }

    fn labels(&self) -> (Vec<String>, Vec<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        self.with_identity(move |m, _| {
            let names = m.dpns_names.iter().map(|n| n.label.clone()).collect();
            tx.send((names, m.contested_dpns_names.clone())).unwrap();
        });
        rx.recv().unwrap()
    }

    fn contend(&self, label: &'static str) {
        self.with_identity(move |m, p| record_contest(m, label, p));
    }

    fn give_rows(&self, rows: Vec<(Identifier, DpnsNameStateEntry)>) {
        let manager = self.session.manager().unwrap();
        let id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&id.0).await.unwrap();
            wallet.state_mut().await.dpns_name_states.extend(rows);
        });
    }
}

#[test]
fn a_plain_name_budgets_its_fees_against_the_grant_and_the_balance() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let plain = name_cost(
        &valid_label("alice2").unwrap(),
        0,
        PlatformVersion::latest(),
    );
    assert_eq!(
        plain,
        NameCost {
            fund: None,
            total: NAME_FEE_BOUND
        }
    );

    // A one-credit grant, and a zero-credit one, are refused and kept.
    let net = Arc::new(FakeNet::available(false));
    for cap in [0, 1, NAME_FEE_BOUND - 1] {
        let grant = f.grant(cap);
        let err = f.register(&net, "alice2", &grant).unwrap_err();
        assert_eq!(
            err.platform(),
            Some(&PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Credits,
                needed: NAME_FEE_BOUND,
                remaining: cap,
            })
        );
        assert!(f.grant_unused(&grant));
    }
    // A balance short of the fees, before the grant is looked at.
    let poor = Arc::new(FakeNet::available(false).balance(NAME_FEE_BOUND - 1));
    let grant = f.grant(NAME_FEE_BOUND);
    assert_eq!(
        f.register(&poor, "alice2", &grant).unwrap_err().platform(),
        Some(&PlatformError::InsufficientCredits {
            needed: NAME_FEE_BOUND,
            available: NAME_FEE_BOUND - 1,
        })
    );
    assert!(f.grant_unused(&grant));
    // Another wallet's grant, or another kind, is refused and kept too.
    let wipe = f
        .session
        .vault
        .authorize(GrantPurpose::Wipe, Some(&f.wallet.0), Credential::None)
        .unwrap()
        .id;
    assert_eq!(
        f.register(&net, "alice2", &wipe).unwrap_err().code(),
        "platform.grant_invalid"
    );
    assert_eq!(net.submits() + poor.submits(), 0);

    // Fees covered exactly: one write, and the grant is spent.
    let rich = Arc::new(FakeNet::available(false).balance(NAME_FEE_BOUND));
    assert_eq!(
        f.register(&rich, "alice2", &grant),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(rich.submits(), 1);
    assert!(!f.grant_unused(&grant));
    assert_eq!(f.labels().0, ["carol", "alice2"]);
}

#[test]
fn a_contested_name_budgets_the_fund_and_the_fees() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let cost = name_cost(&valid_label("alice").unwrap(), 0, PlatformVersion::latest());
    let fund = cost.fund.unwrap();
    assert!(fund > 0);
    assert_eq!(cost.total, fund + NAME_FEE_BOUND);

    // The quote is the cost.
    let quote = f
        .engine
        .block_on(DashPay::name_grant(
            &FakeNet::available(true),
            &valid_label("alice").unwrap(),
        ))
        .unwrap();
    assert_eq!(
        quote,
        GrantRequest {
            max_duffs: 0,
            max_credits: cost.total
        }
    );

    // A grant, or a balance, that covers only the fund.
    let net = Arc::new(FakeNet::available(true));
    let grant = f.grant(fund);
    assert_eq!(
        f.register(&net, "alice", &grant).unwrap_err().code(),
        "platform.grant_exceeded"
    );
    assert!(f.grant_unused(&grant));
    let fund_only = Arc::new(FakeNet::available(true).balance(fund));
    let grant = f.grant(cost.total);
    assert_eq!(
        f.register(&fund_only, "alice", &grant).unwrap_err().code(),
        "platform.insufficient_credits"
    );
    assert!(f.grant_unused(&grant));
    assert_eq!(net.submits() + fund_only.submits(), 0);

    // Both covered: the contest starts and is recorded.
    let both = Arc::new(FakeNet::available(true).balance(cost.total).ending(9_000));
    assert_eq!(
        f.register(&both, "alice", &grant),
        Ok(NameOutcome::ContestStarted {
            ends_at: Some(9_000)
        })
    );
    assert_eq!(f.labels(), (vec!["carol".into()], vec!["a11ce".into()]));
    let prefs = f.prefs();
    assert_eq!(prefs.contested.as_deref(), Some("alice"));
    assert!(prefs.pending.is_empty());
}

/// A failed write spends the grant: a retry needs a new one, whose cap is
/// held against the cost again.
#[test]
fn a_retry_after_a_failed_write_needs_a_new_grant() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let net = Arc::new(FakeNet::available(false).failing());
    let grant = f.grant(NAME_FEE_BOUND);
    assert_eq!(
        f.register(&net, "alice2", &grant).unwrap_err().code(),
        "platform.unavailable"
    );
    assert_eq!(net.submits(), 1);
    assert_eq!(
        f.register(&net, "alice2", &grant).unwrap_err().code(),
        "platform.grant_invalid"
    );
    let short = f.grant(NAME_FEE_BOUND - 1);
    assert_eq!(
        f.register(&net, "alice2", &short).unwrap_err().code(),
        "platform.grant_exceeded"
    );
    assert_eq!(net.submits(), 1);
    // The write did not land, so the intent stays pending, and a read
    // leaves it so.
    assert_eq!(f.prefs().pending, [(NameKind::Extra, "alice2".to_string())]);
    assert_eq!(f.main_name().as_deref(), Some("carol"));
    assert_eq!(f.prefs().pending.len(), 1);
}

/// Review DP1-03 R2: joining another identity's contest needs its deadline.
#[test]
fn an_unknown_join_deadline_spends_nothing() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let grant = f.grant(u64::MAX);
    let unknown = Arc::new(FakeNet::new(vec![contest_with(vec![id(2)], None)]));
    assert_eq!(
        f.register(&unknown, "alice", &grant).unwrap_err().code(),
        "platform.unavailable"
    );
    let closed = Arc::new(FakeNet::new(vec![contest_with(vec![id(2)], Some(1))]));
    assert_eq!(
        f.register(&closed, "alice", &grant).unwrap_err().code(),
        "name.contest_open"
    );
    assert_eq!(unknown.submits() + closed.submits(), 0);
    assert!(f.grant_unused(&grant));
    assert!(f.prefs().pending.is_empty());

    // An open window, far off: the join goes out.
    let later = crate::events::unix_now() + 365 * 24 * 3600;
    let open = Arc::new(FakeNet::new(vec![contest_with(vec![id(2)], Some(later))]));
    assert!(matches!(
        f.register(&open, "alice", &grant),
        Ok(NameOutcome::ContestStarted { .. })
    ));
    assert_eq!(open.submits(), 1);
}

/// An identity that owns `carol2` and contends for `alice`.
fn contending(dir: &Path) -> Fixture {
    let f = Fixture::signing(dir, &["carol2"]);
    f.contend("alice");
    assert_eq!(f.main_name().as_deref(), Some("carol2"));
    f
}

/// Review DP1-03 R3: every path that observes a temporary name records it.
#[test]
fn a_temporary_name_is_recorded_on_every_path() {
    // The library listed it, then the process stopped: a retry answers
    // from the wallet, with no network and no grant.
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    f.set_pending(&[(NameKind::Temporary, "alice2")]);
    f.sync_names(&["carol2", "alice2"], DpnsFetch::Partial);
    let net = Arc::new(offline());
    assert_eq!(
        f.register(&net, "alice2", "no grant"),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(f.main_name().as_deref(), Some("alice2"));

    // Platform has it but the wallet does not: Taken by this identity.
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    f.set_pending(&[(NameKind::Temporary, "alice2")]);
    let net = Arc::new(FakeNet::new(vec![Lookup::verdict(
        NameAvailability::Taken { owner: me() },
    )]));
    assert_eq!(
        f.register(&net, "alice2", "no grant"),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(f.labels().0, ["carol2", "alice2"]);
    assert_eq!(f.main_name().as_deref(), Some("alice2"));
    assert!(f.prefs().pending.is_empty());

    // A name the identity got some other way is not its temporary name.
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    assert_eq!(
        f.register(&net, "alice2", "no grant"),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(f.prefs().temporary, None);
    assert_eq!(f.main_name().as_deref(), Some("carol2"));

    // The write landed but its confirmation failed.
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    let net = Arc::new(
        FakeNet::new(vec![
            Lookup::verdict(NameAvailability::Available { contested: false }),
            Lookup::verdict(NameAvailability::Taken { owner: me() }),
        ])
        .failing(),
    );
    let grant = f.grant(NAME_FEE_BOUND);
    assert_eq!(
        f.register(&net, "alice2", &grant),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(net.submits(), 1);
    assert_eq!(f.main_name().as_deref(), Some("alice2"));
    assert!(f.prefs().pending.is_empty());

    // Another identity took it meanwhile: nothing stays pending.
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    let net = Arc::new(
        FakeNet::new(vec![
            Lookup::verdict(NameAvailability::Available { contested: false }),
            Lookup::verdict(NameAvailability::Taken { owner: b58(2) }),
        ])
        .failing(),
    );
    let grant = f.grant(NAME_FEE_BOUND);
    assert_eq!(f.register(&net, "alice2", &grant), Err(NameError::Taken));
    assert!(f.prefs().pending.is_empty());
    assert_eq!(f.main_name().as_deref(), Some("carol2"));
}

/// Review DP1-03 R3: every path that observes a contest records it.
#[test]
fn a_contest_is_recorded_on_every_path() {
    let recorded = |f: &Fixture| {
        assert_eq!(f.labels(), (vec!["carol".into()], vec!["a11ce".into()]));
        assert_eq!(f.prefs().contested.as_deref(), Some("alice"));
        assert!(f.prefs().pending.is_empty());
        assert_eq!(f.main_name().as_deref(), Some("carol"));
    };
    // Platform lists the identity as a contender: no second join.
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let net = Arc::new(FakeNet::new(vec![contest_with(
        vec![id(2), IDENTITY.into()],
        Some(5),
    )]));
    let grant = f.grant(u64::MAX);
    assert_eq!(
        f.register(&net, "alice", &grant),
        Ok(NameOutcome::ContestStarted { ends_at: Some(5) })
    );
    assert_eq!(net.submits(), 0);
    assert!(f.grant_unused(&grant));
    recorded(&f);

    // The join landed but its confirmation failed.
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    let net = Arc::new(
        FakeNet::new(vec![
            Lookup::verdict(NameAvailability::Available { contested: true }),
            contest_with(vec![IDENTITY.into()], Some(5)),
        ])
        .failing(),
    );
    let grant = f.grant(u64::MAX);
    assert_eq!(
        f.register(&net, "alice", &grant),
        Ok(NameOutcome::ContestStarted { ends_at: Some(5) })
    );
    assert_eq!(net.submits(), 1);
    recorded(&f);

    // A contest the identity won shows as its name.
    let net = Arc::new(FakeNet::new(vec![Lookup::verdict(
        NameAvailability::Taken { owner: me() },
    )]));
    assert_eq!(
        f.register(&net, "alice", "no grant"),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(f.labels(), (vec!["carol".into(), "alice".into()], vec![]));
    assert_eq!(f.main_name().as_deref(), Some("alice"));
}

/// Review DP1-03 R3: a registration stopped between the library's record
/// and the engine's is settled by the next read, after a restart too, each
/// of several.
#[test]
fn registrations_cut_short_are_settled_by_the_next_read() {
    let dir = dw_testutil::private_tempdir();
    let mut f = Fixture::signing(dir.path(), &["carol2"]);
    // A contested join and then the temporary name returned; the process
    // stopped before either was recorded. The contest shows once the
    // library's contest sweep lists it.
    f.set_pending(&[
        (NameKind::Contested, "alice"),
        (NameKind::Temporary, "alice2"),
        (NameKind::Extra, "never-landed"),
    ]);
    f.sync_names(&["carol2", "alice2"], DpnsFetch::Partial);
    f.with_identity(|m, p| m.add_contested_dpns_name("a11ce".into(), p));
    f.reopen();
    assert_eq!(f.main_name().as_deref(), Some("alice2"));
    let prefs = f.prefs();
    assert_eq!(prefs.temporary.as_deref(), Some("alice2"));
    assert_eq!(prefs.contested.as_deref(), Some("alice"));
    // A write that never showed stays pending.
    assert_eq!(
        prefs.pending,
        [(NameKind::Extra, "never-landed".to_string())]
    );
}

/// A contested label the library lists may be a running contest (its
/// write returned) or a won one (a sync found the domain): only a
/// marketplace row, or Platform on a retry, tells them apart.
#[test]
fn a_listed_contested_label_is_settled_only_on_evidence() {
    // Listed, no row: left as it is, then a retry asks Platform.
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    f.set_pending(&[(NameKind::Contested, "alice")]);
    f.sync_names(&["carol", "alice"], DpnsFetch::Partial);
    let _ = f.main_name();
    assert_eq!(f.labels(), (vec!["carol".into(), "alice".into()], vec![]));
    assert_eq!(f.prefs().pending.len(), 1);
    let net = Arc::new(FakeNet::new(vec![contest_with(
        vec![IDENTITY.into()],
        Some(5),
    )]));
    assert_eq!(
        f.register(&net, "alice", "no grant"),
        Ok(NameOutcome::ContestStarted { ends_at: Some(5) })
    );
    assert_eq!(f.labels(), (vec!["carol".into()], vec!["a11ce".into()]));
    assert_eq!(f.main_name().as_deref(), Some("carol"));
    assert!(f.prefs().pending.is_empty());

    // Listed with a row: the contest was won before the read.
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    f.set_pending(&[(NameKind::Contested, "alice")]);
    f.sync_names(&["carol", "alice"], DpnsFetch::Partial);
    f.give_rows(vec![row(
        1,
        "alice",
        DpnsNameSaleStatus::Owned,
        Some(9),
        None,
    )]);
    assert_eq!(f.main_name().as_deref(), Some("alice"));
    assert_eq!(f.labels(), (vec!["carol".into(), "alice".into()], vec![]));
    assert_eq!(f.prefs().contested.as_deref(), Some("alice"));
    assert!(f.prefs().pending.is_empty());
}

/// Review F5: asking for a contested name the identity already owns
/// changes no pref.
#[test]
fn an_owned_contested_name_is_not_a_won_contest() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol", "alice"]);
    f.contend("bob");
    f.set_pref(PREF_CONTESTED_NAME, "bob");
    let net = Arc::new(offline());
    assert_eq!(
        f.register(&net, "alice", "no grant"),
        Ok(NameOutcome::Registered)
    );
    assert_eq!(f.prefs().contested.as_deref(), Some("bob"));
    assert_eq!(f.main_name().as_deref(), Some("carol"));
}

/// One identity's registrations run one at a time, and a read does not
/// settle what a registration under way will.
#[test]
fn registrations_of_one_identity_are_serial() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    f.set_pending(&[(NameKind::Extra, "alice2")]);
    f.sync_names(&["carol", "alice2"], DpnsFetch::Partial);
    let lock = identity_lock(f.wallet, IDENTITY.into());
    let held = f.engine.block_on(Arc::clone(&lock).lock_owned());
    assert_eq!(f.main_name().as_deref(), Some("carol"));
    assert_eq!(f.prefs().pending.len(), 1);

    let ask = Registration {
        wallet_id: f.wallet,
        identity: IDENTITY.into(),
        label: "alice2".into(),
        check: valid_label("alice2").unwrap(),
    };
    let (session, dp) = (Arc::clone(&f.session), f.dp());
    let call = f.session.rt.spawn(async move {
        dp.on_wallet(move |_, _, wallet| async move {
            DashPay::register(&offline(), &session, &wallet, ask, "no grant".into()).await
        })
        .await
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!call.is_finished());
    drop(held);
    assert_eq!(
        f.engine.block_on(call).unwrap(),
        Ok(NameOutcome::Registered)
    );
    assert!(f.prefs().pending.is_empty());
}

/// Settling never writes the user's pick.
#[test]
fn settling_keeps_the_pick() {
    let dir = dw_testutil::private_tempdir();
    let f = contending(dir.path());
    f.engine
        .block_on(f.dp().set_main_name(identity_id(), Some("carol2".into())))
        .unwrap();
    let net = Arc::new(FakeNet::available(false));
    let grant = f.grant(NAME_FEE_BOUND);
    assert_eq!(
        f.register(&net, "alice2", &grant),
        Ok(NameOutcome::Registered)
    );
    let prefs = f.prefs();
    assert_eq!(prefs.pick.as_deref(), Some("carol2"));
    assert_eq!(prefs.temporary.as_deref(), Some("alice2"));
    assert_eq!(f.main_name().as_deref(), Some("carol2"));
}

/// Review r2: a retry Platform refuses for good drops its pending intent,
/// which could otherwise mark a name got later some other way.
#[test]
fn a_refused_retry_drops_its_pending_intent() {
    let dir = dw_testutil::private_tempdir();
    let f = Fixture::signing(dir.path(), &["carol"]);
    for (label, found) in [
        (
            "alice",
            Lookup::verdict(NameAvailability::Taken { owner: b58(2) }),
        ),
        ("dasher", Lookup::verdict(NameAvailability::Locked)),
        ("bob", contest_with(vec![id(2)], Some(1))),
    ] {
        f.set_pending(&[(NameKind::Contested, label)]);
        let net = Arc::new(FakeNet::new(vec![found]));
        assert!(f.register(&net, label, "no grant").is_err());
        assert!(f.prefs().pending.is_empty(), "{label}");
        assert_eq!(net.submits(), 0);
    }
}
