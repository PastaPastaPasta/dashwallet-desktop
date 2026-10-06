//! The amounts of the transactions mixing creates by itself: create
//! denominations, make collateral inputs, and the per-session collateral
//! transaction (Dash Core `src/coinjoin/client.cpp:2022-2412`, the size and
//! fee arithmetic of `CTransactionBuilder`, `src/coinjoin/util.cpp:120-343`).
//!
//! The planner decides amounts only. The engine owns coins, addresses and
//! signing; it builds the transaction from a [`TxPlan`].

use crate::denoms::{
    COLLATERAL_AMOUNT, DENOMINATIONS, MAX_COLLATERAL_AMOUNT, SMALLEST_DENOMINATION, is_denominated,
};
use crate::rounds::is_collateral_amount;

/// `COINJOIN_DENOM_OUTPUTS_THRESHOLD` (`src/coinjoin/options.h:43`).
pub const DENOM_OUTPUTS_THRESHOLD: usize = 500;
/// Inputs taken per address (`SelectCoinsGroupedByAddresses(…, 400)`).
pub const MAX_INPUTS_PER_ADDRESS: usize = 400;
/// Signed P2PKH input: outpoint 36, script length 1, signature push
/// 1 + 72, pubkey push 1 + 33, sequence 4 (`CalculateMaximumSignedTxSize`).
pub const P2PKH_INPUT_SIZE: usize = 148;
/// P2PKH output: value 8, script length 1, script 25.
pub const P2PKH_OUTPUT_SIZE: usize = 34;
/// Version and type (4) plus lock time (4).
const TX_OVERHEAD: usize = 8;

/// Dash Core's default discard rate (`DEFAULT_DISCARD_FEE`, 10000 duffs/kB):
/// a remainder below the dust threshold at this rate goes to the fee.
pub const DISCARD_FEE_PER_KB: u64 = 10_000;

/// Serialized size of a compact-size integer.
pub fn compact_size_len(n: usize) -> usize {
    match n {
        0..=0xfc => 1,
        0xfd..=0xffff => 3,
        0x1_0000..=0xffff_ffff => 5,
        _ => 9,
    }
}

/// `CFeeRate::GetFee`: `ceil(rate · size / 1000)`.
pub fn fee_for_size(fee_per_kb: u64, size: usize) -> u64 {
    (fee_per_kb * size as u64).div_ceil(1000)
}

/// `IsDust` of a P2PKH output at the discard rate: below the cost of
/// spending it (output + input size at that rate).
pub fn is_dust(value: u64) -> bool {
    value < fee_for_size(DISCARD_FEE_PER_KB, P2PKH_OUTPUT_SIZE + P2PKH_INPUT_SIZE)
}

/// The transaction builder's arithmetic for one tally item (coins of one
/// address): what is left after outputs and the fee for the current size.
#[derive(Debug, Clone)]
pub struct TxBudget {
    amount_initial: u64,
    inputs: usize,
    fee_per_kb: u64,
    outputs: Vec<u64>,
}

impl TxBudget {
    pub fn new(amount_initial: u64, inputs: usize, fee_per_kb: u64) -> Self {
        Self {
            amount_initial,
            inputs,
            fee_per_kb,
            outputs: Vec::new(),
        }
    }

    fn base_size(&self) -> usize {
        TX_OVERHEAD + compact_size_len(self.inputs) + self.inputs * P2PKH_INPUT_SIZE
    }

    /// `GetBytesTotal` with `extra` more outputs.
    fn size_with(&self, extra: usize) -> usize {
        let n = self.outputs.len() + extra;
        self.base_size() + compact_size_len(n) + n * P2PKH_OUTPUT_SIZE
    }

    fn used(&self) -> u64 {
        self.outputs.iter().sum()
    }

    /// `GetAmountLeft()` at the current output count.
    pub fn amount_left(&self) -> i64 {
        self.amount_initial as i64
            - self.used() as i64
            - fee_for_size(self.fee_per_kb, self.size_with(0)) as i64
    }

    /// `CouldAddOutputs`.
    pub fn could_add(&self, amounts: &[u64]) -> bool {
        let fee = fee_for_size(self.fee_per_kb, self.size_with(amounts.len()));
        self.amount_initial as i64
            - self.used() as i64
            - amounts.iter().sum::<u64>() as i64
            - fee as i64
            >= 0
    }

