//! The R3 restore's money gate and holds (Q92 H1–H5).
//!
//! A restore leaves `upgrade-backups/RESTORED` (dw-store-upgrade). While it
//! is set, every funds action is refused ([`NetworkSession::refuse_while_restored`]).
//! At a complete history load (and after a truncation) while it is set,
//! each unmined own payment P of a wallet gets an app.sqlite row
//! `restore_hold:<P>` (wallet scope) naming the wallet's inputs it spends
//! (H3, H5). While the row exists and P stays unmined and not abandoned,
//! P's outputs to the wallet and those inputs are neither offered nor
//! counted as spendable (H1). The row is deleted in the same durable write
//! as whatever ends P: its block, a sweep, an abandon or a drop (H2). The
//! mark clears at the first all-Synced once a drop-dead pass started after
//! it has completed (H4).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dashcore::{OutPoint, Txid};
use dw_appdb::{AppDb, AppDbError};

use crate::history::WalletHistory;
use crate::{EngineError, NetworkSession, WalletId};

/// Settings key prefix (wallet scope) of a restore hold; the value is the
/// wallet's inputs of the payment, comma-separated outpoints.
pub(crate) const RESTORE_HOLD_PREFIX: &str = "restore_hold:";

/// A test's fault point on the holds' write.
#[cfg(all(test, debug_assertions))]
pub(crate) const BUILD_FAULT: &str = "restore_gate.build";

/// How often the gate's watcher looks again without a wake.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The network's RESTORED state: its mark, read where it lives.
#[derive(Default)]
pub(crate) struct RestoreGate {
    network_dir: OnceLock<PathBuf>,
    /// The mark's pending payments: H3's cross-check.
    pending: Mutex<Vec<dw_store_upgrade::PendingPayment>>,
    /// A Platform-address pass completed since the session opened.
    platform_pass: AtomicBool,
    /// A shielded pass completed since the session opened.
    shielded_pass: AtomicBool,
    pub(crate) wake: tokio::sync::Notify,
    /// Tests: what all-Synced reads, in place of the services.
    #[cfg(test)]
    pub(crate) all_synced: Mutex<Option<bool>>,
    /// Tests: holds the next drop-dead pass once it started.
    #[cfg(test)]
    pub(crate) pause_pass: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: the wallets and seeds each drop-dead pass ran with.
    #[cfg(test)]
    pub(crate) pass_runs: Mutex<Vec<(WalletId, BTreeSet<Txid>)>>,
    /// Tests: this many clears fail as the crate's clear does on an
    /// unreadable mark.
    #[cfg(test)]
    pub(crate) clear_fails: std::sync::atomic::AtomicU32,
}

impl RestoreGate {
    /// Whether the money gate is on: the network's restored mark is
    /// present, as `NetworkInfo.restored` reads it.
    pub(crate) fn is_restored(&self) -> bool {
        (self.network_dir.get()).is_some_and(|d| dw_store_upgrade::restored_mark_present(d))
    }

    /// Reads the network's mark; returns whether it is set. A mark whose
    /// pending list doesn't read keeps the gate on with no cross-check: the
    /// holds are the history's unmined own payments alone (Q102 102-3).
    fn init(&self, network_dir: &Path) -> bool {
        let _ = self.network_dir.set(network_dir.to_path_buf());
        if !self.is_restored() {
            return false;
        }
        let pending = match dw_store_upgrade::restored_mark(network_dir) {
            Ok(mark) => mark.map(|m| m.pending).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "restore: the restored mark's pending list does not read");
                Vec::new()
            }
        };
        *self.pending.lock().unwrap_or_else(|p| p.into_inner()) = pending;
        true
    }

    fn pending(&self) -> Vec<dw_store_upgrade::PendingPayment> {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub(crate) fn note_platform_pass(&self) {
        if !self.platform_pass.swap(true, Ordering::SeqCst) {
            self.wake.notify_one();
        }
    }

    pub(crate) fn note_shielded_pass(&self) {
        if !self.shielded_pass.swap(true, Ordering::SeqCst) {
            self.wake.notify_one();
        }
    }
}

fn encode(inputs: &BTreeSet<OutPoint>) -> String {
    (inputs.iter().map(ToString::to_string))
        .collect::<Vec<_>>()
        .join(",")
}

