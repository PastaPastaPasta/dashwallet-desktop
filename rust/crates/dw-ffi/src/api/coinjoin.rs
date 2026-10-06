//! M3 CoinJoin: mixing control and status per wallet, the global CoinJoin
//! options, the salt, and the IOS-057 recovery scan and "move mixed coins".
//! Owner: R1 (`dw-coinjoin`, `dw-p2p`, `dw-engine/src/coinjoin.rs`).
//! Contract: docs/contracts/m3-engine.md §2.1.
//!
//! The CoinJoin send page (QT-051) is `TxDraft` with
//! `CoinSource::FullyMixedOnly` (M1); M3 gives that source its rules
//! (fully mixed coins only, no change output, DS=1 mark), not a new call.
//! Unlock for mixing only (QT-112) is `Vault.unlock(…, UnlockScope::MixingOnly)`
//! (M1).

use crate::NetworkSession;
use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
use dw_coinjoin::{denoms, settings};

/// The CoinJoin options (Options → CoinJoin, QT-046). Global for the
/// network; mixing state is per wallet (QT-049). The UI-only options
/// (advanced interface, low-keys warning, popups) stay in the host's
/// settings, as dash-qt keeps them in QSettings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinSettings {
    /// `enablecoinjoin`, default on.
    pub enabled: bool,
    /// `coinjoinmultisession`, default off.
    pub multi_session: bool,
    /// `coinjoinsessions`, 1–10, default 4.
    pub max_sessions: u32,
    /// `coinjoinrounds`, 2–16, default 4.
    pub rounds: u32,
    /// `coinjoinamount`, whole DASH, 2–21,000,000, default 1000.
    pub target_amount_dash: u32,
    /// `coinjoindenomsgoal`, 10–`denoms_hard_cap`, default 50.
    pub denoms_goal: u32,
    /// `coinjoindenomshardcap`, 10–100,000, default 300.
    pub denoms_hard_cap: u32,
}

impl From<CoinJoinSettings> for settings::CoinJoinSettings {
    fn from(s: CoinJoinSettings) -> Self {
        Self {
            enabled: s.enabled,
            multi_session: s.multi_session,
            max_sessions: s.max_sessions,
            rounds: s.rounds,
            target_amount_dash: s.target_amount_dash,
            denoms_goal: s.denoms_goal,
            denoms_hard_cap: s.denoms_hard_cap,
        }
    }
}

impl From<settings::CoinJoinSettings> for CoinJoinSettings {
    fn from(s: settings::CoinJoinSettings) -> Self {
        Self {
            enabled: s.enabled,
            multi_session: s.multi_session,
            max_sessions: s.max_sessions,
            rounds: s.rounds,
            target_amount_dash: s.target_amount_dash,
            denoms_goal: s.denoms_goal,
            denoms_hard_cap: s.denoms_hard_cap,
        }
    }
}

/// Fixed CoinJoin values (Dash Core `coinjoin/{common,options}.h`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinLimits {
    /// Largest first, duffs.
    pub denominations: Vec<u64>,
    /// 0.00140001 DASH: smallest denomination + largest collateral.
    pub min_mixing_balance: u64,
    pub collateral_amount: u64,
    pub max_collateral_amount: u64,
    pub min_rounds: u32,
    pub max_rounds: u32,
    pub min_sessions: u32,
    pub max_sessions: u32,
    pub min_amount_dash: u32,
    pub max_amount_dash: u32,
    pub min_denoms: u32,
    pub max_denoms: u32,
    /// The defaults dash-qt starts with.
    pub defaults: CoinJoinSettings,
}

/// The fixed CoinJoin values. **Works** (constants of `dw-coinjoin`).
#[uniffi::export]
pub fn coinjoin_limits() -> CoinJoinLimits {
    CoinJoinLimits {
        denominations: denoms::DENOMINATIONS.to_vec(),
        min_mixing_balance: denoms::MIN_MIXING_BALANCE,
        collateral_amount: denoms::COLLATERAL_AMOUNT,
        max_collateral_amount: denoms::MAX_COLLATERAL_AMOUNT,
        min_rounds: settings::MIN_ROUNDS,
        max_rounds: settings::MAX_ROUNDS,
        min_sessions: settings::MIN_SESSIONS,
        max_sessions: settings::MAX_SESSIONS,
        min_amount_dash: settings::MIN_AMOUNT_DASH,
        max_amount_dash: settings::MAX_AMOUNT_DASH,
        min_denoms: settings::MIN_DENOMS,
        max_denoms: settings::MAX_DENOMS,
        defaults: settings::CoinJoinSettings::default().into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinState {
    /// Not mixing ("Start CoinJoin").
    Idle,
    /// Mixing ("Stop CoinJoin").
    Mixing,
    /// Stop requested; open sessions are being reset.
    Stopping,
}

/// Why mixing stopped without the user asking (the panel shows it once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinStopReason {
    UserRequested,
    /// The vault was locked while mixing ("Wallet is locked and user
    /// declined to unlock. Disabling CoinJoin." when the host's prompt was
    /// cancelled).
    VaultLocked,
    WalletUnloaded,
    SessionClosed,
    /// CoinJoin was turned off in Options.
    Disabled,
}

/// Why "Start CoinJoin" is unavailable (the button shows "(Disabled)").
/// A locked vault is not here: the host asks to unlock for mixing only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinUnavailable {
    /// Options: CoinJoin features off.
    Disabled,
    WatchOnly,
    /// Balance below `min_duffs` ("CoinJoin requires at least %2 to use.").
    InsufficientFunds {
        min_duffs: u64,
    },
}

