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
use key_wallet::transaction_checking::TransactionContext;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::{Signer, Utxo};
use rand::seq::SliceRandom;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::coins::{CoinFence, WalletCoin, now_secs};
use crate::history::TxEntry;
use crate::send::SpendOwner;
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
    /// The coins a move or consolidation would spend are not those of the
    /// plan the user confirmed (another set, or a higher fee): plan again.
    PlanChanged,
    /// The recovery scan needs a running SPV client.
    SpvNotRunning,
    NoPeers,
    /// A peer rejected a sweep transaction; the reason is Core's.
    BroadcastRejected(String),
    /// "Move mixed coins" and its consolidation wait for mixing to stop:
    /// the wallet mixes, or a session of it is still open.
    MixingActive,
    /// Mixing, or another move, waits for a running move of the wallet's
    /// mixed coins to end.
    MoveRunning,
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
            Self::PlanChanged => f.write_str("the coins to move changed since the plan"),
            Self::SpvNotRunning => f.write_str("spv is not running"),
            Self::NoPeers => f.write_str("no connected peers"),
            Self::BroadcastRejected(r) => write!(f, "broadcast rejected: {r}"),
            Self::MixingActive => f.write_str("stop mixing first"),
            Self::MoveRunning => f.write_str("a move of the mixed coins is running"),
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
    /// Every free confirmed coin of the CoinJoin account, mixed or not
    /// (collaterals, change, a consolidated output): what "Move mixed
    /// coins" can take, so what it is offered for.
    pub coinjoin_account: u64,
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
    /// Of `total`, the value of the coins with the wallet's rounds.
    pub fully_mixed: u64,
    /// Of `total`, the value of the CoinJoin account's other coins
    /// (denominations short of the rounds, collaterals, change): the move
    /// takes the whole account.
    pub not_fully_mixed: u64,
    /// The CoinJoin account's coins that wait for a confirmation, which
    /// the move leaves (an InstantSend-locked coin among them).
    pub unconfirmed: u32,
    /// What [`NetworkSession::move_mixed_coins`] takes to run this plan.
    pub quote_id: String,
}

/// The consolidation round before a move to Shielded (iOS #1221).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsolidationPlan {
    /// What leaves the coins swept: the outputs plus the fees. The outputs
    /// stay in the wallet's CoinJoin account.
    pub total: u64,
    /// One transaction each, to one fresh CoinJoin-account address.
    pub chunks: Vec<SweepChunkInfo>,
    pub fully_mixed: u64,
    pub not_fully_mixed: u64,
    /// The CoinJoin account's coins once the round confirms: its outputs
    /// and every coin it leaves (not final yet, held, dust).
    pub coins_after: u32,
    /// The account's coins that wait for a confirmation, which the round
    /// leaves (an InstantSend-locked coin among them).
    pub unconfirmed: u32,
    /// What [`NetworkSession::consolidate_mixed_coins`] takes to run this
    /// plan.
    pub quote_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepResult {
    pub txids: Vec<String>,
    /// What left the CoinJoin account in the chunks the network took:
    /// `received + fee`.
    pub moved: u64,
    /// Of `moved`, what their outputs pay.
    pub received: u64,
    /// Of `moved`, their fees.
    pub fee: u64,
    pub remaining: u64,
    pub failure_code: Option<String>,
    /// With `coinjoin.broadcast_unknown`: the chunk that may be on the
    /// network (display txid), its coins reserved until a block or an
    /// InstantSend lock shows it, or a block spends one of them otherwise.
    pub unknown_txid: Option<String>,
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
    /// Inputs of successful sessions waiting for the final transaction, per
    /// holding session (two sessions may have locked the same coin), with
    /// the time they were added.
    held: HashMap<(OutPoint, SpendOwner), u64>,
    /// Collateral inputs locked for open sessions, with each session's owner.
    collateral_inputs: HashMap<u64, (SpendOwner, Vec<OutPoint>)>,
    /// A move of the account's coins runs ([`CoinJoinRuntime::claim_sweep`]):
    /// mixing does not start until it ends.
    sweeping: bool,
    /// Recorded sweep chunks are being sent again
    /// ([`CoinJoinRuntime::claim_resend`]): no move starts until it ends.
    resending: bool,
}

/// A wallet's claim on its CoinJoin account for a move: while it lives no
/// mixing starts and no other move runs ([`CoinJoinRuntime::claim_sweep`]).
pub(crate) struct SweepClaim<'a> {
    runtime: &'a CoinJoinRuntime,
    wallet: WalletId,
    /// A resend's claim ([`CoinJoinRuntime::claim_resend`]).
    resend: bool,
}

