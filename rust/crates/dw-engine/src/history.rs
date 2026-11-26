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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

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
    /// Set on an `Abandoned` transaction this app dropped itself, as one
    /// that can never confirm, rather than one the user abandoned.
    pub dropped: Option<TxDrop>,
}

/// Why this app dropped a transaction as one that can never confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DropCause {
    /// It spends what a transaction that was never valid paid (#739).
    InvalidSource,
    /// It is that never-valid transaction: it spends a coin of the wallet
    /// its owner never signed for.
    InvalidClaim,
    /// It spends what a payment another transaction beat paid (#170 cases
    /// 3 and 4).
    Replaced,
}

/// Where the wallet's own coins a dropped transaction spent are. The
/// variants are in precedence order, lowest first: a row takes the highest
/// state any of its coins needs ([`row_state`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DropState {
    /// Each is back: unspent in the wallet, or spent by a block or by
    /// another transaction the wallet counts.
    Released,
    /// One is not back yet, and a scan that brings it back is running.
    Releasing,
    /// One is not back, and no scan is running that would bring it back:
    /// a rescan from the wallet's birth would.
    RescanNeeded,
    /// One is not back and its funding block is below the wallet's birth:
    /// the birth must be lowered to `height` (the lowest such block) or
    /// below.
    RescanFromHeight { height: u32 },
    /// One came from a transaction that is not in a block yet: it is back
    /// once that confirms.
    AwaitingFunding,
    /// The drop holds some for good: the wallet can't tell whether they
    /// were spent elsewhere.
    Held,
}

/// A transaction this app dropped as one that can never confirm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TxDrop {
    pub cause: DropCause,
    pub state: DropState,
}

/// What the history keeps of a transaction this app dropped: why, and the
/// coins its drop holds for good.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DropInfo {
    pub cause: DropCause,
    pub held: BTreeSet<OutPoint>,
}

impl DropInfo {
    pub(crate) fn new(cause: DropCause) -> Self {
        Self {
            cause,
            held: BTreeSet::new(),
        }
    }
}

/// The resolved drop of each dropped transaction a view shows.
pub(crate) type DropStates = BTreeMap<Txid, TxDrop>;

/// What a view shows of the transactions this app dropped: each one's
/// resolved drop, and the own inputs of those listed from their marks
/// alone (shown, never spent or released by them).
#[derive(Debug, Default)]
pub(crate) struct Drops {
    pub states: DropStates,
    pub inputs: BTreeMap<Txid, crate::tx_actions::OwnInputs>,
}

/// A view that shows no transaction as dropped.
#[cfg(test)]
pub(crate) static NO_DROPS: Drops = Drops {
    states: BTreeMap::new(),
    inputs: BTreeMap::new(),
};

/// What [`drop_states`] reads of key-wallet and the scan.
pub(crate) struct DropFacts<'a> {
    /// A coin key-wallet holds unspent in a funding account.
    pub unspent: &'a dyn Fn(&OutPoint) -> bool,
    /// A coin key-wallet saw a block spend.
    pub block_spent: &'a dyn Fn(&OutPoint) -> bool,
    /// The checkpoint of the wallet's running scan: one below its target
    /// and not cancelled. It brings back a coin funded above it.
    pub scan_at: Option<u32>,
    /// A drop's batch has not finished, or the wallet is opening: the
    /// rewinds they owe are not made yet.
    pub pending: bool,
    /// The wallet's birth height: a rescan from it finds no coin funded
    /// below it.
    pub birth: u32,
}

/// What [`DropFacts`] says of the scan, from the wallet's scan state and
/// whether a drop's batch is in progress (`batch`): the checkpoint of a
/// scan that is running (below its target and not cancelled), and whether
/// rewinds are owed and not made yet (the batch, or the wallet opening).
pub(crate) fn scan_facts(
    state: Option<&crate::events::WalletState>,
    batch: bool,
) -> (Option<u32>, bool) {
    let scan_at = state
        .filter(|s| s.replay_cancelled.is_none() && s.synced_height < s.scan_target)
        .map(|s| s.synced_height);
    (scan_at, batch || state.is_some_and(|s| s.loading))
}

/// The state one coin a dropped transaction waits for needs, when it is
/// not back: its funding transaction's block height (`None` when it is in
/// none).
pub(crate) fn coin_state(funded_at: Option<u32>, facts: &DropFacts<'_>) -> DropState {
    match funded_at {
        None => DropState::AwaitingFunding,
        Some(h) if facts.scan_at.is_some_and(|c| c < h) => DropState::Releasing,
        Some(h) if h < facts.birth => DropState::RescanFromHeight { height: h },
        Some(_) if facts.pending => DropState::Releasing,
        Some(_) => DropState::RescanNeeded,
    }
}

/// A dropped row's one state: `Held` when its drop holds coins for good,
/// else the highest state of those of its coins not back (`Released` when
/// all are), with the lowest height among `RescanFromHeight` ones.
pub(crate) fn row_state(held: bool, coins: impl IntoIterator<Item = DropState>) -> DropState {
    if held {
        return DropState::Held;
    }
    let coins: Vec<DropState> = coins.into_iter().collect();
    let lowest = (coins.iter())
        .filter_map(|c| match c {
            DropState::RescanFromHeight { height } => Some(*height),
            _ => None,
        })
        .min();
    match coins.into_iter().max() {
        Some(DropState::RescanFromHeight { .. }) => DropState::RescanFromHeight {
            height: lowest.unwrap_or_default(),
        },
        Some(state) => state,
        None => DropState::Released,
    }
}

/// The state of each of `drops` whose transaction `txs` holds abandoned.
/// The coins a drop waits for are its inputs that pay an `owned` script of
/// a transaction the history holds, counts (not abandoned) and did not
/// drop, or that settled (a block or an InstantSend lock) since its drop:
/// what an unsettled dropped or never-valid parent paid never comes back. A coin
/// is back when it is unspent, spent by a block, or spent by another
/// transaction of the history that is not abandoned. One pass over the
/// history's inputs only when some coin is not found otherwise.
pub(crate) fn drop_states(
    txs: &HashMap<Txid, TxEntry>,
    drops: &HashMap<Txid, DropInfo>,
    owned: &HashMap<ScriptBuf, Owned>,
    facts: &DropFacts<'_>,
) -> DropStates {
    let waited = |e: &TxEntry| -> Vec<OutPoint> {
        (e.tx.input.iter())
            .map(|i| i.previous_output)
            .filter(|o| {
                (txs.get(&o.txid)).is_some_and(|f| {
                    !(drops.contains_key(&o.txid)
                        && f.context.block_info().is_none()
                        && !instant_locked(f))
                        && !f.is_abandoned()
                        && (f.tx.output.get(o.vout as usize))
                            .is_some_and(|out| owned.contains_key(&out.script_pubkey))
                })
            })
            .collect()
    };
    let mut spent_by_live: Option<HashSet<OutPoint>> = None;
    let mut out = DropStates::new();
    for (t, info) in drops {
        let Some(e) = txs.get(t).filter(|e| e.is_abandoned()) else {
            continue;
        };
        let mut missing: Vec<OutPoint> = (waited(e).into_iter())
            .filter(|o| !(facts.unspent)(o) && !(facts.block_spent)(o))
            .collect();
        if !missing.is_empty() {
            let live = spent_by_live.get_or_insert_with(|| {
                (txs.values())
                    .filter(|e| !e.is_abandoned())
                    .flat_map(|e| e.tx.input.iter().map(|i| i.previous_output))
                    .collect()
            });
            missing.retain(|o| !live.contains(o));
        }
        let coins = missing.iter().map(|o| {
            let funded_at =
                (txs.get(&o.txid)).and_then(|f| f.context.block_info().map(|b| b.height()));
            coin_state(funded_at, facts)
        });
        let state = row_state(!info.held.is_empty(), coins);
        out.insert(
            *t,
            TxDrop {
                cause: info.cause,
                state,
            },
        );
    }
    out
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
    /// What an asset lock of the wallet was for, from the registration
    /// flows it keeps; `None` for every other transaction.
    pub purpose: Option<TxPurpose>,
}

/// What a Dash Platform funding transaction paid for, read from the
/// wallet's registration records (never from the chain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxPurpose {
    /// The lock funded the registration of this username.
    UsernameRegistration { username: String },
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

/// A ChainLock covers the block `context` names: the record says so, or the
/// block is at or below the best ChainLock (key-wallet's own rule).
pub(crate) fn chain_locked(context: &TransactionContext, chainlock_height: Option<u32>) -> bool {
    let height = context.block_info().map(|b| b.height());
    matches!(context, TransactionContext::InChainLockedBlock(_))
        || matches!((height, chainlock_height), (Some(h), Some(cl)) if cl >= h)
}

/// Whether another transaction in a block the SPV header chain proves
/// final ([`proven_final`] under the wallet's own `chainlock`, `active`
/// the chain's block hash at each height it has) spends one of `txid`'s
/// inputs: a ChainLock outranks an InstantSend lock (DIP-10, as
/// key-wallet's conflict sweep orders them), so `txid` can never confirm.
/// A record's ChainLocked context can come from its height alone, and a
/// record of a block a fork replaced comes back from wallet.sqlite after a
/// restart.
pub(crate) fn chainlock_beaten(
    txs: &HashMap<Txid, TxEntry>,
    txid: &Txid,
    chainlock: Option<(u32, dashcore::BlockHash)>,
    active: &HashMap<u32, dashcore::BlockHash>,
) -> bool {
    let Some(e) = txs.get(txid) else {
        return false;
    };
    let inputs: HashSet<OutPoint> = e.tx.input.iter().map(|i| i.previous_output).collect();
    (txs.iter()).any(|(w, we)| {
        w != txid
            && (we.context.block_info())
                .is_some_and(|b| proven_final((b.height(), b.block_hash()), chainlock, active))
            && (we.tx.input.iter()).any(|i| inputs.contains(&i.previous_output))
    })
}

/// Heights of the blocks holding a transaction that spends an input of a
/// transaction out of a block: the headers [`chainlock_beaten`] reads.
pub(crate) fn rival_heights(txs: &HashMap<Txid, TxEntry>) -> BTreeSet<u32> {
    let unmined: HashSet<OutPoint> = (txs.values())
        .filter(|e| e.context.block_info().is_none())
        .flat_map(|e| e.tx.input.iter().map(|i| i.previous_output))
        .collect();
    (txs.values())
        .filter_map(|e| Some((e, e.context.block_info()?.height())))
        .filter(|(e, _)| (e.tx.input.iter()).any(|i| unmined.contains(&i.previous_output)))
        .map(|(_, h)| h)
        .collect()
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
/// is abandoned. An InstantSend-locked one is conflicted only by a block
/// `chainlock` covers (key-wallet's finality order).
pub(crate) fn spent_outpoints(
    txs: &HashMap<Txid, TxEntry>,
    chainlock: Option<u32>,
) -> HashSet<OutPoint> {
    let conflicted = conflicted(txs, chainlock);
    (txs.iter())
        .filter(|(t, e)| {
            e.context.block_info().is_some() || !(e.is_abandoned() || conflicted.contains(*t))
        })
        .flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output))
        .collect()
}

