//! Transaction actions and exports (QT-031…033, QT-075, QT-091…093,
//! IOS-031, IOS-034; docs/contracts/m2-engine.md §2.2).
//!
//! Abandon goes through key-wallet's `abandon_transaction` (the transaction
//! and every recorded descendant leave the wallet; the coins they spent are
//! released from the spent set) and is persisted as a sweep batch whose
//! inputs are all released, so wallet.sqlite drops the rows and a restart
//! neither re-credits the change nor re-sends the transaction. The app
//! database keeps the raw transaction so the history still shows it as
//! `Abandoned`. Key-wallet rediscovers the released coins through a filter
//! rescan, which abandon schedules from the lowest funding height.
//!
//! SPV sees no mempool: `in_mempool` is always `None` and abandon is allowed
//! without proof; the host warns that the transaction may still confirm
//! (DESIGN-opus §1.14). If it does, the history shows it confirmed again.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::consensus::encode::{deserialize, serialize_hex};
use dashcore::{OutPoint, Transaction, Txid};
use dw_appdb::{AppDb, LockReason};
use dw_units::{Chain, Unit};
use platform_wallet::changeset::changeset::SweepBatch;
use platform_wallet::changeset::{
    CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
};

use crate::csv_export::{TX_TYPE_COUNT, write_csv};
use crate::history::{
    ChainView, HistoryFilter, HistorySort, Labels, TxEntry, TxType, amounts, depth_of, filtered,
    records_of, status_of,
};
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

/// Why `abandon_transaction` / `resend_transaction` refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxActionRefusal {
    Confirmed,
    InstantLocked,
    AlreadyAbandoned,
    Coinbase,
    InMempool,
    NotSentByWallet,
}

/// The dash-qt details fields `TxDetail` lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxDetailExtras {
    pub txid: String,
    pub is_coinbase: bool,
    pub total_credit: u64,
    pub total_debit: Option<u64>,
    pub net: i64,
    pub matures_in: Option<u32>,
    pub in_mempool: Option<bool>,
    pub abandoned: bool,
    pub can_abandon: bool,
    pub can_resend: bool,
    pub dust_locked_outputs: Vec<OutPoint>,
    pub last_announced_at: Option<u64>,
}

/// One notification row (one per dash-qt record).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxNotice {
    pub txid: String,
    pub record_index: u32,
    pub amount: i64,
    pub timestamp: Option<u64>,
    pub tx_type: TxType,
    pub address: Option<String>,
    pub label: Option<String>,
    pub coinjoin_internal: bool,
}

/// How long `resend_transaction` waits for dash-spv to refuse the send
/// before it reports the transaction as handed over.
const RESEND_HANDOFF: std::time::Duration = std::time::Duration::from_secs(2);

/// Settings key prefix (wallet scope) of an abandoned transaction; the
/// value is its consensus hex.
const ABANDONED_PREFIX: &str = "abandoned:";

/// Settings key prefix (wallet scope) of a payment this app signed and
/// handed to the network (#88): the evidence a spend claim is restored
/// from at load. Written before the first broadcast, removed when it was
/// never sent, and dropped at load once the payment is in a block,
/// abandoned or gone from the store. Its value names the wallet's coins it
/// signed.
pub(crate) const SENT_PREFIX: &str = "sent:";

/// Settings key prefix (wallet scope) of the signed transaction of a sent
/// mark, its consensus hex, written and dropped with the mark (#170 case
/// 1). Apart from the mark, so a build that knows no such key still reads
/// the mark; one an earlier build wrote has none.
pub(crate) const SENT_TX_PREFIX: &str = "senttx:";

/// Settings key prefix of a sent mark's inputs: each signed coin's
/// outpoint, value and address, so it is listed when its funding is not.
pub(crate) const SENT_IN_PREFIX: &str = "sentin:";

/// The marks of `wallet` under `prefix` that `parse` reads from their txid
/// and value, and whether every one was read (each failure is logged).
fn load_marks<T>(
    appdb: &AppDb,
    wallet: WalletId,
    prefix: &str,
    parse: impl Fn(Txid, String) -> Option<T>,
) -> (Vec<T>, bool) {
    let rows = match appdb.settings_with_prefix(&wallet.to_string(), prefix) {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(%wallet, prefix, error = %e, "could not read the marks");
            return (Vec::new(), false);
        }
    };
    let mut read = true;
    let marks = (rows.into_iter())
        .filter_map(|(key, value)| {
            let mark = (Txid::from_str(&key[prefix.len()..]).ok()).and_then(|t| parse(t, value));
            if mark.is_none() {
                tracing::warn!(%wallet, %key, "unreadable mark");
                read = false;
            }
            mark
        })
        .collect();
    (marks, read)
}

