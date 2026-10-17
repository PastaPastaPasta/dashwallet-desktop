//! `DWCLI_LOG=<EnvFilter>` logs the engine, dash-spv and the SDK to stderr,
//! with links redacted (dw-uri, DP5-01 D14) for every command.
//! For an `mn` command every global option's value is replaced in each line
//! first: a library may quote one (the SDK's quorum fetch logs its URL), and
//! no `mn` output may repeat a value (MG-03 D32, D33). Every non-empty value
//! is, in the forms a library prints it: as given, `{:?}`-quoted, and for a
//! URL as parsed, with its user name, password, host and path apart; in any
//! ASCII case, and in one pass, so overlapping values go together. A list
//! value is split as its reader splits it, each item a value too (D37).

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
            .with_writer(Redacting {
                values: Arc::new(option_values(cli)),
                sink: Arc::new(|text: &[u8]| {
                    let text = String::from_utf8_lossy(text);
                    let text = dw_uri::redact::redact_links(&text);
                    let _ = std::io::stderr().write_all(text.as_bytes());
                }),
            })
            .init();
    } else {
        builder
            .with_ansi(false)
            .with_writer(|| dw_uri::redact::RedactingWriter::new(std::io::stderr()))
            .init();
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
    // The engine joins the `--dapi` values with commas, and the SDK splits
    // the list again (rs-dapi-client `AddressList::from_str`), logging a
    // failing address alone. The only list option: a `--peer` is one
    // address, and `--quorum-url` one URL.
    let items: Vec<String> = cli
        .dapi
        .iter()
        .flat_map(|v| v.split(','))
        .flat_map(|item| [item.to_string(), item.trim().to_string()])
        .collect();
    values.extend(items);
    let forms: Vec<String> = values.iter().flat_map(|v| forms(v)).collect();
    values.extend(forms);
    values.retain(|v| !v.is_empty());
    values.sort();
    values.dedup();
    values
}

/// The other forms of `value`: `{:?}`-quoted, a URL's parts, and a socket
/// address as printed (dash-spv prints a `--peer`, `[::1]` for `[0::1]`,
/// and `10.1.2.3` for `[::ffff:10.1.2.3]`).
fn forms(value: &str) -> Vec<String> {
    let mut forms = vec![value.escape_debug().to_string()];
    if let Ok(addr) = value.trim().parse::<std::net::SocketAddr>() {
        forms.push(addr.to_string());
        forms.push(addr.ip().to_string());
        // dashcore prints a mapped or compatible IPv6 address as IPv4.
        if let std::net::IpAddr::V6(ip) = addr.ip() {
            forms.extend(ip.to_ipv4().map(|v4| v4.to_string()));
        }
    }
    if let Ok(url) = url::Url::parse(value) {
        forms.push(url.as_str().to_string());
        forms.push(url.as_str().trim_end_matches('/').to_string());
        forms.push(url.username().to_string());
        forms.extend(url.password().map(String::from));
        if let Some(host) = url.host_str() {
            forms.push(host.to_string());
            // An IPv6 host printed as an address has no brackets.
            forms.push(
                host.trim_start_matches('[')
                    .trim_end_matches(']')
                    .to_string(),
            );
            forms.extend(url.port_or_known_default().map(|p| format!("{host}:{p}")));
        }
        if url.path() != "/" {
            forms.push(url.path().to_string());
        }
    }
    forms
}

/// Where a redacted line goes: stderr, or a test's buffer.
type Sink = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// One event's text, written to the sink redacted when dropped.
struct Redacting {
    values: Arc<Vec<String>>,
    sink: Sink,
}

impl<'a> MakeWriter<'a> for Redacting {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line {
            values: Arc::clone(&self.values),
            sink: Arc::clone(&self.sink),
            text: Vec::new(),
        }
    }
}

struct Line {
    values: Arc<Vec<String>>,
    sink: Sink,
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
        (self.sink)(text.as_bytes());
    }
}

