//! The governance sync task and its state (MG-02 §2.3, mg-engine §5; not
//! facade). One [`GovernanceRuntime`] per session holds the store the
//! facade reads, the masternode list snapshot it was fed (by a follower
//! that runs whether sync is on or off), and the task running
//! `dw_governance::sync::run` with the backoff of D13.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use dw_governance::params::EVONODE_VOTE_WEIGHT;
use dw_governance::peer::PeerConfig;
use dw_governance::store::ListEntry;
use dw_governance::sync::{Phase, Shared, SyncConfig, SyncError, SyncHistory};
use platform_wallet::masternode::MasternodeListSummary;
use rand::seq::SliceRandom;
use tokio::runtime::Handle;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::events::{EngineEvent, EventSink, unix_now};
use crate::{DashNetwork, NetworkSession};

/// `governance.sync_enabled` in app.sqlite (global scope per network
/// session's own database).
pub(crate) const SYNC_ENABLED_KEY: &str = "governance.sync_enabled";

/// The stored setting; off when absent or unreadable.
pub(crate) fn load_enabled(db: &dw_appdb::AppDb) -> bool {
    match db.setting(dw_appdb::GLOBAL_SCOPE, SYNC_ENABLED_KEY) {
        Ok(v) => v.as_deref() == Some("1"),
        Err(e) => {
            tracing::warn!(error = %e, "could not read governance.sync_enabled; sync stays off");
            false
        }
    }
}

/// The first retry after a failed run; doubles up to [`MAX_BACKOFF`].
const MIN_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// How often the follower looks for a new tip or list, and the sync task
/// for a list to start from.
const POLL: Duration = Duration::from_secs(2);
/// Pause before reconnecting after every peer of a synced run was lost.
const RECONNECT: Duration = Duration::from_secs(5);
/// The `Governance` event's rate (mg-engine §5).
const EVENT_INTERVAL: Duration = Duration::from_secs(1);

/// The tip list as the facade needs it.
#[derive(Debug, Clone, Default)]
pub(crate) struct ListSnapshot {
    pub height: u32,
    pub entries: Vec<MasternodeListSummary>,
}

impl ListSnapshot {
    pub fn weight(e: &MasternodeListSummary) -> u32 {
        if e.is_evonode { EVONODE_VOTE_WEIGHT } else { 1 }
    }

    fn store_entries(&self) -> Vec<ListEntry> {
        self.entries
            .iter()
            .map(|e| ListEntry {
                voting_key_id: e.voting_key_id,
                // A revocation resets the state to the legacy version, whose
                // list entry carries no type; its operator key is null (D34).
                weight: (e.operator_public_key != [0; 48]).then(|| Self::weight(e)),
                valid: e.is_valid,
            })
            .collect()
    }

    /// Full nodes to ask: valid masternodes in random order, then the
    /// configured SPV peers (MG-02 §2.3).
    fn candidates(&self, configured: &[SocketAddr]) -> Vec<SocketAddr> {
        let mut out: Vec<SocketAddr> = self
            .entries
            .iter()
            .filter(|e| e.is_valid)
            .filter_map(|e| e.service_address)
            .collect();
        out.shuffle(&mut rand::thread_rng());
        for a in configured {
            if !out.contains(a) {
                out.push(*a);
            }
        }
        out
    }
}

struct Task {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

pub(crate) struct GovernanceRuntime {
    pub shared: Arc<Shared>,
    history: Arc<Mutex<SyncHistory>>,
    enabled: AtomicBool,
    task: Mutex<Option<Task>>,
    resync: Arc<Notify>,
    pub list: RwLock<Arc<ListSnapshot>>,
    coalescer: JoinHandle<()>,
    follower: Mutex<Option<JoinHandle<()>>>,
    changed: Arc<Notify>,
}

impl GovernanceRuntime {
    /// Built at session open on the engine runtime; `enabled` is the stored
    /// setting (the session starts the task after it is built).
    pub fn new(network: DashNetwork, sink: Arc<dyn EventSink>, rt: &Handle, enabled: bool) -> Self {
        let changed = Arc::new(Notify::new());
        let notify = Arc::clone(&changed);
        let shared = Arc::new(Shared::new(
            network.core_network(),
            Box::new(move || notify.notify_one()),
        ));
        // One event a second at most: a change during the pause leaves a
        // permit, so the trailing edge is sent when it ends.
        let wake = Arc::clone(&changed);
        let coalescer = rt.spawn(async move {
            loop {
                wake.notified().await;
                sink.emit(EngineEvent::Governance {
                    network: network.clone(),
                });
                tokio::time::sleep(EVENT_INTERVAL).await;
            }
        });
        Self {
            shared,
            history: Arc::default(),
            enabled: AtomicBool::new(enabled),
            task: Mutex::default(),
            resync: Arc::default(),
            list: RwLock::default(),
            coalescer,
            follower: Mutex::default(),
            changed,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    pub fn list(&self) -> Arc<ListSnapshot> {
        Arc::clone(&self.list.read().unwrap_or_else(|p| p.into_inner()))
    }

    fn changed(&self) {
        self.changed.notify_one();
    }

    /// Starts feeding the store the tip and list for the session's life,
    /// so the reads have them with sync off too.
    pub fn follow(&self, session: &Arc<NetworkSession>) {
        let weak = Arc::downgrade(session);
        let handle = session.rt.spawn(async move {
            loop {
                let Some(s) = weak.upgrade() else { return };
                s.governance.refresh(&s).await;
                drop(s);
                tokio::time::sleep(POLL).await;
            }
        });
        *self.follower.lock().unwrap_or_else(|p| p.into_inner()) = Some(handle);
    }

    /// Starts or stops the task; the store is kept either way.
    pub fn set_enabled(&self, session: &Arc<NetworkSession>, enabled: bool) {
        self.enabled.store(enabled, Ordering::SeqCst);
        if enabled {
            self.start(session);
        } else {
            self.stop();
        }
    }

    /// Starts the task unless it runs.
    pub fn start(&self, session: &Arc<NetworkSession>) {
        let mut slot = self.task.lock().unwrap_or_else(|p| p.into_inner());
        if slot.as_ref().is_some_and(|t| !t.handle.is_finished()) {
            return;
        }
        let (stop, stop_rx) = watch::channel(false);
        let handle = session
            .rt
            .spawn(sync_loop(Arc::downgrade(session), stop_rx));
        *slot = Some(Task { stop, handle });
    }

    /// Stops the task (it ends its run and says `Disabled`).
    pub fn stop(&self) {
        if let Some(t) = self.task.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = t.stop.send(true);
        }
    }

    /// Ends the current run or backoff and starts a new run.
    pub fn resync(&self) {
        self.resync.notify_one();
    }

    /// Stops the task and the event coalescer for good (session close).
    pub async fn shutdown(&self) {
        let task = self.task.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(t) = task {
            let _ = t.stop.send(true);
            if tokio::time::timeout(Duration::from_secs(5), t.handle)
                .await
                .is_err()
            {
                tracing::warn!("governance sync task did not stop in 5 s");
            }
        }
        if let Some(f) = self
            .follower
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            f.abort();
        }
        self.coalescer.abort();
    }