/// The restore holds of `wallet`, by payment, with the inputs each names.
/// A row whose key names no transaction is left out; one whose value
/// doesn't read holds the payment's outputs only.
pub(crate) fn read_holds(
    appdb: &AppDb,
    wallet: WalletId,
) -> Result<BTreeMap<Txid, BTreeSet<OutPoint>>, AppDbError> {
    let rows = appdb.settings_with_prefix_lossy(&wallet.to_string(), RESTORE_HOLD_PREFIX)?;
    Ok((rows.into_iter())
        .filter_map(|(key, value)| {
            let txid = Txid::from_str(key.get(RESTORE_HOLD_PREFIX.len()..)?).ok()?;
            let inputs = (value.unwrap_or_default().split(','))
                .filter_map(|o| OutPoint::from_str(o).ok())
                .collect();
            Some((txid, inputs))
        })
        .collect())
}

/// Settings key (wallet scope) of a wallet registered but not loaded when
/// the restored mark cleared: its next complete load builds its holds as
/// if the mark were still set, and ends it in the same write.
pub(crate) const RESTORE_BUILD_PENDING: &str = "restore_build_pending";

/// Whether `wallet`'s holds wait for its next complete load.
pub(crate) fn build_pending(appdb: &AppDb, wallet: WalletId) -> Result<bool, AppDbError> {
    Ok((appdb.setting_lossy(&wallet.to_string(), RESTORE_BUILD_PENDING)?).is_some())
}

/// The keys deleting the holds of `txids`.
pub(crate) fn hold_keys<'a>(txids: impl IntoIterator<Item = &'a Txid>) -> Vec<String> {
    (txids.into_iter())
        .map(|t| format!("{RESTORE_HOLD_PREFIX}{t}"))
        .collect()
}

impl WalletHistory {
    /// Whether the hold of `txid` holds anything: its payment is in no
    /// block and not abandoned. One the history lacks holds only while the
    /// history may be short (its load missed a record, or the holds didn't
    /// read).
    fn hold_in_effect(&self, txid: &Txid) -> bool {
        match self.txs.get(txid) {
            Some(e) => e.context.block_info().is_none() && !e.is_abandoned(),
            None => self.load_incomplete || self.restore_holds_unread,
        }
    }

    /// The wallet's own payments in no block and not abandoned, with the
    /// wallet's coins each spends: what a restore holds (H3).
    pub(crate) fn restore_hold_candidates(&self) -> BTreeMap<Txid, BTreeSet<OutPoint>> {
        (self.txs.iter())
            .filter(|(t, e)| {
                (!e.own_inputs.is_empty() || self.marked.contains_key(*t))
                    && e.context.block_info().is_none()
                    && !e.is_abandoned()
            })
            .map(|(t, e)| {
                let mut inputs: BTreeSet<OutPoint> = (e.own_inputs.keys())
                    .filter_map(|n| e.tx.input.get(*n as usize))
                    .map(|i| i.previous_output)
                    .collect();
                inputs.extend(self.marked.get(t).into_iter().flatten().copied());
                (*t, inputs)
            })
            .collect()
    }

    /// The holds in effect, with their inputs. Holds that didn't read hold
    /// every candidate (fail closed).
    pub(crate) fn restore_holds_in_effect(&self) -> BTreeMap<Txid, BTreeSet<OutPoint>> {
        if self.restore_holds_unread {
            return self.restore_hold_candidates();
        }
        (self.restore_holds.iter())
            .filter(|(t, _)| self.hold_in_effect(t))
            .map(|(t, i)| (*t, i.clone()))
            .collect()
    }

    /// The coins the holds in effect keep back: each held payment's
    /// outputs (by txid) and its inputs.
    pub(crate) fn restore_held_coins(&self) -> (HashSet<Txid>, HashSet<OutPoint>) {
        let held = self.restore_holds_in_effect();
        let inputs = held.values().flatten().copied().collect();
        (held.into_keys().collect(), inputs)
    }

