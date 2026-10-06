//! M2 fee policy and the coin-control summary. Owner: R1 (engine-tools,
//! `dw-engine::fee`). Contract: docs/contracts/m2-engine.md §2.3.

use crate::api::common::{OutPoint, ensure_open, not_implemented, parse_wallet_id};
use crate::{CoinsError, FeeMode, NetworkSession, SendError};

/// Where the recommended rates come from (DESIGN-opus §1.14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FeeSource {
    /// SPV: every target pays the minimum relay fee. The host shows "Dash
    /// blocks are rarely full; all targets use the minimum relay fee."
    MinimumRelay,
    /// The optional dashd RPC data source (`estimatesmartfee`); not in M2.
    NodeEstimate,
}

/// One confirmation target of dash-qt's fee drop-down (QT-057: 2, 4, 6, 12,
/// 24, 48, 144, 504, 1008 blocks).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FeeTarget {
    pub target_blocks: u32,
    pub duffs_per_kb: u64,
}

/// Fee rules the send page and the Options dialog show (QT-057/058).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct FeePolicy {
    pub source: FeeSource,
    /// Minimum relay fee, duff/kB (1000); the floor of a custom rate.
    pub min_relay_per_kb: u64,
    /// Highest custom rate the engine accepts, duff/kB.
    pub max_custom_per_kb: u64,
    /// Highest absolute fee (`send.absurd_fee` above it): 0.1 DASH, dash-qt
    /// `-maxtxfee`.
    pub max_tx_fee: u64,
    /// Highest fee rate `broadcast_psbt` accepts, duff/kB (0.1 DASH/kB, as
    /// dash-qt's PSBT dialog).
    pub max_broadcast_rate_per_kb: u64,
    /// One row per dash-qt target, ascending.
    pub targets: Vec<FeeTarget>,
}

/// dash-qt's coin-control panel values (QT-072, §5.6), computed with its
/// formula: bytes = 148·inputs + 34·(outputs + 1) + 10, minus 34 without
/// change. The host prefixes "≈".
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoinSelectionSummary {
    /// Selected coins that are still unspent and selectable.
    pub quantity: u32,
    /// Their value.
    pub amount: u64,
    pub bytes: u32,
    pub fee: u64,
    /// `amount − fee` ("After Fee").
    pub after_fee: u64,
    /// Change, or 0 when dust change went to the fee.
    pub change: u64,
    /// Change below the dust threshold was added to the fee (or every change
    /// on the CoinJoin page).
    pub change_to_fee: bool,
    /// The selection does not cover the amounts plus fee ("Insufficient
    /// funds!").
    pub insufficient_funds: bool,
    /// The fee may vary by this many duffs per input (tooltip "Can vary
    /// +/- %1 duff(s) per input.").
    pub fee_tolerance_per_input: u64,
    /// Selected coins that are spent or gone (QT-074): the host unselects
    /// them and says "Some coins were unselected because they were spent."
    pub unavailable: Vec<OutPoint>,
}

#[uniffi::export]
impl NetworkSession {
    /// Fee rules for this network (in-memory, constant on SPV).
    pub fn fee_policy(&self) -> Result<FeePolicy, SendError> {
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.fee_policy")
    }

    /// The coin-control summary for `outpoints` paying `pay_amounts` at
    /// `fee` (QT-072/074). `all_change_to_fee` is the CoinJoin page rule.
    /// Reads the wallet's coins; signs and reserves nothing.
    pub async fn coin_selection_summary(
        &self,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
        pay_amounts: Vec<u64>,
        fee: FeeMode,
        all_change_to_fee: bool,
    ) -> Result<CoinSelectionSummary, CoinsError> {
        let _ = (pay_amounts, fee, all_change_to_fee);
        parse_wallet_id(&wallet_id)?;
        for outpoint in &outpoints {
            outpoint.to_core()?;
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.coin_selection_summary")
    }
}
