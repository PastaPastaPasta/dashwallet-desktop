//! `dwcli` — headless driver over dw-engine. Grows into the regtest test driver
//! and console host (DESIGN-opus §1.4).

mod coinjoin;
mod compat;
mod dashpay;
mod pay;
mod tools;

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, HistoryFilter,
    HistoryQuery, HistorySort, ImportOptions, NetworkSession, SessionOptions, WalletId,
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
    /// PEM CA certificate DAPI TLS is checked against (a dashmate devnet's
    /// self-signed gateway) in addition to the system roots.
    #[arg(long)]
    ca_cert: Option<PathBuf>,
    /// Protocol version the Platform SDK starts at, used as given (even below
    /// the network's floor); it still ratchets up from what the network
    /// reports.
    #[arg(long)]
    initial_protocol_version: Option<u32>,
    /// The chain has no Platform (a plain dashd regtest): no DashPay
    /// bring-up before SPV, no Platform sync loops, and every DashPay
    /// command refuses up front with `platform.feature_off`.
    #[arg(long)]
    no_platform: bool,
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
        /// Display name.
        #[arg(long)]
        name: Option<String>,
        /// Address lookahead of the restore scan (1..=1000).
        #[arg(long)]
        lookahead: Option<u32>,
        /// File holding the BIP39 passphrase ("25th word"), first line, as
        /// bytes.
        #[arg(long)]
        bip39_passphrase_file: Option<PathBuf>,
    },
    /// List wallets with names and balances (duffs; `unknown` before the
    /// first scan).
    List,
    /// Fetch and verify the DPNS contract from DAPI: checks the endpoints,
    /// TLS (`--ca-cert`), the quorum context and proofs. Needs no wallet.
    PlatformStatus,
    /// Start SPV, wait until the condition holds, print the sync state and
    /// the wallets, stop SPV. Without `--txid` / `--min-height` it waits
    /// until dash-spv is caught up.
    Sync {
        /// Give up after this many seconds (exit status 1).
        #[arg(long, default_value_t = 120)]
        timeout_secs: u64,
        /// Wait until every wallet has scanned this height.
        #[arg(long)]
        min_height: Option<u32>,
        /// Wait until this txid is in the first wallet's history; repeatable.
        #[arg(long = "txid")]
        txids: Vec<String>,
        /// Confirmations the `--txid` transactions need.
        #[arg(long, default_value_t = 0)]
        confirmations: u32,
    },
    /// Print the first wallet's history, newest first, one record per line:
    /// txid, record index, type, amount, status, confirmations, address.
    History {
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Print the first wallet's current receive address, or issue a fresh
    /// one with `--next`.
    Receive {
        #[arg(long)]
        next: bool,
        #[arg(long)]
        label: Option<String>,
    },
    #[command(flatten)]
    Pay(pay::PayCommand),
    #[command(flatten)]
    Tools(tools::ToolsCommand),
    #[command(flatten)]
    Compat(compat::CompatCommand),
    #[command(flatten)]
    CoinJoin(coinjoin::CoinJoinCommand),
    #[command(flatten)]
    DashPay(dashpay::DashPayCommand),
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
    /// Print `NewTransactions` events to stdout (`watch-events`).
    new_tx: bool,
}

