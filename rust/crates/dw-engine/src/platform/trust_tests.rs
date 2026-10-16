//! E0-10b's layered quorum provider over fake SPV and trusted sources and a
//! real app.sqlite (design §8): one test per rule, the latch refusal, the
//! status mapping, the scope tally and the post-sync re-fetch.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dash_context_provider::ContextProviderError;
use dashcore::sml::llmq_entry_verification::{
    LLMQEntryVerificationSkipStatus, LLMQEntryVerificationStatus,
};
use dashcore::sml::quorum_validation_error::QuorumValidationError;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::{IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::prelude::Identifier;
use dw_appdb::AppDb;
use platform_wallet::ContactRequest;

use super::errors::PlatformError;
use super::provenance::{Entity, Provenance, contact_request_key, kind};
use super::reverify::{Held, Refetcher, ShownProfile, confirmed, record_all, reverify};
use super::startup::QuorumSource;
use super::trust::{
    BoxFuture, SpvAnswer, SpvQuorums, TrustMode, TrustProvider, TrustedQuorums, is_trust_mismatch,
    scoped,
};
use crate::{DashNetwork, EngineEvent, EventSink, NoticeCode};

const A: (u32, [u8; 32]) = (6, [0xaa; 32]);
const B: (u32, [u8; 32]) = (6, [0xbb; 32]);
const C: (u32, [u8; 32]) = (6, [0xcc; 32]);
const REAL: [u8; 48] = [1; 48];
const FORGED: [u8; 48] = [2; 48];

type Quorum = (u32, [u8; 32]);

#[derive(Default)]
struct FakeSpv {
    answers: Mutex<HashMap<Quorum, SpvAnswer>>,
    calls: AtomicUsize,
    /// Lookups never answer, as behind an engine lock held for the whole
    /// masternode sync.
    hung: AtomicBool,
}

impl FakeSpv {
    fn set(&self, q: Quorum, a: SpvAnswer) {
        self.answers.lock().unwrap().insert(q, a);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl SpvQuorums for FakeSpv {
    fn lookup(&self, t: u32, h: [u8; 32], _height: u32) -> BoxFuture<SpvAnswer> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let a = self
            .answers
            .lock()
            .unwrap()
            .get(&(t, h))
            .copied()
            .unwrap_or(SpvAnswer::Absent);
        if self.hung.load(Ordering::SeqCst) {
            return Box::pin(std::future::pending());
        }
        Box::pin(async move { a })
    }
}

#[derive(Default)]
struct FakeTrusted {
    keys: Mutex<HashMap<Quorum, [u8; 48]>>,
    calls: AtomicUsize,
}

impl FakeTrusted {
    fn set(&self, q: Quorum, k: [u8; 48]) {
        self.keys.lock().unwrap().insert(q, k);
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TrustedQuorums for FakeTrusted {
    fn quorum_key(&self, t: u32, h: [u8; 32], _: u32) -> Result<[u8; 48], ContextProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.keys
            .lock()
            .unwrap()
            .get(&(t, h))
            .copied()
            .ok_or_else(|| ContextProviderError::Generic("trusted service unreachable".into()))
    }
}

#[derive(Default)]
struct Notices(Mutex<Vec<NoticeCode>>);

impl EventSink for Notices {
    fn emit(&self, event: EngineEvent) {
        if let EngineEvent::Notice { code, .. } = event {
            self.0.lock().unwrap().push(code);
        }
    }
}

impl Notices {
    fn codes(&self) -> Vec<NoticeCode> {
        self.0.lock().unwrap().clone()
    }
}

struct Rig {
    trust: Arc<TrustProvider>,
    spv: Arc<FakeSpv>,
    trusted: Arc<FakeTrusted>,
    notices: Arc<Notices>,
    provenance: Arc<Provenance>,
    path: std::path::PathBuf,
    _dir: tempfile::TempDir,
}

fn rig(mode: TrustMode) -> Rig {
    rig_with(mode, |t| t)
}

fn rig_with(mode: TrustMode, f: impl FnOnce(TrustProvider) -> TrustProvider) -> Rig {
    let dir = dw_testutil::private_tempdir();
    let path = dir.path().join(dw_appdb::APP_DB_FILE);
    let provenance = Arc::new(Provenance::open(Arc::new(AppDb::open(&path).unwrap())));
    let (spv, trusted, notices) = (
        Arc::new(FakeSpv::default()),
        Arc::new(FakeTrusted::default()),
        Arc::new(Notices::default()),
    );
    let trust = f(TrustProvider::new(
        mode,
        Arc::clone(&trusted) as _,
        Arc::clone(&notices) as _,
        DashNetwork::Regtest,
    ));
    trust.attach(Arc::clone(&spv) as _, &provenance);
    Rig {
        trust: Arc::new(trust),
        spv,
        trusted,
        notices,
        provenance,
        path,
        _dir: dir,
    }
}

impl Rig {
    fn lookup(&self, q: Quorum) -> Result<[u8; 48], ContextProviderError> {
        self.trust.quorum_public_key(q.0, q.1, 1000)
    }

    /// The fallback latch is set (data counts as unverified from now on).
    fn latched(&self) -> bool {
        self.provenance.is_unverified(&probe())
    }

    fn latched_on_reopen(&self) -> bool {
        Provenance::open(Arc::new(AppDb::open(&self.path).unwrap())).is_unverified(&probe())
    }
}

fn probe() -> Entity {
    Entity {
        kind: kind::IDENTITY,
        key: "probe".into(),
        digest: Some("v1:00".into()),
    }
}

fn mismatch(r: Result<[u8; 48], ContextProviderError>) -> bool {
    matches!(r, Err(e) if is_trust_mismatch(&e))
}

/// Rule 1: a `Verified` SPV key is served, and cached.
#[test]
fn a_verified_spv_key_is_served_and_cached() {
    let r = rig(TrustMode::Enforcing);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    assert_eq!(r.lookup(A).unwrap(), REAL);
    assert_eq!(r.lookup(A).unwrap(), REAL);
    assert_eq!(r.spv.calls(), 1, "the second lookup is a cache hit");
    assert_eq!(r.trusted.calls(), 0, "rule 2: no fallback needed");
    assert!(!r.latched());
}

/// Rule 2: before masternode sync the trusted service is the fallback, and
/// only behind the durable latch.
#[test]
fn before_masternode_sync_the_trusted_key_is_the_fallback() {
    let r = rig(TrustMode::Enforcing);
    r.trusted.set(A, REAL);
    assert_eq!(r.trust.source(), QuorumSource::TrustedFallback);
    assert!(r.trust.writes_wait());
    assert_eq!(r.lookup(A).unwrap(), REAL);
    assert!(r.latched(), "the fallback set the latch");
    assert!(r.latched_on_reopen(), "durably");
}

/// Rules 2 and 3: once masternode sync is done the trusted service is never
/// asked. An unknown quorum waits, fails closed, and fails at once while the
/// window lasts.
#[test]
fn after_masternode_sync_an_unknown_quorum_fails_closed() {
    let r = rig_with(TrustMode::Enforcing, |t| {
        t.with_waits(Duration::from_millis(300), Duration::from_millis(50))
    });
    r.trusted.set(A, REAL);
    r.spv.set(A, SpvAnswer::Held(REAL));
    r.trust.note_masternodes_synced();
    assert_eq!(r.trust.source(), QuorumSource::Spv);
    assert!(!r.trust.writes_wait());

    let started = Instant::now();
    assert!(r.lookup(A).is_err());
    assert!(started.elapsed() >= Duration::from_millis(250), "it waited");
    let polls = r.spv.calls();
    assert!(polls >= 3, "SPV was polled during the wait: {polls}");

    let started = Instant::now();
    assert!(r.lookup(A).is_err());
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "failed closed at once"
    );
    assert_eq!(r.spv.calls(), polls + 1);
    assert_eq!(r.trusted.calls(), 0, "never the fallback after sync");
    assert!(!r.latched());
}

/// Rule 3: a quorum SPV verifies during the wait is served.
#[test]
fn after_masternode_sync_a_quorum_verified_during_the_wait_is_served() {
    let r = rig_with(TrustMode::Enforcing, |t| {
        t.with_waits(Duration::from_secs(5), Duration::from_millis(20))
    });
    r.trust.note_masternodes_synced();
    let spv = Arc::clone(&r.spv);
    let later = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        spv.set(A, SpvAnswer::Verified(REAL));
    });
    assert_eq!(r.lookup(A).unwrap(), REAL);
    later.join().unwrap();
    assert_eq!(r.trusted.calls(), 0);
}

