//! Tools window (QT-040, QT-117, QT-143, QT-147, QT-148, IOS-113;
//! docs/contracts/m2-engine.md §2.4): node information, warnings, rescan
//! progress and abort, chain-data reset and birth-height edits.
//!
//! Values only a full node has (mempool size and usage, local addresses,
//! inbound connections) are `None` or empty: SPV never estimates them
//! (DESIGN-opus §1.14). Peer disconnect and ban need dash-spv's network
//! manager, which platform-wallet does not expose (upstream U2); those
//! calls return `NotImplemented`.

use std::collections::BTreeMap;
use std::sync::Arc;

use dashcore::BlockHash;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::changeset::{PlatformWalletChangeSet, PlatformWalletPersistence};

use crate::events::unix_now;
use crate::session::SPV_DIR;
use crate::{DashNetwork, EngineError, NetworkSession, WalletId};

/// Version of this engine (`CARGO_PKG_VERSION`).
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The user agent the SPV client announces to peers.
pub const USER_AGENT: &str = concat!("/dashwallet-desktop:", env!("CARGO_PKG_VERSION"), "/");
/// File that exists while a session is open; found at open, it means the
/// previous session did not close cleanly.
pub(crate) const SESSION_MARKER: &str = ".session-open";

/// "Total: X (Enabled: Y)".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasternodeCount {
    pub total: u32,
    pub enabled: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainLockInfo {
    pub height: u32,
    pub block_hash: String,
    pub block_time: Option<u64>,
}

/// dash-qt Tools → Information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    pub user_agent: String,
    pub data_dir: String,
    pub startup_time: u64,
    pub network: DashNetwork,
    pub connections_in: u32,
    pub connections_out: u32,
    pub local_addresses: Vec<String>,
    pub tip_height: Option<u32>,
    pub tip_time: Option<u64>,
    pub tip_hash: Option<String>,
    pub best_chainlock: Option<ChainLockInfo>,
    pub masternodes: Option<MasternodeCount>,
    pub evonodes: Option<MasternodeCount>,
    pub mempool_tx_count: Option<u32>,
    pub mempool_usage_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WarningCode {
    UncleanShutdown,
    SyncStalled,
    ClockSkew,
    PlatformContextUnavailable,
    PrereleaseBuild,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineWarning {
    pub code: WarningCode,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RescanProgress {
    pub from_height: u32,
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub started_at: u64,
}

/// One wallet's part of a running rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalletRescan {
    /// The filter checkpoint before the rescan; the rescan is done for the
    /// wallet once the checkpoint is back here.
    pub resume_height: u32,
    /// Latest checkpoint reported since the rescan started.
    pub reached: Option<u32>,
}

/// What [`NetworkSession::rewind`] does besides storing the replay record.
#[derive(Debug, Clone, Copy)]
enum Rewind {
    /// Lowers the checkpoint: a rescan, an abandon's, Rebuild Index.
    Now,
    /// Only stores the record, ahead of the change that rewinds.
    Ahead,
    /// Resumes a stored replay to `target`, or to the stored checkpoint
    /// when that is above it and not one a fork left (`forked_from`).
    Resume { target: u32, forked_from: u32 },
}

/// A rescan the engine started and has not seen finish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RescanState {
    pub from_height: u32,
    pub started_at: u64,
    pub wallets: BTreeMap<WalletId, WalletRescan>,
}

impl RescanState {
    pub(crate) fn finished(&self) -> bool {
        self.wallets
            .values()
            .all(|w| w.reached.unwrap_or(0) >= w.resume_height)
    }

    /// Whether `wallet` is in the rescan and not back yet.
    pub(crate) fn replaying(&self, wallet: &WalletId) -> bool {
        (self.wallets.get(wallet)).is_some_and(|w| w.reached.unwrap_or(0) < w.resume_height)
    }