impl EventSink for StderrSink {
    fn emit(&self, event: EngineEvent) {
        if self.new_tx
            && let EngineEvent::NewTransactions {
                wallet_id,
                txids,
                catch_up,
                ..
            } = &event
        {
            let list: Vec<String> = txids.iter().map(ToString::to_string).collect();
            println!(
                "newtx wallet={wallet_id} count={} catch_up={} txids={}",
                txids.len(),
                u8::from(*catch_up),
                list.join(",")
            );
        }
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
    unlock(engine, session, passphrase).map_err(|e| e.to_string())
}

/// [`unlock_if_needed`] with the engine's error, for callers that report
/// its code.
fn unlock(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
) -> Result<(), EngineError> {
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
}

fn print_wallets(session: &Arc<NetworkSession>) -> Result<(), EngineError> {
    for w in session.wallet_infos()? {
        let balances = match w.balances {
            Some(b) => format!(
                "confirmed={} unconfirmed={} immature={} locked={} total={} coinjoin={}",
                b.confirmed, b.unconfirmed, b.immature, b.locked, b.total, b.coinjoin
            ),
            None => "balance=unknown".to_string(),
        };
        println!("{} name={:?} {balances}", w.wallet_id, w.name);
    }
    Ok(())
}

/// The oldest wallet; the headless commands act on it.
fn first_wallet(session: &Arc<NetworkSession>) -> Result<WalletId, EngineError> {
    session
        .wallet_infos()?
        .first()
        .map(|w| w.wallet_id)
        .ok_or_else(|| EngineError::WalletNotFound("no wallet on this network".into()))
}

/// Whether every `txids` entry is in `wallet`'s history with
/// `confirmations` or more.
fn txids_seen(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    wallet: WalletId,
    txids: &[String],
    confirmations: u32,
) -> Result<bool, EngineError> {
    for txid in txids {
        match engine.block_on(session.tx_detail(wallet, txid.clone())) {
            Ok(d) if d.status.confirmations >= confirmations => {}
            Ok(_) | Err(EngineError::TxNotFound(_)) => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

fn sync(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    timeout: Duration,
    min_height: Option<u32>,
    txids: &[String],
    confirmations: u32,
) -> Result<(), EngineError> {
    let deadline = Instant::now() + timeout;
    pay::ensure_spv_running(engine, session, timeout).map_err(EngineError::Spv)?;
    let mut last_line = String::new();
    let outcome = loop {
        let snap = session.sync_snapshot()?;
        let line = format!(
            "sync running={} caught_up={} tip={:?} peers={} active={:?}",
            snap.running, snap.caught_up, snap.tip_height, snap.connected_peers, snap.active_phase
        );
        if line != last_line {
            eprintln!("{line}");
            last_line = line;
        }
        let wallets = session.wallet_infos()?;
        let heights_ok = min_height.is_none_or(|h| {
            !wallets.is_empty()
                && wallets.iter().all(|w| {
                    session
                        .wallet_scan_height(&w.wallet_id)
                        .is_some_and(|scanned| scanned >= h)
                })
        });
        let txids_ok = match wallets.first() {
            Some(w) if !txids.is_empty() => {
                txids_seen(engine, session, w.wallet_id, txids, confirmations)?
            }
            _ => txids.is_empty(),
        };
        let done = if min_height.is_none() && txids.is_empty() {
            snap.caught_up
        } else {
            heights_ok && txids_ok
        };
        if done {
            println!(
                "synced tip={} peers={} caught_up={}",
                snap.tip_height.map_or("-".into(), |h| h.to_string()),
                snap.connected_peers,
                snap.caught_up
            );
            break Ok(());
        }
        if Instant::now() >= deadline {
            break Err(EngineError::Spv(format!(
                "sync condition not reached within {timeout:?}: {last_line}"
            )));
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let printed = outcome.and_then(|()| print_wallets(session));
    engine.block_on(session.stop_spv())?;
    printed
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
            new_tx: matches!(
                cli.command,
                Command::Tools(tools::ToolsCommand::WatchEvents { .. })
            ),
        }),
    )
    .map_err(|e| e.to_string())?;
    let opts = SessionOptions {
        dapi_addresses: cli.dapi,
        quorum_url: cli.quorum_url,
        spv_peers: cli.peers,
        ca_cert_path: cli.ca_cert,
        initial_protocol_version: cli.initial_protocol_version,
        no_platform: cli.no_platform,
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
            name,
            lookahead,
            bip39_passphrase_file,
        } => {
            let bip39_passphrase = bip39_passphrase_file
                .as_deref()
                .map(read_passphrase)
                .transpose()?
                .unwrap_or_default();
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            let mut phrase = Zeroizing::new(String::new());
            std::io::stdin()
                .read_to_string(&mut phrase)
                .map_err(|e| e.to_string())?;
            let phrase = Zeroizing::new(phrase.trim().as_bytes().to_vec());
            engine
                .block_on(session.import_wallet(
                    phrase,
                    bip39_passphrase,
                    ImportOptions {
                        birth_height,
                        core_compat,
                        name,
                        lookahead,
                    },
                ))
                .map(|id| println!("wallet_id {id}"))
        }
        Command::List => print_wallets(&session),
        Command::PlatformStatus => engine.block_on(session.platform_status()).map(|s| {
            println!(
                "dpns_contract id={} version={} owner={}",
                s.dpns_contract_id, s.dpns_contract_version, s.dpns_contract_owner
            );
            println!("protocol_version {}", s.protocol_version);
        }),
        Command::Sync {
            timeout_secs,
            min_height,
            txids,
            confirmations,
        } => sync(
            &engine,
            &session,
            Duration::from_secs(timeout_secs),
            min_height,
            &txids,
            confirmations,
        ),
        Command::History { limit } => first_wallet(&session).and_then(|id| {
            let page = engine.block_on(session.history_page(
                id,
                HistoryQuery {
                    filter: HistoryFilter::default(),
                    sort: HistorySort::NewestFirst,
                    cursor: None,
                    limit,
                },
            ))?;
            for r in page.records {
                println!(
                    "{} {} {:?} {} {:?} {} {}",
                    r.txid,
                    r.record_index,
                    r.tx_type,
                    r.amount,
                    r.status.kind,
                    r.status.confirmations,
                    r.address.as_deref().unwrap_or("-")
                );
            }
            Ok(())
        }),
        Command::Receive { next, label } => first_wallet(&session).and_then(|id| {
            let a = if next {
                engine.block_on(session.next_receive_address(id, label))?
            } else {
                engine.block_on(session.current_receive_address(id))?
            };
            println!(
                "address {} index {} path {}",
                a.address, a.index, a.derivation_path
            );
            Ok(())
        }),
        Command::Tools(cmd) => {
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            let result = tools::run(&engine, &session, passphrase.as_ref(), cmd);
            engine
                .block_on(engine.shutdown())
                .map_err(|e| e.to_string())?;
            return result;
        }
        Command::Pay(cmd) => {
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            let result = pay::run(&engine, &session, passphrase.as_ref(), cmd);
            engine
                .block_on(engine.shutdown())
                .map_err(|e| e.to_string())?;
            return result;
        }
        Command::Compat(cmd) => {
            unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            let result = compat::run(&engine, &session, passphrase.as_ref(), cmd);
            engine
                .block_on(engine.shutdown())
                .map_err(|e| e.to_string())?;
            return result;
        }
        Command::CoinJoin(cmd) => {
            // `coinjoin-mix --mixing-only` unlocks for mixing only itself.
            if !cmd.mixing_only() {
                unlock_if_needed(&engine, &session, passphrase.as_ref())?;
            }
            let result = coinjoin::run(&engine, &session, passphrase.as_ref(), cmd);
            engine
                .block_on(engine.shutdown())
                .map_err(|e| e.to_string())?;
            return result;
        }
        Command::DashPay(cmd) => {
            // Unlocks itself, so a failed unlock is a JSON error too, and
            // prints its JSON line once the engine is shut down.
            let result = dashpay::run(&engine, &session, passphrase.as_ref(), cmd);
            if dashpay::poisoned(&result) {
                // The session printed its last line. An engine that failed
                // its health probe may never shut down: exit without it.
                std::process::exit(1);
            }
            let teardown = engine
                .block_on(engine.shutdown())
                .map_err(|e| format!("shutdown: {e}"));
            return dashpay::report(result, teardown);
        }
    };
    engine
        .block_on(engine.shutdown())
        .map_err(|e| e.to_string())?;
    result.map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    // `DWCLI_LOG=info,dash_spv=debug` (EnvFilter syntax) logs to stderr.
    if let Ok(filter) = std::env::var("DWCLI_LOG") {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::new(filter))
            .with_writer(std::io::stderr)
            .init();
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => return parse_failure(&e),
    };
    if !matches!(cli.command, Command::DashPay(_)) {
        return match run(cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        };
    }
    // DashPay commands print one JSON line whatever happens, and a panic's
    // payload never reaches stderr (it may quote an input).
    if cli.no_platform {
        return dashpay::refuse_no_platform().map_or(ExitCode::FAILURE, |()| ExitCode::SUCCESS);
    }
    dashpay::quiet_panics();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(cli))) {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(e)) => {
            dashpay::report_setup_failure(&e);
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
        Err(_) => {
            dashpay::report_panic();
            ExitCode::FAILURE
        }
    }
}

