//! DP3-04's money-move gate: every row of the design's decision table
//! against a scripted library and Platform over a real (in-memory)
//! provenance, in paused time; and the fake Platform and the switches the
//! session-level tests (`send/contact_tests.rs`, `topup_tests.rs`) use.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dpp::identity::{IdentityPublicKey, KeyID};
use dpp::prelude::Identifier;
use dw_appdb::AppDb;
use dw_vault::LockState;
use platform_wallet::ContactRequest;
use tokio::sync::watch;
use tokio::time::Instant;

use super::errors::PlatformError;
use super::money_gate::{GateEnv, Refusal, Signals, Waits, run};
use super::provenance::Provenance;
use super::reverify::{self, Held, Refetcher, ShownProfile};
use super::runtime::{LockView, guard};
use super::trust::{self, BoxFuture, Rule5, TrustMode};
use crate::NetworkSession;

/// What the fake Platform's proofs are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Proof {
    /// Every quorum key SPV-verified.
    Verified,
    /// A key served through the fallback (SPV does not verify it yet).
    Unverified,
    /// SPV and the trusted service disagree.
    Mismatch,
}

#[derive(Default)]
struct Answers {
    keys: BTreeMap<Identifier, BTreeMap<KeyID, IdentityPublicKey>>,
    profiles: BTreeMap<Identifier, ShownProfile>,
    domains: BTreeMap<String, (String, Identifier)>,
    requests: Vec<ContactRequest>,
}

/// Platform as these tests script it: what each fetch returns, and what
/// its proof's quorum lookups were.
pub(crate) struct FakePlatform {
    proof: Mutex<Proof>,
    answers: Mutex<Answers>,
    fetches: AtomicU32,
}

impl FakePlatform {
    pub(crate) fn new(proof: Proof) -> Arc<Self> {
        Arc::new(Self {
            proof: Mutex::new(proof),
            answers: Mutex::default(),
            fetches: AtomicU32::new(0),
        })
    }

    pub(crate) fn set_proof(&self, proof: Proof) {
        *guard(&self.proof) = proof;
    }

    /// Platform holds these contact requests.
    pub(crate) fn answer_requests(&self, requests: Vec<ContactRequest>) {
        guard(&self.answers).requests = requests;
    }

    /// Platform holds the identity with these keys.
    pub(crate) fn answer_identity(&self, id: Identifier, keys: BTreeMap<KeyID, IdentityPublicKey>) {
        guard(&self.answers).keys.insert(id, keys);
    }

    /// Platform holds exactly what `held` says.
    fn answer(&self, held: &[Held]) {
        let mut a = guard(&self.answers);
        for h in held {
            match h {
                Held::Identity { id, keys } => {
                    a.keys.insert(*id, keys.clone());
                }
                Held::Profile { id, shown } => {
                    a.profiles.insert(*id, shown.clone());
                }
                Held::Label {
                    label,
                    spelling,
                    owner,
                } => {
                    a.domains.insert(label.clone(), (spelling.clone(), *owner));
                }
                Held::ContactRequest(r) => a.requests.push(r.clone()),
            }
        }
    }

    pub(crate) fn fetches(&self) -> u32 {
        self.fetches.load(Ordering::SeqCst)
    }

    /// One fetch: its proof's lookup, then `answer`.
    fn fetch<T: Send + 'static>(&self, answer: T) -> BoxFuture<Result<T, PlatformError>> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let r = match *guard(&self.proof) {
            Proof::Verified => {
                trust::note_lookup(true);
                Ok(answer)
            }
            Proof::Unverified => {
                trust::note_lookup(false);
                Ok(answer)
            }
            Proof::Mismatch => Err(PlatformError::TrustMismatch),
        };
        Box::pin(async move { r })
    }
}