    /// Feeds the store the tip and, when it changed, the list.
    async fn refresh(&self, session: &NetworkSession) {
        let Some(tip) = session.hub.tracker().tip_height() else {
            return;
        };
        let known = self.list();
        if known.height != tip || known.entries.is_empty() {
            let Ok(manager) = session.manager() else {
                return;
            };
            let Some(entries) = manager.spv().masternode_list_summaries().await else {
                return;
            };
            let snapshot = ListSnapshot {
                height: tip,
                entries,
            };
            let same = snapshot.entries == known.entries;
            if !same {
                self.shared
                    .store()
                    .set_masternode_list(&snapshot.store_entries());
            }
            *self.list.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(snapshot);
            if !same {
                self.changed();
            }
        }
        self.shared.store().set_tip(tip);
    }

    /// The followed tip, once its list is known and not empty.
    fn ready(&self) -> Option<u32> {
        let list = self.list();
        (!list.entries.is_empty()).then_some(list.height)
    }
}

/// Waits `d`, or less on a stop or resync; `true` when stopped.
async fn pause(d: Duration, stop: &mut watch::Receiver<bool>, resync: &Notify) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => false,
        _ = resync.notified() => false,
        r = stop.changed() => r.is_err() || *stop.borrow(),
    }
}

/// The task: wait for a tip and a list, run, back off on failure, repeat
/// until stopped (mg-engine §5).
async fn sync_loop(session: Weak<NetworkSession>, mut stop: watch::Receiver<bool>) {
    let mut backoff = MIN_BACKOFF;
    'runs: loop {
        let Some(s) = session.upgrade() else { return };
        let g = &s.governance;
        let shared = Arc::clone(&g.shared);
        let resync = Arc::clone(&g.resync);
        shared.set_phase(Phase::Waiting);
        // Waiting: a tip and a non-empty list.
        let tip = loop {
            if *stop.borrow() {
                break 'runs;
            }
            if let Some(tip) = g.ready() {
                break tip;
            }
            if pause(POLL, &mut stop, &resync).await {
                break 'runs;
            }
        };
        let network = s.network.core_network();
        let cfg = SyncConfig::new(PeerConfig::new(network, crate::tools::USER_AGENT, tip));
        let candidates = g.list().candidates(&s.spv_peers);
        let history = Arc::clone(&g.history);
        drop(s);

        let synced_before = shared.progress().last_synced_at;
        let (run_stop, run_stop_rx) = watch::channel(false);
        let run =
            dw_governance::sync::run(Arc::clone(&shared), cfg, candidates, history, run_stop_rx);
        tokio::pin!(run);
        let (mut stopped, mut resynced) = (false, false);
        let result = loop {
            tokio::select! {
                r = &mut run => break r,
                _ = resync.notified(), if !resynced => {
                    resynced = true;
                    let _ = run_stop.send(true);
                }
                r = stop.changed(), if !stopped => {
                    if r.is_err() || *stop.borrow() {
                        stopped = true;
                        let _ = run_stop.send(true);
                    }
                }
            }
        };
        if stopped || *stop.borrow() {
            break;
        }
        match result {
            Ok(()) => backoff = MIN_BACKOFF,
            // A synced run that lost its peers reconnects soon (§2.3
            // "Follow"); its progress says Failed until the next run.
            Err(SyncError::PeersLost) if shared.progress().last_synced_at != synced_before => {
                backoff = MIN_BACKOFF;
                if pause(RECONNECT, &mut stop, &resync).await {
                    break;
                }
            }
            Err(e) => {
                tracing::info!(error = %e, "governance sync run failed");
                if resynced {
                    continue;
                }
                let retry_at = unix_now() + backoff.as_secs();
                shared.progress().next_retry_at = Some(retry_at);
                (shared.on_change)();
                let wait = backoff;
                backoff = (backoff * 2).min(MAX_BACKOFF);
                if pause(wait, &mut stop, &resync).await {
                    break;
                }
                shared.progress().next_retry_at = None;
            }
        }
    }
    let Some(s) = session.upgrade() else { return };
    let shared = &s.governance.shared;
    {
        let mut p = shared.progress();
        p.phase = Phase::Disabled;
        p.peers = 0;
        p.next_retry_at = None;
    }
    (shared.on_change)();
}
