//! `MasternodesError`, the facade's error with its stable codes
//! (`docs/contracts/mn-engine.md` §4). The `Display` text is diagnostic
//! detail for logs; the UI picks its copy by `code()`. No message repeats a
//! hash or key the caller passed: a pasted secret has a hash's shape
//! (MG-03 D23). The m1 common codes
//! (`network_not_open`, `wallet_not_found`, `invalid_argument`, …) come
//! through `Engine` unchanged, so no m1–m4 table gains a code.

use crate::error::EngineError;
use crate::platform::PlatformError;

use super::records::AttachedKeyRole;

#[derive(Debug, thiserror::Error)]
pub enum MasternodesError {
    /// The SPV masternode list isn't synced (no list yet, or the headers or
    /// masternode phase is still running).
    #[error("masternode list unavailable")]
    ListUnavailable,
    /// No masternode the wallet or the synced list knows matches.
    #[error("unknown masternode")]
    NotFound,
    #[error("masternode already tracked")]
    AlreadyTracked,
    #[error("masternode not tracked")]
    NotTracked,
    /// The key isn't the masternode's key for `role` (MG-00 D13).
    #[error("the key does not belong to this masternode ({role:?})")]
    KeyMismatch { role: AttachedKeyRole },
    #[error("not an evonode")]
    NotEvonode,
    /// Platform has no identity for the evonode.
    #[error("no Platform identity for this evonode")]
    NoIdentity,
    /// The session runs without Platform, or Platform could not be read.
    #[error("Platform unavailable: {detail}")]
    PlatformUnavailable { detail: String },
    /// The call needs an API the pinned libraries don't expose
    /// (MG-03 §7, D11), or a version-3 ProTx (MG-04 D1, `protx_v3`):
    /// `field` names what needs it (`service`: an address kind only
    /// version 3 carries).
    #[error(
        "unsupported: {feature}{}",
        field.as_deref().map(|f| format!(" (needed by the {f})")).unwrap_or_default()
    )]
    Unsupported {
        feature: String,
        field: Option<String>,
    },
    /// A contract stub (`call` is `"Masternodes.<method>"`).
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// A Core rule refuses the ProTx (MG-04 D5): `reason` is Core's reject
    /// reason verbatim (`bad-protx-dup-key`, …).
    #[error("refused by Core's rules: {reason}")]
    ProtxRefused { reason: String },
    /// An `ExistingUtxo` collateral isn't an unspent wallet coin of the
    /// type's amount at a P2PKH address, or is already a collateral of the
    /// wallet's masternodes or operations (MG-04 D23).
    #[error("collateral unavailable")]
    CollateralUnavailable,
    /// The `External` signature isn't the collateral address's over the
    /// saved message.
    #[error("the signature does not match the collateral address and message")]
    SignatureMismatch,
    #[error("unknown ProTx operation")]
    OperationNotFound,
    /// The call doesn't apply to the operation's state (its snake-case
    /// name).
    #[error("the operation is {state}")]
    OperationState { state: String },
    /// Another call on the operation is running.
    #[error("another attempt on the operation is running")]
    InFlight,
    /// Every keychain key of `role` the registration may use is in use
    /// (MG-04 D13).
    #[error("no unused {role} key")]
    KeysExhausted { role: String },
    /// ProUpRegTx: no keychain owner key 0–19 nor attached `owner` key has
    /// the masternode's owner key id, or the id isn't known (MG-04 D12).
    #[error("no owner key of this masternode")]
    NoOwnerKey,
    /// ProUpRegTx: a key is kept while an operation of the wallet that may
    /// change it (`operation`, its id) isn't in the synced list yet; every
    /// key must be given (MG-04 D43).
    #[error(
        "operation {operation} may change this masternode's keys before the list shows it: give every key"
    )]
    UpdatePending { operation: String },
    /// Resending a ProUpRegTx that `operation` (its id), saved no earlier,
    /// would undo: discard that one first (MG-04 D43).
    #[error("operation {operation} came after this update: resending it would undo that one")]
    UpdateSuperseded { operation: String },
    /// A lease refused (MG-05 D12).
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

impl MasternodesError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ListUnavailable => "masternodes.list_unavailable",
            Self::NotFound => "masternodes.not_found",
            Self::AlreadyTracked => "masternodes.already_tracked",
            Self::NotTracked => "masternodes.not_tracked",
            Self::KeyMismatch { .. } => "masternodes.key_mismatch",
            Self::NotEvonode => "masternodes.not_evonode",
            Self::NoIdentity => "masternodes.no_identity",
            Self::PlatformUnavailable { .. } => "masternodes.platform_unavailable",
            Self::Unsupported { .. } => "masternodes.unsupported",
            Self::NotImplemented { .. } => "masternodes.not_implemented",
            Self::ProtxRefused { .. } => "masternodes.protx_refused",
            Self::CollateralUnavailable => "masternodes.collateral_unavailable",
            Self::SignatureMismatch => "masternodes.signature_mismatch",
            Self::OperationNotFound => "masternodes.operation_not_found",
            Self::OperationState { .. } => "masternodes.operation_state",
            Self::InFlight => "masternodes.in_flight",
            Self::KeysExhausted { .. } => "masternodes.keys_exhausted",
            Self::NoOwnerKey => "masternodes.no_owner_key",
            Self::UpdatePending { .. } => "masternodes.update_pending",
            Self::UpdateSuperseded { .. } => "masternodes.update_superseded",
            Self::Platform(e) => e.code(),
            Self::Engine(e) => e.code(),
        }
    }

    /// The wrapped engine error, if any.
    pub fn engine(&self) -> Option<&EngineError> {
        match self {
            Self::Engine(e) => Some(e),
            _ => None,
        }
    }
}

impl From<dw_vault::VaultError> for MasternodesError {
    fn from(e: dw_vault::VaultError) -> Self {
        Self::Engine(e.into())
    }
}

impl From<tokio::task::JoinError> for MasternodesError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Engine(e.into())
    }
}
