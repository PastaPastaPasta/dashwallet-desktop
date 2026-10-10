//! The catch-up's resume pass (E0-04 H6 part 2, §2a.1; Mode B's pin
//! semantics, §2a.3).
//!
//! **What it does.** For one wallet, it reads the tracked asset-lock rows
//! ([`TrackedLocks::rows`]) and resumes every `Built` or `Broadcast` one
//! through the library's public `resume_asset_lock` ([`Resumer::resume`]).
//! The rows resume concurrently, each in its own task, so one slow proof
//! wait does not hold the others. Once a row's attempt has returned,
//! whatever it returned, the row stops blocking the wallet's funding calls
//! ([`LeaseTable::note_resumed`], §2a.3's funding gate); a row the library
//! could not resume stays `Built`/`Broadcast` and is resumed again by the
//! next pass. An attempt that could not start (the wallet is not loaded, the
//! session is gone, the task panicked) is not noted: its row keeps
//! blocking until a later pass runs it. After the rows, a funding refresh
//! is requested, so the statuses of §2a.5 move.
//!
//! **Pin semantics** (§2a.3). Mode B has no fence in the library, so the
//! resume may resend a `Built` row signed before a lock (r4 M1): allowed
//! under Mode B's weaker promise. The pass runs in tasks it spawns, outside
//! any `DispatchScope` (a task-local, not inherited), so the hand-offs are
//! unscoped; at the pin the library never calls the fence's `admit`, so
//! H7's notice for unscoped hand-offs has nothing to see.
//!
//! **When it runs** (H6, §6.4, §8.6). Nothing runs before SPV has started:
//! [`LeaseTable::spv_started`] (E0-05's bring-up, through
//! `start_spv_inner`) arms the passes and runs one for every loaded
//! wallet; a stop disarms them until the next start, which runs them all
//! again. After that a pass runs again for every loaded wallet on each SPV
//! peers 0 → >0 transition ([`LeaseTable::request_resume`] from the hub)
//! and after the dispatch journal recovers from a write failure (the first
//! write that succeeds after one failed, `Effects::resume`), and for one
//! wallet when it is opened again ([`LeaseTable::wallet_opened`]).
//!
//! **One pass at a time per wallet.** A trigger while the wallet's pass
//! runs schedules exactly one more, which starts when it ends (it reads the
//! rows again). The coordinator lives in J; J is never held across the
//! library call or any await.
//!
//! **Ending.** Close's `abort_tasks` aborts every pass (its row tasks with
//! it), and no pass starts once close froze the table. Closing a wallet
//! ([`LeaseTable::wallet_closed`]) or removing it ends its pass and forgets
//! which of its rows were resumed: they block its funding again until the
//! pass that runs when it is opened again. A pass that was ended notes
//! nothing more.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use dashcore::Txid;
use tokio::task::{AbortHandle, JoinSet};

use super::table::LeaseTable;
use crate::WalletId;

/// What one row's resume did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumed {
    /// The library made its attempt (whatever it returned), or the row is
    /// no longer tracked: it stops blocking the wallet's funding calls.
    Ran,
    /// The attempt could not start: the row keeps blocking until a later
    /// pass runs it.
    NotRun,
}

/// How the pass reaches the library. The real one is
/// [`super::mode_b::SessionTracked`]; tests use a fake.
#[async_trait]
pub(crate) trait Resumer: Send + Sync + 'static {
    /// The loaded wallets.
    async fn wallets(&self) -> Vec<WalletId>;
    /// Resumes tracked row `txid` of `wallet` (`resume_asset_lock`, with
    /// the library's own bounds).
    async fn resume(&self, wallet: WalletId, txid: Txid) -> Resumed;
}

/// One wallet's running pass.
#[derive(Debug)]
struct Pass {
    id: u64,
    /// A trigger arrived while it ran: one more pass follows.
    again: bool,
    task: Option<AbortHandle>,
}

/// The passes' coordinator, in J.
#[derive(Debug, Default)]
pub(crate) struct Passes {
    /// SPV has started this session: passes run.
    armed: bool,
    next: u64,
    running: HashMap<WalletId, Pass>,
}

impl Passes {
    /// Whether pass `id` of `wallet` is still the wallet's pass.
    pub(super) fn current(&self, wallet: &WalletId, id: u64) -> bool {
        self.running.get(wallet).is_some_and(|p| p.id == id)
    }

    /// Ends `wallet`'s pass (a close or removal of the wallet).
    pub(super) fn end(&mut self, wallet: &WalletId) {
        if let Some(task) = self.running.remove(wallet).and_then(|p| p.task) {
            task.abort();
        }
    }

    /// Ends every pass (close).
    pub(super) fn abort_all(&mut self) {
        for (_, p) in self.running.drain() {
            if let Some(task) = p.task {
                task.abort();
            }
        }
    }
}

impl LeaseTable {
    /// Installs the session's resume path (once).
    pub(crate) fn install_resumer(&self, resumer: Arc<dyn Resumer>) {
        let _ = self.resumer.set(resumer);
    }

    /// SPV has started (E0-05's bring-up, a peer rotation): arms the passes
    /// and runs one for every loaded wallet (H6 part 2).
    pub(crate) fn spv_started(self: &Arc<Self>) {
        self.with_j(|i, _| i.mode_b.passes.armed = true);
        self.request_resume(None);
    }

    /// SPV has stopped: no pass starts until it starts again, which runs
    /// one for every loaded wallet. A pass running now finishes.
    pub(crate) fn spv_stopped(&self) {
        self.with_j(|i, _| i.mode_b.passes.armed = false);
    }

