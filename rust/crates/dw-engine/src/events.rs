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
use dash_spv::network::NetworkEvent;
use dash_spv::sync::SyncProgress;
use dashcore::Txid;
use key_wallet::account::AccountType;
use key_wallet::transaction_checking::TransactionContext;
use key_wallet::wallet::balance::WalletCoreBalance;
use platform_wallet::PlatformEventHandler;
use platform_wallet::events::WalletEvent;
use platform_wallet::manager::shielded_sync::{ShieldedSyncPassSummary, WalletShieldedOutcome};

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
            | Self::Rates => true,
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
    /// The wallet is being opened and its stored replay not yet resumed:
    /// spends wait (`NetworkSession::load_wallet`).
    pub loading: bool,
    /// The height of the last truncation the hub found in key-wallet
    /// ([`SessionHub::detect_truncation`]) and has not yet heard the
    /// `ChainTruncated` event of: the event at this height, or one above
    /// it that would lower nothing, is that truncation's, heard late.
    pub detected: Option<u32>,
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
            ..read
        };
    }

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
    /// Runs on each SPV peers 0 -> >0 transition: the recorded sweep
    /// chunks are sent again (`coinjoin`); unset in hub-only tests.
    peers_up: OnceLock<Box<dyn Fn() + Send + Sync>>,
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
    /// Tests: the sweep row checks started, to wait for.
    #[cfg(test)]
    pub(crate) sweep_checks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// The session's replay records, told of each fork truncation.
    replay: OnceLock<std::sync::Weak<crate::replay_record::ReplayRecords>>,
    /// Tests: holds the next first-seen write before it runs.
    #[cfg(test)]
    pub(crate) pause_seen: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next wallet open before its replay is resumed.
    #[cfg(test)]
    pub(crate) pause_load: Mutex<Option<Arc<crate::coins::TestPause>>>,
    #[cfg(test)]
    pub(crate) pause_abandon: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next resume between its record read and its rewind.
    #[cfg(test)]
    pub(crate) pause_resume: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next rescan's publication, after its rewind.
    #[cfg(test)]
    pub(crate) pause_publish: Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next truncation between its heights and the rest.
    #[cfg(test)]
    pub(crate) pause_truncation: Mutex<Option<(Arc<std::sync::Barrier>, Arc<std::sync::Barrier>)>>,
    /// Held through each fork truncation's application
    /// ([`Self::apply_truncation`]), and through a check for one
    /// ([`Self::detect_truncation`]): a check waits for the heights and
    /// the history of one being applied, not only its heights.
    truncating: Mutex<()>,
    /// The wallets unloaded or removed, and not added again: an event of
    /// one is stale, from before.
    unloaded: Mutex<HashSet<WalletId>>,
}

impl SessionHub {
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
            peers_up: OnceLock::new(),
            sweep_news: OnceLock::new(),
            sweep_retry_due: Mutex::new(HashSet::new()),
            sweep_retry: OnceLock::new(),
            headers: RwLock::new(None),
            header_tip: Mutex::new(None),
            #[cfg(test)]
            sweep_checks: Mutex::new(Vec::new()),
            replay: OnceLock::new(),
            #[cfg(test)]
            pause_seen: Mutex::new(None),
            #[cfg(test)]
            pause_load: Mutex::new(None),
            #[cfg(test)]
            pause_abandon: Mutex::new(None),
            #[cfg(test)]
            pause_resume: Mutex::new(None),
            #[cfg(test)]
            pause_publish: Mutex::new(None),
            #[cfg(test)]
            pause_truncation: Mutex::new(None),
            truncating: Mutex::new(()),
            unloaded: Mutex::new(HashSet::new()),
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

