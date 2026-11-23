//! dash-qt's shared-masternode messages (MG-04b §2): the message code
//! (`EnvelopeFingerprint`, rc.3 `src/qt/mnsharesession.cpp:46-57,225-249`),
//! the maintenance signing envelope (`dash-shared-mn-sigs`,
//! `src/qt/sharedmndialogs.cpp:559-730`), and the router that tells the three
//! message kinds apart (`SharedMnImport::Detect`,
//! `src/qt/sharedmnwidgets.cpp:354-399`). The session envelope is
//! [`super::session`]'s.
//!
//! Decoding here is structure only: what the readers check before they need
//! the masternode's share table or the session's terms. Signature
//! verification and the comparisons with a held session are the caller's.

use dashcore::Network;
use dashcore::hashes::{Hash, sha256};

use super::TRANSACTION_PROVIDER_DISSOLVE;
use super::json::Json;
use super::rawtx::SpecialTx;
use super::session::{SESSION_TYPE, Session};
use crate::params::MAX_ENVELOPE_BYTES;

/// The key the producer appends last.
pub const FINGERPRINT_KEY: &str = "fingerprint";
/// `SIG_ENVELOPE_TYPE` (`sharedmndialogs.cpp:64`).
pub const SIGS_TYPE: &str = "dash-shared-mn-sigs";
/// `SIG_ENVELOPE_VERSION` (`sharedmndialogs.cpp:65`).
pub const SIGS_VERSION: i32 = 1;

/// Why a message was refused. None of them repeats the pasted text
/// (MG-00 D36); [`EnvelopeError::Parse`] names the rule, not the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    #[error("this isn't a shared-masternode message")]
    NotShared,
    #[error("the message is larger than 2 MiB")]
    TooLarge,
    #[error("the message is malformed ({0})")]
    Parse(&'static str),
    #[error("the message is for another network")]
    Network,
    /// MG-04b D28: labels empty or repeated after trimming, a contribution
    /// naming no share, or two contributions for one share.
    #[error("the message's share labels are ambiguous")]
    Labels,
    /// A session past `combined` whose consent signature for this share
    /// doesn't verify (`mnsharesession.cpp:637-648`).
    #[error("the approval for share {share} doesn't verify")]
    BadSignature { share: u16 },
}

/// `CChainParams::NetworkIDString()` (rc.3 `src/chainparamsbase.cpp:13-16`).
pub fn network_id(network: Network) -> &'static str {
    match network {
        Network::Mainnet => "main",
        Network::Testnet => "test",
        Network::Devnet => "devnet",
        Network::Regtest => "regtest",
    }
}

/// `WithoutFingerprint`: the object rebuilt with `pushKV` minus every
/// `fingerprint` key. `pushKV` replaces the first value of a key already
/// pushed, so a top-level duplicate key collapses to one entry, in the first
/// one's place, holding the last one's value. Anything but an object is
/// returned as is.
pub fn without_fingerprint(json: &Json) -> Json {
    let Json::Obj(entries) = json else {
        return json.clone();
    };
    let mut kept: Vec<(Vec<u8>, Json)> = Vec::new();
    for (key, value) in entries {
        if key == FINGERPRINT_KEY.as_bytes() {
            continue;
        }
        match kept.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value.clone(),
            None => kept.push((key.clone(), value.clone())),
        }
    }
    Json::Obj(kept)
}

/// `EnvelopeFingerprint`: single SHA-256 of `write(2)` of
/// [`without_fingerprint`], the first four bytes as upper-case hex,
/// `XXXX-XXXX`.
pub fn fingerprint(json: &Json) -> String {
    let digest = sha256::Hash::hash(&without_fingerprint(json).write(2));
    let hex = hex::encode_upper(&digest.as_byte_array()[..4]);
    format!("{}-{}", &hex[..4], &hex[4..])
}

/// `CheckFingerprint`: `true` unless the object carries a string
/// `fingerprint` (the first, as `find_value`) that differs from
/// [`fingerprint`] ignoring case. A mismatch only warns in dash-qt.
pub fn fingerprint_matches(json: &Json) -> bool {
    match json.find(FINGERPRINT_KEY) {
        Some(stamped) if json.is_object() && stamped.is_str() => stamped
            .as_text()
            .is_some_and(|s| s.eq_ignore_ascii_case(&fingerprint(json))),
        _ => true,
    }
}

