//! Coin selection and fee planning for a payment, as pure functions.
//!
//! The plan fixes the input set, every output amount, the change and the fee
//! before anything is reserved or signed. `prepare` then hands key-wallet's
//! `TransactionBuilder` exactly that input set plus the change as an explicit
//! output, which makes the builder reproduce the plan instead of re-deciding
//! it; the caller checks the built transaction
//! against the plan.
//!
//! Sizes follow key-wallet's builder estimate (`calculate_base_size`):
//! 8 bytes of version/type/locktime, 1 byte of input count, the output-count
//! varint, every output (`8 + varint(len) + script`), 148 bytes per P2PKH
//! input, and a budgeted change output, which the builder always counts
//! because `prepare` always sets a change address.
//!
//! Policy (dash-qt / Dash Core `CreateTransaction`):
//! - automatic selection uses key-wallet's branch-and-bound selector over the
//!   candidate set; coin control (`UseAll`) spends every chosen coin;
//! - change is kept only when it is above the dust threshold of its script
//!   (`3 × (output size + 148)`, 546 duffs for P2PKH); otherwise it goes to
//!   the fee;
//! - with "subtract fee from amount" the recipients that ticked it pay the
//!   size-based fee, split equally, the first of them paying the remainder;
//!   a dust remainder that cannot become change goes to the fee on top and is
//!   paid by the wallet;
//! - selection for a subtract-fee payment targets the amounts alone, because
//!   the fee comes out of them.

use dashcore::ScriptBuf;
use key_wallet::Utxo;
use key_wallet::wallet::managed_wallet_info::coin_selection::{
    CoinSelector, SelectionError, SelectionStrategy,
};
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;

/// Bytes key-wallet budgets for one signed P2PKH input.
pub(crate) const INPUT_SIZE: usize = 148;
/// key-wallet refuses transactions with more inputs (relay size cap).
pub(crate) const MAX_INPUTS: usize = 500;
/// Dash Core `MAX_MONEY`: 21 million DASH in duffs.
pub(crate) const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

/// One recipient output of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanOutput {
    pub script: ScriptBuf,
    pub amount: u64,
    pub subtract_fee: bool,
}

/// Which inputs a plan may use.
#[derive(Debug, Clone, Copy)]
pub(crate) enum InputChoice<'a> {
    /// Choose from these candidates (automatic coin selection).
    Select(&'a [Utxo]),
    /// Spend exactly these coins (coin control).
    UseAll(&'a [Utxo]),
}

/// A complete payment: inputs, outputs, change and fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Plan {
    /// The coins to spend, in selection order.
    pub inputs: Vec<Utxo>,
    /// Recipient amounts in recipient order, after any fee subtraction.
    pub amounts: Vec<u64>,
    /// Change output value; `None` when the remainder went to the fee.
    pub change: Option<u64>,
    /// What the transaction pays: inputs − outputs.
    pub fee: u64,
    /// The builder's size estimate of the transaction, in bytes.
    pub estimated_size: usize,
}

impl Plan {
    pub fn total_in(&self) -> u64 {
        self.inputs.iter().map(Utxo::value).sum()
    }

