//! Per-session governance state: the synced store, the sync task that
//! waits for the chain and the masternode list, picks peers and runs
//! govsync, and the coalesced `Governance` event (≤ 1 Hz, trailing edge).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use dw_appdb::GLOBAL_SCOPE;
use dw_governance::net::PeerConfig;
use dw_governance::proposal::ChainPoint;
use dw_governance::sync::{self, Phase, Shared, SyncConfig, SyncHistory};
use platform_wallet::masternode::MasternodeListSummary;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use super::GovernanceSyncState;
use crate::{EngineError, EngineEvent, NetworkSession};

/// app.sqlite setting that keeps govsync on across restarts.
const SYNC_ENABLED_KEY: &str = "governance.sync_enabled";
/// How often the sync task re-checks its prerequisites.
const WAIT_POLL: Duration = Duration::from_secs(2);
/// Pause after a failed run before the next.
const RETRY_AFTER: Duration = Duration::from_secs(30);
/// Masternodes tried as peers per run (3 connect, the rest are spares).
const PEER_CANDIDATES: usize = 12;

/// The governance part of a session.
pub(crate) struct GovernanceState {
    pub(crate) shared: Arc<Shared>,
    notify: Arc<Notify>,
    enabled: AtomicBool,
    /// The running sync task and its stop signal.
    task: Mutex<Option<(watch::Sender<bool>, JoinHandle<()>)>>,
    /// The event coalescer.
    events: Mutex<Option<JoinHandle<()>>>,
    history: Arc<Mutex<SyncHistory>>,
    /// `budget_allocated / budget_available`, refreshed with the events,
    /// for the in-memory clock read.
    pub(crate) budget_committed: Mutex<Option<f64>>,
}

