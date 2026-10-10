//! DP1-02's state machine against a real session, vault and app.sqlite,
//! with the library scripted ([`Fake`]): the kill matrix, the funding gate
//! and cap, restore adoption, the funding failures (H12), discard and
//! "Finish transfers".

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use dw_appdb::{AppDb, NewRegistration, RegistrationChange, RegistrationRow};
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, LockState, MemoryOsStore, VaultConfig,
};
use zeroize::Zeroizing;

use super::errors::{NameError, PlatformError, RegistrationError};
use super::flows::FlowKind;
use super::lease::Lease;
use super::names::NameOutcome;
use super::registration::{
    self, InitialProfile, RegistrationFunding, RegistrationPhase, RegistrationRequest,
    RegistrationWait,
};
use super::registration_flow::{
    self as flow, Driver, FundError, LockInfo, LockStatus, RegistrationLib, Spendable,
};
use super::registration_rows::{Funding, Phase, ProfileRecord};
use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, WalletId};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASS: &[u8] = b"pass phrase";
const COIN: u64 = 100_000_000;
/// Not contested (it has a digit 2–9).
const PLAIN: &str = "alice2";
const CONTESTED: &str = "alice";
const NAME_CREDITS: u64 = 50_000_000;

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

struct Fixture {
    _dir: tempfile::TempDir,
    engine: Engine,
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

fn fixture(encrypted: bool) -> Fixture {
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
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
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    let pass: Option<&'static [u8]> = encrypted.then_some(PASS);
    engine
        .block_on(session.vault_op(move |v| v.create(pass)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                core_compat: false,
                ..Default::default()
            },
        ))
        .unwrap();
    Fixture {
        _dir: dir,
        engine,
        session,
        wallet,
    }
}

impl Fixture {
    fn appdb(&self) -> Arc<AppDb> {
        self.session.live().unwrap().appdb
    }

    fn grant(&self, max_duffs: u64, max_credits: u64, credential: Credential<'_>) -> String {
        self.session
            .vault()
            .authorize(
                GrantPurpose::PlatformOp {
                    max_duffs,
                    max_credits,
                },
                Some(&self.wallet.0),
                credential,
            )
            .unwrap()
            .id
    }

    fn lease(&self) -> Lease {
        let g = self.grant(COIN, 10 * COIN * 1000, Credential::None);
        self.engine
            .block_on(
                self.session
                    .begin_lease(self.wallet, FlowKind::Registration, &[g]),
            )
            .unwrap()
    }

    fn fake(&self) -> Arc<Fake> {
        Arc::new(Fake::new(self.appdb(), self.wallet))
    }

    fn insert(&self, row: NewRegistration) -> i64 {
        self.appdb()
            .insert_registration(&self.wallet.to_string(), &row, 1)
            .unwrap()
    }

    fn row(&self, draft: i64) -> RegistrationRow {
        self.appdb()
            .registration_row(&self.wallet.to_string(), draft)
            .unwrap()
            .unwrap()
    }

    fn rows(&self) -> Vec<RegistrationRow> {
        self.appdb()
            .registration_rows(&self.wallet.to_string())
            .unwrap()
    }

    /// One run inline, under its own claim; `waits`: a task's run (see
    /// `Driver::waits`).
    fn drive(&self, lib: &Fake, draft: i64, lease: Option<Lease>, waits: bool) {
        self.engine
            .block_on(async {
                let claim = self
                    .session
                    .platform
                    .registrations
                    .claim(&self.session, (self.wallet, draft))
                    .await?
                    .expect("the flow is free");
                Driver::new(&self.session, lib, &claim, lease, waits)
                    .run()
                    .await
            })
            .unwrap();
    }

    fn spawn(&self, lib: &Arc<Fake>, draft: i64, lease: Option<Lease>) {
        self.engine
            .block_on(flow::start_run(
                &self.session,
                self.wallet,
                Arc::clone(lib),
                draft,
                lease,
            ))
            .unwrap();
    }

    fn discard(&self, lib: &Arc<Fake>, draft: i64) -> Result<(), RegistrationError> {
        self.engine.block_on(registration::discard(
            &self.session,
            self.wallet,
            Arc::clone(lib),
            &draft.to_string(),
        ))
    }

    fn running(&self, draft: i64) -> bool {
        self.session
            .platform
            .registrations
            .get((self.wallet, draft))
            .running()
    }

    fn settle(&self, draft: i64) {
        eventually("the flow stops", || !self.running(draft));
    }

    /// A process restart as the rows see it: the flow's task and every
    /// in-memory state of it are gone; app.sqlite stays.
    fn kill(&self, draft: i64) {
        self.session
            .platform
            .registrations
            .forget((self.wallet, draft));
        eventually("the killed flow is gone", || !self.running(draft));
    }

    fn statuses(&self, lib: &Fake) -> Vec<registration::RegistrationStatus> {
        self.engine
            .block_on(registration::statuses(&self.session, self.wallet, lib))
            .unwrap()
    }

    fn status(&self, lib: &Fake, draft: i64) -> registration::RegistrationStatus {
        self.statuses(lib)
            .into_iter()
            .find(|s| s.draft == draft.to_string())
            .unwrap()
    }
}

fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn core_row(label: &str) -> NewRegistration {
    NewRegistration {
        identity_index: None,
        identity: None,
        label: label.into(),
        temp_label: None,
        funding: Funding::CoreBalance {
            lock_duffs: flow::LOCK_DUFFS,
        }
        .encode(),
        initial_profile: Some(
            ProfileRecord::without_avatar(&InitialProfile {
                display_name: Some("Alice".into()),
                public_message: None,
                avatar_candidate: None,
            })
            .encode(),
        ),
        phase: Phase::Draft.as_str().into(),
    }
}

fn request(label: &str) -> RegistrationRequest {
    RegistrationRequest {
        label: label.into(),
        temporary_label: None,
        funding: RegistrationFunding::CoreBalance,
        initial_profile: None,
    }
}

fn phase(row: &RegistrationRow) -> Phase {
    Phase::parse(&row.phase).unwrap()
}

/// A library call of the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    Fund,
    WaitProof,
    RegisterIdentity,
    RegisterName,
    CreateProfile,
}

/// Where a scripted call hangs, as a kill -9 would leave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// Before the library acted.
    Before,
    /// `Fund` only: the lock is tracked (the tap wrote the row), not yet
    /// broadcast.
    Tracked,
    /// The library acted; the row was not written.
    After,
}

