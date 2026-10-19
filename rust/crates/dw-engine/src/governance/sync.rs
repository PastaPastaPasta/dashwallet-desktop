//! The governance sync task and its state (MG-02 §2.3, mg-engine §5; not
//! facade). One [`GovernanceRuntime`] per session holds the store the
//! facade reads, the masternode list snapshot it was fed (by a follower
//! that runs whether sync is on or off), and the task running
//! `dw_governance::sync::run` with the backoff of D13, beside the resolver
//! of the collaterals votes name (D80, `collateral.rs`).
//!
//! The follower is the only writer of the store's list and resolved
//! outpoints. Locks are taken in the order `applied`, `list`, store, and
//! none is held across an await.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::Duration;

use dw_governance::params::EVONODE_VOTE_WEIGHT;
use dw_governance::peer::PeerConfig;
use dw_governance::store::{ListEntry, ResolvedMasternode};
use dw_governance::sync::{Phase, Shared, SyncConfig, SyncError, SyncHistory};
use platform_wallet::masternode::MasternodeListSummary;
use rand::seq::SliceRandom;
use tokio::runtime::Handle;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use super::collateral::{self, CollateralSource, Collaterals};
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

/// The resolved outpoints the store was last handed.
#[derive(Debug, Default)]
struct Applied {
    /// [`Collaterals::generation`] they were built from.
    generation: Option<u64>,
    resolved: HashMap<dashcore::OutPoint, ResolvedMasternode>,
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
    /// Where ProRegTxs come from; `None` without Platform (and no regtest
    /// RPC), when every outpoint stays unresolved.
    pub(super) source: Option<Arc<dyn CollateralSource>>,
    pub(super) collaterals: Arc<Collaterals>,
    /// The resolver found collaterals: the follower hands them over.
    pub(super) resolved_changed: Arc<Notify>,
    applied: Mutex<Applied>,
}

impl GovernanceRuntime {
    /// Built at session open on the engine runtime; `enabled` is the stored
    /// setting (the session starts the task after it is built).
    pub fn new(
        network: DashNetwork,
        sink: Arc<dyn EventSink>,
        rt: &Handle,
        enabled: bool,
        source: Option<Arc<dyn CollateralSource>>,
    ) -> Self {
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
            source,
            collaterals: Arc::default(),
            resolved_changed: Arc::default(),
            applied: Mutex::default(),
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
                let resolved = Arc::clone(&s.governance.resolved_changed);
                drop(s);
                tokio::select! {
                    _ = tokio::time::sleep(POLL) => {}
                    _ = resolved.notified() => {}
                }
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

    /// Starts the task unless it runs. A task still stopping is awaited
    /// first, so two never share the store and its last `Disabled` cannot
    /// land after the new task's phase.
    pub fn start(&self, session: &Arc<NetworkSession>) {
        let mut slot = self.task.lock().unwrap_or_else(|p| p.into_inner());
        if slot
            .as_ref()
            .is_some_and(|t| !*t.stop.borrow() && !t.handle.is_finished())
        {
            return;
        }
        let previous = slot.take().map(|t| t.handle);
        let (stop, stop_rx) = watch::channel(false);
        let weak = Arc::downgrade(session);
        let handle = session.rt.spawn(async move {
            if let Some(p) = previous {
                let _ = p.await;
            }
            // Collaterals are fetched only while sync runs (DEC-265).
            tokio::join!(
                sync_loop(weak.clone(), stop_rx.clone()),
                collateral::resolve_loop(weak, stop_rx)
            );
        });
        *slot = Some(Task { stop, handle });
    }

    /// Stops the task (it ends its run and says `Disabled`); it stays in
    /// its slot until it ends or a start replaces it.
    pub fn stop(&self) {
        if let Some(t) = self.task.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
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

    /// Feeds the store the tip, the list when it changed, and the resolved
    /// outpoints when they changed.
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
            let gone = self.apply_list(ListSnapshot {
                height: tip,
                entries,
            });
            let appdb = session.live().ok().map(|l| l.appdb);
            collateral::persist(&appdb, Vec::new(), gone).await;
        }
        self.apply_resolved();
        self.shared.store().set_tip(tip);
    }

    /// Takes a list. A changed one goes to the store with the resolved
    /// outpoints it implies, under one lock and list first (D76 judges a
    /// removed copy by the list it was stored under). Returns the
    /// collaterals of the masternodes it dropped, to delete from app.sqlite.
    pub(super) fn apply_list(&self, snapshot: ListSnapshot) -> Vec<[u8; 32]> {
        let same = snapshot.entries == self.list().entries;
        let mut gone = Vec::new();
        if !same {
            gone = self.collaterals.retain_listed(&snapshot);
            let mut applied = self.applied.lock().unwrap_or_else(|p| p.into_inner());
            let generation = self.collaterals.generation();
            let resolved = self.collaterals.resolved(&snapshot);
            let mut store = self.shared.store();
            store.set_masternode_list(&snapshot.store_entries());
            if resolved != applied.resolved {
                store.set_resolved(resolved.clone());
                applied.resolved = resolved;
            }
            applied.generation = Some(generation);
        }
        *self.list.write().unwrap_or_else(|p| p.into_inner()) = Arc::new(snapshot);
        if !same {
            self.changed();
        }
        gone
    }

    /// Hands the store the resolved outpoints of the current list once the
    /// resolver changed them.
    pub(super) fn apply_resolved(&self) {
        let mut applied = self.applied.lock().unwrap_or_else(|p| p.into_inner());
        let generation = self.collaterals.generation();
        if applied.generation == Some(generation) {
            return;
        }
        let resolved = self.collaterals.resolved(&self.list());
        applied.generation = Some(generation);
        if resolved == applied.resolved {
            return;
        }
        self.shared.store().set_resolved(resolved.clone());
        applied.resolved = resolved;
        drop(applied);
        self.changed();
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