    /// Runs a pass for `wallet`, or for every loaded wallet (`None`): an
    /// SPV peers 0 → >0 transition, the journal's recovery. Nothing before
    /// SPV has started, or once close froze the table.
    pub(crate) fn request_resume(self: &Arc<Self>, wallet: Option<WalletId>) {
        let Some(resumer) = self.resumer.get().cloned() else {
            return;
        };
        if let Some(wallet) = wallet {
            self.start_pass(wallet);
            return;
        }
        let table = Arc::clone(self);
        self.rt.spawn(async move {
            for wallet in resumer.wallets().await {
                table.start_pass(wallet);
            }
        });
    }

    /// A closed wallet was opened again (§8.6): its funding statuses are
    /// read again and its rows resumed.
    pub(crate) fn wallet_opened(self: &Arc<Self>, wallet: WalletId) {
        self.request_refresh(wallet);
        self.request_resume(Some(wallet));
    }

    /// A wallet was closed (§8.6): its pass ends, and its rows block its
    /// funding again until the pass that runs when it is opened.
    pub(crate) fn wallet_closed(&self, wallet: WalletId) {
        self.with_j(|i, fx| {
            i.mode_b.wallet_closed(&wallet);
            fx.notify();
        });
    }

    /// Starts `wallet`'s pass, or asks the running one for one more.
    fn start_pass(self: &Arc<Self>, wallet: WalletId) {
        let id = self.with_j(|i, _| {
            if i.closed || !i.mode_b.passes.armed {
                return None;
            }
            let passes = &mut i.mode_b.passes;
            if let Some(p) = passes.running.get_mut(&wallet) {
                p.again = true;
                return None;
            }
            let id = passes.next;
            passes.next += 1;
            passes.running.insert(
                wallet,
                Pass {
                    id,
                    again: false,
                    task: None,
                },
            );
            Some(id)
        });
        let Some(id) = id else {
            return;
        };
        let table = Arc::clone(self);
        let task = self
            .rt
            .spawn(async move { table.run_pass(wallet, id).await })
            .abort_handle();
        // Registered so close aborts it. Aborted here when close ran since
        // the J step above (its `abort_tasks` already ran), or when the
        // pass was ended meanwhile (the wallet closed or removed: its
        // `end` found no handle); a pass that already finished is not hurt.
        self.with_j(|i, _| match i.mode_b.passes.running.get_mut(&wallet) {
            Some(p) if p.id == id && !i.closed => p.task = Some(task),
            _ => task.abort(),
        });
    }

    async fn run_pass(self: Arc<Self>, wallet: WalletId, id: u64) {
        // A pass that panics ends here, so the wallet's next trigger
        // starts a pass again.
        struct Unwind<'a>(&'a LeaseTable, WalletId, u64);
        impl Drop for Unwind<'_> {
            fn drop(&mut self) {
                if std::thread::panicking() {
                    self.0.with_j(|i, _| {
                        if i.mode_b.passes.current(&self.1, self.2) {
                            i.mode_b.passes.running.remove(&self.1);
                        }
                    });
                }
            }
        }
        let _unwind = Unwind(&self, wallet, id);
        let source = self.tracked.get().cloned().zip(self.resumer.get().cloned());
        loop {
            let mut resumes = JoinSet::new();
            // `None`: the wallet is not loaded, so it has no rows to resume.
            let mut loaded = false;
            if let Some((tracked, resumer)) = &source
                && let Some(rows) = tracked.rows(wallet).await
            {
                loaded = true;
                for row in rows.into_iter().filter(|r| r.status.pending()) {
                    let resumer = Arc::clone(resumer);
                    resumes
                        .spawn(async move { (row.txid, resumer.resume(wallet, row.txid).await) });
                }
            }
            while let Some(done) = resumes.join_next().await {
                match done {
                    Ok((txid, Resumed::Ran)) => {
                        if !self.note_resumed_by(wallet, txid, id) {
                            // Ended (wallet closed or removed): nothing more.
                            return;
                        }
                    }
                    Ok((txid, Resumed::NotRun)) => {
                        tracing::debug!(wallet_id = %wallet, %txid, "the catch-up could not resume a tracked asset lock");
                    }
                    Err(e) => {
                        tracing::warn!(wallet_id = %wallet, error = %e, "a tracked asset lock's resume did not return");
                    }
                }
            }
            if loaded {
                self.request_refresh(wallet);
            }
            let again = self.with_j(|i, _| {
                let passes = &mut i.mode_b.passes;
                match passes.running.get_mut(&wallet) {
                    Some(p) if p.id == id && p.again && !i.closed => {
                        p.again = false;
                        true
                    }
                    Some(p) if p.id == id => {
                        passes.running.remove(&wallet);
                        false
                    }
                    _ => false,
                }
            });
            if !again {
                return;
            }
        }
    }

    /// Pass `id` resumed row `txid`; `false` once the pass was ended.
    fn note_resumed_by(&self, wallet: WalletId, txid: Txid, id: u64) -> bool {
        self.with_j(|i, fx| {
            if !i.mode_b.passes.current(&wallet, id) {
                return false;
            }
            i.mode_b.note_resumed(wallet, txid);
            fx.notify();
            true
        })
    }

    /// Whether a pass of `wallet` runs now (tests).
    #[cfg(test)]
    pub(crate) fn pass_running(&self, wallet: WalletId) -> bool {
        self.with_j(|i, _| i.mode_b.passes.running.contains_key(&wallet))
    }
}
