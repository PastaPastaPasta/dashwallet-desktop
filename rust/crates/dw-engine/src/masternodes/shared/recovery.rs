//! A shared session's recovery (MG-10 §4.4 EP 4): the coin test the
//! abandon call and the read both run (change 2), the latest abandon
//! spend's state, and `SharedSession.recovery`, each field computed with
//! the predicate its call runs.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use dashcore::blockdata::script::Instruction;
use dashcore::secp256k1::{Message, ecdsa};
use dashcore::{OutPoint, PublicKey, ScriptBuf, Txid};
use dw_appdb::SharedSessionRow;
use dw_protx::shared::SpecialTx;
use dw_protx::shared::session::Session;
use dw_protx::shared::session::funding::decode_frozen;
use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
use key_wallet::{DerivationPath, Utxo};
use serde::Deserialize;

use super::super::MasternodesError;
use super::super::protx::build::tx_size;
use super::super::protx::ops::{conflict_of, overdue, tip_overdue};
use super::super::protx::require_protx_sending;
use super::super::shared_records::{
    AbandonSpend, AbandonSpendState, Blocked, BlockedReason, HandoffError, HandoffErrorOutcome,
    HandoffTx, RecoveryAction, SettledBy, Settlement, SharedKind, SharedRecovery, SharedRole,
    SharedSession, SharedStage, UnreadableSession,
};
use super::key_change;
use super::key_change::KeyChange;
use super::store::{ChainView, Loaded, reserved_of, session_hash, signed_inputs, unreadable_view};
use crate::coins::{own_address_info, read_coins};
use crate::history::TxEntry;
use crate::send::plan::{dust_threshold, fee_for};
use crate::send::{HandoffOutcome, MIN_FEE_PER_KB, outcome_of_code};
use crate::tx_actions::{ActivityRefusal, TxActionRefusal};
use crate::{EngineError, NetworkSession, WalletId};

/// The length of every change script an abandon spend pays (P2PKH): the
/// coin test's value check uses it, so it issues no address.
pub(super) const CHANGE_SCRIPT_LEN: usize = 25;

/// One entry of a session's `abandon_log`.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct LogEntry {
    pub txid: String,
    #[serde(default)]
    pub fee: Option<u64>,
    #[serde(default)]
    pub inputs: Vec<String>,
    #[serde(default)]
    pub signed_at: u64,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub legacy: bool,
}

impl LogEntry {
    /// Its inputs; `None` when one doesn't parse (the bound then takes the
    /// whole reservation).
    fn inputs(&self) -> Option<Vec<OutPoint>> {
        self.inputs
            .iter()
            .map(|o| OutPoint::from_str(o).ok())
            .collect()
    }
}

/// Every abandon spend `row` signed, oldest first.
pub(super) fn abandon_log(row: &SharedSessionRow) -> Result<Vec<LogEntry>, MasternodesError> {
    serde_json::from_str(row.abandon_log.as_deref().unwrap_or("[]"))
        .map_err(|_| EngineError::Internal("a stored abandon log".into()).into())
}

/// The entry of the latest abandon spend, `abandon_txid`'s.
pub(super) fn latest<'a>(row: &SharedSessionRow, log: &'a [LogEntry]) -> Option<&'a LogEntry> {
    let txid = row.abandon_txid.as_deref()?;
    log.iter().rev().find(|e| e.txid == txid)
}

fn txid_of(text: Option<&str>) -> Option<Txid> {
    text.and_then(|t| Txid::from_str(t).ok())
}

/// A history transaction that may still confirm or be replayed: out of a
/// block and not abandoned.
fn live(entry: &TxEntry) -> bool {
    entry.context.block_info().is_none() && !entry.is_abandoned()
}

fn spends(entry: &TxEntry, coins: &[OutPoint]) -> bool {
    entry
        .tx
        .input
        .iter()
        .any(|i| coins.contains(&i.previous_output))
}

/// The latest abandon spend's state (§4.4 rule 2), first match; `None`
/// with none signed. The caller has settled terminal state first.
pub(super) fn spend_state(
    row: &SharedSessionRow,
    entry: Option<&LogEntry>,
    history: &HashMap<Txid, TxEntry>,
) -> Option<AbandonSpendState> {
    let text = row.abandon_txid.as_deref()?;
    // An id that doesn't parse can't be looked up: it may be out.
    let Some(txid) = txid_of(Some(text)) else {
        return Some(AbandonSpendState::Uncertain);
    };
    if let Some(e) = history
        .get(&txid)
        .filter(|e| e.context.block_info().is_none())
    {
        return Some(match e.is_abandoned() {
            true => AbandonSpendState::AbandonedInActivity,
            false => AbandonSpendState::InHistory,
        });
    }
    if entry.is_some_and(|e| e.outcome.as_deref() == Some("not_sent")) {
        return Some(AbandonSpendState::Refused);
    }
    Some(match row.abandon_tx.is_some() {
        true => AbandonSpendState::Uncertain,
        false => AbandonSpendState::Legacy,
    })
}

