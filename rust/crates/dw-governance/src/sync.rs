//! govsync: downloading governance objects and the votes of current
//! proposals from full-node peers, then following new ones as peers relay
//! them; and relaying the wallet's own objects and votes.
//!
//! Core's server side (`governance/net_governance.cpp`): a `govsync` with a
//! zero hash answers with `ssc(10, n)` and `n` object invs; with an object
//! hash and a non-empty bloom filter, `ssc(11, n)` and the vote invs not in
//! the filter. Items are then fetched with `getdata`. A peer refuses (and
//! penalizes) a second full sync from the same address within its
//! fulfilled-request window, so a reconnect to a peer synced earlier in the
//! session skips the request and only follows relays. Objects and votes are
//! accepted by a peer only after it announced or requested them, so the
//! wallet relays its own by `inv` and answers the peer's `getdata`.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::Network;
use tokio::sync::{mpsc, watch};

use crate::net::{
    INV_GOVERNANCE_OBJECT, INV_GOVERNANCE_VOTE, InvItem, Peer, PeerConfig, PeerEvent, SSC_GOVOBJ,
    SSC_GOVOBJ_VOTE, empty_vote_filter, object_fetch_filter,
};
use crate::object::GovernanceObject;
use crate::store::GovernanceStore;
use crate::vote::GovernanceVote;

/// Where a sync is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Disabled,
    Waiting,
    SyncingObjects,
    SyncingVotes,
    Synced,
    Failed,
}

/// Progress counters (QT-128, QT-134, the G7 measurement).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    pub phase: Phase,
    pub objects: u32,
    pub votes: u64,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
    pub last_error: Option<String>,
    /// Wall time of the objects and votes phases of the last full sync.
    pub objects_secs: Option<f64>,
    pub votes_secs: Option<f64>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: Phase::Disabled,
            objects: 0,
            votes: 0,
            peers: 0,
            bytes_received: 0,
            last_synced_at: None,
            last_error: None,
            objects_secs: None,
            votes_secs: None,
        }
    }
}

/// State shared between the sync task and readers.
pub struct Shared {
    pub network: Network,
    pub store: Mutex<GovernanceStore>,
    pub progress: Mutex<Progress>,
    /// Called after the store or the progress changed (the engine
    /// coalesces it into its `Governance` event).
    pub on_change: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    pub fn new(network: Network, on_change: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            network,
            store: Mutex::new(GovernanceStore::new()),
            progress: Mutex::new(Progress::default()),
            on_change,
        }
    }

    pub fn store(&self) -> std::sync::MutexGuard<'_, GovernanceStore> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn set_phase(&self, phase: Phase) {
        let changed = {
            let mut p = self.progress();
            let changed = p.phase != phase;
            p.phase = phase;
            changed
        };
        if changed {
            (self.on_change)();
        }
    }

    fn refresh_counts(&self, peers: &[Peer], base_bytes: u64) {
        let (objects, votes) = {
            let s = self.store();
            (s.object_count() as u32, s.vote_count())
        };
        let mut p = self.progress();
        p.objects = objects;
        p.votes = votes;
        p.peers = peers.len() as u32;
        p.bytes_received = base_bytes + peers.iter().map(Peer::bytes_received).sum::<u64>();
    }
}

/// Sync settings.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub peer: PeerConfig,
    /// How many peers to sync from.
    pub max_peers: usize,
    /// A phase ends when nothing arrived for this long.
    pub idle_timeout: Duration,
    /// Most items asked for in one `getdata`.
    pub getdata_batch: usize,
}

impl SyncConfig {
    pub fn new(peer: PeerConfig) -> Self {
        Self {
            peer,
            max_peers: 3,
            idle_timeout: Duration::from_secs(20),
            getdata_batch: 1000,
        }
    }
}

/// Why a sync run ended.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("no peer could be reached: {0}")]
    NoPeers(String),
    #[error("every peer disconnected")]
    PeersLost,
}

/// Peers this session already full-synced from, with when (Core penalizes a
/// repeat within its fulfilled-request window).
#[derive(Debug, Default)]
pub struct SyncHistory {
    full_synced: HashMap<String, Instant>,
}

