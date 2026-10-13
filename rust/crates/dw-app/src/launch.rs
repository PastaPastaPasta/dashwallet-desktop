//! Command-line options of the desktop app.
//!
//! Parsed by hand: the set is small, and the app must ignore the arguments
//! the WebKitGTK and WebView2 runtimes add on some platforms instead of
//! refusing to start.

use std::path::PathBuf;

use dw_engine::DashNetwork;

use crate::dto::ThemeName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Data root; `None` = the platform app-data directory.
    pub data_dir: Option<PathBuf>,
    pub network: DashNetwork,
    /// Render the four gate screens, write every `data-testid` box to this
    /// file and exit (DASHPAY §2.1 G-01).
    pub selftest: Option<PathBuf>,
    /// Pause this long on each self-test screen after announcing it on
    /// stderr (`dw-app: selftest-screen <theme>/<screen>`), so a harness can
    /// capture the window.
    pub selftest_pause_ms: u32,
    /// Start SPV once the network is open.
    pub sync: bool,
    pub spv_peers: Vec<String>,
    pub dapi_addresses: Vec<String>,
    /// The trusted quorum service, instead of the network's default.
    pub quorum_url: Option<String>,
    /// A PEM CA certificate the SDK trusts for DAPI TLS (E0-02).
    pub ca_cert_path: Option<PathBuf>,
    /// The protocol version the SDK starts at (E0-02).
    pub initial_protocol_version: Option<u32>,
    /// The chain has no Platform (a plain dashd regtest): no DashPay bring-up
    /// or Platform sync when SPV starts (as `dwcli --no-platform`).
    pub no_platform: bool,
    /// Force an appearance instead of following the system.
    pub theme: Option<ThemeName>,
    /// The window's inner size in logical pixels, instead of the configured
    /// 1280×820. The gate compares element boxes across engines at one
    /// viewport, and the CI runners' desktops are smaller than the default.
    pub window_size: Option<(u32, u32)>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            data_dir: None,
            network: DashNetwork::Mainnet,
            selftest: None,
            selftest_pause_ms: 0,
            sync: false,
            spv_peers: Vec::new(),
            dapi_addresses: Vec::new(),
            quorum_url: None,
            ca_cert_path: None,
            initial_protocol_version: None,
            no_platform: false,
            theme: None,
            window_size: None,
        }
    }
}

pub const USAGE: &str = "\
usage: dash-wallet [options]
  --datadir <dir>        data root (default: the platform app-data directory)
  --network <name>       mainnet | testnet | regtest | devnet-<name> (default mainnet)
  --sync                 start SPV after opening the network
  --spv-peer <ip:port>   connect SPV to this peer only (repeatable)
  --dapi <url>           DAPI endpoint (repeatable)
  --quorum-url <url>     trusted quorum service
  --ca-cert <file>       PEM CA certificate to trust for DAPI TLS (a devnet gateway)
  --protocol-version <n> Platform protocol version to start at
  --no-platform          the chain has no Platform (a plain dashd regtest)
  --selftest <file>      render the gate screens, write element boxes to <file>, exit
  --selftest-pause <ms>  pause on each self-test screen (for screenshots)
  --theme light|dark     force an appearance
  --window-size <WxH>    the window's inner size in logical pixels (default 1280x820)
  --help                 print this text";

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    /// Boxed: on Windows, `PathBuf` makes the options large.
    Run(Box<LaunchOptions>),
    Help,
}

fn parse_network(s: &str) -> Result<DashNetwork, String> {
    crate::dto::parse_network(s).map_err(|e| e.message)
}

