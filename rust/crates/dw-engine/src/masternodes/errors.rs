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
    /// reason verbatim (`bad-protx-dup-key`, …). `operation`: the wallet's
    /// operation (its id) holding what the ProTx claims, for a `protx-dup`
    /// the wallet finds itself (MG-04 D51); `None` otherwise.
    #[error(
        "refused by Core's rules: {reason}{}",
        operation.as_deref().map(|o| format!(" (operation {o} holds it: end that one first)")).unwrap_or_default()
    )]
    ProtxRefused {
        reason: String,
        operation: Option<String>,
    },
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
    /// ProUpRegTx: an operation on this masternode (`operation`, its id,
    /// maybe another loaded wallet's) isn't in the synced list yet: an
    /// update now could keep a key it changes, or be mined before it
    /// (MG-04 D43, D49).
    #[error(
        "operation {operation} on this masternode isn't in the list yet: wait for it, or end it (discard it if never sent, else abandon its transaction)"
    )]
    UpdatePending { operation: String },
    /// Resending a ProUpRegTx that `operation` (its id, maybe another
    /// loaded wallet's), saved no earlier, would undo (MG-04 D43).
    #[error("operation {operation} came after this update: resending it would undo that one")]
    UpdateSuperseded { operation: String },
    /// No shared-masternode session of the wallet has this id (MG-04b §10).
    #[error("unknown shared masternode session")]
    SharedSessionNotFound,
    /// The call doesn't apply at the session's stage or to its role
    /// (MG-04b §10): `stage` names it, the derived `dead` among them, or
    /// `replied` for a fee change after another party's reply (D33).
    #[error("the shared session is {stage}")]
    SharedStage { stage: String },
    /// A pasted or opened message is refused (MG-04b §10): `reason` names
    /// why, never the text (MG-00 D36); `share` is the share whose approval
    /// doesn't verify, for `bad_signature`.
    #[error(
        "shared masternode message refused: {reason}{}",
        share.map(|s| format!(" (share {s})")).unwrap_or_default()
    )]
    SharedMessage { reason: String, share: Option<u16> },
    /// A consent or funding check of MG-04b §4 fails: `reason` names it;
    /// `needed` is the minimum fee, in duffs, for `fee_low`.
    #[error(
        "the shared registration fails this wallet's check: {reason}{}",
        needed.map(|n| format!(" (needs {n} duffs)")).unwrap_or_default()
    )]
    SharedFundingMismatch { reason: String, needed: Option<u64> },
    /// A share whose owner key this wallet doesn't hold.
    #[error("this wallet holds no owner key of that share")]
    ShareNotMine,
    /// A maintenance call before the registration's block is in the list
    /// (`tip ≤ registered_height`, MG-04b §3.3).
    #[error("too early for the shared masternode")]
    SharedTooEarly,
    /// The registration ProRegTx isn't in the wallet's history (MG-04b D29).
    #[error("the shared masternode's registration is unknown to this wallet")]
    SharedRegistrationUnknown,
    /// A dissolve-together request another wallet prepared takes `fee`
    /// duffs from this wallet's actor share, above the relay minimum:
    /// approving it needs `accept_actor_fee` (MG-04b m-15).
    #[error(
        "the request takes a fee of {fee} duffs from this wallet's share: accept it to approve"
    )]
    SharedActorFee { fee: u64 },
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
            Self::SharedSessionNotFound => "masternodes.shared_session_not_found",
            Self::SharedStage { .. } => "masternodes.shared_stage",
            Self::SharedMessage { .. } => "masternodes.shared_message",
            Self::SharedFundingMismatch { .. } => "masternodes.shared_funding_mismatch",
            Self::ShareNotMine => "masternodes.share_not_mine",
            Self::SharedTooEarly => "masternodes.shared_too_early",
            Self::SharedRegistrationUnknown => "masternodes.shared_registration_unknown",
            Self::SharedActorFee { .. } => "masternodes.shared_actor_fee",
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
