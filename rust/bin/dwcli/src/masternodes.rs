//! `dwcli mn …`: the masternodes facade (MG-03, `docs/contracts/mn-engine.md`)
//! and nothing else. Every command prints one JSON line on stdout:
//! `{"ok":true,"result":…}`, or `{"ok":false,"error":{"code":…,"message":…}}`
//! with exit status 1, whatever failed: the vault's unlock, a setup step
//! before the command, a refused command line, a panic (MG-03 D26), or the
//! teardown after it: the line is printed once SPV is stopped and the
//! engine shut down (D32). No message repeats an argument or an option's
//! value. A key to attach is read from stdin, never argv.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use dw_engine::governance::{GovernanceError, NameChoice};
use dw_engine::masternodes::{AttachedKeyRole, Masternodes, MasternodesError};
use dw_engine::{Engine, EngineError, NetworkSession, WalletId};
use dw_vault::GrantPurpose;
use serde::Serialize;
use serde_json::{Value, json};
use zeroize::Zeroizing;

/// Set once the JSON line is printed, so `main` prints none after it.
static PRINTED: AtomicBool = AtomicBool::new(false);

/// Prints the line once, best effort: a broken stdout must not turn the
/// exit into a panic.
fn print_line(line: &Value) {
    if !PRINTED.swap(true, Ordering::SeqCst) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}").and_then(|()| out.flush());
    }
}

fn error_line(code: &str, message: &str) -> Value {
    json!({ "ok": false, "error": { "code": code, "message": message } })
}

/// The line of an `mn` command that failed before it ran (the passphrase
/// file, the data root, opening the network), unless one was printed.
/// `message` is fixed text naming the option at fault (D32).
pub fn report_setup_failure(message: &'static str) {
    print_line(&error_line("setup", message));
}

/// The line after a panic, unless one was printed. The payload may quote
/// an argument, so it isn't used.
pub fn report_panic() {
    let f = panicked();
    print_line(&error_line(&f.code, &f.message));
}

/// The line of a refused `mn` command line (`text` names no value).
pub fn report_parse_failure(text: &str) {
    print_line(&error_line("invalid_argument", text));
}

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
    /// The open contested DPNS names and the wallet's masternodes that can
    /// vote on them (proved Platform reads; MG-07).
    Names {
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Vote on a contested DPNS name with the voting keys of `--masternode`
    /// (repeat it), under a `Governance` grant: the passphrase file unlocks
    /// an encrypted vault (MG-07).
    VoteName {
        label: String,
        /// `abstain`, `lock`, or `identity:<base58 id>` of a contender.
        #[arg(long, value_parser = parse_choice)]
        choice: NameChoice,
        #[arg(long = "masternode", required = true)]
        masternodes: Vec<String>,
        #[arg(long)]
        wallet: Option<String>,
    },
}

impl MnArgs {
    pub fn starts_spv(&self) -> bool {
        self.spv || self.sync_height.is_some()
    }
}

fn parse_role(s: &str) -> Result<AttachedKeyRole, String> {
    match s {
        "owner" => Ok(AttachedKeyRole::Owner),
        "voting" => Ok(AttachedKeyRole::Voting),
        "payout" => Ok(AttachedKeyRole::Payout),
        _ => Err("one of owner, voting, payout".into()),
    }
}

fn parse_choice(s: &str) -> Result<NameChoice, String> {
    match s {
        "abstain" => Ok(NameChoice::Abstain),
        "lock" => Ok(NameChoice::Lock),
        _ => match s.strip_prefix("identity:") {
            Some(id) if !id.is_empty() => Ok(NameChoice::TowardsIdentity {
                identity: id.into(),
            }),
            _ => Err("one of abstain, lock, identity:<base58 id>".into()),
        },
    }
}

