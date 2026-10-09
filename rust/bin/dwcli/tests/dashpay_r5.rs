//! DEC-110 through the `dwcli` binary: the engine's shutdown runs under a
//! deadline, so an engine that stalls with no panic at all (nothing marks
//! it poisoned, nothing probes it) still ends the process, with status 1,
//! its line written and the passphrase wiped; a healthy engine shuts down
//! as before, well within the deadline.
//!
//! The stall is the debug-build fault hook `DWCLI_FAULT_INJECT=stall`: it
//! parks every engine worker after the DashPay command. Its own process: no
//! other test's panic marks the engine poisoned.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const WIPED: &str = "dwcli: forced exit, passphrase wiped";

fn dwcli(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> (Output, Duration) {
    let start = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "2")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let out = child.wait_with_output().unwrap();
    (out, start.elapsed())
}

fn lines(out: &Output) -> Vec<Value> {
    String::from_utf8(out.stdout.clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?}: {e}")))
        .collect()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// An encrypted vault with one wallet, unlocked by `pass`.
fn vault(dir: &Path) {
    std::fs::write(dir.join("pass"), "dwcli r5 passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let (out, _) = dwcli(dir, args, &[], b"");
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
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

/// A stalled engine misses the shutdown deadline (`--shutdown-timeout`, or
/// `DWCLI_SHUTDOWN_TIMEOUT`): exit 1 soon after it, the command's own line,
/// the passphrase wiped, and no shutdown (no `SessionClosed`).
#[test]
fn a_stalled_engine_misses_the_shutdown_deadline() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let stall = ("DWCLI_FAULT_INJECT", "stall");
    // Parking the workers takes about a second, then 2 s of deadline: far
    // below the 10 s health probe, which no panic triggers here anyway.
    let bound = Duration::from_secs(8);

    let args = [
        "--verbose-events",
        "--shutdown-timeout",
        "2",
        "dashpay",
        "status",
    ];
    let (out, elapsed) = dwcli(dir.path(), &args, &[stall], b"");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(lines(&out), [stub_line()]);
    let err = stderr(&out);
    assert!(
        err.contains("the engine's shutdown missed its 2s deadline"),
        "{err}"
    );
    assert!(err.contains(WIPED), "{err}");
    assert!(!err.contains("SessionClosed"), "{err}");
    assert!(!err.contains("health probe"), "{err}");
    assert!(elapsed < bound, "{elapsed:?}");

    let timeout = ("DWCLI_SHUTDOWN_TIMEOUT", "2");
    let args = ["--verbose-events", "dashpay", "session"];
    let (out, elapsed) = dwcli(dir.path(), &args, &[stall, timeout], b"");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [json!({"ok": true, "result": {"requests": 0}})]
    );
    let err = stderr(&out);
    assert!(err.contains("missed its 2s deadline"), "{err}");
    assert!(err.contains(WIPED), "{err}");
    assert!(!err.contains("SessionClosed"), "{err}");
    assert!(elapsed < bound, "{elapsed:?}");
}

/// A healthy engine shuts down as before: the session closes, the exit
/// status is the command's, and it takes far less than the deadline.
#[test]
fn a_healthy_engine_shuts_down_within_the_deadline() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let bound = Duration::from_secs(10);

    let args = ["--verbose-events", "dashpay", "status"];
    let (out, elapsed) = dwcli(dir.path(), &args, &[], b"");
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert_eq!(lines(&out), [stub_line()]);
    let err = stderr(&out);
    assert!(err.contains("SessionClosed"), "{err}");
    assert!(!err.contains("deadline"), "{err}");
    assert!(elapsed < bound, "{elapsed:?}");

    let args = ["--verbose-events", "dashpay", "session"];
    let (out, elapsed) = dwcli(dir.path(), &args, &[], b"");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        lines(&out),
        [json!({"ok": true, "result": {"requests": 0}})]
    );
    assert!(stderr(&out).contains("SessionClosed"), "{}", stderr(&out));
    assert!(elapsed < bound, "{elapsed:?}");
}
