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
//! tally (`NONE` signals or outcomes, votes on triggers, on expired
//! proposals or on ones marked deleted): those are ignored and their hashes
//! remembered (the hash covers what made them ignored), with no penalty for
//! the sender.
//!
//! For each proposal the latest funding vote of every (collateral outpoint,
//! voting key) pair is kept. Core keeps one per outpoint and signal
//! (`mapCurrentMNVotes`), but the SPV list has no outpoints: a vote counts
//! when its signature recovers a voting key that a masternode of the tip
//! list holds, PoSe-banned ones included (Core `GetMNByCollateral` finds
//! them), or, for an outpoint the wallet resolved itself, that masternode's
//! key. Keyed by the pair, a key holder who signs for another masternode's
//! outpoint adds a competing claim the tally ranges over, and cannot
//! replace or block that masternode's own votes (MG-02 D1, D19). One key
//! gets at most `2·n + 2` unresolved outpoints per object, `n` being the
//! masternodes holding it (D11); resolved outpoints are exact and not
//! limited. Funding votes the wallet holds no countable copy of widen the
//! funding tally (D64–D81).
//!
//! A proposal's `valid`, `delete` and `endorsed` votes are its masternodes'
//! operator votes: Core takes only the operator key's BLS signature for
//! them (MG-02 D89). One per (outpoint, signal) is kept, as Core keeps them
//! (D82). A vote is checked once the wallet resolved its outpoint to a
//! masternode (D80), against that masternode's operator key, and counts
//! with the masternode's weight; until then it is parked, bounded. A
//! `delete` NO parked, or one parked and dropped unchecked, keeps its
//! proposal from being marked (D85). A proposal whose `delete` votes reach
//! Core's threshold is marked at the prune pass and leaves 10 minutes later
//! (D84).
//!
//! A vote whose object has not arrived waits as an orphan once it passed the
//! checks that need no object (Core `IsValidForUnknownParent`, MG-01 D23):
//! for 10 minutes (`GOVERNANCE_ORPHAN_EXPIRATION_TIME`), at most
//! [`MAX_ORPHAN_VOTES`] in all with the oldest evicted, at most
//! [`MAX_ORPHANS_PER_PEER`] from one peer, one per (object, outpoint, key,
//! signal).
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
use crate::tally::{CurrentVote, KeyWeights, RangedTally, delete_threshold, ranged_tally};
use crate::validate::{MAX_TIME_FUTURE_DEVIATION, ObjectRejection, ValidObject, check_object};
use crate::vote::{
    BLS_SIG_SIZE, COMPACT_SIG_SIZE, GovernanceVote, SIGNAL_FUNDING, SentinelSignal, VoteOutcome,
    canonical_compact,
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
/// Lost votes kept for objects the store does not hold: past twice this,
/// stale entries go and the oldest down to this (D76). The object's own
/// vote sync asks for its votes when it arrives.
pub const MAX_UNSTORED_LOST: usize = 10_000;
/// Signal and outcome ranges Core accepts (`VOTE_SIGNAL_UNKNOWN`,
/// `VOTE_OUTCOME_UNKNOWN` exclusive).
const MAX_SIGNAL: i32 = 4;
const MAX_OUTCOME: i32 = 3;
/// Operator votes waiting for their outpoint to resolve, or for a new
/// operator key, in all and from one peer; the oldest is evicted (D89).
pub const MAX_PARKED_VOTES: usize = 10_000;
pub const MAX_PARKED_PER_PEER: u32 = 2_000;

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
    /// A vote Core relays that the wallet does not tally: a `NONE` signal or
    /// outcome, or a vote on a trigger, an expired proposal or one marked
    /// deleted. Remembered.
    Ignored,
    /// A signature the wallet cannot take this copy on: an operator
    /// signature for an object that has not arrived, a voting-key one on a
    /// proposal's other signal (Core takes only the operator's, D89), or an
    /// operator vote that fails under its masternode's key. Dropped, not
    /// remembered (the hash leaves the signature out): another announcer's
    /// copy may be good.
    Unverifiable,
    /// A signal or outcome outside Core's ranges, or a signature of neither
    /// size; a BLS signature on a proposal's funding vote.
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
    /// An operator vote held until its outpoint resolves or its masternode's
    /// operator key changes (D89).
    Parked,
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
    /// The list's operator public key (48 bytes as the list carries it).
    pub operator_key: [u8; 48],
    pub weight: u32,
}

/// A proposal's `valid`, `delete` or `endorsed` vote, BLS-signed by an
/// operator (D89), as the caller checked it: `checked` is the operator key
/// [`GovernanceStore::operator_key`] gave and whether the signature
/// verified under it ([`GovernanceVote::verify_operator`], run without the
/// store's lock), `None` when the outpoint was not resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorVote {
    pub vote: GovernanceVote,
    pub outcome: VoteOutcome,
    pub checked: Option<([u8; 48], bool)>,
    /// Taken as held by the sync when it was parked: no other announcer's
    /// copy was asked for (D89).
    pub held: bool,
}

impl OperatorVote {
    /// `vote`, not held, checked against `operator_key`, its outpoint's
    /// ([`GovernanceStore::operator_key`]), if resolved.
    pub fn checked(
        vote: GovernanceVote,
        outcome: VoteOutcome,
        operator_key: Option<[u8; 48]>,
    ) -> Self {
        let checked = operator_key.map(|k| (k, vote.verify_operator(&k)));
        Self {
            vote,
            outcome,
            checked,
            held: false,
        }
    }
}

/// An operator vote waiting to be checked (D89).
#[derive(Debug, Clone)]
struct Parked {
    vote: GovernanceVote,
    outcome: VoteOutcome,
    /// The operator key it failed under; `None` while its outpoint is
    /// unresolved.
    failed_under: Option<[u8; 48]>,
    held: bool,
    from: Option<SocketAddr>,
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

    /// Whether a record stands for a vote of a valid masternode's key: an
    /// awaited copy's masternode's, or a removed copy's when it went. (A
    /// refused vote's key holds stored votes on the proposal.)
    fn by_valid_key(&self, valid_keys: &HashSet<[u8; 20]>) -> bool {
        self.awaited
            .values()
            .flatten()
            .any(|a| valid_keys.contains(&a.key))
            || self.lost.values().flatten().any(|l| l.valid)
    }

    /// Drops the lost votes at `outpoint` older than `time`: its
    /// masternode's newer vote supersedes them, as in Core (DEC-257, D77).
    fn supersede_lost(&mut self, outpoint: &OutPoint, time: i64) {
        let Some(list) = self.lost.get_mut(outpoint) else {
            return;
        };
        let before = list.len();
        list.retain(|l| l.time >= time);
        self.held -= before - list.len();
        if list.is_empty() {
            self.lost.remove(outpoint);
        }
    }

    /// Drops the lost vote of `hash` at `outpoint`; whether it was held.
    fn drop_lost(&mut self, outpoint: &OutPoint, hash: &[u8; 32]) -> bool {
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
    time: i64,
    /// Whether the key that signed the removed copy was a valid
    /// masternode's when it went: only then does the record keep its
    /// object from eviction, as the copy did (D47).
    valid: bool,
}

#[derive(Debug, Clone)]
struct Orphan {
    vote: CheckedVote,
    from: Option<SocketAddr>,
    at: i64,
}

/// (object, outpoint, key): the unit of a current vote.
type Slot = ([u8; 32], OutPoint, [u8; 20]);
/// (object, outpoint, key, signal): where an orphan waits.
type OrphanSlot = ([u8; 32], OutPoint, [u8; 20], i32);
/// (outpoint, key) → current funding vote, for one object.
type ObjectVotes = HashMap<(OutPoint, [u8; 20]), CurrentVote>;
/// (signal, outpoint) → current operator vote of another signal, for one
/// object (D82, D89).
type SignalSlot = (SentinelSignal, OutPoint);
type SignalVotes = HashMap<SignalSlot, CurrentVote>;

/// Core's order of two votes in one slot: an older one is obsolete, and of
/// two of the same time the higher outcome stays.
fn replaces(
    time: i64,
    outcome: VoteOutcome,
    current_time: i64,
    current_outcome: VoteOutcome,
) -> Result<(), VoteRejection> {
    match time.cmp(&current_time) {
        std::cmp::Ordering::Less => Err(VoteRejection::Obsolete),
        std::cmp::Ordering::Equal if outcome.code() <= current_outcome.code() => {
            Err(VoteRejection::SameTimestamp)
        }
        _ => Ok(()),
    }
}

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
    /// Where the next [`Self::requeue_lost`] starts in hash order: the
    /// first hash a run left unasked, so each gets a turn (D79).
    refetch_from: Option<[u8; 32]>,
    /// Whether this pass's first unasked hash is already noted.
    refetch_deferred: bool,
    /// Object → its votes held as uncertainty (D71, D72, D76).
    uncounted: HashMap<[u8; 32], Uncounted>,
    /// (object, outpoint, hash) of lost votes noted while the object was
    /// not stored, oldest first; entries gone stale are skipped (D76).
    unstored_lost: VecDeque<([u8; 32], OutPoint, [u8; 32])>,
    /// Lapsed proposals, stored or refused.
    expired: HashMap<[u8; 32], ()>,
    expired_order: VecDeque<[u8; 32]>,
    /// object hash → (outpoint, key) → current funding vote.
    votes: HashMap<[u8; 32], ObjectVotes>,
    /// (object, key) → unresolved outpoints voting with the key.
    per_key: HashMap<([u8; 32], [u8; 20]), u32>,
    /// object hash → its operator votes of the other signals (D82, D89).
    signal_votes: HashMap<[u8; 32], SignalVotes>,
    /// Operator votes waiting to be checked, by copy (hash and signature),
    /// oldest first (D89).
    parked: HashMap<[u8; 32], Parked>,
    parked_order: VecDeque<[u8; 32]>,
    max_parked: usize,
    parked_per_peer: HashMap<SocketAddr, u32>,
    /// Proposal → outpoints whose `delete` NO the sync held but the wallet
    /// could not check → the NO's time (D85).
    doubts: HashMap<[u8; 32], HashMap<OutPoint, i64>>,
    /// Proposals whose votes a completed sync covered (D84).
    votes_synced: HashSet<[u8; 32]>,
    /// Proposals marked deleted by vote → when (D84).
    deleted: HashMap<[u8; 32], i64>,
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
    orphan_slots: HashMap<OrphanSlot, [u8; 32]>,
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
            refetch_from: None,
            refetch_deferred: false,
            uncounted: HashMap::new(),
            unstored_lost: VecDeque::new(),
            expired: HashMap::new(),
            expired_order: VecDeque::new(),
            votes: HashMap::new(),
            per_key: HashMap::new(),
            signal_votes: HashMap::new(),
            parked: HashMap::new(),
            parked_order: VecDeque::new(),
            max_parked: MAX_PARKED_VOTES,
            parked_per_peer: HashMap::new(),
            doubts: HashMap::new(),
            votes_synced: HashSet::new(),
            deleted: HashMap::new(),
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