    /// Starts wallet `id`'s reconcile unless one is started and not yet
    /// running: events during a run start the next one.
    fn schedule_reconcile(&self, id: WalletId) {
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

    /// A reconcile of wallet `id` begins: later events start another.
    pub(crate) fn reconcile_running(&self, id: WalletId) {
        self.reconcile_pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// Publishes key-wallet's own balances for wallet `id` (after the engine
    /// changed its coins) and announces them when they differ.
    pub(crate) fn publish_core_balance(
        &self,
        id: WalletId,
        balance: &WalletCoreBalance,
        accounts: &BTreeMap<AccountType, WalletCoreBalance>,
    ) {
        if self.update_balance(id, balance, accounts) {
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

    /// Forgets the in-memory state of an unloaded wallet; its name stays
    /// (the "Open Wallet" menu lists it).
    pub(crate) fn unload_wallet(&self, id: &WalletId) {
        (self.unloaded.lock().unwrap_or_else(|p| p.into_inner())).insert(*id);
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id);
        self.history.remove_wallet(id);
        if let Some(rescan) = self.rescan().as_mut() {
            rescan.wallets.remove(id);
        }
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
    pub(crate) fn admit(&self, id: WalletId) {
        (self.unloaded.lock().unwrap_or_else(|p| p.into_inner())).remove(&id);
        self.history.with_wallet(id, |h| h.held.clear());
    }

    /// Forgets everything about a removed wallet.
    pub(crate) fn forget_wallet(&self, id: &WalletId) {
        (self.unloaded.lock().unwrap_or_else(|p| p.into_inner())).insert(*id);
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
            state.scan_target = state.scan_target.max(h);
        }
        if let Some(h) = processed {
            state.processed_height = state.processed_height.max(h);
        }
        known_before != state.balances().is_some()
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
                let known_before = state.balances().is_some();
                let from = state.scan_target.max(state.processed_height);
                state.synced_height = state.synced_height.min(height);
                state.scan_target = state.scan_target.min(height);
                state.processed_height = state.processed_height.min(height);
                if found.detected {
                    state.detected = Some(height);
                }
                (from, known_before != state.balances().is_some())
            })
        };
        let Some((from, flipped)) = lowered else {
            return false;
        };
        #[cfg(test)]
        if let Some((reached, release)) = self.pause_truncation.lock().unwrap().take() {
            reached.wait();
            release.wait();
        }
        self.note_sweep_news(id, crate::coinjoin::SweepNews::Truncated { height });
        let changed = self
            .history
            .with_wallet(id, |h| h.truncate(found.removed, found.unconfirmed));
        if let Some(rescan) = self.rescan().as_mut()
            && rescan.truncate(&id, height)
        {
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
    /// nothing tells it apart from one mined below the fork. Key-wallet's
    /// ChainLock bounds nothing here: it can be the new chain's.
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
        let held: HashMap<Txid, bool> = (accounts.iter())
            .flat_map(|a| a.transactions().iter())
            .filter(|(_, r)| crate::history::is_wallets(r))
            .map(|(t, r)| (*t, r.context.block_info().is_some()))
            .collect();
        let finalized = |t: &Txid| accounts.iter().any(|a| a.transaction_is_finalized(t));
        let (mut removed, mut unconfirmed) = (Vec::new(), Vec::new());
        self.history.read_wallet(&id, |h| {
            for (txid, entry) in &h.txs {
                let Some(mined) = entry.context.block_info().map(|b| b.height()) else {
                    continue;
                };
                let is_final = matches!(entry.context, TransactionContext::InChainLockedBlock(_))
                    || finalized(txid);
                if mined <= height && (!h.held.contains(txid) || is_final) {
                    continue;
                }
                match held.get(txid) {
                    None => removed.push(*txid),
                    Some(false) => unconfirmed.push(*txid),
                    Some(true) => {}
                }
            }
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
        if (self.unloaded.lock().unwrap_or_else(|p| p.into_inner())).contains(&id) {
            return;
        }
        let now = Some(unix_now());
        self.run_sweep_retry(id);
        let carried = carried_txids(event);
        if !carried.is_empty() {
            self.history.with_wallet(id, |h| h.held.extend(carried));
        }
        match event {
            WalletEvent::TransactionDetected {
                record,
                balance,
                account_balances,
                ..
            } => {
                if crate::history::is_wallets(record) {
                    if self.history.with_wallet(id, |h| h.upsert(record, now)) {
                        self.pump.mark_new_txs(id, &[record.txid]);
                    }
                    self.pump.mark_history(id, Some(&[record.txid]));
                }
                if self.update_balance(id, balance, account_balances) {
                    self.pump.mark_balances(id);
                }
                // First seen through its InstantSend lock: evidence. In
                // the mempool: none (BL-76).
                if matches!(record.context, TransactionContext::InstantSend(_)) {
                    self.note_seen(id, [record.txid]);
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
                self.note_seen(id, [*txid]);
            }
            WalletEvent::TransactionsSwept {
                txids,
                superseded_by,
                winner_block,
                balance,
                account_balances,
                ..
            } => {
                // The winner may pay no address of the wallet, and then no
                // other event names it: a sweep chunk it beat learns of it
                // here only.
                self.note_sweep_news(
                    id,
                    crate::coinjoin::SweepNews::Beaten {
                        txids,
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
                let changed: Vec<Txid> = self.history.with_wallet(id, |h| {
                    inserted
                        .iter()
                        .chain(updated)
                        .chain(matured)
                        .filter(|r| crate::history::is_wallets(r))
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
                // In this block: inserted, or confirmed now. A matured
                // record's block was evidence already.
                self.note_seen(id, inserted.iter().chain(updated).map(|r| r.txid));
                self.schedule_reconcile(id);
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
                self.schedule_reconcile(id);
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
                self.note_seen(id, locked_transactions.values().flatten().copied());
            }
        }
    }
}

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
        WalletEvent::SyncHeightAdvanced { .. } => Vec::new(),
        WalletEvent::ChainLockProcessed {
            locked_transactions,
            ..
        } => locked_transactions.values().flatten().copied().collect(),
    }
}

impl EventHandler for SessionHub {
    fn on_progress(&self, progress: &SyncProgress) {
        let (changed, masternodes_synced, tip) = {
            let mut tracker = self.tracker();
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
    /// synced re-reads its balance and activity. A skipped pass (another in
    /// flight) names no wallet.
    fn on_platform_address_sync_completed(
        &self,
        summary: &platform_wallet::manager::platform_address_sync::PlatformAddressSyncSummary,
    ) {
        for (id, outcome) in &summary.wallet_results {
            if outcome.is_ok() {
                self.pump
                    .mark_platform(WalletId(*id), [PlatformChange::PlatformAddresses]);
            }
        }
    }

    /// A shielded sync pass that found notes or spends of a wallet's account
    /// (not a cooldown skip) tells its hosts (m4 §2.13).
    fn on_shielded_sync_completed(&self, summary: &ShieldedSyncPassSummary) {
        for (id, outcome) in &summary.wallet_results {
            if let WalletShieldedOutcome::Ok(pass) = outcome
                && !pass.is_cooldown_skip
                && (pass
                    .notes_result
                    .new_notes_per_account
                    .values()
                    .any(|n| *n > 0)
                    || pass.newly_spent_per_account.values().any(|n| *n > 0))
            {
                self.pump
                    .mark_platform(WalletId(*id), [PlatformChange::Shielded]);
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
                balances: state.balances(),
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
        if seen.is_empty() {
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

#[cfg(test)]
mod tests {
    use dashcore::hashes::Hash;
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
        TransactionRecord::new(
            t.clone(),
            AccountType::Standard {
                index: 0,
                standard_account_type: StandardAccountType::BIP44Account,
            },
            TransactionContext::InBlock(BlockInfo::new(height, BlockHash::all_zeros(), 0)),
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

        hub.publish_core_balance(W, &WalletCoreBalance::new(1_000, 0, 0, 0), &BTreeMap::new());
        assert_eq!(
            hub.wallet_state(&W).unwrap().balances().unwrap().confirmed,
            1_000
        );
        assert!(hub.pump.take_wallet_marks().0.contains(&W));
        hub.publish_core_balance(W, &WalletCoreBalance::new(1_000, 0, 0, 0), &BTreeMap::new());
        assert!(
            !hub.pump.take_wallet_marks().0.contains(&W),
            "unchanged: not announced"
        );
    }

    /// dash-spv truncated the wallet at a fork: the hub's history, heights and balance
    /// follow it, and hosts hear of each.
    #[test]
    fn a_fork_truncation_lowers_the_wallet_and_its_history() {
        let hub = SessionHub::new(DashNetwork::Regtest, Arc::new(NoEvents));
        let (kept, removed, locked) = (tx(1), tx(2), tx(3));
        hub.apply_wallet_event(&WalletEvent::BlockProcessed {
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
        assert_eq!(state.balances().unwrap().confirmed, 2_000);
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

        // Truncated below the first scanned block: the balance is unknown again, so
        // hosts hear of it although the amount didn't change.
        hub.apply_wallet_event(&truncated(0, Vec::new(), Vec::new(), 2_000));
        assert!(hub.wallet_state(&W).unwrap().balances().is_none());
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