    pub fn total_sent(&self) -> u64 {
        self.amounts.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum PlanError {
    /// The recipients alone need more than the candidates hold.
    #[error("amount exceeds the available {available}")]
    AmountExceedsBalance { available: u64 },
    /// The recipients fit, but not with the fee.
    #[error("amount with fee {fee} exceeds the available {available}")]
    AmountWithFeeExceedsBalance { fee: u64, available: u64 },
    /// A subtract-fee recipient would be left with dust or nothing.
    #[error("recipient {index} is too small to pay its share of the fee")]
    AmountTooSmallAfterFee { index: usize },
    /// More inputs than one standard transaction may carry.
    #[error("{count} inputs exceed the limit of {MAX_INPUTS}")]
    TooManyInputs { count: usize },
    /// An amount sum or a fee does not fit in 64 bits.
    #[error("amount or fee overflows")]
    Overflow,
}

/// Bitcoin compact-size length of `n`.
pub(crate) fn varint_size(n: usize) -> usize {
    match n {
        0..=0xFC => 1,
        0xFD..=0xFFFF => 3,
        0x1_0000..=0xFFFF_FFFF => 5,
        _ => 9,
    }
}

/// Serialized size of an output paying a script of `script_len` bytes.
pub(crate) fn output_size(script_len: usize) -> usize {
    8 + varint_size(script_len) + script_len
}

/// Dash Core's dust threshold for an output paying a script of
/// `script_len` bytes at the 3000 duff/kB dust relay fee.
pub(crate) fn dust_threshold(script_len: usize) -> u64 {
    3 * (output_size(script_len) as u64 + INPUT_SIZE as u64)
}

/// `rate × size`, rounded up, in duffs; `None` on overflow.
pub(crate) fn fee_for(rate: FeeRate, size: usize) -> Option<u64> {
    rate.as_sat_per_kb()
        .checked_mul(size as u64)
        .map(|v| v.div_ceil(1000))
}

/// Transaction size without inputs and without the change budget, for
/// `outputs` explicit outputs (script lengths). The output-count varint
/// still counts the budgeted change output, as the builder's does.
fn base_without_budget(outputs: &[usize]) -> usize {
    8 + 1 + varint_size(outputs.len() + 1) + outputs.iter().map(|l| output_size(*l)).sum::<usize>()
}

/// What [`plan`] computes for a fixed input set: the shapes with and
/// without a change output.
struct Shapes {
    /// Size with the change output as an explicit output.
    with_change: usize,
    /// Size with no change output.
    without_change: usize,
}

fn shapes(outputs: &[usize], change_len: usize, inputs: usize) -> Shapes {
    let mut with = outputs.to_vec();
    with.push(change_len);
    Shapes {
        with_change: base_without_budget(&with) + INPUT_SIZE * inputs,
        without_change: base_without_budget(outputs) + INPUT_SIZE * inputs,
    }
}

/// Plans a payment of `outputs` from `choice` at `rate`, with change to a
/// script of `change_len` bytes. `height` is the wallet's processed height
/// (coinbase maturity).
pub(crate) fn plan(
    choice: InputChoice<'_>,
    outputs: &[PlanOutput],
    rate: FeeRate,
    change_len: usize,
    height: u32,
) -> Result<Plan, PlanError> {
    let total_out = outputs
        .iter()
        .try_fold(0u64, |acc, o| acc.checked_add(o.amount))
        .ok_or(PlanError::Overflow)?;
    let subtract = outputs.iter().any(|o| o.subtract_fee);
    let lens: Vec<usize> = outputs.iter().map(|o| o.script.len()).collect();

    let inputs: Vec<Utxo> = match choice {
        InputChoice::UseAll(coins) => {
            let available = coins.iter().map(Utxo::value).sum::<u64>();
            if total_out > available {
                return Err(PlanError::AmountExceedsBalance { available });
            }
            coins.to_vec()
        }
        InputChoice::Select(candidates) => {
            let spendable: Vec<&Utxo> = candidates
                .iter()
                .filter(|u| u.is_spendable(height))
                .collect();
            let available = spendable.iter().map(|u| u.value()).sum::<u64>();
            if total_out > available {
                return Err(PlanError::AmountExceedsBalance { available });
            }
            // A subtract-fee payment needs only the amounts: the fee comes
            // out of them.
            let selection_rate = if subtract { FeeRate::new(0) } else { rate };
            let base = base_without_budget(&lens) + change_len;
            let selected = CoinSelector::new(SelectionStrategy::BranchAndBound)
                .select_coins_with_size(
                    spendable.iter().copied(),
                    total_out,
                    selection_rate,
                    height,
                    base,
                    change_len,
                );
            match selected {
                Ok(selection) => selection.selected,
                Err(SelectionError::InsufficientFunds { .. })
                | Err(SelectionError::NoUtxosAvailable) => {
                    let all = shapes(&lens, change_len, spendable.len());
                    let fee = fee_for(rate, all.without_change).ok_or(PlanError::Overflow)?;
                    return Err(PlanError::AmountWithFeeExceedsBalance { fee, available });
                }
                Err(other) => {
                    // The remaining selector errors are not reachable for
                    // branch-and-bound; report them as a shortfall of the
                    // candidates rather than inventing a figure.
                    tracing::warn!(error = %other, "coin selection failed");
                    return Err(PlanError::AmountExceedsBalance { available });
                }
            }
        }
    };
    if inputs.len() > MAX_INPUTS {
        return Err(PlanError::TooManyInputs {
            count: inputs.len(),
        });
    }
    use_all(inputs, outputs, &lens, total_out, rate, change_len)
}

/// Plans spending every coin of `inputs`.
fn use_all(
    inputs: Vec<Utxo>,
    outputs: &[PlanOutput],
    lens: &[usize],
    total_out: u64,
    rate: FeeRate,
    change_len: usize,
) -> Result<Plan, PlanError> {
    let total_in = inputs
        .iter()
        .try_fold(0u64, |acc, u| acc.checked_add(u.value()))
        .ok_or(PlanError::Overflow)?;
    let s = shapes(lens, change_len, inputs.len());
    let fee_with = fee_for(rate, s.with_change).ok_or(PlanError::Overflow)?;
    let fee_without = fee_for(rate, s.without_change).ok_or(PlanError::Overflow)?;
    let dust = dust_threshold(change_len);
    let mut amounts: Vec<u64> = outputs.iter().map(|o| o.amount).collect();

    if !outputs.iter().any(|o| o.subtract_fee) {
        if let Some(change) = total_in
            .checked_sub(total_out)
            .and_then(|r| r.checked_sub(fee_with))
            .filter(|c| *c > dust)
        {
            return Ok(Plan {
                inputs,
                amounts,
                change: Some(change),
                fee: fee_with,
                estimated_size: s.with_change,
            });
        }
        if total_in < total_out.saturating_add(fee_without) {
            return Err(PlanError::AmountWithFeeExceedsBalance {
                fee: fee_without,
                available: total_in,
            });
        }
        return Ok(Plan {
            inputs,
            amounts,
            change: None,
            fee: total_in - total_out,
            estimated_size: s.without_change,
        });
    }

    // Subtract-fee: the recipients pay the size-based fee; whatever the
    // inputs hold beyond the amounts is change, or fee when it is dust.
    let remainder = total_in
        .checked_sub(total_out)
        .ok_or(PlanError::AmountExceedsBalance {
            available: total_in,
        })?;
    let (fee_share, change, size) = if remainder > dust {
        (fee_with, Some(remainder), s.with_change)
    } else {
        (fee_without, None, s.without_change)
    };
    let payers: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, o)| o.subtract_fee)
        .map(|(i, _)| i)
        .collect();
    let count = payers.len() as u64;
    for (n, &i) in payers.iter().enumerate() {
        let mut share = fee_share / count;
        if n == 0 {
            share += fee_share % count;
        }
        let after = amounts[i]
            .checked_sub(share)
            .filter(|a| *a >= dust_threshold(lens[i]))
            .ok_or(PlanError::AmountTooSmallAfterFee { index: i })?;
        amounts[i] = after;
    }
    let fee = if change.is_some() {
        fee_share
    } else {
        fee_share + remainder
    };
    Ok(Plan {
        inputs,
        amounts,
        change,
        fee,
        estimated_size: size,
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use dashcore::address::Payload;
    use dashcore::hashes::Hash;
    use dashcore::{Address, Network, OutPoint, PubkeyHash, ScriptHash, TxOut, Txid};
    use key_wallet::wallet::managed_wallet_info::transaction_builder::TransactionBuilder;

    use super::*;

    fn p2pkh(seed: u8) -> Address {
        Address::new(
            Network::Regtest,
            Payload::PubkeyHash(PubkeyHash::from_byte_array([seed; 20])),
        )
    }

    fn coin(seed: u8, value: u64) -> Utxo {
        let address = p2pkh(seed);
        let mut u = Utxo::new(
            OutPoint::new(Txid::from_byte_array([seed; 32]), u32::from(seed)),
            TxOut {
                value,
                script_pubkey: address.script_pubkey(),
            },
            address,
            100,
            false,
        );
        u.is_confirmed = true;
        u
    }

    fn out(seed: u8, amount: u64, subtract_fee: bool) -> PlanOutput {
        PlanOutput {
            script: p2pkh(seed).script_pubkey(),
            amount,
            subtract_fee,
        }
    }

    const P2PKH: usize = 25;

    fn rate1() -> FeeRate {
        FeeRate::min()
    }

    /// Builds `plan` with key-wallet's builder exactly as `prepare` does
    /// (the plan's inputs seeded, change as an explicit output, a change
    /// address preset) and returns (input count, fee, outputs).
    fn rebuild(plan: &Plan, outputs: &[PlanOutput]) -> (usize, u64, Vec<u64>) {
        let change_address = p2pkh(0xCC);
        let mut builder = TransactionBuilder::new()
            .set_current_height(1000)
            .set_fee_rate(rate1())
            .set_selection_strategy(SelectionStrategy::LargestFirst)
            .set_change_address(change_address.clone())
            .add_inputs(plan.inputs.clone());
        for (o, amount) in outputs.iter().zip(&plan.amounts) {
            let address = Address::from_script(&o.script, Network::Regtest).unwrap();
            builder = builder.add_output(&address, *amount);
        }
        if let Some(change) = plan.change {
            builder = builder.add_output(&change_address, change);
        }
        let (tx, fee, _) = builder.build_unsigned_reserved().unwrap();
        let mut values: Vec<u64> = tx.output.iter().map(|o| o.value).collect();
        values.sort_unstable();
        (tx.input.len(), fee, values)
    }

    fn expected_outputs(plan: &Plan) -> Vec<u64> {
        let mut v: Vec<u64> = plan.amounts.iter().copied().chain(plan.change).collect();
        v.sort_unstable();
        v
    }

    fn assert_builder_reproduces(plan: &Plan, outputs: &[PlanOutput]) {
        let (inputs, fee, values) = rebuild(plan, outputs);
        assert_eq!(
            inputs,
            plan.inputs.len(),
            "builder dropped inputs: {plan:?}"
        );
        assert_eq!(fee, plan.fee, "builder fee differs: {plan:?}");
        assert_eq!(
            values,
            expected_outputs(plan),
            "builder outputs differ: {plan:?}"
        );
        assert_eq!(
            plan.total_in(),
            plan.total_sent() + plan.change.unwrap_or(0) + plan.fee
        );
    }

    #[test]
    fn sizes_match_dash_qt_estimate() {
        // dash-qt: 148·inputs + 34·(outputs+1) + 10, minus 34 without change.
        let s = shapes(&[P2PKH], P2PKH, 2);
        assert_eq!(s.with_change, 148 * 2 + 34 * 2 + 10);
        assert_eq!(s.without_change, 148 * 2 + 34 + 10);
        assert_eq!(dust_threshold(P2PKH), 546);
        assert_eq!(dust_threshold(23), 540);
        assert_eq!(fee_for(rate1(), 226), Some(226));
        assert_eq!(fee_for(FeeRate::new(1500), 225), Some(338));
        assert_eq!(fee_for(FeeRate::new(u64::MAX), 2), None);
    }

    #[test]
    fn automatic_selection_with_change_matches_the_builder() {
        let coins = [coin(1, 50_000_000), coin(2, 20_000_000), coin(3, 7_000_000)];
        let outputs = [out(9, 30_000_000, false)];
        let p = plan(InputChoice::Select(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        assert_eq!(p.inputs.len(), 1);
        assert_eq!(p.inputs[0].value(), 50_000_000);
        assert_eq!(p.fee, 226);
        assert_eq!(p.change, Some(50_000_000 - 30_000_000 - 226));
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn automatic_selection_takes_an_exact_match_without_change() {
        // 20_000_000 covers 19_999_808 + the 192-duff no-change fee exactly.
        let coins = [coin(1, 50_000_000), coin(2, 20_000_000)];
        let outputs = [out(9, 19_999_808, false)];
        let p = plan(InputChoice::Select(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        assert_eq!(p.inputs.len(), 1);
        assert_eq!(p.inputs[0].value(), 20_000_000);
        assert_eq!(p.change, None);
        assert_eq!(p.fee, 192);
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn coin_control_spends_every_chosen_coin() {
        let coins = [coin(1, 50_000_000), coin(2, 20_000_000), coin(3, 7_000_000)];
        let outputs = [out(9, 1_000_000, false), out(8, 2_000_000, false)];
        let p = plan(InputChoice::UseAll(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        assert_eq!(p.inputs.len(), 3);
        let size = 148 * 3 + 34 * 3 + 10;
        assert_eq!(p.estimated_size, size);
        assert_eq!(p.fee, size as u64);
        assert_eq!(p.change, Some(77_000_000 - 3_000_000 - size as u64));
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn dust_change_goes_to_the_fee() {
        let coins = [coin(1, 1_000_000)];
        // 1_000_000 − 999_000 − 226 = 774 > 546 keeps change; 999_400 leaves
        // 374, which is dust.
        let outputs = [out(9, 999_400, false)];
        let p = plan(InputChoice::UseAll(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        assert_eq!(p.change, None);
        assert_eq!(p.fee, 600);
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn custom_fee_rate_and_p2sh_change() {
        let coins = [coin(1, 5_000_000), coin(2, 6_000_000)];
        let outputs = [out(9, 8_000_000, false)];
        let rate = FeeRate::new(5_000);
        let p = plan(InputChoice::UseAll(&coins), &outputs, rate, 23, 1000).unwrap();
        // 148·2 + 34 + 32 + 10 = 372 bytes at 5 duff/byte.
        assert_eq!(p.estimated_size, 372);
        assert_eq!(p.fee, 1_860);
        assert_eq!(p.change, Some(11_000_000 - 8_000_000 - 1_860));
    }

    #[test]
    fn subtract_fee_splits_the_fee_and_keeps_change() {
        let coins = [coin(1, 50_000_000)];
        let outputs = [
            out(9, 10_000_001, true),
            out(8, 5_000_000, true),
            out(7, 1_000_000, false),
        ];
        let p = plan(InputChoice::Select(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        let size = 148 + 34 * 4 + 10;
        assert_eq!(p.fee, size as u64);
        // 294 / 2 = 147 each; the first payer takes the odd duff.
        let share = size as u64 / 2;
        assert_eq!(
            p.amounts,
            vec![
                10_000_001 - share - (size as u64 % 2),
                5_000_000 - share,
                1_000_000
            ]
        );
        assert_eq!(p.change, Some(50_000_000 - 16_000_001));
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn subtract_fee_can_spend_the_whole_balance() {
        let coins = [coin(1, 3_000_000), coin(2, 2_000_000), coin(3, 1_000_000)];
        let outputs = [out(9, 6_000_000, true)];
        let p = plan(InputChoice::Select(&coins), &outputs, rate1(), P2PKH, 1000).unwrap();
        assert_eq!(p.inputs.len(), 3);
        let size = (148 * 3 + 34 + 10) as u64;
        assert_eq!(p.change, None);
        assert_eq!(p.fee, size);
        assert_eq!(p.amounts, vec![6_000_000 - size]);
        assert_builder_reproduces(&p, &outputs);
    }

    #[test]
    fn subtract_fee_rejects_a_recipient_left_with_dust() {
        let coins = [coin(1, 3_000_000)];
        let outputs = [out(9, 700, true), out(8, 1_000_000, false)];
        let r = plan(InputChoice::Select(&coins), &outputs, rate1(), P2PKH, 1000);
        assert_eq!(r, Err(PlanError::AmountTooSmallAfterFee { index: 0 }));
    }

    #[test]
    fn shortfalls_name_the_available_amount_and_fee() {
        let coins = [coin(1, 1_000_000), coin(2, 500_000)];
        let r = plan(
            InputChoice::Select(&coins),
            &[out(9, 2_000_000, false)],
            rate1(),
            P2PKH,
            1000,
        );
        assert_eq!(
            r,
            Err(PlanError::AmountExceedsBalance {
                available: 1_500_000
            })
        );
        let r = plan(
            InputChoice::Select(&coins),
            &[out(9, 1_499_900, false)],
            rate1(),
            P2PKH,
            1000,
        );
        assert_eq!(
            r,
            Err(PlanError::AmountWithFeeExceedsBalance {
                fee: (148 * 2 + 34 + 10) as u64,
                available: 1_500_000
            })
        );
        let r = plan(
            InputChoice::UseAll(&coins[..1]),
            &[out(9, 999_900, false)],
            rate1(),
            P2PKH,
            1000,
        );
        assert_eq!(
            r,
            Err(PlanError::AmountWithFeeExceedsBalance {
                fee: 192,
                available: 1_000_000
            })
        );
    }

    #[test]
    fn immature_coinbase_is_not_selected() {
        let mut young = coin(1, 50_000_000);
        young.is_coinbase = true;
        young.height = 950;
        let coins = [young, coin(2, 1_000_000)];
        let r = plan(
            InputChoice::Select(&coins),
            &[out(9, 2_000_000, false)],
            rate1(),
            P2PKH,
            1000,
        );
        assert_eq!(
            r,
            Err(PlanError::AmountExceedsBalance {
                available: 1_000_000
            })
        );
    }

    #[test]
    fn p2sh_recipient_dust_threshold() {
        let script = Address::new(
            Network::Regtest,
            Payload::ScriptHash(ScriptHash::from_byte_array([7; 20])),
        )
        .script_pubkey();
        assert_eq!(script.len(), 23);
        assert_eq!(dust_threshold(script.len()), 540);
        let _ = Address::from_str("yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n").unwrap();
    }
}
