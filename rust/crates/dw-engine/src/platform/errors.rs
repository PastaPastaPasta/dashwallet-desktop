//! Per-domain errors of the DashPay facade with their stable codes
//! (DASHPAY §3.6, `docs/contracts/m4-dashpay-engine.md` §4). The `Display`
//! text is diagnostic detail for logs; the UI picks its copy by `code()`.
//! No `Display` or `detail` ever quotes a `BearerSecret` input.
//!
//! `PlatformError` holds the codes every call can return: `platform.*`, the
//! `identity.*` codes and the m1 common codes. Each other domain wraps it, so
//! a `platform.*` code reaches the host whichever domain the call belongs to;
//! `platform()` finds it again.

use super::contacts::Eligibility;
use super::flows::{BudgetPurpose, RevokeCause};
use super::identity::KeyPurpose;
use super::names::UsernameRule;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("platform unavailable")]
    Unavailable,
    #[error("platform timeout")]
    Timeout,
    #[error("platform proof invalid")]
    ProofInvalid,
    /// SPV and the trusted service disagree on a quorum key (§2.2 rule 4).
    #[error("quorum key mismatch between SPV and the trusted service")]
    TrustMismatch,
    #[error("platform context unavailable")]
    ContextUnavailable,
    /// The vault is locked or the flow lease has ended.
    #[error("signer unavailable")]
    SignerUnavailable,
    /// The seed-binding check failed.
    #[error("seed mismatch")]
    SeedMismatch,
    #[error("insufficient credits: {needed} needed, {available} available")]
    InsufficientCredits { needed: u64, available: u64 },
    #[error("grant invalid")]
    GrantInvalid,
    /// A lease budget cannot cover the charge (E0-04 §4.2).
    #[error("{purpose:?} budget exceeded: {needed} needed, {remaining} remaining")]
    GrantExceeded {
        purpose: BudgetPurpose,
        needed: u64,
        remaining: u64,
    },
    /// A signed artifact was handed off and its outcome is unknown: it may
    /// already be out (E0-04 `MaybeSent`). Never retried blindly.
    #[error("broadcast outcome unknown for {artifact}")]
    BroadcastUnknown { artifact: String },
    /// The artifact is committed and the engine will send it again, for
    /// example when the network is back. Never retried.
    #[error("{artifact} will be sent")]
    WillBeSent { artifact: String },
    /// Lock won the flow's permit before the hand-off: nothing was sent
    /// (E0-04 `Cancelled`, `lease.locked`).
    #[error("cancelled before anything was sent")]
    Cancelled,
    /// The lease is parked or its grants died with an unlock, a scope or a
    /// passphrase change: the flow needs a fresh grant for `purpose`.
    #[error("needs a new grant for {purpose:?}")]
    NeedsGrant { purpose: BudgetPurpose },
    /// The lease the call named was revoked.
    #[error("lease revoked: {cause:?}")]
    LeaseRevoked { cause: RevokeCause },
    /// The lease the call named has ended: `end_flow` or the idle reaper.
    /// (Its own key's expiry parks it: `NeedsGrant`.)
    #[error("lease expired")]
    LeaseExpired,
    #[error("feature off: {feature}")]
    FeatureOff { feature: String },
    /// `call` is `"DashPay.<method>"`, `"NetworkSession.<method>"` or the
    /// free function's name.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    #[error("identity: {0}")]
    Identity(IdentityError),
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    #[error("network session is not open")]
    NetworkNotOpen,
    #[error("wallet not found")]
    WalletNotFound,
    #[error("storage error: {detail}")]
    Storage { detail: String },
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

