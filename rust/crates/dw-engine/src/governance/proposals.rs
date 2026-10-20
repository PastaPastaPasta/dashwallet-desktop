//! The wallet's own proposals (MG-06, mg-engine §2.5): validated as Core
//! validates them, the fee paid under a "Proposal" lease, the proposal and
//! its signed fee saved before the fee is broadcast (MG-00 D18), and the
//! proposal submitted by the wallet itself once the fee has a confirmation
//! (MG-00 D16).
//!
//! Submitting is announcing: the object is queued in the governance outbox
//! and served on `getdata`, and a peer's `getdata` is the only sign that it
//! took it (D6). A Core peer holds it postponed until the fee has six
//! confirmations, and only then relays it (D19); it never announces back
//! what the wallet announced to it, so "public" comes from a fresh
//! connection's sync (D20).
//!
//! States are derived on every read from facts (D5): the saved row, the
//! fee in the wallet's history, the store and the per-session pace below.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashcore::hashes::Hash;
use dashcore::{OutPoint, ScriptBuf, Transaction, TxOut};
use dw_appdb::ProposalRow;
use dw_governance::object::{GovernanceObject, display_hex, parse_display_hex};
use dw_governance::params::PROPOSAL_FEE_CONFIRMATIONS;
use dw_governance::proposal::{self, ChainPoint, Draft};
use dw_governance::sync::Phase;
use dw_governance::validate::check_object;

use super::reads::now;
use super::records::{
    MyProposal, MyProposalState, ProposalDraft, ProposalField, ProposalPreview, SubmitRefusal,
    SubmitWait,
};
use super::voting::parse_outpoint;
use super::{Governance, GovernanceError};
use crate::events::unix_now;
use crate::history::depth_of;
use crate::platform::lease::{Outcome, lease_string};
use crate::platform::{FlowKind, PlatformError};
use crate::send::{CoinSource, TxDraft, attempt_outcome};
use crate::{EngineError, NetworkSession, WalletId};

/// Confirmations a Core peer needs before it takes a proposal at all
/// (`GOVERNANCE_MIN_RELAY_FEE_CONFIRMATIONS`, governance/object.h:89).
const MIN_RELAY_CONFIRMATIONS: u32 = 1;
/// How long the fee's first confirmation is seen, by a caught-up wallet,
/// before the proposal is announced: a peer without that block yet would
/// find no collateral and penalise the wallet (D21).
const PROPAGATION_WAIT: Duration = Duration::from_secs(15);
/// How long a synced run's peers have to fetch the announced proposal
/// before it reads `ready_to_submit{no_peer_accepted}` (D7).
const FETCH_WAIT: Duration = Duration::from_secs(60);
/// How long after the fee's sixth confirmation the proposal may stay
/// unannounced by peers before a fresh run looks for it (D20).
const ECHO_WAIT: Duration = Duration::from_secs(120);
/// The first retry's delay; each retry doubles it up to [`MAX_RETRY`]. A
/// retry is a new sync run, with new connections (D7).
const FIRST_RETRY: Duration = Duration::from_secs(60);
const MAX_RETRY: Duration = Duration::from_secs(30 * 60);

/// One own proposal's submission pace in this session (not stored: a
/// restart starts it again).
#[derive(Debug, Default)]
pub(crate) struct Pace {
    /// The fee's depth at the last tick.
    depth: u32,
    /// The fee lost its confirmations after a peer took the proposal: the
    /// peers dropped their postponed copies, so a fetch is expected again.
    lost: bool,
    /// Since when the caught-up wallet has seen the fee mined (D21).
    mined_since: Option<Instant>,
    /// The synced run the timers below belong to (its `last_synced_at`):
    /// a new run's peers are new connections, so the waits start again.
    run: Option<u64>,
    /// Since when this synced run was announcing it and no peer fetched it.
    unfetched_since: Option<Instant>,
    /// Since when, in this synced run, the fee has six confirmations
    /// without the proposal being public.
    six_since: Option<Instant>,
    next_retry: Option<Instant>,
    backoff: Duration,
    no_peer_accepted: bool,
}

