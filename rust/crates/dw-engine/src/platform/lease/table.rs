//! The lease table (E0-04 §4): its mutex J and what lives under it.
//!
//! J is a leaf (H1): every critical section is a synchronous step that
//! takes no other lock, does no I/O and never awaits. What a step decides
//! to drop (a `KeyHold`, signers) and the events it decides to send are
//! collected in [`Effects`] and run once J is released, so a drop's erase
//! or a host observer never runs under J.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use dw_vault::{KeyHold, VaultSigner};
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::budget::{Budget, Charge};
use super::fence::FenceState;
use super::{DispatchScope, LeaseConfig, LeaseError, LeaseId, Origin, lease_string};
use crate::platform::flows::{
    BudgetPurpose, BudgetView, FlowKind, LeaseStateView, LeaseView, ParkReason, RevokeCause,
};
use crate::platform::signers::{VaultContactCrypto, VaultIdentitySigner};
use crate::{DashNetwork, EngineEvent, EventSink, WalletId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaseState {
    Active,
    /// Funding handed off with an own key; the key lives until `key_until`.
    AwaitingProof,
    Parked,
    NeedsGrant,
    Revoked(RevokeCause),
    Ended,
}

impl LeaseState {
    /// Signs and admits Firsts.
    pub(crate) fn signs(self) -> bool {
        matches!(self, Self::Active | Self::AwaitingProof)
    }

    /// Admits a First (§4.3): every state but the two terminal ones.
    pub(crate) fn live(self) -> bool {
        !matches!(self, Self::Revoked(_) | Self::Ended)
    }

    fn refusal(self, purpose: BudgetPurpose) -> LeaseError {
        match self {
            Self::Parked => LeaseError::Parked(purpose),
            Self::Revoked(cause) => LeaseError::Revoked(cause),
            Self::Ended => LeaseError::Ended,
            Self::Active | Self::AwaitingProof | Self::NeedsGrant => {
                LeaseError::NeedsGrant(purpose)
            }
        }
    }
}

/// The scoped signers a lease issued from its tokens (§4.2).
#[derive(Default)]
pub(crate) struct Signers {
    pub(crate) funding: Option<VaultSigner>,
    pub(crate) identity: Option<VaultSigner>,
    pub(crate) crypto: Option<VaultSigner>,
    pub(crate) spend: Option<VaultSigner>,
}

/// What a redemption produced (§4.1 step 2): the caps per purpose, the one
/// `KeyHold` and the signers. The tokens are already dropped.
#[derive(Default)]
pub(crate) struct Issued {
    pub(crate) funding: u64,
    pub(crate) credits: u64,
    pub(crate) spend: u64,
    pub(crate) crypto: bool,
    pub(crate) key: Option<KeyHold>,
    pub(crate) signers: Signers,
    /// The vault epoch before the first redemption and after the last
    /// signer: equal unless an epoch change raced the redemption.
    pub(crate) epoch_before: u64,
    pub(crate) epoch_after: u64,
}

pub(crate) struct Entry {
    pub(crate) wallet: WalletId,
    pub(crate) flow: FlowKind,
    pub(crate) state: LeaseState,
    pub(crate) funding: Budget,
    pub(crate) credits: Budget,
    pub(crate) spend: Budget,
    pub(crate) crypto: bool,
    pub(crate) key: Option<KeyHold>,
    pub(crate) key_until: Option<Instant>,
    /// Issued on a vault without its full-scope key (§4.4).
    pub(crate) own_key: bool,
    pub(crate) signers: Signers,
    /// Flow tasks registered with the lease; close aborts them (§8.5).
    pub(crate) tasks: Vec<AbortHandle>,
    pub(crate) last_use: Instant,
    /// Every artifact committed under the lease, in order; only grows
    /// (§8.4).
    pub(crate) history: Vec<super::ArtifactId>,
    /// Permits held now.
    pub(crate) permits: u32,
    /// Library calls of the flow running now (Mode B's call permits).
    pub(crate) calls: u32,
    pub(crate) ended_at: Option<Instant>,
}

impl Entry {
    fn budget_mut(&mut self, purpose: BudgetPurpose) -> Option<&mut Budget> {
        match purpose {
            BudgetPurpose::Funding => Some(&mut self.funding),
            BudgetPurpose::Credits => Some(&mut self.credits),
            BudgetPurpose::Spend => Some(&mut self.spend),
            BudgetPurpose::Crypto => None,
        }
    }

    /// Drops the key and the signers into `fx`.
    fn disarm(&mut self, fx: &mut Effects) {
        if let Some(key) = self.key.take() {
            fx.drop_later(key);
        }
        fx.drop_later(std::mem::take(&mut self.signers));
        self.key_until = None;
    }

    /// Moves to `Revoked{cause}` unless already terminal.
    pub(crate) fn revoke(&mut self, cause: RevokeCause, fx: &mut Effects) -> bool {
        if !self.state.live() {
            return false;
        }
        self.state = LeaseState::Revoked(cause);
        self.disarm(fx);
        true
    }

    /// An epoch change other than a lock (§4.3): `NeedsGrant`, key dropped.
    pub(crate) fn needs_grant(&mut self, fx: &mut Effects) -> bool {
        if !self.state.live() || self.state == LeaseState::NeedsGrant {
            return false;
        }
        self.state = LeaseState::NeedsGrant;
        self.disarm(fx);
        true
    }

    pub(crate) fn park(&mut self, fx: &mut Effects) -> bool {
        if !self.state.signs() {
            return false;
        }
        self.state = LeaseState::Parked;
        self.disarm(fx);
        true
    }

    pub(crate) fn refund(&mut self, purpose: BudgetPurpose, charge: Charge) {
        if let Some(b) = self.budget_mut(purpose) {
            b.refund(charge);
        }
    }

    /// Whether the idle reaper may end it: a vault-key lease with no call,
    /// no permit and no running flow task for `idle`.
    fn idle(&self, now: Instant, idle: std::time::Duration) -> bool {
        !self.own_key
            && self.state.live()
            && self.permits == 0
            && self.calls == 0
            && self.tasks.iter().all(AbortHandle::is_finished)
            && now.saturating_duration_since(self.last_use) >= idle
    }
}

/// The lock barrier (§8.1): set while any lock's gate is pending or any
/// drain runs. Per-wallet drains (wallet removal and close) bar only their
/// wallet.
#[derive(Debug, Default)]
pub(crate) struct Barrier {
    pub(crate) gates: u32,
    pub(crate) drains: u32,
    pub(crate) wallet_drains: HashMap<WalletId, u32>,
}

impl Barrier {
    pub(crate) fn set_for(&self, wallet: &WalletId) -> bool {
        self.gates > 0 || self.drains > 0 || self.wallet_drains.get(wallet).is_some_and(|n| *n > 0)
    }
}

/// The background `DashPayCrypto` lease of a wallet (§4.5).
pub(crate) struct Background {
    pub(crate) crypto: VaultContactCrypto,
}

pub(crate) struct Inner {
    pub(crate) lock_gen: u64,
    /// Per-wallet revocations (removal, close) bump this instead of
    /// `lock_gen`.
    pub(crate) wallet_gen: HashMap<WalletId, u64>,
    /// The highest vault epoch any step has seen; epochs only grow, so a
    /// stale reading never moves it back.
    pub(crate) observed_epoch: u64,
    /// Set by close's freeze, never cleared: nothing is created after it.
    pub(crate) closed: bool,
    pub(crate) barrier: Barrier,
    pub(crate) leases: HashMap<LeaseId, Entry>,
    pub(crate) background: HashMap<WalletId, Background>,
    pub(crate) fence: FenceState,
    #[cfg(test)]
    pub(crate) log: Vec<super::stress_tests::LogEvent>,
    #[cfg(test)]
    pub(crate) mutation: Option<super::stress_tests::Mutation>,
}

impl Inner {
    fn gens(&self, wallet: &WalletId) -> (u64, u64) {
        (
            self.lock_gen,
            self.wallet_gen.get(wallet).copied().unwrap_or(0),
        )
    }

    #[cfg(test)]
    pub(crate) fn note(&mut self, e: super::stress_tests::LogEvent) {
        self.log.push(e);
    }
}

/// What a J step decided to do once J is released.
#[derive(Default)]
pub(crate) struct Effects {
    events: Vec<EngineEvent>,
    drops: Vec<Box<dyn Send>>,
    changed: BTreeSet<LeaseId>,
    notify: bool,
}

impl Effects {
    pub(crate) fn drop_later<T: Send + 'static>(&mut self, v: T) {
        self.drops.push(Box::new(v));
    }

    pub(crate) fn emit(&mut self, e: EngineEvent) {
        self.events.push(e);
    }

    /// The lease's view changed: `LeaseChanged` after J.
    pub(crate) fn changed(&mut self, id: LeaseId) {
        self.changed.insert(id);
    }

    /// Wake every waiter on the table (barrier, permits, gates).
    pub(crate) fn notify(&mut self) {
        self.notify = true;
    }
}

