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
    /// file and exit (DASHPAY §2.1 G-01). This and the next two are set only
    /// in a build with the `selftest` feature; elsewhere their options are
    /// unknown (E0-13 D17).
    pub selftest: Option<PathBuf>,
    /// Pause this long on each self-test screen after announcing it on
    /// stderr (`dw-app: selftest-screen <theme>/<screen>`), so a harness can
    /// capture the window.
    pub selftest_pause_ms: u32,
    /// The vault passphrase, first line of this file, for `selftest_unlock`:
    /// the self-test then records the unlocked screens and the lock screen
    /// of an encrypted vault. The renderer never sees the passphrase.
    pub selftest_passphrase_file: Option<PathBuf>,
    /// Append what happens to deep links to this file: `received <source>
    /// <scheme>` from the shell, `routed …` from the renderer
    /// (`selftest_link_routed`). The CI link jobs read it (U-01).
    pub selftest_link_trace: Option<PathBuf>,
    /// Links among the arguments (`dash:`, `pay:`, `dashwallet:`,
    /// `dashpay:`), as the OS passes a clicked link. The shell takes them
    /// into its queue at start-up (`shell::links`).
    pub links: Vec<String>,
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
            selftest_passphrase_file: None,
            selftest_link_trace: None,
            links: Vec::new(),
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
usage: dash-wallet [options] [link]
  link                   a dash:, pay:, dashwallet: or dashpay: link to open
  --datadir <dir>        data root (default: the platform app-data directory)
  --network <name>       mainnet | testnet | regtest | devnet-<name> (default mainnet)
  --sync                 start SPV after opening the network
  --spv-peer <ip:port>   connect SPV to this peer only (repeatable)
  --dapi <url>           DAPI endpoint (repeatable)
  --quorum-url <url>     trusted quorum service
  --ca-cert <file>       PEM CA certificate to trust for DAPI TLS (a devnet gateway)
  --protocol-version <n> Platform protocol version to start at
  --no-platform          the chain has no Platform (a plain dashd regtest)
  --theme light|dark     force an appearance
  --window-size <WxH>    the window's inner size in logical pixels (default 1280x820)
  --help                 print this text";

/// The self-test's options: a build with the `selftest` feature only.
#[cfg(feature = "selftest")]
const SELFTEST_USAGE: &str = "
  --selftest <file>      render the gate screens, write element boxes to <file>, exit
  --selftest-pause <ms>  pause on each self-test screen (for screenshots)
  --selftest-passphrase-file <file>
                         the self-test unlocks the vault with this file's first line
  --selftest-link-trace <file>
                         append each deep link's scheme and route to <file>";

/// The environment, after every option.
const ENV_USAGE: &str = "
environment:
  DW_LOG=<filter>        log to stderr, EnvFilter syntax (e.g. info,dw_engine=debug)";

