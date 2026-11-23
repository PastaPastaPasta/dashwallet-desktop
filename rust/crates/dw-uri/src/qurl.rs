//! The subset of Qt 5.15 `QUrl` (TolerantMode) and `QUrlQuery` behaviour that
//! `GUIUtil::parseBitcoinURI` depends on.
//!
//! Qt's rules, as observed by `testdata/oracle/qt` and recorded in
//! `testdata/uri_cases.json`:
//! - The scheme is `ALPHA *(ALPHA / DIGIT / "+" / "-" / ".")` before the first
//!   `:`, compared lower-cased. Anything else (leading whitespace included)
//!   leaves the URL without a scheme.
//! - TolerantMode repairs percent signs per component: if any `%` in the
//!   path (or query) is not followed by two hex digits, every `%` in that
//!   component is taken literally.
//! - `path()` is fully decoded. Bytes that do not form valid UTF-8 become one
//!   U+FFFD each; noncharacters (U+FDD0–U+FDEF, U+xFFFE, U+xFFFF) count as
//!   invalid.
//! - `QUrlQuery::queryItems()` splits on raw `&` and the first raw `=`, then
//!   decodes `%XX` only for unreserved characters, space, `"#&<=>\^`{|}` and
//!   valid non-ASCII UTF-8. Everything else stays encoded with upper-case hex,
//!   a literal `%` becomes `%25`, and raw control characters are encoded.
//!   `+` is not a space.
//! - With an authority (`dash://…`), the path starts after it. A host with
//!   characters outside RFC 3986 reg-name, or a non-numeric port, makes the
//!   URL invalid.
//!
//! Known gap: Qt converts non-ASCII host names with IDNA and may accept them;
//! here any non-ASCII host is invalid. This only affects `dash://` input,
//! which dash-qt's URI handler rejects before it looks at the parse result.

/// A URL split into the parts `parseBitcoinURI` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Url<'a> {
    /// Lower-cased scheme.
    pub scheme: String,
    /// Raw path component.
    pub path: &'a str,
    /// Raw query component (without `?`), when present.
    pub query: Option<&'a str>,
}

/// Parses `input` the way `QUrl(input)` does. `None` means the URL has no
/// scheme or `QUrl::isValid()` would be false.
pub(crate) fn parse(input: &str) -> Option<Url<'_>> {
    let colon = input.find(':')?;
    let scheme = &input[..colon];
    let mut bytes = scheme.bytes();
    let first = bytes.next()?;
    if !first.is_ascii_alphabetic()
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'-' || b == b'.')
    {
        return None;
    }
    let mut rest = &input[colon + 1..];
    if let Some(after) = rest.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        if !authority_is_valid(&after[..end]) {
            return None;
        }
        rest = &after[end..];
    }
    let (before_fragment, _fragment) = match rest.find('#') {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let (path, query) = match before_fragment.find('?') {
        Some(i) => (&before_fragment[..i], Some(&before_fragment[i + 1..])),
        None => (before_fragment, None),
    };
    Some(Url {
        scheme: scheme.to_ascii_lowercase(),
        path,
        query,
    })
}

fn authority_is_valid(authority: &str) -> bool {
    let host_port = match authority.rfind('@') {
        Some(i) => &authority[i + 1..],
        None => authority,
    };
    let (host, port) = if let Some(inner) = host_port.strip_prefix('[') {
        let Some(close) = inner.find(']') else {
            return false;
        };
        let literal = &inner[..close];
        let valid_literal = literal.parse::<std::net::Ipv6Addr>().is_ok()
            || (literal.starts_with(['v', 'V']) && literal.contains('.'));
        if !valid_literal {
            return false;
        }
        let after = &inner[close + 1..];
        match after.strip_prefix(':') {
            Some(p) => ("", Some(p)),
            None if after.is_empty() => ("", None),
            None => return false,
        }
    } else {
        match host_port.rfind(':') {
            Some(i) => (&host_port[..i], Some(&host_port[i + 1..])),
            None => (host_port, None),
        }
    };
    if let Some(port) = port
        && !port.is_empty()
        && !(port.bytes().all(|b| b.is_ascii_digit())
            && port.parse::<u32>().is_ok_and(|p| p <= 65535))
    {
        return false;
    }
    host.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=%".contains(&b))
}

/// True when some `%` in `s` is not followed by two hex digits; Qt then treats
/// every `%` in the component as a literal character.
fn has_stray_percent(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| b[i] == b'%' && !is_hex_pair(b, i + 1))
}

fn is_hex_pair(b: &[u8], i: usize) -> bool {
    i + 1 < b.len() && b[i].is_ascii_hexdigit() && b[i + 1].is_ascii_hexdigit()
}