    /// Starts a sync run: no proposal is marked deleted until this run's
    /// vote sync covered it, as votes cast while no run synced may be
    /// unseen (D84).
    pub fn begin_vote_sync(&mut self) {
        self.votes_synced.clear();
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

    /// Notes that a sync completed, having synced the votes of `proposals`:
    /// from now on triggers are checked against their creation-time window,
    /// as Core does once synced, and those proposals may be marked deleted
    /// (D84).
    pub fn set_synced(&mut self, proposals: &[[u8; 32]]) {
        self.synced = true;
        self.votes_synced.extend(
            proposals
                .iter()
                .filter(|h| self.objects.contains_key(*h))
                .copied(),
        );
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

    /// The current votes of the other signals on `hash` (D82).
    pub fn signal_votes_on(
        &self,
        hash: &[u8; 32],
    ) -> impl Iterator<Item = (SentinelSignal, &CurrentVote)> {
        self.signal_votes
            .get(hash)
            .into_iter()
            .flat_map(|m| m.iter().map(|((signal, _), v)| (*signal, v)))
    }

    /// `hash`'s tally of `signal` (D82): its operator votes, each counted
    /// with its resolved masternode's weight, so the range is exact (D89).
    pub fn signal_tally(&self, hash: &[u8; 32], signal: SentinelSignal) -> RangedTally {
        ranged_tally(
            self.signal_votes_on(hash)
                .filter(|(s, _)| *s == signal)
                .map(|(_, v)| v),
            |k| self.key_weights(k),
            |op| self.resolved.get(op).map(|r| r.weight),
        )
    }

    /// When `hash` was marked deleted by vote (D84).
    pub fn deletion_mark(&self, hash: &[u8; 32]) -> Option<i64> {
        self.deleted.get(hash).copied()
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
        // Votes on a trigger are ignored: none is uncertain, and a
        // remembered `KeyLimit` verdict goes as with a removed object (D75,
        // D76).
        if matches!(self.objects[&hash].kind, ObjectKind::Trigger(_))
            && self
                .uncounted
                .remove(&hash)
                .is_some_and(|u| !u.refused.is_empty())
        {
            self.forget_dismissed();
        }
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
    /// (Core's tally counts no banned masternode), or such a key's vote on
    /// it is held as uncertainty, which evicting it would forget (D72,
    /// D76). A trigger holds none: its records go when it is stored.
    fn weighted(&self, hash: &[u8; 32]) -> bool {
        // Evicting a marked proposal would forget the mark (D84); its
        // operator votes count as Core counts them (D89).
        self.deleted.contains_key(hash)
            || self.signal_votes.get(hash).is_some_and(|m| !m.is_empty())
            || self
                .votes
                .get(hash)
                .is_some_and(|m| m.values().any(|v| self.valid_keys.contains(&v.key_id)))
            || self
                .uncounted
                .get(hash)
                .is_some_and(|u| u.by_valid_key(&self.valid_keys))
    }

    /// Marks deleted the proposals Core would (D84), drops the proposals
    /// lapsed by `now` (D65) or marked [`DELETION_DELAY_SECS`] ago, erases
    /// the triggers Core's cleanup would by the tip, drops objects
    /// unconfirmed for [`UNCONFIRMED_TTL_SECS`] and orphans past their
    /// expiry, and forgets erased hashes whose time passed.
    pub fn prune(&mut self, now: i64) {
        self.mark_deleted(now);
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
                ObjectKind::Proposal(d) => {
                    d.end_epoch.is_some_and(|e| e <= now)
                        || self
                            .deleted
                            .get(hash)
                            .is_some_and(|t| t.saturating_add(DELETION_DELAY_SECS) <= now)
                }
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

    /// Marks deleted the proposals whose votes this run's completed sync
    /// covered and whose `delete` signal's net yes reaches Core's
    /// `nAbsDeleteReq`, with no `delete` NO doubted or held parked
    /// (`UpdateSentinelVariables`, D84, D85). A mark keeps its first time (`MarkForDeletion`).
    fn mark_deleted(&mut self, now: i64) {
        // Core leaves the flags as they are with no valid weight.
        if self.valid_weight == 0 {
            return;
        }
        let need = i64::from(delete_threshold(
            self.valid_weight,
            governance_params(self.network).min_quorum,
        ));
        // A `delete` NO held parked may be its masternode's (D85).
        let waiting: HashSet<[u8; 32]> = self
            .parked
            .values()
            .filter(|p| {
                p.held
                    && p.vote.signal == SentinelSignal::Delete.code()
                    && p.outcome == VoteOutcome::No
            })
            .map(|p| p.vote.parent_hash)
            .collect();
        let marked: Vec<[u8; 32]> = self
            .signal_votes
            .keys()
            .filter(|h| {
                self.votes_synced.contains(*h)
                    && !self.deleted.contains_key(*h)
                    && !self.doubts.contains_key(*h)
                    && !waiting.contains(*h)
                    && matches!(
                        self.object(h).map(|o| &o.kind),
                        Some(ObjectKind::Proposal(_))
                    )
            })
            .filter(|h| {
                self.signal_tally(h, SentinelSignal::Delete)
                    .net_yes_bounds()
                    .0
                    >= need
            })
            .copied()
            .collect();
        for h in marked {
            self.deleted.insert(h, now);
        }
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
        for (_, v) in self.signal_votes.remove(hash).unwrap_or_default() {
            self.seen_votes.remove(&v.vote_hash);
            self.vote_count -= 1;
        }
        self.doubts.remove(hash);
        let parked: Vec<[u8; 32]> = self
            .parked
            .iter()
            .filter(|(_, p)| &p.vote.parent_hash == hash)
            .map(|(c, _)| *c)
            .collect();
        for c in parked {
            self.unpark(&c);
        }
        self.votes_synced.remove(hash);
        self.deleted.remove(hash);
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
        self.orphan_slots.remove(&(
            v.parent_hash,
            v.masternode_outpoint,
            o.vote.key_id,
            v.signal,
        ));
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

    /// Drops an orphan that was not placed: a funding vote's hash stays
    /// uncertain on its object (D76). Its object unknown, another signal's
    /// is left to the object's vote sync (D85).
    fn lose_orphan(&mut self, hash: &[u8; 32]) {
        if let Some(o) = self.remove_orphan(hash)
            && o.vote.vote.signal == SIGNAL_FUNDING
        {
            let v = &o.vote.vote;
            self.note_lost(
                v.parent_hash,
                v.masternode_outpoint,
                o.vote.key_id,
                o.vote.hash,
                o.vote.outcome,
                v.time,
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
        self.valid_weight = valid_weight;
        self.list_size = entries.len() as u32;
        // A removed copy's key counts as valid by the list it was stored
        // under (D76).
        self.drop_unbacked_votes();
        self.valid_keys = valid_keys;
        self.place_suspects();
    }

    /// Sets the outpoints the wallet resolved to masternodes (MG-03); their
    /// votes are checked against that masternode's key and counted exactly.
    pub fn set_resolved(&mut self, resolved: HashMap<OutPoint, ResolvedMasternode>) {
        // A resolved outpoint is not limited (D11): its verdicts may change.
        self.forget_dismissed();
        let previous = std::mem::replace(&mut self.resolved, resolved);
        // A masternode whose voting key changed (ProUpRegTx): Core drops
        // the votes its old key signed (`RemoveInvalidVotes`), and they were
        // checked against that key, so they are no copies to fetch again.
        let rekeyed: HashSet<(OutPoint, [u8; 20])> = previous
            .iter()
            .filter(|(o, old)| {
                self.resolved
                    .get(*o)
                    .is_some_and(|new| new.voting_key_id != old.voting_key_id)
            })
            .map(|(o, old)| (*o, old.voting_key_id))
            .collect();
        // A copy re-signed by another key has the honest vote's hash, so it
        // kept the honest copy out: drop it and fetch the hash again. Until
        // the honest copy lands, the vote stays uncertain (D72).
        for (object, m) in &self.votes {
            for ((outpoint, key), v) in m {
                let Some(r) = self.resolved.get(outpoint) else {
                    continue;
                };
                if &r.voting_key_id == key || rekeyed.contains(&(*outpoint, *key)) {
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
        self.drop_changed_operator_votes(&previous);
        // Lost votes its masternode's stored vote is newer than (D77).
        for (object, u) in &mut self.uncounted {
            let outpoints: Vec<OutPoint> = u.lost.keys().copied().collect();
            for outpoint in outpoints {
                let newest = self.resolved.get(&outpoint).and_then(|r| {
                    self.votes
                        .get(object)
                        .and_then(|m| m.get(&(outpoint, r.voting_key_id)))
                });
                if let Some(v) = newest {
                    u.supersede_lost(&outpoint, v.time);
                }
            }
        }
        self.uncounted.retain(|_, u| !u.is_empty());
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
                self.note_lost(slot.0, v.outpoint, slot.2, v.vote_hash, v.outcome, v.time);
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

    /// Drops the stored operator votes of outpoints whose masternode left
    /// the list or whose operator key changed, as Core drops them
    /// (`ClearMasternodeVotes`, `RemoveInvalidVotes`); a parked one is tried
    /// under the new key. The parked votes and doubts of an outpoint that is
    /// no masternode's go too: one whose masternode left, and, with every
    /// listed masternode resolved, any outpoint not resolved (Core: unknown
    /// masternode) (D89).
    fn drop_changed_operator_votes(&mut self, previous: &HashMap<OutPoint, ResolvedMasternode>) {
        let all_resolved = self.all_resolved();
        let resolved = &self.resolved;
        let changed = |o: &OutPoint| {
            previous.get(o).is_some_and(|old| {
                resolved
                    .get(o)
                    .is_none_or(|new| new.operator_key != old.operator_key)
            })
        };
        let dropped: Vec<([u8; 32], SignalSlot)> = self
            .signal_votes
            .iter()
            .flat_map(|(h, m)| {
                m.keys()
                    .filter(|(_, o)| changed(o))
                    .map(move |slot| (*h, *slot))
            })
            .collect();
        let known =
            |o: &OutPoint| resolved.contains_key(o) || !(all_resolved || previous.contains_key(o));
        self.doubts.retain(|_, d| {
            d.retain(|o, _| known(o));
            !d.is_empty()
        });
        let stale: Vec<[u8; 32]> = self
            .parked
            .iter()
            .filter(|(_, p)| !known(&p.vote.masternode_outpoint))
            .map(|(c, _)| *c)
            .collect();
        for (h, slot) in dropped {
            self.remove_signal_vote(&h, &slot);
        }
        for c in stale {
            self.unpark(&c);
        }
    }

    /// Keeps a removed copy's vote, of `key` at `time`, as uncertainty on
    /// its object until a copy of it is stored, its masternode's newer vote
    /// is, or the object goes (D76, D77), and asks for it again if the
    /// object is stored.
    fn note_lost(
        &mut self,
        parent: [u8; 32],
        outpoint: OutPoint,
        key: [u8; 20],
        hash: [u8; 32],
        outcome: VoteOutcome,
        time: i64,
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
            // An object not stored is vote-synced when it arrives; past the
            // bound, its lost votes are left to that sync.
            if self.objects.contains_key(&parent) {
                u.lost_past.insert(outcome);
            }
            return;
        }
        u.lost.entry(outpoint).or_default().push(Lost {
            hash,
            outcome,
            time,
            valid: self.valid_keys.contains(&key),
        });
        u.held += 1;
        if self.objects.contains_key(&parent) {
            self.refetch.push(hash);
            return;
        }
        self.unstored_lost.push_back((parent, outpoint, hash));
        // Past twice the bound, stale entries go, then the oldest live ones
        // down to the bound.
        let max = self.max_unstored_lost;
        if self.unstored_lost.len() > 2 * max {
            let live: VecDeque<_> = std::mem::take(&mut self.unstored_lost)
                .into_iter()
                .filter(|(p, op, h)| self.unstored_lost_live(p, op, h))
                .collect();
            self.unstored_lost = live;
            while self.unstored_lost.len() > max {
                let Some((p, op, h)) = self.unstored_lost.pop_front() else {
                    break;
                };
                self.forget_lost(&p, &op, &h);
            }
        }
    }

    /// Whether an `unstored_lost` entry still stands for a held vote of an
    /// object the store does not hold.
    fn unstored_lost_live(&self, parent: &[u8; 32], outpoint: &OutPoint, hash: &[u8; 32]) -> bool {
        !self.objects.contains_key(parent)
            && self
                .uncounted
                .get(parent)
                .and_then(|u| u.lost.get(outpoint))
                .is_some_and(|l| l.iter().any(|l| &l.hash == hash))
    }

    /// Drops the held lost vote `hash` of `parent` at `outpoint`.
    fn forget_lost(&mut self, parent: &[u8; 32], outpoint: &OutPoint, hash: &[u8; 32]) {
        if let Some(u) = self.uncounted.get_mut(parent) {
            u.drop_lost(outpoint, hash);
            if u.is_empty() {
                self.uncounted.remove(parent);
            }
        }
    }

    /// Asks again for the awaited and lost votes of a stored `object` (D72,
    /// D76).
    fn refetch_lost(&mut self, object: &[u8; 32]) {
        if let Some(u) = self.uncounted.get(object) {
            let awaited = u.awaited.values().flatten().map(|a| a.hash);
            let lost = u.lost.values().flatten().map(|l| l.hash);
            self.refetch.extend(awaited.chain(lost));
        }
    }

    /// Asks again for every awaited and lost vote of a stored object: a
    /// run's peers may have it where the last run's had not, or the last
    /// run's budget left it (D76, D77). In hash order from where the last
    /// pass left off, so a run's budget cannot keep a hash waiting past
    /// a turn of the whole set (D79).
    pub fn requeue_lost(&mut self) {
        let mut ring: Vec<[u8; 32]> = self
            .uncounted
            .iter()
            .filter(|(h, _)| self.objects.contains_key(*h))
            .flat_map(|(_, u)| {
                let awaited = u.awaited.values().flatten().map(|a| a.hash);
                awaited.chain(u.lost.values().flatten().map(|l| l.hash))
            })
            .collect();
        ring.sort_unstable();
        ring.dedup();
        if let Some(from) = self.refetch_from {
            let start = ring.partition_point(|h| *h < from);
            ring.rotate_left(start);
        }
        self.refetch = ring;
        self.refetch_deferred = false;
    }

    /// Notes `hash` as the first a run left unasked, its budget spent or no
    /// peer connected: the next [`Self::requeue_lost`] starts there (D79).
    pub fn defer_refetch(&mut self, hash: [u8; 32]) {
        if !self.refetch_deferred {
            self.refetch_from = Some(hash);
            self.refetch_deferred = true;
        }
    }

    /// Lost votes `ids` on `parent`, of invented hashes; their hashes.
    #[cfg(test)]
    pub(crate) fn lose_for_tests(
        &mut self,
        parent: [u8; 32],
        ids: std::ops::Range<u32>,
    ) -> Vec<[u8; 32]> {
        ids.map(|i| {
            let mut hash = sha256::Hash::hash(&parent).to_byte_array();
            hash[..4].copy_from_slice(&i.to_le_bytes());
            let outpoint = OutPoint::new(dashcore::Txid::from_byte_array(hash), i);
            self.note_lost(parent, outpoint, [0; 20], hash, VoteOutcome::No, 0);
            hash
        })
        .collect()
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
        // Any copy of a lost vote, stored, counts for it (D64, D76); its
        // masternode's newer vote supersedes it. An unresolved outpoint's
        // masternode key is not known, so no other vote does (D77).
        u.drop_lost(&v.outpoint, &v.vote_hash);
        if self
            .resolved
            .get(&v.outpoint)
            .is_some_and(|r| r.voting_key_id == v.key_id)
        {
            u.supersede_lost(&v.outpoint, v.time);
        }
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

    fn remove_signal_vote(&mut self, object: &[u8; 32], slot: &SignalSlot) -> Option<CurrentVote> {
        let v = self
            .signal_votes
            .get_mut(object)
            .and_then(|m| m.remove(slot))?;
        self.seen_votes.remove(&v.vote_hash);
        self.vote_count -= 1;
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
        // So are a deleted one's, as Core ignores them (D84).
        if self.erased.contains_key(hash)
            || self.expired.contains_key(hash)
            || self.refused.contains_key(hash)
            || self.deleted.contains_key(hash)
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
        // Core refuses a `NONE` signal (`ProcessVote`, a warning).
        if matches!(parent, Parent::Trigger | Parent::Expired)
            || vote.signal == 0
            || outcome.is_none()
        {
            self.remember_ignored(hash, vote.parent_hash);
            return Err(VoteRejection::Ignored);
        }
        let operator_signal = SentinelSignal::from_code(vote.signal).is_some();
        let bls = vote.signature.len() == BLS_SIG_SIZE;
        match (&parent, operator_signal, bls) {
            // A proposal's other signals take only its masternode's operator
            // signature (`onlyVotingKeyAllowed`, D89), checked against the
            // operator key once the outpoint is resolved.
            (Parent::Proposal, true, true) => {}
            // Its funding votes take only the voting key's.
            (Parent::Proposal, false, true) => return Err(VoteRejection::Malformed),
            // A funding vote, signed by the voting key.
            (_, false, false) => {}
            // A voting-key signature Core refuses for another signal, and
            // relays none: another announcer's copy may be the operator's
            // (D89). Not remembered: the hash leaves the signature out. Or
            // its object has not arrived: an operator signature waits for
            // the object's vote sync, which fetches it again (D58, D89).
            _ => return Err(VoteRejection::Unverifiable),
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
        if SentinelSignal::from_code(vote.signal).is_some() {
            let key = self.operator_key(&vote.masternode_outpoint);
            let v = OperatorVote::checked(vote, outcome, key);
            return self.add_operator_vote(v, from, now);
        }
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
        let current = self
            .votes
            .get(&parent)
            .and_then(|m| m.get(&(outpoint, key)));
        self.check_slot(&v, current, rate_checked, now)?;
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
            self.note_lost(parent, outpoint, key, c.vote_hash, c.outcome, c.time);
        }
        Ok(())
    }

    /// Core's checks of `v` against its slot's current vote and the one
    /// waiting there for the object: an older vote is obsolete, of two of
    /// the same time Core keeps the higher outcome, and the wallet's own
    /// may not replace one less than an hour old.
    fn check_slot(
        &self,
        v: &CheckedVote,
        current: Option<&CurrentVote>,
        rate_checked: bool,
        now: i64,
    ) -> Result<(), VoteRejection> {
        let replaces = |t: i64, o: VoteOutcome| replaces(v.vote.time, v.outcome, t, o);
        if let Some(c) = current {
            replaces(c.time, c.outcome)?;
            // Core's last update is when the node accepted the current
            // vote; its own time stands in for it (D37).
            if rate_checked && c.time > now - VOTE_UPDATE_MIN_SECS {
                return Err(VoteRejection::TooOften);
            }
        }
        let vote = &v.vote;
        let slot = (
            vote.parent_hash,
            vote.masternode_outpoint,
            v.key_id,
            vote.signal,
        );
        if let Some(w) = self
            .orphan_slots
            .get(&slot)
            .and_then(|h| self.orphans.get(h))
        {
            replaces(w.vote.vote.time, w.vote.outcome)?;
        }
        Ok(())
    }

    /// The operator key to check `outpoint`'s operator votes against, once
    /// the wallet resolved it (D89).
    pub fn operator_key(&self, outpoint: &OutPoint) -> Option<[u8; 48]> {
        self.resolved.get(outpoint).map(|r| r.operator_key)
    }

    /// Stores an operator vote the caller checked (D89) as its (outpoint,
    /// signal)'s current vote, with Core's checks against that slot. One
    /// whose outpoint is not resolved, or that was checked under another
    /// key than its masternode's now, is parked and held: `Parked`. One that
    /// failed under its masternode's key is parked until that key changes
    /// (the wallet's list may lag the sender's) but not held:
    /// `Unverifiable`, so another announcer's copy is asked for. A `delete`
    /// NO held parked that fails is a NO the wallet misses: a doubt on its
    /// proposal (D85), asked for again.
    pub fn add_operator_vote(
        &mut self,
        v: OperatorVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> Result<(), VoteRejection> {
        let outcome = self.precheck(&v.vote, now)?;
        let Some(signal) = SentinelSignal::from_code(v.vote.signal) else {
            return Err(VoteRejection::Malformed);
        };
        let outpoint = v.vote.masternode_outpoint;
        let resolved = self.resolved.get(&outpoint).copied();
        match (resolved, v.checked) {
            (Some(r), Some((key, true))) if key == r.operator_key => {
                self.place_signal(&v.vote, outcome, signal, &r)
            }
            (Some(r), Some((key, false))) if key == r.operator_key => {
                if v.held {
                    self.lose_held(&v.vote, outcome);
                }
                self.park(v, Some(key), from);
                Err(VoteRejection::Unverifiable)
            }
            // With every listed masternode resolved, an unknown outpoint
            // is no masternode's (Core: unknown masternode).
            (None, _) if self.all_resolved() => Err(VoteRejection::Unverifiable),
            _ => {
                let (vote, outcome) = (v.vote.clone(), v.outcome);
                if self.park(v, None, from) {
                    Err(VoteRejection::Parked)
                } else {
                    // Its sender's bound is full: perhaps its only
                    // announcer's copy, lost as one held that fails.
                    self.lose_held(&vote, outcome);
                    Err(VoteRejection::Unverifiable)
                }
            }
        }
    }

    /// An operator vote the sync took as held (no other announcer's copy
    /// asked for) but the wallet could not keep: asked for again, and a
    /// `delete` NO is a doubt on its proposal until then (D85).
    fn lose_held(&mut self, vote: &GovernanceVote, outcome: VoteOutcome) {
        if vote.signal == SentinelSignal::Delete.code() && outcome == VoteOutcome::No {
            self.doubt(vote.parent_hash, vote.masternode_outpoint, vote.time);
        }
        self.refetch.push(vote.hash());
    }

    /// Whether every masternode of the list is resolved.
    fn all_resolved(&self) -> bool {
        self.list_size > 0 && self.resolved.len() >= self.list_size as usize
    }

    /// Up to `max` parked votes, oldest first, whose outpoint now has an
    /// operator key they were not tried under, taken out: the caller checks
    /// each against [`Self::operator_key`] and hands it back to
    /// [`Self::add_operator_vote`] with the sender it came from (D89).
    pub fn take_parked_ready(&mut self, max: usize) -> Vec<(OperatorVote, Option<SocketAddr>)> {
        let ready: Vec<[u8; 32]> = self
            .parked_order
            .iter()
            .filter(|c| {
                self.parked.get(*c).is_some_and(|p| {
                    self.resolved
                        .get(&p.vote.masternode_outpoint)
                        .is_some_and(|r| p.failed_under != Some(r.operator_key))
                })
            })
            .take(max)
            .copied()
            .collect();
        ready
            .iter()
            .filter_map(|c| self.unpark(c))
            .map(|p| {
                let v = OperatorVote {
                    vote: p.vote,
                    outcome: p.outcome,
                    checked: None,
                    held: p.held,
                };
                (v, p.from)
            })
            .collect()
    }

    /// Parks an operator vote, held unless it `failed_under` its
    /// masternode's key; `false` when its sender's bound is full. The oldest
    /// goes past [`MAX_PARKED_VOTES`]: one held so is lost (D85).
    fn park(
        &mut self,
        v: OperatorVote,
        failed_under: Option<[u8; 48]>,
        from: Option<SocketAddr>,
    ) -> bool {
        let held = failed_under.is_none();
        let copy = copy_digest(&v.vote.hash(), &v.vote.signature);
        if let Some(p) = self.parked.get_mut(&copy) {
            p.failed_under = failed_under;
            p.held |= held;
            return true;
        }
        if let Some(peer) = from
            && self.parked_per_peer.get(&peer).copied().unwrap_or(0) >= MAX_PARKED_PER_PEER
        {
            return false;
        }
        while self.parked.len() >= self.max_parked {
            let Some(old) = self.parked_order.pop_front() else {
                break;
            };
            if let Some(p) = self.unpark(&old)
                && p.held
            {
                self.lose_held(&p.vote, p.outcome);
            }
        }
        if let Some(peer) = from {
            *self.parked_per_peer.entry(peer).or_default() += 1;
        }
        self.parked.insert(
            copy,
            Parked {
                vote: v.vote,
                outcome: v.outcome,
                failed_under,
                held,
                from,
            },
        );
        self.parked_order.push_back(copy);
        if self.parked_order.len() > 2 * self.parked.len() + 1024 {
            let parked = &self.parked;
            self.parked_order.retain(|c| parked.contains_key(c));
        }
        true
    }

    fn unpark(&mut self, copy: &[u8; 32]) -> Option<Parked> {
        let p = self.parked.remove(copy)?;
        if let Some(peer) = p.from
            && let Some(n) = self.parked_per_peer.get_mut(&peer)
        {
            *n -= 1;
            if *n == 0 {
                self.parked_per_peer.remove(&peer);
            }
        }
        Some(p)
    }

    /// Notes a `delete` NO of `outpoint` on `object`, timed `time`, that the
    /// sync held but the wallet could not check: the proposal is not marked
    /// until a checked `delete` vote of that outpoint as new is stored, or
    /// the outpoint is no masternode's (D85).
    fn doubt(&mut self, object: [u8; 32], outpoint: OutPoint, time: i64) {
        if self.objects.contains_key(&object) {
            let t = self
                .doubts
                .entry(object)
                .or_default()
                .entry(outpoint)
                .or_insert(time);
            *t = (*t).max(time);
        }
    }

    /// Stores a checked operator vote in its (outpoint, signal) slot, with
    /// Core's order against the current one (D82).
    fn place_signal(
        &mut self,
        vote: &GovernanceVote,
        outcome: VoteOutcome,
        signal: SentinelSignal,
        r: &ResolvedMasternode,
    ) -> Result<(), VoteRejection> {
        let (parent, outpoint) = (vote.parent_hash, vote.masternode_outpoint);
        let slot = (signal, outpoint);
        if let Some(c) = self.signal_votes.get(&parent).and_then(|m| m.get(&slot)) {
            replaces(vote.time, outcome, c.time, c.outcome)?;
        }
        self.remove_signal_vote(&parent, &slot);
        let hash = vote.hash();
        let stored = CurrentVote {
            outpoint,
            outcome,
            time: vote.time,
            key_id: r.voting_key_id,
            vote_hash: hash,
        };
        self.signal_votes
            .entry(parent)
            .or_default()
            .insert(slot, stored);
        self.vote_count += 1;
        self.seen_votes.insert(hash, parent);
        // The doubted NO is this vote or one it replaces.
        if signal == SentinelSignal::Delete
            && let Some(d) = self.doubts.get_mut(&parent)
        {
            d.retain(|o, t| *o != outpoint || *t > vote.time);
            if d.is_empty() {
                self.doubts.remove(&parent);
            }
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
        let slot = (parent, v.vote.masternode_outpoint, v.key_id, v.vote.signal);
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

    /// Hashes of the confirmed proposals whose end epoch is after `now`,
    /// not marked deleted (the ones whose votes the sync downloads; Core's
    /// `FetchGovernanceObjectVotes` skips deleted ones, D84).
    pub fn current_proposals(&self, now: i64) -> Vec<[u8; 32]> {
        let mut out: Vec<[u8; 32]> = self
            .proposals()
            .map(|(o, _)| o.object.hash())
            .filter(|h| self.is_current_proposal(h, now))
            .collect();
        out.sort();
        out
    }

    /// Whether `hash` is a confirmed proposal whose end epoch is after
    /// `now`, not marked deleted.
    pub fn is_current_proposal(&self, hash: &[u8; 32], now: i64) -> bool {
        !self.deleted.contains_key(hash)
            && matches!(
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
        // No answer settles them; they go with their proposal (D77).
        s.remove_object(&h);
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
            operator_key: [0; 48],
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
                operator_key: [0; 48],
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

    /// BL-229 (D80): a resolved masternode's honest vote, then a ProUpRegTx
    /// gives it a new voting key. Core drops the votes its old key signed
    /// (`RemoveInvalidVotes`) and counts nothing for it; the old vote is no
    /// wrong-key copy, so nothing is awaited or fetched again.
    #[test]
    fn a_new_voting_key_drops_the_old_keys_vote_without_a_refetch() {
        let obj = proposal("a");
        let h = obj.hash();
        let resolved = |key: u8| {
            HashMap::from([(
                outpoint(1),
                ResolvedMasternode {
                    pro_tx_hash: [1; 32],
                    voting_key_id: key_id(key),
                    operator_key: [0; 48],
                    weight: 4,
                },
            )])
        };
        // Key 1 stays listed (another masternode holds it), so only the
        // resolution says the vote is no longer the masternode's.
        let mut s = store(&obj, &[entry(1, 4), entry(1, 4), entry(3, 4)]);
        s.set_resolved(resolved(1));
        let honest = vote(h, 1, 1, VoteOutcome::No, NOW - 40);
        assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(()));
        assert_eq!(s.exact_weight(&h, &outpoint(1)), Some(4));
        assert!(s.take_refetch().is_empty());

        s.set_resolved(resolved(3));
        assert_eq!(s.tally(&h).uncounted, 0);
        assert!(s.take_refetch().is_empty());
        // Exact: the masternode is known to count nothing.
        assert_eq!(s.exact_weight(&h, &outpoint(1)), Some(4));
        assert_eq!((s.tally(&h).no.min, s.tally(&h).no.max), (0, 0));
    }

    #[test]
    fn resolving_a_masternode_takes_its_vote_off_the_key_cap() {
        // BL-229: `set_resolved` recounts the key cap (D76) over the votes
        // still unresolved; key 1 holds two masternodes, so six of them.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(1, 4), entry(2, 4)]);
        let add = |s: &mut GovernanceStore, op: u8| {
            s.add_vote(vote(h, 1, op, VoteOutcome::Yes, NOW - 40), peer(1), NOW)
        };
        for op in 1..=5 {
            assert_eq!(add(&mut s, op), Ok(()));
        }
        s.set_resolved(HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [1; 32],
                voting_key_id: key_id(1),
                operator_key: [0; 48],
                weight: 4,
            },
        )]));
        assert_eq!(add(&mut s, 6), Ok(()));
        assert_eq!(add(&mut s, 7), Ok(()));
        assert_eq!(add(&mut s, 8), Err(VoteRejection::KeyLimit));
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

    #[derive(Clone, Copy, Debug)]
    enum Settle {
        Copy,
        ResolvedThenNewer,
        NewerThenResolved,
    }

    #[test]
    fn a_copy_its_slot_replaces_is_lost_until_settled() {
        // Validator r21 Major 1 (D76): key 2's holder replaces its own copy
        // of key 1's NO with a newer vote; the honest copies were skipped
        // as known. The NO stays up to 4 and is asked for again, until a
        // copy is stored or its masternode's newer vote is (DEC-257, D77):
        // while the outpoint is unresolved no vote is known to be that.
        let obj = proposal("a");
        let h = obj.hash();
        let mn = HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [1; 32],
                voting_key_id: key_id(1),
                operator_key: [0; 48],
                weight: 4,
            },
        )]);
        for how in [
            Settle::Copy,
            Settle::ResolvedThenNewer,
            Settle::NewerThenResolved,
        ] {
            let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
            let honest = forged_no(&mut s, h);
            assert!(s.take_refetch().is_empty());
            let own = vote(h, 2, 1, VoteOutcome::Abstain, NOW - 30);
            assert_eq!(s.add_vote(own, peer(2), NOW), Ok(()));
            assert!(!s.has_vote(&honest.hash()));
            assert_eq!(s.take_refetch(), vec![honest.hash()]);
            lost_no(&s, &h);
            assert_eq!(s.exact_weight(&h, &outpoint(1)), None);
            // The holder's newer vote again settles nothing.
            let again = vote(h, 2, 1, VoteOutcome::Yes, NOW - 25);
            assert_eq!(s.add_vote(again, peer(2), NOW), Ok(()));
            assert_eq!(s.tally(&h).no.max, 4, "{how:?}");
            let newer = vote(h, 1, 1, VoteOutcome::Yes, NOW - 20);
            match how {
                Settle::Copy => assert_eq!(s.add_vote(honest, peer(1), NOW), Ok(())),
                Settle::ResolvedThenNewer => {
                    s.set_resolved(mn.clone());
                    assert_eq!(s.tally(&h).no.max, 4);
                    // Older than the lost vote: it stays.
                    let older = vote(h, 1, 1, VoteOutcome::Abstain, NOW - 45);
                    assert_eq!(s.add_vote(older, peer(1), NOW), Ok(()));
                    assert_eq!(s.tally(&h).no.max, 4);
                    assert_eq!(s.add_vote(newer, peer(1), NOW), Ok(()));
                }
                Settle::NewerThenResolved => {
                    assert_eq!(s.add_vote(newer, peer(1), NOW), Ok(()));
                    assert_eq!(s.tally(&h).no.max, 4);
                    s.set_resolved(mn.clone());
                }
            }
            let t = s.tally(&h);
            assert_eq!(
                t.no.max,
                u32::from(matches!(how, Settle::Copy)) * 4,
                "{how:?}"
            );
            if !matches!(how, Settle::Copy) {
                assert_eq!(t.uncounted, 0, "{how:?}");
                assert!(!s.uncounted.contains_key(&h), "{how:?}");
            }
        }
    }

    #[test]
    fn a_same_time_vote_does_not_settle_a_lost_vote() {
        // Validator r26 (D77): Core refuses a vote no newer than the one it
        // holds for the slot, so only a strictly newer vote of the resolved
        // key settles the lost NO; a same-time YES leaves it either way.
        let obj = proposal("a");
        let h = obj.hash();
        let mn = HashMap::from([(
            outpoint(1),
            ResolvedMasternode {
                pro_tx_hash: [1; 32],
                voting_key_id: key_id(1),
                operator_key: [0; 48],
                weight: 4,
            },
        )]);
        for resolve_first in [true, false] {
            let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
            forged_no(&mut s, h);
            let own = vote(h, 2, 1, VoteOutcome::Abstain, NOW - 30);
            assert_eq!(s.add_vote(own, peer(2), NOW), Ok(()));
            lost_no(&s, &h);
            let same = vote(h, 1, 1, VoteOutcome::Yes, NOW - 40);
            if resolve_first {
                s.set_resolved(mn.clone());
            }
            assert_eq!(s.add_vote(same, peer(1), NOW), Ok(()));
            if !resolve_first {
                s.set_resolved(mn.clone());
            }
            assert_eq!(s.tally(&h).no.max, 4, "resolve first {resolve_first}");
            assert!(s.uncounted[&h].lost.contains_key(&outpoint(1)));
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
                operator_key: [0; 48],
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
        // Objects not stored: past twice the bound, the oldest lost votes
        // go down to it.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_unstored_lost = 1;
        let (b, c, d) = (proposal("b"), proposal("c"), proposal("d"));
        let lose = |s: &mut GovernanceStore, o: &GovernanceObject, op: u8, at: i64| {
            let v = vote(o.hash(), 2, op, VoteOutcome::No, NOW - 40);
            assert_eq!(s.add_vote(v, peer(2), at), Err(VoteRejection::Orphan));
            s.expire_orphans(at + ORPHAN_EXPIRY_SECS + 1);
        };
        lose(&mut s, &b, 1, NOW);
        lose(&mut s, &c, 1, NOW);
        assert!(s.uncounted.contains_key(&b.hash()));
        lose(&mut s, &d, 1, NOW);
        assert!(!s.uncounted.contains_key(&b.hash()));
        assert!(!s.uncounted.contains_key(&c.hash()));
        assert!(s.uncounted.contains_key(&d.hash()));
        assert_eq!(s.unstored_lost.len(), 1);
        // Entries gone stale (their object stored since) are not counted.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_unstored_lost = 2;
        let at = NOW + ORPHAN_EXPIRY_SECS + 2;
        lose(&mut s, &b, 1, NOW);
        lose(&mut s, &c, 1, NOW);
        assert_eq!(s.add_object(c.clone(), peer(1), at), Ok(true));
        lose(&mut s, &d, 1, at);
        for o in [&b, &c, &d] {
            assert!(s.uncounted.contains_key(&o.hash()));
        }
        // Past the per-proposal bound, an object not stored keeps nothing
        // more: no record outlives the queue.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_refused = 1;
        s.max_unstored_lost = 1;
        for i in 0..20 {
            let o = proposal(&format!("unstored-{i}"));
            lose(&mut s, &o, 1, NOW);
            lose(&mut s, &o, 2, NOW);
            assert!(
                s.uncounted
                    .get(&o.hash())
                    .is_none_or(|u| u.lost_past.is_empty())
            );
        }
        assert!(s.uncounted.len() <= 2, "{}", s.uncounted.len());
        // And with their object.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        forged_no(&mut s, h);
        s.set_masternode_list(&[entry(1, 4)]);
        s.remove_object(&h);
        assert!(s.uncounted.is_empty());
    }

    /// Validator r23 Minor 4 (D72, D76): a proposal whose only trace is a
    /// lost vote, or an awaited one, is not evicted; the uncertainty would
    /// go with it.
    #[test]
    fn a_proposal_holding_uncertain_votes_is_not_evicted() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_objects = 2;
        forged_no(&mut s, h);
        s.set_masternode_list(&[entry(1, 4)]);
        assert!(s.votes.get(&h).is_none_or(|m| m.is_empty()));
        lost_no(&s, &h);
        let (b, c) = (proposal("b"), proposal("c"));
        assert_eq!(s.add_object(b.clone(), peer(1), NOW + 1), Ok(true));
        assert_eq!(s.add_object(c.clone(), peer(1), NOW + 2), Ok(true));
        assert!(s.object(&h).is_some() && s.object(&b.hash()).is_none());
        lost_no(&s, &h);
        // An awaited copy of a valid masternode's vote.
        let mut s = store(&obj, &[entry(1, 4), entry(2, 4)]);
        s.max_objects = 2;
        forged_no(&mut s, h);
        let resolved = ResolvedMasternode {
            pro_tx_hash: [1; 32],
            voting_key_id: key_id(1),
            operator_key: [0; 48],
            weight: 4,
        };
        s.set_resolved(HashMap::from([(outpoint(1), resolved)]));
        assert!(s.votes.get(&h).is_none_or(|m| m.is_empty()));
        assert_eq!(s.tally(&h).uncounted, 1);
        assert_eq!(s.add_object(b, peer(1), NOW + 1), Ok(true));
        assert_eq!(s.add_object(c.clone(), peer(1), NOW + 2), Ok(true));
        assert!(s.object(&h).is_some() && s.object(&c.hash()).is_some());
        assert_eq!(s.tally(&h).uncounted, 1);
    }

    /// Validator r24 Moderate 1 (D47, D76): a lost vote of a banned key
    /// keeps nothing, as its stored copy did not.
    #[test]
    fn a_banned_keys_lost_vote_does_not_keep_an_object() {
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
        s.add_vote(vote(hb, 2, 1, VoteOutcome::Yes, NOW - 20), None, NOW)
            .unwrap();
        s.add_vote(vote(hb, 2, 1, VoteOutcome::No, NOW - 10), None, NOW)
            .unwrap();
        assert_eq!(s.tally(&hb).uncounted, 1);
        s.add_object(valid, None, NOW + 1).unwrap();
        s.add_vote(vote(hv, 1, 2, VoteOutcome::Yes, NOW - 10), None, NOW)
            .unwrap();
        assert_eq!(s.add_object(proposal("new"), None, NOW + 2), Ok(true));
        assert!(!s.has_object(&hb) && s.has_object(&hv));
    }

    /// Validator r24 Moderate 1 (D47, D76): votes on a trigger are ignored,
    /// so a lost vote noted before it arrived is dropped with it: nothing
    /// is asked for again, and the trigger is evicted as unvoted.
    #[test]
    fn a_trigger_keeps_no_lost_vote() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_confirmations(1);
        s.max_objects = 2;
        s.set_masternode_list(&[entry(1, 1), entry(2, 1)]);
        s.begin_run();
        let t = trigger(100);
        let ht = t.hash();
        let v = vote(ht, 2, 1, VoteOutcome::Yes, NOW - 40);
        assert_eq!(s.add_vote(v, peer(2), NOW), Err(VoteRejection::Orphan));
        s.expire_orphans(NOW + ORPHAN_EXPIRY_SECS + 1);
        assert!(s.uncounted.contains_key(&ht));
        assert_eq!(s.add_object(t, peer(1), NOW), Ok(true));
        assert!(!s.uncounted.contains_key(&ht));
        assert!(s.take_refetch().is_empty());
        s.requeue_lost();
        assert!(s.take_refetch().is_empty());
        s.begin_run();
        assert_eq!(s.add_object(proposal("x"), peer(1), NOW + 1), Ok(true));
        assert_eq!(s.add_object(proposal("y"), peer(1), NOW + 2), Ok(true));
        assert!(!s.has_object(&ht));
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
        // The NONE signal and the NONE outcome, of any signal.
        for (signal, outcome) in [(0, 1), (1, 0), (3, 0)] {
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
                operator_key: [0; 48],
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
        s.set_synced(&[]);
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
                operator_key: [0; 48],
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

    /// The operator key of outpoint `(op, 0)`'s masternode in these tests.
    fn op_key(op: u8) -> [u8; 48] {
        [op; 48]
    }

    /// An operator vote of `signal` for outpoint `(op, 0)`. Its signature
    /// is a placeholder: the store takes the caller's check (D89).
    fn signal_vote(
        parent: [u8; 32],
        op: u8,
        signal: SentinelSignal,
        outcome: VoteOutcome,
        time: i64,
    ) -> GovernanceVote {
        let mut v = GovernanceVote::funding(outpoint(op), parent, outcome, time);
        v.signal = signal.code();
        v.signature = vec![op; BLS_SIG_SIZE];
        v
    }

    /// Hands `v` to the store as the sync does: checked under its
    /// outpoint's operator key, if resolved, with the verdict `verifies`.
    fn add_signal(
        s: &mut GovernanceStore,
        v: GovernanceVote,
        verifies: bool,
    ) -> Result<(), VoteRejection> {
        let outcome = v.vote_outcome().unwrap();
        let checked = s
            .operator_key(&v.masternode_outpoint)
            .map(|k| (k, verifies));
        let v = OperatorVote {
            vote: v,
            outcome,
            checked,
            held: false,
        };
        s.add_operator_vote(v, peer(1), NOW)
    }

    /// The sync's tick: checks the ready parked votes, those of outpoints
    /// in `good` verifying. Returns how many were stored.
    fn check_parked(s: &mut GovernanceStore, good: &[u8]) -> usize {
        let mut stored = 0;
        for (mut v, from) in s.take_parked_ready(usize::MAX) {
            let op = v.vote.masternode_outpoint;
            let ok = good.iter().any(|g| outpoint(*g) == op);
            v.checked = s.operator_key(&op).map(|k| (k, ok));
            stored += usize::from(s.add_operator_vote(v, from, NOW).is_ok());
        }
        stored
    }

    /// Resolves outpoint `(op, 0)` to the masternode of key `key`, weight 1,
    /// operator key [`op_key`]`(op)`, for each `(op, key)`.
    fn resolve(s: &mut GovernanceStore, ops: &[(u8, u8)]) {
        resolve_keys(
            s,
            &ops.iter()
                .map(|&(op, key)| (op, key, op))
                .collect::<Vec<_>>(),
        );
    }

    /// [`resolve`] with operator key [`op_key`]`(operator)` for each
    /// `(op, key, operator)`.
    fn resolve_keys(s: &mut GovernanceStore, ops: &[(u8, u8, u8)]) {
        s.set_resolved(
            ops.iter()
                .map(|&(op, key, operator)| {
                    let r = ResolvedMasternode {
                        pro_tx_hash: [op; 32],
                        voting_key_id: key_id(key),
                        operator_key: op_key(operator),
                        weight: 1,
                    };
                    (outpoint(op), r)
                })
                .collect(),
        );
    }

    fn signal_estimates(s: &GovernanceStore, h: &[u8; 32], signal: SentinelSignal) -> (u32, u32) {
        let t = s.signal_tally(h, signal).estimate;
        (t.yes, t.no)
    }

    #[test]
    fn other_signals_are_operator_votes_stored_per_signal_and_tallied() {
        // D82, D89: Core keeps each outpoint's current vote per signal
        // (`mapInstances[signal]`), counts each signal apart, and takes a
        // proposal's other signals signed by the operator key only.
        use SentinelSignal::{Delete, Endorsed, Valid};
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1), entry(2, 4)]);
        resolve(&mut s, &[(1, 1), (2, 2)]);
        assert_eq!(s.operator_key(&outpoint(1)), Some(op_key(1)));
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), peer(1), NOW),
            Ok(())
        );
        for (signal, outcome) in [
            (Valid, VoteOutcome::No),
            (Delete, VoteOutcome::Yes),
            (Endorsed, VoteOutcome::Yes),
        ] {
            assert_eq!(
                add_signal(&mut s, signal_vote(h, 1, signal, outcome, 10), true),
                Ok(())
            );
        }
        assert_eq!(
            add_signal(&mut s, signal_vote(h, 2, Delete, VoteOutcome::No, 10), true),
            Ok(())
        );
        assert_eq!(s.vote_count(), 5);
        assert_eq!(s.votes_on(&h).count(), 1, "funding keeps its own map");
        assert_eq!(s.signal_votes_on(&h).count(), 4);
        assert_eq!(s.tally(&h).estimate.yes, 1);
        assert_eq!(s.tally(&h).estimate.no, 0);
        assert_eq!(signal_estimates(&s, &h, Valid), (0, 1));
        // The resolved masternode's weight, exactly.
        assert_eq!(signal_estimates(&s, &h, Delete), (1, 1));
        assert_eq!(signal_estimates(&s, &h, Endorsed), (1, 0));
        let delete = s.signal_tally(&h, Delete);
        assert_eq!((delete.yes.min, delete.no.max), (1, 1));
        assert_eq!(delete.voters, 2);
        // Stored as its masternode's, with its voting key for the rows.
        for (_, v) in s.signal_votes_on(&h) {
            let key = if v.outpoint == outpoint(1) { 1 } else { 2 };
            assert_eq!(v.key_id, key_id(key));
        }

        // A voting-key signature on another signal is no operator's: Core
        // refuses it and relays none, so the copy is forged. Not tallied,
        // not remembered, no penalty: another announcer's may be good.
        let mut forged = signal_vote(h, 2, Valid, VoteOutcome::Yes, 11);
        sign(&mut forged, 2);
        let r = s.add_vote(forged.clone(), peer(1), NOW);
        assert_eq!(r, Err(VoteRejection::Unverifiable));
        assert!(!r.unwrap_err().is_invalid());
        assert!(!s.has_vote(&forged.hash()));
        // The real operator vote of that hash is then taken.
        let real = signal_vote(h, 2, Valid, VoteOutcome::Yes, 11);
        assert_eq!(real.hash(), forged.hash());
        assert_eq!(add_signal(&mut s, real, true), Ok(()));
        // An operator vote whose object has not arrived is not kept: that
        // object's vote sync fetches it again (D58).
        let later = proposal("b");
        let hb = later.hash();
        for v in [signal_vote(hb, 1, Delete, VoteOutcome::Yes, 10), {
            let mut v = signal_vote(hb, 1, Delete, VoteOutcome::Yes, 10);
            sign(&mut v, 1);
            v
        }] {
            assert_eq!(
                s.add_vote(v.clone(), peer(1), NOW),
                Err(VoteRejection::Unverifiable)
            );
            assert!(!s.has_vote(&v.hash()));
        }
        assert_eq!(s.orphan_count(), 0);
        // A funding vote still takes only the voting key's signature.
        let mut bls_funding = vote(h, 1, 1, VoteOutcome::No, 12);
        bls_funding.signature = vec![1; BLS_SIG_SIZE];
        assert_eq!(
            s.add_vote(bls_funding, peer(1), NOW),
            Err(VoteRejection::Malformed)
        );
    }

    #[test]
    fn a_signal_vote_replaces_only_its_own_signal() {
        use SentinelSignal::{Delete, Endorsed};
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1)]);
        resolve(&mut s, &[(1, 1)]);
        let add = |s: &mut GovernanceStore, v| add_signal(s, v, true);
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Yes, 10), peer(1), NOW),
            Ok(())
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Endorsed, VoteOutcome::Yes, 10)),
            Ok(())
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::Yes, 10)),
            Ok(())
        );
        // Core's checks against the signal's own current vote.
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::No, 9)),
            Err(VoteRejection::Obsolete)
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::Yes, 10)),
            Err(VoteRejection::Duplicate)
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::No, 10)),
            Ok(()),
            "of the same time, the higher outcome replaces"
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::Yes, 10)),
            Err(VoteRejection::SameTimestamp)
        );
        assert_eq!(
            add(&mut s, signal_vote(h, 1, Delete, VoteOutcome::Abstain, 11)),
            Ok(())
        );
        let mut by_signal: Vec<_> = s
            .signal_votes_on(&h)
            .map(|(signal, v)| (signal, v.outcome, v.time))
            .collect();
        by_signal.sort();
        assert_eq!(
            by_signal,
            [
                (Delete, VoteOutcome::Abstain, 11),
                (Endorsed, VoteOutcome::Yes, 10)
            ]
        );
        assert_eq!(current(&s, &h, 1), [(VoteOutcome::Yes, key_id(1))]);
        assert_eq!(s.vote_count(), 3);
        // A funding vote older than the delete vote is not obsolete.
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::No, 11), peer(1), NOW),
            Ok(())
        );
        assert_eq!(s.vote_count(), 3);
    }

    #[test]
    fn operator_votes_wait_for_their_outpoint_and_a_new_key() {
        // D89: a vote of an unresolved outpoint is parked and held, then
        // checked once the outpoint resolves; one failing under its
        // masternode's key waits, unheld, for that key to change.
        use SentinelSignal::Valid;
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1), entry(2, 1)]);
        let v1 = signal_vote(h, 1, Valid, VoteOutcome::No, 10);
        assert_eq!(
            add_signal(&mut s, v1.clone(), true),
            Err(VoteRejection::Parked)
        );
        assert!(!s.has_vote(&v1.hash()));
        assert_eq!(s.signal_votes_on(&h).count(), 0);
        assert!(
            s.take_parked_ready(usize::MAX).is_empty(),
            "not resolved yet"
        );
        // The same copy again is the one parked; another copy of the hash
        // waits beside it.
        assert_eq!(
            add_signal(&mut s, v1.clone(), true),
            Err(VoteRejection::Parked)
        );
        let mut other = v1.clone();
        other.signature = vec![9; BLS_SIG_SIZE];
        assert_eq!(add_signal(&mut s, other, true), Err(VoteRejection::Parked));
        assert_eq!(s.parked.len(), 2);
        resolve(&mut s, &[(1, 1)]);
        // Only the real copy verifies; the forged one, checked after it, is
        // then a copy of a stored vote.
        let results: Vec<_> = s
            .take_parked_ready(usize::MAX)
            .into_iter()
            .map(|(mut v, from)| {
                let ok = v.vote.signature == v1.signature;
                v.checked = s.operator_key(&v.vote.masternode_outpoint).map(|k| (k, ok));
                s.add_operator_vote(v, from, NOW)
            })
            .collect();
        assert_eq!(results, [Ok(()), Err(VoteRejection::Duplicate)]);
        assert_eq!(signal_estimates(&s, &h, Valid), (0, 1));
        assert!(s.parked.is_empty());

        // A vote failing under its resolved masternode's key: refused (so
        // another announcer's copy is asked for) and parked unheld; tried
        // again once the key changes, as the wallet's list may lag.
        resolve(&mut s, &[(1, 1), (2, 2)]);
        let v2 = signal_vote(h, 2, Valid, VoteOutcome::Yes, 10);
        assert_eq!(
            add_signal(&mut s, v2.clone(), false),
            Err(VoteRejection::Unverifiable)
        );
        assert!(s.take_parked_ready(usize::MAX).is_empty());
        resolve_keys(&mut s, &[(1, 1, 1), (2, 2, 20)]);
        assert_eq!(check_parked(&mut s, &[2]), 1);
        assert_eq!(signal_estimates(&s, &h, Valid), (1, 1));
        assert!(s.has_vote(&v2.hash()));
        // A checked vote whose key changed since it was checked is parked.
        let v3 = signal_vote(h, 2, Valid, VoteOutcome::No, 11);
        let stale = OperatorVote {
            vote: v3,
            outcome: VoteOutcome::No,
            checked: Some((op_key(2), true)),
            held: false,
        };
        assert_eq!(
            s.add_operator_vote(stale, peer(1), NOW),
            Err(VoteRejection::Parked)
        );
        // So is one that failed under a key since replaced: held, as no
        // check under the current key refused it.
        let v4 = signal_vote(h, 2, Valid, VoteOutcome::No, 12);
        let failed = OperatorVote {
            vote: v4.clone(),
            outcome: VoteOutcome::No,
            checked: Some((op_key(2), false)),
            held: false,
        };
        assert_eq!(
            s.add_operator_vote(failed, peer(1), NOW),
            Err(VoteRejection::Parked)
        );
        assert!(s.parked.values().any(|p| p.vote == v4 && p.held));
        assert_eq!(check_parked(&mut s, &[]), 0);
        assert_eq!(signal_estimates(&s, &h, Valid), (1, 1));
    }

    #[test]
    fn a_masternode_leaving_or_rekeying_its_operator_drops_its_votes() {
        // Core drops the votes of a masternode that left the list
        // (`ClearMasternodeVotes`) or whose operator key changed
        // (`RemoveInvalidVotes`) (D89).
        use SentinelSignal::{Delete, Endorsed};
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1), entry(2, 1), entry(3, 1)]);
        resolve(&mut s, &[(1, 1), (2, 2), (3, 3)]);
        for op in 1..=3 {
            assert_eq!(
                add_signal(
                    &mut s,
                    signal_vote(h, op, Delete, VoteOutcome::Yes, 10),
                    true
                ),
                Ok(())
            );
        }
        assert_eq!(
            add_signal(
                &mut s,
                signal_vote(h, 3, Endorsed, VoteOutcome::Yes, 10),
                true
            ),
            Ok(())
        );
        // Masternode 2 changes its voting key only: its votes stay.
        resolve_keys(&mut s, &[(1, 1, 1), (2, 9, 2), (3, 3, 3)]);
        assert_eq!(signal_estimates(&s, &h, Delete), (3, 0));
        // Masternode 1 leaves, masternode 3 gets a new operator key.
        resolve_keys(&mut s, &[(2, 9, 2), (3, 3, 30)]);
        assert_eq!(signal_estimates(&s, &h, Delete), (1, 0));
        assert_eq!(signal_estimates(&s, &h, Endorsed), (0, 0));
        assert_eq!(s.vote_count(), 1);
    }

    /// A store with masternodes of keys 1..=6 (weight 1, W = 6, so the
    /// deletion threshold is 4), outpoints 1..=6 resolved to them.
    fn deletion_store(obj: &GovernanceObject) -> GovernanceStore {
        let list: Vec<ListEntry> = (1..=6).map(|k| entry(k, 1)).collect();
        let mut s = store(obj, &list);
        resolve(&mut s, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)]);
        s
    }

    fn delete(s: &mut GovernanceStore, h: [u8; 32], op: u8, outcome: VoteOutcome) {
        let v = signal_vote(h, op, SentinelSignal::Delete, outcome, 10);
        assert_eq!(add_signal(s, v, true), Ok(()));
    }

    #[test]
    fn a_proposal_voted_deleted_leaves_ten_minutes_after_the_mark() {
        // D84: `absyes(DELETE) >= max(q, 2W/3)` marks it
        // (`UpdateSentinelVariables`); `UpdateCachesAndClean` erases it
        // `GOVERNANCE_DELETION_DELAY` later.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = deletion_store(&obj);
        for op in 1..=4 {
            delete(&mut s, h, op, VoteOutcome::Yes);
        }
        delete(&mut s, h, 6, VoteOutcome::No);
        s.set_synced(&[h]);
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None, "a net 3 is below 4");
        delete(&mut s, h, 5, VoteOutcome::Yes);
        s.begin_vote_sync();
        s.prune(NOW);
        assert_eq!(
            s.deletion_mark(&h),
            None,
            "not covered by this run's vote sync"
        );
        s.set_synced(&[h]);
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), Some(NOW), "a net 4 is at it");
        // Listed until it leaves, but no longer vote-synced.
        assert_eq!(s.proposals().count(), 1);
        assert!(s.current_proposals(NOW).is_empty());
        assert!(!s.is_current_proposal(&h, NOW));
        s.prune(NOW + DELETION_DELAY_SECS - 1);
        assert!(s.object(&h).is_some());
        s.prune(NOW + DELETION_DELAY_SECS);
        assert!(s.object(&h).is_none());
        assert_eq!(s.vote_count(), 0);
        // Remembered, so not fetched again.
        assert!(s.has_object(&h));
        assert_eq!(s.add_object(obj, peer(1), NOW), Ok(false));
    }

    #[test]
    fn the_deletion_mark_is_kept_and_freezes_the_votes() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = deletion_store(&obj);
        s.set_synced(&[h]);
        for op in 1..=4 {
            delete(&mut s, h, op, VoteOutcome::Yes);
        }
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), Some(NOW));
        // Core ignores votes on a deleted object (`ProcessVote`).
        let no = signal_vote(h, 5, SentinelSignal::Delete, VoteOutcome::No, 20);
        assert_eq!(add_signal(&mut s, no, true), Err(VoteRejection::Ignored));
        assert_eq!(
            s.add_vote(vote(h, 5, 5, VoteOutcome::Yes, 20), peer(1), NOW),
            Err(VoteRejection::Ignored)
        );
        // The mark keeps its first time (`MarkForDeletion`), even where a
        // bigger list would no longer reach the threshold.
        let list: Vec<ListEntry> = (1..=12).map(|k| entry(k, 1)).collect();
        s.set_masternode_list(&list);
        s.prune(NOW + 60);
        assert_eq!(s.deletion_mark(&h), Some(NOW));
        // A marked proposal is not evicted: that would forget the mark.
        s.max_objects = 1;
        assert_eq!(
            s.add_object(proposal("b"), peer(1), NOW + 61),
            Err(ObjectRejection::StoreFull)
        );
        s.prune(NOW + DELETION_DELAY_SECS);
        assert!(s.object(&h).is_none());
    }

    #[test]
    fn parked_delete_votes_count_once_checked() {
        // D84, D89: an unresolved outpoint's vote is no masternode's yet; it
        // counts once resolved and checked. With no valid weight, as in
        // Core, nothing marks.
        let obj = proposal("a");
        let h = obj.hash();
        let list: Vec<ListEntry> = (1..=6).map(|k| entry(k, 1)).collect();
        let mut s = store(&obj, &list);
        s.set_synced(&[h]);
        for op in 1..=6 {
            let v = signal_vote(h, op, SentinelSignal::Delete, VoteOutcome::Yes, 10);
            assert_eq!(add_signal(&mut s, v, true), Err(VoteRejection::Parked));
        }
        assert_eq!(signal_estimates(&s, &h, SentinelSignal::Delete), (0, 0));
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None);
        resolve(&mut s, &[(1, 1), (2, 2), (3, 3), (4, 4)]);
        assert_eq!(check_parked(&mut s, &[1, 2, 3, 4]), 4);
        let mut banned = list.clone();
        for e in &mut banned {
            e.valid = false;
        }
        s.set_masternode_list(&banned);
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None);
        s.set_masternode_list(&list);
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), Some(NOW));
    }

    #[test]
    fn a_delete_no_the_wallet_cannot_check_blocks_the_mark() {
        // D85: a delete NO held but unchecked might be one Core counts;
        // without it the four YES would mark.
        let marks = |prepare: &dyn Fn(&mut GovernanceStore, [u8; 32])| {
            let obj = proposal("a");
            let h = obj.hash();
            let mut s = deletion_store(&obj);
            s.set_synced(&[h]);
            for op in 1..=4 {
                delete(&mut s, h, op, VoteOutcome::Yes);
            }
            prepare(&mut s, h);
            s.prune(NOW);
            s.deletion_mark(&h).is_some()
        };
        let unresolved =
            |op, outcome, time| move |h| signal_vote(h, op, SentinelSignal::Delete, outcome, time);
        // A seventh masternode, not resolved yet (W = 7, still 4 to mark).
        let seventh = |s: &mut GovernanceStore| {
            let list: Vec<ListEntry> = (1..=7).map(|k| entry(k, 1)).collect();
            s.set_masternode_list(&list);
        };
        assert!(marks(&|_, _| {}));
        assert!(marks(&|s, _| seventh(s)));
        // A NO parked for its unresolved outpoint.
        assert!(!marks(&|s, h| {
            seventh(s);
            let v = unresolved(7, VoteOutcome::No, 10)(h);
            assert_eq!(add_signal(s, v, true), Err(VoteRejection::Parked));
        }));
        // A YES parked so is no doubt.
        assert!(marks(&|s, h| {
            seventh(s);
            let v = unresolved(7, VoteOutcome::Yes, 10)(h);
            assert_eq!(add_signal(s, v, true), Err(VoteRejection::Parked));
        }));
        // With every listed masternode resolved, an unknown outpoint is no
        // masternode's (Core: unknown masternode): refused, or dropped when
        // parked before.
        assert!(marks(&|s, h| {
            let v = unresolved(10, VoteOutcome::No, 10)(h);
            assert_eq!(add_signal(s, v, true), Err(VoteRejection::Unverifiable));
            assert!(s.parked.is_empty());
        }));
        assert!(marks(&|s, h| {
            seventh(s);
            let v = unresolved(10, VoteOutcome::No, 10)(h);
            assert_eq!(add_signal(s, v, true), Err(VoteRejection::Parked));
            resolve(s, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (7, 7)]);
            assert!(s.parked.is_empty());
        }));
        // A NO refused at arrival for failing under its masternode's key:
        // another announcer's copy is asked for, so it is no doubt.
        assert!(marks(&|s, h| {
            let v = signal_vote(h, 6, SentinelSignal::Delete, VoteOutcome::No, 10);
            assert_eq!(add_signal(s, v, false), Err(VoteRejection::Unverifiable));
        }));
        // A held NO that fails once its outpoint resolves: a doubt, asked
        // for again, until a checked delete vote of that masternode at
        // least as new is stored.
        let failed = |s: &mut GovernanceStore, h| {
            seventh(s);
            let v = unresolved(7, VoteOutcome::No, 10)(h);
            assert_eq!(add_signal(s, v.clone(), true), Err(VoteRejection::Parked));
            resolve(s, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6), (7, 7)]);
            assert_eq!(check_parked(s, &[]), 0);
            assert_eq!(s.take_refetch(), [v.hash()]);
        };
        assert!(!marks(&|s, h| failed(s, h)));
        assert!(!marks(&|s, h| {
            failed(s, h);
            let older = signal_vote(h, 7, SentinelSignal::Delete, VoteOutcome::Yes, 9);
            assert_eq!(add_signal(s, older, true), Ok(()));
        }));
        assert!(marks(&|s, h| {
            failed(s, h);
            let newer = signal_vote(h, 7, SentinelSignal::Delete, VoteOutcome::Yes, 10);
            assert_eq!(add_signal(s, newer, true), Ok(()));
        }));
        // ...or its masternode leaves the list.
        assert!(marks(&|s, h| {
            failed(s, h);
            resolve(s, &[(1, 1), (2, 2), (3, 3), (4, 4), (5, 5), (6, 6)]);
        }));
        // A funding NO is funding's.
        assert!(marks(&|s, h| {
            assert_eq!(
                s.add_vote(vote(h, 5, 5, VoteOutcome::No, 10), peer(1), NOW),
                Ok(())
            );
        }));
    }

    #[test]
    fn parked_votes_are_bounded_and_a_held_no_evicted_is_a_doubt() {
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = deletion_store(&obj);
        s.set_synced(&[h]);
        for op in 1..=4 {
            delete(&mut s, h, op, VoteOutcome::Yes);
        }
        // A seventh masternode not resolved: unknown outpoints may be its.
        let list: Vec<ListEntry> = (1..=7).map(|k| entry(k, 1)).collect();
        s.set_masternode_list(&list);
        let parked = |s: &mut GovernanceStore, i: u32, outcome, from| {
            let mut v = signal_vote(h, 0, SentinelSignal::Delete, outcome, 10);
            v.masternode_outpoint.vout = 1_000 + i;
            let o = OperatorVote {
                vote: v,
                outcome,
                checked: None,
                held: false,
            };
            s.add_operator_vote(o, from, NOW)
        };
        // One sender's bound: past it, refused, and asked for again of
        // another announcer.
        for i in 0..MAX_PARKED_PER_PEER {
            assert_eq!(
                parked(&mut s, i, VoteOutcome::Yes, peer(1)),
                Err(VoteRejection::Parked)
            );
        }
        assert!(s.take_refetch().is_empty());
        assert_eq!(
            parked(&mut s, MAX_PARKED_PER_PEER, VoteOutcome::Yes, peer(1)),
            Err(VoteRejection::Unverifiable)
        );
        assert_eq!(s.take_refetch().len(), 1);
        assert_eq!(
            parked(&mut s, MAX_PARKED_PER_PEER, VoteOutcome::No, None),
            Err(VoteRejection::Parked)
        );
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None, "a held NO waits");
        // In all: the oldest goes; the held NO evicted becomes a doubt.
        s.max_parked = MAX_PARKED_PER_PEER as usize + 1;
        let next = MAX_PARKED_PER_PEER + 1;
        for i in next..=next + MAX_PARKED_PER_PEER {
            assert_eq!(
                parked(&mut s, i, VoteOutcome::Yes, None),
                Err(VoteRejection::Parked)
            );
        }
        assert_eq!(s.parked.len(), MAX_PARKED_PER_PEER as usize + 1);
        assert!(s.parked.values().all(|p| p.outcome == VoteOutcome::Yes));
        assert_eq!(
            s.take_refetch().len(),
            MAX_PARKED_PER_PEER as usize + 1,
            "every held vote evicted is asked for again"
        );
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None, "the evicted NO is a doubt");
        assert!(!s.parked_per_peer.contains_key(&peer(1).unwrap()));
    }

    #[test]
    fn a_proposal_with_operator_votes_only_is_not_evicted() {
        // Validator r1 m2: its operator votes count as Core counts them
        // (D89), so it is weighted, as one with funding votes is.
        let obj = proposal("a");
        let mut s = store(&obj, &[entry(1, 1)]);
        s.max_objects = 1;
        assert!(
            s.add_object(proposal("b"), peer(1), NOW).is_ok(),
            "unweighted, evicted"
        );
        let mut s = store(&obj, &[entry(1, 1)]);
        resolve(&mut s, &[(1, 1)]);
        let v = signal_vote(obj.hash(), 1, SentinelSignal::Valid, VoteOutcome::No, 10);
        assert_eq!(add_signal(&mut s, v, true), Ok(()));
        s.max_objects = 1;
        assert_eq!(
            s.add_object(proposal("b"), peer(1), NOW),
            Err(ObjectRejection::StoreFull)
        );
    }

    #[test]
    fn removing_an_object_unparks_its_votes_only() {
        // Its parked votes go with it; another object's stay, and their
        // sender's count with them.
        use SentinelSignal::Valid;
        let (a, b) = (proposal("a"), proposal("b"));
        let mut s = store(&a, &[entry(1, 1), entry(2, 1)]);
        s.add_object(b.clone(), None, NOW).unwrap();
        for obj in [&a, &b] {
            let v = signal_vote(obj.hash(), 1, Valid, VoteOutcome::No, 10);
            assert_eq!(add_signal(&mut s, v, true), Err(VoteRejection::Parked));
        }
        assert_eq!(s.parked_per_peer[&peer(1).unwrap()], 2);
        s.remove_object(&a.hash());
        assert_eq!(s.parked.len(), 1);
        assert!(s.parked.values().all(|p| p.vote.parent_hash == b.hash()));
        assert_eq!(s.parked_per_peer[&peer(1).unwrap()], 1);
    }

    #[test]
    fn with_no_list_yet_an_operator_vote_waits() {
        // No outpoint is known to be no masternode's before the list.
        let obj = proposal("a");
        let mut s = store(&obj, &[]);
        let v = signal_vote(obj.hash(), 1, SentinelSignal::Valid, VoteOutcome::No, 10);
        assert_eq!(add_signal(&mut s, v, true), Err(VoteRejection::Parked));
    }

    #[test]
    fn a_delete_no_refused_for_its_senders_bound_is_a_doubt() {
        // Validator r2 m-C: perhaps its only announcer's copy.
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = deletion_store(&obj);
        s.set_synced(&[h]);
        for op in 1..=4 {
            delete(&mut s, h, op, VoteOutcome::Yes);
        }
        let list: Vec<ListEntry> = (1..=7).map(|k| entry(k, 1)).collect();
        s.set_masternode_list(&list);
        let parked = |s: &mut GovernanceStore, i: u32, outcome| {
            let mut v = signal_vote(h, 0, SentinelSignal::Delete, outcome, 10);
            v.masternode_outpoint.vout = 1_000 + i;
            let o = OperatorVote {
                vote: v,
                outcome,
                checked: None,
                held: false,
            };
            s.add_operator_vote(o, peer(1), NOW)
        };
        for i in 0..MAX_PARKED_PER_PEER {
            assert_eq!(
                parked(&mut s, i, VoteOutcome::Yes),
                Err(VoteRejection::Parked)
            );
        }
        assert_eq!(
            parked(&mut s, MAX_PARKED_PER_PEER, VoteOutcome::No),
            Err(VoteRejection::Unverifiable)
        );
        assert!(s.parked.values().all(|p| p.outcome == VoteOutcome::Yes));
        s.prune(NOW);
        assert_eq!(s.deletion_mark(&h), None);
    }

    #[test]
    fn a_held_operator_vote_that_fails_is_asked_for_again() {
        // Validator r2 m-D: no other announcer's copy was asked for while
        // it was held, whatever its signal.
        use SentinelSignal::Valid;
        let obj = proposal("a");
        let h = obj.hash();
        let mut s = store(&obj, &[entry(1, 1), entry(2, 1)]);
        let v = signal_vote(h, 1, Valid, VoteOutcome::Yes, 10);
        assert_eq!(
            add_signal(&mut s, v.clone(), true),
            Err(VoteRejection::Parked)
        );
        assert!(s.take_refetch().is_empty());
        resolve(&mut s, &[(1, 1)]);
        assert_eq!(check_parked(&mut s, &[]), 0);
        assert_eq!(s.take_refetch(), vec![v.hash()]);
        assert_eq!(signal_estimates(&s, &h, Valid), (0, 0));
        // Not held a second time: a failing copy is refused at once.
        assert_eq!(
            add_signal(&mut s, v, false),
            Err(VoteRejection::Unverifiable)
        );
        assert!(s.take_refetch().is_empty());
    }
}
