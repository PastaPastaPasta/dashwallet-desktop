//! `engine_poisoned` (DEC-110): a panic on the engine's threads marks the
//! engine poisoned even when nobody awaits the task, and the hook the
//! engine installed still calls the one it replaced. Its own process: the
//! flag is process-wide and never cleared.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use dw_engine::{Engine, EngineConfig, EngineEvent, EventSink, engine_poisoned};

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

#[test]
fn an_unawaited_task_panic_poisons_the_engine() {
    static HOST_HOOK: AtomicBool = AtomicBool::new(false);
    std::panic::set_hook(Box::new(|_| HOST_HOOK.store(true, Ordering::SeqCst)));
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: dw_vault::VaultConfig::default(),
        },
        Arc::new(NullSink),
    )
    .unwrap();
    assert!(!engine_poisoned());

    // A panic on the caller's thread is not the engine's.
    let caught = std::panic::catch_unwind(|| panic!("not an engine task"));
    assert!(caught.is_err());
    assert!(!engine_poisoned());
    assert!(HOST_HOOK.swap(false, Ordering::SeqCst));

    // A task nobody awaits: only the hook can see its panic.
    engine.block_on(async {
        drop(tokio::spawn(async { panic!("unawaited engine task") }));
    });
    let start = Instant::now();
    while !engine_poisoned() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(engine_poisoned());
    assert!(HOST_HOOK.load(Ordering::SeqCst));
}
