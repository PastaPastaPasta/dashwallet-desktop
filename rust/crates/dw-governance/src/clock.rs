//! Superblock arithmetic from the chain tip alone (QT-026, QT-132, QT-134):
//! the nearest superblocks (Core `CSuperblock::GetNearestSuperblocksHeights`),
//! the voting cutoff, ETAs, and the superblock budget
//! (`CSuperblock::GetPaymentsLimit`, which on every network is a function of
//! the height only).

use dashcore::Network;

use crate::params::{GovernanceParams, governance_params};

const COIN: u64 = 100_000_000;

/// The last and next superblock heights around `height` (Core: before the
/// first superblock, `last` is 0).
pub fn nearest_superblocks(p: &GovernanceParams, height: u32) -> (u32, u32) {
    let cycle = p.superblock_cycle;
    let start = p.superblock_start_height;
    let first = start + (cycle - start % cycle) % cycle;
    if height < first {
        (0, first)
    } else {
        let last = height - height % cycle;
        (last, last + cycle)
    }
}

/// Whether `height` lies inside the maturity window before the next
/// superblock, where voting no longer changes the trigger (dash-qt
/// `Proposal::status`: `height % cycle >= cycle − window`).
pub fn in_maturity_window(p: &GovernanceParams, height: u32) -> bool {
    height % p.superblock_cycle >= p.superblock_cycle - p.maturity_window
}

/// The estimated time of `height` from the tip: `tip_time` plus the target
/// spacing per block (negative distances count back).
pub fn estimated_time(p: &GovernanceParams, tip_height: u32, tip_time: u64, height: u32) -> u64 {
    let blocks = i64::from(height) - i64::from(tip_height);
    let t = tip_time as i64 + blocks * i64::from(p.target_spacing_secs);
    t.max(0) as u64
}

/// The next `count` superblocks after `tip_height` (the Create Proposal
/// "Payment date" choices).
pub fn upcoming_superblocks(p: &GovernanceParams, tip_height: u32, count: u32) -> Vec<u32> {
    let (_, next) = nearest_superblocks(p, tip_height);
    (0..count).map(|i| next + i * p.superblock_cycle).collect()
}

/// The status-bar clock (QT-026).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub cycle_progress: f64,
    pub last_superblock: u32,
    pub next_superblock: u32,
    pub blocks_to_superblock: u32,
    pub superblock_eta: u64,
    pub voting_cutoff: u32,
    pub voting_open: bool,
}

/// The clock at `tip_height` / `tip_time` (`now` stands in when the tip
/// time is older: the ETA is never in the past).
pub fn clock(p: &GovernanceParams, tip_height: u32, eta_base: u64) -> Clock {
    let (last, next) = nearest_superblocks(p, tip_height);
    let cycle_start = if last == 0 {
        next.saturating_sub(p.superblock_cycle)
    } else {
        last
    };
    let elapsed = tip_height.saturating_sub(cycle_start);
    let cycle_progress = (f64::from(elapsed) / f64::from(p.superblock_cycle)).clamp(0.0, 1.0);
    let voting_cutoff = next - p.maturity_window;
    Clock {
        cycle_progress,
        last_superblock: last,
        next_superblock: next,
        blocks_to_superblock: next - tip_height,
        superblock_eta: estimated_time(p, tip_height, eta_base, next),
        voting_cutoff,
        voting_open: tip_height < voting_cutoff,
    }
}

/// Subsidy constants of one network that the superblock budget needs
/// (Dash Core `chainparams.cpp`).
struct SubsidyParams {
    halving_interval: u32,
    budget_payments_start: u32,
    /// V20 activation (treasury 20 % instead of 10 %, fixed subsidy base).
    v20_height: u32,
    /// `fPowAllowMinDifficultyBlocks`: the budget uses the PoW limit's
    /// compact bits instead of 1.
    min_difficulty_bits: Option<u32>,
}

fn subsidy_params(network: Network) -> SubsidyParams {
    match network {
        Network::Mainnet => SubsidyParams {
            halving_interval: 210_240,
            budget_payments_start: 328_008,
            v20_height: 1_987_776,
            min_difficulty_bits: None,
        },
        Network::Testnet => SubsidyParams {
            halving_interval: 210_240,
            budget_payments_start: 4_100,
            v20_height: 905_100,
            // powLimit 00000fff… → compact 0x1e0fffff.
            min_difficulty_bits: Some(0x1e0f_ffff),
        },
        Network::Devnet => SubsidyParams {
            halving_interval: 210_240,
            budget_payments_start: 4_100,
            v20_height: 2,
            // powLimit 7fff… → compact 0x207fffff.
            min_difficulty_bits: Some(0x207f_ffff),
        },
        Network::Regtest => SubsidyParams {
            halving_interval: 150,
            budget_payments_start: 1_000,
            // Core's default (DIP0003Height 432); DashTestFramework moves it
            // with -testactivationheight, which the wallet cannot see.
            v20_height: 432,
            min_difficulty_bits: Some(0x207f_ffff),
        },
    }
}