/// [`USAGE`], plus the self-test's options in a `selftest` build, then the
/// environment.
pub fn usage() -> String {
    #[cfg(feature = "selftest")]
    return format!("{USAGE}{SELFTEST_USAGE}{ENV_USAGE}");
    #[cfg(not(feature = "selftest"))]
    format!("{USAGE}{ENV_USAGE}")
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseOutcome {
    /// Boxed: on Windows, `PathBuf` makes the options large.
    Run(Box<LaunchOptions>),
    Help,
}

fn parse_network(s: &str) -> Result<DashNetwork, String> {
    crate::dto::parse_network(s).map_err(|_| {
        "--network: unknown network (mainnet, testnet, regtest or devnet-<name> \
         with a lower-case name)"
            .to_owned()
    })
}

/// How an unknown `--` option reads in an error: its name, when that is
/// letters, digits and `-` only, else a placeholder. No error quotes an
/// argument's value or text that is not a plain option name: the OS may
/// pass a link (a bearer secret, DP5-01 D30) after an option or inside one.
fn option_name(arg: &str) -> &str {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    let plain = name.len() <= 64
        && name[2..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-');
    if plain { name } else { "(not shown)" }
}

/// Parses the arguments after the program name. Unknown arguments that do
/// not start with `--` are ignored (runtimes add some); unknown `--` options
/// are errors. No error quotes a value (D30).
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
                        .map_err(|_| "--protocol-version: not a number".to_owned())?,
                );
            }
            "--no-platform" => opts.no_platform = true,
            // Without the `selftest` feature these are unknown options (E0-13 D17).
            #[cfg(feature = "selftest")]
            "--selftest" => opts.selftest = Some(PathBuf::from(value("--selftest")?)),
            #[cfg(feature = "selftest")]
            "--selftest-pause" => {
                let v = value("--selftest-pause")?;
                opts.selftest_pause_ms = v
                    .parse()
                    .map_err(|_| "--selftest-pause: not a number".to_owned())?;
            }
            #[cfg(feature = "selftest")]
            "--selftest-passphrase-file" => {
                opts.selftest_passphrase_file =
                    Some(PathBuf::from(value("--selftest-passphrase-file")?))
            }
            #[cfg(feature = "selftest")]
            "--selftest-link-trace" => {
                opts.selftest_link_trace = Some(PathBuf::from(value("--selftest-link-trace")?))
            }
            "--theme" => {
                opts.theme = Some(match value("--theme")?.as_str() {
                    "light" => ThemeName::Light,
                    "dark" => ThemeName::Dark,
                    _ => return Err("--theme: expected light or dark".into()),
                })
            }
            "--window-size" => {
                let v = value("--window-size")?;
                let size = v
                    .split_once('x')
                    .and_then(|(w, h)| w.parse().ok().zip(h.parse().ok()))
                    .filter(|&(w, h)| w > 0 && h > 0);
                opts.window_size =
                    Some(size.ok_or_else(|| "--window-size: expected WxH".to_owned())?);
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown option {}", option_name(other)));
            }
            link if crate::shell::links::link_scheme(link).is_some() => {
                opts.links.push(link.to_owned())
            }
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
        assert_eq!(o.theme, Some(ThemeName::Dark));
        assert_eq!(o.window_size, Some((1000, 620)));
    }

    #[cfg(feature = "selftest")]
    #[test]
    fn parses_the_selftest_options() {
        let o = run(&[
            "--selftest",
            "/tmp/boxes.json",
            "--selftest-pause",
            "1500",
            "--selftest-passphrase-file",
            "/tmp/pass",
            "--selftest-link-trace",
            "/tmp/links",
        ]);
        assert_eq!(o.selftest, Some(PathBuf::from("/tmp/boxes.json")));
        assert_eq!(o.selftest_link_trace, Some(PathBuf::from("/tmp/links")));
        assert_eq!(o.selftest_pause_ms, 1500);
        assert_eq!(o.selftest_passphrase_file, Some(PathBuf::from("/tmp/pass")));
        assert!(usage().contains("--selftest-passphrase-file"));
    }

    /// A build without the `selftest` feature (every release and packaged
    /// build) refuses the self-test's options (E0-13 D17).
    #[cfg(not(feature = "selftest"))]
    #[test]
    fn a_build_without_the_selftest_feature_refuses_its_options() {
        for args in [
            ["--selftest", "/tmp/boxes.json"],
            ["--selftest-pause", "1500"],
            ["--selftest-passphrase-file", "/tmp/pass"],
            ["--selftest-link-trace", "/tmp/links"],
        ] {
            let e = parse(args.iter().map(|s| s.to_string())).unwrap_err();
            assert_eq!(e, format!("unknown option {}", args[0]));
        }
        assert!(!usage().contains("--selftest"));
    }

    #[test]
    fn rejects_bad_input() {
        let err = |a: &[&str]| parse(a.iter().map(|s| s.to_string())).unwrap_err();
        assert!(err(&["--network", "moon"]).contains("unknown network"));
        assert!(err(&["--network", "devnet-Paloma"]).contains("devnet"));
        assert!(err(&["--datadir"]).contains("needs a value"));
        assert!(err(&["--protocol-version", "nine"]).contains("not a number"));
        assert!(err(&["--bogus"]).contains("unknown option"));
        assert_eq!(err(&["--bogus=1"]), "unknown option --bogus");
        // D30: no value, and no option name that is not plain, is quoted.
        for (arg, value) in [
            (
                &["--bad=dashpay://invite?pk=cFirst\ncSuffix"][..],
                "cSuffix",
            ),
            (&["--dashpay://invite?pk=cFirst"], "cFirst"),
            (&["--theme", "dash:yPlanted"], "Planted"),
            (&["--window-size", "dash:yPlanted"], "Planted"),
            (&["--protocol-version", "dash:yPlanted"], "Planted"),
            (&["--network", "dashpay://invite?pk=cPlanted"], "Planted"),
        ] {
            let e = err(arg);
            assert!(!e.contains(value) && !e.contains("dash"), "{e}");
        }
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

    #[test]
    fn links_among_the_arguments_are_kept_in_order() {
        let o = run(&[
            "--network",
            "testnet",
            "dash:yXabc?amount=1",
            "file.txt",
            "DashPay://user?id=x&username=bob",
            "https://example.com",
        ]);
        assert_eq!(o.network, DashNetwork::Testnet);
        assert_eq!(
            o.links,
            ["dash:yXabc?amount=1", "DashPay://user?id=x&username=bob"]
        );
    }
}
