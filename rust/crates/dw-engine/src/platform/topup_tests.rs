//! DP1-06's top-up against a real session, vault and dispatch journal,
//! with the library scripted ([`Fake`]): the quote's refusals, the step and
//! lock idempotency, a restart at each point of the flow, the vault locked
//! under it, "Finish transfers", and Platform's unproved "already
//! consumed". A [`Fake`] over storage keeps its locks in the wallet's own
//! store, so a restart reads them back from `wallet.sqlite`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, Txid};
use dpp::platform_value::string_encoding::Encoding;
use dpp::prelude::Identifier;
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, VaultConfig, VaultSigner,
};
use platform_wallet::changeset::{
    AssetLockChangeSet, AssetLockEntry, PlatformWalletChangeSet, PlatformWalletPersistence,
};
use platform_wallet::{AssetLockFundingType, AssetLockStatus, TrackedAssetLock};
use zeroize::Zeroizing;

use super::credits::{TopUpOutcome, TopUpState};
use super::errors::{CreditsError, PlatformError};
use super::flows::{DispatchState, FlowKind};
use super::lease::mode_b::{TrackedRow, TrackedStatus, index_trusted};
use super::lease::{CallPermit, FundingKey, Lease, StoredLock};
use super::registration::{FinishReport, RegistrationWait};
use super::registration_flow::{FundError, LockInfo, LockStatus, PwLib, Spendable};
use super::topup::{self, Applied, TopUpLib, top_up_step};
use crate::{DashNetwork, Engine, EngineConfig, ImportOptions, NetworkSession, WalletId};

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const PASS: &[u8] = b"pass phrase";
const COIN: u64 = 100_000_000;
/// A top-up of 0.05 DASH.
const DUFFS: u64 = 5_000_000;
/// What the fake Platform keeps of a top-up as its fee.
const PLATFORM_FEE: u64 = 1_000_000;
const START_BALANCE: u64 = 7_000;

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

fn options() -> crate::SessionOptions {
    crate::SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

pub(super) struct Fixture {
    dir: tempfile::TempDir,
    pub(super) engine: Engine,
    pub(super) session: Arc<NetworkSession>,
    pub(super) wallet: WalletId,
    encrypted: bool,
}

/// An engine with a regtest session (no network) and one wallet.
pub(super) fn fixture(encrypted: bool) -> Fixture {
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
        .block_on(engine.open_network(DashNetwork::Regtest, options()))
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
        dir,
        engine,
        session,
        wallet,
        encrypted,
    }
}

/// The wallet's identity at index 0.
fn identity() -> Identifier {
    Identifier::new([7; 32])
}

fn b58(id: Identifier) -> String {
    id.to_string(Encoding::Base58)
}

impl Fixture {
    fn grant(&self, max_duffs: u64) -> String {
        let credential = match self.encrypted {
            true => Credential::Passphrase(PASS),
            false => Credential::None,
        };
        self.session
            .vault()
            .authorize(
                GrantPurpose::PlatformOp {
                    max_duffs,
                    max_credits: 0,
                },
                Some(&self.wallet.0),
                credential,
            )
            .unwrap()
            .id
    }

    fn fake(&self) -> Arc<Fake> {
        self.fake_over(Arc::new(Mutex::new(World::new())))
    }

    /// A fake of this session over `world` (the library's state, which a
    /// restart keeps).
    fn fake_over(&self, world: Arc<Mutex<World>>) -> Arc<Fake> {
        self.new_fake(world, false)
    }

    /// A fake whose locks live in the wallet's store: a restart reads them
    /// back from `wallet.sqlite`, and `platform` (Platform's state, which
    /// no restart touches) is all the fake keeps.
    fn stored_fake(&self, platform: Option<&Fake>) -> Arc<Fake> {
        let mut world = World::new();
        if let Some(platform) = platform {
            let w = platform.w();
            (world.balances, world.consumed, world.false_taken) =
                (w.balances.clone(), w.consumed.clone(), w.false_taken);
            world.funds = w.funds;
        }
        self.new_fake(Arc::new(Mutex::new(world)), true)
    }

    fn new_fake(&self, world: Arc<Mutex<World>>, stored: bool) -> Arc<Fake> {
        let fake = Arc::new(Fake {
            session: Arc::downgrade(&self.session),
            wallet: self.wallet,
            hung: AtomicBool::new(false),
            stored,
            w: world,
        });
        fake.sync_rows();
        fake
    }

    fn quote(&self, lib: &Fake, duffs: u64) -> Result<super::credits::TopUpQuote, CreditsError> {
        self.engine.block_on(topup::quote(
            &self.session,
            self.wallet,
            lib,
            &b58(identity()),
            duffs,
        ))
    }

    /// A top-up of `duffs` under a grant of its quote's total.
    fn top_up(&self, lib: &Fake, duffs: u64) -> Result<TopUpOutcome, CreditsError> {
        let grant = self.grant(duffs + self.fee_bound());
        self.top_up_with(lib, duffs, &grant)
    }

    fn top_up_with(
        &self,
        lib: &Fake,
        duffs: u64,
        grant: &str,
    ) -> Result<TopUpOutcome, CreditsError> {
        self.engine.block_on(topup::top_up(
            &self.session,
            self.wallet,
            lib,
            &b58(identity()),
            duffs,
            grant,
        ))
    }

    fn resume(&self, lib: &Fake, txid: &str) -> Result<TopUpOutcome, CreditsError> {
        let grant = self.grant(1_000);
        self.engine
            .block_on(topup::resume(&self.session, self.wallet, lib, txid, &grant))
    }

    fn pending(&self, lib: &Fake) -> Vec<super::credits::PendingTopUp> {
        self.engine
            .block_on(topup::pending(&self.session, self.wallet, lib))
            .unwrap()
    }

    fn resolve(&self, lib: &Fake, txid: &str) -> Result<(), CreditsError> {
        self.engine
            .block_on(topup::resolve(&self.session, self.wallet, lib, txid))
    }

    fn finish(&self, lib: &Fake) -> FinishReport {
        let grant = self.grant(1_000);
        let lease = self
            .engine
            .block_on(
                self.session
                    .begin_lease(self.wallet, FlowKind::TopUp, &[grant]),
            )
            .unwrap();
        let mut report = FinishReport {
            resumed: 0,
            completed: 0,
            still_pending: 0,
        };
        self.engine.block_on(topup::finish(
            &self.session,
            self.wallet,
            lib,
            &lease,
            &mut report,
        ));
        report
    }

    fn fee_bound(&self) -> u64 {
        self.engine
            .block_on(self.session.fee_bound_worst(self.wallet))
            .unwrap()
    }

    /// Funding step `step`'s status on freshly read rows.
    fn status(&self, step: &str) -> Option<DispatchState> {
        assert!(
            self.engine
                .block_on(self.session.leases.refresh_funding(self.wallet))
        );
        self.session.leases.funding_status(self.wallet, step)
    }

    fn signed(&self, step: &str) -> Vec<Txid> {
        self.session.leases.signed_lock_txids(self.wallet, step)
    }

