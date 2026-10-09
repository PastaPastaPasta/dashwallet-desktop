//! Regressions from review DW-E0-09 r1 (GPT), through the `dwcli` binary:
//! session framing at the line limit, the `--no-platform` refusal, the
//! panic boundary, and clap errors that must not echo a bearer input.
//!
//! The panic cases use the debug-build fault hook `DWCLI_FAULT_INJECT`
//! (`panic`: `dashpay status` panics; `panic-poison`: it panics and the
//! session's health probe finds the engine poisoned).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

/// The session's line limit (content bytes, without the line ending).
const MAX_LINE: usize = 64 * 1024;

fn dwcli(dir: &Path, args: &[&str], env: &[(&str, &str)], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1"])
        .args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A child that refuses up front may not read stdin at all.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    child.wait_with_output().unwrap()
}

fn lines(out: &Output) -> Vec<Value> {
    String::from_utf8(out.stdout.clone())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l:?}: {e}")))
        .collect()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const NEXT: &[u8] = br#"{"args":["name","check","alice"],"id":2}"#;

/// GPT finding 2: a line exactly at the read boundary swallowed the next
/// request. Content lengths around the limit, each ending in LF, CRLF or
/// EOF; with a line ending, the next request must still be answered.
#[test]
fn session_framing_at_the_line_limit() {
    let dir = dw_testutil::private_tempdir();
    for len in [MAX_LINE - 1, MAX_LINE, MAX_LINE + 1] {
        for end in [&b"\n"[..], b"\r\n", b""] {
            let mut stdin = vec![b'x'; len];
            stdin.extend_from_slice(end);
            if !end.is_empty() {
                stdin.extend_from_slice(NEXT);
                stdin.push(b'\n');
            }
            let out = dwcli(dir.path(), &["dashpay", "session"], &[], &stdin);
            let case = format!("len {len} end {end:?}");
            assert!(out.status.success(), "{case}: {out:?}");
            let got = lines(&out);
            let n = if end.is_empty() { 1 } else { 2 };
            assert_eq!(got.len(), n + 1, "{case}: {got:?}");
            let detail = got[0]["error"]["params"]["detail"].as_str().unwrap();
            if len > MAX_LINE {
                assert_eq!(
                    detail,
                    format!("request line over {MAX_LINE} bytes"),
                    "{case}"
                );
            } else {
                assert!(detail.starts_with("malformed request"), "{case}: {detail}");
            }
            if n == 2 {
                assert_eq!(got[1]["id"], 2, "{case}");
                assert_eq!(
                    got[1]["error"]["params"]["call"], "check_username",
                    "{case}"
                );
            }
            assert_eq!(
                got[n],
                json!({"ok": true, "result": {"requests": n}}),
                "{case}"
            );
        }
    }
}

/// GPT finding 3: `--no-platform` refuses every DashPay command up front,
/// before the network opens (the data root here cannot be created), in
/// one-shot and session mode.
#[test]
fn no_platform_refuses_dashpay_commands() {
    let dir = dw_testutil::private_tempdir();
    let blocked = dir.path().join("file");
    std::fs::write(&blocked, b"").unwrap();
    for args in [&["dashpay", "status"][..], &["dashpay", "session"]] {
        let out = Command::new(env!("CARGO_BIN_EXE_dwcli"))
            .arg("--datadir")
            .arg(blocked.join("data"))
            .args(["--dapi", "http://127.0.0.1:1", "--no-platform"])
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
        assert_eq!(
            lines(&out),
            [json!({
                "ok": false,
                "error": {
                    "code": "platform.feature_off",
                    "message": "feature off: platform",
                    "params": {"feature": "platform"},
                },
            })],
            "{args:?}"
        );
    }
    // Other commands still run.
    let out = dwcli(dir.path(), &["--no-platform", "list"], &[], b"");
    assert!(out.status.success(), "{out:?}");
}

const PANIC_SECRET: &str = "PANIC-SECRET";

