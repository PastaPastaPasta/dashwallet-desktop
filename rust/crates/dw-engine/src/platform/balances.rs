//! The identities' credit balances as `identities()` shows them (DP1-06,
//! review Sol r1): the level a balance sets follows the spending.
//!
//! - Every credit-spending write (a name, a contact request or accept, the
//!   key upgrade, a profile, private details) marks its identity's balance
//!   spent and, once its write lock is released, refreshes it in the
//!   background ([`AfterSpend`], bound before the write lock so it drops
//!   after it).
//! - The balance pass refreshes every balance that is stale: with each
//!   DashPay pass the engine runs (`kick_dashpay_sync`), and every
//!   [`BALANCE_PASS_EVERY`] while SPV runs.
//! - `IdentitySummary.balance_as_of` is the Platform block time of the
//!   balance shown, and `balance_stale` says it may be behind ([`stale`]):
//!   spent since, or neither read by this process nor stored by Platform
//!   within [`BALANCE_MAX_AGE`]. A mark and a refresh each tell the host
//!   (`Platform{Identities}`), so it reads the flag again.
//!
//! A refresh is the library's proved fetch (`refresh_identity_balance`),
//! stored with its block time. It clears a spend only with a balance whose
//! block time is at or after the spend's end by this machine's clock: a
//! node a block behind answers the balance before the spend, and Platform
//! may have no balance at all. A background refresh holds neither the
//! session nor its operation gate (it reaches the library through weak
//! handles); `close` aborts and awaits them before the wallets unload
//! ([`Balances::close`]), and none starts after.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dpp::identity::accessors::IdentityGettersV0;
use dpp::prelude::Identifier;
use tokio::runtime::Handle;
use tokio::task::JoinSet;

use super::errors::PlatformError;
use crate::events::SessionHub;
use crate::session::Manager;
use crate::{NetworkSession, PlatformChange, WalletId};

/// A balance neither read by this process nor stored by Platform (its
/// block time) within this is stale.
pub(crate) const BALANCE_MAX_AGE: Duration = Duration::from_secs(600);
/// How often the balance pass runs while SPV runs, besides each DashPay
/// pass.
pub(crate) const BALANCE_PASS_EVERY: Duration = Duration::from_secs(60);

/// Whether a balance may be behind at `now` (ms): `spent` since its
/// refresh, or neither `refreshed` (this process's last refresh, by its
/// clock) nor `as_of` (Platform's block time of the balance stored) within
/// [`BALANCE_MAX_AGE`].
pub(crate) fn stale(as_of: Option<u64>, refreshed: Option<u64>, spent: bool, now: u64) -> bool {
    let recent = |t: Option<u64>| {
        t.is_some_and(|t| now.saturating_sub(t) <= BALANCE_MAX_AGE.as_millis() as u64)
    };
    spent || !(recent(refreshed) || recent(as_of))
}

pub(crate) fn now_ms() -> u64 {
    let since = SystemTime::now().duration_since(UNIX_EPOCH);
    since.map_or(0, |d| d.as_millis() as u64)
}

/// A refresh's answer: the balance and the Platform block time (ms) of the
/// balance the library now holds; `None` when Platform has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Refreshed {
    pub(crate) balance: u64,
    pub(crate) as_of: Option<u64>,
}

#[cfg(test)]
pub(crate) type TestRefresh =
    Arc<dyn Fn(WalletId, Identifier) -> Result<Option<Refreshed>, PlatformError> + Send + Sync>;

/// A spend not yet refreshed: its mark and its end by this machine's clock.
#[derive(Debug, Clone, Copy)]
struct Spend {
    mark: u64,
    at: u64,
}

/// The handles a background refresh reaches the session through.
#[derive(Clone)]
pub(crate) struct Reach {
    rt: Handle,
    manager: Weak<Manager>,
    hub: Weak<SessionHub>,
}

impl Reach {
    /// Tells the host `wallet`'s identities changed (`balance_stale`).
    fn notify(&self, wallet: WalletId) {
        if let Some(hub) = self.hub.upgrade() {
            hub.pump.mark_platform(wallet, [PlatformChange::Identities]);
        }
    }
}

type Key = (WalletId, Identifier);

