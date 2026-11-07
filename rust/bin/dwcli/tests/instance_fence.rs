//! dwcli takes the data root's instance lock as the desktop app does
//! (dw-fs `instance`): it refuses a data root another process holds, and
//! while it runs, the app's lock on that root is refused.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use dw_fs::instance::{InstanceError, InstanceLock};

fn dwcli(root: &Path, timeout_secs: u32) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_dwcli"));
    cmd.arg("--datadir")
        .arg(root)
        .args(["--network", "regtest", "--no-platform"])
        .args([
            "--dapi",
            "http://127.0.0.1:1",
            "--quorum-url",
            "http://127.0.0.1:1",
        ])
        .args(["--peer", "127.0.0.1:1"])
        .args(["watch-events", "--sync-height", "0"])
        .args([
            "--timeout-secs",
            &timeout_secs.to_string(),
            "--linger-secs",
            "0",
        ])
        .env("TOKIO_WORKER_THREADS", "2");
    cmd
}

#[test]
fn dwcli_refuses_a_data_root_the_app_holds() {
    let dir = dw_testutil::private_tempdir();
    let _held = InstanceLock::acquire(dir.path(), Duration::ZERO).unwrap();
    let out = dwcli(dir.path(), 3).output().unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("is in use by another process"), "{stderr}");
}

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_app_refuses_a_data_root_dwcli_holds() {
    let dir = dw_testutil::private_tempdir();
    let child = dwcli(dir.path(), 60)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut child = Killed(child);
    // dwcli's engine makes the file and locks it at once; probing before
    // that would take the lock from it.
    let file = dir.path().join(InstanceLock::FILE);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !file.exists() {
        assert!(Instant::now() < deadline, "dwcli never opened the root");
        assert!(child.0.try_wait().unwrap().is_none(), "dwcli exited");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.0.try_wait().unwrap().is_none(), "dwcli exited");
    assert!(matches!(
        InstanceLock::acquire(dir.path(), Duration::ZERO),
        Err(InstanceError::Busy)
    ));
    drop(child);
    InstanceLock::acquire(dir.path(), Duration::from_secs(5)).expect("released when dwcli ends");
}
