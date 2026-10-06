//! The proposal list, details, info panel, clock and Create Proposal
//! helpers: dw-governance's rules over the synced store, the SPV masternode
//! list and the tip.

use std::collections::HashSet;
use std::sync::Arc;

use dw_governance::clock::{self, superblock_budget, upcoming_superblocks};
use dw_governance::object::{GovernanceObject, display_hex, parse_display_hex};
use dw_governance::params::{PROPOSAL_FEE_CONFIRMATIONS, governance_params};
use dw_governance::proposal::{self, ChainPoint, ProposalData};
use dw_governance::store::GovernanceStore;
use dw_governance::tally::{
    self, BudgetCandidate, Fundable, MasternodeEntry, MasternodeIndex, StatusInput, Tally,
};
use platform_wallet::masternode::MasternodeListSummary;

use super::voting::Controlled;
use super::{
    GovernanceClock, GovernanceFailure, GovernanceInfo, MyVotes, ProposalDetail, ProposalDraft,
    ProposalField, ProposalQuery, ProposalRow, ProposalSource, SuperblockDate,
};
use crate::{EngineError, NetworkSession};

/// Everything a row needs besides the object.
pub(super) struct Context {
    pub tip: ChainPoint,
    pub index: MasternodeIndex,
    pub threshold: u32,
    pub fundable: Fundable,
    pub fundable_set: HashSet<String>,
    pub in_window: bool,
    pub now: i64,
    /// Masternodes whose voting keys the wallets hold; `None` when the
    /// wallets hold none.
    pub controlled: Option<Vec<Controlled>>,
}

pub(super) fn index_of(list: &[MasternodeListSummary]) -> MasternodeIndex {
    let entries: Vec<MasternodeEntry> = list
        .iter()
        .map(|m| MasternodeEntry {
            pro_tx_hash: m.pro_tx_hash,
            voting_key_id: m.voting_key_id,
            is_valid: m.is_valid,
            is_evonode: m.is_evonode,
        })
        .collect();
    MasternodeIndex::new(&entries)
}

/// The budget candidates of every synced proposal.
fn candidates(store: &GovernanceStore, index: &MasternodeIndex) -> Vec<BudgetCandidate> {
    store
        .proposals()
        .filter_map(|(o, p)| {
            let hash = o.object.hash();
            let t = tally::tally(store.votes_on(&hash), index);
            Some(BudgetCandidate {
                hash: o.hash_hex.clone(),
                payment_amount: p.payment_amount?,
                absolute_yes: t.absolute_yes(),
            })
        })
        .collect()
}

impl Context {
    pub(super) fn new(
        store: &GovernanceStore,
        list: Option<&[MasternodeListSummary]>,
        tip: ChainPoint,
        network: dashcore::Network,
        controlled: Option<Vec<Controlled>>,
    ) -> Self {
        let p = governance_params(network);
        let index = list.map(index_of).unwrap_or_default();
        let threshold = index.threshold(p.min_quorum);
        let (_, next) = clock::nearest_superblocks(&p, tip.height);
        let fundable = tally::fundable(
            &candidates(store, &index),
            threshold,
            superblock_budget(network, next),
        );
        let fundable_set = fundable.hashes.iter().cloned().collect();
        Self {
            tip,
            index,
            threshold,
            fundable,
            fundable_set,
            in_window: clock::in_maturity_window(&p, tip.height),
            now: tip.time as i64,
            controlled,
        }
    }

