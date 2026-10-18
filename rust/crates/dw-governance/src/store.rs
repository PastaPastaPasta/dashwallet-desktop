//! The synced governance objects and current votes, in memory (MG-02 §3,
//! §4.1).
//!
//! Objects arrive checked ([`crate::validate`]). One counts (is listed,
//! tallied, used for funded heights) once enough distinct peers announced
//! it, two by default (MG-02 D3): the wallet cannot see the collateral or a
//! trigger's operator signature, which each relaying full node checked.
//! Objects waiting for confirmation are bounded per first sender and in all,
//! by count and bytes, and dropped after [`UNCONFIRMED_TTL_SECS`]. The
//! wallet keeps no lapsed proposal (MG-02 D65): one leaves at its end epoch
//! and one that arrives lapsed is refused, both remembered among the last
//! [`MAX_EXPIRED_PROPOSALS`] so they are not fetched again (Core keeps a
//! lapsed proposal until it is voted deleted). A trigger leaves as Core's
//! `UpdateCachesAndClean` erases it, a superblock cycle after its height
//! (one paid is kept that long, `CSuperblock::IsExpired`; D17), and its hash
//! is kept, among the last [`MAX_ERASED_OBJECTS`], until two cycles after
//! its creation (`mapErasedGovernanceObjects`).
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

use dashcore::hashes::{Hash, HashEngine, sha256};
use dashcore::{Network, OutPoint};

use crate::object::{GovernanceObject, display_hex};
use crate::params::EVONODE_VOTE_WEIGHT;
use crate::params::governance_params;
use crate::proposal::{ProposalData, TriggerData};
use crate::tally::{CurrentVote, KeyWeights, RangedTally, ranged_tally};
use crate::validate::{MAX_TIME_FUTURE_DEVIATION, ObjectRejection, ValidObject, check_object};
use crate::vote::{
    BLS_SIG_SIZE, COMPACT_SIG_SIZE, GovernanceVote, SIGNAL_FUNDING, VoteOutcome, canonical_compact,
};

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
/// Of them, the data bytes of objects one peer sent first (MG-02 D53).
pub const MAX_UNCONFIRMED_BYTES_PER_PEER: usize = 8 << 20;
/// Objects kept in all, confirmed or not, and their data bytes: past
/// either, objects no listed key voted on make room, those not announced
/// in the current run first, then the oldest (MG-02 D47).
pub const MAX_OBJECTS: usize = 5_000;
pub const MAX_OBJECT_BYTES: usize = 64 << 20;
/// How long an object waits for its confirmations.
pub const UNCONFIRMED_TTL_SECS: i64 = 60 * 60;
/// Core's `GOVERNANCE_UPDATE_MIN`: a masternode's vote on an object is not
/// replaced within the hour.
pub const VOTE_UPDATE_MIN_SECS: i64 = 60 * 60;
/// Hashes of ignored votes remembered, oldest forgotten first.
pub const MAX_IGNORED_VOTES: usize = 100_000;
/// Copies of votes (hash and canonical signature) recovered and then not
/// kept, with why, oldest forgotten first (MG-02 D62, D63).
pub const MAX_DISMISSED_VOTES: usize = 100_000;
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
/// Hashes of erased triggers remembered, oldest forgotten first (MG-02
/// D65).
pub const MAX_ERASED_OBJECTS: usize = 10_000;
/// Hashes of lapsed proposals remembered, oldest forgotten first (MG-02
/// D65): a forgotten one is refused again if it comes back.
pub const MAX_EXPIRED_PROPOSALS: usize = 10_000;
/// Hashes of votes the per-key cap refused (MG-02 D71) or lost (D76) kept
/// per proposal: past it, a key's refused votes count as many as it has
/// masternodes, and a lost vote's outcome as the whole list.
pub const MAX_REFUSED_PER_PROPOSAL: usize = 1_000;
/// Lost votes kept for objects the store does not hold, oldest forgotten
/// first (D76): the object's own vote sync asks for its votes when it
/// arrives.
pub const MAX_UNSTORED_LOST: usize = 10_000;
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
    /// The last [`GovernanceStore::begin_run`] it was announced or sent in.
    pub seen_in_run: u64,
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
    /// An older vote than the (outpoint, key)'s current one.
    Obsolete,
    /// Of the same time as the current one with a lower outcome (Core keeps
    /// the higher one).
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
    /// The masternode the outpoint is the collateral of.
    pub pro_tx_hash: [u8; 32],
    pub voting_key_id: [u8; 20],
    pub weight: u32,
}

/// Votes on one proposal the store holds no countable copy of, widened
/// into its tally (D71, D72).
#[derive(Debug, Default)]
struct Uncounted {
    /// (key, outcome) → votes the per-key cap refused.
    refused: HashMap<([u8; 20], VoteOutcome), Refused>,
    /// Hashes held in `refused` and `lost`.
    held: usize,
    /// Outpoint → wrong-key copies dropped while their honest copy is
    /// fetched again.
    awaited: HashMap<OutPoint, Vec<Awaited>>,
    /// Outpoint → removed copies whose hash may be a vote Core counts,
    /// fetched again (D76).
    lost: HashMap<OutPoint, Vec<Lost>>,
    /// Outcomes of lost votes past the bound, no longer held (D76).
    lost_past: HashSet<VoteOutcome>,
}

impl Uncounted {
    fn is_empty(&self) -> bool {
        self.refused.is_empty()
            && self.awaited.is_empty()
            && self.lost.is_empty()
            && self.lost_past.is_empty()
    }

    /// Drops the lost vote of `hash` at `outpoint`; whether it was held.
    fn settle_lost(&mut self, outpoint: &OutPoint, hash: &[u8; 32]) -> bool {
        let Some(list) = self.lost.get_mut(outpoint) else {
            return false;
        };
        let before = list.len();
        list.retain(|l| &l.hash != hash);
        let settled = list.len() < before;
        if list.is_empty() {
            self.lost.remove(outpoint);
        }
        if settled {
            self.held -= 1;
        }
        settled
    }

    /// Awaits `a` at `outpoint`, once per hash.
    fn await_copy(&mut self, outpoint: OutPoint, a: Awaited) {
        let list = self.awaited.entry(outpoint).or_default();
        if list.iter().all(|b| b.hash != a.hash) {
            list.push(a);
        }
    }
}

/// A key's refused votes on one proposal: their hashes, until more than
/// the key has masternodes were refused, which no fetch can undo.
#[derive(Debug, Default)]
struct Refused {
    hashes: HashSet<[u8; 32]>,
    saturated: bool,
}

/// A dropped wrong-key copy: the vote of its hash, honest copy unseen, by
/// the outpoint's masternode `key`.
#[derive(Debug)]
struct Awaited {
    hash: [u8; 32],
    outcome: VoteOutcome,
    time: i64,
    key: [u8; 20],
}

/// A removed unresolved copy (or orphan): its hash may be a masternode's
/// vote that Core counts, whose other copies the wallet skipped as known.
#[derive(Debug)]
struct Lost {
    hash: [u8; 32],
    outcome: VoteOutcome,
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
    /// How many of `objects` are unconfirmed, kept as they change: read on
    /// every sync event (D60).
    unconfirmed: usize,
    /// Erased triggers → until when their hash is kept.
    erased: HashMap<[u8; 32], i64>,
    erased_order: VecDeque<[u8; 32]>,
    /// Votes whose stored copy a resolved outpoint showed signed by another
    /// key: the honest copy, of the same hash, is asked for again (D64).
    refetch: Vec<[u8; 32]>,
    /// Object → its votes held as uncertainty (D71, D72, D76).
    uncounted: HashMap<[u8; 32], Uncounted>,
    /// (object, hash) of lost votes noted while the object was not stored,
    /// oldest first; entries gone stale are skipped (D76).
    unstored_lost: VecDeque<([u8; 32], [u8; 32])>,
    /// Lapsed proposals, stored or refused.
    expired: HashMap<[u8; 32], ()>,
    expired_order: VecDeque<[u8; 32]>,
    /// object hash → (outpoint, key) → current funding vote.
    votes: HashMap<[u8; 32], ObjectVotes>,
    /// (object, key) → unresolved outpoints voting with the key.
    per_key: HashMap<([u8; 32], [u8; 20]), u32>,
    /// Stored vote → its object.
    seen_votes: HashMap<[u8; 32], [u8; 32]>,
    vote_count: u64,
    /// Ignored vote → its object.
    ignored: HashMap<[u8; 32], [u8; 32]>,
    ignored_order: VecDeque<[u8; 32]>,
    refused: HashMap<[u8; 32], ()>,
    refused_order: VecDeque<[u8; 32]>,
    /// [`copy_digest`] of a vote recovered and not kept → why: signatures of
    /// one canonical form recover the same key and get the same answer, so
    /// they are not recovered again while remembered (D62, D63).
    dismissed: HashMap<[u8; 32], VoteRejection>,
    dismissed_order: VecDeque<[u8; 32]>,
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
    /// [`MAX_OBJECTS`], lower in tests.
    max_objects: usize,
    /// [`MAX_ERASED_OBJECTS`] and [`MAX_EXPIRED_PROPOSALS`], lower in
    /// tests.
    max_erased: usize,
    max_expired: usize,
    /// [`MAX_REFUSED_PER_PROPOSAL`] and [`MAX_UNSTORED_LOST`], lower in
    /// tests.
    max_refused: usize,
    max_unstored_lost: usize,
    /// The current sync run, and how many objects announced in it the cap
    /// evicted or refused (D47).
    run: u64,
    overflow: u32,
}