    /// `AddOutput`: adds when affordable.
    pub fn add(&mut self, amount: u64) -> bool {
        if self.could_add(&[amount]) {
            self.outputs.push(amount);
            true
        } else {
            false
        }
    }

    pub fn count(&self) -> usize {
        self.outputs.len()
    }

    /// The plan when committed: a remainder that is dust goes to the fee
    /// (no change output), otherwise it becomes change and pays for the
    /// change output's size (`CTransactionBuilder::Commit`).
    pub fn finish(self) -> TxPlan {
        let left = self.amount_left();
        if left < 0 {
            // Callers only add affordable outputs; keep the invariant visible.
            return TxPlan {
                fee: 0,
                change: None,
                outputs: self.outputs,
                invalid: true,
            };
        }
        let left = left as u64;
        let base_fee = fee_for_size(self.fee_per_kb, self.size_with(0));
        if is_dust(left) {
            TxPlan {
                fee: base_fee + left,
                change: None,
                outputs: self.outputs,
                invalid: false,
            }
        } else {
            let fee = fee_for_size(self.fee_per_kb, self.size_with(1));
            let change = left + base_fee - fee;
            TxPlan {
                fee,
                change: Some(change),
                outputs: self.outputs,
                invalid: false,
            }
        }
    }
}

/// Amounts of a transaction to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxPlan {
    /// Outputs to fresh CoinJoin-account addresses, in order.
    pub outputs: Vec<u64>,
    /// Change back to the funding account, if any.
    pub change: Option<u64>,
    pub fee: u64,
    /// The outputs cost more than the inputs (never returned by the
    /// planners below).
    pub invalid: bool,
}

/// Coins of one address (`CompactTallyItem`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tally {
    pub amount: u64,
    pub inputs: usize,
}

/// `CreateDenominated(nBalanceToDenominate, tallyItem, fCreateMixingCollaterals)`
/// (client.cpp:2227-2412). `denom_counts[i]` is how many coins of
/// `DENOMINATIONS[i]` the wallet holds. `None` when no useful transaction
/// can be made from this tally item.
pub fn plan_denominations(
    tally: &Tally,
    mut balance_to_denominate: i64,
    create_collateral: bool,
    mut denom_counts: [u32; 5],
    goal: u32,
    hard_cap: u32,
    fee_per_kb: u64,
) -> Option<TxPlan> {
    if tally.inputs == 1 && is_denominated(tally.amount) {
        return None;
    }
    let mut b = TxBudget::new(tally.amount, tally.inputs, fee_per_kb);
    if create_collateral && !b.add(MAX_COLLATERAL_AMOUNT) {
        return None;
    }

    let mut add_final = true;
    while b.could_add(&[SMALLEST_DENOMINATION]) && b.count() < DENOM_OUTPUTS_THRESHOLD {
        // Smallest denomination first (`denoms.rbegin()`).
        for i in (0..DENOMINATIONS.len()).rev() {
            let value = DENOMINATIONS[i];
            let mut outputs = 0;
            loop {
                let need_more = b.could_add(&[value])
                    && if add_final
                        && balance_to_denominate > 0
                        && balance_to_denominate < value as i64
                    {
                        add_final = false;
                        true
                    } else {
                        balance_to_denominate >= value as i64
                    };
                if !(need_more && outputs <= 10 && denom_counts[i] < goal) {
                    break;
                }
                if !b.add(value) {
                    return None;
                }
                outputs += 1;
                denom_counts[i] += 1;
                balance_to_denominate -= value as i64;
            }
            if b.amount_left() == 0 || balance_to_denominate <= 0 {
                break;
            }
        }
        let finished = !DENOMINATIONS.iter().enumerate().any(|(i, value)| {
            denom_counts[i] < goal && b.could_add(&[*value]) && balance_to_denominate > 0
        });
        if finished {
            break;
        }
    }

    // The remainder: largest denominations first, at most the hard cap of
    // each smaller one, overshooting by one coin (client.cpp:2327-2387).
    if b.could_add(&[SMALLEST_DENOMINATION])
        && balance_to_denominate >= SMALLEST_DENOMINATION as i64
        && b.count() < DENOM_OUTPUTS_THRESHOLD
    {
        let largest = DENOMINATIONS[0];
        for (i, &value) in DENOMINATIONS.iter().enumerate() {
            if balance_to_denominate <= 0 {
                break;
            }
            let possible = {
                let mut v = Vec::new();
                loop {
                    v.push(value);
                    if !b.could_add(&v) || b.count() + v.len() > DENOM_OUTPUTS_THRESHOLD {
                        v.pop();
                        break;
                    }
                }
                v.len() as i64
            };
            let by_balance = balance_to_denominate / value as i64 + 1;
            let to_create = possible.min(by_balance);
            for _ in 0..to_create {
                if value != largest && denom_counts[i] >= hard_cap {
                    break;
                }
                if !b.add(value) {
                    break;
                }
                denom_counts[i] += 1;
                balance_to_denominate -= value as i64;
                if b.count() >= DENOM_OUTPUTS_THRESHOLD {
                    break;
                }
            }
            if b.count() >= DENOM_OUTPUTS_THRESHOLD {
                break;
            }
        }
    }

    if (create_collateral && b.count() == 1) || b.count() == 0 {
        return None;
    }
    Some(b.finish())
}