/// How the scripted funding call ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FundScript {
    /// Locked by InstantSend in time.
    Ok,
    /// Broadcast; no InstantSend lock in the window.
    IsTimeout,
    /// Failed after tracking (the broadcaster's outcome unknown).
    FailTracked,
    /// Tracked, then definitively rejected and untracked.
    Rejected,
    /// Failed before anything was tracked (coin selection).
    FailUntracked,
    /// Tracked, then untracked as swept, with an unknown outcome.
    Swept,
    /// Failed with an unknown outcome and no tracked row (the wallet
    /// closing under the call).
    LostUntracked,
}

#[derive(Default)]
struct World {
    locks: Vec<LockInfo>,
    identities: Vec<(u32, String)>,
    names: Vec<(String, String)>,
    profiles: BTreeSet<String>,
    spendable: Spendable,
    funds: u32,
    identity_creates: u32,
    fund_script: Option<FundScript>,
    hang: Option<(Call, When)>,
    contested: BTreeSet<String>,
    /// The proof `wait_proof` delivers (`None`: it times out).
    proof_arrives: bool,
    /// Every `wait_proof` bound asked for.
    waits: Vec<Duration>,
}

/// platform-wallet as the flow sees it, scripted.
struct Fake {
    appdb: Arc<AppDb>,
    wallet: WalletId,
    ready: AtomicBool,
    hung: AtomicBool,
    w: Mutex<World>,
}

impl Fake {
    fn new(appdb: Arc<AppDb>, wallet: WalletId) -> Self {
        Self {
            appdb,
            wallet,
            ready: AtomicBool::new(true),
            hung: AtomicBool::new(false),
            w: Mutex::new(World {
                spendable: Spendable {
                    duffs: COIN,
                    inputs: 2,
                    protected: 0,
                },
                proof_arrives: true,
                contested: [CONTESTED.to_string()].into(),
                ..Default::default()
            }),
        }
    }

    fn w(&self) -> std::sync::MutexGuard<'_, World> {
        self.w.lock().unwrap()
    }

    /// Fundings and identity creations so far.
    fn counts(&self) -> (u32, u32) {
        let w = self.w();
        (w.funds, w.identity_creates)
    }

    fn hang_at(&self, call: Call, when: When) {
        self.w().hang = Some((call, when));
    }

    async fn maybe_hang(&self, call: Call, when: When) {
        if self.w().hang == Some((call, when)) {
            self.hung.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
    }

    fn lock_at(&self, index: u32, status: LockStatus) -> OutPoint {
        let mut w = self.w();
        let outpoint = OutPoint::new(Txid::from_byte_array([index as u8 + 1; 32]), 0);
        w.locks.push(LockInfo {
            outpoint,
            registration: true,
            identity_index: index,
            account: 0,
            amount: flow::LOCK_DUFFS,
            status,
        });
        outpoint
    }

    fn set_status(&self, outpoint: OutPoint, status: LockStatus) {
        let mut w = self.w();
        if let Some(l) = w.locks.iter_mut().find(|l| l.outpoint == outpoint) {
            l.status = status;
        }
    }
}

impl RegistrationLib for Fake {
    async fn tracked_locks(&self) -> Vec<LockInfo> {
        self.w().locks.clone()
    }

    async fn identities(&self) -> Vec<(u32, String)> {
        self.w().identities.clone()
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.w().spendable)
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        (!self.ready.load(Ordering::SeqCst)).then_some(RegistrationWait::Sync)
    }

    async fn fund(&self, _: u64, index: u32, lease: &Lease) -> Result<OutPoint, FundError> {
        lease
            .funding_signer()
            .map_err(|e| FundError::Refused(e.into()))?;
        self.maybe_hang(Call::Fund, When::Before).await;
        let script = self.w().fund_script.unwrap_or(FundScript::Ok);
        self.w().funds += 1;
        match script {
            FundScript::FailUntracked => {
                return Err(FundError::NotSent(PlatformError::Internal {
                    detail: "no coins".into(),
                }));
            }
            FundScript::LostUntracked => return Err(FundError::Failed(PlatformError::Unavailable)),
            _ => {}
        }
        // Tracked as Built, stored through the tap before the broadcast.
        let outpoint = self.lock_at(index, LockStatus::Built);
        flow::note_lock_tracked(
            &self.appdb,
            &self.wallet.to_string(),
            outpoint,
            Some(index),
            2,
        )
        .unwrap();
        self.maybe_hang(Call::Fund, When::Tracked).await;
        let result = match script {
            FundScript::Ok => {
                self.set_status(outpoint, LockStatus::InstantSendLocked);
                Ok(outpoint)
            }
            FundScript::IsTimeout => {
                self.set_status(outpoint, LockStatus::Broadcast);
                Err(FundError::IsTimeout)
            }
            FundScript::FailTracked => {
                self.set_status(outpoint, LockStatus::Broadcast);
                Err(FundError::Failed(PlatformError::Unavailable))
            }
            FundScript::Rejected => {
                self.w().locks.retain(|l| l.outpoint != outpoint);
                Err(FundError::NotSent(PlatformError::Internal {
                    detail: "rejected".into(),
                }))
            }
            FundScript::Swept => {
                self.w().locks.retain(|l| l.outpoint != outpoint);
                Err(FundError::Failed(PlatformError::Unavailable))
            }
            FundScript::FailUntracked | FundScript::LostUntracked => unreachable!(),
        };
        self.maybe_hang(Call::Fund, When::After).await;
        result
    }

    async fn wait_proof(&self, outpoint: OutPoint, timeout: Duration) -> Result<(), FundError> {
        self.w().waits.push(timeout);
        self.maybe_hang(Call::WaitProof, When::Before).await;
        if !self.w().proof_arrives {
            // The bound elapsing, compressed.
            tokio::time::sleep(Duration::from_millis(5)).await;
            return Err(FundError::IsTimeout);
        }
        self.set_status(outpoint, LockStatus::ChainLocked);
        self.maybe_hang(Call::WaitProof, When::After).await;
        Ok(())
    }

    async fn find_identity(&self, outpoint: OutPoint) -> Result<Option<String>, PlatformError> {
        let id = flow::b58(&flow::identity_for(outpoint));
        Ok(self
            .w()
            .identities
            .iter()
            .any(|(_, i)| *i == id)
            .then_some(id))
    }

    async fn register_identity(
        &self,
        outpoint: OutPoint,
        index: u32,
        lease: &Lease,
    ) -> Result<String, PlatformError> {
        // The proof is signed with the lock's one-time key: the funding
        // signer, as `PwLib` asks.
        lease.identity_signer(&[index])?;
        lease.funding_signer()?;
        self.maybe_hang(Call::RegisterIdentity, When::Before).await;
        let id = flow::b58(&flow::identity_for(outpoint));
        {
            let mut w = self.w();
            assert!(
                !w.identities.iter().any(|(_, i)| *i == id),
                "the lock is consumed twice"
            );
            w.identities.push((index, id.clone()));
            w.identity_creates += 1;
        }
        self.set_status(outpoint, LockStatus::Consumed);
        self.maybe_hang(Call::RegisterIdentity, When::After).await;
        Ok(id)
    }

    async fn register_name(
        &self,
        identity: &str,
        label: &str,
        lease: &Lease,
    ) -> Result<NameOutcome, NameError> {
        lease
            .available(super::flows::BudgetPurpose::Credits)
            .map_err(PlatformError::from)?;
        self.maybe_hang(Call::RegisterName, When::Before).await;
        let contested = self.w().contested.contains(label);
        {
            let mut w = self.w();
            let name = (identity.to_owned(), label.to_owned());
            if !w.names.contains(&name) {
                w.names.push(name);
            }
        }
        self.maybe_hang(Call::RegisterName, When::After).await;
        Ok(if contested {
            NameOutcome::ContestStarted { ends_at: None }
        } else {
            NameOutcome::Registered
        })
    }

    async fn create_profile(
        &self,
        identity: &str,
        _: u32,
        _: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        lease.available(super::flows::BudgetPurpose::Credits)?;
        self.maybe_hang(Call::CreateProfile, When::Before).await;
        self.w().profiles.insert(identity.to_owned());
        self.maybe_hang(Call::CreateProfile, When::After).await;
        Ok(())
    }

    async fn name_credits(&self, _: &str) -> Result<u64, NameError> {
        Ok(NAME_CREDITS)
    }

    async fn credit_balance(&self, _: &str) -> Result<Option<u64>, PlatformError> {
        Ok(Some(0))
    }
}

