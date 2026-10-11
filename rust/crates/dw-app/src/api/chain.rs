//! Chain and sync: SPV, peers, rescans, node information and the Platform
//! runtime's start-up and loops.
//!
//! dw-ffi's `network_stats` (the "Network" sub-tab) only reshapes
//! `node_info`, so it has no command: the renderer takes the masternode
//! counts and the best ChainLock from `node_info`. The console is
//! `api::console`.

use dw_engine::platform::{DashPayStartup, PlatformCadence, StartupStatus, SyncLoop};
use dw_engine::{
    ChainLockInfo, EngineWarning, MasternodeCount, NodeInfo, PeerInfo, PlatformStatus,
    RescanFrom, RescanProgress, SpvState, SyncLoopStatus, WarningCode,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::{NetworkInfo, SyncStatus, mirror_enum};
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// A connected SPV peer. dash-spv reports only addresses: the other fields
/// are `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PeerSummary {
    /// `ip:port`.
    pub address: String,
    pub user_agent: Option<String>,
    pub protocol_version: Option<u32>,
    pub best_height: Option<u32>,
    pub ping_ms: Option<u32>,
    /// UNIX seconds.
    pub connected_since: Option<u64>,
    pub inbound: bool,
    pub bytes_sent: Option<u64>,
    pub bytes_received: Option<u64>,
}

impl From<PeerInfo> for PeerSummary {
    fn from(p: PeerInfo) -> Self {
        let PeerInfo {
            address,
            user_agent,
            protocol_version,
            best_height,
            ping_ms,
            connected_since,
            inbound,
            bytes_sent,
            bytes_received,
        } = p;
        PeerSummary {
            address,
            user_agent,
            protocol_version,
            best_height,
            ping_ms,
            connected_since,
            inbound,
            bytes_sent,
            bytes_received,
        }
    }
}

/// Where a rescan starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum RescanStart {
    /// Each wallet's own birth height.
    WalletBirth,
    Genesis,
    Height { height: u32 },
}

impl From<RescanStart> for RescanFrom {
    fn from(r: RescanStart) -> Self {
        match r {
            RescanStart::WalletBirth => RescanFrom::WalletBirth,
            RescanStart::Genesis => RescanFrom::Genesis,
            RescanStart::Height { height } => RescanFrom::Height(height),
        }
    }
}

impl From<RescanFrom> for RescanStart {
    fn from(r: RescanFrom) -> Self {
        match r {
            RescanFrom::WalletBirth => RescanStart::WalletBirth,
            RescanFrom::Genesis => RescanStart::Genesis,
            RescanFrom::Height(height) => RescanStart::Height { height },
        }
    }
}

/// "Total: X (Enabled: Y)".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MasternodeCounts {
    pub total: u32,
    pub enabled: u32,
}

