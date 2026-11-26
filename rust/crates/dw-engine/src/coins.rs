//! Coin control: the UTXO list with labels, locks and confirmations, user
//! locks persisted in dw-appdb, and dash-qt's dust attack protection
//! (QT-068…075; docs/contracts/m1-engine.md §2.8).
//!
//! Locks are app metadata: the engine keeps "Lock unspent" and dust locks
//! in `app.sqlite` and applies them when it lists coins and builds
//! candidate sets for a payment. Into key-wallet's own lock set
//! (`ManagedWalletInfo::locked_outpoints`, where the library keeps and
//! stores masternode collaterals) the engine puts only a funding fence's
//! denomination holds ([`NetworkSession::exclude_denominations_under_fence`]),
//! and never hands those to the persister.
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
//! build still refuses them. The inputs of a lock a funding call signed
//! whose step is unresolved read the same way once its row is gone
//! ([`WalletCoin::unresolved_lock`], review Opus O-2): the library no
//! longer reserves them, and Repair's self-spend must take one of each.
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
//!   reservation of its inputs ([`crate::send::PendingSpends`], under the
//!   build's own [`crate::send::SpendOwner`], DEC-179);
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
//! inputs outside it; their final-input signing takes the fence again to
//! read the funding holds, through the signing (review P2b r7).
//!
//! `broadcast_psbt` alone takes more than one fence: those of the wallets
//! owning an input, in ascending wallet id, held through its send. Every
//! other holder takes one and waits for no other, so no cycle forms.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use dashcore::{Address, OutPoint, Txid};
use dw_appdb::{AppDb, AppDbError, GLOBAL_SCOPE, LabelKind, LockReason};
use key_wallet::Utxo;
use key_wallet::bip32::ChildNumber;
use key_wallet::managed_account::ManagedCoreFundsAccount;
use key_wallet::managed_account::address_pool::AddressInfo;
use key_wallet::managed_account::managed_account_collection::ManagedAccountCollection;
use key_wallet::managed_account::managed_account_ref::ManagedAccountRef;
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

/// Wall clock in UNIX seconds.
pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The wallet's own record of `address` (its derivation path and key), if
/// it is one of the wallet's derived addresses. A DashPay contact's
/// external account holds the contact's addresses, derived from the
/// contact's xpub (the library's `is_contact_owned`): they are not the
/// wallet's, so a payment to one leaves it, every cap counts it, and the
/// wallet neither signs for one nor claims it in a PSBT (MG-05 D19).
pub(crate) fn own_address_info(info: &ManagedWalletInfo, address: &Address) -> Option<AddressInfo> {
    own_accounts(info).find_map(|account| account.get_address_info(address))
}

/// [`own_address_info`] in an account that holds funds. key-wallet tracks
/// no coin in a keys-only account (identity, asset-lock, DIP-3 provider
/// keys), so a payment to one of its addresses leaves the wallet: every
/// cap counts it and it shows going out (MG-04 D55).
pub(crate) fn own_funds_address_info(
    info: &ManagedWalletInfo,
    address: &Address,
) -> Option<AddressInfo> {
    own_accounts(info)
        .filter(|account| account.as_funds().is_some())
        .find_map(|account| account.get_address_info(address))
}

fn own_accounts(info: &ManagedWalletInfo) -> impl Iterator<Item = ManagedAccountRef<'_>> {
    info.all_managed_accounts()
        .into_iter()
        .filter(|account| !account.managed_account_type().is_contact_owned())
}

/// Whether `address` is one of the wallet's derived addresses
/// ([`own_address_info`]).
pub(crate) fn wallet_owns(info: &ManagedWalletInfo, address: &Address) -> bool {
    own_address_info(info, address).is_some()
}

/// Whether a payment to `address` stays in the wallet's funds
/// ([`own_funds_address_info`]).
pub(crate) fn wallet_funds_address(info: &ManagedWalletInfo, address: &Address) -> bool {
    own_funds_address_info(info, address).is_some()
}

/// Tests: adds a DashPay contact's external account (watch-only, from a
/// fixed xpub) to the wallet `id` and returns the contact's first address,
/// which the account lists and the wallet does not own (MG-05 D19).
#[cfg(test)]
pub(crate) fn insert_contact_account(info: &mut ManagedWalletInfo, id: [u8; 32]) -> Address {
    use key_wallet::account::AccountType;
    use key_wallet::bip32::{ExtendedPrivKey, ExtendedPubKey};

    let network = dashcore::Network::Regtest;
    let master = ExtendedPrivKey::new_master(network, &[0x42; 32]).unwrap();
    let xpub = ExtendedPubKey::from_priv(&master);
    let child = xpub
        .derive_pub(&[ChildNumber::Normal { index: 0 }])
        .unwrap();
    let contact = Address::p2pkh(&dashcore::PublicKey::new(child.public_key), network);
    let managed = ManagedCoreFundsAccount::from_account(&key_wallet::Account {
        parent_wallet_id: Some(id),
        account_type: AccountType::DashpayExternalAccount {
            index: 0,
            user_identity_id: [1; 32],
            friend_identity_id: [2; 32],
        },
        network,
        account_xpub: xpub,
        is_watch_only: true,
    })
    .unwrap();
    info.accounts.insert_funds_bearing_account(managed).unwrap();
    let listed = info
        .all_managed_accounts()
        .into_iter()
        .any(|a| a.get_address_info(&contact).is_some());
    assert!(listed && !wallet_owns(info, &contact), "listed, not owned");
    contact
}

/// One unspent output of the wallet with what coin control shows about it.
#[derive(Debug, Clone)]
pub(crate) struct WalletCoin {
    pub utxo: Utxo,
    /// Held by an account a plain send funds from (BIP44, BIP32, DashPay
    /// receiving), as opposed to CoinJoin.
    pub send_account: bool,
    pub coinjoin_account: bool,
    /// A denomination on the CoinJoin account or a BIP44 receive or change
    /// address: Dash Core would count it as mixed or still mixing.
    pub counted_denomination: bool,
    /// On an internal (change) chain.
    pub is_change: bool,
    pub block_time: Option<u64>,
    pub chain_locked: bool,
    /// The funding transaction spent none of the wallet's coins (or, when
    /// key-wallet no longer has the record, the coin is not change).
    pub foreign_incoming: bool,
    /// Locked by the user or by dust protection (`dust_locked` says which).
    pub user_locked: bool,
    /// The lock is dust protection's (QT-075), not the user's.
    pub dust_locked: bool,
    pub reserved: bool,
    /// Reserved only as the input of a tracked `Built` or `Broadcast`
    /// asset lock, or of an unresolved signed one (module doc), not by a
    /// payment of the engine.
    pub lock_input: bool,
    /// An input of an asset lock a funding call signed, whose funding step
    /// reads neither `Sent` nor `NotSent`, and of no tracked pending lock:
    /// nothing reserves it in the library (review Opus O-2).
    pub unresolved_lock: bool,
    /// Reserved while its transaction's event hasn't reached the history
    /// ([`NetworkSession::coin_snapshot`]).
    pub awaiting_history: bool,
    /// Kept out of spending and every balance, and why
    /// ([`crate::withhold`]): reserved as well, so every rule that skips a
    /// reserved coin skips it.
    pub excluded: Option<crate::ExclusionReason>,
}

