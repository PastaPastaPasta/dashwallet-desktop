//! DashPay and Platform glue over platform-wallet (DASHPAY §3.1). The
//! library owns the protocol; this module supplies what it asks the host
//! for: signers over the vault, bring-up order, read models and flows.
//! [`DashPay`] is the facade the UI binding and `dwcli` call; its records
//! live in the domain files and are re-exported here one by one, so a `pub`
//! helper never becomes API by accident.

pub(crate) mod attribution;
mod balances;
mod banner;
#[cfg(test)]
mod banner_tests;
mod bringup;
mod contact_names;
mod contacts;
pub(crate) mod contacts_model;
#[cfg(test)]
mod contacts_tests;
mod contest_watch;
#[cfg(test)]
mod contest_watch_tests;
mod credit_costs;
mod credits;
mod dashpay;
mod errors;
mod flows;
mod identity;
mod identity_writes;
mod invitations;
pub(crate) mod journal;
#[cfg(test)]
mod journal_tests;
pub mod keys_policy;
pub mod lease;
pub(crate) mod money_gate;
#[cfg(test)]
pub(crate) mod money_gate_tests;
mod names;
mod names_check;
mod notifications;
#[cfg(test)]
mod notifications_tests;
pub(crate) mod payment_locks;
mod payments;
#[cfg(test)]
mod payments_tests;
mod private_details;
#[cfg(test)]
mod private_details_tests;
mod profile;
#[cfg(test)]
mod profile_tests;
mod profiles;
pub(crate) mod provenance;
mod recovery;
#[cfg(test)]
mod recovery_tests;
mod registration;
pub(crate) mod registration_flow;
pub(crate) mod registration_ledger;
mod registration_rows;
#[cfg(test)]
mod registration_tests;
mod requests;
mod reverify;
pub(crate) mod runtime;
#[cfg(test)]
mod runtime_tests;
pub mod signers;
mod startup;
mod startup_status;
mod status;
mod topup;
#[cfg(test)]
mod topup_tests;
pub(crate) mod trust;
#[cfg(test)]
mod trust_tests;
mod users;
#[cfg(test)]
mod users_tests;

pub use bringup::CREATED_HERE_BUDGET;
pub use contacts::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSectionPage, ContactSort,
    ContactSummary, ContactsPage, Eligibility, PrivateDetails, PublishState, Relation,
    RequestOutcome, ScannedCode, ScannedContact,
};
pub use credits::{
    CostTable, CreditLevel, PendingTopUp, TopUpOutcome, TopUpQuote, TopUpState, WithdrawAmount,
    WithdrawOutcome, WithdrawQuote,
};
pub use dashpay::{BearerSecret, DashPay};
pub use errors::{
    AvatarError, ContactError, CreditsError, IdentityError, InvitationError, NameError,
    PlatformError, ProfileError, RegistrationError,
};
pub use flows::{
    BudgetPurpose, BudgetView, DispatchResolution, DispatchResolved, DispatchState, FlowKind,
    GrantAction, GrantRequest, LeaseStateView, LeaseView, ParkReason, RevokeCause,
};
pub use identity::{
    IdentityDetail, IdentityKeyInfo, IdentitySummary, KeyPurpose, KeyType, NamesCheck,
    SecurityLevel,
};
pub use invitations::{InvitationInvalidReason, InvitationStatus};
pub use lease::{FlowOutcome, FlowReport, LeaseError, LockPhase, LockReport};
pub use names::{
    ContestContender, ContestState, ContestStatus, NameAvailability, NameOutcome, UserHit,
    UsernameCheck, UsernameRule, UsernameRuleCheck, check_username,
};
pub use notifications::{DashPayEvent, EventKind, EventPage, MAX_NEWS};
pub use payments::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, Counterparty, LockResolution,
    PaymentLock,
};
pub use profile::{
    AvatarCandidate, AvatarChange, AvatarImage, AvatarSize, AvatarSource, CropRect, Profile,
    ProfileBaseline, ProfileEdit, ProfileField, ProfileLimits,
};
pub use registration::{
    FaucetLockKey, FinishReport, InitialProfile, RegistrationFailure, RegistrationFunding,
    RegistrationPhase, RegistrationQuote, RegistrationRequest, RegistrationStatus,
    RegistrationWait,
};
pub use signers::{CountingFundingSigner, VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
pub use startup::{
    DashPayStatus, DashPaySyncStatus, NoIdentityReason, QuorumSource, StartupStatus, SyncLoop,
    SyncLoopStatus, SyncPassReport,
};
pub use startup_status::{DashPayStartup, PlatformCadence, SpvState};
pub use status::PlatformStatus;