/// Rule 4 in both modes, against a verified or a held SPV key: a refusal
/// that maps to `platform.trust_mismatch`, and one notice per quorum.
#[test]
fn a_mismatch_refuses_and_notifies_once_per_quorum() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.spv.set(B, SpvAnswer::Held(REAL));
    r.trusted.set(A, FORGED);
    r.trusted.set(B, FORGED);
    assert!(mismatch(r.lookup(A)));
    assert!(mismatch(r.lookup(A)));
    assert!(mismatch(r.lookup(B)));
    assert_eq!(
        r.notices.codes(),
        vec![NoticeCode::PlatformTrustMismatch; 2],
        "one per quorum"
    );
    assert!(!r.latched(), "nothing was served");

    let in_proof = dash_sdk::Error::Proof(dash_sdk::ProofVerifierError::ContextProviderError(
        r.lookup(A).unwrap_err(),
    ));
    assert_eq!(PlatformError::from(in_proof), PlatformError::TrustMismatch);
    assert_eq!(
        PlatformError::from(dash_sdk::Error::ContextProviderError(
            r.lookup(A).unwrap_err()
        )),
        PlatformError::TrustMismatch
    );
    let other = ContextProviderError::InvalidQuorum("not found".into());
    assert_eq!(
        PlatformError::from(dash_sdk::Error::Proof(
            dash_sdk::ProofVerifierError::ContextProviderError(other)
        )),
        PlatformError::ContextUnavailable,
        "an unjudged proof is not an invalid one"
    );

    let r = rig(TrustMode::Enforcing);
    r.spv.set(A, SpvAnswer::Held(REAL));
    r.trusted.set(A, FORGED);
    assert!(mismatch(r.lookup(A)), "before sync, against a held key");
}

