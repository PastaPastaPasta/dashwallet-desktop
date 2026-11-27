//! The DashPay facade's error domains (DASHPAY §3.6, m4 §4). The strings are
//! the engine's `code()`s; `tests::codes_match_the_engine` holds them equal.

use dw_engine::platform::{
    AvatarError, ConnectError, ConnectFailure, ContactError, CreditsError, IdentityError,
    InvitationError, NameError, PlatformError, ProfileError, RegistrationError, ShieldedError,
};

use super::{AppError, EngineCode, ErrorData, codes};

codes! {
    PlatformCode {
        Unavailable = "platform.unavailable",
        Timeout = "platform.timeout",
        ProofInvalid = "platform.proof_invalid",
        TrustMismatch = "platform.trust_mismatch",
        ContextUnavailable = "platform.context_unavailable",
        SignerUnavailable = "platform.signer_unavailable",
        SeedMismatch = "platform.seed_mismatch",
        InsufficientCredits = "platform.insufficient_credits",
        GrantInvalid = "platform.grant_invalid",
        GrantExceeded = "platform.grant_exceeded",
        BroadcastUnknown = "platform.broadcast_unknown",
        WillBeSent = "platform.will_be_sent",
        Cancelled = "platform.cancelled",
        Restored = "platform.restored",
        NeedsGrant = "platform.needs_grant",
        LeaseRevoked = "platform.lease_revoked",
        LeaseExpired = "platform.lease_expired",
        FeatureOff = "platform.feature_off",
        NotImplemented = "platform.not_implemented",
    }
}

codes! {
    IdentityCode {
        NotFound = "identity.not_found",
        KeysMissing = "identity.keys_missing",
    }
}

codes! {
    ProfileCode {
        TooLong = "profile.too_long",
        Empty = "profile.empty",
        Conflict = "profile.conflict",
        UnsupportedContract = "profile.unsupported_contract",
    }
}

codes! {
    RegistrationCode {
        InProgress = "registration.in_progress",
        FundingInsufficient = "registration.funding_insufficient",
        CoinsLocked = "registration.coins_locked",
        IslockTimeout = "registration.islock_timeout",
        Recoverable = "registration.recoverable",
        AlreadyHasUsername = "registration.already_has_username",
        Unsupported = "registration.unsupported",
        FundedElsewhere = "registration.funded_elsewhere",
        FundingCancelled = "registration.funding_cancelled",
        NotSynced = "registration.not_synced",
        FundingUnknownRecent = "registration.funding_unknown_recent",
        ShieldedNotReady = "registration.shielded_not_ready",
        QuoteChanged = "registration.quote_changed",
        Refunded = "registration.refunded",
        PaymentConsumed = "registration.payment_consumed",
    }
}

codes! {
    NameCode {
        Invalid = "name.invalid",
        Taken = "name.taken",
        ContestOpen = "name.contest_open",
        Locked = "name.locked",
        UnavailableForInvite = "name.unavailable_for_invite",
    }
}

codes! {
    ContactCode {
        Ineligible = "contact.ineligible",
        AlreadyContact = "contact.already_contact",
        RequestPending = "contact.request_pending",
        IsSelf = "contact.self",
        ChannelBroken = "contact.channel_broken",
        PaymentLocked = "contact.payment_locked",
        ScanExpired = "contact.scan_expired",
        ScanUnverified = "contact.scan_unverified",
        NoUsername = "contact.no_username",
        DetailsTooLong = "contact.details_too_long",
    }
}

codes! {
    InvitationCode {
        Invalid = "invitation.invalid",
        Claimed = "invitation.claimed",
        Expired = "invitation.expired",
        AlreadyHasIdentity = "invitation.already_has_identity",
    }
}

codes! {
    AvatarCode {
        TooLarge = "avatar.too_large",
        Unsupported = "avatar.unsupported",
        FetchFailed = "avatar.fetch_failed",
        HashMismatch = "avatar.hash_mismatch",
        UploadUnconfigured = "avatar.upload_unconfigured",
    }
}

codes! {
    CreditsCode {
        FundingInsufficient = "credits.funding_insufficient",
        BelowMinimum = "credits.below_minimum",
        CoinsLocked = "credits.coins_locked",
        TopUpPending = "credits.top_up_pending",
        WithdrawalOutOfRange = "credits.withdrawal_out_of_range",
        WithdrawalLanded = "credits.withdrawal_landed",
        MovePending = "credits.move_pending",
        RecipientInvalid = "credits.recipient_invalid",
        SendOutOfRange = "credits.send_out_of_range",
        QuoteChanged = "credits.quote_changed",
        FundPending = "credits.fund_pending",
        TooManyInputs = "credits.too_many_inputs",
        CoinsChanged = "credits.coins_changed",
        MixingActive = "credits.mixing_active",
        MoveRunning = "credits.move_running",
        PreflightRefused = "credits.preflight_refused",
        RulesUnsupported = "credits.rules_unsupported",
    }
}

