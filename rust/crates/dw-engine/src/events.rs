//! Engine events and the per-session hub that turns dash-spv and
//! platform-wallet callbacks into them.
//!
//! [`SessionHub`] is the session's in-memory state that callbacks update:
//! the sync tracker, the history store and each wallet's balance and sync
//! height. Callbacks only update that state and mark the event pump
//! ([`crate::pump`]); the pump task delivers `Sync`, `Balances` and
//! `HistoryChanged` at most 4 times a second per domain and always delivers
//! the last change of a burst (reviews H2, M4).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use dash_spv::EventHandler;
use dash_spv::network::NetworkEvent;
use dash_spv::sync::SyncProgress;
use dashcore::Txid;
use key_wallet::account::AccountType;
use key_wallet::wallet::balance::WalletCoreBalance;
use platform_wallet::PlatformEventHandler;
use platform_wallet::events::WalletEvent;

use crate::history::HistoryStore;
use crate::pump::{EventPump, PumpTarget, TxidSet};
use crate::sync::{SyncSnapshot, SyncTracker};
use crate::tools::{RescanState, better_chainlock, count_masternodes};
use crate::{DashNetwork, WalletBalances, WalletId};

/// Engine → host signal. Events say *what changed*; hosts pull the data again
/// (DESIGN-opus §1.5 rule 4).
#[derive(Debug, Clone, PartialEq)]
pub enum EngineEvent {
    SessionOpened {
        network: DashNetwork,
    },
    SessionClosed {
        network: DashNetwork,
    },
    /// A wallet was registered, or keys were attached to a registered
    /// wallet. Hosts reload the wallet list.
    WalletCreated {
        network: DashNetwork,
        wallet_id: WalletId,
    },
    /// `remove_wallet` finished.
    WalletRemoved {
        network: DashNetwork,
        wallet_id: WalletId,
    },
    SpvStateChanged {
        network: DashNetwork,
        running: bool,
    },
    /// The sync snapshot changed (≤ 4 Hz, trailing edge kept).
    Sync {
        network: DashNetwork,
        snapshot: SyncSnapshot,
    },
    /// The wallet's balance buckets changed. `None` while the balance is not
    /// known yet (the scan has not reached the birth height).
    Balances {
        network: DashNetwork,
        wallet_id: WalletId,
        balances: Option<WalletBalances>,
    },
    /// Transactions were added or changed status. Empty `txids` means
    /// "reload the whole history".
    HistoryChanged {
        network: DashNetwork,
        wallet_id: WalletId,
        txids: Vec<Txid>,
    },
    Notice {
        network: Option<DashNetwork>,
        code: NoticeCode,
        detail: String,
    },
    /// The vault of `network` changed lock state (created, unlocked,
    /// locked, encrypted).
    VaultLockState {
        network: DashNetwork,
        state: dw_vault::LockState,
    },
    /// Transactions of the wallet seen for the first time, batched over
    /// 100 ms as dash-qt batches its popups (QT-031…033). Never sent for
    /// status changes. `catch_up`: SPV was not caught up when the batch was
    /// sent (dash-qt shows no popups during initial sync).
    NewTransactions {
        network: DashNetwork,
        wallet_id: WalletId,
        txids: Vec<Txid>,
        catch_up: bool,
    },
    /// A wallet was loaded or unloaded (QT-101).
    WalletLoadChanged {
        network: DashNetwork,
        wallet_id: WalletId,
        loaded: bool,
    },
    /// M3 (QT-041…050): the wallet's mixing status, CoinJoin balances or
    /// progress changed. At most once a second per wallet (dash-qt's panel
    /// timer), trailing edge kept. Hosts re-query `coinjoin_status`.
    CoinJoin {
        network: DashNetwork,
        wallet_id: WalletId,
    },
}

