//! Coin control: the UTXO list with labels, locks and confirmations, user
//! locks persisted in dw-appdb, and dash-qt's dust attack protection
//! (QT-068…075; docs/contracts/m1-engine.md §2.8).
//!
//! Locks are app metadata: key-wallet's own `Utxo::is_locked` flag is not
//! persisted, so the engine keeps "Lock unspent" and dust locks in
//! `app.sqlite` and applies them when it lists coins and builds candidate
//! sets for a payment.
//!
//! The inputs of the wallet's tracked `Built` and `Broadcast` asset locks
//! read as reserved (E0-04 §2a.3's F7 mitigation, H6 part 1 in Mode B):
//! such a lock may already be on the wire, and the catch-up resends it, so
//! no engine build may spend its inputs, neither by automatic selection nor
//! by coin control. They are read with the coins, from one wallet state,
//! so the exclusion holds from the first build of a session, before any
//! resume ran, and lifts once the row reaches a proof, is consumed or is
//! untracked. Repair's "Resolve stuck funding" self-spend (§6.5, §2a.5) is
//! the one build that must spend them: its draft names exactly those
//! inputs as exempt ([`WalletCoin::self_spend_selectable`]); every other
//! build still refuses them.
//!
//! **The coin fence** (E0-04 §2a.6, DP1-02 Sol r2 R2-04). One async mutex
//! per wallet ([`CoinFence`], [`NetworkSession::coin_fence`]) orders every
//! change of what a build may spend against every build that reads it:
//! - "Lock unspent"/"Unlock unspent" hold it across their check and
//!   persist;
//! - a coin read ([`NetworkSession::coin_snapshot`]) needs it, so dust
//!   locks are only applied under it;
//! - an engine build (a `TxDraft`'s prepare, Repair's self-spend, CoinJoin's
//!   denomination and collateral funding, "Move mixed coins") holds one
//!   fence from its coin read through its selection, its signing and the
//!   reservation of its inputs ([`crate::send::PendingSpends`]);
//! - a library funding call holds the fence its policy check read the coins
//!   under ([`NetworkSession::fenced_coins`]) inside its call permit, until
//!   the library has tracked the lock it signed (`Built`, whose inputs then
//!   read reserved) or the call ends
//!   ([`NetworkSession::funding_call_permit`]).
//!
//! So a coin locked after a build read the coins is never signed by that
//! build: the lock waits for the build's reservation, and a build that
//! starts after the lock reads it.
//!
//! Lock order: the fence before `wallet.state()` and the wallet manager
//! (no holder of either awaits the fence), and never inside J. A holder
//! passes its fence down and never asks for it again (the mutex is not
//! reentrant). Nothing the library or SPV can run while a fence is held
//! asks for one: the changeset tap, Mode B's refresh, the fee bound, the
//! event handlers and the catch-up's resume pass read no coins through it.
//! CoinJoin sessions (denominations of the CoinJoin account, unattended)
//! read their coins under a fence they release at once and reserve their
//! inputs outside it.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashcore::{Address, OutPoint, Txid};
use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE, LabelKind};
use key_wallet::Utxo;
use key_wallet::bip32::ChildNumber;
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use key_wallet::managed_account::transaction_record::TransactionDirection;
use key_wallet::wallet::managed_wallet_info::ManagedWalletInfo;
use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
use platform_wallet::wallet::platform_wallet::PlatformWallet;

use crate::{EngineError, NetworkSession, WalletId};

/// Settings key of the dust protection threshold (duffs; absent = off).
pub const DUST_PROTECTION_KEY: &str = "dust_protection_threshold";
/// dash-qt's range for the threshold (QT-075).
pub const DUST_PROTECTION_MAX: u64 = 1_000_000;
/// CoinJoin denominations in duffs (Dash Core `CoinJoin::GetStandardDenominations`).
pub const COINJOIN_DENOMINATIONS: [u64; 5] =
    [1_000_010_000, 100_001_000, 10_000_100, 1_000_010, 100_001];

