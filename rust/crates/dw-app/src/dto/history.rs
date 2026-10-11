//! History records (QT-092, IOS-028…031): the rows of `history_page` and
//! the transaction detail sheet.

use dw_engine::{
    HistoryFilter, HistoryPage, HistoryQuery, HistorySort, TxCategory, TxDetail, TxInputDetail,
    TxOutputDetail, TxRecord, TxStatus, TxStatusKind, TxType,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::mirror_enum;
use crate::error::AppError;

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

impl HistoryRequest {
    /// The engine query. Blank text is no text filter.
    pub fn into_query(self) -> Result<HistoryQuery, AppError> {
        let HistoryRequest {
            categories,
            text,
            sort,
            cursor,
            limit,
        } = self;
        Ok(HistoryQuery {
            filter: HistoryFilter {
                categories: categories.into_iter().map(Into::into).collect(),
                text: text.filter(|t| !t.trim().is_empty()),
                ..HistoryFilter::default()
            },
            sort: sort.into(),
            cursor,
            limit,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRows {
    pub rows: Vec<TxRow>,
    pub next_cursor: Option<String>,
    pub total_matching: Option<u32>,
}

/// `history_page_timed` (U5): the page plus the time the engine took to build
/// it, so the round trip splits into engine work and IPC + serialisation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TimedHistoryRows {
    pub engine_micros: u64,
    pub page: HistoryRows,
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
