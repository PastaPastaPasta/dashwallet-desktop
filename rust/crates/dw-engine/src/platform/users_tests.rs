//! User search, lookup, links and scans (DP2-03) over a session whose
//! Platform answers are scripted, then the testnet fixture (ignored): an
//! iOS-made link verifies, an expired `dapk` is refused, a valid one leaves
//! the send its proof.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dpp::util::strings::convert_to_homograph_safe_chars;
use platform_wallet::AutoAcceptProofSource;
use platform_wallet::wallet::identity::DpnsNameInfo;
use platform_wallet::wallet::identity::crypto::auto_accept::{
    encode_auto_accept_key_blob, encode_dashpay_contact_uri, sign_auto_accept_proof,
    verify_auto_accept_proof_with_pubkey,
};
use platform_wallet::{ContactRequest, EstablishedContact};
use zeroize::Zeroizing;

use super::contacts::{Relation, ScannedCode, ScannedContact};
use super::dashpay::{BearerSecret, DashPay};
use super::errors::PlatformError;
use super::journal_tests::{Recorder, engine, session};
use super::names::UserHit;
use super::reverify::{Held, ShownProfile};
use super::runtime::guard;
use super::trust::{self, BoxFuture};
use super::users::UserNet;
use super::{SpvState, recovery_tests};
use crate::events::unix_now;
use crate::{
    DashNetwork, Engine, EventSink, ImportOptions, NetworkSession, SessionOptions, WalletId,
};

const ME: u8 = 1;
const BOB: u8 = 2;
const CAROL: u8 = 3;
const DAVE: u8 = 4;
const EVE: u8 = 5;
const FRANK: u8 = 6;
/// Our other identity.
const OTHER: u8 = 7;

fn id(n: u8) -> Identifier {
    Identifier::from([n; 32])
}

fn b58(n: u8) -> String {
    id(n).to_string(Encoding::Base58)
}

fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        id(sender),
        id(recipient),
        0,
        0,
        0,
        vec![1; 96],
        1,
        created_at,
    )
}

fn profile(display_name: &str) -> ShownProfile {
    ShownProfile {
        display_name: Some(display_name.into()),
        avatar_url: Some(format!("https://example.com/{display_name}.png")),
        ..ShownProfile::default()
    }
}

/// Platform as a test scripts it: domains (label as spelled, its identity),
/// profiles, the quorum lookup each fetch's proofs make (verified or not;
/// none if `None`), and every question asked.
#[derive(Default)]
struct FakeNet {
    names: Vec<(String, Identifier)>,
    profiles: HashMap<Identifier, ShownProfile>,
    failing_profiles: bool,
    names_lookup: Option<bool>,
    profiles_lookup: Option<bool>,
    asked: Mutex<Vec<String>>,
    /// The next domain lookup stops until the test lets it go on.
    pause: Mutex<Option<Pause>>,
}

/// A lookup's stop: it says it got there, then waits to be let go.
struct Pause {
    reached: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

impl FakeNet {
    fn ask(&self, q: String) {
        self.asked.lock().unwrap().push(q);
    }
}

/// `out`, after the quorum lookup a proof would make (in the fetch's scope).
fn answer<T: Send + 'static>(lookup: Option<bool>, out: T) -> BoxFuture<T> {
    Box::pin(async move {
        if let Some(verified) = lookup {
            trust::note_lookup(verified);
        }
        out
    })
}

impl UserNet for FakeNet {
    fn search(
        &self,
        prefix: String,
        limit: u32,
    ) -> BoxFuture<Result<Vec<(String, Identifier)>, PlatformError>> {
        self.ask(format!("search {prefix} {limit}"));
        let mut found: Vec<_> = (self.names.iter())
            .filter(|(l, _)| convert_to_homograph_safe_chars(l).starts_with(&prefix))
            .cloned()
            .collect();
        found.sort_by_key(|(l, _)| convert_to_homograph_safe_chars(l));
        found.truncate(limit as usize);
        answer(self.names_lookup, Ok(found))
    }

    fn domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        self.ask(format!("domain {label}"));
        let found = (self.names.iter())
            .find(|(l, _)| convert_to_homograph_safe_chars(l) == label)
            .cloned();
        let found = answer(self.names_lookup, Ok(found));
        match self.pause.lock().unwrap().take() {
            None => found,
            Some(pause) => Box::pin(async move {
                pause.reached.send(()).unwrap();
                pause.resume.await.unwrap();
                found.await
            }),
        }
    }

    fn profiles(
        &self,
        ids: Vec<Identifier>,
    ) -> BoxFuture<Result<Vec<(Identifier, ShownProfile)>, PlatformError>> {
        self.ask(format!("profiles {}", ids.len()));
        let found = if self.failing_profiles {
            Err(PlatformError::Unavailable)
        } else {
            Ok((ids.into_iter())
                .filter_map(|id| Some((id, self.profiles.get(&id)?.clone())))
                .collect())
        };
        answer(self.profiles_lookup, found)
    }
}

