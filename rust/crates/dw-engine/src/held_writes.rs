//! Tests: an app.sqlite write held in its job past the removal of its
//! wallet (DEC-210).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::coins::{HOLD_APPDB_JOB, TestPause};
use crate::{Engine, NetworkSession, WalletId};

/// Releases a held write at the end of a test, passed or failed: the
/// runtime's drop would wait for it.
pub(crate) struct ReleaseOnDrop(pub(crate) Arc<TestPause>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release.notify_one();
    }
}

/// Runs `write` on the session's runtime with its first app.sqlite job
/// held at the returned pause.
pub(crate) fn hold_first_job<F>(
    s: &Arc<NetworkSession>,
    write: F,
) -> (Arc<TestPause>, tokio::task::JoinHandle<()>)
where
    F: Future<Output = ()> + Send + 'static,
{
    let pause = Arc::new(TestPause::default());
    let task = s.rt.spawn(HOLD_APPDB_JOB.scope(Arc::clone(&pause), write));
    (pause, task)
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// With a write held at `pause` in its app.sqlite job (its task, if any,
/// cancelled): the removal of wallet `id` with the `wipe` grant returns
/// while the write is held, and the write, run after it, leaves nothing
/// (`nothing` reads the wallet's rows of that write).
pub(crate) fn a_write_held_past_the_removal(
    engine: &Engine,
    s: &Arc<NetworkSession>,
    (id, wipe): (WalletId, String),
    pause: &Arc<TestPause>,
    task: Option<tokio::task::JoinHandle<()>>,
    nothing: impl Fn() -> bool,
) {
    let _release = ReleaseOnDrop(Arc::clone(pause));
    wait_until("the held write", || pause.reached.load(Ordering::SeqCst));
    if let Some(task) = task {
        task.abort();
        assert!(engine.block_on(task).is_err_and(|e| e.is_cancelled()));
    }
    engine
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(60), s.remove_wallet(id, wipe)).await
        })
        .expect("the removal waited for the held write")
        .unwrap();
    assert!(!pause.done.load(Ordering::SeqCst), "held until now");
    pause.release.notify_one();
    wait_until("the late write", || pause.done.load(Ordering::SeqCst));
    assert!(nothing(), "the late write left a row of the removed wallet");
}