    /// The process dies while `lib` hangs at `at` inside a top-up: the
    /// dispatch journal is the one the crash left, the library's state
    /// stays, and the session opens again. Returns the new session's
    /// library.
    fn crash_at(&mut self, lib: Arc<Fake>, at: (Call, When)) -> Arc<Fake> {
        lib.w().hang = Some(at);
        let grant = self.grant(DUFFS + self.fee_bound());
        let (session, wallet, run) = (Arc::clone(&self.session), self.wallet, Arc::clone(&lib));
        let task = self.session.rt.spawn(async move {
            topup::top_up(&session, wallet, &*run, &b58(identity()), DUFFS, &grant).await
        });
        eventually("the top-up reaches the hang", || {
            lib.hung.load(Ordering::SeqCst)
        });
        let crashed = self.dir.path().join("crashed-dispatch.sqlite");
        let _ = std::fs::remove_file(&crashed);
        rusqlite::Connection::open(self.session.data_dir().join("dispatch.sqlite"))
            .unwrap()
            .execute("VACUUM INTO ?1", [crashed.to_str().unwrap()])
            .unwrap();
        task.abort();
        eventually("the killed top-up ended", || task.is_finished());
        self.reopen(Some(&crashed));
        let world = Arc::clone(&lib.w);
        world.lock().unwrap().hang = None;
        self.fake_over(world)
    }

    /// Removes the wallet (a wipe grant) and imports it again: the same
    /// wallet id, its `wallet.sqlite` and app rows new, its dispatch
    /// journal rows as the removal kept them.
    fn remove_and_import_again(&self) {
        let (vault, wallet) = (self.session.vault(), self.wallet);
        let wipe = (vault.authorize(GrantPurpose::Wipe, Some(&wallet.0), Credential::None))
            .unwrap()
            .id;
        (self.engine)
            .block_on(self.session.remove_wallet(wallet, wipe))
            .unwrap();
        let again = (self.engine)
            .block_on(self.session.import_wallet(
                Zeroizing::new(PHRASE.as_bytes().to_vec()),
                Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    core_compat: false,
                    ..Default::default()
                },
            ))
            .unwrap();
        assert_eq!(again, wallet);
        assert!(
            self.engine
                .block_on(self.session.leases.refresh_funding(wallet))
        );
    }

    /// A restart over a dispatch journal that cannot be opened: the
    /// session runs with it unavailable (E0-04 §6.4).
    fn reopen_without_journal(&mut self) {
        self.engine
            .block_on(self.engine.close_network(DashNetwork::Regtest))
            .unwrap();
        let path = self.session.data_dir().join("dispatch.sqlite");
        for suffix in ["-wal", "-shm"] {
            let mut side = path.clone().into_os_string();
            side.push(suffix);
            let _ = std::fs::remove_file(side);
        }
        std::fs::write(&path, b"not a journal").unwrap();
        self.session = self
            .engine
            .block_on(self.engine.open_network(DashNetwork::Regtest, options()))
            .unwrap();
        assert!(!self.session.leases.journal_ready());
    }

    /// A restart: the session closes and opens again over `journal` (the
    /// dispatch journal as a crash left it).
    fn reopen(&mut self, journal: Option<&std::path::Path>) {
        self.engine
            .block_on(self.engine.close_network(DashNetwork::Regtest))
            .unwrap();
        if let Some(journal) = journal {
            let path = self.session.data_dir().join("dispatch.sqlite");
            for suffix in ["-wal", "-shm"] {
                let mut side = path.clone().into_os_string();
                side.push(suffix);
                let _ = std::fs::remove_file(side);
            }
            std::fs::copy(journal, &path).unwrap();
        }
        self.session = self
            .engine
            .block_on(self.engine.open_network(DashNetwork::Regtest, options()))
            .unwrap();
        assert!(
            self.engine
                .block_on(self.session.leases.refresh_funding(self.wallet))
        );
    }
}

pub(super) fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ok() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A library call of the top-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    /// The funding permit, which writes the step's marker.
    Permit,
    Fund,
    Apply,
}

/// Where a scripted call hangs, as a kill -9 would leave it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum When {
    /// Before the library acted (`Permit`: before the step's marker;
    /// `Fund`: before its signature).
    Before,
    /// `Fund` only: the lock is signed, recorded and tracked (`Built`),
    /// not yet broadcast.
    Tracked,
    /// `Apply` only: Platform took the lock; the library has not marked it
    /// consumed.
    After,
}

/// How the scripted funding call ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FundScript {
    /// Locked by InstantSend in time.
    Ok,
    /// Broadcast; no InstantSend lock in the window.
    IsTimeout,
    /// Failed before any signature (coin selection).
    NotSent,
    /// Signed, then lost untracked with an unknown outcome.
    LostUntracked,
}

#[derive(Default)]
struct World {
    locks: Vec<LockInfo>,
    identities: Vec<(u32, String)>,
    spendable: Spendable,
    /// Platform's balances.
    balances: BTreeMap<Identifier, u64>,
    /// The locks Platform took.
    consumed: BTreeSet<OutPoint>,
    funds: u32,
    applies: u32,
    fund_script: Option<FundScript>,
    /// The next apply fails with this (once).
    apply_error: Option<PlatformError>,
    /// The funding call locks the vault once its lock is tracked.
    lock_vault_in_fund: bool,
    hang: Option<(Call, When)>,
    /// The proof `wait_proof` delivers (`None`: it times out).
    proof_arrives: bool,
    unsynced: bool,
    /// The library cannot prove a lock Platform reports taken final (no
    /// ChainLock): it stays as it was, not `RecoveredFromChain`.
    taken_unproved: bool,
    /// Platform answers "already consumed" for every lock, taken or not
    /// (a lying node: the answer is unproved).
    false_taken: bool,
    /// Right after the next listing of the locks, another apply takes
    /// every proved one (a concurrent resume).
    applied_after_list: bool,
}

impl World {
    fn new() -> Self {
        World {
            spendable: Spendable {
                duffs: COIN,
                inputs: 2,
                protected: 0,
            },
            identities: vec![(0, b58(identity()))],
            balances: [(identity(), START_BALANCE)].into(),
            proof_arrives: true,
            ..Default::default()
        }
    }
}

/// platform-wallet and Platform as a top-up sees them, scripted.
struct Fake {
    session: Weak<NetworkSession>,
    wallet: WalletId,
    hung: AtomicBool,
    /// The locks are the wallet's own, in its store, not `World::locks`.
    stored: bool,
    w: Arc<Mutex<World>>,
}

