//! CoinJoin in the engine (M3, owner R1; docs/contracts/m3-engine.md §2.1):
//! per-wallet mixing over `dw-coinjoin`, the network's CoinJoin options,
//! the per-wallet salt, the `CoinJoin` event, the IOS-057 recovery scan and
//! "move mixed coins".
//!
//! Mixing follows Dash Core's client (`src/coinjoin/client.cpp`):
//! - a per-wallet loop ticks every second (`DoMaintenance`, client.cpp:2439)
//!   and runs the automatic denominating step every 5–15 ticks
//!   (`COINJOIN_AUTO_TIMEOUT_MIN/MAX`, coinjoin.h:45-46);
//! - the step creates denominations and collateral inputs from the
//!   wallet's own coins when needed, then joins a known queue or starts a
//!   new one with a random unused masternode (`DoAutomaticDenominating`,
//!   client.cpp:962-1212);
//! - each session runs [`dw_coinjoin::client::run_session`] against the
//!   masternode; this module implements its [`MixingWallet`].
//!
//! Outputs: denominations, collaterals and mixing outputs go to the DIP9
//! CoinJoin account `m/9'/coin'/4'/0'` (DESIGN.md R2). Mixing inputs and
//! collaterals are signed with the vault's mixing signer (CoinJoin account
//! only); denomination and collateral transactions made from BIP44 coins
//! with its funding signer (the same wallet's BIP44 and CoinJoin keys,
//! outputs only to the wallet).
//!
//! SPV approximations (DESIGN-opus §1.14, m3-engine.md §7): the
//! masternode-payment "skip winners" check (client.cpp:1414-1418) needs
//! `nLastPaidHeight`, which the simplified list does not carry, so it is
//! not applied; our collateral counts as valid when it is confirmed or
//! InstantSend-locked, unspent in our view and of the right amount; foreign
//! inputs of a final transaction are counted, not checked against a UTXO
//! set.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use dashcore::blockdata::transaction::outpoint::OutPoint;
use dashcore::hashes::Hash;
use dashcore::sighash::SighashCache;
use dashcore::{Address, ScriptBuf, Transaction, TxIn, TxOut, Txid};
use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE};
use dw_coinjoin::client::{
    MixCoin, MixingWallet, SessionOutcome, SessionParams, SessionProgress, WalletError,
    choose_session_denom, run_session, select_by_denomination, select_denominated_amounts,
};
use dw_coinjoin::denoms::{
    COLLATERAL_AMOUNT, DENOMINATIONS, MIN_MIXING_BALANCE, SMALLEST_DENOMINATION, is_denominated,
};
use dw_coinjoin::messages::{Queue, denomination_to_amount};
use dw_coinjoin::planner::{self, Tally, TxPlan};
use dw_coinjoin::progress::{self, ProgressInputs};
use dw_coinjoin::queue::{QueueManager, QueueMasternode};
use dw_coinjoin::rounds::{self, RoundsCalculator, WalletTx, is_collateral_amount};
pub use dw_coinjoin::settings::CoinJoinSettings;
pub use dw_coinjoin::status::{PoolMessage, PoolState, StatusCode};
use dw_p2p::{PeerEntry, PeerPicker, Session, SessionConfig};
use dw_vault::{LockState, VaultError, VaultSigner, is_bip44_path, is_coinjoin_path};
use key_wallet::DerivationPath;
use key_wallet::managed_account::address_pool::KeySource;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::managed_account_type::ManagedAccountType;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::{Signer, Utxo};
use rand::seq::SliceRandom;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::coins::{WalletCoin, now_secs};
use crate::{EngineError, EngineEvent, NetworkSession, WalletId};

/// Why a CoinJoin call failed (`coinjoin.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoinJoinFailure {
    /// CoinJoin features are turned off (`enablecoinjoin` false).
    Disabled,
    /// The wallet has no private keys.
    WatchOnly,
    /// The balance is below the mixing minimum (0.00140001 DASH);
    /// `min_duffs` is that minimum ("CoinJoin requires at least %2 to use").
    InsufficientFunds {
        min_duffs: u64,
    },
    /// The vault is locked: unlock it for mixing only (or fully) first.
    VaultLocked,
    /// The grant is unknown, expired or of another purpose.
    GrantInvalid,
    /// "Move mixed coins": nothing above the 1000-duff threshold to move.
    NothingToMove,
    /// The recovery scan needs a running SPV client.
    SpvNotRunning,
    NoPeers,
    /// A peer rejected a sweep transaction; the reason is Core's.
    BroadcastRejected(String),
}

impl std::fmt::Display for CoinJoinFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => f.write_str("coinjoin is disabled"),
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::InsufficientFunds { min_duffs } => {
                write!(f, "balance below the mixing minimum of {min_duffs} duffs")
            }
            Self::VaultLocked => f.write_str("vault locked"),
            Self::GrantInvalid => f.write_str("grant invalid"),
            Self::NothingToMove => f.write_str("no mixed coins to move"),
            Self::SpvNotRunning => f.write_str("spv is not running"),
            Self::NoPeers => f.write_str("no connected peers"),
            Self::BroadcastRejected(r) => write!(f, "broadcast rejected: {r}"),
        }
    }
}

impl From<CoinJoinFailure> for EngineError {
    fn from(f: CoinJoinFailure) -> Self {
        EngineError::CoinJoin(f)
    }
}