/// An offline session with an imported wallet holding our identity, named
/// `me1`, with Bob a contact, a request out to Carol, one in from Eve and
/// Frank ignored, and our other identity; Platform knows `me1`, `Bob`,
/// `b0bby` (Carol), `dave`, `eve`, `frank` and `other`, and profiles for
/// Bob and Dave.
struct Fixture {
    engine: Engine,
    s: Arc<NetworkSession>,
    wallet: WalletId,
    net: Arc<FakeNet>,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with(FakeNet {
            names: vec![
                ("me1".into(), id(ME)),
                ("Bob".into(), id(BOB)),
                ("b0bby".into(), id(CAROL)),
                ("dave".into(), id(DAVE)),
                ("eve".into(), id(EVE)),
                ("frank".into(), id(FRANK)),
                ("other".into(), id(OTHER)),
            ],
            profiles: HashMap::from([(id(BOB), profile("Bob")), (id(DAVE), profile("Dave"))]),
            ..FakeNet::default()
        })
    }

    fn with(net: FakeNet) -> Self {
        Self::named(net, &["me1"])
    }

    fn named(net: FakeNet, names: &[&str]) -> Self {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(
            dir.path(),
            Arc::new(Recorder::default()) as Arc<dyn EventSink>,
        );
        let (s, wallet) = session(&engine, true);
        let net = Arc::new(net);
        *guard(&s.platform.users.net) = Some(Arc::clone(&net) as Arc<dyn UserNet>);
        let f = Self {
            engine,
            s,
            wallet,
            net,
            _dir: dir,
        };
        f.give_me(names);
        f
    }

    /// Our identity, owning `names`, with Bob a contact, Carol asked, Eve
    /// asking and Frank ignored; then our other identity.
    fn give_me(&self, names: &[&str]) {
        let manager = self.s.manager().unwrap();
        let (wallet_id, names) = (self.wallet, names.iter().map(|n| n.to_string()));
        let names: Vec<DpnsNameInfo> = names
            .map(|label| DpnsNameInfo {
                label,
                acquired_at: Some(1_000),
            })
            .collect();
        self.engine.block_on(async move {
            let wallet = manager.get_wallet(&wallet_id.0).await.unwrap();
            let mut state = wallet.state_mut().await;
            let identity = |n| {
                dpp::identity::Identity::V0(dpp::identity::v0::IdentityV0 {
                    id: id(n),
                    public_keys: Default::default(),
                    balance: 0,
                    revision: 0,
                })
            };
            let persister = wallet.persister();
            for (index, n) in [(0, ME), (1, OTHER)] {
                (state.identity_manager)
                    .add_identity(identity(n), index, wallet_id.0, persister)
                    .unwrap();
            }
            let me = state
                .identity_manager
                .managed_identity_mut(&id(ME))
                .unwrap();
            me.dpns_names = names;
            me.apply_established_contact(EstablishedContact::new(
                id(BOB),
                request(ME, BOB, 1_000),
                request(BOB, ME, 2_000),
            ));
            me.add_sent_contact_request(request(ME, CAROL, 3_000), persister)
                .unwrap();
            me.add_incoming_contact_request(request(EVE, ME, 4_000), persister)
                .unwrap();
            me.ignore_sender(&id(FRANK));
        });
    }

    fn dp(&self) -> Arc<DashPay> {
        self.s.dashpay(self.wallet)
    }

    fn asked(&self) -> Vec<String> {
        std::mem::take(&mut *self.net.asked.lock().unwrap())
    }

    fn search(&self, prefix: &str, limit: u32) -> Result<Vec<UserHit>, String> {
        let dp = self.dp();
        (self.engine)
            .block_on(dp.search_users(prefix.into(), limit))
            .map_err(|e| e.code().to_owned())
    }

    fn scan(&self, text: &str) -> Result<ScannedContact, String> {
        let dp = self.dp();
        (self.engine)
            .block_on(dp.verify_scanned(BearerSecret::new(text.into())))
            .map_err(|e| e.code().to_owned())
    }
}

fn hit(n: u8, username: &str, display_name: Option<&str>, relation: Relation) -> UserHit {
    UserHit {
        identity: b58(n),
        username: username.into(),
        display_name: display_name.map(Into::into),
        avatar_url: display_name.map(|d| format!("https://example.com/{d}.png")),
        relation,
        unverified: false,
    }
}