impl Drop for SweepClaim<'_> {
    fn drop(&mut self) {
        if let Some(m) = self.runtime.mixers().get_mut(&self.wallet) {
            if self.resend {
                m.resending = false;
            } else {
                m.sweeping = false;
            }
        }
    }
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
    /// Each wallet's last plan per sweep target ([`SweepQuote`]).
    quotes: Mutex<HashMap<(WalletId, SweepTarget), SweepQuote>>,
    /// Tests: stand-ins for a sweep's broadcast and chunk size.
    #[cfg(test)]
    pub(crate) sweep_hooks: Mutex<sweep_test_hooks::Hooks>,
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
            quotes: Mutex::new(HashMap::new()),
            #[cfg(test)]
            sweep_hooks: Mutex::default(),
        }
    }

    fn mixers(&self) -> std::sync::MutexGuard<'_, HashMap<WalletId, Mixer>> {
        self.mixers.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn quotes(&self) -> std::sync::MutexGuard<'_, HashMap<(WalletId, SweepTarget), SweepQuote>> {
        self.quotes.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Issues the quote of `picked`, `wallet`'s plan to `target`, in place
    /// of the last one; returns its id.
    fn issue_quote(&self, wallet: WalletId, target: SweepTarget, picked: &SweepCoins) -> String {
        let quote = SweepQuote::of(picked);
        let id = quote.id.clone();
        self.quotes().insert((wallet, target), quote);
        id
    }

    /// Takes `wallet`'s quote `id` for `target`: `PlanChanged` unless it is
    /// the wallet's last one and covers `picked` ([`SweepQuote::covers`]).
    /// A matching id is used once, whatever the outcome; another id leaves
    /// the last quote as it is.
    fn redeem_quote(
        &self,
        wallet: WalletId,
        target: SweepTarget,
        id: &str,
        picked: &SweepCoins,
    ) -> Result<(), CoinJoinFailure> {
        let mut quotes = self.quotes();
        let key = (wallet, target);
        if !matches!(quotes.get(&key), Some(q) if q.id == id) {
            return Err(CoinJoinFailure::PlanChanged);
        }
        match quotes.remove(&key) {
            Some(q) if q.covers(picked) => Ok(()),
            _ => Err(CoinJoinFailure::PlanChanged),
        }
    }

    pub(crate) fn settings(&self) -> CoinJoinSettings {
        *self.settings.read().unwrap_or_else(|p| p.into_inner())
    }

    fn is_mixing(&self, id: &WalletId) -> bool {
        self.mixers().get(id).is_some_and(|m| m.mixing)
    }

    /// The wallet mixes, or a session of it is still open.
    fn busy(&self, id: &WalletId) -> bool {
        (self.mixers().get(id)).is_some_and(|m| m.mixing || !m.sessions.is_empty())
    }

    /// Claims `id`'s CoinJoin account for a move, through its last
    /// broadcast: refused while the wallet mixes or a session of it is open
    /// (`MixingActive`), or another move runs (`MoveRunning`). Checked and
    /// taken under the lock `start_mixing` sets the mixing flag under, so
    /// exactly one of the two wins.
    pub(crate) fn claim_sweep(&self, id: WalletId) -> Result<SweepClaim<'_>, CoinJoinFailure> {
        let mut mixers = self.mixers();
        let m = mixers.entry(id).or_default();
        if m.mixing || !m.sessions.is_empty() {
            return Err(CoinJoinFailure::MixingActive);
        }
        if m.sweeping || m.resending {
            return Err(CoinJoinFailure::MoveRunning);
        }
        m.sweeping = true;
        Ok(SweepClaim {
            runtime: self,
            wallet: id,
            resend: false,
        })
    }

    /// Claims `id`'s recorded sweep chunks for a send again: refused while
    /// a move of the wallet runs or another resend does (`MoveRunning`),
    /// whose chunks are its own to send. Mixing may run: the chunks'
    /// inputs read reserved to it.
    pub(crate) fn claim_resend(&self, id: WalletId) -> Result<SweepClaim<'_>, CoinJoinFailure> {
        let mut mixers = self.mixers();
        let m = mixers.entry(id).or_default();
        if m.sweeping || m.resending {
            return Err(CoinJoinFailure::MoveRunning);
        }
        m.resending = true;
        Ok(SweepClaim {
            runtime: self,
            wallet: id,
            resend: true,
        })
    }

    fn sweeping(&self, id: &WalletId) -> bool {
        self.mixers().get(id).is_some_and(|m| m.sweeping)
    }

    /// Tests: marks `id` mixing (or not) without starting its loop.
    #[cfg(test)]
    pub(crate) fn set_mixing_for_test(&self, id: WalletId, mixing: bool) {
        self.mixers().entry(id).or_default().mixing = mixing;
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

    /// The coins `CreateDenominated` (`denominations`) or
    /// `MakeCollateralAmounts` may fund from: every tally's (tests).
    #[cfg(test)]
    pub(crate) fn funding_inputs(
        &self,
        network: dashcore::Network,
        denominations: bool,
    ) -> Vec<OutPoint> {
        (self
            .tallies(network, denominations, denominations)
            .into_iter())
        .flat_map(|(_, coins)| coins.into_iter().map(|c| c.coin.utxo.outpoint))
        .collect()
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
    let coinjoin_account = (view.coins.iter())
        .filter(|c| c.coin.coinjoin_account && c.free() && c.coin.utxo.is_confirmed)
        .map(|c| c.value())
        .sum();
    (
        CoinJoinBalances {
            anonymizable,
            denominated,
            normalized_anonymized,
            fully_mixed,
            coinjoin_account,
        },
        average,
    )
}

/// The full derivation path of `address` in the wallet; a DashPay
/// contact's address is not the wallet's (MG-05 D19).
fn path_of(info: &ManagedWalletInfo, address: &Address) -> Option<DerivationPath> {
    crate::coins::own_address_info(info, address).map(|i| i.path)
}

/// `scriptSig` of a P2PKH input: `<DER signature ‖ sighash type> <pubkey>`.
pub(crate) fn p2pkh_script_sig(
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
pub(crate) async fn sign_p2pkh_inputs(
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
pub(crate) const SIGHASH_ALL: u32 = 1;

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

    /// The fenced wallet's coins with rounds, the fully-mixed rule and
    /// Core's balances, read under `fence` (`coins.rs`).
    pub(crate) async fn mix_view(&self, fence: &CoinFence) -> Result<MixView, EngineError> {
        let wallet_id = fence.wallet_id();
        let wallet = self.wallet(&wallet_id).await?;
        let snapshot = self.coin_snapshot(fence, &wallet).await?;
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

    /// [`Self::mix_view`] for an unattended spender (the automatic pass, a
    /// mixing session's inputs): `None` while the dispatch journal is not
    /// read, whose signed locks' inputs (O-2) would read free (review Opus
    /// closure C-1), or while a replay runs (0.1.x-bal §1).
    pub(crate) async fn pass_view(
        &self,
        fence: &CoinFence,
    ) -> Result<Option<MixView>, EngineError> {
        if !self.leases.journal_ready() {
            return Ok(None);
        }
        let view = (self.replay_guarded(fence.wallet_id(), self.mix_view(fence))).await;
        if let Err(EngineError::RescanInProgress | EngineError::RescanInterrupted) = view {
            self.set_mix_status(fence.wallet_id(), StatusCode::SyncInProgress);
            return Ok(None);
        }
        view.map(Some)
    }

    /// [`Self::mix_view`] under a fence taken for the read only: for
    /// callers that spend nothing of what they read.
    pub(crate) async fn read_mix_view(&self, wallet_id: WalletId) -> Result<MixView, EngineError> {
        let fence = self.coin_fence(wallet_id).await?;
        self.mix_view(&fence).await
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
                .spawn(move || {
                    rt.block_on(async {
                        let fence = this.coin_fence(wallet_id).await?;
                        this.compute_coinjoin_status(&fence).await
                    })
                })
                .join()
                .map_err(|_| EngineError::Internal("CoinJoin status computation panicked".into()))?
        })
    }

    /// The fenced wallet's status, computed and cached.
    async fn compute_coinjoin_status(
        &self,
        fence: &CoinFence,
    ) -> Result<CoinJoinStatus, EngineError> {
        let wallet_id = fence.wallet_id();
        let view = self.mix_view(fence).await?;
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
    /// Skipped while the wallet's coin fence is held (a build, a funding
    /// call, "Move mixed coins"): the cached status stays, and the
    /// refresher's next tick reads it again, so a held fence stalls no other
    /// wallet's status and no caller (`coins.rs`).
    pub(crate) async fn refresh_coinjoin_status(&self, wallet_id: WalletId) {
        let fence = match self.try_coin_fence(wallet_id).await {
            Ok(Some(fence)) => fence,
            Ok(None) => return,
            Err(e) => {
                tracing::debug!(%wallet_id, error = %e, "CoinJoin status refresh failed");
                return;
            }
        };
        let before = self
            .coinjoin
            .status
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&wallet_id)
            .cloned();
        match self.compute_coinjoin_status(&fence).await {
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
            if this.coinjoin.sweeping(&wallet_id) {
                return Err(CoinJoinFailure::MoveRunning.into());
            }
            let view = this.read_mix_view(wallet_id).await?;
            #[cfg(test)]
            test_pause(sweep_hooks(&this).start_after_read).await;
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
                // A move claimed the account since the check above.
                if m.sweeping {
                    return Err(CoinJoinFailure::MoveRunning.into());
                }
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
    /// released, except those of a session whose signatures are out, held
    /// until the wallet sees them or the observation timeout) then stop.
    /// Idempotent.
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
        let collateral: Vec<(SpendOwner, Vec<OutPoint>)>;
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
            collateral = m.collateral_inputs.drain().map(|(_, v)| v).collect();
        }
        // Sessions release their own inputs and outputs as they end; the
        // collateral inputs are released here.
        for (owner, inputs) in collateral {
            self.spends.release_some(&wallet_id, owner, inputs);
        }
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
    /// internal chain otherwise). Returns the txid. `fence`, the coin fence
    /// `coins` were read under, is released once the inputs are reserved
    /// and signed, before the broadcast (`coins.rs`).
    async fn commit_funding_tx(
        &self,
        fence: &mut Option<CoinFence>,
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
        if let Err(e) = sign_p2pkh_inputs(&signer, &mut tx, &to_sign, SIGHASH_ALL).await {
            self.release_coinjoin_addresses(wallet_id, &reserved).await;
            return Err(e);
        }
        // Reserved under the fence, as the signed transaction's own claim.
        let owner = SpendOwner::Tx(tx.txid());
        self.spends
            .reserve(wallet_id, owner, coins.iter().map(|c| c.coin.utxo.outpoint));
        drop(fence.take());
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
                self.spends.release(&wallet_id, owner);
                self.release_coinjoin_addresses(wallet_id, &reserved).await;
                Err(CoinJoinFailure::BroadcastRejected(e.to_string()).into())
            }
        }
    }

    /// `CreateDenominated(nBalanceToDenominate)` (client.cpp:2194-2224).
    async fn create_denominated(
        &self,
        fence: &mut Option<CoinFence>,
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
            let txid = self
                .commit_funding_tx(fence, wallet_id, coins, &plan)
                .await?;
            tracing::info!(%wallet_id, %txid, outputs = plan.outputs.len(), "CoinJoin denominations created");
            return Ok(true);
        }
        Ok(false)
    }

    /// `MakeCollateralAmounts()` (client.cpp:2022-2060).
    async fn make_collateral_amounts(
        &self,
        fence: &mut Option<CoinFence>,
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
                let txid = self
                    .commit_funding_tx(fence, wallet_id, coins, &plan)
                    .await?;
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
        // One fence over the pass, from its coin read through the signing
        // and reservation of what it spends: denomination and collateral
        // funding spend funding-account (BIP44) coins. A funding
        // transaction releases it before its broadcast; a session's own
        // denominations are reserved outside it (`coins.rs`).
        let fence = self.coin_fence(wallet_id).await?;
        let Some(view) = self.pass_view(&fence).await? else {
            return Ok(());
        };
        let mut fence = Some(fence);
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
                .create_denominated(&mut fence, wallet_id, &view, to_denominate)
                .await?
        {
            self.note_success_height(wallet_id, view.height);
            return Ok(());
        }
        if !view.has_collateral_inputs(true) {
            if !view.has_collateral_inputs(false)
                && self
                    .make_collateral_amounts(&mut fence, wallet_id, &view)
                    .await?
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
        let owner = SpendOwner::mixing_session();
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
            m.collateral_inputs
                .insert(key, (owner, vec![collateral_input]));
            m.status = Some(StatusCode::TryingToConnect);
            key
        };
        self.spends.reserve(wallet_id, owner, [collateral_input]);
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
            owner,
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
            this.spends
                .release_some(&wallet_id, owner, [collateral_input]);
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
                                m.held.insert((*i, owner), now);
                            }
                            m.last_success_height = height.max(1);
                            m.status = Some(StatusCode::Masternode(PoolMessage::Success));
                        }
                        // Stopped, timed out or cut off with our signatures
                        // out: held as a success's until the wallet sees
                        // the mixing transaction or the observation timeout.
                        SessionOutcome::SignedUnknown { inputs, failure } => {
                            tracing::info!(%wallet_id, %failure, "CoinJoin session ended after signing");
                            let now = now_secs();
                            for i in inputs {
                                m.held.insert((*i, owner), now);
                            }
                            m.status = Some(StatusCode::Masternode(PoolMessage::Session));
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
        let expired: Vec<(OutPoint, SpendOwner)> = {
            let mut mixers = self.coinjoin.mixers();
            let Some(m) = mixers.get_mut(&wallet_id) else {
                return;
            };
            // Gone from the reserved set: the wallet saw them spent.
            m.held.retain(|(o, _), _| reserved.contains(o));
            let expired: Vec<(OutPoint, SpendOwner)> = m
                .held
                .iter()
                .filter(|(_, at)| now.saturating_sub(**at) >= PENDING_OBSERVATION_SECS)
                .map(|(held, _)| *held)
                .collect();
            for held in &expired {
                m.held.remove(held);
            }
            expired
        };
        for (o, owner) in expired {
            self.spends.release_some(&wallet_id, owner, [o]);
        }
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
            let view = this.read_mix_view(wallet_id).await?;
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

    /// The CoinJoin account's coins a sweep to `target` takes, in its
    /// chunks: free (an input of a sweep chunk that may be on the network,
    /// [`SweepOut`], reads reserved), confirmed, spendable at the tip, on a
    /// CoinJoin path, above the threshold. Confirmed only, for a move and a
    /// consolidation alike: a chunk of up to 500 inputs with unconfirmed
    /// parents can exceed Core's ancestor limits (`-limitancestorcount` 25,
    /// `-limitancestorsize` 101 kvB, policy.h), and the wallet cannot count
    /// them: a mixing transaction's other inputs are foreign, their parents
    /// unknown to SPV. An InstantSend lock does not help (the parents stay
    /// in the mempool), and an unconfirmed input that never locks would
    /// hold every coin swept with it (iOS #1221). The rest wait for a
    /// confirmation (`unconfirmed`). With `prune`
    /// (a sweep, under the replay guard), the records that resolved are
    /// dropped first; a plan only reads.
    async fn sweep_coins(
        &self,
        wallet_id: WalletId,
        fence: &CoinFence,
        target: SweepTarget,
        prune: bool,
    ) -> Result<SweepCoins, EngineError> {
        // A stopped wallet's loop no longer expires its sessions' held
        // inputs: a session whose outcome stayed unknown frees them here.
        self.expire_held(wallet_id);
        let view = self.mix_view(fence).await?;
        if prune {
            self.prune_sweep_outs(wallet_id).await?;
        }
        let network = self.network.core_network();
        let account: Vec<&MixViewCoin> = view
            .coins
            .iter()
            .filter(|c| c.coin.coinjoin_account)
            .collect();
        let takes = |c: &MixViewCoin| {
            c.free()
                && c.path
                    .as_ref()
                    .is_some_and(|p| is_coinjoin_path(p, network))
        };
        let unconfirmed = (account.iter())
            .filter(|c| {
                takes(c)
                    && !c.coin.utxo.is_confirmed
                    && c.value() > dw_coinjoin::recovery::SWEEP_MIN_COIN
            })
            .count() as u32;
        let coins: Vec<MixViewCoin> = account
            .iter()
            .filter(|c| {
                takes(c) && c.coin.utxo.is_confirmed && c.coin.utxo.is_spendable(view.height)
            })
            .map(|c| (*c).clone())
            .collect();
        let values: Vec<u64> = coins.iter().map(MixViewCoin::value).collect();
        let max_inputs = sweep_max_inputs(self);
        let fee = crate::send::MIN_FEE_PER_KB;
        let chunks = match target {
            SweepTarget::Bip44 => {
                dw_coinjoin::recovery::plan_sweep_chunked(&values, fee, max_inputs)
            }
            SweepTarget::CoinJoin => {
                dw_coinjoin::recovery::plan_consolidation(&values, fee, max_inputs)
            }
        };
        // A consolidation pays for itself only when the drain needs it: more
        // coins than one shield lock takes, and fewer coins after it (m4).
        let taken: usize = chunks.iter().map(|c| c.coins.len()).sum();
        let pointless = target == SweepTarget::CoinJoin
            && (taken <= shield_lock_inputs(self) || chunks.len() >= taken);
        let chunks = if pointless { Vec::new() } else { chunks };
        let swept: usize = chunks.iter().map(|c| c.coins.len()).sum();
        Ok(SweepCoins {
            coins_after: (account.len() - swept + chunks.len()) as u32,
            unconfirmed,
            coins,
            chunks,
        })
    }

    /// Drops `wallet_id`'s sweep records that resolved or do not decode
    /// ([`split_sweep_outs`]).
    async fn prune_sweep_outs(&self, wallet_id: WalletId) -> Result<(), EngineError> {
        let rows = self.sweep_out_rows(wallet_id).await?;
        let (_, gone) = (self.hub.history).read(&wallet_id, |txs| split_sweep_outs(rows, txs));
        self.drop_sweep_outs(wallet_id, gone).await
    }

    /// `wallet_id`'s sweep record rows.
    async fn sweep_out_rows(
        &self,
        wallet_id: WalletId,
    ) -> Result<Vec<(String, String)>, EngineError> {
        let scope = dw_appdb::local_scope(&wallet_id.to_string());
        self.appdb_op(move |db| db.settings_with_prefix(&scope, SWEEP_OUT_PREFIX))
            .await
    }

    /// Deletes the sweep records `keys` of `wallet_id`, durably.
    async fn drop_sweep_outs(
        &self,
        wallet_id: WalletId,
        gone: Vec<String>,
    ) -> Result<(), EngineError> {
        if gone.is_empty() {
            return Ok(());
        }
        let scope = dw_appdb::local_scope(&wallet_id.to_string());
        self.appdb_op(move |db| {
            let writes: Vec<(&str, Option<&str>)> =
                gone.iter().map(|k| (k.as_str(), None)).collect();
            db.set_settings_durable(&scope, &writes)
        })
        .await
    }

    /// Records (`keep`) or drops the sweep chunks `txs`, with their signed
    /// bytes, durably, in one write.
    async fn write_sweep_outs(
        &self,
        wallet_id: WalletId,
        txs: &[&Transaction],
        keep: bool,
    ) -> Result<(), EngineError> {
        if txs.is_empty() {
            return Ok(());
        }
        let scope = dw_appdb::local_scope(&wallet_id.to_string());
        let rows: Vec<(String, Option<String>)> = txs
            .iter()
            .map(|tx| {
                let o = SweepOut { tx: (*tx).clone() };
                (o.key(), keep.then(|| o.value()))
            })
            .collect();
        self.appdb_op(move |db| {
            let writes: Vec<(&str, Option<&str>)> = rows
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_deref()))
                .collect();
            db.set_settings_durable(&scope, &writes)
        })
        .await
    }

    /// Refuses a sweep's plan while `wallet_id` mixes: a session's coins
    /// may be in a masternode's hands, and the account the sweep reads is
    /// the one mixing writes. The sweep itself takes the wallet's claim
    /// ([`CoinJoinRuntime::claim_sweep`]).
    fn refuse_while_mixing(&self, wallet_id: WalletId) -> Result<(), EngineError> {
        if self.coinjoin.busy(&wallet_id) {
            return Err(CoinJoinFailure::MixingActive.into());
        }
        Ok(())
    }

    /// The plan of a sweep to `target`: its chunks, how much of it is
    /// fully mixed, and the id of its quote, which replaces the wallet's
    /// last one for `target`. Reads coins only.
    async fn sweep_plan(
        self: &Arc<Self>,
        wallet_id: WalletId,
        target: SweepTarget,
    ) -> Result<(SweepCoins, MixedAmounts, String), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.refuse_while_mixing(wallet_id)?;
            let picked = {
                let fence = this.coin_fence(wallet_id).await?;
                this.sweep_coins(wallet_id, &fence, target, false).await?
            };
            if picked.chunks.is_empty() {
                return Err(CoinJoinFailure::NothingToMove.into());
            }
            let amounts = picked.amounts();
            let quote = this.coinjoin.issue_quote(wallet_id, target, &picked);
            Ok((picked, amounts, quote))
        })
        .await
    }

    /// What "Move mixed coins" would broadcast. Reads coins only. Refused
    /// while the wallet mixes (`coinjoin.mixing_active`).
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
        let (picked, amounts, quote_id) = self.sweep_plan(wallet_id, SweepTarget::Bip44).await?;
        Ok(SweepPlan {
            destination,
            total: picked.total(),
            chunks: picked.chunk_infos(),
            fully_mixed: amounts.fully_mixed,
            not_fully_mixed: amounts.not_fully_mixed,
            unconfirmed: picked.unconfirmed,
            quote_id,
        })
    }

    /// The consolidation round before a move to Shielded (iOS #1221): the
    /// CoinJoin account's confirmed coins in the fewest chunks of at most 500
    /// inputs, each to one fresh address of the same CoinJoin account.
    /// Reads coins only. Refused while the wallet mixes, and as pointless
    /// (`NothingToMove`) when the coins fit one shield lock or the chunks
    /// would not leave fewer coins.
    pub async fn mixed_coins_consolidation_plan(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<ConsolidationPlan, EngineError> {
        let (picked, amounts, quote_id) = self.sweep_plan(wallet_id, SweepTarget::CoinJoin).await?;
        Ok(ConsolidationPlan {
            total: picked.total(),
            chunks: picked.chunk_infos(),
            fully_mixed: amounts.fully_mixed,
            not_fully_mixed: amounts.not_fully_mixed,
            coins_after: picked.coins_after,
            unconfirmed: picked.unconfirmed,
            quote_id,
        })
    }

    /// "Move mixed coins" (IOS-057): broadcasts the sweep chunks to fresh
    /// BIP44 receive addresses of the same wallet. `quote_id`: the plan the
    /// user confirmed ([`SweepPlan::quote_id`]); `grant_id`: a `Spend` grant
    /// of at least the total. Stops at the first failing chunk. Refused
    /// while the wallet mixes.
    pub async fn move_mixed_coins(
        self: &Arc<Self>,
        wallet_id: WalletId,
        destination: SweepDestination,
        quote_id: String,
        grant_id: String,
    ) -> Result<SweepResult, EngineError> {
        if destination == SweepDestination::Shielded {
            return Err(EngineError::NotImplemented(
                "NetworkSession.move_mixed_coins.shielded".into(),
            ));
        }
        self.sweep(wallet_id, SweepTarget::Bip44, quote_id, grant_id)
            .await
    }

    /// Runs the consolidation round of [`Self::mixed_coins_consolidation_plan`]:
    /// every chunk pays one fresh address of the wallet's own CoinJoin
    /// account, never BIP44. `quote_id`: the plan the user confirmed
    /// ([`ConsolidationPlan::quote_id`]); `grant_id`: a `Spend` grant of at
    /// least the plan's total. Stops at the first failing chunk, as a move
    /// does.
    pub async fn consolidate_mixed_coins(
        self: &Arc<Self>,
        wallet_id: WalletId,
        quote_id: String,
        grant_id: String,
    ) -> Result<SweepResult, EngineError> {
        self.sweep(wallet_id, SweepTarget::CoinJoin, quote_id, grant_id)
            .await
    }

    /// Sends `wallet_id`'s recorded sweep chunks again: the bytes signed for
    /// the move, never signed again ("Try again" on a chunk of unknown
    /// outcome; every SPV peers 0 -> >0 transition, so at startup and on
    /// each reconnect, [`Self::attach_sweep_resend`]). `txid`: that chunk
    /// alone; one the wallet no longer records resolved earlier and reads
    /// `Resolved`. Records that resolved are dropped and read `Resolved`;
    /// any broadcast error keeps a record (`Unknown`), since an earlier
    /// send may have reached the network: only network evidence of the
    /// chunk or a confirmed conflict releases its inputs
    /// ([`sweep_out_state`]). Refused while a move of the wallet runs
    /// (`coinjoin.move_running`).
    pub async fn resend_mixed_coins(
        self: &Arc<Self>,
        wallet_id: WalletId,
        txid: Option<Txid>,
    ) -> Result<Vec<SweepResend>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let wallet = this.wallet(&wallet_id).await?;
            let _claim = this.coinjoin.claim_resend(wallet_id)?;
            let rows = this.sweep_out_rows(wallet_id).await?;
            let records: Vec<(String, Option<(SweepOut, SweepOutState)>)> =
                (this.hub.history).read(&wallet_id, |txs| {
                    (rows.into_iter())
                        .map(|(key, value)| {
                            let o = SweepOut::decode(&key, &value).map(|o| {
                                let state = sweep_out_state(&o, txs);
                                (o, state)
                            });
                            (key, o)
                        })
                        .collect()
                });
            let mut gone = Vec::new();
            let mut out = Vec::new();
            for (key, record) in records {
                let Some((o, state)) = record else {
                    gone.push(key);
                    continue;
                };
                let pending = state == SweepOutState::Pending;
                if !pending {
                    gone.push(key);
                }
                if txid.is_some_and(|t| t != o.txid()) {
                    continue;
                }
                let state = if !pending {
                    SweepResendState::Resolved
                } else {
                    match broadcast_sweep(&this, &wallet, &o.tx).await {
                        Ok(sent) => {
                            this.hub.note_announced(sent);
                            SweepResendState::Sent
                        }
                        Err(e) => {
                            tracing::info!(txid = %o.txid(), error = %e, "sweep chunk sent again: no verdict");
                            SweepResendState::Unknown
                        }
                    }
                };
                out.push(SweepResend {
                    txid: o.txid(),
                    state,
                });
            }
            if let Some(t) = txid
                && out.is_empty()
            {
                out.push(SweepResend {
                    txid: t,
                    state: SweepResendState::Resolved,
                });
            }
            let dropped = !gone.is_empty();
            this.drop_sweep_outs(wallet_id, gone).await?;
            if dropped {
                this.hub.pump.mark_balances(wallet_id);
            }
            Ok(out)
        })
        .await
    }

    /// Sends every wallet's recorded sweep chunks again on each SPV peers
    /// 0 -> >0 transition: at startup and on every reconnect.
    pub(crate) fn attach_sweep_resend(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.attach_peers_up(Box::new(move || {
            let weak = weak.clone();
            rt.spawn(async move {
                let Some(session) = weak.upgrade() else {
                    return;
                };
                let Ok(manager) = session.manager() else {
                    return;
                };
                for id in manager.list_wallet_ids_blocking() {
                    if let Err(e) = session.resend_mixed_coins(WalletId(id), None).await {
                        tracing::debug!(error = %e, "sweep chunks not sent again");
                    }
                }
            });
        }));
    }

    /// The sweep of [`Self::move_mixed_coins`] and
    /// [`Self::consolidate_mixed_coins`] to `target`.
    ///
    /// Every chunk is signed and its inputs reserved under the coin fence;
    /// the chunks are recorded with their bytes in `app.sqlite`
    /// ([`SweepOut`]) before the first is handed to the network, so a
    /// restart, which forgets the reservations, still keeps their inputs
    /// out of every spend (the coin snapshot reads the records) until each
    /// resolves, and can send them again. The broadcasts run in order and
    /// the first failure stops the sweep ([`settle_sweep_failure`]); the
    /// records of the chunks it never handed off are dropped.
    async fn sweep(
        self: &Arc<Self>,
        wallet_id: WalletId,
        target: SweepTarget,
        quote_id: String,
        grant_id: String,
    ) -> Result<SweepResult, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(CoinJoinFailure::WatchOnly.into());
            }
            // Through the last broadcast: no mixing starts meanwhile, and
            // none runs now.
            let _claim = this.coinjoin.claim_sweep(wallet_id)?;
            // A sweep takes every mixed coin: none while the dispatch
            // journal, which holds signed locks' inputs, is not read
            // (review Opus closure C-1).
            if !this.leases.journal_ready() {
                return Err(EngineError::Storage(
                    "the dispatch journal is not read".into(),
                ));
            }
            // Held from the coin read through every chunk's signing and
            // reservation; released before the broadcasts (`coins.rs`).
            let fence = this.coin_fence(wallet_id).await?;
            let picked = (this
                .replay_guarded(wallet_id, this.sweep_coins(wallet_id, &fence, target, true)))
            .await?;
            #[cfg(test)]
            test_pause(sweep_hooks(&this).after_read).await;
            if picked.chunks.is_empty() {
                return Err(CoinJoinFailure::NothingToMove.into());
            }
            // Under the fence: exactly the confirmed plan's coins, at no
            // higher fee, or nothing is signed.
            (this.coinjoin).redeem_quote(wallet_id, target, &quote_id, &picked)?;
            let total = picked.total();
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
                received: 0,
                fee: 0,
                remaining: total,
                failure_code: None,
                unknown_txid: None,
            };
            // Every chunk is signed and reserved under the fence, then
            // broadcast in order; the first failure stops the sweep.
            let mut signed: Vec<(Transaction, u64)> = Vec::with_capacity(picked.chunks.len());
            // Each signed chunk's fee, by index.
            let mut fees: Vec<u64> = Vec::with_capacity(picked.chunks.len());
            // A consolidation's reserved CoinJoin address, per signed chunk.
            let mut addresses: Vec<Address> = Vec::new();
            let mut sign_failure = None;
            let release_all = |signed: &[(Transaction, u64)]| {
                for (unsent, _) in signed {
                    this.spends
                        .release(&wallet_id, SpendOwner::Tx(unsent.txid()));
                }
            };
            for chunk in &picked.chunks {
                let dest = match target {
                    SweepTarget::Bip44 => wallet
                        .core()
                        .next_receive_address_for_account(0)
                        .await
                        .map_err(EngineError::from),
                    SweepTarget::CoinJoin => this
                        .reserve_coinjoin_addresses(wallet_id, 1, false)
                        .await
                        .and_then(|a| {
                            a.into_iter()
                                .next()
                                .ok_or_else(|| EngineError::Internal("no CoinJoin address".into()))
                        }),
                };
                let dest = match dest {
                    Ok(dest) => dest,
                    Err(e) => {
                        // Nothing was broadcast: give back every reservation.
                        release_all(&signed);
                        this.release_coinjoin_addresses(wallet_id, &addresses).await;
                        return Err(e);
                    }
                };
                let picked_coins: Vec<&MixViewCoin> =
                    chunk.coins.iter().map(|i| &picked.coins[*i]).collect();
                let mut tx = Transaction {
                    version: 2,
                    lock_time: 0,
                    input: picked_coins
                        .iter()
                        .map(|c| unsigned_input(c.coin.utxo.outpoint))
                        .collect(),
                    output: vec![TxOut {
                        value: chunk.amount,
                        script_pubkey: dest.script_pubkey(),
                    }],
                    special_transaction_payload: None,
                };
                let to_sign: Vec<(usize, ScriptBuf, DerivationPath)> = picked_coins
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        c.path
                            .clone()
                            .map(|p| (i, c.coin.utxo.txout.script_pubkey.clone(), p))
                    })
                    .collect();
                if let Err(e) = sign_p2pkh_inputs(&signer, &mut tx, &to_sign, SIGHASH_ALL).await {
                    sign_failure = Some(e.code().to_string());
                    if target == SweepTarget::CoinJoin {
                        this.release_coinjoin_addresses(wallet_id, &[dest]).await;
                    }
                    break;
                }
                this.spends.reserve(
                    wallet_id,
                    SpendOwner::Tx(tx.txid()),
                    tx.input.iter().map(|i| i.previous_output),
                );
                signed.push((tx, chunk.amount + chunk.fee));
                fees.push(chunk.fee);
                if target == SweepTarget::CoinJoin {
                    addresses.push(dest);
                }
            }
            let txs: Vec<&Transaction> = signed.iter().map(|(tx, _)| tx).collect();
            if let Err(e) = (this.write_sweep_outs(wallet_id, &txs, true)).await {
                release_all(&signed);
                this.release_coinjoin_addresses(wallet_id, &addresses).await;
                return Err(e);
            }
            drop(fence);
            for (n, (tx, value)) in signed.iter().enumerate() {
                #[cfg(test)]
                if let Some((at, pause)) = sweep_hooks(&this).before_broadcast
                    && at == n
                {
                    test_pause(Some(pause)).await;
                }
                match broadcast_sweep(&this, &wallet, tx).await {
                    Ok(txid) => {
                        this.hub.note_announced(txid);
                        result.txids.push(txid.to_string());
                        result.moved += value;
                        result.fee += fees[n];
                        result.received += value - fees[n];
                        result.remaining -= value;
                    }
                    Err(e) => {
                        let settled = settle_sweep_failure(&this.spends, wallet_id, &signed, n, &e);
                        let unsent: Vec<&Transaction> =
                            signed[settled.unsent..].iter().map(|(tx, _)| tx).collect();
                        if let Err(e) = this.write_sweep_outs(wallet_id, &unsent, false).await {
                            // Kept: the inputs stay held, and a resend sends
                            // the never-sent chunks then.
                            tracing::warn!(error = %e, "could not drop unsent sweep chunks");
                        }
                        if target == SweepTarget::CoinJoin {
                            let unused = addresses.get(settled.unsent..).unwrap_or_default();
                            this.release_coinjoin_addresses(wallet_id, unused).await;
                        }
                        result.failure_code = Some(settled.code);
                        result.unknown_txid = settled.unknown.map(|t| t.to_string());
                        break;
                    }
                }
            }
            if result.failure_code.is_none() {
                result.failure_code = sign_failure;
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

/// What sending a recorded sweep chunk again came to
/// ([`NetworkSession::resend_mixed_coins`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepResendState {
    /// The network took it (peer echo, InstantSend lock or a block).
    Sent,
    /// No verdict, or refused this time: the record stays, its inputs held,
    /// and it is sent again on the next reconnect.
    Unknown,
    /// A block or an InstantSend lock shows it, or a block spent one of
    /// its inputs in another transaction (or the wallet records no such
    /// chunk any more): the record is dropped.
    Resolved,
}

