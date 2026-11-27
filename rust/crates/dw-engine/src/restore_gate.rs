//! The R3 restore's money gate and holds (Q92 H1–H5).
//!
//! A restore leaves `upgrade-backups/RESTORED` (dw-store-upgrade). While it
//! is set, every funds action is refused ([`NetworkSession::refuse_while_restored`]).
//! At a complete history load (and after a truncation) while it is set,
//! each unmined own payment P of a wallet gets an app.sqlite row
//! `restore_hold:<P>` (wallet scope) naming the wallet's inputs it spends
//! (H3, H5). While the row exists and P stays unmined and not abandoned,
//! P's outputs to the wallet and those inputs are neither offered nor
//! counted as spendable (H1). A block that mines P deletes the row after
//! its records' update; a sweep that beats P and the retire of a dropped P
//! delete it in their own write. An abandon (Resolve stuck funding's of
//! its own locks too) and the drop-dead pass mark P abandoned and leave
//! the row: a row on an abandoned payment holds nothing and may stay,
//! so an abandon key-wallet refuses has no hold to put back (H2). The
//! mark clears at the first all-Synced once a drop-dead pass started after
//! it has completed (H4).
//!
//! A mark cleared in a session without Platform leaves
//! `upgrade-backups/PLATFORM_PENDING` (C6), written before the mark goes:
//! while it is present, funds actions that need Platform are refused
//! (`platform.restored`), and the asset-lock resume and the masternode
//! withdrawals' recheck wait, until a session with Platform has checked it
//! and run its own drop-dead pass. Core is not gated by it.
//!
//! When the gate opens in a session with Platform, every wallet of the
//! network's wallet list that Platform has not checked since its load
//! (closed, or loaded after the passes) gets an app.sqlite row
//! `restore_platform_pending` (wallet scope), written before the mark or
//! the residue goes: while it is present, or does not read, that wallet's
//! Platform funds wait the same way, until a session with Platform has
//! checked the wallet after its load and run its drop-dead pass.

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

/// A test's fault point on the Platform rows' write.
#[cfg(all(test, debug_assertions))]
pub(crate) const PLATFORM_ROWS_FAULT: &str = "restore_gate.platform_rows";

/// How often the gate's watcher looks again without a wake.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// The longest a failed clear waits before the next attempt.
const CLEAR_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// The Platform-pending residue, beside the restored mark.
const PLATFORM_PENDING: &str = "PLATFORM_PENDING";

fn residue_path(network_dir: &Path) -> PathBuf {
    (network_dir.join(dw_store_upgrade::UPGRADE_BACKUPS_DIR)).join(PLATFORM_PENDING)
}

/// Makes the directory's entries durable.
fn sync_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// What a restored network waits for before its money gate opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreStatus {
    /// Empty: the clear runs now or at the watcher's next look.
    pub waiting: Vec<RestoreWait>,
    /// The loaded wallets whose history load has not built their holds.
    pub history_wallets: Vec<WalletId>,
    /// The last attempt to clear failed (the mark did not read or write,
    /// the dropped records did not read, or the session was closing). It
    /// is tried again after a growing delay, at once on
    /// [`NetworkSession::restore_retry`].
    pub clear_failed: bool,
    /// Core's gate is open: only Platform's check is waited for, after a
    /// Platform-off finish or of a loaded wallet Platform has not checked
    /// since its load.
    pub platform_only: bool,
    /// This session runs without Platform.
    pub platform_off: bool,
}

