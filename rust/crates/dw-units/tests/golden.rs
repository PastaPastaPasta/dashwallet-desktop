//! Checks every vector in testdata/amount_format.json (generated from dash-qt
//! code by testdata/oracle/qt).

use dw_units::{Chain, SeparatorStyle, Unit};
use proptest::prelude::*;
use serde_json::Value;

fn load() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/amount_format.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read amount_format.json"))
        .expect("parse json")
}

fn unit(v: &Value) -> Unit {
    match v["unit"].as_str().unwrap() {
        "DASH" => Unit::Dash,
        "mDASH" => Unit::MilliDash,
        "uDASH" => Unit::MicroDash,
        "duffs" => Unit::Duffs,
        other => panic!("unknown unit {other}"),
    }
}

fn sep(v: &Value) -> SeparatorStyle {
    match v["separators"].as_str().unwrap() {
        "never" => SeparatorStyle::Never,
        "standard" => SeparatorStyle::Standard,
        "always" => SeparatorStyle::Always,
        other => panic!("unknown separator style {other}"),
    }
}

fn chain(v: &Value) -> Chain {
    match v["network"].as_str().unwrap() {
        "main" => Chain::Main,
        "test" => Chain::Test,
        other => panic!("unknown network {other}"),
    }
}

fn amount(v: &Value) -> i64 {
    int_field(v, "amount")
}

/// The oracle writes integers above 2^53 as strings so JSON readers that use
/// doubles stay exact.
fn int_field(v: &Value, key: &str) -> i64 {
    match &v[key] {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap(),
        Value::String(s) => s.parse().unwrap(),
        other => panic!("bad amount {other}"),
    }
}

fn cases<'a>(doc: &'a Value, key: &str) -> &'a Vec<Value> {
    let list = doc[key]
        .as_array()
        .unwrap_or_else(|| panic!("missing {key}"));
    assert!(!list.is_empty(), "{key} is empty");
    list
}

#[test]
fn format_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "format") {
        let got = dw_units::format(
            unit(c),
            amount(c),
            c["plus"].as_bool().unwrap(),
            sep(c),
            c["justify"].as_bool().unwrap(),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn format_with_unit_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "format_with_unit") {
        let got = dw_units::format_with_unit(
            unit(c),
            amount(c),
            c["plus"].as_bool().unwrap(),
            sep(c),
            chain(c),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn format_with_privacy_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "format_with_privacy") {
        let got = dw_units::format_with_privacy(
            unit(c),
            amount(c),
            sep(c),
            c["privacy"].as_bool().unwrap(),
            chain(c),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn floor_with_unit_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "floor_with_unit") {
        let got = dw_units::floor_with_unit(
            unit(c),
            amount(c),
            c["plus"].as_bool().unwrap(),
            sep(c),
            c["digits"].as_u64().unwrap() as usize,
            chain(c),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn format_amount_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "format_amount") {
        let truncate = c["truncate"].as_u64().map(|t| t as u8);
        let got = dw_units::format_amount(
            unit(c),
            amount(c),
            c["signed"].as_bool().unwrap(),
            truncate,
            chain(c),
        );
        assert_eq!(got, c["out"].as_str().unwrap(), "case {c}");
    }
}

#[test]
fn parse_matches_dash_qt() {
    let doc = load();
    for c in cases(&doc, "parse") {
        let got = dw_units::parse(unit(c), c["input"].as_str().unwrap());
        if c["ok"].as_bool().unwrap() {
            assert_eq!(got, Some(int_field(c, "value")), "case {c}");
        } else {
            assert_eq!(got, None, "case {c}");
        }
    }
}

fn any_unit() -> impl Strategy<Value = Unit> {
    prop::sample::select(Unit::ALL.to_vec())
}

fn any_sep() -> impl Strategy<Value = SeparatorStyle> {
    prop::sample::select(vec![
        SeparatorStyle::Never,
        SeparatorStyle::Standard,
        SeparatorStyle::Always,
    ])
}

proptest! {
    // dash-qt reads back what it writes: `parse(format(x)) == x` for every
    // amount whose padded digit string fits in 18 characters.
    #[test]
    fn format_parse_round_trip(u in any_unit(), s in any_sep(), plus in any::<bool>(),
                               amount in -dw_units::MAX_MONEY..=dw_units::MAX_MONEY) {
        let text = dw_units::format(u, amount, plus, s, false);
        prop_assert_eq!(dw_units::parse(u, &text), Some(amount));
    }

    #[test]
    fn justified_round_trip(u in any_unit(), amount in 0..=dw_units::MAX_MONEY) {
        let text = dw_units::format(u, amount, false, SeparatorStyle::Always, true);
        prop_assert_eq!(dw_units::parse(u, &text), Some(amount));
    }

    #[test]
    fn floor_never_exceeds_amount(amount in 0..=dw_units::MAX_MONEY, digits in 2usize..=8) {
        let text = dw_units::floor_with_unit(Unit::Dash, amount, false, SeparatorStyle::Never, digits, Chain::Main);
        let number = text.trim_end_matches(" DASH");
        let back = dw_units::parse(Unit::Dash, number).unwrap();
        prop_assert!(back <= amount);
        prop_assert!(amount - back < 10i64.pow(8 - digits as u32));
    }
}
