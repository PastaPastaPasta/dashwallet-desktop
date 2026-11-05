//! What `rates.json` holds and the rules on it, as pure functions of the
//! time (`docs/contracts/rates-engine.md` §3): which new prices are taken,
//! and what the chosen currency's state is now.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::decimal::Price;
use super::sources::SourceRates;
use super::types::{
    CurrencyRate, FiatRate, RateSourceId, RateState, RatesOff, RatesSettings, RatesUnavailable,
    RatesView,
};

/// A price is fresh this long (both mobile apps' stale threshold).
pub(crate) const FRESH_SECS: u64 = 30 * 60;
/// After this no number is shown ("never a guessed fiat").
pub(crate) const EXPIRE_SECS: u64 = 24 * 60 * 60;
/// A provider time further ahead than this is refused (clock skew allowed
/// for; a price "from the future" is not taken).
pub(crate) const FUTURE_SKEW_SECS: u64 = 5 * 60;
/// Volatile: a move of more than half (Android's rule, both directions).
const VOLATILE: (u128, u128) = (1, 2);
/// ... against a price at most this old.
const VOLATILE_WINDOW_SECS: u64 = 7 * 24 * 60 * 60;
/// A new price more than this many times above or below a cached one that
/// has not expired is refused as absurd.
pub(crate) const MAX_JUMP: u128 = 10;

/// The file's format version.
const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Prev {
    pub price: String,
    pub fetched_at: u64,
}

/// One currency's last taken price.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cached {
    pub price: String,
    pub retrieved_at: u64,
    pub fetched_at: u64,
    pub source: RateSourceId,
    /// The price before this one, for the volatility check.
    pub prev: Option<Prev>,
}

impl Cached {
    fn rate_at(&self) -> u64 {
        self.retrieved_at.min(self.fetched_at)
    }

    fn age(&self, now: u64) -> u64 {
        now.saturating_sub(self.rate_at())
    }

    fn price(&self) -> Option<Price> {
        Price::parse(&self.price)
    }
}

/// `rates.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Stored {
    pub v: u32,
    pub settings: RatesSettings,
    pub rates: BTreeMap<String, Cached>,
    pub last_attempt_at: Option<u64>,
    /// Every source failed on the last attempt.
    pub last_attempt_failed: bool,
    /// Currencies whose new price the last attempt refused.
    pub refused: BTreeSet<String>,
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            v: VERSION,
            settings: RatesSettings::default(),
            rates: BTreeMap::new(),
            last_attempt_at: None,
            last_attempt_failed: false,
            refused: BTreeSet::new(),
        }
    }
}

/// Why a new row was not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    Future,
    OlderThanCached,
    Jump,
}

impl Stored {
    /// Reads the file's text; anything unreadable, of another version or
    /// holding an invalid price is the default (nothing fetched, the
    /// settings' defaults), as if there were no file.
    pub(crate) fn from_json(text: &str) -> Stored {
        match serde_json::from_str::<Stored>(text) {
            Ok(s) if s.v == VERSION && s.is_valid() => s,
            _ => Stored::default(),
        }
    }

    fn is_valid(&self) -> bool {
        self.settings.currency.as_deref().is_none_or(is_code)
            && self.rates.iter().all(|(code, c)| {
                is_code(code)
                    && c.price().is_some()
                    && c.prev
                        .as_ref()
                        .is_none_or(|p| Price::parse(&p.price).is_some())
            })
    }

    /// The attempt failed at every source.
    pub(crate) fn note_failure(&mut self, now: u64) {
        self.last_attempt_at = Some(now);
        self.last_attempt_failed = true;
    }

    /// Takes `got`'s rows that pass the checks; returns the refused ones.
    pub(crate) fn take(&mut self, got: SourceRates, now: u64) -> Vec<(String, Refusal)> {
        self.last_attempt_at = Some(now);
        self.last_attempt_failed = false;
        self.refused.clear();
        let mut refused = Vec::new();
        for row in got.rows {
            let cached = self.rates.get(&row.currency);
            let check = check_row(cached, &row.price, row.retrieved_at, now);
            if let Err(why) = check {
                // A row older than the cached one is not a failure: the
                // price kept is the newer (a fallback's rows are stamped
                // with the fetch time, so CTX's next row can be older).
                if why != Refusal::OlderThanCached {
                    self.refused.insert(row.currency.clone());
                }
                refused.push((row.currency, why));
                continue;
            }
            let price = row.price.to_string();
            let next = match cached {
                // The same price taken at the same time: only the check time moves.
                Some(c) if c.retrieved_at == row.retrieved_at && c.price == price => Cached {
                    fetched_at: now,
                    ..c.clone()
                },
                _ => Cached {
                    price,
                    retrieved_at: row.retrieved_at,
                    fetched_at: now,
                    source: got.source,
                    prev: cached.map(|c| Prev {
                        price: c.price.clone(),
                        fetched_at: c.fetched_at,
                    }),
                },
            };
            self.rates.insert(row.currency, next);
        }
        refused
    }