/// Whether the latest spend back may still be on its way (`Uncertain` or
/// `InHistory`): no second spend of the same coins while it is.
pub(super) fn spend_back_pending(
    row: &SharedSessionRow,
    history: &HashMap<Txid, TxEntry>,
) -> Result<bool, MasternodesError> {
    if row.abandon_txid.is_none() {
        return Ok(false);
    }
    let log = abandon_log(row)?;
    Ok(matches!(
        spend_state(row, latest(row, &log), history),
        Some(AbandonSpendState::Uncertain | AbandonSpendState::InHistory)
    ))
}

/// What the coin test says a new abandon spend may take (§4.4 change 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AbandonCoins {
    /// A pending spend holds a reserved coin; it ends at restart.
    HeldInFlight,
    /// The wallet can't sign for a reserved coin it holds.
    NoOwnershipPath { outpoint: OutPoint },
    /// No reserved coin is held (none of the session's own signed
    /// transaction's inputs, with its bytes saved), and this unconfirmed,
    /// not abandoned history transaction spends one.
    BlockedByWalletTx { txid: Txid },
    /// No such coin is held, and no such transaction is known.
    Unavailable,
    /// The held coins don't include every outpoint a new spend must take
    /// to conflict with every earlier abandon spend.
    Unbounded {
        missing: Vec<OutPoint>,
        spender: Option<Txid>,
    },
    /// The held coins pay no more than the fee and dust.
    TooSmall { fee: u64, available: u64 },
    /// Every held reserved coin, outpoint order, with its ownership path,
    /// and the relay minimum of spending them all to one P2PKH output.
    Usable {
        coins: Vec<(Utxo, DerivationPath)>,
        fee: u64,
    },
}

/// What the coin test reads of a session, gathered once.
pub(super) struct CoinFacts {
    pub reserved: Vec<OutPoint>,
    /// The reserved coins the wallet holds unspent, outpoint order.
    pub held: Vec<(Utxo, Option<DerivationPath>)>,
    pub pending: HashSet<OutPoint>,
    /// The session's own transactions, in lookup order.
    pub own: Vec<Txid>,
    /// With an abandon spend signed: its id (when it parses) and the
    /// outpoints a new one must take.
    pub latest: Option<(Option<Txid>, Vec<OutPoint>)>,
    /// The inputs of the session's own transaction whose signed bytes the
    /// row saved: a new spend conflicts with it only by taking one.
    pub signed: Option<Vec<OutPoint>>,
}

impl CoinFacts {
    fn held_outpoints(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.held.iter().map(|(u, _)| u.outpoint)
    }

    /// The outpoints a new spend must take: every reserved one while none
    /// was signed (a lookup set only) or the legacy mark is set, else the
    /// latest spend's inputs.
    fn required(&self) -> &[OutPoint] {
        self.latest
            .as_ref()
            .map_or(&self.reserved, |(_, required)| required)
    }

    /// The required outpoints the wallet doesn't hold.
    fn missing(&self) -> Vec<OutPoint> {
        let held: HashSet<OutPoint> = self.held_outpoints().collect();
        self.required()
            .iter()
            .filter(|o| !held.contains(o))
            .copied()
            .collect()
    }

    /// The coin test, first match.
    pub fn test(&self, history: &HashMap<Txid, TxEntry>) -> AbandonCoins {
        if self.held_outpoints().any(|o| self.pending.contains(&o)) {
            return AbandonCoins::HeldInFlight;
        }
        if let Some((u, _)) = self.held.iter().find(|(_, path)| path.is_none()) {
            return AbandonCoins::NoOwnershipPath {
                outpoint: u.outpoint,
            };
        }
        let conflicts = (self.signed.as_ref())
            .is_none_or(|inputs| self.held_outpoints().any(|o| inputs.contains(&o)));
        if self.held.is_empty() || !conflicts {
            return match self.blocker(history) {
                Some(txid) => AbandonCoins::BlockedByWalletTx { txid },
                None => AbandonCoins::Unavailable,
            };
        }
        if self.latest.is_some() {
            let missing = self.missing();
            if !missing.is_empty() {
                let spender = first_spender(history, &missing);
                return AbandonCoins::Unbounded { missing, spender };
            }
        }
        let available: u64 = self.held.iter().map(|(u, _)| u.value()).sum();
        let fee = spend_fee(self.held.len());
        if available
            .checked_sub(fee)
            .is_none_or(|v| v <= dust_threshold(CHANGE_SCRIPT_LEN))
        {
            return AbandonCoins::TooSmall { fee, available };
        }
        AbandonCoins::Usable {
            coins: self
                .held
                .iter()
                .filter_map(|(u, p)| Some((u.clone(), p.clone()?)))
                .collect(),
            fee,
        }
    }