/// Non-fatal conditions the UI may surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeCode {
    /// The trusted quorum service could not be reached; Platform proof
    /// verification retries lazily.
    PlatformContextUnavailable,
    /// dash-spv reported a fatal sync error.
    SpvError,
    /// Session shutdown left a worker running (see the detail text).
    UncleanShutdown,
    /// SPV made no progress for 45 s while not caught up (IOS-023). The
    /// engine owns this rule and sends one notice per stall; the host may
    /// offer `rotate_peers`.
    SyncStalled,
    /// An automatic wallet backup failed (QT-116); the detail names the
    /// wallet and the cause. A backup skipped because the data key is not
    /// available is not a failure.
    BackupFailed,
    /// `remove_wallet` removed the wallet but could not delete its seed from
    /// the vault (for example the vault was locked meanwhile). The detail
    /// names the wallet id; the seed stays in the vault, encrypted.
    WalletSecretNotDeleted,
}

/// Receives engine events. Called from engine threads; must not block.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: EngineEvent);
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Balance and scan state of one wallet, kept from wallet events so the
/// synchronous calls never wait for the wallet-manager lock.
#[derive(Debug, Clone, Default)]
pub(crate) struct WalletState {
    pub birth_height: u32,
    /// Filter-scan checkpoint (`synced_height`).
    pub synced_height: u32,
    /// Best processed block (`last_processed_height`).
    pub processed_height: u32,
    pub balance: WalletCoreBalance,
    /// Latest balance per account.
    pub accounts: BTreeMap<AccountType, WalletCoreBalance>,
    /// Fully mixed CoinJoin balance by the QT-043 rule, once the CoinJoin
    /// view of the wallet was computed (`coinjoin.rs`).
    pub fully_mixed: Option<u64>,
}

impl WalletState {
    /// Balances are known once the scan has processed the birth block
    /// (review M-3): before that a zero would be a guess.
    pub fn balances(&self) -> Option<WalletBalances> {
        if self.synced_height == 0 || self.synced_height < self.birth_height {
            return None;
        }
        let b = &self.balance;
        // Before the CoinJoin view is computed: the CoinJoin accounts'
        // spendable balance (an upper bound of the fully mixed one).
        let coinjoin = self.fully_mixed.unwrap_or_else(|| {
            self.accounts
                .iter()
                .filter(|(t, _)| matches!(t, AccountType::CoinJoin { .. }))
                .map(|(_, b)| b.confirmed().saturating_add(b.unconfirmed()))
                .sum()
        });
        Some(WalletBalances {
            confirmed: b.confirmed(),
            unconfirmed: b.unconfirmed(),
            immature: b.immature(),
            locked: b.locked(),
            total: b.total(),
            coinjoin,
        })
    }

    /// Best height the wallet has seen, used as the tip for confirmations.
    pub fn tip(&self) -> u32 {
        self.processed_height.max(self.synced_height)
    }
}

/// Display name and creation time of a wallet (dw-appdb `wallets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletName {
    pub name: String,
    pub created_at: Option<u64>,
    /// Position in the order wallets were added (ties within a second).
    pub order: u64,
}

/// In-memory state of one session, updated from SPV and wallet callbacks.
pub(crate) struct SessionHub {
    pub network: DashNetwork,
    pub sink: Arc<dyn EventSink>,
    pub tracker: Mutex<SyncTracker>,
    pub pump: EventPump,
    pub history: HistoryStore,
    pub wallets: RwLock<BTreeMap<WalletId, WalletState>>,
    pub names: RwLock<HashMap<WalletId, WalletName>>,
    /// The rescan the engine started, until it finishes or is cancelled.
    pub rescan: Mutex<Option<RescanState>>,
    /// Height and block hash of the best ChainLock any wallet applied.
    pub chainlock: Mutex<Option<(u32, dashcore::BlockHash)>>,
    /// When this session last handed a transaction to the network (send or
    /// resend), UNIX seconds.
    pub announced: Mutex<HashMap<Txid, u64>>,
}

