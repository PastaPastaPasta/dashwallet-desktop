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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::consensus::encode::{deserialize, serialize_hex};
use dashcore::{BlockHash, OutPoint, Transaction, Txid};
use dw_appdb::{AppDb, LockReason};
use dw_units::{Chain, Unit};
use platform_wallet::changeset::changeset::SweepBatch;
use platform_wallet::changeset::{
    CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
};

use crate::csv_export::{TX_TYPE_COUNT, write_csv};
use crate::history::{
    ChainView, HistoryFilter, HistorySort, Labels, TxEntry, TxType, amounts, chainlock_beaten,
    depth_of, filtered, records_of, rival_heights, status_of,
};
use crate::session::Manager;
use crate::{EngineError, NetworkSession, WalletId};
use key_wallet::transaction_checking::TransactionContext;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
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
    /// The ChainLock exception (#170 case 14) lacks a coin the payment's
    /// own spend claim, restored at the wallet's load, keeps out of the
    /// wallet: a restart (which no longer restores it) and a rescan give
    /// the evidence.
    RescanFirst,
}

impl TxActionRefusal {
    /// The refusal's message: what to do for [`Self::RescanFirst`], the
    /// refusal's name otherwise.
    pub fn message(&self) -> String {
        match self {
            Self::RescanFirst => {
                "Restart Dash Wallet, run Rescan from the wallet's birth, then try Abandon again."
                    .into()
            }
            other => format!("transaction action refused: {other:?}"),
        }
    }
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
/// never sent, and dropped once the payment is gone from the history or
/// proven ChainLocked: in the session by its wallet's ChainLock of its own
/// block, or at load by the SPV header chain.
pub(crate) const SENT_PREFIX: &str = "sent:";

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
/// signed ([`SENT_PREFIX`]), and whether every mark was read.
pub(crate) fn load_sent(appdb: &AppDb, wallet: WalletId) -> (Sent, bool) {
    let (marks, read) = load_marks(appdb, wallet, SENT_PREFIX, |txid, coins| {
        let coins = (coins.split(',').filter(|c| !c.is_empty()))
            .map(OutPoint::from_str)
            .collect::<Result<BTreeSet<_>, _>>()
            .ok()?;
        Some((txid, coins))
    });
    (marks.into_iter().collect(), read)
}

/// The payments this app sent, each with the wallet's coins it signed.
pub(crate) type Sent = BTreeMap<Txid, BTreeSet<OutPoint>>;

/// Drops the marks of the payments of `wallet` (`gone`) gone from the
/// history or ChainLocked.
pub(crate) fn forget_sent(appdb: &AppDb, wallet: WalletId, gone: &[Txid]) {
    if gone.is_empty() {
        return;
    }
    let keys: Vec<String> = gone.iter().map(|t| format!("{SENT_PREFIX}{t}")).collect();
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
    /// [`chainlock_beaten`]: a block under the wallet's ChainLock spends
    /// one of its inputs.
    chainlock_beaten: bool,
    coinbase: bool,
    abandoned: bool,
    from_me: usize,
    credit: u64,
    debit: u64,
    /// The debit counts none of the inputs, some are unresolved, and the
    /// transaction pays one of this wallet's change addresses: a payment of
    /// this wallet whose coins it does not know yet.
    debit_unknown: bool,
    /// Some inputs are not known to be this wallet's.
    inputs_unresolved: bool,
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

/// What [`chainlock_beaten`] reads: the wallet's ChainLock when the SPV
/// header chain holds its block, and that chain's block hash at each
/// height it has of those asked. `blocked`: the locked
/// transactions out of a block the exception cannot apply to
/// ([`override_blocked`]) when it was read.
#[derive(Default)]
struct ChainProof {
    chainlock: Option<(u32, BlockHash)>,
    active: HashMap<u32, BlockHash>,
    blocked: HashSet<Txid>,
}

fn locked(e: &TxEntry) -> bool {
    e.instant_locked || matches!(e.context, TransactionContext::InstantSend(_))
}

/// Whether the ChainLock exception cannot apply to a root: key-wallet
/// holds the root's own record in a block, InstantSend-locked or finalized
/// (it refuses the abandon), or a transaction that descends from it is in
/// a block, InstantSend-locked or finalized by any of the history
/// (abandoned ones too), key-wallet's records, its locks and the txids it
/// keeps finalized. Reads `txs` once, for any number of roots.
fn override_blocked<'a>(
    txs: &HashMap<Txid, TxEntry>,
    core: &'a ManagedWalletInfo,
) -> impl Fn(&Txid) -> bool + 'a {
    let settled = |c: &TransactionContext| {
        c.block_info().is_some() || matches!(c, TransactionContext::InstantSend(_))
    };
    let accounts = core.accounts.all_accounts();
    let finalized = |t: &Txid| accounts.iter().any(|a| a.transaction_is_finalized(t));
    let mut known: HashMap<Txid, (Vec<Txid>, bool)> = HashMap::new();
    let mut add = |t: Txid, tx: &Transaction, settled: bool| {
        let (_, s) = known.entry(t).or_insert_with(|| {
            (
                tx.input.iter().map(|i| i.previous_output.txid).collect(),
                false,
            )
        });
        *s |= settled || core.instant_send_locks().contains(&t) || finalized(&t);
    };
    for (t, e) in txs {
        add(*t, &e.tx, locked(e) || settled(&e.context));
    }
    for account in &accounts {
        for (t, r) in account.transactions() {
            add(*t, &r.transaction, settled(&r.context));
        }
    }
    let mut under = HashSet::new();
    let mut parents: Vec<Txid> = (known.values())
        .filter(|(_, settled)| *settled)
        .flat_map(|(parents, _)| parents.iter().copied())
        .collect();
    while let Some(t) = parents.pop() {
        if under.insert(t) {
            parents.extend(
                known
                    .get(&t)
                    .into_iter()
                    .flat_map(|(p, _)| p.iter().copied()),
            );
        }
    }
    move |root| {
        under.contains(root)
            || accounts.iter().any(|a| {
                a.transaction_is_finalized(root)
                    || a.transactions()
                        .get(root)
                        .is_some_and(|r| settled(&r.context))
            })
    }
}