/// Core `PoolState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinPoolState {
    Idle,
    Queue,
    AcceptingEntries,
    Signing,
    Error,
}

/// Core `PoolMessage`: a masternode's reply in a session (research 02
/// §9.4 "pool messages"). The host holds Core's English texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinPoolMessage {
    AlreadyHave,
    Denom,
    EntriesFull,
    ExistingTx,
    Fees,
    InvalidCollateral,
    InvalidInput,
    InvalidScript,
    InvalidTx,
    Maximum,
    MnList,
    Mode,
    NonStandardPubkey,
    NotAMasternode,
    QueueFull,
    Recent,
    Session,
    MissingTx,
    Version,
    NoError,
    Success,
    EntriesAdded,
    SizeMismatch,
}

/// The status line of `coinjoin status` (Core `strAutoDenomResult`,
/// research 02 §9.4); QT-050 shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoinJoinStatusCode {
    Idle,
    SyncInProgress,
    WalletLocked,
    MixingInProgress,
    NoMasternodes,
    NotEnoughFunds,
    UnconfirmedDenominated,
    NoCompatibleMasternode,
    NoCompatibleInputs,
    TryingToConnect,
    NoQueueToJoin,
    NoRandomMasternode,
    FailedToStartQueue,
    WaitingInQueue,
    Signing,
    /// "Masternode: <message>".
    Masternode {
        message: CoinJoinPoolMessage,
    },
}

/// The balances dash-qt's progress formula and panel use, duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinBalances {
    /// Core `GetAnonymizableBalance`: what mixing could still use.
    pub anonymizable: u64,
    /// Denominated coins, confirmed and unconfirmed.
    pub denominated: u64,
    /// Σ over denominated coins of `value · min(rounds, N) / N`.
    pub normalized_anonymized: u64,
    /// Fully mixed by the QT-043 rule (rounds ≥ N and (rounds ≥ N+3 or the
    /// salted hash is odd)): the panel's "CoinJoin Balance" and what the
    /// CoinJoin send page may spend.
    pub fully_mixed: u64,
}

/// dash-qt's progress (`overviewpage.cpp:462-512`, QT-042) and its tooltip
/// parts, each 0–100.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct CoinJoinProgress {
    pub overall_percent: f64,
    pub denominated_percent: f64,
    pub partially_mixed_percent: f64,
    pub mixed_percent: f64,
    /// "Denominated inputs have X of N rounds on average".
    pub average_rounds: f64,
}

/// "Amount and Rounds": `amount / rounds Rounds`, shown as `~amount` in red
/// with "Not enough compatible inputs to mix…" when `insufficient_inputs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinAmountAndRounds {
    /// `min(anonymizable + fully_mixed, target)`.
    pub amount: u64,
    pub rounds: u32,
    pub insufficient_inputs: bool,
}

/// One open mixing session (`getcoinjoininfo` `sessions[]`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinSession {
    /// The masternode's proTxHash, display order.
    pub pro_tx_hash: Option<String>,
    pub service: Option<String>,
    pub denomination: Option<u64>,
    pub state: CoinJoinPoolState,
    pub entries: u32,
    pub last_message: Option<CoinJoinPoolMessage>,
}

/// Everything the Overview CoinJoin panel, the advanced view and the
/// session status text show for one wallet (QT-041…050).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CoinJoinStatus {
    pub wallet_id: String,
    pub state: CoinJoinState,
    /// Why the last run stopped; `None` while mixing or before any run.
    pub stop_reason: Option<CoinJoinStopReason>,
    /// `Some` when "Start CoinJoin" cannot be offered.
    pub unavailable: Option<CoinJoinUnavailable>,
    pub balances: CoinJoinBalances,
    pub progress: CoinJoinProgress,
    pub amount_and_rounds: CoinJoinAmountAndRounds,
    /// "Submitted Denom" (advanced): denominations of the open sessions.
    pub submitted_denominations: Vec<u64>,
    pub sessions: Vec<CoinJoinSession>,
    pub status: CoinJoinStatusCode,
    /// `dsq` queues known.
    pub queue_size: u32,
    /// Legacy keypool keys left; `None` for HD wallets (all of ours).
    pub keys_left: Option<u32>,
}