/// Only the normalized prefix goes out; hits come in Platform's order with
/// their profiles and their relations to the main identity.
#[test]
fn search_shows_profiles_and_relations() {
    let f = Fixture::new();
    assert_eq!(
        f.search(" BO", 10).unwrap(),
        vec![
            hit(BOB, "Bob", Some("Bob"), Relation::Contact),
            hit(CAROL, "b0bby", None, Relation::PendingOutgoing),
        ]
    );
    assert_eq!(f.asked(), ["search b0 10", "profiles 2"]);
    assert_eq!(
        f.search("me", 1000).unwrap(),
        vec![hit(ME, "me1", None, Relation::IsSelf)]
    );
    assert_eq!(f.asked(), ["search me 100", "profiles 1"]);
    assert_eq!(
        f.search("d", 1).unwrap(),
        vec![hit(DAVE, "dave", Some("Dave"), Relation::Stranger)]
    );
    // No name found: no profiles asked for.
    f.asked();
    assert_eq!(f.search("zz", 10).unwrap(), vec![]);
    assert_eq!(f.asked(), ["search zz 10"]);

    // Nothing a label could start with: no question at all.
    f.asked();
    assert_eq!(f.search("a b", 10).unwrap(), vec![]);
    assert_eq!(f.search("bob.dash", 10).unwrap(), vec![]);
    assert_eq!(f.asked(), Vec::<String>::new());
    assert_eq!(f.search("  ", 10).unwrap_err(), "invalid_argument");
    assert_eq!(f.search("bo", 0).unwrap_err(), "invalid_argument");
    let stranger = f.s.dashpay(WalletId([9; 32]));
    let other = f.engine.block_on(stranger.search_users("bo".into(), 10));
    assert_eq!(other.unwrap_err().code(), "wallet_not_found");
}

/// A profile fetch that fails shows the hit without one.
#[test]
fn a_failed_profile_fetch_shows_the_hit_alone() {
    let f = Fixture::with(FakeNet {
        names: vec![("Bob".into(), id(BOB))],
        profiles: HashMap::from([(id(BOB), profile("Bob"))]),
        failing_profiles: true,
        ..FakeNet::default()
    });
    assert_eq!(
        f.search("bob", 10).unwrap(),
        vec![hit(BOB, "Bob", None, Relation::Contact)]
    );
}

/// One query fetches the profiles, once per identity.
#[test]
fn profiles_come_in_one_query() {
    let f = Fixture::with(FakeNet {
        names: vec![("Bob".into(), id(BOB)), ("b0bby".into(), id(BOB))],
        profiles: HashMap::from([(id(BOB), profile("Bob"))]),
        ..FakeNet::default()
    });
    assert_eq!(
        f.search("bo", 10).unwrap(),
        vec![
            hit(BOB, "Bob", Some("Bob"), Relation::Contact),
            hit(BOB, "b0bby", Some("Bob"), Relation::Contact),
        ]
    );
    assert_eq!(f.asked(), ["search b0 10", "profiles 1"]);
}

/// Relations are to the main identity, whatever the others hold.
#[test]
fn relations_are_the_main_identitys() {
    let f = Fixture::new();
    let relation = |name: &str| {
        let dp = f.dp();
        let found = f.engine.block_on(dp.resolve_user(name.into())).unwrap();
        found.unwrap().relation
    };
    assert_eq!(relation("me1"), Relation::IsSelf);
    assert_eq!(relation("bob"), Relation::Contact);
    assert_eq!(relation("b0bby"), Relation::PendingOutgoing);
    assert_eq!(relation("eve"), Relation::PendingIncoming);
    assert_eq!(relation("frank"), Relation::Ignored);
    assert_eq!(relation("dave"), Relation::Stranger);
    assert_eq!(relation("other"), Relation::Stranger);
}

/// Once the fallback is latched, a hit's label is unverified unless the
/// names' fetch verified, and its profile unless the profiles' fetch did
/// (no records here).
#[test]
fn hits_follow_their_fetches() {
    let unverified = |names_lookup, profiles_lookup, with_profile: bool| {
        let profiles = HashMap::from([(id(BOB), profile("Bob"))]);
        let f = Fixture::with(FakeNet {
            names: vec![("Bob".into(), id(BOB))],
            profiles: if with_profile {
                profiles
            } else {
                HashMap::new()
            },
            names_lookup,
            profiles_lookup,
            ..FakeNet::default()
        });
        f.s.live().unwrap().provenance.fallback_in_use().unwrap();
        let dp = f.dp();
        let resolved = f.engine.block_on(dp.resolve_user("bob".into()));
        let searched = f.search("bob", 1).unwrap()[0].unverified;
        assert_eq!(resolved.unwrap().unwrap().unverified, searched);
        searched
    };
    let (yes, no) = (Some(true), Some(false));
    assert!(!unverified(yes, yes, true));
    assert!(
        unverified(yes, no, true),
        "the profile's fetch did not verify"
    );
    assert!(unverified(no, yes, true), "the names' fetch did not verify");
    assert!(
        unverified(yes, None, true),
        "a fetch with no proof verifies nothing"
    );
    // No profile shown: only the label counts.
    assert!(!unverified(yes, no, false));
    assert!(unverified(no, yes, false));
}