/// Every transition, killed before the library acted, mid-funding and
/// after it acted: the restarted flow ends registered, with one lock, one
/// identity, one name and one profile.
#[test]
fn a_kill_at_any_transition_resumes_to_one_registration() {
    let f = fixture(false);
    let points = [
        (Call::Fund, When::Before),
        (Call::Fund, When::Tracked),
        (Call::Fund, When::After),
        (Call::WaitProof, When::Before),
        (Call::WaitProof, When::After),
        (Call::RegisterIdentity, When::Before),
        (Call::RegisterIdentity, When::After),
        (Call::RegisterName, When::Before),
        (Call::RegisterName, When::After),
        (Call::CreateProfile, When::Before),
        (Call::CreateProfile, When::After),
    ];
    for (call, when) in points {
        let lib = f.fake();
        if call == Call::WaitProof {
            // The proof comes after the window.
            lib.w().fund_script = Some(FundScript::IsTimeout);
        }
        lib.hang_at(call, when);
        let draft = f.insert(core_row(PLAIN));
        f.spawn(&lib, draft, Some(f.lease()));
        eventually("the scripted call is reached", || {
            lib.hung.load(Ordering::SeqCst)
        });
        f.kill(draft);
        let killed = f.row(draft);
        if call == Call::Fund && when != When::Before {
            // Restore rule 3: the outpoint went to the row before the
            // broadcast.
            assert!(killed.asset_lock_outpoint.is_some(), "{call:?} {when:?}");
            assert_eq!(phase(&killed), Phase::FundingSent, "{call:?} {when:?}");
        }
        lib.w().hang = None;
        f.drive(&lib, draft, Some(f.lease()), true);
        let row = f.row(draft);
        assert_eq!(
            (phase(&row), row.error.as_deref()),
            (Phase::Done, None),
            "{call:?} {when:?}"
        );
        let w = lib.w();
        assert_eq!((w.funds, w.identity_creates), (1, 1), "{call:?} {when:?}");
        assert_eq!(w.names.len(), 1, "{call:?} {when:?}");
        assert_eq!(w.profiles.len(), 1, "{call:?} {when:?}");
        assert_eq!(row.identity.as_deref(), Some(w.identities[0].1.as_str()));
        assert_eq!(row.initial_profile, None, "consumed at ProfileCreated");
    }
}

#[test]
fn start_runs_to_done_and_reports_the_flow() {
    let f = fixture(false);
    let lib = f.fake();
    let grant = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    let draft = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(PLAIN),
            grant,
        ))
        .unwrap();
    let draft: i64 = draft.parse().unwrap();
    f.settle(draft);
    let status = f.status(&lib, draft);
    assert_eq!(status.phase, RegistrationPhase::Done);
    assert_eq!((status.waiting, status.failure), (None, None));
    assert!(status.funds_committed, "the lock was consumed");
    assert!(status.txid.is_some());
    assert!(
        f.session
            .platform
            .registrations
            .get((f.wallet, draft))
            .backed_up,
        "Restore rule 4"
    );
    // Done does not count as in progress.
    let lib2 = f.fake();
    let g = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    let second = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        lib2,
        request("bob2"),
        g,
    ));
    assert!(second.is_ok(), "{second:?}");
}

#[test]
fn a_second_registration_waits_for_the_first() {
    let f = fixture(false);
    let lib = f.fake();
    lib.ready.store(false, Ordering::SeqCst);
    let start = |label: &str| {
        let g = f.grant(COIN, 10 * COIN * 1000, Credential::None);
        f.engine.block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(label),
            g,
        ))
    };
    let draft: i64 = start(PLAIN).unwrap().parse().unwrap();
    eventually("the sync hold", || {
        f.status(&lib, draft).waiting == Some(RegistrationWait::Sync)
    });
    assert_eq!(start("bob2"), Err(RegistrationError::InProgress));
    // Parked before funding: the sync hold (Restore rule 1).
    let status = f.status(&lib, draft);
    assert_eq!(status.phase, RegistrationPhase::Draft);
    assert_eq!(status.waiting, Some(RegistrationWait::Sync));
    assert!(!status.funds_committed);
    assert_eq!(lib.w().funds, 0);
}

