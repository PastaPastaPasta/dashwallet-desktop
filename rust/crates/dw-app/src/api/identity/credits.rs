//! Credits: the cost table, top-up and withdraw, each with a quote.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::flows::GrantRequest;

/// Credit costs of the DashPay actions, for "≈ N contact requests" and the
/// low-credit warnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CostTable {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub contact_request: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub profile_update: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub contact_info: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub enable_dashpay_keys: u64,
    /// Credits one duff buys.
    pub credits_per_duff: u64,
    pub top_up_min_duffs: u64,
    /// Below this balance the UI warns.
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub low_credits: u64,
}

impl From<engine::CostTable> for CostTable {
    fn from(c: engine::CostTable) -> Self {
        let engine::CostTable {
            contact_request,
            profile_update,
            contact_info,
            enable_dashpay_keys,
            credits_per_duff,
            top_up_min_duffs,
            low_credits,
        } = c;
        CostTable {
            contact_request,
            profile_update,
            contact_info,
            enable_dashpay_keys,
            credits_per_duff,
            top_up_min_duffs,
            low_credits,
        }
    }
}

/// What a top-up of some duffs costs and buys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TopUpQuote {
    pub fee_duffs: u64,
    /// The duffs plus `feeDuffs`: what the grant must cover.
    pub total_duffs: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub grant: GrantRequest,
}

impl From<engine::TopUpQuote> for TopUpQuote {
    fn from(q: engine::TopUpQuote) -> Self {
        let engine::TopUpQuote {
            fee_duffs,
            total_duffs,
            credits,
            grant,
        } = q;
        TopUpQuote {
            fee_duffs,
            total_duffs,
            credits,
            grant: grant.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TopUpOutcome {
    pub txid: String,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub credits_added: Option<u64>,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub balance: Option<u64>,
}

impl From<engine::TopUpOutcome> for TopUpOutcome {
    fn from(o: engine::TopUpOutcome) -> Self {
        let engine::TopUpOutcome {
            txid,
            credits_added,
            balance,
        } = o;
        TopUpOutcome {
            txid,
            credits_added,
            balance,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WithdrawAmount {
    /// The balance less the fee reserve.
    All,
    Credits {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        credits: u64,
    },
}

impl From<WithdrawAmount> for engine::WithdrawAmount {
    fn from(a: WithdrawAmount) -> Self {
        match a {
            WithdrawAmount::All => engine::WithdrawAmount::All,
            WithdrawAmount::Credits { credits } => engine::WithdrawAmount::Credits { credits },
        }
    }
}

/// What a withdrawal takes and pays out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawQuote {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub fee_credits: u64,
    pub expected_duffs: u64,
    pub grant: GrantRequest,
}

impl From<engine::WithdrawQuote> for WithdrawQuote {
    fn from(q: engine::WithdrawQuote) -> Self {
        let engine::WithdrawQuote {
            credits,
            fee_credits,
            expected_duffs,
            grant,
        } = q;
        WithdrawQuote {
            credits,
            fee_credits,
            expected_duffs,
            grant: grant.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WithdrawOutcome {
    #[ts(type = "string")]
    #[serde(with = "crate::dto::credits")]
    pub credits: u64,
    pub expected_duffs: Option<u64>,
    #[ts(type = "string | null")]
    #[serde(with = "crate::dto::credits::option")]
    pub remaining_credits: Option<u64>,
}

impl From<engine::WithdrawOutcome> for WithdrawOutcome {
    fn from(o: engine::WithdrawOutcome) -> Self {
        let engine::WithdrawOutcome {
            credits,
            expected_duffs,
            remaining_credits,
        } = o;
        WithdrawOutcome {
            credits,
            expected_duffs,
            remaining_credits,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn withdraw_amounts_parse_credits_from_strings() {
        let a: WithdrawAmount =
            serde_json::from_str(r#"{"kind":"credits","credits":"100000000000"}"#).unwrap();
        assert_eq!(
            engine::WithdrawAmount::from(a),
            engine::WithdrawAmount::Credits {
                credits: 100_000_000_000
            }
        );
        let a: WithdrawAmount = serde_json::from_str(r#"{"kind":"all"}"#).unwrap();
        assert_eq!(engine::WithdrawAmount::from(a), engine::WithdrawAmount::All);
        assert!(serde_json::from_str::<WithdrawAmount>(r#"{"kind":"credits","credits":5}"#).is_err());
    }

    #[test]
    fn quotes_mix_duffs_numbers_and_credit_strings() {
        let q = WithdrawQuote::from(engine::WithdrawQuote {
            credits: 5_000_000_000_000_000_000,
            fee_credits: 3,
            expected_duffs: 49,
            grant: engine::GrantRequest {
                max_duffs: 0,
                max_credits: 9,
            },
        });
        assert_eq!(
            serde_json::to_value(q).unwrap(),
            serde_json::json!({
                "credits": "5000000000000000000",
                "feeCredits": "3",
                "expectedDuffs": 49,
                "grant": {"maxDuffs": 0, "maxCredits": "9"},
            })
        );
    }
}