// ---------------------------------------------------------------------------
// Public status types (mapped one to one by dw-ffi `coinjoin.rs`)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MixingState {
    Idle,
    Mixing,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StopReason {
    UserRequested,
    VaultLocked,
    WalletUnloaded,
    SessionClosed,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unavailable {
    Disabled,
    WatchOnly,
    InsufficientFunds { min_duffs: u64 },
}

/// Core's balance names, duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoinJoinBalances {
    pub anonymizable: u64,
    pub denominated: u64,
    pub normalized_anonymized: u64,
    pub fully_mixed: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ProgressInfo {
    pub overall_percent: f64,
    pub denominated_percent: f64,
    pub partially_mixed_percent: f64,
    pub mixed_percent: f64,
    pub average_rounds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AmountAndRounds {
    pub amount: u64,
    pub rounds: u32,
    pub insufficient_inputs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    /// proTxHash in display order.
    pub pro_tx_hash: Option<String>,
    pub service: Option<String>,
    pub denomination: Option<u64>,
    pub state: PoolState,
    pub entries: u32,
    pub last_message: Option<PoolMessage>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CoinJoinStatus {
    pub wallet_id: WalletId,
    pub state: MixingState,
    pub stop_reason: Option<StopReason>,
    pub unavailable: Option<Unavailable>,
    pub balances: CoinJoinBalances,
    pub progress: ProgressInfo,
    pub amount_and_rounds: AmountAndRounds,
    pub submitted_denominations: Vec<u64>,
    pub sessions: Vec<SessionInfo>,
    pub status: StatusCode,
    pub queue_size: u32,
    /// HD wallets have no keypool: always `None`.
    pub keys_left: Option<u32>,
}

/// IOS-057 recovery scan result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub coinjoin_addresses_scanned: u32,
    pub bip44_addresses_scanned: u32,
    pub coinjoin_balance: u64,
    pub new_transactions: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SweepDestination {
    Wallet,
    Shielded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepChunkInfo {
    pub inputs: u32,
    pub amount: u64,
    pub fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepPlan {
    pub destination: SweepDestination,
    pub total: u64,
    pub chunks: Vec<SweepChunkInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepResult {
    pub txids: Vec<String>,
    pub moved: u64,
    pub remaining: u64,
    pub failure_code: Option<String>,
}

// ---------------------------------------------------------------------------
// Settings and salt persistence (app.sqlite)
// ---------------------------------------------------------------------------

/// Network-wide options (`GLOBAL_SCOPE`).
const SETTINGS_KEY: &str = "coinjoin.settings";
/// Per-wallet salt, display hex (`coinjoinsalt get`).
const SALT_KEY: &str = "coinjoin.salt";
/// The restore lookahead a recovery scan raises the accounts to.
pub const RECOVERY_LOOKAHEAD: u32 = 1_000;
/// How long inputs of a successful session stay locked waiting for the
/// final transaction (`COINJOIN_PENDING_OBSERVATION_TIMEOUT`, coinjoin.h:56).
const PENDING_OBSERVATION_SECS: u64 = 60 * 60;
/// dash-qt's 1 s panel timer: status refresh and event rate.
const TICK: Duration = Duration::from_secs(1);

fn encode_settings(s: &CoinJoinSettings) -> String {
    format!(
        "enabled={};multi={};sessions={};rounds={};amount={};goal={};cap={}",
        u8::from(s.enabled),
        u8::from(s.multi_session),
        s.max_sessions,
        s.rounds,
        s.target_amount_dash,
        s.denoms_goal,
        s.denoms_hard_cap
    )
}

fn decode_settings(text: &str) -> Option<CoinJoinSettings> {
    let mut s = CoinJoinSettings::default();
    for part in text.split(';') {
        let (k, v) = part.split_once('=')?;
        let n: u32 = v.parse().ok()?;
        match k {
            "enabled" => s.enabled = n != 0,
            "multi" => s.multi_session = n != 0,
            "sessions" => s.max_sessions = n,
            "rounds" => s.rounds = n,
            "amount" => s.target_amount_dash = n,
            "goal" => s.denoms_goal = n,
            "cap" => s.denoms_hard_cap = n,
            _ => {}
        }
    }
    s.validate().ok()?;
    Some(s)
}

/// Reads the stored options (defaults when none or unreadable).
pub(crate) fn load_settings(db: &AppDb) -> CoinJoinSettings {
    match db.setting(GLOBAL_SCOPE, SETTINGS_KEY) {
        Ok(Some(text)) => decode_settings(&text).unwrap_or_else(|| {
            tracing::warn!(%text, "ignoring unreadable CoinJoin settings");
            CoinJoinSettings::default()
        }),
        Ok(None) => CoinJoinSettings::default(),
        Err(e) => {
            tracing::warn!(error = %e, "could not read CoinJoin settings; using defaults");
            CoinJoinSettings::default()
        }
    }
}

fn random_salt() -> [u8; 32] {
    rand::random()
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

/// One running session of a wallet.
struct SessionSlot {
    masternode: PeerEntry,
    progress: SessionProgress,
    stop: watch::Sender<bool>,
}

/// Mixing state of one wallet.
#[derive(Default)]
struct Mixer {
    mixing: bool,
    stopping: bool,
    stop_reason: Option<StopReason>,
    /// `strAutoDenomResult`.
    status: Option<StatusCode>,
    sessions: BTreeMap<u64, SessionSlot>,
    next_session: u64,
    /// Wallet height of the last successful step (`nCachedLastSuccessBlock`).
    last_success_height: u32,
    stop: Option<watch::Sender<bool>>,
    task: Option<JoinHandle<()>>,
    /// Inputs of successful sessions waiting for the final transaction, with
    /// the time they were added.
    held: HashMap<OutPoint, u64>,
    /// Collateral inputs locked for open sessions.
    collateral_inputs: HashMap<u64, Vec<OutPoint>>,
}

/// Per-session CoinJoin state (one per [`NetworkSession`]).
pub(crate) struct CoinJoinRuntime {
    settings: RwLock<CoinJoinSettings>,
    mixers: Mutex<HashMap<WalletId, Mixer>>,
    status: RwLock<HashMap<WalletId, CoinJoinStatus>>,
    queues: Mutex<QueueManager>,
    picker: Mutex<PeerPicker>,
    /// Wallets whose status a host asked for recently (UNIX seconds).
    watched: Mutex<HashMap<WalletId, u64>>,
    refresher: Mutex<Option<JoinHandle<()>>>,
    listener: Mutex<Option<JoinHandle<()>>>,
}

impl CoinJoinRuntime {
    pub(crate) fn new(settings: CoinJoinSettings) -> Self {
        Self {
            settings: RwLock::new(settings),
            mixers: Mutex::new(HashMap::new()),
            status: RwLock::new(HashMap::new()),
            queues: Mutex::new(QueueManager::default()),
            picker: Mutex::new(PeerPicker::default()),
            watched: Mutex::new(HashMap::new()),
            refresher: Mutex::new(None),
            listener: Mutex::new(None),
        }
    }

    fn mixers(&self) -> std::sync::MutexGuard<'_, HashMap<WalletId, Mixer>> {
        self.mixers.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn settings(&self) -> CoinJoinSettings {
        *self.settings.read().unwrap_or_else(|p| p.into_inner())
    }

    fn is_mixing(&self, id: &WalletId) -> bool {
        self.mixers().get(id).is_some_and(|m| m.mixing)
    }

    fn any_mixing(&self) -> bool {
        self.mixers().values().any(|m| m.mixing)
    }

    /// Stops every task (session close).
    pub(crate) fn shutdown(&self) {
        for m in self.mixers().values_mut() {
            if let Some(stop) = m.stop.take() {
                let _ = stop.send(true);
            }
            for s in m.sessions.values() {
                let _ = s.stop.send(true);
            }
            if let Some(t) = m.task.take() {
                t.abort();
            }
            m.mixing = false;
        }
        for slot in [&self.refresher, &self.listener] {
            if let Some(t) = slot.lock().unwrap_or_else(|p| p.into_inner()).take() {
                t.abort();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The wallet's coins as mixing sees them
// ---------------------------------------------------------------------------

/// One unspent coin with its CoinJoin facts.
#[derive(Debug, Clone)]
pub(crate) struct MixViewCoin {
    pub coin: WalletCoin,
    pub path: Option<DerivationPath>,
    /// Rounds by the input-chain walk (negative: Core's special values).
    pub rounds: i32,
    pub fully_mixed: bool,
}

impl MixViewCoin {
    fn value(&self) -> u64 {
        self.coin.utxo.value()
    }

    fn trusted(&self) -> bool {
        let u = &self.coin.utxo;
        u.is_confirmed || u.is_instantlocked || u.is_trusted
    }

    /// Confirmed or InstantSend-locked (what Core's mixing needs of its
    /// inputs and collateral).
    fn final_enough(&self) -> bool {
        self.coin.utxo.is_confirmed || self.coin.utxo.is_instantlocked
    }

    fn free(&self) -> bool {
        !self.coin.user_locked && !self.coin.reserved
    }

    /// A denomination in the CoinJoin account.
    pub(crate) fn denominated(&self) -> bool {
        self.coin.coinjoin_account && is_denominated(self.value())
    }

    fn collateral(&self) -> bool {
        self.coin.coinjoin_account && is_collateral_amount(self.value())
    }

    fn bip44(&self, network: dashcore::Network) -> bool {
        self.path
            .as_ref()
            .is_some_and(|p| is_bip44_path(p, network))
    }
}

/// Every coin of a wallet with rounds and balances.
#[derive(Debug, Clone)]
pub(crate) struct MixView {
    pub height: u32,
    pub coins: Vec<MixViewCoin>,
    pub balances: CoinJoinBalances,
    pub total: u64,
    pub average_rounds: f32,
}

impl MixView {
    pub(crate) fn fully_mixed_spendable(&self) -> impl Iterator<Item = &MixViewCoin> {
        self.coins
            .iter()
            .filter(|c| c.denominated() && c.fully_mixed && c.final_enough() && c.free())
    }

    fn has_collateral_inputs(&self, only_confirmed: bool) -> bool {
        self.coins.iter().any(|c| {
            c.collateral()
                && c.free()
                && if only_confirmed {
                    c.final_enough()
                } else {
                    c.trusted()
                }
        })
    }

    /// Denominated coins per denomination (`CountInputsWithAmount`).
    fn denom_counts(&self) -> [u32; 5] {
        let mut counts = [0u32; 5];
        for c in self.coins.iter().filter(|c| c.denominated()) {
            if let Some(i) = DENOMINATIONS.iter().position(|d| *d == c.value()) {
                counts[i] += 1;
            }
        }
        counts
    }

    /// Unconfirmed denominations (`denominated_untrusted_pending`).
    fn unconfirmed_denominated(&self) -> u64 {
        self.coins
            .iter()
            .filter(|c| c.denominated() && !c.final_enough())
            .map(MixViewCoin::value)
            .sum()
    }

    /// Mixing inputs of `amount`: denominated, not fully mixed, final,
    /// free, shuffled (`ONLY_READY_TO_MIX`).
    fn ready_to_mix(&self, amount: u64) -> Vec<MixCoin> {
        let mut coins: Vec<MixCoin> = self
            .coins
            .iter()
            .filter(|c| {
                c.denominated()
                    && c.value() == amount
                    && c.rounds >= 0
                    && !c.fully_mixed
                    && c.final_enough()
                    && c.free()
            })
            .map(|c| MixCoin {
                outpoint: c.coin.utxo.outpoint,
                value: c.value(),
                script_pubkey: c.coin.utxo.txout.script_pubkey.clone(),
                rounds: c.rounds,
            })
            .collect();
        coins.shuffle(&mut rand::thread_rng());
        coins
    }

    /// Coins grouped by address for funding transactions
    /// (`SelectCoinsGroupedByAddresses`, wallet/coinjoin.cpp:124-230):
    /// spendable BIP44 coins and CoinJoin-account coins that are not
    /// denominations, trusted, free; `anonymizable` additionally skips
    /// collaterals, coins at most a tenth of the smallest denomination and
    /// fully mixed coins, and groups below the smallest denomination.
    fn tallies(
        &self,
        network: dashcore::Network,
        skip_denominated: bool,
        anonymizable: bool,
    ) -> Vec<(Address, Vec<&MixViewCoin>)> {
        let mut map: BTreeMap<String, (Address, Vec<&MixViewCoin>)> = BTreeMap::new();
        for c in &self.coins {
            let usable_account = c.bip44(network) || c.coin.coinjoin_account;
            if !usable_account || !c.trusted() || !c.free() {
                continue;
            }
            if !c.coin.utxo.is_spendable(self.height) {
                continue;
            }
            if skip_denominated && c.denominated() {
                continue;
            }
            if anonymizable
                && (is_collateral_amount(c.value())
                    || c.value() <= SMALLEST_DENOMINATION / 10
                    || c.fully_mixed)
            {
                continue;
            }
            let entry = map
                .entry(c.coin.utxo.address.to_string())
                .or_insert_with(|| (c.coin.utxo.address.clone(), Vec::new()));
            if entry.1.len() < planner::MAX_INPUTS_PER_ADDRESS {
                entry.1.push(c);
            }
        }
        map.into_values()
            .filter(|(_, coins)| {
                !anonymizable
                    || coins.iter().map(|c| c.value()).sum::<u64>() >= SMALLEST_DENOMINATION
            })
            .collect()
    }
}

/// Core's balances over `coins` (wallet/coinjoin.cpp:500-580).
fn compute_balances(
    coins: &[MixViewCoin],
    network: dashcore::Network,
    rounds_target: u32,
) -> (CoinJoinBalances, f32) {
    let view = MixView {
        height: u32::MAX,
        coins: coins.to_vec(),
        balances: CoinJoinBalances::default(),
        total: 0,
        average_rounds: 0.0,
    };
    // GetAnonymizableBalance(false, false): every trusted-or-not coin.
    let mut anonymizable = 0u64;
    let mut by_address: BTreeMap<String, (u64, bool)> = BTreeMap::new();
    for c in &view.coins {
        let usable = c.bip44(network) || c.coin.coinjoin_account;
        if !usable || c.coin.user_locked || c.coin.reserved {
            continue;
        }
        if is_collateral_amount(c.value())
            || c.value() <= SMALLEST_DENOMINATION / 10
            || c.fully_mixed
        {
            continue;
        }
        let e = by_address
            .entry(c.coin.utxo.address.to_string())
            .or_insert((0, false));
        e.0 += c.value();
        e.1 |= c.denominated();
    }
    for (amount, denominated) in by_address.into_values() {
        if amount < SMALLEST_DENOMINATION {
            continue;
        }
        let is_denom = denominated && is_denominated(amount);
        if amount >= SMALLEST_DENOMINATION + if is_denom { 0 } else { COLLATERAL_AMOUNT } {
            anonymizable += amount;
        }
    }
    let denoms: Vec<&MixViewCoin> = view.coins.iter().filter(|c| c.denominated()).collect();
    let denominated = denoms.iter().map(|c| c.value()).sum();
    let capped: Vec<(u64, i32)> = denoms
        .iter()
        .map(|c| {
            (
                c.value(),
                rounds::capped_rounds(c.rounds.max(0), rounds_target),
            )
        })
        .collect();
    let normalized_anonymized = progress::normalized_anonymized(&capped, rounds_target);
    let fully_mixed = denoms
        .iter()
        .filter(|c| c.fully_mixed && c.trusted())
        .map(|c| c.value())
        .sum();
    let average = progress::average_rounds(&capped.iter().map(|(_, r)| *r).collect::<Vec<_>>());
    (
        CoinJoinBalances {
            anonymizable,
            denominated,
            normalized_anonymized,
            fully_mixed,
        },
        average,
    )
}

/// The full derivation path of `address` in the wallet.
fn path_of(info: &ManagedWalletInfo, address: &Address) -> Option<DerivationPath> {
    info.all_managed_accounts()
        .into_iter()
        .find_map(|a| a.get_address_info(address))
        .map(|i| i.path)
}

/// `scriptSig` of a P2PKH input: `<DER signature ‖ sighash type> <pubkey>`.
fn p2pkh_script_sig(
    sig: &dashcore::secp256k1::ecdsa::Signature,
    sighash_type: u32,
    pubkey: &dashcore::secp256k1::PublicKey,
) -> ScriptBuf {
    let mut der = sig.serialize_der().to_vec();
    der.push(sighash_type as u8);
    let pk = pubkey.serialize();
    let mut out = Vec::with_capacity(der.len() + pk.len() + 2);
    out.push(der.len() as u8);
    out.extend_from_slice(&der);
    out.push(pk.len() as u8);
    out.extend_from_slice(&pk);
    ScriptBuf::from_bytes(out)
}

/// Signs inputs of `tx` with `signer`: `(input index, previous script,
/// derivation path)`.
async fn sign_p2pkh_inputs(
    signer: &VaultSigner,
    tx: &mut Transaction,
    inputs: &[(usize, ScriptBuf, DerivationPath)],
    sighash_type: u32,
) -> Result<(), EngineError> {
    for (index, script, path) in inputs {
        let hash = SighashCache::new(&*tx)
            .legacy_signature_hash(*index, script, sighash_type)
            .map_err(|e| EngineError::Internal(format!("sighash: {e}")))?;
        let (sig, pubkey) = signer
            .sign_ecdsa(path, hash.to_byte_array())
            .await
            .map_err(|e| EngineError::Internal(format!("signing: {e}")))?;
        tx.input[*index].script_sig = p2pkh_script_sig(&sig, sighash_type, &pubkey);
    }
    Ok(())
}

fn unsigned_input(outpoint: OutPoint) -> TxIn {
    TxIn {
        previous_output: outpoint,
        script_sig: ScriptBuf::new(),
        sequence: 0xffff_ffff,
        witness: Default::default(),
    }
}

fn display_hex(hash: &[u8; 32]) -> String {
    hash.iter().rev().map(|b| format!("{b:02x}")).collect()
}

/// `SIGHASH_ALL`.
const SIGHASH_ALL: u32 = 1;

// ---------------------------------------------------------------------------
// Session-level operations
// ---------------------------------------------------------------------------

impl NetworkSession {
    /// The network's CoinJoin options. In-memory read.
    pub fn coinjoin_settings(&self) -> Result<CoinJoinSettings, EngineError> {
        let _op = self.try_enter()?;
        Ok(self.coinjoin.settings())
    }

    /// Applies and stores the options. Turning CoinJoin off stops every
    /// wallet (`stop_reason = Disabled`).
    pub async fn set_coinjoin_settings(
        self: &Arc<Self>,
        settings: CoinJoinSettings,
    ) -> Result<(), EngineError> {
        settings
            .validate()
            .map_err(|e| EngineError::InvalidArgument(e.to_string()))?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let text = encode_settings(&settings);
            this.appdb_op(move |db| db.set_setting(GLOBAL_SCOPE, SETTINGS_KEY, Some(&text)))
                .await?;
            *this
                .coinjoin
                .settings
                .write()
                .unwrap_or_else(|p| p.into_inner()) = settings;
            if !settings.enabled {
                let mixing: Vec<WalletId> = this
                    .coinjoin
                    .mixers()
                    .iter()
                    .filter(|(_, m)| m.mixing)
                    .map(|(id, _)| *id)
                    .collect();
                for id in mixing {
                    this.halt_mixing(id, StopReason::Disabled);
                }
            }
            this.refresh_all_coinjoin_status().await;
            Ok(())
        })
        .await
    }

    /// The wallet's salt in display hex, created on first use.
    pub async fn coinjoin_salt(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<String, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            Ok(rounds::salt_to_display_hex(&this.salt_of(wallet_id).await?))
        })
        .await
    }

    /// `coinjoinsalt set`. Refused while the wallet mixes.
    pub async fn set_coinjoin_salt(
        self: &Arc<Self>,
        wallet_id: WalletId,
        salt_hex: String,
    ) -> Result<(), EngineError> {
        let salt = rounds::salt_from_display_hex(&salt_hex).ok_or_else(|| {
            EngineError::InvalidArgument("salt must be 64 lowercase hex characters".into())
        })?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            if this.coinjoin.is_mixing(&wallet_id) {
                return Err(EngineError::InvalidArgument(
                    "the salt cannot change while the wallet mixes".into(),
                ));
            }
            this.store_salt(wallet_id, &salt).await?;
            this.refresh_coinjoin_status(wallet_id).await;
            Ok(())
        })
        .await
    }

    /// `coinjoinsalt generate`: a new random salt. Refused while mixing.
    pub async fn generate_coinjoin_salt(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<String, EngineError> {
        let salt = random_salt();
        let hex = rounds::salt_to_display_hex(&salt);
        self.set_coinjoin_salt(wallet_id, hex.clone()).await?;
        Ok(hex)
    }

    /// Stores a salt read from a Dash Core wallet (`cj_salt`) at import.
    pub(crate) async fn import_coinjoin_salt(
        &self,
        wallet_id: WalletId,
        salt: [u8; 32],
    ) -> Result<(), EngineError> {
        self.store_salt(wallet_id, &salt).await
    }

    async fn store_salt(&self, wallet_id: WalletId, salt: &[u8; 32]) -> Result<(), EngineError> {
        let hex = rounds::salt_to_display_hex(salt);
        let scope = wallet_id.to_string();
        self.appdb_op(move |db| db.set_setting(&scope, SALT_KEY, Some(&hex)))
            .await
    }

    /// The wallet's salt (internal byte order), generated and stored on
    /// first use.
    pub(crate) async fn salt_of(&self, wallet_id: WalletId) -> Result<[u8; 32], EngineError> {
        let scope = wallet_id.to_string();
        let stored = self
            .appdb_op(move |db| db.setting(&scope, SALT_KEY))
            .await?;
        if let Some(hex) = stored {
            return rounds::salt_from_display_hex(&hex).ok_or_else(|| {
                EngineError::from(AppDbError::Corrupt(format!("{SALT_KEY} {hex:?}")))
            });
        }
        let salt = random_salt();
        self.store_salt(wallet_id, &salt).await?;
        Ok(salt)
    }

    /// The wallet's coins with rounds, the fully-mixed rule and Core's
    /// balances.
    pub(crate) async fn mix_view(&self, wallet_id: WalletId) -> Result<MixView, EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        let snapshot = self.coin_snapshot(&wallet, wallet_id).await?;
        let salt = self.salt_of(wallet_id).await?;
        let settings = self.coinjoin.settings();
        let network = self.network.core_network();
        let (owned, paths) = {
            let state = wallet.state().await;
            let owned = crate::history_ops::owned_scripts(&state);
            let paths: HashMap<Address, DerivationPath> = snapshot
                .coins
                .iter()
                .filter_map(|c| {
                    path_of(&state.core_wallet, &c.utxo.address)
                        .map(|p| (c.utxo.address.clone(), p))
                })
                .collect();
            (owned, paths)
        };
        let history = self.hub.history.snapshot(&wallet_id);
        let txs: HashMap<Txid, WalletTx> = history
            .iter()
            .map(|(txid, e)| {
                (
                    *txid,
                    WalletTx {
                        outputs: e.tx.output.iter().map(|o| o.value).collect(),
                        outputs_mine: e
                            .tx
                            .output
                            .iter()
                            .map(|o| owned.contains_key(&o.script_pubkey))
                            .collect(),
                        inputs: e
                            .tx
                            .input
                            .iter()
                            .enumerate()
                            .map(|(i, input)| {
                                (
                                    input.previous_output,
                                    e.own_inputs.get(&(i as u32)).map(|(v, _)| *v),
                                )
                            })
                            .collect(),
                    },
                )
            })
            .collect();
        let mut calc = RoundsCalculator::new(&txs);
        let coins: Vec<MixViewCoin> = snapshot
            .coins
            .into_iter()
            .map(|coin| {
                let denominated = coin.coinjoin_account && is_denominated(coin.utxo.value());
                let rounds = if denominated {
                    calc.rounds(&coin.utxo.outpoint)
                } else if is_collateral_amount(coin.utxo.value()) {
                    rounds::ROUNDS_COLLATERAL
                } else {
                    rounds::ROUNDS_NOT_DENOMINATED
                };
                let fully_mixed = denominated
                    && rounds::is_fully_mixed(rounds, settings.rounds, &coin.utxo.outpoint, &salt);
                MixViewCoin {
                    path: paths.get(&coin.utxo.address).cloned(),
                    coin,
                    rounds,
                    fully_mixed,
                }
            })
            .collect();
        let (balances, average_rounds) = compute_balances(&coins, network, settings.rounds);
        let total = coins.iter().map(MixViewCoin::value).sum();
        Ok(MixView {
            height: snapshot.height,
            coins,
            balances,
            total,
            average_rounds,
        })
    }

    // ---- status ----------------------------------------------------------

    /// The wallet's mixing status. In-memory read of the status the
    /// refresher keeps (once a second); the first call for a wallet computes
    /// it on a helper thread.
    pub fn coinjoin_status(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<CoinJoinStatus, EngineError> {
        let _op = self.try_enter()?;
        let manager = self.manager()?;
        if manager.get_wallet_blocking(&wallet_id.0).is_none() {
            return Err(EngineError::WalletNotFound(wallet_id.to_string()));
        }
        self.coinjoin
            .watched
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(wallet_id, now_secs());
        self.ensure_coinjoin_refresher();
        let cached = self
            .coinjoin
            .status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&wallet_id)
            .cloned();
        if let Some(s) = cached {
            return Ok(s);
        }
        let this = Arc::clone(self);
        let rt = self.rt.clone();
        std::thread::scope(|scope| {
            scope
                .spawn(move || rt.block_on(this.compute_coinjoin_status(wallet_id)))
                .join()
                .map_err(|_| EngineError::Internal("CoinJoin status computation panicked".into()))?
        })
    }

    async fn compute_coinjoin_status(
        &self,
        wallet_id: WalletId,
    ) -> Result<CoinJoinStatus, EngineError> {
        let view = self.mix_view(wallet_id).await?;
        self.hub
            .set_fully_mixed(wallet_id, view.balances.fully_mixed);
        let status = self.status_from_view(wallet_id, &view);
        self.coinjoin
            .status
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(wallet_id, status.clone());
        Ok(status)
    }

    fn status_from_view(&self, wallet_id: WalletId, view: &MixView) -> CoinJoinStatus {
        let settings = self.coinjoin.settings();
        let b = view.balances;
        let target = u64::from(settings.target_amount_dash) * 100_000_000;
        let p = progress::progress(&ProgressInputs {
            balance: view.total,
            anonymizable: b.anonymizable,
            anonymized: b.fully_mixed,
            denominated: b.denominated,
            normalized_anonymized: b.normalized_anonymized,
            rounds: settings.rounds,
            target_dash: settings.target_amount_dash,
        });
        let max =
            progress::max_to_anonymize(b.anonymizable, b.fully_mixed, settings.target_amount_dash);
        let watch_only = !self.vault.has_wallet_secret(&wallet_id.0);
        let unavailable = if !settings.enabled {
            Some(Unavailable::Disabled)
        } else if watch_only {
            Some(Unavailable::WatchOnly)
        } else if view.total < MIN_MIXING_BALANCE {
            Some(Unavailable::InsufficientFunds {
                min_duffs: MIN_MIXING_BALANCE,
            })
        } else {
            None
        };
        let mixers = self.coinjoin.mixers();
        let m = mixers.get(&wallet_id);
        let state = match m {
            Some(m) if m.stopping => MixingState::Stopping,
            Some(m) if m.mixing => MixingState::Mixing,
            _ => MixingState::Idle,
        };
        let sessions: Vec<SessionInfo> = m
            .map(|m| {
                m.sessions
                    .values()
                    .map(|s| SessionInfo {
                        pro_tx_hash: Some(display_hex(&s.masternode.pro_tx_hash)),
                        service: s.masternode.service.map(|a| a.to_string()),
                        denomination: denomination_to_amount(s.progress.denom),
                        state: s.progress.state,
                        entries: s.progress.entries,
                        last_message: s.progress.last_message,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let status = m
            .and_then(|m| {
                m.sessions
                    .values()
                    .last()
                    .map(|s| s.progress.status)
                    .or(m.status)
            })
            .unwrap_or(StatusCode::Idle);
        CoinJoinStatus {
            wallet_id,
            state,
            stop_reason: if state == MixingState::Idle {
                m.and_then(|m| m.stop_reason)
            } else {
                None
            },
            unavailable,
            balances: b,
            progress: ProgressInfo {
                overall_percent: f64::from(p.overall),
                denominated_percent: f64::from(p.denominated),
                partially_mixed_percent: f64::from(p.partially_mixed),
                mixed_percent: f64::from(p.mixed),
                average_rounds: f64::from(view.average_rounds),
            },
            amount_and_rounds: AmountAndRounds {
                amount: if max == 0 { target } else { max },
                rounds: settings.rounds,
                insufficient_inputs: view.total > 0 && max < target,
            },
            submitted_denominations: sessions.iter().filter_map(|s| s.denomination).collect(),
            sessions,
            status,
            queue_size: self
                .coinjoin
                .queues
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .len() as u32,
            keys_left: None,
        }
    }

    /// Recomputes one wallet's status and emits `CoinJoin` when it changed.
    pub(crate) async fn refresh_coinjoin_status(&self, wallet_id: WalletId) {
        let before = self
            .coinjoin
            .status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&wallet_id)
            .cloned();
        match self.compute_coinjoin_status(wallet_id).await {
            Ok(after) if before.as_ref() != Some(&after) => self.sink.emit(EngineEvent::CoinJoin {
                network: self.network.clone(),
                wallet_id,
            }),
            Ok(_) => {}
            Err(e) => tracing::debug!(%wallet_id, error = %e, "CoinJoin status refresh failed"),
        }
    }

    async fn refresh_all_coinjoin_status(&self) {
        let ids: Vec<WalletId> = self
            .coinjoin
            .status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .keys()
            .copied()
            .collect();
        for id in ids {
            self.refresh_coinjoin_status(id).await;
        }
    }

    /// Starts the once-a-second status refresher: wallets that mix or whose
    /// status was asked for in the last minute.
    fn ensure_coinjoin_refresher(self: &Arc<Self>) {
        let mut slot = self
            .coinjoin
            .refresher
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        *slot = Some(self.rt.spawn(async move {
            loop {
                tokio::time::sleep(TICK).await;
                let Some(this) = weak.upgrade() else { return };
                let Ok(_op) = this.try_enter() else { return };
                let now = now_secs();
                let ids: Vec<WalletId> = {
                    let mut watched = this
                        .coinjoin
                        .watched
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    watched.retain(|_, at| now.saturating_sub(*at) < 60);
                    let mut ids: HashSet<WalletId> = watched.keys().copied().collect();
                    ids.extend(
                        this.coinjoin
                            .mixers()
                            .iter()
                            .filter(|(_, m)| m.mixing)
                            .map(|(id, _)| *id),
                    );
                    ids.into_iter().collect()
                };
                for id in ids {
                    this.refresh_coinjoin_status(id).await;
                }
            }
        }));
    }

    // ---- start / stop ------------------------------------------------------

    /// "Start CoinJoin" for a wallet (QT-044, QT-112). Idempotent.
    pub async fn start_mixing(self: &Arc<Self>, wallet_id: WalletId) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            let settings = this.coinjoin.settings();
            if !settings.enabled {
                return Err(CoinJoinFailure::Disabled.into());
            }
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(CoinJoinFailure::WatchOnly.into());
            }
            match this.vault.mixing_signer(&wallet_id.0) {
                Ok(_) => {}
                Err(VaultError::NoSecret) => return Err(CoinJoinFailure::WatchOnly.into()),
                Err(_) => return Err(CoinJoinFailure::VaultLocked.into()),
            }
            if this.coinjoin.is_mixing(&wallet_id) {
                return Ok(());
            }
            let view = this.mix_view(wallet_id).await?;
            if view.total < MIN_MIXING_BALANCE {
                return Err(CoinJoinFailure::InsufficientFunds {
                    min_duffs: MIN_MIXING_BALANCE,
                }
                .into());
            }
            let (stop, stop_rx) = watch::channel(false);
            let weak = Arc::downgrade(&this);
            {
                let mut mixers = this.coinjoin.mixers();
                let m = mixers.entry(wallet_id).or_default();
                m.mixing = true;
                m.stopping = false;
                m.stop_reason = None;
                m.status = None;
                m.stop = Some(stop);
                m.task = Some(tokio::spawn(mixing_loop(weak, wallet_id, stop_rx)));
            }
            this.ensure_coinjoin_refresher();
            this.ensure_queue_listener();
            this.refresh_coinjoin_status(wallet_id).await;
            Ok(())
        })
        .await
    }

    /// "Stop CoinJoin": `resetPool` (open sessions abandoned, their coins
    /// released) then stop. Idempotent.
    pub async fn stop_mixing(self: &Arc<Self>, wallet_id: WalletId) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            this.halt_mixing(wallet_id, StopReason::UserRequested);
            this.refresh_coinjoin_status(wallet_id).await;
            Ok(())
        })
        .await
    }

    /// Stops a wallet's loop and sessions and records why.
    pub(crate) fn halt_mixing(&self, wallet_id: WalletId, reason: StopReason) {
        let collateral: Vec<OutPoint>;
        {
            let mut mixers = self.coinjoin.mixers();
            let Some(m) = mixers.get_mut(&wallet_id) else {
                return;
            };
            if !m.mixing && m.sessions.is_empty() {
                return;
            }
            m.mixing = false;
            m.stopping = false;
            m.stop_reason = Some(reason);
            m.status = Some(StatusCode::Idle);
            if let Some(stop) = m.stop.take() {
                let _ = stop.send(true);
            }
            for s in m.sessions.values() {
                let _ = s.stop.send(true);
            }
            collateral = m.collateral_inputs.drain().flat_map(|(_, v)| v).collect();
        }
        // Sessions release their own inputs and outputs as they end; the
        // collateral inputs are released here.
        self.spends.remove(&wallet_id, collateral);
        if !self.coinjoin.any_mixing()
            && let Some(t) = self
                .coinjoin
                .listener
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take()
        {
            t.abort();
        }
    }

    /// Stops mixing when a wallet is unloaded or removed.
    pub(crate) fn coinjoin_wallet_gone(&self, wallet_id: WalletId) {
        self.halt_mixing(wallet_id, StopReason::WalletUnloaded);
        self.coinjoin
            .status
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&wallet_id);
    }

    // ---- queue listener and masternode list --------------------------------

    /// Refreshes the masternode picker from the SPV list; `false` when the
    /// list is not available yet.
    async fn refresh_picker(&self) -> bool {
        let Ok(manager) = self.manager() else {
            return false;
        };
        let Some(list) = manager.spv().masternode_list_summaries().await else {
            return false;
        };
        let entries: Vec<PeerEntry> = list
            .into_iter()
            .map(|s| PeerEntry {
                pro_tx_hash: s.pro_tx_hash,
                service: s.service_address,
                operator_public_key: s.operator_public_key,
                is_valid: s.is_valid,
                is_evonode: s.is_evonode,
            })
            .collect();
        let empty = entries.is_empty();
        self.coinjoin
            .picker
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .update(entries);
        !empty
    }

    /// Keeps one connection to a random masternode with `senddsq` open
    /// while any wallet mixes, feeding announced queues into the queue
    /// manager (Core clients learn queues from every peer; an SPV wallet
    /// has no Core peers of its own).
    fn ensure_queue_listener(self: &Arc<Self>) {
        let mut slot = self
            .coinjoin
            .listener
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|t| !t.is_finished()) {
            return;
        }
        let weak = Arc::downgrade(self);
        *slot = Some(self.rt.spawn(async move {
            loop {
                let Some(this) = weak.upgrade() else { return };
                if !this.coinjoin.any_mixing() {
                    return;
                }
                this.refresh_picker().await;
                let target = {
                    let picker = this
                        .coinjoin
                        .picker
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    let mut all: Vec<PeerEntry> = picker.masternodes().cloned().collect();
                    all.shuffle(&mut rand::thread_rng());
                    all.into_iter().next()
                };
                let network = this.network.core_network();
                drop(this);
                let Some(mn) = target else {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                };
                let Some(addr) = mn.service else { continue };
                let config = SessionConfig {
                    want_dsq: true,
                    ..SessionConfig::default()
                };
                let session = match Session::connect(addr, network, config).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::debug!(error = %e, "CoinJoin queue listener could not connect");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                };
                let mut rx = session.subscribe(&[dw_p2p::commands::DSQUEUE]);
                while let Some(msg) = rx.recv().await {
                    let Some(this) = weak.upgrade() else { return };
                    if !this.coinjoin.any_mixing() {
                        return;
                    }
                    if let Ok(dsq) = Queue::decode(&msg.payload) {
                        this.receive_queue(dsq);
                    }
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }));
    }

    fn receive_queue(&self, dsq: Queue) {
        let (mn, enabled) = {
            let picker = self
                .coinjoin
                .picker
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            (
                picker
                    .by_pro_tx_hash(&dsq.pro_tx_hash)
                    .map(|e| QueueMasternode {
                        operator_public_key: e.operator_public_key,
                        is_valid: e.is_valid,
                    }),
                picker.enabled_count(),
            )
        };
        let verdict = self
            .coinjoin
            .queues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .receive(dsq, mn.as_ref(), now_secs() as i64, enabled);
        tracing::trace!(?verdict, "dsq");
    }

    // ---- funding transactions ----------------------------------------------

    /// Reserves `count` addresses of the CoinJoin account's external
    /// (`internal = false`) or internal chain.
    pub(crate) async fn reserve_coinjoin_addresses(
        &self,
        wallet_id: WalletId,
        count: usize,
        internal: bool,
    ) -> Result<Vec<Address>, EngineError> {
        let manager = self.manager()?;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm
            .get_wallet_and_info_mut(&wallet_id.0)
            .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))?;
        let xpub = wallet
            .accounts
            .coinjoin_accounts
            .get(&0)
            .map(|a| a.account_xpub)
            .ok_or_else(|| EngineError::Internal("the wallet has no CoinJoin account".into()))?;
        let account = info
            .core_wallet
            .accounts
            .coinjoin_accounts
            .get_mut(&0)
            .ok_or_else(|| EngineError::Internal("the wallet has no CoinJoin account".into()))?;
        let now = now_secs();
        let mut out = Vec::with_capacity(count);
        {
            let pool = match account.managed_account_type_mut() {
                ManagedAccountType::CoinJoin {
                    external_addresses,
                    internal_addresses,
                    ..
                } => {
                    if internal {
                        internal_addresses
                    } else {
                        external_addresses
                    }
                }
                _ => return Err(EngineError::Internal("not a CoinJoin account".into())),
            };
            for _ in 0..count {
                out.push(
                    pool.next_unused_and_reserve(&KeySource::Public(xpub), now)
                        .map_err(|e| EngineError::Internal(format!("CoinJoin address: {e}")))?,
                );
            }
        }
        account.bump_monitor_revision();
        Ok(out)
    }

    /// Returns reserved CoinJoin-account addresses to the pool.
    pub(crate) async fn release_coinjoin_addresses(
        &self,
        wallet_id: WalletId,
        addresses: &[Address],
    ) {
        let Ok(manager) = self.manager() else { return };
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some(info) = wm.get_wallet_info_mut(&wallet_id.0) else {
            return;
        };
        let Some(account) = info.core_wallet.accounts.coinjoin_accounts.get_mut(&0) else {
            return;
        };
        if let ManagedAccountType::CoinJoin {
            external_addresses,
            internal_addresses,
            ..
        } = account.managed_account_type_mut()
        {
            for a in addresses {
                for pool in [&mut *external_addresses, &mut *internal_addresses] {
                    if let Some(i) = pool.address_index(a) {
                        pool.release_reservation(i);
                    }
                }
            }
        }
    }

    /// Builds, signs with the funding signer and broadcasts a transaction
    /// spending `coins` into `plan`'s outputs (fresh CoinJoin-account
    /// addresses) and change (BIP44 change for BIP44 coins, the CoinJoin
    /// internal chain otherwise). Returns the txid.
    async fn commit_funding_tx(
        &self,
        wallet_id: WalletId,
        coins: &[&MixViewCoin],
        plan: &TxPlan,
    ) -> Result<Txid, EngineError> {
        let network = self.network.core_network();
        let signer = self
            .vault
            .mixing_funding_signer(&wallet_id.0)
            .map_err(|_| CoinJoinFailure::VaultLocked)?;
        let wallet = self.wallet(&wallet_id).await?;
        let mut reserved = self
            .reserve_coinjoin_addresses(wallet_id, plan.outputs.len(), false)
            .await?;
        let change = match plan.change {
            Some(_) if coins.iter().all(|c| c.bip44(network)) => {
                Some(wallet.core().next_change_address_for_account(0).await?)
            }
            Some(_) => {
                let mut a = self.reserve_coinjoin_addresses(wallet_id, 1, true).await?;
                reserved.extend(a.iter().cloned());
                a.pop()
            }
            None => None,
        };
        let mut output: Vec<TxOut> = plan
            .outputs
            .iter()
            .zip(&reserved)
            .map(|(value, addr)| TxOut {
                value: *value,
                script_pubkey: addr.script_pubkey(),
            })
            .collect();
        if let (Some(value), Some(addr)) = (plan.change, &change) {
            output.push(TxOut {
                value,
                script_pubkey: addr.script_pubkey(),
            });
        }
        output.shuffle(&mut rand::thread_rng());
        let mut tx = Transaction {
            version: 2,
            lock_time: 0,
            input: coins
                .iter()
                .map(|c| unsigned_input(c.coin.utxo.outpoint))
                .collect(),
            output,
            special_transaction_payload: None,
        };
        let to_sign: Vec<(usize, ScriptBuf, DerivationPath)> = coins
            .iter()
            .enumerate()
            .map(|(i, c)| {
                c.path
                    .clone()
                    .map(|p| (i, c.coin.utxo.txout.script_pubkey.clone(), p))
                    .ok_or_else(|| EngineError::Internal("input without a derivation path".into()))
            })
            .collect::<Result<_, _>>()?;
        let outpoints: Vec<OutPoint> = coins.iter().map(|c| c.coin.utxo.outpoint).collect();
        self.spends.add(wallet_id, outpoints.iter().copied());
        if let Err(e) = sign_p2pkh_inputs(&signer, &mut tx, &to_sign, SIGHASH_ALL).await {
            self.spends.remove(&wallet_id, outpoints);
            self.release_coinjoin_addresses(wallet_id, &reserved).await;
            return Err(e);
        }
        match wallet.core().broadcast_transaction(&tx).await {
            Ok(txid) => {
                self.hub.note_announced(txid);
                if let Ok(manager) = self.manager() {
                    self.refresh_wallet_state(&manager, wallet_id).await;
                }
                self.hub.pump.mark_balances(wallet_id);
                self.hub.pump.mark_history(wallet_id, None);
                Ok(txid)
            }
            Err(platform_wallet::PlatformWalletError::TransactionBroadcastUnconfirmed(r)) => {
                // May be on the network: keep inputs and outputs as they are.
                tracing::warn!(reason = %r, "CoinJoin funding transaction outcome unknown");
                Ok(tx.txid())
            }
            Err(e) => {
                self.spends.remove(&wallet_id, outpoints);
                self.release_coinjoin_addresses(wallet_id, &reserved).await;
                Err(CoinJoinFailure::BroadcastRejected(e.to_string()).into())
            }
        }
    }

    /// `CreateDenominated(nBalanceToDenominate)` (client.cpp:2194-2224).
    async fn create_denominated(
        &self,
        wallet_id: WalletId,
        view: &MixView,
        to_denominate: i64,
    ) -> Result<bool, EngineError> {
        let settings = self.coinjoin.settings();
        let network = self.network.core_network();
        let mut tallies = view.tallies(network, true, true);
        tallies.sort_by_key(|(_, coins)| {
            std::cmp::Reverse(coins.iter().map(|c| c.value()).sum::<u64>())
        });
        let create_collateral = !view.has_collateral_inputs(false);
        for (_, coins) in &tallies {
            let tally = Tally {
                amount: coins.iter().map(|c| c.value()).sum(),
                inputs: coins.len(),
            };
            let Some(plan) = planner::plan_denominations(
                &tally,
                to_denominate,
                create_collateral,
                view.denom_counts(),
                settings.denoms_goal,
                settings.denoms_hard_cap,
                crate::send::MIN_FEE_PER_KB,
            ) else {
                continue;
            };
            let txid = self.commit_funding_tx(wallet_id, coins, &plan).await?;
            tracing::info!(%wallet_id, %txid, outputs = plan.outputs.len(), "CoinJoin denominations created");
            return Ok(true);
        }
        Ok(false)
    }

    /// `MakeCollateralAmounts()` (client.cpp:2022-2060).
    async fn make_collateral_amounts(
        &self,
        wallet_id: WalletId,
        view: &MixView,
    ) -> Result<bool, EngineError> {
        let network = self.network.core_network();
        let mut tallies = view.tallies(network, false, false);
        tallies.sort_by_key(|(_, coins)| coins.iter().map(|c| c.value()).sum::<u64>());
        for try_denominated in [false, true] {
            for (_, coins) in &tallies {
                let tally = Tally {
                    amount: coins.iter().map(|c| c.value()).sum(),
                    inputs: coins.len(),
                };
                let Some(plan) =
                    planner::plan_collaterals(&tally, try_denominated, crate::send::MIN_FEE_PER_KB)
                else {
                    continue;
                };
                // Case 1's remainder (the last output) is not a collateral:
                // it goes back as change.
                let mut plan = plan;
                if plan.outputs.len() == 2 && !is_collateral_amount(plan.outputs[1]) {
                    plan.change = plan.outputs.pop();
                }
                let txid = self.commit_funding_tx(wallet_id, coins, &plan).await?;
                tracing::info!(%wallet_id, %txid, "CoinJoin collateral inputs created");
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `CreateCollateralTransaction` (client.cpp:2149-2191): one random
    /// collateral coin, change back when it holds two collaterals, else an
    /// `OP_RETURN` output and all of it as fee.
    async fn collateral_tx(
        &self,
        wallet_id: WalletId,
        view: &MixView,
    ) -> Result<Option<(Transaction, OutPoint, Vec<Address>)>, EngineError> {
        let mut candidates: Vec<&MixViewCoin> = view
            .coins
            .iter()
            .filter(|c| c.collateral() && c.final_enough() && c.free())
            .collect();
        candidates.shuffle(&mut rand::thread_rng());
        let Some(coin) = candidates.first() else {
            return Ok(None);
        };
        let Some(path) = coin.path.clone() else {
            return Ok(None);
        };
        let signer = self
            .vault
            .mixing_signer(&wallet_id.0)
            .map_err(|_| CoinJoinFailure::VaultLocked)?;
        let (output, reserved) = match planner::collateral_change(coin.value()) {
            Some(change) => {
                let addrs = self.reserve_coinjoin_addresses(wallet_id, 1, true).await?;
                (
                    TxOut {
                        value: change,
                        script_pubkey: addrs[0].script_pubkey(),
                    },
                    addrs,
                )
            }
            None => (
                TxOut {
                    value: 0,
                    script_pubkey: ScriptBuf::from_bytes(vec![0x6a]),
                },
                Vec::new(),
            ),
        };
        let mut tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![unsigned_input(coin.coin.utxo.outpoint)],
            output: vec![output],
            special_transaction_payload: None,
        };
        let script = coin.coin.utxo.txout.script_pubkey.clone();
        if let Err(e) = sign_p2pkh_inputs(&signer, &mut tx, &[(0, script, path)], SIGHASH_ALL).await
        {
            self.release_coinjoin_addresses(wallet_id, &reserved).await;
            return Err(e);
        }
        Ok(Some((tx, coin.coin.utxo.outpoint, reserved)))
    }

    // ---- automatic denominating ------------------------------------------

    fn set_mix_status(&self, wallet_id: WalletId, status: StatusCode) {
        tracing::debug!(%wallet_id, ?status, "CoinJoin step");
        if let Some(m) = self.coinjoin.mixers().get_mut(&wallet_id) {
            m.status = Some(status);
        }
    }

    /// One `DoAutomaticDenominating` pass (client.cpp:962-1262).
    async fn auto_denominate(self: &Arc<Self>, wallet_id: WalletId) -> Result<(), EngineError> {
        let settings = self.coinjoin.settings();
        let synced = self.hub.tracker().caught_up();
        if !synced {
            self.set_mix_status(wallet_id, StatusCode::SyncInProgress);
            return Ok(());
        }
        if self.vault.mixing_signer(&wallet_id.0).is_err() {
            self.set_mix_status(wallet_id, StatusCode::WalletLocked);
            return Ok(());
        }
        let has_list = self.refresh_picker().await;
        {
            let mut picker = self
                .coinjoin
                .picker
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            picker.trim_used();
        }
        let view = self.mix_view(wallet_id).await?;
        let (active, last_success) = {
            let mixers = self.coinjoin.mixers();
            let m = mixers.get(&wallet_id);
            (
                m.map(|m| m.sessions.len()).unwrap_or(0),
                m.map(|m| m.last_success_height).unwrap_or(0),
            )
        };
        let max_sessions = if settings.multi_session {
            settings.max_sessions as usize
        } else {
            1
        };
        // WaitForAnotherBlock (client.cpp:885-892).
        if !settings.multi_session && last_success != 0 && view.height <= last_success {
            return Ok(());
        }
        if !has_list {
            self.set_mix_status(wallet_id, StatusCode::NoMasternodes);
            return Ok(());
        }
        let b = view.balances;
        let target = i64::from(settings.target_amount_dash) * 100_000_000;
        let mut needs = target - b.fully_mixed as i64;
        if needs < 0 {
            // Nothing to do (rebalancing is not done by this client).
            self.set_mix_status(wallet_id, StatusCode::Idle);
            return Ok(());
        }
        let value_min = planner::min_value_to_mix(view.has_collateral_inputs(true));
        if b.anonymizable < value_min {
            self.set_mix_status(wallet_id, StatusCode::NotEnoughFunds);
            return Ok(());
        }
        let anonymizable_non_denom: u64 = view
            .tallies(self.network.core_network(), true, true)
            .iter()
            .map(|(_, coins)| coins.iter().map(|c| c.value()).sum::<u64>())
            .filter(|amount| *amount >= SMALLEST_DENOMINATION + COLLATERAL_AMOUNT)
            .sum();
        let to_denominate = target - b.denominated as i64;
        if b.denominated as i64 - b.fully_mixed as i64 > needs {
            // Consume the final denomination (client.cpp:1062-1074).
            let extra = DENOMINATIONS
                .iter()
                .rev()
                .copied()
                .find(|d| needs < *d as i64)
                .unwrap_or(0);
            needs += extra as i64;
        }
        if anonymizable_non_denom >= value_min + COLLATERAL_AMOUNT
            && to_denominate > 0
            && self
                .create_denominated(wallet_id, &view, to_denominate)
                .await?
        {
            self.note_success_height(wallet_id, view.height);
            return Ok(());
        }
        if !view.has_collateral_inputs(true) {
            if !view.has_collateral_inputs(false)
                && self.make_collateral_amounts(wallet_id, &view).await?
            {
                self.note_success_height(wallet_id, view.height);
            }
            return Ok(());
        }
        if active >= max_sessions {
            self.set_mix_status(wallet_id, StatusCode::MixingInProgress);
            return Ok(());
        }
        if !settings.multi_session && view.unconfirmed_denominated() > 0 {
            self.set_mix_status(wallet_id, StatusCode::UnconfirmedDenominated);
            return Ok(());
        }
        if needs <= 0 {
            return Ok(());
        }
        let Some((collateral, collateral_input, collateral_addrs)) =
            self.collateral_tx(wallet_id, &view).await?
        else {
            return Ok(());
        };
        // JoinExistingQueue (client.cpp:1384-1471).
        let joined = loop {
            let next = self
                .coinjoin
                .queues
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .next_untried(0, now_secs() as i64);
            let Some(dsq) = next else { break None };
            let mn = {
                let picker = self
                    .coinjoin
                    .picker
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                picker
                    .by_pro_tx_hash(&dsq.pro_tx_hash)
                    .filter(|e| e.is_valid && e.service.is_some())
                    .cloned()
            };
            let Some(mn) = mn else { continue };
            let Some(amount) = denomination_to_amount(dsq.denom) else {
                continue;
            };
            let have =
                select_by_denomination(&view.ready_to_mix(amount), amount, needs.max(0) as u64);
            if have.is_empty() {
                continue;
            }
            self.coinjoin
                .picker
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .mark_used(mn.pro_tx_hash);
            break Some((mn, dsq.denom));
        };
        let chosen = match joined {
            Some(j) => Some(j),
            None => self.start_new_queue(&view, needs.max(0) as u64, wallet_id),
        };
        let Some((mn, denom)) = chosen else {
            self.release_coinjoin_addresses(wallet_id, &collateral_addrs)
                .await;
            return Ok(());
        };
        self.spawn_session(
            wallet_id,
            mn,
            denom,
            collateral,
            collateral_input,
            collateral_addrs,
        );
        Ok(())
    }

    /// `StartNewQueue` (client.cpp:1473-1555): a random unused masternode
    /// that did not queue too recently, a random denomination we hold.
    fn start_new_queue(
        &self,
        view: &MixView,
        needs: u64,
        wallet_id: WalletId,
    ) -> Option<(PeerEntry, u32)> {
        let values: Vec<u64> = DENOMINATIONS
            .iter()
            .flat_map(|d| view.ready_to_mix(*d).into_iter().map(|c| c.value))
            .collect();
        let Some(amounts) = select_denominated_amounts(&values, needs) else {
            self.set_mix_status(wallet_id, StatusCode::NoCompatibleInputs);
            return None;
        };
        let mut picker = self
            .coinjoin
            .picker
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let enabled = picker.enabled_count();
        for _ in 0..10 {
            let Some(mn) = picker.random_unused_masternode().cloned() else {
                drop(picker);
                self.set_mix_status(wallet_id, StatusCode::NoRandomMasternode);
                return None;
            };
            picker.mark_used(mn.pro_tx_hash);
            if self
                .coinjoin
                .queues
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .threshold_exceeded(&mn.pro_tx_hash, enabled)
            {
                continue;
            }
            let denom = choose_session_denom(&amounts, &mut rand::thread_rng())?;
            return Some((mn, denom));
        }
        drop(picker);
        self.set_mix_status(wallet_id, StatusCode::FailedToStartQueue);
        None
    }

    fn note_success_height(&self, wallet_id: WalletId, height: u32) {
        if let Some(m) = self.coinjoin.mixers().get_mut(&wallet_id) {
            m.last_success_height = height.max(1);
        }
    }

    /// Starts one session task.
    fn spawn_session(
        self: &Arc<Self>,
        wallet_id: WalletId,
        mn: PeerEntry,
        denom: u32,
        collateral: Transaction,
        collateral_input: OutPoint,
        collateral_addrs: Vec<Address>,
    ) {
        let settings = self.coinjoin.settings();
        let (stop, stop_rx) = watch::channel(false);
        let key = {
            let mut mixers = self.coinjoin.mixers();
            let Some(m) = mixers.get_mut(&wallet_id).filter(|m| m.mixing) else {
                return;
            };
            let key = m.next_session;
            m.next_session += 1;
            m.sessions.insert(
                key,
                SessionSlot {
                    masternode: mn.clone(),
                    progress: SessionProgress {
                        state: PoolState::Queue,
                        session_id: 0,
                        denom,
                        entries: 0,
                        last_message: None,
                        status: StatusCode::TryingToConnect,
                    },
                    stop,
                },
            );
            m.collateral_inputs.insert(key, vec![collateral_input]);
            m.status = Some(StatusCode::TryingToConnect);
            key
        };
        self.spends.add(wallet_id, [collateral_input]);
        let params = SessionParams::new(
            self.network.core_network(),
            mn,
            denom,
            collateral,
            settings.rounds,
        );
        let adapter = EngineMixingWallet {
            session: Arc::downgrade(self),
            wallet_id,
            reserved: Mutex::new(Vec::new()),
        };
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            let observe = |p: &SessionProgress| {
                if let Some(this) = weak.upgrade()
                    && let Some(m) = this.coinjoin.mixers().get_mut(&wallet_id)
                    && let Some(slot) = m.sessions.get_mut(&key)
                {
                    slot.progress = p.clone();
                }
            };
            let outcome = run_session(&adapter, params, observe, stop_rx).await;
            let Some(this) = weak.upgrade() else { return };
            this.release_coinjoin_addresses(wallet_id, &collateral_addrs)
                .await;
            this.spends.remove(&wallet_id, [collateral_input]);
            let height = this
                .hub
                .wallet_state(&wallet_id)
                .map(|s| s.tip())
                .unwrap_or(0);
            {
                let mut mixers = this.coinjoin.mixers();
                if let Some(m) = mixers.get_mut(&wallet_id) {
                    let slot = m.sessions.remove(&key);
                    m.collateral_inputs.remove(&key);
                    match &outcome {
                        SessionOutcome::Success { inputs } => {
                            let now = now_secs();
                            for i in inputs {
                                m.held.insert(*i, now);
                            }
                            m.last_success_height = height.max(1);
                            m.status = Some(StatusCode::Masternode(PoolMessage::Success));
                        }
                        SessionOutcome::Failed(f) => {
                            tracing::info!(%wallet_id, failure = %f, "CoinJoin session ended");
                            m.status = Some(match f {
                                dw_coinjoin::client::SessionFailure::Rejected(msg)
                                | dw_coinjoin::client::SessionFailure::Completed(msg) => {
                                    StatusCode::Masternode(*msg)
                                }
                                dw_coinjoin::client::SessionFailure::NoInputs => {
                                    StatusCode::NoCompatibleInputs
                                }
                                dw_coinjoin::client::SessionFailure::Timeout(_) => {
                                    StatusCode::Masternode(PoolMessage::Session)
                                }
                                _ => slot.map(|s| s.progress.status).unwrap_or(StatusCode::Idle),
                            });
                        }
                    }
                }
            }
            if matches!(outcome, SessionOutcome::Success { .. }) {
                tracing::info!(%wallet_id, "CoinJoin session succeeded");
            }
            this.refresh_coinjoin_status(wallet_id).await;
        });
    }

    /// Releases held inputs older than the observation timeout and drops
    /// those the wallet saw spent.
    fn expire_held(&self, wallet_id: WalletId) {
        let now = now_secs();
        let reserved = self.spends.snapshot(&wallet_id);
        let expired: Vec<OutPoint> = {
            let mut mixers = self.coinjoin.mixers();
            let Some(m) = mixers.get_mut(&wallet_id) else {
                return;
            };
            // Gone from the reserved set: the wallet saw them spent.
            m.held.retain(|o, _| reserved.contains(o));
            let expired: Vec<OutPoint> = m
                .held
                .iter()
                .filter(|(_, at)| now.saturating_sub(**at) >= PENDING_OBSERVATION_SECS)
                .map(|(o, _)| *o)
                .collect();
            for o in &expired {
                m.held.remove(o);
            }
            expired
        };
        self.spends.remove(&wallet_id, expired);
    }

    // ---- recovery and moving mixed coins ---------------------------------

    /// IOS-057 recovery scan: raises the CoinJoin account's and the BIP44
    /// chains' lookahead to 1000 and rescans from the wallet's birth.
    pub async fn coinjoin_recovery_scan(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<RecoveryReport, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let (known_before, birth) = {
                let _op = this.enter().await?;
                let manager = this.manager()?;
                if !manager.spv().is_started() {
                    return Err(CoinJoinFailure::SpvNotRunning.into());
                }
                if this.hub.rescan().is_some() {
                    return Err(EngineError::InvalidArgument(
                        "another rescan is running".into(),
                    ));
                }
                let wallet = this.wallet(&wallet_id).await?;
                crate::keys::apply_lookahead(&wallet, RECOVERY_LOOKAHEAD).await?;
                // Kept for later sessions, as a restore's lookahead is.
                this.store_lookahead(wallet_id, RECOVERY_LOOKAHEAD).await;
                let birth = this
                    .hub
                    .wallet_state(&wallet_id)
                    .map(|s| s.birth_height)
                    .unwrap_or(0);
                (this.hub.history.snapshot(&wallet_id).len(), birth)
            };
            this.rescan(crate::sync::RescanFrom::Height(birth)).await?;
            // Wait for the rescan to finish (it reports through the hub); a
            // stopped SPV client or a closed session ends it unfinished.
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if this.hub.rescan().is_none() {
                    break;
                }
                let running = this.manager().is_ok_and(|m| m.spv().is_started());
                if !running {
                    return Err(CoinJoinFailure::SpvNotRunning.into());
                }
            }
            let view = this.mix_view(wallet_id).await?;
            let coinjoin_balance = view
                .coins
                .iter()
                .filter(|c| c.coin.coinjoin_account)
                .map(MixViewCoin::value)
                .sum();
            let known_after = this.hub.history.snapshot(&wallet_id).len();
            Ok(RecoveryReport {
                coinjoin_addresses_scanned: RECOVERY_LOOKAHEAD * 2,
                bip44_addresses_scanned: RECOVERY_LOOKAHEAD * 2,
                coinjoin_balance,
                new_transactions: known_after.saturating_sub(known_before) as u32,
            })
        })
        .await
    }

    /// The CoinJoin account's coins above the threshold, in sweep chunks.
    async fn sweep_coins(
        &self,
        wallet_id: WalletId,
    ) -> Result<(Vec<MixViewCoin>, Vec<dw_coinjoin::recovery::SweepChunk>), EngineError> {
        let view = self.mix_view(wallet_id).await?;
        let coins: Vec<MixViewCoin> = view
            .coins
            .into_iter()
            .filter(|c| {
                c.coin.coinjoin_account
                    && c.free()
                    && c.trusted()
                    && c.coin.utxo.is_spendable(view.height)
                    && c.path
                        .as_ref()
                        .is_some_and(|p| is_coinjoin_path(p, self.network.core_network()))
            })
            .collect();
        let values: Vec<u64> = coins.iter().map(MixViewCoin::value).collect();
        let chunks = dw_coinjoin::recovery::plan_sweep(&values, crate::send::MIN_FEE_PER_KB);
        Ok((coins, chunks))
    }

    /// What "Move mixed coins" would broadcast. Reads coins only.
    pub async fn mixed_coins_sweep_plan(
        self: &Arc<Self>,
        wallet_id: WalletId,
        destination: SweepDestination,
    ) -> Result<SweepPlan, EngineError> {
        if destination == SweepDestination::Shielded {
            return Err(EngineError::NotImplemented(
                "NetworkSession.mixed_coins_sweep_plan.shielded".into(),
            ));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let (_, chunks) = this.sweep_coins(wallet_id).await?;
            if chunks.is_empty() {
                return Err(CoinJoinFailure::NothingToMove.into());
            }
            Ok(SweepPlan {
                destination,
                total: chunks.iter().map(|c| c.amount + c.fee).sum(),
                chunks: chunks
                    .iter()
                    .map(|c| SweepChunkInfo {
                        inputs: c.coins.len() as u32,
                        amount: c.amount,
                        fee: c.fee,
                    })
                    .collect(),
            })
        })
        .await
    }

    /// "Move mixed coins" (IOS-057): broadcasts the sweep chunks to fresh
    /// BIP44 receive addresses of the same wallet. `grant_id`: a `Spend`
    /// grant of at least the total. Stops at the first failing chunk.
    pub async fn move_mixed_coins(
        self: &Arc<Self>,
        wallet_id: WalletId,
        destination: SweepDestination,
        grant_id: String,
    ) -> Result<SweepResult, EngineError> {
        if destination == SweepDestination::Shielded {
            return Err(EngineError::NotImplemented(
                "NetworkSession.move_mixed_coins.shielded".into(),
            ));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(CoinJoinFailure::WatchOnly.into());
            }
            let (coins, chunks) = this.sweep_coins(wallet_id).await?;
            if chunks.is_empty() {
                return Err(CoinJoinFailure::NothingToMove.into());
            }
            let total: u64 = chunks.iter().map(|c| c.amount + c.fee).sum();
            let vault = this.vault.clone();
            let signer = tokio::task::spawn_blocking(move || {
                let token = vault.redeem_grant(
                    &grant_id,
                    dw_vault::GrantKind::Spend,
                    Some(&wallet_id.0),
                )?;
                if token.max_duffs().unwrap_or(0) < total {
                    return Err(VaultError::GrantInvalid);
                }
                vault.signer(&wallet_id.0, &token)
            })
            .await?
            .map_err(|e| match e {
                VaultError::NoVault | VaultError::Locked | VaultError::MixingOnly => {
                    EngineError::from(CoinJoinFailure::VaultLocked)
                }
                _ => CoinJoinFailure::GrantInvalid.into(),
            })?;
            let wallet = this.wallet(&wallet_id).await?;
            let mut result = SweepResult {
                txids: Vec::new(),
                moved: 0,
                remaining: total,
                failure_code: None,
            };
            for chunk in &chunks {
                let dest = wallet.core().next_receive_address_for_account(0).await?;
                let picked: Vec<&MixViewCoin> = chunk.coins.iter().map(|i| &coins[*i]).collect();
                let mut tx = Transaction {
                    version: 2,
                    lock_time: 0,
                    input: picked
                        .iter()
                        .map(|c| unsigned_input(c.coin.utxo.outpoint))
                        .collect(),
                    output: vec![TxOut {
                        value: chunk.amount,
                        script_pubkey: dest.script_pubkey(),
                    }],
                    special_transaction_payload: None,
                };
                let to_sign: Vec<(usize, ScriptBuf, DerivationPath)> = picked
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        c.path
                            .clone()
                            .map(|p| (i, c.coin.utxo.txout.script_pubkey.clone(), p))
                    })
                    .collect();
                if let Err(e) = sign_p2pkh_inputs(&signer, &mut tx, &to_sign, SIGHASH_ALL).await {
                    result.failure_code = Some(e.code().to_string());
                    break;
                }
                let outpoints: Vec<OutPoint> = tx.input.iter().map(|i| i.previous_output).collect();
                this.spends.add(wallet_id, outpoints.iter().copied());
                match wallet.core().broadcast_transaction(&tx).await {
                    Ok(txid) => {
                        this.hub.note_announced(txid);
                        result.txids.push(txid.to_string());
                        result.moved += chunk.amount + chunk.fee;
                        result.remaining -= chunk.amount + chunk.fee;
                    }
                    Err(e) => {
                        this.spends.remove(&wallet_id, outpoints);
                        result.failure_code = Some(
                            if e.to_string().to_lowercase().contains("no peers") {
                                "coinjoin.no_peers"
                            } else {
                                "coinjoin.broadcast_rejected"
                            }
                            .to_string(),
                        );
                        break;
                    }
                }
            }
            if let Ok(manager) = this.manager() {
                this.refresh_wallet_state(&manager, wallet_id).await;
            }
            this.hub.pump.mark_balances(wallet_id);
            this.hub.pump.mark_history(wallet_id, None);
            Ok(result)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// The mixing loop
