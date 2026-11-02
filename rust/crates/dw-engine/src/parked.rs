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
//! [`REHOME_SEARCH`] indexes past what it holds), and moves the coin there.
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

/// The re-home's search window: how many indexes past a pool's highest
/// generated one (or past the last parked address it found there) it derives
/// looking for a parked coin's address. It is key-wallet's largest gap limit
/// (`gap_limit::MAX_GAP_LIMIT`, 1000, also the engine's `MAX_LOOKAHEAD`): a
/// scan never derives more than a gap limit past the last used address, so
/// any coin the scan itself found lies within one window of a used address.
/// The one miss is a used address whose coins were all spent and whose pool
/// row was never stored, more than a window before a parked coin; that coin
/// stays (and is logged). The test bar's wallet needed about 70 past the
/// CoinJoin window (index ~170 against 0..99).
pub(crate) const REHOME_SEARCH: u32 = key_wallet::gap_limit::MAX_GAP_LIMIT;

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
    /// Coins that stay where they are: no funds account derives their address
    /// within [`REHOME_SEARCH`] (counted once).
    pub stranded: usize,
    pub stranded_duffs: u64,
}

impl Rehomed {
    pub(crate) fn changed(&self) -> bool {
        self.moved + self.dropped + self.evicted > 0
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
    // Addresses no pool holds yet: look for them past each pool's end.
    let mut wanted: HashMap<ScriptBuf, Address> = HashMap::new();
    for (holder, outpoint) in &parked {
        let utxo = &info.accounts.funds_account(holder).expect("listed").utxos[outpoint];
        if owner(info, &accounts, &utxo.address).is_none() {
            wanted.insert(utxo.txout.script_pubkey.clone(), utxo.address.clone());
        }
    }
    for account in &accounts {
        if wanted.is_empty() {
            break;
        }
        extend_to(wallet, info, account, &mut wanted, search);
    }
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
        mark_used(wallet, info, &home, &utxo.address);
    }
    if done.stranded > 0 {
        tracing::warn!(
            coins = done.stranded,
            duffs = done.stranded_duffs,
            search,
            "coins whose address no funds account derives within the search bound stay where they were loaded"
        );
    }
    info.update_balance();
    done
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
/// or the pool's gap limit if larger, past the pool's highest generated
/// index and past each wanted address found. Then extends the pool up to the
/// furthest one found. Found scripts leave `wanted`.
fn extend_to(
    wallet: &Wallet,
    info: &mut ManagedWalletInfo,
    account: &AccountType,
    wanted: &mut HashMap<ScriptBuf, Address>,
    search: u32,
) {
    let key_source = wallet.key_source_for_account(*account);
    let KeySource::Public(xpub) = key_source else {
        return;
    };
    let Some(a) = info.accounts.funds_account_mut(account) else {
        return;
    };
    let mut extended = false;
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
        // Never less than the pool's gap: marking a found address used then
        // generates no address the search skipped.
        let window = search.max(pool.gap_limit);
        let start = pool.highest_generated.map_or(0, |h| h + 1);
        let (mut furthest, mut end) = (None, start.saturating_add(window));
        let mut index = start;
        // As a gap-limit scan: each address found opens another window.
        while index < end && !wanted.is_empty() {
            let Ok(child) = branch.derive_pub(&[ChildNumber::Normal { index }]) else {
                break;
            };
            let address = Address::p2pkh(&dashcore::PublicKey::new(child.public_key), pool.network);
            if wanted.remove(&address.script_pubkey()).is_some() {
                furthest = Some(index);
                end = index.saturating_add(1).saturating_add(window);
            }
            index += 1;
        }
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
}

/// Marks `address` used in `account`'s pools and keeps their gap past it.
fn mark_used(
    wallet: &Wallet,
    info: &mut ManagedWalletInfo,
    account: &AccountType,
    address: &Address,
) {
    let key_source = wallet.key_source_for_account(*account);
    let Some(a) = info.accounts.funds_account_mut(account) else {
        return;
    };
    a.managed_account_type_mut().mark_address_used(address);
    if !matches!(key_source, KeySource::NoKeySource) {
        for pool in a.managed_account_type_mut().address_pools_mut() {
            if let Err(e) = pool.maintain_gap_limit(&key_source) {
                tracing::warn!(?account, error = %e, "could not keep a pool's gap past a re-homed coin");
            }
        }
    }
    a.bump_monitor_revision();
}

impl NetworkSession {
    /// Re-homes wallet `id`'s parked coins ([`rehome_parked`]). Runs after
    /// the wallet's stored state is loaded and before SPV can replay a block.
    /// True when it changed the wallet's coins.
    pub(crate) async fn rehome_parked_coins(&self, manager: &Manager, id: WalletId) -> bool {
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some((wallet, info)) = wm.get_wallet_and_info_mut(&id.0) else {
            return false;
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
                "re-homed coins loaded into an account that does not derive their address"
            );
        }
        done.changed()
    }

    /// Drops wallet `id`'s duplicate coins ([`reconcile_duplicates`]) after
    /// its block events, then publishes key-wallet's balance: it also undoes
    /// a doubled balance an event computed before an earlier reconcile.
    pub(crate) async fn reconcile_parked_coins(&self, id: WalletId) {
        self.hub.reconcile_running(id);
        let Ok(manager) = self.manager() else {
            return;
        };
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some(info) = wm.get_wallet_info_mut(&id.0) else {
            return;
        };
        let core = &mut info.core_wallet;
        let (dropped, duffs) = reconcile_duplicates(core);
        if dropped > 0 {
            tracing::warn!(wallet_id = %id, dropped, duffs, "dropped coins two funds accounts held");
        }
        // An event computed before this ran but applied after it starts
        // another reconcile (the pending mark is cleared above).
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
        // the way SPV's gap extension does.
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
        // the window `near` opens, `far` one past the window `chained` opens.
        let near = coinjoin_address(&wallet, &info, have + gap);
        let chained = coinjoin_address(&wallet, &info, have + 2 * gap);
        let far = coinjoin_address(&wallet, &info, have + 3 * gap + 1);
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

        let started = std::time::Instant::now();
        let done = rehome_parked(&wallet, &mut info, REHOME_SEARCH);
        println!("re-homed {} coins in {:?}", done.moved, started.elapsed());

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
}
