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
//! a peer it was asked of (the one asked now, or one asked before whose
//! request timed out); anything else is unsolicited and counted, as are
//! undecodable and invalid items, and a peer past
//! [`SyncConfig::max_misbehaviour`] is closed (D10). Votes Core relays but
//! the wallet does not tally are ignored without a penalty; votes whose key
//! the list lacks may be honest under the peer's list and count against the
//! looser [`SyncConfig::max_suspect`]. At most
//! [`SyncConfig::max_in_flight`] items are asked of one peer at a time. A
//! request ends with the item, `notfound`, an item that does not decode or
//! validate, or the peer closing, and the next peer that announced the hash
//! is asked (MG-01 D17). One that got nothing from its peer for
//! [`SyncConfig::request_timeout`] since that peer's last delivery moves to
//! the next announcer, or with none is asked again once and then given up;
//! a sync item given up on fails the phase unless it still arrives (D13).
//!
//! Objects are full-synced from every fresh peer, so the store's two-peer
//! rule (D3) is met at once; objects one peer announced are then asked of
//! the peers that did not full-sync this run. A phase completes when every
//! peer's `ssc` count arrived and the phase's requests are done; it fails
//! when it made no progress for [`SyncConfig::idle_timeout`], ran past
//! [`SyncConfig::phase_deadline`], or no peer completed the object sync. A
//! peer that leaves an `ssc` unanswered for [`SyncConfig::answer_timeout`]
//! is closed, and the vote syncs it had not finished go to another peer.
//! The wallet's own votes are announced only once the run is synced: Core
//! leaves out of its vote invs the ones the wallet announced on that
//! connection but still counts them in `ssc`. A run that does not complete
//! both phases ends `Failed` (D13).

use std::collections::{HashMap, HashSet, VecDeque, hash_map::Entry};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dashcore::Network;
use tokio::sync::{Notify, mpsc, watch};

use crate::object::GovernanceObject;
use crate::peer::{
    EVENTS_QUEUE, INV_GOVERNANCE_OBJECT, INV_GOVERNANCE_VOTE, Inbound, InvItem, Peer, PeerConfig,
    PeerEvent, SSC_GOVOBJ, SSC_GOVOBJ_VOTE, empty_vote_filter, object_fetch_filter,
};
use crate::store::{GovernanceStore, VoteRejection, recover};
use crate::validate::ObjectRejection;
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
    /// peers once the run is synced (at once if it is), to the peers of
    /// later runs once those are, and served on `getdata`.
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
    /// A phase fails when it made no progress for this long.
    pub idle_timeout: Duration,
    /// A phase fails when it has not completed in this long.
    pub phase_deadline: Duration,
    /// Most items asked for in one `getdata` (Core `MAX_GETDATA_SZ`).
    pub getdata_batch: usize,
    /// Most items asked of one peer at a time.
    pub max_in_flight: usize,
    /// A request moves on when its peer delivered nothing for this long
    /// (D12), or, queued behind the peer's full slots, when another
    /// announcer can be asked (D55).
    pub request_timeout: Duration,
    /// A request moves on this long after it was sent, however much else
    /// its peer delivers (D55).
    pub max_request_age: Duration,
    /// A peer asked to sync that sends no `ssc` for this long is closed.
    pub answer_timeout: Duration,
    /// Unsolicited, undecodable or invalid items before a peer is closed
    /// (D10).
    pub max_misbehaviour: u32,
    /// Votes whose key the list lacks before a peer is closed (D10).
    pub max_suspect: u32,
    /// Hashes one peer may announce that the wallet has not fetched (Core
    /// `MAX_PEER_OBJECT_ANNOUNCEMENTS`).
    pub max_announcements: usize,
    /// Parent objects asked for per run after one peer's orphan votes
    /// (D52).
    pub max_parent_fetches: usize,
    /// Hashes whose announcers are kept for a late copy; once full, no new
    /// ones (D53).
    pub max_late_announcers: usize,
    /// Peers asked for one proposal's votes (Core
    /// `REALNET_PEERS_PER_HASH`, D56).
    pub vote_peers_per_proposal: usize,
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
            phase_deadline: Duration::from_secs(30 * 60),
            getdata_batch: 1000,
            max_in_flight: 5000,
            request_timeout: Duration::from_secs(30),
            max_request_age: Duration::from_secs(120),
            answer_timeout: Duration::from_secs(60),
            max_misbehaviour: 1000,
            max_suspect: 10_000,
            max_announcements: 100_000,
            max_parent_fetches: 100,
            max_late_announcers: MAX_LATE,
            vote_peers_per_proposal: 3,
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

/// Unconfirmed objects asked of peers that did not full-sync, per run.
const MAX_CONFIRMATION_ASKS: usize = 200;
/// Given-up requests whose peers may still deliver, remembered per run.
const MAX_LATE: usize = 100_000;