impl Refetcher for FakePlatform {
    fn identity_keys(
        &self,
        id: Identifier,
    ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>> {
        let keys = guard(&self.answers).keys.get(&id).cloned();
        self.fetch(keys)
    }

    fn profile(&self, id: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>> {
        let profile = guard(&self.answers).profiles.get(&id).cloned();
        self.fetch(profile)
    }

    fn dpns_domain(
        &self,
        label: String,
    ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
        let domain = guard(&self.answers).domains.get(&label).cloned();
        self.fetch(domain)
    }

    fn contact_requests(
        &self,
        sender: Identifier,
        recipient: Identifier,
        created_at: u64,
    ) -> BoxFuture<Result<Vec<ContactRequest>, PlatformError>> {
        let found = (guard(&self.answers).requests.iter())
            .filter(|r| {
                (r.sender_id, r.recipient_id, r.created_at) == (sender, recipient, created_at)
            })
            .cloned()
            .collect();
        self.fetch(found)
    }
}

/// Shows `contact`'s DPNS name as `name`, as a landed fetch does (the
/// session tests run without Platform, so no fetch runs).
pub(crate) fn show_username(session: &NetworkSession, contact: Identifier, name: &str) {
    session.platform.contacts.names.show(contact, name);
    let provenance = session.live().unwrap().provenance;
    provenance.show_label(contact, Some(name));
}

impl FakePlatform {
    /// Platform's DPNS domain spelled `spelling`, its `records.identity`
    /// `owner`.
    pub(crate) fn answer_domain(&self, spelling: &str, owner: Identifier) {
        self.answer(&[Held::label(spelling, owner)]);
    }
}

/// The session tests' switches: the fallback latch set, the provider in
/// `mode`, short waits, and `platform` answering the gate's re-fetches.
pub(crate) fn gate_session(session: &NetworkSession, mode: TrustMode, platform: Arc<FakePlatform>) {
    session
        .live()
        .unwrap()
        .provenance
        .fallback_in_use()
        .unwrap();
    session.trust.set_mode(mode);
    (session.platform.money_gate).set_waits(Waits {
        wait: Duration::from_secs(4),
        retry: Duration::from_millis(50),
        probe: Duration::from_secs(2),
    });
    session.platform.money_gate.set_refetcher(platform);
}

/// The library's copy of the gate's subject, and the session around it.
struct World {
    provenance: Arc<Provenance>,
    held: Mutex<Option<Vec<Held>>>,
    open: AtomicBool,
}

impl World {
    fn new(held: Vec<Held>, latched: bool) -> Arc<Self> {
        let provenance = Provenance::open(Arc::new(AppDb::open_in_memory().unwrap()));
        if latched {
            provenance.fallback_in_use().unwrap();
        }
        Arc::new(Self {
            provenance: Arc::new(provenance),
            held: Mutex::new(Some(held)),
            open: AtomicBool::new(true),
        })
    }

    fn unverified(&self) -> Option<Vec<Held>> {
        let held = guard(&self.held).clone()?;
        Some(
            (held.into_iter())
                .filter(|h| self.provenance.is_unverified(&h.entity()))
                .collect(),
        )
    }

    fn verified(&self) -> bool {
        self.unverified().is_some_and(|u| u.is_empty())
    }
}

impl GateEnv for Arc<World> {
    async fn pending(&self) -> Result<Option<Vec<Held>>, Refusal> {
        Ok(self.unverified())
    }

    async fn record(&self, verified: Vec<Held>) {
        let verified: Vec<&Held> = verified.iter().collect();
        let records = reverify::confirmed(&verified, &self.unverified().unwrap_or_default());
        reverify::record_all(&self.provenance, &records);
    }

    fn open(&self) -> Result<(), Refusal> {
        match self.open.load(Ordering::SeqCst) {
            true => Ok(()),
            false => Err(Refusal::Failed(PlatformError::NetworkNotOpen)),
        }
    }
}

const WAIT: Duration = Duration::from_secs(60);

fn waits() -> Waits {
    Waits {
        wait: WAIT,
        retry: Duration::from_secs(3),
        probe: Duration::from_secs(10),
    }
}

fn request(sender: u8, recipient: u8, created_at: u64) -> ContactRequest {
    ContactRequest::new(
        Identifier::from([sender; 32]),
        Identifier::from([recipient; 32]),
        0,
        0,
        0,
        vec![1; 96],
        1,
        created_at,
    )
}

/// A contact payment's subject: the pair's two requests and the shown name.
fn payee() -> Vec<Held> {
    vec![
        Held::ContactRequest(request(1, 2, 1_000)),
        Held::ContactRequest(request(2, 1, 2_000)),
        Held::label("Bob", Identifier::from([2; 32])),
    ]
}

/// The vault's lock reports, as `PlatformRuntime::lock` sends them.
fn lock_reports() -> watch::Sender<LockView> {
    watch::Sender::new(LockView {
        state: LockState::Unlocked,
        locks: 0,
    })
}

/// Runs the gate on `world` with `platform` and the lock reports `lock`.
async fn gate(
    world: &Arc<World>,
    platform: &Arc<FakePlatform>,
    rule: Rule5,
    lock: &watch::Sender<LockView>,
) -> Result<(), Refusal> {
    let fetch = Arc::clone(platform) as Arc<dyn Refetcher>;
    let signals = Signals {
        rule,
        lock: lock.subscribe(),
    };
    run(world, move || Ok(fetch), signals, waits()).await
}

fn platform_holding(held: &[Held], proof: Proof) -> Arc<FakePlatform> {
    let platform = FakePlatform::new(proof);
    platform.answer(held);
    platform
}

/// Before the fallback was ever used nothing is unverified: every rule
/// lets the money move, and nothing is fetched.
#[tokio::test(start_paused = true)]
async fn without_the_latch_everything_passes() {
    let world = World::new(payee(), false);
    let platform = platform_holding(&payee(), Proof::Unverified);
    for rule in [Rule5::Enforce, Rule5::Probe, Rule5::Off] {
        assert_eq!(gate(&world, &platform, rule, &lock_reports()).await, Ok(()));
    }
    assert_eq!(platform.fetches(), 0);
}

/// A payee whose content is on record passes at once.
#[tokio::test(start_paused = true)]
async fn a_verified_payee_passes_without_a_fetch() {
    let world = World::new(payee(), true);
    for h in payee() {
        world.provenance.record_verified(&h.entity()).unwrap();
    }
    let platform = platform_holding(&payee(), Proof::Unverified);
    let started = Instant::now();
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
    assert_eq!((platform.fetches(), started.elapsed()), (0, Duration::ZERO));
}

/// Enforcing, unverified: the gate re-fetches each entity, records the
/// verified matches and lets the money move; a later gate fetches nothing.
#[tokio::test(start_paused = true)]
async fn enforcing_refetches_records_and_passes() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Verified);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
    assert!(world.verified(), "recorded");
    assert_eq!(platform.fetches(), 3);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
    assert_eq!(platform.fetches(), 3, "nothing fetched again");
}

