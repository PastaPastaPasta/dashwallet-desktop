//! UniValue's JSON model, reader and writer (MG-04b D7), so the session
//! messages read and render exactly as dash-qt's (rc.3 `src/univalue/`):
//!
//! - an object is an ordered list of keys and values; duplicate keys are
//!   kept, and lookup takes the first ([`Json::find`], `find_value`);
//! - a number keeps the token it was read as and is written back as that
//!   token; integers are read as `std::from_chars` reads them
//!   ([`Json::get_u64`], [`Json::get_i64`]);
//! - strings are bytes: the reader validates UTF-8 the way
//!   `JSONUTF8StringFilter` does, which re-encodes code points (an overlong
//!   sequence becomes its shortest form, a `\uD834\uDD1E` pair one code
//!   point), and the writer escapes with UniValue's table;
//! - [`Json::write`] is `UniValue::write(prettyIndent)`.
//!
//! Only the envelopes use this. Any divergence from UniValue would change
//! the message code dash-qt shows, so the module's tests run against
//! vectors UniValue itself produced (`testdata/oracle/univalue/`).

/// `MAX_JSON_DEPTH` (rc.3 `univalue/lib/univalue_read.cpp:20`).
const MAX_DEPTH: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number's token text.
    Num(String),
    Str(Vec<u8>),
    Arr(Vec<Json>),
    Obj(Vec<(Vec<u8>, Json)>),
}

impl Json {
    pub fn obj() -> Self {
        Json::Obj(Vec::new())
    }

    pub fn str(s: &str) -> Self {
        Json::Str(s.as_bytes().to_vec())
    }

    pub fn int(n: i64) -> Self {
        Json::Num(n.to_string())
    }

    pub fn uint(n: u64) -> Self {
        Json::Num(n.to_string())
    }

    /// `pushKV`: replaces the first value under `key`, else appends
    /// (rc.3 `univalue/lib/univalue.cpp:126-135`). Panics on a non-object,
    /// as UniValue throws.
    pub fn push_kv(&mut self, key: &str, value: Json) {
        let Json::Obj(entries) = self else {
            panic!("push_kv on a non-object");
        };
        match entries.iter_mut().find(|(k, _)| k == key.as_bytes()) {
            Some((_, v)) => *v = value,
            None => entries.push((key.as_bytes().to_vec(), value)),
        }
    }

    /// `push_back` on an array.
    pub fn push(&mut self, value: Json) {
        let Json::Arr(values) = self else {
            panic!("push on a non-array");
        };
        values.push(value);
    }