impl Pace {
    /// Whether a retry is due now; if so, schedules the next one.
    fn retry_due(&mut self, now: Instant) -> bool {
        if self.next_retry.is_some_and(|t| now < t) {
            return false;
        }
        self.backoff = if self.backoff.is_zero() {
            FIRST_RETRY
        } else {
            (self.backoff * 2).min(MAX_RETRY)
        };
        self.next_retry = Some(now + self.backoff);
        true
    }

    fn progressed(&mut self) {
        self.unfetched_since = None;
        self.no_peer_accepted = false;
        self.next_retry = None;
        self.backoff = Duration::ZERO;
    }

    /// Keys the waits to `run` (`None`: not synced), restarting them when
    /// it changes.
    fn in_run(&mut self, run: Option<u64>) {
        if self.run != run {
            self.run = run;
            self.unfetched_since = None;
            self.six_since = None;
        }
    }

    /// Not announced: no wait runs.
    fn unannounced(&mut self) {
        self.unfetched_since = None;
        self.six_since = None;
    }
}

/// The facts a proposal's state is derived from (D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    /// The fee's depth; `None` while the wallet's history lacks it.
    pub depth: Option<u32>,
    /// The fee was handed to the network (`fee_sent_at`).
    pub sent: bool,
    /// Another confirmed transaction spends one of the fee's coins.
    pub conflicted: bool,
    /// A peer took it, and its fee has not lost its confirmations since.
    pub submitted: bool,
    /// The store holds it confirmed; the status once synced.
    pub public: Option<Option<super::ProposalStatus>>,
    pub expired: bool,
    /// Why it is not submitted yet, if it should be.
    pub wait: Option<SubmitWait>,
}

/// MG-06 §2.3.
pub(crate) fn derive_state(f: Facts) -> MyProposalState {
    if let Some(status) = f.public {
        return MyProposalState::Public { status };
    }
    if f.expired {
        return MyProposalState::Expired;
    }
    if f.conflicted {
        return MyProposalState::FeeFailed;
    }
    match f.depth {
        None if !f.sent => MyProposalState::FeeUnsent,
        None | Some(0) => MyProposalState::FeePaid { confirmations: 0 },
        Some(confirmations) if f.submitted => MyProposalState::Confirming { confirmations },
        Some(confirmations) => match f.wait {
            Some(reason) => MyProposalState::ReadyToSubmit { reason },
            None => MyProposalState::FeePaid { confirmations },
        },
    }
}

/// One saved row, decoded.
struct Saved {
    wallet: WalletId,
    row: ProposalRow,
    object: GovernanceObject,
    /// The object's data, parsed.
    data: proposal::ProposalData,
    fee: Transaction,
}

impl Saved {
    fn decode(row: ProposalRow) -> Result<Self, EngineError> {
        let bad = |what: &str| EngineError::Storage(format!("proposal row: {what}"));
        let wallet: WalletId = row.wallet_id.parse().map_err(|_| bad("wallet id"))?;
        let mut object = GovernanceObject::new_proposal(row.revision, row.time, row.data.clone());
        object.collateral_hash = row.fee_txid;
        if object.hash() != row.hash {
            return Err(bad("hash"));
        }
        let fee: Transaction =
            dashcore::consensus::deserialize(&row.fee_tx).map_err(|_| bad("fee transaction"))?;
        if fee.txid().to_byte_array() != row.fee_txid {
            return Err(bad("fee txid"));
        }
        let data = proposal::parse_proposal(&row.data).unwrap_or_default();
        Ok(Self {
            wallet,
            row,
            object,
            data,
            fee,
        })
    }

    fn key(&self) -> (WalletId, [u8; 32]) {
        (self.wallet, self.row.hash)
    }
}

/// The fee's depth at `tip` (`None`: not in the wallet's history) and
/// whether another confirmed transaction spends one of its coins.
fn fee_status(
    session: &NetworkSession,
    wallet: &WalletId,
    fee: &Transaction,
    tip: u32,
) -> (Option<u32>, bool) {
    let txid = fee.txid();
    let coins: HashSet<OutPoint> = fee.input.iter().map(|i| i.previous_output).collect();
    session.hub.history.read(wallet, |txs| {
        let depth = txs.get(&txid).map(|e| depth_of(e, tip));
        let conflicted = txs.iter().any(|(id, e)| {
            *id != txid
                && depth_of(e, tip) > 0
                && e.tx
                    .input
                    .iter()
                    .any(|i| coins.contains(&i.previous_output))
        });
        (depth, conflicted)
    })
}

