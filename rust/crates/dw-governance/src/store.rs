//! The synced governance objects and current votes, in memory (MG-02 §3,
//! §4.1).
//!
//! Objects arrive checked ([`crate::validate`]). One counts (is listed,
//! tallied, used for funded heights) once enough distinct peers announced
//! it, two by default (MG-02 D3): the wallet cannot see the collateral or a
//! trigger's operator signature, which each relaying full node checked.
//! Objects waiting for confirmation are bounded per first sender and in all,
//! by count and bytes, and dropped after [`UNCONFIRMED_TTL_SECS`]. Objects
//! leave as Core's `UpdateCachesAndClean` erases them: a proposal
//! [`DELETION_DELAY_SECS`] after its end epoch, a trigger a superblock cycle
//! after its height (one paid is kept that long, `CSuperblock::IsExpired`;
//! MG-02 D17). Their hashes are kept so they are not fetched again: a
//! proposal's for the session, a trigger's until two cycles after its
//! creation (`mapErasedGovernanceObjects`).
//!
//! Votes are checked in two steps so the signature recovery runs without
//! the store's lock: [`GovernanceStore::precheck`] (cheap, `&mut` to
//! remember ignored hashes), [`recover`], then
//! [`GovernanceStore::add_checked`]. Core relays votes the wallet does not
//! tally (other signals, `NONE` outcomes, votes on triggers or on expired
//! proposals): those are ignored and their hashes remembered (the hash
//! covers what made them ignored), with no penalty for the sender.
//!
//! For each proposal the latest funding vote of every (collateral outpoint,
//! voting key) pair is kept. Core keeps one per outpoint
//! (`mapCurrentMNVotes`), but the SPV list has no outpoints: a vote counts
//! when its signature recovers a voting key that a masternode of the tip
//! list holds, PoSe-banned ones included (Core `GetMNByCollateral` finds
//! them), or, for an outpoint the wallet resolved itself, that masternode's
//! key. Keyed by the pair, a key holder who signs for another masternode's
//! outpoint adds a competing claim the tally ranges over, and cannot
//! replace or block that masternode's own votes (MG-02 D1, D19). One key
//! gets at most `2·n + 2` unresolved outpoints per object, `n` being the
//! masternodes holding it (D11); resolved outpoints are exact and not
//! limited.
//!
//! A vote whose object has not arrived waits as an orphan once it passed the
//! checks that need no object (Core `IsValidForUnknownParent`, MG-01 D23):
//! for 10 minutes (`GOVERNANCE_ORPHAN_EXPIRATION_TIME`), at most
//! [`MAX_ORPHAN_VOTES`] in all with the oldest evicted, at most
//! [`MAX_ORPHANS_PER_PEER`] from one peer, one per (object, outpoint, key).
//! Core's per-masternode minimum between vote updates
//! (`GOVERNANCE_UPDATE_MIN`, timed by the node's own receive clock) is not
//! mirrored (MG-01 D13).

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;

use dashcore::{Network, OutPoint};

use crate::object::{GovernanceObject, display_hex};
use crate::params::governance_params;
use crate::proposal::{ProposalData, TriggerData};
use crate::tally::{CurrentVote, KeyWeights};
use crate::validate::{MAX_TIME_FUTURE_DEVIATION, ObjectRejection, ValidObject, check_object};
use crate::vote::{BLS_SIG_SIZE, COMPACT_SIG_SIZE, GovernanceVote, SIGNAL_FUNDING, VoteOutcome};

/// Votes kept for objects that have not arrived, in all.
pub const MAX_ORPHAN_VOTES: usize = 50_000;
/// Orphan votes kept from one peer.
pub const MAX_ORPHANS_PER_PEER: u32 = 2_000;
/// How long an orphan vote waits (Core `GOVERNANCE_ORPHAN_EXPIRATION_TIME`).
pub const ORPHAN_EXPIRY_SECS: i64 = 10 * 60;
/// Distinct peers that must announce an object before it counts (D3).
pub const DEFAULT_CONFIRMATIONS: usize = 2;
/// Unconfirmed objects kept in all, first sent by one peer, and their data
/// bytes in all (triggers have no size rule of their own).
pub const MAX_UNCONFIRMED_OBJECTS: usize = 2_000;
pub const MAX_UNCONFIRMED_PER_PEER: usize = 500;
pub const MAX_UNCONFIRMED_BYTES: usize = 32 << 20;
/// How long an object waits for its confirmations.
pub const UNCONFIRMED_TTL_SECS: i64 = 60 * 60;
/// Core's `GOVERNANCE_UPDATE_MIN`: a masternode's vote on an object is not
/// replaced within the hour.
pub const VOTE_UPDATE_MIN_SECS: i64 = 60 * 60;
/// Hashes of ignored votes remembered, oldest forgotten first.
pub const MAX_IGNORED_VOTES: usize = 100_000;
/// Hashes of objects every announcer's copy of was refused, oldest
/// forgotten first (MG-02 D35).
pub const MAX_REFUSED_OBJECTS: usize = 10_000;
/// Votes whose key the list does not hold yet, kept per (vote, key) for a
/// later list (MG-02 D43); oldest forgotten first.
pub const MAX_SUSPECT_VOTES: usize = 10_000;
/// Of them, at most this many from one peer.
pub const MAX_SUSPECTS_PER_PEER: u32 = 1_000;
/// How long such a vote waits for its key.
pub const SUSPECT_TTL_SECS: i64 = 60 * 60;
/// Core `GOVERNANCE_DELETION_DELAY`.
pub const DELETION_DELAY_SECS: i64 = 10 * 60;
/// Signal and outcome ranges Core accepts (`VOTE_SIGNAL_UNKNOWN`,
/// `VOTE_OUTCOME_UNKNOWN` exclusive).
const MAX_SIGNAL: i32 = 4;
const MAX_OUTCOME: i32 = 3;

/// What an object's data says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectKind {
    Proposal(ProposalData),
    Trigger(TriggerData),
}

/// A synced object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub object: GovernanceObject,
    pub kind: ObjectKind,
    /// Display-order hash.
    pub hash_hex: String,
    /// Distinct peers that announced or sent it (up to the confirmations
    /// needed); the first sent it.
    pub announcers: Vec<SocketAddr>,
    /// Announced by enough peers to count.
    pub confirmed: bool,
    /// When it arrived (local clock).
    pub stored_at: i64,
}

/// Why a vote was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteRejection {
    /// Already stored, waiting or ignored.
    Duplicate,
    /// A vote Core relays that the wallet does not tally: another signal, a
    /// `NONE` outcome, or a vote on a trigger or an expired proposal.
    /// Remembered.
    Ignored,
    /// BLS-signed (an operator's trigger vote) for an object that has not
    /// arrived: the wallet cannot check it. Dropped, not remembered (the
    /// hash leaves the signature out).
    Unverifiable,
    /// A signal or outcome outside Core's ranges, or a signature of neither
    /// size; a BLS signature on a proposal vote.
    Malformed,
    /// The compact signature recovers no key.
    BadSignature,
    /// The recovered key is no voting key of the tip list.
    UnknownVotingKey,
    /// The outpoint is one the wallet resolved, and the key is not its
    /// masternode's.
    WrongVotingKey,
    /// Timed more than an hour ahead of the local clock.
    TooNew,
    /// An older vote than the (outpoint, key)'s current one. Not
    /// remembered, as in Core.
    Obsolete,
    /// Of the same time as the current one with a lower outcome (Core keeps
    /// the higher one). Not remembered.
    SameTimestamp,
    /// The wallet's own vote replaces a current one timed less than an hour
    /// ago (Core `GOVERNANCE_UPDATE_MIN`, D37).
    TooOften,
    /// The wallet's own vote's key is held only by banned masternodes (Core
    /// `GetValidMNByCollateral`).
    Banned,
    /// The key already votes for `2·n + 2` unresolved outpoints on this
    /// object (D11).
    KeyLimit,
    /// Valid, but its object has not arrived: waits, unless the orphan
    /// bounds refused it.
    Orphan,
}

impl VoteRejection {
    /// Whether the peer that sent it misbehaved: no honest node relays it.
    pub fn is_invalid(self) -> bool {
        matches!(self, Self::Malformed | Self::BadSignature)
    }

    /// Whether it may be honest under a different masternode list (the
    /// peer's tip may differ), but costs a signature recovery: counted
    /// against a looser bound (D10).
    pub fn is_suspect(self) -> bool {
        matches!(self, Self::UnknownVotingKey | Self::WrongVotingKey)
    }
}

