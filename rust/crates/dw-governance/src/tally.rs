//! Weighted funding tallies, the passing threshold, the fundable set and
//! dash-qt's proposal statuses (research 02 §11.1, Core
//! `CGovernanceObject::CountMatchingVotes`, `GetApprovedProposals`, node
//! `getFundableProposalHashes`, dash-qt `Proposal::status`).
//!
//! Core counts the latest funding vote of every collateral outpoint that is
//! a masternode of the tip list, weighted 1 (regular) or 4 (evonode). The
//! SPV list carries no outpoints, so the store keeps votes by (outpoint,
//! voting key their signature recovers), and [`ranged_tally`] attributes
//! them to that key's masternodes: exactly when the key's masternodes share
//! one weight and no more outpoints vote with it than it has masternodes,
//! as a range otherwise (MG-02 D1, D2; MG-00 D45). An outpoint claimed by
//! more than one key weighs between 0 and the highest weight for each claim
//! (MG-02 D19). Outpoints the wallet resolved (MG-03) count exactly.

use std::collections::{HashMap, HashSet};

use dashcore::OutPoint;

use crate::params::EVONODE_VOTE_WEIGHT;
use crate::vote::VoteOutcome;

/// Core's passing threshold: `max(min_quorum, valid_weighted / 10)`, where
/// `valid_weighted` is `GetCounts().m_valid_weighted` of the tip list.
pub fn threshold(valid_weighted: u32, min_quorum: u32) -> u32 {
    min_quorum.max(valid_weighted / 10)
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

/// The masternodes of the tip list that hold one voting key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyWeights {
    pub count: u32,
    pub regular: u32,
    pub evonodes: u32,
    /// Holders whose type the list does not carry: revoked masternodes
    /// (MG-02 D34).
    pub unknown: u32,
}

impl KeyWeights {
    pub fn one(weight: Option<u32>) -> Self {
        let mut k = Self {
            count: 0,
            regular: 0,
            evonodes: 0,
            unknown: 0,
        };
        k.add(weight);
        k
    }

    /// Adds a holder of weight 1 or 4, or of an unknown type (`None`).
    pub fn add(&mut self, weight: Option<u32>) {
        self.count += 1;
        match weight {
            Some(EVONODE_VOTE_WEIGHT) => self.evonodes += 1,
            Some(_) => self.regular += 1,
            None => self.unknown += 1,
        }
    }

    pub fn min(&self) -> u32 {
        if self.regular + self.unknown > 0 {
            1
        } else {
            EVONODE_VOTE_WEIGHT
        }
    }

    pub fn max(&self) -> u32 {
        if self.evonodes + self.unknown > 0 {
            EVONODE_VOTE_WEIGHT
        } else {
            1
        }
    }

    /// The most common weight, ties to the lower; a holder of unknown type
    /// counts as regular (D34).
    pub fn common(&self) -> u32 {
        if self.evonodes > self.regular + self.unknown {
            EVONODE_VOTE_WEIGHT
        } else {
            1
        }
    }
}

/// A weight sum the list cannot pin down: its bounds and the estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WeightRange {
    pub min: u32,
    pub max: u32,
    pub estimate: u32,
}

impl WeightRange {
    fn add(&mut self, min: u32, max: u32, estimate: u32) {
        self.min += min;
        self.max += max;
        self.estimate += estimate;
    }
}

/// The funding votes of one proposal, attributed as well as the list
/// allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RangedTally {
    pub yes: WeightRange,
    pub no: WeightRange,
    pub abstain: WeightRange,
    /// Outpoints voting.
    pub voters: u32,
    /// Votes whose weight is not exact.
    pub ranged_votes: u32,
    /// The estimates as a [`Tally`] (statuses and the budget use it).
    pub estimate: Tally,
}

impl RangedTally {
    /// Counts one claim; `new_voter` when it is its outpoint's first.
    fn count(&mut self, outcome: VoteOutcome, min: u32, max: u32, estimate: u32, new_voter: bool) {
        let r = match outcome {
            VoteOutcome::Yes => &mut self.yes,
            VoteOutcome::No => &mut self.no,
            VoteOutcome::Abstain => &mut self.abstain,
        };
        r.add(min, max, estimate);
        if new_voter {
            self.voters += 1;
        }
        if min != max {
            self.ranged_votes += 1;
        }
        match estimate {
            0 => {}
            EVONODE_VOTE_WEIGHT => self.estimate.evonode_voters += 1,
            _ => self.estimate.regular_voters += 1,
        }
    }

    /// The smallest and largest net yes the ranges allow.
    pub fn net_yes_bounds(&self) -> (i64, i64) {
        (
            i64::from(self.yes.min) - i64::from(self.no.max),
            i64::from(self.yes.max) - i64::from(self.no.min),
        )
    }
}