// ---------------------------------------------------------------------------

/// Ticks once a second until stopped; runs the denominating step every
/// 5–15 ticks (client.cpp:2439-2455).
async fn mixing_loop(
    session: Weak<NetworkSession>,
    wallet_id: WalletId,
    mut stop: watch::Receiver<bool>,
) {
    use dw_coinjoin::denoms::{AUTO_TIMEOUT_MAX, AUTO_TIMEOUT_MIN};
    let mut next_run = AUTO_TIMEOUT_MIN;
    let mut tick = 0u64;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(TICK) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
        tick += 1;
        let Some(this) = session.upgrade() else {
            return;
        };
        let Ok(_op) = this.try_enter() else { return };
        if !this.coinjoin.settings().enabled {
            this.halt_mixing(wallet_id, StopReason::Disabled);
            this.refresh_coinjoin_status(wallet_id).await;
            return;
        }
        // A vault that locks stops mixing (QT-112, m3-engine.md §5).
        if matches!(
            this.vault.lock_state(),
            LockState::Locked | LockState::NoVault
        ) {
            this.halt_mixing(wallet_id, StopReason::VaultLocked);
            this.refresh_coinjoin_status(wallet_id).await;
            return;
        }
        if this
            .manager()
            .ok()
            .and_then(|m| m.get_wallet_blocking(&wallet_id.0))
            .is_none()
        {
            this.coinjoin_wallet_gone(wallet_id);
            return;
        }
        this.expire_held(wallet_id);
        this.coinjoin
            .queues
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .expire(now_secs() as i64);
        if tick >= next_run {
            if let Err(e) = this.auto_denominate(wallet_id).await {
                tracing::warn!(%wallet_id, error = %e, "CoinJoin step failed");
                if matches!(e, EngineError::CoinJoin(CoinJoinFailure::VaultLocked)) {
                    this.halt_mixing(wallet_id, StopReason::VaultLocked);
                    this.refresh_coinjoin_status(wallet_id).await;
                    return;
                }
            }
            next_run = tick
                + AUTO_TIMEOUT_MIN
                + rand::random::<u64>() % (AUTO_TIMEOUT_MAX - AUTO_TIMEOUT_MIN + 1);
        }
    }
}

