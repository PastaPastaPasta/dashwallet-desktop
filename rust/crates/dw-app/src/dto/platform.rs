//! Platform and E0-04 records that the event bridge, the error union and
//! more than one domain share: flows, budgets, leases, the lock's report and
//! dispatch outcomes (E0-04 §16).

use dw_engine::platform as engine;
use dw_engine::platform::{
    BudgetPurpose, BudgetView, DispatchResolution, DispatchResolved, DispatchState, Eligibility,
    FlowKind, FlowOutcome, FlowReport, KeyPurpose, LeaseStateView, LeaseView, LockPhase,
    LockReport, ParkReason, RevokeCause, UsernameRule,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{LockStatus, mirror_enum};

/// What a lease is for; it fixes which budgets its grants may fund.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FlowKindName {
    Registration,
    TopUp,
    Withdraw,
    NameRegistration,
    ProfileEdit,
    ContactRequest,
    Accept,
    /// "Accept and pay": the accept, then the payment's draft, under one lease.
    AcceptAndPay,
    PrivateDetails,
    EnableDashPayKeys,
}

mirror_enum!(FlowKind => FlowKindName {
    Registration, TopUp, Withdraw, NameRegistration, ProfileEdit, ContactRequest, Accept,
    AcceptAndPay, PrivateDetails, EnableDashPayKeys
});
mirror_enum!(FlowKindName => FlowKind {
    Registration, TopUp, Withdraw, NameRegistration, ProfileEdit, ContactRequest, Accept,
    AcceptAndPay, PrivateDetails, EnableDashPayKeys
});

/// A lease budget: `funding` and `spend` are duffs, `credits` credits;
/// `crypto` hands nothing off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BudgetPurposeName {
    Funding,
    Credits,
    Spend,
    Crypto,
}

mirror_enum!(BudgetPurpose => BudgetPurposeName { Funding, Credits, Spend, Crypto });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RevokeCauseName {
    Lock,
    Close,
    PassphraseChange,
    WalletRemoved,
    WalletClosed,
}

mirror_enum!(RevokeCause => RevokeCauseName { Lock, Close, PassphraseChange, WalletRemoved, WalletClosed });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum KeyPurposeName {
    Authentication,
    Encryption,
    Decryption,
    Transfer,
    System,
    Voting,
    Owner,
}

mirror_enum!(KeyPurpose => KeyPurposeName {
    Authentication, Encryption, Decryption, Transfer, System, Voting, Owner
});

/// A DPNS label rule (`check_username`, `name.invalid`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UsernameRuleName {
    /// At least 3 characters.
    MinLength,
    /// At most 23 characters.
    MaxLength,
    /// Only `[A-Za-z0-9-]`.
    AllowedCharacters,
    /// No hyphen at either end.
    NoEdgeHyphen,
    /// No two hyphens in a row.
    NoDoubleHyphen,
}

mirror_enum!(UsernameRule => UsernameRuleName {
    MinLength, MaxLength, AllowedCharacters, NoEdgeHyphen, NoDoubleHyphen
});

/// Whether an identity may send a contact request to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EligibilityName {
    Ok,
    NoDashPayKeys,
    IsSelf,
    AlreadyContact,
    PendingOutgoing,
    PendingIncoming,
}

mirror_enum!(Eligibility => EligibilityName {
    Ok, NoDashPayKeys, IsSelf, AlreadyContact, PendingOutgoing, PendingIncoming
});

/// One budget of a lease's current generation, in its purpose's unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    pub purpose: BudgetPurposeName,
    #[ts(type = "string")]
    #[serde(with = "super::credits")]
    pub ceiling: u64,
    #[ts(type = "string")]
    #[serde(with = "super::credits")]
    pub spent: u64,
}