    /// `find_value`: the first value under `key`, or `None` (UniValue's
    /// null) for a non-object or a missing key.
    pub fn find(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries
                .iter()
                .find(|(k, _)| k == key.as_bytes())
                .map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(v) => Some(v),
            _ => None,
        }
    }

    pub fn is_object(&self) -> bool {
        matches!(self, Json::Obj(_))
    }

    /// A string value as text; `None` for another type or bytes that
    /// aren't UTF-8 (which UniValue can hold, see the module docs).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Json::Str(b) => std::str::from_utf8(b).ok(),
            _ => None,
        }
    }

    pub fn is_str(&self) -> bool {
        matches!(self, Json::Str(_))
    }

    pub fn is_num(&self) -> bool {
        matches!(self, Json::Num(_))
    }

    /// `getInt<uint64_t>()`: digits only, in range (`std::from_chars`, rc.3
    /// `univalue/include/univalue.h:137-147`).
    pub fn get_u64(&self) -> Option<u64> {
        match self {
            Json::Num(t) if !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()) => {
                t.parse().ok()
            }
            _ => None,
        }
    }

    /// `getInt<int64_t>()`: an optional `-`, then digits only, in range.
    pub fn get_i64(&self) -> Option<i64> {
        match self {
            Json::Num(t) => {
                let digits = t.strip_prefix('-').unwrap_or(t);
                if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                    return None;
                }
                t.parse().ok()
            }
            _ => None,
        }
    }

    /// `getInt<int>()` (32-bit).
    pub fn get_i32(&self) -> Option<i32> {
        self.get_i64().and_then(|n| i32::try_from(n).ok())
    }

    /// `getInt<uint32_t>()`.
    pub fn get_u32(&self) -> Option<u32> {
        self.get_u64().and_then(|n| u32::try_from(n).ok())
    }

    /// `UniValue::read`: `None` where it returns false.
    pub fn read(input: &[u8]) -> Option<Json> {
        Reader { s: input, pos: 0 }.read()
    }

    /// `UniValue::write(pretty_indent)`.
    pub fn write(&self, pretty_indent: usize) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_into(pretty_indent, 0, &mut out);
        out
    }

    fn write_into(&self, pretty: usize, indent_level: usize, out: &mut Vec<u8>) {
        let level = indent_level.max(1);
        match self {
            Json::Null => out.extend_from_slice(b"null"),
            Json::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Json::Num(t) => out.extend_from_slice(t.as_bytes()),
            Json::Str(s) => write_string(s, out),
            Json::Arr(values) => {
                out.push(b'[');
                if pretty > 0 {
                    out.push(b'\n');
                }
                for (i, v) in values.iter().enumerate() {
                    indent(pretty, level, out);
                    v.write_into(pretty, level + 1, out);
                    if i + 1 != values.len() {
                        out.push(b',');
                    }
                    if pretty > 0 {
                        out.push(b'\n');
                    }
                }
                indent(pretty, level - 1, out);
                out.push(b']');
            }
            Json::Obj(entries) => {
                out.push(b'{');
                if pretty > 0 {
                    out.push(b'\n');
                }
                for (i, (k, v)) in entries.iter().enumerate() {
                    indent(pretty, level, out);
                    write_string(k, out);
                    out.push(b':');
                    if pretty > 0 {
                        out.push(b' ');
                    }
                    v.write_into(pretty, level + 1, out);
                    if i + 1 != entries.len() {
                        out.push(b',');
                    }
                    if pretty > 0 {
                        out.push(b'\n');
                    }
                }
                indent(pretty, level - 1, out);
                out.push(b'}');
            }
        }
    }
}

fn indent(pretty: usize, level: usize, out: &mut Vec<u8>) {
    out.extend(std::iter::repeat_n(b' ', pretty * level));
}

