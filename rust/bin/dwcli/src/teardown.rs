//! How dwcli ends the engine (DEC-110). Two rules cover every path, found
//! or not, by which a command can leave the engine wedged:
//!
//! - An engine task panicked ([`dw_engine::engine_poisoned`]), whatever
//!   became of its result: the engine is probed ([`healthy`]) before SPV is
//!   stopped or the engine shut down, and abandoned if it fails the probe.
//! - Every SPV stop and engine shutdown runs under a deadline
//!   (`--shutdown-timeout`, 10 s by default), and the engine is abandoned if
//!   it misses it.
//!
//! An abandoned engine is neither stopped nor shut down: `main` wipes the
//! passphrase, writes what it owes best effort and exits with status 1.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dw_engine::{Engine, EngineError, NetworkSession, engine_poisoned};

/// The default deadline of an SPV stop and of the engine's shutdown.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(10);

/// How long the health probe may take.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

static DEADLINE: OnceLock<Duration> = OnceLock::new();

/// Why the engine is abandoned, once it is.
static ABANDONED: Mutex<Option<String>> = Mutex::new(None);

/// Sets the deadline (`--shutdown-timeout`), once.
pub fn set_deadline(deadline: Duration) {
    let _ = DEADLINE.set(deadline);
}

pub fn deadline() -> Duration {
    DEADLINE.get().copied().unwrap_or(DEFAULT_DEADLINE)
}

fn abandoned_slot() -> std::sync::MutexGuard<'static, Option<String>> {
    ABANDONED.lock().unwrap_or_else(|p| p.into_inner())
}

/// Abandons the engine; the first reason stands.
pub fn abandon(why: impl Into<String>) {
    abandoned_slot().get_or_insert_with(|| why.into());
}

/// Why the engine is abandoned, if it is.
pub fn abandoned() -> Option<String> {
    abandoned_slot().clone()
}

/// For a test that may abandon the engine, or tears one down: holds the
/// other such tests off (the abandonment is the process's) and starts and
/// ends with none.
#[cfg(test)]
pub fn exclusive() -> impl Drop {
    static SERIAL: Mutex<()> = Mutex::new(());
    struct Exclusive(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);
    impl Drop for Exclusive {
        fn drop(&mut self) {
            *abandoned_slot() = None;
        }
    }
    let serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    *abandoned_slot() = None;
    Exclusive(serial)
}

/// Whether the engine is abandoned, or must be: an engine task panicked
/// and the engine fails its probe (`probe_fault` fails it on purpose, in
/// debug builds' fault injection).
pub fn must_abandon(session: &Arc<NetworkSession>, probe_fault: bool) -> bool {
    if abandoned().is_some() {
        return true;
    }
    if engine_poisoned() && !healthy(session, probe_fault) {
        abandon("the engine failed its health probe after an engine task panicked");
        return true;
    }
    false
}

/// Whether the engine still works. It fails closed: the session must be
/// open, the vault must answer without panicking (its status has no
/// error), the wallet list without an error, and the engine runtime must
/// run a task, all within [`PROBE_TIMEOUT`]. A panic, an error or the
/// timeout fails it, and so does `fail`.
pub fn healthy(session: &Arc<NetworkSession>, fail: bool) -> bool {
    let session = Arc::clone(session);
    within("dwcli-health-probe", PROBE_TIMEOUT, move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if fail {
                panic!("injected poison");
            }
            let _ = session.vault().status();
            session.is_open()
                && session.wallet_infos().is_ok()
                && wait(session.run_on_engine(async {})).is_ok()
        }))
        .unwrap_or(false)
    })
    .unwrap_or(false)
}

/// Stops SPV, unless the engine [`must_abandon`], under the deadline: a
/// missed one abandons the engine. The error is the stop's, or why it did
/// not finish.
pub fn stop_spv(session: &Arc<NetworkSession>, probe_fault: bool) -> Result<(), EngineError> {
    if must_abandon(session, probe_fault) {
        return Err(EngineError::Spv(
            "not stopped: the engine is abandoned".into(),
        ));
    }
    let stopping = Arc::clone(session);
    within("dwcli-stop-spv", deadline(), move || {
        wait(stopping.stop_spv())
    })
    .unwrap_or_else(|| {
        let why = format!("stopping SPV missed its {:?} deadline", deadline());
        abandon(why.clone());
        Err(EngineError::Spv(why))
    })
}

/// Shuts the engine down, unless it [`must_abandon`], under the deadline.
/// `Err` is why the engine is abandoned instead: the caller exits without
/// the rest of the shutdown.
pub fn shut_down(
    engine: Engine,
    session: &Arc<NetworkSession>,
    probe_fault: bool,
) -> Result<Result<(), EngineError>, String> {
    if must_abandon(session, probe_fault) {
        // Its drop would close the sessions in the background.
        std::mem::forget(engine);
    } else if let Some(done) = within("dwcli-shutdown", deadline(), move || {
        engine.block_on(engine.shutdown())
    }) {
        return Ok(done);
    } else {
        abandon(format!(
            "the engine's shutdown missed its {:?} deadline",
            deadline()
        ));
    }
    Err(abandoned().unwrap_or_default())
}

/// Runs `f` on a thread of its own for up to `timeout`: `None` if it did
/// not finish (the thread is then left behind) or could not start.
fn within<T: Send + 'static>(
    name: &str,
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(timeout).ok()
}

/// Polls `fut` to completion on this thread, which runs no runtime: for a
/// future that only waits for engine tasks (the engine's own `block_on`
/// needs the engine, which stays with its owner).
pub fn wait<F: std::future::Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Arc::new(Unpark(std::thread::current())).into();
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}
