//! Tests of the masternode withdrawal (MG-13 commit 4, plan §3): the core
//! over a scripted Platform ([`Platform`]: Drive's nonce rule, its window at
//! execution, its documents, and each read's heights and provenance), a
//! scripted session ([`Scene`]: the app DB on disk, rule 5, the header tip,
//! the barriers' operations) and the real vault's single-use signer
//! ([`Auth`]); the lease path on a real session; and the fault sweep. The
//! facade's calls on a real session are `tests/facade.rs`.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dashcore::Network;
use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::ProtocolError;
use dpp::consensus::ConsensusError;
use dpp::consensus::basic::BasicError;
use dpp::consensus::state::identity::invalid_identity_contract_nonce_error::InvalidIdentityNonceError;
use dpp::consensus::state::state_error::StateError;
use dpp::document::{Document, DocumentV0Getters};
use dpp::identity::accessors::{IdentityGettersV0, IdentitySettersV0};
use dpp::identity::core_script::CoreScript;
use dpp::identity::identity_nonce::{
    IDENTITY_NONCE_VALUE_FILTER_MAX_BYTES, MergeIdentityNonceResult,
    validate_identity_nonce_update, validate_new_identity_nonce,
};
use dpp::identity::identity_public_key::accessors::v0::{
    IdentityPublicKeyGettersV0, IdentityPublicKeySettersV0,
};
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::v0::IdentityV0;
use dpp::identity::{Identity, IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::Value;
use dpp::prelude::Identifier;
use dpp::serialization::Signable;
use dpp::state_transition::StateTransition;
use dpp::state_transition::identity_credit_withdrawal_transition::v1::IdentityCreditWithdrawalTransitionV1;
use dpp::state_transition::identity_credit_withdrawal_transition::{
    IdentityCreditWithdrawalTransition, MIN_CORE_FEE_PER_BYTE,
};
use dpp::system_data_contracts::withdrawals_contract::v1::document_types::withdrawal;
use dpp::version::PlatformVersion;
use dpp::withdrawal::Pooling;
use dw_appdb::{
    AppDb, MasternodeOpRow, SignedWithdrawal, WithdrawalKeyKind, WithdrawalMark, WithdrawalProof,
    WithdrawalRow, WithdrawalState,
};
use dw_vault::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, SeedDerivation, SignerError,
    Vault, VaultConfig, WalletSecret, WithdrawalDestination, WithdrawalKey,
};
use zeroize::Zeroizing;

use super::check::{Destination, gated, identity_of, quote};
use super::drive::{Window, candidates, identity_key, matches, window};
use super::net::{Answer, MnWithdrawNet, Proved, classify};
use super::recheck::{Outcome, mark, recheck_all, recheck_group, recheck_record};
use super::sign::{Request, WithdrawalKeyChoice, retry, shown, withdraw};
use super::{
    Alarm, Authorize, Chain, CheckingReason, Env, HeldKey, UNLOCKED, Waits, WalletKeys,
    WithdrawalError, identity_lock, points,
};
use crate::EngineError;
use crate::masternodes::display_hex;
use crate::masternodes::protx_records::ProTxOperationState;
use crate::platform::trust::Rule5;
use crate::platform::withdrawal::{
    CORE_FEE_PER_BYTE, READ_BACK_TRIES, Rules, document_id, drive_document, expected_duffs, takes,
};
use crate::platform::{PlatformError, WithdrawAmount};

mod facade;

// ---------------------------------------------------------------------------
// The scripted Platform.

/// The masternode's proTxHash (wire order) for `n`; its identity is the
/// reverse.
fn pro_tx(n: u8) -> [u8; 32] {
    std::array::from_fn(|i| n.wrapping_add(i as u8))
}

const PRO_TX: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];
const OWNER_SECRET: [u8; 32] = [0x07; 32];
const PAYOUT_SECRET: [u8; 32] = [0x09; 32];
const WALLET: [u8; 32] = [0x31; 32];
const DASH: u64 = 100_000_000_000;
const AMOUNT: u64 = DASH;
const BALANCE: u64 = 10 * DASH;
/// SPV's synced header tip, and Platform's core height by default.
const TIP: u32 = 100;
/// What Drive charges an executed withdrawal on top of its amount.
const FEE: u64 = 210_000_000;
/// A deviation of every read of its kind.
const ALWAYS: usize = usize::MAX;
/// Short waits: the same code paths at test speed.
const WAITS: Waits = Waits {
    quote: Duration::from_secs(1),
    sign: Duration::from_millis(150),
    poll: Duration::from_millis(20),
    probe: Duration::from_millis(100),
    recheck: Duration::from_secs(1),
};

fn hash_of(secret: &[u8; 32]) -> [u8; 20] {
    let secret = SecretKey::from_secret_bytes(*secret).unwrap();
    hash160::Hash::hash(&PublicKey::from_secret_key(&secret).serialize()).to_byte_array()
}

fn key(id: u32, purpose: Purpose, key_type: KeyType, data: &[u8]) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id,
        purpose,
        security_level: SecurityLevel::CRITICAL,
        contract_bounds: None,
        key_type,
        read_only: true,
        data: data.to_vec().into(),
        disabled_at: None,
    })
}

/// Drive's owner identity key (OWNER, id 1).
fn owner_key() -> IdentityPublicKey {
    key(
        1,
        Purpose::OWNER,
        KeyType::ECDSA_HASH160,
        &hash_of(&OWNER_SECRET),
    )
}

/// A TRANSFER key on the payout key's hash160 (Drive's is id 0).
fn payout_key(id: u32) -> IdentityPublicKey {
    key(
        id,
        Purpose::TRANSFER,
        KeyType::ECDSA_HASH160,
        &hash_of(&PAYOUT_SECRET),
    )
}

/// A TRANSFER key on another hash160 (a payout moved elsewhere).
fn other_key(id: u32, n: u8) -> IdentityPublicKey {
    key(id, Purpose::TRANSFER, KeyType::ECDSA_HASH160, &[n; 20])
}

fn disabled(mut k: IdentityPublicKey) -> IdentityPublicKey {
    k.set_disabled_at(1);
    k
}

fn payout_script() -> CoreScript {
    CoreScript::new_p2pkh(hash_of(&PAYOUT_SECRET))
}

fn masternode_identity(pro_tx: &[u8; 32], keys: Vec<IdentityPublicKey>) -> Identity {
    Identity::V0(IdentityV0 {
        id: identity_of(pro_tx),
        public_keys: keys.into_iter().map(|k| (k.id(), k)).collect(),
        balance: BALANCE,
        revision: 0,
    })
}

/// The document Drive writes for `owner`'s withdrawal of `amount` at
/// `nonce` to `script`.
fn document(owner: Identifier, nonce: u64, script: &CoreScript, amount: u64) -> Document {
    let mut d = drive_document(amount, script, PlatformVersion::latest()).unwrap();
    let Document::V0(v) = &mut d;
    v.id = document_id(&owner, nonce, script);
    v.owner_id = owner;
    d
}

fn decoded(bytes: &[u8]) -> IdentityCreditWithdrawalTransitionV1 {
    let t = StateTransition::deserialize_from_bytes_untrusted_exact_in_version(
        bytes,
        PlatformVersion::latest(),
    )
    .unwrap();
    let StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(w)) = t
    else {
        panic!("not a V1 withdrawal");
    };
    w
}

/// Platform's state at one height.
#[derive(Clone)]
struct World {
    identity: Option<Identity>,
    /// The identity's nonces Drive took.
    used: BTreeSet<u64>,
    height: u64,
    core_height: u32,
    time_ms: u64,
    documents: Vec<Document>,
}

impl World {
    /// The nonce as Drive stores it: the newest, and a bit for each of the
    /// 24 below it not yet used (`validate_identity_nonce_update`).
    fn nonce_state(&self) -> u64 {
        let tip = self.used.last().copied().unwrap_or(0);
        (1..=24u64)
            .filter(|&p| tip > p && !self.used.contains(&(tip - p)))
            .fold(tip, |state, p| {
                state | 1 << (p - 1 + IDENTITY_NONCE_VALUE_FILTER_MAX_BYTES)
            })
    }

