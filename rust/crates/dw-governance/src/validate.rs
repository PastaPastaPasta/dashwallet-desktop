//! Checks a governance object from a peer before it is stored (MG-02 §3),
//! as Dash Core v24 `CGovernanceObject::IsValidLocally` does, minus what an
//! SPV wallet cannot check: the proposal collateral (no transaction index)
//! and the trigger's operator signature (the SML has no collateral
//! outpoints). Those are replaced by the two-peer rule of the store (MG-02
//! D3, D4).
//!
//! - The wire type picks the checks, as `IsValidLocally` does (a received
//!   object never runs `LoadData`), and the data's `type` must equal it:
//!   `ValidateProposal` wants 1, the `CSuperblock` constructor 2 (D5).
//! - Proposals: `governance::ValidateProposal(hex, fCheckExpiration=false,
//!   fAllowScript=true)`, `governance/object.cpp`.
//! - Triggers: the `CSuperblock` constructor and `ParsePaymentSchedule`
//!   (`governance/superblock.cpp`), and the time window of
//!   `CGovernanceManager::MasternodeRateCheck`.
//!
//! JSON is read as UniValue reads it: nesting up to 512 deep
//! (`MAX_JSON_DEPTH`), numbers kept as their text, the first of duplicate
//! keys wins (`UniValue::operator[]`), integers are number tokens that
//! `std::from_chars` reads whole within the type (so `-0` is 0 and `1.0` is
//! no integer), and a `VNUM` is any number token. A top-level array is the
//! legacy form `[[name, {…}]]`, whose object is the second value of the
//! first element (`CGovernanceObject::GetJSONObject`); only triggers can be
//! valid in it, since `ValidateProposal` wants an object.

use dashcore::Network;
use serde::de::{Deserialize, DeserializeOwned, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::value::RawValue;
use std::fmt;

use crate::object::{GovernanceObject, MAX_DATA_SIZE, OBJECT_TYPE_PROPOSAL, OBJECT_TYPE_TRIGGER};
use crate::params::governance_params;
use crate::proposal::{
    MAX_MONEY, ProposalData, TriggerData, amount_from_number, is_space, name_valid,
    payment_address_valid, url_valid,
};

/// How far ahead of the local clock an object may be timed (Core
/// `MAX_TIME_FUTURE_DEVIATION`, one hour).
pub const MAX_TIME_FUTURE_DEVIATION: i64 = 60 * 60;

/// The highest trigger height the wallet keeps: the wallet bounds heights
/// (MG-02 D66), far past any chain's (some 200,000 blocks a year).
pub const MAX_TRIGGER_HEIGHT: i32 = 100_000_000;

/// UniValue's `MAX_JSON_DEPTH`: containers nested deeper fail to read.
pub const MAX_JSON_DEPTH: usize = 512;

/// What a valid object's data says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidObject {
    Proposal(ProposalData),
    Trigger(TriggerData),
}

