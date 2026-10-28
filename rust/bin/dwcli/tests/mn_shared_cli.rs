//! `dwcli mn shared …` end to end through the binary (MG-04b slice 1a): one
//! JSON line on stdout whatever fails, the facade's codes in it, and neither
//! a pasted message nor an argument in any output. The offline cases: a
//! datadir without a wallet, and a fresh wallet with no session and no v24
//! sighting. The session flows are the engine's three-wallet test.
#![allow(non_snake_case)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// A private key's shape, where a session id, a hash or a message goes.
const SECRET: &str = "e9873d79c6d87dc0fb6a5778633389f4453213303da61f20bd67fc233aa33262";
/// A well-formed session id no wallet holds.
const SESSION: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn dwcli(dir: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--no-platform"])
        .args(["--peer", "127.0.0.1:1", "--passphrase-file"])
        .arg(dir.join("pass"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    child.wait_with_output().unwrap()
}

/// A datadir with a vault, and a wallet if `wallet`.
fn datadir(wallet: bool) -> impl AsRef<Path> {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli mn shared passphrase\n").unwrap();
    let out = dwcli(dir.as_ref(), &["init-vault"], b"");
    assert!(out.status.success(), "{out:?}");
    if wallet {
        let out = dwcli(dir.as_ref(), &["create"], b"");
        assert!(out.status.success(), "{out:?}");
    }
    dir
}

/// The one JSON line on stdout.
fn line(out: &Output) -> Value {
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout:?}");
    serde_json::from_str(lines[0]).unwrap()
}

/// The result of a command that succeeded.
fn result(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    let line = line(out);
    assert_eq!(line["ok"], true, "{line}");
    line["result"].clone()
}

/// The code of a command that failed with status 1.
fn code(out: &Output) -> String {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let line = line(out);
    assert_eq!(line["ok"], false, "{line}");
    line["error"]["code"].as_str().unwrap().to_string()
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

/// `mn shared start` with the secret in the places it could be quoted.
fn start_args<'a>(shares: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["mn", "shared", "start"];
    for s in shares {
        args.extend(["--share", s]);
    }
    args.extend([
        "--service",
        "1.2.3.4:19999",
        "--early-period-blocks",
        "720",
        "--early-penalty",
        "5",
        "--fee",
        "1000",
    ]);
    args
}

#[test]
fn test_QT_126_shared_commands_on_a_datadir_without_a_wallet_report_wallet_not_found() {
    let dir = datadir(false);
    let secret_share = format!("{SECRET}:10000000000000");
    let other_share = "other:10000000000000";
    let commands: Vec<(Vec<&str>, &[u8])> = vec![
        (vec!["mn", "shared", "capability"], b""),
        (vec!["mn", "shared", "sessions"], b""),
        (vec!["mn", "shared", "show", SESSION], b""),
        (start_args(&[&secret_share, other_share]), b""),
        (vec!["mn", "shared", "fee", SESSION, "1000"], b""),
        (vec!["mn", "shared", "import"], SECRET.as_bytes()),
        (
            vec!["mn", "shared", "import", "--target", SESSION],
            SECRET.as_bytes(),
        ),
        (vec!["mn", "shared", "join", SESSION], b""),
        (vec!["mn", "shared", "message", SESSION], b""),
        (
            vec!["mn", "shared", "lock", SESSION, "--revision", "3"],
            b"",
        ),
        (vec!["mn", "shared", "unlock", SESSION], b""),
        (
            vec!["mn", "shared", "approve", SESSION, "--consent-hash", SECRET],
            b"",
        ),
        (vec!["mn", "shared", "sign", SESSION], b""),
        (vec!["mn", "shared", "send", SESSION], b""),
        (vec!["mn", "shared", "abandon", SESSION], b""),
        (vec!["mn", "shared", "discard", SESSION], b""),
        (
            vec![
                "mn",
                "shared",
                "preview-update-share",
                SECRET,
                "--share-index",
                "0",
                "--reward",
                SECRET,
            ],
            b"",
        ),
        (
            vec![
                "mn",
                "shared",
                "update-share",
                SECRET,
                "--share-index",
                "0",
                "--reward",
                SECRET,
            ],
            b"",
        ),
    ];
    for (args, stdin) in commands {
        let out = dwcli(dir.as_ref(), &args, stdin);
        assert_eq!(code(&out), "wallet_not_found", "{args:?}");
        assert!(!all_output(&out).contains(SECRET), "{args:?} quotes input");
    }
}

#[test]
fn test_QT_126_shared_capability_on_a_fresh_wallet() {
    let dir = datadir(true);
    let capability = result(&dwcli(dir.as_ref(), &["mn", "shared", "capability"], b""));
    assert_eq!(capability["can_send"], true, "{capability}");
    assert_eq!(capability["v24"], "unknown", "{capability}");
    assert_eq!(capability["evonode_shares"], "inactive", "{capability}");

    let sessions = result(&dwcli(dir.as_ref(), &["mn", "shared", "sessions"], b""));
    assert_eq!(sessions, serde_json::json!([]));
}

#[test]
fn test_QT_126_shared_refusals_are_the_facades_codes_and_quote_nothing() {
    let dir = datadir(true);
    let path = dir.as_ref();
    let run = |args: &[&str], stdin: &[u8]| {
        let out = dwcli(path, args, stdin);
        let all = all_output(&out);
        assert!(!all.contains(SECRET), "{args:?} quotes its input: {all}");
        out
    };

    // A session id that isn't 64 lower-case hex, and one nobody holds.
    for command in ["show", "message", "unlock", "discard"] {
        let out = run(
            &["mn", "shared", command, SECRET.to_uppercase().as_str()],
            b"",
        );
        assert_eq!(code(&out), "invalid_argument", "{command}");
        let out = run(&["mn", "shared", command, SESSION], b"");
        assert_eq!(
            code(&out),
            "masternodes.shared_session_not_found",
            "{command}"
        );
    }
    let out = run(&["mn", "shared", "fee", SESSION, "1000"], b"");
    assert_eq!(code(&out), "masternodes.shared_session_not_found");
    let out = run(&["mn", "shared", "fee", SESSION, "0"], b"");
    assert_eq!(code(&out), "invalid_argument");

    // The creation calls need v24 to be active, which a fresh wallet hasn't
    // seen; the grant they take is capped at 0 and never charged.
    for args in [
        &["lock", SESSION, "--revision", "1"][..],
        &["approve", SESSION, "--consent-hash", SECRET],
        &["join", SESSION],
    ] {
        let out = run(&[&["mn", "shared"][..], args].concat(), b"");
        assert_eq!(code(&out), "masternodes.unsupported", "{args:?}");
    }
    let shares = ["me:10000000000000", "you:10000000000000"];
    let out = run(&start_args(&shares), b"");
    assert_eq!(code(&out), "masternodes.unsupported");
    // The request is checked before that.
    let out = run(&start_args(&shares[..1]), b"");
    assert_eq!(code(&out), "invalid_argument");
    let out = run(
        &[
            "mn",
            "shared",
            "preview-update-share",
            SECRET,
            "--share-index",
            "0",
            "--reward",
            "refund",
        ],
        b"",
    );
    assert_eq!(code(&out), "masternodes.unsupported");
    let out = run(
        &[
            "mn",
            "shared",
            "preview-update-share",
            "nothex",
            "--share-index",
            "0",
            "--reward",
            "refund",
        ],
        b"",
    );
    assert_eq!(code(&out), "invalid_argument");

    // `--out` writes nothing when the call fails.
    let file = path.join("message.json");
    let file_arg = file.to_str().unwrap();
    let out = run(
        &["mn", "shared", "message", SESSION, "--out", file_arg],
        b"",
    );
    assert_eq!(code(&out), "masternodes.shared_session_not_found");
    assert!(!file.exists());
}

#[test]
fn test_QT_126_shared_import_reads_stdin_and_refuses_without_echo() {
    let dir = datadir(true);
    let path = dir.as_ref();
    // Text that isn't a session message, shaped like a secret and like JSON
    // quoting it.
    let json = format!("{{\"sessionId\": \"{SECRET}\", \"stage\": \"{SECRET}\"}}");
    for stdin in [
        SECRET.as_bytes(),
        json.as_bytes(),
        b"",
        // Not UTF-8 (MG-04b D34).
        &[0xff, 0xfe, 0x00, b'{'][..],
    ] {
        for target in [None, Some(SESSION)] {
            let mut args = vec!["mn", "shared", "import"];
            args.extend(target.iter().flat_map(|t| ["--target", t]));
            let out = dwcli(path, &args, stdin);
            assert_eq!(code(&out), "masternodes.shared_message", "{args:?}");
            assert!(
                !all_output(&out).contains(SECRET),
                "{args:?} quotes the text"
            );
        }
    }
    // The text is stdin's: an argument is not a message.
    let out = dwcli(path, &["mn", "shared", "import", SECRET], b"");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(line(&out)["error"]["code"], "invalid_argument");
    assert!(!all_output(&out).contains(SECRET));
}

#[test]
fn test_QT_126_shared_command_lines_that_are_refused_name_no_value() {
    let dir = datadir(false);
    let path = dir.as_ref();
    let refused: &[&[&str]] = &[
        &["mn", "shared"],
        &["mn", "shared", "show"],
        &["mn", "shared", "fee", SESSION, SECRET],
        &["mn", "shared", "start", "--share", SECRET],
        &["mn", "shared", "join", SESSION, "--share-index", SECRET],
        &[
            "mn",
            "shared",
            "preview-update-share",
            SECRET,
            "--share-index",
            SECRET,
        ],
        // Slice 1b's commands: a bad fee, a missing share or reward.
        &["mn", "shared", "sign"],
        &["mn", "shared", "send"],
        &["mn", "shared", "abandon", SESSION, "--max-fee", SECRET],
        &["mn", "shared", "update-share", SECRET, "--share-index", "0"],
        &["mn", "shared", "update-share", SECRET, "--reward", "refund"],
    ];
    for args in refused {
        let out = dwcli(path, args, b"");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert_eq!(line(&out)["error"]["code"], "invalid_argument", "{args:?}");
        assert!(!all_output(&out).contains(SECRET), "{args:?}");
    }
}
