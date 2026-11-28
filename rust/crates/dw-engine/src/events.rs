//! Engine events and the per-session hub that turns dash-spv and
//! platform-wallet callbacks into them.
//!
//! [`SessionHub`] is the session's in-memory state that callbacks update:
//! the sync tracker, the history store and each wallet's balance and sync
//! height. Callbacks only update that state and mark the event pump
//! ([`crate::pump`]); the pump task delivers `Sync`, `Balances` and
//! `HistoryChanged` at most 4 times a second per domain and always delivers
//! the last change of a burst (reviews H2, M4).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use dash_spv::EventHandler;
use dash_spv::SyncEvent;
use dash_spv::SyncProgress;
use dash_spv::network::NetworkEvent;
use dashcore::ephemerealdata::instant_lock::InstantLock;
use dashcore::{Transaction, Txid};
use key_wallet::account::AccountType;
use key_wallet::transaction_checking::TransactionContext;
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::PlatformEventHandler;
use platform_wallet::events::WalletEvent;
use platform_wallet::manager::shielded_sync::{ShieldedSyncPassSummary, WalletShieldedOutcome};

use crate::history::HistoryStore;
use crate::pump::{EventPump, NewTxs, PumpTarget, TxidSet};
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
    /// The wallet's balance buckets changed, or whether it is still
    /// catching up (`WalletBalances::catching_up`).
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
    /// status changes, so a transaction is named once: a payment seen in
    /// the mempool and then mined is in the batch of its mempool sighting
    /// only. `catch_up`: SPV was not caught up when the batch was sent
    /// (dash-qt shows no popups during initial sync).
    NewTransactions {
        network: DashNetwork,
        wallet_id: WalletId,
        txids: Vec<Txid>,
        /// Those of `txids` learned live, in their order: first seen in the
        /// mempool or with an InstantSend lock (dash-spv relays both during
        /// sync too), or in a block at most two blocks (`LIVE_DEPTH`) below the
        /// network's best height when the block was processed. The others
        /// were found by the scan in older blocks, or with no best height
        /// known yet.
        live: Vec<Txid>,
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
    /// DashPay data of the wallet changed (DASHPAY §3.5): the changeset tap
    /// saw the library store it. At most 4 times a second, trailing edge
    /// kept; hosts re-query the domain `change` names.
    Platform {
        network: DashNetwork,
        wallet_id: WalletId,
        change: PlatformChange,
    },
    /// DashPay news for the OS notifier (DP2-05): journal rows of the
    /// identity that this session added unread and that notify: never
    /// catch-up, our own accept or name, or a payment (the
    /// `NewTransactions` notice covers it). On the `Platform` clock (≤ 4 Hz, trailing edge kept),
    /// at most [`crate::platform::MAX_NEWS`] per event in arrival order,
    /// `more` counting the rest. Public data only: ids, kinds, a label, a
    /// request time or a txid; no alias, note or amount.
    DashPayNews {
        network: DashNetwork,
        wallet_id: WalletId,
        identity: String,
        events: Vec<crate::platform::DashPayEvent>,
        more: u32,
    },
    /// MG-03 (mn-engine §6): a masternode of interest (the wallets' own or
    /// a tracked one) changed in the list, or a track, label or key
    /// changed. At most once a second, trailing edge kept; hosts re-read
    /// what they show. Engine-side until E0-13 binds the facade.
    Masternodes {
        network: DashNetwork,
    },
    /// E0-04 §16.7, engine-side until E0-13: a lease's view changed.
    LeaseChanged {
        network: DashNetwork,
        lease: crate::platform::LeaseView,
    },
    /// E0-04 §16.7, engine-side until E0-13: a lock's drain is running, or
    /// it is done with its report.
    LockProgress {
        network: DashNetwork,
        phase: crate::platform::LockPhase,
    },
    /// E0-04 §16.7, engine-side until E0-13: a provisional outcome settled.
    DispatchResolved {
        network: DashNetwork,
        resolved: crate::platform::DispatchResolved,
    },
    /// MG-02 (mg-engine §5): the governance store, the sync state or the
    /// masternode list's voting keys changed. At most once a second,
    /// trailing edge kept; hosts re-read what they show. Bound for the Tauri
    /// host (BL-232); dw-ffi withholds it (MG-02 D95).
    Governance {
        network: DashNetwork,
    },
    /// MP-01 (rates-engine §5): the rates view changed (a fetch, a setting,
    /// a price turning stale or expiring, a mainnet network opened or
    /// closed). Not a network's: prices are the engine's. Hosts re-read
    /// `Rates::status`. dw-ffi withholds it.
    Rates,
    /// An automatic backup was written (QT-116): hosts re-read the
    /// automatic backups and their failures. dw-ffi withholds it.
    Backups {
        network: DashNetwork,
    },
}

impl EngineEvent {
    /// Variants the frozen bindings do not carry yet (E0-04 §13): dw-ffi
    /// drops them until E0-13 forwards them.
    pub fn engine_side(&self) -> bool {
        match self {
            Self::Platform { .. }
            | Self::Masternodes { .. }
            | Self::DashPayNews { .. }
            | Self::LeaseChanged { .. }
            | Self::LockProgress { .. }
            | Self::DispatchResolved { .. }
            | Self::Governance { .. }
            | Self::Rates
            | Self::Backups { .. } => true,
            Self::Notice { code, .. } => code.engine_side(),
            _ => false,
        }
    }
}

/// The domain of an [`EngineEvent::Platform`] signal (DASHPAY §3.5).
/// Identities are Base58; `draft` is the registration's id, `None` for an
/// asset lock no registration row names (a top-up).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PlatformChange {
    /// Identities, their keys, profiles or names. Contact views re-query
    /// on it too: contact profiles ride the identity snapshots.
    Identities,
    /// The identity's contacts, requests, contact accounts or ignored
    /// senders.
    Contacts { identity: String },
    /// A tracked asset lock.
    Registration { draft: Option<String> },
    /// The identity's DashPay payments.
    Payments { identity: String },
    /// The DPNS name states (the username marketplace).
    Names,
    /// The identity's notifications feed or unread count (DP2-05).
    Events { identity: String },
    /// The wallet's DashPay bring-up outcome, or the identity it named
    /// (`dashpay_startup`, and with it `DashPay::status`; BL-153).
    Startup,
    /// The wallet's Platform-address balances or their activity (a sync
    /// pass changed them, or a move to Core was recorded or settled).
    PlatformAddresses,
    /// The wallet's shielded account: a sync pass, its balance or its
    /// activity (m4 §2.13); hosts re-query `shielded_status` and
    /// `shielded_history`.
    Shielded,
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
    /// No longer sent: since E0-04 DEC-134 `remove_wallet` fails closed
    /// when it cannot delete the seed, and returns that error instead. Kept
    /// because the frozen bindings carry it.
    WalletSecretNotDeleted,
    /// A wallet's DashPay bring-up ended short of settled (DASHPAY §3.7):
    /// SPV started anyway, and the DIP-15 rescan or a later start finishes
    /// the work. The detail names the wallet and its `StartupStatus`; hosts
    /// read `dashpay_startup`.
    DashPayStartupIncomplete,
    /// A tracked asset-lock row has no dispatch record (unknown
    /// provenance): it is kept and not sent (E0-04 §6.5). Engine-side.
    DispatchRecordMissing,
    /// A hand-off ran outside any dispatch scope, an engine bug; it was not
    /// sent (E0-04 §5.4, H7). Engine-side.
    UnscopedDispatch,
    /// The dispatch journal cannot be opened or is newer than this build:
    /// leased hand-offs are refused (E0-04 §6.4). Engine-side.
    DispatchJournalUnavailable,
    /// SPV and the trusted quorum service disagree on a quorum key (DASHPAY
    /// §2.2 rule 4): every proof citing it is refused with
    /// `platform.trust_mismatch`. Once per quorum and session; the detail
    /// names the quorum. Engine-side.
    PlatformTrustMismatch,
    /// At the session's open, a Dash Core export that did not end had left
    /// its staging file, which holds keys unencrypted; it was deleted. The
    /// detail is its path.
    ExportStagingDeleted,
    /// As [`Self::ExportStagingDeleted`], but the file could not be
    /// deleted: the detail is its path, for the user to delete. Sent again
    /// at each open until a delete succeeds.
    ExportStagingNotDeleted,
}

impl NoticeCode {
    /// Codes the frozen bindings do not carry yet (E0-04 §16.8, E0-05).
    pub fn engine_side(self) -> bool {
        matches!(
            self,
            Self::DashPayStartupIncomplete
                | Self::DispatchRecordMissing
                | Self::UnscopedDispatch
                | Self::DispatchJournalUnavailable
                | Self::PlatformTrustMismatch
        )
    }
}

/// Receives engine events. Called from engine threads; must not block.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: EngineEvent);
}

/// How far below the network's best height a block's transactions are
/// still learned live (`EngineEvent::NewTransactions::live`): the block at
/// the tip, and the two before it for a tip that moved while the block was
/// processed.
pub(crate) const LIVE_DEPTH: u32 = 2;

/// Whether a transaction first seen in `context` is learned live, the
/// network's best height being `network_tip`. A block above that height is
/// not: the reading is behind, and may be a whole scan behind.
pub(crate) fn learned_live(context: &TransactionContext, network_tip: Option<u32>) -> bool {
    match context {
        TransactionContext::Mempool | TransactionContext::InstantSend(_) => true,
        TransactionContext::InBlock(block) | TransactionContext::InChainLockedBlock(block) => {
            network_tip
                .and_then(|tip| tip.checked_sub(block.height()))
                .is_some_and(|depth| depth <= LIVE_DEPTH)
        }
    }
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
    /// The highest filter checkpoint seen (lowered only by a fork): a
    /// replay that rewound the checkpoint is done once it is back here.
    pub scan_target: u32,
    /// The checkpoint a replay `cancel_rescan` stopped had reached: what it
    /// re-credited above it stays in memory (and stored) until a rewind to
    /// it or below replays those blocks (`note_rewind`). Its replay record
    /// keeps the hold across a restart (`crate::replay_record`).
    pub replay_cancelled: Option<u32>,
    /// What the restore holds keep out of the spendable balance
    /// (`restore_gate`, H1): shown pending instead.
    pub restore_held: u64,
    /// The wallet is being opened and its stored replay not yet resumed:
    /// spends wait (`NetworkSession::load_wallet`).
    pub loading: bool,
    /// The height of the last truncation the hub found in key-wallet
    /// ([`SessionHub::detect_truncation`]) and has not yet heard the
    /// `ChainTruncated` event of: the event at this height, or one above
    /// it that would lower nothing, is that truncation's, heard late.
    pub detected: Option<u32>,
    /// What the wallet's drops hold, summed, the held coins of unknown
    /// value, and whether one awaits what spends it
    /// (`NetworkSession::refresh_held`).
    pub held: (u64, u32, bool),
    /// The share of the excluded coins (`crate::withhold`) in `balance`
    /// and `accounts`, read with them from the manager. An event's
    /// snapshot keeps the last one until the manager is read again.
    pub excluded: crate::withhold::Excluded,
}

impl WalletState {
    /// Whether spends wait for a replay at filter checkpoint `checkpoint`
    /// (`NetworkSession::replay_guard`).
    pub fn replaying_at(&self, checkpoint: u32) -> bool {
        checkpoint < self.scan_target || self.replay_cancelled.is_some() || self.loading
    }

    /// The engine rewound the checkpoint to `checkpoint`; the live check
    /// holds spends from there to the target.
    pub fn note_rewind(&mut self, checkpoint: u32) {
        if self
            .replay_cancelled
            .is_some_and(|reached| checkpoint <= reached)
        {
            self.replay_cancelled = None;
        }
    }

    /// key-wallet's fields of `read` (`refresh_wallet_state`), merged
    /// under the hub's lock: the target and the cancel hold stay this
    /// state's, so an advance or a fork applied since the read stands.
    pub fn take_core(&mut self, read: WalletState) {
        self.note_rewind(read.synced_height);
        *self = WalletState {
            scan_target: self.scan_target.max(read.synced_height),
            replay_cancelled: self.replay_cancelled,
            loading: self.loading,
            fully_mixed: self.fully_mixed,
            detected: self.detected,
            held: self.held,
            restore_held: self.restore_held,
            ..read
        };
    }

    /// The scan has not processed the birth block yet (review M-3): the
    /// balance holds what the wallet has seen so far, which can miss coins
    /// in blocks not scanned yet.
    pub fn catching_up(&self) -> bool {
        self.synced_height == 0 || self.synced_height < self.birth_height
    }

    /// The balance known so far: confirmed coins the scan found, and the
    /// mempool and InstantSend payments seen while it runs.
    pub fn balances(&self) -> WalletBalances {
        use crate::withhold::Excluded;
        let b = Excluded::subtract(&self.balance, Some(&self.excluded.balance));
        // Before the CoinJoin view is computed: the CoinJoin accounts'
        // spendable balance (an upper bound of the fully mixed one).
        let coinjoin = self.fully_mixed.unwrap_or_else(|| {
            self.accounts
                .iter()
                .filter(|(t, _)| matches!(t, AccountType::CoinJoin { .. }))
                .map(|(t, b)| Excluded::subtract(b, self.excluded.accounts.get(t)))
                .map(|b| b.confirmed().saturating_add(b.unconfirmed()))
                .sum()
        });
        let held = self.restore_held.min(b.confirmed());
        WalletBalances {
            confirmed: b.confirmed() - held,
            unconfirmed: b.unconfirmed().saturating_add(held),
            immature: b.immature(),
            locked: b.locked(),
            total: b.total(),
            coinjoin,
            withheld: self.excluded.pending,
            catching_up: self.catching_up(),
            held_unfinished: self.held.0,
            held_unvalued: self.held.1,
            held_awaiting: self.held.2,
        }
    }

    /// Best height the wallet has seen, used as the tip for confirmations.
    pub fn tip(&self) -> u32 {
        self.processed_height.max(self.synced_height)
    }
}

/// Tests: moves key-wallet's filter checkpoint to `height`, as a commit
/// of dash-spv's filter batch does (its processed height rises with it,
/// filters/manager.rs:628-634 at 8e7ad2c1) or a rewind does (it stays).
#[cfg(test)]
pub(crate) fn set_checkpoint(
    core: &mut key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    height: u32,
) {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    core.update_synced_height(height);
    process_to(core, height);
}

/// Tests: key-wallet processed a block at `height`, as before its
/// `BlockProcessed` event (process_block.rs:620-626 at 8e7ad2c1).
#[cfg(test)]
pub(crate) fn process_to(
    core: &mut key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    height: u32,
) {
    use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
    if height > core.last_processed_height() {
        core.update_last_processed_height(height);
    }
}

/// What a fork truncation dropped ([`SessionHub::apply_truncation`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Truncated<'a> {
    /// Transactions recorded in blocks above the height, gone.
    pub removed: &'a [Txid],
    /// InstantSend-locked transactions recorded in blocks above the
    /// height, out of a block now.
    pub unconfirmed: &'a [Txid],
    /// Found in key-wallet ([`SessionHub::detect_truncation`]), not
    /// announced by its event.
    pub detected: bool,
}

/// Display name and creation time of a wallet (dw-appdb `wallets`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WalletName {
    pub name: String,
    pub created_at: Option<u64>,
    /// Position in the order wallets were added (ties within a second).
    pub order: u64,
}

/// How old a transaction may be to be stamped with the current price
/// (MP-01; Android's rule).
const STAMP_MAX_AGE: u64 = 3 * 3600;