/// `MakeCollateralAmounts(tallyItem, fTryDenominated)` (client.cpp:2063-2147):
/// one max collateral plus the remainder, two equal collaterals, or one.
/// The remainder output of the first case is the last output; a remainder
/// that would be a denomination gives one duff to the fee.
pub fn plan_collaterals(tally: &Tally, try_denominated: bool, fee_per_kb: u64) -> Option<TxPlan> {
    if !try_denominated && tally.inputs == 1 && is_denominated(tally.amount) {
        return None;
    }
    if tally.inputs == 1 && is_collateral_amount(tally.amount) {
        return None;
    }
    let mut b = TxBudget::new(tally.amount, tally.inputs, fee_per_kb);
    if !b.could_add(&[COLLATERAL_AMOUNT]) {
        return None;
    }
    if b.could_add(&[MAX_COLLATERAL_AMOUNT, COLLATERAL_AMOUNT]) {
        b.add(MAX_COLLATERAL_AMOUNT);
        // A zero output reserves the size; then it takes what is left.
        b.outputs.push(0);
        let left = b.amount_left().max(0) as u64;
        let value = if is_denominated(left) { left - 1 } else { left };
        *b.outputs.last_mut().expect("just pushed") = value;
    } else if b.could_add(&[COLLATERAL_AMOUNT, COLLATERAL_AMOUNT]) {
        b.outputs.push(0);
        b.outputs.push(0);
        let each = b.amount_left().max(0) as u64 / 2;
        if !is_collateral_amount(each) {
            return None;
        }
        b.outputs[0] = each;
        b.outputs[1] = each;
    } else {
        b.outputs.push(0);
        let left = b.amount_left().max(0) as u64;
        if !is_collateral_amount(left) {
            return None;
        }
        b.outputs[0] = left;
    }
    let plan = b.finish();
    // The builder spent everything: no change (Core asserts the remainder
    // is dust here).
    (plan.change.is_none() && !plan.invalid).then_some(plan)
}

/// What the per-session collateral transaction pays
/// (`CreateCollateralTransaction`, client.cpp:2149-2191): one collateral
/// coin in; `Some(change)` back to the wallet when the coin holds at least
/// two collaterals, else a single `OP_RETURN` output of 0 and everything as
/// fee.
pub fn collateral_change(coin_value: u64) -> Option<u64> {
    (coin_value >= COLLATERAL_AMOUNT * 2).then(|| coin_value - COLLATERAL_AMOUNT)
}