// ---------------------------------------------------------------------------
// The wallet side of a session
// ---------------------------------------------------------------------------

struct EngineMixingWallet {
    session: Weak<NetworkSession>,
    wallet_id: WalletId,
    /// Addresses reserved for this session's outputs.
    reserved: Mutex<Vec<Address>>,
}

impl EngineMixingWallet {
    fn session(&self) -> Result<Arc<NetworkSession>, WalletError> {
        self.session
            .upgrade()
            .ok_or_else(|| WalletError("session closed".into()))
    }
}

impl MixingWallet for EngineMixingWallet {
    async fn ready_to_mix(&self, amount: u64) -> Result<Vec<MixCoin>, WalletError> {
        let s = self.session()?;
        let view = s
            .mix_view(self.wallet_id)
            .await
            .map_err(|e| WalletError(e.to_string()))?;
        Ok(view.ready_to_mix(amount))
    }

    async fn reserve_scripts(&self, count: usize) -> Result<Vec<ScriptBuf>, WalletError> {
        let s = self.session()?;
        let addrs = s
            .reserve_coinjoin_addresses(self.wallet_id, count, false)
            .await
            .map_err(|e| WalletError(e.to_string()))?;
        let scripts = addrs.iter().map(Address::script_pubkey).collect();
        self.reserved
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .extend(addrs);
        Ok(scripts)
    }