/// The acceptance case: a forged trusted key, with SPV holding the real
/// ones, refuses every lookup, so every proof and every DashPay read fails
/// with `platform.trust_mismatch`. Nothing is served or latched.
#[test]
fn a_forged_quorum_key_refuses_every_lookup() {
    let r = rig(TrustMode::Degraded);
    for q in [A, B, C] {
        r.spv.set(q, SpvAnswer::Verified(REAL));
        r.trusted.set(q, FORGED);
    }
    for q in [A, B, C, A, B, C] {
        let e = r.lookup(q).unwrap_err();
        assert_eq!(
            PlatformError::from(dash_sdk::Error::Proof(
                dash_sdk::ProofVerifierError::ContextProviderError(e)
            )),
            PlatformError::TrustMismatch
        );
    }
    assert!(!r.latched());
}

/// DEC-135/139: the fallback is refused when the latch cannot be written (a
/// writer holds app.sqlite) or made durable (a reader blocks the checkpoint).
#[test]
fn a_latch_that_cannot_be_made_durable_refuses_the_fallback() {
    let r = rig(TrustMode::Degraded);
    r.trusted.set(A, REAL);

    let holder = rusqlite::Connection::open(&r.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let e = r.lookup(A).unwrap_err();
    assert!(!is_trust_mismatch(&e));
    holder.execute_batch("ROLLBACK").unwrap();

    let reader = rusqlite::Connection::open(&r.path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT * FROM settings_kv;")
        .unwrap();
    assert!(r.lookup(A).is_err(), "checkpoint refused: no key served");
    reader.execute_batch("ROLLBACK").unwrap();

    assert_eq!(r.lookup(A).unwrap(), REAL);
    assert!(r.latched_on_reopen());
}

/// The status mapping (E0-10c §5, §5.1): only `Verified` verifies; `Unknown`
/// and every `Skipped` reason are held keys; `Invalid` refuses.
#[test]
fn only_verified_counts_and_invalid_refuses() {
    use LLMQEntryVerificationSkipStatus as Skip;
    use LLMQEntryVerificationStatus as S;
    let held = [
        S::Unknown,
        S::Skipped(Skip::MissedList(5)),
        S::Skipped(Skip::NotMarkedForVerification),
    ];
    for status in &held {
        assert_eq!(
            SpvAnswer::of(REAL, status),
            SpvAnswer::Held(REAL),
            "{status:?}"
        );
    }
    assert_eq!(SpvAnswer::of(REAL, &S::Verified), SpvAnswer::Verified(REAL));
    let invalid = S::Invalid(QuorumValidationError::RequiredMasternodeListNotPresent(5));
    assert_eq!(SpvAnswer::of(REAL, &invalid), SpvAnswer::Invalid);

    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Invalid);
    r.trusted.set(A, REAL);
    assert!(r.lookup(A).is_err());
    assert!(!r.latched());

    // A held key (a rotated-out quorum below the start height, §5.1) after
    // sync in Enforcing: refused, never verified.
    let r = rig_with(TrustMode::Enforcing, |t| {
        t.with_waits(Duration::from_millis(60), Duration::from_millis(20))
    });
    r.spv.set(A, SpvAnswer::Held(REAL));
    r.trust.note_masternodes_synced();
    assert!(r.lookup(A).is_err());
}

/// The degraded mode (DEC-168): the trusted key behind the latch when SPV
/// does not verify the quorum; SPV's verified key when the service agrees or
/// is unreachable.
#[test]
fn the_degraded_mode_serves_trusted_keys_and_prefers_verified_spv_ones() {
    let r = rig(TrustMode::Degraded);
    assert_eq!(r.trust.source(), QuorumSource::Trusted);
    assert!(!r.trust.writes_wait(), "rule 5 is suspended");

    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(A, REAL);
    assert_eq!(r.lookup(A).unwrap(), REAL);
    assert!(!r.latched(), "SPV verified it");

    r.spv.set(B, SpvAnswer::Verified(REAL));
    assert_eq!(r.lookup(B).unwrap(), REAL, "trusted service unreachable");
    assert!(r.lookup(C).is_err(), "neither source has it");
    assert!(!r.latched());

    r.trusted.set(C, REAL);
    assert_eq!(r.lookup(C).unwrap(), REAL);
    assert!(r.latched(), "an unverified key set the latch");
}

/// BL-162: masternode sync changes nothing in the degraded mode. A quorum SPV
/// does not hold (testnet's Platform quorum on a fresh data root) is served
/// from the trusted service at once, unverified, with no wait for SPV.
#[test]
fn after_masternode_sync_the_degraded_mode_falls_back_at_once() {
    let r = rig(TrustMode::Degraded);
    r.trusted.set(A, REAL);
    r.trust.note_masternodes_synced();
    assert_eq!(r.trust.source(), QuorumSource::Trusted);
    assert!(!r.trust.writes_wait());

    let started = Instant::now();
    for _ in 0..3 {
        assert_eq!(r.lookup(A).unwrap(), REAL);
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "no wait for SPV: {:?}",
        started.elapsed()
    );
    assert_eq!(r.spv.calls(), 3, "SPV asked once per lookup, never polled");
    assert!(r.latched(), "served unverified, behind the latch");
}

/// BL-162: a lookup SPV never answers counts as `Absent` once its bound
/// passes, in both modes before masternode sync: the trusted key is served
/// and no proof waits longer than the bound.
#[test]
fn a_hung_spv_lookup_is_bounded_and_falls_back() {
    for mode in [TrustMode::Degraded, TrustMode::Enforcing] {
        let r = rig_with(mode, |t| t.with_lookup_timeout(Duration::from_millis(200)));
        r.trusted.set(A, REAL);
        r.spv.hung.store(true, Ordering::SeqCst);
        let started = Instant::now();
        assert_eq!(r.lookup(A).unwrap(), REAL, "{mode:?}");
        let took = started.elapsed();
        assert!(
            took >= Duration::from_millis(200) && took < Duration::from_secs(3),
            "{mode:?}: bounded by the lookup timeout: {took:?}"
        );
        assert!(r.latched(), "{mode:?}: served unverified");
    }
}

/// The developer toggle: the trusted service only, SPV never consulted.
#[test]
fn the_toggle_forces_trusted_only() {
    for mode in [TrustMode::Degraded, TrustMode::Enforcing] {
        let r = rig(mode);
        r.trust.set_trusted_only(true);
        r.trust.note_masternodes_synced();
        assert_eq!(r.trust.source(), QuorumSource::Trusted);
        r.spv.set(A, SpvAnswer::Verified(REAL));
        r.trusted.set(A, FORGED);
        assert_eq!(r.lookup(A).unwrap(), FORGED, "{mode:?}");
        assert_eq!(r.spv.calls(), 0);
        assert!(r.latched());

        r.trust.set_trusted_only(false);
        match mode {
            TrustMode::Degraded => assert!(mismatch(r.lookup(A)), "off again: compared"),
            // Rule 2: after sync SPV's verified key, the service not asked.
            TrustMode::Enforcing => assert_eq!(r.lookup(A).unwrap(), REAL),
        }
    }
}

/// Before `attach` (the SDK exists before the wallet manager) nothing is
/// served: the latch cannot be set.
#[test]
fn nothing_is_served_before_attach() {
    let trusted = Arc::new(FakeTrusted::default());
    trusted.set(A, REAL);
    let trust = TrustProvider::new(
        TrustMode::Degraded,
        trusted as _,
        Arc::new(Notices::default()) as _,
        DashNetwork::Regtest,
    );
    assert!(trust.quorum_public_key(A.0, A.1, 1).is_err());
}

/// Design §4 on the engine's multi-thread runtime: a scope is verified only
/// when it made lookups and every one was SPV-verified.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_scope_counts_only_fully_verified_fetches() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(A, REAL);
    r.trusted.set(B, REAL);
    let trust = Arc::clone(&r.trust);
    tokio::spawn(async move {
        let (_, t) = scoped(async { trust.quorum_public_key(A.0, A.1, 1) }).await;
        assert!(t.verified());
        let (_, t) = scoped(async {
            trust.quorum_public_key(A.0, A.1, 1).unwrap();
            trust.quorum_public_key(B.0, B.1, 1).unwrap();
        })
        .await;
        assert!(!t.verified(), "one key came from the fallback");
        let (_, t) = scoped(async {}).await;
        assert!(!t.verified(), "no lookup verifies nothing");
        let (_, t) = scoped(async { trust.quorum_public_key(C.0, C.1, 1) }).await;
        assert!(!t.verified(), "a failed lookup");
    })
    .await
    .unwrap();
}