/// Core's fulfilled-request window for `govsync`.
const REPEAT_WINDOW: Duration = Duration::from_secs(60 * 60);

impl SyncHistory {
    fn recently_synced(&self, addr: &str) -> bool {
        self.full_synced
            .get(addr)
            .is_some_and(|t| t.elapsed() < REPEAT_WINDOW)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Connects to up to `max` of `candidates` in parallel.
async fn connect_peers(
    candidates: &[SocketAddr],
    cfg: &SyncConfig,
    events: &mpsc::UnboundedSender<(usize, PeerEvent)>,
) -> (Vec<Peer>, Vec<String>) {
    let mut peers = Vec::new();
    let mut errors = Vec::new();
    for (round, chunk) in candidates.chunks(cfg.max_peers.max(1)).enumerate() {
        let first_id = round * cfg.max_peers.max(1);
        let tries = chunk.iter().enumerate().map(|(i, addr)| {
            let events = events.clone();
            let pc = cfg.peer.clone();
            let id = first_id + i;
            let addr = *addr;
            async move { Peer::connect(id, addr, &pc, events).await }
        });
        for r in futures_join_all(tries).await {
            match r {
                Ok(p) if peers.len() < cfg.max_peers => peers.push(p),
                Ok(_) => {}
                Err(e) => errors.push(e.to_string()),
            }
        }
        if peers.len() >= cfg.max_peers {
            break;
        }
    }
    (peers, errors)
}

/// `join_all` without the futures crate: spawns each on the runtime.
async fn futures_join_all<F, T>(futs: impl Iterator<Item = F>) -> Vec<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    out
}

/// Requests in flight: item → peer id.
#[derive(Default)]
struct Requests {
    pending: HashMap<[u8; 32], usize>,
}

impl Requests {
    /// Asks `peer` for the items of `items` the store lacks and nobody was
    /// asked for yet.
    fn request(&mut self, peer: &Peer, items: &[InvItem], store: &GovernanceStore, batch: usize) {
        let wanted: Vec<InvItem> = items
            .iter()
            .filter(|i| match i.kind {
                INV_GOVERNANCE_OBJECT => !store.has_object(&i.hash),
                INV_GOVERNANCE_VOTE => !store.has_vote(&i.hash),
                _ => false,
            })
            .filter(|i| !self.pending.contains_key(&i.hash))
            .copied()
            .collect();
        for chunk in wanted.chunks(batch.max(1)) {
            for i in chunk {
                self.pending.insert(i.hash, peer.id);
            }
            peer.send_getdata(chunk);
        }
    }

    fn done(&mut self, hash: &[u8; 32]) {
        self.pending.remove(hash);
    }

