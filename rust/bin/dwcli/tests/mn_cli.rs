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