/// `AppendFingerprint`: the producer's last key.
pub fn append_fingerprint(json: &mut Json) {
    let code = fingerprint(json);
    json.push_kv(FINGERPRINT_KEY, Json::str(&code));
}

/// What a reader learns from the code of a message it received (§2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The received object's recomputed fingerprint: the code its sender
    /// reads out.
    pub code: String,
    /// The stamped code didn't match: "This message was edited after it was
    /// copied." (`mnsharesession.cpp:239-249`).
    pub edited: bool,
}

impl Received {
    fn of(json: &Json) -> Self {
        Self {
            code: fingerprint(json),
            edited: !fingerprint_matches(json),
        }
    }
}

/// `EncodeBase64`: standard alphabet, padded (rc.3
/// `src/util/strencodings.cpp:128-137`).
pub fn encode_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `DecodeBase64` (rc.3 `src/util/strencodings.cpp:139-175`): the length a
/// multiple of 4, at most two trailing `=`, every other character in the
/// alphabet, and no set bits left over (`ConvertBits<6, 8, false>`).
pub fn decode_base64(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let s = s.strip_suffix('=').unwrap_or(s);
    let s = s.strip_suffix('=').unwrap_or(s);
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6 | u32::from(v)) & 0xfff;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 || (acc << (8 - bits)) & 0xff != 0 {
        return None;
    }
    Some(out)
}

/// `IsHex` (rc.3 `src/util/strencodings.cpp`): non-empty, even length, hex
/// digits only.
pub fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.len().is_multiple_of(2) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The sigs envelope's `kind` (`sharedmndialogs.cpp:658`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigsKind {
    /// A unanimous ProDisTx.
    Dissolve,
    /// A ProUpSharedRegTx.
    Registrar,
}

impl SigsKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Dissolve => "dissolve",
            Self::Registrar => "registrar",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Dissolve, Self::Registrar]
            .into_iter()
            .find(|k| k.key() == key)
    }
}

/// A sigs envelope as this wallet writes it (`envelopeJson`,
/// `sharedmndialogs.cpp:653-665`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigsEnvelope {
    pub network: Network,
    pub kind: SigsKind,
    /// `GetHex()`: lower-case display hex.
    pub pro_tx_hash: dashcore::Txid,
    /// The transaction every owner signs; empty before there is one.
    pub tx: Vec<u8>,
    /// Ascending by share index (a `std::map` in dash-qt), each verified by
    /// whoever added it.
    pub signatures: Vec<(u16, [u8; 65])>,
}

impl SigsEnvelope {
    /// The envelope with its code appended last.
    pub fn to_json(&self) -> Json {
        let mut json = Json::obj();
        json.push_kv("type", Json::str(SIGS_TYPE));
        json.push_kv("version", Json::int(SIGS_VERSION.into()));
        json.push_kv("network", Json::str(network_id(self.network)));
        json.push_kv("kind", Json::str(self.kind.key()));
        json.push_kv("proTxHash", Json::str(&self.pro_tx_hash.to_string()));
        json.push_kv("tx", Json::str(&hex::encode(&self.tx)));
        let mut sigs: Vec<_> = self.signatures.clone();
        sigs.sort_by_key(|(i, _)| *i);
        json.push_kv("signatures", signatures_json(&sigs));
        append_fingerprint(&mut json);
        json
    }

    /// The text to copy: `write(2)`.
    pub fn to_text(&self) -> String {
        String::from_utf8(self.to_json().write(2)).expect("the writer emits UTF-8")
    }
}

/// `[{shareIndex, signature}]` as both envelopes write them.
pub(crate) fn signatures_json(sigs: &[(u16, [u8; 65])]) -> Json {
    let mut arr = Json::Arr(Vec::new());
    for (index, sig) in sigs {
        let mut entry = Json::obj();
        entry.push_kv("shareIndex", Json::int((*index).into()));
        entry.push_kv("signature", Json::str(&encode_base64(sig)));
        arr.push(entry);
    }
    arr
}

