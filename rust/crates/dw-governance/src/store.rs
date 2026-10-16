//! The synced governance objects and current votes, in memory (MG-02 §3,
//! §4.1).
//!
//! Objects arrive checked ([`crate::validate`]). One counts (is listed,
//! tallied, used for funded heights) once enough distinct peers announced
//! it, two by default (MG-02 D3): the wallet cannot see the collateral or a
//! trigger's operator signature, which each relaying full node checked.
//!
//! For each proposal only the latest funding vote of every collateral
//! outpoint is kept (Core `mapCurrentMNVotes`). The SPV list has no
//! outpoints, so a vote counts when its signature recovers a voting key
//! that a masternode of the tip list holds, PoSe-banned ones included (Core
//! `GetMNByCollateral` finds them), or, for an outpoint the wallet resolved
//! itself, that masternode's key (D1). One key gets at most `2·n + 2`
//! outpoints per object, `n` being the masternodes holding it (D11).
//! Votes on triggers are not kept: Core checks them against the operator
//! key and the wallet does not tally them.
//!
//! A vote whose object has not arrived waits as an orphan once it passed the
//! checks that need no object (Core `IsValidForUnknownParent`, MG-01 D23):
//! for 10 minutes (`GOVERNANCE_ORPHAN_EXPIRATION_TIME`), at most
//! [`MAX_ORPHAN_VOTES`] in all with the oldest evicted, at most
//! [`MAX_ORPHANS_PER_PEER`] from one peer, one per (object, outpoint).
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
use crate::vote::{GovernanceVote, SIGNAL_FUNDING};

/// Votes kept for objects that have not arrived, in all.
pub const MAX_ORPHAN_VOTES: usize = 50_000;
/// Orphan votes kept from one peer.
pub const MAX_ORPHANS_PER_PEER: u32 = 2_000;
/// How long an orphan vote waits (Core `GOVERNANCE_ORPHAN_EXPIRATION_TIME`).
pub const ORPHAN_EXPIRY_SECS: i64 = 10 * 60;
/// Distinct peers that must announce an object before it counts (D3).
pub const DEFAULT_CONFIRMATIONS: usize = 2;

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
    /// needed).
    pub announcers: Vec<SocketAddr>,
    /// Announced by enough peers to count.
    pub confirmed: bool,
}

/// Why a vote was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteRejection {
    /// Already stored or waiting.
    Duplicate,
    /// Not a funding vote with a yes / no / abstain outcome on a proposal.
    NotTallied,
    /// The signature recovers no key.
    BadSignature,
    /// The recovered key is no voting key of the tip list.
    UnknownVotingKey,
    /// The outpoint is one the wallet resolved, and the key is not its
    /// masternode's.
    WrongVotingKey,
    /// Timed more than an hour ahead of the local clock.
    TooNew,
    /// An older vote than the outpoint's current one, or one of the same
    /// time with a lower outcome (Core keeps the higher one). Not
    /// remembered, as in Core.
    Obsolete,
    /// The key already votes for `2·n + 2` outpoints on this object (D11).
    KeyLimit,
    /// Valid, but its object has not arrived: waits, unless the orphan
    /// bounds refused it.
    Orphan,
}

impl VoteRejection {
    /// Whether the peer that sent it misbehaved (the vote cannot be from an
    /// honest node with the same list).
    pub fn is_invalid(self) -> bool {
        matches!(self, Self::BadSignature | Self::NotTallied)
    }
}

/// A masternode of the tip list as the store needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListEntry {
    pub voting_key_id: [u8; 20],
    /// 1 regular, 4 evonode.
    pub weight: u32,
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
    vote: GovernanceVote,
    from: Option<SocketAddr>,
    at: i64,
}

/// Synced objects and current votes.
#[derive(Debug)]
pub struct GovernanceStore {
    network: Network,
    confirmations: usize,
    objects: HashMap<[u8; 32], StoredObject>,
    /// object hash → outpoint → current funding vote.
    votes: HashMap<[u8; 32], HashMap<OutPoint, CurrentVote>>,
    /// (object, key) → outpoints voting with the key.
    per_key: HashMap<([u8; 32], [u8; 20]), u32>,
    seen_votes: HashSet<[u8; 32]>,
    vote_count: u64,
    orphans: HashMap<[u8; 32], Orphan>,
    /// Orphan hashes in arrival order (lazily pruned).
    orphan_order: VecDeque<[u8; 32]>,
    /// (object, outpoint) → its waiting vote.
    orphan_slots: HashMap<([u8; 32], OutPoint), [u8; 32]>,
    orphans_per_peer: HashMap<SocketAddr, u32>,
    /// Voting key → the masternodes of the tip list holding it.
    keys: HashMap<[u8; 20], KeyWeights>,
    valid_weight: u32,
    list_size: u32,
    resolved: HashMap<OutPoint, ResolvedMasternode>,
}

