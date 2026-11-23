//! A JSON value written exactly as Dash Core's UniValue writes it
//! (`UniValue::write(2)`): two-space indent, `"key": value`, empty
//! containers as `{\n}` / `[\n]`, numbers kept as their text (amounts with
//! eight decimals, as `ValueFromAmount`).

/// A JSON value with insertion-ordered object keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// The number's text, written as is.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn int(n: impl Into<i128>) -> Self {
        Json::Num(n.into().to_string())
    }

    /// Dash Core `ValueFromAmount`: duffs as DASH with eight decimals.
    pub fn amount(duffs: i64) -> Self {
        let sign = if duffs < 0 { "-" } else { "" };
        let abs = duffs.unsigned_abs();
        Json::Num(format!(
            "{sign}{}.{:08}",
            abs / 100_000_000,
            abs % 100_000_000
        ))
    }

    pub fn str(s: impl Into<String>) -> Self {
        Json::Str(s.into())
    }

    pub fn obj<const N: usize>(fields: [(&str, Json); N]) -> Self {
        Json::Obj(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// `[key]` indexing of dash-qt's console: arrays by integer, objects by
    /// key. A missing key gives `null` (UniValue's `find_value`).
    pub fn query(&self, key: &str) -> Option<Json> {
        match self {
            Json::Arr(items) => {
                let i: usize = key.parse().ok()?;
                Some(items.get(i).cloned().unwrap_or(Json::Null))
            }
            Json::Obj(fields) => Some(
                fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.clone())
                    .unwrap_or(Json::Null),
            ),
            _ => None,
        }
    }

    /// `UniValue::write(pretty_indent)`.
    pub fn write(&self, pretty_indent: usize) -> String {
        let mut s = String::new();
        self.write_into(pretty_indent, 0, &mut s);
        s
    }

    fn write_into(&self, pretty: usize, indent_level: usize, s: &mut String) {
        let level = indent_level.max(1);
        match self {
            Json::Null => s.push_str("null"),
            Json::Bool(b) => s.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => s.push_str(n),
            Json::Str(v) => {
                s.push('"');
                escape_into(v, s);
                s.push('"');
            }
            Json::Arr(items) => {
                s.push('[');
                if pretty > 0 {
                    s.push('\n');
                }
                for (i, v) in items.iter().enumerate() {
                    if pretty > 0 {
                        s.push_str(&" ".repeat(pretty * level));
                    }
                    v.write_into(pretty, level + 1, s);
                    if i + 1 != items.len() {
                        s.push(',');
                    }
                    if pretty > 0 {
                        s.push('\n');
                    }
                }
                if pretty > 0 {
                    s.push_str(&" ".repeat(pretty * (level - 1)));
                }
                s.push(']');
            }
            Json::Obj(fields) => {
                s.push('{');
                if pretty > 0 {
                    s.push('\n');
                }
                for (i, (k, v)) in fields.iter().enumerate() {
                    if pretty > 0 {
                        s.push_str(&" ".repeat(pretty * level));
                    }
                    s.push('"');
                    escape_into(k, s);
                    s.push_str("\":");
                    if pretty > 0 {
                        s.push(' ');
                    }
                    v.write_into(pretty, level + 1, s);
                    if i + 1 != fields.len() {
                        s.push(',');
                    }
                    if pretty > 0 {
                        s.push('\n');
                    }
                }
                if pretty > 0 {
                    s.push_str(&" ".repeat(pretty * (level - 1)));
                }
                s.push('}');
            }
        }
    }
}

/// UniValue's escape table: `"`, `\`, the C0 controls (`\b \t \n \f \r`
/// short forms) and DEL.
fn escape_into(v: &str, s: &mut String) {
    for ch in v.chars() {
        match ch {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\u{8}' => s.push_str("\\b"),
            '\t' => s.push_str("\\t"),
            '\n' => s.push_str("\\n"),
            '\u{c}' => s.push_str("\\f"),
            '\r' => s.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                s.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => s.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_like_univalue() {
        let v = Json::obj([
            ("a", Json::int(1)),
            ("b", Json::Arr(vec![Json::str("x"), Json::Null])),
            ("c", Json::Obj(vec![])),
            ("d", Json::Arr(vec![])),
            ("e", Json::obj([("f", Json::Bool(true))])),
        ]);
        assert_eq!(
            v.write(2),
            "{\n  \"a\": 1,\n  \"b\": [\n    \"x\",\n    null\n  ],\n  \"c\": {\n  },\n  \"d\": [\n  ],\n  \"e\": {\n    \"f\": true\n  }\n}"
        );
        assert_eq!(Json::Arr(vec![]).write(2), "[\n]");
        assert_eq!(
            Json::str("q\"\\\n\u{1}").write(0),
            "\"q\\\"\\\\\\n\\u0001\""
        );
    }

    #[test]
    fn amounts_have_eight_decimals() {
        assert_eq!(Json::amount(150_000_000).write(2), "1.50000000");
        assert_eq!(Json::amount(-5).write(2), "-0.00000005");
        assert_eq!(Json::amount(0).write(2), "0.00000000");
    }

    #[test]
    fn queries_index_arrays_and_objects() {
        let v = Json::obj([("tx", Json::Arr(vec![Json::str("t0")]))]);
        assert_eq!(v.query("tx").unwrap().query("0"), Some(Json::str("t0")));
        assert_eq!(v.query("nope"), Some(Json::Null));
        assert_eq!(Json::str("s").query("0"), None);
        assert_eq!(Json::Arr(vec![]).query("x"), None);
    }
}