impl From<BudgetView> for Budget {
    fn from(b: BudgetView) -> Self {
        let BudgetView {
            purpose,
            ceiling,
            spent,
        } = b;
        Budget {
            purpose: purpose.into(),
            ceiling,
            spent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LeaseState {
    Active,
    /// Funding handed off with an own key, until `keyExpiresInSecs`.
    AwaitingProof,
    /// Waiting for a proof (`proofWaiting`), holding no key.
    Parked,
    NeedsGrant,
    Revoked {
        cause: RevokeCauseName,
    },
    Ended,
}

impl From<LeaseStateView> for LeaseState {
    fn from(s: LeaseStateView) -> Self {
        match s {
            LeaseStateView::Active => LeaseState::Active,
            LeaseStateView::AwaitingProof => LeaseState::AwaitingProof,
            LeaseStateView::Parked {
                reason: ParkReason::ProofWaiting,
            } => LeaseState::Parked,
            LeaseStateView::NeedsGrant => LeaseState::NeedsGrant,
            LeaseStateView::Revoked { cause } => LeaseState::Revoked {
                cause: cause.into(),
            },
            LeaseStateView::Ended => LeaseState::Ended,
        }
    }
}

/// A lease as the UI sees it (E0-04 §4.6, §16.5): the copy C1–C7 is chosen
/// from it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Lease {
    /// The first 8 hex digits of the id; for logs and the UI only.
    pub id: String,
    pub wallet_id: String,
    pub flow: FlowKindName,
    pub state: LeaseState,
    pub own_key: bool,
    /// While an own key is held: seconds until it is dropped.
    pub key_expires_in_secs: Option<u64>,
    /// E0-04 §16.5's single definition.
    pub funds_committed: bool,
    pub budgets: Vec<Budget>,
    /// Permits held now.
    pub in_flight: u32,
    /// A library call of the flow is running; selects the copy.
    pub call_running: bool,
}

impl From<LeaseView> for Lease {
    fn from(l: LeaseView) -> Self {
        let LeaseView {
            id,
            wallet_id,
            flow,
            state,
            own_key,
            key_expires_in_secs,
            funds_committed,
            budgets,
            in_flight,
            call_running,
        } = l;
        Lease {
            id,
            wallet_id,
            flow: flow.into(),
            state: state.into(),
            own_key,
            key_expires_in_secs,
            funds_committed,
            budgets: budgets.into_iter().map(Into::into).collect(),
            in_flight,
            call_running,
        }
    }
}

/// How a flow (or one artifact of it) ended up when a lock cut it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum FlowOutcomeName {
    Cancelled,
    MaybeSent,
    WillBeSent,
    Sent,
}

mirror_enum!(FlowOutcome => FlowOutcomeName { Cancelled, MaybeSent, WillBeSent, Sent });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactOutcome {
    pub artifact: String,
    pub outcome: FlowOutcomeName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FlowLockReport {
    /// The facade's lease id.
    pub lease: String,
    pub wallet_id: String,
    pub flow: FlowKindName,
    pub outcome: FlowOutcomeName,
    /// Each committed artifact of the flow, with its own outcome.
    pub artifacts: Vec<ArtifactOutcome>,
}

impl From<FlowReport> for FlowLockReport {
    fn from(r: FlowReport) -> Self {
        let FlowReport {
            lease,
            wallet_id,
            flow,
            outcome,
            artifacts,
        } = r;
        FlowLockReport {
            lease,
            wallet_id,
            flow: flow.into(),
            outcome: outcome.into(),
            artifacts: artifacts
                .into_iter()
                .map(|(artifact, outcome)| ArtifactOutcome {
                    artifact,
                    outcome: outcome.into(),
                })
                .collect(),
        }
    }
}

/// What a full lock did: the vault's state after it, and each flow it cut
/// (E0-04 §8.1, copy C4/C5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LockOutcome {
    pub status: LockStatus,
    pub flows: Vec<FlowLockReport>,
}

impl From<LockReport> for LockOutcome {
    fn from(r: LockReport) -> Self {
        let LockReport { status, flows } = r;
        LockOutcome {
            status: status.into(),
            flows: flows.into_iter().map(Into::into).collect(),
        }
    }
}

/// A lock's drain (E0-04 §16.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum LockProgress {
    /// Permits handed off before the lock are still in flight (copy C6).
    Draining {
        in_flight: u32,
        deadline_in_ms: u64,
    },
    Done {
        report: LockOutcome,
    },
}