codes! {
    ShieldedCode {
        NotActive = "shielded.not_active",
        NotSynced = "shielded.not_synced",
        InvalidAddress = "shielded.invalid_address",
        InsufficientFunds = "shielded.insufficient_funds",
        FundingInsufficient = "shielded.funding_insufficient",
        BelowMinimum = "shielded.below_minimum",
        CoinsLocked = "shielded.coins_locked",
        ShieldPending = "shielded.shield_pending",
        PoolTooSmall = "shielded.pool_too_small",
        TooManyInputs = "shielded.too_many_inputs",
        UnrecordedSpend = "shielded.unrecorded_spend",
        SpendRefused = "shielded.spend_refused",
        MixingActive = "shielded.mixing_active",
        NothingToMove = "shielded.nothing_to_move",
        CoinsUnconfirmed = "shielded.coins_unconfirmed",
        MoveRunning = "shielded.move_running",
        CoinsChanged = "shielded.coins_changed",
        StoreClosed = "shielded.store_closed",
    }
}

impl From<ShieldedError> for AppError {
    fn from(e: ShieldedError) -> Self {
        use ShieldedError as S;
        let message = e.to_string();
        let (code, data) = match e {
            S::NotActive {
                protocol_version,
                activation_version,
            } => (
                ShieldedCode::NotActive,
                Some(ErrorData::Protocol {
                    protocol_version,
                    activation_version,
                }),
            ),
            S::NotSynced { reason } => (
                ShieldedCode::NotSynced,
                Some(ErrorData::Reason {
                    reason: reason.as_str().into(),
                }),
            ),
            S::InvalidAddress { detail } => (
                ShieldedCode::InvalidAddress,
                Some(ErrorData::Reason { reason: detail }),
            ),
            S::InsufficientFunds { needed, available } => (
                ShieldedCode::InsufficientFunds,
                Some(ErrorData::Credits { needed, available }),
            ),
            S::FundingInsufficient { needed, available } => (
                ShieldedCode::FundingInsufficient,
                Some(ErrorData::Duffs {
                    needed: Some(needed),
                    available,
                }),
            ),
            S::BelowMinimum { min, max } => (
                ShieldedCode::BelowMinimum,
                Some(match max {
                    Some(m) => ErrorData::MaxMinimum {
                        min,
                        duffs: m.duffs,
                        left_duffs: m.left_duffs,
                        arriving_duffs: m.arriving_duffs,
                        arriving_clears: m.arriving_clears,
                    },
                    None => ErrorData::Number { value: min },
                }),
            ),
            S::CoinsLocked => (ShieldedCode::CoinsLocked, None),
            S::ShieldPending { txid } => {
                (ShieldedCode::ShieldPending, Some(ErrorData::Txid { txid }))
            }
            S::PoolTooSmall { notes, required } => (
                ShieldedCode::PoolTooSmall,
                Some(ErrorData::PoolNotes { notes, required }),
            ),
            S::TooManyInputs { inputs, .. } => (
                ShieldedCode::TooManyInputs,
                Some(ErrorData::Number { value: inputs }),
            ),
            S::UnrecordedSpend => (ShieldedCode::UnrecordedSpend, None),
            S::SpendRefused => (ShieldedCode::SpendRefused, None),
            S::MixingActive => (ShieldedCode::MixingActive, None),
            S::NothingToMove => (ShieldedCode::NothingToMove, None),
            S::CoinsUnconfirmed { coins } => (
                ShieldedCode::CoinsUnconfirmed,
                Some(ErrorData::Number { value: coins }),
            ),
            S::MoveRunning => (ShieldedCode::MoveRunning, None),
            S::CoinsChanged => (ShieldedCode::CoinsChanged, None),
            S::StoreClosed { reason } => (
                ShieldedCode::StoreClosed,
                Some(ErrorData::Reason {
                    reason: reason.as_str().into(),
                }),
            ),
            S::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

impl From<PlatformError> for AppError {
    fn from(e: PlatformError) -> Self {
        use PlatformError as P;
        let message = e.to_string();
        let (code, data): (super::ErrorCode, _) = match e {
            P::Unavailable => (PlatformCode::Unavailable.into(), None),
            P::Timeout => (PlatformCode::Timeout.into(), None),
            P::ProofInvalid => (PlatformCode::ProofInvalid.into(), None),
            P::TrustMismatch => (PlatformCode::TrustMismatch.into(), None),
            P::ContextUnavailable => (PlatformCode::ContextUnavailable.into(), None),
            P::SignerUnavailable => (PlatformCode::SignerUnavailable.into(), None),
            P::SeedMismatch => (PlatformCode::SeedMismatch.into(), None),
            P::InsufficientCredits { needed, available } => (
                PlatformCode::InsufficientCredits.into(),
                Some(ErrorData::Credits { needed, available }),
            ),
            P::GrantInvalid => (PlatformCode::GrantInvalid.into(), None),
            P::GrantExceeded {
                purpose,
                needed,
                remaining,
            } => (
                PlatformCode::GrantExceeded.into(),
                Some(ErrorData::Budget {
                    purpose: purpose.into(),
                    needed,
                    remaining,
                }),
            ),
            // No artifact when the SDK named none: nothing to look up.
            P::BroadcastUnknown { artifact } => (
                PlatformCode::BroadcastUnknown.into(),
                (!artifact.is_empty()).then_some(ErrorData::Artifact { artifact }),
            ),
            P::WillBeSent { artifact } => (
                PlatformCode::WillBeSent.into(),
                Some(ErrorData::Artifact { artifact }),
            ),
            P::Cancelled => (PlatformCode::Cancelled.into(), None),
            P::Restored => (PlatformCode::Restored.into(), None),
            P::NeedsGrant { purpose } => (
                PlatformCode::NeedsGrant.into(),
                Some(ErrorData::NeedsGrant {
                    purpose: purpose.into(),
                }),
            ),
            P::LeaseRevoked { cause } => (
                PlatformCode::LeaseRevoked.into(),
                Some(ErrorData::Revoked {
                    cause: cause.into(),
                }),
            ),
            P::LeaseExpired => (PlatformCode::LeaseExpired.into(), None),
            P::FeatureOff { feature } => (
                PlatformCode::FeatureOff.into(),
                Some(ErrorData::Feature { feature }),
            ),
            P::NotImplemented { call } => (
                PlatformCode::NotImplemented.into(),
                Some(ErrorData::Call { call }),
            ),
            P::Identity(IdentityError::NotFound) => (IdentityCode::NotFound.into(), None),
            P::Identity(IdentityError::KeysMissing { purpose }) => (
                IdentityCode::KeysMissing.into(),
                Some(ErrorData::KeysMissing {
                    purpose: purpose.into(),
                }),
            ),
            P::Profile(ProfileError::TooLong { field, length, max }) => (
                ProfileCode::TooLong.into(),
                Some(ErrorData::ProfileTooLong {
                    field: field.into(),
                    length,
                    max,
                }),
            ),
            P::Profile(ProfileError::Empty) => (ProfileCode::Empty.into(), None),
            P::Profile(ProfileError::Conflict { current }) => (
                ProfileCode::Conflict.into(),
                Some(ErrorData::ProfileConflict {
                    current: current.map(|p| Box::new((*p).into())),
                }),
            ),
            P::Profile(ProfileError::UnsupportedContract) => {
                (ProfileCode::UnsupportedContract.into(), None)
            }
            P::InvalidArgument { .. } => (EngineCode::InvalidArgument.into(), None),
            P::NetworkNotOpen => (EngineCode::NetworkNotOpen.into(), None),
            P::WalletNotFound => (EngineCode::WalletNotFound.into(), None),
            P::RescanInterrupted => (EngineCode::RescanInterrupted.into(), None),
            P::Storage { .. } => (EngineCode::Storage.into(), None),
            P::Internal { .. } => (EngineCode::Internal.into(), None),
        };
        AppError {
            code,
            message,
            data,
        }
    }
}

impl From<RegistrationError> for AppError {
    fn from(e: RegistrationError) -> Self {
        use RegistrationError as R;
        let message = e.to_string();
        let (code, data) = match e {
            R::InProgress { draft } => (
                RegistrationCode::InProgress,
                Some(ErrorData::Draft { draft }),
            ),
            R::FundingInsufficient { needed, available } => (
                RegistrationCode::FundingInsufficient,
                Some(ErrorData::Duffs {
                    needed: Some(needed),
                    available,
                }),
            ),
            R::CoinsLocked => (RegistrationCode::CoinsLocked, None),
            R::IslockTimeout => (RegistrationCode::IslockTimeout, None),
            R::Recoverable { draft } => (
                RegistrationCode::Recoverable,
                Some(ErrorData::Draft { draft }),
            ),
            R::AlreadyHasUsername => (RegistrationCode::AlreadyHasUsername, None),
            R::Unsupported => (RegistrationCode::Unsupported, None),
            R::FundedElsewhere => (RegistrationCode::FundedElsewhere, None),
            R::FundingCancelled => (RegistrationCode::FundingCancelled, None),
            R::NotSynced => (RegistrationCode::NotSynced, None),
            R::FundingUnknownRecent { draft, wait_secs } => (
                RegistrationCode::FundingUnknownRecent,
                Some(ErrorData::DraftWait { draft, wait_secs }),
            ),
            R::ShieldedNotReady { reason, ready_at } => (
                RegistrationCode::ShieldedNotReady,
                Some(ErrorData::ShieldedNotReady {
                    reason: reason.into(),
                    ready_at,
                }),
            ),
            R::QuoteChanged => (RegistrationCode::QuoteChanged, None),
            R::Refunded => (RegistrationCode::Refunded, None),
            R::PaymentConsumed => (RegistrationCode::PaymentConsumed, None),
            R::Name(e) => return e.into(),
            R::Invitation(e) => return e.into(),
            R::Credits(e) => return e.into(),
            R::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

impl From<NameError> for AppError {
    fn from(e: NameError) -> Self {
        use NameError as N;
        let message = e.to_string();
        let (code, data) = match e {
            N::Invalid { rules } => (
                NameCode::Invalid,
                Some(ErrorData::UsernameRules {
                    rules: rules.into_iter().map(Into::into).collect(),
                }),
            ),
            N::Taken => (NameCode::Taken, None),
            N::ContestOpen => (NameCode::ContestOpen, None),
            N::Locked => (NameCode::Locked, None),
            N::UnavailableForInvite => (NameCode::UnavailableForInvite, None),
            N::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

impl From<ContactError> for AppError {
    fn from(e: ContactError) -> Self {
        use ContactError as C;
        let message = e.to_string();
        let (code, data) = match e {
            C::Ineligible { reason } => (
                ContactCode::Ineligible,
                Some(ErrorData::Ineligible {
                    reason: reason.into(),
                }),
            ),
            C::AlreadyContact => (ContactCode::AlreadyContact, None),
            C::RequestPending => (ContactCode::RequestPending, None),
            C::IsSelf => (ContactCode::IsSelf, None),
            C::ChannelBroken => (ContactCode::ChannelBroken, None),
            C::PaymentLocked { txid } => {
                (ContactCode::PaymentLocked, Some(ErrorData::Txid { txid }))
            }
            C::ScanExpired => (ContactCode::ScanExpired, None),
            C::ScanUnverified => (ContactCode::ScanUnverified, None),
            C::NoUsername => (ContactCode::NoUsername, None),
            C::DetailsTooLong { bytes, max } => (
                ContactCode::DetailsTooLong,
                Some(ErrorData::Size { bytes, max }),
            ),
            C::Shielded(e) => return e.into(),
            C::Credits(e) => return (*e).into(),
            C::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

impl From<InvitationError> for AppError {
    fn from(e: InvitationError) -> Self {
        use InvitationError as I;
        let message = e.to_string();
        let code = match e {
            I::Invalid => InvitationCode::Invalid,
            I::Claimed => InvitationCode::Claimed,
            I::Expired => InvitationCode::Expired,
            I::AlreadyHasIdentity => InvitationCode::AlreadyHasIdentity,
            I::Platform(e) => return e.into(),
        };
        AppError::new(code, message)
    }
}

codes! {
    ConnectCode {
        Network = "connect.network",
        MainnetOff = "connect.mainnet_off",
        Unsupported = "connect.unsupported",
        Invalid = "connect.invalid",
        Stale = "connect.stale",
        Protocol = "connect.protocol",
        NoIdentity = "connect.no_identity",
        Hint = "connect.hint",
        Owner = "connect.owner",
        Answered = "connect.answered",
        Credits = "connect.credits",
        PlanChanged = "connect.plan_changed",
        SendFailed = "connect.send_failed",
        Failed = "connect.failed",
    }
}

/// Why an approved App Connect login stopped (`connect.failed`), from the
/// engine's fixed list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum ConnectFailureName {
    KeyConflict,
    NotConfirmed,
    NoResponseKey,
    NothingToSend,
    ScopeChanged,
}

/// The network an App Connect request is for (`connect.network`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
pub enum ConnectNetworkName {
    Mainnet,
    Testnet,
    Devnet,
}

impl From<dw_uri::connect::NetworkSel> for ConnectNetworkName {
    fn from(n: dw_uri::connect::NetworkSel) -> Self {
        match n {
            dw_uri::connect::NetworkSel::Mainnet => Self::Mainnet,
            dw_uri::connect::NetworkSel::Testnet => Self::Testnet,
            dw_uri::connect::NetworkSel::Devnet => Self::Devnet,
        }
    }
}

impl From<ConnectFailure> for ConnectFailureName {
    fn from(f: ConnectFailure) -> Self {
        match f {
            ConnectFailure::KeyConflict => Self::KeyConflict,
            ConnectFailure::NotConfirmed => Self::NotConfirmed,
            ConnectFailure::NoResponseKey => Self::NoResponseKey,
            ConnectFailure::NothingToSend => Self::NothingToSend,
            ConnectFailure::ScopeChanged => Self::ScopeChanged,
        }
    }
}

impl From<ConnectError> for AppError {
    fn from(e: ConnectError) -> Self {
        use ConnectError as C;
        let message = e.code().to_owned();
        let (code, data) = match e {
            C::Network => (ConnectCode::Network, None),
            C::MainnetOff => (ConnectCode::MainnetOff, None),
            C::Unsupported => (ConnectCode::Unsupported, None),
            C::Invalid => (ConnectCode::Invalid, None),
            C::Stale => (ConnectCode::Stale, None),
            C::Protocol => (ConnectCode::Protocol, None),
            C::NoIdentity => (ConnectCode::NoIdentity, None),
            C::Hint => (ConnectCode::Hint, None),
            C::Owner => (ConnectCode::Owner, None),
            C::Answered => (ConnectCode::Answered, None),
            C::Credits { needed, available } => (
                ConnectCode::Credits,
                Some(ErrorData::Credits { needed, available }),
            ),
            C::PlanChanged => (ConnectCode::PlanChanged, None),
            C::SendFailed => (ConnectCode::SendFailed, None),
            C::Failed { failure } => (
                ConnectCode::Failed,
                Some(ErrorData::ConnectFailure {
                    failure: failure.into(),
                }),
            ),
            C::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

impl From<AvatarError> for AppError {
    fn from(e: AvatarError) -> Self {
        use AvatarError as A;
        let message = e.to_string();
        let code = match e {
            A::TooLarge => AvatarCode::TooLarge,
            A::Unsupported => AvatarCode::Unsupported,
            A::FetchFailed => AvatarCode::FetchFailed,
            A::HashMismatch => AvatarCode::HashMismatch,
            A::UploadUnconfigured => AvatarCode::UploadUnconfigured,
            A::Platform(e) => return e.into(),
        };
        AppError::new(code, message)
    }
}

impl From<CreditsError> for AppError {
    fn from(e: CreditsError) -> Self {
        use CreditsError as C;
        let message = e.to_string();
        let (code, data) = match e {
            C::FundingInsufficient { needed, available } => (
                CreditsCode::FundingInsufficient,
                Some(ErrorData::Duffs {
                    needed: Some(needed),
                    available,
                }),
            ),
            C::BelowMinimum { min, max } => (
                CreditsCode::BelowMinimum,
                Some(match max {
                    Some(m) => ErrorData::MaxMinimum {
                        min,
                        duffs: m.duffs,
                        left_duffs: m.left_duffs,
                        arriving_duffs: m.arriving_duffs,
                        arriving_clears: m.arriving_clears,
                    },
                    None => ErrorData::Limit { duffs: min },
                }),
            ),
            C::CoinsLocked => (CreditsCode::CoinsLocked, None),
            C::TopUpPending { txid } => (CreditsCode::TopUpPending, Some(ErrorData::Txid { txid })),
            C::WithdrawalOutOfRange { min, max } => (
                CreditsCode::WithdrawalOutOfRange,
                Some(ErrorData::CreditRange { min, max }),
            ),
            C::WithdrawalLanded { artifact } => (
                CreditsCode::WithdrawalLanded,
                Some(ErrorData::Artifact { artifact }),
            ),
            C::MovePending { id } => (
                CreditsCode::MovePending,
                Some(ErrorData::Artifact { artifact: id }),
            ),
            C::RecipientInvalid { reason } => (
                CreditsCode::RecipientInvalid,
                Some(ErrorData::Reason {
                    reason: reason.as_str().into(),
                }),
            ),
            C::SendOutOfRange { min } => (
                CreditsCode::SendOutOfRange,
                Some(ErrorData::CreditMinimum { min }),
            ),
            C::QuoteChanged => (CreditsCode::QuoteChanged, None),
            C::FundPending { txid } => (CreditsCode::FundPending, Some(ErrorData::Txid { txid })),
            C::TooManyInputs { inputs, .. } => (
                CreditsCode::TooManyInputs,
                Some(ErrorData::Number { value: inputs }),
            ),
            C::CoinsChanged => (CreditsCode::CoinsChanged, None),
            C::MixingActive => (CreditsCode::MixingActive, None),
            C::MoveRunning => (CreditsCode::MoveRunning, None),
            C::PreflightRefused => (CreditsCode::PreflightRefused, None),
            C::RulesUnsupported => (CreditsCode::RulesUnsupported, None),
            C::Shielded(e) => return e.into(),
            C::Platform(e) => return e.into(),
        };
        AppError {
            code: code.into(),
            message,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::platform::{
        BudgetPurpose, Eligibility, KeyPurpose, ProfileField, RecipientProblem, RevokeCause,
        ShieldedCheck, ShieldedStoreClosed, ShieldedSyncWait, UsernameRule,
    };

    use super::*;

    /// Every variant's code equals the engine's `code()`: the strings here
    /// are copies of the contract's, and must not drift from it.
    #[test]
    fn codes_match_the_engine() {
        let s = || String::from("x");
        let platform = [
            PlatformError::Unavailable,
            PlatformError::Timeout,
            PlatformError::ProofInvalid,
            PlatformError::TrustMismatch,
            PlatformError::ContextUnavailable,
            PlatformError::SignerUnavailable,
            PlatformError::SeedMismatch,
            PlatformError::InsufficientCredits {
                needed: 2,
                available: 1,
            },
            PlatformError::GrantInvalid,
            PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Credits,
                needed: 2,
                remaining: 1,
            },
            PlatformError::BroadcastUnknown { artifact: s() },
            PlatformError::WillBeSent { artifact: s() },
            PlatformError::Cancelled,
            PlatformError::Restored,
            PlatformError::NeedsGrant {
                purpose: BudgetPurpose::Spend,
            },
            PlatformError::LeaseRevoked {
                cause: RevokeCause::Lock,
            },
            PlatformError::LeaseExpired,
            PlatformError::FeatureOff { feature: s() },
            PlatformError::NotImplemented { call: s() },
            PlatformError::Identity(IdentityError::NotFound),
            PlatformError::Identity(IdentityError::KeysMissing {
                purpose: KeyPurpose::Encryption,
            }),
            PlatformError::Profile(ProfileError::TooLong {
                field: ProfileField::PublicMessage,
                length: 141,
                max: 140,
            }),
            PlatformError::Profile(ProfileError::Empty),
            PlatformError::Profile(ProfileError::Conflict { current: None }),
            PlatformError::Profile(ProfileError::UnsupportedContract),
            PlatformError::InvalidArgument { detail: s() },
            PlatformError::NetworkNotOpen,
            PlatformError::WalletNotFound,
            PlatformError::RescanInterrupted,
            PlatformError::Storage { detail: s() },
            PlatformError::Internal { detail: s() },
        ];
        for e in platform {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let registration = [
            RegistrationError::InProgress { draft: s() },
            RegistrationError::FundingInsufficient {
                needed: 2,
                available: 1,
            },
            RegistrationError::CoinsLocked,
            RegistrationError::IslockTimeout,
            RegistrationError::Recoverable { draft: s() },
            RegistrationError::AlreadyHasUsername,
            RegistrationError::Unsupported,
            RegistrationError::FundedElsewhere,
            RegistrationError::FundingCancelled,
            RegistrationError::NotSynced,
            RegistrationError::FundingUnknownRecent {
                draft: s(),
                wait_secs: Some(1),
            },
            RegistrationError::ShieldedNotReady {
                reason: ShieldedCheck::Rested,
                ready_at: Some(1),
            },
            RegistrationError::QuoteChanged,
            RegistrationError::Refunded,
            RegistrationError::PaymentConsumed,
            RegistrationError::Name(NameError::Taken),
            RegistrationError::Invitation(InvitationError::Expired),
            RegistrationError::Credits(CreditsError::MovePending { id: s() }),
            RegistrationError::Platform(PlatformError::Timeout),
        ];
        for e in registration {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let names = [
            NameError::Invalid {
                rules: vec![UsernameRule::MinLength],
            },
            NameError::Taken,
            NameError::ContestOpen,
            NameError::Locked,
            NameError::UnavailableForInvite,
            NameError::Platform(PlatformError::Unavailable),
        ];
        for e in names {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let contacts = [
            ContactError::Ineligible {
                reason: Eligibility::NoDashPayKeys,
            },
            ContactError::AlreadyContact,
            ContactError::RequestPending,
            ContactError::IsSelf,
            ContactError::ChannelBroken,
            ContactError::PaymentLocked { txid: s() },
            ContactError::ScanExpired,
            ContactError::ScanUnverified,
            ContactError::NoUsername,
            ContactError::DetailsTooLong {
                bytes: 2032,
                max: 2031,
            },
            ContactError::Credits(Box::new(CreditsError::MovePending { id: s() })),
            ContactError::Platform(PlatformError::Unavailable),
        ];
        for e in contacts {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let invitations = [
            InvitationError::Invalid,
            InvitationError::Claimed,
            InvitationError::Expired,
            InvitationError::AlreadyHasIdentity,
            InvitationError::Platform(PlatformError::Unavailable),
        ];
        for e in invitations {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let avatars = [
            AvatarError::TooLarge,
            AvatarError::Unsupported,
            AvatarError::FetchFailed,
            AvatarError::HashMismatch,
            AvatarError::UploadUnconfigured,
            AvatarError::Platform(PlatformError::Unavailable),
        ];
        for e in avatars {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let credits = [
            CreditsError::FundingInsufficient {
                needed: 2,
                available: 1,
            },
            CreditsError::BelowMinimum { min: 5, max: None },
            CreditsError::CoinsLocked,
            CreditsError::TopUpPending { txid: s() },
            CreditsError::WithdrawalOutOfRange { min: 1, max: 2 },
            CreditsError::WithdrawalLanded { artifact: s() },
            CreditsError::MovePending { id: s() },
            CreditsError::RecipientInvalid {
                reason: RecipientProblem::Network,
            },
            CreditsError::SendOutOfRange { min: 1 },
            CreditsError::QuoteChanged,
            CreditsError::FundPending { txid: s() },
            CreditsError::TooManyInputs {
                inputs: 51,
                max: 50,
            },
            CreditsError::CoinsChanged,
            CreditsError::MixingActive,
            CreditsError::MoveRunning,
            CreditsError::PreflightRefused,
            CreditsError::RulesUnsupported,
            CreditsError::Shielded(ShieldedError::NotActive {
                protocol_version: 11,
                activation_version: 12,
            }),
            CreditsError::Platform(PlatformError::Unavailable),
        ];
        for e in credits {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let shielded = [
            ShieldedError::NotActive {
                protocol_version: 11,
                activation_version: 12,
            },
            ShieldedError::NotSynced {
                reason: ShieldedSyncWait::Journal,
            },
            ShieldedError::InvalidAddress { detail: s() },
            ShieldedError::InsufficientFunds {
                needed: 2,
                available: 1,
            },
            ShieldedError::FundingInsufficient {
                needed: 2,
                available: 1,
            },
            ShieldedError::BelowMinimum { min: 5, max: None },
            ShieldedError::CoinsLocked,
            ShieldedError::ShieldPending { txid: s() },
            ShieldedError::PoolTooSmall {
                notes: 10,
                required: 250,
            },
            ShieldedError::TooManyInputs {
                inputs: 51,
                max: 50,
            },
            ShieldedError::UnrecordedSpend,
            ShieldedError::SpendRefused,
            ShieldedError::MixingActive,
            ShieldedError::NothingToMove,
            ShieldedError::CoinsUnconfirmed { coins: 2 },
            ShieldedError::MoveRunning,
            ShieldedError::CoinsChanged,
            ShieldedError::StoreClosed {
                reason: ShieldedStoreClosed::NoUpgradeCopy,
            },
            ShieldedError::Platform(PlatformError::Unavailable),
        ];
        for e in shielded {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
        let connect = [
            ConnectError::Network,
            ConnectError::MainnetOff,
            ConnectError::Unsupported,
            ConnectError::Invalid,
            ConnectError::Stale,
            ConnectError::Protocol,
            ConnectError::NoIdentity,
            ConnectError::Hint,
            ConnectError::Owner,
            ConnectError::Answered,
            ConnectError::Credits {
                needed: 2,
                available: 1,
            },
            ConnectError::PlanChanged,
            ConnectError::SendFailed,
            ConnectError::Failed {
                failure: dw_engine::platform::ConnectFailure::KeyConflict,
            },
        ];
        assert_eq!(connect.len(), ConnectCode::ALL.len());
        for e in connect {
            let code = e.code();
            assert_eq!(AppError::from(e).code_str(), code);
        }
        assert_eq!(
            AppError::from(ConnectError::Platform(PlatformError::Timeout)).code_str(),
            "platform.timeout"
        );
    }

    #[test]
    fn fields_ride_in_data() {
        let e = AppError::from(PlatformError::GrantExceeded {
            purpose: BudgetPurpose::Credits,
            needed: 30_000_000_000_000_000,
            remaining: 7,
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({
                "kind": "budget",
                "purpose": "credits",
                "needed": "30000000000000000",
                "remaining": "7"
            })
        );
        // An unknown outcome with no artifact carries no data to look up.
        let e = AppError::from(PlatformError::BroadcastUnknown {
            artifact: String::new(),
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            (&v["code"], &v["data"]),
            (
                &serde_json::json!("platform.broadcast_unknown"),
                &serde_json::Value::Null
            )
        );
        let e = AppError::from(PlatformError::BroadcastUnknown {
            artifact: "st".into(),
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({"kind": "artifact", "artifact": "st"})
        );
        // Q110 110-2: why the shielded store stays closed.
        let e = AppError::from(ShieldedError::StoreClosed {
            reason: ShieldedStoreClosed::InUse,
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "shielded.store_closed");
        assert_eq!(
            v["data"],
            serde_json::json!({"kind": "reason", "reason": "in_use"})
        );
        // DEC-1058 #4: what Shielded is still syncing.
        let e = AppError::from(ShieldedError::NotSynced {
            reason: ShieldedSyncWait::Core,
        });
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "shielded.not_synced");
        assert_eq!(
            v["data"],
            serde_json::json!({"kind": "reason", "reason": "core"})
        );
        let e = AppError::from(NameError::Invalid {
            rules: vec![UsernameRule::NoEdgeHyphen],
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"]["rules"],
            serde_json::json!(["noEdgeHyphen"])
        );
        let e = AppError::from(PlatformError::Profile(ProfileError::TooLong {
            field: ProfileField::DisplayName,
            length: 26,
            max: 25,
        }));
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({
                "kind": "profileTooLong",
                "field": "displayName",
                "length": 26,
                "max": 25
            })
        );
        let current = dw_engine::platform::Profile {
            display_name: Some("Al".into()),
            public_message: None,
            avatar_url: None,
            avatar_hash: None,
            avatar_fingerprint: None,
            updated_at: None,
        };
        let e = AppError::from(PlatformError::Profile(ProfileError::Conflict {
            current: Some(Box::new(current)),
        }));
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({
                "kind": "profileConflict",
                "current": {
                    "displayName": "Al",
                    "publicMessage": null,
                    "avatarUrl": null,
                    "avatarHash": null,
                    "avatarFingerprint": null,
                    "updatedAt": null
                }
            })
        );
        let e = AppError::from(RegistrationError::InProgress {
            draft: "registration/3/funding".into(),
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({ "kind": "draft", "draft": "registration/3/funding" })
        );
        let e = AppError::from(RegistrationError::FundingUnknownRecent {
            draft: "d".into(),
            wait_secs: Some(600),
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({ "kind": "draftWait", "draft": "d", "waitSecs": 600 })
        );
        let e = AppError::from(RegistrationError::ShieldedNotReady {
            reason: ShieldedCheck::Rested,
            ready_at: Some(10_800),
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"],
            serde_json::json!({ "kind": "shieldedNotReady", "reason": "rested", "readyAt": 10_800 })
        );
        let short = Some(dw_engine::platform::CoreMaxShort {
            duffs: 1_200,
            left_duffs: 40_000,
            arriving_duffs: 5_000,
            arriving_clears: true,
        });
        let max = serde_json::json!({
            "kind": "maxMinimum", "min": 30_000, "duffs": 1_200, "leftDuffs": 40_000,
            "arrivingDuffs": 5_000, "arrivingClears": true
        });
        let below = [
            (
                AppError::from(CreditsError::BelowMinimum {
                    min: 30_000,
                    max: short,
                }),
                max.clone(),
            ),
            (
                AppError::from(ShieldedError::BelowMinimum {
                    min: 30_000,
                    max: short,
                }),
                max,
            ),
            (
                AppError::from(CreditsError::BelowMinimum {
                    min: 30_000,
                    max: None,
                }),
                serde_json::json!({ "kind": "limit", "duffs": 30_000 }),
            ),
            (
                AppError::from(ShieldedError::BelowMinimum {
                    min: 30_000,
                    max: None,
                }),
                serde_json::json!({ "kind": "number", "value": 30_000 }),
            ),
        ];
        for (e, data) in below {
            assert_eq!(serde_json::to_value(&e).unwrap()["data"], data);
        }
    }

    /// dw-engine writes the `data` of the errors in the accept contract vector by hand
    /// (`requests_tests.rs`, `accept_vectors::data`): every error the vector records must be
    /// this crate's conversion of one of these engine errors, and each of them is recorded.
    #[test]
    fn accept_vector_errors_are_dw_apps() {
        use serde_json::{Value, json};
        let quote = 150_000_000;
        let engine: [AppError; 11] = [
            PlatformError::GrantInvalid.into(),
            PlatformError::LeaseExpired.into(),
            PlatformError::SignerUnavailable.into(),
            PlatformError::LeaseRevoked {
                cause: RevokeCause::Lock,
            }
            .into(),
            PlatformError::InvalidArgument { detail: "x".into() }.into(),
            PlatformError::InsufficientCredits {
                needed: quote,
                available: quote - 1,
            }
            .into(),
            PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Credits,
                needed: quote,
                remaining: quote - 1,
            }
            .into(),
            PlatformError::NeedsGrant {
                purpose: BudgetPurpose::Credits,
            }
            .into(),
            ContactError::IsSelf.into(),
            ContactError::AlreadyContact.into(),
            ContactError::Ineligible {
                reason: Eligibility::NoDashPayKeys,
            }
            .into(),
        ];
        let converted: Vec<Value> = (engine.iter())
            .map(|e| {
                let v = serde_json::to_value(e).unwrap();
                json!({ "code": v["code"], "data": v["data"] })
            })
            .collect();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/desktop/src/fixture/vectors/accept.json");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let recorded: Vec<&Value> = (doc["cases"].as_array().unwrap().iter())
            .flat_map(|c| c["results"].as_array().unwrap())
            .filter_map(|r| r.get("error"))
            .collect();
        for e in &recorded {
            assert!(
                converted.contains(e),
                "{e} in {} is not dw-app's conversion of any listed engine error: fix \
                 `accept_vectors::data`, or list the new error here",
                path.display()
            );
        }
        for e in &converted {
            assert!(
                recorded.contains(&e),
                "{e} is not in {}: drop it here",
                path.display()
            );
        }
    }

    /// dw-engine writes the vector's leases by hand too (`accept_vectors::lease_json`): each
    /// of these views, as dw-app's `Lease` less `id`, `walletId` and `keyExpiresInSecs`, is
    /// recorded there.
    #[test]
    fn accept_vector_leases_are_dw_apps() {
        use dw_engine::platform::{BudgetView, FlowKind, LeaseStateView, LeaseView};
        use serde_json::Value;
        let quote = 150_000_000;
        let view = |state, own_key, spent| LeaseView {
            id: "1ea50001".into(),
            wallet_id: "a1".repeat(32),
            flow: FlowKind::Accept,
            state,
            own_key,
            key_expires_in_secs: None,
            funds_committed: false,
            budgets: vec![BudgetView {
                purpose: BudgetPurpose::Credits,
                ceiling: quote,
                spent,
            }],
            in_flight: 0,
            call_running: false,
        };
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../apps/desktop/src/fixture/vectors/accept.json");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let recorded: Vec<&Value> = (doc["cases"].as_array().unwrap().iter())
            .flat_map(|c| c["results"].as_array().unwrap())
            .filter_map(|r| r["ok"].as_array())
            .flatten()
            .collect();
        let revoked = LeaseStateView::Revoked {
            cause: RevokeCause::Lock,
        };
        for v in [
            view(LeaseStateView::Ended, true, quote),
            view(LeaseStateView::Active, false, 0),
            view(LeaseStateView::NeedsGrant, true, 0),
            view(revoked, false, 0),
            view(revoked, true, 0),
        ] {
            let mut lease = serde_json::to_value(crate::dto::platform::Lease::from(v)).unwrap();
            for k in ["id", "walletId", "keyExpiresInSecs"] {
                lease.as_object_mut().unwrap().remove(k);
            }
            assert!(
                recorded.contains(&&lease),
                "{lease} is not in {}: fix `accept_vectors::lease_json`",
                path.display()
            );
        }
    }
}
