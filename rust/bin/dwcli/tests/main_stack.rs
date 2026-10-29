//! The command line runs on its own thread (`CLI_STACK_SIZE` in main.rs),
//! not on the main thread's stack: Windows gives that 1 MiB, less than a
//! debug build's argument parsing takes since MG-04b slice 1b, and every
//! command overflowed before printing anything. `ulimit -s` limits the
//! main thread's stack only, so here it stands in for Windows' limit.
#![cfg(unix)]

use std::process::Command;

#[test]
fn the_command_line_runs_off_the_main_threads_stack() {
    let out = Command::new("sh")
        .arg("-c")
        .arg(r#"ulimit -s 512 && exec "$0" mn shared --help"#)
        .arg(env!("CARGO_BIN_EXE_dwcli"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("update-share"));
}
