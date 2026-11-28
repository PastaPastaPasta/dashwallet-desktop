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
//!
//! [`ExclusionReason::ForkKeptLock`]: inputs a fork gave back while a verified
//! InstantSend lock still holds them, or while a lock may have been evicted
//! from the session's buffer ([`fork_kept_inputs`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use dashcore::{Address, OutPoint, Txid};
use key_wallet::account::AccountType;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::wallet::balance::WalletCoreBalance;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;

use crate::WalletId;
use crate::events::SessionHub;
use crate::history::{EarlyInstantLocks, TxEntry, WalletHistory, instant_locked};

/// Why a coin is kept out of spending and every balance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExclusionReason {
    /// An output of a payment that may never confirm, or of a transaction
    /// built on one ([`pending_unproven`]): shown as pending.
    PendingUnproven,
    /// A coin an abandon or a drop gave back with no evidence that it is
    /// unspent, waiting for that rewind's rescan (`crate::rescan_hold`):
    /// not listed at all.
    RescanHold,
    /// A coin a fork gave back that may still be spent by an InstantSend
    /// payment. Left out of the coin list.
    ForkKeptLock,
}

impl ExclusionReason {
    /// Whether the coin is still listed (with its reason) rather than
    /// left out of the coin list altogether.
    pub(crate) fn listed(self) -> bool {
        match self {
            Self::PendingUnproven => true,
            Self::RescanHold | Self::ForkKeptLock => false,
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
    /// The value of those shown as pending ([`ExclusionReason::listed`]):
    /// a coin waiting for its rescan is kept out without being shown.
    pub pending: u64,
    /// Each coin's share, with its account and why it is excluded: a coin
    /// released before the next read leaves at once ([`Self::release_rescan`]).
    pub coins: BTreeMap<OutPoint, (AccountType, WalletCoreBalance, ExclusionReason)>,
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
            let t = account.managed_account_type().to_account_type();
            for u in account.utxos.values() {
                let Some(&reason) = coins.get(&u.outpoint) else {
                    continue;
                };
                let value = u.txout.value;
                if reason.listed() {
                    out.pending += value;
                }
                let share = if u.is_locked {
                    WalletCoreBalance::new(0, 0, 0, value)
                } else if !u.is_mature(height) {
                    WalletCoreBalance::new(0, 0, value, 0)
                } else if u.is_confirmed || u.is_instantlocked || u.is_trusted {
                    WalletCoreBalance::new(value, 0, 0, 0)
                } else {
                    WalletCoreBalance::new(0, value, 0, 0)
                };
                if share.total() > 0 {
                    out.balance += share;
                    *out.accounts.entry(t).or_default() += share;
                    out.coins.insert(u.outpoint, (t, share, reason));
                }
            }
        }
        out
    }

    pub(crate) fn rescan_share(&self, o: &OutPoint) -> Option<WalletCoreBalance> {
        self.coins.get(o).and_then(|(_, share, reason)| {
            (*reason == ExclusionReason::RescanHold).then_some(*share)
        })
    }

    /// Releases settled rescan holds. A later read may already have
    /// recorded another reason for excluding a coin.
    pub(crate) fn release_rescan(&mut self, gone: &BTreeSet<OutPoint>) {
        for o in gone {
            let std::collections::btree_map::Entry::Occupied(entry) = self.coins.entry(*o) else {
                continue;
            };
            if entry.get().2 != ExclusionReason::RescanHold {
                continue;
            }
            let (t, share, _) = entry.remove();
            self.balance = Self::subtract(&self.balance, Some(&share));
            if let Some(a) = self.accounts.get_mut(&t) {
                *a = Self::subtract(a, Some(&share));
            }
        }
    }

    /// The coins kept out while they wait for their rescan.
    pub(crate) fn waiting(&self) -> BTreeSet<OutPoint> {
        (self.coins.iter())
            .filter(|(_, (_, _, r))| *r == ExclusionReason::RescanHold)
            .map(|(o, _)| *o)
            .collect()
    }

    /// The excluded coins' value.
    pub(crate) fn total(&self) -> u64 {
        self.balance.total()
    }

    /// `balance` without `share` (`None`: nothing), bucket by bucket. What
    /// a bucket can't cover comes out of the others (locked, unconfirmed,
    /// confirmed, immature): key-wallet's cached buckets may predate
    /// a flag the share was read with (a funding's temporary lock), and the
    /// excluded coins are counted in none of them either way.
    pub(crate) fn subtract(
        balance: &WalletCoreBalance,
        share: Option<&WalletCoreBalance>,
    ) -> WalletCoreBalance {
        let Some(s) = share else {
            return *balance;
        };
        let mut left = [
            balance.locked(),
            balance.unconfirmed(),
            balance.confirmed(),
            balance.immature(),
        ];
        let want = [s.locked(), s.unconfirmed(), s.confirmed(), s.immature()];
        let mut owed = 0u64;
        for (b, w) in left.iter_mut().zip(want) {
            let taken = w.min(*b);
            *b -= taken;
            owed += w - taken;
        }
        for b in &mut left {
            let taken = owed.min(*b);
            *b -= taken;
            owed -= taken;
        }
        let [locked, unconfirmed, confirmed, immature] = left;
        WalletCoreBalance::new(confirmed, unconfirmed, immature, locked)
    }
}

