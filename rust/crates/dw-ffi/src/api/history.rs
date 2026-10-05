//! Transaction history: paged queries, classification, details. Owner: E1.
//! Contract: docs/contracts/m1-engine.md §history.

use crate::NetworkSession;
use crate::api::common::{OutPoint, domain_error_common, not_implemented, parse_wallet_id};

/// dash-qt `TransactionRecord::Type`, same order (QT-086; research 02 §4.1).
/// The order is the bit position of dash-qt's persisted type filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TxType {
    Other,
    Generated,
    SendToAddress,
    SendToOther,
    RecvWithAddress,
    RecvFromOther,
    SendToSelf,
    /// Kept for parity; dash-qt never assigns it.
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

/// iOS history categories (`TransactionFilterCategory`, IOS-028), derived
/// from `TxType` plus wallet context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TxCategory {
    Sent,
    Received,
    /// Coinbase / masternode reward.
    Reward,
    /// ProTx registration and updates.
    Masternode,
    /// All inputs and outputs are this wallet's (dash-qt "Payment to yourself").
    InternalTransfer,
    /// CoinJoin mixing, denomination and collateral transactions.
    CoinJoin,
    /// Asset lock / Platform transfer.
    Platform,
    Other,
}

/// dash-qt status model (QT-087; research 02 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum TxStatusKind {
    Unconfirmed,
    /// Fewer than 6 confirmations and no ChainLock.
    Confirming,
    Confirmed,
    Conflicted,
    Abandoned,
    /// Coinbase not yet mature.
    Immature,
    /// Coinbase that did not make it into the best chain.
    NotAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxStatus {
    pub kind: TxStatusKind,
    pub confirmations: u32,
    /// InstantSend lock seen (", verified via InstantSend").
    pub instant_locked: bool,
    /// Included in a ChainLocked block (", locked via ChainLocks"; confirms at once).
    pub chain_locked: bool,
    /// Immature coinbase: blocks until spendable.
    pub matures_in: Option<u32>,
}

/// One dash-qt transaction record. A transaction yields one or more records
/// (one per non-change output for sends); `record_index` orders them.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxRecord {
    pub txid: String,
    pub record_index: u32,
    pub tx_type: TxType,
    pub category: TxCategory,
    pub status: TxStatus,
    /// First-seen or block time, UNIX seconds.
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    /// Net effect on the wallet in duffs. For sends the fee is included in
    /// the first non-change record, as dash-qt does.
    pub amount: i64,
    /// Fee paid by this wallet; `None` when it did not fund the transaction
    /// or the input values are unknown.
    pub fee: Option<u64>,
    /// Counterparty or own address of this record; `None` for data outputs.
    pub address: Option<String>,
    /// Address-book or tx label.
    pub label: Option<String>,
    /// `false` renders the amount in brackets (abandoned, conflicted, immature).
    pub counts_toward_balance: bool,
    pub involves_watch_only: bool,
}

/// Watch-only filter (QT-089).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum WatchOnlyFilter {
    All,
    Yes,
    No,
}

/// Every field narrows the result; empty lists and `None` mean "any".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryFilter {
    pub types: Vec<TxType>,
    pub categories: Vec<TxCategory>,
    pub statuses: Vec<TxStatusKind>,
    /// Inclusive lower bound, UNIX seconds.
    pub date_from: Option<u64>,
    /// Exclusive upper bound, UNIX seconds (dash-qt range semantics).
    pub date_to: Option<u64>,
    /// Case-insensitive match against address, label or txid.
    pub text: Option<String>,
    /// Minimum absolute `amount`, duffs.
    pub min_amount: Option<u64>,
    pub watch_only: WatchOnlyFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum HistorySort {
    NewestFirst,
    OldestFirst,
    AmountDescending,
    AmountAscending,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryQuery {
    pub filter: HistoryFilter,
    pub sort: HistorySort,
    /// `next_cursor` of the previous page; `None` for the first page.
    pub cursor: Option<String>,
    /// Records per page, 1..=500.
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HistoryPage {
    pub records: Vec<TxRecord>,
    /// `None` on the last page.
    pub next_cursor: Option<String>,
    /// Matching records across all pages, when cheap to know.
    pub total_matching: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxInputDetail {
    pub previous_output: OutPoint,
    pub address: Option<String>,
    /// `None` when the spent output is not the wallet's and was never seen.
    pub amount: Option<u64>,
    pub is_mine: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxOutputDetail {
    pub vout: u32,
    pub address: Option<String>,
    pub amount: u64,
    pub is_mine: bool,
    pub is_change: bool,
    /// OP_RETURN payload, hex, for data outputs.
    pub data_hex: Option<String>,
}

/// Transaction details dialog (QT-092, IOS-031).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TxDetail {
    pub txid: String,
    pub records: Vec<TxRecord>,
    pub status: TxStatus,
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    pub block_hash: Option<String>,
    pub fee: Option<u64>,
    pub size_bytes: u32,
    pub inputs: Vec<TxInputDetail>,
    pub outputs: Vec<TxOutputDetail>,
    /// `message` stored with a payment sent from a `dash:` URI.
    pub message: Option<String>,
    pub label: Option<String>,
    /// Serialized transaction, hex.
    pub raw_hex: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum HistoryError {
    /// Code `history.invalid_query`: bad limit, date range or text.
    #[error("invalid query: {detail}")]
    InvalidQuery { detail: String },
    /// Code `history.stale_cursor`: the cursor is from another query or the
    /// history changed underneath it; restart from the first page.
    #[error("stale cursor")]
    StaleCursor,
    /// Code `history.tx_not_found`.
    #[error("transaction not found: {txid}")]
    TxNotFound { txid: String },
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

domain_error_common!(HistoryError);

impl HistoryError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidQuery { .. } => "history.invalid_query",
            Self::StaleCursor => "history.stale_cursor",
            Self::TxNotFound { .. } => "history.tx_not_found",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// One page of history records. Hosts call it again after
    /// `EngineEvent::HistoryChanged` for the wallet.
    pub async fn history_page(
        &self,
        wallet_id: String,
        query: HistoryQuery,
    ) -> Result<HistoryPage, HistoryError> {
        let _ = (parse_wallet_id(&wallet_id)?, query);
        not_implemented("NetworkSession.history_page")
    }

    pub async fn tx_detail(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<TxDetail, HistoryError> {
        let _ = (parse_wallet_id(&wallet_id)?, txid);
        not_implemented("NetworkSession.tx_detail")
    }
}
