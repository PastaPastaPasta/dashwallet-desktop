//! The one error every command rejects with (E0-13 D-row "errors").
//!
//! `AppError { code, message, data }`:
//!
//! - `code` is a stable string the UI picks its copy by. In TypeScript it is
//!   a union of string literals ([`ErrorCode`]), so a `switch` over it is
//!   checked and a misspelt code does not compile. The strings are the
//!   engine's contract codes (m1–m4: `wallet_not_found`, `vault.locked`,
//!   `send.dust_amount`, `platform.grant_exceeded`, …) plus `app.*` for the
//!   binding's own refusals.
//! - `message` is diagnostic text for logs, never shown as copy.
//! - `data` carries the fields some codes have ([`ErrorData`]).
//!
//! Each engine error type maps here through one `From` with an exhaustive
//! `match`, so a new engine variant fails to compile until it has a code.

mod engine;
mod nested;
mod platform;

use serde::Serialize;
use ts_rs::TS;

use crate::dto::platform::{
    BudgetPurposeName, DispatchStateName, EligibilityName, KeyPurposeName, RevokeCauseName,
    UsernameRuleName,
};

pub use engine::{EngineCode, VaultCode};
pub use nested::{
    BackupCode, CoinJoinCode, CompatCode, ConsoleCode, LabelsCode, MasternodeCode, PsbtCode,
    SendCode, SignerCode, UriCode,
};
pub use platform::{
    AvatarCode, ContactCode, CreditsCode, IdentityCode, InvitationCode, NameCode, PlatformCode,
    RegistrationCode,
};

/// Error returned by every command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{}: {message}", code.as_str())]
pub struct AppError {
    pub code: ErrorCode,
    /// Diagnostic detail for logs; the UI's copy comes from `code`.
    pub message: String,
    pub data: Option<ErrorData>,
}

/// Every code a command can reject with, by domain. Serialised as the bare
/// code string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(untagged)]
pub enum ErrorCode {
    App(AppCode),
    Engine(EngineCode),
    Vault(VaultCode),
    Signer(SignerCode),
    Send(SendCode),
    Labels(LabelsCode),
    Compat(CompatCode),
    Backup(BackupCode),
    Psbt(PsbtCode),
    CoinJoin(CoinJoinCode),
    Masternode(MasternodeCode),
    Uri(UriCode),
    Console(ConsoleCode),
    Platform(PlatformCode),
    Identity(IdentityCode),
    Registration(RegistrationCode),
    Name(NameCode),
    Contact(ContactCode),
    Invitation(InvitationCode),
    Avatar(AvatarCode),
    Credits(CreditsCode),
}

/// Declares a code enum: each variant serialises as its string, and
/// `as_str` returns it.
macro_rules! codes {
    ($(#[$meta:meta])* $name:ident { $($(#[doc = $doc:literal])* $variant:ident = $code:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, ts_rs::TS)]
        pub enum $name {
            $($(#[doc = $doc])* #[serde(rename = $code)] $variant),+
        }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $code),+
                }
            }
        }
    };
}
pub(crate) use codes;

codes! {
    /// The binding's own refusals (`app.*`).
    AppCode {
        /// A self-test command in a normal launch.
        NotSelftest = "app.not_selftest",
        /// A developer-only command in a release build.
        NotDeveloperBuild = "app.not_developer_build",
        /// The self-test report could not be written.
        Selftest = "app.selftest",
        /// The self-test's passphrase file cannot be read, or is not the
        /// user's alone (on Unix: a regular file the user owns, mode 0600 or
        /// stricter, its final component not a symlink).
        SelftestPassphraseFile = "app.selftest_passphrase_file",
        /// The platform's app-data directory is unknown.
        DataDir = "app.data_dir",
        /// A handle id that is not (or no longer) open.
        UnknownHandle = "app.unknown_handle",
        /// More than `SendHandles::MAX` handles of a kind are open.
        TooManyHandles = "app.too_many_handles",
    }
}