/// One entry of a received signature list, as `addSignatures` reads it
/// before it needs the share table (`sharedmndialogs.cpp:577-603`). The
/// caller checks the index against the share count, then verifies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SigEntry {
    /// "An approval is missing its share number or its signature."
    Missing,
    /// "An approval has an unreadable share number." (`getInt<int>` threw.)
    UnreadableIndex,
    /// A share number with a signature that isn't base64 of 65 bytes. Core
    /// reports an out-of-range index first.
    BadSignature {
        index: i32,
    },
    Signature {
        index: i32,
        sig: [u8; 65],
    },
}

impl SigEntry {
    /// The share number, when the entry has a readable one.
    pub fn index(&self) -> Option<i32> {
        match self {
            Self::BadSignature { index } | Self::Signature { index, .. } => Some(*index),
            Self::Missing | Self::UnreadableIndex => None,
        }
    }
}

/// `addSignatures`' per-entry reading of a JSON array.
pub fn read_sig_entries(entries: &[Json]) -> Vec<SigEntry> {
    entries
        .iter()
        .map(|entry| {
            let (Some(index), Some(signature)) =
                (entry.find("shareIndex"), entry.find("signature"))
            else {
                return SigEntry::Missing;
            };
            let Some(index) = index.get_i32() else {
                return SigEntry::UnreadableIndex;
            };
            // A non-string signature reads as "" (`:597`), which doesn't
            // decode to 65 bytes.
            let sig = signature
                .as_text()
                .and_then(decode_base64)
                .and_then(|b| <[u8; 65]>::try_from(b).ok());
            match sig {
                Some(sig) => SigEntry::Signature { index, sig },
                None => SigEntry::BadSignature { index },
            }
        })
        .collect()
}

/// A received sigs envelope (`importEnvelopeChecked`,
/// `sharedmndialogs.cpp:693-724`), before the masternode is looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedSigs {
    pub kind: SigsKind,
    /// As received; dash-qt compares it case-insensitively (`:710-714`).
    pub pro_tx_hash: dashcore::Txid,
    /// The transaction, if the envelope carries one (`tx` non-empty).
    pub tx: Option<SpecialTx>,
    /// `None` when there is no `signatures` key. A non-array value is
    /// `Some(None)`: dash-qt reports "This message does not contain any
    /// approvals." and still takes the rest (`:569-572,720-724`).
    pub entries: Option<Option<Vec<SigEntry>>>,
    pub received: Received,
}

/// A string member, as dash-qt reads `json.exists(k) ? get_str() : …`:
/// absent is `Ok(None)`; present but not a string throws in UniValue, which
/// `importEnvelope` reports as "not a shared masternode message"
/// (`:672-680`).
fn string_member<'a>(json: &'a Json, key: &str) -> Result<Option<&'a str>, EnvelopeError> {
    match json.find(key) {
        None => Ok(None),
        Some(v) => v.as_text().map(Some).ok_or(if v.is_str() {
            EnvelopeError::Parse("non-UTF-8 string")
        } else {
            EnvelopeError::NotShared
        }),
    }
}

impl ReceivedSigs {
    /// Reads a `dash-shared-mn-sigs` object. The wallet adds two rules
    /// (MG-04b §2.5): `version` must be 1 (dash-qt never reads it, but always
    /// writes 1), and `proTxHash` must be 64 hex.
    pub fn from_json(json: &Json, network: Network) -> Result<Self, EnvelopeError> {
        if string_member(json, "type")? != Some(SIGS_TYPE) {
            return Err(EnvelopeError::NotShared);
        }
        let received = Received::of(json);
        // A missing network is "", a mismatch (`:698-704`).
        if string_member(json, "network")?.unwrap_or("") != network_id(network) {
            return Err(EnvelopeError::Network);
        }
        if json.find("version").and_then(Json::get_i32) != Some(SIGS_VERSION) {
            return Err(EnvelopeError::Parse("version"));
        }
        let kind = string_member(json, "kind")?
            .and_then(SigsKind::from_key)
            .ok_or(EnvelopeError::Parse("kind"))?;
        let pro_tx_hash = string_member(json, "proTxHash")?
            .filter(|h| h.len() == 64 && is_hex(h))
            .and_then(|h| h.parse().ok())
            .ok_or(EnvelopeError::Parse("proTxHash"))?;
        // `setTransaction` trims the hex and decodes it (`:540-555`).
        let tx = match string_member(json, "tx")?.map(str::trim) {
            None | Some("") => None,
            Some(h) => Some(decode_tx_hex(h).ok_or(EnvelopeError::Parse("tx"))?),
        };
        let entries = json
            .find("signatures")
            .map(|v| v.as_array().map(read_sig_entries));
        Ok(Self {
            kind,
            pro_tx_hash,
            tx,
            entries,
            received,
        })
    }
}

