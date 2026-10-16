//! govsync (MG-02 §2): downloading governance objects and the votes of
//! current proposals from full-node peers, following new ones as peers
//! relay them, and relaying the wallet's own.
//!
//! Core's server side (`governance/net_governance.cpp`): a `govsync` with a
//! zero hash answers with `ssc(10, n)` and `n` object invs, at most once per
//! address within the fulfilled-request window (1 hour on mainnet, 5
//! minutes elsewhere; a repeat is penalized); with an object hash and a
//! non-empty bloom filter, `ssc(11, n)` and the vote invs not in the filter,
//! once per object within the same window; with an object hash and an
//! empty filter, an inv of that object. Items are then fetched with
//! `getdata`. A peer accepts objects and votes only after it announced or
//! requested them, so the wallet relays its own by `inv` and answers the
//! peer's `getdata`.
//!
//! The wallet keeps the same request rules: an item is accepted only from
//! the peer it was asked of; anything else is unsolicited and counted, as
//! are undecodable and invalid items, and a peer past
//! [`SyncConfig::max_misbehaviour`] is closed (D10). A request ends with the
//! item, `notfound`, an item that does not decode or validate, the peer
//! closing, or [`SyncConfig::request_timeout`]; the next peer that announced
//! the hash is then asked (MG-01 D17). Objects are full-synced from every
//! fresh peer, so the store's two-peer rule (D3) is met at once. A run that
//! does not complete both phases ends `Failed` (D13).

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::Network;
use tokio::sync::{Notify, mpsc, watch};

use crate::object::GovernanceObject;
use crate::peer::{
    INV_GOVERNANCE_OBJECT, INV_GOVERNANCE_VOTE, InvItem, Peer, PeerConfig, PeerEvent, SSC_GOVOBJ,
    SSC_GOVOBJ_VOTE, empty_vote_filter, object_fetch_filter,
};
use crate::store::{GovernanceStore, VoteRejection};
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
    /// Confirmed objects.
    pub objects: u32,
    /// Objects announced by too few peers yet (D3).
    pub unconfirmed_objects: u32,
    pub votes: u64,
    pub orphan_votes: u32,
    pub peers: u32,
    pub bytes_received: u64,
    pub last_synced_at: Option<u64>,
    pub last_error: Option<String>,
    pub next_retry_at: Option<u64>,
    /// Wall time of the objects and votes phases of the last full sync.
    pub objects_secs: Option<f64>,
    pub votes_secs: Option<f64>,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: Phase::Disabled,
            objects: 0,
            unconfirmed_objects: 0,
            votes: 0,
            orphan_votes: 0,
            peers: 0,
            bytes_received: 0,
            last_synced_at: None,
            last_error: None,
            next_retry_at: None,
            objects_secs: None,
            votes_secs: None,
        }
    }
}

/// The wallet's own objects and votes, announced to every peer and served
/// on `getdata`.
#[derive(Debug, Default)]
struct Outbox {
    items: HashMap<[u8; 32], (u32, Vec<u8>)>,
    unannounced: Vec<InvItem>,
}

/// State shared between the sync task and readers.
pub struct Shared {
    pub network: Network,
    pub store: Mutex<GovernanceStore>,
    pub progress: Mutex<Progress>,
    /// Called after the store or the progress changed (the engine
    /// coalesces it into its `Governance` event).
    pub on_change: Box<dyn Fn() + Send + Sync>,
    outbox: Mutex<Outbox>,
    relay: Notify,
}