    /// A bound on what the holds in effect keep out of the confirmed
    /// bucket, from the history alone (the exact figure is read from the
    /// manager's coins, [`SessionHub::restore_held_in`]): each held
    /// payment's outputs to the wallet that no live transaction of the
    /// history spends and that key-wallet may count confirmed. That is
    /// every such output of an InstantSend-locked payment, and the change of
    /// one whose inputs are all the wallet's (key-wallet's trusted change,
    /// whatever its parents).
    pub(crate) fn restore_held_value(&self) -> u64 {
        let held = self.restore_holds_in_effect();
        if held.is_empty() {
            return 0;
        }
        let spent: HashSet<OutPoint> = (self.txs.values())
            .filter(|e| !e.is_abandoned())
            .flat_map(|e| e.tx.input.iter().map(|i| i.previous_output))
            .collect();
        (held.keys())
            .filter_map(|t| Some((t, self.txs.get(t)?, self.own_outputs.get(t)?)))
            .map(|(t, e, outs)| {
                let locked = crate::history::instant_locked(e);
                let trusted = !e.tx.input.is_empty() && e.own_inputs.len() == e.tx.input.len();
                (outs.iter())
                    .filter(|(n, (_, change))| {
                        (locked || (trusted && *change)) && !spent.contains(&OutPoint::new(*t, **n))
                    })
                    .map(|(_, (v, _))| *v)
                    .sum::<u64>()
            })
            .sum()
    }
}

impl crate::events::SessionHub {
    /// What the restore holds keep out of the confirmed bucket of `core`,
    /// wallet `id`'s state in the manager: the held coins it counts there
    /// (key-wallet's `update_balance` rule).
    pub(crate) fn restore_held_in(
        &self,
        id: WalletId,
        core: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    ) -> u64 {
        let (txids, inputs) = self.history.read_wallet(&id, |h| h.restore_held_coins());
        if txids.is_empty() && inputs.is_empty() {
            return 0;
        }
        (core.accounts.all_funding_accounts().into_iter())
            .flat_map(|a| a.utxos.iter())
            .filter(|(o, u)| {
                (txids.contains(&o.txid) || inputs.contains(*o))
                    && !u.is_locked
                    && (u.is_confirmed || u.is_instantlocked || u.is_trusted)
            })
            .map(|(_, u)| u.txout.value)
            .sum()
    }

    /// Writes the holds H3 builds for `wallet` that don't exist yet, in one
    /// durable write: its candidates, and the restored mark's payments the
    /// history holds unmined and not abandoned (the cross-check). Returns
    /// whether the holds are all written.
    ///
    /// `at_load`: the build of a complete load, which alone counts as built
    /// (Q102 102-2) and ends a [`RESTORE_BUILD_PENDING`] mark in the same
    /// write; a truncation's (H5) only writes rows.
    pub(crate) fn build_restore_holds(
        &self,
        appdb: &AppDb,
        wallet: WalletId,
        at_load: bool,
    ) -> bool {
        let pending = self.restore.pending();
        let new = self.history.read_wallet(&wallet, |h| {
            let mut want = h.restore_hold_candidates();
            for p in &pending {
                match h.txs.get(&p.txid) {
                    Some(e) if e.context.block_info().is_none() && !e.is_abandoned() => {
                        want.entry(p.txid).or_default().extend(p.inputs.iter().copied());
                    }
                    Some(_) => {}
                    None => {
                        tracing::warn!(%wallet, txid = %p.txid, "restore: a pending payment of the restored set is not in the history")
                    }
                }
            }
            want.retain(|t, _| !h.restore_holds.contains_key(t));
            want
        });
        if !new.is_empty() || at_load {
            let rows: Vec<(String, String)> = (new.iter())
                .map(|(t, i)| (format!("{RESTORE_HOLD_PREFIX}{t}"), encode(i)))
                .collect();
            let writes: Vec<(&str, Option<&str>)> = (rows.iter())
                .map(|(k, v)| (k.as_str(), Some(v.as_str())))
                .chain(at_load.then_some((RESTORE_BUILD_PENDING, None)))
                .collect();
            #[cfg(all(test, debug_assertions))]
            let fault = appdb.fault_point(BUILD_FAULT, dw_appdb::fault::Stage::Before);
            #[cfg(not(all(test, debug_assertions)))]
            let fault = Ok(());
            if let Err(e) =
                fault.and_then(|()| appdb.set_settings_durable(&wallet.to_string(), &writes))
            {
                tracing::warn!(%wallet, error = %e, "restore: the holds were not written");
                return false;
            }
            tracing::info!(%wallet, held = new.len(), "restore: pending payments held");
        }
        self.history.with_wallet(wallet, |h| {
            h.restore_holds.extend(new);
            h.restore_holds_built |= at_load;
            h.restore_holds_unread &= !at_load;
        });
        self.refresh_restore_held(wallet);
        true
    }

