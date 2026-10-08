//! DashPay status, bring-up outcome and sync loops (DASHPAY §3.2; E0-05).
//!
//! Conventions for every facade record (m1-engine.md §1): identity ids are
//! Base58, txids lower-case hex, duffs and credits `u64`, times UNIX
//! seconds, and an unknown value is `None`. Data-carrying enums serialize
//! with a `kind` tag.

use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::PlatformError;

/// The banner state shared by the Home card, the identity chip and the
/// Contacts empty state (F1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DashPayStatus {
    /// No identity yet. `reason` is why joining is not offered now; `None`
    /// means the user can join.
    NoIdentity { reason: Option<NoIdentityReason> },
    /// A registration flow is running; `draft` is its id.
    Registering { draft: String },
    /// The main identity's username is in a contest.
    ContestPending {
        identity: String,
        label: String,
        ends_at: Option<u64>,
    },
    /// `main` is the main identity.
    Ready { main: String },
    /// The bring-up did not settle (§3.2).
    StartupIncomplete { startup: StartupStatus },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoIdentityReason {
    /// The wallet has no keys (DashPay is read-only).
    WatchOnly,
    /// Core or masternode sync has not finished (§2.2 rule 5, §3.7).
    WaitingForSync,
}

/// The bring-up outcome: the library's seven `WalletStartupStatus` values
/// plus our own `NotRun`, `Starting` and `IdentityUnsettled` (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    /// The bring-up ran with a locked vault and no signers; it runs again at
    /// the first unlock.
    IdentityUnsettled,
}

/// Tools ▸ Information's DashPay card (F23).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPaySyncStatus {
    pub startup: StartupStatus,
    pub last_pass: Option<SyncPassReport>,
    /// Contacts waiting for an unlock to finish setting up.
    pub pending_contact_crypto: u32,
    pub loops: Vec<SyncLoopStatus>,
    pub quorum_source: QuorumSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPassReport {
    pub started_at: u64,
    pub finished_at: u64,
    pub new_requests: u32,
    pub new_contacts: u32,
    pub new_payments: u32,
    /// The error code of a failed pass.
    pub failure_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncLoopStatus {
    pub sync_loop: SyncLoop,
    pub running: bool,
    pub last_run_at: Option<u64>,
    pub next_run_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncLoop {
    IdentitySync,
    DashPaySync,
    DpnsSync,
    PlatformAddressSync,
}

/// Where Platform proofs get their quorum keys (§2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuorumSource {
    /// dash-spv's masternode state.
    Spv,
    /// The trusted service, before masternode sync; results are unverified.
    TrustedFallback,
    /// The trusted service only: the developer toggle or the degraded mode.
    Trusted,
}

impl DashPay {
    pub fn status(&self) -> Result<DashPayStatus, PlatformError> {
        stub("DashPay.status")
    }

    pub fn sync_status(&self) -> Result<DashPaySyncStatus, PlatformError> {
        stub("DashPay.sync_status")
    }

    pub async fn sync_now(&self) -> Result<SyncPassReport, PlatformError> {
        stub("DashPay.sync_now")
    }
}
