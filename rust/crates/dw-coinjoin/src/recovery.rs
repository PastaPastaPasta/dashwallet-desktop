//! IOS-057 "move mixed coins": the CoinJoin account's coins above a small
//! threshold, swept in chunks of at most 500 inputs, one output each
//! (dashwallet-iOS chunks its CoinJoin sweep the same way so a transaction
//! stays below the 100 kB standardness limit: 500 × 148 B ≈ 74 kB).

use crate::planner::{P2PKH_INPUT_SIZE, P2PKH_OUTPUT_SIZE, compact_size_len, fee_for_size};

/// Coins at or below this value are left behind (they cost about as much
/// to spend as they are worth).
pub const SWEEP_MIN_COIN: u64 = 1_000;
/// Inputs per sweep transaction.
pub const SWEEP_MAX_INPUTS: usize = 500;

/// One planned sweep transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SweepChunk {
    /// Indices into the coin list passed to [`plan_sweep`].
    pub coins: Vec<usize>,
    /// What the single output receives.
    pub amount: u64,
    pub fee: u64,
}

/// Size of a sweep transaction with `inputs` P2PKH inputs and one output.
pub fn sweep_size(inputs: usize) -> usize {
    8 + compact_size_len(inputs) + inputs * P2PKH_INPUT_SIZE + 1 + P2PKH_OUTPUT_SIZE
}

/// Splits `values` (coin values, any order) into chunks, largest coins
/// first. A chunk whose value does not cover its fee is dropped.
pub fn plan_sweep(values: &[u64], fee_per_kb: u64) -> Vec<SweepChunk> {
    let mut order: Vec<usize> = (0..values.len())
        .filter(|i| values[*i] > SWEEP_MIN_COIN)
        .collect();
    order.sort_by(|a, b| values[*b].cmp(&values[*a]).then(a.cmp(b)));
    order
        .chunks(SWEEP_MAX_INPUTS)
        .filter_map(|chunk| {
            let total: u64 = chunk.iter().map(|i| values[*i]).sum();
            let fee = fee_for_size(fee_per_kb, sweep_size(chunk.len()));
            (total > fee).then(|| SweepChunk {
                coins: chunk.to_vec(),
                amount: total - fee,
                fee,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ios_057_sweep_chunks_of_500_inputs() {
        let mut values = vec![100_001u64; 1_203];
        values.push(900); // below the threshold
        let plan = plan_sweep(&values, 1_000);
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].coins.len(), 500);
        assert_eq!(plan[2].coins.len(), 203);
        assert_eq!(plan[0].fee, fee_for_size(1_000, sweep_size(500)));
        let moved: u64 = plan.iter().map(|c| c.amount + c.fee).sum();
        assert_eq!(moved, 100_001 * 1_203);
        assert!(plan_sweep(&[1_000, 500], 1_000).is_empty());
        // 3 bytes of compact size from 253 inputs on.
        assert_eq!(sweep_size(253) - sweep_size(252), P2PKH_INPUT_SIZE + 2);
    }
}