/// The balances spent since their refresh, the refreshes' times, and the
/// background refreshes.
pub(crate) struct Balances {
    spent: Mutex<HashMap<Key, Spend>>,
    /// Identity → this process's last successful refresh (ms).
    refreshed: Mutex<HashMap<Key, u64>>,
    marks: AtomicU64,
    /// The background refreshes; `None` once closed, when none starts.
    tasks: Mutex<Option<JoinSet<()>>>,
    /// One balance pass at a time; a pass asked for while one runs is
    /// skipped (the running one reads the same state).
    pass: tokio::sync::Mutex<()>,
    /// Tests: refreshes in place of the library. A background refresh
    /// without it does nothing, so no test reaches for the network.
    #[cfg(test)]
    pub(crate) test_refresh: Mutex<Option<TestRefresh>>,
}

fn guard<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Default for Balances {
    fn default() -> Self {
        Self {
            spent: Mutex::default(),
            refreshed: Mutex::default(),
            marks: AtomicU64::default(),
            tasks: Mutex::new(Some(JoinSet::new())),
            pass: tokio::sync::Mutex::default(),
            #[cfg(test)]
            test_refresh: Mutex::default(),
        }
    }
}

impl Balances {
    fn mark(&self, wallet: WalletId, identity: Identifier) {
        let mark = self.marks.fetch_add(1, Ordering::SeqCst) + 1;
        let spend = Spend { mark, at: now_ms() };
        guard(&self.spent).insert((wallet, identity), spend);
    }

    /// Whether a write spent `identity`'s credits since its last refresh.
    pub(crate) fn spent(&self, wallet: WalletId, identity: Identifier) -> bool {
        guard(&self.spent).contains_key(&(wallet, identity))
    }

    /// [`stale`] for `identity`, whose stored balance is of `as_of`.
    pub(crate) fn stale(&self, wallet: WalletId, identity: Identifier, as_of: Option<u64>) -> bool {
        let refreshed = guard(&self.refreshed).get(&(wallet, identity)).copied();
        stale(as_of, refreshed, self.spent(wallet, identity), now_ms())
    }

    pub(crate) fn forget(&self, wallet: &WalletId) {
        guard(&self.spent).retain(|(w, _), _| w != wallet);
        guard(&self.refreshed).retain(|(w, _), _| w != wallet);
    }

    /// Refreshes `identity`'s balance in the library (proved, stored with
    /// its block time). One that succeeds clears a spend marked before it
    /// began, only with a balance of the spend's end or later (module doc).
    pub(crate) async fn refresh(
        &self,
        manager: &Manager,
        wallet: WalletId,
        identity: Identifier,
    ) -> Result<Option<Refreshed>, PlatformError> {
        let began = self.marks.load(Ordering::SeqCst);
        let refreshed = self.read(manager, wallet, identity).await;
        if let Ok(answer) = refreshed {
            let key = (wallet, identity);
            guard(&self.refreshed).insert(key, now_ms());
            let as_of = answer.and_then(|r| r.as_of);
            if let Some(as_of) = as_of {
                (guard(&self.spent)).retain(|k, s| *k != key || s.mark > began || as_of < s.at);
            }
        }
        refreshed
    }

    /// The library's refresh, or the tests' hook in its place.
    async fn read(
        &self,
        manager: &Manager,
        wallet: WalletId,
        identity: Identifier,
    ) -> Result<Option<Refreshed>, PlatformError> {
        #[cfg(test)]
        {
            let hook = guard(&self.test_refresh).clone();
            if let Some(hook) = hook {
                return hook(wallet, identity);
            }
        }
        fetch(manager, wallet, identity).await
    }

    /// Spawns `task` unless closed; reaps the finished ones.
    fn spawn(&self, rt: &Handle, task: impl Future<Output = ()> + Send + 'static) {
        let mut tasks = guard(&self.tasks);
        if let Some(set) = tasks.as_mut() {
            while set.try_join_next().is_some() {}
            set.spawn_on(task, rt);
        }
    }

    /// Aborts the background refreshes and waits for them, and starts none
    /// after: the session's `close`, before the wallets unload.
    pub(crate) async fn close(&self) {
        let set = guard(&self.tasks).take();
        if let Some(mut set) = set {
            set.abort_all();
            while set.join_next().await.is_some() {}
        }
    }