pub struct LeaseTable {
    j: Mutex<Inner>,
    /// Woken when the barrier, a gate, a permit or an attempt changes.
    pub(crate) changed: Notify,
    pub(crate) config: LeaseConfig,
    pub(crate) network: DashNetwork,
    sink: Arc<dyn EventSink>,
    pub(crate) rt: Handle,
    /// This process's journal nonce (diagnostics, §6.2).
    pub(crate) process: [u8; 16],
    pub(crate) journal: super::fence::JournalSlot,
}

impl std::fmt::Debug for LeaseTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LeaseTable")
    }
}

impl LeaseTable {
    pub(crate) fn new(
        network: DashNetwork,
        sink: Arc<dyn EventSink>,
        rt: Handle,
        config: LeaseConfig,
        epoch: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            j: Mutex::new(Inner {
                lock_gen: 0,
                wallet_gen: HashMap::new(),
                observed_epoch: epoch,
                closed: false,
                barrier: Barrier::default(),
                leases: HashMap::new(),
                background: HashMap::new(),
                fence: FenceState::default(),
                #[cfg(test)]
                log: Vec::new(),
                #[cfg(test)]
                mutation: None,
            }),
            changed: Notify::new(),
            config,
            network,
            sink,
            rt,
            process: rand::random(),
            journal: Default::default(),
        })
    }

    fn lock_j(&self) -> MutexGuard<'_, Inner> {
        self.j.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// One J step. `f` must not block; its effects run after J.
    pub(crate) fn with_j<R>(&self, f: impl FnOnce(&mut Inner, &mut Effects) -> R) -> R {
        let mut fx = Effects::default();
        let (r, views) = {
            let mut inner = self.lock_j();
            let r = f(&mut inner, &mut fx);
            let now = Instant::now();
            let views: Vec<LeaseView> = fx
                .changed
                .iter()
                .filter_map(|id| view_of(&inner, id, now))
                .collect();
            (r, views)
        };
        drop(std::mem::take(&mut fx.drops));
        for lease in views {
            self.sink.emit(EngineEvent::LeaseChanged {
                network: self.network.clone(),
                lease,
            });
        }
        for e in fx.events {
            self.sink.emit(e);
        }
        if fx.notify {
            self.changed.notify_waiters();
        }
        r
    }

    pub(crate) fn emit(&self, e: EngineEvent) {
        self.sink.emit(e);
    }

    /// Waits until `ready` holds under J. `ready` is checked after the
    /// waiter registered, so no wake-up is lost.
    pub(crate) async fn wait_until<R>(&self, mut ready: impl FnMut(&Inner) -> Option<R>) -> R {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(r) = ready(&self.lock_j()) {
                return r;
            }
            notified.await;
        }
    }

    /// H8 step 1: waits until no barrier bars `wallet`, then reads the
    /// generations.
    pub(crate) async fn wait_clear(&self, wallet: &WalletId) -> (u64, u64) {
        self.wait_until(|i| (!i.barrier.set_for(wallet)).then(|| i.gens(wallet)))
            .await
    }

    /// `wait_clear`, then `Locked` once close froze the table: nothing is
    /// redeemed for a lease that could never be inserted.
    async fn wait_open(&self, wallet: &WalletId) -> Result<(u64, u64), LeaseError> {
        let gens = self.wait_clear(wallet).await;
        if self.with_j(|i, _| i.closed) {
            return Err(LeaseError::Locked);
        }
        Ok(gens)
    }

    /// Notes an epoch change seen by a vault call other than a revoking
    /// one (§4.3, H9): every live lease moves to `NeedsGrant` and drops its
    /// key; the background leases go (the caller re-creates them).
    pub(crate) fn epoch_changed(&self, epoch: u64) {
        self.with_j(|i, fx| {
            if epoch <= i.observed_epoch {
                return;
            }
            i.observed_epoch = epoch;
            for (id, e) in i.leases.iter_mut() {
                if e.needs_grant(fx) {
                    fx.changed(*id);
                }
            }
            for (_, bg) in i.background.drain() {
                fx.drop_later(bg);
            }
            fx.notify();
        });
    }

    pub(crate) fn observe_epoch(&self, epoch: u64) {
        self.with_j(|i, _| i.observed_epoch = i.observed_epoch.max(epoch));
    }

    /// The table's lease `id` for `wallet`: `Invalid` when unknown or of
    /// another wallet (N-6).
    pub(crate) fn lease(
        self: &Arc<Self>,
        id: &LeaseId,
        wallet: &WalletId,
    ) -> Result<Lease, LeaseError> {
        self.with_j(|i, _| match i.leases.get(id) {
            Some(e) if e.wallet == *wallet => Ok(Lease {
                id: *id,
                wallet: e.wallet,
                flow: e.flow,
                table: Arc::clone(self),
            }),
            _ => Err(LeaseError::Invalid),
        })
    }

    /// §4.1 / H8: creates a lease from what `issue` redeems. `issue` runs on
    /// the blocking pool after the barrier cleared; `epoch` reads the
    /// vault's epoch right before the insert.
    pub(crate) async fn begin(
        self: &Arc<Self>,
        wallet: WalletId,
        flow: FlowKind,
        issue: impl FnOnce() -> Result<Issued, LeaseError> + Send + 'static,
        epoch: impl FnOnce() -> u64,
    ) -> Result<Lease, LeaseError> {
        let gens = self.wait_open(&wallet).await?;
        let issued = self
            .rt
            .spawn_blocking(issue)
            .await
            .map_err(|_| LeaseError::Vault("redemption panicked"))??;
        let epoch_now = epoch();
        let id: LeaseId = rand::random();
        let config = self.config;
        let inserted = self.with_j(|i, fx| {
            if i.closed || i.barrier.set_for(&wallet) || i.gens(&wallet) != gens {
                fx.drop_later(issued);
                return Err(LeaseError::Locked);
            }
            let now = Instant::now();
            let fresh = issued.epoch_before == issued.epoch_after
                && issued.epoch_after == epoch_now
                && i.observed_epoch == epoch_now;
            let own_key = issued.key.is_some();
            let mut entry = Entry {
                wallet,
                flow,
                state: LeaseState::Active,
                funding: Budget::new(issued.funding),
                credits: Budget::new(issued.credits),
                spend: Budget::new(issued.spend),
                crypto: issued.crypto,
                key: issued.key,
                key_until: own_key.then(|| now + config.key_ttl),
                own_key,
                signers: issued.signers,
                tasks: Vec::new(),
                last_use: now,
                history: Vec::new(),
                permits: 0,
                calls: 0,
                ended_at: None,
            };
            if !fresh {
                // Its tokens died with the epoch (an unlock, a scope change).
                entry.needs_grant(fx);
            }
            i.leases.insert(id, entry);
            #[cfg(test)]
            i.note(super::stress_tests::LogEvent::Begin { lease: id });
            fx.changed(id);
            Ok(own_key)
        })?;
        if inserted {
            self.spawn_key_timer(id);
        }
        Ok(Lease {
            id,
            wallet,
            flow,
            table: Arc::clone(self),
        })
    }

    /// Drops the key at `key_until` and parks the lease (§4.4). A later
    /// `proof_wait_started` moves `key_until`; the timer follows it.
    fn spawn_key_timer(self: &Arc<Self>, id: LeaseId) {
        let table = Arc::downgrade(self);
        self.rt.spawn(async move {
            loop {
                let Some(until) = with_table(&table, |t| {
                    t.with_j(|i, _| {
                        let e = i.leases.get(&id)?;
                        e.key.as_ref().and(e.key_until)
                    })
                })
                .flatten() else {
                    return;
                };
                tokio::time::sleep_until(until).await;
                let done = with_table(&table, |t| {
                    t.with_j(|i, fx| {
                        let Some(e) = i.leases.get_mut(&id) else {
                            return true;
                        };
                        match e.key_until {
                            Some(until) if e.key.is_some() && Instant::now() < until => false,
                            _ => {
                                if e.park(fx) {
                                    fx.changed(id);
                                }
                                true
                            }
                        }
                    })
                });
                if done != Some(false) {
                    return;
                }
            }
        });
    }

    /// Ends every vault-key lease idle for `config.idle`, and forgets ended
    /// leases that long after their end (their later use is then
    /// `grant_invalid` rather than `lease_expired`).
    pub(crate) fn reap(&self) {
        let idle = self.config.idle;
        self.with_j(|i, fx| {
            let now = Instant::now();
            for (id, e) in i.leases.iter_mut() {
                if e.idle(now, idle) {
                    end_entry(e, now, fx);
                    fx.changed(*id);
                }
            }
            i.leases.retain(|_, e| {
                e.permits > 0
                    || e.ended_at
                        .is_none_or(|at| now.saturating_duration_since(at) < idle)
            });
        });
    }

    /// Runs [`Self::reap`] every tenth of the idle period (at most a
    /// minute) while the table lives.
    pub(crate) fn start_reaper(self: &Arc<Self>) {
        let table = Arc::downgrade(self);
        let period = (self.config.idle / 10).min(std::time::Duration::from_secs(60));
        self.rt.spawn(async move {
            let mut tick = tokio::time::interval(period);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                if with_table(&table, |t| t.reap()).is_none() {
                    return;
                }
            }
        });
    }

    /// `end_flow`: ends lease `id` whatever its wallet. Idempotent.
    pub(crate) fn end(&self, id: &LeaseId) {
        self.with_j(|i, fx| {
            if let Some(e) = i.leases.get_mut(id) {
                end_entry(e, Instant::now(), fx);
                fx.changed(*id);
            }
        });
    }

    pub(crate) fn views(&self) -> Vec<LeaseView> {
        let now = Instant::now();
        let inner = self.lock_j();
        let mut ids: Vec<&LeaseId> = inner.leases.keys().collect();
        ids.sort();
        ids.into_iter()
            .filter_map(|id| view_of(&inner, id, now))
            .collect()
    }

    /// The background crypto of `wallet`, if one exists.
    pub(crate) fn background(&self, wallet: &WalletId) -> Option<VaultContactCrypto> {
        self.lock_j()
            .background
            .get(wallet)
            .map(|b| b.crypto.clone())
    }

    /// Inserts a background lease under H8's rule: refused if a barrier is
    /// set or `gens` moved since the creation began.
    pub(crate) fn insert_background(
        &self,
        wallet: WalletId,
        gens: (u64, u64),
        epoch: u64,
        crypto: VaultContactCrypto,
    ) -> bool {
        self.with_j(|i, fx| {
            if i.closed
                || i.barrier.set_for(&wallet)
                || i.gens(&wallet) != gens
                || i.observed_epoch != epoch
            {
                fx.drop_later(crypto);
                return false;
            }
            if let Some(old) = i.background.insert(wallet, Background { crypto }) {
                fx.drop_later(old);
            }
            true
        })
    }

    #[cfg(test)]
    pub(crate) fn inspect<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.lock_j())
    }
}