impl Fake {
    fn w(&self) -> MutexGuard<'_, World> {
        self.w.lock().unwrap()
    }

    fn session(&self) -> Arc<NetworkSession> {
        self.session.upgrade().expect("the fixture's session")
    }

    async fn maybe_hang(&self, call: Call, when: When) {
        if self.w().hang == Some((call, when)) {
            self.hung.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
    }

    async fn set_status(&self, outpoint: OutPoint, status: LockStatus) {
        if self.stored {
            let session = self.session();
            let w = session.wallet(&self.wallet).await.unwrap();
            let lock = (w.state().await.tracked_asset_locks.get(&outpoint)).cloned();
            let mut lock = lock.expect("a stored lock");
            lock.status = asset_lock_status(status);
            self.store(lock).await;
            return;
        }
        {
            let mut w = self.w();
            if let Some(l) = w.locks.iter_mut().find(|l| l.outpoint == outpoint) {
                l.status = status;
            }
        }
        self.sync_rows();
    }

    /// Tracks `lock` as the library does: in the wallet's state and, through
    /// the session's store (and its tap), in `wallet.sqlite`.
    async fn store(&self, lock: TrackedAssetLock) {
        let (session, wallet) = (self.session(), self.wallet);
        let entry = AssetLockEntry::from(&lock);
        let outpoint = lock.out_point;
        let w = session.wallet(&wallet).await.unwrap();
        w.state_mut()
            .await
            .tracked_asset_locks
            .insert(outpoint, lock);
        let changes = PlatformWalletChangeSet {
            asset_locks: Some(AssetLockChangeSet {
                asset_locks: [(outpoint, entry)].into(),
                removed: Default::default(),
            }),
            ..Default::default()
        };
        (session.live().unwrap().store)
            .store(wallet.0, changes)
            .unwrap();
    }

    /// Hands the session's evidence source the library's rows, as the
    /// real wallet's tracked locks would be read. A stored fake's rows are
    /// the wallet's own.
    fn sync_rows(&self) {
        if self.stored {
            return;
        }
        let rows = self.w().locks.iter().map(row_of).collect();
        (self.session().leases.test_rows.lock().unwrap()).insert(self.wallet, rows);
    }

    /// The locks as the library lists them: a stored fake's through the
    /// real `PwLib`, from the wallet.
    async fn locks(&self) -> Vec<LockInfo> {
        if !self.stored {
            return self.w().locks.clone();
        }
        let session = self.session();
        let (_, lib) = PwLib::open(&session, self.wallet).await.unwrap();
        TopUpLib::top_up_locks(&lib).await
    }

    /// The library's locks (from the test's thread).
    fn rt_locks(&self) -> Vec<LockInfo> {
        self.session().rt.block_on(self.locks())
    }

    /// The library's tracked lock of `txid` (from the test's thread).
    fn tracked(&self, txid: &str) -> TrackedAssetLock {
        let outpoint = self.lock(txid).outpoint;
        let session = self.session();
        (session.rt).block_on(async {
            let w = session.wallet(&self.wallet).await.unwrap();
            w.state().await.tracked_asset_locks[&outpoint].clone()
        })
    }

    /// Tracks `locks` as a restore reads them back from the chain:
    /// `RecoveredFromChain`, stored (from the test's thread).
    fn recover(&self, locks: &[TrackedAssetLock]) {
        for lock in locks {
            let mut lock = lock.clone();
            lock.status = AssetLockStatus::RecoveredFromChain;
            self.session().rt.block_on(self.store(lock));
        }
    }

    /// The lock of `txid` (from the test's thread).
    fn lock(&self, txid: &str) -> LockInfo {
        (self.rt_locks().into_iter())
            .find(|l| l.outpoint.txid.to_string() == txid)
            .unwrap()
    }

    /// [`Self::set_status`] from the test's thread.
    fn mark(&self, txid: &str, status: LockStatus) {
        let outpoint = self.lock(txid).outpoint;
        self.session()
            .rt
            .block_on(self.set_status(outpoint, status));
    }

    fn balance(&self) -> u64 {
        self.w().balances[&identity()]
    }

    fn counts(&self) -> (u32, u32) {
        let w = self.w();
        (w.funds, w.applies)
    }
}

/// Lock `lock` as P2b reads its tracked row: as `fund` signs it, its
/// sighash the txid's bytes.
fn row_of(lock: &LockInfo) -> TrackedRow {
    let key = FundingKey {
        funding_type: AssetLockFundingType::IdentityTopUp,
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
    let txid = lock.outpoint.txid;
    TrackedRow {
        txid,
        key,
        status,
        inputs: vec![input_of(txid)],
        trusted: index_trusted(&key, status),
        sighashes: status.pending().then(|| vec![txid.to_byte_array()]),
    }
}

fn asset_lock_status(status: LockStatus) -> AssetLockStatus {
    match status {
        LockStatus::Built => AssetLockStatus::Built,
        LockStatus::Broadcast => AssetLockStatus::Broadcast,
        LockStatus::InstantSendLocked => AssetLockStatus::InstantSendLocked,
        LockStatus::ChainLocked => AssetLockStatus::ChainLocked,
        LockStatus::Consumed => AssetLockStatus::Consumed,
        LockStatus::RecoveredFromChain => AssetLockStatus::RecoveredFromChain,
    }
}

/// The coin the lock `txid` spends.
fn input_of(txid: Txid) -> OutPoint {
    OutPoint::new(
        Txid::from_byte_array([0x77; 32]),
        u32::from(txid.to_byte_array()[0]),
    )
}

/// What the fake Platform adds for a lock of `duffs`.
fn credited(duffs: u64) -> u64 {
    duffs * 1000 - PLATFORM_FEE
}

impl TopUpLib for Fake {
    async fn identities(&self) -> Vec<(u32, String)> {
        self.w().identities.clone()
    }

    async fn top_up_locks(&self) -> Vec<LockInfo> {
        let locks = self.locks().await;
        if std::mem::take(&mut self.w().applied_after_list) {
            for lock in locks.iter().filter(|l| l.status.has_proof()) {
                {
                    let mut w = self.w();
                    w.consumed.insert(lock.outpoint);
                    *w.balances.entry(identity()).or_default() += credited(lock.amount);
                }
                self.set_status(lock.outpoint, LockStatus::Consumed).await;
            }
        }
        locks
    }

    async fn spendable(&self) -> Result<Spendable, PlatformError> {
        Ok(self.w().spendable)
    }

    async fn fee_bound(&self) -> Result<u64, PlatformError> {
        self.session().fee_bound_worst(self.wallet).await
    }

    fn funding_hold(&self) -> Option<RegistrationWait> {
        self.w().unsynced.then_some(RegistrationWait::Sync)
    }

    async fn funding_permit(
        &self,
        lease: &Lease,
        step: String,
        key: FundingKey,
        duffs: u64,
    ) -> Result<CallPermit, FundError> {
        self.maybe_hang(Call::Permit, When::Before).await;
        let session = self.session();
        let fence = session
            .coin_fence(self.wallet)
            .await
            .map_err(|e| FundError::Refused(e.into()))?;
        session
            .funding_call_permit(lease, step.clone(), duffs, step, key, fence)
            .await
            .map_err(FundError::Refused)
    }

    async fn fund(
        &self,
        duffs: u64,
        index: u32,
        permit: &CallPermit,
    ) -> Result<OutPoint, FundError> {
        permit
            .funding_signer()
            .map_err(|e| FundError::Refused(e.into()))?;
        self.maybe_hang(Call::Fund, When::Before).await;
        let script = self.w().fund_script.take().unwrap_or(FundScript::Ok);
        let n = {
            let mut w = self.w();
            w.funds += 1;
            w.funds
        };
        match script {
            FundScript::NotSent => {
                return Err(FundError::NotSent(PlatformError::Internal {
                    detail: "no coins".into(),
                }));
            }
            FundScript::LostUntracked => {
                permit.note_signed([0xfe; 32]);
                return Err(FundError::Failed(PlatformError::Unavailable));
            }
            _ => {}
        }
        // Signed by the permit's funding signer, recorded as the step's own
        // (P2b's provenance check), then tracked as Built. A stored lock is
        // a transaction of its own, tracked by the wallet.
        let transaction = dashcore::Transaction {
            version: 3,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0x66; 32]), n),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let outpoint = match self.stored {
            true => OutPoint::new(transaction.txid(), 0),
            false => OutPoint::new(Txid::from_byte_array([n as u8; 32]), 0),
        };
        let inputs = match self.stored {
            true => vec![transaction.input[0].previous_output],
            false => vec![input_of(outpoint.txid)],
        };
        let sighash = outpoint.txid.to_byte_array();
        permit.note_signed(sighash);
        self.session().leases.record_signed_locks(
            self.wallet,
            &[StoredLock {
                txid: outpoint.txid,
                inputs,
                sighashes: Some(vec![sighash]),
                built: true,
            }],
        );
        if self.stored {
            // In the wallet's state only: the session's store refuses an
            // unsigned `Built` lock while a funding call signs (fail
            // closed). It is stored from its next status on.
            let lock = TrackedAssetLock {
                out_point: outpoint,
                transaction,
                account_index: 0,
                funding_type: AssetLockFundingType::IdentityTopUp,
                identity_index: index,
                amount: duffs,
                status: AssetLockStatus::Built,
                proof: None,
            };
            let w = self.session().wallet(&self.wallet).await.unwrap();
            w.state_mut()
                .await
                .tracked_asset_locks
                .insert(outpoint, lock);
        } else {
            self.w().locks.push(LockInfo {
                outpoint,
                registration: false,
                identity_index: index,
                account: 0,
                amount: duffs,
                status: LockStatus::Built,
            });
            self.sync_rows();
        }
        self.maybe_hang(Call::Fund, When::Tracked).await;
        if self.w().lock_vault_in_fund {
            self.session().lock_vault_sync().unwrap();
        }
        match script {
            FundScript::Ok => {
                (self.set_status(outpoint, LockStatus::InstantSendLocked)).await;
                Ok(outpoint)
            }
            FundScript::IsTimeout => {
                self.set_status(outpoint, LockStatus::Broadcast).await;
                Err(FundError::IsTimeout)
            }
            FundScript::NotSent | FundScript::LostUntracked => unreachable!(),
        }
    }

    async fn wait_proof(&self, outpoint: OutPoint, _: Duration) -> Result<(), FundError> {
        if !self.w().proof_arrives {
            return Err(FundError::IsTimeout);
        }
        self.set_status(outpoint, LockStatus::ChainLocked).await;
        Ok(())
    }

    async fn tracked_row(&self, outpoint: OutPoint) -> Option<TrackedRow> {
        if self.stored {
            let session = self.session();
            let w = session.wallet(&self.wallet).await.unwrap();
            let state = w.state().await;
            return state.tracked_asset_locks.get(&outpoint).map(TrackedRow::of);
        }
        (self.w().locks.iter())
            .find(|l| l.outpoint == outpoint)
            .map(row_of)
    }

    async fn refresh_balance(&self, identity: Identifier) -> Result<Option<u64>, PlatformError> {
        Ok(self.w().balances.get(&identity).copied())
    }

    async fn apply(
        &self,
        identity: Identifier,
        outpoint: OutPoint,
        _: &VaultSigner,
    ) -> Result<Applied, PlatformError> {
        self.maybe_hang(Call::Apply, When::Before).await;
        {
            let mut w = self.w();
            w.applies += 1;
            if let Some(e) = w.apply_error.take() {
                return Err(e);
            }
        }
        let lock = (self.locks().await.into_iter()).find(|l| l.outpoint == outpoint);
        let lock = lock.expect("a tracked lock");
        assert!(lock.status.has_proof(), "applied without a proof");
        let taken = {
            let w = self.w();
            w.consumed.contains(&outpoint) || w.false_taken
        };
        if taken {
            // The library keeps it "consumption unknown" with its ChainLock
            // proof, when it can get one.
            if !self.w().taken_unproved {
                (self.set_status(outpoint, LockStatus::RecoveredFromChain)).await;
            }
            return Ok(Applied::Taken);
        }
        let balance = {
            let mut w = self.w();
            w.consumed.insert(outpoint);
            let balance = w.balances.entry(identity).or_default();
            *balance += credited(lock.amount);
            *balance
        };
        self.maybe_hang(Call::Apply, When::After).await;
        self.set_status(outpoint, LockStatus::Consumed).await;
        Ok(Applied::Balance(balance))
    }
}

