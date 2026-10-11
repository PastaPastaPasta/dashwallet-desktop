//! Transaction history, the detail sheet and the actions on a transaction
//! (abandon, resend, drop, CSV export, notification rows).

use std::str::FromStr;

use dashcore::{OutPoint, Txid};
use dw_engine::{TxDetailExtras, TxNotice};
use dw_units::Unit;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::history::{
    HistoryFilterRequest, HistoryRequest, HistoryRows, HistorySortName, TxDetails, TxTypeName,
};
use crate::dto::mirror_enum;
use crate::error::AppError;
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// dash-qt's display units (`nDisplayUnit` order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CsvUnitName {
    Dash,
    MilliDash,
    MicroDash,
    Duffs,
}

mirror_enum!(CsvUnitName => Unit { Dash, MilliDash, MicroDash, Duffs });

/// A transaction output, `txid:vout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxOutpoint {
    pub txid: String,
    pub vout: u32,
}

impl From<OutPoint> for TxOutpoint {
    fn from(o: OutPoint) -> Self {
        let OutPoint { txid, vout } = o;
        TxOutpoint {
            txid: txid.to_string(),
            vout,
        }
    }
}

/// The dash-qt details-dialog fields `TxDetails` lacks, and which actions
/// the sheet enables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxDetailExtrasInfo {
    pub txid: String,
    pub is_coinbase: bool,
    /// Sum of the outputs paying this wallet, duffs.
    pub total_credit: u64,
    /// Sum of this wallet's inputs; `null` when an input value is unknown.
    pub total_debit: Option<u64>,
    /// Signed duffs.
    pub net: i64,
    /// Immature coinbase: blocks until it matures.
    pub matures_in: Option<u32>,
    /// Whether a peer announced it; `null` unless the engine saw an
    /// announcement since start (SPV sees no mempool).
    pub in_mempool: Option<bool>,
    pub abandoned: bool,
    pub can_abandon: bool,
    pub can_resend: bool,
    /// Dust-locked outputs of this transaction.
    pub dust_locked_outputs: Vec<TxOutpoint>,
    /// UNIX seconds the engine last announced it (send or resend).
    pub last_announced_at: Option<u64>,
}

impl From<TxDetailExtras> for TxDetailExtrasInfo {
    fn from(x: TxDetailExtras) -> Self {
        let TxDetailExtras {
            txid,
            is_coinbase,
            total_credit,
            total_debit,
            net,
            matures_in,
            in_mempool,
            abandoned,
            can_abandon,
            can_resend,
            dust_locked_outputs,
            last_announced_at,
        } = x;
        TxDetailExtrasInfo {
            txid,
            is_coinbase,
            total_credit,
            total_debit,
            net,
            matures_in,
            in_mempool,
            abandoned,
            can_abandon,
            can_resend,
            dust_locked_outputs: dust_locked_outputs.into_iter().map(Into::into).collect(),
            last_announced_at,
        }
    }
}

/// One notification row (one per dash-qt record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxNoticeRow {
    pub txid: String,
    pub record_index: u32,
    /// Signed duffs; positive is "Incoming transaction".
    pub amount: i64,
    pub timestamp: Option<u64>,
    pub tx_type: TxTypeName,
    pub address: Option<String>,
    pub label: Option<String>,
    /// A CoinJoin mixing, denomination or collateral transaction; hidden
    /// unless "show CoinJoin popups" is on.
    pub coinjoin_internal: bool,
}

impl From<TxNotice> for TxNoticeRow {
    fn from(n: TxNotice) -> Self {
        let TxNotice {
            txid,
            record_index,
            amount,
            timestamp,
            tx_type,
            address,
            label,
            coinjoin_internal,
        } = n;
        TxNoticeRow {
            txid,
            record_index,
            amount,
            timestamp,
            tx_type: tx_type.into(),
            address,
            label,
            coinjoin_internal,
        }
    }
}

/// A txid as the renderer sends it: display-order hex.
fn parse_txid(s: &str) -> Result<Txid, AppError> {
    Txid::from_str(s).map_err(|e| AppError::invalid_argument(format!("txid {s:?}: {e}")))
}