/// One recorded sweep chunk sent again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepResend {
    pub txid: Txid,
    pub state: SweepResendState,
}

/// Where a sweep pays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SweepTarget {
    /// A fresh BIP44 receive address per chunk ("Move mixed coins").
    Bip44,
    /// A fresh address of the same CoinJoin account per chunk (the
    /// consolidation before a move to Shielded).
    CoinJoin,
}

/// A sweep's coins and chunks ([`NetworkSession::sweep_coins`]).
#[derive(Clone)]
struct SweepCoins {
    coins: Vec<MixViewCoin>,
    chunks: Vec<dw_coinjoin::recovery::SweepChunk>,
    /// The CoinJoin account's coins once the chunks confirm: each chunk's
    /// output and every coin the sweep leaves.
    coins_after: u32,
    /// The CoinJoin account's coins that wait for a confirmation.
    unconfirmed: u32,
}

/// A plan as the user confirms it (Sol R3): its chunks' inputs, and their
/// fee as a ceiling. The sweep it confirms runs only on the same chunks at
/// no higher fee, compared under the coin fence; the coins may change in
/// between (a payment arrives, another spend takes one, a fee rule moves).
#[derive(Debug, Clone, PartialEq, Eq)]
struct SweepQuote {
    id: String,
    chunks: Vec<Vec<OutPoint>>,
    fee: u64,
}