fn applied(credits_added: Option<u64>, balance: u64) -> TopUpState {
    TopUpState::Applied {
        credits_added,
        balance: Some(balance),
    }
}

fn already_applied(balance: u64) -> TopUpState {
    TopUpState::AlreadyApplied {
        balance: Some(balance),
    }
}

fn consumption_unknown(balance: u64) -> TopUpState {
    TopUpState::ConsumptionUnknown {
        balance: Some(balance),
    }
}

/// `(resumed, completed, still_pending)`.
fn counted(report: FinishReport) -> (u32, u32, u32) {
    (report.resumed, report.completed, report.still_pending)
}

/// The quote prices the top-up from the network's table and the funding's
/// fee bound, and refuses what the top-up would refuse.
#[test]
fn a_quote_prices_the_top_up_and_refuses_what_cannot_be_funded() {
    let f = fixture(false);
    let lib = f.fake();
    let table = f.session.cost_table().unwrap();
    let fee = f.fee_bound();

    let quote = f.quote(&lib, DUFFS).unwrap();
    assert_eq!(quote.fee_duffs, fee);
    assert_eq!(quote.total_duffs, DUFFS + fee);
    assert_eq!(
        quote.credits,
        DUFFS * table.credits_per_duff - table.top_up_fee
    );
    assert_eq!(
        (quote.grant.max_duffs, quote.grant.max_credits),
        (DUFFS + fee, 0)
    );

    let code = |r: Result<_, CreditsError>| r.unwrap_err().code();
    assert_eq!(
        code(f.quote(&lib, table.top_up_min_duffs - 1)),
        "credits.below_minimum"
    );
    let not_ours = f.engine.block_on(topup::quote(
        &f.session,
        f.wallet,
        &*lib,
        &b58(Identifier::new([8; 32])),
        DUFFS,
    ));
    assert_eq!(code(not_ours), "identity.not_found");
    let garbled = f
        .engine
        .block_on(topup::quote(&f.session, f.wallet, &*lib, "0OIl", DUFFS));
    assert_eq!(code(garbled), "invalid_argument");
    lib.w().spendable.duffs = DUFFS + fee - 1;
    assert!(matches!(
        f.quote(&lib, DUFFS),
        Err(CreditsError::FundingInsufficient { needed, available })
            if needed == DUFFS + fee && available == DUFFS + fee - 1
    ));
    lib.w().spendable = Spendable {
        duffs: COIN,
        inputs: 2,
        protected: 1,
    };
    assert_eq!(code(f.quote(&lib, DUFFS)), "credits.coins_locked");
    // The top-up refuses alike, before it redeems its grant.
    assert_eq!(
        f.top_up(&lib, DUFFS).unwrap_err().code(),
        "credits.coins_locked"
    );
    lib.w().spendable.protected = 0;
    lib.w().unsynced = true;
    assert_eq!(
        f.top_up(&lib, DUFFS).unwrap_err().code(),
        "platform.context_unavailable"
    );
    assert_eq!(lib.counts(), (0, 0));
}

/// A top-up funds one lock under its own step and applies it: the balance
/// rises by the credits the lock bought less Platform's fee. The next
/// top-up funds under the identity's next step.
#[test]
fn a_top_up_funds_one_lock_and_applies_it() {
    let f = fixture(false);
    let lib = f.fake();
    let out = f.top_up(&lib, DUFFS).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(out.state, applied(Some(credited(DUFFS)), balance));
    assert_eq!(lib.balance(), balance);
    assert_eq!(lib.lock(&out.txid).status, LockStatus::Consumed);
    let first = top_up_step(0, 0);
    assert_eq!(f.status(&first), Some(DispatchState::Sent));
    assert_eq!(f.signed(&first), vec![lib.lock(&out.txid).outpoint.txid]);
    assert!(f.pending(&lib).is_empty());

    let again = f.top_up(&lib, DUFFS).unwrap();
    assert_ne!(again.txid, out.txid);
    assert_eq!(
        f.signed(&top_up_step(0, 1)),
        vec![lib.lock(&again.txid).outpoint.txid]
    );
    assert_eq!(lib.balance(), balance + credited(DUFFS));
    assert_eq!(lib.counts(), (2, 2));
}

/// A grant smaller than the quote's total is refused before anything is
/// funded, and before it is redeemed: a top-up it covers then funds under
/// it (review Sol r1).
#[test]
fn a_grant_below_the_quote_funds_nothing_and_stays_usable() {
    let f = fixture(false);
    let lib = f.fake();
    let grant = f.grant(DUFFS);
    let err = f.top_up_with(&lib, DUFFS, &grant).unwrap_err();
    assert_eq!(err.code(), "platform.grant_exceeded");
    assert_eq!(lib.counts(), (0, 0));
    let smaller = DUFFS - f.fee_bound();
    let out = f.top_up_with(&lib, smaller, &grant).unwrap();
    assert_eq!(
        out.state,
        applied(Some(credited(smaller)), START_BALANCE + credited(smaller))
    );
    assert_eq!(lib.counts(), (1, 1));
}

