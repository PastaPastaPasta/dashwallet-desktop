//! Records of the send flow: the draft's inputs and the estimate, the
//! prepared transaction's summary, and PSBT analysis.

use dw_engine::{
    BroadcastOutcome as EngineBroadcastOutcome, ChangePolicy as EngineChangePolicy,
    CoinSource as EngineCoinSource, ContactRecipient as EngineContactRecipient,
    FeeMode as EngineFeeMode, PreparedInput as EnginePreparedInput,
    PreparedOutput as EnginePreparedOutput, PreparedSummary, PsbtAnalysis as EnginePsbtAnalysis,
    PsbtOutputInfo, PsbtSignability as EnginePsbtSignability, Recipient as EngineRecipient,
    TxEstimate as EngineTxEstimate,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::coins::OutPoint;
use crate::dto::mirror_enum;
use crate::error::AppError;

/// One payment line (QT-052/053).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Recipient {
    pub address: String,
    /// Duffs; at least the output's dust threshold (546 for P2PKH).
    pub amount: u64,
    /// dash-qt "Subtract fee from amount".
    pub subtract_fee_from_amount: bool,
    /// Saved to the address book after a broadcast (QT-063).
    pub label: Option<String>,
    /// `message` of a `dash:` URI; stored with the transaction.
    pub message: Option<String>,
}

impl From<Recipient> for EngineRecipient {
    fn from(r: Recipient) -> Self {
        let Recipient {
            address,
            amount,
            subtract_fee_from_amount,
            label,
            message,
        } = r;
        EngineRecipient {
            address,
            amount,
            subtract_fee_from_amount,
            label,
            message,
        }
    }
}

/// A DashPay contact as a draft's one recipient (DP3-01): its DIP-15
/// address is reserved at `draftPrepare`; until then a same-size
/// placeholder plans the fee. `identity` is ours and `contact` the one it
/// pays, both Base58 (`contact` established, or with its request waiting
/// for "Accept and pay"); `amount` is in duffs, at least 546; `note` is
/// stored with the transaction as its message.
// No field docs: ts-rs would put trailing spaces in the generated file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ContactRecipient {
    pub identity: String,
    pub contact: String,
    pub amount: u64,
    pub subtract_fee: bool,
    pub note: Option<String>,
}

impl From<ContactRecipient> for EngineContactRecipient {
    fn from(r: ContactRecipient) -> Self {
        let ContactRecipient {
            identity,
            contact,
            amount,
            subtract_fee,
            note,
        } = r;
        EngineContactRecipient {
            identity,
            contact,
            amount,
            subtract_fee,
            note,
        }
    }
}

/// Which coins a draft may spend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CoinSource {
    /// Any spendable coin of the standard accounts, except user-locked,
    /// reserved, immature and untrusted unconfirmed coins.
    Any,
    /// Only fully mixed CoinJoin coins (the CoinJoin send page, QT-051).
    FullyMixedOnly,
    /// Exactly these outpoints, all of them (coin control, QT-068..074).
    Outpoints { outpoints: Vec<OutPoint> },
}