/// The facade's name of a failing draft field (kept out of `records.rs`,
/// whose impls are contract surface).
fn field_of(f: proposal::Field) -> ProposalField {
    use proposal::Field as F;
    match f {
        F::Name => ProposalField::Name,
        F::Url => ProposalField::Url,
        F::PaymentAddress => ProposalField::PaymentAddress,
        F::PaymentAmount => ProposalField::PaymentAmount,
        F::PaymentCount => ProposalField::PaymentCount,
        F::FirstPayment => ProposalField::FirstPayment,
        F::Payload => ProposalField::Payload,
    }
}

impl Governance {
    /// The proposal object for `draft` created at `time`, checked as Core
    /// checks it before taking a fee (D11), and the fee's coin control.
    fn build(
        &self,
        draft: &ProposalDraft,
        time: i64,
    ) -> Result<(GovernanceObject, Vec<OutPoint>), GovernanceError> {
        let coins = (draft.coins.iter())
            .map(|c| parse_outpoint(c))
            .collect::<Result<Vec<_>, _>>()?;
        let tip = self.tip()?;
        let network = self.session.network.core_network();
        let d = Draft {
            name: draft.name.clone(),
            url: draft.url.clone(),
            payment_address: draft.payment_address.clone(),
            payment_amount: draft.payment_amount,
            payment_count: draft.payment_count,
            first_superblock_height: draft.first_superblock_height,
        };
        let at = ChainPoint {
            height: tip.height,
            time: tip.eta_base,
        };
        let bad = proposal::validate(network, &d, at);
        if !bad.is_empty() {
            return Err(GovernanceError::InvalidProposal {
                fields: bad.into_iter().map(field_of).collect(),
            });
        }
        let object = proposal::proposal_object(network, &d, at, time);
        // Core's `ValidateProposal` with the expiration check, as `gobject
        // prepare` runs it: what a peer would refuse costs no fee.
        if let Err(e) = check_object(&object, network, time, false) {
            tracing::warn!(?e, "a validated proposal draft fails Core's checks");
            return Err(GovernanceError::InvalidProposal {
                fields: vec![ProposalField::Payload],
            });
        }
        Ok((object, coins))
    }

    /// The fee transaction's draft: `OP_RETURN <hash>` of the proposal fee
    /// (Core `GenBudgetSystemCollateralTx`, D1).
    fn fee_draft(
        &self,
        wallet: WalletId,
        object: &GovernanceObject,
        coins: Vec<OutPoint>,
    ) -> Result<Arc<TxDraft>, GovernanceError> {
        let draft = self.session.new_tx_draft(wallet)?;
        if !coins.is_empty() {
            draft.set_source(CoinSource::Outpoints(coins))?;
        }
        draft.set_data_output(TxOut {
            value: self.params().proposal_fee,
            script_pubkey: ScriptBuf::new_op_return(&object.hash()),
        })?;
        #[cfg(test)]
        if let Some(seam) = self.runtime()?.fee_broadcaster.lock().unwrap().clone() {
            *draft.broadcaster.lock().unwrap() = Some(seam);
        }
        Ok(draft)
    }

    /// The review step's facts: the proposal as it would be created now
    /// and the fee transaction's estimated network fee. Nothing is
    /// reserved, signed or saved.
    pub async fn preview_proposal(
        &self,
        wallet: WalletId,
        draft: ProposalDraft,
    ) -> Result<ProposalPreview, GovernanceError> {
        let (object, coins) = self.build(&draft, now())?;
        let estimate = self.fee_draft(wallet, &object, coins)?.estimate().await?;
        let data = proposal::parse_proposal(&object.data).unwrap_or_default();
        Ok(ProposalPreview {
            hash: display_hex(&object.hash()),
            data_json: String::from_utf8_lossy(&object.data).into_owned(),
            data_hex: hex::encode(&object.data),
            start_epoch: data.start_epoch.unwrap_or_default(),
            end_epoch: data.end_epoch.unwrap_or_default(),
            fee: self.params().proposal_fee,
            network_fee: estimate.fee,
        })
    }

