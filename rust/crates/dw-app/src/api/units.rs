//! Amounts with dash-qt's exact formatting and parsing rules (dw-units,
//! QT-020, QT-036, QT-039, QT-152), so the renderer does not re-implement
//! them. Pure functions: no session. Networks are named by their directory
//! name (`NetworkInfo.dirName`), which picks `DASH` or `tDASH`.

use dw_units::{Chain, SeparatorStyle, Unit};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::{mirror_enum, parse_network};
use crate::error::AppError;
use crate::ipc::commands;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// dash-qt's display units, in its `nDisplayUnit` order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DisplayUnit {
    Dash,
    MilliDash,
    MicroDash,
    Duffs,
}

mirror_enum!(DisplayUnit => Unit { Dash, MilliDash, MicroDash, Duffs });

/// Thousands separators (dash-qt `SeparatorStyle`), U+2009 thin spaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Separators {
    Never,
    /// Only when the integer part has more than four digits.
    Standard,
    Always,
}

mirror_enum!(Separators => SeparatorStyle { Never, Standard, Always });

/// Which dash-qt formatter to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
    /// (dash-qt's "decimal digits" option; 0–8).
    Floored {
        plus_sign: bool,
        separators: Separators,
        digits: u8,
    },
    /// `BitcoinUnits::formatWithPrivacy`: right-justified; digits replaced by
    /// `#` when `hidden` (discreet mode, QT-039).
    Privacy { separators: Separators, hidden: bool },
    /// `GUIUtil::formatAmount`: always separated, optionally truncated to
    /// `truncate` decimals, then the unit name.
    Gui { signed: bool, truncate: Option<u8> },
}

fn chain(network: &str) -> Result<Chain, AppError> {
    Ok(match parse_network(network)? {
        dw_engine::DashNetwork::Mainnet => Chain::Main,
        dw_engine::DashNetwork::Testnet
        | dw_engine::DashNetwork::Devnet { .. }
        | dw_engine::DashNetwork::Regtest => Chain::Test,
    })
}

fn format(amount: i64, unit: DisplayUnit, chain: Chain, style: AmountStyle) -> Result<String, AppError> {
    let unit = Unit::from(unit);
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
                return Err(AppError::invalid_argument(format!(
                    "digits must be 0..=8, got {digits}"
                )));
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

commands! {
    /// Formats `amount` duffs in `unit` with one of dash-qt's formatters.
    #[facade()]
    fn format_amount(
        state,
        amount: i64,
        unit: DisplayUnit,
        network: String,
        style: AmountStyle,
    ) -> String {
        format(amount, unit, chain(&network)?, style)
    }

    /// Parses user input in `unit` to duffs with `BitcoinUnits::parse`'s
    /// rules (spaces and thin spaces ignored; at most the unit's decimals);
    /// `null` when dash-qt's parser rejects the text. Range checks (dust,
    /// the maximum supply) are the caller's.
    #[facade()]
    fn parse_amount(state, text: String, unit: DisplayUnit) -> Option<i64> {
        Ok(dw_units::parse(unit.into(), &text))
    }

    /// The unit's name on `network` (`DASH`, `mtDASH`, `μDASH`, `tduffs`, …).
    #[facade()]
    fn unit_name(state, unit: DisplayUnit, network: String) -> String {
        Ok(Unit::from(unit).name(chain(&network)?).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_like_dash_qt() {
        let plain = AmountStyle::WithUnit {
            plus_sign: false,
            separators: Separators::Standard,
        };
        let main = chain("mainnet").unwrap();
        assert_eq!(format(150_000_000, DisplayUnit::Dash, main, plain).unwrap(), "1.50000000 DASH");
        assert_eq!(
            format(1, DisplayUnit::Duffs, chain("regtest").unwrap(), plain).unwrap(),
            "1 tduffs"
        );
        let floored = AmountStyle::Floored {
            plus_sign: false,
            separators: Separators::Never,
            digits: 9,
        };
        assert_eq!(
            format(1, DisplayUnit::Dash, main, floored).unwrap_err().code_str(),
            "invalid_argument"
        );
        assert_eq!(dw_units::parse(Unit::Dash, "1.5"), Some(150_000_000));
        assert_eq!(dw_units::parse(Unit::Dash, "1.123456789"), None);
        let json = serde_json::to_value(AmountStyle::Gui {
            signed: true,
            truncate: Some(2),
        })
        .unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "gui", "signed": true, "truncate": 2 }));
    }
}
