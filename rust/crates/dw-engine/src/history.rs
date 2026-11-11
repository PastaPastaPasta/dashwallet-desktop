//! Transaction history read model (QT-086…094, IOS-027/028/031).
//!
//! [`HistoryStore`] keeps every wallet transaction in memory: the raw
//! transaction, the latest context platform-wallet reported for it, and the
//! values of the wallet's own inputs. It is filled at session open from the
//! persisted records in `wallet.sqlite` and kept current from wallet events.
//! (platform-wallet evicts chainlocked records from its own in-memory map, so
//! the store is the only complete in-memory history.)
//!
//! Records are computed on demand by [`decompose`], which follows dash-qt's
//! `TransactionRecord::decomposeTransaction`: one record per received output,
//! one per non-change output of a send with the fee on the first, a single
//! record for self-sends, CoinJoin internals, masternode and asset-lock
//! transactions, and `Other` for mixed debit transactions.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, RwLock};

use dashcore::blockdata::transaction::special_transaction::TransactionPayload;
use dashcore::blockdata::transaction::special_transaction::TransactionType as SpecialType;
use dashcore::{Address, Network, OutPoint, ScriptBuf, Transaction, Txid};
use dw_protx::payloads::resolve_collateral;
use key_wallet::managed_account::transaction_record::TransactionRecord;
use key_wallet::transaction_checking::TransactionContext;

use crate::platform::Counterparty;
use crate::platform::attribution::Payees;
use crate::{EngineError, WalletId};

/// Confirmations dash-qt recommends before a transaction counts as
/// confirmed (`TransactionRecord::RecommendedNumConfirmations`).
pub const RECOMMENDED_CONFIRMATIONS: u32 = 6;
/// Coinbase outputs are spendable after this many confirmations plus one
/// (Dash Core `COINBASE_MATURITY`).
pub const COINBASE_MATURITY: u32 = 100;
/// Largest page `history_page` returns.
pub const MAX_PAGE: u32 = 500;
/// Longest search text accepted by the history filter.
pub const MAX_SEARCH_TEXT: usize = 256;

/// CoinJoin denominations in duffs, largest first (research 02 §9.1).
pub const COINJOIN_DENOMINATIONS: [u64; 5] =
    [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001];
/// CoinJoin collateral range: smallest denomination / 10 up to four times that.
pub const COINJOIN_COLLATERAL_MIN: u64 = 10_000;
pub const COINJOIN_COLLATERAL_MAX: u64 = 40_000;

pub fn is_denominated_amount(value: u64) -> bool {
    COINJOIN_DENOMINATIONS.contains(&value)
}

pub fn is_collateral_amount(value: u64) -> bool {
    (COINJOIN_COLLATERAL_MIN..=COINJOIN_COLLATERAL_MAX).contains(&value)
}

/// dash-qt `TransactionRecord::Type`, same order (research 02 §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TxType {
    Other,
    Generated,
    SendToAddress,
    SendToOther,
    RecvWithAddress,
    RecvFromOther,
    SendToSelf,
    RecvWithCoinJoin,
    CoinJoinMixing,
    CoinJoinCollateralPayment,
    CoinJoinMakeCollaterals,
    CoinJoinCreateDenominations,
    CoinJoinSend,
    PlatformTransfer,
    DustReceive,
    DataTransaction,
    MasternodeRegistration,
    MasternodeUpdate,
    AssetLock,
}

/// iOS history filter categories (IOS-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxCategory {
    Sent,
    Received,
    Reward,
    Masternode,
    InternalTransfer,
    CoinJoin,
    Platform,
    Other,
}

