//! Post-V24 denomination promotion and demotion (Dash Core
//! `src/coinjoin/common.h` `ShouldPromoteDenoms`/`ShouldDemoteDenoms`,
//! `client.cpp` `DoAutomaticDenominating`/`SelectRebalanceInputs`,
//! `wallet/coinjoin.cpp` `GetDenominationCounts`/`SelectFullyMixedForPromotion`).
//!
//! A promotion spends ten fully mixed coins of a denomination for one coin of
//! the next larger one; a demotion spends one fully mixed coin for ten of the
//! next smaller one. Both run in a session at the smaller denomination.

use std::collections::HashSet;

use crate::client::MixCoin;
use crate::denoms::DENOMINATIONS;
use crate::messages::denomination_to_amount;

/// Coins of the smaller denomination per coin of the larger (`PROMOTION_RATIO`).
pub const PROMOTION_RATIO: usize = 10;

/// The gap is this fraction of the goal (`GAP_DIVISOR`).
pub const GAP_DIVISOR: u32 = 5;

/// The smallest gap that keeps a conversion from making the opposite one
/// worthwhile (`MIN_GAP_THRESHOLD`).
pub const MIN_GAP_THRESHOLD: u32 = (PROMOTION_RATIO as u32).div_ceil(2);

/// `FLAG_PROMOTION` of `dsa` (`CCoinJoinAccept`).
pub const FLAG_PROMOTION: u8 = 1;
/// `FLAG_DEMOTION` of `dsa`.
pub const FLAG_DEMOTION: u8 = 1 << 1;

/// `GetGapThreshold`.
pub fn gap_threshold(goal: u32) -> u32 {
    (goal / GAP_DIVISOR).max(MIN_GAP_THRESHOLD)
}

fn deficit(count: u32, goal: u32) -> u32 {
    goal.saturating_sub(count)
}

/// `ShouldPromoteDenoms`.
pub fn should_promote(smaller: u32, larger: u32, smaller_fully_mixed: u32, goal: u32) -> bool {
    if smaller < goal / 2 || (smaller_fully_mixed as usize) < PROMOTION_RATIO {
        return false;
    }
    deficit(larger, goal) > deficit(smaller, goal) + gap_threshold(goal)
}

/// `ShouldDemoteDenoms`.
pub fn should_demote(larger: u32, smaller: u32, larger_fully_mixed: u32, goal: u32) -> bool {
    if larger < goal / 2 || larger_fully_mixed < 1 {
        return false;
    }
    deficit(smaller, goal) > deficit(larger, goal) + gap_threshold(goal)
}

/// The denomination bit one step larger (`GetLargerAdjacentDenom`); `None`
/// for the largest or an invalid one.
pub fn larger_denom(denom: u32) -> Option<u32> {
    denomination_to_amount(denom)?;
    (denom > 1).then_some(denom >> 1)
}

/// Which way a conversion goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebalanceKind {
    Promotion,
    Demotion,
}

impl RebalanceKind {
    /// The `dsa` flag that declares it.
    pub fn dsa_flag(self) -> u8 {
        match self {
            Self::Promotion => FLAG_PROMOTION,
            Self::Demotion => FLAG_DEMOTION,
        }
    }

    /// Inputs of the entry.
    pub fn input_count(self) -> usize {
        match self {
            Self::Promotion => PROMOTION_RATIO,
            Self::Demotion => 1,
        }
    }

    /// Outputs of the entry.
    pub fn output_count(self) -> usize {
        match self {
            Self::Promotion => 1,
            Self::Demotion => PROMOTION_RATIO,
        }
    }
}

/// A conversion worth making: `denom` is the session denomination, the
/// smaller of the two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rebalance {
    pub denom: u32,
    pub kind: RebalanceKind,
}

impl Rebalance {
    /// The amount of the entry's inputs.
    pub fn input_amount(&self) -> Option<u64> {
        match self.kind {
            RebalanceKind::Promotion => denomination_to_amount(self.denom),
            RebalanceKind::Demotion => denomination_to_amount(larger_denom(self.denom)?),
        }
    }

    /// The amount of the entry's outputs.
    pub fn output_amount(&self) -> Option<u64> {
        match self.kind {
            RebalanceKind::Promotion => denomination_to_amount(larger_denom(self.denom)?),
            RebalanceKind::Demotion => denomination_to_amount(self.denom),
        }
    }
}