#[test]
fn the_funding_cap_and_balance_are_checked_before_a_row_exists() {
    let f = fixture(false);
    let lib = f.fake();
    let quote = f
        .engine
        .block_on(registration::quote(&*lib, &request(PLAIN)))
        .unwrap();
    assert!(!quote.contested);
    assert_eq!(quote.lock_duffs, flow::LOCK_DUFFS);
    assert_eq!(quote.fee_duffs, flow::fee_bound_worst(2));
    assert_eq!(quote.total_duffs, quote.lock_duffs + quote.fee_duffs);
    assert_eq!(quote.grant.max_duffs, quote.total_duffs);
    assert_eq!(
        quote.grant.max_credits,
        flow::IDENTITY_CREATE_CREDITS + NAME_CREDITS
    );
    let contested = f
        .engine
        .block_on(registration::quote(&*lib, &request(CONTESTED)))
        .unwrap();
    assert!(contested.contested);
    assert_eq!(contested.lock_duffs, flow::CONTESTED_LOCK_DUFFS);

    // One duff short of the cap.
    let g = f.grant(
        quote.total_duffs - 1,
        quote.grant.max_credits,
        Credential::None,
    );
    let err = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(PLAIN),
            g,
        ))
        .unwrap_err();
    assert_eq!(err.code(), "platform.grant_exceeded");
    lib.w().spendable.duffs = quote.total_duffs - 1;
    let g = f.grant(quote.total_duffs, quote.grant.max_credits, Credential::None);
    let err = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(PLAIN),
            g,
        ))
        .unwrap_err();
    assert_eq!(err.code(), "registration.funding_insufficient");
    assert!(f.rows().is_empty());
    // An avatar candidate waits for DP4-02.
    let mut req = request(PLAIN);
    req.initial_profile = Some(InitialProfile {
        display_name: None,
        public_message: None,
        avatar_candidate: Some("c1".into()),
    });
    let err = f
        .engine
        .block_on(registration::quote(&*lib, &req))
        .unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    // Invitation funding is DP5-02's; the faucet is off.
    let mut req = request(PLAIN);
    req.funding = RegistrationFunding::Invitation {
        link_id: "l1".into(),
    };
    let err = f
        .engine
        .block_on(registration::quote(&*lib, &req))
        .unwrap_err();
    assert_eq!(err.code(), "platform.not_implemented");
    req.funding = RegistrationFunding::FaucetAssetLock {
        key: "k".into(),
        proof: "00".into(),
    };
    let err = f
        .engine
        .block_on(registration::quote(&*lib, &req))
        .unwrap_err();
    assert_eq!(err.code(), "platform.feature_off");
}

#[test]
fn the_funding_step_refuses_a_lock_past_the_lease_cap() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(core_row(PLAIN));
    let g = f.grant(flow::LOCK_DUFFS, 10 * COIN * 1000, Credential::None);
    let lease = f
        .engine
        .block_on(
            f.session
                .begin_lease(f.wallet, FlowKind::Registration, &[g]),
        )
        .unwrap();
    f.drive(&lib, draft, Some(lease), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::KeysPrepared);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("platform.grant_exceeded"), true)
    );
    assert_eq!(lib.w().funds, 0);
    // A new grant resumes it.
    let g = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            Some(g),
        ))
        .unwrap();
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
}

/// Restore rule 2: a restored row behind reality adopts the lock the other
/// machine funded for its index, and the identity that lock made; an
/// identity at its index is the row's only by its lock (DEC-171 (2)).
#[test]
fn a_restored_row_adopts_what_the_seed_already_did() {
    let f = fixture(false);
    let lib = f.fake();
    // A lock the library tracked live for index 3 (its index is known).
    let mut row = core_row(PLAIN);
    row.identity_index = Some(3);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let outpoint = lib.lock_at(3, LockStatus::ChainLocked);
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(flow::outpoint_of(&row), Some(outpoint));
    assert_eq!(lib.counts(), (0, 1));

    // The identity the other machine made at the index, its lock rebuilt
    // from the chain (index unknown): adopted through the lock.
    let lib = f.fake();
    let mut row = core_row("carol2");
    row.identity_index = Some(7);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let lock = lib.lock_at(0, LockStatus::RecoveredFromChain);
    let id = flow::b58(&flow::identity_for(lock));
    lib.w().identities.push((7, id.clone()));
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(row.identity.as_deref(), Some(id.as_str()));
    assert_eq!(flow::outpoint_of(&row), Some(lock));
    assert_eq!(lib.counts(), (0, 0));

    // An identity at the index with no lock to prove it the row's: the row
    // waits, funding nothing and adopting nothing.
    let lib = f.fake();
    let mut row = core_row("dora2");
    row.identity_index = Some(9);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    lib.w().identities.push((9, "made-elsewhere".into()));
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.identity), (Phase::KeysPrepared, None));
    assert_eq!(lib.counts(), (0, 0));
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::ChainLock)
    );
}

#[test]
fn identity_indexes_skip_every_used_one() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().identities.push((2, "x".into()));
    lib.lock_at(4, LockStatus::Consumed);
    let mut other = core_row("dave2");
    other.identity_index = Some(3);
    f.insert(other);
    let draft = f.insert(core_row(PLAIN));
    lib.ready.store(true, Ordering::SeqCst);
    lib.hang_at(Call::Fund, When::Before);
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("funding starts", || lib.hung.load(Ordering::SeqCst));
    f.kill(draft);
    assert_eq!(f.row(draft).identity_index, Some(5));
}

/// The InstantSend window ends: an own key is dropped, the flow waits
/// keyless for the ChainLock, then asks for an authority once (H12 never
/// fires: the lock is tracked).
#[test]
fn an_islock_timeout_parks_keyless_until_the_chainlock() {
    let f = fixture(true);
    f.session.lock_vault_sync().unwrap();
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().proof_arrives = false;
    let draft = f.insert(core_row(PLAIN));
    let g = f.grant(COIN, 10 * COIN * 1000, Credential::Passphrase(PASS));
    let lease = f
        .engine
        .block_on(
            f.session
                .begin_lease(f.wallet, FlowKind::Registration, &[g]),
        )
        .unwrap();
    assert!(lease.view().unwrap().own_key);
    f.drive(&lib, draft, Some(lease.clone()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::ProofWaiting, None)
    );
    let view = lease.view().unwrap();
    assert!(
        !matches!(view.state, super::flows::LeaseStateView::Active),
        "the own key is dropped: {view:?}"
    );
    let status = f.status(&lib, draft);
    assert_eq!(status.waiting, Some(RegistrationWait::ChainLock));
    assert!(status.funds_committed && !status.holds_key);
    // Discard is refused: the funds are committed.
    let err = f.discard(&lib, draft).unwrap_err();
    assert_eq!(err.code(), "invalid_argument");
    drop(lease);

    // The ChainLock arrives; a keyless resume takes the flow to the
    // identity step, where it asks once.
    lib.w().proof_arrives = true;
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            None,
        ))
        .unwrap();
    f.settle(draft);
    let status = f.status(&lib, draft);
    assert_eq!(status.phase, RegistrationPhase::ProofWaiting);
    assert_eq!(status.waiting, Some(RegistrationWait::Unlock));
    let g = f.grant(COIN, 10 * COIN * 1000, Credential::Passphrase(PASS));
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            Some(g),
        ))
        .unwrap();
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(f.session.vault().lock_state(), LockState::Locked);
}

