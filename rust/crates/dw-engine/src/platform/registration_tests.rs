//! DP1-02's state machine against a real session, vault and app.sqlite,
//! with the library scripted ([`Fake`]): the kill matrix, the funding gate
//! and cap, restore adoption, the funding failures (H12), discard and
//! "Finish transfers".

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
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
use crate::send::SpendOwner;
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
        Arc::new(Fake::new(
            self.appdb(),
            self.wallet,
            Arc::clone(&self.session.platform.registrations),
            Arc::downgrade(&self.session),
        ))
    }

    /// A confirmed coin of 1 DASH on the wallet's BIP44 account 0 (no
    /// transaction behind it: the asset-lock builds stay offline).
    fn coin(&self) -> (Arc<platform_wallet::PlatformWallet>, OutPoint) {
        self.coin_at(0)
    }

    /// A confirmed 1 DASH BIP44 coin at output `vout` of txid 0xee….
    fn coin_at(&self, vout: u32) -> (Arc<platform_wallet::PlatformWallet>, OutPoint) {
        let wallet = self
            .engine
            .block_on(self.session.wallet(&self.wallet))
            .unwrap();
        let outpoint = OutPoint::new(Txid::from_byte_array([0xee; 32]), vout);
        self.engine.block_on(async {
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
        (wallet, outpoint)
    }

    fn pw_lib(&self, wallet: &Arc<platform_wallet::PlatformWallet>) -> flow::PwLib {
        flow::PwLib {
            session: Arc::clone(&self.session),
            wallet_id: self.wallet,
            wallet: Arc::clone(wallet),
            sdk: self.session.manager().unwrap().sdk_arc(),
        }
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

    fn claim(&self, draft: i64) -> flow::Claim {
        self.engine
            .block_on(
                self.session
                    .platform
                    .registrations
                    .claim(&self.session, (self.wallet, draft)),
            )
            .unwrap()
            .expect("the flow is free")
    }

    /// `claim`'s run inline, which must end.
    fn run_claim(&self, lib: &Fake, claim: &flow::Claim) {
        let lease = self.lease();
        let run = self.engine.block_on(async {
            let driver = Driver::new(&self.session, lib, claim, Some(lease), false);
            tokio::time::timeout(Duration::from_secs(30), driver.run()).await
        });
        assert!(matches!(run, Ok(Ok(()))), "the run did not end: {run:?}");
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

/// [`eventually`] for async code: waits up to 30 s, leaving the check to the
/// caller.
async fn until(mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !ok() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
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
    /// Platform's profile documents, one per identity.
    profiles: BTreeMap<String, ProfileRecord>,
    /// `register_name` fails with this (MJ-1).
    name_error: Option<NameError>,
    /// Publishing a profile fails with this (MJ-1).
    profile_error: Option<PlatformError>,
    /// The tap does not record the funding's lock: the library's store, or
    /// the tap's own write, failed (MN-1).
    skip_tap: bool,
    profile_updates: u32,
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
    flows: Arc<flow::Flows>,
    /// For the tap's `on_funded`, as the session wires it.
    session: std::sync::Weak<NetworkSession>,
    ready: AtomicBool,
    hung: AtomicBool,
    w: Mutex<World>,
}

impl Fake {
    fn new(
        appdb: Arc<AppDb>,
        wallet: WalletId,
        flows: Arc<flow::Flows>,
        session: std::sync::Weak<NetworkSession>,
    ) -> Self {
        Self {
            appdb,
            wallet,
            flows,
            session,
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

/// What Platform keeps of a published profile.
fn published(profile: &ProfileRecord) -> ProfileRecord {
    ProfileRecord {
        display_name: profile.display_name.clone(),
        public_message: profile.public_message.clone(),
        avatar_url: profile.avatar_url.clone(),
        ..ProfileRecord::default()
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
        if !self.w().skip_tap {
            let funded = flow::lock_tracked(
                &self.flows,
                &self.appdb,
                self.wallet,
                outpoint,
                Some(index),
                false,
            )
            .unwrap();
            // The tap's `on_funded`, as the session wires it.
            if let (Some(draft), Some(session)) = (funded, self.session.upgrade()) {
                flow::backup_once(&session, self.wallet, draft);
            }
        }
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
        let failure = self.w().name_error.clone();
        if let Some(e) = failure {
            return Err(e);
        }
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

    async fn platform_profile(
        &self,
        identity: &str,
    ) -> Result<Option<ProfileRecord>, PlatformError> {
        Ok(self.w().profiles.get(identity).cloned())
    }

    async fn create_profile(
        &self,
        identity: &str,
        _: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        lease.available(super::flows::BudgetPurpose::Credits)?;
        self.maybe_hang(Call::CreateProfile, When::Before).await;
        {
            let mut w = self.w();
            if let Some(e) = w.profile_error.clone() {
                return Err(e);
            }
            // Platform's unique `$ownerId` index: one profile document.
            if w.profiles.contains_key(identity) {
                return Err(PlatformError::Internal {
                    detail: "duplicate unique index $ownerId".into(),
                });
            }
            w.profiles.insert(identity.to_owned(), published(profile));
        }
        self.maybe_hang(Call::CreateProfile, When::After).await;
        Ok(())
    }

    async fn update_profile(
        &self,
        identity: &str,
        _: u32,
        profile: &ProfileRecord,
        lease: &Lease,
    ) -> Result<(), PlatformError> {
        lease.available(super::flows::BudgetPurpose::Credits)?;
        let mut w = self.w();
        if let Some(e) = w.profile_error.clone() {
            return Err(e);
        }
        let Some(current) = w.profiles.get_mut(identity) else {
            return Err(PlatformError::Internal {
                detail: "no profile to update".into(),
            });
        };
        // The library's merge: the fields given replace, the others stay.
        let new = published(profile);
        for (field, value) in [
            (&mut current.display_name, new.display_name),
            (&mut current.public_message, new.public_message),
            (&mut current.avatar_url, new.avatar_url),
        ] {
            if value.is_some() {
                *field = value;
            }
        }
        w.profile_updates += 1;
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
        // Platform's profile is not created twice (MN-3).
        assert_eq!(
            (row.name_error.as_deref(), row.profile_error.as_deref()),
            (None, None),
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
    let moved = flow::lock_tracked(
        &f.session.platform.registrations,
        &f.appdb(),
        f.wallet,
        lock,
        Some(index),
        false,
    )
    .unwrap();
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
            0,
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
            0,
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
            0,
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
            0,
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
    let (wallet, outpoint) = f.coin();
    f.engine
        .block_on(f.session.lock_outpoints(f.wallet, vec![outpoint]))
        .unwrap();
    let lib = Arc::new(f.pw_lib(&wallet));
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

/// Sol r2 R2-02 (DEC-173 (2)): a lock whose index the library lost, which a
/// row this build cannot read and with no outpoint may own, is no other
/// row's. "Finish transfers" imports none and runs none, and the unreadable
/// row's funds count as committed until positive evidence settles it.
#[test]
fn an_unreadable_row_parks_a_lock_whose_index_was_lost() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.identity_index = Some(7);
    row.phase = Phase::KeysPrepared.as_str().into();
    row.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    let draft = f.insert(row);
    lib.lock_at(0, LockStatus::RecoveredFromChain);
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
        lib.counts(),
        (0, 0),
        "Finish took the possibly unread row's lock"
    );
    assert_eq!((f.rows().len(), report.completed), (1, 0));
    assert!(f.status(&lib, draft).funds_committed);
}

/// Polls `fut` once: a claim sends its job to the actor on its first poll.
async fn send_once<F: Future + Unpin>(fut: &mut F) {
    std::future::poll_fn(|cx| {
        assert!(Pin::new(&mut *fut).poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
}

/// Sol r2 R2-01 (DEC-173 (1)): claims run on the wallet's actor in the
/// order they were sent, so a claim sent before its wallet was unloaded
/// and still queued behind it cannot raise the row past the claim sent
/// after: the retired claim's run stops at once, writing nothing, and the
/// reopened one runs to the end.
#[test]
fn a_retired_claim_cannot_invalidate_the_reopened_owner() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = f.insert(core_row(PLAIN));
    let key = (f.wallet, draft);
    let (retired, reopened) = f.engine.block_on(async {
        let flows = &f.session.platform.registrations;
        // Hold the actor so both claims queue behind the unload.
        let (open, gate) = std::sync::mpsc::channel::<()>();
        let mut held = Box::pin(flow::rows_op(&f.session, f.wallet, move |_, _| {
            let _ = gate.recv();
            Ok(())
        }));
        send_once(&mut held).await;
        let mut retired = Box::pin(flows.claim(&f.session, key));
        send_once(&mut retired).await;
        flows.forget_wallet(f.wallet);
        let mut reopened = Box::pin(flows.claim(&f.session, key));
        send_once(&mut reopened).await;
        open.send(()).unwrap();
        held.await.unwrap();
        (retired.await, reopened.await)
    });
    let (retired, reopened) = (retired.unwrap().unwrap(), reopened.unwrap().unwrap());
    assert!(reopened.owner() > retired.owner());
    assert_eq!(f.row(draft).owner, reopened.owner());
    f.run_claim(&lib, &retired);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.owner), (Phase::Draft, reopened.owner()));
    f.run_claim(&lib, &reopened);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
}

/// DEC-173 (1): a run whose generation was lost (the row claimed again)
/// stops at the funding gate instead of stepping again: it funds nothing
/// and the row keeps its phase.
#[test]
fn a_run_that_lost_its_generation_stops_before_funding() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.identity_index = Some(1);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let claim = f.claim(draft);
    let raised = f
        .appdb()
        .claim_registration(&f.wallet.to_string(), draft)
        .unwrap();
    assert!(raised > Some(claim.owner()));
    f.run_claim(&lib, &claim);
    assert_eq!(lib.counts(), (0, 0));
    assert_eq!(phase(&f.row(draft)), Phase::KeysPrepared);
}

/// Sol r2 R2-03 (DEC-173 (1)): the tap's evidence is read and written in
/// one job of the actor, under the owner it read, so an index reassignment
/// sent while it runs waits for it, and then finds the row moved: the row
/// never carries one index with the lock funded for another.
#[test]
fn tap_evidence_cannot_land_inside_an_index_reassignment() {
    let f = fixture(false);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(7);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let flows = Arc::clone(&f.session.platform.registrations);
    let claim = f.claim(draft);
    let lock = OutPoint::new(Txid::from_byte_array([0xab; 32]), 0);
    let (paused, on_pause) = std::sync::mpsc::channel();
    let (go, on_go) = std::sync::mpsc::channel();
    *flow::NOTE_PAUSE.lock().unwrap() = Some((lock, paused, on_go));
    let tap = {
        let (flows, appdb, wallet) = (Arc::clone(&flows), f.appdb(), f.wallet);
        std::thread::spawn(move || flow::lock_tracked(&flows, &appdb, wallet, lock, Some(7), false))
    };
    on_pause.recv().unwrap();
    // The claim's reassignment (`assign_index`), from its read of the row.
    let (session, wallet, owner) = (Arc::clone(&f.session), f.wallet, claim.owner());
    let reassign = f.session.rt.spawn(async move {
        flow::rows_op(&session, wallet, move |db, _| {
            let change = RegistrationChange {
                identity_index: Some(8),
                ..Default::default()
            };
            let w = wallet.to_string();
            db.update_registration(
                &w,
                draft,
                owner,
                Phase::KeysPrepared.as_str(),
                false,
                &change,
                3,
            )
        })
        .await
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!reassign.is_finished(), "a write ran inside the tap's job");
    go.send(()).unwrap();
    assert_eq!(tap.join().unwrap().unwrap(), Some(draft));
    let written = f.engine.block_on(reassign).unwrap().unwrap();
    assert_eq!(written, dw_appdb::RegistrationUpdate::Stale);
    let row = f.row(draft);
    assert_eq!(
        (
            row.identity_index,
            row.asset_lock_outpoint.clone(),
            phase(&row)
        ),
        (
            Some(7),
            Some(flow::canonical(lock).as_str().to_owned()),
            Phase::FundingSent
        )
    );
}

/// Builds the registration lock of `index` offline (nothing is sent),
/// tracks it as built in the wallet's state and reports it through the tap,
/// as the library does; returns the lock and its inputs.
async fn build_and_track(
    f: &Fixture,
    wallet: &platform_wallet::PlatformWallet,
    index: u32,
    lease: &Lease,
) -> (OutPoint, Vec<OutPoint>) {
    let signer = lease.funding_signer().unwrap();
    let (tx, _) = wallet
        .asset_locks()
        .build_asset_lock_transaction(
            flow::LOCK_DUFFS,
            0,
            platform_wallet::AssetLockFundingType::IdentityRegistration,
            index,
            &signer,
        )
        .await
        .unwrap();
    let inputs: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
    let lock = OutPoint::new(tx.txid(), 0);
    let tracked = platform_wallet::TrackedAssetLock {
        out_point: lock,
        transaction: tx,
        account_index: 0,
        funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
        identity_index: index,
        amount: flow::LOCK_DUFFS,
        status: platform_wallet::AssetLockStatus::Built,
        proof: None,
    };
    wallet
        .state_mut()
        .await
        .tracked_asset_locks
        .insert(lock, tracked);
    let flows = &f.session.platform.registrations;
    flow::lock_tracked(flows, &f.appdb(), f.wallet, lock, Some(index), true).unwrap();
    (lock, inputs)
}

/// Whether `coin` reads reserved in the wallet's coin view.
fn reads_reserved(f: &Fixture, coin: OutPoint) -> bool {
    f.engine.block_on(async {
        let fence = f.session.coin_fence(f.wallet).await.unwrap();
        let coins = f.session.fenced_coins(&fence).await.unwrap();
        coins
            .coins
            .iter()
            .any(|c| c.utxo.outpoint == coin && c.reserved)
    })
}

/// Sol r2 R2-04 (DEC-173 (3)): a funding's coin check and the library's
/// selection and signing run under the wallet's coin fence, so "Lock
/// unspent" completes before the check (and the funding is refused) or
/// after the lock is built, never between them. The fence comes down when
/// the built lock is tracked, its inputs then reading reserved.
#[test]
fn a_coin_lock_waits_for_the_funding_it_did_not_precede() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lease = f.lease();
    let locked = Arc::new(AtomicBool::new(false));
    let (inputs, locked_inside, locked_after_tracking) = f.engine.block_on(async {
        let locker = {
            let (session, locked, w) = (Arc::clone(&f.session), Arc::clone(&locked), f.wallet);
            async move {
                session.lock_outpoints(w, vec![coin]).await.unwrap();
                locked.store(true, Ordering::SeqCst);
            }
        };
        let mut locking = None;
        let built = lib
            .fenced(1, async {
                // "Lock unspent" after the check, while the coins are
                // selected and signed.
                locking = Some(tokio::spawn(locker));
                // The locker waits for the fence this call holds.
                until(|| f.session.coin_fences.users(f.wallet) >= 2).await;
                assert_eq!(f.session.coin_fences.users(f.wallet), 2);
                tokio::time::sleep(Duration::from_millis(100)).await;
                let inside = locked.load(Ordering::SeqCst);
                let (_, inputs) = build_and_track(&f, &wallet, 1, &lease).await;
                // The broadcast and the proof wait run unfenced.
                until(|| locked.load(Ordering::SeqCst)).await;
                Ok((inputs, inside, locked.load(Ordering::SeqCst)))
            })
            .await
            .unwrap();
        locking.unwrap().await.unwrap();
        built
    });
    assert!(
        !locked_inside,
        "lock_outpoints completed inside the funding's fence"
    );
    assert!(
        locked_after_tracking,
        "the fence outlived the built lock's tracking"
    );
    // Selected before the lock, and reserved from the build on.
    assert!(inputs.contains(&coin));
    assert!(reads_reserved(&f, coin));
    // With the lock in place the next funding is refused before the library.
    let refused = f.engine.block_on(lib.fenced(1, async { Ok(()) }));
    assert_eq!(refused, Err(FundError::CoinsLocked));
}

/// DEC-173 (3) on E0-04 §2a.6's coin view: a built lock's inputs read
/// reserved while the library tracks the lock, so no engine build spends
/// them, yet they hold no funding back (the library reserves them itself);
/// once the library untracks the lock (positively not sent) they are free,
/// with no reservation of the engine's to clean up.
#[test]
fn a_built_lock_keeps_its_inputs_from_other_spends() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lease = f.lease();
    let (lock, inputs) = f
        .engine
        .block_on(lib.fenced(1, async {
            Ok(build_and_track(&f, &wallet, 1, &lease).await)
        }))
        .unwrap();
    assert!(inputs.contains(&coin));
    assert!(reads_reserved(&f, coin));
    assert!(f.session.spends.snapshot(&f.wallet).is_empty());
    assert_eq!(f.engine.block_on(lib.fenced(1, async { Ok(()) })), Ok(()));
    f.engine.block_on(async {
        wallet.state_mut().await.tracked_asset_locks.remove(&lock);
    });
    assert!(!reads_reserved(&f, coin));
}

/// `lock`, tracked as built, spending `coin`.
fn built_lock(lock: OutPoint, coin: OutPoint) -> platform_wallet::TrackedAssetLock {
    let transaction = dashcore::Transaction {
        version: 3,
        lock_time: 0,
        input: vec![dashcore::TxIn {
            previous_output: coin,
            ..Default::default()
        }],
        output: Vec::new(),
        special_transaction_payload: None,
    };
    platform_wallet::TrackedAssetLock {
        out_point: lock,
        transaction,
        account_index: 0,
        funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
        identity_index: 1,
        amount: flow::LOCK_DUFFS,
        status: platform_wallet::AssetLockStatus::Built,
        proof: None,
    }
}

/// Funds `lock` from `coin` as the library does up to the tap, offline: the
/// lock tracked as built in the wallet, then reported by the tap.
fn track_built(
    f: &Fixture,
    wallet: &Arc<platform_wallet::PlatformWallet>,
    lib: &flow::PwLib,
    lock: OutPoint,
    coin: OutPoint,
) {
    f.engine.block_on(async {
        lib.fenced(1, async {
            // As the library tracks the lock it built, before the tap.
            wallet
                .state_mut()
                .await
                .tracked_asset_locks
                .insert(lock, built_lock(lock, coin));
            let flows = &f.session.platform.registrations;
            flow::lock_tracked(flows, &f.appdb(), f.wallet, lock, Some(1), true).unwrap();
            Ok(())
        })
        .await
        .unwrap();
    });
}

/// The tap reports no block or name times.
struct NoTimes;

impl super::journal::Times for NoTimes {
    fn block_time(&self, _: WalletId, _: &Txid) -> Option<u32> {
        None
    }
    fn name_time(&self, _: WalletId, _: &dpp::prelude::Identifier, _: &str) -> Option<u64> {
        None
    }
}

/// Inside a funding call: the library tracks `tracked` as it does before
/// the tap, a coin reader queues behind the call's fence, and the session's
/// own tap then classifies and records the lock's changeset. Returns the
/// queued reader.
async fn track_and_tap(
    f: &Fixture,
    wallet: &Arc<platform_wallet::PlatformWallet>,
    tracked: platform_wallet::TrackedAssetLock,
) -> tokio::task::JoinHandle<Result<crate::coins::CoinSnapshot, crate::EngineError>> {
    let lock = tracked.out_point;
    let entry = platform_wallet::changeset::AssetLockEntry::from(&tracked);
    wallet
        .state_mut()
        .await
        .tracked_asset_locks
        .insert(lock, tracked);
    let (session, id) = (Arc::clone(&f.session), f.wallet);
    let reader = tokio::spawn(async move {
        let fence = session.coin_fence(id).await?;
        session.fenced_coins(&fence).await
    });
    until(|| f.session.coin_fences.users(f.wallet) >= 2).await;
    assert!(!reader.is_finished());
    let changes = platform_wallet::changeset::PlatformWalletChangeSet {
        asset_locks: Some(platform_wallet::changeset::AssetLockChangeSet {
            asset_locks: [(lock, entry)].into(),
            removed: Default::default(),
        }),
        ..Default::default()
    };
    let classified = super::journal::classify(&changes).unwrap();
    f.session
        .live()
        .unwrap()
        .tap
        .record(f.wallet, classified, &NoTimes);
    reader
}

/// Sol r5 N1, the whole path: a built lock reported through the session's
/// tap (classify, record, `lock_tracked`, `release_built`) releases the
/// funding call's fence while the call still runs, so a queued coin reader
/// completes and reads the lock's input reserved by the library, not by a
/// stored claim; the row is claimed and funding-sent with the lock.
#[test]
fn a_tapped_built_lock_frees_the_fence_before_its_call_returns() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let mut row = core_row(PLAIN);
    row.identity_index = Some(0);
    row.phase = Phase::KeysPrepared.as_str().into();
    let draft = f.insert(row);
    let claim = f.claim(draft);
    let lock = OutPoint::new(Txid::from_byte_array([0xe3; 32]), 0);
    f.engine.block_on(async {
        lib.fenced(0, async {
            let mut tracked = built_lock(lock, coin);
            tracked.identity_index = 0;
            let reader = track_and_tap(&f, &wallet, tracked).await;
            // Still inside the call: only the tap can have let it through.
            let read = tokio::time::timeout(Duration::from_secs(30), reader)
                .await
                .expect("the tap released the fence")
                .unwrap()
                .unwrap();
            assert!(read.coins[0].reserved && read.coins[0].lock_input);
            assert!(f.session.spends.snapshot(&f.wallet).is_empty());
            let row = f.row(draft);
            assert_eq!(row.owner, claim.owner());
            assert_eq!(phase(&row), Phase::FundingSent);
            assert_eq!(
                row.asset_lock_outpoint.as_deref(),
                Some(flow::canonical(lock).as_str())
            );
            Ok(())
        })
        .await
        .unwrap();
    });
}

/// Sol r5 N1, the control: a lock first tapped as broadcast does not
/// release the fence (Phase A releases on built only, L2), so the queued
/// reader waits for the call to return.
#[test]
fn a_tapped_broadcast_lock_keeps_the_fence_until_its_call_returns() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lock = OutPoint::new(Txid::from_byte_array([0xe5; 32]), 0);
    f.engine.block_on(async {
        let mut queued = None;
        lib.fenced(1, async {
            let mut tracked = built_lock(lock, coin);
            tracked.status = platform_wallet::AssetLockStatus::Broadcast;
            let mut reader = track_and_tap(&f, &wallet, tracked).await;
            let early = tokio::time::timeout(Duration::from_millis(200), &mut reader).await;
            assert!(early.is_err(), "the fence came down on a broadcast tap");
            queued = Some(reader);
            Ok(())
        })
        .await
        .unwrap();
        let read = tokio::time::timeout(Duration::from_secs(30), queued.unwrap())
            .await
            .expect("the call's return released the fence")
            .unwrap()
            .unwrap();
        assert!(read.coins[0].reserved && read.coins[0].lock_input);
    });
}

/// Sol r3 R3-01: a built lock's inputs read reserved only while the library
/// tracks it (the coin view derives them, E0-04 §2a.6), and hold no funding
/// back; once it stops (a later rejection, a sweep) nothing stale is left,
/// so the ordinary `start` gate does not refuse on its own funding's lock.
#[test]
fn an_untracked_locks_reservation_does_not_block_the_next_start() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lock = OutPoint::new(Txid::from_byte_array([0xda; 32]), 0);
    track_built(&f, &wallet, &lib, lock, coin);
    // Tracked: the coin reads reserved, the library's own reservation.
    let held = f.engine.block_on(lib.spendable()).unwrap();
    assert_eq!(held.protected, 0);
    assert!(reads_reserved(&f, coin));
    // The library drops the lock, as a later definite rejection does.
    untrack(&f, &wallet, lock);
    assert!(!reads_reserved(&f, coin));
    let grant = f.grant(COIN, 10 * COIN * 1000, Credential::None);
    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::new(f.pw_lib(&wallet)),
        request(PLAIN),
        grant,
    ));
    assert!(
        !matches!(started, Err(RegistrationError::CoinsLocked)),
        "the stale reservation refused the next start: {started:?}"
    );
    assert_eq!(f.rows().len(), 1);
}

/// R3-01, DEC-179: the lock's untracking frees its inputs and no other
/// operation's reservation, of another coin or of the same one.
#[test]
fn an_untracked_lock_frees_its_inputs_and_no_other_reservation() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let (_, other) = f.coin_at(1);
    let lib = f.pw_lib(&wallet);
    let lock = OutPoint::new(Txid::from_byte_array([0xdb; 32]), 0);
    track_built(&f, &wallet, &lib, lock, coin);
    let mixing = SpendOwner::mixing_session();
    f.session.spends.reserve(f.wallet, mixing, [other]);
    // A CoinJoin session that chose the lock's coin before the fence (it
    // reserves outside the fence) claims it too.
    f.session.spends.reserve(f.wallet, mixing, [coin]);
    untrack(&f, &wallet, lock);
    assert!(
        reads_reserved(&f, coin),
        "the session's claim went with the lock"
    );
    assert!(reads_reserved(&f, other));
    f.session.spends.release(&f.wallet, mixing);
    assert!(!reads_reserved(&f, coin) && !reads_reserved(&f, other));
}

/// The library drops `lock` (a later definite rejection).
fn untrack(f: &Fixture, wallet: &Arc<platform_wallet::PlatformWallet>, lock: OutPoint) {
    f.engine.block_on(async {
        wallet.state_mut().await.tracked_asset_locks.remove(&lock);
    });
}

/// Sol r4 R4-01, DEC-179: a prepared send holding the same coin as a built
/// lock the library later drops keeps its reservation through the lock's
/// untracking, until the send itself releases it. (Sol's probe reaches the
/// two by pausing the send's selection before the funding's fence; here the
/// send is prepared first and the library then tracks a lock of its coin.)
#[test]
fn a_prepared_send_keeps_its_coin_through_a_locks_untracking() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let draft = f.session.new_tx_draft(f.wallet).unwrap();
    draft
        .set_source(crate::send::CoinSource::Outpoints(vec![coin]))
        .unwrap();
    draft
        .set_recipients(vec![crate::send::Recipient {
            address: "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n".into(),
            amount: 10_000_000,
            subtract_fee_from_amount: false,
            label: None,
            message: None,
        }])
        .unwrap();
    let grant = f
        .session
        .vault()
        .authorize(
            GrantPurpose::Spend { max_duffs: COIN },
            Some(&f.wallet.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let prepared = f.engine.block_on(draft.prepare(grant)).unwrap();
    let raw = prepared.raw().unwrap();
    let tx: dashcore::Transaction = dashcore::consensus::deserialize(&raw).unwrap();
    assert!(tx.input.iter().any(|i| i.previous_output == coin));
    let lock = OutPoint::new(Txid::from_byte_array([0xd6; 32]), 0);
    f.engine.block_on(async {
        wallet
            .state_mut()
            .await
            .tracked_asset_locks
            .insert(lock, built_lock(lock, coin));
    });
    untrack(&f, &wallet, lock);
    assert!(
        reads_reserved(&f, coin),
        "the lock's untracking freed the prepared send's coin"
    );
    // The send's own release frees it.
    drop(prepared);
    assert!(!reads_reserved(&f, coin));
}

/// DEC-173 (2), DEC-171 (2): a lock is a row's by its index, so a known
/// index's lock is not parked for a row this build cannot read that has no
/// index: "Finish transfers" imports it, and that row's funds stay
/// uncommitted.
#[test]
fn an_unreadable_row_without_an_index_parks_no_indexed_lock() {
    let f = fixture(false);
    let lib = f.fake();
    let mut row = core_row(PLAIN);
    row.phase = Phase::KeysPrepared.as_str().into();
    row.funding = r#"{"v":2,"kind":"core_balance","lock_duffs":3000000}"#.into();
    let draft = f.insert(row);
    let lock = lib.lock_at(4, LockStatus::ChainLocked);
    f.engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    let held = f
        .rows()
        .into_iter()
        .filter(|r| r.id != draft)
        .any(|r| r.asset_lock_outpoint.as_deref() == Some(flow::canonical(lock).as_str()));
    assert!(held, "the indexed lock stayed parked: {:?}", f.rows());
    assert!(!f.status(&lib, draft).funds_committed);
}

/// MJ-1: once the identity exists, a name step that fails for good
/// settles the row: `Done`, the name's failure recorded for the names flow,
/// the profile still published, and no later registration blocked.
#[test]
fn a_failed_name_after_the_identity_settles_the_row() {
    let cases = [
        NameError::Taken,
        NameError::ContestOpen,
        NameError::Locked,
        NameError::Invalid { rules: vec![] },
    ];
    for error in cases {
        let f = fixture(false);
        let lib = f.fake();
        // The name is taken while the flow waits for its proof.
        lib.w().fund_script = Some(FundScript::IsTimeout);
        lib.w().proof_arrives = false;
        let draft = f.insert(core_row(PLAIN));
        f.drive(&lib, draft, Some(f.lease()), false);
        assert_eq!(phase(&f.row(draft)), Phase::ProofWaiting, "{error:?}");
        let code = error.code();
        lib.w().name_error = Some(error);
        lib.w().proof_arrives = true;
        f.drive(&lib, draft, Some(f.lease()), true);
        let row = f.row(draft);
        assert_eq!(
            (phase(&row), row.error.as_deref()),
            (Phase::Done, None),
            "{code}"
        );
        assert_eq!(row.name_error.as_deref(), Some(code));
        assert!(row.identity.is_some());
        assert!(lib.w().names.is_empty());
        assert_eq!(
            lib.w().profiles.len(),
            1,
            "{code}: the profile is published"
        );
        let status = f.status(&lib, draft);
        assert_eq!(status.phase, RegistrationPhase::Done, "{code}");
        assert_eq!(
            (
                status.name_failure.as_deref(),
                status.profile_failure.as_deref()
            ),
            (Some(code), None)
        );
        let start = f.engine.block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request("erin2"),
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ));
        assert!(start.is_ok(), "{code}: {start:?}");
    }
}

