//! Coins the wallet loader parked in the wrong funds account (0.1 BAL fix).
//!
//! When it loads a wallet, platform-wallet-storage puts every unspent coin
//! whose address has no `core_address_pool` row into the first funds account
//! (BIP44: `rehydrate.rs:347-353`, `route_to_funds_account` `:429`). It only
//! persists the pool rows of a pool snapshot, never the addresses SPV derives
//! past them (`core_state.rs:113`). So a wallet that mixed past CoinJoin
//! index 99 loads its later mixed coins into BIP44. There:
//! - a send or shield that selects one is refused, because no BIP44 pool can
//!   give its key ("no derivation path for selected input address");
//! - a replay of the coin's block (the DIP-15 rescan, a user rescan, an
//!   abandon's rescan, a crash catch-up) inserts it again into the CoinJoin
//!   account, which dedups only within itself
//!   (`managed_core_funds_account.rs:420`). `update_balance` sums the
//!   accounts without deduplicating (`wallet_info_interface.rs:508`), so the
//!   coin counts twice.
//!
//! [`rehome_parked`] runs at load, before SPV can replay anything. It finds the
//! funds account that derives each parked coin's address, extends that
//! account's pool to it (from the account's own xpub, at most
//! [`REHOME_SEARCH`] indexes past what it holds or would generate, never
//! past [`REHOME_MAX_INDEX`]), and moves the coin there, marking its address
//! used.
//! [`reconcile_duplicates`] is the safety net after block events: one copy
//! per outpoint, in the account that owns the address. Neither persists
//! anything (`core_utxos` has no account column).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dashcore::{Address, OutPoint, ScriptBuf};
use key_wallet::account::AccountType;
use key_wallet::managed_account::address_pool::{AddressPoolType, KeySource};
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::wallet::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::{ChildNumber, Wallet};

use crate::WalletId;
use crate::session::{Manager, NetworkSession};

/// The re-home's search window: how many indexes past the furthest one a
/// pool holds or would generate keeping its gap (or past the gap refill of
/// the last parked address it found there) it derives looking for a parked
/// coin's address. It is key-wallet's largest gap limit
/// (`gap_limit::MAX_GAP_LIMIT`, 1000, also the engine's `MAX_LOOKAHEAD`): a
/// scan never derives more than a gap limit past the last used address, so
/// any coin the scan itself found lies within one window of a used address.
/// The one miss is a used address whose coins were all spent and whose pool
/// row was never stored, more than a window before a parked coin; that coin
/// stays (and is logged). The test bar's wallet needed about 70 past the
/// CoinJoin window (index ~170 against 0..99).
pub(crate) const REHOME_SEARCH: u32 = key_wallet::gap_limit::MAX_GAP_LIMIT;

/// The re-home's hard budget: no pool is searched, derived or extended past
/// this child index. It is the pinned rehydrator's own discovery ceiling
/// (platform-wallet-storage `rehydrate.rs:479`
/// `MAX_REHYDRATION_DERIVATION_INDEX`, checked at `:609`). An absolute index,
/// not a distance from the pool's loaded end, so a second load searches no
/// further than the first. Per pool this bounds the derivations to
/// `REHOME_MAX_INDEX + 1`, and the addresses generated, the refill past a
/// used address included, to the index itself: past it lie addresses no
/// search covered, and holding them would let the next load claim a coin
/// this one left. Coins past it stay where they were loaded, counted once,
/// and are logged.
pub(crate) const REHOME_MAX_INDEX: u32 = 10_000;

/// What [`rehome_parked`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Rehomed {
    /// Coins moved to the account that derives their address.
    pub moved: usize,
    pub moved_duffs: u64,
    /// Parked copies dropped because the owning account already held the
    /// outpoint.
    pub dropped: usize,
    /// Coins removed from the funds accounts because their address is a
    /// DashPay contact's (`AccountType::is_contact_owned`): not the wallet's.
    pub evicted: usize,
    pub evicted_duffs: u64,
    /// Coins found in the account that derives their address once its pool
    /// was extended: they stay, their address marked used.
    pub marked: usize,
    /// Coins that stay where they are: no funds account derives their address
    /// within [`REHOME_SEARCH`] and [`REHOME_MAX_INDEX`] (counted once).
    pub stranded: usize,
    pub stranded_duffs: u64,
    /// [`REHOME_MAX_INDEX`] ended a pool's search while coins were still
    /// unresolved (they may lie past it, or belong to no pool).
    pub capped: bool,
}

impl Rehomed {
    pub(crate) fn changed(&self) -> bool {
        self.moved + self.dropped + self.evicted + self.marked > 0
    }
}

/// Moves each coin a funds account holds but cannot derive to the funds
/// account that derives its address, extending that account's address pool
/// up to the address (see [`REHOME_SEARCH`] for `search`). Removes those
/// whose address a DashPay contact's account holds. Coins no account derives
/// stay. Recomputes the balance.
pub(crate) fn rehome_parked(wallet: &Wallet, info: &mut ManagedWalletInfo, search: u32) -> Rehomed {
    let accounts = funds_accounts(info);
    // The coins each account holds without deriving their address.
    let mut parked: Vec<(AccountType, OutPoint)> = Vec::new();
    for account in &accounts {
        let Some(a) = info.accounts.funds_account(account) else {
            continue;
        };
        let owned = a.managed_account_type();
        parked.extend(
            (a.utxos.values())
                .filter(|u| !owned.contains_address(&u.address))
                .map(|u| (*account, u.outpoint)),
        );
    }
    let mut done = Rehomed::default();
    if parked.is_empty() {
        return done;
    }
    // Coins whose address a pool already holds (a stored lookahead, the
    // loader's refill) are used before the search, so it starts past their
    // gap, as a later load's would.
    for (holder, outpoint) in &parked {
        let address = &info.accounts.funds_account(holder).expect("listed").utxos[outpoint].address;
        if let Some(home) = owner(info, &accounts, address) {
            let address = address.clone();
            mark(info, &home, &address);
        }
    }
    let mut wanted = searched_for(info, &accounts, &parked);
    for account in &accounts {
        if wanted.is_empty() {
            break;
        }
        done.capped |= extend_to(wallet, info, account, &mut wanted, search);
    }
    // Every coin is placed against the pools as the search left them: the
    // loop only marks addresses used, and the gap refill waits until after
    // it, so no refill makes a later coin's address (past the search) owned.
    let mut touched: Vec<AccountType> = Vec::new();
    for (holder, outpoint) in parked {
        let utxo = info.accounts.funds_account(&holder).expect("listed").utxos[&outpoint].clone();
        let Some(home) = owner(info, &accounts, &utxo.address) else {
            if contact_owned(info, &utxo.address) {
                info.accounts
                    .funds_account_mut(&holder)
                    .expect("listed")
                    .utxos
                    .remove(&outpoint);
                done.evicted += 1;
                done.evicted_duffs += utxo.value();
                continue;
            }
            done.stranded += 1;
            done.stranded_duffs += utxo.value();
            continue;
        };
        if home == holder {
            // Its own pool now holds it: it was only underived.
            mark(info, &home, &utxo.address);
            touched.push(home);
            done.marked += 1;
            continue;
        }
        info.accounts
            .funds_account_mut(&holder)
            .expect("listed")
            .utxos
            .remove(&outpoint);
        let target = info.accounts.funds_account_mut(&home).expect("listed");
        match target.utxos.entry(outpoint) {
            std::collections::btree_map::Entry::Occupied(_) => done.dropped += 1,
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(utxo.clone());
                done.moved += 1;
                done.moved_duffs += utxo.value();
            }
        }
        mark(info, &home, &utxo.address);
        touched.push(home);
    }
    touched.sort();
    touched.dedup();
    for account in &touched {
        refill(wallet, info, account);
    }
    info.update_balance();
    done
}

/// The addresses of the `parked` coins to look for past the pools' ends:
/// those no funds account holds yet. Not a contact's (those are evicted
/// without a search, which a contact's address would run through every pool
/// to the end of its window).
fn searched_for(
    info: &ManagedWalletInfo,
    accounts: &[AccountType],
    parked: &[(AccountType, OutPoint)],
) -> HashMap<ScriptBuf, Address> {
    let mut wanted = HashMap::new();
    for (holder, outpoint) in parked {
        let utxo = &info.accounts.funds_account(holder).expect("listed").utxos[outpoint];
        if owner(info, accounts, &utxo.address).is_none() && !contact_owned(info, &utxo.address) {
            wanted.insert(utxo.txout.script_pubkey.clone(), utxo.address.clone());
        }
    }
    wanted
}