/// Fields some codes carry. Credit amounts are decimal strings (see
/// `dto::credits`); duff amounts are numbers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ErrorData {
    /// `vault.throttled`: when to allow the next try.
    RetryAfter { secs: u64 },
    /// `vault.wrong_passphrase`: failed attempts so far, and the wait before
    /// the next try once the vault throttles.
    WrongPassphrase {
        failed_attempts: u32,
        retry_after_secs: Option<u64>,
    },
    /// `console.rpc_error`: Core's error code and message for the same
    /// failure, shown as "message (code N)".
    Rpc { rpc_code: i32, rpc_message: String },
    /// `console.authorization_required`: the grant to ask for (`authorize`),
    /// and the wallet it is for; then run the line again with it.
    Authorize {
        purpose: crate::api::vault::GrantPurposeSpec,
        wallet_id: Option<String>,
    },
    /// A duff shortfall (`*.funding_insufficient`, `send.amount_exceeds_balance`,
    /// `send.insufficient_mixed_funds`, …).
    Duffs { needed: Option<u64>, available: u64 },
    /// `platform.insufficient_credits`: a credit shortfall.
    Credits {
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        available: u64,
    },
    /// `insufficient_credits`: the identity (Base58) whose balance is short.
    IdentityCredits {
        identity: String,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        available: u64,
    },
    /// `credits.below_minimum`, `vault.quick_unlock_limit_exceeded`,
    /// `psbt.grant_exceeded` (the grant's cap), `coinjoin.insufficient_funds`
    /// (the mixing minimum): a limit in duffs.
    Limit { duffs: u64 },
    /// `send.absurd_fee`, `send.amount_with_fee_exceeds_balance`, `psbt.absurd_fee`.
    Fee { fee: u64, available: Option<u64> },
    /// `send.grant_exceeded`: the payment's outflow against the grant's cap, in duffs.
    SpendCap { max_duffs: u64, outflow: u64 },
    /// `send.*` refusals of one recipient (its index in the draft).
    Recipient { index: u32 },
    /// `platform.grant_exceeded`: amounts in the budget's unit (duffs or credits), as strings.
    Budget {
        purpose: BudgetPurposeName,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        needed: u64,
        #[ts(type = "string")]
        #[serde(with = "crate::dto::credits")]
        remaining: u64,
    },
    /// `platform.needs_grant`.
    NeedsGrant { purpose: BudgetPurposeName },
    /// `platform.lease_revoked`.
    Revoked { cause: RevokeCauseName },
    /// `platform.broadcast_unknown`, `platform.will_be_sent`: what to ask
    /// `dispatch_status` about.
    Artifact { artifact: String },
    /// `contact.payment_locked`, and a txid elsewhere.
    Txid { txid: String },
    /// `registration.recoverable`: the draft to resume;
    /// `registration.in_progress`: the draft that blocks (or
    /// `registration/<n>/funding` for a funding step whose row is gone).
    Draft { draft: String },
    /// `registration.funding_unknown_recent`: the earlier registration
    /// whose funding may still appear, and how long until a start may pass
    /// it, when known.
    DraftWait {
        draft: String,
        wait_secs: Option<u64>,
    },
    /// `name.invalid`: the rules the label breaks.
    UsernameRules { rules: Vec<UsernameRuleName> },
    /// `contact.ineligible`: the eligibility answer that refused.
    Ineligible { reason: EligibilityName },
    /// `identity.keys_missing`.
    KeysMissing { purpose: KeyPurposeName },
    /// `platform.feature_off`.
    Feature { feature: String },
    /// `platform.not_implemented`, `not_implemented`: the call that is not.
    Call { call: String },
    /// `send.broadcast_rejected`, `send.broadcast_unknown` and their `psbt.*` and
    /// `coinjoin.broadcast_rejected` twins: the peer's or library's reason.
    Reason { reason: String },
    /// `tx_action_refused`: why the abandon or resend was refused.
    TxAction { refusal: TxActionRefusalName },
    /// `request_not_found`, `height_out_of_range`, `backup.unsupported_version`,
    /// `psbt.too_large` (bytes): the number named.
    Number { value: u64 },
    /// `contact.details_too_long`: the details' DIP-15 encoding and its cap,
    /// in bytes.
    Size { bytes: u32, max: u32 },
    /// `send.outpoint_unavailable`, `outpoint_not_found`: the output (txid hex and index).
    Outpoint { txid: String, vout: u32 },
    /// `backup.already_exists`: the wallet (lower-case hex id) the backup would duplicate.
    Wallet { wallet_id: String },
    /// `compat.network_mismatch`: the keys are mainnet keys, or else test-network keys.
    KeysNetwork { mainnet: bool },
    /// `psbt.fee_rate_too_high`: the offending fee rate in duffs per kB.
    FeeRate { duffs_per_kb: u64 },
    /// `stuck_funding`: why Repair refused (E0-04 §6.5), and whether the
    /// same Repair may succeed later with nothing done by the user.
    StuckFunding {
        refusal: StuckFundingRefusalName,
        retry_later: bool,
    },
}

/// Why Repair refused a stuck funding step (E0-04 §6.5; the engine's
/// `StuckFundingRefusal`). Txids are display hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum StuckFundingRefusalName {
    /// The wallet's tracked asset locks were not read yet this session.
    RowsUnknown,
    /// The funding is not stuck: `status` is its dispatch state (`null`:
    /// it never started).
    NotStuck {
        status: Option<DispatchStateName>,
    },
    /// No marker of the funding step stands.
    NoMarker,
    /// A call of the funding step is running.
    CallRunning,
    JournalUnavailable,
    /// This transaction spends the wallet's coins and is not ChainLocked
    /// yet: wait for its ChainLock, or abandon it.
    SpendUnsettled {
        txid: String,
    },
    /// Every coin this asset lock spends is already spent.
    LockInputsSpent {
        txid: String,
    },
    /// The self-spend spends no input of this signed asset lock.
    MissesSignedLock {
        txid: String,
    },
    /// This tracked asset lock may be the funding's; its identity is not
    /// known yet.
    UnattributedLock {
        txid: String,
    },
    /// No asset lock the funding signed is known.
    NoKnownLock,
}