/// No InstantSend lock in the window: the lock waits for its proof, a new
/// top-up of the identity is refused for it, and resuming applies it once
/// the proof arrives.
#[test]
fn an_islock_timeout_leaves_the_top_up_pending_until_resumed() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(out.state, TopUpState::ProofWaiting);
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    assert_eq!(
        (
            pending[0].txid.as_str(),
            pending[0].identity.clone(),
            pending[0].duffs,
            pending[0].proved
        ),
        (out.txid.as_str(), Some(b58(identity())), DUFFS, false)
    );
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid }) if txid == out.txid
    ));
    assert!(matches!(
        f.quote(&lib, DUFFS),
        Err(CreditsError::TopUpPending { .. })
    ));

    lib.w().proof_arrives = false;
    let resumed = f.resume(&lib, &out.txid).unwrap();
    assert_eq!(resumed.state, TopUpState::ProofWaiting);
    lib.w().proof_arrives = true;
    let resumed = f.resume(&lib, &out.txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, applied(Some(credited(DUFFS)), balance));
    assert!(f.pending(&lib).is_empty());
    // Resumed again, it reads applied already and applies nothing.
    let again = f.resume(&lib, &out.txid).unwrap();
    assert_eq!(again.state, already_applied(balance));
    assert_eq!(lib.counts(), (1, 1));
}

/// A failed apply keeps the proved lock: it is listed, blocks a new
/// top-up, and resuming applies it.
#[test]
fn a_failed_apply_keeps_the_lock_for_resume() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().apply_error = Some(PlatformError::Unavailable);
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(
        out.state,
        TopUpState::NotApplied {
            error: "platform.unavailable".into()
        }
    );
    assert!(f.pending(&lib)[0].proved);
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { .. })
    ));
    let resumed = f.resume(&lib, &out.txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, applied(Some(credited(DUFFS)), balance));
    assert_eq!(lib.counts(), (1, 2));
}

/// A funding whose outcome is unknown (signed, nothing tracked) is never
/// funded again: its step reads `MaybeSent`, and every later top-up of the
/// identity is refused on it.
#[test]
fn an_unknown_funding_outcome_is_never_funded_again() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::LostUntracked);
    let first = top_up_step(0, 0);
    let unknown = |r: Result<TopUpOutcome, CreditsError>| match r {
        Err(CreditsError::Platform(PlatformError::BroadcastUnknown { artifact })) => artifact,
        other => panic!("not broadcast_unknown: {other:?}"),
    };
    assert_eq!(unknown(f.top_up(&lib, DUFFS)), first);
    assert_eq!(f.status(&first), Some(DispatchState::MaybeSent));
    // Refused before its grant is redeemed: the same grant is refused alike
    // again, never `platform.grant_invalid`.
    let grant = f.grant(DUFFS + f.fee_bound());
    for _ in 0..2 {
        assert_eq!(unknown(f.top_up_with(&lib, DUFFS, &grant)), first);
    }
    assert_eq!(lib.counts(), (1, 0));
}

/// A funding definitively not sent is retried under the same step.
#[test]
fn a_funding_not_sent_is_retried_under_its_step() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::NotSent);
    let err = f.top_up(&lib, DUFFS).unwrap_err();
    assert_eq!(err.code(), "internal");
    let first = top_up_step(0, 0);
    assert!(f.signed(&first).is_empty());
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(f.signed(&first), vec![lib.lock(&out.txid).outpoint.txid]);
    assert!(matches!(out.state, TopUpState::Applied { .. }));
    assert_eq!(lib.counts(), (2, 1));
}

/// Platform reports the lock taken, unproved (an earlier apply landed
/// unseen, or a node lies): nothing is concluded. The top-up reads
/// `ConsumptionUnknown` with the balance read afresh, and the lock, now
/// final with its consumption unknown, stays pending and blocks a new
/// top-up until the user resolves it (review Sol r1).
#[test]
fn an_unproved_already_consumed_keeps_the_lock_pending_until_resolved() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    let out = f.top_up(&lib, DUFFS).unwrap();
    let outpoint = lib.lock(&out.txid).outpoint;
    {
        let mut w = lib.w();
        w.consumed.insert(outpoint);
        *w.balances.get_mut(&identity()).unwrap() += credited(DUFFS);
    }
    let resumed = f.resume(&lib, &out.txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, consumption_unknown(balance));
    assert_eq!(lib.lock(&out.txid).status, LockStatus::RecoveredFromChain);
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    assert!(pending[0].proved && pending[0].consumption_unknown);
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid }) if txid == out.txid
    ));
    assert_eq!(counted(f.finish(&lib)), (1, 0, 1));

    f.resolve(&lib, &out.txid).unwrap();
    assert!(f.pending(&lib).is_empty());
    // Settled for the gates, and Finish counts Platform's answer as nothing.
    assert_eq!(counted(f.finish(&lib)), (0, 0, 0));
    assert!(matches!(
        f.top_up(&lib, DUFFS).unwrap().state,
        TopUpState::Applied { .. }
    ));
    assert_eq!(lib.balance(), balance + credited(DUFFS));
}

/// Review Sol r1's regression: a lock a top-up funded that the library
/// reads back final without consuming it (a restart lost its apply)
/// stays pending and blocks a new top-up; resuming applies it.
#[test]
fn a_funded_lock_recovered_from_the_chain_stays_pending_and_blocks() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    let out = f.top_up(&lib, DUFFS).unwrap();
    lib.mark(&out.txid, LockStatus::RecoveredFromChain);
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    assert!(pending[0].consumption_unknown);
    assert!(matches!(
        f.quote(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid }) if txid == out.txid
    ));
    let resumed = f.resume(&lib, &out.txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, applied(Some(credited(DUFFS)), balance));
    assert!(f.pending(&lib).is_empty());
    assert_eq!(lib.counts(), (1, 1));
}

/// Only a pending lock with its proof is resolved: one without a proof
/// (never applied), a settled one and an unknown txid are refused, and a
/// resolution keeps across a restart and whatever status the lock moves
/// to (review F6). A resolved lock Platform had not taken is still
/// applied by "Finish transfers".
#[test]
fn only_a_pending_proved_lock_is_resolved_and_finish_still_applies_it() {
    let mut f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    let out = f.top_up(&lib, DUFFS).unwrap();
    let refused = |r: Result<(), CreditsError>| r.unwrap_err().code();
    assert_eq!(refused(f.resolve(&lib, &out.txid)), "invalid_argument");
    assert_eq!(refused(f.resolve(&lib, "00")), "invalid_argument");
    lib.mark(&out.txid, LockStatus::RecoveredFromChain);
    f.resolve(&lib, &out.txid).unwrap();
    assert!(f.pending(&lib).is_empty());
    assert_eq!(refused(f.resolve(&lib, &out.txid)), "invalid_argument");
    // The library's live advance may overwrite the final mark.
    lib.mark(&out.txid, LockStatus::ChainLocked);
    assert!(f.pending(&lib).is_empty());

    f.reopen(None);
    let lib = f.fake_over(Arc::clone(&lib.w));
    assert!(f.pending(&lib).is_empty());
    assert_eq!(counted(f.finish(&lib)), (1, 1, 0));
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
    assert_eq!(lib.lock(&out.txid).status, LockStatus::Consumed);
    assert_eq!(counted(f.finish(&lib)), (0, 0, 0));
}

