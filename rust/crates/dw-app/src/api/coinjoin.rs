//! CoinJoin: mixing control and status per wallet, the network-wide options,
//! the salt, and the recovery scan and "Move mixed coins" (IOS-057).
//!
//! The CoinJoin send page is a draft with `CoinSource::fullyMixedOnly`; the
//! "unlock for mixing only" is `unlock` with the mixing scope. Status
//! changes arrive as `coinjoin` events; `coinjoin_status` is the in-memory
//! read to re-query on them.

use dw_coinjoin::{denoms, settings};
use dw_engine::coinjoin::{
    AmountAndRounds, CoinJoinBalances as EngineBalances, CoinJoinStatus as EngineStatus,
    MixingState, PoolMessage, PoolState, ProgressInfo, RecoveryReport, SessionInfo, StatusCode,
    StopReason, SweepChunkInfo, SweepDestination, SweepPlan, SweepResult, Unavailable,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// The CoinJoin options (Options, CoinJoin, QT-046). Global for the
/// network; mixing state is per wallet (QT-049).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinSettings {
    /// `enablecoinjoin`, default on.
    pub enabled: bool,
    /// `coinjoinmultisession`, default off.
    pub multi_session: bool,
    /// `coinjoinsessions`, 1..=10, default 4.
    pub max_sessions: u32,
    /// `coinjoinrounds`, 2..=16, default 4.
    pub rounds: u32,
    /// `coinjoinamount`, whole DASH, 2..=21,000,000, default 1000.
    pub target_amount_dash: u32,
    /// `coinjoindenomsgoal`, 10..=`denomsHardCap`, default 50.
    pub denoms_goal: u32,
    /// `coinjoindenomshardcap`, 10..=100,000, default 300.
    pub denoms_hard_cap: u32,
}

impl From<settings::CoinJoinSettings> for CoinJoinSettings {
    fn from(s: settings::CoinJoinSettings) -> Self {
        let settings::CoinJoinSettings {
            enabled,
            multi_session,
            max_sessions,
            rounds,
            target_amount_dash,
            denoms_goal,
            denoms_hard_cap,
        } = s;
        CoinJoinSettings {
            enabled,
            multi_session,
            max_sessions,
            rounds,
            target_amount_dash,
            denoms_goal,
            denoms_hard_cap,
        }
    }
}

impl From<CoinJoinSettings> for settings::CoinJoinSettings {
    fn from(s: CoinJoinSettings) -> Self {
        let CoinJoinSettings {
            enabled,
            multi_session,
            max_sessions,
            rounds,
            target_amount_dash,
            denoms_goal,
            denoms_hard_cap,
        } = s;
        settings::CoinJoinSettings {
            enabled,
            multi_session,
            max_sessions,
            rounds,
            target_amount_dash,
            denoms_goal,
            denoms_hard_cap,
        }
    }
}

/// Fixed CoinJoin values (Dash Core `coinjoin/{common,options}.h`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinLimits {
    /// Largest first, duffs.
    pub denominations: Vec<u64>,
    /// 0.00140001 DASH: smallest denomination plus the largest collateral.
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

impl CoinJoinLimits {
    fn new() -> Self {
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CoinJoinState {
    /// Not mixing ("Start CoinJoin").
    Idle,
    /// Mixing ("Stop CoinJoin").
    Mixing,
    /// Stop requested; open sessions are being reset.
    Stopping,
}

mirror_enum!(MixingState => CoinJoinState { Idle, Mixing, Stopping });

/// Why mixing stopped without the user asking (the panel shows it once).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CoinJoinStopReason {
    UserRequested,
    VaultLocked,
    WalletUnloaded,
    SessionClosed,
    /// CoinJoin was turned off in Options.
    Disabled,
}

mirror_enum!(StopReason => CoinJoinStopReason { UserRequested, VaultLocked, WalletUnloaded, SessionClosed, Disabled });

/// Why "Start CoinJoin" is unavailable. A locked vault is not here: the
/// renderer asks to unlock for mixing only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CoinJoinUnavailable {
    Disabled,
    WatchOnly,
    /// Balance below `minDuffs` ("CoinJoin requires at least %2 to use.").
    InsufficientFunds { min_duffs: u64 },
}