/// Attributes `votes` to masternodes: `resolved` gives the exact weight of
/// an outpoint the wallet resolved, `holders` the masternodes of a voting
/// key. Per key with `n` masternodes not otherwise accounted for and `c`
/// unresolved outpoints claimed with it: if `c ≤ n` each vote weighs
/// between the key's lowest and highest weight, estimated at its most
/// common one; if `c > n` each weighs between 0 and the highest, and only
/// the `n` newest are estimated (at the common weight). An outpoint claimed
/// by several keys (a key holder signing for an outpoint not theirs) counts
/// once as a voter; each claim weighs between 0 and its key's highest
/// weight, and the estimate goes to the one claim whose key has `c ≤ n`, if
/// exactly one has, else to none.
pub fn ranged_tally<'a>(
    votes: impl IntoIterator<Item = &'a CurrentVote>,
    holders: impl Fn(&[u8; 20]) -> Option<KeyWeights>,
    resolved: impl Fn(&OutPoint) -> Option<u32>,
) -> RangedTally {
    let mut t = RangedTally::default();
    for (v, c) in attribute(votes, holders, resolved) {
        t.count(
            v.outcome,
            c.weight.min,
            c.weight.max,
            c.weight.estimate,
            c.new_voter,
        );
    }
    t.estimate.yes = t.yes.estimate;
    t.estimate.no = t.no.estimate;
    t.estimate.abstain = t.abstain.estimate;
    t
}

/// One vote's weight as [`attribute`] sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claim {
    pub weight: WeightRange,
    /// The vote's outpoint was resolved to its masternode.
    pub exact: bool,
    /// The first claim counted for its outpoint.
    pub new_voter: bool,
}

/// The weight of each vote, by the rules of [`ranged_tally`]. Votes whose
/// key no masternode holds are left out.
pub fn attribute<'a>(
    votes: impl IntoIterator<Item = &'a CurrentVote>,
    holders: impl Fn(&[u8; 20]) -> Option<KeyWeights>,
    resolved: impl Fn(&OutPoint) -> Option<u32>,
) -> Vec<(&'a CurrentVote, Claim)> {
    let claim = |min, max, estimate, new_voter| Claim {
        weight: WeightRange { min, max, estimate },
        exact: false,
        new_voter,
    };
    let mut out = Vec::new();
    let mut claims: HashMap<OutPoint, Vec<&CurrentVote>> = HashMap::new();
    let mut resolved_per_key: HashMap<[u8; 20], u32> = HashMap::new();
    let mut claimed_per_key: HashMap<[u8; 20], u32> = HashMap::new();
    for v in votes {
        match resolved(&v.outpoint) {
            Some(w) => {
                out.push((
                    v,
                    Claim {
                        exact: true,
                        ..claim(w, w, w, true)
                    },
                ));
                *resolved_per_key.entry(v.key_id).or_default() += 1;
            }
            None => {
                *claimed_per_key.entry(v.key_id).or_default() += 1;
                claims.entry(v.outpoint).or_default().push(v);
            }
        }
    }
    // Masternodes of the key not taken by a resolved outpoint, and whether
    // the key claims no more outpoints than that.
    let free = |key: &[u8; 20], k: &KeyWeights| {
        k.count
            .saturating_sub(resolved_per_key.get(key).copied().unwrap_or(0))
    };
    let fits = |key: &[u8; 20], k: &KeyWeights| {
        claimed_per_key.get(key).copied().unwrap_or(0) <= free(key, k)
    };
    let mut by_key: HashMap<[u8; 20], Vec<&CurrentVote>> = HashMap::new();
    for (_, group) in claims {
        if let [v] = group[..] {
            by_key.entry(v.key_id).or_default().push(v);
            continue;
        }
        let known: Vec<(&CurrentVote, KeyWeights)> = group
            .iter()
            .filter_map(|v| holders(&v.key_id).map(|k| (*v, k)))
            .collect();
        let fitting: Vec<usize> = (0..known.len())
            .filter(|&i| fits(&known[i].0.key_id, &known[i].1))
            .collect();
        let chosen = match fitting[..] {
            [i] => Some(i),
            _ => None,
        };
        for (i, (v, k)) in known.iter().enumerate() {
            let estimate = if chosen == Some(i) { k.common() } else { 0 };
            out.push((*v, claim(0, k.max(), estimate, i == 0)));
        }
    }
    for (key, mut group) in by_key {
        let Some(k) = holders(&key) else { continue };
        if fits(&key, &k) {
            for v in group {
                out.push((v, claim(k.min(), k.max(), k.common(), true)));
            }
        } else {
            let n = free(&key, &k) as usize;
            group.sort_by(|a, b| b.time.cmp(&a.time).then(a.vote_hash.cmp(&b.vote_hash)));
            for (i, v) in group.into_iter().enumerate() {
                let estimate = if i < n { k.common() } else { 0 };
                out.push((v, claim(0, k.max(), estimate, true)));
            }
        }
    }
    out
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

/// A budget candidate with the range of its absolute yes count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangedCandidate {
    /// Display order, as [`BudgetCandidate::hash`].
    pub hash: String,
    pub payment_amount: u64,
    pub estimate: i64,
    pub low: i64,
    pub high: i64,
}