impl SweepQuote {
    fn of(picked: &SweepCoins) -> Self {
        SweepQuote {
            id: hex::encode(rand::random::<[u8; 16]>()),
            chunks: picked.chunk_inputs(),
            fee: picked.fee(),
        }
    }

    /// Whether `picked`, read for the sweep, is the quoted plan: the same
    /// chunks of the same inputs, at no higher fee.
    fn covers(&self, picked: &SweepCoins) -> bool {
        picked.chunk_inputs() == self.chunks && picked.fee() <= self.fee
    }
}

/// What a sweep moves, by how mixed it is.
struct MixedAmounts {
    fully_mixed: u64,
    not_fully_mixed: u64,
}

impl SweepCoins {
    fn total(&self) -> u64 {
        self.chunks.iter().map(|c| c.amount + c.fee).sum()
    }

    fn fee(&self) -> u64 {
        self.chunks.iter().map(|c| c.fee).sum()
    }

    /// Each chunk's inputs, in order.
    fn chunk_inputs(&self) -> Vec<Vec<OutPoint>> {
        (self.chunks.iter())
            .map(|c| {
                (c.coins.iter())
                    .map(|&i| self.coins[i].coin.utxo.outpoint)
                    .collect()
            })
            .collect()
    }

    fn chunk_infos(&self) -> Vec<SweepChunkInfo> {
        self.chunks
            .iter()
            .map(|c| SweepChunkInfo {
                inputs: c.coins.len() as u32,
                amount: c.amount,
                fee: c.fee,
            })
            .collect()
    }

