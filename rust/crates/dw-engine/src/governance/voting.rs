//! Voting (mg-engine §2.4): the wallet's voting masternodes, relaying a
//! vote signed elsewhere, and the `vote` stub until MG-05 (MG-02 D6).

use std::collections::HashMap;
use std::str::FromStr;

use base64::Engine as _;
use dashcore::hashes::{Hash, hash160};
use dashcore::{OutPoint, Txid};
use dw_governance::object::display_hex;
use dw_governance::store::{GovernanceStore, ObjectKind, VoteRejection};
use dw_governance::tally::CurrentVote;
use dw_governance::vote::GovernanceVote;

use super::reads::{now, parse_hash};
use super::records::{MasternodeType, RawVote, VoteOutcome, VoteRelayed, VotingMasternode};
use super::sync::ListSnapshot;
use super::{Governance, GovernanceError, stub};
use crate::masternode_keys::MasternodeKeyRole;
use crate::{EngineError, WalletId};

/// The voting-key indexes the wallet looks at (the keychain view's
/// provider-key gap, MG-02 §4.3).
pub const VOTING_KEY_INDEXES: u32 = 20;

impl Governance {
    /// The masternodes of the tip list whose voting key the wallet derives
    /// (indexes 0–19), with each one's own vote seen on `proposal` and, for
    /// a shared key, the key's latest (D49).
    pub async fn voting_masternodes(
        &self,
        wallet: WalletId,
        proposal: String,
    ) -> Result<Vec<VotingMasternode>, GovernanceError> {
        let key = parse_hash(&proposal)?;
        self.runtime()?;
        let keys = self
            .session
            .masternode_keys(wallet, MasternodeKeyRole::Voting, 0, VOTING_KEY_INDEXES)
            .await?;
        let indexes: HashMap<[u8; 20], u32> = keys
            .iter()
            .filter_map(|k| {
                let pk = hex::decode(&k.public_key_hex).ok()?;
                Some((hash160::Hash::hash(&pk).to_byte_array(), k.index))
            })
            .collect();
        let rt = self.runtime()?;
        let list = rt.list();
        if list.entries.is_empty() {
            return Err(GovernanceError::ListUnavailable);
        }
        let store = rt.shared.store();
        let mut holders: HashMap<[u8; 20], u32> = HashMap::new();
        for e in &list.entries {
            *holders.entry(e.voting_key_id).or_default() += 1;
        }
        let mut out: Vec<VotingMasternode> = list
            .entries
            .iter()
            .filter_map(|e| {
                let key_index = *indexes.get(&e.voting_key_id)?;
                let key_holders = holders[&e.voting_key_id];
                let vote = own_vote(&store, &key, &e.pro_tx_hash, &e.voting_key_id, key_holders);
                let key_last = (key_holders > 1)
                    .then(|| latest_under(&store, &key, &e.voting_key_id))
                    .flatten();
                Some(VotingMasternode {
                    pro_tx_hash: display_hex(&e.pro_tx_hash),
                    kind: if e.is_evonode {
                        MasternodeType::Evonode
                    } else {
                        MasternodeType::Regular
                    },
                    weight: ListSnapshot::weight(e),
                    valid: e.is_valid,
                    key_index,
                    current_vote: vote.map(|v| VoteOutcome::from(v.outcome)),
                    vote_time: vote.map(|v| v.time),
                    key_holders,
                    key_last_vote: key_last.map(|v| VoteOutcome::from(v.outcome)),
                    key_last_vote_time: key_last.map(|v| v.time),
                })
            })
            .collect();
        out.sort_by(|a, b| {
            a.key_index
                .cmp(&b.key_index)
                .then(a.pro_tx_hash.cmp(&b.pro_tx_hash))
        });
        Ok(out)
    }