/// The [`fundable`] set with each candidate at the yes count `count` gives.
pub fn fundable_at(
    candidates: &[RangedCandidate],
    threshold: u32,
    budget: u64,
    count: impl Fn(&RangedCandidate) -> i64,
) -> HashSet<String> {
    let c: Vec<BudgetCandidate> = candidates
        .iter()
        .map(|r| BudgetCandidate {
            hash: r.hash.clone(),
            payment_amount: r.payment_amount,
            absolute_yes: count(r),
        })
        .collect();
    fundable(&c, threshold, budget).hashes.into_iter().collect()
}

/// The candidates whose place in the [`fundable`] set the vote ranges
/// could change (MG-00 D45): a funded one that the set leaves out with it
/// at its low count and every other candidate at its high, an unfunded one
/// that the set takes with it high and the others low, and any whose place
/// differs with every candidate low or every one high (MG-02 D48).
pub fn budget_close_calls(
    candidates: &[RangedCandidate],
    threshold: u32,
    budget: u64,
) -> HashSet<String> {
    let with =
        |count: fn(&RangedCandidate) -> i64| fundable_at(candidates, threshold, budget, count);
    let estimate = with(|r| r.estimate);
    let low = with(|r| r.low);
    let high = with(|r| r.high);
    candidates
        .iter()
        .filter(|me| {
            let funded = estimate.contains(&me.hash);
            if low.contains(&me.hash) != funded || high.contains(&me.hash) != funded {
                return true;
            }
            // Its worst case if funded, its best if not.
            let at_end = fundable_at(candidates, threshold, budget, |r| {
                if (r.hash == me.hash) == funded {
                    r.low
                } else {
                    r.high
                }
            });
            at_end.contains(&me.hash) != funded
        })
        .map(|r| r.hash.clone())
        .collect()
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

    fn cv(key: u8, op: u8, outcome: VoteOutcome, time: i64) -> CurrentVote {
        CurrentVote {
            outpoint: OutPoint::new(dashcore::Txid::from_byte_array([op; 32]), 0),
            outcome,
            time,
            key_id: [key; 20],
            vote_hash: [op; 32],
        }
    }

    fn weights(regular: u32, evonodes: u32) -> KeyWeights {
        KeyWeights {
            count: regular + evonodes,
            regular,
            evonodes,
            unknown: 0,
        }
    }

    #[test]
    fn ranged_tally_is_exact_for_uniform_keys_and_ranges_the_rest() {
        let keys: HashMap<[u8; 20], KeyWeights> = HashMap::from([
            ([1; 20], weights(1, 0)),
            ([2; 20], weights(0, 2)),
            ([3; 20], weights(1, 1)),
            ([4; 20], weights(1, 0)),
        ]);
        let votes = [
            cv(1, 1, VoteOutcome::Yes, 10),
            cv(2, 2, VoteOutcome::Yes, 10),
            cv(2, 3, VoteOutcome::No, 10),
            cv(3, 4, VoteOutcome::Abstain, 10),
            // Key 4 holds one masternode and votes for two outpoints.
            cv(4, 5, VoteOutcome::Yes, 20),
            cv(4, 6, VoteOutcome::No, 10),
        ];
        let t = ranged_tally(&votes, |k| keys.get(k).copied(), |_| None);
        assert_eq!(
            t.yes,
            WeightRange {
                min: 1 + 4,
                max: 1 + 4 + 1,
                estimate: 1 + 4 + 1
            }
        );
        assert_eq!(
            t.no,
            WeightRange {
                min: 4,
                max: 4 + 1,
                estimate: 4
            }
        );
        assert_eq!(
            t.abstain,
            WeightRange {
                min: 1,
                max: 4,
                estimate: 1
            }
        );
        assert_eq!(t.voters, 6);
        assert_eq!(t.ranged_votes, 3);
        assert_eq!(t.estimate.absolute_yes(), 2);
        assert_eq!(
            (t.estimate.regular_voters, t.estimate.evonode_voters),
            (3, 2)
        );
        assert_eq!(t.net_yes_bounds(), (0, 2));

        // Resolved outpoints are exact and take their masternode out of the
        // key's pool.
        let votes = [
            cv(4, 5, VoteOutcome::Yes, 20),
            cv(4, 6, VoteOutcome::Yes, 10),
        ];
        let t = ranged_tally(
            &votes,
            |k| keys.get(k).copied(),
            |op| (op.txid.to_byte_array() == [6; 32]).then_some(1),
        );
        assert_eq!(
            t.yes,
            WeightRange {
                min: 1,
                max: 2,
                estimate: 1
            }
        );
    }

    #[test]
    fn a_contested_outpoint_ranges_and_estimates_the_fitting_claim() {
        let keys: HashMap<[u8; 20], KeyWeights> =
            HashMap::from([([1; 20], weights(0, 1)), ([2; 20], weights(1, 0))]);
        // Key 1 (one evonode) votes yes for outpoint 1; key 2 (one regular
        // masternode) votes no for its own outpoint 2 and, newer, claims
        // outpoint 1 with no as well.
        let mut forged = cv(2, 1, VoteOutcome::No, 30);
        forged.vote_hash = [9; 32];
        let votes = [
            cv(1, 1, VoteOutcome::Yes, 10),
            cv(2, 2, VoteOutcome::No, 10),
            forged,
        ];
        let t = ranged_tally(&votes, |k| keys.get(k).copied(), |_| None);
        // Outpoint 1 counts once; key 1 fits (1 claim, 1 masternode), so its
        // claim is the estimate; key 2 claims two outpoints with one
        // masternode, so each of its votes weighs 0..1.
        assert_eq!(t.voters, 2);
        assert_eq!(
            t.yes,
            WeightRange {
                min: 0,
                max: 4,
                estimate: 4
            }
        );
        assert_eq!(
            t.no,
            WeightRange {
                min: 0,
                max: 2,
                estimate: 1
            }
        );
        assert_eq!(t.estimate.absolute_yes(), 3);
        assert_eq!(t.ranged_votes, 3);
    }

    #[test]
    fn a_revoked_holder_weighs_one_to_four_estimated_regular() {
        // Key 1: one revoked masternode, whose list entry carries no type;
        // key 2: a regular masternode and a revoked one (D34).
        let mut shared = KeyWeights::one(Some(1));
        shared.add(None);
        let keys: HashMap<[u8; 20], KeyWeights> =
            HashMap::from([([1; 20], KeyWeights::one(None)), ([2; 20], shared)]);
        let votes = [
            cv(1, 1, VoteOutcome::Yes, 10),
            cv(2, 2, VoteOutcome::No, 10),
            cv(2, 3, VoteOutcome::No, 10),
        ];
        let t = ranged_tally(&votes, |k| keys.get(k).copied(), |_| None);
        assert_eq!(
            t.yes,
            WeightRange {
                min: 1,
                max: 4,
                estimate: 1
            }
        );
        assert_eq!(
            t.no,
            WeightRange {
                min: 2,
                max: 8,
                estimate: 2
            }
        );
        assert_eq!(t.ranged_votes, 3);
        // An evonode majority still estimates 4.
        let mut evo = KeyWeights::one(Some(EVONODE_VOTE_WEIGHT));
        evo.add(Some(EVONODE_VOTE_WEIGHT));
        evo.add(None);
        assert_eq!((evo.min(), evo.max(), evo.common()), (1, 4, 4));
    }

    #[test]
    fn test_qt_129_threshold_has_the_min_quorum_floor() {
        assert_eq!(threshold(6, 1), 1);
        assert_eq!(threshold(6, 10), 10);
        assert_eq!(threshold(4_000, 10), 400);
    }

    /// Sol r1 Major 6: the budget fits one of two equal proposals; A is
    /// estimated at 24 (15–24) and B at 23 (14–23). Every candidate low,
    /// or every one high, still funds A, but A at 15 against B at 23 funds
    /// B: both are close calls.
    #[test]
    fn opposite_ends_of_two_ranges_that_swap_the_budget_are_close_calls() {
        let c = |hash: &str, estimate, low, high| RangedCandidate {
            hash: hash.into(),
            payment_amount: 100,
            estimate,
            low,
            high,
        };
        let candidates = [c("aa", 24, 15, 24), c("bb", 23, 14, 23)];
        let close = budget_close_calls(&candidates, 10, 100);
        assert_eq!(close, HashSet::from(["aa".to_string(), "bb".to_string()]));
        // Ranges that cannot reorder them are not close calls.
        let apart = [c("aa", 24, 20, 24), c("bb", 13, 11, 19)];
        assert!(budget_close_calls(&apart, 10, 100).is_empty());
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