/// A vote copy's identity: its hash leaves the signature out (D62); the
/// signature enters in its canonical form, so those that recover the same
/// key are one (D63).
fn copy_digest(hash: &[u8; 32], signature: &[u8]) -> [u8; 32] {
    let mut engine = sha256::Hash::engine();
    engine.input(hash);
    match canonical_compact(signature) {
        Some(c) => engine.input(&c),
        None => engine.input(signature),
    }
    sha256::Hash::from_engine(engine).to_byte_array()
}

/// Whether `key` may sign for `outpoint`: its resolved masternode's key, or
/// any key of the list if unresolved.
fn check_key(
    resolved: &HashMap<OutPoint, ResolvedMasternode>,
    keys: &HashMap<[u8; 20], KeyWeights>,
    outpoint: &OutPoint,
    key: &[u8; 20],
) -> Result<(), VoteRejection> {
    match resolved.get(outpoint) {
        Some(r) if &r.voting_key_id == key => Ok(()),
        Some(_) => Err(VoteRejection::WrongVotingKey),
        None if keys.contains_key(key) => Ok(()),
        None => Err(VoteRejection::UnknownVotingKey),
    }
}

/// Adds `hash` to a bounded map, forgetting the oldest past `max`.
fn remember<V>(
    map: &mut HashMap<[u8; 32], V>,
    order: &mut VecDeque<[u8; 32]>,
    max: usize,
    hash: [u8; 32],
    value: V,
) {
    if map.contains_key(&hash) {
        return;
    }
    map.insert(hash, value);
    order.push_back(hash);
    if order.len() > max
        && let Some(old) = order.pop_front()
    {
        map.remove(&old);
    }
}