/// A vote that passed [`GovernanceStore::precheck`], with the key its
/// signature recovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedVote {
    pub vote: GovernanceVote,
    pub hash: [u8; 32],
    pub outcome: VoteOutcome,
    pub key_id: [u8; 20],
}

/// Recovers the signer of a prechecked vote; run without the store's lock.
pub fn recover(
    vote: GovernanceVote,
    outcome: VoteOutcome,
    network: Network,
) -> Result<CheckedVote, VoteRejection> {
    let key_id = vote
        .signer_key_id(network)
        .ok_or(VoteRejection::BadSignature)?;
    Ok(CheckedVote {
        hash: vote.hash(),
        vote,
        outcome,
        key_id,
    })
}

/// A masternode of the tip list as the store needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListEntry {
    pub voting_key_id: [u8; 20],
    /// 1 regular, 4 evonode; `None` when the list does not carry the type
    /// (a revoked masternode, D34).
    pub weight: Option<u32>,
    pub valid: bool,
}

/// An outpoint the wallet resolved to its masternode (MG-03, MG-00 D43).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedMasternode {
    pub voting_key_id: [u8; 20],
    pub weight: u32,
}

#[derive(Debug, Clone)]
struct Orphan {
    vote: CheckedVote,
    from: Option<SocketAddr>,
    at: i64,
}

/// (object, outpoint, key): the unit of a current vote.
type Slot = ([u8; 32], OutPoint, [u8; 20]);
/// (outpoint, key) → current funding vote, for one object.
type ObjectVotes = HashMap<(OutPoint, [u8; 20]), CurrentVote>;

/// What a vote's object is to the store.
enum Parent {
    Proposal,
    Expired,
    Trigger,
    Unknown,
}

/// Synced objects and current votes.
#[derive(Debug)]
pub struct GovernanceStore {
    network: Network,
    confirmations: usize,
    tip: Option<u32>,
    objects: HashMap<[u8; 32], StoredObject>,
    /// Erased objects → until when their hash is kept.
    erased: HashMap<[u8; 32], i64>,
    /// object hash → (outpoint, key) → current funding vote.
    votes: HashMap<[u8; 32], ObjectVotes>,
    /// (object, key) → unresolved outpoints voting with the key.
    per_key: HashMap<([u8; 32], [u8; 20]), u32>,
    seen_votes: HashSet<[u8; 32]>,
    vote_count: u64,
    ignored: HashSet<[u8; 32]>,
    ignored_order: VecDeque<[u8; 32]>,
    refused: HashSet<[u8; 32]>,
    refused_order: VecDeque<[u8; 32]>,
    orphans: HashMap<[u8; 32], Orphan>,
    /// (arrival, hash) in arrival order; entries whose orphan left or was
    /// re-queued are skipped, and compacted when they pile up.
    orphan_order: VecDeque<(i64, [u8; 32])>,
    orphan_slots: HashMap<Slot, [u8; 32]>,
    orphans_by_parent: HashMap<[u8; 32], HashSet<[u8; 32]>>,
    orphans_per_peer: HashMap<SocketAddr, u32>,
    /// Voting key → the masternodes of the tip list holding it.
    keys: HashMap<[u8; 20], KeyWeights>,
    /// Keys held by at least one valid (not banned) masternode.
    valid_keys: HashSet<[u8; 20]>,
    valid_weight: u32,
    list_size: u32,
    resolved: HashMap<OutPoint, ResolvedMasternode>,
    /// (vote, recovered key) → a vote refused for a key the list lacks.
    suspects: HashMap<([u8; 32], [u8; 20]), Orphan>,
    suspect_order: VecDeque<([u8; 32], [u8; 20])>,
    suspects_per_peer: HashMap<SocketAddr, u32>,
    /// Whether a sync has completed, after which triggers outside their
    /// creation-time window are ignored (Core's `MasternodeRateCheck`).
    synced: bool,
}

/// Adds `hash` to a bounded set, forgetting the oldest past `max`.
fn remember(
    set: &mut HashSet<[u8; 32]>,
    order: &mut VecDeque<[u8; 32]>,
    max: usize,
    hash: [u8; 32],
) {
    if !set.insert(hash) {
        return;
    }
    order.push_back(hash);
    if order.len() > max
        && let Some(old) = order.pop_front()
    {
        set.remove(&old);
    }
}