/// The payments of `wallet` this app sent, each with the wallet's coins it
/// signed and its transaction when the mark holds it ([`SENT_PREFIX`]),
/// and whether every mark was read.
pub(crate) fn load_sent(appdb: &AppDb, wallet: WalletId) -> (Sent, bool) {
    let scope = wallet.to_string();
    let rows = |prefix: &str| match appdb.settings_with_prefix_lossy(&scope, prefix) {
        Ok(rows) => Some(
            (rows.into_iter())
                .filter_map(|(key, value)| {
                    Some((Txid::from_str(key.get(prefix.len()..)?).ok()?, value))
                })
                .collect::<BTreeMap<_, _>>(),
        ),
        Err(e) => {
            tracing::warn!(%wallet, prefix, error = %e, "could not read the sent marks");
            None
        }
    };
    let (Some(marks), Some(txs)) = (rows(SENT_PREFIX), rows(SENT_TX_PREFIX)) else {
        return (Sent::new(), false);
    };
    let mut inputs = rows(SENT_IN_PREFIX).unwrap_or_default();
    let sent = (marks.into_iter())
        .filter_map(|(txid, coins)| {
            let coins = coins.and_then(|coins| {
                (coins.split(',').filter(|c| !c.is_empty()))
                    .map(OutPoint::from_str)
                    .collect::<Result<BTreeSet<_>, _>>()
                    .ok()
            });
            let Some(coins) = coins else {
                tracing::warn!(%wallet, "a sent mark does not read back");
                return None;
            };
            let kept = txs.contains_key(&txid);
            let tx = (txs.get(&txid).cloned().flatten())
                .and_then(|hex| hex::decode(hex).ok())
                .and_then(|b| deserialize::<Transaction>(&b).ok())
                .filter(|tx| tx.txid() == txid);
            if kept && tx.is_none() {
                tracing::warn!(%wallet, "a sent payment's stored transaction does not read back");
            }
            let inputs = (inputs.remove(&txid).flatten().iter())
                .flat_map(|v| v.split(','))
                .filter_map(|e| match e.split(' ').collect::<Vec<_>>()[..] {
                    [o, v, a] => {
                        Some((OutPoint::from_str(o).ok()?, (v.parse().ok()?, a.to_owned())))
                    }
                    _ => None,
                })
                .collect();
            let mark = SentMark {
                coins,
                tx,
                kept,
                inputs,
            };
            Some((txid, mark))
        })
        .collect();
    (sent, true)
}

/// What a sent mark holds: the wallet's coins the app signed, and the
/// signed transaction ([`SENT_TX_PREFIX`]), which a mark an earlier build
/// wrote lacks. `kept`: a transaction is stored for it, read or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SentMark {
    pub coins: BTreeSet<OutPoint>,
    pub tx: Option<Transaction>,
    pub kept: bool,
    /// The coins' values and addresses ([`SENT_IN_PREFIX`]).
    pub inputs: BTreeMap<OutPoint, (u64, String)>,
}

/// The payments this app sent.
pub(crate) type Sent = BTreeMap<Txid, SentMark>;

/// Drops the marks of the payments of `wallet` no longer out of a block
/// in its history (`gone`): in a block, abandoned, or not stored.
pub(crate) fn forget_sent(appdb: &AppDb, wallet: WalletId, gone: &[Txid]) {
    if gone.is_empty() {
        return;
    }
    let keys: Vec<String> = (gone.iter())
        .flat_map(|t| [SENT_PREFIX, SENT_TX_PREFIX, SENT_IN_PREFIX].map(|p| format!("{p}{t}")))
        .collect();
    let writes: Vec<(&str, Option<&str>)> = keys.iter().map(|k| (k.as_str(), None)).collect();
    if let Err(e) = appdb.set_settings_durable(&wallet.to_string(), &writes) {
        tracing::warn!(%wallet, error = %e, "could not drop the marks of settled payments");
    }
}

/// dash-qt hides popups for these unless "show CoinJoin popups" is on
/// (`WalletView::processNewTransaction`).
pub fn is_coinjoin_internal(t: TxType) -> bool {
    matches!(
        t,
        TxType::CoinJoinMixing
            | TxType::CoinJoinCollateralPayment
            | TxType::CoinJoinMakeCollaterals
            | TxType::CoinJoinCreateDenominations
    )
}