    /// One list row. `own`: the collateral confirmations and broadcast flag
    /// of the wallet's own proposal; `None` for a synced object (peers
    /// relay objects only after 6 confirmations).
    pub(super) fn row(
        &self,
        store: &GovernanceStore,
        object: &GovernanceObject,
        data: &ProposalData,
        own: Option<(u32, bool)>,
    ) -> ProposalRow {
        let hash = object.hash();
        let hash_hex = display_hex(&hash);
        let t: Tally = tally::tally(store.votes_on(&hash), &self.index);
        let funded_height = store.funded_height(&hash_hex, self.tip.height);
        let (confirmations, broadcast) = match own {
            Some((c, b)) => (c, b || store.has_object(&hash)),
            None => (PROPOSAL_FEE_CONFIRMATIONS, true),
        };
        let status = tally::status(&StatusInput {
            funded_height,
            lapsed: data.end_epoch.is_some_and(|e| self.now >= e),
            collateral_confirmations: confirmations,
            required_confirmations: PROPOSAL_FEE_CONFIRMATIONS,
            broadcast,
            in_maturity_window: self.in_window,
            absolute_yes: t.absolute_yes(),
            threshold: self.threshold,
            fundable: self.fundable_set.contains(&hash_hex),
        });
        let my_votes = self.controlled.as_ref().map(|mns| {
            let mut m = MyVotes::default();
            for mn in mns {
                let outcome = mn.collateral.and_then(|c| store.outcome_of(&hash, &c));
                match outcome {
                    Some(dw_governance::vote::VoteOutcome::Yes) => m.yes += mn.weight,
                    Some(dw_governance::vote::VoteOutcome::No) => m.no += mn.weight,
                    Some(dw_governance::vote::VoteOutcome::Abstain) => m.abstain += mn.weight,
                    None => m.unvoted += mn.weight,
                }
            }
            m
        });
        ProposalRow {
            hash: hash_hex,
            name: data.name.clone().unwrap_or_default(),
            url: data.url.clone().unwrap_or_default(),
            payment_address: data.payment_address.clone().unwrap_or_default(),
            payment_amount: data.payment_amount.unwrap_or(0),
            start_epoch: data.start_epoch.unwrap_or(0).max(0) as u64,
            end_epoch: data.end_epoch.unwrap_or(0).max(0) as u64,
            status,
            collateral_confirmations: own.map(|(c, _)| c),
            yes: t.yes,
            no: t.no,
            abstain: t.abstain,
            margin: t.absolute_yes() - i64::from(self.threshold),
            my_votes,
            funded_height,
        }
    }
}

fn title_matches(row: &ProposalRow, filter: &Option<String>) -> bool {
    match filter.as_deref().map(str::trim) {
        None | Some("") => true,
        Some(f) => row.name.to_lowercase().contains(&f.to_lowercase()),
    }
}

/// dash-qt's order: status group, then the deficit.
fn sort_rows(rows: &mut [ProposalRow], threshold: u32) {
    rows.sort_by_key(|r| {
        (
            tally::sort_key(r.status, r.margin + i64::from(threshold), threshold),
            r.hash.clone(),
        )
    });
}

impl NetworkSession {
    fn governance_params(&self) -> dw_governance::params::GovernanceParams {
        governance_params(self.network.core_network())
    }

    /// The tip, or `governance.not_synced` before headers synced.
    pub(super) fn require_tip(&self) -> Result<ChainPoint, EngineError> {
        self.governance_tip()
            .ok_or_else(|| GovernanceFailure::NotSynced.into())
    }

