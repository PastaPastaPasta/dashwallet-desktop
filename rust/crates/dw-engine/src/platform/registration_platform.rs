//! A registration paid from the wallet's Platform addresses (C-funding
//! F8, F9): the inputs an identity create from addresses signs at, input 0
//! keeping the fee reserve unclaimed (C-routes §0 "Fee on input 0"), and
//! the identity those inputs and nonces give. Not part of the facade.

use std::collections::BTreeMap;

use dpp::address_funds::PlatformAddress;
use dpp::version::PlatformVersion;
use platform_wallet::PlatformWallet;

use super::credits::WithdrawAmount;
use super::errors::{CreditsError, PlatformError};
use super::input0::{self, Input0Plan, Input0Refused, Input0Rules};
use super::platform_moves::{self, MoveNet};
use crate::{NetworkSession, WalletId};

/// What input 0 keeps for the create's fee, as a multiple of dpp's minimum
/// (the top-up's band, C-routes C3-3).
pub(crate) const REGISTER_FEE_BAND: u64 = 3;

/// A planned registration from the Platform addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegisterPlan {
    /// Each input's nonce to sign at (Platform's + 1) and its claim, input 0
    /// first.
    pub(crate) inputs: BTreeMap<PlatformAddress, (u32, u64)>,
    /// What the identity receives: the claims' sum, with no change output.
    pub(crate) funding_credits: u64,
    /// dpp's minimum fee for these inputs and keys, shown as "about".
    pub(crate) fee_credits: u64,
    /// What input 0 keeps unclaimed for the fee.
    pub(crate) fee_reserve_credits: u64,
}

impl RegisterPlan {
    /// What the addresses may lose: the grant's cap, not what the copy shows.
    pub(crate) fn debit_credits(&self) -> u64 {
        self.funding_credits
            .saturating_add(self.fee_reserve_credits)
    }
}

/// dpp's `calculate_min_required_fee` of an identity create from `inputs`
/// addresses creating `keys` keys, with no output.
pub(crate) fn register_fee(version: &PlatformVersion, inputs: usize, keys: usize) -> u64 {
    let fees = &version.fee_version.state_transition_min_fees;
    (fees.identity_create_base_cost)
        .saturating_add(
            fees.address_funds_transfer_input_cost
                .saturating_mul(inputs as u64),
        )
        .saturating_add(
            fees.identity_key_in_creation_cost
                .saturating_mul(keys as u64),
        )
}

/// The least an identity created from addresses receives.
pub(crate) fn min_funding(version: &PlatformVersion) -> u64 {
    (version.dpp.state_transitions.address_funds).min_identity_funding_amount
}

/// The input-0 plan of an identity receiving `funding` over the proved
/// `balances`; with its fee estimate and the reserve input 0 keeps, the
/// band of the fee for as many inputs as the plan takes. Refused, before
/// anything is written, when no plan meets dpp's input and funding minimums.
pub(crate) fn register_inputs(
    balances: &BTreeMap<PlatformAddress, u64>,
    funding: u64,
    keys: usize,
    version: &PlatformVersion,
) -> Result<(Input0Plan, u64, u64), PlatformError> {
    let funds = &version.dpp.state_transitions;
    let min_input = funds.address_funds.min_input_amount;
    let mut inputs = 1;
    loop {
        let reserve = register_fee(version, inputs, keys).saturating_mul(REGISTER_FEE_BAND);
        let rules = Input0Rules {
            reserve,
            min_input,
            max_inputs: funds.max_address_inputs as usize,
            min_claim_0: min_input,
            min_total: min_funding(version),
        };
        let amount = WithdrawAmount::Credits { credits: funding };
        let planned = input0::plan(balances, amount, &rules).map_err(|r| match r {
            Input0Refused::Short { needed, available } => {
                PlatformError::InsufficientCredits { needed, available }
            }
            Input0Refused::ClaimBelow { min } | Input0Refused::TotalBelow { min } => {
                PlatformError::InsufficientCredits {
                    needed: min.saturating_add(reserve),
                    available: balances.values().copied().fold(0, u64::saturating_add),
                }
            }
        })?;
        let taken = planned.inputs.len();
        // A reserve for more inputs than the plan took still covers its fee.
        if taken <= inputs {
            return Ok((planned, register_fee(version, taken, keys), reserve));
        }
        inputs = taken;
    }
}

/// The plan of a registration whose identity receives at least `funding`
/// and creates `keys` keys, over the proved `balances` and stored `nonces`.
pub(crate) fn register_plan(
    (balances, nonces): (
        &BTreeMap<PlatformAddress, u64>,
        &BTreeMap<PlatformAddress, u32>,
    ),
    funding: u64,
    keys: usize,
    version: &PlatformVersion,
) -> Result<RegisterPlan, PlatformError> {
    let funding = funding.max(min_funding(version));
    let (planned, fee_credits, fee_reserve_credits) =
        register_inputs(balances, funding, keys, version)?;
    let mut inputs = BTreeMap::new();
    for (address, credits) in planned.inputs {
        let nonce = nonces.get(&address).ok_or(PlatformError::Unavailable)?;
        inputs.insert(address, (nonce.saturating_add(1), credits));
    }
    let funding_credits = inputs
        .values()
        .map(|(_, c)| *c)
        .fold(0, u64::saturating_add);
    Ok(RegisterPlan {
        inputs,
        funding_credits,
        fee_credits,
        fee_reserve_credits,
    })
}

/// [`register_plan`] from one proved read of the addresses under rule 5
/// (C-funding F9). Refused while any move of the wallet may still execute
/// (`credits.move_pending`): a registration signs no other record's terms.
pub(crate) async fn register_quote(
    net: &impl MoveNet,
    session: &NetworkSession,
    wallet: &PlatformWallet,
    wallet_id: WalletId,
    (funding, keys): (u64, usize),
) -> Result<RegisterPlan, CreditsError> {
    let version = net.sends_version().await;
    platform_moves::sends_active(version)?;
    let read = platform_moves::proved_read(net, session, wallet, wallet_id).await?;
    if let Some(id) = read.held {
        return Err(CreditsError::MovePending { id });
    }
    Ok(register_plan(
        (&read.balances, &read.nonces),
        funding,
        keys,
        version,
    )?)
}

#[cfg(test)]
#[path = "registration_platform_tests.rs"]
mod tests;
