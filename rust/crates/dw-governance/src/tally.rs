//! Weighted funding tallies, the passing threshold, the fundable set and
//! dash-qt's proposal statuses (research 02 §11.1, Core
//! `CGovernanceObject::CountMatchingVotes`, `GetApprovedProposals`, node
//! `getFundableProposalHashes`, dash-qt `Proposal::status`).
//!
//! # Which votes count (SPV)
//!
//! Core counts the latest funding vote of every collateral outpoint that is
//! a masternode of the tip list, weighted 1 (regular) or 4 (EvoNode). The
//! SPV masternode list (DIP-4 simplified entries) has no collateral
//! outpoints, so a vote is matched to masternodes through the key that
//! signed it instead: the voting key id recovered from its signature must be
//! the voting key of at least one masternode of the list. A key that `n`
//! masternodes share counts for at most `n` distinct outpoints (their latest
//! votes first), so a key holder can never cast more weight than its
//! masternodes have. What this cannot tell apart: a vote from a stale
//! outpoint whose key is reused by a newer masternode (counted while the
//! key has a free slot), and the weight of a key shared by regular
//! masternodes and EvoNodes (counted as 1 each).

use std::collections::HashMap;

use dashcore::OutPoint;

use crate::params::EVONODE_VOTE_WEIGHT;
use crate::vote::VoteOutcome;

/// One entry of the SPV masternode list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasternodeEntry {
    /// Internal order.
    pub pro_tx_hash: [u8; 32],
    pub voting_key_id: [u8; 20],
    /// Not PoSe-banned.
    pub is_valid: bool,
    pub is_evonode: bool,
}

/// Masternodes that share one voting key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyGroup {
    members: usize,
    /// The weight of each vote with this key: 4 when every member is an
    /// EvoNode, 1 otherwise.
    weight: u32,
    evonode: bool,
}

/// The tip masternode list indexed by voting key.
#[derive(Debug, Clone, Default)]
pub struct MasternodeIndex {
    by_key: HashMap<[u8; 20], KeyGroup>,
    pub valid_regular: u32,
    pub valid_evonodes: u32,
    /// Core `GetCounts().m_valid_weighted`.
    pub valid_weighted: u32,
}

impl MasternodeIndex {
    pub fn new(entries: &[MasternodeEntry]) -> Self {
        let mut idx = Self::default();
        for e in entries {
            let g = idx.by_key.entry(e.voting_key_id).or_insert(KeyGroup {
                members: 0,
                weight: if e.is_evonode { EVONODE_VOTE_WEIGHT } else { 1 },
                evonode: e.is_evonode,
            });
            g.members += 1;
            if g.evonode != e.is_evonode {
                g.weight = 1;
                g.evonode = false;
            }
            if e.is_valid {
                if e.is_evonode {
                    idx.valid_evonodes += 1;
                    idx.valid_weighted += EVONODE_VOTE_WEIGHT;
                } else {
                    idx.valid_regular += 1;
                    idx.valid_weighted += 1;
                }
            }
        }
        idx
    }

    /// Whether any masternode of the list votes with `key_id`.
    pub fn knows_key(&self, key_id: &[u8; 20]) -> bool {
        self.by_key.contains_key(key_id)
    }

    /// Core's passing threshold: `max(min_quorum, valid_weighted / 10)`.
    pub fn threshold(&self, min_quorum: u32) -> u32 {
        min_quorum.max(self.valid_weighted / 10)
    }
}

/// A masternode's current funding vote on one object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentVote {
    pub outpoint: OutPoint,
    pub outcome: VoteOutcome,
    pub time: i64,
    /// The voting key id recovered from the signature.
    pub key_id: [u8; 20],
    /// The vote's hash (Core `CGovernanceVote::GetHash`), internal order.
    pub vote_hash: [u8; 32],
}

/// Weighted funding votes of one proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tally {
    pub yes: u32,
    pub no: u32,
    pub abstain: u32,
    /// Distinct voters (Core `GetUniqueVoterCount`).
    pub regular_voters: u32,
    pub evonode_voters: u32,
}

impl Tally {
    /// Core `GetAbsoluteYesCount`.
    pub fn absolute_yes(&self) -> i64 {
        i64::from(self.yes) - i64::from(self.no)
    }
}