/// An exact lookup: one `.dash` and case do not matter; a label DPNS could
/// not hold asks nothing.
#[test]
fn resolve_reads_one_name() {
    let f = Fixture::new();
    let resolve = |name: &str| {
        let dp = f.dp();
        f.engine.block_on(dp.resolve_user(name.into())).unwrap()
    };
    let bob = Some(hit(BOB, "Bob", Some("Bob"), Relation::Contact));
    assert_eq!(resolve(" BOB.Dash "), bob);
    assert_eq!(resolve("b0b"), bob);
    assert_eq!(
        f.asked(),
        ["domain b0b", "profiles 1", "domain b0b", "profiles 1"]
    );
    assert_eq!(resolve("nobody"), None);
    assert_eq!(f.asked(), ["domain n0b0dy"]);
    for unheld in ["ab", "a b", ".dash", "-bob", "bob.dash.dash"] {
        assert_eq!(resolve(unheld), None, "{unheld}");
    }
    assert_eq!(f.asked(), Vec::<String>::new());
}

/// Once the fallback served a read, a hit is unverified until a record
/// covers its label and the profile it shows (no fetch here verifies).
#[test]
fn hits_follow_provenance() {
    let f = Fixture::new();
    let unverified = || f.search("bob", 1).unwrap()[0].unverified;
    assert!(!unverified());
    let provenance = f.s.live().unwrap().provenance;
    provenance.fallback_in_use().unwrap();
    assert!(unverified());
    let label = Held::label("Bob", id(BOB)).entity();
    provenance.record_verified(&label).unwrap();
    assert!(unverified(), "the profile shown is not on record");
    let shown = Held::Profile {
        id: id(BOB),
        shown: profile("Bob"),
    };
    provenance.record_verified(&shown.entity()).unwrap();
    assert!(!unverified());
    // Another spelling of the same name is other content.
    let f2 = Fixture::with(FakeNet {
        names: vec![("B0b".into(), id(BOB))],
        ..FakeNet::default()
    });
    let provenance2 = f2.s.live().unwrap().provenance;
    provenance2.fallback_in_use().unwrap();
    provenance2.record_verified(&label).unwrap();
    assert!(f2.search("bob", 1).unwrap()[0].unverified);
}

/// The own link is iOS's, with the main name; no name, no link.
#[test]
fn my_user_link_uses_the_main_name() {
    let f = Fixture::new();
    let dp = f.dp();
    assert_eq!(
        dp.my_user_link(b58(ME)).unwrap(),
        format!("dashpay://user?id={}&username=me1", b58(ME))
    );
    assert_eq!(
        dp.my_user_link(b58(BOB)).unwrap_err().code(),
        "identity.not_found"
    );
    assert_eq!(
        dp.my_user_link("not base58!".into()).unwrap_err().code(),
        "invalid_argument"
    );
    let nameless = Fixture::named(FakeNet::default(), &[]);
    let link = nameless.dp().my_user_link(b58(ME));
    assert_eq!(link.unwrap_err().code(), "contact.no_username");

    // While a pass holds the identities, no answer is final.
    let wm = f.s.manager().unwrap().wallet_manager_arc();
    let held = f.engine.block_on(wm.write());
    for who in [ME, BOB] {
        let link = dp.my_user_link(b58(who));
        assert_eq!(link.unwrap_err().code(), "platform.unavailable");
    }
    drop(held);
}

/// The iOS-made links (`testdata/dashpay_user_links.json`), as iOS verifies
/// a scan: the link's username must belong to the link's identity now.
#[test]
fn ios_made_links_verify() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/dashpay_user_links.json"
    );
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let links = fixture["links"].as_array().unwrap();
    let owner = |link: &serde_json::Value| {
        let text = link["identity"].as_str().unwrap();
        Identifier::from_string(text, Encoding::Base58).unwrap()
    };
    let label = |link: &serde_json::Value| {
        let given = link["username_given"].as_str().unwrap();
        given.strip_suffix(".dash").unwrap_or(given).to_owned()
    };
    let names = links.iter().map(|l| (label(l), owner(l))).collect();
    let f = Fixture::with(FakeNet {
        names,
        ..FakeNet::default()
    });
    for link in links {
        let scanned = f.scan(link["uri"].as_str().unwrap()).unwrap();
        assert_eq!(
            scanned,
            ScannedContact {
                identity: owner(link).to_string(Encoding::Base58),
                username: Some(label(link)),
                display_name: None,
                code: ScannedCode::UserLink,
                relation: Relation::Stranger,
                unverified: false,
            }
        );
    }
    // One's identity with the other's name: someone else holds it.
    let (a, b) = (&links[0], &links[1]);
    let swapped = format!(
        "dashpay://user?id={}&username={}",
        owner(a).to_string(Encoding::Base58),
        label(b)
    );
    assert_eq!(f.scan(&swapped).unwrap_err(), "contact.scan_unverified");
}

