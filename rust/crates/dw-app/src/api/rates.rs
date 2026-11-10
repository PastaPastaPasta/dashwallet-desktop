//! Prices in fiat (MP-01, `docs/contracts/rates-engine.md`): the settings,
//! the chosen currency's price and its state, the picker's list and
//! "Update now". Prices are the app's, not a network's: these commands need
//! no open network. A change of what they return sends a `rates` event.
//! The engine converts no amount; the renderer does (`format/fiat.ts`).

use dw_engine::rates as r;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// Which source a price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RateSourceName {
    /// `rates.ctx.com`.
    Ctx,
    /// BitPay's fiat per BTC crossed with DashCentral's DASH/USD.
    BitPayDashCentral,
}

crate::dto::mirror_enum!(r::RateSourceId => RateSourceName { Ctx, BitPayDashCentral });

/// What the user chose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RatesSettings {
    /// "Show values in local currency". Off: no request at all.
    pub enabled: bool,
    /// ISO 4217 code; `null` until one is chosen.
    pub currency: Option<String>,
    /// The disclosure was shown; no request is made before it was.
    pub disclosed: bool,
}

impl From<r::RatesSettings> for RatesSettings {
    fn from(s: r::RatesSettings) -> Self {
        let r::RatesSettings {
            enabled,
            currency,
            disclosed,
        } = s;
        Self {
            enabled,
            currency,
            disclosed,
        }
    }
}

impl From<RatesSettings> for r::RatesSettings {
    fn from(s: RatesSettings) -> Self {
        let RatesSettings {
            enabled,
            currency,
            disclosed,
        } = s;
        Self {
            enabled,
            currency,
            disclosed,
        }
    }
}

/// Why no request is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RatesOffName {
    Disabled,
    NotDisclosed,
    /// Test coins have no price.
    NotMainnet,
}

crate::dto::mirror_enum!(r::RatesOff => RatesOffName { Disabled, NotDisclosed, NotMainnet });

/// Why there is no number although prices are on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RatesUnavailableName {
    NeverFetched,
    Failed,
    /// Over 24 hours old.
    Expired,
    NoCurrency,
    CurrencyMissing,
}

crate::dto::mirror_enum!(r::RatesUnavailable => RatesUnavailableName {
    NeverFetched, Failed, Expired, NoCurrency, CurrencyMissing
});

/// The chosen currency's price state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", content = "reason", rename_all = "camelCase")]
pub enum RateStateName {
    Off(RatesOffName),
    /// No number anywhere.
    Unavailable(RatesUnavailableName),
    /// At most 30 minutes old.
    Fresh,
    /// Over 30 minutes, at most 24 hours old: shown with a warning.
    Stale,
}

impl From<r::RateState> for RateStateName {
    fn from(s: r::RateState) -> Self {
        match s {
            r::RateState::Off(o) => Self::Off(o.into()),
            r::RateState::Unavailable(u) => Self::Unavailable(u.into()),
            r::RateState::Fresh => Self::Fresh,
            r::RateState::Stale => Self::Stale,
        }
    }
}

/// Fiat per one DASH.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FiatRate {
    pub currency: String,
    /// An exact decimal string (`^[0-9]{1,15}(\.[0-9]{1,12})?$`).
    pub price: String,
    /// When the price was taken (UNIX seconds).
    pub rate_at: u64,
    pub source: RateSourceName,
}

impl From<r::FiatRate> for FiatRate {
    fn from(f: r::FiatRate) -> Self {
        let r::FiatRate {
            currency,
            price,
            rate_at,
            source,
        } = f;
        Self {
            currency,
            price,
            rate_at,
            source: source.into(),
        }
    }
}

/// What every fiat line reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RatesView {
    pub settings: RatesSettings,
    pub state: RateStateName,
    /// Present exactly when `state` is `fresh` or `stale`.
    pub rate: Option<FiatRate>,
    /// Moved more than 50 % since the price before (never while stale).
    pub volatile: bool,
    pub last_attempt_failed: bool,
    pub last_attempt_at: Option<u64>,
    pub last_success_at: Option<u64>,
}

impl From<r::RatesView> for RatesView {
    fn from(v: r::RatesView) -> Self {
        let r::RatesView {
            settings,
            state,
            rate,
            volatile,
            last_attempt_failed,
            last_attempt_at,
            last_success_at,
        } = v;
        Self {
            settings: settings.into(),
            state: state.into(),
            rate: rate.map(Into::into),
            volatile,
            last_attempt_failed,
            last_attempt_at,
            last_success_at,
        }
    }
}

/// The price a transaction was first seen at (rates-engine §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TxFiat {
    pub currency: String,
    /// Fiat per 1 DASH, an exact decimal string.
    pub price: String,
    /// UNIX seconds the price is from.
    pub rate_at: u64,
    pub source: RateSourceName,
}

impl From<r::TxFiat> for TxFiat {
    fn from(f: r::TxFiat) -> Self {
        let r::TxFiat {
            currency,
            price,
            rate_at,
            source,
        } = f;
        Self {
            currency,
            price,
            rate_at,
            source: source.into(),
        }
    }
}

/// A row of the currency picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CurrencyRate {
    pub code: String,
    pub price: String,
    pub rate_at: u64,
    pub source: RateSourceName,
}

impl From<r::CurrencyRate> for CurrencyRate {
    fn from(c: r::CurrencyRate) -> Self {
        let r::CurrencyRate {
            code,
            price,
            rate_at,
            source,
        } = c;
        Self {
            code,
            price,
            rate_at,
            source: source.into(),
        }
    }
}

commands! {
    /// The settings and the chosen currency's price and state now.
    #[facade("Engine::rates", "Rates::status")]
    fn rates_status(state) -> RatesView {
        Ok(state.engine.rates().status().into())
    }

    /// Every currency with a price that has not expired, for the picker;
    /// empty while prices are off.
    #[facade("Engine::rates", "Rates::currencies")]
    fn rates_currencies(state) -> Vec<CurrencyRate> {
        Ok(state.engine.rates().currencies().into_iter().map(Into::into).collect())
    }

    /// Saves and applies the settings: off stops every request at once.
    /// `rates.invalid_currency` for a code that is not three capital letters.
    #[facade("Engine::rates", "Rates::set_settings")]
    fn set_rates_settings(state, settings: RatesSettings) -> RatesView {
        Ok(state.engine.rates().set_settings(settings.into())?.into())
    }

    /// "Update now": one attempt; `rates.off` while no request may be made,
    /// `rates.throttled` within 30 seconds of the last attempt.
    #[facade("Engine::rates", "Rates::refresh")]
    fn refresh_rates(state) -> RatesView {
        Ok(state.engine.rates().refresh().await?.into())
    }
}