impl From<Unavailable> for CoinJoinUnavailable {
    fn from(u: Unavailable) -> Self {
        match u {
            Unavailable::Disabled => Self::Disabled,
            Unavailable::WatchOnly => Self::WatchOnly,
            Unavailable::InsufficientFunds { min_duffs } => Self::InsufficientFunds { min_duffs },
        }
    }
}

/// Core `PoolState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CoinJoinPoolState {
    Idle,
    Queue,
    AcceptingEntries,
    Signing,
    Error,
}

mirror_enum!(PoolState => CoinJoinPoolState { Idle, Queue, AcceptingEntries, Signing, Error });

/// Core `PoolMessage`: a masternode's reply in a session. The renderer
/// holds Core's English texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
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

mirror_enum!(PoolMessage => CoinJoinPoolMessage {
    AlreadyHave, Denom, EntriesFull, ExistingTx, Fees, InvalidCollateral, InvalidInput,
    InvalidScript, InvalidTx, Maximum, MnList, Mode, NonStandardPubkey, NotAMasternode,
    QueueFull, Recent, Session, MissingTx, Version, NoError, Success, EntriesAdded, SizeMismatch
});

/// The status line (Core `strAutoDenomResult`); QT-050 shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
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
    Masternode { message: CoinJoinPoolMessage },
}

impl From<StatusCode> for CoinJoinStatusCode {
    fn from(c: StatusCode) -> Self {
        match c {
            StatusCode::Idle => Self::Idle,
            StatusCode::SyncInProgress => Self::SyncInProgress,
            StatusCode::WalletLocked => Self::WalletLocked,
            StatusCode::MixingInProgress => Self::MixingInProgress,
            StatusCode::NoMasternodes => Self::NoMasternodes,
            StatusCode::NotEnoughFunds => Self::NotEnoughFunds,
            StatusCode::UnconfirmedDenominated => Self::UnconfirmedDenominated,
            StatusCode::NoCompatibleMasternode => Self::NoCompatibleMasternode,
            StatusCode::NoCompatibleInputs => Self::NoCompatibleInputs,
            StatusCode::TryingToConnect => Self::TryingToConnect,
            StatusCode::NoQueueToJoin => Self::NoQueueToJoin,
            StatusCode::NoRandomMasternode => Self::NoRandomMasternode,
            StatusCode::FailedToStartQueue => Self::FailedToStartQueue,
            StatusCode::WaitingInQueue => Self::WaitingInQueue,
            StatusCode::Signing => Self::Signing,
            StatusCode::Masternode(m) => Self::Masternode { message: m.into() },
        }
    }
}

/// The balances dash-qt's progress formula and panel use, duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinBalances {
    /// What mixing could still use (Core `GetAnonymizableBalance`).
    pub anonymizable: u64,
    /// Denominated coins, confirmed and unconfirmed.
    pub denominated: u64,
    pub normalized_anonymized: u64,
    /// Fully mixed by the QT-043 rule: the panel's "CoinJoin Balance" and
    /// what the CoinJoin send page may spend.
    pub fully_mixed: u64,
}

impl From<EngineBalances> for CoinJoinBalances {
    fn from(b: EngineBalances) -> Self {
        let EngineBalances {
            anonymizable,
            denominated,
            normalized_anonymized,
            fully_mixed,
        } = b;
        CoinJoinBalances {
            anonymizable,
            denominated,
            normalized_anonymized,
            fully_mixed,
        }
    }
}

/// dash-qt's progress (QT-042) and its tooltip parts, each 0..=100.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinProgress {
    pub overall_percent: f64,
    pub denominated_percent: f64,
    pub partially_mixed_percent: f64,
    pub mixed_percent: f64,
    /// "Denominated inputs have X of N rounds on average".
    pub average_rounds: f64,
}

