//! `dashpay session` request lines, read without leaving a bearer input in
//! freed memory (review DW-E0-09 r1 finding 1).
//!
//! serde_json would decode into ordinary `String`s and `Value`s, plus an
//! escape scratch buffer, all freed unwiped. This reader handles the one
//! shape a request has, `{"args":[strings],"input":string|null,"id":…}`,
//! and decodes every string, keys included, into a zeroizing buffer sized
//! to the raw string first, so it never reallocates. Errors never quote the
//! line.
//!
//! [`ZeroStdin`] reads stdin into a zeroizing buffer instead of std's
//! stdin buffer, and [`bearer_shaped`] spots a bearer input put in `args`
//! before clap copies it.

use std::fs::File;
use std::io::{self, BufRead, Read};

use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

/// A parsed request.
pub(super) struct Request {
    pub(super) args: Vec<Zeroizing<String>>,
    pub(super) input: Option<Zeroizing<String>>,
}

/// What a line parses to: its `id` (when the line is a JSON object whose
/// `id` is a string or an integer) and the request or why it is refused.
pub(super) struct Parsed {
    pub(super) id: Option<Value>,
    pub(super) request: Result<Request, String>,
}

pub(super) const SHAPE: &str =
    r#"a request is {"args": [strings], "input"?: string, "id"?: string or integer}"#;

/// Nesting allowed in a value that is skipped (an unknown field's).
const MAX_DEPTH: usize = 32;

pub(super) fn parse(line: &[u8]) -> Parsed {
    let mut p = Parser { b: line, i: 0 };
    let mut fields = Fields::default();
    match p.object(&mut fields) {
        Err(Syntax(at)) => Parsed {
            id: None,
            request: Err(format!("malformed request at column {}", at + 1)),
        },
        Ok(()) => {
            let request = match (fields.refusal, fields.args) {
                (Some(why), _) => Err(why),
                (None, None) => Err(SHAPE.into()),
                (None, Some(args)) => Ok(Request {
                    args,
                    input: fields.input,
                }),
            };
            Parsed {
                id: fields.id,
                request,
            }
        }
    }
}

/// A syntax error at a byte offset.
struct Syntax(usize);

#[derive(Default)]
struct Fields {
    args: Option<Vec<Zeroizing<String>>>,
    input: Option<Zeroizing<String>>,
    id: Option<Value>,
    seen: [bool; 3],
    /// The first shape error; parsing goes on so `id` is still found.
    refusal: Option<String>,
}

impl Fields {
    fn refuse(&mut self, why: &str) {
        self.refusal.get_or_insert_with(|| why.to_string());
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err<T>(&self) -> Result<T, Syntax> {
        Err(Syntax(self.i))
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) -> Result<(), Syntax> {
        self.ws();
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            self.err()
        }
    }

