//! Checks a governance object from a peer before it is stored (MG-02 §3),
//! as Dash Core v24 `CGovernanceObject::IsValidLocally` does, minus what an
//! SPV wallet cannot check: the proposal collateral (no transaction index)
//! and the trigger's operator signature (the SML has no collateral
//! outpoints). Those are replaced by the two-peer rule of the store (MG-02
//! D3, D4).
//!
//! - The type is the data's `type` (`CGovernanceObject::LoadData`), not the
//!   wire field (D5).
//! - Proposals: `governance::ValidateProposal(hex, fCheckExpiration=false,
//!   fAllowScript=true)`, `governance/object.cpp`.
//! - Triggers: the `CSuperblock` constructor and `ParsePaymentSchedule`
//!   (`governance/superblock.cpp`), and the time window of
//!   `CGovernanceManager::MasternodeRateCheck`.
//!
//! JSON is read as UniValue reads it: the first of duplicate keys wins
//! (`UniValue::operator[]`), integers are JSON integers that fit, and a
//! `VNUM` is any JSON number.

use std::fmt;

use dashcore::Network;
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::Value;

use crate::object::{GovernanceObject, MAX_DATA_SIZE, OBJECT_TYPE_PROPOSAL, OBJECT_TYPE_TRIGGER};
use crate::params::governance_params;
use crate::proposal::{
    MAX_MONEY, ProposalData, TriggerData, json_amount, name_valid, payment_address_valid, url_valid,
};

/// How far ahead of the local clock an object may be timed (Core
/// `MAX_TIME_FUTURE_DEVIATION`, one hour).
pub const MAX_TIME_FUTURE_DEVIATION: i64 = 60 * 60;

/// What a valid object's data says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidObject {
    Proposal(ProposalData),
    Trigger(TriggerData),
}