commands! {
    #[facade("NetworkSession::history_page")]
    fn history_page(state, wallet_id: String, request: HistoryRequest) -> HistoryRows {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session.history_page(id, request.into_query()?).await?.into())
    }

    #[facade("NetworkSession::tx_detail")]
    fn tx_detail(state, wallet_id: String, txid: String) -> TxDetails {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.tx_detail(id, txid).await?.into())
    }

    /// The dash-qt detail fields `txDetail` lacks and the enablement of
    /// "Abandon" and "Resend" for one transaction.
    #[facade("NetworkSession::tx_detail_extras")]
    fn tx_detail_extras(state, wallet_id: String, txid: String) -> TxDetailExtrasInfo {
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        Ok(state.session().await?.tx_detail_extras(id, txid).await?.into())
    }

    /// dash-qt "Abandon transaction": marks it abandoned, releases its
    /// inputs and excludes it from balances. SPV cannot prove it is in no
    /// mempool, so it may still confirm; the history then shows it
    /// confirmed again.
    #[facade("NetworkSession::abandon_transaction")]
    fn abandon_transaction(state, wallet_id: String, txid: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        Ok(state.session().await?.abandon_transaction(id, txid).await?)
    }

    /// dash-qt "Resend transaction": hands the stored transaction to the SPV
    /// client again. Returns once handed over; there is no acceptance
    /// verdict.
    #[facade("NetworkSession::resend_transaction")]
    fn resend_transaction(state, wallet_id: String, txid: String) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        let txid = parse_txid(&txid)?;
        Ok(state.session().await?.resend_transaction(id, txid).await?)
    }

    /// "Remove unconfirmed": abandons every unconfirmed, not
    /// InstantSend-locked transaction of the wallet (every wallet when
    /// `walletId` is `null`) and rescans from the lowest funding height.
    /// Needs a running SPV client. Returns how many were dropped.
    #[facade("NetworkSession::drop_unconfirmed")]
    fn drop_unconfirmed(state, wallet_id: Option<String>) -> u32 {
        let id = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        Ok(state.session().await?.drop_unconfirmed(id).await?)
    }

    /// dash-qt CSV export of the filtered view, as text: the caller writes
    /// the file. `typeNames` are the 19 type display strings in type order
    /// (empty: dash-qt's English); `utcOffsetSecs` is the local time offset
    /// of the `Date` column.
    #[facade("NetworkSession::export_history_csv")]
    fn export_history_csv(
        state,
        wallet_id: String,
        filter: HistoryFilterRequest,
        sort: HistorySortName,
        unit: CsvUnitName,
        type_names: Vec<String>,
        utc_offset_secs: i32,
    ) -> String {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session
            .export_history_csv(
                id,
                filter.into_filter(),
                sort.into(),
                unit.into(),
                type_names,
                utc_offset_secs,
            )
            .await?)
    }

    /// Notification rows for the transactions a `newTransactions` event
    /// names. Unknown txids are skipped.
    #[facade("NetworkSession::tx_notices")]
    fn tx_notices(state, wallet_id: String, txids: Vec<String>) -> Vec<TxNoticeRow> {
        let id = parse_wallet_id(&wallet_id)?;
        let txids = txids
            .iter()
            .map(|t| parse_txid(t))
            .collect::<Result<Vec<_>, _>>()?;
        let notices = state.session().await?.tx_notices(id, txids).await?;
        Ok(notices.into_iter().map(Into::into).collect())
    }

    /// Whether a record type is a CoinJoin mixing, denomination or
    /// collateral transaction, whose popup dash-qt hides unless "show
    /// CoinJoin popups" is on.
    #[facade("dw_engine::is_coinjoin_internal")]
    fn is_coinjoin_internal(state, tx_type: TxTypeName) -> bool {
        Ok(dw_engine::is_coinjoin_internal(tx_type.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txids_parse_as_display_hex() {
        let hex = "00".repeat(31) + "01";
        assert_eq!(parse_txid(&hex).unwrap().to_string(), hex);
        assert_eq!(
            parse_txid("not hex").unwrap_err().code_str(),
            "invalid_argument"
        );
        assert!(parse_txid("abcd").is_err());
    }

    #[test]
    fn csv_units_follow_the_display_unit_order() {
        for (name, unit) in [
            (CsvUnitName::Dash, Unit::Dash),
            (CsvUnitName::MilliDash, Unit::MilliDash),
            (CsvUnitName::MicroDash, Unit::MicroDash),
            (CsvUnitName::Duffs, Unit::Duffs),
        ] {
            assert_eq!(Unit::from(name), unit);
        }
        assert_eq!(
            serde_json::from_str::<CsvUnitName>("\"milliDash\"").unwrap(),
            CsvUnitName::MilliDash
        );
    }

    #[test]
    fn a_notice_row_names_its_type_in_camel_case() {
        let row = TxNoticeRow::from(TxNotice {
            txid: "aa".into(),
            record_index: 1,
            amount: -5,
            timestamp: None,
            tx_type: dw_engine::TxType::CoinJoinMixing,
            address: None,
            label: None,
            coinjoin_internal: true,
        });
        let json = serde_json::to_value(&row).unwrap();
        assert_eq!(json["txType"], "coinJoinMixing");
        assert_eq!(json["coinjoinInternal"], true);
        assert_eq!(json["recordIndex"], 1);
    }

    #[test]
    fn an_outpoint_crosses_as_txid_and_vout() {
        let o = OutPoint {
            txid: parse_txid(&("11".repeat(32))).unwrap(),
            vout: 3,
        };
        let json = serde_json::to_value(TxOutpoint::from(o)).unwrap();
        assert_eq!(json["txid"], "11".repeat(32));
        assert_eq!(json["vout"], 3);
    }
}
