//! Lock, close and the other revocations (E0-04 §8).
//!
//! A revocation is a [`Freeze`]: one J step, before the caller's first
//! await, that revokes the leases in scope, drops their keys and the
//! background leases, snapshots the permits in flight, bumps the generation
//! and sets the barrier (H4). Then the request runs its own vault gate (a
//! lock's `vault.lock()`, a passphrase change's own call; H14) and drains
//! its snapshot. Concurrent requests share only the drain: a later
//! request's snapshot is the subset of the earlier one still in flight. The
//! barrier clears when every pending gate and drain is done.

use std::sync::Arc;

use dw_vault::VaultStatus;
use tokio::time::Instant;

use super::table::LeaseTable;
use super::{FlowOutcome, FlowReport, LeaseId, LockPhase, LockReport, lease_string};
use crate::platform::flows::RevokeCause;
use crate::{EngineEvent, WalletId};

/// Which leases a revocation ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    All,
    /// `remove_wallet` and "Close Wallet": one wallet's leases and permits.
    Wallet(WalletId),
}

/// One revocation in progress: its snapshot and what it still holds of the
/// barrier. Dropping it releases what it holds, so a revocation abandoned
/// half-way (a runtime shutting down) cannot wedge the barrier.
pub(crate) struct Freeze {
    table: Arc<LeaseTable>,
    scope: Scope,
    /// The permits in flight at the freeze, with their deadlines.
    snapshot: Vec<(u64, Instant)>,
    /// Leases to report: revoked by this freeze, or holding a permit of
    /// the snapshot.
    leases: Vec<LeaseId>,
    #[cfg_attr(not(test), expect(dead_code, reason = "the stress log names locks"))]
    lock: u64,
    gate_pending: bool,
    drain_pending: bool,
}

impl LeaseTable {
    /// The freeze (§8.1 step 1). `gate`: the request runs a vault gate the
    /// barrier waits for.
    pub(crate) fn freeze(self: &Arc<Self>, cause: RevokeCause, scope: Scope, gate: bool) -> Freeze {
        let (snapshot, leases, lock) = self.with_j(|i, fx| {
            #[cfg(test)]
            let revoke_now = i.mutation != Some(super::stress_tests::Mutation::RevokeInDrain);
            #[cfg(not(test))]
            let revoke_now = true;
            let in_scope = |w: &WalletId| match scope {
                Scope::All => true,
                Scope::Wallet(only) => *w == only,
            };
            let mut leases = Vec::new();
            for (id, e) in i.leases.iter_mut().filter(|(_, e)| in_scope(&e.wallet)) {
                if revoke_now && e.revoke(cause, fx) {
                    leases.push(*id);
                    fx.changed(*id);
                }
            }
            let dropped: Vec<WalletId> = i
                .background
                .keys()
                .filter(|w| in_scope(w))
                .copied()
                .collect();
            for w in dropped {
                if let Some(bg) = i.background.remove(&w) {
                    fx.drop_later(bg);
                }
            }
            let snapshot: Vec<(u64, Instant, Option<LeaseId>)> = i
                .fence
                .permits()
                .filter(|(_, _, lease)| {
                    lease.is_some_and(|l| i.leases.get(&l).is_some_and(|e| in_scope(&e.wallet)))
                })
                .collect();
            for (_, _, lease) in &snapshot {
                if let Some(l) = lease
                    && !leases.contains(l)
                {
                    leases.push(*l);
                }
            }
            if cause == RevokeCause::Close {
                i.closed = true;
            }
            match scope {
                Scope::All => {
                    i.lock_gen += 1;
                    i.barrier.drains += 1;
                }
                Scope::Wallet(w) => {
                    *i.wallet_gen.entry(w).or_default() += 1;
                    *i.barrier.wallet_drains.entry(w).or_default() += 1;
                }
            }
            if gate {
                i.barrier.gates += 1;
            }
            fx.notify();
            let lock = i.lock_gen;
            #[cfg(test)]
            i.note(super::stress_tests::LogEvent::Freeze {
                lock,
                at: Instant::now(),
                revoked: leases.clone(),
                in_flight: snapshot.iter().map(|(id, d, _)| (*id, *d)).collect(),
            });
            let snapshot = snapshot.into_iter().map(|(id, d, _)| (id, d)).collect();
            (snapshot, leases, lock)
        });
        Freeze {
            table: Arc::clone(self),
            scope,
            snapshot,
            leases,
            lock,
            gate_pending: gate,
            drain_pending: true,
        }
    }

    /// Waits while any lock gate is pending (H14): an unlock issued after a
    /// lock's call is ordered after that lock's gate.
    pub(crate) async fn wait_gates(&self) {
        self.wait_until(|i| (i.barrier.gates == 0).then_some(()))
            .await;
    }

    /// The async lock (§8.1): freeze now, then a spawned coordinator runs
    /// `gate` on the blocking pool and the drain, so a dropped caller
    /// cannot leave the barrier set. Returns once both are done.
    pub(crate) async fn lock(
        self: &Arc<Self>,
        cause: RevokeCause,
        gate: impl FnOnce() -> VaultStatus + Send + 'static,
    ) -> LockReport {
        let mut freeze = self.freeze(cause, Scope::All, true);
        let table = Arc::clone(self);
        let coordinator = self.rt.spawn(async move {
            let status = table.rt.spawn_blocking(gate).await;
            freeze.gate_done();
            freeze.drain().await;
            freeze.finish(status.unwrap_or_else(|_| gate_panicked()))
        });
        coordinator.await.unwrap_or_else(|_| LockReport {
            status: gate_panicked(),
            flows: Vec::new(),
        })
    }