    fn amounts(&self) -> MixedAmounts {
        let mut out = MixedAmounts {
            fully_mixed: 0,
            not_fully_mixed: 0,
        };
        for coin in self
            .chunks
            .iter()
            .flat_map(|c| &c.coins)
            .map(|i| &self.coins[*i])
        {
            if coin.fully_mixed {
                out.fully_mixed += coin.value();
            } else {
                out.not_fully_mixed += coin.value();
            }
        }
        out
    }
}

/// The key prefix, in a wallet's local scope of `app.sqlite`, of its sweep
/// chunks that may be on the network ([`SweepOut`]).
pub(crate) const SWEEP_OUT_PREFIX: &str = "coinjoin.sweep_out.";

/// A sweep chunk that may be on the network, with its signed bytes:
/// recorded before its first broadcast, so a restart neither forgets its
/// inputs nor what to send again. Its inputs are in no spend (a sweep, a
/// send, mixing, a drain of the account) until it resolves
/// ([`sweep_out_state`]): a block or an InstantSend lock shows it, a block spends one of its
/// inputs in another transaction, or its first broadcast was refused before
/// anything was sent. Elapsed time resolves nothing: a signed transaction
/// stays valid until a conflict confirms, and v24 keeps one in its mempool
/// for 336 hours (`DEFAULT_MEMPOOL_EXPIRY_HOURS`, mempool_options.h:29) and
/// any peer may relay it later. The bytes are sent again, never signed
/// again ([`NetworkSession::resend_mixed_coins`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SweepOut {
    pub(crate) tx: Transaction,
}