/// One thing a restored network waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreWait {
    /// Core has not caught up with its peers.
    CoreSync,
    /// Platform (its masternode list, a clean Platform-address pass and the
    /// shielded pass) has not been checked since the network opened.
    PlatformPass,
    /// A loaded wallet's history has not loaded.
    HistoryLoad,
    /// No wallet is loaded (a recovery's new folder before its wallets
    /// are restored, or every wallet closed): the gate waits for one.
    Wallet,
}

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
    /// The wallets a Platform-address pass synced cleanly since their load.
    wallet_platform_pass: Mutex<HashSet<WalletId>>,
    /// The wallets a shielded pass synced, or found without a shielded
    /// account, since their load.
    wallet_shielded_pass: Mutex<HashSet<WalletId>>,
    /// The wallets whose next Platform-address pass result does not count:
    /// loaded while a pass that may have started before was in flight.
    wallet_skip_platform_pass: Mutex<HashSet<WalletId>>,
    /// What waited for the gate was let run in this session.
    opened: AtomicBool,
    pub(crate) wake: tokio::sync::Notify,
    /// The last clear failed.
    clear_failed: AtomicBool,
    /// A retry asked the watcher to try the clear now.
    retry: AtomicBool,
    /// The status the host last read about: a change marks sync.
    published: Mutex<Option<RestoreStatus>>,
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
    /// Tests: how many clears were tried.
    #[cfg(test)]
    pub(crate) clear_attempts: std::sync::atomic::AtomicU32,
    /// Tests: every backoff after a failed clear, in place of the growing one.
    #[cfg(test)]
    pub(crate) backoff: Mutex<Option<Duration>>,
    /// Tests: what Core's sync (and its masternode list) reads, in place of
    /// the tracker.
    #[cfg(test)]
    pub(crate) core_synced: Mutex<Option<bool>>,
    /// Tests: the gate as the masternode withdrawals' recheck read it when
    /// it started.
    #[cfg(test)]
    pub(crate) restored_at_recheck_start: Mutex<Option<bool>>,
    /// Tests: how many masternode withdrawal rechecks ran.
    #[cfg(test)]
    pub(crate) recheck_runs: std::sync::atomic::AtomicU32,
    /// Tests: how many times what waited for the gate was let run.
    #[cfg(test)]
    pub(crate) cleared_runs: std::sync::atomic::AtomicU32,
    /// Tests: this many writes of the Platform residue fail.
    #[cfg(test)]
    pub(crate) residue_fails: std::sync::atomic::AtomicU32,
    /// Tests: this many removals of the Platform residue fail.
    #[cfg(test)]
    pub(crate) residue_remove_fails: std::sync::atomic::AtomicU32,
    /// Tests: every wallet's Platform row reads as an error.
    #[cfg(test)]
    pub(crate) rows_unreadable: AtomicBool,
    /// Tests: the network's wallet list reads as an error.
    #[cfg(test)]
    pub(crate) wallet_list_unreadable: AtomicBool,
}

impl RestoreGate {
    /// Whether the money gate is on: the network's restored mark is
    /// present, as `NetworkInfo.restored` reads it. On before the
    /// network's dir is set.
    pub(crate) fn is_restored(&self) -> bool {
        (self.network_dir.get()).is_none_or(|d| dw_store_upgrade::restored_mark_present(d))
    }

    /// Whether the Platform-pending residue is present: a mark cleared
    /// without Platform. Present before the network's dir is set and when
    /// it does not read.
    pub(crate) fn platform_pending(&self) -> bool {
        (self.network_dir.get()).is_none_or(|d| {
            !std::fs::symlink_metadata(residue_path(d))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        })
    }

    /// Funds actions that need Platform wait: the mark or the residue.
    pub(crate) fn platform_held(&self) -> bool {
        self.is_restored() || self.platform_pending()
    }