/// Keeps one copy of every outpoint that two or more funds accounts hold: the
/// one in the account whose pools contain its address. If none does, the
/// copy in the first of them in key-wallet's funds order (BIP44, BIP32,
/// CoinJoin, DashPay receiving), logged. Never removes an outpoint from its
/// owning account, nor one only one account holds. Returns the copies
/// dropped and their value, and recomputes the balance when it dropped any.
///
/// In-flight sends: a dropped copy sits in an account whose pools lack its
/// address, so no build can sign it (platform-wallet refuses it before
/// signing, `transaction.rs:817-829`) and no transaction in flight spends
/// it. The owner's copy, the only signable one, is never dropped, so a
/// reservation on it (reservations are per account, per outpoint) stays
/// valid through a reconcile between a send's selection and its broadcast.
pub(crate) fn reconcile_duplicates(info: &mut ManagedWalletInfo) -> (usize, u64) {
    let accounts = funds_accounts(info);
    let mut held: BTreeMap<OutPoint, Vec<AccountType>> = BTreeMap::new();
    for account in &accounts {
        for outpoint in info
            .accounts
            .funds_account(account)
            .expect("listed")
            .utxos
            .keys()
        {
            held.entry(*outpoint).or_default().push(*account);
        }
    }
    let (mut dropped, mut duffs) = (0, 0);
    for (outpoint, holders) in held.into_iter().filter(|(_, h)| h.len() > 1) {
        let address = info
            .accounts
            .funds_account(&holders[0])
            .expect("listed")
            .utxos[&outpoint]
            .address
            .clone();
        let keep = match holders.iter().find(|a| derives(info, a, &address)) {
            Some(owner) => *owner,
            None => {
                tracing::warn!(
                    %outpoint,
                    holders = holders.len(),
                    kept = ?holders[0],
                    "no account holding a duplicated coin derives it: kept the first copy"
                );
                holders[0]
            }
        };
        for account in holders.into_iter().filter(|a| *a != keep) {
            let removed = info
                .accounts
                .funds_account_mut(&account)
                .expect("listed")
                .utxos
                .remove(&outpoint);
            if let Some(u) = removed {
                dropped += 1;
                duffs += u.value();
            }
        }
    }
    if dropped > 0 {
        info.update_balance();
    }
    (dropped, duffs)
}

/// Whether two or more funds accounts hold one outpoint. Funds accounts
/// derive from disjoint keys, so at least one copy of a duplicate sits in an
/// account that does not derive its address: only those coins are looked
/// up. Allocates nothing past the account list.
fn has_duplicates(info: &ManagedWalletInfo) -> bool {
    let accounts = funds_accounts(info);
    accounts.iter().any(|holder| {
        let a = info.accounts.funds_account(holder).expect("listed");
        let owned = a.managed_account_type();
        (a.utxos.values())
            .filter(|u| !owned.contains_address(&u.address))
            .any(|u| {
                (accounts.iter().filter(|other| *other != holder)).any(|other| {
                    (info.accounts.funds_account(other))
                        .is_some_and(|o| o.utxos.contains_key(&u.outpoint))
                })
            })
    })
}

/// The wallet's funds accounts, in key-wallet's order (BIP44, BIP32,
/// CoinJoin, DashPay receiving).
fn funds_accounts(info: &ManagedWalletInfo) -> Vec<AccountType> {
    (info.accounts.all_funding_accounts().into_iter())
        .map(|a| a.managed_account_type().to_account_type())
        .collect()
}

fn derives(info: &ManagedWalletInfo, account: &AccountType, address: &Address) -> bool {
    (info.accounts.funds_account(account))
        .is_some_and(|a| a.managed_account_type().contains_address(address))
}

/// Whether a DashPay contact's account (watched, never ours) holds `address`.
fn contact_owned(info: &ManagedWalletInfo, address: &Address) -> bool {
    (info.accounts.dashpay_external_accounts.values())
        .any(|a| a.managed_account_type().contains_address(address))
}

/// The funds account whose pools hold `address`.
fn owner(
    info: &ManagedWalletInfo,
    accounts: &[AccountType],
    address: &Address,
) -> Option<AccountType> {
    accounts.iter().find(|a| derives(info, a, address)).copied()
}

/// Looks for the `wanted` scripts in each address pool of `account`, deriving
/// from the account's xpub (without storing) a window of `search` indexes,
/// or the pool's gap limit if larger, past the furthest index the pool holds
/// or would generate keeping its gap, and past the gap refill of each wanted
/// address found, never past [`REHOME_MAX_INDEX`]. Then extends the pool up
/// to the furthest one found. Found scripts leave `wanted`. True when
/// [`REHOME_MAX_INDEX`] cut a pool's search short.
fn extend_to(
    wallet: &Wallet,
    info: &mut ManagedWalletInfo,
    account: &AccountType,
    wanted: &mut HashMap<ScriptBuf, Address>,
    search: u32,
) -> bool {
    let key_source = wallet.key_source_for_account(*account);
    let KeySource::Public(xpub) = key_source else {
        return false;
    };
    let Some(a) = info.accounts.funds_account_mut(account) else {
        return false;
    };
    let (mut extended, mut capped) = (false, false);
    for pool in a.managed_account_type_mut().address_pools_mut() {
        if wanted.is_empty() {
            break;
        }
        let branch = match pool.pool_type {
            AddressPoolType::External => xpub.derive_pub(&[ChildNumber::Normal { index: 0 }]),
            AddressPoolType::Internal => xpub.derive_pub(&[ChildNumber::Normal { index: 1 }]),
            AddressPoolType::Absent => Ok(xpub),
            AddressPoolType::AbsentHardened => continue,
        };
        let Ok(branch) = branch else {
            continue;
        };
        let gap = pool.gap_limit;
        // Never less than the pool's gap: marking a found address used then
        // generates no address the search skipped.
        let window = search.max(gap);
        let start = pool.highest_generated.map_or(0, |h| h + 1);
        // The furthest index keeping the gap past the pool's used addresses
        // generates (`maintain_gap_limit`: highest used + gap), so where a
        // later load's search would start. Searching a window past it, and
        // past the gap refill of each address found, leaves a later load
        // nothing more to find (a fixed point).
        let mut reach = (pool.highest_generated)
            .max(pool.highest_used.map(|u| u.saturating_add(gap)))
            .unwrap_or(gap.saturating_sub(1));
        // The hard budget: nothing past REHOME_MAX_INDEX, checked before
        // any derivation or allocation.
        let ceiling = REHOME_MAX_INDEX.saturating_add(1);
        let horizon = |reach: u32| reach.saturating_add(1).saturating_add(window).min(ceiling);
        let (mut furthest, mut end) = (None, horizon(reach));
        let mut index = start;
        // As a gap-limit scan: each address found opens another window.
        while index < end && !wanted.is_empty() {
            let Ok(child) = branch.derive_pub(&[ChildNumber::Normal { index }]) else {
                break;
            };
            let address = Address::p2pkh(&dashcore::PublicKey::new(child.public_key), pool.network);
            if wanted.remove(&address.script_pubkey()).is_some() {
                furthest = Some(index);
                reach = reach.max(index.saturating_add(gap));
                end = horizon(reach);
            }
            index += 1;
        }
        capped |= end == ceiling && index >= end && !wanted.is_empty();
        if let Some(furthest) = furthest {
            if let Err(e) = pool.generate_addresses(furthest + 1 - start, &key_source, true) {
                tracing::warn!(?account, error = %e, "could not extend a pool to a parked coin");
                continue;
            }
            extended = true;
        }
    }
    if extended {
        a.bump_monitor_revision();
    }
    capped
}

/// Marks `address` used in `account`'s pools (no address is generated).
fn mark(info: &mut ManagedWalletInfo, account: &AccountType, address: &Address) {
    if let Some(a) = info.accounts.funds_account_mut(account) {
        a.managed_account_type_mut().mark_address_used(address);
    }
}

