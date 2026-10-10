//! E0-09 DashPay commands: `dashpay`, `identity`, `name`, `contact`,
//! `pay-contact`, `profile` and `invite`. They call the facade only
//! (`NetworkSession::dashpay`, `docs/contracts/m4-dashpay-engine.md`), never
//! platform-wallet, so the T2/T3 suites drive DashPay the way a UI host does.
//!
//! Every command prints one JSON line on stdout: `{"ok":true,"result":…}`
//! with the facade's record in its serde form, or
//! `{"ok":false,"error":{"code":…,"message":…,"params":{…}}}` with exit
//! status 1. `code` is the stable code (m4 §4, or m1's for vault and engine
//! calls); `params` holds the code's parameters, such as `call` of
//! `platform.not_implemented`. Calls still stubbed report that code and fill
//! in as the DP tasks land.
//!
//! Writes ask for their grant the way a host does: a `PlatformOp` capped by
//! the quote's or `grant_request`'s `GrantRequest`, from `Vault.authorize`;
//! the request is printed with the result. The calls the contract gives no
//! quote (`identity resume`, `finish-asset-locks`, `faucet-key`) take their
//! caps as `--max-duffs` and `--max-credits`.
//!
//! Bearer inputs (invitation links, scanned payloads that may carry a
//! `dapk`) are read from stdin or `--input-file`, never from argv (m4 §1).
//!
//! State the engine keeps per session (scan ids, avatar candidates, leases,
//! a running registration, the dispatch tombstones `resolve-lock` reads)
//! dies with a one-shot process. `dashpay session` keeps one engine open
//! and runs one command per stdin line: `{"args":[…],"input":…,"id":…}`
//! in, the command's JSON line (with `id`) out. A request line is read and
//! parsed into zeroizing buffers only (`request.rs`), and its `args` are
//! checked against the command grammar (`grammar.rs`) before clap copies
//! them. A panic, in dwcli or in an engine task, answers its request with
//! `internal` (outcome unknown) and the session goes on, unless the health
//! probe then fails, which ends it with `session_poisoned`.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use dw_engine::platform::{
    ActivityFilter, AvatarChange, AvatarError, AvatarSize, AvatarSource, BearerSecret,
    ContactError, ContactQuery, ContactSection, ContactSort, CreditsError, DashPay, GrantAction,
    GrantRequest, IdentityError, InitialProfile, InvitationError, NameError, PlatformError,
    PrivateDetails, ProfileEdit, RegistrationError, RegistrationFunding, RegistrationRequest,
    RegistrationWait, WithdrawAmount, check_username,
};
use dw_engine::{Engine, EngineError, NetworkSession, TASK_PANICKED};
use dw_vault::{GrantPurpose, VaultError};
use serde::Serialize;
use serde_json::{Value, json};
use zeroize::{Zeroize, Zeroizing};

use crate::pay::{credential, wallet_id};

mod grammar;
mod request;

pub(crate) use request::ZeroStdin;

