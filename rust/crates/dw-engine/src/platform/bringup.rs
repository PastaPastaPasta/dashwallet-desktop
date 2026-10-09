//! Engine-owned bring-up (DASHPAY §2.5, §3.2; ROADMAP E0-05). The runtime
//! state it works on, the status reads, the loops' cadence and the lock view
//! are in `runtime.rs`.
//!
//! [`NetworkSession::start_spv`] returns at once. One engine task per start,
//! the *supervisor*, then:
//!
//! 1. brings each wallet's DashPay state up with platform-wallet's
//!    `start_wallet_subsystems` (identity, contact requests, contact
//!    accounts), every wallet at once and each within its budget, so the
//!    first filter scan already watches every contact's DIP-15 addresses;
//! 2. starts SPV whatever the outcome, then the sync loops;
//! 3. until SPV stops, serves the vault's unlocks (the bring-up again where
//!    it left the identity unsettled, the contact-crypto drain,
//!    `reconcile_dashpay_rescan`) and the wallets added while SPV runs.
//!
//! `stop_spv` and close cancel the supervisor and wait for it. A lock drops a
//! running bring-up: the scan key it may have resolved is beyond the vault's
//! lock (`signers.rs`). Removing or closing a wallet ends its bring-up.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use dpp::platform_value::string_encoding::Encoding;
use dw_vault::{Credential, GrantKind, GrantPurpose};
use platform_wallet::PlatformWalletError;
use platform_wallet::manager::startup::{
    DEFAULT_STARTUP_BUDGET, ScanKeyError, WalletStartupOptions, WalletStartupOutcome,
};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use super::runtime::{PlatformSignal, Stamped, Supervisor, guard, prompt_free};
use super::startup::StartupStatus;
use super::startup_status::{DashPayStartup, SpvState};
use super::{VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
use crate::events::unix_now;
use crate::session::Manager;
use crate::{EngineError, EngineEvent, NetworkSession, NoticeCode, WalletId};

/// Our budget for a wallet created here and never restored (DASHPAY §3.2):
/// its seed is new, so the only identities it can own were registered here
/// and are on file. Restored and imported wallets get the library's
/// `DEFAULT_STARTUP_BUDGET` (20 s).
pub const CREATED_HERE_BUDGET: Duration = Duration::from_secs(3);
/// How long past the budget the engine waits before it stops waiting: the
/// library bounds its awaits by the budget, not its synchronous work (the
/// vault's derivations, a few milliseconds each).
const BUDGET_SLACK: Duration = Duration::from_millis(500);

/// How often a start retries starting the loops while a pass of an earlier
/// start is still in flight.
const LOOP_RETRY: Duration = Duration::from_secs(30);
/// How long Platform's proof that a seed owns no identity is trusted
/// (E0-05 r1 ruling): after that, discovery runs again.
const NO_IDENTITY_TTL: Duration = Duration::from_secs(7 * 24 * 3600);

/// `settings_kv` keys in the wallet's local scope (`dw_appdb::local_scope`):
/// they describe this installation, so a `.dwbackup` does not carry them.
pub(crate) const CREATED_HERE_KEY: &str = "dashpay.created_here";
/// Platform proved that the seed owns no identity: later starts skip the
/// bring-up until the wallet has one on file (§3.2).
pub(crate) const NO_IDENTITY_KEY: &str = "dashpay.no_identity";

/// The keys of an unattended bring-up, built only while the vault is
/// prompt-free and dropped when it ends (§3.2 "Signers at startup").
struct BringUpKeys {
    crypto: VaultContactCrypto,
    scan: VaultScanKey,
}

/// What became of one wallet's `start_wallet_subsystems`.
enum Ended {
    Done(Result<WalletStartupOutcome, EngineError>),
    /// Past the budget and its slack.
    OverBudget,
    /// The vault locked while the bring-up held its keys.
    Locked,
}

#[derive(Debug, Clone, Copy)]
enum Job {
    BringUp,
    Unlock,
}

impl NetworkSession {
    /// Starts SPV (DASHPAY §2.5). Returns once the start is scheduled: an
    /// engine task brings the wallets' DashPay state up first and then
    /// starts SPV and the Platform sync loops; `spv_state` is `Starting`
    /// meanwhile, and `SpvStateChanged{running: true}` reports the start.
    /// A dash-spv failure at that point is `Notice{SpvError}`. Idempotent
    /// while starting or running. Configuration errors are returned here.
    pub async fn start_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let _lifecycle = this.platform.lifecycle.lock().await;
            this.spv_config()?;
            if this.platform_spv_state(&manager) != SpvState::Stopped {
                return Ok(());
            }
            // A supervisor left behind by SPV stopping underneath it. Its
            // loops are drained by the new start task, not here: a pass
            // still in flight would hold this call (review r1 M1).
            this.platform.deactivate(false).await;
            let since = Instant::now();
            // The slot first, so a wallet registered from here on is
            // signalled; one registered before is in the list below, admitted
            // after its signal's stamp, and the supervisor skips the signal.
            let (cancel, cancel_rx) = watch::channel(false);
            let (signals, signals_rx) = mpsc::unbounded_channel();
            *guard(&this.platform.supervisor) = Some(Supervisor {
                cancel,
                signals,
                task: None,
                running: false,
            });
            let listed = this.platform.stamp();
            let wallets: Vec<WalletId> = if this.platform.enabled {
                let ids = manager.wallet_ids().await.into_iter().map(WalletId);
                ids.filter(|id| {
                    this.platform
                        .admit(id, || this.vault.has_wallet_secret(&id.0))
                        == Some(true)
                })
                .collect()
            } else {
                Vec::new()
            };
            for id in &wallets {
                this.platform.set_status(*id, StartupStatus::Starting);
            }
            let task = tokio::spawn(Arc::clone(&this).supervise(
                manager,
                wallets,
                (since, listed),
                cancel_rx,
                signals_rx,
            ));
            let orphan = match guard(&this.platform.supervisor).as_mut() {
                Some(s) => {
                    s.task = Some(task);
                    None
                }
                // Close took the slot before the task was in it, and sent
                // the cancel: wait for the task here, while this call still
                // holds close at its gate. Not an abort: it may be inside
                // dash-spv's start.
                None => Some(task),
            };
            if let Some(task) = orphan {
                let _ = task.await;
            }
            Ok(())
        })
        .await
    }

    /// Stops SPV: cancels a bring-up still running and waits for its key
    /// work, quiesces the Platform loops, then stops dash-spv. Idempotent.
    /// SPV is stopped even when Platform work did not end in time; the error
    /// then names that work (`sdk`). It ends on its own, and the next start
    /// drains it again before its loops start.
    pub async fn stop_spv(self: &Arc<Self>) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let manager = this.manager()?;
            let _lifecycle = this.platform.lifecycle.lock().await;
            let drained = this.stop_platform(&manager).await;
            this.stop_spv_inner(&manager).await?;
            drained
        })
        .await
    }

    /// Cancels the supervisor (and with it a running bring-up), waits for its
    /// key work and quiesces the loops, before SPV stops (review r1 M1, M2).
    /// Close cancels only and leaves the drain to the manager's shutdown,
    /// which seals it: draining twice would wait twice for a pass stuck on
    /// the network.
    async fn stop_platform(&self, manager: &Manager) -> Result<(), EngineError> {
        self.platform.deactivate(false).await;
        let keys_idle = self.platform.key_work.wait_idle().await;
        let mut stuck = undrained_loops(manager).await;
        if !keys_idle {
            stuck.push("a vault key read");
        }
        if stuck.is_empty() {
            Ok(())
        } else {
            Err(EngineError::Sdk(format!(
                "SPV stopped, but this Platform work did not end: {}",
                stuck.join(", ")
            )))
        }
    }

    async fn supervise(
        self: Arc<Self>,
        manager: Arc<Manager>,
        wallets: Vec<WalletId>,
        (since, listed): (Instant, u64),
        mut cancel: watch::Receiver<bool>,
        mut signals: mpsc::UnboundedReceiver<Stamped>,
    ) {
        // When each wallet's latest bring-up was admitted. A signal of an
        // event before that is skipped: the pass saw it. Signals arriving
        // while a pass runs make one follow-up pass (review r4 M4-R4).
        let mut admitted: HashMap<WalletId, u64> = wallets.iter().map(|id| (*id, listed)).collect();
        if !self
            .for_wallets(&manager, wallets, Job::BringUp, since, &mut cancel)
            .await
            || *cancel.borrow()
        {
            return;
        }
        if let Err(e) = self.start_spv_inner(&manager).await {
            guard(&self.platform.supervisor).take();
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code: NoticeCode::SpvError,
                detail: format!("SPV did not start: {e}"),
            });
            return;
        }
        if !self.platform.mark_running() || !self.platform.enabled {
            return;
        }
        self.apply_cadence(&manager);
        let mut loops_pending = !self.start_loops(&manager).await;

        // The session must not be kept open by a task only close ends.
        let session: Weak<Self> = Arc::downgrade(&self);
        drop(self);
        loop {
            let retry = async {
                if loops_pending {
                    tokio::time::sleep(LOOP_RETRY).await;
                } else {
                    std::future::pending::<()>().await;
                }
            };
            let signal = tokio::select! {
                biased;
                () = until(&mut cancel, |c| *c) => return,
                () = retry => {
                    let Some(this) = session.upgrade() else { return };
                    loops_pending = !this.start_loops(&manager).await;
                    continue;
                }
                signal = signals.recv() => match signal {
                    Some(signal) => signal,
                    None => return,
                },
            };
            let (at, signal) = signal;
            let Some(this) = session.upgrade() else {
                return;
            };
            let (wallets, job) = match signal {
                PlatformSignal::Unlocked => {
                    let ids = manager.wallet_ids().await.into_iter().map(WalletId);
                    (ids.collect(), Job::Unlock)
                }
                PlatformSignal::WalletAdded(id) | PlatformSignal::Readmit(id)
                    if admitted.get(&id).is_some_and(|&pass| at < pass) =>
                {
                    continue;
                }
                // Not after a rollback removed it.
                PlatformSignal::Readmit(id) if manager.get_wallet(&id.0).await.is_none() => {
                    continue;
                }
                PlatformSignal::WalletAdded(id) | PlatformSignal::Readmit(id) => {
                    admitted.insert(id, this.platform.stamp());
                    (vec![id], Job::BringUp)
                }
            };
            let done = this
                .for_wallets(&manager, wallets, job, Instant::now(), &mut cancel)
                .await;
            if matches!(job, Job::Unlock) {
                this.kick_dashpay_sync(&manager);
            }
            if !done {
                return;
            }
        }
    }

    /// Starts the loops once no pass of an earlier start is in flight: a pass
    /// that outlived the last stop keeps its loop's admission shut until a
    /// quiesce succeeds (the library's rule). `false` while one still runs;
    /// the supervisor tries again every [`LOOP_RETRY`].
    async fn start_loops(&self, manager: &Manager) -> bool {
        let stuck = undrained_loops(manager).await;
        if !stuck.is_empty() {
            tracing::warn!(
                ?stuck,
                "a Platform sync pass of an earlier start is still running; the loops start later"
            );
            return false;
        }
        let loops = (
            manager.identity_sync_arc(),
            manager.dashpay_sync_arc(),
            manager.dpns_sync_arc(),
        );
        // `start` reaps a previous loop thread for up to a second and needs
        // the runtime's context; the blocking pool has both.
        match tokio::task::spawn_blocking(move || {
            loops.0.start();
            loops.1.start();
            loops.2.start();
        })
        .await
        {
            Ok(()) => self.platform.loops_on.store(true, Ordering::Release),
            Err(e) => tracing::warn!(error = %e, "the Platform sync loops did not start"),
        }
        true
    }

    /// Runs `job` for every wallet at once; `since` starts the bring-up
    /// budgets. `false` when cancelled; the wallets' tasks have ended either
    /// way.
    async fn for_wallets(
        self: &Arc<Self>,
        manager: &Arc<Manager>,
        wallets: Vec<WalletId>,
        job: Job,
        since: Instant,
        cancel: &mut watch::Receiver<bool>,
    ) -> bool {
        let mut set = JoinSet::new();
        let mut owners = HashMap::new();
        for id in wallets {
            let (this, manager) = (Arc::clone(self), Arc::clone(manager));
            let task = set.spawn(async move {
                match job {
                    Job::BringUp => this.bring_up_wallet(&manager, id, since).await,
                    Job::Unlock => this.after_unlock(&manager, id).await,
                }
            });
            owners.insert(task.id(), id);
            guard(&self.platform.tasks).insert(id, task);
        }
        let done = loop {
            tokio::select! {
                biased;
                () = until(cancel, |c| *c) => {
                    set.shutdown().await;
                    break false;
                }
                next = set.join_next_with_id() => {
                    let (task, failed) = match next {
                        None => break true,
                        Some(Ok((task, ()))) => (task, None),
                        Some(Err(e)) => (e.id(), Some(e)),
                    };
                    let Some(id) = owners.remove(&task) else { continue };
                    self.platform.untrack(id, task);
                    if let Some(e) = failed
                        && e.is_panic()
                    {
                        tracing::warn!(wallet_id = %id, error = %e, "a wallet's Platform task panicked");
                        if let Some(entry) = self.platform.write_startup().get_mut(&id) {
                            entry.startup = StartupStatus::NotRun;
                        }
                    }
                }
            }
        };
        for (task, id) in owners {
            self.platform.untrack(id, task);
        }
        done
    }

    /// One wallet's bring-up (§3.2): skipped for a watch-only wallet and for
    /// one that Platform proved recently has no identity and has none on
    /// file. Its budget, counted from `since`, bounds all of it, the reads
    /// and the key acquisition included (review r1 M3).
    async fn bring_up_wallet(&self, manager: &Manager, id: WalletId, since: Instant) {
        match self
            .platform
            .admit(&id, || self.vault.has_wallet_secret(&id.0))
        {
            // A restore of it has not committed; it is readmitted when the
            // last one ends.
            None => return,
            Some(false) => {
                self.platform
                    .record(id, DashPayStartup::new(StartupStatus::NotRun, true));
                return;
            }
            Some(true) => {}
        }
        // The budget is known once the markers are read; the shorter one
        // bounds reading them.
        let prelude = async {
            (
                local_identity(manager, id).await,
                self.bring_up_markers(id).await,
            )
        };
        let Ok((identity, (created_here, no_identity))) =
            tokio::time::timeout_at((since + CREATED_HERE_BUDGET).into(), prelude).await
        else {
            let startup = DashPayStartup::new(StartupStatus::PartialNoIdentity, false);
            self.finish_bring_up(id, startup, since, CREATED_HERE_BUDGET)
                .await;
            return;
        };
        if identity.is_none() && no_identity {
            self.platform
                .record(id, DashPayStartup::new(StartupStatus::NotRun, false));
            return;
        }
        self.platform.set_status(id, StartupStatus::Starting);
        let budget = if created_here {
            CREATED_HERE_BUDGET
        } else {
            DEFAULT_STARTUP_BUDGET
        };
        let deadline = since + budget;
        let lock = *self.platform.lock.borrow();
        // Past the deadline the key work keeps running on the blocking pool,
        // counted, and what it builds is dropped there.
        let keys = tokio::time::timeout_at(deadline.into(), self.bring_up_keys(id)).await;
        #[cfg(test)]
        {
            let pause = guard(&self.platform.pause_after_keys).take();
            if let Some(pause) = pause {
                pause.reached.store(true, Ordering::SeqCst);
                pause.release.notified().await;
            }
        }
        let ended = match &keys {
            Err(_) => Ended::OverBudget,
            Ok(keys) => {
                let left = deadline.saturating_duration_since(Instant::now());
                self.run_subsystems(manager, id, left, keys.as_ref(), lock.locks)
                    .await
            }
        };
        // The vault refused the keys (as opposed to not answering in time).
        let refused = matches!(keys, Ok(None));
        drop(keys);
        // Removed or closed meanwhile: nothing to record.
        if manager.get_wallet(&id.0).await.is_none() {
            self.platform.write_startup().remove(&id);
            return;
        }

        let mut startup = DashPayStartup::new(StartupStatus::NotRun, false);
        startup.startup = match ended {
            Ended::Done(Ok(outcome)) => {
                startup.identity = outcome.identity_id.map(|i| i.to_string(Encoding::Base58));
                startup.contact_accounts_pending =
                    u32::try_from(outcome.contact_accounts_pending).unwrap_or(u32::MAX);
                startup.identity_scan_incomplete = outcome.identity_scan_incomplete;
                outcome.status.into()
            }
            Ended::Done(Err(e)) => {
                tracing::warn!(wallet_id = %id, error = %e, "DashPay bring-up refused");
                self.platform.write_startup().remove(&id);
                return;
            }
            // Cut off: what the run left on file is all there is to know.
            Ended::OverBudget | Ended::Locked => {
                startup.identity = local_identity(manager, id).await;
                match (ended, &startup.identity) {
                    (Ended::Locked, _) => StartupStatus::IdentityUnsettled,
                    (_, Some(_)) => StartupStatus::PartialAccountsPending,
                    (_, None) => StartupStatus::PartialNoIdentity,
                }
            }
        };
        // A lock since the keys were sampled, or keys the vault refused,
        // leaves anything short of a settled answer for the first unlock to
        // run again: the library may have answered from a key the lock had
        // already ended (review r1 M5).
        let locked_since = self.platform.lock.borrow().locks != lock.locks;
        let settled = matches!(
            startup.startup,
            StartupStatus::Ready | StartupStatus::NoIdentity
        );
        if !settled && (refused || locked_since) {
            startup.startup = if refused && prompt_free(lock.state) && !locked_since {
                // The vault served no key although it needs no prompt:
                // nothing an unlock would change.
                StartupStatus::DiscoveryFailed
            } else {
                StartupStatus::IdentityUnsettled
            };
        }
        self.finish_bring_up(id, startup, since, budget).await;
    }

    /// Records a bring-up's outcome: the proven-absence marker, the notice
    /// and the status.
    async fn finish_bring_up(
        &self,
        id: WalletId,
        mut startup: DashPayStartup,
        since: Instant,
        budget: Duration,
    ) {
        startup.finished_at = Some(unix_now());
        tracing::info!(
            wallet_id = %id,
            status = startup.startup.as_str(),
            elapsed_ms = since.elapsed().as_millis() as u64,
            budget_ms = budget.as_millis() as u64,
            "DashPay bring-up finished"
        );
        if startup.startup == StartupStatus::NoIdentity {
            self.set_bring_up_marker(id, NO_IDENTITY_KEY).await;
        }
        if startup.startup.is_incomplete() {
            self.notice_incomplete(id, startup.startup);
        }
        self.platform.record(id, startup);
    }

    /// `Notice{DashPayStartupIncomplete}` (DASHPAY §3.7).
    fn notice_incomplete(&self, id: WalletId, status: StartupStatus) {
        self.sink.emit(EngineEvent::Notice {
            network: Some(self.network.clone()),
            code: NoticeCode::DashPayStartupIncomplete,
            detail: format!("wallet {id}: {}", status.as_str()),
        });
    }

    /// `start_wallet_subsystems` within `budget`, dropped if the vault locks
    /// (its lock count passes `locks`) while it holds `keys`. Without keys
    /// the scan key is unavailable, which the library reports as retryable,
    /// not as a local failure.
    async fn run_subsystems(
        &self,
        manager: &Manager,
        id: WalletId,
        budget: Duration,
        keys: Option<&BringUpKeys>,
        locks: u64,
    ) -> Ended {
        let resolve = || match keys {
            Some(keys) => keys.scan.resolve(),
            None => Err(ScanKeyError::Unavailable("the vault is locked".into())),
        };
        let call = manager.start_wallet_subsystems(
            &id.0,
            Some(&resolve),
            keys.map(|k| &k.crypto),
            // No identity signer: the DIP-15 auto-accept pass submits state
            // transitions, which an unattended bring-up never does (§2.6).
            None::<&VaultIdentitySigner>,
            WalletStartupOptions {
                budget,
                gap_limit: None,
            },
        );
        let mut lock = self.platform.lock.subscribe();
        let locked = async {
            match keys {
                Some(_) => until(&mut lock, |v| v.locks != locks).await,
                None => std::future::pending().await,
            }
        };
        // The lock first: a key the lock ended can make the call answer at
        // once, and both are then ready (review r1 M5).
        tokio::select! {
            biased;
            () = locked => Ended::Locked,
            ended = tokio::time::timeout(budget + BUDGET_SLACK, call) => match ended {
                Ok(result) => Ended::Done(result.map_err(EngineError::from)),
                Err(_) => Ended::OverBudget,
            },
        }
    }

    /// The bring-up's providers, or `None` unless the vault is prompt-free.
    async fn bring_up_keys(&self, id: WalletId) -> Option<BringUpKeys> {
        self.platform
            .key_work
            .run(self.vault.clone(), move |vault| {
                Ok(BringUpKeys {
                    crypto: background_contact_crypto(vault, id)?,
                    scan: unattended_scan_key(vault, id)?,
                })
            })
            .await
    }

    /// The unlock drain for one wallet (§3.2): the bring-up again if it left
    /// the identity unsettled, otherwise the seed-gated contact-crypto drain;
    /// then `reconcile_dashpay_rescan`, so payments to contacts whose
    /// accounts appear only now are found by a rescan.
    async fn after_unlock(&self, manager: &Manager, id: WalletId) {
        // No seed, a restore not committed yet, or a signal from an unlock
        // the vault has since locked again.
        let admitted = self
            .platform
            .admit(&id, || self.vault.has_wallet_secret(&id.0));
        if admitted != Some(true) || !prompt_free(self.platform.lock.borrow().state) {
            return;
        }
        let unsettled = self
            .platform
            .startup_of(&id)
            .is_some_and(|s| s.startup == StartupStatus::IdentityUnsettled);
        if unsettled {
            self.bring_up_wallet(manager, id, Instant::now()).await;
        } else {
            self.drain_contact_crypto(manager, id).await;
        }
        let Some(wallet) = manager.get_wallet(&id.0).await else {
            return;
        };
        match wallet.identity().dashpay().reconcile_dashpay_rescan().await {
            Ok(Some(floor)) => {
                tracing::info!(wallet_id = %id, floor, "DIP-15 rescan scheduled after unlock");
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(wallet_id = %id, error = %e, "DIP-15 rescan reconcile failed"),
        }
    }

    async fn drain_contact_crypto(&self, manager: &Manager, id: WalletId) {
        let Some(wallet) = manager.get_wallet(&id.0).await else {
            return;
        };
        let locks = self.platform.lock.borrow().locks;
        let Some(crypto) = self
            .platform
            .key_work
            .run(self.vault.clone(), move |vault| {
                background_contact_crypto(vault, id)
            })
            .await
        else {
            return;
        };
        let deadline = Instant::now() + DEFAULT_STARTUP_BUDGET;
        let mut lock = self.platform.lock.subscribe();
        tokio::select! {
            drained = wallet.drain_pending_contact_crypto_verified(
                &crypto,
                None::<&VaultIdentitySigner>,
                Some(deadline),
            ) => match drained {
                Ok(_) => {}
                Err(e @ PlatformWalletError::SeedMismatch { .. }) => {
                    tracing::error!(wallet_id = %id, error = %e, "the vault's seed does not bind this wallet");
                    self.platform.set_status(id, StartupStatus::SeedBindingUnverified);
                    self.notice_incomplete(id, StartupStatus::SeedBindingUnverified);
                }
                // Unanswered in time, or the vault locked: still queued for
                // the next drain.
                Err(e) => tracing::warn!(wallet_id = %id, error = %e, "contact-crypto drain refused"),
            },
            () = until(&mut lock, |v| v.locks != locks) => return,
        }
        let pending = wallet
            .identity()
            .dashpay()
            .pending_contact_crypto_count()
            .await;
        if let Some(entry) = self.platform.write_startup().get_mut(&id) {
            entry.contact_accounts_pending = u32::try_from(pending).unwrap_or(u32::MAX);
        }
    }

    /// (created here, Platform proved no identity within
    /// [`NO_IDENTITY_TTL`]) from the wallet's local settings. A read error
    /// counts as neither: the longer budget, and a bring-up that runs.
    async fn bring_up_markers(&self, id: WalletId) -> (bool, bool) {
        let scope = dw_appdb::local_scope(&id.to_string());
        let now = unix_now();
        let read = self
            .appdb_op(move |db| {
                let proven_at = db.setting(&scope, NO_IDENTITY_KEY)?;
                Ok((
                    db.setting(&scope, CREATED_HERE_KEY)?.is_some(),
                    proven_at
                        .and_then(|at| at.parse::<u64>().ok())
                        .is_some_and(|at| now.saturating_sub(at) < NO_IDENTITY_TTL.as_secs()),
                ))
            })
            .await;
        read.unwrap_or_else(|e| {
            tracing::warn!(wallet_id = %id, error = %e, "could not read the bring-up markers");
            (false, false)
        })
    }

    async fn set_bring_up_marker(&self, id: WalletId, key: &'static str) {
        let scope = dw_appdb::local_scope(&id.to_string());
        let now = unix_now().to_string();
        if let Err(e) = self
            .appdb_op(move |db| db.set_setting(&scope, key, Some(&now)))
            .await
        {
            tracing::warn!(wallet_id = %id, key, error = %e, "could not store a bring-up marker");
        }
    }

    /// Records that `id` was created here: its bring-up gets
    /// [`CREATED_HERE_BUDGET`]. Not carried by a `.dwbackup`, so a restore
    /// elsewhere gets the full budget.
    pub(crate) async fn mark_created_here(&self, id: WalletId) {
        self.set_bring_up_marker(id, CREATED_HERE_KEY).await;
    }

    /// Forgets that Platform proved `id` owns no identity, so the next
    /// bring-up runs discovery again: DP6-01's "find" (E0-05 r1 ruling).
    #[cfg_attr(not(test), expect(dead_code, reason = "DP6-01's find calls it"))]
    pub(crate) async fn forget_proven_absence(&self, id: WalletId) -> Result<(), EngineError> {
        let scope = dw_appdb::local_scope(&id.to_string());
        self.appdb_op(move |db| db.set_setting(&scope, NO_IDENTITY_KEY, None))
            .await
    }
}

