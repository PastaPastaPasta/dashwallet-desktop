//! dash-qt's session envelope (`dash-shared-mn-session`, MG-04b §2.3): the
//! whole state of one shared registration, passed between its parties
//! (`MnShareSession`, rc.3 `src/qt/mnsharesession.{h,cpp}`).
//!
//! [`Session::to_json`] writes `toJson`'s keys in its order
//! (`mnsharesession.cpp:276-349`); [`Session::from_json`] reads as
//! `fromJson` does (`:371-664`), in its order, plus the wallet's stricter
//! rules (D28). Past `draft` the frozen transaction is checked as dash-qt
//! does on import ([`funding`]); comparing the copy with a held session is
//! the merge's (§3.0).

use dashcore::Network;

use super::envelope::{EnvelopeError, append_fingerprint, fingerprint, network_id};
use super::json::Json;
use crate::params::MAX_SHARES;

pub mod funding;
pub mod merge;

/// `ENVELOPE_TYPE` (`mnsharesession.cpp:35`).
pub const SESSION_TYPE: &str = "dash-shared-mn-session";
/// `ENVELOPE_VERSION` (`mnsharesession.cpp:36`).
pub const SESSION_VERSION: i32 = 1;
/// `MAX_CONTRIBUTION_INPUTS` (`mnsharesession.cpp:41`).
pub const MAX_CONTRIBUTION_INPUTS: usize = 64;
/// `MAX_MONEY` (rc.3 `consensus/amount.h`).
pub const MAX_MONEY: i64 = 21_000_000 * 100_000_000;

fn money_range(amount: i64) -> bool {
    (0..=MAX_MONEY).contains(&amount)
}

/// `MnShareSession::Stage`, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    Draft,
    Frozen,
    Signing,
    Combined,
    FundingSigned,
    Broadcast,
}

impl Stage {
    pub const ALL: [Stage; 6] = [
        Self::Draft,
        Self::Frozen,
        Self::Signing,
        Self::Combined,
        Self::FundingSigned,
        Self::Broadcast,
    ];

    /// `StageKey` (`mnsharesession.cpp:60-71`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Frozen => "frozen",
            Self::Signing => "signing",
            Self::Combined => "combined",
            Self::FundingSigned => "fundingSigned",
            Self::Broadcast => "broadcast",
        }
    }

    /// `StageFromKey`: exact.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

/// A row of the share table; `""` for an address not given yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Share {
    pub label: String,
    /// Duffs (`CAmount`).
    pub amount: i64,
    /// P2PKH.
    pub owner_address: String,
    /// P2PKH or P2SH.
    pub refund_address: String,
    /// Empty: rewards go to the refund address.
    pub reward_address: String,
}

impl Share {
    /// `SameShare`: everything but the label (`mnsharesession.cpp:100-104`).
    pub fn same(&self, other: &Self) -> bool {
        self.amount == other.amount
            && self.owner_address == other.owner_address
            && self.refund_address == other.refund_address
            && self.reward_address == other.reward_address
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// 64 hex, as received (dash-qt compares it case-insensitively).
    pub txid: String,
    pub vout: u32,
    pub sequence: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub address: String,
    pub amount: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    /// The share it funds: the first share with this label.
    pub label: String,
    pub inputs: Vec<Input>,
    pub change: Option<Change>,
    /// The change's output in the funding transaction, or -1.
    pub change_index: i32,
}

impl Contribution {
    /// `SameContribution` (`mnsharesession.cpp:106-118`): the label and
    /// `changeIndex` aside, txids compared ignoring case.
    pub fn same(&self, other: &Self) -> bool {
        self.change == other.change
            && self.inputs.len() == other.inputs.len()
            && self.inputs.iter().zip(&other.inputs).all(|(a, b)| {
                a.txid.eq_ignore_ascii_case(&b.txid) && a.vout == b.vout && a.sequence == b.sequence
            })
    }
}

/// The masternode-wide terms. Compared as exact strings and integers
/// (`sameTerms`, `mnsharesession.cpp:1098-1106`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Terms {
    /// `ip:port` entries joined by `,`, or empty.
    pub core_p2p_addrs: String,
    /// Basic-scheme BLS public key, hex.
    pub operator_pub_key: String,
    /// P2PKH.
    pub voting_address: String,
    /// Hundredths of a percent; read as `getInt<int>`, range-checked later.
    pub operator_reward: i32,
    pub early_period_blocks: u32,
    pub early_penalty: i64,
}