impl From<ProgressInfo> for CoinJoinProgress {
    fn from(p: ProgressInfo) -> Self {
        let ProgressInfo {
            overall_percent,
            denominated_percent,
            partially_mixed_percent,
            mixed_percent,
            average_rounds,
        } = p;
        CoinJoinProgress {
            overall_percent,
            denominated_percent,
            partially_mixed_percent,
            mixed_percent,
            average_rounds,
        }
    }
}

/// "Amount and Rounds".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinAmountAndRounds {
    /// `min(anonymizable + fullyMixed, target)`.
    pub amount: u64,
    pub rounds: u32,
    /// "Not enough compatible inputs to mix...".
    pub insufficient_inputs: bool,
}

impl From<AmountAndRounds> for CoinJoinAmountAndRounds {
    fn from(a: AmountAndRounds) -> Self {
        let AmountAndRounds {
            amount,
            rounds,
            insufficient_inputs,
        } = a;
        CoinJoinAmountAndRounds {
            amount,
            rounds,
            insufficient_inputs,
        }
    }
}

/// One open mixing session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinSession {
    /// The masternode's proTxHash, display order.
    pub pro_tx_hash: Option<String>,
    pub service: Option<String>,
    pub denomination: Option<u64>,
    pub state: CoinJoinPoolState,
    pub entries: u32,
    pub last_message: Option<CoinJoinPoolMessage>,
}

impl From<SessionInfo> for CoinJoinSession {
    fn from(s: SessionInfo) -> Self {
        let SessionInfo {
            pro_tx_hash,
            service,
            denomination,
            state,
            entries,
            last_message,
        } = s;
        CoinJoinSession {
            pro_tx_hash,
            service,
            denomination,
            state: state.into(),
            entries,
            last_message: last_message.map(Into::into),
        }
    }
}

/// Everything the Overview CoinJoin panel, the advanced view and the
/// session status text show for one wallet (QT-041..050).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinStatus {
    pub wallet_id: String,
    pub state: CoinJoinState,
    /// Why the last run stopped; `null` while mixing or before any run.
    pub stop_reason: Option<CoinJoinStopReason>,
    /// Set when "Start CoinJoin" cannot be offered.
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
    /// Legacy keypool keys left; `null` for HD wallets (all of ours).
    pub keys_left: Option<u32>,
}

impl From<EngineStatus> for CoinJoinStatus {
    fn from(s: EngineStatus) -> Self {
        let EngineStatus {
            wallet_id,
            state,
            stop_reason,
            unavailable,
            balances,
            progress,
            amount_and_rounds,
            submitted_denominations,
            sessions,
            status,
            queue_size,
            keys_left,
        } = s;
        CoinJoinStatus {
            wallet_id: wallet_id.to_string(),
            state: state.into(),
            stop_reason: stop_reason.map(Into::into),
            unavailable: unavailable.map(Into::into),
            balances: balances.into(),
            progress: progress.into(),
            amount_and_rounds: amount_and_rounds.into(),
            submitted_denominations,
            sessions: sessions.into_iter().map(Into::into).collect(),
            status: status.into(),
            queue_size,
            keys_left,
        }
    }
}

/// The recovery scan's result (IOS-057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinJoinRecoveryReport {
    /// Addresses checked on the DIP9 CoinJoin account.
    pub coinjoin_addresses_scanned: u32,
    /// BIP44 addresses checked (gap 1000, Core-mixed funds).
    pub bip44_addresses_scanned: u32,
    /// The CoinJoin account's balance after the scan.
    pub coinjoin_balance: u64,
    /// Transactions the scan found that the wallet did not know.
    pub new_transactions: u32,
}

impl From<RecoveryReport> for CoinJoinRecoveryReport {
    fn from(r: RecoveryReport) -> Self {
        let RecoveryReport {
            coinjoin_addresses_scanned,
            bip44_addresses_scanned,
            coinjoin_balance,
            new_transactions,
        } = r;
        CoinJoinRecoveryReport {
            coinjoin_addresses_scanned,
            bip44_addresses_scanned,
            coinjoin_balance,
            new_transactions,
        }
    }
}