fn with_table<R>(table: &Weak<LeaseTable>, f: impl FnOnce(&LeaseTable) -> R) -> Option<R> {
    table.upgrade().map(|t| f(&t))
}

pub(crate) fn end_entry(e: &mut Entry, now: Instant, fx: &mut Effects) {
    if e.state != LeaseState::Ended {
        e.state = LeaseState::Ended;
        e.disarm(fx);
        e.ended_at = Some(now);
    }
}

fn view_of(inner: &Inner, id: &LeaseId, now: Instant) -> Option<LeaseView> {
    let e = inner.leases.get(id)?;
    let state = match e.state {
        LeaseState::Active => LeaseStateView::Active,
        LeaseState::AwaitingProof => LeaseStateView::AwaitingProof,
        LeaseState::Parked => LeaseStateView::Parked {
            reason: ParkReason::ProofWaiting,
        },
        LeaseState::NeedsGrant => LeaseStateView::NeedsGrant,
        LeaseState::Revoked(cause) => LeaseStateView::Revoked { cause },
        LeaseState::Ended => LeaseStateView::Ended,
    };
    let budgets = [
        (BudgetPurpose::Funding, &e.funding),
        (BudgetPurpose::Credits, &e.credits),
        (BudgetPurpose::Spend, &e.spend),
    ]
    .into_iter()
    .filter(|(_, b)| b.granted())
    .map(|(purpose, b)| BudgetView {
        purpose,
        ceiling: b.ceiling(),
        spent: b.spent(),
    })
    .collect();
    Some(LeaseView {
        id: hex::encode(id)[..8].to_owned(),
        wallet_id: e.wallet.to_string(),
        flow: e.flow,
        state,
        own_key: e.own_key,
        key_expires_in_secs: e
            .key
            .as_ref()
            .and(e.key_until)
            .map(|u| u.saturating_duration_since(now).as_secs()),
        funds_committed: inner.fence.funds_committed(e),
        budgets,
        in_flight: e.permits,
        call_running: e.calls > 0,
    })
}