/// Core `ConvertBitsToDouble`.
fn bits_to_difficulty(bits: u32) -> f64 {
    let mut shift = (bits >> 24) & 0xff;
    let mut diff = f64::from(0x0000_ffffu32) / f64::from(bits & 0x00ff_ffff);
    while shift < 29 {
        diff *= 256.0;
        shift += 1;
    }
    while shift > 29 {
        diff /= 256.0;
        shift -= 1;
    }
    diff
}

/// Core `GetBlockSubsidyHelper(...).second`: the treasury part of the
/// subsidy of the block after `prev_height`.
fn superblock_part(network: Network, prev_bits: u32, prev_height: u32, v20: bool) -> u64 {
    let sp = subsidy_params(network);
    let diff = if prev_height <= 4500 && network == Network::Mainnet {
        f64::from(0x0000_ffffu32) / f64::from(prev_bits & 0x00ff_ffff)
    } else {
        bits_to_difficulty(prev_bits)
    };
    let fixed = v20 || network == Network::Devnet;
    let base: f64 = if fixed {
        5.0
    } else if prev_height < 5465 {
        (1111.0 / (diff + 1.0).powi(2)).clamp(1.0, 500.0)
    } else if prev_height < 17000 || (diff <= 75.0 && prev_height < 24000) {
        (11111.0 / ((diff + 51.0) / 6.0).powi(2)).clamp(25.0, 500.0)
    } else {
        (2_222_222.0 / ((diff + 2600.0) / 9.0).powi(2)).clamp(5.0, 25.0)
    };
    // Core truncates the double into a CAmount of whole coins first.
    let mut subsidy = (base as i64 as u64) * COIN;
    let mut i = sp.halving_interval;
    while i <= prev_height {
        subsidy -= subsidy / 14;
        i += sp.halving_interval;
    }
    if prev_height > sp.budget_payments_start {
        subsidy / if v20 { 5 } else { 10 }
    } else {
        0
    }
}

/// Core `CSuperblock::GetPaymentsLimit(height)`: what the superblock at
/// `height` may pay out, duffs; 0 when `height` is not a superblock.
pub fn superblock_budget(network: Network, height: u32) -> u64 {
    let p = governance_params(network);
    if height < p.superblock_start_height || !height.is_multiple_of(p.superblock_cycle) {
        return 0;
    }
    let sp = subsidy_params(network);
    let bits = sp.min_difficulty_bits.unwrap_or(1);
    let v20 = height >= sp.v20_height;
    superblock_part(network, bits, height - 1, v20) * u64::from(p.superblock_cycle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_qt_026_nearest_superblocks_follow_core() {
        let p = governance_params(Network::Mainnet);
        // The first superblock is the first multiple of the cycle at or
        // after the start height.
        let (last, next) = nearest_superblocks(&p, 100);
        assert_eq!(last, 0);
        assert_eq!(next % p.superblock_cycle, 0);
        assert!(next >= p.superblock_start_height);
        let (last, next) = nearest_superblocks(&p, 2_400_000);
        assert_eq!(last, 2_400_000 - 2_400_000 % 16_616);
        assert_eq!(next, last + 16_616);
        let r = governance_params(Network::Regtest);
        assert_eq!(nearest_superblocks(&r, 1505), (1500, 1520));
        assert_eq!(nearest_superblocks(&r, 1500), (1500, 1520));
        assert_eq!(nearest_superblocks(&r, 10), (0, 1500));
    }

    #[test]
    fn test_qt_026_clock_voting_cutoff_and_progress() {
        let r = governance_params(Network::Regtest);
        let c = clock(&r, 1505, 1_000);
        assert_eq!(
            (c.next_superblock, c.voting_cutoff, c.blocks_to_superblock),
            (1520, 1510, 15)
        );
        assert!(c.voting_open);
        assert!((c.cycle_progress - 0.25).abs() < 1e-9);
        assert_eq!(c.superblock_eta, 1_000 + 15 * 150);
        let c = clock(&r, 1512, 0);
        assert!(!c.voting_open);
        assert!(in_maturity_window(&r, 1512));
        assert!(!in_maturity_window(&r, 1509));
    }

    #[test]
    fn test_qt_134_superblock_budget_matches_core_formula() {
        // Mainnet after V20: 5 DASH base, reduced by 1/14 per 210240 blocks,
        // 20 % to the treasury, times the cycle.
        let h = 2_392_704; // 144 · 16616
        assert_eq!(h % 16_616, 0);
        let mut subsidy: u64 = 5 * COIN;
        let mut i = 210_240;
        while i < h {
            subsidy -= subsidy / 14;
            i += 210_240;
        }
        assert_eq!(superblock_budget(Network::Mainnet, h), subsidy / 5 * 16_616);
        assert_eq!(superblock_budget(Network::Mainnet, h + 1), 0);
        // Regtest before V20 uses the difficulty formula with the PoW limit
        // (difficulty ≈ 0 → base 500 is clamped by the early-era branch).
        assert!(superblock_budget(Network::Regtest, 1500) > 0);
        assert_eq!(superblock_budget(Network::Regtest, 1490), 0);
    }

    #[test]
    fn upcoming_superblocks_start_at_next() {
        let r = governance_params(Network::Regtest);
        assert_eq!(upcoming_superblocks(&r, 1505, 3), vec![1520, 1540, 1560]);
    }
}