impl Shared {
    pub fn new(network: Network, on_change: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            network,
            store: Mutex::new(GovernanceStore::new(network)),
            progress: Mutex::new(Progress::default()),
            on_change,
            outbox: Mutex::new(Outbox::default()),
            relay: Notify::new(),
        }
    }

    pub fn store(&self) -> std::sync::MutexGuard<'_, GovernanceStore> {
        self.store.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn progress(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.progress.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn outbox(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.outbox.lock().unwrap_or_else(|p| p.into_inner())
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

    /// Queues the wallet's own vote for relay: announced to the connected
    /// peers now (and to peers of later runs) and served on `getdata`.
    /// Returns how many peers are connected. The caller stored the vote.
    pub fn relay_vote(&self, vote: &GovernanceVote) -> u32 {
        let item = InvItem {
            kind: INV_GOVERNANCE_VOTE,
            hash: vote.hash(),
        };
        {
            let mut o = self.outbox();
            o.items.insert(item.hash, (item.kind, vote.encode()));
            o.unannounced.push(item);
        }
        self.relay.notify_one();
        self.progress().peers
    }

    fn refresh(&self, peers: &[PeerState], base_bytes: u64) {
        let (objects, unconfirmed, votes, orphans) = {
            let s = self.store();
            let unconfirmed = s.unconfirmed_count();
            (
                (s.object_count() - unconfirmed) as u32,
                unconfirmed as u32,
                s.vote_count(),
                s.orphan_count() as u32,
            )
        };
        let mut p = self.progress();
        p.objects = objects;
        p.unconfirmed_objects = unconfirmed;
        p.votes = votes;
        p.orphan_votes = orphans;
        p.peers = peers.len() as u32;
        p.bytes_received = base_bytes + peers.iter().map(|s| s.peer.bytes_received()).sum::<u64>();
    }
}

/// Sync settings.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub peer: PeerConfig,
    /// How many peers to sync from.
    pub max_peers: usize,
    /// A phase fails when nothing arrived for this long.
    pub idle_timeout: Duration,
    /// Most items asked for in one `getdata` (Core `MAX_GETDATA_SZ`).
    pub getdata_batch: usize,
    /// A request moves to the next announcer after this long (D12).
    pub request_timeout: Duration,
    /// Unsolicited, undecodable or invalid items before a peer is closed
    /// (D10).
    pub max_misbehaviour: u32,
    /// Hashes one peer may announce that the wallet has not fetched (Core
    /// `MAX_PEER_OBJECT_ANNOUNCEMENTS`).
    pub max_announcements: usize,
    /// Core's fulfilled-request window for `govsync`.
    pub repeat_window: Duration,
}