/// Enforcing: while SPV does not verify the quorum (before masternode
/// sync, or a quorum it has not got yet), the gate keeps re-fetching and
/// lets the money move once a fetch verifies.
#[tokio::test(start_paused = true)]
async fn enforcing_waits_until_spv_verifies() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    let later = Arc::clone(&platform);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        later.set_proof(Proof::Verified);
    });
    let started = Instant::now();
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(20) && waited < WAIT,
        "{waited:?}"
    );
    assert!(world.verified());
}

/// Enforcing: nothing verifies within the wait, so the money does not
/// move (`platform.context_unavailable`) and nothing is recorded.
#[tokio::test(start_paused = true)]
async fn enforcing_refuses_when_nothing_verifies_in_time() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    let started = Instant::now();
    let refused = gate(&world, &platform, Rule5::Enforce, &lock_reports()).await;
    assert_eq!(refused, Err(Refusal::Unverified));
    assert_eq!(
        refused.unwrap_err().platform().code(),
        "platform.context_unavailable"
    );
    let waited = started.elapsed();
    assert!(
        waited > WAIT - Duration::from_secs(4) && waited <= WAIT,
        "{waited:?}"
    );
    assert_eq!(world.unverified().unwrap().len(), 3, "nothing recorded");
}

/// A re-fetch SPV and the trusted service disagree on refuses at once, in
/// the enforcing and the degraded mode (rule 4 is never suspended).
#[tokio::test(start_paused = true)]
async fn a_mismatch_refuses_in_every_mode() {
    for rule in [Rule5::Enforce, Rule5::Probe] {
        let world = World::new(payee(), true);
        let platform = platform_holding(&payee(), Proof::Mismatch);
        let started = Instant::now();
        let refused = gate(&world, &platform, rule, &lock_reports()).await;
        assert_eq!(refused, Err(Refusal::Mismatch), "{rule:?}");
        assert_eq!(
            refused.unwrap_err().platform().code(),
            "platform.trust_mismatch"
        );
        assert_eq!(started.elapsed(), Duration::ZERO, "at once");
        assert_eq!(platform.fetches(), 1);
    }
}