/// One connected peer and what the run asked of it.
struct PeerState {
    peer: Peer,
    misbehaviour: u32,
    suspect: u32,
    /// Hashes it announced that are not fetched or given up yet.
    announced: usize,
    /// Objects phase: `ssc(10)`'s count and the object invs since.
    full_sync: Option<FullSync>,
    /// Votes phase: the objects whose votes it was asked for and its
    /// `ssc(11)` counts, in the same order (Core answers in order).
    vote_asked: Vec<[u8; 32]>,
    vote_expected: Vec<i64>,
    /// Asked object → the votes on it the peer announced (D46).
    vote_announced: HashMap<[u8; 32], i64>,
    /// Votes it announced whose object the run does not know yet.
    unresolved: HashSet<[u8; 32]>,
    /// Of those, how many the run gave up on, so never learns the object
    /// of: they may make up a count its invs fall short of.
    unattributable: i64,
    /// Votes it announced on objects not asked of it → their object. Core
    /// counts them in a later vote sync of the object on the connection
    /// but leaves them out of its invs (D36).
    relayed: HashMap<[u8; 32], [u8; 32]>,
    /// When its next `ssc` is due, while one is outstanding.
    answer_due: Option<Instant>,
    /// When it last delivered an item.
    last_delivery: Instant,
    /// Hashes assigned to it, not asked yet ([`SyncConfig::max_in_flight`]).
    queue: VecDeque<Item>,
    in_flight: usize,
    /// Parents its orphan votes had the wallet ask for: its budget.
    parent_fetches: HashSet<[u8; 32]>,
    /// Parents asked of it, for any peer's orphan votes.
    parents_asked: HashSet<[u8; 32]>,
}

#[derive(Default)]
struct FullSync {
    expected: Option<i32>,
    announced: i32,
    /// Counted in [`Run::full_syncs_done`].
    counted: bool,
}

impl FullSync {
    fn complete(&self) -> bool {
        self.expected.is_some_and(|n| self.announced >= n)
    }
}

impl PeerState {
    fn new(peer: Peer) -> Self {
        Self {
            peer,
            misbehaviour: 0,
            suspect: 0,
            announced: 0,
            full_sync: None,
            vote_asked: Vec::new(),
            vote_expected: Vec::new(),
            vote_announced: HashMap::new(),
            unresolved: HashSet::new(),
            unattributable: 0,
            relayed: HashMap::new(),
            answer_due: None,
            last_delivery: Instant::now(),
            queue: VecDeque::new(),
            in_flight: 0,
            parent_fetches: HashSet::new(),
            parents_asked: HashSet::new(),
        }
    }

    fn objects_done(&self) -> bool {
        self.full_sync.as_ref().is_none_or(FullSync::complete)
    }

    /// Every vote sync asked of it answered, and each count met by the
    /// invs of votes on its own object (D46).
    fn votes_done(&self) -> bool {
        let short: i64 = self
            .vote_asked
            .iter()
            .zip(&self.vote_expected)
            .map(|(h, n)| (n - self.vote_announced.get(h).copied().unwrap_or(0)).max(0))
            .sum();
        self.vote_expected.len() >= self.vote_asked.len() && short <= self.unattributable
    }

    fn answers_outstanding(&self) -> bool {
        self.full_sync
            .as_ref()
            .is_some_and(|f| f.expected.is_none())
            || self.vote_expected.len() < self.vote_asked.len()
    }

    /// Counts its inv of `vote`, on `parent`, towards its vote sync of
    /// `parent`, or keeps it for a later one (D36).
    fn attribute(&mut self, vote: [u8; 32], parent: [u8; 32]) {
        if let Some(n) = self.vote_announced.get_mut(&parent) {
            *n += 1;
        } else if self.relayed.len() < MAX_LATE {
            self.relayed.insert(vote, parent);
        }
    }

    /// Restarts the `ssc` deadline after an ask or an answer.
    fn rearm_answer(&mut self, timeout: Duration) {
        self.answer_due = self.answers_outstanding().then(|| Instant::now() + timeout);
    }
}

struct Pending {
    peer: usize,
    /// When it was queued at `peer`.
    queued: Instant,
    /// When the `getdata` went out; `None` while queued.
    sent: Option<Instant>,
    /// Asked of the same peer again after a timeout.
    asked_again: bool,
    /// An item of the running phase: giving up on it fails the phase.
    phase: bool,
    /// Peers asked before, whose late delivery is still accepted.
    asked: Vec<usize>,
}

/// An inventory item, hash and kind: requests are keyed by both, as
/// Core's `CInv` is, so announcing a hash as the other kind asks for that
/// besides (D54).
type Item = ([u8; 32], u32);

/// Who announced an item not fetched yet.
#[derive(Default)]
struct Announced {
    /// Not asked yet, in order: who to ask next.
    untried: Vec<usize>,
    /// Every peer whose announcement counts against its limit.
    all: Vec<usize>,
}

/// A request given up on: its peers may still deliver.
struct Late {
    asked: Vec<usize>,
    /// Counted in [`Run::lost`].
    lost: bool,
}

/// What [`Run::take`] accepted an item as.
struct Taken {
    phase: bool,
    asked: Vec<usize>,
}

/// Requests in flight and who announced what.
#[derive(Default)]
struct Requests {
    pending: HashMap<Item, Pending>,
    announced: HashMap<Item, Announced>,
    /// Every peer that announced an object not stored yet, tried or not:
    /// they count towards its confirmation once it arrives (D3).
    object_announcers: HashMap<[u8; 32], Vec<SocketAddr>>,
    late: HashMap<Item, Late>,
}