/// A recovered lock no dw top-up funded (an earlier top-up a restore read
/// back from the chain) is historical: it neither pends nor blocks, and
/// "Finish transfers" offers it to Platform and counts any answer but an
/// apply as nothing.
#[test]
fn a_historical_recovered_lock_neither_pends_nor_blocks() {
    let f = fixture(false);
    let lib = f.fake();
    let historical = OutPoint::new(Txid::from_byte_array([0xab; 32]), 0);
    {
        let mut w = lib.w();
        w.locks.push(LockInfo {
            outpoint: historical,
            registration: false,
            identity_index: 0,
            account: 0,
            amount: DUFFS,
            status: LockStatus::RecoveredFromChain,
        });
        w.consumed.insert(historical);
    }
    lib.sync_rows();
    assert!(f.pending(&lib).is_empty());
    for _ in 0..2 {
        assert_eq!(counted(f.finish(&lib)), (0, 0, 0));
    }
    // Any answer but an apply counts nothing for it (review F7).
    lib.w().apply_error = Some(PlatformError::Unavailable);
    assert_eq!(counted(f.finish(&lib)), (0, 0, 0));
    assert_eq!(lib.counts(), (0, 3));
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(out.state, TopUpState::Applied { .. }));
    assert_eq!(lib.counts(), (1, 4));
}

/// A crash before the funding permit wrote the step's marker: after the
/// restart the step stands unused, and the next top-up funds under it,
/// once.
#[test]
fn a_crash_before_the_funding_marker_funds_under_its_step() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Permit, When::Before));
    let first = top_up_step(0, 0);
    assert_eq!(f.status(&first), None);
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(out.state, TopUpState::Applied { .. }));
    assert_eq!(f.signed(&first), vec![lib.lock(&out.txid).outpoint.txid]);
    assert_eq!(lib.counts(), (1, 1));
}

/// A crash inside the funding call before its signature: after the
/// restart the step's marker stands over nothing sent, so the next top-up
/// passes it and funds under the identity's next step, once.
#[test]
fn a_crash_before_the_funding_signature_passes_its_step() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Fund, When::Before));
    assert_eq!(lib.counts(), (0, 0));
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(out.state, TopUpState::Applied { .. }));
    assert!(f.signed(&top_up_step(0, 0)).is_empty());
    assert_eq!(
        f.signed(&top_up_step(0, 1)),
        vec![lib.lock(&out.txid).outpoint.txid]
    );
    assert_eq!(lib.counts(), (1, 1));
}

/// A crash once the lock is signed and tracked: after the restart the
/// lock is pending and blocks a new top-up; resuming waits for its proof
/// and applies it. Nothing is funded twice.
#[test]
fn a_crash_after_the_lock_is_tracked_is_resumed() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Fund, When::Tracked));
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    assert!(!pending[0].proved);
    assert_eq!(
        f.status(&top_up_step(0, 0)),
        Some(DispatchState::WillBeSent)
    );
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid }) if txid == pending[0].txid
    ));
    let resumed = f.resume(&lib, &pending[0].txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, applied(Some(credited(DUFFS)), balance));
    assert_eq!(f.status(&top_up_step(0, 0)), Some(DispatchState::Sent));
    let next = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(
        f.signed(&top_up_step(0, 1)),
        vec![lib.lock(&next.txid).outpoint.txid]
    );
    assert_eq!(lib.counts(), (2, 2));
}

/// A crash before the apply: the proved lock is pending after the
/// restart, and "Finish transfers" applies it.
#[test]
fn a_crash_before_the_apply_is_finished_by_finish_transfers() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Apply, When::Before));
    let pending = f.pending(&lib);
    assert!(pending.len() == 1 && pending[0].proved);
    let report = f.finish(&lib);
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (1, 1, 0)
    );
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
    assert!(f.pending(&lib).is_empty());
    // A second pass finds nothing to do.
    let report = f.finish(&lib);
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (0, 0, 0)
    );
    assert_eq!(lib.counts(), (1, 1));
}

/// A crash once Platform took the lock, before the library marked it:
/// resuming adds the credits once and reads Platform's unproved "already
/// consumed" as `ConsumptionUnknown`; the lock pends until resolved.
#[test]
fn a_crash_after_platform_took_the_lock_adds_the_credits_once() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Apply, When::After));
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    let resumed = f.resume(&lib, &pending[0].txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, consumption_unknown(balance));
    assert_eq!(lib.balance(), balance);
    assert!(f.pending(&lib)[0].consumption_unknown);
    f.resolve(&lib, &pending[0].txid).unwrap();
    assert!(f.pending(&lib).is_empty());
    assert_eq!(lib.balance(), balance);
}

/// A crash once the library marked the lock `Consumed`, before the
/// journal recorded it applied: the next read records it, so a removal
/// (which erases the mark) does not turn it back into a pending lock
/// (review F1).
#[test]
fn a_lock_applied_without_its_record_is_recorded_on_the_next_read() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Apply, When::After));
    let txid = f.pending(&lib)[0].txid.clone();
    lib.mark(&txid, LockStatus::Consumed);
    let settled = |f: &Fixture| {
        (f.engine)
            .block_on(f.session.leases.top_ups_settled(f.wallet))
            .unwrap()
    };
    assert!(settled(&f).is_empty());
    assert!(f.pending(&lib).is_empty());
    assert_eq!(settled(&f), [txid.parse::<Txid>().unwrap()].into());
}

/// A lock Platform reports taken that the library cannot prove final stays
/// tracked as InstantSend-locked: it stays pending and blocks a new top-up
/// (no process-local note hides it), "Finish transfers" keeps it pending,
/// and so does a restart. Once the library holds it final it reads
/// `consumption_unknown` and is resolved (the user could resolve it
/// before, too: it has its proof).
#[test]
fn a_lock_taken_without_a_chainlock_stays_pending_across_a_restart() {
    let mut f = fixture(false);
    let lib = f.fake();
    let lib = f.crash_at(lib, (Call::Apply, When::After));
    lib.w().taken_unproved = true;
    let txid = f.pending(&lib)[0].txid.clone();
    lib.mark(&txid, LockStatus::InstantSendLocked);
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(
        f.resume(&lib, &txid).unwrap().state,
        consumption_unknown(balance)
    );
    assert_eq!(lib.lock(&txid).status, LockStatus::InstantSendLocked);
    let pending = f.pending(&lib);
    assert!(pending.len() == 1 && !pending[0].consumption_unknown);
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid: t }) if t == txid
    ));
    assert_eq!(counted(f.finish(&lib)), (1, 0, 1));

    f.reopen(None);
    let lib = f.fake_over(Arc::clone(&lib.w));
    assert_eq!(f.pending(&lib).len(), 1);
    assert_eq!(counted(f.finish(&lib)), (1, 0, 1));
    lib.w().taken_unproved = false;
    assert_eq!(counted(f.finish(&lib)), (1, 0, 1));
    assert!(f.pending(&lib)[0].consumption_unknown);
    f.resolve(&lib, &txid).unwrap();
    assert!(f.pending(&lib).is_empty());
    assert_eq!(lib.balance(), balance);
    assert_eq!(lib.counts(), (1, 5));
}

