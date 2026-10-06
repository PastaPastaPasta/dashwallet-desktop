//! dash-qt's transaction CSV export, byte for byte (QT-093, research 02
//! §4.7): `TransactionView::exportClicked` over `CSVModelWriter`.
//!
//! Columns: `Confirmed` (`true` when Confirming or Confirmed), `Watch-only`
//! (`1`/`0`, only for watch-only wallets), `Date` (`yyyy-MM-ddTHH:mm:ss` in
//! the caller's local time), `Type`, `Label`, `Address`, `Amount (<unit>)`
//! (signed, no separators), `ID`. Every field is quoted with `"` doubled;
//! fields are separated by `,` and every line ends with `\n`.

use dw_units::{Chain, SeparatorStyle, Unit};

use crate::history::{TxRecord, TxStatusKind, TxType};

/// Number of `TxType` values, the length `type_names` must have.
pub const TX_TYPE_COUNT: usize = 19;

/// Every `TxType` in enum order.
pub const TX_TYPES: [TxType; TX_TYPE_COUNT] = [
    TxType::Other,
    TxType::Generated,
    TxType::SendToAddress,
    TxType::SendToOther,
    TxType::RecvWithAddress,
    TxType::RecvFromOther,
    TxType::SendToSelf,
    TxType::RecvWithCoinJoin,
    TxType::CoinJoinMixing,
    TxType::CoinJoinCollateralPayment,
    TxType::CoinJoinMakeCollaterals,
    TxType::CoinJoinCreateDenominations,
    TxType::CoinJoinSend,
    TxType::PlatformTransfer,
    TxType::DustReceive,
    TxType::DataTransaction,
    TxType::MasternodeRegistration,
    TxType::MasternodeUpdate,
    TxType::AssetLock,
];

/// dash-qt's English `formatTxType` strings.
pub fn english_type_name(t: TxType) -> &'static str {
    match t {
        TxType::Other => "",
        TxType::Generated => "Mined",
        TxType::SendToAddress | TxType::SendToOther => "Sent to",
        TxType::RecvWithAddress => "Received with",
        TxType::RecvFromOther => "Received from",
        TxType::SendToSelf => "Payment to yourself",
        TxType::RecvWithCoinJoin => "Received via CoinJoin",
        TxType::CoinJoinMixing => "CoinJoin Mixing",
        TxType::CoinJoinCollateralPayment => "CoinJoin Collateral Payment",
        TxType::CoinJoinMakeCollaterals => "CoinJoin Make Collateral Inputs",
        TxType::CoinJoinCreateDenominations => "CoinJoin Create Denominations",
        TxType::CoinJoinSend => "CoinJoin Send",
        TxType::PlatformTransfer => "Platform Transfer",
        TxType::DustReceive => "Dust Receive",
        TxType::DataTransaction => "Data Transaction",
        TxType::MasternodeRegistration => "Masternode Registration",
        TxType::MasternodeUpdate => "Masternode Update",
        TxType::AssetLock => "Asset Lock",
    }
}

fn type_index(t: TxType) -> usize {
    TX_TYPES.iter().position(|x| *x == t).unwrap_or(0)
}

/// `QDateTime::toString(Qt::ISODate)` of UNIX seconds shifted by
/// `utc_offset_secs`: `yyyy-MM-ddTHH:mm:ss`.
pub fn iso_local(unix: u64, utc_offset_secs: i32) -> String {
    let t = i64::try_from(unix).unwrap_or(i64::MAX) + i64::from(utc_offset_secs);
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

fn push_field(out: &mut String, value: &str) {
    out.push('"');
    out.push_str(&value.replace('"', "\"\""));
    out.push('"');
}

fn push_row(out: &mut String, fields: &[&str]) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        push_field(out, f);
    }
    out.push('\n');
}