impl PlatformError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unavailable => "platform.unavailable",
            Self::Timeout => "platform.timeout",
            Self::ProofInvalid => "platform.proof_invalid",
            Self::TrustMismatch => "platform.trust_mismatch",
            Self::ContextUnavailable => "platform.context_unavailable",
            Self::SignerUnavailable => "platform.signer_unavailable",
            Self::SeedMismatch => "platform.seed_mismatch",
            Self::InsufficientCredits { .. } => "platform.insufficient_credits",
            Self::GrantInvalid => "platform.grant_invalid",
            Self::GrantExceeded { .. } => "platform.grant_exceeded",
            Self::BroadcastUnknown { .. } => "platform.broadcast_unknown",
            Self::WillBeSent { .. } => "platform.will_be_sent",
            Self::Cancelled => "platform.cancelled",
            Self::NeedsGrant { .. } => "platform.needs_grant",
            Self::LeaseRevoked { .. } => "platform.lease_revoked",
            Self::LeaseExpired => "platform.lease_expired",
            Self::FeatureOff { .. } => "platform.feature_off",
            Self::NotImplemented { .. } => "platform.not_implemented",
            Self::Identity(e) => e.code(),
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen => "network_not_open",
            Self::WalletNotFound => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::Internal { .. } => "internal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdentityError {
    #[error("identity not found")]
    NotFound,
    #[error("identity has no {purpose:?} key")]
    KeysMissing { purpose: KeyPurpose },
}

impl IdentityError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "identity.not_found",
            Self::KeysMissing { .. } => "identity.keys_missing",
        }
    }
}

impl From<IdentityError> for PlatformError {
    fn from(e: IdentityError) -> Self {
        Self::Identity(e)
    }
}