impl SyncConfig {
    pub fn new(peer: PeerConfig) -> Self {
        let repeat_window = match peer.network {
            Network::Mainnet => Duration::from_secs(60 * 60),
            _ => Duration::from_secs(5 * 60),
        };
        Self {
            peer,
            max_peers: 3,
            idle_timeout: Duration::from_secs(20),
            getdata_batch: 1000,
            request_timeout: Duration::from_secs(30),
            max_misbehaviour: 1000,
            max_announcements: 100_000,
            repeat_window,
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
    #[error("sync incomplete: {0}")]
    Incomplete(String),
}

/// What this session asked of each peer, with when: Core penalizes a repeat
/// of a full sync or of one object's vote sync within its window.
#[derive(Debug, Default)]
pub struct SyncHistory {
    full_synced: HashMap<SocketAddr, Instant>,
    vote_synced: HashMap<(SocketAddr, [u8; 32]), Instant>,
}

impl SyncHistory {
    fn recently_synced(&self, addr: &SocketAddr, window: Duration) -> bool {
        self.full_synced
            .get(addr)
            .is_some_and(|t| t.elapsed() < window)
    }

    fn votes_recently_synced(&self, addr: &SocketAddr, hash: &[u8; 32], window: Duration) -> bool {
        self.vote_synced
            .get(&(*addr, *hash))
            .is_some_and(|t| t.elapsed() < window)
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One connected peer and what the run asked of it.
struct PeerState {
    peer: Peer,
    misbehaviour: u32,
    /// Hashes it announced that are not fetched yet.
    announced: usize,
    /// Objects phase: `ssc(10)`'s count and the object invs since.
    full_sync: Option<FullSync>,
    /// Votes phase: the objects whose votes it was asked for, `ssc(11)`
    /// answers, their counts and the vote invs since.
    vote_asked: Vec<[u8; 32]>,
    vote_answers: usize,
    vote_expected: i64,
    vote_announced: i64,
    /// Parents it was asked for after it sent an orphan vote.
    object_fetches: HashSet<[u8; 32]>,
}

#[derive(Default)]
struct FullSync {
    expected: Option<i32>,
    announced: i32,
}

impl PeerState {
    fn new(peer: Peer) -> Self {
        Self {
            peer,
            misbehaviour: 0,
            announced: 0,
            full_sync: None,
            vote_asked: Vec::new(),
            vote_answers: 0,
            vote_expected: 0,
            vote_announced: 0,
            object_fetches: HashSet::new(),
        }
    }

    fn objects_done(&self) -> bool {
        self.full_sync
            .as_ref()
            .is_none_or(|f| f.expected.is_some_and(|n| f.announced >= n))
    }

    fn votes_done(&self) -> bool {
        self.vote_answers >= self.vote_asked.len() && self.vote_announced >= self.vote_expected
    }
}

struct Pending {
    peer: usize,
    kind: u32,
    sent: Instant,
}

/// Requests in flight and who announced what.
#[derive(Default)]
struct Requests {
    pending: HashMap<[u8; 32], Pending>,
    /// Peers that announced a hash not fetched yet and were not tried, in
    /// order: who to ask next.
    announcers: HashMap<[u8; 32], Vec<usize>>,
    /// Every peer that announced an object not stored yet, tried or not:
    /// they count towards its confirmation once it arrives (D3).
    object_announcers: HashMap<[u8; 32], Vec<SocketAddr>>,
}

/// Connects to up to `max` of `candidates`, peers not full-synced recently
/// first.
async fn connect_peers(
    candidates: &[SocketAddr],
    cfg: &SyncConfig,
    history: &Mutex<SyncHistory>,
    events: &mpsc::UnboundedSender<(usize, PeerEvent)>,
) -> (Vec<PeerState>, Vec<String>) {
    let mut ordered = candidates.to_vec();
    {
        let h = history.lock().unwrap_or_else(|p| p.into_inner());
        ordered.sort_by_key(|a| h.recently_synced(a, cfg.repeat_window));
    }
    let max = cfg.max_peers.max(1);
    let mut peers = Vec::new();
    let mut errors = Vec::new();
    for (round, chunk) in ordered.chunks(max).enumerate() {
        let tries: Vec<_> = chunk
            .iter()
            .enumerate()
            .map(|(i, addr)| {
                let events = events.clone();
                let pc = cfg.peer.clone();
                let (id, addr) = (round * max + i, *addr);
                tokio::spawn(async move { Peer::connect(id, addr, &pc, events).await })
            })
            .collect();
        for t in tries {
            match t.await {
                Ok(Ok(p)) if peers.len() < max => peers.push(PeerState::new(p)),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => errors.push(e.to_string()),
                Err(e) => errors.push(e.to_string()),
            }
        }
        if peers.len() >= max {
            break;
        }
    }
    (peers, errors)
}

/// One sync run.
struct Run {
    shared: Arc<Shared>,
    cfg: SyncConfig,
    history: Arc<Mutex<SyncHistory>>,
    peers: Vec<PeerState>,
    requests: Requests,
    base_bytes: u64,
    /// Whether the votes of newly confirmed proposals are asked for at
    /// once (after the objects phase).
    votes_open: bool,
    /// Votes phase: proposals whose vote sync failed with every peer.
    unassigned: Vec<[u8; 32]>,
}

impl Run {
    fn peer(&self, id: usize) -> Option<&PeerState> {
        self.peers.iter().find(|p| p.peer.id == id)
    }

    fn peer_mut(&mut self, id: usize) -> Option<&mut PeerState> {
        self.peers.iter_mut().find(|p| p.peer.id == id)
    }

    fn changed(&self) {
        self.shared.refresh(&self.peers, self.base_bytes);
        (self.shared.on_change)();
    }

    fn misbehaved(&mut self, id: usize, why: &str) {
        let max = self.cfg.max_misbehaviour;
        if let Some(p) = self.peer_mut(id) {
            p.misbehaviour += 1;
            if p.misbehaviour == max + 1 {
                tracing::info!(peer = %p.peer.addr, why, "closing a governance peer that misbehaves");
                p.peer.close();
            }
        }
    }

    async fn announce_outbox(&self, only_new: bool) {
        let items: Vec<InvItem> = {
            let mut o = self.shared.outbox();
            if only_new {
                std::mem::take(&mut o.unannounced)
            } else {
                o.unannounced.clear();
                o.items
                    .iter()
                    .map(|(h, (k, _))| InvItem { kind: *k, hash: *h })
                    .collect()
            }
        };
        if items.is_empty() {
            return;
        }
        for p in &self.peers {
            p.peer.send_inv(&items).await;
        }
    }

    /// Asks `id` for `items` (already recorded as pending).
    async fn get(&self, id: usize, items: &[InvItem]) {
        if items.is_empty() {
            return;
        }
        if let Some(p) = self.peer(id) {
            for chunk in items.chunks(self.cfg.getdata_batch.max(1)) {
                p.peer.send_getdata(chunk).await;
            }
        }
    }

    /// Asks the next peer that announced `hash`, other than the ones tried.
    async fn retry(&mut self, hash: [u8; 32], kind: u32, tried: usize) {
        let next = self.requests.announcers.get_mut(&hash).and_then(|a| {
            a.retain(|p| *p != tried);
            a.first().copied()
        });
        let Some(next) = next.filter(|n| self.peer(*n).is_some()) else {
            self.requests.announcers.remove(&hash);
            self.requests.object_announcers.remove(&hash);
            return;
        };
        self.requests.pending.insert(
            hash,
            Pending {
                peer: next,
                kind,
                sent: Instant::now(),
            },
        );
        self.get(next, &[InvItem { kind, hash }]).await;
    }

    async fn on_inv(&mut self, from: usize, items: Vec<InvItem>) {
        let Some(addr) = self.peer(from).map(|p| p.peer.addr) else {
            return;
        };
        let (objects, votes) = items.iter().fold((0, 0), |(o, v), i| match i.kind {
            INV_GOVERNANCE_OBJECT => (o + 1, v),
            _ => (o, v + 1),
        });
        let mut wanted = Vec::new();
        let mut confirmed = Vec::new();
        let max_announced = self.cfg.max_announcements;
        {
            let mut store = self.shared.store();
            let state = self
                .peers
                .iter_mut()
                .find(|p| p.peer.id == from)
                .expect("looked up");
            if let Some(f) = state.full_sync.as_mut() {
                f.announced += objects;
            }
            state.vote_announced += votes;
            for i in items {
                let known = match i.kind {
                    INV_GOVERNANCE_OBJECT => {
                        if store.note_announcer(&i.hash, addr) {
                            confirmed.push(i.hash);
                        }
                        store.has_object(&i.hash)
                    }
                    _ => store.has_vote(&i.hash),
                };
                if known || state.announced >= max_announced {
                    continue;
                }
                if i.kind == INV_GOVERNANCE_OBJECT {
                    let by = self.requests.object_announcers.entry(i.hash).or_default();
                    if !by.contains(&addr) {
                        by.push(addr);
                    }
                }
                let announcers = self.requests.announcers.entry(i.hash).or_default();
                if announcers.contains(&from) {
                    continue;
                }
                announcers.push(from);
                state.announced += 1;
                if let std::collections::hash_map::Entry::Vacant(e) =
                    self.requests.pending.entry(i.hash)
                {
                    e.insert(Pending {
                        peer: from,
                        kind: i.kind,
                        sent: Instant::now(),
                    });
                    wanted.push(i);
                }
            }
        }
        self.get(from, &wanted).await;
        if !confirmed.is_empty() {
            self.changed();
            if self.votes_open {
                self.ask_votes(confirmed).await;
            }
        }
    }

    /// Takes the item `hash` from `from` if it was asked of `from`; counts
    /// misbehaviour otherwise.
    fn take_request(&mut self, from: usize, hash: &[u8; 32]) -> bool {
        match self.requests.pending.get(hash) {
            Some(p) if p.peer == from => {
                self.requests.pending.remove(hash);
                if let Some(p) = self.peer_mut(from) {
                    p.announced = p.announced.saturating_sub(1);
                }
                true
            }
            _ => {
                self.misbehaved(from, "unsolicited item");
                false
            }
        }
    }

    async fn on_object(&mut self, from: usize, payload: Vec<u8>) {
        let Ok(object) = GovernanceObject::decode(&payload) else {
            // The pending hash is unknown; its request times out.
            self.misbehaved(from, "undecodable object");
            return;
        };
        let hash = object.hash();
        if !self.take_request(from, &hash) {
            return;
        }
        let addr = self.peer(from).map(|p| p.peer.addr);
        let now = unix_now();
        let result = {
            let mut store = self.shared.store();
            let r = store.add_object(object, addr, now);
            if r.is_ok() {
                self.requests.announcers.remove(&hash);
                for addr in self
                    .requests
                    .object_announcers
                    .remove(&hash)
                    .unwrap_or_default()
                {
                    store.note_announcer(&hash, addr);
                }
            }
            r
        };
        match result {
            Ok(_) => {
                self.changed();
                let current = self.shared.store().current_proposals(now).contains(&hash);
                if self.votes_open && current {
                    self.ask_votes(vec![hash]).await;
                }
            }
            Err(e) => {
                tracing::debug!(?e, "governance object refused");
                self.misbehaved(from, "invalid object");
                self.retry(hash, INV_GOVERNANCE_OBJECT, from).await;
            }
        }
    }

    async fn on_vote(&mut self, from: usize, payload: Vec<u8>) {
        let Ok(vote) = GovernanceVote::decode(&payload) else {
            self.misbehaved(from, "undecodable vote");
            return;
        };
        let hash = vote.hash();
        if !self.take_request(from, &hash) {
            return;
        }
        let parent = vote.parent_hash;
        let addr = self.peer(from).map(|p| p.peer.addr);
        let result = self.shared.store().add_vote(vote, addr, unix_now());
        match result {
            Ok(()) => {
                self.requests.announcers.remove(&hash);
                self.changed();
            }
            Err(VoteRejection::Orphan) => {
                self.requests.announcers.remove(&hash);
                self.fetch_parent(from, parent).await;
            }
            Err(r @ (VoteRejection::BadSignature | VoteRejection::NotTallied)) => {
                // The vote hash leaves out the signature: another peer's
                // copy may be the genuine one.
                tracing::debug!(?r, "governance vote refused");
                self.misbehaved(from, "invalid vote");
                self.retry(hash, INV_GOVERNANCE_VOTE, from).await;
            }
            Err(_) => {
                self.requests.announcers.remove(&hash);
            }
        }
    }

    /// Core answers an orphan vote by asking its sender for the object
    /// (`govsync <hash>` with an empty filter); once per parent and peer.
    async fn fetch_parent(&mut self, from: usize, parent: [u8; 32]) {
        let known =
            self.shared.store().has_object(&parent) || self.requests.pending.contains_key(&parent);
        let Some(p) = self.peer_mut(from) else { return };
        if known || !p.object_fetches.insert(parent) {
            return;
        }
        let p = self.peer(from).expect("looked up");
        p.peer.send_govsync(parent, &object_fetch_filter()).await;
    }

    async fn on_not_found(&mut self, from: usize, items: Vec<InvItem>) {
        for i in items {
            if self
                .requests
                .pending
                .get(&i.hash)
                .is_some_and(|p| p.peer == from)
            {
                self.requests.pending.remove(&i.hash);
                self.retry(i.hash, i.kind, from).await;
            }
        }
    }

    async fn on_get_data(&self, from: usize, items: Vec<InvItem>) {
        let answers: Vec<(u32, Vec<u8>)> = {
            let o = self.shared.outbox();
            items
                .iter()
                .filter_map(|i| o.items.get(&i.hash).cloned())
                .collect()
        };
        let Some(p) = self.peer(from) else { return };
        for (kind, payload) in answers {
            match kind {
                INV_GOVERNANCE_OBJECT => p.peer.send_object(payload).await,
                _ => p.peer.send_vote(payload).await,
            }
        }
    }

    async fn on_closed(&mut self, id: usize) {
        let Some(pos) = self.peers.iter().position(|p| p.peer.id == id) else {
            return;
        };
        let gone = self.peers.remove(pos);
        let pending: Vec<([u8; 32], u32)> = self
            .requests
            .pending
            .iter()
            .filter(|(_, p)| p.peer == id)
            .map(|(h, p)| (*h, p.kind))
            .collect();
        for (h, kind) in pending {
            self.requests.pending.remove(&h);
            self.retry(h, kind, id).await;
        }
        self.requests.announcers.retain(|_, a| {
            a.retain(|p| *p != id);
            !a.is_empty()
        });
        // Vote syncs it had not answered go to another peer.
        let unanswered: Vec<[u8; 32]> = gone
            .vote_asked
            .iter()
            .skip(gone.vote_answers)
            .copied()
            .collect();
        if !unanswered.is_empty() {
            self.ask_votes(unanswered).await;
        }
        self.changed();
    }

    async fn handle(&mut self, from: usize, ev: PeerEvent) {
        match ev {
            PeerEvent::Inv(items) => self.on_inv(from, items).await,
            PeerEvent::GetData(items) => self.on_get_data(from, items).await,
            PeerEvent::NotFound(items) => self.on_not_found(from, items).await,
            PeerEvent::Object(payload) => self.on_object(from, payload).await,
            PeerEvent::Vote(payload) => self.on_vote(from, payload).await,
            PeerEvent::SyncCount { item, count } => {
                if let Some(p) = self.peer_mut(from) {
                    match item {
                        SSC_GOVOBJ => {
                            if let Some(f) = p.full_sync.as_mut() {
                                f.expected = Some(count);
                            }
                        }
                        SSC_GOVOBJ_VOTE => {
                            p.vote_answers += 1;
                            p.vote_expected += i64::from(count);
                        }
                        _ => {}
                    }
                }
            }
            PeerEvent::Malformed => self.misbehaved(from, "malformed message"),
            PeerEvent::Closed => self.on_closed(from).await,
        }
    }

    /// Requests that waited past the timeout move to the next announcer.
    async fn expire_requests(&mut self) {
        let timeout = self.cfg.request_timeout;
        let late: Vec<([u8; 32], usize, u32)> = self
            .requests
            .pending
            .iter()
            .filter(|(_, p)| p.sent.elapsed() >= timeout)
            .map(|(h, p)| (*h, p.peer, p.kind))
            .collect();
        for (h, peer, kind) in late {
            self.requests.pending.remove(&h);
            if let Some(p) = self.peer_mut(peer) {
                p.announced = p.announced.saturating_sub(1);
            }
            self.retry(h, kind, peer).await;
        }
        self.shared.store().expire_orphans(unix_now());
    }

    /// Asks peers for the votes of `hashes`, spread over the peers that did
    /// not vote-sync each within the window; hashes no peer may be asked
    /// for are kept in `unassigned`.
    async fn ask_votes(&mut self, hashes: Vec<[u8; 32]>) {
        let window = self.cfg.repeat_window;
        let filter = empty_vote_filter();
        let mut turn = 0usize;
        for hash in hashes {
            let n = self.peers.len();
            let pick = {
                let h = self.history.lock().unwrap_or_else(|p| p.into_inner());
                (0..n)
                    .map(|k| (turn + k) % n)
                    .find(|&i| !h.votes_recently_synced(&self.peers[i].peer.addr, &hash, window))
            };
            let Some(i) = pick else {
                self.unassigned.push(hash);
                continue;
            };
            turn = i + 1;
            let addr = self.peers[i].peer.addr;
            self.history
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .vote_synced
                .insert((addr, hash), Instant::now());
            self.peers[i].vote_asked.push(hash);
            self.peers[i].peer.send_govsync(hash, &filter).await;
        }
    }

    fn requests_done(&self) -> bool {
        self.requests.pending.is_empty()
    }

    /// Processes events until `done` holds and nothing is pending (`Ok`),
    /// nothing arrived for the idle timeout, every peer is gone (`Err`), or
    /// `stop` (`Ok(false)`).
    async fn drain(
        &mut self,
        rx: &mut mpsc::UnboundedReceiver<(usize, PeerEvent)>,
        stop: &mut watch::Receiver<bool>,
        done: impl Fn(&Run) -> bool,
    ) -> Result<bool, SyncError> {
        let mut last_event = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            if done(self) && self.requests_done() {
                return Ok(true);
            }
            if self.peers.is_empty() {
                return Err(SyncError::PeersLost);
            }
            let idle = self.cfg.idle_timeout.saturating_sub(last_event.elapsed());
            tokio::select! {
                r = stop.changed() => {
                    if r.is_err() || *stop.borrow() { return Ok(false); }
                }
                _ = tokio::time::sleep(idle) => {
                    return Err(SyncError::Incomplete("no progress within the idle timeout".into()));
                }
                _ = tick.tick() => self.expire_requests().await,
                _ = self.shared.relay.notified() => self.announce_outbox(true).await,
                ev = rx.recv() => {
                    let Some((from, ev)) = ev else { return Err(SyncError::PeersLost) };
                    last_event = Instant::now();
                    self.handle(from, ev).await;
                    self.shared.refresh(&self.peers, self.base_bytes);
                }
            }
        }
    }
}

/// One sync run: connect, full-sync objects from every fresh peer, then the
/// votes of current proposals, then follow relays until `stop` turns true
/// or the run fails. `base_bytes` of the progress carries earlier runs'
/// bytes. On failure the progress says `Failed` with the reason.
pub async fn run(
    shared: Arc<Shared>,
    cfg: SyncConfig,
    candidates: Vec<SocketAddr>,
    history: Arc<Mutex<SyncHistory>>,
    mut stop: watch::Receiver<bool>,
) -> Result<(), SyncError> {
    let result = run_inner(Arc::clone(&shared), cfg, candidates, history, &mut stop).await;
    if let Err(e) = &result {
        {
            let mut p = shared.progress();
            p.phase = Phase::Failed;
            p.last_error = Some(e.to_string());
            p.peers = 0;
        }
        (shared.on_change)();
    }
    result
}

async fn run_inner(
    shared: Arc<Shared>,
    cfg: SyncConfig,
    candidates: Vec<SocketAddr>,
    history: Arc<Mutex<SyncHistory>>,
    stop: &mut watch::Receiver<bool>,
) -> Result<(), SyncError> {
    let base_bytes = shared.progress().bytes_received;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (peers, errors) = connect_peers(&candidates, &cfg, &history, &tx).await;
    drop(tx);
    if peers.is_empty() {
        return Err(SyncError::NoPeers(if errors.is_empty() {
            "no candidate peers".into()
        } else {
            errors.join("; ")
        }));
    }
    let mut run = Run {
        shared: Arc::clone(&shared),
        cfg,
        history,
        peers,
        requests: Requests::default(),
        base_bytes,
        votes_open: false,
        unassigned: Vec::new(),
    };
    run.changed();
    run.announce_outbox(false).await;

    let window = run.cfg.repeat_window;
    let fresh: Vec<usize> = {
        let h = run.history.lock().unwrap_or_else(|p| p.into_inner());
        run.peers
            .iter()
            .filter(|p| !h.recently_synced(&p.peer.addr, window))
            .map(|p| p.peer.id)
            .collect()
    };
    let synced_before = shared.progress().last_synced_at.is_some();
    if fresh.is_empty() && !synced_before {
        return Err(SyncError::Incomplete(
            "every peer was full-synced within its window".into(),
        ));
    }

    if !fresh.is_empty() {
        // Objects, from every fresh peer.
        shared.set_phase(Phase::SyncingObjects);
        let started = Instant::now();
        let filter = object_fetch_filter();
        for p in run.peers.iter_mut().filter(|p| fresh.contains(&p.peer.id)) {
            p.full_sync = Some(FullSync::default());
            run.history
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .full_synced
                .insert(p.peer.addr, Instant::now());
            p.peer.send_govsync([0; 32], &filter).await;
        }
        if !run
            .drain(&mut rx, stop, |r| {
                r.peers.iter().all(PeerState::objects_done)
            })
            .await?
        {
            return Ok(());
        }
        shared.progress().objects_secs = Some(started.elapsed().as_secs_f64());

        // Votes of the current proposals, spread over the peers.
        shared.set_phase(Phase::SyncingVotes);
        let started = Instant::now();
        let current = shared.store().current_proposals(unix_now());
        run.votes_open = true;
        run.ask_votes(current).await;
        if !run
            .drain(&mut rx, stop, |r| r.peers.iter().all(PeerState::votes_done))
            .await?
        {
            return Ok(());
        }
        if !run.unassigned.is_empty() {
            return Err(SyncError::Incomplete(format!(
                "the votes of {} proposals could not be asked for",
                run.unassigned.len()
            )));
        }
        shared.progress().votes_secs = Some(started.elapsed().as_secs_f64());
    }
    run.votes_open = true;

    {
        let mut p = shared.progress();
        p.phase = Phase::Synced;
        p.last_synced_at = Some(unix_now() as u64);
        p.last_error = None;
        p.next_retry_at = None;
    }
    run.changed();

    // Follow relays until stopped or every peer is gone.
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        if run.peers.is_empty() {
            return Err(SyncError::PeersLost);
        }
        tokio::select! {
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() { return Ok(()); }
            }
            _ = tick.tick() => run.expire_requests().await,
            _ = shared.relay.notified() => run.announce_outbox(true).await,
            ev = rx.recv() => {
                let Some((from, ev)) = ev else { return Err(SyncError::PeersLost) };
                run.handle(from, ev).await;
                shared.refresh(&run.peers, run.base_bytes);
            }
        }
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
