//! The wider payment-string parser and the `dash:` writer of the iOS wallet
//! (dashwallet-ios `BIP70URI.swift`, `PaymentURIBuilder.swift`).
//!
//! This parser accepts everything dash-qt's does and more: `pay:` and
//! `dashwallet:` schemes, an optional `//`, `&` as the query separator, the
//! BIP72 `r=` URL and the `sender`, `user`, `currency` and `local` keys. Use
//! it for QR scans, pasted text and deep links; use `crate::core` where
//! dash-qt's exact behaviour is required (Open URI dialog, IPC hand-off).
//!
//! Differences from the iOS code, all deliberate:
//! - Amounts must be a complete decimal number (`1.5`, `1e2`); Foundation's
//!   `Decimal(string:)` also accepts a number followed by junk (`1abc`).
//! - `r=` is kept as text; the caller validates it before fetching.
//! - Values are percent-decoded with Foundation's `removingPercentEncoding`
//!   rules: an invalid escape or non-UTF-8 result leaves the value raw.

use std::collections::BTreeSet;

/// How the input classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaymentKind {
    /// An explicit `dash:`/`pay:`/`dashwallet:`/`bitcoin:` URI.
    PaymentUri,
    /// A string without a scheme: an address or key candidate, unvalidated.
    BareAddress,
    /// A plain `http(s)` BIP73 payment-request URL.
    Bip73Url,
}

/// A parsed payment string. Nothing here is validated against a network.
#[derive(Debug, Clone, PartialEq)]
pub struct PaymentRequest {
    pub kind: PaymentKind,
    /// `dash` (also for `pay:`/`dashwallet:`), `bitcoin`, or `http`/`https`.
    pub scheme: String,
    /// Percent-decoded address text; `None` for `dash:?r=…`.
    pub address: Option<String>,
    /// Duffs, rounded up from the decimal DASH value; `None` when absent or
    /// not a non-negative number.
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
    /// BIP72 payment-request URL (`r=`), or the whole BIP73 URL.
    pub r: Option<String>,
    /// `sender=`: the x-callback-url scheme of the requesting app.
    pub callback_scheme: Option<String>,
    /// `user=`: DashPay username.
    pub dashpay_username: Option<String>,
    /// `currency=`: requested fiat currency code.
    pub fiat_currency_code: Option<String>,
    /// `local=`: requested fiat amount (`NSString.floatValue` semantics).
    pub fiat_amount: Option<f32>,
    /// Keys that carried the `req-` prefix, without it.
    pub required_fields: BTreeSet<String>,
}

impl PaymentRequest {
    /// True when the request is a BIP70 payment request (has `r`).
    pub fn is_bip70(&self) -> bool {
        self.r.is_some()
    }

    fn empty(kind: PaymentKind, scheme: &str) -> PaymentRequest {
        PaymentRequest {
            kind,
            scheme: scheme.to_owned(),
            address: None,
            amount: None,
            label: None,
            message: None,
            r: None,
            callback_scheme: None,
            dashpay_username: None,
            fiat_currency_code: None,
            fiat_amount: None,
            required_fields: BTreeSet::new(),
        }
    }
}

