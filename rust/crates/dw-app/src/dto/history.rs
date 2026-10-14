//! History records (QT-092, IOS-028…031): the rows of `history_page` and
//! the transaction detail sheet.

use dw_engine::platform::Counterparty;
use dw_engine::{
    HistoryFilter, HistoryPage, HistoryQuery, HistorySort, TxCategory, TxDetail, TxInputDetail,
    TxOutputDetail, TxRecord, TxStatus, TxStatusKind, TxType, WatchOnlyFilter,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::mirror_enum;

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

mirror_enum!(TxTypeName => TxType { Other, Generated, SendToAddress, SendToOther, RecvWithAddress, RecvFromOther, SendToSelf, RecvWithCoinJoin, CoinJoinMixing, CoinJoinCollateralPayment, CoinJoinMakeCollaterals, CoinJoinCreateDenominations, CoinJoinSend, PlatformTransfer, DustReceive, DataTransaction, MasternodeRegistration, MasternodeUpdate, AssetLock });

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

mirror_enum!(TxStatusKindName => TxStatusKind { Unconfirmed, Confirming, Confirmed, Conflicted, Abandoned, Immature, NotAccepted });

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
        let TxStatus {
            kind,
            confirmations,
            instant_locked,
            chain_locked,
            matures_in,
        } = s;
        TxStatusInfo {
            kind: kind.into(),
            confirmations,
            instant_locked,
            chain_locked,
            matures_in,
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
    /// The record involves a watch-only address.
    pub involves_watch_only: bool,
    /// The DashPay contact paid or paying (DP3-02); absent when the record
    /// is not a contact payment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub counterparty: Option<TxCounterparty>,
}

/// The DashPay identity on the other side of a history record (DP3-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxCounterparty {
    /// Base58 identity id.
    pub identity: String,
    pub username: Option<String>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

impl From<Counterparty> for TxCounterparty {
    fn from(c: Counterparty) -> Self {
        let Counterparty {
            identity,
            username,
            display_name,
            avatar_url,
        } = c;
        TxCounterparty {
            identity,
            username,
            display_name,
            avatar_url,
        }
    }
}

impl From<TxRecord> for TxRow {
    fn from(r: TxRecord) -> Self {
        let TxRecord {
            txid,
            record_index,
            tx_type,
            category,
            status,
            timestamp,
            block_height,
            amount,
            fee,
            address,
            label,
            counts_toward_balance,
            involves_watch_only,
            counterparty,
        } = r;
        TxRow {
            txid,
            record_index,
            tx_type: tx_type.into(),
            category: category.into(),
            status: status.into(),
            timestamp,
            block_height,
            amount,
            fee,
            address,
            label,
            counts_toward_balance,
            involves_watch_only,
            counterparty: counterparty.map(Into::into),
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

mirror_enum!(HistorySort => HistorySortName { NewestFirst, OldestFirst, AmountDescending, AmountAscending });

/// Whether watch-only records show (`All`), only they do (`Yes`) or none
/// do (`No`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WatchOnlyFilterName {
    #[default]
    All,
    Yes,
    No,
}

mirror_enum!(WatchOnlyFilter => WatchOnlyFilterName { All, Yes, No });

mirror_enum!(WatchOnlyFilterName => WatchOnlyFilter { All, Yes, No });

/// The engine's `HistoryFilter`. Every field but `categories` and `text`
/// may be left out: no restriction.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryFilterRequest {
    /// Record types to show; empty: all.
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<TxTypeName>>")]
    pub types: Vec<TxTypeName>,
    /// Categories to show; empty: all.
    pub categories: Vec<TxCategoryName>,
    /// Statuses to show; empty: all.
    #[serde(default)]
    #[ts(optional, as = "Option<Vec<TxStatusKindName>>")]
    pub statuses: Vec<TxStatusKindName>,
    /// Earliest record time, UNIX seconds.
    #[serde(default)]
    #[ts(optional)]
    pub date_from: Option<u64>,
    /// Latest record time, UNIX seconds.
    #[serde(default)]
    #[ts(optional)]
    pub date_to: Option<u64>,
    /// Matches address, label and txid; blank is no filter.
    pub text: Option<String>,
    /// Smallest absolute amount, duffs.
    #[serde(default)]
    #[ts(optional)]
    pub min_amount: Option<u64>,
    #[serde(default)]
    #[ts(optional, as = "Option<WatchOnlyFilterName>")]
    pub watch_only: WatchOnlyFilterName,
}

impl HistoryFilterRequest {
    /// The engine filter. Blank text is no text filter.
    pub fn into_filter(self) -> HistoryFilter {
        let HistoryFilterRequest {
            types,
            categories,
            statuses,
            date_from,
            date_to,
            text,
            min_amount,
            watch_only,
        } = self;
        HistoryFilter {
            types: types.into_iter().map(Into::into).collect(),
            categories: categories.into_iter().map(Into::into).collect(),
            statuses: statuses.into_iter().map(Into::into).collect(),
            date_from,
            date_to,
            text: text.filter(|t| !t.trim().is_empty()),
            min_amount,
            watch_only: watch_only.into(),
        }
    }
}

impl From<HistoryFilter> for HistoryFilterRequest {
    fn from(f: HistoryFilter) -> Self {
        let HistoryFilter {
            types,
            categories,
            statuses,
            date_from,
            date_to,
            text,
            min_amount,
            watch_only,
        } = f;
        HistoryFilterRequest {
            types: types.into_iter().map(Into::into).collect(),
            categories: categories.into_iter().map(Into::into).collect(),
            statuses: statuses.into_iter().map(Into::into).collect(),
            date_from,
            date_to,
            text,
            min_amount,
            watch_only: watch_only.into(),
        }
    }
}

/// The engine's `HistoryQuery`: a filter (its fields sit beside `sort`),
/// the sort order and the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRequest {
    #[serde(flatten)]
    #[ts(flatten)]
    pub filter: HistoryFilterRequest,
    pub sort: HistorySortName,
    pub cursor: Option<String>,
    pub limit: u32,
}

impl HistoryRequest {
    /// The engine query. Blank text is no text filter.
    pub fn into_query(self) -> HistoryQuery {
        let HistoryRequest {
            filter,
            sort,
            cursor,
            limit,
        } = self;
        HistoryQuery {
            filter: filter.into_filter(),
            sort: sort.into(),
            cursor,
            limit,
        }
    }
}

impl From<HistoryQuery> for HistoryRequest {
    fn from(q: HistoryQuery) -> Self {
        let HistoryQuery {
            filter,
            sort,
            cursor,
            limit,
        } = q;
        HistoryRequest {
            filter: filter.into(),
            sort: sort.into(),
            cursor,
            limit,
        }
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
        let HistoryPage {
            records,
            next_cursor,
            total_matching,
        } = p;
        HistoryRows {
            rows: records.into_iter().map(TxRow::from).collect(),
            next_cursor,
            total_matching,
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
        let TxInputDetail {
            previous_txid,
            previous_vout,
            address,
            amount,
            is_mine,
        } = i;
        TxInput {
            previous_txid,
            previous_vout,
            address,
            amount,
            is_mine,
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
    /// An `OP_RETURN` output's payload, hex.
    pub data_hex: Option<String>,
}

impl From<TxOutputDetail> for TxOutput {
    fn from(o: TxOutputDetail) -> Self {
        let TxOutputDetail {
            vout,
            address,
            amount,
            is_mine,
            is_change,
            data_hex,
        } = o;
        TxOutput {
            vout,
            address,
            amount,
            is_mine,
            is_change,
            data_hex,
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
    /// The consensus-serialised transaction, hex.
    pub raw_hex: String,
}

impl From<TxDetail> for TxDetails {
    fn from(d: TxDetail) -> Self {
        let TxDetail {
            txid,
            records,
            status,
            timestamp,
            block_height,
            block_hash,
            fee,
            size_bytes,
            inputs,
            outputs,
            message,
            label,
            raw_hex,
        } = d;
        TxDetails {
            txid,
            rows: records.into_iter().map(TxRow::from).collect(),
            status: status.into(),
            timestamp,
            block_height,
            block_hash,
            fee,
            size_bytes,
            inputs: inputs.into_iter().map(TxInput::from).collect(),
            outputs: outputs.into_iter().map(TxOutput::from).collect(),
            message,
            label,
            raw_hex,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_with_only_the_original_fields_still_parses() {
        let r: HistoryRequest = serde_json::from_str(
            r#"{"categories":["sent"],"text":"  ","sort":"newestFirst","cursor":null,"limit":50}"#,
        )
        .unwrap();
        let q = r.into_query();
        assert_eq!(q.filter.categories, [TxCategory::Sent]);
        assert_eq!(q.filter.text, None, "blank text is no filter");
        assert!(q.filter.types.is_empty() && q.filter.statuses.is_empty());
        assert_eq!(q.filter.watch_only, WatchOnlyFilter::All);
        assert_eq!((q.sort, q.limit), (HistorySort::NewestFirst, 50));
    }

    #[test]
    fn every_filter_field_survives_the_round_trip() {
        let query = HistoryQuery {
            filter: HistoryFilter {
                types: vec![TxType::DustReceive, TxType::AssetLock],
                categories: vec![TxCategory::Platform],
                statuses: vec![TxStatusKind::Immature, TxStatusKind::Conflicted],
                date_from: Some(10),
                date_to: Some(20),
                text: Some("Coffee".into()),
                min_amount: Some(1_000),
                watch_only: WatchOnlyFilter::No,
            },
            sort: HistorySort::AmountAscending,
            cursor: Some("c".into()),
            limit: 25,
        };
        let request = HistoryRequest::from(query.clone());
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["types"][0], "dustReceive");
        assert_eq!(json["statuses"][1], "conflicted");
        assert_eq!(json["dateFrom"], 10);
        assert_eq!(json["minAmount"], 1_000);
        assert_eq!(json["watchOnly"], "no");
        assert_eq!(json["sort"], "amountAscending");
        let back: HistoryRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back.into_query(), query);
    }
}