impl SweepOut {
    pub(crate) fn txid(&self) -> Txid {
        self.tx.txid()
    }

    pub(crate) fn inputs(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.tx.input.iter().map(|i| i.previous_output)
    }

    pub(crate) fn key(&self) -> String {
        format!("{SWEEP_OUT_PREFIX}{}", self.txid())
    }

    /// The transaction, consensus-encoded, in hex.
    pub(crate) fn value(&self) -> String {
        dashcore::consensus::encode::serialize_hex(&self.tx)
    }

    /// The record of a row; `None` unless its bytes are a transaction with
    /// inputs whose txid the key names.
    pub(crate) fn decode(key: &str, value: &str) -> Option<Self> {
        let txid: Txid = key.strip_prefix(SWEEP_OUT_PREFIX)?.parse().ok()?;
        let bytes = hex::decode(value).ok()?;
        let tx: Transaction = dashcore::consensus::deserialize(&bytes).ok()?;
        (tx.txid() == txid && !tx.input.is_empty()).then_some(Self { tx })
    }
}

/// Where a recorded sweep chunk stands against the wallet's history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SweepOutState {
    /// Neither seen nor conflicted: its inputs stay held. The wallet's own
    /// mempool record of it is no evidence: dash-spv hands a broadcast to
    /// the wallet before any peer has it (BL-76).
    Pending,
    /// A block holds it, or an InstantSend lock: the network took it, and
    /// the wallet's own records spend its inputs from now on.
    Seen,
    /// A block spends one of its inputs in another transaction: it can
    /// never confirm, and its other inputs are free. A reorg out of that
    /// block is not watched for (rare under ChainLocks).
    Conflicted,
}