impl WalletCoin {
    /// Dash Core `IsTrusted`: confirmed, InstantSend-locked, or our own
    /// unconfirmed change.
    fn trusted(&self) -> bool {
        self.utxo.is_confirmed || self.utxo.is_instantlocked || self.utxo.is_trusted
    }

    /// Spendable now without coin control: trusted, neither locked nor
    /// reserved, a denomination included.
    pub(crate) fn trusted_spendable(&self, height: u32) -> bool {
        self.utxo.is_spendable(height) && self.trusted() && !self.user_locked && !self.reserved
    }

    /// Eligible for automatic ordinary selection. A denomination is not:
    /// Pay, a move to Shielded and an asset-lock funding leave it alone.
    pub(crate) fn auto_selectable(&self, height: u32) -> bool {
        self.trusted_spendable(height) && !self.counted_denomination
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
}

/// Every unspent output of a wallet at its processed height.
#[derive(Debug, Clone)]
pub(crate) struct CoinSnapshot {
    pub height: u32,
    pub coins: Vec<WalletCoin>,
}

/// Why a funding-account coin holds a library funding build back (E0-04
/// §2a.6): the pinned asset-lock builder takes no exclusion set, so it
/// would select the coin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CoinHold {
    /// An input of a pending payment of the engine: it clears when the
    /// payment settles or is released.
    Reserved,
    /// "Lock unspent" by the user.
    UserLock,
    /// Dust protection's lock. Spending the coin would link its address to
    /// the wallet's others (the lock's purpose), so it is not exempted; the
    /// UI can offer "release this dust coin".
    DustLock,
    /// An input of an asset lock a funding call signed whose step is not
    /// resolved, its row gone (review Opus O-2): spent by another lock, it
    /// would leave Repair no input of it to take. It clears once Repair's
    /// self-spend resolves the step, or its lock is proved.
    UnresolvedLock,
    /// A coin of a masternode collateral amount whose transaction's event
    /// hasn't reached the history: it may be a ProRegTx's collateral.
    AwaitingHistory,
}

/// One coin that holds a funding build back, for the refusal to name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HeldCoin {
    pub outpoint: OutPoint,
    pub value: u64,
    pub hold: CoinHold,
}

impl CoinSnapshot {
    /// The coins no lock, reservation or hold keeps back: all a library
    /// funding under this read's fence may spend.
    pub(crate) fn free_outpoints(&self) -> HashSet<OutPoint> {
        (self.coins.iter())
            .filter(|c| {
                !(c.reserved || c.user_locked || c.dust_locked || c.unresolved_lock)
                    && !(c.send_account && c.counted_denomination)
            })
            .map(|c| c.utxo.outpoint)
            .collect()
    }

    /// The coins of the funding accounts (BIP44, BIP32, DashPay receiving)
    /// the user or dust protection locked, or a pending payment of the
    /// engine reserves: what the library's asset-lock builder does not see.
    /// DP1-02's funding refuses while any stands, naming each with its
    /// hold, checked under the fence its call permit then holds (module
    /// doc). A coin both reserved and locked reads `Reserved`: releasing
    /// its lock would not free it. A tracked lock's input (`lock_input`) is
    /// not among them: the library reserves those itself. An unresolved
    /// signed lock's input is ([`CoinHold::UnresolvedLock`]). An excluded
    /// coin is not either: the funding's fence locks it in key-wallet for
    /// the build ([`NetworkSession::exclude_denominations_under_fence`]).
    pub(crate) fn held_funding_coins(&self) -> impl Iterator<Item = HeldCoin> + '_ {
        self.coins
            .iter()
            .filter(|c| c.send_account && c.excluded.is_none())
            .filter_map(|c| {
                let hold = if c.awaiting_history {
                    CoinHold::AwaitingHistory
                } else if c.reserved && !c.lock_input {
                    CoinHold::Reserved
                } else if c.unresolved_lock {
                    CoinHold::UnresolvedLock
                } else if c.dust_locked {
                    CoinHold::DustLock
                } else if c.user_locked {
                    CoinHold::UserLock
                } else {
                    return None;
                };
                Some(HeldCoin {
                    outpoint: c.utxo.outpoint,
                    value: c.utxo.value(),
                    hold,
                })
            })
    }
}

type ManagerArc = Arc<
    tokio::sync::RwLock<
        platform_wallet::key_wallet_manager::WalletManager<platform_wallet::PlatformWalletInfo>,
    >,
>;

struct TempUtxoLocks {
    wm: ManagerArc,
    hub: Arc<crate::events::SessionHub>,
    wallet_id: [u8; 32],
    outpoints: Vec<OutPoint>,
    rt: tokio::runtime::Handle,
}

/// Takes `outpoints` out of wallet `wallet_id`'s lock set, held or spent
/// since; returns the wallet, its balances recomputed, when one was in it.
fn unlock_temp_utxos<'a>(
    wm: &'a mut platform_wallet::key_wallet_manager::WalletManager<
        platform_wallet::PlatformWalletInfo,
    >,
    wallet_id: &[u8; 32],
    outpoints: &[OutPoint],
) -> Option<&'a ManagedWalletInfo> {
    let info = wm.get_wallet_info_mut(wallet_id)?;
    let mut changed = false;
    for op in outpoints {
        changed |= info.core_wallet.unlock_outpoint(op);
    }
    changed.then(|| {
        info.core_wallet.update_balance();
        &info.core_wallet
    })
}

impl TempUtxoLocks {
    /// Unlocks the coins and, under the same manager guard, hands the
    /// recomputed balances to the hub, which would otherwise keep the last
    /// event's until the next one.
    fn release(
        &self,
        wm: &mut platform_wallet::key_wallet_manager::WalletManager<
            platform_wallet::PlatformWalletInfo,
        >,
    ) {
        if let Some(core) = unlock_temp_utxos(wm, &self.wallet_id, &self.outpoints) {
            (self.hub).apply_balance_read(WalletId(self.wallet_id), core);
        }
    }
}

/// Proof that the caller holds a wallet's coin fence (module doc): no
/// coin lock, dust lock, reservation or other build of the wallet changes
/// what may be spent until it is dropped. Taken with
/// [`NetworkSession::coin_fence`].
pub struct CoinFence {
    wallet: WalletId,
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    /// The coins the last read under this fence found free
    /// ([`CoinSnapshot::free_outpoints`]): a library funding that keeps the
    /// fence signs no other ([`crate::platform::signers::CountingFundingSigner`]).
    checked: std::sync::Mutex<Option<Arc<HashSet<OutPoint>>>>,
    temp_locks: std::sync::Mutex<Option<TempUtxoLocks>>,
}

impl CoinFence {
    pub fn wallet_id(&self) -> WalletId {
        self.wallet
    }

    /// The coins the last read under this fence found free, if it read any.
    pub(crate) fn checked_coins(&self) -> Option<Arc<HashSet<OutPoint>>> {
        self.checked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn record_checked(&self, free: HashSet<OutPoint>) {
        *self.checked.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(free));
    }

    fn add_temp_locks(&self, locks: TempUtxoLocks) {
        let mut slot = self.temp_locks.lock().unwrap_or_else(|p| p.into_inner());
        match slot.as_mut() {
            Some(held) => held.outpoints.extend(locks.outpoints),
            None => *slot = Some(locks),
        }
    }
}