fn hex_val(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

/// Decodes one UTF-8 sequence from the start of `bytes` with Qt's rules:
/// shortest form only, no surrogates, at most U+10FFFF, no noncharacters.
/// Returns the character and the number of bytes used.
fn decode_utf8_qt(bytes: &[u8]) -> Option<(char, usize)> {
    let b0 = *bytes.first()?;
    let (len, min, init) = match b0 {
        0x00..=0x7F => return Some((char::from(b0), 1)),
        0xC0..=0xDF => (2, 0x80, u32::from(b0 & 0x1F)),
        0xE0..=0xEF => (3, 0x800, u32::from(b0 & 0x0F)),
        0xF0..=0xF7 => (4, 0x1_0000, u32::from(b0 & 0x07)),
        _ => return None,
    };
    if bytes.len() < len {
        return None;
    }
    let mut cp = init;
    for &b in &bytes[1..len] {
        if b & 0xC0 != 0x80 {
            return None;
        }
        cp = (cp << 6) | u32::from(b & 0x3F);
    }
    if cp < min || is_noncharacter(cp) {
        return None;
    }
    char::from_u32(cp).map(|c| (c, len))
}

fn is_noncharacter(cp: u32) -> bool {
    (0xFDD0..=0xFDEF).contains(&cp) || (cp & 0xFFFE) == 0xFFFE
}

/// Qt's `QString::fromUtf8` over bytes: invalid bytes become one U+FFFD each.
fn utf8_to_string_qt(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match decode_utf8_qt(&bytes[i..]) {
            Some((c, n)) => {
                out.push(c);
                i += n;
            }
            None => {
                out.push('\u{FFFD}');
                i += 1;
            }
        }
    }
    out
}

/// `QUrl::path()` (FullyDecoded) for a raw path component.
pub(crate) fn path_fully_decoded(path: &str) -> String {
    if has_stray_percent(path) {
        return path.to_owned();
    }
    let b = path.as_bytes();
    let mut bytes = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && is_hex_pair(b, i + 1) {
            bytes.push(hex_val(b[i + 1]) << 4 | hex_val(b[i + 2]));
            i += 3;
        } else {
            bytes.push(b[i]);
            i += 1;
        }
    }
    utf8_to_string_qt(&bytes)
}

/// ASCII characters that `QUrlQuery::queryItems()` returns decoded when they
/// arrive percent-encoded.
fn query_decodes(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b" -._~\"#&<=>\\^`{|}".contains(&b)
}

fn push_pct(out: &mut String, b: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push('%');
    out.push(char::from(HEX[usize::from(b >> 4)]));
    out.push(char::from(HEX[usize::from(b & 0xF)]));
}

/// Recodes one key or value of a query item. `stray` is the component-wide
/// TolerantMode flag from `has_stray_percent`.
fn recode_query_part(part: &str, stray: bool) -> String {
    let b = part.as_bytes();
    let mut out = String::with_capacity(part.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if stray || !is_hex_pair(b, i + 1) {
                out.push_str("%25");
                i += 1;
                continue;
            }
            let byte = hex_val(b[i + 1]) << 4 | hex_val(b[i + 2]);
            if byte < 0x80 {
                if query_decodes(byte) {
                    out.push(char::from(byte));
                } else {
                    push_pct(&mut out, byte);
                }
                i += 3;
                continue;
            }
            // Gather the run of consecutive %XX escapes and try to decode
            // one UTF-8 sequence from its start.
            let mut run = Vec::new();
            let mut j = i;
            while run.len() < 4 && j < b.len() && b[j] == b'%' && is_hex_pair(b, j + 1) {
                run.push(hex_val(b[j + 1]) << 4 | hex_val(b[j + 2]));
                j += 3;
            }
            match decode_utf8_qt(&run) {
                Some((c, n)) => {
                    out.push(c);
                    i += 3 * n;
                }
                None => {
                    push_pct(&mut out, byte);
                    i += 3;
                }
            }
            continue;
        }
        let c = part[i..]
            .chars()
            .next()
            .expect("index is on a char boundary");
        if c.is_ascii_control() {
            push_pct(&mut out, c as u8);
        } else {
            out.push(c);
        }
        i += c.len_utf8();
    }
    out
}

/// `QUrlQuery(url).queryItems()` for a raw query component. A key without
/// `=` gets an empty value (Qt returns a null string; both read as empty).
pub(crate) fn query_items(query: &str) -> Vec<(String, String)> {
    let stray = has_stray_percent(query);
    query
        .split('&')
        .map(|pair| match pair.find('=') {
            Some(i) => (
                recode_query_part(&pair[..i], stray),
                recode_query_part(&pair[i + 1..], stray),
            ),
            None => (recode_query_part(pair, stray), String::new()),
        })
        .collect()
}

/// `QUrl::toPercentEncoding(s)`: every UTF-8 byte except unreserved ASCII
/// (`A-Z a-z 0-9 - . _ ~`) becomes `%XX` with upper-case hex.
pub(crate) fn to_percent_encoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            push_pct(&mut out, b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_rules() {
        assert_eq!(parse("DaSh:x").unwrap().scheme, "dash");
        assert!(parse(" dash:x").is_none());
        assert!(parse("1dash:x").is_none());
        assert!(parse("dash").is_none());
    }

    #[test]
    fn stray_percent_is_component_wide() {
        assert_eq!(
            query_items("label=%41&message=%"),
            vec![
                ("label".to_owned(), "%2541".to_owned()),
                ("message".to_owned(), "%25".to_owned()),
            ]
        );
        assert_eq!(path_fully_decoded("X%zz"), "X%zz");
        assert_eq!(path_fully_decoded("%58"), "X");
    }

    #[test]
    fn invalid_utf8_is_one_replacement_per_byte() {
        assert_eq!(path_fully_decoded("%F0%9FA"), "\u{FFFD}\u{FFFD}A");
        assert_eq!(query_items("l=%F0%9FA")[0].1, "%F0%9FA");
    }

    #[test]
    fn percent_encoding_is_upper_case() {
        assert_eq!(to_percent_encoding("a b/é"), "a%20b%2F%C3%A9");
    }
}