impl SessionHub {
    /// Wallet `wallet`'s excluded coins as `core` (read under the manager's
    /// lock) and the history hold them now. Every surface that counts or
    /// offers coins reads this map and nothing else.
    pub(crate) fn excluded_coins(&self, wallet: WalletId, core: &ManagedWalletInfo) -> Exclusions {
        self.exclusions(wallet, core, false)
    }

    pub(crate) fn recovery_exclusions(
        &self,
        wallet: WalletId,
        core: &ManagedWalletInfo,
    ) -> Exclusions {
        self.detect_truncation(wallet, core);
        self.exclusions(wallet, core, true)
    }

    fn exclusions(
        &self,
        wallet: WalletId,
        core: &ManagedWalletInfo,
        check_mined_locks: bool,
    ) -> Exclusions {
        #[cfg(test)]
        let skip = (self.skip_withhold).load(std::sync::atomic::Ordering::SeqCst);
        #[cfg(not(test))]
        let skip = false;
        let (withheld, kept) = (self.history).read_wallet_with_locks(&wallet, |h, locks| {
            let withheld = if skip {
                BTreeSet::new()
            } else {
                pending_unproven(core, h)
            };
            (
                withheld,
                fork_kept_inputs(core, h, check_mined_locks.then_some(locks)),
            )
        });
        let mut out: Exclusions = (withheld.into_iter())
            .map(|o| (o, ExclusionReason::PendingUnproven))
            .collect();
        out.extend(
            (self.rescan_held(&wallet).into_iter()).map(|o| (o, ExclusionReason::RescanHold)),
        );
        out.extend(kept.into_iter().map(|o| (o, ExclusionReason::ForkKeptLock)));
        out
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
                !h.rescan_holds.is_empty()
                    || !h.fork_kept_locks.is_empty()
                    || !h.fork_kept_ancestor_inputs.is_empty()
                    || !h.uncertain_fork_spends.is_empty()
                    || (h.txs.iter())
                        .any(|(t, e)| withholding_candidate(e) && spends_known_coin(h, t, e))
            })
    }
}

/// Unmined, neither abandoned nor InstantSend-locked as the history holds
/// it.
fn withholding_candidate(e: &TxEntry) -> bool {
    e.context.block_info().is_none() && !e.abandoned && !instant_locked(e)
}

/// Whether the history holds `o`, input `n` of `t`, as the wallet's: an
/// own input of `e`, or claimed or marked by `t`.
fn history_owns(h: &WalletHistory, t: &Txid, n: u32, o: &OutPoint, e: &TxEntry) -> bool {
    e.own_inputs.contains_key(&n)
        || h.claimed.get(o) == Some(t)
        || h.marked.get(t).is_some_and(|m| m.contains(o))
}

/// Whether `e` spends a coin the history alone knows as the wallet's,
/// as [`pending_unproven`]'s ownership test can find it without key-wallet.
fn spends_known_coin(h: &WalletHistory, t: &Txid, e: &TxEntry) -> bool {
    (e.tx.input.iter().zip(0u32..)).any(|(i, n)| {
        let o = &i.previous_output;
        history_owns(h, t, n, o, e) || h.txs.contains_key(&o.txid)
    })
}