impl GovernanceState {
    pub(crate) fn new(network: dashcore::Network) -> Self {
        let notify = Arc::new(Notify::new());
        let n = Arc::clone(&notify);
        Self {
            shared: Arc::new(Shared::new(network, Box::new(move || n.notify_one()))),
            notify,
            enabled: AtomicBool::new(false),
            task: Mutex::new(None),
            events: Mutex::new(None),
            history: Arc::new(Mutex::new(SyncHistory::default())),
            budget_committed: Mutex::new(None),
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// Signals a change (pending proposals, own votes) to the event
    /// coalescer.
    pub(crate) fn changed(&self) {
        self.notify.notify_one();
    }

    pub(crate) fn sync_state(&self) -> GovernanceSyncState {
        let p = self.shared.progress().clone();
        GovernanceSyncState {
            phase: if self.enabled() { p.phase } else { Phase::Disabled },
            objects: p.objects,
            votes: p.votes,
            peers: p.peers,
            bytes_received: p.bytes_received,
            last_synced_at: p.last_synced_at,
            last_error: p.last_error,
            objects_secs: p.objects_secs,
            votes_secs: p.votes_secs,
        }
    }

    fn stop_task(&self) -> Option<JoinHandle<()>> {
        let taken = self.task.lock().unwrap_or_else(|p| p.into_inner()).take();
        taken.map(|(stop, handle)| {
            let _ = stop.send(true);
            handle
        })
    }

    /// Stops the sync and the event task (session close).
    pub(crate) async fn shutdown(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        if let Some(h) = self.stop_task() {
            let _ = h.await;
        }
        if let Some(h) = self.events.lock().unwrap_or_else(|p| p.into_inner()).take() {
            h.abort();
        }
    }
}

impl NetworkSession {
    /// Starts the event coalescer and, when the setting says so, govsync
    /// (session open).
    pub(crate) async fn start_governance(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let notify = Arc::clone(&self.governance.notify);
        let events = self.rt.spawn(async move {
            loop {
                notify.notified().await;
                let Some(session) = weak.upgrade() else { return };
                session.refresh_budget_committed().await;
                session.sink.emit(EngineEvent::Governance {
                    network: session.network.clone(),
                });
                drop(session);
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        *self.governance.events.lock().unwrap_or_else(|p| p.into_inner()) = Some(events);
        let on = self
            .appdb_op(|db| db.setting(GLOBAL_SCOPE, SYNC_ENABLED_KEY))
            .await
            .ok()
            .flatten()
            .is_some_and(|v| v == "1");
        if on {
            self.spawn_governance_sync();
        }
    }

    fn spawn_governance_sync(self: &Arc<Self>) {
        let mut task = self.governance.task.lock().unwrap_or_else(|p| p.into_inner());
        if task.is_some() {
            return;
        }
        self.governance.enabled.store(true, Ordering::SeqCst);
        let (stop_tx, stop_rx) = watch::channel(false);
        let handle = self.rt.spawn(drive(Arc::downgrade(self), stop_rx));
        *task = Some((stop_tx, handle));
        self.governance.shared.set_phase(Phase::Waiting);
    }

    /// Turns govsync on or off and stores the choice.
    pub async fn set_governance_sync_enabled(self: &Arc<Self>, enabled: bool) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let value = enabled.then_some("1".to_string());
            this.appdb_op(move |db| db.set_setting(GLOBAL_SCOPE, SYNC_ENABLED_KEY, value.as_deref()))
                .await?;
            if enabled {
                this.spawn_governance_sync();
            } else {
                this.governance.enabled.store(false, Ordering::SeqCst);
                if let Some(h) = this.governance.stop_task() {
                    let _ = h.await;
                }
                this.governance.shared.set_phase(Phase::Disabled);
            }
            this.governance.changed();
            Ok(())
        })
        .await
    }

    /// In-memory read of the sync state.
    pub fn governance_sync_state(&self) -> Result<GovernanceSyncState, EngineError> {
        let _op = self.try_enter()?;
        Ok(self.governance.sync_state())
    }

    /// The chain tip as the governance views use it: the best header and
    /// the later of its time and now.
    pub(crate) fn governance_tip(&self) -> Option<ChainPoint> {
        let snap = self.hub.tracker().snapshot();
        let height = snap.tip_height?;
        let now = crate::events::unix_now();
        Some(ChainPoint {
            height,
            time: snap.tip_time.unwrap_or(now).max(now),
        })
    }

    /// The SPV masternode list, when synced.
    pub(crate) async fn governance_masternode_list(&self) -> Option<Vec<MasternodeListSummary>> {
        let manager = self.manager().ok()?;
        manager.spv().masternode_list_summaries().await
    }

    /// Where governance messages go: the configured SPV peers when the
    /// session is restricted to them (regtest, devnet), else valid
    /// masternodes of the list with a service address, else the connected
    /// SPV peers.
    pub(crate) async fn governance_peer_candidates(&self) -> Vec<SocketAddr> {
        if !self.spv_peers.is_empty() {
            return self.spv_peers.clone();
        }
        if let Some(list) = self.governance_masternode_list().await {
            let mut addrs: Vec<SocketAddr> = list
                .iter()
                .filter(|m| m.is_valid)
                .filter_map(|m| m.service_address)
                .collect();
            // A different subset each run, without a random number source:
            // rotate by the clock.
            if !addrs.is_empty() {
                let shift = (crate::events::unix_now() as usize) % addrs.len();
                addrs.rotate_left(shift);
            }
            addrs.truncate(PEER_CANDIDATES);
            if !addrs.is_empty() {
                return addrs;
            }
        }
        self.hub
            .tracker()
            .peers()
            .into_iter()
            .filter_map(|p| p.address.parse().ok())
            .collect()
    }

    pub(crate) fn governance_sync_config(&self, start_height: u32) -> SyncConfig {
        SyncConfig::new(PeerConfig::new(
            self.network.core_network(),
            crate::tools::USER_AGENT,
            start_height,
        ))
    }
}

/// The sync task: waits for the tip and the masternode list, runs govsync,
/// retries after failures, until stopped.
async fn drive(session: Weak<NetworkSession>, mut stop: watch::Receiver<bool>) {
    loop {
        if *stop.borrow() {
            return;
        }
        let Some(s) = session.upgrade() else { return };
        let shared = Arc::clone(&s.governance.shared);
        let history = Arc::clone(&s.governance.history);
        let tip = s.governance_tip();
        let ready = tip.is_some() && s.governance_masternode_list().await.is_some();
        let candidates = if ready {
            s.governance_peer_candidates().await
        } else {
            Vec::new()
        };
        let cfg = s.governance_sync_config(tip.map_or(0, |t| t.height));
        drop(s);
        if candidates.is_empty() {
            shared.set_phase(Phase::Waiting);
            if wait_or_stop(&mut stop, WAIT_POLL).await {
                return;
            }
            continue;
        }
        match sync::run(Arc::clone(&shared), cfg, candidates, history, stop.clone()).await {
            Ok(()) => return,
            Err(e) => {
                tracing::warn!(error = %e, "governance sync run ended");
                shared.progress().last_error = Some(e.to_string());
                shared.set_phase(Phase::Failed);
                if wait_or_stop(&mut stop, RETRY_AFTER).await {
                    return;
                }
            }
        }
    }
}

/// Sleeps `d`; true when stopped meanwhile.
async fn wait_or_stop(stop: &mut watch::Receiver<bool>, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => *stop.borrow(),
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}
