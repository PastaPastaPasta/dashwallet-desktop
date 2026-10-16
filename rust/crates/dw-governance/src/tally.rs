//! Weighted funding tallies, the passing threshold, the fundable set and
//! dash-qt's proposal statuses (research 02 §11.1, Core
//! `CGovernanceObject::CountMatchingVotes`, `GetApprovedProposals`, node
//! `getFundableProposalHashes`, dash-qt `Proposal::status`).
//!
//! Which votes count is MG-02's: Core counts the latest funding vote of
//! every collateral outpoint that is a masternode of the tip list, weighted
//! 1 (regular) or 4 (EvoNode), and the SPV list carries no outpoints, so the
//! facade maps votes by the outpoints it derives (MG-00 D43) and reports a
//! range for the weight it cannot verify (D45). The parked branch's
//! voting-key mapping was not salvaged (docs/design/MG-01-salvage.md D5).

use dashcore::OutPoint;

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

    #[test]
    fn test_qt_129_threshold_has_the_min_quorum_floor() {
        assert_eq!(threshold(6, 1), 1);
        assert_eq!(threshold(6, 10), 10);
        assert_eq!(threshold(4_000, 10), 400);
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