    /// The proposal list (QT-128/129), in dash-qt's order.
    pub async fn proposals(self: &Arc<Self>, query: ProposalQuery) -> Result<Vec<ProposalRow>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            if matches!(query.source, ProposalSource::Active) && !this.governance.enabled() {
                return Err(GovernanceFailure::SyncDisabled.into());
            }
            let tip = this.require_tip()?;
            let list = this.governance_masternode_list().await;
            let controlled = this.controlled_masternodes(None, list.as_deref()).await;
            let own = match &query.source {
                ProposalSource::Mine(w) => Some(this.own_proposals(*w).await?),
                ProposalSource::Active => None,
            };
            let network = this.network.core_network();
            let cycle_secs = {
                let p = this.governance_params();
                i64::from(p.superblock_cycle) * i64::from(p.target_spacing_secs)
            };
            let store = this.governance.shared.store();
            let ctx = Context::new(&store, list.as_deref(), tip, network, controlled);
            let mut rows: Vec<ProposalRow> = match own {
                Some(records) => records
                    .iter()
                    .filter_map(|r| {
                        let data = proposal::parse_proposal(&r.object.data)?;
                        Some(ctx.row(&store, &r.object, &data, Some((r.confirmations, r.submitted))))
                    })
                    .collect(),
                None => store
                    .proposals()
                    .filter(|(_, p)| p.end_epoch.is_none_or(|e| e + cycle_secs > ctx.now))
                    .map(|(o, p)| ctx.row(&store, &o.object, p, None))
                    .collect(),
            };
            rows.retain(|r| title_matches(r, &query.title_filter));
            sort_rows(&mut rows, ctx.threshold);
            Ok(rows)
        })
        .await
    }

    /// The details view (QT-130) of a synced or own proposal.
    pub async fn proposal_detail(self: &Arc<Self>, hash: String) -> Result<ProposalDetail, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let key = parse_display_hex(&hash)
                .ok_or_else(|| EngineError::InvalidArgument("hash must be 64 hex characters".into()))?;
            let tip = this.require_tip()?;
            let own = this.find_own_proposal(&hash).await?;
            let list = this.governance_masternode_list().await;
            let controlled = this.controlled_masternodes(None, list.as_deref()).await;
            let network = this.network.core_network();
            let p = this.governance_params();
            let store = this.governance.shared.store();
            let (object, own_state) = match (store.object(&key), &own) {
                (Some(o), _) => (o.object.clone(), own.as_ref().map(|r| (r.confirmations, r.submitted))),
                (None, Some(r)) => (r.object.clone(), Some((r.confirmations, r.submitted))),
                (None, None) => return Err(GovernanceFailure::ProposalNotFound(hash).into()),
            };
            let data = proposal::parse_proposal(&object.data).unwrap_or_default();
            let ctx = Context::new(&store, list.as_deref(), tip, network, controlled);
            let row = ctx.row(&store, &object, &data, own_state);
            let mut collateral = object.collateral_hash;
            collateral.reverse();
            Ok(ProposalDetail {
                row,
                parent_hash: display_hex(&object.parent_hash),
                collateral_txid: hex::encode(collateral),
                created_at: object.time.max(0) as u64,
                payments: data.payments_requested(&p),
                raw_json: object.data_text(),
            })
        })
        .await
    }

    /// The next `count` (≤ 12) superblocks with estimated times.
    pub fn superblock_dates(&self, count: u32) -> Result<Vec<SuperblockDate>, EngineError> {
        let _op = self.try_enter()?;
        if count > proposal::PAYMENT_DATE_CHOICES {
            return Err(EngineError::InvalidArgument(format!(
                "count {count} is above {}",
                proposal::PAYMENT_DATE_CHOICES
            )));
        }
        let tip = self.require_tip()?;
        let p = self.governance_params();
        Ok(upcoming_superblocks(&p, tip.height, count)
            .into_iter()
            .map(|height| SuperblockDate {
                height,
                estimated_time: clock::estimated_time(&p, tip.height, tip.time, height),
            })
            .collect())
    }

    /// Every failing field of `draft` in wizard order; empty = valid.
    pub fn validate_proposal(&self, draft: &ProposalDraft) -> Result<Vec<ProposalField>, EngineError> {
        let _op = self.try_enter()?;
        let tip = self.require_tip()?;
        Ok(proposal::validate(self.network.core_network(), draft, tip)
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// The first failing field as `governance.invalid_proposal`.
    pub(super) fn check_draft(&self, draft: &ProposalDraft, tip: ChainPoint) -> Result<(), EngineError> {
        match proposal::validate(self.network.core_network(), draft, tip).first() {
            Some(f) => Err(GovernanceFailure::InvalidProposal((*f).into()).into()),
            None => Ok(()),
        }
    }

    /// "View JSON".
    pub fn proposal_json(&self, draft: &ProposalDraft) -> Result<String, EngineError> {
        let _op = self.try_enter()?;
        let tip = self.require_tip()?;
        self.check_draft(draft, tip)?;
        Ok(proposal::proposal_json(self.network.core_network(), draft, tip))
    }

    /// "View Payload": the hex of the data JSON.
    pub fn proposal_payload_hex(&self, draft: &ProposalDraft) -> Result<String, EngineError> {
        Ok(hex::encode(self.proposal_json(draft)?))
    }

    /// Recomputes the cached `budget_committed` of the clock.
    pub(super) async fn refresh_budget_committed(&self) {
        let value = if self.governance.sync_state().last_synced_at.is_none() {
            None
        } else if let Some(tip) = self.governance_tip() {
            let list = self.governance_masternode_list().await;
            let network = self.network.core_network();
            let store = self.governance.shared.store();
            let ctx = Context::new(&store, list.as_deref(), tip, network, None);
            let p = self.governance_params();
            let (_, next) = clock::nearest_superblocks(&p, tip.height);
            let available = superblock_budget(network, next);
            (available > 0).then(|| ctx.fundable.allocated as f64 / available as f64)
        } else {
            None
        };
        *self
            .governance
            .budget_committed
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = value;
    }

    /// The status-bar clock (QT-026). In-memory read.
    pub fn governance_clock(&self) -> Result<GovernanceClock, EngineError> {
        let _op = self.try_enter()?;
        let tip = self.require_tip()?;
        let p = self.governance_params();
        let c = clock::clock(&p, tip.height, tip.time);
        Ok(GovernanceClock {
            cycle_progress: c.cycle_progress,
            next_superblock: c.next_superblock,
            blocks_to_superblock: c.blocks_to_superblock,
            superblock_eta: c.superblock_eta,
            voting_cutoff: c.voting_cutoff,
            voting_open: c.voting_open,
            budget_committed: *self
                .governance
                .budget_committed
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        })
    }

    /// The info panel (QT-134).
    pub async fn governance_info(self: &Arc<Self>) -> Result<GovernanceInfo, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let sync = this.governance.sync_state();
            let p = this.governance_params();
            let network = this.network.core_network();
            let tip = this.governance_tip();
            let list = this.governance_masternode_list().await;
            let controlled = this.controlled_masternodes(None, list.as_deref()).await;
            let (masternodes_controlled, votes_controlled) = controlled
                .as_ref()
                .map(|mns| {
                    let valid: Vec<_> = mns.iter().filter(|m| m.is_valid).collect();
                    (valid.len() as u32, valid.iter().map(|m| m.weight).sum())
                })
                .unwrap_or((0, 0));
            let synced = sync.last_synced_at.is_some();
            let mut info = GovernanceInfo {
                sync,
                superblock_cycle: p.superblock_cycle,
                last_superblock: None,
                next_superblock: None,
                next_superblock_eta: None,
                voting_cutoff: None,
                masternodes_voting: None,
                masternodes_eligible: None,
                evonodes_voting: None,
                evonodes_eligible: None,
                passing_threshold: None,
                masternodes_controlled,
                votes_controlled,
                proposal_count: None,
                passing: None,
                failing: None,
                unfunded: None,
                unfunded_short: None,
                budget_available: None,
                budget_allocated: None,
            };
            let Some(tip) = tip else { return Ok(info) };
            let c = clock::clock(&p, tip.height, tip.time);
            info.last_superblock = Some(c.last_superblock);
            info.next_superblock = Some(c.next_superblock);
            info.next_superblock_eta = Some(c.superblock_eta);
            info.voting_cutoff = Some(c.voting_cutoff);
            info.budget_available = Some(superblock_budget(network, c.next_superblock));
            if let Some(list) = &list {
                let idx = index_of(list);
                info.masternodes_eligible = Some(idx.valid_regular);
                info.evonodes_eligible = Some(idx.valid_evonodes);
                info.passing_threshold = Some(idx.threshold(p.min_quorum));
            }
            if !synced || list.is_none() {
                return Ok(info);
            }
            let store = this.governance.shared.store();
            let ctx = Context::new(&store, list.as_deref(), tip, network, None);
            let (mut count, mut passing, mut unfunded, mut short) = (0u32, 0u32, 0u32, 0u64);
            let (mut max_regular, mut max_evo) = (0u32, 0u32);
            for (o, data) in store.proposals() {
                let hash = o.object.hash();
                let t = tally::tally(store.votes_on(&hash), &ctx.index);
                max_regular = max_regular.max(t.regular_voters);
                max_evo = max_evo.max(t.evonode_voters);
                count += 1;
                if t.absolute_yes() >= i64::from(ctx.threshold) {
                    passing += 1;
                    if !ctx.fundable_set.contains(&o.hash_hex) {
                        unfunded += 1;
                        short += data.payment_amount.unwrap_or(0);
                    }
                }
            }
            info.masternodes_voting = Some(max_regular);
            info.evonodes_voting = Some(max_evo);
            info.proposal_count = Some(count);
            info.passing = Some(passing);
            info.failing = Some(count - passing);
            info.unfunded = Some(unfunded);
            info.unfunded_short = Some(short);
            info.budget_allocated = Some(ctx.fundable.allocated);
            Ok(info)
        })
        .await
    }
}