/// One consent signature as the envelope carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature {
    pub share_index: i32,
    /// Base64 of 65 bytes, as received.
    pub signature: String,
}

/// `MnShareSession`'s envelope state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub network: Network,
    /// 64 lower-case hex (D28).
    pub session_id: String,
    pub revision: i32,
    pub stage: Stage,
    /// Hex of the funding template; never trusted on import (D8).
    pub funding_tx: String,
    pub shares: Vec<Share>,
    pub terms: Terms,
    pub contributions: Vec<Contribution>,
    /// Hex of the shared ProRegTx, `""` in `draft`.
    pub protx: String,
    /// Lower-case display hex, or `""`.
    pub consent_hash: String,
    pub collateral_index: i32,
    /// In the order received; ascending as this wallet adds them.
    pub sigs: Vec<Signature>,
    /// The wallet writes `""` (D14).
    pub prepare_wallet: String,
    pub operator_secret_holder: String,
    pub coordinator_label: String,
}

impl Session {
    /// A new draft: revision 1, no shares (`MnShareSession()`). The id is the
    /// caller's `GetRandHash().ToString()`.
    pub fn new(network: Network, session_id: String) -> Self {
        Self {
            network,
            session_id,
            revision: 1,
            stage: Stage::Draft,
            funding_tx: String::new(),
            shares: Vec::new(),
            terms: Terms::default(),
            contributions: Vec::new(),
            protx: String::new(),
            consent_hash: String::new(),
            collateral_index: -1,
            sigs: Vec::new(),
            prepare_wallet: String::new(),
            operator_secret_holder: String::new(),
            coordinator_label: String::new(),
        }
    }

    /// `sessionCode()`: the first six characters of the id, upper-cased.
    pub fn session_code(&self) -> String {
        self.session_id
            .chars()
            .take(6)
            .collect::<String>()
            .to_ascii_uppercase()
    }

    /// `fingerprint()`: the message code of [`Self::to_json`].
    pub fn message_code(&self) -> String {
        fingerprint(&self.to_json_body())
    }

    /// The index of the first share named `label` (`ShareIndexOfLabel`).
    pub fn share_index_of(&self, label: &str) -> Option<usize> {
        self.shares.iter().position(|s| s.label == label)
    }

    /// The signature recorded for a share (`findSignature`).
    pub fn signature_for(&self, share_index: i32) -> Option<&Signature> {
        self.sigs.iter().find(|s| s.share_index == share_index)
    }

    /// `signedCount()`: signatures whose index is a share.
    pub fn signed_count(&self) -> usize {
        self.sigs
            .iter()
            .filter(|s| usize::try_from(s.share_index).is_ok_and(|i| i < self.shares.len()))
            .count()
    }

    /// `sameTerms`.
    pub fn same_terms(&self, other: &Self) -> bool {
        self.terms == other.terms
    }

    /// MG-04b D28: share labels non-empty and unique after trimming, every
    /// contribution naming a share, at most one contribution per share.
    pub fn check_labels(&self) -> Result<(), EnvelopeError> {
        let mut seen = std::collections::BTreeSet::new();
        for share in &self.shares {
            let label = share.label.trim();
            if label.is_empty() || !seen.insert(label) {
                return Err(EnvelopeError::Labels);
            }
        }
        let mut funded = std::collections::BTreeSet::new();
        for c in &self.contributions {
            match self.share_index_of(&c.label) {
                Some(i) if funded.insert(i) => {}
                _ => return Err(EnvelopeError::Labels),
            }
        }
        Ok(())
    }

