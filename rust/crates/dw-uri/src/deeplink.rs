//! Classifies a URL the app was opened with, or a scanned/pasted link, the
//! way the iOS wallet routes it (`DWURLParser.m`, `DashConnectDeepLink.swift`,
//! `DashPayUserLink.swift`, `DWInvitationLinkNormalizer.swift`).
//!
//! Routing order, as on iOS: DashConnect (`dash-key:` / `dash-st:`), then
//! anything mentioning `uphold`, then `dashwallet:` actions, then payment
//! URIs. After that come the DashPay user QR, invitation links and BIP73
//! `http(s)` payment-request URLs, which iOS routes from the scanner.
//!
//! Differences from iOS, all deliberate:
//! - Scheme comparison is case-insensitive (RFC 3986); iOS compares
//!   `dash`/`pay`/`dashwallet` case-sensitively in `canHandleURL`.
//! - `dashwallet://pay=…` builds the payment URI with its parameters sorted
//!   by key; iOS iterates an `NSDictionary`, whose order is unspecified.
//! - Payload checks for DashConnect URIs (Base58 body, 4 KiB limit) are not
//!   done here; they belong to the DashConnect parser.

use zeroize::Zeroizing;

use crate::ext::{self, PaymentRequest};

/// Where a link should go.
#[derive(Debug, Clone, PartialEq)]
pub enum DeepLink {
    /// `dash-key:` DashConnect login-key request (whole URI).
    DashConnectKey(String),
    /// `dash-st:` DashConnect state-transition request (whole URI).
    DashConnectSt(String),
    /// A link for the Uphold integration (contains `uphold`).
    Integration(String),
    /// `dashwallet://scanqr`: open the QR scanner.
    ScanQr,
    /// `dashwallet://request=address&sender=<app>`: another app asks for a
    /// receive address.
    AddressRequest { sender: String, request: String },
    /// A payment (`dash:`, `pay:`, `dashwallet://pay=…`, BIP73 `http(s)`).
    Payment(PaymentRequest),
    /// `dashpay://user?id=…&username=…`: a DashPay contact QR.
    DashPayUser(DashPayUserLink),
    /// A DashPay invitation, normalized to the URI the SDK parser accepts.
    /// It contains a bearer private key: never log it.
    Invitation(String),
    /// Nothing the wallet handles (also `dashwallet:` links with invalid
    /// parameters).
    Unknown,
}

/// The parts of a generic `scheme:[//authority]path[?query][#fragment]` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Parts<'a> {
    scheme: String,
    userinfo: Option<&'a str>,
    host: Option<&'a str>,
    port: Option<&'a str>,
    path: &'a str,
    query: Option<&'a str>,
    fragment: Option<&'a str>,
}

fn split_url(s: &str) -> Option<Parts<'_>> {
    let colon = s.find(':')?;
    let scheme = &s[..colon];
    let mut sb = scheme.bytes();
    if !sb.next()?.is_ascii_alphabetic()
        || !sb.all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
    {
        return None;
    }
    let mut rest = &s[colon + 1..];
    let (mut userinfo, mut host, mut port) = (None, None, None);
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let mut authority = &after[..end];
        if let Some(at) = authority.rfind('@') {
            userinfo = Some(&authority[..at]);
            authority = &authority[at + 1..];
        }
        match authority.rfind(':') {
            Some(c) if !authority.ends_with(']') => {
                host = Some(&authority[..c]);
                port = Some(&authority[c + 1..]);
            }
            _ => host = Some(authority),
        }
        rest = &after[end..];
    }
    let (rest, fragment) = match rest.find('#') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let (path, query) = match rest.find('?') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    Some(Parts {
        scheme: scheme.to_ascii_lowercase(),
        userinfo,
        host,
        port,
        path,
        query,
        fragment,
    })
}

/// `URLComponents.queryItems`: `&`-separated, split at the first `=`,
/// names and values percent-decoded (left raw when that fails).
fn query_items(query: &str) -> Vec<(String, Option<String>)> {
    query
        .split('&')
        .map(|item| match item.split_once('=') {
            Some((k, v)) => (decode(k), Some(decode(v))),
            None => (decode(item), None),
        })
        .collect()
}