/// A flow's handle on its lease (§4.7). Cheap to clone: the table owns the
/// entry. The handle does not end the lease when dropped; the flow calls
/// [`Lease::end`] (the facade's `end_flow`), and the idle reaper ends an
/// abandoned vault-key lease.
#[derive(Clone)]
pub struct Lease {
    pub(crate) id: LeaseId,
    pub(crate) wallet: WalletId,
    pub(crate) flow: FlowKind,
    pub(crate) table: Arc<LeaseTable>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("id", &hex::encode(&self.id[..4]))
            .field("flow", &self.flow)
            .finish_non_exhaustive()
    }
}

impl Lease {
    pub fn id(&self) -> LeaseId {
        self.id
    }

    /// The facade's id (`lease-<32 hex>`).
    pub fn id_string(&self) -> String {
        lease_string(&self.id)
    }

    pub fn wallet(&self) -> WalletId {
        self.wallet
    }

    pub fn flow(&self) -> FlowKind {
        self.flow
    }

    /// Runs `f` on a usable lease entry; refreshes its last use.
    fn with_entry<R>(
        &self,
        purpose: BudgetPurpose,
        f: impl FnOnce(&mut Entry) -> Result<R, LeaseError>,
    ) -> Result<R, LeaseError> {
        self.table.with_j(|i, _| {
            let e = i.leases.get_mut(&self.id).ok_or(LeaseError::Invalid)?;
            e.last_use = Instant::now();
            if !e.state.signs() {
                return Err(e.state.refusal(purpose));
            }
            f(e)
        })
    }

