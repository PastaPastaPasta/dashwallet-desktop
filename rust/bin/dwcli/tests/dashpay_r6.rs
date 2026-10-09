//! DEC-118 through the `dwcli` binary: a teardown thread that cannot start
//! counts as a missed deadline. A stalled engine whose shutdown thread is
//! refused is abandoned at once, far within its deadline, with status 1,
//! the command's line, the abandonment on stderr and the passphrase wiped;
//! the engine is never dropped on the caller, whose drop would wait for the
//! wedged workers forever.
//!
//! The debug-build fault hook `DWCLI_FAULT_INJECT=stall-spawn-fail` parks
//! every engine worker after the DashPay command (`stall`), then starts no
//! teardown thread, and on Linux lowers `RLIMIT_NPROC` so that the engine
//! drop's own thread is refused too, as with an OS out of threads. With
//! `--spv` the first refused thread is SPV's stop; nothing starts after it
//! (Sol r7 R7-1: the stall's own thread was refused and panicked, and the
//! engine unwound on the caller).

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The shutdown deadline the runs set: the abandonment must come far
/// sooner.
const DEADLINE_SECS: u64 = 300;

/// Covers vault, engine and SPV start-up and the worker parking under a
/// loaded machine; a run still going then is killed.
const BOUND: Duration = Duration::from_secs(120);

const WIPED: &str = "dwcli: forced exit, passphrase wiped";

struct Run {
    code: Option<i32>,
    elapsed: Duration,
    stdout: String,
    stderr: String,
}

/// Runs `dwcli`, killing it after [`BOUND`] (its `code` is then `None`).
fn dwcli(dir: &Path, args: &[&str], fault: Option<&str>, stdin: &[u8]) -> Run {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dwcli"));
    command
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "2")
        .env_remove("DWCLI_LOG")
        .env_remove("DWCLI_FAULT_INJECT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(fault) = fault {
        command.env("DWCLI_FAULT_INJECT", fault);
    }
    let start = Instant::now();
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    // Drained as they come: a full pipe must not stall the run.
    let mut stdout = child.stdout.take().unwrap();
    let out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let mut stderr = child.stderr.take().unwrap();
    let err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > BOUND {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Run {
        code: status.and_then(|s| s.code()),
        elapsed: start.elapsed(),
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

fn lines(run: &Run) -> Vec<Value> {
    run.stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?}: {e}")))
        .collect()
}

/// An encrypted vault with one wallet, unlocked by `pass`.
fn vault(dir: &Path) {
    std::fs::write(dir.join("pass"), "dwcli r6 passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let run = dwcli(dir, args, None, b"");
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
    }
}

/// Exit 1 well within the deadline, `step`'s refused thread as the
/// abandonment on stderr, the passphrase wiped, no panic and no shutdown
/// (no `SessionClosed`).
fn assert_abandoned(run: &Run, step: &str) {
    assert_eq!(run.code, Some(1), "{:?}: {}", run.elapsed, run.stderr);
    let err = &run.stderr;
    assert!(
        err.contains(&format!(
            "error: {step} could not start its thread ({}), which counts as missing its \
             {DEADLINE_SECS}s deadline; exiting without the engine's shutdown",
            std::io::Error::from(std::io::ErrorKind::WouldBlock)
        )),
        "{err}"
    );
    assert!(err.contains(WIPED), "{err}");
    assert!(!err.contains("SessionClosed"), "{err}");
    assert!(!err.contains("health probe"), "{err}");
    assert!(!err.contains("dwcli: panic at"), "{err}");
    assert!(run.elapsed < BOUND, "{:?}", run.elapsed);
}

/// The one-shot line `dashpay status` prints while its body is a stub.
fn stub_line() -> Value {
    json!({
        "ok": false,
        "error": {
            "code": "platform.not_implemented",
            "message": "not implemented: DashPay.status",
            "params": {"call": "DashPay.status"},
        },
    })
}

/// Sol r6 finding 1: a stalled, unpoisoned engine whose shutdown thread
/// cannot start, after a one-shot command and after a session.
#[test]
fn a_refused_shutdown_thread_abandons_the_engine_at_once() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let fault = Some("stall-spawn-fail");
    let timeout = DEADLINE_SECS.to_string();

    let args = [
        "--verbose-events",
        "--shutdown-timeout",
        &timeout,
        "dashpay",
        "status",
    ];
    let run = dwcli(dir.path(), &args, fault, b"");
    assert_abandoned(&run, "the engine's shutdown");
    assert_eq!(lines(&run), [stub_line()]);

    let args = [
        "--verbose-events",
        "--shutdown-timeout",
        &timeout,
        "dashpay",
        "session",
    ];
    let run = dwcli(dir.path(), &args, fault, b"");
    assert_abandoned(&run, "the engine's shutdown");
    assert_eq!(
        lines(&run),
        [json!({"ok": true, "result": {"requests": 0}})]
    );
}

/// Sol r7 R7-1: with `--spv` SPV's stop is refused its thread first; the
/// stall then starts none, and the engine is abandoned with the command's
/// line, or the session's request answer and its summary, intact.
#[test]
fn a_refused_spv_stop_thread_abandons_the_engine_at_once() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let fault = Some("stall-spawn-fail");
    let timeout = DEADLINE_SECS.to_string();

    let args = [
        "--verbose-events",
        "--shutdown-timeout",
        &timeout,
        "dashpay",
        "status",
        "--spv",
    ];
    let run = dwcli(dir.path(), &args, fault, b"");
    assert_abandoned(&run, "stopping SPV");
    assert_eq!(lines(&run), [stub_line()]);

    let args = [
        "--verbose-events",
        "--shutdown-timeout",
        &timeout,
        "dashpay",
        "session",
        "--spv",
    ];
    let request = concat!(r#"{"args":["dashpay","status"],"id":"r7-spv-stall"}"#, "\n");
    let run = dwcli(dir.path(), &args, fault, request.as_bytes());
    assert_abandoned(&run, "stopping SPV");
    let mut answer = stub_line();
    answer["id"] = json!("r7-spv-stall");
    assert_eq!(
        lines(&run),
        [answer, json!({"ok": true, "result": {"requests": 1}})]
    );
}