    /// Refreshes in the background, best effort: `only` one identity, or
    /// every stale one of every wallet (the balance pass).
    pub(crate) fn kick(self: &Arc<Self>, reach: &Reach, only: Option<Key>) {
        #[cfg(test)]
        if guard(&self.test_refresh).is_none() {
            return;
        }
        let (this, reach) = (Arc::clone(self), reach.clone());
        let rt = reach.rt.clone();
        self.spawn(&rt, async move {
            let (due, _pass) = match only {
                Some(one) => (vec![one], None),
                None => {
                    let Ok(pass) = this.pass.try_lock() else {
                        return;
                    };
                    let Some(manager) = reach.manager.upgrade() else {
                        return;
                    };
                    (this.stale_identities(&manager).await, Some(pass))
                }
            };
            for (wallet, identity) in due {
                let Some(manager) = reach.manager.upgrade() else {
                    return;
                };
                let refreshed = this.refresh(&manager, wallet, identity).await;
                drop(manager);
                match refreshed {
                    Ok(_) => reach.notify(wallet),
                    Err(e) => tracing::debug!(wallet_id = %wallet, error = %e, "a balance refresh failed; it stays stale"),
                }
            }
        });
    }

    /// Every identity of every wallet whose balance is [`stale`].
    async fn stale_identities(&self, manager: &Manager) -> Vec<Key> {
        let ids = manager.wallet_ids().await;
        let wm = manager.wallet_manager_arc();
        let wm = wm.read().await;
        let mut out = Vec::new();
        for id in ids.into_iter().map(WalletId) {
            let Some(info) = wm.get_wallet_info(&id.0) else {
                continue;
            };
            for m in info.identity_manager.wallet_managed_identities(&id.0) {
                let identity = m.identity.id();
                let as_of = m.last_updated_balance_block_time.map(|t| t.timestamp);
                if self.stale(id, identity, as_of) {
                    out.push((id, identity));
                }
            }
        }
        out
    }
}

/// The library's proved refresh of `identity`'s balance, with the block
/// time of the balance it then holds.
async fn fetch(
    manager: &Manager,
    wallet: WalletId,
    identity: Identifier,
) -> Result<Option<Refreshed>, PlatformError> {
    let w = (manager.get_wallet(&wallet.0).await).ok_or(PlatformError::Unavailable)?;
    let balance = match w.identity().refresh_identity_balance(&identity).await {
        Ok(balance) => balance,
        Err(platform_wallet::PlatformWalletError::IdentityBalanceUnavailable(_)) => {
            return Ok(None);
        }
        Err(e) => return Err(e.into()),
    };
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    let as_of = (wm.get_wallet_info(&wallet.0))
        .and_then(|info| {
            (info
                .identity_manager
                .wallet_managed_identities(&wallet.0)
                .into_iter())
            .find(|m| m.identity.id() == identity)
            .and_then(|m| m.last_updated_balance_block_time)
        })
        .map(|t| t.timestamp);
    Ok(Some(Refreshed { balance, as_of }))
}

/// Bound before a credit-spending write's write lock (so it drops after
/// it): marks the identity's balance spent, tells the host, and refreshes
/// it in the background (module doc).
#[must_use]
pub(crate) struct AfterSpend {
    balances: Arc<Balances>,
    reach: Reach,
    key: Key,
}

impl Drop for AfterSpend {
    fn drop(&mut self) {
        let (wallet, identity) = self.key;
        self.balances.mark(wallet, identity);
        self.reach.notify(wallet);
        self.balances.kick(&self.reach, Some(self.key));
    }
}

impl NetworkSession {
    fn reach(&self) -> Reach {
        Reach {
            rt: self.rt.clone(),
            manager: self
                .manager()
                .map_or_else(|_| Weak::new(), |m| Arc::downgrade(&m)),
            hub: Arc::downgrade(&self.hub),
        }
    }

    /// The [`AfterSpend`] of a write that spends `identity`'s credits.
    pub(crate) fn after_spend(&self, wallet: WalletId, identity: Identifier) -> AfterSpend {
        AfterSpend {
            balances: Arc::clone(&self.platform.balances),
            reach: self.reach(),
            key: (wallet, identity),
        }
    }

    /// Runs the balance pass in the background (module doc).
    pub(crate) fn kick_balance_pass(&self) {
        (self.platform.balances).kick(&self.reach(), None);
    }