#[test]
fn a_user_link_must_match_platform() {
    let f = Fixture::new();
    let link = |n: u8, name: &str| format!("dashpay://user?id={}&username={name}", b58(n));
    let bob = f.scan(&link(BOB, "bob.dash")).unwrap();
    assert_eq!(bob.identity, b58(BOB));
    assert_eq!(bob.username.as_deref(), Some("Bob"), "Platform's spelling");
    assert_eq!(bob.display_name.as_deref(), Some("Bob"));
    assert_eq!(
        (bob.relation, bob.code),
        (Relation::Contact, ScannedCode::UserLink)
    );
    assert_eq!(f.scan(&link(ME, "me1")).unwrap().relation, Relation::IsSelf);
    assert_eq!(
        f.scan(&link(CAROL, "bob")).unwrap_err(),
        "contact.scan_unverified"
    );
    assert_eq!(
        f.scan(&link(BOB, "nobody")).unwrap_err(),
        "contact.scan_unverified"
    );
    // The link drops one `.dash`, as iOS does; nothing drops another.
    assert_eq!(
        f.scan(&link(BOB, "bob.dash.dash")).unwrap_err(),
        "contact.scan_unverified"
    );
    for junk in ["alice", "dash:XabcD?amount=1", "dashpay://invite?du=a", ""] {
        assert_eq!(f.scan(junk).unwrap_err(), "invalid_argument", "{junk}");
    }
}

fn key() -> SecretKey {
    SecretKey::from_secret_bytes([0x42; 32]).unwrap()
}

/// A DIP-15 code as platform-wallet writes one.
fn dapk(username: &str, expiry: u32) -> String {
    encode_dashpay_contact_uri(username, &encode_auto_accept_key_blob(&key(), expiry))
}

/// The scan a `dapk` code left for the send.
fn scan_id(scanned: &ScannedContact) -> String {
    match &scanned.code {
        ScannedCode::AutoAcceptClaim { scan, .. } => scan.clone(),
        ScannedCode::UserLink => panic!("not a contact code"),
    }
}

/// A valid code leaves a scan for the send; an expired one is refused
/// before anything is asked; a malformed one never shows its key.
#[test]
fn dapk_codes_are_checked() {
    let f = Fixture::new();
    let soon = (unix_now() + 3600) as u32;
    let bob = f.scan(&dapk("bob", soon)).unwrap();
    assert_eq!(bob.identity, b58(BOB));
    assert_eq!(bob.relation, Relation::Contact);
    assert_eq!(bob.username.as_deref(), Some("Bob"));
    let ScannedCode::AutoAcceptClaim { scan, expires_at } = bob.code else {
        panic!("no auto-accept claim");
    };
    assert_eq!(expires_at, u64::from(soon));
    assert!(
        scan.len() <= 64
            && scan
                .bytes()
                .all(|b| b"0123456789abcdefghijklmnopqrstuvwxyz_-".contains(&b))
    );
    assert_eq!(f.asked(), ["domain b0b", "profiles 1"]);
    // The parser drops one `.dash`; nothing drops another.
    assert!(f.scan(&dapk("BOB.dash", soon)).is_ok());
    let twice = f.scan(&dapk("bob.dash.dash", soon));
    assert_eq!(twice.unwrap_err(), "contact.scan_unverified");
    f.asked();

    let past = (unix_now() - 1) as u32;
    assert_eq!(
        f.scan(&dapk("bob", past)).unwrap_err(),
        "contact.scan_expired"
    );
    assert_eq!(f.asked(), Vec::<String>::new());
    assert_eq!(
        f.scan(&dapk("nobody", soon)).unwrap_err(),
        "contact.scan_unverified"
    );
    // Past 2^31 − 1 the owner cannot derive the key to verify the proof.
    assert_eq!(
        f.scan(&dapk("bob", 1 << 31)).unwrap_err(),
        "invalid_argument"
    );

    let mut blob = encode_auto_accept_key_blob(&key(), soon);
    blob[0] = 1;
    let other_type = encode_dashpay_contact_uri("bob", &blob);
    let dp = f.dp();
    let err = f
        .engine
        .block_on(dp.verify_scanned(BearerSecret::new(other_type.clone())));
    let err = err.unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    let dapk_value = other_type.split("dapk=").nth(1).unwrap();
    assert!(!format!("{err} {err:?}").contains(dapk_value));
}

