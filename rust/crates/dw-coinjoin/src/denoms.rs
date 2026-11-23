//! CoinJoin amounts (Dash Core `src/coinjoin/common.h`, `coinjoin.h`;
//! research 02 §9.1).

/// 1 DASH in duffs.
const COIN: u64 = 100_000_000;

/// Standard denominations, largest first (`vecStandardDenominations`). The
/// denomination bitmask bit of entry `i` is `1 << i`.
pub const DENOMINATIONS: [u64; 5] = [
    10 * COIN + 10_000,
    COIN + 1_000,
    COIN / 10 + 100,
    COIN / 100 + 10,
    COIN / 1_000 + 1,
];

/// Smallest denomination (`GetSmallestDenomination`), 0.00100001 DASH.
pub const SMALLEST_DENOMINATION: u64 = DENOMINATIONS[4];

/// Collateral amount (`GetCollateralAmount`), 0.0001 DASH.
pub const COLLATERAL_AMOUNT: u64 = SMALLEST_DENOMINATION / 10;

/// Largest collateral (`GetMaxCollateralAmount`), 0.0004 DASH.
pub const MAX_COLLATERAL_AMOUNT: u64 = COLLATERAL_AMOUNT * 4;

/// Balance needed to start mixing (dash-qt `overviewpage.cpp`): smallest
/// denomination plus the largest collateral, 0.00140001 DASH.
pub const MIN_MIXING_BALANCE: u64 = SMALLEST_DENOMINATION + MAX_COLLATERAL_AMOUNT;

/// Inputs per entry (`COINJOIN_ENTRY_MAX_SIZE`).
pub const ENTRY_MAX_INPUTS: usize = 9;

/// Seconds a queue waits for participants (`COINJOIN_QUEUE_TIMEOUT`).
pub const QUEUE_TIMEOUT_SECS: u64 = 30;

/// Seconds the signing phase may take (`COINJOIN_SIGNING_TIMEOUT`).
pub const SIGNING_TIMEOUT_SECS: u64 = 15;

/// Maintenance ticks between automatic denominating passes
/// (`COINJOIN_AUTO_TIMEOUT_MIN/MAX`, coinjoin.h:45-46).
pub const AUTO_TIMEOUT_MIN: u64 = 5;
pub const AUTO_TIMEOUT_MAX: u64 = 15;

/// Whether `amount` is one of the standard denominations.
pub fn is_denominated(amount: u64) -> bool {
    DENOMINATIONS.contains(&amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_balance_is_the_dash_qt_constant() {
        assert_eq!(MIN_MIXING_BALANCE, 140_001);
        assert_eq!(COLLATERAL_AMOUNT, 10_000);
        assert_eq!(MAX_COLLATERAL_AMOUNT, 40_000);
        assert!(is_denominated(100_001) && !is_denominated(100_000));
    }
}