    /// A live history transaction spending a reserved coin: the session's
    /// own, then the latest abandon spend, then the first by outpoint.
    fn blocker(&self, history: &HashMap<Txid, TxEntry>) -> Option<Txid> {
        self.own
            .iter()
            .chain(self.latest.as_ref().and_then(|(a, _)| a.as_ref()))
            .find(|id| {
                history
                    .get(*id)
                    .is_some_and(|e| live(e) && spends(e, &self.reserved))
            })
            .copied()
            .or_else(|| first_spender(history, &self.reserved))
    }
}

/// The lowest live history transaction spending the first of `coins`, by
/// outpoint order, that one spends.
fn first_spender(history: &HashMap<Txid, TxEntry>, coins: &[OutPoint]) -> Option<Txid> {
    let mut sorted = coins.to_vec();
    sorted.sort();
    sorted.iter().find_map(|o| {
        history
            .iter()
            .filter(|(_, e)| live(e) && spends(e, std::slice::from_ref(o)))
            .map(|(id, _)| *id)
            .min()
    })
}

/// The relay minimum of an abandon spend of `inputs` coins.
pub(super) fn spend_fee(inputs: usize) -> u64 {
    fee_for(
        FeeRate::new(MIN_FEE_PER_KB),
        tx_size(inputs, &[CHANGE_SCRIPT_LEN], 0),
    )
    .unwrap_or(u64::MAX)
}

/// The session's own transactions, in the coin test's lookup order: its
/// registration in the wallet's history (a participant's signed copy has
/// another txid), then its stored txid.
fn own_txids(row: &SharedSessionRow, chain: &ChainView, network: dashcore::Network) -> Vec<Txid> {
    let mut own = Vec::new();
    if row.kind != key_change::KIND
        && let Ok(l) = Loaded::of(row.clone(), network)
        && let Some(hash) = session_hash(&l, chain)
    {
        own.push(hash);
    }
    if let Some(txid) = txid_of(row.txid.as_deref())
        && !own.contains(&txid)
    {
        own.push(txid);
    }
    own
}

/// The reading state the recovery takes from the session once per read.
pub(super) struct ReadContext {
    spv_running: bool,
    peers: u32,
    tip: Option<u32>,
    chainlock: Option<u32>,
    tip_time: Option<u64>,
    list_synced: bool,
    list_height: Option<u32>,
    buried: Box<dyn Fn(u32) -> bool + Send + Sync>,
    can_send: bool,
}

impl NetworkSession {
    /// [`unreadable_view`] with the coin test `abandon_unreadable_shared_session`
    /// spends by: `abandonable` only while it passes, charged its fee. A coin
    /// read that fails lists the row not abandonable.
    pub(super) async fn unreadable_listing(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
        chain: &ChainView,
    ) -> UnreadableSession {
        let mut view = unreadable_view(row, chain);
        if view.abandonable && spend_back_pending(row, &chain.history).unwrap_or(true) {
            view.abandonable = false;
        }
        if view.abandonable {
            view.abandon_charge = match self.coin_facts(wallet, row, chain).await {
                Ok(facts) => match facts.test(&chain.history) {
                    AbandonCoins::Usable { fee, .. } => Some(fee),
                    _ => None,
                },
                Err(_) => None,
            };
        }
        view.abandonable = view.abandon_charge.is_some();
        view
    }

    /// [`CoinFacts`] of `row` as the wallet holds its coins now.
    pub(super) async fn coin_facts(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
        chain: &ChainView,
    ) -> Result<CoinFacts, MasternodesError> {
        let reserved = reserved_of(row);
        let w = self.wallet(&wallet).await?;
        let mut held: Vec<(Utxo, Option<DerivationPath>)> = {
            let state = crate::wallet_read::info_of(&w).await?;
            read_coins(&state.core_wallet)
                .coins
                .into_iter()
                .filter(|c| reserved.contains(&c.utxo.outpoint))
                .map(|c| {
                    let path =
                        own_address_info(&state.core_wallet, &c.utxo.address).map(|i| i.path);
                    (c.utxo, path)
                })
                .collect()
        };
        held.sort_by_key(|(u, _)| u.outpoint);
        let log = abandon_log(row)?;
        let latest = row.abandon_txid.as_deref().map(|a| {
            let inputs = latest(row, &log)
                .and_then(LogEntry::inputs)
                .filter(|i| !i.is_empty() && !row.legacy_bound);
            (txid_of(Some(a)), inputs.unwrap_or_else(|| reserved.clone()))
        });
        Ok(CoinFacts {
            reserved,
            held,
            pending: self.spends.snapshot(&wallet),
            own: own_txids(row, chain, self.network.core_network()),
            latest,
            signed: signed_inputs(row),
        })
    }