/// Keeps the gap past the highest used address of each of `account`'s
/// pools, as `maintain_gap_limit` does, but generates nothing past
/// [`REHOME_MAX_INDEX`] (checked before allocating).
fn refill(wallet: &Wallet, info: &mut ManagedWalletInfo, account: &AccountType) {
    let key_source = wallet.key_source_for_account(*account);
    let Some(a) = info.accounts.funds_account_mut(account) else {
        return;
    };
    if !matches!(key_source, KeySource::NoKeySource) {
        for pool in a.managed_account_type_mut().address_pools_mut() {
            let Some(used) = pool.highest_used else {
                continue;
            };
            let target = used.saturating_add(pool.gap_limit).min(REHOME_MAX_INDEX);
            let next = pool.highest_generated.map_or(0, |h| h + 1);
            if target < next {
                continue;
            }
            if let Err(e) = pool.generate_addresses(target + 1 - next, &key_source, true) {
                tracing::warn!(?account, error = %e, "could not keep a pool's gap past a re-homed coin");
            }
        }
    }
    a.bump_monitor_revision();
}

impl NetworkSession {
    /// Re-homes wallet `id`'s parked coins ([`rehome_parked`]). Runs after
    /// the wallet's stored state is loaded and before SPV can replay a block.
    /// Returns what it did.
    pub(crate) async fn rehome_parked_coins(&self, manager: &Manager, id: WalletId) -> Rehomed {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some((wallet, info)) = wm.get_wallet_and_info_mut(&id.0) else {
            return Rehomed::default();
        };
        let done = rehome_parked(wallet, &mut info.core_wallet, REHOME_SEARCH);
        if done.changed() {
            tracing::info!(
                wallet_id = %id,
                moved = done.moved,
                moved_duffs = done.moved_duffs,
                dropped = done.dropped,
                evicted = done.evicted,
                evicted_duffs = done.evicted_duffs,
                marked = done.marked,
                "re-homed coins loaded into an account that does not derive their address"
            );
        }
        if done.stranded > 0 {
            tracing::warn!(
                wallet_id = %id,
                coins = done.stranded,
                duffs = done.stranded_duffs,
                search = REHOME_SEARCH,
                max_index = REHOME_MAX_INDEX,
                capped = done.capped,
                "coins whose address no funds account derives within the search bounds stay where they were loaded"
            );
        }
        done
    }

    /// Drops wallet `id`'s duplicate coins ([`reconcile_duplicates`]) after
    /// its block events, then publishes key-wallet's balance: it also undoes
    /// a doubled balance an event computed before an earlier reconcile.
    /// Looks for duplicates under the read lock first, and takes the write
    /// lock only when there are some (rare: a replay of a parked coin).
    pub(crate) async fn reconcile_parked_coins(&self, id: WalletId) {
        let Ok(manager) = self.manager() else {
            self.hub.reconcile_running(id);
            return;
        };
        let wm = manager.wallet_manager_arc();
        {
            let wm = wm.read().await;
            // Only now: events apply under the write lock, so one applied
            // while this waited is covered by this run, and one applied
            // after it starts the next.
            self.hub.reconcile_running(id);
            let Some(info) = wm.get_wallet_info(&id.0) else {
                return;
            };
            let core = &info.core_wallet;
            if !has_duplicates(core) {
                self.hub
                    .publish_core_balance(id, &core.balance(), &core.account_balances());
                return;
            }
        }
        let mut wm = wm.write().await;
        let Some(info) = wm.get_wallet_info_mut(&id.0) else {
            return;
        };
        let core = &mut info.core_wallet;
        let (dropped, duffs) = reconcile_duplicates(core);
        if dropped > 0 {
            tracing::warn!(wallet_id = %id, dropped, duffs, "dropped coins two funds accounts held");
        }
        // An event computed before this took the lock but applied after it
        // starts another reconcile (the pending mark is cleared above).
        self.hub
            .publish_core_balance(id, &core.balance(), &core.account_balances());
    }

