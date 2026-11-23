//! Fee policy and dash-qt's coin-control summary (QT-057, QT-058, QT-072,
//! QT-074; docs/contracts/m2-engine.md §2.3).
//!
//! SPV has no fee estimator: every confirmation target pays the minimum relay
//! fee (DESIGN-opus §1.14). The policy says so (`FeeSource::MinimumRelay`)
//! instead of inventing per-target rates.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use dashcore::OutPoint;

use crate::send::plan::{dust_threshold, fee_for};
use crate::send::{FeeMode, MAX_FEE_PER_KB, MAX_TX_FEE, MIN_FEE_PER_KB};
use crate::{EngineError, NetworkSession, WalletId};

/// dash-qt's confirmation targets, in blocks (`confTargets` in
/// sendcoinsdialog.cpp).
pub const FEE_TARGETS: [u32; 9] = [2, 4, 6, 12, 24, 48, 144, 504, 1008];
/// Highest fee rate `broadcast_psbt` accepts (Dash Core `DEFAULT_MAX_RAW_TX_FEE_RATE`,
/// 0.1 DASH/kB).
pub const MAX_BROADCAST_RATE_PER_KB: u64 = 10_000_000;

/// Where the recommended rates come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeeSource {
    /// Every target pays the minimum relay fee (SPV).
    MinimumRelay,
    /// A dashd `estimatesmartfee` data source (not available in M2).
    NodeEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeeTarget {
    pub target_blocks: u32,
    pub duffs_per_kb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeePolicy {
    pub source: FeeSource,
    pub min_relay_per_kb: u64,
    pub max_custom_per_kb: u64,
    pub max_tx_fee: u64,
    pub max_broadcast_rate_per_kb: u64,
    pub targets: Vec<FeeTarget>,
}

/// The SPV fee policy: every dash-qt target at the minimum relay fee.
pub fn fee_policy() -> FeePolicy {
    FeePolicy {
        source: FeeSource::MinimumRelay,
        min_relay_per_kb: MIN_FEE_PER_KB,
        max_custom_per_kb: MAX_FEE_PER_KB,
        max_tx_fee: MAX_TX_FEE,
        max_broadcast_rate_per_kb: MAX_BROADCAST_RATE_PER_KB,
        targets: FEE_TARGETS
            .iter()
            .map(|&target_blocks| FeeTarget {
                target_blocks,
                duffs_per_kb: MIN_FEE_PER_KB,
            })
            .collect(),
    }
}

/// dash-qt's coin-control panel values.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoinSelectionSummary {
    pub quantity: u32,
    pub amount: u64,
    pub bytes: u32,
    pub fee: u64,
    pub after_fee: u64,
    pub change: u64,
    pub change_to_fee: bool,
    pub insufficient_funds: bool,
    pub fee_tolerance_per_input: u64,
    pub unavailable: Vec<OutPoint>,
}

/// Size of a P2PKH input in dash-qt's estimate.
const INPUT_BYTES: u64 = 148;
/// Size of a P2PKH output in dash-qt's estimate.
const OUTPUT_BYTES: u64 = 34;
/// Version, locktime and counts in dash-qt's estimate.
const OVERHEAD_BYTES: u64 = 10;
/// Script length dash-qt's dust check assumes for change (a 24-byte push).
const CHANGE_SCRIPT_LEN: usize = 25;

