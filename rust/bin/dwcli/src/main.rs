//! `dwcli` — headless driver over dw-engine. Grows into the regtest test driver
//! and console host (DESIGN-opus §1.4).

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use dw_engine::{DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(name = "dwcli", about = "dashwallet-desktop engine CLI")]
struct Cli {
    /// Data root (each network gets a sub-directory).
    #[arg(long)]
    datadir: PathBuf,
    /// mainnet | testnet | regtest | devnet:<name>
    #[arg(long, default_value = "regtest", value_parser = parse_network)]
    network: DashNetwork,
    /// DAPI endpoint; repeatable. Required for regtest/devnet.
    #[arg(long = "dapi")]
    dapi: Vec<String>,
    /// Trusted quorum service URL override.
    #[arg(long)]
    quorum_url: Option<String>,
    /// SPV peer ip:port; repeatable.
    #[arg(long = "peer")]
    peers: Vec<String>,
    /// Print engine events to stderr.
    #[arg(long)]
    verbose_events: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a wallet from a fresh mnemonic; prints id and phrase.
    Create {
        #[arg(long, default_value_t = 12)]
        words: u8,
    },
    /// Restore a wallet; reads the mnemonic from stdin (never argv).
    Import {
        #[arg(long)]
        birth_height: Option<u32>,
    },
    /// List wallets with balances (duffs).
    List,
}

fn parse_network(s: &str) -> Result<DashNetwork, String> {
    let network = match s {
        "mainnet" => DashNetwork::Mainnet,
        "testnet" => DashNetwork::Testnet,
        "regtest" => DashNetwork::Regtest,
        other => match other.strip_prefix("devnet:") {
            Some(name) => DashNetwork::Devnet {
                name: name.to_string(),
            },
            None => return Err(format!("unknown network {other:?}")),
        },
    };
    network.validate().map_err(|e| e.to_string())?;
    Ok(network)
}

struct StderrSink {
    verbose: bool,
}

impl EventSink for StderrSink {
    fn emit(&self, event: EngineEvent) {
        if self.verbose {
            eprintln!("event: {event:?}");
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let engine = Engine::new(
        EngineConfig {
            data_root: cli.datadir,
            worker_threads: None,
        },
        Arc::new(StderrSink {
            verbose: cli.verbose_events,
        }),
    )
    .map_err(|e| e.to_string())?;
    let opts = SessionOptions {
        dapi_addresses: cli.dapi,
        quorum_url: cli.quorum_url,
        spv_peers: cli.peers,
    };
    let session = engine
        .block_on(engine.open_network(cli.network, opts))
        .map_err(|e| e.to_string())?;

    let result = match cli.command {
        Command::Create { words } => engine.block_on(session.create_wallet(words)).map(|w| {
            println!("wallet_id {}", w.wallet_id);
            // TODO(vault): printed because there is no vault yet; the
            // operator must record it.
            println!("mnemonic {}", w.mnemonic.as_str());
        }),
        Command::Import { birth_height } => {
            let mut phrase = Zeroizing::new(String::new());
            std::io::stdin()
                .read_to_string(&mut phrase)
                .map_err(|e| e.to_string())?;
            let phrase = Zeroizing::new(phrase.trim().to_string());
            engine
                .block_on(session.import_wallet(phrase, birth_height))
                .map(|id| println!("wallet_id {id}"))
        }
        Command::List => session.list_wallets().map(|wallets| {
            for w in wallets {
                let b = w.balances;
                println!(
                    "{} confirmed={} unconfirmed={} immature={} locked={} total={}",
                    w.wallet_id, b.confirmed, b.unconfirmed, b.immature, b.locked, b.total
                );
            }
        }),
    };
    engine
        .block_on(engine.shutdown())
        .map_err(|e| e.to_string())?;
    result.map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_networks() {
        assert_eq!(parse_network("regtest").unwrap(), DashNetwork::Regtest);
        assert_eq!(
            parse_network("devnet:abc").unwrap(),
            DashNetwork::Devnet { name: "abc".into() }
        );
        assert!(parse_network("devnet:../x").is_err());
        assert!(parse_network("moon").is_err());
    }

    #[test]
    fn create_then_list_round_trips_through_cli_paths() {
        let dir = tempfile::tempdir().unwrap();
        let base = |cmd: Command| Cli {
            datadir: dir.path().to_path_buf(),
            network: DashNetwork::Regtest,
            dapi: vec!["http://127.0.0.1:1".into()],
            quorum_url: None,
            peers: vec![],
            verbose_events: false,
            command: cmd,
        };
        run(base(Command::Create { words: 12 })).unwrap();
        run(base(Command::List)).unwrap();
        assert!(dir.path().join("regtest/wallet.sqlite").exists());
    }
}