/// Degraded: rule 5 is suspended. One round of re-fetches looks for a
/// mismatch; data that does not verify still moves, and what verifies is
/// recorded.
#[tokio::test(start_paused = true)]
async fn degraded_probes_once_and_proceeds() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    assert_eq!(
        gate(&world, &platform, Rule5::Probe, &lock_reports()).await,
        Ok(())
    );
    assert_eq!(platform.fetches(), 3, "one round");
    assert_eq!(world.unverified().unwrap().len(), 3);

    platform.set_proof(Proof::Verified);
    assert_eq!(
        gate(&world, &platform, Rule5::Probe, &lock_reports()).await,
        Ok(())
    );
    assert!(world.verified(), "the probe records");
}

/// A Platform that never answers holds a degraded payment for the probe's
/// time only.
#[tokio::test(start_paused = true)]
async fn the_degraded_probe_is_bounded() {
    struct Hung;
    impl Refetcher for Hung {
        fn identity_keys(
            &self,
            _: Identifier,
        ) -> BoxFuture<Result<Option<BTreeMap<KeyID, IdentityPublicKey>>, PlatformError>> {
            Box::pin(std::future::pending())
        }
        fn profile(&self, _: Identifier) -> BoxFuture<Result<Option<ShownProfile>, PlatformError>> {
            Box::pin(std::future::pending())
        }
        fn dpns_domain(
            &self,
            _: String,
        ) -> BoxFuture<Result<Option<(String, Identifier)>, PlatformError>> {
            Box::pin(std::future::pending())
        }
        fn contact_requests(
            &self,
            _: Identifier,
            _: Identifier,
            _: u64,
        ) -> BoxFuture<Result<Vec<ContactRequest>, PlatformError>> {
            Box::pin(std::future::pending())
        }
    }
    let world = World::new(payee(), true);
    let signals = |rule| Signals {
        rule,
        lock: lock_reports().subscribe(),
    };
    let hung = || Ok(Arc::new(Hung) as Arc<dyn Refetcher>);
    let started = Instant::now();
    let probed = run(&world, hung, signals(Rule5::Probe), waits()).await;
    assert_eq!(probed, Ok(()));
    assert_eq!(started.elapsed(), waits().probe);
    let started = Instant::now();
    let enforced = run(&world, hung, signals(Rule5::Enforce), waits()).await;
    assert_eq!(enforced, Err(Refusal::Unverified));
    assert_eq!(started.elapsed(), WAIT);
}

/// The trusted-only toggle: SPV is not consulted, so nothing is fetched
/// and the money moves.
#[tokio::test(start_paused = true)]
async fn trusted_only_proceeds_without_a_fetch() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Verified);
    assert_eq!(
        gate(&world, &platform, Rule5::Off, &lock_reports()).await,
        Ok(())
    );
    assert_eq!(platform.fetches(), 0);
}

/// A verified fetch of other content than the library holds (a replaced
/// request, another spelling) records nothing and does not let money move.
#[tokio::test(start_paused = true)]
async fn a_differing_refetch_records_nothing() {
    let world = World::new(payee(), true);
    let platform = FakePlatform::new(Proof::Verified);
    platform.answer(&[
        Held::ContactRequest(request(1, 2, 1_000)),
        Held::ContactRequest(request(2, 1, 2_001)),
        Held::label("B0b", Identifier::from([2; 32])),
    ]);
    let refused = gate(&world, &platform, Rule5::Enforce, &lock_reports()).await;
    assert_eq!(refused, Err(Refusal::Unverified));
    let left = world.unverified().unwrap();
    assert_eq!(left.len(), 2, "only the matching request recorded");
    assert!(
        left.iter()
            .all(|h| !matches!(h, Held::ContactRequest(r) if r.created_at == 1_000))
    );
}