    /// The lowest checkpoint among the wallets still rescanning.
    fn current_height(&self) -> Option<u32> {
        self.wallets
            .values()
            .filter(|w| w.reached.unwrap_or(0) < w.resume_height)
            .map(|w| w.reached)
            .min()
            .flatten()
    }

    /// A fork truncated the chain to `height`: `wallet`'s rescan is back
    /// at the new chain's height, not above it.
    pub(crate) fn truncate(&mut self, wallet: &WalletId, height: u32) -> bool {
        match self.wallets.get_mut(wallet) {
            Some(w) => {
                w.resume_height = w.resume_height.min(height);
                w.reached = w.reached.map(|r| r.min(height));
                true
            }
            None => false,
        }
    }

    /// Records a wallet's new checkpoint. Returns whether it belongs to the
    /// rescan.
    pub(crate) fn note_height(&mut self, wallet: &WalletId, height: u32) -> bool {
        match self.wallets.get_mut(wallet) {
            Some(w) => {
                w.reached = Some(height);
                true
            }
            None => false,
        }
    }
}

/// The version this build was released as. The release workflow passes its run's version as
/// `DW_BUILD_VERSION` (`0.1.0`, `0.1.0-rc.1`, `0.1.0-dev.57.g1a2b3c4`); a build without it (local,
/// tests, Nightly) is a dev build.
pub(crate) const BUILD_VERSION: &str = match option_env!("DW_BUILD_VERSION") {
    Some(v) if !v.is_empty() => v,
    _ => concat!(env!("CARGO_PKG_VERSION"), "-dev"),
};

/// Whether a build version is a test build, for dash-qt's "This is a pre-release test build"
/// warning. dash-qt shows it only when CLIENT_VERSION_IS_RELEASE is false. Only a build stamped
/// exactly this crate's X.Y.Z (a tagged vX.Y.Z, 0.x included) is a release; `-rc`, `-dev`, a
/// dry-run or odd tag (`0.1`, `0.2.0rc1`) and an unstamped build all warn.
pub(crate) fn is_test_build(version: &str) -> bool {
    version != ENGINE_VERSION
}