/// The abandoned transactions of `wallet` recorded in the app database, and
/// whether every one was read.
pub(crate) fn load_abandoned(appdb: &AppDb, wallet: WalletId) -> (Vec<(Txid, Transaction)>, bool) {
    load_marks(appdb, wallet, ABANDONED_PREFIX, |txid, hex| {
        let tx: Transaction = deserialize(&hex::decode(hex).ok()?).ok()?;
        (tx.txid() == txid).then_some((txid, tx))
    })
}

/// What decides a transaction's actions.
#[derive(Debug, Clone, Copy)]
struct Facts {
    depth: u32,
    chain_locked: bool,
    instant_locked: bool,
    coinbase: bool,
    abandoned: bool,
    from_me: usize,
    credit: u64,
    debit: u64,
    matures_in: Option<u32>,
}

/// Abandons `root`, then each of `descendants` key-wallet's own walk did
/// not reach; nothing when it refuses `root`.
fn abandon_with(
    core: &mut key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    root: Txid,
    descendants: &[Txid],
) -> BTreeSet<Txid> {
    let mut abandoned = core
        .abandon_transaction_with_spends(root, &BTreeMap::new())
        .abandoned;
    if abandoned.is_empty() {
        return abandoned;
    }
    for t in descendants {
        if !abandoned.contains(t) {
            let more = core.abandon_transaction_with_spends(*t, &BTreeMap::new());
            abandoned.extend(more.abandoned);
        }
    }
    abandoned
}

fn facts(entry: &TxEntry, view: &ChainView<'_>) -> Facts {
    let (credit, debit, from_me) = amounts(entry, view);
    let all_from_me = from_me > 0 && from_me == entry.tx.input.len();
    let (status, _) = status_of(entry, view, all_from_me);
    let depth = depth_of(entry, view.tip);
    Facts {
        depth,
        chain_locked: status.chain_locked,
        instant_locked: status.instant_locked,
        coinbase: entry.tx.is_coin_base(),
        abandoned: entry.is_abandoned(),
        from_me,
        credit,
        debit,
        matures_in: status.matures_in,
    }
}

impl Facts {
    /// dash-qt Abandon enablement. "In a mempool" cannot be known on SPV
    /// and does not refuse.
    fn abandon_refusal(&self) -> Option<TxActionRefusal> {
        if self.abandoned {
            Some(TxActionRefusal::AlreadyAbandoned)
        } else if self.depth > 0 || self.chain_locked {
            Some(TxActionRefusal::Confirmed)
        } else if self.instant_locked {
            Some(TxActionRefusal::InstantLocked)
        } else if self.coinbase {
            Some(TxActionRefusal::Coinbase)
        } else {
            None
        }
    }

    /// dash-qt Resend enablement.
    fn resend_refusal(&self) -> Option<TxActionRefusal> {
        if self.abandoned {
            Some(TxActionRefusal::AlreadyAbandoned)
        } else if self.depth > 0 || self.chain_locked {
            Some(TxActionRefusal::Confirmed)
        } else if self.coinbase {
            Some(TxActionRefusal::Coinbase)
        } else if self.instant_locked {
            Some(TxActionRefusal::InstantLocked)
        } else if self.from_me == 0 {
            Some(TxActionRefusal::NotSentByWallet)
        } else {
            None
        }
    }
}