/// Wall clock in UNIX seconds.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `address` is one of the wallet's derived addresses.
pub(crate) fn wallet_owns(info: &ManagedWalletInfo, address: &Address) -> bool {
    info.all_managed_accounts()
        .into_iter()
        .any(|account| account.get_address_info(address).is_some())
}

/// One unspent output of the wallet with what coin control shows about it.
#[derive(Debug, Clone)]
pub(crate) struct WalletCoin {
    pub utxo: Utxo,
    /// Held by an account a plain send funds from (BIP44, BIP32, DashPay
    /// receiving), as opposed to CoinJoin.
    pub send_account: bool,
    pub coinjoin_account: bool,
    /// On an internal (change) chain.
    pub is_change: bool,
    pub block_time: Option<u64>,
    pub chain_locked: bool,
    /// The funding transaction spent none of the wallet's coins (or, when
    /// key-wallet no longer has the record, the coin is not change).
    pub foreign_incoming: bool,
    pub user_locked: bool,
    pub reserved: bool,
    /// Reserved only as the input of a tracked `Built` or `Broadcast`
    /// asset lock (module doc), not by a payment of the engine.
    pub lock_input: bool,
}

impl WalletCoin {
    /// Dash Core `IsTrusted`: confirmed, InstantSend-locked, or our own
    /// unconfirmed change.
    fn trusted(&self) -> bool {
        self.utxo.is_confirmed || self.utxo.is_instantlocked || self.utxo.is_trusted
    }

    /// Eligible for automatic coin selection.
    pub(crate) fn auto_selectable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && self.trusted() && !self.user_locked && !self.reserved
    }

    /// Eligible when picked by hand (coin control may spend untrusted
    /// unconfirmed coins, never locked, reserved or immature ones).
    pub(crate) fn chosen_selectable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && !self.user_locked && !self.reserved
    }

    /// Eligible for Repair's self-spend when the draft exempts it (E0-04
    /// §6.5): as when picked by hand, except that a tracked asset lock's
    /// reservation does not hold it back.
    pub(crate) fn self_spend_selectable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && !self.user_locked && (!self.reserved || self.lock_input)
    }

    fn denominated(&self) -> bool {
        self.coinjoin_account && COINJOIN_DENOMINATIONS.contains(&self.utxo.value())
    }
}

/// Every unspent output of a wallet at its processed height.
#[derive(Debug, Clone)]
pub(crate) struct CoinSnapshot {
    pub height: u32,
    pub coins: Vec<WalletCoin>,
}

impl CoinSnapshot {
    /// The coins of the funding accounts (BIP44, BIP32, DashPay receiving)
    /// the user locked or a pending payment of the engine reserves: what
    /// the library's asset-lock builder does not see. DP1-02's funding
    /// refuses while any stands, checked under the fence its call permit
    /// then holds (module doc). A tracked lock's input (`lock_input`) is
    /// not among them: the library reserves those itself.
    #[cfg_attr(not(test), expect(dead_code, reason = "DP1-02's funding policy check"))]
    pub(crate) fn held_funding_coins(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.coins
            .iter()
            .filter(|c| c.send_account && (c.user_locked || (c.reserved && !c.lock_input)))
            .map(|c| c.utxo.outpoint)
    }
}

/// Proof that the caller holds a wallet's coin fence (module doc): no
/// coin lock, dust lock, reservation or other build of the wallet changes
/// what may be spent until it is dropped. Taken with
/// [`NetworkSession::coin_fence`].
pub struct CoinFence {
    wallet: WalletId,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl CoinFence {
    pub fn wallet_id(&self) -> WalletId {
        self.wallet
    }
}

impl std::fmt::Debug for CoinFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoinFence")
            .field("wallet", &self.wallet)
            .finish_non_exhaustive()
    }
}

/// The session's coin fences, one per wallet, made on first use.
#[derive(Default)]
pub(crate) struct CoinFences {
    fences: std::sync::Mutex<HashMap<WalletId, Arc<tokio::sync::Mutex<()>>>>,
    /// Tests: holds the next `TxDraft` prepare after its coin read, its
    /// fence held.
    #[cfg(test)]
    pub(crate) pause_prepare: std::sync::Mutex<Option<Arc<TestPause>>>,
}