/// The transactions out of a block that descend from `root` and are not
/// InstantSend-locked, every child of an output followed, in the history
/// `txs` and in key-wallet's records of `core` (which credits a mempool
/// transaction before the history hears of it): those not abandoned in
/// the order found (key-wallet holds no record of a payment restored as a
/// claimant at load, #88), and the abandoned ones passed through. The
/// history's view of a transaction it holds wins.
fn unmined_descendants(
    txs: &HashMap<Txid, TxEntry>,
    core: &ManagedWalletInfo,
    root: Txid,
) -> (Vec<Txid>, BTreeSet<Txid>) {
    // A contact's account records the contact's own spends: not this
    // wallet's to abandon.
    let accounts: Vec<_> = (core.accounts.all_accounts().into_iter())
        .filter(|a| !a.managed_account_type().is_contact_owned())
        .collect();
    let locks = core.instant_send_locks();
    let mut open: Vec<(Txid, &Transaction, bool)> = (txs.iter())
        .filter(|(_, e)| e.context.block_info().is_none() && !locked(e))
        .map(|(t, e)| (*t, &e.tx, e.abandoned))
        .collect();
    let mut recorded = HashSet::new();
    for account in &accounts {
        for (t, r) in account.transactions() {
            if !txs.contains_key(t)
                && recorded.insert(*t)
                && r.context.block_info().is_none()
                && !matches!(r.context, TransactionContext::InstantSend(_))
                && !locks.contains(t)
            {
                open.push((*t, &r.transaction, false));
            }
        }
    }
    let mut found = BTreeSet::from([root]);
    let (mut live, mut links) = (Vec::new(), BTreeSet::new());
    let mut parents = vec![root];
    while let Some(parent) = parents.pop() {
        for (t, tx, abandoned) in &open {
            if !found.contains(t) && tx.input.iter().any(|i| i.previous_output.txid == parent) {
                found.insert(*t);
                match abandoned {
                    true => links.insert(*t),
                    false => {
                        live.push(*t);
                        true
                    }
                };
                parents.push(*t);
            }
        }
    }
    (live, links)
}