    /// The latest abandon spend's fee: its log entry's, else what its saved
    /// bytes pay when every input is a reserved coin the wallet values (one
    /// it holds, or an own input of the spend's history entry), less their
    /// one output. `None` when neither shows it.
    pub(super) async fn abandon_fee(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
        entry: Option<&LogEntry>,
        history: &HashMap<Txid, TxEntry>,
    ) -> Result<Option<u64>, MasternodesError> {
        if let Some(fee) = entry.and_then(|e| e.fee) {
            return Ok(Some(fee));
        }
        let Some(tx) = (row.abandon_tx.as_deref())
            .and_then(|b| dashcore::consensus::deserialize::<dashcore::Transaction>(b).ok())
        else {
            return Ok(None);
        };
        let ([output], reserved) = (tx.output.as_slice(), reserved_of(row)) else {
            return Ok(None);
        };
        let recorded = history.get(&tx.txid());
        let w = self.wallet(&wallet).await?;
        let state = crate::wallet_read::info_of(&w).await?;
        let coins = read_coins(&state.core_wallet).coins;
        let mut total = 0u64;
        for (index, input) in tx.input.iter().enumerate() {
            let o = input.previous_output;
            let value = (coins.iter().find(|c| c.utxo.outpoint == o))
                .map(|c| c.utxo.value())
                .or_else(|| {
                    let own = recorded?.own_inputs.get(&u32::try_from(index).ok()?)?;
                    Some(own.0)
                });
            match value {
                Some(v) if reserved.contains(&o) => total = total.saturating_add(v),
                _ => return Ok(None),
            }
        }
        Ok(total.checked_sub(output.value))
    }

    /// `shared_stage{stage: "abandoning"}` while the latest abandon spend of
    /// `row` may be out: recorded unconfirmed, or of unknown outcome. Only a
    /// copy of the same bytes may go then (MG-10 §4.4 change 4).
    pub(super) async fn check_not_abandoning(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
    ) -> Result<(), MasternodesError> {
        if row.abandon_txid.is_none() {
            return Ok(());
        }
        let chain = self.shared_chain(wallet).await?;
        match spend_back_pending(row, &chain.history)? {
            true => Err(MasternodesError::SharedStage {
                stage: "abandoning".into(),
            }),
            false => Ok(()),
        }
    }

    /// The transaction of a copy `l`'s import refused, when it carries this
    /// wallet's valid signature over one of `l`'s own inputs and `l` isn't
    /// out yet: those bytes, or a later copy of them, may reach the network
    /// (the wallet on another install, or a database restored from an
    /// older backup). A transaction spending an outpoint twice is never
    /// valid, so it is no such copy; each own input's signature is checked
    /// once, before the wallet is read.
    pub(super) async fn own_signed_copy(
        &self,
        wallet: WalletId,
        l: &Loaded,
        theirs: &Session,
    ) -> Result<Option<SpecialTx>, MasternodesError> {
        let own = l.own_inputs();
        if l.out() || own.is_empty() {
            return Ok(None);
        }
        let Ok(frozen) = decode_frozen(theirs) else {
            return Ok(None);
        };
        let tx = frozen.tx;
        let mut spent = HashSet::new();
        if !tx.input.iter().all(|i| spent.insert(i.previous_output)) {
            return Ok(None);
        }
        let keys: Vec<PublicKey> = (tx.input.iter().enumerate())
            .filter(|(_, input)| own.contains(&input.previous_output))
            .filter_map(|(index, _)| signer_of(&tx, index))
            .collect();
        if keys.is_empty() {
            return Ok(None);
        }
        let network = self.network.core_network();
        let w = self.wallet(&wallet).await?;
        let state = crate::wallet_read::info_of(&w).await?;
        let signed = keys.iter().any(|key| {
            let address = dashcore::Address::p2pkh(key, network);
            own_address_info(&state.core_wallet, &address).is_some()
        });
        Ok(signed.then_some(tx))
    }

    /// Marks `l` out with `tx`, its outcome unknown (MG-10 §4.4's uncertain
    /// rules): the stage reads out, no discard is accepted, and the abandon
    /// is the exit. Its reservation and locks stay.
    pub(super) async fn mark_possibly_out(
        &self,
        wallet: WalletId,
        l: &Loaded,
        tx: &SpecialTx,
    ) -> Result<(), MasternodesError> {
        let (w, id) = (wallet.to_string(), l.row.id.clone());
        let (bytes, txid) = (tx.to_bytes(), tx.txid().to_string());
        self.appdb_op(move |db| {
            if db.mark_shared_session_out_durable(
                &w,
                &id,
                &bytes,
                &txid,
                crate::coins::now_secs(),
            )? {
                db.record_shared_session_attempt(&w, &id, Some(UNKNOWN_CODE))?;
            }
            Ok(())
        })
        .await?;
        Ok(())
    }

