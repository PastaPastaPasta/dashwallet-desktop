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
    /// (indexes 0–19), with each key's latest vote seen on `proposal`.
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
        let mut latest: HashMap<[u8; 20], &CurrentVote> = HashMap::new();
        for v in store.votes_on(&key) {
            let slot = latest.entry(v.key_id).or_insert(v);
            if v.time > slot.time {
                *slot = v;
            }
        }
        let mut out: Vec<VotingMasternode> = list
            .entries
            .iter()
            .filter_map(|e| {
                let key_index = *indexes.get(&e.voting_key_id)?;
                let vote = latest.get(&e.voting_key_id);
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
