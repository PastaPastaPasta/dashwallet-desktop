//! Coins a wallet's store holds past the addresses its load derives.
//!
//! At load, platform-wallet-storage restores each stored coin into the funds
//! account whose pools derive its address within the default gap limits
//! (key-wallet `restore_utxos`, dashpay/platform#5724). It stores pool rows
//! only for a pool snapshot, never for the addresses SPV derives past them
//! (dashpay/platform#5723). So it leaves a coin in its stored owner, or in the
//! first funds account (BIP44), where no account can sign for it, when:
//! - only a lookahead raised after the load reaches it (a stored one, the
//!   CoinJoin recovery scan's), possibly through a spent coin's address that
//!   only that gap reaches;
//! - it lies past the default gap with no stored used address near it (a
//!   sparse coin).
//!
//! Such a coin counts in the balance but can't be spent, and a replay of its
//! block (a rescan, an abandon's rescan, a crash catch-up) credits it again
//! in the account that derives it.
//!
//! [`restore_beyond_gap`] runs after every load and lookahead raise: at
//! session open before SPV starts; at `load_wallet` after the wallet joins
//! the running SPV, whose blocks until then see the default gaps only. It
//! marks the address of every coin the store
//! holds, spent or unspent, used wherever a funds pool derives it, keeping
//! the gap, until nothing more matches. Then it looks for the rest a window
//! past each pool, from the account's own keys. Then it places each coin in
//! the account that derives its address.
//! [`reconcile_duplicates`] stays as the safety net after block events: one
//! copy per outpoint, in the account that owns the address. Nothing here
//! persists anything (`core_utxos` has no account column).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use dashcore::{Address, OutPoint, ScriptBuf};
use key_wallet::account::AccountType;
use key_wallet::managed_account::address_pool::{AddressPoolType, KeySource};
use key_wallet::managed_account::managed_account_ref::ManagedAccountRef;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::wallet::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use key_wallet::{ChildNumber, Utxo, Wallet};

use crate::WalletId;
use crate::error::EngineError;
use crate::session::{Manager, NetworkSession, WALLET_DB_FILE};

/// How far past a pool's reach (the furthest index it holds, or would
/// generate keeping its gap) the search derives looking for a stored address
/// no pool holds; each address found opens another window. It is key-wallet's
/// largest gap limit (`gap_limit::MAX_GAP_LIMIT`, 1000, also the engine's
/// `MAX_LOOKAHEAD`): a scan never derives more than a gap limit past the last
/// used address, so every coin it found lies within one window of a used
/// address, or of the start.
pub(crate) const SEARCH_WINDOW: u32 = key_wallet::gap_limit::MAX_GAP_LIMIT;

/// The search's budget per pool and restore: it derives at most this many
/// indexes past the pool's end. Coins past it stay where they were loaded,
/// counted once, and are logged.
pub(crate) const SEARCH_BUDGET: u32 = 10_000;

/// What [`restore_beyond_gap`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Restored {
    /// Coins held by an account that doesn't derive them, matched to the
    /// account that does (spent ones among them are dropped, not credited).
    pub matched: usize,
    /// Coins no funds account derives within the search: they stay.
    pub unmatched: usize,
    pub unmatched_duffs: u64,
    /// Stored addresses the window search found past the pools.
    pub found: usize,
    /// [`SEARCH_BUDGET`] ended a pool's search with addresses still unfound.
    pub capped: bool,
}

/// The scripts of every coin, spent or unspent, wallet `wallet_id`'s store
/// holds, read from `wallet.sqlite` over a read-only connection
/// (SqlitePersister has no API for them).
pub(crate) fn stored_scripts(
    db_path: &Path,
    wallet_id: &[u8; 32],
) -> Result<HashSet<ScriptBuf>, rusqlite::Error> {
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    // A script longer than consensus allows is no address of ours: a corrupt
    // row is skipped rather than read.
    let mut stmt = conn.prepare(
        "SELECT DISTINCT script FROM core_utxos \
         WHERE wallet_id = ?1 AND is_sweep_placeholder = 0 AND length(script) <= 10000",
    )?;
    let rows = stmt.query_map([wallet_id.as_slice()], |r| r.get::<_, Vec<u8>>(0))?;
    rows.map(|r| r.map(ScriptBuf::from)).collect()
}

