//! Transaction history: paged queries, classification, details. Owner: E1.
//! Contract: docs/contracts/m1-engine.md §history.

use crate::NetworkSession;
use crate::api::common::{OutPoint, domain_error_common, parse_wallet_id};

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

domain_error_common!(@not_implemented HistoryError);

impl From<dw_engine::EngineError> for HistoryError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::InvalidQuery(detail) => Self::InvalidQuery { detail },
            E::StaleCursor => Self::StaleCursor,
            E::TxNotFound(txid) => Self::TxNotFound { txid },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

/// Maps an enum between the FFI and dw-engine, variant by variant (the two
/// declare the same variants in the same order).
macro_rules! map_enum {
    ($from:ty => $to:ty { $($v:ident),* $(,)? }) => {
        impl From<$from> for $to {
            fn from(x: $from) -> Self {
                match x { $(<$from>::$v => <$to>::$v),* }
            }
        }
    };
}

map_enum!(TxType => dw_engine::TxType {
    Other, Generated, SendToAddress, SendToOther, RecvWithAddress, RecvFromOther, SendToSelf,
    RecvWithCoinJoin, CoinJoinMixing, CoinJoinCollateralPayment, CoinJoinMakeCollaterals,
    CoinJoinCreateDenominations, CoinJoinSend, PlatformTransfer, DustReceive, DataTransaction,
    MasternodeRegistration, MasternodeUpdate, AssetLock,
});
map_enum!(dw_engine::TxType => TxType {
    Other, Generated, SendToAddress, SendToOther, RecvWithAddress, RecvFromOther, SendToSelf,
    RecvWithCoinJoin, CoinJoinMixing, CoinJoinCollateralPayment, CoinJoinMakeCollaterals,
    CoinJoinCreateDenominations, CoinJoinSend, PlatformTransfer, DustReceive, DataTransaction,
    MasternodeRegistration, MasternodeUpdate, AssetLock,
});
map_enum!(TxCategory => dw_engine::TxCategory {
    Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform, Other,
});
map_enum!(dw_engine::TxCategory => TxCategory {
    Sent, Received, Reward, Masternode, InternalTransfer, CoinJoin, Platform, Other,
});
map_enum!(TxStatusKind => dw_engine::TxStatusKind {
    Unconfirmed, Confirming, Confirmed, Conflicted, Abandoned, Immature, NotAccepted,
});
map_enum!(dw_engine::TxStatusKind => TxStatusKind {
    Unconfirmed, Confirming, Confirmed, Conflicted, Abandoned, Immature, NotAccepted,
});
map_enum!(WatchOnlyFilter => dw_engine::WatchOnlyFilter { All, Yes, No });
map_enum!(HistorySort => dw_engine::HistorySort {
    NewestFirst, OldestFirst, AmountDescending, AmountAscending,
});

impl From<dw_engine::TxStatus> for TxStatus {
    fn from(s: dw_engine::TxStatus) -> Self {
        Self {
            kind: s.kind.into(),
            confirmations: s.confirmations,
            instant_locked: s.instant_locked,
            chain_locked: s.chain_locked,
            matures_in: s.matures_in,
        }
    }
}

impl From<dw_engine::TxRecord> for TxRecord {
    fn from(r: dw_engine::TxRecord) -> Self {
        Self {
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
            involves_watch_only: r.involves_watch_only,
        }
    }
}

impl From<HistoryQuery> for dw_engine::HistoryQuery {
    fn from(q: HistoryQuery) -> Self {
        let f = q.filter;
        Self {
            filter: dw_engine::HistoryFilter {
                types: f.types.into_iter().map(Into::into).collect(),
                categories: f.categories.into_iter().map(Into::into).collect(),
                statuses: f.statuses.into_iter().map(Into::into).collect(),
                date_from: f.date_from,
                date_to: f.date_to,
                text: f.text,
                min_amount: f.min_amount,
                watch_only: f.watch_only.into(),
            },
            sort: q.sort.into(),
            cursor: q.cursor,
            limit: q.limit,
        }
    }
}

impl From<dw_engine::TxDetail> for TxDetail {
    fn from(d: dw_engine::TxDetail) -> Self {
        Self {
            txid: d.txid,
            records: d.records.into_iter().map(Into::into).collect(),
            status: d.status.into(),
            timestamp: d.timestamp,
            block_height: d.block_height,
            block_hash: d.block_hash,
            fee: d.fee,
            size_bytes: d.size_bytes,
            inputs: d
                .inputs
                .into_iter()
                .map(|i| TxInputDetail {
                    previous_output: OutPoint {
                        txid: i.previous_txid,
                        vout: i.previous_vout,
                    },
                    address: i.address,
                    amount: i.amount,
                    is_mine: i.is_mine,
                })
                .collect(),
            outputs: d
                .outputs
                .into_iter()
                .map(|o| TxOutputDetail {
                    vout: o.vout,
                    address: o.address,
                    amount: o.amount,
                    is_mine: o.is_mine,
                    is_change: o.is_change,
                    data_hex: o.data_hex,
                })
                .collect(),
            message: d.message,
            label: d.label,
            raw_hex: d.raw_hex,
        }
    }
}

crate::api::common::export_error_code!(HistoryError);

impl HistoryError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
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
    /// `EngineEvent::HistoryChanged` for the wallet. Cursors are keyset
    /// cursors: transactions arriving between pages do not invalidate them
    /// (a CSV export can page through a running sync); only a cursor from a
    /// different filter or sort is `StaleCursor`.
    pub async fn history_page(
        &self,
        wallet_id: String,
        query: HistoryQuery,
    ) -> Result<HistoryPage, HistoryError> {
        let id = parse_wallet_id(&wallet_id)?;
        let page = self.inner.history_page(id, query.into()).await?;
        Ok(HistoryPage {
            records: page.records.into_iter().map(Into::into).collect(),
            next_cursor: page.next_cursor,
            total_matching: page.total_matching,
        })
    }

    pub async fn tx_detail(
        &self,
        wallet_id: String,
        txid: String,
    ) -> Result<TxDetail, HistoryError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.tx_detail(id, txid).await?.into())
    }
}
