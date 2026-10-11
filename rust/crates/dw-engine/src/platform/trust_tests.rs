//! E0-10b's layered quorum provider over fake SPV and trusted sources and a
//! real app.sqlite (design §8): one test per rule, the latch refusal, the
//! status mapping, the scope tally and the post-sync re-fetch.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dash_context_provider::ContextProviderError;
use dashcore::sml::llmq_entry_verification::{
    LLMQEntryVerificationSkipStatus, LLMQEntryVerificationStatus,
};
use dashcore::sml::quorum_validation_error::QuorumValidationError;
use dpp::identity::{IdentityPublicKey, KeyID};
use dpp::prelude::Identifier;
use dw_appdb::AppDb;

use super::errors::PlatformError;
use super::provenance::{Provenance, kind};
use super::reverify::{Held, Refetcher, ShownProfile, confirmed, reverify};
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
        self.provenance.is_unverified(kind::IDENTITY, "probe")
    }

    fn latched_on_reopen(&self) -> bool {
        Provenance::open(Arc::new(AppDb::open(&self.path).unwrap()))
            .is_unverified(kind::IDENTITY, "probe")
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
    owner: Identifier,
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
    fn dpns_owner(&self, _: String) -> BoxFuture<Result<Option<Identifier>, PlatformError>> {
        let r = self.prove(kind::DPNS_LABEL).map(|()| Some(self.owner));
        Box::pin(async move { r })
    }
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
        owner: alice,
        quorum: HashMap::from([
            (kind::IDENTITY, A),
            (kind::PROFILE, A),
            (kind::DPNS_LABEL, B),
        ]),
    };
    let held = vec![
        Held::Identity {
            id: alice,
            keys: BTreeMap::new(),
        },
        Held::Profile { id: alice, shown },
        Held::Label {
            label: "a11ce".into(),
            owner: alice,
        },
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
        [true, false, false],
        "the profile differs and the label came through the fallback"
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
    assert_eq!(
        confirmed(&verified, &[profile("Mallory"), identity]),
        vec![(kind::IDENTITY, alice58)],
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