    /// The chosen currency's state at `now`; `mainnet` says whether a
    /// mainnet network is open.
    pub(crate) fn view(&self, now: u64, mainnet: bool) -> RatesView {
        let s = &self.settings;
        let mut view = RatesView {
            settings: s.clone(),
            state: RateState::Unavailable(RatesUnavailable::NeverFetched),
            rate: None,
            volatile: false,
            last_attempt_failed: self.last_attempt_failed,
            last_attempt_at: self.last_attempt_at,
            last_success_at: None,
        };
        let off = if !s.enabled {
            Some(RatesOff::Disabled)
        } else if !s.disclosed {
            Some(RatesOff::NotDisclosed)
        } else if !mainnet {
            Some(RatesOff::NotMainnet)
        } else {
            None
        };
        if let Some(off) = off {
            view.state = RateState::Off(off);
            return view;
        }
        let Some(code) = s.currency.as_deref() else {
            view.state = RateState::Unavailable(RatesUnavailable::NoCurrency);
            return view;
        };
        view.last_attempt_failed |= self.refused.contains(code);
        let Some(row) = self.rates.get(code) else {
            view.state = RateState::Unavailable(if self.last_attempt_at.is_none() {
                RatesUnavailable::NeverFetched
            } else if self.rates.is_empty() {
                RatesUnavailable::Failed
            } else {
                RatesUnavailable::CurrencyMissing
            });
            return view;
        };
        view.last_success_at = Some(row.fetched_at);
        let age = row.age(now);
        view.state = if age <= FRESH_SECS {
            RateState::Fresh
        } else if age <= EXPIRE_SECS {
            RateState::Stale
        } else {
            view.state = RateState::Unavailable(RatesUnavailable::Expired);
            return view;
        };
        view.rate = Some(FiatRate {
            currency: code.to_owned(),
            price: row.price.clone(),
            rate_at: row.rate_at(),
            source: row.source,
        });
        view.volatile = view.state == RateState::Fresh && volatile(row);
        view
    }

    /// The picker's rows: every currency with a price that has not expired.
    pub(crate) fn currencies(&self, now: u64) -> Vec<CurrencyRate> {
        self.rates
            .iter()
            .filter(|(_, c)| c.age(now) <= EXPIRE_SECS)
            .map(|(code, c)| CurrencyRate {
                code: code.clone(),
                price: c.price.clone(),
                rate_at: c.rate_at(),
                source: c.source,
            })
            .collect()
    }
}

fn check_row(
    cached: Option<&Cached>,
    price: &Price,
    retrieved_at: u64,
    now: u64,
) -> Result<(), Refusal> {
    if retrieved_at > now.saturating_add(FUTURE_SKEW_SECS) {
        return Err(Refusal::Future);
    }
    let Some(c) = cached else {
        return Ok(());
    };
    if retrieved_at < c.retrieved_at {
        return Err(Refusal::OlderThanCached);
    }
    if c.age(now) <= EXPIRE_SECS
        && let Some(old) = c.price()
    {
        use std::cmp::Ordering::{Greater, Less};
        let up = price.ratio_cmp(old, MAX_JUMP, 1) == Some(Greater);
        let down = price.ratio_cmp(old, 1, MAX_JUMP) == Some(Less);
        if up || down {
            return Err(Refusal::Jump);
        }
    }
    Ok(())
}

fn volatile(row: &Cached) -> bool {
    let Some(prev) = &row.prev else {
        return false;
    };
    if row.fetched_at.saturating_sub(prev.fetched_at) > VOLATILE_WINDOW_SECS {
        return false;
    }
    match (row.price(), Price::parse(&prev.price)) {
        (Some(new), Some(old)) => new.moved_more_than(old, VOLATILE.0, VOLATILE.1) == Some(true),
        _ => false,
    }
}

pub(crate) fn is_code(c: &str) -> bool {
    c.len() == 3 && c.bytes().all(|b| b.is_ascii_uppercase())
}