/// Platform's answers for the re-fetch, each proof using `quorum`.
struct FakeRefetch {
    trust: Arc<TrustProvider>,
    keys: BTreeMap<KeyID, IdentityPublicKey>,
    profile: ShownProfile,
    /// The DPNS domain: its spelling and its `records.identity`.
    domain: (String, Identifier),
    requests: Vec<ContactRequest>,
    /// The quorum each kind's proof cites.
    quorum: HashMap<&'static str, Quorum>,
}

impl FakeRefetch {
    fn prove(&self, k: &'static str) -> Result<(), PlatformError> {
        let q = self.quorum[k];
        self.trust
            .quorum_public_key(q.0, q.1, 1)
            .map(|_| ())
            .map_err(|_| PlatformError::ContextUnavailable)
    }
}

impl Refetcher for FakeRefetch {
    fn identity_keys(
        &self,
        _: Identifier,
    ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>> {
        let r = self.prove(kind::IDENTITY).map(|()| Some(self.keys.clone()));
        Box::pin(async move { r })
    }
    fn profile(&self, _: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>> {
        let r = self
            .prove(kind::PROFILE)
            .map(|()| Some(self.profile.clone()));
        Box::pin(async move { r })
    }
    fn dpns_domain(
        &self,
        _: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        let r = (self.prove(kind::DPNS_LABEL)).map(|()| Some(self.domain.clone()));
        Box::pin(async move { r })
    }
    fn contact_requests(
        &self,
        sender: Identifier,
        recipient: Identifier,
        created_at: u64,
    ) -> BoxFuture<Result<Vec<ContactRequest>, PlatformError>> {
        let found = (self.requests.iter())
            .filter(|r| {
                (r.sender_id, r.recipient_id, r.created_at) == (sender, recipient, created_at)
            })
            .cloned()
            .collect();
        let r = self.prove(kind::CONTACT_REQUEST).map(|()| found);
        Box::pin(async move { r })
    }
}

fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        Identifier::new([sender; 32]),
        Identifier::new([recipient; 32]),
        1,
        2,
        3,
        vec![4; 96],
        900,
        created_at,
    )
}

