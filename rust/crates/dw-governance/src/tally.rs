//! Weighted funding tallies, the passing threshold, the fundable set and
//! dash-qt's proposal statuses (research 02 §11.1, Core
//! `CGovernanceObject::CountMatchingVotes`, `GetApprovedProposals`, the
//! trigger builder's budget loop, dash-qt `Proposal::status`).
//!
//! Core counts the latest funding vote of every collateral outpoint that is
//! a masternode of the tip list, weighted 1 (regular) or 4 (evonode). The
//! SPV list carries no outpoints, so the store keeps votes by (outpoint,
//! voting key their signature recovers). The wallet cannot tell that an
//! outpoint it did not resolve is a masternode's holding that key, so
//! [`ranged_tally`] weighs each such vote between 0 and 4 and estimates it
//! by the key's masternodes (MG-02 D1, D2, D64; MG-00 D45). Outpoints the
//! wallet resolved (MG-03) count exactly.

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
    /// Of those, votes the store holds no countable copy of (MG-02 D71,
    /// D72): whatever the ranges, the row is a close call.
    pub uncounted: u32,
    /// The estimates as a [`Tally`] (statuses and the budget use it).
    pub estimate: Tally,
}

impl RangedTally {
    /// The range of `outcome`.
    fn range(&mut self, outcome: VoteOutcome) -> &mut WeightRange {
        match outcome {
            VoteOutcome::Yes => &mut self.yes,
            VoteOutcome::No => &mut self.no,
            VoteOutcome::Abstain => &mut self.abstain,
        }
    }

