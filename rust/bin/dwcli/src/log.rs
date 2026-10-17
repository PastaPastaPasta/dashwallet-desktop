//! `DWCLI_LOG=<EnvFilter>` logs the engine, dash-spv and the SDK to stderr.
//! For an `mn` command every global option's value is replaced in each line
//! first: a library may quote one (the SDK's quorum fetch logs its URL), and
//! no `mn` output may repeat a value (MG-03 D32, D33). Every non-empty value
//! is, in the forms a library prints it: as given, `{:?}`-quoted, and for a
//! URL as parsed, with its user name, password, host and path apart; in any
//! ASCII case, and in one pass, so overlapping values go together.

use std::io::Write;
use std::sync::Arc;

use dw_engine::DashNetwork;
use tracing_subscriber::fmt::MakeWriter;

use crate::Cli;

pub fn init(cli: &Cli, mn: bool) {
    let Ok(filter) = std::env::var("DWCLI_LOG") else {
        return;
    };
    let builder =
        tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(filter));
    if mn {
        builder
            .with_ansi(false)
            .with_writer(Redacting(Arc::new(option_values(cli))))
            .init();
    } else {
        builder.with_writer(std::io::stderr).init();
    }
}

/// The values of the global options, in every form above.
fn option_values(cli: &Cli) -> Vec<String> {
    let mut values: Vec<String> = cli
        .peers
        .iter()
        .chain(&cli.dapi)
        .chain(&cli.quorum_url)
        .cloned()
        .chain(
            [
                Some(&cli.datadir),
                cli.ca_cert.as_ref(),
                cli.passphrase_file.as_ref(),
            ]
            .into_iter()
            .flatten()
            .map(|p| p.display().to_string()),
        )
        .collect();
    if let DashNetwork::Devnet { name } = &cli.network {
        values.push(name.clone());
    }
    let forms: Vec<String> = values.iter().flat_map(|v| forms(v)).collect();
    values.extend(forms);
    values.retain(|v| !v.is_empty());
    values.sort();
    values.dedup();
    values
}

/// The other forms of `value`: `{:?}`-quoted, and a URL's parts.
fn forms(value: &str) -> Vec<String> {
    let mut forms = vec![value.escape_debug().to_string()];
    if let Ok(url) = url::Url::parse(value) {
        forms.push(url.as_str().to_string());
        forms.push(url.as_str().trim_end_matches('/').to_string());
        forms.push(url.username().to_string());
        forms.extend(url.password().map(String::from));
        if let Some(host) = url.host_str() {
            forms.push(host.to_string());
            forms.extend(url.port_or_known_default().map(|p| format!("{host}:{p}")));
        }
        if url.path() != "/" {
            forms.push(url.path().to_string());
        }
    }
    forms
}

/// One event's text, written to stderr redacted when dropped.
struct Redacting(Arc<Vec<String>>);

impl<'a> MakeWriter<'a> for Redacting {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line {
            values: Arc::clone(&self.0),
            text: Vec::new(),
        }
    }
}

struct Line {
    values: Arc<Vec<String>>,
    text: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.text.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        let text = redact(&String::from_utf8_lossy(&self.text), &self.values);
        let _ = std::io::stderr().write_all(text.as_bytes());
    }
}

/// `text` with each run of bytes some value covers (ignoring ASCII case)
/// replaced by one marker.
fn redact(text: &str, values: &[String]) -> String {
    let lower = text.to_ascii_lowercase();
    let mut covered = vec![false; text.len()];
    for value in values {
        let value = value.to_ascii_lowercase();
        let mut from = 0;
        while let Some(at) = lower[from..].find(&value) {
            let start = from + at;
            covered[start..start + value.len()].fill(true);
            from = start + lower[start..].chars().next().map_or(1, char::len_utf8);
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(c) = text[at..].chars().next() {
        if covered[at] {
            while at < text.len() && covered[at] {
                at += 1;
            }
            out.push_str("<redacted>");
        } else {
            out.push(c);
            at += c.len_utf8();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A URL's user-info and host go even where a library prints them
    /// apart or normalised (D33).
    #[test]
    fn a_urls_parts_are_redacted() {
        let values = forms("HTTP://User:Tok@Example.COM:81/p");
        assert_eq!(
            redact(
                "url: http://User:Tok@example.com:81/p/quorums, Tok, example.com:81",
                &values
            ),
            "url: <redacted>/quorums, <redacted>, <redacted>"
        );
    }

    #[test]
    fn every_value_and_its_quoted_form_is_redacted() {
        let values = vec![
            "http://h/a\"b".to_string(),
            "http://h/a\\\"b".to_string(),
            "h/a".to_string(),
        ];
        assert_eq!(
            redact(
                "url: \"http://h/a\\\"b\" then http://h/a\"b and h/a",
                &values
            ),
            "url: \"<redacted>\" then <redacted> and <redacted>"
        );
    }

    /// A value in another case, overlapping values and a short value are
    /// redacted in one pass; a marker is never redacted again (D33).
    #[test]
    fn redaction_ignores_case_and_overlaps() {
        let values = ["example.com:81", "abc", "bcd", "e"].map(String::from);
        assert_eq!(
            redact("host EXAMPLE.Com:81, abcd, xyz é", &values),
            "host <redacted>, <redacted>, xyz é"
        );
    }
}