/// DP2-02's seam: the scan's key signs a proof the code's key verifies, and
/// stays until the send spends it; a spent, unknown, other wallet's or
/// expired scan is `contact.scan_expired`.
#[test]
fn a_scan_gives_a_send_its_proof() {
    let f = Fixture::new();
    let soon = (unix_now() + 3600) as u32;
    let scan = || scan_id(&f.scan(&dapk("bob", soon)).unwrap());
    let intent = |to: u8, scan: Option<&str>| {
        (f.s)
            .send_request_intent(f.wallet, &b58(ME), &b58(to), scan)
            .map_err(|e| e.code())
    };
    let scans = &f.s.platform.users.scans;
    let claim = |wallet, scan: &str, now| {
        scans
            .claim(wallet, scan, id(BOB), now)
            .err()
            .map(|e| e.code())
    };

    let first = scan();
    assert_eq!(intent(CAROL, Some(&first)).err(), Some("invalid_argument"));
    // A send refused before it took the key releases the scan for a retry.
    drop(intent(BOB, Some(&first)).unwrap());
    let sent = intent(BOB, Some(&first)).unwrap();
    assert_eq!((sent.sender, sent.recipient), (id(ME), id(BOB)));
    let auto_accept = sent.auto_accept.unwrap();
    assert_eq!(
        (auto_accept.expiry, auto_accept.recipient_unverified),
        (soon, false)
    );
    let (source, spend) = auto_accept.proof_source(unix_now()).unwrap();
    let AutoAcceptProofSource::SignWithKey { secret_key, expiry } = source else {
        panic!("not signed with the scanned key");
    };
    assert_eq!((secret_key, expiry), (key(), soon));
    let account_reference = 7;
    let proof = sign_auto_accept_proof(&secret_key, &id(ME), &id(BOB), account_reference, expiry);
    let owner_key = PublicKey::from_secret_key(&key());
    assert!(verify_auto_accept_proof_with_pubkey(
        &owner_key,
        &proof,
        &id(ME),
        &id(BOB),
        account_reference
    ));
    assert_eq!(
        intent(BOB, Some(&first)).err(),
        Some("contact.request_pending")
    );
    // Once the key went out the scan is spent, whatever the send did.
    drop(spend);
    assert_eq!(
        intent(BOB, Some(&first)).err(),
        Some("contact.scan_expired")
    );

    let plain = intent(BOB, None).unwrap();
    assert!(plain.auto_accept.is_none());
    assert_eq!(
        intent(BOB, Some("scan-unknown")).err(),
        Some("contact.scan_expired")
    );
    let other_wallet = scan();
    assert_eq!(
        claim(WalletId([9; 32]), &other_wallet, 0),
        Some("contact.scan_expired")
    );
    // Expired by the time the send claims it: it goes.
    let late = scan();
    assert_eq!(
        claim(f.wallet, &late, u64::from(soon)),
        Some("contact.scan_expired")
    );
    assert_eq!(claim(f.wallet, &late, 0), Some("contact.scan_expired"));
    assert!(intent(BOB, Some(&other_wallet)).is_ok());
    // Expired by the time the send signs: no proof, and it goes.
    let crossing = scan();
    let held = scans.claim(f.wallet, &crossing, id(BOB), 0).ok().unwrap();
    let signed = held.proof_source(u64::from(soon));
    assert_eq!(signed.err().map(|e| e.code()), Some("contact.scan_expired"));
    assert_eq!(claim(f.wallet, &crossing, 0), Some("contact.scan_expired"));

    // A recipient Platform named without verified proof is flagged.
    f.s.live().unwrap().provenance.fallback_in_use().unwrap();
    let unverified = f.scan(&dapk("bob", soon)).unwrap();
    assert!(unverified.unverified);
    let sent = intent(BOB, Some(&scan_id(&unverified))).unwrap();
    assert!(sent.auto_accept.unwrap().recipient_unverified);
}

/// Sol r1 M2: one send at a time holds a scan, however many race for it,
/// so one scan cannot make two requests.
#[test]
fn one_send_at_a_time_claims_a_scan() {
    let f = Fixture::new();
    let scan = scan_id(&f.scan(&dapk("bob", (unix_now() + 3600) as u32)).unwrap());
    let scans = &f.s.platform.users.scans;
    let barrier = std::sync::Barrier::new(8);
    let claims: Vec<_> = std::thread::scope(|threads| {
        let racers: Vec<_> = (0..8)
            .map(|_| {
                threads.spawn(|| {
                    barrier.wait();
                    scans.claim(f.wallet, &scan, id(BOB), 0)
                })
            })
            .collect();
        racers.into_iter().map(|r| r.join().unwrap()).collect()
    });
    assert_eq!(claims.iter().filter(|c| c.is_ok()).count(), 1);
    for refused in claims.iter().filter_map(|c| c.as_ref().err()) {
        assert_eq!(refused.code(), "contact.request_pending");
    }
    drop(claims);
    // Released (no send went out): the next send may claim it.
    assert!(scans.claim(f.wallet, &scan, id(BOB), 0).is_ok());
}

/// Sol r2 M2: a claim makes one proof. `proof_source` consumes it (a second
/// call on one claim does not compile); the spend guard it returns keeps
/// the scan held, so no other claim can make another, and spends it when
/// it goes, a panic included.
#[test]
fn a_claim_makes_one_proof() {
    let f = Fixture::new();
    let scan = scan_id(&f.scan(&dapk("bob", (unix_now() + 3600) as u32)).unwrap());
    let scans = &f.s.platform.users.scans;
    let claim = |now| scans.claim(f.wallet, &scan, id(BOB), now);
    let (_, spend) = claim(0).ok().unwrap().proof_source(0).unwrap();
    for _ in 0..2 {
        let again = claim(0).err().map(|e| e.code());
        assert_eq!(again, Some("contact.request_pending"));
    }
    assert_eq!(scans.len(), 1);
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _spend = spend;
        panic!("the send panics after signing");
    }));
    assert!(unwound.is_err());
    assert_eq!(scans.len(), 0);
    let gone = claim(0).err().map(|e| e.code());
    assert_eq!(gone, Some("contact.scan_expired"));
}