impl GovernanceStore {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            confirmations: DEFAULT_CONFIRMATIONS,
            tip: None,
            objects: HashMap::new(),
            erased: HashMap::new(),
            votes: HashMap::new(),
            per_key: HashMap::new(),
            seen_votes: HashSet::new(),
            vote_count: 0,
            ignored: HashSet::new(),
            ignored_order: VecDeque::new(),
            refused: HashSet::new(),
            refused_order: VecDeque::new(),
            orphans: HashMap::new(),
            orphan_order: VecDeque::new(),
            orphan_slots: HashMap::new(),
            orphans_by_parent: HashMap::new(),
            orphans_per_peer: HashMap::new(),
            keys: HashMap::new(),
            valid_keys: HashSet::new(),
            valid_weight: 0,
            list_size: 0,
            resolved: HashMap::new(),
            suspects: HashMap::new(),
            suspect_order: VecDeque::new(),
            suspects_per_peer: HashMap::new(),
            synced: false,
        }
    }

    /// Notes that a sync completed: from now on triggers are checked
    /// against their creation-time window, as Core does once synced.
    pub fn set_synced(&mut self) {
        self.synced = true;
    }

    /// Sets how many distinct peers must announce an object (at least 1).
    pub fn set_confirmations(&mut self, n: usize) {
        self.confirmations = n.max(1);
        let n = self.confirmations;
        for o in self.objects.values_mut() {
            o.confirmed = o.confirmed || o.announcers.len() >= n;
        }
    }

    pub fn network(&self) -> Network {
        self.network
    }

    /// Sets the chain tip height (triggers are erased against it).
    pub fn set_tip(&mut self, height: u32) {
        self.tip = Some(height);
    }

    /// Objects stored, confirmed or not.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn unconfirmed_count(&self) -> usize {
        self.objects.values().filter(|o| !o.confirmed).count()
    }

    /// Current votes stored, all objects.
    pub fn vote_count(&self) -> u64 {
        self.vote_count
    }

    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// Whether `hash` is stored, was erased or was refused (either way it
    /// is not fetched).
    pub fn has_object(&self, hash: &[u8; 32]) -> bool {
        self.objects.contains_key(hash)
            || self.erased.contains_key(hash)
            || self.refused.contains(hash)
    }

    /// Remembers an object the wallet refused from every peer that sent
    /// it, so it is not fetched again and its votes are ignored (D35).
    pub fn refuse_object(&mut self, hash: [u8; 32]) {
        if !self.objects.contains_key(&hash) {
            remember(
                &mut self.refused,
                &mut self.refused_order,
                MAX_REFUSED_OBJECTS,
                hash,
            );
        }
    }

    /// Whether the vote `hash` is counted, waits for its object, or was
    /// ignored.
    pub fn has_vote(&self, hash: &[u8; 32]) -> bool {
        self.seen_votes.contains(hash)
            || self.orphans.contains_key(hash)
            || self.ignored.contains(hash)
    }

    /// A confirmed object.
    pub fn object(&self, hash: &[u8; 32]) -> Option<&StoredObject> {
        self.objects.get(hash).filter(|o| o.confirmed)
    }

    /// The confirmed objects.
    pub fn objects(&self) -> impl Iterator<Item = &StoredObject> {
        self.objects.values().filter(|o| o.confirmed)
    }

    /// The objects waiting for confirmations.
    pub fn unconfirmed(&self) -> impl Iterator<Item = &StoredObject> {
        self.objects.values().filter(|o| !o.confirmed)
    }

    /// The current funding votes on `hash`.
    pub fn votes_on(&self, hash: &[u8; 32]) -> impl Iterator<Item = &CurrentVote> {
        self.votes.get(hash).into_iter().flat_map(|m| m.values())
    }

    /// The current votes claiming `outpoint` on `hash`: one per key (more
    /// than one only when a key holder signs for an outpoint not theirs).
    pub fn votes_of(
        &self,
        hash: &[u8; 32],
        outpoint: &OutPoint,
    ) -> impl Iterator<Item = &CurrentVote> {
        self.votes_on(hash).filter(move |v| &v.outpoint == outpoint)
    }

    /// The masternodes holding `key`, if any.
    pub fn key_weights(&self, key: &[u8; 20]) -> Option<KeyWeights> {
        self.keys.get(key).copied()
    }

    /// The masternode an outpoint was resolved to.
    pub fn resolved(&self, outpoint: &OutPoint) -> Option<ResolvedMasternode> {
        self.resolved.get(outpoint).copied()
    }

    /// Weight of the valid masternodes of the tip list (Core
    /// `m_valid_weighted`, for the passing threshold).
    pub fn valid_weight(&self) -> u32 {
        self.valid_weight
    }

    /// Masternodes in the tip list, banned included.
    pub fn list_size(&self) -> u32 {
        self.list_size
    }

    /// Records that `peer` announced or sent `hash`; returns whether that
    /// confirmed a stored object.
    pub fn note_announcer(&mut self, hash: &[u8; 32], peer: SocketAddr) -> bool {
        let n = self.confirmations;
        let Some(o) = self.objects.get_mut(hash) else {
            return false;
        };
        if o.confirmed || o.announcers.contains(&peer) {
            return false;
        }
        o.announcers.push(peer);
        o.confirmed = o.announcers.len() >= n;
        o.confirmed
    }

    /// Checks and stores `object` sent by `from` (`None`: the wallet's
    /// own). Returns whether it was new; orphan votes waiting for it are
    /// applied.
    pub fn add_object(
        &mut self,
        object: GovernanceObject,
        from: Option<SocketAddr>,
        now: i64,
    ) -> Result<bool, ObjectRejection> {
        let hash = object.hash();
        if self.has_object(&hash) {
            if let Some(peer) = from {
                self.note_announcer(&hash, peer);
            }
            return Ok(false);
        }
        let kind = match check_object(&object, self.network, now, self.synced)? {
            ValidObject::Proposal(p) => ObjectKind::Proposal(p),
            ValidObject::Trigger(t) => ObjectKind::Trigger(t),
        };
        let announcers: Vec<SocketAddr> = from.into_iter().collect();
        let confirmed = from.is_none() || announcers.len() >= self.confirmations;
        if !confirmed {
            self.room_for_unconfirmed(from, object.data.len())?;
        }
        self.objects.insert(
            hash,
            StoredObject {
                object,
                kind,
                hash_hex: display_hex(&hash),
                announcers,
                confirmed,
                stored_at: now,
            },
        );
        self.adopt_orphans(&hash, now);
        Ok(true)
    }

    fn room_for_unconfirmed(
        &self,
        from: Option<SocketAddr>,
        bytes: usize,
    ) -> Result<(), ObjectRejection> {
        let (mut count, mut from_peer, mut total) = (0usize, 0usize, bytes);
        for o in self.objects.values().filter(|o| !o.confirmed) {
            count += 1;
            total += o.object.data.len();
            if from.is_some() && o.announcers.first() == from.as_ref() {
                from_peer += 1;
            }
        }
        if count >= MAX_UNCONFIRMED_OBJECTS
            || from_peer >= MAX_UNCONFIRMED_PER_PEER
            || total > MAX_UNCONFIRMED_BYTES
        {
            return Err(ObjectRejection::StoreFull);
        }
        Ok(())
    }

    /// Erases what Core's cleanup would by `now` (and the tip set), drops
    /// objects unconfirmed for [`UNCONFIRMED_TTL_SECS`] and orphans past
    /// their expiry, and forgets erased hashes whose time passed.
    pub fn prune(&mut self, now: i64) {
        let expired: Vec<_> = self
            .suspects
            .iter()
            .filter(|(_, o)| o.at + SUSPECT_TTL_SECS <= now)
            .map(|(slot, _)| *slot)
            .collect();
        for slot in expired {
            self.remove_suspect(&slot);
        }
        let p = governance_params(self.network);
        let cycle = i64::from(p.superblock_cycle);
        let cycle_secs = cycle * i64::from(p.target_spacing_secs);
        let tip = self.tip.map(i64::from);
        let mut erase = Vec::new();
        let mut drop = Vec::new();
        for (hash, o) in &self.objects {
            let gone = match &o.kind {
                ObjectKind::Proposal(d) => d
                    .end_epoch
                    .is_some_and(|e| e.saturating_add(DELETION_DELAY_SECS) <= now),
                ObjectKind::Trigger(t) => tip.is_some_and(|tip| {
                    let h = i64::from(t.event_block_height);
                    tip > h + cycle || (self.network != Network::Mainnet && tip + 2 * cycle < h)
                }),
            };
            if !o.confirmed && (gone || o.stored_at + UNCONFIRMED_TTL_SECS <= now) {
                drop.push(*hash);
            } else if gone {
                let until = match o.kind {
                    ObjectKind::Proposal(_) => i64::MAX,
                    ObjectKind::Trigger(_) => o
                        .object
                        .time
                        .saturating_add(2 * cycle_secs + DELETION_DELAY_SECS),
                };
                erase.push((*hash, until));
            }
        }
        for hash in drop {
            self.remove_object(&hash);
        }
        for (hash, until) in erase {
            self.remove_object(&hash);
            self.erased.insert(hash, until);
        }
        self.erased.retain(|_, until| *until >= now);
        self.expire_orphans(now);
    }

    fn remove_object(&mut self, hash: &[u8; 32]) {
        self.objects.remove(hash);
        for (_, v) in self.votes.remove(hash).unwrap_or_default() {
            self.seen_votes.remove(&v.vote_hash);
            self.vote_count -= 1;
        }
        self.per_key.retain(|(object, _), _| object != hash);
    }

    fn adopt_orphans(&mut self, parent: &[u8; 32], now: i64) {
        let waiting = self.orphans_by_parent.remove(parent).unwrap_or_default();
        for h in waiting {
            if let Some(o) = self.remove_orphan(&h) {
                let _ = self.add_checked(o.vote, o.from, now);
            }
        }
    }

    fn remove_orphan(&mut self, hash: &[u8; 32]) -> Option<Orphan> {
        let o = self.orphans.remove(hash)?;
        let v = &o.vote.vote;
        self.orphan_slots
            .remove(&(v.parent_hash, v.masternode_outpoint, o.vote.key_id));
        if let Some(set) = self.orphans_by_parent.get_mut(&v.parent_hash) {
            set.remove(hash);
            if set.is_empty() {
                self.orphans_by_parent.remove(&v.parent_hash);
            }
        }
        if let Some(peer) = o.from
            && let Some(n) = self.orphans_per_peer.get_mut(&peer)
        {
            *n -= 1;
            if *n == 0 {
                self.orphans_per_peer.remove(&peer);
            }
        }
        Some(o)
    }

    /// Whether an `orphan_order` entry is still the orphan's own.
    fn live_entry(&self, at: i64, hash: &[u8; 32]) -> bool {
        self.orphans.get(hash).is_some_and(|o| o.at == at)
    }

    /// Drops orphans older than [`ORPHAN_EXPIRY_SECS`].
    pub fn expire_orphans(&mut self, now: i64) {
        while let Some(&(at, h)) = self.orphan_order.front() {
            if !self.live_entry(at, &h) {
                self.orphan_order.pop_front();
            } else if at + ORPHAN_EXPIRY_SECS < now {
                self.orphan_order.pop_front();
                self.remove_orphan(&h);
            } else {
                break;
            }
        }
    }

    fn evict_oldest_orphan(&mut self) {
        while let Some((at, h)) = self.orphan_order.pop_front() {
            if self.live_entry(at, &h) {
                self.remove_orphan(&h);
                return;
            }
        }
    }

    /// Sets the masternodes of the tip list, PoSe-banned ones included, and
    /// drops the votes and orphans whose key no longer belongs to one (Core
    /// `RemoveInvalidVotes`). Dropped votes become unseen, so a newer one
    /// can be fetched again.
    pub fn set_masternode_list(&mut self, entries: &[ListEntry]) {
        let mut keys: HashMap<[u8; 20], KeyWeights> = HashMap::new();
        let mut valid_keys = HashSet::new();
        let mut valid_weight = 0u32;
        for e in entries {
            keys.entry(e.voting_key_id)
                .and_modify(|k| k.add(e.weight))
                .or_insert_with(|| KeyWeights::one(e.weight));
            // Revoked entries are banned, so `None` never counts here.
            if e.valid {
                valid_weight = valid_weight.saturating_add(e.weight.unwrap_or(1));
                valid_keys.insert(e.voting_key_id);
            }
        }
        self.keys = keys;
        self.valid_keys = valid_keys;
        self.valid_weight = valid_weight;
        self.list_size = entries.len() as u32;
        self.drop_unbacked_votes();
        self.place_suspects();
    }

    /// Sets the outpoints the wallet resolved to masternodes (MG-03); their
    /// votes are checked against that masternode's key and counted exactly.
    pub fn set_resolved(&mut self, resolved: HashMap<OutPoint, ResolvedMasternode>) {
        self.resolved = resolved;
        self.drop_unbacked_votes();
        let mut per_key: HashMap<([u8; 32], [u8; 20]), u32> = HashMap::new();
        for (object, m) in &self.votes {
            for (outpoint, key) in m.keys() {
                if !self.resolved.contains_key(outpoint) {
                    *per_key.entry((*object, *key)).or_default() += 1;
                }
            }
        }
        self.per_key = per_key;
    }

    fn key_ok(&self, outpoint: &OutPoint, key: &[u8; 20]) -> Result<(), VoteRejection> {
        match self.resolved.get(outpoint) {
            Some(r) if &r.voting_key_id == key => Ok(()),
            Some(_) => Err(VoteRejection::WrongVotingKey),
            None if self.keys.contains_key(key) => Ok(()),
            None => Err(VoteRejection::UnknownVotingKey),
        }
    }

    fn drop_unbacked_votes(&mut self) {
        let mut dropped = Vec::new();
        for (object, per_object) in &self.votes {
            for (outpoint, key) in per_object.keys() {
                if self.key_ok(outpoint, key).is_err() {
                    dropped.push((*object, *outpoint, *key));
                }
            }
        }
        for slot in dropped {
            self.remove_vote(&slot);
        }
        let stale: Vec<[u8; 32]> = self
            .orphans
            .iter()
            .filter(|(_, o)| {
                self.key_ok(&o.vote.vote.masternode_outpoint, &o.vote.key_id)
                    .is_err()
            })
            .map(|(h, _)| *h)
            .collect();
        for h in stale {
            self.remove_orphan(&h);
        }
    }

    fn remove_vote(&mut self, (object, outpoint, key): &Slot) {
        let Some(v) = self
            .votes
            .get_mut(object)
            .and_then(|m| m.remove(&(*outpoint, *key)))
        else {
            return;
        };
        self.seen_votes.remove(&v.vote_hash);
        self.vote_count -= 1;
        if !self.resolved.contains_key(outpoint)
            && let Some(n) = self.per_key.get_mut(&(*object, *key))
        {
            *n -= 1;
            if *n == 0 {
                self.per_key.remove(&(*object, *key));
            }
        }
    }

    fn remember_ignored(&mut self, hash: [u8; 32]) {
        remember(
            &mut self.ignored,
            &mut self.ignored_order,
            MAX_IGNORED_VOTES,
            hash,
        );
    }

    fn parent(&self, hash: &[u8; 32], now: i64) -> Parent {
        // A refused parent's votes are ignored as an erased one's are.
        if self.erased.contains_key(hash) || self.refused.contains(hash) {
            return Parent::Expired;
        }
        match self.objects.get(hash).map(|o| &o.kind) {
            None => Parent::Unknown,
            Some(ObjectKind::Trigger(_)) => Parent::Trigger,
            Some(ObjectKind::Proposal(p)) if p.end_epoch.is_some_and(|e| e <= now) => {
                Parent::Expired
            }
            Some(ObjectKind::Proposal(_)) => Parent::Proposal,
        }
    }

    /// The checks that need no signature recovery, in Core's order where it
    /// has one (`CGovernanceVote::IsValid`, then the object's state).
    /// Remembers ignored votes. Returns the outcome to tally.
    pub fn precheck(
        &mut self,
        vote: &GovernanceVote,
        now: i64,
    ) -> Result<VoteOutcome, VoteRejection> {
        let hash = vote.hash();
        if self.has_vote(&hash) {
            return Err(VoteRejection::Duplicate);
        }
        if vote.time > now + MAX_TIME_FUTURE_DEVIATION {
            return Err(VoteRejection::TooNew);
        }
        if !(0..=MAX_SIGNAL).contains(&vote.signal)
            || !(0..=MAX_OUTCOME).contains(&vote.outcome)
            || !matches!(vote.signature.len(), COMPACT_SIG_SIZE | BLS_SIG_SIZE)
        {
            return Err(VoteRejection::Malformed);
        }
        let parent = self.parent(&vote.parent_hash, now);
        let outcome = vote.vote_outcome();
        if matches!(parent, Parent::Trigger | Parent::Expired)
            || vote.signal != SIGNAL_FUNDING
            || outcome.is_none()
        {
            self.remember_ignored(hash);
            return Err(VoteRejection::Ignored);
        }
        if vote.signature.len() == BLS_SIG_SIZE {
            return Err(match parent {
                Parent::Proposal => VoteRejection::Malformed,
                _ => VoteRejection::Unverifiable,
            });
        }
        Ok(outcome.expect("checked above"))
    }

    /// [`Self::precheck`], [`recover`] and [`Self::add_checked`] in one (the
    /// recovery under the caller's borrow).
    pub fn add_vote(
        &mut self,
        vote: GovernanceVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> Result<(), VoteRejection> {
        let outcome = self.precheck(&vote, now)?;
        let checked = recover(vote, outcome, self.network)?;
        self.add_checked(checked, from, now)
    }

    /// Stores a recovered vote from `from` (`None`: relayed by the wallet
    /// itself) as its (outpoint, key)'s current funding vote, or queues it
    /// until its object arrives. Repeats the precheck: the store may have
    /// changed since.
    pub fn add_checked(
        &mut self,
        v: CheckedVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> Result<(), VoteRejection> {
        self.precheck(&v.vote, now)?;
        if self.key_ok(&v.vote.masternode_outpoint, &v.key_id)
            == Err(VoteRejection::UnknownVotingKey)
        {
            if self.is_current_proposal(&v.vote.parent_hash, now) {
                self.keep_suspect(v, from, now);
            }
            return Err(VoteRejection::UnknownVotingKey);
        }
        self.place(v, from, now, false)
    }

    /// Keeps a vote whose key the list may not hold yet: peers announce a
    /// vote once, so it is placed when a later list holds the key (D43).
    /// Per recovered key, so a forged copy cannot stand in for the real one.
    fn keep_suspect(&mut self, vote: CheckedVote, from: Option<SocketAddr>, at: i64) {
        let slot = (vote.vote.hash(), vote.key_id);
        let from_peer = from.map_or(0, |p| self.suspects_per_peer.get(&p).copied().unwrap_or(0));
        if self.suspects.contains_key(&slot) || from_peer >= MAX_SUSPECTS_PER_PEER {
            return;
        }
        while self.suspects.len() >= MAX_SUSPECT_VOTES {
            let Some(old) = self.suspect_order.pop_front() else {
                break;
            };
            self.remove_suspect(&old);
        }
        if let Some(p) = from {
            *self.suspects_per_peer.entry(p).or_default() += 1;
        }
        self.suspects.insert(slot, Orphan { vote, from, at });
        self.suspect_order.push_back(slot);
        // Entries placed or expired leave stale order slots; compact them.
        if self.suspect_order.len() > 2 * MAX_SUSPECT_VOTES {
            let suspects = &self.suspects;
            self.suspect_order.retain(|s| suspects.contains_key(s));
        }
    }

    fn remove_suspect(&mut self, slot: &([u8; 32], [u8; 20])) -> Option<Orphan> {
        let o = self.suspects.remove(slot)?;
        if let Some(p) = o.from
            && let Some(n) = self.suspects_per_peer.get_mut(&p)
        {
            *n -= 1;
            if *n == 0 {
                self.suspects_per_peer.remove(&p);
            }
        }
        Some(o)
    }

    /// Places the kept votes whose key the list now holds.
    fn place_suspects(&mut self) {
        let ready: Vec<_> = self
            .suspects
            .iter()
            .filter(|(_, o)| self.keys.contains_key(&o.vote.key_id))
            .map(|(slot, _)| *slot)
            .collect();
        for slot in ready {
            if let Some(o) = self.remove_suspect(&slot) {
                let _ = self.place(o.vote, o.from, o.at, false);
            }
        }
    }

    /// A funding vote the wallet relays for a caller (Core `voteraw`), with
    /// Core's checks in its order: the signature, the masternode (here its
    /// key: the list carries no collateral, D31), the time, then the
    /// object's (duplicate, expired, obsolete, same timestamp, the hourly
    /// update limit), then the per-key limit (D11).
    pub fn add_own_vote(&mut self, vote: GovernanceVote, now: i64) -> Result<(), VoteRejection> {
        let outcome = vote.vote_outcome().ok_or(VoteRejection::Malformed)?;
        if vote.signature.len() != COMPACT_SIG_SIZE {
            return Err(VoteRejection::BadSignature);
        }
        let checked = recover(vote, outcome, self.network)?;
        self.key_ok(&checked.vote.masternode_outpoint, &checked.key_id)?;
        if !self
            .resolved
            .contains_key(&checked.vote.masternode_outpoint)
            && !self.valid_keys.contains(&checked.key_id)
        {
            return Err(VoteRejection::Banned);
        }
        if checked.vote.time > now + MAX_TIME_FUTURE_DEVIATION {
            return Err(VoteRejection::TooNew);
        }
        self.place(checked, None, now, true)
    }

    fn place(
        &mut self,
        v: CheckedVote,
        from: Option<SocketAddr>,
        now: i64,
        rate_checked: bool,
    ) -> Result<(), VoteRejection> {
        let outcome = self.precheck(&v.vote, now)?;
        let (parent, outpoint, key) = (v.vote.parent_hash, v.vote.masternode_outpoint, v.key_id);
        // Core keeps the higher outcome of two votes of the same time.
        let replaces = |t: i64, o: VoteOutcome| match v.vote.time.cmp(&t) {
            std::cmp::Ordering::Less => Err(VoteRejection::Obsolete),
            std::cmp::Ordering::Equal if outcome.code() <= o.code() => {
                Err(VoteRejection::SameTimestamp)
            }
            _ => Ok(()),
        };
        let current = self
            .votes
            .get(&parent)
            .and_then(|m| m.get(&(outpoint, key)));
        if let Some(c) = current {
            replaces(c.time, c.outcome)?;
            // Core's last update is when the node accepted the current
            // vote; its own time stands in for it (D37).
            if rate_checked && c.time > now - VOTE_UPDATE_MIN_SECS {
                return Err(VoteRejection::TooOften);
            }
        }
        let waiting = self
            .orphan_slots
            .get(&(parent, outpoint, key))
            .and_then(|h| self.orphans.get(h));
        if let Some(w) = waiting {
            replaces(w.vote.vote.time, w.vote.outcome)?;
        }
        self.key_ok(&outpoint, &key)?;
        let resolved = self.resolved.contains_key(&outpoint);
        if current.is_none() && !resolved {
            let n = self.keys.get(&key).map_or(0, |k| k.count);
            let used = self.per_key.get(&(parent, key)).copied().unwrap_or(0);
            if used >= 2 * n + 2 {
                return Err(VoteRejection::KeyLimit);
            }
        }
        if !self.objects.contains_key(&parent) {
            return Err(self.queue_orphan(v, from, now));
        }
        self.remove_vote(&(parent, outpoint, key));
        self.votes.entry(parent).or_default().insert(
            (outpoint, key),
            CurrentVote {
                outpoint,
                outcome,
                time: v.vote.time,
                key_id: key,
                vote_hash: v.hash,
            },
        );
        if !resolved {
            *self.per_key.entry((parent, key)).or_default() += 1;
        }
        self.vote_count += 1;
        self.seen_votes.insert(v.hash);
        Ok(())
    }

    fn queue_orphan(
        &mut self,
        v: CheckedVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> VoteRejection {
        self.expire_orphans(now);
        if let Some(peer) = from
            && self.orphans_per_peer.get(&peer).copied().unwrap_or(0) >= MAX_ORPHANS_PER_PEER
        {
            return VoteRejection::Orphan;
        }
        let parent = v.vote.parent_hash;
        let slot = (parent, v.vote.masternode_outpoint, v.key_id);
        if let Some(older) = self.orphan_slots.get(&slot).copied() {
            self.remove_orphan(&older);
        }
        if self.orphans.len() >= MAX_ORPHAN_VOTES {
            self.evict_oldest_orphan();
        }
        if let Some(peer) = from {
            *self.orphans_per_peer.entry(peer).or_default() += 1;
        }
        let hash = v.hash;
        self.orphan_slots.insert(slot, hash);
        self.orphans_by_parent
            .entry(parent)
            .or_default()
            .insert(hash);
        self.orphan_order.push_back((now, hash));
        self.orphans.insert(
            hash,
            Orphan {
                vote: v,
                from,
                at: now,
            },
        );
        if self.orphan_order.len() > 2 * self.orphans.len() + 1024 {
            let order = std::mem::take(&mut self.orphan_order);
            self.orphan_order = order
                .into_iter()
                .filter(|(at, h)| self.live_entry(*at, h))
                .collect();
        }
        VoteRejection::Orphan
    }

    /// The highest superblock height ≤ `tip` paid by a confirmed trigger
    /// that lists `hash_hex` (node `getProposalFundedHeight` over Core's
    /// active triggers). A trigger at or below the tip is taken as
    /// executed: Core keeps those for a cycle (`CSuperblock::IsExpired`;
    /// MG-02 D17).
    pub fn funded_height(&self, hash_hex: &str, tip: u32) -> Option<u32> {
        let cycle = i64::from(governance_params(self.network).superblock_cycle);
        let tip = i64::from(tip);
        self.objects()
            .filter_map(|o| match &o.kind {
                ObjectKind::Trigger(t) => {
                    let h = i64::from(t.event_block_height);
                    (h >= 0
                        && h <= tip
                        && tip <= h + cycle
                        && t.proposal_hashes.iter().any(|x| x == hash_hex))
                    .then_some(h as u32)
                }
                ObjectKind::Proposal(_) => None,
            })
            .max()
    }

    /// The confirmed proposals, for the list.
    pub fn proposals(&self) -> impl Iterator<Item = (&StoredObject, &ProposalData)> {
        self.objects().filter_map(|o| match &o.kind {
            ObjectKind::Proposal(p) => Some((o, p)),
            ObjectKind::Trigger(_) => None,
        })
    }

    /// Hashes of the confirmed proposals whose end epoch is after `now`
    /// (the ones whose votes the sync downloads).
    pub fn current_proposals(&self, now: i64) -> Vec<[u8; 32]> {
        let mut out: Vec<[u8; 32]> = self
            .proposals()
            .filter(|(_, p)| p.end_epoch.is_none_or(|e| e > now))
            .map(|(o, _)| o.object.hash())
            .collect();
        out.sort();
        out
    }

    /// Whether `hash` is a confirmed proposal whose end epoch is after
    /// `now`.
    pub fn is_current_proposal(&self, hash: &[u8; 32], now: i64) -> bool {
        matches!(
            self.object(hash).map(|o| &o.kind),
            Some(ObjectKind::Proposal(p)) if p.end_epoch.is_none_or(|e| e > now)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vote::to_recoverable_compact;
    use dashcore::hashes::{Hash, hash160};
    use dashcore::secp256k1::{Message, PublicKey, SecretKey};

    const NOW: i64 = 1_000_000;

    fn peer(n: u16) -> Option<SocketAddr> {
        Some(SocketAddr::from(([127, 0, 0, 1], n)))
    }

    fn key_id(n: u8) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
        hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
    }

    fn outpoint(op: u8) -> OutPoint {
        OutPoint::new(dashcore::Txid::from_byte_array([op; 32]), 0)
    }

    fn sign(v: &mut GovernanceVote, key: u8) {
        let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
        let d = v.signing_digest(Network::Regtest);
        let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
            .unwrap()
            .to_vec();
    }

    /// A funding vote by voting key `key` for outpoint `(op, 0)`.
    fn vote(parent: [u8; 32], key: u8, op: u8, outcome: VoteOutcome, time: i64) -> GovernanceVote {
        let mut v = GovernanceVote::funding(outpoint(op), parent, outcome, time);
        sign(&mut v, key);
        v
    }

    fn proposal_ending(name: &str, end: i64) -> GovernanceObject {
        let data = format!(
            r#"{{"end_epoch":{end},"name":"{name}","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#
        );
        GovernanceObject::new_proposal(1, NOW, data.into_bytes())
    }

    fn proposal(name: &str) -> GovernanceObject {
        proposal_ending(name, NOW + 10_000)
    }

    fn trigger(height: i32) -> GovernanceObject {
        let data = format!(
            r#"{{"event_block_height":{height},"payment_addresses":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amounts":"1","proposal_hashes":"{HASH}","type":2}}"#
        );
        let mut t = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        t.object_type = crate::object::OBJECT_TYPE_TRIGGER;
        t
    }

    const HASH: &str = "4f6e2b3e7d8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b";

    fn entry(key: u8, weight: u32) -> ListEntry {
        ListEntry {
            voting_key_id: key_id(key),
            weight: Some(weight),
            valid: true,
        }
    }

    /// A store on regtest with one confirmation, `obj` stored and the list.
    fn store(obj: &GovernanceObject, list: &[ListEntry]) -> GovernanceStore {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_confirmations(1);
        s.set_masternode_list(list);
        assert_eq!(s.add_object(obj.clone(), peer(1), NOW), Ok(true));
        s
    }

    fn current(s: &GovernanceStore, h: &[u8; 32], op: u8) -> Vec<(VoteOutcome, [u8; 20])> {
        let mut v: Vec<_> = s
            .votes_of(h, &outpoint(op))
            .map(|c| (c.outcome, c.key_id))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn objects_count_after_two_announcers() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = GovernanceStore::new(Network::Regtest);
        assert_eq!(s.add_object(obj.clone(), peer(1), NOW), Ok(true));
        assert!(s.object(&h).is_none());
        assert_eq!(s.unconfirmed_count(), 1);
        assert!(s.current_proposals(NOW).is_empty());
        assert!(!s.note_announcer(&h, peer(1).unwrap()));
        assert!(s.note_announcer(&h, peer(2).unwrap()));
        assert!(s.object(&h).is_some());
        assert_eq!(s.current_proposals(NOW), vec![h]);
        assert!(s.is_current_proposal(&h, NOW));
        // The wallet's own objects count at once.
        let own = proposal("b");
        assert_eq!(s.add_object(own.clone(), None, NOW), Ok(true));
        assert!(s.object(&own.hash()).is_some());
        // Invalid data is refused.
        let bad = GovernanceObject::new_proposal(1, NOW, br#"{"type":1}"#.to_vec());
        assert!(s.add_object(bad, peer(1), NOW).is_err());
    }

    #[test]
    fn unconfirmed_objects_are_bounded_and_expire() {
        let mut s = GovernanceStore::new(Network::Regtest);
        for i in 0..MAX_UNCONFIRMED_PER_PEER {
            let o = proposal(&format!("p{i}"));
            assert_eq!(s.add_object(o, peer(1), NOW), Ok(true));
        }
        let one_more = proposal("extra");
        assert_eq!(
            s.add_object(one_more.clone(), peer(1), NOW),
            Err(ObjectRejection::StoreFull)
        );
        assert!(!ObjectRejection::StoreFull.is_invalid());
        // Another peer still has room; the wallet's own objects always do.
        assert_eq!(s.add_object(one_more, peer(2), NOW), Ok(true));
        assert_eq!(s.add_object(proposal("own"), None, NOW), Ok(true));
        // Unconfirmed ones go after the TTL; confirmed ones stay.
        let kept = proposal("p0").hash();
        assert!(s.note_announcer(&kept, peer(3).unwrap()));
        s.prune(NOW + UNCONFIRMED_TTL_SECS - 1);
        assert_eq!(s.unconfirmed_count(), MAX_UNCONFIRMED_PER_PEER);
        s.prune(NOW + UNCONFIRMED_TTL_SECS);
        assert_eq!(s.unconfirmed_count(), 0);
        assert_eq!(s.object_count(), 2);
        assert!(s.object(&kept).is_some());
        // Dropped unconfirmed objects can come again.
        assert!(!s.has_object(&proposal("p1").hash()));
    }

    #[test]
    fn expired_objects_are_erased_and_not_fetched_again() {
        let end = NOW + 100;
        let obj = proposal_ending("a", end);
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), None, NOW),
            Ok(())
        );
        // From its end epoch on, votes on it are ignored, not penalized.
        let late = vote(h, 1, 2, VoteOutcome::Yes, 11);
        assert_eq!(
            s.add_vote(late.clone(), peer(1), end),
            Err(VoteRejection::Ignored)
        );
        assert!(s.has_vote(&late.hash()));
        assert!(!s.is_current_proposal(&h, end));
        // Erased after Core's deletion delay, votes with it; the hash stays
        // known for the session.
        s.prune(end + DELETION_DELAY_SECS - 1);
        assert!(s.object(&h).is_some());
        s.prune(end + DELETION_DELAY_SECS);
        assert!(s.object(&h).is_none());
        assert_eq!(s.vote_count(), 0);
        assert!(s.has_object(&h));
        assert_eq!(
            s.add_object(obj, peer(2), end + DELETION_DELAY_SECS),
            Ok(false)
        );
        s.prune(i64::MAX / 2);
        assert!(s.has_object(&h));

        // A trigger goes a cycle after its height (regtest: 20 blocks), and
        // its hash two cycles after its creation.
        let t = trigger(100);
        let th = t.hash();
        let mut s = store(&t, &[]);
        s.set_tip(120);
        s.prune(NOW);
        assert!(s.object(&th).is_some());
        s.set_tip(121);
        s.prune(NOW);
        assert!(s.object(&th).is_none() && s.has_object(&th));
        let p = governance_params(Network::Regtest);
        let keep = 2 * i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs)
            + DELETION_DELAY_SECS;
        s.prune(NOW + keep);
        assert!(s.has_object(&th));
        s.prune(NOW + keep + 1);
        assert!(!s.has_object(&th));
    }

    #[test]
    fn orphans_wait_for_their_object_and_latest_vote_wins() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_confirmations(1);
        s.set_masternode_list(&[entry(1, 1)]);
        let v1 = vote(h, 1, 1, VoteOutcome::Yes, 10);
        assert_eq!(
            s.add_vote(v1.clone(), peer(1), NOW),
            Err(VoteRejection::Orphan)
        );
        assert_eq!(
            s.add_vote(v1.clone(), peer(1), NOW),
            Err(VoteRejection::Duplicate)
        );
        // An older vote for the same slot is obsolete while one waits.
        let v0 = vote(h, 1, 1, VoteOutcome::No, 5);
        assert_eq!(s.add_vote(v0, peer(1), NOW), Err(VoteRejection::Obsolete));
        assert_eq!(s.add_object(obj, peer(1), NOW), Ok(true));
        assert_eq!(s.orphan_count(), 0);
        assert_eq!(s.vote_count(), 1);
        assert_eq!(current(&s, &h, 1), vec![(VoteOutcome::Yes, key_id(1))]);
        let v2 = vote(h, 1, 1, VoteOutcome::No, 20);
        assert_eq!(s.add_vote(v2, peer(1), NOW), Ok(()));
        assert_eq!(current(&s, &h, 1), vec![(VoteOutcome::No, key_id(1))]);
        assert_eq!(s.vote_count(), 1);
        assert!(!s.has_vote(&v1.hash()));
    }

    #[test]
    fn own_votes_follow_cores_voteraw_checks_in_order() {
        let obj = proposal("a");
        let h = obj.hash();
        let banned = ListEntry {
            valid: false,
            ..entry(2, 1)
        };
        let mut s = store(&obj, &[entry(1, 1), banned]);
        let mut unsigned = vote(h, 1, 1, VoteOutcome::Yes, NOW);
        unsigned.signature[1] ^= 1;
        assert!(matches!(
            s.add_own_vote(unsigned, NOW),
            Err(VoteRejection::BadSignature | VoteRejection::UnknownVotingKey)
        ));
        assert_eq!(
            s.add_own_vote(vote(h, 3, 3, VoteOutcome::Yes, NOW), NOW),
            Err(VoteRejection::UnknownVotingKey)
        );
        // The banned masternode is reported before the time (Core finds the
        // masternode before `IsValid`).
        assert_eq!(
            s.add_own_vote(vote(h, 2, 2, VoteOutcome::Yes, NOW + 7200), NOW),
            Err(VoteRejection::Banned)
        );
        assert_eq!(
            s.add_own_vote(vote(h, 1, 1, VoteOutcome::Yes, NOW + 7200), NOW),
            Err(VoteRejection::TooNew)
        );
        let first = vote(h, 1, 1, VoteOutcome::Yes, NOW - 4000);
        assert_eq!(s.add_own_vote(first.clone(), NOW), Ok(()));
        assert_eq!(s.add_own_vote(first, NOW), Err(VoteRejection::Duplicate));
        assert_eq!(
            s.add_own_vote(vote(h, 1, 1, VoteOutcome::No, NOW - 5000), NOW),
            Err(VoteRejection::Obsolete)
        );
        // A peer's vote of the same time and a lower outcome: rejected.
        let peers = vote(h, 1, 1, VoteOutcome::Abstain, NOW - 100);
        assert_eq!(s.add_vote(peers, peer(1), NOW), Ok(()));
        assert_eq!(
            s.add_own_vote(vote(h, 1, 1, VoteOutcome::No, NOW - 100), NOW),
            Err(VoteRejection::SameTimestamp)
        );
        // Newer, but the current vote is less than an hour old.
        assert_eq!(
            s.add_own_vote(vote(h, 1, 1, VoteOutcome::No, NOW), NOW),
            Err(VoteRejection::TooOften)
        );
        assert_eq!(
            s.add_own_vote(vote(h, 1, 1, VoteOutcome::No, NOW + 3500), NOW + 3500),
            Ok(())
        );
        assert_eq!(current(&s, &h, 1), vec![(VoteOutcome::No, key_id(1))]);
    }

    #[test]
    fn same_time_votes_keep_the_higher_outcome() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::No, 10), None, NOW),
            Ok(())
        );
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), None, NOW),
            Err(VoteRejection::SameTimestamp)
        );
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Abstain, 10), None, NOW),
            Ok(())
        );
    }

    #[test]
    fn a_key_holder_cannot_replace_another_masternodes_vote() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(2, 1)]);
        // The evonode of key 1 votes yes for its outpoint 1; the masternode
        // of key 2 votes yes for its outpoint 2.
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), peer(1), NOW),
            Ok(())
        );
        assert_eq!(
            s.add_vote(vote(h, 2, 2, VoteOutcome::Yes, 10), peer(2), NOW),
            Ok(())
        );
        // Key 2 signs a newer no for outpoint 1: a second claim, not a
        // replacement.
        assert_eq!(
            s.add_vote(vote(h, 2, 1, VoteOutcome::No, NOW + 3600), peer(2), NOW),
            Ok(())
        );
        assert_eq!(
            current(&s, &h, 1),
            vec![(VoteOutcome::Yes, key_id(1)), (VoteOutcome::No, key_id(2))]
        );
        // The evonode's own later votes still land.
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Abstain, 20), peer(1), NOW),
            Ok(())
        );
        assert_eq!(
            current(&s, &h, 1),
            vec![
                (VoteOutcome::No, key_id(2)),
                (VoteOutcome::Abstain, key_id(1))
            ]
        );
        // The tally ranges over the contested outpoint. Key 1 claims one
        // outpoint with one masternode and carries the estimate; key 2
        // claims two with one, so each of its votes weighs 0..1 and only the
        // newest uncontested one is estimated.
        let t = crate::tally::ranged_tally(
            s.votes_on(&h),
            |k| s.key_weights(k),
            |op| s.resolved(op).map(|r| r.weight),
        );
        assert_eq!(t.voters, 2);
        assert_eq!(
            (t.abstain.min, t.abstain.max, t.abstain.estimate),
            (0, 4, 4)
        );
        assert_eq!((t.no.min, t.no.max, t.no.estimate), (0, 1, 0));
        assert_eq!((t.yes.min, t.yes.max, t.yes.estimate), (0, 1, 1));
    }

    #[test]
    fn a_vote_waits_for_a_list_holding_its_key() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        let late = vote(h, 3, 3, VoteOutcome::No, 10);
        let r = s.add_vote(late.clone(), None, NOW);
        assert_eq!(r, Err(VoteRejection::UnknownVotingKey));
        assert_eq!(s.vote_count(), 0);
        // The list catching up places it: peers announce a vote once (D43).
        s.set_masternode_list(&[entry(1, 1), entry(3, 1)]);
        assert_eq!(s.vote_count(), 1);
        assert!(s.has_vote(&late.hash()));
        // Only votes on a current proposal wait: an unknown parent adds
        // nothing to keep.
        let stray = vote([9; 32], 5, 5, VoteOutcome::No, 10);
        let _ = s.add_vote(stray, None, NOW);
        assert!(s.suspects.is_empty());
        // One peer fills at most its share, so it cannot evict others'.
        for t in 0..=MAX_SUSPECTS_PER_PEER {
            let _ = s.add_vote(vote(h, 6, 6, VoteOutcome::No, i64::from(t)), peer(1), NOW);
        }
        assert_eq!(s.suspects.len(), MAX_SUSPECTS_PER_PEER as usize);
        let last = i64::from(MAX_SUSPECTS_PER_PEER) + 1;
        let _ = s.add_vote(vote(h, 6, 6, VoteOutcome::No, last), peer(2), NOW);
        assert_eq!(s.suspects.len(), MAX_SUSPECTS_PER_PEER as usize + 1);
        s.prune(NOW + SUSPECT_TTL_SECS);
        assert!(s.suspects.is_empty() && s.suspects_per_peer.is_empty());
        // Kept votes expire.
        let other = vote(h, 4, 4, VoteOutcome::No, 10);
        let _ = s.add_vote(other.clone(), None, NOW);
        s.prune(NOW + SUSPECT_TTL_SECS);
        s.set_masternode_list(&[entry(1, 1), entry(3, 1), entry(4, 1)]);
        assert!(!s.has_vote(&other.hash()));
    }

    #[test]
    fn keys_come_from_the_whole_list() {
        let obj = proposal("a");
        let h = obj.hash();
        let banned = ListEntry {
            valid: false,
            ..entry(2, 4)
        };
        let mut s = store(&obj, &[entry(1, 1), banned]);
        assert_eq!(s.valid_weight(), 1);
        assert_eq!(s.list_size(), 2);
        // A banned masternode's key still votes (Core GetMNByCollateral).
        assert_eq!(
            s.add_vote(vote(h, 2, 2, VoteOutcome::Yes, 10), None, NOW),
            Ok(())
        );
        // A key outside the list does not; that is suspect, not invalid.
        let r = s.add_vote(vote(h, 3, 3, VoteOutcome::Yes, 10), None, NOW);
        assert_eq!(r, Err(VoteRejection::UnknownVotingKey));
        assert!(r.unwrap_err().is_suspect() && !r.unwrap_err().is_invalid());
        // Compact signatures that recover nothing are invalid and left
        // unseen.
        let mut junk = vote(h, 1, 1, VoteOutcome::Yes, 10);
        junk.signature = vec![0; COMPACT_SIG_SIZE];
        assert_eq!(
            s.add_vote(junk.clone(), None, NOW),
            Err(VoteRejection::BadSignature)
        );
        assert!(!s.has_vote(&junk.hash()));
        // Too far ahead.
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::No, NOW + 3601), None, NOW),
            Err(VoteRejection::TooNew)
        );
        // Key 2 leaves the list: its vote goes and can be fetched again.
        let kept = vote(h, 1, 1, VoteOutcome::Yes, 10);
        assert_eq!(s.add_vote(kept, None, NOW), Ok(()));
        s.set_masternode_list(&[entry(1, 1)]);
        assert_eq!(s.vote_count(), 1);
        let v2 = vote(h, 2, 2, VoteOutcome::Yes, 10);
        assert!(!s.has_vote(&v2.hash()));
    }

    #[test]
    fn votes_core_relays_but_the_wallet_does_not_tally_are_ignored() {
        let obj = proposal("a");
        let h = obj.hash();
        let t = trigger(100);
        let mut s = store(&obj, &[entry(1, 1)]);
        assert_eq!(s.add_object(t.clone(), peer(1), NOW), Ok(true));
        let ignored = |s: &mut GovernanceStore, v: GovernanceVote| {
            assert_eq!(
                s.add_vote(v.clone(), peer(1), NOW),
                Err(VoteRejection::Ignored)
            );
            assert!(s.has_vote(&v.hash()), "remembered");
            assert!(!VoteRejection::Ignored.is_invalid());
        };
        // On a trigger (signed by the operator key in Core).
        let mut on_trigger = vote(t.hash(), 1, 1, VoteOutcome::Yes, 10);
        on_trigger.signature = vec![7; BLS_SIG_SIZE];
        ignored(&mut s, on_trigger);
        // Other signals and the NONE outcome.
        for (signal, outcome) in [(2, 1), (3, 1), (4, 2), (0, 1), (1, 0)] {
            let mut v = vote(h, 1, 1, VoteOutcome::Yes, 10);
            v.signal = signal;
            v.outcome = outcome;
            sign(&mut v, 1);
            ignored(&mut s, v);
        }
        // Outside Core's ranges, or a signature of neither size.
        for (signal, outcome, sig) in [(5, 1, 65), (1, 4, 65), (-1, 1, 65), (1, 1, 64)] {
            let mut v = vote(h, 1, 1, VoteOutcome::Yes, 11);
            v.signal = signal;
            v.outcome = outcome;
            v.signature.resize(sig, 0);
            let r = s.add_vote(v, peer(1), NOW);
            assert_eq!(r, Err(VoteRejection::Malformed));
            assert!(r.unwrap_err().is_invalid());
        }
        // A BLS signature: unverifiable for an unknown object (dropped, not
        // remembered), invalid on a proposal.
        let mut bls = vote([9; 32], 1, 1, VoteOutcome::Yes, 12);
        bls.signature = vec![7; BLS_SIG_SIZE];
        assert_eq!(
            s.add_vote(bls.clone(), peer(1), NOW),
            Err(VoteRejection::Unverifiable)
        );
        assert!(!s.has_vote(&bls.hash()));
        bls.parent_hash = h;
        assert_eq!(s.add_vote(bls, peer(1), NOW), Err(VoteRejection::Malformed));
        assert_eq!(s.vote_count(), 0);
    }

    #[test]
    fn resolved_outpoints_need_their_own_key_and_are_not_limited() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1), entry(2, 1)]);
        let v = vote(h, 2, 1, VoteOutcome::Yes, 10);
        s.set_resolved(HashMap::from([(
            v.masternode_outpoint,
            ResolvedMasternode {
                voting_key_id: key_id(1),
                weight: 1,
            },
        )]));
        let r = s.add_vote(v, None, NOW);
        assert_eq!(r, Err(VoteRejection::WrongVotingKey));
        assert!(r.unwrap_err().is_suspect());
        // Key 1 (one masternode) fills its 2·1 + 2 unresolved outpoints...
        for op in 2..=5 {
            assert_eq!(
                s.add_vote(vote(h, 1, op, VoteOutcome::Yes, 10), None, NOW),
                Ok(())
            );
        }
        assert_eq!(
            s.add_vote(vote(h, 1, 6, VoteOutcome::Yes, 10), None, NOW),
            Err(VoteRejection::KeyLimit)
        );
        // ...and its resolved masternode's own vote still counts.
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), None, NOW),
            Ok(())
        );
    }

    #[test]
    fn one_key_votes_for_at_most_2n_plus_2_outpoints() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        for op in 1..=4 {
            assert_eq!(
                s.add_vote(vote(h, 1, op, VoteOutcome::Yes, 10), None, NOW),
                Ok(())
            );
        }
        assert_eq!(
            s.add_vote(vote(h, 1, 5, VoteOutcome::Yes, 10), None, NOW),
            Err(VoteRejection::KeyLimit)
        );
        // A newer vote of an outpoint already counted is not limited.
        assert_eq!(
            s.add_vote(vote(h, 1, 4, VoteOutcome::No, 11), None, NOW),
            Ok(())
        );
    }

    #[test]
    fn the_trigger_time_window_applies_once_synced() {
        // Regtest cycles are 20 blocks of 150 s; two cycles is 6000 s.
        let mut old = trigger(1520);
        old.time = NOW - 6001;
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_confirmations(1);
        assert_eq!(s.add_object(old.clone(), peer(1), NOW), Ok(true));
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_confirmations(1);
        s.set_synced();
        assert_eq!(
            s.add_object(old, peer(1), NOW),
            Err(ObjectRejection::TriggerTimeOutOfRange)
        );
    }

    #[test]
    fn triggers_give_funded_heights() {
        let t = trigger(1520);
        let s = store(&t, &[entry(1, 1)]);
        assert_eq!(s.funded_height(HASH, 1519), None);
        assert_eq!(s.funded_height(HASH, 1520), Some(1520));
        // Kept for one cycle (regtest 20 blocks) after its height.
        assert_eq!(s.funded_height(HASH, 1540), Some(1520));
        assert_eq!(s.funded_height(HASH, 1541), None);
        // A negative height never comes due.
        let s = store(&trigger(-1), &[]);
        assert_eq!(s.funded_height(HASH, 0), None);
    }

    fn parent(i: u32) -> [u8; 32] {
        let mut p = [0u8; 32];
        p[..4].copy_from_slice(&i.to_le_bytes());
        p
    }

    #[test]
    fn orphans_expire_are_bounded_per_peer_and_evict_the_oldest() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_masternode_list(&[entry(1, 1)]);
        let first = vote(parent(0), 1, 1, VoteOutcome::Yes, 10);
        assert_eq!(
            s.add_vote(first.clone(), peer(1), NOW),
            Err(VoteRejection::Orphan)
        );
        // Expiry after 10 minutes.
        s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS);
        assert!(s.has_vote(&first.hash()));
        s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS + 1);
        assert!(!s.has_vote(&first.hash()));

        // Per-peer bound.
        for i in 0..MAX_ORPHANS_PER_PEER + 5 {
            let _ = s.add_vote(vote(parent(i), 1, 1, VoteOutcome::Yes, 10), peer(1), NOW);
        }
        assert_eq!(s.orphan_count(), MAX_ORPHANS_PER_PEER as usize);
        // Another peer still gets in.
        let other = vote(parent(u32::MAX), 1, 1, VoteOutcome::Yes, 10);
        assert_eq!(
            s.add_vote(other.clone(), peer(2), NOW),
            Err(VoteRejection::Orphan)
        );
        assert!(s.has_vote(&other.hash()));
    }

    fn filler(i: u32) -> CheckedVote {
        let vote = GovernanceVote::funding(OutPoint::null(), parent(i), VoteOutcome::Yes, 10);
        CheckedVote {
            hash: vote.hash(),
            vote,
            outcome: VoteOutcome::Yes,
            key_id: [0; 20],
        }
    }

    #[test]
    fn a_full_orphan_queue_evicts_the_oldest() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_masternode_list(&[entry(1, 1)]);
        // The wallet's own relays have no per-peer bound; fill with them
        // (unsigned: the queue itself checks nothing).
        let oldest = vote(parent(0), 1, 1, VoteOutcome::Yes, 10);
        let _ = s.add_vote(oldest.clone(), None, NOW);
        for i in 1..MAX_ORPHAN_VOTES as u32 {
            s.queue_orphan(filler(i), None, NOW);
        }
        assert_eq!(s.orphan_count(), MAX_ORPHAN_VOTES);
        let newest = vote(parent(MAX_ORPHAN_VOTES as u32), 1, 1, VoteOutcome::Yes, 10);
        let _ = s.add_vote(newest.clone(), None, NOW);
        assert_eq!(s.orphan_count(), MAX_ORPHAN_VOTES);
        assert!(!s.has_vote(&oldest.hash()));
        assert!(s.has_vote(&newest.hash()));
    }

    #[test]
    fn replaced_orphans_neither_grow_the_queue_nor_block_expiry() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_masternode_list(&[entry(1, 1)]);
        // One orphan stays at the front while another slot is replaced by
        // ever newer votes.
        let front = vote(parent(0), 1, 1, VoteOutcome::Yes, 10);
        let _ = s.add_vote(front.clone(), peer(1), NOW);
        for t in 0..10_000 {
            let r = s.add_vote(
                vote(parent(1), 1, 1, VoteOutcome::Yes, 11 + t),
                peer(2),
                NOW,
            );
            assert_eq!(r, Err(VoteRejection::Orphan));
        }
        assert_eq!(s.orphan_count(), 2);
        assert!(s.orphan_order.len() <= 2 * 2 + 1024 + 1);
        // A hash queued again after it left leaves a stale entry ahead of
        // the live one; expiry skips it.
        let again = filler(7);
        s.queue_orphan(again.clone(), None, NOW);
        s.remove_orphan(&again.hash);
        s.queue_orphan(again.clone(), None, NOW + 500);
        s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS + 1);
        assert!(!s.has_vote(&front.hash()));
        assert!(s.has_vote(&again.hash));
        s.expire_orphans(NOW + 500 + ORPHAN_EXPIRY_SECS + 1);
        assert_eq!(s.orphan_count(), 0);
    }
}
