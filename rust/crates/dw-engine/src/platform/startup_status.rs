//! The records of the bring-up (DASHPAY §3.2; E0-05). `StartupStatus`,
//! `SyncLoop` and `SyncLoopStatus` are the M4 facade's, in `startup.rs`; this
//! file maps the library's outcome onto them.

use platform_wallet::manager::startup::WalletStartupStatus;
use serde::{Deserialize, Serialize};

use super::startup::StartupStatus;

/// SPV as hosts see it. `Starting` covers the bring-up and dash-spv's own
/// start (DASHPAY §2.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpvState {
    Stopped,
    Starting,
    Running,
}

impl From<WalletStartupStatus> for StartupStatus {
    fn from(s: WalletStartupStatus) -> Self {
        match s {
            WalletStartupStatus::Ready => Self::Ready,
            WalletStartupStatus::NoIdentity => Self::NoIdentity,
            WalletStartupStatus::PartialNoIdentity => Self::PartialNoIdentity,
            WalletStartupStatus::DiscoveryFailed => Self::DiscoveryFailed,
            WalletStartupStatus::PartialAccountsPending => Self::PartialAccountsPending,
            WalletStartupStatus::SeedBindingUnverified => Self::SeedBindingUnverified,
            WalletStartupStatus::IdentityScanIncomplete => Self::IdentityScanIncomplete,
        }
    }
}

impl StartupStatus {
    /// The serde name: the stable string notices and logs carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRun => "not_run",
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::NoIdentity => "no_identity",
            Self::PartialNoIdentity => "partial_no_identity",
            Self::DiscoveryFailed => "discovery_failed",
            Self::PartialAccountsPending => "partial_accounts_pending",
            Self::SeedBindingUnverified => "seed_binding_unverified",
            Self::IdentityScanIncomplete => "identity_scan_incomplete",
            Self::IdentityUnsettled => "identity_unsettled",
        }
    }

    /// An outcome `Notice{DashPayStartupIncomplete}` reports (§3.7): the
    /// bring-up ran with keys and left work for the DIP-15 rescan or a later
    /// start. `IdentityUnsettled` is not one: the first unlock settles it.
    pub(crate) fn is_incomplete(self) -> bool {
        matches!(
            self,
            Self::PartialNoIdentity
                | Self::DiscoveryFailed
                | Self::PartialAccountsPending
                | Self::SeedBindingUnverified
                | Self::IdentityScanIncomplete
        )
    }
}

/// One wallet's last bring-up (`NetworkSession::dashpay_startup`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashPayStartup {
    pub startup: StartupStatus,
    /// The wallet has no keys, so DashPay is read-only for it and nothing
    /// runs ("This wallet can't use DashPay because it has no keys").
    pub read_only: bool,
    /// Base58 id of the identity the bring-up knew, if any.
    pub identity: Option<String>,
    /// Contact-account builds still queued: their contacts' payments wait
    /// for an unlock or the DIP-15 rescan.
    pub contact_accounts_pending: u32,
    /// The identity scan is on record as incomplete (the library's flag; a
    /// pending queue outranks it in `startup`).
    pub identity_scan_incomplete: bool,
    /// When the bring-up ended, UNIX seconds.
    pub finished_at: Option<u64>,
}

impl DashPayStartup {
    pub(crate) fn new(startup: StartupStatus, read_only: bool) -> Self {
        Self {
            startup,
            read_only,
            identity: None,
            contact_accounts_pending: 0,
            identity_scan_incomplete: false,
            finished_at: None,
        }
    }
}

/// What the host tells the engine to pace the loops by (§3.2 "Cadence").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformCadence {
    /// A window is visible: `dashpay_sync` every 15 s; hidden or in the
    /// tray, every 60 s.
    pub window_visible: bool,
    /// A watched DPNS contest ends within the hour: the contest watch polls
    /// every minute instead of every 10.
    pub contest_ending_soon: bool,
}

impl Default for PlatformCadence {
    fn default() -> Self {
        Self {
            window_visible: true,
            contest_ending_soon: false,
        }
    }
}