/// `CoinControlDialog::updateLabels` over the values of the selected coins
/// that are still unspent. `fee_rate_per_kb` is the rate the payment pays.
/// dash-qt's subtract-fee-from-amount adjustment is not applied: the summary
/// describes the plain payment.
pub(crate) fn summarize(
    values: &[u64],
    pay_amounts: &[u64],
    fee_rate_per_kb: u64,
    all_change_to_fee: bool,
) -> CoinSelectionSummary {
    let quantity = values.len() as u64;
    let amount: u64 = values.iter().copied().fold(0u64, u64::saturating_add);
    let pay: u64 = pay_amounts.iter().copied().fold(0u64, u64::saturating_add);
    let mut s = CoinSelectionSummary {
        quantity: u32::try_from(quantity).unwrap_or(u32::MAX),
        amount,
        ..Default::default()
    };
    if quantity == 0 {
        s.insufficient_funds = pay > 0;
        return s;
    }
    // Always assume one change output; with no recipients, two outputs.
    let outputs = if pay_amounts.is_empty() {
        2
    } else {
        pay_amounts.len() as u64 + 1
    };
    let mut bytes = INPUT_BYTES * quantity + OUTPUT_BYTES * outputs + OVERHEAD_BYTES;
    let rate = key_wallet::wallet::managed_wallet_info::fee::FeeRate::new(fee_rate_per_kb);
    let mut fee = fee_for(rate, bytes as usize).unwrap_or(u64::MAX);
    let mut change: i128 = 0;
    if pay > 0 {
        change = i128::from(amount) - i128::from(pay);
        s.insufficient_funds = change < i128::from(fee);
        if all_change_to_fee && change > 0 {
            // CoinJoin page: every bit of change is paid as fee.
            fee = fee.max(u64::try_from(change).unwrap_or(u64::MAX));
            change = 0;
            s.change_to_fee = true;
        } else {
            change -= i128::from(fee);
        }
        if change > 0 && (change as u64) < dust_threshold(CHANGE_SCRIPT_LEN) {
            fee = fee.saturating_add(change as u64);
            change = 0;
            s.change_to_fee = true;
        }
        if change <= 0 {
            bytes -= OUTPUT_BYTES;
        }
    }
    s.bytes = u32::try_from(bytes).unwrap_or(u32::MAX);
    s.fee = fee;
    s.change = u64::try_from(change.max(0)).unwrap_or(u64::MAX);
    s.after_fee = amount.saturating_sub(fee);
    s.fee_tolerance_per_input = (fee + bytes / 2).checked_div(bytes).unwrap_or(0);
    s
}

/// The asset-lock builder's fee rate at the pin, duffs per kB:
/// platform-wallet `asset_lock::manager::DEFAULT_FEE_PER_KB` (not exported),
/// which `build_asset_lock_transaction_fenced` passes to key-wallet.
const ASSET_LOCK_FEE_PER_KB: u64 = 1_000;
/// A P2PKH input at its largest: the outpoint (36), the script's length
/// (1), a 72-byte DER signature with its hash type and push (74), a
/// compressed key with its push (34), the sequence (4).
const MAX_INPUT_BYTES: u64 = 149;
/// An asset lock without its inputs, bounded generously: it is at most
/// 103 bytes (version and type 4, the input count's varint at its widest 9,
/// the output count 1, the burn output 12, a change output 34, the lock
/// time 4, and the payload with its length, 39, for one credit output).
const ASSET_LOCK_BASE_BYTES: u64 = 200;
/// key-wallet's `MAX_STANDARD_TX_INPUTS`: its builder refuses a selection
/// of more inputs, so no asset lock spends more.
const MAX_LOCK_INPUTS: usize = 500;

/// E0-04 §2a.3's `fee_bound_worst` for a wallet whose funding accounts hold
/// `inputs` unspent outputs: the fee of an asset lock spending every one of
/// them at the builder's rate, plus a change remainder at the dust
/// threshold, which the builder pays as fee instead of an output. A true
/// upper bound on the fee of any asset lock the library can build from
/// those coins: coin selection picks a subset, every input is at most
/// [`MAX_INPUT_BYTES`], and the fee is the size's at the fixed rate plus
/// at most one dust remainder.
pub(crate) fn asset_lock_fee_bound(inputs: usize) -> u64 {
    let bytes = (inputs.min(MAX_LOCK_INPUTS) as u64)
        .saturating_mul(MAX_INPUT_BYTES)
        .saturating_add(ASSET_LOCK_BASE_BYTES);
    ASSET_LOCK_FEE_PER_KB
        .saturating_mul(bytes)
        .div_ceil(1000)
        .saturating_add(dust_threshold(CHANGE_SCRIPT_LEN))
}

