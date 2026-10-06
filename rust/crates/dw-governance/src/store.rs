//! The synced governance objects and current votes, in memory.
//!
//! Objects are kept by hash; for each object only the latest funding vote
//! of every masternode outpoint is kept (Core `mapCurrentMNVotes`), with
//! the voting key id recovered from its signature. Votes whose object has
//! not arrived yet wait (bounded) until it does.

use std::collections::{HashMap, HashSet};

use dashcore::{Network, OutPoint};

use crate::object::{GovernanceObject, OBJECT_TYPE_PROPOSAL, OBJECT_TYPE_TRIGGER, display_hex};
use crate::proposal::{ProposalData, TriggerData, parse_proposal, parse_trigger};
use crate::tally::CurrentVote;
use crate::vote::{GovernanceVote, SIGNAL_FUNDING, VoteOutcome};

/// Votes kept for objects that have not arrived (per store).
const MAX_ORPHAN_VOTES: usize = 50_000;

/// What an object's data says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObjectKind {
    Proposal(ProposalData),
    Trigger(TriggerData),
    /// Another type, or data that does not parse.
    Other,
}

/// A synced object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredObject {
    pub object: GovernanceObject,
    pub kind: ObjectKind,
    /// Display-order hash.
    pub hash_hex: String,
}

/// Why a vote was not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoteRejection {
    /// Already stored.
    Duplicate,
    /// Not a funding vote with an ECDSA signature (trigger votes are
    /// operator-signed and not tallied here).
    NotTallied,
    /// The signature recovers no key.
    BadSignature,
    /// An older vote than the masternode's current one.
    Obsolete,
    /// Kept until its object arrives.
    Orphan,
}

/// Synced objects and current votes.
#[derive(Debug, Default)]
pub struct GovernanceStore {
    objects: HashMap<[u8; 32], StoredObject>,
    /// object hash → outpoint → current funding vote.
    votes: HashMap<[u8; 32], HashMap<OutPoint, CurrentVote>>,
    seen_votes: HashSet<[u8; 32]>,
    orphans: HashMap<[u8; 32], Vec<GovernanceVote>>,
    orphan_count: usize,
    vote_count: u64,
}

