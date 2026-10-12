//! The DashPay facade's error domains (DASHPAY §3.6, m4 §4). The strings are
//! the engine's `code()`s; `tests::codes_match_the_engine` holds them equal.

use dw_engine::platform::{
    AvatarError, ContactError, CreditsError, IdentityError, InvitationError, NameError,
    PlatformError, RegistrationError,
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
            P::BroadcastUnknown { artifact } => (
                PlatformCode::BroadcastUnknown.into(),
                Some(ErrorData::Artifact { artifact }),
            ),
            P::WillBeSent { artifact } => (
                PlatformCode::WillBeSent.into(),
                Some(ErrorData::Artifact { artifact }),
            ),
            P::Cancelled => (PlatformCode::Cancelled.into(), None),
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
            P::InvalidArgument { .. } => (EngineCode::InvalidArgument.into(), None),
            P::NetworkNotOpen => (EngineCode::NetworkNotOpen.into(), None),
            P::WalletNotFound => (EngineCode::WalletNotFound.into(), None),
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
            R::Name(e) => return e.into(),
            R::Invitation(e) => return e.into(),
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
                ErrorData::Duffs {
                    needed: Some(needed),
                    available,
                },
            ),
            C::BelowMinimum { min } => (CreditsCode::BelowMinimum, ErrorData::Limit { duffs: min }),
            C::Platform(e) => return e.into(),
        };
        AppError::new(code, message).with(data)
    }
}

#[cfg(test)]
mod tests {
    use dw_engine::platform::{BudgetPurpose, Eligibility, KeyPurpose, RevokeCause, UsernameRule};

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
            PlatformError::InvalidArgument { detail: s() },
            PlatformError::NetworkNotOpen,
            PlatformError::WalletNotFound,
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
            RegistrationError::Name(NameError::Taken),
            RegistrationError::Invitation(InvitationError::Expired),
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
            CreditsError::BelowMinimum { min: 5 },
            CreditsError::Platform(PlatformError::Unavailable),
        ];
        for e in credits {
            assert_eq!(AppError::from(e.clone()).code_str(), e.code());
        }
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
        let e = AppError::from(NameError::Invalid {
            rules: vec![UsernameRule::NoEdgeHyphen],
        });
        assert_eq!(
            serde_json::to_value(&e).unwrap()["data"]["rules"],
            serde_json::json!(["noEdgeHyphen"])
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
    }
}
