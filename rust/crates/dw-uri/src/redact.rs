//! Log redaction of links (DASHPAY §3.8, DP5-01 D14): every link the
//! wallet reads, and anything that looks like a part of an invitation or a
//! contact code, is replaced by [`REDACTED`] before a log line is written.
//!
//! A line is cut at the first link in it, and the rest of the line is
//! replaced: parsers accept quotes, brackets and more inside a link, so no
//! delimiter but the end of the line safely ends one (the stash refuses
//! links with control characters, so a kept link is on one line). The
//! search looks at *tokens*, runs of characters between whitespace, quotes,
//! backticks and angle brackets: a link starts at the first link scheme in
//! a token (`dash:`, `pay:`, `dashpay:`, `dashwallet:`, `dash-key:`,
//! `dash-st:`, case-insensitive, not preceded by a letter, digit, `+`, `-`
//! or `.`, nor followed by another `:` as in a Rust path), or at the start
//! of a token that names an invitation host (the invitations site,
//! AppsFlyer's OneLink hosts) or a parameter that carries a secret or a
//! payee (`pk=`, `dapk=`, `assetlocktx=`, `islock=`, `du=`, also a
//! percent-encoded `dashpay%3a`). Lines without a link are left as they
//! are, so logs stay readable.
//!
//! [`RedactingWriter`] applies it to a `std::io::Write` sink one log event
//! at a time: tracing's fmt layer makes one writer per event, writes the
//! event into it and drops it.

use std::borrow::Cow;
use std::io::{self, Write};

/// What a redacted link reads as in a log.
pub const REDACTED: &str = "<link>";

const SCHEMES: [&str; 6] = [
    "dashwallet:",
    "dash-key:",
    "dash-st:",
    "dashpay:",
    "dash:",
    "pay:",
];

/// Markers that make a whole token a link: invitation hosts, the
/// parameters of invitations and contact codes, and an encoded `dashpay:`.
const MARKERS: [&str; 10] = [
    "invitations.dashpay.io",
    "onelink.me",
    "appsflyer",
    "dashpay%3a",
    "pk=",
    "dapk=",
    "assetlocktx=",
    "islock=",
    "du=",
    "deep_link_value=",
];

fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '<' | '>')
}

/// Where the redaction of `token` starts, if anywhere.
fn redact_from(token: &str) -> Option<usize> {
    let lower = token.to_ascii_lowercase();
    if MARKERS.iter().any(|m| lower.contains(m)) {
        return Some(0);
    }
    let bytes = lower.as_bytes();
    SCHEMES
        .iter()
        .flat_map(|scheme| lower.match_indices(scheme).map(|(i, m)| (i, i + m.len())))
        .filter(|&(i, end)| {
            // `dashpay::overlay` is a Rust path, not a link.
            let path = bytes.get(end) == Some(&b':');
            let word = i > 0 && {
                let b = bytes[i - 1];
                b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')
            };
            !path && !word
        })
        .map(|(i, _)| i)
        .min()
}

/// The length of the terminal escape sequence at the start of `rest`
/// (which starts with ESC): a CSI sequence (`ESC [`, parameters,
/// intermediates and, with `final_byte`, the final byte), or the ESC
/// alone. All ASCII.
fn escape_len(rest: &str, final_byte: bool) -> usize {
    let b = rest.as_bytes();
    if b.get(1) != Some(&b'[') {
        return 1;
    }
    let mut i = 2;
    while b.get(i).is_some_and(|c| (0x30..=0x3f).contains(c)) {
        i += 1;
    }
    while b.get(i).is_some_and(|c| (0x20..=0x2f).contains(c)) {
        i += 1;
    }
    i + usize::from(final_byte && b.get(i).is_some_and(|c| (0x40..=0x7e).contains(c)))
}