    /// Hooks [`Self::reconcile_parked_coins`] to the hub's block events.
    pub(crate) fn attach_reconcile(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.attach_reconcile(Box::new(move |id| {
            let weak = weak.clone();
            rt.spawn(async move {
                if let Some(session) = weak.upgrade() {
                    session.reconcile_parked_coins(id).await;
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::{BlockHash, Network, Transaction, TxIn, TxOut, Txid};
    use key_wallet::Utxo;
    use key_wallet::transaction_checking::{
        BlockInfo, TransactionContext, WalletTransactionChecker,
    };
    use key_wallet::wallet::initialization::WalletAccountCreationOptions;
    use key_wallet::wallet::managed_wallet_info::ManagedAccountOperations;
    use key_wallet::wallet::managed_wallet_info::asset_lock_builder::{
        AssetLockError, AssetLockFundingType, AssetLockResult, CreditOutputFunding,
    };
    use key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy;
    use key_wallet::wallet::managed_wallet_info::fee::FeeRate;
    use key_wallet::wallet::managed_wallet_info::transaction_builder::BuilderError;
    use key_wallet::wallet::managed_wallet_info::transaction_building::AccountTypePreference;

    use super::*;

    /// A 1.00001 DASH CoinJoin denomination.
    const VALUE: u64 = 100_001_000;
    const HEIGHT: u32 = 1_583_940;

    fn cj() -> AccountType {
        AccountType::CoinJoin { index: 0 }
    }

    fn bip44() -> AccountType {
        AccountType::Standard {
            index: 0,
            standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
        }
    }

    /// A fresh wallet, as registered: the CoinJoin pool holds its first
    /// window (0..99) only.
    fn wallet() -> (Wallet, ManagedWalletInfo) {
        let wallet = Wallet::new_random(Network::Testnet, WalletAccountCreationOptions::Default)
            .expect("wallet");
        let info = ManagedWalletInfo::from_wallet(&wallet, 0);
        (wallet, info)
    }

    /// The CoinJoin account's external address at `index`, derived on a copy
    /// of its pool (the wallet's own pool is left as loaded).
    fn coinjoin_address(wallet: &Wallet, info: &ManagedWalletInfo, index: u32) -> Address {
        let ks = wallet.key_source_for_account(cj());
        let mut pool = info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0]
            .clone();
        let have = pool.highest_generated.unwrap_or(0);
        if index > have {
            pool.generate_addresses(index - have, &ks, true).unwrap();
        }
        pool.address_at_index(index).expect("derived")
    }

    fn pays(n: u8, to: &Address) -> Transaction {
        Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([n; 32]), 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: VALUE,
                script_pubkey: to.script_pubkey(),
            }],
            special_transaction_payload: None,
        }
    }

    /// What rehydrate.rs:347-353 does with a coin whose address has no pool
    /// row: it goes into the first funds account, BIP44.
    fn park(info: &mut ManagedWalletInfo, tx: &Transaction, to: &Address) -> OutPoint {
        let outpoint = OutPoint::new(tx.txid(), 0);
        let mut utxo = Utxo::new(outpoint, tx.output[0].clone(), to.clone(), HEIGHT, false);
        utxo.is_confirmed = true;
        info.accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .utxos
            .insert(outpoint, utxo);
        info.update_last_processed_height(HEIGHT + 300);
        info.update_balance();
        outpoint
    }

    /// The replay of the coin's block: no tx record survived the load, so
    /// the transaction is new.
    async fn replay(wallet: &mut Wallet, info: &mut ManagedWalletInfo, tx: &Transaction) {
        let context = TransactionContext::InChainLockedBlock(BlockInfo::new(
            HEIGHT,
            BlockHash::from_byte_array([1; 32]),
            1_793_598_712,
        ));
        info.check_core_transaction(tx, context, wallet, true, true)
            .await;
        info.update_last_processed_height(HEIGHT + 300);
        info.update_balance();
    }

    fn holders(info: &ManagedWalletInfo, outpoint: &OutPoint) -> Vec<AccountType> {
        (info.accounts.all_funding_accounts().into_iter())
            .filter(|a| a.utxos.contains_key(outpoint))
            .map(|a| a.managed_account_type().to_account_type())
            .collect()
    }

    fn derives(info: &ManagedWalletInfo, account: &AccountType, address: &Address) -> bool {
        (info.accounts.funds_account(account)).is_some_and(|a| {
            a.managed_account_type()
                .get_address_derivation_path(address)
                .is_some()
        })
    }

    /// (a) A mixed coin past the CoinJoin window, parked in BIP44 at load,
    /// goes back to CoinJoin: counted once, and its key is found, so a build
    /// that selects it (a send, or a shield's asset lock) has its path.
    #[tokio::test]
    async fn a_parked_coinjoin_coin_goes_home_at_load() {
        let (wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let tx = pays(1, &to);
        let outpoint = park(&mut info, &tx, &to);
        assert!(!derives(&info, &bip44(), &to) && !derives(&info, &cj(), &to));
        // Parked, BIP44 selects it and has no key for it: a send and a
        // shield's asset lock (BIP44-funded) both fail.
        let sent = send(&wallet, &mut info.clone(), AccountTypePreference::BIP44).await;
        assert!(
            matches!(sent, Err(BuilderError::SigningFailed(_))),
            "{sent:?}"
        );
        let lock = shield(
            &wallet,
            &mut info.clone(),
            AccountTypePreference::BIP44,
            Some(100_000_000),
        )
        .await;
        assert!(
            matches!(
                lock,
                Err(AssetLockError::SigningFailed(_)
                    | AssetLockError::Signer(_)
                    | AssetLockError::Builder(BuilderError::SigningFailed(_)))
            ),
            "{:?}",
            lock.err()
        );

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!(
            done,
            Rehomed {
                moved: 1,
                moved_duffs: VALUE,
                ..Rehomed::default()
            }
        );
        assert_eq!(holders(&info, &outpoint), [cj()]);
        assert!(derives(&info, &cj(), &to), "its key is found");
        assert_eq!(info.balance.total(), VALUE);
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.address_info(&to).is_some_and(|i| i.is_used()),
            "marked used"
        );
        assert!(
            pool.highest_generated.unwrap() >= 170 + pool.gap_limit - 1,
            "the gap is kept past it"
        );
        // The CoinJoin account signs for it (its funding drains)...
        let lock = shield(
            &wallet,
            &mut info.clone(),
            AccountTypePreference::CoinJoin,
            None,
        )
        .await
        .unwrap();
        assert_eq!(inputs(&lock.transaction), [outpoint]);
        // Clones share the accounts' reservations: free the coin again.
        info.accounts
            .funds_account(&cj())
            .unwrap()
            .release_reservation(&lock.transaction);
        // ...and BIP44 no longer offers it: with a real BIP44 coin, a send and
        // a shield's asset lock build from that coin.
        let real = wallet_receive_address(&wallet, &mut info);
        let mut real_tx = pays(9, &real);
        real_tx.output[0].value = 3 * VALUE;
        let real_op = OutPoint::new(real_tx.txid(), 0);
        let mut utxo = Utxo::new(real_op, real_tx.output[0].clone(), real, HEIGHT, false);
        utxo.is_confirmed = true;
        info.accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .utxos
            .insert(real_op, utxo);
        info.update_balance();
        let (sent, _) = send(&wallet, &mut info.clone(), AccountTypePreference::BIP44)
            .await
            .unwrap();
        assert_eq!(inputs(&sent), [real_op]);
        info.accounts
            .funds_account(&bip44())
            .unwrap()
            .release_reservation(&sent);
        let lock = shield(
            &wallet,
            &mut info.clone(),
            AccountTypePreference::BIP44,
            Some(100_000_000),
        )
        .await
        .unwrap();
        assert_eq!(inputs(&lock.transaction), [real_op]);
    }

    fn inputs(tx: &Transaction) -> Vec<OutPoint> {
        tx.input.iter().map(|i| i.previous_output).collect()
    }

    /// Load is idempotent: each load runs the re-home, and a second run on
    /// the same wallet finds nothing to move and changes nothing (the coins
    /// of each account, its pools and used marks, and the balance), whether
    /// coins moved, were evicted or stayed.
    #[test]
    fn a_second_rehome_changes_nothing() {
        let (mut wallet, mut info) = wallet();
        let contact = AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [1; 32],
            friend_identity_id: [2; 32],
        };
        wallet.add_account(contact, None).unwrap();
        info.add_managed_account(&wallet, contact).unwrap();
        let theirs = (info
            .accounts
            .dashpay_external_accounts
            .values()
            .next()
            .unwrap())
        .managed_account_type()
        .address_pools()[0]
            .address_at_index(0)
            .unwrap();
        park(&mut info, &pays(1, &theirs), &theirs);
        let mixed = coinjoin_address(&wallet, &info, 170);
        park(&mut info, &pays(2, &mixed), &mixed);
        let far = coinjoin_address(&wallet, &info, 170 + 2 * REHOME_SEARCH);
        park(&mut info, &pays(3, &far), &far);

        let first = rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        assert_eq!((first.moved, first.evicted, first.stranded), (1, 1, 1));
        let after_first = state(&info);
        let second = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!(
            second,
            Rehomed {
                stranded: 1,
                stranded_duffs: VALUE,
                ..Rehomed::default()
            },
            "only the stranded coin is seen again"
        );
        assert!(!second.changed());
        assert_eq!(state(&info), after_first);
    }

    /// Every funds account's coins and pools (generated and used indexes,
    /// each address's used mark), and the wallet balance.
    fn state(info: &ManagedWalletInfo) -> String {
        let accounts: Vec<String> = (info.accounts.all_funding_accounts().into_iter())
            .map(|a| {
                let pools: Vec<_> = (a.managed_account_type().address_pools().into_iter())
                    .map(|p| {
                        let used: BTreeMap<u32, bool> = p
                            .addresses
                            .values()
                            .map(|i| (i.index, i.is_used()))
                            .collect();
                        (p.highest_generated, p.highest_used, used)
                    })
                    .collect();
                format!(
                    "{:?} {:?} {:?}",
                    a.managed_account_type().to_account_type(),
                    a.utxos,
                    pools
                )
            })
            .collect();
        format!("{accounts:?} {:?}", info.balance)
    }

    /// The reconcile removes only a true duplicate: a coin one account holds
    /// (a CoinJoin coin, a BIP44 coin, or a parked one) is left as it is.
    #[test]
    fn the_reconcile_leaves_single_copies_alone() {
        let (wallet, mut info) = wallet();
        let mixed = coinjoin_address(&wallet, &info, 5);
        let mixed_tx = pays(1, &mixed);
        let mixed_op = OutPoint::new(mixed_tx.txid(), 0);
        let utxo = Utxo::new(mixed_op, mixed_tx.output[0].clone(), mixed, HEIGHT, false);
        info.accounts
            .funds_account_mut(&cj())
            .unwrap()
            .utxos
            .insert(mixed_op, utxo);
        let plain = wallet_receive_address(&wallet, &mut info);
        let plain_tx = pays(2, &plain);
        let plain_op = OutPoint::new(plain_tx.txid(), 0);
        let utxo = Utxo::new(plain_op, plain_tx.output[0].clone(), plain, HEIGHT, false);
        info.accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .utxos
            .insert(plain_op, utxo);
        let far = coinjoin_address(&wallet, &info, 170);
        let parked_op = park(&mut info, &pays(3, &far), &far);
        let before = info.balance.total();
        assert_eq!(before, 3 * VALUE);

        assert_eq!(reconcile_duplicates(&mut info), (0, 0));

        assert_eq!(holders(&info, &mixed_op), [cj()]);
        assert_eq!(holders(&info, &plain_op), [bip44()]);
        assert_eq!(holders(&info, &parked_op), [bip44()]);
        assert_eq!(info.balance.total(), before);
    }

    /// The search bound is [`REHOME_SEARCH`]: a coin at the last index it
    /// covers goes home, one index further stays.
    #[test]
    fn the_rehome_bound_is_rehome_search() {
        let (wallet, loaded) = wallet();
        let pool = &loaded
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.gap_limit < REHOME_SEARCH,
            "the bound, not the pool's gap, decides"
        );
        let have = pool.highest_generated.unwrap();

        let mut info = loaded.clone();
        let last = coinjoin_address(&wallet, &info, have + REHOME_SEARCH);
        let at = park(&mut info, &pays(1, &last), &last);
        assert_eq!(rehome_parked(&wallet, &mut info, REHOME_SEARCH).moved, 1);
        assert_eq!(holders(&info, &at), [cj()]);

        let mut info = loaded;
        let past = coinjoin_address(&wallet, &info, have + REHOME_SEARCH + 1);
        let beyond = park(&mut info, &pays(2, &past), &past);
        assert_eq!(rehome_parked(&wallet, &mut info, REHOME_SEARCH).stranded, 1);
        assert_eq!(holders(&info, &beyond), [bip44()]);
        assert_eq!(info.balance.total(), VALUE);
    }

    /// A parked coin whose address is a DashPay contact's (an output we paid
    /// them) is the contact's: it leaves the funds accounts, uncounted.
    #[test]
    fn a_parked_contact_coin_leaves_the_funds_accounts() {
        let (mut wallet, mut info) = wallet();
        let contact = AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [1; 32],
            friend_identity_id: [2; 32],
        };
        wallet.add_account(contact, None).unwrap();
        info.add_managed_account(&wallet, contact).unwrap();
        let theirs = (info
            .accounts
            .dashpay_external_accounts
            .values()
            .next()
            .unwrap())
        .managed_account_type()
        .address_pools()[0]
            .address_at_index(0)
            .unwrap();
        let outpoint = park(&mut info, &pays(1, &theirs), &theirs);
        assert_eq!(info.balance.total(), VALUE, "parked, it counts");

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!(
            done,
            Rehomed {
                evicted: 1,
                evicted_duffs: VALUE,
                ..Rehomed::default()
            }
        );
        assert!(holders(&info, &outpoint).is_empty());
        assert_eq!(info.balance.total(), 0);
    }