impl Drop for CoinFence {
    fn drop(&mut self) {
        let locks = self
            .temp_locks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
        let Some(locks) = locks else { return };
        if let Ok(mut wm) = locks.wm.try_write() {
            locks.release(&mut wm);
        } else {
            let guard = self.guard.take();
            let rt = locks.rt.clone();
            rt.spawn(async move {
                let mut wm = locks.wm.write().await;
                locks.release(&mut wm);
                drop(wm);
                drop(guard);
            });
        }
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
    /// Tests: holds the next PSBT broadcast after its peer wait, before it
    /// takes the coin fences.
    #[cfg(test)]
    pub(crate) pause_broadcast: std::sync::Mutex<Option<Arc<TestPause>>>,
    /// Tests: holds the next PSBT broadcast between a wallet's lookup and
    /// its ownership read.
    #[cfg(test)]
    pub(crate) pause_ownership: std::sync::Mutex<Option<Arc<TestPause>>>,
    /// Tests: holds the next masternode row write after its reservation,
    /// its fence held: a shared session's (`start_shared`, `join_shared`,
    /// MG-04b D63) or a ProTx operation's (`save_operation`, BL-352).
    #[cfg(test)]
    pub(crate) pause_row_write: std::sync::Mutex<Option<Arc<TestPause>>>,
}

/// A one-shot pause point for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct TestPause {
    pub(crate) reached: std::sync::atomic::AtomicBool,
    pub(crate) release: tokio::sync::Notify,
    /// Set by [`Self::hold_then`] once the held work has run.
    pub(crate) done: std::sync::atomic::AtomicBool,
    gate: std::sync::Mutex<()>,
    signal: std::sync::Condvar,
}

#[cfg(test)]
impl TestPause {
    /// Marks the pause reached and wakes [`Self::wait_reached`].
    pub(crate) fn mark_reached(&self) {
        let _gate = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.reached
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.signal.notify_all();
    }

    /// Blocks until the pause is reached, or `bound` passed: false then.
    /// Woken by [`Self::mark_reached`]; a pause marked by a plain store is
    /// seen within 5 ms.
    pub(crate) fn wait_reached(&self, bound: std::time::Duration) -> bool {
        let guard = std::time::Instant::now() + bound;
        let mut gate = self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !self.reached.load(std::sync::atomic::Ordering::SeqCst) {
            if std::time::Instant::now() >= guard {
                return false;
            }
            let slice = std::time::Duration::from_millis(5);
            gate = (self.signal.wait_timeout(gate, slice))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }

    /// Marks the pause reached and blocks until it is released. On a
    /// blocking-pool thread of the runtime.
    pub(crate) fn hold(&self) {
        self.mark_reached();
        tokio::runtime::Handle::current().block_on(self.release.notified());
    }

    /// [`Self::hold`], then runs `f` and marks the held work done.
    pub(crate) fn hold_then<R>(&self, f: impl FnOnce() -> R) -> R {
        self.hold();
        let done = f();
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        done
    }
}

#[cfg(test)]
tokio::task_local! {
    /// Tests: holds the first [`NetworkSession::appdb_op`] job of the task
    /// in scope before it runs.
    pub(crate) static HOLD_APPDB_JOB: Arc<TestPause>;
    /// Tests: the gate's holds for the operations of the task in scope.
    pub(crate) static GATE_HOLDS: Arc<GateHolds>;
}

/// Tests: where [`crate::gate::OpGate::enter`] holds an operation of the
/// task in the scope of [`GATE_HOLDS`], and what it saw there.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct GateHolds {
    /// Before it is admitted: its read lock taken, the gate not closed.
    pub(crate) entering: std::sync::Mutex<Option<Arc<TestPause>>>,
    /// Admitted, between its last two loads.
    pub(crate) between_loads: std::sync::Mutex<Option<Arc<TestPause>>>,
    /// How often it was admitted, saw the pause and backed off.
    pub(crate) backed_off: std::sync::atomic::AtomicUsize,
}

/// Tests: the pause of the first app.sqlite job of a task in the scope of
/// [`HOLD_APPDB_JOB`], until it is reached.
#[cfg(test)]
pub(crate) fn held_appdb_job() -> Option<Arc<TestPause>> {
    HOLD_APPDB_JOB
        .try_with(Arc::clone)
        .ok()
        .filter(|p| !p.reached.load(std::sync::atomic::Ordering::SeqCst))
}

/// [`NetworkSession::appdb_op`] on `db`, for a future that must own it.
pub(crate) async fn appdb_job<T, F>(db: Arc<AppDb>, f: F) -> Result<T, EngineError>
where
    F: FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
    T: Send + 'static,
{
    #[cfg(test)]
    let pause = held_appdb_job();
    tokio::task::spawn_blocking(move || {
        #[cfg(test)]
        if let Some(pause) = pause {
            return pause.hold_then(|| f(&db));
        }
        f(&db)
    })
    .await?
    .map_err(EngineError::from)
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
            guard: Some(fence.lock_owned().await),
            checked: std::sync::Mutex::new(None),
            temp_locks: std::sync::Mutex::new(None),
        }
    }

    /// Tests: how many wait for `wallet`'s fence while one holds it.
    #[cfg(test)]
    pub(crate) fn waiting(&self, wallet: WalletId) -> usize {
        (self.map().get(&wallet)).map_or(0, |f| Arc::strong_count(f).saturating_sub(2))
    }

    /// `wallet`'s fence if no one holds it now.
    pub(crate) fn try_acquire(&self, wallet: WalletId) -> Option<CoinFence> {
        let fence = Arc::clone(self.map().entry(wallet).or_default());
        Some(CoinFence {
            wallet,
            guard: Some(fence.try_lock_owned().ok()?),
            checked: std::sync::Mutex::new(None),
            temp_locks: std::sync::Mutex::new(None),
        })
    }

    /// Tests: how many hold or wait for `wallet`'s fence.
    #[cfg(test)]
    pub(crate) fn users(&self, wallet: WalletId) -> usize {
        self.map()
            .get(&wallet)
            .map_or(0, |f| Arc::strong_count(f) - 1)
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
        pause_at(&self.pause_prepare).await;
    }

    /// Tests: the pause point of a PSBT broadcast past its peer wait.
    pub(crate) async fn broadcast_pause_point(&self) {
        #[cfg(test)]
        pause_at(&self.pause_broadcast).await;
    }
}