/// The transactions out of a block that can never confirm, by
/// [`spent_outpoints`]' rule (Core's `MarkConflicted`): another
/// transaction in a block spends one of their inputs, or they descend out
/// of a block from one that does. Abandoned ones count, so a restart reads
/// the same set from the abandoned copies the load restores. An
/// InstantSend-locked one is conflicted only by a block `chainlock` covers,
/// and never through its parent.
pub(crate) fn conflicted(txs: &HashMap<Txid, TxEntry>, chainlock: Option<u32>) -> HashSet<Txid> {
    let mined = |e: &TxEntry| e.context.block_info().is_some();
    let mut in_block: HashMap<OutPoint, Vec<(Txid, &TxEntry)>> = HashMap::new();
    for (t, e) in txs.iter().filter(|(_, e)| mined(e)) {
        for i in &e.tx.input {
            in_block.entry(i.previous_output).or_default().push((*t, e));
        }
    }
    let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
    let mut todo = Vec::new();
    for (t, e) in txs.iter().filter(|(_, e)| !mined(e)) {
        for i in &e.tx.input {
            if !instant_locked(e) {
                children.entry(i.previous_output.txid).or_default().push(*t);
            }
            let beats = |(b, be): &(Txid, &TxEntry)| {
                b != t && (!instant_locked(e) || chain_locked(&be.context, chainlock))
            };
            if in_block
                .get(&i.previous_output)
                .is_some_and(|v| v.iter().any(beats))
            {
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
    conflicted
}

/// The wallet's coins that the payments this app sent
/// ([`crate::tx_actions::load_sent`]) spend while they are out of a block
/// and not abandoned, each with its payment: key-wallet's spent-output
/// claims (`restore_spent_outpoints`), which it rebuilds from no stored
/// record (#88). A coin is claimed only when the app signed it for that
/// payment and the payment's transaction spends it. A payment that another
/// transaction beats on any input claims nothing: its coins are the
/// winner's or free. Key-wallet's finality order decides the beat: a block
/// or an InstantSend lock beats a payment without a lock, and only a block
/// `chainlock` covers beats an InstantSend-locked one.
pub(crate) fn unmined_own_spends(
    txs: &HashMap<Txid, TxEntry>,
    sent: &crate::tx_actions::Sent,
    chainlock: Option<u32>,
) -> Vec<(OutPoint, Txid)> {
    if sent.is_empty() {
        return Vec::new();
    }
    let candidates: Vec<(&Txid, &BTreeSet<OutPoint>, &TxEntry)> = (sent.iter())
        .filter_map(|(t, mark)| Some((t, &mark.coins, txs.get(t)?)))
        .filter(|(_, _, e)| e.context.block_info().is_none() && !e.is_abandoned())
        .collect();
    let mut spenders: HashMap<OutPoint, Vec<(&Txid, &TxEntry)>> = HashMap::new();
    for (w, we) in txs {
        for i in &we.tx.input {
            spenders.entry(i.previous_output).or_default().push((w, we));
        }
    }
    let beaten = |t: &Txid, e: &TxEntry| {
        (e.tx.input.iter()).any(|i| {
            (spenders.get(&i.previous_output))
                .is_some_and(|v| v.iter().any(|(w, we)| *w != t && beats(we, e, chainlock)))
        })
    };
    // A beaten payment's descendants out of a block can never confirm either
    // (#170 case 4), as `spent_outpoints` treats them; an InstantSend-locked
    // one stays live, since only a ChainLocked block can beat it. A locked
    // payment a ChainLock beat keeps its children's claims: Abandon owns
    // it (#170 case 14).
    let unmined = |e: &TxEntry| e.context.block_info().is_none() && !e.is_abandoned();
    let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
    for (t, e) in txs.iter().filter(|(_, e)| unmined(e) && !instant_locked(e)) {
        for i in &e.tx.input {
            children.entry(i.previous_output.txid).or_default().push(*t);
        }
    }
    let mut todo: Vec<Txid> = (txs.iter())
        .filter(|(t, e)| unmined(e) && beaten(t, e))
        .map(|(t, _)| *t)
        .collect();
    let mut dead = HashSet::new();
    while let Some(t) = todo.pop() {
        if dead.insert(t) && !txs.get(&t).is_some_and(instant_locked) {
            todo.extend(children.get(&t).into_iter().flatten());
        }
    }
    (candidates.into_iter())
        .filter(|(t, _, _)| !dead.contains(*t))
        .flat_map(|(t, coins, e)| {
            (e.tx.input.iter())
                .map(|i| i.previous_output)
                .filter(|o| coins.contains(o))
                .map(move |o| (o, *t))
        })
        .chain(
            // A payment whose stored copy does not read back keeps its coins
            // out, unlisted, until a record of it settles it.
            (sent.iter())
                .filter(|(t, m)| m.kept && m.tx.is_none() && !txs.contains_key(*t))
                .flat_map(|(t, m)| m.coins.iter().map(move |o| (*o, *t))),
        )
        .collect()
}

/// Key-wallet's finality order: a block or an InstantSend lock beats a
/// payment without a lock, and only a block `chainlock` covers beats an
/// InstantSend-locked one.
fn beats(winner: &TxEntry, payment: &TxEntry, chainlock: Option<u32>) -> bool {
    match instant_locked(payment) {
        true => chain_locked(&winner.context, chainlock),
        false => winner.context.block_info().is_some() || instant_locked(winner),
    }
}

/// What beat a transaction out of a block, as far as the wallet knows: the
/// winner's txid, and the height of the block that spent one of the
/// loser's coins when the wallet processed it. Its inputs are known when
/// the history holds it, or when it is mined: key-wallet observes every
/// spend of a block it processes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Winner {
    pub txid: Option<Txid>,
    pub mined: Option<u32>,
}

/// The payments out of a block and not abandoned that key-wallet holds no
/// record of (`recorded`), so its conflict handling cannot reach them
/// (#170 case 3), beaten on an input by [`beats`]'s order: by another
/// transaction of the history that is mined or that key-wallet holds (a
/// restored lock may itself have lost to a ChainLock, #170 case 14), or by
/// a block key-wallet processed
/// (`observed`). Each with what beat it; an InstantSend-locked one only
/// under a ChainLock, which the drop leaves to Abandon all the same (#170
/// case 14). Only those paying the wallet (`pays_wallet`): key-wallet keeps
/// no record of a payment without change mined after a restart, and the
/// block `observed` names is then its own.
pub(crate) fn beaten_unrecorded(
    txs: &HashMap<Txid, TxEntry>,
    recorded: impl Fn(&Txid) -> bool,
    observed: &BTreeMap<OutPoint, u32>,
    chainlock: Option<u32>,
    mut pays_wallet: impl FnMut(&Transaction) -> bool,
) -> Vec<(Txid, Winner)> {
    let candidates: Vec<(&Txid, &TxEntry)> = (txs.iter())
        .filter(|(t, e)| e.context.block_info().is_none() && !e.is_abandoned() && !recorded(t))
        .collect();
    if candidates.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<OutPoint> = (candidates.iter())
        .flat_map(|(_, e)| e.tx.input.iter().map(|i| i.previous_output))
        .collect();
    let mut spenders: HashMap<OutPoint, Vec<(&Txid, &TxEntry)>> = HashMap::new();
    for (w, we) in txs {
        for i in we
            .tx
            .input
            .iter()
            .filter(|i| wanted.contains(&i.previous_output))
        {
            spenders.entry(i.previous_output).or_default().push((w, we));
        }
    }
    let mut beaten = Vec::new();
    for (t, e) in candidates {
        let in_history = (e.tx.input.iter()).find_map(|i| {
            let (w, we) = (spenders.get(&i.previous_output)?.iter()).find(|(w, we)| {
                *w != t
                    && (we.context.block_info().is_some() || recorded(w))
                    && beats(we, e, chainlock)
            })?;
            Some(Winner {
                txid: Some(**w),
                mined: we.context.block_info().map(|b| b.height()),
            })
        });
        let in_block = || {
            (e.tx.input.iter()).find_map(|i| {
                let h = *observed.get(&i.previous_output)?;
                (!instant_locked(e) || chainlock.is_some_and(|cl| cl >= h)).then_some(Winner {
                    txid: None,
                    mined: Some(h),
                })
            })
        };
        if let Some(w) = in_history.or_else(in_block)
            && pays_wallet(&e.tx)
        {
            beaten.push((*t, w));
        }
    }
    beaten
}

/// The transactions of the history out of a block, neither abandoned nor
/// InstantSend-locked, that descend from `parents` by any output: they can
/// never confirm once `parents` cannot.
pub(crate) fn unmined_children(txs: &HashMap<Txid, TxEntry>, parents: &[Txid]) -> BTreeSet<Txid> {
    let open: Vec<(&Txid, &TxEntry)> = (txs.iter())
        .filter(|(_, e)| {
            e.context.block_info().is_none() && !e.is_abandoned() && !instant_locked(e)
        })
        .collect();
    let mut dead: HashSet<Txid> = parents.iter().copied().collect();
    let mut todo = parents.to_vec();
    let mut children = BTreeSet::new();
    while let Some(p) = todo.pop() {
        for (t, e) in &open {
            if !dead.contains(*t) && e.tx.input.iter().any(|i| i.previous_output.txid == p) {
                dead.insert(**t);
                children.insert(**t);
                todo.push(**t);
            }
        }
    }
    children
}

/// The stored transactions (`stored`) out of a block, neither abandoned nor
/// InstantSend-locked, that spend a coin of the wallet whose transaction
/// the store holds no row of and the history no live entry of (#170 case
/// 4): its parent was swept or dropped, so they can never confirm. The
/// store keeps every row once final, so a missing one is not a pruned
/// funding. One that spends a transaction the store dropped as an unsigned
/// claim, or one key-wallet swept as a claim (`orphan_exempt`), is the claim
/// rule's to judge (#739), and is left.
pub(crate) fn orphaned_unmined(
    txs: &HashMap<Txid, TxEntry>,
    stored: &HashSet<Txid>,
    orphan_exempt: &HashSet<Txid>,
) -> Vec<Txid> {
    let gone = |p: &Txid| !stored.contains(p) && !txs.get(p).is_some_and(|e| !e.is_abandoned());
    let exempt =
        |e: &TxEntry| (e.tx.input.iter()).any(|i| orphan_exempt.contains(&i.previous_output.txid));
    (txs.iter())
        .filter(|(t, e)| {
            stored.contains(*t)
                && e.context.block_info().is_none()
                && !e.is_abandoned()
                && !instant_locked(e)
                && !exempt(e)
                && (e.own_inputs.keys())
                    .filter_map(|i| e.tx.input.get(*i as usize))
                    .any(|i| gone(&i.previous_output.txid))
        })
        .map(|(t, _)| *t)
        .collect()
}

/// The parents of the `orphans` ([`orphaned_unmined`]) that are gone: no
/// stored row and no live entry, the evidence that they never confirm.
pub(crate) fn gone_parents(
    txs: &HashMap<Txid, TxEntry>,
    stored: &HashSet<Txid>,
    orphans: &[Txid],
) -> BTreeSet<Txid> {
    let gone = |p: &Txid| !stored.contains(p) && !txs.get(p).is_some_and(|e| !e.is_abandoned());
    (orphans.iter().filter_map(|t| txs.get(t)))
        .flat_map(|e| (e.own_inputs.keys()).filter_map(|i| e.tx.input.get(*i as usize)))
        .map(|i| i.previous_output.txid)
        .filter(gone)
        .collect()
}

/// What the coins the drops still hold (`held`, [`still_held`]) add up to,
/// as the wallet's balances report them (#749): the sum of those whose
/// value it knows, and how many are held whose value it does not. A
/// coin's value comes from the output that made it when the history holds
/// that transaction and the wallet owns the output (`owns`), else from a
/// held payment's record of its own inputs. A coin known neither way is
/// counted as of unknown value, never guessed, and only when an input
/// spending it in the history is signed with a key of the wallet
/// ([`p2pkh_signer`]): a beaten incoming payment, or the other inputs of a
/// beaten mix, hold coins that are not the wallet's.
pub(crate) fn held_total(
    held: &BTreeSet<OutPoint>,
    txs: &HashMap<Txid, TxEntry>,
    owns: impl Fn(&ScriptBuf) -> bool,
) -> (u64, u32) {
    let mut own_inputs: HashMap<OutPoint, u64> = HashMap::new();
    let mut signed_by_own = HashSet::new();
    for e in txs.values() {
        for i in &e.tx.input {
            if held.contains(&i.previous_output)
                && p2pkh_signer(&i.script_sig).is_some_and(|s| owns(&s))
            {
                signed_by_own.insert(i.previous_output);
            }
        }
        for (index, (value, _)) in &e.own_inputs {
            if let Some(i) =
                (e.tx.input.get(*index as usize)).filter(|i| held.contains(&i.previous_output))
            {
                own_inputs.insert(i.previous_output, *value);
            }
        }
    }
    let (mut sum, mut unvalued) = (0u64, 0u32);
    for o in held {
        let output = (txs.get(&o.txid)).and_then(|e| e.tx.output.get(o.vout as usize));
        match (output, own_inputs.get(o)) {
            (Some(out), _) if owns(&out.script_pubkey) => sum = sum.saturating_add(out.value),
            (_, Some(value)) => sum = sum.saturating_add(*value),
            (Some(_), None) => {}
            (None, None) if signed_by_own.contains(o) => unvalued += 1,
            (None, None) => {}
        }
    }
    (sum, unvalued)
}

/// Of the coins the drops hold (`holds`), those still held: none a
/// processed block (`observed`) or a mined transaction of the history
/// spent, or that `left_out` names (a final spend's note, or an output of
/// a parent a load found gone).
pub(crate) fn still_held(
    holds: &BTreeSet<OutPoint>,
    txs: &HashMap<Txid, TxEntry>,
    observed: &BTreeMap<OutPoint, u32>,
    left_out: impl Fn(&OutPoint) -> bool,
) -> BTreeSet<OutPoint> {
    let mined: HashSet<OutPoint> = (txs.values())
        .filter(|e| e.context.block_info().is_some())
        .flat_map(|e| e.tx.input.iter().map(|i| i.previous_output))
        .filter(|o| holds.contains(o))
        .collect();
    (holds.iter())
        .filter(|o| !observed.contains_key(*o) && !mined.contains(*o) && !left_out(o))
        .copied()
        .collect()
}

/// What a drop noted of what beat its transactions (`drop_winner:`), to
/// judge when the coins it holds are settled ([`held_awaiting`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DropOrigin {
    /// The transaction that beat the drop's roots, when known.
    pub winner: Option<Txid>,
    /// The coins the winner took from them: the inputs of the drop's
    /// roots, and of the payments a sweep took for it that the history
    /// held.
    pub contested: BTreeSet<OutPoint>,
    /// The parents a load found gone (#170 case 4): an orphan's drop, which
    /// nothing beat. Their outputs never exist.
    pub gone: BTreeSet<Txid>,
}

impl DropOrigin {
    /// `winner|contested|gone`, each list comma-separated.
    pub(crate) fn encode(&self) -> String {
        let join = |i: Vec<String>| i.join(",");
        format!(
            "{}|{}|{}",
            self.winner.map(|w| w.to_string()).unwrap_or_default(),
            join(self.contested.iter().map(|o| o.to_string()).collect()),
            join(self.gone.iter().map(|t| t.to_string()).collect()),
        )
    }

    /// `None` for a value [`Self::encode`] did not write.
    pub(crate) fn decode(value: &str) -> Option<Self> {
        use std::str::FromStr;
        let mut parts = value.split('|');
        let (Some(winner), Some(contested), Some(gone), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        fn items(list: &str) -> impl Iterator<Item = &str> {
            list.split(',').filter(|i| !i.is_empty())
        }
        Some(Self {
            winner: match winner {
                "" => None,
                w => Some(Txid::from_str(w).ok()?),
            },
            contested: (items(contested).map(OutPoint::from_str))
                .collect::<Result<_, _>>()
                .ok()?,
            gone: (items(gone).map(Txid::from_str))
                .collect::<Result<_, _>>()
                .ok()?,
        })
    }
}

/// By dropped transaction, the coins its `drop_holds:` note holds and its
/// origin, when its `drop_winner:` note reads.
pub(crate) type HeldDrops = BTreeMap<Txid, (BTreeSet<OutPoint>, Option<DropOrigin>)>;

/// The origin of a drop with no `drop_winner:` note (an older drop, #749
/// Q81c C2): its winner unknown, and as what it
/// contested the inputs of its roots, read from the bytes (`copy_of`) of
/// its transaction `t` and of each dead transaction (`dead`: dropped or
/// swept) it descends from. An input made by a dead transaction is never
/// spent on chain, and is left out. With no bytes, it contests nothing, so
/// it awaits.
pub(crate) fn older_origin<'a>(
    t: Txid,
    copy_of: impl Fn(&Txid) -> Option<&'a Transaction>,
    dead: impl Fn(&Txid) -> bool,
) -> DropOrigin {
    let mut origin = DropOrigin::default();
    let (mut todo, mut seen) = (vec![t], HashSet::from([t]));
    while let Some(t) = todo.pop() {
        for i in copy_of(&t).into_iter().flat_map(|tx| &tx.input) {
            let parent = i.previous_output.txid;
            match dead(&parent) {
                true if seen.insert(parent) => todo.push(parent),
                true => {}
                false => {
                    origin.contested.insert(i.previous_output);
                }
            }
        }
    }
    origin
}

/// A block's spend of a coin the drops hold or contest (`drop_spent:`),
/// noted while key-wallet still observed it: it counts once the SPV header
/// chain proves the block final ([`proven_final`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpendNote {
    pub height: u32,
    pub block: dashcore::BlockHash,
    /// The spending transaction, when the history holds it.
    pub spender: Option<Txid>,
}

impl SpendNote {
    /// `height,block,spender`, the spender empty when unknown.
    pub(crate) fn encode(&self) -> String {
        let spender = self.spender.map(|t| t.to_string()).unwrap_or_default();
        format!("{},{},{spender}", self.height, self.block)
    }

    /// `None` for a value [`Self::encode`] did not write.
    pub(crate) fn decode(value: &str) -> Option<Self> {
        use std::str::FromStr;
        let mut parts = value.split(',');
        let (Some(height), Some(block), Some(spender), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Self {
            height: height.parse().ok()?,
            block: dashcore::BlockHash::from_str(block).ok()?,
            spender: match spender {
                "" => None,
                t => Some(Txid::from_str(t).ok()?),
            },
        })
    }
}

/// How a sweep removed a transaction (`swept_by:`, #749 Q81d S1-S5): with
/// the transaction's bytes, read before the history forgot them, its
/// winner, and the winner's block once a block names it. Only a conflict
/// sweep's final winner proves the transaction dead ([`dead_for_good`]):
/// in a claim sweep the winner's finality proves nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SweptBy {
    pub claim: bool,
    pub winner: Txid,
    /// The winner's block: from the sweep's event or a block record of the
    /// winner, never from an InstantSend lock.
    pub block: Option<(u32, dashcore::BlockHash)>,
    pub tx: Transaction,
}

impl SweptBy {
    /// `kind|winner|height,block|hex`, the block empty when unknown.
    pub(crate) fn encode(&self) -> String {
        let block = (self.block)
            .map(|(h, b)| format!("{h},{b}"))
            .unwrap_or_default();
        format!(
            "{}|{}|{block}|{}",
            if self.claim { "claim" } else { "conflict" },
            self.winner,
            dashcore::consensus::encode::serialize_hex(&self.tx),
        )
    }

    /// `None` for a value [`Self::encode`] did not write, or whose
    /// transaction is not `txid`: such a note counts as absent.
    pub(crate) fn decode(txid: &Txid, value: &str) -> Option<Self> {
        use std::str::FromStr;
        let mut parts = value.split('|');
        let (Some(kind), Some(winner), Some(block), Some(hex), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return None;
        };
        let claim = match kind {
            "claim" => true,
            "conflict" => false,
            _ => return None,
        };
        let block = match block.split_once(',') {
            None if block.is_empty() => None,
            None => return None,
            Some((h, b)) => Some((h.parse().ok()?, dashcore::BlockHash::from_str(b).ok()?)),
        };
        let bytes = hex::decode(hex).ok()?;
        let tx: Transaction = dashcore::consensus::encode::deserialize(&bytes).ok()?;
        (tx.txid() == *txid).then_some(Self {
            claim,
            winner: Txid::from_str(winner).ok()?,
            block,
            tx,
        })
    }
}

/// The block a conflict sweep's note (`note`) keeps for its winner (Q81d
/// S4, S5): none once the header chain (`headers`) names another block at
/// its height or a fork truncated at or below it (`fork`); filled from a
/// block record of the winner (`mined`) the header chain doesn't
/// contradict. A claim sweep's note keeps none.
pub(crate) fn swept_by_block(
    note: &SweptBy,
    mined: Option<(u32, dashcore::BlockHash)>,
    headers: &HashMap<u32, dashcore::BlockHash>,
    fork: Option<u32>,
) -> Option<(u32, dashcore::BlockHash)> {
    let stands = |(h, b): &(u32, dashcore::BlockHash)| {
        headers.get(h).is_none_or(|x| x == b) && fork.is_none_or(|f| f > *h)
    };
    match note.claim {
        true => None,
        false => (note.block.filter(stands)).or(mined.filter(stands)),
    }
}

/// What can prove a gone parent of an orphan's drop dead for good
/// ([`dead_for_good`]).
#[derive(Debug, Default)]
pub(crate) struct Deaths {
    /// The transactions whose `dropped:` mark is a never-valid verdict, as
    /// the strict reader reads it at this evaluation.
    pub never_valid: BTreeSet<Txid>,
    /// Each swept transaction's `swept_by:` note that reads.
    pub swept_by: BTreeMap<Txid, SweptBy>,
    /// Each abandoned transaction's copy that reads back.
    pub copies: BTreeMap<Txid, Transaction>,
}