fn facts(entry: &TxEntry, view: &ChainView<'_>, proof: &ChainProof) -> Facts {
    let (credit, debit, from_me) = amounts(entry, view);
    let all_from_me = from_me > 0 && from_me == entry.tx.input.len();
    let (status, _) = status_of(entry, view, all_from_me);
    let depth = depth_of(entry, view.tip);
    Facts {
        depth,
        chain_locked: status.chain_locked,
        instant_locked: status.instant_locked,
        chainlock_beaten: status.instant_locked
            && depth == 0
            && !proof.blocked.contains(&entry.tx.txid())
            && chainlock_beaten(
                view.history,
                &entry.tx.txid(),
                proof.chainlock,
                &proof.active,
            ),
        coinbase: entry.tx.is_coin_base(),
        abandoned: entry.is_abandoned(),
        from_me,
        credit,
        debit,
        inputs_unresolved: from_me < entry.tx.input.len() && !entry.tx.is_coin_base(),
        debit_unknown: debit == 0
            && from_me < entry.tx.input.len()
            && !entry.tx.is_coin_base()
            && (entry.tx.output.iter()).any(|o| {
                (view.owned.get(&o.script_pubkey))
                    .is_some_and(|w| w.chain == crate::history::AddressChain::Change)
            }),
        matures_in: status.matures_in,
    }
}

