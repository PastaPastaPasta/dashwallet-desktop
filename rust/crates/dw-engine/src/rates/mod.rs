//! The rates facade (MP-01, `docs/contracts/rates-engine.md`): DASH prices
//! in fiat for the UI. Prices are app data, not a network's: one service
//! per [`crate::Engine`], persisted in `<data_root>/rates.json`.
//!
//! No request is made while prices are off, before the disclosure was
//! shown, or without an open mainnet network. A request carries nothing of
//! the wallet: fixed URLs, no cookie, a User-Agent with no app or version.
//! The engine converts no amount; the renderer does, from the decimal
//! price this facade gives.

mod decimal;
mod errors;
mod service;
mod sources;
mod state;
#[cfg(test)]
pub(crate) mod tests;
mod types;

use std::sync::Arc;

pub use errors::RatesError;
pub(crate) use service::{RatesConfig, RatesHub};
pub use types::{
    CurrencyRate, FiatRate, RateSourceId, RateState, RatesOff, RatesSettings, RatesUnavailable,
    RatesView, TxFiat,
};

/// Tests: `<data_root>/rates.json` with prices on and disclosed, `currency`
/// chosen and its price `price` fetched at `now`, for an engine started on
/// `data_root` afterwards.
#[cfg(test)]
pub(crate) fn fresh_rates_file_for_tests(
    data_root: &std::path::Path,
    currency: &str,
    price: &str,
    now: u64,
) {
    let cached = state::Cached {
        price: price.into(),
        retrieved_at: now,
        fetched_at: now,
        source: RateSourceId::Ctx,
        prev: None,
    };
    let stored = state::Stored {
        settings: RatesSettings {
            enabled: true,
            currency: Some(currency.into()),
            disclosed: true,
        },
        rates: [(currency.to_owned(), cached)].into(),
        last_attempt_at: Some(now),
        ..state::Stored::default()
    };
    std::fs::create_dir_all(data_root).unwrap();
    std::fs::write(
        data_root.join("rates.json"),
        serde_json::to_vec(&stored).unwrap(),
    )
    .unwrap();
}

/// The engine's rates. A cheap handle on the engine's one service.
#[derive(Clone)]
pub struct Rates {
    pub(crate) hub: Arc<RatesHub>,
}

impl crate::Engine {
    /// The rates facade.
    pub fn rates(&self) -> Rates {
        Rates {
            hub: self.rates_hub(),
        }
    }
}

impl Rates {
    /// The settings and the chosen currency's price and state now.
    pub fn status(&self) -> RatesView {
        self.hub.view()
    }

    /// Every currency with a price that has not expired, for the picker;
    /// empty while prices are off.
    pub fn currencies(&self) -> Vec<CurrencyRate> {
        self.hub.currencies()
    }

    /// Stores the settings (`rates.json`) and applies them: turning prices
    /// off stops every request at once; a request already out is let
    /// finish, and what it brings is dropped.
    pub fn set_settings(&self, settings: RatesSettings) -> Result<RatesView, RatesError> {
        self.hub.set_settings(settings)
    }

    /// One attempt now ("Update now"): refused while prices are off and
    /// within 30 seconds of the last attempt. The view says how it went.
    pub async fn refresh(&self) -> Result<RatesView, RatesError> {
        self.hub.refresh().await
    }
}