/// The locks live in the wallet's store: a restart reads a proved lock and
/// its identity index back from `wallet.sqlite` (nothing kept in memory),
/// and a `RecoveredFromChain` mark too; both pend and block, and "Finish
/// transfers" applies the lock once, after which the store no longer loads
/// it (review Sol r1).
#[test]
fn a_restart_reads_the_top_up_locks_back_from_wallet_storage() {
    let mut f = fixture(false);
    let lib = f.stored_fake(None);
    lib.w().apply_error = Some(PlatformError::Unavailable);
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(out.state, TopUpState::NotApplied { .. }));

    f.reopen(None);
    let lib = f.stored_fake(Some(&lib));
    let pending = f.pending(&lib);
    assert_eq!(pending.len(), 1);
    let read = &pending[0];
    assert_eq!(
        (
            read.txid.as_str(),
            read.identity_index,
            read.duffs,
            read.proved
        ),
        (out.txid.as_str(), 0, DUFFS, true)
    );
    assert!(!read.consumption_unknown);
    assert_eq!(f.status(&top_up_step(0, 0)), Some(DispatchState::Sent));
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { .. })
    ));
    lib.mark(&out.txid, LockStatus::RecoveredFromChain);

    f.reopen(None);
    let lib = f.stored_fake(Some(&lib));
    assert_eq!(lib.lock(&out.txid).status, LockStatus::RecoveredFromChain);
    assert!(f.pending(&lib)[0].consumption_unknown);
    assert!(matches!(
        f.quote(&lib, DUFFS),
        Err(CreditsError::TopUpPending { .. })
    ));
    assert_eq!(counted(f.finish(&lib)), (1, 1, 0));
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));

    // The store loads only unconsumed locks: the consumed one is gone.
    assert_eq!(lib.lock(&out.txid).status, LockStatus::Consumed);
    f.reopen(None);
    let lib = f.stored_fake(Some(&lib));
    assert!(lib.rt_locks().is_empty());
    assert!(f.pending(&lib).is_empty());
    let next = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(next.state, TopUpState::Applied { .. }));
    assert_eq!(f.signed(&top_up_step(0, 1)).len(), 1);
}

/// A node answers "already consumed" for a lock Platform never took: the
/// top-up reads `ConsumptionUnknown`, the lock (stored final, consumption
/// unknown) stays pending and blocking across a restart, and once Platform
/// answers honestly "Finish transfers" applies it, crediting it once
/// (review Sol r1).
#[test]
fn a_false_already_consumed_survives_a_restart() {
    let mut f = fixture(false);
    let lib = f.stored_fake(None);
    lib.w().false_taken = true;
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(out.state, consumption_unknown(START_BALANCE));
    assert_eq!(lib.lock(&out.txid).status, LockStatus::RecoveredFromChain);

    f.reopen(None);
    let lib = f.stored_fake(Some(&lib));
    let pending = f.pending(&lib);
    assert!(pending.len() == 1 && pending[0].consumption_unknown);
    assert!(matches!(
        f.top_up(&lib, DUFFS),
        Err(CreditsError::TopUpPending { txid }) if txid == out.txid
    ));
    assert_eq!(counted(f.finish(&lib)), (1, 0, 1));
    assert_eq!(
        f.resume(&lib, &out.txid).unwrap().state,
        consumption_unknown(START_BALANCE)
    );

    lib.w().false_taken = false;
    assert_eq!(counted(f.finish(&lib)), (1, 1, 0));
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
    assert!(f.pending(&lib).is_empty());
    assert_eq!(counted(f.finish(&lib)), (0, 0, 0));
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
}

/// "Finish transfers" reads each lock again under the identity's write
/// lock: one a concurrent resume applied meanwhile is not applied twice.
#[test]
fn finish_skips_a_lock_applied_meanwhile() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().apply_error = Some(PlatformError::Unavailable);
    f.top_up(&lib, DUFFS).unwrap();
    lib.w().applied_after_list = true;
    let report = f.finish(&lib);
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (0, 0, 0)
    );
    assert_eq!(lib.counts(), (1, 1));
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
}

/// "Finish transfers" applies every proved top-up lock and leaves one
/// without a proof pending.
#[test]
fn finish_transfers_applies_proved_top_up_locks_only() {
    let f = fixture(false);
    let lib = f.fake();
    lib.w().fund_script = Some(FundScript::IsTimeout);
    let out = f.top_up(&lib, DUFFS).unwrap();
    let report = f.finish(&lib);
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (0, 0, 1)
    );
    lib.mark(&out.txid, LockStatus::ChainLocked);
    let report = f.finish(&lib);
    assert_eq!(
        (report.resumed, report.completed, report.still_pending),
        (1, 1, 0)
    );
    assert_eq!(lib.balance(), START_BALANCE + credited(DUFFS));
}

/// The vault locked before the top-up: its grant is gone and nothing is
/// funded. Locked once the lock is out: the apply is refused, the lock
/// stays pending, and a new grant resumes it.
#[test]
fn a_vault_lock_before_or_after_the_funding_loses_nothing() {
    let f = fixture(true);
    let lib = f.fake();
    let grant = f.grant(DUFFS + f.fee_bound());
    f.session.lock_vault_sync().unwrap();
    assert!(f.top_up_with(&lib, DUFFS, &grant).is_err());
    assert_eq!(lib.counts(), (0, 0));

    lib.w().lock_vault_in_fund = true;
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert!(
        matches!(out.state, TopUpState::NotApplied { .. }),
        "{:?}",
        out.state
    );
    lib.w().lock_vault_in_fund = false;
    assert!(f.pending(&lib)[0].proved);
    let resumed = f.resume(&lib, &out.txid).unwrap();
    let balance = START_BALANCE + credited(DUFFS);
    assert_eq!(resumed.state, applied(Some(credited(DUFFS)), balance));
    assert_eq!(lib.counts(), (1, 1));
}