/// H12: a funding failure is retryable exactly when nothing can be on the
/// wire; a tracked lock is adopted, and a lock untracked with an unknown
/// outcome keeps its row funded.
#[test]
fn funding_failures_follow_the_tracked_row() {
    let f = fixture(false);
    for script in [
        FundScript::FailUntracked,
        FundScript::Rejected,
        FundScript::FailTracked,
        FundScript::Swept,
    ] {
        let lib = f.fake();
        lib.w().fund_script = Some(script);
        lib.w().proof_arrives = false;
        let draft = f.insert(core_row(PLAIN));
        f.drive(&lib, draft, Some(f.lease()), false);
        let row = f.row(draft);
        let status = f.status(&lib, draft);
        match script {
            FundScript::FailUntracked | FundScript::Rejected => {
                assert_eq!(phase(&row), Phase::KeysPrepared, "{script:?}");
                assert_eq!(row.asset_lock_outpoint, None, "{script:?}");
                assert!(row.retryable && row.error.is_some(), "{script:?}");
                assert!(!status.funds_committed, "{script:?}");
                // NotSent: discard is allowed.
                f.discard(&lib, draft).unwrap();
            }
            FundScript::FailTracked => {
                assert_eq!(
                    (phase(&row), row.error.as_deref()),
                    (Phase::ProofWaiting, None),
                    "{script:?}"
                );
                assert!(status.funds_committed);
                // Finish it so the next one can start.
                lib.w().proof_arrives = true;
                f.drive(&lib, draft, Some(f.lease()), true);
                assert_eq!(phase(&f.row(draft)), Phase::Done);
            }
            FundScript::Swept => {
                // The outpoint reached the row; no tracked row is evidence
                // of nothing: MaybeSent, waiting for the chain.
                assert_eq!(
                    (phase(&row), row.error.as_deref()),
                    (Phase::FundingSent, None),
                    "{script:?}"
                );
                assert!(row.asset_lock_outpoint.is_some());
                assert!(status.funds_committed);
                assert_eq!(status.waiting, Some(RegistrationWait::ChainLock));
                let discard = f.discard(&lib, draft);
                assert_eq!(discard.unwrap_err().code(), "invalid_argument");
            }
            _ => unreachable!(),
        }
    }
}

/// A funding that failed with an unknown outcome before any row was
/// tracked (the wallet closing under the call) is MaybeSent: the row fails
/// `broadcast_unknown`, and discard, a retry and a new registration wait
/// for evidence; the lock tracked later (a restore's reconstruction,
/// through the tap) resumes it.
#[test]
fn an_unbacked_funding_is_maybe_sent_until_its_lock_is_tracked() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::LostUntracked);
    // An earlier identity: the flow takes index 1, which a lock rebuilt
    // from the chain keeps (only index 0 is the library's stand-in).
    lib.w().identities.push((0, "earlier".into()));
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::KeysPrepared);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some(flow::broadcast_unknown()), false)
    );
    let index = row.identity_index.unwrap();
    assert_eq!(index, 1);
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert_eq!(status.phase, RegistrationPhase::Failed);
    let discard = f.discard(&lib, draft);
    assert_eq!(discard.unwrap_err().code(), "invalid_argument");
    let resume = f.engine.block_on(registration::resume(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        &draft.to_string(),
        Some(f.grant(COIN, COIN, Credential::None)),
    ));
    assert_eq!(resume.unwrap_err().code(), "invalid_argument");
    // A new registration waits for it too.
    let start = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("erin2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert_eq!(start, Err(RegistrationError::InProgress));

    // The lock shows up (the restore's reconstruction): positive evidence,
    // and the tap hands it to the row.
    let lock = lib.lock_at(index, LockStatus::ChainLocked);
    let moved =
        flow::note_lock_tracked(&f.appdb(), &f.wallet.to_string(), lock, Some(index), 3).unwrap();
    assert_eq!(moved, Some(draft));
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error), (Phase::FundingSent, None));
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            &draft.to_string(),
            Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
        ))
        .unwrap();
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
}

#[test]
fn rows_this_build_cannot_run_fail_and_free_the_slot() {
    let f = fixture(false);
    let lib = f.fake();
    let mut invitation = core_row(PLAIN);
    invitation.funding = Funding::Invitation {
        link_id: "l1".into(),
    }
    .encode();
    let invitation = f.insert(invitation);
    let mut newer = core_row("frank2");
    newer.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":1}"#.into();
    let newer = f.insert(newer);
    let mut odd_phase = core_row("gina2");
    odd_phase.phase = "verified_on_mars".into();
    let odd_phase = f.insert(odd_phase);
    // None of them blocks a new registration, before any status read
    // swept them.
    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("hank2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    f.settle(started.unwrap().parse().unwrap());
    let statuses = f.statuses(&lib);
    for (draft, code) in [
        (invitation, "invitation.invalid"),
        (newer, "registration.unsupported"),
        (odd_phase, "registration.unsupported"),
    ] {
        let s = statuses
            .iter()
            .find(|s| s.draft == draft.to_string())
            .unwrap();
        assert_eq!(s.phase, RegistrationPhase::Failed, "{code}");
        let failure = s.failure.as_ref().unwrap();
        assert_eq!((failure.code.as_str(), failure.retryable), (code, false));
        assert!(!s.funds_committed);
    }
    // DEC-171 (3): what this build cannot read stays as it is, never
    // advanced and never deleted; an invitation row it reads is deleted.
    for draft in [newer, odd_phase] {
        assert_eq!(
            f.discard(&lib, draft).unwrap_err().code(),
            "invalid_argument"
        );
        f.drive(&lib, draft, Some(f.lease()), true);
        assert_eq!(f.row(draft).error, None, "nothing written");
    }
    f.discard(&lib, invitation).unwrap();
    assert_eq!(lib.counts(), (1, 1), "only the new registration funded");
}

#[test]
fn a_contested_name_registers_the_temporary_one_too() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(CONTESTED);
    row.temp_label = Some("alice9".into());
    let draft = f.insert(row);
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Done);
    let names: Vec<String> = lib.w().names.iter().map(|(_, l)| l.clone()).collect();
    assert_eq!(names, [CONTESTED, "alice9"]);
}