/// MJ-1: a profile Platform refuses for good after the identity settles
/// the row too, with the name registered.
#[test]
fn a_failed_profile_after_the_identity_settles_the_row() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().profile_error = Some(PlatformError::InvalidArgument {
        detail: "display name too long".into(),
    });
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error.as_deref()), (Phase::Done, None));
    assert_eq!(
        (row.name_error.as_deref(), row.profile_error.as_deref()),
        (None, Some("invalid_argument"))
    );
    assert_eq!(lib.w().names.len(), 1);
    assert!(lib.w().profiles.is_empty());
    let status = f.status(&lib, draft);
    assert_eq!(status.profile_failure.as_deref(), Some("invalid_argument"));
    let start = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("erin2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert!(start.is_ok(), "{start:?}");
}

/// MJ-1: a row an earlier build failed for good after its identity (the
/// wedge) no longer counts as in progress.
#[test]
fn a_failed_row_with_an_identity_is_not_in_progress() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().identities.push((0, "earlier".into()));
    let mut row = core_row(PLAIN);
    row.identity_index = Some(0);
    row.identity = Some("earlier".into());
    row.phase = Phase::NameRequested.as_str().into();
    let draft = f.insert(row);
    f.appdb()
        .update_registration(
            &f.wallet.to_string(),
            draft,
            0,
            Phase::NameRequested.as_str(),
            false,
            &RegistrationChange {
                failure: Some(Some(("name.taken".into(), false))),
                ..Default::default()
            },
            2,
        )
        .unwrap();
    assert_eq!(f.status(&lib, draft).phase, RegistrationPhase::Failed);
    let start = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("erin2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert!(start.is_ok(), "{start:?}");
}