impl GovernanceStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Current votes stored, all objects.
    pub fn vote_count(&self) -> u64 {
        self.vote_count
    }

    pub fn has_object(&self, hash: &[u8; 32]) -> bool {
        self.objects.contains_key(hash)
    }

    pub fn has_vote(&self, hash: &[u8; 32]) -> bool {
        self.seen_votes.contains(hash)
    }

    pub fn object(&self, hash: &[u8; 32]) -> Option<&StoredObject> {
        self.objects.get(hash)
    }

    pub fn objects(&self) -> impl Iterator<Item = &StoredObject> {
        self.objects.values()
    }

    /// The current funding votes on `hash`.
    pub fn votes_on(&self, hash: &[u8; 32]) -> impl Iterator<Item = &CurrentVote> {
        self.votes.get(hash).into_iter().flat_map(|m| m.values())
    }

    /// Stores `object`; returns whether it was new. Orphan votes waiting
    /// for it are applied.
    pub fn add_object(&mut self, object: GovernanceObject, network: Network) -> bool {
        let hash = object.hash();
        if self.objects.contains_key(&hash) {
            return false;
        }
        let kind = match object.object_type {
            OBJECT_TYPE_PROPOSAL => parse_proposal(&object.data)
                .map(ObjectKind::Proposal)
                .unwrap_or(ObjectKind::Other),
            OBJECT_TYPE_TRIGGER => parse_trigger(&object.data)
                .map(ObjectKind::Trigger)
                .unwrap_or(ObjectKind::Other),
            _ => ObjectKind::Other,
        };
        self.objects.insert(
            hash,
            StoredObject {
                object,
                kind,
                hash_hex: display_hex(&hash),
            },
        );
        if let Some(waiting) = self.orphans.remove(&hash) {
            self.orphan_count -= waiting.len();
            for v in waiting {
                let _ = self.add_vote(v, network);
            }
        }
        true
    }

    /// Stores `vote` as its masternode's current funding vote on its object.
    pub fn add_vote(&mut self, vote: GovernanceVote, network: Network) -> Result<(), VoteRejection> {
        let hash = vote.hash();
        if self.seen_votes.contains(&hash) {
            return Err(VoteRejection::Duplicate);
        }
        let Some(outcome) = vote.vote_outcome() else {
            return Err(VoteRejection::NotTallied);
        };
        if vote.signal != SIGNAL_FUNDING {
            return Err(VoteRejection::NotTallied);
        }
        if !self.objects.contains_key(&vote.parent_hash) {
            if self.orphan_count < MAX_ORPHAN_VOTES {
                self.orphan_count += 1;
                self.orphans.entry(vote.parent_hash).or_default().push(vote);
            }
            return Err(VoteRejection::Orphan);
        }
        let Some(key_id) = vote.signer_key_id(network) else {
            return Err(VoteRejection::NotTallied);
        };
        let per_object = self.votes.entry(vote.parent_hash).or_default();
        if let Some(current) = per_object.get(&vote.masternode_outpoint)
            && current.time > vote.time
        {
            self.seen_votes.insert(hash);
            return Err(VoteRejection::Obsolete);
        }
        let replaced = per_object
            .insert(
                vote.masternode_outpoint,
                CurrentVote {
                    outpoint: vote.masternode_outpoint,
                    outcome,
                    time: vote.time,
                    key_id,
                    vote_hash: hash,
                },
            )
            .is_some();
        if !replaced {
            self.vote_count += 1;
        }
        self.seen_votes.insert(hash);
        Ok(())
    }

    /// The current vote of `outpoint` on `hash`.
    pub fn current_vote(&self, hash: &[u8; 32], outpoint: &OutPoint) -> Option<&CurrentVote> {
        self.votes.get(hash)?.get(outpoint)
    }

    /// Records a vote the wallet cast before any peer echoes it back.
    pub fn record_own_vote(&mut self, vote: &GovernanceVote, key_id: [u8; 20]) {
        let Some(outcome) = vote.vote_outcome() else {
            return;
        };
        self.seen_votes.insert(vote.hash());
        let per_object = self.votes.entry(vote.parent_hash).or_default();
        if per_object
            .insert(
                vote.masternode_outpoint,
                CurrentVote {
                    outpoint: vote.masternode_outpoint,
                    outcome,
                    time: vote.time,
                    key_id,
                    vote_hash: vote.hash(),
                },
            )
            .is_none()
        {
            self.vote_count += 1;
        }
    }

    /// The highest superblock height ≤ `tip` whose synced trigger pays
    /// `hash_hex` (dash-qt `getProposalFundedHeight`).
    pub fn funded_height(&self, hash_hex: &str, tip: u32) -> Option<u32> {
        self.objects
            .values()
            .filter_map(|o| match &o.kind {
                ObjectKind::Trigger(t)
                    if t.event_block_height <= tip
                        && t.proposal_hashes.iter().any(|h| h == hash_hex) =>
                {
                    Some(t.event_block_height)
                }
                _ => None,
            })
            .max()
    }

    /// The proposals, for the list.
    pub fn proposals(&self) -> impl Iterator<Item = (&StoredObject, &ProposalData)> {
        self.objects.values().filter_map(|o| match &o.kind {
            ObjectKind::Proposal(p) => Some((o, p)),
            _ => None,
        })
    }

    /// Hashes of proposals whose end epoch is after `now` (the ones whose
    /// votes the sync downloads).
    pub fn current_proposals(&self, now: i64) -> Vec<[u8; 32]> {
        let mut out: Vec<[u8; 32]> = self
            .objects
            .iter()
            .filter_map(|(h, o)| match &o.kind {
                ObjectKind::Proposal(p) if p.end_epoch.is_none_or(|e| e > now) => Some(*h),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// The outcome of `outpoint`'s current vote on `hash`.
    pub fn outcome_of(&self, hash: &[u8; 32], outpoint: &OutPoint) -> Option<VoteOutcome> {
        self.current_vote(hash, outpoint).map(|v| v.outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vote::to_recoverable_compact;
    use dashcore::hashes::{Hash, hash160};
    use dashcore::secp256k1::{Message, PublicKey, Secp256k1, SecretKey};

    fn signed_vote(parent: [u8; 32], n: u8, outcome: VoteOutcome, time: i64) -> (GovernanceVote, [u8; 20]) {
        let sk = SecretKey::from_slice(&[n; 32]).unwrap();
        let secp = Secp256k1::new();
        let pk = PublicKey::from_secret_key(&secp, &sk);
        let mut v = GovernanceVote::funding(
            OutPoint::new(dashcore::Txid::from_byte_array([n; 32]), 0),
            parent,
            outcome,
            time,
        );
        let d = v.signing_digest(Network::Regtest);
        let s = secp.sign_ecdsa(&Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d).unwrap().to_vec();
        (v, hash160::Hash::hash(&pk.serialize()).to_byte_array())
    }

    #[test]
    fn test_qt_128_orphans_wait_for_their_object_and_latest_vote_wins() {
        let obj = GovernanceObject::new_proposal(1, 1, br#"{"name":"a","end_epoch":100,"type":1}"#.to_vec());
        let h = obj.hash();
        let mut s = GovernanceStore::new();
        let (v1, key) = signed_vote(h, 1, VoteOutcome::Yes, 10);
        assert_eq!(s.add_vote(v1.clone(), Network::Regtest), Err(VoteRejection::Orphan));
        assert!(s.add_object(obj.clone(), Network::Regtest));
        assert!(!s.add_object(obj, Network::Regtest));
        assert_eq!(s.vote_count(), 1);
        assert_eq!(s.current_vote(&h, &v1.masternode_outpoint).unwrap().key_id, key);
        let (v2, _) = signed_vote(h, 1, VoteOutcome::No, 20);
        assert_eq!(s.add_vote(v2.clone(), Network::Regtest), Ok(()));
        assert_eq!(s.outcome_of(&h, &v2.masternode_outpoint), Some(VoteOutcome::No));
        let (v0, _) = signed_vote(h, 1, VoteOutcome::Abstain, 5);
        assert_eq!(s.add_vote(v0, Network::Regtest), Err(VoteRejection::Obsolete));
        assert_eq!(s.add_vote(v2, Network::Regtest), Err(VoteRejection::Duplicate));
        assert_eq!(s.vote_count(), 1);
        assert_eq!(s.current_proposals(50), vec![h]);
        assert!(s.current_proposals(100).is_empty());
    }

    #[test]
    fn test_qt_129_funded_height_from_triggers() {
        let p = GovernanceObject::new_proposal(1, 1, br#"{"name":"a","type":1}"#.to_vec());
        let hex = display_hex(&p.hash());
        let mut t = GovernanceObject::new_proposal(1, 2, Vec::new());
        t.object_type = OBJECT_TYPE_TRIGGER;
        t.data = format!(r#"{{"event_block_height":1520,"proposal_hashes":"{hex}","type":2}}"#).into_bytes();
        let mut s = GovernanceStore::new();
        s.add_object(p, Network::Regtest);
        s.add_object(t, Network::Regtest);
        assert_eq!(s.funded_height(&hex, 1519), None);
        assert_eq!(s.funded_height(&hex, 1520), Some(1520));
    }
}
