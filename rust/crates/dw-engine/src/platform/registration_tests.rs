//! DP1-02's state machine against a real session, vault and app.sqlite,
//! with the library scripted ([`Fake`]): the kill matrix, the funding gate
//! and cap, restore adoption, the funding failures (H12), discard and
//! "Finish transfers".

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
use super::lease::mode_b::{TrackedRow, TrackedStatus, index_trusted};
use super::lease::{CallPermit, FundingKey, Lease, StoredLock};
use super::names::NameOutcome;
use super::recovery::{BoxedFuture, MockPlatform};
use super::registration::{
    self, InitialProfile, RegistrationFunding, RegistrationPhase, RegistrationRequest,
    RegistrationWait,
};
use super::registration_flow::{
    self as flow, Driver, FundError, LockInfo, LockStatus, RegistrationLib, Spendable,
};
use super::registration_rows::{Funding, Phase, ProfileRecord};
use super::runtime::guard;
use super::{SpvState, StartupStatus};
use crate::send::SpendOwner;
use crate::session::Manager;
use crate::{
    DashNetwork, Engine, EngineConfig, EngineError, ImportOptions, NetworkSession, WalletId,
};

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
        let registrations = &self.session.platform.registrations;
        let task = registrations.get((self.wallet, draft)).task;
        registrations.forget((self.wallet, draft));
        eventually("the killed flow is gone", || !self.running(draft));
        // A restart ends its library calls too: their permits end as they
        // would with the process (the aborted task drops them).
        eventually("the killed flow's calls ended", || {
            task.as_ref().is_none_or(|t| t.is_finished())
        });
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
    /// With `profile_error`: the put landed all the same (an earlier put
    /// in flight, or an outcome the error hid).
    profile_lands: bool,
    /// The tap does not record the funding's lock: the library's store, or
    /// the tap's own write, failed (MN-1).
    skip_tap: bool,
    /// P2b keeps no record of the lock the funding call signed: its write
    /// failed and the process restarted, or the journal was not ready
    /// (E0-04 §2a.6).
    skip_signed: bool,
    /// The input the funding's lock spends: a real coin of the wallet, for
    /// Repair's self-spend to take (else a made-up one).
    lock_input: Option<OutPoint>,
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
    /// SPV is behind the network (DEC-197).
    unsynced: bool,
    /// What each `register_identity` call meets, in order; none left, it
    /// creates the identity at once.
    identity_script: VecDeque<IdentityAttempt>,
}

/// One `register_identity` call as step 2's retries play out inside it
/// (E0-04 §2a.3, F10, F16): Platform refuses a ChainLock proof whose height
/// it has not reached, and the library signs the transition again.
#[derive(Debug, Clone, Copy)]
enum IdentityAttempt {
    /// The first attempt's signature was released and submitted; the
    /// re-sign met a locked signer. `executes`: the first attempt lands on
    /// Platform all the same.
    ReleasedThenLocked { executes: bool },
    /// The identity keys signed and were released; the proof's signer met
    /// the vault locked under the call.
    ReleasedThenVaultLocked,
    /// Every re-sign was released and refused on the height until the
    /// library gave up: nothing landed.
    ReleasedThenUnavailable,
    /// The height caught up at the second attempt: created.
    RetriedThenCreated,
}

/// The tap's report of the lock `outpoint`, as the journal builds it: its
/// index known (`Built`), or lost (index 0, past `Built`).
fn tapped(outpoint: OutPoint, index: Option<u32>) -> LockInfo {
    LockInfo {
        outpoint,
        registration: true,
        identity_index: index.unwrap_or(0),
        account: 0,
        amount: flow::LOCK_DUFFS,
        status: match index {
            Some(_) => LockStatus::Built,
            None => LockStatus::Broadcast,
        },
    }
}

