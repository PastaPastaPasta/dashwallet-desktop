//! `DWCLI_LOG=<EnvFilter>` logs the engine, dash-spv and the SDK to stderr.
//! For an `mn` command every global option's value is replaced in each line
//! first: a library may quote one (the SDK's quorum fetch logs its URL), and
//! no `mn` output may repeat a value (MG-03 D32).

use std::io::Write;
use std::sync::Arc;

use dw_engine::DashNetwork;
use tracing_subscriber::fmt::MakeWriter;

use crate::Cli;

/// Shorter values would redact unrelated text; no value meant to stay out
/// of the log is that short.
const MIN_REDACTED: usize = 4;

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

/// The values of the global options, as given and as `{:?}` quotes them.
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
    let quoted: Vec<String> = values
        .iter()
        .map(|v| v.escape_debug().to_string())
        .collect();
    values.extend(quoted);
    values.retain(|v| v.len() >= MIN_REDACTED);
    // The longest first, so a value inside another goes with it.
    values.sort_by_key(|v| std::cmp::Reverse(v.len()));
    values.dedup();
    values
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

fn redact(text: &str, values: &[String]) -> String {
    values.iter().fold(text.to_string(), |text, v| {
        text.replace(v.as_str(), "<redacted>")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_and_its_quoted_form_is_redacted() {
        let values = [
            "http://h/a\"b".to_string(),
            "http://h/a\\\"b".to_string(),
            "h/a".to_string(),
        ];
        let mut values = values.to_vec();
        values.sort_by_key(|v| std::cmp::Reverse(v.len()));
        assert_eq!(
            redact(
                "url: \"http://h/a\\\"b\" then http://h/a\"b and h/a",
                &values
            ),
            "url: \"<redacted>\" then <redacted> and <redacted>"
        );
    }
}