    fn object(&mut self, f: &mut Fields) -> Result<(), Syntax> {
        self.eat(b'{')?;
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
        } else {
            loop {
                self.ws();
                let key = self.string()?;
                self.eat(b':')?;
                self.ws();
                self.field(&key, f)?;
                self.ws();
                match self.peek() {
                    Some(b',') => self.i += 1,
                    Some(b'}') => {
                        self.i += 1;
                        break;
                    }
                    _ => return self.err(),
                }
            }
        }
        self.ws();
        if self.i == self.b.len() {
            Ok(())
        } else {
            self.err()
        }
    }

    fn field(&mut self, key: &str, f: &mut Fields) -> Result<(), Syntax> {
        let slot = match key {
            "args" => 0,
            "input" => 1,
            "id" => 2,
            _ => {
                f.refuse("unknown request field");
                return self.skip(0);
            }
        };
        if std::mem::replace(&mut f.seen[slot], true) {
            f.refuse("duplicate request field");
            return self.skip(0);
        }
        match (slot, self.peek()) {
            (0, Some(b'[')) => {
                let args = self.strings(f)?;
                f.args = Some(args);
            }
            (1, Some(b'"')) => f.input = Some(self.string()?),
            (1 | 2, Some(b'n')) => self.literal(b"null")?,
            (2, Some(b'"')) => f.id = Some(Value::String(self.string()?.to_string())),
            (2, Some(b'-' | b'0'..=b'9')) => {
                let start = self.i;
                self.number()?;
                let digits = std::str::from_utf8(&self.b[start..self.i]).unwrap_or_default();
                match digits.parse::<i64>() {
                    Ok(n) => f.id = Some(n.into()),
                    Err(_) => f.refuse(SHAPE),
                }
            }
            _ => {
                f.refuse(SHAPE);
                self.skip(0)?;
            }
        }
        Ok(())
    }

    /// An array that must hold strings only.
    fn strings(&mut self, f: &mut Fields) -> Result<Vec<Zeroizing<String>>, Syntax> {
        self.eat(b'[')?;
        let mut out = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(out);
        }
        loop {
            self.ws();
            if self.peek() == Some(b'"') {
                out.push(self.string()?);
            } else {
                f.refuse(SHAPE);
                self.skip(1)?;
            }
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(out);
                }
                _ => return self.err(),
            }
        }
    }

    /// A JSON string, decoded into a zeroizing buffer that never grows:
    /// the decoded form is never longer than the raw one.
    fn string(&mut self) -> Result<Zeroizing<String>, Syntax> {
        if self.peek() != Some(b'"') {
            return self.err();
        }
        let start = self.i + 1;
        let mut j = start;
        loop {
            match self.b.get(j) {
                None => return Err(Syntax(j)),
                Some(b'"') => break,
                Some(b'\\') => j += 2,
                Some(_) => j += 1,
            }
        }
        let mut out = Zeroizing::new(Vec::with_capacity(j - start));
        let mut k = start;
        while k < j {
            let c = self.b[k];
            match c {
                b'\\' => {
                    let e = *self.b.get(k + 1).ok_or(Syntax(k))?;
                    k += 2;
                    let simple = match e {
                        b'"' => Some(b'"'),
                        b'\\' => Some(b'\\'),
                        b'/' => Some(b'/'),
                        b'b' => Some(8),
                        b'f' => Some(12),
                        b'n' => Some(b'\n'),
                        b'r' => Some(b'\r'),
                        b't' => Some(b'\t'),
                        b'u' => None,
                        _ => return Err(Syntax(k - 1)),
                    };
                    match simple {
                        Some(b) => out.push(b),
                        None => {
                            let ch = self.unicode_escape(&mut k)?;
                            let mut tmp = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
                            tmp.zeroize();
                        }
                    }
                }
                0..=0x1f => return Err(Syntax(k)),
                _ => {
                    out.push(c);
                    k += 1;
                }
            }
        }
        self.i = j + 1;
        let bytes = std::mem::take(&mut *out);
        match String::from_utf8(bytes) {
            Ok(s) => Ok(Zeroizing::new(s)),
            Err(e) => {
                e.into_bytes().zeroize();
                Err(Syntax(start))
            }
        }
    }

    /// The `XXXX` (and a low surrogate's `\uXXXX`) after `\u`; `k` points
    /// past `\u` and moves past what was read.
    fn unicode_escape(&self, k: &mut usize) -> Result<char, Syntax> {
        let hex = |at: usize| -> Result<u32, Syntax> {
            let digits = self.b.get(at..at + 4).ok_or(Syntax(at))?;
            let mut v = 0;
            for d in digits {
                v = v * 16 + (*d as char).to_digit(16).ok_or(Syntax(at))?;
            }
            Ok(v)
        };
        let hi = hex(*k)?;
        *k += 4;
        let code = if (0xD800..0xDC00).contains(&hi) {
            if self.b.get(*k..*k + 2) != Some(b"\\u") {
                return Err(Syntax(*k));
            }
            let lo = hex(*k + 2)?;
            if !(0xDC00..0xE000).contains(&lo) {
                return Err(Syntax(*k));
            }
            *k += 6;
            0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
        } else {
            hi
        };
        char::from_u32(code).ok_or(Syntax(*k))
    }

    fn number(&mut self) -> Result<(), Syntax> {
        let digits = |p: &mut Self| {
            let s = p.i;
            while matches!(p.peek(), Some(b'0'..=b'9')) {
                p.i += 1;
            }
            if p.i == s { p.err() } else { Ok(()) }
        };
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        digits(self)?;
        if self.peek() == Some(b'.') {
            self.i += 1;
            digits(self)?;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            digits(self)?;
        }
        Ok(())
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), Syntax> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(())
        } else {
            self.err()
        }
    }

    /// Skips any value; its strings are decoded into zeroizing buffers and
    /// dropped.
    fn skip(&mut self, depth: usize) -> Result<(), Syntax> {
        if depth > MAX_DEPTH {
            return self.err();
        }
        self.ws();
        match self.peek() {
            Some(b'"') => self.string().map(drop),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(open @ (b'[' | b'{')) => {
                let close = if open == b'[' { b']' } else { b'}' };
                self.i += 1;
                self.ws();
                if self.peek() == Some(close) {
                    self.i += 1;
                    return Ok(());
                }
                loop {
                    if open == b'{' {
                        self.ws();
                        self.string().map(drop)?;
                        self.eat(b':')?;
                    }
                    self.skip(depth + 1)?;
                    self.ws();
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(c) if c == close => {
                            self.i += 1;
                            return Ok(());
                        }
                        _ => return self.err(),
                    }
                }
            }
            _ => self.err(),
        }
    }
}