    /// Deletes the holds of `wallet`'s payments now in a block, durably,
    /// after their records' update (H2).
    pub(crate) fn settle_mined_restore_holds(&self, wallet: WalletId) {
        let mined: Vec<Txid> = self.history.read_wallet(&wallet, |h| {
            (h.restore_holds.keys())
                .filter(|t| {
                    h.txs
                        .get(*t)
                        .is_some_and(|e| e.context.block_info().is_some())
                })
                .copied()
                .collect()
        });
        if mined.is_empty() {
            return;
        }
        let Some(appdb) = self.appdb.get().and_then(std::sync::Weak::upgrade) else {
            return;
        };
        let keys = hold_keys(&mined);
        let writes: Vec<(&str, Option<&str>)> = keys.iter().map(|k| (k.as_str(), None)).collect();
        match appdb.set_settings_durable(&wallet.to_string(), &writes) {
            Ok(()) => self.forget_restore_holds(wallet, &mined),
            Err(e) => {
                tracing::warn!(%wallet, error = %e, "restore: the holds of mined payments were not deleted")
            }
        }
    }

    /// Drops `txids`' holds from memory once their rows are deleted.
    pub(crate) fn forget_restore_holds<'a>(
        &self,
        wallet: WalletId,
        txids: impl IntoIterator<Item = &'a Txid>,
    ) {
        let changed = self.history.with_wallet(wallet, |h| {
            let mut changed = false;
            for t in txids {
                changed |= h.restore_holds.remove(t).is_some();
            }
            changed
        });
        if changed {
            self.refresh_restore_held(wallet);
        }
    }

    /// A truncation while the mark is set builds the holds again (H5).
    pub(crate) fn rebuild_restore_holds_after_truncation(&self, wallet: WalletId) {
        if !self.restore.is_restored() {
            return;
        }
        if let Some(appdb) = self.appdb.get().and_then(std::sync::Weak::upgrade) {
            self.build_restore_holds(&appdb, wallet, false);
        }
    }
}

impl NetworkSession {
    /// The money gate: every funds action is refused while the network's
    /// restored mark is set.
    pub(crate) fn refuse_while_restored(&self) -> Result<(), EngineError> {
        match self.hub.restore.is_restored() {
            true => Err(EngineError::Restored),
            false => Ok(()),
        }
    }

    /// Whether the network's restored mark is set (the money gate).
    #[cfg(test)]
    pub(crate) fn restored(&self) -> bool {
        self.hub.restore.is_restored()
    }

