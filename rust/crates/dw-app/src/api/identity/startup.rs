//! DashPay status, bring-up outcome and sync loops.

use dw_engine::platform as engine;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::mirror_enum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NoIdentityReason {
    /// The wallet has no keys (DashPay is read-only).
    WatchOnly,
    /// Core or masternode sync has not finished.
    WaitingForSync,
}

mirror_enum!(engine::NoIdentityReason => NoIdentityReason { WatchOnly, WaitingForSync });

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

/// The banner state shared by the Home card, the identity chip and the
/// Contacts empty state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum DashPayStatus {
    /// No identity yet. `reason` is why joining is not offered now; `null`
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
    /// The bring-up did not settle.
    StartupIncomplete { startup: StartupStatus },
}

impl From<engine::DashPayStatus> for DashPayStatus {
    fn from(s: engine::DashPayStatus) -> Self {
        use engine::DashPayStatus as E;
        match s {
            E::NoIdentity { reason } => DashPayStatus::NoIdentity {
                reason: reason.map(Into::into),
            },
            E::Registering { draft } => DashPayStatus::Registering { draft },
            E::ContestPending {
                identity,
                label,
                ends_at,
            } => DashPayStatus::ContestPending {
                identity,
                label,
                ends_at,
            },
            E::Ready { main } => DashPayStatus::Ready { main },
            E::StartupIncomplete { startup } => DashPayStatus::StartupIncomplete {
                startup: startup.into(),
            },
        }
    }
}

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

/// Where Platform proofs get their quorum keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum QuorumSource {
    /// dash-spv's masternode state.
    Spv,
    /// The trusted service, before masternode sync; results are unverified.
    TrustedFallback,
    /// The trusted service only.
    Trusted,
}

mirror_enum!(engine::QuorumSource => QuorumSource { Spv, TrustedFallback, Trusted });

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncPassReport {
    pub started_at: u64,
    pub finished_at: u64,
    pub new_requests: u32,
    pub new_contacts: u32,
    pub new_payments: u32,
    /// The error code of a failed pass.
    pub failure_code: Option<String>,
}

impl From<engine::SyncPassReport> for SyncPassReport {
    fn from(r: engine::SyncPassReport) -> Self {
        let engine::SyncPassReport {
            started_at,
            finished_at,
            new_requests,
            new_contacts,
            new_payments,
            failure_code,
        } = r;
        SyncPassReport {
            started_at,
            finished_at,
            new_requests,
            new_contacts,
            new_payments,
            failure_code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncLoopStatus {
    pub sync_loop: SyncLoop,
    pub running: bool,
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

/// Tools ▸ Information's DashPay card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPaySyncStatus {
    pub startup: StartupStatus,
    pub last_pass: Option<SyncPassReport>,
    /// Contacts waiting for an unlock to finish setting up.
    pub pending_contact_crypto: u32,
    pub loops: Vec<SyncLoopStatus>,
    pub quorum_source: QuorumSource,
}

impl From<engine::DashPaySyncStatus> for DashPaySyncStatus {
    fn from(s: engine::DashPaySyncStatus) -> Self {
        let engine::DashPaySyncStatus {
            startup,
            last_pass,
            pending_contact_crypto,
            loops,
            quorum_source,
        } = s;
        DashPaySyncStatus {
            startup: startup.into(),
            last_pass: last_pass.map(Into::into),
            pending_contact_crypto,
            loops: loops.into_iter().map(Into::into).collect(),
            quorum_source: quorum_source.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_is_tagged_in_camel_case() {
        let s = DashPayStatus::from(engine::DashPayStatus::ContestPending {
            identity: "id".into(),
            label: "alice".into(),
            ends_at: Some(9),
        });
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({
                "kind": "contestPending", "identity": "id", "label": "alice", "endsAt": 9
            })
        );
        let s = DashPayStatus::from(engine::DashPayStatus::StartupIncomplete {
            startup: engine::StartupStatus::SeedBindingUnverified,
        });
        assert_eq!(
            serde_json::to_value(s).unwrap(),
            serde_json::json!({"kind": "startupIncomplete", "startup": "seedBindingUnverified"})
        );
    }
}
