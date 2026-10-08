//! Records that cross the IPC boundary.
//!
//! Each one mirrors an engine record and derives `ts_rs::TS`, so the
//! renderer's types are generated (`apps/desktop/src/api/generated`), never
//! hand-written. Fields are camelCase. Duff amounts are JSON numbers: the
//! largest valid amount (21e14 duffs) is far below 2^53, so no precision is
//! lost in JavaScript.

use dw_engine::{
    DashNetwork, HistoryPage, HistorySort, SyncPhase, SyncSnapshot, TxCategory, TxDetail,
    TxInputDetail, TxOutputDetail, TxRecord, TxStatus, TxStatusKind, TxType, WalletBalances,
    WalletInfo,
};
use dw_vault::{LockState, VaultStatus};
use serde::{Deserialize, Serialize};
use tauri::Theme;
use ts_rs::TS;

use crate::launch::LaunchOptions;

/// `impl From<$from> for $to` for two enums with the same variant names. A
/// variant missing on either side is a compile error.
macro_rules! mirror_enum {
    ($from:ident => $to:ident { $($v:ident),+ $(,)? }) => {
        impl From<$from> for $to {
            fn from(x: $from) -> Self {
                match x {
                    $($from::$v => $to::$v),+
                }
            }
        }
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NetworkKind {
    Mainnet,
    Testnet,
    Devnet,
    Regtest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub kind: NetworkKind,
    /// `devnet-<name>` for devnets, else the kind.
    pub dir_name: String,
    /// dash-qt's unit name on this network: `DASH` on mainnet, `tDASH` elsewhere (QT-003).
    pub unit_name: String,
}

impl From<&DashNetwork> for NetworkInfo {
    fn from(n: &DashNetwork) -> Self {
        let kind = match n {
            DashNetwork::Mainnet => NetworkKind::Mainnet,
            DashNetwork::Testnet => NetworkKind::Testnet,
            DashNetwork::Devnet { .. } => NetworkKind::Devnet,
            DashNetwork::Regtest => NetworkKind::Regtest,
        };
        NetworkInfo {
            kind,
            dir_name: n.dir_name(),
            unit_name: if kind == NetworkKind::Mainnet {
                "DASH"
            } else {
                "tDASH"
            }
            .into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ThemeName {
    Light,
    Dark,
}

mirror_enum!(ThemeName => Theme { Light, Dark });

/// What the renderer needs to know before its first query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LaunchInfo {
    pub fixture: bool,
    pub selftest: bool,
    /// Milliseconds the self-test waits on each screen (0 = no pause).
    pub selftest_pause_ms: u32,
    pub theme: Option<ThemeName>,
    pub network: NetworkInfo,
    pub app_version: String,
    pub engine_version: String,
}

impl LaunchInfo {
    pub fn new(opts: &LaunchOptions) -> Self {
        LaunchInfo {
            fixture: opts.fixture,
            selftest: opts.selftest.is_some(),
            selftest_pause_ms: opts.selftest_pause_ms,
            theme: opts.theme,
            network: NetworkInfo::from(&opts.network),
            app_version: env!("CARGO_PKG_VERSION").into(),
            engine_version: dw_engine::ENGINE_VERSION.into(),
        }
    }
}

/// Balance buckets in duffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Balances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
    pub coinjoin: u64,
}

impl From<WalletBalances> for Balances {
    fn from(b: WalletBalances) -> Self {
        Balances {
            confirmed: b.confirmed,
            unconfirmed: b.unconfirmed,
            immature: b.immature,
            locked: b.locked,
            total: b.total,
            coinjoin: b.coinjoin,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WalletSummary {
    pub wallet_id: String,
    pub name: String,
    pub watch_only: bool,
    pub hd: bool,
    pub birth_height: Option<u32>,
    /// `null` until the scan has processed the birth block: a zero would be a guess.
    pub balances: Option<Balances>,
}

impl From<WalletInfo> for WalletSummary {
    fn from(w: WalletInfo) -> Self {
        WalletSummary {
            wallet_id: w.wallet_id.to_string(),
            name: w.name,
            watch_only: w.watch_only,
            hd: w.hd,
            birth_height: w.birth_height,
            balances: w.balances.map(Balances::from),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum LockStateName {
    NoVault,
    NoKeys,
    Unencrypted,
    Locked,
    UnlockedMixingOnly,
    Unlocked,
}

mirror_enum!(LockState => LockStateName { NoVault, NoKeys, Unencrypted, Locked, UnlockedMixingOnly, Unlocked });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LockStatus {
    pub state: LockStateName,
    pub encrypted: bool,
    pub failed_attempts: u32,
    pub retry_after_secs: Option<u64>,
}

impl From<VaultStatus> for LockStatus {
    fn from(s: VaultStatus) -> Self {
        LockStatus {
            state: s.state.into(),
            encrypted: s.encrypted,
            failed_attempts: s.failed_attempts,
            retry_after_secs: s.retry_after_secs,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SyncPhaseName {
    Headers,
    FilterHeaders,
    Filters,
    Masternodes,
}

mirror_enum!(SyncPhase => SyncPhaseName { Headers, FilterHeaders, Filters, Masternodes });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncPhaseStatus {
    pub phase: SyncPhaseName,
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub done: bool,
}

/// Raw sync state; the renderer derives the percentage (damping is the host's job).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    pub phases: Vec<SyncPhaseStatus>,
    pub active_phase: Option<SyncPhaseName>,
    pub tip_height: Option<u32>,
    pub tip_time: Option<u64>,
    pub chainlock_height: Option<u32>,
    pub connected_peers: u32,
    pub caught_up: bool,
    pub seconds_since_progress: Option<u64>,
}

impl From<SyncSnapshot> for SyncStatus {
    fn from(s: SyncSnapshot) -> Self {
        SyncStatus {
            running: s.running,
            phases: s
                .phases
                .into_iter()
                .map(|p| SyncPhaseStatus {
                    phase: p.phase.into(),
                    current_height: p.current_height,
                    target_height: p.target_height,
                    done: p.done,
                })
                .collect(),
            active_phase: s.active_phase.map(Into::into),
            tip_height: s.tip_height,
            tip_time: s.tip_time,
            chainlock_height: s.chainlock_height,
            connected_peers: s.connected_peers,
            caught_up: s.caught_up,
            seconds_since_progress: s.seconds_since_progress,
        }
    }
}

/// dash-qt `TransactionRecord::Type` (research 02 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TxTypeName {
    Other,
    Generated,
    SendToAddress,
    SendToOther,
    RecvWithAddress,
    RecvFromOther,
    SendToSelf,
    RecvWithCoinJoin,
    CoinJoinMixing,
    CoinJoinCollateralPayment,
    CoinJoinMakeCollaterals,
    CoinJoinCreateDenominations,
    CoinJoinSend,
    PlatformTransfer,
    DustReceive,
    DataTransaction,
    MasternodeRegistration,
    MasternodeUpdate,
    AssetLock,
}

mirror_enum!(TxType => TxTypeName { Other, Generated, SendToAddress, SendToOther, RecvWithAddress, RecvFromOther, SendToSelf, RecvWithCoinJoin, CoinJoinMixing, CoinJoinCollateralPayment, CoinJoinMakeCollaterals, CoinJoinCreateDenominations, CoinJoinSend, PlatformTransfer, DustReceive, DataTransaction, MasternodeRegistration, MasternodeUpdate, AssetLock });

/// iOS history filter categories (IOS-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TxCategoryName {
    Sent,
    Received,
    Reward,
    Masternode,
    InternalTransfer,
    CoinJoin,
    Platform,
    Other,
}

mirror_enum!(TxCategory => TxCategoryName { Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform, Other });

mirror_enum!(TxCategoryName => TxCategory { Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform, Other });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TxStatusKindName {
    Unconfirmed,
    Confirming,
    Confirmed,
    Conflicted,
    Abandoned,
    Immature,
    NotAccepted,
}

mirror_enum!(TxStatusKind => TxStatusKindName { Unconfirmed, Confirming, Confirmed, Conflicted, Abandoned, Immature, NotAccepted });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxStatusInfo {
    pub kind: TxStatusKindName,
    pub confirmations: u32,
    pub instant_locked: bool,
    pub chain_locked: bool,
    pub matures_in: Option<u32>,
}

impl From<TxStatus> for TxStatusInfo {
    fn from(s: TxStatus) -> Self {
        TxStatusInfo {
            kind: s.kind.into(),
            confirmations: s.confirmations,
            instant_locked: s.instant_locked,
            chain_locked: s.chain_locked,
            matures_in: s.matures_in,
        }
    }
}

/// One dash-qt history record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxRow {
    pub txid: String,
    pub record_index: u32,
    pub tx_type: TxTypeName,
    pub category: TxCategoryName,
    pub status: TxStatusInfo,
    /// UNIX seconds.
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    /// Signed duffs.
    pub amount: i64,
    pub fee: Option<u64>,
    pub address: Option<String>,
    pub label: Option<String>,
    pub counts_toward_balance: bool,
}

impl From<TxRecord> for TxRow {
    fn from(r: TxRecord) -> Self {
        TxRow {
            txid: r.txid,
            record_index: r.record_index,
            tx_type: r.tx_type.into(),
            category: r.category.into(),
            status: r.status.into(),
            timestamp: r.timestamp,
            block_height: r.block_height,
            amount: r.amount,
            fee: r.fee,
            address: r.address,
            label: r.label,
            counts_toward_balance: r.counts_toward_balance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum HistorySortName {
    NewestFirst,
    OldestFirst,
    AmountDescending,
    AmountAscending,
}

mirror_enum!(HistorySortName => HistorySort { NewestFirst, OldestFirst, AmountDescending, AmountAscending });

/// The part of the engine's history query the gate screens use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRequest {
    pub categories: Vec<TxCategoryName>,
    pub text: Option<String>,
    pub sort: HistorySortName,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRows {
    pub rows: Vec<TxRow>,
    pub next_cursor: Option<String>,
    pub total_matching: Option<u32>,
}

impl From<HistoryPage> for HistoryRows {
    fn from(p: HistoryPage) -> Self {
        HistoryRows {
            rows: p.records.into_iter().map(TxRow::from).collect(),
            next_cursor: p.next_cursor,
            total_matching: p.total_matching,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxInput {
    pub previous_txid: String,
    pub previous_vout: u32,
    pub address: Option<String>,
    pub amount: Option<u64>,
    pub is_mine: bool,
}

impl From<TxInputDetail> for TxInput {
    fn from(i: TxInputDetail) -> Self {
        TxInput {
            previous_txid: i.previous_txid,
            previous_vout: i.previous_vout,
            address: i.address,
            amount: i.amount,
            is_mine: i.is_mine,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxOutput {
    pub vout: u32,
    pub address: Option<String>,
    pub amount: u64,
    pub is_mine: bool,
    pub is_change: bool,
}

impl From<TxOutputDetail> for TxOutput {
    fn from(o: TxOutputDetail) -> Self {
        TxOutput {
            vout: o.vout,
            address: o.address,
            amount: o.amount,
            is_mine: o.is_mine,
            is_change: o.is_change,
        }
    }
}

/// The transaction detail sheet (QT-092, IOS-031).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxDetails {
    pub txid: String,
    pub rows: Vec<TxRow>,
    pub status: TxStatusInfo,
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    pub block_hash: Option<String>,
    pub fee: Option<u64>,
    pub size_bytes: u32,
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
    pub message: Option<String>,
    pub label: Option<String>,
}

impl From<TxDetail> for TxDetails {
    fn from(d: TxDetail) -> Self {
        TxDetails {
            txid: d.txid,
            rows: d.records.into_iter().map(TxRow::from).collect(),
            status: d.status.into(),
            timestamp: d.timestamp,
            block_height: d.block_height,
            block_hash: d.block_hash,
            fee: d.fee,
            size_bytes: d.size_bytes,
            inputs: d.inputs.into_iter().map(TxInput::from).collect(),
            outputs: d.outputs.into_iter().map(TxOutput::from).collect(),
            message: d.message,
            label: d.label,
        }
    }
}

/// Engine → renderer signal (event `engine://signal`). It says *what*
/// changed; the renderer re-queries (DESIGN-opus §1.5 rule 4). The engine
/// already debounces `sync`, `balances` and `history` to 4 Hz.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum EngineSignal {
    Session { network: String, open: bool },
    Wallets { network: String },
    Sync { network: String },
    Balances { network: String, wallet_id: String },
    History { network: String, wallet_id: String },
    LockState { network: String },
    Notice { code: String, detail: String },
}

/// Error returned by every command: a stable code plus diagnostic text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{code}: {message}")]
pub struct AppError {
    /// Engine codes (`wallet_not_found`, `vault.wrong_passphrase`, …) or `app.*`.
    pub code: String,
    pub message: String,
    /// Set by `vault.wrong_passphrase` and `vault.throttled`.
    pub retry_after_secs: Option<u64>,
}

impl AppError {
    pub fn app(code: &str, message: impl Into<String>) -> Self {
        AppError {
            code: format!("app.{code}"),
            message: message.into(),
            retry_after_secs: None,
        }
    }
}

impl From<dw_engine::EngineError> for AppError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_vault::VaultError;
        let (code, message, retry_after_secs) = match &e {
            dw_engine::EngineError::Vault(v) => (
                v.code(),
                v.to_string(),
                match v {
                    VaultError::WrongPassphrase {
                        retry_after_secs, ..
                    } => *retry_after_secs,
                    VaultError::Throttled { retry_after_secs } => Some(*retry_after_secs),
                    _ => None,
                },
            ),
            _ => (e.code(), e.to_string(), None),
        };
        AppError {
            code: code.into(),
            message,
            retry_after_secs,
        }
    }
}

/// Every root type exported to TypeScript (dependencies follow).
#[cfg(test)]
pub(crate) fn export_all(dir: &std::path::Path) -> Result<(), ts_rs::ExportError> {
    // Duff amounts and UNIX times are u64/i64; JSON numbers hold them exactly (see the top).
    let cfg = ts_rs::Config::new()
        .with_out_dir(dir)
        .with_large_int("number");
    LaunchInfo::export_all(&cfg)?;
    WalletSummary::export_all(&cfg)?;
    LockStatus::export_all(&cfg)?;
    SyncStatus::export_all(&cfg)?;
    HistoryRequest::export_all(&cfg)?;
    HistoryRows::export_all(&cfg)?;
    TxDetails::export_all(&cfg)?;
    EngineSignal::export_all(&cfg)?;
    AppError::export_all(&cfg)?;
    Ok(())
}