/// `text` with each run of bytes some value covers (ignoring ASCII case)
/// replaced by one marker.
fn redact(text: &str, values: &[String]) -> String {
    let lower = text.to_ascii_lowercase();
    let mut covered = vec![false; text.len()];
    for value in values {
        if value.is_empty() {
            continue;
        }
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

    /// A peer's address is redacted as dash-spv and dashcore print it
    /// (D37).
    #[test]
    fn a_socket_address_is_redacted_as_printed() {
        let values = forms("[0:0:0:0:0:0:0:1]:9999");
        assert_eq!(
            redact("peer [::1]:9999 and ::1", &values),
            "peer <redacted> and <redacted>"
        );
        let values = forms("[::ffff:10.1.2.3]:9999");
        assert_eq!(redact("address: 10.1.2.3", &values), "address: <redacted>");
    }

    /// The SDK splits a comma-separated `--dapi` into addresses and logs a
    /// failing one alone ("ban address {address} due to error"): neither
    /// address, its host nor its path is in the log (Sol r4 Major 1, D37).
    /// The request is the credits read's (an identity balance), through
    /// the SDK a session builds from the options, against closed ports.
    /// It runs in a process of its own with the log global, as dwcli's is:
    /// tracing's callsite cache is per process, and this binary's other
    /// tests reach the same SDK callsites with no subscriber.
    #[cfg(all(test, debug_assertions))]
    #[test]
    fn a_failing_address_of_a_dapi_list_is_redacted() {
        let inner = "log::tests::a_failing_address_of_a_dapi_list_is_redacted_here";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", inner, "--ignored", "--test-threads", "1"])
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[cfg(all(test, debug_assertions))]
    #[test]
    #[ignore = "run in its own process by a_failing_address_of_a_dapi_list_is_redacted"]
    fn a_failing_address_of_a_dapi_list_is_redacted_here() {
        use std::sync::Mutex;

        use clap::Parser;
        use dw_engine::{Engine, EngineConfig, EngineEvent, EventSink};

        struct NoEvents;
        impl EventSink for NoEvents {
            fn emit(&self, _: EngineEvent) {}
        }

        let dir = dw_testutil::private_tempdir();
        // Hosts every CI runner answers on (macOS has no 127.0.0.2; the
        // pinned SDK panics on an IPv6 DAPI host's TLS name).
        let dapi = "http://127.0.0.1:1/pathTOKENa,http://localhost:2/pathTOKENb";
        let cli = Cli::try_parse_from([
            "dwcli",
            "--datadir",
            dir.path().to_str().unwrap(),
            "--dapi",
            dapi,
            "--quorum-url",
            "http://127.0.0.1:3",
            "--peer",
            "127.0.0.1:4",
            "mn",
            "credits",
            &"11".repeat(32),
        ])
        .unwrap();
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = {
            let out = Arc::clone(&out);
            Arc::new(move |text: &[u8]| out.lock().unwrap().extend_from_slice(text))
        };
        let subscriber = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
            .with_ansi(false)
            .with_writer(Redacting {
                values: Arc::new(option_values(&cli)),
                sink,
            })
            .finish();
        tracing::subscriber::set_global_default(subscriber).unwrap();

        let engine = Engine::new(
            EngineConfig {
                data_root: cli.datadir.clone(),
                worker_threads: None,
                vault: dw_vault::VaultConfig::default(),
            },
            Arc::new(NoEvents),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(cli.network.clone(), crate::session_options(&cli)))
            .unwrap();
        let fetched = engine.block_on(dw_engine::test_support::fetch_identity_balance(
            &session, [1; 32],
        ));
        assert!(!fetched.unwrap(), "a closed port answered");
        drop(session);
        engine.block_on(engine.shutdown()).unwrap();

        let log = String::from_utf8(out.lock().unwrap().clone()).unwrap();
        assert!(
            log.matches("ban address").count() >= 2,
            "the SDK didn't log both addresses: {log}"
        );
        for leak in ["pathTOKEN", "127.0.0.1", "localhost"] {
            assert!(!log.contains(leak), "{leak} in the log: {log}");
        }
    }
}