/// Race: another task (the background re-fetch) records the payee while
/// the gate waits; the gate's next read sees it and lets the money move.
#[tokio::test(start_paused = true)]
async fn a_record_written_during_the_wait_releases_it() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    let other = Arc::clone(&world);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(7)).await;
        for h in payee() {
            other.provenance.record_verified(&h.entity()).unwrap();
        }
    });
    let started = Instant::now();
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_secs(7) && waited < Duration::from_secs(10),
        "{waited:?}"
    );
}

/// Race: the payee's content changes during the wait (a sync pass): the
/// gate verifies the new content, never the old one's record.
#[tokio::test(start_paused = true)]
async fn content_replaced_during_the_wait_is_verified_anew() {
    let world = World::new(payee(), true);
    for h in payee() {
        world.provenance.record_verified(&h.entity()).unwrap();
    }
    let replaced = vec![
        Held::ContactRequest(request(1, 2, 1_000)),
        Held::ContactRequest(request(2, 1, 9_000)),
        Held::label("Bob", Identifier::from([2; 32])),
    ];
    *guard(&world.held) = Some(replaced.clone());
    let platform = platform_holding(&payee(), Proof::Verified);
    let refused = gate(&world, &platform, Rule5::Enforce, &lock_reports()).await;
    assert_eq!(
        refused,
        Err(Refusal::Unverified),
        "the old content's answer"
    );
    platform.answer(&replaced);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
}

/// Race: the vault is locked during the wait. The wait ends at once and
/// the money does not move.
#[tokio::test(start_paused = true)]
async fn a_vault_lock_ends_the_wait() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    let lock = Arc::new(lock_reports());
    let locker = Arc::clone(&lock);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        locker.send_modify(|v| {
            v.state = LockState::Locked;
            v.locks += 1;
        });
    });
    let started = Instant::now();
    let refused = gate(&world, &platform, Rule5::Enforce, &lock).await;
    assert_eq!(refused, Err(Refusal::Locked));
    assert!(started.elapsed() < Duration::from_secs(6));
}

/// The session closing during the wait ends it.
#[tokio::test(start_paused = true)]
async fn a_closing_session_ends_the_wait() {
    let world = World::new(payee(), true);
    let platform = platform_holding(&payee(), Proof::Unverified);
    let closing = Arc::clone(&world);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        closing.open.store(false, Ordering::SeqCst);
    });
    let refused = gate(&world, &platform, Rule5::Enforce, &lock_reports()).await;
    assert_eq!(refused, Err(Refusal::Failed(PlatformError::NetworkNotOpen)));
}

/// A subject the library no longer holds: enforcing refuses; otherwise the
/// subject's own checks decide.
#[tokio::test(start_paused = true)]
async fn a_subject_gone_refuses_only_when_enforcing() {
    let world = World::new(payee(), true);
    *guard(&world.held) = None;
    let platform = platform_holding(&payee(), Proof::Verified);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Err(Refusal::Unverified)
    );
    for rule in [Rule5::Probe, Rule5::Off] {
        assert_eq!(gate(&world, &platform, rule, &lock_reports()).await, Ok(()));
    }
    assert_eq!(platform.fetches(), 0);
}

/// An identity subject (a top-up): its keys are its content.
#[tokio::test(start_paused = true)]
async fn an_identity_verifies_by_its_keys() {
    let id = Identifier::from([7; 32]);
    let held = vec![Held::Identity {
        id,
        keys: BTreeMap::new(),
    }];
    let world = World::new(held.clone(), true);
    let platform = FakePlatform::new(Proof::Verified);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Err(Refusal::Unverified),
        "Platform has no such identity"
    );
    platform.answer(&held);
    assert_eq!(
        gate(&world, &platform, Rule5::Enforce, &lock_reports()).await,
        Ok(())
    );
}
