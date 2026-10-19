//! The read calls (mg-engine §2.2, §2.3): the clock and the budget from the
//! tip, proposal rows and votes from the store. Memory only.

use dw_governance::clock;
use dw_governance::object::{display_hex, outpoint_short, parse_display_hex};
use dw_governance::params::{GovernanceParams, PROPOSAL_FEE_CONFIRMATIONS, governance_params};
use dw_governance::store::{GovernanceStore, ObjectKind};
use dw_governance::tally::{self, RangedCandidate, StatusInput};
use dw_governance::vote::SentinelSignal;

use super::records::{
    CurrentVoteRow, GovernanceClock, GovernanceInfo, ProposalCounts, ProposalDetail, ProposalQuery,
    ProposalRow, ProposalScope, ProposalStatus, SignalTallies, SuperblockDate, VoteSignal,
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
    pub(super) fn require_synced(&self) -> Result<&GovernanceRuntime, GovernanceError> {
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
        let signal = |s| store.signal_tally(&key, s).into();
        Ok(ProposalDetail {
            row,
            collateral_txid: display_hex(&o.object.collateral_hash),
            created_at: o.object.time,
            data_json: o.object.data_text(),
            signals: SignalTallies {
                valid: signal(SentinelSignal::Valid),
                delete: signal(SentinelSignal::Delete),
                endorsed: signal(SentinelSignal::Endorsed),
            },
            deletion_marked_at: store.deletion_mark(&key),
        })
    }

    /// Core's `gobject getcurrentvotes`: the current votes of every signal
    /// on a proposal (MG-02 D86), optionally of one outpoint
    /// (`"<txid>-<n>"`), each with its weight range.
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
        Ok(vote_rows(&store, &key)
            .into_iter()
            .filter(|r| outpoint.as_ref().is_none_or(|o| &r.outpoint == o))
            .collect())
    }
}