    /// Core's `voteraw`: a funding vote signed elsewhere, checked as a
    /// peer's vote is, stored, and announced to the peers once the run is
    /// synced (MG-02 D24). `governance.no_peers` when none is connected;
    /// the vote is kept and announced on the next connection.
    pub async fn relay_vote(&self, vote: RawVote) -> Result<VoteRelayed, GovernanceError> {
        let rt = self.runtime()?;
        if !rt.enabled() {
            return Err(GovernanceError::SyncDisabled);
        }
        // Its obsolete and hourly checks need every current vote (D40).
        self.require_synced()?;
        let parent = parse_hash(&vote.proposal)?;
        let outpoint = parse_outpoint(&vote.collateral)?;
        // Core's `voteraw` order: the object, the base64, then the vote's
        // checks (`GovernanceStore::add_own_vote`). Votes on triggers are
        // refused where Core would check them (D31).
        let known = |store: &GovernanceStore| match store.object(&parent).map(|o| &o.kind) {
            None => Err(rejected("unknown_object")),
            Some(ObjectKind::Trigger(_)) => Err(rejected("not_a_proposal")),
            Some(ObjectKind::Proposal(_)) => Ok(()),
        };
        if rt.shared.store().object(&parent).is_none() {
            return Err(rejected("unknown_object"));
        }
        let signature = base64::engine::general_purpose::STANDARD
            .decode(vote.signature.as_bytes())
            .map_err(|_| EngineError::InvalidArgument("signature is not base64".into()))?;
        let mut v = GovernanceVote::funding(outpoint, parent, vote.outcome.into(), vote.time);
        v.signature = signature;
        let hash = v.hash();
        {
            let mut store = rt.shared.store();
            known(&store)?;
            store
                .add_own_vote(v.clone(), now())
                .map_err(|r| rejected(rejection_reason(r)))?;
        }
        (rt.shared.on_change)();
        let peers = rt.shared.relay_vote(&v);
        if peers == 0 {
            return Err(GovernanceError::NoPeers);
        }
        Ok(VoteRelayed {
            hash: display_hex(&hash),
            peers,
        })
    }

    /// Signs and relays a funding vote per masternode (proTxHash) with its
    /// voting key. Stub until MG-05 (the `Governance` grant purpose).
    pub async fn vote(
        &self,
        wallet: WalletId,
        proposal: String,
        outcome: VoteOutcome,
        masternodes: Vec<String>,
        grant: String,
    ) -> Result<Vec<VoteRelayed>, GovernanceError> {
        let _ = (wallet, proposal, outcome, masternodes, grant);
        self.runtime()?;
        stub("Governance.vote")
    }
}

/// The current vote on `proposal` of the masternode `pro_tx_hash`, whose
/// voting key `key` `holders` entries of the tip list hold: one under the
/// key whose outpoint is resolved to it, or, when no other entry holds the
/// key, one whose outpoint is not resolved to another masternode (D49).
fn own_vote<'a>(
    store: &'a GovernanceStore,
    proposal: &[u8; 32],
    pro_tx_hash: &[u8; 32],
    key: &[u8; 20],
    holders: u32,
) -> Option<&'a CurrentVote> {
    store
        .votes_on(proposal)
        .filter(|v| &v.key_id == key)
        .filter(|v| match store.resolved(&v.outpoint) {
            Some(r) => &r.pro_tx_hash == pro_tx_hash && r.voting_key_id == v.key_id,
            None => holders == 1,
        })
        .max_by_key(|v| v.time)
}

/// The latest current vote on `proposal` under `key`, for any outpoint.
fn latest_under<'a>(
    store: &'a GovernanceStore,
    proposal: &[u8; 32],
    key: &[u8; 20],
) -> Option<&'a CurrentVote> {
    store
        .votes_on(proposal)
        .filter(|v| &v.key_id == key)
        .max_by_key(|v| v.time)
}

fn rejected(reason: &str) -> GovernanceError {
    GovernanceError::VoteRejected {
        reason: reason.into(),
    }
}

/// The `vote_rejected` reason of a store rejection (mg-engine §4). The
/// parent is a known proposal by then, so an ignored vote is one on an
/// expired proposal.
fn rejection_reason(r: VoteRejection) -> &'static str {
    match r {
        VoteRejection::Duplicate => "duplicate",
        VoteRejection::Ignored => "expired",
        VoteRejection::Unverifiable | VoteRejection::Orphan => "unknown_object",
        VoteRejection::Malformed => "malformed",
        VoteRejection::BadSignature => "bad_signature",
        VoteRejection::UnknownVotingKey => "unknown_voting_key",
        VoteRejection::WrongVotingKey => "wrong_voting_key",
        VoteRejection::TooNew => "too_far_ahead",
        VoteRejection::Obsolete => "obsolete",
        VoteRejection::SameTimestamp => "same_timestamp",
        VoteRejection::TooOften => "too_often",
        VoteRejection::Banned => "masternode_not_found",
        VoteRejection::KeyLimit => "key_limit",
    }
}

