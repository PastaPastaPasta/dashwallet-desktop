//! The input-0 planner (C-routes C1-4, §0 "Fee on input 0"): the inputs of
//! an address transition whose fee Platform deducts from input 0
//! (`DeductFromInput(0)`), the first input in `PlatformAddress` order. A
//! fee reserve stays unclaimed on input 0 for it. Used by the shield from
//! Platform; the top-up from Platform (C3a) passes its own reserve and
//! bounds. Not part of the facade.
//!
//! It mirrors platform-wallet's shield planner (`plan_shield_inputs` and
//! `select_inputs`, `platform_wallet.rs:177` at the pin): input 0 is the
//! first funded address whose balance is above the reserve, earlier ones
//! are left out; later addresses below the input minimum are left out; at
//! most `max_inputs` are taken. Max takes the usable balance less the
//! reserve. An amount is claimed from input 0 first, then in address order;
//! a later input's last claim below the input minimum claims the minimum
//! (Drive leaves the excess on the address), or, for a route whose claims
//! are what it pays ([`Input0Rules::exact`], the top-up), borrows the
//! shortfall from the input before.

use std::collections::BTreeMap;

use dpp::address_funds::PlatformAddress;

use super::credits::WithdrawAmount;

/// What one route's transition requires of its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Input0Rules {
    /// What input 0 keeps unclaimed for the fee.
    pub(crate) reserve: u64,
    /// The least a later input holds (dpp `min_input_amount`).
    pub(crate) min_input: u64,
    /// The most inputs one transition has.
    pub(crate) max_inputs: usize,
    /// The least input 0 may claim.
    pub(crate) min_claim_0: u64,
    /// The least the claims may total.
    pub(crate) min_total: u64,
    /// The claims total exactly the amount: a later input's short last
    /// claim borrows from the one before rather than claiming more.
    pub(crate) exact: bool,
}

/// The inputs: each address's claim, input 0 first in map order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Input0Plan {
    pub(crate) inputs: BTreeMap<PlatformAddress, u64>,
    /// What the claims were planned to give (the amount, or Max).
    pub(crate) credits: u64,
}

impl Input0Plan {
    /// The fee-paying input.
    pub(crate) fn input_0(&self) -> (PlatformAddress, u64) {
        let (a, c) = self.inputs.first_key_value().expect("a plan has an input");
        (*a, *c)
    }
}

/// Why no plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Input0Refused {
    /// The usable addresses hold `available`, less than the amount and the
    /// reserve (`needed`); with no address above the reserve, `available`
    /// is the largest funded address's balance, as input 0 is one address.
    Short { needed: u64, available: u64 },
    /// Input 0 would claim less than [`Input0Rules::min_claim_0`].
    ClaimBelow { min: u64 },
    /// The claims would total less than [`Input0Rules::min_total`].
    TotalBelow { min: u64 },
}

/// The plan for `amount` over the proved `balances`.
pub(crate) fn plan(
    balances: &BTreeMap<PlatformAddress, u64>,
    amount: WithdrawAmount,
    rules: &Input0Rules,
) -> Result<Input0Plan, Input0Refused> {
    let funded: Vec<(PlatformAddress, u64)> = (balances.iter())
        .filter(|(_, c)| **c > 0)
        .map(|(a, c)| (*a, *c))
        .collect();
    let usable: Vec<(PlatformAddress, u64)> =
        match funded.iter().position(|(_, c)| *c > rules.reserve) {
            Some(first) => std::iter::once(funded[first])
                .chain(
                    funded[first + 1..]
                        .iter()
                        .copied()
                        .filter(|(_, c)| *c >= rules.min_input),
                )
                .take(rules.max_inputs)
                .collect(),
            None => Vec::new(),
        };
    let held = total(usable.iter().map(|(_, c)| *c));
    let max = held.saturating_sub(rules.reserve);
    let credits = match amount {
        WithdrawAmount::All => max,
        WithdrawAmount::Credits { credits: 0 } => {
            return Err(Input0Refused::TotalBelow {
                min: rules.min_total.max(1),
            });
        }
        WithdrawAmount::Credits { credits } => credits,
    };
    if credits == 0 || credits > max {
        let available = if usable.is_empty() {
            funded.iter().map(|(_, c)| *c).max().unwrap_or(0)
        } else {
            held
        };
        return Err(Input0Refused::Short {
            needed: credits.max(1).saturating_add(rules.reserve),
            available,
        });
    }
    let mut inputs = BTreeMap::new();
    let mut left = credits;
    for (i, &(address, balance)) in usable.iter().enumerate() {
        if left == 0 {
            break;
        }
        let room = if i == 0 {
            balance - rules.reserve
        } else {
            balance
        };
        let claim = room.min(left);
        left -= claim;
        let claim = if i == 0 || rules.exact {
            claim
        } else {
            claim.max(rules.min_input)
        };
        inputs.insert(address, claim);
    }
    if rules.exact {
        borrow(&mut inputs, rules)?;
    }
    let plan = Input0Plan { inputs, credits };
    if plan.input_0().1 < rules.min_claim_0 {
        return Err(Input0Refused::ClaimBelow {
            min: rules.min_claim_0,
        });
    }
    if total(plan.inputs.values().copied()) < rules.min_total {
        return Err(Input0Refused::TotalBelow {
            min: rules.min_total,
        });
    }
    Ok(plan)
}

/// An exact plan's last claim below the input minimum takes the shortfall
/// from the claim before, which keeps its own minimum.
fn borrow(
    inputs: &mut BTreeMap<PlatformAddress, u64>,
    rules: &Input0Rules,
) -> Result<(), Input0Refused> {
    let claims: Vec<PlatformAddress> = inputs.keys().copied().collect();
    let [.., before, last] = claims.as_slice() else {
        return Ok(());
    };
    let short = rules.min_input.saturating_sub(inputs[last]);
    if short == 0 {
        return Ok(());
    }
    let floor = if Some(before) == claims.first() {
        rules.min_claim_0
    } else {
        rules.min_input
    };
    if inputs[before] < floor.saturating_add(short) {
        return Err(Input0Refused::ClaimBelow {
            min: rules.min_input,
        });
    }
    *inputs.get_mut(before).expect("planned") -= short;
    inputs.insert(*last, rules.min_input);
    Ok(())
}

fn total(credits: impl Iterator<Item = u64>) -> u64 {
    credits.fold(0, u64::saturating_add)
}

#[cfg(test)]
#[path = "input0_tests.rs"]
mod tests;