/// [`SweepOutState`] of `o` in the wallet's history `txs`.
pub(crate) fn sweep_out_state(o: &SweepOut, txs: &HashMap<Txid, TxEntry>) -> SweepOutState {
    let txid = o.txid();
    if txs.get(&txid).is_some_and(|e| {
        e.context.block_info().is_some()
            || e.instant_locked
            || matches!(e.context, TransactionContext::InstantSend(_))
    }) {
        return SweepOutState::Seen;
    }
    let inputs: HashSet<OutPoint> = o.inputs().collect();
    let conflicted = txs.iter().any(|(t, e)| {
        *t != txid
            && e.context.block_info().is_some()
            && e.tx
                .input
                .iter()
                .any(|i| inputs.contains(&i.previous_output))
    });
    if conflicted {
        SweepOutState::Conflicted
    } else {
        SweepOutState::Pending
    }
}

/// Splits a wallet's sweep records (`app.sqlite` rows) into the pending
/// ones, whose inputs stay held, and the keys of the rest: resolved
/// ([`sweep_out_state`]) or not decodable (no inputs to hold, nothing to
/// send).
pub(crate) fn split_sweep_outs(
    rows: Vec<(String, String)>,
    txs: &HashMap<Txid, TxEntry>,
) -> (Vec<SweepOut>, Vec<String>) {
    let (mut pending, mut gone) = (Vec::new(), Vec::new());
    for (key, value) in rows {
        match SweepOut::decode(&key, &value) {
            Some(o) if sweep_out_state(&o, txs) == SweepOutState::Pending => pending.push(o),
            _ => gone.push(key),
        }
    }
    (pending, gone)
}

/// The inputs the pending records hold.
pub(crate) fn held_inputs(pending: &[SweepOut]) -> HashSet<OutPoint> {
    pending.iter().flat_map(SweepOut::inputs).collect()
}

/// Tests: a session's stand-ins for a sweep's broadcast and its chunk
/// size ([`CoinJoinRuntime::sweep_hooks`]).
#[cfg(test)]
pub(crate) mod sweep_test_hooks {
    use std::sync::Arc;

    use dashcore::{Transaction, Txid};

    pub(crate) type Broadcast = Arc<
        dyn Fn(&Transaction) -> Result<Txid, Box<platform_wallet::PlatformWalletError>>
            + Send
            + Sync,
    >;

    #[derive(Clone, Default)]
    pub(crate) struct Hooks {
        pub(crate) broadcast: Option<Broadcast>,
        pub(crate) max_inputs: Option<usize>,
        /// The shield lock's input cap a consolidation must exceed.
        pub(crate) lock_inputs: Option<usize>,
        /// A sweep waits here once it read its coins, under the fence.
        pub(crate) after_read: Option<Arc<crate::coins::TestPause>>,
        /// A sweep waits here before broadcasting chunk `n`.
        pub(crate) before_broadcast: Option<(usize, Arc<crate::coins::TestPause>)>,
        /// `start_mixing` waits here once it read the mix view.
        pub(crate) start_after_read: Option<Arc<crate::coins::TestPause>>,
    }
}

/// Tests: marks `pause` reached and waits for its release.
#[cfg(test)]
async fn test_pause(pause: Option<Arc<crate::coins::TestPause>>) {
    if let Some(p) = pause {
        p.reached.store(true, std::sync::atomic::Ordering::SeqCst);
        p.release.notified().await;
    }
}

