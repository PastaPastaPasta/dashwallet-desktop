//! The lease table's background work (BL-544): Mode B's funding refreshes
//! and the catch-up's resume passes, with the resumes a pass starts.
//!
//! Each holds a session wallet while it reads or resumes it, and through
//! the wallet its database (the persister's process-wide open-path claim).
//! A task that close only aborts lets go of the wallet when the runtime next
//! drops its future, which may be after `close_network` has returned: a
//! reopen in that window finds the database still open (`storage_in_use`).
//! So every one of them runs tracked here, and the table's close aborts
//! them, starts none after, and returns only once each future is dropped.

use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::runtime::Handle;
use tokio::sync::Notify;
use tokio::task::AbortHandle;

/// The tracked work of one lease table.
#[derive(Debug, Default)]
pub(crate) struct Background {
    state: Mutex<State>,
    /// Woken when the last tracked future is dropped.
    idle: Notify,
}

#[derive(Debug, Default)]
struct State {
    closed: bool,
    /// Tracked futures not yet dropped, spawned or not.
    live: usize,
    /// The spawned ones, for close to abort.
    tasks: Vec<AbortHandle>,
}

/// Held by a tracked future; dropped with it.
struct Token(Arc<Background>);

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

    /// `fut`, tracked from now until it is dropped (run to its end or
    /// not); `None` once closed, so nothing starts after close.
    pub(crate) fn track<F: Future>(
        self: &Arc<Self>,
        fut: F,
    ) -> Option<impl Future<Output = F::Output> + use<F>> {
        let token = {
            let mut state = self.state();
            if state.closed {
                return None;
            }
            state.live += 1;
            Token(Arc::clone(self))
        };
        Some(async move {
            let _token = token;
            fut.await
        })
    }

    /// Spawns `fut` on `rt`, tracked; `None` (nothing spawned) once closed.
    pub(crate) fn spawn<F>(self: &Arc<Self>, rt: &Handle, fut: F) -> Option<AbortHandle>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task = rt.spawn(self.track(fut)?).abort_handle();
        let mut state = self.state();
        state.tasks.retain(|t| !t.is_finished());
        // Close ran since `track`: it did not see this task.
        if state.closed {
            task.abort();
        } else {
            state.tasks.push(task.clone());
        }
        Some(task)
    }

    /// Close: aborts the spawned tasks, starts none after, and returns once
    /// every tracked future has been dropped.
    pub(crate) async fn close(&self) {
        let tasks = {
            let mut state = self.state();
            state.closed = true;
            std::mem::take(&mut state.tasks)
        };
        for task in tasks {
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
    async fn close_returns_after_every_tracked_future_is_dropped() {
        let bg = Arc::new(Background::default());
        let rt = Handle::current();
        let (held, weak) = probe();
        let (started_tx, started) = tokio::sync::oneshot::channel();
        bg.spawn(&rt, async move {
            let _held = held;
            let _ = started_tx.send(());
            std::future::pending::<()>().await;
        })
        .unwrap();
        // A tracked future polled inside another task (a pass's resumes).
        let (inner_held, inner_weak) = probe();
        let inner = bg
            .track(async move {
                let _held = inner_held;
                std::future::pending::<()>().await;
            })
            .unwrap();
        let outer = rt.spawn(inner).abort_handle();
        started.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            // The outer task is not the table's: close waits for its tracked
            // future until something else drops it.
            let close = bg.close();
            tokio::pin!(close);
            assert!(
                tokio::time::timeout(Duration::from_millis(50), close.as_mut())
                    .await
                    .is_err(),
                "close returned while a tracked future was alive"
            );
            outer.abort();
            close.await;
        })
        .await
        .expect("close returns once every tracked future is dropped");
        assert!(
            weak.upgrade().is_none(),
            "the spawned task's state is dropped"
        );
        assert!(
            inner_weak.upgrade().is_none(),
            "the tracked future is dropped"
        );
    }

    /// A tracked task that panics is dropped like one that ends: close
    /// does not wait for it forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_panicking_task_does_not_hold_close() {
        let bg = Arc::new(Background::default());
        let (held, weak) = probe();
        let task = bg
            .spawn(&Handle::current(), async move {
                let _held = held;
                panic!("a tracked task panics");
            })
            .unwrap();
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        tokio::time::timeout(Duration::from_secs(5), bg.close())
            .await
            .expect("close returns after a panicked task");
        assert!(weak.upgrade().is_none());
    }

    #[tokio::test]
    async fn nothing_starts_after_close() {
        let bg = Arc::new(Background::default());
        bg.close().await;
        assert!(bg.track(async {}).is_none());
        assert!(bg.spawn(&Handle::current(), async {}).is_none());
        // Closing again returns at once.
        bg.close().await;
    }

    #[tokio::test]
    async fn a_finished_future_leaves_nothing_to_wait_for() {
        let bg = Arc::new(Background::default());
        bg.track(async {}).unwrap().await;
        let task = bg.spawn(&Handle::current(), async {}).unwrap();
        while !task.is_finished() {
            tokio::task::yield_now().await;
        }
        tokio::time::timeout(Duration::from_secs(5), bg.close())
            .await
            .unwrap();
    }
}