impl Facts {
    /// dash-qt Abandon enablement. "In a mempool" cannot be known on SPV
    /// and does not refuse; an InstantSend lock does, unless a ChainLocked
    /// spend of one of its inputs overrode it.
    fn abandon_refusal(&self) -> Option<TxActionRefusal> {
        if self.abandoned {
            Some(TxActionRefusal::AlreadyAbandoned)
        } else if self.depth > 0 || self.chain_locked {
            Some(TxActionRefusal::Confirmed)
        } else if self.instant_locked && !self.chainlock_beaten {
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
    /// With `offer`, the ChainLock exception counts only when the abandon's
    /// plan would not refuse it (what the detail offers); without, the
    /// abandon plans it once, under its lock.
    async fn tx_facts(
        &self,
        manager: &Manager,
        wallet: WalletId,
        txid: Txid,
        offer: bool,
    ) -> Result<(TxEntry, Facts), EngineError> {
        let proof = self.chain_proof(manager, wallet).await;
        let (entry, mut f) = self
            .with_view(manager, wallet, |view| {
                view.history
                    .get(&txid)
                    .map(|entry| (entry.clone(), facts(entry, view, &proof)))
            })
            .await?
            .ok_or_else(|| EngineError::TxNotFound(txid.to_string()))?;
        // The exception's evidence on the inputs, as the abandon checks it:
        // not offered when it would refuse as locked.
        if offer && f.chainlock_beaten && f.abandon_refusal().is_none() {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let planned = (wm.get_wallet_info(&wallet.0)).map(|i| {
                let (descendants, _) = (self.hub.history).read(&wallet, |txs| {
                    unmined_descendants(txs, &i.core_wallet, txid)
                });
                self.plan_abandon(wallet, txid, &descendants, &proof, &i.core_wallet)
            });
            if let Some(Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked))) = planned
            {
                f.chainlock_beaten = false;
            }
        }
        Ok((entry, f))
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
            let (entry, f) = this.tx_facts(&live.manager, wallet, txid, true).await?;
            // A payment this app sent whose coins are not resolved yet (one
            // with no change too): its debit is unknown. A failed read is
            // taken as one.
            let debit_unknown = f.debit_unknown
                || (f.debit == 0 && f.inputs_unresolved && {
                    let (scope, key) = (wallet.to_string(), format!("{SENT_PREFIX}{txid}"));
                    !matches!(
                        this.appdb_op(move |db| db.setting(&scope, &key)).await,
                        Ok(None)
                    )
                });
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
                total_debit: (!debit_unknown).then_some(f.debit),
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
            let (_, f) = this.tx_facts(&manager, wallet, txid, false).await?;
            if let Some(r) = f.abandon_refusal() {
                return Err(EngineError::TxActionRefused(r));
            }
            this.abandon_inner(&manager, wallet, txid).await?;
            Ok(())
        })
        .await
    }

    /// Abandons `root` and its recorded descendants (checks done by the
    /// caller). Returns the abandoned txids. An InstantSend-locked root goes
    /// only by [`Self::lock_override`], and loses its lock in the history
    /// too, as the store's sweep drops it.
    pub(crate) async fn abandon_inner(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let mut recorded = None;
        let result = self
            .abandon_unrewound(manager, wallet, root, &mut recorded)
            .await;
        let abandoned = result.as_ref().ok().cloned().unwrap_or_default();
        #[cfg(test)]
        if (self.hub.quit_before_rescan).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit before the rescan".into()));
        }
        if result.is_ok() || recorded.is_some() {
            self.finish_abandons(manager, wallet, &abandoned, recorded)
                .await?;
        }
        result
    }

    /// [`Self::abandon_inner`] up to the rescan: the rewind is recorded,
    /// not made, and `recorded` lowered to the checkpoint it starts from,
    /// failure or not. Returns the abandoned txids.
    async fn abandon_unrewound(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
        recorded: &mut Option<u32>,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        #[cfg(test)]
        let pause = match (self.hub.pause_abandon_skip).fetch_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_sub(1),
        ) {
            Ok(_) => None,
            Err(_) => self.hub.pause_abandon.lock().unwrap().take(),
        };
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let proof = self.root_proof(manager, wallet, root).await;
        // Planned on a copy under the lock that applies it, so the replay
        // record stored before it covers what is abandoned; held until the
        // sweep is stored, so no descendant is credited between the plan and
        // the sweep.
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let held_since = std::time::Instant::now();
        let (abandoned, released, overridden, copies, links) = {
            let info = wm
                .get_wallet_info_mut(&wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
            let (descendants, links) = (self.hub.history).read(&wallet, |txs| {
                unmined_descendants(txs, &info.core_wallet, root)
            });
            let (planned, overridden) =
                self.plan_abandon(wallet, root, &descendants, &proof, &info.core_wallet)?;
            // What key-wallet abandons and the history has not heard of is
            // marked, and its coins released, from key-wallet's copy in this
            // wallet's own accounts.
            let copies: HashMap<Txid, Transaction> = (info.core_wallet.accounts.all_accounts())
                .into_iter()
                .filter(|a| !a.managed_account_type().is_contact_owned())
                .flat_map(|a| a.transactions().iter())
                .filter(|(t, _)| planned.contains(*t))
                .map(|(t, r)| (*t, r.transaction.clone()))
                .collect();
            let checkpoint = self.abandon_rewind(wallet, &planned, &links, &info.core_wallet);
            if let Some(checkpoint) = checkpoint {
                self.record_rewind_locked(wallet, info, checkpoint).await?;
                *recorded = (*recorded).into_iter().chain([checkpoint]).min();
            }
            let abandoned = abandon_with(&mut info.core_wallet, root, &descendants);
            if !abandoned.is_empty() {
                info.core_wallet.update_balance();
            }
            let mut released = overridden.clone().unwrap_or_else(|| {
                self.released(
                    wallet,
                    &abandoned,
                    &links,
                    &copies,
                    info.core_wallet.observed_spent_outpoints(),
                )
            });
            // A coin another pending transaction of the wallet still spends
            // (key-wallet records conflicting mempool spends) stays spent.
            let still_spent: HashSet<OutPoint> = (info.core_wallet.accounts.all_accounts())
                .into_iter()
                .flat_map(|a| a.transactions().values())
                .flat_map(|r| r.transaction.input.iter().map(|i| i.previous_output))
                .collect();
            released.retain(|o| !still_spent.contains(o));
            (abandoned, released, overridden.is_some(), copies, links)
        };
        // key-wallet refuses (returns nothing) when it holds the root as
        // settled; its own record of an InstantSend lock is that.
        if abandoned.is_empty() {
            return Err(EngineError::TxActionRefused(match overridden {
                true => TxActionRefusal::InstantLocked,
                false => TxActionRefusal::Confirmed,
            }));
        }
        let txs: Vec<(Txid, Transaction)> = abandoned
            .iter()
            .filter_map(|t| {
                let tx = (self.hub.history.get(&wallet, t).map(|e| e.tx))
                    .or_else(|| copies.get(t).cloned())?;
                Some((*t, tx))
            })
            .collect();

        // An override's root mark goes first: an exit before the sweep leaves
        // the root locked, so it loads as it was, and its descendants unmarked,
        // so a second abandon walks them again.
        if let Some((t, tx)) = txs.iter().find(|(t, _)| overridden && *t == root) {
            let (key, hex) = (format!("{ABANDONED_PREFIX}{t}"), serialize_hex(tx));
            let scope = wallet.to_string();
            (self.appdb_op(move |db| db.set_setting(&scope, &key, Some(&hex)))).await?;
        }
        #[cfg(test)]
        if (self.hub.quit_before_sweep).swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(EngineError::Internal("quit before the sweep".into()));
        }
        let live = self.live()?;
        let persister = Arc::clone(&live.persister);
        #[cfg(test)]
        {
            *self.hub.last_release.lock().unwrap() = Some(released.clone());
        }
        #[cfg(test)]
        let pause = self.hub.pause_sweep.lock().unwrap().take();
        #[cfg(test)]
        if let Some(pause) = pause {
            pause
                .reached
                .store(true, std::sync::atomic::Ordering::SeqCst);
            pause.release.notified().await;
        }
        let sweep = SweepBatch {
            // The abandoned transactions passed through are swept again, so
            // an input of theirs is a dead parent's output, not a claim.
            txids: abandoned.iter().chain(&links).copied().collect(),
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
        drop(wm);
        let held = held_since.elapsed();
        tracing::debug!(%wallet, held_ms = held.as_millis() as u64, "an abandon held the wallet manager through its sweep");
        #[cfg(test)]
        {
            *self.hub.last_abandon_hold.lock().unwrap() = Some(held);
        }

        let rows: Vec<(String, String)> = txs
            .iter()
            .map(|(t, tx)| (format!("{ABANDONED_PREFIX}{t}"), serialize_hex(tx)))
            .collect();
        #[cfg(test)]
        {
            let mut fail = self.hub.fail_marks_of.lock().unwrap();
            if *fail == Some(root) {
                *fail = None;
                return Err(EngineError::Storage("abandoned marks".into()));
            }
        }
        let scope = wallet.to_string();
        self.appdb_op(move |db| {
            for (key, hex) in &rows {
                db.set_setting(&scope, key, Some(hex))?;
            }
            Ok(())
        })
        .await?;

        self.hub.history.with_wallet(wallet, |h| {
            let unheard: Vec<(Txid, Transaction)> = (txs.iter())
                .filter(|(t, _)| !h.txs.contains_key(t))
                .cloned()
                .collect();
            h.mark_abandoned(&unheard);
            for t in &abandoned {
                if let Some(e) = h.txs.get_mut(t) {
                    e.abandoned = true;
                    if overridden && *t == root {
                        e.instant_locked = false;
                        if matches!(e.context, TransactionContext::InstantSend(_)) {
                            e.context = TransactionContext::Mempool;
                        }
                    }
                }
            }
        });
        // Each abandoned transaction's own claim (DEC-179); a coin another
        // owner holds stays reserved.
        for t in &abandoned {
            self.spends
                .release(&wallet, crate::send::SpendOwner::Tx(*t));
        }
        Ok(abandoned)
    }

    /// Plans abandoning `root` and `descendants` on a copy of `core`: the
    /// txids key-wallet would abandon, and the coins the ChainLock
    /// exception releases (`None` without it). Refuses as the abandon
    /// would.
    fn plan_abandon(
        &self,
        wallet: WalletId,
        root: Txid,
        descendants: &[Txid],
        proof: &ChainProof,
        core: &ManagedWalletInfo,
    ) -> Result<(BTreeSet<Txid>, Option<Vec<OutPoint>>), EngineError> {
        let mut trial = core.clone();
        let planned = abandon_with(&mut trial, root, descendants);
        // key-wallet holds no record of a payment restored at load, so
        // it cannot see the InstantSend lock rehydrated for it.
        let locks = core.instant_send_locks();
        let overridden = self.lock_override(wallet, &root, &planned, proof, core, &trial)?;
        let exempt = |t: &Txid| overridden.is_some() && *t == root;
        // key-wallet walks by its own records: one it holds as pending can
        // be locked or mined in the history, and then what spends it is
        // not the wallet's to abandon.
        let settled = (self.hub.history).read(&wallet, |txs| {
            (planned.iter()).any(|t| {
                !exempt(t)
                    && txs
                        .get(t)
                        .is_some_and(|e| locked(e) || e.context.block_info().is_some())
            })
        });
        if settled || planned.iter().any(|t| locks.contains(t) && !exempt(t)) {
            return Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked));
        }
        Ok((planned, overridden))
    }

    /// [`Self::chain_proof`] when `root` is InstantSend-locked, in the
    /// history or by key-wallet's locks: only then does the exception read
    /// it.
    async fn root_proof(&self, manager: &Manager, wallet: WalletId, root: Txid) -> ChainProof {
        let locked = match self
            .hub
            .history
            .read(&wallet, |txs| txs.get(&root).map(locked))
        {
            None => false,
            Some(true) => true,
            Some(false) => {
                let wm = manager.wallet_manager_arc();
                let wm = wm.read().await;
                (wm.get_wallet_info(&wallet.0))
                    .is_some_and(|i| i.core_wallet.instant_send_locks().contains(&root))
            }
        };
        match locked {
            true => self.chain_proof(manager, wallet).await,
            false => ChainProof::default(),
        }
    }

    /// Rescans `wallet` from `checkpoint`, after abandons of `abandoned`
    /// recorded it: the released coins come back when the filter scan
    /// passes their funding blocks again.
    async fn finish_abandons(
        &self,
        manager: &Manager,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        checkpoint: Option<u32>,
    ) -> Result<(), EngineError> {
        if let Some(checkpoint) = checkpoint {
            self.rewind_recorded(manager, wallet, checkpoint).await?;
        }
        self.refresh_wallet_state(manager, wallet).await;
        let changed: Vec<Txid> = abandoned.iter().copied().collect();
        self.hub.pump.mark_history(wallet, Some(&changed));
        self.hub.pump.mark_balances(wallet);
        Ok(())
    }

    /// The ChainLock exception (#170 case 14) for `root`, whose abandon
    /// plans `planned`, when it is InstantSend-locked: a transaction
    /// in a block of `proof`'s ChainLocked chain spends one of its inputs,
    /// nothing settled descends from it, the history loaded every stored
    /// record and holds each of `planned`, and the scan is past the
    /// ChainLock with no replay pending. Each input of `planned` that a
    /// recorded block spends must be spent in a block of that chain at or
    /// below the ChainLock, and stays stored spent, claimed by that record;
    /// each other one must be a coin key-wallet holds unspent from a
    /// scanned block once `planned` is abandoned (`trial`), and is
    /// released; `RescanFirst` when all that is missing is coins the root's
    /// own spend claim, restored at the load, holds (a descendant's comes
    /// back at the next load). `None` for a root
    /// without a lock, `InstantLocked` when the evidence falls short.
    fn lock_override(
        &self,
        wallet: WalletId,
        root: &Txid,
        planned: &BTreeSet<Txid>,
        proof: &ChainProof,
        core: &ManagedWalletInfo,
        trial: &ManagedWalletInfo,
    ) -> Result<Option<Vec<OutPoint>>, EngineError> {
        let refused = Err(EngineError::TxActionRefused(TxActionRefusal::InstantLocked));
        let (incomplete, claimed) =
            (self.hub.history).with_wallet(wallet, |h| (h.load_incomplete, h.claimed.clone()));
        self.hub.history.read(&wallet, |txs| {
            let Some(e) = txs.get(root) else {
                return Ok(None);
            };
            if !locked(e) && !core.instant_send_locks().contains(root) {
                return Ok(None);
            }
            let synced = core.synced_height();
            let (Some((chainlock, _)), Some(state)) =
                (proof.chainlock, self.hub.wallet_state(&wallet))
            else {
                return refused;
            };
            let (live, links) = unmined_descendants(txs, core, *root);
            if !chainlock_beaten(txs, root, proof.chainlock, &proof.active)
                || !live.iter().all(|t| planned.contains(t))
                || override_blocked(txs, core)(root)
                || incomplete
                || synced < chainlock
                || state.replaying_at(synced)
                || !planned.iter().all(|t| txs.contains_key(t))
            {
                return refused;
            }
            let proven = |w: &TxEntry| {
                (w.context.block_info()).is_some_and(|b| {
                    b.height() <= chainlock
                        && proof.active.get(&b.height()) == Some(&b.block_hash())
                })
            };
            let mut mined: HashMap<OutPoint, bool> = HashMap::new();
            for w in txs.values().filter(|w| w.context.block_info().is_some()) {
                for i in &w.tx.input {
                    *mined.entry(i.previous_output).or_insert(true) &= proven(w);
                }
            }
            let held: HashSet<OutPoint> = (trial.utxos().iter())
                .filter(|u| u.is_confirmed && u.height <= synced)
                .map(|u| u.outpoint)
                .collect();
            let inputs: BTreeSet<OutPoint> = (planned.iter())
                .filter_map(|t| txs.get(t))
                .flat_map(|p| p.tx.input.iter().map(|i| i.previous_output))
                .filter(|o| !planned.contains(&o.txid) && !links.contains(&o.txid))
                .collect();
            let (mut free, mut claim_held) = (Vec::new(), false);
            for o in inputs {
                match mined.get(&o) {
                    Some(true) => {}
                    Some(false) => return refused,
                    None if held.contains(&o) => free.push(o),
                    None if claimed.get(&o) == Some(root) => claim_held = true,
                    None => return refused,
                }
            }
            match claim_held {
                true => Err(EngineError::TxActionRefused(TxActionRefusal::RescanFirst)),
                false => Ok(Some(free)),
            }
        })
    }

    /// What [`chainlock_beaten`] reads for `wallet`: the SPV header chain's
    /// block hash at each height [`rival_heights`] names, and the height of
    /// the wallet's ChainLock when that chain holds its block before and
    /// after those are read (a Rebuild Index keeps the ChainLock but not
    /// the headers), and the locked transactions [`override_blocked`] keeps
    /// Abandon off for. No ChainLock while the scan is below it or replays,
    /// or the history's load was incomplete.
    async fn chain_proof(&self, manager: &Manager, wallet: WalletId) -> ChainProof {
        let (chainlock, blocked) = {
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let Some(core) = wm.get_wallet_info(&wallet.0).map(|i| &i.core_wallet) else {
                return ChainProof::default();
            };
            let chainlock = core.metadata.last_applied_chain_lock.clone();
            let synced = core.synced_height();
            let scanned = (self.hub.wallet_state(&wallet)).is_some_and(|s| !s.replaying_at(synced))
                && !self.hub.history.with_wallet(wallet, |h| h.load_incomplete);
            let Some(chainlock) = chainlock.filter(|c| scanned && synced >= c.block_height) else {
                return ChainProof::default();
            };
            let blocked = self.hub.history.read(&wallet, |txs| {
                let roots: Vec<Txid> = (txs.iter())
                    .filter(|(_, e)| locked(e) && e.context.block_info().is_none())
                    .map(|(t, _)| *t)
                    .collect();
                if roots.is_empty() {
                    return None;
                }
                let blocks = override_blocked(txs, core);
                Some(roots.into_iter().filter(|t| blocks(t)).collect())
            });
            let Some(blocked) = blocked else {
                return ChainProof::default();
            };
            (chainlock, blocked)
        };
        let cl = (chainlock.block_height, chainlock.block_hash);
        let heights = self.hub.history.read(&wallet, rival_heights);
        let Some(active) = self.headers_under(cl, heights).await else {
            return ChainProof::default();
        };
        ChainProof {
            chainlock: Some(cl),
            active,
            blocked,
        }
    }

    /// The coins abandoning `abandoned` releases: the inputs of each (from
    /// the history, or key-wallet's copy of one it has not heard of) that
    /// none of them nor of the abandoned `links` between them created and
    /// no processed block spent (`observed`): key-wallet keeps the claim on
    /// a coin a block spent.
    fn released(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        links: &BTreeSet<Txid>,
        copies: &HashMap<Txid, Transaction>,
        observed: &BTreeMap<OutPoint, u32>,
    ) -> Vec<OutPoint> {
        (abandoned.iter())
            .filter_map(|t| {
                (self.hub.history.get(&wallet, t).map(|e| e.tx)).or_else(|| copies.get(t).cloned())
            })
            .flat_map(|tx| tx.input.into_iter().map(|i| i.previous_output))
            .filter(|o| {
                !abandoned.contains(&o.txid)
                    && !links.contains(&o.txid)
                    && !observed.contains_key(o)
            })
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
    /// rewind. A coin of the abandoned `links` between them is none.
    fn abandon_rewind(
        &self,
        wallet: WalletId,
        abandoned: &BTreeSet<Txid>,
        links: &BTreeSet<Txid>,
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
            if records.is_empty() && entry.is_none() {
                return Some(birth);
            }
            let recorded = (records.iter())
                .flat_map(|r| own(&r.transaction, &mut r.input_details.iter().map(|i| i.index)));
            let held = (entry.iter()).flat_map(|e| own(&e.tx, &mut e.own_inputs.keys().copied()));
            released.extend(
                recorded
                    .chain(held)
                    .filter(|o| !abandoned.contains(&o.txid) && !links.contains(&o.txid)),
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
            let (entry, f) = this.tx_facts(&manager, wallet, txid, false).await?;
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

    /// iOS "remove unconfirmed" (IOS-034): abandons every transaction of
    /// `wallet` (all loaded wallets for `None`) that Abandon would: out of a
    /// block, not abandoned, and not InstantSend-locked unless a ChainLocked
    /// spend of one of its inputs beat it; and rescans from the lowest
    /// funding height of the coins they spent. Needs a running SPV client
    /// for the rescan. Returns how many transactions were dropped
    /// (descendants included).
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
                let proof = this.chain_proof(&manager, id).await;
                let candidates: Vec<Txid> = this
                    .with_view(&manager, id, |view| {
                        view.history
                            .iter()
                            .filter(|(_, e)| facts(e, view, &proof).abandon_refusal().is_none())
                            .map(|(t, _)| *t)
                            .collect()
                    })
                    .await?;
                // One rescan after all: one made between them would hold
                // the next ChainLock exception's scan below its ChainLock.
                // Until it is made, a sync commit leaves the rewind records
                // the abandons store.
                let _batch = this.hub.abandon_batch(id);
                let (mut done, mut lowest, mut failed) = (BTreeSet::new(), None::<u32>, None);
                for txid in candidates {
                    if done.contains(&txid) {
                        continue;
                    }
                    match this
                        .abandon_unrewound(&manager, id, txid, &mut lowest)
                        .await
                    {
                        Ok(set) => done.extend(set),
                        // Settled meanwhile: nothing to drop.
                        Err(EngineError::TxActionRefused(_)) => {}
                        Err(e) => {
                            failed = Some(e);
                            break;
                        }
                    }
                }
                if !done.is_empty() || lowest.is_some() {
                    this.finish_abandons(&manager, id, &done, lowest).await?;
                }
                dropped += done.len() as u32;
                if let Some(e) = failed {
                    return Err(e);
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
            chainlock_beaten: false,
            coinbase: false,
            abandoned: false,
            from_me: 1,
            credit: 0,
            debit: 0,
            debit_unknown: false,
            inputs_unresolved: false,
            matures_in: None,
        }
    }

    /// A mark whose txid suffix or value does not parse is skipped and
    /// makes the read incomplete; the other marks are still read.
    #[test]
    fn a_malformed_mark_makes_the_marks_unread() {
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
        let sent = Sent::from([(txid, BTreeSet::from([coin]))]);
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
            assert_eq!(load_sent(&db, wallet), (sent.clone(), false), "{key}");
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
        let beaten = Facts {
            chainlock_beaten: true,
            ..is
        };
        assert_eq!(beaten.abandon_refusal(), None);
        assert_eq!(
            beaten.resend_refusal(),
            Some(TxActionRefusal::InstantLocked)
        );
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
            facts(&entry, &view, &ChainProof::default()).abandon_refusal(),
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