/// Where "Move mixed coins" sends the CoinJoin balance (IOS-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum MixedCoinsDestination {
    /// A fresh receive address of the same wallet's BIP44 account.
    Wallet,
    /// The wallet's shielded pool (`not_implemented` until it exists).
    Shielded,
}

mirror_enum!(MixedCoinsDestination => SweepDestination { Wallet, Shielded });
mirror_enum!(SweepDestination => MixedCoinsDestination { Wallet, Shielded });

/// One sweep transaction of the plan (at most 500 inputs each).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MixedCoinsChunk {
    pub inputs: u32,
    pub amount: u64,
    pub fee: u64,
}

impl From<SweepChunkInfo> for MixedCoinsChunk {
    fn from(c: SweepChunkInfo) -> Self {
        let SweepChunkInfo {
            inputs,
            amount,
            fee,
        } = c;
        MixedCoinsChunk {
            inputs,
            amount,
            fee,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MixedCoinsSweepPlan {
    pub destination: MixedCoinsDestination,
    /// The CoinJoin account balance the plan moves (coins above the
    /// 1000-duff threshold, mixed or not).
    pub total: u64,
    pub chunks: Vec<MixedCoinsChunk>,
}

impl From<SweepPlan> for MixedCoinsSweepPlan {
    fn from(p: SweepPlan) -> Self {
        let SweepPlan {
            destination,
            total,
            chunks,
        } = p;
        MixedCoinsSweepPlan {
            destination: destination.into(),
            total,
            chunks: chunks.into_iter().map(Into::into).collect(),
        }
    }
}

/// The result of "Move mixed coins". A later chunk can fail after earlier
/// ones were broadcast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MixedCoinsSweepResult {
    pub txids: Vec<String>,
    pub moved: u64,
    pub remaining: u64,
    /// The code of the error that stopped the sweep; `null` when complete.
    pub failure_code: Option<String>,
}

impl From<SweepResult> for MixedCoinsSweepResult {
    fn from(r: SweepResult) -> Self {
        let SweepResult {
            txids,
            moved,
            remaining,
            failure_code,
        } = r;
        MixedCoinsSweepResult {
            txids,
            moved,
            remaining,
            failure_code,
        }
    }
}

commands! {
    /// The fixed CoinJoin values and the defaults (constants of the
    /// CoinJoin library).
    #[facade()]
    fn coinjoin_limits(state) -> CoinJoinLimits {
        Ok(CoinJoinLimits::new())
    }

    /// The network's CoinJoin options (the defaults until changed).
    #[facade("NetworkSession::coinjoin_settings")]
    fn coinjoin_settings(state) -> CoinJoinSettings {
        Ok(state.session().await?.coinjoin_settings()?.into())
    }

    /// Applies the options live and stores them. Turning CoinJoin off stops
    /// every wallet's mixing (`stopReason: disabled`). Out-of-range values
    /// are `invalid_argument`, the message names the field.
    #[facade("NetworkSession::set_coinjoin_settings")]
    fn set_coinjoin_settings(state, settings: CoinJoinSettings) -> () {
        Ok(state.session().await?.set_coinjoin_settings(settings.into()).await?)
    }

    /// The wallet's mixing status (QT-041..050): the status the engine
    /// refreshes once a second; re-query on `coinjoin` events.
    #[facade("NetworkSession::coinjoin_status")]
    fn coinjoin_status(state, wallet_id: String) -> CoinJoinStatus {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        // Before the first refresh the engine computes it on the spot,
        // blocking the calling thread for the wallet's whole coin view.
        let status = tokio::task::spawn_blocking(move || session.coinjoin_status(id))
            .await
            .map_err(dw_engine::EngineError::from)??;
        Ok(status.into())
    }

    /// "Start CoinJoin" for one wallet. Needs the vault unencrypted, unlocked
    /// or unlocked for mixing only. Idempotent.
    #[facade("NetworkSession::start_mixing")]
    fn start_mixing(state, wallet_id: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.start_mixing(id).await?)
    }

    /// "Stop CoinJoin": resets the open sessions, releases their reserved
    /// coins, then stops. Idempotent.
    #[facade("NetworkSession::stop_mixing")]
    fn stop_mixing(state, wallet_id: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.stop_mixing(id).await?)
    }

