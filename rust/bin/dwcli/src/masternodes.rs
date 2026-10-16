//! `dwcli mn …`: the masternodes facade (MG-03, `docs/contracts/mn-engine.md`)
//! and nothing else. Every command prints one JSON line on stdout:
//! `{"ok":true,"result":…}`, or `{"ok":false,"error":{"code":…,"message":…}}`
//! with exit status 1. A key to attach is read from stdin, never argv.

use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use dw_engine::masternodes::{AttachedKeyRole, Masternodes, MasternodesError};
use dw_engine::{Engine, NetworkSession, WalletId};
use serde::Serialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

#[derive(Args)]
pub struct MnArgs {
    /// Start SPV and wait for the masternode list before the command; SPV
    /// stops after it.
    #[arg(long, global = true)]
    spv: bool,
    /// Seconds `--spv` waits for the list.
    #[arg(long, global = true, default_value_t = 120)]
    sync_timeout: u64,
    /// With `--spv`, also wait until every loaded wallet scanned this
    /// height (implies `--spv`).
    #[arg(long, global = true)]
    sync_height: Option<u32>,
    #[command(subcommand)]
    command: MnCommand,
}

#[derive(Subcommand)]
enum MnCommand {
    /// Whether the SPV list is synced, and its height.
    ListState,
    /// The wallet's masternodes, then the tracked ones (default: the oldest
    /// wallet).
    List {
        #[arg(long)]
        wallet: Option<String>,
    },
    /// One masternode in full.
    Status {
        pro_tx_hash: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Search the list by ProTx hash or service address (`ip[:port]`).
    Find { query: String },
    /// Track a listed masternode.
    Track {
        pro_tx_hash: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Stop tracking; deletes the attached keys first.
    Untrack { pro_tx_hash: String },
    /// Name a masternode; without `--label`, back to the default name.
    Label {
        pro_tx_hash: String,
        #[arg(long)]
        label: Option<String>,
    },
    /// Attach the private key of `--role` (WIF or 64 hex, read from stdin)
    /// to a tracked masternode.
    Attach {
        pro_tx_hash: String,
        #[arg(long, value_parser = parse_role)]
        role: AttachedKeyRole,
    },
    /// Delete the key attached for `--role`.
    Detach {
        pro_tx_hash: String,
        #[arg(long, value_parser = parse_role)]
        role: AttachedKeyRole,
    },
    /// Payouts to the masternode's payout address, newest first.
    Payouts {
        pro_tx_hash: String,
        #[arg(long)]
        wallet: Option<String>,
        /// Only payouts at or after this Unix time.
        #[arg(long)]
        since: Option<u64>,
    },
    /// An evonode's claimable credits (a proved Platform read).
    Credits { pro_tx_hash: String },
}

fn parse_role(s: &str) -> Result<AttachedKeyRole, String> {
    match s {
        "owner" => Ok(AttachedKeyRole::Owner),
        "voting" => Ok(AttachedKeyRole::Voting),
        "payout" => Ok(AttachedKeyRole::Payout),
        _ => Err("one of owner, voting, payout".into()),
    }
}

/// A failed command: the facade's code and message.
struct Failure {
    code: String,
    message: String,
}

impl From<MasternodesError> for Failure {
    fn from(e: MasternodesError) -> Self {
        Self {
            code: e.code().into(),
            message: e.to_string(),
        }
    }
}

impl From<dw_engine::EngineError> for Failure {
    fn from(e: dw_engine::EngineError) -> Self {
        MasternodesError::from(e).into()
    }
}

fn value(v: impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(v).map_err(|e| Failure {
        code: "internal".into(),
        message: e.to_string(),
    })
}

fn wallet(session: &Arc<NetworkSession>, arg: Option<String>) -> Result<WalletId, Failure> {
    match arg {
        Some(w) => Ok(w.parse()?),
        None => Ok(crate::first_wallet(session)?),
    }
}

/// Starts SPV, then waits until the list is synced and every loaded wallet
/// scanned `height`.
fn wait_for_list(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    mn: &Masternodes,
    timeout: Duration,
    height: Option<u32>,
) -> Result<(), Failure> {
    let deadline = Instant::now() + timeout;
    crate::pay::ensure_spv_running(engine, session, timeout).map_err(|message| Failure {
        code: "spv".into(),
        message,
    })?;
    let scanned = || -> Result<bool, Failure> {
        let Some(h) = height else { return Ok(true) };
        Ok(session.wallet_infos()?.iter().all(|w| {
            session
                .wallet_scan_height(&w.wallet_id)
                .is_some_and(|s| s >= h)
        }))
    };
    while !(mn.list_state()?.synced && scanned()?) {
        if Instant::now() >= deadline {
            return Err(MasternodesError::ListUnavailable.into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn command(engine: &Engine, session: &Arc<NetworkSession>, args: MnArgs) -> Result<Value, Failure> {
    let mn = session.masternodes();
    if args.spv || args.sync_height.is_some() {
        let timeout = Duration::from_secs(args.sync_timeout);
        wait_for_list(engine, session, &mn, timeout, args.sync_height)?;
    }
    match args.command {
        MnCommand::ListState => value(mn.list_state()?),
        MnCommand::List { wallet: w } => {
            let w = wallet(session, w)?;
            value(engine.block_on(mn.list(w))?)
        }
        MnCommand::Status {
            pro_tx_hash,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            value(engine.block_on(mn.masternode(w, pro_tx_hash))?)
        }
        MnCommand::Find { query } => value(engine.block_on(mn.find(query))?),
        MnCommand::Track { pro_tx_hash, label } => {
            value(engine.block_on(mn.track(pro_tx_hash, label))?)
        }
        MnCommand::Untrack { pro_tx_hash } => value(engine.block_on(mn.untrack(pro_tx_hash))?),
        MnCommand::Label { pro_tx_hash, label } => {
            value(engine.block_on(mn.set_label(pro_tx_hash, label))?)
        }
        MnCommand::Attach { pro_tx_hash, role } => {
            let mut key = Zeroizing::new(String::new());
            std::io::stdin()
                .read_to_string(&mut key)
                .map_err(|e| Failure {
                    code: "invalid_argument".into(),
                    message: format!("reading the key from stdin: {e}"),
                })?;
            let check = engine.block_on(mn.attach_key(pro_tx_hash, role, key))?;
            value(json!({ "role": role, "check": check }))
        }
        MnCommand::Detach { pro_tx_hash, role } => {
            let deleted = engine.block_on(mn.detach_key(pro_tx_hash, role))?;
            value(json!({ "role": role, "deleted": deleted }))
        }
        MnCommand::Payouts {
            pro_tx_hash,
            wallet: w,
            since,
        } => {
            let w = wallet(session, w)?;
            value(engine.block_on(mn.payouts(w, pro_tx_hash, since))?)
        }
        MnCommand::Credits { pro_tx_hash } => {
            value(engine.block_on(mn.evonode_credits(pro_tx_hash))?)
        }
    }
}

/// Runs one `mn` command and prints its JSON line. `Err` (exit status 1)
/// carries the message for stderr.
pub fn run(engine: &Engine, session: &Arc<NetworkSession>, args: MnArgs) -> Result<(), String> {
    let started_spv = args.spv || args.sync_height.is_some();
    let result = command(engine, session, args);
    if started_spv {
        crate::teardown::stop_spv(session, false).map_err(|e| e.to_string())?;
    }
    match result {
        Ok(v) => {
            println!("{}", json!({ "ok": true, "result": v }));
            Ok(())
        }
        Err(f) => {
            println!(
                "{}",
                json!({ "ok": false, "error": { "code": f.code, "message": f.message } })
            );
            Err(f.message)
        }
    }
}
