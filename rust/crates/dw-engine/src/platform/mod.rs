//! DashPay and Platform glue over platform-wallet (DASHPAY §3.1). The
//! library owns the protocol; this module supplies what it asks the host
//! for: signers over the vault, bring-up order, read models and flows.
//! [`DashPay`] is the facade the UI binding and `dwcli` call; its records
//! live in the domain files and are re-exported here one by one, so a `pub`
//! helper never becomes API by accident.

pub(crate) mod app_connect;
mod app_connect_net;
mod app_connect_plan;
pub(crate) mod attribution;
mod avatar;
mod avatar_cache;
mod avatar_dhash;
mod avatar_fetch;
mod avatar_image;
#[cfg(test)]
pub(crate) mod avatar_tests;
mod balances;
mod banner;
#[cfg(test)]
mod banner_tests;
mod bringup;
#[cfg(test)]
mod close_unload_gate_tests;
mod contact_names;
pub(crate) mod contact_payout;
mod contacts;
pub(crate) mod contacts_model;
#[cfg(all(test, target_os = "linux"))]
mod contacts_perf_tests;
#[cfg(test)]
mod contacts_tests;
pub(crate) mod contest_watch;
#[cfg(test)]
mod contest_watch_tests;
pub(crate) mod core_max;
mod credit_costs;
mod credit_transfer;
mod credits;
mod dashpay;
mod errors;
mod flows;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
mod identity;
mod identity_reads;
mod identity_writes;
mod input0;
mod invitation_check;
mod invitations;
pub(crate) mod journal;
#[cfg(test)]
mod journal_tests;
pub mod keys_policy;
#[cfg(test)]
mod known_wallets_tests;
pub mod lease;
mod lock_inputs;
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
mod platform_addresses;
mod platform_fund;
#[cfg(test)]
mod platform_fund_tests;
pub(crate) mod platform_moves;
mod private_details;
#[cfg(test)]
mod private_details_tests;
mod profile;
#[cfg(test)]
mod profile_tests;
mod profiles;
pub(crate) mod protocol;
pub(crate) mod provenance;
mod recovery;
#[cfg(test)]
mod recovery_tests;
mod registration;
pub(crate) mod registration_flow;
pub(crate) mod registration_ledger;
mod registration_platform;
mod registration_rows;
mod registration_shielded;
#[cfg(test)]
mod registration_tests;
#[cfg(test)]
mod removal_gate_tests;
mod requests;
pub(crate) mod restore_hold;
#[cfg(test)]
mod restore_hold_tests;
mod reverify;
pub(crate) mod runtime;
#[cfg(test)]
mod runtime_tests;
pub(crate) mod shield_drain;
mod shielded;
#[cfg(test)]
mod shielded_close_tests;
#[cfg(test)]
mod shielded_import_tests;
pub(crate) mod shielded_payout;
pub(crate) mod shielded_pool;
pub(crate) mod shielded_release;
#[cfg(test)]
pub(crate) mod shielded_tests;
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
mod unfinished;
#[cfg(test)]
mod unfinished_conformance;
mod unfinished_funds;
pub(crate) mod unfinished_locks;
#[cfg(test)]
mod unfinished_locks_tests;
mod unfinished_remainder;
mod unfinished_shields;
mod users;
#[cfg(test)]
mod users_tests;
pub(crate) mod withdrawal;

