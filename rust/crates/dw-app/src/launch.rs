//! Command-line options of the desktop app.
//!
//! Parsed by hand: the set is small, and the app must ignore the arguments
//! the WebKitGTK and WebView2 runtimes add on some platforms instead of
//! refusing to start.

use std::path::PathBuf;

use dw_engine::DashNetwork;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    /// Data root; `None` = the platform app-data directory.
    pub data_dir: Option<PathBuf>,
    pub network: DashNetwork,
    /// The renderer uses its TypeScript fixture backend; no engine is created.
    pub fixture: bool,
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
    /// Force an appearance instead of following the system.
    pub theme: Option<Theme>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            data_dir: None,
            network: DashNetwork::Mainnet,
            fixture: false,
            selftest: None,
            selftest_pause_ms: 0,
            sync: false,
            spv_peers: Vec::new(),
            dapi_addresses: Vec::new(),
            theme: None,
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
  --fixture              use the built-in sample wallet; no engine
  --selftest <file>      render the gate screens, write element boxes to <file>, exit
  --selftest-pause <ms>  pause on each self-test screen (for screenshots)
  --theme light|dark     force an appearance
  --help                 print this text";

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    Run(LaunchOptions),
    Help,
}

fn parse_network(s: &str) -> Result<DashNetwork, String> {
    let network = match s {
        "mainnet" => DashNetwork::Mainnet,
        "testnet" => DashNetwork::Testnet,
        "regtest" => DashNetwork::Regtest,
        other => match other.strip_prefix("devnet-") {
            Some(name) => DashNetwork::Devnet {
                name: name.to_string(),
            },
            None => return Err(format!("unknown network {other:?}")),
        },
    };
    network.validate().map_err(|e| e.to_string())?;
    Ok(network)
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
            "--fixture" => opts.fixture = true,
            "--selftest" => opts.selftest = Some(PathBuf::from(value("--selftest")?)),
            "--selftest-pause" => {
                let v = value("--selftest-pause")?;
                opts.selftest_pause_ms = v
                    .parse()
                    .map_err(|_| format!("--selftest-pause: not a number: {v}"))?;
            }
            "--theme" => {
                opts.theme = Some(match value("--theme")?.as_str() {
                    "light" => Theme::Light,
                    "dark" => Theme::Dark,
                    other => return Err(format!("unknown theme {other:?}")),
                })
            }
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => {}
        }
    }
    Ok(ParseOutcome::Run(opts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> LaunchOptions {
        match parse(args.iter().map(|s| s.to_string())).unwrap() {
            ParseOutcome::Run(o) => o,
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
            "--fixture",
            "--selftest",
            "/tmp/boxes.json",
            "--selftest-pause",
            "1500",
            "--theme",
            "dark",
        ]);
        assert_eq!(o.data_dir, Some(PathBuf::from("/tmp/dw")));
        assert_eq!(
            o.network,
            DashNetwork::Devnet {
                name: "paloma".into()
            }
        );
        assert!(o.sync && o.fixture);
        assert_eq!(o.spv_peers, ["127.0.0.1:19899"]);
        assert_eq!(o.dapi_addresses, ["https://127.0.0.1:2443"]);
        assert_eq!(o.selftest, Some(PathBuf::from("/tmp/boxes.json")));
        assert_eq!(o.selftest_pause_ms, 1500);
        assert_eq!(o.theme, Some(Theme::Dark));
    }

    #[test]
    fn rejects_bad_input() {
        let err = |a: &[&str]| parse(a.iter().map(|s| s.to_string())).unwrap_err();
        assert!(err(&["--network", "moon"]).contains("unknown network"));
        assert!(err(&["--network", "devnet-Paloma"]).contains("devnet"));
        assert!(err(&["--datadir"]).contains("needs a value"));
        assert!(err(&["--bogus"]).contains("unknown option"));
    }

    #[test]
    fn ignores_runtime_positional_arguments() {
        assert_eq!(run(&["-psn_0_12345", "file.txt"]), LaunchOptions::default());
        assert_eq!(parse(["--help".to_string()]).unwrap(), ParseOutcome::Help);
    }
}
