//! DashPay status, bring-up outcome and sync loops (DASHPAY §3.2; E0-05).
//!
//! Conventions for every facade record (m1-engine.md §1): identity ids are
//! Base58, txids lower-case hex, duffs and credits `u64`, times UNIX
//! seconds, and an unknown value is `None`. Data-carrying enums serialize
//! with a `kind` tag.

use dpp::version::PlatformVersion;
use serde::{Deserialize, Serialize};

use super::dashpay::{DashPay, stub};
use super::errors::PlatformError;
use crate::NetworkSession;

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
    /// The session runs with Platform (not `--no-platform`): without it no
    /// bring-up or pass runs, and none is waited on.
    pub platform_enabled: bool,
    /// The wallet's last Platform-address pass of this session.
    pub addresses: AddressSync,
}

/// A wallet's Platform-address pass, as the status bar waits on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressSync {
    /// No pass has run for the wallet yet this session.
    Pending,
    /// Its last pass failed; the next one tries again.
    Failed,
    Done,
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

/// The open network's Platform protocol compatibility (`NetworkSession::protocol_compatibility`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolCompatibility {
    /// Active Platform protocol version from the latest proved epoch read
    /// (or proved response metadata); `None` until Platform has answered,
    /// or when `SessionOptions::no_platform` is set.
    pub network_version: Option<u32>,
    /// Highest Platform protocol version this build supports.
    pub supported_version: u32,
}

impl NetworkSession {
    /// The open network's Platform protocol version and the highest version
    /// this build supports. In-memory read; answers offline (`network_version`
    /// is `None` until Platform has answered).
    pub fn protocol_compatibility(&self) -> Result<ProtocolCompatibility, PlatformError> {
        let _op = self.try_enter().map_err(PlatformError::from)?;
        Ok(ProtocolCompatibility {
            network_version: self
                .platform
                .enabled()
                .then(|| self.platform.protocol.observed())
                .flatten(),
            supported_version: PlatformVersion::latest().protocol_version,
        })
    }
}

impl DashPay {
    /// The banner state, from memory and app.sqlite only (BL-153): no
    /// Platform read, so it answers offline.
    pub fn status(&self) -> Result<DashPayStatus, PlatformError> {
        Ok(self.session.dashpay_status_of(self.wallet_id)?)
    }

    /// In-memory read. `last_pass` is `None` until sync passes report.
    pub fn sync_status(&self) -> Result<DashPaySyncStatus, PlatformError> {
        let s = &self.session;
        let startup = s.dashpay_startup(&self.wallet_id)?;
        Ok(DashPaySyncStatus {
            startup: startup.startup,
            last_pass: None,
            pending_contact_crypto: startup.contact_accounts_pending,
            loops: s.platform_loops()?,
            quorum_source: s.quorum_source()?,
            platform_enabled: s.platform.enabled(),
            addresses: match s.hub.platform_addresses_synced(self.wallet_id) {
                None => AddressSync::Pending,
                Some(false) => AddressSync::Failed,
                Some(true) => AddressSync::Done,
            },
        })
    }

    pub async fn sync_now(&self) -> Result<SyncPassReport, PlatformError> {
        stub("DashPay.sync_now")
    }
}
