//! The read calls (mg-engine §2.2, §2.3): the clock and the budget from the
//! tip, proposal rows and votes from the store. Memory only.

use std::collections::HashSet;

use dw_governance::clock;
use dw_governance::object::{display_hex, outpoint_short, parse_display_hex};
use dw_governance::params::{GovernanceParams, PROPOSAL_FEE_CONFIRMATIONS, governance_params};
use dw_governance::store::{GovernanceStore, ObjectKind};
use dw_governance::tally::{self, BudgetCandidate, StatusInput};

use super::records::{
    CurrentVoteRow, GovernanceClock, GovernanceInfo, ProposalCounts, ProposalDetail, ProposalQuery,
    ProposalRow, ProposalScope, ProposalStatus, SuperblockDate, VoteTally,
};
use super::sync::{GovernanceRuntime, ListSnapshot};
use super::{Governance, GovernanceError, superblock_budget};
use crate::events::unix_now;

/// Now in the store's seconds.
pub(super) fn now() -> i64 {
    i64::try_from(unix_now()).unwrap_or(i64::MAX)
}

/// Most superblocks [`Governance::upcoming_superblocks`] lists.
pub const MAX_UPCOMING_SUPERBLOCKS: u32 = 24;

/// The tip a read is answered for.
#[derive(Debug, Clone, Copy)]
struct Tip {
    height: u32,
    /// The ETA base: the tip's time, or now when that is older.
    eta_base: u64,
}

/// Every proposal row with what the counts need.
struct Table {
    rows: Vec<ProposalRow>,
    threshold: u32,
    budget: u64,
    allocated: u64,
}

impl Governance {
    pub(super) fn runtime(&self) -> Result<&GovernanceRuntime, GovernanceError> {
        self.session.live()?;
        Ok(&self.session.governance)
    }

    fn params(&self) -> GovernanceParams {
        governance_params(self.session.network.core_network())
    }

    fn tip(&self) -> Result<Tip, GovernanceError> {
        let rt = self.runtime()?;
        let (height, time) = {
            let t = self.session.hub.tracker();
            (t.tip_height(), t.snapshot().tip_time)
        };
        let Some(height) = height else {
            return Err(GovernanceError::NotSynced {
                phase: rt.shared.progress().phase.into(),
            });
        };
        let now = unix_now();
        Ok(Tip {
            height,
            eta_base: time.map_or(now, |t| t.max(now)),
        })
    }

    /// `governance.not_synced{phase}` until a run reached `synced` (MG-00
    /// D21).
    fn require_synced(&self) -> Result<&GovernanceRuntime, GovernanceError> {
        let rt = self.runtime()?;
        let p = rt.shared.progress();
        if p.last_synced_at.is_none() {
            return Err(GovernanceError::NotSynced {
                phase: p.phase.into(),
            });
        }
        drop(p);
        Ok(rt)
    }

    /// The governance clock for the tip (QT-026).
    pub fn clock(&self) -> Result<GovernanceClock, GovernanceError> {
        let tip = self.tip()?;
        Ok(clock_at(&self.params(), tip))
    }

    /// Core's `getgovernanceinfo` numbers, the budget, the threshold and,
    /// once synced, the proposal counts.
    pub fn info(&self) -> Result<GovernanceInfo, GovernanceError> {
        let tip = self.tip()?;
        let rt = self.runtime()?;
        let p = self.params();
        let clock = clock_at(&p, tip);
        let list = rt.list();
        let (valid_masternodes, valid_evonodes, valid_weight) = valid_counts(&list);
        let synced = rt.shared.progress().last_synced_at.is_some();
        let now = now();
        let table = {
            let store = rt.shared.store();
            build_table(&self.session.network, &store, tip.height, now)
        };
        let (proposals, allocated, unallocated) = if synced {
            let mut c = ProposalCounts {
                passing: 0,
                failing: 0,
                unfunded: 0,
                total: 0,
                close_call: 0,
            };
            for r in table.rows.iter().filter(|r| r.end_epoch > now) {
                c.total += 1;
                match r.status {
                    ProposalStatus::Passing | ProposalStatus::Funded => c.passing += 1,
                    ProposalStatus::Failing | ProposalStatus::Voting => c.failing += 1,
                    ProposalStatus::Unfunded => c.unfunded += 1,
                    _ => {}
                }
                c.close_call += u32::from(r.close_call);
            }
            (
                Some(c),
                Some(table.allocated),
                Some(table.budget.saturating_sub(table.allocated)),
            )
        } else {
            (None, None, None)
        };
        Ok(GovernanceInfo {
            clock,
            superblock_cycle: p.superblock_cycle,
            maturity_window: p.maturity_window,
            target_spacing: p.target_spacing_secs,
            proposal_fee: p.proposal_fee,
            budget: table.budget,
            passing_threshold: table.threshold,
            valid_masternodes,
            valid_evonodes,
            valid_weight,
            proposals,
            allocated_estimate: allocated,
            unallocated_estimate: unallocated,
        })
    }