/// The current votes on `key` with the weights its tallies give them
/// (MG-02 D72, D82), in Core's order: by outpoint as `COutPoint` compares
/// (the txid's internal bytes, then the index), then by signal (D86).
fn vote_rows(store: &GovernanceStore, key: &[u8; 32]) -> Vec<CurrentVoteRow> {
    let mut votes: Vec<_> = tally::attribute(
        store.votes_on(key),
        |k| store.key_weights(k),
        |o| store.exact_weight(key, o),
    )
    .into_iter()
    .map(|vc| (VoteSignal::Funding, vc))
    .collect();
    for signal in SentinelSignal::ALL {
        let of_signal = store
            .signal_votes_on(key)
            .filter(|(s, _)| *s == signal)
            .map(|(_, v)| v);
        votes.extend(
            tally::attribute(
                of_signal,
                |k| store.key_weights(k),
                |o| store.resolved(o).map(|r| r.weight),
            )
            .into_iter()
            .map(|vc| (signal.into(), vc)),
        );
    }
    votes.sort_by(|(sa, (a, _)), (sb, (b, _))| {
        (a.outpoint, sa, a.time).cmp(&(b.outpoint, sb, b.time))
    });
    votes
        .into_iter()
        .map(|(signal, (v, c))| CurrentVoteRow {
            outpoint: outpoint_short(&v.outpoint),
            signal,
            time: v.time,
            outcome: v.outcome.into(),
            weight: c.weight.into(),
            exact: c.exact,
        })
        .collect()
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
/// the estimates, dash-qt's status, and whether the ranges could change it
/// (MG-00 D45).
fn build_table(network: &crate::DashNetwork, store: &GovernanceStore, tip: u32, now: i64) -> Table {
    let p = governance_params(network.core_network());
    let threshold = tally::threshold(store.valid_weight(), p.min_quorum);
    let (_, next) = clock::nearest_superblocks(&p, tip);
    let budget = superblock_budget(network, next);
    let maturity = clock::in_maturity_window(&p, tip);

    let tallied: Vec<_> = store
        .proposals()
        .map(|(o, d)| {
            let t = store.tally(&o.object.hash());
            (o, d, t)
        })
        .collect();
    // The fundable set is dash-qt's (`getFundableProposalHashes`) over the
    // current proposals: the wallet keeps no lapsed one, which Core's
    // trigger builder would not pay (MG-02 D65, D69). An amount
    // `ParsePaymentAmount` refuses is skipped, as the builder does.
    let candidates: Vec<RangedCandidate> = tallied
        .iter()
        .filter(|(_, d, _)| d.end_epoch.is_none_or(|e| e > now))
        .filter_map(|(o, d, t)| {
            let (low, high) = t.net_yes_bounds();
            Some(RangedCandidate {
                hash: o.hash_hex.clone(),
                payment_amount: d.payable_amount?,
                estimate: t.estimate.absolute_yes(),
                low,
                high,
            })
        })
        .collect();
    let estimate = tally::fundable_at(&candidates, threshold, budget, |r| r.estimate);
    let budget_close = tally::budget_close_calls(&candidates, threshold, budget);
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
                    || budget_close.contains(&o.hash_hex)
                    || t.uncounted > 0);
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
                tally: t.into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::hashes::{Hash, hash160};
    use dashcore::secp256k1::{Message, PublicKey, SecretKey};
    use dashcore::{Network, OutPoint, Txid};
    use dw_governance::object::GovernanceObject;
    use dw_governance::store::{ListEntry, OperatorVote, ResolvedMasternode, VoteRejection};
    use dw_governance::vote::{GovernanceVote, VoteOutcome, to_recoverable_compact};
    use std::collections::HashMap;

    const NOW: i64 = 1_000_000;

    fn key_id(n: u8) -> [u8; 20] {
        let sk = SecretKey::from_secret_bytes([n; 32]).unwrap();
        hash160::Hash::hash(&PublicKey::from_secret_key(&sk).serialize()).to_byte_array()
    }

    fn outpoint(op: u8) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([op; 32]), 0)
    }

    /// A YES funding vote on `parent` by voting key `key` for outpoint `op`.
    fn yes(parent: [u8; 32], key: u8, op: u8) -> GovernanceVote {
        funding(parent, key, op, VoteOutcome::Yes)
    }

    /// A funding vote on `parent` by voting key `key` for outpoint `op`.
    fn funding(parent: [u8; 32], key: u8, op: u8, outcome: VoteOutcome) -> GovernanceVote {
        let v = GovernanceVote::funding(outpoint(op), parent, outcome, NOW - 100);
        signed(&v, key)
    }

    /// `v` signed by voting key `key`: the same hash, whoever signs.
    fn signed(v: &GovernanceVote, key: u8) -> GovernanceVote {
        let sk = SecretKey::from_secret_bytes([key; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&sk);
        let mut v = v.clone();
        let d = v.signing_digest(Network::Regtest);
        let s = dashcore::secp256k1::ecdsa::sign(Message::from_digest(d), &sk);
        v.signature = to_recoverable_compact(&s.serialize_compact(), &pk, &d)
            .unwrap()
            .to_vec();
        v
    }

    /// MG-02 D86: Core's `GetCurrentVotes` walks `mapCurrentMNVotes` by
    /// outpoint, then each outpoint's instances by signal.
    #[test]
    fn current_votes_list_every_signal_in_core_order() {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let h = p.hash();
        store.add_object(p, None, NOW).unwrap();
        let list: Vec<ListEntry> = [1, 2, 3]
            .map(|k| ListEntry {
                voting_key_id: key_id(k),
                weight: Some(1),
                valid: true,
            })
            .into();
        store.set_masternode_list(&list);
        store.set_resolved(HashMap::from([1, 2].map(|op| {
            (
                outpoint(op),
                ResolvedMasternode {
                    pro_tx_hash: [op; 32],
                    voting_key_id: key_id(op),
                    operator_key: [op; 48],
                    weight: 1,
                },
            )
        })));
        // Operator votes (D89), as the sync hands them over once checked.
        let operator = |store: &mut GovernanceStore, signal: SentinelSignal, op: u8, outcome| {
            let mut v = GovernanceVote::funding(outpoint(op), h, outcome, NOW - 100);
            v.signal = signal.code();
            v.signature = vec![op; 96];
            let v = OperatorVote {
                vote: v,
                outcome,
                checked: Some(([op; 48], true)),
                held: false,
            };
            store.add_operator_vote(v, None, NOW).unwrap();
        };
        // Added out of order; outpoint 3 is unresolved.
        operator(&mut store, SentinelSignal::Endorsed, 2, VoteOutcome::Yes);
        store.add_vote(yes(h, 3, 3), None, NOW).unwrap();
        operator(&mut store, SentinelSignal::Delete, 1, VoteOutcome::No);
        store
            .add_vote(funding(h, 2, 2, VoteOutcome::Abstain), None, NOW)
            .unwrap();
        operator(&mut store, SentinelSignal::Valid, 1, VoteOutcome::Yes);
        store.add_vote(yes(h, 1, 1), None, NOW).unwrap();
        let rows: Vec<_> = vote_rows(&store, &h)
            .into_iter()
            .map(|r| (r.outpoint, r.signal, r.exact))
            .collect();
        let [one, two, three] = [1, 2, 3].map(|op| outpoint_short(&outpoint(op)));
        assert_eq!(
            rows,
            [
                (one.clone(), VoteSignal::Funding, true),
                (one.clone(), VoteSignal::Valid, true),
                (one, VoteSignal::Delete, true),
                (two.clone(), VoteSignal::Funding, true),
                (two, VoteSignal::Endorsed, true),
                (three, VoteSignal::Funding, false),
            ]
        );
    }

    /// Sol r2 Major 5: 99 votes from resolved outpoints, against a threshold
    /// of 100 (list weight 1,003). The holder of a listed evonode key signs
    /// a YES for an outpoint that is not its masternode's: Core counts
    /// nothing for it, and the row must not read as passing. It is a close
    /// call (D64), which the UI shows as "Too close to call" (MG-00 D45).
    #[test]
    fn a_listed_key_signing_for_another_outpoint_makes_no_pass() {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let h = p.hash();
        store.add_object(p, None, NOW).unwrap();
        let mut list: Vec<ListEntry> = (1..=99)
            .map(|k| ListEntry {
                voting_key_id: key_id(k),
                weight: Some(1),
                valid: true,
            })
            .collect();
        list.push(ListEntry {
            voting_key_id: key_id(150),
            weight: Some(4),
            valid: true,
        });
        list.extend((0..900).map(|_| ListEntry {
            voting_key_id: key_id(200),
            weight: Some(1),
            valid: true,
        }));
        store.set_masternode_list(&list);
        store.set_resolved(
            (1..=99)
                .map(|k| {
                    let r = ResolvedMasternode {
                        pro_tx_hash: [k; 32],
                        voting_key_id: key_id(k),
                        operator_key: [0; 48],
                        weight: 1,
                    };
                    (outpoint(k), r)
                })
                .collect::<HashMap<_, _>>(),
        );
        for k in 1..=99 {
            store.add_vote(yes(h, k, k), None, NOW).unwrap();
        }
        let row = |store: &GovernanceStore| {
            let t = build_table(&crate::DashNetwork::Regtest, store, 1_505, NOW);
            assert_eq!(t.threshold, 100);
            t.rows.into_iter().next().unwrap()
        };
        let before = row(&store);
        assert_eq!(before.tally.net_yes_estimate, 99);
        assert_eq!(before.status, ProposalStatus::Voting);
        assert!(!before.close_call);

        store.add_vote(yes(h, 150, 250), None, NOW).unwrap();
        let after = row(&store);
        assert_eq!(after.tally.net_yes_estimate, 103);
        assert_eq!((after.tally.yes.min, after.tally.yes.max), (99, 103));
        assert_eq!(after.tally.ranged_votes, 1);
        assert!(after.close_call);
    }

    /// A proposal with `n` YES votes from resolved outpoints of weight 1,
    /// against a threshold of 100: the list also holds evonode keys 150
    /// and 151, one masternode each, and others up to a weight of 1,005.
    fn resolved_yes_store(
        n: u8,
    ) -> (
        GovernanceStore,
        [u8; 32],
        HashMap<OutPoint, ResolvedMasternode>,
    ) {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let h = p.hash();
        store.add_object(p, None, NOW).unwrap();
        let mut list: Vec<ListEntry> = (1..=n)
            .map(|k| ListEntry {
                voting_key_id: key_id(k),
                weight: Some(1),
                valid: true,
            })
            .collect();
        list.extend([150, 151].map(|k| ListEntry {
            voting_key_id: key_id(k),
            weight: Some(4),
            valid: true,
        }));
        list.extend((0..997 - u32::from(n)).map(|_| ListEntry {
            voting_key_id: key_id(200),
            weight: Some(1),
            valid: true,
        }));
        store.set_masternode_list(&list);
        let resolved: HashMap<_, _> = (1..=n)
            .map(|k| {
                let r = ResolvedMasternode {
                    pro_tx_hash: [k; 32],
                    voting_key_id: key_id(k),
                    operator_key: [0; 48],
                    weight: 1,
                };
                (outpoint(k), r)
            })
            .collect();
        store.set_resolved(resolved.clone());
        for k in 1..=n {
            store.add_vote(yes(h, k, k), None, NOW).unwrap();
        }
        (store, h, resolved)
    }

    fn only_row(store: &GovernanceStore) -> ProposalRow {
        let t = build_table(&crate::DashNetwork::Regtest, store, 1_505, NOW);
        assert_eq!(t.threshold, 100);
        t.rows.into_iter().next().unwrap()
    }

    /// Sol r3 Major 1: 99 exact YES against a threshold of 100. Key 150's
    /// holder fills its 2n + 2 unresolved slots with ABSTAIN votes for
    /// invented outpoints; its masternode's YES (Core counts it: 103) is
    /// refused by the per-key cap. The row is a close call with up to 4
    /// more YES, not a definite non-passing one (MG-02 D71).
    #[test]
    fn a_vote_the_key_cap_refuses_keeps_the_row_a_close_call() {
        // At 50, up to 4 more cannot reach the threshold: the row is a
        // close call all the same.
        for n in [99, 50] {
            let (mut store, h, _) = resolved_yes_store(n);
            for op in 240..244 {
                store
                    .add_vote(funding(h, 150, op, VoteOutcome::Abstain), None, NOW)
                    .unwrap();
            }
            let before = only_row(&store);
            assert_eq!(before.tally.net_yes_estimate, i64::from(n));
            assert_eq!(before.status, ProposalStatus::Voting);
            assert!(!before.close_call);

            assert_eq!(
                store.add_vote(yes(h, 150, 250), None, NOW),
                Err(VoteRejection::KeyLimit)
            );
            let after = only_row(&store);
            assert_eq!(after.tally.net_yes_estimate, i64::from(n));
            let n = u32::from(n);
            assert_eq!((after.tally.yes.min, after.tally.yes.max), (n, n + 4));
            assert!(after.close_call, "{n}");
        }
    }

    /// Sol r3 Major 2: 101 exact YES against a threshold of 100, and an
    /// evonode's honest NO at outpoint 160 (Core: 97, not passing). Key
    /// 151 re-signs it first, and the copy keeps the hash. Once 160
    /// resolves to key 150 the copy is dropped and the refetch fails: the
    /// NO stays up to 4 and the row a close call, not a definite pass,
    /// until the honest copy lands (MG-02 D72).
    #[test]
    fn a_failed_refetch_keeps_the_row_a_close_call() {
        let (mut store, h, mut resolved) = resolved_yes_store(101);
        // The outpoint's older YES, by its own key.
        let older = GovernanceVote::funding(outpoint(160), h, VoteOutcome::Yes, NOW - 200);
        store.add_vote(signed(&older, 150), None, NOW).unwrap();
        let honest = funding(h, 150, 160, VoteOutcome::No);
        store.add_vote(signed(&honest, 151), None, NOW).unwrap();
        let forged = only_row(&store);
        assert_eq!((forged.tally.no.min, forged.tally.no.max), (0, 4));
        assert!(forged.close_call);

        resolved.insert(
            outpoint(160),
            ResolvedMasternode {
                pro_tx_hash: [160; 32],
                voting_key_id: key_id(150),
                operator_key: [0; 48],
                weight: 4,
            },
        );
        store.set_resolved(resolved);
        assert_eq!(store.take_refetch(), vec![honest.hash()]);
        // Every peer answers `notfound`: nothing more lands. The older YES
        // may be replaced, so `getcurrentvotes` shows it as the tally
        // counts it, not exactly.
        let failed = only_row(&store);
        assert_eq!((failed.tally.no.min, failed.tally.no.max), (0, 4));
        assert_eq!((failed.tally.yes.min, failed.tally.yes.max), (101, 105));
        assert!(failed.close_call);
        let row = |store: &GovernanceStore| {
            vote_rows(store, &h)
                .into_iter()
                .find(|r| r.outpoint == outpoint_short(&outpoint(160)))
                .unwrap()
        };
        assert!(!row(&store).exact);

        store.add_vote(honest, None, NOW).unwrap();
        let settled = only_row(&store);
        assert_eq!(settled.tally.net_yes_estimate, 97);
        assert_eq!((settled.tally.no.min, settled.tally.no.max), (4, 4));
        assert!(!settled.close_call);
        assert_eq!(settled.status, ProposalStatus::Voting);
        assert!(row(&store).exact);
    }

    /// BL-229 (D80): evonode key 150 changed its vote on the proposal from
    /// YES to NO, both signed by its own key, against 101 exact YES and a
    /// threshold of 100. While its outpoint is unresolved the NO counts 0
    /// to 4 and the row is a close call; once the resolver fetched its
    /// ProRegTx and the follower handed the store the outpoint, the NO
    /// counts 4 (Core: 97, not passing) and the row leaves the close call.
    #[tokio::test]
    async fn an_updated_vote_leaves_the_close_call_once_the_list_resolves_the_voter() {
        use super::super::collateral::tests::{
            NullSink, Script, external, registration, resolve_all, snapshot, summary,
        };
        use dw_protx::params::EVONODE_COLLATERAL;

        // Keys 1..=101 regular, their collaterals outpoints 1..=101; key 150
        // an evonode whose ProRegTx funds its collateral itself; key 151 an
        // evonode, and key 200 fills the list up to a weight of 1,005.
        let regular: Vec<_> = (1..=101).map(|k| external(outpoint(k))).collect();
        let null = OutPoint::new(Txid::all_zeros(), 0);
        let evo = registration(
            null,
            true,
            vec![dashcore::TxOut {
                value: EVONODE_COLLATERAL,
                script_pubkey: dashcore::ScriptBuf::new(),
            }],
            0,
        );
        let evo_outpoint = OutPoint::new(evo.txid(), 0);
        let mut entries: Vec<_> = regular
            .iter()
            .zip(1u8..)
            .map(|(tx, k)| summary(tx.txid().to_byte_array(), key_id(k), false))
            .collect();
        entries.push(summary(evo.txid().to_byte_array(), key_id(150), true));
        entries.push(summary([151; 32], key_id(151), true));
        entries.extend((0..896u16).map(|n| {
            let mut h = [200; 32];
            h[..2].copy_from_slice(&n.to_le_bytes());
            summary(h, key_id(200), false)
        }));
        let list = snapshot(entries);
        let source = Script::with(regular.iter().cloned().chain([evo.clone()]));
        source
            .failing
            .lock()
            .unwrap()
            .insert(evo.txid().to_byte_array());
        let rt = GovernanceRuntime::new(
            crate::DashNetwork::Regtest,
            std::sync::Arc::new(NullSink),
            &tokio::runtime::Handle::current(),
            true,
            Some(source.clone()),
        );
        rt.shared.store().set_confirmations(1);
        rt.apply_list(list.clone());
        resolve_all(&rt.collaterals, &list, source.as_ref(), &None).await;
        rt.apply_resolved();

        let data = format!(
            r#"{{"end_epoch":{},"name":"p","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":5,"start_epoch":1,"type":1,"url":"http://x.y"}}"#,
            NOW + 10_000
        );
        let p = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
        let h = p.hash();
        let row = |rt: &GovernanceRuntime| only_row(&rt.shared.store());
        {
            let mut store = rt.shared.store();
            store.add_object(p, None, NOW).unwrap();
            for k in 1..=101 {
                store.add_vote(yes(h, k, k), None, NOW).unwrap();
            }
            let older = GovernanceVote::funding(evo_outpoint, h, VoteOutcome::Yes, NOW - 200);
            store.add_vote(signed(&older, 150), None, NOW).unwrap();
            let newer = GovernanceVote::funding(evo_outpoint, h, VoteOutcome::No, NOW - 100);
            store.add_vote(signed(&newer, 150), None, NOW).unwrap();
        }
        // Unresolved, its vote may be either copy and weigh 0 to 4 (D64).
        let open = row(&rt);
        assert_eq!((open.tally.yes.min, open.tally.yes.max), (101, 105));
        assert_eq!((open.tally.no.min, open.tally.no.max), (0, 4));
        assert!(open.close_call);

        source.failing.lock().unwrap().clear();
        resolve_all(&rt.collaterals, &list, source.as_ref(), &None).await;
        // The resolver wakes the follower, which hands the store the
        // outpoint.
        rt.apply_resolved();
        let settled = row(&rt);
        assert_eq!(settled.tally.net_yes_estimate, 97);
        assert_eq!((settled.tally.no.min, settled.tally.no.max), (4, 4));
        assert_eq!((settled.tally.yes.min, settled.tally.yes.max), (101, 101));
        assert!(!settled.close_call);
        assert_eq!(settled.status, ProposalStatus::Voting);
    }

    /// MG-02 D65, D69: the fundable set is dash-qt's over the proposals not
    /// lapsed. One for a later cycle is passing and allocated, as dash-qt
    /// shows it; one at its end epoch (not pruned yet) is neither, as
    /// Core's trigger builder would not pay it. `allocated` is the sum of
    /// the passing rows.
    #[test]
    fn the_fundable_set_is_dash_qts_over_proposals_not_lapsed() {
        let mut store = GovernanceStore::new(Network::Regtest);
        store.set_confirmations(1);
        let proposal = |store: &mut GovernanceStore, name: &str, start: i64, end: i64| {
            let data = format!(
                r#"{{"end_epoch":{end},"name":"{name}","payment_address":"yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf","payment_amount":1,"start_epoch":{start},"type":1,"url":"http://x.y"}}"#
            );
            let o = GovernanceObject::new_proposal(1, NOW, data.into_bytes());
            let h = o.hash();
            store.add_object(o, None, NOW).unwrap();
            h
        };
        let open = proposal(&mut store, "open", 1, NOW + 100_000);
        let later = proposal(&mut store, "later", NOW + 1_000_000, NOW + 2_000_000);
        let ending = proposal(&mut store, "ending", 1, NOW + 50);
        store.set_masternode_list(
            &(1..=10)
                .map(|k| ListEntry {
                    voting_key_id: key_id(k),
                    weight: Some(1),
                    valid: true,
                })
                .collect::<Vec<_>>(),
        );
        for h in [open, later, ending] {
            for k in 1..=10 {
                store.add_vote(yes(h, k, k), None, NOW).unwrap();
            }
        }
        let t = build_table(&crate::DashNetwork::Regtest, &store, 1_505, NOW + 50);
        let status = |name: &str| t.rows.iter().find(|r| r.name == name).unwrap().status;
        assert_eq!(status("open"), ProposalStatus::Passing);
        assert_eq!(status("later"), ProposalStatus::Passing);
        assert_eq!(status("ending"), ProposalStatus::Lapsed);
        assert_eq!(t.allocated, 2 * 100_000_000);
    }
}