/// platform-wallet as the flow sees it, scripted.
struct Fake {
    appdb: Arc<AppDb>,
    wallet: WalletId,
    flows: Arc<flow::Flows>,
    /// For the tap's `on_funded`, as the session wires it.
    session: std::sync::Weak<NetworkSession>,
    ready: AtomicBool,
    /// The hold is the session's real bring-up outcome's, SPV counted as
    /// synced ([`flow::startup_hold`]).
    startup_hold: AtomicBool,
    hung: AtomicBool,
    /// `funding_permit` waits while set, before P2b marks the step: the
    /// window between `enter_funding` and the marker (OD-05). Sets `hung`.
    permit_hold: AtomicBool,
    /// `spendable` waits while set: the window between the funding step's
    /// ledger read (the row `Unfunded` at a free index) and `enter_funding`.
    /// Sets `hung`.
    spendable_hold: AtomicBool,
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
            startup_hold: AtomicBool::new(false),
            hung: AtomicBool::new(false),
            permit_hold: AtomicBool::new(false),
            spendable_hold: AtomicBool::new(false),
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
    /// Platform's IdentityCreate from the lock at `outpoint`: the identity
    /// at `index`, the lock consumed. Never twice for one lock.
    fn create_identity(&self, outpoint: OutPoint, index: u32) -> String {
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
        id
    }

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
        if self.spendable_hold.load(Ordering::SeqCst) {
            self.hung.store(true, Ordering::SeqCst);
            while self.spendable_hold.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        Ok(self.w().spendable)
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        if self.startup_hold.load(Ordering::SeqCst) {
            let session = self.session.upgrade().expect("the fixture's session");
            return match session.dashpay_startup(&self.wallet) {
                Ok(s) => flow::startup_hold(s.startup),
                Err(_) => Some(RegistrationWait::Sync),
            };
        }
        (!self.ready.load(Ordering::SeqCst)).then_some(RegistrationWait::Sync)
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        lock_duffs: u64,
    ) -> Result<CallPermit, FundError> {
        if self.permit_hold.load(Ordering::SeqCst) {
            self.hung.store(true, Ordering::SeqCst);
            while self.permit_hold.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let session = self.session.upgrade().expect("the fixture's session");
        let fence = session
            .coin_fence(self.wallet)
            .await
            .map_err(|e| FundError::Refused(e.into()))?;
        session
            .funding_call_permit(lease, step.clone(), lock_duffs, step, key, fence)
            .await
            .map_err(FundError::Refused)
    }

    async fn fund(&self, _: u64, index: u32, permit: &CallPermit) -> Result<OutPoint, FundError> {
        permit
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
            FundScript::LostUntracked => {
                // Signed, then lost with the wallet closing under the call.
                permit.note_signed([0xfe; 32]);
                return Err(FundError::Failed(PlatformError::Unavailable));
            }
            _ => {}
        }
        // Signed by the permit's funding signer, then tracked as Built and
        // stored: P2b's provenance check records it as the step's own before
        // the persister takes it, then the tap reports it.
        let outpoint = self.lock_at(index, LockStatus::Built);
        let sighash = outpoint.txid.to_byte_array();
        permit.note_signed(sighash);
        if !self.w().skip_signed {
            let session = self.session.upgrade().expect("the fixture's session");
            session.leases.record_signed_locks(
                self.wallet,
                &[StoredLock {
                    txid: outpoint.txid,
                    inputs: vec![
                        self.w()
                            .lock_input
                            .unwrap_or(OutPoint::new(Txid::from_byte_array([0x77; 32]), index)),
                    ],
                    sighashes: Some(vec![sighash]),
                    built: true,
                }],
            );
        }
        if !self.w().skip_tap {
            let session = self.session.upgrade().expect("the fixture's session");
            let funded = flow::lock_tracked(
                &self.flows,
                &self.appdb,
                &session.leases,
                self.wallet,
                tapped(outpoint, Some(index)),
            )
            .unwrap();
            // The tap's `on_funded`, as the session wires it.
            if let Some(draft) = funded {
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

    async fn spv_synced(&self) -> bool {
        !self.w().unsynced
    }

    async fn tracked_row(&self, outpoint: OutPoint) -> Option<TrackedRow> {
        let lock = self
            .w()
            .locks
            .iter()
            .find(|l| l.outpoint == outpoint)?
            .clone();
        let key = FundingKey {
            funding_type: if lock.registration {
                platform_wallet::AssetLockFundingType::IdentityRegistration
            } else {
                platform_wallet::AssetLockFundingType::IdentityTopUp
            },
            identity_index: lock.identity_index,
            account: lock.account,
        };
        let status = match lock.status {
            LockStatus::Built => TrackedStatus::Built,
            LockStatus::Broadcast => TrackedStatus::Broadcast,
            LockStatus::InstantSendLocked => TrackedStatus::InstantSendLocked,
            LockStatus::ChainLocked => TrackedStatus::ChainLocked,
            LockStatus::Consumed => TrackedStatus::Consumed,
            LockStatus::RecoveredFromChain => TrackedStatus::RecoveredFromChain,
        };
        // As `fund` signs it: one input, its sighash the txid's bytes.
        Some(TrackedRow {
            txid: outpoint.txid,
            key,
            status,
            inputs: vec![self.w().lock_input.unwrap_or(OutPoint::new(
                Txid::from_byte_array([0x77; 32]),
                lock.identity_index,
            ))],
            trusted: index_trusted(&key, status),
            sighashes: status
                .pending()
                .then(|| vec![outpoint.txid.to_byte_array()]),
        })
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
        permit: &CallPermit,
    ) -> Result<String, PlatformError> {
        // The proof is signed with the lock's one-time key: the funding
        // signer, as `PwLib` asks.
        permit.identity_signer(&[index])?;
        permit.lease().uncounted_funding_signer()?;
        self.maybe_hang(Call::RegisterIdentity, When::Before).await;
        let attempt = self.w().identity_script.pop_front();
        match attempt {
            Some(IdentityAttempt::ReleasedThenLocked { executes }) => {
                permit.note_released();
                if executes {
                    self.create_identity(outpoint, index);
                }
                permit.signatures().note_locked();
                // As the library reports a signer's refusal: its own
                // error, not the lease's.
                return Err(PlatformError::Internal {
                    detail: "the identity signer refused: locked".into(),
                });
            }
            Some(IdentityAttempt::ReleasedThenVaultLocked) => {
                permit.note_released();
                // The proof's signer, which no permit counts, meets the
                // lock: only the vault's state shows it.
                (self.session.upgrade().unwrap()).lock_vault_sync().unwrap();
                return Err(PlatformError::Internal {
                    detail: "the proof signer refused: locked".into(),
                });
            }
            Some(IdentityAttempt::ReleasedThenUnavailable) => {
                permit.note_released();
                permit.note_released();
                return Err(PlatformError::Unavailable);
            }
            Some(IdentityAttempt::RetriedThenCreated) => {
                permit.note_released();
                permit.note_released();
            }
            None => permit.note_released(),
        }
        let id = self.create_identity(outpoint, index);
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
                if w.profile_lands {
                    w.profiles.insert(identity.to_owned(), published(profile));
                }
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

/// A kill inside the funding call before its signature: the step's marker
/// stands and P2b has no `NotSent` row for it (the in-process resolution
/// did not land), so the step is `Unknown` (DP1-02 §3.1) and the row waits,
/// funding nothing, until PA-2 resolves it (§8: without PA-2 a wedge).
/// A restart: the session closes and opens again, and P2b derives its
/// funding statuses from what is durable. With `journal`, the dispatch
/// journal is that copy, as a crash left it.
fn reopen_with(f: &mut Fixture, journal: Option<&std::path::Path>) {
    f.engine
        .block_on(f.engine.close_network(DashNetwork::Regtest))
        .unwrap();
    if let Some(journal) = journal {
        let path = f.session.data_dir().join("dispatch.sqlite");
        for suffix in ["-wal", "-shm"] {
            let mut side = path.clone().into_os_string();
            side.push(suffix);
            let _ = std::fs::remove_file(side);
        }
        std::fs::copy(journal, &path).unwrap();
    }
    f.session = f
        .engine
        .block_on(f.engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    assert!(
        f.engine
            .block_on(f.session.leases.refresh_funding(f.wallet))
    );
}

/// The process dies while the run's funding call waits before its
/// signature: its marker is durable, and no permit's end runs. Returns the
/// library of the new session and the row, after one run.
fn crash_before_the_funding_signature(f: &mut Fixture) -> (Arc<Fake>, i64) {
    let lib = f.fake();
    lib.hang_at(Call::Fund, When::Before);
    let draft = f.insert(core_row(PLAIN));
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("the funding call runs", || lib.hung.load(Ordering::SeqCst));
    let crashed = f._dir.path().join("crashed-dispatch.sqlite");
    rusqlite::Connection::open(f.session.data_dir().join("dispatch.sqlite"))
        .unwrap()
        .execute("VACUUM INTO ?1", [crashed.to_str().unwrap()])
        .unwrap();
    f.kill(draft);
    reopen_with(f, Some(&crashed));
    let lib = f.fake();
    f.drive(&lib, draft, Some(f.lease()), true);
    (lib, draft)
}

/// A flow aborted in a live process before its funding signature: the
/// permit's end resolves the unsigned step `NotSent` and drops its marker
/// (H12), so the run calls the same step again and completes.
#[test]
fn a_flow_aborted_before_the_funding_signature_retries_its_step() {
    let f = fixture(false);
    let lib = f.fake();
    lib.hang_at(Call::Fund, When::Before);
    let draft = f.insert(core_row(PLAIN));
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("the funding call runs", || lib.hung.load(Ordering::SeqCst));
    f.kill(draft);
    lib.w().hang = None;
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.error), (Phase::Done, None));
    assert_eq!(lib.counts().0, 1);
}

/// DP1-02 §11.2 item 12, C5 (§8.1, Q14): under PA-2 and OD-02 the flagged
/// marker with no durable record and no running call reads `NotSent` but
/// stands, so its step is cancelled: the row closes
/// `Cancelled(funding_cancelled)`, nothing is committed, a new start is
/// allowed, and the step is never called again.
#[test]
fn a_crash_before_the_funding_signature_closes_its_row() {
    let mut f = fixture(false);
    let (lib, draft) = crash_before_the_funding_signature(&mut f);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.draft, s.status, s.durable_record))
            .collect::<Vec<_>>(),
        [(draft, Some(crate::platform::DispatchState::NotSent), true)]
    );
    let row = f.row(draft);
    assert_eq!(
        (row.error.as_deref(), row.retryable),
        (Some("registration.funding_cancelled"), false)
    );
    let status = f.status(&lib, draft);
    assert!(!status.funds_committed);
    assert_eq!(lib.counts(), (0, 0), "the cancelled step was called again");
    f.engine
        .block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request("afterc5"),
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
}

/// Every transition, killed before the library acted, mid-funding and
/// after it acted: the restarted flow ends registered, with one lock, one
/// identity, one name and one profile.
#[test]
fn a_kill_at_any_transition_resumes_to_one_registration() {
    let f = fixture(false);
    // (Fund, Before): `a_crash_before_the_funding_signature_closes_its_row`
    // and `a_flow_aborted_before_the_funding_signature_retries_its_step`.
    let points = [
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
    assert!(matches!(
        start("bob2"),
        Err(RegistrationError::InProgress { draft: d }) if d == draft.to_string()
    ));
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
    // The quote reports the same spendable set the start checks (DP1-09 D5).
    let short = f
        .engine
        .block_on(registration::quote(&*lib, &request(PLAIN)))
        .unwrap();
    assert_eq!(short.available_duffs, quote.total_duffs - 1);
    assert!(short.available_duffs < short.total_duffs);
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
    // The drive's claim is gone: nothing runs the row (DP1-09 D26).
    assert!(!status.running);
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
            FundScript::FailUntracked => {
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
            // A lock signed, then untracked as swept or rejected: the pin
            // cannot tell a rejection a peer may yet relay (P2b review M-2).
            FundScript::Swept | FundScript::Rejected => {
                // The outpoint reached the row; no tracked row is evidence
                // of nothing: MaybeSent, waiting for the chain.
                assert_eq!(
                    (phase(&row), row.error.as_deref()),
                    (Phase::FundingSent, None),
                    "{script:?}"
                );
                assert!(row.asset_lock_outpoint.is_some(), "{script:?}");
                assert!(status.funds_committed, "{script:?}");
                assert_eq!(status.waiting, Some(RegistrationWait::ChainLock));
                let discard = f.discard(&lib, draft);
                assert_eq!(discard.unwrap_err().code(), "invalid_argument");
            }
            _ => unreachable!(),
        }
    }
}

/// A funding call that signed and failed before the library stored its
/// lock (the wallet closing under the call): its funding signature was
/// released, so P2b's `funding_release` row stands and, with no lock
/// recorded, the step reads `MaybeSent` (E0-04 §2a.7, P2b r6 R6-01): a lock
/// may have reached a broadcaster no store of it passed. The row waits
/// `FundingUnknown`, its funds committed: discard and a new registration
/// are refused, and the step is never called again. A lock at its index
/// later is no row's (`Contested`): only the wedged step contests it, with
/// its release known, so Finish recovers it into a recovery row (DEC-198
/// am.1), which makes the lock's one identity; the row stays waiting.
#[test]
fn a_signature_lost_before_its_store_leaves_the_step_maybe_sent() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::LostUntracked);
    // An earlier identity: the flow takes index 1, which a lock rebuilt
    // from the chain keeps (only index 0 is the library's stand-in).
    lib.w().identities.push((0, "earlier".into()));
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), true);
    let steps = f
        .session
        .leases
        .registration_funding_steps(f.wallet)
        .unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.draft, s.status, s.signed.len(), s.durable_record))
            .collect::<Vec<_>>(),
        vec![(
            draft,
            Some(crate::platform::DispatchState::MaybeSent),
            0,
            true
        )]
    );
    let row = f.row(draft);
    assert_eq!(phase(&row), Phase::KeysPrepared);
    let index = row.identity_index.unwrap();
    assert_eq!(index, 1);
    let status = f.status(&lib, draft);
    assert!(status.funds_committed);
    assert_eq!(status.waiting, Some(RegistrationWait::FundingUnknown));
    assert_eq!(lib.counts(), (1, 0), "the step was called again");

    // A lock shows up at the index (a restore's reconstruction): it may be
    // the call's or another device's, so the row takes none.
    let lock = lib.lock_at(index, LockStatus::ChainLocked);
    let moved = flow::lock_tracked(
        &f.session.platform.registrations,
        &f.appdb(),
        &f.session.leases,
        f.wallet,
        tapped(lock, Some(index)),
    )
    .unwrap();
    assert_ne!(moved, Some(draft));
    let report = f
        .engine
        .block_on(registration::finish(
            &f.session,
            f.wallet,
            &*lib,
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ))
        .unwrap();
    assert!(report.still_pending > 0, "{report:?}");
    assert_eq!(flow::outpoint_of(&f.row(draft)), None);
    let recovery: Vec<_> = (f.rows().into_iter())
        .filter(|r| flow::outpoint_of(r) == Some(lock))
        .collect();
    assert_eq!(recovery.len(), 1, "{report:?}");
    assert_ne!(recovery[0].id, draft);
    assert_eq!(lib.counts(), (1, 1), "funded once; the lock's one identity");
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::FundingUnknown)
    );
    assert_eq!(
        f.discard(&lib, draft).unwrap_err().code(),
        "invalid_argument"
    );
    let started = f.engine.block_on(registration::start(
        &f.session,
        f.wallet,
        Arc::clone(&lib),
        request("erin2"),
        f.grant(COIN, 10 * COIN * 1000, Credential::None),
    ));
    assert!(
        matches!(started, Err(RegistrationError::InProgress { draft: ref d }) if *d == draft.to_string()),
        "{started:?}"
    );
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
    // A run on an unlocked vault holds no key of its own, yet runs (DP1-09
    // D26): `holds_key` never tells a running row from an idle one.
    assert!(status.running && !status.holds_key);
    let discard = f.discard(&lib, draft);
    assert_eq!(discard.unwrap_err().code(), "invalid_argument");
    assert!(f.running(draft), "the refused discard stopped the flow");
    lib.w().proof_arrives = true;
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (0, 1));
}