impl GovernanceStore {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            confirmations: DEFAULT_CONFIRMATIONS,
            tip: None,
            objects: HashMap::new(),
            unconfirmed: 0,
            erased: HashMap::new(),
            erased_order: VecDeque::new(),
            refetch: Vec::new(),
            uncounted: HashMap::new(),
            unstored_lost: VecDeque::new(),
            expired: HashMap::new(),
            expired_order: VecDeque::new(),
            votes: HashMap::new(),
            per_key: HashMap::new(),
            seen_votes: HashMap::new(),
            vote_count: 0,
            ignored: HashMap::new(),
            ignored_order: VecDeque::new(),
            refused: HashMap::new(),
            refused_order: VecDeque::new(),
            dismissed: HashMap::new(),
            dismissed_order: VecDeque::new(),
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
            max_objects: MAX_OBJECTS,
            max_erased: MAX_ERASED_OBJECTS,
            max_expired: MAX_EXPIRED_PROPOSALS,
            max_refused: MAX_REFUSED_PER_PROPOSAL,
            max_unstored_lost: MAX_UNSTORED_LOST,
            run: 0,
            overflow: 0,
        }
    }

    /// Starts a sync run's object phase: objects not announced since are
    /// the first the cap evicts, and evicting or refusing one that was is
    /// overflow. A run without one keeps the last one's objects current.
    pub fn begin_run(&mut self) {
        self.run += 1;
        self.overflow = 0;
    }

    /// Objects announced in this run that the cap evicted or refused since
    /// the last call (D47).
    pub fn take_overflow(&mut self) -> u32 {
        std::mem::take(&mut self.overflow)
    }

    #[cfg(test)]
    pub(crate) fn set_max_objects(&mut self, max: usize) {
        self.max_objects = max;
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
            if !o.confirmed && o.announcers.len() >= n {
                o.confirmed = true;
                self.unconfirmed -= 1;
            }
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
        debug_assert_eq!(
            self.unconfirmed,
            self.objects.values().filter(|o| !o.confirmed).count()
        );
        self.unconfirmed
    }

    /// Current votes stored, all objects.
    pub fn vote_count(&self) -> u64 {
        self.vote_count
    }

    pub fn orphan_count(&self) -> usize {
        self.orphans.len()
    }

    /// Whether `hash` is stored, was erased, lapsed or was refused (either
    /// way it is not fetched).
    pub fn has_object(&self, hash: &[u8; 32]) -> bool {
        self.objects.contains_key(hash)
            || self.erased.contains_key(hash)
            || self.expired.contains_key(hash)
            || self.refused.contains_key(hash)
    }

    /// Remembers a lapsed proposal, dropped or refused (D65).
    fn note_expired(&mut self, hash: [u8; 32]) {
        remember(
            &mut self.expired,
            &mut self.expired_order,
            self.max_expired,
            hash,
            (),
        );
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
                (),
            );
        }
    }

    /// Whether the vote `hash` is counted, waits for its object, or was
    /// ignored.
    pub fn has_vote(&self, hash: &[u8; 32]) -> bool {
        self.seen_votes.contains_key(hash)
            || self.orphans.contains_key(hash)
            || self.ignored.contains_key(hash)
    }

    /// The object of a vote [`Self::has_vote`] holds.
    pub fn vote_parent(&self, hash: &[u8; 32]) -> Option<[u8; 32]> {
        self.seen_votes
            .get(hash)
            .or_else(|| self.ignored.get(hash))
            .copied()
            .or_else(|| self.orphans.get(hash).map(|o| o.vote.vote.parent_hash))
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

    /// The unconfirmed objects announced or sent since the last object
    /// phase began, taking each first sender's in turn (D53).
    pub fn unconfirmed_this_run(&self) -> Vec<&StoredObject> {
        let mut by: HashMap<Option<SocketAddr>, Vec<&StoredObject>> = HashMap::new();
        for o in self.unconfirmed().filter(|o| o.seen_in_run == self.run) {
            by.entry(o.announcers.first().copied()).or_default().push(o);
        }
        let mut queues: Vec<_> = by.into_values().collect();
        for q in &mut queues {
            q.sort_unstable_by(|a, b| (b.stored_at, &b.hash_hex).cmp(&(a.stored_at, &a.hash_hex)));
        }
        // Queues by their oldest object, so each round goes oldest first.
        queues.sort_unstable_by(|a, b| {
            let head = |q: &Vec<&StoredObject>| q.last().map(|o| (o.stored_at, o.hash_hex.clone()));
            head(a).cmp(&head(b))
        });
        let mut out = Vec::new();
        while !queues.is_empty() {
            queues.retain_mut(|q| match q.pop() {
                Some(o) => {
                    out.push(o);
                    true
                }
                None => false,
            });
        }
        out
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
        o.seen_in_run = self.run;
        if o.confirmed || o.announcers.contains(&peer) {
            return false;
        }
        o.announcers.push(peer);
        o.confirmed = o.announcers.len() >= n;
        if o.confirmed {
            self.unconfirmed -= 1;
        }
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
        self.add_object_announced(object, from, &[], now)
    }

    /// [`Self::add_object`], with the other peers that announced it
    /// counting towards its confirmation at once: an object enough peers
    /// announced is stored confirmed, outside the unconfirmed bounds.
    pub fn add_object_announced(
        &mut self,
        object: GovernanceObject,
        from: Option<SocketAddr>,
        also: &[SocketAddr],
        now: i64,
    ) -> Result<bool, ObjectRejection> {
        let hash = object.hash();
        if self.has_object(&hash) {
            if let Some(peer) = from {
                self.note_announcer(&hash, peer);
            }
            return Ok(false);
        }
        let kind = match check_object(&object, self.network, now, self.synced) {
            Ok(ValidObject::Proposal(p)) => ObjectKind::Proposal(p),
            Ok(ValidObject::Trigger(t)) => ObjectKind::Trigger(t),
            Err(e) => {
                match e {
                    ObjectRejection::ProposalExpired => self.note_expired(hash),
                    e if e.is_final() => self.refuse_object(hash),
                    _ => {}
                }
                return Err(e);
            }
        };
        let mut announcers: Vec<SocketAddr> = Vec::new();
        for peer in from.iter().chain(also) {
            if announcers.len() < self.confirmations && !announcers.contains(peer) {
                announcers.push(*peer);
            }
        }
        let confirmed = from.is_none() || announcers.len() >= self.confirmations;
        if !confirmed {
            self.room_for_unconfirmed(from, object.data.len())?;
        }
        self.make_room(object.data.len())?;
        self.unconfirmed += usize::from(!confirmed);
        let old = self.objects.insert(
            hash,
            StoredObject {
                object,
                kind,
                hash_hex: display_hex(&hash),
                announcers,
                confirmed,
                stored_at: now,
                seen_in_run: self.run,
            },
        );
        if old.is_some_and(|o| !o.confirmed) {
            self.unconfirmed -= 1;
        }
        self.adopt_orphans(&hash, now);
        self.refetch_lost(&hash);
        Ok(true)
    }

    /// Whether an unconfirmed object of `bytes` data bytes from `from`
    /// fits. Past its sender's share the refusal is the sender's (D53);
    /// past the bounds in all, earlier runs' objects make room, and a
    /// refusal is overflow, which fails the run (D51).
    fn room_for_unconfirmed(
        &mut self,
        from: Option<SocketAddr>,
        bytes: usize,
    ) -> Result<(), ObjectRejection> {
        let (mut count, mut total) = (0usize, bytes);
        let (mut from_peer, mut peer_bytes) = (0usize, bytes);
        for o in self.objects.values().filter(|o| !o.confirmed) {
            count += 1;
            total += o.object.data.len();
            if from.is_some() && o.announcers.first() == from.as_ref() {
                from_peer += 1;
                peer_bytes += o.object.data.len();
            }
        }
        if from_peer >= MAX_UNCONFIRMED_PER_PEER || peer_bytes > MAX_UNCONFIRMED_BYTES_PER_PEER {
            return Err(ObjectRejection::StoreFull);
        }
        let fits = |count: usize, total: usize| {
            count < MAX_UNCONFIRMED_OBJECTS && total <= MAX_UNCONFIRMED_BYTES
        };
        if fits(count, total) {
            return Ok(());
        }
        // Earlier runs' unvoted unconfirmed objects, shown nowhere, make
        // room first, oldest first: this run's senders alone, each held to
        // its share, cannot fill the bounds (D53).
        let mut stale: Vec<(i64, [u8; 32], usize)> = self
            .objects
            .iter()
            .filter(|(h, o)| !o.confirmed && o.seen_in_run != self.run && !self.weighted(h))
            .map(|(h, o)| (o.stored_at, *h, o.object.data.len()))
            .collect();
        stale.sort_unstable();
        let mut evict = Vec::new();
        for (_, h, size) in stale {
            if fits(count, total) {
                break;
            }
            evict.push(h);
            count -= 1;
            total -= size;
        }
        if !fits(count, total) {
            self.overflow += 1;
            return Err(ObjectRejection::StoreFull);
        }
        for h in evict {
            self.remove_object(&h);
        }
        Ok(())
    }

    /// Evicts objects no key of the list voted on until an object of
    /// `bytes` data bytes fits [`MAX_OBJECTS`] and [`MAX_OBJECT_BYTES`]:
    /// those not announced in this run first, then the oldest (D47).
    /// Triggers count as unvoted, the wallet tallying no votes on them.
    /// `StoreFull` when only voted objects are left. Evicting or refusing
    /// an object announced in this run is overflow, which fails the run.
    /// An evicted hash is not remembered: it is fetched again once
    /// announced.
    fn make_room(&mut self, bytes: usize) -> Result<(), ObjectRejection> {
        let max = self.max_objects;
        let mut total: usize = self.objects.values().map(|o| o.object.data.len()).sum();
        let mut count = self.objects.len();
        let fits = |count: usize, total: usize| count < max && total + bytes <= MAX_OBJECT_BYTES;
        if fits(count, total) {
            return Ok(());
        }
        let mut victims: Vec<(u64, i64, [u8; 32], usize)> = self
            .objects
            .iter()
            .filter(|(h, _)| !self.weighted(h))
            .map(|(h, o)| (o.seen_in_run, o.stored_at, *h, o.object.data.len()))
            .collect();
        victims.sort_unstable();
        let mut evict = Vec::new();
        for (run, _, h, size) in victims {
            if fits(count, total) {
                break;
            }
            evict.push((run, h));
            count -= 1;
            total -= size;
        }
        if !fits(count, total) {
            self.overflow += 1;
            return Err(ObjectRejection::StoreFull);
        }
        for (run, h) in evict {
            if run == self.run {
                self.overflow += 1;
            }
            self.remove_object(&h);
        }
        Ok(())
    }

    /// Whether a key of a valid masternode of the list voted on `hash`
    /// (Core's tally counts no banned masternode).
    fn weighted(&self, hash: &[u8; 32]) -> bool {
        self.votes
            .get(hash)
            .is_some_and(|m| m.values().any(|v| self.valid_keys.contains(&v.key_id)))
    }

    /// Drops the proposals lapsed by `now` (D65), erases the triggers
    /// Core's cleanup would by the tip, drops objects unconfirmed for
    /// [`UNCONFIRMED_TTL_SECS`] and orphans past their expiry, and forgets
    /// erased hashes whose time passed.
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
        let mut lapsed = Vec::new();
        for (hash, o) in &self.objects {
            let gone = match &o.kind {
                ObjectKind::Proposal(d) => d.end_epoch.is_some_and(|e| e <= now),
                ObjectKind::Trigger(t) => tip.is_some_and(|tip| {
                    let h = i64::from(t.event_block_height);
                    tip > h + cycle || (self.network != Network::Mainnet && tip + 2 * cycle < h)
                }),
            };
            let proposal = matches!(o.kind, ObjectKind::Proposal(_));
            if gone && proposal {
                lapsed.push(*hash);
            } else if !o.confirmed && (gone || o.stored_at + UNCONFIRMED_TTL_SECS <= now) {
                drop.push(*hash);
            } else if gone {
                let until = o
                    .object
                    .time
                    .saturating_add(2 * cycle_secs + DELETION_DELAY_SECS);
                erase.push((*hash, until));
            }
        }
        for hash in drop {
            self.remove_object(&hash);
        }
        for hash in lapsed {
            self.remove_object(&hash);
            self.note_expired(hash);
        }
        for (hash, until) in erase {
            self.remove_object(&hash);
            remember(
                &mut self.erased,
                &mut self.erased_order,
                self.max_erased,
                hash,
                until,
            );
        }
        self.erased.retain(|_, until| *until >= now);
        if self.erased_order.len() > self.erased.len() {
            self.erased_order.retain(|h| self.erased.contains_key(h));
        }
        self.expire_orphans(now);
    }

    fn remove_object(&mut self, hash: &[u8; 32]) {
        if self.objects.remove(hash).is_some_and(|o| !o.confirmed) {
            self.unconfirmed -= 1;
        }
        for (_, v) in self.votes.remove(hash).unwrap_or_default() {
            self.seen_votes.remove(&v.vote_hash);
            self.vote_count -= 1;
        }
        self.per_key.retain(|(object, _), _| object != hash);
        // A remembered `KeyLimit` verdict stood on this object's slots: an
        // object back again starts with none (D75).
        if self
            .uncounted
            .remove(hash)
            .is_some_and(|u| !u.refused.is_empty())
        {
            self.forget_dismissed();
        }
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

    /// Drops an orphan that was not placed: its hash stays uncertain on its
    /// object (D76).
    fn lose_orphan(&mut self, hash: &[u8; 32]) {
        if let Some(o) = self.remove_orphan(hash) {
            let v = &o.vote.vote;
            self.note_lost(
                v.parent_hash,
                v.masternode_outpoint,
                o.vote.hash,
                o.vote.outcome,
            );
        }
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
                self.lose_orphan(&h);
            } else {
                break;
            }
        }
    }

    fn evict_oldest_orphan(&mut self) {
        while let Some((at, h)) = self.orphan_order.pop_front() {
            if self.live_entry(at, &h) {
                self.lose_orphan(&h);
                return;
            }
        }
    }

    /// Drops the remembered verdicts of unkept vote copies.
    fn forget_dismissed(&mut self) {
        self.dismissed.clear();
        self.dismissed_order.clear();
    }

    /// Sets the masternodes of the tip list, PoSe-banned ones included, and
    /// drops the votes and orphans whose key no longer belongs to one (Core
    /// `RemoveInvalidVotes`). Dropped votes become unseen, so a newer one
    /// can be fetched again.
    pub fn set_masternode_list(&mut self, entries: &[ListEntry]) {
        // A key's limit and resolution follow the list (D62).
        self.forget_dismissed();
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
        // A resolved outpoint is not limited (D11): its verdicts may change.
        self.forget_dismissed();
        self.resolved = resolved;
        // A copy re-signed by another key has the honest vote's hash, so it
        // kept the honest copy out: drop it and fetch the hash again. Until
        // the honest copy lands, the vote stays uncertain (D72).
        for (object, m) in &self.votes {
            for ((outpoint, key), v) in m {
                let Some(r) = self.resolved.get(outpoint) else {
                    continue;
                };
                if &r.voting_key_id == key {
                    continue;
                }
                self.refetch.push(v.vote_hash);
                // An honest vote newer than the copy supersedes it.
                if m.get(&(*outpoint, r.voting_key_id))
                    .is_none_or(|honest| honest.time <= v.time)
                {
                    let a = Awaited {
                        hash: v.vote_hash,
                        outcome: v.outcome,
                        time: v.time,
                        key: r.voting_key_id,
                    };
                    self.uncounted
                        .entry(*object)
                        .or_default()
                        .await_copy(*outpoint, a);
                }
            }
        }
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

    /// The votes to fetch again since the last call (D64).
    pub fn take_refetch(&mut self) -> Vec<[u8; 32]> {
        std::mem::take(&mut self.refetch)
    }

    fn key_ok(&self, outpoint: &OutPoint, key: &[u8; 20]) -> Result<(), VoteRejection> {
        check_key(&self.resolved, &self.keys, outpoint, key)
    }

    /// How many masternodes of the tip list hold `key`.
    fn holders(&self, key: &[u8; 20]) -> u32 {
        self.keys.get(key).map_or(0, |k| k.count)
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
            // A wrong-key copy of a resolved outpoint is awaited (D72); an
            // unresolved one whose key left the list is lost (D76).
            if let Some(v) = self.remove_vote(&slot)
                && !self.resolved.contains_key(&v.outpoint)
            {
                self.note_lost(slot.0, v.outpoint, v.vote_hash, v.outcome);
            }
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
            self.lose_orphan(&h);
        }
        // Votes no key of the list could count are no uncertainty.
        let keys = &self.keys;
        let resolved = &self.resolved;
        self.uncounted.retain(|_, u| {
            u.refused.retain(|(key, _), r| {
                let keep = keys.contains_key(key);
                if !keep {
                    u.held -= r.hashes.len();
                }
                keep
            });
            u.awaited.retain(|outpoint, list| {
                list.retain(|a| check_key(resolved, keys, outpoint, &a.key).is_ok());
                !list.is_empty()
            });
            !u.is_empty()
        });
    }

    /// Keeps a removed copy's vote as uncertainty on its object until a copy
    /// of it is stored or the peers asked have none (D76), and asks for it
    /// again if the object is stored.
    fn note_lost(
        &mut self,
        parent: [u8; 32],
        outpoint: OutPoint,
        hash: [u8; 32],
        outcome: VoteOutcome,
    ) {
        let max = self.max_refused;
        let u = self.uncounted.entry(parent).or_default();
        if u.lost_past.contains(&outcome)
            || u.lost
                .get(&outpoint)
                .is_some_and(|l| l.iter().any(|l| l.hash == hash))
        {
            return;
        }
        if u.held >= max {
            u.lost_past.insert(outcome);
            return;
        }
        u.lost
            .entry(outpoint)
            .or_default()
            .push(Lost { hash, outcome });
        u.held += 1;
        if self.objects.contains_key(&parent) {
            self.refetch.push(hash);
            return;
        }
        self.unstored_lost.push_back((parent, hash));
        let max = self.max_unstored_lost;
        if self.unstored_lost.len() > 2 * max {
            let live: VecDeque<_> = std::mem::take(&mut self.unstored_lost)
                .into_iter()
                .filter(|(p, h)| self.unstored_lost_live(p, h))
                .collect();
            self.unstored_lost = live;
        }
        while self.unstored_lost.len() > max {
            let Some((p, h)) = self.unstored_lost.pop_front() else {
                break;
            };
            if self.unstored_lost_live(&p, &h) {
                self.forget_lost(&p, &h);
            }
        }
    }

    /// Whether an `unstored_lost` entry still stands for a held vote of an
    /// object the store does not hold.
    fn unstored_lost_live(&self, parent: &[u8; 32], hash: &[u8; 32]) -> bool {
        !self.objects.contains_key(parent)
            && self
                .uncounted
                .get(parent)
                .is_some_and(|u| u.lost.values().any(|l| l.iter().any(|l| &l.hash == hash)))
    }

    /// Drops a held lost vote of `parent`; whether there was one.
    fn forget_lost(&mut self, parent: &[u8; 32], hash: &[u8; 32]) -> bool {
        let Some(u) = self.uncounted.get_mut(parent) else {
            return false;
        };
        let outpoints: Vec<OutPoint> = u
            .lost
            .iter()
            .filter(|(_, l)| l.iter().any(|l| &l.hash == hash))
            .map(|(op, _)| *op)
            .collect();
        let settled = outpoints
            .iter()
            .fold(false, |any, op| u.settle_lost(op, hash) | any);
        if u.is_empty() {
            self.uncounted.remove(parent);
        }
        settled
    }

    /// Asks again for the lost votes of a stored `object` (D76).
    fn refetch_lost(&mut self, object: &[u8; 32]) {
        if let Some(u) = self.uncounted.get(object) {
            self.refetch
                .extend(u.lost.values().flatten().map(|l| l.hash));
        }
    }

    /// Asks again for every lost vote of a stored object: a run's peers may
    /// have it where the last run's had not (D76).
    pub fn requeue_lost(&mut self) {
        let objects: Vec<[u8; 32]> = self
            .uncounted
            .keys()
            .filter(|h| self.objects.contains_key(*h))
            .copied()
            .collect();
        for h in objects {
            self.refetch_lost(&h);
        }
    }

    /// Settles the lost vote `hash`: every peer asked for it again (at least
    /// two) answered that it has none, so no node holds it (D76).
    pub fn settle_lost(&mut self, hash: &[u8; 32]) {
        let parents: Vec<[u8; 32]> = self
            .uncounted
            .iter()
            .filter(|(_, u)| u.lost.values().flatten().any(|l| &l.hash == hash))
            .map(|(p, _)| *p)
            .collect();
        for p in parents {
            self.forget_lost(&p, hash);
        }
    }

    /// Keeps a vote the per-key cap refused as uncertainty on its object:
    /// it may be a masternode's honest vote kept out by copies (D71).
    fn note_refused(
        &mut self,
        parent: [u8; 32],
        key: [u8; 20],
        outcome: VoteOutcome,
        hash: [u8; 32],
    ) {
        let n = self.holders(&key) as usize;
        let max = self.max_refused;
        let u = self.uncounted.entry(parent).or_default();
        let r = u.refused.entry((key, outcome)).or_default();
        if r.saturated || r.hashes.contains(&hash) {
            return;
        }
        if r.hashes.len() >= n || u.held >= max {
            u.held -= r.hashes.len();
            r.hashes = HashSet::new();
            r.saturated = true;
        } else {
            r.hashes.insert(hash);
            u.held += 1;
        }
    }

    /// Settles the uncertainty a vote stored at (`parent`, `outpoint`,
    /// `key`) resolves: its own refused, awaited or lost hash, and awaited
    /// votes of that masternode it supersedes (D71, D72, D76).
    fn settle_uncounted(&mut self, parent: &[u8; 32], v: &CurrentVote) {
        let Some(u) = self.uncounted.get_mut(parent) else {
            return;
        };
        if let Some(r) = u.refused.get_mut(&(v.key_id, v.outcome))
            && r.hashes.remove(&v.vote_hash)
        {
            u.held -= 1;
            if r.hashes.is_empty() && !r.saturated {
                u.refused.remove(&(v.key_id, v.outcome));
            }
        }
        if let Some(list) = u.awaited.get_mut(&v.outpoint) {
            list.retain(|a| a.key != v.key_id || (a.hash != v.vote_hash && a.time >= v.time));
            if list.is_empty() {
                u.awaited.remove(&v.outpoint);
            }
        }
        // Any copy of a lost vote, stored, counts for it (D64, D76).
        u.settle_lost(&v.outpoint, &v.vote_hash);
        if u.is_empty() {
            self.uncounted.remove(parent);
        }
    }

    /// The weight `outpoint`'s votes on `hash` count exactly: its
    /// masternode's, if resolved and no vote of it is awaited (D72) or
    /// lost (D76).
    pub fn exact_weight(&self, hash: &[u8; 32], outpoint: &OutPoint) -> Option<u32> {
        if self
            .uncounted
            .get(hash)
            .is_some_and(|u| u.awaited.contains_key(outpoint) || u.lost.contains_key(outpoint))
        {
            return None;
        }
        self.resolved.get(outpoint).map(|r| r.weight)
    }

    /// `hash`'s ranged tally ([`ranged_tally`]), widened by the votes on it
    /// the store holds no countable copy of: a key's refused votes up to an
    /// evonode's weight each, at most one per masternode holding it (D71);
    /// an awaited vote up to its masternode's weight, its outpoint's other
    /// votes counted as unresolved ones (D72); a lost vote likewise, once
    /// per outpoint, and an outcome whose lost votes are past the bound up
    /// to the whole list's weight (D76).
    pub fn tally(&self, hash: &[u8; 32]) -> RangedTally {
        let mut t = ranged_tally(
            self.votes_on(hash),
            |k| self.key_weights(k),
            |op| self.exact_weight(hash, op),
        );
        let Some(u) = self.uncounted.get(hash) else {
            return t;
        };
        for ((key, outcome), r) in &u.refused {
            let n = self.holders(key);
            let votes = if r.saturated {
                n
            } else {
                n.min(r.hashes.len() as u32)
            };
            t.widen(*outcome, votes.saturating_mul(EVONODE_VOTE_WEIGHT), votes);
        }
        for (outpoint, list) in &u.awaited {
            let w = self
                .resolved
                .get(outpoint)
                .map_or(EVONODE_VOTE_WEIGHT, |r| r.weight);
            for a in list {
                t.widen(a.outcome, w, 1);
            }
        }
        // One masternode per outpoint: its lost votes count once, up to its
        // weight for each of their outcomes (D76).
        for (outpoint, list) in &u.lost {
            let w = self
                .resolved
                .get(outpoint)
                .map_or(EVONODE_VOTE_WEIGHT, |r| r.weight);
            let outcomes: HashSet<VoteOutcome> = list.iter().map(|l| l.outcome).collect();
            for (i, outcome) in outcomes.into_iter().enumerate() {
                t.widen(outcome, w, u32::from(i == 0));
            }
        }
        // Past the bound, any masternode may be one of them.
        for outcome in &u.lost_past {
            t.widen(*outcome, self.valid_weight, 1);
        }
        t
    }

    fn remove_vote(&mut self, (object, outpoint, key): &Slot) -> Option<CurrentVote> {
        let v = self
            .votes
            .get_mut(object)
            .and_then(|m| m.remove(&(*outpoint, *key)))?;
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
        Some(v)
    }

    fn remember_ignored(&mut self, hash: [u8; 32], parent: [u8; 32]) {
        remember(
            &mut self.ignored,
            &mut self.ignored_order,
            MAX_IGNORED_VOTES,
            hash,
            parent,
        );
    }

    fn parent(&self, hash: &[u8; 32], now: i64) -> Parent {
        // A refused parent's votes are ignored as an erased one's are.
        if self.erased.contains_key(hash)
            || self.expired.contains_key(hash)
            || self.refused.contains_key(hash)
        {
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
        if let Some(r) = self.dismissed.get(&copy_digest(&hash, &vote.signature)) {
            return Err(*r);
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
            self.remember_ignored(hash, vote.parent_hash);
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
        let copy = copy_digest(&v.hash, &v.vote.signature);
        let placed = self.place(v, from, now, false);
        if let Err(
            r @ (VoteRejection::Obsolete | VoteRejection::SameTimestamp | VoteRejection::KeyLimit),
        ) = placed
        {
            remember(
                &mut self.dismissed,
                &mut self.dismissed_order,
                MAX_DISMISSED_VOTES,
                copy,
                r,
            );
        }
        placed
    }

    /// Whether a vote on `parent` from `from` would wait for its object
    /// past the sender's orphan bound: refused before its signature is
    /// recovered, as Core does not check an orphan's (D62).
    pub fn orphan_bound_full(&mut self, parent: &[u8; 32], from: SocketAddr, now: i64) -> bool {
        if self.objects.contains_key(parent) {
            return false;
        }
        self.expire_orphans(now);
        self.at_orphan_bound(&from)
    }

    fn at_orphan_bound(&self, peer: &SocketAddr) -> bool {
        self.orphans_per_peer.get(peer).copied().unwrap_or(0) >= MAX_ORPHANS_PER_PEER
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
            let n = self.holders(&key);
            let used = self.per_key.get(&(parent, key)).copied().unwrap_or(0);
            if used >= 2 * n + 2 {
                if !rate_checked {
                    self.note_refused(parent, key, outcome, v.hash);
                }
                return Err(VoteRejection::KeyLimit);
            }
        }
        if !self.objects.contains_key(&parent) {
            return Err(self.queue_orphan(v, from, now));
        }
        // A newer vote in an unresolved slot may be the slot key's holder
        // replacing a copy that stood for another masternode's vote (D76).
        let replaced = self
            .remove_vote(&(parent, outpoint, key))
            .filter(|_| !resolved);
        let stored = CurrentVote {
            outpoint,
            outcome,
            time: v.vote.time,
            key_id: key,
            vote_hash: v.hash,
        };
        self.settle_uncounted(&parent, &stored);
        self.votes
            .entry(parent)
            .or_default()
            .insert((outpoint, key), stored);
        if !resolved {
            *self.per_key.entry((parent, key)).or_default() += 1;
        }
        self.vote_count += 1;
        self.seen_votes.insert(v.hash, parent);
        if let Some(c) = replaced {
            self.note_lost(parent, outpoint, c.vote_hash, c.outcome);
        }
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
            && self.at_orphan_bound(&peer)
        {
            return VoteRejection::Orphan;
        }
        let parent = v.vote.parent_hash;
        let slot = (parent, v.vote.masternode_outpoint, v.key_id);
        if let Some(older) = self.orphan_slots.get(&slot).copied() {
            self.lose_orphan(&older);
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

    /// Sol r1 Major 2: two peers confirming one 500-object batch after
    /// another cannot grow the store past its total cap.
    #[test]
    fn confirmed_batches_from_two_peers_stay_within_the_total_cap() {
        let mut s = GovernanceStore::new(Network::Regtest);
        let second = peer(2).unwrap();
        for batch in 0..12i64 {
            for i in 0..500 {
                let o = proposal(&format!("p{batch}-{i}"));
                let h = o.hash();
                let _ = s.add_object(o, peer(1), NOW + batch);
                s.note_announcer(&h, second);
            }
            assert!(
                s.object_count() <= MAX_OBJECTS,
                "batch {batch}: {}",
                s.object_count()
            );
        }
        assert_eq!(s.object_count(), MAX_OBJECTS);
    }

    /// D47: past the cap the oldest proposals no listed key voted on go
    /// first; a voted one never makes room for an unvoted one, and a new
    /// object finding only voted ones is refused.
    #[test]
    fn the_object_cap_evicts_the_oldest_unvoted_proposals() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.max_objects = 3;
        s.set_masternode_list(&[entry(1, 1), entry(2, 1)]);
        let voted = proposal("voted");
        let hv = voted.hash();
        s.add_object(voted, None, NOW).unwrap();
        s.add_vote(vote(hv, 1, 1, VoteOutcome::Yes, NOW - 10), None, NOW)
            .unwrap();
        let (a, b, c) = (proposal("a"), proposal("b"), proposal("c"));
        let (ha, hb, hc) = (a.hash(), b.hash(), c.hash());
        s.add_object(a, None, NOW + 1).unwrap();
        s.add_object(b, None, NOW + 2).unwrap();
        s.add_object(c, None, NOW + 3).unwrap();
        assert!(s.object(&hv).is_some() && s.object(&hc).is_some());
        assert!(s.object(&ha).is_none() && s.object(&hb).is_some());
        // An evicted hash is fetched again.
        assert!(!s.has_object(&ha));
        // b gains a listed key's vote: c is the only one left to evict.
        s.add_vote(vote(hb, 2, 2, VoteOutcome::No, NOW - 10), None, NOW)
            .unwrap();
        s.add_object(proposal("d"), None, NOW + 4).unwrap();
        assert!(s.object(&hc).is_none() && s.object(&hb).is_some());
        // Only voted proposals and the newest unvoted one: d goes for e,
        // and with every proposal voted on, a new one is refused.
        let e = proposal("e");
        let he = e.hash();
        s.add_object(e, None, NOW + 5).unwrap();
        s.add_vote(vote(he, 1, 3, VoteOutcome::Yes, NOW - 10), None, NOW)
            .unwrap();
        assert_eq!(
            s.add_object(proposal("f"), None, NOW + 6),
            Err(ObjectRejection::StoreFull)
        );
        assert_eq!(s.object_count(), 3);
    }

    /// Validator M1 on D47: objects not announced in this run go before
    /// this run's, however new; evicting one of this run's, or refusing an
    /// object, is overflow.
    #[test]
    fn the_cap_evicts_earlier_runs_objects_first_and_counts_this_runs() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.max_objects = 3;
        s.begin_run();
        let (old, renewed) = (proposal("old"), proposal("renewed"));
        let (ho, hr) = (old.hash(), renewed.hash());
        s.add_object(renewed, peer(1), NOW).unwrap();
        s.add_object(old, peer(1), NOW + 10).unwrap();
        s.begin_run();
        // Announced again in the new run: kept over the newer `old`.
        s.note_announcer(&hr, peer(2).unwrap());
        let honest = proposal("honest");
        let hh = honest.hash();
        s.add_object(honest, peer(1), NOW + 20).unwrap();
        s.add_object(proposal("fake0"), peer(3), NOW + 30).unwrap();
        assert!(!s.has_object(&ho) && s.has_object(&hr));
        assert_eq!(s.take_overflow(), 0);
        // Only this run's objects are left to evict.
        s.add_object(proposal("fake1"), peer(3), NOW + 40).unwrap();
        assert!(!s.has_object(&hr) && s.has_object(&hh));
        assert_eq!(s.take_overflow(), 1);
        assert_eq!(s.take_overflow(), 0);
        assert_eq!(s.object_count(), 3);
    }

    /// Validator M1 on D47: unvoted triggers, which the wallet cannot tell
    /// from forged ones, are evicted like unvoted proposals, so a store
    /// full of them takes a new run's proposal; a voted proposal still
    /// outlives them all, and a store of voted proposals refuses as
    /// overflow.
    #[test]
    fn unvoted_triggers_are_evicted_and_voted_proposals_kept() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.max_objects = 3;
        s.set_masternode_list(&[entry(1, 1), entry(2, 1), entry(3, 1)]);
        s.begin_run();
        for h in 100..103 {
            s.add_object(trigger(h), peer(1), NOW).unwrap();
        }
        s.begin_run();
        let mut voted = Vec::new();
        for (i, key) in (1..=3u8).enumerate() {
            let p = proposal(&format!("p{i}"));
            let h = p.hash();
            s.add_object(p, None, NOW + 1 + i as i64).unwrap();
            s.add_vote(vote(h, key, key, VoteOutcome::Yes, NOW - 10), None, NOW)
                .unwrap();
            voted.push(h);
        }
        assert_eq!(s.take_overflow(), 0);
        assert!(voted.iter().all(|h| s.object(h).is_some()));
        assert_eq!(
            s.add_object(trigger(103), peer(1), NOW + 9),
            Err(ObjectRejection::StoreFull)
        );
        assert_eq!(s.take_overflow(), 1);
    }

    /// Validator r2 m3: a vote by a banned masternode's key, which Core's
    /// tally leaves out, does not keep an object from eviction.
    #[test]
    fn a_banned_keys_vote_does_not_keep_an_object() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.max_objects = 2;
        s.set_masternode_list(&[
            entry(1, 1),
            ListEntry {
                valid: false,
                ..entry(2, 1)
            },
        ]);
        let (banned, valid) = (proposal("banned"), proposal("valid"));
        let (hb, hv) = (banned.hash(), valid.hash());
        s.add_object(banned, None, NOW).unwrap();
        s.add_vote(vote(hb, 2, 1, VoteOutcome::Yes, NOW - 10), None, NOW)
            .unwrap();
        s.add_object(valid, None, NOW + 1).unwrap();
        s.add_vote(vote(hv, 1, 2, VoteOutcome::Yes, NOW - 10), None, NOW)
            .unwrap();
        s.add_object(proposal("new"), None, NOW + 2).unwrap();
        assert!(!s.has_object(&hb) && s.object(&hv).is_some());
    }

    /// Validator r3 MJ1: one peer fills at most its share of the
    /// unconfirmed bytes, silently; past the bound in all, a refusal is
    /// overflow.
    #[test]
    fn one_peer_holds_at_most_its_share_of_unconfirmed_bytes() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.begin_run();
        let fill = |s: &mut GovernanceStore, from: Option<SocketAddr>, height: &mut i32| {
            loop {
                let t = big_trigger(*height, 20_000);
                *height += 1;
                if s.add_object(t, from, NOW).is_err() {
                    break;
                }
            }
        };
        let mut height = 100;
        fill(&mut s, peer(1), &mut height);
        let mine: usize = s.unconfirmed().map(|o| o.object.data.len()).sum();
        assert!(mine <= MAX_UNCONFIRMED_BYTES_PER_PEER, "{mine}");
        assert_eq!(s.take_overflow(), 0);
        for n in 2..=6 {
            fill(&mut s, peer(n), &mut height);
        }
        let all: usize = s.unconfirmed().map(|o| o.object.data.len()).sum();
        assert!(all <= MAX_UNCONFIRMED_BYTES, "{all}");
        assert!(s.take_overflow() > 0);
    }

    fn big_trigger(height: i32, payees: usize) -> GovernanceObject {
        let mut t = trigger(height);
        let addresses = vec!["yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf"; payees].join("|");
        let amounts = vec!["1"; payees].join("|");
        let hashes = vec![HASH; payees].join("|");
        t.data = format!(
            r#"{{"event_block_height":{height},"payment_addresses":"{addresses}","payment_amounts":"{amounts}","proposal_hashes":"{hashes}","type":2}}"#
        )
        .into_bytes();
        t
    }

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

    /// Sol r2 Major 1: two peers confirm batch after batch of distinct
    /// proposals that already ended, or that end soon after they arrive;
    /// what the store keeps of them stays bounded (D65).
    #[test]
    fn expired_proposals_from_two_peers_leave_bounded_state() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.max_expired = 50;
        let second = peer(2).unwrap();
        let mut now = NOW;
        for batch in 0..5 {
            // Lapsed on arrival: refused at no cost, never stored, and
            // remembered so it is not fetched again.
            for i in 0..40 {
                let o = proposal_ending(&format!("old{batch}-{i}"), now);
                let h = o.hash();
                assert_eq!(
                    s.add_object(o, peer(1), now),
                    Err(ObjectRejection::ProposalExpired)
                );
                assert!(s.has_object(&h));
                s.note_announcer(&h, second);
            }
            assert_eq!(s.object_count(), 0);
            // Current on arrival, dropped at its end epoch.
            for i in 0..40 {
                let o = proposal_ending(&format!("soon{batch}-{i}"), now + 1);
                let h = o.hash();
                assert_eq!(s.add_object(o, peer(1), now), Ok(true));
                s.note_announcer(&h, second);
            }
            assert_eq!(s.object_count(), 40);
            now += 1;
            s.prune(now);
            assert_eq!(s.object_count(), 0);
            assert_eq!((s.expired.len(), s.expired_order.len()), (50, 50));
            assert!(s.erased.is_empty() && s.refused.is_empty());
        }
        // Erased triggers are bounded too.
        s.max_erased = 3;
        for height in 0..10 {
            s.add_object(trigger(height), None, NOW).unwrap();
        }
        s.set_tip(1_000);
        s.prune(NOW);
        assert_eq!((s.erased.len(), s.erased_order.len()), (3, 3));
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
        // Dropped at its end epoch, votes with it (D65); the hash stays
        // known.
        s.prune(end - 1);
        assert!(s.object(&h).is_some());
        s.prune(end);
        assert!(s.object(&h).is_none());
        assert_eq!(s.vote_count(), 0);
        assert!(s.has_object(&h));
        assert_eq!(s.add_object(obj, peer(2), end), Ok(false));
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
        // The tally ranges over every unresolved vote, 0..4 (D64). Key 1
        // claims one outpoint with one masternode and carries the estimate;
        // key 2 claims two with one, so only its newest uncontested vote is
        // estimated.
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
        assert_eq!((t.no.min, t.no.max, t.no.estimate), (0, 4, 0));
        assert_eq!((t.yes.min, t.yes.max, t.yes.estimate), (0, 4, 1));
    }

    /// The fillers of key 1's 2n + 2 unresolved slots on `h`, ABSTAIN.
    fn fill_key_1(s: &mut GovernanceStore, h: [u8; 32]) {
        let n = s.key_weights(&key_id(1)).unwrap().count as u8;
        for op in 100..100 + 2 * n + 2 {
            let v = vote(h, 1, op, VoteOutcome::Abstain, NOW - 50);
            assert_eq!(s.add_vote(v, peer(9), NOW), Ok(()));
        }
    }

    fn uncounted(s: &GovernanceStore, h: &[u8; 32], outcome: VoteOutcome) -> (u32, u32) {
        let t = s.tally(h);
        let r = match outcome {
            VoteOutcome::Yes => t.yes,
            VoteOutcome::No => t.no,
            VoteOutcome::Abstain => t.abstain,
        };
        (t.uncounted, r.max)
    }

    #[test]
    fn votes_the_key_cap_refuses_widen_their_tally_until_stored() {
        // Sol r3 Major 1: a vote the per-key cap refuses may be the key's
        // masternode's own, which Core counts: up to 4 more for its
        // outcome, and a close call (D71).
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4)]);
        fill_key_1(&mut s, h);
        let yes = vote(h, 1, 200, VoteOutcome::Yes, NOW - 40);
        assert_eq!(
            s.add_vote(yes.clone(), peer(1), NOW),
            Err(VoteRejection::KeyLimit)
        );
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (1, 4));
        // Another copy, of another signature form, is the same vote.
        assert_eq!(
            s.add_vote(yes.clone(), peer(2), NOW),
            Err(VoteRejection::KeyLimit)
        );
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (1, 4));
        // The vote itself, stored once the key holds more masternodes,
        // settles it.
        s.set_masternode_list(&[entry(1, 4), entry(1, 4)]);
        assert_eq!(s.add_vote(yes, peer(1), NOW), Ok(()));
        assert_eq!(s.tally(&h).uncounted, 0);
        // The wallet's own vote is refused to its caller, not kept.
        let filler = vote(h, 1, 150, VoteOutcome::Abstain, NOW - 50);
        assert_eq!(s.add_vote(filler, peer(9), NOW), Ok(()));
        let own = vote(h, 1, 201, VoteOutcome::No, NOW - 40);
        assert_eq!(s.add_own_vote(own, NOW), Err(VoteRejection::KeyLimit));
        assert_eq!(s.tally(&h).uncounted, 0);
    }

    #[test]
    fn refused_votes_stay_bounded_per_proposal() {
        // D71: a key's refused votes count at most one per masternode
        // holding it, and a proposal holds at most `max_refused` hashes;
        // past either, the key's votes of that outcome stay at that
        // bound, never below it.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(1, 4), entry(1, 4)]);
        s.max_refused = 2;
        fill_key_1(&mut s, h);
        let refuse = |s: &mut GovernanceStore, h: [u8; 32], op: u8, outcome: VoteOutcome| {
            let v = vote(h, 1, op, outcome, NOW - 40);
            assert_eq!(s.add_vote(v, peer(1), NOW), Err(VoteRejection::KeyLimit));
        };
        refuse(&mut s, h, 200, VoteOutcome::Yes);
        refuse(&mut s, h, 201, VoteOutcome::Yes);
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (2, 8));
        refuse(&mut s, h, 202, VoteOutcome::Yes);
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (3, 12));
        assert_eq!(s.uncounted[&h].held, 0);
        for op in 203..250 {
            refuse(&mut s, h, op, VoteOutcome::Yes);
        }
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (3, 12));
        refuse(&mut s, h, 250, VoteOutcome::No);
        assert_eq!(uncounted(&s, &h, VoteOutcome::No), (4, 4));
        // One masternode per key, the bound follows the list.
        let obj = proposal("b");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4)]);
        fill_key_1(&mut s, h);
        for op in 200..210 {
            refuse(&mut s, h, op, VoteOutcome::Yes);
        }
        assert_eq!(uncounted(&s, &h, VoteOutcome::Yes), (1, 4));
        assert!(s.uncounted[&h].refused[&(key_id(1), VoteOutcome::Yes)].saturated);
        // A key the list drops counts nothing, so its refused votes leave no
        // uncertainty; its stored votes, dropped, may have stood for other
        // masternodes' and are lost until settled (D76).
        s.set_masternode_list(&[entry(2, 4)]);
        assert!(s.uncounted[&h].refused.is_empty());
        let fillers: Vec<[u8; 32]> = (100..104)
            .map(|op| vote(h, 1, op, VoteOutcome::Abstain, NOW - 50).hash())
            .collect();
        let mut asked = s.take_refetch();
        asked.sort();
        let mut want = fillers.clone();
        want.sort();
        assert_eq!(asked, want);
        assert_eq!(s.tally(&h).uncounted, 4);
        for f in &fillers {
            s.settle_lost(f);
        }
        assert!(!s.uncounted.contains_key(&h));
        // Nor does a proposal removed, and its remembered `KeyLimit`
        // verdicts go with it: back again, the vote is placed (D75).
        let mut s = store(&obj, &[entry(1, 4)]);
        fill_key_1(&mut s, h);
        refuse(&mut s, h, 200, VoteOutcome::Yes);
        s.remove_object(&h);
        assert!(s.uncounted.is_empty());
        assert_eq!(s.add_object(obj.clone(), peer(1), NOW), Ok(true));
        let v = vote(h, 1, 200, VoteOutcome::Yes, NOW - 40);
        assert_eq!(s.add_vote(v, peer(1), NOW), Ok(()));
    }

    #[test]
    fn a_dropped_wrong_key_copy_is_uncertain_until_its_honest_copy() {
        // Sol r3 Major 2: the copy key 2 re-signed of outpoint 1's NO is
        // dropped once 1 resolves to key 1. Until the honest copy lands
        // the NO stays up to the masternode's weight (D72); the outpoint's
        // older honest vote counts unresolved meanwhile, as the newer one
        // may replace it.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        let older = vote(h, 1, 1, VoteOutcome::Yes, NOW - 60);
        assert_eq!(s.add_vote(older, peer(1), NOW), Ok(()));
        let honest = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        let mut forged = honest.clone();
        sign(&mut forged, 2);
        assert_eq!(s.add_vote(forged.clone(), peer(2), NOW), Ok(()));
        let resolved = ResolvedMasternode {
            pro_tx_hash: [1; 32],
            voting_key_id: key_id(1),
            weight: 4,
        };
        s.set_resolved(HashMap::from([(outpoint(1), resolved)]));
        assert_eq!(s.take_refetch(), vec![honest.hash()]);
        let t = s.tally(&h);
        assert_eq!(t.uncounted, 1);
        assert_eq!((t.no.min, t.no.max), (0, 4));
        assert_eq!((t.yes.min, t.yes.max), (0, 4));
        assert_eq!(s.exact_weight(&h, &outpoint(1)), None);
        // The re-signed copy again is refused, and settles nothing.
        assert_eq!(
            s.add_vote(forged, peer(2), NOW),
            Err(VoteRejection::WrongVotingKey)
        );
        assert_eq!(s.tally(&h).uncounted, 1);
        assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(()));
        let t = s.tally(&h);
        assert_eq!(t.uncounted, 0);
        assert_eq!((t.no.min, t.no.max, t.yes.max), (4, 4, 0));
        assert_eq!(s.exact_weight(&h, &outpoint(1)), Some(4));
    }

    #[test]
    fn an_awaited_copy_is_settled_by_a_newer_vote_or_a_key_change() {
        // D72: the masternode's own later vote replaces the awaited one in
        // Core, so settles it; an honest vote already newer leaves nothing
        // awaited; a new voting key leaves it nothing to count for.
        let obj = proposal("a");
        let h = obj.hash();
        let resolved = HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [1; 32],
                voting_key_id: key_id(1),
                weight: 4,
            },
        )]);
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        let mut forged = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        sign(&mut forged, 2);
        assert_eq!(s.add_vote(forged.clone(), peer(2), NOW), Ok(()));
        s.set_resolved(resolved.clone());
        assert_eq!(s.tally(&h).uncounted, 1);
        // A vote of the same time is not newer: it leaves the copy awaited.
        let same_time = vote(h, 1, 1, VoteOutcome::Yes, NOW - 40);
        assert_eq!(s.add_vote(same_time, peer(1), NOW), Ok(()));
        assert_eq!(s.tally(&h).uncounted, 1);
        let newer = vote(h, 1, 1, VoteOutcome::Abstain, NOW - 30);
        assert_eq!(s.add_vote(newer, peer(1), NOW), Ok(()));
        assert_eq!(s.tally(&h).uncounted, 0);

        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        assert_eq!(s.add_vote(forged, peer(2), NOW), Ok(()));
        let newer = vote(h, 1, 1, VoteOutcome::Yes, NOW - 30);
        assert_eq!(s.add_vote(newer, peer(1), NOW), Ok(()));
        s.set_resolved(resolved.clone());
        assert_eq!(s.tally(&h).uncounted, 0);
        assert_eq!(s.exact_weight(&h, &outpoint(1)), Some(4));

        // Nor does an honest vote of the copy's time: Core keeps either.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        let mut forged = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        sign(&mut forged, 2);
        assert_eq!(s.add_vote(forged, peer(2), NOW), Ok(()));
        let same_time = vote(h, 1, 1, VoteOutcome::Yes, NOW - 40);
        assert_eq!(s.add_vote(same_time, peer(1), NOW), Ok(()));
        s.set_resolved(resolved.clone());
        assert_eq!(s.tally(&h).uncounted, 1);

        // A masternode's new voting key drops its old key's votes (Core
        // `RemoveInvalidVotes`), the awaited one too.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4), entry(3, 4)]);
        let mut forged = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        sign(&mut forged, 2);
        assert_eq!(s.add_vote(forged, peer(2), NOW), Ok(()));
        s.set_resolved(resolved.clone());
        assert_eq!(s.tally(&h).uncounted, 1);
        let mut rekeyed = resolved;
        rekeyed.get_mut(&outpoint(1)).unwrap().voting_key_id = key_id(3);
        s.set_resolved(rekeyed);
        assert_eq!(s.tally(&h).uncounted, 0);
    }

    /// Key 2's copy of key 1's NO for outpoint 1 (the same hash), stored
    /// unresolved in key 2's slot, and the honest vote.
    fn forged_no(s: &mut GovernanceStore, h: [u8; 32]) -> GovernanceVote {
        let honest = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        let mut forged = honest.clone();
        sign(&mut forged, 2);
        assert_eq!(s.add_vote(forged, peer(2), NOW), Ok(()));
        assert!(s.has_vote(&honest.hash()));
        honest
    }

    /// A lost vote on `h`: one uncounted, NO up to 4.
    fn lost_no(s: &GovernanceStore, h: &[u8; 32]) {
        let t = s.tally(h);
        assert_eq!(t.uncounted, 1);
        assert_eq!((t.no.min, t.no.max), (0, 4));
    }

    #[test]
    fn a_copy_its_slot_replaces_is_lost_until_settled() {
        // Validator r21 Major 1 (D76): key 2's holder replaces its own copy
        // of key 1's NO with a newer vote; the honest copies were skipped
        // as known. The NO stays up to 4 and is asked for again, until a
        // copy is stored or the peers asked have none.
        let obj = proposal("a");
        let h = obj.hash();
        for settle_by_copy in [true, false] {
            let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
            let honest = forged_no(&mut s, h);
            assert!(s.take_refetch().is_empty());
            let own = vote(h, 2, 1, VoteOutcome::Abstain, NOW - 30);
            assert_eq!(s.add_vote(own, peer(2), NOW), Ok(()));
            assert!(!s.has_vote(&honest.hash()));
            assert_eq!(s.take_refetch(), vec![honest.hash()]);
            lost_no(&s, &h);
            assert_eq!(s.exact_weight(&h, &outpoint(1)), None);
            if settle_by_copy {
                assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(()));
            } else {
                s.settle_lost(&honest.hash());
            }
            assert_eq!(s.tally(&h).uncounted, 0);
            assert!(!s.uncounted.contains_key(&h));
        }
    }

    #[test]
    fn a_copy_whose_key_leaves_the_list_is_lost() {
        // D76: key 2 leaves the list (its masternode removed or re-keyed),
        // and its copy of key 1's NO goes with it.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        let honest = forged_no(&mut s, h);
        s.set_masternode_list(&[entry(1, 4)]);
        assert!(!s.has_vote(&honest.hash()));
        assert_eq!(s.take_refetch(), vec![honest.hash()]);
        lost_no(&s, &h);
        // A newer list keeps it: only a copy or the peers settle it.
        s.set_masternode_list(&[entry(1, 4), entry(3, 4)]);
        lost_no(&s, &h);
        assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(()));
        assert_eq!(s.tally(&h).uncounted, 0);
    }

    #[test]
    fn a_copy_lost_as_an_orphan_is_lost_on_its_object() {
        // D76: key 2's copy waits as an orphan for object B; it expires, is
        // displaced by key 2's newer vote in its slot, or is dropped when
        // its outpoint resolves to key 1. B, stored later, holds the NO as
        // uncertain and asks for it again.
        let a = proposal("a");
        let b = proposal("b");
        let hb = b.hash();
        let resolved = HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [1; 32],
                voting_key_id: key_id(1),
                weight: 4,
            },
        )]);
        for path in ["expired", "displaced", "resolved"] {
            let mut s = store(&a, &[entry(1, 4), entry(2, 4)]);
            let honest = vote(hb, 1, 1, VoteOutcome::No, NOW - 40);
            let mut forged = honest.clone();
            sign(&mut forged, 2);
            assert_eq!(s.add_vote(forged, peer(2), NOW), Err(VoteRejection::Orphan));
            match path {
                "expired" => s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS + 1),
                "displaced" => {
                    let own = vote(hb, 2, 1, VoteOutcome::Abstain, NOW - 30);
                    assert_eq!(s.add_vote(own, peer(2), NOW), Err(VoteRejection::Orphan));
                }
                _ => s.set_resolved(resolved.clone()),
            }
            assert!(!s.has_vote(&honest.hash()), "{path}");
            assert!(s.take_refetch().is_empty(), "{path}");
            assert_eq!(s.add_object(b.clone(), peer(1), NOW), Ok(true));
            assert_eq!(s.take_refetch(), vec![honest.hash()], "{path}");
            lost_no(&s, &hb);
            let exact = (path == "resolved").then_some(4);
            assert_eq!(s.exact_weight(&hb, &outpoint(1)), None, "{path}");
            assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(()), "{path}");
            assert_eq!(s.tally(&hb).uncounted, 0, "{path}");
            assert_eq!(s.exact_weight(&hb, &outpoint(1)), exact, "{path}");
        }
    }

    #[test]
    fn lost_votes_stay_bounded() {
        // D76: past the per-proposal bound, a lost vote's outcome counts up
        // to the whole list; one per outpoint; lost votes of objects not
        // stored are kept up to their bound, oldest forgotten first.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_refused = 1;
        forged_no(&mut s, h);
        let own = vote(h, 2, 1, VoteOutcome::Abstain, NOW - 30);
        assert_eq!(s.add_vote(own, peer(2), NOW), Ok(()));
        lost_no(&s, &h);
        // Key 2's ABSTAIN replaced too: the same outpoint, past the bound.
        let newer = vote(h, 2, 1, VoteOutcome::Yes, NOW - 20);
        assert_eq!(s.add_vote(newer, peer(2), NOW), Ok(()));
        let t = s.tally(&h);
        assert_eq!(t.uncounted, 2);
        assert_eq!(t.no.max, 4);
        assert_eq!(t.abstain.max, s.valid_weight());
        // Two lost votes of one outpoint are one voter.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        forged_no(&mut s, h);
        for (outcome, at) in [(VoteOutcome::Abstain, 30), (VoteOutcome::Yes, 20)] {
            let v = vote(h, 2, 1, outcome, NOW - at);
            assert_eq!(s.add_vote(v, peer(2), NOW), Ok(()));
        }
        let t = s.tally(&h);
        assert_eq!(t.uncounted, 1);
        assert_eq!((t.no.max, t.abstain.max), (4, 4));
        // Objects not stored: the oldest lost vote goes past the bound.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_unstored_lost = 1;
        let (b, c) = (proposal("b"), proposal("c"));
        for o in [&b, &c] {
            let v = vote(o.hash(), 2, 1, VoteOutcome::No, NOW - 40);
            assert_eq!(s.add_vote(v, peer(2), NOW), Err(VoteRejection::Orphan));
        }
        s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS + 1);
        assert!(!s.uncounted.contains_key(&b.hash()));
        assert!(s.uncounted.contains_key(&c.hash()));
        // And with their object.
        s.remove_object(&h);
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        forged_no(&mut s, h);
        s.set_masternode_list(&[entry(1, 4)]);
        s.remove_object(&h);
        assert!(s.uncounted.is_empty());
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
                pro_tx_hash: [7; 32],
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
    fn a_copy_not_kept_is_remembered_until_the_list_changes() {
        // D62: the verdict on a recovered copy is kept by (hash,
        // signature), so the same bytes are not recovered again; another
        // signature on the same hash is checked afresh.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), None, NOW),
            Ok(())
        );
        let old = vote(h, 1, 1, VoteOutcome::No, 5);
        assert_eq!(
            s.add_vote(old.clone(), None, NOW),
            Err(VoteRejection::Obsolete)
        );
        assert_eq!(
            s.precheck(&old, NOW).map(|_| ()),
            Err(VoteRejection::Obsolete)
        );
        let mut other = old.clone();
        sign(&mut other, 2);
        assert_eq!(other.hash(), old.hash());
        assert!(s.precheck(&other, NOW).is_ok());
        s.set_masternode_list(&[entry(1, 1)]);
        assert!(s.precheck(&old, NOW).is_ok());
    }

    #[test]
    fn malleated_copies_of_a_vote_are_one_copy() {
        // Validator r12 MJ1: one public vote the wallet does not keep, sent
        // under the 64 signatures that recover the same listed key, forced
        // 64 recoveries; they are one copy (D63).
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), None, NOW)
            .unwrap();
        let old = vote(h, 1, 1, VoteOutcome::No, 5);
        let mut recovered = 0;
        for c in crate::vote::malleations(&old) {
            if s.precheck(&c, NOW).is_ok() {
                recovered += 1;
                assert_eq!(s.add_vote(c, peer(2), NOW), Err(VoteRejection::Obsolete));
            }
        }
        assert_eq!(
            recovered, 1,
            "one public vote forced {recovered} recoveries"
        );
    }

    #[test]
    fn resolving_an_outpoint_forgets_its_key_limit_verdict() {
        // Validator r12 m2: a resolved outpoint is not limited (D11), but its
        // remembered `KeyLimit` verdict outlived `set_resolved` (D63).
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        for op in 10..14 {
            s.add_vote(vote(h, 1, op, VoteOutcome::No, 10), None, NOW)
                .unwrap();
        }
        let mine = vote(h, 1, 1, VoteOutcome::Yes, 20);
        assert_eq!(
            s.add_vote(mine.clone(), peer(2), NOW),
            Err(VoteRejection::KeyLimit)
        );
        s.set_resolved(HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [7; 32],
                voting_key_id: key_id(1),
                weight: 1,
            },
        )]));
        assert_eq!(s.add_vote(mine, peer(2), NOW), Ok(()));
    }

    #[test]
    fn the_orphan_bound_is_checked_before_recovery() {
        let obj = proposal("a");
        let mut s = store(&obj, &[entry(1, 1)]);
        let addr = peer(1).unwrap();
        for i in 0..MAX_ORPHANS_PER_PEER {
            let _ = s.add_vote(vote(parent(i), 1, 1, VoteOutcome::Yes, 10), peer(1), NOW);
        }
        assert!(s.orphan_bound_full(&parent(u32::MAX), addr, NOW));
        assert!(!s.orphan_bound_full(&parent(u32::MAX), peer(2).unwrap(), NOW));
        // A vote on a held object is no orphan.
        assert!(!s.orphan_bound_full(&obj.hash(), addr, NOW));
        // Expired orphans free the bound.
        assert!(!s.orphan_bound_full(&parent(u32::MAX), addr, NOW + ORPHAN_EXPIRY_SECS + 1));
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
