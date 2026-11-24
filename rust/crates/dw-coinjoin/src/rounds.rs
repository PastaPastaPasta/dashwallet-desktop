//! Mixing rounds per outpoint and the "fully mixed" rule (QT-043).
//!
//! [`RoundsCalculator::rounds`] is Dash Core's
//! `CWallet::GetRealOutpointCoinJoinRounds` (`src/wallet/coinjoin.cpp:261-370`)
//! over the wallet's own transactions; [`is_fully_mixed`] is
//! `CWallet::IsFullyMixed` (coinjoin.cpp:405-426).

use std::collections::HashMap;

use dashcore::hashes::sha256;
use dashcore::{OutPoint, Txid};

use crate::denoms::{COLLATERAL_AMOUNT, MAX_COLLATERAL_AMOUNT, is_denominated};
use crate::messages::amount_to_denomination;
use crate::settings::MAX_ROUNDS;

/// `COINJOIN_RANDOM_ROUNDS` (`src/coinjoin/options.h:51`): rounds a coin
/// may get beyond the target before it surely counts as fully mixed.
pub const RANDOM_ROUNDS: u32 = 3;

/// Depth limit of the walk (`MAX_COINJOIN_ROUNDS + GetRandomRounds()`).
pub const ROUNDS_MAX: i32 = (MAX_ROUNDS + RANDOM_ROUNDS) as i32;

/// Core's special results (coinjoin.cpp:289-311).
pub const ROUNDS_UNKNOWN_TX: i32 = -1;
pub const ROUNDS_NOT_DENOMINATED: i32 = -2;
pub const ROUNDS_COLLATERAL: i32 = -3;
pub const ROUNDS_BAD_INDEX: i32 = -4;

/// What the walk needs to know about one wallet transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WalletTx {
    /// Output values, by index.
    pub outputs: Vec<u64>,
    /// Which outputs pay the wallet.
    pub outputs_mine: Vec<bool>,
    /// Every input's previous outpoint, with the value when the input spends
    /// a wallet output (`InputIsMine`), `None` otherwise.
    pub inputs: Vec<(OutPoint, Option<u64>)>,
}

impl WalletTx {
    /// `CachedTxGetDebit(ISMINE_SPENDABLE)`: value of the wallet's inputs.
    fn debit(&self) -> u64 {
        self.inputs.iter().filter_map(|(_, v)| *v).sum()
    }

    /// `CachedTxGetCredit(ISMINE_SPENDABLE)`: value of outputs to the wallet.
    fn credit(&self) -> u64 {
        self.outputs
            .iter()
            .zip(&self.outputs_mine)
            .filter(|(_, mine)| **mine)
            .map(|(v, _)| *v)
            .sum()
    }
}

/// Collateral amount (`CoinJoin::IsCollateralAmount`, common.h:252-256).
pub fn is_collateral_amount(value: u64) -> bool {
    (COLLATERAL_AMOUNT..=MAX_COLLATERAL_AMOUNT).contains(&value)
}

#[derive(Clone, Copy)]
enum CachedRounds {
    InProgress,
    Exact(i32),
    AtLeast(i32),
}

/// The walk over one wallet's transactions, with Core's memo
/// (`mapOutpointRoundsCache`).
pub struct RoundsCalculator<'a> {
    txs: &'a HashMap<Txid, WalletTx>,
    cache: HashMap<(Txid, u32), CachedRounds>,
    tx_balanced_denoms: HashMap<Txid, bool>,
}

impl<'a> RoundsCalculator<'a> {
    pub fn new(txs: &'a HashMap<Txid, WalletTx>) -> Self {
        Self {
            txs,
            cache: HashMap::new(),
            tx_balanced_denoms: HashMap::new(),
        }
    }

    /// Rounds of `outpoint`: 0 for a fresh denomination, `n + 1` for a
    /// mixing output whose shortest own-input chain has `n`, negative for
    /// the special cases above.
    pub fn rounds(&mut self, outpoint: &OutPoint) -> i32 {
        self.rounds_at(outpoint, 0).0
    }