impl NetworkSession {
    /// The best chain height this session knows: SPV's header tip, or the
    /// highest height a wallet has processed (SPV reports its tip only after
    /// it started). `None` when neither is known.
    pub(crate) fn known_tip(&self) -> Option<u32> {
        let wallets = self
            .hub
            .wallets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|w| w.tip())
            .max()
            .unwrap_or(0);
        let tip = self.hub.tracker().tip_height().unwrap_or(0).max(wallets);
        (tip > 0).then_some(tip)
    }

    /// The Information tab. In-memory read.
    pub fn node_info(&self) -> Result<NodeInfo, EngineError> {
        let _op = self.try_enter()?;
        let tracker = self.hub.tracker();
        let snapshot = tracker.snapshot();
        let (masternodes, evonodes) = match tracker.masternode_counts() {
            Some((m, e)) => (Some(m), Some(e)),
            None => (None, None),
        };
        drop(tracker);
        Ok(NodeInfo {
            user_agent: USER_AGENT.to_string(),
            data_dir: self.data_dir().to_string_lossy().into_owned(),
            startup_time: self.opened_at,
            network: self.network.clone(),
            connections_in: 0,
            connections_out: snapshot.connected_peers,
            local_addresses: Vec::new(),
            tip_height: snapshot.tip_height,
            tip_time: snapshot.tip_time,
            tip_hash: None,
            best_chainlock: self
                .hub
                .best_chainlock()
                .map(|(height, hash)| ChainLockInfo {
                    height,
                    block_hash: hash.to_string(),
                    block_time: None,
                }),
            masternodes,
            evonodes,
            mempool_tx_count: None,
            mempool_usage_bytes: None,
        })
    }

    /// Node warnings, most severe first. In-memory read.
    pub fn warnings(&self) -> Result<Vec<EngineWarning>, EngineError> {
        let _op = self.try_enter()?;
        let mut out = Vec::new();
        if self.unclean_previous {
            out.push(EngineWarning {
                code: WarningCode::UncleanShutdown,
                detail: "the previous session of this network did not close cleanly".into(),
            });
        }
        if let Some(since) = self.hub.tracker().stalled_for() {
            out.push(EngineWarning {
                code: WarningCode::SyncStalled,
                detail: format!("no sync progress for {since} s"),
            });
        }
        if !self.platform_context_ready() {
            out.push(EngineWarning {
                code: WarningCode::PlatformContextUnavailable,
                detail: "the trusted quorum service has not answered yet".into(),
            });
        }
        if is_test_build(BUILD_VERSION) {
            out.push(EngineWarning {
                code: WarningCode::PrereleaseBuild,
                detail: format!("build version {BUILD_VERSION}"),
            });
        }
        out.sort_by_key(|w| w.code);
        Ok(out)
    }

    /// The running rescan, `None` when none runs. A finished rescan is
    /// cleared here. In-memory read.
    pub fn rescan_progress(&self) -> Result<Option<RescanProgress>, EngineError> {
        let _op = self.try_enter()?;
        let target = self.hub.tracker().tip_height();
        let mut guard = self.hub.rescan();
        if guard.as_ref().is_some_and(RescanState::finished) {
            *guard = None;
        }
        Ok(guard.as_ref().map(|r| RescanProgress {
            from_height: r.from_height,
            current_height: r.current_height(),
            target_height: target,
            started_at: r.started_at,
        }))
    }

    /// Refuses a spend of `wallet_id`'s coins while a replay runs
    /// (0.1.x-bal §1). A rescan of any kind (the user's, an abandon's,
    /// platform-wallet's DIP-15 backfill after an unlock or in its DashPay
    /// sync loop) lowers key-wallet's filter checkpoint, and outputs spent
    /// above it read unspent until their spends replay. The checkpoint is
    /// read live; the replay is done back at the highest checkpoint seen
    /// (`WalletState::scan_target`: dash-spv advances it only past applied
    /// blocks), raised here to every live read, since the hub hears of a
    /// checkpoint late (a batch committed before a rewind, events a lagging
    /// subscriber of key-wallet's broadcast lost). Returns the checkpoint;
    /// called again with it as `floor` once the coins are selected, a rewind
    /// since refuses too. A rewind replayed back above `floor` by then is not
    /// seen: the network rejects what such a build spends twice.
    pub(crate) async fn replay_guard(
        &self,
        wallet_id: WalletId,
        floor: u32,
    ) -> Result<u32, EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        // The hub is written under the read lock, so a fork's truncation
        // (under the write lock) lowers the target after this raise.
        let state = wallet.state().await;
        let live = state.core_wallet.synced_height();
        let (refused, target, loading) = self.hub.update_wallet_state(wallet_id, |s| {
            let target = s.scan_target;
            s.scan_target = s.scan_target.max(live);
            // A cancel sets its hold under the write lock, above which it
            // leaves the checkpoint: a read at or below it is a later rewind,
            // whose replay covers what the cancel skipped. A stale batch
            // event could not tell, so `update_heights` does not do this.
            s.note_rewind(live);
            let refused = match s.replay_cancelled {
                Some(_) => Some(EngineError::RescanInterrupted),
                None => {
                    (live < floor || s.replaying_at(live)).then_some(EngineError::RescanInProgress)
                }
            };
            (refused, target, s.loading)
        });
        // A wallet being opened has its replay resumed from its record.
        if live < target && !loading {
            self.record_seen_rewind(wallet_id, live, target, &state)
                .await;
        }
        refused.map_or(Ok(live), Err)
    }

    /// A rewind the guard reads that no replay record covers (platform-
    /// wallet's DashPay backfill, which lowers the checkpoint to its
    /// contacts' lowest funding height, payments.rs:179) is recorded from
    /// that floor, else from the birth height: the replay may have passed
    /// blocks since whose stores are not committed. A target above the
    /// known tip (a fork truncated the chain, its event not yet heard) is
    /// lowered to the tip.
    async fn record_seen_rewind(
        &self,
        wallet: WalletId,
        live: u32,
        target: u32,
        info: &platform_wallet::PlatformWalletInfo,
    ) {
        let Ok(session) = self.live() else {
            return;
        };
        session.replay.note_stale(wallet, target);
        let birth = info.core_wallet.birth_height().saturating_sub(1);
        let tip = self.hub.tracker().tip_height();
        let target = tip.map_or(target, |tip| target.min(tip)).max(live);
        let floor = || {
            let contacts = (info.identity_manager.managed_identities())
                .flat_map(|m| m.dashpay().established_contacts().values())
                .map(|c| {
                    (c.outgoing_request.core_height_created_at)
                        .min(c.incoming_request.core_height_created_at)
                })
                .min();
            match contacts {
                Some(funding) if funding <= live => funding,
                _ => birth,
            }
        };
        if let Err(e) = (session.replay)
            .note_seen(wallet, live, target, birth, floor)
            .await
        {
            tracing::warn!(error = %e, wallet = %wallet, "could not store the replay record");
        }
    }

    /// Lowers `wallet`'s filter checkpoint to `checkpoint`, as
    /// `spv_rescan_filters_blocking` (accessors.rs:851-875) does, storing
    /// its replay record first (`crate::replay_record`): a failed write
    /// leaves the checkpoint. The target is the checkpoint read under the
    /// same lock as the rewind (or the highest seen), so no store of a
    /// batch made before the rewind is above it. A wallet with a record is
    /// recorded even when not lowered. Returns the replay's target, `None`
    /// when the wallet is not loaded.
    pub(crate) async fn rewind_recorded(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        checkpoint: u32,
    ) -> Result<Option<u32>, EngineError> {
        self.rewind(manager, wallet, checkpoint, Rewind::Now).await
    }

    /// Stores the record of a rewind of `wallet` to `checkpoint` that a
    /// change about to be stored makes (an abandon, Rebuild Index), before
    /// that change: [`Self::rewind_recorded`] lowers the checkpoint after it.
    pub(crate) async fn record_rewind(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        checkpoint: u32,
    ) -> Result<(), EngineError> {
        self.rewind(manager, wallet, checkpoint, Rewind::Ahead)
            .await
            .map(drop)
    }

    async fn rewind(
        &self,
        manager: &crate::session::Manager,
        wallet: WalletId,
        checkpoint: u32,
        how: Rewind,
    ) -> Result<Option<u32>, EngineError> {
        let replay = self.live()?.replay;
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some(info) = wm.get_wallet_info_mut(&wallet.0) else {
            return Ok(None);
        };
        let current = info.core_wallet.synced_height();
        let lower = checkpoint < current;
        let target = self
            .record_locked(&replay, wallet, info, checkpoint, how)
            .await?;
        if matches!(how, Rewind::Ahead) {
            return Ok(Some(target));
        }
        #[cfg(test)]
        if (replay.crash_after_write).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit after the replay record".into()));
        }
        if lower {
            replay.note_stale(wallet, current);
            info.core_wallet.update_synced_height(checkpoint);
            tracing::info!(wallet = %wallet, checkpoint, target, "rewound the filter checkpoint for a replay");
        }
        self.hub.update_wallet_state(wallet, |s| {
            s.scan_target = match how {
                Rewind::Resume { .. } => target,
                Rewind::Now | Rewind::Ahead => s.scan_target.max(target),
            };
            if lower {
                s.note_rewind(checkpoint);
            }
        });
        Ok(Some(target))
    }

    /// [`Self::record_rewind`], for a caller that holds the manager's
    /// write lock and `wallet`'s info `info` under it.
    pub(crate) async fn record_rewind_locked(
        &self,
        wallet: WalletId,
        info: &platform_wallet::PlatformWalletInfo,
        checkpoint: u32,
    ) -> Result<(), EngineError> {
        let replay = self.live()?.replay;
        self.record_locked(&replay, wallet, info, checkpoint, Rewind::Ahead)
            .await
            .map(drop)
    }

    /// Stores the record of a rewind of `wallet`, whose info `info` the
    /// caller holds under the manager's write lock, to `checkpoint`.
    /// Returns the replay's target.
    async fn record_locked(
        &self,
        replay: &crate::replay_record::ReplayRecords,
        wallet: WalletId,
        info: &platform_wallet::PlatformWalletInfo,
        checkpoint: u32,
        how: Rewind,
    ) -> Result<u32, EngineError> {
        let current = info.core_wallet.synced_height();
        let birth = info.core_wallet.birth_height().saturating_sub(1);
        let target = match how {
            // platform-wallet stores the highest checkpoint: one above the
            // record's target is the replay's (a quit before its record
            // went), unless a fork left it there.
            Rewind::Resume {
                target,
                forked_from,
            } if current <= forked_from => target,
            Rewind::Resume { target, .. } => target.max(current),
            Rewind::Now | Rewind::Ahead => {
                if checkpoint >= current && replay.get(wallet).await?.is_none() {
                    return Ok(current);
                }
                current.max(self.hub.wallet_state(&wallet).map_or(0, |s| s.scan_target))
            }
        };
        Ok(replay
            .note_rewind(wallet, checkpoint.min(current), target, birth)
            .await?
            .target)
    }

    /// Resumes `wallet`'s stored replay as it is loaded, before
    /// SPV scans it: the checkpoint goes back to the record's start and
    /// spends wait for the record's target. A cancelled replay is not
    /// resumed: its hold comes back, no lower than a rescan from the birth
    /// height rewinds to. A record that cannot be read, or a failed resume,
    /// holds spends until such a rescan; the record stays.
    pub(crate) async fn resume_replay(&self, manager: &crate::session::Manager, wallet: WalletId) {
        let Ok(live) = self.live() else {
            return;
        };
        let birth = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            (wm.get_wallet_info(&wallet.0)).map_or(0, |i| i.core_wallet.birth_height())
        };
        let floor = birth.saturating_sub(1);
        let hold = |reached: u32, target: u32| {
            self.hub.update_wallet_state(wallet, |s| {
                s.scan_target = s.scan_target.max(target);
                s.replay_cancelled = Some(s.replay_cancelled.map_or(reached, |h| h.min(reached)));
            });
        };
        let record = match live.replay.get(wallet).await {
            Ok(Some(record)) => record,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "could not read the replay record: spends wait for a rescan from the birth height");
                hold(floor, 0);
                return;
            }
        };
        if record.cancelled {
            let reached = match live.replay.raise_hold(wallet, floor).await {
                Ok(reached) => reached,
                Err(e) => {
                    tracing::warn!(error = %e, wallet = %wallet, "could not store the replay hold at the birth height");
                    record.reached.max(floor)
                }
            };
            hold(reached, record.target);
            return;
        }
        let resumed = (self.rewind(
            manager,
            wallet,
            record.reached,
            Rewind::Resume {
                target: record.target,
                forked_from: record.forked_from,
            },
        ))
        .await;
        match resumed {
            Ok(Some(target)) => {
                let mut rescan = self.hub.rescan();
                if rescan.as_ref().is_some_and(RescanState::finished) {
                    *rescan = None;
                }
                let state = rescan.get_or_insert_with(|| RescanState {
                    from_height: record.reached.saturating_add(1),
                    started_at: unix_now(),
                    wallets: BTreeMap::new(),
                });
                state.from_height = state.from_height.min(record.reached.saturating_add(1));
                state.wallets.insert(
                    wallet,
                    WalletRescan {
                        resume_height: target,
                        reached: None,
                    },
                );
            }
            Ok(None) => {
                tracing::warn!(wallet = %wallet, "replay not resumed: the wallet is not loaded");
                hold(record.reached, record.target);
            }
            Err(e) => {
                tracing::warn!(error = %e, wallet = %wallet, "replay not resumed: spends wait for a rescan");
                hold(record.reached, record.target);
            }
        }
    }

    /// `select` between [`Self::replay_guard`] and its second call.
    pub(crate) async fn replay_guarded<T, E: From<EngineError>>(
        &self,
        wallet_id: WalletId,
        select: impl Future<Output = Result<T, E>>,
    ) -> Result<T, E> {
        let guarded = self.replay_guard(wallet_id, 0).await?;
        let selected = select.await?;
        self.replay_guard(wallet_id, guarded).await?;
        Ok(selected)
    }

    /// Whether a rescan the engine started is still running.
    pub(crate) fn rescan_running(&self) -> bool {
        let mut guard = self.hub.rescan();
        if guard.as_ref().is_some_and(RescanState::finished) {
            *guard = None;
        }
        guard.is_some()
    }

    /// dash-qt `abortrescan`: every wallet's filter checkpoint goes back to
    /// where it was before the rescan (or stays where the rescan has got to
    /// when that is further), so the scan stops; what it found stays.
    /// `false` when no rescan ran.
    pub async fn cancel_rescan(self: &Arc<Self>) -> Result<bool, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let manager = Arc::clone(&live.manager);
            let Some(state) = this.hub.rescan().take() else {
                return Ok(false);
            };
            if state.finished() {
                return Ok(false);
            }
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let mut cancels = Vec::new();
            for (id, w) in &state.wallets {
                if let Some(info) = wm.get_wallet_info(&id.0) {
                    let current = info.core_wallet.synced_height();
                    // Never above the wallet's replay target, which a fork
                    // lowers.
                    let resume_height = (this.hub.wallet_state(id))
                        .map_or(w.resume_height, |s| w.resume_height.min(s.scan_target));
                    if current < resume_height {
                        cancels.push((*id, current, resume_height));
                    }
                }
            }
            // Each hold is stored before its checkpoint moves: one that
            // cannot be leaves the rescan running, as if not cancelled.
            let mut stored = Vec::new();
            for (id, current, resume_height) in &cancels {
                match live.replay.note_cancel(*id, *current, *resume_height).await {
                    Ok(old) => stored.push((*id, old)),
                    Err(e) => {
                        for (id, old) in stored {
                            if let Err(e) = live.replay.restore(id, old).await {
                                tracing::warn!(error = %e, wallet = %id, "could not put back the replay record of a failed cancel");
                            }
                        }
                        let mut rescan = this.hub.rescan();
                        if rescan.is_none() {
                            *rescan = Some(state);
                        }
                        return Err(e);
                    }
                }
            }
            for (id, current, resume_height) in cancels {
                if let Some(info) = wm.get_wallet_info_mut(&id.0) {
                    info.core_wallet.update_synced_height(resume_height);
                    // Under the lock that moved the checkpoint, so no
                    // spend reads it without the hold; the lowest of
                    // two cancels.
                    (this.hub).update_wallet_state(id, |s| {
                        s.replay_cancelled =
                            Some(s.replay_cancelled.map_or(current, |r| r.min(current)));
                    });
                }
            }
            drop(wm);
            for id in state.wallets.keys() {
                this.refresh_wallet_state(&manager, *id).await;
            }
            this.hub.pump.mark_sync();
            Ok(true)
        })
        .await
    }

    /// Sets the wallet's birth height in memory and in wallet.sqlite.
    /// Returns its filter checkpoint.
    async fn store_birth_height(
        &self,
        wallet_id: WalletId,
        height: u32,
    ) -> Result<u32, EngineError> {
        let live = self.live()?;
        let synced = {
            let wm = live.manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm
                .get_wallet_info_mut(&wallet_id.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet_id.to_string()))?;
            info.core_wallet.metadata.birth_height = height;
            info.core_wallet.synced_height()
        };
        let persister = Arc::clone(&live.persister);
        let network = self.network.core_network();
        tokio::task::spawn_blocking(move || {
            let changeset = PlatformWalletChangeSet {
                wallet_metadata: Some(platform_wallet::changeset::WalletMetadataEntry {
                    network,
                    // Not stored by the SQLite persister (it keys rows by the
                    // wallet id); the wallet's own id is the documented
                    // fallback.
                    wallet_group_id: wallet_id.0,
                    birth_height: height,
                }),
                ..Default::default()
            };
            persister.store(wallet_id.0, changeset)?;
            persister.flush(wallet_id.0)
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("birth height: {e}")))?;
        Ok(synced)
    }

    /// Rewinds the filter checkpoint of `wallets` to just below `start(id)`
    /// and records the rescan for `rescan_progress`. dash-spv never scans
    /// below a wallet's birth height, so a rescan from below it lowers the
    /// birth height to the rescan start (stored), as dash-qt's
    /// `rescanblockchain` scans from any height. `RescanInProgress` when one
    /// already runs. Caller holds the operation guard.
    pub(crate) async fn start_rescan(
        &self,
        wallets: Vec<WalletId>,
        from_height: u32,
        start: impl Fn(&WalletId) -> u32,
    ) -> Result<(), EngineError> {
        if self.rescan_running() {
            return Err(EngineError::RescanInProgress);
        }
        let manager = self.manager()?;
        let mut state = RescanState {
            from_height,
            started_at: unix_now(),
            wallets: BTreeMap::new(),
        };
        {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            for id in &wallets {
                if let Some(info) = wm.get_wallet_info(&id.0) {
                    let resume_height = info.core_wallet.synced_height();
                    // The checkpoint is the last scanned height.
                    let checkpoint = start(id).saturating_sub(1);
                    if checkpoint < resume_height {
                        state.wallets.insert(
                            *id,
                            WalletRescan {
                                resume_height,
                                reached: None,
                            },
                        );
                    }
                }
            }
        }
        for id in wallets {
            let from = start(&id);
            let birth = self.hub.wallet_state(&id).map_or(0, |w| w.birth_height);
            if from < birth {
                self.store_birth_height(id, from).await?;
                self.refresh_wallet_state(&manager, id).await;
            }
            self.rewind_recorded(&manager, id, from.saturating_sub(1))
                .await?;
            self.hub.pump.mark_history(id, None);
        }
        if !state.wallets.is_empty() {
            *self.hub.rescan() = Some(state);
        }
        self.hub.pump.mark_sync();
        Ok(())
    }

    /// QT-148 "Rebuild Index" equivalent: deletes the SPV chain data and
    /// keeps wallets, vault and app data. SPV must be stopped. Every
    /// wallet's filter checkpoint goes back below its birth height, so the
    /// next `start_spv` syncs headers again and rescans each wallet.
    pub async fn reset_chain_data(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let _lifecycle = this.platform.lifecycle.lock().await;
            if this.platform_spv_state(&manager) != crate::SpvState::Stopped {
                return Err(EngineError::SpvRunning);
            }
            let wallets: Vec<(WalletId, u32)> = (manager.list_wallet_ids_blocking().into_iter())
                .map(|id| {
                    let wallet = WalletId(id);
                    let birth = this.hub.wallet_state(&wallet).map_or(0, |s| s.birth_height);
                    (wallet, birth.saturating_sub(1))
                })
                .collect();
            for (wallet, checkpoint) in &wallets {
                this.record_rewind(&manager, *wallet, *checkpoint).await?;
            }
            let dir = this.data_dir().join(SPV_DIR);
            tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(EngineError::from(e)),
            })
            .await??;
            *this.hub.rescan() = None;
            this.hub.tracker().reset();
            for (wallet, checkpoint) in wallets {
                this.rewind_recorded(&manager, wallet, checkpoint).await?;
                this.refresh_wallet_state(&manager, wallet).await;
                this.hub.pump.mark_history(wallet, None);
                this.hub.pump.mark_balances(wallet);
            }
            this.hub.pump.mark_sync();
            Ok(())
        })
        .await
    }

    /// IOS-113 "edit birth height": stores `height` as the wallet's birth
    /// height (wallet.sqlite and memory). Lower than the filter checkpoint:
    /// a rescan of this wallet from `height` is scheduled. Above the known
    /// tip: `HeightOutOfRange`.
    pub async fn set_birth_height(
        self: &Arc<Self>,
        wallet_id: WalletId,
        height: u32,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            this.require_wallet(&wallet_id)?;
            // Without a known tip the height cannot be checked.
            if this.known_tip().is_some_and(|tip| height > tip) {
                return Err(EngineError::HeightOutOfRange(height));
            }
            let synced = this.store_birth_height(wallet_id, height).await?;
            this.refresh_wallet_state(&live.manager, wallet_id).await;
            if height.saturating_sub(1) < synced {
                this.start_rescan(vec![wallet_id], height, |_| height)
                    .await?;
            }
            this.hub.pump.mark_balances(wallet_id);
            Ok(())
        })
        .await
    }
}