    /// `toJsonBody`: every key but the code.
    fn to_json_body(&self) -> Json {
        let s = |v: &str| Json::str(v);
        let mut ret = Json::obj();
        ret.push_kv("type", s(SESSION_TYPE));
        ret.push_kv("version", Json::int(SESSION_VERSION.into()));
        ret.push_kv("network", s(network_id(self.network)));
        ret.push_kv("sessionId", s(&self.session_id));
        ret.push_kv("revision", Json::int(self.revision.into()));
        ret.push_kv("stage", s(self.stage.key()));
        ret.push_kv("fundingTx", s(&self.funding_tx));

        let mut shares = Json::Arr(Vec::new());
        for share in &self.shares {
            let mut entry = Json::obj();
            entry.push_kv("label", s(&share.label));
            entry.push_kv("amount", Json::int(share.amount));
            entry.push_kv("ownerAddress", s(&share.owner_address));
            entry.push_kv("refundAddress", s(&share.refund_address));
            entry.push_kv("rewardAddress", s(&share.reward_address));
            shares.push(entry);
        }
        ret.push_kv("shares", shares);

        let t = &self.terms;
        let mut terms = Json::obj();
        terms.push_kv("coreP2PAddrs", s(&t.core_p2p_addrs));
        terms.push_kv("operatorPubKey", s(&t.operator_pub_key));
        terms.push_kv("votingAddress", s(&t.voting_address));
        terms.push_kv("operatorReward", Json::int(t.operator_reward.into()));
        terms.push_kv("earlyPeriodBlocks", Json::int(t.early_period_blocks.into()));
        terms.push_kv("earlyPenalty", Json::int(t.early_penalty));
        ret.push_kv("terms", terms);

        let mut contributions = Json::Arr(Vec::new());
        for c in &self.contributions {
            let mut entry = Json::obj();
            entry.push_kv("label", s(&c.label));
            let mut inputs = Json::Arr(Vec::new());
            for input in &c.inputs {
                let mut i = Json::obj();
                i.push_kv("txid", s(&input.txid));
                i.push_kv("vout", Json::int(input.vout.into()));
                i.push_kv("sequence", Json::int(input.sequence.into()));
                inputs.push(i);
            }
            entry.push_kv("inputs", inputs);
            if let Some(change) = &c.change {
                let mut ch = Json::obj();
                ch.push_kv("address", s(&change.address));
                ch.push_kv("amount", Json::int(change.amount));
                entry.push_kv("change", ch);
            }
            entry.push_kv("changeIndex", Json::int(c.change_index.into()));
            contributions.push(entry);
        }
        ret.push_kv("contributions", contributions);

        ret.push_kv("protx", s(&self.protx));
        ret.push_kv("consentHash", s(&self.consent_hash));
        ret.push_kv("collateralIndex", Json::int(self.collateral_index.into()));
        let mut sigs = Json::Arr(Vec::new());
        for sig in &self.sigs {
            let mut entry = Json::obj();
            entry.push_kv("shareIndex", Json::int(sig.share_index.into()));
            entry.push_kv("signature", s(&sig.signature));
            sigs.push(entry);
        }
        ret.push_kv("sigs", sigs);
        ret.push_kv("prepareWallet", s(&self.prepare_wallet));
        ret.push_kv("operatorSecretHolder", s(&self.operator_secret_holder));
        ret.push_kv("coordinatorLabel", s(&self.coordinator_label));
        ret
    }

    /// `toJson`: the body with its code appended last.
    pub fn to_json(&self) -> Json {
        let mut json = self.to_json_body();
        append_fingerprint(&mut json);
        json
    }

    /// `toJsonString`: `write(2)`.
    pub fn to_text(&self) -> String {
        String::from_utf8(self.to_json().write(2)).expect("the writer emits UTF-8")
    }