/// GPT finding 4, one-shot: a panic is one sanitized JSON line and a
/// nonzero exit, and its payload reaches neither stream.
#[test]
fn a_one_shot_panic_is_one_json_line() {
    let dir = dw_testutil::private_tempdir();
    let fault = [("DWCLI_FAULT_INJECT", "panic")];
    let out = dwcli(dir.path(), &["dashpay", "status"], &fault, b"");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["error"]["code"], "internal");
    assert!(
        got[0]["error"]["params"]["detail"]
            .as_str()
            .unwrap()
            .contains("outcome is unknown"),
        "{got:?}"
    );
    assert!(!text(&out).contains(PANIC_SECRET), "{}", text(&out));
}

/// GPT finding 4, session: the panicking request gets an error answer and
/// the session goes on; a poisoned engine ends it cleanly, saying so.
#[test]
fn a_session_survives_a_panic_unless_poisoned() {
    let dir = dw_testutil::private_tempdir();
    let requests = concat!(
        r#"{"args":["dashpay","status"],"id":1}"#,
        "\n",
        r#"{"args":["name","check","alice"],"id":2}"#,
        "\n",
    );
    let fault = [("DWCLI_FAULT_INJECT", "panic")];
    let out = dwcli(
        dir.path(),
        &["dashpay", "session"],
        &fault,
        requests.as_bytes(),
    );
    assert!(out.status.success(), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(
        (got[0]["id"].clone(), got[0]["error"]["code"].clone()),
        (json!(1), json!("internal"))
    );
    assert_eq!(got[1]["id"], 2);
    assert_eq!(got[1]["error"]["params"]["call"], "check_username");
    assert_eq!(got[2], json!({"ok": true, "result": {"requests": 2}}));
    assert!(!text(&out).contains(PANIC_SECRET), "{}", text(&out));

    let fault = [("DWCLI_FAULT_INJECT", "panic-poison")];
    let out = dwcli(
        dir.path(),
        &["dashpay", "session"],
        &fault,
        requests.as_bytes(),
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(
        (got[0]["id"].clone(), got[0]["error"]["code"].clone()),
        (json!(1), json!("internal"))
    );
    assert_eq!(got[1]["error"]["code"], "session_poisoned", "{got:?}");
    assert!(!text(&out).contains(PANIC_SECRET), "{}", text(&out));
}

/// GPT finding 5: clap must not echo a refused argument. Every
/// bearer-shaped argument in every position clap can refuse it.
#[test]
fn refused_arguments_are_not_echoed() {
    let dir = dw_testutil::private_tempdir();
    let link = "dashpay://invite?du=alice&pk=ARGV-SECRET";
    let dapk = "dash:?du=alice&dapk=ARGV-SECRET";
    let cases: &[&[&str]] = &[
        &["invite", "stash", link],
        &["invite", "claim", "--label", "a", link],
        &["invite", "stash", "--", link],
        &["invite", "stash", "--link", link],
        &["invite", "stash", &format!("--link={link}")],
        &["contact", "scan", dapk],
        &["contact", "request", "--scanned", dapk],
        &["contact", "request", "C", dapk],
        &["contact", "list", "--sort", dapk],
        &["contact", "activity", "C", "--filter", dapk],
        &["pay-contact", "C", "--amount", dapk],
        &["identity", "withdraw", "--credits", dapk, "--to", "yA"],
        &[
            "identity",
            "register",
            "--label",
            "a",
            "--invitation-id",
            "L",
            "--existing-identity",
            dapk,
        ],
        &["profile", "avatar", "I", "--size", dapk],
        &["dashpay", "session", link],
        &["dashpay", "mark-read", dapk],
        &[link],
        &["--network", link, "dashpay", "status"],
    ];
    for args in cases {
        let out = dwcli(dir.path(), args, &[], b"");
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert!(
            !text(&out).contains("ARGV-SECRET"),
            "{args:?}: {}",
            text(&out)
        );
        assert!(text(&out).contains("Usage:"), "{args:?}: {}", text(&out));
    }
    // Help still renders.
    let out = dwcli(dir.path(), &["invite", "--help"], &[], b"");
    assert!(
        out.status.success() && text(&out).contains("claim"),
        "{out:?}"
    );
}