impl From<MasternodeCount> for MasternodeCounts {
    fn from(c: MasternodeCount) -> Self {
        let MasternodeCount { total, enabled } = c;
        MasternodeCounts { total, enabled }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ChainLock {
    pub height: u32,
    pub block_hash: String,
    pub block_time: Option<u64>,
}

impl From<ChainLockInfo> for ChainLock {
    fn from(c: ChainLockInfo) -> Self {
        let ChainLockInfo {
            height,
            block_hash,
            block_time,
        } = c;
        ChainLock {
            height,
            block_hash,
            block_time,
        }
    }
}

/// dash-qt Tools, Information. Values only a full node has are `null` or
/// empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummary {
    /// The user agent announced to peers.
    pub user_agent: String,
    pub data_dir: String,
    /// UNIX seconds this session opened.
    pub startup_time: u64,
    pub network: NetworkInfo,
    pub connections_in: u32,
    pub connections_out: u32,
    pub local_addresses: Vec<String>,
    pub tip_height: Option<u32>,
    pub tip_time: Option<u64>,
    pub tip_hash: Option<String>,
    pub best_chainlock: Option<ChainLock>,
    /// From the synced masternode list; `null` before that phase finished.
    pub masternodes: Option<MasternodeCounts>,
    pub evonodes: Option<MasternodeCounts>,
    pub mempool_tx_count: Option<u32>,
    pub mempool_usage_bytes: Option<u64>,
}

impl From<NodeInfo> for NodeSummary {
    fn from(n: NodeInfo) -> Self {
        let NodeInfo {
            user_agent,
            data_dir,
            startup_time,
            network,
            connections_in,
            connections_out,
            local_addresses,
            tip_height,
            tip_time,
            tip_hash,
            best_chainlock,
            masternodes,
            evonodes,
            mempool_tx_count,
            mempool_usage_bytes,
        } = n;
        NodeSummary {
            user_agent,
            data_dir,
            startup_time,
            network: NetworkInfo::from(&network),
            connections_in,
            connections_out,
            local_addresses,
            tip_height,
            tip_time,
            tip_hash,
            best_chainlock: best_chainlock.map(Into::into),
            masternodes: masternodes.map(Into::into),
            evonodes: evonodes.map(Into::into),
            mempool_tx_count,
            mempool_usage_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum WarningCodeName {
    UncleanShutdown,
    SyncStalled,
    ClockSkew,
    PlatformContextUnavailable,
    PrereleaseBuild,
}

mirror_enum!(WarningCode => WarningCodeName {
    UncleanShutdown, SyncStalled, ClockSkew, PlatformContextUnavailable, PrereleaseBuild
});

/// A node warning for the alert banner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NodeWarning {
    pub code: WarningCodeName,
    /// Diagnostic text for logs only.
    pub detail: String,
}

impl From<EngineWarning> for NodeWarning {
    fn from(w: EngineWarning) -> Self {
        let EngineWarning { code, detail } = w;
        NodeWarning {
            code: code.into(),
            detail,
        }
    }
}

/// The running rescan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RescanStatus {
    pub from_height: u32,
    /// The lowest filter checkpoint among the wallets still rescanning.
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    /// UNIX seconds.
    pub started_at: u64,
}

impl From<RescanProgress> for RescanStatus {
    fn from(p: RescanProgress) -> Self {
        let RescanProgress {
            from_height,
            current_height,
            target_height,
            started_at,
        } = p;
        RescanStatus {
            from_height,
            current_height,
            target_height,
            started_at,
        }
    }
}

/// SPV as the window sees it; `starting` covers the Platform bring-up and
/// dash-spv's own start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SpvStateName {
    Stopped,
    Starting,
    Running,
}

mirror_enum!(SpvState => SpvStateName { Stopped, Starting, Running });

/// The outcome of a wallet's DashPay bring-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum StartupStatusName {
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

mirror_enum!(StartupStatus => StartupStatusName {
    NotRun, Starting, Ready, NoIdentity, PartialNoIdentity, DiscoveryFailed,
    PartialAccountsPending, SeedBindingUnverified, IdentityScanIncomplete, IdentityUnsettled
});

/// One wallet's last DashPay bring-up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DashPayStartupInfo {
    pub startup: StartupStatusName,
    /// The wallet has no keys, so DashPay is read-only for it.
    pub read_only: bool,
    /// Base58 id of the identity the bring-up knew, if any.
    pub identity: Option<String>,
    /// Contact-account builds still queued for an unlock or the DIP-15 rescan.
    pub contact_accounts_pending: u32,
    pub identity_scan_incomplete: bool,
    /// UNIX seconds the bring-up ended.
    pub finished_at: Option<u64>,
}

impl From<DashPayStartup> for DashPayStartupInfo {
    fn from(s: DashPayStartup) -> Self {
        let DashPayStartup {
            startup,
            read_only,
            identity,
            contact_accounts_pending,
            identity_scan_incomplete,
            finished_at,
        } = s;
        DashPayStartupInfo {
            startup: startup.into(),
            read_only,
            identity,
            contact_accounts_pending,
            identity_scan_incomplete,
            finished_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum SyncLoopName {
    IdentitySync,
    DashPaySync,
    DpnsSync,
    PlatformAddressSync,
}

mirror_enum!(SyncLoop => SyncLoopName { IdentitySync, DashPaySync, DpnsSync, PlatformAddressSync });

/// One Platform sync loop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformLoop {
    pub sync_loop: SyncLoopName,
    pub running: bool,
    /// UNIX seconds.
    pub last_run_at: Option<u64>,
    pub next_run_at: Option<u64>,
}

impl From<SyncLoopStatus> for PlatformLoop {
    fn from(l: SyncLoopStatus) -> Self {
        let SyncLoopStatus {
            sync_loop,
            running,
            last_run_at,
            next_run_at,
        } = l;
        PlatformLoop {
            sync_loop: sync_loop.into(),
            running,
            last_run_at,
            next_run_at,
        }
    }
}

/// How the window paces the Platform loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformPacing {
    /// A window is visible: `dashpay_sync` every 15 s; hidden, every 60 s.
    pub window_visible: bool,
    /// A watched DPNS contest ends within the hour: its watch polls every
    /// minute instead of every 10.
    pub contest_ending_soon: bool,
}

impl From<PlatformPacing> for PlatformCadence {
    fn from(p: PlatformPacing) -> Self {
        let PlatformPacing {
            window_visible,
            contest_ending_soon,
        } = p;
        PlatformCadence {
            window_visible,
            contest_ending_soon,
        }
    }
}

impl From<PlatformCadence> for PlatformPacing {
    fn from(c: PlatformCadence) -> Self {
        let PlatformCadence {
            window_visible,
            contest_ending_soon,
        } = c;
        PlatformPacing {
            window_visible,
            contest_ending_soon,
        }
    }
}

/// What a proof-verified fetch of the DPNS system contract showed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformReachability {
    /// Base58 id of the DPNS contract.
    pub dpns_contract_id: String,
    pub dpns_contract_version: u32,
    /// Base58 owner identity of the contract.
    pub dpns_contract_owner: String,
    /// The protocol version the SDK settled on.
    pub protocol_version: u32,
}