/// Records the news of a wallet's event stream in its sweep rows: whether
/// any changed, `None` when the rows could not be written
/// (`coinjoin::write_beaten`, `coinjoin::delete_beaten_above`).
pub(crate) type SweepNewsHook =
    Box<dyn Fn(WalletId, crate::coinjoin::SweepNews<'_>) -> Option<bool> + Send + Sync>;

/// The hash of the block the SPV header chain holds at a height; `None`
/// unless the header store has one there (`SpvRuntime::header_hash_at`
/// answered `Found`).
pub(crate) type HeaderSource = Arc<
    dyn Fn(u32) -> futures_util::future::BoxFuture<'static, Option<dashcore::BlockHash>>
        + Send
        + Sync,
>;

/// What drops the history's transactions built on a sweep's: the wallet
/// and what the sweep dropped that can never confirm (`tx_actions`, #739).
pub(crate) type SweptDead = dyn Fn(WalletId, crate::tx_actions::Dead) + Send + Sync;

/// What checks the history's pending spends of new transactions' outputs
/// for unsigned claims: the wallet and the new txids (`tx_actions`, #739).
pub(crate) type Funded = dyn Fn(WalletId, BTreeSet<Txid>) + Send + Sync;

/// A hub set's guard; a poisoned one still holds its last state.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// In-memory state of one session, updated from SPV and wallet callbacks.
pub(crate) struct SessionHub {
    pub network: DashNetwork,
    pub sink: Arc<dyn EventSink>,
    pub tracker: Mutex<SyncTracker>,
    pub pump: EventPump,
    pub history: HistoryStore,
    /// The session's app database, once the session is open and until it
    /// closes: a sweep drops the sent marks of the payments it removed.
    pub appdb: std::sync::OnceLock<std::sync::Weak<dw_appdb::AppDb>>,
    pub wallets: RwLock<BTreeMap<WalletId, WalletState>>,
    pub names: RwLock<HashMap<WalletId, WalletName>>,
    /// The rescan the engine started, until it finishes or is cancelled.
    pub rescan: Mutex<Option<RescanState>>,
    /// Height and block hash of the best ChainLock any wallet applied.
    pub chainlock: Mutex<Option<(u32, dashcore::BlockHash)>>,
    /// When this session last handed a transaction to the network (send or
    /// resend), UNIX seconds.
    pub announced: Mutex<HashMap<Txid, u64>>,
    /// The quorum provider, told when the masternode phase is done.
    pub trust: OnceLock<Arc<crate::platform::trust::TrustProvider>>,
    /// The engine's prices (MP-01): a mainnet session stamps the
    /// transactions it first sees with the fresh price (rates-engine §6).
    rates: OnceLock<std::sync::Weak<crate::rates::RatesHub>>,
    /// The lease table: its catch-up runs on each SPV peers 0 → >0
    /// transition (E0-04 H6 part 2), and its dispatch fence hears the
    /// wallet's positive network evidence (BL-76, [`Self::note_seen`]).
    leases: std::sync::OnceLock<std::sync::Weak<crate::platform::lease::LeaseTable>>,
    /// Starts the duplicate-coin reconcile of a wallet after its block
    /// events (`restore`); unset in hub-only tests.
    reconcile: OnceLock<Box<dyn Fn(WalletId) + Send + Sync>>,
    /// The wallets with a reconcile started and not yet running.
    reconcile_pending: Mutex<HashSet<WalletId>>,
    /// Drains the drops that waited for another wallet's open once a wallet
    /// is unloaded or forgotten (`tx_actions`, #818 S1); unset in hub-only
    /// tests.
    drain_drops: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Drops what the history built on a sweep's transactions
    /// (`tx_actions`, #739); unset in hub-only tests.
    swept_dead: OnceLock<Box<SweptDead>>,
    /// Drops the history's pending spends a new transaction shows its
    /// coin's owner never signed (`tx_actions`, #739); unset in hub-only
    /// tests.
    funded: OnceLock<Box<Funded>>,
    /// Runs on each SPV peers 0 -> >0 transition: the recorded sweep
    /// chunks are sent again (`coinjoin`); unset in hub-only tests.
    peers_up: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Reads a wallet's balances again from the manager, after the event
    /// being applied (`update_balance`).
    balance_reread: OnceLock<Box<dyn Fn(WalletId) + Send + Sync>>,
    /// Refreshes the held report of every loaded wallet whose drops hold a
    /// coin: a lock key-wallet heard is noted before a restart forgets it
    /// (#749, Q81c C1).
    held_refresh: OnceLock<Box<dyn Fn() + Send + Sync>>,
    /// Tests: the balance rereads spawned and not yet done.
    #[cfg(test)]
    rereads_running: Arc<std::sync::atomic::AtomicUsize>,
    /// Chooses the spend claims of the wallets again that learned of an
    /// InstantSend lock no event of theirs named, and stores the lock for
    /// them ([`Self::fan_out_lock`]); unset in hub-only tests.
    lock_rerun: OnceLock<Box<LockRerun>>,
    /// The InstantSend locks a wallet learned of without an event of its
    /// own whose store failed, run again at the next lock heard.
    unstored_locks: Mutex<HashMap<(WalletId, Txid), InstantLock>>,
    /// Records a beaten sweep chunk or a truncation in the sweep rows;
    /// unset in hub-only tests.
    sweep_news: OnceLock<SweepNewsHook>,
    /// The wallets whose sweep rows a truncation could not update: their
    /// rows are checked against the header chain on their next event.
    sweep_retry_due: Mutex<HashSet<WalletId>>,
    /// Checks a wallet's sweep rows again (`coinjoin` prune); unset in
    /// hub-only tests.
    sweep_retry: OnceLock<Box<dyn Fn(WalletId) + Send + Sync>>,
    /// The SPV header chain ([`HeaderSource`]); unset, no header answers.
    headers: RwLock<Option<HeaderSource>>,
    /// The last header tip a progress report gave: a lower one (a fork
    /// truncated the header chain, or Rebuild Index) checks every wallet
    /// for a truncation ([`Self::detect_truncation`]).
    header_tip: Mutex<Option<u32>>,
    /// Payments whose own block a wallet's ChainLock named, by wallet:
    /// their sent marks go at the next history flush.
    final_sends: Mutex<Vec<(WalletId, Vec<Txid>)>>,
    /// The running shielded pass's tree progress (`checked`, `total`), from
    /// its latest batch; cleared when the pass ends.
    shielded_scan: Mutex<Option<(u64, u64)>>,
    /// The wallets a shielded pass has synced this session: the first one
    /// establishes the account, which hosts hear even when it found nothing.
    shielded_synced: Mutex<HashSet<WalletId>>,
    /// The wallets whose last shielded pass failed, until one succeeds.
    shielded_failed: Mutex<HashSet<WalletId>>,
    /// The wallets a status read found ready (a ledger restored from the
    /// store), before any pass of this session scanned them.
    shielded_ready: Mutex<HashSet<WalletId>>,
    /// Each wallet's last Platform-address pass of this session; absent
    /// (pending) until a pass ran for it.
    platform_addresses: Mutex<HashMap<WalletId, crate::platform::AddressSync>>,
    /// The children of payments key-wallet's conflict sweep took, which it
    /// holds no record of, by wallet, each set with its winner and the
    /// swept payments, and the truncations applied when it was queued:
    /// dropped at the next reconcile (#170 case 4), unless a truncation
    /// came since (#891).
    dead_children: Mutex<Vec<DeadChildren>>,
    /// The drops that wait, by wallet: for another wallet's open (#818,
    /// S1), or, with no recheck, for key-wallet's risen height to be read
    /// again (#891). Each with its detection's evidence and what it was
    /// found at: taken at the wallet's next drop, which an open's end
    /// runs.
    deferred_drops: Mutex<Vec<(WalletId, DropTake, crate::tx_actions::DeadWork)>>,
    /// The beaten payments whose rechecks ran out while only the proof of
    /// their winner's block was missing, by wallet and root: rechecked with
    /// their detection's evidence at the wallet's next drop, which an
    /// applied ChainLock reaching the block starts (#818, Q99). This
    /// session's only.
    carried_drops: Mutex<BTreeMap<(WalletId, Txid), CarriedDrop>>,
    /// The coins each wallet's drops hold for good, as its open read them
    /// (`tx_actions::finish_marked_abandons`): claimed at its load.
    dropped_holds: Mutex<crate::tx_actions::DroppedHolds>,
    /// The inputs of each verified InstantSend lock heard on the network
    /// that spends a coin the drops hold, whether or not key-wallet hears
    /// it: the held report notes the lock on those coins (#749).
    held_locks: Mutex<HashMap<Txid, BTreeSet<dashcore::OutPoint>>>,
    /// The lowest height a fork truncated each wallet to since its held
    /// report last ran (`NetworkSession::refresh_held`): the spend notes
    /// above it that are not final go.
    held_forks: Mutex<HashMap<WalletId, u32>>,
    /// Coins given back by a sweep while the current client's retry map exists.
    rebroadcast_holds: Mutex<Option<HashMap<WalletId, BTreeSet<dashcore::OutPoint>>>>,
    /// Held through each drop of dead transactions
    /// (`NetworkSession::drop_dead`).
    pub(crate) drop_gate: tokio::sync::Mutex<()>,
    /// The restored mark's money gate and its holds (`restore_gate`).
    pub(crate) restore: crate::restore_gate::RestoreGate,
    /// Tests: the sweep row checks started, to wait for.
    #[cfg(test)]
    pub(crate) sweep_checks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Tests: the reconciles started, to wait for.
    #[cfg(test)]
    pub(crate) reconciles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Tests: the claim choices a lock started again, to wait for.
    #[cfg(test)]
    pub(crate) lock_reruns: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The session's replay records, told of each fork truncation.
    replay: OnceLock<std::sync::Weak<crate::replay_record::ReplayRecords>>,
    /// Tests: holds the next first-seen write before it runs.
    #[cfg(test)]
    pub(crate) pause_seen: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next wallet open before its replay is resumed.
    #[cfg(test)]
    pub(crate) pause_load: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next wallet close just before the hub lets it go.
    #[cfg(test)]
    pub(crate) pause_unload: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: runs once, when a load or watch-only import has published a
    /// wallet and before it widens the wallet's pools.
    #[cfg(test)]
    pub(crate) on_published: Mutex<Option<PublishedHook>>,
    /// Tests: holds the next wallet open before its spend claims are set.
    #[cfg(test)]
    pub(crate) pause_claims: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next spend-claim restore before it takes the
    /// wallet manager's write lock.
    #[cfg(test)]
    pub(crate) pause_spend_claims: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next history load once it has read the stored
    /// records, before it reads the wallet.
    #[cfg(test)]
    pub(crate) pause_history: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: the next history load fails to list the stored records.
    #[cfg(test)]
    pub(crate) fail_history_read: std::sync::atomic::AtomicBool,
    /// Tests: SPV's progress reports leave the sync reading as the test
    /// set it.
    #[cfg(test)]
    pin_progress: std::sync::atomic::AtomicBool,
    /// Tests: the next history load's read of the dropped records fails.
    #[cfg(test)]
    pub(crate) fail_dropped_read: std::sync::atomic::AtomicBool,
    /// Tests: the next history load's read of the store's put-back claims
    /// fails.
    #[cfg(test)]
    pub(crate) fail_restored_read: std::sync::atomic::AtomicBool,
    /// Tests: the next history load's first stored record read goes wrong.
    #[cfg(test)]
    pub(crate) record_read_fault: Mutex<Option<crate::history_ops::RecordFault>>,
    /// Tests: the next history load (`.0`) or stored spend-claims read
    /// (`.1`) cannot read this transaction's record.
    #[cfg(test)]
    pub(crate) unreadable_record: Mutex<(Option<dashcore::Txid>, Option<dashcore::Txid>)>,
    #[cfg(test)]
    pub(crate) pause_abandon: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next read of a masternode's operations once it has
    /// judged their states, before it returns them.
    #[cfg(test)]
    pub(crate) pause_ops_read: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next Activity Resend once it has checked the
    /// transaction, before its hand-off.
    #[cfg(test)]
    pub(crate) pause_resend: Mutex<Option<Arc<crate::coins::TestPause>>>,
    #[cfg(test)]
    pub(crate) pause_rebroadcast_rearm: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: each txid an abandon stopped dash-spv rebroadcasting, with
    /// whether its abandon mark was already written then.
    #[cfg(test)]
    pub(crate) stopped_rebroadcasts: Mutex<Vec<(Txid, bool)>>,
    /// Tests: takes the bytes Activity Resend hands off, in place of the
    /// network.
    #[cfg(test)]
    pub(crate) resend_wire: Mutex<Option<Arc<Mutex<Vec<Txid>>>>>,
    /// Tests: the next abandon quits before its sweep, as at an exit.
    #[cfg(test)]
    pub(crate) quit_before_sweep: std::sync::atomic::AtomicBool,
    /// Tests: how many sweep events' drops of what is built on them ran.
    #[cfg(test)]
    pub(crate) dead_hooks_done: std::sync::atomic::AtomicUsize,
    /// Tests: new transactions don't start the check for unsigned claims.
    #[cfg(test)]
    pub(crate) skip_unsigned_check: std::sync::atomic::AtomicBool,
    /// Tests: no payment's outputs are withheld
    /// ([`crate::withhold::pending_unproven`]), as in a build before the
    /// withhold, for a test that sets up the store such a build left.
    #[cfg(test)]
    pub(crate) skip_withhold: std::sync::atomic::AtomicBool,
    /// Tests: a sweep doesn't start the drop of what is built on it, as at
    /// an exit right after the sweep.
    #[cfg(test)]
    pub(crate) skip_dead_hook: std::sync::atomic::AtomicBool,
    /// Tests: key-wallet refuses every drop of a payment that can never
    /// confirm, as when it settled the root meanwhile.
    #[cfg(test)]
    pub(crate) refuse_drops: std::sync::atomic::AtomicBool,
    /// Tests: how many drops [`Self::refuse_drops`] refused.
    #[cfg(test)]
    pub(crate) drops_refused: std::sync::atomic::AtomicUsize,
    /// Tests: the next abandon returns after its sweep, without its rescan.
    #[cfg(test)]
    pub(crate) quit_before_rescan: std::sync::atomic::AtomicBool,
    /// Tests: holds the next abandon after its sweep, before its rescan.
    #[cfg(test)]
    pub(crate) pause_before_rescan: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: how many abandons pass before [`Self::pause_abandon`] holds one.
    #[cfg(test)]
    pub(crate) pause_abandon_skip: std::sync::atomic::AtomicUsize,
    /// Tests: how long the last abandon held the wallet manager.
    #[cfg(test)]
    pub(crate) last_abandon_hold: Mutex<Option<std::time::Duration>>,
    /// Tests: holds the next abandon between its plan and its sweep.
    #[cfg(test)]
    pub(crate) pause_sweep: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: the next abandon of this root fails once its sweep is stored.
    #[cfg(test)]
    pub(crate) fail_after_sweep_of: Mutex<Option<dashcore::Txid>>,
    /// Tests: the coins the last abandon's sweep released.
    #[cfg(test)]
    pub(crate) last_release: Mutex<Option<Vec<dashcore::OutPoint>>>,
    /// Tests: holds the next abandon after its marks are written, before
    /// its sweep.
    #[cfg(test)]
    pub(crate) pause_abandon_sweep: Mutex<Option<Arc<crate::coins::TestPause>>>,
    #[cfg(test)]
    pub(crate) pause_shared_abandon_sign: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next dropped payment's release of key-wallet's
    /// reservation, before it runs.
    #[cfg(test)]
    pub(crate) pause_release: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: the next abandon's write of its abandoned marks fails.
    #[cfg(test)]
    pub(crate) fail_abandon_mark: std::sync::atomic::AtomicBool,
    /// Tests: key-wallet refuses the next abandon's root (abandons
    /// nothing), as it does a root it holds as settled.
    #[cfg(test)]
    pub(crate) refuse_next_abandon: std::sync::atomic::AtomicBool,
    /// Tests: abandons key-wallet refused after their marks were written.
    #[cfg(test)]
    pub(crate) refused_abandon_undos: std::sync::atomic::AtomicUsize,
    /// Tests: the undo write of the next abandon key-wallet refuses fails.
    #[cfg(test)]
    pub(crate) fail_abandon_undo: std::sync::atomic::AtomicBool,
    /// Tests: the next abandon's store of its sweep fails.
    #[cfg(test)]
    pub(crate) fail_abandon_sweep: std::sync::atomic::AtomicBool,
    /// Tests: Remove unconfirmed transactions reads this wallet's sent marks
    /// as not read.
    #[cfg(test)]
    pub(crate) fail_sent_read: Mutex<Option<WalletId>>,
    /// Tests: holds the next payment's metadata write after its wallet read.
    #[cfg(test)]
    pub(crate) pause_metadata: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next resume between its record read and its rewind.
    #[cfg(test)]
    pub(crate) pause_resume: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next rescan's publication, after its rewind.
    #[cfg(test)]
    pub(crate) pause_publish: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next drop of beaten payments between their
    /// detection and its recheck under the write lock.
    #[cfg(test)]
    pub(crate) pause_drop_recheck: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next drop of a swept payment's children after it
    /// took them, before the write lock.
    #[cfg(test)]
    pub(crate) pause_drop_child: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next drop with no recheck and no swept payment (an
    /// orphan) after it took it, before the write lock.
    #[cfg(test)]
    pub(crate) pause_drop_orphan: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next drop of beaten payments after it read its
    /// block's proof, before the write lock.
    #[cfg(test)]
    pub(crate) pause_drop_proven: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: as [`Self::pause_drop_proven`], at a payment's last recheck.
    #[cfg(test)]
    pub(crate) pause_drop_proven_last: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next truncation between its heights and the rest.
    #[cfg(test)]
    pub(crate) pause_truncation: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
    /// Held through each fork truncation's application
    /// ([`Self::apply_truncation`]), and through a check for one
    /// ([`Self::detect_truncation`]): a check waits for the heights and
    /// the history of one being applied, not only its heights.
    truncating: Mutex<()>,
    /// Rescans cancelled in this session: a caller that started one tells
    /// from it that its rescan was cancelled, even when another replaced it.
    pub(crate) rescan_cancels: std::sync::atomic::AtomicU64,
    /// How many fork truncations the hub applied, to any wallet, this
    /// session; never lowered. A beaten payment's drop reads it at its
    /// detection and again under the write lock
    /// (`NetworkSession::drop_dead_unmined`).
    truncations: std::sync::atomic::AtomicU64,
    /// The wallets unloaded or removed, and not added again: an event of
    /// one is stale, from before. Held through each event's application
    /// and through a wallet's unload, removal and admission, so an event
    /// is applied wholly before the wallet leaves or not at all; taken
    /// before every other lock of the hub's.
    unloaded: Mutex<HashSet<WalletId>>,
    /// Wallets with a Remove Unconfirmed batch, or a single Abandon, whose
    /// rescan is not made yet.
    abandon_batches: Mutex<HashMap<WalletId, usize>>,
    /// Tests: holds the next event right after its wallet's check.
    #[cfg(test)]
    pub(crate) pause_event: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
}

#[cfg(test)]
pub(crate) type PublishedHook = Box<dyn FnOnce(crate::WalletId) -> bool + Send>;

/// A Remove Unconfirmed batch, or a single Abandon, in progress
/// ([`SessionHub::abandon_batch`]).
pub(crate) struct AbandonBatch<'a> {
    hub: &'a SessionHub,
    wallet: WalletId,
}

impl Drop for AbandonBatch<'_> {
    fn drop(&mut self) {
        let done = {
            let mut batches = (self.hub.abandon_batches.lock()).unwrap_or_else(|p| p.into_inner());
            let done = (batches.get_mut(&self.wallet)).is_some_and(|n| {
                *n -= 1;
                *n == 0
            });
            if done {
                batches.remove(&self.wallet);
            }
            done
        };
        if done {
            self.hub.mark_drops(self.wallet);
        }
    }
}

impl SessionHub {
    /// Tests: runs [`Self::on_published`], if set, for wallet `id`. A load
    /// stops there, as a quit would, when it returns `true`.
    #[cfg(test)]
    pub(crate) fn run_on_published(&self, id: crate::WalletId) -> bool {
        let hook = self.on_published.lock().unwrap().take();
        hook.is_some_and(|hook| hook(id))
    }

