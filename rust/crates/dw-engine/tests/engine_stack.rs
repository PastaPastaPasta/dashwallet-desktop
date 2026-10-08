//! The engine runtime's threads have 8 MiB stacks, not tokio's 2 MiB default
//! (E0-02). Kept apart from the other tests: a regression overflows the stack,
//! which aborts the whole test process.

use std::sync::Arc;

use dw_engine::{Engine, EngineConfig, EngineEvent, EventSink};

struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

/// Recurses until it holds `frames` live 64 KiB buffers: `frames * 64 KiB`
/// of stack, plus a little.
fn use_stack(frames: usize) -> usize {
    let mut buf = [0u8; 64 * 1024];
    buf[frames % buf.len()] = frames as u8;
    let below = if frames == 0 {
        0
    } else {
        use_stack(frames - 1)
    };
    std::hint::black_box(&buf);
    below + usize::from(buf[frames % buf.len()] == frames as u8)
}

/// 112 frames = 7 MiB: far over tokio's 2 MiB default (and over any 4 or 6 MiB
/// setting), under 8 MiB.
const FRAMES: usize = 112;

fn engine() -> (tempfile::TempDir, Engine) {
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
    (dir, engine)
}

#[test]
fn engine_workers_have_an_8_mib_stack() {
    let (_dir, engine) = engine();
    let on_worker = engine.block_on(async {
        tokio::spawn(async {
            assert_eq!(std::thread::current().name(), Some("dw-engine"));
            use_stack(FRAMES)
        })
        .await
        .unwrap()
    });
    assert_eq!(on_worker, FRAMES + 1);
}

#[test]
fn engine_blocking_threads_have_an_8_mib_stack() {
    let (_dir, engine) = engine();
    // `block_on` enters the engine runtime, so `spawn_blocking` uses its pool.
    let on_blocking = engine.block_on(async {
        tokio::task::spawn_blocking(|| {
            assert_eq!(std::thread::current().name(), Some("dw-engine"));
            use_stack(FRAMES)
        })
        .await
        .unwrap()
    });
    assert_eq!(on_blocking, FRAMES + 1);
}