    /// The next `count` superblocks (at most 24) with their ETAs and budgets.
    pub fn upcoming_superblocks(&self, count: u32) -> Result<Vec<SuperblockDate>, GovernanceError> {
        if count > MAX_UPCOMING_SUPERBLOCKS {
            return Err(crate::EngineError::InvalidArgument(format!(
                "count must be at most {MAX_UPCOMING_SUPERBLOCKS}"
            ))
            .into());
        }
        let tip = self.tip()?;
        let network = &self.session.network;
        Ok(
            clock::upcoming_superblocks(&self.params(), tip.height, count)
                .into_iter()
                .map(|height| SuperblockDate {
                    height,
                    estimated_time: clock::estimated_time(tip.height, tip.eta_base, height),
                    budget: superblock_budget(network, height),
                })
                .collect(),
        )
    }

    /// The confirmed proposals (MG-02 D3) in dash-qt's order.
    pub fn proposals(&self, query: ProposalQuery) -> Result<Vec<ProposalRow>, GovernanceError> {
        let rt = self.require_synced()?;
        let tip = self.tip()?;
        let now = now();
        let needle = query.text.map(|t| t.to_lowercase());
        let table = build_table(&self.session.network, &rt.shared.store(), tip.height, now);
        Ok(table
            .rows
            .into_iter()
            .filter(|r| query.scope == ProposalScope::All || r.end_epoch > now)
            .filter(|r| {
                needle.as_ref().is_none_or(|n| {
                    r.name.to_lowercase().contains(n) || r.url.to_lowercase().contains(n)
                })
            })
            .collect())
    }

    /// One confirmed proposal with its object's fields.
    pub fn proposal(&self, hash: String) -> Result<ProposalDetail, GovernanceError> {
        let rt = self.require_synced()?;
        let tip = self.tip()?;
        let key = parse_hash(&hash)?;
        let store = rt.shared.store();
        let Some(o) = store
            .object(&key)
            .filter(|o| matches!(o.kind, ObjectKind::Proposal(_)))
        else {
            return Err(GovernanceError::ProposalNotFound { hash });
        };
        let table = build_table(&self.session.network, &store, tip.height, now());
        let row = table
            .rows
            .into_iter()
            .find(|r| r.hash == o.hash_hex)
            .ok_or(GovernanceError::ProposalNotFound { hash })?;
        Ok(ProposalDetail {
            row,
            collateral_txid: display_hex(&o.object.collateral_hash),
            created_at: o.object.time,
            data_json: o.object.data_text(),
        })
    }

    /// Core's `gobject getcurrentvotes`: the current funding votes on a
    /// proposal, optionally of one outpoint (`"<txid>-<n>"`), each with its
    /// weight range.
    pub fn current_votes(
        &self,
        hash: String,
        outpoint: Option<String>,
    ) -> Result<Vec<CurrentVoteRow>, GovernanceError> {
        let rt = self.require_synced()?;
        let key = parse_hash(&hash)?;
        let store = rt.shared.store();
        if !store
            .object(&key)
            .is_some_and(|o| matches!(o.kind, ObjectKind::Proposal(_)))
        {
            return Err(GovernanceError::ProposalNotFound { hash });
        }
        let mut rows: Vec<CurrentVoteRow> = tally::attribute(
            store.votes_on(&key),
            |k| store.key_weights(k),
            |o| store.resolved(o).map(|r| r.weight),
        )
        .into_iter()
        .map(|(v, c)| CurrentVoteRow {
            outpoint: outpoint_short(&v.outpoint),
            time: v.time,
            outcome: v.outcome.into(),
            weight: c.weight.into(),
            exact: c.exact,
        })
        .filter(|r| outpoint.as_ref().is_none_or(|o| &r.outpoint == o))
        .collect();
        rows.sort_by(|a, b| a.outpoint.cmp(&b.outpoint).then(a.time.cmp(&b.time)));
        Ok(rows)
    }
}

pub(super) fn parse_hash(hash: &str) -> Result<[u8; 32], GovernanceError> {
    parse_display_hex(hash).ok_or_else(|| {
        crate::EngineError::InvalidArgument(format!("not a 64-hex object hash: {hash:?}")).into()
    })
}

fn clock_at(p: &GovernanceParams, tip: Tip) -> GovernanceClock {
    let c = clock::clock(p, tip.height, tip.eta_base);
    GovernanceClock {
        tip_height: tip.height,
        cycle_progress: c.cycle_progress,
        last_superblock: c.last_superblock,
        next_superblock: c.next_superblock,
        blocks_to_superblock: c.blocks_to_superblock,
        superblock_eta: c.superblock_eta,
        voting_cutoff_estimate: clock::estimated_time(tip.height, tip.eta_base, c.voting_cutoff),
        voting_cutoff_height: c.voting_cutoff,
        voting_open: c.voting_open,
    }
}