    /// The running shielded pass's tree progress, once a batch proved the
    /// pool's size.
    pub(crate) fn shielded_scan(&self) -> Option<crate::platform::ShieldedScanProgress> {
        let scan = *self.shielded_scan.lock().unwrap_or_else(|e| e.into_inner());
        scan.filter(|&(_, total)| total > 0)
            .map(|(checked, total)| crate::platform::ShieldedScanProgress {
                checked: checked.min(total),
                total,
            })
    }

    /// `wallet`'s Shielded ledger is established this session: a pass
    /// scanned it, or a status read found it ready.
    pub(crate) fn shielded_established(&self, wallet: WalletId) -> bool {
        lock(&self.shielded_synced).contains(&wallet)
            || lock(&self.shielded_ready).contains(&wallet)
    }

    /// A status read found `wallet`'s ledger ready.
    pub(crate) fn note_shielded_ready(&self, wallet: WalletId) {
        lock(&self.shielded_ready).insert(wallet);
    }

    /// `wallet`'s last Platform-address pass of this session.
    pub(crate) fn platform_addresses(&self, wallet: WalletId) -> crate::platform::AddressSync {
        (lock(&self.platform_addresses).get(&wallet).copied())
            .unwrap_or(crate::platform::AddressSync::Pending)
    }

    /// `wallet`'s last shielded pass of this session failed.
    pub(crate) fn shielded_pass_failed(&self, wallet: WalletId) -> bool {
        lock(&self.shielded_failed).contains(&wallet)
    }

    pub(crate) fn set_rates(&self, rates: std::sync::Weak<crate::rates::RatesHub>) {
        let _ = self.rates.set(rates);
    }

    /// The price to stamp a transaction first seen now with: on Mainnet,
    /// while the chosen currency's price is fresh.
    pub(crate) fn stamp_rate(&self) -> Option<crate::rates::FiatRate> {
        if self.network != DashNetwork::Mainnet {
            return None;
        }
        self.rates.get()?.upgrade()?.fresh_rate()
    }

