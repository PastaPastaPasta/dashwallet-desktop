//! In-memory sync state: the snapshot behind `sync_snapshot`, `peers` and
//! the `Sync` event. Fed by dash-spv callbacks; read without I/O.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dash_spv::network::NetworkEvent;
use dash_spv::sync::{ProgressPercentage, SyncProgress, SyncState};

/// SPV made no progress for this long while not caught up (IOS-023).
pub const STALL_AFTER: Duration = Duration::from_secs(45);

/// dash-spv sync phases, in the order they run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncPhase {
    Headers,
    FilterHeaders,
    Filters,
    Masternodes,
}

/// Progress of one phase. Heights are `None` until dash-spv reports the phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPhaseProgress {
    pub phase: SyncPhase,
    pub current_height: Option<u32>,
    pub target_height: Option<u32>,
    pub done: bool,
}

/// Whole-network sync state. Raw values; damping is the host's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSnapshot {
    pub running: bool,
    /// One row per phase, in [`SyncPhase`] order.
    pub phases: Vec<SyncPhaseProgress>,
    /// The phase dash-spv is working on; `None` when idle or caught up.
    pub active_phase: Option<SyncPhase>,
    /// Best stored header height.
    pub tip_height: Option<u32>,
    /// Block time of the best stored header, UNIX seconds.
    pub tip_time: Option<u64>,
    /// Height of the best validated ChainLock.
    pub chainlock_height: Option<u32>,
    pub connected_peers: u32,
    /// dash-spv reached its steady state. See [`caught_up`].
    pub caught_up: bool,
    /// Seconds since the last progress while running and not caught up.
    pub seconds_since_progress: Option<u64>,
}

/// A connected SPV peer. dash-spv reports only addresses, so every other
/// field is `None` until upstream U2 exposes per-peer data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub address: String,
    pub user_agent: Option<String>,
    pub protocol_version: Option<u32>,
    pub best_height: Option<u32>,
    pub ping_ms: Option<u32>,
    /// UNIX seconds this engine first saw the connection.
    pub connected_since: Option<u64>,
    pub inbound: bool,
    pub bytes_sent: Option<u64>,
    pub bytes_received: Option<u64>,
}

/// Where a rescan starts (QT-117, QT-148, IOS-113).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RescanFrom {
    /// Each wallet's own birth height.
    WalletBirth,
    Genesis,
    Height(u32),
}

/// One phase as read from dash-spv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PhaseReading {
    state: SyncState,
    current: u32,
    target: u32,
}

impl PhaseReading {
    fn of<P: ProgressPercentage>(state: SyncState, p: &P) -> Self {
        Self {
            state,
            current: p.current_height(),
            target: p.target_height(),
        }
    }