    fn signer(
        &self,
        purpose: BudgetPurpose,
        pick: impl FnOnce(&Signers) -> Option<&VaultSigner>,
        budget: impl FnOnce(&Entry) -> bool,
    ) -> Result<VaultSigner, LeaseError> {
        self.with_entry(purpose, |e| {
            if !budget(e) {
                return Err(LeaseError::NeedsGrant(purpose));
            }
            pick(&e.signers)
                .cloned()
                .ok_or(LeaseError::NeedsGrant(purpose))
        })
    }

    /// Identity-key signer for the identities at `identity_indices`
    /// (`PlatformIdentity`); refused once the Credits budget is 0.
    pub fn identity_signer(
        &self,
        identity_indices: &[u32],
    ) -> Result<VaultIdentitySigner, LeaseError> {
        let signer = self.signer(
            BudgetPurpose::Credits,
            |s| s.identity.as_ref(),
            |e| e.credits.available() > 0,
        )?;
        VaultIdentitySigner::new(signer, identity_indices.iter().copied())
            .map_err(|_| LeaseError::Vault("identity signer scope"))
    }

    /// The asset-lock funding signer (`PlatformFunding`, cap fixed at
    /// issue; the remaining budget is charged at `register`).
    pub fn funding_signer(&self) -> Result<VaultSigner, LeaseError> {
        self.signer(
            BudgetPurpose::Funding,
            |s| s.funding.as_ref(),
            |e| e.funding.available() > 0,
        )
    }

