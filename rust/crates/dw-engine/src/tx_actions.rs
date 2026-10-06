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

/// Settings key prefix (wallet scope) of an abandoned transaction; the
/// value is its consensus hex.
const ABANDONED_PREFIX: &str = "abandoned:";

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

/// The abandoned transactions of `wallet` recorded in the app database.
pub(crate) fn load_abandoned(appdb: &AppDb, wallet: WalletId) -> Vec<(Txid, Transaction)> {
    let rows = appdb
        .settings_with_prefix(&wallet.to_string(), ABANDONED_PREFIX)
        .unwrap_or_else(|e| {
            tracing::warn!(%wallet, error = %e, "could not read abandoned transactions");
            Vec::new()
        });
    rows.into_iter()
        .filter_map(|(key, hex)| {
            let txid = Txid::from_str(&key[ABANDONED_PREFIX.len()..]).ok()?;
            let bytes = hex::decode(hex).ok()?;
            let tx: Transaction = deserialize(&bytes).ok()?;
            (tx.txid() == txid).then_some((txid, tx))
        })
        .collect()
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
        abandoned: entry.abandoned && depth == 0,
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
    pub async fn abandon_transaction(
        self: &Arc<Self>,
        wallet: WalletId,
        txid: Txid,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
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
    async fn abandon_inner(
        &self,
        manager: &Manager,
        wallet: WalletId,
        root: Txid,
    ) -> Result<BTreeSet<Txid>, EngineError> {
        let outcome = {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm
                .get_wallet_info_mut(&wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(wallet.to_string()))?;
            let outcome = info
                .core_wallet
                .abandon_transaction_with_spends(root, &BTreeMap::new());
            if !outcome.abandoned.is_empty() {
                info.core_wallet.update_balance();
            }
            outcome
        };
        // key-wallet refuses (returns nothing) when it holds the root as
        // settled.
        let abandoned = outcome.abandoned;
        if abandoned.is_empty() {
            return Err(EngineError::TxActionRefused(TxActionRefusal::Confirmed));
        }
        let txs: Vec<(Txid, Transaction)> = abandoned
            .iter()
            .filter_map(|t| self.hub.history.get(&wallet, t).map(|e| (*t, e.tx)))
            .collect();
        let mut released: Vec<OutPoint> = txs
            .iter()
            .flat_map(|(_, tx)| tx.input.iter().map(|i| i.previous_output))
            .filter(|o| !abandoned.contains(&o.txid))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        released.sort();

        let live = self.live()?;
        let persister = Arc::clone(&live.persister);
        let sweep = SweepBatch {
            txids: abandoned.iter().copied().collect(),
            // Every input is released below, so no spend claim is attributed
            // to this txid; the field only names the batch.
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
        self.spends.remove(&wallet, released.iter().copied());

        // The released coins come back when the filter scan passes their
        // funding blocks again.
        let rescan_from = released
            .iter()
            .filter_map(|o| self.hub.history.get(&wallet, &o.txid))
            .filter_map(|e| e.context.block_info().map(|b| b.height()))
            .min();
        if let Some(height) = rescan_from {
            let m = self.manager()?;
            tokio::task::spawn_blocking(move || {
                m.spv_rescan_filters_blocking(&wallet.0, height.saturating_sub(1))
            })
            .await?;
        }
        // platform-wallet's balance mirror is only refreshed by wallet
        // events, so the hub reads key-wallet's recomputed balance.
        self.refresh_wallet_state_from_core(manager, wallet).await;
        let changed: Vec<Txid> = abandoned.iter().copied().collect();
        self.hub.pump.mark_history(wallet, Some(&changed));
        self.hub.pump.mark_balances(wallet);
        Ok(abandoned)
    }

    /// dash-qt "Resend transaction" (QT-091): hands the stored transaction
    /// to the SPV client again and returns; the announcement's outcome is
    /// only logged (there is no verdict).
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
            this.hub.note_announced(txid);
            tokio::spawn(async move {
                match handle.core().broadcast_transaction(&entry.tx).await {
                    Ok(_) => tracing::info!(%txid, "resent transaction accepted"),
                    Err(e) => tracing::info!(%txid, error = %e, "resent transaction: no acceptance seen"),
                }
            });
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

    #[test]
    fn test_qt_091_abandon_and_resend_enablement() {
        assert_eq!(f().abandon_refusal(), None);
        assert_eq!(f().resend_refusal(), None);
        let confirmed = Facts { depth: 1, ..f() };
        assert_eq!(confirmed.abandon_refusal(), Some(TxActionRefusal::Confirmed));
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
        assert_eq!(gone.abandon_refusal(), Some(TxActionRefusal::AlreadyAbandoned));
        assert_eq!(gone.resend_refusal(), Some(TxActionRefusal::AlreadyAbandoned));
        let incoming = Facts { from_me: 0, ..f() };
        assert_eq!(incoming.abandon_refusal(), None, "dash-qt abandons any unconfirmed");
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

    #[test]
    fn test_qt_033_coinjoin_internal_types() {
        assert!(is_coinjoin_internal(TxType::CoinJoinMixing));
        assert!(is_coinjoin_internal(TxType::CoinJoinCreateDenominations));
        assert!(!is_coinjoin_internal(TxType::CoinJoinSend));
        assert!(!is_coinjoin_internal(TxType::RecvWithAddress));
    }
}