/// Where the first link in `line` (one line, no terminator) starts. A
/// terminal escape sequence (a coloured log's `ESC[2m`) delimits tokens
/// whole, so its final letter never hides the scheme after it (review r1
/// #1); a line with an ESC is also read with the sequence's final byte
/// left in the next token, so a bare `ESC[` cannot take a scheme's first
/// letter for its final byte. The earlier start wins.
fn link_start(line: &str) -> Option<usize> {
    let whole = scan(line, true);
    if !line.contains('\x1b') {
        return whole;
    }
    match (whole, scan(line, false)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// [`link_start`] with escape sequences read with or without their final
/// byte.
fn scan(line: &str, final_byte: bool) -> Option<usize> {
    let mut start = 0;
    let mut i = 0;
    loop {
        let (step, delimits) = match line[i..].chars().next() {
            None => (0, true),
            Some('\x1b') => (escape_len(&line[i..], final_byte), true),
            Some(c) => (c.len_utf8(), is_delimiter(c)),
        };
        if delimits {
            if start < i
                && let Some(from) = redact_from(&line[start..i])
            {
                return Some(start + from);
            }
            if i == line.len() {
                return None;
            }
            start = i + step;
        }
        i += step;
    }
}

/// Whether `link` (one line) reads as a link from its first character
/// on: what the link stash requires of a link it keeps, so a kept link is
/// cut whole in a log line wherever a delimiter, an escape sequence or any
/// character but a letter, digit, `+`, `-` or `.` precedes it (an
/// invitation's markers match anywhere). `dash::x`, a Rust path to the
/// redaction, is not one, whatever a parser makes of it.
pub fn redacted_whole(link: &str) -> bool {
    link_start(link) == Some(0)
}

/// `text` with every line that holds a link cut at the link, the rest of
/// the line replaced by [`REDACTED`]; borrowed when nothing needed it.
pub fn redact_links(text: &str) -> Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut line_start = 0;
    for line in text.split_inclusive(['\n', '\r']) {
        let body = line.trim_end_matches(['\n', '\r']);
        if let Some(at) = link_start(body) {
            let o = out.get_or_insert_with(|| String::with_capacity(text.len()));
            o.push_str(&text[copied..line_start + at]);
            o.push_str(REDACTED);
            copied = line_start + body.len();
        }
        line_start += line.len();
    }
    match out {
        None => Cow::Borrowed(text),
        Some(mut o) => {
            o.push_str(&text[copied..]);
            Cow::Owned(o)
        }
    }
}

/// Makes panics print redacted: a panic message may quote a link (an
/// `expect` on a parse, a formatted assert), and the default hook prints
/// it as it is (review r1 #2). Prints what the default hook does, the
/// thread, the location and the message, each line redacted, and the
/// backtrace when `RUST_BACKTRACE` asks for one.
pub fn redact_panics() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let text = format!("thread '{name}' {info}\n");
        let backtrace = std::backtrace::Backtrace::capture();
        let mut err = io::stderr().lock();
        let _ = err.write_all(redact_links(&text).as_bytes());
        if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            let _ = err.write_all(redact_links(&format!("{backtrace}\n")).as_bytes());
        }
    }));
}

/// A writer that keeps one log event and writes it redacted to `inner`
/// when flushed or dropped. Bytes that are not UTF-8 are replaced first.
pub struct RedactingWriter<W: Write> {
    inner: W,
    buf: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    pub fn new(inner: W) -> Self {
        Self {
            inner,
            buf: Vec::new(),
        }
    }

    fn emit(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&self.buf);
        let redacted = redact_links(&text);
        let result = self.inner.write_all(redacted.as_bytes());
        drop(redacted);
        drop(text);
        self.buf.clear();
        result.and_then(|()| self.inner.flush())
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        let _ = self.emit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String {
        redact_links(s).into_owned()
    }

