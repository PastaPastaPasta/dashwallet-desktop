//! Coin control and fee records (QT-057/058, QT-068..075).

use dw_engine::{
    CoinInfo, CoinSelectionSummary as EngineSummary, FeePolicy as EngineFeePolicy,
    FeeSource as EngineFeeSource, FeeTarget as EngineFeeTarget,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;
use crate::error::AppError;

/// A transaction output reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OutPoint {
    /// 64 lower-case hex characters, display byte order.
    pub txid: String,
    pub vout: u32,
}

impl OutPoint {
    /// The engine outpoint; the txid must be 64 lower-case hex characters.
    pub fn to_core(&self) -> Result<dashcore::OutPoint, AppError> {
        let lower_hex = self.txid.len() == 64
            && self
                .txid
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let txid = lower_hex
            .then(|| self.txid.parse::<dashcore::Txid>().ok())
            .flatten()
            .ok_or_else(|| {
                AppError::invalid_argument(format!(
                    "txid {:?} is not 64 lowercase hex",
                    self.txid
                ))
            })?;
        Ok(dashcore::OutPoint::new(txid, self.vout))
    }

    /// A list of outpoints as the engine takes it.
    pub fn to_core_all(list: &[OutPoint]) -> Result<Vec<dashcore::OutPoint>, AppError> {
        list.iter().map(OutPoint::to_core).collect()
    }
}

impl From<dashcore::OutPoint> for OutPoint {
    fn from(o: dashcore::OutPoint) -> Self {
        OutPoint {
            txid: o.txid.to_string(),
            vout: o.vout,
        }
    }
}

/// One row of the coin control list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Utxo {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub block_height: Option<u32>,
    /// Block or first-seen time, UNIX seconds.
    pub timestamp: Option<u64>,
    pub instant_locked: bool,
    pub chain_locked: bool,
    /// Locked by the user (dash-qt "Lock unspent").
    pub user_locked: bool,
    /// Reserved by a prepared, not yet broadcast transaction.
    pub reserved: bool,
    pub label: Option<String>,
    pub is_change: bool,
    pub is_coinbase: bool,
    /// A CoinJoin denomination output.
    pub coinjoin_denominated: bool,
    /// Completed mixing rounds; `null` when not denominated or unknown.
    pub coinjoin_rounds: Option<u32>,
    /// Spendable now: mature, not locked or reserved, keys available.
    pub spendable: bool,
}

impl From<CoinInfo> for Utxo {
    fn from(c: CoinInfo) -> Self {
        let CoinInfo {
            outpoint,
            address,
            amount,
            confirmations,
            block_height,
            timestamp,
            instant_locked,
            chain_locked,
            user_locked,
            reserved,
            label,
            is_change,
            is_coinbase,
            coinjoin_denominated,
            coinjoin_rounds,
            spendable,
        } = c;
        Utxo {
            outpoint: outpoint.into(),
            address,
            amount,
            confirmations,
            block_height,
            timestamp,
            instant_locked,
            chain_locked,
            user_locked,
            reserved,
            label,
            is_change,
            is_coinbase,
            coinjoin_denominated,
            coinjoin_rounds,
            spendable,
        }
    }
}

/// Which coins `utxos` lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UtxoFilter {
    pub include_locked: bool,
    /// Only fully mixed coins (QT-071).
    pub fully_mixed_only: bool,
    pub min_confirmations: Option<u32>,
}

impl From<UtxoFilter> for dw_engine::CoinFilter {
    fn from(f: UtxoFilter) -> Self {
        let UtxoFilter {
            include_locked,
            fully_mixed_only,
            min_confirmations,
        } = f;
        dw_engine::CoinFilter {
            include_locked,
            fully_mixed_only,
            min_confirmations,
        }
    }
}

/// Where the recommended rates come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FeeSource {
    /// SPV: every target pays the minimum relay fee.
    MinimumRelay,
    /// A dashd `estimatesmartfee` data source (not available yet).
    NodeEstimate,
}