impl GovernanceStore {
    pub fn new(network: Network) -> Self {
        Self {
            network,
            confirmations: DEFAULT_CONFIRMATIONS,
            objects: HashMap::new(),
            votes: HashMap::new(),
            per_key: HashMap::new(),
            seen_votes: HashSet::new(),
            vote_count: 0,
            orphans: HashMap::new(),
            orphan_order: VecDeque::new(),
            orphan_slots: HashMap::new(),
            orphans_per_peer: HashMap::new(),
            keys: HashMap::new(),
            valid_weight: 0,
            list_size: 0,
            resolved: HashMap::new(),
        }
    }

    /// Sets how many distinct peers must announce an object (at least 1).
    pub fn set_confirmations(&mut self, n: usize) {
        self.confirmations = n.max(1);
        let n = self.confirmations;
        for o in self.objects.values_mut() {
            o.confirmed = o.announcers.len() >= n;
        }
    }

    pub fn network(&self) -> Network {
        self.network
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

    pub fn has_object(&self, hash: &[u8; 32]) -> bool {
        self.objects.contains_key(hash)
    }

    /// Whether the vote `hash` is counted, or waits for its object.
    pub fn has_vote(&self, hash: &[u8; 32]) -> bool {
        self.seen_votes.contains(hash) || self.orphans.contains_key(hash)
    }

    /// A confirmed object.
    pub fn object(&self, hash: &[u8; 32]) -> Option<&StoredObject> {
        self.objects.get(hash).filter(|o| o.confirmed)
    }

    /// The confirmed objects.
    pub fn objects(&self) -> impl Iterator<Item = &StoredObject> {
        self.objects.values().filter(|o| o.confirmed)
    }

    /// The current funding votes on `hash`.
    pub fn votes_on(&self, hash: &[u8; 32]) -> impl Iterator<Item = &CurrentVote> {
        self.votes.get(hash).into_iter().flat_map(|m| m.values())
    }

    /// The current vote of `outpoint` on `hash`.
    pub fn current_vote(&self, hash: &[u8; 32], outpoint: &OutPoint) -> Option<&CurrentVote> {
        self.votes.get(hash)?.get(outpoint)
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
        if self.objects.contains_key(&hash) {
            if let Some(peer) = from {
                self.note_announcer(&hash, peer);
            }
            return Ok(false);
        }
        let kind = match check_object(&object, self.network, now)? {
            ValidObject::Proposal(p) => ObjectKind::Proposal(p),
            ValidObject::Trigger(t) => ObjectKind::Trigger(t),
        };
        let announcers: Vec<SocketAddr> = from.into_iter().collect();
        let confirmed = from.is_none() || announcers.len() >= self.confirmations;
        self.objects.insert(
            hash,
            StoredObject {
                object,
                kind,
                hash_hex: display_hex(&hash),
                announcers,
                confirmed,
            },
        );
        self.adopt_orphans(&hash, now);
        Ok(true)
    }

    fn adopt_orphans(&mut self, parent: &[u8; 32], now: i64) {
        let waiting: Vec<[u8; 32]> = self
            .orphans
            .iter()
            .filter(|(_, o)| &o.vote.parent_hash == parent)
            .map(|(h, _)| *h)
            .collect();
        for h in waiting {
            if let Some(o) = self.remove_orphan(&h) {
                let _ = self.add_vote(o.vote, o.from, now);
            }
        }
    }

    fn remove_orphan(&mut self, hash: &[u8; 32]) -> Option<Orphan> {
        let o = self.orphans.remove(hash)?;
        self.orphan_slots
            .remove(&(o.vote.parent_hash, o.vote.masternode_outpoint));
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

    /// Drops orphans older than [`ORPHAN_EXPIRY_SECS`].
    pub fn expire_orphans(&mut self, now: i64) {
        while let Some(h) = self.orphan_order.front().copied() {
            match self.orphans.get(&h) {
                None => {
                    self.orphan_order.pop_front();
                }
                Some(o) if o.at + ORPHAN_EXPIRY_SECS < now => {
                    self.orphan_order.pop_front();
                    self.remove_orphan(&h);
                }
                Some(_) => break,
            }
        }
    }

    fn evict_oldest_orphan(&mut self) {
        while let Some(h) = self.orphan_order.pop_front() {
            if self.remove_orphan(&h).is_some() {
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
        let mut valid_weight = 0u32;
        for e in entries {
            keys.entry(e.voting_key_id)
                .and_modify(|k| k.add(e.weight))
                .or_insert_with(|| KeyWeights::one(e.weight));
            if e.valid {
                valid_weight = valid_weight.saturating_add(e.weight);
            }
        }
        self.keys = keys;
        self.valid_weight = valid_weight;
        self.list_size = entries.len() as u32;
        self.drop_unbacked_votes();
    }

    /// Sets the outpoints the wallet resolved to masternodes (MG-03); their
    /// votes are checked against that masternode's key and counted exactly.
    pub fn set_resolved(&mut self, resolved: HashMap<OutPoint, ResolvedMasternode>) {
        self.resolved = resolved;
        self.drop_unbacked_votes();
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
            for (outpoint, v) in per_object {
                if self.key_ok(outpoint, &v.key_id).is_err() {
                    dropped.push((*object, *outpoint));
                }
            }
        }
        for (object, outpoint) in dropped {
            self.remove_vote(&object, &outpoint);
        }
        let stale: Vec<[u8; 32]> = self
            .orphans
            .iter()
            .filter(|(_, o)| {
                o.vote
                    .signer_key_id(self.network)
                    .is_none_or(|k| self.key_ok(&o.vote.masternode_outpoint, &k).is_err())
            })
            .map(|(h, _)| *h)
            .collect();
        for h in stale {
            self.remove_orphan(&h);
        }
    }

    fn remove_vote(&mut self, object: &[u8; 32], outpoint: &OutPoint) {
        let Some(v) = self.votes.get_mut(object).and_then(|m| m.remove(outpoint)) else {
            return;
        };
        self.seen_votes.remove(&v.vote_hash);
        self.vote_count -= 1;
        if let Some(n) = self.per_key.get_mut(&(*object, v.key_id)) {
            *n -= 1;
            if *n == 0 {
                self.per_key.remove(&(*object, v.key_id));
            }
        }
    }

    /// Checks `vote` from `from` (`None`: relayed by the wallet itself) and
    /// stores it as its outpoint's current funding vote, or queues it until
    /// its object arrives.
    pub fn add_vote(
        &mut self,
        vote: GovernanceVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> Result<(), VoteRejection> {
        let hash = vote.hash();
        if self.has_vote(&hash) {
            return Err(VoteRejection::Duplicate);
        }
        let Some(outcome) = vote.vote_outcome() else {
            return Err(VoteRejection::NotTallied);
        };
        if vote.signal != SIGNAL_FUNDING {
            return Err(VoteRejection::NotTallied);
        }
        let parent = self.objects.get(&vote.parent_hash);
        if parent.is_some_and(|o| !matches!(o.kind, ObjectKind::Proposal(_))) {
            return Err(VoteRejection::NotTallied);
        }
        let parent_known = parent.is_some();
        if vote.time > now + MAX_TIME_FUTURE_DEVIATION {
            return Err(VoteRejection::TooNew);
        }
        let outpoint = vote.masternode_outpoint;
        let newer_than = |t: i64, o: i32| vote.time > t || (vote.time == t && outcome.code() > o);
        let current = self
            .votes
            .get(&vote.parent_hash)
            .and_then(|m| m.get(&outpoint));
        if current.is_some_and(|c| !newer_than(c.time, c.outcome.code())) {
            return Err(VoteRejection::Obsolete);
        }
        let waiting = self
            .orphan_slots
            .get(&(vote.parent_hash, outpoint))
            .and_then(|h| self.orphans.get(h));
        if waiting.is_some_and(|w| {
            let o = w.vote.vote_outcome().map_or(0, |o| o.code());
            !newer_than(w.vote.time, o)
        }) {
            return Err(VoteRejection::Obsolete);
        }
        // Checked last, as Core does (the cost of recovery). A refused
        // signature leaves the hash unseen, so the vote can still arrive
        // from another peer.
        let key_id = vote
            .signer_key_id(self.network)
            .ok_or(VoteRejection::BadSignature)?;
        self.key_ok(&outpoint, &key_id)?;
        let replaces_same_key = current.is_some_and(|c| c.key_id == key_id);
        if !replaces_same_key {
            let n = self
                .resolved
                .get(&outpoint)
                .map_or_else(|| self.keys.get(&key_id).map_or(0, |k| k.count), |_| 1);
            let used = self
                .per_key
                .get(&(vote.parent_hash, key_id))
                .copied()
                .unwrap_or(0);
            if used >= 2 * n + 2 {
                return Err(VoteRejection::KeyLimit);
            }
        }
        if !parent_known {
            return Err(self.queue_orphan(hash, vote, from, now));
        }
        self.remove_vote(&vote.parent_hash, &outpoint);
        self.votes.entry(vote.parent_hash).or_default().insert(
            outpoint,
            CurrentVote {
                outpoint,
                outcome,
                time: vote.time,
                key_id,
                vote_hash: hash,
            },
        );
        *self.per_key.entry((vote.parent_hash, key_id)).or_default() += 1;
        self.vote_count += 1;
        self.seen_votes.insert(hash);
        Ok(())
    }

    fn queue_orphan(
        &mut self,
        hash: [u8; 32],
        vote: GovernanceVote,
        from: Option<SocketAddr>,
        now: i64,
    ) -> VoteRejection {
        self.expire_orphans(now);
        if let Some(peer) = from
            && self.orphans_per_peer.get(&peer).copied().unwrap_or(0) >= MAX_ORPHANS_PER_PEER
        {
            return VoteRejection::Orphan;
        }
        let slot = (vote.parent_hash, vote.masternode_outpoint);
        if let Some(older) = self.orphan_slots.get(&slot).copied() {
            self.remove_orphan(&older);
        }
        if self.orphans.len() >= MAX_ORPHAN_VOTES {
            self.evict_oldest_orphan();
        }
        if let Some(peer) = from {
            *self.orphans_per_peer.entry(peer).or_default() += 1;
        }
        self.orphan_slots.insert(slot, hash);
        self.orphan_order.push_back(hash);
        self.orphans.insert(
            hash,
            Orphan {
                vote,
                from,
                at: now,
            },
        );
        VoteRejection::Orphan
    }

    /// The highest superblock height ≤ `tip` paid by a confirmed,
    /// unexpired trigger that lists `hash_hex` (node
    /// `getProposalFundedHeight` over Core's active triggers). A trigger at
    /// or below the tip is taken as executed: Core keeps those for a cycle
    /// (`CSuperblock::IsExpired`).
    pub fn funded_height(&self, hash_hex: &str, tip: u32) -> Option<u32> {
        let cycle = governance_params(self.network).superblock_cycle;
        self.objects()
            .filter_map(|o| match &o.kind {
                ObjectKind::Trigger(t)
                    if t.event_block_height <= tip
                        && tip <= t.event_block_height.saturating_add(cycle)
                        && t.proposal_hashes.iter().any(|h| h == hash_hex) =>
                {
                    Some(t.event_block_height)
                }
                _ => None,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vote::{VoteOutcome, to_recoverable_compact};
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

    /// A funding vote by voting key `key` for outpoint `(op, 0)`.
    fn vote(parent: [u8; 32], key: u8, op: u8, outcome: VoteOutcome, time: i64) -> GovernanceVote {
        let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
        let mut v = GovernanceVote::funding(
            OutPoint::new(dashcore::Txid::from_byte_array([op; 32]), 0),
            parent,
            outcome,
            time,
        );
        let d = v.signing_digest(Network::Regtest);
        let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
            .unwrap()
            .to_vec();
        v
    }

    fn proposal(name: &str) -> GovernanceObject {
        let data = format!(
            r#"{{"end_epoch":{},"name":"{name}","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        GovernanceObject::new_proposal(1, NOW, data.into_bytes())
    }

    fn entry(key: u8, weight: u32) -> ListEntry {
        ListEntry {
            voting_key_id: key_id(key),
            weight,
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
        // The wallet's own objects count at once.
        let own = proposal("b");
        assert_eq!(s.add_object(own.clone(), None, NOW), Ok(true));
        assert!(s.object(&own.hash()).is_some());
        // Invalid data is refused.
        let bad = GovernanceObject::new_proposal(1, NOW, br#"{"type":1}"#.to_vec());
        assert!(s.add_object(bad, peer(1), NOW).is_err());
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
        let op = v1.masternode_outpoint;
        assert_eq!(s.current_vote(&h, &op).unwrap().outcome, VoteOutcome::Yes);
        let v2 = vote(h, 1, 1, VoteOutcome::No, 20);
        assert_eq!(s.add_vote(v2, peer(1), NOW), Ok(()));
        assert_eq!(s.current_vote(&h, &op).unwrap().outcome, VoteOutcome::No);
        assert_eq!(s.vote_count(), 1);
        assert!(!s.has_vote(&v1.hash()));
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
            Err(VoteRejection::Obsolete)
        );
        assert_eq!(
            s.add_vote(vote(h, 1, 1, VoteOutcome::Abstain, 10), None, NOW),
            Ok(())
        );
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
        // A key outside the list does not.
        assert_eq!(
            s.add_vote(vote(h, 3, 3, VoteOutcome::Yes, 10), None, NOW),
            Err(VoteRejection::UnknownVotingKey)
        );
        // Junk signatures are refused and left unseen.
        let mut junk = vote(h, 1, 1, VoteOutcome::Yes, 10);
        junk.signature.truncate(10);
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
    fn resolved_outpoints_need_their_own_key() {
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
        assert_eq!(s.add_vote(v, None, NOW), Err(VoteRejection::WrongVotingKey));
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
    fn votes_on_triggers_are_not_tallied() {
        let hash = "4f6e2b3e7d8f9a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f3a4b";
        let data = format!(
            r#"{{"event_block_height":1520,"payment_addresses":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amounts":"1","proposal_hashes":"{hash}","type":2}}"#
        );
        let t = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let mut s = store(&t, &[entry(1, 1)]);
        let v = vote(t.hash(), 1, 1, VoteOutcome::Yes, 10);
        assert_eq!(s.add_vote(v, None, NOW), Err(VoteRejection::NotTallied));
        assert_eq!(s.funded_height(hash, 1519), None);
        assert_eq!(s.funded_height(hash, 1520), Some(1520));
        // Kept for one cycle (regtest 20 blocks) after its height.
        assert_eq!(s.funded_height(hash, 1540), Some(1520));
        assert_eq!(s.funded_height(hash, 1541), None);
    }

    #[test]
    fn orphans_expire_are_bounded_per_peer_and_evict_the_oldest() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_masternode_list(&[entry(1, 1)]);
        let parent = |i: u32| {
            let mut p = [0u8; 32];
            p[..4].copy_from_slice(&i.to_le_bytes());
            p
        };
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

    #[test]
    fn a_full_orphan_queue_evicts_the_oldest() {
        let mut s = GovernanceStore::new(Network::Regtest);
        s.set_masternode_list(&[entry(1, 1)]);
        let v = |i: u32| {
            let mut p = [0u8; 32];
            p[..4].copy_from_slice(&i.to_le_bytes());
            vote(p, 1, 1, VoteOutcome::Yes, 10)
        };
        // The wallet's own relays have no per-peer bound; fill with them
        // (unsigned: the queue itself checks nothing).
        let oldest = v(0);
        let _ = s.add_vote(oldest.clone(), None, NOW);
        for i in 1..MAX_ORPHAN_VOTES as u32 {
            let mut p = [0u8; 32];
            p[..4].copy_from_slice(&i.to_le_bytes());
            let filler = GovernanceVote::funding(OutPoint::null(), p, VoteOutcome::Yes, 10);
            s.queue_orphan(filler.hash(), filler, None, NOW);
        }
        assert_eq!(s.orphan_count(), MAX_ORPHAN_VOTES);
        let newest = v(MAX_ORPHAN_VOTES as u32);
        let _ = s.add_vote(newest.clone(), None, NOW);
        assert_eq!(s.orphan_count(), MAX_ORPHAN_VOTES);
        assert!(!s.has_vote(&oldest.hash()));
        assert!(s.has_vote(&newest.hash()));
    }
}