    /// `DashPay::refresh_balance`: [`Balances::refresh`] now.
    pub(crate) async fn refresh_balance_of(
        &self,
        wallet: WalletId,
        identity: Identifier,
    ) -> Result<Option<u64>, PlatformError> {
        let manager = self.manager()?;
        let refreshed = (self.platform.balances)
            .refresh(&manager, wallet, identity)
            .await?;
        self.reach().notify(wallet);
        Ok(refreshed.map(|r| r.balance))
    }
}

#[cfg(test)]
mod tests {
    use dpp::identity::Identity;
    use dpp::identity::v0::IdentityV0;
    use dpp::platform_value::string_encoding::Encoding;
    use platform_wallet::BlockTime;

    use super::super::identity_writes::identity_locks;
    use super::super::topup_tests::{Fixture, eventually, fixture};
    use super::*;

    const HOUR_MS: u64 = 3_600_000;

    #[test]
    fn a_balance_is_stale_when_old_spent_or_never_refreshed() {
        let now = 10 * HOUR_MS;
        let max = BALANCE_MAX_AGE.as_millis() as u64;
        assert!(stale(None, None, false, now));
        assert!(!stale(Some(now - max), None, false, now));
        assert!(stale(Some(now - max - 1), None, false, now));
        assert!(stale(Some(now), None, true, now));
        // A block time ahead of the clock is fresh.
        assert!(!stale(Some(now + HOUR_MS), None, false, now));
        // A refresh of this process counts, whatever Platform's block
        // time (a stalled chain, a clock ahead, no balance at all).
        assert!(!stale(None, Some(now - max), false, now));
        assert!(!stale(Some(now - HOUR_MS), Some(now), false, now));
        assert!(stale(Some(now - HOUR_MS), Some(now - max - 1), false, now));
        assert!(stale(None, Some(now), true, now));
    }

    /// The refreshes the hook saw, and whether the identity's write lock
    /// was free at each.
    type Seen = Arc<Mutex<Vec<(Identifier, bool)>>>;

    /// What a hooked refresh answers.
    #[derive(Clone, Copy)]
    enum Answer {
        /// A balance of the refresh's moment.
        Now,
        /// A balance of an hour before.
        Behind,
        /// Platform has none.
        NoBalance,
        Fails,
    }

    /// Refreshes through a hook that records each call and answers
    /// `answer`.
    fn hook(f: &Fixture, answer: Answer) -> Seen {
        let seen = Seen::default();
        let record = Arc::clone(&seen);
        let refresh: TestRefresh = Arc::new(move |wallet, identity| {
            let free = identity_locks(wallet, identity).write.try_lock().is_ok();
            record.lock().unwrap().push((identity, free));
            let at = |as_of| {
                Ok(Some(Refreshed {
                    balance: 5,
                    as_of: Some(as_of),
                }))
            };
            match answer {
                Answer::Now => at(now_ms()),
                Answer::Behind => at(now_ms() - HOUR_MS),
                Answer::NoBalance => Ok(None),
                Answer::Fails => Err(PlatformError::Unavailable),
            }
        });
        *f.session.platform.balances.test_refresh.lock().unwrap() = Some(refresh);
        seen
    }

    /// A credit-spending write marks the balance spent once it ends and
    /// refreshes it after its write lock is released; a refresh that works
    /// clears the mark, one that fails keeps it (review Sol r1).
    #[test]
    fn a_spend_refreshes_the_balance_once_the_write_lock_is_released() {
        let f = fixture(false);
        let balances = Arc::clone(&f.session.platform.balances);
        let id = Identifier::new([9; 32]);
        let seen = hook(&f, Answer::Now);
        let locks = identity_locks(f.wallet, id);
        {
            let _refresh = f.session.after_spend(f.wallet, id);
            let _write = f.engine.block_on(locks.write.lock());
            assert!(!balances.spent(f.wallet, id));
        }
        eventually("the refresh", || !seen.lock().unwrap().is_empty());
        assert_eq!(*seen.lock().unwrap(), vec![(id, true)]);
        eventually("the mark cleared", || !balances.spent(f.wallet, id));

        let seen = hook(&f, Answer::Fails);
        drop(f.session.after_spend(f.wallet, id));
        eventually("the failed refresh", || !seen.lock().unwrap().is_empty());
        assert!(balances.spent(f.wallet, id));
    }