    /// Counts one claim; `new_voter` when it is its outpoint's first.
    fn count(&mut self, outcome: VoteOutcome, min: u32, max: u32, estimate: u32, new_voter: bool) {
        self.range(outcome).add(min, max, estimate);
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

    /// Adds `votes` the store holds no countable copy of, weighing up to
    /// `max` together, to `outcome` (MG-02 D71, D72).
    pub fn widen(&mut self, outcome: VoteOutcome, max: u32, votes: u32) {
        let r = self.range(outcome);
        r.max = r.max.saturating_add(max);
        self.ranged_votes += votes;
        self.uncounted += votes;
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
/// key. A vote for an outpoint the wallet did not resolve weighs between 0
/// and an evonode's weight, whatever its key (MG-02 D64): the wallet cannot
/// tell that the outpoint is a masternode's holding that key, and Core
/// counts nothing for one that is not, while a re-signed copy of another
/// masternode's vote (the hash leaves the signature out) stands for a vote
/// of up to 4. Its estimate, per key with `n` masternodes not otherwise
/// accounted for and `c` unresolved outpoints claimed with it: if `c ≤ n`
/// the key's most common weight; if `c > n` that for the `n` newest and 0
/// for the rest. An outpoint claimed by several keys (a key holder signing
/// for an outpoint not theirs) counts once as a voter, and its estimate goes
/// to the one claim whose key has `c ≤ n`, if exactly one has, else to
/// none.
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
    let unverified = |estimate, new_voter| Claim {
        weight: WeightRange {
            min: 0,
            max: EVONODE_VOTE_WEIGHT,
            estimate,
        },
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
                        weight: WeightRange {
                            min: w,
                            max: w,
                            estimate: w,
                        },
                        exact: true,
                        new_voter: true,
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
            out.push((*v, unverified(estimate, i == 0)));
        }
    }
    for (key, mut group) in by_key {
        let Some(k) = holders(&key) else { continue };
        let n = if fits(&key, &k) {
            usize::MAX
        } else {
            free(&key, &k) as usize
        };
        group.sort_by(|a, b| b.time.cmp(&a.time).then(a.vote_hash.cmp(&b.vote_hash)));
        for (i, v) in group.into_iter().enumerate() {
            let estimate = if i < n { k.common() } else { 0 };
            out.push((v, unverified(estimate, true)));
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

/// The budget the passing proposals fit into: Core's trigger builder over
/// `GetApprovedProposals` (absolute yes ≥ threshold, most yes first, ties by
/// the larger hash), each added while the running total stays within
/// `budget`. The caller picks the candidates (MG-02 D65).
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
/// could change (MG-00 D45): those not shown funded, or shown unfunded, at
/// every count the ranges allow. The greedy loop skips what does not fit,
/// so no handful of scenarios bounds a candidate's place (MG-02 D48, D67);
/// this over-approximates instead, and may mark more than could change.
/// With candidate X at its low and the others at their highs, those that
/// may rank above X and are not surely unfunded take at most their cost:
/// if X still fits, and its low passes, it is surely funded. With X at its
/// high, the surely funded ones that surely rank above it take at least
/// theirs: if X no longer fits, it is surely unfunded. Each answer feeds
/// the others until none changes. Candidates that cannot reach `threshold`
/// take part in no scenario and are left out.
pub fn budget_close_calls(
    candidates: &[RangedCandidate],
    threshold: u32,
    budget: u64,
) -> HashSet<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Place {
        Open,
        Funded,
        Unfunded,
    }
    let thr = i64::from(threshold);
    let budget = u128::from(budget);
    let reachable: Vec<&RangedCandidate> = candidates.iter().filter(|r| r.high >= thr).collect();
    let mut place: Vec<Place> = reachable
        .iter()
        .map(|r| {
            if u128::from(r.payment_amount) > budget {
                Place::Unfunded
            } else {
                Place::Open
            }
        })
        .collect();
    loop {
        let costs = |of: fn(Place) -> bool, count: fn(&RangedCandidate) -> i64| {
            CostsAbove::new(
                reachable
                    .iter()
                    .zip(&place)
                    .filter(|(_, p)| of(**p))
                    .map(|(r, _)| ((count(r), r.hash.as_str()), r.payment_amount)),
            )
        };
        let may = costs(|p| p != Place::Unfunded, |r| r.high);
        let sure = costs(|p| p == Place::Funded, |r| r.low);
        let mut changed = false;
        for (r, p) in reachable.iter().zip(&mut place) {
            if *p != Place::Open {
                continue;
            }
            let cost = u128::from(r.payment_amount);
            let (low, high) = ((r.low, r.hash.as_str()), (r.high, r.hash.as_str()));
            // `may` counts the candidate itself unless its range is a point.
            let itself = if high > low { cost } else { 0 };
            if r.low >= thr && may.above(low) - itself + cost <= budget {
                *p = Place::Funded;
            } else if sure.above(high) + cost > budget {
                *p = Place::Unfunded;
            } else {
                continue;
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
    reachable
        .iter()
        .zip(&place)
        .filter(|(_, p)| **p == Place::Open)
        .map(|(r, _)| r.hash.clone())
        .collect()
}

/// Candidates' costs by their rank (a yes count, ties by the larger hash,
/// as [`fundable`] orders them), summed over those ranking above a rank.
struct CostsAbove<'a> {
    ranks: Vec<(i64, &'a str)>,
    /// `sums[i]`: the cost of `ranks[i..]`.
    sums: Vec<u128>,
}

impl<'a> CostsAbove<'a> {
    fn new(items: impl Iterator<Item = ((i64, &'a str), u64)>) -> Self {
        let mut items: Vec<_> = items.collect();
        items.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut sums = vec![0u128; items.len() + 1];
        for i in (0..items.len()).rev() {
            sums[i] = sums[i + 1] + u128::from(items[i].1);
        }
        Self {
            ranks: items.into_iter().map(|(rank, _)| rank).collect(),
            sums,
        }
    }

    fn above(&self, rank: (i64, &str)) -> u128 {
        self.sums[self.ranks.partition_point(|r| *r <= rank)]
    }
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
    fn ranged_tally_ranges_unresolved_votes_and_estimates_by_key() {
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
        // Unresolved, each vote weighs 0..4 (D64); the estimates follow the
        // keys.
        assert_eq!(
            t.yes,
            WeightRange {
                min: 0,
                max: 3 * 4,
                estimate: 1 + 4 + 1
            }
        );
        assert_eq!(
            t.no,
            WeightRange {
                min: 0,
                max: 2 * 4,
                estimate: 4
            }
        );
        assert_eq!(
            t.abstain,
            WeightRange {
                min: 0,
                max: 4,
                estimate: 1
            }
        );
        assert_eq!(t.voters, 6);
        assert_eq!(t.ranged_votes, 6);
        assert_eq!(t.estimate.absolute_yes(), 2);
        assert_eq!(
            (t.estimate.regular_voters, t.estimate.evonode_voters),
            (3, 2)
        );
        assert_eq!(t.net_yes_bounds(), (-8, 12));

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
                max: 1 + 4,
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
        // masternode, so only its newest outside the contest is estimated.
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
                max: 8,
                estimate: 1
            }
        );
        assert_eq!(t.estimate.absolute_yes(), 3);
        assert_eq!(t.ranged_votes, 3);
    }

    #[test]
    fn a_revoked_holder_is_estimated_regular() {
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
                min: 0,
                max: 4,
                estimate: 1
            }
        );
        assert_eq!(
            t.no,
            WeightRange {
                min: 0,
                max: 8,
                estimate: 2
            }
        );
        assert_eq!(t.ranged_votes, 3);
        // An evonode majority still estimates 4.
        let mut evo = KeyWeights::one(Some(EVONODE_VOTE_WEIGHT));
        evo.add(Some(EVONODE_VOTE_WEIGHT));
        evo.add(None);
        assert_eq!(evo.common(), 4);
    }

    /// Sol r2 Major 5: the holder of one listed key signs a YES for an
    /// outpoint that is not its masternode's, which Core does not count; or
    /// a regular masternode's key re-signs an evonode's vote, whose copy
    /// (same hash) the wallet then never fetches, which Core counts at 4.
    /// Unresolved, a vote proves neither: it weighs 0..4 and is ranged.
    #[test]
    fn an_unresolved_vote_proves_no_weight_whatever_its_key() {
        let keys: HashMap<[u8; 20], KeyWeights> =
            HashMap::from([([1; 20], weights(0, 1)), ([2; 20], weights(1, 0))]);
        let forged = [cv(1, 7, VoteOutcome::Yes, 10)];
        let t = ranged_tally(&forged, |k| keys.get(k).copied(), |_| None);
        assert_eq!(
            t.yes,
            WeightRange {
                min: 0,
                max: 4,
                estimate: 4
            }
        );
        assert_eq!(t.ranged_votes, 1);
        let resigned = [cv(2, 8, VoteOutcome::Yes, 10)];
        let t = ranged_tally(&resigned, |k| keys.get(k).copied(), |_| None);
        assert_eq!(
            t.yes,
            WeightRange {
                min: 0,
                max: 4,
                estimate: 1
            }
        );
        assert_eq!(t.ranged_votes, 1);
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
        // Candidates that cannot pass change nothing (and are skipped).
        let mut flooded = candidates.to_vec();
        flooded.extend((0..5_000).map(|i| c(&format!("f{i:04}"), 0, 0, 9)));
        assert_eq!(budget_close_calls(&flooded, 10, 100), close);
        // A range ending at the threshold can still pass.
        let edge = [c("cc", 10, 9, 10)];
        assert_eq!(
            budget_close_calls(&edge, 10, 100),
            HashSet::from(["cc".to_string()])
        );
    }

    /// Sol r2 Major 2: threshold 10, budget 14. The estimates, every
    /// candidate low or high, and each one's own extreme against the
    /// others' fund A and C; but A=14, B=21, C=20 funds B, then skips C and
    /// A. Each of the three can land on either side.
    #[test]
    fn a_greedy_skip_among_three_ranges_is_a_close_call() {
        let c = |hash: &str, payment_amount, estimate, low, high| RangedCandidate {
            hash: hash.into(),
            payment_amount,
            estimate,
            low,
            high,
        };
        let candidates = [
            c("aa", 8, 23, 14, 23),
            c("bb", 9, 18, 18, 21),
            c("cc", 6, 20, 20, 23),
        ];
        assert_eq!(
            fundable_at(&candidates, 10, 14, |r| r.estimate),
            HashSet::from(["aa".to_string(), "cc".to_string()])
        );
        assert_eq!(
            budget_close_calls(&candidates, 10, 14),
            HashSet::from(["aa".to_string(), "bb".to_string(), "cc".to_string()])
        );
    }

    /// Every candidate whose place differs between two counts in the
    /// ranges is a close call; with no ranges there are none, skips
    /// included.
    #[test]
    fn close_calls_cover_every_count_in_the_ranges() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for round in 0..3_000 {
            let exact = round % 3 == 0;
            let threshold = 5 + next(6) as u32;
            let budget = 5 + next(16);
            let candidates: Vec<RangedCandidate> = (0..2 + next(3))
                .map(|i| {
                    let low = next(13) as i64;
                    let high = if exact { low } else { low + next(5) as i64 };
                    RangedCandidate {
                        hash: format!("{:02x}", next(256) * 4 + i),
                        payment_amount: 1 + next(10),
                        estimate: low,
                        low,
                        high,
                    }
                })
                .collect();
            let close = budget_close_calls(&candidates, threshold, budget);
            // Every combination of counts.
            let mut funded: HashMap<&str, HashSet<bool>> = HashMap::new();
            let mut counts: Vec<i64> = candidates.iter().map(|r| r.low).collect();
            loop {
                let set = fundable_at(&candidates, threshold, budget, |r| {
                    counts[candidates.iter().position(|x| x.hash == r.hash).unwrap()]
                });
                for r in &candidates {
                    funded
                        .entry(&r.hash)
                        .or_default()
                        .insert(set.contains(&r.hash));
                }
                let Some(i) = (0..counts.len()).find(|&i| counts[i] < candidates[i].high) else {
                    break;
                };
                counts[i] += 1;
                for (j, r) in candidates.iter().enumerate().take(i) {
                    counts[j] = r.low;
                }
            }
            for r in &candidates {
                let both = funded[r.hash.as_str()].len() == 2;
                assert!(!both || close.contains(&r.hash), "{round}: {candidates:?}");
            }
            if exact {
                assert!(close.is_empty(), "{round}: {candidates:?} {close:?}");
            }
        }
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