    async fn release_scripts(&self, scripts: &[ScriptBuf]) {
        let Ok(s) = self.session() else { return };
        let addrs: Vec<Address> = {
            let mut reserved = self.reserved.lock().unwrap_or_else(|p| p.into_inner());
            let (release, keep): (Vec<Address>, Vec<Address>) = reserved
                .drain(..)
                .partition(|a| scripts.contains(&a.script_pubkey()));
            *reserved = keep;
            release
        };
        s.release_coinjoin_addresses(self.wallet_id, &addrs).await;
    }

    async fn keep_scripts(&self, scripts: &[ScriptBuf]) {
        // They stay reserved until the mixing transaction pays them.
        self.reserved
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|a| !scripts.contains(&a.script_pubkey()));
    }

    fn lock_coins(&self, outpoints: &[OutPoint]) {
        if let Ok(s) = self.session() {
            s.spends.add(self.wallet_id, outpoints.iter().copied());
        }
    }

    fn unlock_coins(&self, outpoints: &[OutPoint]) {
        if let Ok(s) = self.session() {
            s.spends.remove(&self.wallet_id, outpoints.iter().copied());
        }
    }

    fn hold_until_spent(&self, _outpoints: &[OutPoint]) {
        // The inputs stay in the session's pending spends; the loop records
        // them as held (with the time) when the session reports success and
        // releases them after the observation timeout.
    }

    async fn sign_inputs(
        &self,
        tx: &Transaction,
        inputs: &[(usize, ScriptBuf)],
    ) -> Result<Vec<TxIn>, WalletError> {
        let s = self.session()?;
        let signer = s
            .vault
            .mixing_signer(&self.wallet_id.0)
            .map_err(|e| WalletError(e.to_string()))?;
        let wallet = s
            .wallet(&self.wallet_id)
            .await
            .map_err(|e| WalletError(e.to_string()))?;
        let network = s.network.core_network();
        let to_sign: Vec<(usize, ScriptBuf, DerivationPath)> = {
            let state = wallet.state().await;
            inputs
                .iter()
                .map(|(i, script)| {
                    let addr = Address::from_script(script, network)
                        .map_err(|e| WalletError(e.to_string()))?;
                    let path = path_of(&state.core_wallet, &addr)
                        .ok_or_else(|| WalletError(format!("no key for {addr}")))?;
                    Ok((*i, script.clone(), path))
                })
                .collect::<Result<_, WalletError>>()?
        };
        let mut signed = tx.clone();
        sign_p2pkh_inputs(
            &signer,
            &mut signed,
            &to_sign,
            dw_coinjoin::client::MIXING_SIGHASH,
        )
        .await
        .map_err(|e| WalletError(e.to_string()))?;
        Ok(inputs
            .iter()
            .map(|(i, _)| signed.input[*i].clone())
            .collect())
    }
}