    /// The SPV client's state as Activity's calls read it: running, and
    /// connected peers.
    fn spv_status(&self) -> (bool, u32) {
        #[cfg(test)]
        if let Some(status) = self
            .masternodes
            .protx_seam()
            .and_then(|s| *s.spv.lock().unwrap())
        {
            return status;
        }
        let running = self.manager().is_ok_and(|m| m.spv().is_started());
        (running, self.hub.tracker().snapshot().connected_peers)
    }

    fn read_context(&self, can_send: bool) -> ReadContext {
        let (spv_running, peers) = self.spv_status();
        // `buried` takes the tracker's lock itself: read it first.
        let buried = Box::new(self.buried());
        let t = self.hub.tracker();
        ReadContext {
            spv_running,
            peers,
            tip: t.tip_height(),
            chainlock: t.chainlock_height(),
            tip_time: t.current_tip_time(),
            list_synced: t.masternodes_synced(),
            list_height: t.masternode_list_height(),
            buried,
            can_send,
        }
    }

    /// Sets `recovery` on each of `sessions` of `wallet` from its stored
    /// row. `can_send`: the facade's sending switch, without which no call
    /// that hands bytes off is listed. A recovery that can't be read stays
    /// empty (no action listed), so a call that already acted still
    /// returns its session.
    pub(super) async fn fill_recovery(
        &self,
        wallet: WalletId,
        sessions: &mut [SharedSession],
        can_send: bool,
    ) {
        if sessions.is_empty() {
            return;
        }
        let w = wallet.to_string();
        let (Ok(rows), Ok(chain)) = (
            self.appdb_op(move |db| db.shared_sessions(&w)).await,
            self.shared_chain(wallet).await,
        ) else {
            return;
        };
        let rows: HashMap<String, SharedSessionRow> =
            rows.into_iter().map(|r| (r.id.clone(), r)).collect();
        let ctx = self.read_context(can_send);
        for session in sessions {
            if let Some(row) = rows.get(&session.id)
                && let Ok(r) = self.recovery_of(wallet, row, session, &chain, &ctx).await
            {
                session.recovery = r;
            }
        }
    }