    fn tip(&self) -> u64 {
        self.used.last().copied().unwrap_or(0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Read {
    Identity,
    Nonce,
    Balance,
    Document,
}

/// How the next reads of one kind deviate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Dev {
    /// Its proof used a quorum key SPV has not verified (P12).
    Unverified,
    /// A node behind answers, from the snapshot ([`Platform::snapshot`]).
    Behind,
    /// The read fails (`platform.unavailable`).
    Fail,
    /// The proof disagrees with SPV (rule 4).
    Mismatch,
}

type Hook = Box<dyn FnOnce() + Send>;

/// Platform as a masternode withdrawal sees it.
pub(super) struct Platform {
    world: Mutex<World>,
    old: Mutex<Option<World>>,
    devs: Mutex<HashMap<(Read, Dev), usize>>,
    /// The next reads of a kind claim this height.
    claims: Mutex<HashMap<Read, (usize, u64)>>,
    /// The core heights the next identity reads see, in order.
    core_heights: Mutex<VecDeque<u32>>,
    /// Scripted CheckTx answers, and whether the bytes reach a mempool.
    answers: Mutex<VecDeque<(Answer, bool)>>,
    /// Every broadcast's bytes.
    sent: Mutex<Vec<Vec<u8>>>,
    mempool: Mutex<Vec<Vec<u8>>>,
    reads: Mutex<HashMap<Read, usize>>,
    /// Runs at the next read of its kind.
    hook: Mutex<Option<(Read, Hook)>>,
    /// Each identity read: whether the identity's lock was free then.
    watch_lock: AtomicBool,
    lock_free: Mutex<Vec<bool>>,
}

impl Platform {
    fn new(pro_tx: &[u8; 32]) -> Arc<Self> {
        Arc::new(Self {
            world: Mutex::new(World {
                identity: Some(masternode_identity(
                    pro_tx,
                    vec![payout_key(0), owner_key()],
                )),
                used: BTreeSet::from([1, 2]),
                height: 10,
                core_height: TIP,
                time_ms: 2_000_000,
                documents: Vec::new(),
            }),
            old: Mutex::default(),
            devs: Mutex::default(),
            claims: Mutex::default(),
            core_heights: Mutex::default(),
            answers: Mutex::default(),
            sent: Mutex::default(),
            mempool: Mutex::default(),
            reads: Mutex::default(),
            hook: Mutex::default(),
            watch_lock: AtomicBool::new(false),
            lock_free: Mutex::default(),
        })
    }

    fn with<T>(&self, f: impl FnOnce(&mut World) -> T) -> T {
        f(&mut self.world.lock().unwrap())
    }

    fn identity_mut<T>(&self, f: impl FnOnce(&mut Identity) -> T) -> T {
        self.with(|w| f(w.identity.as_mut().unwrap()))
    }

    /// A node behind answers from Platform as it is now.
    fn snapshot(&self) {
        *self.old.lock().unwrap() = Some(self.world.lock().unwrap().clone());
    }

    fn deviate(&self, read: Read, dev: Dev, n: usize) {
        self.devs.lock().unwrap().insert((read, dev), n);
    }

    fn calm(&self) {
        self.devs.lock().unwrap().clear();
        self.claims.lock().unwrap().clear();
    }

    fn claim(&self, read: Read, n: usize, height: u64) {
        self.claims.lock().unwrap().insert(read, (n, height));
    }

    fn answer(&self, answer: Answer, lands: bool) {
        self.answers.lock().unwrap().push_back((answer, lands));
    }

    fn take(&self, read: Read, dev: Dev) -> bool {
        let mut devs = self.devs.lock().unwrap();
        match devs.get_mut(&(read, dev)) {
            Some(0) | None => false,
            Some(n) => {
                if *n != ALWAYS {
                    *n -= 1;
                }
                true
            }
        }
    }

    fn reads(&self, read: Read) -> usize {
        self.reads.lock().unwrap().get(&read).copied().unwrap_or(0)
    }

    fn sent(&self) -> Vec<Vec<u8>> {
        self.sent.lock().unwrap().clone()
    }

    fn read<T>(&self, kind: Read, f: impl FnOnce(&World) -> T) -> Result<Proved<T>, PlatformError> {
        *self.reads.lock().unwrap().entry(kind).or_default() += 1;
        let hook = {
            let mut hook = self.hook.lock().unwrap();
            match hook.take() {
                Some((k, h)) if k == kind => Some(h),
                other => {
                    *hook = other;
                    None
                }
            }
        };
        if let Some(hook) = hook {
            hook();
        }
        if self.take(kind, Dev::Fail) {
            return Err(PlatformError::Unavailable);
        }
        if self.take(kind, Dev::Mismatch) {
            return Err(PlatformError::TrustMismatch);
        }
        let world = if self.take(kind, Dev::Behind) {
            self.old.lock().unwrap().clone().expect("a snapshot")
        } else {
            self.world.lock().unwrap().clone()
        };
        let mut core_height = world.core_height;
        if kind == Read::Identity {
            if let Some(c) = self.core_heights.lock().unwrap().pop_front() {
                core_height = c;
            }
            if self.watch_lock.load(Ordering::SeqCst) {
                let id = world.identity.as_ref().unwrap().id();
                let free = identity_lock(id).try_lock().is_ok();
                self.lock_free.lock().unwrap().push(free);
            }
        }
        let height = {
            let mut claims = self.claims.lock().unwrap();
            match claims.get_mut(&kind) {
                Some((n, h)) if *n > 0 => {
                    *n -= 1;
                    Some(*h)
                }
                _ => None,
            }
        };
        Ok(Proved {
            value: f(&world),
            height: height.unwrap_or(world.height),
            core_height,
            time_ms: world.time_ms,
            verified: !self.take(kind, Dev::Unverified),
        })
    }

    /// Drive executes `bytes` by its rules: the nonce (`takes`), the signing
    /// key enabled, the balance, and with no output script its window at
    /// execution (F3). Whether it did.
    fn execute(&self, bytes: &[u8]) -> bool {
        let w = decoded(bytes);
        let mut world = self.world.lock().unwrap();
        let state = world.nonce_state();
        let Some(identity) = world.identity.as_mut() else {
            return false;
        };
        if !takes(state, w.nonce, w.identity_id) {
            return false;
        }
        let key_ok = (identity.public_keys().get(&w.signature_public_key_id))
            .is_some_and(|k| !k.is_disabled());
        let script = match &w.output_script {
            Some(s) => Some(s.clone()),
            None => window(identity).script().ok().cloned(),
        };
        let (Some(script), true) = (script, key_ok) else {
            return false;
        };
        let Some(after) = identity.balance().checked_sub(w.amount + FEE) else {
            return false;
        };
        identity.set_balance(after);
        world.used.insert(w.nonce);
        world
            .documents
            .push(document(w.identity_id, w.nonce, &script, w.amount));
        world.height += 1;
        world.time_ms += 1_000;
        true
    }

    /// The mempool reaches a block.
    fn block(&self) -> usize {
        let pool = std::mem::take(&mut *self.mempool.lock().unwrap());
        pool.iter().filter(|b| self.execute(b)).count()
    }

    /// Another transition of the identity executes at `nonce`, writing a
    /// withdrawal document of `amount` to `script` (`None`: a key update).
    fn rival(&self, nonce: u64, paid: Option<(&CoreScript, u64)>) {
        self.with(|w| {
            let id = w.identity.as_ref().unwrap().id();
            assert!(takes(w.nonce_state(), nonce, id), "Drive takes {nonce}");
            w.used.insert(nonce);
            if let Some((script, amount)) = paid {
                w.documents.push(document(id, nonce, script, amount));
            }
            w.height += 1;
            w.time_ms += 1_000;
        });
    }

    /// CheckTx as Drive's: a disabled signing key or a nonce Drive does not
    /// take is refused; bytes already in the mempool are "in cache".
    fn check_tx(&self, bytes: &[u8]) -> Answer {
        let w = decoded(bytes);
        let (state, key_ok) = self.with(|world| {
            let key_ok = (world.identity.as_ref())
                .and_then(|i| i.public_keys().get(&w.signature_public_key_id).cloned())
                .is_some_and(|k| !k.is_disabled());
            (world.nonce_state(), key_ok)
        });
        let check = if state == 0 {
            validate_new_identity_nonce(w.nonce, w.identity_id)
        } else {
            validate_identity_nonce_update(state, w.nonce, w.identity_id)
        };
        if let Some(e) = check.errors.into_iter().next() {
            return classify(Err(dash_sdk::Error::Protocol(
                ProtocolError::ConsensusError(Box::new(e)),
            )));
        }
        if !key_ok {
            return Answer::Refused("40206 the signing key is disabled".into());
        }
        let mut pool = self.mempool.lock().unwrap();
        if pool.iter().any(|b| b == bytes) {
            return classify(Err(dash_sdk::Error::AlreadyExists(
                "tx already exists in cache".into(),
            )));
        }
        pool.push(bytes.to_vec());
        Answer::Accepted
    }
}

impl MnWithdrawNet for Platform {
    fn version(&self) -> &'static PlatformVersion {
        PlatformVersion::latest()
    }

    async fn identity(&self, id: Identifier) -> Result<Proved<Option<Identity>>, PlatformError> {
        self.read(Read::Identity, |w| {
            w.identity.clone().filter(|i| i.id() == id)
        })
    }

    async fn nonce_state(&self, _: Identifier) -> Result<Proved<u64>, PlatformError> {
        self.read(Read::Nonce, World::nonce_state)
    }

    async fn balance(&self, _: Identifier) -> Result<Proved<Option<u64>>, PlatformError> {
        self.read(Read::Balance, |w| w.identity.as_ref().map(|i| i.balance()))
    }

    async fn document(&self, id: Identifier) -> Result<Proved<Option<Document>>, PlatformError> {
        self.read(Read::Document, |w| {
            w.documents.iter().find(|d| d.id() == id).cloned()
        })
    }

    async fn broadcast(&self, bytes: &[u8]) -> Answer {
        self.sent.lock().unwrap().push(bytes.to_vec());
        let scripted = self.answers.lock().unwrap().pop_front();
        match scripted {
            Some((answer, lands)) => {
                if lands {
                    self.mempool.lock().unwrap().push(bytes.to_vec());
                }
                answer
            }
            None => self.check_tx(bytes),
        }
    }
}

/// What a real session's withdrawal calls use instead of the SDK, SPV's
/// header tip and the default waits (`WithdrawalRuntime::seam`; the
/// facade's tests, `tests/facade.rs`).
pub(super) struct Seam {
    pub(super) platform: Arc<Platform>,
    pub(super) header_tip: Option<u32>,
    pub(super) waits: Waits,
}

// ---------------------------------------------------------------------------
// The scripted session.

/// The session as the core reads it: the app DB on disk, rule 5, SPV's
/// header tip, the barrier backfill and each barrier's operation.
struct Scene {
    path: PathBuf,
    db: Arc<AppDb>,
    rule: Mutex<Rule5>,
    tip: Mutex<Option<u32>>,
    ready: AtomicBool,
    /// Each barrier's operation by its `op_id`; absent: unreadable.
    ops: Mutex<HashMap<String, ProTxOperationState>>,
    clock: AtomicU64,
    alarms: Mutex<Vec<Alarm>>,
}

impl Scene {
    fn open(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            db: Arc::new(AppDb::open(path).unwrap()),
            rule: Mutex::new(Rule5::Enforce),
            tip: Mutex::new(Some(TIP)),
            ready: AtomicBool::new(true),
            ops: Mutex::default(),
            clock: AtomicU64::new(1_000),
            alarms: Mutex::default(),
        }
    }

    fn rule(&self, rule: Rule5) {
        *self.rule.lock().unwrap() = rule;
    }

    fn alarms(&self) -> Vec<Alarm> {
        self.alarms.lock().unwrap().clone()
    }
}

impl Env for Scene {
    fn db(&self) -> &Arc<AppDb> {
        &self.db
    }

    fn rule_5(&self) -> Rule5 {
        *self.rule.lock().unwrap()
    }

    fn chain(&self) -> Chain {
        Chain {
            header_tip: *self.tip.lock().unwrap(),
        }
    }

    fn barriers_ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }

    async fn barrier_state(
        &self,
        barrier: &dw_appdb::PayoutBarrierRow,
    ) -> Result<Option<ProTxOperationState>, WithdrawalError> {
        Ok(self
            .ops
            .lock()
            .unwrap()
            .get(&barrier.barrier.op_id)
            .cloned())
    }

    fn waits(&self) -> Waits {
        WAITS
    }

    /// Strictly increasing: the newest record is the last signed.
    fn now(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::SeqCst)
    }

    fn alarm(&self, alarm: Alarm) {
        self.alarms.lock().unwrap().push(alarm);
    }
}

/// The wallet's keys: the attached owner and payout keys.
struct Keys(Vec<HeldKey>);

impl Keys {
    fn attached(pro_tx: &[u8; 32]) -> Self {
        Self(vec![
            HeldKey {
                key: WithdrawalKey::AttachedOwner {
                    pro_tx_hash: *pro_tx,
                },
                hash: hash_of(&OWNER_SECRET),
            },
            HeldKey {
                key: WithdrawalKey::AttachedPayout {
                    pro_tx_hash: *pro_tx,
                },
                hash: hash_of(&PAYOUT_SECRET),
            },
        ])
    }
}

impl WalletKeys for Keys {
    async fn held(
        &self,
        owner: Vec<[u8; 20]>,
        payout: Vec<[u8; 20]>,
    ) -> Result<Vec<HeldKey>, WithdrawalError> {
        Ok((self.0.iter())
            .filter(|h| match h.key.is_owner() {
                true => owner.contains(&h.hash),
                false => payout.contains(&h.hash),
            })
            .cloned()
            .collect())
    }
}

/// The real vault's grant and single-use signer, the grant bound to the
/// charge and the transition's script as the user approved them.
#[derive(Clone)]
struct Auth {
    vault: Arc<Vault>,
    calls: Arc<AtomicUsize>,
    /// Both racers meet here before signing (the index-alone race).
    meet: Option<Arc<tokio::sync::Barrier>>,
}

impl Authorize for Auth {
    async fn sign(
        self,
        credits: u64,
        key: &WithdrawalKey,
        identity_key: &IdentityPublicKey,
        transition: &StateTransition,
        pro_tx_hash: [u8; 32],
    ) -> Result<[u8; 65], WithdrawalError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(meet) = &self.meet {
            meet.wait().await;
        }
        let StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(t)) =
            transition
        else {
            panic!("not a V1 withdrawal");
        };
        let purpose = GrantPurpose::MasternodeWithdraw {
            max_credits: credits,
            destination: (t.output_script.as_ref())
                .map(|s| WithdrawalDestination::of_script(s.as_bytes())),
        };
        let v = &self.vault;
        let grant = v
            .authorize(purpose, Some(&WALLET), Credential::None)
            .unwrap();
        let token = (v.redeem_grant(&grant.id, purpose.kind(), Some(&WALLET))).unwrap();
        let signer = v.masternode_withdrawal_signer(&WALLET, &token).unwrap();
        (signer.sign_masternode_withdrawal(key, identity_key, transition, &pro_tx_hash)).map_err(
            |e| match e {
                SignerError::KeyMismatch(_) => WithdrawalError::KeyMissing,
                e => EngineError::from(e).into(),
            },
        )
    }
}

fn test_vault(dir: &Path, pro_tx: &[u8; 32]) -> Vault {
    let config = VaultConfig {
        kdf: KdfPolicy::Fixed(KdfParams::TEST),
        os_store: Arc::new(MemoryOsStore::new()),
        ..VaultConfig::default()
    };
    let v = Vault::open(dir, Network::Regtest, "regtest", config).unwrap();
    v.create(None).unwrap();
    let secret = WalletSecret {
        mnemonic: Zeroizing::new(b"phrase".to_vec()),
        mnemonic_passphrase: Zeroizing::new(Vec::new()),
        seed: Zeroizing::new([3; 64]),
        derivation: SeedDerivation::Bip39,
    };
    v.store_wallet_secret(&WALLET, &secret).unwrap();
    v.store_masternode_key(pro_tx, "owner", &OWNER_SECRET)
        .unwrap();
    v.store_masternode_key(pro_tx, "payout", &PAYOUT_SECRET)
        .unwrap();
    v
}