impl Deaths {
    /// The inputs of `t`, from its abandoned copy or its sweep's note.
    pub(crate) fn inputs_of(&self, t: &Txid) -> impl Iterator<Item = OutPoint> + '_ {
        (self.copies.get(t).or(self.swept_by.get(t).map(|s| &s.tx)))
            .into_iter()
            .flat_map(|tx| tx.input.iter().map(|i| i.previous_output))
    }
}

/// Whether `parent`, a gone parent of an orphan's drop, is dead for good
/// (#749 Q81c §2, Q81d): it is not live in the history, and (a) its
/// `dropped:` mark is a never-valid verdict, (b) a conflict sweep removed
/// it for a winner whose block the header chain proves final (`proven`),
/// or (c) another transaction spent one of its inputs
/// ([`Deaths::inputs_of`]) in a block the header chain proves final: a
/// spend note naming that spender (`spent`), or a transaction of the
/// history. A claim sweep's parent dies only by (a) (S3). Its abandoned
/// mark alone proves nothing, nor does a note with no spender: a mined
/// parent spends its own inputs.
pub(crate) fn dead_for_good(
    parent: &Txid,
    deaths: &Deaths,
    txs: &HashMap<Txid, TxEntry>,
    spent: &BTreeMap<OutPoint, SpendNote>,
    proven: impl Fn((u32, dashcore::BlockHash)) -> bool,
) -> bool {
    if txs.get(parent).is_some_and(|e| !e.is_abandoned()) {
        return false;
    }
    if deaths.never_valid.contains(parent) {
        return true;
    }
    let swept = deaths.swept_by.get(parent);
    if swept.is_some_and(|s| s.claim) {
        return false;
    }
    let spent_finally = |o: OutPoint| {
        (spent.get(&o))
            .is_some_and(|n| n.spender.is_some_and(|s| s != *parent) && proven((n.height, n.block)))
            || (txs.iter()).any(|(t, e)| {
                t != parent
                    && !e.is_abandoned()
                    && (e.context.block_info())
                        .is_some_and(|b| proven((b.height(), b.block_hash())))
                    && e.tx.input.iter().any(|i| i.previous_output == o)
            })
    };
    swept.is_some_and(|s| s.block.is_some_and(&proven))
        || deaths.inputs_of(parent).any(spent_finally)
}

/// What [`held_awaiting`] reads beside the drops and the history.
pub(crate) struct HeldEvidence<'a> {
    /// The header chain proves a transaction's block final
    /// ([`proven_final`]): a stored context or a height alone never does,
    /// since a record of a block a fork replaced comes back after a
    /// restart.
    pub chain_final: &'a dyn Fn(&TxEntry) -> bool,
    /// A coin has a final spend: a processed block's at or below the
    /// ChainLock, or a proven note.
    pub final_spend: &'a dyn Fn(&OutPoint) -> bool,
    /// A key-wallet record spends the coin.
    pub spent_by_record: &'a dyn Fn(&OutPoint) -> bool,
    /// An InstantSend lock of a transaction the wallet holds no copy of may
    /// spend the coin: one heard this session, or noted
    /// (`drop_locked:`, Q81c C1).
    pub unknown_lock: &'a dyn Fn(&OutPoint) -> bool,
    /// A gone parent of an orphan's drop is dead for good
    /// ([`dead_for_good`]).
    pub dead_for_good: &'a dyn Fn(&Txid) -> bool,
}

/// Whether a coin the drops still hold (`held`, [`still_held`]) awaits
/// what spends it (#749, Q81b S2): the way out is offered only when none
/// does. Judged per coin, from each drop that holds it (`drops`: what each
/// dropped transaction's note holds, and its origin's note): a coin is
/// settled when no live transaction of the history or key-wallet record
/// spends it, no InstantSend lock of a transaction the wallet holds no
/// copy of may spend it, and every drop holding it is settled. A drop is
/// settled when its winner is in the history and its block proven final
/// (`chain_final`); when the history doesn't hold it, or it is unknown,
/// when one of the coins it contested has a final spend: the evidence's,
/// or a spend by a transaction of the history in a block proven final. An
/// orphan's drop, which nothing beat, is settled once each parent a load
/// found gone is dead for good (Q81c §2). A drop with no origin note
/// awaits.
pub(crate) fn held_awaiting(
    held: &BTreeSet<OutPoint>,
    drops: &HeldDrops,
    txs: &HashMap<Txid, TxEntry>,
    evidence: &HeldEvidence<'_>,
) -> bool {
    let contested: HashSet<OutPoint> = (drops.values())
        .filter_map(|(_, o)| o.as_ref())
        .flat_map(|o| o.contested.iter().copied())
        .collect();
    let (mut live, mut locked) = (HashSet::new(), HashSet::new());
    for e in txs.values().filter(|e| !e.is_abandoned()) {
        for o in e.tx.input.iter().map(|i| i.previous_output) {
            if held.contains(&o) {
                live.insert(o);
            }
            if contested.contains(&o) && (evidence.chain_final)(e) {
                locked.insert(o);
            }
        }
    }
    let settled_drop = |origin: &DropOrigin| match origin {
        DropOrigin {
            winner: None, gone, ..
        } if !gone.is_empty() => gone.iter().all(evidence.dead_for_good),
        _ => match (origin.winner.and_then(|w| txs.get(&w))).filter(|e| !e.is_abandoned()) {
            Some(winner) => (evidence.chain_final)(winner),
            None => {
                (origin.contested.iter()).any(|o| locked.contains(o) || (evidence.final_spend)(o))
            }
        },
    };
    held.iter().any(|coin| {
        live.contains(coin)
            || (evidence.spent_by_record)(coin)
            || (evidence.unknown_lock)(coin)
            || (drops.values())
                .filter(|(holds, _)| holds.contains(coin))
                .any(|(_, origin)| !origin.as_ref().is_some_and(settled_drop))
    })
}

/// The P2PKH script of the key a `[signature, key]` input script pushes.
fn p2pkh_signer(script: &dashcore::Script) -> Option<ScriptBuf> {
    use dashcore::blockdata::script::Instruction;
    let mut pushes = script.instructions();
    let (Some(Ok(Instruction::PushBytes(_))), Some(Ok(Instruction::PushBytes(key))), None) =
        (pushes.next(), pushes.next(), pushes.next())
    else {
        return None;
    };
    let key = dashcore::PublicKey::from_slice(key.as_bytes()).ok()?;
    Some(ScriptBuf::new_p2pkh(&key.pubkey_hash()))
}

/// Whether the history knows an InstantSend lock of `e`: its flag or its
/// context.
pub(crate) fn instant_locked(e: &TxEntry) -> bool {
    e.instant_locked || matches!(e.context, TransactionContext::InstantSend(_))
}

/// The payments of `claims` the history holds InstantSend-locked.
pub(crate) fn locked_claimants(
    txs: &HashMap<Txid, TxEntry>,
    claims: &[(OutPoint, Txid)],
) -> HashSet<Txid> {
    (claims.iter().map(|c| c.1))
        .filter(|t| txs.get(t).is_some_and(instant_locked))
        .collect()
}

/// The payments this app sent whose marks may go, each with its stored
/// block: gone from the history (`None`) while their mark keeps no
/// transaction to restore or the store settled them (`stored`: a record,
/// or a height-only row with a height), or in a block at or below the
/// wallet's own ChainLock height. Every other payment keeps its mark, an
/// abandoned or unmined one too, and one whose saved copy does not read
/// back ([`proven_settled`] decides the rest).
pub(crate) fn settled_candidates(
    txs: &HashMap<Txid, TxEntry>,
    sent: &crate::tx_actions::Sent,
    stored: &HashSet<Txid>,
    chainlock_height: Option<u32>,
) -> Vec<(Txid, Option<(u32, dashcore::BlockHash)>)> {
    (sent.iter())
        .filter_map(|(t, mark)| match txs.get(t) {
            None => (!mark.kept || stored.contains(t)).then_some((*t, None)),
            Some(e) => {
                let b = e.context.block_info()?;
                (b.height() <= chainlock_height?)
                    .then_some((*t, Some((b.height(), b.block_hash()))))
            }
        })
        .collect()
}

/// The `candidates` whose marks go: gone from the history, or in the block
/// the header chain holds at their height while it holds the ChainLock's
/// block too. A stored context, a height or a missing header proves
/// nothing: a stored block a reorg replaced is not marked, and a later lock
/// or reorg can make the payment live again.
pub(crate) fn proven_settled(
    candidates: Vec<(Txid, Option<(u32, dashcore::BlockHash)>)>,
    chainlock: Option<(u32, dashcore::BlockHash)>,
    headers: &HashMap<u32, dashcore::BlockHash>,
) -> Vec<Txid> {
    (candidates.into_iter())
        .filter(|(_, block)| block.is_none_or(|b| proven_final(b, chainlock, headers)))
        .map(|(t, _)| t)
        .collect()
}

/// Whether the SPV header chain `headers` proves the block `(height, hash)`
/// final: it holds that block, at or below the block of the wallet's own
/// `chainlock`, which it holds too. A stored context, a height or a
/// missing header proves nothing.
pub(crate) fn proven_final(
    (height, hash): (u32, dashcore::BlockHash),
    chainlock: Option<(u32, dashcore::BlockHash)>,
    headers: &HashMap<u32, dashcore::BlockHash>,
) -> bool {
    chainlock.is_some_and(|(cl_height, cl_hash)| {
        height <= cl_height && headers.get(&cl_height) == Some(&cl_hash)
    }) && headers.get(&height) == Some(&hash)
}

#[derive(Debug, Default)]
pub(crate) struct WalletHistory {
    pub txs: HashMap<Txid, TxEntry>,
    /// The transactions this app dropped as ones that can never confirm,
    /// from their `dropped:` marks and as each drop runs.
    pub drops: HashMap<Txid, DropInfo>,
    /// The own inputs of dropped transactions listed from their abandoned
    /// marks alone, as their `dropped:` marks keep them: what their rows
    /// show, which nothing spends or releases.
    pub drop_inputs: BTreeMap<Txid, crate::tx_actions::OwnInputs>,
    /// Transactions a conflict sweep removed this session: a load whose
    /// marks were read before the sweep restores none of them.
    pub swept: HashSet<Txid>,
    /// Transactions an Abandon took this session. key-wallet dismissed
    /// them, which it keeps in memory only, so a mempool event of one is
    /// one queued before the Abandon.
    pub abandoned_here: HashSet<Txid>,
    /// Payments whose first hand-off this session was definitely not sent
    /// but whose sent marks could not be dropped (`Held`): in memory only,
    /// like the hold itself.
    pub held_here: HashSet<Txid>,
    /// Payments whose spend claims a load restored this session, from their
    /// sent marks: their marks may be all that holds those coins.
    pub claimed_at_load: HashSet<Txid>,
    /// The sent marks the last load chose its spend claims from, when it
    /// restored them: a lock the wallet learns of later chooses again from
    /// them ([`HistoryStore::note_instant_lock`]).
    pub claim_marks: Option<crate::tx_actions::Sent>,
    /// The wallet's coins each payment restored from its sent mark
    /// (`add_unrecorded_sends`) signed, which a later record of it may not
    /// name.
    pub marked: HashMap<Txid, BTreeSet<OutPoint>>,
    /// The coins abandons and drops gave back that wait for their rescan
    /// (`crate::rescan_hold`), from their marks at the open and as each
    /// gives one back; one released goes.
    pub rescan_holds: BTreeMap<OutPoint, crate::rescan_hold::RescanHold>,
    /// Transactions key-wallet held this session: its records when the
    /// history was filled at open and each one its events named since.
    /// Records only read from storage are not among them (platform-wallet
    /// rebuilds no records at load).
    pub session_held: HashSet<Txid>,
    /// The last load of the history missed a stored record or mark.
    pub load_incomplete: bool,
    /// A load filled the history: until then it may lack any record.
    pub loaded: bool,
    /// Transactions with a verified InstantSend lock stored and no record
    /// at the last load, by the transactions their lock names inputs of.
    pub lock_parents: HashMap<Txid, Vec<Txid>>,
    /// The spend claims restored this session, by coin, with the payment
    /// each names.
    pub claimed: HashMap<OutPoint, Txid>,
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
        self.drops.remove(txid);
        self.drop_inputs.remove(txid);
        self.hidden_registrations.remove(txid);
        self.lock_parents.remove(txid);
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

    /// Marks the transactions the app database holds abandoned: gone from
    /// wallet.sqlite, they are kept there so the history still shows them.
    pub fn mark_abandoned(&mut self, abandoned: &[(Txid, Transaction)]) {
        for (txid, tx) in abandoned {
            self.txs
                .entry(*txid)
                .or_insert_with(|| TxEntry {
                    tx: tx.clone(),
                    context: TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs: Default::default(),
                    abandoned: true,
                })
                .abandoned = true;
        }
    }

    /// Notes, for each abandoned entry listed from its mark alone (so with no
    /// own inputs), those its drop mark keeps: its row shows the payment it
    /// was. Its entry stays without them, so nothing releases them.
    pub(crate) fn fill_dropped_inputs(
        &mut self,
        inputs: &BTreeMap<Txid, crate::tx_actions::OwnInputs>,
    ) {
        for (txid, own) in inputs {
            if (self.txs.get(txid)).is_some_and(|e| e.abandoned && e.own_inputs.is_empty()) {
                self.drop_inputs.insert(*txid, own.clone());
            }
        }
    }