/// Why an object was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRejection {
    /// Not a JSON object, or no integer `type` (Core `fUnparsable`).
    Unparsable,
    /// A `type` other than proposal or trigger.
    UnknownType,
    /// A rule of `ValidateProposal`; the field it failed on.
    InvalidProposal(&'static str),
    /// A rule of the `CSuperblock` constructor; the field it failed on.
    InvalidTrigger(&'static str),
    /// A trigger created more than an hour ahead or two cycles ago.
    TriggerTimeOutOfRange,
}

/// A JSON object whose first occurrence of a key wins, as UniValue's
/// lookup does (serde_json's map keeps the last).
struct FirstWins(serde_json::Map<String, Value>);

impl<'de> Deserialize<'de> for FirstWins {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = FirstWins;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<FirstWins, A::Error> {
                let mut out = serde_json::Map::new();
                while let Some((k, v)) = map.next_entry::<String, Value>()? {
                    out.entry(k).or_insert(v);
                }
                Ok(FirstWins(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// The data as a JSON object, or `None` (not JSON, not an object).
fn json_object(data: &[u8]) -> Option<serde_json::Map<String, Value>> {
    serde_json::from_slice::<FirstWins>(data).ok().map(|o| o.0)
}

/// `getInt<T>`: a JSON integer within `T`'s range.
fn int<T: TryFrom<i64>>(o: &serde_json::Map<String, Value>, key: &str) -> Option<T> {
    o.get(key)?.as_i64().and_then(|v| T::try_from(v).ok())
}

fn text<'a>(o: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    o.get(key)?.as_str()
}

/// Checks `object` as Core would before storing it from a peer; `now` is
/// the local clock in Unix seconds.
pub fn check_object(
    object: &GovernanceObject,
    network: Network,
    now: i64,
) -> Result<ValidObject, ObjectRejection> {
    let o = json_object(&object.data).ok_or(ObjectRejection::Unparsable)?;
    match int::<i32>(&o, "type").ok_or(ObjectRejection::Unparsable)? {
        OBJECT_TYPE_PROPOSAL => {
            check_proposal(&object.data, &o, network).map(ValidObject::Proposal)
        }
        OBJECT_TYPE_TRIGGER => {
            let trigger = check_trigger(&o, network)?;
            let p = governance_params(network);
            let cycle_secs = i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs);
            if object.time < now - 2 * cycle_secs || object.time > now + MAX_TIME_FUTURE_DEVIATION {
                return Err(ObjectRejection::TriggerTimeOutOfRange);
            }
            Ok(ValidObject::Trigger(trigger))
        }
        _ => Err(ObjectRejection::UnknownType),
    }
}

/// `governance::ValidateProposal` with scripts allowed and no expiry check.
fn check_proposal(
    data: &[u8],
    o: &serde_json::Map<String, Value>,
    network: Network,
) -> Result<ProposalData, ObjectRejection> {
    use ObjectRejection::InvalidProposal as Bad;
    if data.len() > MAX_DATA_SIZE {
        return Err(Bad("size"));
    }
    let name = text(o, "name")
        .filter(|n| name_valid(n))
        .ok_or(Bad("name"))?;
    let start = int::<i64>(o, "start_epoch").ok_or(Bad("start_epoch"))?;
    let end = int::<i64>(o, "end_epoch").ok_or(Bad("end_epoch"))?;
    if end <= start {
        return Err(Bad("end_epoch"));
    }
    // `get_real() > 0`; the duffs are read from the number's text.
    let amount = o.get("payment_amount").ok_or(Bad("payment_amount"))?;
    if !amount.as_f64().is_some_and(|a| a > 0.0) {
        return Err(Bad("payment_amount"));
    }
    let address = text(o, "payment_address")
        .filter(|a| payment_address_valid(a, network))
        .ok_or(Bad("payment_address"))?;
    let url = text(o, "url").filter(|u| url_valid(u)).ok_or(Bad("url"))?;
    Ok(ProposalData {
        name: Some(name.to_string()),
        url: Some(url.to_string()),
        payment_address: Some(address.to_string()),
        payment_amount: json_amount(amount),
        start_epoch: Some(start),
        end_epoch: Some(end),
    })
}

/// The `CSuperblock` constructor: integer `event_block_height`, and the
/// `|`-separated addresses, amounts and proposal hashes in equal, non-empty
/// lists of valid entries.
fn check_trigger(
    o: &serde_json::Map<String, Value>,
    network: Network,
) -> Result<TriggerData, ObjectRejection> {
    use ObjectRejection::InvalidTrigger as Bad;
    let height = int::<i32>(o, "event_block_height").ok_or(Bad("event_block_height"))?;
    let addresses = text(o, "payment_addresses").ok_or(Bad("payment_addresses"))?;
    let amounts = text(o, "payment_amounts").ok_or(Bad("payment_amounts"))?;
    let hashes = text(o, "proposal_hashes").ok_or(Bad("proposal_hashes"))?;
    // Core's `SplitString` gives one empty entry for an empty string, which
    // then fails as an address: no payments is refused either way.
    let addresses: Vec<&str> = addresses.split('|').collect();
    let amounts: Vec<&str> = amounts.split('|').collect();
    let hashes: Vec<&str> = hashes.split('|').collect();
    if addresses.len() != amounts.len() || addresses.len() != hashes.len() {
        return Err(Bad("payment_count"));
    }
    let mut proposal_hashes = Vec::with_capacity(hashes.len());
    for ((address, amount), hash) in addresses.iter().zip(&amounts).zip(&hashes) {
        if !payment_address_valid(address, network) {
            return Err(Bad("payment_addresses"));
        }
        parse_payment_amount(amount).ok_or(Bad("payment_amounts"))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Bad("proposal_hashes"));
        }
        proposal_hashes.push(hash.to_ascii_lowercase());
    }
    Ok(TriggerData {
        event_block_height: u32::try_from(height).map_err(|_| Bad("event_block_height"))?,
        proposal_hashes,
    })
}

/// Core `ParsePaymentAmount`: at most 20 characters of digits and one
/// point, not leading, then `ParseFixedPoint(…, 8)` (no leading zeros but a
/// single `0`, a digit after the point, at most 8 decimals besides trailing
/// zeros) within the money range. Duffs.
pub fn parse_payment_amount(s: &str) -> Option<u64> {
    if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (s, None),
    };
    if whole.is_empty() || (whole.len() > 1 && whole.starts_with('0')) {
        return None;
    }
    let frac = match frac {
        None => "",
        Some(f) if f.is_empty() || f.contains('.') => return None,
        Some(f) => f.trim_end_matches('0'),
    };
    if frac.len() > 8 {
        return None;
    }
    let w: u64 = whole.parse().ok()?;
    let f: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<8}").parse().ok()?
    };
    let v = w.checked_mul(100_000_000)?.checked_add(f)?;
    (v <= MAX_MONEY).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
    const HASH: &str = "4f6e2b3e7d8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b";

    fn object(data: &str, time: i64) -> GovernanceObject {
        GovernanceObject::new_proposal(1, time, data.as_bytes().to_vec())
    }

    fn proposal(fields: &str) -> String {
        format!(r#"{{{fields}"type":1}}"#)
    }

    const GOOD: &str = r#""name":"my-prop_1","start_epoch":100,"end_epoch":200,"payment_amount":12.5,"payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","url":"http://x.y","#;

    fn check(data: &str) -> Result<ValidObject, ObjectRejection> {
        check_object(&object(data, 1_000), Network::Testnet, 1_000)
    }

    #[test]
    fn proposals_follow_validate_proposal() {
        let ok = check(&proposal(GOOD)).unwrap();
        let ValidObject::Proposal(p) = ok else {
            panic!("not a proposal")
        };
        assert_eq!(p.payment_amount, Some(1_250_000_000));
        assert_eq!(p.name.as_deref(), Some("my-prop_1"));

        let bad = |from: &str, to: &str, field| {
            let d = proposal(&GOOD.replace(from, to));
            assert_eq!(
                check(&d),
                Err(ObjectRejection::InvalidProposal(field)),
                "{d}"
            );
        };
        bad(r#""my-prop_1""#, r#""a b""#, "name");
        bad(r#""my-prop_1""#, r#""""#, "name");
        bad(
            r#""my-prop_1""#,
            &format!(r#""{}""#, "a".repeat(41)),
            "name",
        );
        bad(r#""end_epoch":200"#, r#""end_epoch":100"#, "end_epoch");
        bad(r#""end_epoch":200"#, r#""end_epoch":200.0"#, "end_epoch");
        bad(
            r#""start_epoch":100"#,
            r#""start_epoch":"100""#,
            "start_epoch",
        );
        bad("12.5", "0", "payment_amount");
        bad("12.5", r#""12.5""#, "payment_amount");
        bad(
            ADDR,
            "XjB5QkRfEcYFv7TaQHxbHWNdS9iz1x6tCy",
            "payment_address",
        );
        bad(ADDR, &format!("{ADDR} "), "payment_address");
        bad("http://x.y", "x.y", "url");
        bad("http://x.y", "http://[x", "url");
        bad("http://x.y", "http:// x", "url");

        // Type comes from the data; a proposal labelled a trigger on the
        // wire is still checked as a proposal.
        let mut wire = object(&proposal(GOOD), 1_000);
        wire.object_type = OBJECT_TYPE_TRIGGER;
        assert!(matches!(
            check_object(&wire, Network::Testnet, 1_000),
            Ok(ValidObject::Proposal(_))
        ));
        assert_eq!(check("[]"), Err(ObjectRejection::Unparsable));
        assert_eq!(check(r#"{"type":"1"}"#), Err(ObjectRejection::Unparsable));
        assert_eq!(check(r#"{"type":3}"#), Err(ObjectRejection::UnknownType));
        let long = proposal(&format!("{GOOD}\"pad\":\"{}\",", "x".repeat(400)));
        assert_eq!(check(&long), Err(ObjectRejection::InvalidProposal("size")));
    }

    #[test]
    fn duplicate_keys_read_the_first_as_univalue_does() {
        let d = proposal(&GOOD.replace(
            r#""payment_amount":12.5"#,
            r#""payment_amount":1,"payment_amount":1000"#,
        ));
        let Ok(ValidObject::Proposal(p)) = check(&d) else {
            panic!("refused")
        };
        assert_eq!(p.payment_amount, Some(100_000_000));
    }

    fn trigger(height: &str, addrs: &str, amounts: &str, hashes: &str) -> String {
        format!(
            r#"{{"event_block_height":{height},"payment_addresses":"{addrs}","payment_amounts":"{amounts}","proposal_hashes":"{hashes}","type":2}}"#
        )
    }

    #[test]
    fn triggers_follow_the_superblock_constructor() {
        let two_addrs = format!("{ADDR}|{ADDR}");
        let two_hashes = format!("{HASH}|{HASH}");
        let ok = check(&trigger("4224", &two_addrs, "1.5|2", &two_hashes)).unwrap();
        assert_eq!(
            ok,
            ValidObject::Trigger(TriggerData {
                event_block_height: 4224,
                proposal_hashes: vec![HASH.into(), HASH.into()],
            })
        );
        let bad = |d: String, field| {
            assert_eq!(
                check(&d),
                Err(ObjectRejection::InvalidTrigger(field)),
                "{d}"
            )
        };
        bad(trigger("4224", ADDR, "1.5|2", HASH), "payment_count");
        bad(trigger("4224", "", "", ""), "payment_addresses");
        bad(trigger("4224", ADDR, ".5", HASH), "payment_amounts");
        bad(trigger("4224", ADDR, "1", &HASH[1..]), "proposal_hashes");
        bad(trigger("\"4224\"", ADDR, "1", HASH), "event_block_height");
        bad(trigger("-1", ADDR, "1", HASH), "event_block_height");

        // Created more than two cycles ago or an hour ahead.
        let p = governance_params(Network::Testnet);
        let window = 2 * i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs);
        let d = trigger("4224", ADDR, "1", HASH);
        for (time, ok) in [
            (1_000_000 - window, true),
            (1_000_000 - window - 1, false),
            (1_000_000 + 3600, true),
            (1_000_000 + 3601, false),
        ] {
            let r = check_object(&object(&d, time), Network::Testnet, 1_000_000);
            assert_eq!(r.is_ok(), ok, "{time}");
        }
    }

    #[test]
    fn payment_amounts_follow_parse_fixed_point() {
        for (s, v) in [
            ("0", Some(0)),
            ("1", Some(100_000_000)),
            ("0.00000001", Some(1)),
            ("1.5000000000", Some(150_000_000)),
            ("01", None),
            ("1.", None),
            (".5", None),
            ("1.000000001", None),
            ("1.2.3", None),
            ("1e3", None),
            ("21000001", None),
            ("", None),
        ] {
            assert_eq!(parse_payment_amount(s), v, "{s}");
        }
    }
}