    /// r1 Opus m2: a contact's coin is evicted without a search: its address
    /// never enters the searched set (it would scan every pool to the end of
    /// its window on every load).
    #[test]
    fn a_contacts_coin_is_not_searched_for() {
        let (mut wallet, mut info) = wallet();
        let contact = AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [1; 32],
            friend_identity_id: [2; 32],
        };
        wallet.add_account(contact, None).unwrap();
        info.add_managed_account(&wallet, contact).unwrap();
        let theirs = (info
            .accounts
            .dashpay_external_accounts
            .values()
            .next()
            .unwrap())
        .managed_account_type()
        .address_pools()[0]
            .address_at_index(0)
            .unwrap();
        let mixed = coinjoin_address(&wallet, &info, 170);
        let contact_op = park(&mut info, &pays(1, &theirs), &theirs);
        let mixed_op = park(&mut info, &pays(2, &mixed), &mixed);
        let parked = [(bip44(), contact_op), (bip44(), mixed_op)];

        let wanted = searched_for(&info, &funds_accounts(&info), &parked);

        assert_eq!(
            wanted.into_values().collect::<Vec<_>>(),
            [mixed],
            "only the mixed coin's address is searched for"
        );
    }

    /// r1 Opus m5: the read-lock check sees only true duplicates: never single
    /// copies in CoinJoin or BIP44, nor a parked coin alone.
    #[test]
    fn has_duplicates_sees_only_one_outpoint_in_two_accounts() {
        let (wallet, mut info) = wallet();
        let plain = wallet_receive_address(&wallet, &mut info);
        let mixed = coinjoin_address(&wallet, &info, 5);
        let far = coinjoin_address(&wallet, &info, 170);
        park(&mut info, &pays(1, &plain), &plain);
        let mixed_op = park(&mut info, &pays(2, &mixed), &mixed);
        let coin = info
            .accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .utxos
            .remove(&mixed_op);
        (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(mixed_op, coin.unwrap());
        let far_op = park(&mut info, &pays(3, &far), &far);
        assert!(!has_duplicates(&info), "single copies, one of them parked");

        let copy = info.accounts.funds_account(&bip44()).unwrap().utxos[&far_op].clone();
        (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(far_op, copy);

        assert!(has_duplicates(&info));
        reconcile_duplicates(&mut info);
        assert!(!has_duplicates(&info));
    }

    /// Builds and signs a 1 DASH send from `source`.
    async fn send(
        wallet: &Wallet,
        info: &mut ManagedWalletInfo,
        source: AccountTypePreference,
    ) -> Result<(Transaction, u64), BuilderError> {
        let to = wallet_receive_address(wallet, info);
        info.build_and_sign_transaction(
            wallet,
            &[source],
            0,
            vec![(to.as_unchecked().clone(), 100_000_000)],
            FeeRate::normal(),
            SelectionStrategy::LargestFirst,
        )
        .await
    }

    /// Builds a shielded top-up's asset lock from `source`: `amount`, or all
    /// of it (a drain) when `None`.
    async fn shield(
        wallet: &Wallet,
        info: &mut ManagedWalletInfo,
        source: AccountTypePreference,
        amount: Option<u64>,
    ) -> Result<AssetLockResult, AssetLockError> {
        let drain = amount.is_none();
        let to = wallet_receive_address(wallet, info);
        let credit = CreditOutputFunding {
            output: TxOut {
                value: amount.unwrap_or(0),
                script_pubkey: to.script_pubkey(),
            },
            funding_type: AssetLockFundingType::AssetLockShieldedAddressTopUp,
            identity_index: 0,
        };
        info.build_asset_lock(wallet, &[source], 0, vec![credit], 1000, drain)
            .await
    }

    fn wallet_receive_address(wallet: &Wallet, info: &mut ManagedWalletInfo) -> Address {
        let KeySource::Public(xpub) = wallet.key_source_for_account(bip44()) else {
            unreachable!("a seed wallet's BIP44 account has an xpub")
        };
        (info.accounts.funds_account_mut(&bip44()).unwrap())
            .next_receive_address(Some(&xpub), true)
            .unwrap()
    }

    /// (b) A replay of the parked coin's block counts it once: after the
    /// re-home, the replay's insert dedups within the CoinJoin account.
    #[tokio::test]
    async fn a_replay_after_the_rehome_counts_the_coin_once() {
        let (mut wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let tx = pays(1, &to);
        let outpoint = park(&mut info, &tx, &to);
        rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        replay(&mut wallet, &mut info, &tx).await;
        assert_eq!(holders(&info, &outpoint), [cj()]);
        assert_eq!(info.balance.total(), VALUE);
    }

    /// (b) Without the re-home, the replay counts it twice (the bug), and the
    /// reconcile alone brings it back to once, in CoinJoin.
    #[tokio::test]
    async fn the_reconcile_alone_undoes_a_replays_double_count() {
        let (mut wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let tx = pays(1, &to);
        let outpoint = park(&mut info, &tx, &to);
        // The replay derives the CoinJoin pool up to the coin as it scans,
        // the way SPV's gap extension does. (Real SPV reaches 170 only
        // through used addresses in between; this stands in for them. The
        // reconcile itself needs no extension: it keeps the copy in the
        // account whose pools hold the address.)
        let ks = wallet.key_source_for_account(cj());
        let account = info.accounts.funds_account_mut(&cj()).unwrap();
        let pool = &mut account.managed_account_type_mut().address_pools_mut()[0];
        let have = pool.highest_generated.unwrap();
        pool.generate_addresses(170 - have, &ks, true).unwrap();
        replay(&mut wallet, &mut info, &tx).await;
        assert_eq!(info.balance.total(), 2 * VALUE, "the bug");
        assert_eq!(holders(&info, &outpoint), [bip44(), cj()]);

        assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
        assert_eq!(holders(&info, &outpoint), [cj()]);
        assert_eq!(info.balance.total(), VALUE);
        assert_eq!(reconcile_duplicates(&mut info), (0, 0), "nothing left");
    }

    /// (c) A coin whose address no funds account derives within the bound
    /// stays where it was, counted once; one just inside the bound moves.
    #[tokio::test]
    async fn a_coin_past_the_bound_stays_and_counts_once() {
        let (wallet, mut info) = wallet();
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        let (have, gap) = (pool.highest_generated.unwrap(), pool.gap_limit);
        // A window of one gap: `near` is its last index, `chained` the last of
        // the window past `near`'s gap refill, `far` one past the window past
        // `chained`'s.
        let near = coinjoin_address(&wallet, &info, have + gap);
        let chained = coinjoin_address(&wallet, &info, have + 3 * gap);
        let far = coinjoin_address(&wallet, &info, have + 5 * gap + 1);
        let (near_tx, chained_tx, far_tx) = (pays(1, &near), pays(4, &chained), pays(2, &far));
        let near_op = park(&mut info, &near_tx, &near);
        let chained_op = park(&mut info, &chained_tx, &chained);
        let far_op = park(&mut info, &far_tx, &far);

        let done = rehome_parked(&wallet, &mut info, gap);

        assert_eq!(
            done,
            Rehomed {
                moved: 2,
                moved_duffs: 2 * VALUE,
                stranded: 1,
                stranded_duffs: VALUE,
                ..Rehomed::default()
            }
        );
        assert_eq!(holders(&info, &near_op), [cj()]);
        assert_eq!(holders(&info, &chained_op), [cj()]);
        assert_eq!(
            holders(&info, &far_op),
            [bip44()],
            "never moved to an account that can't derive it"
        );
        assert_eq!(info.balance.total(), 3 * VALUE, "each counted once");
        assert_eq!(reconcile_duplicates(&mut info), (0, 0));
    }

    /// No holder derives a duplicated coin: the reconcile keeps exactly one
    /// copy, the first in funds order (BIP44 before CoinJoin), whatever order
    /// the copies were added in.
    #[test]
    fn the_reconcile_tie_break_keeps_the_first_account_in_funds_order() {
        let foreign = Address::p2pkh(
            &dashcore::PublicKey::from_slice(&[2; 33]).unwrap(),
            Network::Testnet,
        );
        for order in [[cj(), bip44()], [bip44(), cj()]] {
            let (_, mut info) = wallet();
            let tx = pays(4, &foreign);
            let outpoint = OutPoint::new(tx.txid(), 0);
            let utxo = Utxo::new(
                outpoint,
                tx.output[0].clone(),
                foreign.clone(),
                HEIGHT,
                false,
            );
            for account in order {
                info.accounts
                    .funds_account_mut(&account)
                    .unwrap()
                    .utxos
                    .insert(outpoint, utxo.clone());
            }
            info.update_balance();
            assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
            assert_eq!(holders(&info, &outpoint), [bip44()]);
            assert_eq!(info.balance.total(), VALUE);
        }
    }

    /// The test bar's step-4 failure (BAL-RC REPORT.md §A5): W1's BIP44 map
    /// at 14:44Z, 5 real coins and 67 parked CoinJoin coins (CoinJoin
    /// indexes 100..=166). key-wallet's default selector picks a parked coin
    /// for a 0.3 shield, whose signing then fails. After the re-home the same
    /// asset lock builds from real coins only, and the balance is exact.
    #[tokio::test]
    async fn the_test_bars_0_3_shield_builds_from_real_coins_after_the_rehome() {
        let (wallet, mut info) = wallet();
        let real: [u64; 5] = [29_999_774, 4_999_810, 98_773, 95_945, 83_449];
        let mut parked: Vec<u64> = Vec::new();
        parked.extend(std::iter::repeat_n(10_000_100, 12));
        parked.extend(std::iter::repeat_n(1_000_010, 30));
        parked.extend(std::iter::repeat_n(100_001, 23));
        parked.extend([40_000, 14_766]);
        assert_eq!(parked.iter().sum::<u64>(), 152_356_289);
        let mut real_ops = Vec::new();
        for (n, value) in real.iter().enumerate() {
            let to = wallet_receive_address(&wallet, &mut info);
            let mut tx = pays(100 + n as u8, &to);
            tx.output[0].value = *value;
            real_ops.push(park(&mut info, &tx, &to));
        }
        for (n, value) in parked.iter().enumerate() {
            let to = coinjoin_address(&wallet, &info, 100 + n as u32);
            let mut tx = pays(n as u8, &to);
            tx.output[0].value = *value;
            park(&mut info, &tx, &to);
        }
        let total = real.iter().sum::<u64>() + 152_356_289;
        assert_eq!(info.balance.total(), total);
        let lock = shield(
            &wallet,
            &mut info.clone(),
            AccountTypePreference::BIP44,
            Some(30_000_000),
        )
        .await;
        assert!(
            matches!(
                lock,
                Err(AssetLockError::Builder(BuilderError::SigningFailed(_)))
            ),
            "{:?}",
            lock.err()
        );

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!(
            done,
            Rehomed {
                moved: 67,
                moved_duffs: 152_356_289,
                ..Rehomed::default()
            }
        );
        assert_eq!(info.balance.total(), total, "exact: each coin once");
        let lock = shield(
            &wallet,
            &mut info.clone(),
            AccountTypePreference::BIP44,
            Some(30_000_000),
        )
        .await
        .unwrap();
        assert!(
            inputs(&lock.transaction)
                .iter()
                .all(|i| real_ops.contains(i)),
            "real coins only"
        );
    }

    /// The reconcile never takes a coin from its owning account: a copy the
    /// owner holds is the one kept, whichever order the accounts come in.
    #[test]
    fn the_reconcile_keeps_the_owners_copy() {
        let (wallet, mut info) = wallet();
        // A BIP44 address (owned by BIP44) also copied into CoinJoin.
        let ks = wallet.key_source_for_account(bip44());
        let to = info
            .accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .next_receive_address(
                Some(match &ks {
                    KeySource::Public(x) => x,
                    _ => unreachable!(),
                }),
                true,
            )
            .unwrap();
        let tx = pays(3, &to);
        let outpoint = OutPoint::new(tx.txid(), 0);
        let utxo = Utxo::new(outpoint, tx.output[0].clone(), to.clone(), HEIGHT, false);
        for account in [bip44(), cj()] {
            info.accounts
                .funds_account_mut(&account)
                .unwrap()
                .utxos
                .insert(outpoint, utxo.clone());
        }
        info.update_balance();
        assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
        assert_eq!(holders(&info, &outpoint), [bip44()]);
    }

    /// The CoinJoin account's external addresses at `indexes`, derived on one
    /// copy of its pool.
    fn coinjoin_addresses(
        wallet: &Wallet,
        info: &ManagedWalletInfo,
        indexes: &[u32],
    ) -> Vec<Address> {
        let ks = wallet.key_source_for_account(cj());
        let mut pool = info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0]
            .clone();
        let top = *indexes.iter().max().unwrap();
        let have = pool.highest_generated.unwrap_or(0);
        if top > have {
            pool.generate_addresses(top - have, &ks, true).unwrap();
        }
        indexes
            .iter()
            .map(|i| pool.address_at_index(*i).expect("derived"))
            .collect()
    }

    /// r1 m1 (Sol): CoinJoin generated through 99, coins at 170 and 1220.
    /// Marking 170 used keeps the gap through 270, so a later load's window
    /// reaches 1270: the first call must already find 1220, and the second
    /// then changes nothing.
    #[test]
    fn the_rehome_reaches_a_fixed_point_at_a_windows_tail() {
        let (wallet, mut info) = wallet();
        let [near, tail] =
            <[Address; 2]>::try_from(coinjoin_addresses(&wallet, &info, &[170, 1220])).unwrap();
        let near_op = park(&mut info, &pays(1, &near), &near);
        let tail_op = park(&mut info, &pays(2, &tail), &tail);

        let first = rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        assert_eq!((first.moved, first.stranded), (2, 0), "{first:?}");
        assert_eq!(holders(&info, &near_op), [cj()]);
        assert_eq!(holders(&info, &tail_op), [cj()]);
        let after_first = state(&info);
        let second = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert!(!second.changed(), "{second:?}");
        assert_eq!(state(&info), after_first);
    }

    /// r1 m2 (Sol): a coin whose own account derives it only past the pool's
    /// end is not moved, but its address is marked used and the gap is kept
    /// past it, as for a moved coin.
    #[test]
    fn a_coin_found_in_its_own_account_is_marked_used() {
        let (wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let op = park(&mut info, &pays(1, &to), &to);
        let utxo = info
            .accounts
            .funds_account_mut(&bip44())
            .unwrap()
            .utxos
            .remove(&op)
            .unwrap();
        info.accounts
            .funds_account_mut(&cj())
            .unwrap()
            .utxos
            .insert(op, utxo);
        info.update_balance();

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert!(done.changed(), "the pool changed: {done:?}");
        assert_eq!(holders(&info, &op), [cj()]);
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(pool.address_info(&to).unwrap().is_used(), "{done:?}");
        assert!(
            pool.highest_generated.unwrap() >= 170 + pool.gap_limit,
            "the gap is kept past it"
        );
        assert_eq!(info.balance.total(), VALUE);
    }

    /// r1 M2: a chain of coins, each within a window of the last, goes home
    /// up to exactly [`REHOME_MAX_INDEX`]; no pool is derived or extended
    /// past it.
    #[test]
    fn chained_coins_go_home_up_to_rehome_max_index() {
        let (wallet, mut info) = wallet();
        let mut indexes: Vec<u32> = (1..=9).map(|k| 99 + 1000 * k).collect();
        indexes.push(REHOME_MAX_INDEX);
        let addresses = coinjoin_addresses(&wallet, &info, &indexes);
        let ops: Vec<OutPoint> = addresses
            .iter()
            .enumerate()
            .map(|(n, a)| park(&mut info, &pays(n as u8, a), a))
            .collect();

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!((done.moved, done.stranded), (10, 0), "{done:?}");
        assert!(ops.iter().all(|op| holders(&info, op) == [cj()]));
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(pool.highest_generated.unwrap() <= REHOME_MAX_INDEX + pool.gap_limit);
    }

    /// r1 M2: the same chain, its last coin one index past
    /// [`REHOME_MAX_INDEX`]: that coin stays in BIP44, counted once, and the
    /// pool is never searched past the ceiling.
    #[test]
    fn a_chained_coin_past_rehome_max_index_stays() {
        let (wallet, mut info) = wallet();
        let mut indexes: Vec<u32> = (1..=9).map(|k| 99 + 1000 * k).collect();
        indexes.push(REHOME_MAX_INDEX + 1);
        let addresses = coinjoin_addresses(&wallet, &info, &indexes);
        let ops: Vec<OutPoint> = addresses
            .iter()
            .enumerate()
            .map(|(n, a)| park(&mut info, &pays(n as u8, a), a))
            .collect();

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert_eq!((done.moved, done.stranded), (9, 1), "{done:?}");
        assert_eq!(holders(&info, ops.last().unwrap()), [bip44()]);
        assert_eq!(info.balance.total(), 10 * VALUE, "each counted once");
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.address_info(addresses.last().unwrap()).is_none(),
            "never derived past the ceiling"
        );
    }

    struct Silent;

    impl crate::EventSink for Silent {
        fn emit(&self, _: crate::EngineEvent) {}
    }

    const PHRASE: &[u8] =
        b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    /// Unreachable SPV peer, DAPI and quorum service: no network.
    fn offline() -> crate::SessionOptions {
        crate::SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        }
    }

    /// A real engine and regtest session ([`offline`]) with one imported
    /// wallet.
    fn engine_wallet() -> (
        tempfile::TempDir,
        crate::Engine,
        Arc<NetworkSession>,
        WalletId,
    ) {
        use crate::{DashNetwork, Engine, EngineConfig, ImportOptions};
        use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..Default::default()
                },
            },
            Arc::new(Silent),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(DashNetwork::Regtest, offline()))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(None)))
            .unwrap();
        let id = engine
            .block_on(session.import_wallet(
                zeroize::Zeroizing::new(PHRASE.to_vec()),
                zeroize::Zeroizing::new(Vec::new()),
                ImportOptions {
                    birth_height: Some(0),
                    ..Default::default()
                },
            ))
            .unwrap();
        (dir, engine, session, id)
    }

    /// Delivers `balance` to platform-wallet's balance mirror of wallet `id`
    /// and to the hub, as a block event does.
    fn deliver(
        session: &NetworkSession,
        pw: &Arc<platform_wallet::PlatformWallet>,
        id: WalletId,
        balance: u64,
    ) {
        use dash_spv::EventHandler;
        use key_wallet::wallet::balance::WalletCoreBalance;
        let mirrors = Arc::new(Default::default());
        let handler =
            platform_wallet::wallet::core::BalanceUpdateHandler::new(Arc::clone(&mirrors));
        mirrors.store(Arc::new(BTreeMap::from([(id.0, Arc::clone(pw))])));
        let event = platform_wallet::events::WalletEvent::BlockProcessed {
            wallet_id: id.0,
            height: HEIGHT,
            chain_lock: None,
            inserted: vec![],
            updated: vec![],
            matured: vec![],
            balance: WalletCoreBalance::new(balance, 0, 0, 0),
            account_balances: BTreeMap::new(),
            addresses_derived: vec![],
        };
        handler.on_wallet_event(&event);
        session.hub.apply_wallet_event(&event);
    }

    /// r1 M1 (Sol): a replay's doubled balance reaches platform-wallet's
    /// mirror; the reconcile corrects key-wallet and the hub; a later
    /// refresh (set_birth_height, a cancelled rescan, a send) must not bring
    /// the doubled total back.
    #[test]
    fn a_refresh_after_the_reconcile_keeps_the_deduplicated_total() {
        let (_dir, engine, session, id) = engine_wallet();
        engine.block_on(async {
            let manager = session.manager().unwrap();
            let pw = manager.get_wallet(&id.0).await.unwrap();
            {
                let wm = manager.wallet_manager_arc();
                let mut wm = wm.write().await;
                let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
                let info = &mut info.core_wallet;
                let to = coinjoin_address(wallet, info, 5);
                let op = park(info, &pays(1, &to), &to);
                let coin = info.accounts.funds_account(&bip44()).unwrap().utxos[&op].clone();
                info.accounts
                    .funds_account_mut(&cj())
                    .unwrap()
                    .utxos
                    .insert(op, coin);
                info.update_balance();
                assert_eq!(info.balance.total(), 2 * VALUE);
            }
            deliver(&session, &pw, id, 2 * VALUE);
            session.reconcile_parked_coins(id).await;
            assert_eq!(
                session.hub.wallet_state(&id).unwrap().balance.total(),
                VALUE
            );

            session.refresh_wallet_state(&manager, id).await;

            assert_eq!(
                session.hub.wallet_state(&id).unwrap().balance.total(),
                VALUE
            );
        });
    }

    /// r1 M1: the same after the load-time eviction of a contact's coin: the
    /// mirror still counts it, and a refresh must not.
    #[test]
    fn a_refresh_after_a_contact_eviction_keeps_it_uncounted() {
        let (_dir, engine, session, id) = engine_wallet();
        engine.block_on(async {
            let manager = session.manager().unwrap();
            let pw = manager.get_wallet(&id.0).await.unwrap();
            {
                let wm = manager.wallet_manager_arc();
                let mut wm = wm.write().await;
                let info = &mut wm.get_wallet_info_mut(&id.0).unwrap().core_wallet;
                let contact = AccountType::DashpayExternalAccount {
                    index: 0,
                    user_identity_id: [1; 32],
                    friend_identity_id: [2; 32],
                };
                let other =
                    Wallet::new_random(Network::Regtest, WalletAccountCreationOptions::Default)
                        .unwrap();
                let KeySource::Public(theirs) = other.key_source_for_account(bip44()) else {
                    unreachable!()
                };
                info.add_managed_account_from_xpub(contact, theirs).unwrap();
                let to = (info
                    .accounts
                    .dashpay_external_accounts
                    .values()
                    .next()
                    .unwrap())
                .managed_account_type()
                .address_pools()[0]
                    .address_at_index(0)
                    .unwrap();
                park(info, &pays(1, &to), &to);
                assert_eq!(info.balance.total(), VALUE);
            }
            deliver(&session, &pw, id, VALUE);
            session.rehome_parked_coins(&manager, id).await;

            session.refresh_wallet_state(&manager, id).await;

            assert_eq!(session.hub.wallet_state(&id).unwrap().balance.total(), 0);
        });
    }

    /// r1 m2 (Sol), through the real hooks: a BIP44 coin past the stored
    /// pool, written by the real persister, then `unload_wallet` and
    /// `load_wallet` (the load hook runs the re-home): its address is marked
    /// used and the gap is kept past it.
    #[test]
    fn a_real_load_marks_a_sparse_bip44_coin_used() {
        let (_dir, engine, session, id) = engine_wallet();
        let (to, index) = engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let wallet = wm.get_wallet(&id.0).unwrap();
            let info = wm.get_wallet_info(&id.0).unwrap();
            let mut pool = (info.core_wallet.accounts.funds_account(&bip44()).unwrap())
                .managed_account_type()
                .address_pools()[0]
                .clone();
            let index = pool.highest_generated.unwrap() + 50;
            pool.generate_addresses(50, &wallet.key_source_for_account(bip44()), true)
                .unwrap();
            (pool.address_at_index(index).unwrap(), index)
        });
        store_coin(&session, id, confirmed_coin(&pays(1, &to), &to));
        engine.block_on(session.unload_wallet(id)).unwrap();

        engine.block_on(session.load_wallet(id)).unwrap();

        engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
            let pool = &info
                .accounts
                .funds_account(&bip44())
                .unwrap()
                .managed_account_type()
                .address_pools()[0];
            let at = pool
                .address_info(&to)
                .expect("the load's re-home derives it");
            assert!(at.is_used(), "highest_used={:?}", pool.highest_used);
            assert!(
                pool.highest_generated.unwrap() >= index + pool.gap_limit,
                "the gap is kept past it"
            );
            assert_eq!(info.balance.total(), VALUE);
        });
    }

    /// r1 Opus m5: a reconcile that finds no duplicate takes no write lock
    /// (it would block SPV's block processing on every height step), and still
    /// publishes key-wallet's balance.
    #[test]
    fn a_reconcile_without_duplicates_needs_no_write_lock() {
        let (_dir, engine, session, id) = engine_wallet();
        engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let reader = wm.read().await;

            let ran = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                session.reconcile_parked_coins(id),
            )
            .await;

            drop(reader);
            assert!(ran.is_ok(), "it waited for the write lock");
            assert_eq!(session.hub.wallet_state(&id).unwrap().balance.total(), 0);
        });
    }

    /// Writes `coin` to wallet `id`'s store as SPV would: a new UTXO, with no
    /// pool row for its address.
    fn store_coin(session: &NetworkSession, id: WalletId, coin: Utxo) {
        use platform_wallet::changeset::{
            CoreChangeSet, PlatformWalletChangeSet, PlatformWalletPersistence,
        };
        let live = session.live().unwrap();
        let core = CoreChangeSet {
            new_utxos: vec![coin],
            last_processed_height: Some(HEIGHT + 300),
            synced_height: Some(HEIGHT + 300),
            ..Default::default()
        };
        live.persister
            .store(
                id.0,
                PlatformWalletChangeSet {
                    core: Some(core),
                    ..Default::default()
                },
            )
            .unwrap();
        live.persister.flush(id.0).unwrap();
    }

    fn confirmed_coin(tx: &Transaction, to: &Address) -> Utxo {
        let mut coin = Utxo::new(
            OutPoint::new(tx.txid(), 0),
            tx.output[0].clone(),
            to.clone(),
            HEIGHT,
            false,
        );
        coin.is_confirmed = true;
        coin
    }

    /// r1 Opus M1, the session-open hook: a CoinJoin coin past index 99 with
    /// no pool row, stored, then the session closed and reopened offline.
    /// The open re-homes it before SPV starts: CoinJoin holds it, once.
    #[test]
    fn a_reopened_session_rehomes_a_coinjoin_coin_past_99() {
        let (_dir, engine, session, id) = engine_wallet();
        let to = engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let wallet = wm.get_wallet(&id.0).unwrap();
            coinjoin_address(wallet, &wm.get_wallet_info(&id.0).unwrap().core_wallet, 150)
        });
        let tx = pays(1, &to);
        store_coin(&session, id, confirmed_coin(&tx, &to));
        drop(session);
        assert!(
            engine
                .block_on(engine.close_network(crate::DashNetwork::Regtest))
                .unwrap()
        );

        let session = engine
            .block_on(engine.open_network(crate::DashNetwork::Regtest, offline()))
            .unwrap();

        engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
            assert_eq!(holders(info, &OutPoint::new(tx.txid(), 0)), [cj()]);
            assert!(derives(info, &cj(), &to), "signable from CoinJoin");
            assert_eq!(info.balance.total(), VALUE);
        });
        assert_eq!(
            session.hub.wallet_state(&id).unwrap().balance.total(),
            VALUE
        );
    }

    /// r1 Opus M1, the block-event hook: a replay's duplicate, then a block
    /// event through the hub. The attached reconcile runs on its own, drops
    /// the parked copy and publishes the deduplicated balance.
    #[test]
    fn a_block_event_starts_the_attached_reconcile() {
        let (_dir, engine, session, id) = engine_wallet();
        let op = engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let (wallet, info) = wm.get_wallet_and_info_mut(&id.0).unwrap();
            let info = &mut info.core_wallet;
            let to = coinjoin_address(wallet, info, 5);
            let op = park(info, &pays(1, &to), &to);
            let coin = info.accounts.funds_account(&bip44()).unwrap().utxos[&op].clone();
            (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(op, coin);
            info.update_balance();
            assert_eq!(info.balance.total(), 2 * VALUE);
            op
        });
        let pw = engine
            .block_on(session.manager().unwrap().get_wallet(&id.0))
            .unwrap();

        deliver(&session, &pw, id, 2 * VALUE);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while session.hub.wallet_state(&id).unwrap().balance.total() != VALUE {
            assert!(std::time::Instant::now() < deadline, "no reconcile ran");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        engine.block_on(async {
            let manager = session.manager().unwrap();
            let wm = manager.wallet_manager_arc();
            let wm = wm.read().await;
            let info = &wm.get_wallet_info(&id.0).unwrap().core_wallet;
            assert_eq!(holders(info, &op), [cj()]);
        });
    }

    /// One transaction paying `addresses` in output order, parked in BIP44:
    /// same-txid outpoints iterate in `vout` order, so the order the re-home
    /// sees the coins in is fixed.
    fn park_batch(info: &mut ManagedWalletInfo, addresses: &[Address]) -> Vec<OutPoint> {
        let mut tx = pays(1, &addresses[0]);
        tx.output = (addresses.iter())
            .map(|a| TxOut {
                value: VALUE,
                script_pubkey: a.script_pubkey(),
            })
            .collect();
        let mut outpoints = Vec::new();
        for (vout, address) in addresses.iter().enumerate() {
            let outpoint = OutPoint::new(tx.txid(), vout as u32);
            let mut coin = Utxo::new(
                outpoint,
                tx.output[vout].clone(),
                address.clone(),
                HEIGHT,
                false,
            );
            coin.is_confirmed = true;
            (info.accounts.funds_account_mut(&bip44()).unwrap().utxos).insert(outpoint, coin);
            outpoints.push(outpoint);
        }
        info.update_balance();
        outpoints
    }

    /// r2 Sol M1 (Sol's regression): a coin at the cap, then coins every 100
    /// past it. Marking one used must not refill the pool to the next, and so
    /// on: nothing is generated past [`REHOME_MAX_INDEX`] and the coins past
    /// it stay parked, counted once.
    #[test]
    fn gap_refills_cannot_walk_past_rehome_max_index() {
        let (wallet, mut info) = wallet();
        let mut indexes: Vec<u32> = (1..=9).map(|k| 99 + 1000 * k).collect();
        indexes.extend((0..=10).map(|k| REHOME_MAX_INDEX + k * 100));
        let addresses = coinjoin_addresses(&wallet, &info, &indexes);
        let outpoints = park_batch(&mut info, &addresses);

        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.highest_generated.unwrap() <= REHOME_MAX_INDEX + pool.gap_limit,
            "highest_generated={:?}, highest_used={:?}, {done:?}",
            pool.highest_generated,
            pool.highest_used
        );
        assert_eq!((done.moved, done.stranded), (10, 10), "{done:?}");
        for (index, outpoint) in indexes.iter().zip(&outpoints) {
            let held = if *index > REHOME_MAX_INDEX {
                bip44()
            } else {
                cj()
            };
            assert_eq!(holders(&info, outpoint), [held], "index {index}");
        }
        assert_eq!(info.balance.total(), 20 * VALUE);
    }

    /// r2 Sol M1 (Sol's regression): the coin past the cap first in output
    /// order, then the one at it. A second call changes nothing.
    #[test]
    fn the_cap_tail_is_idempotent_in_reverse_output_order() {
        let (wallet, mut info) = wallet();
        let mut indexes = vec![REHOME_MAX_INDEX + 1, REHOME_MAX_INDEX];
        indexes.extend((1..=9).map(|k| 99 + 1000 * k));
        let addresses = coinjoin_addresses(&wallet, &info, &indexes);
        park_batch(&mut info, &addresses);
        let first = rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        let after_first = state(&info);

        let second = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert!(!second.changed(), "first={first:?}; second={second:?}");
        assert_eq!(state(&info), after_first);
    }

    /// r2 Opus m1 (Opus's regression): a stored lookahead (applied before the
    /// re-home) already holds one parked coin's address, unused. Marking it
    /// used must not take a second call's search past the first's.
    #[test]
    fn a_coin_a_lookahead_holds_keeps_the_fixed_point() {
        let (wallet, mut info) = wallet();
        let ks = wallet.key_source_for_account(cj());
        let account = info.accounts.funds_account_mut(&cj()).unwrap();
        for pool in account.managed_account_type_mut().address_pools_mut() {
            pool.set_gap_limit(1000, &ks).unwrap();
        }
        let addresses = coinjoin_addresses(&wallet, &info, &[500, 2300]);
        let held = park(&mut info, &pays(1, &addresses[0]), &addresses[0]);
        let tail = park(&mut info, &pays(2, &addresses[1]), &addresses[1]);
        let first = rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        assert_eq!(holders(&info, &held), [cj()]);
        let after_first = state(&info);

        let second = rehome_parked(&wallet, &mut info, REHOME_SEARCH);

        assert!(!second.changed(), "first={first:?}; second={second:?}");
        assert_eq!(state(&info), after_first);
        assert_eq!(holders(&info, &tail), [cj()], "the first call finds it");
    }
}