    /// Where stamps are written, after [`Self::stamp_rate`]: answers whether
    /// they still may be, so an Off between the read and the write stamps
    /// nothing. Read it before the rate.
    pub(crate) fn stamp_check(&self) -> impl Fn() -> bool + Send + 'static {
        let rates = self.rates.get().cloned().unwrap_or_default();
        let epoch = rates.upgrade().map(|h| h.off_epoch());
        move || match (rates.upgrade(), epoch) {
            (Some(hub), Some(epoch)) => hub.stamps_still_allowed(epoch),
            _ => false,
        }
    }

    /// The stamps for `txids` of `wallet`: each one whose time, the earlier
    /// of its first sight and its block's, is at most [`STAMP_MAX_AGE`] ago,
    /// so a rescan, a restore or a payment seen while no price was fresh is
    /// never stamped with a much later price.
    pub(crate) fn price_stamps(
        &self,
        rate: &crate::rates::FiatRate,
        wallet: &WalletId,
        txids: &[Txid],
        now: u64,
    ) -> Vec<dw_appdb::TxFiat> {
        self.history.read(wallet, |txs| {
            txids
                .iter()
                .filter(|t| {
                    txs.get(*t).is_some_and(|e| {
                        let block = e.context.block_info().map(|b| u64::from(b.timestamp()));
                        e.first_seen
                            .into_iter()
                            .chain(block)
                            .min()
                            .is_none_or(|time| now.saturating_sub(time) <= STAMP_MAX_AGE)
                    })
                })
                .map(|t| dw_appdb::TxFiat {
                    txid: t.to_string(),
                    currency: rate.currency.clone(),
                    price: rate.price.clone(),
                    rate_at: rate.rate_at,
                    source: rate.source.stored().to_owned(),
                })
                .collect()
        })
    }

    pub(crate) fn new(network: DashNetwork, sink: Arc<dyn EventSink>) -> Self {
        Self {
            network,
            sink,
            tracker: Mutex::new(SyncTracker::default()),
            pump: EventPump::default(),
            history: HistoryStore::default(),
            appdb: std::sync::OnceLock::new(),
            wallets: RwLock::new(BTreeMap::new()),
            names: RwLock::new(HashMap::new()),
            rescan: Mutex::new(None),
            chainlock: Mutex::new(None),
            announced: Mutex::new(HashMap::new()),
            trust: OnceLock::new(),
            rates: OnceLock::new(),
            leases: std::sync::OnceLock::new(),
            reconcile: OnceLock::new(),
            reconcile_pending: Mutex::new(HashSet::new()),
            drain_drops: OnceLock::new(),
            swept_dead: OnceLock::new(),
            funded: OnceLock::new(),
            peers_up: OnceLock::new(),
            balance_reread: OnceLock::new(),
            held_refresh: OnceLock::new(),
            #[cfg(test)]
            rereads_running: Arc::default(),
            lock_rerun: OnceLock::new(),
            unstored_locks: Mutex::new(HashMap::new()),
            sweep_news: OnceLock::new(),
            sweep_retry_due: Mutex::new(HashSet::new()),
            sweep_retry: OnceLock::new(),
            headers: RwLock::new(None),
            header_tip: Mutex::new(None),
            final_sends: Mutex::new(Vec::new()),
            shielded_scan: Mutex::new(None),
            shielded_synced: Mutex::new(HashSet::new()),
            shielded_failed: Mutex::new(HashSet::new()),
            shielded_ready: Mutex::new(HashSet::new()),
            platform_addresses: Mutex::new(HashMap::new()),
            dead_children: Mutex::new(Vec::new()),
            deferred_drops: Mutex::new(Vec::new()),
            carried_drops: Mutex::new(BTreeMap::new()),
            dropped_holds: Mutex::new(HashMap::new()),
            held_locks: Mutex::new(HashMap::new()),
            held_forks: Mutex::new(HashMap::new()),
            rebroadcast_holds: Mutex::new(None),
            drop_gate: tokio::sync::Mutex::new(()),
            restore: Default::default(),
            #[cfg(test)]
            sweep_checks: Mutex::new(Vec::new()),
            #[cfg(test)]
            reconciles: Mutex::new(Vec::new()),
            #[cfg(test)]
            lock_reruns: Mutex::new(Vec::new()),
            replay: OnceLock::new(),
            #[cfg(test)]
            pause_seen: Mutex::new(None),
            #[cfg(test)]
            pause_load: Mutex::new(None),
            #[cfg(test)]
            pause_unload: Mutex::new(None),
            #[cfg(test)]
            on_published: Mutex::new(None),
            #[cfg(test)]
            pause_claims: Mutex::new(None),
            #[cfg(test)]
            pause_spend_claims: Mutex::new(None),
            #[cfg(test)]
            pause_history: Mutex::new(None),
            #[cfg(test)]
            pause_ops_read: Mutex::new(None),
            #[cfg(test)]
            pause_resend: Mutex::new(None),
            #[cfg(test)]
            pause_rebroadcast_rearm: Mutex::new(None),
            #[cfg(test)]
            stopped_rebroadcasts: Mutex::new(Vec::new()),
            #[cfg(test)]
            resend_wire: Mutex::new(None),
            #[cfg(test)]
            fail_history_read: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            pin_progress: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_dropped_read: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_restored_read: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            record_read_fault: Mutex::new(None),
            #[cfg(test)]
            unreadable_record: Mutex::default(),
            #[cfg(test)]
            pause_abandon: Mutex::new(None),
            #[cfg(test)]
            quit_before_sweep: Default::default(),
            #[cfg(test)]
            dead_hooks_done: Default::default(),
            #[cfg(test)]
            skip_unsigned_check: Default::default(),
            #[cfg(test)]
            skip_withhold: Default::default(),
            #[cfg(test)]
            skip_dead_hook: Default::default(),
            #[cfg(test)]
            refuse_drops: Default::default(),
            #[cfg(test)]
            drops_refused: Default::default(),
            #[cfg(test)]
            quit_before_rescan: Default::default(),
            #[cfg(test)]
            pause_before_rescan: Mutex::new(None),
            #[cfg(test)]
            last_release: Mutex::new(None),
            #[cfg(test)]
            fail_after_sweep_of: Mutex::new(None),
            #[cfg(test)]
            pause_sweep: Mutex::new(None),
            #[cfg(test)]
            pause_abandon_skip: Default::default(),
            #[cfg(test)]
            last_abandon_hold: Mutex::new(None),
            #[cfg(test)]
            pause_abandon_sweep: Mutex::new(None),
            #[cfg(test)]
            pause_shared_abandon_sign: Mutex::new(None),
            #[cfg(test)]
            pause_release: Mutex::new(None),
            #[cfg(test)]
            fail_abandon_mark: Default::default(),
            #[cfg(test)]
            refuse_next_abandon: Default::default(),
            #[cfg(test)]
            refused_abandon_undos: Default::default(),
            #[cfg(test)]
            fail_abandon_undo: Default::default(),
            #[cfg(test)]
            fail_abandon_sweep: Default::default(),
            #[cfg(test)]
            fail_sent_read: Mutex::new(None),
            #[cfg(test)]
            pause_metadata: Mutex::new(None),
            #[cfg(test)]
            pause_resume: Mutex::new(None),
            #[cfg(test)]
            pause_publish: Mutex::new(None),
            #[cfg(test)]
            pause_drop_recheck: Mutex::new(None),
            #[cfg(test)]
            pause_drop_child: Mutex::new(None),
            #[cfg(test)]
            pause_drop_orphan: Mutex::new(None),
            #[cfg(test)]
            pause_drop_proven: Mutex::new(None),
            #[cfg(test)]
            pause_drop_proven_last: Mutex::new(None),
            #[cfg(test)]
            pause_truncation: Mutex::new(None),
            truncating: Mutex::new(()),
            rescan_cancels: std::sync::atomic::AtomicU64::new(0),
            truncations: std::sync::atomic::AtomicU64::new(0),
            unloaded: Mutex::new(HashSet::new()),
            abandon_batches: Mutex::new(HashMap::new()),
            #[cfg(test)]
            pause_event: Mutex::new(None),
        }
    }

    /// Runs `leases`' resume pass on every SPV peers 0 → >0 transition and
    /// feeds its fence the wallet's sightings ([`Self::note_seen`]).
    pub(crate) fn attach_leases(&self, leases: &Arc<crate::platform::lease::LeaseTable>) {
        let _ = self.leases.set(Arc::downgrade(leases));
    }

    fn lease_table(&self) -> Option<Arc<crate::platform::lease::LeaseTable>> {
        self.leases.get().and_then(std::sync::Weak::upgrade)
    }

    /// Sets what reads a wallet's balances again (once).
    pub(crate) fn attach_balance_reread(&self, run: Box<dyn Fn(WalletId) + Send + Sync>) {
        let _ = self.balance_reread.set(run);
    }

    /// Sets what refreshes the held reports of the wallets with holds
    /// (once).
    pub(crate) fn attach_held_refresh(&self, run: Box<dyn Fn() + Send + Sync>) {
        let _ = self.held_refresh.set(run);
    }

    /// The loaded wallets whose drops hold a coin.
    pub(crate) fn wallets_with_holds(&self) -> Vec<WalletId> {
        let holds = self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner());
        (holds.iter())
            .filter(|(_, held)| !held.is_empty())
            .map(|(id, _)| *id)
            .collect()
    }

    /// A verified lock was heard on the network: when it spends a coin a
    /// drop holds, its inputs are kept for the held report, which notes
    /// it on those coins, though key-wallet may never hear it (#749).
    fn held_lock_heard(&self, lock: &InstantLock) {
        let inputs: BTreeSet<dashcore::OutPoint> = lock.inputs.iter().copied().collect();
        let hit = {
            let holds = self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner());
            (holds.values()).any(|held| held.iter().any(|o| inputs.contains(o)))
        };
        if !hit {
            return;
        }
        (self.held_locks.lock().unwrap_or_else(|p| p.into_inner())).insert(lock.txid, inputs);
        if let Some(refresh) = self.held_refresh.get() {
            refresh();
        }
    }

    /// The verified locks heard on the network that spend a coin of
    /// `holds`, each with the coins of `holds` it spends.
    pub(crate) fn held_locks(
        &self,
        holds: &BTreeSet<dashcore::OutPoint>,
    ) -> HashMap<Txid, BTreeSet<dashcore::OutPoint>> {
        let locks = self.held_locks.lock().unwrap_or_else(|p| p.into_inner());
        (locks.iter())
            .map(|(t, inputs)| {
                (
                    *t,
                    inputs.intersection(holds).copied().collect::<BTreeSet<_>>(),
                )
            })
            .filter(|(_, spent)| !spent.is_empty())
            .collect()
    }

    /// A lock was heard: key-wallet adds it to every wallet's locks, so
    /// each wallet with holds notes it if it may spend a held coin.
    fn lock_heard(&self) {
        if self.wallets_with_holds().is_empty() {
            return;
        }
        if let Some(refresh) = self.held_refresh.get() {
            refresh();
        }
    }

    /// Tests: counts a balance reread as running until the guard drops.
    #[cfg(test)]
    pub(crate) fn reread_started(&self) -> RereadRunning {
        let n = Arc::clone(&self.rereads_running);
        n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        RereadRunning(n)
    }

    /// Tests: waits up to 10 s for the balance rereads the events applied
    /// so far spawned, so a read after them sees what they read.
    #[cfg(test)]
    pub(crate) fn settle_rereads(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self
            .rereads_running
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
        {
            assert!(
                std::time::Instant::now() < deadline,
                "a balance reread never ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Sets what chooses the spend claims again after a lock (once).
    pub(crate) fn attach_lock_rerun(&self, run: Box<LockRerun>) {
        let _ = self.lock_rerun.set(run);
    }

    /// Tests: the claim choices a lock started again so far.
    #[cfg(test)]
    pub(crate) fn take_lock_reruns(&self) -> Vec<tokio::task::JoinHandle<()>> {
        std::mem::take(&mut *self.lock_reruns.lock().unwrap_or_else(|p| p.into_inner()))
    }

    fn unstored_locks(&self) -> std::sync::MutexGuard<'_, HashMap<(WalletId, Txid), InstantLock>> {
        self.unstored_locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    /// Keeps `lock` of `txid` for `wallet` until its store takes it.
    pub(crate) fn note_unstored_lock(&self, wallet: WalletId, txid: Txid, lock: InstantLock) {
        self.unstored_locks().insert((wallet, txid), lock);
    }

    /// The locks whose store failed so far, taken to be written again.
    pub(crate) fn take_unstored_locks(&self) -> HashMap<(WalletId, Txid), InstantLock> {
        std::mem::take(&mut *self.unstored_locks())
    }

    /// An InstantSend lock: each loaded wallet but those
    /// `unloaded` whose history holds it out of a block without a lock
    /// shows it locked, and its spend claims are chosen again as at its
    /// load. key-wallet tells only the wallets holding the transaction's
    /// record or coins, and no wallet when it pays none of them (#828).
    /// Only SPV's verified event can add the lock to the reorg fallback.
    fn fan_out_lock(&self, lock: &InstantLock, unloaded: &HashSet<WalletId>, verified: bool) {
        let txid = lock.txid;
        let inputs = verified.then_some(lock.inputs.as_slice());
        let wallets = self.history.note_instant_lock(&txid, inputs, unloaded);
        if wallets.is_empty() && self.unstored_locks().is_empty() {
            return;
        }
        for id in &wallets {
            self.pump.mark_history(*id, Some(&[txid]));
        }
        if let Some(rerun) = self.lock_rerun.get() {
            rerun(txid, lock.clone(), wallets);
        }
    }

    /// Sets what runs on each SPV peers 0 -> >0 transition (once).
    pub(crate) fn attach_peers_up(&self, run: Box<dyn Fn() + Send + Sync>) {
        let _ = self.peers_up.set(run);
    }

    /// Sets what records the sweep records' news (once).
    pub(crate) fn attach_sweep_news(&self, note: SweepNewsHook) {
        let _ = self.sweep_news.set(note);
    }

    /// Sets what checks a wallet's sweep rows again (once).
    pub(crate) fn attach_sweep_retry(&self, retry: Box<dyn Fn(WalletId) + Send + Sync>) {
        let _ = self.sweep_retry.set(retry);
    }

    /// Sets the SPV header chain the sweep records' evidence is checked
    /// against.
    pub(crate) fn attach_headers(&self, headers: HeaderSource) {
        *self.headers.write().unwrap_or_else(|p| p.into_inner()) = Some(headers);
    }

    /// The hash of the block the SPV header chain holds at `height`, if the
    /// header store has one.
    pub(crate) async fn header_hash(&self, height: u32) -> Option<dashcore::BlockHash> {
        let source = self
            .headers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        source(height).await
    }

    /// [`Self::header_hash`] of each of `heights` that has one.
    pub(crate) async fn header_hashes(
        &self,
        heights: BTreeSet<u32>,
    ) -> HashMap<u32, dashcore::BlockHash> {
        let mut found = HashMap::new();
        for h in heights {
            if let Some(hash) = self.header_hash(h).await {
                found.insert(h, hash);
            }
        }
        found
    }

    /// Tests: the sweep row checks started so far.
    #[cfg(test)]
    pub(crate) fn take_sweep_checks(&self) -> Vec<tokio::task::JoinHandle<()>> {
        std::mem::take(&mut *self.sweep_checks.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Checks wallet `id`'s sweep rows again on its next event.
    pub(crate) fn retry_sweep_rows(&self, id: WalletId) {
        (self
            .sweep_retry_due
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .insert(id);
    }

    /// Hands `news` of wallet `id` to the sweep records; a change can free
    /// or hold coins, so the balances are read again. A failed write is
    /// checked again on the wallet's next event.
    fn note_sweep_news(&self, id: WalletId, news: crate::coinjoin::SweepNews<'_>) {
        let Some(note) = self.sweep_news.get() else {
            return;
        };
        match note(id, news) {
            Some(true) => self.pump.mark_balances(id),
            Some(false) => {}
            None => self.retry_sweep_rows(id),
        }
    }

    /// Wallet `id`'s due check of its sweep rows ([`Self::retry_sweep_rows`]).
    fn run_sweep_retry(&self, id: WalletId) {
        let due = (self
            .sweep_retry_due
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .remove(&id);
        if due && let Some(retry) = self.sweep_retry.get() {
            retry(id);
        }
    }

    /// Installs the session's replay records (once).
    pub(crate) fn attach_replay(&self, replay: &Arc<crate::replay_record::ReplayRecords>) {
        let _ = self.replay.set(Arc::downgrade(replay));
    }

    /// Sets what starts a wallet's duplicate-coin reconcile (once).
    pub(crate) fn attach_reconcile(&self, start: Box<dyn Fn(WalletId) + Send + Sync>) {
        let _ = self.reconcile.set(start);
    }

    /// Sets what drains the drops that waited for another wallet's open
    /// (once).
    pub(crate) fn attach_drain_drops(&self, start: Box<dyn Fn() + Send + Sync>) {
        let _ = self.drain_drops.set(start);
    }

    /// The drops that waited for a wallet the hub no longer holds go: it
    /// was all that kept them, and no open of it ends to drain them.
    fn drain_drops(&self) {
        if let Some(start) = self.drain_drops.get() {
            start();
        }
    }

    /// Sets what drops the history's transactions built on swept ones
    /// (once).
    pub(crate) fn attach_swept_dead(&self, start: Box<SweptDead>) {
        let _ = self.swept_dead.set(start);
    }

    /// Sets what checks the history's pending spends of a new
    /// transaction's outputs (once).
    pub(crate) fn attach_funded(&self, start: Box<Funded>) {
        let _ = self.funded.set(start);
    }

    /// Starts the check for unsigned claims on what `funded` pays, when a
    /// pending, unabandoned transaction of the history spends an output of
    /// one: most new transactions fund none, and start no task.
    fn check_funded(&self, id: WalletId, funded: BTreeSet<Txid>) {
        #[cfg(test)]
        if self
            .skip_unsigned_check
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let Some(start) = self.funded.get() else {
            return;
        };
        let spent = self.history.read_wallet(&id, |h| {
            h.txs.values().any(|e| {
                !e.is_abandoned()
                    && e.context.block_info().is_none()
                    && (e.tx.input.iter()).any(|i| funded.contains(&i.previous_output.txid))
            })
        });
        if spent {
            start(id, funded);
        }
    }

    /// Starts wallet `id`'s reconcile unless one is started and not yet
    /// running: events during a run start the next one.
    pub(crate) fn schedule_reconcile(&self, id: WalletId) {
        let Some(start) = self.reconcile.get() else {
            return;
        };
        if self
            .reconcile_pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id)
        {
            start(id);
        }
    }

    /// Records header tip `tip`; whether it is below the last one.
    fn header_tip_dropped(&self, tip: u32) -> bool {
        let mut last = self.header_tip.lock().unwrap_or_else(|p| p.into_inner());
        last.replace(tip).is_some_and(|last| tip < last)
    }

    /// The children of swept payments queued for wallet `id`'s drop: each
    /// set with the truncations applied when it was queued, its winner, the
    /// swept payments, and their inputs.
    pub(crate) fn take_dead_children(&self, id: WalletId) -> Vec<(u64, DeadSet)> {
        let mut queued = (self.dead_children.lock()).unwrap_or_else(|p| p.into_inner());
        let (mine, others) = std::mem::take(&mut *queued)
            .into_iter()
            .partition(|(w, ..)| *w == id);
        *queued = others;
        (mine.into_iter()).map(|(_, t, set)| (t, set)).collect()
    }

    /// Keeps `work`, drops of wallet `id` that wait for another wallet's
    /// open or for the chain to be read again, each with the take it was
    /// found at; one already waiting is kept once.
    pub(crate) fn defer_drops(
        &self,
        id: WalletId,
        work: impl IntoIterator<Item = (DropTake, crate::tx_actions::DeadWork)>,
    ) {
        // Under the lock an unload forgets them under: none outlives it.
        let unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        if unloaded.contains(&id) {
            return;
        }
        let mut deferred = (self.deferred_drops.lock()).unwrap_or_else(|p| p.into_inner());
        for (take, w) in work {
            let waiting = |(d, t, waiting): &(WalletId, DropTake, crate::tx_actions::DeadWork)| {
                (*d, *t) == (id, take) && *waiting == w
            };
            if !deferred.iter().any(waiting) {
                deferred.push((id, take, w));
            }
        }
    }

    /// The drops of wallet `id` that waited, each with the take it was
    /// found at.
    pub(crate) fn take_deferred_drops(
        &self,
        id: WalletId,
    ) -> Vec<(DropTake, crate::tx_actions::DeadWork)> {
        let mut deferred = (self.deferred_drops.lock()).unwrap_or_else(|p| p.into_inner());
        let (mine, others) = std::mem::take(&mut *deferred)
            .into_iter()
            .partition(|(w, ..)| *w == id);
        *deferred = others;
        mine.into_iter().map(|(_, t, w)| (t, w)).collect()
    }

    /// The wallets with drops that wait.
    pub(crate) fn deferred_drop_wallets(&self) -> BTreeSet<WalletId> {
        let deferred = (self.deferred_drops.lock()).unwrap_or_else(|p| p.into_inner());
        deferred.iter().map(|(w, ..)| *w).collect()
    }

    /// Keeps `roots` of wallet `id`, beaten payments whose rechecks ran out
    /// while only their proof was missing, with `drop`; a root kept already
    /// is kept once, with the later detection.
    pub(crate) fn carry_drops(&self, id: WalletId, roots: &[Txid], drop: CarriedDrop) {
        let unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        if unloaded.contains(&id) {
            return;
        }
        let mut carried = (self.carried_drops.lock()).unwrap_or_else(|p| p.into_inner());
        for t in roots {
            carried.insert((id, *t), drop.clone());
        }
    }

    /// The beaten payments of wallet `id` kept to recheck, by root.
    pub(crate) fn take_carried_drops(&self, id: WalletId) -> Vec<(Txid, CarriedDrop)> {
        let mut carried = (self.carried_drops.lock()).unwrap_or_else(|p| p.into_inner());
        let (mine, others): (BTreeMap<_, _>, BTreeMap<_, _>) = std::mem::take(&mut *carried)
            .into_iter()
            .partition(|((w, _), _)| *w == id);
        *carried = others;
        mine.into_iter().map(|((_, t), d)| (t, d)).collect()
    }

    /// The roots wallet `id` keeps to recheck (tests only).
    #[cfg(test)]
    pub(crate) fn carried_drop_roots(&self, id: WalletId) -> BTreeSet<Txid> {
        let carried = (self.carried_drops.lock()).unwrap_or_else(|p| p.into_inner());
        (carried.keys())
            .filter(|(w, _)| *w == id)
            .map(|(_, t)| *t)
            .collect()
    }

    /// Whether wallet `id`'s ChainLock applied at `height` reaches the block
    /// of a payment it keeps to recheck, above what that recheck read.
    fn carried_due(&self, id: WalletId, height: u32) -> bool {
        let carried = (self.carried_drops.lock()).unwrap_or_else(|p| p.into_inner());
        (carried.iter()).any(|((w, _), (.., at, seen))| {
            *w == id && *at <= height && seen.is_none_or(|s| height > s)
        })
    }

    /// Keeps the drops' holds an open read, by wallet.
    pub(crate) fn note_dropped_holds(&self, holds: crate::tx_actions::DroppedHolds) {
        (self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner())).extend(holds);
    }

    /// Adds the coins a drop of wallet `id` noted it holds.
    pub(crate) fn add_dropped_holds(
        &self,
        id: WalletId,
        held: impl IntoIterator<Item = dashcore::OutPoint>,
    ) {
        let mut holds = self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner());
        holds.entry(id).or_default().extend(held);
    }

    /// A fork truncated wallet `id` to `height`: its next held report drops
    /// the spend notes above it that are not final.
    pub(crate) fn note_held_fork(&self, id: WalletId, height: u32) {
        let mut forks = self.held_forks.lock().unwrap_or_else(|p| p.into_inner());
        let fork = forks.entry(id).or_insert(height);
        *fork = (*fork).min(height);
    }

    /// The lowest height a fork truncated wallet `id` to since this was last
    /// taken.
    pub(crate) fn take_held_fork(&self, id: WalletId) -> Option<u32> {
        (self.held_forks.lock().unwrap_or_else(|p| p.into_inner())).remove(&id)
    }

    /// The coins wallet `id`'s drops hold for good, as its open read them.
    pub(crate) fn dropped_holds(&self, id: WalletId) -> BTreeSet<dashcore::OutPoint> {
        let holds = self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner());
        holds.get(&id).cloned().unwrap_or_default()
    }

    /// A reconcile of wallet `id` begins: later events start another.
    pub(crate) fn reconcile_running(&self, id: WalletId) {
        self.reconcile_pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// Publishes key-wallet's own balances for wallet `id` (after the engine
    /// changed its coins), read from `core` under the manager's lock with
    /// the excluded coins' share, and announces them when they differ.
    pub(crate) fn publish_core_balance(&self, id: WalletId, core: &ManagedWalletInfo) {
        let excluded = self.excluded_share(id, core);
        let held = self.restore_held_in(id, core, &excluded);
        let (balance, accounts) = (core.balance(), core.account_balances());
        self.publish_balances(id, &balance, &accounts, Some(excluded), Some(held));
    }

    fn publish_balances(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
        excluded: Option<crate::withhold::Excluded>,
        restore_held: Option<u64>,
    ) {
        let changed = self.write_balances(id, balance, accounts, excluded, restore_held);
        self.reread_if_locked(id, balance, accounts);
        if changed {
            self.pump.mark_balances(id);
        }
    }

    /// BL-76 (E0-04 H12, §4.6): `txids` carry positive network evidence
    /// for wallet `id`: an InstantSend lock, a block that holds them, or a
    /// ChainLock over that block. Each is `Sent` for the dispatch fence
    /// ([`LeaseTable::note_seen`](crate::platform::lease::LeaseTable::note_seen),
    /// which ignores an artifact the wallet never dispatched). A mempool
    /// sighting is not evidence: the wallet records its own transactions
    /// before any peer took them.
    fn note_seen(&self, id: WalletId, txids: impl IntoIterator<Item = Txid>) {
        let Some(leases) = self.lease_table() else {
            return;
        };
        let artifacts = txids
            .into_iter()
            .map(|t| crate::platform::lease::ArtifactId::from_txid(&t));
        leases.note_seen_all(id, artifacts);
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

    #[cfg(test)]
    pub(crate) fn pin_progress(&self, progress: &SyncProgress) {
        let mut tracker = self.tracker();
        self.pin_progress
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tracker.on_progress(progress);
    }

    /// How many fork truncations the hub applied to any wallet this
    /// session ([`Self::truncations`]).
    pub(crate) fn truncations(&self) -> u64 {
        self.truncations.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub(crate) fn wallet_state(&self, id: &WalletId) -> Option<WalletState> {
        self.wallets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn set_wallet_state(&self, id: WalletId, state: WalletState) {
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, state);
    }

    pub(crate) fn update_wallet_state<R>(
        &self,
        id: WalletId,
        f: impl FnOnce(&mut WalletState) -> R,
    ) -> R {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        f(map.entry(id).or_default())
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

    /// Marks a Remove Unconfirmed batch, or a single Abandon, of `wallet` in
    /// progress until the returned guard drops: its abandons store rewind
    /// records it makes only after their sweeps, which a sync commit must
    /// leave stored meanwhile.
    pub(crate) fn abandon_batch(&self, wallet: WalletId) -> AbandonBatch<'_> {
        *(self
            .abandon_batches
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .entry(wallet)
        .or_default() += 1;
        AbandonBatch { hub: self, wallet }
    }

    /// Whether a Remove Unconfirmed batch, or a single Abandon, of `wallet`
    /// is in progress.
    pub(crate) fn abandon_batch_pending(&self, wallet: &WalletId) -> bool {
        (self
            .abandon_batches
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .contains_key(wallet)
    }

    /// Forgets the in-memory state of an unloaded wallet; its name stays
    /// (the "Open Wallet" menu lists it).
    pub(crate) fn unload_wallet(&self, id: &WalletId) {
        let mut unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        unloaded.insert(*id);
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.history.remove_wallet(id);
        if let Some(rescan) = self.rescan().as_mut() {
            rescan.wallets.remove(id);
        }
        self.forget_drops(id);
        self.forget_shielded_passes(id);
        drop(unloaded);
        self.drain_drops();
    }

    /// Forgets the drops wallet `id` waits with or keeps to recheck.
    fn forget_drops(&self, id: &WalletId) {
        (self
            .deferred_drops
            .lock()
            .unwrap_or_else(|p| p.into_inner()))
        .retain(|(w, ..)| w != id);
        (self.carried_drops.lock().unwrap_or_else(|p| p.into_inner())).retain(|(w, _), _| w != id);
        (self.dead_children.lock().unwrap_or_else(|p| p.into_inner())).retain(|(w, ..)| w != id);
    }

    /// Tests' passes fail for want of Platform: those that pin the rules
    /// of a succeeding pass forget the failures.
    #[cfg(test)]
    pub(crate) fn forget_shielded_failures(&self) {
        (self
            .shielded_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner()))
        .clear();
    }

    /// A wallet loaded again starts its shielded passes afresh: its first
    /// pass establishes its account again, and an earlier failure is not
    /// this load's.
    fn forget_shielded_passes(&self, id: &WalletId) {
        lock(&self.shielded_ready).remove(id);
        lock(&self.platform_addresses).remove(id);
        lock(&self.shielded_synced).remove(id);
        lock(&self.shielded_failed).remove(id);
    }

    /// Publishes rescan state: `f` changes the running rescan under the
    /// lock a fork's truncation is applied under, and each wallet's part
    /// is then no higher than its replay target. A truncation either
    /// lowers the target first or finds the published part.
    pub(crate) fn publish_rescan<R>(&self, f: impl FnOnce(&mut Option<RescanState>) -> R) -> R {
        let _truncating = self.truncating.lock().unwrap_or_else(|p| p.into_inner());
        let mut rescan = self.rescan();
        let out = f(&mut rescan);
        if let Some(state) = rescan.as_mut() {
            for id in state.wallets.keys().copied().collect::<Vec<_>>() {
                if let Some(s) = self.wallet_state(&id) {
                    state.truncate(&id, s.scan_target);
                }
            }
        }
        out
    }

    /// An unloaded or removed wallet joins the manager again: its events
    /// are heard, and the transactions key-wallet holds are this load's.
    /// Returns whether it was unloaded or removed.
    pub(crate) fn admit(&self, id: WalletId) -> bool {
        let mut unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        let was = unloaded.remove(&id);
        self.history.with_wallet(id, |h| {
            h.session_held.clear();
            h.held_here.clear();
        });
        was
    }

    /// Admits those of `ids`, wallets the manager holds, still unloaded or
    /// removed ([`Self::admit`]): a load from the store registers every
    /// wallet stored and not closed, a removed one whose deletion failed
    /// too.
    pub(crate) fn admit_loaded(&self, ids: impl IntoIterator<Item = WalletId>) {
        let mut unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        for id in ids {
            if unloaded.remove(&id) {
                self.restore.forget_wallet_checks(id, true);
                self.history.with_wallet(id, |h| {
                    h.session_held.clear();
                    h.held_here.clear();
                });
            }
        }
    }

    /// Forgets everything about a removed wallet.
    pub(crate) fn forget_wallet(&self, id: &WalletId) {
        let mut unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        unloaded.insert(*id);
        self.restore.forget_wallet_checks(*id, false);
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.names
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.history.remove_wallet(id);
        self.forget_drops(id);
        // Its notes go with its rows: a wallet restored under the same id
        // holds nothing of them.
        (self.dropped_holds.lock().unwrap_or_else(|p| p.into_inner())).remove(id);
        (self.held_forks.lock().unwrap_or_else(|p| p.into_inner())).remove(id);
        self.forget_shielded_passes(id);
        drop(unloaded);
        self.drain_drops();
    }

    pub(crate) fn begin_rebroadcast_holds(&self) {
        let mut holds = self
            .rebroadcast_holds
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        holds.get_or_insert_with(HashMap::new);
    }

    pub(crate) fn end_rebroadcast_holds(&self) {
        let holds = self
            .rebroadcast_holds
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        if let Some(holds) = holds {
            for id in holds.into_keys() {
                if let Some(reread) = self.balance_reread.get() {
                    reread(id);
                }
                self.pump.mark_history(id, None);
            }
        }
    }

    pub(crate) fn hold_rebroadcast_coins(&self, id: WalletId, coins: &[dashcore::OutPoint]) {
        if coins.is_empty() {
            return;
        }
        let mut holds = self
            .rebroadcast_holds
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(wallets) = holds.as_mut() {
            wallets.entry(id).or_default().extend(coins);
        }
    }

    pub(crate) fn rebroadcast_held(&self, id: &WalletId) -> BTreeSet<dashcore::OutPoint> {
        let holds = self
            .rebroadcast_holds
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        holds
            .as_ref()
            .and_then(|wallets| wallets.get(id))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn set_spv_running(&self, running: bool) {
        if self.tracker().set_running(running) {
            self.pump.mark_sync();
        }
    }

    /// Applies a wallet event's balance snapshot. Returns whether the
    /// visible balances changed.
    ///
    /// A snapshot with locked coins may have been taken while a funding
    /// held its temporary locks, and applied after they were released: the
    /// balances are read again once it is.
    ///
    /// The snapshot comes without the excluded coins' share
    /// (`crate::withhold`): while the wallet may hold some, it is not
    /// applied, and the balances are read again from the manager with it,
    /// so no read pairs one with the other's share.
    fn update_balance(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
    ) -> bool {
        let reread = self.balance_reread.get();
        if let Some(reread) = reread
            && self.may_withhold(&id)
        {
            reread(id);
            return false;
        }
        let changed = self.write_balances(id, balance, accounts, None, None);
        self.reread_if_locked(id, balance, accounts);
        changed
    }

    /// Balances with locked coins were read while a funding held its
    /// temporary locks, or while the restore holds are in effect (their
    /// exact figure is read from the manager's coins): they are read
    /// again.
    fn reread_if_locked(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
    ) {
        let held = (self.history).read_wallet(&id, |h| !h.restore_holds_in_effect().is_empty());
        if (held || balance.locked() > 0 || accounts.values().any(|b| b.locked() > 0))
            && let Some(reread) = self.balance_reread.get()
        {
            reread(id);
        }
    }

    /// Applies the balances of `core`, read from the manager under its
    /// lock, with the excluded coins' share and what of it the restore
    /// holds keep back, unless the wallet is unloaded; announces them when
    /// they changed.
    pub(crate) fn apply_balance_read(&self, id: WalletId, core: &ManagedWalletInfo) {
        let unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        if unloaded.contains(&id) {
            return;
        }
        let excluded = self.excluded_share(id, core);
        let held = self.restore_held_in(id, core, &excluded);
        let (balance, accounts) = (core.balance(), core.account_balances());
        if self.write_balances(id, &balance, &accounts, Some(excluded), Some(held)) {
            self.pump.mark_balances(id);
        }
    }

    /// Lets go of the share of each coin of `waiting`, read as waiting for
    /// its rescan with balances just written, whose hold went meanwhile: a
    /// release between that read and its write must not be undone. The
    /// confirmed share of a released coin the restore holds keep back joins
    /// their figure, which skipped it while it was excluded
    /// ([`Self::restore_held_in`]).
    pub(crate) fn release_settled(&self, id: WalletId, waiting: BTreeSet<dashcore::OutPoint>) {
        if waiting.is_empty() {
            return;
        }
        let held = self.rescan_held(&id);
        let gone: BTreeSet<dashcore::OutPoint> = waiting.difference(&held).copied().collect();
        if gone.is_empty() {
            return;
        }
        let (txids, inputs) = self.history.read_wallet(&id, |h| h.restore_held_coins());
        self.update_wallet_state(id, |s| {
            let restore_held: u64 = (gone.iter())
                .filter(|o| txids.contains(&o.txid) || inputs.contains(*o))
                .filter_map(|o| s.excluded.rescan_share(o))
                .map(|share| share.confirmed())
                .sum();
            s.excluded.release_rescan(&gone);
            s.restore_held = s.restore_held.saturating_add(restore_held);
        });
        self.pump.mark_balances(id);
    }

    /// `excluded`: the excluded coins' share read with the balances;
    /// `None` keeps the last one (an event's snapshot). `restore_held`:
    /// what the restore holds keep out of the confirmed bucket, read from
    /// the manager's coins; `None` (an event's snapshot) keeps the last
    /// reading, raised to the history's bound until the coins are read
    /// again.
    fn write_balances(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
        excluded: Option<crate::withhold::Excluded>,
        restore_held: Option<u64>,
    ) -> bool {
        let waiting = (excluded.as_ref()).map_or_else(BTreeSet::new, |e| e.waiting());
        let bound = match restore_held {
            Some(_) => 0,
            None => self.history.read_wallet(&id, |h| h.restore_held_value()),
        };
        let changed = {
            let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
            let state = map.entry(id).or_default();
            let before = state.balances();
            state.restore_held = restore_held.unwrap_or(state.restore_held.max(bound));
            state.balance = *balance;
            for (t, b) in accounts {
                state.accounts.insert(*t, *b);
            }
            if let Some(excluded) = excluded {
                state.excluded = excluded;
            }
            before != state.balances()
        };
        self.release_settled(id, waiting);
        changed
    }

    /// After the restore holds changed: the history's bound on what they
    /// keep out of the spendable balance at once, and the manager's coins
    /// read again for the exact figure. Announces the balance when it
    /// changed.
    pub(crate) fn refresh_restore_held(&self, id: WalletId) {
        let (held, in_effect) = (self.history).read_wallet(&id, |h| {
            (
                h.restore_held_value(),
                !h.restore_holds_in_effect().is_empty(),
            )
        });
        let changed = {
            let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
            let Some(state) = map.get_mut(&id) else {
                return;
            };
            if !in_effect && state.restore_held == 0 {
                return;
            }
            let before = state.balances();
            state.restore_held = if in_effect {
                state.restore_held.max(held)
            } else {
                held
            };
            before != state.balances()
        };
        if changed {
            self.pump.mark_balances(id);
        }
        if let Some(reread) = self.balance_reread.get() {
            reread(id);
        }
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

    /// Whether wallet `id`'s checkpoint is below its scan target: a rescan
    /// or replay is bringing its coins back.
    fn scanning(&self, id: &WalletId) -> bool {
        (self.wallets.read().unwrap_or_else(|p| p.into_inner()))
            .get(id)
            .is_some_and(|s| s.synced_height < s.scan_target)
    }

    /// Re-reads wallet `id`'s dropped rows, whose state follows the scan
    /// ([`crate::history::scan_facts`]): when it ends, is cancelled, or an
    /// abandon batch is done.
    pub(crate) fn mark_drops(&self, id: WalletId) {
        let txids: Vec<Txid> = self
            .history
            .read_wallet(&id, |h| h.drops.keys().copied().collect());
        if !txids.is_empty() {
            self.pump.mark_history(id, Some(&txids));
        }
        let _ = self.settle_rescan_holds(id, None);
    }

    /// Coins of wallet `id` that still wait for the rescan or the old
    /// SPV client to stop: neither counted nor offered.
    pub(crate) fn rescan_held(&self, id: &WalletId) -> BTreeSet<dashcore::OutPoint> {
        let mut coins = self.rebroadcast_held(id);
        self.history
            .read_wallet(id, |h| coins.extend(h.rescan_holds.keys()));
        coins
    }

    /// Lets go of wallet `id`'s holds whose rescan is back, and reads its
    /// balances again when one went. `synced` is key-wallet's synced
    /// height, read under the manager's lock: the hub's can be raised past
    /// a rewind by a height event queued before it. Without it, a hold the
    /// hub's state shows back asks for the balance reread, which reads it.
    /// Returns the released coins, whose marks the caller drops
    /// (`NetworkSession::forget_released_holds`).
    pub(crate) fn settle_rescan_holds(
        &self,
        id: WalletId,
        synced: Option<u32>,
    ) -> BTreeSet<dashcore::OutPoint> {
        let holds = self.history.read_wallet(&id, |h| h.rescan_holds.clone());
        if holds.is_empty() {
            return BTreeSet::new();
        }
        let state = self.wallet_state(&id);
        let rescanning = (self.rescan().as_ref()).is_some_and(|r| r.replaying(&id));
        let mut back = crate::rescan_hold::back(&holds, state.as_ref(), rescanning, synced);
        let retrying = self.rebroadcast_held(&id);
        back.retain(|o| !retrying.contains(o));
        if back.is_empty() {
            return BTreeSet::new();
        }
        if synced.is_none() {
            if let Some(reread) = self.balance_reread.get() {
                reread(id);
            }
            return BTreeSet::new();
        }
        let released: BTreeSet<dashcore::OutPoint> = self.history.with_wallet(id, |h| {
            let mut gone = BTreeSet::new();
            for o in back {
                if h.rescan_holds.contains_key(&o) && h.rescan_holds.get(&o) == holds.get(&o) {
                    h.rescan_holds.remove(&o);
                    gone.insert(o);
                }
            }
            gone
        });
        if !released.is_empty() {
            tracing::info!(wallet = %id, coins = released.len(), "coins given back are released: their rescan is back");
            self.update_wallet_state(id, |s| s.excluded.release_rescan(&released));
            if let Some(reread) = self.balance_reread.get() {
                reread(id);
            }
            self.pump.mark_balances(id);
            // The abandoned payments that spent them no longer say their
            // coins are being checked.
            let spenders: Vec<Txid> = self.history.read(&id, |txs| {
                (txs.iter())
                    .filter(|(_, e)| {
                        e.is_abandoned()
                            && (e.tx.input.iter()).any(|i| released.contains(&i.previous_output))
                    })
                    .map(|(t, _)| *t)
                    .collect()
            });
            if !spenders.is_empty() {
                self.pump.mark_history(id, Some(&spenders));
            }
        }
        released
    }

    /// Advances the wallet's scan heights. Returns whether the wallet
    /// started or stopped catching up.
    fn update_heights(&self, id: WalletId, synced: Option<u32>, processed: Option<u32>) -> bool {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        let state = map.entry(id).or_default();
        let catching_up = state.catching_up();
        if let Some(h) = synced {
            state.synced_height = h;
            state.scan_target = state.scan_target.max(h);
        }
        if let Some(h) = processed {
            state.processed_height = state.processed_height.max(h);
        }
        catching_up != state.catching_up()
    }

    /// A fork truncated wallet `id` to `height`: the wallet dropped what
    /// the blocks above recorded, kept their InstantSend-locked
    /// transactions unconfirmed and lowered its heights. The hub's heights
    /// follow it, and so do the history, the sweep chunks a winner in
    /// those blocks beat, the running rescan and the replay record. Applied
    /// once: a truncation found ([`Self::detect_truncation`]) only while the
    /// hub's processed height is above `height`, and the event of one found
    /// (at its height, or above it with nothing to lower) not at all.
    /// Returns whether it was applied.
    pub(crate) fn apply_truncation(
        &self,
        _truncating: &std::sync::MutexGuard<'_, ()>,
        id: WalletId,
        height: u32,
        found: Truncated<'_>,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
    ) -> bool {
        let lowered = {
            let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
            let state = map.entry(id).or_default();
            // An event is matched to a found truncation by its height
            // alone. That holds while a truncation's event reaches the hub
            // before the new chain's progress past its height does, so a
            // truncation is never found above its own event's height.
            let heard = !found.detected
                && state.detected.is_some_and(|d| {
                    height == d || (height > d && state.processed_height <= height)
                });
            if heard || (!found.detected && state.detected.is_some_and(|d| height <= d)) {
                state.detected = None;
            }
            let news = !heard && (!found.detected || state.processed_height > height);
            news.then(|| {
                let catching_up = state.catching_up();
                let from = state.scan_target.max(state.processed_height);
                let checkpoint_lowered = height < state.synced_height;
                state.synced_height = state.synced_height.min(height);
                state.scan_target = state.scan_target.min(height);
                state.processed_height = state.processed_height.min(height);
                if found.detected {
                    state.detected = Some(height);
                }
                (from, catching_up != state.catching_up(), checkpoint_lowered)
            })
        };
        let Some((from, flipped, checkpoint_lowered)) = lowered else {
            return false;
        };
        (self.truncations).fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        #[cfg(test)]
        if let Some((reached, release)) = self.pause_truncation.lock().unwrap().take() {
            reached.wait();
            release.wait();
        }
        self.note_sweep_news(id, crate::coinjoin::SweepNews::Truncated { height });
        self.note_held_fork(id, height);
        let changed = self
            .history
            .truncate(id, height, found.removed, found.unconfirmed);
        self.rebuild_restore_holds_after_truncation(id);
        // A reader of the wallet's checkpoint hears it go down, as it hears
        // every advance.
        let rescan_moved = self
            .rescan()
            .as_mut()
            .is_some_and(|rescan| rescan.truncate(&id, height));
        if rescan_moved || checkpoint_lowered {
            self.pump.mark_sync();
        }
        if let Some(replay) = self.replay.get().and_then(std::sync::Weak::upgrade) {
            replay.note(crate::replay_record::ReplayNote::Truncated {
                wallet: id,
                height,
                from,
            });
        }
        if !changed.is_empty() {
            self.pump.mark_history(id, Some(&changed));
        }
        self.mark_young(id);
        if self.update_balance(id, balance, accounts) || flipped {
            self.pump.mark_balances(id);
        }
        true
    }

    /// Applies a truncation of wallet `id` that no event announced
    /// (dash-spv truncates with its event monitor stopped): key-wallet's
    /// processed height, read with `core` under the manager's lock, is
    /// below the hub's. Only a truncation lowers it. Returns the height
    /// when applied.
    ///
    /// The history drops the transactions mined above that height that
    /// key-wallet no longer holds, and keeps those it holds out of a block
    /// (InstantSend-locked) unconfirmed. The new chain may have advanced
    /// past the fork before the check, so the height can be above it: the
    /// same applies below it to the transactions key-wallet held this
    /// session and neither holds in a block nor finalized, unless the
    /// history has them in a ChainLocked block (no fork replaces one). A
    /// record only read from storage stays: key-wallet never held it, and
    /// nothing tells it apart from one mined below the fork. A ProRegTx the
    /// history keeps out of sight (a contact's record) has no height: it
    /// goes when key-wallet held it this session and now neither holds nor
    /// finalized it. Key-wallet's ChainLock bounds nothing here: it can be
    /// the new chain's.
    pub(crate) fn detect_truncation(
        &self,
        id: WalletId,
        core: &key_wallet::wallet::managed_wallet_info::ManagedWalletInfo,
    ) -> Option<u32> {
        use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
        let truncating = self.truncating.lock().unwrap_or_else(|p| p.into_inner());
        let height = core.last_processed_height();
        if !self
            .wallet_state(&id)
            .is_some_and(|s| height < s.processed_height)
        {
            return None;
        }
        let accounts = core.accounts.all_accounts();
        let records: HashMap<Txid, bool> = (accounts.iter())
            .flat_map(|a| a.transactions().iter())
            .filter(|(_, r)| crate::history::is_wallets(r))
            .map(|(t, r)| (*t, r.context.block_info().is_some()))
            .collect();
        let finalized = |t: &Txid| accounts.iter().any(|a| a.transaction_is_finalized(t));
        let still_recorded = |t: &Txid| accounts.iter().any(|a| a.transactions().contains_key(t));
        let (mut removed, mut unconfirmed) = (Vec::new(), Vec::new());
        self.history.read_wallet(&id, |h| {
            for (txid, entry) in &h.txs {
                let Some(mined) = entry.context.block_info().map(|b| b.height()) else {
                    continue;
                };
                let is_final = matches!(entry.context, TransactionContext::InChainLockedBlock(_))
                    || finalized(txid);
                if mined <= height && (!h.session_held.contains(txid) || is_final) {
                    continue;
                }
                match records.get(txid) {
                    None => removed.push(*txid),
                    Some(false) => unconfirmed.push(*txid),
                    Some(true) => {}
                }
            }
            // A ProRegTx kept out of sight has no height: one key-wallet
            // held this session, and now neither holds in an account nor
            // has finalized, went with the fork.
            removed.extend(
                (h.hidden())
                    .map(|(t, _)| *t)
                    .filter(|t| h.session_held.contains(t) && !still_recorded(t) && !finalized(t)),
            );
        });
        self.apply_truncation(
            &truncating,
            id,
            height,
            Truncated {
                removed: &removed,
                unconfirmed: &unconfirmed,
                detected: true,
            },
            &core.balance(),
            &core.account_balances(),
        )
        .then(|| {
            tracing::warn!(wallet = %id, height, removed = removed.len(), unconfirmed = unconfirmed.len(), "applied a fork truncation no event announced");
            height
        })
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
        let unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
        if unloaded.contains(&id) {
            return;
        }
        #[cfg(test)]
        if let Some((reached, release)) = self.pause_event.lock().unwrap().take() {
            reached.wait();
            release.wait();
        }
        let now = Some(unix_now());
        self.run_sweep_retry(id);
        let carried = carried_txids(event);
        if !carried.is_empty() {
            self.history
                .with_wallet(id, |h| h.session_held.extend(carried));
        }
        match event {
            WalletEvent::TransactionDetected {
                record,
                balance,
                account_balances,
                ..
            } => {
                // A contact's record shows nowhere; a ProRegTx among them is
                // kept out of sight for its collateral, as at a load.
                if self
                    .history
                    .with_wallet_records(id, |h, locks| h.upsert_with_locks(record, now, locks))
                {
                    let live = learned_live(&record.context, self.tracker().network_tip());
                    self.pump.mark_new_txs(id, &[(record.txid, live)]);
                }
                if crate::history::is_wallets(record) {
                    self.pump.mark_history(id, Some(&[record.txid]));
                }
                self.check_funded(id, BTreeSet::from([record.txid]));
                // An event queued before an Abandon this session took its
                // transaction carries the balance from before the sweep:
                // read the manager's instead. One queued while the entry
                // is still abandoned, whatever its own context, is that
                // too; after a restart an event is the transaction seen
                // again, with its own balance.
                let stale = (self.history).read_wallet(&id, |h| {
                    h.abandoned_here.contains(&record.txid)
                        && (matches!(record.context, TransactionContext::Mempool)
                            || h.txs.get(&record.txid).is_some_and(|e| e.is_abandoned()))
                });
                if stale {
                    if let Some(reread) = self.balance_reread.get() {
                        reread(id);
                    }
                } else if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
                // First seen through its InstantSend lock: evidence. In
                // the mempool: none (BL-76).
                if let TransactionContext::InstantSend(lock) = &record.context {
                    self.note_seen(id, [record.txid]);
                    self.lock_heard();
                    if lock.txid == record.txid {
                        self.fan_out_lock(lock, &unloaded, false);
                    } else {
                        tracing::debug!(wallet = %id, txid = %record.txid, lock = %lock.txid,
                            "a detected record's lock names another transaction: not fanned out");
                    }
                }
            }
            WalletEvent::TransactionInstantLocked {
                txid,
                instant_lock,
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
                self.note_seen(id, [*txid]);
                self.lock_heard();
                if instant_lock.txid == *txid {
                    self.fan_out_lock(instant_lock, &unloaded, false);
                }
            }
            WalletEvent::TransactionsSwept {
                txids,
                superseded_by,
                winner_block,
                released_outpoints,
                balance,
                account_balances,
                ..
            } => {
                self.hold_rebroadcast_coins(id, released_outpoints);
                // Read before the history forgets them: what is built on
                // them is checked against their bytes (#739).
                let (swept, own): (Vec<Transaction>, Vec<_>) =
                    (self.history).read_wallet(&id, |h| {
                        (txids.iter())
                            .filter_map(|t| {
                                h.txs.get(t).map(|e| {
                                    let locked = crate::history::instant_locked(e);
                                    (e.tx.clone(), (e.own_inputs.clone(), locked))
                                })
                            })
                            .unzip()
                    });
                // What can never confirm is recorded as dropped with the
                // marks' removal, so a load drops what is built on it even
                // when the drop below never ran (#739).
                let dead =
                    crate::tx_actions::swept_claims(id, *superseded_by, &swept).unwrap_or_default();
                // The winner may pay no address of the wallet, and then no
                // other event names it: a sweep chunk it beat learns of it
                // here only. A winner that funds a swept claim beat nothing:
                // key-wallet dropped the claim as unsigned and gave back only
                // what it released (rust-dashcore#1523). A claim's coin it
                // released names the drop when the history lacks the claim.
                let mut claims = crate::tx_actions::claim_tree(*superseded_by, &swept);
                if !claims.is_empty()
                    || (released_outpoints.iter()).any(|o| o.txid == *superseded_by)
                {
                    let read: HashSet<Txid> = swept.iter().map(|tx| tx.txid()).collect();
                    claims.extend(txids.iter().filter(|t| !read.contains(*t)));
                    claims = crate::tx_actions::built_on(claims, &swept);
                }
                let beaten: Vec<Txid> = (txids.iter())
                    .filter(|t| !claims.contains(*t))
                    .copied()
                    .collect();
                if !beaten.is_empty() {
                    self.note_sweep_news(
                        id,
                        crate::coinjoin::SweepNews::Beaten {
                            txids: &beaten,
                            winner: *superseded_by,
                            at: match winner_block {
                                Some(b) => crate::coinjoin::BeatenAt::Mined {
                                    height: b.height(),
                                    hash: b.block_hash(),
                                },
                                None => crate::coinjoin::BeatenAt::Is,
                            },
                        },
                    );
                }
                if !dead.is_empty() {
                    self.note_sweep_news(
                        id,
                        crate::coinjoin::SweepNews::Dropped {
                            dead: &dead.keys().copied().collect(),
                            released: released_outpoints,
                        },
                    );
                }
                // What is built on a never-valid claim stays listed, as
                // dropped: its abandoned mark lists it again at a load, as
                // the other drops of what is built on a claim do (#766).
                let kept: Vec<(Txid, Transaction, crate::tx_actions::OwnInputs)> = (swept.iter())
                    .zip(own)
                    .filter(|(tx, (_, locked))| {
                        !locked
                            && dead.get(&tx.txid())
                                == Some(&crate::history::DropCause::InvalidSource)
                    })
                    .map(|(tx, (own, _))| (tx.txid(), tx.clone(), own))
                    .collect();
                // Upstream deleted these records (provably beaten spends);
                // the history drops them too, and their sent marks go. Their
                // children it holds no record of can never confirm either:
                // the next reconcile drops them, with this winner.
                let (children, claim) = self.history.with_wallet(id, |h| {
                    // A sweep whose winner funds a swept transaction is the
                    // claim rule's (#739): what is built on the claim is
                    // its to judge. So is one whose swept transaction the
                    // history does not hold.
                    let claim = (txids.iter()).any(|t| {
                        h.txs.get(t).is_none_or(|e| {
                            (e.tx.input.iter()).any(|i| i.previous_output.txid == *superseded_by)
                        })
                    });
                    if claim
                        && (txids.iter()).all(|t| {
                            h.txs.get(t).is_none_or(|e| {
                                !(e.tx.input.iter())
                                    .any(|i| i.previous_output.txid == *superseded_by)
                            })
                        })
                    {
                        let unheld: Vec<&Txid> =
                            (txids.iter()).filter(|t| !h.txs.contains_key(*t)).collect();
                        tracing::debug!(wallet = %id, ?unheld, winner = %superseded_by,
                            "a sweep is taken as a claim only because the history lacks a swept transaction");
                    }
                    let children = match claim {
                        true => BTreeSet::new(),
                        false => crate::history::unmined_children(&h.txs, txids),
                    };
                    for t in txids {
                        match (kept.iter()).any(|(k, ..)| k == t) {
                            true => {
                                if let Some(e) = h.txs.get_mut(t) {
                                    e.abandoned = true;
                                }
                                let info = crate::history::DropInfo::new(
                                    crate::history::DropCause::InvalidSource,
                                );
                                h.drops.insert(*t, info);
                            }
                            false => h.forget(t),
                        }
                        h.swept.insert(*t);
                    }
                    (children, claim)
                });
                let queued = !children.is_empty();
                if queued {
                    let winner = crate::history::Winner {
                        txid: Some(*superseded_by),
                        mined: winner_block.as_ref().map(|b| b.height()),
                    };
                    let took = (swept.iter())
                        .flat_map(|tx| tx.input.iter().map(|i| i.previous_output))
                        .collect();
                    let truncations = self.truncations();
                    (self.dead_children.lock().unwrap_or_else(|p| p.into_inner())).push((
                        id,
                        truncations,
                        (
                            children.into_iter().collect(),
                            winner,
                            txids.iter().copied().collect(),
                            took,
                        ),
                    ));
                }
                if let Some(appdb) = self.appdb.get().and_then(std::sync::Weak::upgrade) {
                    let claims: &[Txid] = if claim { txids } else { &[] };
                    let sweep = crate::tx_actions::Sweep {
                        winner: *superseded_by,
                        block: winner_block.as_ref().map(|b| (b.height(), b.block_hash())),
                        claim,
                        txs: &swept,
                    };
                    let forgotten = crate::tx_actions::forget_swept(
                        &appdb,
                        id,
                        txids,
                        &dead,
                        claims,
                        &kept,
                        Some(&sweep),
                    );
                    match forgotten {
                        Ok(()) => {
                            self.history.drop_claims(&id, txids.iter().copied());
                            self.forget_restore_holds(id, txids);
                        }
                        Err(e) => {
                            tracing::warn!(wallet = %id, error = %e, "could not drop the marks of swept payments")
                        }
                    }
                }
                self.pump.mark_history(id, Some(txids));
                if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
                if queued {
                    self.schedule_reconcile(id);
                }
                // A swept transaction can never confirm, nor can what the
                // history alone holds built on its outputs (a payment
                // key-wallet has no record of after a restart, #739).
                #[cfg(test)]
                let skip = self
                    .skip_dead_hook
                    .load(std::sync::atomic::Ordering::SeqCst);
                #[cfg(not(test))]
                let skip = false;
                if !dead.is_empty()
                    && !skip
                    && let Some(start) = self.swept_dead.get()
                {
                    start(id, dead);
                }
            }
            WalletEvent::ChainTruncated {
                height,
                txids,
                unconfirmed_txids,
                balance,
                account_balances,
                ..
            } => {
                let truncating = self.truncating.lock().unwrap_or_else(|p| p.into_inner());
                self.apply_truncation(
                    &truncating,
                    id,
                    *height,
                    Truncated {
                        removed: txids,
                        unconfirmed: unconfirmed_txids,
                        detected: false,
                    },
                    balance,
                    account_balances,
                );
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
                let tip = self.tracker().network_tip();
                let changed: Vec<Txid> = self.history.with_wallet_records(id, |h, locks| {
                    inserted
                        .iter()
                        .chain(updated)
                        .chain(matured)
                        .filter_map(|r| {
                            if h.upsert_with_locks(r, now, locks) {
                                new.push((r.txid, learned_live(&r.context, tip)));
                            }
                            crate::history::is_wallets(r).then_some(r.txid)
                        })
                        .collect()
                });
                if !new.is_empty() {
                    self.pump.mark_new_txs(id, &new);
                }
                self.settle_mined_restore_holds(id);
                let flipped = self.update_heights(id, None, Some(*height));
                if !changed.is_empty() {
                    self.pump.mark_history(id, Some(&changed));
                }
                self.mark_young(id);
                if self.update_balance(id, balance, account_balances) || flipped {
                    self.pump.mark_balances(id);
                }
                // In this block: inserted, or confirmed now. A matured
                // record's block was evidence already.
                self.note_seen(id, inserted.iter().chain(updated).map(|r| r.txid));
                self.schedule_reconcile(id);
                if !inserted.is_empty() {
                    self.check_funded(id, inserted.iter().map(|r| r.txid).collect());
                }
            }
            WalletEvent::SyncHeightAdvanced { height, .. } => {
                let scanning = self.scanning(&id);
                if self.update_heights(id, Some(*height), Some(*height)) {
                    self.pump.mark_balances(id);
                }
                if scanning && !self.scanning(&id) {
                    self.mark_drops(id);
                }
                if let Some(rescan) = self.rescan().as_mut() {
                    rescan.note_height(&id, *height);
                }
                // A rescan's last height may change no balance: its holds
                // are settled once the rescan has noted it (`crate::rescan_hold`).
                let _ = self.settle_rescan_holds(id, None);
                // A reader of the wallet's checkpoint is told of every advance:
                // the last one can come after the last progress report.
                self.pump.mark_sync();
                self.mark_young(id);
                self.schedule_reconcile(id);
            }
            WalletEvent::ChainLockProcessed {
                chain_lock,
                locked_transactions,
                ..
            } => {
                self.note_chainlock(chain_lock.block_height, chain_lock.block_hash);
                if self.carried_due(id, chain_lock.block_height) {
                    self.schedule_reconcile(id);
                }
                let (locked, settled): (Vec<Txid>, Vec<Txid>) = self.history.with_wallet(id, |h| {
                    // key-wallet promotes by height; only a record whose own
                    // block the lock names is final for its sent mark. This
                    // relies on dash-spv dispatching only validated locks
                    // (BLS-verified, header-matched): recheck that on any
                    // rust-dashcore bump.
                    let settled = (locked_transactions.values().flatten())
                        .filter(|t| {
                            (h.txs.get(*t).and_then(|e| e.context.block_info())).is_some_and(|b| {
                                b.height() == chain_lock.block_height
                                    && b.block_hash() == chain_lock.block_hash
                            })
                        })
                        .copied()
                        .collect();
                    let locked = (locked_transactions.values().flatten())
                        .filter(|t| h.set_chain_locked(t))
                        .copied()
                        .collect();
                    (locked, settled)
                });
                let mut changed = locked;
                if !settled.is_empty() {
                    changed.extend(&settled);
                    (self.final_sends.lock().unwrap_or_else(|p| p.into_inner()))
                        .push((id, settled));
                }
                if !changed.is_empty() {
                    self.pump.mark_history(id, Some(&changed));
                }
                self.note_seen(id, locked_transactions.values().flatten().copied());
                // The lock may prune key-wallet's observed spends of a
                // withheld payment's coins (`crate::withhold`).
                if self.may_withhold(&id)
                    && let Some(reread) = self.balance_reread.get()
                {
                    reread(id);
                }
            }
            WalletEvent::DismissalsChanged { .. } => {}
        }
    }
}

/// A beaten payment kept to recheck: its winner, its detection's evidence,
/// the winner's block height, and the wallet's applied ChainLock its last
/// recheck read.
pub(crate) type CarriedDrop = (
    crate::history::Winner,
    crate::tx_actions::Recheck,
    u32,
    Option<u32>,
);

/// Chooses the spend claims of `wallets` again after `txid`'s lock.
pub(crate) type LockRerun = dyn Fn(Txid, InstantLock, Vec<WalletId>) + Send + Sync;

/// What a drop with no recheck was found at (#891): the truncations
/// applied, key-wallet's processed height, and the header chain's block at
/// its anchor height ([`crate::tx_actions::take_anchor`]).
pub(crate) type DropTake = (u64, Option<u32>, Option<dashcore::BlockHash>);

/// Children of swept payments queued for a drop, with their wallet and
/// the truncations applied when they were queued.
type DeadChildren = (WalletId, u64, DeadSet);

/// Children of swept payments to drop: their txids, the winner, the swept
/// payments, and the coins those spent, read before the history forgot
/// them.
pub(crate) type DeadSet = (
    Vec<Txid>,
    crate::history::Winner,
    BTreeSet<Txid>,
    BTreeSet<dashcore::OutPoint>,
);

/// The transactions a wallet event names.
fn carried_txids(event: &WalletEvent) -> Vec<Txid> {
    match event {
        WalletEvent::TransactionDetected { record, .. } => vec![record.txid],
        WalletEvent::TransactionInstantLocked { txid, .. } => vec![*txid],
        WalletEvent::TransactionsSwept { txids, .. } => txids.clone(),
        WalletEvent::ChainTruncated {
            txids,
            unconfirmed_txids,
            ..
        } => txids.iter().chain(unconfirmed_txids).copied().collect(),
        WalletEvent::BlockProcessed {
            inserted,
            updated,
            matured,
            ..
        } => (inserted.iter().chain(updated).chain(matured))
            .map(|r| r.txid)
            .collect(),
        WalletEvent::SyncHeightAdvanced { .. } | WalletEvent::DismissalsChanged { .. } => {
            Vec::new()
        }
        WalletEvent::ChainLockProcessed {
            locked_transactions,
            ..
        } => locked_transactions.values().flatten().copied().collect(),
    }
}

impl EventHandler for SessionHub {
    /// A verified InstantSend lock, which the mempool manager hands the
    /// wallet as well: a payment to no loaded wallet's address raises no
    /// wallet event (#828).
    fn on_sync_event(&self, event: &SyncEvent) {
        if let SyncEvent::InstantLockReceived {
            instant_lock,
            validated: true,
        } = event
        {
            self.held_lock_heard(instant_lock);
            let unloaded = self.unloaded.lock().unwrap_or_else(|p| p.into_inner());
            self.fan_out_lock(instant_lock, &unloaded, true);
        }
    }

    fn on_progress(&self, progress: &SyncProgress) {
        let (changed, masternodes_synced, tip) = {
            let mut tracker = self.tracker();
            #[cfg(test)]
            if self.pin_progress.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            (
                tracker.on_progress(progress),
                tracker.masternodes_synced(),
                tracker.tip_height(),
            )
        };
        if masternodes_synced && let Some(trust) = self.trust.get() {
            trust.note_masternodes_synced();
        }
        if changed {
            self.pump.mark_sync();
            self.restore.wake.notify_one();
        }
        if tip.is_some_and(|tip| self.header_tip_dropped(tip)) {
            let wallets: Vec<WalletId> = (self.wallets.read().unwrap_or_else(|p| p.into_inner()))
                .keys()
                .copied()
                .collect();
            for id in wallets {
                self.schedule_reconcile(id);
            }
        }
    }

    fn on_network_event(&self, event: &NetworkEvent) {
        let (changed, up) = {
            let mut tracker = self.tracker();
            let before = tracker.peer_count();
            let changed = tracker.on_network_event(event);
            (changed, before == 0 && tracker.peer_count() > 0)
        };
        if changed {
            self.pump.mark_sync();
        }
        if up && let Some(leases) = self.lease_table() {
            leases.request_resume(None);
        }
        if up && let Some(run) = self.peers_up.get() {
            run();
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

impl PlatformEventHandler for SessionHub {
    /// A Platform-address pass (release 0.1, m4 §2.12): every wallet it
    /// synced re-reads its balance and activity, and a wallet's first
    /// outcome, or a changed one, re-reads its sync status. A skipped pass
    /// (another in flight) names no wallet.
    fn on_platform_address_sync_completed(
        &self,
        summary: &platform_wallet::manager::platform_address_sync::PlatformAddressSyncSummary,
    ) {
        if !summary.wallet_results.is_empty() && summary.wallet_results.values().all(|o| o.is_ok())
        {
            self.restore.note_platform_pass();
        }
        (self.restore).note_wallet_platform_pass(
            (summary.wallet_results.iter())
                .filter(|(_, o)| o.is_ok())
                .map(|(id, _)| WalletId(*id)),
        );
        use crate::platform::AddressSync;
        let mut last = lock(&self.platform_addresses);
        for (id, outcome) in &summary.wallet_results {
            let wallet = WalletId(*id);
            let ok = outcome.is_ok();
            let now = if ok {
                AddressSync::Done
            } else {
                AddressSync::Failed
            };
            let changed = last.insert(wallet, now) != Some(now);
            if ok {
                self.pump
                    .mark_platform(wallet, [PlatformChange::PlatformAddresses]);
            }
            if changed {
                self.pump.mark_platform(wallet, [PlatformChange::Startup]);
            }
        }
    }

    /// A shielded sync pass that found notes or spends of a wallet's account
    /// (not a cooldown skip) tells its hosts (m4 §2.13), and so does the
    /// wallet's first pass of the session, which establishes its account
    /// (`ShieldedStatus::ready`) whatever it found. A cooldown skip ran no
    /// pass: it is neither a wallet's first nor a recovery.
    fn on_shielded_sync_completed(&self, summary: &ShieldedSyncPassSummary) {
        let results = || summary.wallet_results.values();
        if !results().any(|o| matches!(o, WalletShieldedOutcome::Err(_)))
            && results().any(|o| matches!(o, WalletShieldedOutcome::Ok(p) if !p.is_cooldown_skip))
        {
            self.restore.note_shielded_pass();
        }
        (self.restore).note_wallet_shielded_pass(
            (summary.wallet_results.iter())
                .filter(|(_, o)| match o {
                    WalletShieldedOutcome::Ok(pass) => !pass.is_cooldown_skip,
                    WalletShieldedOutcome::Skipped => true,
                    WalletShieldedOutcome::Err(_) => false,
                })
                .map(|(id, _)| WalletId(*id)),
        );
        *self.shielded_scan.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut synced = self
            .shielded_synced
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut failed = self
            .shielded_failed
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (id, outcome) in &summary.wallet_results {
            let wallet = WalletId(*id);
            let pass = match outcome {
                WalletShieldedOutcome::Ok(pass) if pass.is_cooldown_skip => continue,
                WalletShieldedOutcome::Ok(pass) => pass,
                WalletShieldedOutcome::Err(_) => {
                    if failed.insert(wallet) {
                        self.pump.mark_platform(wallet, [PlatformChange::Shielded]);
                    }
                    continue;
                }
                WalletShieldedOutcome::Skipped => continue,
            };
            let recovered = failed.remove(&wallet);
            let first = synced.insert(wallet);
            let found = (pass.notes_result.new_notes_per_account.values())
                .chain(pass.newly_spent_per_account.values())
                .any(|n| *n > 0);
            if first || found || recovered {
                self.pump.mark_platform(wallet, [PlatformChange::Shielded]);
            }
        }
    }

    /// The running pass's tree progress, for `ShieldedStatus::scan`. The
    /// pass's first batch tells the established wallets' hosts a pass runs
    /// (the others read their status until established).
    fn on_shielded_tree_progress(&self, leaves_committed: u64, total_target: u64) {
        let started = (lock(&self.shielded_scan))
            .replace((leaves_committed, total_target))
            .is_none();
        if started {
            let established: HashSet<WalletId> = (lock(&self.shielded_synced).iter())
                .chain(lock(&self.shielded_ready).iter())
                .copied()
                .collect();
            for wallet in established {
                self.pump.mark_platform(wallet, [PlatformChange::Shielded]);
            }
        }
    }
}

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
                balances: Some(state.balances()),
            });
        }
    }

    fn flush_history(&self, changes: BTreeMap<WalletId, TxidSet>) {
        let mut seen: Vec<(WalletId, Vec<String>)> = Vec::new();
        let mut stamps: Vec<(WalletId, Vec<dw_appdb::TxFiat>)> = Vec::new();
        let now = unix_now();
        let stamps_allowed = self.hub.stamp_check();
        let rate = self.hub.stamp_rate();
        for (wallet_id, set) in changes {
            let txids = match set {
                TxidSet::All => Vec::new(),
                TxidSet::Some(set) => set.into_iter().collect(),
            };
            if !txids.is_empty() {
                seen.push((wallet_id, txids.iter().map(Txid::to_string).collect()));
            }
            if let Some(rate) = &rate {
                let rows = self.hub.price_stamps(rate, &wallet_id, &txids, now);
                if !rows.is_empty() {
                    stamps.push((wallet_id, rows));
                }
            }
            self.hub.emit(EngineEvent::HistoryChanged {
                network: self.hub.network.clone(),
                wallet_id,
                txids,
            });
        }
        let finals =
            std::mem::take(&mut *(self.hub.final_sends.lock()).unwrap_or_else(|p| p.into_inner()));
        if seen.is_empty() && finals.is_empty() {
            return;
        }
        // First-seen times keep a mempool payment's arrival time across
        // restarts (the persisted record has only the block time). Price
        // stamps are written once, after the first-seen time.
        let appdb = Arc::clone(&self.appdb);
        let write = move || {
            for (wallet_id, txids) in seen {
                if let Err(e) = appdb.note_txs_seen(&wallet_id.to_string(), &txids, now) {
                    tracing::warn!(%wallet_id, error = %e, "could not store first-seen times");
                }
            }
            // Turned off since the price was read: nothing is stamped.
            let stamps = if stamps_allowed() { stamps } else { Vec::new() };
            for (wallet_id, rows) in stamps {
                if let Err(e) = appdb.stamp_tx_fiat(&wallet_id.to_string(), &rows, now) {
                    tracing::warn!(%wallet_id, error = %e, "could not store price stamps");
                }
            }
            for (wallet_id, txids) in finals {
                let (sent, _) = crate::tx_actions::load_sent(&appdb, wallet_id);
                let marked: Vec<Txid> = (txids.into_iter())
                    .filter(|t| sent.contains_key(t))
                    .collect();
                if let Err(e) = crate::tx_actions::forget_sent(&appdb, wallet_id, &marked) {
                    tracing::warn!(wallet = %wallet_id, error = %e, "could not drop the marks of settled payments");
                }
            }
        };
        #[cfg(test)]
        let write = {
            let pause = self
                .hub
                .pause_seen
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            move || match pause {
                Some(pause) => pause.hold_then(write),
                None => write(),
            }
        };
        tokio::task::spawn_blocking(write);
    }

    fn flush_new_txs(&self, batches: BTreeMap<WalletId, NewTxs>) {
        let catch_up = !self.hub.tracker().caught_up();
        for (wallet_id, batch) in batches {
            self.hub.emit(EngineEvent::NewTransactions {
                network: self.hub.network.clone(),
                wallet_id,
                live: batch.live_in_order(),
                txids: batch.txids,
                catch_up,
            });
        }
    }

    fn flush_platform(
        &self,
        changes: BTreeMap<WalletId, BTreeSet<PlatformChange>>,
        news: crate::pump::NewsBatch,
    ) {
        for (wallet_id, set) in changes {
            for change in set {
                self.hub.emit(EngineEvent::Platform {
                    network: self.hub.network.clone(),
                    wallet_id,
                    change,
                });
            }
        }
        for (wallet_id, identities) in news {
            for (identity, n) in identities {
                self.hub.emit(EngineEvent::DashPayNews {
                    network: self.hub.network.clone(),
                    wallet_id,
                    identity,
                    events: n.events,
                    more: n.more,
                });
            }
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

/// Tests: a running balance reread ([`SessionHub::reread_started`]).
#[cfg(test)]
pub(crate) struct RereadRunning(Arc<std::sync::atomic::AtomicUsize>);

#[cfg(test)]
impl Drop for RereadRunning {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use dashcore::{BlockHash, OutPoint, Transaction, TxIn};
    use key_wallet::account::StandardAccountType;
    use key_wallet::managed_account::transaction_record::{
        TransactionDirection, TransactionRecord,
    };
    use key_wallet::transaction_checking::{BlockInfo, TransactionType};

    use super::*;

    struct NoEvents;

    impl EventSink for NoEvents {
        fn emit(&self, _: EngineEvent) {}
    }

    const W: WalletId = WalletId([7; 32]);

    fn tx(n: u8) -> Transaction {
        Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
                ..Default::default()
            }],
            output: Vec::new(),
            special_transaction_payload: None,
        }
    }

    fn in_block(t: &Transaction, height: u32) -> TransactionRecord {
        seen(
            t,
            TransactionContext::InBlock(BlockInfo::new(height, BlockHash::all_zeros(), 0)),
        )
    }

    fn seen(t: &Transaction, context: TransactionContext) -> TransactionRecord {
        TransactionRecord::new(
            t.clone(),
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            context,
            TransactionType::Standard,
            TransactionDirection::Incoming,
            Vec::new(),
            Vec::new(),
            1_000,
        )
    }

    fn truncated(
        height: u32,
        removed: Vec<Txid>,
        unconfirmed: Vec<Txid>,
        balance: u64,
    ) -> WalletEvent {
        WalletEvent::ChainTruncated {
            wallet_id: W.0,
            height,
            txids: removed,
            unconfirmed_txids: unconfirmed,
            restored_outpoints: Vec::new(),
            balance: WalletCoreBalance::new(balance, 0, 0, 0),
            account_balances: BTreeMap::new(),
        }
    }

    fn shielded_pass(
        outcomes: impl IntoIterator<Item = (WalletId, WalletShieldedOutcome)>,
    ) -> ShieldedSyncPassSummary {
        ShieldedSyncPassSummary {
            wallet_results: (outcomes.into_iter()).map(|(w, o)| (w.0, o)).collect(),
            sync_unix_seconds: 1_700_000_000,
            evidence: None,
        }
    }

    fn found_nothing() -> WalletShieldedOutcome {
        WalletShieldedOutcome::Ok(Default::default())
    }

    fn shielded_marks(hub: &SessionHub) -> Vec<WalletId> {
        let marks = hub.pump.platform_marks.lock().unwrap().replace(Vec::new());
        (marks.unwrap_or_default().into_iter())
            .filter(|(_, c)| c.contains(&PlatformChange::Shielded))
            .map(|(w, _)| w)
            .collect()
    }

    fn startup_marks(hub: &SessionHub) -> Vec<WalletId> {
        let marks = hub.pump.platform_marks.lock().unwrap().replace(Vec::new());
        (marks.unwrap_or_default().into_iter())
            .filter(|(_, c)| c.contains(&PlatformChange::Startup))
            .map(|(w, _)| w)
            .collect()
    }

    /// A coin both waiting for its rescan and kept back by a restore hold
    /// stays out of confirmed when its rescan hold settles: the restore
    /// figure takes its confirmed share as the excluded share lets it go.
    #[test]
    fn a_settled_rescan_share_stays_out_of_confirmed_while_restore_held() {
        const COIN: u64 = 100_000_000;
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let p = Txid::from_byte_array([3; 32]);
        let (held, free, fork_held) = (
            OutPoint::new(p, 1),
            OutPoint::new(Txid::from_byte_array([4; 32]), 0),
            OutPoint::new(p, 2),
        );
        hub.history.with_wallet(W, |h| {
            h.load_incomplete = true;
            h.restore_holds.insert(p, BTreeSet::new());
        });
        let account = AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        };
        let share = WalletCoreBalance::new(COIN, 0, 0, 0);
        hub.update_wallet_state(W, |s| {
            s.balance = WalletCoreBalance::new(4 * COIN, 0, 0, 0);
            s.excluded = crate::withhold::Excluded {
                balance: WalletCoreBalance::new(3 * COIN, 0, 0, 0),
                accounts: BTreeMap::from([(account, WalletCoreBalance::new(3 * COIN, 0, 0, 0))]),
                pending: 0,
                coins: BTreeMap::from([
                    (held, (account, share, crate::ExclusionReason::RescanHold)),
                    (free, (account, share, crate::ExclusionReason::RescanHold)),
                    (
                        fork_held,
                        (account, share, crate::ExclusionReason::ForkKeptLock),
                    ),
                ]),
            };
        });
        assert_eq!(hub.wallet_state(&W).unwrap().balances().confirmed, COIN);
        assert_eq!(
            hub.wallet_state(&W).unwrap().excluded.waiting(),
            BTreeSet::from([held, free])
        );

        hub.release_settled(W, BTreeSet::from([held, free, fork_held]));
        let b = hub.wallet_state(&W).unwrap().balances();
        assert_eq!(
            b.confirmed,
            2 * COIN,
            "the restore-held coin counts confirmed: {b:?}"
        );
        assert_eq!(b.unconfirmed, COIN, "{b:?}");
        assert_eq!(b.total, 3 * COIN, "{b:?}");
    }

    /// A wallet's Platform addresses are synced once its own pass succeeded
    /// this session, not after any pass of the network: pending until a
    /// pass ran for it, failed until one succeeds, and its hosts re-read its
    /// sync status at each change.
    #[test]
    fn a_wallet_platform_addresses_are_synced_by_its_own_pass() {
        use crate::platform::AddressSync;
        use platform_wallet::manager::platform_address_sync::{
            PlatformAddressSyncSummary, WalletSyncOutcome,
        };
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        let pass = |outcomes: Vec<(WalletId, bool)>| PlatformAddressSyncSummary {
            wallet_results: (outcomes.into_iter())
                .map(|(w, ok)| {
                    let outcome = match ok {
                        true => WalletSyncOutcome::Ok(Default::default()),
                        false => WalletSyncOutcome::Err("unreachable".into()),
                    };
                    (w.0, outcome)
                })
                .collect(),
            sync_unix_seconds: 1_700_000_000,
        };
        let other = WalletId([8; 32]);
        assert_eq!(hub.platform_addresses(W), AddressSync::Pending);
        hub.on_platform_address_sync_completed(&pass(vec![(other, true)]));
        assert_eq!(hub.platform_addresses(W), AddressSync::Pending);
        assert_eq!(startup_marks(&hub), [other]);
        hub.on_platform_address_sync_completed(&pass(vec![(W, false)]));
        assert_eq!(hub.platform_addresses(W), AddressSync::Failed);
        assert_eq!(startup_marks(&hub), [W]);
        hub.on_platform_address_sync_completed(&pass(vec![(W, false)]));
        assert_eq!(startup_marks(&hub), []);
        hub.on_platform_address_sync_completed(&pass(vec![(W, true)]));
        assert_eq!(hub.platform_addresses(W), AddressSync::Done);
        assert_eq!(startup_marks(&hub), [W]);
        // A skipped pass (another in flight) names no wallet.
        hub.on_platform_address_sync_completed(&PlatformAddressSyncSummary::default());
        assert_eq!(hub.platform_addresses(W), AddressSync::Done);
        // A wallet loaded again waits for its own pass again.
        hub.forget_shielded_passes(&W);
        assert_eq!(hub.platform_addresses(W), AddressSync::Pending);
    }

    /// A pass's first batch tells the established wallets' hosts a pass
    /// runs, once per pass; a wallet not established reads its status until
    /// it is.
    #[test]
    fn a_running_pass_tells_the_established_wallets_hosts() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        let (ready, fresh) = (WalletId([8; 32]), WalletId([9; 32]));
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        hub.note_shielded_ready(ready);
        assert!(hub.shielded_established(W) && hub.shielded_established(ready));
        assert!(!hub.shielded_established(fresh));
        shielded_marks(&hub);
        hub.on_shielded_tree_progress(10, 100);
        let mut marked = shielded_marks(&hub);
        marked.sort();
        assert_eq!(marked, [W, ready]);
        hub.on_shielded_tree_progress(50, 100);
        assert_eq!(shielded_marks(&hub), []);
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        shielded_marks(&hub);
        hub.on_shielded_tree_progress(10, 100);
        assert_eq!(shielded_marks(&hub).len(), 2);
    }

    /// The wallet's first shielded pass of the session establishes its
    /// account (`ready`) even when it finds nothing, so hosts hear of it;
    /// later passes that find nothing stay quiet.
    #[test]
    fn a_first_shielded_pass_that_finds_nothing_tells_the_hosts() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert_eq!(shielded_marks(&hub), [W]);
        for _ in 0..3 {
            hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        }
        assert_eq!(shielded_marks(&hub), []);
        // A skipped wallet (not bound) has had no pass.
        let other = WalletId([8; 32]);
        hub.on_shielded_sync_completed(&shielded_pass([(other, WalletShieldedOutcome::Skipped)]));
        assert_eq!(shielded_marks(&hub), []);
        hub.on_shielded_sync_completed(&shielded_pass([(other, found_nothing())]));
        assert_eq!(shielded_marks(&hub), [other]);
    }

    /// A failed pass is the wallet's state until a pass succeeds, before
    /// the account is established or after; hosts hear each change.
    #[test]
    fn a_failed_shielded_pass_holds_until_one_succeeds() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        let failed = || WalletShieldedOutcome::Err("unreachable".into());
        hub.on_shielded_sync_completed(&shielded_pass([(W, failed())]));
        assert!(hub.shielded_pass_failed(W));
        assert_eq!(shielded_marks(&hub), [W]);
        hub.on_shielded_sync_completed(&shielded_pass([(W, failed())]));
        assert!(hub.shielded_pass_failed(W));
        assert_eq!(shielded_marks(&hub), []);
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert!(!hub.shielded_pass_failed(W));
        assert_eq!(shielded_marks(&hub), [W]);
        // After the account is established: fails, then recovers.
        hub.on_shielded_sync_completed(&shielded_pass([(W, failed())]));
        assert!(hub.shielded_pass_failed(W));
        assert_eq!(shielded_marks(&hub), [W]);
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert!(!hub.shielded_pass_failed(W));
        assert_eq!(shielded_marks(&hub), [W]);
        // A skipped pass changes nothing.
        hub.on_shielded_sync_completed(&shielded_pass([(W, failed())]));
        hub.on_shielded_sync_completed(&shielded_pass([(W, WalletShieldedOutcome::Skipped)]));
        assert!(hub.shielded_pass_failed(W));
    }

    /// A cooldown skip ran no pass: a wallet bound just before it is still
    /// told of its real first pass, and a failed one stays failed.
    #[test]
    fn a_cooldown_skip_is_neither_a_first_pass_nor_a_recovery() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        let skip = || {
            WalletShieldedOutcome::Ok(platform_wallet::wallet::shielded::ShieldedSyncSummary {
                is_cooldown_skip: true,
                ..Default::default()
            })
        };
        hub.on_shielded_sync_completed(&shielded_pass([(W, skip())]));
        assert_eq!(shielded_marks(&hub), []);
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert_eq!(shielded_marks(&hub), [W], "the real first pass");
        hub.on_shielded_sync_completed(&shielded_pass([(
            W,
            WalletShieldedOutcome::Err("x".into()),
        )]));
        assert_eq!(shielded_marks(&hub), [W]);
        hub.on_shielded_sync_completed(&shielded_pass([(W, skip())]));
        assert!(hub.shielded_pass_failed(W), "a skip is no recovery");
        assert_eq!(shielded_marks(&hub), []);
    }

    /// A wallet closed and opened again in the session is told of its
    /// first pass again, and an earlier failure is forgotten.
    #[test]
    fn an_unloaded_wallet_starts_its_shielded_passes_afresh() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        *hub.pump.platform_marks.lock().unwrap() = Some(Vec::new());
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        hub.on_shielded_sync_completed(&shielded_pass([(
            W,
            WalletShieldedOutcome::Err("x".into()),
        )]));
        assert_eq!(shielded_marks(&hub), [W, W]);
        hub.unload_wallet(&W);
        assert!(!hub.shielded_pass_failed(W));
        hub.admit(W);
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert_eq!(shielded_marks(&hub), [W], "its first pass again");
    }

    /// The tree progress reads once a batch proved the pool's size, never
    /// past it, and clears when the pass ends.
    #[test]
    fn the_shielded_scan_progress_lasts_the_pass() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        assert_eq!(hub.shielded_scan(), None);
        hub.on_shielded_tree_progress(500, 0);
        assert_eq!(hub.shielded_scan(), None);
        hub.on_shielded_tree_progress(4_000, 10_000);
        let scan = hub.shielded_scan().unwrap();
        assert_eq!((scan.checked, scan.total), (4_000, 10_000));
        hub.on_shielded_tree_progress(10_200, 10_000);
        let scan = hub.shielded_scan().unwrap();
        assert_eq!((scan.checked, scan.total), (10_000, 10_000));
        hub.on_shielded_sync_completed(&shielded_pass([(W, found_nothing())]));
        assert_eq!(hub.shielded_scan(), None);
    }

    /// A checkpoint advance outside a rescan tells whoever reads the
    /// wallet's scan height: the last advance can come after the last
    /// progress report, and nothing else would announce it.
    #[test]
    fn a_checkpoint_advance_marks_sync_without_a_rescan() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        assert!(!hub.pump.sync_marked());
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height: 100,
        });
        assert!(hub.pump.sync_marked());
    }

    /// A wallet's block events start its duplicate-coin reconcile (`restore`)
    /// once per run: while one is started and not yet running, more events
    /// start nothing; once it runs, the next event starts another.
    #[test]
    fn block_events_start_one_reconcile_at_a_time() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let started = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&started);
        hub.attach_reconcile(Box::new(move |id| seen.lock().unwrap().push(id)));
        let block = |height| WalletEvent::BlockProcessed {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            height,
            chain_lock: None,
            inserted: Vec::new(),
            updated: Vec::new(),
            matured: Vec::new(),
            balance: WalletCoreBalance::new(1_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        };
        let advanced = |height| WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height,
        };
        let other = WalletId([8; 32]);

        hub.apply_wallet_event(&block(100));
        hub.apply_wallet_event(&advanced(100));
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: other.0,
            height: 100,
        });
        assert_eq!(*started.lock().unwrap(), [W, other]);

        hub.reconcile_running(W);
        hub.apply_wallet_event(&advanced(101));
        assert_eq!(*started.lock().unwrap(), [W, other, W]);
    }

    /// Only the wallet's own records are marked for the history view: a
    /// block event carrying an own record and a contact's ProRegTx marks
    /// the own one alone, and a transaction event carrying a contact's
    /// ProRegTx marks nothing. Both ProRegTxs are kept out of sight.
    #[test]
    fn only_the_wallets_own_records_are_marked_for_the_history_view() {
        use crate::masternodes::provider_fixtures::{coins, null, registration};
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let own = tx(1);
        let amount = dw_protx::params::collateral_amount(false);
        let [mined, seen] = [1, 2].map(|n| registration(null(0), false, n, coins(&[amount])));
        let contact = |t: &Transaction, context| {
            TransactionRecord::new(
                t.clone(),
                AccountType::DashpayExternalAccount {
                    index: 0,
                    user_identity_id: [0xaa; 32],
                    friend_identity_id: [0xbb; 32],
                },
                context,
                TransactionType::Standard,
                TransactionDirection::Internal,
                Vec::new(),
                Vec::new(),
                0,
            )
        };
        hub.apply_wallet_event(&WalletEvent::BlockProcessed {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            height: 100,
            chain_lock: None,
            inserted: vec![
                in_block(&own, 100),
                contact(
                    &mined,
                    TransactionContext::InBlock(BlockInfo::new(100, BlockHash::all_zeros(), 0)),
                ),
            ],
            updated: Vec::new(),
            matured: Vec::new(),
            balance: WalletCoreBalance::new(1_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        });
        let block_marks = (hub.pump.take_wallet_marks().1, hub.pump.take_new_txs());
        hub.apply_wallet_event(&WalletEvent::TransactionDetected {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            record: Box::new(contact(&seen, TransactionContext::Mempool)),
            balance: WalletCoreBalance::new(1_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        });
        let tx_marks = (hub.pump.take_wallet_marks().1, hub.pump.take_new_txs());
        let hidden: std::collections::HashSet<Txid> =
            (hub.history).read_wallet(&W, |h| h.hidden().map(|(t, _)| *t).collect());
        assert_eq!(
            block_marks,
            (
                BTreeMap::from([(W, crate::pump::TxidSet::Some([own.txid()].into()))]),
                BTreeMap::from([(
                    W,
                    NewTxs {
                        txids: vec![own.txid()],
                        live: BTreeSet::new(),
                    }
                )]),
            ),
            "the block event marks its own record alone"
        );
        assert_eq!(
            tx_marks,
            (BTreeMap::new(), BTreeMap::new()),
            "a contact's record marks nothing"
        );
        assert_eq!(hidden, [mined.txid(), seen.txid()].into());
    }

    fn detected(record: TransactionRecord) -> WalletEvent {
        WalletEvent::TransactionDetected {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            record: Box::new(record),
            balance: WalletCoreBalance::new(1_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        }
    }

    fn block_of(
        height: u32,
        inserted: Vec<TransactionRecord>,
        updated: Vec<TransactionRecord>,
    ) -> WalletEvent {
        WalletEvent::BlockProcessed {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            height,
            chain_lock: None,
            inserted,
            updated,
            matured: Vec::new(),
            balance: WalletCoreBalance::new(1_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        }
    }

    /// A hub whose headers are at `current` of the peers' `target`.
    fn headers_at(current: u32, target: u32) -> SessionHub {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        hub.tracker().set_headers_syncing(current, target);
        hub
    }

    fn new_txs(hub: &SessionHub) -> NewTxs {
        hub.pump.take_new_txs().remove(&W).unwrap_or_default()
    }

    /// Issue #141: a payment relayed from the mempool, or with its
    /// InstantSend lock, while SPV is still catching up is live.
    #[test]
    fn a_mempool_or_instantsend_payment_during_catch_up_is_live() {
        let hub = headers_at(600, 1000);
        let (mempool, locked) = (tx(1), tx(2));
        let lock = dashcore::InstantLock {
            txid: locked.txid(),
            ..dashcore::InstantLock::default()
        };
        hub.apply_wallet_event(&detected(seen(&mempool, TransactionContext::Mempool)));
        hub.apply_wallet_event(&detected(seen(
            &locked,
            TransactionContext::InstantSend(lock),
        )));
        let batch = new_txs(&hub);
        assert!(!hub.tracker().caught_up());
        assert_eq!(batch.txids, [mempool.txid(), locked.txid()]);
        assert_eq!(batch.live_in_order(), batch.txids);
    }

    /// A block's transactions are live when the block is at most
    /// `LIVE_DEPTH` below the peers' best height, whatever height the scan
    /// is at; an older block's are the scan's findings, and so are those of
    /// a block above that height, which the reading has not reached.
    #[test]
    fn a_blocks_transactions_are_live_only_near_the_network_tip() {
        let hub = headers_at(600, 1000);
        let [old, behind, near, tip, above] = [1, 2, 3, 4, 5].map(tx);
        hub.apply_wallet_event(&block_of(600, vec![in_block(&old, 600)], vec![]));
        hub.apply_wallet_event(&block_of(997, vec![in_block(&behind, 997)], vec![]));
        hub.apply_wallet_event(&block_of(998, vec![in_block(&near, 998)], vec![]));
        let locked_tip = seen(
            &tip,
            TransactionContext::InChainLockedBlock(BlockInfo::new(1000, BlockHash::all_zeros(), 0)),
        );
        hub.apply_wallet_event(&block_of(1000, vec![locked_tip], vec![]));
        hub.apply_wallet_event(&block_of(1001, vec![in_block(&above, 1001)], vec![]));
        let batch = new_txs(&hub);
        assert_eq!(
            batch.txids,
            [&old, &behind, &near, &tip, &above].map(|t| t.txid())
        );
        assert_eq!(batch.live_in_order(), [near.txid(), tip.txid()]);
    }

    /// Before dash-spv reports a best height, no block's transaction is
    /// live; a mempool one still is.
    #[test]
    fn without_a_best_height_only_off_chain_payments_are_live() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let (mined, relayed) = (tx(1), tx(2));
        hub.apply_wallet_event(&block_of(5, vec![in_block(&mined, 5)], vec![]));
        hub.apply_wallet_event(&detected(seen(&relayed, TransactionContext::Mempool)));
        assert_eq!(new_txs(&hub).live_in_order(), [relayed.txid()]);
    }

    /// A payment seen in the mempool and then in its block is named once,
    /// live, in the batch of its mempool sighting.
    #[test]
    fn a_payment_seen_then_mined_is_named_once() {
        let hub = headers_at(600, 1000);
        let t = tx(1);
        hub.apply_wallet_event(&detected(seen(&t, TransactionContext::Mempool)));
        assert_eq!(new_txs(&hub).live_in_order(), [t.txid()]);
        hub.apply_wallet_event(&block_of(1000, vec![], vec![in_block(&t, 1000)]));
        assert_eq!(new_txs(&hub), NewTxs::default());
    }

    /// A wallet restored from its phrase scans years of blocks: none of its
    /// old payments is live, and a payment relayed meanwhile is, in the
    /// same batch.
    #[test]
    fn a_restore_scanning_years_of_history_finds_no_live_payment() {
        let hub = headers_at(2_400_000, 2_400_000);
        let mut found = Vec::new();
        for (n, height) in (1..=40u8).zip((1_000_000..).step_by(35_000)) {
            let t = tx(n);
            hub.apply_wallet_event(&block_of(height, vec![in_block(&t, height)], vec![]));
            found.push(t.txid());
        }
        let relayed = tx(100);
        hub.apply_wallet_event(&detected(seen(&relayed, TransactionContext::Mempool)));
        found.push(relayed.txid());
        let batch = new_txs(&hub);
        assert_eq!(batch.txids, found);
        assert_eq!(batch.live_in_order(), [relayed.txid()]);
    }

    /// The reconcile's balance replaces an event's doubled one, and is
    /// announced.
    #[test]
    fn the_reconciled_balance_replaces_an_events() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height: 100,
        });
        hub.apply_wallet_event(&WalletEvent::BlockProcessed {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            height: 100,
            chain_lock: None,
            inserted: Vec::new(),
            updated: Vec::new(),
            matured: Vec::new(),
            balance: WalletCoreBalance::new(2_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        });
        hub.pump.take_wallet_marks();

        hub.publish_balances(
            W,
            &WalletCoreBalance::new(1_000, 0, 0, 0),
            &BTreeMap::new(),
            None,
            None,
        );
        assert_eq!(hub.wallet_state(&W).unwrap().balances().confirmed, 1_000);
        assert!(hub.pump.take_wallet_marks().0.contains(&W));
        hub.publish_balances(
            W,
            &WalletCoreBalance::new(1_000, 0, 0, 0),
            &BTreeMap::new(),
            None,
            None,
        );
        assert!(
            !hub.pump.take_wallet_marks().0.contains(&W),
            "unchanged: not announced"
        );
    }

    /// A fork that lowers the checkpoint outside a rescan tells whoever
    /// read the wallet's scan height; one that leaves it where it was does
    /// not.
    #[test]
    fn a_truncation_that_lowers_the_checkpoint_marks_sync() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height: 110,
        });
        assert!(hub.pump.take_sync_mark());
        hub.apply_wallet_event(&truncated(120, Vec::new(), Vec::new(), 0));
        assert_eq!(hub.wallet_state(&W).unwrap().synced_height, 110);
        assert!(!hub.pump.sync_marked());
        hub.apply_wallet_event(&truncated(105, Vec::new(), Vec::new(), 0));
        assert_eq!(hub.wallet_state(&W).unwrap().synced_height, 105);
        assert!(hub.pump.sync_marked());
    }

    /// #818: a coin's hold going marks the abandoned payment that spent it,
    /// so its details stop saying the coin is being checked.
    #[test]
    fn a_released_hold_marks_the_abandoned_payment_that_spent_its_coin() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height: 110,
        });
        let (spender, other) = (tx(1), tx(2));
        let coin = spender.input[0].previous_output;
        hub.history.with_wallet(W, |h| {
            h.mark_abandoned(&[
                (spender.txid(), spender.clone()),
                (other.txid(), other.clone()),
            ]);
            h.rescan_holds.insert(
                coin,
                crate::rescan_hold::RescanHold {
                    checkpoint: 100,
                    target: 110,
                    armed: true,
                },
            );
        });
        hub.pump.take_wallet_marks();

        assert_eq!(
            hub.settle_rescan_holds(W, Some(110)),
            BTreeSet::from([coin])
        );
        let mut history = hub.pump.take_wallet_marks().1;
        let Some(TxidSet::Some(changed)) = history.remove(&W) else {
            panic!("history marks: {history:?}");
        };
        assert_eq!(changed, BTreeSet::from([spender.txid()]));
    }

    /// dash-spv truncated the wallet at a fork: the hub's history, heights and balance
    /// follow it, and hosts hear of each.
    #[test]
    fn a_fork_truncation_lowers_the_wallet_and_its_history() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let (kept, removed, locked) = (tx(1), tx(2), tx(3));
        hub.apply_wallet_event(&WalletEvent::BlockProcessed {
            locked_outpoints: Vec::new(),
            wallet_id: W.0,
            height: 110,
            chain_lock: None,
            inserted: vec![
                in_block(&kept, 100),
                in_block(&removed, 108),
                in_block(&locked, 109),
            ],
            updated: Vec::new(),
            matured: Vec::new(),
            balance: WalletCoreBalance::new(3_000, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: Vec::new(),
        });
        hub.apply_wallet_event(&WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height: 110,
        });
        hub.pump.take_wallet_marks();

        hub.apply_wallet_event(&truncated(
            105,
            vec![removed.txid()],
            vec![locked.txid()],
            2_000,
        ));

        let state = hub.wallet_state(&W).unwrap();
        assert_eq!((state.synced_height, state.processed_height), (105, 105));
        assert_eq!(state.balances().confirmed, 2_000);
        assert!(hub.history.get(&W, &removed.txid()).is_none());
        let entry = hub.history.get(&W, &locked.txid()).unwrap();
        assert_eq!(entry.context, TransactionContext::Mempool);
        assert!(entry.instant_locked);
        assert!(hub.history.get(&W, &kept.txid()).is_some());
        let (balances, history) = hub.pump.take_wallet_marks();
        assert!(balances.contains(&W));
        let Some(TxidSet::Some(changed)) = history.get(&W) else {
            panic!("history marks: {history:?}");
        };
        assert!(changed.contains(&removed.txid()) && changed.contains(&locked.txid()));

        // Truncated below the first scanned block: the wallet is catching up again,
        // so hosts hear of it although the amount didn't change.
        hub.apply_wallet_event(&truncated(0, Vec::new(), Vec::new(), 2_000));
        assert!(hub.wallet_state(&W).unwrap().balances().catching_up);
        assert!(hub.pump.take_wallet_marks().0.contains(&W));
    }

    /// 0.1.x-bal §1: a replay's target is the highest filter checkpoint the
    /// hub saw. A replay (lower `SyncHeightAdvanced` heights) keeps it, a
    /// fork lowers it, and a cancelled replay holds spends until a rewind
    /// to where it stopped.
    #[test]
    fn the_replay_target_is_the_highest_checkpoint_seen() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let advanced = |height| WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height,
        };
        let state = || hub.wallet_state(&W).unwrap();
        let replaying = || state().replaying_at(state().synced_height);
        hub.apply_wallet_event(&advanced(110));
        assert_eq!(state().scan_target, 110);
        assert!(!replaying());
        hub.apply_wallet_event(&advanced(50));
        assert_eq!(state().scan_target, 110);
        assert!(replaying());
        assert!(state().replaying_at(109) && !state().replaying_at(110));
        hub.apply_wallet_event(&truncated(105, Vec::new(), Vec::new(), 0));
        assert_eq!(state().scan_target, 105);
        hub.apply_wallet_event(&advanced(105));
        assert!(!replaying());

        hub.update_wallet_state(W, |s| s.replay_cancelled = Some(80));
        assert!(replaying());
        hub.apply_wallet_event(&advanced(106));
        assert!(replaying(), "no rewind since the cancel");
        hub.update_wallet_state(W, |s| s.note_rewind(81));
        assert!(replaying(), "block 81 stays unreplayed");
        hub.update_wallet_state(W, |s| s.note_rewind(80));
        assert!(!replaying());
    }

    /// 0.1.x-bal §1 (Sol r1 M2): `refresh_wallet_state` reads key-wallet,
    /// then merges the read into the hub under one lock. An advance or a
    /// fork truncation the hub applies between the read and the merge
    /// stands: the merge never writes back a target from before them.
    #[test]
    fn a_refresh_keeps_an_advance_or_a_fork_applied_after_its_read() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let advanced = |height| WalletEvent::SyncHeightAdvanced {
            wallet_id: W.0,
            height,
        };
        let read = |synced_height| WalletState {
            synced_height,
            ..WalletState::default()
        };
        let target = || hub.wallet_state(&W).unwrap().scan_target;
        hub.apply_wallet_event(&advanced(1_000));
        // Read during a replay (500); an advance to 1200 from before the
        // rewind lands, then the merge.
        let r = read(500);
        hub.apply_wallet_event(&advanced(1_200));
        hub.update_wallet_state(W, |s| s.take_core(r));
        assert_eq!(target(), 1_200);
        assert!(hub.wallet_state(&W).unwrap().replaying_at(500));
        // Read after a fork truncated the checkpoint to 900; its event lands,
        // then the merge.
        let r = read(900);
        hub.apply_wallet_event(&truncated(900, Vec::new(), Vec::new(), 0));
        hub.update_wallet_state(W, |s| s.take_core(r));
        assert_eq!(target(), 900);
        assert!(!hub.wallet_state(&W).unwrap().replaying_at(900));
    }
}
