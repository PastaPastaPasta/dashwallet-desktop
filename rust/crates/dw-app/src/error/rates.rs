//! The rates facade's error (rates-engine §4, MP-01). The strings are the
//! engine's `code()`s; `tests::codes_match_the_engine` holds them equal.

use dw_engine::rates::RatesError;

use super::{AppError, codes};

codes! {
    RatesCode {
        /// Not an ISO 4217 code (three capital letters).
        InvalidCurrency = "rates.invalid_currency",
        /// "Update now" within 30 seconds of the last attempt.
        Throttled = "rates.throttled",
        /// "Update now" while prices are off, not disclosed yet, or no
        /// mainnet network is open: no request is made.
        Off = "rates.off",
        /// `rates.json` could not be written.
        Storage = "rates.storage",
    }
}

impl From<RatesError> for AppError {
    fn from(e: RatesError) -> Self {
        let message = e.to_string();
        let code = match e {
            RatesError::InvalidCurrency(_) => RatesCode::InvalidCurrency,
            RatesError::Throttled => RatesCode::Throttled,
            RatesError::Off(_) => RatesCode::Off,
            RatesError::Storage(_) => RatesCode::Storage,
        };
        AppError::new(code, message)
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::rates::RatesOff;

    use super::*;

    #[test]
    fn codes_match_the_engine() {
        let all = [
            RatesError::InvalidCurrency("x".into()),
            RatesError::Throttled,
            RatesError::Off(RatesOff::Disabled),
            RatesError::Storage("x".into()),
        ];
        assert_eq!(all.len(), RatesCode::ALL.len());
        for e in all {
            let engine = e.code();
            assert_eq!(AppError::from(e).code_str(), engine);
        }
    }
}
