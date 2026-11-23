use super::types::RatesOff;

/// The rates facade's errors (`docs/contracts/rates-engine.md` §4).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RatesError {
    /// Not an ISO 4217 code (three capital letters).
    #[error("not a currency code: {0:?}")]
    InvalidCurrency(String),
    /// `refresh` within 30 seconds of the last attempt.
    #[error("prices were asked for less than 30 seconds ago")]
    Throttled,
    /// `refresh` while no request may be made.
    #[error("prices are off ({0:?})")]
    Off(RatesOff),
    /// `rates.json` could not be written.
    #[error("storage: {0}")]
    Storage(String),
}

impl RatesError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidCurrency(_) => "rates.invalid_currency",
            Self::Throttled => "rates.throttled",
            Self::Off(_) => "rates.off",
            Self::Storage(_) => "rates.storage",
        }
    }
}
