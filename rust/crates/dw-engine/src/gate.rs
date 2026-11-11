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
//!
//! An in-app close [`OpGate::hold`]s the gate until it is decided (a
//! switch's close marks it [`OpGate::is_held`] meanwhile, as a refusal
//! leaves the network open), and a
//! network switch's close also pauses it: async operations asking for a
//! guard wait, the ones admitted drain, and the close checks whether the
//! session is busy before it commits. A refused close resumes the gate and
//! the waiting operations are admitted as if nothing happened; a committed
//! one seals it and they are refused. The pause is not a queued writer, so
//! `try_enter` still admits meanwhile: the session's timers and loops
//! (mixing ticks, the relock) give up on a refused `try_enter`, and none of
//! them starts what the check looks for.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard, Notify, RwLock, RwLockReadGuard, RwLockWriteGuard};

#[derive(Default)]
pub(crate) struct OpGate {
    closed: AtomicBool,
    lock: RwLock<()>,
    paused: AtomicBool,
    /// An in-app close holds the gate ([`OpGate::hold`]) and is not decided.
    held: AtomicBool,
    /// Guards alive. Read with `paused` in the opposite order to `enter`
    /// (both `SeqCst`): an operation either sees the pause and backs off,
    /// or the pause sees it and waits for it.
    admitted: AtomicUsize,
    /// A guard was dropped, or the pause ended.
    changed: Notify,
    /// Tests: how many pauses reached their drain.
    #[cfg(test)]
    drains: AtomicUsize,
    /// Tests: how many wait on a pause ([`OpGate::unpaused`]).
    #[cfg(test)]
    waiting_on_pause: AtomicUsize,
    /// Tests: how many entrants, admitted, saw the pause and backed off.
    #[cfg(test)]
    backed_off: AtomicUsize,
    /// Tests: hold the next `enter` before it is admitted (its read lock
    /// taken, the gate not closed), and between its last two loads.
    #[cfg(test)]
    pub(crate) hold_entering: std::sync::Mutex<Option<std::sync::Arc<crate::coins::TestPause>>>,
    #[cfg(test)]
    pub(crate) hold_between_loads:
        std::sync::Mutex<Option<std::sync::Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next in-app close once it is checked, before it
    /// takes the engine's sessions lock.
    #[cfg(test)]
    pub(crate) hold_before_sessions:
        std::sync::Mutex<Option<std::sync::Arc<crate::coins::TestPause>>>,
    /// Tests: runs when a hold ends, between the release of its load fence
    /// and the end of its pause.
    #[cfg(test)]
    pub(crate) on_release: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// Held by one admitted operation; dropping it lets a pending close proceed.
pub(crate) struct OpGuard<'a> {
    gate: &'a OpGate,
    _guard: RwLockReadGuard<'a, ()>,
}

impl Drop for OpGuard<'_> {
    fn drop(&mut self) {
        self.gate.admitted.fetch_sub(1, Ordering::SeqCst);
        self.gate.changed.notify_waiters();
    }
}

/// An in-app close not decided yet ([`OpGate::hold`]). Dropping it (a
/// refusal) releases the close's load fence first and then resumes the
/// gate: a shielded store's first open waiting for the decision
/// (`shielded_pool::coordinator`) then finds the fence free.
pub(crate) struct Hold<'a> {
    gate: &'a OpGate,
    loads: Option<RwLockWriteGuard<'a, ()>>,
    paused: bool,
    waited: bool,
}

impl<'a> Hold<'a> {
    /// Keeps the close's wallet-load fence until the hold ends or commits.
    pub(crate) fn fence(&mut self, loads: RwLockWriteGuard<'a, ()>) {
        self.loads = Some(loads);
    }

    /// Pauses admissions: async operations that ask wait until the hold
    /// ends, or are refused once it commits.
    pub(crate) fn pause(&mut self) {
        self.gate.paused.store(true, Ordering::SeqCst);
        self.paused = true;
    }