#[derive(Subcommand)]
pub enum DashPayCommand {
    /// DashPay status, sync, notifications and the flow journal.
    Dashpay {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: StatusCmd,
    },
    /// The wallet's identities: list, registration, credits.
    Identity {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: IdentityCmd,
    },
    /// Usernames (DPNS).
    Name {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: NameCmd,
    },
    /// Contacts, contact requests and payment locks.
    Contact {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: ContactCmd,
    },
    /// Pay a contact (DP3-01: `TxDraft` with `Recipient::Contact`; until
    /// then `not_implemented{call: "Recipient::Contact"}`).
    PayContact {
        #[command(flatten)]
        common: Common,
        #[command(flatten)]
        who: Who,
        contact: String,
        /// Duffs.
        #[arg(long)]
        amount: u64,
        #[arg(long)]
        subtract_fee: bool,
        #[arg(long)]
        note: Option<String>,
    },
    /// The DashPay profile.
    Profile {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Invitation links.
    Invite {
        #[command(flatten)]
        common: Common,
        #[command(subcommand)]
        cmd: InviteCmd,
    },
}

#[derive(Args)]
pub struct Common {
    /// Wallet id; the oldest wallet when omitted.
    #[arg(long, global = true)]
    wallet: Option<String>,
    /// Start SPV, and with it the DashPay bring-up, before the command, and
    /// stop it after.
    #[arg(long, global = true)]
    spv: bool,
}

#[derive(Args)]
pub struct Who {
    /// The wallet's identity (Base58); the main identity when omitted.
    #[arg(long)]
    identity: Option<String>,
}

impl Who {
    /// `--identity`, or the main identity.
    fn or_main(self, dp: &DashPay) -> Result<String, CliError> {
        if let Some(id) = self.identity {
            return Ok(id);
        }
        dp.identities()?
            .into_iter()
            .find(|i| i.is_main)
            .map(|i| i.identity)
            .ok_or_else(|| PlatformError::Identity(IdentityError::NotFound).into())
    }
}

/// The caps of a `PlatformOp` grant for a call with no quote (0: that part
/// is not granted).
#[derive(Args)]
pub struct Caps {
    #[arg(long, default_value_t = 0)]
    max_duffs: u64,
    #[arg(long, default_value_t = 0)]
    max_credits: u64,
}

impl From<Caps> for GrantRequest {
    fn from(c: Caps) -> Self {
        GrantRequest {
            max_duffs: c.max_duffs,
            max_credits: c.max_credits,
        }
    }
}

/// Where a bearer input comes from: stdin, or this file.
#[derive(Args)]
pub struct SecretInput {
    #[arg(long)]
    input_file: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum StatusCmd {
    /// Run DashPay commands from stdin in one engine session, one JSON
    /// request per line: `{"args":["contact","scan"],"input":"…","id":1}`.
    /// `input` is the command's bearer input; `id` is echoed. Each answer
    /// is that command's JSON line plus `id` (none for a line that is not
    /// a JSON object), in order; the last line is `{"requests":N}`. With
    /// `--spv`, a failed SPV start answers nothing but that error.
    Session,
    /// The banner state (`DashPayStatus`).
    Status,
    /// Run one DashPay pass now and report it.
    Sync,
    /// Tools ▸ Information's DashPay card (`DashPaySyncStatus`).
    SyncStatus,
    /// The wallet's live flow leases.
    Leases,
    /// What the engine knows about a handed-off artifact: a txid, a
    /// state-transition hash or a funding step id.
    DispatchStatus { artifact: String },
    /// Notifications: pending, new and earlier events.
    Events {
        #[command(flatten)]
        who: Who,
        /// An event id.
        #[arg(long)]
        cursor: Option<u64>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// The unread notification count.
    Unread {
        #[command(flatten)]
        who: Who,
    },
    /// Mark events up to this id read.
    MarkRead {
        #[command(flatten)]
        who: Who,
        up_to: u64,
    },
}

#[derive(Subcommand)]
pub enum IdentityCmd {
    /// The wallet's identities.
    List,
    /// One identity with its revision and public keys.
    Show {
        #[command(flatten)]
        who: Who,
    },
    /// Make an identity the main one.
    SetMain { identity: String },
    /// Fetch an identity's credit balance (`null`: not on Platform yet).
    Balance {
        #[command(flatten)]
        who: Who,
    },
    /// Discover the identities of this wallet's seed.
    Discover,
    /// Register an identity with a username: quote, authorize, start.
    /// Funded from the Core balance unless a funding option is given.
    Register {
        #[command(flatten)]
        reg: RegistrationArgs,
        /// Fund with a stashed invitation (`invite stash` id).
        #[arg(long, group = "funding")]
        invitation_id: Option<String>,
        /// Register the name for an identity that already exists.
        #[arg(long, group = "funding")]
        existing_identity: Option<String>,
        /// Fund with a faucet asset lock: the `identity faucet-key` id
        /// (developer builds).
        #[arg(long, group = "funding", requires = "faucet_proof_file")]
        faucet_key: Option<String>,
        /// File holding the faucet's `assetLockProof` (hex).
        #[arg(long, requires = "faucet_key")]
        faucet_proof_file: Option<PathBuf>,
    },
    /// The registration rows and what each waits for.
    Registrations,
    /// Advance a parked registration (authorizes with the caps when it
    /// waits for an unlock or a confirmation).
    Resume {
        draft: String,
        #[command(flatten)]
        caps: Caps,
    },
    /// Discard a registration whose funds were never committed.
    Discard { draft: String },
    /// Tools ▸ Repair "Finish transfers": resume stranded asset locks.
    FinishAssetLocks {
        #[command(flatten)]
        caps: Caps,
    },
    /// Derive an asset-lock key for the faucet (developer builds).
    FaucetKey {
        #[command(flatten)]
        caps: Caps,
    },
    /// Top up an identity's credits from the Core balance.
    TopUp {
        #[command(flatten)]
        who: Who,
        #[arg(long)]
        duffs: u64,
        /// Print the quote only.
        #[arg(long)]
        quote_only: bool,
    },
    /// Withdraw credits to a Core address.
    Withdraw {
        #[command(flatten)]
        who: Who,
        #[arg(long, required_unless_present = "quote_only")]
        to: Option<String>,
        #[arg(long, required_unless_present = "all", conflicts_with = "all")]
        credits: Option<u64>,
        /// Everything but the fee reserve.
        #[arg(long)]
        all: bool,
        /// Print the quote only.
        #[arg(long)]
        quote_only: bool,
    },
    /// Credit costs and thresholds.
    Costs,
}

/// What `identity register` and `invite claim` share.
#[derive(Args)]
pub struct RegistrationArgs {
    #[arg(long)]
    label: String,
    /// The name used while a contested `label` is decided.
    #[arg(long)]
    temporary_label: Option<String>,
    #[arg(long)]
    display_name: Option<String>,
    #[arg(long)]
    public_message: Option<String>,
    /// Avatar for the initial profile, prepared from this URL.
    #[arg(long)]
    avatar_url: Option<String>,
    /// Print the quote only.
    #[arg(long)]
    quote_only: bool,
}

#[derive(Subcommand)]
pub enum NameCmd {
    /// The offline rule checklist (needs no wallet).
    Check { label: String },
    /// Whether a label is available, taken or contested.
    Availability { label: String },
    /// Register an extra name for an identity.
    Register {
        #[command(flatten)]
        who: Who,
        label: String,
    },
    /// The identity's own contest for `label`.
    Contest {
        #[command(flatten)]
        who: Who,
        label: String,
    },
    /// DPNS prefix search.
    Search {
        prefix: String,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Exact lookup (`null` if no such name).
    Resolve { username: String },
}

#[derive(Subcommand)]
pub enum ContactCmd {
    /// Contacts by section (requests, contacts, pending, hidden).
    List {
        #[command(flatten)]
        who: Who,
        /// requests | contacts | pending | hidden; repeatable, all when
        /// omitted.
        #[arg(long = "section", value_parser = snake::<ContactSection>)]
        sections: Vec<ContactSection>,
        /// display_name | username | date_added | last_activity
        #[arg(long, default_value = "display_name", value_parser = snake::<ContactSort>)]
        sort: ContactSort,
        #[arg(long)]
        text: Option<String>,
    },
    /// One contact (`null` if unknown).
    Show {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    /// Contacts waiting for an unlock to finish their crypto.
    PendingSetup,
    /// Whether a contact request to `contact` is possible.
    Eligibility {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    /// Send a contact request to `contact`, or with `--scanned` to the
    /// scanned payload read from stdin (`dash:?du=&dapk=` or a user link).
    Request {
        #[command(flatten)]
        who: Who,
        #[arg(required_unless_present = "scanned", conflicts_with = "scanned")]
        contact: Option<String>,
        #[arg(long)]
        scanned: bool,
        #[command(flatten)]
        input: SecretInput,
    },
    /// Verify a scanned payload read from stdin.
    Scan {
        #[command(flatten)]
        input: SecretInput,
    },
    /// Accept a contact request (send the reverse request).
    Accept {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    Ignore {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    Unignore {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    /// Set the private details (alias, note, hidden); the whole set, so an
    /// omitted option is cleared.
    Details {
        #[command(flatten)]
        who: Who,
        contact: String,
        #[arg(long)]
        alias: Option<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        hidden: bool,
        /// Authorize publishing them as encrypted `contactInfo`.
        #[arg(long)]
        publish: bool,
    },
    /// Add the DashPay keys 4–5 to an identity that lacks them.
    EnableKeys {
        #[command(flatten)]
        who: Who,
    },
    /// The identity's `dashpay://user` link.
    Link {
        #[command(flatten)]
        who: Who,
    },
    /// The payment lock an ambiguous broadcast set (`null` if none).
    Lock {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    /// Reconcile a payment lock with the dispatch evidence.
    ResolveLock {
        #[command(flatten)]
        who: Who,
        contact: String,
    },
    /// Payments to and from a contact, newest first.
    Activity {
        #[command(flatten)]
        who: Who,
        contact: String,
        /// all | sent | received
        #[arg(long, default_value = "all", value_parser = snake::<ActivityFilter>)]
        filter: ActivityFilter,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// The Pay screen's frequent contacts.
    Frequent {
        #[command(flatten)]
        who: Who,
        #[arg(long, default_value_t = 8)]
        limit: u32,
    },
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// The stored profile (`null` if none).
    Show {
        #[command(flatten)]
        who: Who,
    },
    /// Display name and public message limits.
    Limits,
    /// Whether avatar uploads (Imgur) are configured.
    UploadAvailable,
    /// Publish the whole profile: an omitted `--display-name` or
    /// `--public-message` is cleared, and the avatar is kept unless an
    /// avatar option is given.
    Set {
        #[command(flatten)]
        who: Who,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        public_message: Option<String>,
        #[arg(long, group = "avatar")]
        avatar_url: Option<String>,
        /// An image file, uploaded when the engine needs a URL for it.
        #[arg(long, group = "avatar")]
        avatar_file: Option<PathBuf>,
        #[arg(long, group = "avatar")]
        remove_avatar: bool,
    },
    /// Any identity's avatar thumbnail (PNG, base64; `null` if none).
    Avatar {
        identity: String,
        /// small | large
        #[arg(long, default_value = "small", value_parser = snake::<AvatarSize>)]
        size: AvatarSize,
    },
}

#[derive(Subcommand)]
pub enum InviteCmd {
    /// Claim an invitation read from stdin: stash it, quote, authorize and
    /// start the registration it funds. The link stays stashed (also with
    /// `--quote-only`, and on errors, which carry its `link_id`); `invite
    /// forget` drops it.
    Claim {
        #[command(flatten)]
        reg: RegistrationArgs,
        #[command(flatten)]
        input: SecretInput,
    },
    /// Stash an invitation link read from stdin; prints its id.
    Stash {
        #[command(flatten)]
        input: SecretInput,
    },
    /// A stashed link's status.
    Status { link_id: String },
    /// The stashed links, oldest first.
    Pending,
    /// Delete a stashed link.
    Forget { link_id: String },
}

/// Parses a facade enum from its serde (snake-case) name, so the CLI and
/// the JSON spell values alike.
fn snake<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, String> {
    serde_json::from_value(Value::String(s.to_string())).map_err(|e| e.to_string())
}

/// A failed command: a stable code, diagnostic text and the code's
/// parameters.
#[derive(Debug)]
pub(crate) struct CliError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) params: Value,
    /// A panic (dwcli's, or an engine task's) caused it: the outcome is
    /// unknown, and a session checks the engine's health.
    panicked: bool,
}

impl CliError {
    fn new(code: &'static str, message: impl ToString, params: Value) -> Self {
        Self {
            code,
            message: message.to_string(),
            params,
            panicked: false,
        }
    }

    /// Whether this is a facade's `internal` error for a panicking engine
    /// task: its `detail` is [`TASK_PANICKED`], or ends with it (the
    /// engine error's `Display`).
    fn is_task_panic(&self) -> bool {
        self.code.ends_with("internal")
            && self
                .params
                .get("detail")
                .and_then(Value::as_str)
                .is_some_and(|d| d.ends_with(TASK_PANICKED))
    }

    /// `invalid_argument` and `internal` carry `detail`, as the facade's
    /// do.
    fn invalid(message: impl ToString) -> Self {
        Self::detail("invalid_argument", message.to_string())
    }

    fn is_panic(&self) -> bool {
        self.panicked
    }

    fn detail(code: &'static str, detail: String) -> Self {
        Self::new(code, &detail, json!({"detail": detail}))
    }
}

fn platform_params(e: &PlatformError) -> Value {
    use PlatformError as P;
    match e {
        P::Unavailable
        | P::Timeout
        | P::ProofInvalid
        | P::TrustMismatch
        | P::ContextUnavailable
        | P::SignerUnavailable
        | P::SeedMismatch
        | P::GrantInvalid
        | P::Cancelled
        | P::LeaseExpired
        | P::NetworkNotOpen
        | P::WalletNotFound
        | P::Identity(IdentityError::NotFound) => json!({}),
        P::InsufficientCredits { needed, available } => {
            json!({"needed": needed, "available": available})
        }
        P::GrantExceeded {
            purpose,
            needed,
            remaining,
        } => json!({"purpose": purpose, "needed": needed, "remaining": remaining}),
        P::BroadcastUnknown { artifact } | P::WillBeSent { artifact } => {
            json!({"artifact": artifact})
        }
        P::NeedsGrant { purpose } => json!({"purpose": purpose}),
        P::Identity(IdentityError::KeysMissing { purpose }) => json!({"purpose": purpose}),
        P::LeaseRevoked { cause } => json!({"cause": cause}),
        P::FeatureOff { feature } => json!({"feature": feature}),
        P::NotImplemented { call } => json!({"call": call}),
        P::InvalidArgument { detail } | P::Storage { detail } | P::Internal { detail } => {
            json!({"detail": detail})
        }
    }
}

fn name_params(e: &NameError) -> Value {
    match e {
        NameError::Invalid { rules } => json!({"rules": rules}),
        NameError::Taken
        | NameError::ContestOpen
        | NameError::Locked
        | NameError::UnavailableForInvite => json!({}),
        NameError::Platform(p) => platform_params(p),
    }
}

fn invitation_params(e: &InvitationError) -> Value {
    match e {
        InvitationError::Invalid
        | InvitationError::Claimed
        | InvitationError::Expired
        | InvitationError::AlreadyHasIdentity => json!({}),
        InvitationError::Platform(p) => platform_params(p),
    }
}

fn registration_params(e: &RegistrationError) -> Value {
    use RegistrationError as R;
    match e {
        R::InProgress | R::IslockTimeout | R::AlreadyHasUsername => json!({}),
        R::FundingInsufficient { needed, available } => {
            json!({"needed": needed, "available": available})
        }
        R::Recoverable { draft } => json!({"draft": draft}),
        R::Name(n) => name_params(n),
        R::Invitation(i) => invitation_params(i),
        R::Platform(p) => platform_params(p),
    }
}

fn contact_params(e: &ContactError) -> Value {
    use ContactError as C;
    match e {
        C::Ineligible { reason } => json!({"reason": reason}),
        C::PaymentLocked { txid } => json!({"txid": txid}),
        C::AlreadyContact | C::RequestPending | C::IsSelf | C::ChannelBroken | C::ScanExpired => {
            json!({})
        }
        C::Platform(p) => platform_params(p),
    }
}

fn avatar_params(e: &AvatarError) -> Value {
    match e {
        AvatarError::TooLarge
        | AvatarError::Unsupported
        | AvatarError::FetchFailed
        | AvatarError::HashMismatch
        | AvatarError::UploadUnconfigured => json!({}),
        AvatarError::Platform(p) => platform_params(p),
    }
}

fn credits_params(e: &CreditsError) -> Value {
    match e {
        CreditsError::FundingInsufficient { needed, available } => {
            json!({"needed": needed, "available": available})
        }
        CreditsError::BelowMinimum { min } => json!({"min": min}),
        CreditsError::Platform(p) => platform_params(p),
    }
}

macro_rules! facade_error {
    ($($ty:ty => $params:ident),* $(,)?) => {$(
        impl From<$ty> for CliError {
            fn from(e: $ty) -> Self {
                let e = Self::new(e.code(), &e, $params(&e));
                if e.is_task_panic() {
                    return panic_error("an engine task", "the call");
                }
                e
            }
        }
    )*};
}

facade_error!(
    PlatformError => platform_params,
    NameError => name_params,
    InvitationError => invitation_params,
    RegistrationError => registration_params,
    ContactError => contact_params,
    AvatarError => avatar_params,
    CreditsError => credits_params,
);

impl From<VaultError> for CliError {
    fn from(e: VaultError) -> Self {
        Self::new(e.code(), &e, json!({}))
    }
}

impl From<EngineError> for CliError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Vault(v) => v.into(),
            e if e.is_task_panic() => panic_error("an engine task", "the call"),
            e => Self::new(e.code(), &e, json!({})),
        }
    }
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        EngineError::from(e).into()
    }
}

/// The line a command prints.
pub(crate) fn envelope(result: &Result<Value, CliError>) -> Value {
    match result {
        Ok(v) => json!({"ok": true, "result": v}),
        Err(e) => json!({
            "ok": false,
            "error": {"code": e.code, "message": e.message, "params": e.params},
        }),
    }
}

fn out<T: Serialize>(value: T) -> Result<Value, CliError> {
    serde_json::to_value(value).map_err(|e| CliError::detail("internal", e.to_string()))
}

/// The largest bearer input read; a link or a scanned payload is far
/// smaller.
const MAX_SECRET: usize = 16 * 1024;

/// Reads a bearer input into one zeroized buffer that never reallocates.
/// Errors never quote it.
fn read_secret(input: &SecretInput, stdin: &mut dyn Read) -> Result<BearerSecret, CliError> {
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_SECRET + 1));
    let limit = (MAX_SECRET + 1) as u64;
    match &input.input_file {
        Some(path) => std::fs::File::open(path)?
            .take(limit)
            .read_to_end(&mut bytes)?,
        None => stdin.take(limit).read_to_end(&mut bytes)?,
    };
    if bytes.len() > MAX_SECRET {
        return Err(CliError::invalid(format!(
            "the input is over {MAX_SECRET} bytes"
        )));
    }
    let end = bytes.trim_ascii_end().len();
    bytes.truncate(end);
    let start = bytes.len() - bytes.trim_ascii_start().len();
    bytes.drain(..start);
    if bytes.is_empty() {
        return Err(CliError::invalid("the input is empty"));
    }
    Ok(BearerSecret::from_utf8(bytes)?)
}

struct Ctx<'a> {
    engine: &'a Engine,
    session: &'a Arc<NetworkSession>,
    passphrase: Option<&'a Zeroizing<Vec<u8>>>,
    wallet: Option<String>,
}

impl Ctx<'_> {
    fn block_on<F: std::future::Future>(&self, fut: F) -> F::Output {
        self.engine.block_on(fut)
    }

    fn dashpay(&self) -> Result<Arc<DashPay>, CliError> {
        let id = match &self.wallet {
            Some(w) => wallet_id(w).map_err(CliError::invalid)?,
            None => crate::first_wallet(self.session)?,
        };
        Ok(self.session.dashpay(id))
    }

    /// A `PlatformOp` grant for the wallet with `caps`.
    fn platform_op(&self, dp: &DashPay, caps: GrantRequest) -> Result<String, CliError> {
        let purpose = GrantPurpose::PlatformOp {
            max_duffs: caps.max_duffs,
            max_credits: caps.max_credits,
        };
        self.grant(dp, purpose)
    }

