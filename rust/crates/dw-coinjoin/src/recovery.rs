//! IOS-057 "move mixed coins": the CoinJoin account's coins above a small
//! threshold, swept in chunks of at most 500 inputs, one output each
//! (dashwallet-iOS chunks its CoinJoin sweep the same way so a transaction
//! stays below the 100 kB standardness limit: 500 × 148 B ≈ 74 kB).
//!
//! The same chunks consolidate the account before a move to Shielded
//! (dashwallet-iOS #1221): an asset lock spends at most 100 inputs (Dash
//! Core v24 `MAX_STANDARD_ASSET_LOCK_INPUTS`) and its shield transition
//! outgrows Platform's 20 480-byte limit from about 65, so an account with
//! more coins than one lock takes is first swept onto fresh addresses of its
//! own CoinJoin account ([`plan_consolidation`]).

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
    plan_sweep_chunked(values, fee_per_kb, SWEEP_MAX_INPUTS)
}

/// [`plan_sweep`] with at most `max_inputs` inputs per chunk.
pub fn plan_sweep_chunked(values: &[u64], fee_per_kb: u64, max_inputs: usize) -> Vec<SweepChunk> {
    let order = largest_first(values);
    priced(values, order.chunks(max_inputs.max(1)), fee_per_kb)
}

/// The consolidation round of a move to Shielded: every coin of `values`
/// above [`SWEEP_MIN_COIN`] in the fewest chunks of at most `max_inputs`
/// inputs, their sizes as even as they can be (a round of 501 coins is two
/// chunks of 251 and 250, not 500 and 1). A chunk whose value does not cover
/// its fee is dropped.
pub fn plan_consolidation(values: &[u64], fee_per_kb: u64, max_inputs: usize) -> Vec<SweepChunk> {
    let order = largest_first(values);
    let max = max_inputs.max(1);
    let count = order.len().div_ceil(max);
    let mut chunks = Vec::with_capacity(count);
    let mut rest = order.as_slice();
    for left in (1..=count).rev() {
        let (chunk, tail) = rest.split_at(rest.len().div_ceil(left));
        chunks.push(chunk);
        rest = tail;
    }
    priced(values, chunks.into_iter(), fee_per_kb)
}

/// The coins of `values` above [`SWEEP_MIN_COIN`], largest first.
fn largest_first(values: &[u64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..values.len())
        .filter(|i| values[*i] > SWEEP_MIN_COIN)
        .collect();
    order.sort_by(|a, b| values[*b].cmp(&values[*a]).then(a.cmp(b)));
    order
}

fn priced<'a>(
    values: &[u64],
    chunks: impl Iterator<Item = &'a [usize]>,
    fee_per_kb: u64,
) -> Vec<SweepChunk> {
    chunks
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

    /// The fewest chunks under the cap, balanced, every coin above the
    /// threshold once, and the value conserved less the fees.
    #[test]
    fn a_consolidation_takes_the_fewest_balanced_chunks_under_the_cap() {
        let mut values = vec![100_001u64; 501];
        values.push(900); // left behind, as a sweep leaves it
        let plan = plan_consolidation(&values, 1_000, SWEEP_MAX_INPUTS);
        let sizes: Vec<usize> = plan.iter().map(|c| c.coins.len()).collect();
        assert_eq!(sizes, [251, 250]);
        let mut seen: Vec<usize> = plan.iter().flat_map(|c| c.coins.clone()).collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..501).collect::<Vec<_>>(),
            "each coin once, the dust none"
        );
        let moved: u64 = plan.iter().map(|c| c.amount + c.fee).sum();
        assert_eq!(moved, 100_001 * 501);

        for (n, max, want) in [
            (0, 500, vec![]),
            (50, 500, vec![50]),
            (500, 500, vec![500]),
            (1_001, 500, vec![334, 334, 333]),
            (7, 3, vec![3, 2, 2]),
        ] {
            let plan = plan_consolidation(&vec![10_000u64; n], 1_000, max);
            let sizes: Vec<usize> = plan.iter().map(|c| c.coins.len()).collect();
            assert_eq!(sizes, want, "{n} coins, at most {max} per chunk");
            assert!(sizes.iter().all(|s| *s <= max));
        }
        // A chunk of dust that cannot pay its own fee is dropped.
        assert!(plan_consolidation(&[1_001, 1_001], 10_000, 500).is_empty());
    }
}