    /// One session's recovery (§4.4 rules 1–11): terminal state first, then
    /// the actions its calls accept, then why no exit is listed.
    async fn recovery_of(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
        session: &SharedSession,
        chain: &ChainView,
        ctx: &ReadContext,
    ) -> Result<SharedRecovery, MasternodesError> {
        let network = self.network.core_network();
        let log = abandon_log(row)?;
        let entry = latest(row, &log);
        let attributed: Vec<Txid> = log.iter().filter_map(|e| txid_of(Some(&e.txid))).collect();
        let mut r = SharedRecovery {
            out_at: row.out_at,
            attributed: attributed.iter().map(Txid::to_string).collect(),
            ..Default::default()
        };
        if session.kind == SharedKind::DissolveTogether {
            if self.together_discard(wallet, row.clone()).await.is_ok() {
                r.actions.push(RecoveryAction::Discard {
                    releases: self.reserved_total(wallet, row).await?,
                });
            }
            return Ok(r);
        }
        let key_change = session.kind == SharedKind::KeyChange;
        let (own_inputs, except) = match key_change {
            true => (
                KeyChange::of(row.clone(), network)
                    .map(|k| k.own_inputs())
                    .unwrap_or_else(|_| reserved_of(row)),
                txid_of(row.txid.as_deref()),
            ),
            false => (
                Loaded::of(row.clone(), network)
                    .map(|l| l.conflict_inputs())
                    .unwrap_or_else(|_| reserved_of(row)),
                txid_of(session.pro_tx_hash.as_deref()).or(txid_of(row.txid.as_deref())),
            ),
        };
        let confirmed = matches!(session.stage, SharedStage::Confirmed { .. });
        let conflict = (row.out_at.is_some() && !confirmed)
            .then(|| conflict_of(&chain.history, &own_inputs, except.as_ref(), &attributed))
            .flatten();
        if let Some((by, height)) = conflict {
            let by = match attributed.contains(&by) {
                true => SettledBy::AbandonTx,
                false => SettledBy::OtherSpend,
            };
            r.settlement = match (ctx.buried)(height) {
                true => Settlement::Settled {
                    by,
                    released: reserved_of(row).is_empty(),
                },
                false => Settlement::Confirming {
                    by,
                    confirmations: ctx
                        .tip
                        .map_or(1, |t| t.saturating_sub(height).saturating_add(1))
                        .max(1),
                    chainlocked: ctx.chainlock.is_some(),
                },
            };
        } else if let (Some(txid), false) = (&row.abandon_txid, confirmed) {
            r.settlement = Settlement::Pending { txid: txid.clone() };
        }
        let terminal = confirmed
            || conflict.is_some()
            || (key_change && matches!(session.stage, SharedStage::Dead { .. }));
        let pending = matches!(r.settlement, Settlement::Pending { .. });
        let state = (pending && !terminal)
            .then(|| spend_state(row, entry, &chain.history))
            .flatten();
        if let Some(txid) = &row.abandon_txid {
            let signed_at = entry.map_or(0, |e| e.signed_at);
            r.abandon_spend = Some(AbandonSpend {
                txid: txid.clone(),
                signed_at,
                legacy: entry.is_some_and(|e| e.legacy),
                overdue: tip_overdue(signed_at, ctx.tip_time),
                state,
            });
        }
        let discard = row.out_at.is_none();
        let funder = !key_change || session.key_change.as_ref().is_some_and(|k| k.funder);
        if terminal || discard {
            // Before `out_at` a funder's key change is released, not spent
            // (MG-04b D102).
            if !terminal && key_change && funder && ctx.can_send {
                r.actions.push(RecoveryAction::Abandon {
                    charge: 0,
                    spends: false,
                    replaces: None,
                });
            }
            if discard {
                r.actions.push(RecoveryAction::Discard {
                    releases: self.reserved_total(wallet, row).await?,
                });
            }
            return Ok(r);
        }
        let out_at = row.out_at.unwrap_or(0);
        r.overdue = overdue(
            out_at,
            ctx.list_synced,
            ctx.list_height,
            ctx.tip,
            ctx.tip_time,
        );
        r.handoff_error = handoff_error(row, entry);
        if !funder {
            return Ok(r);
        }
        let facts = self.coin_facts(wallet, row, chain).await?;
        let coins = facts.test(&chain.history);
        let a = txid_of(row.abandon_txid.as_deref());
        let signed = row.abandon_txid.is_some();
        let replaceable = matches!(
            state,
            Some(AbandonSpendState::Refused | AbandonSpendState::AbandonedInActivity)
        );
        let new_spend = match (signed, state) {
            (false, _) => true,
            (true, Some(AbandonSpendState::Legacy)) => {
                r.abandon_spend.as_ref().is_some_and(|s| s.overdue)
            }
            _ => replaceable,
        };
        let mut spv_held = false;
        let mut no_peers = false;
        let mut locked: Option<Txid> = None;

        if ctx.can_send
            && session.stage == SharedStage::Sent
            && !signed
            && self.resend_passes(wallet, row, session).await
        {
            match ctx.spv_running {
                true => r.actions.push(RecoveryAction::Resend {
                    charge: match key_change {
                        true => session.fee.unwrap_or(0),
                        false => session.debit.unwrap_or(0),
                    },
                }),
                false => spv_held = true,
            }
        }
        if let (true, true, AbandonCoins::Usable { fee, .. }) = (ctx.can_send, new_spend, &coins) {
            r.actions.push(RecoveryAction::Abandon {
                charge: *fee,
                spends: true,
                replaces: row.abandon_txid.clone(),
            });
        }
        let replayable = match state {
            Some(AbandonSpendState::Uncertain) => true,
            Some(AbandonSpendState::InHistory) => row.abandon_tx.is_some(),
            _ => false,
        };
        let charge = match ctx.can_send && replayable {
            true => self.abandon_fee(wallet, row, entry, &chain.history).await?,
            false => None,
        };
        if let Some(charge) = charge
            && self.abandon_replay_passes(wallet, row)
        {
            match ctx.spv_running {
                true => r.actions.push(RecoveryAction::ResendAbandon { charge }),
                false => spv_held = true,
            }
        }
        let own_blocker = match &coins {
            AbandonCoins::BlockedByWalletTx { txid }
                if facts.own.contains(txid) && (!signed || replaceable) =>
            {
                Some(*txid)
            }
            _ => None,
        };
        let a_recorded = a.filter(|_| state == Some(AbandonSpendState::InHistory));
        for txid in own_blocker.into_iter().chain(a_recorded) {
            let Some(exits) = self
                .activity_exits(wallet, txid, ctx.spv_running, ctx.peers)
                .await?
            else {
                continue;
            };
            match exits.abandon {
                None => r.actions.push(RecoveryAction::AbandonWalletTx {
                    txid: txid.to_string(),
                }),
                Some(ActivityRefusal::SpvNotRunning) => spv_held = true,
                Some(ActivityRefusal::Tx(TxActionRefusal::InstantLocked)) => {
                    locked.get_or_insert(txid);
                }
                Some(_) => {}
            }
            if Some(txid) == a_recorded && row.abandon_tx.is_none() {
                match exits.resend {
                    None => r.actions.push(RecoveryAction::ResendWalletTx {
                        txid: txid.to_string(),
                    }),
                    Some(ActivityRefusal::SpvNotRunning) => spv_held = true,
                    Some(ActivityRefusal::NoPeers) => no_peers = true,
                    Some(_) => {}
                }
            }
        }
        let exit = r.actions.iter().any(|x| {
            matches!(
                x,
                RecoveryAction::Abandon { .. } | RecoveryAction::AbandonWalletTx { .. }
            )
        });
        if !exit {
            let replay = r
                .actions
                .iter()
                .any(|x| matches!(x, RecoveryAction::ResendAbandon { .. }));
            r.blocked = blocked(
                &facts,
                &coins,
                &chain.history,
                &attributed,
                a,
                new_spend,
                spv_held || (no_peers && !replay),
                locked,
            );
        }
        Ok(r)
    }

