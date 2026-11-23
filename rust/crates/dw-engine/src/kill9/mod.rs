//! The kill -9 matrix's harness (ROADMAP H-03): a row's child process (this
//! test binary, re-run on one `#[ignore]`d child test) takes an operation to
//! a named point and reports; the parent sends it SIGKILL, opens the same
//! data folder again and checks the state it finds.
//!
//! Every row is opt-in: `cargo test -p dw-engine --lib kill9 -- --ignored`,
//! run by the nightly job. A child test does nothing unless this harness
//! started it.

use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, UnlockScope, VaultConfig};

use crate::{DashNetwork, Engine, EngineConfig, NetworkSession};

const ROOT: &str = "DWD_KILL9_ROOT";
const ROW: &str = "DWD_KILL9_ROW";
const POINT: &str = "DWD_KILL9_POINT";
/// How long a child may take to reach its point, and the parent's state
/// checks to settle (a cold debug build on a loaded machine is slow). The
/// 60 s acceptance is timed on a release build, not asserted here.
pub(crate) const WAIT: Duration = Duration::from_secs(180);

/// What a child process was started for.
pub(crate) struct Child {
    pub(crate) root: PathBuf,
    pub(crate) point: String,
}

/// The child's task if this process is `row`'s child, else `None` (the
/// child test then returns at once).
pub(crate) fn child(row: &str) -> Option<Child> {
    if std::env::var(ROW).ok()? != row {
        return None;
    }
    Some(Child {
        root: std::env::var_os(ROOT)?.into(),
        point: std::env::var(POINT).ok()?,
    })
}

/// An engine on `root` with its regtest session open offline and its vault,
/// encrypted with `pass` (made if there is none: the parent unlocks the
/// child's), unlocked.
pub(crate) fn engine_at(root: &Path, pass: &'static [u8]) -> (Engine, Arc<NetworkSession>) {
    let engine = Engine::new(
        EngineConfig {
            data_root: root.join("data"),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..Default::default()
            },
        },
        Arc::new(NullSink),
    )
    .unwrap();
    let session = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            crate::SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    if engine
        .block_on(session.vault_op(move |v| v.create(Some(pass))))
        .is_err()
    {
        engine
            .block_on(session.vault_op(move |v| v.unlock(pass, UnlockScope::Full)))
            .unwrap();
    }
    (engine, session)
}

struct NullSink;
impl crate::EventSink for NullSink {
    fn emit(&self, _: crate::EngineEvent) {}
}

/// The child's report at its point, written whole (a rename), then the
/// child waits for its SIGKILL.
pub(crate) fn reached(child: &Child, report: &str) -> ! {
    let tmp = child.root.join("reached.tmp");
    std::fs::write(&tmp, report).unwrap();
    std::fs::rename(&tmp, child.root.join("reached")).unwrap();
    loop {
        std::thread::park();
    }
}

/// Runs the child test `test` (its full path in this binary) for `row` up
/// to `point`, kills it with SIGKILL once it reports, and returns its data
/// folder and report.
pub(crate) fn killed_at(test: &str, row: &str, point: &str) -> (tempfile::TempDir, String) {
    let dir = dw_testutil::private_tempdir();
    let log_path = dir.path().join("child.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([test, "--exact", "--ignored", "--test-threads=1"])
        .env(ROOT, dir.path())
        .env(ROW, row)
        .env(POINT, point)
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let reached = dir.path().join("reached");
    let start = Instant::now();
    while !reached.exists() {
        let exited = child.try_wait().unwrap();
        if exited.is_some() || start.elapsed() > WAIT {
            let _ = child.kill();
            let _ = child.wait();
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            panic!("{row} never reached {point} ({exited:?}): {log}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // `Child::kill` is SIGKILL: no destructor, close or flush runs.
    child.kill().unwrap();
    let status = child.wait().unwrap();
    assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    let report = std::fs::read_to_string(&reached).unwrap();
    (dir, report)
}

/// Waits up to [`WAIT`] for `ok`, polling.
pub(crate) fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(start.elapsed() < WAIT, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
