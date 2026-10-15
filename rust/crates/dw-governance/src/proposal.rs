//! Proposals: the Create Proposal wizard's validation, the data JSON with
//! dash-qt's key order, the honoured payment date, and reading the proposal
//! and trigger JSON of synced objects (QT-129, QT-130, QT-132).
//!
//! Sources: Core `governance/object.cpp` (`ValidateProposal` and its
//! helpers), dash-qt `proposalcreate.cpp` (`buildJsonAndHex`), dash-qt
//! `proposalmodel.cpp` (`Proposal`, `paymentsRequested`).

use std::str::FromStr;

use dashcore::address::Payload;
use dashcore::{Address, Network};

use crate::clock::{estimated_time, upcoming_superblocks};
use crate::object::{GovernanceObject, MAX_DATA_SIZE, OBJECT_TYPE_PROPOSAL};
use crate::params::{
    GovernanceParams, PROPOSAL_MAX_PAYMENTS, PROPOSAL_NAME_MAX_LEN, governance_params,
};

const COIN: u64 = 100_000_000;
/// Dash Core `MAX_MONEY`.
const MAX_MONEY: u64 = 21_000_000 * COIN;

/// The proposal field a validation failure concerns, in wizard order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Field {
    Name,
    Url,
    PaymentAddress,
    PaymentAmount,
    PaymentCount,
    FirstPayment,
    Payload,
}

/// The Create Proposal wizard's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub url: String,
    pub payment_address: String,
    /// Per payment, duffs.
    pub payment_amount: u64,
    pub payment_count: u32,
    pub first_superblock_height: u32,
}

/// Where the chain is: the tip and the time its ETAs count from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainPoint {
    pub height: u32,
    /// UNIX seconds the tip stands for (the later of its block time and
    /// now, so ETAs do not lie in the past while SPV catches up).
    pub time: u64,
}

/// How many superblocks the wizard offers as the first payment.
pub const PAYMENT_DATE_CHOICES: u32 = 12;

/// Core `FormatMoney`: at least two decimals, trailing zeros trimmed.
pub fn format_money(duffs: u64) -> String {
    let mut s = format!("{}.{:08}", duffs / COIN, duffs % COIN);
    while s.ends_with('0') && s.len() - s.find('.').unwrap_or(0) > 3 {
        s.pop();
    }
    s
}

/// Core's `IsSpace`.
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// Core `CheckURL` (sentinel's `urlparse` acceptance): a netloc with an
/// unbalanced `[` / `]` is refused.
fn check_url(url: &str) -> bool {
    let rest = match url.find(':') {
        Some(pos) => &url[pos + 1..],
        None => url,
    };
    if rest.len() > 2 && rest.starts_with("//") {
        let after = &rest[2..];
        let netloc = match after.find(['/', '?', '#']) {
            Some(end) => &after[..end],
            None => after,
        };
        if netloc.contains('[') != netloc.contains(']') {
            return false;
        }
    }
    true
}

/// Core `ValidateName`: 1–40 bytes of `[-_a-z0-9]`, case-insensitive.
pub fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= PROPOSAL_NAME_MAX_LEN
        && name
            .to_ascii_lowercase()
            .bytes()
            .all(|b| matches!(b, b'-' | b'_' | b'a'..=b'z' | b'0'..=b'9'))
}

/// Core `ValidateURL`: no whitespace, at least 4 characters, `CheckURL`.
pub fn url_valid(url: &str) -> bool {
    !url.chars().any(is_space) && url.len() >= 4 && check_url(url)
}

/// A P2PKH or P2SH address of `network` (Core `ValidatePaymentAddress`
/// with scripts allowed, as dash-qt's wizard).
pub fn payment_address_valid(address: &str, network: Network) -> bool {
    if address.chars().any(is_space) {
        return false;
    }
    let Ok(parsed) = Address::from_str(address) else {
        return false;
    };
    let Ok(checked) = parsed.require_network(network) else {
        return false;
    };
    matches!(
        checked.payload(),
        Payload::PubkeyHash(_) | Payload::ScriptHash(_)
    )
}

/// The proposal's start and end epochs for its chosen first superblock
/// (dash-qt ignores the choice and counts from "now"; the engine honours
/// it): half a cycle before the first payment's superblock and half a
/// cycle after the last one's, in estimated wall time.
pub fn epochs(p: &GovernanceParams, draft: &Draft, at: ChainPoint) -> (i64, i64) {
    let cycle_secs = i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs);
    let first = draft.first_superblock_height;
    let last = first + draft.payment_count.saturating_sub(1) * p.superblock_cycle;
    let start = estimated_time(p, at.height, at.time, first) as i64 - cycle_secs / 2;
    let end = estimated_time(p, at.height, at.time, last) as i64 + cycle_secs / 2;
    (start, end)
}

