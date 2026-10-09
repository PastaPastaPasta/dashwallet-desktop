//! Tests of names.rs (DP1-03): the main-name order, the temporary-name
//! policy, the contest join deadline, the vote-state verdicts and
//! registration steps, and, on a session with no network, a main name that
//! survives DPNS sync (platform #4978) and a restart.

use std::path::Path;
use std::sync::Arc;

use dpp::identity::Identity;
use dpp::identity::v0::IdentityV0;
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
    }
}

fn main_of(owned: &[&str], open: &[&str], prefs: MainNamePrefs) -> Option<String> {
    resolve_main_name(&names(owned), &contests(open), &prefs)
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
    assert_eq!(step(&contest(vec![id(2)]), None), Ok(None));
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
        f.give_identity(labels);
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

    fn give_identity(&self, labels: &[&str]) {
        let manager = self.session.manager().unwrap();
        let id = self.wallet;
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&id.0).await.unwrap();
            let identity = Identity::V0(IdentityV0 {
                id: IDENTITY.into(),
                public_keys: Default::default(),
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