    /// Creates `draft`: pays its fee from `wallet` under a "Proposal" lease
    /// begun from `grant` (a `Spend` grant of `wallet`), saving the proposal
    /// and its signed fee before the fee is broadcast (MG-06 §2.1). The
    /// wallet submits it once the fee confirms.
    pub async fn create_proposal(
        &self,
        wallet: WalletId,
        draft: ProposalDraft,
        grant: String,
    ) -> Result<MyProposal, GovernanceError> {
        let (object, coins) = self.build(&draft, now())?;
        {
            // Not held across the payment: its prepare and broadcast each
            // enter on their own, and the gate is not reentrant.
            let _op = self.session.enter().await?;
            self.session.require_wallet(&wallet)?;
        }
        let fee_draft = self.fee_draft(wallet, &object, coins)?;
        let lease = (self.session)
            .begin_lease(wallet, FlowKind::Proposal, &[grant])
            .await
            .map_err(PlatformError::from)?;
        let paid = self
            .pay_fee(wallet, object, &fee_draft, lease_string(&lease.id))
            .await;
        lease.end();
        let hash = paid?;
        self.tick().await;
        self.my_proposal(wallet, &hash)
    }

    /// Signs the fee, saves the proposal, then broadcasts the fee. Returns
    /// the proposal's hash once saved and the fee handed off, or out with
    /// its outcome unknown.
    async fn pay_fee(
        &self,
        wallet: WalletId,
        mut object: GovernanceObject,
        fee_draft: &Arc<TxDraft>,
        lease: String,
    ) -> Result<[u8; 32], GovernanceError> {
        let prepared = fee_draft.prepare(lease).await?;
        let raw = prepared
            .raw()
            .ok_or_else(|| EngineError::Internal("the prepared fee has no bytes".into()))?;
        let fee: Transaction = dashcore::consensus::deserialize(&raw)
            .map_err(|e| EngineError::Internal(format!("the prepared fee: {e}")))?;
        let fee_txid = fee.txid().to_byte_array();
        object.collateral_hash = fee_txid;
        let hash = object.hash();
        let row = ProposalRow {
            wallet_id: wallet.to_string(),
            hash,
            revision: object.revision,
            time: object.time,
            data: object.data,
            fee_txid,
            fee_tx: raw,
            created_at: unix_now(),
            fee_sent_at: None,
            submitted_at: None,
            last_error: None,
        };
        // D4: saved, durably, before the broadcast, as Core writes the
        // object before committing the fee (rpc/governance.cpp:245-250).
        if let Err(e) = self
            .session
            .appdb_op(move |db| db.insert_proposal(&row))
            .await
        {
            let _ = fee_draft.abandon(prepared).await;
            // A save that failed after its commit leaves nothing either.
            let wallet_id = wallet.to_string();
            let _ = (self.session)
                .appdb_op(move |db| db.delete_proposal(&wallet_id, &hash))
                .await;
            return Err(e.into());
        }
        let sent = fee_draft.broadcast(prepared).await;
        let wallet_id = wallet.to_string();
        let outcome = attempt_outcome(&sent);
        let error = sent.as_ref().err().map(ToString::to_string);
        let recorded = (self.session)
            .appdb_op(move |db| match (error, outcome) {
                (None, _) => db
                    .mark_proposal_sent(&wallet_id, &hash, unix_now())
                    .map(drop),
                // Nothing was paid: the proposal goes with it (D4).
                (_, Outcome::NotSent) => db.delete_proposal(&wallet_id, &hash).map(drop),
                // Maybe out: kept, `fee_unsent` until the wallet sees it.
                (Some(e), _) => db.set_proposal_error(&wallet_id, &hash, Some(&e)),
            })
            .await;
        // The fee's fate is the answer: a row left behind reads
        // `fee_unsent`, and a fee that went out still shows in history.
        if let Err(e) = recorded {
            tracing::warn!(error = %e, "recording a proposal fee's broadcast");
        }
        match (sent, outcome) {
            (Err(e), Outcome::NotSent) => Err(e.into()),
            _ => Ok(hash),
        }
    }