/// The data JSON in dash-qt's key order
/// (`name, payment_address, payment_amount, url, start_epoch, end_epoch,
/// type`), written compactly as UniValue does.
pub fn proposal_json(network: Network, draft: &Draft, at: ChainPoint) -> String {
    let p = governance_params(network);
    let (start, end) = epochs(&p, draft, at);
    let s = |v: &str| serde_json::to_string(v).expect("a string always serializes");
    let mut out = String::with_capacity(256);
    out.push('{');
    out.push_str(&format!("\"name\":{},", s(&draft.name)));
    out.push_str(&format!(
        "\"payment_address\":{},",
        s(&draft.payment_address)
    ));
    out.push_str(&format!(
        "\"payment_amount\":{},",
        format_money(draft.payment_amount)
    ));
    out.push_str(&format!("\"url\":{},", s(&draft.url)));
    if start > 0 {
        out.push_str(&format!("\"start_epoch\":{start},"));
    }
    if end > 0 {
        out.push_str(&format!("\"end_epoch\":{end},"));
    }
    out.push_str(&format!("\"type\":{OBJECT_TYPE_PROPOSAL}"));
    out.push('}');
    out
}

/// Every failing field in wizard order; empty = valid.
pub fn validate(network: Network, draft: &Draft, at: ChainPoint) -> Vec<Field> {
    let p = governance_params(network);
    let mut bad = Vec::new();
    if !name_valid(&draft.name) {
        bad.push(Field::Name);
    }
    if !url_valid(&draft.url) {
        bad.push(Field::Url);
    }
    if !payment_address_valid(&draft.payment_address, network) {
        bad.push(Field::PaymentAddress);
    }
    if draft.payment_amount == 0 || draft.payment_amount > MAX_MONEY {
        bad.push(Field::PaymentAmount);
    }
    if !(1..=PROPOSAL_MAX_PAYMENTS).contains(&draft.payment_count) {
        bad.push(Field::PaymentCount);
    }
    if !upcoming_superblocks(&p, at.height, PAYMENT_DATE_CHOICES)
        .contains(&draft.first_superblock_height)
    {
        bad.push(Field::FirstPayment);
    }
    if proposal_json(network, draft, at).len() > MAX_DATA_SIZE {
        bad.push(Field::Payload);
    }
    bad
}

/// The unsigned proposal object for `draft` created at `time`.
pub fn proposal_object(
    network: Network,
    draft: &Draft,
    at: ChainPoint,
    time: i64,
) -> GovernanceObject {
    GovernanceObject::new_proposal(1, time, proposal_json(network, draft, at).into_bytes())
}

/// Parses a decimal DASH amount with at most 8 decimals into duffs (Core
/// `ParsePaymentAmount` on the JSON number's text).
pub fn parse_amount(text: &str) -> Option<u64> {
    let text = text.trim();
    let (whole, frac) = match text.split_once('.') {
        Some((w, f)) => (w, f),
        None => (text, ""),
    };
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    if frac.len() > 8 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let w: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    let f: u64 = format!("{frac:0<8}").parse().ok()?;
    let v = w.checked_mul(COIN)?.checked_add(f)?;
    (v <= MAX_MONEY).then_some(v)
}

/// The fields of a synced proposal's data JSON. Missing or mistyped fields
/// are `None` (the list shows what is there, as dash-qt does).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProposalData {
    pub name: Option<String>,
    pub url: Option<String>,
    pub payment_address: Option<String>,
    /// Duffs.
    pub payment_amount: Option<u64>,
    pub start_epoch: Option<i64>,
    pub end_epoch: Option<i64>,
}

impl ProposalData {
    /// dash-qt `Proposal::paymentsRequested`: `ceil((end − start) / cycle
    /// time)`, at least 1.
    pub fn payments_requested(&self, p: &GovernanceParams) -> u32 {
        let (Some(start), Some(end)) = (self.start_epoch, self.end_epoch) else {
            return 1;
        };
        let cycle = i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs);
        let duration = end - start;
        ((duration + cycle - 1) / cycle).max(1) as u32
    }
}

fn json_amount(v: &serde_json::Value) -> Option<u64> {
    match v {
        serde_json::Value::Number(n) => parse_amount(&n.to_string()).or_else(|| {
            let f = n.as_f64()?;
            (f > 0.0 && f * (COIN as f64) < MAX_MONEY as f64)
                .then(|| (f * COIN as f64).round() as u64)
        }),
        serde_json::Value::String(s) => parse_amount(s),
        _ => None,
    }
}

/// Reads a proposal object's data JSON.
pub fn parse_proposal(data: &[u8]) -> Option<ProposalData> {
    let v: serde_json::Value = serde_json::from_slice(data).ok()?;
    let o = v.as_object()?;
    let text = |k: &str| o.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let int = |k: &str| o.get(k).and_then(|x| x.as_i64());
    Some(ProposalData {
        name: text("name"),
        url: text("url"),
        payment_address: text("payment_address"),
        payment_amount: o.get("payment_amount").and_then(json_amount),
        start_epoch: int("start_epoch"),
        end_epoch: int("end_epoch"),
    })
}