/// `json_escape` with `escapes[256]` (rc.3 `univalue/lib/univalue_write.cpp:12-28`,
/// `univalue/include/univalue_escapes.h`).
fn write_string(s: &[u8], out: &mut Vec<u8>) {
    out.push(b'"');
    for &b in s {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            0x08 => out.extend_from_slice(b"\\b"),
            0x09 => out.extend_from_slice(b"\\t"),
            0x0a => out.extend_from_slice(b"\\n"),
            0x0c => out.extend_from_slice(b"\\f"),
            0x0d => out.extend_from_slice(b"\\r"),
            0x00..=0x1f | 0x7f => out.extend_from_slice(format!("\\u{b:04x}").as_bytes()),
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

/// `JSONUTF8StringFilter` (rc.3 `univalue/include/univalue_utffilter.h`),
/// state for state.
struct Utf8Filter {
    out: Vec<u8>,
    valid: bool,
    codepoint: u32,
    state: u32,
    surpair: u32,
}

impl Utf8Filter {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            valid: true,
            codepoint: 0,
            state: 0,
            surpair: 0,
        }
    }

    fn push_byte(&mut self, ch: u8) {
        if self.state == 0 {
            if ch < 0x80 {
                self.out.push(ch);
            } else if ch < 0xc0 {
                self.valid = false;
            } else if ch < 0xe0 {
                self.codepoint = u32::from(ch & 0x1f) << 6;
                self.state = 6;
            } else if ch < 0xf0 {
                self.codepoint = u32::from(ch & 0x0f) << 12;
                self.state = 12;
            } else if ch < 0xf8 {
                self.codepoint = u32::from(ch & 0x07) << 18;
                self.state = 18;
            } else {
                self.valid = false;
            }
        } else {
            if ch & 0xc0 != 0x80 {
                self.valid = false;
            }
            self.state -= 6;
            self.codepoint |= u32::from(ch & 0x3f) << self.state;
            if self.state == 0 {
                self.push_codepoint(self.codepoint);
            }
        }
    }

    fn push_codepoint(&mut self, cp: u32) {
        if self.state != 0 {
            self.valid = false;
        }
        if (0xd800..0xdc00).contains(&cp) {
            if self.surpair != 0 {
                self.valid = false;
            } else {
                self.surpair = cp;
            }
        } else if (0xdc00..0xe000).contains(&cp) {
            if self.surpair != 0 {
                self.append(0x10000 | ((self.surpair - 0xd800) << 10) | (cp - 0xdc00));
                self.surpair = 0;
            } else {
                self.valid = false;
            }
        } else if self.surpair != 0 {
            self.valid = false;
        } else {
            self.append(cp);
        }
    }

    fn append(&mut self, cp: u32) {
        if cp <= 0x7f {
            self.out.push(cp as u8);
        } else if cp <= 0x7ff {
            self.out.push((0xc0 | (cp >> 6)) as u8);
            self.out.push((0x80 | (cp & 0x3f)) as u8);
        } else if cp <= 0xffff {
            self.out.push((0xe0 | (cp >> 12)) as u8);
            self.out.push((0x80 | ((cp >> 6) & 0x3f)) as u8);
            self.out.push((0x80 | (cp & 0x3f)) as u8);
        } else if cp <= 0x1f_ffff {
            self.out.push((0xf0 | (cp >> 18)) as u8);
            self.out.push((0x80 | ((cp >> 12) & 0x3f)) as u8);
            self.out.push((0x80 | ((cp >> 6) & 0x3f)) as u8);
            self.out.push((0x80 | (cp & 0x3f)) as u8);
        }
    }

    fn finish(self) -> Option<Vec<u8>> {
        (self.valid && self.state == 0 && self.surpair == 0).then_some(self.out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    ObjOpen,
    ObjClose,
    ArrOpen,
    ArrClose,
    Colon,
    Comma,
    Null,
    True,
    False,
    Num(String),
    Str(Vec<u8>),
}

impl Token {
    /// `jsonTokenIsValue`.
    fn is_value(&self) -> bool {
        matches!(
            self,
            Token::Null | Token::True | Token::False | Token::Num(_) | Token::Str(_)
        )
    }
}

struct Reader<'a> {
    s: &'a [u8],
    pos: usize,
}

/// One open container while reading: an array, or an object with the key
/// waiting for its value.
enum Frame {
    Arr(Vec<Json>),
    Obj(Vec<(Vec<u8>, Json)>, Option<Vec<u8>>),
}

impl Reader<'_> {
    fn at(&self, i: usize) -> Option<u8> {
        self.s.get(i).copied()
    }

    /// `getJsonToken`: `Ok(None)` at the end, `Err` for `JTOK_ERR`
    /// (rc.3 `univalue/lib/univalue_read.cpp:54-240`).
    fn token(&mut self) -> Result<Option<Token>, ()> {
        while matches!(self.at(self.pos), Some(0x20 | 0x09 | 0x0a | 0x0d)) {
            self.pos += 1;
        }
        let Some(c) = self.at(self.pos) else {
            return Ok(None);
        };
        let simple = match c {
            b'{' => Some(Token::ObjOpen),
            b'}' => Some(Token::ObjClose),
            b'[' => Some(Token::ArrOpen),
            b']' => Some(Token::ArrClose),
            b':' => Some(Token::Colon),
            b',' => Some(Token::Comma),
            _ => None,
        };
        if let Some(t) = simple {
            self.pos += 1;
            return Ok(Some(t));
        }
        match c {
            b'n' | b't' | b'f' => {
                let rest = &self.s[self.pos..];
                for (word, tok, len) in [
                    (&b"null"[..], Token::Null, 4),
                    (b"true", Token::True, 4),
                    (b"false", Token::False, 5),
                ] {
                    if rest.starts_with(word) {
                        self.pos += len;
                        return Ok(Some(tok));
                    }
                }
                Err(())
            }
            b'-' | b'0'..=b'9' => self.number().map(Some),
            b'"' => self.string().map(Some),
            _ => Err(()),
        }
    }

    fn digits(&mut self, num: &mut String) {
        while let Some(d @ b'0'..=b'9') = self.at(self.pos) {
            num.push(d as char);
            self.pos += 1;
        }
    }

    fn number(&mut self) -> Result<Token, ()> {
        let first = self.pos;
        let first_digit = if self.s[first] == b'-' {
            first + 1
        } else {
            first
        };
        if self.at(first_digit) == Some(b'0')
            && self.at(first_digit + 1).is_some_and(|b| b.is_ascii_digit())
        {
            return Err(());
        }
        let mut num = String::new();
        num.push(self.s[first] as char);
        self.pos += 1;
        // UniValue checks for a digit after `-` only when one more byte
        // exists, so a lone `-` at the very end is a number token.
        if self.s[first] == b'-' && self.at(self.pos).is_some_and(|b| !b.is_ascii_digit()) {
            return Err(());
        }
        self.digits(&mut num);
        if self.at(self.pos) == Some(b'.') {
            num.push('.');
            self.pos += 1;
            if !self.at(self.pos).is_some_and(|b| b.is_ascii_digit()) {
                return Err(());
            }
            self.digits(&mut num);
        }
        if let Some(e @ (b'e' | b'E')) = self.at(self.pos) {
            num.push(e as char);
            self.pos += 1;
            if let Some(sign @ (b'-' | b'+')) = self.at(self.pos) {
                num.push(sign as char);
                self.pos += 1;
            }
            if !self.at(self.pos).is_some_and(|b| b.is_ascii_digit()) {
                return Err(());
            }
            self.digits(&mut num);
        }
        Ok(Token::Num(num))
    }

    fn string(&mut self) -> Result<Token, ()> {
        self.pos += 1;
        let mut filter = Utf8Filter::new();
        loop {
            let c = self.at(self.pos).filter(|c| *c >= 0x20).ok_or(())?;
            if c == b'\\' {
                self.pos += 1;
                let e = self.at(self.pos).ok_or(())?;
                match e {
                    b'"' => filter.push_byte(b'"'),
                    b'\\' => filter.push_byte(b'\\'),
                    b'/' => filter.push_byte(b'/'),
                    b'b' => filter.push_byte(0x08),
                    b'f' => filter.push_byte(0x0c),
                    b'n' => filter.push_byte(b'\n'),
                    b'r' => filter.push_byte(b'\r'),
                    b't' => filter.push_byte(b'\t'),
                    b'u' => {
                        // `raw + 1 + 4 >= end`: four hex digits and at least
                        // one more byte.
                        if self.pos + 5 >= self.s.len() {
                            return Err(());
                        }
                        let hex = &self.s[self.pos + 1..self.pos + 5];
                        if !hex.iter().all(|b| b.is_ascii_hexdigit()) {
                            return Err(());
                        }
                        let cp = u32::from_str_radix(std::str::from_utf8(hex).map_err(|_| ())?, 16)
                            .map_err(|_| ())?;
                        filter.push_codepoint(cp);
                        self.pos += 4;
                    }
                    _ => return Err(()),
                }
                self.pos += 1;
            } else if c == b'"' {
                self.pos += 1;
                break;
            } else {
                filter.push_byte(c);
                self.pos += 1;
            }
        }
        filter.finish().map(Token::Str).ok_or(())
    }

    /// `UniValue::read`'s expectation machine
    /// (rc.3 `univalue/lib/univalue_read.cpp:256-460`).
    fn read(mut self) -> Option<Json> {
        #[derive(Default)]
        struct Expect {
            obj_name: bool,
            colon: bool,
            arr_value: bool,
            value: bool,
            not_value: bool,
        }
        let mut ex = Expect::default();
        let mut stack: Vec<Frame> = Vec::new();
        let mut result: Option<Json> = None;
        let mut prev: Option<Token> = None;
        loop {
            let last = prev.take();
            let t = self.token().ok()??;
            let is_value_open = t.is_value() || matches!(t, Token::ObjOpen | Token::ArrOpen);

            if ex.value {
                if !is_value_open {
                    return None;
                }
                ex.value = false;
            } else if ex.arr_value {
                if !(is_value_open || t == Token::ArrClose) {
                    return None;
                }
                ex.arr_value = false;
            } else if ex.obj_name {
                if !matches!(t, Token::ObjClose | Token::Str(_)) {
                    return None;
                }
            } else if ex.colon {
                if t != Token::Colon {
                    return None;
                }
                ex.colon = false;
            } else if t == Token::Colon {
                return None;
            }
            if ex.not_value {
                if is_value_open {
                    return None;
                }
                ex.not_value = false;
            }

            match &t {
                Token::ObjOpen | Token::ArrOpen => {
                    let obj = t == Token::ObjOpen;
                    stack.push(if obj {
                        Frame::Obj(Vec::new(), None)
                    } else {
                        Frame::Arr(Vec::new())
                    });
                    if stack.len() > MAX_DEPTH {
                        return None;
                    }
                    if obj {
                        ex.obj_name = true;
                    } else {
                        ex.arr_value = true;
                    }
                }
                Token::ObjClose | Token::ArrClose => {
                    if stack.is_empty() || last == Some(Token::Comma) {
                        return None;
                    }
                    let value = match (stack.pop()?, &t) {
                        (Frame::Obj(entries, None), Token::ObjClose) => Json::Obj(entries),
                        (Frame::Arr(values), Token::ArrClose) => Json::Arr(values),
                        _ => return None,
                    };
                    ex.obj_name = false;
                    ex.not_value = true;
                    if !attach(&mut stack, value, &mut result) {
                        return None;
                    }
                }
                Token::Colon => {
                    if !matches!(stack.last(), Some(Frame::Obj(..))) {
                        return None;
                    }
                    ex.value = true;
                }
                Token::Comma => {
                    if stack.is_empty() || matches!(last, Some(Token::Comma | Token::ArrOpen)) {
                        return None;
                    }
                    match stack.last() {
                        Some(Frame::Obj(..)) => ex.obj_name = true,
                        _ => ex.arr_value = true,
                    }
                }
                Token::Str(s) if ex.obj_name => {
                    let Some(Frame::Obj(_, pending)) = stack.last_mut() else {
                        return None;
                    };
                    *pending = Some(s.clone());
                    ex.obj_name = false;
                    ex.colon = true;
                    ex.not_value = true;
                }
                Token::Null | Token::True | Token::False | Token::Num(_) | Token::Str(_) => {
                    let value = match &t {
                        Token::Null => Json::Null,
                        Token::True => Json::Bool(true),
                        Token::False => Json::Bool(false),
                        Token::Num(n) => Json::Num(n.clone()),
                        Token::Str(s) => Json::Str(s.clone()),
                        _ => unreachable!(),
                    };
                    if stack.is_empty() {
                        result = Some(value);
                    } else {
                        if !attach(&mut stack, value, &mut result) {
                            return None;
                        }
                        ex.not_value = true;
                    }
                }
            }
            prev = Some(t);
            if stack.is_empty() {
                break;
            }
        }
        // Nothing may follow the value.
        match self.token() {
            Ok(None) => result,
            _ => None,
        }
    }
}