/// Quiesces the three loops; the names of those whose pass did not drain
/// within the library's budget (review r1 M1: the library asks its caller to
/// fail closed then).
async fn undrained_loops(manager: &Manager) -> Vec<&'static str> {
    let (identity, dashpay, dpns) = tokio::join!(
        manager.identity_sync().quiesce(),
        manager.dashpay_sync().quiesce(),
        manager.dpns_sync().quiesce(),
    );
    [
        ("identity_sync", identity),
        ("dashpay_sync", dashpay),
        ("dpns_sync", dpns),
    ]
    .into_iter()
    .filter_map(|(name, drained)| (!drained).then_some(name))
    .collect()
}

/// The DashPay crypto provider of unattended work: the bring-up and the
/// unlock drain. Built from the vault's prompt-free signer, so it exists only
/// while the vault is unlocked with scope Full or unencrypted. E0-04 replaces
/// the body with the background lease's signer (E0-04 design §4.5); the
/// callers stay as they are.
fn background_contact_crypto(
    vault: &dw_vault::Vault,
    id: WalletId,
) -> Result<VaultContactCrypto, EngineError> {
    VaultContactCrypto::new(vault.dashpay_crypto_signer(&id.0)?)
}

/// The identity-scan key of an unattended bring-up, under a grant the engine
/// authorizes itself; that works only while the vault is prompt-free
/// (dw-vault `Vault::scan_key`). E0-04 moves it to its `IdentityScan`
/// purpose (E0-04 design §3.2).
fn unattended_scan_key(vault: &dw_vault::Vault, id: WalletId) -> Result<VaultScanKey, EngineError> {
    let grant = vault.authorize(GrantPurpose::PlatformOp, Some(&id.0), Credential::None)?;
    let token = vault.redeem_grant(&grant.id, GrantKind::PlatformOp, Some(&id.0))?;
    Ok(VaultScanKey::new(vault.scan_key(&id.0, &token)?))
}

/// Waits until `rx`'s value satisfies `f`, or its sender is gone. The guard
/// `wait_for` returns stays in here: it must not live across an await.
pub(super) async fn until<T>(rx: &mut watch::Receiver<T>, f: impl FnMut(&T) -> bool) {
    let _ = rx.wait_for(f).await;
}

/// Base58 id of the first identity the wallet has on file.
async fn local_identity(manager: &Manager, id: WalletId) -> Option<String> {
    let wm = manager.wallet_manager_arc();
    let wm = wm.read().await;
    let info = wm.get_wallet_info(&id.0)?;
    info.identity_manager
        .wallet_identity_ids(&id.0)
        .into_iter()
        .next()
        .map(|identity| identity.to_string(Encoding::Base58))
}