/// The votes that count and their weights (see the module docs).
pub fn counted_votes<'a>(
    votes: impl IntoIterator<Item = &'a CurrentVote>,
    index: &MasternodeIndex,
) -> Vec<(&'a CurrentVote, u32, bool)> {
    let mut by_key: HashMap<[u8; 20], Vec<&CurrentVote>> = HashMap::new();
    for v in votes {
        if index.knows_key(&v.key_id) {
            by_key.entry(v.key_id).or_default().push(v);
        }
    }
    let mut out = Vec::new();
    for (key, mut list) in by_key {
        let g = index.by_key[&key];
        // Latest first; ties by outpoint for a stable choice.
        list.sort_by(|a, b| b.time.cmp(&a.time).then(a.outpoint.cmp(&b.outpoint)));
        list.truncate(g.members);
        out.extend(list.into_iter().map(|v| (v, g.weight, g.evonode)));
    }
    out
}

/// The tally of one proposal's current funding votes.
pub fn tally<'a>(
    votes: impl IntoIterator<Item = &'a CurrentVote>,
    index: &MasternodeIndex,
) -> Tally {
    let mut t = Tally::default();
    for (v, weight, evonode) in counted_votes(votes, index) {
        match v.outcome {
            VoteOutcome::Yes => t.yes += weight,
            VoteOutcome::No => t.no += weight,
            VoteOutcome::Abstain => t.abstain += weight,
        }
        if evonode {
            t.evonode_voters += 1;
        } else {
            t.regular_voters += 1;
        }
    }
    t
}

/// A proposal in the budget computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetCandidate {
    /// Display order (the tie-break compares it as Core's `uint256`).
    pub hash: String,
    pub payment_amount: u64,
    pub absolute_yes: i64,
}

/// The budget the passing proposals fit into: node
/// `getFundableProposalHashes` over Core `GetApprovedProposals` (absolute
/// yes ≥ threshold, most yes first, ties by the larger hash), each added
/// while the running total stays within `budget`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Fundable {
    pub hashes: Vec<String>,
    pub allocated: u64,
}

pub fn fundable(candidates: &[BudgetCandidate], threshold: u32, budget: u64) -> Fundable {
    let mut approved: Vec<&BudgetCandidate> = candidates
        .iter()
        .filter(|c| c.absolute_yes >= i64::from(threshold))
        .collect();
    approved.sort_by(|a, b| {
        b.absolute_yes
            .cmp(&a.absolute_yes)
            .then_with(|| b.hash.cmp(&a.hash))
    });
    let mut out = Fundable::default();
    for c in approved {
        if out.allocated.saturating_add(c.payment_amount) > budget {
            continue;
        }
        out.allocated += c.payment_amount;
        out.hashes.push(c.hash.clone());
    }
    out
}

/// dash-qt `ProposalStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Funded,
    Lapsed,
    Confirming,
    Pending,
    Passing,
    Failing,
    Voting,
    Unfunded,
}

impl Status {
    /// dash-qt's sort group (`ProposalModel::data`, `EditRole`).
    pub fn sort_group(self) -> u8 {
        match self {
            Self::Funded => 0,
            Self::Passing => 1,
            Self::Unfunded => 2,
            Self::Voting => 3,
            Self::Confirming => 4,
            Self::Pending => 5,
            Self::Failing => 6,
            Self::Lapsed => 7,
        }
    }
}

/// What dash-qt's status rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusInput {
    pub funded_height: Option<u32>,
    /// `now ≥ end_epoch`.
    pub lapsed: bool,
    pub collateral_confirmations: u32,
    pub required_confirmations: u32,
    /// Relayed to the network (a synced object, or a submitted one).
    pub broadcast: bool,
    pub in_maturity_window: bool,
    pub absolute_yes: i64,
    pub threshold: u32,
    pub fundable: bool,
}

/// dash-qt `Proposal::status`, in its evaluation order.
pub fn status(i: &StatusInput) -> Status {
    if i.funded_height.is_some() {
        Status::Funded
    } else if i.lapsed {
        Status::Lapsed
    } else if i.collateral_confirmations < i.required_confirmations {
        Status::Confirming
    } else if !i.broadcast {
        Status::Pending
    } else if i.in_maturity_window {
        if i.fundable {
            Status::Passing
        } else {
            Status::Failing
        }
    } else if i.absolute_yes < i64::from(i.threshold) {
        Status::Voting
    } else if i.fundable {
        Status::Passing
    } else {
        Status::Unfunded
    }
}