/// A one-shot pause point for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestPause {
    pub(crate) reached: std::sync::atomic::AtomicBool,
    pub(crate) release: tokio::sync::Notify,
}

impl CoinFences {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<WalletId, Arc<tokio::sync::Mutex<()>>>> {
        self.fences.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Waits for `wallet`'s fence.
    pub(crate) async fn acquire(&self, wallet: WalletId) -> CoinFence {
        let fence = Arc::clone(self.map().entry(wallet).or_default());
        CoinFence {
            wallet,
            _guard: fence.lock_owned().await,
        }
    }

    /// `wallet`'s fence if no one holds it now.
    pub(crate) fn try_acquire(&self, wallet: WalletId) -> Option<CoinFence> {
        let fence = Arc::clone(self.map().entry(wallet).or_default());
        Some(CoinFence {
            wallet,
            _guard: fence.try_lock_owned().ok()?,
        })
    }

    /// Drops a removed or unloaded wallet's fence unless it is held or
    /// awaited (a new one for the same id would let two holders in).
    pub(crate) fn forget(&self, wallet: &WalletId) {
        let mut map = self.map();
        if map.get(wallet).is_some_and(|f| Arc::strong_count(f) == 1) {
            map.remove(wallet);
        }
    }

    /// Tests: the pause point of a prepare that holds its fence.
    pub(crate) async fn prepare_pause_point(&self) {
        #[cfg(test)]
        {
            let pause = self
                .pause_prepare
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(pause) = pause {
                pause
                    .reached
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                pause.release.notified().await;
            }
        }
    }
}

/// UTXO list filter (contract `UtxoFilter`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoinFilter {
    pub include_locked: bool,
    pub fully_mixed_only: bool,
    pub min_confirmations: Option<u32>,
}

/// One row of the coin control list (contract `Utxo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoinInfo {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub block_height: Option<u32>,
    /// Block time; `None` while unconfirmed or once the record was pruned.
    pub timestamp: Option<u64>,
    pub instant_locked: bool,
    pub chain_locked: bool,
    pub user_locked: bool,
    pub reserved: bool,
    pub label: Option<String>,
    pub is_change: bool,
    pub is_coinbase: bool,
    pub coinjoin_denominated: bool,
    /// Rounds of a CoinJoin denomination (input-chain walk); `None` for
    /// other coins and unknown chains ("n/a").
    pub coinjoin_rounds: Option<u32>,
    pub spendable: bool,
}

/// `(txid display hex, vout)`, the dw-appdb key of an outpoint.
fn lock_key(o: &OutPoint) -> (String, u32) {
    (o.txid.to_string(), o.vout)
}

fn is_internal_path(path: &key_wallet::DerivationPath) -> bool {
    let p: &[ChildNumber] = path.as_ref();
    p.len() >= 2 && p[p.len() - 2] == ChildNumber::Normal { index: 1 }
}

fn collect(
    coins: &mut Vec<WalletCoin>,
    account: &ManagedCoreFundsAccount,
    send_account: bool,
    coinjoin_account: bool,
) {
    for utxo in account.utxos.values() {
        let txid = utxo.outpoint.txid;
        let record = account.transactions().get(&txid);
        let is_change = !coinjoin_account
            && account
                .get_address_info(&utxo.address)
                .is_some_and(|info| is_internal_path(&info.path));
        coins.push(WalletCoin {
            utxo: utxo.clone(),
            send_account,
            coinjoin_account,
            is_change,
            block_time: record
                .and_then(|r| r.context.block_info())
                .map(|b| u64::from(b.timestamp())),
            chain_locked: record.is_some_and(|r| r.context.is_chain_locked())
                || account.transaction_is_finalized(&txid),
            // key-wallet drops the record of a ChainLocked transaction
            // (only its txid survives), so the direction is often gone by
            // the time coins are read. Without it, a coin on a receiving
            // (non-change) address counts as foreign: dust protection then
            // locks it, and the user can unlock it.
            foreign_incoming: record.map_or(!is_change, |r| {
                r.direction == TransactionDirection::Incoming
            }),
            user_locked: false,
            reserved: false,
            lock_input: false,
        });
    }
}