/// Parses the arguments after the program name. Unknown arguments that do
/// not start with `--` are ignored (runtimes add some); unknown `--` options
/// are errors.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<ParseOutcome, String> {
    let mut opts = LaunchOptions::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--help" | "-h" => return Ok(ParseOutcome::Help),
            "--datadir" => opts.data_dir = Some(PathBuf::from(value("--datadir")?)),
            "--network" => opts.network = parse_network(&value("--network")?)?,
            "--sync" => opts.sync = true,
            "--spv-peer" => opts.spv_peers.push(value("--spv-peer")?),
            "--dapi" => opts.dapi_addresses.push(value("--dapi")?),
            "--quorum-url" => opts.quorum_url = Some(value("--quorum-url")?),
            "--ca-cert" => opts.ca_cert_path = Some(PathBuf::from(value("--ca-cert")?)),
            "--protocol-version" => {
                let v = value("--protocol-version")?;
                opts.initial_protocol_version = Some(
                    v.parse()
                        .map_err(|_| format!("--protocol-version: not a number: {v}"))?,
                );
            }
            "--no-platform" => opts.no_platform = true,
            "--selftest" => opts.selftest = Some(PathBuf::from(value("--selftest")?)),
            "--selftest-pause" => {
                let v = value("--selftest-pause")?;
                opts.selftest_pause_ms = v
                    .parse()
                    .map_err(|_| format!("--selftest-pause: not a number: {v}"))?;
            }
            "--theme" => {
                opts.theme = Some(match value("--theme")?.as_str() {
                    "light" => ThemeName::Light,
                    "dark" => ThemeName::Dark,
                    other => return Err(format!("unknown theme {other:?}")),
                })
            }
            "--window-size" => {
                let v = value("--window-size")?;
                let size = v
                    .split_once('x')
                    .and_then(|(w, h)| w.parse().ok().zip(h.parse().ok()))
                    .filter(|&(w, h)| w > 0 && h > 0);
                opts.window_size =
                    Some(size.ok_or_else(|| format!("--window-size: expected WxH, got {v:?}"))?);
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => {}
        }
    }
    Ok(ParseOutcome::Run(Box::new(opts)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> LaunchOptions {
        match parse(args.iter().map(|s| s.to_string())).unwrap() {
            ParseOutcome::Run(o) => *o,
            ParseOutcome::Help => panic!("help"),
        }
    }

    #[test]
    fn defaults_to_mainnet_with_engine() {
        assert_eq!(run(&[]), LaunchOptions::default());
    }

    #[test]
    fn parses_every_option() {
        let o = run(&[
            "--datadir",
            "/tmp/dw",
            "--network",
            "devnet-paloma",
            "--sync",
            "--spv-peer",
            "127.0.0.1:19899",
            "--dapi",
            "https://127.0.0.1:2443",
            "--quorum-url",
            "https://127.0.0.1:22444",
            "--ca-cert",
            "/tmp/gw.pem",
            "--protocol-version",
            "9",
            "--no-platform",
            "--selftest",
            "/tmp/boxes.json",
            "--selftest-pause",
            "1500",
            "--theme",
            "dark",
            "--window-size",
            "1000x620",
        ]);
        assert_eq!(o.data_dir, Some(PathBuf::from("/tmp/dw")));
        assert_eq!(
            o.network,
            DashNetwork::Devnet {
                name: "paloma".into()
            }
        );
        assert!(o.sync && o.no_platform);
        assert_eq!(o.spv_peers, ["127.0.0.1:19899"]);
        assert_eq!(o.dapi_addresses, ["https://127.0.0.1:2443"]);
        assert_eq!(o.quorum_url.as_deref(), Some("https://127.0.0.1:22444"));
        assert_eq!(o.ca_cert_path, Some(PathBuf::from("/tmp/gw.pem")));
        assert_eq!(o.initial_protocol_version, Some(9));
        assert_eq!(o.selftest, Some(PathBuf::from("/tmp/boxes.json")));
        assert_eq!(o.selftest_pause_ms, 1500);
        assert_eq!(o.theme, Some(ThemeName::Dark));
        assert_eq!(o.window_size, Some((1000, 620)));
    }

    #[test]
    fn rejects_bad_input() {
        let err = |a: &[&str]| parse(a.iter().map(|s| s.to_string())).unwrap_err();
        assert!(err(&["--network", "moon"]).contains("unknown network"));
        assert!(err(&["--network", "devnet-Paloma"]).contains("devnet"));
        assert!(err(&["--datadir"]).contains("needs a value"));
        assert!(err(&["--protocol-version", "nine"]).contains("not a number"));
        assert!(err(&["--bogus"]).contains("unknown option"));
        // No build of the app runs on fixture data (U-07): the fixture backend
        // exists only in the renderer's browser harness.
        assert!(err(&["--fixture"]).contains("unknown option"));
        for bad in ["1000", "0x620", "1000x", "ax620"] {
            assert!(
                err(&["--window-size", bad]).contains("expected WxH"),
                "{bad}"
            );
        }
    }

    #[test]
    fn ignores_runtime_positional_arguments() {
        assert_eq!(run(&["-psn_0_12345", "file.txt"]), LaunchOptions::default());
        assert_eq!(parse(["--help".to_string()]).unwrap(), ParseOutcome::Help);
    }
}