#[test]
fn finish_adopts_a_stranded_lock_and_builds_none() {
    let f = fixture(false);
    let lib = f.fake();
    let outpoint = lib.lock_at(5, LockStatus::InstantSendLocked);
    // A consumed lock and another kind are left alone.
    lib.lock_at(6, LockStatus::Consumed);
    lib.w().locks.push(LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([0xee; 32]), 0),
        registration: false,
        identity_index: 0,
        account: 0,
        amount: 1,
        status: LockStatus::ChainLocked,
    });
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (1, 1, 0)
    );
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(flow::outpoint_of(&rows[0]), Some(outpoint));
    assert_eq!((phase(&rows[0]), rows[0].label.as_str()), (Phase::Done, ""));
    assert_eq!(lib.counts(), (0, 1));
    assert!(lib.w().names.is_empty(), "no name to register");
    // Run again: nothing to do.
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(0, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!((report.resumed, report.still_pending), (0, 0));
}

#[test]
fn discard_stops_an_unfunded_flow() {
    let f = fixture(false);
    let lib = f.fake();
    lib.ready.store(false, Ordering::SeqCst);
    let draft = f.insert(core_row(PLAIN));
    f.spawn(&lib, draft, Some(f.lease()));
    // The run waits for the sync, in its task.
    eventually("the sync hold", || {
        f.status(&lib, draft).waiting == Some(RegistrationWait::Sync)
    });
    f.discard(&lib, draft).unwrap();
    assert!(f.rows().is_empty());
    f.settle(draft);
    let err = f.discard(&lib, draft).unwrap_err();
    assert_eq!(err.code(), "invalid_argument", "unknown draft");
}

#[test]
fn an_existing_identity_registers_only_its_name() {
    let f = fixture(false);
    let lib = f.fake();
    let id = flow::b58(&dpp::prelude::Identifier::from([7u8; 32]));
    lib.w().identities.push((3, id.clone()));
    let mut req = request(PLAIN);
    req.funding = RegistrationFunding::ExistingIdentity {
        identity: id.clone(),
    };
    let quote = f.engine.block_on(registration::quote(&*lib, &req)).unwrap();
    assert_eq!(
        (quote.lock_duffs, quote.fee_duffs, quote.grant.max_duffs),
        (0, 0, 0)
    );
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            req,
            f.grant(0, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap()
        .parse()
        .unwrap();
    f.settle(draft);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.identity_index), (Phase::Done, Some(3)));
    assert_eq!(lib.w().funds, 0);
    assert_eq!(lib.w().names, [(id, PLAIN.to_string())]);
}

/// A `KeysPrepared` row at `index` whose lock the library tracks already:
/// the run adopts it and goes on to the proof.
fn adopted_lock_row(f: &Fixture, lib: &Fake, index: u32, status: LockStatus) -> i64 {
    lib.lock_at(index, status);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(index);
    row.phase = Phase::KeysPrepared.as_str().into();
    f.insert(row)
}

fn resume(f: &Fixture, lib: &Arc<Fake>, draft: i64, grant: Option<String>) {
    f.engine
        .block_on(registration::resume(
            &f.session,
            f.wallet,
            Arc::clone(lib),
            &draft.to_string(),
            grant,
        ))
        .unwrap();
}

/// The ChainLock wait has no limit, in bounded waits (the library's own
/// unbounded wait gives up after 180 s): a run keeps waiting, and a
/// refused discard leaves it running.
#[test]
fn the_chainlock_wait_is_bounded_and_survives_a_refused_discard() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().proof_arrives = false;
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Broadcast);
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("several bounded waits", || lib.w().waits.len() >= 3);
    assert!(lib.w().waits.iter().all(|w| *w == flow::CHAINLOCK_POLL));
    let status = f.status(&lib, draft);
    assert_eq!(status.waiting, Some(RegistrationWait::ChainLock));
    let discard = f.discard(&lib, draft);
    assert_eq!(discard.unwrap_err().code(), "invalid_argument");
    assert!(f.running(draft), "the refused discard stopped the flow");
    lib.w().proof_arrives = true;
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (0, 1));
}

/// A grant given to a flow that waits keyless reaches its run: it signs
/// when the proof arrives, without parking for a second prompt.
#[test]
fn a_grant_reaches_a_flow_that_waits_keyless() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().proof_arrives = false;
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Broadcast);
    f.spawn(&lib, draft, None);
    eventually("the proof wait", || !lib.w().waits.is_empty());
    resume(
        &f,
        &lib,
        draft,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    lib.w().proof_arrives = true;
    f.settle(draft);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error), (Phase::Done, None));
    assert_eq!(lib.counts(), (0, 1));
}

/// A run held for the sync keeps its lease and goes on when the sync
/// ends: one prompt (§2.6).
#[test]
fn a_run_waits_for_the_sync_with_its_lease() {
    let f = fixture(false);
    let lib = f.fake();
    lib.ready.store(false, Ordering::SeqCst);
    let draft: i64 = f
        .engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request(PLAIN),
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap()
        .parse()
        .unwrap();
    eventually("the sync hold", || {
        f.status(&lib, draft).waiting == Some(RegistrationWait::Sync)
    });
    assert!(f.running(draft));
    lib.ready.store(true, Ordering::SeqCst);
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
}