impl From<PlatformStatus> for PlatformReachability {
    fn from(s: PlatformStatus) -> Self {
        let PlatformStatus {
            dpns_contract_id,
            dpns_contract_version,
            dpns_contract_owner,
            protocol_version,
        } = s;
        PlatformReachability {
            dpns_contract_id,
            dpns_contract_version,
            dpns_contract_owner,
            protocol_version,
        }
    }
}

commands! {
    /// Raw sync state of the active network; the renderer derives the percentage.
    #[facade("NetworkSession::sync_snapshot")]
    fn sync_status(state) -> SyncStatus {
        Ok(state.session().await?.sync_snapshot()?.into())
    }

    /// Connected SPV peers (empty while SPV is stopped).
    #[facade("NetworkSession::peers")]
    fn peers(state) -> Vec<PeerSummary> {
        let peers = state.session().await?.peers()?;
        Ok(peers.into_iter().map(Into::into).collect())
    }

    /// "Change peers": restarts the SPV client, which drops every connection
    /// and dials again. With configured peers only, the same ones are dialled.
    #[facade("NetworkSession::rotate_peers")]
    fn rotate_peers(state) -> () {
        Ok(state.session().await?.rotate_peers().await?)
    }

    /// Schedules a compact-filter rescan of every wallet of the network.
    /// Returns once scheduled; progress arrives as `sync` events. The rewind
    /// is not persisted: after a restart, call it again.
    #[facade("NetworkSession::rescan")]
    fn rescan(state, from: RescanStart) -> () {
        Ok(state.session().await?.rescan(from.into()).await?)
    }

    /// The running rescan, `null` when none runs.
    #[facade("NetworkSession::rescan_progress")]
    fn rescan_progress(state) -> Option<RescanStatus> {
        Ok(state.session().await?.rescan_progress()?.map(Into::into))
    }

    /// dash-qt `abortrescan`: stops the running rescan where it is; the
    /// wallets keep what was found. `false` when no rescan ran.
    #[facade("NetworkSession::cancel_rescan")]
    fn cancel_rescan(state) -> bool {
        Ok(state.session().await?.cancel_rescan().await?)
    }

    /// dash-qt Tools, Information.
    #[facade("NetworkSession::node_info")]
    fn node_info(state) -> NodeSummary {
        Ok(state.session().await?.node_info()?.into())
    }

    /// Node warnings for the alert banner, most severe first.
    #[facade("NetworkSession::warnings")]
    fn warnings(state) -> Vec<NodeWarning> {
        let warnings = state.session().await?.warnings()?;
        Ok(warnings.into_iter().map(Into::into).collect())
    }

    /// "Rebuild index": deletes the SPV chain data and keeps wallets, vault
    /// and app data. SPV must be stopped; the next `startSpv` syncs headers
    /// again and rescans each wallet from its birth height.
    #[facade("NetworkSession::reset_chain_data")]
    fn reset_chain_data(state) -> () {
        Ok(state.session().await?.reset_chain_data().await?)
    }

    /// Edits a wallet's birth height. Lower than its filter checkpoint: a
    /// rescan of the wallet from `height` is scheduled. Above the known
    /// tip: `sync.height_out_of_range`.
    #[facade("NetworkSession::set_birth_height")]
    fn set_birth_height(state, wallet_id: String, height: u32) -> () {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.set_birth_height(id, height).await?)
    }

    /// Starts SPV. Returns once the start is scheduled: the wallets'
    /// DashPay bring-up runs first, `spvState` is `starting` meanwhile and
    /// `spvStateChanged` reports the start. Idempotent while starting or
    /// running.
    #[facade("NetworkSession::start_spv")]
    fn start_spv(state) -> () {
        Ok(state.session().await?.start_spv().await?)
    }

    /// Stops SPV: cancels a bring-up still running, quiesces the Platform
    /// loops, then stops dash-spv. Idempotent.
    #[facade("NetworkSession::stop_spv")]
    fn stop_spv(state) -> () {
        Ok(state.session().await?.stop_spv().await?)
    }

    /// Whether SPV is starting or running (`spvState` tells which).
    #[facade("NetworkSession::spv_running")]
    fn spv_running(state) -> bool {
        Ok(state.session().await?.spv_running()?)
    }

    /// SPV's state, `starting` included.
    #[facade("NetworkSession::spv_state")]
    fn spv_state(state) -> SpvStateName {
        Ok(state.session().await?.spv_state()?.into())
    }

    /// A wallet's last DashPay bring-up (`notRun` before the first one).
    #[facade("NetworkSession::dashpay_startup")]
    fn dashpay_startup(state, wallet_id: String) -> DashPayStartupInfo {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.dashpay_startup(&id)?.into())
    }

    /// Runs a DashPay pass now, in the background (Contacts or the bell
    /// opened, after an unlock, after a DashPay write). Nothing runs while
    /// SPV is stopped; a pass in flight is not repeated.
    #[facade("NetworkSession::dashpay_sync_soon")]
    fn dashpay_sync_soon(state) -> () {
        Ok(state.session().await?.dashpay_sync_soon()?)
    }

    /// The Platform sync loops this session runs.
    #[facade("NetworkSession::platform_loops")]
    fn platform_loops(state) -> Vec<PlatformLoop> {
        let loops = state.session().await?.platform_loops()?;
        Ok(loops.into_iter().map(Into::into).collect())
    }

    /// Paces the Platform loops by window visibility and watched contests.
    /// A window becoming visible also runs a DashPay pass at once.
    #[facade("NetworkSession::set_platform_cadence")]
    fn set_platform_cadence(state, cadence: PlatformPacing) -> () {
        Ok(state.session().await?.set_platform_cadence(cadence.into())?)
    }

    /// One proof-verified fetch of the DPNS system contract: DAPI, TLS, the
    /// quorum context and proof verification end to end.
    #[facade("NetworkSession::platform_status")]
    fn platform_status(state) -> PlatformReachability {
        Ok(state.session().await?.platform_status().await?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rescan_start_is_a_tagged_enum() {
        let height: RescanStart = serde_json::from_str(r#"{"kind":"height","height":42}"#).unwrap();
        assert_eq!(RescanFrom::from(height), RescanFrom::Height(42));
        let birth: RescanStart = serde_json::from_str(r#"{"kind":"walletBirth"}"#).unwrap();
        assert_eq!(RescanFrom::from(birth), RescanFrom::WalletBirth);
        let genesis: RescanStart = serde_json::from_str(r#"{"kind":"genesis"}"#).unwrap();
        assert_eq!(RescanFrom::from(genesis), RescanFrom::Genesis);
        assert_eq!(
            serde_json::to_string(&RescanStart::from(RescanFrom::Height(7))).unwrap(),
            r#"{"kind":"height","height":7}"#
        );
        assert!(serde_json::from_str::<RescanStart>(r#"{"kind":"height"}"#).is_err());
    }

    #[test]
    fn the_cadence_crosses_in_camel_case_both_ways() {
        let pacing: PlatformPacing =
            serde_json::from_str(r#"{"windowVisible":false,"contestEndingSoon":true}"#).unwrap();
        let cadence = PlatformCadence::from(pacing);
        assert!(!cadence.window_visible && cadence.contest_ending_soon);
        assert_eq!(PlatformPacing::from(cadence), pacing);
    }

    #[test]
    fn a_loop_and_a_startup_status_name_themselves_in_camel_case() {
        let l = PlatformLoop::from(SyncLoopStatus {
            sync_loop: SyncLoop::DashPaySync,
            running: true,
            last_run_at: Some(5),
            next_run_at: None,
        });
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["syncLoop"], "dashPaySync");
        assert_eq!(json["lastRunAt"], 5);
        let s = DashPayStartupInfo::from(DashPayStartup {
            startup: StartupStatus::PartialAccountsPending,
            read_only: false,
            identity: Some("id".into()),
            contact_accounts_pending: 2,
            identity_scan_incomplete: true,
            finished_at: None,
        });
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["startup"], "partialAccountsPending");
        assert_eq!(json["contactAccountsPending"], 2);
    }

    #[test]
    fn spv_states_mirror_the_engine() {
        assert_eq!(SpvStateName::from(SpvState::Starting), SpvStateName::Starting);
        assert_eq!(
            serde_json::to_string(&SpvStateName::Running).unwrap(),
            r#""running""#
        );
    }
}