    /// `busy` at once, then again once every admitted operation has
    /// finished, waiting up to `drain` for them. As no async operation is
    /// admitted meanwhile, none starts between the second check and the
    /// close.
    pub(crate) async fn check<B>(
        &self,
        drain: Duration,
        busy: impl Fn() -> Option<B>,
    ) -> Result<(), PauseRefused<B>> {
        debug_assert!(self.paused, "a check drains a paused gate");
        if let Some(b) = busy() {
            return Err(PauseRefused::Busy(b));
        }
        #[cfg(test)]
        self.gate.drains.fetch_add(1, Ordering::SeqCst);
        let drained = tokio::time::timeout(
            drain,
            self.gate.until(|g| g.admitted.load(Ordering::SeqCst) == 0),
        )
        .await;
        if let Some(b) = busy() {
            return Err(PauseRefused::Busy(b));
        }
        drained.map_err(|_| PauseRefused::Draining)
    }

    /// The close goes ahead: a paused gate is sealed, refusing what waits
    /// and what asks from now on. Returns the load fence, which the close
    /// keeps until it is done.
    pub(crate) fn commit(mut self) -> Option<RwLockWriteGuard<'a, ()>> {
        if self.paused {
            self.gate.closed.store(true, Ordering::SeqCst);
        }
        self.loads.take()
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        drop(self.loads.take());
        #[cfg(test)]
        if let Some(f) = (self.gate.on_release.lock().unwrap()).take() {
            f();
        }
        if self.paused {
            self.gate.paused.store(false, Ordering::SeqCst);
        }
        if self.waited {
            self.gate.held.store(false, Ordering::SeqCst);
        }
        self.gate.changed.notify_waiters();
    }
}

/// Why [`Hold::check`] refused.
pub(crate) enum PauseRefused<B> {
    /// What the check found running.
    Busy(B),
    /// The admitted operations did not finish within the drain.
    Draining,
}

/// Serialises, per session, every operation that registers or removes a
/// wallet: seed and watch-only imports, restores and their rollback, the
/// removal, and dash-qt's open and close of a wallet (DEC-210 (3b)). A
/// removal and an import of the same wallet run one after the other, so
/// app.sqlite records it as removed exactly when it is not registered.
/// These operations are rare, so one lock covers every wallet.
///
/// Lock order: an [`OpGuard`], then this lock, then the wallet's barrier
/// (`revoke_wallet`), the end of its discoveries, and its write fence
/// (private details, the bell's `mark_read`) and identity locks (`remove_from_manager`). It is taken
/// only by an operation that holds its `OpGuard` and nothing later in the
/// order, and its holder never asks the gate again: a nested admission
/// waits behind a pending close, which waits for the holder's guard.
/// `close` never takes it: once the gate has drained, no holder is left.
/// It is unrelated to the SPV start and stop lock (`platform.lifecycle`):
/// neither is taken while the other is held. It orders operations of
/// one process; across processes only the removal records apply.
#[derive(Default)]
pub(crate) struct WalletLifecycle {
    lock: Mutex<()>,
    /// Tests: how many wait for it.
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
}

/// Held by a wallet registration or removal. Helpers that must run under
/// it take a reference as proof.
pub(crate) struct Lifecycle<'a> {
    _guard: MutexGuard<'a, ()>,
}

impl WalletLifecycle {
    pub(crate) async fn hold(&self) -> Lifecycle<'_> {
        #[cfg(test)]
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let guard = self.lock.lock().await;
        #[cfg(test)]
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        Lifecycle { _guard: guard }
    }

    /// Tests: how many operations wait for it.
    #[cfg(test)]
    pub(crate) fn waiting(&self) -> usize {
        self.waiting.load(Ordering::SeqCst)
    }
}