impl From<LockPhase> for LockProgress {
    fn from(p: LockPhase) -> Self {
        match p {
            LockPhase::Draining {
                in_flight,
                deadline_in_ms,
            } => LockProgress::Draining {
                in_flight,
                deadline_in_ms,
            },
            LockPhase::Done(report) => LockProgress::Done {
                report: report.into(),
            },
        }
    }
}

/// What the engine knows about a handed-off artifact (`dispatch_status`).
/// Only `notSent` allows a retry the UI offers (H11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DispatchStateName {
    WillBeSent,
    MaybeSent,
    Sent,
    NotSent,
}

mirror_enum!(DispatchState => DispatchStateName { WillBeSent, MaybeSent, Sent, NotSent });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DispatchResolutionName {
    Sent,
    NotSent,
}

mirror_enum!(DispatchResolution => DispatchResolutionName { Sent, NotSent });

/// A provisional outcome settled (E0-04 §16.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DispatchSettled {
    pub wallet_id: String,
    /// A txid, a state-transition hash, or a funding step id
    /// (`registration/<draft>/funding`, `topup/<id>/funding`).
    pub artifact: String,
    pub resolution: DispatchResolutionName,
}

impl From<DispatchResolved> for DispatchSettled {
    fn from(r: DispatchResolved) -> Self {
        let DispatchResolved {
            wallet_id,
            artifact,
            resolution,
        } = r;
        DispatchSettled {
            wallet_id,
            artifact,
            resolution: resolution.into(),
        }
    }
}

/// The bring-up outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum StartupStatus {
    NotRun,
    Starting,
    Ready,
    NoIdentity,
    PartialNoIdentity,
    DiscoveryFailed,
    PartialAccountsPending,
    SeedBindingUnverified,
    IdentityScanIncomplete,
    /// The bring-up ran with a locked vault; it runs again at the first unlock.
    IdentityUnsettled,
}

mirror_enum!(engine::StartupStatus => StartupStatus {
    NotRun, Starting, Ready, NoIdentity, PartialNoIdentity, DiscoveryFailed,
    PartialAccountsPending, SeedBindingUnverified, IdentityScanIncomplete, IdentityUnsettled
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::enum_variant_names, reason = "mirrors the engine's variants")]
pub enum SyncLoop {
    IdentitySync,
    DashPaySync,
    DpnsSync,
    PlatformAddressSync,
}

mirror_enum!(engine::SyncLoop => SyncLoop {
    IdentitySync, DashPaySync, DpnsSync, PlatformAddressSync
});

/// One Platform sync loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncLoopStatus {
    pub sync_loop: SyncLoop,
    pub running: bool,
    /// UNIX seconds.
    pub last_run_at: Option<u64>,
    pub next_run_at: Option<u64>,
}

impl From<engine::SyncLoopStatus> for SyncLoopStatus {
    fn from(s: engine::SyncLoopStatus) -> Self {
        let engine::SyncLoopStatus {
            sync_loop,
            running,
            last_run_at,
            next_run_at,
        } = s;
        SyncLoopStatus {
            sync_loop: sync_loop.into(),
            running,
            last_run_at,
            next_run_at,
        }
    }
}

/// A DashPay profile as published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayProfile {
    pub display_name: Option<String>,
    pub public_message: Option<String>,
    pub avatar_url: Option<String>,
    /// Hex SHA-256 of the image bytes, as published.
    pub avatar_hash: Option<String>,
    /// Hex dHash, as published.
    pub avatar_fingerprint: Option<String>,
    pub updated_at: Option<u64>,
}

impl From<engine::Profile> for DashPayProfile {
    fn from(p: engine::Profile) -> Self {
        let engine::Profile {
            display_name,
            public_message,
            avatar_url,
            avatar_hash,
            avatar_fingerprint,
            updated_at,
        } = p;
        DashPayProfile {
            display_name,
            public_message,
            avatar_url,
            avatar_hash,
            avatar_fingerprint,
            updated_at,
        }
    }
}

/// How a user relates to one of the wallet's identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum UserRelation {
    Stranger,
    Contact,
    PendingOutgoing,
    PendingIncoming,
    Ignored,
    IsSelf,
}

mirror_enum!(engine::Relation => UserRelation {
    Stranger, Contact, PendingOutgoing, PendingIncoming, Ignored, IsSelf
});
