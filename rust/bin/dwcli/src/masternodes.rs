//! `dwcli mn …`: the masternodes facade (MG-03, `docs/contracts/mn-engine.md`)
//! and nothing else. Every command prints one JSON line on stdout:
//! `{"ok":true,"result":…}`, or `{"ok":false,"error":{"code":…,"message":…}}`
//! with exit status 1, whatever failed: the vault's unlock, a setup step
//! before the command, a refused command line, a panic (MG-03 D26), or the
//! teardown after it: the line is printed once SPV is stopped and the
//! engine shut down (D32). No message repeats an argument or an option's
//! value. A key to attach is read from stdin, never argv. The ProTx owner
//! commands (MG-04 D20) take a `MasternodeOp` grant capped at the fee the
//! facade previews; no command takes or signs with an operator secret. The
//! `mn shared` commands (MG-04b slice 1a) read a pasted message from stdin,
//! take a grant capped at 0 to give consent, and reach nothing that hands
//! bytes to the network.

use std::io::{Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use clap::{Args, Subcommand};
use dw_engine::governance::{GovernanceError, NameChoice};
use dw_engine::masternodes::{
    AttachedKeyRole, CollateralChoice, DissolveRequest, EvonodeTerms, JoinSharedRequest, KeepOr,
    MasternodeType, Masternodes, MasternodesError, OperatorChoice, PayoutChoice,
    PlatformNodeChoice, RegistrationRequest, RewardChoice, ShareSlot, ShareTerms,
    StartSharedRequest, UpdateRegistrarRequest, UpdateShareRequest, VotingChoice,
};
use dw_engine::{Engine, EngineError, NetworkSession, WalletId};
use dw_protx::params::MAX_ENVELOPE_BYTES;
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
    /// Shared masternodes (v24, MG-04b slice 1a): the session protocol up to
    /// this wallet's consent signature. Nothing here hands bytes to the
    /// network.
    Shared(Box<SharedArgs>),
}

#[derive(Args)]
struct SharedArgs {
    /// The wallet to act for (default: the oldest wallet).
    #[arg(long, global = true)]
    wallet: Option<String>,
    #[command(subcommand)]
    command: SharedCommand,
}

