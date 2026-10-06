//! M2 Tools window: Information tab, peer moderation, rescan control and
//! repair, node warnings. Owner: R1 (engine-tools). Values a full node would
//! have and SPV does not are `None` (DESIGN-opus §1.14), never estimated.
//! Contract: docs/contracts/m2-engine.md §2.4.

use crate::api::common::{ensure_open, not_implemented, parse_wallet_id};
use crate::{DashNetwork, NetworkSession, SyncError};

/// A count of masternodes from the SML ("Total: X (Enabled: Y)").
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct MasternodeCount {
    pub total: u32,
    pub enabled: u32,
}

/// The best ChainLock seen.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ChainLockInfo {
    pub height: u32,
    pub block_hash: String,
    /// Block time of the locked block.
    pub block_time: Option<u64>,
}

/// dash-qt Tools → Information (QT-143, §14.1).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NodeInfo {
    /// `core_version()`.
    pub client_version: String,
    /// The user agent sent to peers, e.g. `/dashwallet-desktop:0.1.0/`.
    pub user_agent: String,
    pub data_dir: String,
    /// When this session opened, UNIX seconds ("Startup time").
    pub startup_time: u64,
    pub network: DashNetwork,
    pub connections_in: u32,
    pub connections_out: u32,
    /// SPV does not listen: always empty.
    pub local_addresses: Vec<String>,
    pub tip_height: Option<u32>,
    pub tip_time: Option<u64>,
    pub tip_hash: Option<String>,
    pub best_chainlock: Option<ChainLockInfo>,
    /// From the synced SML; `None` before the masternode phase finished.
    pub masternodes: Option<MasternodeCount>,
    pub evonodes: Option<MasternodeCount>,
    /// Full node only (mempool, InstantSend counters, credit pool, quorum
    /// health): `None` on SPV.
    pub mempool_tx_count: Option<u32>,
    pub mempool_usage_bytes: Option<u64>,
}

/// A banned peer (QT-147 "Banned peers").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BannedPeer {
    /// IP or IP/netmask.
    pub subnet: String,
    /// UNIX seconds.
    pub banned_until: u64,
}

/// A running rescan (QT-117 progress dialog, IOS-113).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RescanProgress {
    pub from_height: u32,
    /// Last filter height scanned.
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub started_at: u64,
}

/// Node warnings for the alert banner (QT-040).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum WarningCode {
    /// The engine is a pre-release build ("This is a pre-release test build
    /// - use at your own risk").
    PrereleaseBuild,
    /// The previous session did not close cleanly.
    UncleanShutdown,
    /// SPV made no progress for 45 s (same rule as `Notice{SyncStalled}`).
    SyncStalled,
    /// Peers' timestamps differ from the local clock by more than 70 min
    /// (dash-qt "Please check that your computer's date and time are
    /// correct").
    ClockSkew,
    /// The trusted quorum service is unreachable; Platform features wait.
    PlatformContextUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EngineWarning {
    pub code: WarningCode,
    /// Diagnostic text for logs only.
    pub detail: String,
}

impl From<dw_engine::MasternodeCount> for MasternodeCount {
    fn from(c: dw_engine::MasternodeCount) -> Self {
        Self {
            total: c.total,
            enabled: c.enabled,
        }
    }
}

impl From<dw_engine::WarningCode> for WarningCode {
    fn from(c: dw_engine::WarningCode) -> Self {
        use dw_engine::WarningCode as W;
        match c {
            W::PrereleaseBuild => Self::PrereleaseBuild,
            W::UncleanShutdown => Self::UncleanShutdown,
            W::SyncStalled => Self::SyncStalled,
            W::ClockSkew => Self::ClockSkew,
            W::PlatformContextUnavailable => Self::PlatformContextUnavailable,
        }
    }
}

