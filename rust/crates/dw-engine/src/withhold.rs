//! Coins the engine keeps out of spending and out of every balance, each
//! with its reason ([`ExclusionReason`]). One map per wallet,
//! [`SessionHub::excluded_coins`], recomputed from key-wallet and the
//! history on every read, never stored: the balance read
//! ([`Excluded::of`], subtracted in `WalletState::balances`) and the coin
//! snapshot (`NetworkSession::coin_snapshot`) consume only it.
//!
//! [`ExclusionReason::PendingUnproven`] (#818's insurance): the outputs of
//! an unmined payment one of whose coins no evidence holds spent
//! ([`pending_unproven`]), and of every unmined transaction built on them.
//! A block that mines the payment, or an InstantSend lock, brings them back
//! at the next read; a reorg that takes it out of its block withholds them
//! again. Nothing is dropped, freed or abandoned here.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use dashcore::{Address, OutPoint, Txid};
use key_wallet::account::AccountType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

use crate::WalletId;
use crate::events::SessionHub;
use crate::history::{TxEntry, WalletHistory, instant_locked};

/// Why a coin is kept out of spending and every balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExclusionReason {
    /// An output of a payment that may never confirm, or of a transaction
    /// built on one ([`pending_unproven`]): shown as pending.
    PendingUnproven,
}

impl ExclusionReason {
    /// Whether the coin is still listed (with its reason) rather than
    /// left out of the coin list altogether.
    pub(crate) fn listed(self) -> bool {
        match self {
            Self::PendingUnproven => true,
        }
    }
}

/// A wallet's excluded coins, each with its reason.
pub(crate) type Exclusions = BTreeMap<OutPoint, ExclusionReason>;

/// What the excluded coins add to key-wallet's balances: the wallet's
/// buckets and each account's, by key-wallet's own bucket rule, so that
/// subtracting them leaves the balances without those coins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Excluded {
    pub balance: WalletCoreBalance,
    pub accounts: BTreeMap<AccountType, WalletCoreBalance>,
}

impl Excluded {
    /// The share of `coins` in `core`'s balances. Buckets as key-wallet's
    /// `update_balance` fills them: locked, then immature, then confirmed
    /// (confirmed, InstantSend-locked or trusted), else unconfirmed.
    pub(crate) fn of(core: &ManagedWalletInfo, coins: &Exclusions) -> Self {
        let mut out = Self::default();
        if coins.is_empty() {
            return out;
        }
        let height = core.last_processed_height();
        for account in core.accounts.all_funding_accounts() {
            let (mut confirmed, mut unconfirmed, mut immature, mut locked) = (0u64, 0u64, 0, 0);
            for u in (account.utxos.values()).filter(|u| coins.contains_key(&u.outpoint)) {
                let value = u.txout.value;
                if u.is_locked {
                    locked += value;
                } else if !u.is_mature(height) {
                    immature += value;
                } else if u.is_confirmed || u.is_instantlocked || u.is_trusted {
                    confirmed += value;
                } else {
                    unconfirmed += value;
                }
            }
            let share = WalletCoreBalance::new(confirmed, unconfirmed, immature, locked);
            if share.total() > 0 {
                out.balance += share;
                let t = account.managed_account_type().to_account_type();
                *out.accounts.entry(t).or_default() += share;
            }
        }
        out
    }

    /// The excluded coins' value: what shows as pending.
    pub(crate) fn total(&self) -> u64 {
        self.balance.total()
    }

    /// `balance` without `share` (`None`: nothing), bucket by bucket.
    pub(crate) fn subtract(
        balance: &WalletCoreBalance,
        share: Option<&WalletCoreBalance>,
    ) -> WalletCoreBalance {
        let Some(s) = share else {
            return *balance;
        };
        WalletCoreBalance::new(
            balance.confirmed().saturating_sub(s.confirmed()),
            balance.unconfirmed().saturating_sub(s.unconfirmed()),
            balance.immature().saturating_sub(s.immature()),
            balance.locked().saturating_sub(s.locked()),
        )
    }
}

impl SessionHub {
    /// Wallet `wallet`'s excluded coins as `core` (read under the manager's
    /// lock) and the history hold them now. Every surface that counts or
    /// offers coins reads this map and nothing else.
    pub(crate) fn excluded_coins(&self, wallet: WalletId, core: &ManagedWalletInfo) -> Exclusions {
        let withheld = (self.history).read_wallet(&wallet, |h| pending_unproven(core, h));
        (withheld.into_iter())
            .map(|o| (o, ExclusionReason::PendingUnproven))
            .collect()
    }

    /// The share of wallet `wallet`'s excluded coins in `core`'s balances.
    pub(crate) fn excluded_share(&self, wallet: WalletId, core: &ManagedWalletInfo) -> Excluded {
        Excluded::of(core, &self.excluded_coins(wallet, core))
    }

