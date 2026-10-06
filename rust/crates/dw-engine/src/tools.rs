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

    /// The lowest checkpoint among the wallets still rescanning.
    fn current_height(&self) -> Option<u32> {
        self.wallets
            .values()
            .filter(|w| w.reached.unwrap_or(0) < w.resume_height)
            .map(|w| w.reached)
            .min()
            .flatten()
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

/// Whether this engine build is a pre-release (0.x or a `-` suffix), for
/// dash-qt's "This is a pre-release test build" warning.
pub(crate) fn is_prerelease(version: &str) -> bool {
    version.starts_with("0.") || version.contains('-')
}

impl NetworkSession {
    /// The best chain height this session knows: SPV's header tip, or the
    /// highest height a wallet has processed (SPV reports its tip only after
    /// it started).
    pub(crate) fn known_tip(&self) -> u32 {
        let wallets = self
            .hub
            .wallets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|w| w.tip())
            .max()
            .unwrap_or(0);
        self.hub.tracker().tip_height().unwrap_or(0).max(wallets)
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
        if is_prerelease(ENGINE_VERSION) {
            out.push(EngineWarning {
                code: WarningCode::PrereleaseBuild,
                detail: format!("engine version {ENGINE_VERSION}"),
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
            let manager = this.manager()?;
            let Some(state) = this.hub.rescan().take() else {
                return Ok(false);
            };
            if state.finished() {
                return Ok(false);
            }
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            for (id, w) in &state.wallets {
                if let Some(info) = wm.get_wallet_info_mut(&id.0) {
                    let current = info.core_wallet.synced_height();
                    if current < w.resume_height {
                        info.core_wallet.update_synced_height(w.resume_height);
                    }
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

    /// Rewinds the filter checkpoint of `wallets` to just below `start(id)`
    /// and records the rescan for `rescan_progress`. `RescanInProgress` when
    /// one already runs. Caller holds the operation guard.
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
            let checkpoint = start(&id).saturating_sub(1);
            let m = Arc::clone(&manager);
            tokio::task::spawn_blocking(move || m.spv_rescan_filters_blocking(&id.0, checkpoint))
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
            if manager.spv().is_started() {
                return Err(EngineError::SpvRunning);
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
            for id in manager.list_wallet_ids_blocking() {
                let wallet = WalletId(id);
                let birth = this.hub.wallet_state(&wallet).map_or(0, |s| s.birth_height);
                let m = Arc::clone(&manager);
                tokio::task::spawn_blocking(move || {
                    m.spv_rescan_filters_blocking(&id, birth.saturating_sub(1))
                })
                .await?;
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
            if height > this.known_tip() {
                return Err(EngineError::HeightOutOfRange(height));
            }
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
            let network = this.network.core_network();
            tokio::task::spawn_blocking(move || {
                let changeset = PlatformWalletChangeSet {
                    wallet_metadata: Some(platform_wallet::changeset::WalletMetadataEntry {
                        network,
                        // Not stored by the SQLite persister (it keys rows by
                        // the wallet id); the wallet's own id is the
                        // documented fallback.
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
    fn test_qt_040_prerelease_rule() {
        assert!(is_prerelease("0.1.0"));
        assert!(is_prerelease("1.0.0-rc.1"));
        assert!(!is_prerelease("1.2.3"));
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