    fn drop_peer(&mut self, id: usize) {
        self.pending.retain(|_, p| *p != id);
    }
}

/// Applies one peer event to the store; returns whether the store changed.
fn apply(
    shared: &Shared,
    peers: &[Peer],
    from: usize,
    event: &PeerEvent,
    requests: &mut Requests,
    batch: usize,
) -> bool {
    match event {
        PeerEvent::Inv(items) => {
            if let Some(peer) = peers.iter().find(|p| p.id == from) {
                let store = shared.store();
                requests.request(peer, items, &store, batch);
            }
            false
        }
        PeerEvent::Object(payload) => {
            let Ok(obj) = GovernanceObject::decode(payload) else {
                return false;
            };
            requests.done(&obj.hash());
            shared.store().add_object(obj, shared.network)
        }
        PeerEvent::Vote(payload) => {
            let Ok(vote) = GovernanceVote::decode(payload) else {
                return false;
            };
            requests.done(&vote.hash());
            shared.store().add_vote(vote, shared.network).is_ok()
        }
        PeerEvent::GetData(_) | PeerEvent::SyncCount { .. } | PeerEvent::Closed(_) => false,
    }
}

/// One sync run: connect, download objects and current votes from peers
/// not synced recently, then follow relays until `stop` turns true or every
/// peer disconnected. `base_bytes` carries the byte counter of earlier
/// runs.
pub async fn run(
    shared: Arc<Shared>,
    cfg: SyncConfig,
    candidates: Vec<SocketAddr>,
    history: Arc<Mutex<SyncHistory>>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), SyncError> {
    let base_bytes = shared.progress().bytes_received;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (mut peers, errors) = connect_peers(&candidates, &cfg, &tx).await;
    if peers.is_empty() {
        return Err(SyncError::NoPeers(if errors.is_empty() {
            "no candidate peers".into()
        } else {
            errors.join("; ")
        }));
    }
    shared.refresh_counts(&peers, base_bytes);
    let mut requests = Requests::default();

    let fresh: Vec<usize> = {
        let h = history.lock().unwrap_or_else(|p| p.into_inner());
        peers
            .iter()
            .filter(|p| !h.recently_synced(&p.addr))
            .map(|p| p.id)
            .collect()
    };

    // Objects.
    if let Some(&primary) = fresh.first() {
        shared.set_phase(Phase::SyncingObjects);
        let started = Instant::now();
        let p = peers.iter().find(|p| p.id == primary).expect("connected");
        p.send_govsync([0; 32], &object_fetch_filter());
        history
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .full_synced
            .insert(p.addr.clone(), Instant::now());
        let mut expected: Option<i32> = None;
        let mut announced = 0i32;
        let outcome = drain(
            &shared,
            &mut peers,
            &mut rx,
            &mut requests,
            &cfg,
            &mut stop,
            base_bytes,
            |from, ev| {
                match ev {
                    PeerEvent::SyncCount { item, count }
                        if *item == SSC_GOVOBJ && from == primary =>
                    {
                        expected = Some(*count);
                    }
                    PeerEvent::Inv(items) if from == primary => {
                        announced += items
                            .iter()
                            .filter(|i| i.kind == INV_GOVERNANCE_OBJECT)
                            .count() as i32;
                    }
                    _ => {}
                }
                expected.is_some_and(|n| announced >= n)
            },
        )
        .await;
        if let Drained::Stopped = outcome {
            return Ok(());
        }
        shared.progress().objects_secs = Some(started.elapsed().as_secs_f64());

        // Votes of the proposals still running, spread over the fresh peers.
        shared.set_phase(Phase::SyncingVotes);
        let started = Instant::now();
        let current = shared.store().current_proposals(unix_now());
        let filter = empty_vote_filter();
        let vote_peers: Vec<usize> = fresh.clone();
        for (i, hash) in current.iter().enumerate() {
            let id = vote_peers[i % vote_peers.len()];
            if let Some(p) = peers.iter().find(|p| p.id == id) {
                p.send_govsync(*hash, &filter);
            }
        }
        let mut answers = 0usize;
        let mut expected_votes = 0i64;
        let mut announced_votes = 0i64;
        let total = current.len();
        let outcome = if total == 0 {
            Drained::Done
        } else {
            drain(
                &shared,
                &mut peers,
                &mut rx,
                &mut requests,
                &cfg,
                &mut stop,
                base_bytes,
                |_, ev| {
                    match ev {
                        PeerEvent::SyncCount { item, count } if *item == SSC_GOVOBJ_VOTE => {
                            answers += 1;
                            expected_votes += i64::from(*count);
                        }
                        PeerEvent::Inv(items) => {
                            announced_votes += items
                                .iter()
                                .filter(|i| i.kind == INV_GOVERNANCE_VOTE)
                                .count() as i64;
                        }
                        _ => {}
                    }
                    answers >= total && announced_votes >= expected_votes
                },
            )
            .await
        };
        if let Drained::Stopped = outcome {
            return Ok(());
        }
        shared.progress().votes_secs = Some(started.elapsed().as_secs_f64());
    }

    {
        let mut p = shared.progress();
        p.phase = Phase::Synced;
        p.last_synced_at = Some(unix_now() as u64);
        p.last_error = None;
    }
    shared.refresh_counts(&peers, base_bytes);
    (shared.on_change)();

    // Follow relays.
    loop {
        if peers.is_empty() {
            return Err(SyncError::PeersLost);
        }
        tokio::select! {
            // A dropped sender counts as stop (no busy loop on `Err`).
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() { return Ok(()); }
            }
            ev = rx.recv() => {
                let Some((from, ev)) = ev else { return Err(SyncError::PeersLost) };
                if let PeerEvent::Closed(_) = ev {
                    peers.retain(|p| p.id != from);
                    requests.drop_peer(from);
                    shared.refresh_counts(&peers, base_bytes);
                    (shared.on_change)();
                    continue;
                }
                let changed = apply(&shared, &peers, from, &ev, &mut requests, cfg.getdata_batch);
                shared.refresh_counts(&peers, base_bytes);
                if changed { (shared.on_change)(); }
            }
        }
    }
}

