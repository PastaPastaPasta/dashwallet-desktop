//! DashPay and Platform glue over platform-wallet (DASHPAY §3.1). The
//! library owns the protocol; this module supplies what it asks the host
//! for: signers over the vault, bring-up order, read models and flows.
//! [`DashPay`] is the facade the UI binding and `dwcli` call; its records
//! live in the domain files and are re-exported here one by one, so a `pub`
//! helper never becomes API by accident.

mod contacts;
mod credits;
mod dashpay;
mod errors;
mod identity;
mod invitations;
pub mod keys_policy;
mod names;
mod notifications;
mod payments;
mod profile;
mod registration;
pub mod signers;
mod startup;
mod status;

pub use contacts::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSectionPage, ContactSort,
    ContactSummary, ContactsPage, Eligibility, PrivateDetails, PublishState, Relation,
    RequestOutcome, ScannedContact,
};
pub use credits::{
    CostTable, TopUpOutcome, TopUpQuote, WithdrawAmount, WithdrawOutcome, WithdrawQuote,
};
pub use dashpay::{BearerSecret, DashPay};
pub use errors::{
    AvatarError, ContactError, CreditsError, IdentityError, InvitationError, NameError,
    PlatformError, RegistrationError,
};
pub use identity::{
    IdentityDetail, IdentityKeyInfo, IdentitySummary, KeyPurpose, KeyType, SecurityLevel,
};
pub use invitations::{InvitationInvalidReason, InvitationStatus};
pub use names::{
    ContestContender, ContestState, ContestStatus, NameAvailability, NameOutcome, UserHit,
    UsernameCheck, UsernameRule, UsernameRuleCheck, check_username,
};
pub use notifications::{DashPayEvent, EventKind, EventPage};
pub use payments::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, Counterparty, LockResolution,
    PaymentLock,
};
pub use profile::{
    AvatarCandidate, AvatarChange, AvatarImage, AvatarSize, AvatarSource, CropRect, Profile,
    ProfileEdit, ProfileLimits,
};
pub use registration::{
    FaucetLockKey, FinishReport, InitialProfile, RegistrationFailure, RegistrationFunding,
    RegistrationPhase, RegistrationQuote, RegistrationRequest, RegistrationStatus,
    RegistrationWait,
};
pub use signers::{VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
pub use startup::{
    DashPayStatus, DashPaySyncStatus, NoIdentityReason, QuorumSource, StartupStatus, SyncLoop,
    SyncLoopStatus, SyncPassReport,
};
pub use status::PlatformStatus;