    /// The wallet's own proposals, newest first, each with its state. A
    /// row that does not decode is skipped (and logged).
    pub fn my_proposals(&self, wallet: WalletId) -> Result<Vec<MyProposal>, GovernanceError> {
        self.session.require_wallet(&wallet)?;
        let appdb = self.session.live()?.appdb;
        let rows = appdb
            .proposals(Some(&wallet.to_string()))
            .map_err(EngineError::from)?;
        Ok(rows
            .into_iter()
            .filter_map(|row| match Saved::decode(row) {
                Ok(saved) => Some(self.view(&saved)),
                Err(e) => {
                    tracing::warn!(error = %e, "skipping a proposal row");
                    None
                }
            })
            .collect())
    }

    /// `wallet`'s saved proposal `hash`.
    fn saved(&self, wallet: WalletId, hash: &[u8; 32]) -> Result<Saved, GovernanceError> {
        let appdb = self.session.live()?.appdb;
        let row = (appdb.proposals(Some(&wallet.to_string())))
            .map_err(EngineError::from)?
            .into_iter()
            .find(|r| &r.hash == hash)
            .ok_or_else(|| GovernanceError::ProposalNotFound {
                hash: display_hex(hash),
            })?;
        Ok(Saved::decode(row)?)
    }

    fn my_proposal(
        &self,
        wallet: WalletId,
        hash: &[u8; 32],
    ) -> Result<MyProposal, GovernanceError> {
        Ok(self.view(&self.saved(wallet, hash)?))
    }

    /// The [`Facts`] of `saved` now; `status`: with a public proposal's
    /// status (a read of the store's tally).
    fn facts(&self, saved: &Saved, status: bool) -> Facts {
        let rt = self.runtime().ok();
        let tip = self.session.hub.tracker().tip_height().unwrap_or(0);
        let (depth, conflicted) = fee_status(&self.session, &saved.wallet, &saved.fee, tip);
        let confirmed = rt.is_some_and(|rt| {
            (rt.shared.store())
                .object(&saved.row.hash)
                .is_some_and(|o| o.confirmed)
        });
        let public = confirmed.then(|| {
            status
                .then(|| self.proposal(display_hex(&saved.row.hash)).ok())
                .flatten()
                .map(|d| d.row.status)
        });
        let (lost, no_peer_accepted) = rt.map_or((false, false), |rt| {
            rt.pace(&saved.key(), |p| (p.lost, p.no_peer_accepted))
        });
        let wait = match rt {
            Some(rt) if !rt.enabled() => Some(SubmitWait::SyncDisabled),
            Some(rt) if rt.shared.progress().phase != Phase::Synced => Some(SubmitWait::NotSynced),
            Some(_) if no_peer_accepted => Some(SubmitWait::NoPeerAccepted),
            Some(_) => None,
            None => Some(SubmitWait::SyncDisabled),
        };
        Facts {
            depth,
            sent: saved.row.fee_sent_at.is_some(),
            conflicted,
            submitted: saved.row.submitted_at.is_some() && !lost,
            public,
            expired: saved.data.end_epoch.is_some_and(|e| e <= now()),
            wait,
        }
    }

    fn view(&self, saved: &Saved) -> MyProposal {
        let data = &saved.data;
        MyProposal {
            hash: display_hex(&saved.row.hash),
            payment_count: data.payments_requested(&self.params()),
            name: data.name.clone().unwrap_or_default(),
            url: data.url.clone().unwrap_or_default(),
            payment_address: data.payment_address.clone().unwrap_or_default(),
            payment_amount: data.payment_amount.unwrap_or_default(),
            start_epoch: data.start_epoch.unwrap_or_default(),
            end_epoch: data.end_epoch.unwrap_or_default(),
            fee_txid: display_hex(&saved.row.fee_txid),
            created_at: saved.row.created_at,
            submitted_at: saved.row.submitted_at,
            state: derive_state(self.facts(saved, true)),
        }
    }