fn decode(s: &str) -> String {
    ext::remove_percent_encoding(s).unwrap_or_else(|| s.to_owned())
}

/// Classifies `input` (surrounding whitespace trimmed).
pub fn classify_link(input: &str) -> DeepLink {
    let trimmed = input.trim();
    if trimmed.starts_with("dash-key:") {
        return DeepLink::DashConnectKey(trimmed.to_owned());
    }
    if trimmed.starts_with("dash-st:") {
        return DeepLink::DashConnectSt(trimmed.to_owned());
    }
    if trimmed.contains("uphold") {
        return DeepLink::Integration(trimmed.to_owned());
    }
    let Some(parts) = split_url(trimmed) else {
        return DeepLink::Unknown;
    };
    match parts.scheme.as_str() {
        "dashwallet" => return dashwallet_action(&parts),
        "dash" | "pay" => {
            return ext::parse_payment_uri(trimmed).map_or(DeepLink::Unknown, DeepLink::Payment);
        }
        _ => {}
    }
    if let Some(user) = DashPayUserLink::parse(trimmed) {
        return DeepLink::DashPayUser(user);
    }
    if let Some(invite) = normalize_invitation(trimmed) {
        return DeepLink::Invitation(invite);
    }
    if parts.scheme == "http" || parts.scheme == "https" {
        return ext::parse_payment_string(trimmed).map_or(DeepLink::Unknown, DeepLink::Payment);
    }
    DeepLink::Unknown
}

/// `DWURLParser actionForURL:` for the `dashwallet` scheme. Parameters are
/// read from the host part (`dashwallet://request=address&sender=app`).
fn dashwallet_action(parts: &Parts<'_>) -> DeepLink {
    let host = parts.host.unwrap_or_default();
    if host == "scanqr" || parts.path == "/scanqr" {
        return DeepLink::ScanQr;
    }
    // `parseParamsFromURL:` keeps only `k=v` pairs that split into exactly two.
    let params: std::collections::BTreeMap<String, String> = host
        .split('&')
        .filter_map(|p| {
            let kv: Vec<&str> = p.split('=').collect();
            (kv.len() == 2).then(|| (kv[0].to_owned(), kv[1].to_owned()))
        })
        .collect();
    if host.starts_with("request") || parts.path == "/request" {
        let valid = params.get("request").is_some_and(|r| r == "address")
            && params.contains_key("sender")
            && params.get("account").is_none_or(|a| a == "0");
        if !valid {
            return DeepLink::Unknown;
        }
        return DeepLink::AddressRequest {
            sender: params["sender"].clone(),
            request: params["request"].clone(),
        };
    }
    if host.starts_with("pay") || parts.path == "/pay" {
        let (Some(pay), Some(sender)) = (params.get("pay"), params.get("sender")) else {
            return DeepLink::Unknown;
        };
        let label = format!(
            "Application {} is requesting a payment to",
            capitalized(sender)
        );
        let mut query = vec![format!("label={}", ext::ios_query_encode(&label))];
        for (k, v) in &params {
            if k != "label" {
                query.push(format!(
                    "{}={}",
                    ext::ios_query_encode(k),
                    ext::ios_query_encode(v)
                ));
            }
        }
        let uri = format!("dash:{pay}?{}", query.join("&"));
        return ext::parse_payment_uri(&uri).map_or(DeepLink::Unknown, DeepLink::Payment);
    }
    DeepLink::Unknown
}