/// Design §5: after sync, a re-fetch records what verified and matches, and
/// nothing that differs or came through the fallback.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_post_sync_refetch_records_verified_matches_only() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(A, REAL);
    r.trusted.set(B, REAL); // SPV does not hold B: the fallback.
    let alice = Identifier::new([7; 32]);
    let shown = ShownProfile {
        display_name: Some("Alice".into()),
        ..Default::default()
    };
    let fetch = FakeRefetch {
        trust: Arc::clone(&r.trust),
        keys: BTreeMap::new(),
        profile: ShownProfile {
            display_name: Some("Mallory".into()),
            ..Default::default()
        },
        domain: ("Alice".into(), alice),
        // Two requests of one pair in one block: either verifies.
        requests: vec![
            ContactRequest {
                account_reference: 4,
                ..request(7, 8, 1_700_000_000_123)
            },
            request(7, 8, 1_700_000_000_123),
        ],
        quorum: HashMap::from([
            (kind::IDENTITY, A),
            (kind::PROFILE, A),
            (kind::DPNS_LABEL, B),
            (kind::CONTACT_REQUEST, A),
        ]),
    };
    let held = vec![
        Held::Identity {
            id: alice,
            keys: BTreeMap::new(),
        },
        Held::Profile { id: alice, shown },
        Held::label("Alice", alice),
        Held::ContactRequest(request(7, 8, 1_700_000_000_123)),
        Held::ContactRequest(ContactRequest {
            encrypted_public_key: vec![5; 96],
            ..request(7, 8, 1_700_000_000_123)
        }),
        Held::ContactRequest(request(7, 8, 1_700_000_000_999)),
    ];
    let verified = tokio::spawn(async move {
        let mut verified = Vec::new();
        for h in &held {
            verified.push(reverify(h, &fetch).await);
        }
        verified
    })
    .await
    .unwrap();
    assert_eq!(
        verified,
        [true, false, false, true, false, false],
        "the profile differs, the label came through the fallback, the second \
         request's key differs and the third is not on Platform"
    );
}

/// DP2-01's rows look up contact requests by m4 §5's key:
/// `sender:recipient:$createdAt`, Base58 ids and milliseconds. The record a
/// verified request writes is exactly that key.
#[test]
fn a_verified_contact_request_records_the_contacts_model_key() {
    let held = Held::ContactRequest(request(7, 8, 1_700_000_000_123));
    let b58 = |b: u8| {
        Identifier::new([b; 32]).to_string(dpp::platform_value::string_encoding::Encoding::Base58)
    };
    let key = format!("{}:{}:1700000000123", b58(7), b58(8));
    assert_eq!(
        key,
        contact_request_key(
            &Identifier::new([7; 32]),
            &Identifier::new([8; 32]),
            1_700_000_000_123
        )
    );
    let records = confirmed(&[&held], std::slice::from_ref(&held));
    assert_eq!(records, vec![held.entity()]);
    assert_eq!(
        (records[0].kind, records[0].key.as_str()),
        (kind::CONTACT_REQUEST, key.as_str())
    );

    let r = rig(TrustMode::Degraded);
    r.trusted.set(A, REAL); // the fallback sets the latch
    r.lookup(A).unwrap();
    assert!(r.provenance.is_unverified(&held.entity()));
    r.provenance.record_verified(&records[0]).unwrap();
    assert!(!r.provenance.is_unverified(&held.entity()));
    let changed = Held::ContactRequest(ContactRequest {
        account_reference: 4,
        ..request(7, 8, 1_700_000_000_123)
    });
    assert_eq!(changed.entity().key, key, "the same record key");
    assert!(
        r.provenance.is_unverified(&changed.entity()),
        "other content"
    );
}

/// Design §5: a verified fetch is recorded only if the library still holds
/// the entity unchanged (and unverified) when the record is written.
#[test]
fn a_record_needs_the_entity_unchanged_at_write_time() {
    let alice = Identifier::new([7; 32]);
    let profile = |name: &str| Held::Profile {
        id: alice,
        shown: ShownProfile {
            display_name: Some(name.into()),
            ..Default::default()
        },
    };
    let identity = Held::Identity {
        id: alice,
        keys: BTreeMap::new(),
    };
    let fetched = [profile("Alice"), identity.clone()];
    let verified: Vec<&Held> = fetched.iter().collect();
    let alice58 = alice.to_string(dpp::platform_value::string_encoding::Encoding::Base58);
    let records = confirmed(&verified, &[profile("Mallory"), identity]);
    assert_eq!(
        records
            .iter()
            .map(|e| (e.kind, e.key.as_str()))
            .collect::<Vec<_>>(),
        vec![(kind::IDENTITY, alice58.as_str())],
        "the profile changed during the fetch"
    );
    assert!(
        confirmed(&verified, &[]).is_empty(),
        "recorded meanwhile, or gone"
    );
}

/// A nested scope's lookups count in the enclosing scope too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_nested_scope_counts_in_the_outer_one() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(B, REAL);
    let trust = Arc::clone(&r.trust);
    tokio::spawn(async move {
        let (inner, outer) = scoped(async {
            trust.quorum_public_key(A.0, A.1, 1).unwrap();
            scoped(async { trust.quorum_public_key(B.0, B.1, 1).unwrap() })
                .await
                .1
        })
        .await;
        assert!(!inner.verified(), "the inner lookup used the fallback");
        assert!(!outer.verified(), "and so does the outer scope");
    })
    .await
    .unwrap();
}