/// A refused command line. clap's own text quotes the refused argument,
/// which may be a bearer input (review DW-E0-09 r1), so only the error
/// kind, the names of the arguments involved and the usage are printed.
/// Help and version print as usual.
fn parse_failure(e: &clap::Error) -> ExitCode {
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    let code = u8::try_from(e.exit_code()).unwrap_or(2);
    if matches!(
        e.kind(),
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayVersion
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        let _ = e.print();
        return ExitCode::from(code);
    }
    eprintln!(
        "error: {}\n\n{}\n\nFor more information, try '--help'.",
        dashpay::clap_error_text(e),
        Cli::command().render_usage()
    );
    ExitCode::from(code)
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
        let dir = dw_testutil::private_tempdir();
        let pass = dir.path().join("pass");
        std::fs::write(&pass, "dwcli test passphrase\n").unwrap();
        let base = |cmd: Command, passphrase: bool| Cli {
            datadir: dir.path().join("data"),
            network: DashNetwork::Regtest,
            dapi: vec!["http://127.0.0.1:1".into()],
            quorum_url: None,
            peers: vec![],
            ca_cert: None,
            initial_protocol_version: None,
            no_platform: false,
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
        let dir = dw_testutil::private_tempdir();
        let p = dir.path().join("p");
        std::fs::write(&p, "secret\r\nignored\n").unwrap();
        assert_eq!(&read_passphrase(&p).unwrap()[..], b"secret");
    }
}