    /// The wallet's CoinJoin salt (`coinjoinsalt get`), 64 lower-case hex
    /// characters; created on first use.
    #[facade("NetworkSession::coinjoin_salt")]
    fn coinjoin_salt(state, wallet_id: String) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.coinjoin_salt(id).await?)
    }

    /// `coinjoinsalt set`: replaces the salt, which changes which coins count
    /// as fully mixed. Refused while the wallet mixes.
    #[facade("NetworkSession::set_coinjoin_salt")]
    fn set_coinjoin_salt(state, wallet_id: String, salt_hex: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.set_coinjoin_salt(id, salt_hex).await?)
    }

    /// `coinjoinsalt generate`: a new random salt, returned. Refused while
    /// the wallet mixes.
    #[facade("NetworkSession::generate_coinjoin_salt")]
    fn generate_coinjoin_salt(state, wallet_id: String) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.generate_coinjoin_salt(id).await?)
    }

    /// The IOS-057 recovery scan: raises the CoinJoin account's lookahead and
    /// the BIP44 chains' to the restore gap, rescans from the wallet's birth
    /// height and reports what it found.
    #[facade("NetworkSession::coinjoin_recovery_scan")]
    fn coinjoin_recovery_scan(state, wallet_id: String) -> CoinJoinRecoveryReport {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.coinjoin_recovery_scan(id).await?.into())
    }

    /// The chunks "Move mixed coins" would broadcast. Reads coins only.
    #[facade("NetworkSession::mixed_coins_sweep_plan")]
    fn mixed_coins_sweep_plan(
        state,
        wallet_id: String,
        destination: MixedCoinsDestination,
    ) -> MixedCoinsSweepPlan {
        let id = parse_wallet_id(&wallet_id)?;
        let plan = state
            .session()
            .await?
            .mixed_coins_sweep_plan(id, destination.into())
            .await?;
        Ok(plan.into())
    }

    /// "Move mixed coins" (IOS-057): sweeps the CoinJoin account in chunks of
    /// at most 500 inputs. `grantId`: a `Spend` grant of at least the plan's
    /// total. Stops at the first failing chunk and reports what moved.
    #[facade("NetworkSession::move_mixed_coins")]
    fn move_mixed_coins(
        state,
        wallet_id: String,
        destination: MixedCoinsDestination,
        grant_id: String,
    ) -> MixedCoinsSweepResult {
        let id = parse_wallet_id(&wallet_id)?;
        let result = state
            .session()
            .await?
            .move_mixed_coins(id, destination.into(), grant_id)
            .await?;
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_the_engine_record() {
        let defaults = settings::CoinJoinSettings::default();
        let dto = CoinJoinSettings::from(defaults);
        assert_eq!(settings::CoinJoinSettings::from(dto), defaults);
        assert_eq!(CoinJoinLimits::new().defaults, dto);
    }

    #[test]
    fn limits_list_the_denominations_largest_first() {
        let l = CoinJoinLimits::new();
        assert!(l.denominations.windows(2).all(|w| w[0] > w[1]));
        assert_eq!(l.min_mixing_balance, denoms::MIN_MIXING_BALANCE);
    }

    #[test]
    fn data_carrying_enums_are_tagged() {
        let code = CoinJoinStatusCode::from(StatusCode::Masternode(PoolMessage::QueueFull));
        assert_eq!(
            serde_json::to_value(code).unwrap(),
            serde_json::json!({ "kind": "masternode", "message": "queueFull" })
        );
        let u = CoinJoinUnavailable::from(Unavailable::InsufficientFunds { min_duffs: 140_001 });
        assert_eq!(
            serde_json::to_value(u).unwrap(),
            serde_json::json!({ "kind": "insufficientFunds", "minDuffs": 140001 })
        );
    }

    #[test]
    fn the_destination_maps_both_ways() {
        for d in [MixedCoinsDestination::Wallet, MixedCoinsDestination::Shielded] {
            assert_eq!(MixedCoinsDestination::from(SweepDestination::from(d)), d);
        }
    }
}