/// The inputs of the tracked asset locks that may be sent (`Built`,
/// `Broadcast`): reserved for them (module doc).
fn pending_lock_inputs<'a>(
    locks: impl IntoIterator<Item = &'a platform_wallet::TrackedAssetLock>,
) -> HashSet<OutPoint> {
    locks
        .into_iter()
        .filter(|l| crate::platform::lease::TrackedStatus::from(&l.status).pending())
        .flat_map(|l| l.transaction.input.iter().map(|i| i.previous_output))
        .collect()
}

/// Reads every funds account of `info` (key-wallet state only).
fn read_coins(info: &ManagedWalletInfo) -> CoinSnapshot {
    let accounts = &info.accounts;
    let mut coins = Vec::new();
    for a in accounts.standard_bip44_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.standard_bip32_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.dashpay_receival_accounts.values() {
        collect(&mut coins, a, true, false);
    }
    for a in accounts.coinjoin_accounts.values() {
        collect(&mut coins, a, false, true);
    }
    CoinSnapshot {
        height: info.last_processed_height(),
        coins,
    }
}

fn parse_threshold(value: Option<String>) -> Result<Option<u64>, AppDbError> {
    value
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| AppDbError::Corrupt(format!("{DUST_PROTECTION_KEY} {v:?}")))
        })
        .transpose()
}

/// Whether the wallet holds `txid` as ChainLocked in any of its funds
/// accounts: its record's context, or the finalized txid key-wallet keeps
/// once it drops a ChainLocked record (as [`collect`] reads a coin's).
pub(crate) fn tx_chain_locked(info: &ManagedWalletInfo, txid: &Txid) -> bool {
    let accounts = &info.accounts;
    let locked = |a: &ManagedCoreFundsAccount| {
        a.transactions()
            .get(txid)
            .is_some_and(|r| r.context.is_chain_locked())
            || a.transaction_is_finalized(txid)
    };
    accounts.standard_bip44_accounts.values().any(locked)
        || accounts.standard_bip32_accounts.values().any(locked)
        || accounts.dashpay_receival_accounts.values().any(locked)
        || accounts.coinjoin_accounts.values().any(locked)
}

/// A transaction of the funding accounts (BIP44, BIP32, DashPay receiving)
/// that spends one of the wallet's coins and is not ChainLocked yet, if
/// any. Repair's self-spend waits for none (`platform/lease/repair.rs`):
/// a coin such a transaction spends is no coin of the self-spend, so a lost
/// asset lock that conflicts only with it is not beaten by the self-spend.
pub(crate) fn unsettled_spend(info: &ManagedWalletInfo) -> Option<Txid> {
    let accounts = &info.accounts;
    let pending = |a: &ManagedCoreFundsAccount| {
        a.transactions()
            .values()
            .find(|r| !r.input_details.is_empty() && !r.context.is_chain_locked())
            .map(|r| r.txid)
    };
    accounts
        .standard_bip44_accounts
        .values()
        .chain(accounts.standard_bip32_accounts.values())
        .chain(accounts.dashpay_receival_accounts.values())
        .find_map(pending)
}

/// How many unspent outputs the asset-lock builder may fund from: every coin
/// of the accounts platform-wallet's `ASSET_LOCK_FUNDING_SOURCES` pools
/// (BIP44, BIP32, DashPay receiving), whatever its maturity, locks or
/// reservations, which the library does not all see (E0-04 §2a.3's
/// `fee_bound_worst` counts a superset, so its bound stays an upper one).
pub(crate) async fn funding_coin_count(wallet: &PlatformWallet) -> usize {
    let state = wallet.state().await;
    read_coins(&state.core_wallet)
        .coins
        .iter()
        .filter(|c| c.send_account)
        .count()
}

impl NetworkSession {
    /// Runs a dw-appdb call on the blocking pool.
    pub(crate) async fn appdb_op<T, F>(&self, f: F) -> Result<T, EngineError>
    where
        F: FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
        T: Send + 'static,
    {
        let db = self.live()?.appdb;
        tokio::task::spawn_blocking(move || f(&db))
            .await?
            .map_err(EngineError::from)
    }