    /// The [Submit] of a proposal that is not submitted (UX-SPEC §4.25.3):
    /// a fee the wallet never saw, or saw unmined, is handed to the network
    /// again from its saved bytes (no grant: it was signed and charged once, D10), and a
    /// proposal no peer took is looked for by a new sync run at once (D7).
    pub async fn submit_proposal(
        &self,
        wallet: WalletId,
        hash: String,
    ) -> Result<MyProposal, GovernanceError> {
        let key = parse_display_hex(&hash).ok_or_else(|| {
            GovernanceError::from(EngineError::InvalidArgument(format!(
                "not a proposal hash: {hash:?}"
            )))
        })?;
        let mine = {
            // Not held across the resend, which takes the wallet.
            let _op = self.session.enter().await?;
            self.my_proposal(wallet, &key)?
        };
        let reason = match mine.state {
            MyProposalState::Public { .. } => Some(SubmitRefusal::Public),
            MyProposalState::Expired => Some(SubmitRefusal::Expired),
            MyProposalState::FeeFailed => Some(SubmitRefusal::FeeFailed),
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(GovernanceError::SubmitRefused { reason });
        }
        let rt = self.runtime()?;
        match mine.state {
            // Unseen, or seen unmined: dash-qt resends an unconfirmed
            // transaction too (QT-091).
            MyProposalState::FeeUnsent | MyProposalState::FeePaid { confirmations: 0 } => {
                self.resend_fee(wallet, &key).await?;
            }
            _ if !rt.enabled() => return Err(GovernanceError::SyncDisabled),
            MyProposalState::ReadyToSubmit {
                reason: SubmitWait::NoPeerAccepted,
            } => {
                // The new run's wait starts once it is synced (D7).
                rt.with_pace((wallet, key), |p| {
                    p.next_retry = None;
                    p.unfetched_since = None;
                    p.no_peer_accepted = false;
                });
                rt.resync();
            }
            _ => {}
        }
        self.tick().await;
        self.my_proposal(wallet, &key)
    }

    /// Hands the saved fee to the network again (dash-qt "Resend", QT-091).
    async fn resend_fee(&self, wallet: WalletId, hash: &[u8; 32]) -> Result<(), GovernanceError> {
        let saved = self.saved(wallet, hash)?;
        let handle = self.session.wallet(&wallet).await?;
        handle
            .core()
            .broadcast_transaction(&saved.fee)
            .await
            .map_err(EngineError::from)?;
        let (at, hash) = (unix_now(), saved.row.hash);
        (self.session)
            .appdb_op(move |db| {
                db.mark_proposal_sent(&wallet.to_string(), &hash, at)?;
                db.set_proposal_error(&wallet.to_string(), &hash, None)
            })
            .await?;
        Ok(())
    }

    /// One pass over every wallet's unsettled proposals (MG-06 §2.2):
    /// announce those whose fee a caught-up wallet has seen mined for a
    /// while (D6, D21), withdraw the others, record the first fetch, and
    /// retry by a new run when no peer of a synced run fetched one (D7) or
    /// none announces it six confirmations on (D20).
    pub(super) async fn tick(&self) {
        self.tick_at(Instant::now()).await;
    }

