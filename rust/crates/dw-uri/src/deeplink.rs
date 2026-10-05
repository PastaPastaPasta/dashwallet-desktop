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
        let b58 = dashcore::base58::encode_slice(&id);
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
    }
}
