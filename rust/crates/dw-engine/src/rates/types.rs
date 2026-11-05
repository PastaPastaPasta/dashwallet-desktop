//! The rates facade's types (`docs/contracts/rates-engine.md` §2).

use serde::{Deserialize, Serialize};

/// Which source a price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RateSourceId {
    /// `rates.ctx.com`.
    Ctx,
    /// BitPay's fiat per BTC crossed with DashCentral's DASH/USD.
    BitPayDashCentral,
}

/// What the user chose (persisted in `<data_root>/rates.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesSettings {
    /// "Show values in local currency". Off: no request at all.
    pub enabled: bool,
    /// ISO 4217 code; `None` until one is chosen.
    pub currency: Option<String>,
    /// The disclosure was shown (onboarding's last step, or Home's card for
    /// a wallet from 0.1). No request is made before it was.
    pub disclosed: bool,
}

impl Default for RatesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            currency: None,
            disclosed: false,
        }
    }
}

/// Why no request is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatesOff {
    /// The user turned prices off.
    Disabled,
    /// The disclosure was not shown yet.
    NotDisclosed,
    /// No mainnet network is open: test coins have no price.
    NotMainnet,
}

/// Why there is no number to show although prices are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RatesUnavailable {
    /// No fetch has answered yet.
    NeverFetched,
    /// The last attempt failed and nothing usable is cached.
    Failed,
    /// The cached price is over 24 hours old.
    Expired,
    /// No currency is chosen.
    NoCurrency,
    /// The sources have no price for the chosen currency.
    CurrencyMissing,
}

/// The state of the chosen currency's price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum RateState {
    Off(RatesOff),
    /// No number anywhere.
    Unavailable(RatesUnavailable),
    /// At most 30 minutes old.
    Fresh,
    /// Over 30 minutes and at most 24 hours old: shown with a warning.
    Stale,
}

/// A price: fiat per one DASH, as an exact decimal string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiatRate {
    pub currency: String,
    /// `^[0-9]{1,15}(\.[0-9]{1,12})?$`, above zero.
    pub price: String,
    /// The price's age anchor (UNIX seconds): the earlier of when the
    /// provider took it and when it was fetched.
    pub rate_at: u64,
    pub source: RateSourceId,
}

/// What every fiat line reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatesView {
    pub settings: RatesSettings,
    pub state: RateState,
    /// `Some` exactly when `state` is `Fresh` or `Stale`.
    pub rate: Option<FiatRate>,
    /// The price moved more than 50 % since the one before it (within 7
    /// days). Never set while `Stale` or without a rate.
    pub volatile: bool,
    /// The last attempt (every source) failed, or refused the chosen
    /// currency's new price.
    pub last_attempt_failed: bool,
    pub last_attempt_at: Option<u64>,
    /// The last attempt that brought a price for the chosen currency.
    pub last_success_at: Option<u64>,
}

/// A row of the currency picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyRate {
    pub code: String,
    pub price: String,
    pub rate_at: u64,
    pub source: RateSourceId,
}

/// The price a transaction was first seen at (rates-engine §6), in the
/// currency chosen then. Written once; a later price or currency never
/// replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxFiat {
    pub currency: String,
    /// Fiat per 1 DASH, an exact decimal string.
    pub price: String,
    /// UNIX seconds the price is from.
    pub rate_at: u64,
    pub source: RateSourceId,
}

impl RateSourceId {
    /// The name `tx_fiat.source` stores.
    pub(crate) fn stored(self) -> &'static str {
        match self {
            Self::Ctx => "ctx",
            Self::BitPayDashCentral => "bitPayDashCentral",
        }
    }

    pub(crate) fn from_stored(s: &str) -> Option<Self> {
        match s {
            "ctx" => Some(Self::Ctx),
            "bitPayDashCentral" => Some(Self::BitPayDashCentral),
            _ => None,
        }
    }
}