/// Whether `arg` looks like a bearer input: the patterns DASHPAY §3.8's log
/// redaction uses (`dashpay://invite`, `dapk=`), any `dash:` URI (DIP-15
/// contact payloads; no DashPay argument takes one) and the mobile
/// invitation link's key fields. Compared without making a copy.
pub(super) fn bearer_shaped(arg: &str) -> bool {
    const PATTERNS: [&[u8]; 7] = [
        b"dashpay://invite",
        b"dash:",
        b"dapk=",
        b"?pk=",
        b"&pk=",
        b"assetlocktx=",
        b"invitations.dashpay",
    ];
    let b = arg.as_bytes();
    PATTERNS
        .iter()
        .any(|p| b.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p)))
}

/// Stdin through a zeroizing buffer that is wiped as it is consumed. It
/// reads a duplicate of the stdin handle, made at the first read, so std's
/// own stdin buffer never holds a request line or a bearer input.
pub(crate) struct ZeroStdin {
    file: Option<File>,
    buf: Zeroizing<Vec<u8>>,
    pos: usize,
    end: usize,
}

impl ZeroStdin {
    pub(crate) fn new() -> Self {
        Self {
            file: None,
            buf: Zeroizing::new(vec![0; 16 * 1024]),
            pos: 0,
            end: 0,
        }
    }

    fn file(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            #[cfg(unix)]
            let file = {
                use std::os::fd::AsFd;
                File::from(io::stdin().as_fd().try_clone_to_owned()?)
            };
            #[cfg(windows)]
            let file = {
                use std::os::windows::io::AsHandle;
                File::from(io::stdin().as_handle().try_clone_to_owned()?)
            };
            self.file = Some(file);
        }
        Ok(self.file.as_mut().expect("set above"))
    }
}