impl NetworkSession {
    /// Signs a CoinJoin-page payment (QT-051): `inputs` (fully mixed coins
    /// of the CoinJoin account, in random order) to `outputs`, no change,
    /// `SIGHASH_ALL`, with a signer the caller's `Spend` grant issued.
    pub(crate) async fn sign_coinjoin_payment(
        &self,
        wallet_id: WalletId,
        inputs: &[Utxo],
        outputs: Vec<TxOut>,
        signer: &VaultSigner,
    ) -> Result<Transaction, EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        let mut inputs = inputs.to_vec();
        inputs.shuffle(&mut rand::thread_rng());
        let to_sign: Vec<(usize, ScriptBuf, DerivationPath)> = {
            let state = wallet.state().await;
            inputs
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    path_of(&state.core_wallet, &u.address)
                        .map(|p| (i, u.txout.script_pubkey.clone(), p))
                        .ok_or_else(|| EngineError::Internal(format!("no key for {}", u.address)))
                })
                .collect::<Result<_, _>>()?
        };
        let mut tx = Transaction {
            version: 2,
            lock_time: 0,
            input: inputs.iter().map(|u| unsigned_input(u.outpoint)).collect(),
            output: outputs,
            special_transaction_payload: None,
        };
        sign_p2pkh_inputs(signer, &mut tx, &to_sign, SIGHASH_ALL).await?;
        Ok(tx)
    }
}

