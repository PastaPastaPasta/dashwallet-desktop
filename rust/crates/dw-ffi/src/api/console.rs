//! M2 local RPC console (QT-145): dash-qt's console grammar (`dw-console`)
//! over engine calls, with Core command names. Commands that need a full
//! node answer "not available" honestly (DESIGN-opus §1.14). Owner: R1
//! (engine-tools, `dw-console`). Contract: docs/contracts/m2-engine.md §2.5.

use zeroize::Zeroizing;

use crate::api::common::{domain_error_common, ensure_open, not_implemented, parse_wallet_id};
use crate::{GrantPurpose, NetworkSession};

/// One console command for `help` and tab completion.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ConsoleCommand {
    pub name: String,
    /// Core's help category ("Wallet", "Network", "Control", …).
    pub category: String,
    /// dash-qt redacts its arguments in the echo and history.
    pub sensitive: bool,
    /// Answered locally; `false` = "Not available in SPV mode."
    pub available: bool,
}

/// A command's result as dashd prints it: JSON pretty-printed with 2-space
/// indent, or plain text for string results (`help`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ConsoleOutput {
    pub text: String,
    pub is_json: bool,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ConsoleError {
    /// Code `console.parse_error`: dash-qt "Error: Invalid command line".
    #[error("parse error: {detail}")]
    ParseError { detail: String },
    /// Code `console.rpc_error`: shown as "message (code N)"; `code` and
    /// `message` are Core's for the same failure.
    #[error("{message} (code {code})")]
    RpcError { code: i32, message: String },
    /// Code `console.not_available`: the command needs a full node
    /// ("Not available in SPV mode.").
    #[error("{command} not available in SPV mode")]
    NotAvailable { command: String },
    /// Code `console.authorization_required`: the command spends, signs or
    /// reveals. The host authorizes `purpose` for `wallet_id` and runs the
    /// same line again with the grant.
    #[error("authorization required")]
    AuthorizationRequired {
        purpose: GrantPurpose,
        wallet_id: Option<String>,
    },
    /// Code `console.wallet_required`: a wallet command with no wallet
    /// selected ("Executing command without any wallet").
    #[error("wallet required")]
    WalletRequired,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(ConsoleError);
crate::api::common::export_error_code!(ConsoleError);

impl ConsoleError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::ParseError { .. } => "console.parse_error",
            Self::RpcError { .. } => "console.rpc_error",
            Self::NotAvailable { .. } => "console.not_available",
            Self::AuthorizationRequired { .. } => "console.authorization_required",
            Self::WalletRequired => "console.wallet_required",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

/// Every console command, sorted by name (tab completion, `help`).
#[uniffi::export]
pub fn console_commands() -> Result<Vec<ConsoleCommand>, ConsoleError> {
    not_implemented("console_commands")
}

/// dash-qt history redaction: the arguments of a sensitive command
/// (`walletpassphrase`, `importprivkey`, `upgradetohd`, …) become `(…)`.
/// The host echoes and stores only this text. `line` is UTF-8 bytes and is
/// zeroized.
#[uniffi::export]
pub fn console_redact(line: Vec<u8>) -> Result<String, ConsoleError> {
    let _line = Zeroizing::new(line);
    not_implemented("console_redact")
}

#[uniffi::export]
impl NetworkSession {
    /// Parses and runs one console line (nested calls, `[key]` indexing,
    /// quoting as dash-qt) against `wallet_id` (`None` = no wallet). `line`
    /// is UTF-8 bytes because it may hold a passphrase; it is zeroized.
    /// `grant_id` answers an earlier `AuthorizationRequired`.
    pub async fn console_execute(
        &self,
        wallet_id: Option<String>,
        line: Vec<u8>,
        grant_id: Option<String>,
    ) -> Result<ConsoleOutput, ConsoleError> {
        let _line = Zeroizing::new(line);
        let _ = grant_id;
        if let Some(id) = &wallet_id {
            parse_wallet_id(id)?;
        }
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.console_execute")
    }
}