enum Drained {
    Done,
    Idle,
    Stopped,
}

/// Processes events until `finished` says the phase is complete and no
/// request is outstanding, nothing arrived for the idle timeout, or `stop`.
#[allow(clippy::too_many_arguments)]
async fn drain(
    shared: &Shared,
    peers: &mut Vec<Peer>,
    rx: &mut mpsc::UnboundedReceiver<(usize, PeerEvent)>,
    requests: &mut Requests,
    cfg: &SyncConfig,
    stop: &mut watch::Receiver<bool>,
    base_bytes: u64,
    mut finished: impl FnMut(usize, &PeerEvent) -> bool,
) -> Drained {
    let mut complete = false;
    let mut last_change = Instant::now();
    loop {
        if complete && requests.pending.is_empty() {
            return Drained::Done;
        }
        if peers.is_empty() {
            return Drained::Idle;
        }
        let wait = cfg.idle_timeout.saturating_sub(last_change.elapsed());
        tokio::select! {
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() { return Drained::Stopped; }
            }
            _ = tokio::time::sleep(wait) => return Drained::Idle,
            ev = rx.recv() => {
                let Some((from, ev)) = ev else { return Drained::Idle };
                last_change = Instant::now();
                if let PeerEvent::Closed(_) = ev {
                    peers.retain(|p| p.id != from);
                    requests.drop_peer(from);
                    continue;
                }
                complete |= finished(from, &ev);
                let changed = apply(shared, peers, from, &ev, requests, cfg.getdata_batch);
                shared.refresh_counts(peers, base_bytes);
                if changed { (shared.on_change)(); }
            }
        }
    }
}

/// What a relay achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayReport {
    /// Peers the items were announced to.
    pub announced_to: usize,
    /// Items (by hash) at least one peer fetched.
    pub fetched: HashSet<[u8; 32]>,
    pub errors: Vec<String>,
}