    /// The grant `discover_identities` takes.
    fn identity_scan(&self, dp: &DashPay) -> Result<String, CliError> {
        self.grant(dp, GrantPurpose::IdentityScan)
    }

    fn grant(&self, dp: &DashPay, purpose: GrantPurpose) -> Result<String, CliError> {
        let grant = self.session.vault().authorize(
            purpose,
            Some(&dp.wallet_id().0),
            credential(self.session, self.passphrase),
        )?;
        Ok(grant.id)
    }
}

/// Set once the command's JSON line is printed, so `main` prints one for a
/// failure before the command ran.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// The answer a poisoned session still owes its last request (with the
/// request's `id`). It is written after `main` wiped the passphrase, as
/// the write may block.
type Owed = (Option<request::Id>, Result<Value, CliError>);
static OWED: std::sync::Mutex<Option<Owed>> = std::sync::Mutex::new(None);

/// Prints one line, best effort: a broken stdout must not turn the exit
/// into a panic. Only the first line counts ([`REPORTED`]).
fn print_line(line: &Value) {
    if !REPORTED.swap(true, Ordering::SeqCst) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}").and_then(|()| out.flush());
    }
}

/// Unlocks the vault if a passphrase was given, then runs `cmd`. `main`
/// prints the result with [`report`] after the engine shut down, or with
/// [`report_abandoned`] if it abandoned the engine ([`crate::teardown`]).
pub fn run(
    engine: &Engine,
    session: &Arc<NetworkSession>,
    passphrase: Option<&Zeroizing<Vec<u8>>>,
    cmd: DashPayCommand,
) -> Result<Value, CliError> {
    let base = Ctx {
        engine,
        session,
        passphrase,
        wallet: None,
    };
    let result = crate::unlock(engine, session, passphrase)
        .map_err(CliError::from)
        .and_then(|()| exec(&base, cmd, &mut ZeroStdin::new(), &mut std::io::stdout()));
    // An abandoned engine is left alone: nothing more starts on its way
    // out (a refused `-spawn-fail` thread has also used up the process's
    // threads).
    #[cfg(debug_assertions)]
    if injected_fault("stall") && crate::teardown::abandoned().is_none() {
        wedge_engine(session);
    }
    result
}

/// Prints the command's JSON line; the error is returned for the exit
/// status. `teardown` (the engine's shutdown) never changes the line: a
/// write that succeeded is reported as such, and a failed shutdown is a
/// warning on stderr.
pub fn report(result: Result<Value, CliError>, teardown: Result<(), String>) -> Result<(), String> {
    if let Err(e) = teardown {
        eprintln!("warning: {e}");
    }
    print_line(&envelope(&result));
    result.map(drop).map_err(|e| e.message)
}

/// The lines of an abandoned engine's forced exit, written once `main`
/// wiped the passphrase: the answer a poisoned session owes, then the
/// result's line. Best effort: an error changes nothing. A write blocks
/// while stdout is full and nobody reads it (callers must drain stdout).
pub fn report_abandoned(result: &Result<Value, CliError>) {
    write_abandoned(&mut std::io::stdout().lock(), result);
}

fn write_abandoned(out: &mut dyn Write, result: &Result<Value, CliError>) {
    REPORTED.store(true, Ordering::SeqCst);
    let owed = OWED.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some((id, answer_to)) = owed {
        let _ = answer(out, id.as_ref(), &answer_to);
    }
    let _ = answer(out, None, result);
}

/// Debug builds under fault injection: says on stderr whether `main` wiped
/// the passphrase it held before its forced exit (which runs no
/// destructor).
pub fn report_wiped(passphrase: Option<&Zeroizing<Vec<u8>>>) {
    #[cfg(debug_assertions)]
    if std::env::var_os("DWCLI_FAULT_INJECT").is_some() {
        let state = match passphrase {
            None => "absent",
            Some(p) if p.is_empty() => "wiped",
            Some(_) => "held",
        };
        eprintln!("dwcli: forced exit, passphrase {state}");
    }
    #[cfg(not(debug_assertions))]
    let _ = passphrase;
}

/// The JSON line for a DashPay command that failed before it ran (the
/// passphrase file, the data root, opening the network), unless one was
/// printed.
pub fn report_setup_failure(message: &str) {
    let e = CliError::detail("setup", message.to_string());
    print_line(&envelope(&Err(e)));
}

/// The line for a DashPay command under `--no-platform`, printed before
/// anything is opened.
pub fn refuse_no_platform() -> Result<(), String> {
    let off = PlatformError::FeatureOff {
        feature: "platform".into(),
    };
    report(Err(off.into()), Ok(()))
}

/// The JSON line after a panic in a one-shot command, unless one was
/// printed. The panic hook prints only where it happened.
pub fn report_panic() {
    print_line(&envelope(&Err(panic_error("dwcli", "the command"))));
}

/// What a panic answers: `internal`, saying the outcome is unknown (a write
/// may or may not have gone through) and never quoting the payload.
pub(crate) fn panic_error(who: &str, what: &str) -> CliError {
    let mut e = CliError::detail(
        "internal",
        format!("{who} panicked; {what}'s outcome is unknown"),
    );
    e.panicked = true;
    e
}

/// The panic hook for DashPay commands: the payload may quote an input, so
/// only the location is printed.
pub fn quiet_panics() {
    std::panic::set_hook(Box::new(|info| {
        let at = info.location().map_or_else(
            || "unknown".into(),
            |l| format!("{}:{}", l.file(), l.line()),
        );
        eprintln!("dwcli: panic at {at} (message withheld)");
    }));
}

/// clap's rendering quotes the arguments it refuses, which may be a bearer
/// input: this names the error kind and, where clap gives them, the
/// arguments involved, never a value.
pub(crate) fn clap_error_text(e: &clap::Error) -> String {
    use clap::error::{ContextKind, ContextValue, ErrorKind as K};
    let mut text = e.kind().to_string();
    let names_args = matches!(
        e.kind(),
        K::InvalidValue
            | K::ValueValidation
            | K::MissingRequiredArgument
            | K::ArgumentConflict
            | K::TooManyValues
            | K::TooFewValues
            | K::WrongNumberOfValues
            | K::NoEquals
    );
    if names_args {
        for (kind, value) in e.context() {
            if matches!(kind, ContextKind::InvalidArg | ContextKind::PriorArg) {
                match value {
                    ContextValue::String(name) => text += &format!(": {name}"),
                    ContextValue::Strings(names) => text += &format!(": {}", names.join(", ")),
                    _ => {}
                }
            }
        }
    }
    text
}

