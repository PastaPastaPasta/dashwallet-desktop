//! Review DW-E0-09 r1 (GPT) finding 1 and r2 (Sol) findings 1–2: no heap
//! block that held a bearer input may be freed unwiped. A scanning
//! allocator checks every block this thread frees while armed for a marker,
//! as the reviews' probes did, over the session's request paths: literal
//! and escaped inputs, a refused shape, a secret in `args` in any form or
//! position the grammar refuses, a secret `id`, and the bearer reader.
//!
//! Reading freed blocks may touch bytes that were never initialized; that
//! is acceptable in this test-only allocator.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use dw_engine::{DashNetwork, EngineConfig, EngineEvent, EventSink, SessionOptions};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

use super::*;

const MARKER: &[u8] = b"HEAP-SECRET-PROBE-ONLY";

struct Scan;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

static HITS: AtomicUsize = AtomicUsize::new(0);

// SAFETY: forwards to `System`; `dealloc` only reads the block it frees.
unsafe impl GlobalAlloc for Scan {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ARMED.try_with(Cell::get).unwrap_or(false) && layout.size() >= MARKER.len() {
            // SAFETY: the block is still allocated and `size` bytes long.
            let block = unsafe { std::slice::from_raw_parts(ptr, layout.size()) };
            if block.windows(MARKER.len()).any(|w| w == MARKER) {
                HITS.fetch_add(1, Ordering::SeqCst);
            }
        }
        unsafe { System.dealloc(ptr, layout) }
    }
    // `realloc` keeps the default (alloc, copy, dealloc), so a block a
    // reallocation frees is scanned as well.
}

#[global_allocator]
static ALLOC: Scan = Scan;

/// Blocks freed unwiped by this thread while running `f`.
fn unwiped_frees(f: impl FnOnce()) -> usize {
    let before = HITS.load(Ordering::SeqCst);
    ARMED.with(|a| a.set(true));
    f();
    ARMED.with(|a| a.set(false));
    HITS.load(Ordering::SeqCst) - before
}

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

#[test]
fn session_requests_free_no_unwiped_secret() {
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let opts = SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        ..Default::default()
    };
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts))
        .unwrap();
    let ctx = Ctx {
        engine: &engine,
        session: &session,
        passphrase: None,
        wallet: None,
    };
    let long_name = format!(
        r#"{{"args":["profile","set","--display-name","HEAP-SECRET-PROBE-ONLY{}"]}}"#,
        "x".repeat(300)
    );
    let mut cases: Vec<Vec<u8>> = [
        // The review's four reproductions.
        r#"{"args":["invite","stash"],"input":"dash:?invite=HEAP-SECRET-PROBE-ONLY","id":1}"#,
        r#"{"args":["invite","stash"],"input":"dash:?invite=HEAP-SECRET-PROBE-ONLY"}"#,
        r#"{"args":false,"input":"dash:?invite=HEAP-SECRET-PROBE-ONLY","id":2}"#,
        r#"{"args":["invite","stash","dash:?invite=HEAP-SECRET-PROBE-ONLY"],"id":3}"#,
        // Neighbours: an escaped bearer in `args`, an unknown field, a
        // malformed line, input before args, a wrong-typed input.
        r#"{"args":["invite","stash","dashpay://invite?pk=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash"],"secret":"HEAP-SECRET-PROBE-ONLY"}"#,
        r#"{"args":["invite","stash"],"input":"HEAP-SECRET-PROBE-ONLY"#,
        r#"{"input":"dash:?invite=HEAP-SECRET-PROBE-ONLY\n","args":["contact","scan"]}"#,
        r#"{"args":["invite","stash"],"input":["HEAP-SECRET-PROBE-ONLY"]}"#,
        // r2 (Sol) finding 1: percent-encoded, split and bare secrets in
        // `args`, which no pattern list catches.
        r#"{"args":["invite","stash","dashpay%3A%2F%2Finvite%3Fpk%3DHEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","dashpay","://","invite?pk","=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","HEAP-SECRET-PROBE-ONLY"]}"#,
        // One refused value per type, then unknown names in each slot.
        r#"{"args":["identity","list","--wallet","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","set-main","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","discard","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["dashpay","dispatch-status","registration/HEAP-SECRET-PROBE-ONLY/funding"]}"#,
        r#"{"args":["identity","withdraw","--credits","5","--to","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","top-up","--duffs","1HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["name","search","abc","--limit","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["contact","list","--sort","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["name","register","dash:?invite=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["name","check","dash:?invite=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["name","resolve","HEAP-SECRET-PROBE-ONLY.dash.dash"]}"#,
        r#"{"args":["profile","set","--avatar-url","dashpay://invite?pk=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["profile","set","--avatar-url","https://invitations.dashpay.io/applink?du=a&assetlocktx=b&pk=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","--input-file","/invitations.dashpay.io/HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","--input-file","dash:?invite=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["contact","details","29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV2","--note","dash:?invite=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["contact","details","29d2S7vB453rNYFdR5Ycwt7y9haRT5fwVwL9zTmBhfV2","--alias","HEAP-SECRET-PROBE-ONLY\u0007"]}"#,
        long_name.as_str(),
        r#"{"args":["HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","--HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["invite","stash","--link=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","list","--spv=HEAP-SECRET-PROBE-ONLY"]}"#,
        r#"{"args":["identity","list","-HEAP-SECRET-PROBE-ONLY"]}"#,
        // r2 (Sol) finding 2: a secret `id` on a malformed line and on a
        // refused shape.
        r#"{"args":[],"id":"dash:?invite=HEAP-SECRET-PROBE-ONLY","input":"\q"}"#,
        r#"{"args":false,"id":"dash:?invite=HEAP-SECRET-PROBE-ONLY"}"#,
    ]
    .into_iter()
    .map(|s| s.as_bytes().to_vec())
    .collect();
    for line in &cases {
        let mut answered = None;
        let hits = unwiped_frees(|| answered = Some(session_line(&ctx, line)));
        let (_, result) = answered.unwrap();
        assert!(result.is_err(), "{}", String::from_utf8_lossy(line));
        assert_eq!(hits, 0, "{}", String::from_utf8_lossy(line));
    }
    // The whole loop, answers included: an accepted request's `id` is
    // written from its zeroizing buffer, never copied.
    cases.push(
        br#"{"args":["name","check","alice"],"id":"dash:?invite=HEAP-SECRET-PROBE-ONLY"}"#.to_vec(),
    );
    let stream = cases.join(&b'\n');
    let hits = unwiped_frees(|| {
        let done = session_loop(&ctx, &mut &stream[..], &mut std::io::sink()).unwrap();
        assert_eq!(done, json!({"requests": cases.len()}));
    });
    assert_eq!(hits, 0);
    // The one-shot bearer reader, from stdin and from a file.
    let file = dir.path().join("link");
    std::fs::write(&file, "dash:?invite=HEAP-SECRET-PROBE-ONLY\n").unwrap();
    for input in [
        SecretInput { input_file: None },
        SecretInput {
            input_file: Some(file),
        },
    ] {
        let hits = unwiped_frees(|| {
            let mut stdin = &b"dash:?invite=HEAP-SECRET-PROBE-ONLY\n"[..];
            drop(read_secret(&input, &mut stdin).unwrap());
        });
        assert_eq!(hits, 0);
    }
    engine.block_on(engine.shutdown()).unwrap();
}