/// Places each coin a funds account holds without deriving its address in
/// the funds account that derives it, first extending the pools to the
/// stored coins' addresses (`stored`, spent and unspent): marking each used
/// where a pool derives it, keeping the gap, until nothing more matches;
/// then searching [`SEARCH_WINDOW`] past each pool from the account's keys,
/// within [`SEARCH_BUDGET`]. Recomputes the balance.
///
/// An error (a key derivation key-wallet refuses) leaves every coin where it
/// was; pools may have grown.
pub(crate) fn restore_beyond_gap(
    wallet: &Wallet,
    info: &mut ManagedWalletInfo,
    stored: &HashSet<ScriptBuf>,
) -> Result<Restored, key_wallet::Error> {
    let underived: Vec<Utxo> = (info.accounts.all_funding_accounts().into_iter())
        .flat_map(|a| {
            let pools = a.managed_account_type();
            (a.utxos.values()).filter(move |u| !pools.contains_address(&u.address))
        })
        .cloned()
        .collect();
    let mut wanted: HashSet<ScriptBuf> = (stored.iter().cloned())
        .chain(underived.iter().map(|u| u.txout.script_pubkey.clone()))
        .collect();
    let mut done = Restored::default();
    let mut checked: Vec<Option<u32>> = Vec::new();
    let mut searched: HashMap<(AccountType, AddressPoolType), u32> = HashMap::new();
    loop {
        let mut moved = false;
        loop {
            let marked = mark_derived(info, &mut wanted, &mut checked);
            info.restore_utxos(wallet, std::iter::empty())?;
            if !marked {
                break;
            }
            moved = true;
        }
        wanted.retain(|s| !derived_outside_funds(info, s));
        // A search already scanned a window past every pool it extended:
        // another finds something only past a pool a stored address moved.
        if wanted.is_empty() || (done.found > 0 && !moved) {
            break;
        }
        let found = search(wallet, info, &mut wanted, &mut searched, &mut done.capped);
        if found == 0 {
            break;
        }
        done.found += found;
    }
    if !underived.is_empty() {
        let count = underived.len();
        let unmatched = info.restore_utxos(wallet, underived)?;
        done.matched = count - unmatched.len();
        done.unmatched = unmatched.len();
        done.unmatched_duffs = unmatched.iter().map(|u| u.value()).sum();
    }
    info.update_balance();
    Ok(done)
}

/// Marks every `wanted` script a funds pool holds used there, and drops it
/// from `wanted`. `checked` keeps, per pool in key-wallet's funds order, the
/// highest index already looked at, so each address is looked at once.
/// True when an address was newly marked (its pool's gap moves).
fn mark_derived(
    info: &mut ManagedWalletInfo,
    wanted: &mut HashSet<ScriptBuf>,
    checked: &mut Vec<Option<u32>>,
) -> bool {
    let mut marked = false;
    let mut k = 0;
    for funds in info.accounts.all_funding_accounts_mut() {
        for pool in funds.managed_account_type_mut().address_pools_mut() {
            if checked.len() <= k {
                checked.push(None);
            }
            let from = checked[k].map_or(0, |c| c + 1);
            let hits: Vec<u32> = (pool.addresses.range(from..))
                .filter(|(_, a)| wanted.remove(&a.script_pubkey))
                .map(|(i, _)| *i)
                .collect();
            for index in hits {
                marked |= pool.mark_index_used(index);
            }
            checked[k] = pool.highest_generated.or(checked[k]);
            k += 1;
        }
    }
    marked
}