    /// The synchronous lock (§8.1, `Vault.lock()`): freeze, `gate` inline as
    /// this request's own gate, then a spawned drain; never block-waits.
    /// The drain's end arrives as `LockProgress::Done`.
    pub(crate) fn lock_sync(
        self: &Arc<Self>,
        cause: RevokeCause,
        gate: impl FnOnce() -> VaultStatus,
    ) -> VaultStatus {
        let mut freeze = self.freeze(cause, Scope::All, true);
        let status = gate();
        freeze.gate_done();
        let report_status = status.clone();
        self.rt.spawn(async move {
            freeze.drain().await;
            freeze.finish(report_status);
        });
        status
    }

    /// Aborts every flow task registered with a lease (close, §8.5).
    pub(crate) fn abort_tasks(&self) {
        self.with_j(|i, _| {
            for e in i.leases.values_mut() {
                for t in e.tasks.drain(..) {
                    t.abort();
                }
            }
        });
    }
}

impl Freeze {
    /// This request's vault gate has run.
    pub(crate) fn gate_done(&mut self) {
        if std::mem::take(&mut self.gate_pending) {
            self.table.with_j(|i, fx| {
                i.barrier.gates = i.barrier.gates.saturating_sub(1);
                fx.notify();
            });
        }
    }

    /// The drain (§8.1 step 3): waits until every permit of the snapshot
    /// has ended or passed its deadline (H3), without polling; emits
    /// `LockProgress::Draining` when the count changes. Ends at the latest
    /// deadline, at most H after the freeze, and releases the barrier.
    pub(crate) async fn drain(&mut self) {
        self.drained().await;
        self.release_drain();
    }

    /// [`Self::drain`] keeping the barrier set until this is dropped: a
    /// wallet's removal holds it through the whole removal (DEC-134).
    pub(crate) async fn drained(&mut self) {
        let table = Arc::clone(&self.table);
        let mut shown = None;
        loop {
            let notified = table.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let now = Instant::now();
            let open: Vec<Instant> = table.with_j(|i, _| {
                self.snapshot
                    .iter()
                    .filter(|(id, deadline)| now < *deadline && i.fence.has_attempt(*id))
                    .map(|(_, d)| *d)
                    .collect()
            });
            let Some(last) = open.iter().max().copied() else {
                break;
            };
            let in_flight = open.len() as u32;
            if shown != Some(in_flight) && self.scope == Scope::All {
                shown = Some(in_flight);
                table.emit(EngineEvent::LockProgress {
                    network: table.network.clone(),
                    phase: LockPhase::Draining {
                        in_flight,
                        deadline_in_ms: last.saturating_duration_since(now).as_millis() as u64,
                    },
                });
            }
            let next = open.iter().min().copied().unwrap_or(last);
            tokio::select! {
                _ = &mut notified => {}
                _ = tokio::time::sleep_until(next) => {}
            }
        }
    }

    fn release_drain(&mut self) {
        if !std::mem::take(&mut self.drain_pending) {
            return;
        }
        let scope = self.scope;
        #[cfg(test)]
        let lock = self.lock;
        self.table.with_j(|i, fx| {
            #[cfg(test)]
            if i.mutation == Some(super::stress_tests::Mutation::RevokeInDrain) {
                // The mutation: revocation only once the drain is over.
                for (id, e) in i.leases.iter_mut() {
                    if e.revoke(RevokeCause::Lock, fx) {
                        fx.changed(*id);
                    }
                }
            }
            match scope {
                Scope::All => i.barrier.drains = i.barrier.drains.saturating_sub(1),
                Scope::Wallet(w) => {
                    if let Some(n) = i.barrier.wallet_drains.get_mut(&w) {
                        *n = n.saturating_sub(1);
                        if *n == 0 {
                            i.barrier.wallet_drains.remove(&w);
                        }
                    }
                }
            }
            #[cfg(test)]
            i.note(super::stress_tests::LogEvent::LockReturned {
                lock,
                at: Instant::now(),
            });
            fx.notify();
        });
    }

    /// The report (§8.4), from each lease's history, never from what was
    /// in flight; sent as `LockProgress::Done` for a lock.
    pub(crate) fn finish(mut self, status: VaultStatus) -> LockReport {
        self.gate_done();
        self.release_drain();
        let flows = self.table.with_j(|i, _| {
            self.leases
                .iter()
                .filter_map(|id| {
                    let e = i.leases.get(id)?;
                    let artifacts: Vec<(String, FlowOutcome)> = e
                        .history
                        .iter()
                        .map(|a| (a.to_string(), i.fence.outcome(&e.wallet, a)))
                        .collect();
                    let outcome = artifacts
                        .iter()
                        .map(|(_, o)| *o)
                        .max()
                        .unwrap_or(FlowOutcome::Cancelled);
                    Some(FlowReport {
                        lease: lease_string(id),
                        wallet_id: e.wallet.to_string(),
                        flow: e.flow,
                        outcome,
                        artifacts,
                    })
                })
                .collect()
        });
        let report = LockReport { status, flows };
        if self.scope == Scope::All {
            self.table.emit(EngineEvent::LockProgress {
                network: self.table.network.clone(),
                phase: LockPhase::Done(report.clone()),
            });
        }
        report
    }
}

impl Drop for Freeze {
    fn drop(&mut self) {
        self.gate_done();
        self.release_drain();
    }
}

/// The status reported when a gate panicked: the vault may hold its key,
/// so nothing about it is claimed but "locked" from the lock's side.
fn gate_panicked() -> VaultStatus {
    VaultStatus {
        state: dw_vault::LockState::Locked,
        encrypted: true,
        quick_unlock_enrolled: false,
        failed_attempts: 0,
        retry_after_secs: None,
        wallets_with_secrets: Vec::new(),
    }
}
