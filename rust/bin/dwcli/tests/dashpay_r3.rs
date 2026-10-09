//! Regressions from review DW-E0-09 r3 (Sol), through the `dwcli` binary:
//! an engine whose runtime is stalled after a panic is abandoned within a
//! bound, whatever stdout does, and the passphrase `main` holds is wiped
//! before the forced exit, and before its last lines, whose write may block.
//!
//! The stall uses the debug-build fault hook `DWCLI_FAULT_INJECT`
//! (`task-panic-wedge`: `dashpay status` runs an engine task that panics,
//! then parks every engine worker, so the health probe times out and an
//! engine shutdown would never finish).

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// Longer than the probe's 10 s and the worker parking, far shorter than
/// a hang.
const DEADLINE: Duration = Duration::from_secs(60);

const PASSPHRASE: &str = "dwcli r3 passphrase\n";

struct Run {
    code: Option<i32>,
    elapsed: Duration,
    stdout: String,
    stderr: String,
}

/// Runs `dwcli` with `stdin`, killing it at [`DEADLINE`]. Without
/// `capture`, stdout is `/dev/full`.
fn dwcli(dir: &Path, args: &[&str], fault: Option<&str>, stdin: &[u8], capture: bool) -> Run {
    let stdout = if capture {
        Stdio::piped()
    } else {
        File::options()
            .write(true)
            .open("/dev/full")
            .unwrap()
            .into()
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_dwcli"));
    command
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "2")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(Stdio::piped());
    if let Some(fault) = fault {
        command.env("DWCLI_FAULT_INJECT", fault);
    }
    let start = Instant::now();
    let mut child = command.spawn().unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if start.elapsed() > DEADLINE {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let elapsed = start.elapsed();
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        s.read_to_string(&mut out).unwrap();
    }
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let status = status.unwrap_or_else(|| panic!("no exit within {DEADLINE:?}: {stderr}"));
    Run {
        code: status.code(),
        elapsed,
        stdout: out,
        stderr,
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
    std::fs::write(dir.join("pass"), PASSPHRASE).unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let run = dwcli(dir, args, None, b"", true);
        assert_eq!(run.code, Some(0), "{args:?}: {}", run.stderr);
    }
}

const WIPED: &str = "dwcli: forced exit, passphrase wiped";

/// Sol finding 1 (and 2): a session whose engine stalls after a task panic
/// prints the request's error and `session_poisoned`, runs no later
/// request, and exits 1 without the engine's shutdown; with stdout on
/// `/dev/full` it exits the same way, within the same bound. The
/// passphrase is wiped before either exit.
#[test]
fn a_stalled_engine_is_abandoned_whatever_stdout_does() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let requests = concat!(
        r#"{"args":["dashpay","status"],"id":1}"#,
        "\n",
        r#"{"args":["name","check","alice"],"id":2}"#,
        "\n",
    );
    let fault = Some("task-panic-wedge");
    let args = ["--verbose-events", "dashpay", "session"];

    let run = dwcli(dir.path(), &args, fault, requests.as_bytes(), true);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    let got = lines(&run);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(
        (got[0]["id"].clone(), got[0]["error"]["code"].clone()),
        (json!(1), json!("internal"))
    );
    assert_eq!(got[1]["error"]["code"], "session_poisoned", "{got:?}");
    assert!(run.stderr.contains(WIPED), "{}", run.stderr);
    assert!(!run.stderr.contains("SessionClosed"), "{}", run.stderr);
    assert!(run.elapsed < DEADLINE, "{:?}", run.elapsed);

    let run = dwcli(dir.path(), &args, fault, requests.as_bytes(), false);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert!(run.stderr.contains(WIPED), "{}", run.stderr);
    assert!(!run.stderr.contains("SessionClosed"), "{}", run.stderr);
    assert!(run.elapsed < DEADLINE, "{:?}", run.elapsed);
}

/// The same for a one-shot command: its error line, then exit 1 without
/// the engine's shutdown and with the passphrase wiped, whatever stdout
/// does.
#[test]
fn a_one_shot_command_abandons_a_stalled_engine() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let fault = Some("task-panic-wedge");
    let args = ["--verbose-events", "dashpay", "status"];

    let run = dwcli(dir.path(), &args, fault, b"", true);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    let got = lines(&run);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["error"]["code"], "internal", "{got:?}");
    assert!(run.stderr.contains(WIPED), "{}", run.stderr);
    assert!(!run.stderr.contains("SessionClosed"), "{}", run.stderr);

    let run = dwcli(dir.path(), &args, fault, b"", false);
    assert_eq!(run.code, Some(1), "{}", run.stderr);
    assert!(run.stderr.contains(WIPED), "{}", run.stderr);
    assert!(!run.stderr.contains("SessionClosed"), "{}", run.stderr);
}

/// Runs `dwcli` with stdout on a full pipe nobody reads, and returns its
/// stderr once the wipe marker shows, with the process still blocked on its
/// last lines. Panics if the marker does not show within [`DEADLINE`].
fn wiped_while_blocked(dir: &Path, args: &[&str], stdin: &[u8]) -> String {
    let (reader, mut filler) = std::io::pipe().unwrap();
    let stdout = filler.try_clone().unwrap();
    // Blocks once the pipe is full; ends when the reader closes.
    let fill = std::thread::spawn(move || while filler.write_all(&[b'x'; 4096]).is_ok() {});
    std::thread::sleep(Duration::from_millis(500));
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "2")
        .env("DWCLI_FAULT_INJECT", "task-panic-wedge")
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    let (tx, rx) = std::sync::mpsc::channel();
    let mut stderr = child.stderr.take().unwrap();
    std::thread::spawn(move || {
        let mut buf = [0; 4096];
        while let Ok(n @ 1..) = stderr.read(&mut buf) {
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
        }
    });
    let start = Instant::now();
    let mut text = String::new();
    while !text.contains(WIPED) && start.elapsed() < DEADLINE {
        if let Ok(chunk) = rx.recv_timeout(Duration::from_millis(100)) {
            text.push_str(&chunk);
        }
    }
    let running = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    drop(reader);
    fill.join().unwrap();
    assert!(text.contains(WIPED), "no wipe within {DEADLINE:?}: {text}");
    assert!(running, "{text}");
    text
}

/// Sol r4 finding 2 (DEC-106): a caller that stops reading stdout blocks
/// dwcli's last lines, as any CLI, but the passphrase is wiped before them,
/// in a poisoned session and in a one-shot command.
#[test]
fn the_passphrase_is_wiped_before_a_blocked_write() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path());
    let requests = concat!(
        r#"{"args":["dashpay","status"],"id":1}"#,
        "\n",
        r#"{"args":["name","check","alice"],"id":2}"#,
        "\n",
    );
    wiped_while_blocked(dir.path(), &["dashpay", "session"], requests.as_bytes());
    wiped_while_blocked(dir.path(), &["dashpay", "status"], b"");
}