/// Whether an account other than a funds account holds `script`: a DashPay
/// contact's (watched, never ours) or a keys account's. Such a coin is not
/// looked for.
fn derived_outside_funds(info: &ManagedWalletInfo, script: &ScriptBuf) -> bool {
    (info.accounts.all_accounts().into_iter())
        .map(|a| match a {
            ManagedAccountRef::Funds(f) => (f.managed_account_type(), false),
            ManagedAccountRef::Keys(k) => (k.managed_account_type(), true),
        })
        .filter(|(t, keys)| *keys || t.is_contact_owned())
        .any(|(t, _)| (t.address_pools().iter()).any(|p| p.contains_script_pubkey(script)))
}

/// Looks for the `wanted` scripts in each address pool of each funds
/// account, deriving from the account's xpub (without storing) a window of
/// [`SEARCH_WINDOW`], or the pool's gap limit if larger, past the furthest
/// index the pool holds or would generate keeping its gap, and past the gap
/// of each address found, never more than [`SEARCH_BUDGET`] indexes per pool
/// (counted in `searched`). Extends each pool up to the furthest address
/// found and marks the found ones used. Returns how many it found.
fn search(
    wallet: &Wallet,
    info: &mut ManagedWalletInfo,
    wanted: &mut HashSet<ScriptBuf>,
    searched: &mut HashMap<(AccountType, AddressPoolType), u32>,
    capped: &mut bool,
) -> usize {
    let mut found = 0;
    for account in funds_accounts(info) {
        if wanted.is_empty() {
            break;
        }
        let key_source = wallet.key_source_for_account(account);
        let KeySource::Public(xpub) = key_source else {
            continue;
        };
        let Some(a) = info.accounts.funds_account_mut(&account) else {
            continue;
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
            let gap = pool.gap_limit;
            let window = SEARCH_WINDOW.max(gap);
            let start = pool.highest_generated.map_or(0, |h| h + 1);
            let spent = searched.entry((account, pool.pool_type)).or_insert(0);
            let ceiling = start.saturating_add(SEARCH_BUDGET.saturating_sub(*spent));
            // Where a later load's search would start: past what keeping
            // the gap generates (highest used + gap).
            let mut reach = (pool.highest_generated)
                .max(pool.highest_used.map(|u| u.saturating_add(gap)))
                .unwrap_or(gap.saturating_sub(1));
            let horizon = |reach: u32| reach.saturating_add(1).saturating_add(window).min(ceiling);
            let mut end = horizon(reach);
            let mut hits = Vec::new();
            let mut index = start;
            // As a gap-limit scan: each address found opens another window.
            while index < end && !wanted.is_empty() {
                let Ok(child) = branch.derive_pub(&[ChildNumber::Normal { index }]) else {
                    break;
                };
                let address =
                    Address::p2pkh(&dashcore::PublicKey::new(child.public_key), pool.network);
                if wanted.remove(&address.script_pubkey()) {
                    hits.push(index);
                    reach = reach.max(index.saturating_add(gap));
                    end = horizon(reach);
                }
                index += 1;
            }
            *spent += index - start;
            *capped |= index >= ceiling && !wanted.is_empty();
            let Some(&furthest) = hits.last() else {
                continue;
            };
            if let Err(e) = pool.generate_up_to(furthest, &key_source) {
                tracing::warn!(?account, error = %e, "could not extend a pool to a stored coin");
                continue;
            }
            for index in &hits {
                pool.mark_index_used(*index);
            }
            found += hits.len();
            extended = true;
        }
        if extended {
            a.bump_monitor_revision();
        }
    }
    found
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

#[cfg(test)]
tokio::task_local! {
    /// Tests: [`NetworkSession::restore_coins`] fails in the task in scope,
    /// after reading the store, before changing any coin.
    pub(crate) static FAIL_RESTORE: ();
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

impl NetworkSession {
    /// Runs [`restore_beyond_gap`] on open wallet `id` with its store's coin
    /// scripts. Callers refresh the hub. An error leaves the coins where they
    /// were, so a caller about to rescan stops.
    pub(crate) async fn restore_coins(
        &self,
        manager: &Manager,
        id: WalletId,
    ) -> Result<Restored, EngineError> {
        let db_path = self.data_dir().join(WALLET_DB_FILE);
        let stored = tokio::task::spawn_blocking(move || stored_scripts(&db_path, &id.0))
            .await?
            .unwrap_or_else(|e| {
                // Only the coins the load left outside their account are
                // looked for then.
                tracing::warn!(wallet_id = %id, error = %e, "could not read the stored coin scripts");
                HashSet::new()
            });
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let Some((wallet, info)) = wm.get_wallet_and_info_mut(&id.0) else {
            return Ok(Restored::default());
        };
        #[cfg(test)]
        if FAIL_RESTORE.try_with(|_| ()).is_ok() {
            return Err(EngineError::Internal(
                "restoring coins past the gap: a test's failure".into(),
            ));
        }
        let done = restore_beyond_gap(wallet, &mut info.core_wallet, &stored)
            .map_err(|e| EngineError::Internal(format!("restoring coins past the gap: {e}")))?;
        if done.matched + done.found > 0 {
            tracing::info!(
                wallet_id = %id,
                matched = done.matched,
                found = done.found,
                "placed coins past the loaded pools in the accounts that derive them"
            );
        }
        if done.unmatched > 0 {
            tracing::warn!(
                wallet_id = %id,
                coins = done.unmatched,
                duffs = done.unmatched_duffs,
                window = SEARCH_WINDOW,
                budget = SEARCH_BUDGET,
                capped = done.capped,
                "coins whose address no funds account derives within the search stay where they were loaded"
            );
        }
        Ok(done)
    }

    /// Drops wallet `id`'s duplicate coins ([`reconcile_duplicates`]) after
    /// its block events, then publishes key-wallet's balance: it also undoes
    /// a doubled balance an event computed before an earlier reconcile.
    /// Looks for duplicates under the read lock first, and takes the write
    /// lock only when there are some (rare: a replay of a coin no restore
    /// placed).
    pub(crate) async fn reconcile_duplicate_coins(&self, id: WalletId) {
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

    /// Hooks [`Self::reconcile_duplicate_coins`] to the hub's block events.
    pub(crate) fn attach_reconcile(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.attach_reconcile(Box::new(move |id| {
            let weak = weak.clone();
            rt.spawn(async move {
                if let Some(session) = weak.upgrade() {
                    session.reconcile_duplicate_coins(id).await;
                }
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use dashcore::hashes::Hash;
    use dashcore::{BlockHash, Network, Transaction, TxIn, TxOut, Txid};
    use key_wallet::transaction_checking::{
        BlockInfo, TransactionContext, WalletTransactionChecker,
    };
    use key_wallet::wallet::initialization::WalletAccountCreationOptions;

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

    /// A fresh wallet, as loaded: the CoinJoin pool holds its first window
    /// (0..99) only.
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
        let mut pool = (info.accounts.funds_account(&cj()).unwrap())
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

    /// What the load does with a coin no account derives within its gap: it
    /// goes into the first funds account, BIP44.
    fn park(info: &mut ManagedWalletInfo, tx: &Transaction, to: &Address) -> OutPoint {
        let outpoint = OutPoint::new(tx.txid(), 0);
        let mut utxo = Utxo::new(outpoint, tx.output[0].clone(), to.clone(), HEIGHT, false);
        utxo.is_confirmed = true;
        (info.accounts.funds_account_mut(&bip44()).unwrap().utxos).insert(outpoint, utxo);
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

    fn wallet_receive_address(wallet: &Wallet, info: &mut ManagedWalletInfo) -> Address {
        let KeySource::Public(xpub) = wallet.key_source_for_account(bip44()) else {
            unreachable!("a seed wallet's BIP44 account has an xpub")
        };
        (info.accounts.funds_account_mut(&bip44()).unwrap())
            .next_receive_address(Some(&xpub), true)
            .unwrap()
    }

    /// A coin past the CoinJoin window the load left in BIP44 goes back to
    /// CoinJoin, signable from there, and a replay of its block counts it
    /// once.
    #[tokio::test]
    async fn a_coin_within_the_window_goes_home_and_a_replay_counts_it_once() {
        let (mut wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let tx = pays(1, &to);
        let outpoint = park(&mut info, &tx, &to);

        let done = restore_beyond_gap(&wallet, &mut info, &HashSet::new()).unwrap();

        assert_eq!((done.matched, done.found, done.unmatched), (1, 1, 0));
        assert_eq!(holders(&info, &outpoint), [cj()]);
        replay(&mut wallet, &mut info, &tx).await;
        assert_eq!(holders(&info, &outpoint), [cj()]);
        assert_eq!(info.balance.total(), VALUE);
    }

    /// The search reaches [`SEARCH_WINDOW`] past the pool's reach: a coin at
    /// the last index it covers goes home, one index further stays, counted
    /// once.
    #[test]
    fn the_search_reaches_search_window_past_the_pool() {
        let (wallet, loaded) = wallet();
        let pool = &loaded
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.gap_limit < SEARCH_WINDOW,
            "the window, not the gap, decides"
        );
        let have = pool.highest_generated.unwrap();

        let mut info = loaded.clone();
        let last = coinjoin_address(&wallet, &info, have + SEARCH_WINDOW);
        let at = park(&mut info, &pays(1, &last), &last);
        restore_beyond_gap(&wallet, &mut info, &HashSet::new()).unwrap();
        assert_eq!(holders(&info, &at), [cj()]);

        let mut info = loaded;
        let past = coinjoin_address(&wallet, &info, have + SEARCH_WINDOW + 1);
        let beyond = park(&mut info, &pays(2, &past), &past);
        let done = restore_beyond_gap(&wallet, &mut info, &HashSet::new()).unwrap();
        assert_eq!((done.unmatched, done.capped), (1, false));
        assert_eq!(holders(&info, &beyond), [bip44()]);
        assert_eq!(info.balance.total(), VALUE);
    }

    /// Sol r1 M1 with the default gap: the stored address of a spent coin
    /// (CoinJoin 600) carries the search past its gap to the coin at 1,500,
    /// which no window from the pool reaches alone.
    #[test]
    fn a_spent_coins_stored_address_carries_the_search_to_a_coin_past_it() {
        let (wallet, loaded) = wallet();
        let bridge = coinjoin_address(&wallet, &loaded, 600);
        let deep = coinjoin_address(&wallet, &loaded, 1_500);

        let mut info = loaded.clone();
        let at = park(&mut info, &pays(1, &deep), &deep);
        restore_beyond_gap(&wallet, &mut info, &HashSet::new()).unwrap();
        assert_eq!(holders(&info, &at), [bip44()], "out of reach alone");

        let mut info = loaded;
        let at = park(&mut info, &pays(1, &deep), &deep);
        let stored = HashSet::from([bridge.script_pubkey(), deep.script_pubkey()]);
        let done = restore_beyond_gap(&wallet, &mut info, &stored).unwrap();
        assert_eq!((done.matched, done.found), (1, 2));
        assert_eq!(holders(&info, &at), [cj()]);
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        assert!(
            pool.address_info(&bridge).unwrap().is_used(),
            "never handed out again"
        );
        assert_eq!(info.balance.total(), VALUE);
    }

    /// A stored address a pool already derives is marked used and its gap
    /// kept, chaining to the next: no search (a stored lookahead's gap).
    #[test]
    fn stored_addresses_within_the_gap_chain_without_a_search() {
        let (wallet, loaded) = wallet();
        let addrs: Vec<Address> = [60, 150, 240]
            .iter()
            .map(|i| coinjoin_address(&wallet, &loaded, *i))
            .collect();
        let mut info = loaded;
        let at = park(&mut info, &pays(1, &addrs[2]), &addrs[2]);
        let stored: HashSet<ScriptBuf> = addrs.iter().map(|a| a.script_pubkey()).collect();

        let done = restore_beyond_gap(&wallet, &mut info, &stored).unwrap();

        assert_eq!((done.matched, done.found), (1, 0));
        assert_eq!(holders(&info, &at), [cj()]);
    }

    /// A coin on a DashPay contact's address (an output we paid them) is not
    /// looked for: it goes to the contact's account, out of the balance.
    #[test]
    fn a_contacts_coin_is_not_searched_for_and_leaves_the_funds_accounts() {
        let (wallet, mut info) = wallet();
        let theirs = crate::coins::insert_contact_account(&mut info, wallet.wallet_id);
        let outpoint = park(&mut info, &pays(1, &theirs), &theirs);
        assert_eq!(info.balance.total(), VALUE, "parked, it counts");

        let done = restore_beyond_gap(&wallet, &mut info, &HashSet::from([theirs.script_pubkey()]))
            .unwrap();

        assert_eq!((done.found, done.capped), (0, false), "no search");
        assert!(holders(&info, &outpoint).is_empty());
        assert!(
            (info.accounts.dashpay_external_accounts.values())
                .any(|a| a.utxos.contains_key(&outpoint)),
            "the contact's account holds it"
        );
        assert_eq!(info.balance.total(), 0);
    }

    /// Each address found opens another window, up to [`SEARCH_BUDGET`]
    /// indexes past the pool: coins chained a window apart go home until it
    /// ends; the next stays, counted once, and the search reports the cap.
    #[test]
    fn chained_coins_go_home_until_the_search_budget() {
        let (wallet, mut info) = wallet();
        let pool = &info
            .accounts
            .funds_account(&cj())
            .unwrap()
            .managed_account_type()
            .address_pools()[0];
        let (have, gap) = (pool.highest_generated.unwrap(), pool.gap_limit);
        let indexes: Vec<u32> = (0..)
            .map(|k| have + SEARCH_WINDOW + k * (gap + SEARCH_WINDOW))
            .take_while(|i| *i <= have + SEARCH_BUDGET + gap + SEARCH_WINDOW)
            .collect();
        let (inside, past) = indexes.split_at(indexes.len() - 1);
        assert!(past[0] > have + SEARCH_BUDGET);
        let mut coins = Vec::new();
        for (n, index) in indexes.iter().enumerate() {
            let to = coinjoin_address(&wallet, &info, *index);
            coins.push(park(&mut info, &pays(n as u8 + 1, &to), &to));
        }

        let done = restore_beyond_gap(&wallet, &mut info, &HashSet::new()).unwrap();

        assert_eq!(
            (done.matched, done.unmatched, done.capped),
            (inside.len(), 1, true)
        );
        for at in &coins[..inside.len()] {
            assert_eq!(holders(&info, at), [cj()]);
        }
        assert_eq!(holders(&info, coins.last().unwrap()), [bip44()]);
        assert_eq!(info.balance.total(), indexes.len() as u64 * VALUE);
    }

    /// The reconcile removes only a true duplicate: a coin one account holds
    /// (a CoinJoin coin, a BIP44 coin, or one the load left in BIP44) is left
    /// as it is.
    #[test]
    fn the_reconcile_leaves_single_copies_alone() {
        let (wallet, mut info) = wallet();
        let mixed = coinjoin_address(&wallet, &info, 5);
        let mixed_tx = pays(1, &mixed);
        let mixed_op = OutPoint::new(mixed_tx.txid(), 0);
        let utxo = Utxo::new(mixed_op, mixed_tx.output[0].clone(), mixed, HEIGHT, false);
        (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(mixed_op, utxo);
        let plain = wallet_receive_address(&wallet, &mut info);
        let plain_tx = pays(2, &plain);
        let plain_op = OutPoint::new(plain_tx.txid(), 0);
        let utxo = Utxo::new(plain_op, plain_tx.output[0].clone(), plain, HEIGHT, false);
        (info.accounts.funds_account_mut(&bip44()).unwrap().utxos).insert(plain_op, utxo);
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

    /// Without a restore, a replay counts a coin the load left in BIP44
    /// twice once a pool reaches it, and the reconcile alone brings it back
    /// to once, in CoinJoin.
    #[tokio::test]
    async fn the_reconcile_alone_undoes_a_replays_double_count() {
        let (mut wallet, mut info) = wallet();
        let to = coinjoin_address(&wallet, &info, 170);
        let tx = pays(1, &to);
        let outpoint = park(&mut info, &tx, &to);
        // The replay derives the CoinJoin pool up to the coin as it scans,
        // the way SPV's gap extension does through used addresses between.
        let ks = wallet.key_source_for_account(cj());
        let account = info.accounts.funds_account_mut(&cj()).unwrap();
        let pool = &mut account.managed_account_type_mut().address_pools_mut()[0];
        let have = pool.highest_generated.unwrap();
        pool.generate_addresses(170 - have, &ks, true).unwrap();
        replay(&mut wallet, &mut info, &tx).await;
        assert_eq!(info.balance.total(), 2 * VALUE, "the double count");
        assert_eq!(holders(&info, &outpoint), [bip44(), cj()]);

        assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
        assert_eq!(holders(&info, &outpoint), [cj()]);
        assert_eq!(info.balance.total(), VALUE);
        assert_eq!(reconcile_duplicates(&mut info), (0, 0), "nothing left");
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
                (info.accounts.funds_account_mut(&account).unwrap().utxos)
                    .insert(outpoint, utxo.clone());
            }
            info.update_balance();
            assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
            assert_eq!(holders(&info, &outpoint), [bip44()]);
            assert_eq!(info.balance.total(), VALUE);
        }
    }

    /// The reconcile never takes a coin from its owning account: a copy the
    /// owner holds is the one kept, whichever order the accounts come in.
    #[test]
    fn the_reconcile_keeps_the_owners_copy() {
        let (wallet, mut info) = wallet();
        let to = wallet_receive_address(&wallet, &mut info);
        let tx = pays(3, &to);
        let outpoint = OutPoint::new(tx.txid(), 0);
        let utxo = Utxo::new(outpoint, tx.output[0].clone(), to.clone(), HEIGHT, false);
        for account in [bip44(), cj()] {
            (info.accounts.funds_account_mut(&account).unwrap().utxos)
                .insert(outpoint, utxo.clone());
        }
        info.update_balance();
        assert_eq!(reconcile_duplicates(&mut info), (1, VALUE));
        assert_eq!(holders(&info, &outpoint), [bip44()]);
    }

    /// The read-lock check sees only true duplicates: never single copies in
    /// CoinJoin or BIP44, nor a coin the load left in BIP44 alone.
    #[test]
    fn has_duplicates_sees_only_one_outpoint_in_two_accounts() {
        let (wallet, mut info) = wallet();
        let plain = wallet_receive_address(&wallet, &mut info);
        let mixed = coinjoin_address(&wallet, &info, 5);
        let far = coinjoin_address(&wallet, &info, 170);
        park(&mut info, &pays(1, &plain), &plain);
        let mixed_op = park(&mut info, &pays(2, &mixed), &mixed);
        let coin = (info.accounts.funds_account_mut(&bip44()).unwrap().utxos).remove(&mixed_op);
        (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(mixed_op, coin.unwrap());
        let far_op = park(&mut info, &pays(3, &far), &far);
        assert!(
            !has_duplicates(&info),
            "single copies, one of them in BIP44"
        );

        let copy = info.accounts.funds_account(&bip44()).unwrap().utxos[&far_op].clone();
        (info.accounts.funds_account_mut(&cj()).unwrap().utxos).insert(far_op, copy);

        assert!(has_duplicates(&info));
        reconcile_duplicates(&mut info);
        assert!(!has_duplicates(&info));
    }
}