/// Sol r1 M2: a lookup paused across an unload and reload of its wallet
/// keeps no key, and its scan is refused.
#[test]
fn a_scan_outlived_by_its_wallet_keeps_no_key() {
    let f = Fixture::new();
    let (reached, at_lookup) = tokio::sync::oneshot::channel();
    let (go_on, resume) = tokio::sync::oneshot::channel();
    *f.net.pause.lock().unwrap() = Some(Pause { reached, resume });
    let soon = (unix_now() + 3600) as u32;
    let scanned = std::thread::scope(|threads| {
        let scanning = threads.spawn(|| f.scan(&dapk("bob", soon)));
        at_lookup.blocking_recv().unwrap();
        // The wallet goes and comes back while the lookup waits.
        f.s.platform.forget(&f.wallet);
        go_on.send(()).unwrap();
        scanning.join().unwrap()
    });
    assert_eq!(scanned.unwrap_err(), "wallet_not_found");
    assert_eq!(f.s.platform.users.scans.len(), 0);
    // The reloaded wallet scans again.
    assert!(f.scan(&dapk("bob", soon)).is_ok());
}

/// Sol r1 M2: closing the session drops its scans, even with the session
/// handle and a claim kept: the claim has no key left to sign with.
#[test]
fn closing_the_session_drops_its_scans() {
    let f = Fixture::new();
    let soon = (unix_now() + 3600) as u32;
    let first = scan_id(&f.scan(&dapk("bob", soon)).unwrap());
    let second = scan_id(&f.scan(&dapk("bob", soon)).unwrap());
    let scans = Arc::clone(&f.s.platform.users.scans);
    let claim = scans.claim(f.wallet, &first, id(BOB), 0).ok().unwrap();
    f.engine.block_on(f.s.close());
    assert_eq!(scans.len(), 0);
    assert_eq!(
        claim.proof_source(0).err().map(|e| e.code()),
        Some("contact.scan_expired")
    );
    let taken = scans.claim(f.wallet, &second, id(BOB), 0);
    assert_eq!(taken.err().map(|e| e.code()), Some("contact.scan_expired"));
    assert_eq!(f.scan(&dapk("bob", soon)).unwrap_err(), "network_not_open");
}

/// Scans end with their wallet, the oldest goes once 16 wait (not one a
/// send holds), and a new scan drops the expired ones.
#[test]
fn scans_are_bounded_and_forgotten() {
    let f = Fixture::new();
    let soon = (unix_now() + 3600) as u32;
    let scans: Vec<String> = (0..17)
        .map(|_| scan_id(&f.scan(&dapk("bob", soon)).unwrap()))
        .collect();
    let store = &f.s.platform.users.scans;
    let key = |scan: &str| {
        let claim = store.claim(f.wallet, scan, id(BOB), 0);
        claim.map(drop).map_err(|e| e.code())
    };
    assert_eq!(key(&scans[0]), Err("contact.scan_expired"));
    assert_eq!(key(&scans[1]), Ok(()));
    assert_eq!(store.len(), 16);
    let held = store.claim(f.wallet, &scans[1], id(BOB), 0).ok().unwrap();
    for _ in 0..16 {
        f.scan(&dapk("bob", soon)).unwrap();
    }
    assert_eq!(store.len(), 17);
    let (_, spend) = held.proof_source(0).unwrap();
    drop(spend);
    assert_eq!(store.len(), 16);
    f.s.platform.forget(&f.wallet);
    assert_eq!(key(&scans[2]), Err("contact.scan_expired"));
    assert_eq!(store.len(), 0);

    let brief = (unix_now() + 1) as u32;
    f.scan(&dapk("bob", brief)).unwrap();
    wait_for("the brief scan to expire", Duration::from_secs(5), || {
        unix_now() >= u64::from(brief)
    });
    f.scan(&dapk("bob", soon)).unwrap();
    assert_eq!(store.len(), 1);
}

