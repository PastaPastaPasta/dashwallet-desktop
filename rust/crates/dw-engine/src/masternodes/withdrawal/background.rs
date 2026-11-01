//! The withdrawal runtime's background work (MG-13 P4, P8): the recheck
//! task, the quotes' `withdrawal_ready` waits, and every blocking app
//! database call (`on_db`), on the shape of the lease table's (BL-544,
//! desktop main `platform/lease/background.rs`; copied, not shared).
//!
//! A task close only aborts lets go of the session and its app database
//! when the runtime next drops its future, and an aborted task's blocking
//! write runs on to its end: either can outlive close. So each runs
//! tracked here, and close marks the work closed under the lock every start
//! takes (nothing starts after it), aborts the tasks, and returns only once
//! every tracked future and blocking call has been dropped.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

/// A tracked task: one recheck, one wait per masternode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Job {
    /// The background recheck (P8).
    Recheck,
    /// A masternode's `withdrawal_ready` wait (P4; wire order).
    Ready([u8; 32]),
}

#[derive(Debug, Default)]
pub(crate) struct Background {
    state: Mutex<State>,
    /// Woken when the last tracked future or call is dropped.
    idle: Notify,
}

#[derive(Debug, Default)]
struct State {
    closed: bool,
    /// Tokens not yet dropped: tracked futures and blocking calls.
    live: usize,
    /// The spawned tasks, for close to abort.
    jobs: HashMap<Job, AbortHandle>,
}

/// Held by a tracked future or a blocking call; dropped with it.
#[derive(Debug)]
pub(crate) struct Token(Arc<Background>);

impl Drop for Token {
    fn drop(&mut self) {
        let mut state = self.0.state();
        state.live -= 1;
        if state.live == 0 {
            self.0.idle.notify_waiters();
        }
    }
}

impl Background {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A token close waits for, taken even once closed: a call already
    /// running finishes before close returns.
    pub(crate) fn hold(self: &Arc<Self>) -> Token {
        self.state().live += 1;
        Token(Arc::clone(self))
    }

    /// Spawns `task` on `rt` as `job`, tracked until its future is dropped;
    /// `false` (nothing spawned) once closed or while `job` runs.
    pub(crate) fn spawn<F>(self: &Arc<Self>, rt: &Handle, job: Job, task: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let token = {
            let mut state = self.state();
            if state.closed || state.jobs.get(&job).is_some_and(|t| !t.is_finished()) {
                return false;
            }
            state.live += 1;
            Token(Arc::clone(self))
        };
        // Spawned outside the lock: a runtime shutting down drops the
        // future, and with it the token, at once.
        let handle = rt
            .spawn(async move {
                let _token = token;
                task.await
            })
            .abort_handle();
        let mut state = self.state();
        state.jobs.retain(|_, t| !t.is_finished());
        // Close ran meanwhile (it did not see this task), or a racing start
        // of the same job won.
        if state.closed || state.jobs.contains_key(&job) {
            handle.abort();
            return false;
        }
        state.jobs.insert(job, handle);
        true
    }

    /// Close: starts nothing after, aborts the tasks, and returns once every
    /// tracked future and blocking call has been dropped. A panicking one is
    /// dropped as it unwinds.
    pub(crate) async fn close(&self) {
        let jobs = {
            let mut state = self.state();
            state.closed = true;
            std::mem::take(&mut state.jobs)
        };
        for task in jobs.into_values() {
            task.abort();
        }
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            // Registered before the check, so a drop between the two wakes it.
            idle.as_mut().enable();
            if self.state().live == 0 {
                return;
            }
            idle.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn is_closed(&self) -> bool {
        self.state().closed
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// A value whose drop the test can see.
    fn probe() -> (Arc<()>, std::sync::Weak<()>) {
        let held = Arc::new(());
        let weak = Arc::downgrade(&held);
        (held, weak)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_returns_after_every_task_and_call_is_dropped() {
        let bg = Arc::new(Background::default());
        let rt = Handle::current();
        let (held, weak) = probe();
        let (started_tx, started) = tokio::sync::oneshot::channel();
        assert!(bg.spawn(&rt, Job::Recheck, async move {
            let _held = held;
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        }));
        started.await.unwrap();
        // A blocking call still running.
        let (release, released) = std::sync::mpsc::channel::<()>();
        let call = {
            let token = bg.hold();
            tokio::task::spawn_blocking(move || {
                let _token = token;
                released.recv().unwrap();
            })
        };
        let close = bg.close();
        tokio::pin!(close);
        assert!(
            tokio::time::timeout(Duration::from_millis(50), close.as_mut())
                .await
                .is_err(),
            "close returned while a call was running"
        );
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), close)
            .await
            .expect("close returns once every task and call is dropped");
        assert!(weak.upgrade().is_none(), "the task's state is dropped");
        call.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_task_does_not_hold_close() {
        let bg = Arc::new(Background::default());
        let (held, weak) = probe();
        assert!(bg.spawn(&Handle::current(), Job::Recheck, async move {
            let _held = held;
            panic!("a tracked task panics");
        }));
        tokio::time::timeout(Duration::from_secs(5), bg.close())
            .await
            .expect("close returns after a panicked task");
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn nothing_starts_after_close_and_one_runs_per_job() {
        let bg = Arc::new(Background::default());
        let rt = Handle::current();
        let job = Job::Ready([7; 32]);
        assert!(bg.spawn(&rt, job, std::future::pending()));
        assert!(!bg.spawn(&rt, job, async {}), "one per job");
        assert!(bg.spawn(&rt, Job::Ready([8; 32]), std::future::pending()));
        bg.close().await;
        assert!(!bg.spawn(&rt, Job::Recheck, async {}));
        assert!(!bg.spawn(&rt, Job::Ready([9; 32]), async {}));
        // Closing again returns at once.
        bg.close().await;
    }
}