    /// Whether the session's own resend passes its checks before the lease,
    /// as `send_shared_registration` and `send_maintenance` run them at
    /// `sent`: never on mainnet (#438).
    async fn resend_passes(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
        session: &SharedSession,
    ) -> bool {
        if require_protx_sending(self).is_err() || super::v24_context(self, wallet).await.is_err() {
            return false;
        }
        match session.kind {
            SharedKind::KeyChange => {
                match KeyChange::of(row.clone(), self.network.core_network()) {
                    Ok(k) if k.funded.is_some() => {
                        self.key_change_resend_checks(wallet, &k).await.is_ok()
                    }
                    _ => false,
                }
            }
            _ => {
                session.role == SharedRole::Coordinator
                    && self.registration_resend_checks(wallet, row).is_ok()
            }
        }
    }

    /// Whether `resend_shared_abandon`'s inputs check passes on the saved
    /// spend.
    fn abandon_replay_passes(&self, wallet: WalletId, row: &SharedSessionRow) -> bool {
        row.abandon_tx
            .as_deref()
            .and_then(|b| dashcore::consensus::deserialize::<dashcore::Transaction>(b).ok())
            .is_some_and(|tx| self.check_resend_inputs(wallet, &tx).is_ok())
    }

    /// What a discard of `row` releases: the reserved coins the wallet holds.
    async fn reserved_total(
        &self,
        wallet: WalletId,
        row: &SharedSessionRow,
    ) -> Result<u64, MasternodesError> {
        let reserved = reserved_of(row);
        if reserved.is_empty() {
            return Ok(0);
        }
        let w = self.wallet(&wallet).await?;
        let state = crate::wallet_read::info_of(&w).await?;
        Ok(read_coins(&state.core_wallet)
            .coins
            .iter()
            .filter(|c| reserved.contains(&c.utxo.outpoint))
            .map(|c| c.utxo.value())
            .sum())
    }
}

/// The code a hand-off of unknown outcome records.
const UNKNOWN_CODE: &str = "send.broadcast_unknown";

/// The transaction of each [`signer_of`] run, in tests.
#[cfg(test)]
pub(super) static SIGNER_RUNS: std::sync::Mutex<Vec<Txid>> = std::sync::Mutex::new(Vec::new());

/// The key whose valid `SIGHASH_ALL` P2PKH signature over `tx` input
/// `index` its scriptSig carries (a high-S one counts: it could be made
/// standard).
fn signer_of(tx: &SpecialTx, index: usize) -> Option<PublicKey> {
    #[cfg(test)]
    SIGNER_RUNS.lock().unwrap().push(tx.txid());
    let mut pushes = tx.input.get(index)?.script_sig.instructions();
    let (Some(Ok(Instruction::PushBytes(sig))), Some(Ok(Instruction::PushBytes(key))), None) =
        (pushes.next(), pushes.next(), pushes.next())
    else {
        return None;
    };
    let (&hash_type, der) = sig.as_bytes().split_last()?;
    if hash_type != 0x01 {
        return None;
    }
    let mut signature = ecdsa::Signature::from_der(der).ok()?;
    signature.normalize_s();
    let key = PublicKey::from_slice(key.as_bytes()).ok()?;
    let hash = tx.signature_hash_all(index, &ScriptBuf::new_p2pkh(&key.pubkey_hash()))?;
    ecdsa::verify(&signature, Message::from_digest(hash), &key.inner).ok()?;
    Some(key)
}