    /// Writes the residue durably, its directory's entry included.
    fn write_residue(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if (self.residue_fails)
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(std::io::Error::other("the residue did not write"));
        }
        let dir = self
            .network_dir
            .get()
            .ok_or_else(|| std::io::Error::other("the network's dir is not set"))?;
        let path = residue_path(dir);
        let mut file = std::fs::File::create(&path)?;
        std::io::Write::write_all(&mut file, b"platform-pending\n")?;
        file.sync_all()?;
        sync_dir(path.parent().unwrap_or(dir))
    }

    /// Removes the residue durably; one already gone is no error.
    fn remove_residue(&self) -> std::io::Result<()> {
        #[cfg(test)]
        if (self.residue_remove_fails)
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(std::io::Error::other("the residue did not clear"));
        }
        let dir = self
            .network_dir
            .get()
            .ok_or_else(|| std::io::Error::other("the network's dir is not set"))?;
        let path = residue_path(dir);
        match std::fs::remove_file(&path) {
            Ok(()) => sync_dir(path.parent().unwrap_or(dir)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Where the network's mark lives; set as the session opens, before
    /// anything that reads the gate starts.
    pub(crate) fn set_dir(&self, network_dir: &Path) {
        let _ = self.network_dir.set(network_dir.to_path_buf());
    }

    /// How long the watcher waits after `failures` failed clears in a row.
    fn backoff(&self, failures: u32) -> Duration {
        if failures == 0 {
            return Duration::ZERO;
        }
        #[cfg(test)]
        if let Some(backoff) = *self.backoff.lock().unwrap() {
            return backoff;
        }
        clear_backoff(failures)
    }

    /// Reads the network's mark; returns whether it is set. A mark whose
    /// pending list doesn't read keeps the gate on with no cross-check: the
    /// holds are the history's unmined own payments alone (Q102 102-3).
    fn init(&self, network_dir: &Path) -> bool {
        self.set_dir(network_dir);
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

    /// The wallets a Platform-address pass synced cleanly.
    pub(crate) fn note_wallet_platform_pass(&self, wallets: impl IntoIterator<Item = WalletId>) {
        {
            let mut skip =
                (self.wallet_skip_platform_pass.lock()).unwrap_or_else(|p| p.into_inner());
            let mut passed = (self.wallet_platform_pass.lock()).unwrap_or_else(|p| p.into_inner());
            for wallet in wallets {
                if !skip.remove(&wallet) {
                    passed.insert(wallet);
                }
            }
        }
        self.wake.notify_one();
    }

    /// The wallets a shielded pass synced or found without a shielded
    /// account.
    pub(crate) fn note_wallet_shielded_pass(&self, wallets: impl IntoIterator<Item = WalletId>) {
        (self
            .wallet_shielded_pass
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .extend(wallets);
        self.wake.notify_one();
    }

    /// A wallet loads, closes or goes: only a pass after its load checks
    /// it. `pass_in_flight`: a Platform-address pass may have started
    /// before, so its result does not count.
    pub(crate) fn forget_wallet_checks(&self, wallet: WalletId, pass_in_flight: bool) {
        (self
            .wallet_platform_pass
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .remove(&wallet);
        (self
            .wallet_shielded_pass
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .remove(&wallet);
        let mut skip = (self.wallet_skip_platform_pass.lock()).unwrap_or_else(|p| p.into_inner());
        match pass_in_flight {
            true => skip.insert(wallet),
            false => skip.remove(&wallet),
        };
    }

    fn wallet_passes(&self, wallet: WalletId) -> (bool, bool) {
        (
            (self
                .wallet_platform_pass
                .lock()
                .unwrap_or_else(|p| p.into_inner()))
            .contains(&wallet),
            (self
                .wallet_shielded_pass
                .lock()
                .unwrap_or_else(|p| p.into_inner()))
            .contains(&wallet),
        )
    }

    /// Whether `wallet`'s Platform row is present; one that does not read
    /// is.
    pub(crate) fn row_present(&self, appdb: &AppDb, wallet: WalletId) -> bool {
        let read = appdb.setting(&wallet.to_string(), RESTORE_PLATFORM_PENDING);
        #[cfg(test)]
        let read = match self.rows_unreadable.load(Ordering::SeqCst) {
            true => Err(AppDbError::Corrupt("the Platform row".into())),
            false => read,
        };
        !matches!(read, Ok(None))
    }
}

/// The growing delay after `failures` failed clears: twice the watcher's
/// look per failure, up to [`CLEAR_BACKOFF_MAX`].
pub(crate) fn clear_backoff(failures: u32) -> Duration {
    WATCH_EVERY
        .saturating_mul(1 << failures.min(5))
        .min(CLEAR_BACKOFF_MAX)
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

/// Settings key (wallet scope) of a wallet whose Platform funds wait for a
/// check after a restore: one Platform had not checked since its load when
/// the gate opened in a session with Platform (closed, or loaded after the
/// passes). It goes once a session with Platform has checked the wallet
/// after its load and run its drop-dead pass.
pub(crate) const RESTORE_PLATFORM_PENDING: &str = "restore_platform_pending";

/// The keys deleting the holds of `txids`.
pub(crate) fn hold_keys<'a>(txids: impl IntoIterator<Item = &'a Txid>) -> Vec<String> {
    (txids.into_iter())
        .map(|t| format!("{RESTORE_HOLD_PREFIX}{t}"))
        .collect()
}

impl WalletHistory {
    /// Whether the hold of `txid` holds anything: its payment is in no
    /// block and not abandoned (a row on an abandoned payment may stay,
    /// H2). One the history lacks holds only while the
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
    /// (key-wallet's `update_balance` rule), less those `excluded` already
    /// keeps out of every bucket (`crate::withhold`).
    pub(crate) fn restore_held_in(
        &self,
        id: WalletId,
        core: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
        excluded: &crate::withhold::Excluded,
    ) -> u64 {
        let (txids, inputs) = self.history.read_wallet(&id, |h| h.restore_held_coins());
        if txids.is_empty() && inputs.is_empty() {
            return 0;
        }
        (core.accounts.all_funding_accounts().into_iter())
            .flat_map(|a| a.utxos.iter())
            .filter(|(o, u)| {
                (txids.contains(&o.txid) || inputs.contains(*o))
                    && !excluded.coins.contains_key(*o)
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

    /// A funds action of `wallet` that needs Platform, before any grant,
    /// lease, intent, journal row or signature: `platform.unavailable` in
    /// a session without Platform (C7), else `platform.restored` while the
    /// restored mark, its Platform residue or the wallet's Platform row is
    /// present (C6).
    pub(crate) fn refuse_platform_funds(
        &self,
        wallet: WalletId,
    ) -> Result<(), crate::platform::PlatformError> {
        if self.platform.funds_off() {
            return Err(crate::platform::PlatformError::Unavailable);
        }
        match self.wallet_platform_held(wallet) {
            true => Err(crate::platform::PlatformError::Restored),
            false => Ok(()),
        }
    }

    /// A wallet added in the session (imported, restored from its phrase,
    /// created, watch-only): while the mark holds, its history loads so its
    /// first complete load builds its holds (H3) and wakes the watcher; a
    /// failed load leaves them unbuilt and the mark held. With the residue
    /// alone the watcher looks again.
    pub(crate) async fn load_added_wallet(&self, wallet_id: WalletId) {
        let gate = &self.hub.restore;
        if gate.is_restored() {
            if let Err(e) = self.load_history_for(vec![wallet_id]).await {
                tracing::warn!(%wallet_id, error = %e, "restore: the added wallet's history did not load; the mark stays");
            }
        } else if gate.platform_pending() {
            gate.wake.notify_one();
        }
    }

    /// Whether `wallet`'s Platform funds wait: the mark, the residue, or
    /// the wallet's own Platform row (present when it does not read).
    pub(crate) fn wallet_platform_held(&self, wallet: WalletId) -> bool {
        let gate = &self.hub.restore;
        gate.platform_held()
            || self
                .live()
                .map_or(true, |live| gate.row_present(&live.appdb, wallet))
    }

    /// Whether any loaded wallet's Platform funds wait.
    pub(crate) fn any_platform_held(&self) -> bool {
        let gate = &self.hub.restore;
        if gate.platform_held() {
            return true;
        }
        let Ok(live) = self.live() else {
            return true;
        };
        (live.manager.list_wallet_ids_blocking().into_iter())
            .any(|w| gate.row_present(&live.appdb, WalletId(w)))
    }

    /// Every wallet of the network's wallet list (wallet.sqlite, not the
    /// loaded set): an error when it does not read.
    async fn registered_wallets(&self) -> Result<Vec<WalletId>, EngineError> {
        #[cfg(test)]
        if (self.hub.restore.wallet_list_unreadable).load(Ordering::SeqCst) {
            return Err(EngineError::Storage(
                "restore: the wallet list did not read".into(),
            ));
        }
        let db_path = self.data_dir().join(crate::session::WALLET_DB_FILE);
        let ids = tokio::task::spawn_blocking(move || {
            std::fs::symlink_metadata(&db_path).map_err(|e| e.to_string())?;
            crate::store::registered_wallet_ids(&db_path).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| EngineError::Internal(format!("restore: the wallet list: {e}")))?
        .map_err(|e| EngineError::Storage(format!("restore: the wallet list: {e}")))?;
        Ok(ids.into_iter().map(WalletId).collect())
    }

    /// Whether a wallet of the network's list has its Platform row.
    async fn rows_remain(&self) -> Result<bool, EngineError> {
        let appdb = self.live()?.appdb;
        let gate = &self.hub.restore;
        Ok((self.registered_wallets().await?.into_iter()).any(|w| gate.row_present(&appdb, w)))
    }

    /// Whether Platform has checked loaded `wallet` since its load: the
    /// masternode list synced, a clean Platform-address pass and the
    /// shielded pass of it (or no shielded sync to run).
    async fn wallet_checked(&self, manager: &crate::session::Manager, wallet: WalletId) -> bool {
        let gate = &self.hub.restore;
        #[cfg(test)]
        if let Some(synced) = *gate.all_synced.lock().unwrap() {
            return synced;
        }
        let masternodes = self.hub.tracker().masternodes_synced();
        #[cfg(test)]
        let masternodes = (*gate.core_synced.lock().unwrap()).unwrap_or(masternodes);
        let (platform, shielded) = gate.wallet_passes(wallet);
        self.platform.enabled()
            && masternodes
            && platform
            && (shielded || !crate::platform::shielded_pool::runs_sync(manager).await)
    }

    /// Writes the Platform row of every wallet of the network's list that
    /// Platform has not checked since its load, durably in one write.
    async fn write_platform_rows(
        &self,
        manager: &crate::session::Manager,
        appdb: &Arc<AppDb>,
    ) -> Result<(), EngineError> {
        let loaded: HashSet<WalletId> = (manager.list_wallet_ids_blocking().into_iter())
            .map(WalletId)
            .collect();
        let mut unchecked = Vec::new();
        for wallet in self.registered_wallets().await? {
            if !(loaded.contains(&wallet) && self.wallet_checked(manager, wallet).await) {
                unchecked.push(wallet.to_string());
            }
        }
        if unchecked.is_empty() {
            return Ok(());
        }
        let appdb = Arc::clone(appdb);
        tokio::task::spawn_blocking(move || {
            #[cfg(all(test, debug_assertions))]
            appdb.fault_point(PLATFORM_ROWS_FAULT, dw_appdb::fault::Stage::Before)?;
            let writes: Vec<(&str, &str, Option<&str>)> = (unchecked.iter())
                .map(|w| (w.as_str(), RESTORE_PLATFORM_PENDING, Some("")))
                .collect();
            appdb.set_scoped_settings_durable(&writes)
        })
        .await
        .map_err(|e| EngineError::Internal(format!("restore: Platform rows: {e}")))?
        .map_err(|e| EngineError::Storage(format!("restore: Platform rows: {e}")))
    }

    /// Whether the network's restored mark is set (the money gate).
    #[cfg(test)]
    pub(crate) fn restored(&self) -> bool {
        self.hub.restore.is_restored()
    }

    /// Reads the network's mark before the history loads and, when it is
    /// set, or its Platform residue is in a session with Platform, starts
    /// the watcher that clears them (H4, C6). A failed clear is tried again
    /// after a growing delay, or at once on a retry; meanwhile the watcher
    /// still publishes what the gate waits for.
    pub(crate) fn attach_restore_gate(self: &Arc<Self>) {
        let gate = &self.hub.restore;
        if gate.init(self.data_dir()) {
            tracing::info!("restore: the money gate is on until the first full sync");
        } else if self.platform.enabled() && gate.platform_pending() {
            tracing::info!("restore: Platform funds wait for the first Platform check");
        } else if self.platform.enabled() && self.platform_rows_at_open() {
            gate.opened.store(true, Ordering::SeqCst);
            tracing::info!("restore: a wallet's Platform funds wait for its Platform check");
        } else {
            return;
        }
        let weak = Arc::downgrade(self);
        self.rt.spawn(async move {
            let mut failures = 0u32;
            let mut next_try = tokio::time::Instant::now();
            loop {
                let (hub, wait) = {
                    let Some(session) = weak.upgrade() else {
                        return;
                    };
                    let gate = &session.hub.restore;
                    if gate.retry.swap(false, Ordering::SeqCst) {
                        next_try = tokio::time::Instant::now();
                    }
                    if tokio::time::Instant::now() >= next_try {
                        match session.try_clear_restored().await {
                            Ok(true) => {
                                gate.clear_failed.store(false, Ordering::SeqCst);
                                return;
                            }
                            Ok(false) => failures = 0,
                            Err(e) => {
                                failures = failures.saturating_add(1);
                                tracing::warn!(error = %e, failures, "restore: the mark did not clear");
                            }
                        }
                        gate.clear_failed.store(failures > 0, Ordering::SeqCst);
                        next_try = tokio::time::Instant::now() + gate.backoff(failures);
                    }
                    session.publish_restore_status().await;
                    let left = next_try.saturating_duration_since(tokio::time::Instant::now());
                    let wait = match failures {
                        0 => WATCH_EVERY,
                        _ => WATCH_EVERY.min(left),
                    };
                    (Arc::clone(&session.hub), wait)
                };
                let _ = tokio::time::timeout(wait, hub.restore.wake.notified()).await;
            }
        });
    }

    /// Whether a wallet of the network's list has its Platform row as the
    /// session opens; yes when the list or the store does not read.
    fn platform_rows_at_open(&self) -> bool {
        #[cfg(test)]
        if (self.hub.restore.wallet_list_unreadable).load(Ordering::SeqCst) {
            return true;
        }
        let Ok(live) = self.live() else {
            return true;
        };
        let db_path = self.data_dir().join(crate::session::WALLET_DB_FILE);
        if std::fs::symlink_metadata(&db_path).is_err() {
            return true;
        }
        match crate::store::registered_wallet_ids(&db_path) {
            Ok(ids) => {
                (ids.into_iter()).any(|w| self.hub.restore.row_present(&live.appdb, WalletId(w)))
            }
            Err(_) => true,
        }
    }

    /// What the money gate waits for; `None` when it is off. With the
    /// mark gone and the Platform residue present: `platform_only`, waiting
    /// for `PlatformPass` until a session with Platform has checked it (and
    /// `Wallet` while none is loaded).
    pub async fn restore_status(&self) -> Result<Option<RestoreStatus>, EngineError> {
        let gate = &self.hub.restore;
        let clear_failed = gate.clear_failed.load(Ordering::SeqCst);
        let platform_off = !self.platform.enabled();
        if gate.is_restored() {
            let (waiting, history_wallets) = self.restore_waiting().await?;
            return Ok(Some(RestoreStatus {
                waiting,
                history_wallets,
                clear_failed,
                platform_only: false,
                platform_off,
            }));
        }
        if !gate.platform_pending() {
            if !self.any_platform_held() {
                return Ok(None);
            }
            return Ok(Some(RestoreStatus {
                waiting: vec![RestoreWait::PlatformPass],
                history_wallets: Vec::new(),
                clear_failed,
                platform_only: true,
                platform_off,
            }));
        }
        Ok(Some(RestoreStatus {
            waiting: self.residue_waiting().await?,
            history_wallets: Vec::new(),
            clear_failed,
            platform_only: true,
            platform_off,
        }))
    }

    /// Marks sync when what the gate waits for changed since the last time.
    async fn publish_restore_status(&self) {
        let Ok(status) = self.restore_status().await else {
            return;
        };
        let changed = {
            let mut published =
                (self.hub.restore.published.lock()).unwrap_or_else(|p| p.into_inner());
            let changed = *published != status;
            *published = status;
            changed
        };
        if changed {
            self.hub.pump.mark_sync();
        }
    }

    /// Asks Platform for a full Platform-address pass, and the gate's
    /// watcher to try the clear at once, a failed clear's delay cut short.
    /// The shielded pass comes from the Platform loops, which admit it only
    /// while the session is open. Does nothing while the gate is off.
    pub async fn restore_retry(self: &Arc<Self>) -> Result<(), EngineError> {
        let gate = &self.hub.restore;
        if !self.any_platform_held() {
            return Ok(());
        }
        gate.retry.store(true, Ordering::SeqCst);
        gate.wake.notify_one();
        if !self.platform.enabled() {
            return Ok(());
        }
        let manager = self.manager()?;
        let sync = manager.platform_address_sync_arc();
        if !sync.is_syncing() {
            let handle = self.rt.clone();
            // The library's pass is not `Send`: it runs on the blocking pool.
            drop(self.rt.spawn_blocking(move || {
                handle.block_on(sync.sync_now());
            }));
        }
        Ok(())
    }

    /// What all-Synced still waits for, and the loaded wallets whose holds
    /// are not built. Core, Platform and Shielded synced, a service that
    /// is off counting as synced, every loaded wallet's holds built, and at
    /// least one wallet loaded.
    async fn restore_waiting(&self) -> Result<(Vec<RestoreWait>, Vec<WalletId>), EngineError> {
        let manager = self.manager()?;
        let wallets = manager.list_wallet_ids_blocking();
        let some_wallet = !wallets.is_empty();
        let unbuilt: Vec<WalletId> = (wallets.into_iter())
            .map(WalletId)
            .filter(|id| !(self.hub.history).read_wallet(id, |h| h.restore_holds_built))
            .collect();
        let (core, platform) = self.restore_services(&manager).await;
        let waiting = [
            (core, RestoreWait::CoreSync),
            (platform, RestoreWait::PlatformPass),
            (unbuilt.is_empty(), RestoreWait::HistoryLoad),
            (some_wallet, RestoreWait::Wallet),
        ];
        let waiting = (waiting.into_iter())
            .filter_map(|(done, wait)| (!done).then_some(wait))
            .collect();
        Ok((waiting, unbuilt))
    }

    /// Whether Core, and Platform with Shielded, read as synced.
    async fn restore_services(&self, manager: &crate::session::Manager) -> (bool, bool) {
        let gate = &self.hub.restore;
        #[cfg(test)]
        if let Some(synced) = *gate.all_synced.lock().unwrap() {
            return (synced, synced);
        }
        let (core, masternodes) = {
            let tracker = self.hub.tracker();
            (tracker.caught_up(), tracker.masternodes_synced())
        };
        #[cfg(test)]
        let (core, masternodes) =
            (*gate.core_synced.lock().unwrap()).map_or((core, masternodes), |c| (c, c));
        let platform =
            !self.platform.enabled() || (masternodes && gate.platform_pass.load(Ordering::SeqCst));
        let shielded = !self.platform.enabled()
            || gate.shielded_pass.load(Ordering::SeqCst)
            || !crate::platform::shielded_pool::runs_sync(manager).await;
        (core, platform && shielded)
    }

    /// With the mark: all-Synced. Without it: Platform checked in this
    /// session and a wallet loaded.
    async fn restore_ready(&self, mark: bool) -> Result<bool, EngineError> {
        Ok(match mark {
            true => self.restore_waiting().await?.0.is_empty(),
            false => self.residue_waiting().await?.is_empty(),
        })
    }

    /// What the Platform residue waits for: `PlatformPass` until a session
    /// with Platform has checked it, `Wallet` while no wallet is loaded.
    async fn residue_waiting(&self) -> Result<Vec<RestoreWait>, EngineError> {
        let manager = self.manager()?;
        let checked = self.platform.enabled() && self.restore_services(&manager).await.1;
        let some_wallet = !manager.list_wallet_ids_blocking().is_empty();
        Ok([
            (checked, RestoreWait::PlatformPass),
            (some_wallet, RestoreWait::Wallet),
        ]
        .into_iter()
        .filter_map(|(done, wait)| (!done).then_some(wait))
        .collect())
    }

    /// H4: at all-Synced, runs one drop-dead pass over every loaded wallet
    /// and then clears the mark; without Platform it first writes the
    /// Platform residue, so a stop leaves both or the mark (C6). With
    /// Platform, the residue goes once Platform is checked and the pass
    /// ran, with the mark or alone. Returns whether nothing is left that
    /// this session can clear.
    pub(crate) async fn try_clear_restored(self: &Arc<Self>) -> Result<bool, EngineError> {
        let gate = &self.hub.restore;
        let mark = gate.is_restored();
        if !mark && (!gate.platform_pending() || !self.platform.enabled()) {
            if !gate.opened.load(Ordering::SeqCst) {
                self.restore_cleared()?;
            }
            return self.clear_platform_rows().await;
        }
        if !self.restore_ready(mark).await? {
            return Ok(false);
        }
        let _op = self.enter().await?;
        let manager = self.manager()?;
        let residue = {
            let _lifecycle = self.wallet_lifecycle.hold().await;
            #[cfg(test)]
            crate::coins::pause_at(&self.hub.restore.pause_pass).await;
            let live = self.live()?;
            let wallets: Vec<WalletId> = (manager.list_wallet_ids_blocking().into_iter())
                .map(WalletId)
                .collect();
            self.drop_dead_pass(&manager, &live, wallets).await?;
            if !self.restore_ready(mark).await? {
                return Ok(false);
            }
            if self.platform.enabled() {
                self.write_platform_rows(&manager, &live.appdb).await?;
            }
            if mark {
                self.clear_mark(&live).await?;
            }
            match self.platform.enabled() {
                true => gate.remove_residue().map_err(|e| {
                    EngineError::Storage(format!(
                        "restore: the Platform residue did not clear: {e}"
                    ))
                }),
                false => Ok(()),
            }
        };
        if mark || residue.is_ok() {
            self.restore_cleared()?;
        }
        residue?;
        Ok(!self.platform.enabled() || !self.rows_remain().await?)
    }

    /// Runs one drop-dead pass over `wallets`: an error when a wallet's
    /// dropped records do not read.
    async fn drop_dead_pass(
        &self,
        manager: &Arc<crate::session::Manager>,
        live: &crate::session::Live,
        wallets: Vec<WalletId>,
    ) -> Result<(), EngineError> {
        let db_path = self.data_dir().join(crate::session::WALLET_DB_FILE);
        let appdb = Arc::clone(&live.appdb);
        {
            let seeds = tokio::task::spawn_blocking(move || {
                let persisted = crate::history_ops::persisted_txids(&db_path);
                (wallets.into_iter())
                    .map(|w| {
                        let (here, read) = crate::tx_actions::load_dropped(&appdb, w);
                        let mut dead: crate::tx_actions::Dead = match &persisted {
                            Ok(p) => (p.get(&w.0).into_iter())
                                .flat_map(|p| {
                                    (p.dropped.iter())
                                        .filter(|t| !p.dropped_doubtful.contains(*t))
                                        .map(|t| (*t, crate::history::DropCause::InvalidClaim))
                                })
                                .collect(),
                            Err(_) => Default::default(),
                        };
                        for (t, cause) in here {
                            dead.entry(t).or_insert(cause);
                        }
                        let complete = read && persisted.is_ok();
                        (w, dead, complete)
                    })
                    .collect::<Vec<(WalletId, crate::tx_actions::Dead, bool)>>()
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
                    self.abandon_on_dead(
                        manager,
                        wallet,
                        &dead,
                        crate::tx_actions::DeadSource::Restore,
                    )
                    .await;
                    #[cfg(test)]
                    (self.hub.restore.pass_runs.lock().unwrap())
                        .push((wallet, dead.keys().copied().collect()));
                }
            }
        }
        Ok(())
    }

    /// With Platform: the rows of the loaded wallets Platform has checked
    /// since their load go, after one drop-dead pass over them, durably in
    /// one write; then what waited for them runs. Returns whether no
    /// wallet of the network's list keeps one (without Platform: nothing
    /// this session can clear).
    async fn clear_platform_rows(self: &Arc<Self>) -> Result<bool, EngineError> {
        if !self.platform.enabled() {
            return Ok(true);
        }
        let gate = &self.hub.restore;
        let appdb = self.live()?.appdb;
        let rowed: Vec<WalletId> = (self.registered_wallets().await?.into_iter())
            .filter(|w| gate.row_present(&appdb, *w))
            .collect();
        if rowed.is_empty() {
            return Ok(true);
        }
        let manager = self.manager()?;
        if !self.checked_of(&manager, &rowed).await.is_empty() {
            let _op = self.enter().await?;
            let _lifecycle = self.wallet_lifecycle.hold().await;
            let live = self.live()?;
            let ready = self.checked_of(&manager, &rowed).await;
            self.drop_dead_pass(&manager, &live, ready.clone()).await?;
            let ready = self.checked_of(&manager, &ready).await;
            if !ready.is_empty() {
                let scopes: Vec<String> = ready.iter().map(ToString::to_string).collect();
                let appdb = Arc::clone(&live.appdb);
                tokio::task::spawn_blocking(move || {
                    let writes: Vec<(&str, &str, Option<&str>)> = (scopes.iter())
                        .map(|w| (w.as_str(), RESTORE_PLATFORM_PENDING, None))
                        .collect();
                    appdb.set_scoped_settings_durable(&writes)
                })
                .await
                .map_err(|e| EngineError::Internal(format!("restore: Platform rows: {e}")))?
                .map_err(|e| EngineError::Storage(format!("restore: Platform rows: {e}")))?;
                self.leases.request_resume(None);
                self.hub.pump.mark_sync();
                for wallet in ready {
                    self.hub.pump.mark_balances(wallet);
                }
                tracing::info!("restore: a wallet's Platform funds are checked");
            }
        }
        Ok(!self.rows_remain().await?)
    }

    /// The loaded wallets of `wallets` Platform has checked since their
    /// load.
    async fn checked_of(
        &self,
        manager: &crate::session::Manager,
        wallets: &[WalletId],
    ) -> Vec<WalletId> {
        let loaded: HashSet<WalletId> = (manager.list_wallet_ids_blocking().into_iter())
            .map(WalletId)
            .collect();
        let mut out = Vec::new();
        for wallet in wallets {
            if loaded.contains(wallet) && self.wallet_checked(manager, *wallet).await {
                out.push(*wallet);
            }
        }
        out
    }

    /// Writes the holds' build marks of the unloaded wallets and, without
    /// Platform, the Platform residue, then clears the mark.
    async fn clear_mark(&self, live: &crate::session::Live) -> Result<(), EngineError> {
        let unloaded: Vec<WalletId> = live.store.unloaded().into_iter().map(WalletId).collect();
        if !unloaded.is_empty() {
            let appdb = Arc::clone(&live.appdb);
            tokio::task::spawn_blocking(move || {
                (unloaded.iter()).try_for_each(|w| {
                    appdb.set_settings_durable(&w.to_string(), &[(RESTORE_BUILD_PENDING, Some(""))])
                })
            })
            .await
            .map_err(|e| EngineError::Internal(format!("restore: build marks: {e}")))?
            .map_err(|e| EngineError::Storage(format!("restore: build marks: {e}")))?;
        }
        if !self.platform.enabled() {
            (self.hub.restore.write_residue()).map_err(|e| {
                EngineError::Storage(format!("restore: the Platform residue did not write: {e}"))
            })?;
        }
        #[cfg(test)]
        (self.hub.restore.clear_attempts).fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        if (self.hub.restore.clear_fails)
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(EngineError::Storage(
                "restore: the mark did not read".into(),
            ));
        }
        dw_store_upgrade::clear_restored_mark_on_first_all_synced(&live._persister_opened)
            .map(|_| ())
            .map_err(|e| EngineError::Storage(e.to_string()))
    }

    /// Once the mark is gone, however it went (a clear whose last step
    /// failed after the remove included): what waited for the gate runs.
    fn restore_cleared(&self) -> Result<(), EngineError> {
        let manager = self.manager()?;
        self.hub.restore.opened.store(true, Ordering::SeqCst);
        #[cfg(test)]
        (self.hub.restore.cleared_runs).fetch_add(1, Ordering::SeqCst);
        self.leases.request_resume(None);
        self.hub.pump.mark_sync();
        for id in manager.list_wallet_ids_blocking() {
            self.hub.pump.mark_balances(WalletId(id));
        }
        tracing::info!("restore: the money gate is off");
        Ok(())
    }
}