impl NetworkSession {
    /// Facts of one transaction of `wallet`, `TxNotFound` when unknown.
    async fn tx_facts(
        &self,
        manager: &Manager,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(TxEntry, Facts), EngineError> {
        self.with_view(manager, wallet, |view| {
            view.history
                .get(&txid)
                .map(|entry| (entry.clone(), facts(entry, view)))
        })
        .await?
        .ok_or_else(|| EngineError::TxNotFound(txid.to_string()))
    }

    /// dash-qt detail fields and action enablement for one transaction.
    pub async fn tx_detail_extras(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<TxDetailExtras, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let (entry, f) = this.tx_facts(&live.manager, wallet, txid).await?;
            let txid_s = txid.to_string();
            let id = wallet.to_string();
            let locks = this.appdb_op(move |db| db.locks(&id)).await?;
            let dust_locked_outputs = locks
                .into_iter()
                .filter(|row| {
                    row.txid == txid_s && row.reason == LockReason::Dust && row.is_locked()
                })
                .filter(|row| (row.vout as usize) < entry.tx.output.len())
                .map(|row| OutPoint::new(txid, row.vout))
                .collect();
            Ok(TxDetailExtras {
                txid: txid.to_string(),
                is_coinbase: f.coinbase,
                total_credit: f.credit,
                total_debit: Some(f.debit),
                net: i64::try_from(f.credit).unwrap_or(i64::MAX)
                    - i64::try_from(f.debit).unwrap_or(i64::MAX),
                matures_in: f.matures_in,
                in_mempool: None,
                abandoned: f.abandoned,
                can_abandon: f.abandon_refusal().is_none(),
                can_resend: f.resend_refusal().is_none(),
                dust_locked_outputs,
                last_announced_at: this.hub.announced_at(&txid),
            })
        })
        .await
    }

    /// dash-qt "Abandon transaction" (QT-091). Refusals as
    /// [`TxActionRefusal`]; the abandoned transaction and its recorded
    /// descendants show as `Abandoned`, their change leaves the balance and
    /// the coins they spent come back through a rescan.
    ///
    /// Needs a running SPV client (`SpvNotRunning`, checked first, review
    /// M2): the spent coins return only through the filter rescan, which
    /// rewinds the running client's in-memory sync height. Abandoning
    /// offline would leave them missing from the balance.
    pub async fn abandon_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            // Not while a wallet loads: its spend claims are read before
            // they are restored.
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            this.require_wallet(&wallet)?;
            if !manager.spv().is_started() {
                return Err(EngineError::SpvNotRunning);
            }
            let (_, f) = this.tx_facts(&manager, wallet, txid).await?;
            if let Some(r) = f.abandon_refusal() {
                return Err(EngineError::TxActionRefused(r));
            }
            this.abandon_inner(&manager, wallet, txid).await?;
            Ok(())
        })
        .await
    }

    /// Abandons `root` and its recorded descendants (checks done by the
    /// caller). Returns the abandoned txids.
    pub(crate) async fn abandon_inner(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        #[cfg(test)]
        let pause = self.hub.pause_abandon.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        // Planned on a copy under the lock that applies it, so the replay
        // record stored before it covers what is abandoned.
        let descendants = self.unmined_descendants(wallet, root);
        let (abandoned, checkpoint, released) = {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm
                .get_wallet_info_mut(&wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
            let mut trial = info.core_wallet.clone();
            let planned = abandon_with(&mut trial, root, &descendants);
            // key-wallet holds no record of a payment restored at load, so
            // it cannot see the InstantSend lock rehydrated for it.
            let locks = info.core_wallet.instant_send_locks();
            if planned.iter().any(|t| locks.contains(t)) {
                return Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked));
            }
            let checkpoint = self.abandon_rewind(wallet, &planned, &info.core_wallet);
            if let Some(checkpoint) = checkpoint {
                self.record_rewind_locked(wallet, info, checkpoint).await?;
            }
            let abandoned = abandon_with(&mut info.core_wallet, root, &descendants);
            if !abandoned.is_empty() {
                info.core_wallet.update_balance();
            }
            let released = self.released(
                wallet,
                &abandoned,
                info.core_wallet.observed_spent_outpoints(),
            );
            (abandoned, checkpoint, released)
        };
        // key-wallet refuses (returns nothing) when it holds the root as
        // settled.
        if abandoned.is_empty() {
            return Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed));
        }
        let txs: Vec<(Txid, Transaction)> = abandoned
            .iter()
            .filter_map(|t| self.hub.history.get(&wallet, t).map(|e| (*t, e.tx)))
            .collect();

        let live = self.live()?;
        let persister = Arc::clone(&live.persister);
        let sweep = SweepBatch {
            txids: abandoned.iter().copied().collect(),
            // An input not released (a block spent it) stays stored spent,
            // attributed to the root.
            superseded_by: root,
            winner_mined_height: None,
            released_outpoints: released.clone(),
        };
        tokio::task::spawn_blocking(move || {
            let changeset = PlatformWalletChangeSet {
                core: Some(CoreChangeSet {
                    sweeps: vec![sweep],
                    ..Default::default()
                }),
                ..Default::default()
            };
            persister.store(wallet.0, changeset)?;
            persister.flush(wallet.0)
        })
        .await?
        .map_err(|e| EngineError::Storage(format!("abandon: {e}")))?;

        let rows: Vec<(String, String)> = txs
            .iter()
            .map(|(t, tx)| (format!("{ABANDONED_PREFIX}{t}"), serialize_hex(tx)))
            .collect();
        let scope = wallet.to_string();
        self.appdb_op(move |db| {
            for (key, hex) in &rows {
                db.set_setting(&scope, key, Some(hex))?;
            }
            Ok(())
        })
        .await?;

        self.hub.history.with_wallet(wallet, |h| {
            for t in &abandoned {
                if let Some(e) = h.txs.get_mut(t) {
                    e.abandoned = true;
                }
            }
        });
        // Each abandoned transaction's own claim (DEC-179); a coin another
        // owner holds stays reserved.
        for t in &abandoned {
            self.spends
                .release(&wallet, crate::send::SpendOwner::Tx(*t));
        }

        // The released coins come back when the filter scan passes their
        // funding blocks again.
        if let Some(checkpoint) = checkpoint {
            self.rewind_recorded(manager, wallet, checkpoint).await?;
        }
        self.refresh_wallet_state(manager, wallet).await;
        let changed: Vec<Txid> = abandoned.iter().copied().collect();
        self.hub.pump.mark_history(wallet, Some(&changed));
        self.hub.pump.mark_balances(wallet);
        Ok(abandoned)
    }

    /// The history's transactions out of a block that descend from `root`,
    /// unless abandoned or InstantSend-locked: the descendants key-wallet
    /// holds no record of (a payment restored as a claimant at load, #88),
    /// every child of an output followed.
    fn unmined_descendants(&self, wallet: WalletId, root: Txid) -> Vec<Txid> {
        self.hub.history.read(&wallet, |txs| {
            let live: Vec<(&Txid, &TxEntry)> = (txs.iter())
                .filter(|(_, e)| {
                    e.context.block_info().is_none()
                        && !e.abandoned
                        && !e.instant_locked
                        && !matches!(
                            e.context,
                            key_wallet::transaction_checking::TransactionContext::InstantSend(_)
                        )
                })
                .collect();
            let mut found = BTreeSet::from([root]);
            let mut order = Vec::new();
            let mut parents = vec![root];
            while let Some(parent) = parents.pop() {
                for (t, e) in &live {
                    if !found.contains(*t)
                        && e.tx.input.iter().any(|i| i.previous_output.txid == parent)
                    {
                        found.insert(**t);
                        order.push(**t);
                        parents.push(**t);
                    }
                }
            }
            order
        })
    }

    /// The coins abandoning `abandoned` releases: their inputs that none of
    /// them created and no processed block spent (`observed`): key-wallet
    /// keeps the claim on a coin a block spent.
    fn released(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        observed: &BTreeMap<OutPoint, u32>,
    ) -> Vec<OutPoint> {
        (abandoned.iter())
            .filter_map(|t| self.hub.history.get(&wallet, t))
            .flat_map(|e| e.tx.input.into_iter().map(|i| i.previous_output))
            .filter(|o| !abandoned.contains(&o.txid) && !observed.contains_key(o))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The checkpoint abandoning `abandoned` rewinds to: below the lowest
    /// funding height of the wallet's coins it releases, or below the birth
    /// height when a coin's funding block is unknown. Each transaction's
    /// coins are the union of the wallet-owned inputs its key-wallet records
    /// name and those the history's record names (key-wallet holds none for
    /// a send an earlier session recorded, and may name fewer); one that
    /// neither holds replays from the birth height. No coins released, no
    /// rewind.
    fn abandon_rewind(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        info: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    ) -> Option<u32> {
        let birth = info.birth_height().saturating_sub(1);
        let own = |tx: &Transaction, indexes: &mut dyn Iterator<Item = u32>| -> Vec<OutPoint> {
            indexes
                .filter_map(|i| tx.input.get(i as usize))
                .map(|i| i.previous_output)
                .collect()
        };
        let mut released = BTreeSet::new();
        for t in abandoned {
            let records: Vec<_> = (info.accounts.all_accounts().into_iter())
                .filter_map(|a| a.transactions().get(t))
                .collect();
            let entry = self.hub.history.get(&wallet, t);
            let marked = (self.hub.history).read_wallet(&wallet, |h| h.marked.get(t).cloned());
            if records.is_empty() && entry.is_none() {
                return Some(birth);
            }
            let recorded = (records.iter())
                .flat_map(|r| own(&r.transaction, &mut r.input_details.iter().map(|i| i.index)));
            let held = (entry.iter()).flat_map(|e| own(&e.tx, &mut e.own_inputs.keys().copied()));
            released.extend(
                recorded
                    .chain(held)
                    .chain(marked.into_iter().flatten())
                    .filter(|o| !abandoned.contains(&o.txid)),
            );
        }
        let mut lowest = None::<u32>;
        for o in &released {
            let funded = (self.hub.history.get(&wallet, &o.txid))
                .and_then(|e| e.context.block_info().map(|b| b.height()));
            match funded {
                Some(h) => lowest = Some(lowest.map_or(h, |l| l.min(h))),
                None => return Some(birth),
            }
        }
        lowest.map(|h| h.saturating_sub(1))
    }

    /// dash-qt "Resend transaction" (QT-091): hands the stored transaction
    /// to the SPV client again. Returns once handed over (or `NoPeers` when
    /// dash-spv refused to send it); the network's verdict is only logged.
    /// The send continues only while the session stays open.
    pub async fn resend_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let (entry, f) = this.tx_facts(&manager, wallet, txid).await?;
            if let Some(r) = f.resend_refusal() {
                return Err(EngineError::TxActionRefused(r));
            }
            if !manager.spv().is_started() {
                return Err(EngineError::SpvNotRunning);
            }
            if this.hub.tracker().snapshot().connected_peers == 0 {
                return Err(EngineError::NoPeers);
            }
            let handle = this.wallet(&wallet).await?;
            // platform-wallet's only broadcast entry point waits for the
            // network's acceptance (up to dash-spv's 60 s timeout). The send
            // itself happens at once, so the call returns after a short
            // hand-off window and leaves the wait to a background task; a
            // send dash-spv refuses outright (no connection) is reported.
            let mut task = tokio::spawn(async move {
                let outcome = handle.core().broadcast_transaction(&entry.tx).await;
                match &outcome {
                    Ok(_) => tracing::info!(%txid, "resent transaction accepted"),
                    Err(e) => {
                        tracing::info!(%txid, error = %e, "resent transaction: no acceptance seen")
                    }
                }
                outcome
            });
            {
                let mut resends = this.resends.lock().unwrap_or_else(|p| p.into_inner());
                resends.retain(|r| !r.is_finished());
                resends.push(task.abort_handle());
            }
            match tokio::time::timeout(RESEND_HANDOFF, &mut task).await {
                Ok(Ok(Err(platform_wallet::PlatformWalletError::TransactionBroadcast(_)))) => {
                    return Err(EngineError::NoPeers);
                }
                Ok(Err(e)) => return Err(e.into()),
                Ok(Ok(_)) | Err(_) => {}
            }
            this.hub.note_announced(txid);
            Ok(())
        })
        .await
    }

    /// iOS "remove unconfirmed" (IOS-034): abandons every unconfirmed,
    /// non-InstantSend, non-abandoned transaction of `wallet` (all loaded
    /// wallets for `None`) and rescans from the lowest funding height of the
    /// coins they spent. Needs a running SPV client for the rescan. Returns
    /// how many transactions were dropped (descendants included).
    pub async fn drop_unconfirmed(
        self: &Arc<Self>,
        wallet: Option<WalletId>,
    ) -> Result<u32, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let _lifecycle = this.wallet_lifecycle.hold().await;
            let manager = this.manager()?;
            if let Some(id) = &wallet {
                this.require_wallet(id)?;
            }
            if !manager.spv().is_started() {
                return Err(EngineError::SpvNotRunning);
            }
            let wallets: Vec<WalletId> = match wallet {
                Some(id) => vec![id],
                None => manager
                    .list_wallet_ids_blocking()
                    .into_iter()
                    .map(WalletId)
                    .collect(),
            };
            let mut dropped = 0u32;
            for id in wallets {
                let candidates: Vec<Txid> = this
                    .with_view(&manager, id, |view| {
                        view.history
                            .iter()
                            .filter(|(_, e)| facts(e, view).abandon_refusal().is_none())
                            .map(|(t, _)| *t)
                            .collect()
                    })
                    .await?;
                let mut done: HashSet<Txid> = HashSet::new();
                for txid in candidates {
                    if done.contains(&txid) {
                        continue;
                    }
                    match this.abandon_inner(&manager, id, txid).await {
                        Ok(set) => {
                            dropped += set.len() as u32;
                            done.extend(set);
                        }
                        // Settled meanwhile: nothing to drop.
                        Err(EngineError::TxActionRefused(_)) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(dropped)
        })
        .await
    }

    /// dash-qt CSV export of the filtered history (QT-093), exact bytes.
    /// `type_names`: 19 names in `TxType` order, or empty for English.
    pub async fn export_history_csv(
        self: &Arc<Self>,
        wallet: WalletId,
        filter: HistoryFilter,
        sort: HistorySort,
        unit: Unit,
        type_names: Vec<String>,
        utc_offset_secs: i32,
    ) -> Result<String, EngineError> {
        if !type_names.is_empty() && type_names.len() != TX_TYPE_COUNT {
            return Err(EngineError::InvalidQuery(format!(
                "type_names has {} entries, expected 0 or {TX_TYPE_COUNT}",
                type_names.len()
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let watch_only = !this.vault.has_wallet_secret(&wallet.0);
            let records = filtered(this.wallet_records(wallet).await?, &filter, sort)?;
            let chain = match this.network {
                crate::DashNetwork::Mainnet => Chain::Main,
                _ => Chain::Test,
            };
            Ok(write_csv(
                &records,
                watch_only,
                unit,
                chain,
                &type_names,
                utc_offset_secs,
            ))
        })
        .await
    }

    /// Notification rows for `txids` (a `NewTransactions` event). Unknown
    /// txids are skipped.
    pub async fn tx_notices(
        self: &Arc<Self>,
        wallet: WalletId,
        txids: Vec<Txid>,
    ) -> Result<Vec<TxNotice>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let live = this.live()?;
            let labels: Labels = Self::labels(&live.appdb, wallet).await?;
            this.with_view(&live.manager, wallet, |view| {
                txids
                    .iter()
                    .filter_map(|t| view.history.get(t).map(|e| (t, e)))
                    .flat_map(|(t, e)| records_of(t, e, view, &labels).0)
                    .map(|r| TxNotice {
                        coinjoin_internal: is_coinjoin_internal(r.tx_type),
                        txid: r.txid,
                        record_index: r.record_index,
                        amount: r.amount,
                        timestamp: r.timestamp,
                        tx_type: r.tx_type,
                        address: r.address,
                        label: r.label,
                    })
                    .collect()
            })
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f() -> Facts {
        Facts {
            depth: 0,
            chain_locked: false,
            instant_locked: false,
            coinbase: false,
            abandoned: false,
            from_me: 1,
            credit: 0,
            debit: 0,
            matures_in: None,
        }
    }

    /// A mark with its signed transaction stored apart (#170 case 1) reads
    /// back with it; one an earlier build wrote reads without it; and one
    /// whose stored transaction does not decode reads without it but kept,
    /// the read of the others whole.
    #[test]
    fn a_sent_mark_reads_back_its_transaction() {
        use dashcore::hashes::Hash;
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let coin = OutPoint::new(Txid::from_byte_array([3; 32]), 1);
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![dashcore::TxIn {
                previous_output: coin,
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let (old, bad) = (
            Txid::from_byte_array([4; 32]),
            Txid::from_byte_array([5; 32]),
        );
        let set = |key: &str, value: &str| db.set_setting(&scope, key, Some(value)).unwrap();
        for t in [tx.txid(), old, bad] {
            set(&format!("{SENT_PREFIX}{t}"), &coin.to_string());
        }
        set(
            &format!("{SENT_TX_PREFIX}{}", tx.txid()),
            &serialize_hex(&tx),
        );
        set(&format!("{SENT_TX_PREFIX}{bad}"), "zz");
        let mark = |tx, kept| SentMark {
            inputs: Default::default(),
            coins: BTreeSet::from([coin]),
            tx,
            kept,
        };
        let want = Sent::from([
            (tx.txid(), mark(Some(tx.clone()), true)),
            (old, mark(None, false)),
            (bad, mark(None, true)),
        ]);
        assert_eq!(load_sent(&db, wallet), (want, true));
    }

    /// A mark whose txid suffix or value does not parse is skipped and the
    /// other marks are still read; a malformed abandoned mark also makes
    /// that read incomplete, a malformed sent mark does not.
    #[test]
    fn a_malformed_mark_is_skipped() {
        use dashcore::hashes::Hash;
        let db = AppDb::open_in_memory().unwrap();
        let wallet = WalletId([7; 32]);
        let scope = wallet.to_string();
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: Vec::new(),
            output: Vec::new(),
            special_transaction_payload: None,
        };
        let txid = tx.txid();
        let coin = OutPoint::new(txid, 0);
        let set = |key: &str, value: Option<&str>| db.set_setting(&scope, key, value).unwrap();
        set(&format!("{SENT_PREFIX}{txid}"), Some(&coin.to_string()));
        set(
            &format!("{ABANDONED_PREFIX}{txid}"),
            Some(&serialize_hex(&tx)),
        );
        let sent = Sent::from([(
            txid,
            SentMark {
                inputs: Default::default(),
                coins: BTreeSet::from([coin]),
                tx: None,
                kept: false,
            },
        )]);
        assert_eq!(load_sent(&db, wallet), (sent.clone(), true));
        assert_eq!(
            load_abandoned(&db, wallet),
            (vec![(txid, tx.clone())], true)
        );

        let other = Txid::all_zeros();
        for (key, value) in [
            (format!("{SENT_PREFIX}not-a-txid"), coin.to_string()),
            (
                format!("{SENT_PREFIX}{other}"),
                "not-an-outpoint".to_owned(),
            ),
        ] {
            set(&key, Some(&value));
            assert_eq!(load_sent(&db, wallet), (sent.clone(), true), "{key}");
            set(&key, None);
        }
        for (key, value) in [
            (format!("{ABANDONED_PREFIX}not-a-txid"), serialize_hex(&tx)),
            (format!("{ABANDONED_PREFIX}{other}"), "zz".to_owned()),
            (format!("{ABANDONED_PREFIX}{other}"), serialize_hex(&tx)),
        ] {
            set(&key, Some(&value));
            let abandoned = vec![(txid, tx.clone())];
            assert_eq!(load_abandoned(&db, wallet), (abandoned, false), "{key}");
            set(&key, None);
        }
    }

    #[test]
    fn test_qt_091_abandon_and_resend_enablement() {
        assert_eq!(f().abandon_refusal(), None);
        assert_eq!(f().resend_refusal(), None);
        let confirmed = Facts { depth: 1, ..f() };
        assert_eq!(
            confirmed.abandon_refusal(),
            Some(TxActionRefusal::Confirmed)
        );
        assert_eq!(confirmed.resend_refusal(), Some(TxActionRefusal::Confirmed));
        let is = Facts {
            instant_locked: true,
            ..f()
        };
        assert_eq!(is.abandon_refusal(), Some(TxActionRefusal::InstantLocked));
        assert_eq!(is.resend_refusal(), Some(TxActionRefusal::InstantLocked));
        let gone = Facts {
            abandoned: true,
            ..f()
        };
        assert_eq!(
            gone.abandon_refusal(),
            Some(TxActionRefusal::AlreadyAbandoned)
        );
        assert_eq!(
            gone.resend_refusal(),
            Some(TxActionRefusal::AlreadyAbandoned)
        );
        let incoming = Facts { from_me: 0, ..f() };
        assert_eq!(
            incoming.abandon_refusal(),
            None,
            "dash-qt abandons any unconfirmed"
        );
        assert_eq!(
            incoming.resend_refusal(),
            Some(TxActionRefusal::NotSentByWallet)
        );
        let cb = Facts {
            coinbase: true,
            ..f()
        };
        assert_eq!(cb.abandon_refusal(), Some(TxActionRefusal::Coinbase));
        assert_eq!(cb.resend_refusal(), Some(TxActionRefusal::Coinbase));
    }

    /// An abandoned transaction seen since with an InstantSend lock counts
    /// again (the history's status rule), so Abandon refuses it as locked.
    #[test]
    fn an_abandoned_transaction_seen_instant_locked_refuses_as_locked() {
        let entry = TxEntry {
            tx: Transaction {
                version: 2,
                lock_time: 0,
                input: Vec::new(),
                output: Vec::new(),
                special_transaction_payload: None,
            },
            context: key_wallet::transaction_checking::TransactionContext::InstantSend(
                Default::default(),
            ),
            instant_locked: false,
            first_seen: None,
            own_inputs: Default::default(),
            abandoned: true,
        };
        let (owned, history) = (Default::default(), Default::default());
        let payees = Default::default();
        let view = ChainView {
            owned: &owned,
            history: &history,
            tip: 100,
            chainlock_height: None,
            network: dashcore::Network::Regtest,
            payees: &payees,
        };
        assert_eq!(
            facts(&entry, &view).abandon_refusal(),
            Some(TxActionRefusal::InstantLocked)
        );
    }

    #[test]
    fn test_qt_033_coinjoin_internal_types() {
        assert!(is_coinjoin_internal(TxType::CoinJoinMixing));
        assert!(is_coinjoin_internal(TxType::CoinJoinCreateDenominations));
        assert!(!is_coinjoin_internal(TxType::CoinJoinSend));
        assert!(!is_coinjoin_internal(TxType::RecvWithAddress));
    }
}