pub use app_connect::{
    ConnectBinding, ConnectChoice, ConnectDocumentType, ConnectDone, ConnectFailure, ConnectGroup,
    ConnectIdentity, ConnectLevel, ConnectQuote, ConnectScope, ConnectSheet, ConnectTerms,
    ConnectToken,
};
pub(crate) use avatar::{Locks, keyed_lock};
pub use bringup::CREATED_HERE_BUDGET;
pub use contacts::{
    ChannelState, ContactDetail, ContactQuery, ContactSection, ContactSectionPage, ContactSort,
    ContactSummary, ContactsPage, Eligibility, PrivateDetails, PublishState, Relation,
    RequestOutcome, ScannedCode, ScannedContact,
};
pub use credits::{
    CoreMax, CoreMaxShort, CostTable, CreditLevel, FundAmount, PendingTopUp, StuckTopUpFunding,
    TopUpFromPlatformOutcome, TopUpFromPlatformQuote, TopUpOutcome, TopUpQuote, TopUpState,
    WithdrawAmount, WithdrawOutcome, WithdrawQuote, WithdrawToPlatformOutcome,
    WithdrawToPlatformQuote, Withdrawal, WithdrawalStatus,
};
pub use dashpay::{BearerSecret, DashPay};
pub use errors::{
    AvatarError, ConnectError, ContactError, CreditsError, IdentityError, InvitationError,
    NameError, PlatformError, ProfileError, RegistrationError, ShieldedError,
};
pub use flows::{
    BudgetPurpose, BudgetView, DispatchResolution, DispatchResolved, DispatchState, FlowKind,
    GrantAction, GrantRequest, LeaseStateView, LeaseView, ParkReason, RevokeCause,
};
pub use identity::{
    FoundBy, FoundIdentity, IdentityDetail, IdentityKeyInfo, IdentityRefresh, IdentitySummary,
    KeyPurpose, KeyType, NamesCheck, SecurityLevel,
};
pub use invitations::{
    InvitationInvalidReason, InvitationStatus, StashedLink, StashedLinkKind, TakenLink,
};
pub use lease::{FlowOutcome, FlowReport, LeaseError, LockPhase, LockReport};
pub use names::{
    ContestContender, ContestState, ContestStatus, NameAvailability, NameOutcome, UserHit,
    UsernameCheck, UsernameRule, UsernameRuleCheck, check_username,
};
pub use notifications::{DashPayEvent, EventKind, EventPage, MAX_NEWS};
pub use payments::{
    ActivityDirection, ActivityFilter, ActivityItem, ActivityPage, ContactPayOutcome,
    ContactPayQuote, Counterparty, LockResolution, PaymentLock, PaymentSource, PayoutState,
};
pub use platform_addresses::{
    MoveToCoreOutcome, MoveToCoreQuote, PendingPlatformFund, PlatformActivity,
    PlatformActivityKind, PlatformActivityState, PlatformAddressBalance, PlatformAddressRow,
    PlatformFundOutcome, PlatformFundQuote, PlatformFundState, PlatformReceiveAddress,
    PlatformSendOutcome, PlatformSendQuote, PlatformShieldQuote, RecipientProblem,
};
pub use profile::{
    AvatarCandidate, AvatarChange, AvatarImage, AvatarSize, AvatarSource, CropRect, Profile,
    ProfileBaseline, ProfileEdit, ProfileField, ProfileLimits,
};
pub use registration::{
    FaucetLockKey, FinishReport, InitialProfile, PlatformRegistrationQuote, RegistrationFailure,
    RegistrationFunding, RegistrationInvitation, RegistrationPhase, RegistrationQuote,
    RegistrationRequest, RegistrationStatus, RegistrationWait, ShieldedCheck, ShieldedCheckState,
    ShieldedReadiness,
};
pub use shielded::{
    HeldSpend, MixedShieldQuote, PendingShield, SHIELDED_ACTIVATION_VERSION, ShieldOutcome,
    ShieldQuote, ShieldRepairCost, ShieldState, ShieldedAccount, ShieldedActivity,
    ShieldedActivityKind, ShieldedActivityStatus, ShieldedAddress, ShieldedDirection,
    ShieldedRefusal, ShieldedScanProgress, ShieldedSpendOutcome, ShieldedSpendQuote,
    ShieldedSpendState, ShieldedStatus, ShieldedStoreClosed, ShieldedSyncInfo, ShieldedSyncWait,
    StuckShieldFunding, UnrecordedSpend, UnresolvedSpend, UnshieldToPlatformOutcome,
};
pub use signers::{
    CountingFundingSigner, VaultContactCrypto, VaultIdentitySigner, VaultPlatformAddressSigner,
    VaultScanKey,
};
pub use startup::{
    AddressSync, DashPayStatus, DashPaySyncStatus, NoIdentityReason, ProtocolCompatibility,
    QuorumSource, StartupStatus, SyncLoop, SyncLoopStatus, SyncPassReport,
};
pub use startup_status::{DashPayStartup, PlatformCadence, SpvState};
pub use status::PlatformStatus;
pub use unfinished::{
    LockFinish, LockKindId, LockOrigin, RecoverQuote, TransfersReport, UnfinishedLock,
    UnfinishedState, UnfinishedTransfers,
};