/// One masternode's withdrawals over the scripted Platform and session.
struct Fx {
    _dir: tempfile::TempDir,
    pro_tx: [u8; 32],
    id: Identifier,
    plat: Arc<Platform>,
    scene: Scene,
    vault: Arc<Vault>,
    calls: Arc<AtomicUsize>,
    keys: Keys,
}

fn fx() -> Fx {
    Fx::new(PRO_TX)
}

use WithdrawalKeyChoice::{Owner, Payout};

impl Fx {
    fn new(pro_tx: [u8; 32]) -> Self {
        let dir = dw_testutil::private_tempdir();
        Self {
            scene: Scene::open(&dir.path().join("app.sqlite")),
            vault: Arc::new(test_vault(&dir.path().join("vault"), &pro_tx)),
            _dir: dir,
            pro_tx,
            id: identity_of(&pro_tx),
            plat: Platform::new(&pro_tx),
            calls: Arc::default(),
            keys: Keys::attached(&pro_tx),
        }
    }

    fn auth(&self) -> Auth {
        Auth {
            vault: Arc::clone(&self.vault),
            calls: Arc::clone(&self.calls),
            meet: None,
        }
    }

    fn signatures(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn req(&self, key: WithdrawalKeyChoice, amount: u64) -> Request {
        Request {
            pro_tx_hash: self.pro_tx,
            key,
            amount: WithdrawAmount::Credits { credits: amount },
            destination: None,
            created_by: "wallet-1".into(),
        }
    }

    async fn withdraw(
        &self,
        key: WithdrawalKeyChoice,
        amount: u64,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        self.withdraw_req(&self.req(key, amount)).await
    }

    async fn withdraw_req(&self, req: &Request) -> Result<WithdrawalRow, WithdrawalError> {
        let net = &*self.plat;
        withdraw(
            &self.scene,
            net,
            &self.keys,
            self.auth(),
            Network::Regtest,
            req,
        )
        .await
    }

    async fn retry(
        &self,
        refused: &str,
        key: WithdrawalKeyChoice,
        amount: u64,
    ) -> Result<WithdrawalRow, WithdrawalError> {
        let req = self.req(key, amount);
        let net = &*self.plat;
        retry(
            &self.scene,
            net,
            &self.keys,
            self.auth(),
            Network::Regtest,
            refused,
            &req,
        )
        .await
    }

    async fn recheck(&self, nonce: u64) -> Outcome {
        recheck_group(&self.scene, &*self.plat, &self.id, nonce)
            .await
            .unwrap()
    }

    fn rows(&self) -> Vec<WithdrawalRow> {
        self.scene
            .db
            .withdrawals_of_identity(&shown(&self.id))
            .unwrap()
    }

    fn row(&self, id: &str) -> WithdrawalRow {
        self.scene.db.withdrawal(id).unwrap().unwrap()
    }

    /// A restart: the app DB reopened and the session's state fresh, over
    /// the same Platform.
    fn restart(&mut self) {
        let path = self.scene.path.clone();
        let (rule, tip) = (self.scene.rule_5(), self.scene.chain().header_tip);
        let ops = self.scene.ops.lock().unwrap().clone();
        self.scene = Scene::open(&path);
        self.scene.rule(rule);
        *self.scene.tip.lock().unwrap() = tip;
        *self.scene.ops.lock().unwrap() = ops;
    }

    /// A ProUpRegTx of `pro_tx` to `script`, signed at `created_at`, with
    /// its barrier (MG-04, P16).
    fn barrier(&self, pro_tx: &[u8; 32], op: &str, script: &[u8], created_at: u64) {
        let row = MasternodeOpRow {
            id: op.into(),
            kind: "update_registrar".into(),
            terms: "{}".into(),
            request: "{}".into(),
            unsigned_tx: None,
            sign_address: None,
            sign_message: None,
            fee_inputs: "[]".into(),
            signed_tx: Some(vec![2]),
            txid: Some("ab".repeat(32)),
            pro_tx_hash: Some(display_hex(pro_tx)),
            locked_collateral: false,
            fee: 7,
            created_at,
            signed_at: Some(created_at),
            sent_at: None,
            last_error: None,
        };
        (self.scene.db)
            .insert_masternode_op_with_barrier_durable("w", &row, &[], created_at, Some(script))
            .unwrap();
    }

    fn barriers(&self) -> usize {
        self.scene
            .db
            .payout_barriers_of(&display_hex(&self.pro_tx))
            .unwrap()
            .len()
    }

    /// A record written straight to the app DB at `nonce`, in `state`.
    fn crafted(&self, nonce: u64, state: WithdrawalState) -> WithdrawalRow {
        let id = format!("crafted-{nonce}-{}", state.as_str());
        let row = SignedWithdrawal {
            id: id.clone(),
            transition_hash: format!("{:064x}", nonce),
            created_by: "wallet-1".into(),
            pro_tx_hash: display_hex(&self.pro_tx),
            identity_id: shown(&self.id),
            key_kind: WithdrawalKeyKind::Payout,
            amount: AMOUNT,
            core_fee_per_byte: CORE_FEE_PER_BYTE,
            pooling: Pooling::Never as u8,
            expected_duffs: 1,
            nonce,
            signed_bytes: vec![0],
            script: Some(payout_script().as_bytes().to_vec()),
            expected_script: None,
            signed_at: 1,
        };
        let db = &self.scene.db;
        db.insert_withdrawal_durable(&row, 0).unwrap();
        use WithdrawalState::*;
        let to = |from: &[WithdrawalState], to| {
            assert!(
                db.set_withdrawal_state_durable(&id, from, to, None)
                    .unwrap()
            );
        };
        match state {
            Signed => {}
            Sent => to(&[Signed], Sent),
            Refused => to(&[Signed], Refused),
            CouldntConfirm => to(&[Signed], CouldntConfirm),
            MarkedReceived | MarkedNotReceived => {
                to(&[Signed], CouldntConfirm);
                let mark = match state {
                    MarkedReceived => WithdrawalMark::Received,
                    _ => WithdrawalMark::NotReceived,
                };
                assert!(db.mark_withdrawal_durable(&id, mark, 2).unwrap());
            }
            Done => {
                let proof = WithdrawalProof {
                    document_id: "x".into(),
                    height: 1,
                    block_time: 1,
                    paid_script: payout_script().as_bytes().to_vec(),
                    document_status: 0,
                };
                assert!(
                    db.finalize_withdrawal_group_durable(&id, &[Signed], &proof)
                        .unwrap()
                );
            }
            NotExecuted => {
                let evidence = dw_appdb::NotExecutedEvidence {
                    nonce_height: 1,
                    identity_height: 1,
                    document_height: 1,
                    reason: None,
                };
                let shown = shown(&self.id);
                (db.not_executed_withdrawal_group_durable(
                    &shown,
                    nonce,
                    &[(&id, Signed)],
                    &evidence,
                ))
                .unwrap();
            }
            other => panic!("{other:?}"),
        }
        self.row(&id)
    }
}

fn state(row: &WithdrawalRow) -> WithdrawalState {
    row.state
}

// ---------------------------------------------------------------------------
// P2, F3: keys and Drive's window.

#[test]
fn drives_window_pays_the_highest_transfer_key_or_the_highest_enabled_of_the_next_five() {
    let id = |keys: Vec<IdentityPublicKey>| masternode_identity(&PRO_TX, keys);
    let paid = |i: &Identity| match window(i) {
        Window::Pays { key, .. } => Some(key),
        _ => None,
    };
    // The highest id wins, not the first.
    assert_eq!(
        paid(&id(vec![other_key(0, 1), other_key(4, 2), other_key(2, 3)])),
        Some(4)
    );
    // Disabled: the highest enabled among the next 5 (`limit 5, offset 1`).
    let keys = |n: u32, enabled: &[u32]| {
        (0..n)
            .map(|i| match enabled.contains(&i) {
                true => other_key(i, i as u8),
                false => disabled(other_key(i, i as u8)),
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(paid(&id(keys(8, &[2, 3]))), Some(3));
    assert_eq!(paid(&id(keys(8, &[2]))), Some(2), "the 6th from the top");
    assert_eq!(
        window(&id(keys(8, &[1]))),
        Window::Disabled,
        "the 7th is outside"
    );
    assert_eq!(window(&id(keys(3, &[]))), Window::Disabled);
    assert_eq!(window(&id(vec![owner_key()])), Window::NoTransferKey);
    // The mapping: P2PKH for ECDSA_HASH160, P2SH for BIP13_SCRIPT_HASH.
    let sh = key(5, Purpose::TRANSFER, KeyType::BIP13_SCRIPT_HASH, &[8; 20]);
    assert_eq!(
        window(&id(vec![other_key(0, 1), sh])),
        Window::Pays {
            script: CoreScript::new_p2sh([8; 20]),
            key: 5
        }
    );
    assert_eq!(
        window(&id(vec![other_key(0, 1)])).script().unwrap(),
        &CoreScript::new_p2pkh([1; 20])
    );
    let secp = key(3, Purpose::TRANSFER, KeyType::ECDSA_SECP256K1, &[2; 33]);
    assert_eq!(window(&id(vec![other_key(0, 1), secp])), Window::OtherType);
    for refused in [Window::Disabled, Window::NoTransferKey, Window::OtherType] {
        assert_eq!(
            refused.script().unwrap_err().code(),
            "masternodes.withdrawal_no_payout_key"
        );
    }
}

#[test]
fn the_selector_takes_a_matching_enabled_key_only() {
    let hash = hash_of(&PAYOUT_SECRET);
    let i = masternode_identity(&PRO_TX, vec![disabled(payout_key(0)), owner_key()]);
    assert!(matches!(
        identity_key(&i, Purpose::TRANSFER, &hash),
        Err(WithdrawalError::KeyMissing)
    ));
    let i = masternode_identity(&PRO_TX, vec![disabled(payout_key(0)), payout_key(3)]);
    assert_eq!(identity_key(&i, Purpose::TRANSFER, &hash).unwrap().id(), 3);
    // Purpose and type are part of the match.
    assert!(identity_key(&i, Purpose::OWNER, &hash).is_err());
    let sh = key(4, Purpose::TRANSFER, KeyType::BIP13_SCRIPT_HASH, &hash);
    let i = masternode_identity(&PRO_TX, vec![sh]);
    assert!(identity_key(&i, Purpose::TRANSFER, &hash).is_err());
}

#[tokio::test]
async fn a_disabled_payout_key_or_a_multi_payee_window_refuses_before_signing() {
    let f = fx();
    // v24 multi-payee: every TRANSFER key disabled (F8).
    f.plat.identity_mut(|i| {
        i.public_keys_mut().insert(0, disabled(payout_key(0)));
    });
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_key_missing");
    let e = f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_no_payout_key");
    assert_eq!(f.signatures(), 0);
    assert!(f.rows().is_empty());
}

#[tokio::test]
async fn an_owner_key_withdrawal_takes_no_destination() {
    let f = fx();
    let mut req = f.req(Owner, AMOUNT);
    req.destination = Some(CoreScript::new_p2pkh([4; 20]));
    let e = f.withdraw_req(&req).await.unwrap_err();
    assert_eq!(e.code(), "invalid_argument");
    // A payout key's destination is P2PKH or P2SH.
    let mut req = f.req(Payout, AMOUNT);
    req.destination = Some(CoreScript::from_bytes(vec![0x6a, 1, 2]));
    assert_eq!(
        f.withdraw_req(&req).await.unwrap_err().code(),
        "invalid_argument"
    );
    assert_eq!(f.signatures(), 0);
    req.destination = Some(CoreScript::new_p2sh([4; 20]));
    let row = f.withdraw_req(&req).await.unwrap();
    assert_eq!(
        decoded(&row.signed.signed_bytes).output_script,
        req.destination
    );
}

// ---------------------------------------------------------------------------
// P3, P5, P7: what is signed, stored and sent.

#[tokio::test]
async fn a_withdrawal_signs_v1_at_platforms_terms_stores_the_bytes_first_and_sends_them_unchanged()
{
    let f = fx();
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    let t = decoded(&row.signed.signed_bytes);
    assert_eq!(t.identity_id, f.id);
    assert_eq!(t.amount, AMOUNT);
    assert_eq!(t.core_fee_per_byte, MIN_CORE_FEE_PER_BYTE);
    assert_eq!(t.pooling, Pooling::Never);
    assert_eq!(t.user_fee_increase, 0);
    assert_eq!(t.output_script, Some(payout_script()));
    assert_eq!(t.nonce, 3, "tip + 1");
    assert_eq!(t.signature_public_key_id, 0);
    // The vault signed the transition's signable bytes with the payout key.
    let st = StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(
        t.clone(),
    ));
    let public = PublicKey::from_secret_key(&SecretKey::from_secret_bytes(PAYOUT_SECRET).unwrap());
    dpp::dashcore::signer::verify_data_signature(
        &st.signable_bytes().unwrap(),
        t.signature.as_slice(),
        &public.serialize(),
    )
    .unwrap();
    assert_eq!(
        row.signed.transition_hash,
        hex::encode(st.transaction_id().unwrap())
    );
    // Sent once, the stored bytes exactly; `sent_at` set; still open.
    assert_eq!(f.plat.sent(), vec![row.signed.signed_bytes.clone()]);
    assert_eq!(row.state, WithdrawalState::Sent);
    assert!(row.sent_at.is_some());
    assert_eq!(row.signed.key_kind, WithdrawalKeyKind::Payout);
    assert_eq!(
        row.signed.expected_duffs,
        expected_duffs(AMOUNT, PlatformVersion::latest()).unwrap()
    );
    assert_eq!(row.floor_height, 10, "the verified nonce read's height");
    // Executed: the recheck proves it.
    assert_eq!(f.plat.block(), 1);
    assert_eq!(f.recheck(3).await, Outcome::Done);
    let row = f.row(&row.signed.id);
    assert_eq!(row.state, WithdrawalState::Done);
    assert_eq!(row.paid_script, Some(payout_script().as_bytes().to_vec()));
    assert_eq!(row.proof.unwrap().height, 11);
}

#[tokio::test]
async fn an_owner_key_withdrawal_signs_no_script_and_records_the_window() {
    let f = fx();
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    let t = decoded(&row.signed.signed_bytes);
    assert_eq!(t.output_script, None);
    assert_eq!(t.signature_public_key_id, 1);
    assert_eq!(row.signed.key_kind, WithdrawalKeyKind::Owner);
    assert_eq!(row.signed.script, None);
    assert_eq!(
        row.signed.expected_script,
        Some(payout_script().as_bytes().to_vec())
    );
    f.plat.block();
    assert_eq!(f.recheck(3).await, Outcome::Done);
}

#[tokio::test]
async fn the_read_back_after_the_hand_off_decides_only_on_a_match() {
    let f = fx();
    // The node executes at once: P8 step 1 proves it in the call.
    f.plat.answer(Answer::Accepted, true);
    let plat = Arc::clone(&f.plat);
    *f.plat.hook.lock().unwrap() = Some((
        Read::Document,
        Box::new(move || {
            plat.block();
        }),
    ));
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(row.state, WithdrawalState::Done);
    // A rival at the next nonce with no document: the read-back decides
    // nothing (no step 2 in the call).
    f.plat.answer(Answer::Unknown("timeout".into()), false);
    let plat = Arc::clone(&f.plat);
    *f.plat.hook.lock().unwrap() = Some((
        Read::Document,
        Box::new(move || {
            plat.rival(4, None);
        }),
    ));
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(row.signed.nonce, 4);
    assert_eq!(row.state, WithdrawalState::Sent);
}

#[tokio::test]
async fn checktx_answers_leave_the_record_open_unless_refused_on_the_merits() {
    for (answer, want) in [
        (Answer::Accepted, WithdrawalState::Sent),
        (Answer::InCache, WithdrawalState::Sent),
        (Answer::Unknown("deadline".into()), WithdrawalState::Sent),
        (
            Answer::NonceRefused("40204 nonce".into()),
            WithdrawalState::Sent,
        ),
        (
            Answer::Refused("10001 merits".into()),
            WithdrawalState::Refused,
        ),
    ] {
        let f = fx();
        f.plat.answer(answer.clone(), false);
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        assert_eq!(row.state, want, "{answer:?}");
        if want == WithdrawalState::Refused {
            assert_eq!(row.state_reason.as_deref(), Some("10001 merits"));
        }
    }
}

#[test]
fn every_nonce_refusal_is_a_nonce_refusal_and_in_cache_is_in_cache() {
    let id = Identifier::from([1; 32]);
    let nonce = |e: MergeIdentityNonceResult| {
        ConsensusError::StateError(StateError::InvalidIdentityNonceError(
            InvalidIdentityNonceError::new(id, Some(3), 3, e),
        ))
    };
    let protocol =
        |c: ConsensusError| dash_sdk::Error::Protocol(ProtocolError::ConsensusError(Box::new(c)));
    for e in [
        MergeIdentityNonceResult::InvalidNonce,
        MergeIdentityNonceResult::NonceTooFarInFuture,
        MergeIdentityNonceResult::NonceTooFarInPast,
        MergeIdentityNonceResult::NonceAlreadyPresentAtTip,
        MergeIdentityNonceResult::NonceAlreadyPresentInPast(2),
    ] {
        let answer = classify(Err(protocol(nonce(e))));
        assert!(
            matches!(&answer, Answer::NonceRefused(t) if t.starts_with("40204")),
            "{e:?}"
        );
        // The same through the broadcast error and the retry envelope.
        let wrapped = dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(
            dash_sdk::Error::StateTransitionBroadcastError(
                dash_sdk::error::StateTransitionBroadcastError {
                    code: 40204,
                    message: "nonce".into(),
                    cause: Some(nonce(e)),
                },
            ),
        ));
        assert!(matches!(classify(Err(wrapped)), Answer::NonceRefused(_)));
    }
    let merits = ConsensusError::BasicError(BasicError::UnsupportedProtocolVersionError(
        dpp::consensus::basic::UnsupportedProtocolVersionError::new(99, 1),
    ));
    assert!(matches!(
        classify(Err(protocol(merits))),
        Answer::Refused(_)
    ));
    let cache = dash_sdk::Error::AlreadyExists("tx already exists in cache".into());
    assert_eq!(classify(Err(cache)), Answer::InCache);
    let lost = dash_sdk::Error::StateTransitionBroadcastError(
        dash_sdk::error::StateTransitionBroadcastError {
            code: 13,
            message: "internal".into(),
            cause: None,
        },
    );
    assert!(matches!(classify(Err(lost)), Answer::Unknown(_)));
    assert!(matches!(
        classify(Err(dash_sdk::Error::Generic("x".into()))),
        Answer::Unknown(_)
    ));
    assert_eq!(classify(Ok(())), Answer::Accepted);
}

#[tokio::test]
async fn one_node_refusing_after_another_took_the_bytes_is_refused_after_an_earlier_send_and_still_paid()
 {
    let f = fx();
    f.plat.answer(Answer::Accepted, true);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    // The recheck finds the nonce taken and sends again; another node
    // refuses.
    f.plat
        .answer(Answer::Refused("10504 balance".into()), false);
    assert_eq!(f.recheck(3).await, Outcome::Pending);
    let refused = f.row(&row.signed.id);
    assert_eq!(refused.state, WithdrawalState::Refused);
    assert_eq!(
        refused.state_reason.as_deref(),
        Some("10504 balance (after an earlier send)")
    );
    // The first node's mempool reaches a block.
    assert_eq!(f.plat.block(), 1);
    assert_eq!(f.recheck(3).await, Outcome::Done);
    assert_eq!(f.row(&row.signed.id).state, WithdrawalState::Done);
}

// ---------------------------------------------------------------------------
// P7: matches and non-matches.

#[tokio::test]
async fn each_candidate_kind_matches_a_disabled_keys_included_and_each_mapping() {
    // Platform's TRANSFER keys at the read: a P2SH one, a disabled one and
    // the quote's destination (key 0); Drive could have paid any.
    let sh = key(5, Purpose::TRANSFER, KeyType::BIP13_SCRIPT_HASH, &[8; 20]);
    for paid in [
        CoreScript::new_p2sh([8; 20]),
        CoreScript::new_p2pkh([6; 20]),
        payout_script(),
    ] {
        let f = fx();
        f.plat.answer(Answer::Unknown("lost".into()), false);
        let row = f.withdraw(Owner, AMOUNT).await.unwrap();
        f.plat.identity_mut(|i| {
            i.public_keys_mut().insert(5, sh.clone());
            i.public_keys_mut().insert(6, disabled(other_key(6, 6)));
        });
        let scripts = candidates(&row, f.plat.with(|w| w.identity.clone()).as_ref());
        assert!(scripts.contains(&paid));
        f.plat.rival(3, Some((&paid, AMOUNT)));
        assert_eq!(f.recheck(3).await, Outcome::Done, "{paid:?}");
        assert_eq!(
            f.row(&row.signed.id).paid_script,
            Some(paid.as_bytes().to_vec())
        );
    }
}

/// P7's candidates for an owner-key record: every TRANSFER key of the
/// identity read (disabled ones included, each as Drive maps it) and the
/// destination the signing look saw, even when the read lacks its key or
/// has no identity.
#[tokio::test]
async fn an_owner_key_records_candidates_keep_the_destination_it_saw() {
    let f = fx();
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    let seen = payout_script();
    assert_eq!(candidates(&row, None), vec![seen.clone()]);
    let moved = masternode_identity(
        &PRO_TX,
        vec![other_key(2, 0x22), disabled(other_key(3, 0x23))],
    );
    assert_eq!(
        candidates(&row, Some(&moved)),
        vec![
            CoreScript::new_p2pkh([0x23; 20]),
            CoreScript::new_p2pkh([0x22; 20]),
            seen
        ]
    );
    // A payout-key record's only candidate is its signed script.
    let row = fx().crafted(9, WithdrawalState::Sent);
    assert_eq!(candidates(&row, Some(&moved)), vec![payout_script()]);
}

#[tokio::test]
async fn another_owner_script_amount_fee_or_pooling_at_the_records_document_is_a_proved_non_match()
{
    type Change = fn(&mut dpp::document::DocumentV0);
    let cases: [(&str, Change); 5] = [
        ("owner", |d| d.owner_id = Identifier::from([0x44; 32])),
        ("script", |d| {
            let v = Value::Bytes(CoreScript::new_p2pkh([0x45; 20]).as_bytes().to_vec());
            d.properties
                .insert(withdrawal::properties::OUTPUT_SCRIPT.into(), v);
        }),
        ("amount", |d| {
            let v = Value::U64(AMOUNT + 1_000);
            d.properties
                .insert(withdrawal::properties::AMOUNT.into(), v);
        }),
        ("fee", |d| {
            let v = Value::U32(2);
            d.properties
                .insert(withdrawal::properties::CORE_FEE_PER_BYTE.into(), v);
        }),
        ("pooling", |d| {
            let v = Value::U8(Pooling::IfAvailable as u8);
            d.properties
                .insert(withdrawal::properties::POOLING.into(), v);
        }),
    ];
    for (what, change) in cases {
        let f = fx();
        f.plat.answer(Answer::Unknown("lost".into()), false);
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        // A document at the record's id (its identity, nonce and script)
        // with other effects.
        let mut d = document(f.id, 3, &payout_script(), AMOUNT);
        let Document::V0(v) = &mut d;
        change(v);
        assert_eq!(d.id(), document_id(&f.id, 3, &payout_script()));
        assert!(!matches(&d, &row, &f.id, &payout_script()), "{what}");
        f.plat.rival(3, None);
        f.plat.with(|w| w.documents.push(d));
        assert_eq!(f.recheck(3).await, Outcome::NotExecuted, "{what}");
        let row = f.row(&row.signed.id);
        assert_eq!(row.state, WithdrawalState::NotExecuted);
        let evidence = row.evidence.unwrap();
        assert!(evidence.nonce_height >= row.floor_height);
        assert!(evidence.document_height >= evidence.nonce_height);
    }
}

#[tokio::test]
async fn a_whole_nonce_group_is_not_executed_together() {
    let f = fx();
    f.plat.answer(Answer::Refused("10001 merits".into()), false);
    let refused = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let sibling = f
        .retry(&refused.signed.id, Owner, AMOUNT / 2)
        .await
        .unwrap();
    f.plat.rival(3, None);
    assert_eq!(f.recheck(3).await, Outcome::NotExecuted);
    for id in [&refused.signed.id, &sibling.signed.id] {
        let row = f.row(id);
        assert_eq!(row.state, WithdrawalState::NotExecuted);
        assert!(row.evidence.is_some());
    }
}

#[tokio::test]
async fn an_advanced_nonce_or_a_balance_drop_with_no_document_is_not_executed() {
    let f = fx();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat
        .identity_mut(|i| i.set_balance(BALANCE - AMOUNT - FEE));
    // While Drive still takes the nonce, the balance proves nothing.
    assert_eq!(f.recheck(3).await, Outcome::Pending);
    f.plat.mempool.lock().unwrap().clear();
    f.plat.rival(3, None);
    assert_eq!(f.recheck(3).await, Outcome::NotExecuted);
    assert_eq!(f.row(&row.signed.id).state, WithdrawalState::NotExecuted);
}

// ---------------------------------------------------------------------------
// P8: the recheck.

#[tokio::test]
async fn a_recheck_resends_only_the_open_siblings_stored_bytes_identical() {
    let f = fx();
    f.plat.answer(Answer::Refused("10001 merits".into()), false);
    let refused = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let sibling = f.retry(&refused.signed.id, Owner, AMOUNT).await.unwrap();
    assert_eq!(sibling.signed.nonce, 3);
    assert_eq!(f.row(&refused.signed.id).state, WithdrawalState::Superseded);
    let before = f.plat.sent().len();
    assert_eq!(f.recheck(3).await, Outcome::Pending);
    let sent = f.plat.sent();
    assert_eq!(sent.len(), before + 1);
    assert_eq!(sent.last().unwrap(), &sibling.signed.signed_bytes);
    assert_eq!(
        f.row(&sibling.signed.id).signed.signed_bytes,
        sibling.signed.signed_bytes
    );
    // The superseded one is never sent again.
    assert_eq!(
        (sent.iter())
            .filter(|b| **b == refused.signed.signed_bytes)
            .count(),
        1
    );
}

#[tokio::test]
async fn a_key_added_after_signing_with_the_identity_served_below_h_decides_no_not_executed() {
    let f = fx();
    f.plat.answer(Answer::Accepted, true);
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    f.plat.snapshot();
    // The payout moves to key 2 before the bytes execute: Drive pays it.
    f.plat.identity_mut(|i| {
        i.public_keys_mut().insert(2, other_key(2, 0x22));
    });
    assert_eq!(f.plat.block(), 1);
    // Nonce and documents current; the identity from a node behind H.
    f.plat.deviate(Read::Identity, Dev::Behind, ALWAYS);
    let outcome = f.recheck(3).await;
    assert!(matches!(outcome, Outcome::CouldntConfirm(_)), "{outcome:?}");
    assert_ne!(f.row(&row.signed.id).state, WithdrawalState::NotExecuted);
    // Served current, the new key's document proves it.
    f.plat.calm();
    assert_eq!(f.recheck(3).await, Outcome::Done);
    let done = f.row(&row.signed.id);
    assert_eq!(
        done.paid_script,
        Some(CoreScript::new_p2pkh([0x22; 20]).as_bytes().to_vec())
    );
}

#[tokio::test]
async fn a_payout_change_between_signing_and_execution_pays_the_new_window() {
    let f = fx();
    f.plat.answer(Answer::Accepted, true);
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    // F8: the masternode list update disables key 0 and adds key 2.
    f.plat.identity_mut(|i| {
        i.public_keys_mut().insert(0, disabled(payout_key(0)));
        i.public_keys_mut().insert(2, other_key(2, 0x22));
    });
    assert_eq!(f.plat.block(), 1);
    assert_eq!(f.recheck(3).await, Outcome::Done);
    let done = f.row(&row.signed.id);
    assert_eq!(
        done.paid_script,
        Some(CoreScript::new_p2pkh([0x22; 20]).as_bytes().to_vec())
    );
    assert_ne!(done.paid_script, row.signed.expected_script);
}

#[tokio::test]
async fn a_nonce_above_a_stale_tip_drive_does_not_take_decides_nothing() {
    let f = fx();
    let row = f.crafted(40, WithdrawalState::Sent);
    assert!(!takes(f.plat.with(|w| w.nonce_state()), 40, f.id));
    assert_eq!(f.recheck(40).await, Outcome::Stale);
    assert_eq!(f.row(&row.signed.id).state, WithdrawalState::Sent);
}

#[tokio::test]
async fn a_read_below_the_floor_is_ignored() {
    let f = fx();
    f.plat.snapshot();
    f.plat.with(|w| w.height = 20);
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(row.floor_height, 20);
    f.plat.rival(3, None);
    // Every document read from a node at 10, which has none: no proof of
    // absence.
    f.plat.deviate(Read::Document, Dev::Behind, ALWAYS);
    let outcome = f.recheck(3).await;
    assert!(matches!(outcome, Outcome::CouldntConfirm(_)), "{outcome:?}");
    assert_ne!(f.row(&row.signed.id).state, WithdrawalState::NotExecuted);
    // One stale read, then a current one: decided.
    f.plat.deviate(Read::Document, Dev::Behind, 1);
    assert_eq!(f.recheck(3).await, Outcome::NotExecuted);
}

#[tokio::test]
async fn an_unverified_read_with_a_huge_height_raises_no_floor_in_any_mode() {
    for rule in [Rule5::Enforce, Rule5::Probe, Rule5::Off] {
        let f = fx();
        f.scene.rule(rule);
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        assert_eq!(row.floor_height, 10);
        // Both the read and Probe's second one: unverified, at 10^9.
        f.plat.deviate(Read::Nonce, Dev::Unverified, 2);
        f.plat.claim(Read::Nonce, 2, 1_000_000_000);
        assert_eq!(f.recheck(3).await, Outcome::Pending, "{rule:?}");
        assert_eq!(f.row(&row.signed.id).floor_height, 10, "{rule:?}");
        // A later, lower, SPV-verified read is accepted.
        f.plat.block();
        assert_eq!(f.recheck(3).await, Outcome::Done, "{rule:?}");
    }
}

#[tokio::test]
async fn a_verified_nonce_read_raises_the_groups_floor() {
    let f = fx();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.with(|w| w.height = 30);
    assert_eq!(f.recheck(3).await, Outcome::Pending);
    assert_eq!(f.row(&row.signed.id).floor_height, 30);
}

#[tokio::test]
async fn couldnt_confirm_comes_only_after_every_round() {
    let f = fx();
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    // Two rounds fail; the third decides.
    f.plat
        .deviate(Read::Document, Dev::Fail, READ_BACK_TRIES - 1);
    assert_eq!(f.recheck(3).await, Outcome::Pending);
    assert_eq!(f.row(&row.signed.id).state, WithdrawalState::Sent);
    // Every round fails.
    let reads = f.plat.reads(Read::Document);
    f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
    let outcome = f.recheck(3).await;
    assert!(matches!(outcome, Outcome::CouldntConfirm(_)), "{outcome:?}");
    assert_eq!(f.plat.reads(Read::Document) - reads, READ_BACK_TRIES);
    let row = f.row(&row.signed.id);
    assert_eq!(row.state, WithdrawalState::CouldntConfirm);
    assert!(row.state_reason.unwrap().contains("document read failed"));
}

#[tokio::test]
async fn a_late_proof_after_either_mark_makes_the_record_done() {
    for to in [WithdrawalMark::Received, WithdrawalMark::NotReceived] {
        let f = fx();
        f.plat.answer(Answer::Unknown("lost".into()), false);
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        // Only from couldnt_confirm.
        let e = mark(&f.scene, &row.signed.id, to).await.unwrap_err();
        assert_eq!(e.code(), "masternodes.withdrawal_mark_state");
        f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
        f.recheck(3).await;
        f.plat.calm();
        let marked = mark(&f.scene, &row.signed.id, to).await.unwrap();
        assert!(matches!(
            marked.state,
            WithdrawalState::MarkedReceived | WithdrawalState::MarkedNotReceived
        ));
        assert!(!marked.state.is_open() && !marked.state.is_final());
        // Its bytes reach a block after all.
        assert!(f.plat.execute(&row.signed.signed_bytes));
        assert_eq!(
            recheck_all(&f.scene, &*f.plat).await.unwrap(),
            vec![Outcome::Done]
        );
        let done = f.row(&row.signed.id);
        assert_eq!(done.state, WithdrawalState::Done);
        assert_eq!(
            done.late_proof_link.as_deref(),
            Some(row.signed.id.as_str())
        );
        assert!(done.mark.is_some(), "the mark is kept apart from the proof");
    }
}

#[tokio::test]
async fn a_done_records_recheck_refreshes_its_status_and_a_not_executed_match_alarms() {
    let f = fx();
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.block();
    f.recheck(3).await;
    f.plat.with(|w| {
        let Document::V0(d) = &mut w.documents[0];
        d.properties
            .insert(withdrawal::properties::STATUS.into(), Value::U8(4));
    });
    let done = recheck_record(&f.scene, &*f.plat, &row.signed.id)
        .await
        .unwrap();
    assert_eq!(done.document_status, Some(4));
    // A not-executed group whose document later matches: kept, alarmed.
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let lost = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.rival(4, None);
    assert_eq!(f.recheck(4).await, Outcome::NotExecuted);
    f.plat.with(|w| {
        w.documents
            .push(document(f.id, 4, &payout_script(), AMOUNT))
    });
    let kept = recheck_record(&f.scene, &*f.plat, &lost.signed.id)
        .await
        .unwrap();
    assert_eq!(kept.state, WithdrawalState::NotExecuted);
    assert_eq!(f.scene.alarms().len(), 1);
}

// ---------------------------------------------------------------------------
// P10, P15: the nonce and Try again.

#[tokio::test]
async fn try_again_signs_a_sibling_at_the_refused_nonce_and_either_one_pays() {
    for refused_lands in [true, false] {
        let f = fx();
        f.plat.answer(Answer::Refused("10001 merits".into()), false);
        let refused = f.withdraw(Payout, AMOUNT).await.unwrap();
        f.plat.answer(Answer::Unknown("lost".into()), false);
        let sibling = f
            .retry(&refused.signed.id, Owner, AMOUNT / 2)
            .await
            .unwrap();
        assert_eq!(sibling.signed.nonce, refused.signed.nonce);
        let (paid, other) = match refused_lands {
            true => (&refused, &sibling),
            false => (&sibling, &refused),
        };
        assert!(f.plat.execute(&paid.signed.signed_bytes));
        assert_eq!(f.recheck(3).await, Outcome::Done);
        assert_eq!(f.row(&paid.signed.id).state, WithdrawalState::Done);
        assert_eq!(f.row(&other.signed.id).state, WithdrawalState::SiblingPaid);
    }
}

#[tokio::test]
async fn equal_effect_siblings_make_the_newest_done() {
    let f = fx();
    f.plat.answer(Answer::Refused("10001 merits".into()), false);
    let refused = f.withdraw(Payout, AMOUNT).await.unwrap();
    // The owner key pays the same script, the same amount: one document id.
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let sibling = f.retry(&refused.signed.id, Owner, AMOUNT).await.unwrap();
    assert!(f.plat.execute(&refused.signed.signed_bytes));
    assert_eq!(f.recheck(3).await, Outcome::Done);
    assert_eq!(f.row(&sibling.signed.id).state, WithdrawalState::Done);
    assert_eq!(
        f.row(&refused.signed.id).state,
        WithdrawalState::SiblingPaid
    );
}

#[tokio::test]
async fn try_again_only_from_refused_while_the_nonce_is_taken() {
    let f = fx();
    let sent = f.withdraw(Payout, AMOUNT).await.unwrap();
    let e = f.retry(&sent.signed.id, Owner, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_retry_state");
    let f = fx();
    f.plat.answer(Answer::Refused("10001 merits".into()), false);
    let refused = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.rival(3, None);
    let e = f
        .retry(&refused.signed.id, Owner, AMOUNT)
        .await
        .unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_retry_state");
    assert_eq!(f.signatures(), 1);
}

#[tokio::test]
async fn tip_plus_one_only_once_every_group_is_used_and_at_or_below_the_tip() {
    let f = fx();
    let first = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.block();
    f.recheck(3).await;
    let second = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!((first.signed.nonce, second.signed.nonce), (3, 4));
    // A not-executed group (its nonce used by another write) too.
    f.plat.mempool.lock().unwrap().clear();
    f.plat.rival(4, None);
    assert_eq!(f.recheck(4).await, Outcome::NotExecuted);
    let third = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(third.signed.nonce, 5);
}

#[tokio::test]
async fn an_undecided_group_whose_nonce_is_taken_signs_again_at_that_nonce() {
    let f = fx();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
    f.recheck(3).await;
    f.plat.calm();
    mark(&f.scene, &row.signed.id, WithdrawalMark::NotReceived)
        .await
        .unwrap();
    let again = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(again.signed.nonce, 3);
    assert_eq!(
        f.scene.db.withdrawal_group(&shown(&f.id), 3).unwrap().len(),
        2
    );
}

#[tokio::test]
async fn a_stale_tip_or_an_undecided_group_above_it_is_refused() {
    // Every nonce read below the records' floor.
    let f = fx();
    f.plat.snapshot();
    f.plat.with(|w| w.height = 20);
    f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.block();
    f.recheck(3).await;
    f.plat.deviate(Read::Nonce, Dev::Behind, ALWAYS);
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_pending");
    assert_eq!(f.signatures(), 1);
    // One read below the floor (where 3 still looks taken) is ignored and
    // made again: tip + 1, with no alarm (P5).
    f.plat.deviate(Read::Nonce, Dev::Behind, 1);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(row.signed.nonce, 4);
    assert!(f.scene.alarms().is_empty());
    // An undecided group Drive doesn't take, above the tip: neither.
    let f = fx();
    f.crafted(40, WithdrawalState::MarkedNotReceived);
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_pending");
    assert_eq!(f.signatures(), 0);
}

#[tokio::test]
async fn a_final_group_whose_nonce_drive_still_takes_refuses_tip_plus_one_and_alarms() {
    let f = fx();
    f.crafted(3, WithdrawalState::Done);
    // 3 is in the missing window: at or below the tip 4, and takeable.
    f.plat.rival(4, None);
    assert!(takes(f.plat.with(|w| w.nonce_state()), 3, f.id));
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_pending");
    assert_eq!(f.signatures(), 0);
    let alarms = f.scene.alarms();
    assert_eq!((alarms.len(), alarms[0].nonce), (1, 3));
}

// ---------------------------------------------------------------------------
// P6: the open set and compare-and-set.

#[tokio::test]
async fn withdraw_is_refused_while_any_record_is_open_and_only_then() {
    use WithdrawalState::*;
    for s in [Signed, Sent, Refused, CouldntConfirm] {
        let f = fx();
        f.crafted(3, s);
        let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
        assert_eq!(e.code(), "masternodes.withdrawal_pending", "{s:?}");
        assert_eq!(f.signatures(), 0, "{s:?}");
    }
    for s in [MarkedReceived, MarkedNotReceived, NotExecuted] {
        let f = fx();
        f.crafted(3, s);
        if s == NotExecuted {
            f.plat.rival(3, None);
        }
        f.withdraw(Payout, AMOUNT)
            .await
            .unwrap_or_else(|e| panic!("{s:?}: {e}"));
    }
}

#[tokio::test]
async fn a_mark_racing_a_proof_leaves_the_proof_and_the_mark() {
    let f = fx();
    f.plat.answer(Answer::Unknown("lost".into()), false);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
    f.recheck(3).await;
    f.plat.calm();
    assert!(f.plat.execute(&row.signed.signed_bytes));
    // The user marks while the recheck reads the document.
    let (db, id) = (Arc::clone(&f.scene.db), row.signed.id.clone());
    *f.plat.hook.lock().unwrap() = Some((
        Read::Document,
        Box::new(move || {
            assert!(
                db.mark_withdrawal_durable(&id, WithdrawalMark::NotReceived, 5)
                    .unwrap()
            );
        }),
    ));
    assert_eq!(f.recheck(3).await, Outcome::Done);
    let done = f.row(&row.signed.id);
    assert_eq!(done.state, WithdrawalState::Done);
    assert_eq!(done.mark.map(|(m, _)| m), Some(WithdrawalMark::NotReceived));
}

#[tokio::test]
async fn two_rechecks_decide_a_group_once_with_or_without_the_lock() {
    for unlocked in [false, true] {
        let f = Fx::new(pro_tx(0x50 + unlocked as u8));
        if unlocked {
            UNLOCKED.lock().unwrap().push(f.id);
        }
        f.plat.answer(Answer::Accepted, true);
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        f.plat.block();
        let (a, b) = tokio::join!(f.recheck(3), f.recheck(3));
        let mut outcomes = [a, b];
        outcomes.sort_by_key(|o| format!("{o:?}"));
        assert_eq!(
            outcomes,
            [Outcome::Done, Outcome::Final],
            "unlocked {unlocked}"
        );
        assert_eq!(f.row(&row.signed.id).state, WithdrawalState::Done);
        UNLOCKED.lock().unwrap().retain(|id| *id != f.id);
    }
}

// ---------------------------------------------------------------------------
// Concurrency and storage.

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_sessions_on_one_app_db_racing_one_identity_sign_once() {
    let f = fx();
    let other = Scene::open(&f.scene.path);
    let (req, owner) = (f.req(Payout, AMOUNT), f.req(Owner, AMOUNT));
    let (net, keys) = (&*f.plat, &f.keys);
    let (a, b) = tokio::join!(
        withdraw(&f.scene, net, keys, f.auth(), Network::Regtest, &req),
        withdraw(&other, net, keys, f.auth(), Network::Regtest, &owner),
    );
    let refused = [&a, &b].into_iter().filter(|r| r.is_err()).count();
    assert_eq!(refused, 1);
    let e = a.err().or(b.err()).unwrap();
    assert_eq!(e.code(), "masternodes.withdrawal_pending");
    assert_eq!(f.signatures(), 1, "the loser signed nothing");
    assert_eq!(f.rows().len(), 1);
    assert_eq!(f.plat.sent().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_unique_index_alone_refuses_the_second_open_record() {
    let f = Fx::new(pro_tx(0x60));
    UNLOCKED.lock().unwrap().push(f.id);
    let other = Scene::open(&f.scene.path);
    let meet = Arc::new(tokio::sync::Barrier::new(2));
    let auth = Auth {
        meet: Some(meet),
        ..f.auth()
    };
    let (req, owner) = (f.req(Payout, AMOUNT), f.req(Owner, AMOUNT));
    let (net, keys) = (&*f.plat, &f.keys);
    let (a, b) = tokio::join!(
        withdraw(&f.scene, net, keys, auth.clone(), Network::Regtest, &req),
        withdraw(&other, net, keys, auth, Network::Regtest, &owner),
    );
    UNLOCKED.lock().unwrap().retain(|id| *id != f.id);
    assert_eq!(f.signatures(), 2, "both passed the open-record check");
    let (won, lost) = match (a, b) {
        (Ok(w), Err(l)) | (Err(l), Ok(w)) => (w, l),
        other => panic!("{other:?}"),
    };
    assert_eq!(lost.code(), "masternodes.withdrawal_pending");
    assert_eq!(f.rows().len(), 1);
    // Only the winner's bytes left.
    assert_eq!(f.plat.sent(), vec![won.signed.signed_bytes]);
}

// ---------------------------------------------------------------------------
// P4: the destination's check.

#[tokio::test]
async fn platform_behind_the_header_tip_is_checking_and_signs_nothing() {
    let f = fx();
    // An outside ProUpRegTx at the tip: SPV's header tip is one block past
    // the chain-locked height Platform committed.
    *f.scene.tip.lock().unwrap() = Some(TIP + 1);
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        Duration::ZERO,
    )
    .await
    .unwrap();
    let checking = Destination::Checking {
        reason: CheckingReason::PlatformBehind,
    };
    assert_eq!(q.owner_destination, Some(checking.clone()));
    assert_eq!(q.payout_destination, Some(checking));
    assert_eq!(q.status.lag, Some(1));
    let e = f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert!(matches!(
        e,
        WithdrawalError::DestinationChecking {
            reason: CheckingReason::PlatformBehind
        }
    ));
    assert_eq!(f.signatures(), 0);
    assert!(f.rows().is_empty());
    // SPV unsynced: nothing to compare with.
    *f.scene.tip.lock().unwrap() = None;
    let e = f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert!(matches!(
        e,
        WithdrawalError::DestinationChecking {
            reason: CheckingReason::SpvUnsynced
        }
    ));
}

#[tokio::test]
async fn condition_a_holds_at_and_above_the_header_tip() {
    for (core, lag) in [(TIP, 0), (TIP + 5, 0)] {
        let f = fx();
        f.plat.with(|w| w.core_height = core);
        let q = quote(
            &f.scene,
            &*f.plat,
            &f.keys,
            Network::Regtest,
            &PRO_TX,
            Duration::ZERO,
        )
        .await
        .unwrap();
        let Some(Destination::Fixed {
            script,
            lag: l,
            core_height,
            ..
        }) = q.owner_destination
        else {
            panic!("{:?}", q.owner_destination);
        };
        assert_eq!((script, l, core_height), (payout_script(), lag, core));
        f.withdraw(Owner, AMOUNT).await.unwrap();
    }
}

#[tokio::test]
async fn the_quote_shows_the_keys_the_claimable_credits_and_the_open_record() {
    let f = fx();
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        Duration::ZERO,
    )
    .await
    .unwrap();
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    assert_eq!(q.owner_key.unwrap().identity_key, 1);
    assert_eq!(q.payout_keys.len(), 1);
    assert_eq!(
        q.claimable,
        rules.credits(BALANCE, WithdrawAmount::All).unwrap()
    );
    assert!(q.expected_duffs.is_some());
    assert!(q.open.is_none());
    assert!(q.status.identity_found);
    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(q.open.unwrap().signed.id, row.signed.id);
}

#[tokio::test]
async fn the_quote_waits_while_platform_catches_up_and_stops_at_its_bound() {
    let f = fx();
    f.plat
        .core_heights
        .lock()
        .unwrap()
        .extend([TIP - 2, TIP - 1]);
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        WAITS.quote,
    )
    .await
    .unwrap();
    assert!(matches!(
        q.owner_destination,
        Some(Destination::Fixed { .. })
    ));
    assert_eq!(f.plat.reads(Read::Identity), 3);
    // Stuck behind: checking, after the bound.
    f.plat.with(|w| w.core_height = TIP - 1);
    let start = tokio::time::Instant::now();
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        WAITS.quote,
    )
    .await
    .unwrap();
    assert!(start.elapsed() >= WAITS.quote - WAITS.poll);
    assert!(matches!(
        q.owner_destination,
        Some(Destination::Checking {
            reason: CheckingReason::PlatformBehind
        })
    ));
}

#[tokio::test]
async fn the_pre_sign_wait_runs_outside_the_identity_lock_and_before_the_grant() {
    // A masternode of its own: no other test holds its lock.
    let f = Fx::new(pro_tx(0x90));
    f.plat
        .core_heights
        .lock()
        .unwrap()
        .extend([TIP - 1, TIP - 1, TIP - 1]);
    f.plat.watch_lock.store(true, Ordering::SeqCst);
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    f.plat.watch_lock.store(false, Ordering::SeqCst);
    // Three reads while Platform is behind and the passing one, all
    // outside the lock; the final look (and the read-back) under it.
    let free = f.plat.lock_free.lock().unwrap().clone();
    assert_eq!(free[..5], [true, true, true, true, false]);
    assert_eq!(row.state, WithdrawalState::Sent);
    assert_eq!(f.signatures(), 1);
    // A wait that runs out signs nothing and takes no grant.
    let f = fx();
    f.plat.with(|w| w.core_height = TIP - 1);
    let start = tokio::time::Instant::now();
    f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert!(start.elapsed() >= WAITS.sign - WAITS.poll);
    assert!(start.elapsed() < WAITS.quote);
    assert_eq!(f.signatures(), 0);
}

#[tokio::test]
async fn the_final_look_under_the_lock_refuses_after_a_passing_wait() {
    let f = fx();
    f.plat.core_heights.lock().unwrap().extend([TIP, TIP - 1]);
    let e = f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert!(matches!(
        e,
        WithdrawalError::DestinationChecking {
            reason: CheckingReason::PlatformBehind
        }
    ));
    assert_eq!(f.signatures(), 0);
    assert!(f.rows().is_empty());
}

// ---------------------------------------------------------------------------
// P12: rule 5 on each gated read.

#[tokio::test]
async fn gated_reads_follow_rule_5_in_each_mode() {
    let read = |verified: bool| {
        Ok::<_, PlatformError>(Proved {
            value: verified,
            height: 1,
            core_height: 1,
            time_ms: 1,
            verified,
        })
    };
    let probe = Duration::from_millis(50);
    for rule in [Rule5::Enforce, Rule5::Probe, Rule5::Off] {
        let g = gated(rule, probe, || async { read(true) }).await.unwrap();
        assert!(g.passed, "verified passes under {rule:?}");
    }
    let g = gated(Rule5::Enforce, probe, || async { read(false) })
        .await
        .unwrap();
    assert!(!g.passed);
    let g = gated(Rule5::Off, probe, || async { read(false) })
        .await
        .unwrap();
    assert!(g.passed);
    // Probe: one more read; verified, that one.
    let n = AtomicUsize::new(0);
    let g = gated(Rule5::Probe, probe, || async {
        read(n.fetch_add(1, Ordering::SeqCst) > 0)
    })
    .await
    .unwrap();
    assert!(g.passed && g.read.verified);
    // Unverified again, or too slow: the first, rule 5 suspended.
    let g = gated(Rule5::Probe, probe, || async { read(false) })
        .await
        .unwrap();
    assert!(g.passed && !g.read.verified);
    let n = AtomicUsize::new(0);
    let g = gated(Rule5::Probe, probe, || async {
        if n.fetch_add(1, Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        read(false)
    })
    .await
    .unwrap();
    assert!(g.passed);
    // A mismatch refuses.
    let n = AtomicUsize::new(0);
    let e = gated(Rule5::Probe, probe, || async {
        match n.fetch_add(1, Ordering::SeqCst) {
            0 => read(false),
            _ => Err(PlatformError::TrustMismatch),
        }
    })
    .await
    .unwrap_err();
    assert_eq!(e, PlatformError::TrustMismatch);
}

#[tokio::test]
async fn an_unverified_identity_read_under_enforce_signs_nothing() {
    for (rule, signs) in [
        (Rule5::Enforce, false),
        (Rule5::Probe, true),
        (Rule5::Off, true),
    ] {
        let f = fx();
        f.scene.rule(rule);
        f.plat.deviate(Read::Identity, Dev::Unverified, ALWAYS);
        let r = f.withdraw(Payout, AMOUNT).await;
        assert_eq!(r.is_ok(), signs, "{rule:?}: {r:?}");
        if !signs {
            assert!(matches!(
                r.unwrap_err(),
                WithdrawalError::DestinationChecking {
                    reason: CheckingReason::Unverified
                }
            ));
            assert_eq!(f.signatures(), 0);
        }
    }
    // Probe with SPV-verified second reads: passes on them.
    let f = fx();
    f.scene.rule(Rule5::Probe);
    f.plat.deviate(Read::Identity, Dev::Unverified, 1);
    f.withdraw(Payout, AMOUNT).await.unwrap();
    // Probe and a mismatch: refused.
    let f = fx();
    f.scene.rule(Rule5::Probe);
    f.plat.deviate(Read::Identity, Dev::Unverified, 1);
    f.plat.deviate(Read::Identity, Dev::Mismatch, 1);
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert_eq!(e.code(), PlatformError::TrustMismatch.code());
    assert_eq!(f.signatures(), 0);
}

#[tokio::test]
async fn an_unverified_nonce_read_under_enforce_signs_nothing_at_any_nonce() {
    // The initial (tip + 1), the same-nonce (after a mark) and the next
    // tip + 1 (after a done group).
    for case in 0..3 {
        for rule in [Rule5::Enforce, Rule5::Probe, Rule5::Off] {
            let f = fx();
            match case {
                0 => {}
                1 => {
                    f.plat.answer(Answer::Unknown("lost".into()), false);
                    let row = f.withdraw(Payout, AMOUNT).await.unwrap();
                    f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
                    f.recheck(3).await;
                    f.plat.calm();
                    mark(&f.scene, &row.signed.id, WithdrawalMark::Received)
                        .await
                        .unwrap();
                }
                _ => {
                    f.withdraw(Payout, AMOUNT).await.unwrap();
                    f.plat.block();
                    f.recheck(3).await;
                }
            }
            let before = f.signatures();
            f.scene.rule(rule);
            f.plat.deviate(Read::Nonce, Dev::Unverified, ALWAYS);
            let r = f.withdraw(Payout, AMOUNT).await;
            match rule {
                Rule5::Enforce => {
                    assert_eq!(r.unwrap_err().code(), "masternodes.withdrawal_unverified");
                    assert_eq!(f.signatures(), before, "case {case}");
                }
                _ => {
                    let row = r.unwrap_or_else(|e| panic!("case {case} {rule:?}: {e}"));
                    // An unverified read sets no floor: the new group's is
                    // 0, the same-nonce group keeps its verified 10.
                    let floor = if case == 1 { 10 } else { 0 };
                    assert_eq!(row.floor_height, floor, "case {case} {rule:?}");
                }
            }
        }
    }
}

#[tokio::test]
async fn an_unverified_positive_match_under_enforce_decides_nothing() {
    for (rule, done) in [
        (Rule5::Enforce, false),
        (Rule5::Probe, true),
        (Rule5::Off, true),
    ] {
        let f = fx();
        let row = f.withdraw(Payout, AMOUNT).await.unwrap();
        f.plat.block();
        f.scene.rule(rule);
        f.plat.deviate(Read::Document, Dev::Unverified, ALWAYS);
        let outcome = f.recheck(3).await;
        assert_eq!(outcome == Outcome::Done, done, "{rule:?}: {outcome:?}");
        assert_eq!(f.row(&row.signed.id).state == WithdrawalState::Done, done);
    }
}

#[tokio::test]
async fn an_unverified_non_match_or_nonce_read_under_enforce_decides_no_not_executed() {
    for read in [Read::Document, Read::Nonce] {
        for (rule, decided) in [(Rule5::Enforce, false), (Rule5::Off, true)] {
            let f = fx();
            f.plat.answer(Answer::Unknown("lost".into()), false);
            let row = f.withdraw(Payout, AMOUNT).await.unwrap();
            f.plat.rival(3, Some((&payout_script(), AMOUNT + 1)));
            f.scene.rule(rule);
            f.plat.deviate(read, Dev::Unverified, ALWAYS);
            let outcome = f.recheck(3).await;
            assert_eq!(
                outcome == Outcome::NotExecuted,
                decided,
                "{read:?} {rule:?}: {outcome:?}"
            );
            if !decided {
                assert!(matches!(outcome, Outcome::CouldntConfirm(_)));
                assert_eq!(f.row(&row.signed.id).state, WithdrawalState::CouldntConfirm);
            }
        }
    }
}

#[tokio::test]
async fn an_unverified_identity_read_under_enforce_decides_no_owner_key_not_executed() {
    for (rule, decided) in [(Rule5::Enforce, false), (Rule5::Off, true)] {
        let f = fx();
        f.plat.answer(Answer::Unknown("lost".into()), false);
        f.withdraw(Owner, AMOUNT).await.unwrap();
        f.plat.rival(3, None);
        f.scene.rule(rule);
        f.plat.deviate(Read::Identity, Dev::Unverified, ALWAYS);
        let outcome = f.recheck(3).await;
        assert_eq!(
            outcome == Outcome::NotExecuted,
            decided,
            "{rule:?}: {outcome:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// P16: the payout-change barriers' clearing.

#[tokio::test]
async fn a_barrier_of_an_unreadable_operation_blocks_until_platform_evidence() {
    // Its creating wallet deleted or not loaded: no operation state.
    let f = fx();
    let moved = CoreScript::new_p2pkh([0x22; 20]);
    f.barrier(&PRO_TX, "op", moved.as_bytes(), 1_000);
    let pending = Destination::Checking {
        reason: CheckingReason::PayoutChangePending,
    };
    let q = quote(
        &f.scene,
        &*f.plat,
        &f.keys,
        Network::Regtest,
        &PRO_TX,
        Duration::ZERO,
    )
    .await
    .unwrap();
    assert_eq!(q.owner_destination, Some(pending));
    assert!(matches!(
        f.withdraw(Owner, AMOUNT).await.unwrap_err(),
        WithdrawalError::DestinationChecking {
            reason: CheckingReason::PayoutChangePending
        }
    ));
    assert_eq!((f.signatures(), f.barriers()), (0, 1));
    // Platform applied the change: its window pays the new script, read
    // after the barrier.
    f.plat.identity_mut(|i| {
        i.public_keys_mut().insert(0, disabled(payout_key(0)));
        i.public_keys_mut().insert(2, other_key(2, 0x22));
    });
    let row = f.withdraw(Owner, AMOUNT).await.unwrap();
    assert_eq!(row.signed.expected_script, Some(moved.as_bytes().to_vec()));
    assert_eq!(f.barriers(), 0);
}

#[tokio::test]
async fn platform_evidence_counts_only_read_after_the_barrier_and_passing_rule_5() {
    // A read from before the barrier was written.
    let f = fx();
    f.barrier(&PRO_TX, "op", payout_script().as_bytes(), 5_000);
    f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert_eq!(f.barriers(), 1);
    // After it, but unverified under Enforce.
    let f = fx();
    f.barrier(&PRO_TX, "op", payout_script().as_bytes(), 1_000);
    f.plat.deviate(Read::Identity, Dev::Unverified, ALWAYS);
    f.withdraw(Owner, AMOUNT).await.unwrap_err();
    assert_eq!(f.barriers(), 1);
    // Under Off, the same read clears it.
    f.scene.rule(Rule5::Off);
    f.withdraw(Owner, AMOUNT).await.unwrap();
    assert_eq!(f.barriers(), 0);
}

#[tokio::test]
async fn a_conflicted_or_a_confirmed_and_covered_operation_clears_its_barrier() {
    let moved = CoreScript::new_p2pkh([0x22; 20]);
    for (op, clears) in [
        (ProTxOperationState::Conflicted, true),
        (ProTxOperationState::Confirmed { height: TIP }, true),
        (ProTxOperationState::Confirmed { height: TIP + 1 }, false),
        (ProTxOperationState::Sent, false),
    ] {
        let f = fx();
        *f.scene.tip.lock().unwrap() = Some(TIP);
        f.barrier(&PRO_TX, "op", moved.as_bytes(), 1_000);
        f.scene.ops.lock().unwrap().insert("op".into(), op);
        let r = f.withdraw(Payout, AMOUNT).await;
        assert_eq!(r.is_ok(), clears, "{op:?}: {r:?}");
        assert_eq!(f.barriers(), usize::from(!clears), "{op:?}");
    }
}

#[tokio::test]
async fn barriers_not_backfilled_read_as_a_pending_payout_change() {
    let f = fx();
    f.scene.ready.store(false, Ordering::SeqCst);
    let e = f.withdraw(Payout, AMOUNT).await.unwrap_err();
    assert!(matches!(
        e,
        WithdrawalError::DestinationChecking {
            reason: CheckingReason::PayoutChangePending
        }
    ));
    assert_eq!(f.signatures(), 0);
}

#[tokio::test]
async fn another_masternodes_barrier_does_not_block() {
    let f = fx();
    f.barrier(&pro_tx(0x70), "op", &[9], 1_000);
    f.withdraw(Payout, AMOUNT).await.unwrap();
    assert_eq!(
        f.scene
            .db
            .payout_barriers_of(&display_hex(&pro_tx(0x70)))
            .unwrap()
            .len(),
        1
    );
}

// ---------------------------------------------------------------------------
// The lease path on a real session (P3, P12).

#[test]
fn the_lease_grant_charges_the_credits_and_signs_once() {
    use crate::platform::lease::session_tests::fixture;

    let f = fixture(false);
    let session = Arc::clone(&f.session);
    f.session
        .vault()
        .store_masternode_key(&PRO_TX, "owner", &OWNER_SECRET)
        .unwrap();
    let rules = Rules::at(PlatformVersion::latest()).unwrap();
    let transition = |amount| {
        StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(
            IdentityCreditWithdrawalTransitionV1 {
                identity_id: identity_of(&PRO_TX),
                amount,
                core_fee_per_byte: MIN_CORE_FEE_PER_BYTE,
                pooling: Pooling::Never,
                output_script: None,
                nonce: 3,
                user_fee_increase: 0,
                signature_public_key_id: 1,
                signature: Default::default(),
            },
        ))
    };
    let grant = |max| {
        f.grant(
            GrantPurpose::MasternodeWithdraw {
                max_credits: max,
                destination: None,
            },
            Credential::None,
        )
    };
    let key = WithdrawalKey::AttachedOwner {
        pro_tx_hash: PRO_TX,
    };
    let sign = |grant: &str, credits, amount, identity_key: &IdentityPublicKey| {
        let auth = super::session::LeaseGrant {
            session: Arc::clone(&session),
            wallet: f.wallet,
            grant: grant.to_owned(),
        };
        let (key, identity_key, t) = (key.clone(), identity_key.clone(), transition(amount));
        f.engine
            .block_on(async move { auth.sign(credits, &key, &identity_key, &t, PRO_TX).await })
    };
    let charge = AMOUNT + rules.fee;
    let g = grant(charge);
    let sig = sign(&g, charge, AMOUNT, &owner_key()).unwrap();
    let public = PublicKey::from_secret_key(&SecretKey::from_secret_bytes(OWNER_SECRET).unwrap());
    dpp::dashcore::signer::verify_data_signature(
        &transition(AMOUNT).signable_bytes().unwrap(),
        &sig,
        &public.serialize(),
    )
    .unwrap();
    // The grant was redeemed: once.
    assert!(sign(&g, charge, AMOUNT, &owner_key()).is_err());
    // A charge past the grant's credits signs nothing.
    let g = grant(AMOUNT);
    assert!(sign(&g, charge, AMOUNT, &owner_key()).is_err());
    // The wrong key: `withdrawal_key_missing`, nothing signed.
    let g = grant(charge);
    let wrong = key_with(owner_key(), &[1; 20]);
    let e = sign(&g, charge, AMOUNT, &wrong).unwrap_err();
    assert_eq!(e.code(), "masternodes.withdrawal_key_missing");
}

fn key_with(k: IdentityPublicKey, data: &[u8]) -> IdentityPublicKey {
    let IdentityPublicKey::V0(mut v) = k else {
        panic!("a V0 key");
    };
    v.data = data.to_vec().into();
    IdentityPublicKey::V0(v)
}

// ---------------------------------------------------------------------------
// The fault sweep (MG-13 §3, DEC-998).

/// The dw-appdb points no withdrawal step writes: MG-04's barrier write and
/// discard (commit 3) and the open's backfill (commit 2) have their own
/// sweeps there.
const NOT_IN_A_WITHDRAWAL: [&str; 3] = [
    dw_appdb::fault::MN_BARRIER_WRITE,
    dw_appdb::fault::MN_BARRIER_DISCARD,
    dw_appdb::fault::MN_BARRIER_BACKFILL,
];

/// One step of the sweep's script; its error is the fault's or the state's
/// after one.
async fn step(f: &Fx, n: usize) -> Result<(), WithdrawalError> {
    let net = &*f.plat;
    match n {
        // A barrier the look clears (its operation conflicted); a payout-key
        // withdrawal refused on its merits.
        0 => {
            f.plat.answer(Answer::Refused("10001 merits".into()), false);
            f.withdraw(Payout, AMOUNT).await.map(drop)
        }
        // Try again with the owner key: taken into a mempool.
        1 => {
            let refused = (f.rows().into_iter())
                .find(|r| r.state == WithdrawalState::Refused)
                .ok_or(WithdrawalError::RetryState)?;
            f.retry(&refused.signed.id, Owner, AMOUNT).await.map(drop)
        }
        // The recheck: the nonce taken, the open sibling sent again, the
        // floor raised.
        2 => recheck_all(&f.scene, net).await.map(drop),
        // A block; the recheck proves it.
        3 => {
            f.plat.block();
            recheck_all(&f.scene, net).await.map(drop)
        }
        // A withdrawal at tip + 1 that is lost; a rival at its nonce with
        // another amount; not executed.
        4 => {
            f.plat.answer(Answer::Unknown("lost".into()), false);
            f.withdraw(Payout, AMOUNT).await.map(drop)
        }
        5 => {
            let n = f.plat.with(|w| w.tip()) + 1;
            let open = f.rows().into_iter().find(|r| r.state.is_open());
            if let Some(open) = open.filter(|r| r.signed.nonce == n) {
                f.plat
                    .rival(open.signed.nonce, Some((&payout_script(), AMOUNT + 1)));
            }
            recheck_all(&f.scene, net).await.map(drop)
        }
        // An owner-key withdrawal that is lost; every document read fails:
        // couldnt_confirm, then its note; the user's mark.
        6 => {
            f.plat.answer(Answer::Unknown("lost".into()), false);
            f.withdraw(Owner, AMOUNT).await.map(drop)
        }
        7 | 8 => {
            f.plat.deviate(Read::Document, Dev::Fail, ALWAYS);
            let r = recheck_all(&f.scene, net).await.map(drop);
            f.plat.calm();
            r
        }
        9 => {
            let row = (f.rows().into_iter())
                .find(|r| r.state == WithdrawalState::CouldntConfirm)
                .ok_or(WithdrawalError::MarkState)?;
            mark(&f.scene, &row.signed.id, WithdrawalMark::Received)
                .await
                .map(drop)
        }
        // Its bytes land late: the proof and the late link.
        10 => {
            for row in f.rows().into_iter().filter(|r| !r.state.is_final()) {
                f.plat.execute(&row.signed.signed_bytes);
            }
            recheck_all(&f.scene, net).await.map(drop)
        }
        _ => unreachable!(),
    }
}

const STEPS: usize = 11;

/// The five invariants after a restart and the settling rechecks.
fn invariants(f: &Fx, what: &str) {
    let rows = f.rows();
    let world = f.plat.with(|w| w.clone());
    let state = world.nonce_state();
    let tip = world.tip();
    // 1. No transition at another nonce while an earlier group is
    //    undecided and its nonce takeable or above the tip.
    let nonces: BTreeSet<u64> = rows.iter().map(|r| r.signed.nonce).collect();
    for &n in &nonces {
        let undecided = (rows.iter()).any(|r| r.signed.nonce == n && !r.state.is_final());
        if undecided && (takes(state, n, f.id) || n > tip) {
            assert!(
                nonces.iter().all(|&m| m <= n),
                "{what}: signed past undecided {n}"
            );
        }
    }
    // 2. Stored bytes unchanged: everything sent is a record's bytes, and
    //    each record's bytes are its transition.
    for sent in f.plat.sent() {
        assert!(
            rows.iter().any(|r| r.signed.signed_bytes == sent),
            "{what}: foreign bytes"
        );
    }
    for r in &rows {
        let t = decoded(&r.signed.signed_bytes);
        assert_eq!(
            (t.nonce, t.amount),
            (r.signed.nonce, r.signed.amount),
            "{what}"
        );
        let st =
            StateTransition::IdentityCreditWithdrawal(IdentityCreditWithdrawalTransition::V1(t));
        assert_eq!(
            r.signed.transition_hash,
            hex::encode(st.transaction_id().unwrap()),
            "{what}"
        );
    }
    // 3. No done without a matching proved document.
    for r in rows.iter().filter(|r| r.state == WithdrawalState::Done) {
        let proof = r.proof.as_ref().unwrap();
        let paid = CoreScript::from_bytes(r.paid_script.clone().unwrap());
        let doc = (world.documents.iter())
            .find(|d| shown(&d.id()) == proof.document_id)
            .unwrap_or_else(|| panic!("{what}: done without a document"));
        assert!(matches(doc, r, &f.id, &paid), "{what}: done on a non-match");
    }
    // 4. No not_executed without P8's four conditions.
    for r in rows
        .iter()
        .filter(|r| r.state == WithdrawalState::NotExecuted)
    {
        let n = r.signed.nonce;
        assert!(n <= tip && !takes(state, n, f.id), "{what}: (i)");
        assert!(r.evidence.is_some(), "{what}: evidence");
        for script in candidates(r, world.identity.as_ref()) {
            let id = document_id(&f.id, n, &script);
            let matched =
                (world.documents.iter()).any(|d| d.id() == id && matches(d, r, &f.id, &script));
            assert!(!matched, "{what}: not executed with a matching document");
        }
    }
    // 5. No barrier lost: another masternode's stays; this one's went only
    //    by its term (conflicted).
    let other = f
        .scene
        .db
        .payout_barriers_of(&display_hex(&pro_tx(0x70)))
        .unwrap();
    assert_eq!(other.len(), 1, "{what}: another masternode's barrier");
}

/// The script, with `armed` armed; the restart after the step that hit it.
/// Whether it was hit.
async fn sweep_run(armed: Option<(&str, dw_appdb::fault::Stage)>) -> bool {
    let mut f = Fx::new(pro_tx(0x80));
    f.barrier(&f.pro_tx, "mine", &[7], 1_000);
    f.scene
        .ops
        .lock()
        .unwrap()
        .insert("mine".into(), ProTxOperationState::Conflicted);
    f.barrier(&pro_tx(0x70), "other", &[9], 1_000);
    let path = f.scene.path.clone();
    let what = format!("{armed:?}");
    let mut hit = false;
    if let Some((point, stage)) = armed {
        dw_appdb::fault::arm(&path, point, stage);
    }
    for n in 0..STEPS {
        let r = step(&f, n).await;
        if let (Some((point, stage)), false) = (armed, hit) {
            // Still armed: re-armed by the probe's hit; else hit here.
            match dw_appdb::fault::hit(&path, point, stage) {
                Err(_) => dw_appdb::fault::arm(&path, point, stage),
                Ok(()) => {
                    hit = true;
                    f.restart();
                    if let Err(e) = &r {
                        assert!(e.to_string().contains("fault"), "{what}: {e}");
                    }
                }
            }
        } else if let Err(e) = r {
            // Without a fault, only the script's own state refusals.
            assert!(
                !matches!(e, WithdrawalError::Engine(_)),
                "{what} step {n}: {e}"
            );
        }
    }
    dw_appdb::fault::disarm(&path);
    // Settle: every mempool to a block, every group rechecked.
    for _ in 0..2 {
        f.plat.block();
        recheck_all(&f.scene, &*f.plat).await.unwrap();
    }
    invariants(&f, &what);
    if armed.is_none() {
        use WithdrawalState::*;
        let states: Vec<_> = f.rows().iter().map(state).collect();
        assert_eq!(
            states,
            [SiblingPaid, Done, NotExecuted, Done],
            "the script's path"
        );
        assert!(f.rows()[3].late_proof_link.is_some());
    }
    hit
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_fault_sweep_keeps_the_five_invariants_at_every_point() {
    use dw_appdb::fault::Stage;
    assert!(!sweep_run(None).await);
    let mut missed = Vec::new();
    let engine = points::ALL.iter();
    let appdb = (dw_appdb::fault::POINTS.iter()).filter(|p| !NOT_IN_A_WITHDRAWAL.contains(p));
    for point in engine.chain(appdb) {
        for stage in [Stage::Before, Stage::After] {
            if !sweep_run(Some((point, stage))).await {
                missed.push(dw_appdb::fault::name(point, stage));
            }
        }
    }
    assert!(
        missed.is_empty(),
        "points the script never reached: {missed:?}"
    );
}