impl From<dw_engine::platform::lease::StuckFundingRefusal> for StuckFundingRefusalName {
    fn from(r: dw_engine::platform::lease::StuckFundingRefusal) -> Self {
        use dw_engine::platform::lease::StuckFundingRefusal as R;
        let txid = |t: dashcore::Txid| t.to_string();
        match r {
            R::RowsUnknown => Self::RowsUnknown,
            R::NotStuck(status) => Self::NotStuck {
                status: status.map(Into::into),
            },
            R::NoMarker => Self::NoMarker,
            R::CallRunning => Self::CallRunning,
            R::JournalUnavailable => Self::JournalUnavailable,
            R::SpendUnsettled(t) => Self::SpendUnsettled { txid: txid(t) },
            R::LockInputsSpent(t) => Self::LockInputsSpent { txid: txid(t) },
            R::MissesSignedLock(t) => Self::MissesSignedLock { txid: txid(t) },
            R::UnattributedLock(t) => Self::UnattributedLock { txid: txid(t) },
            R::NoKnownLock => Self::NoKnownLock,
        }
    }
}

/// Why `abandon_transaction` or `resend_transaction` refused (QT-105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TxActionRefusalName {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    InMempool,
    NotSentByWallet,
}

crate::dto::mirror_enum!(dw_engine::TxActionRefusal => TxActionRefusalName {
    Confirmed, InstantLocked, AlreadyAbandoned, Coinbase, InMempool, NotSentByWallet
});

impl AppError {
    pub fn new(code: impl Into<ErrorCode>, message: impl Into<String>) -> Self {
        AppError {
            code: code.into(),
            message: message.into(),
            data: None,
        }
    }

    pub fn with(mut self, data: ErrorData) -> Self {
        self.data = Some(data);
        self
    }

    pub fn app(code: AppCode, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    /// The engine's `invalid_argument`, for arguments the binding parses
    /// itself (hex, outpoints, enum strings).
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(EngineCode::InvalidArgument, message)
    }

    pub fn code_str(&self) -> &'static str {
        self.code.as_str()
    }
}

macro_rules! into_code {
    ($($variant:ident($ty:ident)),+ $(,)?) => {
        $(impl From<$ty> for ErrorCode {
            fn from(c: $ty) -> Self {
                ErrorCode::$variant(c)
            }
        })+

        impl ErrorCode {
            pub fn as_str(self) -> &'static str {
                match self {
                    $(ErrorCode::$variant(c) => c.as_str(),)+
                }
            }

            /// Every code, in declaration order (the TypeScript union's).
            pub fn all() -> Vec<ErrorCode> {
                let mut all = Vec::new();
                $(all.extend($ty::ALL.iter().map(|c| ErrorCode::$variant(*c)));)+
                all
            }
        }
    };
}

into_code!(
    App(AppCode),
    Engine(EngineCode),
    Vault(VaultCode),
    Signer(SignerCode),
    Send(SendCode),
    Labels(LabelsCode),
    Compat(CompatCode),
    Backup(BackupCode),
    Psbt(PsbtCode),
    CoinJoin(CoinJoinCode),
    Masternode(MasternodeCode),
    Uri(UriCode),
    Console(ConsoleCode),
    Platform(PlatformCode),
    Identity(IdentityCode),
    Registration(RegistrationCode),
    Name(NameCode),
    Contact(ContactCode),
    Invitation(InvitationCode),
    Avatar(AvatarCode),
    Credits(CreditsCode),
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialise_as_their_strings_and_are_unique() {
        let all = ErrorCode::all();
        let mut seen = std::collections::HashSet::new();
        for code in &all {
            let json = serde_json::to_value(code).unwrap();
            assert_eq!(json, serde_json::Value::String(code.as_str().into()));
            assert!(
                seen.insert(code.as_str()),
                "duplicate code {}",
                code.as_str()
            );
        }
    }

    #[test]
    fn an_error_is_code_message_and_data() {
        let e = AppError::new(VaultCode::Throttled, "slow down")
            .with(ErrorData::RetryAfter { secs: 30 });
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "code": "vault.throttled",
                "message": "slow down",
                "data": { "kind": "retryAfter", "secs": 30 }
            })
        );
        let credits = ErrorData::Credits {
            needed: u64::MAX,
            available: 1,
        };
        assert_eq!(
            serde_json::to_value(&credits).unwrap()["needed"],
            serde_json::Value::String(u64::MAX.to_string())
        );
    }
}