/// Candidates of the CoinJoin send page (QT-051): fully mixed, final, free
/// coins of the CoinJoin account.
pub(crate) fn fully_mixed_candidates(view: &MixView) -> Vec<Utxo> {
    view.fully_mixed_spendable()
        .map(|c| c.coin.utxo.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qt_046_settings_round_trip_and_validation() {
        let s = CoinJoinSettings {
            enabled: false,
            multi_session: true,
            max_sessions: 7,
            rounds: 8,
            target_amount_dash: 250,
            denoms_goal: 60,
            denoms_hard_cap: 400,
        };
        assert_eq!(decode_settings(&encode_settings(&s)), Some(s));
        assert_eq!(decode_settings("rounds=99"), None);
        assert_eq!(decode_settings("garbage"), None);
        assert_eq!(
            decode_settings("rounds=6"),
            Some(CoinJoinSettings {
                rounds: 6,
                ..CoinJoinSettings::default()
            })
        );
    }

    fn coin(
        n: u8,
        value: u64,
        coinjoin: bool,
        rounds: i32,
        fully_mixed: bool,
        address: &str,
    ) -> MixViewCoin {
        use std::str::FromStr;
        let address = Address::from_str(address).unwrap().assume_checked();
        let mut utxo = Utxo::new(
            OutPoint::new(Txid::from_byte_array([n; 32]), 0),
            TxOut {
                value,
                script_pubkey: address.script_pubkey(),
            },
            address,
            100,
            false,
        );
        utxo.is_confirmed = true;
        let path = if coinjoin {
            "m/9'/1'/4'/0'/0/1"
        } else {
            "m/44'/1'/0'/0/1"
        };
        MixViewCoin {
            coin: WalletCoin {
                utxo,
                send_account: !coinjoin,
                coinjoin_account: coinjoin,
                is_change: false,
                block_time: None,
                chain_locked: false,
                foreign_incoming: true,
                user_locked: false,
                reserved: false,
            },
            path: Some(path.parse().unwrap()),
            rounds,
            fully_mixed,
        }
    }

    const A: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";
    const B: &str = "yfGodgKENPQ1ePSRfbEHfuz1BBPD48vsDk";
    const C: &str = "yTcx2EJ24TUobNtGfN8UKEQZ5fUziT8yRE";

    #[test]
    fn test_qt_041_balances_follow_core_definitions() {
        let d = DENOMINATIONS[2]; // 0.100001
        let coins = vec![
            // Ordinary coin: anonymizable (one address above the minimum).
            coin(
                1,
                50_000_000,
                false,
                rounds::ROUNDS_NOT_DENOMINATED,
                false,
                A,
            ),
            // Denominations: 0, 2 and 4 rounds; the last is fully mixed.
            coin(2, d, true, 0, false, B),
            coin(3, d, true, 2, false, C),
            coin(4, d, true, 4, true, C),
            // A collateral is neither anonymizable nor denominated.
            coin(5, 40_000, true, rounds::ROUNDS_COLLATERAL, false, B),
        ];
        let (b, average) = compute_balances(&coins, dashcore::Network::Testnet, 4);
        assert_eq!(b.denominated, 3 * d);
        assert_eq!(b.fully_mixed, d);
        // value · min(rounds, 4) / 4: 0 + d/2 + d.
        assert_eq!(b.normalized_anonymized, d * 2 / 4 + d);
        // A's coin plus the not-fully-mixed denominations (B and C groups).
        assert_eq!(b.anonymizable, 50_000_000 + d + d);
        assert_eq!(average, 2.0);
    }

    #[test]
    fn test_qt_051_fully_mixed_candidates_need_final_free_coins() {
        let d = DENOMINATIONS[3];
        let mut pending = coin(1, d, true, 4, true, B);
        pending.coin.utxo.is_confirmed = false;
        let mut locked = coin(2, d, true, 4, true, B);
        locked.coin.reserved = true;
        let view = MixView {
            height: 200,
            coins: vec![
                coin(3, d, true, 4, true, C),
                coin(4, d, true, 2, false, C),
                pending,
                locked,
                coin(
                    5,
                    50_000_000,
                    false,
                    rounds::ROUNDS_NOT_DENOMINATED,
                    false,
                    A,
                ),
            ],
            balances: CoinJoinBalances::default(),
            total: 0,
            average_rounds: 0.0,
        };
        let got = fully_mixed_candidates(&view);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].outpoint.txid, Txid::from_byte_array([3; 32]));
        // Ready to mix: the 2-round coin only (not fully mixed, final, free).
        let ready = view.ready_to_mix(d);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].rounds, 2);
    }
}