/// Foundation's `String.removingPercentEncoding`: `None` when an escape is
/// malformed or the decoded bytes are not UTF-8.
pub fn remove_percent_encoding(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(
                u8::from_str_radix(hex, 16)
                    .ok()
                    .filter(|_| hex.bytes().all(|c| c.is_ascii_hexdigit()))?,
            );
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn decode_or_raw(s: &str) -> String {
    remove_percent_encoding(s).unwrap_or_else(|| s.to_owned())
}

/// `BIP70URI(_:)`: an explicit payment URI, or `None`.
pub fn parse_payment_uri(raw: &str) -> Option<PaymentRequest> {
    let trimmed = raw.trim();
    let colon = trimmed.find(':')?;
    let scheme = match trimmed[..colon].to_lowercase().as_str() {
        "pay" | "dash" | "dashwallet" => "dash",
        "bitcoin" => "bitcoin",
        _ => return None,
    };
    let mut req = PaymentRequest::empty(PaymentKind::PaymentUri, scheme);
    let mut rest = &trimmed[colon + 1..];
    if let Some(r) = rest.strip_prefix("//") {
        rest = r;
    }
    // Wallets in the wild write `dash:<address>&amount=…`; whichever of `?`
    // and `&` comes first ends the address.
    let (address_part, query_part) = match rest.find(['?', '&']) {
        Some(q) => (&rest[..q], Some(&rest[q + 1..])),
        None => (rest, None),
    };
    if !address_part.is_empty() {
        req.address = Some(decode_or_raw(address_part));
    }
    for pair in query_part
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        let (raw_key, raw_value) = match pair.find('=') {
            Some(i) => (&pair[..i], Some(&pair[i + 1..])),
            None => (pair, None),
        };
        let key = decode_or_raw(raw_key);
        let value = raw_value.map(decode_or_raw).unwrap_or_default();
        if let Some(stripped) = key.strip_prefix("req-") {
            req.required_fields.insert(stripped.to_owned());
        }
        match key.strip_prefix("req-").unwrap_or(&key) {
            "amount" => req.amount = duffs_from_decimal_dash(&value),
            "label" => req.label = Some(value),
            "message" => req.message = Some(value),
            "r" => req.r = (!value.is_empty()).then_some(value),
            "sender" => req.callback_scheme = Some(value),
            "user" => req.dashpay_username = Some(value),
            "currency" => req.fiat_currency_code = Some(value),
            "local" => req.fiat_amount = Some(ns_string_float_value(&value)),
            _ => {}
        }
    }
    Some(req)
}

/// `BIP70URI(paymentString:)`: also accepts bare (schemeless) candidates
/// and plain `http(s)` BIP73 URLs. `None` for empty input and for other
/// schemes (`mailto:` etc.).
pub fn parse_payment_string(raw: &str) -> Option<PaymentRequest> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let Some(colon) = trimmed.find(':') else {
        let mut req = PaymentRequest::empty(PaymentKind::BareAddress, "dash");
        req.address = Some(trimmed.to_owned());
        return Some(req);
    };
    if let Some(req) = parse_payment_uri(trimmed) {
        return Some(req);
    }
    let scheme = trimmed[..colon].to_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let mut req = PaymentRequest::empty(PaymentKind::Bip73Url, &scheme);
    req.r = Some(trimmed.replace(' ', "%20"));
    Some(req)
}

/// Decimal DASH text to duffs, rounding sub-duff remainders up. Accepts
/// `[+-]digits[.digits][e[+-]digits]` with at least one digit; the value
/// must not be negative and must fit in `u64` duffs.
pub fn duffs_from_decimal_dash(value: &str) -> Option<u64> {
    let b = value.as_bytes();
    let mut i = 0;
    let negative = match b.first() {
        Some(b'-') => {
            i = 1;
            true
        }
        Some(b'+') => {
            i = 1;
            false
        }
        _ => false,
    };
    let mut digits = String::new();
    let mut frac_len: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        digits.push(char::from(b[i]));
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            digits.push(char::from(b[i]));
            frac_len += 1;
            i += 1;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let mut exponent: i64 = 0;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        // `i64::from_str` takes an optional sign and requires digits.
        exponent = value[i..].parse().ok()?;
        i = b.len();
    }
    if i != b.len() {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    if negative {
        return None;
    }
    // value = digits × 10^(exponent − frac_len); duffs = value × 10^8.
    let shift = exponent.checked_sub(frac_len)?.checked_add(8)?;
    if shift >= 0 {
        let base: u128 = digits.parse().ok()?;
        let pow = 10u128.checked_pow(u32::try_from(shift).ok()?)?;
        return u64::try_from(base.checked_mul(pow)?).ok();
    }
    let cut = usize::try_from(-shift).ok()?;
    if cut >= digits.len() {
        return Some(1);
    }
    let (int_part, frac_part) = digits.split_at(digits.len() - cut);
    let whole: u64 = int_part.parse().ok()?;
    let round_up = frac_part.bytes().any(|c| c != b'0');
    whole.checked_add(u64::from(round_up))
}