/// A failed row behind reality whose index another flow funded (Finish
/// transfers adopted the lock) takes a new index; it neither adopts the
/// other flow's lock nor sticks.
#[test]
fn a_lock_another_flow_holds_moves_this_one_to_a_new_index() {
    let f = fixture(false);
    let lib = f.fake();
    let lock = lib.lock_at(0, LockStatus::ChainLocked);
    let w = f.wallet.to_string();
    let mut held = core_row("");
    held.identity_index = Some(0);
    held.initial_profile = None;
    held.phase = Phase::ProofWaiting.as_str().into();
    let (holder, _) = f
        .appdb()
        .adopt_registration_lock(&w, &flow::canonical(lock), &held, 1)
        .unwrap();
    let mut behind = core_row(PLAIN);
    behind.identity_index = Some(0);
    behind.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(behind);
    f.appdb()
        .update_registration(
            &w,
            draft,
            None,
            Phase::KeysPrepared.as_str(),
            false,
            &RegistrationChange {
                failure: Some(Some(("registration.funding_insufficient".into(), true))),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert!(!f.status(&lib, draft).funds_committed);
    resume(
        &f,
        &lib,
        draft,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    f.settle(draft);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error.as_deref()), (Phase::Done, None));
    assert_eq!(row.identity_index, Some(1));
    assert_ne!(flow::outpoint_of(&row), Some(lock));
    let holder = f.row(holder);
    assert_eq!(flow::outpoint_of(&holder), Some(lock));
    assert_eq!(phase(&holder), Phase::ProofWaiting);
    assert_eq!(lib.counts(), (1, 1));
}

/// Two flows that choose their index at once never take one index.
#[test]
fn two_flows_never_take_one_identity_index() {
    let f = fixture(false);
    let lib = f.fake();
    let drafts = [f.insert(core_row(PLAIN)), f.insert(core_row("bob2"))];
    for draft in drafts {
        f.spawn(&lib, draft, Some(f.lease()));
    }
    for draft in drafts {
        f.settle(draft);
    }
    let indexes: BTreeSet<_> = drafts
        .iter()
        .map(|d| f.row(*d).identity_index.unwrap())
        .collect();
    assert_eq!(indexes.len(), 2, "{indexes:?}");
}

/// "Finish transfers" runs every row under one lease: a row without its
/// proof parks itself, not that lease, so an own-key grant still signs
/// the rows that have theirs.
#[test]
fn finish_keeps_an_own_key_past_a_proofless_row() {
    let f = fixture(true);
    f.session.lock_vault_sync().unwrap();
    let lib = f.fake();
    lib.w().proof_arrives = false;
    lib.lock_at(0, LockStatus::Broadcast);
    lib.lock_at(1, LockStatus::ChainLocked);
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::Passphrase(PASS)),
        ))
        .unwrap();
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (2, 1, 1)
    );
    assert_eq!(lib.counts(), (0, 1));
    let waiting: Vec<_> = f.statuses(&lib).into_iter().map(|s| s.waiting).collect();
    assert!(
        waiting.contains(&Some(RegistrationWait::ChainLock)),
        "{waiting:?}"
    );
}

/// A lease is offered only to a running driver; one offered too late
/// comes back to `resume_registration`, which runs the flow with it.
#[test]
fn a_lease_is_offered_only_to_a_running_flow() {
    let f = fixture(false);
    let flows = &f.session.platform.registrations;
    assert!(flows.offer((f.wallet, 99), f.lease()).is_err());
    let lib = f.fake();
    lib.w().proof_arrives = false;
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Broadcast);
    f.spawn(&lib, draft, None);
    eventually("the proof wait", || !lib.w().waits.is_empty());
    assert!(flows.offer((f.wallet, draft), f.lease()).is_ok());
    f.kill(draft);
    assert!(flows.offer((f.wallet, draft), f.lease()).is_err());
}

// Sol r1's probes (/work/scratch/dp1-02-r1-sol/review-probes.patch), as
// regression tests: each failed on 02ae788.

/// R1-01: a lock the library rebuilt from the chain carries index 0
/// whatever index funded it, so a restored row at index 7 cannot tell it
/// from its own: it waits instead of funding a second lock.
#[test]
fn reconstructed_nonzero_index_does_not_fund_twice() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.identity_index = Some(7);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    lib.w().identities.push((0, "earlier-identity".into()));
    let lock = lib.lock_at(0, LockStatus::RecoveredFromChain);
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.counts(), (0, 0), "no second lock");
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.asset_lock_outpoint),
        (Phase::KeysPrepared, None)
    );
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::ChainLock)
    );

    // "Finish transfers" gives the lock to the row waiting for one, not to
    // a new row beside it (which would leave this row to fund again).
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!((report.resumed, report.completed), (1, 1), "{report:?}");
    assert_eq!(f.rows().len(), 1);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(flow::outpoint_of(&row), Some(lock));
    assert_eq!(row.identity_index, Some(7));
    assert_eq!(lib.counts(), (0, 1));
}

/// "Finish transfers" chooses no index for a lock while the funding hold
/// lasts: discovery has not answered which indexes the seed uses.
#[test]
fn finish_adopts_no_lock_before_the_seeds_indexes_are_known() {
    let f = fixture(false);
    let lib = f.fake();
    lib.lock_at(0, LockStatus::RecoveredFromChain);
    lib.ready.store(false, Ordering::SeqCst);
    let finish = |lib: &Fake| {
        f.engine
            .block_on(registration::finish(
                &f.session,
                f.wallet,
                lib,
                f.grant(COIN, 10 * COIN * 1000, Credential::None),
            ))
            .unwrap()
    };
    let report = finish(&lib);
    assert_eq!((report.resumed, report.still_pending), (0, 1));
    assert!(f.rows().is_empty());
    lib.ready.store(true, Ordering::SeqCst);
    let report = finish(&lib);
    assert_eq!((report.resumed, report.completed), (1, 1));
    assert_eq!(lib.counts(), (0, 1));
}

/// DEC-171 (3): the lock built for the index of a row this build cannot
/// read is that row's; "Finish transfers" never gives it to another row.
#[test]
fn finish_leaves_the_lock_of_a_row_this_build_cannot_read() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.identity_index = Some(1);
    row.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    lib.lock_at(1, LockStatus::ChainLocked);
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!(report.resumed, 0, "{report:?}");
    assert_eq!(f.rows().len(), 1);
    assert_eq!(lib.counts(), (0, 0));
    assert!(f.status(&lib, draft).funds_committed);
}

/// R1-04/R1-06: a profile this build cannot read stops the row before
/// anything is funded, and the row is kept for the build that can.
#[test]
fn newer_profile_is_refused_before_funding() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.initial_profile = Some(r#"{"v":2,"display_name":"Alice"}"#.into());
    let draft = f.insert(row);
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.counts(), (0, 0));
    let status = f.status(&lib, draft);
    assert_eq!(status.phase, RegistrationPhase::Failed);
    let failure = status.failure.unwrap();
    assert_eq!(
        (failure.code.as_str(), failure.retryable),
        ("registration.unsupported", false)
    );
    assert!(!status.funds_committed);
    assert_eq!(f.row(draft).phase, Phase::Draft.as_str(), "nothing written");
    assert!(f.discard(&lib, draft).is_err());
    assert_eq!(f.rows().len(), 1);
}