    /// Waits for `wallet_id`'s coin fence (module doc). A holder passes it
    /// down and never asks again: the fence is not reentrant. Not to be
    /// awaited while holding the wallet's state or manager lock.
    /// `WalletNotFound` for a wallet that is not loaded.
    pub async fn coin_fence(&self, wallet_id: WalletId) -> Result<CoinFence, EngineError> {
        self.wallet(&wallet_id).await?;
        Ok(self.coin_fences.acquire(wallet_id).await)
    }

    /// [`Self::coin_fence`] if no one holds the fence now (`Ok(None)`
    /// otherwise): for reads that may be skipped.
    pub(crate) async fn try_coin_fence(
        &self,
        wallet_id: WalletId,
    ) -> Result<Option<CoinFence>, EngineError> {
        self.wallet(&wallet_id).await?;
        Ok(self.coin_fences.try_acquire(wallet_id))
    }

    /// The fenced wallet's coins ([`Self::coin_snapshot`]), read under
    /// `fence`: what DP1-02's funding policy checks
    /// ([`CoinSnapshot::held_funding_coins`]) before it hands the same fence
    /// to [`Self::funding_call_permit`].
    #[cfg_attr(not(test), expect(dead_code, reason = "DP1-02's funding policy check"))]
    pub(crate) async fn fenced_coins(
        &self,
        fence: &CoinFence,
    ) -> Result<CoinSnapshot, EngineError> {
        let wallet = self.wallet(&fence.wallet).await?;
        self.coin_snapshot(fence, &wallet).await
    }

    /// The fenced wallet's coins with user/dust locks and reservations
    /// applied. With dust protection on, first dust-locks small foreign
    /// incoming coins (Dash Core: value ≤ threshold, funding transaction
    /// not ours; a released dust lock is not renewed): only under the
    /// fence (module doc).
    pub(crate) async fn coin_snapshot(
        &self,
        fence: &CoinFence,
        wallet: &PlatformWallet,
    ) -> Result<CoinSnapshot, EngineError> {
        let wallet_id = fence.wallet;
        debug_assert_eq!(
            wallet.wallet_id(),
            wallet_id.0,
            "the fence of another wallet"
        );
        let (mut snapshot, lock_inputs) = {
            let state = wallet.state().await;
            (
                read_coins(&state.core_wallet),
                pending_lock_inputs(state.tracked_asset_locks.values()),
            )
        };
        let id = wallet_id.to_string();
        let dust_candidates: Vec<(u64, String, u32)> = snapshot
            .coins
            .iter()
            .filter(|c| c.foreign_incoming && !c.is_change && !c.utxo.is_coinbase)
            .map(|c| {
                let (txid, vout) = lock_key(&c.utxo.outpoint);
                (c.utxo.value(), txid, vout)
            })
            .collect();
        let locked: HashSet<(String, u32)> = self
            .appdb_op(move |db| {
                if let Some(threshold) =
                    parse_threshold(db.setting(GLOBAL_SCOPE, DUST_PROTECTION_KEY)?)?
                {
                    let now = now_secs();
                    for (value, txid, vout) in &dust_candidates {
                        if *value <= threshold {
                            db.lock_dust(&id, txid, *vout, now)?;
                        }
                    }
                }
                Ok(db
                    .locks(&id)?
                    .into_iter()
                    .filter(|row| row.is_locked())
                    .map(|row| (row.txid, row.vout))
                    .collect())
            })
            .await?;
        let unspent: HashSet<OutPoint> = snapshot.coins.iter().map(|c| c.utxo.outpoint).collect();
        self.spends.retain_unspent(&wallet_id, &unspent);
        let reserved = self.spends.snapshot(&wallet_id);
        for coin in &mut snapshot.coins {
            let o = coin.utxo.outpoint;
            coin.user_locked = locked.contains(&lock_key(&o));
            coin.lock_input = !reserved.contains(&o) && lock_inputs.contains(&o);
            coin.reserved = reserved.contains(&o) || lock_inputs.contains(&o);
        }
        Ok(snapshot)
    }