/// After the session closed (its provenance dropped) no unverified key is
/// served: the latch cannot be set.
#[test]
fn nothing_unverified_is_served_once_the_session_closed() {
    let Rig {
        trust,
        spv,
        trusted,
        provenance,
        _dir,
        ..
    } = rig(TrustMode::Degraded);
    spv.set(A, SpvAnswer::Verified(REAL));
    trusted.set(A, REAL);
    trusted.set(B, REAL);
    drop(provenance);
    assert_eq!(
        trust.quorum_public_key(A.0, A.1, 1).unwrap(),
        REAL,
        "a verified key needs no latch"
    );
    assert!(trust.quorum_public_key(B.0, B.1, 1).is_err());
}

/// Rule 3 from a multi-thread runtime worker, as the SDK calls it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_3_holds_on_a_runtime_worker() {
    let r = rig_with(TrustMode::Enforcing, |t| {
        t.with_waits(Duration::from_millis(200), Duration::from_millis(20))
    });
    r.trusted.set(A, REAL);
    r.trust.note_masternodes_synced();
    let trust = Arc::clone(&r.trust);
    let spv = Arc::clone(&r.spv);
    tokio::spawn(async move {
        assert!(
            trust.quorum_public_key(A.0, A.1, 1).is_err(),
            "failed closed"
        );
        spv.set(B, SpvAnswer::Verified(REAL));
        assert_eq!(trust.quorum_public_key(B.0, B.1, 1).unwrap(), REAL);
    })
    .await
    .unwrap();
    assert_eq!(r.trusted.calls(), 0);
    assert!(!r.latched());
}

/// The re-fetch task starts on the masternode-sync transition, which is
/// sticky.
#[tokio::test]
async fn masternode_sync_wakes_the_refetch_once_and_stays() {
    let r = rig(TrustMode::Enforcing);
    let mut synced = r.trust.subscribe_synced();
    assert!(!*synced.borrow_and_update());
    r.trust.note_masternodes_synced();
    r.trust.note_masternodes_synced();
    assert!(synced.has_changed().unwrap());
    synced.wait_for(|s| *s).await.unwrap();
    assert!(
        !synced.has_changed().unwrap(),
        "a second note changes nothing"
    );
    assert!(r.trust.masternodes_synced());
}

/// Opus-high N1: SPV progress reaches the provider through the session hub.
/// The masternode phase alone is not enough (headers can be stale); once
/// both are done, the hub notes it and the re-fetch task wakes.
#[tokio::test]
async fn spv_progress_marks_masternode_sync_through_the_hub() {
    use dash_spv::EventHandler;
    use dash_spv::sync::{BlockHeadersProgress, MasternodesProgress, SyncProgress, SyncState};

    let r = rig(TrustMode::Enforcing);
    let hub = crate::events::SessionHub::new(DashNetwork::Regtest, Arc::new(Notices::default()));
    assert!(hub.trust.set(Arc::clone(&r.trust)).is_ok());
    let mut synced = r.trust.subscribe_synced();
    let mut p = SyncProgress::default();
    let mut headers = BlockHeadersProgress::default();
    headers.update_target_height(1000);
    headers.update_tip_height(500);
    headers.set_state(SyncState::Syncing);
    p.update_headers(headers.clone());
    let mut masternodes = MasternodesProgress::default();
    masternodes.update_target_height(1000);
    masternodes.update_current_height(1000);
    masternodes.set_state(SyncState::WaitForEvents);
    p.update_masternodes(masternodes);
    hub.on_progress(&p);
    assert!(!r.trust.masternodes_synced(), "headers are still syncing");
    assert!(!synced.has_changed().unwrap());
    headers.update_tip_height(1000);
    headers.set_state(SyncState::Synced);
    p.update_headers(headers);
    hub.on_progress(&p);
    assert!(r.trust.masternodes_synced());
    tokio::time::timeout(Duration::from_secs(5), synced.wait_for(|s| *s))
        .await
        .expect("the re-fetch task is woken")
        .unwrap();
}

/// One re-fetch of `h` through `fetch`, recording it if it verified (the
/// task's steps, design §5).
async fn refetch(r: &Rig, h: Held, fetch: FakeRefetch) -> bool {
    let verified = tokio::spawn(async move { reverify(&h, &fetch).await.then_some(h) })
        .await
        .unwrap();
    let Some(h) = verified else { return false };
    for e in confirmed(&[&h], std::slice::from_ref(&h)) {
        r.provenance.record_verified(&e).unwrap();
    }
    true
}