#[cfg(test)]
thread_local! {
    /// A test's `DWCLI_FAULT_INJECT`, for this thread only.
    static TEST_FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Debug builds only, `DWCLI_FAULT_INJECT`: with `panic`, `dashpay status`
/// panics (with a bearer-shaped payload the hooks must withhold); with
/// `task-panic` an engine task it runs does, and with `spv-task-panic` one
/// that `--spv`'s start runs first. A `-poison` suffix also makes the
/// health probe panic, and `-wedge` (after a task panic) then blocks every
/// engine worker, so the probe times out. `stall` blocks every engine
/// worker after any DashPay command, with no panic, so only the shutdown's
/// deadline ends the process. A `-spawn-fail` suffix starts no teardown
/// thread (see `teardown`).
#[cfg(debug_assertions)]
fn injected_fault(point: &str) -> bool {
    #[cfg(test)]
    let fault = TEST_FAULT.get().map(String::from);
    #[cfg(not(test))]
    let fault = None;
    let Some(v) = fault.or_else(|| std::env::var("DWCLI_FAULT_INJECT").ok()) else {
        return false;
    };
    match point {
        "poison" | "wedge" => v.ends_with(&format!("-{point}")),
        _ => {
            v.split_once("-poison")
                .or(v.split_once("-wedge"))
                .or(v.split_once("-spawn-fail"))
                .map_or(&*v, |(f, _)| f)
                == point
        }
    }
}

/// Blocks every worker of the engine runtime: parks tasks until one no
/// longer starts. The threads that spawned them are left behind. A thread
/// that cannot start ends it, without a panic.
#[cfg(debug_assertions)]
fn wedge_engine(session: &Arc<NetworkSession>) {
    loop {
        let (tx, rx) = std::sync::mpsc::channel();
        let session = Arc::clone(session);
        let spawned = std::thread::Builder::new().spawn(move || {
            crate::teardown::wait(session.run_on_engine(async move {
                let _ = tx.send(());
                loop {
                    std::thread::park();
                }
            }))
        });
        if spawned.is_err() || rx.recv_timeout(Duration::from_secs(1)).is_err() {
            return;
        }
    }
}

/// The injected `dashpay status` faults (see [`injected_fault`]).
#[cfg(debug_assertions)]
fn inject_status_faults(ctx: &Ctx, cmd: &DashPayCommand) -> Result<(), CliError> {
    let status = matches!(
        cmd,
        DashPayCommand::Dashpay {
            cmd: StatusCmd::Status,
            ..
        }
    );
    if status && injected_fault("panic") {
        panic!("injected fault carrying dashpay://invite?pk=PANIC-SECRET");
    }
    if status && injected_fault("task-panic") {
        panic_engine_task(ctx.engine, ctx.session)?;
    }
    Ok(())
}

/// The injected `--spv` start fault (see [`injected_fault`]).
#[cfg(debug_assertions)]
pub(crate) fn inject_spv_start_fault(
    engine: &Engine,
    session: &Arc<NetworkSession>,
) -> Result<(), EngineError> {
    if injected_fault("spv-task-panic") {
        panic_engine_task(engine, session)?;
    }
    Ok(())
}

/// Runs an engine task that panics, then wedges the engine if asked.
#[cfg(debug_assertions)]
fn panic_engine_task(engine: &Engine, session: &Arc<NetworkSession>) -> Result<(), EngineError> {
    let panicked = engine.block_on(session.run_on_engine::<(), _>(async {
        panic!("injected task fault carrying dashpay://invite?pk=PANIC-SECRET")
    }));
    if injected_fault("wedge") {
        wedge_engine(session);
    }
    panicked
}

/// Whether the health probe is to fail on purpose (debug builds' fault
/// injection, `-poison`).
pub(crate) fn probe_fault() -> bool {
    #[cfg(debug_assertions)]
    {
        injected_fault("poison")
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

fn common(cmd: &mut DashPayCommand) -> &mut Common {
    match cmd {
        DashPayCommand::Dashpay { common, .. }
        | DashPayCommand::Identity { common, .. }
        | DashPayCommand::Name { common, .. }
        | DashPayCommand::Contact { common, .. }
        | DashPayCommand::PayContact { common, .. }
        | DashPayCommand::Profile { common, .. }
        | DashPayCommand::Invite { common, .. } => common,
    }
}

/// Runs `cmd`; its `--wallet` overrides `base`'s (a session's).
fn exec(
    base: &Ctx,
    mut cmd: DashPayCommand,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<Value, CliError> {
    let (engine, session) = (base.engine, base.session);
    let common = common(&mut cmd);
    let ctx = Ctx {
        engine,
        session,
        passphrase: base.passphrase,
        wallet: common.wallet.clone().or_else(|| base.wallet.clone()),
    };
    let spv = common.spv;
    if spv {
        // `start_spv` returns while the DashPay bring-up runs.
        crate::pay::spv_running_within(engine, session, SPV_START_TIMEOUT)?
            .map_err(EngineError::Spv)?;
    }
    let result = match cmd {
        DashPayCommand::Dashpay {
            cmd: StatusCmd::Session,
            ..
        } => session_loop(&ctx, stdin, stdout),
        cmd => dispatch(&ctx, cmd, stdin),
    };
    // Requests in a session take no `--spv`: this is a one-shot command or
    // the session itself. An abandoned engine is not stopped.
    if spv
        && crate::teardown::abandoned().is_none()
        && let Err(e) = crate::teardown::stop_spv(session, probe_fault())
    {
        // The result stands: a write that went through must not read as
        // failed (and be retried). Best effort: a panic here would drop the
        // engine on this thread.
        let _ = writeln!(std::io::stderr(), "warning: stopping SPV: {e}");
    }
    result
}

/// How long `--spv` waits for SPV to run (the DashPay bring-up before it
/// takes up to 20 s).
const SPV_START_TIMEOUT: Duration = Duration::from_secs(60);

/// A request's argument list, parsed as a DashPay command.
#[derive(Parser)]
#[command(name = "dwcli", no_binary_name = true)]
struct RequestArgs {
    #[command(subcommand)]
    cmd: DashPayCommand,
}

/// The longest `dashpay session` request line, without its line ending.
const MAX_LINE: usize = 64 * 1024;

fn session_loop(
    ctx: &Ctx,
    stdin: &mut dyn BufRead,
    stdout: &mut dyn Write,
) -> Result<Value, CliError> {
    // One buffer that never reallocates (the limit, plus CR LF), zeroed
    // between lines: it carries bearer inputs.
    let mut line = Zeroizing::new(Vec::with_capacity(MAX_LINE + 2));
    let mut requests = 0u64;
    loop {
        line.zeroize();
        let read = (&mut *stdin)
            .take(MAX_LINE as u64 + 2)
            .read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        let ended = line.last() == Some(&b'\n');
        if !ended && line.len() == MAX_LINE + 2 {
            // The rest of an over-long line. A chunk that already ends in
            // LF has consumed its line: skipping would eat the next one.
            stdin.skip_until(b'\n')?;
        }
        let mut content = &line[..];
        if ended {
            content = &content[..content.len() - 1];
            content = content.strip_suffix(b"\r").unwrap_or(content);
        }
        if content.len() <= MAX_LINE && content.trim_ascii().is_empty() {
            continue;
        }
        requests += 1;
        let (id, result) = session_line(ctx, content);
        let poisoned = result.as_ref().is_err_and(CliError::is_panic)
            && !crate::teardown::healthy(ctx.session, probe_fault());
        if poisoned {
            // `main` exits without the engine's shutdown, which may never
            // finish, and writes this answer once it wiped the passphrase
            // ([`report_abandoned`]).
            *OWED.lock().unwrap_or_else(|p| p.into_inner()) = Some((id, result));
            crate::teardown::abandon("the engine failed its health probe after a panic");
            return Err(CliError::new(
                "session_poisoned",
                "the engine failed its health probe after a panic; the session ends",
                json!({}),
            ));
        }
        answer(stdout, id.as_ref(), &result)?;
    }
    Ok(json!({"requests": requests}))
}

/// Writes and flushes one answer line, with the request's `id` written
/// from its zeroizing buffer.
fn answer(
    stdout: &mut dyn Write,
    id: Option<&request::Id>,
    result: &Result<Value, CliError>,
) -> std::io::Result<()> {
    let answer = envelope(result).to_string();
    match id {
        Some(id) => {
            let open = answer.strip_suffix('}').expect("an answer is an object");
            write!(stdout, r#"{open},"id":"#)?;
            id.write_json(stdout)?;
            writeln!(stdout, "}}")?;
        }
        None => writeln!(stdout, "{answer}")?,
    }
    stdout.flush()
}

/// Runs one request line (without its line ending): its `id`, if it has
/// one, and its result. A panic in the request is caught and answered.
fn session_line(ctx: &Ctx, line: &[u8]) -> (Option<request::Id>, Result<Value, CliError>) {
    if line.len() > MAX_LINE {
        let over = format!("request line over {MAX_LINE} bytes");
        return (None, Err(CliError::invalid(over)));
    }
    let request::Parsed { id, request } = request::parse(line);
    let result = request.map_err(CliError::invalid).and_then(|req| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session_request(ctx, req)))
            .unwrap_or_else(|_| Err(panic_error("dwcli", "the request")))
    });
    (id, result)
}

fn session_request(ctx: &Ctx, req: request::Request) -> Result<Value, CliError> {
    // Nothing reaches clap, which copies it, before the grammar admits it.
    grammar::check(&req.args).map_err(|why| CliError::invalid(format!("bad arguments: {why}")))?;
    let mut cmd = RequestArgs::try_parse_from(req.args.iter().map(|a| a.as_str()))
        .map_err(|e| CliError::invalid(format!("bad arguments: {}", clap_error_text(&e))))?
        .cmd;
    if common(&mut cmd).spv {
        return Err(CliError::invalid(
            "--spv is the session's option, not a request's",
        ));
    }
    if matches!(
        cmd,
        DashPayCommand::Dashpay {
            cmd: StatusCmd::Session,
            ..
        }
    ) {
        return Err(CliError::invalid("a session cannot nest"));
    }
    let input = req.input.as_ref().map_or(&b""[..], |s| s.as_bytes());
    exec(ctx, cmd, &mut &input[..], &mut std::io::sink())
}

fn dispatch(ctx: &Ctx, cmd: DashPayCommand, stdin: &mut dyn Read) -> Result<Value, CliError> {
    #[cfg(debug_assertions)]
    inject_status_faults(ctx, &cmd)?;
    match cmd {
        DashPayCommand::Dashpay { cmd, .. } => status_cmd(ctx, cmd),
        DashPayCommand::Identity { cmd, .. } => identity_cmd(ctx, cmd),
        DashPayCommand::Name { cmd, .. } => name_cmd(ctx, cmd),
        DashPayCommand::Contact { cmd, .. } => contact_cmd(ctx, cmd, stdin),
        DashPayCommand::PayContact { who, .. } => {
            let dp = ctx.dashpay()?;
            let _identity = who.or_main(&dp)?;
            // DP3-01 adds `Recipient::Contact{identity, contact, amount,
            // subtract_fee, note}` to `TxDraft` (m4 §6): this then prepares
            // it under a `Spend` grant and broadcasts. The engine refuses a
            // locked contact itself (`contact.payment_locked`), so dwcli
            // does not check `payment_lock` first.
            Err(PlatformError::NotImplemented {
                call: "Recipient::Contact".into(),
            }
            .into())
        }
        DashPayCommand::Profile { cmd, .. } => profile_cmd(ctx, cmd),
        DashPayCommand::Invite { cmd, .. } => invite_cmd(ctx, cmd, stdin),
    }
}

fn status_cmd(ctx: &Ctx, cmd: StatusCmd) -> Result<Value, CliError> {
    let dp = ctx.dashpay()?;
    match cmd {
        StatusCmd::Session => unreachable!("exec runs the session"),
        StatusCmd::Status => out(dp.status()?),
        StatusCmd::Sync => out(ctx.block_on(dp.sync_now())?),
        StatusCmd::SyncStatus => out(dp.sync_status()?),
        StatusCmd::Leases => {
            let wallet = dp.wallet_id().to_string();
            let leases = ctx.session.leases()?;
            out(leases
                .into_iter()
                .filter(|l| l.wallet_id == wallet)
                .collect::<Vec<_>>())
        }
        StatusCmd::DispatchStatus { artifact } => out(ctx.block_on(dp.dispatch_status(artifact))?),
        StatusCmd::Events { who, cursor, limit } => {
            out(ctx.block_on(dp.events(who.or_main(&dp)?, cursor, limit))?)
        }
        StatusCmd::Unread { who } => out(dp.unread_count(who.or_main(&dp)?)?),
        StatusCmd::MarkRead { who, up_to } => {
            out(ctx.block_on(dp.mark_read(who.or_main(&dp)?, up_to))?)
        }
    }
}

/// The initial profile of a registration, with its avatar prepared from
/// `--avatar-url`.
fn initial_profile(
    ctx: &Ctx,
    dp: &DashPay,
    reg: &RegistrationArgs,
) -> Result<Option<InitialProfile>, CliError> {
    if reg.display_name.is_none() && reg.public_message.is_none() && reg.avatar_url.is_none() {
        return Ok(None);
    }
    let avatar_candidate = match &reg.avatar_url {
        Some(url) => Some(
            ctx.block_on(dp.prepare_avatar(AvatarSource::Url { url: url.clone() }))?
                .id,
        ),
        None => None,
    };
    Ok(Some(InitialProfile {
        display_name: reg.display_name.clone(),
        public_message: reg.public_message.clone(),
        avatar_candidate,
    }))
}

/// Quote, authorize and start a registration: `{"quote", "draft"}`, or
/// `{"quote"}` with `--quote-only`.
fn register(
    ctx: &Ctx,
    dp: &DashPay,
    reg: RegistrationArgs,
    funding: RegistrationFunding,
) -> Result<Value, CliError> {
    let req = RegistrationRequest {
        initial_profile: initial_profile(ctx, dp, &reg)?,
        label: reg.label,
        temporary_label: reg.temporary_label,
        funding,
    };
    let quote = ctx.block_on(dp.registration_quote(req.clone()))?;
    let caps = quote.grant;
    let mut result = json!({"quote": quote});
    if !reg.quote_only {
        let grant = ctx.platform_op(dp, caps)?;
        result["draft"] = ctx.block_on(dp.start_registration(req, grant))?.into();
    }
    Ok(result)
}

fn identity_cmd(ctx: &Ctx, cmd: IdentityCmd) -> Result<Value, CliError> {
    let dp = ctx.dashpay()?;
    match cmd {
        IdentityCmd::List => out(dp.identities()?),
        IdentityCmd::Show { who } => out(ctx.block_on(dp.identity_detail(who.or_main(&dp)?))?),
        IdentityCmd::SetMain { identity } => out(ctx.block_on(dp.set_main_identity(identity))?),
        IdentityCmd::Balance { who } => out(ctx.block_on(dp.refresh_balance(who.or_main(&dp)?))?),
        IdentityCmd::Discover => {
            let grant = ctx.identity_scan(&dp)?;
            out(ctx.block_on(dp.discover_identities(grant))?)
        }
        IdentityCmd::Register {
            reg,
            invitation_id,
            existing_identity,
            faucet_key,
            faucet_proof_file,
        } => {
            let funding = match (invitation_id, existing_identity, faucet_key) {
                (Some(link_id), _, _) => RegistrationFunding::Invitation { link_id },
                (_, Some(identity), _) => RegistrationFunding::ExistingIdentity { identity },
                (_, _, Some(key)) => {
                    let path = faucet_proof_file.expect("clap requires it with --faucet-key");
                    RegistrationFunding::FaucetAssetLock {
                        key,
                        proof: read_text(&path)?,
                    }
                }
                _ => RegistrationFunding::CoreBalance,
            };
            register(ctx, &dp, reg, funding)
        }
        IdentityCmd::Registrations => out(ctx.block_on(dp.registrations())?),
        IdentityCmd::Resume { draft, caps } => {
            let rows = ctx.block_on(dp.registrations())?;
            let row = rows
                .iter()
                .find(|r| r.draft == draft)
                .ok_or_else(|| CliError::invalid(format!("no registration {draft:?}")))?;
            let grant = match row.waiting {
                Some(RegistrationWait::Unlock | RegistrationWait::Authorize) => {
                    Some(ctx.platform_op(&dp, caps.into())?)
                }
                _ => None,
            };
            out(ctx.block_on(dp.resume_registration(draft, grant))?)
        }
        IdentityCmd::Discard { draft } => out(ctx.block_on(dp.discard_registration(draft))?),
        IdentityCmd::FinishAssetLocks { caps } => {
            let grant = ctx.platform_op(&dp, caps.into())?;
            out(ctx.block_on(dp.finish_asset_locks(grant))?)
        }
        IdentityCmd::FaucetKey { caps } => {
            let grant = ctx.platform_op(&dp, caps.into())?;
            out(ctx.block_on(dp.prepare_faucet_lock(grant))?)
        }
        IdentityCmd::TopUp {
            who,
            duffs,
            quote_only,
        } => {
            let identity = who.or_main(&dp)?;
            let quote = ctx.block_on(dp.top_up_quote(identity.clone(), duffs))?;
            if quote_only {
                return Ok(json!({"quote": quote}));
            }
            let grant = ctx.platform_op(&dp, quote.grant)?;
            let outcome = ctx.block_on(dp.top_up(identity, duffs, grant))?;
            Ok(json!({"quote": quote, "outcome": outcome}))
        }
        IdentityCmd::Withdraw {
            who,
            to,
            credits,
            all: _,
            quote_only,
        } => {
            let identity = who.or_main(&dp)?;
            let amount = credits.map_or(WithdrawAmount::All, |credits| WithdrawAmount::Credits {
                credits,
            });
            let quote = ctx.block_on(dp.withdraw_quote(identity.clone(), amount))?;
            if quote_only {
                return Ok(json!({"quote": quote}));
            }
            let to = to.expect("clap requires --to without --quote-only");
            let grant = ctx.platform_op(&dp, quote.grant)?;
            let outcome = ctx.block_on(dp.withdraw(identity, to, amount, grant))?;
            Ok(json!({"quote": quote, "outcome": outcome}))
        }
        IdentityCmd::Costs => out(dp.cost_table()?),
    }
}

/// A file's text, trimmed; for public inputs such as a faucet proof.
fn read_text(path: &Path) -> Result<String, CliError> {
    Ok(std::fs::read_to_string(path)?.trim().to_string())
}

fn name_cmd(ctx: &Ctx, cmd: NameCmd) -> Result<Value, CliError> {
    match cmd {
        NameCmd::Check { label } => out(check_username(&label)?),
        NameCmd::Availability { label } => {
            out(ctx.block_on(ctx.dashpay()?.name_availability(label))?)
        }
        NameCmd::Register { who, label } => {
            let dp = ctx.dashpay()?;
            let identity = who.or_main(&dp)?;
            let action = GrantAction::RegisterName {
                label: label.clone(),
            };
            let (request, grant) = action_grant(ctx, &dp, &identity, action)?;
            granted(
                request,
                ctx.block_on(dp.register_name(identity, label, grant))?,
            )
        }
        NameCmd::Contest { who, label } => {
            let dp = ctx.dashpay()?;
            let identity = who.or_main(&dp)?;
            out(ctx.block_on(dp.contest_status(identity, label))?)
        }
        NameCmd::Search { prefix, limit } => {
            out(ctx.block_on(ctx.dashpay()?.search_users(prefix, limit))?)
        }
        NameCmd::Resolve { username } => out(ctx.block_on(ctx.dashpay()?.resolve_user(username))?),
    }
}

/// `grant_request` for `action`, then a `PlatformOp` grant: the request
/// (printed as the result's `grant`) and the grant id.
fn action_grant(
    ctx: &Ctx,
    dp: &DashPay,
    identity: &str,
    action: GrantAction,
) -> Result<(Value, String), CliError> {
    let request = ctx.block_on(dp.grant_request(identity.to_string(), action))?;
    Ok((json!(request), ctx.platform_op(dp, request)?))
}

/// A write's result: the grant it asked for and the call's outcome.
fn granted<T: Serialize>(grant: Value, outcome: T) -> Result<Value, CliError> {
    Ok(json!({"grant": grant, "outcome": outcome}))
}

fn contact_cmd(ctx: &Ctx, cmd: ContactCmd, stdin: &mut dyn Read) -> Result<Value, CliError> {
    let dp = ctx.dashpay()?;
    match cmd {
        ContactCmd::List {
            who,
            sections,
            sort,
            text,
        } => {
            let identity = who.or_main(&dp)?;
            let q = ContactQuery {
                sections,
                sort,
                text,
            };
            out(dp.contacts(identity, q)?)
        }
        ContactCmd::Show { who, contact } => out(dp.contact(who.or_main(&dp)?, contact)?),
        ContactCmd::PendingSetup => out(dp.pending_setup_count()?),
        ContactCmd::Eligibility { who, contact } => {
            out(ctx.block_on(dp.eligibility(who.or_main(&dp)?, contact))?)
        }
        ContactCmd::Request {
            who,
            contact,
            scanned,
            input,
        } => {
            if !scanned && input.input_file.is_some() {
                return Err(CliError::invalid("--input-file needs --scanned"));
            }
            let identity = who.or_main(&dp)?;
            let (to, scan, scanned) = if scanned {
                let text = read_secret(&input, stdin)?;
                let s = ctx.block_on(dp.verify_scanned(text))?;
                (s.identity.clone(), s.scan.clone(), Some(s))
            } else {
                let to = contact.expect("clap requires a contact without --scanned");
                (to, None, None)
            };
            let (request, grant) = action_grant(ctx, &dp, &identity, GrantAction::SendRequest)?;
            let outcome = ctx.block_on(dp.send_request(identity, to, scan, grant))?;
            Ok(json!({"scanned": scanned, "grant": request, "outcome": outcome}))
        }
        ContactCmd::Scan { input } => {
            let text = read_secret(&input, stdin)?;
            out(ctx.block_on(dp.verify_scanned(text))?)
        }
        ContactCmd::Accept { who, contact } => {
            let identity = who.or_main(&dp)?;
            let (request, grant) = action_grant(ctx, &dp, &identity, GrantAction::AcceptRequest)?;
            granted(
                request,
                ctx.block_on(dp.accept_request(identity, contact, grant))?,
            )
        }
        ContactCmd::Ignore { who, contact } => {
            out(ctx.block_on(dp.ignore(who.or_main(&dp)?, contact))?)
        }
        ContactCmd::Unignore { who, contact } => {
            out(ctx.block_on(dp.unignore(who.or_main(&dp)?, contact))?)
        }
        ContactCmd::Details {
            who,
            contact,
            alias,
            note,
            hidden,
            publish,
        } => {
            let identity = who.or_main(&dp)?;
            let (request, grant) = if publish {
                let action = GrantAction::PublishPrivateDetails;
                let (request, grant) = action_grant(ctx, &dp, &identity, action)?;
                (request, Some(grant))
            } else {
                (Value::Null, None)
            };
            let d = PrivateDetails {
                alias,
                note,
                hidden,
            };
            granted(
                request,
                ctx.block_on(dp.set_private_details(identity, contact, d, grant))?,
            )
        }
        ContactCmd::EnableKeys { who } => {
            let identity = who.or_main(&dp)?;
            let (request, grant) =
                action_grant(ctx, &dp, &identity, GrantAction::EnableDashPayKeys)?;
            granted(
                request,
                ctx.block_on(dp.enable_dashpay_keys(identity, grant))?,
            )
        }
        ContactCmd::Link { who } => out(dp.my_user_link(who.or_main(&dp)?)?),
        ContactCmd::Lock { who, contact } => out(dp.payment_lock(who.or_main(&dp)?, contact)?),
        ContactCmd::ResolveLock { who, contact } => {
            out(ctx.block_on(dp.resolve_payment_lock(who.or_main(&dp)?, contact))?)
        }
        ContactCmd::Activity {
            who,
            contact,
            filter,
            cursor,
        } => out(ctx.block_on(dp.contact_activity(who.or_main(&dp)?, contact, cursor, filter))?),
        ContactCmd::Frequent { who, limit } => out(dp.frequent_contacts(who.or_main(&dp)?, limit)?),
    }
}

fn profile_cmd(ctx: &Ctx, cmd: ProfileCmd) -> Result<Value, CliError> {
    let dp = ctx.dashpay()?;
    match cmd {
        ProfileCmd::Show { who } => out(dp.profile(who.or_main(&dp)?)?),
        ProfileCmd::Limits => out(dp.profile_limits()?),
        ProfileCmd::UploadAvailable => out(dp.avatar_upload_available()?),
        ProfileCmd::Set {
            who,
            display_name,
            public_message,
            avatar_url,
            avatar_file,
            remove_avatar,
        } => {
            let identity = who.or_main(&dp)?;
            let source = match (avatar_url, avatar_file) {
                (Some(url), _) => Some(AvatarSource::Url { url }),
                (_, Some(path)) => Some(AvatarSource::File {
                    bytes: std::fs::read(&path)?,
                    crop: None,
                }),
                _ => None,
            };
            let avatar = match source {
                Some(src) => {
                    let candidate = ctx.block_on(dp.prepare_avatar(src))?;
                    if candidate.needs_upload {
                        ctx.block_on(dp.upload_avatar(candidate.id.clone()))?;
                    }
                    AvatarChange::Set {
                        candidate: candidate.id,
                    }
                }
                None if remove_avatar => AvatarChange::Remove,
                None => AvatarChange::Keep,
            };
            let edit = ProfileEdit {
                display_name,
                public_message,
                avatar,
            };
            let (request, grant) = action_grant(ctx, &dp, &identity, GrantAction::UpdateProfile)?;
            granted(
                request,
                ctx.block_on(dp.update_profile(identity, edit, grant))?,
            )
        }
        ProfileCmd::Avatar { identity, size } => out(ctx.block_on(dp.avatar(identity, size))?),
    }
}

fn invite_cmd(ctx: &Ctx, cmd: InviteCmd, stdin: &mut dyn Read) -> Result<Value, CliError> {
    let session = ctx.session;
    match cmd {
        InviteCmd::Claim { reg, input } => {
            let dp = ctx.dashpay()?;
            let link = read_secret(&input, stdin)?;
            let link_id = ctx.block_on(session.stash_invitation(link))?;
            let funding = RegistrationFunding::Invitation {
                link_id: link_id.clone(),
            };
            // The link stays stashed whatever happens next, so its id is in
            // the error too.
            let mut result = register(ctx, &dp, reg, funding);
            match &mut result {
                Ok(v) => v["link_id"] = link_id.into(),
                Err(e) => e.params["link_id"] = link_id.into(),
            }
            result
        }
        InviteCmd::Stash { input } => {
            let link = read_secret(&input, stdin)?;
            out(ctx.block_on(session.stash_invitation(link))?)
        }
        InviteCmd::Status { link_id } => out(ctx.block_on(session.invitation_status(link_id))?),
        InviteCmd::Pending => out(ctx.block_on(session.pending_invitations())?),
        InviteCmd::Forget { link_id } => out(ctx.block_on(session.forget_invitation(link_id))?),
    }
}

#[cfg(test)]
#[path = "dashpay_heap_tests.rs"]
mod heap_tests;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use clap::{CommandFactory, Parser};
    use dw_engine::platform::{DashPayStatus, Eligibility, UsernameRule};
    use dw_engine::{DashNetwork, EngineConfig, EngineEvent, EventSink, SessionOptions};
    use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

    use super::*;
    use crate::{Cli, Command};

    const GROUPS: &[&str] = &[
        "dashpay",
        "identity",
        "name",
        "contact",
        "pay-contact",
        "profile",
        "invite",
    ];

    /// The rows whose calls have real bodies (DP1-05, DP1-03, E0-04), with the
    /// line each prints on the tests' wallet, which has no identity and no
    /// Platform.
    const IMPLEMENTED: &[(&str, &str)] = &[
        ("dashpay events", NO_IDENTITY),
        ("identity list", r#"{"ok":true,"result":[]}"#),
        ("identity show", NO_IDENTITY),
        ("identity set-main I", NO_IDENTITY),
        ("identity discover", NO_PLATFORM),
        ("pay-contact C --amount 1", NO_IDENTITY),
        ("name check alice", ALICE),
        ("name availability alice", NO_PLATFORM),
        ("name register --identity I bob", NO_PLATFORM),
        ("dashpay leases", r#"{"ok":true,"result":[]}"#),
        (
            "dashpay dispatch-status ab12",
            r#"{"ok":false,"error":{"code":"invalid_argument","message":"invalid argument: not a txid, transition hash or step id","params":{"detail":"not a txid, transition hash or step id"}}}"#,
        ),
    ];

    const NO_IDENTITY: &str = r#"{"ok":false,"error":{"code":"identity.not_found","message":"identity: identity not found","params":{}}}"#;

    const NO_PLATFORM: &str = r#"{"ok":false,"error":{"code":"platform.unavailable","message":"platform unavailable","params":{}}}"#;

    /// `name check alice`: valid, normalized and contested (DPNS rules).
    const ALICE: &str = r#"{"ok":true,"result":{"valid":true,"normalized":"a11ce","contested":true,"rules":[{"rule":"min_length","passed":true},{"rule":"max_length","passed":true},{"rule":"allowed_characters","passed":true},{"rule":"no_edge_hyphen","passed":true},{"rule":"no_double_hyphen","passed":true}]}}"#;

    /// What tests feed a command that reads a bearer input.
    const SECRET: &[u8] = b"  dash:?du=alice&dapk=SECRET-MATERIAL\n";

    /// Every DashPay command but `dashpay session` with the facade call it
    /// reaches first while the bodies are stubs. A write asks for its grant
    /// first, so its first call is its quote or `grant_request`. A DP task
    /// that fills in a body moves its rows on to the next stub, or gives
    /// the row its line in [`IMPLEMENTED`].
    const TABLE: &[(&str, &str)] = &[
        ("dashpay status", "DashPay.status"),
        ("dashpay sync", "DashPay.sync_now"),
        ("dashpay sync-status", "DashPay.sync_status"),
        ("dashpay leases", "NetworkSession.leases"),
        ("dashpay dispatch-status ab12", "DashPay.dispatch_status"),
        ("dashpay events --identity I --cursor 3", "DashPay.events"),
        ("dashpay events", "DashPay.identities"),
        ("dashpay unread --identity I", "DashPay.unread_count"),
        ("dashpay mark-read --identity I 7", "DashPay.mark_read"),
        ("identity list", "DashPay.identities"),
        ("identity show --identity I", "DashPay.identity_detail"),
        ("identity show", "DashPay.identities"),
        ("identity set-main I", "DashPay.set_main_identity"),
        ("identity balance --identity I", "DashPay.refresh_balance"),
        ("identity discover", "DashPay.discover_identities"),
        (
            "identity register --label alice",
            "DashPay.registration_quote",
        ),
        (
            "identity register --label alice --avatar-url https://x/a.png",
            "DashPay.prepare_avatar",
        ),
        (
            "identity register --label alice --invitation-id L --quote-only",
            "DashPay.registration_quote",
        ),
        (
            "identity register --label alice --existing-identity I",
            "DashPay.registration_quote",
        ),
        ("identity registrations", "DashPay.registrations"),
        (
            "identity resume D --max-duffs 1 --max-credits 2",
            "DashPay.registrations",
        ),
        ("identity discard D", "DashPay.discard_registration"),
        (
            "identity finish-asset-locks --max-credits 50000",
            "DashPay.finish_asset_locks",
        ),
        ("identity faucet-key", "DashPay.prepare_faucet_lock"),
        (
            "identity top-up --identity I --duffs 100000",
            "DashPay.top_up_quote",
        ),
        (
            "identity withdraw --identity I --credits 5 --to yA",
            "DashPay.withdraw_quote",
        ),
        (
            "identity withdraw --identity I --all --quote-only",
            "DashPay.withdraw_quote",
        ),
        ("identity costs", "DashPay.cost_table"),
        ("name check alice", "check_username"),
        ("name availability alice", "DashPay.name_availability"),
        ("name register --identity I bob", "DashPay.grant_request"),
        ("name contest --identity I alice", "DashPay.contest_status"),
        ("name search al --limit 5", "DashPay.search_users"),
        ("name resolve alice.dash", "DashPay.resolve_user"),
        (
            "contact list --identity I --section requests --sort username",
            "DashPay.contacts",
        ),
        ("contact show --identity I C", "DashPay.contact"),
        ("contact pending-setup", "DashPay.pending_setup_count"),
        ("contact eligibility --identity I C", "DashPay.eligibility"),
        ("contact request --identity I C", "DashPay.send_request"),
        (
            "contact request --identity I --scanned",
            "DashPay.verify_scanned",
        ),
        ("contact scan", "DashPay.verify_scanned"),
        ("contact accept --identity I C", "DashPay.accept_request"),
        ("contact ignore --identity I C", "DashPay.ignore"),
        ("contact unignore --identity I C", "DashPay.unignore"),
        (
            "contact details --identity I C --alias Al --hidden",
            "DashPay.set_private_details",
        ),
        (
            "contact details --identity I C --publish",
            "DashPay.set_private_details",
        ),
        (
            "contact enable-keys --identity I",
            "DashPay.enable_dashpay_keys",
        ),
        ("contact link --identity I", "DashPay.my_user_link"),
        ("contact lock --identity I C", "DashPay.payment_lock"),
        (
            "contact resolve-lock --identity I C",
            "DashPay.resolve_payment_lock",
        ),
        (
            "contact activity --identity I C --filter sent",
            "DashPay.contact_activity",
        ),
        ("contact frequent --identity I", "DashPay.frequent_contacts"),
        (
            "pay-contact --identity I C --amount 1000 --note hi",
            "Recipient::Contact",
        ),
        ("pay-contact C --amount 1", "DashPay.identities"),
        ("profile show --identity I", "DashPay.profile"),
        ("profile limits", "DashPay.profile_limits"),
        (
            "profile upload-available",
            "DashPay.avatar_upload_available",
        ),
        (
            "profile set --identity I --display-name Al",
            "DashPay.update_profile",
        ),
        (
            "profile set --identity I --avatar-url https://x/a.png",
            "DashPay.prepare_avatar",
        ),
        ("profile avatar I --size large", "DashPay.avatar"),
        (
            "invite claim --label alice",
            "NetworkSession.stash_invitation",
        ),
        ("invite stash", "NetworkSession.stash_invitation"),
        ("invite status L", "NetworkSession.invitation_status"),
        ("invite pending", "NetworkSession.pending_invitations"),
        ("invite forget L", "NetworkSession.forget_invitation"),
    ];

    /// Parses a command line (no argument holds a space).
    fn parse(line: &str) -> Result<DashPayCommand, clap::Error> {
        let argv = ["dwcli", "--datadir", "/nonexistent"]
            .into_iter()
            .chain(line.split_whitespace());
        match Cli::try_parse_from(argv)?.command {
            Command::DashPay(cmd) => Ok(cmd),
            _ => panic!("{line:?} is not a DashPay command"),
        }
    }

    /// The command a table row names: `group sub`, or the group alone.
    fn path(line: &str) -> String {
        match line.split_whitespace().collect::<Vec<_>>()[..] {
            ["pay-contact", ..] => "pay-contact".into(),
            [group, sub, ..] => format!("{group} {sub}"),
            ref other => panic!("bad row {other:?}"),
        }
    }

    /// Parses and runs a command line, with [`SECRET`] on stdin.
    fn run(engine: &Engine, session: &Arc<NetworkSession>, line: &str) -> Result<Value, CliError> {
        let cmd = parse(line).unwrap_or_else(|e| panic!("{line:?}: {e}"));
        let base = Ctx {
            engine,
            session,
            passphrase: None,
            wallet: None,
        };
        exec(&base, cmd, &mut &SECRET[..], &mut std::io::sink())
    }

    #[test]
    fn the_table_covers_every_dashpay_command() {
        let cli = Cli::command();
        let mut want = BTreeSet::new();
        for group in GROUPS {
            let g = cli.find_subcommand(group).expect(group);
            let subs: Vec<_> = g
                .get_subcommands()
                .map(|s| s.get_name())
                .filter(|n| *n != "help")
                .collect();
            if subs.is_empty() {
                want.insert(group.to_string());
            }
            want.extend(subs.into_iter().map(|s| format!("{group} {s}")));
        }
        // Tested on its own below.
        assert!(want.remove("dashpay session"));
        let have: BTreeSet<_> = TABLE.iter().map(|(args, _)| path(args)).collect();
        assert_eq!(have, want);
    }

    /// Identity ids for the table's `I` and `C`.
    const ID1: &str = "29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV2";
    const ID2: &str = "3JF3sEqM796hk5WFqA6EtmEwJQ9quALszsfJyvXNQKy3";

    /// Every table row, with real ids for its placeholders, is admitted by
    /// a session's grammar and parsed by clap.
    #[test]
    fn every_command_passes_the_session_grammar() {
        let hex = "ab".repeat(32);
        for (line, _) in TABLE {
            let args: Vec<Zeroizing<String>> = line
                .split_whitespace()
                .map(|t| match t {
                    "I" => ID1,
                    "C" => ID2,
                    "L" | "D" => "d1",
                    "yA" => "yTw4tvqFpDXagzVgq6v2WShbBBUYoMnCXp",
                    "ab12" => &hex,
                    t => t,
                })
                .map(|t| Zeroizing::new(t.to_string()))
                .collect();
            assert_eq!(grammar::check(&args), Ok(()), "{line:?}");
            let parsed = RequestArgs::try_parse_from(args.iter().map(|a| a.as_str()));
            assert!(parsed.is_ok(), "{line:?}");
        }
    }

    #[test]
    fn global_options_parse_on_either_side_of_the_subcommand() {
        let w = "00".repeat(32);
        for args in [
            format!("identity --wallet {w} --spv list"),
            format!("identity list --wallet {w} --spv"),
        ] {
            let Ok(DashPayCommand::Identity { common, .. }) = parse(&args) else {
                panic!("{args:?}");
            };
            assert_eq!(common.wallet.as_deref(), Some(w.as_str()));
            assert!(common.spv);
        }
    }

    #[test]
    fn bad_arguments_are_refused_by_the_parser() {
        for args in [
            // Bearer inputs never come from argv.
            "invite claim --label a dash:?invite",
            "invite stash dash:?invite",
            "contact scan dash:?du=a&dapk=b",
            "identity register",
            "identity register --label a --invitation-id L --existing-identity I",
            "identity register --label a --faucet-key K",
            "identity register --label a --faucet-proof-file /p",
            "identity top-up",
            "identity withdraw --to yA",
            "identity withdraw --credits 5 --all --to yA",
            "identity withdraw --credits 5",
            "identity set-main",
            "name check",
            "contact request",
            "contact request C --scanned",
            "dashpay session extra",
            "contact list --sort bogus",
            "contact list --section everyone",
            "contact activity C --filter both",
            "profile set --avatar-url u --remove-avatar",
            "profile avatar I --size huge",
            "pay-contact C",
            "pay-contact C --amount -1",
            "dashpay mark-read",
        ] {
            assert!(parse(args).is_err(), "{args:?} parsed");
        }
    }

    #[test]
    fn enum_options_take_the_json_names() {
        let Ok(DashPayCommand::Contact {
            cmd: ContactCmd::List { sections, sort, .. },
            ..
        }) = parse("contact list --section hidden --section pending --sort last_activity")
        else {
            panic!();
        };
        assert_eq!(sections, [ContactSection::Hidden, ContactSection::Pending]);
        assert_eq!(sort, ContactSort::LastActivity);
    }

    struct NullSink;

    impl EventSink for NullSink {
        fn emit(&self, _event: EngineEvent) {}
    }

    fn engine(root: &Path) -> Engine {
        Engine::new(
            EngineConfig {
                data_root: root.to_path_buf(),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(NullSink),
        )
        .unwrap()
    }

    /// An open regtest session with no DAPI behind it.
    fn open(engine: &Engine) -> Arc<NetworkSession> {
        let opts = SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            ..Default::default()
        };
        engine
            .block_on(engine.open_network(DashNetwork::Regtest, opts))
            .unwrap()
    }

    /// [`open`] with an unencrypted vault (its key in memory) and one
    /// wallet.
    fn session(root: &Path) -> (Engine, Arc<NetworkSession>) {
        let engine = engine(root);
        let session = open(&engine);
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        engine.block_on(session.create_wallet(12)).unwrap();
        (engine, session)
    }

    /// Output that fails: every write once `fail_after` lines are written,
    /// or (with `fail_flush`) the flush after that many lines.
    struct FailWriter {
        lines: usize,
        fail_after: usize,
        fail_flush: bool,
        written: Vec<u8>,
    }

    impl Write for FailWriter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if !self.fail_flush && self.lines >= self.fail_after {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.lines += b.iter().filter(|c| **c == b'\n').count();
            self.written.extend_from_slice(b);
            Ok(b.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if self.fail_flush && self.lines > self.fail_after {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            Ok(())
        }
    }

    /// Sol r3 finding 1: a failed probe ends the session poisoned whatever
    /// its output does. Each envelope's write and each flush fails in turn,
    /// and the result still tells `main` to skip the engine's shutdown.
    #[test]
    fn a_failed_probe_survives_output_errors() {
        let _exclusive = crate::teardown::exclusive();
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(dir.path());
        let ctx = Ctx {
            engine: &engine,
            session: &session,
            passphrase: None,
            wallet: None,
        };
        let requests = concat!(
            r#"{"args":["dashpay","status"],"id":1}"#,
            "\n",
            r#"{"args":["name","check","alice"],"id":2}"#,
            "\n"
        );
        TEST_FAULT.set(Some("task-panic-poison"));
        for fail_flush in [false, true] {
            for fail_after in [0, 1] {
                let mut output = FailWriter {
                    lines: 0,
                    fail_after,
                    fail_flush,
                    written: Vec::new(),
                };
                let result = session_loop(&ctx, &mut requests.as_bytes(), &mut output);
                write_abandoned(&mut output, &result);
                let case = format!("fail_after {fail_after}, fail_flush {fail_flush}");
                assert_eq!(
                    result.as_ref().unwrap_err().code,
                    "session_poisoned",
                    "{case}"
                );
                assert!(crate::teardown::abandoned().is_some(), "{case}");
                // Both envelopes were attempted, and request 2 never ran.
                let written = String::from_utf8(output.written).unwrap();
                let expected = match (fail_flush, fail_after) {
                    (false, 0) => 0,
                    (false, _) => 1,
                    (true, _) => 2,
                };
                assert_eq!(written.lines().count(), expected, "{case}: {written}");
                if expected == 2 {
                    assert!(written.contains(r#""id":1}"#), "{case}: {written}");
                    assert!(written.contains("session_poisoned"), "{case}: {written}");
                }
                assert!(!written.contains(r#""id":2"#), "{case}: {written}");
            }
        }
        TEST_FAULT.set(None);
    }

    /// Runs `line` with `passphrase`, then the teardown `main` runs after
    /// it, and checks that the engine is abandoned after its health probe
    /// failed, within `bound`. Returns the command's result.
    fn assert_abandoned(
        engine: Engine,
        session: Arc<NetworkSession>,
        passphrase: Option<&Zeroizing<Vec<u8>>>,
        line: &str,
        bound: Duration,
    ) -> Result<Value, CliError> {
        let _exclusive = crate::teardown::exclusive();
        let start = std::time::Instant::now();
        let result = super::run(&engine, &session, passphrase, parse(line).unwrap());
        let teardown = crate::teardown::shut_down(engine, &session, false);
        let elapsed = start.elapsed();
        let why = teardown.expect_err("the engine was shut down");
        assert!(why.contains("health probe"), "{why}");
        assert!(elapsed < bound, "{elapsed:?}");
        result
    }

    /// A sink that, once armed, wedges the engine and panics on the next
    /// event `when` picks, in the engine task that emits it (Sol r4's and
    /// r5's sketch).
    struct WedgeOn {
        when: fn(&EngineEvent) -> bool,
        armed: AtomicBool,
        session: std::sync::Mutex<Option<Arc<NetworkSession>>>,
    }

    impl EventSink for WedgeOn {
        fn emit(&self, event: EngineEvent) {
            if (self.when)(&event) && self.armed.swap(false, Ordering::SeqCst) {
                let session = self.session.lock().unwrap().clone().unwrap();
                wedge_engine(&session);
                panic!("injected event callback panic");
            }
        }
    }

    /// An open regtest session whose sink is a [`WedgeOn`], not armed yet.
    fn wedge_on(
        root: &Path,
        kdf: KdfParams,
        when: fn(&EngineEvent) -> bool,
    ) -> (Engine, Arc<NetworkSession>, Arc<WedgeOn>) {
        let sink = Arc::new(WedgeOn {
            when,
            armed: AtomicBool::new(false),
            session: std::sync::Mutex::new(None),
        });
        let engine = Engine::new(
            EngineConfig {
                data_root: root.to_path_buf(),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(kdf),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::clone(&sink) as Arc<dyn EventSink>,
        )
        .unwrap();
        let session = open(&engine);
        *sink.session.lock().unwrap() = Some(Arc::clone(&session));
        (engine, session, sink)
    }

    /// Sol r4 finding 1: a task panic in the unlock before the command is
    /// probed like the command's own, and a failed probe abandons the
    /// engine instead of shutting it down.
    #[test]
    fn an_unlock_task_panic_abandons_a_failed_engine() {
        let dir = dw_testutil::private_tempdir();
        // An encrypted vault: dwcli builds no test KDF.
        let (engine, session, sink) = wedge_on(dir.path(), KdfParams::FLOOR, |e| {
            matches!(e, EngineEvent::VaultLockState { .. })
        });
        let passphrase = Zeroizing::new(b"r4 unlock passphrase".to_vec());
        let create = passphrase.clone();
        engine
            .block_on(session.vault_op(move |v| v.create(Some(&create[..]))))
            .unwrap();
        session.lock_vault_sync().unwrap();
        sink.armed.store(true, Ordering::SeqCst);
        let result = assert_abandoned(
            engine,
            session,
            Some(&passphrase),
            "dashpay status",
            crate::teardown::PROBE_TIMEOUT * 3,
        );
        assert!(result.is_err_and(|e| e.is_panic()));
    }

    /// Sol r5 finding 1: the task that actually starts SPV panics (its
    /// `start_spv` call has returned), so `--spv` waits out its start
    /// timeout for a state that never comes. The panic marked the engine
    /// poisoned: it fails its probe before SPV is stopped or the engine is
    /// shut down.
    #[test]
    fn a_panic_in_the_spv_start_task_abandons_a_failed_engine() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session, sink) = wedge_on(dir.path(), KdfParams::FLOOR, |e| {
            matches!(e, EngineEvent::SpvStateChanged { running: true, .. })
        });
        sink.armed.store(true, Ordering::SeqCst);
        let bound = SPV_START_TIMEOUT + crate::teardown::PROBE_TIMEOUT * 2;
        let result = assert_abandoned(engine, session, None, "dashpay status --spv", bound);
        assert!(
            !sink.armed.load(Ordering::SeqCst),
            "the start task never ran"
        );
        assert_eq!(result.unwrap_err().code, "spv");
    }

    /// Sol r5 finding 2: the SPV stop's task panics after an ordinary
    /// command error. The command's result stands, and the engine fails its
    /// probe before its shutdown.
    #[test]
    fn a_panic_in_the_spv_stop_task_abandons_a_failed_engine() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session, sink) = wedge_on(dir.path(), KdfParams::FLOOR, |e| {
            matches!(e, EngineEvent::SpvStateChanged { running: false, .. })
        });
        crate::pay::ensure_spv_running(&engine, &session, Duration::from_secs(60)).unwrap();
        sink.armed.store(true, Ordering::SeqCst);
        let result = assert_abandoned(
            engine,
            session,
            None,
            "dashpay status --spv",
            crate::teardown::PROBE_TIMEOUT * 3,
        );
        assert!(
            !sink.armed.load(Ordering::SeqCst),
            "the stop task never ran"
        );
        assert_eq!(result.unwrap_err().code, "wallet_not_found");
    }

    /// DEC-110's deadline: on an engine that stalls with no panic at all,
    /// the SPV stop and the shutdown end within their bounds and the engine
    /// is abandoned. In its own process (`dashpay_r5`) no panic marks the
    /// engine poisoned and the deadline ends it; here another test's panic
    /// may have, and the probe may end it first.
    #[test]
    fn a_stalled_engine_is_abandoned_within_the_deadline() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(dir.path());
        let _exclusive = crate::teardown::exclusive();
        wedge_engine(&session);
        let start = std::time::Instant::now();
        let stop = crate::teardown::stop_spv(&session, false);
        let teardown = crate::teardown::shut_down(engine, &session, false);
        let elapsed = start.elapsed();
        assert!(stop.is_err(), "{stop:?}");
        let why = teardown.expect_err("the engine was shut down");
        assert!(
            why.contains("deadline") || why.contains("health probe"),
            "{why}"
        );
        let bound = crate::teardown::DEFAULT_DEADLINE + crate::teardown::PROBE_TIMEOUT;
        assert!(elapsed < bound + Duration::from_secs(5), "{elapsed:?}");
    }

    /// Sol r4 finding 1, `--spv`: a task panic while SPV starts keeps its
    /// classification across the start helper, so it is probed too.
    #[test]
    fn an_spv_start_task_panic_abandons_a_failed_engine() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(dir.path());
        TEST_FAULT.set(Some("spv-task-panic-wedge"));
        let result = assert_abandoned(
            engine,
            session,
            None,
            "dashpay status --spv",
            crate::teardown::PROBE_TIMEOUT * 3,
        );
        TEST_FAULT.set(None);
        assert!(result.is_err_and(|e| e.is_panic()));
    }

    #[test]
    fn every_command_reports_its_stub_as_a_json_error() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        for (row, _) in IMPLEMENTED {
            assert!(TABLE.iter().any(|(args, _)| args == row), "{row:?}");
        }
        for (args, call) in TABLE {
            let line = envelope(&run(&engine, &session, args));
            assert!(!line.to_string().contains("SECRET"), "{args:?}");
            if let Some((_, want)) = IMPLEMENTED.iter().find(|(row, _)| row == args) {
                let want: Value = serde_json::from_str(want).unwrap();
                assert_eq!(line, want, "{args:?}");
                continue;
            }
            assert_eq!(
                line,
                json!({
                    "ok": false,
                    "error": {
                        "code": "platform.not_implemented",
                        "message": format!("not implemented: {call}"),
                        "params": {"call": call},
                    },
                }),
                "{args:?}"
            );
        }
        engine.block_on(engine.shutdown()).unwrap();
    }

    #[test]
    fn the_wallet_option_is_checked() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        let w = "ab".repeat(32);
        let err = run(&engine, &session, "dashpay status --wallet nothex").unwrap_err();
        assert_eq!(err.code, "invalid_argument");
        // An unknown id still gets a handle; the call checks the wallet once
        // implemented (m4 §0), and the stub answers first.
        let err = run(&engine, &session, &format!("dashpay status --wallet {w}")).unwrap_err();
        assert_eq!(err.code, "platform.not_implemented");
        engine.block_on(engine.shutdown()).unwrap();
    }

    #[test]
    fn no_wallet_is_the_engine_code() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(&dir.path().join("data"));
        let session = open(&engine);
        let err = run(&engine, &session, "identity list").unwrap_err();
        assert_eq!(err.code, "wallet_not_found");
        // `name check` and the invitation calls need no wallet.
        let alice: Value = serde_json::from_str(ALICE).unwrap();
        assert_eq!(
            run(&engine, &session, "name check alice").unwrap(),
            alice["result"]
        );
        let err = run(&engine, &session, &crate::test_stub::STUB_ARGS.join(" ")).unwrap_err();
        assert_eq!(err.params, json!({"call": crate::test_stub::STUB_CALL}));
        engine.block_on(engine.shutdown()).unwrap();
    }

    #[test]
    fn results_are_the_records_serde_form() {
        let status = DashPayStatus::Ready { main: "Id1".into() };
        assert_eq!(
            envelope(&out(status)),
            json!({"ok": true, "result": {"kind": "ready", "main": "Id1"}})
        );
        assert_eq!(envelope(&out(())), json!({"ok": true, "result": null}));
        assert_eq!(
            envelope(&out(Option::<u64>::None)),
            json!({"ok": true, "result": null})
        );
    }

    fn error_json(e: impl Into<CliError>) -> Value {
        envelope(&Err(e.into()))["error"].clone()
    }

    #[test]
    fn errors_carry_their_code_and_parameters() {
        assert_eq!(
            error_json(PlatformError::InsufficientCredits {
                needed: 5,
                available: 2
            }),
            json!({
                "code": "platform.insufficient_credits",
                "message": "insufficient credits: 5 needed, 2 available",
                "params": {"needed": 5, "available": 2},
            })
        );
        assert_eq!(
            error_json(PlatformError::GrantExceeded {
                purpose: dw_engine::platform::BudgetPurpose::Credits,
                needed: 9,
                remaining: 1,
            })["params"],
            json!({"purpose": "credits", "needed": 9, "remaining": 1})
        );
        assert_eq!(
            error_json(PlatformError::Identity(IdentityError::KeysMissing {
                purpose: dw_engine::platform::KeyPurpose::Encryption,
            })),
            json!({
                "code": "identity.keys_missing",
                "message": "identity: identity has no Encryption key",
                "params": {"purpose": "encryption"},
            })
        );
        let e = error_json(ContactError::Ineligible {
            reason: Eligibility::NoDashPayKeys,
        });
        assert_eq!(e["code"], "contact.ineligible");
        assert_eq!(e["params"], json!({"reason": "no_dash_pay_keys"}));
        // A wrapped domain reports the inner code and parameters.
        let e = error_json(RegistrationError::from(NameError::Invalid {
            rules: vec![UsernameRule::MinLength, UsernameRule::NoEdgeHyphen],
        }));
        assert_eq!(e["code"], "name.invalid");
        assert_eq!(
            e["params"],
            json!({"rules": ["min_length", "no_edge_hyphen"]})
        );
        let e = error_json(CreditsError::Platform(PlatformError::WillBeSent {
            artifact: "topup/1/funding".into(),
        }));
        assert_eq!(e["code"], "platform.will_be_sent");
        assert_eq!(e["params"], json!({"artifact": "topup/1/funding"}));
        let e = error_json(CreditsError::BelowMinimum { min: 100 });
        assert_eq!(
            (e["code"].clone(), e["params"].clone()),
            (json!("credits.below_minimum"), json!({"min": 100}))
        );
        // Vault and engine errors keep their own m1 codes.
        assert_eq!(
            error_json(EngineError::Vault(VaultError::NoVault))["code"],
            "vault.no_vault"
        );
        assert_eq!(
            error_json(EngineError::WalletNotFound("x".into()))["code"],
            "wallet_not_found"
        );
    }

    #[test]
    fn bearer_inputs_are_trimmed_and_never_quoted() {
        let none = SecretInput { input_file: None };
        let s = read_secret(&none, &mut &SECRET[..]).unwrap();
        assert_eq!(s.expose(), "dash:?du=alice&dapk=SECRET-MATERIAL");
        let err = read_secret(&none, &mut &b" \n\t"[..]).unwrap_err();
        assert_eq!(err.code, "invalid_argument");
        let err = read_secret(&none, &mut &b"\xffSECRET"[..]).unwrap_err();
        assert_eq!(err.code, "invalid_argument");
        assert!(!err.message.contains("SECRET"), "{}", err.message);

        let dir = dw_testutil::private_tempdir();
        let file = dir.path().join("link");
        std::fs::write(&file, SECRET).unwrap();
        let from_file = SecretInput {
            input_file: Some(file),
        };
        let s = read_secret(&from_file, &mut &b"ignored"[..]).unwrap();
        assert_eq!(s.expose(), "dash:?du=alice&dapk=SECRET-MATERIAL");
        let missing = SecretInput {
            input_file: Some(dir.path().join("missing")),
        };
        assert_eq!(read_secret(&missing, &mut &[][..]).unwrap_err().code, "io");
    }

    #[test]
    fn a_session_runs_one_command_per_line() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        let mut requests = Vec::new();
        for line in [
            &br#"{"args":["dashpay","status"],"id":1}"#[..],
            b"",
            br#"{"args":["invite","stash"],"input":"dash:?invite=SECRET-MATERIAL","id":"b"}"#,
            br#"{"args":["contact","request","C"],"input":"x"} "dash:?SECRET-MATERIAL""#,
            br#"{"args":["invite","stash","dash:?invite=SECRET-MATERIAL"],"id":3}"#,
            br#"{"argz":["dashpay","status"],"input":"SECRET-MATERIAL","id":4}"#,
            b"\xff\xfeSECRET-MATERIAL",
            &[b'S'; MAX_LINE + 10],
            br#"{"args":["dashpay","session"]}"#,
            br#"{"args":["identity","list","--spv"]}"#,
            br#"{"args":["teleport"]}"#,
            br#"{"args":["dashpay","sync"],"id":5}"#,
        ] {
            requests.extend_from_slice(line);
            requests.push(b'\n');
        }
        let base = Ctx {
            engine: &engine,
            session: &session,
            passphrase: None,
            wallet: None,
        };
        let cmd = parse("dashpay session").unwrap();
        let mut answers = Vec::new();
        let done = exec(&base, cmd, &mut &requests[..], &mut answers).unwrap();
        assert_eq!(done, json!({"requests": 11}));
        let answers = String::from_utf8(answers).unwrap();
        assert!(!answers.contains("SECRET"), "{answers}");
        let lines: Vec<Value> = answers
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 11);
        let call = |i: usize| lines[i]["error"]["params"]["call"].clone();
        assert_eq!(
            (lines[0]["id"].clone(), call(0)),
            (json!(1), json!("DashPay.status"))
        );
        assert_eq!(
            (lines[1]["id"].clone(), call(1)),
            (json!("b"), json!("NetworkSession.stash_invitation"))
        );
        // Every refusal is answered in turn and the session goes on.
        let ids = [None, Some(3), Some(4), None, None, None, None, None];
        for (i, id) in (2..10).zip(ids) {
            assert_eq!(
                lines[i]["error"]["code"], "invalid_argument",
                "{}",
                lines[i]
            );
            assert_eq!(
                lines[i].get("id"),
                id.map(Value::from).as_ref(),
                "{}",
                lines[i]
            );
        }
        assert_eq!(
            (lines[10]["id"].clone(), call(10)),
            (json!(5), json!("DashPay.sync_now"))
        );
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// Review DW-E0-09 r2 finding 4: the probe fails on an error, not only
    /// on a panic. A closed session answers `network_not_open`.
    #[test]
    fn the_health_probe_fails_closed() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        let ctx = Ctx {
            engine: &engine,
            session: &session,
            passphrase: None,
            wallet: None,
        };
        assert!(crate::teardown::healthy(ctx.session, false));
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        assert!(session.wallet_infos().is_err());
        assert!(!crate::teardown::healthy(ctx.session, false));
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// Review DW-E0-09 r2 finding 3: an engine task's panic is a panic
    /// error (outcome unknown, health probed) that quotes nothing, through
    /// the engine's error and through a facade error's `detail`.
    #[test]
    fn engine_task_panics_are_panics() {
        let dir = dw_testutil::private_tempdir();
        let engine = engine(&dir.path().join("data"));
        let session = open(&engine);
        let e: CliError = engine
            .block_on(session.run_on_engine::<(), _>(async {
                panic!("task panic carrying dashpay://invite?pk=PANIC-SECRET")
            }))
            .unwrap_err()
            .into();
        assert!(e.is_panic());
        let line = envelope(&Err(e));
        assert_eq!(line["error"]["code"], "internal");
        assert_eq!(
            line["error"]["params"]["detail"],
            "an engine task panicked; the call's outcome is unknown"
        );
        assert!(!line.to_string().contains("PANIC-SECRET"), "{line}");
        let facade: CliError = PlatformError::Internal {
            detail: TASK_PANICKED.into(),
        }
        .into();
        assert!(facade.is_panic());
        let other: CliError = PlatformError::Internal { detail: "x".into() }.into();
        assert!(!other.is_panic());
        engine.block_on(engine.shutdown()).unwrap();
    }

    #[test]
    fn grants_come_from_the_vault() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        let ctx = Ctx {
            engine: &engine,
            session: &session,
            passphrase: None,
            wallet: None,
        };
        let dp = ctx.dashpay().unwrap();
        let caps = GrantRequest {
            max_duffs: 100_000,
            max_credits: 1_000_000,
        };
        assert!(!ctx.platform_op(&dp, caps).unwrap().is_empty());
        assert!(!ctx.identity_scan(&dp).unwrap().is_empty());
        engine.block_on(engine.shutdown()).unwrap();

        // Without a vault there is no grant, and the command's JSON line
        // carries the vault's code.
        let dir = dw_testutil::private_tempdir();
        let engine = self::engine(&dir.path().join("data"));
        let session = open(&engine);
        let w = "ab".repeat(32);
        let err = run(
            &engine,
            &session,
            &format!("identity discover --wallet {w}"),
        )
        .unwrap_err();
        assert_eq!(err.code, "vault.no_vault");
        engine.block_on(engine.shutdown()).unwrap();
    }

    #[test]
    fn an_input_file_needs_scanned() {
        let dir = dw_testutil::private_tempdir();
        let (engine, session) = session(&dir.path().join("data"));
        let err = run(
            &engine,
            &session,
            "contact request --identity I C --input-file /x",
        )
        .unwrap_err();
        assert_eq!(
            (err.code, err.params),
            (
                "invalid_argument",
                json!({"detail": "--input-file needs --scanned"})
            )
        );
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// The row's rule: the DashPay commands reach Platform through the
    /// facade only. dwcli does not depend on platform-wallet or the SDK,
    /// and this module uses none of dw-engine's non-facade Platform modules.
    #[test]
    fn dwcli_does_not_depend_on_platform_crates() {
        let source = include_str!("dashpay.rs");
        let code = &source[..source.find("#[cfg(test)]").unwrap()];
        for banned in [
            "platform_wallet",
            "dash_sdk",
            "dpp::",
            "signers",
            "keys_policy",
        ] {
            assert!(!code.contains(banned), "dashpay.rs uses {banned}");
        }
        let manifest = include_str!("../Cargo.toml");
        for line in manifest.lines() {
            let name = line.split([' ', '=']).next().unwrap_or_default();
            for banned in [
                "platform-wallet",
                "platform-wallet-storage",
                "dash-sdk",
                "dpp",
            ] {
                assert_ne!(name, banned, "dwcli depends on {banned}");
                assert!(
                    !line.contains(&format!("package = \"{banned}\""))
                        && line.trim() != format!("[dependencies.{banned}]"),
                    "dwcli depends on {banned}"
                );
            }
        }
    }
}