impl NetworkSession {
    /// The coin-control summary for `outpoints` paying `pay_amounts` at
    /// `fee` (QT-072/074). Selected coins that are no longer unspent coins of
    /// the wallet are listed in `unavailable` and left out of the sums.
    pub async fn coin_selection_summary(
        self: &Arc<Self>,
        wallet_id: WalletId,
        outpoints: Vec<OutPoint>,
        pay_amounts: Vec<u64>,
        fee: FeeMode,
        all_change_to_fee: bool,
    ) -> Result<CoinSelectionSummary, EngineError> {
        let rate = fee.rate()?.as_sat_per_kb();
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let wallet = this.wallet(&wallet_id).await?;
            let snapshot = {
                let fence = this.coin_fence(wallet_id).await?;
                this.coin_snapshot(&fence, &wallet).await?
            };
            let unspent: HashMap<OutPoint, u64> = snapshot
                .coins
                .iter()
                .map(|c| (c.utxo.outpoint, c.utxo.value()))
                .collect();
            let mut seen = HashSet::new();
            let mut values = Vec::new();
            let mut unavailable = Vec::new();
            for outpoint in outpoints {
                if !seen.insert(outpoint) {
                    continue;
                }
                match unspent.get(&outpoint) {
                    Some(v) => values.push(*v),
                    None => unavailable.push(outpoint),
                }
            }
            let mut summary = summarize(&values, &pay_amounts, rate, all_change_to_fee);
            summary.unavailable = unavailable;
            Ok(summary)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qt_057_policy_lists_dash_qt_targets_at_the_relay_fee() {
        let p = fee_policy();
        assert_eq!(p.source, FeeSource::MinimumRelay);
        assert_eq!(
            p.targets
                .iter()
                .map(|t| t.target_blocks)
                .collect::<Vec<_>>(),
            FEE_TARGETS
        );
        assert!(p.targets.iter().all(|t| t.duffs_per_kb == 1000));
        assert_eq!(p.min_relay_per_kb, 1000);
        assert_eq!(p.max_tx_fee, 10_000_000);
        assert_eq!(p.max_broadcast_rate_per_kb, 10_000_000);
    }

    #[test]
    fn test_qt_072_bytes_and_fee_follow_dash_qt() {
        // 2 inputs, 1 recipient, change: 148·2 + 34·2 + 10 = 374 bytes.
        let s = summarize(&[100_000_000, 50_000_000], &[120_000_000], 1000, false);
        assert_eq!(s.quantity, 2);
        assert_eq!(s.amount, 150_000_000);
        assert_eq!(s.bytes, 374);
        assert_eq!(s.fee, 374);
        assert_eq!(s.change, 150_000_000 - 120_000_000 - 374);
        assert_eq!(s.after_fee, 150_000_000 - 374);
        assert!(!s.insufficient_funds && !s.change_to_fee);
        assert_eq!(s.fee_tolerance_per_input, 1);

        // No recipients yet: two outputs assumed.
        let s = summarize(&[1_000], &[], 1000, false);
        assert_eq!(s.bytes, 148 + 68 + 10);
        assert_eq!(s.change, 0);
    }

    #[test]
    fn test_qt_072_dust_change_goes_to_fee_and_drops_the_output() {
        // Change of 500 after the fee is dust (< 546).
        let fee = (148 + 68 + 10) as u64;
        let s = summarize(&[100_000 + fee + 500], &[100_000], 1000, false);
        assert_eq!(s.change, 0);
        assert!(s.change_to_fee);
        assert_eq!(s.fee, fee + 500);
        assert_eq!(s.bytes, 148 + 68 + 10 - 34);
    }

    #[test]
    fn test_qt_072_coinjoin_page_pays_all_change_as_fee() {
        let s = summarize(&[1_000_010, 1_000_010], &[1_500_000], 1000, true);
        assert_eq!(s.change, 0);
        assert_eq!(s.fee, 500_020);
        assert!(s.change_to_fee);
    }

    #[test]
    fn the_asset_lock_fee_bound_covers_every_input_at_its_largest() {
        // No coins: the base and a dust remainder.
        assert_eq!(asset_lock_fee_bound(0), 200 + 546);
        // 10 inputs: 1690 bytes at 1000 duff/kB, plus the dust remainder.
        assert_eq!(asset_lock_fee_bound(10), 1_690 + 546);
        // Above what key-wallet's estimate charges for the same inputs
        // (148 bytes each).
        assert!(asset_lock_fee_bound(500) > 500 * 148);
        // No lock spends more than 500 inputs.
        assert_eq!(asset_lock_fee_bound(usize::MAX), asset_lock_fee_bound(500));
        assert_eq!(asset_lock_fee_bound(500), 74_700 + 546);
    }

    #[test]
    fn test_qt_072_insufficient_funds() {
        let s = summarize(&[100_000], &[100_000], 1000, false);
        assert!(s.insufficient_funds);
        let s = summarize(&[], &[1], 1000, false);
        assert!(s.insufficient_funds);
        assert_eq!(s.bytes, 0);
    }
}
