//! Regressions from the Opus-high review of E0-09, through the `dwcli`
//! binary:
//!
//! - F1 (DEC-106): the passphrase is wiped before the last JSON line, whose
//!   write blocks while the caller does not read stdout, on the healthy
//!   path too: none of its bytes are left in the process's memory then
//!   (Linux, read through `/proc/<pid>/mem`).
//! - F2: a panic on dwcli's own thread before the engine's shutdown still
//!   ends the engine through the teardown (debug-build fault hook
//!   `panic-spawn-fail`: `dashpay status` panics, then no teardown thread
//!   starts, so the engine is abandoned), never by unwinding on that
//!   thread.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

/// Covers vault and engine start-up under a loaded machine; a run still
/// going then is killed.
const BOUND: Duration = Duration::from_secs(120);

const WIPED: &str = "dwcli: forced exit, passphrase wiped";

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_dwcli"));
    command
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .env("TOKIO_WORKER_THREADS", "2")
        .env_remove("DWCLI_LOG")
        .env_remove("DWCLI_FAULT_INJECT");
    command
}

/// An encrypted vault with one wallet, unlocked by `pass`.
fn vault(dir: &Path, passphrase: &str) {
    std::fs::write(dir.join("pass"), format!("{passphrase}\n")).unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let out = command(dir, args).stdin(Stdio::null()).output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
}

/// F2: the panic's line, then the engine abandoned through the teardown
/// (its thread refused), with the passphrase wiped.
#[test]
fn a_panic_before_the_shutdown_still_ends_the_engine_through_teardown() {
    let dir = dw_testutil::private_tempdir();
    vault(dir.path(), "dwcli final passphrase");
    let mut child = command(dir.path(), &["dashpay", "status"])
        .env("DWCLI_FAULT_INJECT", "panic-spawn-fail")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
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
    let start = Instant::now();
    let code = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status.code();
        }
        if start.elapsed() > BOUND {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let (out, err) = (out.join().unwrap(), err.join().unwrap());
    assert_eq!(code, Some(1), "{err}");
    let lines: Vec<Value> = out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{out}");
    assert_eq!(lines[0]["error"]["code"], "internal", "{out}");
    assert!(
        lines[0]["error"]["params"]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("outcome is unknown")),
        "{out}"
    );
    assert!(
        err.contains(&format!(
            "error: the engine's shutdown could not start its thread ({}), which counts as \
             missing its 10s deadline; exiting without the engine's shutdown",
            std::io::Error::from(std::io::ErrorKind::WouldBlock)
        )),
        "{err}"
    );
    assert!(err.contains(WIPED), "{err}");
    assert!(!err.contains("SessionClosed"), "{err}");
    assert!(!out.contains("PANIC-SECRET") && !err.contains("PANIC-SECRET"));
}

#[cfg(target_os = "linux")]
mod blocked_last_line {
    use std::fs::File;
    use std::io::{ErrorKind, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use super::{BOUND, command, vault};

    /// A pipe whose write end is full, with its read end, which nobody
    /// reads.
    fn full_pipe() -> (std::io::PipeReader, std::io::PipeWriter) {
        let (reader, mut writer) = std::io::pipe().unwrap();
        let fd = writer.as_raw_fd();
        // SAFETY: fcntl on a pipe fd this function owns.
        let set = |flags: libc::c_int| unsafe { libc::fcntl(fd, libc::F_SETFL, flags) };
        // SAFETY: as above.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(set(flags | libc::O_NONBLOCK), 0);
        for chunk in [4096, 1] {
            loop {
                match writer.write(&vec![b'x'; chunk]) {
                    Ok(_) => {}
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => panic!("{e}"),
                }
            }
        }
        // The flag belongs to the open pipe, which dwcli shares: blocking
        // again.
        assert_eq!(set(flags), 0);
        (reader, writer)
    }

    /// Whether the process's main thread is blocked writing to stdout.
    fn writing_stdout(pid: u32) -> bool {
        let Ok(syscall) = std::fs::read_to_string(format!("/proc/{pid}/syscall")) else {
            return false;
        };
        let mut fields = syscall.split_whitespace();
        fields.next() == Some(&libc::SYS_write.to_string()) && fields.next() == Some("0x1")
    }

    /// How many times each needle occurs in the process's anonymous memory
    /// (heap, stacks, anonymous mappings).
    fn count_in_memory(pid: u32, needles: &[&[u8]]) -> Vec<usize> {
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
        let mem = File::open(format!("/proc/{pid}/mem")).unwrap();
        let overlap = needles.iter().map(|n| n.len()).max().unwrap() - 1;
        let mut counts = vec![0; needles.len()];
        for line in maps.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            let path = fields.get(5).copied().unwrap_or("");
            if !fields[1].starts_with('r') || !(path.is_empty() || path.starts_with('[')) {
                continue;
            }
            let (start, end) = fields[0].split_once('-').unwrap();
            let start = u64::from_str_radix(start, 16).unwrap();
            let end = u64::from_str_radix(end, 16).unwrap();
            let mut at = start;
            let mut carry = Vec::new();
            while at < end {
                let mut buf = vec![0; (end - at).min(1 << 20) as usize];
                let Ok(n @ 1..) = mem.read_at(&mut buf, at) else {
                    break;
                };
                at += n as u64;
                let mut window = std::mem::take(&mut carry);
                window.extend_from_slice(&buf[..n]);
                for (needle, count) in needles.iter().zip(&mut counts) {
                    *count += window.windows(needle.len()).filter(|w| w == needle).count();
                }
                // The tail, shorter than any needle, joins the next chunk.
                carry = window[window.len().saturating_sub(overlap)..].to_vec();
            }
        }
        counts
    }

    /// Runs `args` with stdout a full pipe nobody reads, waits until dwcli
    /// blocks writing its last line, and returns how many copies of the
    /// passphrase and of the data directory's path its memory holds then.
    fn blocked_counts(dir: &std::path::Path, passphrase: &str, args: &[&str]) -> (usize, usize) {
        let (reader, writer) = full_pipe();
        let mut child = command(dir, args)
            .stdin(Stdio::null())
            .stdout(writer)
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let start = Instant::now();
        let mut blocked = 0;
        while blocked < 3 {
            assert!(child.try_wait().unwrap().is_none(), "{args:?} exited");
            assert!(start.elapsed() < BOUND, "{args:?} never blocked");
            blocked = if writing_stdout(pid) { blocked + 1 } else { 0 };
            std::thread::sleep(Duration::from_millis(200));
        }
        let data = dir.join("data");
        let counts = count_in_memory(
            pid,
            &[passphrase.as_bytes(), data.to_str().unwrap().as_bytes()],
        );
        child.kill().unwrap();
        child.wait().unwrap();
        drop(reader);
        (counts[0], counts[1])
    }

    /// F1: a one-shot command and a session (its summary) block on their
    /// last line with no copy of the passphrase left; the data directory's
    /// path, which dwcli keeps, shows the scan reads the right memory.
    #[test]
    fn the_passphrase_is_wiped_before_a_blocked_last_line() {
        let dir = dw_testutil::private_tempdir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let passphrase = format!("dwcli-f1-{}-{nanos:08x}", std::process::id());
        vault(dir.path(), &passphrase);
        for args in [&["dashpay", "status"][..], &["dashpay", "session"]] {
            let (copies, control) = blocked_counts(dir.path(), &passphrase, args);
            assert!(control > 0, "{args:?}: the scan found nothing");
            assert_eq!(copies, 0, "{args:?}: passphrase copies in memory");
        }
    }
}
