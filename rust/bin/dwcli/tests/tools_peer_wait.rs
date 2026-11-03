//! The synced tools wait for a connected peer, not only for the stored
//! heights, which a data root meets before any peer connects. A peer that
//! never connects ends the command at its timeout, with status 1.

use std::process::Command;

#[test]
fn an_unreachable_peer_times_the_sync_out() {
    let dir = dw_testutil::private_tempdir();
    let out = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.path())
        .args(["--network", "regtest", "--no-platform"])
        .args([
            "--dapi",
            "http://127.0.0.1:1",
            "--quorum-url",
            "http://127.0.0.1:1",
        ])
        .args(["--peer", "127.0.0.1:1"])
        .args(["watch-events", "--sync-height", "0"])
        .args(["--timeout-secs", "3", "--linger-secs", "0"])
        .env("TOKIO_WORKER_THREADS", "2")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("error: no peer connected in 3s"),
        "{stderr}"
    );
}