impl Read for ZeroStdin {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let avail = self.fill_buf()?;
            let n = avail.len().min(out.len());
            out[..n].copy_from_slice(&avail[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for ZeroStdin {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.pos == self.end {
            let mut buf = std::mem::take(&mut self.buf);
            let read = self.file().and_then(|f| f.read(&mut buf));
            self.buf = buf;
            self.end = read?;
            self.pos = 0;
        }
        Ok(&self.buf[self.pos..self.end])
    }

    fn consume(&mut self, amt: usize) {
        let amt = amt.min(self.end - self.pos);
        self.buf[self.pos..self.pos + amt].zeroize();
        self.pos += amt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(line: &str) -> (Option<Value>, Vec<String>, Option<String>) {
        let p = parse(line.as_bytes());
        let r = p.request.unwrap_or_else(|e| panic!("{line}: {e}"));
        (
            p.id,
            r.args.iter().map(|a| a.to_string()).collect(),
            r.input.map(|i| i.to_string()),
        )
    }

    fn refused(line: &str) -> (Option<Value>, String) {
        let p = parse(line.as_bytes());
        (
            p.id,
            p.request.err().unwrap_or_else(|| panic!("{line} parsed")),
        )
    }

    #[test]
    fn parses_requests() {
        assert_eq!(
            ok(r#" {"args":["a","b"],"input":"x","id":7} "#),
            (
                Some(7.into()),
                vec!["a".into(), "b".into()],
                Some("x".into())
            )
        );
        assert_eq!(
            ok(r#"{"id":"q","input":null,"args":[]}"#),
            (Some("q".into()), vec![], None)
        );
        // Escapes, a surrogate pair and raw UTF-8 decode as serde_json does.
        let line = r#"{"args":["\"\\\/\b\f\n\r\tAé😀é"]}"#;
        let want: String = serde_json::from_str::<Value>(line).unwrap()["args"][0]
            .as_str()
            .unwrap()
            .into();
        assert_eq!(ok(line).1, [want]);
        assert_eq!(ok(r#"{"args":[],"id":-12}"#).0, Some((-12).into()));
    }

    #[test]
    fn refusals_keep_the_id() {
        for line in [
            r#"{"args":false,"id":2}"#,
            r#"{"args":["a",3],"id":2}"#,
            r#"{"args":[],"input":["x"],"id":2}"#,
            r#"{"args":[],"extra":{"a":[1,{"b":null}]},"id":2}"#,
            r#"{"args":[],"args":[],"id":2}"#,
            r#"{"input":"x","id":2}"#,
        ] {
            assert_eq!(refused(line).0, Some(2.into()), "{line}");
        }
        assert_eq!(refused(r#"{"args":[],"id":1.5}"#), (None, SHAPE.into()));
        assert_eq!(refused(r#"{"args":[],"id":true}"#), (None, SHAPE.into()));
    }

    #[test]
    fn malformed_lines_say_only_where() {
        for (line, col) in [
            ("", 1),
            ("[]", 1),
            (r#"{"args":[]"#, 11),
            (r#"{"args":[]} x"#, 13),
            (r#"{"args":["a\q"]}"#, 13),
            (r#"{"args":["\ud800"]}"#, 17),
            ("{\"args\":[\"a\u{1}\"]}", 12),
            (r#"{"args":["unterminated]}"#, 25),
        ] {
            let (id, why) = refused(line);
            assert_eq!(
                (id, why),
                (None, format!("malformed request at column {col}")),
                "{line}"
            );
        }
        // Invalid UTF-8 inside a string.
        let p = parse(b"{\"args\":[\"\xff\"]}");
        assert!(p.request.err().unwrap().starts_with("malformed request"));
        // Deep nesting in a skipped value is refused, not a stack overflow.
        let deep = format!(r#"{{"x":{}{}}}"#, "[".repeat(100), "]".repeat(100));
        assert!(refused(&deep).1.starts_with("malformed request"));
    }

    #[test]
    fn bearer_shapes() {
        for s in [
            "dashpay://invite?x",
            "DASH:?du=a&DAPK=b",
            "https://invitations.dashpay.io/applink?du=a&assetlocktx=b&pk=c",
            "x?pk=1",
            "dash:?invite=x",
        ] {
            assert!(bearer_shaped(s), "{s}");
        }
        for s in [
            "dashpay://user?id=a&username=b",
            "--invitation-id",
            "alice",
            "pkg=1",
        ] {
            assert!(!bearer_shaped(s), "{s}");
        }
    }
}