/// Valid masternodes, valid evonodes and their weight (Core
/// `m_valid_weighted`).
fn valid_counts(list: &ListSnapshot) -> (u32, u32, u32) {
    let valid = list.entries.iter().filter(|e| e.is_valid);
    let (mut regular, mut evo, mut weight) = (0u32, 0u32, 0u32);
    for e in valid {
        if e.is_evonode {
            evo += 1;
        } else {
            regular += 1;
        }
        weight += ListSnapshot::weight(e);
    }
    (regular + evo, evo, weight)
}

/// Every confirmed proposal's row: the ranged tally, the fundable set on
/// the estimates (node `getFundableProposalHashes`), dash-qt's status, and
/// whether the ranges could change it (MG-00 D45).
fn build_table(network: &crate::DashNetwork, store: &GovernanceStore, tip: u32, now: i64) -> Table {
    let p = governance_params(network.core_network());
    let threshold = tally::threshold(store.valid_weight(), p.min_quorum);
    let (_, next) = clock::nearest_superblocks(&p, tip);
    let budget = superblock_budget(network, next);
    let maturity = clock::in_maturity_window(&p, tip);

    let tallied: Vec<_> = store
        .proposals()
        .map(|(o, d)| {
            let t = tally::ranged_tally(
                store.votes_on(&o.object.hash()),
                |k| store.key_weights(k),
                |op| store.resolved(op).map(|r| r.weight),
            );
            (o, d, t)
        })
        .collect();
    let fundable_with = |net: &dyn Fn(&tally::RangedTally) -> i64| -> HashSet<String> {
        let candidates: Vec<BudgetCandidate> = tallied
            .iter()
            .filter(|(_, d, _)| d.end_epoch.is_none_or(|e| e > now))
            // Core's trigger builder skips an amount `ParsePaymentAmount`
            // refuses.
            .filter_map(|(o, d, t)| {
                Some(BudgetCandidate {
                    hash: o.hash_hex.clone(),
                    payment_amount: d.payable_amount?,
                    absolute_yes: net(t),
                })
            })
            .collect();
        tally::fundable(&candidates, threshold, budget)
            .hashes
            .into_iter()
            .collect()
    };
    let estimate = fundable_with(&|t| t.estimate.absolute_yes());
    let low = fundable_with(&|t| t.net_yes_bounds().0);
    let high = fundable_with(&|t| t.net_yes_bounds().1);
    let allocated = tallied
        .iter()
        .filter(|(o, _, _)| estimate.contains(&o.hash_hex))
        .filter_map(|(_, d, _)| d.payable_amount)
        .sum();

    let mut keyed: Vec<(i64, ProposalRow)> = tallied
        .into_iter()
        .map(|(o, d, t)| {
            let lapsed = d.end_epoch.is_some_and(|e| now >= e);
            let funded_height = store.funded_height(&o.hash_hex, tip);
            let absolute_yes = t.estimate.absolute_yes();
            let fundable = estimate.contains(&o.hash_hex);
            let status = tally::status(&StatusInput {
                funded_height,
                lapsed,
                // Core relays a proposal only once its collateral matured.
                collateral_confirmations: PROPOSAL_FEE_CONFIRMATIONS,
                required_confirmations: PROPOSAL_FEE_CONFIRMATIONS,
                broadcast: true,
                in_maturity_window: maturity,
                absolute_yes,
                threshold,
                fundable,
            });
            let (min_net, max_net) = t.net_yes_bounds();
            let thr = i64::from(threshold);
            let close_call = !lapsed
                && funded_height.is_none()
                && ((min_net >= thr) != (max_net >= thr)
                    || low.contains(&o.hash_hex) != fundable
                    || high.contains(&o.hash_hex) != fundable);
            let row = ProposalRow {
                hash: o.hash_hex.clone(),
                name: d.name.clone().unwrap_or_default(),
                url: d.url.clone().unwrap_or_default(),
                payment_address: d.payment_address.clone().unwrap_or_default(),
                payment_amount: d.payment_amount.unwrap_or(0),
                start_epoch: d.start_epoch.unwrap_or(0),
                end_epoch: d.end_epoch.unwrap_or(0),
                payments_requested: d.payments_requested(&p),
                status: status.into(),
                close_call,
                tally: VoteTally {
                    yes: t.yes.into(),
                    no: t.no.into(),
                    abstain: t.abstain.into(),
                    net_yes_estimate: absolute_yes,
                    voters: t.voters,
                    ranged_votes: t.ranged_votes,
                },
                funded_height,
            };
            (tally::sort_key(status, absolute_yes, threshold), row)
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.hash.cmp(&b.1.hash)));
    Table {
        rows: keyed.into_iter().map(|(_, r)| r).collect(),
        threshold,
        budget,
        allocated,
    }
}