    /// Coin control list of `wallet_id`, largest first (dash-qt's default
    /// sort), with each CoinJoin denomination's mixing rounds (dash-qt's
    /// "Mixing Rounds" column, the input-chain walk; `None` shows "n/a").
    /// `fully_mixed_only` lists the CoinJoin page's coins (QT-071): fully
    /// mixed denominations by the QT-043 rule.
    pub async fn utxos(
        self: &Arc<Self>,
        wallet_id: WalletId,
        filter: CoinFilter,
    ) -> Result<Vec<CoinInfo>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let view = this.read_mix_view(wallet_id).await?;
            let has_keys = this.vault.has_wallet_secret(&wallet_id.0);
            let id = wallet_id.to_string();
            let labels: HashMap<String, String> = this
                .appdb_op(move |db| Ok(db.labels(&id, LabelKind::Address)?.into_iter().collect()))
                .await?;
            let height = view.height;
            let mut rows: Vec<CoinInfo> = view
                .coins
                .iter()
                .filter(|m| filter.include_locked || !m.coin.user_locked)
                .filter(|m| !filter.fully_mixed_only || (m.denominated() && m.fully_mixed))
                .map(|m| {
                    let c = &m.coin;
                    let u = &c.utxo;
                    let address = u.address.to_string();
                    CoinInfo {
                        outpoint: u.outpoint,
                        label: labels.get(&address).cloned(),
                        address,
                        amount: u.value(),
                        confirmations: u.confirmations(height),
                        block_height: u.is_confirmed.then_some(u.height),
                        timestamp: c.block_time,
                        instant_locked: u.is_instantlocked,
                        chain_locked: c.chain_locked,
                        user_locked: c.user_locked,
                        reserved: c.reserved,
                        is_change: c.is_change,
                        is_coinbase: u.is_coinbase,
                        coinjoin_denominated: c.denominated(),
                        coinjoin_rounds: (m.denominated() && m.rounds >= 0)
                            .then_some(m.rounds as u32),
                        spendable: has_keys && c.chosen_selectable(height),
                    }
                })
                .filter(|row| {
                    filter
                        .min_confirmations
                        .is_none_or(|n| row.confirmations >= n)
                })
                .collect();
            rows.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.outpoint.cmp(&b.outpoint)));
            Ok(rows)
        })
        .await
    }

    /// Checks that every outpoint is an unspent output of the wallet.
    async fn require_unspent(
        &self,
        wallet_id: WalletId,
        outpoints: &[OutPoint],
    ) -> Result<(), EngineError> {
        let wallet = self.wallet(&wallet_id).await?;
        let unspent: HashSet<OutPoint> = {
            let state = wallet.state().await;
            read_coins(&state.core_wallet)
                .coins
                .into_iter()
                .map(|c| c.utxo.outpoint)
                .collect()
        };
        match outpoints.iter().find(|o| !unspent.contains(o)) {
            Some(o) => Err(EngineError::OutpointNotFound(*o)),
            None => Ok(()),
        }
    }

    /// "Lock unspent" (QT-070): excludes the outpoints from automatic
    /// selection and coin control until unlocked. Persisted. Holds the
    /// wallet's coin fence across its check and write, so it waits for a
    /// build that read the coins before it to reserve its inputs (module
    /// doc).
    pub async fn lock_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let _fence = this.coin_fence(wallet_id).await?;
            this.require_unspent(wallet_id, &outpoints).await?;
            let id = wallet_id.to_string();
            this.appdb_op(move |db| {
                let now = now_secs();
                for o in &outpoints {
                    let (txid, vout) = lock_key(o);
                    db.lock_manual(&id, &txid, vout, now)?;
                }
                Ok(())
            })
            .await
        })
        .await
    }

    /// "Unlock unspent": deletes a user lock or releases a dust lock (which
    /// dust protection then leaves alone). An outpoint that is neither
    /// locked nor an unspent output is `OutpointNotFound`. Under the coin
    /// fence, as "Lock unspent".
    pub async fn unlock_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let _fence = this.coin_fence(wallet_id).await?;
            let id = wallet_id.to_string();
            let keys: Vec<(String, u32)> = outpoints.iter().map(lock_key).collect();
            let active: HashSet<(String, u32)> = this
                .appdb_op({
                    let id = id.clone();
                    move |db| {
                        Ok(db
                            .locks(&id)?
                            .into_iter()
                            .filter(|r| r.is_locked())
                            .map(|r| (r.txid, r.vout))
                            .collect())
                    }
                })
                .await?;
            let unlisted: Vec<OutPoint> = outpoints
                .iter()
                .zip(&keys)
                .filter(|(_, k)| !active.contains(*k))
                .map(|(o, _)| *o)
                .collect();
            this.require_unspent(wallet_id, &unlisted).await?;
            this.appdb_op(move |db| {
                let now = now_secs();
                for (txid, vout) in &keys {
                    db.unlock(&id, txid, *vout, now)?;
                }
                Ok(())
            })
            .await
        })
        .await
    }

    /// Outpoints currently locked (user or dust), oldest first.
    pub async fn locked_outpoints(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<Vec<OutPoint>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.wallet(&wallet_id).await?;
            let id = wallet_id.to_string();
            let rows = this.appdb_op(move |db| db.locks(&id)).await?;
            rows.into_iter()
                .filter(|r| r.is_locked())
                .map(|r| {
                    Txid::from_str(&r.txid)
                        .map(|txid| OutPoint::new(txid, r.vout))
                        .map_err(|_| EngineError::Storage(format!("bad lock txid {:?}", r.txid)))
                })
                .collect()
        })
        .await
    }

    /// Dust attack protection threshold in duffs (`None` = off; QT-075).
    pub async fn dust_protection(self: &Arc<Self>) -> Result<Option<u64>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.manager()?;
            this.appdb_op(|db| parse_threshold(db.setting(GLOBAL_SCOPE, DUST_PROTECTION_KEY)?))
                .await
        })
        .await
    }

    /// Turns dust protection on (1…1,000,000 duffs) or off. Coins are locked
    /// the next time the wallet's coins are read.
    pub async fn set_dust_protection(
        self: &Arc<Self>,
        threshold: Option<u64>,
    ) -> Result<(), EngineError> {
        if let Some(t) = threshold
            && !(1..=DUST_PROTECTION_MAX).contains(&t)
        {
            return Err(EngineError::InvalidArgument(format!(
                "dust threshold {t} is outside 1..={DUST_PROTECTION_MAX}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.manager()?;
            this.appdb_op(move |db| {
                db.set_setting(
                    GLOBAL_SCOPE,
                    DUST_PROTECTION_KEY,
                    threshold.map(|t| t.to_string()).as_deref(),
                )
            })
            .await
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_chain_detection() {
        let change = key_wallet::DerivationPath::from_str("m/44'/1'/0'/1/7").unwrap();
        let receive = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/7").unwrap();
        assert!(is_internal_path(&change));
        assert!(!is_internal_path(&receive));
    }

    #[test]
    fn threshold_parsing() {
        assert_eq!(parse_threshold(None).unwrap(), None);
        assert_eq!(parse_threshold(Some("10000".into())).unwrap(), Some(10_000));
        assert!(parse_threshold(Some("x".into())).is_err());
    }

    /// One fence per wallet, never reentrant; `try_acquire` refuses a held
    /// one. A removed wallet's fence is forgotten only when idle: a held
    /// one stays, so a later holder still waits for it.
    #[tokio::test]
    async fn a_wallets_fence_is_forgotten_only_when_idle() {
        let fences = CoinFences::default();
        let (w, other) = (WalletId([1; 32]), WalletId([2; 32]));
        let held = fences.acquire(w).await;
        assert!(fences.try_acquire(w).is_none());
        drop(fences.try_acquire(other).expect("another wallet's is free"));
        fences.forget(&w);
        assert!(fences.map().contains_key(&w), "a held fence is kept");
        assert!(fences.try_acquire(w).is_none(), "and still excludes");
        drop(held);
        drop(fences.try_acquire(w).expect("released"));
        fences.forget(&w);
        fences.forget(&other);
        assert!(fences.map().is_empty(), "idle fences are forgotten");
    }
}