#[derive(Subcommand)]
enum SharedCommand {
    /// What this session can do with shared masternodes.
    Capability,
    /// The wallet's sessions, newest first.
    Sessions,
    /// One session in full.
    Show { session: String },
    /// Start a session as its coordinator: saves a draft and prints it with
    /// the Invitation to hand to the other parties.
    Start(Box<StartSharedArgs>),
    /// Change the coordinator's registration fee (a draft no other party
    /// replied to), in duffs.
    Fee { session: String, fee: u64 },
    /// Read a pasted or saved message from stdin (never argv) and apply it:
    /// an Invitation opens a session to `join`, a later copy advances it.
    Import {
        /// The maintenance session a bare signatures array is for.
        #[arg(long)]
        target: Option<String>,
    },
    /// Take a share of an imported Invitation; prints the session and the
    /// Details to send back to the coordinator.
    Join {
        session: String,
        /// The share to take (default: the one open row).
        #[arg(long)]
        share_index: Option<u16>,
        /// Where this share's rewards go: `refund` (its refund address),
        /// `new` (a new receive address of the wallet) or an address.
        #[arg(long, default_value = "refund", value_parser = parse_reward)]
        reward: RewardChoice,
    },
    /// The message to hand to the other parties now. `--out` also writes
    /// its text to that file.
    Message {
        session: String,
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    /// Coordinator: freeze the terms and sign this wallet's consent (a
    /// `MasternodeOp` grant capped at 0); prints the Locked Terms.
    Lock {
        session: String,
        /// The `revision` `show` printed for the table reviewed: another
        /// one now (a reply absorbed since) is refused `conflict`.
        #[arg(long)]
        revision: u32,
    },
    /// Coordinator: back to a draft before the funding is combined.
    Unlock { session: String },
    /// Participant: check the locked registration and sign this wallet's
    /// consent (a `MasternodeOp` grant capped at 0); prints the Approval.
    Approve {
        session: String,
        /// The `consent_hash` `show` printed for the terms reviewed:
        /// another one now (a re-locked copy imported since) is refused
        /// `conflict`.
        #[arg(long)]
        consent_hash: String,
    },
    /// Participant: sign this wallet's inputs on the combined funding (a
    /// `MasternodeOp` grant capped at its share); prints the Signed
    /// Contribution.
    Sign { session: String },
    /// Coordinator: sign this wallet's inputs last and send the
    /// registration, or send it again once out (a grant capped at its share
    /// and the fee); prints the Final Record.
    Send { session: String },
    /// Spend one of an out session's reserved coins back to the wallet, so
    /// the registration can no longer be mined (a grant capped at
    /// `--max-fee`).
    Abandon {
        session: String,
        /// Duffs; the default covers the minimum relay fee of the
        /// one-input transaction.
        #[arg(long, default_value_t = 1_000)]
        max_fee: u64,
    },
    /// Delete a session nothing of which may be out, and unlock its coins.
    Discard { session: String },
    /// Preview the ProUpShareTx that would change a share's reward address
    /// of a confirmed registration; issues, locks and signs nothing.
    PreviewUpdateShare {
        pro_tx_hash: String,
        /// The share (row index) of this wallet.
        #[arg(long)]
        share_index: u16,
        /// `refund` (the share's refund address), `new` (a new receive
        /// address of the wallet) or an address: always named.
        #[arg(long, value_parser = parse_reward)]
        reward: RewardChoice,
    },
    /// Sign and send the ProUpShareTx `preview-update-share` shows (a grant
    /// capped at its fee); prints the operation.
    UpdateShare {
        pro_tx_hash: String,
        #[arg(long)]
        share_index: u16,
        #[arg(long, value_parser = parse_reward)]
        reward: RewardChoice,
    },
    /// Preview dissolving the masternode now by one of this wallet's
    /// shares: what each share gets back, the penalty while early, the
    /// fee; signs nothing.
    PreviewDissolve {
        pro_tx_hash: String,
        /// The share (row index) of this wallet: the actor.
        #[arg(long)]
        share_index: u16,
        /// Duffs, from the share; dash-qt's default.
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
    },
    /// Sign and send the dissolution `preview-dissolve` shows (a grant
    /// capped at its penalty and fee); prints the operation.
    Dissolve {
        pro_tx_hash: String,
        /// The share (row index) of this wallet: the actor.
        #[arg(long)]
        share_index: u16,
        /// Duffs, from the share; dash-qt's default.
        #[arg(long, default_value_t = 1_000)]
        fee: u64,
    },
}

#[derive(Args)]
struct StartSharedArgs {
    /// `LABEL:AMOUNT_DUFFS`; repeat it for 2 to 8 shares, the first being
    /// this wallet's.
    #[arg(long = "share", required = true, value_parser = parse_share)]
    shares: Vec<ShareSlot>,
    /// `a.b.c.d:port`; repeat it for several.
    #[arg(long = "service", required = true)]
    services: Vec<String>,
    /// The operator's BLS public key (96 hex); default: the keychain's
    /// next unused.
    #[arg(long, conflicts_with = "operator_index")]
    operator_key: Option<String>,
    #[arg(long)]
    operator_index: Option<u32>,
    /// Basis points (0..=10000).
    #[arg(long, default_value_t = 0)]
    operator_reward: u16,
    /// Another wallet's voting address; default: the keychain's next unused.
    #[arg(long, conflicts_with = "voting_index")]
    voting_address: Option<String>,
    #[arg(long)]
    voting_index: Option<u32>,
    /// Blocks after registration in which an owner leaving early pays the
    /// penalty.
    #[arg(long)]
    early_period_blocks: u32,
    /// The early-exit penalty, in duffs.
    #[arg(long)]
    early_penalty: u64,
    /// Free text naming who runs the node.
    #[arg(long)]
    node_run_by: Option<String>,
    /// This wallet's rewards: `refund` (its refund address), `new` (a new
    /// receive address of the wallet) or an address.
    #[arg(long, default_value = "refund", value_parser = parse_reward)]
    reward: RewardChoice,
    /// The registration's network fee, in duffs, paid by this wallet.
    #[arg(long)]
    fee: u64,
}

impl StartSharedArgs {
    fn request(self) -> StartSharedRequest {
        StartSharedRequest {
            shares: self.shares,
            terms: ShareTerms {
                service: self.services.join(","),
                operator: match (self.operator_key, self.operator_index) {
                    (Some(public_key), _) => OperatorChoice::Given { public_key },
                    (None, Some(index)) => OperatorChoice::Keychain { index },
                    (None, None) => OperatorChoice::NextUnused,
                },
                operator_reward: self.operator_reward,
                voting: match (self.voting_address, self.voting_index) {
                    (Some(address), _) => VotingChoice::Address { address },
                    (None, Some(index)) => VotingChoice::Keychain { index },
                    (None, None) => VotingChoice::NextUnused,
                },
                early_period_blocks: self.early_period_blocks,
                early_penalty: self.early_penalty,
            },
            fee: self.fee,
            reward: self.reward,
            node_run_by: self.node_run_by,
        }
    }
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

/// `LABEL:AMOUNT_DUFFS`: the amount follows the last colon, so a label may
/// hold one. The text names no value (clap's error isn't printed anyway).
fn parse_share(s: &str) -> Result<ShareSlot, String> {
    const FORM: &str = "LABEL:AMOUNT_DUFFS, a label and a whole number of duffs";
    let (label, amount) = s.rsplit_once(':').ok_or(FORM)?;
    if label.trim().is_empty() {
        return Err(FORM.into());
    }
    Ok(ShareSlot {
        label: label.into(),
        amount: amount.parse().map_err(|_| FORM)?,
    })
}

/// `refund`, `new`, or an address (the facade checks it).
fn parse_reward(s: &str) -> Result<RewardChoice, String> {
    match s {
        "" => Err("one of refund, new, or an address".into()),
        "refund" => Ok(RewardChoice::Refund),
        "new" => Ok(RewardChoice::NewAddress),
        address => Ok(RewardChoice::Address {
            address: address.into(),
        }),
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
        MnCommand::Shared(a) => shared_command(engine, session, passphrase, &mn, *a),
    }
}

/// The message text on stdin, whole. It is never in argv and no failure
/// quotes it. At most [`MAX_ENVELOPE_BYTES`] + 1 bytes are read: more is
/// the facade's `shared_message{too_large}`, before invalid UTF-8 is its
/// `shared_message{parse}` (MG-04b D34), whose reader takes a string.
fn read_message() -> Result<String, Failure> {
    read_message_from(std::io::stdin().lock())
}

fn read_message_from(input: impl Read) -> Result<String, Failure> {
    let refused = |reason: &str| -> Failure {
        MasternodesError::SharedMessage {
            reason: reason.into(),
            share: None,
        }
        .into()
    };
    let mut bytes = Vec::new();
    input
        .take(MAX_ENVELOPE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Failure {
            code: "invalid_argument".into(),
            message: "reading the message from stdin failed".into(),
        })?;
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(refused("too_large"));
    }
    String::from_utf8(bytes).map_err(|_| refused("parse"))
}

/// `mn shared …`: one facade call each. `lock` and `approve` charge nothing,
/// so their `MasternodeOp` grant is capped at 0 (consent only); `sign` and
/// `send` cap theirs at the session's `debit` (MG-04b D9), read first.
fn shared_command(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    mn: &Masternodes,
    args: SharedArgs,
) -> Result<Value, Failure> {
    let w = wallet(session, args.wallet)?;
    // A grant capped at the session's `debit` (MG-04b D9).
    let debit_grant = |id: &str| -> Result<String, Failure> {
        let debit = engine.block_on(mn.shared_session(w, id.to_string()))?.debit;
        masternode_grant(session, passphrase, w, debit.unwrap_or(0))
    };
    match args.command {
        SharedCommand::Capability => value(engine.block_on(mn.shared_capability(w))?),
        SharedCommand::Sessions => value(engine.block_on(mn.shared_sessions(w))?),
        SharedCommand::Show { session: id } => value(engine.block_on(mn.shared_session(w, id))?),
        SharedCommand::Start(a) => value(engine.block_on(mn.start_shared(w, a.request()))?),
        SharedCommand::Fee { session: id, fee } => {
            value(engine.block_on(mn.set_shared_fee(w, id, fee))?)
        }
        SharedCommand::Import { target } => {
            let text = read_message()?;
            value(engine.block_on(mn.import_shared_message(w, text, target))?)
        }
        SharedCommand::Join {
            session: id,
            share_index,
            reward,
        } => {
            let request = JoinSharedRequest {
                share_index,
                reward,
            };
            value(engine.block_on(mn.join_shared(w, id, request))?)
        }
        SharedCommand::Message { session: id, out } => {
            let message = engine.block_on(mn.shared_message(w, id))?;
            if let Some(path) = out {
                std::fs::write(path, &message.text).map_err(|_| Failure {
                    code: "invalid_argument".into(),
                    message: "writing --out failed".into(),
                })?;
            }
            value(message)
        }
        SharedCommand::Lock {
            session: id,
            revision,
        } => {
            let grant = masternode_grant(session, passphrase, w, 0)?;
            value(engine.block_on(mn.lock_shared_terms(w, id, revision, grant))?)
        }
        SharedCommand::Unlock { session: id } => {
            value(engine.block_on(mn.unlock_shared_terms(w, id))?)
        }
        SharedCommand::Approve {
            session: id,
            consent_hash,
        } => {
            let grant = masternode_grant(session, passphrase, w, 0)?;
            value(engine.block_on(mn.approve_shared(w, id, consent_hash, grant))?)
        }
        SharedCommand::Sign { session: id } => {
            let grant = debit_grant(&id)?;
            value(engine.block_on(mn.sign_shared_funding(w, id, grant))?)
        }
        SharedCommand::Send { session: id } => {
            let grant = debit_grant(&id)?;
            value(engine.block_on(mn.send_shared_registration(w, id, grant))?)
        }
        SharedCommand::Abandon {
            session: id,
            max_fee,
        } => {
            let grant = masternode_grant(session, passphrase, w, max_fee)?;
            value(engine.block_on(mn.abandon_shared_session(w, id, grant))?)
        }
        SharedCommand::Discard { session: id } => {
            engine.block_on(mn.discard_shared_session(w, id))?;
            value(json!({ "discarded": true }))
        }
        SharedCommand::PreviewUpdateShare {
            pro_tx_hash,
            share_index,
            reward,
        } => {
            let request = UpdateShareRequest {
                pro_tx_hash,
                share_index,
                reward,
            };
            value(engine.block_on(mn.preview_update_share(w, request))?)
        }
        SharedCommand::UpdateShare {
            pro_tx_hash,
            share_index,
            reward,
        } => {
            let request = UpdateShareRequest {
                pro_tx_hash,
                share_index,
                reward,
            };
            let preview = engine.block_on(mn.preview_update_share(w, request.clone()))?;
            let grant = masternode_grant(session, passphrase, w, preview.total_debit)?;
            value(engine.block_on(mn.update_share(w, request, grant))?)
        }
        SharedCommand::PreviewDissolve {
            pro_tx_hash,
            share_index,
            fee,
        } => {
            let request = DissolveRequest {
                pro_tx_hash,
                share_index,
                fee,
            };
            value(engine.block_on(mn.preview_dissolve(w, request))?)
        }
        SharedCommand::Dissolve {
            pro_tx_hash,
            share_index,
            fee,
        } => {
            let request = DissolveRequest {
                pro_tx_hash,
                share_index,
                fee,
            };
            let preview = engine.block_on(mn.preview_dissolve(w, request.clone()))?;
            let grant = masternode_grant(session, passphrase, w, preview.total_debit)?;
            value(engine.block_on(mn.dissolve(w, request, grant))?)
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
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The `mn shared` arguments of a command line, or clap's refusal.
    fn shared(args: &[&str]) -> Result<SharedArgs, clap::error::ErrorKind> {
        let cli = crate::Cli::try_parse_from(
            ["dwcli", "--datadir", "d", "mn", "shared"]
                .into_iter()
                .chain(args.iter().copied()),
        )
        .map_err(|e| e.kind())?;
        match cli.command {
            crate::Command::Mn(MnArgs {
                command: MnCommand::Shared(a),
                ..
            }) => Ok(*a),
            _ => panic!("not an `mn shared` command"),
        }
    }

    const SESSION: &str = "ab";
    const HASH: &str = "cd";

    #[test]
    fn test_QT_126_every_shared_subcommand_parses_with_its_required_arguments() {
        let start = [
            "start",
            "--share",
            "me:10000000000000",
            "--share",
            "you:10000000000000",
            "--service",
            "1.2.3.4:19999",
            "--early-period-blocks",
            "10",
            "--early-penalty",
            "5",
            "--fee",
            "1000",
        ];
        for args in [
            &["capability"][..],
            &["sessions"],
            &["show", SESSION],
            &start,
            &["fee", SESSION, "1000"],
            &["import"],
            &["import", "--target", SESSION],
            &["join", SESSION],
            &["join", SESSION, "--share-index", "1", "--reward", "new"],
            &["message", SESSION],
            &["message", SESSION, "--out", "f.json"],
            &["lock", SESSION, "--revision", "3"],
            &["unlock", SESSION],
            &["approve", SESSION, "--consent-hash", HASH],
            &["discard", SESSION],
            &[
                "preview-update-share",
                HASH,
                "--share-index",
                "0",
                "--reward",
                "refund",
            ],
            &["preview-dissolve", HASH, "--share-index", "0"],
            &["dissolve", HASH, "--share-index", "1", "--fee", "1000000"],
            &["sessions", "--wallet", "w"],
        ] {
            assert!(shared(args).is_ok(), "{args:?}");
        }
    }

    #[test]
    fn test_QT_126_shared_subcommands_refuse_missing_or_malformed_arguments() {
        use clap::error::ErrorKind::*;
        let refused = |args: &[&str]| shared(args).err();
        assert_eq!(refused(&[]), Some(DisplayHelpOnMissingArgumentOrSubcommand));
        for args in [
            &["show"][..],
            &["fee", SESSION],
            &["fee", SESSION, "-1"],
            &["fee", SESSION, "x"],
            &["join"],
            &["join", SESSION, "--share-index", "70000"],
            &["join", SESSION, "--reward", ""],
            &["message"],
            &["lock"],
            // Lock and approve name the copy reviewed (MG-04b D56).
            &["lock", SESSION],
            &["lock", SESSION, "--revision", "-1"],
            &["lock", SESSION, "--revision", "x"],
            &["unlock"],
            &["approve"],
            &["approve", SESSION],
            &["discard"],
            &["start"],
            // The update names its reward, and both numbers.
            &["preview-update-share", HASH, "--share-index", "0"],
            &["preview-update-share", HASH, "--reward", "refund"],
            &[
                "preview-update-share",
                "--share-index",
                "0",
                "--reward",
                "new",
            ],
        ] {
            assert!(refused(args).is_some(), "{args:?}");
        }
        // Slice 1b's commands name their session or masternode.
        for args in [
            &["sign"][..],
            &["send"],
            &["abandon"],
            &["abandon", SESSION, "--max-fee", "x"],
            &["update-share", HASH, "--share-index", "0"],
            // Slice 2a's: the masternode and the share are named.
            &["preview-dissolve", "--share-index", "0"],
            &["preview-dissolve", HASH],
            &["dissolve", HASH, "--share-index", "0", "--fee", "-1"],
            &["dissolve", HASH, "--share-index", "x"],
        ] {
            assert!(refused(args).is_some(), "{args:?}");
        }
    }

    /// MG-04b D56: lock and approve carry the revision and consent hash
    /// the session record showed.
    #[test]
    fn test_QT_126_lock_and_approve_name_the_copy_reviewed() {
        let a = shared(&["lock", SESSION, "--revision", "7"]).unwrap();
        assert!(matches!(
            a.command,
            SharedCommand::Lock { ref session, revision: 7 } if session == SESSION
        ));
        let a = shared(&["approve", SESSION, "--consent-hash", HASH]).unwrap();
        assert!(matches!(
            a.command,
            SharedCommand::Approve { ref session, ref consent_hash }
                if session == SESSION && consent_hash == HASH
        ));
    }

    /// fixes-1a Nit 4: stdin is read up to the envelope limit + 1 bytes;
    /// more is `too_large` before any UTF-8 check, and an endless input
    /// ends there.
    #[test]
    fn test_QT_126_a_message_is_read_bounded() {
        let reason = |r: Result<String, Failure>| {
            let Err(f) = r else {
                panic!("read");
            };
            assert_eq!(f.code, "masternodes.shared_message");
            f.message
        };
        assert_eq!(
            read_message_from(&b"{\"a\": 1}"[..]).ok().as_deref(),
            Some("{\"a\": 1}")
        );
        let at_limit = vec![b' '; MAX_ENVELOPE_BYTES];
        assert_eq!(
            read_message_from(&at_limit[..]).ok().map(|t| t.len()),
            Some(MAX_ENVELOPE_BYTES)
        );
        assert!(reason(read_message_from(std::io::repeat(b'x'))).ends_with(": too_large"));
        let mut invalid = vec![0xff];
        invalid.extend(vec![b' '; MAX_ENVELOPE_BYTES]);
        assert!(reason(read_message_from(&invalid[..])).ends_with(": too_large"));
        assert!(reason(read_message_from(&[0xff, b' '][..])).ends_with(": parse"));
    }

    #[test]
    fn test_QT_126_share_flags_parse_label_and_duffs() {
        let share = |s| parse_share(s);
        let slot = |label: &str, amount| ShareSlot {
            label: label.into(),
            amount,
        };
        assert_eq!(share("alice:10000"), Ok(slot("alice", 10_000)));
        assert_eq!(share("a:b:7"), Ok(slot("a:b", 7)));
        assert_eq!(share("my share:0"), Ok(slot("my share", 0)));
        assert_eq!(share("x:18446744073709551615"), Ok(slot("x", u64::MAX)));
        for bad in [
            "",
            "alice",
            ":5",
            " :5",
            "alice:",
            "alice:-1",
            "alice:1.5",
            "alice:x",
        ] {
            assert!(share(bad).is_err(), "{bad:?}");
        }
        // The error names no value.
        let text = share("hunter2:secret").unwrap_err();
        assert!(
            !text.contains("hunter2") && !text.contains("secret"),
            "{text}"
        );
    }

    #[test]
    fn test_QT_126_reward_flags_name_refund_new_or_an_address() {
        assert_eq!(parse_reward("refund"), Ok(RewardChoice::Refund));
        assert_eq!(parse_reward("new"), Ok(RewardChoice::NewAddress));
        assert_eq!(
            parse_reward("yXaddr"),
            Ok(RewardChoice::Address {
                address: "yXaddr".into()
            })
        );
        assert!(parse_reward("").is_err());
    }

    #[test]
    fn test_QT_126_start_flags_build_the_whole_request() {
        let a = shared(&[
            "start",
            "--share",
            "me:100",
            "--share",
            "you:200",
            "--service",
            "1.2.3.4:19999",
            "--service",
            "5.6.7.8:19999",
            "--operator-index",
            "3",
            "--operator-reward",
            "250",
            "--voting-address",
            "yVote",
            "--early-period-blocks",
            "720",
            "--early-penalty",
            "9",
            "--node-run-by",
            "Bob",
            "--reward",
            "yReward",
            "--fee",
            "1500",
        ])
        .ok()
        .unwrap();
        let SharedCommand::Start(start) = a.command else {
            panic!("not start")
        };
        let slot = |label: &str, amount| ShareSlot {
            label: label.into(),
            amount,
        };
        assert_eq!(
            start.request(),
            StartSharedRequest {
                shares: vec![slot("me", 100), slot("you", 200)],
                terms: ShareTerms {
                    service: "1.2.3.4:19999,5.6.7.8:19999".into(),
                    operator: OperatorChoice::Keychain { index: 3 },
                    operator_reward: 250,
                    voting: VotingChoice::Address {
                        address: "yVote".into()
                    },
                    early_period_blocks: 720,
                    early_penalty: 9,
                },
                fee: 1500,
                reward: RewardChoice::Address {
                    address: "yReward".into()
                },
                node_run_by: Some("Bob".into()),
            }
        );
        // The defaults: the keychain's next unused keys, rewards to the
        // refund address, no operator reward.
        let a = shared(&[
            "start",
            "--share",
            "me:100",
            "--service",
            "1.2.3.4:19999",
            "--early-period-blocks",
            "1",
            "--early-penalty",
            "0",
            "--fee",
            "1",
        ])
        .ok()
        .unwrap();
        let SharedCommand::Start(start) = a.command else {
            panic!("not start")
        };
        let r = start.request();
        assert_eq!(r.terms.operator, OperatorChoice::NextUnused);
        assert_eq!(r.terms.voting, VotingChoice::NextUnused);
        assert_eq!(r.terms.operator_reward, 0);
        assert_eq!(r.reward, RewardChoice::Refund);
        assert_eq!(r.node_run_by, None);
        // A key is given or picked by index, not both.
        let both = shared(&[
            "start",
            "--share",
            "me:1",
            "--service",
            "1.2.3.4:1",
            "--early-period-blocks",
            "1",
            "--early-penalty",
            "0",
            "--fee",
            "1",
            "--operator-key",
            "k",
            "--operator-index",
            "1",
        ]);
        assert!(both.is_err());
    }

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