/// A testnet engine with the fixture's wallets `who` imported and SPV
/// started (DW_DP106_TESTNET_FIXTURE: the fixture directory). Mnemonics
/// are read from their 0600 files and never printed.
fn testnet(
    who: &[&str],
) -> (
    tempfile::TempDir,
    Engine,
    Arc<NetworkSession>,
    Vec<WalletId>,
) {
    use super::recovery_tests;
    use crate::SessionOptions;

    let fixture = std::path::PathBuf::from(
        std::env::var("DW_DP106_TESTNET_FIXTURE").expect("DW_DP106_TESTNET_FIXTURE"),
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let birth = meta["funding"]["birth_height_hint"].as_u64().unwrap() as u32;
    let dir = dw_testutil::private_tempdir();
    let engine = recovery_tests::engine(dir.path());
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let wallets = (who.iter())
        .map(|who| {
            let phrase = Zeroizing::new(
                std::fs::read_to_string(fixture.join(format!("{who}.mnemonic"))).unwrap(),
            );
            let words = Zeroizing::new(phrase.trim().as_bytes().to_vec());
            let options = ImportOptions {
                birth_height: Some(birth),
                ..ImportOptions::default()
            };
            engine
                .block_on(s.import_wallet(words, Zeroizing::new(Vec::new()), options))
                .unwrap()
        })
        .collect();
    engine.block_on(s.start_spv()).unwrap();
    (dir, engine, s, wallets)
}

/// The wallet's first identity, once the bring-up found it.
fn first_identity(dp: &super::DashPay) -> String {
    let mut found = String::new();
    super::requests::tests::wait_until("an identity", Duration::from_secs(180), || {
        let ids = dp.identities().unwrap_or_default();
        ids.first().map(|i| found = i.identity.clone()).is_some()
    });
    found
}

/// DP1-06's T2 on the public testnet, read-only, with wallets A, B and C
/// of the fixture: `refresh_balance` returns Platform's proved balance and
/// stores it (`identities()` shows it with its level), and the cost table
/// prices the network's protocol version at its fee version. Prints the
/// table, the balances and A's first receive address (where T3's tDASH
/// goes).
#[test]
#[ignore = "public testnet; set DW_DP106_TESTNET_FIXTURE"]
fn t2_testnet_credits() {
    use dash_sdk::platform::Fetch;
    use dash_sdk::query_types::IdentityBalance;

    let (_dir, engine, s, wallets) = testnet(&["A", "B", "C"]);
    let sdk = s.manager().unwrap().sdk_arc();
    for (who, wallet) in ["A", "B", "C"].iter().zip(&wallets) {
        let dp = s.dashpay(*wallet);
        let identity = first_identity(&dp);
        let refreshed = engine
            .block_on(dp.refresh_balance(identity.clone()))
            .unwrap()
            .expect("the identity is on Platform");
        let id = Identifier::from_string(&identity, Encoding::Base58).unwrap();
        let fetched = engine
            .block_on(IdentityBalance::fetch(&sdk, id))
            .unwrap()
            .unwrap();
        assert_eq!(
            refreshed, fetched,
            "{who}: the refresh is Platform's balance"
        );
        let table = dp.cost_table().unwrap();
        let summary = (dp.identities().unwrap().into_iter())
            .find(|i| i.identity == identity)
            .unwrap();
        assert_eq!(summary.balance, Some(refreshed), "{who}: stored");
        assert_eq!(
            summary.credit_level,
            Some(super::credit_costs::credit_level(refreshed, &table))
        );
        eprintln!(
            "T2 {who} {identity}: balance {refreshed} credits, level {:?}",
            summary.credit_level
        );
    }
    let table = s.dashpay(wallets[0]).cost_table().unwrap();
    let network = sdk.protocol_version_number();
    eprintln!("T2 network protocol version (proved metadata): {network}");
    eprintln!("T2 cost table: {table:?}");
    assert_eq!(table.protocol_version, network);
    assert_eq!(table, super::credit_costs::cost_table(network).unwrap());
    // DP2-02's measured testnet charges (m4 §7 item 41) are within it: a
    // contact request sent, and one sent to accept.
    let request_charges = [94_048_500, 81_967_400];
    assert!(request_charges.iter().all(|c| *c <= table.contact_request));
    assert!(61_330_100 <= table.enable_dashpay_keys);
    let address = engine
        .block_on(s.next_receive_address(wallets[0], None))
        .unwrap();
    eprintln!("T2 A receive address for T3 funding: {}", address.address);
}

/// DP1-06's T3 on the public testnet: a real top-up of A's identity from
/// A's Core balance (fund it first: `t2_testnet_credits` prints the
/// address). Waits for SPV and the coins, tops up `top_up_min_duffs`, and
/// checks the balance rose by the lock's credits less Platform's fee.
/// Prints the measured fee against the table's.
#[test]
#[ignore = "public testnet, spends tDASH; set DW_DP106_TESTNET_FIXTURE"]
fn t3_testnet_top_up() {
    let (_dir, engine, s, wallets) = testnet(&["A"]);
    let (a, dp) = (wallets[0], s.dashpay(wallets[0]));
    let identity = first_identity(&dp);
    let table = dp.cost_table().unwrap();
    let duffs = table.top_up_min_duffs;
    super::requests::tests::wait_until("A's coins", Duration::from_secs(1800), || {
        engine
            .block_on(dp.top_up_quote(identity.clone(), duffs))
            .is_ok()
    });
    let quote = engine
        .block_on(dp.top_up_quote(identity.clone(), duffs))
        .unwrap();
    let before = engine
        .block_on(dp.refresh_balance(identity.clone()))
        .unwrap()
        .unwrap();
    let grant = s
        .vault()
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: quote.grant.max_duffs,
                max_credits: 0,
            },
            Some(&a.0),
            Credential::None,
        )
        .unwrap()
        .id;
    let out = engine
        .block_on(dp.top_up(identity.clone(), duffs, grant))
        .unwrap();
    eprintln!("T3 top-up {duffs} duffs: {out:?} (quote {quote:?})");
    let out = match out.state {
        TopUpState::Applied { .. } => out,
        _ => {
            let grant = s
                .vault()
                .authorize(
                    GrantPurpose::PlatformOp {
                        max_duffs: 1,
                        max_credits: 0,
                    },
                    Some(&a.0),
                    Credential::None,
                )
                .unwrap()
                .id;
            let resumed = engine
                .block_on(dp.resume_top_up(out.txid.clone(), grant))
                .unwrap();
            eprintln!("T3 resumed: {resumed:?}");
            resumed
        }
    };
    let after = engine
        .block_on(dp.refresh_balance(identity.clone()))
        .unwrap()
        .unwrap();
    let added = after - before;
    let funded = duffs * table.credits_per_duff;
    eprintln!(
        "T3 before {before} after {after}: added {added}, funded {funded}, fee {} (table's minimum {}, quoted credits {})",
        funded - added,
        table.top_up_fee,
        quote.credits
    );
    assert!(matches!(out.state, TopUpState::Applied { .. }));
    assert!(
        added > 0 && added <= funded,
        "the balance rose by less than the lock"
    );
    if let TopUpState::Applied {
        credits_added: Some(reported),
        ..
    } = out.state
    {
        assert_eq!(reported, added);
    }
    assert!(engine.block_on(dp.pending_top_ups()).unwrap().is_empty());
}

/// A wallet removed and imported again (the same wallet id) reads its
/// applied and its resolved top-ups back settled when a restore recovers
/// their locks: the journal's records outlive `wallet.sqlite`, whose
/// `Consumed` mark the removal erased, as the steps' signed locks do
/// (review F1). The identity tops up again at once.
#[test]
fn a_wallet_imported_again_reads_its_top_ups_settled() {
    let f = fixture(false);
    let lib = f.stored_fake(None);
    let applied = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(applied.state, TopUpState::Applied { .. }));
    lib.w().false_taken = true;
    let resolved = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(
        resolved.state,
        consumption_unknown(START_BALANCE + credited(DUFFS))
    );
    f.resolve(&lib, &resolved.txid).unwrap();
    lib.w().false_taken = false;
    let locks = [lib.tracked(&applied.txid), lib.tracked(&resolved.txid)];

    f.remove_and_import_again();
    let lib = f.stored_fake(Some(&lib));
    assert!(lib.rt_locks().is_empty(), "wallet.sqlite went");
    lib.recover(&locks);
    assert_eq!(lib.rt_locks().len(), 2);
    assert!(f.pending(&lib).is_empty());
    // Finish still offers both to Platform: the applied one is taken
    // (nothing counted), the resolved one Platform never took is applied.
    assert_eq!(counted(f.finish(&lib)), (1, 1, 0));
    let next = f.top_up(&lib, DUFFS).unwrap();
    assert!(matches!(next.state, TopUpState::Applied { .. }));
    assert_eq!(f.signed(&top_up_step(0, 2)).len(), 1);
}

/// Until the dispatch journal is read no recovered lock can be told
/// apart: the quote, the list, the top-up and the resolution are
/// `platform.unavailable`, never a pending lock or an empty list (review
/// L5, F5).
#[test]
fn every_top_up_call_waits_for_the_journal() {
    let mut f = fixture(false);
    let lib = f.stored_fake(None);
    lib.w().false_taken = true;
    let out = f.top_up(&lib, DUFFS).unwrap();
    assert_eq!(out.state, consumption_unknown(START_BALANCE));

    f.reopen_without_journal();
    let lib = f.stored_fake(Some(&lib));
    let unavailable = |r: Result<(), CreditsError>| {
        matches!(r, Err(CreditsError::Platform(PlatformError::Unavailable)))
    };
    assert!(unavailable(f.quote(&lib, DUFFS).map(drop)));
    assert!(unavailable(f.top_up(&lib, DUFFS).map(drop)));
    assert!(unavailable(f.resolve(&lib, &out.txid)));
    let listed = f
        .engine
        .block_on(topup::pending(&f.session, f.wallet, &*lib));
    assert!(matches!(listed, Err(PlatformError::Unavailable)));
}