    /// A phase is done when dash-spv says `Synced`, or when it idles in
    /// `WaitForEvents` with its target reached. The second case is dash-spv's
    /// steady state once fully synced (the iOS gotcha: a synced client sits
    /// in `WaitForEvents` at ≈ 1.0 progress, not in `Synced`).
    fn done(&self) -> bool {
        match self.state {
            SyncState::Synced => true,
            SyncState::WaitForEvents => self.target > 0 && self.current >= self.target,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Readings {
    headers: Option<PhaseReading>,
    filter_headers: Option<PhaseReading>,
    filters: Option<PhaseReading>,
    masternodes: Option<PhaseReading>,
    /// `Blocks` has no heights; only its state matters for `caught_up`.
    blocks_state: Option<SyncState>,
    overall: SyncState,
    chainlock_height: Option<u32>,
}

impl Readings {
    fn from_progress(p: &SyncProgress) -> Self {
        Self {
            headers: p
                .headers()
                .ok()
                .map(|h| PhaseReading::of(h.state(), h))
                // The stored tip, not tip + buffered: heights the UI shows
                // must be stored headers.
                .map(|mut r| {
                    if let Ok(h) = p.headers() {
                        r.current = h.tip_height();
                    }
                    r
                }),
            filter_headers: p
                .filter_headers()
                .ok()
                .map(|f| PhaseReading::of(f.state(), f)),
            filters: p.filters().ok().map(|f| PhaseReading {
                state: f.state(),
                current: f.committed_height(),
                target: f.target_height(),
            }),
            masternodes: p.masternodes().ok().map(|m| PhaseReading {
                state: m.state(),
                current: m.current_height(),
                target: m.target_height(),
            }),
            blocks_state: p.blocks().ok().map(|b| b.state()),
            overall: p.state(),
            chainlock_height: p
                .chainlocks()
                .ok()
                .map(|c| c.best_validated_height())
                .filter(|h| *h > 0),
        }
    }

    fn heights(&self) -> [u32; 4] {
        [
            self.headers.map_or(0, |r| r.current),
            self.filter_headers.map_or(0, |r| r.current),
            self.filters.map_or(0, |r| r.current),
            self.masternodes.map_or(0, |r| r.current),
        ]
    }
}

/// Whether dash-spv is caught up with the network.
///
/// `SyncProgress::is_synced` is true only when every manager says `Synced`.
/// A fully synced dash-spv instead parks its managers in `WaitForEvents`
/// with progress ≈ 1.0, so that state counts as caught up when every phase
/// that has reported is at its target and none is syncing or waiting for
/// connections. Headers must have reported: an idle client that has not
/// started is not caught up.
fn caught_up(r: &Readings) -> bool {
    if matches!(r.overall, SyncState::Synced) {
        return true;
    }
    if !matches!(r.overall, SyncState::WaitForEvents) || r.headers.is_none() {
        return false;
    }
    let phases_done = [r.headers, r.filter_headers, r.filters, r.masternodes]
        .into_iter()
        .flatten()
        .all(|p| p.done());
    let blocks_idle = r
        .blocks_state
        .is_none_or(|s| matches!(s, SyncState::Synced | SyncState::WaitForEvents));
    phases_done && blocks_idle
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PeerEntry {
    connected_since: u64,
}

/// Sync state of one session.
#[derive(Debug)]
pub(crate) struct SyncTracker {
    running: bool,
    readings: Readings,
    tip_time: Option<u64>,
    /// Height `tip_time` belongs to.
    tip_time_height: Option<u32>,
    peers: BTreeMap<SocketAddr, PeerEntry>,
    last_progress: Instant,
    stall_notified: bool,
    /// Masternode and evonode counts of the synced list, with the
    /// masternode-phase height they were read at.
    masternode_counts: Option<(crate::MasternodeCount, crate::MasternodeCount)>,
    masternode_counts_height: Option<u32>,
}

impl Default for SyncTracker {
    fn default() -> Self {
        Self {
            running: false,
            readings: Readings::default(),
            tip_time: None,
            tip_time_height: None,
            peers: BTreeMap::new(),
            last_progress: Instant::now(),
            stall_notified: false,
            masternode_counts: None,
            masternode_counts_height: None,
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl SyncTracker {
    /// SPV started or stopped. Returns whether the snapshot changed.
    pub(crate) fn set_running(&mut self, running: bool) -> bool {
        if self.running == running {
            return false;
        }
        self.running = running;
        self.last_progress = Instant::now();
        self.stall_notified = false;
        if running {
            // A fresh client reports its own progress; a verdict from the
            // previous run must not make it look caught up.
            self.readings = Readings::default();
            self.tip_time_height = None;
        } else {
            // The heights stay: they still describe the stored chain.
            // `caught_up` and the peer list are only reported while running.
            self.peers.clear();
        }
        true
    }

    /// Applies a dash-spv progress report. Returns whether the snapshot changed.
    pub(crate) fn on_progress(&mut self, progress: &SyncProgress) -> bool {
        self.apply(Readings::from_progress(progress))
    }

    fn apply(&mut self, new: Readings) -> bool {
        if new == self.readings {
            return false;
        }
        let advanced = new
            .heights()
            .iter()
            .zip(self.readings.heights().iter())
            .any(|(n, o)| n > o);
        if advanced || caught_up(&new) {
            self.last_progress = Instant::now();
            self.stall_notified = false;
        }
        self.readings = new;
        true
    }

    /// Applies a peer connect/disconnect. Returns whether the snapshot changed.
    pub(crate) fn on_network_event(&mut self, event: &NetworkEvent) -> bool {
        match event {
            NetworkEvent::PeersUpdated { addresses, .. } => {
                let now = unix_now();
                let before: Vec<SocketAddr> = self.peers.keys().copied().collect();
                self.peers.retain(|a, _| addresses.contains(a));
                for a in addresses {
                    self.peers.entry(*a).or_insert(PeerEntry {
                        connected_since: now,
                    });
                }
                before != self.peers.keys().copied().collect::<Vec<_>>()
            }
            NetworkEvent::PeerConnected { address } => {
                let fresh = !self.peers.contains_key(address);
                self.peers.entry(*address).or_insert(PeerEntry {
                    connected_since: unix_now(),
                });
                fresh
            }
            NetworkEvent::PeerDisconnected { address } => self.peers.remove(address).is_some(),
        }
    }

    /// Best stored header height, if dash-spv reported one.
    pub(crate) fn tip_height(&self) -> Option<u32> {
        self.readings.headers.map(|h| h.current).filter(|h| *h > 0)
    }

    pub(crate) fn chainlock_height(&self) -> Option<u32> {
        self.readings.chainlock_height
    }

    /// The header height a tip-time lookup is due for, if any.
    pub(crate) fn tip_time_due(&self) -> Option<u32> {
        let tip = self.tip_height()?;
        (self.tip_time_height != Some(tip)).then_some(tip)
    }

    pub(crate) fn set_tip_time(&mut self, height: u32, time: Option<u64>) {
        self.tip_time_height = Some(height);
        self.tip_time = time;
    }

    /// Forgets every reading (the chain data was deleted). Keeps the
    /// running flag.
    pub(crate) fn reset(&mut self) {
        let running = self.running;
        *self = Self::default();
        self.running = running;
    }

    /// The masternode-phase height a masternode count is due for: the
    /// phase is done and the counts were not read at this height yet.
    pub(crate) fn masternode_counts_due(&self) -> Option<u32> {
        let r = self.readings.masternodes.filter(|r| r.done())?;
        (self.masternode_counts_height != Some(r.current)).then_some(r.current)
    }

    pub(crate) fn set_masternode_counts(
        &mut self,
        height: u32,
        counts: Option<(crate::MasternodeCount, crate::MasternodeCount)>,
    ) {
        self.masternode_counts_height = Some(height);
        self.masternode_counts = counts;
    }

    /// Masternode and evonode counts, once the masternode phase finished.
    pub(crate) fn masternode_counts(
        &self,
    ) -> Option<(crate::MasternodeCount, crate::MasternodeCount)> {
        self.readings
            .masternodes
            .is_some_and(|r| r.done())
            .then_some(self.masternode_counts)
            .flatten()
    }

    /// Seconds without progress while a stall is reported (same rule as
    /// `Notice{SyncStalled}`).
    pub(crate) fn stalled_for(&self) -> Option<u64> {
        (self.running && self.stall_notified && !caught_up(&self.readings))
            .then(|| self.last_progress.elapsed().as_secs())
    }

    /// `true` once per stall: running, not caught up, and no progress for
    /// [`STALL_AFTER`].
    pub(crate) fn check_stall(&mut self) -> bool {
        if !self.running || caught_up(&self.readings) || self.stall_notified {
            return false;
        }
        if self.last_progress.elapsed() >= STALL_AFTER {
            self.stall_notified = true;
            return true;
        }
        false
    }

    pub(crate) fn caught_up(&self) -> bool {
        self.running && caught_up(&self.readings)
    }

    pub(crate) fn snapshot(&self) -> SyncSnapshot {
        let r = &self.readings;
        let row = |phase, reading: Option<PhaseReading>| SyncPhaseProgress {
            phase,
            current_height: reading.map(|p| p.current),
            target_height: reading.map(|p| p.target).filter(|t| *t > 0),
            done: reading.is_some_and(|p| p.done()),
        };
        let phases = vec![
            row(SyncPhase::Headers, r.headers),
            row(SyncPhase::FilterHeaders, r.filter_headers),
            row(SyncPhase::Filters, r.filters),
            row(SyncPhase::Masternodes, r.masternodes),
        ];
        let caught_up = self.caught_up();
        let active_phase = if !self.running || caught_up {
            None
        } else {
            [
                (SyncPhase::Headers, r.headers),
                (SyncPhase::FilterHeaders, r.filter_headers),
                (SyncPhase::Filters, r.filters),
                (SyncPhase::Masternodes, r.masternodes),
            ]
            .into_iter()
            .find(|(_, p)| p.is_some_and(|p| matches!(p.state, SyncState::Syncing)))
            .map(|(phase, _)| phase)
        };
        SyncSnapshot {
            running: self.running,
            phases,
            active_phase,
            tip_height: self.tip_height(),
            tip_time: self.tip_time,
            chainlock_height: r.chainlock_height,
            connected_peers: if self.running {
                u32::try_from(self.peers.len()).unwrap_or(u32::MAX)
            } else {
                0
            },
            caught_up,
            seconds_since_progress: (self.running && !caught_up)
                .then(|| self.last_progress.elapsed().as_secs()),
        }
    }

    pub(crate) fn peers(&self) -> Vec<PeerInfo> {
        if !self.running {
            return Vec::new();
        }
        self.peers
            .iter()
            .map(|(address, entry)| PeerInfo {
                address: address.to_string(),
                user_agent: None,
                protocol_version: None,
                best_height: None,
                ping_ms: None,
                connected_since: Some(entry.connected_since),
                // dash-spv only makes outbound connections.
                inbound: false,
                bytes_sent: None,
                bytes_received: None,
            })
            .collect()
    }
}

impl crate::NetworkSession {
    /// Current sync state. In-memory read; also pushed as `EngineEvent::Sync`.
    pub fn sync_snapshot(&self) -> Result<SyncSnapshot, crate::EngineError> {
        let _op = self.try_enter()?;
        Ok(self.hub.tracker().snapshot())
    }

    /// Connected peers. In-memory read.
    pub fn peers(&self) -> Result<Vec<PeerInfo>, crate::EngineError> {
        let _op = self.try_enter()?;
        Ok(self.hub.tracker().peers())
    }

    /// "Change peers" (IOS-023): restarts the SPV client, which drops every
    /// connection and dials again from dash-spv's peer store and seeds.
    /// dash-spv has no per-peer disconnect, so with configured peers only
    /// (`spv_peers`) the same peers are dialled again.
    pub async fn rotate_peers(self: &std::sync::Arc<Self>) -> Result<(), crate::EngineError> {
        let this = std::sync::Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            // Not beside a start or stop, which would read SPV stopped while
            // it restarts.
            let _lifecycle = this.platform.lifecycle.lock().await;
            if !manager.spv().is_started() {
                return Err(crate::EngineError::SpvNotRunning);
            }
            this.stop_spv_inner(&manager).await?;
            this.start_spv_inner(&manager).await
        })
        .await
    }

    /// Schedules a compact-filter rescan for every wallet of the network
    /// (QT-117, QT-148, IOS-113): rewinds each wallet's filter checkpoint so
    /// the running filter sync re-matches from there. Progress arrives as
    /// `Sync` events; found transactions as `HistoryChanged`. The rewound
    /// checkpoint is in memory only: a restart before the rescan finishes
    /// needs another `rescan`. `RescanInProgress` while an earlier rescan
    /// runs.
    pub async fn rescan(
        self: &std::sync::Arc<Self>,
        from: RescanFrom,
    ) -> Result<(), crate::EngineError> {
        let this = std::sync::Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            if !manager.spv().is_started() {
                return Err(crate::EngineError::SpvNotRunning);
            }
            if let RescanFrom::Height(h) = from
                // No known height yet: nothing to rescan up to (M1 rule).
                && h > this.known_tip().unwrap_or(0)
            {
                return Err(crate::EngineError::HeightOutOfRange(h));
            }
            let ids: Vec<crate::WalletId> = manager
                .list_wallet_ids_blocking()
                .into_iter()
                .map(crate::WalletId)
                .collect();
            let from_height = match from {
                RescanFrom::WalletBirth => ids
                    .iter()
                    .map(|id| this.hub.wallet_state(id).map_or(0, |s| s.birth_height))
                    .min()
                    .unwrap_or(0),
                RescanFrom::Genesis => 0,
                RescanFrom::Height(h) => h,
            };
            let hub = std::sync::Arc::clone(&this.hub);
            this.start_rescan(ids, from_height, move |wallet| match from {
                RescanFrom::WalletBirth => hub.wallet_state(wallet).map_or(0, |s| s.birth_height),
                RescanFrom::Genesis => 0,
                RescanFrom::Height(h) => h,
            })
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use dash_spv::sync::{BlockHeadersProgress, FilterHeadersProgress, MasternodesProgress};

    use super::*;

    fn headers(state: SyncState, tip: u32, target: u32) -> BlockHeadersProgress {
        let mut h = BlockHeadersProgress::default();
        h.update_target_height(target);
        h.update_tip_height(tip);
        h.set_state(state);
        h
    }

    fn filter_headers(state: SyncState, current: u32, target: u32) -> FilterHeadersProgress {
        let mut f = FilterHeadersProgress::default();
        f.update_target_height(target);
        f.update_current_height(current);
        f.set_state(state);
        f
    }

    fn masternodes(state: SyncState, current: u32, target: u32) -> MasternodesProgress {
        let mut m = MasternodesProgress::default();
        m.update_target_height(target);
        m.update_current_height(current);
        m.set_state(state);
        m
    }

    fn running() -> SyncTracker {
        let mut t = SyncTracker::default();
        t.set_running(true);
        t
    }

    #[test]
    fn steady_wait_for_events_at_target_counts_as_caught_up() {
        let mut t = running();
        let mut p = SyncProgress::default();
        p.update_headers(headers(SyncState::WaitForEvents, 1000, 1000));
        p.update_filter_headers(filter_headers(SyncState::WaitForEvents, 1000, 1000));
        p.update_masternodes(masternodes(SyncState::WaitForEvents, 1000, 1000));
        assert!(!p.is_synced(), "dash-spv itself does not call this synced");
        assert!(t.on_progress(&p));
        let s = t.snapshot();
        assert!(s.caught_up);
        assert_eq!(s.active_phase, None);
        assert_eq!(s.tip_height, Some(1000));
        assert!(
            s.phases
                .iter()
                .filter(|p| p.current_height.is_some())
                .all(|p| p.done)
        );
        assert_eq!(s.seconds_since_progress, None);
    }

    #[test]
    fn syncing_phase_is_active_and_not_caught_up() {
        let mut t = running();
        let mut p = SyncProgress::default();
        p.update_headers(headers(SyncState::Synced, 1000, 1000));
        p.update_filter_headers(filter_headers(SyncState::Syncing, 400, 1000));
        t.on_progress(&p);
        let s = t.snapshot();
        assert!(!s.caught_up);
        assert_eq!(s.active_phase, Some(SyncPhase::FilterHeaders));
        assert_eq!(s.phases[1].current_height, Some(400));
        assert_eq!(s.phases[1].target_height, Some(1000));
        assert!(s.seconds_since_progress.is_some());
    }

    #[test]
    fn idle_before_any_target_is_not_caught_up() {
        let mut t = running();
        let mut p = SyncProgress::default();
        p.update_headers(headers(SyncState::WaitForEvents, 0, 0));
        t.on_progress(&p);
        assert!(!t.snapshot().caught_up);
        // A phase behind its target while idle is not done either.
        p.update_headers(headers(SyncState::WaitForEvents, 900, 1000));
        t.on_progress(&p);
        assert!(!t.snapshot().caught_up);
    }

    #[test]
    fn stopped_client_is_never_caught_up_and_has_no_peers() {
        let mut t = running();
        let mut p = SyncProgress::default();
        p.update_headers(headers(SyncState::Synced, 10, 10));
        t.on_progress(&p);
        t.on_network_event(&NetworkEvent::PeersUpdated {
            connected_count: 1,
            addresses: vec!["127.0.0.1:19899".parse().unwrap()],
            best_height: Some(10),
        });
        assert_eq!(t.snapshot().connected_peers, 1);
        assert_eq!(t.peers().len(), 1);
        assert!(t.snapshot().caught_up);
        t.set_running(false);
        let s = t.snapshot();
        assert!(!s.caught_up && !s.running);
        assert_eq!(s.connected_peers, 0);
        assert_eq!(s.tip_height, Some(10), "stored heights stay");
        assert!(t.peers().is_empty());
    }

    #[test]
    fn stall_is_reported_once_and_cleared_by_progress() {
        let mut t = running();
        let mut p = SyncProgress::default();
        p.update_headers(headers(SyncState::Syncing, 10, 1000));
        t.on_progress(&p);
        assert!(!t.check_stall());
        t.last_progress = Instant::now() - STALL_AFTER;
        assert!(t.check_stall());
        assert!(!t.check_stall(), "one notice per stall");
        p.update_headers(headers(SyncState::Syncing, 20, 1000));
        t.on_progress(&p);
        assert!(!t.check_stall());
        t.last_progress = Instant::now() - STALL_AFTER;
        assert!(t.check_stall(), "a new stall is reported again");
    }

    #[test]
    fn peer_events_track_connections() {
        let mut t = running();
        let a: SocketAddr = "10.0.0.1:9999".parse().unwrap();
        let b: SocketAddr = "10.0.0.2:9999".parse().unwrap();
        assert!(t.on_network_event(&NetworkEvent::PeerConnected { address: a }));
        assert!(t.on_network_event(&NetworkEvent::PeersUpdated {
            connected_count: 2,
            addresses: vec![a, b],
            best_height: None,
        }));
        assert!(t.on_network_event(&NetworkEvent::PeerDisconnected { address: a }));
        let peers = t.peers();
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].address, b.to_string());
        assert!(peers[0].connected_since.is_some());
        assert!(peers[0].user_agent.is_none());
    }
}
