//! `dwcli` — headless driver over dw-engine. Grows into the regtest test driver
//! and console host (DESIGN-opus §1.4).

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, NetworkSession,
    SessionOptions,
};
use dw_vault::{LockState, UnlockScope, VaultConfig};
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
    /// File holding the vault passphrase (first line). `init-vault` encrypts
    /// the new vault with it; other commands unlock an encrypted vault with it.
    #[arg(long)]
    passphrase_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the network's vault: encrypted with --passphrase-file, or
    /// unencrypted (data key in the OS keyring) with --unencrypted.
    InitVault {
        #[arg(long)]
        unencrypted: bool,
    },
    /// Create a wallet from a fresh mnemonic, stored in the vault; prints id
    /// and phrase.
    Create {
        #[arg(long, default_value_t = 12)]
        words: u8,
    },
    /// Restore a wallet into the vault; reads the mnemonic from stdin (never
    /// argv).
    Import {
        /// First block to scan; omit for the SPV tip (new wallets only).
        #[arg(long)]
        birth_height: Option<u32>,
        /// Derive the seed with Dash Core's BIP39 quirks (QT-104).
        #[arg(long)]
        core_compat: bool,
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

/// The first line of `path`, as bytes.
fn read_passphrase(path: &std::path::Path) -> Result<Zeroizing<Vec<u8>>, String> {
    let raw = Zeroizing::new(std::fs::read(path).map_err(|e| format!("passphrase file: {e}"))?);
    let line = raw.split(|b| *b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    Ok(Zeroizing::new(line.to_vec()))
}

/// Unlocks an encrypted, locked vault with the passphrase file, if one was
/// given. Other states are left as they are.
fn unlock_if_needed(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
) -> Result<(), String> {
    let Some(passphrase) = passphrase else {
        return Ok(());
    };
    if session.vault().lock_state() != LockState::Locked {
        return Ok(());
    }
    let passphrase = passphrase.clone();
    engine
        .block_on(session.vault_op(move |v| v.unlock(&passphrase, UnlockScope::Full)))
        .map(drop)
        .map_err(|e| e.to_string())
}

fn run(cli: Cli) -> Result<(), String> {
    let passphrase = cli
        .passphrase_file
        .as_deref()
        .map(read_passphrase)
        .transpose()?;
    let engine = Engine::new(
        EngineConfig {
            data_root: cli.datadir,
            worker_threads: None,
            vault: VaultConfig::default(),
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
        Command::InitVault { unencrypted } => {
            let secret = match (unencrypted, passphrase) {
                (true, None) => None,
                (false, Some(p)) => Some(p),
                (true, Some(_)) => {
                    return Err("--unencrypted and --passphrase-file are exclusive".into());
                }
                (false, None) => {
                    return Err("init-vault needs --passphrase-file or --unencrypted".into());
                }
            };
            engine
                .block_on(session.vault_op(move |v| v.create(secret.as_ref().map(|p| &p[..]))))
                .map(|status| println!("vault {:?}", status.state))
        }
        Command::Create { words } => {
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            engine.block_on(session.create_wallet(words)).map(|w| {
                println!("wallet_id {}", w.wallet_id);
                // The seed is in the vault; the phrase is printed so the
                // operator can record it.
                println!("mnemonic {}", w.mnemonic.as_str());
            })
        }
        Command::Import {
            birth_height,
            core_compat,
        } => {
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            let mut phrase = Zeroizing::new(String::new());
            std::io::stdin()
                .read_to_string(&mut phrase)
                .map_err(|e| e.to_string())?;
            let phrase = Zeroizing::new(phrase.trim().as_bytes().to_vec());
            engine
                .block_on(session.import_wallet(
                    phrase,
                    Zeroizing::new(Vec::new()),
                    ImportOptions {
                        birth_height,
                        core_compat,
                    },
                ))
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
        let pass = dir.path().join("pass");
        std::fs::write(&pass, "dwcli test passphrase\n").unwrap();
        let base = |cmd: Command, passphrase: bool| Cli {
            datadir: dir.path().join("data"),
            network: DashNetwork::Regtest,
            dapi: vec!["http://127.0.0.1:1".into()],
            quorum_url: None,
            peers: vec![],
            verbose_events: false,
            passphrase_file: passphrase.then(|| pass.clone()),
            command: cmd,
        };
        // Without a vault there is nowhere to keep the seed.
        let err = run(base(Command::Create { words: 12 }, false)).unwrap_err();
        assert!(err.contains("no vault"), "{err}");
        assert!(run(base(Command::InitVault { unencrypted: false }, false)).is_err());
        run(base(Command::InitVault { unencrypted: false }, true)).unwrap();
        // A fresh process sees a locked vault; the passphrase file unlocks it.
        let err = run(base(Command::Create { words: 12 }, false)).unwrap_err();
        assert!(err.contains("locked"), "{err}");
        run(base(Command::Create { words: 12 }, true)).unwrap();
        run(base(Command::List, false)).unwrap();
        assert!(dir.path().join("data/regtest/wallet.sqlite").exists());
        assert!(dir.path().join("data/regtest/vault").exists());
    }

    #[test]
    fn passphrase_file_takes_the_first_line() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p");
        std::fs::write(&p, "secret\r\nignored\n").unwrap();
        assert_eq!(&read_passphrase(&p).unwrap()[..], b"secret");
    }
}