    fn rounds_at(&mut self, outpoint: &OutPoint, depth: i32) -> (i32, bool) {
        let Some(tx) = self.txs.get(&outpoint.txid) else {
            return (ROUNDS_UNKNOWN_TX, true);
        };
        let Some(&value) = tx.outputs.get(outpoint.vout as usize) else {
            return (ROUNDS_BAD_INDEX, true);
        };
        if is_collateral_amount(value) {
            return (ROUNDS_COLLATERAL, true);
        }
        if !is_denominated(value) {
            return (ROUNDS_NOT_DENOMINATED, true);
        }
        if !self.balanced_denoms(outpoint.txid, tx) {
            return (0, true);
        }
        let output_denom = amount_to_denomination(value);
        let key = (outpoint.txid, output_denom);
        match self.cache.get(&key).copied() {
            Some(CachedRounds::InProgress) => return (-10, false),
            Some(CachedRounds::Exact(r)) => return (r, true),
            Some(CachedRounds::AtLeast(lb)) if lb >= ROUNDS_MAX - depth => return (lb, false),
            _ => {}
        }
        if depth >= ROUNDS_MAX {
            return (0, false);
        }
        let prev_lb = match self.cache.get(&key) {
            Some(&CachedRounds::AtLeast(lb)) => Some(lb),
            _ => None,
        };
        self.cache.insert(key, CachedRounds::InProgress);
        let (rounds, exact, has_cycle) = self.compute(tx, output_denom, depth);
        if exact {
            self.cache.insert(key, CachedRounds::Exact(rounds));
            (rounds, true)
        } else if !has_cycle {
            let lb = prev_lb.map_or(rounds, |old| old.max(rounds));
            self.cache.insert(key, CachedRounds::AtLeast(lb));
            (lb, false)
        } else {
            match prev_lb {
                Some(lb) => {
                    self.cache.insert(key, CachedRounds::AtLeast(lb));
                }
                None => {
                    self.cache.remove(&key);
                }
            }
            (rounds, false)
        }
    }

    fn balanced_denoms(&mut self, txid: Txid, tx: &WalletTx) -> bool {
        *self.tx_balanced_denoms.entry(txid).or_insert_with(|| {
            tx.outputs.iter().all(|v| is_denominated(*v)) && tx.debit() == tx.credit()
        })
    }

    fn compute(&mut self, tx: &'a WalletTx, output_denom: u32, depth: i32) -> (i32, bool, bool) {
        let mut shortest: Option<i32> = None;
        let mut exact_shortest: Option<i32> = None;
        let mut has_cycle = false;
        for &(prev, own) in &tx.inputs {
            if own.is_none() {
                continue;
            }
            if let Some(prev_tx) = self.txs.get(&prev.txid)
                && let Some(prev_value) = prev_tx.outputs.get(prev.vout as usize)
                && amount_to_denomination(*prev_value) != output_denom
            {
                return (0, true, false);
            }
            let (n, child_exact) = self.rounds_at(&prev, depth + 1);
            if n == -10 {
                has_cycle = true;
            }
            if n >= 0 {
                if shortest.is_none_or(|s| n < s) {
                    shortest = Some(n);
                }
                if child_exact && exact_shortest.is_none_or(|s| n < s) {
                    exact_shortest = Some(n);
                }
            }
        }
        let rounds = match shortest {
            Some(s) if s >= ROUNDS_MAX - 1 => ROUNDS_MAX,
            Some(s) => s + 1,
            None => 0,
        };
        let exact = !has_cycle
            && (rounds >= ROUNDS_MAX
                || match shortest {
                    Some(s) => exact_shortest == Some(s),
                    None => true,
                });
        (rounds, exact, has_cycle)
    }
}

/// `SHA256(outpoint ‖ salt)` read as a little-endian u64 is odd
/// (coinjoin.cpp:414-421). `salt` is the uint256 in internal byte order
/// (the reverse of the hex `coinjoinsalt get` prints).
pub fn salted_hash_is_odd(outpoint: &OutPoint, salt: &[u8; 32]) -> bool {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(outpoint.txid.as_byte_array());
    data.extend_from_slice(&outpoint.vout.to_le_bytes());
    data.extend_from_slice(salt);
    let h = sha256::Hash::hash(&data);
    let b = h.as_byte_array();
    let low = u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    low % 2 == 1
}

/// QT-043: at least `target` rounds, and either `target + 3` rounds or an
/// odd salted hash (about half the coins stop at the target, a quarter at
/// one more, …).
pub fn is_fully_mixed(rounds: i32, target: u32, outpoint: &OutPoint, salt: &[u8; 32]) -> bool {
    let target = target as i32;
    if rounds < target {
        return false;
    }
    if rounds < target + RANDOM_ROUNDS as i32 {
        return salted_hash_is_odd(outpoint, salt);
    }
    true
}

/// `GetCappedOutpointCoinJoinRounds` (coinjoin.cpp:372-378).
pub fn capped_rounds(rounds: i32, target: u32) -> i32 {
    rounds.min(target as i32)
}

/// Converts the hex Core prints for a uint256 (display order) into its
/// internal bytes.
pub fn salt_from_display_hex(hex_str: &str) -> Option<[u8; 32]> {
    if hex_str.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, chunk) in hex_str.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).ok()?;
        out[31 - i] = u8::from_str_radix(s, 16).ok()?;
    }
    Some(out)
}