    /// [`Self::tick`] with the clock at `instant` (the tests move it).
    async fn tick_at(&self, instant: Instant) {
        let Ok(rt) = self.runtime() else { return };
        let _one = rt.proposal_tick.lock().await;
        let rows = match self.session.appdb_op(|db| db.proposals(None)).await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!(error = %e, "reading the wallet's proposals");
                return;
            }
        };
        let (phase, last_synced_at) = {
            let p = rt.shared.progress();
            (p.phase, p.last_synced_at)
        };
        let synced = rt.enabled() && phase == Phase::Synced;
        let run = synced.then_some(last_synced_at).flatten();
        let caught_up = self.session.hub.tracker().caught_up();
        let mut live_keys = HashSet::new();
        for row in rows {
            let saved = match Saved::decode(row) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "skipping a proposal row");
                    continue;
                }
            };
            let (key, hash) = (saved.key(), saved.row.hash);
            let f = self.facts(&saved, false);
            if f.public.is_some() || f.expired || f.conflicted {
                rt.shared.withdraw_object(&hash);
                continue;
            }
            live_keys.insert(key);
            let mined = f.depth.is_some_and(|d| d >= MIN_RELAY_CONFIRMATIONS);
            let announce = rt.with_pace(key, |p| {
                p.in_run(run);
                if !mined {
                    // Only a fee seen back in the mempool lost its block;
                    // one missing from history may be a stale read, and
                    // keeps the depth last seen.
                    if f.depth == Some(0) {
                        p.lost |= p.depth > 0 && saved.row.submitted_at.is_some();
                        p.depth = 0;
                    }
                    p.mined_since = None;
                } else if caught_up {
                    p.mined_since.get_or_insert(instant);
                }
                let ready = mined
                    && p.mined_since
                        .is_some_and(|t| instant.duration_since(t) >= PROPAGATION_WAIT);
                if !ready {
                    p.unannounced();
                }
                ready
            });
            if !announce {
                // A Core peer would refuse it and penalise the wallet (D6,
                // D21).
                rt.shared.withdraw_object(&hash);
                continue;
            }
            // Not stored here: the store takes the wallet's own objects as
            // confirmed, and "public" must come from peers (MG-02 D3, D20).
            rt.shared.relay_object(&saved.object);
            let fetched = !rt.shared.object_fetchers(&hash).is_empty();
            if fetched && saved.row.submitted_at.is_none() {
                let wallet = saved.row.wallet_id.clone();
                let marked = (self.session)
                    .appdb_op(move |db| db.mark_proposal_submitted(&wallet, &hash, unix_now()))
                    .await;
                if let Err(e) = marked {
                    tracing::warn!(error = %e, "recording a proposal's submission");
                }
            }
            let depth = f.depth.unwrap_or(0);
            let resync = rt.with_pace(key, |p| {
                let expect_fetch = saved.row.submitted_at.is_none() || p.lost;
                p.depth = depth;
                if fetched {
                    p.lost = false;
                    p.progressed();
                } else if synced && expect_fetch {
                    let since = *p.unfetched_since.get_or_insert(instant);
                    if instant.duration_since(since) >= FETCH_WAIT {
                        p.no_peer_accepted = true;
                        return p.retry_due(instant);
                    }
                }
                // Before six confirmations no peer relays it: waiting is
                // normal there (D19). After, peers that did not hear it
                // from the wallet announce it, on a new connection (D20).
                if depth >= PROPOSAL_FEE_CONFIRMATIONS && synced {
                    let since = *p.six_since.get_or_insert(instant);
                    return instant.duration_since(since) >= ECHO_WAIT && p.retry_due(instant);
                }
                false
            });
            if resync {
                tracing::info!(hash = %display_hex(&hash), "looking for the wallet's proposal with a new governance run");
                rt.resync();
            }
        }
        // A row gone since (its wallet removed) is announced no more.
        for (_, hash) in rt.retain_paces(&live_keys) {
            rt.shared.withdraw_object(&hash);
        }
    }
}

#[cfg(test)]
#[path = "proposals_tests.rs"]
mod tests;

/// The per-session paces of the wallet's own proposals.
#[derive(Debug, Default)]
pub(crate) struct Paces(std::sync::Mutex<HashMap<(WalletId, [u8; 32]), Pace>>);

impl super::sync::GovernanceRuntime {
    fn paces(&self) -> std::sync::MutexGuard<'_, HashMap<(WalletId, [u8; 32]), Pace>> {
        self.paces.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn pace<R>(&self, key: &(WalletId, [u8; 32]), f: impl FnOnce(&Pace) -> R) -> R {
        f(self.paces().get(key).unwrap_or(&Pace::default()))
    }

    fn with_pace<R>(&self, key: (WalletId, [u8; 32]), f: impl FnOnce(&mut Pace) -> R) -> R {
        f(self.paces().entry(key).or_default())
    }

    /// Drops the paces of keys not in `keys`, returning those keys.
    fn retain_paces(&self, keys: &HashSet<(WalletId, [u8; 32])>) -> Vec<(WalletId, [u8; 32])> {
        let mut paces = self.paces();
        let gone: Vec<_> = paces
            .keys()
            .filter(|k| !keys.contains(k))
            .copied()
            .collect();
        for k in &gone {
            paces.remove(k);
        }
        gone
    }
}