/// IOS-057 recovery scan result.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoinJoinRecoveryReport {
    /// Addresses checked on the DIP9 CoinJoin account.
    pub coinjoin_addresses_scanned: u32,
    /// BIP44 addresses checked (gap 1000, Core-mixed funds).
    pub bip44_addresses_scanned: u32,
    /// CoinJoin-account balance after the scan.
    pub coinjoin_balance: u64,
    /// Transactions the scan found that the wallet did not know.
    pub new_transactions: u32,
}

/// Where "Move mixed coins" sends the CoinJoin balance (IOS-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MixedCoinsDestination {
    /// A fresh receive address of the same wallet's BIP44 account.
    Wallet,
    /// The wallet's shielded pool (M4: `NotImplemented` until then).
    Shielded,
}

/// One sweep transaction of the plan (≤ 500 inputs each, iOS chunking).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MixedCoinsChunk {
    pub inputs: u32,
    pub amount: u64,
    pub fee: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MixedCoinsSweepPlan {
    pub destination: MixedCoinsDestination,
    /// The CoinJoin-account balance the plan moves (coins above the
    /// 1000-duff threshold, mixed or not).
    pub total: u64,
    pub chunks: Vec<MixedCoinsChunk>,
}

/// Result of "Move mixed coins". A later chunk can fail after earlier ones
/// were broadcast (iOS "may partially succeed").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MixedCoinsSweepResult {
    pub txids: Vec<String>,
    pub moved: u64,
    pub remaining: u64,
    /// Code of the error that stopped the sweep, `None` when complete.
    pub failure_code: Option<String>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoinJoinError {
    /// Code `coinjoin.disabled`.
    #[error("coinjoin is disabled")]
    Disabled,
    /// Code `coinjoin.watch_only`.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `coinjoin.insufficient_funds`.
    #[error("balance below {min_duffs} duffs")]
    InsufficientFunds { min_duffs: u64 },
    /// Code `coinjoin.vault_locked`.
    #[error("vault locked")]
    VaultLocked,
    /// Code `coinjoin.grant_invalid`.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `coinjoin.nothing_to_move`.
    #[error("nothing to move")]
    NothingToMove,
    /// Code `coinjoin.spv_not_running`.
    #[error("spv is not running")]
    SpvNotRunning,
    /// Code `coinjoin.no_peers`.
    #[error("no connected peers")]
    NoPeers,
    /// Code `coinjoin.broadcast_rejected`.
    #[error("broadcast rejected: {reason}")]
    BroadcastRejected { reason: String },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

crate::api::common::domain_error_common!(@not_implemented CoinJoinError);
crate::api::common::export_error_code!(CoinJoinError);

impl CoinJoinError {
    /// Stable code (docs/contracts/m3-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::Disabled => "coinjoin.disabled",
            Self::WatchOnly => "coinjoin.watch_only",
            Self::InsufficientFunds { .. } => "coinjoin.insufficient_funds",
            Self::VaultLocked => "coinjoin.vault_locked",
            Self::GrantInvalid => "coinjoin.grant_invalid",
            Self::NothingToMove => "coinjoin.nothing_to_move",
            Self::SpvNotRunning => "coinjoin.spv_not_running",
            Self::NoPeers => "coinjoin.no_peers",
            Self::BroadcastRejected { .. } => "coinjoin.broadcast_rejected",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<dw_engine::EngineError> for CoinJoinError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::CoinJoinFailure as F;
        use dw_engine::EngineError as E;
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::CoinJoin(f) => match f {
                F::Disabled => Self::Disabled,
                F::WatchOnly => Self::WatchOnly,
                F::InsufficientFunds { min_duffs } => Self::InsufficientFunds { min_duffs },
                F::VaultLocked => Self::VaultLocked,
                F::GrantInvalid => Self::GrantInvalid,
                F::NothingToMove => Self::NothingToMove,
                F::SpvNotRunning => Self::SpvNotRunning,
                F::NoPeers => Self::NoPeers,
                F::BroadcastRejected(reason) => Self::BroadcastRejected { reason },
            },
            E::Vault(V::NoVault | V::Locked | V::MixingOnly) => Self::VaultLocked,
            E::Vault(V::GrantInvalid | V::GrantPurposeMismatch) => Self::GrantInvalid,
            E::SpvNotRunning => Self::SpvNotRunning,
            E::NoPeers => Self::NoPeers,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

/// Parses a CoinJoin salt: 64 lowercase hex characters (Core
/// `coinjoinsalt set`, a uint256 in display order).
fn parse_salt(salt_hex: &str) -> Result<(), CoinJoinError> {
    let ok = salt_hex.len() == 64
        && salt_hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if ok {
        Ok(())
    } else {
        Err(CoinJoinError::InvalidArgument {
            detail: "salt must be 64 lowercase hex characters".to_string(),
        })
    }
}

#[uniffi::export]
impl NetworkSession {
    /// The network's CoinJoin options (defaults until changed). In-memory
    /// read.
    pub fn coinjoin_settings(&self) -> Result<CoinJoinSettings, CoinJoinError> {
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.coinjoin_settings")
    }

    /// Applies the options live (dash-qt applies without restart) and
    /// stores them in app.sqlite. Turning CoinJoin off stops every wallet's
    /// mixing (`stop_reason = Disabled`). Ranges are checked first
    /// (`invalid_argument`, detail names the field).
    pub async fn set_coinjoin_settings(
        &self,
        settings: CoinJoinSettings,
    ) -> Result<(), CoinJoinError> {
        settings::CoinJoinSettings::from(settings)
            .validate()
            .map_err(|e| CoinJoinError::InvalidArgument {
                detail: e.to_string(),
            })?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.set_coinjoin_settings")
    }

    /// The wallet's mixing status (QT-041…050). In-memory read; re-query on
    /// `CoinJoin` events.
    pub fn coinjoin_status(&self, wallet_id: String) -> Result<CoinJoinStatus, CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.coinjoin_status")
    }

    /// "Start CoinJoin" for one wallet. Needs the vault unencrypted,
    /// unlocked or unlocked for mixing only (`coinjoin.vault_locked`
    /// otherwise: the host offers "Unlock wallet for mixing only"). Idempotent.
    pub async fn start_mixing(&self, wallet_id: String) -> Result<(), CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.start_mixing")
    }

    /// "Stop CoinJoin": resets the open sessions (`resetPool`), releases
    /// their reserved coins, then stops. Idempotent.
    pub async fn stop_mixing(&self, wallet_id: String) -> Result<(), CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.stop_mixing")
    }

    /// The wallet's CoinJoin salt (`coinjoinsalt get`), 64 hex characters.
    /// Created on first use; imported from `cj_salt` with a Dash Core wallet.
    pub async fn coinjoin_salt(&self, wallet_id: String) -> Result<String, CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.coinjoin_salt")
    }

    /// `coinjoinsalt set`: replaces the salt, which changes which coins
    /// count as fully mixed. Refused while mixing (`invalid_argument`).
    pub async fn set_coinjoin_salt(
        &self,
        wallet_id: String,
        salt_hex: String,
    ) -> Result<(), CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        parse_salt(&salt_hex)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.set_coinjoin_salt")
    }

    /// `coinjoinsalt generate`: a new random salt; returns it.
    pub async fn generate_coinjoin_salt(&self, wallet_id: String) -> Result<String, CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.generate_coinjoin_salt")
    }

    /// IOS-057 recovery scan: raises the CoinJoin account's lookahead and
    /// the BIP44 chains' to the restore gap (1000), rescans from the
    /// wallet's birth height and reports what it found. Runs as a rescan
    /// (`rescan_progress`, `cancel_rescan`).
    pub async fn coinjoin_recovery_scan(
        &self,
        wallet_id: String,
    ) -> Result<CoinJoinRecoveryReport, CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.coinjoin_recovery_scan")
    }

    /// The chunks "Move mixed coins" would broadcast. Reads coins only.
    pub async fn mixed_coins_sweep_plan(
        &self,
        wallet_id: String,
        destination: MixedCoinsDestination,
    ) -> Result<MixedCoinsSweepPlan, CoinJoinError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        match destination {
            MixedCoinsDestination::Wallet => {
                not_implemented("NetworkSession.mixed_coins_sweep_plan")
            }
            MixedCoinsDestination::Shielded => {
                not_implemented("NetworkSession.mixed_coins_sweep_plan.shielded")
            }
        }
    }

    /// "Move mixed coins" (IOS-057): sweeps the CoinJoin account in chunks
    /// of ≤ 500 inputs to `destination`. `grant_id`: a `Spend` grant of at
    /// least the plan's total. Stops at the first failing chunk and reports
    /// what moved.
    pub async fn move_mixed_coins(
        &self,
        wallet_id: String,
        destination: MixedCoinsDestination,
        grant_id: String,
    ) -> Result<MixedCoinsSweepResult, CoinJoinError> {
        let _ = grant_id;
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        match destination {
            MixedCoinsDestination::Wallet => not_implemented("NetworkSession.move_mixed_coins"),
            MixedCoinsDestination::Shielded => {
                not_implemented("NetworkSession.move_mixed_coins.shielded")
            }
        }
    }
}