/// The coins of `core` that a payment of [`WalletHistory::fork_kept_locks`]
/// spends, which key-wallet holds unspent: a fork gave them back when it
/// removed that payment. Held while the history keeps the payment out of a
/// block and key-wallet holds no record of it. Its block, or its record seen
/// again, ends the hold: key-wallet then spends them itself. So does a
/// winner's block that spends them (key-wallet no longer holds them), an
/// abandon the ChainLock exception lets through, and the end of the session.
/// Also includes restored inputs from removed ancestors and from spends
/// whose lock may have been evicted. With `recovery`'s lock buffer, it also
/// holds what a truncation the history has not applied yet would: coins a
/// buffered verified lock spends whose spender key-wallet does not record
/// (even one the history abandoned: the lock outranks it),
/// inputs of a session lock still mined in the history and, once the buffer
/// has evicted evidence, inputs of any mined spender key-wallet does not
/// record.
fn fork_kept_inputs(
    core: &ManagedWalletInfo,
    h: &WalletHistory,
    recovery: Option<&EarlyInstantLocks>,
) -> BTreeSet<OutPoint> {
    if h.fork_kept_locks.is_empty()
        && h.fork_kept_ancestor_inputs.is_empty()
        && h.uncertain_fork_spends.is_empty()
        && recovery.is_none_or(|l| h.session_locks.is_empty() && l.spends.is_empty() && !l.overflow)
    {
        return BTreeSet::new();
    }
    let accounts = core.accounts.all_accounts();
    let recorded = |t: &Txid| {
        accounts.iter().any(|a| {
            !a.managed_account_type()
                .to_account_type()
                .is_contact_owned()
                && a.has_transaction(t)
        })
    };
    let funding = core.accounts.all_funding_accounts();
    let unspent = |o: &OutPoint| funding.iter().any(|a| a.utxos.contains_key(o));
    let live = |e: &TxEntry, t: &Txid, mined: bool| {
        !e.is_abandoned() && e.context.block_info().is_some() == mined && !recorded(t)
    };
    let inputs = |e: &TxEntry| e.tx.input.iter().map(|i| i.previous_output).collect();
    let kept = (h.fork_kept_locks.iter())
        .filter_map(|t| h.txs.get(t).filter(|e| live(e, t, false)).map(inputs));
    // A wallet that has advanced past the fork can have a lower history
    // checkpoint while the history still holds a verified lock as mined,
    // or holds no record of its spender yet.
    let unreconciled = recovery.into_iter().flat_map(|locks| {
        let session = (h.session_locks.iter())
            .filter_map(|t| h.txs.get(t).filter(|e| live(e, t, true)).map(inputs));
        let evicted = (h.txs.iter())
            .filter(|_| locks.overflow)
            .filter(|(t, e)| live(e, t, true))
            .map(|(_, e)| inputs(e));
        session.chain(evicted)
    });
    let buffered = recovery.into_iter().flat_map(|locks| {
        let spent = move |o: &&OutPoint| {
            locks
                .spends
                .get(*o)
                .is_some_and(|by| by.iter().any(|t| !recorded(t)))
        };
        (funding.iter())
            .flat_map(|a| a.utxos.keys())
            .filter(spent)
            .copied()
            .collect::<Vec<_>>()
    });
    kept.chain(unreconciled)
        .flat_map(|i: Vec<OutPoint>| i)
        .chain(buffered)
        .chain(h.fork_kept_ancestor_inputs.values().flatten().copied())
        .chain(h.uncertain_fork_spends.values().flatten().copied())
        .filter(unspent)
        .collect()
}

/// E1 (Q100): the coins of `core` withheld because a payment they descend
/// from may never confirm. A root is a payment of the history that is
/// unmined, not abandoned, not InstantSend-locked (by the history or by
/// key-wallet's locks), that key-wallet holds no record of, and that
/// spends a coin of the wallet which nothing else holds spent: not in
/// key-wallet's observed spends, spent by no other transaction of the
/// history that is not abandoned, and not an unspent coin of key-wallet's
/// (only the payment's own claim holds it). A claim the store's repair put
/// back pending is no such other spender: a peer's spend it could not
/// judge proves nothing about the coin. When the load could not read
/// those claims, no other spender counts. The root's outputs that
/// key-wallet holds unspent are withheld, and so is every output of an
/// unmined transaction built on a withheld one that is neither abandoned
/// nor InstantSend-locked (a lock on it proves its inputs).
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
    let proves = |t: &Txid| !h.put_back_unread && !h.put_back.contains(t);
    let mut spenders: HashMap<OutPoint, Vec<Txid>> = HashMap::new();
    for (t, e) in txs.iter().filter(|(t, e)| !e.is_abandoned() && proves(t)) {
        for i in &e.tx.input {
            spenders.entry(i.previous_output).or_default().push(*t);
        }
    }
    let owns = |o: &OutPoint, t: &Txid, index: u32, e: &TxEntry| {
        history_owns(h, t, index, o, e)
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
        if withholding_candidate(e) && !locks.contains(t) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn b(confirmed: u64, unconfirmed: u64, immature: u64, locked: u64) -> WalletCoreBalance {
        WalletCoreBalance::new(confirmed, unconfirmed, immature, locked)
    }

    #[test]
    fn a_share_comes_out_of_its_own_buckets() {
        let got = Excluded::subtract(&b(10, 7, 3, 2), Some(&b(4, 5, 0, 2)));
        assert_eq!(got, b(6, 2, 3, 0));
    }

    #[test]
    fn a_coin_read_locked_while_its_bucket_counts_it_unlocked_is_still_taken_out() {
        let got = Excluded::subtract(&b(10, 5, 0, 0), Some(&b(0, 0, 0, 5)));
        assert_eq!(got.total(), 10);
        assert_eq!(got, b(10, 0, 0, 0));
    }

    #[test]
    fn a_coin_read_unlocked_while_its_bucket_counts_it_locked_is_still_taken_out() {
        let got = Excluded::subtract(&b(10, 0, 0, 5), Some(&b(0, 5, 0, 0)));
        assert_eq!(got, b(10, 0, 0, 0));
    }

    #[test]
    fn nothing_goes_below_zero() {
        let got = Excluded::subtract(&b(1, 1, 0, 0), Some(&b(5, 5, 0, 0)));
        assert_eq!(got, b(0, 0, 0, 0));
    }
}