/// dash-qt's list order: status group, then the vote deficit (smallest
/// first) clamped to 16 bits.
pub fn sort_key(status: Status, absolute_yes: i64, threshold: u32) -> i64 {
    let deficit = (i64::from(threshold) - absolute_yes).clamp(-32768, 32767);
    (i64::from(status.sort_group()) << 16) + deficit
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::hashes::Hash;

    fn op(n: u8) -> OutPoint {
        OutPoint::new(dashcore::Txid::from_byte_array([n; 32]), 0)
    }

    fn mn(key: u8, evo: bool, valid: bool) -> MasternodeEntry {
        MasternodeEntry {
            pro_tx_hash: [key; 32],
            voting_key_id: [key; 20],
            is_valid: valid,
            is_evonode: evo,
        }
    }

    fn vote(n: u8, key: u8, outcome: VoteOutcome, time: i64) -> CurrentVote {
        CurrentVote {
            outpoint: op(n),
            outcome,
            time,
            key_id: [key; 20],
            vote_hash: [n; 32],
        }
    }

    #[test]
    fn test_qt_129_weights_threshold_and_key_caps() {
        let list = [
            mn(1, false, true),
            mn(2, true, true),
            mn(3, false, false),
            mn(4, false, true),
        ];
        let idx = MasternodeIndex::new(&list);
        assert_eq!(
            (idx.valid_regular, idx.valid_evonodes, idx.valid_weighted),
            (2, 1, 6)
        );
        assert_eq!(idx.threshold(1), 1);
        assert_eq!(idx.threshold(10), 10);
        let votes = [
            vote(1, 1, VoteOutcome::Yes, 10),
            // A second outpoint with key 1 exceeds its one masternode: the
            // older one is dropped.
            vote(9, 1, VoteOutcome::No, 5),
            vote(2, 2, VoteOutcome::Yes, 10),
            // Banned masternodes' votes still count (Core).
            vote(3, 3, VoteOutcome::No, 10),
            vote(4, 4, VoteOutcome::Abstain, 10),
            // Unknown key: not counted.
            vote(5, 7, VoteOutcome::Yes, 10),
        ];
        let t = tally(&votes, &idx);
        assert_eq!((t.yes, t.no, t.abstain), (5, 1, 1));
        assert_eq!((t.regular_voters, t.evonode_voters), (3, 1));
        assert_eq!(t.absolute_yes(), 4);
    }

    #[test]
    fn test_qt_129_mixed_key_counts_one_each() {
        let list = [mn(1, false, true), mn(1, true, true)];
        let idx = MasternodeIndex::new(&list);
        let votes = [
            vote(1, 1, VoteOutcome::Yes, 1),
            vote(2, 1, VoteOutcome::Yes, 2),
        ];
        assert_eq!(tally(&votes, &idx).yes, 2);
    }

    #[test]
    fn test_qt_134_fundable_set_respects_budget_and_order() {
        let c = |h: &str, amt: u64, yes: i64| BudgetCandidate {
            hash: h.into(),
            payment_amount: amt,
            absolute_yes: yes,
        };
        let cands = [
            c("aa", 60, 10),
            c("bb", 50, 10),
            c("cc", 30, 5),
            c("dd", 10, 0),
        ];
        let f = fundable(&cands, 1, 100);
        // bb before aa (same yes, larger hash); aa does not fit after bb;
        // cc fits; dd is below the threshold.
        assert_eq!(f.hashes, vec!["bb", "cc"]);
        assert_eq!(f.allocated, 80);
    }

    #[test]
    fn test_qt_129_status_order_follows_dash_qt() {
        let base = StatusInput {
            funded_height: None,
            lapsed: false,
            collateral_confirmations: 6,
            required_confirmations: 6,
            broadcast: true,
            in_maturity_window: false,
            absolute_yes: 5,
            threshold: 3,
            fundable: true,
        };
        assert_eq!(status(&base), Status::Passing);
        assert_eq!(
            status(&StatusInput {
                fundable: false,
                ..base
            }),
            Status::Unfunded
        );
        assert_eq!(
            status(&StatusInput {
                absolute_yes: 2,
                ..base
            }),
            Status::Voting
        );
        assert_eq!(
            status(&StatusInput {
                in_maturity_window: true,
                fundable: false,
                absolute_yes: 0,
                ..base
            }),
            Status::Failing
        );
        assert_eq!(
            status(&StatusInput {
                broadcast: false,
                ..base
            }),
            Status::Pending
        );
        assert_eq!(
            status(&StatusInput {
                collateral_confirmations: 2,
                broadcast: false,
                ..base
            }),
            Status::Confirming
        );
        assert_eq!(
            status(&StatusInput {
                lapsed: true,
                ..base
            }),
            Status::Lapsed
        );
        assert_eq!(
            status(&StatusInput {
                funded_height: Some(1),
                lapsed: true,
                ..base
            }),
            Status::Funded
        );
        assert!(sort_key(Status::Funded, 0, 0) < sort_key(Status::Passing, 100, 0));
        assert!(sort_key(Status::Passing, 10, 3) < sort_key(Status::Passing, 5, 3));
    }
}