    /// The flow's DashPay crypto (`DashPayCrypto`); never hands off.
    pub fn contact_crypto(&self) -> Result<VaultContactCrypto, LeaseError> {
        let signer = self.signer(BudgetPurpose::Crypto, |s| s.crypto.as_ref(), |e| e.crypto)?;
        VaultContactCrypto::new(signer).map_err(|_| LeaseError::Vault("crypto signer scope"))
    }

    /// The `Spend` signer for `TxDraft` (DP3-01).
    pub(crate) fn spend_signer(&self) -> Result<VaultSigner, LeaseError> {
        self.signer(
            BudgetPurpose::Spend,
            |s| s.spend.as_ref(),
            |e| e.spend.available() > 0,
        )
    }

    /// Whether the lease carries a `Spend` grant at all.
    pub(crate) fn has_spend(&self) -> bool {
        self.table
            .with_j(|i, _| i.leases.get(&self.id).is_some_and(|e| e.spend.granted()))
    }

    /// Charges `amount` to the Spend budget before signing (§4.2); the
    /// charge is bound to the txid with `fence::bind_spend` once signed.
    pub(crate) fn charge_spend(&self, amount: u64) -> Result<Charge, LeaseError> {
        let charge = self.with_entry(BudgetPurpose::Spend, |e| {
            e.spend.charge(amount).map_err(|x| LeaseError::Exceeded {
                purpose: BudgetPurpose::Spend,
                needed: x.needed,
                remaining: x.remaining,
            })
        })?;
        self.table.with_j(|_, fx| fx.changed(self.id));
        Ok(charge)
    }