/// DEC-183: a record verifies the content it was written for. An entity the
/// library replaced afterwards reads as unverified; a replacement Platform
/// only served through the fallback never reads as verified; the re-fetch
/// verifies the new content once SPV-held keys back it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replaced_entity_reads_unverified_until_its_new_content_verifies() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(A, REAL);
    r.trusted.set(B, REAL); // SPV does not hold B: the fallback
    r.lookup(B).unwrap();
    assert!(r.latched());
    let alice = Identifier::new([7; 32]);
    let shown = |name: &str| ShownProfile {
        display_name: Some(name.into()),
        ..Default::default()
    };
    let held = |name: &str| Held::Profile {
        id: alice,
        shown: shown(name),
    };
    let platform = |name: &str, quorum: Quorum| FakeRefetch {
        trust: Arc::clone(&r.trust),
        keys: BTreeMap::new(),
        profile: shown(name),
        domain: ("Alice".into(), alice),
        requests: Vec::new(),
        quorum: HashMap::from([(kind::PROFILE, quorum)]),
    };
    let unverified = |name: &str| r.provenance.is_unverified(&held(name).entity());

    assert!(refetch(&r, held("Alice"), platform("Alice", A)).await);
    assert!(!unverified("Alice"));

    // The library's sync replaced the profile with what the trusted
    // service's keys proved: it reads as unverified, and stays so.
    assert!(unverified("Mallory"), "replaced after verification");
    assert!(!refetch(&r, held("Mallory"), platform("Mallory", B)).await);
    assert!(unverified("Mallory"), "a fallback-sourced replacement");
    assert!(
        Provenance::open(Arc::new(AppDb::open(&r.path).unwrap()))
            .is_unverified(&held("Mallory").entity()),
        "after a reopen too"
    );

    // Platform serves it under SPV-held keys: the re-fetch verifies it.
    assert!(refetch(&r, held("Mallory"), platform("Mallory", A)).await);
    assert!(!unverified("Mallory"), "re-verified");
    assert!(unverified("Alice"), "the record moved to the new content");
}

/// DEC-183: every field a reader shows is in the digest, and the encoding
/// names its version.
#[test]
fn the_digest_covers_the_content_and_names_its_version() {
    let alice = Identifier::new([7; 32]);
    let profile = |p: ShownProfile| {
        Held::Profile {
            id: alice,
            shown: p,
        }
        .entity()
        .digest
        .unwrap()
    };
    let base = ShownProfile {
        display_name: Some("Alice".into()),
        ..Default::default()
    };
    let d = profile(base.clone());
    assert!(d.starts_with("v2:") && d.len() == 3 + 64, "{d}");
    assert_eq!(d, profile(base.clone()), "deterministic");
    for changed in [
        ShownProfile {
            display_name: Some("Alicf".into()),
            ..base.clone()
        },
        ShownProfile {
            public_message: Some(String::new()),
            ..base.clone()
        },
        ShownProfile {
            avatar_url: Some("https://a.example/x".into()),
            ..base.clone()
        },
        ShownProfile {
            avatar_hash: Some([0; 32]),
            ..base.clone()
        },
        ShownProfile {
            avatar_fingerprint: Some([0; 8]),
            ..base.clone()
        },
    ] {
        assert_ne!(profile(changed.clone()), d, "{changed:?}");
    }
    let r = request(7, 8, 1);
    let req = |r: ContactRequest| Held::ContactRequest(r).entity().digest.unwrap();
    let d = req(r.clone());
    for changed in [
        ContactRequest {
            sender_key_index: 9,
            ..r.clone()
        },
        ContactRequest {
            recipient_key_index: 9,
            ..r.clone()
        },
        ContactRequest {
            account_reference: 9,
            ..r.clone()
        },
        ContactRequest {
            encrypted_account_label: Some(vec![]),
            ..r.clone()
        },
        ContactRequest {
            encrypted_public_key: vec![4; 95],
            ..r.clone()
        },
        ContactRequest {
            auto_accept_proof: Some(vec![1]),
            ..r.clone()
        },
        ContactRequest {
            core_height_created_at: 901,
            ..r.clone()
        },
    ] {
        assert_ne!(req(changed.clone()), d, "{changed:?}");
    }
    let label = |l: &str, o: u8| Held::label(l, Identifier::new([o; 32])).entity();
    assert_ne!(
        label("Alice", 7).digest,
        label("Alice", 8).digest,
        "the owner"
    );
    assert_eq!(
        label("Alice", 7).key,
        label("Al1ce", 7).key,
        "one record key"
    );
    assert_ne!(
        label("Alice", 7).digest,
        label("Al1ce", 7).digest,
        "the spelling (Sol r1 R1)"
    );
    let identity = |keys: &[IdentityPublicKey]| {
        let keys = keys.iter().map(|k| (k.id(), k.clone())).collect();
        Held::Identity { id: alice, keys }.entity().digest.unwrap()
    };
    let v0 = |id: KeyID| IdentityPublicKeyV0 {
        id,
        purpose: Purpose::AUTHENTICATION,
        security_level: SecurityLevel::MASTER,
        contract_bounds: None,
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: vec![2; 33].into(),
        disabled_at: None,
    };
    let key = |id: KeyID| IdentityPublicKey::V0(v0(id));
    let edit = |f: fn(&mut IdentityPublicKeyV0)| {
        let mut k = v0(0);
        f(&mut k);
        IdentityPublicKey::V0(k)
    };
    let d = identity(&[key(0)]);
    assert_eq!(d, identity(&[key(0)]));
    assert_ne!(d, identity(&[]), "a key removed");
    assert_ne!(d, identity(&[key(0), key(1)]), "a key added");
    for changed in [
        edit(|k| k.disabled_at = Some(1)),
        edit(|k| k.data = vec![3; 33].into()),
        edit(|k| k.purpose = Purpose::TRANSFER),
        edit(|k| k.security_level = SecurityLevel::HIGH),
        edit(|k| k.read_only = true),
    ] {
        assert_ne!(identity(std::slice::from_ref(&changed)), d, "{changed:?}");
    }
    let refiled = Held::Identity {
        id: alice,
        keys: BTreeMap::from([(5, key(0))]),
    };
    assert_ne!(
        refiled.entity().digest.unwrap(),
        d,
        "filed under another id"
    );
}