/// MN-1: the funding call's own lock reaches its row though its outpoint
/// never did (the tap's write failed), at index 0 too, where the index
/// alone cannot say whose a `Broadcast` lock is; then Finish has nothing
/// left to adopt.
#[test]
fn an_untapped_index_zero_lock_is_its_calls_own() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().skip_tap = true;
    lib.w().proof_arrives = false;
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(row.identity_index, Some(0));
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::ProofWaiting, None)
    );
    let lock = lib.w().locks[0].outpoint;
    assert_eq!(flow::outpoint_of(&row), Some(lock));
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    // The row's own run resumes, still waiting for the ChainLock; no row is
    // made for its lock.
    assert_eq!((report.resumed, report.still_pending), (1, 1), "{report:?}");
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(flow::outpoint_of(&rows[0]), Some(lock));
}

/// MN-1 control: at index 1 the index says it as well.
#[test]
fn an_untapped_index_one_lock_is_its_calls_own() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().skip_tap = true;
    lib.w().proof_arrives = false;
    lib.w().identities.push((0, "earlier".into()));
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(row.identity_index, Some(1));
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::ProofWaiting, None)
    );
    assert_eq!(flow::outpoint_of(&row), Some(lib.w().locks[0].outpoint));
}

/// MN-1: the lock's record is retried; one that never lands ends the run
/// as a storage error with the row still bound to its index, unfailed, so
/// it funds nothing again and Finish hands it its own lock.
#[test]
fn a_lock_record_that_fails_is_retried_then_left_to_its_row() {
    let f = fixture(false);
    let flows = &f.session.platform.registrations;
    // Two faults: the third try lands.
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().skip_tap = true;
    lib.w().proof_arrives = false;
    let draft = f.insert(core_row(PLAIN));
    flows.fail_lock_writes((f.wallet, draft), 2);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::ProofWaiting, None)
    );
    assert_eq!(flow::outpoint_of(&row), Some(lib.w().locks[0].outpoint));

    // Three: the run fails as storage.
    let f = fixture(false);
    let flows = &f.session.platform.registrations;
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().skip_tap = true;
    lib.w().proof_arrives = false;
    let draft = f.insert(core_row(PLAIN));
    flows.fail_lock_writes((f.wallet, draft), 3);
    let lease = f.lease();
    let run = f.engine.block_on(async {
        let claim = flows
            .claim(&f.session, (f.wallet, draft))
            .await
            .unwrap()
            .expect("the flow is free");
        Driver::new(&f.session, &*lib, &claim, Some(lease), false)
            .run()
            .await
    });
    assert_eq!(run.unwrap_err().code(), "storage");
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref(), row.identity_index),
        (Phase::KeysPrepared, None, Some(0))
    );
    assert_eq!(row.asset_lock_outpoint, None);
    let lock = lib.w().locks[0].outpoint;
    // Still in progress: no second registration, and its own run funds
    // nothing again.
    let start = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("erin2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert_eq!(start, Err(RegistrationError::InProgress));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.counts(), (1, 0));
    // Finish attributes the lock to this row; a resume completes it.
    f.engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    let rows = f.rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].id, draft);
    assert_eq!(flow::outpoint_of(&rows[0]), Some(lock));
    lib.w().proof_arrives = true;
    resume(
        &f,
        &lib,
        draft,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(f.rows().len(), 1);
    assert_eq!(lib.counts(), (1, 1));
}