/// `NSString.capitalizedString` for ASCII words: first letter of each
/// whitespace-separated word upper-cased, the rest lower-cased.
fn capitalized(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut at_word_start = true;
    for c in s.chars() {
        if c.is_whitespace() {
            at_word_start = true;
            out.push(c);
        } else if at_word_start {
            out.extend(c.to_uppercase());
            at_word_start = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// A DashPay user QR: `dashpay://user?id=<base58 identity id>&username=<label>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashPayUserLink {
    /// 32-byte Platform identity id.
    pub identity_id: [u8; 32],
    /// DPNS label without a `.dash` suffix.
    pub username: String,
}

impl DashPayUserLink {
    /// Strict parse, as iOS: scheme/host/parameter names case-insensitive;
    /// userinfo, port, path, fragment, duplicate or unknown parameters
    /// rejected; `id` must be Base58 for 32 bytes; `username` non-empty
    /// after trimming and removing one `.dash` suffix. The username claim is
    /// not verified here.
    pub fn parse(s: &str) -> Option<DashPayUserLink> {
        let parts = split_url(s.trim())?;
        if parts.scheme != "dashpay"
            || !parts.host.is_some_and(|h| h.eq_ignore_ascii_case("user"))
            || parts.userinfo.is_some()
            || parts.port.is_some()
            || !parts.path.is_empty()
            || parts.fragment.is_some()
        {
            return None;
        }
        let mut seen = std::collections::BTreeSet::new();
        let (mut id, mut username) = (None, None);
        for (name, value) in query_items(parts.query.unwrap_or_default()) {
            let name = name.to_ascii_lowercase();
            if !seen.insert(name.clone()) {
                return None;
            }
            match name.as_str() {
                "id" => id = value.and_then(|v| dashcore::base58::decode(&v).ok()),
                "username" => username = value,
                _ => return None,
            }
        }
        let identity_id: [u8; 32] = id?.try_into().ok()?;
        let trimmed = username.unwrap_or_default();
        let trimmed = trimmed.trim();
        let label = trimmed.strip_suffix(".dash").unwrap_or(trimmed);
        if label.is_empty() {
            return None;
        }
        Some(DashPayUserLink {
            identity_id,
            username: label.to_owned(),
        })
    }

    /// The identity id in plain Base58, as Platform writes identifiers.
    pub fn identity_base58(&self) -> String {
        base58(&self.identity_id)
    }

    /// iOS `uriString`: `dashpay://user?id=<base58>&username=<label>`. The
    /// label is percent-encoded outside RFC 3986's unreserved set; a DPNS
    /// label (`[A-Za-z0-9-]`) needs none, so for any name Platform holds the
    /// bytes are the ones iOS's `URLComponents` writes
    /// (`testdata/dashpay_user_links.json`).
    pub fn to_uri(&self) -> String {
        format!(
            "dashpay://user?id={}&username={}",
            base58(&self.identity_id),
            crate::qurl::to_percent_encoding(&self.username)
        )
    }
}

const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Plain Base58 (Bitcoin alphabet, no checksum), as Platform writes
/// identifiers.
fn base58(bytes: &[u8]) -> String {
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    // Little-endian base-58 digits.
    let mut digits: Vec<u8> = Vec::new();
    for &byte in &bytes[zeros..] {
        let mut carry = u32::from(byte);
        for d in &mut digits {
            carry += u32::from(*d) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let ones = std::iter::repeat_n('1', zeros);
    ones.chain(
        digits
            .iter()
            .rev()
            .map(|d| char::from(BASE58_ALPHABET[usize::from(*d)])),
    )
    .collect()
}

/// Plain Base58 decoded into memory that is erased on drop, with no other
/// buffer: the capacity (one byte per character, more than Base58 needs)
/// is reserved up front, so the vector never moves.
fn base58_decode_secret(text: &str) -> Option<Zeroizing<Vec<u8>>> {
    let mut out = Zeroizing::new(Vec::with_capacity(text.len()));
    // Big-endian is built little-endian, then reversed in place.
    for c in text.bytes() {
        let mut carry = BASE58_ALPHABET.iter().position(|a| *a == c)? as u32;
        for d in out.iter_mut() {
            carry += u32::from(*d) * 58;
            *d = carry as u8;
            carry >>= 8;
        }
        while carry > 0 {
            out.push(carry as u8);
            carry >>= 8;
        }
    }
    let zeros = text.bytes().take_while(|c| *c == b'1').count();
    out.extend(std::iter::repeat_n(0, zeros));
    out.reverse();
    Some(out)
}

/// `name` with one `.dash` dropped, in any case: a DPNS label as typed or
/// as a contact code carries it. A second `.dash` stays, for the label
/// check to refuse.
pub fn dpns_label(name: &str) -> &str {
    match name.len().checked_sub(5) {
        Some(i) if name.is_char_boundary(i) && name[i..].eq_ignore_ascii_case(".dash") => {
            &name[..i]
        }
        _ => name,
    }
}

/// The longest `dapk` value decoded (platform-wallet's
/// `MAX_DAPK_BASE58_LEN`): a 38-byte key blob is about 52 characters, and the
/// cap bounds the work a hostile QR can force before the blob is checked.
const MAX_DAPK_LEN: usize = 128;

/// secp256k1's group order: a private key is in `1..ORDER`.
const SECP256K1_ORDER: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B, 0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
];

/// A DIP-15 contact QR ("Auto Accept Proof"):
/// `dash:?du=<username>&dapk=<Base58 key blob>`, the form platform-wallet's
/// `encode_dashpay_contact_uri` writes.
///
/// Only the contact form: no address, no parameter but `du` and `dapk`, each
/// once. DIP-15's merchant form (an address and payment parameters next to
/// `du` and `dapk`) is a payment request, not a contact code. Scheme and
/// parameter names are case-insensitive. `du` loses one `.dash` suffix (any
/// case, as DPNS resolves it) and must not be empty. `dapk` is plain Base58
/// (no percent-encoding) of at most 128 characters, and its blob is what
/// platform-wallet's `decode_auto_accept_key_blob` accepts: type 0, the
/// expiry (big-endian), size 32, then a valid secp256k1 private key. The
/// expiry must also be below 2^31: it is the owner's hardened derivation
/// index (`m/9'/coin'/16'/expiry'`), so no proof past that can verify.
///
/// The code is unsigned. Nothing in it ties the key to the username's
/// identity: the key derives on a hardened path of the owner's seed that
/// is never published, so only the recipient can check it, when a request
/// arrives with its proof.
pub struct DashPayContactCode {
    /// The DPNS label: trimmed, one `.dash` dropped.
    pub username: String,
    /// When the key stops being honoured: UNIX seconds.
    pub expiry: u32,
    /// The auto-accept private key, a bearer credential, decoded with no
    /// copy left behind but those the caller makes. Boxed, so that moving
    /// the code moves no copy of it. Never log it.
    pub key: Box<Zeroizing<[u8; 32]>>,
}

impl DashPayContactCode {
    pub fn parse(s: &str) -> Option<DashPayContactCode> {
        let parts = split_url(s.trim())?;
        if parts.scheme != "dash"
            || parts.host.is_some()
            || !parts.path.is_empty()
            || parts.fragment.is_some()
        {
            return None;
        }
        let (mut username, mut dapk) = (None, None);
        // By hand, not `query_items`: the key's text is read in place, never
        // copied into a decoded string.
        for item in parts.query?.split('&') {
            let (name, value) = item.split_once('=')?;
            let slot = match decode(name).to_ascii_lowercase().as_str() {
                "du" => &mut username,
                "dapk" => &mut dapk,
                _ => return None,
            };
            if slot.replace(value).is_some() {
                return None;
            }
        }
        let username = decode(username?);
        let label = dpns_label(username.trim());
        let dapk = dapk?;
        if label.is_empty() || dapk.len() > MAX_DAPK_LEN {
            return None;
        }
        let blob = base58_decode_secret(dapk)?;
        let (expiry, key) = Self::key_blob(&blob)?;
        Some(DashPayContactCode {
            username: label.to_owned(),
            expiry,
            key,
        })
    }

    /// The expiry and key of a DIP-15 ECDSA key blob.
    fn key_blob(blob: &[u8]) -> Option<(u32, Box<Zeroizing<[u8; 32]>>)> {
        let [0, e0, e1, e2, e3, 32, key @ ..] = blob else {
            return None;
        };
        let key: &[u8; 32] = key.try_into().ok()?;
        let expiry = u32::from_be_bytes([*e0, *e1, *e2, *e3]);
        if expiry >= 1 << 31 || *key == [0; 32] || *key >= SECP256K1_ORDER {
            return None;
        }
        let mut out = Box::new(Zeroizing::new([0; 32]));
        out.copy_from_slice(key);
        Some((expiry, out))
    }
}

impl std::fmt::Debug for DashPayContactCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashPayContactCode")
            .field("username", &self.username)
            .field("expiry", &self.expiry)
            .field("key", &"..")
            .finish()
    }
}

const ONELINK_HOST_SUFFIXES: [&str; 3] = ["onelink.me", "appsflyersdk.com", "appsflyer.com"];
const ONELINK_PAYLOAD_KEYS: [&str; 4] = ["af_dp", "deep_link_value", "link", "af_sub1"];

/// `DWInvitationLinkNormalizer.normalize`: the canonical invitation URI for
/// `dashpay://invite?…`, `https://invitations.dashpay.io/applink?…`, or an
/// AppsFlyer OneLink wrapping either. `None` when it is not one of those.
/// Only the transport is recognized; the payload is validated by the SDK.
pub fn normalize_invitation(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let parts = split_url(trimmed)?;
    let host = parts.host.unwrap_or_default().to_ascii_lowercase();
    match parts.scheme.as_str() {
        "dashpay" => (host == "invite").then(|| trimmed.to_owned()),
        "https" | "http" => {
            if host == "invitations.dashpay.io" {
                let path = parts.path.strip_suffix('/').unwrap_or(parts.path);
                return (path == "/applink").then(|| trimmed.to_owned());
            }
            let is_onelink = ONELINK_HOST_SUFFIXES
                .iter()
                .any(|s| host == *s || host.ends_with(&format!(".{s}")));
            if !is_onelink {
                return None;
            }
            let items = query_items(parts.query?);
            ONELINK_PAYLOAD_KEYS.iter().find_map(|key| {
                let value = items.iter().find(|(k, _)| k == key)?.1.as_deref()?;
                if value.is_empty() {
                    None
                } else {
                    normalize_invitation(value)
                }
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashconnect_first() {
        assert!(matches!(
            classify_link("dash-key:abc?x=1"),
            DeepLink::DashConnectKey(_)
        ));
        assert!(matches!(
            classify_link(" dash-st:abc "),
            DeepLink::DashConnectSt(_)
        ));
    }

    #[test]
    fn dashwallet_actions() {
        assert_eq!(classify_link("dashwallet://scanqr"), DeepLink::ScanQr);
        assert_eq!(
            classify_link("dashwallet://request=address&sender=myapp"),
            DeepLink::AddressRequest {
                sender: "myapp".into(),
                request: "address".into()
            }
        );
        assert_eq!(
            classify_link("dashwallet://request=address&sender=a&account=1"),
            DeepLink::Unknown
        );
        let DeepLink::Payment(p) = classify_link("dashwallet://pay=XabcD&sender=shop&amount=1.5")
        else {
            panic!()
        };
        assert_eq!(p.address.as_deref(), Some("XabcD"));
        assert_eq!(p.amount, Some(150_000_000));
        assert_eq!(
            p.label.as_deref(),
            Some("Application Shop is requesting a payment to")
        );
        assert_eq!(p.callback_scheme.as_deref(), Some("shop"));
    }

    #[test]
    fn invitations() {
        let canonical = "dashpay://invite?du=alice&assetlocktx=00&pk=cXX&islock=11";
        assert_eq!(normalize_invitation(canonical).as_deref(), Some(canonical));
        assert!(normalize_invitation("https://invitations.dashpay.io/applink/?du=a").is_some());
        assert!(normalize_invitation("https://invitations.dashpay.io/other?du=a").is_none());
        let wrapped = "https://dashpay.onelink.me/abc?af_dp=dashpay%3A%2F%2Finvite%3Fdu%3Dalice";
        assert_eq!(
            normalize_invitation(wrapped).as_deref(),
            Some("dashpay://invite?du=alice")
        );
        assert!(
            normalize_invitation("https://evil.example/abc?af_dp=dashpay%3A%2F%2Finvite").is_none()
        );
    }

    #[test]
    fn dashpay_user_link() {
        let id = [7u8; 32];
        // base58 of `id`; base58ck has no plain encoder.
        let b58 = "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx";
        let link =
            DashPayUserLink::parse(&format!("DashPay://USER?ID={b58}&username=bob.dash")).unwrap();
        assert_eq!(
            link,
            DashPayUserLink {
                identity_id: id,
                username: "bob".into()
            }
        );
        assert!(
            DashPayUserLink::parse(&format!("dashpay://user?id={b58}&id={b58}&username=bob"))
                .is_none()
        );
        assert!(
            DashPayUserLink::parse(&format!("dashpay://user/x?id={b58}&username=bob")).is_none()
        );
        assert!(
            DashPayUserLink::parse(&format!("dashpay://user?id={b58}&username=bob&x=1")).is_none()
        );
        assert!(
            DashPayUserLink::parse(&format!("dashpay://user?id={b58}&username=.dash")).is_none()
        );
        assert_eq!(
            link.to_uri(),
            format!("dashpay://user?id={b58}&username=bob")
        );
    }

    /// Links iOS's own `DashPayUserLink` wrote (the fixture's `source`):
    /// written byte for byte and parsed back.
    #[test]
    fn ios_made_user_links() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../testdata/dashpay_user_links.json"
        );
        let fixture = std::fs::read_to_string(path).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&fixture).unwrap();
        let links = fixture["links"].as_array().unwrap();
        assert!(!links.is_empty());
        for case in links {
            let uri = case["uri"].as_str().unwrap();
            let link = DashPayUserLink::parse(uri).unwrap();
            let id = dashcore::base58::decode(case["identity"].as_str().unwrap()).unwrap();
            assert_eq!(link.identity_id.as_slice(), id.as_slice());
            let given = case["username_given"].as_str().unwrap();
            assert_eq!(link.username, given.strip_suffix(".dash").unwrap_or(given));
            assert_eq!(link.to_uri(), uri);
        }
    }

    #[test]
    fn base58_matches_the_decoder() {
        for bytes in [
            vec![],
            vec![0],
            vec![0, 0, 1],
            vec![0xff; 32],
            (0..=40).collect(),
        ] {
            let encoded = base58(&bytes);
            assert_eq!(
                dashcore::base58::decode(&encoded).unwrap(),
                bytes,
                "{encoded}"
            );
            assert_eq!(*base58_decode_secret(&encoded).unwrap(), bytes);
        }
        for text in [
            "11",
            "1z",
            "z1",
            "2NEpo7TZRRrLZSi2U",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            assert_eq!(
                *base58_decode_secret(text).unwrap(),
                dashcore::base58::decode(text).unwrap(),
                "{text}"
            );
        }
        for bad in ["0", "O", "I", "l", "a b", "%41"] {
            assert!(base58_decode_secret(bad).is_none(), "{bad}");
        }
        assert_eq!(
            base58(&[7u8; 32]),
            "US517G5965aydkZ46HS38QLi7UQiSojurfbQfKCELFx"
        );
    }

    /// platform-wallet's `encode_auto_accept_key_blob` layout.
    fn key_blob(expiry: u32) -> Vec<u8> {
        blob_with(expiry, [0x11; 32])
    }

    fn blob_with(expiry: u32, key: [u8; 32]) -> Vec<u8> {
        let mut blob = vec![0x00];
        blob.extend_from_slice(&expiry.to_be_bytes());
        blob.push(0x20);
        blob.extend_from_slice(&key);
        blob
    }

    #[test]
    fn dip15_contact_code() {
        let blob = key_blob(1_700_000_000);
        let dapk = base58(&blob);
        let code = DashPayContactCode::parse(&format!(" dash:?du=bob&dapk={dapk}\n")).unwrap();
        assert_eq!(code.username, "bob");
        assert_eq!((code.expiry, **code.key), (1_700_000_000, [0x11; 32]));
        // Names and scheme in any case; one `.dash` dropped, any case.
        let code = DashPayContactCode::parse(&format!("DASH:?DAPK={dapk}&Du=Bob.DASH")).unwrap();
        assert_eq!(code.username, "Bob");
        let code = DashPayContactCode::parse(&format!("dash:?du=bob.dash.dash&dapk={dapk}"));
        assert_eq!(code.unwrap().username, "bob.dash");
        let code = DashPayContactCode::parse(&format!("dash:?du=b%6Fb&dapk={dapk}"));
        assert_eq!(code.unwrap().username, "bob");

        // The blob: what secp256k1 takes as a key, below 2^31, type 0, size 32.
        let mut order = SECP256K1_ORDER;
        let mut below = SECP256K1_ORDER;
        below[31] -= 1;
        order[31] += 1;
        for (key, ok) in [
            ([0; 32], false),
            (SECP256K1_ORDER, false),
            (below, true),
            (order, false),
            ([0xFF; 32], false),
            ([1; 32], true),
        ] {
            let secp = dashcore::secp256k1::SecretKey::from_secret_bytes(key).is_ok();
            assert_eq!(secp, ok, "{key:?}");
            let code = format!("dash:?du=bob&dapk={}", base58(&blob_with(5, key)));
            assert_eq!(DashPayContactCode::parse(&code).is_some(), ok, "{key:?}");
        }
        let mut bad_blobs = vec![blob_with(1 << 31, [1; 32]), blob[..37].to_vec()];
        bad_blobs.push([&blob[..], &[0]].concat());
        let mut other_type = blob.clone();
        other_type[0] = 1;
        let mut other_size = blob.clone();
        other_size[5] = 33;
        bad_blobs.extend([other_type, other_size]);
        for bad in bad_blobs {
            let code = format!("dash:?du=bob&dapk={}", base58(&bad));
            assert!(DashPayContactCode::parse(&code).is_none(), "{bad:?}");
        }
        let code = format!(
            "dash:?du=bob&dapk={}",
            base58(&blob_with((1 << 31) - 1, [1; 32]))
        );
        assert_eq!(
            DashPayContactCode::parse(&code).unwrap().expiry,
            (1 << 31) - 1
        );

        // DIP-15's own example decodes to 38 bytes whose key-size byte is
        // 0x91, not 0x20: platform-wallet's layout refuses it, and so do we.
        let example =
            "dash:?du=bobspizza&dapk=13SuoA8Z5tG9PYhvPYzxyDb6yA1yQ4rTMUpLDgMF9q4z41pbKWES";
        assert!(DashPayContactCode::parse(example).is_none());

        let long = "2".repeat(MAX_DAPK_LEN + 1);
        for bad in [
            // DIP-15's merchant form is a payment.
            format!("dash:Xcu5iYBH3szP744sQ1RUp3JHTVFHrFVdYu?amount=0.11&du=bob&dapk={dapk}"),
            format!("dash:?du=bob&dapk={dapk}&amount=1"),
            format!("dash:?du=bob&du=bob&dapk={dapk}"),
            format!("dash:?du=bob&dapk={dapk}&dapk={dapk}"),
            format!("dash:?du=&dapk={dapk}"),
            format!("dash:?du=.dash&dapk={dapk}"),
            format!("dash:?du=bob&dapk={dapk}&"),
            format!("dash:?du=bob&dapk=%31{}", &dapk[1..]),
            format!("dash:?du&dapk={dapk}"),
            format!("dash:?dapk={dapk}"),
            "dash:?du=bob".to_owned(),
            "dash:?du=bob&dapk=0OIl".to_owned(),
            format!("dash:?du=bob&dapk={long}"),
            format!("dash://x?du=bob&dapk={dapk}"),
            format!("dash:?du=bob&dapk={dapk}#f"),
            format!("dashpay:?du=bob&dapk={dapk}"),
            "dash:".to_owned(),
        ] {
            assert!(DashPayContactCode::parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn contact_code_debug_hides_the_key() {
        let code =
            DashPayContactCode::parse(&format!("dash:?du=bob&dapk={}", base58(&key_blob(1))))
                .unwrap();
        assert_eq!(
            format!("{code:?}"),
            r#"DashPayContactCode { username: "bob", expiry: 1, key: ".." }"#
        );
    }
}