/// Why an object was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectRejection {
    /// Not JSON UniValue reads into an object.
    Unparsable,
    /// A wire type other than proposal or trigger.
    UnknownType,
    /// The data names the other object type than the wire's. The wire type
    /// is outside the hash, so this copy says nothing about the object.
    WireTypeMismatch,
    /// A rule of `ValidateProposal`; the field it failed on.
    InvalidProposal(&'static str),
    /// A rule of the `CSuperblock` constructor; the field it failed on.
    InvalidTrigger(&'static str),
    /// A trigger created more than an hour ahead or two cycles ago: Core's
    /// rate check ignores it without a penalty.
    TriggerTimeOutOfRange,
    /// A trigger for a height above [`MAX_TRIGGER_HEIGHT`] (D66).
    TriggerHeightOutOfRange,
    /// A proposal whose end epoch is past: the wallet keeps no lapsed
    /// proposal (D65).
    ProposalExpired,
    /// Not a rule: the store holds as many unconfirmed objects as it keeps
    /// (from this peer or in all).
    StoreFull,
}

impl ObjectRejection {
    /// Whether Core's rules refuse the object outright (the time window,
    /// the wallet's own bounds and the store's do not).
    pub fn is_invalid(self) -> bool {
        !matches!(
            self,
            Self::TriggerTimeOutOfRange
                | Self::TriggerHeightOutOfRange
                | Self::ProposalExpired
                | Self::StoreFull
        )
    }

    /// Whether every copy of the object, now and later, is refused the same
    /// way: the wallet remembers it so it is not fetched again.
    pub fn is_final(self) -> bool {
        matches!(self, Self::TriggerHeightOutOfRange | Self::ProposalExpired)
    }
}

/// Whether no container in `data` nests deeper than [`MAX_JSON_DEPTH`]
/// (brackets inside strings do not count). Checked before parsing, which
/// then runs without serde_json's own limit of 128.
fn depth_ok(data: &[u8]) -> bool {
    let (mut depth, mut in_string, mut escaped) = (0usize, false, false);
    for &b in data {
        if in_string {
            match (escaped, b) {
                (true, _) => escaped = false,
                (false, b'\\') => escaped = true,
                (false, b'"') => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > MAX_JSON_DEPTH {
                    return false;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    true
}

/// Parses `data` as one JSON value of type `T`, nothing after it.
fn parse<T: DeserializeOwned>(data: &[u8]) -> Option<T> {
    if !depth_ok(data) {
        return None;
    }
    let mut de = serde_json::Deserializer::from_slice(data);
    de.disable_recursion_limit();
    let v = T::deserialize(&mut de).ok()?;
    de.end().ok()?;
    Some(v)
}

/// A JSON object's members in order, duplicates kept, values as their text.
struct Members(Vec<(String, Box<RawValue>)>);

/// The values of a JSON array, or of an object in member order
/// (`UniValue::getValues`).
struct Values(Vec<Box<RawValue>>);

/// A top-level object or array.
enum Top {
    Object(Members),
    Array(Values),
}

struct MembersVisitor;

impl<'de> Visitor<'de> for MembersVisitor {
    type Value = Vec<(String, Box<RawValue>)>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut out = Vec::new();
        while let Some(entry) = map.next_entry::<String, Box<RawValue>>()? {
            out.push(entry);
        }
        Ok(out)
    }
}

fn seq_values<'de, A: SeqAccess<'de>>(mut seq: A) -> Result<Vec<Box<RawValue>>, A::Error> {
    let mut out = Vec::new();
    while let Some(v) = seq.next_element::<Box<RawValue>>()? {
        out.push(v);
    }
    Ok(out)
}

impl<'de> Deserialize<'de> for Members {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_map(MembersVisitor).map(Members)
    }
}

impl<'de> Deserialize<'de> for Values {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Values;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON array or object")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Values, A::Error> {
                seq_values(seq).map(Values)
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Values, A::Error> {
                let members = MembersVisitor.visit_map(map)?;
                Ok(Values(members.into_iter().map(|(_, v)| v).collect()))
            }
        }
        d.deserialize_any(V)
    }
}

impl<'de> Deserialize<'de> for Top {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Top;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON array or object")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Top, A::Error> {
                seq_values(seq).map(|v| Top::Array(Values(v)))
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Top, A::Error> {
                MembersVisitor
                    .visit_map(map)
                    .map(|m| Top::Object(Members(m)))
            }
        }
        d.deserialize_any(V)
    }
}

impl Members {
    /// The first value under `key`, as text.
    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.get())
    }

    /// `getInt<T>`: a number token that is all digits after an optional
    /// `-`, within `T`'s range.
    fn int<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        let t = self.get(key)?;
        let digits = t.strip_prefix('-').unwrap_or(t);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        t.parse().ok()
    }

    /// `get_str`: a JSON string.
    fn text(&self, key: &str) -> Option<String> {
        let t = self.get(key)?;
        if !t.starts_with('"') {
            return None;
        }
        serde_json::from_str(t).ok()
    }

    /// `isNum`: a number token, as its text.
    fn number(&self, key: &str) -> Option<&str> {
        self.get(key)
            .filter(|t| t.starts_with(|c: char| c == '-' || c.is_ascii_digit()))
    }
}