/// MN-2: a funding definitely rejected after its tap scheduled the backup
/// unschedules it; the retry that funds schedules it again.
#[test]
fn a_retried_funding_schedules_the_backup_again() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::Rejected);
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    let flows = &f.session.platform.registrations;
    let state = flows.get((f.wallet, draft));
    assert_eq!((state.backups, state.backed_up), (1, false));
    lib.w().fund_script = Some(FundScript::Ok);
    resume(
        &f,
        &lib,
        draft,
        Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
    );
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    let state = flows.get((f.wallet, draft));
    assert_eq!((state.backups, state.backed_up), (2, true));
}

/// MN-3: the profile step reads Platform first. A profile that says what
/// the row's does is published; a different one is updated, keeping the
/// fields the row leaves out.
#[test]
fn the_profile_step_reads_platform_first() {
    // The identity index 0's lock builds.
    let lock = OutPoint::new(Txid::from_byte_array([1; 32]), 0);
    let id = flow::b58(&flow::identity_for(lock));
    let alice = ProfileRecord {
        display_name: Some("Alice".into()),
        ..ProfileRecord::default()
    };
    let older = ProfileRecord {
        display_name: Some("Old".into()),
        public_message: Some("hi".into()),
        ..ProfileRecord::default()
    };
    for (on_platform, updates) in [(alice.clone(), 0), (older, 1)] {
        let f = fixture(false);
        let lib = f.fake();
        lib.w().profiles.insert(id.clone(), on_platform.clone());
        let draft = f.insert(core_row(PLAIN));
        f.drive(&lib, draft, Some(f.lease()), true);
        let row = f.row(draft);
        assert_eq!(row.identity.as_deref(), Some(id.as_str()));
        assert_eq!(
            (
                phase(&row),
                row.error.as_deref(),
                row.profile_error.as_deref()
            ),
            (Phase::Done, None, None),
            "{on_platform:?}"
        );
        let w = lib.w();
        assert_eq!(w.profile_updates, updates, "{on_platform:?}");
        let published = &w.profiles[&id];
        assert_eq!(published.display_name.as_deref(), Some("Alice"));
        assert_eq!(published.public_message, on_platform.public_message);
    }
}

mod rules;
