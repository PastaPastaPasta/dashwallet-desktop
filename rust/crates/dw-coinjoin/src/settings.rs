//! Options → CoinJoin (QT-046; Dash Core `src/coinjoin/options.h`). The
//! options are global for a network; mixing state is per wallet (QT-049).

/// The CoinJoin options. `target_amount_dash` is whole DASH, as Core's
/// `coinjoinamount`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoinJoinSettings {
    /// `enablecoinjoin` (dash-qt default on).
    pub enabled: bool,
    pub multi_session: bool,
    pub max_sessions: u32,
    pub rounds: u32,
    pub target_amount_dash: u32,
    pub denoms_goal: u32,
    pub denoms_hard_cap: u32,
}

pub const MIN_SESSIONS: u32 = 1;
pub const MAX_SESSIONS: u32 = 10;
pub const MIN_ROUNDS: u32 = 2;
pub const MAX_ROUNDS: u32 = 16;
pub const MIN_AMOUNT_DASH: u32 = 2;
/// `MAX_MONEY / COIN`.
pub const MAX_AMOUNT_DASH: u32 = 21_000_000;
pub const MIN_DENOMS: u32 = 10;
pub const MAX_DENOMS: u32 = 100_000;

impl Default for CoinJoinSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            multi_session: false,
            max_sessions: 4,
            rounds: 4,
            target_amount_dash: 1_000,
            denoms_goal: 50,
            denoms_hard_cap: 300,
        }
    }
}

/// A setting outside its range.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{field} = {value} is outside {min}..={max}")]
pub struct SettingOutOfRange {
    pub field: &'static str,
    pub value: u32,
    pub min: u32,
    pub max: u32,
}

impl CoinJoinSettings {
    /// Checks every range, and that the denominations goal is not above
    /// the hard cap (dash-qt keeps Target ≤ Maximum).
    pub fn validate(&self) -> Result<(), SettingOutOfRange> {
        let check = |field, value, min, max| {
            if (min..=max).contains(&value) {
                Ok(())
            } else {
                Err(SettingOutOfRange {
                    field,
                    value,
                    min,
                    max,
                })
            }
        };
        check(
            "max_sessions",
            self.max_sessions,
            MIN_SESSIONS,
            MAX_SESSIONS,
        )?;
        check("rounds", self.rounds, MIN_ROUNDS, MAX_ROUNDS)?;
        check(
            "target_amount_dash",
            self.target_amount_dash,
            MIN_AMOUNT_DASH,
            MAX_AMOUNT_DASH,
        )?;
        check(
            "denoms_hard_cap",
            self.denoms_hard_cap,
            MIN_DENOMS,
            MAX_DENOMS,
        )?;
        check(
            "denoms_goal",
            self.denoms_goal,
            MIN_DENOMS,
            self.denoms_hard_cap,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_ranges_are_enforced() {
        assert!(CoinJoinSettings::default().validate().is_ok());
        let bad = CoinJoinSettings {
            rounds: 17,
            ..CoinJoinSettings::default()
        };
        assert_eq!(bad.validate().unwrap_err().field, "rounds");
        let goal_above_cap = CoinJoinSettings {
            denoms_goal: 301,
            ..CoinJoinSettings::default()
        };
        assert_eq!(goal_above_cap.validate().unwrap_err().field, "denoms_goal");
    }
}