    /// Adds a pending entry for each payment of `sent` whose mark holds its
    /// transaction and that neither the history nor the store (`stored`)
    /// holds: one the app signed, and may have handed to the network,
    /// before it quit with no record stored (#170 case 1). Its inputs the
    /// app signed are its own ones, resolved from their funding entries.
    pub fn add_unrecorded_sends(
        &mut self,
        sent: &crate::tx_actions::Sent,
        stored: &HashSet<Txid>,
        network: Network,
    ) {
        for (txid, mark) in sent {
            if self.swept.contains(txid) {
                continue;
            }
            self.marked.insert(*txid, mark.coins.clone());
            let from_mark = |o: &OutPoint| {
                let (value, address) = mark.inputs.get(o)?;
                let address = address.parse::<Address<_>>().ok()?;
                Some((*value, address.require_network(network).ok()?))
            };
            // A record of it read back (a relay after the restart) may name
            // none of its inputs: the mark's fill in what it lacks.
            if let Some(e) = self.txs.get_mut(txid) {
                for (n, i) in e.tx.input.iter().enumerate() {
                    if mark.coins.contains(&i.previous_output)
                        && !e.own_inputs.contains_key(&(n as u32))
                        && let Some(own) = from_mark(&i.previous_output)
                    {
                        e.own_inputs.insert(n as u32, own);
                    }
                }
                continue;
            }
            let Some(tx) = &mark.tx else { continue };
            if stored.contains(txid) {
                continue;
            }
            let own_inputs = (tx.input.iter().enumerate())
                .filter(|(_, i)| mark.coins.contains(&i.previous_output))
                .filter_map(|(n, i)| {
                    let o = i.previous_output;
                    let own = from_mark(&o).or_else(|| {
                        let out = self.txs.get(&o.txid)?.tx.output.get(o.vout as usize)?;
                        Some((
                            out.value,
                            Address::from_script(&out.script_pubkey, network).ok()?,
                        ))
                    })?;
                    Some((n as u32, own))
                })
                .collect::<BTreeMap<_, _>>();
            self.txs.insert(
                *txid,
                TxEntry {
                    tx: tx.clone(),
                    context: TransactionContext::Mempool,
                    instant_locked: false,
                    first_seen: None,
                    own_inputs,
                    abandoned: false,
                },
            );
        }
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

    /// Runs `f` over every wallet's entries together, under the store's
    /// read lock.
    pub fn read_all<R>(&self, f: impl FnOnce(&[&HashMap<Txid, TxEntry>]) -> R) -> R {
        let map = self.wallets.read().unwrap_or_else(|p| p.into_inner());
        f(&map.values().map(|h| &h.txs).collect::<Vec<_>>())
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

    /// Marks `txid` InstantSend-locked in each loaded history but those of
    /// `skip` that holds it out of a block without a known lock: key-wallet
    /// tells only the wallets holding its record or coins (#828). The
    /// wallets marked.
    pub(crate) fn note_instant_lock(&self, txid: &Txid, skip: &HashSet<WalletId>) -> Vec<WalletId> {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        (map.iter_mut())
            .filter(|(id, h)| {
                !skip.contains(*id)
                    && h.loaded
                    && (h.txs.get(txid))
                        .is_some_and(|e| e.context.block_info().is_none() && !instant_locked(e))
            })
            .filter_map(|(id, h)| h.set_instant_locked(txid).then_some(*id))
            .collect()
    }

    /// Forgets a load's claims of `txids` once their marks are gone or
    /// they're abandoned (WalletHistory's `claimed_at_load`).
    pub(crate) fn drop_claims(&self, wallet: &WalletId, txids: impl IntoIterator<Item = Txid>) {
        let mut map = self.wallets.write().unwrap_or_else(|p| p.into_inner());
        if let Some(h) = map.get_mut(wallet) {
            for t in txids {
                h.claimed_at_load.remove(&t);
            }
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
#[derive(Debug, Clone)]
pub(crate) struct Owned {
    pub address: Address,
    pub chain: AddressChain,
    /// Owned by a CoinJoin account (dash-qt treats those outputs as change).
    pub coinjoin: bool,
    /// `address` as text, encoded once per view: a history read shows one
    /// owned address on many records (H-06).
    text: OnceLock<String>,
}

impl Owned {
    pub(crate) fn new(address: Address, chain: AddressChain, coinjoin: bool) -> Self {
        Self {
            address,
            chain,
            coinjoin,
            text: OnceLock::new(),
        }
    }

    /// `address` as text.
    pub(crate) fn text(&self) -> &str {
        self.text.get_or_init(|| self.address.to_string())
    }
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
    /// The transactions this app dropped, resolved ([`drop_states`]).
    pub drops: &'a Drops,
}

fn depth(context: &TransactionContext, tip: u32) -> u32 {
    match context.block_info() {
        Some(info) if tip >= info.height() => tip - info.height() + 1,
        Some(_) => 1,
        None => 0,
    }
}

/// One wallet input: its value and address when the wallet owns it.
/// The cached record wins; otherwise the parent output must pay an owned script.
pub(crate) fn resolve_own_input(
    cached: Option<&(u64, Address)>,
    prev: Option<&OutPoint>,
    history: &HashMap<Txid, TxEntry>,
    owned: &HashMap<ScriptBuf, Owned>,
) -> Option<(u64, Address)> {
    if let Some((v, a)) = cached {
        return Some((*v, a.clone()));
    }
    let prev = prev?;
    let funding = history.get(&prev.txid)?;
    let out = funding.tx.output.get(prev.vout as usize)?;
    let owned = owned.get(&out.script_pubkey)?;
    Some((out.value, owned.address.clone()))
}

fn own_input(view: &ChainView<'_>, entry: &TxEntry, index: usize) -> Option<(u64, Address)> {
    let marked = || {
        (entry.abandoned && entry.own_inputs.is_empty() && !view.drops.inputs.is_empty())
            .then(|| {
                view.drops
                    .inputs
                    .get(&entry.tx.txid())?
                    .get(&(index as u32))
            })
            .flatten()
    };
    resolve_own_input(
        entry.own_inputs.get(&(index as u32)).or_else(marked),
        entry
            .tx
            .input
            .get(index)
            .map(|input| &input.previous_output),
        view.history,
        view.owned,
    )
}

/// Status of a transaction (dash-qt `TransactionRecord::updateStatus`).
pub(crate) fn status_of(
    entry: &TxEntry,
    view: &ChainView<'_>,
    all_from_me: bool,
) -> (TxStatus, bool) {
    let depth = depth(&entry.context, view.tip);
    let chain_locked = chain_locked(&entry.context, view.chainlock_height);
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
            dropped: None,
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
    decompose_with(entry, view, false)
}

fn decompose_with(entry: &TxEntry, view: &ChainView<'_>, marked_coinjoin_send: bool) -> Decomposed {
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
                address: Some(owned.text().to_owned()),
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
        // A payment from the CoinJoin send page (QT-051): marked when sent
        // from this wallet (`DS=1`), or every input a denomination of the
        // CoinJoin account.
        let coinjoin_send = marked_coinjoin_send
            || inputs.iter().flatten().all(|(v, a)| {
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
    pub purpose: HashMap<String, TxPurpose>,
    pub coinjoin_sends: HashSet<String>,
}

/// The asset-lock txids of registrations that took a username, by txid.
/// Only flows whose name step succeeded count: a lock of a nameless flow, or
/// of one that failed before or at its name, stays the generic Platform
/// funding. A later step's error (the profile, the inviter's request) does not
/// undo a name already on Platform. A contested name goes to a vote the
/// registration records no outcome of, and its temporary name may not have
/// been taken either, so neither is named.
pub(crate) fn username_purposes(rows: &[dw_appdb::RegistrationRow]) -> HashMap<String, TxPurpose> {
    use crate::platform::registration_flow::{name_taken, outpoint_of};
    use dash_sdk::platform::dpns_usernames::is_contested_username;
    rows.iter()
        .filter(|r| !r.label.is_empty() && !is_contested_username(&r.label) && name_taken(r))
        .filter_map(|r| {
            let outpoint = outpoint_of(r)?;
            Some((
                outpoint.txid.to_string(),
                TxPurpose::UsernameRegistration {
                    username: r.label.clone(),
                },
            ))
        })
        .collect()
}

/// Every record of one transaction.
pub(crate) fn records_of(
    txid: &Txid,
    entry: &TxEntry,
    view: &ChainView<'_>,
    labels: &Labels,
) -> (Vec<TxRecord>, Decomposed, TxStatus) {
    let txid_s = txid.to_string();
    let d = decompose_with(entry, view, labels.coinjoin_sends.contains(&txid_s));
    let (mut status, counts) = status_of(entry, view, d.all_from_me);
    if status.kind == TxStatusKind::Abandoned {
        status.dropped = view.drops.states.get(txid).copied();
    }
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
            purpose: labels.purpose.get(&txid_s).cloned(),
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
                .is_some_and(|l| l.to_lowercase().contains(text))
            || r.purpose.as_ref().is_some_and(|p| match p {
                TxPurpose::UsernameRegistration { username } => {
                    username.to_lowercase().contains(text)
                }
            });
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

impl SortKey {
    fn parts(&self) -> (i128, &str, u32) {
        (self.primary, &self.txid, self.index)
    }
}

fn primary_of(r: &TxRecord, sort: HistorySort) -> i128 {
    match sort {
        // Unknown times sort as newest: they are fresh mempool arrivals.
        HistorySort::NewestFirst | HistorySort::OldestFirst => {
            i128::from(r.timestamp.unwrap_or(u64::MAX))
        }
        HistorySort::AmountDescending | HistorySort::AmountAscending => i128::from(r.amount),
    }
}

fn key_of(r: &TxRecord, sort: HistorySort) -> SortKey {
    SortKey {
        primary: primary_of(r, sort),
        txid: r.txid.clone(),
        index: r.record_index,
    }
}

/// The sort's order of two keys given as (primary, txid, record index).
fn order(sort: HistorySort, a: (i128, &str, u32), b: (i128, &str, u32)) -> Ordering {
    let primary = match sort {
        HistorySort::NewestFirst | HistorySort::AmountDescending => b.0.cmp(&a.0),
        HistorySort::OldestFirst | HistorySort::AmountAscending => a.0.cmp(&b.0),
    };
    primary
        .then_with(|| a.1.cmp(b.1))
        .then_with(|| a.2.cmp(&b.2))
}

/// `r`'s key in `sort`, borrowed.
fn parts_of(r: &TxRecord, sort: HistorySort) -> (i128, &str, u32) {
    (primary_of(r, sort), &r.txid, r.record_index)
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
    let kept = matching(records, &q.filter);
    let total = u32::try_from(kept.len()).ok();
    // Only the page is sorted: the records after the cursor are selected
    // by index and only the page's are copied out, so a large history is
    // neither sorted nor moved (H-06).
    let part = |i: usize| parts_of(&kept[i], q.sort);
    let by = |a: &usize, b: &usize| order(q.sort, part(*a), part(*b));
    let mut rest: Vec<usize> = (0..kept.len())
        .filter(|&i| {
            (after.as_ref()).is_none_or(|k| order(q.sort, part(i), k.parts()) == Ordering::Greater)
        })
        .collect();
    let limit = q.limit as usize;
    let more = rest.len() > limit;
    if more {
        rest.select_nth_unstable_by(limit - 1, by);
        rest.truncate(limit);
    }
    rest.sort_unstable_by(by);
    let next_cursor = more
        .then(|| {
            rest.last()
                .map(|&i| encode_cursor(tag, &key_of(&kept[i], q.sort)))
        })
        .flatten();
    Ok(HistoryPage {
        records: rest.iter().map(|&i| kept[i].clone()).collect(),
        next_cursor,
        total_matching: total,
    })
}

/// The records `filter` keeps, in their order.
fn matching(mut records: Vec<TxRecord>, filter: &HistoryFilter) -> Vec<TxRecord> {
    let text = filter
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase);
    records.retain(|r| matches(r, filter, text.as_deref()));
    records
}

/// Every record `filter` keeps, in `sort` order (the CSV export's rows).
pub(crate) fn filtered(
    records: Vec<TxRecord>,
    filter: &HistoryFilter,
    sort: HistorySort,
) -> Result<Vec<TxRecord>, EngineError> {
    validate_filter(filter)?;
    let mut records = matching(records, filter);
    records.sort_by(|a, b| order(sort, parts_of(a, sort), parts_of(b, sort)));
    Ok(records)
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
                owned.insert(a.script_pubkey(), Owned::new(a, chain, false));
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
                drops: &crate::history::NO_DROPS,
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

    fn test_key(n: u8) -> dashcore::PublicKey {
        let secret = dashcore::secp256k1::SecretKey::from_secret_bytes([n; 32]).unwrap();
        dashcore::PublicKey::new(dashcore::secp256k1::PublicKey::from_secret_key(&secret))
    }

    fn key_script(n: u8) -> ScriptBuf {
        ScriptBuf::new_p2pkh(&test_key(n).pubkey_hash())
    }

    /// An input spending `o` whose script pushes a signature-shaped blob
    /// and key `n`'s public key, as a P2PKH spend does.
    fn signed_input(o: OutPoint, n: u8) -> TxIn {
        let mut i = input(o.txid, o.vout);
        i.script_sig = dashcore::blockdata::script::Builder::new()
            .push_slice([0x30; 71])
            .push_key(&test_key(n))
            .into_script();
        i
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

    /// One view encodes an owned address once, however many records show
    /// it (H-06: a 40k wallet's page encoded one per record).
    #[test]
    fn a_view_encodes_each_owned_address_once() {
        let mut f = Fixture::new();
        let ids: Vec<Txid> = (0..3)
            .map(|n| {
                let t = tx(vec![foreign_input(n)], vec![out(1_000, &hash_addr(1))]);
                f.add(entry(t, in_block(100, 1_700_000_000)))
            })
            .collect();
        let view = f.view(100);
        let script = hash_addr(1).script_pubkey();
        assert!(view.owned[&script].text.get().is_none());
        let mut encoded = Vec::new();
        for id in &ids {
            let (records, _, _) = records_of(id, &f.history[id], &view, &Labels::default());
            let shown = hash_addr(1).to_string();
            assert_eq!(records[0].address.as_deref(), Some(shown.as_str()));
            encoded.push(view.owned[&script].text.get().unwrap().as_ptr());
        }
        assert!(encoded.windows(2).all(|w| w[0] == w[1]));
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

    fn registration(label: &str, phase: &str, outpoint: Option<&str>) -> dw_appdb::RegistrationRow {
        dw_appdb::RegistrationRow {
            id: 1,
            identity_index: Some(0),
            identity: None,
            label: label.into(),
            temp_label: None,
            funding: String::new(),
            initial_profile: None,
            asset_lock_outpoint: outpoint.map(Into::into),
            phase: phase.into(),
            error: None,
            retryable: false,
            name_error: None,
            profile_error: None,
            owner: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn a_registration_lock_is_labelled_with_its_username() {
        let txid = format!("{:064x}", 7);
        let outpoint = format!("{txid}:0");
        let named = registration("alice2611", "done", Some(&outpoint));
        let purposes = username_purposes(std::slice::from_ref(&named));
        assert_eq!(
            purposes.get(&txid),
            Some(&TxPurpose::UsernameRegistration {
                username: "alice2611".into()
            })
        );

        let mut later_step_failed = named.clone();
        later_step_failed.error = Some("profile.failed".into());
        assert_eq!(username_purposes(&[later_step_failed]).len(), 1);

        // "alice" is contested: whether with a temporary name or not, the lock
        // stays generic, during the vote and after it.
        let contested = registration("alice", "done", Some(&outpoint));
        let mut with_temporary = registration("alice", "contested", Some(&outpoint));
        with_temporary.temp_label = Some("alice2".into());

        let mut failed = registration("alice2611", "name_requested", Some(&outpoint));
        failed.error = Some("registration.failed".into());
        let mut name_refused = named.clone();
        name_refused.name_error = Some("names.taken".into());
        let nameless = registration("", "done", Some(&outpoint));
        let early = registration("alice2611", "funding_sent", Some(&outpoint));
        let unfunded = registration("alice2611", "done", None);
        let unknown_phase = registration("alice2611", "verified_on_mars", Some(&outpoint));
        let malformed = registration("alice2611", "done", Some("not-an-outpoint"));
        for row in [
            contested,
            with_temporary,
            failed,
            name_refused,
            nameless,
            early,
            unfunded,
            unknown_phase,
            malformed,
        ] {
            assert!(username_purposes(&[row]).is_empty());
        }
    }

    #[test]
    fn a_stored_purpose_reaches_the_record_of_its_transaction() {
        let mut f = Fixture::new();
        let locked = f.add(entry(
            tx(vec![foreign_input(8)], vec![out(3_000, &hash_addr(1))]),
            TransactionContext::Mempool,
        ));
        let purpose = TxPurpose::UsernameRegistration {
            username: "alice".into(),
        };
        let labels = Labels {
            purpose: HashMap::from([(locked.to_string(), purpose.clone())]),
            ..Labels::default()
        };
        let (records, ..) = records_of(&locked, &f.history[&locked], &f.view(5), &labels);
        assert!(!records.is_empty());
        assert!(records.iter().all(|r| r.purpose.as_ref() == Some(&purpose)));
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
                dropped: None,
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
            purpose: None,
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
            TxRecord {
                purpose: Some(TxPurpose::UsernameRegistration {
                    username: "alice".into(),
                }),
                ..rec(5, 0, Some(400), -30, TxType::AssetLock)
            },
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
        assert_eq!(
            run(HistoryFilter {
                text: Some("Alice".into()),
                ..Default::default()
            }),
            vec!["05"]
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

    /// Walking every page, for each sort and page size, gives the records
    /// in the full sort's order, each once, whatever order the history
    /// arrives in: with tied times and amounts, unknown times and several
    /// records per transaction.
    #[test]
    fn every_page_walk_matches_the_full_sort() {
        let records: Vec<TxRecord> = (0..40u8)
            .map(|n| {
                let ts = (n % 7 != 0).then_some(u64::from(n % 5) * 100);
                let amount = i64::from(n % 4) * 10 - 15;
                rec(n / 2, u32::from(n % 2), ts, amount, TxType::RecvWithAddress)
            })
            .collect();
        for sort in [
            HistorySort::NewestFirst,
            HistorySort::OldestFirst,
            HistorySort::AmountDescending,
            HistorySort::AmountAscending,
        ] {
            let full: Vec<(String, u32)> =
                filtered(records.clone(), &HistoryFilter::default(), sort)
                    .unwrap()
                    .into_iter()
                    .map(|r| (r.txid, r.record_index))
                    .collect();
            let reversed: Vec<TxRecord> = records.iter().rev().cloned().collect();
            let mut interleaved = records.clone();
            interleaved
                .sort_by_key(|r| (r.record_index, u32::MAX - u32::from(r.txid.as_bytes()[0])));
            for (input, shape) in [
                (&records, "txid order"),
                (&reversed, "reversed"),
                (&interleaved, "interleaved"),
            ] {
                for limit in [1, 3, 7, 40, 41] {
                    let mut q = query(limit);
                    q.sort = sort;
                    let mut walked = Vec::new();
                    let pages = full.len().div_ceil(limit as usize);
                    for step in 0.. {
                        assert!(
                            step < pages,
                            "more than {pages} pages: {sort:?}, limit {limit}, {shape}"
                        );
                        let p = page(input.clone(), &q).unwrap();
                        assert_eq!(p.total_matching, Some(40));
                        assert!(p.records.len() <= limit as usize);
                        walked.extend(p.records.into_iter().map(|r| (r.txid, r.record_index)));
                        match p.next_cursor {
                            Some(c) => q.cursor = Some(c),
                            None => break,
                        }
                    }
                    assert_eq!(walked, full, "{sort:?}, limit {limit}, {shape}");
                }
            }
        }
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

    /// #170 case 4: a payment that spends the change of a beaten payment can
    /// never confirm either, so it claims nothing: its other coins are free
    /// or the winner's. A child that is InstantSend-locked keeps its claims,
    /// as only a ChainLocked block can beat it.
    #[test]
    fn unmined_own_spends_give_a_beaten_payments_children_no_claim() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let mark = |coins: &[OutPoint]| crate::tx_actions::SentMark {
            inputs: Default::default(),
            coins: coins.iter().copied().collect(),
            tx: None,
            kept: false,
        };
        let spend = |n: u8, coins: &[OutPoint], context| {
            entry(
                tx(
                    coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                    vec![out(n.into(), &hash_addr(9))],
                ),
                context,
            )
        };
        let mut txs = HashMap::new();
        let mut sent = crate::tx_actions::Sent::new();
        let parent = spend(1, &[coin(20)], TransactionContext::Mempool);
        let p = parent.tx.txid();
        txs.insert(p, parent);
        sent.insert(p, mark(&[coin(20)]));
        let winner = spend(2, &[coin(20)], in_block(10, 0));
        txs.insert(winner.tx.txid(), winner);
        let change = OutPoint::new(p, 0);
        let child = spend(3, &[change, coin(21)], TransactionContext::Mempool);
        let c = child.tx.txid();
        txs.insert(c, child);
        sent.insert(c, mark(&[change, coin(21)]));
        let grandchild = spend(
            4,
            &[OutPoint::new(c, 0), coin(22)],
            TransactionContext::Mempool,
        );
        let g = grandchild.tx.txid();
        txs.insert(g, grandchild);
        sent.insert(g, mark(&[OutPoint::new(c, 0), coin(22)]));
        let mut locked = spend(
            5,
            &[OutPoint::new(p, 1), coin(23)],
            TransactionContext::Mempool,
        );
        locked.instant_locked = true;
        let l = locked.tx.txid();
        txs.insert(l, locked);
        sent.insert(l, mark(&[coin(23)]));

        let mut claims = unmined_own_spends(&txs, &sent, None);
        claims.sort();
        assert_eq!(claims, vec![(coin(23), l)], "only the locked child claims");
    }

    /// #170 case 3: a payment out of a block that key-wallet holds no record
    /// of and that pays the wallet is beaten by a mined or locked rival of
    /// the history, or by a block key-wallet saw spend its coin; by
    /// key-wallet's finality order, so an InstantSend-locked one only under
    /// a ChainLock. A recorded, abandoned or change-less one is left, and so
    /// is one only a restored lock key-wallet holds no record of beats.
    #[test]
    fn beaten_unrecorded_finds_the_restored_payments_a_rival_beat() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let pays = |coins: &[OutPoint], to: u8, context| {
            entry(
                tx(
                    coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                    vec![out(10, &hash_addr(to))],
                ),
                context,
            )
        };
        let wallet = hash_addr(9).script_pubkey();
        let pays_wallet = |t: &Transaction| t.output.iter().any(|o| o.script_pubkey == wallet);
        let mut txs = HashMap::new();
        let mut add = |e: TxEntry| {
            let t = e.tx.txid();
            txs.insert(t, e);
            t
        };
        let p = add(pays(&[coin(1), coin(2)], 9, TransactionContext::Mempool));
        let winner = add(pays(&[coin(2)], 3, in_block(10, 0)));
        let recorded = add(pays(&[coin(4)], 9, TransactionContext::Mempool));
        add(pays(&[coin(4)], 3, in_block(10, 0)));
        let mut gone = pays(&[coin(5)], 9, TransactionContext::Mempool);
        gone.abandoned = true;
        add(gone);
        add(pays(&[coin(5)], 3, in_block(10, 0)));
        add(pays(&[coin(6)], 3, TransactionContext::Mempool));
        let by_block = add(pays(&[coin(7)], 9, TransactionContext::Mempool));
        add(pays(&[coin(8)], 3, TransactionContext::Mempool));
        let mut locked = pays(&[coin(11)], 9, TransactionContext::Mempool);
        locked.instant_locked = true;
        let locked = add(locked);
        add(pays(&[coin(6), coin(12)], 9, TransactionContext::Mempool));
        add(pays(&[coin(13)], 9, TransactionContext::Mempool));
        let mut restored_lock = pays(&[coin(13)], 3, TransactionContext::Mempool);
        restored_lock.instant_locked = true;
        add(restored_lock);
        let observed = BTreeMap::from([(coin(7), 11), (coin(8), 11), (coin(11), 12)]);
        let txs = txs;

        let found = |chainlock| {
            let mut found =
                beaten_unrecorded(&txs, |t| *t == recorded, &observed, chainlock, pays_wallet);
            found.sort();
            found
        };
        let mut want = vec![
            (
                p,
                Winner {
                    txid: Some(winner),
                    mined: Some(10),
                },
            ),
            (
                by_block,
                Winner {
                    txid: None,
                    mined: Some(11),
                },
            ),
        ];
        want.sort();
        assert_eq!(
            found(Some(11)),
            want,
            "the locked one only under a ChainLock"
        );
        want.push((
            locked,
            Winner {
                txid: None,
                mined: Some(12),
            },
        ));
        want.sort();
        assert_eq!(found(Some(12)), want);
    }

    /// #170 case 4: a swept payment's children, and theirs, out of a block;
    /// a locked, mined or abandoned one is not followed.
    #[test]
    fn unmined_children_follow_every_output_out_of_a_block() {
        let spend = |parents: &[(Txid, u32)], n: u8| {
            entry(
                tx(
                    parents.iter().map(|(t, v)| input(*t, *v)).collect(),
                    vec![out(n.into(), &hash_addr(9))],
                ),
                TransactionContext::Mempool,
            )
        };
        let p = Txid::from_byte_array([1; 32]);
        let mut txs = HashMap::new();
        let mut add = |e: TxEntry| {
            let t = e.tx.txid();
            txs.insert(t, e);
            t
        };
        let c = add(spend(&[(p, 1)], 1));
        let g = add(spend(&[(c, 0)], 2));
        let mut locked = spend(&[(p, 2)], 3);
        locked.instant_locked = true;
        let l = add(locked);
        add(spend(&[(l, 0)], 4));
        let mut mined = spend(&[(p, 3)], 5);
        mined.context = in_block(10, 0);
        add(mined);
        let mut gone = spend(&[(p, 4)], 6);
        gone.abandoned = true;
        add(gone);
        add(spend(&[(Txid::from_byte_array([2; 32]), 0)], 7));

        assert_eq!(unmined_children(&txs, &[p]), BTreeSet::from([c, g]));
    }

    /// #749: the held total sums the held coins whose value the wallet
    /// knows (from the output that made them, or a held payment's record
    /// of its own inputs) and counts apart the one it doesn't but spent
    /// with its own key; a coin a block spent since, and an output the
    /// wallet doesn't own, are left.
    #[test]
    fn held_total_sums_the_valued_coins_and_counts_the_unvalued() {
        let mut txs = HashMap::new();
        let mut add = |e: TxEntry| {
            let t = e.tx.txid();
            txs.insert(t, e);
            t
        };
        let funding = add(entry(
            tx(
                vec![foreign_input(1)],
                vec![
                    out(700, &hash_addr(1)),
                    out(900, &hash_addr(3)),
                    out(50, &hash_addr(1)),
                ],
            ),
            in_block(10, 0),
        ));
        let (valued, foreign, settled) = (
            OutPoint::new(funding, 0),
            OutPoint::new(funding, 1),
            OutPoint::new(funding, 2),
        );
        add(entry(
            tx(vec![input(funding, 2)], vec![out(40, &hash_addr(3))]),
            in_block(11, 0),
        ));
        let unknown = OutPoint::new(Txid::from_byte_array([8; 32]), 0);
        let recorded = OutPoint::new(Txid::from_byte_array([9; 32]), 1);
        let mut held = entry(
            tx(
                vec![signed_input(unknown, 4), input(recorded.txid, 1)],
                vec![out(10, &hash_addr(3))],
            ),
            TransactionContext::Mempool,
        );
        held.own_inputs.insert(1, (300, hash_addr(2)));
        add(held);
        let observed_coin = OutPoint::new(Txid::from_byte_array([7; 32]), 0);
        let owned = HashSet::from([
            hash_addr(1).script_pubkey(),
            hash_addr(2).script_pubkey(),
            key_script(4),
        ]);
        let holds = BTreeSet::from([valued, foreign, settled, unknown, recorded, observed_coin]);
        let observed = BTreeMap::from([(observed_coin, 12)]);

        assert_eq!(
            held_total(&still_held(&holds, &txs, &observed, |_| false), &txs, |s| {
                owned.contains(s)
            }),
            (700 + 300, 1)
        );
        assert_eq!(
            held_total(&BTreeSet::from([valued, unknown]), &txs, |s| owned
                .contains(s)),
            (700, 1),
            "one valued and one unvalued"
        );
    }

    /// #749 (Q81 C2): the coins a beaten incoming payment, or the other
    /// inputs of a beaten mix, held are other people's: neither valued nor
    /// counted. The mix's own input still counts by its record, as does a
    /// coin whose output in the history the wallet doesn't own.
    #[test]
    fn held_total_leaves_out_coins_others_spent() {
        let owned: HashSet<ScriptBuf> =
            HashSet::from([hash_addr(1).script_pubkey(), key_script(4)]);
        let mut txs = HashMap::new();
        let (theirs, unsigned) = (
            OutPoint::new(Txid::from_byte_array([8; 32]), 0),
            OutPoint::new(Txid::from_byte_array([8; 32]), 1),
        );
        let incoming = entry(
            tx(
                vec![signed_input(theirs, 5), input(unsigned.txid, 1)],
                vec![out(500, &hash_addr(1))],
            ),
            TransactionContext::Mempool,
        );
        txs.insert(incoming.tx.txid(), incoming);
        let (mine, other) = (
            OutPoint::new(Txid::from_byte_array([9; 32]), 0),
            OutPoint::new(Txid::from_byte_array([9; 32]), 1),
        );
        let mut mix = entry(
            tx(
                vec![signed_input(mine, 4), signed_input(other, 6)],
                vec![out(100, &hash_addr(1)), out(100, &hash_addr(3))],
            ),
            TransactionContext::Mempool,
        );
        mix.own_inputs.insert(0, (100, hash_addr(1)));
        txs.insert(mix.tx.txid(), mix);
        let made = entry(
            tx(vec![foreign_input(2)], vec![out(900, &hash_addr(3))]),
            TransactionContext::Mempool,
        );
        let recorded = OutPoint::new(made.tx.txid(), 0);
        txs.insert(made.tx.txid(), made);
        let mut spent = entry(
            tx(vec![input(recorded.txid, 0)], vec![out(10, &hash_addr(3))]),
            TransactionContext::Mempool,
        );
        spent.own_inputs.insert(0, (250, hash_addr(1)));
        txs.insert(spent.tx.txid(), spent);
        let holds = BTreeSet::from([theirs, unsigned, mine, other, recorded]);

        assert_eq!(
            held_total(&holds, &txs, |s| owned.contains(s)),
            (100 + 250, 0),
            "an own-input record values a coin whose output isn't owned"
        );
    }

    /// #170 case 4 at load: a stored payment out of a block whose own coin's
    /// transaction has no stored row and no live entry is orphaned; a
    /// parent stored, or live in the history, keeps it.
    #[test]
    fn orphaned_unmined_are_the_children_of_a_parent_the_store_lost() {
        let spend = |parent: Txid, n: u8, own: bool| {
            let mut e = entry(
                tx(vec![input(parent, 0)], vec![out(n.into(), &hash_addr(9))]),
                TransactionContext::Mempool,
            );
            if own {
                e.own_inputs.insert(0, (10, hash_addr(1)));
            }
            e
        };
        let (gone, kept) = (
            Txid::from_byte_array([1; 32]),
            Txid::from_byte_array([2; 32]),
        );
        let mut txs = HashMap::new();
        let mut stored = HashSet::from([kept]);
        let mut add = |e: TxEntry, store: bool| {
            let t = e.tx.txid();
            txs.insert(t, e);
            if store {
                stored.insert(t);
            }
            t
        };
        let orphan = add(spend(gone, 1, true), true);
        add(spend(kept, 2, true), true);
        add(spend(gone, 3, false), true);
        add(spend(gone, 4, true), false);
        let live = add(spend(gone, 5, false), false);
        add(spend(live, 6, true), true);
        let mut dead = spend(gone, 7, false);
        dead.abandoned = true;
        let dead = add(dead, false);
        let under_dead = add(spend(dead, 8, true), true);
        let mut locked = spend(gone, 9, true);
        locked.instant_locked = true;
        add(locked, true);
        let mut mined = spend(gone, 10, true);
        mined.context = in_block(10, 0);
        add(mined, true);

        let mut found = orphaned_unmined(&txs, &stored, &HashSet::new());
        found.sort();
        let mut want = vec![orphan, under_dead];
        want.sort();
        assert_eq!(found, want);
        assert_eq!(
            orphaned_unmined(&txs, &stored, &HashSet::from([gone])),
            vec![under_dead],
            "a claim the store dropped is the claim rule's"
        );
    }

    /// #749 (Q81b): a drop's origin note reads back as written, with or
    /// without a winner and with any number of coins; a value it did not
    /// write reads as none.
    #[test]
    fn a_drop_origin_reads_back_and_refuses_what_it_did_not_write() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let beaten = DropOrigin {
            winner: Some(t(1)),
            contested: BTreeSet::from([
                OutPoint::new(t(2), 0),
                OutPoint::new(t(2), 3),
                OutPoint::new(t(4), 1),
            ]),
            gone: BTreeSet::from([t(5), t(6)]),
        };
        let orphan = DropOrigin {
            winner: None,
            contested: BTreeSet::from([OutPoint::new(t(2), 0)]),
            gone: BTreeSet::from([t(7)]),
        };
        for origin in [beaten, orphan, DropOrigin::default()] {
            assert_eq!(DropOrigin::decode(&origin.encode()), Some(origin));
        }
        assert_eq!(DropOrigin::default().encode(), "||");
        let w = t(1).to_string();
        for bad in [
            String::new(),
            format!("{w}|"),
            format!("{w}|||"),
            "nope||".to_owned(),
            format!("|{w}|"),
            format!("||{w}:0"),
        ] {
            assert_eq!(DropOrigin::decode(&bad), None, "{bad:?}");
        }
    }

    /// #749 (Q81b S3): a spend note reads back as written, with or without
    /// its spender; a value it did not write reads as none.
    #[test]
    fn a_spend_note_reads_back_and_refuses_what_it_did_not_write() {
        let block = BlockHash::from_byte_array([3; 32]);
        let spender = Txid::from_byte_array([4; 32]);
        for note in [
            SpendNote {
                height: 12,
                block,
                spender: Some(spender),
            },
            SpendNote {
                height: 0,
                block,
                spender: None,
            },
        ] {
            assert_eq!(SpendNote::decode(&note.encode()), Some(note));
        }
        let unknown = SpendNote {
            height: 7,
            block,
            spender: None,
        };
        assert_eq!(unknown.encode(), format!("7,{block},"));
        for bad in [
            String::new(),
            format!("7,{block}"),
            format!("7,{block},,"),
            format!("x,{block},"),
            format!("-1,{block},"),
            "7,nope,".to_owned(),
            format!("7,{block},nope"),
        ] {
            assert_eq!(SpendNote::decode(&bad), None, "{bad:?}");
        }
    }

    /// #749: of the coins the drops hold, those `left_out` names (a final
    /// spend's note, an output of a gone parent) are no longer held; the
    /// rest are.
    #[test]
    fn still_held_leaves_out_the_coins_left_out_names() {
        let gone = Txid::from_byte_array([1; 32]);
        let other = Txid::from_byte_array([2; 32]);
        let (noted, kept, orphaned) = (
            OutPoint::new(other, 0),
            OutPoint::new(other, 1),
            OutPoint::new(gone, 0),
        );
        let holds = BTreeSet::from([noted, kept, orphaned]);
        let (txs, observed) = (HashMap::new(), BTreeMap::new());
        let left_out = |o: &OutPoint| *o == noted || o.txid == gone;
        assert_eq!(
            still_held(&holds, &txs, &observed, left_out),
            BTreeSet::from([kept])
        );
        assert_eq!(still_held(&holds, &txs, &observed, |_| false), holds);
    }

    /// #170 case 4 (#749): an orphan's gone parents are the parents of its
    /// own inputs with no stored row and no live entry (none, or
    /// abandoned); a stored parent, a live one, and the parent of an input
    /// not its own are not, nor is anything of a txid the history lacks.
    #[test]
    fn gone_parents_are_the_own_inputs_parents_with_no_row_and_no_live_entry() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let (gone, stored_parent, not_own) = (t(1), t(2), t(5));
        let mut txs = HashMap::new();
        let live = entry(
            tx(vec![foreign_input(9)], vec![out(10, &hash_addr(1))]),
            TransactionContext::Mempool,
        );
        let live_parent = live.tx.txid();
        txs.insert(live_parent, live);
        let mut dead = entry(
            tx(vec![foreign_input(8)], vec![out(20, &hash_addr(1))]),
            TransactionContext::Mempool,
        );
        dead.abandoned = true;
        let dead_parent = dead.tx.txid();
        txs.insert(dead_parent, dead);
        let mut orphan = entry(
            tx(
                vec![
                    input(gone, 0),
                    input(stored_parent, 0),
                    input(live_parent, 0),
                    input(dead_parent, 0),
                    input(not_own, 0),
                ],
                vec![out(5, &hash_addr(3))],
            ),
            TransactionContext::Mempool,
        );
        for i in 0..4 {
            orphan.own_inputs.insert(i, (10, hash_addr(1)));
        }
        let orphan_id = orphan.tx.txid();
        txs.insert(orphan_id, orphan);
        let stored = HashSet::from([stored_parent, orphan_id]);

        assert_eq!(
            gone_parents(&txs, &stored, &[orphan_id, t(6)]),
            BTreeSet::from([gone, dead_parent])
        );
    }

    /// #749 (Q81b S1, S2): whether a held coin awaits, rule by rule. R,
    /// dropped, spent a and b; W, its winner, spends a, mined at 10; W2
    /// spends b, InstantSend-locked and out of a block. b awaits W2 though
    /// W is ChainLocked, awaits W above the ChainLock, and is settled once W
    /// is ChainLocked and nothing live spends it. A winner the history
    /// doesn't hold settles on a contested coin's final spend only. A
    /// record spending the coin, or a second drop holding it unsettled,
    /// keeps it awaiting; nothing held awaits nothing.
    #[test]
    fn held_awaiting_judges_each_held_coin() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let (a, b) = (OutPoint::new(t(1), 0), OutPoint::new(t(1), 1));
        let mut txs = HashMap::new();
        let mut r = entry(
            tx(
                vec![input(t(1), 0), input(t(1), 1)],
                vec![out(10, &hash_addr(3))],
            ),
            TransactionContext::Mempool,
        );
        r.abandoned = true;
        let r_id = r.tx.txid();
        txs.insert(r_id, r);
        let w = entry(
            tx(vec![input(t(1), 0)], vec![out(20, &hash_addr(3))]),
            in_block(10, 0),
        );
        let w_id = w.tx.txid();
        txs.insert(w_id, w);
        let mut w2 = entry(
            tx(vec![input(t(1), 1)], vec![out(30, &hash_addr(3))]),
            TransactionContext::Mempool,
        );
        w2.instant_locked = true;
        let mut with_w2 = txs.clone();
        with_w2.insert(w2.tx.txid(), w2);
        let origin = |winner: Txid| {
            Some(DropOrigin {
                winner: Some(winner),
                contested: BTreeSet::from([a, b]),
                gone: BTreeSet::new(),
            })
        };
        let dropped =
            |origin: Option<DropOrigin>| HeldDrops::from([(r_id, (BTreeSet::from([b]), origin))]);
        let held = BTreeSet::from([b]);
        let awaits = |drops: &HeldDrops, txs: &HashMap<Txid, TxEntry>, chainlock: Option<u32>| {
            held_awaiting(&held, drops, txs, &no_evidence(chainlock))
        };

        assert!(
            awaits(&dropped(origin(w_id)), &with_w2, Some(10)),
            "b awaits W2 though W is ChainLocked"
        );
        assert!(
            awaits(&dropped(origin(w_id)), &txs, Some(9)),
            "W mined above the ChainLock"
        );
        assert!(
            !awaits(&dropped(origin(w_id)), &txs, Some(10)),
            "W ChainLocked, nothing live spends b"
        );
        let unheld = t(9);
        assert!(
            !held_awaiting(
                &held,
                &dropped(origin(unheld)),
                &txs,
                &HeldEvidence {
                    final_spend: &|o| *o == a,
                    ..no_evidence(Some(9))
                },
            ),
            "a contested coin's final spend settles a winner the history doesn't hold"
        );
        assert!(
            awaits(&dropped(origin(unheld)), &txs, Some(9)),
            "a winner the history doesn't hold, and no final spend"
        );
        assert!(
            !awaits(&dropped(origin(unheld)), &txs, Some(10)),
            "a ChainLocked transaction of the history spends a contested coin"
        );
        assert!(
            held_awaiting(
                &held,
                &dropped(origin(w_id)),
                &txs,
                &HeldEvidence {
                    spent_by_record: &|o| *o == b,
                    ..no_evidence(Some(10))
                },
            ),
            "a key-wallet record spends b"
        );
        let mut two = dropped(origin(w_id));
        let elsewhere = DropOrigin {
            winner: Some(unheld),
            contested: BTreeSet::from([OutPoint::new(t(2), 0)]),
            gone: BTreeSet::new(),
        };
        two.insert(t(8), (BTreeSet::from([b]), Some(elsewhere)));
        assert!(
            awaits(&two, &txs, Some(10)),
            "another drop holding b awaits"
        );
        assert!(
            !held_awaiting(
                &BTreeSet::new(),
                &dropped(origin(w_id)),
                &with_w2,
                &HeldEvidence {
                    spent_by_record: &|_| true,
                    unknown_lock: &|_| true,
                    ..no_evidence(None)
                },
            ),
            "nothing held"
        );
        assert!(
            held_awaiting(
                &held,
                &dropped(origin(w_id)),
                &txs,
                &HeldEvidence {
                    unknown_lock: &|o| *o == b,
                    ..no_evidence(Some(10))
                },
            ),
            "a lock of a transaction the wallet holds no copy of may spend b"
        );
    }

    /// #907: a drop's winner W, or a ChainLocked transaction spending a
    /// coin it contested, settles the drop only once the header chain
    /// proves its block final: W stored in block B at 10, under a ChainLock
    /// of 11, while the header chain holds B' at 10, is no proof; W in B'
    /// is.
    #[test]
    fn held_awaiting_does_not_settle_on_a_winner_whose_block_the_header_chain_replaced() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let (a, b) = (OutPoint::new(t(1), 0), OutPoint::new(t(1), 1));
        let (replaced, kept, lock) = (
            BlockHash::from_byte_array([1; 32]),
            BlockHash::from_byte_array([2; 32]),
            BlockHash::from_byte_array([3; 32]),
        );
        let headers = HashMap::from([(10, kept), (11, lock)]);
        let chain_final = |e: &TxEntry| {
            (e.context.block_info()).is_some_and(|b| {
                proven_final((b.height(), b.block_hash()), Some((11, lock)), &headers)
            })
        };
        let evidence = HeldEvidence {
            chain_final: &chain_final,
            ..no_evidence(None)
        };
        let in_hash = |hash: BlockHash| TransactionContext::InBlock(BlockInfo::new(10, hash, 0));
        let spend_of_a_in = |hash| {
            entry(
                tx(vec![input(t(1), 0)], vec![out(20, &hash_addr(3))]),
                in_hash(hash),
            )
        };
        let held = BTreeSet::from([b]);
        for (name, winner_in_history) in [
            ("the winner", true),
            ("a spender of a contested coin", false),
        ] {
            for (hash, awaits) in [(replaced, true), (kept, false)] {
                let w = spend_of_a_in(hash);
                let w_id = w.tx.txid();
                let txs = HashMap::from([(w_id, w)]);
                let winner = match winner_in_history {
                    true => w_id,
                    false => t(9),
                };
                let drops = HeldDrops::from([(
                    t(5),
                    (
                        held.clone(),
                        Some(DropOrigin {
                            winner: Some(winner),
                            contested: BTreeSet::from([a, b]),
                            gone: BTreeSet::new(),
                        }),
                    ),
                )]);
                assert_eq!(
                    held_awaiting(&held, &drops, &txs, &evidence),
                    awaits,
                    "{name} in {hash}"
                );
            }
        }
    }

    /// [`HeldEvidence`] of nothing, under `chainlock`, whose header chain
    /// proves every block at or below it.
    fn no_evidence(chainlock: Option<u32>) -> HeldEvidence<'static> {
        let final_under = move |e: &TxEntry| {
            (e.context.block_info()).is_some_and(|b| chainlock.is_some_and(|cl| b.height() <= cl))
        };
        HeldEvidence {
            chain_final: Box::leak(Box::new(final_under)),
            final_spend: &|_| false,
            spent_by_record: &|_| false,
            unknown_lock: &|_| false,
            dead_for_good: &|_| false,
        }
    }

    /// A sweep note reads back as written, its block known or not, and
    /// counts as absent when it does not read, or its bytes are not those
    /// of the transaction its key names (Q81d S2).
    #[test]
    fn a_swept_by_note_reads_back_and_refuses_what_it_did_not_write() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let p = tx(vec![input(t(1), 0)], vec![out(10, &hash_addr(3))]);
        let block = (7, BlockHash::from_byte_array([5; 32]));
        for (claim, block) in [(false, Some(block)), (false, None), (true, None)] {
            let note = SweptBy {
                claim,
                winner: t(2),
                block,
                tx: p.clone(),
            };
            assert_eq!(SweptBy::decode(&p.txid(), &note.encode()), Some(note));
        }
        let note = SweptBy {
            claim: false,
            winner: t(2),
            block: Some(block),
            tx: p.clone(),
        }
        .encode();
        assert_eq!(SweptBy::decode(&t(9), &note), None, "another's bytes");
        let parts: Vec<&str> = note.split('|').collect();
        for bad in [
            String::new(),
            format!("replaced|{}|{}|{}", parts[1], parts[2], parts[3]),
            format!("{}|{}|7|{}", parts[0], parts[1], parts[3]),
            format!("{}|{}|{}|{}00", parts[0], parts[1], parts[2], parts[3]),
            format!("{}|{}|{}|zz", parts[0], parts[1], parts[2]),
            format!("{note}|"),
        ] {
            assert_eq!(SweptBy::decode(&p.txid(), &bad), None, "{bad:?}");
        }
    }

    /// A conflict sweep's note keeps its winner's block while the header
    /// chain agrees, fills it from the winner's block record, and drops it
    /// when the header chain names another block at its height or a fork
    /// truncated at or below it (Q81d S4, S5); a claim sweep's keeps none.
    #[test]
    fn a_sweep_notes_block_follows_the_header_chain() {
        let p = tx(vec![input(Txid::all_zeros(), 0)], Vec::new());
        let (x, y) = (
            BlockHash::from_byte_array([1; 32]),
            BlockHash::from_byte_array([2; 32]),
        );
        let note = |block| SweptBy {
            claim: false,
            winner: Txid::from_byte_array([9; 32]),
            block,
            tx: p.clone(),
        };
        let none = HashMap::new();
        let says_x = HashMap::from([(7, x)]);
        let says_y = HashMap::from([(7, y)]);
        let at_x = Some((7, x));
        assert_eq!(swept_by_block(&note(at_x), None, &none, None), at_x);
        assert_eq!(swept_by_block(&note(at_x), None, &says_x, None), at_x);
        assert_eq!(swept_by_block(&note(at_x), None, &says_y, None), None);
        assert_eq!(swept_by_block(&note(at_x), None, &none, Some(7)), None);
        assert_eq!(swept_by_block(&note(at_x), None, &none, Some(6)), None);
        assert_eq!(swept_by_block(&note(at_x), None, &none, Some(8)), at_x);
        assert_eq!(swept_by_block(&note(None), at_x, &says_x, None), at_x);
        assert_eq!(swept_by_block(&note(None), at_x, &says_y, None), None);
        assert_eq!(swept_by_block(&note(None), None, &says_x, None), None);
        let claim = SweptBy {
            claim: true,
            ..note(at_x)
        };
        assert_eq!(swept_by_block(&claim, at_x, &says_x, None), None);
    }

    /// An orphan's drop (Q81c §2, Q81d): its gone parent P's death is final
    /// only by (a) a never-valid verdict, (b) a conflict sweep's winner in
    /// a proven block, or (c) another transaction's final spend of one of
    /// P's inputs. P's abandoned copy alone, an IS-only winner, a block the
    /// header chain doesn't prove, a claim sweep's final winner, a spend
    /// note with no spender or P's own, and a P live again prove nothing.
    /// A swept descendant of P settles through its own note.
    #[test]
    fn an_orphans_drop_settles_only_on_each_gone_parents_final_death() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let a = OutPoint::new(t(1), 0);
        let p = tx(vec![input(t(1), 0)], vec![out(10, &hash_addr(3))]);
        let p_id = p.txid();
        let q = tx(vec![input(p_id, 0)], vec![out(9, &hash_addr(3))]);
        let q_id = q.txid();
        let d = OutPoint::new(t(4), 0);
        let c_id = t(5);
        let (x, y) = (
            BlockHash::from_byte_array([1; 32]),
            BlockHash::from_byte_array([2; 32]),
        );
        let proven = |b: (u32, BlockHash)| b == (7, x);
        let orphan = |gone: &[Txid]| {
            HeldDrops::from([(
                c_id,
                (
                    BTreeSet::from([d]),
                    Some(DropOrigin {
                        winner: None,
                        contested: BTreeSet::from([d]),
                        gone: gone.iter().copied().collect(),
                    }),
                ),
            )])
        };
        let held = BTreeSet::from([d]);
        let awaits = |deaths: &Deaths,
                      txs: &HashMap<Txid, TxEntry>,
                      spent: &BTreeMap<OutPoint, SpendNote>,
                      gone: &[Txid]| {
            let dead = |g: &Txid| dead_for_good(g, deaths, txs, spent, proven);
            held_awaiting(
                &held,
                &orphan(gone),
                txs,
                &HeldEvidence {
                    dead_for_good: &dead,
                    ..no_evidence(None)
                },
            )
        };
        let none = HashMap::new();
        let no_notes = BTreeMap::new();
        let swept = |claim, block| SweptBy {
            claim,
            winner: t(2),
            block,
            tx: p.clone(),
        };
        let with_note = |note: SweptBy| Deaths {
            swept_by: BTreeMap::from([(p_id, note)]),
            ..Deaths::default()
        };

        let copied = Deaths {
            copies: BTreeMap::from([(p_id, p.clone())]),
            ..Deaths::default()
        };
        assert!(awaits(&copied, &none, &no_notes, &[p_id]), "copy alone");
        let never_valid = Deaths {
            never_valid: BTreeSet::from([p_id]),
            ..Deaths::default()
        };
        assert!(!awaits(&never_valid, &none, &no_notes, &[p_id]), "(a)");
        let mut live = none.clone();
        live.insert(p_id, entry(p.clone(), TransactionContext::Mempool));
        assert!(awaits(&never_valid, &live, &no_notes, &[p_id]), "P live");

        assert!(
            awaits(&with_note(swept(false, None)), &none, &no_notes, &[p_id]),
            "IS-only"
        );
        let final_note = with_note(swept(false, Some((7, x))));
        assert!(!awaits(&final_note, &none, &no_notes, &[p_id]), "(b)");
        let other_block = with_note(swept(false, Some((7, y))));
        assert!(
            awaits(&other_block, &none, &no_notes, &[p_id]),
            "unproven block"
        );
        let claim = with_note(swept(true, Some((7, x))));
        assert!(awaits(&claim, &none, &no_notes, &[p_id]), "claim sweep");

        let spend = |spender| {
            BTreeMap::from([(
                a,
                SpendNote {
                    height: 7,
                    block: x,
                    spender,
                },
            )])
        };
        let in_note = with_note(swept(false, None));
        assert!(
            !awaits(&in_note, &none, &spend(Some(t(2))), &[p_id]),
            "(c) by note"
        );
        assert!(
            !awaits(&copied, &none, &spend(Some(t(2))), &[p_id]),
            "(c) by copy"
        );
        assert!(awaits(&in_note, &none, &spend(None), &[p_id]), "no spender");
        assert!(
            awaits(&in_note, &none, &spend(Some(p_id)), &[p_id]),
            "P's own"
        );
        let unproven = BTreeMap::from([(
            a,
            SpendNote {
                height: 7,
                block: y,
                spender: Some(t(2)),
            },
        )]);
        assert!(
            awaits(&in_note, &none, &unproven, &[p_id]),
            "unproven spend"
        );
        let mut chainlocked = none.clone();
        let w = entry(
            tx(vec![input(t(1), 0)], vec![out(5, &hash_addr(3))]),
            TransactionContext::InChainLockedBlock(BlockInfo::new(7, x, 0)),
        );
        chainlocked.insert(w.tx.txid(), w);
        assert!(
            !awaits(&copied, &chainlocked, &no_notes, &[p_id]),
            "(c) by the history"
        );

        assert!(
            awaits(&final_note, &none, &no_notes, &[p_id, q_id]),
            "a swept descendant with no note of its own"
        );
        let mut both = with_note(swept(false, Some((7, x))));
        both.swept_by.insert(
            q_id,
            SweptBy {
                tx: q.clone(),
                ..swept(false, Some((7, x)))
            },
        );
        assert!(
            !awaits(&both, &none, &no_notes, &[p_id, q_id]),
            "each by its note"
        );
        assert!(
            !awaits(&both, &none, &no_notes, &[q_id]),
            "the descendant alone"
        );
    }

    /// Q81d: a parent whose `dropped:` mark is no verdict by its bytes (a
    /// replaced transaction's, or one that doesn't read) leaves the orphan
    /// waiting until (b) or (c) holds; a claim sweep's parent waits for (a)
    /// alone, however final its winner or a spend of its input. A child of
    /// a payment the claim rule kept as built on a never-valid claim
    /// settles with no ChainLock, as the claim rule's own drop of it would.
    #[test]
    fn an_orphan_of_a_parent_with_no_verdict_waits_for_its_death() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let p = tx(vec![input(t(1), 0)], vec![out(10, &hash_addr(3))]);
        let p_id = p.txid();
        let x = BlockHash::from_byte_array([1; 32]);
        let proven = |b: (u32, BlockHash)| b == (7, x);
        let none = HashMap::new();
        let dead = |deaths: &Deaths, spent: &BTreeMap<OutPoint, SpendNote>| {
            dead_for_good(&p_id, deaths, &none, spent, proven)
        };
        let replaced_or_garbage = Deaths {
            copies: BTreeMap::from([(p_id, p.clone())]),
            ..Deaths::default()
        };
        assert!(!dead(&replaced_or_garbage, &BTreeMap::new()), "no verdict");
        let spent = BTreeMap::from([(
            OutPoint::new(t(1), 0),
            SpendNote {
                height: 7,
                block: x,
                spender: Some(t(2)),
            },
        )]);
        assert!(dead(&replaced_or_garbage, &spent), "(c)");
        let conflict = Deaths {
            swept_by: BTreeMap::from([(
                p_id,
                SweptBy {
                    claim: false,
                    winner: t(2),
                    block: Some((7, x)),
                    tx: p.clone(),
                },
            )]),
            ..Deaths::default()
        };
        assert!(dead(&conflict, &BTreeMap::new()), "(b)");
        let claim = Deaths {
            swept_by: BTreeMap::from([(
                p_id,
                SweptBy {
                    claim: true,
                    winner: t(2),
                    block: Some((7, x)),
                    tx: p.clone(),
                },
            )]),
            ..Deaths::default()
        };
        assert!(
            !dead(&claim, &BTreeMap::new()),
            "a claim sweep's final winner"
        );
        assert!(
            !dead(&claim, &spent),
            "a claim sweep's parent dies by (a) only"
        );

        let mut path3 = none.clone();
        let mut kept = entry(p.clone(), TransactionContext::Mempool);
        kept.abandoned = true;
        path3.insert(p_id, kept);
        let built_on_a_claim = Deaths {
            never_valid: BTreeSet::from([p_id]),
            copies: BTreeMap::from([(p_id, p.clone())]),
            ..claim
        };
        assert!(dead_for_good(
            &p_id,
            &built_on_a_claim,
            &path3,
            &BTreeMap::new(),
            |_| false
        ));
    }

    /// An older drop (no `drop_winner:` note, Q81c C2) contests the inputs
    /// of its roots, read through the copies of the dead transactions it
    /// descends from, never an output of one; with no copy it contests
    /// nothing. It settles once one has a final spend, and awaits before.
    #[test]
    fn an_older_drop_settles_on_a_final_spend_of_a_root_input() {
        let t = |n: u8| Txid::from_byte_array([n; 32]);
        let (a, d) = (OutPoint::new(t(1), 0), OutPoint::new(t(1), 1));
        let r = tx(vec![input(t(1), 0)], vec![out(10, &hash_addr(3))]);
        let r_id = r.txid();
        let c = tx(
            vec![input(r_id, 0), input(t(1), 1)],
            vec![out(9, &hash_addr(3))],
        );
        let c_id = c.txid();
        let copies = BTreeMap::from([(r_id, r), (c_id, c)]);
        let dropped = |p: &Txid| *p == r_id || *p == c_id;
        let origin = older_origin(c_id, |t| copies.get(t), dropped);
        assert_eq!(origin.winner, None);
        assert_eq!(origin.contested, BTreeSet::from([a, d]));
        assert!(origin.gone.is_empty());
        let no_copy = older_origin(t(9), |t| copies.get(t), dropped);
        assert_eq!(no_copy, DropOrigin::default());

        let drops = HeldDrops::from([(c_id, (BTreeSet::from([d]), Some(origin)))]);
        let held = BTreeSet::from([d]);
        let none = HashMap::new();
        assert!(held_awaiting(&held, &drops, &none, &no_evidence(Some(9))));
        let evidence = HeldEvidence {
            final_spend: &|o| *o == a,
            ..no_evidence(Some(9))
        };
        assert!(!held_awaiting(&held, &drops, &none, &evidence));
    }

    /// #88: the claims restored at load are the inputs of each payment this
    /// app sent that is out of a block and not abandoned (a lock outranks
    /// the abandon), never those of a pending spend it did not send nor of
    /// one a block beat.
    #[test]
    fn unmined_own_spends_claim_only_live_payments_this_app_sent() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let spend = |n: u8, coins: &[OutPoint], context| {
            entry(
                tx(
                    coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                    vec![out(n.into(), &hash_addr(9))],
                ),
                context,
            )
        };
        let mut txs = HashMap::new();
        let mut sent = crate::tx_actions::Sent::new();
        let mark = |coins: &[OutPoint], tx| crate::tx_actions::SentMark {
            inputs: Default::default(),
            coins: coins.iter().copied().collect(),
            tx,
            kept: false,
        };
        let mut add = |e: TxEntry, signed: &[OutPoint]| {
            let txid = e.tx.txid();
            txs.insert(txid, e);
            if !signed.is_empty() {
                sent.insert(txid, mark(signed, None));
            }
            txid
        };
        let live = add(
            spend(1, &[coin(1), coin(2), coin(7)], TransactionContext::Mempool),
            &[coin(1), coin(2), coin(8)],
        );
        let mined = add(spend(2, &[coin(3)], in_block(10, 0)), &[coin(3)]);
        let mut abandoned = spend(3, &[coin(4)], TransactionContext::Mempool);
        abandoned.abandoned = true;
        let abandoned = add(abandoned, &[coin(4)]);
        let mut relocked = spend(4, &[coin(5)], TransactionContext::Mempool);
        relocked.abandoned = true;
        relocked.instant_locked = true;
        let relocked = add(relocked, &[coin(5)]);
        add(spend(5, &[coin(6)], TransactionContext::Mempool), &[]);
        let beaten = add(
            spend(6, &[coin(10), coin(11)], TransactionContext::Mempool),
            &[coin(10), coin(11)],
        );
        add(spend(7, &[coin(11)], in_block(11, 0)), &[]);
        let unstored = Txid::from_byte_array([9; 32]);
        sent.insert(unstored, mark(&[coin(9)], None));

        let mut claims = unmined_own_spends(&txs, &sent, None);
        claims.sort();
        let mut want = vec![(coin(1), live), (coin(2), live), (coin(5), relocked)];
        want.sort();
        assert_eq!(claims, want);
        assert!(claims.iter().all(|(_, t)| *t != beaten));
        let zeros = BlockHash::all_zeros();
        let other = BlockHash::from_byte_array([1; 32]);
        let tip = BlockHash::from_byte_array([2; 32]);
        let settled = |txs: &HashMap<Txid, TxEntry>,
                       cl: Option<(u32, BlockHash)>,
                       headers: &[(u32, BlockHash)]| {
            let candidates = settled_candidates(txs, &sent, &HashSet::new(), cl.map(|c| c.0));
            let mut s = proven_settled(candidates, cl, &headers.iter().copied().collect());
            s.sort();
            s
        };
        let mut gone_and_mined = vec![mined, unstored];
        gone_and_mined.sort();
        assert_eq!(settled(&txs, None, &[]), vec![unstored]);
        assert_eq!(
            settled(&txs, Some((9, tip)), &[(9, tip), (10, zeros)]),
            vec![unstored]
        );
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(10, zeros), (11, tip)]),
            gone_and_mined,
            "the header chain holds its block and the lock's"
        );
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(10, other), (11, tip)]),
            vec![unstored],
            "the header chain holds another block at its height"
        );
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(10, zeros)]),
            vec![unstored],
            "the lock's header is not known yet"
        );
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(10, zeros), (11, other)]),
            vec![unstored],
            "the header chain holds another block at the lock's height"
        );
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(11, tip)]),
            vec![unstored],
            "its header is not known yet"
        );
        assert!(!settled(&txs, Some((11, tip)), &[(10, zeros), (11, tip)]).contains(&abandoned));
        let chainlocked = txs.get_mut(&mined).unwrap();
        let block = chainlocked.context.block_info().cloned().unwrap();
        chainlocked.context = TransactionContext::InChainLockedBlock(block);
        assert_eq!(
            settled(&txs, Some((11, tip)), &[(10, other), (11, tip)]),
            vec![unstored],
            "a stored ChainLocked context proves nothing"
        );
        let reorged = txs.get_mut(&mined).unwrap();
        reorged.context = TransactionContext::Mempool;
        reorged.instant_locked = true;
        assert!(
            unmined_own_spends(&txs, &sent, None).contains(&(coin(3), mined)),
            "a reorg back to the mempool, locked: the kept mark claims again"
        );
    }

    /// #170 case 11: only a ChainLock beats an InstantSend lock
    /// (key-wallet's finality order). A locked payment beaten on an input
    /// by a plain block or another lock keeps its claims; a block the
    /// wallet's ChainLock covers, by height or by the record's context,
    /// beats it. A payment without a lock loses to any block or lock.
    #[test]
    fn only_a_chainlock_beats_a_locked_payments_claims() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let spend = |n: u8, coins: &[OutPoint], context| {
            entry(
                tx(
                    coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                    vec![out(n.into(), &hash_addr(9))],
                ),
                context,
            )
        };
        let locked = || TransactionContext::InstantSend(dashcore::InstantLock::default());
        let chainlocked =
            TransactionContext::InChainLockedBlock(BlockInfo::new(12, BlockHash::all_zeros(), 0));
        let claims = |payment: TxEntry, winner: TxEntry, chainlock: Option<u32>| {
            let txid = payment.tx.txid();
            let mark = crate::tx_actions::SentMark {
                coins: [coin(1), coin(2)].into(),
                tx: None,
                kept: false,
                inputs: Default::default(),
            };
            let sent = crate::tx_actions::Sent::from([(txid, mark)]);
            let txs = HashMap::from([(txid, payment), (winner.tx.txid(), winner)]);
            let mut got = unmined_own_spends(&txs, &sent, chainlock);
            got.sort();
            got.into_iter().map(|(o, _)| o).collect::<Vec<_>>()
        };
        let all = vec![coin(1), coin(2)];
        let payment = || spend(1, &[coin(1), coin(2)], locked());
        let mut flagged = spend(1, &[coin(1), coin(2)], TransactionContext::Mempool);
        flagged.instant_locked = true;
        let plain = || spend(2, &[coin(2)], in_block(12, 0));

        assert_eq!(claims(payment(), plain(), None), all, "a plain block");
        assert_eq!(claims(payment(), plain(), Some(11)), all, "below it");
        assert_eq!(claims(flagged.clone(), plain(), None), all, "lock flag");
        assert_eq!(
            claims(payment(), spend(2, &[coin(2)], locked()), Some(12)),
            all
        );
        assert!(claims(payment(), plain(), Some(12)).is_empty());
        assert!(claims(flagged, plain(), Some(13)).is_empty());
        assert!(claims(payment(), spend(2, &[coin(2)], chainlocked), None).is_empty());

        let unlocked = || spend(1, &[coin(1), coin(2)], TransactionContext::Mempool);
        assert!(claims(unlocked(), plain(), None).is_empty());
        assert!(claims(unlocked(), spend(2, &[coin(2)], locked()), None).is_empty());
        assert_eq!(
            claims(unlocked(), spend(2, &[coin(3)], in_block(12, 0)), None),
            all
        );
    }

    /// A locked payment P spends the output of an unlocked parent Q that a
    /// plain block beats: P keeps its own inputs held (key-wallet's
    /// `drop_conflicted_transactions` walks no InstantSend-locked child).
    #[test]
    fn a_locked_child_of_a_beaten_parent_keeps_its_inputs_held() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let q = entry(
            tx(vec![input(coin(1).txid, 0)], vec![out(1, &hash_addr(9))]),
            TransactionContext::Mempool,
        );
        let w = entry(
            tx(vec![input(coin(1).txid, 0)], vec![out(2, &hash_addr(9))]),
            in_block(12, 0),
        );
        let p = entry(
            tx(
                vec![input(q.tx.txid(), 0), input(coin(5).txid, 0)],
                vec![out(3, &hash_addr(9))],
            ),
            TransactionContext::InstantSend(dashcore::InstantLock::default()),
        );
        let txs: HashMap<Txid, TxEntry> = [q, w, p].into_iter().map(|e| (e.tx.txid(), e)).collect();
        assert!(
            spent_outpoints(&txs, None).contains(&coin(5)),
            "A stays held"
        );
    }

    /// Two mined spends of one coin B (a fork's block the store kept) beside
    /// a locked payment P of A and B: a ChainLocked one beats P whichever
    /// spend the map visits last, so A is not held. 16 independent pairs, so
    /// a rule that kept the last-visited spend fails with odds 1 - 2^-16.
    #[test]
    fn a_chainlocked_spend_beats_a_locked_payment_beside_a_plain_one() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let chainlocked =
            TransactionContext::InChainLockedBlock(BlockInfo::new(12, BlockHash::all_zeros(), 0));
        let locked = TransactionContext::InstantSend(dashcore::InstantLock::default());
        let mut txs = HashMap::new();
        for n in 0..16u8 {
            let (a, b, v) = (coin(100 + n), coin(150 + n), u64::from(n) * 10);
            let spends = |coins: &[OutPoint], value: u64, context| {
                let ins = coins.iter().map(|c| input(c.txid, c.vout)).collect();
                entry(tx(ins, vec![out(value, &hash_addr(9))]), context)
            };
            for e in [
                spends(&[a, b], v + 1, locked.clone()),
                spends(&[b], v + 2, in_block(13, 0)),
                spends(&[b], v + 3, chainlocked.clone()),
            ] {
                txs.insert(e.tx.txid(), e);
            }
        }
        let spent = spent_outpoints(&txs, None);
        assert!(
            (0..16).all(|n| !spent.contains(&coin(100 + n))),
            "no A held"
        );
    }

    /// #170 case 14: only another transaction that spends one of a
    /// payment's inputs in a block of the header chain at or below the
    /// wallet's ChainLock overrides its InstantSend lock; a block above it,
    /// a record's own ChainLocked context or another lock does not.
    #[test]
    fn only_a_chainlocked_spend_of_an_input_beats_a_payment() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let spend = |n: u8, coins: &[OutPoint], context| {
            entry(
                tx(
                    coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                    vec![out(n.into(), &hash_addr(9))],
                ),
                context,
            )
        };
        let locked = || TransactionContext::InstantSend(dashcore::InstantLock::default());
        let payment = spend(1, &[coin(1), coin(2)], locked());
        let txid = payment.tx.txid();
        let active = HashMap::from([(12, BlockHash::all_zeros())]);
        let beaten_on =
            |winner: TxEntry, chainlock: Option<u32>, active: &HashMap<u32, BlockHash>| {
                let mut active = active.clone();
                let chainlock = chainlock.map(|h| {
                    (
                        h,
                        *active
                            .entry(h)
                            .or_insert(BlockHash::from_byte_array([7; 32])),
                    )
                });
                let txs = HashMap::from([(txid, payment.clone()), (winner.tx.txid(), winner)]);
                chainlock_beaten(&txs, &txid, chainlock, &active)
            };
        let beaten = |winner, chainlock| beaten_on(winner, chainlock, &active);
        let chainlocked =
            TransactionContext::InChainLockedBlock(BlockInfo::new(12, BlockHash::all_zeros(), 0));
        assert!(beaten(spend(2, &[coin(2)], chainlocked.clone()), Some(12)));
        assert!(
            !beaten(spend(2, &[coin(2)], chainlocked.clone()), None),
            "a ChainLocked context alone"
        );
        assert!(beaten(spend(2, &[coin(2)], in_block(12, 0)), Some(12)));
        assert!(!beaten(spend(2, &[coin(2)], in_block(12, 0)), Some(11)));
        assert!(!beaten(spend(2, &[coin(2)], in_block(12, 0)), None));
        assert!(!beaten(spend(2, &[coin(2)], locked()), Some(12)));
        assert!(!beaten(spend(2, &[coin(3)], chainlocked.clone()), Some(12)));
        let forked = HashMap::from([(12, BlockHash::from_byte_array([1; 32]))]);
        let winner = || spend(2, &[coin(2)], chainlocked.clone());
        assert!(
            !beaten_on(winner(), Some(12), &forked),
            "a fork replaced it"
        );
        assert!(!beaten_on(winner(), Some(12), &HashMap::new()), "no header");
        let txs = HashMap::from([(txid, payment.clone()), (winner().tx.txid(), winner())]);
        let lock = Some((13, BlockHash::from_byte_array([7; 32])));
        assert!(
            !chainlock_beaten(&txs, &txid, lock, &active),
            "the ChainLock's header is not known"
        );
        let alone = HashMap::from([(txid, payment.clone())]);
        let lock = Some((12, BlockHash::all_zeros()));
        assert!(!chainlock_beaten(&alone, &txid, lock, &active));
        let mut mined = payment.clone();
        mined.context = in_block(12, 0);
        let alone = HashMap::from([(txid, mined)]);
        assert!(
            !chainlock_beaten(&alone, &txid, lock, &active),
            "not by itself"
        );
        let txs = HashMap::from([(txid, payment.clone()), (winner().tx.txid(), winner())]);
        assert_eq!(rival_heights(&txs), BTreeSet::from([12]));
        let unrelated = spend(3, &[coin(3)], in_block(7, 0));
        let txs = HashMap::from([(txid, payment.clone()), (unrelated.tx.txid(), unrelated)]);
        assert!(rival_heights(&txs).is_empty());
    }

    /// #170 case 1: a payment whose mark holds its transaction and that
    /// no record holds stays marked, gets a pending entry unless the store
    /// holds it, and then claims the coins the app signed.
    #[test]
    fn a_marked_payment_without_a_record_is_restored_pending() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let pay = |coins: &[OutPoint]| {
            tx(
                coins.iter().map(|c| input(c.txid, c.vout)).collect(),
                vec![out(1, &hash_addr(9))],
            )
        };
        let (lost, stored) = (pay(&[coin(1), coin(2)]), pay(&[coin(3)]));
        let sent = crate::tx_actions::Sent::from([
            (
                lost.txid(),
                crate::tx_actions::SentMark {
                    inputs: Default::default(),
                    coins: [coin(1)].into(),
                    tx: Some(lost.clone()),
                    kept: true,
                },
            ),
            (
                stored.txid(),
                crate::tx_actions::SentMark {
                    inputs: Default::default(),
                    coins: [coin(3)].into(),
                    tx: Some(stored.clone()),
                    kept: true,
                },
            ),
        ]);
        let gone = |h: &WalletHistory, stored: &HashSet<Txid>| {
            let candidates = settled_candidates(&h.txs, &sent, stored, None);
            proven_settled(candidates, None, &HashMap::new())
        };
        let mut h = WalletHistory::default();
        assert!(gone(&h, &HashSet::new()).is_empty());
        h.add_unrecorded_sends(&sent, &HashSet::from([stored.txid()]), Network::Regtest);
        let entry = h.txs.get(&lost.txid()).expect("restored");
        assert_eq!(entry.tx, lost);
        assert!(entry.context.block_info().is_none() && !entry.is_abandoned());
        assert!(!h.txs.contains_key(&stored.txid()));
        assert_eq!(
            unmined_own_spends(&h.txs, &sent, None),
            vec![(coin(1), lost.txid())]
        );
        assert!(gone(&h, &HashSet::new()).is_empty());
        let stored_rows = HashSet::from([stored.txid()]);
        assert_eq!(gone(&h, &stored_rows), vec![stored.txid()]);
    }

    /// A payment gone from the history settles only when its mark keeps no
    /// transaction to restore or the store settled it: one whose saved
    /// copy does not read back keeps its mark, so a lost record never
    /// frees the coins it spent.
    #[test]
    fn a_payment_gone_from_the_history_settles_only_on_evidence() {
        let coin = |n: u8| OutPoint::new(Txid::from_byte_array([n; 32]), 0);
        let mark = |kept: bool| crate::tx_actions::SentMark {
            inputs: Default::default(),
            coins: [coin(1)].into(),
            tx: None,
            kept,
        };
        let (damaged, old) = (
            Txid::from_byte_array([7; 32]),
            Txid::from_byte_array([8; 32]),
        );
        let sent = crate::tx_actions::Sent::from([(damaged, mark(true)), (old, mark(false))]);
        let txs = HashMap::new();
        let gone = |stored: &HashSet<Txid>| {
            let mut s = proven_settled(
                settled_candidates(&txs, &sent, stored, None),
                None,
                &HashMap::new(),
            );
            s.sort();
            s
        };
        assert_eq!(gone(&HashSet::new()), vec![old], "a damaged copy settles");
        let mut both = vec![damaged, old];
        both.sort();
        assert_eq!(gone(&HashSet::from([damaged])), both);
    }

    /// A dropped row's state, by precedence: the drop's hold, then the
    /// highest state any coin not back needs (#790).
    #[test]
    fn a_dropped_row_takes_the_highest_state_its_coins_need() {
        use DropState::*;
        let from = |height| RescanFromHeight { height };
        let cases: [(bool, Vec<DropState>, DropState); 12] = [
            (false, vec![], Released),
            (false, vec![Releasing], Releasing),
            (false, vec![Releasing, RescanNeeded], RescanNeeded),
            (false, vec![RescanNeeded, from(90)], from(90)),
            (false, vec![from(90), from(70), Releasing], from(70)),
            (false, vec![from(90), AwaitingFunding], AwaitingFunding),
            (
                false,
                vec![AwaitingFunding, Releasing, RescanNeeded],
                AwaitingFunding,
            ),
            (true, vec![], Held),
            (true, vec![AwaitingFunding], Held),
            (true, vec![from(5), RescanNeeded, Releasing], Held),
            (false, vec![Releasing, Releasing], Releasing),
            (
                false,
                vec![RescanNeeded, Releasing, RescanNeeded],
                RescanNeeded,
            ),
        ];
        for (held, coins, want) in cases {
            assert_eq!(
                row_state(held, coins.clone()),
                want,
                "held {held}, coins {coins:?}"
            );
        }
    }

    fn facts<'a>(
        unspent: &'a dyn Fn(&OutPoint) -> bool,
        scan_at: Option<u32>,
        pending: bool,
        birth: u32,
    ) -> DropFacts<'a> {
        DropFacts {
            unspent,
            block_spent: &|_| false,
            scan_at,
            pending,
            birth,
        }
    }

    /// What one coin not back needs: its funding unmined, a running scan
    /// below its block, a block below the birth, owed rewinds, or else a
    /// rescan.
    #[test]
    fn a_coin_not_back_needs_what_its_block_and_the_scan_say() {
        use DropState::*;
        let no = |_: &OutPoint| false;
        let cases = [
            (None, None, false, 100, AwaitingFunding),
            (None, Some(10), true, 100, AwaitingFunding),
            (Some(150), Some(120), false, 100, Releasing),
            (Some(150), Some(150), false, 100, RescanNeeded),
            (Some(150), Some(160), false, 100, RescanNeeded),
            (Some(150), None, true, 100, Releasing),
            (Some(150), None, false, 100, RescanNeeded),
            (Some(80), None, false, 100, RescanFromHeight { height: 80 }),
            (Some(80), None, true, 100, RescanFromHeight { height: 80 }),
            (Some(80), Some(60), false, 100, Releasing),
            (Some(100), None, false, 100, RescanNeeded),
        ];
        for (funded_at, scan_at, pending, birth, want) in cases {
            let f = facts(&no, scan_at, pending, birth);
            assert_eq!(
                coin_state(funded_at, &f),
                want,
                "funded {funded_at:?}, scan {scan_at:?}, pending {pending}, birth {birth}"
            );
        }
    }

    /// The scan the state reads: a cancelled replay, or a rewind that never
    /// lowered the checkpoint (it failed), runs no scan; a rewind's replay
    /// does until it is back at its target; an open owes its rewinds.
    #[test]
    fn the_scan_a_drop_reads_runs_only_below_its_target_and_not_cancelled() {
        let state = |synced, target, cancelled: Option<u32>, loading| crate::events::WalletState {
            synced_height: synced,
            scan_target: target,
            replay_cancelled: cancelled,
            loading,
            ..Default::default()
        };
        assert_eq!(scan_facts(None, false), (None, false));
        assert_eq!(scan_facts(None, true), (None, true));
        assert_eq!(
            scan_facts(Some(&state(90, 200, None, false)), false),
            (Some(90), false),
            "a rewind's replay"
        );
        assert_eq!(
            scan_facts(Some(&state(90, 200, Some(90), false)), false),
            (None, false),
            "a cancelled replay"
        );
        assert_eq!(
            scan_facts(Some(&state(200, 200, None, false)), false),
            (None, false),
            "back at its target, or never lowered"
        );
        assert_eq!(
            scan_facts(Some(&state(200, 200, None, true)), false),
            (None, true),
            "an open owes its rewinds"
        );
    }

    /// A dropped payment's coins: one spent from a dropped or abandoned
    /// parent, or a never-valid one's, is never waited for; one unspent,
    /// spent by a block or by a transaction the history counts is back.
    #[test]
    fn a_drop_waits_only_for_the_wallets_own_coins_of_counted_parents() {
        let mut f = Fixture::new();
        let fund = |n: u8, v| {
            tx(
                vec![foreign_input(n)],
                vec![out(v, &hash_addr(1)), out(v, &hash_addr(60))],
            )
        };
        let a = f.add(entry(fund(40, 1_000), in_block(150, 1)));
        let b = f.add(entry(fund(41, 2_000), in_block(160, 1)));
        let mut gone = entry(fund(42, 3_000), TransactionContext::Mempool);
        gone.abandoned = true;
        let gone = f.add(gone);
        let claim = f.add(entry(fund(43, 4_000), TransactionContext::Mempool));
        let mut s = entry(
            tx(
                vec![
                    input(a, 0),
                    input(b, 0),
                    input(gone, 0),
                    input(claim, 0),
                    input(a, 1),
                ],
                vec![out(9_000, &hash_addr(70))],
            ),
            TransactionContext::Mempool,
        );
        s.abandoned = true;
        let s = f.add(s);
        let drops = HashMap::from([
            (s, DropInfo::new(DropCause::InvalidSource)),
            (claim, DropInfo::new(DropCause::InvalidClaim)),
        ]);
        let (a0, b0) = (OutPoint::new(a, 0), OutPoint::new(b, 0));
        let state = |unspent: &dyn Fn(&OutPoint) -> bool, scan_at| {
            let facts = facts(unspent, scan_at, false, 100);
            drop_states(&f.history, &drops, &f.owned, &facts)
        };
        let back = |o: &OutPoint| *o == a0 || *o == b0;
        assert_eq!(
            state(&back, None).get(&s),
            Some(&TxDrop {
                cause: DropCause::InvalidSource,
                state: DropState::Released
            }),
            "only a and b, the wallet's own coins of counted parents, are waited for"
        );
        assert!(
            !state(&back, None).contains_key(&claim),
            "the claim isn't abandoned: no row"
        );
        let only_a = |o: &OutPoint| *o == a0;
        assert_eq!(
            state(&only_a, Some(155)).get(&s).map(|d| d.state),
            Some(DropState::Releasing),
            "a scan below b's block"
        );
        assert_eq!(
            state(&only_a, None).get(&s).map(|d| d.state),
            Some(DropState::RescanNeeded)
        );
        let spender = f.add(entry(
            tx(vec![input(b, 0)], vec![out(1_500, &hash_addr(71))]),
            TransactionContext::Mempool,
        ));
        let facts = facts(&only_a, None, false, 100);
        assert_eq!(
            drop_states(&f.history, &drops, &f.owned, &facts)
                .get(&s)
                .map(|d| d.state),
            Some(DropState::Released),
            "b is spent by a transaction the history counts"
        );
        f.history.get_mut(&spender).unwrap().abandoned = true;
        assert_eq!(
            drop_states(&f.history, &drops, &f.owned, &facts)
                .get(&s)
                .map(|d| d.state),
            Some(DropState::RescanNeeded),
            "not by an abandoned one"
        );
        let birth_above = super::tests::facts(&only_a, None, false, 170);
        assert_eq!(
            drop_states(&f.history, &drops, &f.owned, &birth_above)
                .get(&s)
                .map(|d| d.state),
            Some(DropState::RescanFromHeight { height: 160 }),
        );
        let block_spent = |o: &OutPoint| *o == b0;
        let by_block = DropFacts {
            block_spent: &block_spent,
            ..super::tests::facts(&only_a, None, false, 100)
        };
        assert_eq!(
            drop_states(&f.history, &drops, &f.owned, &by_block)
                .get(&s)
                .map(|d| d.state),
            Some(DropState::Released)
        );
        let mut held = drops.clone();
        held.get_mut(&s).unwrap().held.insert(b0);
        assert_eq!(
            drop_states(&f.history, &held, &f.owned, &facts)
                .get(&s)
                .map(|d| d.state),
            Some(DropState::Held)
        );
        f.history.get_mut(&claim).unwrap().context = in_block(165, 1);
        let mined_claim = |unspent: &dyn Fn(&OutPoint) -> bool| {
            let facts = super::tests::facts(unspent, None, false, 100);
            (drop_states(&f.history, &drops, &f.owned, &facts).get(&s)).map(|d| d.state)
        };
        assert_eq!(
            mined_claim(&back),
            Some(DropState::RescanNeeded),
            "a dropped parent mined since is waited for: its coin isn't back"
        );
        let claim0 = OutPoint::new(claim, 0);
        let all_back = |o: &OutPoint| *o == a0 || *o == b0 || *o == claim0;
        assert_eq!(mined_claim(&all_back), Some(DropState::Released));
    }

    /// A dropped row's status names its drop; a user's abandon, or a row
    /// mined since, names none.
    #[test]
    fn only_an_abandoned_row_the_app_dropped_says_dropped() {
        let mut f = Fixture::new();
        let fund = f.add(entry(
            tx(vec![foreign_input(50)], vec![out(5_000, &hash_addr(1))]),
            in_block(150, 1),
        ));
        let mut s = entry(
            tx(vec![input(fund, 0)], vec![out(4_000, &hash_addr(70))]),
            TransactionContext::Mempool,
        );
        s.abandoned = true;
        let s = f.add(s);
        let mut user = entry(
            tx(vec![foreign_input(51)], vec![out(4_000, &hash_addr(71))]),
            TransactionContext::Mempool,
        );
        user.abandoned = true;
        let user = f.add(user);
        let mined = f.add(entry(
            tx(vec![input(fund, 0)], vec![out(3_000, &hash_addr(72))]),
            in_block(151, 1),
        ));
        let drop = TxDrop {
            cause: DropCause::InvalidSource,
            state: DropState::Released,
        };
        let drops = Drops {
            states: [(s, drop), (mined, drop)].into_iter().collect(),
            ..Drops::default()
        };
        let mut view = f.view(160);
        view.drops = &drops;
        let status = |t: Txid| records_of(&t, &f.history[&t], &view, &Labels::default()).2;
        let dropped = records_of(&s, &f.history[&s], &view, &Labels::default());
        assert_eq!(dropped.0[0].status.dropped, Some(drop));
        assert!(
            !dropped.0[0].counts_toward_balance,
            "a dropped row never counts"
        );
        assert_eq!(status(user).dropped, None, "the user's abandon");
        assert_eq!(status(mined).kind, TxStatusKind::Confirmed);
        assert_eq!(
            status(mined).dropped,
            None,
            "mined since: its ordinary status"
        );
    }
}