/// `NSString.floatValue`: the leading decimal number after optional
/// whitespace, 0 when there is none.
fn ns_string_float_value(s: &str) -> f32 {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    if end < b.len() && (b[end] == b'+' || b[end] == b'-') {
        end += 1;
    }
    let mut seen_digit = false;
    while end < b.len() && b[end].is_ascii_digit() {
        end += 1;
        seen_digit = true;
    }
    if end < b.len() && b[end] == b'.' {
        end += 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
            seen_digit = true;
        }
    }
    if !seen_digit {
        return 0.0;
    }
    if end < b.len() && (b[end] == b'e' || b[end] == b'E') {
        let mut e = end + 1;
        if e < b.len() && (b[e] == b'+' || b[e] == b'-') {
            e += 1;
        }
        if e < b.len() && b[e].is_ascii_digit() {
            while e < b.len() && b[e].is_ascii_digit() {
                e += 1;
            }
            end = e;
        }
    }
    t[..end].parse().unwrap_or(0.0)
}

/// Fields of a `dash:` URI written by the iOS receive screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PaymentUriBuilder {
    pub address: String,
    /// Duffs; 0 omits `amount`.
    pub amount: u64,
    pub label: Option<String>,
    pub message: Option<String>,
    pub request_url: Option<String>,
    pub fiat_currency_code: Option<String>,
    /// Emitted as `local=` (two decimals) when > 0 and a currency is set.
    pub fiat_amount: f32,
    pub dashpay_username: Option<String>,
}

impl PaymentUriBuilder {
    /// The URI, byte-for-byte as `PaymentURIBuilder.string` writes it:
    /// parameter order amount, label, message, r, currency, local, user;
    /// label/message/r/currency percent-encoded, amount/local/user raw.
    pub fn build(&self) -> String {
        let mut params: Vec<String> = Vec::new();
        if self.amount > 0 {
            params.push(format!("amount={}", dash_string_from_duffs(self.amount)));
        }
        let non_empty =
            |v: &Option<String>| v.as_deref().filter(|s| !s.is_empty()).map(str::to_owned);
        if let Some(label) = non_empty(&self.label) {
            params.push(format!("label={}", ios_query_encode(&label)));
        }
        if let Some(message) = non_empty(&self.message) {
            params.push(format!("message={}", ios_query_encode(&message)));
        }
        if let Some(r) = non_empty(&self.request_url) {
            params.push(format!("r={}", ios_query_encode(&r)));
        }
        if let Some(currency) = non_empty(&self.fiat_currency_code) {
            params.push(format!("currency={}", ios_query_encode(&currency)));
            if self.fiat_amount > 0.0 {
                params.push(format!("local={}", printf_2f(f64::from(self.fiat_amount))));
            }
        }
        if let Some(user) = non_empty(&self.dashpay_username) {
            params.push(format!("user={user}"));
        }
        let mut uri = format!("dash:{}", self.address);
        if !params.is_empty() {
            uri.push('?');
            uri.push_str(&params.join("&"));
        }
        uri
    }
}