impl CoinSource {
    pub fn into_engine(self) -> Result<EngineCoinSource, AppError> {
        Ok(match self {
            CoinSource::Any => EngineCoinSource::Any,
            CoinSource::FullyMixedOnly => EngineCoinSource::FullyMixedOnly,
            CoinSource::Outpoints { outpoints } => EngineCoinSource::Outpoints(
                outpoints
                    .iter()
                    .map(OutPoint::to_core)
                    .collect::<Result<_, _>>()?,
            ),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum FeeMode {
    /// The engine's rate for a confirmation target, 1..=1008 (on SPV the
    /// minimum relay fee for every target).
    Recommended { target_blocks: u32 },
    /// A custom rate in duffs per 1000 bytes, 1000..=10,000,000.
    PerKb { duffs_per_kb: u64 },
}

impl From<FeeMode> for EngineFeeMode {
    fn from(f: FeeMode) -> Self {
        match f {
            FeeMode::Recommended { target_blocks } => Self::Recommended { target_blocks },
            FeeMode::PerKb { duffs_per_kb } => Self::PerKb(duffs_per_kb),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChangePolicy {
    /// A fresh internal-chain address.
    Auto,
    /// A custom change address (QT-073); a foreign one counts against the
    /// spend cap.
    Address { address: String },
}

impl From<ChangePolicy> for EngineChangePolicy {
    fn from(c: ChangePolicy) -> Self {
        match c {
            ChangePolicy::Auto => Self::Auto,
            ChangePolicy::Address { address } => Self::Address(address),
        }
    }
}

/// Fee and size preview of a draft; nothing is signed or reserved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxEstimate {
    pub fee: u64,
    /// Estimated serialized size in bytes (QT-072).
    pub size_bytes: u32,
    pub input_count: u32,
    /// The change output's value; `null` when the change goes to the fee.
    pub change: Option<u64>,
    /// What the recipients receive, after any subtract-fee share.
    pub total_sent: u64,
}

impl From<EngineTxEstimate> for TxEstimate {
    fn from(e: EngineTxEstimate) -> Self {
        let EngineTxEstimate {
            fee,
            size_bytes,
            input_count,
            change,
            total_sent,
        } = e;
        TxEstimate {
            fee,
            size_bytes,
            input_count,
            change,
            total_sent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreparedInput {
    pub outpoint: OutPoint,
    pub address: Option<String>,
    pub amount: u64,
}

impl From<EnginePreparedInput> for PreparedInput {
    fn from(i: EnginePreparedInput) -> Self {
        let EnginePreparedInput {
            outpoint,
            address,
            amount,
        } = i;
        PreparedInput {
            outpoint: outpoint.into(),
            address,
            amount,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreparedOutput {
    pub address: Option<String>,
    pub amount: u64,
    pub is_change: bool,
    pub label: Option<String>,
    /// Pays one of this wallet's addresses. A change output with
    /// `isMine == false` is a foreign custom change address.
    pub is_mine: bool,
}

impl From<EnginePreparedOutput> for PreparedOutput {
    fn from(o: EnginePreparedOutput) -> Self {
        let EnginePreparedOutput {
            address,
            amount,
            is_change,
            is_mine,
            label,
        } = o;
        PreparedOutput {
            address,
            amount,
            is_change,
            label,
            is_mine,
        }
    }
}

/// What the confirm dialog shows (QT-059, IOS-046).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreparedTxSummary {
    pub txid: String,
    pub fee: u64,
    pub fee_rate_per_kb: u64,
    pub size_bytes: u32,
    pub inputs: Vec<PreparedInput>,
    pub outputs: Vec<PreparedOutput>,
    /// What the recipients receive.
    pub total_sent: u64,
    /// Inputs minus outputs back to the wallet: what leaves the wallet, fee
    /// included; the figure a `Spend` grant caps.
    pub total_debit: u64,
    /// Paid to scripts the wallet does not own (recipients and a foreign
    /// change address), fee excluded.
    pub external_sent: u64,
}

impl From<PreparedSummary> for PreparedTxSummary {
    fn from(s: PreparedSummary) -> Self {
        let PreparedSummary {
            txid,
            fee,
            fee_rate_per_kb,
            size_bytes,
            inputs,
            outputs,
            total_sent,
            external_sent,
            total_debit,
        } = s;
        PreparedTxSummary {
            txid,
            fee,
            fee_rate_per_kb,
            size_bytes,
            inputs: inputs.into_iter().map(Into::into).collect(),
            outputs: outputs.into_iter().map(Into::into).collect(),
            total_sent,
            total_debit,
            external_sent,
        }
    }
}

/// A prepared transaction the app holds for the renderer: the handle every
/// later call names it by, and its summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreparedTxHandle {
    pub handle: u64,
    pub summary: PreparedTxSummary,
}

/// The network accepted the transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BroadcastOutcome {
    pub txid: String,
}

impl From<EngineBroadcastOutcome> for BroadcastOutcome {
    fn from(o: EngineBroadcastOutcome) -> Self {
        let EngineBroadcastOutcome { txid } = o;
        BroadcastOutcome { txid }
    }
}

/// One output line of the PSBT Operations dialog (" * Sends %1 to %2").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PsbtOutput {
    pub address: Option<String>,
    pub amount: u64,
    /// " (own address)".
    pub is_mine: bool,
}

impl From<PsbtOutputInfo> for PsbtOutput {
    fn from(o: PsbtOutputInfo) -> Self {
        let PsbtOutputInfo {
            address,
            amount,
            is_mine,
        } = o;
        PsbtOutput {
            address,
            amount,
            is_mine,
        }
    }
}

/// dash-qt's status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PsbtStatus {
    MissingInputInfo,
    NeedsSignatures,
    Complete,
}

mirror_enum!(dw_psbt::Status => PsbtStatus { MissingInputInfo, NeedsSignatures, Complete });

/// The suffix of "needs signature(s)" for the selected wallet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum PsbtSignability {
    NoWallet,
    WatchOnly,
    NoMatchingKeys,
    CanSign,
}

mirror_enum!(EnginePsbtSignability => PsbtSignability { NoWallet, WatchOnly, NoMatchingKeys, CanSign });

/// The PSBT Operations dialog content (QT-079).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PsbtAnalysis {
    pub outputs: Vec<PsbtOutput>,
    /// `null` while an input's value is missing or unverified.
    pub fee: Option<u64>,
    /// What leaves the wallet; `null` like `fee`. A signing `Spend` grant
    /// must cover it.
    pub total: Option<u64>,
    pub unsigned_inputs: u32,
    pub status: PsbtStatus,
    pub signability: PsbtSignability,
    /// Value paid to scripts the wallet does not own, fee excluded.
    pub external_sent: Option<u64>,
    /// The unsigned transaction's txid: the same for every copy of the
    /// payment whatever its signatures or other PSBT fields (U-05 D40).
    pub unsigned_txid: String,
}

impl From<EnginePsbtAnalysis> for PsbtAnalysis {
    fn from(a: EnginePsbtAnalysis) -> Self {
        let EnginePsbtAnalysis {
            outputs,
            fee,
            total,
            unsigned_inputs,
            status,
            signability,
            external_sent,
            unsigned_txid,
        } = a;
        PsbtAnalysis {
            outputs: outputs.into_iter().map(Into::into).collect(),
            fee,
            total,
            unsigned_inputs,
            status: status.into(),
            signability: signability.into(),
            external_sent,
            unsigned_txid: unsigned_txid.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_enums_use_kind_and_camel_case_fields() {
        let src: CoinSource = serde_json::from_value(serde_json::json!({
            "kind": "outpoints",
            "outpoints": [{ "txid": "ab".repeat(32), "vout": 3 }],
        }))
        .unwrap();
        let EngineCoinSource::Outpoints(o) = src.into_engine().unwrap() else {
            panic!("outpoints");
        };
        assert_eq!(o[0].vout, 3);
        let any: CoinSource = serde_json::from_str(r#"{"kind":"fullyMixedOnly"}"#).unwrap();
        assert_eq!(any.into_engine().unwrap(), EngineCoinSource::FullyMixedOnly);

        let fee: FeeMode =
            serde_json::from_str(r#"{"kind":"perKb","duffsPerKb":2000}"#).unwrap();
        assert_eq!(EngineFeeMode::from(fee), EngineFeeMode::PerKb(2000));
        let fee: FeeMode =
            serde_json::from_str(r#"{"kind":"recommended","targetBlocks":6}"#).unwrap();
        assert_eq!(
            EngineFeeMode::from(fee),
            EngineFeeMode::Recommended { target_blocks: 6 }
        );
        let change: ChangePolicy =
            serde_json::from_str(r#"{"kind":"address","address":"Xabc"}"#).unwrap();
        assert_eq!(
            EngineChangePolicy::from(change),
            EngineChangePolicy::Address("Xabc".into())
        );
    }

    #[test]
    fn a_bad_outpoint_is_an_invalid_argument() {
        let src = CoinSource::Outpoints {
            outpoints: vec![OutPoint {
                txid: "AB".repeat(32),
                vout: 0,
            }],
        };
        assert_eq!(src.into_engine().unwrap_err().code_str(), "invalid_argument");
    }

    #[test]
    fn recipients_default_their_optional_fields() {
        let r: Recipient = serde_json::from_str(
            r#"{"address":"Xabc","amount":1000,"subtractFeeFromAmount":true,"label":null,"message":null}"#,
        )
        .unwrap();
        let e = EngineRecipient::from(r);
        assert!(e.subtract_fee_from_amount && e.label.is_none() && e.amount == 1000);
    }
}