    /// `fromJson`, in dash-qt's order. Where dash-qt's `getInt` would throw
    /// ("could not be parsed") the error is [`EnvelopeError::Parse`] naming
    /// the field. The wallet adds: a `sessionId` of 64 lower-case hex, UTF-8
    /// strings (dash-qt would substitute U+FFFD), and D28's labels.
    pub fn from_json(json: &Json, network: Network) -> Result<Self, EnvelopeError> {
        use EnvelopeError::Parse;
        if !json.is_object() || json.find("type").and_then(Json::as_text) != Some(SESSION_TYPE) {
            return Err(EnvelopeError::NotShared);
        }
        // `!isNum() || getInt<int>() != 1`; a non-integer throws.
        if !json.find("version").is_some_and(Json::is_num) {
            return Err(Parse("version"));
        }
        if json.find("version").and_then(Json::get_i32) != Some(SESSION_VERSION) {
            return Err(Parse("version"));
        }
        if json.find("network").and_then(Json::as_text) != Some(network_id(network)) {
            return Err(EnvelopeError::Network);
        }
        let session_id = text(json.find("sessionId")).ok_or(Parse("sessionId"))?;
        if !(session_id.len() == 64
            && session_id
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        {
            return Err(Parse("sessionId"));
        }
        let revision = int(json.find("revision"), Json::get_i32, "revision")?
            .filter(|r| *r >= 1)
            .ok_or(Parse("revision"))?;
        let stage = text(json.find("stage"))
            .ok_or(Parse("stage"))
            .and_then(|s| Stage::from_key(&s).ok_or(Parse("stage")))?;

        let mut parsed = Session::new(network, session_id);
        parsed.revision = revision;
        parsed.stage = stage;
        parsed.funding_tx = opt_text(json, "fundingTx")?.unwrap_or_default();

        if let Some(shares) = json.find("shares").and_then(Json::as_array) {
            if shares.len() > MAX_SHARES as usize {
                return Err(Parse("shares"));
            }
            for entry in shares {
                let well_formed = entry.is_object()
                    && entry.find("amount").is_some_and(Json::is_num)
                    && entry.find("ownerAddress").is_some_and(Json::is_str)
                    && entry.find("refundAddress").is_some_and(Json::is_str);
                if !well_formed {
                    return Err(Parse("shares"));
                }
                let amount = entry
                    .find("amount")
                    .and_then(Json::get_i64)
                    .filter(|a| money_range(*a))
                    .ok_or(Parse("shares"))?;
                parsed.shares.push(Share {
                    label: opt_text(entry, "label")?.unwrap_or_default(),
                    amount,
                    owner_address: opt_text(entry, "ownerAddress")?.unwrap_or_default(),
                    refund_address: opt_text(entry, "refundAddress")?.unwrap_or_default(),
                    reward_address: opt_text(entry, "rewardAddress")?.unwrap_or_default(),
                });
            }
        }

        if let Some(terms) = json.find("terms").filter(|t| t.is_object()) {
            let t = &mut parsed.terms;
            t.core_p2p_addrs = opt_text(terms, "coreP2PAddrs")?.unwrap_or_default();
            t.operator_pub_key = opt_text(terms, "operatorPubKey")?.unwrap_or_default();
            t.voting_address = opt_text(terms, "votingAddress")?.unwrap_or_default();
            if let Some(r) = int(terms.find("operatorReward"), Json::get_i32, "terms")? {
                t.operator_reward = r;
            }
            if let Some(b) = int(terms.find("earlyPeriodBlocks"), Json::get_i64, "terms")? {
                t.early_period_blocks = u32::try_from(b).map_err(|_| Parse("terms"))?;
            }
            if let Some(p) = int(terms.find("earlyPenalty"), Json::get_i64, "terms")? {
                t.early_penalty = Some(p).filter(|p| money_range(*p)).ok_or(Parse("terms"))?;
            }
        }

        if let Some(contributions) = json.find("contributions").and_then(Json::as_array) {
            if contributions.len() > MAX_SHARES as usize {
                return Err(Parse("contributions"));
            }
            for entry in contributions {
                parsed.contributions.push(read_contribution(entry)?);
            }
        }

        parsed.protx = opt_text(json, "protx")?.unwrap_or_default();
        if let Some(hash) = opt_text(json, "consentHash")? {
            if !hash.is_empty()
                && !(hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            {
                return Err(Parse("consentHash"));
            }
            parsed.consent_hash = hash.to_ascii_lowercase();
        }
        if let Some(i) = int(
            json.find("collateralIndex"),
            Json::get_i32,
            "collateralIndex",
        )? {
            parsed.collateral_index = i;
        }

        if let Some(sigs) = json.find("sigs").and_then(Json::as_array) {
            for entry in sigs {
                let well_formed = entry.is_object()
                    && entry.find("shareIndex").is_some_and(Json::is_num)
                    && entry.find("signature").is_some_and(Json::is_str);
                if !well_formed {
                    return Err(Parse("sigs"));
                }
                let share_index = entry
                    .find("shareIndex")
                    .and_then(Json::get_i32)
                    .ok_or(Parse("sigs"))?;
                if parsed.signature_for(share_index).is_some() {
                    return Err(Parse("sigs"));
                }
                parsed.sigs.push(Signature {
                    share_index,
                    signature: opt_text(entry, "signature")?.unwrap_or_default(),
                });
            }
        }

        parsed.prepare_wallet = opt_text(json, "prepareWallet")?.unwrap_or_default();
        parsed.operator_secret_holder = opt_text(json, "operatorSecretHolder")?.unwrap_or_default();
        parsed.coordinator_label = opt_text(json, "coordinatorLabel")?.unwrap_or_default();

        parsed.check_labels()?;
        if parsed.stage != Stage::Draft {
            funding::check_frozen(&parsed)?;
        }
        Ok(parsed)
    }
}

/// A string value as UTF-8; `None` when absent, not a string, or not UTF-8.
fn text(v: Option<&Json>) -> Option<String> {
    v.and_then(Json::as_text).map(str::to_owned)
}

/// `if (v.isStr()) field = v.get_str()`: a member of another type is
/// skipped; a string that isn't UTF-8 is refused.
fn opt_text(json: &Json, key: &str) -> Result<Option<String>, EnvelopeError> {
    match json.find(key) {
        Some(v) if v.is_str() => text(Some(v))
            .map(Some)
            .ok_or(EnvelopeError::Parse("non-UTF-8 string")),
        _ => Ok(None),
    }
}

/// `if (v.isNum()) field = v.getInt<T>()`: `None` for a member that is
/// absent or not a number; a number `getInt<T>` throws on is refused.
fn int<T>(
    v: Option<&Json>,
    get: fn(&Json) -> Option<T>,
    field: &'static str,
) -> Result<Option<T>, EnvelopeError> {
    match v {
        Some(v) if v.is_num() => get(v).map(Some).ok_or(EnvelopeError::Parse(field)),
        _ => Ok(None),
    }
}

/// One `contributions[]` entry (`mnsharesession.cpp:542-606`).
fn read_contribution(entry: &Json) -> Result<Contribution, EnvelopeError> {
    let malformed = EnvelopeError::Parse("contributions");
    if !entry.is_object() || !entry.find("label").is_some_and(Json::is_str) {
        return Err(malformed);
    }
    let Some(inputs) = entry.find("inputs").and_then(Json::as_array) else {
        return Err(malformed);
    };
    if inputs.len() > MAX_CONTRIBUTION_INPUTS {
        return Err(malformed);
    }
    let mut contribution = Contribution {
        label: opt_text(entry, "label")?.unwrap_or_default(),
        inputs: Vec::with_capacity(inputs.len()),
        change: None,
        change_index: -1,
    };
    for input in inputs {
        let well_formed = input.is_object()
            && input.find("txid").is_some_and(Json::is_str)
            && input.find("vout").is_some_and(Json::is_num)
            && input.find("sequence").is_some_and(Json::is_num);
        if !well_formed {
            return Err(malformed);
        }
        let u32_of = |key| {
            input
                .find(key)
                .and_then(Json::get_i64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(malformed)
        };
        let (vout, sequence) = (u32_of("vout")?, u32_of("sequence")?);
        let txid = opt_text(input, "txid")?.unwrap_or_default();
        if !funding::is_txid_hex(&txid) {
            return Err(malformed);
        }
        contribution.inputs.push(Input {
            txid,
            vout,
            sequence,
        });
    }
    if let Some(change) = entry.find("change").filter(|c| c.is_object()) {
        if !change.find("address").is_some_and(Json::is_str)
            || !change.find("amount").is_some_and(Json::is_num)
        {
            return Err(malformed);
        }
        let amount = change
            .find("amount")
            .and_then(Json::get_i64)
            .filter(|a| money_range(*a))
            .ok_or(malformed)?;
        contribution.change = Some(Change {
            address: opt_text(change, "address")?.unwrap_or_default(),
            amount,
        });
    }
    if let Some(i) = int(entry.find("changeIndex"), Json::get_i32, "contributions")? {
        contribution.change_index = i;
    }
    Ok(contribution)
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests;
