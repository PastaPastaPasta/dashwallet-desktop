//! H-06 performance probes (docs/research/h-06-performance.md): wallet open,
//! `history_page(200)` and engine memory on the regtest 40k wallet, through
//! the public API in a release build. Ignored; Linux only. The contacts
//! probes are in `src/platform/contacts_perf_tests.rs`.
//!
//! ```sh
//! DWD_H06_DATADIR=/work/scratch/h06-perf/fixture DWD_H06_ITERS=60 \
//!   cargo test --release -p dw-engine --test h06_perf -- --ignored --nocapture
//! ```
//!
//! `DWD_H06_DATADIR` is the unpacked `regtest40k-dw.tar.gz` (git show
//! 042695b9f:ci/gate/fixtures/regtest40k-dw.tar.gz); each run opens a copy.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, HistoryFilter, HistoryQuery,
    HistorySort, NetworkSession, SessionOptions, WalletId,
};
use dw_testutil::perf::{iters, measure, memory_mib, ms, sample};

struct Quiet;

impl EventSink for Quiet {
    fn emit(&self, _: EngineEvent) {}
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

/// A private copy of the fixture datadir, removed on drop.
struct Datadir(tempfile::TempDir);

impl Datadir {
    fn new() -> Option<Self> {
        let Some(src) = std::env::var_os("DWD_H06_DATADIR").map(PathBuf::from) else {
            eprintln!("DWD_H06_DATADIR unset: skipped");
            return None;
        };
        let dir = dw_testutil::private_tempdir();
        copy_dir(&src, dir.path());
        Some(Self(dir))
    }
}

fn open_engine(root: &Path) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: None,
            vault: dw_vault::VaultConfig::default(),
        },
        Arc::new(Quiet),
    )
    .unwrap()
}

/// The fixture's session as the gate opens it: offline, no Platform.
fn open_session(engine: &Engine) -> Arc<NetworkSession> {
    engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["https://127.0.0.1:1443".into()],
                spv_peers: vec!["127.0.0.1:1".into()],
                no_platform: true,
                ..Default::default()
            },
        ))
        .unwrap()
}

fn newest(cursor: Option<String>) -> HistoryQuery {
    HistoryQuery {
        filter: HistoryFilter::default(),
        sort: HistorySort::NewestFirst,
        cursor,
        limit: 200,
    }
}

/// Wallet open, `history_page(200)` (first page, a deep page, a search)
/// and memory after open, on the 40k wallet.
#[test]
#[ignore = "H-06 probe: needs DWD_H06_DATADIR; run in release"]
fn h06_history_and_open_on_the_40k_wallet() {
    let Some(dir) = Datadir::new() else { return };
    let (rss0, pss0) = memory_mib();

    let ((engine, session), open) = sample(|| {
        let engine = open_engine(dir.0.path());
        let session = open_session(&engine);
        (engine, session)
    });
    let wallet: WalletId = session
        .wallet_infos()
        .unwrap()
        .first()
        .expect("the fixture's wallet")
        .wallet_id;
    println!(
        "H06 open: wall {:.0} ms cpu {:.0} ms",
        ms(open.wall),
        ms(open.cpu)
    );

    let first = engine
        .block_on(session.history_page(wallet, newest(None)))
        .unwrap();
    let total = first.total_matching.unwrap_or(0);
    assert_eq!(first.records.len(), 200);
    println!("H06 history: {total} records");

    let n = iters(50);
    let page1 = measure(3, n, || {
        engine
            .block_on(session.history_page(wallet, newest(None)))
            .unwrap();
    });
    println!("H06 history_page(200) first page: {page1}");

    let mut cursor = first.next_cursor.clone();
    for _ in 0..(total as usize / 2 / 200) {
        let p = engine
            .block_on(session.history_page(wallet, newest(cursor)))
            .unwrap();
        cursor = p.next_cursor;
    }
    let deep = cursor.clone();
    let deep_page = measure(3, n, || {
        engine
            .block_on(session.history_page(wallet, newest(deep.clone())))
            .unwrap();
    });
    println!("H06 history_page(200) middle page: {deep_page}");

    let search = HistoryQuery {
        filter: HistoryFilter {
            text: Some("yz".into()),
            ..Default::default()
        },
        ..newest(None)
    };
    let searched = measure(3, n, || {
        engine
            .block_on(session.history_page(wallet, search.clone()))
            .unwrap();
    });
    println!("H06 history_page(200) text search: {searched}");

    std::thread::sleep(Duration::from_secs(5));
    let (rss, pss) = memory_mib();
    println!(
        "H06 memory after open and paging: RSS {rss:.1} MiB PSS {pss:.1} MiB \
         (test process before open: RSS {rss0:.1} PSS {pss0:.1})"
    );
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
}