/// The data's object (`GetJSONObject`), and whether it was the top level.
fn json_object(data: &[u8]) -> Option<(Members, bool)> {
    match parse::<Top>(data)? {
        Top::Object(m) => Some((m, true)),
        Top::Array(Values(outer)) => {
            let Values(inner) = parse(outer.first()?.get().as_bytes())?;
            let m = parse(inner.get(1)?.get().as_bytes())?;
            Some((m, false))
        }
    }
}

/// Checks `object` as Core would before storing it from a peer, and against
/// the wallet's own bounds (D65, D66); `now` is the local clock in Unix
/// seconds. `rate_checked` applies a trigger's
/// creation-time window, which Core skips until its governance is synced
/// (`MasternodeRateCheck`).
pub fn check_object(
    object: &GovernanceObject,
    network: Network,
    now: i64,
    rate_checked: bool,
) -> Result<ValidObject, ObjectRejection> {
    let parsed = json_object(&object.data);
    let named = parsed.as_ref().and_then(|(o, _)| o.int::<i32>("type"));
    // The wire type is outside the object's hash, so any peer can change
    // it: data naming the other type was sent mistyped (D42).
    if let Some(t @ (OBJECT_TYPE_PROPOSAL | OBJECT_TYPE_TRIGGER)) = named
        && t != object.object_type
    {
        return Err(ObjectRejection::WireTypeMismatch);
    }
    let (o, top_level) = match object.object_type {
        OBJECT_TYPE_PROPOSAL | OBJECT_TYPE_TRIGGER => parsed.ok_or(ObjectRejection::Unparsable)?,
        _ => return Err(ObjectRejection::UnknownType),
    };
    if object.object_type == OBJECT_TYPE_PROPOSAL {
        if !top_level {
            return Err(ObjectRejection::InvalidProposal("json"));
        }
        if named != Some(OBJECT_TYPE_PROPOSAL) {
            return Err(ObjectRejection::InvalidProposal("type"));
        }
        let p = check_proposal(&object.data, &o, network)?;
        if p.end_epoch.is_some_and(|e| e <= now) {
            return Err(ObjectRejection::ProposalExpired);
        }
        return Ok(ValidObject::Proposal(p));
    }
    if named != Some(OBJECT_TYPE_TRIGGER) {
        return Err(ObjectRejection::InvalidTrigger("type"));
    }
    let trigger = check_trigger(&o, network)?;
    if trigger.event_block_height > MAX_TRIGGER_HEIGHT {
        return Err(ObjectRejection::TriggerHeightOutOfRange);
    }
    let p = governance_params(network);
    let cycle_secs = i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs);
    if rate_checked
        && (object.time < now - 2 * cycle_secs || object.time > now + MAX_TIME_FUTURE_DEVIATION)
    {
        return Err(ObjectRejection::TriggerTimeOutOfRange);
    }
    Ok(ValidObject::Trigger(trigger))
}

/// `governance::ValidateProposal` with scripts allowed and no expiry check.
fn check_proposal(
    data: &[u8],
    o: &Members,
    network: Network,
) -> Result<ProposalData, ObjectRejection> {
    use ObjectRejection::InvalidProposal as Bad;
    if data.len() > MAX_DATA_SIZE {
        return Err(Bad("size"));
    }
    let name = o
        .text("name")
        .filter(|n| name_valid(n))
        .ok_or(Bad("name"))?;
    let start = o.int::<i64>("start_epoch").ok_or(Bad("start_epoch"))?;
    let end = o.int::<i64>("end_epoch").ok_or(Bad("end_epoch"))?;
    if end <= start {
        return Err(Bad("end_epoch"));
    }
    // `get_real() > 0`; the duffs are read from the number's text.
    let amount = o.number("payment_amount").ok_or(Bad("payment_amount"))?;
    if !amount.parse::<f64>().is_ok_and(|a| a > 0.0) {
        return Err(Bad("payment_amount"));
    }
    let address = o
        .text("payment_address")
        .filter(|a| payment_address_valid(a, network))
        .ok_or(Bad("payment_address"))?;
    let url = o.text("url").filter(|u| url_valid(u)).ok_or(Bad("url"))?;
    Ok(ProposalData {
        name: Some(name),
        url: Some(url),
        payment_address: Some(address),
        payment_amount: amount_from_number(amount),
        payable_amount: parse_payment_amount(amount),
        start_epoch: Some(start),
        end_epoch: Some(end),
    })
}