    #[test]
    fn links_are_redacted_and_the_rest_kept() {
        assert_eq!(r("no links here: 3 peers"), "no links here: 3 peers");
        assert!(matches!(redact_links("plain"), Cow::Borrowed(_)));
        assert_eq!(
            r("routing dash:yAddr?amount=1&label=x now"),
            "routing <link>"
        );
        assert_eq!(r("link=\"PAY:yAddr\""), "link=\"<link>");
        // Parsers take quotes inside a link: nothing after one is kept.
        assert_eq!(r("a \"dash:y\"?label=secret\" b"), "a \"<link>");
        assert_eq!(
            r("got dashpay://invite?du=a&pk=s and dashpay://user?id=1&username=b"),
            "got <link>"
        );
        assert_eq!(r("x=dashwallet://pay=y&sender=z"), "x=<link>");
        assert_eq!(r("dash-key:abc dash-st:def"), "<link>");
        assert_eq!(
            r("one dash:a\r\ntwo\nthree pay:b\n"),
            "one <link>\r\ntwo\nthree <link>\n"
        );
        assert_eq!(
            r("https://invitations.dashpay.io/applink?du=a&pk=s"),
            "<link>"
        );
        assert_eq!(
            r("https://x.onelink.me/a?af_dp=dashpay%3A%2F%2Finvite"),
            "<link>"
        );
        assert_eq!(r("query ?du=alice&dapk=abc tail"), "query <link>");
        assert_eq!(r("text\tdash:yA\ny"), "text\t<link>\ny");
        // Not schemes: a word ending in one, a hostname.
        assert_eq!(r("repay: 3 toolbar:pay"), "repay: 3 toolbar:pay");
        assert_eq!(r("a.dash: b+pay: c-dash:"), "a.dash: b+pay: c-dash:");
        assert_eq!(r("error (dash:x)"), "error (<link>");
        assert_eq!(
            r("unimplemented=[\"dashpay::overlay\"] dash::x"),
            "unimplemented=[\"dashpay::overlay\"] dash::x"
        );
        assert_eq!(r("dashpay::a dashpay:x"), "dashpay::a <link>");
        assert_eq!(r("é dash:é"), "é <link>");
    }

    #[test]
    fn a_whole_link_is_cut_from_its_start() {
        assert!(redacted_whole("dash:yAddr?amount=1"));
        assert!(redacted_whole(
            "https://invitations.dashpay.io/applink?du=a"
        ));
        assert!(!redacted_whole("dash::yAddr?amount=1"));
        assert!(!redacted_whole("x dash:yAddr"));
        assert!(!redacted_whole("https://example.com/pay?amount=1"));
        assert!(!redacted_whole(""));
    }

    /// Review r1 #1: a coloured log puts escape sequences around a field's
    /// `=`, right before the value; the sequence's final letter is no
    /// word before the scheme.
    #[test]
    fn escape_sequences_never_hide_a_link() {
        assert_eq!(
            r("link\x1b[2m=\x1b[0mdash:yAddr?label=secret tail"),
            "link\x1b[2m=\x1b[0m<link>"
        );
        assert_eq!(r("\x1b[31mpay:x\x1b[0m"), "\x1b[31m<link>");
        // A marker redacts its whole token: here from the final byte on.
        assert_eq!(
            r("a\x1b[1;32;4mdashpay://invite?pk=s"),
            "a\x1b[1;32;4<link>"
        );
        assert_eq!(r("\x1bdash:x"), "\x1b<link>");
        assert_eq!(r("\x1b[2m"), "\x1b[2m");
        assert_eq!(r("x\x1b["), "x\x1b[");
        assert_eq!(r("\x1b[2mrepay: 3\x1b[0m"), "\x1b[2mrepay: 3\x1b[0m");
        // A bare `ESC[` would take the scheme's `d` for its final byte.
        assert_eq!(r("x\x1b[dash:yAddr?label=secret"), "x\x1b[<link>");
        assert_eq!(r("x\x1b[1;2pay:y"), "x\x1b[1;2<link>");
    }

    #[test]
    fn the_writer_redacts_each_event() {
        let mut out = Vec::new();
        {
            let mut w = RedactingWriter::new(&mut out);
            w.write_all(b"one dashpay://invite?pk=").unwrap();
            w.write_all(b"secret\n").unwrap();
        }
        {
            let mut w = RedactingWriter::new(&mut out);
            w.write_all(b"two\n").unwrap();
            w.flush().unwrap();
            w.write_all(b"three pay:x\n").unwrap();
        }
        {
            let mut w = RedactingWriter::new(&mut out);
            w.write_all(b"four \x1b[3mlink\x1b[0m\x1b[2m=\x1b[0mdash:y")
                .unwrap();
            w.write_all(b"?label=secret\n").unwrap();
        }
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "one <link>\ntwo\nthree <link>\nfour \x1b[3mlink\x1b[0m\x1b[2m=\x1b[0m<link>\n"
        );
    }
}