/// The CSV text of `records` (already filtered and sorted). `type_names`
/// are the 19 `TxType` display strings in enum order, or empty for dash-qt
/// English. `watch_only` adds the `Watch-only` column.
pub fn write_csv(
    records: &[TxRecord],
    watch_only: bool,
    unit: Unit,
    chain: Chain,
    type_names: &[String],
    utc_offset_secs: i32,
) -> String {
    let amount_title = format!("Amount ({})", unit.name(chain));
    let mut header = vec!["Confirmed"];
    if watch_only {
        header.push("Watch-only");
    }
    header.extend(["Date", "Type", "Label", "Address", amount_title.as_str(), "ID"]);
    let mut out = String::new();
    push_row(&mut out, &header);
    for r in records {
        let confirmed = matches!(
            r.status.kind,
            TxStatusKind::Confirming | TxStatusKind::Confirmed
        );
        let date = r
            .timestamp
            .map(|t| iso_local(t, utc_offset_secs))
            .unwrap_or_default();
        let type_name = match type_names.get(type_index(r.tx_type)) {
            Some(name) => name.as_str(),
            None => english_type_name(r.tx_type),
        };
        let amount = dw_units::format(unit, r.amount, false, SeparatorStyle::Never, false);
        let mut row = vec![if confirmed { "true" } else { "false" }];
        if watch_only {
            row.push(if r.involves_watch_only { "1" } else { "0" });
        }
        row.extend([
            date.as_str(),
            type_name,
            r.label.as_deref().unwrap_or(""),
            r.address.as_deref().unwrap_or(""),
            amount.as_str(),
            r.txid.as_str(),
        ]);
        push_row(&mut out, &row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::TxStatus;

    fn status(kind: &str) -> TxStatusKind {
        match kind {
            "Unconfirmed" => TxStatusKind::Unconfirmed,
            "Confirming" => TxStatusKind::Confirming,
            "Confirmed" => TxStatusKind::Confirmed,
            "Conflicted" => TxStatusKind::Conflicted,
            "Abandoned" => TxStatusKind::Abandoned,
            "Immature" => TxStatusKind::Immature,
            "NotAccepted" => TxStatusKind::NotAccepted,
            other => panic!("unknown status {other}"),
        }
    }

    fn unit(name: &str) -> Unit {
        match name {
            "DASH" => Unit::Dash,
            "mDASH" => Unit::MilliDash,
            "uDASH" => Unit::MicroDash,
            "duffs" => Unit::Duffs,
            other => panic!("unknown unit {other}"),
        }
    }

    fn record(v: &serde_json::Value) -> TxRecord {
        let s = |k: &str| v[k].as_str().map(str::to_string);
        let tx_type = TX_TYPES[v["type"].as_u64().unwrap() as usize];
        TxRecord {
            txid: s("txid").unwrap(),
            record_index: 0,
            tx_type,
            category: tx_type.category(),
            status: TxStatus {
                kind: status(v["status"].as_str().unwrap()),
                confirmations: 0,
                instant_locked: false,
                chain_locked: false,
                matures_in: None,
            },
            timestamp: v["timestamp"].as_u64(),
            block_height: None,
            amount: v["amount"].as_i64().unwrap(),
            fee: None,
            address: s("address"),
            label: s("label"),
            counts_toward_balance: true,
            involves_watch_only: v["watch_only"].as_bool().unwrap_or(false),
        }
    }

    /// `testdata/csv_export.json`: each case's rows through the writer give
    /// exactly `expected`.
    #[test]
    fn test_qt_093_csv_golden_vectors() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../../../../testdata/csv_export.json")).unwrap();
        let cases = data["cases"].as_array().unwrap();
        assert!(cases.len() >= 4);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let records: Vec<TxRecord> =
                case["records"].as_array().unwrap().iter().map(record).collect();
            let type_names: Vec<String> = case["type_names"]
                .as_array()
                .map(|a| a.iter().map(|n| n.as_str().unwrap().to_string()).collect())
                .unwrap_or_default();
            let chain = if case["mainnet"].as_bool().unwrap() {
                Chain::Main
            } else {
                Chain::Test
            };
            let csv = write_csv(
                &records,
                case["watch_only_wallet"].as_bool().unwrap(),
                unit(case["unit"].as_str().unwrap()),
                chain,
                &type_names,
                case["utc_offset_secs"].as_i64().unwrap() as i32,
            );
            assert_eq!(csv, case["expected"].as_str().unwrap(), "case {name}");
        }
    }

    #[test]
    fn iso_dates_match_qt() {
        assert_eq!(iso_local(0, 0), "1970-01-01T00:00:00");
        assert_eq!(iso_local(1_700_000_000, 0), "2023-11-14T22:13:20");
        assert_eq!(iso_local(1_700_000_000, 3600), "2023-11-14T23:13:20");
        assert_eq!(iso_local(1_709_164_800, 0), "2024-02-29T00:00:00");
        assert_eq!(iso_local(0, -3600), "1969-12-31T23:00:00");
    }

    #[test]
    fn english_names_cover_every_type() {
        assert_eq!(TX_TYPES.len(), TX_TYPE_COUNT);
        assert_eq!(english_type_name(TxType::CoinJoinMixing), "CoinJoin Mixing");
        assert_eq!(type_index(TxType::AssetLock), 18);
    }
}