/// Counts of regular masternodes and evonodes in a masternode list.
pub(crate) fn count_masternodes(
    list: &[platform_wallet::masternode::MasternodeListSummary],
) -> (MasternodeCount, MasternodeCount) {
    let mut mn = MasternodeCount {
        total: 0,
        enabled: 0,
    };
    let mut evo = mn;
    for entry in list {
        let bucket = if entry.is_evonode { &mut evo } else { &mut mn };
        bucket.total += 1;
        if entry.is_valid {
            bucket.enabled += 1;
        }
    }
    (mn, evo)
}

/// Keeps the higher of two ChainLocks.
pub(crate) fn better_chainlock(
    current: Option<(u32, BlockHash)>,
    new: (u32, BlockHash),
) -> Option<(u32, BlockHash)> {
    match current {
        Some(c) if c.0 >= new.0 => Some(c),
        _ => Some(new),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rescan_state_finishes_when_every_wallet_is_back() {
        let a = WalletId([1; 32]);
        let b = WalletId([2; 32]);
        let mut s = RescanState {
            from_height: 10,
            started_at: 0,
            wallets: BTreeMap::from([
                (
                    a,
                    WalletRescan {
                        resume_height: 100,
                        reached: None,
                    },
                ),
                (
                    b,
                    WalletRescan {
                        resume_height: 50,
                        reached: None,
                    },
                ),
            ]),
        };
        assert!(!s.finished());
        assert_eq!(s.current_height(), None);
        assert!(s.note_height(&a, 40));
        assert!(s.note_height(&b, 30));
        assert_eq!(s.current_height(), Some(30));
        s.note_height(&b, 50);
        assert_eq!(s.current_height(), Some(40), "b is done");
        assert!(!s.note_height(&WalletId([3; 32]), 1));
        s.note_height(&a, 120);
        assert!(s.finished());
    }

    #[test]
    fn test_qt_040_test_build_rule() {
        assert!(!is_test_build(ENGINE_VERSION));
        for v in ["-rc.1", "-dev.57.g1a2b3c4", "-dev", ".1", "rc1"] {
            assert!(is_test_build(&format!("{ENGINE_VERSION}{v}")), "{v}");
        }
        for v in ["", "0.1", "0.0.0-rel1", "foo", "9.9.9"] {
            assert!(is_test_build(v), "{v}");
        }
        if option_env!("DW_BUILD_VERSION").is_none() {
            assert_eq!(BUILD_VERSION, concat!(env!("CARGO_PKG_VERSION"), "-dev"));
        }
    }

    #[test]
    fn chainlock_keeps_the_highest() {
        use dashcore::hashes::Hash;
        let h = |n| BlockHash::from_byte_array([n; 32]);
        let c = better_chainlock(None, (10, h(1)));
        let c = better_chainlock(c, (9, h(2)));
        assert_eq!(c, Some((10, h(1))));
        assert_eq!(better_chainlock(c, (11, h(3))), Some((11, h(3))));
    }
}