/// `DecodeHexTx` of a special transaction, trailing bytes refused.
pub fn decode_tx_hex(h: &str) -> Option<SpecialTx> {
    if !is_hex(h) {
        return None;
    }
    SpecialTx::from_bytes(&hex::decode(h).ok()?).ok()
}

/// One pasted or opened message, told apart as `SharedMnImport::Detect`
/// does (MG-04b §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedMessage {
    Session {
        session: Box<Session>,
        received: Received,
    },
    Sigs(ReceivedSigs),
    /// A bare JSON array of signature entries: dash-qt's collector takes one
    /// for the transaction it holds (`sharedmndialogs.cpp:685-691`); the
    /// caller needs a target session for it (§2.5).
    BareSigs(Vec<SigEntry>),
    /// Hex of a type-10 transaction: a standby. Its checks are the caller's
    /// (MG-04b D32).
    Standby(SpecialTx),
}

/// Reads a message: input over 2 MiB is refused unread
/// (`sharedmnwidgets.h:158`), the text is trimmed; a JSON object with one of
/// the two types is that envelope, any other object is refused; a JSON
/// array is a bare signature list; otherwise hex of a type-10 transaction is
/// a standby, and anything else is refused.
pub fn decode_message(text: &str, network: Network) -> Result<SharedMessage, EnvelopeError> {
    if text.len() > MAX_ENVELOPE_BYTES {
        return Err(EnvelopeError::TooLarge);
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(EnvelopeError::NotShared);
    }
    if let Some(json) = Json::read(trimmed.as_bytes()) {
        if let Some(entries) = json.as_array() {
            return Ok(SharedMessage::BareSigs(read_sig_entries(entries)));
        }
        if !json.is_object() {
            return Err(EnvelopeError::NotShared);
        }
        // `Detect` reads a non-string type as "" (`sharedmnwidgets.cpp:366`).
        return match json.find("type").and_then(Json::as_text) {
            Some(SESSION_TYPE) => Ok(SharedMessage::Session {
                received: Received::of(&json),
                session: Box::new(Session::from_json(&json, network)?),
            }),
            Some(SIGS_TYPE) => Ok(SharedMessage::Sigs(ReceivedSigs::from_json(
                &json, network,
            )?)),
            _ => Err(EnvelopeError::NotShared),
        };
    }
    match decode_tx_hex(trimmed) {
        Some(tx) if tx.tx_type == TRANSACTION_PROVIDER_DISSOLVE => Ok(SharedMessage::Standby(tx)),
        _ => Err(EnvelopeError::NotShared),
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_126_fingerprint_ignores_every_fingerprint_key_and_case() {
        let mut j = Json::obj();
        j.push_kv("type", Json::str("dash-shared-mn-sigs"));
        let code = fingerprint(&j);
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        j.push_kv(FINGERPRINT_KEY, Json::str(&code.to_lowercase()));
        assert_eq!(fingerprint(&j), code);
        assert!(fingerprint_matches(&j));
        j.push_kv(FINGERPRINT_KEY, Json::str("0000-0000"));
        assert!(!fingerprint_matches(&j));
        // A message without a code, or with a non-string one, is not "edited".
        assert!(fingerprint_matches(&Json::obj()));
        let mut n = Json::obj();
        n.push_kv(FINGERPRINT_KEY, Json::int(1));
        assert!(fingerprint_matches(&n));
    }

    #[test]
    fn test_QT_127_base64_is_cores() {
        for (data, text) in [
            (&b""[..], ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(data), text);
            assert_eq!(decode_base64(text).as_deref(), Some(data));
        }
        let sig = [0xa5u8; 65];
        assert_eq!(decode_base64(&encode_base64(&sig)).unwrap(), sig);
        // Core refuses: a bad length, a third `=`, padding inside, a
        // character outside the alphabet, and set bits after the last byte.
        for bad in [
            "Zg=",
            "Z===",
            "Zg==Zg==",
            "Zm9v Zg==",
            "Zm-v",
            "Zh==",
            "Zm9=",
            "====",
        ] {
            assert_eq!(decode_base64(bad), None, "{bad}");
        }
    }

    fn sigs_json(f: impl FnOnce(&mut Json)) -> Json {
        let env = SigsEnvelope {
            network: Network::Regtest,
            kind: SigsKind::Dissolve,
            pro_tx_hash: "00000000000000000000000000000000000000000000000000000000000000ab"
                .parse()
                .unwrap(),
            tx: Vec::new(),
            signatures: vec![(1, [7; 65]), (0, [9; 65])],
        };
        let mut json = env.to_json();
        f(&mut json);
        json
    }

    #[test]
    fn test_QT_127_sigs_envelope_layout_and_round_trip() {
        let json = sigs_json(|_| {});
        let Json::Obj(entries) = &json else { panic!() };
        let keys: Vec<&[u8]> = entries.iter().map(|(k, _)| k.as_slice()).collect();
        assert_eq!(
            keys,
            [
                &b"type"[..],
                b"version",
                b"network",
                b"kind",
                b"proTxHash",
                b"tx",
                b"signatures",
                b"fingerprint"
            ]
        );
        let text = String::from_utf8(json.write(2)).unwrap();
        assert!(text.contains(
            "\"proTxHash\": \"00000000000000000000000000000000000000000000000000000000000000ab\""
        ));
        let SharedMessage::Sigs(r) =
            decode_message(&format!("\n {text}\t"), Network::Regtest).unwrap()
        else {
            panic!("a sigs envelope")
        };
        assert_eq!(r.kind, SigsKind::Dissolve);
        assert!(!r.received.edited);
        assert_eq!(r.tx, None);
        // Ascending by share index.
        assert_eq!(
            r.entries,
            Some(Some(vec![
                SigEntry::Signature {
                    index: 0,
                    sig: [9; 65]
                },
                SigEntry::Signature {
                    index: 1,
                    sig: [7; 65]
                },
            ]))
        );
    }

    #[test]
    fn test_QT_127_sigs_reader_refusals_in_dash_qts_order() {
        let read = |f: &dyn Fn(&mut Json)| {
            let mut json = sigs_json(|_| {});
            f(&mut json);
            ReceivedSigs::from_json(&json, Network::Regtest).map(|_| ())
        };
        assert_eq!(
            read(&|j| j.push_kv("type", Json::int(1))),
            Err(EnvelopeError::NotShared)
        );
        assert_eq!(
            read(&|j| j.push_kv("network", Json::str("test"))),
            Err(EnvelopeError::Network)
        );
        assert_eq!(read(&|j| remove(j, "network")), Err(EnvelopeError::Network));
        assert_eq!(
            read(&|j| j.push_kv("network", Json::int(0))),
            Err(EnvelopeError::NotShared)
        );
        assert_eq!(
            read(&|j| j.push_kv("version", Json::int(2))),
            Err(EnvelopeError::Parse("version"))
        );
        assert_eq!(
            read(&|j| remove(j, "version")),
            Err(EnvelopeError::Parse("version"))
        );
        assert_eq!(
            read(&|j| j.push_kv("kind", Json::str("revoke"))),
            Err(EnvelopeError::Parse("kind"))
        );
        assert_eq!(
            read(&|j| j.push_kv("kind", Json::Null)),
            Err(EnvelopeError::NotShared)
        );
        assert_eq!(
            read(&|j| j.push_kv("proTxHash", Json::str("ab"))),
            Err(EnvelopeError::Parse("proTxHash"))
        );
        assert_eq!(
            read(&|j| j.push_kv("tx", Json::str("0011"))),
            Err(EnvelopeError::Parse("tx"))
        );
        assert_eq!(
            read(&|j| j.push_kv("tx", Json::Bool(true))),
            Err(EnvelopeError::NotShared)
        );
        // An upper-case ProTx hash and an edited code are read.
        assert_eq!(
            read(&|j| j.push_kv(
                "proTxHash",
                Json::str("00000000000000000000000000000000000000000000000000000000000000AB")
            )),
            Ok(())
        );
        let mut edited = sigs_json(|_| {});
        edited.push_kv("tx", Json::str(""));
        edited.push_kv(FINGERPRINT_KEY, Json::str("0000-0000"));
        assert!(
            ReceivedSigs::from_json(&edited, Network::Regtest)
                .unwrap()
                .received
                .edited
        );
    }

    fn remove(json: &mut Json, key: &str) {
        let Json::Obj(entries) = json else { panic!() };
        entries.retain(|(k, _)| k != key.as_bytes());
    }

    #[test]
    fn test_QT_127_signature_entries_as_add_signatures_reads_them() {
        let sig = encode_base64(&[3; 65]);
        let text = format!(
            r#"[{{"shareIndex":0}}, {{"shareIndex":"x","signature":"{sig}"}},
                {{"shareIndex":1.5,"signature":"{sig}"}}, {{"shareIndex":-1,"signature":"{sig}"}},
                {{"shareIndex":2,"signature":"{short}"}}, {{"shareIndex":3,"signature":7}}, 5,
                {{"signature":"{sig}","shareIndex":4}}]"#,
            short = encode_base64(&[3; 64])
        );
        let SharedMessage::BareSigs(entries) = decode_message(&text, Network::Regtest).unwrap()
        else {
            panic!("a bare list")
        };
        assert_eq!(
            entries,
            [
                SigEntry::Missing,
                SigEntry::UnreadableIndex,
                SigEntry::UnreadableIndex,
                SigEntry::Signature {
                    index: -1,
                    sig: [3; 65]
                },
                SigEntry::BadSignature { index: 2 },
                SigEntry::BadSignature { index: 3 },
                SigEntry::Missing,
                SigEntry::Signature {
                    index: 4,
                    sig: [3; 65]
                },
            ]
        );
        // A non-array `signatures` is read, flagged.
        let json = sigs_json(|j| j.push_kv("signatures", Json::obj()));
        assert_eq!(
            ReceivedSigs::from_json(&json, Network::Regtest)
                .unwrap()
                .entries,
            Some(None)
        );
    }

    #[test]
    fn test_QT_126_router_tells_the_kinds_apart() {
        let net = Network::Regtest;
        assert_eq!(decode_message("", net), Err(EnvelopeError::NotShared));
        assert_eq!(decode_message(" \n", net), Err(EnvelopeError::NotShared));
        assert_eq!(decode_message("{}", net), Err(EnvelopeError::NotShared));
        assert_eq!(
            decode_message(r#"{"type":"other"}"#, net),
            Err(EnvelopeError::NotShared)
        );
        assert_eq!(
            decode_message("\"text\"", net),
            Err(EnvelopeError::NotShared)
        );
        assert_eq!(decode_message("hello", net), Err(EnvelopeError::NotShared));
        assert_eq!(decode_message("0011", net), Err(EnvelopeError::NotShared));
        let big = " ".repeat(MAX_ENVELOPE_BYTES + 1);
        assert_eq!(decode_message(&big, net), Err(EnvelopeError::TooLarge));
        // A type-10 transaction is a standby; another special type isn't.
        let mut tx = SpecialTx {
            version: 3,
            tx_type: TRANSACTION_PROVIDER_DISSOLVE,
            input: Vec::new(),
            output: Vec::new(),
            lock_time: 0,
            payload: vec![1, 0],
        };
        let hex_tx = hex::encode(tx.to_bytes());
        assert!(matches!(
            decode_message(&format!(" {hex_tx}\n"), net),
            Ok(SharedMessage::Standby(_))
        ));
        tx.tx_type = 11;
        assert_eq!(
            decode_message(&hex::encode(tx.to_bytes()), net),
            Err(EnvelopeError::NotShared)
        );
    }
}