impl From<dw_engine::RescanProgress> for RescanProgress {
    fn from(p: dw_engine::RescanProgress) -> Self {
        Self {
            from_height: p.from_height,
            current_height: p.current_height,
            target_height: p.target_height,
            started_at: p.started_at,
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// The Information tab. In-memory read; re-query on `Sync` events.
    pub fn node_info(&self) -> Result<NodeInfo, SyncError> {
        let i = self.inner.node_info()?;
        Ok(NodeInfo {
            client_version: crate::core_version(),
            user_agent: i.user_agent,
            data_dir: i.data_dir,
            startup_time: i.startup_time,
            network: i.network.into(),
            connections_in: i.connections_in,
            connections_out: i.connections_out,
            local_addresses: i.local_addresses,
            tip_height: i.tip_height,
            tip_time: i.tip_time,
            tip_hash: i.tip_hash,
            best_chainlock: i.best_chainlock.map(|c| ChainLockInfo {
                height: c.height,
                block_hash: c.block_hash,
                block_time: c.block_time,
            }),
            masternodes: i.masternodes.map(Into::into),
            evonodes: i.evonodes.map(Into::into),
            mempool_tx_count: i.mempool_tx_count,
            mempool_usage_bytes: i.mempool_usage_bytes,
        })
    }

    /// Current node warnings, most severe first. In-memory read; re-query
    /// on `Notice` and `Sync` events. `ClockSkew` is never reported: dash-spv
    /// does not expose peer time offsets.
    pub fn warnings(&self) -> Result<Vec<EngineWarning>, SyncError> {
        Ok(self
            .inner
            .warnings()?
            .into_iter()
            .map(|w| EngineWarning {
                code: w.code.into(),
                detail: w.detail,
            })
            .collect())
    }

    /// Disconnects one peer (QT-147 "Disconnect"). Needs dash-spv's network
    /// manager through platform-wallet (upstream U2): `NotImplemented`
    /// until then.
    pub async fn disconnect_peer(&self, address: String) -> Result<(), SyncError> {
        let _ = address;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.disconnect_peer")
    }

    /// Bans the peer's IP for `duration_secs` (dash-qt: 1 h, 1 d, 1 w, 1 y)
    /// and disconnects it. Needs upstream U2: `NotImplemented` until then.
    pub async fn ban_peer(&self, address: String, duration_secs: u64) -> Result<(), SyncError> {
        let _ = (address, duration_secs);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.ban_peer")
    }

    /// Needs upstream U2: `NotImplemented` until then.
    pub async fn unban_peer(&self, subnet: String) -> Result<(), SyncError> {
        let _ = subnet;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.unban_peer")
    }

    /// Needs upstream U2: `NotImplemented` until then.
    pub async fn banned_peers(&self) -> Result<Vec<BannedPeer>, SyncError> {
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.banned_peers")
    }

    /// The running rescan, `None` when none runs. In-memory read.
    pub fn rescan_progress(&self) -> Result<Option<RescanProgress>, SyncError> {
        Ok(self.inner.rescan_progress()?.map(Into::into))
    }

    /// dash-qt `abortrescan`: stops the running rescan where it is; the
    /// wallets keep what was found. `false` when no rescan ran.
    pub async fn cancel_rescan(&self) -> Result<bool, SyncError> {
        Ok(self.inner.cancel_rescan().await?)
    }

    /// Repair "Rebuild Index" equivalent (QT-148, DESIGN-opus §1.14): deletes
    /// the SPV chain data (`spv/`) and keeps wallets, vault and app data.
    /// SPV must be stopped (`sync.spv_running`); the next `start_spv` syncs
    /// from scratch and rescans every wallet from its birth height.
    pub async fn reset_chain_data(&self) -> Result<(), SyncError> {
        Ok(self.inner.reset_chain_data().await?)
    }

    /// iOS "edit birth height" (IOS-113): stores a new birth height for the
    /// wallet. Lower than the current scan checkpoint: a rescan from it is
    /// scheduled.
    pub async fn set_birth_height(&self, wallet_id: String, height: u32) -> Result<(), SyncError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.set_birth_height(id, height).await?)
    }
}