/// The display hex of internal salt bytes.
pub fn salt_to_display_hex(salt: &[u8; 32]) -> String {
    salt.iter().rev().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::denoms::DENOMINATIONS;

    fn txid(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    const D: u64 = DENOMINATIONS[2]; // 0.100001

    /// A funding tx creating `D` at output 0 next to change.
    fn funding() -> WalletTx {
        WalletTx {
            outputs: vec![D, 5_000_000],
            outputs_mine: vec![true, true],
            inputs: vec![(OutPoint::new(txid(0xee), 0), Some(5_100_200))],
        }
    }

    /// A mixing tx spending our `prev` and a foreign coin into `D` x2.
    fn mixing(prev: OutPoint) -> WalletTx {
        WalletTx {
            outputs: vec![D, D],
            outputs_mine: vec![true, false],
            inputs: vec![(prev, Some(D)), (OutPoint::new(txid(0xaa), 3), None)],
        }
    }

    #[test]
    fn rounds_follow_the_input_chain() {
        let mut txs = HashMap::new();
        txs.insert(txid(1), funding());
        txs.insert(txid(2), mixing(OutPoint::new(txid(1), 0)));
        txs.insert(txid(3), mixing(OutPoint::new(txid(2), 0)));
        let mut calc = RoundsCalculator::new(&txs);
        assert_eq!(calc.rounds(&OutPoint::new(txid(1), 0)), 0);
        assert_eq!(
            calc.rounds(&OutPoint::new(txid(1), 1)),
            ROUNDS_NOT_DENOMINATED
        );
        assert_eq!(calc.rounds(&OutPoint::new(txid(2), 0)), 1);
        assert_eq!(calc.rounds(&OutPoint::new(txid(3), 0)), 2);
        assert_eq!(calc.rounds(&OutPoint::new(txid(3), 9)), ROUNDS_BAD_INDEX);
        assert_eq!(calc.rounds(&OutPoint::new(txid(9), 0)), ROUNDS_UNKNOWN_TX);
    }

    #[test]
    fn collateral_and_fee_paying_transactions() {
        let mut txs = HashMap::new();
        txs.insert(
            txid(1),
            WalletTx {
                outputs: vec![MAX_COLLATERAL_AMOUNT, D],
                outputs_mine: vec![true, true],
                inputs: vec![(OutPoint::new(txid(0xee), 0), Some(D + 50_000))],
            },
        );
        // All outputs denominated but our debit exceeds our credit (a fee).
        txs.insert(
            txid(2),
            WalletTx {
                outputs: vec![D],
                outputs_mine: vec![true],
                inputs: vec![(OutPoint::new(txid(0xee), 1), Some(D + 1))],
            },
        );
        let mut calc = RoundsCalculator::new(&txs);
        assert_eq!(calc.rounds(&OutPoint::new(txid(1), 0)), ROUNDS_COLLATERAL);
        assert_eq!(calc.rounds(&OutPoint::new(txid(1), 1)), 0);
        assert_eq!(calc.rounds(&OutPoint::new(txid(2), 0)), 0);
    }

    #[test]
    fn rounds_cap_at_the_walk_limit() {
        let mut txs = HashMap::new();
        txs.insert(txid(0), funding());
        for i in 1..=25u8 {
            txs.insert(txid(i), mixing(OutPoint::new(txid(i - 1), 0)));
        }
        let mut calc = RoundsCalculator::new(&txs);
        assert_eq!(calc.rounds(&OutPoint::new(txid(5), 0)), 5);
        let mut calc = RoundsCalculator::new(&txs);
        assert_eq!(calc.rounds(&OutPoint::new(txid(25), 0)), ROUNDS_MAX);
    }

    #[test]
    fn a_depth_capped_walk_does_not_fully_mix_the_shallow_sibling() {
        let mut txs = HashMap::new();
        txs.insert(
            txid(0),
            WalletTx {
                outputs: vec![D, D, 5_000_000],
                outputs_mine: vec![true, true, true],
                inputs: vec![(OutPoint::new(txid(0xee), 0), Some(2 * D + 5_000_000))],
            },
        );
        txs.insert(
            txid(1),
            WalletTx {
                outputs: vec![D, D],
                outputs_mine: vec![true, true],
                inputs: vec![
                    (OutPoint::new(txid(0), 0), Some(D)),
                    (OutPoint::new(txid(0), 1), Some(D)),
                ],
            },
        );
        txs.insert(
            txid(2),
            WalletTx {
                outputs: vec![D, D],
                outputs_mine: vec![true, true],
                inputs: vec![
                    (OutPoint::new(txid(1), 0), Some(D)),
                    (OutPoint::new(txid(1), 1), Some(D)),
                ],
            },
        );
        let branch = OutPoint::new(txid(2), 0);
        let sibling = OutPoint::new(txid(2), 1);
        let mut prev = branch;
        for i in 3..(2 + ROUNDS_MAX as u8) {
            txs.insert(txid(i), mixing(prev));
            prev = OutPoint::new(txid(i), 0);
        }
        let tip = prev;
        let salt = [0u8; 32];
        for deep_first in [true, false] {
            let mut calc = RoundsCalculator::new(&txs);
            if deep_first {
                assert_eq!(calc.rounds(&tip), ROUNDS_MAX);
                assert_eq!(calc.rounds(&sibling), 2);
                assert_eq!(calc.rounds(&branch), 2);
            } else {
                assert_eq!(calc.rounds(&sibling), 2);
                assert_eq!(calc.rounds(&branch), 2);
                assert_eq!(calc.rounds(&tip), ROUNDS_MAX);
            }
            assert!(!is_fully_mixed(calc.rounds(&sibling), 4, &sibling, &salt));
        }
    }

    #[test]
    fn dense_deep_mixing_dag_stays_linear_and_exact() {
        const WIDTH: usize = 5;
        let mut txs = HashMap::new();
        txs.insert(
            txid(0),
            WalletTx {
                outputs: {
                    let mut v = vec![D; WIDTH + 1];
                    v.push(5_000_000);
                    v
                },
                outputs_mine: vec![true; WIDTH + 2],
                inputs: vec![(
                    OutPoint::new(txid(0xee), 0),
                    Some((WIDTH as u64 + 1) * D + 5_000_000),
                )],
            },
        );
        txs.insert(
            txid(1),
            WalletTx {
                outputs: vec![D; WIDTH + 1],
                outputs_mine: vec![true; WIDTH + 1],
                inputs: (0..=WIDTH as u32)
                    .map(|vout| (OutPoint::new(txid(0), vout), Some(D)))
                    .collect(),
            },
        );
        txs.insert(
            txid(2),
            WalletTx {
                outputs: vec![D; WIDTH + 1],
                outputs_mine: vec![true; WIDTH + 1],
                inputs: (0..=WIDTH as u32)
                    .map(|vout| (OutPoint::new(txid(1), vout), Some(D)))
                    .collect(),
            },
        );
        let sibling = OutPoint::new(txid(2), WIDTH as u32);
        for i in 3..=25u8 {
            txs.insert(
                txid(i),
                WalletTx {
                    outputs: vec![D; WIDTH],
                    outputs_mine: vec![true; WIDTH],
                    inputs: (0..WIDTH as u32)
                        .map(|vout| (OutPoint::new(txid(i - 1), vout), Some(D)))
                        .collect(),
                },
            );
        }
        let tip = OutPoint::new(txid(25), 0);
        for deep_first in [true, false] {
            let mut calc = RoundsCalculator::new(&txs);
            if deep_first {
                assert_eq!(calc.rounds(&tip), ROUNDS_MAX);
                assert_eq!(calc.rounds(&sibling), 2);
            } else {
                assert_eq!(calc.rounds(&sibling), 2);
                assert_eq!(calc.rounds(&tip), ROUNDS_MAX);
            }
        }
    }

    #[test]
    fn rebalance_outputs_restart_at_zero() {
        let mut txs = HashMap::new();
        txs.insert(
            txid(1),
            WalletTx {
                outputs: vec![DENOMINATIONS[3]],
                outputs_mine: vec![true],
                inputs: vec![],
            },
        );
        // Our input is a smaller denomination than the output (promotion).
        txs.insert(
            txid(2),
            WalletTx {
                outputs: vec![D],
                outputs_mine: vec![true],
                inputs: vec![(OutPoint::new(txid(1), 0), Some(D))],
            },
        );
        let mut calc = RoundsCalculator::new(&txs);
        assert_eq!(calc.rounds(&OutPoint::new(txid(2), 0)), 0);
    }

    #[test]
    fn test_qt_043_fully_mixed_rule_uses_the_salt() {
        let salt = [0x42; 32];
        let op = OutPoint::new(txid(7), 1);
        assert!(!is_fully_mixed(3, 4, &op, &salt));
        assert!(is_fully_mixed(7, 4, &op, &salt));
        let odd = salted_hash_is_odd(&op, &salt);
        assert_eq!(is_fully_mixed(4, 4, &op, &salt), odd);
        // Across many outpoints roughly half stop at the target.
        let odd_count = (0..200u32)
            .filter(|n| salted_hash_is_odd(&OutPoint::new(txid(1), *n), &salt))
            .count();
        assert!((60..140).contains(&odd_count), "{odd_count}");
    }

    #[test]
    fn salt_hex_is_display_order() {
        let mut internal = [0u8; 32];
        internal[0] = 0xab;
        let hex = salt_to_display_hex(&internal);
        assert!(hex.ends_with("ab"));
        assert_eq!(salt_from_display_hex(&hex), Some(internal));
        assert_eq!(salt_from_display_hex("zz"), None);
    }
}