/// Tests: runs `first` up to its row write (held at
/// [`CoinFences::pause_row_write`]), then `second` for two seconds, then
/// lets both finish: `second` waits for the fence `first` holds from its
/// keys to its write, or, without it, reads the keys and coins in use
/// before `first`'s row is written.
#[cfg(test)]
pub(crate) fn write_paused<A, B>(
    engine: &crate::Engine,
    session: &NetworkSession,
    first: impl std::future::Future<Output = A> + Send + 'static,
    second: impl std::future::Future<Output = B> + Send + 'static,
) -> (A, B)
where
    A: Send + 'static,
    B: Send + 'static,
{
    use std::sync::atomic::Ordering::SeqCst;
    let pause = Arc::new(TestPause::default());
    *session.coin_fences.pause_row_write.lock().unwrap() = Some(Arc::clone(&pause));
    let first = session.rt.spawn(first);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pause.reached.load(SeqCst) {
        assert!(
            std::time::Instant::now() < deadline,
            "the first call reaches its write"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let second = session.rt.spawn(second);
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert!(!second.is_finished(), "the second call waits for the first");
    pause.release.notify_one();
    (
        engine.block_on(first).unwrap(),
        engine.block_on(second).unwrap(),
    )
}

#[cfg(test)]
pub(crate) async fn pause_at(slot: &std::sync::Mutex<Option<Arc<TestPause>>>) {
    let pause = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
    if let Some(pause) = pause {
        pause.mark_reached();
        pause.release.notified().await;
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
    /// Kept out of spending and every balance, and why
    /// ([`crate::withhold`]).
    pub excluded: Option<crate::ExclusionReason>,
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
    network: key_wallet::Network,
) {
    for utxo in account.utxos.values() {
        let txid = utxo.outpoint.txid;
        let record = account.transactions().get(&txid);
        let path = account
            .get_address_info(&utxo.address)
            .map(|info| info.path);
        let is_change = !coinjoin_account && path.as_ref().is_some_and(is_internal_path);
        let counted_denomination = dw_coinjoin::denoms::is_denominated(utxo.value())
            && (coinjoin_account
                || path
                    .as_ref()
                    .is_some_and(|p| dw_vault::is_bip44_address(p, network)));
        coins.push(WalletCoin {
            utxo: utxo.clone(),
            send_account,
            coinjoin_account,
            counted_denomination,
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
            dust_locked: false,
            reserved: false,
            lock_input: false,
            unresolved_lock: false,
            awaiting_history: false,
            excluded: None,
        });
    }
}

/// The inputs of the tracked asset locks that may be sent (`Built`,
/// `Broadcast`): reserved for them (module doc).
pub(crate) fn pending_lock_inputs<'a>(
    locks: impl IntoIterator<Item = &'a platform_wallet::TrackedAssetLock>,
) -> HashSet<OutPoint> {
    locks
        .into_iter()
        .filter(|l| crate::platform::lease::TrackedStatus::from(&l.status).pending())
        .flat_map(|l| l.transaction.input.iter().map(|i| i.previous_output))
        .collect()
}

/// The funding accounts: BIP44, BIP32 and DashPay receiving, which
/// platform-wallet's `ASSET_LOCK_FUNDING_SOURCES` pools (CoinJoin accounts
/// are no funding source).
fn funding_accounts(
    accounts: &ManagedAccountCollection,
) -> impl Iterator<Item = &ManagedCoreFundsAccount> {
    accounts
        .standard_bip44_accounts
        .values()
        .chain(accounts.standard_bip32_accounts.values())
        .chain(accounts.dashpay_receival_accounts.values())
}

/// Reads every funds account of `info` (key-wallet state only).
pub(crate) fn read_coins(info: &ManagedWalletInfo) -> CoinSnapshot {
    let accounts = &info.accounts;
    let mut coins = Vec::new();
    let network = info.network();
    for a in funding_accounts(accounts) {
        collect(&mut coins, a, true, false, network);
    }
    for a in accounts.coinjoin_accounts.values() {
        collect(&mut coins, a, false, true, network);
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
    funding_accounts(accounts).any(locked) || accounts.coinjoin_accounts.values().any(locked)
}

/// The transactions of the funding accounts (BIP44, BIP32, DashPay
/// receiving) that spend one of the wallet's coins and are not ChainLocked
/// yet. Repair's self-spend waits for none (`platform/lease/repair.rs`):
/// a coin such a transaction spends is no coin of the self-spend, so a lost
/// asset lock that conflicts only with it is not beaten by the self-spend.
/// Each is mapped to whether it is only in the mempool: neither mined nor
/// InstantSend-locked, so an abandon may take it.
pub(crate) fn unsettled_spends(info: &ManagedWalletInfo) -> BTreeMap<Txid, bool> {
    use key_wallet::transaction_checking::TransactionContext;
    let mut out = BTreeMap::new();
    for r in (funding_accounts(&info.accounts))
        .flat_map(|a| a.transactions().values())
        .filter(|r| !r.input_details.is_empty() && !r.context.is_chain_locked())
    {
        *out.entry(r.txid).or_insert(true) &= matches!(r.context, TransactionContext::Mempool);
    }
    out
}

/// The coins of exactly a masternode collateral amount whose funding
/// transaction `wallet`'s history neither holds nor held a coin of at its
/// last clean load ([`NetworkSession::coin_snapshot`]).
fn unseen_collateral_sized(
    history: &crate::history::HistoryStore,
    wallet: WalletId,
    coins: &[WalletCoin],
) -> HashSet<OutPoint> {
    use dw_protx::params::collateral_amount;
    history.read_with_loaded(&wallet, |txs, loaded| {
        (coins.iter())
            .filter(|c| {
                [collateral_amount(false), collateral_amount(true)].contains(&c.utxo.value())
            })
            .filter(|c| !txs.contains_key(&c.utxo.outpoint.txid))
            .filter(|c| !loaded.contains(&c.utxo.outpoint.txid))
            .map(|c| c.utxo.outpoint)
            .collect()
    })
}

/// How many unspent outputs the asset-lock builder may fund from: every coin
/// of the accounts platform-wallet's `ASSET_LOCK_FUNDING_SOURCES` pools
/// (BIP44, BIP32, DashPay receiving), whatever its maturity, locks or
/// reservations, which the library does not all see (E0-04 §2a.3's
/// `fee_bound_worst` counts a superset, so its bound stays an upper one).
pub(crate) async fn funding_coin_count(wallet: &PlatformWallet) -> Result<usize, EngineError> {
    let state = crate::wallet_read::info_of(wallet).await?;
    Ok(read_coins(&state.core_wallet)
        .coins
        .iter()
        .filter(|c| c.send_account)
        .count())
}

impl NetworkSession {
    /// Runs a dw-appdb call on the blocking pool.
    pub(crate) async fn appdb_op<T, F>(&self, f: F) -> Result<T, EngineError>
    where
        F: FnOnce(&AppDb) -> Result<T, AppDbError> + Send + 'static,
        T: Send + 'static,
    {
        appdb_job(self.live()?.appdb, f).await
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
    pub(crate) async fn fenced_coins(
        &self,
        fence: &CoinFence,
    ) -> Result<CoinSnapshot, EngineError> {
        let wallet = self.wallet(&fence.wallet).await?;
        self.coin_snapshot(fence, &wallet).await
    }

    /// Reads wallet `id`'s balances from the manager into the hub, under
    /// the manager's lock.
    pub(crate) async fn reread_balances(&self, id: WalletId) {
        let Ok(manager) = self.manager() else { return };
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let Some(info) = wm.get_wallet_info(&id.0) else {
            return;
        };
        let core = &info.core_wallet;
        (self.hub).apply_balance_read(id, core);
        let released = self.hub.settle_rescan_holds(id, Some(core.synced_height()));
        self.forget_released_holds(id, released).await;
        drop(wm);
    }

    /// Hands the hub what reads a wallet's balances again
    /// ([`Self::reread_balances`]), on the runtime: the event that asks
    /// for it holds the hub's unload guard.
    pub(crate) fn attach_balance_reread(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let rt = self.rt.clone();
        self.hub.attach_balance_reread(Box::new(move |id| {
            let weak = weak.clone();
            #[cfg(test)]
            let running = weak.upgrade().map(|s| s.hub.reread_started());
            rt.spawn(async move {
                if let Some(session) = weak.upgrade() {
                    session.reread_balances(id).await;
                }
                #[cfg(test)]
                drop(running);
            });
        }));
    }

    /// Keeps the funding coins the library's builder must not select though
    /// no hold refuses the funding (denominations Dash Core counts, excluded
    /// coins ([`crate::withhold`]) and coins waiting for their rescan) out of
    /// its selection and spendable balance until `fence` drops. They go into
    /// key-wallet's lock set, which every balance refresh and funding reads
    /// the coins' lock flags from, through
    /// `ManagedWalletInfo::lock_outpoint`, which stores nothing: the
    /// persister stores only `PlatformWallet::lock_outpoint`/`unlock_outpoint`
    /// and the collateral locks the library itself just added, so a crash
    /// leaves no hold behind. Only the outpoints this call added are
    /// released, so a lock already in the set stays.
    pub(crate) async fn exclude_denominations_under_fence(
        &self,
        fence: &CoinFence,
        snapshot: &CoinSnapshot,
    ) -> Result<(), EngineError> {
        // A coin waiting for its rescan is not in the snapshot at all.
        let denoms: Vec<OutPoint> = snapshot
            .coins
            .iter()
            .filter(|c| c.send_account && (c.counted_denomination || c.excluded.is_some()))
            .map(|c| c.utxo.outpoint)
            .chain(self.hub.rescan_held(&fence.wallet))
            .collect();
        let wm = self.manager()?.wallet_manager_arc();
        let locked: Vec<OutPoint> = {
            let mut guard = wm.write().await;
            let info = guard
                .get_wallet_info_mut(&fence.wallet.0)
                .ok_or_else(|| EngineError::WalletNotFound(fence.wallet.to_string()))?;
            denoms
                .into_iter()
                .filter(|op| info.core_wallet.lock_outpoint(*op))
                .collect()
        };
        if !locked.is_empty() {
            fence.add_temp_locks(TempUtxoLocks {
                wm,
                hub: Arc::clone(&self.hub),
                wallet_id: fence.wallet.0,
                outpoints: locked,
                rt: self.rt.clone(),
            });
        }
        Ok(())
    }

    /// The fenced wallet's coins with user/dust locks and reservations
    /// applied. First locks the collaterals a ProRegTx in the history names
    /// ([`Self::lock_collaterals`]), so no build selects one before the
    /// masternode follower has run. With dust protection on, then
    /// dust-locks small foreign incoming coins (Dash Core: value ≤
    /// threshold, funding transaction not ours; a released dust lock is not
    /// renewed). Both only under the fence (module doc).
    ///
    /// A coin of exactly a masternode collateral amount credited since the
    /// history loaded, whose transaction the history doesn't hold yet, is
    /// reserved (and every asset-lock funding of the wallet refused with
    /// `CoinsLocked`, [`CoinHold::AwaitingHistory`]): the wallet's event
    /// reaches the history one task hop after the coin reaches the wallet,
    /// and until then nothing says whether that transaction is a ProRegTx
    /// whose own output this coin is. A coin the wallet held at a clean
    /// history load whose transaction was stored is not held: a collateral
    /// among those that the history has no ProRegTx for (no record) is
    /// locked once the masternode list syncs. A contact's record comes only
    /// from a store written before platform-wallet stopped persisting
    /// those; a contact's ProRegTx is never left to the history hold alone:
    /// its collateral is locked here from the record the history keeps out
    /// of sight. After a load that failed to read a record, or read one
    /// missing or another's, every such coin missing from the history
    /// stays held for the session, and when the history misses an event (a
    /// lagged event bus), that coin does.
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
        let (mut snapshot, lock_inputs, unseen, applied, excluded) = {
            let state = crate::wallet_read::info_of(wallet).await?;
            let snapshot = read_coins(&state.core_wallet);
            let unseen = unseen_collateral_sized(&self.hub.history, wallet_id, &snapshot.coins);
            (
                snapshot,
                pending_lock_inputs(state.tracked_asset_locks.values()),
                unseen,
                crate::history_ops::applied_chainlock(&state),
                self.hub.excluded_coins(wallet_id, &state.core_wallet),
            )
        };
        // Coins before the history: a ProRegTx that lands between the two
        // reads is still seen, where the other order could read its coin
        // without it.
        let collaterals = self.hub.history.registration_collaterals(&wallet_id);
        self.lock_collaterals(fence, collaterals, &snapshot.coins)
            .await?;
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
        let sweep_scope = dw_appdb::local_scope(&id);
        let (locked, sweep_outs) = self
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
                let locked: HashMap<(String, u32), LockReason> = db
                    .locks(&id)?
                    .into_iter()
                    .filter(|row| row.is_locked())
                    .map(|row| ((row.txid, row.vout), row.reason))
                    .collect();
                let sweep_outs = crate::coinjoin::SweepRows::read(db, &sweep_scope)?;
                Ok((locked, sweep_outs))
            })
            .await?;
        // A sweep chunk that may be on the network holds its inputs from
        // every spender, across restarts, while it is pending (`coinjoin.rs`
        // `sweep_out_state`); a sweep or a resend drops the final records.
        // Once resolved it holds nothing: the session's own claim of its
        // inputs goes too, so an input its rival did not take is free.
        let (states, _) = self.sweep_view(wallet_id, sweep_outs).await;
        let mut pending = Vec::new();
        for (o, state) in states {
            if state == crate::coinjoin::SweepOutState::Pending {
                pending.push(o);
            } else {
                self.spends
                    .release(&wallet_id, crate::send::SpendOwner::Tx(o.txid()));
            }
        }
        let swept = crate::coinjoin::held_inputs(&pending);
        let unspent: HashSet<OutPoint> = snapshot.coins.iter().map(|c| c.utxo.outpoint).collect();
        self.spends.retain_unspent(&wallet_id, &unspent);
        let reserved = self.spends.snapshot(&wallet_id);
        let unresolved = self.leases.unresolved_lock_inputs(wallet_id);
        // A replay re-credits a coin whose spender is still unconfirmed at
        // close until a block spends it (0.1.x-bal §1): held as Core holds it.
        let chainlock = (applied.max(self.hub.tracker().chainlock_height()))
            .max(self.hub.best_chainlock().map(|(h, _)| h));
        let spent = (self.hub.history).read(&wallet_id, |txs| {
            crate::history::spent_outpoints(txs, chainlock)
        });
        for coin in &mut snapshot.coins {
            let o = coin.utxo.outpoint;
            let lock = locked.get(&lock_key(&o));
            coin.user_locked = lock.is_some();
            coin.dust_locked = lock == Some(&LockReason::Dust);
            let lock_reserved = lock_inputs.contains(&o);
            coin.unresolved_lock = !lock_reserved && unresolved.contains(&o);
            // A lock's own input stays held as the lock's.
            let lock_held = lock_reserved || coin.unresolved_lock;
            let spend_reserved = reserved.contains(&o)
                || swept.contains(&o)
                || (spent.contains(&o) && !lock_held)
                || unseen.contains(&o);
            coin.lock_input = !spend_reserved && lock_held;
            coin.reserved = spend_reserved || lock_held;
            coin.awaiting_history = unseen.contains(&o);
            coin.excluded = excluded.get(&o).copied();
            if coin.excluded.is_some() {
                coin.reserved = true;
                coin.lock_input = false;
            }
        }
        // An excluded coin is listed with its reason, or not at all.
        (snapshot.coins).retain(|c| c.excluded.is_none_or(|r| r.listed()));
        fence.record_checked(snapshot.free_outpoints());
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
                        coinjoin_denominated: m.denominated(),
                        coinjoin_rounds: (m.denominated() && m.rounds >= 0)
                            .then_some(m.rounds as u32),
                        spendable: has_keys && c.chosen_selectable(height),
                        excluded: c.excluded,
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
            let state = crate::wallet_read::info_of(&wallet).await?;
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
    /// dust protection then leaves alone). A masternode collateral is
    /// locked again by the next coin read or follower pass, where Core keeps
    /// the unlock until a restart: only `release-collateral` frees one. An
    /// outpoint that is neither locked nor an unspent output is
    /// `OutpointNotFound`; one a masternode
    /// row holds (a ProTx operation's inputs, a shared session's
    /// reservation, MG-04b BL-346) is refused and nothing is unlocked; an
    /// operation abandoned in Activity holds nothing (MG-10 §1.10, D49). Under
    /// the coin fence, as "Lock unspent": rows take coins only under it.
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
            // An operation whose transaction was abandoned holds nothing
            // (MG-10 §1.10, D49): settled first, also after a restart. A
            // held coin then refuses with the settling's failure, if any.
            let settled = (this.settle_operation_locks(wallet_id).await).map_err(|e| {
                EngineError::Storage(format!("releasing a masternode transaction's coins: {e}"))
            });
            this.appdb_op(move |db| {
                let held = db.held_outpoints(&id, dw_appdb::Holder::None)?;
                if keys.iter().any(|k| held.contains(k)) {
                    return Ok(Err(settled.err().unwrap_or_else(|| {
                        EngineError::InvalidArgument(
                            "the coin is reserved by a masternode transaction".into(),
                        )
                    })));
                }
                let now = now_secs();
                for (txid, vout) in &keys {
                    db.unlock(&id, txid, *vout, now)?;
                }
                Ok(Ok(()))
            })
            .await?
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

    const DENOMINATION: u64 = dw_coinjoin::denoms::DENOMINATIONS[2];

    /// Recomputes the wallet's balance, as a block or transaction does, and
    /// hands it to the hub, as that event does.
    async fn wallet_event(s: &NetworkSession, w: WalletId) {
        let manager = s.manager().unwrap();
        {
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            wm.get_wallet_info_mut(&w.0)
                .unwrap()
                .core_wallet
                .update_balance();
        }
        s.refresh_wallet_state(&manager, w).await;
    }

    fn confirmed(s: &NetworkSession, w: WalletId) -> u64 {
        s.hub.wallet_state(&w).unwrap().balance.confirmed()
    }

    async fn locked(s: &NetworkSession, w: WalletId, outpoint: OutPoint) -> Option<bool> {
        let wallet = s.wallet(&w).await.unwrap();
        let state = wallet.state().await;
        (state.core_wallet.accounts.standard_bip44_accounts[&0].utxos)
            .get(&outpoint)
            .map(|u| u.is_locked)
    }

    /// Takes the funding fence and locks the BIP44 denominations, as a
    /// Shielded, top-up or registration funding does.
    async fn funding_fence(s: &Arc<NetworkSession>, w: WalletId) -> CoinFence {
        let fence = s.coin_fence(w).await.unwrap();
        let snapshot = s.fenced_coins(&fence).await.unwrap();
        (s.exclude_denominations_under_fence(&fence, &snapshot).await).unwrap();
        fence
    }

    /// A wallet event inside a funding's fence hands the app a balance
    /// without the locked denominations; once the fence drops, the app has
    /// them back with no further event.
    #[test]
    fn the_balance_counts_the_denominations_again_when_the_funding_fence_drops() {
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let (before, during, after) = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let before = confirmed(&inner, w);
                let fence = funding_fence(&inner, w).await;
                wallet_event(&inner, w).await;
                let during = confirmed(&inner, w);
                drop(fence);
                Ok((before, during, confirmed(&inner, w)))
            }))
            .unwrap();
        assert_eq!(during, before - DENOMINATION);
        assert_eq!(after, before);
    }

    /// A wallet event the library emitted while a funding held its locks,
    /// delivered from its queue only after the fence dropped, carries the
    /// balance without the denominations; the app ends with them counted.
    #[test]
    fn a_wallet_event_queued_across_the_fence_release_leaves_the_balance_whole() {
        use platform_wallet::key_wallet_manager::WalletInterface;
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let before = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let before = confirmed(&inner, w);
                let manager = inner.manager()?;
                let wm = manager.wallet_manager_arc();
                let mut queue = wm.read().await.subscribe_events();
                let fence = funding_fence(&inner, w).await;
                let wallet = inner.wallet(&w).await?;
                let to = wallet.core().next_receive_address_for_account(0).await?;
                let tx = dashcore::Transaction {
                    version: 2,
                    lock_time: 0,
                    input: vec![dashcore::TxIn {
                        previous_output: OutPoint::new(
                            dashcore::hashes::Hash::from_byte_array([0xee; 32]),
                            0,
                        ),
                        ..Default::default()
                    }],
                    output: vec![dashcore::TxOut {
                        value: 10_000_000,
                        script_pubkey: to.script_pubkey(),
                    }],
                    special_transaction_payload: None,
                };
                wm.write()
                    .await
                    .process_mempool_transaction(&tx, None)
                    .await;
                let queued = queue.try_recv().expect("the library emitted an event");
                drop(fence);
                inner.hub.apply_wallet_event(&queued);
                Ok(before)
            }))
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while confirmed(&s, w) != before {
            assert!(
                std::time::Instant::now() < deadline,
                "confirmed {} after the queued event, {before} before",
                confirmed(&s, w)
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// A fence released after its wallet was unloaded hands the hub nothing:
    /// no state comes back for the closed wallet.
    #[test]
    fn a_fence_released_after_an_unload_leaves_the_closed_wallet_alone() {
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        f.engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let fence = funding_fence(&inner, w).await;
                inner.hub.unload_wallet(&w);
                drop(fence);
                Ok(())
            }))
            .unwrap();
        assert!(s.hub.wallet_state(&w).is_none());
    }

    /// A denomination another wallet on the seed spent while a funding held
    /// it locked, given back after the fence dropped (a reorg, a dropped
    /// transaction), comes back unlocked: the release takes it out of the
    /// wallet's lock set though the coin was gone, and the next funding
    /// locks and releases it again.
    #[test]
    fn a_denomination_spent_and_given_back_around_a_fence_ends_unlocked() {
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let denom = f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let (kept_in_lock_set, returned_locked, relocked, after, balance) = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let before = confirmed(&inner, w);
                let fence = funding_fence(&inner, w).await;
                let spent = {
                    let wallet = inner.wallet(&w).await.unwrap();
                    let mut state = wallet.state_mut().await;
                    let account = (state.core_wallet.accounts.standard_bip44_accounts)
                        .get_mut(&0)
                        .unwrap();
                    account.utxos.remove(&denom).unwrap()
                };
                drop(fence);
                let kept_in_lock_set = {
                    let wallet = inner.wallet(&w).await.unwrap();
                    let state = wallet.state().await;
                    state.core_wallet.is_outpoint_locked(&denom)
                };
                {
                    let wallet = inner.wallet(&w).await.unwrap();
                    let mut state = wallet.state_mut().await;
                    let account = (state.core_wallet.accounts.standard_bip44_accounts)
                        .get_mut(&0)
                        .unwrap();
                    account.utxos.insert(denom, spent);
                }
                wallet_event(&inner, w).await;
                let returned_locked = locked(&inner, w, denom).await;
                let second = funding_fence(&inner, w).await;
                let relocked = locked(&inner, w, denom).await;
                drop(second);
                let after = locked(&inner, w, denom).await;
                Ok((
                    kept_in_lock_set,
                    returned_locked,
                    relocked,
                    after,
                    (before, confirmed(&inner, w)),
                ))
            }))
            .unwrap();
        assert!(!kept_in_lock_set);
        assert_eq!(returned_locked, Some(false));
        assert_eq!(relocked, Some(true));
        assert_eq!(after, Some(false));
        assert_eq!(balance.1, balance.0);
    }

    /// A denomination the wallet's lock set already held before a funding
    /// (a lock the library or the user made) stays locked when the
    /// funding's fence drops: the fence releases only its own locks.
    #[test]
    fn a_funding_fence_leaves_a_lock_it_did_not_make() {
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let kept = f.credit(2, DENOMINATION);
        let other = f.credit(3, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let (other_held, kept_locked, other_locked) = f
            .engine
            .block_on(s.on_runtime(async move {
                {
                    let wallet = inner.wallet(&w).await.unwrap();
                    let mut state = wallet.state_mut().await;
                    assert!(state.core_wallet.lock_outpoint(kept));
                }
                let fence = funding_fence(&inner, w).await;
                let other_held = locked(&inner, w, other).await;
                drop(fence);
                wallet_event(&inner, w).await;
                let wallet = inner.wallet(&w).await.unwrap();
                let state = wallet.state().await;
                Ok((
                    other_held,
                    state.core_wallet.is_outpoint_locked(&kept),
                    state.core_wallet.is_outpoint_locked(&other),
                ))
            }))
            .unwrap();
        assert_eq!(other_held, Some(true));
        assert!(kept_locked);
        assert!(!other_locked);
    }

    /// A denomination taken out of the wallet and given back while a
    /// funding's fence still holds it arrives locked, through the library's
    /// next balance refresh, and is free once the fence drops.
    #[test]
    fn a_denomination_given_back_under_the_fence_arrives_locked() {
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let denom = f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let (before, during, under, after, balance) = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let before = confirmed(&inner, w);
                let fence = funding_fence(&inner, w).await;
                {
                    let wallet = inner.wallet(&w).await.unwrap();
                    let mut state = wallet.state_mut().await;
                    let account = (state.core_wallet.accounts.standard_bip44_accounts)
                        .get_mut(&0)
                        .unwrap();
                    let mut back = account.utxos.remove(&denom).unwrap();
                    back.is_locked = false;
                    account.utxos.insert(denom, back);
                }
                wallet_event(&inner, w).await;
                let under = locked(&inner, w, denom).await;
                let during = confirmed(&inner, w);
                drop(fence);
                wallet_event(&inner, w).await;
                let after = locked(&inner, w, denom).await;
                Ok((before, during, under, after, confirmed(&inner, w)))
            }))
            .unwrap();
        assert_eq!(under, Some(true));
        assert_eq!(during, before - DENOMINATION);
        assert_eq!(after, Some(false));
        assert_eq!(balance, before);
    }

    /// A build from BIP44 for `duffs` to the wallet's own next address;
    /// `None` when selection could not fund it.
    async fn build_from_bip44(
        s: &NetworkSession,
        w: WalletId,
        duffs: u64,
        strategy: key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy,
    ) -> Option<HashSet<OutPoint>> {
        use key_wallet::wallet::managed_wallet_info::transaction_building::AccountTypePreference;
        let to = (s.wallet(&w).await.unwrap().core())
            .next_receive_address_for_account(0)
            .await
            .unwrap();
        let wm = s.manager().unwrap().wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm.get_wallet_and_info_mut(&w.0).unwrap();
        let wallet = wallet.clone();
        let built = info.core_wallet.build_unsigned_transaction(
            &wallet,
            &[AccountTypePreference::BIP44],
            0,
            vec![(to.as_unchecked().clone(), duffs)],
            key_wallet::wallet::managed_wallet_info::fee::FeeRate::new(crate::send::MIN_FEE_PER_KB),
            strategy,
        );
        built
            .ok()
            .map(|(tx, _)| tx.input.iter().map(|i| i.previous_output).collect())
    }

    /// Wallet `w`'s lock set as the library holds it (after a load: what
    /// its store handed back).
    async fn lock_set(s: &NetworkSession, w: WalletId) -> Vec<OutPoint> {
        s.wallet(&w)
            .await
            .unwrap()
            .locked_outpoints()
            .await
            .unwrap()
    }

    /// How many locks wallet.sqlite holds for wallet `w`.
    fn stored_locks(s: &NetworkSession, w: WalletId) -> i64 {
        let db = rusqlite::Connection::open_with_flags(
            s.data_dir().join(crate::session::WALLET_DB_FILE),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        db.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
        db.query_row(
            "SELECT COUNT(*) FROM core_locked_outpoints WHERE wallet_id = ?1",
            [&w.0[..]],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// A denomination under a funding's fence stays locked through the
    /// library's own balance refresh and funding, both of which set every
    /// coin's lock flag from the lock set again: it is neither counted
    /// spendable nor selected, whatever the strategy, until the fence drops.
    #[test]
    fn a_fenced_denomination_stays_out_of_a_balance_refresh_and_a_funding() {
        use key_wallet::wallet::managed_wallet_info::coin_selection::SelectionStrategy;
        let f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let denom = f.credit(2, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let needs_both = 100_000_000 + DENOMINATION / 2;
        let (before, during, flag, large, small, freed) = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let before = confirmed(&inner, w);
                let fence = funding_fence(&inner, w).await;
                wallet_event(&inner, w).await;
                let during = confirmed(&inner, w);
                let flag = locked(&inner, w, denom).await;
                let large =
                    build_from_bip44(&inner, w, needs_both, SelectionStrategy::LargestFirst).await;
                let small =
                    build_from_bip44(&inner, w, 10_000, SelectionStrategy::SmallestFirst).await;
                drop(fence);
                let freed =
                    build_from_bip44(&inner, w, 10_000, SelectionStrategy::SmallestFirst).await;
                Ok((before, during, flag, large, small, freed))
            }))
            .unwrap();
        assert_eq!(during, before - DENOMINATION, "counted spendable");
        assert_eq!(flag, Some(true), "the refresh unlocked it");
        assert_eq!(large, None, "a build needing it was funded under the fence");
        let small = small.expect("the other coin funds a small build");
        assert!(
            !small.contains(&denom),
            "selected under the fence: {small:?}"
        );
        assert_eq!(
            freed,
            Some(HashSet::from([denom])),
            "not free once the fence dropped"
        );
    }

    /// A process that dies while a funding holds its fence leaves no hold:
    /// the fence's locks never reach wallet.sqlite, so after the restart the
    /// coin is free and the stored lock set holds only what the library
    /// itself locked and stored.
    #[test]
    fn a_fence_held_at_a_crash_leaves_no_lock_after_the_restart() {
        let mut f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let denom = f.credit(2, DENOMINATION);
        let stored = OutPoint::new(Txid::from_byte_array([0x7c; 32]), 0);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let held = f
            .engine
            .block_on(s.on_runtime(async move {
                let wallet = inner.wallet(&w).await.unwrap();
                assert!(wallet.lock_outpoint(stored).await.unwrap());
                let fence = funding_fence(&inner, w).await;
                let held = lock_set(&inner, w).await;
                std::mem::forget(fence);
                Ok(held)
            }))
            .unwrap();
        assert!(held.contains(&denom) && held.contains(&stored), "{held:?}");
        assert_eq!(stored_locks(&s, w), 1);
        drop(s);
        f.reopen();
        let s = Arc::clone(&f.session);
        assert_eq!(stored_locks(&s, w), 1);
        assert_eq!(f.credit(2, DENOMINATION), denom);
        let inner = Arc::clone(&s);
        let (set, flag, balance) = f
            .engine
            .block_on(s.on_runtime(async move {
                wallet_event(&inner, w).await;
                let set = lock_set(&inner, w).await;
                Ok((set, locked(&inner, w, denom).await, confirmed(&inner, w)))
            }))
            .unwrap();
        assert_eq!(set, vec![stored]);
        assert_eq!(flag, Some(false));
        assert_eq!(balance, DENOMINATION);
    }

    /// A lock the library or the user set on a denomination before a
    /// funding's fence is not the fence's: its release leaves it locked,
    /// and wallet.sqlite still holds it after a restart.
    #[test]
    fn a_lock_set_before_the_fence_outlives_its_release() {
        let mut f = crate::platform::lease::session_tests::fixture(false);
        f.credit(1, 100_000_000);
        let denom = f.credit(2, DENOMINATION);
        let other = f.credit(3, DENOMINATION);
        let (s, w) = (Arc::clone(&f.session), f.wallet);
        let inner = Arc::clone(&s);
        let (during, after, other_after, set) = f
            .engine
            .block_on(s.on_runtime(async move {
                let wallet = inner.wallet(&w).await.unwrap();
                assert!(wallet.lock_outpoint(denom).await.unwrap());
                let fence = funding_fence(&inner, w).await;
                let during = locked(&inner, w, other).await;
                drop(fence);
                wallet_event(&inner, w).await;
                let after = locked(&inner, w, denom).await;
                let other_after = locked(&inner, w, other).await;
                Ok((during, after, other_after, lock_set(&inner, w).await))
            }))
            .unwrap();
        assert_eq!(during, Some(true), "the fence held the other denomination");
        assert_eq!(
            after,
            Some(true),
            "the release unlocked a lock it did not make"
        );
        assert_eq!(other_after, Some(false));
        assert_eq!(set, vec![denom]);
        assert_eq!(stored_locks(&s, w), 1);
        drop(s);
        f.reopen();
        let s = Arc::clone(&f.session);
        let inner = Arc::clone(&s);
        let set = f
            .engine
            .block_on(s.on_runtime(async move { Ok(lock_set(&inner, w).await) }))
            .unwrap();
        assert_eq!(set, vec![denom]);
        assert_eq!(stored_locks(&s, w), 1);
    }

    #[test]
    fn internal_chain_detection() {
        let change = key_wallet::DerivationPath::from_str("m/44'/1'/0'/1/7").unwrap();
        let receive = key_wallet::DerivationPath::from_str("m/44'/1'/0'/0/7").unwrap();
        assert!(is_internal_path(&change));
        assert!(!is_internal_path(&receive));
    }

    fn coin(seed: u8, send_account: bool) -> WalletCoin {
        let address = Address::new(
            dashcore::Network::Regtest,
            dashcore::address::Payload::PubkeyHash(dashcore::PubkeyHash::from_byte_array(
                [seed; 20],
            )),
        );
        let utxo = Utxo::new(
            OutPoint::new(Txid::from_byte_array([seed; 32]), 0),
            dashcore::TxOut {
                value: 1_000 * u64::from(seed),
                script_pubkey: address.script_pubkey(),
            },
            address,
            100,
            false,
        );
        WalletCoin {
            utxo,
            send_account,
            coinjoin_account: !send_account,
            counted_denomination: false,
            is_change: false,
            block_time: None,
            chain_locked: false,
            foreign_incoming: false,
            user_locked: false,
            dust_locked: false,
            reserved: false,
            lock_input: false,
            unresolved_lock: false,
            awaiting_history: false,
            excluded: None,
        }
    }

    /// E0-04 §2a.6: the funding refusal names every held coin with its
    /// hold, a dust lock apart from a user lock, and an unresolved signed
    /// lock's input (review Opus O-2); a tracked lock's input, a free coin
    /// and a CoinJoin coin hold nothing back.
    #[test]
    fn held_funding_coins_name_their_hold() {
        let user = WalletCoin {
            user_locked: true,
            ..coin(1, true)
        };
        let dust = WalletCoin {
            user_locked: true,
            dust_locked: true,
            ..coin(2, true)
        };
        let paying = WalletCoin {
            reserved: true,
            ..coin(3, true)
        };
        let paying_and_locked = WalletCoin {
            reserved: true,
            user_locked: true,
            dust_locked: true,
            ..coin(4, true)
        };
        let lock_input = WalletCoin {
            reserved: true,
            lock_input: true,
            ..coin(5, true)
        };
        let coinjoin = WalletCoin {
            user_locked: true,
            ..coin(6, false)
        };
        let unresolved = WalletCoin {
            reserved: true,
            lock_input: true,
            unresolved_lock: true,
            awaiting_history: false,
            excluded: None,
            ..coin(8, true)
        };
        let snapshot = CoinSnapshot {
            height: 200,
            coins: vec![
                user.clone(),
                dust.clone(),
                paying.clone(),
                paying_and_locked.clone(),
                lock_input,
                coin(7, true),
                coinjoin,
                unresolved.clone(),
            ],
        };
        let held: Vec<HeldCoin> = snapshot.held_funding_coins().collect();
        let expect = |c: &WalletCoin, hold| HeldCoin {
            outpoint: c.utxo.outpoint,
            value: c.utxo.value(),
            hold,
        };
        assert_eq!(
            held,
            [
                expect(&user, CoinHold::UserLock),
                expect(&dust, CoinHold::DustLock),
                expect(&paying, CoinHold::Reserved),
                expect(&paying_and_locked, CoinHold::Reserved),
                expect(&unresolved, CoinHold::UnresolvedLock),
            ]
        );
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