/// Announces `items` (`kind`, hash, payload) to up to `cfg.max_peers` of
/// `candidates` and serves their `getdata` until every item was fetched by
/// some peer or `wait` passed.
pub async fn relay(
    cfg: &SyncConfig,
    candidates: &[SocketAddr],
    items: Vec<(u32, [u8; 32], Vec<u8>)>,
    wait: Duration,
) -> RelayReport {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (peers, errors) = connect_peers(candidates, cfg, &tx).await;
    let mut report = RelayReport {
        announced_to: peers.len(),
        fetched: HashSet::new(),
        errors,
    };
    if peers.is_empty() {
        return report;
    }
    let invs: Vec<InvItem> = items
        .iter()
        .map(|(kind, hash, _)| InvItem {
            kind: *kind,
            hash: *hash,
        })
        .collect();
    for p in &peers {
        p.send_inv(&invs);
    }
    let by_hash: HashMap<[u8; 32], (u32, Vec<u8>)> = items
        .into_iter()
        .map(|(k, h, payload)| (h, (k, payload)))
        .collect();
    let deadline = tokio::time::Instant::now() + wait;
    while report.fetched.len() < by_hash.len() {
        let ev = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            ev = rx.recv() => ev,
        };
        let Some((from, ev)) = ev else { break };
        if let PeerEvent::GetData(wanted) = ev
            && let Some(peer) = peers.iter().find(|p| p.id == from)
        {
            for w in wanted {
                if let Some((kind, payload)) = by_hash.get(&w.hash) {
                    match *kind {
                        INV_GOVERNANCE_OBJECT => peer.send_object(payload),
                        _ => peer.send_vote(payload),
                    }
                    report.fetched.insert(w.hash);
                }
            }
        }
    }
    // Give the writers a moment to flush the last answers.
    tokio::time::sleep(Duration::from_millis(500)).await;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::test_peer;

    /// The sync against an in-process full node: objects, then votes.
    #[tokio::test]
    async fn test_qt_128_full_sync_then_relay() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let obj = GovernanceObject::new_proposal(
            1,
            1,
            format!(
                r#"{{"name":"p","end_epoch":{},"type":1}}"#,
                unix_now() + 10_000
            )
            .into_bytes(),
        );
        let obj_hash = obj.hash();
        let server = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut fake = test_peer::spawn(sock, Network::Regtest);
            while let Some((cmd, payload)) = fake.received.recv().await {
                match cmd.as_str() {
                    "govsync" if payload[..32] == [0; 32] => {
                        let mut ssc = SSC_GOVOBJ.to_le_bytes().to_vec();
                        ssc.extend_from_slice(&1i32.to_le_bytes());
                        fake.send.send(("ssc".into(), ssc)).unwrap();
                        let inv = test_peer::inv_payload(&[InvItem {
                            kind: INV_GOVERNANCE_OBJECT,
                            hash: obj_hash,
                        }]);
                        fake.send.send(("inv".into(), inv)).unwrap();
                    }
                    "govsync" => {
                        let mut ssc = SSC_GOVOBJ_VOTE.to_le_bytes().to_vec();
                        ssc.extend_from_slice(&0i32.to_le_bytes());
                        fake.send.send(("ssc".into(), ssc)).unwrap();
                    }
                    "getdata" => {
                        for i in test_peer::parse_inv(&payload) {
                            if i.hash == obj_hash {
                                fake.send.send(("govobj".into(), obj.encode())).unwrap();
                            }
                        }
                    }
                    _ => {}
                }
            }
        });
        let shared = Arc::new(Shared::new(Network::Regtest, Box::new(|| {})));
        let mut cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
        cfg.idle_timeout = Duration::from_secs(5);
        let (stop_tx, stop_rx) = watch::channel(false);
        let history = Arc::new(Mutex::new(SyncHistory::default()));
        let task = tokio::spawn(run(
            Arc::clone(&shared),
            cfg,
            vec![addr],
            Arc::clone(&history),
            stop_rx,
        ));
        for _ in 0..100 {
            if shared.progress().phase == Phase::Synced {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(shared.progress().phase, Phase::Synced);
        assert_eq!(shared.progress().objects, 1);
        assert!(shared.store().has_object(&obj_hash));
        assert!(history.lock().unwrap().recently_synced(&addr.to_string()));
        stop_tx.send(true).unwrap();
        assert_eq!(task.await.unwrap(), Ok(()));
        server.abort();
    }

    #[tokio::test]
    async fn relay_serves_getdata() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (got_tx, mut got_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut fake = test_peer::spawn(sock, Network::Regtest);
            while let Some((cmd, payload)) = fake.received.recv().await {
                match cmd.as_str() {
                    "inv" => fake.send.send(("getdata".into(), payload)).unwrap(),
                    "govobj" => got_tx.send(payload).unwrap(),
                    _ => {}
                }
            }
        });
        let obj = GovernanceObject::new_proposal(1, 1, b"{}".to_vec());
        let cfg = SyncConfig::new(PeerConfig::new(Network::Regtest, "/t/", 1));
        let report = relay(
            &cfg,
            &[addr],
            vec![(INV_GOVERNANCE_OBJECT, obj.hash(), obj.encode())],
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(report.announced_to, 1);
        assert!(report.fetched.contains(&obj.hash()));
        assert_eq!(got_rx.recv().await.unwrap(), obj.encode());
    }
}