/// The other governance inv kind.
fn other_kind(kind: u32) -> u32 {
    if kind == INV_GOVERNANCE_OBJECT {
        INV_GOVERNANCE_VOTE
    } else {
        INV_GOVERNANCE_OBJECT
    }
}

/// Connects to up to `max` of `candidates`, peers not full-synced recently
/// first.
async fn connect_peers(
    candidates: &[SocketAddr],
    cfg: &SyncConfig,
    history: &Mutex<SyncHistory>,
    events: &mpsc::Sender<Inbound>,
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
                Ok(Ok(p)) if peers.len() < max => {
                    tracing::debug!(peer = %p.addr, "governance peer connected");
                    peers.push(PeerState::new(p));
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => {
                    tracing::debug!(error = %e, "governance peer not reached");
                    errors.push(e.to_string());
                }
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
    /// Whether the wallet's own items are announced (once synced).
    announce_open: bool,
    /// Votes phase: proposals whose vote sync no peer may be asked for.
    unassigned: Vec<[u8; 32]>,
    /// Phase items given up on.
    lost: usize,
    /// Fresh peers whose object sync completed.
    full_syncs_done: usize,
    /// Vote → its object, for the votes decoded this run (D36).
    vote_parents: HashMap<[u8; 32], [u8; 32]>,
    last_progress: Instant,
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

    fn progressed(&mut self) {
        self.last_progress = Instant::now();
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

    fn suspected(&mut self, id: usize) {
        let max = self.cfg.max_suspect;
        if let Some(p) = self.peer_mut(id) {
            p.suspect += 1;
            if p.suspect == max + 1 {
                tracing::info!(peer = %p.peer.addr, "closing a governance peer that sends votes of unknown keys");
                p.peer.close();
            }
        }
    }

    fn announce_outbox(&self, only_new: bool) {
        if !self.announce_open {
            return;
        }
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
            p.peer.send_inv(&items);
        }
    }

    /// Assigns `item` to peer `id` and queues it there.
    fn assign(&mut self, item: Item, id: usize, phase: bool, asked: Vec<usize>) {
        self.requests.pending.insert(
            item,
            Pending {
                peer: id,
                queued: Instant::now(),
                sent: None,
                asked_again: false,
                phase,
                asked,
            },
        );
        let pending = &self.requests.pending;
        let Some(p) = self.peers.iter_mut().find(|p| p.peer.id == id) else {
            return;
        };
        p.queue.push_back(item);
        // Entries of requests ended while queued stay until popped: past
        // twice what the peer may have queued, they go (D55).
        if p.queue.len() > 2 * p.announced + 64 {
            let mut seen = HashSet::new();
            p.queue.retain(|i| {
                pending
                    .get(i)
                    .is_some_and(|r| r.peer == id && r.sent.is_none())
                    && seen.insert(*i)
            });
        }
    }

    /// Asks peer `id` for queued items up to its in-flight limit.
    fn pump(&mut self, id: usize) {
        let (max, batch) = (self.cfg.max_in_flight, self.cfg.getdata_batch.max(1));
        let Some(pos) = self.peers.iter().position(|p| p.peer.id == id) else {
            return;
        };
        let now = Instant::now();
        let mut items = Vec::new();
        let p = &mut self.peers[pos];
        while p.in_flight < max {
            let Some((hash, kind)) = p.queue.pop_front() else {
                break;
            };
            if let Some(r) = self.requests.pending.get_mut(&(hash, kind))
                && r.peer == id
                && r.sent.is_none()
            {
                r.sent = Some(now);
                p.in_flight += 1;
                items.push(InvItem { kind, hash });
            }
        }
        for chunk in items.chunks(batch) {
            p.peer.send_getdata(chunk);
        }
    }

    /// Removes the request for `item`, freeing its peer's slot for the
    /// next queued one (D55).
    fn release(&mut self, item: &Item) -> Option<Pending> {
        let r = self.requests.pending.remove(item)?;
        if r.sent.is_some()
            && let Some(p) = self.peer_mut(r.peer)
        {
            p.in_flight = p.in_flight.saturating_sub(1);
            self.pump(r.peer);
        }
        Some(r)
    }

    /// Done with `item`: its announcements stop counting against their
    /// peers' limits.
    fn finish(&mut self, item: &Item) {
        let Some(a) = self.requests.announced.remove(item) else {
            return;
        };
        for id in a.all {
            if let Some(p) = self.peer_mut(id) {
                p.announced = p.announced.saturating_sub(1);
            }
        }
    }

    /// Accepts `item` from `from` if it was asked of `from` now or before;
    /// counts misbehaviour otherwise.
    fn take(&mut self, from: usize, item: &Item) -> Option<Taken> {
        if let Some(r) = self.requests.pending.get(item)
            && (r.peer == from || r.asked.contains(&from))
        {
            let r = self.release(item).expect("looked up");
            if let Some(p) = self.peer_mut(from) {
                p.last_delivery = Instant::now();
            }
            if r.peer != from || r.asked_again {
                // The peer asked now (or asked twice) may still deliver.
                self.remember_late(*item, vec![r.peer], false);
            }
            return Some(Taken {
                phase: r.phase,
                asked: r.asked,
            });
        }
        if let Some(l) = self.requests.late.get_mut(item)
            && l.asked.contains(&from)
        {
            // Kept: a request asked twice may be answered twice.
            let was_lost = std::mem::replace(&mut l.lost, false);
            let asked = l.asked.clone();
            if was_lost {
                self.lost -= 1;
            }
            if let Some(p) = self.peer_mut(from) {
                p.last_delivery = Instant::now();
            }
            return Some(Taken {
                phase: was_lost,
                asked,
            });
        }
        self.misbehaved(from, "unsolicited item");
        None
    }

    fn remember_late(&mut self, item: Item, asked: Vec<usize>, lost: bool) {
        if self.requests.late.len() >= MAX_LATE {
            self.requests.late.retain(|_, l| l.lost);
        }
        if let Some(old) = self.requests.late.insert(item, Late { asked, lost })
            && old.lost
        {
            self.lost -= 1;
        }
        if lost {
            self.lost += 1;
        }
    }

    /// Asks the next peer that announced `hash` and was not asked; with
    /// none, gives up on it (failing the phase if `phase` and `count`) and
    /// returns `true`.
    fn retry(&mut self, item: Item, tried: usize, t: Taken, count: bool) -> bool {
        let mut asked = t.asked;
        if !asked.contains(&tried) {
            asked.push(tried);
        }
        let live: Vec<usize> = self.peers.iter().map(|p| p.peer.id).collect();
        let next = self.requests.announced.get_mut(&item).and_then(|a| {
            a.untried.retain(|p| !asked.contains(p) && live.contains(p));
            a.untried.first().copied()
        });
        match next {
            Some(next) => {
                self.assign(item, next, t.phase, asked);
                self.pump(next);
                false
            }
            None => {
                // Who announced an object still counts if another peer
                // delivers it later.
                self.remember_late(item, asked, t.phase && count);
                self.finish(&item);
                let (hash, kind) = item;
                if kind == INV_GOVERNANCE_VOTE {
                    for p in &mut self.peers {
                        if p.unresolved.remove(&hash) {
                            p.unattributable += 1;
                        }
                    }
                }
                true
            }
        }
    }

    /// An item the wallet refused: another announcer's copy may be
    /// accepted, but a refusal never fails the phase (D35).
    fn refused(&mut self, item: Item, from: usize, taken: Taken) -> bool {
        self.retry(item, from, taken, false)
    }

    fn on_inv(&mut self, from: usize, items: Vec<InvItem>) {
        let Some(pos) = self.peers.iter().position(|p| p.peer.id == from) else {
            return;
        };
        let addr = self.peers[pos].peer.addr;
        let now = unix_now();
        let max_announced = self.cfg.max_announcements;
        let state = &mut self.peers[pos];
        // Items announced while the peer's sync is still answering belong to
        // the phase.
        let objects_phase = state.full_sync.as_ref().is_some_and(|f| !f.complete());
        let votes_phase = !state.votes_done();
        let mut confirmed = Vec::new();
        let mut wanted = Vec::new();
        {
            let mut store = self.shared.store();
            for i in items {
                let object = i.kind == INV_GOVERNANCE_OBJECT;
                if object {
                    if let Some(f) = state.full_sync.as_mut() {
                        f.announced += 1;
                    }
                    if store.note_announcer(&i.hash, addr) {
                        confirmed.push(i.hash);
                    }
                }
                let known = if object {
                    store.has_object(&i.hash)
                } else {
                    store.has_vote(&i.hash)
                };
                if !object {
                    // Only a vote on an object asked of the peer counts
                    // towards that sync (D46).
                    let parent = store
                        .vote_parent(&i.hash)
                        .or_else(|| self.vote_parents.get(&i.hash).copied());
                    match parent {
                        Some(parent) => state.attribute(i.hash, parent),
                        None if state.announced < max_announced
                            && state.unresolved.len() < MAX_LATE =>
                        {
                            state.unresolved.insert(i.hash);
                        }
                        None => state.unattributable += 1,
                    }
                }
                if known || state.announced >= max_announced {
                    continue;
                }
                if object {
                    // Full, it takes no new hashes rather than forget the
                    // ones it holds: no peer can clear another's (D53).
                    let by_hash = &mut self.requests.object_announcers;
                    if let Some(by) = by_hash.get_mut(&i.hash) {
                        if !by.contains(&addr) {
                            by.push(addr);
                        }
                    } else if by_hash.len() < self.cfg.max_late_announcers {
                        by_hash.insert(i.hash, vec![addr]);
                    }
                }
                let item = (i.hash, i.kind);
                let a = self.requests.announced.entry(item).or_default();
                if a.all.contains(&from) {
                    continue;
                }
                a.all.push(from);
                a.untried.push(from);
                state.announced += 1;
                let phase = if object { objects_phase } else { votes_phase };
                match self.requests.pending.entry(item) {
                    Entry::Vacant(_) => wanted.push((item, phase)),
                    Entry::Occupied(mut e) => e.get_mut().phase |= phase,
                }
            }
            confirmed.retain(|h| store.is_current_proposal(h, now));
        }
        if wanted.iter().any(|(_, phase)| *phase) || !confirmed.is_empty() {
            self.progressed();
        }
        for (item, phase) in wanted {
            self.assign(item, from, phase, Vec::new());
        }
        self.pump(from);
        if !confirmed.is_empty() {
            self.changed();
            if self.votes_open {
                self.ask_votes(confirmed);
            }
        }
    }

    fn on_object(&mut self, from: usize, payload: Vec<u8>) {
        let Ok(object) = GovernanceObject::decode(&payload) else {
            // The pending hash is unknown; its request times out.
            self.misbehaved(from, "undecodable object");
            return;
        };
        let hash = object.hash();
        let item = (hash, INV_GOVERNANCE_OBJECT);
        let Some(taken) = self.take(from, &item) else {
            return;
        };
        let addr = self.peer(from).map(|p| p.peer.addr);
        let now = unix_now();
        let result = {
            // Its announcers this run: the peers asked for it (a bounded
            // record no peer can clear) and those kept for a late copy.
            let mut also: Vec<SocketAddr> = self
                .requests
                .announced
                .get(&item)
                .map(|a| {
                    a.all
                        .iter()
                        .filter_map(|id| self.peer(*id))
                        .map(|p| p.peer.addr)
                        .collect()
                })
                .unwrap_or_default();
            also.extend(
                self.requests
                    .object_announcers
                    .get(&hash)
                    .into_iter()
                    .flatten(),
            );
            let mut store = self.shared.store();
            let r = store.add_object_announced(object, addr, &also, now);
            if r.is_ok() {
                self.requests.object_announcers.remove(&hash);
            }
            r.map(|_| store.is_current_proposal(&hash, now))
        };
        match result {
            Ok(current) => {
                self.finish(&item);
                self.progressed();
                self.changed();
                if self.votes_open && current {
                    self.ask_votes(vec![hash]);
                }
            }
            // Honest peers send their stored objects, so never mistyped;
            // another announcer's copy may be the genuine one (D42).
            Err(ObjectRejection::WireTypeMismatch) => {
                self.misbehaved(from, "mistyped governance object");
                self.refused(item, from, taken);
            }
            // Core relays only objects it accepted, so an honest peer may
            // send one the wallet is stricter about: no penalty (D35).
            Err(e) if e.is_invalid() => {
                tracing::debug!(?e, "governance object refused");
                if self.refused(item, from, taken) {
                    self.shared.store().refuse_object(hash);
                }
            }
            Err(e) => {
                tracing::debug!(?e, "governance object not kept");
                self.finish(&item);
                // A bound refused it: its announcers still count, so a
                // later announcement can confirm it on arrival (D47).
                if e != ObjectRejection::StoreFull {
                    self.requests.object_announcers.remove(&hash);
                }
                self.progressed();
            }
        }
    }

    /// Learns a vote's object: every peer that announced the vote before
    /// the run knew it has it counted now (D46).
    fn credit_relays(&mut self, vote: [u8; 32], parent: [u8; 32]) {
        if self.vote_parents.len() >= MAX_LATE {
            self.vote_parents.clear();
        }
        self.vote_parents.insert(vote, parent);
        for p in &mut self.peers {
            if p.unresolved.remove(&vote) {
                p.attribute(vote, parent);
            }
        }
    }

    fn on_vote(&mut self, from: usize, payload: Vec<u8>) {
        let Ok(vote) = GovernanceVote::decode(&payload) else {
            self.misbehaved(from, "undecodable vote");
            return;
        };
        let hash = vote.hash();
        let item = (hash, INV_GOVERNANCE_VOTE);
        let Some(taken) = self.take(from, &item) else {
            return;
        };
        self.credit_relays(hash, vote.parent_hash);
        let now = unix_now();
        // The hash leaves the signature out: when this copy's signature is
        // the problem, another peer's copy may be the genuine one.
        let prechecked = self.shared.store().precheck(&vote, now);
        let outcome = match prechecked {
            Ok(o) => o,
            Err(r) if r.is_invalid() => {
                self.misbehaved(from, "invalid vote");
                self.refused(item, from, taken);
                return;
            }
            Err(_) => {
                self.finish(&item);
                self.progressed();
                return;
            }
        };
        // Recovered without the store's lock.
        let checked = match recover(vote, outcome, self.shared.network) {
            Ok(c) => c,
            Err(_) => {
                self.misbehaved(from, "vote signature recovers no key");
                self.refused(item, from, taken);
                return;
            }
        };
        let parent = checked.vote.parent_hash;
        let addr = self.peer(from).map(|p| p.peer.addr);
        let result = self.shared.store().add_checked(checked, addr, now);
        match result {
            Ok(()) => {
                self.finish(&item);
                self.progressed();
                self.changed();
            }
            Err(VoteRejection::Orphan) => {
                self.finish(&item);
                self.progressed();
                self.fetch_parent(from, parent);
            }
            Err(r) if r.is_suspect() || r.is_invalid() => {
                if r.is_suspect() {
                    self.suspected(from);
                } else {
                    self.misbehaved(from, "invalid vote");
                }
                self.refused(item, from, taken);
            }
            Err(_) => {
                self.finish(&item);
                self.progressed();
            }
        }
    }

    /// Core asks every peer for an orphan vote's object (`govsync <hash>`
    /// with an empty filter, which Core does not rate-limit), so its
    /// announcers can confirm it; once per parent and peer. The vote's
    /// sender pays: [`SyncConfig::max_parent_fetches`] parents per sender
    /// (D52).
    fn fetch_parent(&mut self, from: usize, parent: [u8; 32]) {
        let known = self.shared.store().has_object(&parent)
            || self
                .requests
                .pending
                .contains_key(&(parent, INV_GOVERNANCE_OBJECT));
        let max = self.cfg.max_parent_fetches;
        let Some(sender) = self.peer_mut(from) else {
            return;
        };
        if known || sender.parent_fetches.len() >= max || !sender.parent_fetches.insert(parent) {
            return;
        }
        for p in &mut self.peers {
            if p.parents_asked.insert(parent) {
                p.peer.send_govsync(parent, &object_fetch_filter());
            }
        }
    }

    fn on_not_found(&mut self, from: usize, items: Vec<InvItem>) {
        for i in items {
            let asked = |item: &Item, run: &Self| {
                run.requests
                    .pending
                    .get(item)
                    .is_some_and(|r| r.peer == from)
            };
            let mut item = (i.hash, i.kind);
            if !asked(&item, self) {
                // Naming a request of the other kind ends it all the same,
                // as misbehaviour, and the request moves on with its own
                // kind (D50).
                item.1 = other_kind(i.kind);
                if !asked(&item, self) {
                    continue;
                }
                self.misbehaved(from, "notfound of another kind");
            }
            let r = self.release(&item).expect("looked up");
            let taken = Taken {
                phase: r.phase,
                asked: r.asked,
            };
            // A peer that no longer has an item it announced (a vote
            // dropped with a list change) is not a lost sync item.
            self.retry(item, from, taken, false);
        }
    }

    fn on_get_data(&self, from: usize, items: Vec<InvItem>) {
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
                INV_GOVERNANCE_OBJECT => p.peer.send_object(payload),
                _ => p.peer.send_vote(payload),
            }
        }
    }

    fn on_closed(&mut self, id: usize) {
        let Some(pos) = self.peers.iter().position(|p| p.peer.id == id) else {
            return;
        };
        let gone = self.peers.remove(pos);
        let pending: Vec<Item> = self
            .requests
            .pending
            .iter()
            .filter(|(_, p)| p.peer == id)
            .map(|(i, _)| *i)
            .collect();
        // Its vote syncs go to another peer unless it both announced and
        // delivered every vote: its `ssc` may have come before its invs, and
        // votes it announced but did not send may have no other announcer.
        // Once synced, its pending votes are relays, which other announcers
        // cover; the phase it served is over.
        let votes_owed = pending.iter().any(|(_, kind)| *kind == INV_GOVERNANCE_VOTE);
        let reask = if self.announce_open || (gone.votes_done() && !votes_owed) {
            Vec::new()
        } else {
            gone.vote_asked.clone()
        };
        // Its announcements of objects not fetched yet move to the late-copy
        // record, past its bound: they still count when another peer
        // delivers (D53), and there are no more of them than the request
        // records held for it.
        let Requests {
            announced,
            object_announcers,
            ..
        } = &mut self.requests;
        for ((hash, kind), a) in announced.iter_mut() {
            a.untried.retain(|p| *p != id);
            let before = a.all.len();
            a.all.retain(|p| *p != id);
            if *kind == INV_GOVERNANCE_OBJECT && a.all.len() < before {
                let by = object_announcers.entry(*hash).or_default();
                if !by.contains(&gone.peer.addr) {
                    by.push(gone.peer.addr);
                }
            }
        }
        for item in pending {
            let r = self.requests.pending.remove(&item).expect("listed");
            let taken = Taken {
                phase: r.phase,
                asked: r.asked,
            };
            // Votes come again from the re-asked syncs; objects it alone
            // announced are not sync items another fresh peer has.
            self.retry(item, id, taken, false);
        }
        self.requests.announced.retain(|_, a| !a.all.is_empty());
        if !reask.is_empty() {
            self.ask_votes(reask);
        }
        self.changed();
    }

    fn on_sync_count(&mut self, from: usize, item: i32, count: i32) {
        let timeout = self.cfg.answer_timeout;
        let Some(p) = self.peer_mut(from) else { return };
        match item {
            SSC_GOVOBJ => match p.full_sync.as_mut() {
                Some(f) if f.expected.is_none() => f.expected = Some(count),
                _ => return,
            },
            SSC_GOVOBJ_VOTE if p.vote_expected.len() < p.vote_asked.len() => {
                p.vote_expected.push(i64::from(count));
            }
            _ => return,
        }
        p.rearm_answer(timeout);
        self.progressed();
    }

    fn handle(&mut self, from: usize, ev: PeerEvent) {
        match ev {
            PeerEvent::Inv(items) => self.on_inv(from, items),
            PeerEvent::GetData(items) => self.on_get_data(from, items),
            PeerEvent::NotFound(items) => self.on_not_found(from, items),
            PeerEvent::Object(payload) => self.on_object(from, payload),
            PeerEvent::Vote(payload) => self.on_vote(from, payload),
            PeerEvent::SyncCount { item, count } => self.on_sync_count(from, item, count),
            PeerEvent::Malformed => self.misbehaved(from, "malformed message"),
            PeerEvent::Closed => self.on_closed(from),
        }
    }

    /// Requests whose peer delivered nothing for the timeout, or sent
    /// longer ago than the age cap, move to the next announcer, or are
    /// asked again once, or given up; a request queued for the timeout
    /// moves when another announcer can be asked (D55); peers past their
    /// `ssc` deadline are closed; the store is pruned.
    fn expire(&mut self) {
        let (timeout, max_age) = (self.cfg.request_timeout, self.cfg.max_request_age);
        let now = Instant::now();
        let others = |run: &Self, item: &Item, r: &Pending| {
            run.requests.announced.get(item).is_some_and(|a| {
                a.untried
                    .iter()
                    .any(|p| *p != r.peer && !r.asked.contains(p) && run.peer(*p).is_some())
            })
        };
        let late: Vec<Item> = self
            .requests
            .pending
            .iter()
            .filter(|(item, r)| match r.sent {
                Some(sent) => {
                    let since = self
                        .peer(r.peer)
                        .map_or(sent, |p| p.last_delivery.max(sent));
                    now.duration_since(since) >= timeout || now.duration_since(sent) >= max_age
                }
                None => now.duration_since(r.queued) >= timeout && others(self, item, r),
            })
            .map(|(i, _)| *i)
            .collect();
        for item in late {
            let r = &self.requests.pending[&item];
            let (peer, again) = (r.peer, r.asked_again);
            if r.sent.is_some() && !again && !others(self, &item, r) {
                let r = self.requests.pending.get_mut(&item).expect("listed");
                r.asked_again = true;
                r.sent = Some(now);
                if let Some(p) = self.peer(peer) {
                    let (hash, kind) = item;
                    p.peer.send_getdata(&[InvItem { kind, hash }]);
                }
                continue;
            }
            let r = self.release(&item).expect("listed");
            let taken = Taken {
                phase: r.phase,
                asked: r.asked,
            };
            self.retry(item, peer, taken, true);
        }
        for p in &self.peers {
            if p.answer_due.is_some_and(|due| due <= now) {
                tracing::info!(peer = %p.peer.addr, "closing a governance peer that did not answer its sync");
                p.peer.close();
            }
        }
        self.shared.store().prune(unix_now());
    }

    /// Asks peers for the votes of the current proposals among `hashes`,
    /// each of up to [`SyncConfig::vote_peers_per_proposal`] connected
    /// peers that did not vote-sync it within the window, spread over them,
    /// so no one peer can withhold a proposal's votes (D56); hashes no peer
    /// may be asked for are kept in `unassigned`.
    fn ask_votes(&mut self, hashes: Vec<[u8; 32]>) {
        let window = self.cfg.repeat_window;
        let timeout = self.cfg.answer_timeout;
        let now = unix_now();
        let hashes: Vec<[u8; 32]> = {
            let store = self.shared.store();
            hashes
                .into_iter()
                .filter(|h| store.is_current_proposal(h, now))
                .collect()
        };
        let filter = empty_vote_filter();
        let per_proposal = self.cfg.vote_peers_per_proposal.max(1);
        let mut turn = 0usize;
        for hash in hashes {
            let n = self.peers.len();
            let asked = self
                .peers
                .iter()
                .filter(|p| p.vote_asked.contains(&hash))
                .count();
            let picks: Vec<usize> = {
                let h = self.history.lock().unwrap_or_else(|p| p.into_inner());
                (0..n)
                    .map(|k| (turn + k) % n)
                    .filter(|&i| !h.votes_recently_synced(&self.peers[i].peer.addr, &hash, window))
                    .take(per_proposal.saturating_sub(asked))
                    .collect()
            };
            if picks.is_empty() && asked == 0 {
                self.unassigned.push(hash);
            }
            for i in picks {
                turn = i + 1;
                self.ask_votes_of(i, hash, &filter, timeout);
            }
        }
    }

    /// Asks peer `i` for the votes on `hash`.
    fn ask_votes_of(&mut self, i: usize, hash: [u8; 32], filter: &[u8], timeout: Duration) {
        let addr = self.peers[i].peer.addr;
        self.history
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .vote_synced
            .insert((addr, hash), Instant::now());
        let p = &mut self.peers[i];
        p.vote_asked.push(hash);
        let before = p.relayed.len();
        p.relayed.retain(|_, parent| *parent != hash);
        *p.vote_announced.entry(hash).or_default() += (before - p.relayed.len()) as i64;
        p.peer.send_govsync(hash, filter);
        if p.answer_due.is_none() {
            p.rearm_answer(timeout);
        }
    }

    /// Asks the peers that did not full-sync this run to announce the
    /// objects only one peer announced (Core answers `govsync <hash>` with
    /// an empty filter by an inv of the object), so they can confirm them.
    fn ask_confirmations(&mut self) {
        // Each first sender's objects in turn, so one peer's cannot use up
        // the asks (D53).
        let wanted: Vec<([u8; 32], Vec<SocketAddr>)> = self
            .shared
            .store()
            .unconfirmed_this_run()
            .into_iter()
            .map(|o| (o.object.hash(), o.announcers.clone()))
            .collect();
        for (hash, by) in wanted.into_iter().take(MAX_CONFIRMATION_ASKS) {
            for p in self
                .peers
                .iter()
                .filter(|p| p.full_sync.is_none() && !by.contains(&p.peer.addr))
            {
                p.peer.send_govsync(hash, &object_fetch_filter());
            }
        }
    }

    fn phase_requests_done(&self) -> bool {
        !self.requests.pending.values().any(|r| r.phase)
    }

    fn note_full_syncs(&mut self) {
        for p in &mut self.peers {
            if let Some(f) = p.full_sync.as_mut()
                && f.complete()
                && !f.counted
            {
                f.counted = true;
                self.full_syncs_done += 1;
            }
        }
    }

    /// Fails the run once the object cap evicted or refused an object
    /// announced in it: the store no longer holds what the peers have
    /// (D47).
    fn check_overflow(&self) -> Result<(), SyncError> {
        match self.shared.store().take_overflow() {
            0 => Ok(()),
            n => Err(SyncError::Incomplete(format!(
                "the governance store is full: {n} objects of this run did not fit"
            ))),
        }
    }

    /// Processes events until `done` holds and the phase's requests are
    /// done (`Ok(true)`), or `stop` (`Ok(false)`); fails when items were
    /// given up on, nothing progressed for the idle timeout, the phase ran
    /// past its deadline, or every peer is gone.
    async fn drain(
        &mut self,
        rx: &mut mpsc::Receiver<Inbound>,
        stop: &mut watch::Receiver<bool>,
        done: impl Fn(&Run) -> Result<bool, SyncError>,
    ) -> Result<bool, SyncError> {
        let started = Instant::now();
        self.last_progress = started;
        self.lost = 0;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            self.note_full_syncs();
            if done(self)? && self.phase_requests_done() {
                if self.lost > 0 {
                    return Err(SyncError::Incomplete(format!(
                        "{} items could not be fetched",
                        self.lost
                    )));
                }
                return Ok(true);
            }
            if self.peers.is_empty() {
                return Err(SyncError::PeersLost);
            }
            let idle = self
                .cfg
                .idle_timeout
                .saturating_sub(self.last_progress.elapsed());
            let deadline = self.cfg.phase_deadline.saturating_sub(started.elapsed());
            tokio::select! {
                r = stop.changed() => {
                    if r.is_err() || *stop.borrow() { return Ok(false); }
                }
                _ = tokio::time::sleep(idle) => {
                    return Err(SyncError::Incomplete("no progress within the idle timeout".into()));
                }
                _ = tokio::time::sleep(deadline) => {
                    return Err(SyncError::Incomplete("the phase ran past its deadline".into()));
                }
                _ = tick.tick() => self.expire(),
                ev = rx.recv() => {
                    let Some(ev) = ev else { return Err(SyncError::PeersLost) };
                    // Its credit is held until the event is handled.
                    self.handle(ev.from, ev.event);
                    self.shared.refresh(&self.peers, self.base_bytes);
                    self.check_overflow()?;
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
    // Overflow before the run is none of its business.
    shared.store().take_overflow();
    let (tx, mut rx) = mpsc::channel(EVENTS_QUEUE);
    let (peers, errors) = connect_peers(&candidates, &cfg, &history, &tx).await;
    // A stop while connecting must not spend the peers' repeat windows.
    if *stop.borrow() {
        return Ok(());
    }
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
        announce_open: false,
        unassigned: Vec::new(),
        lost: 0,
        full_syncs_done: 0,
        vote_parents: HashMap::new(),
        last_progress: Instant::now(),
    };
    run.changed();

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
        shared.store().begin_run();
        shared.set_phase(Phase::SyncingObjects);
        let started = Instant::now();
        let filter = object_fetch_filter();
        let timeout = run.cfg.answer_timeout;
        for p in run.peers.iter_mut().filter(|p| fresh.contains(&p.peer.id)) {
            p.full_sync = Some(FullSync::default());
            run.history
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .full_synced
                .insert(p.peer.addr, Instant::now());
            p.peer.send_govsync([0; 32], &filter);
            p.rearm_answer(timeout);
        }
        let objects_done = |r: &Run| {
            if r.full_syncs_done == 0 && !r.peers.iter().any(|p| p.full_sync.is_some()) {
                return Err(SyncError::Incomplete(
                    "no peer completed the object sync".into(),
                ));
            }
            Ok(r.full_syncs_done > 0 && r.peers.iter().all(PeerState::objects_done))
        };
        if !run.drain(&mut rx, stop, objects_done).await? {
            return Ok(());
        }
        shared.progress().objects_secs = Some(started.elapsed().as_secs_f64());
        run.ask_confirmations();

        // Votes of the current proposals, spread over the peers.
        shared.set_phase(Phase::SyncingVotes);
        let started = Instant::now();
        let current = shared.store().current_proposals(unix_now());
        run.votes_open = true;
        run.ask_votes(current);
        if !run
            .drain(&mut rx, stop, |r| {
                Ok(r.peers.iter().all(PeerState::votes_done))
            })
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

    shared.store().set_synced();
    {
        let mut p = shared.progress();
        p.phase = Phase::Synced;
        p.last_synced_at = Some(unix_now() as u64);
        p.last_error = None;
        p.next_retry_at = None;
    }
    run.changed();
    run.announce_open = true;
    run.announce_outbox(false);

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
            _ = tick.tick() => run.expire(),
            _ = shared.relay.notified() => run.announce_outbox(true),
            ev = rx.recv() => {
                let Some(ev) = ev else { return Err(SyncError::PeersLost) };
                run.handle(ev.from, ev.event);
                shared.refresh(&run.peers, run.base_bytes);
                run.check_overflow()?;
            }
        }
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