impl OpGate {
    /// Admits an async operation, waiting while a close is in progress or
    /// the gate is paused. `None` once the gate is closed.
    ///
    /// The closed check after the pause comes before the read lock: an
    /// entrant a pause held must not queue behind a close's writer. Nested
    /// in an admitted operation, it would wait for the writer, which waits
    /// for the outer guard (a quit during a switch's drain).
    pub(crate) async fn enter(&self) -> Option<OpGuard<'_>> {
        loop {
            if !self.unpaused().await {
                return None;
            }
            let guard = self.lock.read().await;
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            #[cfg(test)]
            crate::coins::pause_at(&self.hold_entering).await;
            let guard = self.admit(guard);
            // Paused after the wait: back off, the pause may be draining.
            if self.paused.load(Ordering::SeqCst) {
                #[cfg(test)]
                self.backed_off.fetch_add(1, Ordering::SeqCst);
                continue;
            }
            #[cfg(test)]
            crate::coins::pause_at(&self.hold_between_loads).await;
            // Read last: a commit seals before its pause ends, so a pause
            // seen ended by a commit is seen sealed.
            if self.closed.load(Ordering::SeqCst) {
                return None;
            }
            return Some(guard);
        }
    }

    /// Waits while a switch's close pauses the gate. `false` once the gate
    /// is closed, which wakes the wait.
    ///
    /// A call that signs asks it before it takes the session's close
    /// preflight, so it never holds that preflight while the pause holds it:
    /// the switch would read it as a spend running and refuse. A pause that
    /// starts between this wait and the preflight read still does; no spend
    /// has started then, and asking again goes ahead.
    pub(crate) async fn unpaused(&self) -> bool {
        #[cfg(test)]
        self.waiting_on_pause.fetch_add(1, Ordering::SeqCst);
        self.until(|g| !g.paused.load(Ordering::SeqCst) || g.closed.load(Ordering::SeqCst))
            .await;
        #[cfg(test)]
        self.waiting_on_pause.fetch_sub(1, Ordering::SeqCst);
        !self.closed.load(Ordering::SeqCst)
    }

    /// Waits until `done`, rechecked whenever a guard drops or a pause ends.
    async fn until(&self, done: impl Fn(&Self) -> bool) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if done(self) {
                return;
            }
            changed.await;
        }
    }

    fn admit<'a>(&'a self, guard: RwLockReadGuard<'a, ()>) -> OpGuard<'a> {
        self.admitted.fetch_add(1, Ordering::SeqCst);
        OpGuard {
            gate: self,
            _guard: guard,
        }
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
        Some(self.admit(guard))
    }

    /// Holds the gate for an in-app close (one at a time) until the
    /// returned [`Hold`] commits or is dropped. `waited`: a shielded
    /// store's first open waits for the decision rather than failing (a
    /// network switch's close, which a refusal leaves open).
    pub(crate) fn hold(&self, waited: bool) -> Hold<'_> {
        if waited {
            self.held.store(true, Ordering::SeqCst);
        }
        Hold {
            gate: self,
            loads: None,
            paused: false,
            waited,
        }
    }

    /// Tests: a held and paused gate.
    #[cfg(test)]
    pub(crate) fn pause(&self) -> Hold<'_> {
        let mut hold = self.hold(true);
        hold.pause();
        hold
    }

    /// Tests: no pause in force.
    #[cfg(test)]
    pub(crate) fn pause_ended(&self) -> bool {
        !self.paused.load(Ordering::SeqCst)
    }

    /// A network switch's close holds the gate and is not decided yet.
    pub(crate) fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst) && !self.closed.load(Ordering::SeqCst)
    }

    /// Tests: how many pauses reached their drain.
    #[cfg(test)]
    pub(crate) fn drains(&self) -> usize {
        self.drains.load(Ordering::SeqCst)
    }

    /// Tests: how many wait on a pause.
    #[cfg(test)]
    pub(crate) fn waiting_on_pause(&self) -> usize {
        self.waiting_on_pause.load(Ordering::SeqCst)
    }

    /// Tests: how many admitted entrants backed off on a pause.
    #[cfg(test)]
    pub(crate) fn backed_off(&self) -> usize {
        self.backed_off.load(Ordering::SeqCst)
    }

    /// Tests: guards alive.
    #[cfg(test)]
    pub(crate) fn admitted(&self) -> usize {
        self.admitted.load(Ordering::SeqCst)
    }

    /// Closes the gate and waits for every admitted operation to finish.
    /// The returned guard keeps new operations out while the caller tears
    /// the session down; they are refused anyway once it is dropped.
    /// Entrants waiting on a pause are refused at once.
    pub(crate) async fn close(&self) -> RwLockWriteGuard<'_, ()> {
        self.closed.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
        self.lock.write().await
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

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

    const LONG: Duration = Duration::from_secs(20);

    async fn until(what: impl Fn() -> bool) {
        tokio::time::timeout(LONG, async {
            while !what() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("timed out");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_refused_pause_admits_what_waited() {
        let gate = Arc::new(OpGate::default());
        let held = gate.enter().await.unwrap();
        let busy = Arc::new(AtomicBool::new(false));
        let pausing = {
            let (gate, busy) = (Arc::clone(&gate), Arc::clone(&busy));
            tokio::spawn(async move {
                let paused = gate.pause();
                let refused = paused.check(LONG, || busy.load(Ordering::SeqCst).then_some(()));
                matches!(refused.await, Err(PauseRefused::Busy(())))
            })
        };
        until(|| gate.drains() == 1).await;
        let late = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.enter().await.is_some() })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!late.is_finished(), "an operation waits while paused");
        // The admitted operation turns the session busy, then finishes.
        busy.store(true, Ordering::SeqCst);
        drop(held);
        assert!(pausing.await.unwrap(), "the check after the drain refuses");
        assert!(late.await.unwrap(), "admitted once the pause ended");
        assert!(!gate.is_closed());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_sealed_pause_refuses_what_waited() {
        let gate = Arc::new(OpGate::default());
        let paused = gate.pause();
        assert!(paused.check(LONG, || None::<()>).await.is_ok());
        let late = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.enter().await.is_none() })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!late.is_finished());
        paused.commit();
        assert!(late.await.unwrap(), "refused once sealed");
        assert!(gate.is_closed());
        drop(gate.close().await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pause_waits_for_sync_operations_and_gives_up_after_the_drain() {
        let gate = Arc::new(OpGate::default());
        let held = gate.try_enter().unwrap();
        let paused = gate.pause();
        let refused = paused.check(Duration::from_millis(50), || None::<()>).await;
        assert!(matches!(refused, Err(PauseRefused::Draining)));
        assert!(gate.is_held());
        drop(paused);
        assert!(gate.enter().await.is_some(), "resumed after a refusal");
        // A sync caller is admitted while paused, and drained.
        let pausing = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.pause().check(LONG, || None::<()>).await.is_ok() })
        };
        until(|| gate.drains() == 2).await;
        let during = gate.try_enter();
        assert!(during.is_some(), "try_enter does not wait for a pause");
        drop(held);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!pausing.is_finished(), "the drain waits for it too");
        drop(during);
        assert!(pausing.await.unwrap());
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

    /// Row 74 r3 M1: an entrant a pause holds, nested in an admitted
    /// operation, is refused once a close starts, without queueing behind
    /// the close's writer (which waits for the outer guard).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_close_refuses_an_entrant_the_pause_holds_without_queueing_it() {
        let gate = Arc::new(OpGate::default());
        let outer = gate.enter().await.unwrap();
        let hold = gate.pause();
        let nested = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move { gate.enter().await.is_none() })
        };
        while gate.waiting_on_pause() == 0 {
            tokio::task::yield_now().await;
        }
        let closing = {
            let gate = Arc::clone(&gate);
            tokio::spawn(async move {
                let _w = gate.close().await;
            })
        };
        let refused = tokio::time::timeout(Duration::from_secs(10), nested)
            .await
            .expect("the nested entrant waits behind the close's writer")
            .unwrap();
        assert!(refused, "a nested entrant is refused once the gate closes");
        assert!(
            !closing.is_finished(),
            "the close waits for the outer guard"
        );
        drop(outer);
        closing.await.unwrap();
        drop(hold);
    }

    /// Row 74 r3 (Opus N2): a closed gate is not held, so a shielded store's
    /// first open stops waiting for a switch once a quit closes the gate.
    #[tokio::test]
    async fn a_closed_gate_is_not_held() {
        let gate = OpGate::default();
        let _hold = gate.hold(true);
        assert!(gate.is_held());
        let _w = gate.close().await;
        assert!(!gate.is_held());
    }
}