    /// Reads the network's mark before the history loads and, when it is
    /// set, starts the watcher that clears it (H4).
    pub(crate) fn attach_restore_gate(self: &Arc<Self>) {
        if !self.hub.restore.init(self.data_dir()) {
            return;
        }
        tracing::info!("restore: the money gate is on until the first full sync");
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            loop {
                let wake = {
                    let Some(session) = weak.upgrade() else {
                        return;
                    };
                    match session.try_clear_restored().await {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(e) => tracing::warn!(error = %e, "restore: the mark did not clear"),
                    }
                    let hub = Arc::clone(&session.hub);
                    drop(session);
                    hub
                };
                let _ = tokio::time::timeout(WATCH_EVERY, wake.restore.wake.notified()).await;
            }
        });
    }

    /// Core, Platform and Shielded synced, a service that is off counting
    /// as synced, and every loaded wallet's holds built.
    async fn restore_all_synced(&self) -> Result<bool, EngineError> {
        let manager = self.manager()?;
        let ids = manager.list_wallet_ids_blocking();
        let built = ids
            .iter()
            .all(|id| (self.hub.history).read_wallet(&WalletId(*id), |h| h.restore_holds_built));
        #[cfg(test)]
        if let Some(synced) = *self.hub.restore.all_synced.lock().unwrap() {
            return Ok(synced && built);
        }
        let gate = &self.hub.restore;
        let (core, masternodes) = {
            let tracker = self.hub.tracker();
            (tracker.caught_up(), tracker.masternodes_synced())
        };
        let platform =
            !self.platform.enabled() || (masternodes && gate.platform_pass.load(Ordering::SeqCst));
        let shielded = !self.platform.enabled()
            || gate.shielded_pass.load(Ordering::SeqCst)
            || !crate::platform::shielded_pool::runs_sync(&manager).await;
        Ok(built && core && platform && shielded)
    }

    /// H4: at all-Synced, runs one drop-dead pass over every loaded wallet
    /// and then clears the mark. Returns whether it cleared.
    pub(crate) async fn try_clear_restored(self: &Arc<Self>) -> Result<bool, EngineError> {
        if !self.hub.restore.is_restored() || !self.restore_all_synced().await? {
            return Ok(false);
        }
        let _op = self.enter().await?;
        let manager = self.manager()?;
        {
            let _lifecycle = self.wallet_lifecycle.hold().await;
            #[cfg(test)]
            crate::coins::pause_at(&self.hub.restore.pause_pass).await;
            let live = self.live()?;
            let db_path = self.data_dir().join(crate::session::WALLET_DB_FILE);
            let wallets: Vec<WalletId> = (manager.list_wallet_ids_blocking().into_iter())
                .map(WalletId)
                .collect();
            let appdb = Arc::clone(&live.appdb);
            let seeds = tokio::task::spawn_blocking(move || {
                let persisted = crate::history_ops::persisted_txids(&db_path);
                (wallets.into_iter())
                    .map(|w| {
                        let (here, read) = crate::tx_actions::load_dropped(&appdb, w);
                        let stored = match &persisted {
                            Ok(p) => p.get(&w.0).map(|p| {
                                (p.dropped.iter())
                                    .filter(|t| !p.dropped_doubtful.contains(*t))
                                    .copied()
                                    .collect::<BTreeSet<Txid>>()
                            }),
                            Err(_) => None,
                        };
                        let complete = read && persisted.is_ok();
                        (
                            w,
                            here.into_iter()
                                .chain(stored.into_iter().flatten())
                                .collect(),
                            complete,
                        )
                    })
                    .collect::<Vec<(WalletId, BTreeSet<Txid>, bool)>>()
            })
            .await
            .map_err(|e| EngineError::Internal(format!("restore: drop-dead seeds: {e}")))?;
            if let Some((w, _, _)) = seeds.iter().find(|(_, _, complete)| !complete) {
                return Err(EngineError::Storage(format!(
                    "restore: the dropped records of wallet {w} did not read"
                )));
            }
            for (wallet, dead, _) in seeds {
                if !dead.is_empty() {
                    self.abandon_on_dead(&manager, wallet, &dead, false).await;
                    #[cfg(test)]
                    (self.hub.restore.pass_runs.lock().unwrap()).push((wallet, dead));
                }
            }
            if !self.restore_all_synced().await? {
                return Ok(false);
            }
            let unloaded: Vec<WalletId> = live.store.unloaded().into_iter().map(WalletId).collect();
            if !unloaded.is_empty() {
                let appdb = Arc::clone(&live.appdb);
                tokio::task::spawn_blocking(move || {
                    (unloaded.iter()).try_for_each(|w| {
                        appdb.set_settings_durable(
                            &w.to_string(),
                            &[(RESTORE_BUILD_PENDING, Some(""))],
                        )
                    })
                })
                .await
                .map_err(|e| EngineError::Internal(format!("restore: build marks: {e}")))?
                .map_err(|e| EngineError::Storage(format!("restore: build marks: {e}")))?;
            }
            #[cfg(test)]
            let fault = (self.hub.restore.clear_fails)
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok();
            #[cfg(not(test))]
            let fault = false;
            let cleared = match fault {
                true => Err(EngineError::Storage(
                    "restore: the mark did not read".into(),
                )),
                false => dw_store_upgrade::clear_restored_mark_on_first_all_synced(
                    &live._persister_opened,
                )
                .map_err(|e| EngineError::Storage(e.to_string())),
            };
            cleared?;
        }
        self.leases.request_resume(None);
        self.hub.pump.mark_sync();
        for id in manager.list_wallet_ids_blocking() {
            self.hub.pump.mark_balances(WalletId(id));
        }
        tracing::info!("restore: the money gate is off");
        Ok(true)
    }
}