    /// Whether wallet `id` may hold withheld coins now or after its next
    /// change: some are, or its history holds an unmined transaction,
    /// neither abandoned nor InstantSend-locked, that spends a coin the
    /// history knows as the wallet's (its own input, claim or mark, or an
    /// output of a stored transaction). A wallet event's balance is then
    /// read again with the excluded coins, not applied (an event's
    /// snapshot comes without them).
    pub(crate) fn may_withhold(&self, id: &WalletId) -> bool {
        let excluded = (self.wallet_state(id)).is_some_and(|s| s.excluded.total() > 0);
        excluded
            || (self.history).read_wallet(id, |h| {
                (h.txs.iter()).any(|(t, e)| withholding_candidate(e) && spends_known_coin(h, t, e))
            })
    }
}

/// Unmined, neither abandoned nor InstantSend-locked as the history holds
/// it.
fn withholding_candidate(e: &TxEntry) -> bool {
    e.context.block_info().is_none() && !e.abandoned && !instant_locked(e)
}

/// Whether `e` spends a coin the history alone knows as the wallet's,
/// as [`pending_unproven`]'s ownership test can find it without key-wallet.
fn spends_known_coin(h: &WalletHistory, t: &Txid, e: &TxEntry) -> bool {
    (e.tx.input.iter().zip(0u32..)).any(|(i, n)| {
        let o = &i.previous_output;
        e.own_inputs.contains_key(&n)
            || h.claimed.get(o) == Some(t)
            || h.marked.get(t).is_some_and(|m| m.contains(o))
            || h.txs.contains_key(&o.txid)
    })
}

/// E1 (Q100): the coins of `core` withheld because a payment they descend
/// from may never confirm. A root is a payment of the history that is
/// unmined, not abandoned, not InstantSend-locked (by the history or by
/// key-wallet's locks), that key-wallet holds no record of, and that
/// spends a coin of the wallet which nothing else holds spent: not in
/// key-wallet's observed spends, spent by no other transaction of the
/// history that is not abandoned, and not an unspent coin of key-wallet's
/// (only the payment's own claim holds it). The root's outputs that
/// key-wallet holds unspent are withheld, and so is every output of an
/// unmined, unabandoned transaction built on a withheld one.
pub(crate) fn pending_unproven(core: &ManagedWalletInfo, h: &WalletHistory) -> BTreeSet<OutPoint> {
    let txs = &h.txs;
    if !txs.values().any(withholding_candidate) {
        return BTreeSet::new();
    }
    let funding = core.accounts.all_funding_accounts();
    let unspent = |o: &OutPoint| funding.iter().any(|a| a.utxos.contains_key(o));
    let accounts = core.accounts.all_accounts();
    let recorded = |t: &Txid| accounts.iter().any(|a| a.has_transaction(t));
    let observed = core.observed_spent_outpoints();
    let locks = core.instant_send_locks();
    let network = core.network();
    let mut spenders: HashMap<OutPoint, Vec<Txid>> = HashMap::new();
    for (t, e) in txs.iter().filter(|(_, e)| !e.is_abandoned()) {
        for i in &e.tx.input {
            spenders.entry(i.previous_output).or_default().push(*t);
        }
    }
    let owns = |o: &OutPoint, t: &Txid, index: u32, e: &TxEntry| {
        e.own_inputs.contains_key(&index)
            || h.claimed.get(o) == Some(t)
            || h.marked.get(t).is_some_and(|m| m.contains(o))
            || (txs.get(&o.txid))
                .and_then(|p| p.tx.output.get(o.vout as usize))
                .and_then(|out| Address::from_script(&out.script_pubkey, network).ok())
                .is_some_and(|a| crate::coins::wallet_owns(core, &a))
    };
    let unproven = |o: &OutPoint, t: &Txid, index: u32, e: &TxEntry| {
        !observed.contains_key(o)
            && !unspent(o)
            && (spenders.get(o)).is_none_or(|s| s.iter().all(|s| s == t))
            && owns(o, t, index, e)
    };
    let mut withheld: HashSet<Txid> = (txs.iter())
        .filter(|(t, e)| withholding_candidate(e) && !locks.contains(*t) && !recorded(t))
        .filter(|(t, e)| {
            (e.tx.input.iter().zip(0u32..)).any(|(i, n)| unproven(&i.previous_output, t, n, e))
        })
        .map(|(t, _)| *t)
        .collect();
    if withheld.is_empty() {
        return BTreeSet::new();
    }
    let mut todo: Vec<Txid> = withheld.iter().copied().collect();
    let mut children: HashMap<Txid, Vec<Txid>> = HashMap::new();
    for (t, e) in txs.iter() {
        if e.context.block_info().is_none() && !e.abandoned {
            for i in &e.tx.input {
                children.entry(i.previous_output.txid).or_default().push(*t);
            }
        }
    }
    while let Some(t) = todo.pop() {
        for c in children.get(&t).into_iter().flatten() {
            if withheld.insert(*c) {
                todo.push(*c);
            }
        }
    }
    (funding.iter())
        .flat_map(|a| a.utxos.keys())
        .filter(|o| withheld.contains(&o.txid))
        .copied()
        .collect()
}