/// Registration also returns the `name.*` refusals of its label and the
/// `invitation.*` refusals of invitation funding.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegistrationError {
    /// Another registration flow is running.
    #[error("a registration is in progress")]
    InProgress,
    #[error("insufficient funds: {needed} duffs needed, {available} available")]
    FundingInsufficient { needed: u64, available: u64 },
    #[error("no InstantSend lock in time")]
    IslockTimeout,
    /// The flow parked and can be resumed with `resume_registration`.
    #[error("registration {draft} can be resumed")]
    Recoverable { draft: String },
    #[error("the identity already has a username")]
    AlreadyHasUsername,
    #[error(transparent)]
    Name(NameError),
    #[error(transparent)]
    Invitation(InvitationError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl RegistrationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InProgress => "registration.in_progress",
            Self::FundingInsufficient { .. } => "registration.funding_insufficient",
            Self::IslockTimeout => "registration.islock_timeout",
            Self::Recoverable { .. } => "registration.recoverable",
            Self::AlreadyHasUsername => "registration.already_has_username",
            Self::Name(e) => e.code(),
            Self::Invitation(e) => e.code(),
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Name(e) => e.platform(),
            Self::Invitation(e) => e.platform(),
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

/// A wrapped domain's `Platform` variant becomes this enum's own, so one
/// code has one representation.
impl From<NameError> for RegistrationError {
    fn from(e: NameError) -> Self {
        match e {
            NameError::Platform(p) => Self::Platform(p),
            other => Self::Name(other),
        }
    }
}

impl From<InvitationError> for RegistrationError {
    fn from(e: InvitationError) -> Self {
        match e {
            InvitationError::Platform(p) => Self::Platform(p),
            other => Self::Invitation(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// `rules` are the rules the label breaks.
    #[error("invalid username: {rules:?}")]
    Invalid { rules: Vec<UsernameRule> },
    #[error("username taken")]
    Taken,
    #[error("username contest open")]
    ContestOpen,
    #[error("username locked")]
    Locked,
    /// A contested name cannot be claimed with this invitation.
    #[error("username unavailable for this invitation")]
    UnavailableForInvite,
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl NameError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid { .. } => "name.invalid",
            Self::Taken => "name.taken",
            Self::ContestOpen => "name.contest_open",
            Self::Locked => "name.locked",
            Self::UnavailableForInvite => "name.unavailable_for_invite",
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContactError {
    /// `reason` is the `eligibility` answer that refused; never `Ok`.
    #[error("contact ineligible: {reason:?}")]
    Ineligible { reason: Eligibility },
    #[error("already a contact")]
    AlreadyContact,
    #[error("contact request pending")]
    RequestPending,
    #[error("contact is the identity itself")]
    IsSelf,
    #[error("payment channel broken")]
    ChannelBroken,
    /// An ambiguous broadcast to this contact is not settled yet.
    #[error("payments to this contact are locked by {txid}")]
    PaymentLocked { txid: String },
    /// The scanned `dapk` proof has expired, or the scan id is unknown or
    /// used: "this QR code has expired; ask for a new one".
    #[error("scanned code expired")]
    ScanExpired,
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl ContactError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Ineligible { .. } => "contact.ineligible",
            Self::AlreadyContact => "contact.already_contact",
            Self::RequestPending => "contact.request_pending",
            Self::IsSelf => "contact.self",
            Self::ChannelBroken => "contact.channel_broken",
            Self::PaymentLocked { .. } => "contact.payment_locked",
            Self::ScanExpired => "contact.scan_expired",
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvitationError {
    #[error("invitation invalid")]
    Invalid,
    #[error("invitation already claimed")]
    Claimed,
    #[error("invitation expired")]
    Expired,
    #[error("the wallet already has an identity")]
    AlreadyHasIdentity,
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl InvitationError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid => "invitation.invalid",
            Self::Claimed => "invitation.claimed",
            Self::Expired => "invitation.expired",
            Self::AlreadyHasIdentity => "invitation.already_has_identity",
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AvatarError {
    #[error("avatar too large")]
    TooLarge,
    #[error("avatar format unsupported")]
    Unsupported,
    #[error("avatar fetch failed")]
    FetchFailed,
    /// The image does not match the profile's `avatarHash` or fingerprint.
    #[error("avatar hash mismatch")]
    HashMismatch,
    /// No Imgur client id is configured.
    #[error("avatar upload unconfigured")]
    UploadUnconfigured,
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl AvatarError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TooLarge => "avatar.too_large",
            Self::Unsupported => "avatar.unsupported",
            Self::FetchFailed => "avatar.fetch_failed",
            Self::HashMismatch => "avatar.hash_mismatch",
            Self::UploadUnconfigured => "avatar.upload_unconfigured",
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

/// Top-up and withdraw. A top-up spends Core duffs, so its shortfall is
/// `credits.funding_insufficient`, not `platform.insufficient_credits`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CreditsError {
    #[error("insufficient funds: {needed} duffs needed, {available} available")]
    FundingInsufficient { needed: u64, available: u64 },
    /// Below `CostTable.top_up_min_duffs`.
    #[error("below the minimum of {min} duffs")]
    BelowMinimum { min: u64 },
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

impl CreditsError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::FundingInsufficient { .. } => "credits.funding_insufficient",
            Self::BelowMinimum { .. } => "credits.below_minimum",
            Self::Platform(e) => e.code(),
        }
    }

    pub fn platform(&self) -> Option<&PlatformError> {
        match self {
            Self::Platform(e) => Some(e),
            _ => None,
        }
    }
}

// ---- The error mapping (m4-dashpay-engine.md §6; E0-05) -------------------
//
// One `From` per source into `PlatformError`. Each maps what has a facade
// code and falls back to `Internal{detail}`; later tasks extend these in
// place rather than mapping on their own.

fn internal(e: impl std::fmt::Display) -> PlatformError {
    PlatformError::Internal {
        detail: e.to_string(),
    }
}

/// `mapped`, or `fallback` when it is only `Internal`: a wrapper's own
/// meaning when what it wraps has no code.
fn or_else(mapped: PlatformError, fallback: impl FnOnce() -> PlatformError) -> PlatformError {
    match mapped {
        PlatformError::Internal { .. } => fallback(),
        known => known,
    }
}

impl From<dash_sdk::Error> for PlatformError {
    fn from(e: dash_sdk::Error) -> Self {
        // Retry envelopes are peeled in a loop, not by recursion: nothing
        // bounds their depth (review r3 N2). The last attempt's error says
        // why no node answered; if it says nothing, the envelope's own
        // `unavailable`.
        let mut leaf = e;
        let mut retried = false;
        while let dash_sdk::Error::NoAvailableAddressesToRetry(last) = leaf {
            leaf = *last;
            retried = true;
        }
        let mapped = sdk_leaf(leaf);
        if retried {
            or_else(mapped, || Self::Unavailable)
        } else {
            mapped
        }
    }
}

/// One SDK error that is not a retry envelope.
fn sdk_leaf(e: dash_sdk::Error) -> PlatformError {
    use dash_sdk::Error as Sdk;
    // Platform's balance refusal keeps its figures. `e` is no retry
    // envelope, so the library's helper does not descend.
    if let Some(promoted) = platform_wallet::error::promote_identity_insufficient_balance(&e) {
        return wallet_leaf(promoted);
    }
    match e {
        Sdk::TimeoutReached(..) => PlatformError::Timeout,
        Sdk::DapiClientError(_) | Sdk::StaleNode(_) => PlatformError::Unavailable,
        Sdk::Proof(_) | Sdk::DriveProofError(..) | Sdk::InvalidProvedResponse(_) => {
            PlatformError::ProofInvalid
        }
        Sdk::ContextProviderError(_) => PlatformError::ContextUnavailable,
        other => internal(other),
    }
}

impl From<platform_wallet::PlatformWalletError> for PlatformError {
    fn from(e: platform_wallet::PlatformWalletError) -> Self {
        use platform_wallet::PlatformWalletError as Pw;
        // Restore envelopes are peeled in a loop (review r3 N2); they map as
        // what they wrap, or `storage` when that has no code.
        let mut leaf = e;
        let mut restoring = false;
        while let Pw::PersisterRestore(inner) = leaf {
            leaf = *inner;
            restoring = true;
        }
        match (wallet_leaf(leaf), restoring) {
            (Self::Internal { detail }, true) => Self::Storage {
                detail: format!("failed to restore persisted platform-address state: {detail}"),
            },
            (mapped, _) => mapped,
        }
    }
}

/// One library error that is not a restore envelope. Its SDK sources map
/// through the shared conversion (review r2 N1), which does not recurse.
fn wallet_leaf(e: platform_wallet::PlatformWalletError) -> PlatformError {
    use platform_wallet::PlatformWalletError as Pw;
    match e {
        Pw::Sdk(source) | Pw::TokenOperationFailed { source, .. } => source.into(),
        // The scan never reached Platform: `unavailable` unless the last
        // probe's failure says more.
        Pw::IdentityDiscoveryIncomplete { source, .. } => {
            or_else(PlatformError::from(*source), || PlatformError::Unavailable)
        }
        Pw::InsufficientIdentityCredits {
            required,
            available,
            ..
        } => PlatformError::InsufficientCredits {
            needed: required,
            available,
        },
        Pw::WalletNotFound(_) => PlatformError::WalletNotFound,
        Pw::WalletLocked => PlatformError::SignerUnavailable,
        Pw::SeedMismatch { .. } => PlatformError::SeedMismatch,
        Pw::SeedBindingUnanswered { .. } | Pw::FinalityTimeout(_) => PlatformError::Timeout,
        Pw::IdentityNotFound(_) => PlatformError::Identity(IdentityError::NotFound),
        Pw::ContactSyncUnreachable { .. } => PlatformError::Unavailable,
        Pw::InvalidParameter(detail) => PlatformError::InvalidArgument { detail },
        e @ (Pw::PersisterLoad(_) | Pw::PersisterStore(_) | Pw::Persistence(_)) => {
            PlatformError::Storage {
                detail: e.to_string(),
            }
        }
        other => internal(other),
    }
}

impl From<dw_vault::VaultError> for PlatformError {
    fn from(e: dw_vault::VaultError) -> Self {
        use dw_vault::VaultError as V;
        match e {
            // Unlock, enter the passphrase, or the wallet has no keys at all
            // (watch-only): m4 §4 gives all of them `signer_unavailable`.
            V::NoVault
            | V::Locked
            | V::MixingOnly
            | V::CredentialRequired
            | V::PassphraseStale
            | V::NoSecret => Self::SignerUnavailable,
            V::GrantInvalid | V::GrantPurposeMismatch => Self::GrantInvalid,
            V::InvalidArgument(detail) => Self::InvalidArgument { detail },
            e @ (V::Storage(_) | V::Corrupt(_) | V::OsStoreUnavailable(_)) => Self::Storage {
                detail: e.to_string(),
            },
            other => internal(other),
        }
    }
}

impl From<crate::EngineError> for PlatformError {
    fn from(e: crate::EngineError) -> Self {
        use crate::EngineError as E;
        match e {
            E::InvalidArgument(detail) | E::InvalidConfig(detail) => {
                Self::InvalidArgument { detail }
            }
            E::NetworkNotOpen(_) => Self::NetworkNotOpen,
            E::WalletNotFound(_) => Self::WalletNotFound,
            e @ (E::Storage(_) | E::StorageInUse(_) | E::Io(_)) => Self::Storage {
                detail: e.to_string(),
            },
            E::InsufficientCredits {
                needed, available, ..
            } => Self::InsufficientCredits { needed, available },
            E::Vault(e) => e.into(),
            E::Signer(dw_vault::SignerError::Locked) => Self::SignerUnavailable,
            E::Signer(dw_vault::SignerError::Vault(e)) => e.into(),
            E::NotImplemented(call) => Self::NotImplemented { call },
            other => internal(other),
        }
    }
}

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use crate::EngineError;
    use dw_vault::{SignerError, VaultError};
    use platform_wallet::PlatformWalletError;

    #[test]
    fn engine_errors_keep_their_common_codes() {
        let cases = [
            (EngineError::InvalidArgument("x".into()), "invalid_argument"),
            (
                EngineError::NetworkNotOpen("regtest".into()),
                "network_not_open",
            ),
            (EngineError::WalletNotFound("w".into()), "wallet_not_found"),
            (EngineError::Storage("disk".into()), "storage"),
            (
                EngineError::InsufficientCredits {
                    identity_id: "i".into(),
                    needed: 5,
                    available: 2,
                },
                "platform.insufficient_credits",
            ),
            (
                EngineError::Vault(VaultError::Locked),
                "platform.signer_unavailable",
            ),
            (
                EngineError::Signer(SignerError::Locked),
                "platform.signer_unavailable",
            ),
            (
                EngineError::Vault(VaultError::GrantInvalid),
                "platform.grant_invalid",
            ),
            (
                EngineError::NotImplemented("x".into()),
                "platform.not_implemented",
            ),
            (EngineError::SpvNotRunning, "internal"),
        ];
        for (engine, code) in cases {
            assert_eq!(PlatformError::from(engine).code(), code);
        }
        assert_eq!(
            PlatformError::from(EngineError::InsufficientCredits {
                identity_id: "i".into(),
                needed: 5,
                available: 2,
            }),
            PlatformError::InsufficientCredits {
                needed: 5,
                available: 2
            }
        );
    }

    #[test]
    fn vault_errors_ask_for_a_signer_or_a_grant() {
        for e in [
            VaultError::NoVault,
            VaultError::Locked,
            VaultError::MixingOnly,
            VaultError::CredentialRequired,
            VaultError::PassphraseStale,
            VaultError::NoSecret,
        ] {
            assert_eq!(PlatformError::from(e).code(), "platform.signer_unavailable");
        }
        assert_eq!(
            PlatformError::from(VaultError::GrantPurposeMismatch).code(),
            "platform.grant_invalid"
        );
        assert_eq!(
            PlatformError::from(VaultError::Corrupt("x".into())).code(),
            "storage"
        );
        assert_eq!(PlatformError::from(VaultError::NotEmpty).code(), "internal");
    }

    #[test]
    fn library_errors_map_to_their_platform_codes() {
        let id = dpp::prelude::Identifier::from([3u8; 32]);
        let cases = [
            (
                PlatformWalletError::WalletLocked,
                "platform.signer_unavailable",
            ),
            (
                PlatformWalletError::SeedMismatch {
                    wallet_id: "w".into(),
                },
                "platform.seed_mismatch",
            ),
            (
                PlatformWalletError::SeedBindingUnanswered {
                    wallet_id: "w".into(),
                },
                "platform.timeout",
            ),
            (
                PlatformWalletError::IdentityNotFound(id),
                "identity.not_found",
            ),
            (
                PlatformWalletError::ContactSyncUnreachable { identities: 1 },
                "platform.unavailable",
            ),
            (
                PlatformWalletError::InvalidParameter("bad".into()),
                "invalid_argument",
            ),
            (
                PlatformWalletError::InsufficientIdentityCredits {
                    identity_id: id,
                    required: 9,
                    available: 1,
                },
                "platform.insufficient_credits",
            ),
            (PlatformWalletError::NoPrimaryIdentity, "internal"),
        ];
        for (library, code) in cases {
            assert_eq!(PlatformError::from(library).code(), code);
        }
    }

    fn timeout() -> dash_sdk::Error {
        dash_sdk::Error::TimeoutReached(std::time::Duration::from_secs(1), "x".into())
    }

    fn context() -> dash_sdk::Error {
        dash_sdk::Error::ContextProviderError(dash_context_provider::ContextProviderError::Generic(
            "x".into(),
        ))
    }

    /// Platform's balance refusal as a broadcast error, as `error.rs` builds
    /// it: 25 018 360 000 credits required, 24 818 360 000 available.
    pub(super) fn balance_refusal() -> dash_sdk::Error {
        use dpp::consensus::ConsensusError;
        use dpp::consensus::codes::ErrorWithCode;
        use dpp::consensus::state::identity::IdentityInsufficientBalanceError;
        let cause: ConsensusError = IdentityInsufficientBalanceError::new(
            dpp::prelude::Identifier::from([9u8; 32]),
            24_818_360_000,
            25_018_360_000,
        )
        .into();
        dash_sdk::Error::StateTransitionBroadcastError(
            dash_sdk::error::StateTransitionBroadcastError {
                code: cause.code(),
                message: cause.to_string(),
                cause: Some(cause),
            },
        )
    }

    /// Review DW-E0-05-r2-gpt N1: an SDK error inside a library wrapper gets
    /// the same code as on its own; `internal` only when nothing maps.
    #[test]
    fn wrapped_sdk_errors_keep_their_codes() {
        let token = |source| PlatformWalletError::TokenOperationFailed {
            operation: "claim",
            source,
        };
        let discovery = |source| PlatformWalletError::IdentityDiscoveryIncomplete {
            start_index: 0,
            probed: 1,
            failed_probes: 1,
            source: Box::new(source),
        };
        let retried = |last| dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(last));
        let cases = [
            (token(timeout()), "platform.timeout"),
            (token(context()), "platform.context_unavailable"),
            (token(retried(timeout())), "platform.timeout"),
            (token(dash_sdk::Error::Generic("x".into())), "internal"),
            (discovery(timeout()), "platform.timeout"),
            (discovery(context()), "platform.context_unavailable"),
            // The scan never reached Platform, whatever the probe said.
            (
                discovery(dash_sdk::Error::Generic("x".into())),
                "platform.unavailable",
            ),
            (
                PlatformWalletError::Sdk(retried(context())),
                "platform.context_unavailable",
            ),
            (
                PlatformWalletError::PersisterRestore(Box::new(PlatformWalletError::WalletLocked)),
                "platform.signer_unavailable",
            ),
            (
                PlatformWalletError::PersisterRestore(Box::new(
                    PlatformWalletError::NoPrimaryIdentity,
                )),
                "storage",
            ),
        ];
        let mut wrong: Vec<String> = cases
            .into_iter()
            .filter_map(|(library, code)| {
                let shown = format!("{library:?}");
                let got = PlatformError::from(library).code();
                (got != code).then(|| format!("{shown}: {got}, want {code}"))
            })
            .collect();
        // The balance refusal keeps its figures through every wrapper.
        let figures = PlatformError::InsufficientCredits {
            needed: 25_018_360_000,
            available: 24_818_360_000,
        };
        for (path, library) in [
            ("token", token(balance_refusal())),
            ("token, retried", token(retried(balance_refusal()))),
            ("sdk", PlatformWalletError::Sdk(balance_refusal())),
        ] {
            let got = PlatformError::from(library);
            if got != figures {
                wrong.push(format!("balance refusal via {path}: {got:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "wrapped errors mapped wrong:\n{}",
            wrong.join("\n")
        );
    }

    #[test]
    fn sdk_errors_map_to_their_platform_codes() {
        use dash_sdk::Error as Sdk;
        let timeout = Sdk::TimeoutReached(std::time::Duration::from_secs(1), "x".into());
        assert_eq!(PlatformError::from(timeout).code(), "platform.timeout");
        let wrapped = PlatformWalletError::Sdk(Sdk::Generic("boom".into()));
        assert_eq!(PlatformError::from(wrapped).code(), "internal");
        let retry = Sdk::NoAvailableAddressesToRetry(Box::new(Sdk::Generic("x".into())));
        assert_eq!(PlatformError::from(retry).code(), "platform.unavailable");
    }
}

#[cfg(test)]
mod deep_envelope_tests {
    use super::*;
    use platform_wallet::PlatformWalletError;

    const DEPTH: usize = 1_000;

    /// Review DW-E0-05-r3-gpt N2: envelopes map without recursing, on a
    /// test thread's default stack; before the fix 128 overflowed it. The
    /// promotion of Platform's balance refusal sees only the leaf.
    #[test]
    fn a_thousand_sdk_retry_envelopes_map_by_their_leaf() {
        let deep = |leaf| {
            (0..DEPTH).fold(leaf, |e, _| {
                dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(e))
            })
        };
        let timeout =
            dash_sdk::Error::TimeoutReached(std::time::Duration::from_secs(1), "x".into());
        assert_eq!(
            PlatformError::from(deep(timeout)).code(),
            "platform.timeout"
        );
        assert_eq!(
            PlatformError::from(deep(super::mapping_tests::balance_refusal())),
            PlatformError::InsufficientCredits {
                needed: 25_018_360_000,
                available: 24_818_360_000,
            }
        );
        let unknown = PlatformError::from(deep(dash_sdk::Error::Generic("leaf".into())));
        assert_eq!(unknown.code(), "platform.unavailable");
    }

    #[test]
    fn a_thousand_persister_restore_envelopes_map_by_their_leaf() {
        let deep = |leaf| {
            (0..DEPTH).fold(leaf, |e, _| {
                PlatformWalletError::PersisterRestore(Box::new(e))
            })
        };
        assert_eq!(
            PlatformError::from(deep(PlatformWalletError::WalletLocked)).code(),
            "platform.signer_unavailable"
        );
        match PlatformError::from(deep(PlatformWalletError::NoPrimaryIdentity)) {
            PlatformError::Storage { detail } => {
                assert!(detail.contains("No primary identity"), "{detail}");
                assert!(detail.len() < 200, "the whole chain was formatted");
            }
            other => panic!("{other:?}"),
        }
        // Both shapes at once: a deep SDK chain inside a deep restore chain.
        let sdk = (0..DEPTH).fold(
            dash_sdk::Error::TimeoutReached(std::time::Duration::from_secs(1), "x".into()),
            |e, _| dash_sdk::Error::NoAvailableAddressesToRetry(Box::new(e)),
        );
        assert_eq!(
            PlatformError::from(deep(PlatformWalletError::Sdk(sdk))).code(),
            "platform.timeout"
        );
    }
}