    /// A refresh clears a spend only with a balance of the spend's end or
    /// later: a node behind, or no balance at all, leaves it marked (review
    /// F3).
    #[test]
    fn only_a_balance_after_the_spend_clears_it() {
        let f = fixture(false);
        let balances = Arc::clone(&f.session.platform.balances);
        let id = Identifier::new([9; 32]);
        for answer in [Answer::Behind, Answer::NoBalance] {
            let seen = hook(&f, answer);
            drop(f.session.after_spend(f.wallet, id));
            eventually("the refresh", || !seen.lock().unwrap().is_empty());
            assert!(balances.spent(f.wallet, id));
        }
        let seen = hook(&f, Answer::Now);
        f.engine
            .block_on(f.session.refresh_balance_of(f.wallet, id))
            .unwrap();
        assert!(!seen.lock().unwrap().is_empty());
        assert!(!balances.spent(f.wallet, id));
    }

    /// A close aborts the background refreshes, and none starts after it
    /// (review F2).
    #[test]
    fn no_background_refresh_outlives_a_close() {
        let f = fixture(false);
        let balances = Arc::clone(&f.session.platform.balances);
        let id = Identifier::new([9; 32]);
        let seen = hook(&f, Answer::Now);
        f.engine.block_on(balances.close());
        assert!(guard(&balances.tasks).is_none());
        drop(f.session.after_spend(f.wallet, id));
        f.session.kick_balance_pass();
        f.engine.block_on(tokio::task::yield_now());
        assert!(guard(&balances.tasks).is_none());
        assert!(seen.lock().unwrap().is_empty(), "no refresh ran");
        assert!(balances.spent(f.wallet, id), "the spend is still marked");
    }

    /// Gives the wallet identities `fresh` (its balance refreshed now) and
    /// `old` (never refreshed).
    fn give_identities(f: &Fixture, fresh: Identifier, old: Identifier) {
        let manager = f.session.manager().unwrap();
        let wallet = f.wallet;
        f.engine.block_on(async move {
            let w = manager.get_wallet(&wallet.0).await.unwrap();
            let wm = manager.wallet_manager_arc();
            let mut wm = wm.write().await;
            let info = wm.get_wallet_info_mut(&wallet.0).unwrap();
            for (index, id) in [(0, fresh), (1, old)] {
                let identity = Identity::V0(IdentityV0 {
                    id,
                    public_keys: Default::default(),
                    balance: 1_000,
                    revision: 0,
                });
                (info.identity_manager)
                    .add_identity(identity, index, wallet.0, w.persister())
                    .unwrap();
            }
            let m = info.identity_manager.managed_identity_mut(&fresh).unwrap();
            m.last_updated_balance_block_time = Some(BlockTime::new(10, 10, now_ms()));
        });
    }

    /// The balance pass refreshes the stale balances only; the summaries
    /// show each balance's block time and whether it is stale.
    #[test]
    fn the_balance_pass_refreshes_stale_balances_and_the_summary_says_which() {
        let f = fixture(false);
        let (fresh, old) = (Identifier::new([1; 32]), Identifier::new([2; 32]));
        give_identities(&f, fresh, old);
        let summaries = f.session.identity_summaries(f.wallet).unwrap();
        let of = |id: Identifier| {
            let id = id.to_string(Encoding::Base58);
            let s = summaries.iter().find(|s| s.identity == id).unwrap();
            (s.balance_as_of.is_some(), s.balance_stale)
        };
        assert_eq!((of(fresh), of(old)), ((true, false), (false, true)));

        let seen = hook(&f, Answer::Now);
        f.session.kick_balance_pass();
        eventually("the pass", || !seen.lock().unwrap().is_empty());
        assert_eq!(*seen.lock().unwrap(), vec![(old, true)]);

        // A spend makes the fresh one stale until its refresh.
        *f.session.platform.balances.test_refresh.lock().unwrap() = None;
        f.session.platform.balances.mark(f.wallet, fresh);
        let summary = (f.session.identity_summaries(f.wallet).unwrap().into_iter())
            .find(|s| s.identity == fresh.to_string(Encoding::Base58))
            .unwrap();
        assert!(summary.balance_stale && summary.balance_as_of.is_some());
    }
}
