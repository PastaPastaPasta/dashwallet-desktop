//! `dwcli mn` end to end through the binary (MG-03 D23, D26): one JSON
//! line on stdout whatever fails, and an argument shaped like a private key
//! appears in no output.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// A private key's shape: 64 hex a caller might paste as a ProTx hash or a
/// wallet id.
const SECRET: &str = "e9873d79c6d87dc0fb6a5778633389f4453213303da61f20bd67fc233aa33262";

fn dwcli(dir: &Path, pass: &str, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--no-platform"])
        .args(["--peer", "127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join(pass))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// The single JSON line of a failed command.
fn error(out: &Output) -> (String, String) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout:?}");
    let line: Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(line["ok"], false, "{line}");
    let text = |k: &str| line["error"][k].as_str().unwrap().to_string();
    (text("code"), text("message"))
}

fn reversed(hex_text: &str) -> String {
    let mut bytes = hex::decode(hex_text).unwrap();
    bytes.reverse();
    hex::encode(bytes)
}

#[test]
fn mn_failures_are_json_lines_that_never_quote_the_arguments() {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli mn passphrase\n").unwrap();
    std::fs::write(dir.path().join("wrong"), "not the passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let out = dwcli(dir.path(), "pass", args, b"");
        assert!(out.status.success(), "{args:?}: {out:?}");
    }

    // A wrong passphrase: the m1 `vault` error, in the line.
    let out = dwcli(dir.path(), "wrong", &["mn", "list-state"], b"");
    let (code, message) = error(&out);
    assert_eq!(code, "vault");
    assert!(message.contains("wrong passphrase"), "{message}");
    assert!(!message.contains("not the passphrase"));

    // A setup step: the passphrase file is missing.
    let out = dwcli(dir.path(), "missing", &["mn", "list-state"], b"");
    assert_eq!(error(&out).0, "setup");

    // A refused command line names no value.
    let out = dwcli(
        dir.path(),
        "pass",
        &["mn", "attach", SECRET, "--role", SECRET],
        b"",
    );
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let line: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(line["error"]["code"], "invalid_argument");
    let all = format!("{out:?}").to_lowercase();
    assert!(!all.contains(SECRET), "{all}");

    // No subcommand is a refused command line too; a value `mn` elsewhere
    // doesn't make one an `mn` command line.
    let out = dwcli(dir.path(), "pass", &["mn"], b"");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let line: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(line["error"]["code"], "invalid_argument");
    let out = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .args(["--datadir", "mn", "no-such-command"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(out.stdout.is_empty(), "{out:?}");

    // Every subcommand with the secret where a hash or a wallet id goes
    // (and as the key to attach), on a list that never synced.
    let commands: &[(&[&str], &[u8])] = &[
        (&["mn", "list", "--wallet", SECRET], b""),
        (&["mn", "status", SECRET], b""),
        (&["mn", "status", SECRET, "--wallet", SECRET], b""),
        (&["mn", "find", SECRET], b""),
        (&["mn", "track", SECRET, "--label", "x"], b""),
        (&["mn", "untrack", SECRET], b""),
        (&["mn", "label", SECRET, "--label", "x"], b""),
        (
            &["mn", "attach", SECRET, "--role", "voting"],
            SECRET.as_bytes(),
        ),
        (&["mn", "detach", SECRET, "--role", "owner"], b""),
        (&["mn", "payouts", SECRET], b""),
        (&["mn", "payouts", SECRET, "--wallet", SECRET], b""),
        (&["mn", "credits", SECRET], b""),
        (
            &["mn", "--spv", "--sync-timeout", "1", "status", SECRET],
            b"",
        ),
    ];
    let back = reversed(SECRET);
    for (args, stdin) in commands {
        let out = dwcli(dir.path(), "pass", args, stdin);
        let (code, _) = error(&out);
        assert!(!code.is_empty());
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .to_lowercase();
        for s in [SECRET, back.as_str()] {
            assert!(!all.contains(s), "{args:?} quotes its argument: {all}");
        }
    }
}

/// One `mn list-state` run with `globals` as the whole global option set.
fn dwcli_with(globals: &[std::ffi::OsString], env: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .args(globals)
        .args(["mn", "list-state"])
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

/// stdout and stderr, lowercased.
fn all_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase()
}

/// Every global option with the secret as (part of) its value: one JSON
/// line, and no output (the log included) quotes the value (MG-03 D32,
/// Sol r2 Major 1).
#[test]
fn mn_setup_failures_never_quote_a_global_option() {
    let dir = dw_testutil::private_tempdir();
    let root = dir.path();
    std::fs::write(root.join("pass"), "dwcli mn passphrase\n").unwrap();
    std::fs::write(root.join(SECRET), "not a certificate\n").unwrap();
    std::fs::write(root.join("file"), "a file, not a directory\n").unwrap();
    let path = |p: &Path| p.as_os_str().to_owned();
    let datadir = root.join("data");
    // The options `skip` names are left out (to give them another value).
    let base = |skip: &[&str]| -> Vec<std::ffi::OsString> {
        let mut args: Vec<std::ffi::OsString> = Vec::new();
        let mut push = |name: &str, value: std::ffi::OsString| {
            if !skip.contains(&name) {
                args.push(name.into());
                args.push(value);
            }
        };
        push("--datadir", path(&datadir));
        push("--dapi", "http://127.0.0.1:1".into());
        push("--peer", "127.0.0.1:1".into());
        push("--passphrase-file", path(&root.join("pass")));
        if !skip.contains(&"--no-platform") {
            args.push("--no-platform".into());
        }
        args
    };
    let out = dwcli(root, "pass", &["init-vault"], b"");
    assert!(out.status.success(), "{out:?}");

    let with = |skip: &[&str], extra: &[std::ffi::OsString]| {
        let mut args = base(skip);
        args.extend_from_slice(extra);
        args
    };
    let os = |s: &str| std::ffi::OsString::from(s);
    let cases: Vec<(&str, Vec<std::ffi::OsString>, Option<&str>)> = vec![
        (
            "--peer",
            with(&[], &[os("--peer"), os(SECRET)]),
            Some("invalid --peer value"),
        ),
        (
            "--peer host",
            with(&[], &[os("--peer"), os(&format!("{SECRET}:9999"))]),
            Some("invalid --peer value"),
        ),
        (
            "--ca-cert missing",
            with(
                &[],
                &[os("--ca-cert"), path(&root.join("missing").join(SECRET))],
            ),
            Some("--ca-cert file not readable or not a PEM certificate"),
        ),
        (
            "--ca-cert not PEM",
            with(&[], &[os("--ca-cert"), path(&root.join(SECRET))]),
            Some("--ca-cert file not readable or not a PEM certificate"),
        ),
        (
            "--datadir",
            with(
                &["--datadir"],
                &[os("--datadir"), path(&root.join("file").join(SECRET))],
            ),
            Some("--datadir could not be opened"),
        ),
        (
            "--passphrase-file",
            with(
                &["--passphrase-file"],
                &[os("--passphrase-file"), path(&root.join(SECRET))],
            ),
            None,
        ),
        (
            "--passphrase-file missing",
            with(
                &["--passphrase-file"],
                &[
                    os("--passphrase-file"),
                    path(&root.join("missing").join(SECRET)),
                ],
            ),
            Some("--passphrase-file not readable"),
        ),
        (
            "--dapi",
            with(&["--dapi"], &[os("--dapi"), os(SECRET)]),
            None,
        ),
        (
            "--quorum-url",
            with(&[], &[os("--quorum-url"), os(SECRET)]),
            None,
        ),
        // With Platform on, so the SDK uses (and may log) the URL.
        (
            "--dapi URL",
            with(
                &["--dapi", "--no-platform"],
                &[os("--dapi"), os(&format!("http://127.0.0.1:1/{SECRET}"))],
            ),
            None,
        ),
        (
            "--quorum-url URL",
            with(
                &["--no-platform"],
                &[
                    os("--quorum-url"),
                    os(&format!("http://127.0.0.1:1/{SECRET}")),
                ],
            ),
            None,
        ),
        ("--network", with(&[], &[os("--network"), os(SECRET)]), None),
        (
            "--network devnet",
            with(&[], &[os("--network"), os(&format!("devnet:{SECRET}"))]),
            None,
        ),
        (
            "--initial-protocol-version",
            with(&[], &[os("--initial-protocol-version"), os(SECRET)]),
            None,
        ),
        (
            "--shutdown-timeout",
            with(&[], &[os("--shutdown-timeout"), os(SECRET)]),
            None,
        ),
    ];
    let back = reversed(SECRET);
    for (name, args, fixed) in cases {
        // With the log on: it may not quote the value either.
        let out = dwcli_with(&args, &[("DWCLI_LOG", "info")]);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let lines: Vec<_> = stdout.lines().collect();
        assert_eq!(lines.len(), 1, "{name}: {out:?}");
        let line: Value = serde_json::from_str(lines[0]).unwrap();
        if let Some(fixed) = fixed {
            assert_eq!(line["ok"], false, "{name}: {line}");
            assert_eq!(line["error"]["message"], fixed, "{name}");
        }
        let all = all_output(&out);
        for s in [SECRET, back.as_str()] {
            assert!(!all.contains(s), "{name} quotes its value: {all}");
        }
    }
    // The `mn` options of the SPV wait, refused by clap or by the wait.
    for args in [
        &["mn", "--sync-timeout", SECRET, "list-state"][..],
        &["mn", "--sync-height", SECRET, "list-state"],
        &[
            "mn",
            "--sync-height",
            "7",
            "--sync-timeout",
            "1",
            "list-state",
        ],
    ] {
        let out = dwcli(root, "pass", args, b"");
        let lines = String::from_utf8_lossy(&out.stdout).lines().count();
        assert_eq!(lines, 1, "{args:?}: {out:?}");
        let all = all_output(&out);
        assert!(!all.contains(SECRET), "{args:?}: {all}");
    }
}

/// The teardown runs before the line: one that fails prints the error line
/// instead of `ok: true`, and the process fails (MG-03 D32, Sol r2 Low 4).
#[test]
#[cfg(debug_assertions)]
fn mn_a_failed_teardown_prints_the_error_line_not_ok() {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli mn passphrase\n").unwrap();
    let out = dwcli(dir.path(), "pass", &["init-vault"], b"");
    assert!(out.status.success(), "{out:?}");
    let ok = dwcli(dir.path(), "pass", &["mn", "list-state"], b"");
    assert!(ok.status.success(), "{ok:?}");

    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.path().join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--no-platform"])
        .args(["--peer", "127.0.0.1:1", "--passphrase-file"])
        .arg(dir.path().join("pass"))
        .args(["mn", "list-state"])
        .env("DWCLI_FAULT_INJECT", "probe-spawn-fail")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = {
        child.stdin.take();
        child.wait_with_output().unwrap()
    };
    let (code, _) = error(&out);
    assert_eq!(code, "shutdown", "{out:?}");
}
