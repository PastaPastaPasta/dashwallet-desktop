//! Amount formatting and parsing with dash-qt's exact rules (dw-units,
//! QT-020, QT-036, QT-039, QT-152). Owner: E2. Pure functions, no session.
//! Contract: docs/contracts/m1-engine.md §units.

use dw_units::{Chain, SeparatorStyle, Unit};

use crate::DashNetwork;
use crate::api::common::domain_error_common;

/// dash-qt display units; the discriminant order is dash-qt's `nDisplayUnit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum DisplayUnit {
    Dash,
    MilliDash,
    MicroDash,
    Duffs,
}

impl From<DisplayUnit> for Unit {
    fn from(u: DisplayUnit) -> Self {
        match u {
            DisplayUnit::Dash => Unit::Dash,
            DisplayUnit::MilliDash => Unit::MilliDash,
            DisplayUnit::MicroDash => Unit::MicroDash,
            DisplayUnit::Duffs => Unit::Duffs,
        }
    }
}

/// Thousands separators (dash-qt `SeparatorStyle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum Separators {
    Never,
    /// Only when the integer part has more than four digits.
    Standard,
    Always,
}

impl From<Separators> for SeparatorStyle {
    fn from(s: Separators) -> Self {
        match s {
            Separators::Never => SeparatorStyle::Never,
            Separators::Standard => SeparatorStyle::Standard,
            Separators::Always => SeparatorStyle::Always,
        }
    }
}

/// Which dash-qt formatter to apply. Separators are U+2009 thin spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AmountStyle {
    /// `BitcoinUnits::format`: the number only.
    Plain {
        plus_sign: bool,
        separators: Separators,
    },
    /// `BitcoinUnits::formatWithUnit`: number, space, unit name.
    WithUnit {
        plus_sign: bool,
        separators: Separators,
    },
    /// `BitcoinUnits::floorWithUnit`: decimals cut (not rounded) to `digits`
    /// (dash-qt "decimal digits" option, 2–8).
    Floored {
        plus_sign: bool,
        separators: Separators,
        digits: u8,
    },
    /// `BitcoinUnits::formatWithPrivacy`: right-justified; digits replaced by
    /// `#` when `hidden` (discreet mode, QT-039).
    Privacy {
        separators: Separators,
        hidden: bool,
    },
    /// `GUIUtil::formatAmount`: always separated, optional truncation to
    /// `truncate` decimals, then the unit name.
    Gui { signed: bool, truncate: Option<u8> },
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum UnitsError {
    /// Code `units.unparsable`: dash-qt's parser rejects the text.
    #[error("unparsable amount")]
    Unparsable,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
}

domain_error_common!(@not_implemented UnitsError);

crate::api::common::export_error_code!(UnitsError);

impl UnitsError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
        match self {
            Self::Unparsable => "units.unparsable",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NotImplemented { .. } => "not_implemented",
        }
    }
}

fn chain(network: &DashNetwork) -> Chain {
    match network {
        DashNetwork::Mainnet => Chain::Main,
        _ => Chain::Test,
    }
}

/// Formats `amount` duffs. `network` picks `DASH` or `tDASH` names.
#[uniffi::export]
pub fn format_amount(
    amount: i64,
    unit: DisplayUnit,
    network: DashNetwork,
    style: AmountStyle,
) -> Result<String, UnitsError> {
    let (unit, chain) = (Unit::from(unit), chain(&network));
    Ok(match style {
        AmountStyle::Plain {
            plus_sign,
            separators,
        } => dw_units::format(unit, amount, plus_sign, separators.into(), false),
        AmountStyle::WithUnit {
            plus_sign,
            separators,
        } => dw_units::format_with_unit(unit, amount, plus_sign, separators.into(), chain),
        AmountStyle::Floored {
            plus_sign,
            separators,
            digits,
        } => {
            if digits > 8 {
                return Err(UnitsError::InvalidArgument {
                    detail: format!("digits must be 0..=8, got {digits}"),
                });
            }
            dw_units::floor_with_unit(
                unit,
                amount,
                plus_sign,
                separators.into(),
                usize::from(digits),
                chain,
            )
        }
        AmountStyle::Privacy { separators, hidden } => {
            dw_units::format_with_privacy(unit, amount, separators.into(), hidden, chain)
        }
        AmountStyle::Gui { signed, truncate } => {
            dw_units::format_amount(unit, amount, signed, truncate, chain)
        }
    })
}

/// Parses user input in `unit` to duffs with `BitcoinUnits::parse` rules
/// (spaces and thin spaces ignored; at most the unit's decimals). Range
/// checks (dust, maximum supply) are the caller's.
#[uniffi::export]
pub fn parse_amount(text: String, unit: DisplayUnit) -> Result<i64, UnitsError> {
    dw_units::parse(unit.into(), &text).ok_or(UnitsError::Unparsable)
}

/// The unit's name on `network` (`DASH`, `mtDASH`, `μDASH`, `tduffs`, …).
#[uniffi::export]
pub fn unit_name(unit: DisplayUnit, network: DashNetwork) -> String {
    Unit::from(unit).name(chain(&network)).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_dash_qt() {
        let s = format_amount(
            123_456_789_012,
            DisplayUnit::Dash,
            DashNetwork::Mainnet,
            AmountStyle::WithUnit {
                plus_sign: true,
                separators: Separators::Always,
            },
        )
        .unwrap();
        assert_eq!(s, "+1\u{2009}234.56789012 DASH");
        assert_eq!(
            unit_name(DisplayUnit::Duffs, DashNetwork::Regtest),
            "tduffs"
        );
        assert_eq!(
            parse_amount("1.5".into(), DisplayUnit::Dash).unwrap(),
            150_000_000
        );
        assert!(matches!(
            parse_amount("1.123456789".into(), DisplayUnit::Dash),
            Err(UnitsError::Unparsable)
        ));
        assert!(matches!(
            format_amount(
                1,
                DisplayUnit::Dash,
                DashNetwork::Mainnet,
                AmountStyle::Floored {
                    plus_sign: false,
                    separators: Separators::Never,
                    digits: 9
                }
            ),
            Err(UnitsError::InvalidArgument { .. })
        ));
    }
}