impl SessionHub {
    pub(crate) fn new(network: DashNetwork, sink: Arc<dyn EventSink>) -> Self {
        Self {
            network,
            sink,
            tracker: Mutex::new(SyncTracker::default()),
            pump: EventPump::default(),
            history: HistoryStore::default(),
            wallets: RwLock::new(BTreeMap::new()),
            names: RwLock::new(HashMap::new()),
            rescan: Mutex::new(None),
            chainlock: Mutex::new(None),
            announced: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn note_announced(&self, txid: Txid) {
        self.announced
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(txid, unix_now());
    }

    pub(crate) fn announced_at(&self, txid: &Txid) -> Option<u64> {
        self.announced
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(txid)
            .copied()
    }

    pub(crate) fn rescan(&self) -> std::sync::MutexGuard<'_, Option<RescanState>> {
        self.rescan.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn best_chainlock(&self) -> Option<(u32, dashcore::BlockHash)> {
        *self.chainlock.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Keeps the best ChainLock seen (by height).
    pub(crate) fn note_chainlock(&self, height: u32, hash: dashcore::BlockHash) {
        let mut best = self.chainlock.lock().unwrap_or_else(|p| p.into_inner());
        *best = better_chainlock(*best, (height, hash));
    }

    pub(crate) fn emit(&self, event: EngineEvent) {
        self.sink.emit(event);
    }

    pub(crate) fn tracker(&self) -> std::sync::MutexGuard<'_, SyncTracker> {
        self.tracker.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub(crate) fn wallet_state(&self, id: &WalletId) -> Option<WalletState> {
        self.wallets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    pub(crate) fn set_wallet_state(&self, id: WalletId, state: WalletState) {
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, state);
    }

    pub(crate) fn name_of(&self, id: &WalletId) -> Option<WalletName> {
        self.names
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    pub(crate) fn set_name(&self, id: WalletId, name: WalletName) {
        self.names
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, name);
    }

    /// The `order` a newly named wallet gets: after every known one.
    pub(crate) fn next_name_order(&self) -> u64 {
        self.names
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|n| n.order + 1)
            .max()
            .unwrap_or(0)
    }

    /// Forgets the in-memory state of an unloaded wallet; its name stays
    /// (the "Open Wallet" menu lists it).
    pub(crate) fn unload_wallet(&self, id: &WalletId) {
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.history.remove_wallet(id);
        if let Some(rescan) = self.rescan().as_mut() {
            rescan.wallets.remove(id);
        }
    }

    /// Forgets everything about a removed wallet.
    pub(crate) fn forget_wallet(&self, id: &WalletId) {
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.names
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.history.remove_wallet(id);
    }

    pub(crate) fn set_spv_running(&self, running: bool) {
        if self.tracker().set_running(running) {
            self.pump.mark_sync();
        }
    }

    /// Applies a wallet event's balance snapshot. Returns whether the
    /// visible balances changed.
    fn update_balance(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
    ) -> bool {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        let state = map.entry(id).or_default();
        let before = state.balances();
        state.balance = *balance;
        for (t, b) in accounts {
            state.accounts.insert(*t, *b);
        }
        before != state.balances()
    }

    /// Records the fully mixed balance the CoinJoin view computed and
    /// announces it as a balance change when it differs.
    pub(crate) fn set_fully_mixed(&self, id: WalletId, value: u64) {
        let changed = {
            let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
            let Some(state) = map.get_mut(&id) else {
                return;
            };
            let before = state.balances();
            state.fully_mixed = Some(value);
            before != state.balances()
        };
        if changed {
            self.pump.mark_balances(id);
        }
    }

    /// Advances the wallet's scan heights. Returns whether the balances
    /// became known or unknown.
    fn update_heights(&self, id: WalletId, synced: Option<u32>, processed: Option<u32>) -> bool {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        let state = map.entry(id).or_default();
        let known_before = state.balances().is_some();
        if let Some(h) = synced {
            state.synced_height = h;
        }
        if let Some(h) = processed {
            state.processed_height = state.processed_height.max(h);
        }
        known_before != state.balances().is_some()
    }

    /// Marks history entries whose confirmation count changed with a new
    /// wallet tip.
    fn mark_young(&self, id: WalletId) {
        let tip = self.wallet_state(&id).map(|s| s.tip()).unwrap_or(0);
        let young = self.history.young_txids(&id, tip);
        if !young.is_empty() {
            self.pump.mark_history(id, Some(&young));
        }
    }

    pub(crate) fn apply_wallet_event(&self, event: &WalletEvent) {
        let id = WalletId(event.wallet_id());
        let now = Some(unix_now());
        match event {
            WalletEvent::TransactionDetected {
                record,
                balance,
                account_balances,
                ..
            } => {
                if self.history.with_wallet(id, |h| h.upsert(record, now)) {
                    self.pump.mark_new_txs(id, &[record.txid]);
                }
                self.pump.mark_history(id, Some(&[record.txid]));
                if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
            }
            WalletEvent::TransactionInstantLocked {
                txid,
                balance,
                account_balances,
                ..
            } => {
                if self.history.with_wallet(id, |h| h.set_instant_locked(txid)) {
                    self.pump.mark_history(id, Some(&[*txid]));
                }
                if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
            }
            WalletEvent::TransactionsSwept {
                txids,
                balance,
                account_balances,
                ..
            } => {
                // Upstream deleted these records (provably beaten spends);
                // the history drops them too.
                self.history.with_wallet(id, |h| {
                    for t in txids {
                        h.txs.remove(t);
                    }
                });
                self.pump.mark_history(id, Some(txids));
                if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
            }
            WalletEvent::BlockProcessed {
                height,
                inserted,
                updated,
                matured,
                balance,
                account_balances,
                ..
            } => {
                let mut new = Vec::new();
                let changed: Vec<Txid> = self.history.with_wallet(id, |h| {
                    inserted
                        .iter()
                        .chain(updated)
                        .chain(matured)
                        .map(|r| {
                            if h.upsert(r, now) {
                                new.push(r.txid);
                            }
                            r.txid
                        })
                        .collect()
                });
                if !new.is_empty() {
                    self.pump.mark_new_txs(id, &new);
                }
                let flipped = self.update_heights(id, None, Some(*height));
                if !changed.is_empty() {
                    self.pump.mark_history(id, Some(&changed));
                }
                self.mark_young(id);
                if self.update_balance(id, balance, account_balances) || flipped {
                    self.pump.mark_balances(id);
                }
            }
            WalletEvent::SyncHeightAdvanced { height, .. } => {
                if self.update_heights(id, Some(*height), Some(*height)) {
                    self.pump.mark_balances(id);
                }
                if let Some(rescan) = self.rescan().as_mut()
                    && rescan.note_height(&id, *height)
                {
                    self.pump.mark_sync();
                }
                self.mark_young(id);
            }
            WalletEvent::ChainLockProcessed {
                chain_lock,
                locked_transactions,
                ..
            } => {
                self.note_chainlock(chain_lock.block_height, chain_lock.block_hash);
                let locked: Vec<Txid> = self.history.with_wallet(id, |h| {
                    locked_transactions
                        .values()
                        .flatten()
                        .filter(|t| h.set_chain_locked(t))
                        .copied()
                        .collect()
                });
                if !locked.is_empty() {
                    self.pump.mark_history(id, Some(&locked));
                }
            }
        }
    }
}

impl EventHandler for SessionHub {
    fn on_progress(&self, progress: &SyncProgress) {
        if self.tracker().on_progress(progress) {
            self.pump.mark_sync();
        }
    }

    fn on_network_event(&self, event: &NetworkEvent) {
        if self.tracker().on_network_event(event) {
            self.pump.mark_sync();
        }
    }

    fn on_wallet_event(&self, event: &WalletEvent) {
        self.apply_wallet_event(event);
    }

    fn on_error(&self, error: &str) {
        self.emit(EngineEvent::Notice {
            network: Some(self.network.clone()),
            code: NoticeCode::SpvError,
            detail: error.to_string(),
        });
    }
}

impl PlatformEventHandler for SessionHub {}

/// Delivers the hub's merged changes. Runs on the pump task, which the
/// session stops before it shuts the manager down.
pub(crate) struct SessionPump {
    pub hub: Arc<SessionHub>,
    pub spv: Arc<platform_wallet::SpvRuntime>,
    pub appdb: Arc<dw_appdb::AppDb>,
    pub vault: dw_vault::Vault,
}

impl PumpTarget for SessionPump {
    async fn flush_sync(&self) {
        let due = self.hub.tracker().tip_time_due();
        if let Some(height) = due {
            let time = self.spv.tip_block_time().await.map(u64::from);
            self.hub.tracker().set_tip_time(height, time);
        }
        let mn_due = self.hub.tracker().masternode_counts_due();
        if let Some(height) = mn_due {
            let counts = self
                .spv
                .masternode_list_summaries()
                .await
                .map(|list| count_masternodes(&list));
            self.hub.tracker().set_masternode_counts(height, counts);
        }
        let snapshot = self.hub.tracker().snapshot();
        self.hub.emit(EngineEvent::Sync {
            network: self.hub.network.clone(),
            snapshot,
        });
    }

    fn flush_balances(&self, wallets: BTreeSet<WalletId>) {
        for wallet_id in wallets {
            let Some(state) = self.hub.wallet_state(&wallet_id) else {
                continue;
            };
            self.hub.emit(EngineEvent::Balances {
                network: self.hub.network.clone(),
                wallet_id,
                balances: state.balances(),
            });
        }
    }

    fn flush_history(&self, changes: BTreeMap<WalletId, TxidSet>) {
        let mut seen: Vec<(WalletId, Vec<String>)> = Vec::new();
        for (wallet_id, set) in changes {
            let txids = match set {
                TxidSet::All => Vec::new(),
                TxidSet::Some(set) => set.into_iter().collect(),
            };
            if !txids.is_empty() {
                seen.push((wallet_id, txids.iter().map(Txid::to_string).collect()));
            }
            self.hub.emit(EngineEvent::HistoryChanged {
                network: self.hub.network.clone(),
                wallet_id,
                txids,
            });
        }
        if seen.is_empty() {
            return;
        }
        // First-seen times keep a mempool payment's arrival time across
        // restarts (the persisted record has only the block time).
        let appdb = Arc::clone(&self.appdb);
        let now = unix_now();
        tokio::task::spawn_blocking(move || {
            for (wallet_id, txids) in seen {
                if let Err(e) = appdb.note_txs_seen(&wallet_id.to_string(), &txids, now) {
                    tracing::warn!(%wallet_id, error = %e, "could not store first-seen times");
                }
            }
        });
    }

    fn flush_new_txs(&self, batches: BTreeMap<WalletId, Vec<Txid>>) {
        let catch_up = !self.hub.tracker().caught_up();
        for (wallet_id, txids) in batches {
            self.hub.emit(EngineEvent::NewTransactions {
                network: self.hub.network.clone(),
                wallet_id,
                txids,
                catch_up,
            });
        }
    }

    fn tick(&self) {
        // An unused grant's own copy of the data key goes when the grant
        // expires, not at the next vault call.
        self.vault.purge_expired_grants();
        let stalled = self.hub.tracker().check_stall();
        if stalled {
            let since = self.hub.tracker().snapshot().seconds_since_progress;
            self.hub.emit(EngineEvent::Notice {
                network: Some(self.hub.network.clone()),
                code: NoticeCode::SyncStalled,
                detail: format!("no sync progress for {} s", since.unwrap_or(0)),
            });
            self.hub.pump.mark_sync();
        }
    }
}