    pub(crate) fn refund(&self, purpose: BudgetPurpose, charge: Charge) {
        self.table.with_j(|i, fx| {
            if let Some(e) = i.leases.get_mut(&self.id) {
                e.refund(purpose, charge);
                fx.changed(self.id);
            }
        });
    }

    /// Runs `f` in this lease's dispatch scope.
    pub fn scope<F: Future>(&self, f: F) -> impl Future<Output = F::Output> {
        self.scope_step(None, f)
    }

    /// Runs `f` in this lease's scope for a resumable step (§7.6).
    pub fn scope_step<F: Future>(
        &self,
        step: Option<String>,
        f: F,
    ) -> impl Future<Output = F::Output> {
        DispatchScope {
            wallet: self.wallet,
            origin: Origin::Lease(self.id),
            step,
        }
        .run(f)
    }

    /// Drops the key at once and parks the lease (§4.4).
    pub fn park(&self) {
        self.table.with_j(|i, fx| {
            if let Some(e) = i.leases.get_mut(&self.id)
                && e.park(fx)
            {
                fx.changed(self.id);
            }
        });
    }

    /// Ends the lease. Idempotent; the entry stays until its last permit
    /// has dropped.
    pub fn end(&self) {
        self.table.end(&self.id);
    }

    /// Registers a task running the flow, so close can abort it (§8.5)
    /// and the reaper leaves the lease alone while it runs.
    pub fn register_task(&self, task: AbortHandle) {
        self.table.with_j(|i, _| match i.leases.get_mut(&self.id) {
            Some(e) => {
                e.tasks.retain(|t| !t.is_finished());
                e.tasks.push(task);
            }
            None => task.abort(),
        });
    }