/// The `CSuperblock` constructor: integer `event_block_height` (any `int`,
/// negative ones too: such a trigger never comes due), and the
/// `|`-separated addresses, amounts and proposal hashes in equal, non-empty
/// lists of valid entries.
fn check_trigger(o: &Members, network: Network) -> Result<TriggerData, ObjectRejection> {
    use ObjectRejection::InvalidTrigger as Bad;
    let height = o
        .int::<i32>("event_block_height")
        .ok_or(Bad("event_block_height"))?;
    let addresses = o
        .text("payment_addresses")
        .ok_or(Bad("payment_addresses"))?;
    let amounts = o.text("payment_amounts").ok_or(Bad("payment_amounts"))?;
    let hashes = o.text("proposal_hashes").ok_or(Bad("proposal_hashes"))?;
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
        // Core decodes payees with `DecodeDestination`, whose base58 skips
        // leading and trailing spaces; proposals refuse any (`ValidatePaymentAddress`).
        if !payment_address_valid(address.trim_matches(is_space), network) {
            return Err(Bad("payment_addresses"));
        }
        parse_payment_amount(amount).ok_or(Bad("payment_amounts"))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Bad("proposal_hashes"));
        }
        proposal_hashes.push(hash.to_ascii_lowercase());
    }
    Ok(TriggerData {
        event_block_height: height,
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

    /// An object whose wire type is the data's, as on every valid object.
    fn object(data: &str, time: i64) -> GovernanceObject {
        let mut o = GovernanceObject::new_proposal(1, time, data.as_bytes().to_vec());
        if data.contains(r#""type":2"#) {
            o.object_type = OBJECT_TYPE_TRIGGER;
        }
        o
    }

    fn proposal(fields: &str) -> String {
        format!(r#"{{{fields}"type":1}}"#)
    }

    const GOOD: &str = r#""name":"my-prop_1","start_epoch":100,"end_epoch":2000,"payment_amount":12.5,"payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","url":"http://x.y","#;

    fn check(data: &str) -> Result<ValidObject, ObjectRejection> {
        check_object(&object(data, 1_000), Network::Testnet, 1_000, true)
    }

    #[test]
    fn proposals_follow_validate_proposal() {
        let ok = check(&proposal(GOOD)).unwrap();
        let ValidObject::Proposal(p) = ok else {
            panic!("not a proposal")
        };
        assert_eq!(p.payment_amount, Some(1_250_000_000));
        assert_eq!(p.payable_amount, Some(1_250_000_000));
        assert_eq!(p.name.as_deref(), Some("my-prop_1"));
        // Valid (`get_real`), but Core's trigger builder cannot read the
        // amount (`ParsePaymentAmount`): never fundable.
        let ValidObject::Proposal(e) = check(&proposal(&GOOD.replace("12.5", "1.25e1"))).unwrap()
        else {
            panic!("not a proposal")
        };
        assert_eq!(
            (e.payment_amount, e.payable_amount),
            (Some(1_250_000_000), None)
        );

        // Lapsed, it is not kept (D65).
        let at = |now| check_object(&object(&proposal(GOOD), 1_000), Network::Testnet, now, true);
        assert!(at(1_999).is_ok());
        let e = at(2_000).unwrap_err();
        assert_eq!(e, ObjectRejection::ProposalExpired);
        assert!(!e.is_invalid() && e.is_final());

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
        bad(r#""end_epoch":2000"#, r#""end_epoch":100"#, "end_epoch");
        bad(r#""end_epoch":2000"#, r#""end_epoch":2000.0"#, "end_epoch");
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

        // The wire type picks the checks and the data's type must equal it
        // (D5). A copy whose data names the other type was mistyped by its
        // sender (D42): any wire type, the legacy trigger form included.
        let legacy = format!(r#"[["trigger",{}]]"#, trigger("4224", ADDR, "1", HASH));
        for (data, wire) in [
            (proposal(GOOD), OBJECT_TYPE_TRIGGER),
            (proposal(GOOD), 3),
            (trigger("4224", ADDR, "1", HASH), OBJECT_TYPE_PROPOSAL),
            (legacy, OBJECT_TYPE_PROPOSAL),
        ] {
            let mut o = object(&data, 1_000);
            o.object_type = wire;
            let r = check_object(&o, Network::Testnet, 1_000, true);
            assert_eq!(
                r,
                Err(ObjectRejection::WireTypeMismatch),
                "{data} as {wire}"
            );
            assert!(r.unwrap_err().is_invalid());
        }
        // Data naming neither type fails by the wire type's rules.
        for data in [r#"{"type":"1"}"#, r#"{"type":3}"#, "{}"] {
            assert_eq!(
                check(data),
                Err(ObjectRejection::InvalidProposal("type")),
                "{data}"
            );
            let mut t = object(data, 1_000);
            t.object_type = OBJECT_TYPE_TRIGGER;
            assert_eq!(
                check_object(&t, Network::Testnet, 1_000, true),
                Err(ObjectRejection::InvalidTrigger("type"))
            );
        }
        let mut unknown = object(r#"{"type":3}"#, 1_000);
        unknown.object_type = 3;
        assert_eq!(
            check_object(&unknown, Network::Testnet, 1_000, true),
            Err(ObjectRejection::UnknownType)
        );
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
        // Core's base58 skips spaces around a payee, not inside it.
        let spaced = format!("  {ADDR} |{ADDR}");
        assert!(check(&trigger("4224", &spaced, "1.5|2", &two_hashes)).is_ok());
        bad(
            trigger(
                "4224",
                &format!("{} {}", &ADDR[..10], &ADDR[10..]),
                "1",
                HASH,
            ),
            "payment_addresses",
        );
        bad(trigger("4224", ADDR, ".5", HASH), "payment_amounts");
        bad(trigger("4224", ADDR, "1", &HASH[1..]), "proposal_hashes");
        bad(trigger("\"4224\"", ADDR, "1", HASH), "event_block_height");
        bad(trigger("4224.0", ADDR, "1", HASH), "event_block_height");
        bad(trigger("2147483648", ADDR, "1", HASH), "event_block_height");
        // Any int, as `getInt<int>`: a negative height is relayed by Core.
        assert!(matches!(
            check(&trigger("-1", ADDR, "1", HASH)),
            Ok(ValidObject::Trigger(TriggerData {
                event_block_height: -1,
                ..
            }))
        ));

        // The wallet bounds heights (D66): refused for good, with no penalty.
        let top = MAX_TRIGGER_HEIGHT.to_string();
        assert!(check(&trigger(&top, ADDR, "1", HASH)).is_ok());
        let over = (MAX_TRIGGER_HEIGHT + 1).to_string();
        let e = check(&trigger(&over, ADDR, "1", HASH)).unwrap_err();
        assert_eq!(e, ObjectRejection::TriggerHeightOutOfRange);
        assert!(!e.is_invalid() && e.is_final());
        let r = check_object(
            &object(&trigger(&over, ADDR, "1", HASH), 1_000),
            Network::Testnet,
            1_000,
            false,
        );
        assert_eq!(r, Err(ObjectRejection::TriggerHeightOutOfRange));

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
            let r = check_object(&object(&d, time), Network::Testnet, 1_000_000, true);
            assert_eq!(r.is_ok(), ok, "{time}");
            // Unsynced, Core skips the window.
            assert!(check_object(&object(&d, time), Network::Testnet, 1_000_000, false).is_ok());
            // Core's rate check ignores such an object without a penalty.
            if let Err(e) = r {
                assert_eq!(e, ObjectRejection::TriggerTimeOutOfRange);
                assert!(!e.is_invalid() && !e.is_final());
            }
        }
    }

    #[test]
    fn the_legacy_array_form_holds_triggers_only() {
        let t = trigger("4224", ADDR, "1", HASH);
        let legacy = format!(r#"[["trigger",{t}]]"#);
        assert!(matches!(check(&legacy), Ok(ValidObject::Trigger(_))));
        // The first element's values may come from an object too.
        let legacy = format!(r#"[{{"a":"trigger","b":{t}}}]"#);
        assert!(matches!(check(&legacy), Ok(ValidObject::Trigger(_))));
        // A proposal in it has its type, but `ValidateProposal` wants an
        // object.
        let p = proposal(GOOD);
        assert_eq!(
            check(&format!(r#"[["proposal",{p}]]"#)),
            Err(ObjectRejection::InvalidProposal("json"))
        );
        for unparsable in [r#"[]"#, r#"[["trigger"]]"#, r#"[[1,2]]"#, r#"[5]"#, "5"] {
            assert_eq!(
                check(unparsable),
                Err(ObjectRejection::Unparsable),
                "{unparsable}"
            );
        }
    }

    #[test]
    fn integers_are_read_from_their_text_as_from_chars_does() {
        // `-0` is the integer 0; serde_json alone would make it a float.
        let d = proposal(&GOOD.replace(r#""start_epoch":100"#, r#""start_epoch":-0"#));
        let Ok(ValidObject::Proposal(p)) = check(&d) else {
            panic!("refused: {d}")
        };
        assert_eq!(p.start_epoch, Some(0));
        let refused = proposal(&GOOD.replace(r#""start_epoch":100"#, r#""start_epoch":1e2"#));
        assert_eq!(
            check(&refused),
            Err(ObjectRejection::InvalidProposal("start_epoch"))
        );
        // An exponent is still a number for the amount.
        let d = proposal(&GOOD.replace("12.5", "1.25e1"));
        let Ok(ValidObject::Proposal(p)) = check(&d) else {
            panic!("refused: {d}")
        };
        assert_eq!(p.payment_amount, Some(1_250_000_000));
    }

    /// A trigger with an extra member nested so that the deepest container
    /// is `depth` deep, counting the top-level object.
    fn nested_trigger(depth: usize) -> String {
        let t = trigger("4224", ADDR, "1", HASH);
        let inner = depth - 1;
        format!(
            r#"{{"deep":{}{},{}"#,
            "[".repeat(inner),
            "]".repeat(inner),
            &t[1..]
        )
    }

    #[test]
    fn nesting_is_bounded_at_univalues_depth() {
        assert!(check(&nested_trigger(MAX_JSON_DEPTH)).is_ok());
        assert_eq!(
            check(&nested_trigger(MAX_JSON_DEPTH + 1)),
            Err(ObjectRejection::Unparsable)
        );
        // Brackets in strings do not nest.
        let quoted = trigger("4224", ADDR, "1", HASH).replacen(
            '{',
            &format!(r#"{{"s":"{}\"{}","#, "[".repeat(600), "{"),
            1,
        );
        assert!(check(&quoted).is_ok(), "{quoted}");
    }

    /// The parse runs without serde_json's recursion limit, so its stack
    /// use grows with the nesting `depth_ok` allows: a 512-deep object
    /// parses on a 256 KiB stack, an eighth of a tokio worker's 2 MiB.
    #[test]
    fn the_deepest_object_parses_on_a_small_stack() {
        const STACK_BOUND: usize = 256 * 1024;
        let data = nested_trigger(MAX_JSON_DEPTH);
        let ok = std::thread::Builder::new()
            .stack_size(STACK_BOUND)
            .spawn(move || check(&data).is_ok())
            .unwrap()
            .join()
            .unwrap();
        assert!(ok);
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