/// §4.4 rule 3, on an out session not settled.
fn handoff_error(row: &SharedSessionRow, entry: Option<&LogEntry>) -> Option<HandoffError> {
    if row.abandon_txid.is_some() {
        let e = entry?;
        let code = e.code.clone()?;
        return Some(HandoffError {
            tx: HandoffTx::Abandon,
            outcome: match e.outcome.as_deref() {
                Some("not_sent") => HandoffErrorOutcome::Refused,
                _ => HandoffErrorOutcome::Unknown,
            },
            code: Some(code),
            resend_refused: false,
        });
    }
    let code = row.last_error.clone()?;
    let refused = outcome_of_code(&code) == HandoffOutcome::Refused;
    Some(HandoffError {
        tx: HandoffTx::Registration,
        outcome: match (refused, row.resend_marked) {
            (true, false) => HandoffErrorOutcome::Refused,
            _ => HandoffErrorOutcome::Unknown,
        },
        code: Some(code),
        resend_refused: refused && row.resend_marked,
    })
}

/// §4.4 rule 9, first match, when no abandon exit is listed. `new_spend`:
/// a new abandon spend is what the latest spend's state allows.
#[expect(clippy::too_many_arguments, reason = "one rule over the read's facts")]
fn blocked(
    facts: &CoinFacts,
    coins: &AbandonCoins,
    history: &HashMap<Txid, TxEntry>,
    attributed: &[Txid],
    a: Option<Txid>,
    new_spend: bool,
    spv_stopped: bool,
    locked: Option<Txid>,
) -> Option<Blocked> {
    let of = |txid: Option<Txid>, reason| {
        Some(Blocked {
            txid: txid.map(|t| t.to_string()),
            reason,
        })
    };
    if spv_stopped {
        return of(None, BlockedReason::SpvStopped);
    }
    if locked.is_some() {
        return of(locked, BlockedReason::InstantLocked);
    }
    if !new_spend {
        return None;
    }
    match coins {
        AbandonCoins::HeldInFlight => return of(None, BlockedReason::InFlight),
        AbandonCoins::NoOwnershipPath { .. } => return of(None, BlockedReason::NoOwnershipPath),
        _ => {}
    }
    if let Some(txid) = rescanning(facts, coins, history, attributed, a) {
        return of(Some(txid), BlockedReason::Rescanning);
    }
    match coins {
        AbandonCoins::Unbounded { spender, .. } if a.is_some() => {
            of(*spender, BlockedReason::CoinElsewhere)
        }
        AbandonCoins::BlockedByWalletTx { txid }
            if a.is_some() && !facts.own.contains(txid) && Some(*txid) != a =>
        {
            of(Some(*txid), BlockedReason::CoinElsewhere)
        }
        AbandonCoins::TooSmall { .. } => of(None, BlockedReason::TooSmall),
        _ => None,
    }
}

/// The transaction abandoned in Activity whose coins the rescan hasn't
/// returned yet (§4.4 rule 9): no reserved coin claimed, and either nothing
/// held while the session's own transaction or the latest spend is marked
/// abandoned, or every missing required coin an input of such a
/// transaction (the session's own, the latest spend, or any it signed).
fn rescanning(
    facts: &CoinFacts,
    coins: &AbandonCoins,
    history: &HashMap<Txid, TxEntry>,
    attributed: &[Txid],
    a: Option<Txid>,
) -> Option<Txid> {
    if facts.reserved.iter().any(|o| facts.pending.contains(o)) {
        return None;
    }
    let abandoned = |id: &&Txid| history.get(*id).is_some_and(TxEntry::is_abandoned);
    match coins {
        AbandonCoins::Unavailable => facts.own.iter().chain(a.as_ref()).find(abandoned).copied(),
        AbandonCoins::Unbounded { .. } | AbandonCoins::BlockedByWalletTx { .. } => {
            let candidates: Vec<&Txid> = facts
                .own
                .iter()
                .chain(a.as_ref())
                .chain(attributed)
                .filter(abandoned)
                .collect();
            let missing = facts.missing();
            let spender = |o: &OutPoint| {
                candidates
                    .iter()
                    .find(|id| {
                        history
                            .get(**id)
                            .is_some_and(|e| spends(e, std::slice::from_ref(o)))
                    })
                    .map(|id| **id)
            };
            let found: Vec<Txid> = missing.iter().map_while(spender).collect();
            (!missing.is_empty() && found.len() == missing.len()).then(|| found[0])
        }
        _ => None,
    }
}

/// Whether `stage` settles a session for the recovery calls: the
/// registration's `confirmed` or `conflicted`, a key change's `dead` too.
pub(super) fn settled_stage(stage: SharedStage, key_change: bool) -> bool {
    matches!(
        stage,
        SharedStage::Confirmed { .. } | SharedStage::Conflicted
    ) || (key_change && matches!(stage, SharedStage::Dead { .. }))
}

#[cfg(test)]
mod tests;