/// P2b's resume gate (E0-04 §2a.7): the proof wait never broadcasts again
/// a `Built` lock whose signature a funding call of this process released
/// before its record is durable. Once the record lands, it does.
#[test]
fn the_proof_wait_resends_no_lock_before_its_record_is_durable() {
    let f = fixture(false);
    let lib = f.fake();
    let draft = adopted_lock_row(&f, &lib, 1, LockStatus::Built);
    let lock = lib.w().locks[0].outpoint;
    let other = prepared(&f, 2);
    let key = super::lease::DraftKey::registration(other, 2).key;
    let call = f
        .engine
        .block_on(lib.funding_permit(&f.lease(), flow::funding_step(other), key, flow::LOCK_DUFFS))
        .unwrap();
    // As `Fake::fund` signs: the lock's sighash is its txid's bytes.
    let sighash = lock.txid.to_byte_array();
    call.note_signed(sighash);
    f.drive(&lib, draft, None, false);
    assert!(lib.w().waits.is_empty(), "sent again before its record");
    f.session.leases.record_signed_locks(
        f.wallet,
        &[StoredLock {
            txid: lock.txid,
            inputs: vec![OutPoint::new(Txid::from_byte_array([0x77; 32]), 1)],
            sighashes: Some(vec![sighash]),
            built: true,
        }],
    );
    f.drive(&lib, draft, None, false);
    assert_eq!(lib.w().waits, vec![flow::REBROADCAST_WAIT]);
    call.finish(false);
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

/// The bring-up as a slow DAPI node or a loaded machine leaves it
/// (dw-tdash B2): the first ends `PartialNoIdentity`; the next, once
/// `rerun` lets it, proves the seed has no identity.
#[derive(Default)]
struct PartialBringUp {
    calls: AtomicU32,
    rerun: Arc<tokio::sync::Notify>,
}

impl MockPlatform for PartialBringUp {
    fn bring_up(
        &self,
        _: Arc<Manager>,
        _: WalletId,
    ) -> BoxedFuture<Result<platform_wallet::manager::startup::WalletStartupOutcome, EngineError>>
    {
        use platform_wallet::manager::startup::{WalletStartupOutcome, WalletStartupStatus};
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let rerun = (!first).then(|| Arc::clone(&self.rerun));
        Box::pin(async move {
            if let Some(rerun) = rerun {
                rerun.notified().await;
            }
            Ok(WalletStartupOutcome {
                status: if first {
                    WalletStartupStatus::PartialNoIdentity
                } else {
                    WalletStartupStatus::NoIdentity
                },
                identity_id: None,
                discovery_attempts: 1,
                dashpay_sync_ran: !first,
                seed_binding_unverified: false,
                identity_scan_incomplete: false,
                contact_accounts_drained: 0,
                contact_accounts_pending: 0,
                elapsed: Duration::ZERO,
            })
        })
    }

    fn names_pass(&self, _: Arc<Manager>, _: WalletId) -> BoxedFuture<()> {
        Box::pin(async {})
    }

    fn discover(&self, _: Arc<Manager>, _: WalletId) -> BoxedFuture<Result<usize, PlatformError>> {
        Box::pin(async { Ok(0) })
    }
}

/// dw-tdash B2: a bring-up that left the seed's identity indexes unanswered
/// holds the funding at `Startup`, not `Network`. The run waits with its
/// lease instead of parking, has the bring-up run again, and registers
/// once that answers, without a restart.
#[test]
fn a_run_held_on_a_partial_bring_up_has_it_run_again_and_registers() {
    let f = fixture(false);
    let platform = Arc::new(PartialBringUp::default());
    *guard(&f.session.platform.recovery.mock) = Some(Arc::clone(&platform) as _);
    f.engine.block_on(f.session.start_spv()).unwrap();
    eventually("SPV", || {
        f.session.spv_state().unwrap() == SpvState::Running
    });
    let startup = || f.session.dashpay_startup(&f.wallet).unwrap().startup;
    eventually("the partial bring-up", || {
        startup() == StartupStatus::PartialNoIdentity
    });
    let lib = f.fake();
    lib.startup_hold.store(true, Ordering::SeqCst);
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
    eventually("the rerun the run asked for", || {
        platform.calls.load(Ordering::SeqCst) == 2
    });
    assert_eq!(
        f.status(&lib, draft).waiting,
        Some(RegistrationWait::Startup)
    );
    assert!(f.running(draft), "it waits with its lease");
    assert_eq!(lib.counts(), (0, 0));

    platform.rerun.notify_one();
    f.settle(draft);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(startup(), StartupStatus::NoIdentity);
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(platform.calls.load(Ordering::SeqCst), 2);
    f.engine.block_on(f.engine.shutdown()).unwrap();
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
    let dw_appdb::Adoption::Inserted(holder) = f
        .appdb()
        .adopt_registration_lock(&w, &flow::canonical(lock), &held, 1)
        .unwrap()
    else {
        panic!("a new row for the lock");
    };
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
    let signer = lease.uncounted_funding_signer().unwrap();
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
        let (flows, appdb, leases, wallet) = (
            Arc::clone(&flows),
            f.appdb(),
            Arc::clone(&f.session.leases),
            f.wallet,
        );
        std::thread::spawn(move || {
            flow::lock_tracked(&flows, &appdb, &leases, wallet, tapped(lock, Some(7)))
        })
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
/// signed by `permit`'s funding signer, tracks it as built in the wallet's
/// state and reports it through the session's tap (P2b's provenance check,
/// then the record), as the library does; returns the lock and its inputs.
async fn build_and_track(
    f: &Fixture,
    wallet: &platform_wallet::PlatformWallet,
    index: u32,
    permit: &CallPermit,
) -> (OutPoint, Vec<OutPoint>) {
    let signer = permit.funding_signer().unwrap();
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
    let classified = lock_changes(&tracked);
    wallet
        .state_mut()
        .await
        .tracked_asset_locks
        .insert(lock, tracked);
    let tap = &f.session.live().unwrap().tap;
    tap.note_signed(f.wallet, &classified);
    tap.record(f.wallet, tap.epoch(f.wallet), classified, &NoTimes);
    (lock, inputs)
}

/// The tap's classification of a changeset that stores `tracked`.
fn lock_changes(tracked: &platform_wallet::TrackedAssetLock) -> super::journal::Classified {
    let entry = platform_wallet::changeset::AssetLockEntry::from(tracked);
    let changes = platform_wallet::changeset::PlatformWalletChangeSet {
        asset_locks: Some(platform_wallet::changeset::AssetLockChangeSet {
            asset_locks: [(tracked.out_point, entry)].into(),
            removed: Default::default(),
        }),
        ..Default::default()
    };
    super::journal::classify(&changes).unwrap()
}

/// A `KeysPrepared` registration row at `index`, whose funding step P2b's
/// permit names (its draft key).
fn prepared(f: &Fixture, index: u32) -> i64 {
    let mut row = core_row(PLAIN);
    row.identity_index = Some(index);
    row.phase = Phase::KeysPrepared.as_str().into();
    f.insert(row)
}

/// The funding permit of `draft` at `index` that the funding step takes
/// through `lib` (`PwLib`): the coin check, and the fence it keeps.
async fn permit(
    lib: &flow::PwLib,
    lease: &Lease,
    draft: i64,
    index: u32,
) -> Result<CallPermit, FundError> {
    let key = super::lease::DraftKey::registration(draft, index).key;
    lib.funding_permit(lease, flow::funding_step(draft), key, flow::LOCK_DUFFS)
        .await
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
    let draft = prepared(&f, 1);
    let locked = Arc::new(AtomicBool::new(false));
    let (inputs, locked_inside, locked_after_tracking) = f.engine.block_on(async {
        let locker = {
            let (session, locked, w) = (Arc::clone(&f.session), Arc::clone(&locked), f.wallet);
            async move {
                session.lock_outpoints(w, vec![coin]).await.unwrap();
                locked.store(true, Ordering::SeqCst);
            }
        };
        let call = permit(&lib, &lease, draft, 1).await.unwrap();
        // "Lock unspent" after the check, while the coins are selected and
        // signed.
        let locking = tokio::spawn(locker);
        // The locker waits for the fence the call's permit holds.
        until(|| f.session.coin_fences.users(f.wallet) >= 2).await;
        assert_eq!(f.session.coin_fences.users(f.wallet), 2);
        tokio::time::sleep(Duration::from_millis(100)).await;
        let inside = locked.load(Ordering::SeqCst);
        let (_, inputs) = build_and_track(&f, &wallet, 1, &call).await;
        // The broadcast and the proof wait run unfenced.
        until(|| locked.load(Ordering::SeqCst)).await;
        let after = locked.load(Ordering::SeqCst);
        call.finish(true);
        locking.await.unwrap();
        (inputs, inside, after)
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
    let refused = f.engine.block_on(lib.fence()).map(drop);
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
    let draft = prepared(&f, 1);
    let (lock, inputs) = f.engine.block_on(async {
        let call = permit(&lib, &lease, draft, 1).await.unwrap();
        let built = build_and_track(&f, &wallet, 1, &call).await;
        call.finish(true);
        built
    });
    assert!(inputs.contains(&coin));
    assert!(reads_reserved(&f, coin));
    assert!(f.session.spends.snapshot(&f.wallet).is_empty());
    assert_eq!(f.engine.block_on(lib.fence()).map(drop), Ok(()));
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
    lock: OutPoint,
    coin: OutPoint,
) {
    f.engine.block_on(async {
        // As the library tracks the lock it built, before the tap.
        wallet
            .state_mut()
            .await
            .tracked_asset_locks
            .insert(lock, built_lock(lock, coin));
        let flows = &f.session.platform.registrations;
        flow::lock_tracked(
            flows,
            &f.appdb(),
            &f.session.leases,
            f.wallet,
            tapped(lock, Some(1)),
        )
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
    let classified = lock_changes(&tracked);
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
    let tap = f.session.live().unwrap().tap;
    tap.record(f.wallet, tap.epoch(f.wallet), classified, &NoTimes);
    reader
}

/// Sol r5 N1, the whole path on P2b's permit: a lock the funding call
/// signed, reported through the session's tap (classify, record, P2b's
/// `note_tracked`, `lock_tracked`), releases the call's fence while the call
/// still runs, so a queued coin reader completes and reads the lock's input
/// reserved by the library, not by a stored claim; the row is claimed and
/// funding-sent with the lock.
#[test]
fn a_tapped_built_lock_frees_the_fence_before_its_call_returns() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lease = f.lease();
    let draft = prepared(&f, 0);
    let claim = f.claim(draft);
    let lock = OutPoint::new(Txid::from_byte_array([0xe3; 32]), 0);
    f.engine.block_on(async {
        let call = permit(&lib, &lease, draft, 0).await.unwrap();
        // The call's funding signer released a signature.
        call.note_signed([0xe3; 32]);
        // The persister's `note_signed` records it as the step's own
        // before the store (§2a.6): the tap gives a calling row only its
        // own lock (DP1-02 §3.4).
        f.session.leases.record_signed_locks(
            f.wallet,
            &[StoredLock {
                txid: lock.txid,
                inputs: vec![coin],
                sighashes: Some(vec![[0xe3; 32]]),
                built: true,
            }],
        );
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
        call.finish(true);
    });
}

/// Sol r5 N1, the control: a lock tapped before the call released any
/// signature is not the call's (P2b's `note_tracked`), so it does not
/// release the fence, and the queued reader waits for the call to return.
#[test]
fn an_unsigned_calls_tapped_lock_keeps_the_fence_until_it_returns() {
    let f = fixture(false);
    let (wallet, coin) = f.coin();
    let lib = f.pw_lib(&wallet);
    let lease = f.lease();
    let draft = prepared(&f, 1);
    let lock = OutPoint::new(Txid::from_byte_array([0xe5; 32]), 0);
    f.engine.block_on(async {
        let call = permit(&lib, &lease, draft, 1).await.unwrap();
        let mut reader = track_and_tap(&f, &wallet, built_lock(lock, coin)).await;
        let early = tokio::time::timeout(Duration::from_millis(200), &mut reader).await;
        assert!(early.is_err(), "the fence came down before a signature");
        call.finish(false);
        let read = tokio::time::timeout(Duration::from_secs(30), reader)
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
    track_built(&f, &wallet, lock, coin);
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
    let lock = OutPoint::new(Txid::from_byte_array([0xdb; 32]), 0);
    track_built(&f, &wallet, lock, coin);
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
    // It holds nothing: discard removes it.
    f.discard(&lib, draft).unwrap();
    assert!(f.rows().iter().all(|r| r.id != draft));
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
/// as a storage error with the row still bound to its index, unfailed. It
/// reads committed (no discard, no second registration). After a restart,
/// its next run, or "Finish transfers", gives it the lock its call signed,
/// by P2b's record (exact provenance, durable). Never a second row.
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
    for via_finish in [false, true] {
        let f = fixture(false);
        let (lib, draft, lock) = lock_record_failed(&f);
        let status = f.status(&lib, draft);
        assert!(status.funds_committed, "{via_finish}");
        let discard = f.discard(&lib, draft);
        assert_eq!(discard.unwrap_err().code(), "invalid_argument");
        let start = f.engine.block_on(registration::start(
            &f.session,
            f.wallet,
            Arc::clone(&lib),
            request("erin2"),
            f.grant(COIN, 10 * COIN * 1000, Credential::None),
        ));
        assert!(matches!(start, Err(RegistrationError::InProgress { .. })));
        // A restart: the flow's memory is gone, P2b's record is not.
        f.kill(draft);
        if via_finish {
            f.engine
                .block_on(registration::finish(
                    &f.session,
                    f.wallet,
                    &*lib,
                    f.grant(COIN, 10 * COIN * 1000, Credential::None),
                ))
                .unwrap();
        } else {
            f.drive(&lib, draft, Some(f.lease()), false);
        }
        let rows = f.rows();
        assert_eq!(rows.len(), 1, "{via_finish}: {rows:?}");
        assert_eq!(rows[0].id, draft);
        assert_eq!(flow::outpoint_of(&rows[0]), Some(lock), "{via_finish}");
        assert_eq!(lib.counts(), (1, 0), "{via_finish}: funded again");
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
}

/// A run at index 0 whose lock (untapped, `Broadcast`, its index lost)
/// could not be recorded: the run failed as storage, the row at
/// `KeysPrepared` without an outpoint. Returns the lock.
fn lock_record_failed(f: &Fixture) -> (Arc<Fake>, i64, OutPoint) {
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
    (lib, draft, lock)
}

/// The tap retries its record too; one that keeps failing is an error
/// (the funding call then attributes the lock: MN-1).
#[test]
fn the_taps_lock_record_is_retried() {
    for (faults, recorded) in [(2, true), (3, false)] {
        let f = fixture(false);
        let lib = f.fake();
        let mut row = core_row(PLAIN);
        row.identity_index = Some(1);
        row.phase = Phase::KeysPrepared.as_str().into();
        let draft = f.insert(row);
        let lock = lib.lock_at(1, LockStatus::Built);
        let flows = &f.session.platform.registrations;
        flows.fail_tap_writes(faults);
        let tracked = flow::lock_tracked(
            flows,
            &f.appdb(),
            &f.session.leases,
            f.wallet,
            tapped(lock, Some(1)),
        );
        match recorded {
            true => {
                assert_eq!(tracked.unwrap(), Some(draft));
                assert_eq!(flow::outpoint_of(&f.row(draft)), Some(lock));
            }
            false => {
                assert!(tracked.is_err());
                assert_eq!(f.row(draft).asset_lock_outpoint, None);
            }
        }
    }
}

/// N-7: one row's storage error in "Finish transfers" leaves it pending
/// and the other rows run.
#[test]
fn finish_runs_the_other_rows_past_a_failed_one() {
    let f = fixture(false);
    let lib = f.fake();
    let failing = adopted_lock_row(&f, &lib, 1, LockStatus::InstantSendLocked);
    let other = adopted_lock_row(&f, &lib, 2, LockStatus::InstantSendLocked);
    let flows = &f.session.platform.registrations;
    flows.fail_writes((f.wallet, failing), 1);
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
        (2, 1, 1),
        "{report:?}"
    );
    assert_eq!(phase(&f.row(other)), Phase::Done);
    let row = f.row(failing);
    assert_ne!(phase(&row), Phase::Done);
    assert_eq!(row.error, None);
}

/// Sol r6 R6-03: a row owns a lock whose index the library lost only by
/// its funding call's signed record (P2b's `signed_lock`), whatever the
/// lock's status: one no call of the row signed is not its own, rebuilt
/// from the chain or still on its way, and one its call signed is, a proof
/// notwithstanding (R6-01). A `Built` lock's index is known, so it is the
/// row's by its index either way.
#[test]
fn only_a_signed_lock_whose_index_is_lost_is_the_rows() {
    let f = fixture(false);
    let draft = prepared(&f, 0);
    let row = f.row(draft);
    let lock = |status| LockInfo {
        outpoint: OutPoint::new(Txid::from_byte_array([9; 32]), 0),
        registration: true,
        identity_index: 0,
        account: 0,
        amount: flow::LOCK_DUFFS,
        status,
    };
    let ledger = |status, steps| {
        crate::platform::registration_ledger::Ledger::new(
            vec![lock(status)],
            vec![],
            vec![row.clone()],
            steps,
            vec![],
        )
    };
    let step = |draft, index| crate::platform::lease::mode_b::FundingStepView {
        draft,
        key: FundingKey {
            funding_type: platform_wallet::AssetLockFundingType::IdentityRegistration,
            identity_index: index,
            account: 0,
        },
        status: Some(crate::platform::DispatchState::WillBeSent),
        signed: vec![Txid::from_byte_array([9; 32])],
        dead: vec![],
        durable_record: false,
        recording: false,
        released_at: None,
    };
    use crate::platform::registration_ledger::IndexOwner;
    for status in [
        LockStatus::Broadcast,
        LockStatus::InstantSendLocked,
        LockStatus::ChainLocked,
        LockStatus::RecoveredFromChain,
    ] {
        let unsigned = ledger(status, vec![]);
        assert_eq!(unsigned.own_lock(&row), None, "{status:?}");
        assert_ne!(
            unsigned.index_owner(&row, 0),
            IndexOwner::Lock(lock(status).outpoint),
            "{status:?}"
        );
        let signed = ledger(status, vec![step(draft, 0)]);
        assert_eq!(
            signed.own_lock(&row).map(|l| l.outpoint),
            Some(lock(status).outpoint),
            "{status:?}"
        );
        // Another row's signed lock is never this one's (its key is not
        // this row's, so the row is not its heir).
        let other = ledger(status, vec![step(draft + 1, 5)]);
        assert_eq!(other.own_lock(&row), None, "{status:?}");
    }
    let built = ledger(LockStatus::Built, vec![]);
    assert_eq!(
        built.index_owner(&row, 0),
        IndexOwner::Lock(lock(LockStatus::Built).outpoint)
    );
}

/// MN-2 on P2b: a lock rejected after the call signed it may still be out
/// (P2b review M-2), so the row keeps it, waiting for the chain, and the
/// backup taken for it still holds the funding: nothing re-arms it, and no
/// retry funds again. (A funding P2b reads `NotSent` re-arms the backup when
/// it unfunds the row; at the pin only a call that signed nothing is.)
#[test]
fn a_rejection_after_signing_keeps_the_row_and_its_backup() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::Rejected);
    lib.w().proof_arrives = false;
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    let flows = &f.session.platform.registrations;
    let state = flows.get((f.wallet, draft));
    assert_eq!((state.backups, state.backed_up), (1, true));
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::FundingSent, None)
    );
    lib.w().fund_script = Some(FundScript::Ok);
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(lib.counts(), (1, 0), "funded again");
    assert_eq!(flows.get((f.wallet, draft)).backups, 1);
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
    // The fields the row leaves out are not compared.
    let alice_hi = ProfileRecord {
        public_message: Some("hi".into()),
        ..alice.clone()
    };
    for (on_platform, updates) in [(alice.clone(), 0), (alice_hi, 0), (older, 1)] {
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

/// MJ-1: a name or profile step whose outcome is unknown, or that met the
/// network, waits rather than settles: the next try reads Platform.
#[test]
fn an_unknown_outcome_after_the_identity_does_not_settle() {
    let unknown = PlatformError::BroadcastUnknown {
        artifact: "st".into(),
    };
    for error in [PlatformError::Timeout, unknown.clone()] {
        let f = fixture(false);
        let lib = f.fake();
        lib.w().name_error = Some(NameError::Platform(error.clone()));
        let draft = f.insert(core_row(PLAIN));
        f.drive(&lib, draft, Some(f.lease()), false);
        let row = f.row(draft);
        assert_eq!(
            (phase(&row), row.error.as_deref(), row.name_error.as_deref()),
            (Phase::NameRequested, None, None),
            "{error:?}"
        );
        lib.w().name_error = None;
        f.drive(&lib, draft, Some(f.lease()), false);
        let row = f.row(draft);
        assert_eq!(
            (phase(&row), row.name_error),
            (Phase::Done, None),
            "{error:?}"
        );
        assert_eq!(lib.w().names.len(), 1);
    }
    // The profile's put: unknown, then found published.
    let f = fixture(false);
    let lib = f.fake();
    lib.w().profile_error = Some(unknown);
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), false);
    assert_eq!(phase(&f.row(draft)), Phase::NameRegistered);
    lib.w().profile_error = None;
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!((phase(&row), row.profile_error), (Phase::Done, None));
}

/// MN-3: a put Platform refuses because an earlier one landed (a duplicate
/// of the same profile) is published.
#[test]
fn a_refused_put_whose_profile_landed_is_published() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().profile_error = Some(PlatformError::Internal {
        detail: "duplicate unique index $ownerId".into(),
    });
    lib.w().profile_lands = true;
    let draft = f.insert(core_row(PLAIN));
    f.drive(&lib, draft, Some(f.lease()), true);
    let row = f.row(draft);
    assert_eq!(
        (
            phase(&row),
            row.error.as_deref(),
            row.profile_error.as_deref()
        ),
        (Phase::Done, None, None)
    );
    assert_eq!(lib.w().profiles.len(), 1);
}

/// MN-1, OD-05 (DP1-02 rev2 §5.1): "Finish transfers" gives the lock a
/// running funding call signed (untapped) to that call's row, calling or
/// not, never to `Recovery`; the row's run after a restart goes on with it.
/// Never a second row.
#[test]
fn finish_gives_a_running_calls_lock_to_its_row() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    lib.w().skip_tap = true;
    lib.hang_at(Call::Fund, When::After);
    let draft = f.insert(core_row(PLAIN));
    f.spawn(&lib, draft, Some(f.lease()));
    eventually("the funding call is out", || {
        lib.hung.load(Ordering::SeqCst)
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
    let lock = lib.w().locks[0].outpoint;
    // The import leaves nothing pending; the row's run is held by its call.
    assert_eq!((report.resumed, report.still_pending), (0, 1), "{report:?}");
    assert_eq!(f.rows().len(), 1, "Finish made a row for the call's lock");
    assert_eq!(flow::outpoint_of(&f.row(draft)), Some(lock));
    // The process dies in the call: the row's run goes on with its lock.
    f.kill(draft);
    lib.w().hang = None;
    f.drive(&lib, draft, Some(f.lease()), true);
    let rows = f.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(flow::outpoint_of(&rows[0]), Some(lock));
    assert_eq!(phase(&rows[0]), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
}

/// A lock whose index is lost and that no funding call of the row signed
/// (one the library rebuilt, another device's on the seed) is not adopted
/// by the row's run, whatever its index and amount: only "Finish transfers"
/// gives it (ownership needs exact provenance, Sol r6 R6-03). Neither a
/// restored row with no call, nor one whose call failed before it signed.
#[test]
fn an_unsigned_lock_waits_for_finish() {
    for called in [false, true] {
        let f = fixture(false);
        let lib = f.fake();
        let draft = match called {
            false => {
                let mut row = core_row(PLAIN);
                row.identity_index = Some(0);
                row.phase = Phase::KeysPrepared.as_str().into();
                f.insert(row)
            }
            true => {
                lib.w().fund_script = Some(FundScript::FailUntracked);
                let draft = f.insert(core_row(PLAIN));
                f.drive(&lib, draft, Some(f.lease()), false);
                assert!(f.row(draft).retryable);
                draft
            }
        };
        let lock = lib.lock_at(0, LockStatus::Broadcast);
        lib.w().fund_script = Some(FundScript::Ok);
        resume(
            &f,
            &lib,
            draft,
            Some(f.grant(COIN, 10 * COIN * 1000, Credential::None)),
        );
        // Its run waits at ChainLock, in its task, taking nothing.
        eventually("the run waits for the lock's owner", || {
            f.status(&lib, draft).waiting == Some(RegistrationWait::ChainLock)
        });
        f.kill(draft);
        let row = f.row(draft);
        assert_eq!(row.asset_lock_outpoint, None, "called {called}");
        assert_eq!(phase(&row), Phase::KeysPrepared, "called {called}");
        let funds = if called { 1 } else { 0 };
        assert_eq!(lib.counts(), (funds, 0), "called {called}: funded again");
        f.engine
            .block_on(registration::finish(
                &f.session,
                f.wallet,
                &*lib,
                f.grant(COIN, 10 * COIN * 1000, Credential::None),
            ))
            .unwrap();
        assert_eq!(f.rows().len(), 1, "called {called}");
        assert_eq!(
            flow::outpoint_of(&f.row(draft)),
            Some(lock),
            "called {called}"
        );
    }
}

/// P2b's gate refuses a funding build while a tracked `Built` lock the
/// catch-up has not resent stands (E0-04 §2a.3, `FundingBlock::Row`), and
/// names that lock, not the step: the row waits with nothing signed and no
/// failure (its step is no `broadcast_unknown`), and funds once the
/// catch-up has resent the lock.
#[test]
fn another_lock_awaiting_its_resend_holds_the_funding_without_failing_the_row() {
    let f = fixture(false);
    let lib = f.fake();
    let wallet = f.engine.block_on(f.session.wallet(&f.wallet)).unwrap();
    let pending = f.engine.block_on(async {
        let transaction = dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input: Vec::new(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let lock = OutPoint::new(transaction.txid(), 0);
        wallet.state_mut().await.tracked_asset_locks.insert(
            lock,
            platform_wallet::TrackedAssetLock {
                out_point: lock,
                transaction,
                account_index: 0,
                funding_type: platform_wallet::AssetLockFundingType::IdentityTopUp,
                identity_index: 40,
                amount: flow::LOCK_DUFFS,
                status: platform_wallet::AssetLockStatus::Built,
                proof: None,
            },
        );
        lock
    });
    let draft = prepared(&f, 1);
    f.drive(&lib, draft, Some(f.lease()), false);
    let row = f.row(draft);
    assert_eq!(
        (phase(&row), row.error.as_deref()),
        (Phase::KeysPrepared, None)
    );
    assert_eq!(lib.counts(), (0, 0));
    f.session.leases.note_resumed(f.wallet, pending.txid);
    f.drive(&lib, draft, Some(f.lease()), true);
    assert_eq!(phase(&f.row(draft)), Phase::Done);
    assert_eq!(lib.counts(), (1, 1));
}

mod ledger;
mod rules;
mod step2;