/// A superblock trigger (object type 2): the height it pays at and the
/// proposals it pays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerData {
    pub event_block_height: u32,
    /// Display-order hashes.
    pub proposal_hashes: Vec<String>,
}

/// Reads a trigger object's data JSON.
pub fn parse_trigger(data: &[u8]) -> Option<TriggerData> {
    let v: serde_json::Value = serde_json::from_slice(data).ok()?;
    let o = v.as_object()?;
    let height = o.get("event_block_height")?.as_u64()?;
    let hashes = o
        .get("proposal_hashes")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .split('|')
        .filter(|h| !h.is_empty())
        .map(|h| h.to_ascii_lowercase())
        .collect();
    Some(TriggerData {
        event_block_height: u32::try_from(height).ok()?,
        proposal_hashes: hashes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n";

    fn draft() -> Draft {
        Draft {
            name: "my-proposal_1".into(),
            url: "https://dash.org/p".into(),
            payment_address: ADDR.into(),
            payment_amount: 1_250_000_000,
            payment_count: 2,
            first_superblock_height: 1520,
        }
    }

    const AT: ChainPoint = ChainPoint {
        height: 1505,
        time: 1_800_000_000,
    };

    #[test]
    fn test_qt_132_format_money_matches_core() {
        assert_eq!(format_money(100_000_000), "1.00");
        assert_eq!(format_money(1_250_000_000), "12.50");
        assert_eq!(format_money(12_345_678), "0.12345678");
        assert_eq!(format_money(10), "0.0000001");
        assert_eq!(format_money(0), "0.00");
    }

    #[test]
    fn test_qt_132_json_key_order_and_honoured_date() {
        let json = proposal_json(Network::Regtest, &draft(), AT);
        // Regtest: cycle 20 × 150 s = 3000 s; superblock 1520 is 15 blocks
        // ahead (2250 s), the last payment (1540) 35 blocks (5250 s).
        let start = 1_800_000_000 + 2250 - 1500;
        let end = 1_800_000_000 + 5250 + 1500;
        assert_eq!(
            json,
            format!(
                "{{\"name\":\"my-proposal_1\",\"payment_address\":\"{ADDR}\",\"payment_amount\":12.50,\"url\":\"https://dash.org/p\",\"start_epoch\":{start},\"end_epoch\":{end},\"type\":1}}"
            )
        );
        let data = parse_proposal(json.as_bytes()).unwrap();
        assert_eq!(data.payment_amount, Some(1_250_000_000));
        let p = governance_params(Network::Regtest);
        assert_eq!(data.payments_requested(&p), 2);
    }

    #[test]
    fn test_qt_132_validation_lists_failing_fields_in_order() {
        assert!(validate(Network::Regtest, &draft(), AT).is_empty());
        let mut d = draft();
        d.name = "Bad Name".into();
        d.url = "a b".into();
        d.payment_address = "XnotTestnet".into();
        d.payment_amount = 0;
        d.payment_count = 13;
        d.first_superblock_height = 1530;
        assert_eq!(
            validate(Network::Regtest, &d, AT),
            vec![
                Field::Name,
                Field::Url,
                Field::PaymentAddress,
                Field::PaymentAmount,
                Field::PaymentCount,
                Field::FirstPayment
            ]
        );
        // Upper case is allowed (Core lower-cases before checking).
        d = draft();
        d.name = "UPPER".into();
        assert!(validate(Network::Regtest, &d, AT).is_empty());
        d.name = "x".repeat(41);
        assert_eq!(validate(Network::Regtest, &d, AT), vec![Field::Name]);
        d = draft();
        d.url = format!("https://{}", "a".repeat(600));
        assert_eq!(validate(Network::Regtest, &d, AT), vec![Field::Payload]);
        d.url = "http://[::1/x".into();
        assert_eq!(validate(Network::Regtest, &d, AT), vec![Field::Url]);
    }

    #[test]
    fn parse_amount_is_exact() {
        assert_eq!(parse_amount("12.5"), Some(1_250_000_000));
        assert_eq!(parse_amount("0.00000001"), Some(1));
        assert_eq!(parse_amount("1.000000001"), None);
        assert_eq!(parse_amount("-1"), None);
        assert_eq!(parse_amount("abc"), None);
    }

    #[test]
    fn parses_triggers() {
        let t = parse_trigger(br#"{"event_block_height":1520,"payment_addresses":"a|b","payment_amounts":"1|2","proposal_hashes":"AB|cd","type":2}"#).unwrap();
        assert_eq!(t.event_block_height, 1520);
        assert_eq!(t.proposal_hashes, vec!["ab", "cd"]);
    }
}