/// Duffs as decimal DASH with trailing zeros dropped (`NSDecimalNumber.stringValue`):
/// 25_000_000 → `0.25`, 100_000_000 → `1`, 1 → `0.00000001`.
pub fn dash_string_from_duffs(duffs: u64) -> String {
    let whole = duffs / 100_000_000;
    let frac = duffs % 100_000_000;
    if frac == 0 {
        return whole.to_string();
    }
    let frac = format!("{frac:08}");
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

/// Percent-encodes with Foundation's `urlQueryAllowed` set minus `&` and `=`:
/// ASCII letters, digits and `!$'()*+,-./:;?@_~` stay; every other UTF-8
/// byte becomes `%XX` (upper-case hex).
pub fn ios_query_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"!$'()*+,-./:;?@_~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// C `printf("%.2f", v)`: the exact binary value rounded half-to-even at two
/// decimals (what Apple's and glibc's printf do); `inf`/`nan` for
/// non-finite values.
fn printf_2f(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_owned();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // An f64 has at most 1074 fractional binary digits, so 1080 decimals
    // print it exactly.
    let exact = format!("{:.1080}", v.abs());
    let (int_part, frac_part) = exact.split_once('.').expect("fixed-point output has a dot");
    let (keep, rest) = frac_part.split_at(2);
    let mut digits: Vec<u8> = int_part.bytes().chain(keep.bytes()).collect();
    let first = rest.as_bytes()[0];
    let beyond_half = rest[1..].bytes().any(|c| c != b'0');
    let last_odd = (digits[digits.len() - 1] - b'0') % 2 == 1;
    if first > b'5' || (first == b'5' && (beyond_half || last_odd)) {
        // Decimal increment with carry.
        let mut i = digits.len();
        loop {
            if i == 0 {
                digits.insert(0, b'1');
                break;
            }
            i -= 1;
            if digits[i] == b'9' {
                digits[i] = b'0';
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let split = digits.len() - 2;
    // printf prints the sign bit even when the value rounds to zero:
    // -0.001 and -0.0 both give "-0.00".
    let sign = if v.is_sign_negative() { "-" } else { "" };
    format!(
        "{sign}{}.{}",
        std::str::from_utf8(&digits[..split]).expect("ascii"),
        std::str::from_utf8(&digits[split..]).expect("ascii")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Cases from dashwallet-ios DashWalletTests/PaymentProtocolTests.swift.
    #[test]
    fn ios_bip70uri_cases() {
        let uri =
            parse_payment_uri("pay:Xabc?amount=0.001&r=http%3A%2F%2Fh%2Fpr&sender=ctx").unwrap();
        assert_eq!(uri.scheme, "dash");
        assert!(uri.is_bip70());
        assert_eq!(uri.r.as_deref(), Some("http://h/pr"));
        assert_eq!(uri.callback_scheme.as_deref(), Some("ctx"));
        assert_eq!(uri.amount, Some(100_000));
        assert_eq!(uri.address.as_deref(), Some("Xabc"));

        let uri = parse_payment_uri("dash:?r=http%3A%2F%2Fh%2Fpr").unwrap();
        assert!(uri.is_bip70());
        assert_eq!(uri.address, None);

        assert!(parse_payment_uri("http://foo").is_none());
        let uri = parse_payment_uri("dash:Xabc?user=alice&currency=USD&local=12.5&req-sender=ctx")
            .unwrap();
        assert_eq!(uri.dashpay_username.as_deref(), Some("alice"));
        assert_eq!(uri.fiat_currency_code.as_deref(), Some("USD"));
        assert_eq!(uri.fiat_amount, Some(12.5));
        assert_eq!(uri.callback_scheme.as_deref(), Some("ctx"));
        assert!(uri.required_fields.contains("sender"));

        assert_eq!(
            parse_payment_uri("dash:X?amount=0.000000001")
                .unwrap()
                .amount,
            Some(1)
        );
        assert_eq!(parse_payment_uri("dash:X?amount=-5").unwrap().amount, None);
        assert_eq!(
            parse_payment_uri("dash:X?amount=1e2").unwrap().amount,
            Some(10_000_000_000)
        );
        let uri =
            parse_payment_uri("dash:ybt3gVM6cM9WprG7bRTMst1YR2GnAbWGLr&amount=0.23243214").unwrap();
        assert_eq!(
            uri.address.as_deref(),
            Some("ybt3gVM6cM9WprG7bRTMst1YR2GnAbWGLr")
        );
        assert_eq!(uri.amount, Some(23_243_214));

        let s = parse_payment_string("http://h/pr with space").unwrap();
        assert_eq!(s.kind, PaymentKind::Bip73Url);
        assert_eq!(s.r.as_deref(), Some("http://h/pr%20with%20space"));
        assert!(parse_payment_string("mailto:foo@bar").is_none());
        assert!(parse_payment_string("   ").is_none());
        assert_eq!(
            parse_payment_string("certainly not dash").unwrap().kind,
            PaymentKind::BareAddress
        );
        let s = parse_payment_string("dash://Xabc?amount=1.5").unwrap();
        assert_eq!(
            (s.address.as_deref(), s.amount),
            (Some("Xabc"), Some(150_000_000))
        );
    }

    #[test]
    fn ios_builder_cases() {
        let a = "yeRZBWYfeNE4yVUHV4ZLs83Ppn9aMRH57A";
        let b = |amount| PaymentUriBuilder {
            address: a.into(),
            amount,
            ..Default::default()
        };
        assert_eq!(b(0).build(), format!("dash:{a}"));
        assert_eq!(b(25_000_000).build(), format!("dash:{a}?amount=0.25"));
        assert_eq!(b(100_000_000).build(), format!("dash:{a}?amount=1"));
        assert_eq!(b(1).build(), format!("dash:{a}?amount=0.00000001"));
        let l = PaymentUriBuilder {
            label: Some("Sticker Pack".into()),
            ..b(10_000_000)
        };
        assert_eq!(
            l.build(),
            format!("dash:{a}?amount=0.1&label=Sticker%20Pack")
        );
        let c = PaymentUriBuilder {
            fiat_currency_code: Some("USD".into()),
            fiat_amount: 25.5,
            ..b(0)
        };
        assert_eq!(c.build(), format!("dash:{a}?currency=USD&local=25.50"));
        let u = PaymentUriBuilder {
            dashpay_username: Some("alice".into()),
            ..b(50_000_000)
        };
        assert_eq!(u.build(), format!("dash:{a}?amount=0.5&user=alice"));
    }

    #[test]
    fn printf_keeps_the_sign_of_negative_values() {
        // Review L4: printf("%.2f") keeps the minus sign when a negative value
        // rounds to zero, and for negative zero.
        assert_eq!(printf_2f(-0.001), "-0.00");
        assert_eq!(printf_2f(-0.0), "-0.00");
        assert_eq!(printf_2f(-0.005), "-0.01"); // -0.005000000000000000104…
        assert_eq!(printf_2f(-1.005), "-1.00");
        assert_eq!(printf_2f(-2.5), "-2.50");
        assert_eq!(printf_2f(0.0), "0.00");
        assert_eq!(printf_2f(0.001), "0.00");
    }

    #[test]
    fn printf_rounding_is_half_even_on_exact_value() {
        assert_eq!(printf_2f(0.125), "0.12");
        assert_eq!(printf_2f(0.375), "0.38");
        assert_eq!(printf_2f(2.5), "2.50");
        assert_eq!(printf_2f(1.005), "1.00"); // 1.005 is 1.00499999… in binary
        assert_eq!(printf_2f(f64::from(0.1f32)), "0.10");
        assert_eq!(printf_2f(9.995), "9.99"); // 9.99499… in binary
        assert_eq!(printf_2f(99.999), "100.00");
        assert_eq!(
            printf_2f(f64::from(f32::MAX)),
            "340282346638528859811704183484516925440.00"
        );
        assert_eq!(printf_2f(f64::INFINITY), "inf");
        assert_eq!(printf_2f(f64::NEG_INFINITY), "-inf");
        let huge = PaymentUriBuilder {
            address: "X".into(),
            fiat_currency_code: Some("USD".into()),
            fiat_amount: "1e39".parse().unwrap(),
            ..Default::default()
        };
        assert_eq!(huge.build(), "dash:X?currency=USD&local=inf");
    }

    #[test]
    fn decimal_amounts() {
        assert_eq!(duffs_from_decimal_dash("1.5"), Some(150_000_000));
        assert_eq!(duffs_from_decimal_dash("1abc"), None);
        assert_eq!(duffs_from_decimal_dash("-0"), Some(0));
        assert_eq!(duffs_from_decimal_dash("."), None);
        assert_eq!(duffs_from_decimal_dash("1e-9"), Some(1));
        assert_eq!(duffs_from_decimal_dash("1e30"), None);
        assert_eq!(duffs_from_decimal_dash("0.123456789"), Some(12_345_679));
    }
}