impl TxType {
    pub fn category(self) -> TxCategory {
        match self {
            TxType::Generated => TxCategory::Reward,
            TxType::SendToAddress
            | TxType::SendToOther
            | TxType::CoinJoinSend
            | TxType::DataTransaction => TxCategory::Sent,
            TxType::RecvWithAddress
            | TxType::RecvFromOther
            | TxType::RecvWithCoinJoin
            | TxType::DustReceive => TxCategory::Received,
            TxType::SendToSelf => TxCategory::InternalTransfer,
            TxType::CoinJoinMixing
            | TxType::CoinJoinCollateralPayment
            | TxType::CoinJoinMakeCollaterals
            | TxType::CoinJoinCreateDenominations => TxCategory::CoinJoin,
            TxType::PlatformTransfer | TxType::AssetLock => TxCategory::Platform,
            TxType::MasternodeRegistration | TxType::MasternodeUpdate => TxCategory::Masternode,
            TxType::Other => TxCategory::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TxStatusKind {
    Unconfirmed,
    Confirming,
    Confirmed,
    Conflicted,
    Abandoned,
    Immature,
    NotAccepted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxStatus {
    pub kind: TxStatusKind,
    pub confirmations: u32,
    pub instant_locked: bool,
    pub chain_locked: bool,
    pub matures_in: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxRecord {
    pub txid: String,
    pub record_index: u32,
    pub tx_type: TxType,
    pub category: TxCategory,
    pub status: TxStatus,
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    pub amount: i64,
    pub fee: Option<u64>,
    pub address: Option<String>,
    pub label: Option<String>,
    pub counts_toward_balance: bool,
    pub involves_watch_only: bool,
    /// The DashPay identity on the other side of a contact payment, sent
    /// or received (DP3-02, `platform/attribution.rs`).
    pub counterparty: Option<Counterparty>,
    /// The price the transaction was first seen at (MP-01); `None` when it
    /// was not stamped (no back-fill).
    pub fiat_then: Option<crate::rates::TxFiat>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchOnlyFilter {
    All,
    Yes,
    No,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryFilter {
    pub types: Vec<TxType>,
    pub categories: Vec<TxCategory>,
    pub statuses: Vec<TxStatusKind>,
    pub date_from: Option<u64>,
    pub date_to: Option<u64>,
    pub text: Option<String>,
    pub min_amount: Option<u64>,
    pub watch_only: WatchOnlyFilter,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        Self {
            types: Vec::new(),
            categories: Vec::new(),
            statuses: Vec::new(),
            date_from: None,
            date_to: None,
            text: None,
            min_amount: None,
            watch_only: WatchOnlyFilter::All,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HistorySort {
    NewestFirst,
    OldestFirst,
    AmountDescending,
    AmountAscending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryQuery {
    pub filter: HistoryFilter,
    pub sort: HistorySort,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryPage {
    pub records: Vec<TxRecord>,
    pub next_cursor: Option<String>,
    pub total_matching: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxInputDetail {
    /// Txid (display order) and vout of the spent output.
    pub previous_txid: String,
    pub previous_vout: u32,
    pub address: Option<String>,
    pub amount: Option<u64>,
    pub is_mine: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxOutputDetail {
    pub vout: u32,
    pub address: Option<String>,
    pub amount: u64,
    pub is_mine: bool,
    pub is_change: bool,
    pub data_hex: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxDetail {
    pub txid: String,
    pub records: Vec<TxRecord>,
    pub status: TxStatus,
    pub timestamp: Option<u64>,
    pub block_height: Option<u32>,
    pub block_hash: Option<String>,
    pub fee: Option<u64>,
    pub size_bytes: u32,
    pub inputs: Vec<TxInputDetail>,
    pub outputs: Vec<TxOutputDetail>,
    pub message: Option<String>,
    pub label: Option<String>,
    pub raw_hex: String,
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// One wallet transaction as the store keeps it.
#[derive(Debug, Clone)]
pub(crate) struct TxEntry {
    pub tx: Transaction,
    /// Latest context platform-wallet reported (see [`merge_context`]).
    pub context: TransactionContext,
    /// An InstantSend lock was seen for this transaction at some point.
    pub instant_locked: bool,
    /// UNIX seconds this device first saw the transaction; `None` for
    /// transactions only known from storage.
    pub first_seen: Option<u64>,
    /// Values and addresses of the wallet's own inputs, by input index,
    /// from platform-wallet's records.
    pub own_inputs: BTreeMap<u32, (u64, Address)>,
    /// The user abandoned it (`abandon_transaction`). Shown as `Abandoned`
    /// while it stays out of a block.
    pub abandoned: bool,
}

impl TxEntry {
    /// Abandoned as Dash Core counts it: the user's abandon stands only
    /// while the transaction is in no block and not InstantSend-locked
    /// (`status_of`'s `Abandoned`; a lock outranks the abandon).
    pub(crate) fn is_abandoned(&self) -> bool {
        self.abandoned
            && self.context.block_info().is_none()
            && !self.instant_locked
            && !matches!(self.context, TransactionContext::InstantSend(_))
    }
}

fn context_rank(c: &TransactionContext) -> u8 {
    match c {
        TransactionContext::Mempool => 0,
        TransactionContext::InstantSend(_) => 1,
        TransactionContext::InBlock(_) => 2,
        TransactionContext::InChainLockedBlock(_) => 3,
    }
}

/// The context to keep when a record reports `new` for a transaction known
/// with `old`. Newer reports win (a reorg moves a block transaction back to
/// the mempool), except that a ChainLocked block is final and an InstantSend
/// lock does not fall back to plain mempool.
fn merge_context(old: &TransactionContext, new: &TransactionContext) -> TransactionContext {
    match (old, new) {
        (TransactionContext::InChainLockedBlock(_), n) if context_rank(n) < 3 => old.clone(),
        (TransactionContext::InstantSend(_), TransactionContext::Mempool) => old.clone(),
        _ => new.clone(),
    }
}

/// The outpoints the wallet's transactions spend by Dash Core's
/// `CWallet::IsSpent` (v24.0.0-rc.6 8d195f18 wallet/wallet.cpp:755-770): the
/// spender is in a block, or out of one (depth 0, :4072-4082) and not
/// abandoned. A conflicted spender (depth < 0) does not count. The store has
/// no conflict state: a spender out of a block is conflicted when another
/// transaction in a block spends one of its inputs, and so are its
/// descendants out of a block, as `MarkConflicted` marks them through
/// `RecursiveUpdateTxState` (:1388-1458). A block spending one of its
/// inputs that is not the wallet's is not seen (BIP158 filters match the
/// wallet's scripts only): such a spender holds its other inputs until it
/// is abandoned.
pub(crate) fn spent_outpoints(txs: &HashMap<Txid, TxEntry>) -> HashSet<OutPoint> {
    let mined = |e: &TxEntry| e.context.block_info().is_some();
    let in_block: HashMap<OutPoint, Txid> = (txs.iter().filter(|(_, e)| mined(e)))
        .flat_map(|(t, e)| e.tx.input.iter().map(move |i| (i.previous_output, *t)))
        .collect();
    let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
    let mut todo = Vec::new();
    for (t, e) in txs.iter().filter(|(_, e)| !mined(e)) {
        for i in &e.tx.input {
            children.entry(i.previous_output.txid).or_default().push(*t);
            if in_block.get(&i.previous_output).is_some_and(|b| b != t) {
                todo.push(*t);
            }
        }
    }
    let mut conflicted = HashSet::new();
    while let Some(t) = todo.pop() {
        if conflicted.insert(t) {
            todo.extend(children.get(&t).into_iter().flatten());
        }
    }
    (txs.iter())
        .filter(|(t, e)| mined(e) || !(e.is_abandoned() || conflicted.contains(*t)))
        .flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output))
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct WalletHistory {
    pub txs: HashMap<Txid, TxEntry>,
    /// Transactions key-wallet held this session: its records when the
    /// history was filled at open and each one its events named since.
    /// Records only read from storage are not among them (platform-wallet
    /// rebuilds no records at load).
    pub held: HashSet<Txid>,
    /// A ProRegTx, or an output one names as its collateral, was merged
    /// since the masternode follower last locked this wallet's collaterals
    /// (MG-00 D9).
    collateral_pending: bool,
    /// The collateral outpoint of every ProRegTx merged, so a coin that
    /// arrives after its ProRegTx (a restore past the gap) marks the wallet
    /// as its ProRegTx did. Unchecked: a mark for an outpoint the lock step
    /// then refuses locks nothing. Never pruned: a stale entry costs at most
    /// one follower step that locks nothing.
    named_collaterals: HashSet<OutPoint>,
    /// The txids of the coins the wallet held when its history last loaded
    /// with every stored record read, whose transaction was stored (with
    /// its record or as a height-only row): their events came before the
    /// load, so no later one is awaited for them. Empty after a load that
    /// failed to read a record, or read one missing or another's. A
    /// contact's record comes only from a store written before
    /// platform-wallet stopped persisting those; a contact's ProRegTx is
    /// never left to this hold alone: its collateral is locked from
    /// `hidden_registrations`.
    loaded_coins: HashSet<Txid>,
    /// The ProRegTxs whose record is a DashPay contact account's, which
    /// [`Self::upsert`] skips: the history shows none of them, yet each
    /// still names a collateral, locked as an own ProRegTx's is
    /// ([`HistoryStore::registration_collaterals`]).
    hidden_registrations: HashMap<Txid, Transaction>,
}

impl WalletHistory {
    /// Sets the coins the wallet held at a load (empty when the load failed
    /// to read a record, or read one missing or another's).
    pub fn set_loaded_coins(&mut self, txids: HashSet<Txid>) {
        self.loaded_coins = txids;
    }

    /// Names the collateral of `record`'s transaction, when it is a
    /// ProRegTx, for the follower's next lock step. Returns whether it is.
    fn name_registration_collateral(&mut self, record: &TransactionRecord) -> bool {
        let Some(TransactionPayload::ProviderRegistrationPayloadType(p)) =
            &record.transaction.special_transaction_payload
        else {
            return false;
        };
        (self.named_collaterals).insert(resolve_collateral(record.txid, p.collateral_outpoint));
        self.collateral_pending = true;
        true
    }

    /// Keeps a skipped (contact's) record that is a ProRegTx out of sight,
    /// its collateral named and marked as an own ProRegTx's is.
    fn hide_registration(&mut self, record: &TransactionRecord) {
        if self.name_registration_collateral(record) {
            (self.hidden_registrations).insert(record.txid, record.transaction.clone());
        }
    }

    /// Drops a transaction upstream deleted, seen or kept out of sight.
    pub fn forget(&mut self, txid: &Txid) {
        self.txs.remove(txid);
        self.hidden_registrations.remove(txid);
    }

    /// The ProRegTxs kept out of sight that the history has no own entry
    /// for.
    pub(crate) fn hidden(&self) -> impl Iterator<Item = (&Txid, &Transaction)> {
        (self.hidden_registrations.iter()).filter(|(t, _)| !self.txs.contains_key(*t))
    }
}

/// Whether `record` is the wallet's own. A DashPay contact's account
/// (`AccountType::is_contact_owned`, key-wallet `account_type.rs:257-300`)
/// watches the contact's coins: its records (the contact spending what we
/// paid them, or the output side of our payment) are never the wallet's
/// activity. Our payment to a contact shows from the paying account's record.
pub(crate) fn is_wallets(record: &TransactionRecord) -> bool {
    !record.account_type.is_contact_owned()
}

impl WalletHistory {
    /// Merges a platform-wallet record. `seen_at` is set as the first-seen
    /// time when the transaction is new to the store. Returns whether it was
    /// new; a contact's record ([`is_wallets`]) is skipped (false), a
    /// ProRegTx among them kept out of sight for its collateral only.
    pub fn upsert(&mut self, record: &TransactionRecord, seen_at: Option<u64>) -> bool {
        if !is_wallets(record) {
            self.hide_registration(record);
            return false;
        }
        let new = !self.txs.contains_key(&record.txid);
        let entry = self.txs.entry(record.txid).or_insert_with(|| TxEntry {
            tx: record.transaction.clone(),
            context: record.context.clone(),
            instant_locked: false,
            first_seen: seen_at,
            own_inputs: BTreeMap::new(),
            abandoned: false,
        });
        entry.context = merge_context(&entry.context, &record.context);
        if matches!(record.context, TransactionContext::InstantSend(_)) {
            entry.instant_locked = true;
        }
        for input in &record.input_details {
            entry
                .own_inputs
                .insert(input.index, (input.value, input.address.clone()));
        }
        self.name_registration_collateral(record);
        if (record.output_details.iter())
            .any(|o| (self.named_collaterals).contains(&OutPoint::new(record.txid, o.index)))
        {
            self.collateral_pending = true;
        }
        new
    }

    pub fn set_instant_locked(&mut self, txid: &Txid) -> bool {
        match self.txs.get_mut(txid) {
            Some(e) if !e.instant_locked => {
                e.instant_locked = true;
                true
            }
            _ => false,
        }
    }

    /// A fork replaced the blocks above the wallet's truncation height:
    /// `removed` are gone, and `unconfirmed` (InstantSend-locked, so the
    /// network mines them again) are out of a block, locked. Returns the
    /// transactions that changed.
    pub fn truncate(&mut self, removed: &[Txid], unconfirmed: &[Txid]) -> Vec<Txid> {
        for txid in removed {
            self.hidden_registrations.remove(txid);
        }
        let mut changed: Vec<Txid> = removed
            .iter()
            .filter(|t| self.txs.remove(*t).is_some())
            .copied()
            .collect();
        for txid in unconfirmed {
            if let Some(e) = self.txs.get_mut(txid) {
                e.context = TransactionContext::Mempool;
                e.instant_locked = true;
                changed.push(*txid);
            }
        }
        changed
    }

    /// Upgrades a transaction in a block to ChainLocked.
    pub fn set_chain_locked(&mut self, txid: &Txid) -> bool {
        match self.txs.get_mut(txid) {
            Some(e) => match &e.context {
                TransactionContext::InBlock(info) => {
                    e.context = TransactionContext::InChainLockedBlock(*info);
                    true
                }
                _ => false,
            },
            None => false,
        }
    }
}

/// In-memory history of every wallet of a session.
#[derive(Debug, Default)]
pub(crate) struct HistoryStore {
    wallets: RwLock<HashMap<WalletId, WalletHistory>>,
    /// Woken when a wallet's collaterals become pending.
    collateral_wake: Arc<tokio::sync::Notify>,
}

impl HistoryStore {
    pub fn with_wallet<R>(&self, wallet: WalletId, f: impl FnOnce(&mut WalletHistory) -> R) -> R {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        let h = map.entry(wallet).or_default();
        let pending = h.collateral_pending;
        let r = f(h);
        if !pending && h.collateral_pending {
            self.collateral_wake.notify_one();
        }
        r
    }

    /// The wallets with a ProRegTx, or a coin one names, merged since the
    /// last call, cleared.
    pub fn take_collateral_pending(&self) -> Vec<WalletId> {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        map.iter_mut()
            .filter_map(|(id, h)| std::mem::take(&mut h.collateral_pending).then_some(*id))
            .collect()
    }

    /// Marks `wallet`'s collaterals pending again, for the next poll; no
    /// wake.
    pub fn rearm_collateral(&self, wallet: WalletId) {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        map.entry(wallet).or_default().collateral_pending = true;
    }

    pub fn collateral_wake(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.collateral_wake)
    }

    /// A copy of one wallet's entries (empty when unknown).
    pub fn snapshot(&self, wallet: &WalletId) -> HashMap<Txid, TxEntry> {
        self.read(wallet, HashMap::clone)
    }

    /// Runs `f` over one wallet's entries under the store's read lock, with
    /// no copy (empty when unknown).
    pub fn read<R>(&self, wallet: &WalletId, f: impl FnOnce(&HashMap<Txid, TxEntry>) -> R) -> R {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        match map.get(wallet) {
            Some(h) => f(&h.txs),
            None => f(&HashMap::new()),
        }
    }

    /// The collaterals the wallet's ProRegTxs name, with the amount each
    /// type needs: every ProRegTx the history counts (not abandoned), and
    /// every one it keeps out of sight (a contact's record) with no own
    /// entry.
    pub fn registration_collaterals(&self, wallet: &WalletId) -> Vec<(OutPoint, u64)> {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        let Some(h) = map.get(wallet) else {
            return Vec::new();
        };
        crate::masternodes::registration_collaterals(
            (h.txs.values())
                .filter(|e| !e.is_abandoned())
                .map(|e| &e.tx)
                .chain(h.hidden().map(|(_, tx)| tx)),
        )
    }

    /// The collaterals the ProRegTxs kept out of sight (a contact's record)
    /// with no own entry name.
    pub fn hidden_registration_collaterals(&self, wallet: &WalletId) -> Vec<(OutPoint, u64)> {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        (map.get(wallet)).map_or_else(Vec::new, |h| {
            crate::masternodes::registration_collaterals(h.hidden().map(|(_, tx)| tx))
        })
    }

    /// The collateral of `pro_tx_hash`, a ProRegTx kept out of sight with
    /// no own entry, when it names one.
    pub fn hidden_registration_collateral(
        &self,
        wallet: &WalletId,
        pro_tx_hash: &Txid,
    ) -> Option<OutPoint> {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        let (_, tx) = map.get(wallet)?.hidden().find(|(t, _)| *t == pro_tx_hash)?;
        let collaterals = crate::masternodes::registration_collaterals([tx]);
        collaterals.first().map(|(o, _)| *o)
    }

    /// Runs `f` over one wallet's entries and the txids of the coins it held
    /// at its last clean load ([`WalletHistory`]'s `loaded_coins`), under the
    /// store's read lock.
    pub fn read_with_loaded<R>(
        &self,
        wallet: &WalletId,
        f: impl FnOnce(&HashMap<Txid, TxEntry>, &HashSet<Txid>) -> R,
    ) -> R {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        match map.get(wallet) {
            Some(h) => f(&h.txs, &h.loaded_coins),
            None => f(&HashMap::new(), &HashSet::new()),
        }
    }

    /// One transaction of a wallet.
    pub fn get(&self, wallet: &WalletId, txid: &Txid) -> Option<TxEntry> {
        self.wallets
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(wallet)
            .and_then(|h| h.txs.get(txid).cloned())
    }

    /// Runs `f` over one wallet's history under the store's read lock
    /// (empty when unknown).
    pub fn read_wallet<R>(&self, wallet: &WalletId, f: impl FnOnce(&WalletHistory) -> R) -> R {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        match map.get(wallet) {
            Some(h) => f(h),
            None => f(&WalletHistory::default()),
        }
    }

    pub fn remove_wallet(&self, wallet: &WalletId) {
        self.wallets
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(wallet);
    }

    /// Txids of the wallet's transactions whose status can still change with
    /// a new block: unconfirmed or fewer than [`RECOMMENDED_CONFIRMATIONS`],
    /// and coinbase outputs that have not matured.
    pub fn young_txids(&self, wallet: &WalletId, tip: u32) -> Vec<Txid> {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        let Some(h) = map.get(wallet) else {
            return Vec::new();
        };
        h.txs
            .iter()
            .filter(|(_, e)| {
                let depth = depth(&e.context, tip);
                if e.tx.is_coin_base() {
                    depth <= COINBASE_MATURITY + 1
                } else {
                    depth <= RECOMMENDED_CONFIRMATIONS
                        && !matches!(e.context, TransactionContext::InChainLockedBlock(_))
                }
            })
            .map(|(t, _)| *t)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Ownership and chain context
// ---------------------------------------------------------------------------

/// Which chain of the wallet an owned script belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AddressChain {
    Receiving,
    Change,
}

/// One script the wallet owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Owned {
    pub address: Address,
    pub chain: AddressChain,
    /// Owned by a CoinJoin account (dash-qt treats those outputs as change).
    pub coinjoin: bool,
}

/// Inputs to [`decompose`] beyond the transaction itself.
pub(crate) struct ChainView<'a> {
    pub owned: &'a HashMap<ScriptBuf, Owned>,
    /// Every transaction of the wallet, for input values.
    pub history: &'a HashMap<Txid, TxEntry>,
    /// Height of the wallet's best processed block.
    pub tip: u32,
    /// Height of the best ChainLock, if any.
    pub chainlock_height: Option<u32>,
    pub network: Network,
    /// Who the wallet's DashPay payments are with.
    pub payees: &'a Payees,
}

fn depth(context: &TransactionContext, tip: u32) -> u32 {
    match context.block_info() {
        Some(info) if tip >= info.height() => tip - info.height() + 1,
        Some(_) => 1,
        None => 0,
    }
}

/// One wallet input: its value and address when the wallet owns it.
fn own_input(view: &ChainView<'_>, entry: &TxEntry, index: usize) -> Option<(u64, Address)> {
    if let Some((v, a)) = entry.own_inputs.get(&(index as u32)) {
        return Some((*v, a.clone()));
    }
    let prev = &entry.tx.input.get(index)?.previous_output;
    let funding = view.history.get(&prev.txid)?;
    let out = funding.tx.output.get(prev.vout as usize)?;
    let owned = view.owned.get(&out.script_pubkey)?;
    Some((out.value, owned.address.clone()))
}

/// Status of a transaction (dash-qt `TransactionRecord::updateStatus`).
pub(crate) fn status_of(
    entry: &TxEntry,
    view: &ChainView<'_>,
    all_from_me: bool,
) -> (TxStatus, bool) {
    let depth = depth(&entry.context, view.tip);
    let height = entry.context.block_info().map(|b| b.height());
    let chain_locked = matches!(entry.context, TransactionContext::InChainLockedBlock(_))
        || matches!((height, view.chainlock_height), (Some(h), Some(cl)) if cl >= h);
    let instant_locked =
        entry.instant_locked || matches!(entry.context, TransactionContext::InstantSend(_));
    let mut matures_in = None;
    let kind = if entry.tx.is_coin_base() && depth <= COINBASE_MATURITY {
        if depth == 0 {
            TxStatusKind::NotAccepted
        } else {
            matures_in = Some(COINBASE_MATURITY + 1 - depth);
            TxStatusKind::Immature
        }
    } else if entry.is_abandoned() {
        TxStatusKind::Abandoned
    } else if depth == 0 {
        TxStatusKind::Unconfirmed
    } else if depth < RECOMMENDED_CONFIRMATIONS && !chain_locked {
        TxStatusKind::Confirming
    } else {
        TxStatusKind::Confirmed
    };
    let counts = !matches!(
        kind,
        TxStatusKind::Immature | TxStatusKind::NotAccepted | TxStatusKind::Abandoned
    ) && (depth > 0 || instant_locked || all_from_me);
    (
        TxStatus {
            kind,
            confirmations: depth,
            instant_locked,
            chain_locked,
            matures_in,
        },
        counts,
    )
}

/// The display time of a transaction: the earlier of first-seen and block
/// time, so a live payment keeps the time it arrived and a rescanned one
/// shows its block time (dash-qt `nTimeSmart` in spirit).
pub(crate) fn timestamp_of(entry: &TxEntry) -> Option<u64> {
    let block = entry.context.block_info().map(|b| u64::from(b.timestamp()));
    match (entry.first_seen, block) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// Record parts before status, labels and txid are attached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Part {
    pub tx_type: TxType,
    pub address: Option<String>,
    pub amount: i64,
    /// The output the record is about: a received output or a payee.
    pub vout: Option<u32>,
}

/// The transaction-level facts [`decompose`] derives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Decomposed {
    pub parts: Vec<Part>,
    pub fee: Option<u64>,
    pub all_from_me: bool,
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// The wallet's side of a transaction: credit (outputs paying the wallet),
/// debit (the wallet's inputs) and how many inputs are the wallet's.
pub(crate) fn amounts(entry: &TxEntry, view: &ChainView<'_>) -> (u64, u64, usize) {
    let credit = entry
        .tx
        .output
        .iter()
        .filter(|o| view.owned.contains_key(&o.script_pubkey))
        .map(|o| o.value)
        .sum();
    let own: Vec<u64> = (0..entry.tx.input.len())
        .filter_map(|i| own_input(view, entry, i).map(|(v, _)| v))
        .collect();
    (credit, own.iter().sum(), own.len())
}

/// Confirmation depth of a transaction at `tip` (0 = unconfirmed).
pub(crate) fn depth_of(entry: &TxEntry, tip: u32) -> u32 {
    depth(&entry.context, tip)
}

/// dash-qt `TransactionRecord::decomposeTransaction` over one transaction.
pub(crate) fn decompose(entry: &TxEntry, view: &ChainView<'_>) -> Decomposed {
    let tx = &entry.tx;
    let inputs: Vec<Option<(u64, Address)>> = (0..tx.input.len())
        .map(|i| own_input(view, entry, i))
        .collect();
    let outputs: Vec<Option<&Owned>> = tx
        .output
        .iter()
        .map(|o| view.owned.get(&o.script_pubkey))
        .collect();

    let from_me = inputs.iter().filter(|i| i.is_some()).count();
    let to_me = outputs.iter().filter(|o| o.is_some()).count();
    let all_from_me = !tx.input.is_empty() && from_me == tx.input.len() && !tx.is_coin_base();
    let all_to_me = !tx.output.is_empty() && to_me == tx.output.len();
    let debit: u64 = inputs.iter().flatten().map(|(v, _)| *v).sum();
    let credit: u64 = tx
        .output
        .iter()
        .zip(&outputs)
        .filter(|(_, o)| o.is_some())
        .map(|(o, _)| o.value)
        .sum();
    let net = to_i64(credit) - to_i64(debit);
    let value_out: u64 = tx.output.iter().map(|o| o.value).sum();
    let fee = (all_from_me && debit >= value_out).then(|| debit - value_out);

    let address_of = |vout: usize| {
        Address::from_script(&tx.output[vout].script_pubkey, view.network)
            .ok()
            .map(|a| a.to_string())
    };
    let single = |tx_type: TxType, amount: i64| Decomposed {
        parts: vec![Part {
            tx_type,
            address: None,
            amount,
            vout: None,
        }],
        fee,
        all_from_me,
    };

    if from_me == 0 && to_me == 0 {
        return Decomposed {
            parts: Vec::new(),
            fee: None,
            all_from_me: false,
        };
    }

    match tx.tx_type() {
        SpecialType::ProviderRegistration => return single(TxType::MasternodeRegistration, net),
        SpecialType::ProviderUpdateService
        | SpecialType::ProviderUpdateRegistrar
        | SpecialType::ProviderUpdateRevocation => return single(TxType::MasternodeUpdate, net),
        SpecialType::AssetLock if from_me > 0 => return single(TxType::AssetLock, net),
        _ => {}
    }
    let platform_transfer = matches!(tx.tx_type(), SpecialType::AssetUnlock);

    let mut parts = Vec::new();
    if net > 0 || tx.is_coin_base() || platform_transfer {
        for (vout, owned) in outputs.iter().enumerate() {
            let Some(owned) = owned else { continue };
            let tx_type = if tx.is_coin_base() {
                TxType::Generated
            } else if platform_transfer {
                TxType::PlatformTransfer
            } else {
                TxType::RecvWithAddress
            };
            parts.push(Part {
                tx_type,
                address: Some(owned.address.to_string()),
                amount: to_i64(tx.output[vout].value),
                vout: Some(vout as u32),
            });
        }
        return Decomposed {
            parts,
            fee,
            all_from_me,
        };
    }

    let own_input_values = || inputs.iter().flatten().map(|(v, _)| *v);
    let all_from_me_denom = from_me > 0 && own_input_values().all(is_denominated_amount);
    let all_to_me_denom = to_me > 0
        && tx
            .output
            .iter()
            .zip(&outputs)
            .filter(|(_, o)| o.is_some())
            .all(|(o, _)| is_denominated_amount(o.value));

    if all_from_me_denom && all_to_me_denom {
        return Decomposed {
            parts: vec![Part {
                tx_type: TxType::CoinJoinMixing,
                address: None,
                amount: net,
                vout: None,
            }],
            fee,
            all_from_me,
        };
    }

    if all_from_me && all_to_me {
        let change: u64 = tx
            .output
            .iter()
            .zip(&outputs)
            .filter_map(|(o, owned)| {
                owned
                    .filter(|w| w.chain == AddressChain::Change || w.coinjoin)
                    .map(|_| o.value)
            })
            .sum();
        let mut tx_type = TxType::SendToSelf;
        if tx.input.len() == 1
            && tx.output.len() == 1
            && is_collateral_amount(debit)
            && is_collateral_amount(credit)
            && net < 0
            && is_collateral_amount(net.unsigned_abs())
        {
            tx_type = TxType::CoinJoinCollateralPayment;
        } else {
            let make_collateral = match tx.output.as_slice() {
                [a, b] => {
                    let (a, b) = (a.value, b.value);
                    (a == COINJOIN_COLLATERAL_MAX
                        && !is_denominated_amount(b)
                        && b >= COINJOIN_COLLATERAL_MIN)
                        || (b == COINJOIN_COLLATERAL_MAX
                            && !is_denominated_amount(a)
                            && a >= COINJOIN_COLLATERAL_MIN)
                        || (a == b && is_collateral_amount(a))
                }
                [a] => is_collateral_amount(a.value),
                _ => false,
            };
            if make_collateral {
                tx_type = TxType::CoinJoinMakeCollaterals;
            } else if tx.output.iter().any(|o| is_denominated_amount(o.value)) {
                tx_type = TxType::CoinJoinCreateDenominations;
            }
        }
        let debit_part = -(to_i64(debit) - to_i64(change));
        let credit_part = to_i64(credit) - to_i64(change);
        return Decomposed {
            parts: vec![Part {
                tx_type,
                address: None,
                amount: credit_part + debit_part,
                vout: None,
            }],
            fee,
            all_from_me,
        };
    }

    if all_from_me {
        if tx.input.len() == 1
            && tx.output.len() == 1
            && is_collateral_amount(debit)
            && credit == 0
            && net < 0
            && is_collateral_amount(net.unsigned_abs())
        {
            return single(TxType::CoinJoinCollateralPayment, -to_i64(debit));
        }
        // A payment from the CoinJoin send page (QT-051): every input a
        // denomination of the CoinJoin account. dash-qt marks these with
        // `mapValue["DS"] = "1"` and shows them as CoinJoin Send
        // (`TransactionRecord::decomposeTransaction`); the inputs say the
        // same without a stored mark.
        let coinjoin_send = inputs.iter().flatten().all(|(v, a)| {
            is_denominated_amount(*v)
                && view
                    .owned
                    .get(&a.script_pubkey())
                    .is_some_and(|o| o.coinjoin)
        });
        let mut remaining_fee = fee.unwrap_or(0);
        for (vout, out) in tx.output.iter().enumerate() {
            if outputs[vout].is_some() {
                // Change back to the wallet.
                continue;
            }
            let (tx_type, address) = if out.script_pubkey.is_op_return() {
                (TxType::DataTransaction, None)
            } else {
                match address_of(vout) {
                    Some(a) if coinjoin_send => (TxType::CoinJoinSend, Some(a)),
                    Some(a) => (TxType::SendToAddress, Some(a)),
                    None => (TxType::SendToOther, None),
                }
            };
            let mut value = to_i64(out.value);
            if remaining_fee > 0 {
                value += to_i64(remaining_fee);
                remaining_fee = 0;
            }
            parts.push(Part {
                tx_type,
                address,
                amount: -value,
                vout: Some(vout as u32),
            });
        }
        return Decomposed {
            parts,
            fee,
            all_from_me,
        };
    }

    // Mixed debit transaction: payees cannot be broken down.
    Decomposed {
        parts: vec![Part {
            tx_type: TxType::Other,
            address: None,
            amount: net,
            vout: None,
        }],
        fee,
        all_from_me,
    }
}

/// What dw-appdb adds to the history: transaction labels by txid, address
/// labels by address, price stamps by txid (MP-01).
#[derive(Debug, Default, Clone)]
pub(crate) struct Labels {
    pub tx: HashMap<String, String>,
    pub address: HashMap<String, String>,
    pub fiat: HashMap<String, crate::rates::TxFiat>,
}

/// Every record of one transaction.
pub(crate) fn records_of(
    txid: &Txid,
    entry: &TxEntry,
    view: &ChainView<'_>,
    labels: &Labels,
) -> (Vec<TxRecord>, Decomposed, TxStatus) {
    let d = decompose(entry, view);
    let (status, counts) = status_of(entry, view, d.all_from_me);
    let txid_s = txid.to_string();
    let tx_label = labels.tx.get(&txid_s).cloned();
    let sides = view.payees.sides(txid, &entry.tx, &d.parts);
    let records = d
        .parts
        .iter()
        .zip(&sides)
        .enumerate()
        .map(|(i, (p, side))| TxRecord {
            txid: txid_s.clone(),
            record_index: i as u32,
            tx_type: p.tx_type,
            category: p.tx_type.category(),
            status: status.clone(),
            timestamp: timestamp_of(entry),
            block_height: entry.context.block_info().map(|b| b.height()),
            amount: p.amount,
            fee: d.fee,
            address: p.address.clone(),
            label: tx_label.clone().or_else(|| {
                p.address
                    .as_ref()
                    .and_then(|a| labels.address.get(a).cloned())
            }),
            counts_toward_balance: counts,
            involves_watch_only: false,
            counterparty: side.and_then(|s| view.payees.counterparty(&s.contact)),
            fiat_then: labels.fiat.get(&txid_s).cloned(),
        })
        .collect();
    (records, d, status)
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

fn matches(r: &TxRecord, f: &HistoryFilter, text: Option<&str>) -> bool {
    if !f.types.is_empty() && !f.types.contains(&r.tx_type) {
        return false;
    }
    if !f.categories.is_empty() && !f.categories.contains(&r.category) {
        return false;
    }
    if !f.statuses.is_empty() && !f.statuses.contains(&r.status.kind) {
        return false;
    }
    if f.date_from.is_some() || f.date_to.is_some() {
        let Some(ts) = r.timestamp else { return false };
        if f.date_from.is_some_and(|from| ts < from) || f.date_to.is_some_and(|to| ts >= to) {
            return false;
        }
    }
    if let Some(min) = f.min_amount
        && r.amount.unsigned_abs() < min
    {
        return false;
    }
    match f.watch_only {
        WatchOnlyFilter::All => {}
        WatchOnlyFilter::Yes if !r.involves_watch_only => return false,
        WatchOnlyFilter::No if r.involves_watch_only => return false,
        _ => {}
    }
    if let Some(text) = text {
        let hit = r.txid.contains(text)
            || r.address
                .as_deref()
                .is_some_and(|a| a.to_lowercase().contains(text))
            || r.label
                .as_deref()
                .is_some_and(|l| l.to_lowercase().contains(text));
        if !hit {
            return false;
        }
    }
    true
}

/// Total order of one sort: primary key, then txid, then record index.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SortKey {
    primary: i128,
    txid: String,
    index: u32,
}

fn key_of(r: &TxRecord, sort: HistorySort) -> SortKey {
    let primary = match sort {
        // Unknown times sort as newest: they are fresh mempool arrivals.
        HistorySort::NewestFirst | HistorySort::OldestFirst => {
            i128::from(r.timestamp.unwrap_or(u64::MAX))
        }
        HistorySort::AmountDescending | HistorySort::AmountAscending => i128::from(r.amount),
    };
    SortKey {
        primary,
        txid: r.txid.clone(),
        index: r.record_index,
    }
}

fn compare(a: &SortKey, b: &SortKey, sort: HistorySort) -> Ordering {
    let primary = match sort {
        HistorySort::NewestFirst | HistorySort::AmountDescending => b.primary.cmp(&a.primary),
        HistorySort::OldestFirst | HistorySort::AmountAscending => a.primary.cmp(&b.primary),
    };
    primary
        .then_with(|| a.txid.cmp(&b.txid))
        .then_with(|| a.index.cmp(&b.index))
}

/// FNV-1a over the query shape, so a cursor is only accepted by the query
/// that produced it.
fn query_tag(filter: &HistoryFilter, sort: HistorySort) -> u64 {
    let text = format!("{filter:?}|{sort:?}");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn encode_cursor(tag: u64, key: &SortKey) -> String {
    format!("v1.{tag:016x}.{}.{}.{}", key.primary, key.txid, key.index)
}

fn decode_cursor(cursor: &str, tag: u64) -> Result<SortKey, EngineError> {
    let mut it = cursor.split('.');
    let (Some("v1"), Some(t), Some(p), Some(txid), Some(i), None) = (
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
        it.next(),
    ) else {
        return Err(EngineError::StaleCursor);
    };
    if u64::from_str_radix(t, 16).ok() != Some(tag) {
        return Err(EngineError::StaleCursor);
    }
    Ok(SortKey {
        primary: p.parse().map_err(|_| EngineError::StaleCursor)?,
        txid: txid.to_string(),
        index: i.parse().map_err(|_| EngineError::StaleCursor)?,
    })
}

pub(crate) fn validate_query(q: &HistoryQuery) -> Result<(), EngineError> {
    if q.limit == 0 || q.limit > MAX_PAGE {
        return Err(EngineError::InvalidQuery(format!(
            "limit {} outside 1..={MAX_PAGE}",
            q.limit
        )));
    }
    validate_filter(&q.filter)
}

/// Checks a filter's date range and search text.
pub(crate) fn validate_filter(filter: &HistoryFilter) -> Result<(), EngineError> {
    let q = HistoryQuery {
        filter: filter.clone(),
        sort: HistorySort::NewestFirst,
        cursor: None,
        limit: 1,
    };
    if let (Some(from), Some(to)) = (q.filter.date_from, q.filter.date_to)
        && from > to
    {
        return Err(EngineError::InvalidQuery(format!(
            "date_from {from} is after date_to {to}"
        )));
    }
    if q.filter
        .text
        .as_ref()
        .is_some_and(|t| t.chars().count() > MAX_SEARCH_TEXT)
    {
        return Err(EngineError::InvalidQuery(format!(
            "search text longer than {MAX_SEARCH_TEXT} characters"
        )));
    }
    Ok(())
}

/// Filters, sorts and pages `records`.
///
/// Cursors are keyset cursors: they name the last record returned, so
/// transactions arriving between pages never invalidate them; a page simply
/// continues after that record. A cursor from a different filter or sort is
/// `StaleCursor`.
///
/// Pure: it pages `records` for `q` and touches no state.
/// `NetworkSession::history_page` runs it over a wallet's records; dw-app's
/// shared history vectors run it over a fixed set.
pub fn page(records: Vec<TxRecord>, q: &HistoryQuery) -> Result<HistoryPage, EngineError> {
    validate_query(q)?;
    let tag = query_tag(&q.filter, q.sort);
    let after = q
        .cursor
        .as_deref()
        .map(|c| decode_cursor(c, tag))
        .transpose()?;
    let keyed = filter_sorted(records, &q.filter, q.sort);
    let total = u32::try_from(keyed.len()).ok();
    let start = match &after {
        Some(k) => keyed.partition_point(|(rk, _)| compare(rk, k, q.sort) != Ordering::Greater),
        None => 0,
    };
    let limit = q.limit as usize;
    let rest = keyed.len().saturating_sub(start);
    let take: Vec<(SortKey, TxRecord)> = keyed.into_iter().skip(start).take(limit).collect();
    let next_cursor = (rest > limit)
        .then(|| take.last().map(|(k, _)| encode_cursor(tag, k)))
        .flatten();
    Ok(HistoryPage {
        records: take.into_iter().map(|(_, r)| r).collect(),
        next_cursor,
        total_matching: total,
    })
}

/// The records `filter` keeps, in `sort` order, with their sort keys.
fn filter_sorted(
    mut records: Vec<TxRecord>,
    filter: &HistoryFilter,
    sort: HistorySort,
) -> Vec<(SortKey, TxRecord)> {
    let text = filter
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase);
    records.retain(|r| matches(r, filter, text.as_deref()));
    let mut keyed: Vec<(SortKey, TxRecord)> =
        records.into_iter().map(|r| (key_of(&r, sort), r)).collect();
    keyed.sort_by(|a, b| compare(&a.0, &b.0, sort));
    keyed
}

/// Every record `filter` keeps, in `sort` order (the CSV export's rows).
pub(crate) fn filtered(
    records: Vec<TxRecord>,
    filter: &HistoryFilter,
    sort: HistorySort,
) -> Result<Vec<TxRecord>, EngineError> {
    validate_filter(filter)?;
    Ok(filter_sorted(records, filter, sort)
        .into_iter()
        .map(|(_, r)| r)
        .collect())
}

/// Push data of an `OP_RETURN` script, hex.
pub(crate) fn op_return_payload(script: &ScriptBuf) -> Option<String> {
    use dashcore::blockdata::script::Instruction;
    if !script.is_op_return() {
        return None;
    }
    let mut data = Vec::new();
    for ins in script.instructions().skip(1) {
        match ins {
            Ok(Instruction::PushBytes(b)) => data.extend_from_slice(b.as_bytes()),
            Ok(Instruction::Op(_)) => {}
            Err(_) => break,
        }
    }
    Some(hex::encode(data))
}

/// The detail view of one transaction.
pub(crate) fn detail_of(
    txid: &Txid,
    entry: &TxEntry,
    view: &ChainView<'_>,
    labels: &Labels,
    message: Option<String>,
) -> TxDetail {
    use dashcore::consensus::encode::serialize_hex;
    let (records, d, status) = records_of(txid, entry, view, labels);
    let inputs = entry
        .tx
        .input
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let own = own_input(view, entry, i);
            let funding_out = view
                .history
                .get(&input.previous_output.txid)
                .and_then(|f| f.tx.output.get(input.previous_output.vout as usize));
            TxInputDetail {
                previous_txid: input.previous_output.txid.to_string(),
                previous_vout: input.previous_output.vout,
                address: own.as_ref().map(|(_, a)| a.to_string()).or_else(|| {
                    funding_out.and_then(|o| {
                        Address::from_script(&o.script_pubkey, view.network)
                            .ok()
                            .map(|a| a.to_string())
                    })
                }),
                amount: own
                    .as_ref()
                    .map(|(v, _)| *v)
                    .or(funding_out.map(|o| o.value)),
                is_mine: own.is_some(),
            }
        })
        .collect();
    let outputs = entry
        .tx
        .output
        .iter()
        .enumerate()
        .map(|(vout, o)| {
            let owned = view.owned.get(&o.script_pubkey);
            TxOutputDetail {
                vout: vout as u32,
                address: Address::from_script(&o.script_pubkey, view.network)
                    .ok()
                    .map(|a| a.to_string()),
                amount: o.value,
                is_mine: owned.is_some(),
                is_change: owned.is_some_and(|w| w.chain == AddressChain::Change || w.coinjoin),
                data_hex: op_return_payload(&o.script_pubkey),
            }
        })
        .collect();
    TxDetail {
        txid: txid.to_string(),
        records,
        status,
        timestamp: timestamp_of(entry),
        block_height: entry.context.block_info().map(|b| b.height()),
        block_hash: entry
            .context
            .block_info()
            .map(|b| b.block_hash().to_string()),
        fee: d.fee,
        size_bytes: u32::try_from(entry.tx.size()).unwrap_or(u32::MAX),
        inputs,
        outputs,
        message,
        label: labels.tx.get(&txid.to_string()).cloned(),
        raw_hex: serialize_hex(&entry.tx),
    }
}

#[cfg(test)]
impl HistoryStore {
    /// Tests: `wallet`'s history holds `txid`, a transaction with no
    /// outputs or payload, as a fixture coin's event would have left it
    /// (an entry already there stays).
    pub(crate) fn hold_funding(&self, wallet: WalletId, txid: Txid) {
        use dashcore::hashes::Hash;
        use key_wallet::transaction_checking::BlockInfo;
        self.with_wallet(wallet, |h| {
            h.txs.entry(txid).or_insert_with(|| TxEntry {
                tx: Transaction {
                    version: 2,
                    lock_time: 0,
                    input: Vec::new(),
                    output: Vec::new(),
                    special_transaction_payload: None,
                },
                context: TransactionContext::InChainLockedBlock(BlockInfo::new(
                    1,
                    dashcore::BlockHash::all_zeros(),
                    0,
                )),
                instant_locked: false,
                first_seen: None,
                own_inputs: BTreeMap::new(),
                abandoned: false,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::{BlockHash, OutPoint, PubkeyHash, ScriptBuf, TxIn, TxOut, Witness};
    use key_wallet::account::{AccountType, StandardAccountType};
    use key_wallet::managed_account::transaction_record::{InputDetail, TransactionDirection};
    use key_wallet::transaction_checking::{BlockInfo, TransactionType};

    use super::*;

    const NET: Network = Network::Regtest;

    fn hash_addr(n: u8) -> Address {
        Address::new(
            NET,
            dashcore::address::Payload::PubkeyHash(PubkeyHash::from_byte_array([n; 20])),
        )
    }

    fn out(value: u64, a: &Address) -> TxOut {
        TxOut {
            value,
            script_pubkey: a.script_pubkey(),
        }
    }

    fn input(txid: Txid, vout: u32) -> TxIn {
        TxIn {
            previous_output: OutPoint { txid, vout },
            script_sig: ScriptBuf::new(),
            sequence: u32::MAX,
            witness: Witness::default(),
        }
    }

    fn tx(inputs: Vec<TxIn>, outputs: Vec<TxOut>) -> Transaction {
        Transaction {
            version: 2,
            lock_time: 0,
            input: inputs,
            output: outputs,
            special_transaction_payload: None,
        }
    }

    fn entry(t: Transaction, context: TransactionContext) -> TxEntry {
        TxEntry {
            tx: t,
            context,
            instant_locked: false,
            first_seen: None,
            own_inputs: BTreeMap::new(),
            abandoned: false,
        }
    }

    fn in_block(height: u32, time: u32) -> TransactionContext {
        TransactionContext::InBlock(BlockInfo::new(height, BlockHash::all_zeros(), time))
    }

    struct Fixture {
        owned: HashMap<ScriptBuf, Owned>,
        history: HashMap<Txid, TxEntry>,
        payees: Payees,
    }

    impl Fixture {
        fn new() -> Self {
            let mut owned = HashMap::new();
            for (n, chain) in [
                (1, AddressChain::Receiving),
                (2, AddressChain::Receiving),
                (9, AddressChain::Change),
            ] {
                let a = hash_addr(n);
                owned.insert(
                    a.script_pubkey(),
                    Owned {
                        address: a,
                        chain,
                        coinjoin: false,
                    },
                );
            }
            Self {
                owned,
                history: HashMap::new(),
                payees: Payees::default(),
            }
        }

        fn view(&self, tip: u32) -> ChainView<'_> {
            ChainView {
                owned: &self.owned,
                history: &self.history,
                tip,
                chainlock_height: None,
                network: NET,
                payees: &self.payees,
            }
        }

        fn add(&mut self, e: TxEntry) -> Txid {
            let id = e.tx.txid();
            self.history.insert(id, e);
            id
        }
    }

    fn foreign_input(n: u8) -> TxIn {
        input(Txid::from_byte_array([n; 32]), 0)
    }

    #[test]
    fn receive_is_one_record_per_owned_output() {
        let mut f = Fixture::new();
        let t = tx(
            vec![foreign_input(7)],
            vec![
                out(150_000_000, &hash_addr(1)),
                out(5, &hash_addr(50)),
                out(25_000_000, &hash_addr(2)),
            ],
        );
        let id = f.add(entry(t, in_block(100, 1_700_000_000)));
        let view = f.view(100);
        let (records, d, status) = records_of(&id, &f.history[&id], &view, &Labels::default());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].tx_type, TxType::RecvWithAddress);
        assert_eq!(records[0].category, TxCategory::Received);
        assert_eq!(records[0].amount, 150_000_000);
        assert_eq!(
            records[0].address.as_deref(),
            Some(hash_addr(1).to_string().as_str())
        );
        assert_eq!(records[1].amount, 25_000_000);
        assert_eq!(records[1].record_index, 1);
        assert_eq!(d.fee, None, "the wallet did not fund it");
        assert_eq!(status.kind, TxStatusKind::Confirming);
        assert_eq!(status.confirmations, 1);
        assert!(records[0].counts_toward_balance);
        assert_eq!(records[0].timestamp, Some(1_700_000_000));
    }

    fn record(
        t: &Transaction,
        account: AccountType,
        inputs: Vec<InputDetail>,
        net: i64,
    ) -> TransactionRecord {
        TransactionRecord::new(
            t.clone(),
            account,
            in_block(20, 2),
            TransactionType::Standard,
            TransactionDirection::Outgoing,
            inputs,
            Vec::new(),
            net,
        )
    }

    /// A ProRegTx with its own collateral (output 0), `seed` setting its
    /// txid, and the collateral that names.
    fn own_registration(seed: u16) -> (Transaction, (OutPoint, u64)) {
        use crate::masternodes::provider_fixtures::{coins, null, registration};
        let amount = dw_protx::params::collateral_amount(false);
        let t = registration(null(0), false, seed, coins(&[amount]));
        let collateral = OutPoint::new(t.txid(), 0);
        (t, (collateral, amount))
    }

    fn registration_record(t: &Transaction, account: AccountType) -> TransactionRecord {
        TransactionRecord::new(
            t.clone(),
            account,
            TransactionContext::Mempool,
            TransactionType::Standard,
            TransactionDirection::Internal,
            Vec::new(),
            Vec::new(),
            0,
        )
    }

    fn contact() -> AccountType {
        AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [0xaa; 32],
            friend_identity_id: [0xbb; 32],
        }
    }

    fn bip44() -> AccountType {
        AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        }
    }

    /// A ProRegTx with both a contact's record and the wallet's own names
    /// its collateral once, from the own entry.
    #[test]
    fn an_own_entry_outranks_a_contacts_copy_of_its_registration() {
        let store = HistoryStore::default();
        let wallet = WalletId([1; 32]);
        let (t, collateral) = own_registration(1);
        store.with_wallet(wallet, |h| {
            h.upsert(&registration_record(&t, contact()), None)
        });
        assert_eq!(store.registration_collaterals(&wallet), vec![collateral]);
        store.with_wallet(wallet, |h| {
            h.upsert(&registration_record(&t, bip44()), None)
        });
        assert_eq!(store.registration_collaterals(&wallet), vec![collateral]);
        assert!(store.hidden_registration_collaterals(&wallet).is_empty());
    }

    /// An abandoned own ProRegTx is not locked again through a contact's
    /// copy of it.
    #[test]
    fn an_abandoned_registration_is_not_locked_through_a_contacts_copy() {
        let store = HistoryStore::default();
        let wallet = WalletId([1; 32]);
        let (t, _) = own_registration(1);
        store.with_wallet(wallet, |h| {
            h.upsert(&registration_record(&t, contact()), None);
            h.upsert(&registration_record(&t, bip44()), None);
            h.txs.get_mut(&t.txid()).unwrap().abandoned = true;
        });
        assert!(store.registration_collaterals(&wallet).is_empty());
        assert!(store.hidden_registration_collaterals(&wallet).is_empty());
    }

    /// A ProRegTx kept out of sight goes with its transaction: upstream's
    /// deletion forgets it, a fork's removal drops it, and a fork leaves an
    /// unconfirmed one in place.
    #[test]
    fn a_hidden_registration_is_forgotten_and_truncated_with_its_transaction() {
        let store = HistoryStore::default();
        let wallet = WalletId([1; 32]);
        let (forgotten, _) = own_registration(1);
        let (removed, _) = own_registration(2);
        let (kept, kept_collateral) = own_registration(3);
        for t in [&forgotten, &removed, &kept] {
            store.with_wallet(wallet, |h| {
                h.upsert(&registration_record(t, contact()), None)
            });
        }
        assert_eq!(store.registration_collaterals(&wallet).len(), 3);
        store.with_wallet(wallet, |h| h.forget(&forgotten.txid()));
        assert_eq!(store.registration_collaterals(&wallet).len(), 2, "forget");
        store.with_wallet(wallet, |h| h.truncate(&[removed.txid()], &[kept.txid()]));
        assert_eq!(
            store.registration_collaterals(&wallet),
            vec![kept_collateral],
            "truncate"
        );
    }

    /// TB-RESMOKE r-4-18/19: a contact spends the output we paid them, which
    /// their account (watched, never ours) records with the spent coin as
    /// its input. That is not the wallet's activity. Our payment to them
    /// shows once, from the paying account's record, net of its fee, and
    /// the CSV export lists the same.
    #[test]
    fn a_contacts_records_stay_out_of_activity() {
        let bip44 = AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        };
        let contact = AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [1; 32],
            friend_identity_id: [2; 32],
        };
        let theirs = hash_addr(70);
        let mut h = WalletHistory::default();
        let fund = tx(vec![foreign_input(7)], vec![out(1_000_000, &hash_addr(1))]);
        assert!(h.upsert(&record(&fund, bip44, Vec::new(), 1_000_000), None));
        let pay = tx(
            vec![input(fund.txid(), 0)],
            vec![out(250_000, &theirs), out(749_000, &hash_addr(9))],
        );
        let ours = InputDetail {
            index: 0,
            value: 1_000_000,
            address: hash_addr(1),
        };
        assert!(h.upsert(&record(&pay, bip44, vec![ours], -251_000), None));
        assert!(!h.upsert(&record(&pay, contact, Vec::new(), 250_000), None));
        let spend = tx(
            vec![input(pay.txid(), 0)],
            vec![out(249_000, &hash_addr(80))],
        );
        let their_coin = InputDetail {
            index: 0,
            value: 250_000,
            address: theirs,
        };
        assert!(!h.upsert(&record(&spend, contact, vec![their_coin], -250_000), None));

        assert!(
            !h.txs.contains_key(&spend.txid()),
            "no Activity entry for the contact's spend"
        );
        let mut f = Fixture::new();
        f.history = h.txs;
        let view = f.view(20);
        let mut rows: Vec<TxRecord> = (f.history.iter())
            .flat_map(|(id, e)| records_of(id, e, &view, &Labels::default()).0)
            .collect();
        rows.sort_by_key(|r| r.amount);
        let paid: Vec<&TxRecord> = rows
            .iter()
            .filter(|r| r.txid == pay.txid().to_string())
            .collect();
        assert_eq!(paid.len(), 1, "our payment shows once");
        assert_eq!(
            (paid[0].amount, paid[0].fee),
            (-251_000, Some(1_000)),
            "net of its fee"
        );
        assert_eq!(rows.len(), 2, "the funding and the payment");

        let csv = crate::csv_export::write_csv(
            &rows,
            false,
            dw_units::Unit::Duffs,
            dw_units::Chain::Test,
            &[],
            0,
        );
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3, "a header and the same two rows: {csv}");
        assert!(
            lines[1].contains("\"-251000\"") && lines[1].contains(&pay.txid().to_string()),
            "{csv}"
        );
        assert!(!csv.contains(&spend.txid().to_string()));
    }

    #[test]
    fn send_has_one_record_per_payee_and_fee_on_the_first() {
        let mut f = Fixture::new();
        let fund = f.add(entry(
            tx(vec![foreign_input(7)], vec![out(1_000_000, &hash_addr(1))]),
            in_block(10, 1),
        ));
        let spend = tx(
            vec![input(fund, 0)],
            vec![
                out(300_000, &hash_addr(60)),
                out(200_000, &hash_addr(61)),
                out(499_000, &hash_addr(9)),
            ],
        );
        let id = f.add(entry(spend, TransactionContext::Mempool));
        let view = f.view(10);
        let (records, d, status) = records_of(&id, &f.history[&id], &view, &Labels::default());
        assert_eq!(d.fee, Some(1_000));
        assert_eq!(records.len(), 2, "change is not a record");
        assert_eq!(records[0].tx_type, TxType::SendToAddress);
        assert_eq!(records[0].amount, -301_000, "fee on the first payee");
        assert_eq!(records[1].amount, -200_000);
        assert_eq!(records[0].category, TxCategory::Sent);
        assert_eq!(status.kind, TxStatusKind::Unconfirmed);
        assert!(
            records[0].counts_toward_balance,
            "own unconfirmed sends count"
        );
    }

    #[test]
    fn self_send_coinjoin_internals_and_mixed() {
        let mut f = Fixture::new();
        let fund = f.add(entry(
            tx(
                vec![foreign_input(7)],
                vec![
                    out(2_000_000_000, &hash_addr(1)),
                    out(1_000_010, &hash_addr(2)),
                ],
            ),
            in_block(10, 1),
        ));
        // Payment to yourself: all in and out are ours.
        let to_self = tx(
            vec![input(fund, 0)],
            vec![out(1_999_990_000, &hash_addr(2))],
        );
        let id = f.add(entry(to_self, in_block(11, 2)));
        let view = f.view(20);
        let d = decompose(&f.history[&id], &view);
        assert_eq!(d.parts.len(), 1);
        assert_eq!(d.parts[0].tx_type, TxType::SendToSelf);
        assert_eq!(d.parts[0].amount, -10_000, "net is the fee");

        // Create denominations.
        let denoms = tx(
            vec![input(fund, 0)],
            vec![
                out(1_000_010_000, &hash_addr(2)),
                out(999_980_000, &hash_addr(9)),
            ],
        );
        let id = f.add(entry(denoms, in_block(12, 3)));
        assert_eq!(
            decompose(&f.history[&id], &f.view(20)).parts[0].tx_type,
            TxType::CoinJoinCreateDenominations
        );

        // Mixing: our denominated input to our denominated output, with
        // foreign inputs and outputs.
        let mix = tx(
            vec![input(fund, 1), foreign_input(8)],
            vec![
                out(1_000_010, &hash_addr(1)),
                out(1_000_010, &hash_addr(70)),
            ],
        );
        let id = f.add(entry(mix, in_block(13, 4)));
        let d = decompose(&f.history[&id], &f.view(20));
        assert_eq!(d.parts[0].tx_type, TxType::CoinJoinMixing);
        assert_eq!(d.parts[0].amount, 0);
        assert_eq!(d.parts[0].tx_type.category(), TxCategory::CoinJoin);

        // Mixed debit: some inputs ours, payout to others, not denominated.
        let mixed = tx(
            vec![input(fund, 0), foreign_input(9)],
            vec![out(2_500_000_000, &hash_addr(80))],
        );
        let id = f.add(entry(mixed, TransactionContext::Mempool));
        let d = decompose(&f.history[&id], &f.view(20));
        assert_eq!(d.parts[0].tx_type, TxType::Other);
        assert_eq!(d.parts[0].amount, -2_000_000_000);
        assert_eq!(d.fee, None);
    }

    #[test]
    fn coinbase_matures_after_100_blocks_and_data_outputs() {
        let mut f = Fixture::new();
        let mut cb_in = foreign_input(0);
        cb_in.previous_output = OutPoint::null();
        let cb = tx(vec![cb_in], vec![out(50_000_000_000, &hash_addr(1))]);
        assert!(cb.is_coin_base());
        let id = f.add(entry(cb, in_block(1, 1)));
        let (records, _, status) = records_of(&id, &f.history[&id], &f.view(1), &Labels::default());
        assert_eq!(records[0].tx_type, TxType::Generated);
        assert_eq!(records[0].category, TxCategory::Reward);
        assert_eq!(status.kind, TxStatusKind::Immature);
        assert_eq!(status.matures_in, Some(100));
        assert!(!records[0].counts_toward_balance, "brackets while immature");
        let (_, _, status) = records_of(&id, &f.history[&id], &f.view(101), &Labels::default());
        assert_eq!(status.kind, TxStatusKind::Confirmed);
        assert_eq!(status.matures_in, None);

        // An OP_RETURN payout in a send is a data record.
        let data = ScriptBuf::new_op_return(&[0xde, 0xad]);
        let spend = tx(
            vec![input(id, 0)],
            vec![
                TxOut {
                    value: 0,
                    script_pubkey: data.clone(),
                },
                out(49_999_990_000, &hash_addr(9)),
            ],
        );
        let id2 = f.add(entry(spend, in_block(120, 2)));
        let d = decompose(&f.history[&id2], &f.view(130));
        assert_eq!(d.parts.len(), 1);
        assert_eq!(d.parts[0].tx_type, TxType::DataTransaction);
        assert_eq!(d.parts[0].amount, -10_000, "fee only");
        assert_eq!(op_return_payload(&data).as_deref(), Some("dead"));
    }

    /// rates-engine §6: the stamp a transaction was first seen with rides
    /// on each of its records; one with no stamp has none.
    #[test]
    fn the_price_stamp_rides_on_every_record() {
        let mut f = Fixture::new();
        let stamped = f.add(entry(
            tx(
                vec![foreign_input(7)],
                vec![out(1_000, &hash_addr(1)), out(2_000, &hash_addr(2))],
            ),
            TransactionContext::Mempool,
        ));
        let plain = f.add(entry(
            tx(vec![foreign_input(8)], vec![out(3_000, &hash_addr(1))]),
            TransactionContext::Mempool,
        ));
        let stamp = crate::rates::TxFiat {
            currency: "EUR".into(),
            price: "44.12".into(),
            rate_at: 1_793_835_763,
            source: crate::rates::RateSourceId::Ctx,
        };
        let labels = Labels {
            fiat: HashMap::from([(stamped.to_string(), stamp.clone())]),
            ..Labels::default()
        };
        let (records, ..) = records_of(&stamped, &f.history[&stamped], &f.view(5), &labels);
        assert!(!records.is_empty());
        assert!(records.iter().all(|r| r.fiat_then.as_ref() == Some(&stamp)));
        let (records, ..) = records_of(&plain, &f.history[&plain], &f.view(5), &labels);
        assert!(records.iter().all(|r| r.fiat_then.is_none()));
    }

    #[test]
    fn chainlock_confirms_at_once_and_instantsend_counts() {
        let mut f = Fixture::new();
        let id = f.add(entry(
            tx(vec![foreign_input(7)], vec![out(1_000, &hash_addr(1))]),
            TransactionContext::InChainLockedBlock(BlockInfo::new(5, BlockHash::all_zeros(), 9)),
        ));
        let (_, _, status) = records_of(&id, &f.history[&id], &f.view(5), &Labels::default());
        assert_eq!(status.kind, TxStatusKind::Confirmed);
        assert!(status.chain_locked);

        let mut e = entry(
            tx(vec![foreign_input(8)], vec![out(2_000, &hash_addr(2))]),
            TransactionContext::Mempool,
        );
        let id = e.tx.txid();
        let view_tip = 5;
        let (records, _, status) = {
            f.history.insert(id, e.clone());
            records_of(&id, &f.history[&id], &f.view(view_tip), &Labels::default())
        };
        assert_eq!(status.kind, TxStatusKind::Unconfirmed);
        assert!(
            !records[0].counts_toward_balance,
            "foreign unconfirmed is bracketed"
        );
        e.instant_locked = true;
        f.history.insert(id, e);
        let (records, _, status) =
            records_of(&id, &f.history[&id], &f.view(view_tip), &Labels::default());
        assert!(status.instant_locked);
        assert!(records[0].counts_toward_balance);
    }

    #[test]
    fn context_merge_never_downgrades_final_states() {
        let cl =
            TransactionContext::InChainLockedBlock(BlockInfo::new(5, BlockHash::all_zeros(), 9));
        assert_eq!(merge_context(&cl, &TransactionContext::Mempool), cl);
        assert_eq!(merge_context(&cl, &in_block(5, 9)), cl);
        let b = in_block(6, 1);
        assert_eq!(
            merge_context(&b, &TransactionContext::Mempool),
            TransactionContext::Mempool
        );
        assert_eq!(merge_context(&TransactionContext::Mempool, &b), b);
    }

    #[test]
    fn truncation_drops_removed_and_unconfirms_locked() {
        let mut h = WalletHistory::default();
        let mut add = |n: u8, context: TransactionContext| {
            let e = entry(
                tx(vec![foreign_input(n)], vec![out(1_000, &hash_addr(1))]),
                context,
            );
            let id = e.tx.txid();
            h.txs.insert(id, e);
            id
        };
        let kept = add(1, in_block(5, 1));
        let removed = add(2, in_block(7, 2));
        let locked = add(3, in_block(8, 3));
        let unknown = Txid::from_byte_array([9; 32]);

        let changed = h.truncate(&[removed, unknown], &[locked, unknown]);

        assert_eq!(changed, [removed, locked]);
        assert!(!h.txs.contains_key(&removed));
        assert_eq!(h.txs[&kept].context, in_block(5, 1));
        assert_eq!(h.txs[&locked].context, TransactionContext::Mempool);
        assert!(h.txs[&locked].instant_locked);
        // Mined again on the new chain.
        let again = merge_context(&h.txs[&locked].context, &in_block(9, 4));
        assert_eq!(again, in_block(9, 4));
    }

    fn rec(txid: u8, idx: u32, ts: Option<u64>, amount: i64, tx_type: TxType) -> TxRecord {
        TxRecord {
            txid: hex::encode([txid; 32]),
            record_index: idx,
            tx_type,
            category: tx_type.category(),
            status: TxStatus {
                kind: TxStatusKind::Confirmed,
                confirmations: 10,
                instant_locked: false,
                chain_locked: false,
                matures_in: None,
            },
            timestamp: ts,
            block_height: Some(1),
            amount,
            fee: None,
            address: Some(format!("addr{txid}")),
            label: (txid == 3).then(|| "Rent".to_string()),
            counts_toward_balance: true,
            involves_watch_only: false,
            counterparty: None,
            fiat_then: None,
        }
    }

    fn query(limit: u32) -> HistoryQuery {
        HistoryQuery {
            filter: HistoryFilter::default(),
            sort: HistorySort::NewestFirst,
            cursor: None,
            limit,
        }
    }

    #[test]
    fn pages_follow_keyset_cursors_and_survive_inserts() {
        let records: Vec<_> = (1..=7u8)
            .map(|n| {
                rec(
                    n,
                    0,
                    Some(u64::from(n) * 100),
                    i64::from(n) * 10,
                    TxType::RecvWithAddress,
                )
            })
            .collect();
        let first = page(records.clone(), &query(3)).unwrap();
        assert_eq!(first.total_matching, Some(7));
        assert_eq!(
            first
                .records
                .iter()
                .map(|r| r.timestamp.unwrap())
                .collect::<Vec<_>>(),
            vec![700, 600, 500]
        );
        let mut q = query(3);
        q.cursor = first.next_cursor.clone();
        // A newer transaction arrives between pages: the next page continues
        // after the last record seen.
        let mut grown = records.clone();
        grown.push(rec(9, 0, Some(900), 5, TxType::RecvWithAddress));
        let second = page(grown.clone(), &q).unwrap();
        assert_eq!(
            second
                .records
                .iter()
                .map(|r| r.timestamp.unwrap())
                .collect::<Vec<_>>(),
            vec![400, 300, 200]
        );
        q.cursor = second.next_cursor.clone();
        let third = page(grown, &q).unwrap();
        assert_eq!(third.records.len(), 1);
        assert_eq!(third.next_cursor, None);

        // A cursor from another query is stale.
        let mut other = query(3);
        other.sort = HistorySort::AmountAscending;
        other.cursor = first.next_cursor;
        assert!(matches!(
            page(records.clone(), &other),
            Err(EngineError::StaleCursor)
        ));
        other.cursor = Some("garbage".into());
        assert!(matches!(
            page(records, &other),
            Err(EngineError::StaleCursor)
        ));
    }

    #[test]
    fn filters_narrow_the_result() {
        let records = vec![
            rec(1, 0, Some(100), 50, TxType::RecvWithAddress),
            rec(2, 0, Some(200), -70, TxType::SendToAddress),
            rec(3, 0, Some(300), 20, TxType::RecvWithAddress),
            rec(4, 0, None, -5, TxType::CoinJoinMixing),
        ];
        let run = |f: HistoryFilter| {
            let mut q = query(10);
            q.filter = f;
            page(records.clone(), &q)
                .unwrap()
                .records
                .into_iter()
                .map(|r| r.txid[..2].to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            run(HistoryFilter {
                types: vec![TxType::RecvWithAddress],
                ..Default::default()
            }),
            vec!["03", "01"]
        );
        assert_eq!(
            run(HistoryFilter {
                categories: vec![TxCategory::CoinJoin],
                ..Default::default()
            }),
            vec!["04"]
        );
        // date_to is exclusive; unknown times never match a date range.
        assert_eq!(
            run(HistoryFilter {
                date_from: Some(100),
                date_to: Some(300),
                ..Default::default()
            }),
            vec!["02", "01"]
        );
        assert_eq!(
            run(HistoryFilter {
                min_amount: Some(50),
                ..Default::default()
            }),
            vec!["02", "01"]
        );
        assert_eq!(
            run(HistoryFilter {
                text: Some("rENT".into()),
                ..Default::default()
            }),
            vec!["03"]
        );
        assert_eq!(
            run(HistoryFilter {
                text: Some("ADDR2".into()),
                ..Default::default()
            }),
            vec!["02"]
        );
        assert!(
            run(HistoryFilter {
                watch_only: WatchOnlyFilter::Yes,
                ..Default::default()
            })
            .is_empty()
        );

        let mut q = query(0);
        assert!(matches!(
            page(records.clone(), &q),
            Err(EngineError::InvalidQuery(_))
        ));
        q.limit = 501;
        assert!(matches!(
            page(records.clone(), &q),
            Err(EngineError::InvalidQuery(_))
        ));
        q.limit = 5;
        q.filter.date_from = Some(10);
        q.filter.date_to = Some(5);
        assert!(matches!(
            page(records, &q),
            Err(EngineError::InvalidQuery(_))
        ));
    }

    #[test]
    fn amount_sort_orders_by_signed_amount() {
        let records = vec![
            rec(1, 0, Some(100), 50, TxType::RecvWithAddress),
            rec(2, 0, Some(200), -70, TxType::SendToAddress),
            rec(3, 0, Some(300), 20, TxType::RecvWithAddress),
        ];
        let mut q = query(10);
        q.sort = HistorySort::AmountDescending;
        let amounts: Vec<_> = page(records, &q)
            .unwrap()
            .records
            .iter()
            .map(|r| r.amount)
            .collect();
        assert_eq!(amounts, vec![50, 20, -70]);
    }
}