/// Appends a finished value to the open container, or makes it the result.
fn attach(stack: &mut [Frame], value: Json, result: &mut Option<Json>) -> bool {
    match stack.last_mut() {
        None => {
            *result = Some(value);
            true
        }
        Some(Frame::Arr(values)) => {
            values.push(value);
            true
        }
        Some(Frame::Obj(entries, pending)) => match pending.take() {
            Some(key) => {
                entries.push((key, value));
                true
            }
            None => false,
        },
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    fn rt(text: &str) -> String {
        String::from_utf8(Json::read(text.as_bytes()).expect(text).write(2)).unwrap()
    }

    #[test]
    fn test_QT_126_pretty_layout_matches_univalue() {
        assert_eq!(
            rt(r#"{"a":1,"b":[],"c":{},"d":[1,{"e":"x"}]}"#),
            "{\n  \"a\": 1,\n  \"b\": [\n  ],\n  \"c\": {\n  },\n  \"d\": [\n    1,\n    {\n      \"e\": \"x\"\n    }\n  ]\n}"
        );
        assert_eq!(rt("[]"), "[\n]");
        assert_eq!(rt("\"x\""), "\"x\"");
    }

    #[test]
    fn test_QT_126_numbers_keep_their_tokens() {
        assert_eq!(
            rt("[1.50,-0,1E+3,10]"),
            "[\n  1.50,\n  -0,\n  1E+3,\n  10\n]"
        );
        let v = Json::read(b"[1.0, 1e3, 18446744073709551615, -1]").unwrap();
        let a = v.as_array().unwrap();
        assert_eq!(a[0].get_u64(), None);
        assert_eq!(a[1].get_u64(), None);
        assert_eq!(a[2].get_u64(), Some(u64::MAX));
        assert_eq!(a[3].get_u64(), None);
        assert_eq!(a[3].get_i64(), Some(-1));
    }

    #[test]
    fn test_QT_126_duplicate_keys_are_kept_and_the_first_wins() {
        let v = Json::read(br#"{"k":1,"k":2}"#).unwrap();
        assert_eq!(v.find("k").unwrap().get_u64(), Some(1));
        assert_eq!(v.write(0), br#"{"k":1,"k":2}"#);
        let mut o = Json::obj();
        o.push_kv("k", Json::int(1));
        o.push_kv("j", Json::int(2));
        o.push_kv("k", Json::int(3));
        assert_eq!(o.write(0), br#"{"k":3,"j":2}"#);
    }

    #[test]
    fn test_QT_126_escapes_follow_univalues_table() {
        let v = Json::Str(b"a\"\\/\x01\x08\x7f\xc3\xa9".to_vec());
        assert_eq!(v.write(2), b"\"a\\\"\\\\/\\u0001\\b\\u007f\xc3\xa9\"");
        let back = Json::read(&v.write(2)).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn test_QT_126_utf8_filter_reencodes_as_univalue_does() {
        // A surrogate pair becomes one code point.
        let v = Json::read(br#""\uD834\uDD1E""#).unwrap();
        assert_eq!(v, Json::Str("\u{1D11E}".as_bytes().to_vec()));
        // An overlong NUL is accepted and re-encoded as one byte.
        let v = Json::read(b"\"\xc0\x80\"").unwrap();
        assert_eq!(v, Json::Str(vec![0]));
        // Unpaired surrogates and bad continuations are refused.
        for bad in [
            &br#""\uD834""#[..],
            br#""\uDD1E""#,
            b"\"\xc3\"",
            b"\"\x80\"",
        ] {
            assert!(Json::read(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn test_QT_126_reader_refuses_what_univalue_refuses() {
        for bad in [
            "",
            " ",
            "[1,]",
            "{\"a\":1,}",
            "[,1]",
            "{,}",
            "01",
            "[-]",
            "1.",
            "1e",
            "[1 2]",
            "{\"a\" 1}",
            "{\"a\":1 \"b\":2}",
            "[1]]",
            "{1:2}",
            "nul",
            "[\"\x01\"]",
            "[1] x",
            "\"\\x\"",
            "{\"a\":}",
            ":",
        ] {
            assert!(Json::read(bad.as_bytes()).is_none(), "{bad:?}");
        }
        let deep = "[".repeat(513) + &"]".repeat(513);
        assert!(Json::read(deep.as_bytes()).is_none());
        let ok = "[".repeat(512) + &"]".repeat(512);
        assert!(Json::read(ok.as_bytes()).is_some());
        // A lone `-` at the end of the input is a number token to UniValue.
        for good in [
            " {} ",
            "null",
            "true",
            "-0",
            "-",
            "[null,false]",
            "\"\\u00e9\"",
        ] {
            assert!(Json::read(good.as_bytes()).is_some(), "{good:?}");
        }
    }
}
