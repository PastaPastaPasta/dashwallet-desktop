//! `dwcli mn …`: the masternodes facade (MG-03, `docs/contracts/mn-engine.md`)
//! and nothing else. Every command prints one JSON line on stdout:
//! `{"ok":true,"result":…}`, or `{"ok":false,"error":{"code":…,"message":…}}`
//! with exit status 1, whatever failed: the vault's unlock, a setup step
//! before the command, a refused command line, a panic (MG-03 D26), or the
//! teardown after it: the line is printed once SPV is stopped and the
//! engine shut down (D32). No message repeats an argument or an option's
//! value. A key to attach is read from stdin, never argv. The ProTx owner
//! commands (MG-04 D20) take a `MasternodeOp` grant capped at the fee the
//! facade previews; no command takes or signs with an operator secret.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use dw_engine::masternodes::{
    AttachedKeyRole, CollateralChoice, EvonodeTerms, KeepOr, MasternodeType, Masternodes,
    MasternodesError, OperatorChoice, PayoutChoice, PlatformNodeChoice, RegistrationRequest,
    UpdateRegistrarRequest, VotingChoice,
};
use dw_engine::{Engine, NetworkSession, WalletId};
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
    /// Register a masternode (a version-2 ProRegTx). With
    /// `--external-address`, only prepares it: the result's `sign_request`
    /// is what the collateral's key signs (`mn register-submit`).
    Register(Box<RegisterArgs>),
    /// Finish an external registration with the collateral key's
    /// `signmessage` signature (base64).
    RegisterSubmit {
        operation: String,
        #[arg(long)]
        signature: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Re-plan an external registration whose fee inputs were spent.
    RegisterRefresh {
        operation: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Change a masternode's operator key, voting key or payout (a
    /// version-2 ProUpRegTx signed by its owner key). A key not given is
    /// kept from the list.
    UpdateRegistrar(Box<UpdateRegistrarArgs>),
    /// The wallet's ProTx operations, newest first.
    Operations {
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Hand an operation's saved transaction to the network again.
    Resend {
        operation: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Delete an operation nothing of which may be out.
    Discard {
        operation: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Unlock a masternode's collateral coin, and keep it unlocked.
    ReleaseCollateral {
        pro_tx_hash: String,
        #[arg(long)]
        wallet: Option<String>,
    },
    /// Lock it again.
    LockCollateral {
        pro_tx_hash: String,
        #[arg(long)]
        wallet: Option<String>,
    },
}

#[derive(Args)]
struct RegisterArgs {
    /// `fund-new` (a new output of the ProRegTx), or an outpoint
    /// `<txid>-<n>`: the wallet's coin, or with `--external-address`
    /// another wallet's.
    #[arg(long)]
    collateral: String,
    /// The external collateral's P2PKH address.
    #[arg(long)]
    external_address: Option<String>,
    /// `a.b.c.d[:port]`.
    #[arg(long)]
    service: String,
    #[arg(long)]
    evonode: bool,
    /// The operator's BLS public key (96 hex); default: the keychain's
    /// next unused.
    #[arg(long, conflicts_with = "operator_index")]
    operator_key: Option<String>,
    #[arg(long)]
    operator_index: Option<u32>,
    /// Basis points (0..=10000).
    #[arg(long, default_value_t = 0)]
    operator_reward: u16,
    #[arg(long)]
    owner_index: Option<u32>,
    /// Another wallet's voting address; default: the keychain's next unused.
    #[arg(long, conflicts_with = "voting_index")]
    voting_address: Option<String>,
    #[arg(long)]
    voting_index: Option<u32>,
    /// Default: a new receive address of the wallet.
    #[arg(long)]
    payout: Option<String>,
    /// An evonode's platform node id (40 hex); default: the keychain's.
    #[arg(long, conflicts_with = "platform_node_index")]
    platform_node_id: Option<String>,
    #[arg(long)]
    platform_node_index: Option<u32>,
    #[arg(long)]
    platform_p2p_port: Option<u16>,
    #[arg(long)]
    platform_http_port: Option<u16>,
    /// Only preview: plan and check, sign nothing.
    #[arg(long)]
    preview: bool,
    #[arg(long)]
    wallet: Option<String>,
}

#[derive(Args)]
struct UpdateRegistrarArgs {
    pro_tx_hash: String,
    /// The new operator's BLS public key (96 hex).
    #[arg(long, conflicts_with = "operator_index")]
    operator_key: Option<String>,
    /// The keychain's operator public key at this index.
    #[arg(long)]
    operator_index: Option<u32>,
    /// Another wallet's voting address.
    #[arg(long, conflicts_with = "voting_index")]
    voting_address: Option<String>,
    #[arg(long)]
    voting_index: Option<u32>,
    /// `new` (a new receive address of the wallet) or an address: always
    /// named, the wallet doesn't know the current one for sure (MG-04 D10).
    #[arg(long)]
    payout: String,
    /// Only preview: plan and check, sign nothing.
    #[arg(long)]
    preview: bool,
    #[arg(long)]
    wallet: Option<String>,
}

impl UpdateRegistrarArgs {
    fn request(&self) -> UpdateRegistrarRequest {
        UpdateRegistrarRequest {
            pro_tx_hash: self.pro_tx_hash.clone(),
            operator: match (&self.operator_key, self.operator_index) {
                (Some(k), _) => KeepOr::Set(OperatorChoice::Given {
                    public_key: k.clone(),
                }),
                (None, Some(index)) => KeepOr::Set(OperatorChoice::Keychain { index }),
                (None, None) => KeepOr::Keep,
            },
            voting: match (&self.voting_address, self.voting_index) {
                (Some(a), _) => KeepOr::Set(VotingChoice::Address { address: a.clone() }),
                (None, Some(index)) => KeepOr::Set(VotingChoice::Keychain { index }),
                (None, None) => KeepOr::Keep,
            },
            payout: match self.payout.as_str() {
                "new" => PayoutChoice::NewAddress,
                a => PayoutChoice::Address { address: a.into() },
            },
        }
    }
}

impl RegisterArgs {
    fn request(&self) -> RegistrationRequest {
        let collateral = match (self.collateral.as_str(), &self.external_address) {
            ("fund-new", _) => CollateralChoice::FundNew,
            (outpoint, Some(address)) => CollateralChoice::External {
                outpoint: outpoint.into(),
                address: address.clone(),
            },
            (outpoint, None) => CollateralChoice::ExistingUtxo {
                outpoint: outpoint.into(),
            },
        };
        RegistrationRequest {
            kind: if self.evonode {
                MasternodeType::Evonode
            } else {
                MasternodeType::Regular
            },
            collateral,
            service: self.service.clone(),
            operator: match (&self.operator_key, self.operator_index) {
                (Some(k), _) => OperatorChoice::Given {
                    public_key: k.clone(),
                },
                (None, Some(index)) => OperatorChoice::Keychain { index },
                (None, None) => OperatorChoice::NextUnused,
            },
            operator_reward: self.operator_reward,
            owner_index: self.owner_index,
            voting: match (&self.voting_address, self.voting_index) {
                (Some(a), _) => VotingChoice::Address { address: a.clone() },
                (None, Some(index)) => VotingChoice::Keychain { index },
                (None, None) => VotingChoice::NextUnused,
            },
            payout: match &self.payout {
                Some(a) => PayoutChoice::Address { address: a.clone() },
                None => PayoutChoice::NewAddress,
            },
            evonode: self.evonode.then(|| EvonodeTerms {
                node: match (&self.platform_node_id, self.platform_node_index) {
                    (Some(id), _) => PlatformNodeChoice::Given {
                        node_id: id.clone(),
                    },
                    (None, Some(index)) => PlatformNodeChoice::Keychain { index },
                    (None, None) => PlatformNodeChoice::NextUnused,
                },
                p2p_port: self.platform_p2p_port,
                http_port: self.platform_http_port,
            }),
        }
    }
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

/// A failed command: the facade's code and message.
pub struct Failure {
    code: String,
    message: String,
}

impl From<MasternodesError> for Failure {
    fn from(e: MasternodesError) -> Self {
        // `TxDraft`'s failures are one engine code, `send`; the line names
        // the variant as the UI's `send.*` codes do.
        let code = match e.engine() {
            Some(dw_engine::EngineError::Send(f)) => format!("send.{}", snake(&format!("{f:?}"))),
            _ => e.code().into(),
        };
        Self {
            code,
            message: e.to_string(),
        }
    }
}

/// `GrantExceeded { … }` → `grant_exceeded`: the leading identifier only.
fn snake(debug: &str) -> String {
    let mut out = String::new();
    for (i, c) in debug
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .enumerate()
    {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// A one-use `MasternodeOp` grant for `wallet` capped at `max_duffs`.
fn masternode_grant(
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    wallet: WalletId,
    max_duffs: u64,
) -> Result<String, Failure> {
    let grant = session
        .vault()
        .authorize(
            GrantPurpose::MasternodeOp { max_duffs },
            Some(&wallet.0),
            crate::pay::credential(session, passphrase),
        )
        .map_err(MasternodesError::from)?;
    Ok(grant.id)
}

/// The fee an operation's grant must cover (its saved transaction's).
fn operation_fee(
    engine: &Engine,
    mn: &Masternodes,
    wallet: WalletId,
    operation: &str,
) -> Result<u64, Failure> {
    let ops = engine.block_on(mn.operations(wallet))?;
    Ok(ops
        .iter()
        .find(|o| o.id == operation)
        .ok_or(MasternodesError::OperationNotFound)?
        .fee)
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
    // At start dash-spv replays the stored lists, which read synced, then
    // asks a peer for the tip's QRInfo, and the list reads unsynced until
    // the answer: the list must stay synced for a while (MG-04 D45).
    let mut steady = 0;
    while steady < LIST_STEADY_CHECKS {
        if Instant::now() >= deadline {
            return Err(MasternodesError::ListUnavailable.into());
        }
        steady = if mn.list_state()?.synced {
            steady + 1
        } else {
            0
        };
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

/// [`wait_for_list`]: 250 ms checks the list must read synced in a row (2 s).
const LIST_STEADY_CHECKS: u32 = 8;

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
        MnCommand::Register(a) => {
            let w = wallet(session, a.wallet.clone())?;
            let request = a.request();
            if a.preview {
                return value(engine.block_on(mn.preview_registration(w, request))?);
            }
            if a.external_address.is_some() {
                return value(engine.block_on(mn.prepare_external_registration(w, request))?);
            }
            let preview = engine.block_on(mn.preview_registration(w, request.clone()))?;
            let grant = masternode_grant(session, passphrase, w, preview.total_debit)?;
            value(engine.block_on(mn.register(w, request, grant))?)
        }
        MnCommand::RegisterSubmit {
            operation,
            signature,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            let fee = operation_fee(engine, &mn, w, &operation)?;
            let grant = masternode_grant(session, passphrase, w, fee)?;
            value(engine.block_on(mn.submit_external_registration(w, operation, signature, grant))?)
        }
        MnCommand::RegisterRefresh {
            operation,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            value(engine.block_on(mn.refresh_external_registration(w, operation))?)
        }
        MnCommand::UpdateRegistrar(a) => {
            let w = wallet(session, a.wallet.clone())?;
            let request = a.request();
            let preview = engine.block_on(mn.preview_update_registrar(w, request.clone()))?;
            if a.preview {
                return value(preview);
            }
            let grant = masternode_grant(session, passphrase, w, preview.total_debit)?;
            value(engine.block_on(mn.update_registrar(w, request, grant))?)
        }
        MnCommand::Operations { wallet: w } => {
            let w = wallet(session, w)?;
            value(engine.block_on(mn.operations(w))?)
        }
        MnCommand::Resend {
            operation,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            let fee = operation_fee(engine, &mn, w, &operation)?;
            let grant = masternode_grant(session, passphrase, w, fee)?;
            value(engine.block_on(mn.resend_operation(w, operation, grant))?)
        }
        MnCommand::Discard {
            operation,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            engine.block_on(mn.discard_operation(w, operation))?;
            value(json!({ "discarded": true }))
        }
        MnCommand::ReleaseCollateral {
            pro_tx_hash,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            engine.block_on(mn.release_collateral(w, pro_tx_hash))?;
            value(json!({ "locked": false }))
        }
        MnCommand::LockCollateral {
            pro_tx_hash,
            wallet: w,
        } => {
            let w = wallet(session, w)?;
            engine.block_on(mn.lock_collateral(w, pro_tx_hash))?;
            value(json!({ "locked": true }))
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