fn wait_for(what: &str, timeout: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// The testnet fixture's wallet A against public testnet: its own link is
/// the iOS-made one, B's iOS-made link verifies as A's contact, a swapped
/// one does not, search and lookup find both, an expired `dapk` for B is
/// refused and a valid one leaves the send a proof B's key verifies. The
/// key is a test key, not B's own `m/9'/1'/16'/expiry'` key: B's client
/// would not auto-accept (DP2-02's T2 sends with B's).
#[test]
#[ignore = "public testnet; set DW_DP203_TESTNET_FIXTURE"]
fn the_testnet_fixture_verifies_links_and_scans() {
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP203_TESTNET_FIXTURE").expect("DW_DP203_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let text = |v: &serde_json::Value| v.as_str().unwrap().to_owned();
    let (a_id, b_id) = (
        text(&meta["A"]["identity_id"]),
        text(&meta["B"]["identity_id"]),
    );
    let b_name = text(&meta["B"]["expected_main_name"]);
    let links: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../testdata/dashpay_user_links.json"
        ))
        .unwrap(),
    )
    .unwrap();
    let ios_link = |identity: &str| {
        let links = links["links"].as_array().unwrap();
        let link = links.iter().find(|l| l["identity"] == identity).unwrap();
        text(&link["uri"])
    };

    let dir = dw_testutil::private_tempdir();
    let engine = recovery_tests::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let phrase = Zeroizing::new(std::fs::read_to_string(fixture.join("A.mnemonic")).unwrap());
    let a = engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.trim().as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(birth),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    drop(phrase);
    let t0 = Instant::now();
    engine.block_on(s.start_spv()).unwrap();
    wait_for("SPV", Duration::from_secs(60), || {
        s.spv_state().unwrap() == SpvState::Running
    });
    let dp = s.dashpay(a);

    // 1. A's own link is byte for byte iOS's once the main name settles: the
    // bring-up may show the names before the names pass has Platform's
    // timestamps, and the link then names another of A's own names.
    let mut link = None;
    wait_for("A's link", Duration::from_secs(180), || {
        link = dp.my_user_link(a_id.clone()).ok();
        link.as_deref() == Some(ios_link(&a_id).as_str())
    });
    eprintln!("A's link after {:?}", t0.elapsed());

    // 2. B's iOS-made link verifies, as A's contact once the contacts are read.
    let scan = |text: String| engine.block_on(dp.verify_scanned(BearerSecret::new(text)));
    let mut b = None;
    wait_for("B as A's contact", Duration::from_secs(180), || {
        b = scan(ios_link(&b_id)).ok();
        b.as_ref().is_some_and(|b| b.relation == Relation::Contact)
    });
    let b = b.unwrap();
    eprintln!("B scanned after {:?}: {b:?}", t0.elapsed());
    assert_eq!(b.identity, b_id);
    assert_eq!(b.username.as_deref(), Some(b_name.as_str()));
    assert_eq!(b.code, ScannedCode::UserLink);
    let swapped = format!("dashpay://user?id={a_id}&username={b_name}");
    assert_eq!(scan(swapped).unwrap_err().code(), "contact.scan_unverified");

    // 3. Search and lookup.
    let hits = engine
        .block_on(dp.search_users("dwrecover".into(), 10))
        .unwrap();
    eprintln!("search: {hits:?}");
    let relation = |who: &str| hits.iter().find(|h| h.identity == who).map(|h| h.relation);
    assert_eq!(relation(&a_id), Some(Relation::IsSelf));
    assert_eq!(relation(&b_id), Some(Relation::Contact));
    let found = engine.block_on(dp.resolve_user(format!("{}.DASH", b_name.to_uppercase())));
    assert_eq!(found.unwrap().map(|h| h.identity), Some(b_id.clone()));

    // Platform takes the profiles' `In` query (a failure there would only
    // hide the profiles).
    let sdk = super::reverify::SdkRefetcher(s.manager().unwrap().sdk_arc());
    let ids = [&a_id, &b_id].map(|i| Identifier::from_string(i, Encoding::Base58).unwrap());
    let profiles = engine.block_on(sdk.profiles(ids.to_vec())).unwrap();
    eprintln!("profiles: {profiles:?}");

    // 4. `dapk`: expired refused; valid leaves the send a proof.
    let expired = dapk(&b_name, (unix_now() - 60) as u32);
    assert_eq!(scan(expired).unwrap_err().code(), "contact.scan_expired");
    let soon = (unix_now() + 3600) as u32;
    let scanned = scan(dapk(&b_name, soon)).unwrap();
    assert_eq!(scanned.identity, b_id);
    let mut intent = s
        .send_request_intent(a, &a_id, &b_id, Some(&scan_id(&scanned)))
        .unwrap();
    let auto_accept = intent.auto_accept.take().unwrap();
    let (source, _spend) = auto_accept.proof_source(unix_now()).unwrap();
    let AutoAcceptProofSource::SignWithKey { secret_key, expiry } = source else {
        panic!("not signed with the scanned key");
    };
    let (sender, recipient) = (intent.sender, intent.recipient);
    let proof = sign_auto_accept_proof(&secret_key, &sender, &recipient, 1, expiry);
    let owner_key = PublicKey::from_secret_key(&key());
    assert!(verify_auto_accept_proof_with_pubkey(
        &owner_key, &proof, &sender, &recipient, 1
    ));
    eprintln!("done after {:?}", t0.elapsed());
}