mirror_enum!(EngineFeeSource => FeeSource { MinimumRelay, NodeEstimate });

/// One confirmation target of dash-qt's fee drop-down (QT-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeeTarget {
    pub target_blocks: u32,
    pub duffs_per_kb: u64,
}

impl From<EngineFeeTarget> for FeeTarget {
    fn from(t: EngineFeeTarget) -> Self {
        let EngineFeeTarget {
            target_blocks,
            duffs_per_kb,
        } = t;
        FeeTarget {
            target_blocks,
            duffs_per_kb,
        }
    }
}

/// The fee rules the send page and the Options dialog show (QT-057/058).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeePolicy {
    pub source: FeeSource,
    /// Minimum relay fee, duffs per kB; the floor of a custom rate.
    pub min_relay_per_kb: u64,
    /// Highest custom rate, duffs per kB.
    pub max_custom_per_kb: u64,
    /// Highest absolute fee, duffs (0.1 DASH).
    pub max_tx_fee: u64,
    /// Highest fee rate `broadcast_psbt` accepts, duffs per kB.
    pub max_broadcast_rate_per_kb: u64,
    /// One row per dash-qt target, ascending.
    pub targets: Vec<FeeTarget>,
}

impl From<EngineFeePolicy> for FeePolicy {
    fn from(p: EngineFeePolicy) -> Self {
        let EngineFeePolicy {
            source,
            min_relay_per_kb,
            max_custom_per_kb,
            max_tx_fee,
            max_broadcast_rate_per_kb,
            targets,
        } = p;
        FeePolicy {
            source: source.into(),
            min_relay_per_kb,
            max_custom_per_kb,
            max_tx_fee,
            max_broadcast_rate_per_kb,
            targets: targets.into_iter().map(Into::into).collect(),
        }
    }
}

/// dash-qt's coin-control panel values (QT-072); the renderer prefixes "≈".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoinSelectionSummary {
    /// Selected coins that are still unspent and selectable.
    pub quantity: u32,
    pub amount: u64,
    pub bytes: u32,
    pub fee: u64,
    /// `amount - fee` ("After Fee").
    pub after_fee: u64,
    /// Change, or 0 when dust change went to the fee.
    pub change: u64,
    pub change_to_fee: bool,
    /// "Insufficient funds!".
    pub insufficient_funds: bool,
    pub fee_tolerance_per_input: u64,
    /// Selected coins that are spent or gone (QT-074).
    pub unavailable: Vec<OutPoint>,
}

impl From<EngineSummary> for CoinSelectionSummary {
    fn from(s: EngineSummary) -> Self {
        let EngineSummary {
            quantity,
            amount,
            bytes,
            fee,
            after_fee,
            change,
            change_to_fee,
            insufficient_funds,
            fee_tolerance_per_input,
            unavailable,
        } = s;
        CoinSelectionSummary {
            quantity,
            amount,
            bytes,
            fee,
            after_fee,
            change,
            change_to_fee,
            insufficient_funds,
            fee_tolerance_per_input,
            unavailable: unavailable.into_iter().map(Into::into).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outpoints_round_trip_and_reject_bad_txids() {
        let o = OutPoint {
            txid: "0a".repeat(32),
            vout: 7,
        };
        let core = o.to_core().unwrap();
        assert_eq!(OutPoint::from(core), o);
        for bad in ["", "zz", &"0A".repeat(32), &"0a".repeat(31)] {
            let o = OutPoint {
                txid: bad.to_string(),
                vout: 0,
            };
            assert_eq!(o.to_core().unwrap_err().code_str(), "invalid_argument");
        }
    }

    #[test]
    fn the_fee_policy_keeps_every_target() {
        let p = FeePolicy::from(dw_engine::fee_policy());
        assert_eq!(p.source, FeeSource::MinimumRelay);
        assert_eq!(p.targets.len(), dw_engine::FEE_TARGETS.len());
        assert_eq!(p.min_relay_per_kb, 1000);
    }
}