/// Smallest wallet balance worth denominating
/// (`DoAutomaticDenominating`, client.cpp:1035-1041): the smallest
/// denomination, plus the max collateral while there are no collateral
/// inputs.
pub fn min_value_to_mix(has_collateral_inputs: bool) -> u64 {
    SMALLEST_DENOMINATION
        + if has_collateral_inputs {
            0
        } else {
            MAX_COLLATERAL_AMOUNT
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEE: u64 = 1_000;

    #[test]
    fn budget_matches_core_size_arithmetic() {
        // 1 input, 2 outputs: 8 + 1 + 148 + 1 + 68 = 226 bytes, 226 duffs.
        let mut b = TxBudget::new(1_000_000, 1, FEE);
        assert!(b.add(400_000));
        assert!(b.add(400_000));
        assert_eq!(b.amount_left(), 1_000_000 - 800_000 - 226);
        let plan = b.finish();
        // Change pays for its own 34 bytes.
        assert_eq!(plan.fee, 260);
        assert_eq!(plan.change, Some(1_000_000 - 800_000 - 260));
        assert_eq!(fee_for_size(1_000, 225), 225);
        assert_eq!(fee_for_size(1_500, 1), 2);
        assert!(is_dust(1_819) && !is_dust(1_820));
    }

    #[test]
    fn test_qt_044_denominations_from_one_dash() {
        let tally = Tally {
            amount: 100_000_000,
            inputs: 1,
        };
        let plan =
            plan_denominations(&tally, 1_000 * 100_000_000, true, [0; 5], 50, 300, FEE).unwrap();
        // The first output is the mixing collateral.
        assert_eq!(plan.outputs[0], MAX_COLLATERAL_AMOUNT);
        let denoms = &plan.outputs[1..];
        assert!(denoms.iter().all(|v| is_denominated(*v)));
        // Smallest first, at most 11 of a kind per pass.
        assert_eq!(denoms[0], SMALLEST_DENOMINATION);
        let smallest = denoms
            .iter()
            .filter(|v| **v == SMALLEST_DENOMINATION)
            .count();
        assert!(smallest <= 50);
        let total: u64 = plan.outputs.iter().sum::<u64>() + plan.fee + plan.change.unwrap_or(0);
        assert_eq!(total, 100_000_000);
        assert!(!plan.invalid);
    }

    #[test]
    fn denominations_respect_goal_and_skip_single_denominated_inputs() {
        let tally = Tally {
            amount: 100_001_000,
            inputs: 1,
        };
        assert!(plan_denominations(&tally, 10_000_000_000, false, [0; 5], 50, 300, FEE).is_none());
        let tally = Tally {
            amount: 50_000_000,
            inputs: 2,
        };
        // Every denomination is already at the goal: only the remainder
        // step runs, largest first, within the hard cap.
        let plan = plan_denominations(&tally, 30_000_000, false, [50; 5], 50, 300, FEE).unwrap();
        assert!(plan.outputs.iter().all(|v| is_denominated(*v)));
        assert!(plan.outputs.iter().sum::<u64>() >= 30_000_000);
    }

    #[test]
    fn collateral_cases_follow_core() {
        // Case 1: max collateral plus remainder.
        let p = plan_collaterals(
            &Tally {
                amount: 1_000_000,
                inputs: 1,
            },
            false,
            FEE,
        )
        .unwrap();
        assert_eq!(p.outputs[0], MAX_COLLATERAL_AMOUNT);
        assert_eq!(p.outputs.iter().sum::<u64>() + p.fee, 1_000_000);
        assert_eq!(p.change, None);
        // Case 2: two equal collaterals.
        let p = plan_collaterals(
            &Tally {
                amount: 45_000,
                inputs: 1,
            },
            false,
            FEE,
        )
        .unwrap();
        assert_eq!(p.outputs.len(), 2);
        assert_eq!(p.outputs[0], p.outputs[1]);
        assert!(is_collateral_amount(p.outputs[0]));
        // A single collateral-sized coin is already usable.
        assert!(
            plan_collaterals(
                &Tally {
                    amount: 30_000,
                    inputs: 1
                },
                false,
                FEE
            )
            .is_none()
        );
        // Too small for any collateral.
        assert!(
            plan_collaterals(
                &Tally {
                    amount: 9_000,
                    inputs: 1
                },
                false,
                FEE
            )
            .is_none()
        );
        // A denominated coin only with try_denominated.
        assert!(
            plan_collaterals(
                &Tally {
                    amount: 100_001_000,
                    inputs: 1
                },
                false,
                FEE
            )
            .is_none()
        );
        assert!(
            plan_collaterals(
                &Tally {
                    amount: 100_001_000,
                    inputs: 1
                },
                true,
                FEE
            )
            .is_some()
        );
    }

    #[test]
    fn collateral_transaction_change() {
        assert_eq!(collateral_change(40_000), Some(30_000));
        assert_eq!(collateral_change(20_000), Some(10_000));
        assert_eq!(collateral_change(19_999), None);
        assert_eq!(min_value_to_mix(false), 140_001);
        assert_eq!(min_value_to_mix(true), 100_001);
    }
}