    /// Mode B's call permit counter (selects C2); P2b takes it per call.
    pub fn call_running(&self) -> CallGuard {
        self.table.with_j(|i, fx| {
            if let Some(e) = i.leases.get_mut(&self.id) {
                e.calls += 1;
                e.last_use = Instant::now();
                fx.changed(self.id);
            }
        });
        CallGuard(self.clone())
    }

    pub fn view(&self) -> Option<LeaseView> {
        let now = Instant::now();
        let inner = self.table.lock_j();
        view_of(&inner, &self.id, now)
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> Option<LeaseState> {
        self.table
            .with_j(|i, _| i.leases.get(&self.id).map(|e| e.state))
    }

    /// §4.3's rebind: fresh authority from `issue`, under H8's rule, for a
    /// lease in `NeedsGrant` or `Parked` (L17). Each purpose's next generation is capped at
    /// `min(available, fresh)`; charges and permits stand.
    pub(crate) async fn rebind(
        &self,
        issue: impl FnOnce() -> Result<Issued, LeaseError> + Send + 'static,
        epoch: impl FnOnce() -> u64,
    ) -> Result<(), LeaseError> {
        let table = &self.table;
        let gens = table.wait_open(&self.wallet).await?;
        let issued = table
            .rt
            .spawn_blocking(issue)
            .await
            .map_err(|_| LeaseError::Vault("redemption panicked"))??;
        let epoch_now = epoch();
        let key_ttl = table.config.key_ttl;
        let own_key = table.with_j(|i, fx| {
            let fresh_epoch = issued.epoch_before == issued.epoch_after
                && issued.epoch_after == epoch_now
                && i.observed_epoch == epoch_now;
            let barred =
                i.closed || i.barrier.set_for(&self.wallet) || i.gens(&self.wallet) != gens;
            let Some(e) = i.leases.get_mut(&self.id) else {
                fx.drop_later(issued);
                return Err(LeaseError::Invalid);
            };
            if barred {
                fx.drop_later(issued);
                return Err(LeaseError::Locked);
            }
            match e.state {
                LeaseState::NeedsGrant | LeaseState::Parked => {}
                state if !state.live() => {
                    fx.drop_later(issued);
                    return Err(state.refusal(BudgetPurpose::Crypto));
                }
                _ => {
                    fx.drop_later(issued);
                    return Err(LeaseError::Invalid);
                }
            }
            if !fresh_epoch {
                fx.drop_later(issued);
                return Err(LeaseError::NeedsGrant(BudgetPurpose::Crypto));
            }
            e.funding.rebind(issued.funding);
            e.credits.rebind(issued.credits);
            e.spend.rebind(issued.spend);
            e.crypto = e.crypto && issued.crypto;
            let old = std::mem::replace(&mut e.signers, issued.signers);
            fx.drop_later(old);
            if let Some(old) = std::mem::replace(&mut e.key, issued.key) {
                fx.drop_later(old);
            }
            let now = Instant::now();
            e.own_key = e.key.is_some();
            e.key_until = e.own_key.then(|| now + key_ttl);
            e.state = LeaseState::Active;
            e.last_use = now;
            fx.changed(self.id);
            Ok(e.own_key)
        })?;
        if own_key {
            table.spawn_key_timer(self.id);
        }
        Ok(())
    }
}

/// While held, the lease counts a running library call (`call_running`).
pub struct CallGuard(Lease);

impl Drop for CallGuard {
    fn drop(&mut self) {
        let id = self.0.id;
        self.0.table.with_j(|i, fx| {
            if let Some(e) = i.leases.get_mut(&id) {
                e.calls = e.calls.saturating_sub(1);
                fx.changed(id);
            }
        });
    }
}
