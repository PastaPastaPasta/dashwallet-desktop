//! Admission gate between session operations and session close (review M1).
//!
//! Every session operation holds an [`OpGuard`] while it uses the manager.
//! [`OpGate::close`] marks the gate closed and then waits until every guard
//! taken before that point is dropped. An operation that asks for a guard
//! after `close` started gets `None` and reports `network_not_open`.
//!
//! tokio's `RwLock` is fair: once `close` waits for the write lock, new
//! readers queue behind it, so a stream of new operations cannot starve the
//! close, and each of them sees the closed flag when it is admitted.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Default)]
pub(crate) struct OpGate {
    closed: AtomicBool,
    lock: RwLock<()>,
}

/// Held by one admitted operation; dropping it lets a pending close proceed.
pub(crate) struct OpGuard<'a> {
    _guard: RwLockReadGuard<'a, ()>,
}

impl OpGate {
    /// Admits an async operation, waiting while a close is in progress.
    /// `None` once the gate is closed.
    pub(crate) async fn enter(&self) -> Option<OpGuard<'_>> {
        let guard = self.lock.read().await;
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        Some(OpGuard { _guard: guard })
    }

    /// Admits a synchronous operation without waiting. `None` when the gate
    /// is closed or a close is in progress.
    pub(crate) fn try_enter(&self) -> Option<OpGuard<'_>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let guard = self.lock.try_read().ok()?;
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        Some(OpGuard { _guard: guard })
    }

    /// Closes the gate and waits for every admitted operation to finish.
    /// The returned guard keeps new operations out while the caller tears
    /// the session down; they are refused anyway once it is dropped.
    pub(crate) async fn close(&self) -> RwLockWriteGuard<'_, ()> {
        self.closed.store(true, Ordering::Release);
        self.lock.write().await
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_waits_for_in_flight_operations() {
        let gate = Arc::new(OpGate::default());
        let finished = Arc::new(AtomicUsize::new(0));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();

        let op = {
            let gate = Arc::clone(&gate);
            let finished = Arc::clone(&finished);
            tokio::spawn(async move {
                let _guard = gate.enter().await.expect("admitted before close");
                let _ = entered_tx.send(());
                tokio::time::sleep(Duration::from_millis(200)).await;
                finished.fetch_add(1, Ordering::SeqCst);
            })
        };
        entered_rx.await.unwrap();

        let closed = {
            let gate = Arc::clone(&gate);
            let finished = Arc::clone(&finished);
            tokio::spawn(async move {
                let _w = gate.close().await;
                // The close only gets here once the operation is done.
                finished.load(Ordering::SeqCst)
            })
        };
        assert_eq!(closed.await.unwrap(), 1);
        op.await.unwrap();
    }

    #[tokio::test]
    async fn operations_after_close_are_refused() {
        let gate = OpGate::default();
        assert!(gate.try_enter().is_some());
        drop(gate.close().await);
        assert!(gate.is_closed());
        assert!(gate.enter().await.is_none());
        assert!(gate.try_enter().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn operations_queued_behind_a_close_are_refused() {
        let gate = Arc::new(OpGate::default());
        let held = gate.enter().await.unwrap();
        let closing = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                let _w = gate.close().await;
            })
        };
        // Wait until the close has flagged the gate.
        while !gate.is_closed() {
            tokio::task::yield_now().await;
        }
        let late = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.enter().await.is_none() })
        };
        // A sync caller does not wait for the close.
        assert!(gate.try_enter().is_none());
        drop(held);
        closing.await.unwrap();
        assert!(
            late.await.unwrap(),
            "an operation admitted after close must be refused"
        );
    }
}
