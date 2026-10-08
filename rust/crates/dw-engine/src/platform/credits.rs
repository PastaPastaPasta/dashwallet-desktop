//! Credits: the cost table, top-up and withdraw, each with a quote
//! (DP1-06, DP6-02).

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::CreditsError;

/// Credit costs of the DashPay actions, for "≈ N contact requests" and the
/// low-credit warnings (F8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostTable {
    pub contact_request: u64,
    pub profile_update: u64,
    pub contact_info: u64,
    pub enable_dashpay_keys: u64,
    pub credits_per_duff: u64,
    pub top_up_min_duffs: u64,
    /// Below this balance the UI warns.
    pub low_credits: u64,
}

/// What a top-up of `duffs` costs and buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpQuote {
    pub fee_duffs: u64,
    /// `duffs + fee_duffs`: what the grant must cover.
    pub total_duffs: u64,
    pub credits: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopUpOutcome {
    pub txid: String,
    pub credits_added: Option<u64>,
    pub balance: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WithdrawAmount {
    /// The balance less the fee reserve.
    All,
    Credits {
        credits: u64,
    },
}

/// What a withdrawal of `amount` takes and pays out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawQuote {
    pub credits: u64,
    pub fee_credits: u64,
    pub expected_duffs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawOutcome {
    pub credits: u64,
    pub expected_duffs: Option<u64>,
    pub remaining_credits: Option<u64>,
}

#[expect(unused_variables, reason = "stubs until DP1-06 and DP6-02")]
impl DashPay {
    pub fn cost_table(&self) -> Result<CostTable, CreditsError> {
        stub("DashPay.cost_table")
    }

    pub async fn top_up_quote(
        &self,
        identity: String,
        duffs: u64,
    ) -> Result<TopUpQuote, CreditsError> {
        stub("DashPay.top_up_quote")
    }

    pub async fn top_up(
        &self,
        identity: String,
        duffs: u64,
        grant: String,
    ) -> Result<TopUpOutcome, CreditsError> {
        stub("DashPay.top_up")
    }

    pub async fn withdraw_quote(
        &self,
        identity: String,
        amount: WithdrawAmount,
    ) -> Result<WithdrawQuote, CreditsError> {
        stub("DashPay.withdraw_quote")
    }

    pub async fn withdraw(
        &self,
        identity: String,
        to: String,
        amount: WithdrawAmount,
        grant: String,
    ) -> Result<WithdrawOutcome, CreditsError> {
        stub("DashPay.withdraw")
    }
}