/// `"<txid>-<n>"` (Core `ToStringShort`).
fn parse_outpoint(text: &str) -> Result<OutPoint, GovernanceError> {
    let bad = || EngineError::InvalidArgument(format!("not an outpoint \"<txid>-<n>\": {text:?}"));
    let (txid, n) = text.rsplit_once('-').ok_or_else(bad)?;
    let txid = Txid::from_str(txid).map_err(|_| bad())?;
    let vout = n.parse::<u32>().map_err(|_| bad())?;
    Ok(OutPoint::new(txid, vout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::Network;
    use dashcore::secp256k1::{Message, PublicKey, SecretKey};
    use dw_governance::object::GovernanceObject;
    use dw_governance::store::{ListEntry, ResolvedMasternode};
    use dw_governance::vote::{VoteOutcome as Outcome, to_recoverable_compact};

    const NOW: i64 = 1_000_000;

    fn key_id(n: u8) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
        hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
    }

    /// A funding vote by voting key `key` for outpoint `(op, 0)`.
    fn signed(parent: [u8; 32], key: u8, op: u8, outcome: Outcome, time: i64) -> GovernanceVote {
        let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
        let outpoint = OutPoint::new(Txid::from_byte_array([op; 32]), 0);
        let mut v = GovernanceVote::funding(outpoint, parent, outcome, time);
        let d = v.signing_digest(Network::Regtest);
        let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
            .unwrap()
            .to_vec();
        v
    }

    /// A store holding one proposal, with a list in which `shared` entries
    /// hold voting key 1 and one entry holds key 2.
    fn store_with(shared: usize) -> (GovernanceStore, [u8; 32]) {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let hash = p.hash();
        store.add_object(p, None, NOW).unwrap();
        let mut list = vec![
            ListEntry {
                voting_key_id: key_id(1),
                weight: Some(1),
                valid: true,
            };
            shared
        ];
        list.push(ListEntry {
            voting_key_id: key_id(2),
            weight: Some(1),
            valid: true,
        });
        store.set_masternode_list(&list);
        (store, hash)
    }

    /// Sol r1 Major 5: two masternodes share voting key 1. A vote under the
    /// key is not reported as either one's own vote, whether both of them
    /// voted (Yes, then No later) or only one did; a key only one
    /// masternode holds votes for that masternode.
    #[test]
    fn a_shared_keys_vote_is_not_reported_as_each_holders_own() {
        let (a, b) = ([0xa; 32], [0xb; 32]);
        let (mut store, h) = store_with(2);
        store
            .add_vote(signed(h, 1, 1, Outcome::Yes, NOW - 200), None, NOW)
            .unwrap();
        store
            .add_vote(signed(h, 1, 2, Outcome::No, NOW - 100), None, NOW)
            .unwrap();
        for mn in [&a, &b] {
            assert_eq!(own_vote(&store, &h, mn, &key_id(1), 2), None);
        }
        let last = latest_under(&store, &h, &key_id(1)).unwrap();
        assert_eq!((last.outcome, last.time), (Outcome::No, NOW - 100));
        // Resolved, each outpoint's vote is its masternode's own.
        let resolved = |pro_tx_hash, op: u8| {
            (
                OutPoint::new(Txid::from_byte_array([op; 32]), 0),
                ResolvedMasternode {
                    pro_tx_hash,
                    voting_key_id: key_id(1),
                    weight: 1,
                },
            )
        };
        store.set_resolved(HashMap::from([resolved(a, 1), resolved(b, 2)]));
        let own = |mn| own_vote(&store, &h, mn, &key_id(1), 2).map(|v| v.outcome);
        assert_eq!((own(&a), own(&b)), (Some(Outcome::Yes), Some(Outcome::No)));

        let (mut store, h) = store_with(2);
        store
            .add_vote(signed(h, 1, 1, Outcome::Yes, NOW - 200), None, NOW)
            .unwrap();
        for mn in [&a, &b] {
            assert_eq!(own_vote(&store, &h, mn, &key_id(1), 2), None);
        }
        let last = latest_under(&store, &h, &key_id(1)).unwrap();
        assert_eq!(last.outcome, Outcome::Yes);

        store
            .add_vote(signed(h, 2, 3, Outcome::No, NOW - 100), None, NOW)
            .unwrap();
        let own = own_vote(&store, &h, &[0xc; 32], &key_id(2), 1).unwrap();
        assert_eq!((own.outcome, own.time), (Outcome::No, NOW - 100));
    }

    #[test]
    fn outpoints_parse_as_core_prints_them() {
        let txid = "aa".repeat(32);
        let o = parse_outpoint(&format!("{txid}-7")).unwrap();
        assert_eq!(o.vout, 7);
        assert_eq!(
            dw_governance::object::outpoint_short(&o),
            format!("{txid}-7")
        );
        for bad in [
            "",
            "x-1",
            &txid.to_string(),
            &format!("{txid}--1"),
            &format!("{txid}-a"),
        ] {
            assert!(parse_outpoint(bad).is_err(), "{bad}");
        }
    }
}