/// Sol r1 R1: a DPNS label record says "this spelling is this identity's
/// name". A spelling Platform does not hold fails the first re-fetch; a
/// spelling the library put in place after verification, or during the
/// network wait, reads as unverified under the same record key; another
/// owner reads as unverified, and a re-fetch naming it records nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_label_record_binds_its_spelling_and_its_owner() {
    let r = rig(TrustMode::Degraded);
    r.spv.set(A, SpvAnswer::Verified(REAL));
    r.trusted.set(A, REAL);
    r.trusted.set(B, REAL); // SPV does not hold B: the fallback
    r.lookup(B).unwrap();
    assert!(r.latched());
    let (alice, bob) = (Identifier::new([7; 32]), Identifier::new([8; 32]));
    let platform = || FakeRefetch {
        trust: Arc::clone(&r.trust),
        keys: BTreeMap::new(),
        profile: ShownProfile::default(),
        domain: ("Alice".into(), alice),
        requests: Vec::new(),
        quorum: HashMap::from([(kind::DPNS_LABEL, A)]),
    };
    let unverified = |h: Held| r.provenance.is_unverified(&h.entity());

    // Held as `Al1ce`; Platform's domain is spelled `Alice`.
    assert!(!refetch(&r, Held::label("Al1ce", alice), platform()).await);
    assert!(unverified(Held::label("Al1ce", alice)), "initial mismatch");

    assert!(refetch(&r, Held::label("Alice", alice), platform()).await);
    assert!(!unverified(Held::label("Alice", alice)));
    assert!(
        unverified(Held::label("Al1ce", alice)),
        "replaced after verification"
    );

    // Fetched as `Alice`, replaced by `Al1ce` before the record step.
    let fetched = Held::label("Alice", alice);
    assert!(
        confirmed(&[&fetched], &[Held::label("Al1ce", alice)]).is_empty(),
        "replaced during the network wait"
    );

    assert!(unverified(Held::label("Alice", bob)), "another owner");
    assert!(!refetch(&r, Held::label("Alice", bob), platform()).await);
    assert!(unverified(Held::label("Alice", bob)));
    assert!(
        !unverified(Held::label("Alice", alice)),
        "alice's record stands"
    );
}

/// Sol r1: the digest keeps field boundaries and option tags (a value
/// holding a tag byte cannot shift into the next field) and does not
/// depend on key insertion order.
#[test]
fn the_digest_separates_fields_and_options_and_ignores_key_order() {
    let alice = Identifier::new([7; 32]);
    let profile = |a: Option<&str>, b: Option<&str>| {
        let shown = ShownProfile {
            display_name: a.map(str::to_owned),
            public_message: b.map(str::to_owned),
            ..Default::default()
        };
        Held::Profile { id: alice, shown }.entity().digest.unwrap()
    };
    assert_ne!(
        profile(Some("ab"), Some("c")),
        profile(Some("a"), Some("bc"))
    );
    assert_ne!(
        profile(Some("a\u{1}b"), Some("c")),
        profile(Some("a"), Some("b\u{1}c"))
    );
    assert_ne!(profile(Some(""), None), profile(None, Some("")));
    assert_ne!(profile(Some(""), None), profile(None, None));
    let key = |id| {
        IdentityPublicKey::V0(IdentityPublicKeyV0 {
            id,
            purpose: Purpose::AUTHENTICATION,
            security_level: SecurityLevel::MASTER,
            contract_bounds: None,
            key_type: KeyType::ECDSA_SECP256K1,
            read_only: false,
            data: vec![2; 33].into(),
            disabled_at: None,
        })
    };
    let identity = |keys| Held::Identity { id: alice, keys }.entity().digest.unwrap();
    assert_eq!(
        identity(BTreeMap::from([(1, key(1)), (0, key(0))])),
        identity(BTreeMap::from([(0, key(0)), (1, key(1))]))
    );
    let base = request(7, 8, 123);
    let req = |r| Held::ContactRequest(r).entity().digest.unwrap();
    for changed in [
        ContactRequest {
            sender_id: Identifier::new([9; 32]),
            ..base.clone()
        },
        ContactRequest {
            recipient_id: Identifier::new([9; 32]),
            ..base.clone()
        },
        ContactRequest {
            created_at: 124,
            ..base.clone()
        },
    ] {
        assert_ne!(req(base.clone()), req(changed));
    }
}

/// Sol r1 R2: the record step counts only the writes that reached
/// app.sqlite. With another writer holding it, nothing is written, the
/// count is 0 and the entity stays unverified; once it is free, 1.
#[test]
fn a_failed_record_write_is_not_counted() {
    let r = rig(TrustMode::Degraded);
    r.provenance.fallback_in_use().unwrap();
    let entity = Held::label("Alice", Identifier::new([7; 32])).entity();
    let records = [entity.clone()];
    let holder = rusqlite::Connection::open(&r.path).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    assert_eq!(record_all(&r.provenance, &records), 0, "app.sqlite locked");
    assert!(r.provenance.is_unverified(&entity));
    holder.execute_batch("ROLLBACK").unwrap();
    assert_eq!(record_all(&r.provenance, &records), 1);
    assert!(!r.provenance.is_unverified(&entity));
}
