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
    /// The lease the call named has ended: `end_flow`, the idle reaper, or
    /// its own key's expiry.
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