#[cfg(test)]
fn sweep_hooks(session: &NetworkSession) -> sweep_test_hooks::Hooks {
    (session.coinjoin.sweep_hooks.lock())
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

/// The inputs one shield asset lock takes: a consolidation of fewer coins
/// buys the drain nothing.
fn shield_lock_inputs(_session: &NetworkSession) -> usize {
    #[cfg(test)]
    if let Some(max) = sweep_hooks(_session).lock_inputs {
        return max;
    }
    crate::platform::shielded_pool::MAX_LOCK_INPUTS
}

/// Inputs per sweep chunk.
fn sweep_max_inputs(_session: &NetworkSession) -> usize {
    #[cfg(test)]
    if let Some(max) = sweep_hooks(_session).max_inputs {
        return max;
    }
    dw_coinjoin::recovery::SWEEP_MAX_INPUTS
}

/// Hands a signed sweep chunk to the network.
async fn broadcast_sweep(
    _session: &NetworkSession,
    wallet: &platform_wallet::PlatformWallet,
    tx: &Transaction,
) -> Result<Txid, Box<platform_wallet::PlatformWalletError>> {
    #[cfg(test)]
    if let Some(hook) = sweep_hooks(_session).broadcast {
        return hook(tx);
    }
    wallet
        .core()
        .broadcast_transaction(tx)
        .await
        .map_err(Box::new)
}

/// How a sweep that stopped at a chunk's broadcast ends.
struct SweepSettled {
    code: String,
    /// The chunk that may be on the network.
    unknown: Option<Txid>,
    /// The first chunk never handed to the network: it and every later
    /// one were released.
    unsent: usize,
}

/// "Move mixed coins" stopped at chunk `n` of `signed` with `e`: releases
/// the claims of the chunks never handed to the network and returns the
/// sweep's failure code. Only dash-spv's rejection before the network took
/// the bytes (`TransactionBroadcast`) leaves chunk `n` definitely unsent;
/// any other error (`TransactionBroadcastUnconfirmed` among them) may have
/// put it on the network, so its claim stays until the wallet sees its
/// coins spent (`coinjoin.broadcast_unknown`), as the funding transaction's
/// does, and its txid is returned for the host to check on (review E0-04
/// P2b F5). The chunks after `n` were never handed off.
fn settle_sweep_failure(
    spends: &crate::send::PendingSpends,
    wallet_id: WalletId,
    signed: &[(Transaction, u64)],
    n: usize,
    e: &platform_wallet::PlatformWalletError,
) -> SweepSettled {
    let (unsent, code, unknown) = match e {
        platform_wallet::PlatformWalletError::TransactionBroadcast(reason) => (
            n,
            if reason.to_lowercase().contains("no peers") {
                "coinjoin.no_peers"
            } else {
                "coinjoin.broadcast_rejected"
            },
            None,
        ),
        _ => (
            n + 1,
            "coinjoin.broadcast_unknown",
            signed.get(n).map(|(tx, _)| tx.txid()),
        ),
    };
    let unsent = unsent.min(signed.len());
    for (tx, _) in &signed[unsent..] {
        spends.release(&wallet_id, SpendOwner::Tx(tx.txid()));
    }
    SweepSettled {
        code: code.to_owned(),
        unknown,
        unsent,
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
    /// The session's claim on the coins it locks (DEC-179).
    owner: SpendOwner,
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
        // Read under a fence released at once: a session's denominations
        // are reserved outside it (`coins.rs`).
        let fence = (s.coin_fence(self.wallet_id).await).map_err(|e| WalletError(e.to_string()))?;
        let view = (s.pass_view(&fence).await).map_err(|e| WalletError(e.to_string()))?;
        Ok(view.map(|v| v.ready_to_mix(amount)).unwrap_or_default())
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
            s.spends
                .reserve(self.wallet_id, self.owner, outpoints.iter().copied());
        }
    }

    fn unlock_coins(&self, outpoints: &[OutPoint]) {
        if let Ok(s) = self.session() {
            s.spends
                .release_some(&self.wallet_id, self.owner, outpoints.iter().copied());
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
        // An input of an asset lock a funding call signed, its step
        // unresolved, is not signed (review P2b r7 O-2): read under the
        // coin fence, which a funding call keeps until its lock is
        // recorded, and kept through the signing.
        let _fence = s
            .coin_fence(self.wallet_id)
            .await
            .map_err(|e| WalletError(e.to_string()))?;
        if !s.leases.journal_ready() {
            return Err(WalletError("the dispatch journal is not read".into()));
        }
        let held = s.leases.unresolved_lock_inputs(self.wallet_id);
        if let Some(o) = (inputs.iter())
            .map(|(i, _)| tx.input[*i].previous_output)
            .find(|o| held.contains(o))
        {
            return Err(WalletError(format!(
                "input {o} is held: an asset lock spends it, unresolved (Resolve stuck funding)"
            )));
        }
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
    /// A mixing session's final-input signing of `wallet_id`'s `inputs` of
    /// `tx` (tests).
    #[cfg(test)]
    pub(crate) async fn sign_mixing_inputs(
        self: &Arc<Self>,
        wallet_id: WalletId,
        tx: &Transaction,
        inputs: &[(usize, ScriptBuf)],
    ) -> Result<Vec<TxIn>, WalletError> {
        let adapter = EngineMixingWallet {
            session: Arc::downgrade(self),
            wallet_id,
            owner: SpendOwner::mixing_session(),
            reserved: Mutex::new(Vec::new()),
        };
        adapter.sign_inputs(tx, inputs).await
    }

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
#[path = "coinjoin_sweep_tests.rs"]
mod sweep_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// Review P2b r1 item 4: a sweep chunk whose broadcast outcome is
    /// unknown keeps its claim; only the chunks never handed off are
    /// released. A rejection before the network took it releases it too.
    #[test]
    fn a_sweep_keeps_the_claim_of_a_chunk_that_may_be_out() {
        use platform_wallet::PlatformWalletError as E;
        let wallet = WalletId([1; 32]);
        let chunk = |i: u8| {
            let tx = Transaction {
                version: 2,
                lock_time: 0,
                input: vec![unsigned_input(OutPoint::new(
                    dashcore::Txid::from_byte_array([i; 32]),
                    0,
                ))],
                output: Vec::new(),
                special_transaction_payload: None,
            };
            (tx, 1_000)
        };
        let signed: Vec<(Transaction, u64)> = (1..=3).map(chunk).collect();
        let outpoint = |n: usize| signed[n].0.input[0].previous_output;
        let reserve = |spends: &crate::send::PendingSpends| {
            for (tx, _) in &signed {
                spends.reserve(
                    wallet,
                    SpendOwner::Tx(tx.txid()),
                    tx.input.iter().map(|i| i.previous_output),
                );
            }
        };

        let spends = crate::send::PendingSpends::default();
        reserve(&spends);
        let SweepSettled { code, unknown, .. } = settle_sweep_failure(
            &spends,
            wallet,
            &signed,
            1,
            &E::TransactionBroadcastUnconfirmed("no verdict".into()),
        );
        assert_eq!(code, "coinjoin.broadcast_unknown");
        assert_eq!(unknown, Some(signed[1].0.txid()), "the chunk to check on");
        assert_eq!(
            spends.snapshot(&wallet),
            HashSet::from([outpoint(0), outpoint(1)])
        );

        let spends = crate::send::PendingSpends::default();
        reserve(&spends);
        let SweepSettled { code, unknown, .. } = settle_sweep_failure(
            &spends,
            wallet,
            &signed,
            1,
            &E::TransactionBroadcast("rejected: bad-txns".into()),
        );
        assert_eq!(code, "coinjoin.broadcast_rejected");
        assert_eq!(unknown, None, "definitely unsent");
        assert_eq!(spends.snapshot(&wallet), HashSet::from([outpoint(0)]));

        let spends = crate::send::PendingSpends::default();
        reserve(&spends);
        let SweepSettled { code, unknown, .. } = settle_sweep_failure(
            &spends,
            wallet,
            &signed,
            2,
            &E::TransactionBroadcast("No peers connected".into()),
        );
        assert_eq!(code, "coinjoin.no_peers");
        assert_eq!(unknown, None);
        assert_eq!(
            spends.snapshot(&wallet),
            HashSet::from([outpoint(0), outpoint(1)])
        );
    }

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
                dust_locked: false,
                reserved: false,
                lock_input: false,
                unresolved_lock: false,
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
        // Every free coin of the CoinJoin account, the collateral too.
        assert_eq!(b.coinjoin_account, 3 * d + 40_000);
    }

    /// Opus m6 / Sol R4: the move card's balance counts what the plan
    /// moves even when no denomination is left: a collateral alone, or a
    /// consolidated output. A held or unconfirmed coin is not counted.
    #[test]
    fn the_account_balance_counts_collaterals_and_consolidated_coins() {
        let collateral = coin(1, 40_000, true, rounds::ROUNDS_COLLATERAL, false, B);
        let (b, _) = compute_balances(&[collateral], dashcore::Network::Testnet, 4);
        assert_eq!(
            (b.denominated, b.fully_mixed, b.coinjoin_account),
            (0, 0, 40_000)
        );
        let consolidated = coin(2, 3_000_000_000, true, 0, false, C);
        let (b, _) = compute_balances(
            std::slice::from_ref(&consolidated),
            dashcore::Network::Testnet,
            4,
        );
        assert_eq!(
            (b.denominated, b.fully_mixed, b.coinjoin_account),
            (0, 0, 3_000_000_000)
        );
        let mut waiting = consolidated.clone();
        waiting.coin.utxo.is_confirmed = false;
        let (b, _) = compute_balances(&[waiting], dashcore::Network::Testnet, 4);
        assert_eq!(b.coinjoin_account, 0, "a move takes confirmed coins only");
        let mut held = consolidated;
        held.coin.reserved = true;
        let (b, _) = compute_balances(&[held], dashcore::Network::Testnet, 4);
        assert_eq!(b.coinjoin_account, 0);
    }

    /// Q4: the plan names how much of the move is fully mixed; the rest of
    /// the account goes with it.
    #[test]
    fn a_sweep_splits_its_value_by_how_mixed_it_is() {
        let coins = vec![
            coin(1, 5_000, true, 4, true, A),
            coin(2, 7_000, true, 1, false, B),
            coin(3, 9_000, true, 4, true, C),
        ];
        let picked = SweepCoins {
            chunks: vec![dw_coinjoin::recovery::SweepChunk {
                coins: vec![0, 1, 2],
                amount: 20_000,
                fee: 1_000,
            }],
            coins,
            coins_after: 1,
            unconfirmed: 0,
        };
        let a = picked.amounts();
        assert_eq!((a.fully_mixed, a.not_fully_mixed), (14_000, 7_000));
        assert_eq!(a.fully_mixed + a.not_fully_mixed, picked.total());

        // Sol R3: a quote covers its own chunks at the same or a lower fee
        // only: a higher fee, or the same total from other coins, is
        // another plan.
        let quote = SweepQuote::of(&picked);
        assert!(quote.covers(&picked));
        let mut cheaper = picked.clone();
        cheaper.chunks[0].fee -= 1;
        assert!(quote.covers(&cheaper));
        let mut dearer = picked.clone();
        dearer.chunks[0].fee += 1;
        assert!(!quote.covers(&dearer));
        let mut other = picked.clone();
        other.chunks[0].coins = vec![0, 1];
        assert!(!quote.covers(&other));
        assert_ne!(SweepQuote::of(&picked).id, quote.id, "fresh ids");
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