/// R1-03: a `KeysPrepared` row whose own lock the tap recorded is funded;
/// resume takes it to the end, and so does "Finish transfers" once it
/// failed with the broadcast unknown.
#[test]
fn keys_prepared_row_with_own_outpoint_is_recovered() {
    let f = fixture(false);
    let w = f.wallet.to_string();
    let lib = f.fake();
    let outpoint = lib.lock_at(1, LockStatus::ChainLocked);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(1);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let change = RegistrationChange {
        outpoint: Some(flow::canonical(outpoint)),
        ..Default::default()
    };
    f.appdb()
        .update_registration(
            &w,
            draft,
            None,
            Phase::KeysPrepared.as_str(),
            false,
            &change,
            2,
        )
        .unwrap();
    resume(
        &f,
        &lib,
        draft,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (0, 1));

    // The same row failed `broadcast_unknown`: Finish completes it.
    let f = fixture(false);
    let w = f.wallet.to_string();
    let lib = f.fake();
    let outpoint = lib.lock_at(1, LockStatus::ChainLocked);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(1);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let change = RegistrationChange {
        outpoint: Some(flow::canonical(outpoint)),
        failure: Some(Some((flow::broadcast_unknown().into(), false))),
        ..Default::default()
    };
    f.appdb()
        .update_registration(
            &w,
            draft,
            None,
            Phase::KeysPrepared.as_str(),
            false,
            &change,
            2,
        )
        .unwrap();
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!((report.resumed, report.completed), (1, 1), "{report:?}");
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(f.rows().len(), 1, "no row imported for its own lock");
    assert_eq!(lib.counts(), (0, 1));
}

/// R1-05: an identity at the row's index is the row's only through its own
/// lock; one a finished row holds sends the restored row to a new index.
#[test]
fn completed_holder_does_not_steal_restored_flow_identity() {
    let f = fixture(false);
    let lib = f.fake();
    let lock = lib.lock_at(0, LockStatus::Consumed);
    let id = flow::b58(&flow::identity_for(lock));
    lib.w().identities.push((0, id.clone()));
    let mut holder = core_row("");
    holder.identity_index = Some(0);
    holder.identity = Some(id.clone());
    holder.initial_profile = None;
    holder.phase = Phase::Done.as_str().into();
    f.appdb()
        .adopt_registration_lock(&f.wallet.to_string(), &flow::canonical(lock), &holder, 1)
        .unwrap();
    let mut behind = core_row(PLAIN);
    behind.identity_index = Some(0);
    behind.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(behind);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::Done);
    assert_eq!(row.identity_index, Some(1));
    assert_ne!(row.identity, Some(id));
    assert_eq!(lib.counts(), (1, 1));
}

/// R1-01: "Finish transfers" gives a lock rebuilt from the chain a fresh
/// index, never the 0 the rebuild stands in with.
#[test]
fn finish_does_not_reuse_synthetic_recovery_index_zero() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().identities.push((0, "earlier-identity".into()));
    let lock = lib.lock_at(0, LockStatus::RecoveredFromChain);
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert_eq!((report.resumed, report.completed), (1, 1), "{report:?}");
    let indexes: BTreeSet<_> = lib.w().identities.iter().map(|(i, _)| *i).collect();
    assert_eq!(indexes, BTreeSet::from([0, 1]));
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].identity_index, Some(1));
    assert_eq!(flow::outpoint_of(&rows[0]), Some(lock));
}

/// R1-04: a row this build cannot read with a lock behind it has its funds
/// committed, and is never deleted.
#[test]
fn unknown_funding_with_tracked_lock_still_refuses_discard() {
    let f = fixture(false);
    let lib = f.fake();
    let lock = lib.lock_at(1, LockStatus::ChainLocked);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(1);
    row.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    row.phase = Phase::FundingSent.as_str().into();
    let draft = f.insert(row);
    let change = RegistrationChange {
        outpoint: Some(flow::canonical(lock)),
        ..Default::default()
    };
    f.appdb()
        .update_registration(
            &f.wallet.to_string(),
            draft,
            None,
            Phase::FundingSent.as_str(),
            false,
            &change,
            2,
        )
        .unwrap();
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert_eq!(status.failure.unwrap().code, "registration.unsupported");
    assert!(f.discard(&lib, draft).is_err());
    assert_eq!(f.rows().len(), 1);
}

/// R1-07: a user-locked coin is outside the funding balance, and since the
/// pinned asset-lock builder cannot be told to skip it (it selects it, as
/// the offline build shows), a registration is refused while it is locked.
#[test]
fn real_spendable_excludes_a_manually_locked_coin() {
    let f = fixture(false);
    let wallet = f.engine.block_on(f.session.wallet(&f.wallet)).unwrap();
    let outpoint = OutPoint::new(Txid::from_byte_array([0xee; 32]), 0);
    f.engine.block_on(async {
        let address = wallet
            .core()
            .next_receive_address_for_account(0)
            .await
            .unwrap();
        let mut utxo = key_wallet::Utxo::new(
            outpoint,
            dashcore::TxOut {
                value: COIN,
                script_pubkey: address.script_pubkey(),
            },
            address,
            1,
            false,
        );
        utxo.is_confirmed = true;
        wallet
            .state_mut()
            .await
            .core_wallet
            .accounts
            .standard_bip44_accounts
            .get_mut(&0)
            .unwrap()
            .utxos
            .insert(outpoint, utxo);
    });
    f.engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![outpoint]))
        .unwrap();
    let lib = Arc::new(flow::PwLib {
        session: Arc::clone(&f.session),
        wallet_id: f.wallet,
        wallet: Arc::clone(&wallet),
        sdk: f.session.manager().unwrap().sdk_arc(),
    });
    let spendable = f.engine.block_on(lib.spendable()).unwrap();
    assert_eq!((spendable.duffs, spendable.protected), (0, 1));

    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request(PLAIN),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert_eq!(started.unwrap_err().code(), "registration.coins_locked");
    assert!(f.rows().is_empty());

    // Why: the builder spends the locked coin (offline build only).
    let lease = f.lease();
    let signer = lease.funding_signer().unwrap();
    let (tx, _) = f
        .engine
        .block_on(wallet.asset_locks().build_asset_lock_transaction(
            flow::LOCK_DUFFS,
            0,
            platform_wallet::AssetLockFundingType::IdentityRegistration,
            0,
            &signer,
        ))
        .unwrap();
    assert!(tx.input.iter().any(|i| i.previous_output == outpoint));
}