/// Per-denomination coin counts, indexed as [`DENOMINATIONS`]
/// (`CoinJoinDenomCounts`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DenomCounts {
    pub total: [u32; 5],
    pub fully_mixed: [u32; 5],
}

/// The conversions `DoAutomaticDenominating` would look for, larger pairs
/// first and a pair's promotion before its demotion. Nothing unless V24
/// is known to be active.
pub fn opportunities(counts: &DenomCounts, goal: u32, v24_active: bool) -> Vec<Rebalance> {
    let mut out = Vec::new();
    if !v24_active {
        return out;
    }
    for i in 0..DENOMINATIONS.len() - 1 {
        let denom = 1u32 << (i + 1);
        let (larger, smaller) = (i, i + 1);
        if should_promote(
            counts.total[smaller],
            counts.total[larger],
            counts.fully_mixed[smaller],
            goal,
        ) {
            out.push(Rebalance {
                denom,
                kind: RebalanceKind::Promotion,
            });
        }
        if should_demote(
            counts.total[larger],
            counts.total[smaller],
            counts.fully_mixed[larger],
            goal,
        ) {
            out.push(Rebalance {
                denom,
                kind: RebalanceKind::Demotion,
            });
        }
    }
    out
}

/// The entry's inputs from fully mixed, spendable `coins` (shuffled by the
/// caller): for a promotion ten of the session amount, distinct
/// transactions first, then siblings (`SelectFullyMixedForPromotion`); for a
/// demotion one of the larger amount (`SelectTxDSInsByDenomination` cut to
/// one). No two inputs pay the same script. `None` when there are not
/// enough.
pub fn select_inputs(rebalance: &Rebalance, coins: &[MixCoin]) -> Option<Vec<MixCoin>> {
    let amount = rebalance.input_amount()?;
    let want = rebalance.kind.input_count();
    let mut txids = HashSet::new();
    // Dash Core refuses an entry whose inputs pay one script twice, and can
    // take the collateral for it (`IsValidInOuts`): a script counts as used
    // only once a coin paying it is picked.
    let mut scripts = HashSet::new();
    let mut picked = Vec::new();
    let mut siblings = Vec::new();
    for c in coins.iter().filter(|c| c.value == amount) {
        if !txids.contains(&c.outpoint.txid) && !scripts.contains(&c.script_pubkey) {
            txids.insert(c.outpoint.txid);
            scripts.insert(c.script_pubkey.clone());
            picked.push(c.clone());
        } else {
            siblings.push(c);
        }
    }
    if rebalance.kind == RebalanceKind::Promotion {
        for c in siblings {
            if picked.len() == want {
                break;
            }
            if scripts.insert(c.script_pubkey.clone()) {
                picked.push(c.clone());
            }
        }
    }
    picked.truncate(want);
    (picked.len() == want).then_some(picked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::hashes::Hash;
    use dashcore::{OutPoint, ScriptBuf, Txid};

    fn promote(smaller: u32, larger: u32, goal: u32) -> bool {
        should_promote(smaller, larger, smaller, goal)
    }

    fn demote(larger: u32, smaller: u32, goal: u32) -> bool {
        should_demote(larger, smaller, larger, goal)
    }

    #[test]
    fn the_gap_follows_core() {
        assert_eq!(MIN_GAP_THRESHOLD, 5);
        assert_eq!(gap_threshold(50), 10);
        assert_eq!(gap_threshold(10), 5);
        assert_eq!(gap_threshold(24), 5);
        assert_eq!(gap_threshold(25), 5);
        assert_eq!(gap_threshold(30), 6);
        assert_eq!(gap_threshold(300), 60);
    }

    #[test]
    fn promotion_needs_ten_fully_mixed_coins() {
        assert!(should_promote(100, 0, 10, 100));
        assert!(!should_promote(100, 0, 9, 100));
        assert!(!should_promote(100, 0, 0, 100));
    }

    #[test]
    fn demotion_needs_one_fully_mixed_coin() {
        assert!(should_demote(100, 0, 1, 100));
        assert!(!should_demote(100, 0, 0, 100));
    }

    #[test]
    fn half_the_goal_is_kept_back() {
        assert!(!promote(49, 0, 100));
        assert!(promote(50, 0, 100));
        assert!(promote(60, 0, 100));
        assert!(!demote(49, 0, 100));
        assert!(demote(50, 0, 100));
        assert!(demote(99, 0, 100));
        assert!(!promote(100, 100, 100));
        assert!(!demote(100, 100, 100));
    }

    /// At the default goal of 50 (gap 10): the larger deficit must exceed
    /// the smaller's by more than 10.
    #[test]
    fn the_gap_must_be_cleared_at_the_default_goal() {
        // 50 smaller (deficit 0), 39 larger (deficit 11): 11 > 0 + 10.
        assert!(promote(50, 39, 50));
        // 40 larger (deficit 10): 10 is not above 10.
        assert!(!promote(50, 40, 50));
        // 45 smaller (deficit 5), 34 larger (deficit 16): 16 > 15.
        assert!(promote(45, 34, 50));
        assert!(!promote(45, 35, 50));
        // 24 smaller is below half the goal.
        assert!(!promote(24, 0, 50));
        assert!(promote(25, 0, 50));
        // Demotion mirrors it.
        assert!(demote(50, 39, 50));
        assert!(!demote(50, 40, 50));
        assert!(!demote(24, 0, 50));
        assert!(demote(25, 0, 50));
        // Counts above the goal have no deficit.
        assert!(promote(80, 39, 50));
        assert!(!promote(80, 60, 50));
    }

    #[test]
    fn the_smallest_goal_still_converts() {
        assert!(promote(10, 0, 10));
        assert!(demote(10, 0, 10));
        assert!(!promote(10, 9, 10));
        assert!(!demote(10, 9, 10));
        // Gap 5 at goal 10: larger deficit 6 > 0 + 5, 5 is not.
        assert!(promote(10, 4, 10));
        assert!(!promote(10, 5, 10));
    }

    /// Core's `rebalance_never_immediately_reverses`.
    #[test]
    fn a_conversion_is_never_undone_by_the_next() {
        let r = PROMOTION_RATIO as u32;
        for goal in 10..=200u32 {
            for smaller in 0..=2 * goal {
                for larger in 0..=2 * goal {
                    if promote(smaller, larger, goal) {
                        assert!(
                            !demote(larger + 1, smaller - r, goal),
                            "goal {goal}: {smaller}/{larger}"
                        );
                    }
                    if demote(larger, smaller, goal) {
                        assert!(
                            !promote(smaller + r, larger - 1, goal),
                            "goal {goal}: {larger}/{smaller}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn opportunities_scan_every_adjacent_pair_only_when_active() {
        let mut counts = DenomCounts::default();
        // 1 DASH (index 1): none; 0.1 DASH (index 2): 50, all fully mixed;
        // 0.01 DASH (index 3): 30, one fully mixed; 0.001 DASH: none.
        counts.total[2] = 50;
        counts.fully_mixed[2] = 50;
        counts.total[3] = 30;
        counts.fully_mixed[3] = 1;
        let found = opportunities(&counts, 50, true);
        assert_eq!(
            found,
            vec![
                // Ten 0.1 into one 1 DASH: deficit 50 > 0 + 10.
                Rebalance {
                    denom: 4,
                    kind: RebalanceKind::Promotion
                },
                // One 0.1 into ten 0.01: deficit 20 > 0 + 10.
                Rebalance {
                    denom: 8,
                    kind: RebalanceKind::Demotion
                },
                // One 0.01 into ten 0.001: deficit 50 > 20 + 10; no
                // promotion of 0.01, only one of them is fully mixed.
                Rebalance {
                    denom: 16,
                    kind: RebalanceKind::Demotion
                },
            ]
        );
        assert!(opportunities(&counts, 50, false).is_empty());
    }

    #[test]
    fn amounts_and_adjacent_denominations() {
        assert_eq!(larger_denom(1), None);
        assert_eq!(larger_denom(2), Some(1));
        assert_eq!(larger_denom(16), Some(8));
        assert_eq!(larger_denom(3), None);
        assert_eq!(larger_denom(32), None);
        let p = Rebalance {
            denom: 4,
            kind: RebalanceKind::Promotion,
        };
        assert_eq!(p.input_amount(), Some(DENOMINATIONS[2]));
        assert_eq!(p.output_amount(), Some(DENOMINATIONS[1]));
        let d = Rebalance {
            denom: 4,
            kind: RebalanceKind::Demotion,
        };
        assert_eq!(d.input_amount(), Some(DENOMINATIONS[1]));
        assert_eq!(d.output_amount(), Some(DENOMINATIONS[2]));
        let top = Rebalance {
            denom: 1,
            kind: RebalanceKind::Demotion,
        };
        assert_eq!(top.input_amount(), None);
        for (i, w) in DENOMINATIONS.windows(2).enumerate() {
            assert_eq!(w[0], w[1] * PROMOTION_RATIO as u64, "denomination {i}");
        }
    }

    fn coin(tx: u8, vout: u32, value: u64) -> MixCoin {
        MixCoin {
            outpoint: OutPoint::new(Txid::from_byte_array([tx; 32]), vout),
            value,
            script_pubkey: ScriptBuf::from_bytes(vec![tx, vout as u8]),
            rounds: 4,
        }
    }

    /// A sibling's script is not claimed before the sibling is picked: a
    /// coin of a distinct transaction paying it still comes first.
    #[test]
    fn a_skipped_sibling_does_not_take_a_distinct_transactions_script() {
        let p = Rebalance {
            denom: 4,
            kind: RebalanceKind::Promotion,
        };
        let a = DENOMINATIONS[2];
        let with = |tx: u8, vout: u32, script: u8| {
            let mut c = coin(tx, vout, a);
            c.script_pubkey = ScriptBuf::from_bytes(vec![script]);
            c
        };
        // Transaction 1 pays X and Y, transaction 2 pays Y, eight more pay
        // their own: ten distinct transactions, ten distinct scripts.
        let mut coins = vec![with(1, 0, 100), with(1, 1, 101), with(2, 0, 101)];
        coins.extend((3..=10).map(|t| with(t, 0, t)));
        let picked = select_inputs(&p, &coins).unwrap();
        let txids: HashSet<_> = picked.iter().map(|c| c.outpoint.txid).collect();
        assert_eq!(txids.len(), 10);
        let scripts: HashSet<_> = picked.iter().map(|c| &c.script_pubkey).collect();
        assert_eq!(scripts.len(), 10);
    }

    /// Dash Core refuses inputs that pay one script twice: a coin whose
    /// script another picked coin pays is left out.
    #[test]
    fn promotion_inputs_pay_distinct_scripts() {
        let p = Rebalance {
            denom: 4,
            kind: RebalanceKind::Promotion,
        };
        let a = DENOMINATIONS[2];
        let mut coins: Vec<MixCoin> = (0..10).map(|t| coin(t + 1, 0, a)).collect();
        let mut same = coin(20, 0, a);
        same.script_pubkey = coins[0].script_pubkey.clone();
        coins[9] = same;
        assert!(select_inputs(&p, &coins).is_none(), "nine distinct scripts");
        coins.push(coin(30, 0, a));
        let picked = select_inputs(&p, &coins).unwrap();
        let scripts: HashSet<_> = picked.iter().map(|c| &c.script_pubkey).collect();
        assert_eq!(scripts.len(), 10);
    }

    #[test]
    fn promotion_inputs_prefer_distinct_transactions_then_siblings() {
        let p = Rebalance {
            denom: 4,
            kind: RebalanceKind::Promotion,
        };
        let a = DENOMINATIONS[2];
        let mut coins: Vec<MixCoin> = (0..6).map(|v| coin(1, v, a)).collect();
        coins.extend((2..8).map(|t| coin(t, 0, a)));
        coins.push(coin(9, 0, DENOMINATIONS[1]));
        let picked = select_inputs(&p, &coins).unwrap();
        assert_eq!(picked.len(), 10);
        let txids: HashSet<_> = picked.iter().map(|c| c.outpoint.txid).collect();
        assert_eq!(
            txids.len(),
            7,
            "every distinct transaction before a sibling"
        );
        assert!(picked.iter().all(|c| c.value == a));
        assert!(select_inputs(&p, &coins[..9]).is_none());
    }

    #[test]
    fn a_demotion_takes_one_larger_coin() {
        let d = Rebalance {
            denom: 4,
            kind: RebalanceKind::Demotion,
        };
        let coins = vec![coin(1, 0, DENOMINATIONS[2]), coin(2, 0, DENOMINATIONS[1])];
        let picked = select_inputs(&d, &coins).unwrap();
        assert_eq!(picked, vec![coins[1].clone()]);
        assert!(select_inputs(&d, &coins[..1]).is_none());
    }
}