/// A failed command: the facade's code and message.
pub struct Failure {
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

/// The governance facade's codes. A contest's label and a contender's
/// identity are public Platform data, quoted as dw-app quotes them (MG-07
/// D19); an invalid argument, which may be a proTxHash, is worded without
/// it (D32).
impl From<GovernanceError> for Failure {
    fn from(e: GovernanceError) -> Self {
        let message = match &e {
            GovernanceError::Engine(EngineError::InvalidArgument(_)) => {
                "the label, --choice or a --masternode is invalid".into()
            }
            e => e.to_string(),
        };
        Self {
            code: e.code().into(),
            message,
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
    // Its message names the timeout, an option's value (D32).
    crate::pay::ensure_spv_running(engine, session, timeout).map_err(|_| Failure {
        code: "spv".into(),
        message: "SPV did not start within --sync-timeout".into(),
    })?;
    let scanned = || -> Result<bool, Failure> {
        let Some(h) = height else { return Ok(true) };
        Ok(session.wallet_infos()?.iter().all(|w| {
            session
                .wallet_scan_height(&w.wallet_id)
                .is_some_and(|s| s >= h)
        }))
    };
    while !mn.list_state()?.synced {
        if Instant::now() >= deadline {
            return Err(MasternodesError::ListUnavailable.into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    while !scanned()? {
        if Instant::now() >= deadline {
            return Err(Failure {
                code: "spv".into(),
                message: "the wallets didn't scan --sync-height within --sync-timeout".into(),
            });
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn command(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    args: MnArgs,
) -> Result<Value, Failure> {
    let mn = session.masternodes();
    if args.starts_spv() {
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
            // Room for a key, so reading doesn't reallocate (and leave
            // copies behind).
            let mut key = Zeroizing::new(String::with_capacity(256));
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
        MnCommand::Names { wallet: w } => {
            let w = wallet(session, w)?;
            value(engine.block_on(session.governance().contested_names(w))?)
        }
        MnCommand::VoteName {
            label,
            choice,
            masternodes,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            let grant = session
                .vault()
                .authorize(
                    GrantPurpose::Governance,
                    Some(&w.0),
                    crate::pay::credential(session, passphrase),
                )
                .map_err(EngineError::from)?;
            let gov = session.governance();
            value(engine.block_on(gov.vote_name(w, label, choice, masternodes, grant.id))?)
        }
    }
}

/// Unlocks the vault if a passphrase was given and runs one `mn` command;
/// `main` tears down, then [`report`]s. A wrong passphrase is the m1
/// `vault` error, which names no input.
pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    args: MnArgs,
) -> Result<Value, Failure> {
    crate::unlock(engine, session, passphrase)
        .map_err(Failure::from)
        .and_then(|()| command(engine, session, passphrase, args))
}

/// What a panic in the command answers; the payload may quote an argument.
pub fn panicked() -> Failure {
    Failure {
        code: "internal".into(),
        message: "dwcli panicked; the command's outcome is unknown".into(),
    }
}

/// Prints the command's line once the teardown ended (`torn_down`: SPV
/// stopped and the engine shut down, in time): a success becomes the
/// `shutdown` error when it didn't, since the process then fails (D32).
/// `Err` (exit status 1) carries the message for stderr.
pub fn report(result: Result<Value, Failure>, torn_down: bool) -> Result<(), String> {
    let result = match result {
        Ok(_) if !torn_down => Err(Failure {
            code: "shutdown".into(),
            message: "the command ran, but stopping SPV or shutting the engine down failed".into(),
        }),
        other => other,
    };
    match result {
        Ok(v) => {
            print_line(&json!({ "ok": true, "result": v }));
            Ok(())
        }
        Err(f) => {
            print_line(&error_line(&f.code, &f.message));
            Err(f.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn governance_failures_quote_the_label_and_identity_only() {
        let label = |e| Failure::from(e).message;
        let closed = GovernanceError::ContestClosed {
            label: "c0ntest".into(),
        };
        assert_eq!(label(closed), "the contest for c0ntest is closed");
        let missing = GovernanceError::ContestNotFound {
            label: "c0ntest".into(),
        };
        assert_eq!(label(missing), "no contest for c0ntest");
        let id = "8ZtWrmgDwJHJnwyoNNsCs5CbE5LedD8eKzmTzpRzR97P";
        let other = GovernanceError::NotAContender {
            identity: id.into(),
        };
        assert_eq!(label(other), format!("{id} is not a contender"));
        let invalid = EngineError::InvalidArgument(format!("{:?} is not hex", "ab".repeat(32)));
        let f = Failure::from(GovernanceError::Engine(invalid));
        assert_eq!(f.code, "invalid_argument");
        assert!(!f.message.contains("abab"), "{}", f.message);
    }
}
