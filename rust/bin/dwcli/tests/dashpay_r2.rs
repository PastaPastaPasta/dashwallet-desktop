//! Regressions from review DW-E0-09 r2 (Sol), through the `dwcli` binary:
//! secrets in `args` that only a grammar refuses, engine-task panics, and
//! JSON integers with a leading zero.
//!
//! The panic cases use the debug-build fault hook `DWCLI_FAULT_INJECT`
//! (`task-panic`: `dashpay status` runs an engine task that panics;
//! `task-panic-poison`: the session's health probe then fails too).

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

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

const SECRET: &str = "ARGS-SECRET";

/// Sol finding 1: percent-encoded, split and bare secrets in `args` are
/// refused by position, unquoted, and the session goes on.
#[test]
fn secrets_in_args_are_refused_by_the_grammar() {
    let dir = dw_testutil::private_tempdir();
    let requests = [
        json!({"args": ["invite", "stash", "dashpay%3A%2F%2Finvite%3Fpk%3DARGS-SECRET"], "id": 1}),
        json!({"args": ["invite", "stash", "dashpay", "://", "invite?pk", "=ARGS-SECRET"], "id": 2}),
        json!({"args": ["invite", "stash", "ARGS-SECRET"], "id": 3}),
        json!({"args": ["name", "check", "alice"], "id": 4}),
    ];
    let stdin: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let out = dwcli(dir.path(), &["dashpay", "session"], &[], stdin.as_bytes());
    assert!(out.status.success(), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 5, "{got:?}");
    for (answer, id) in got.iter().zip(1..=3) {
        assert_eq!(answer["id"], id);
        assert_eq!(answer["error"]["code"], "invalid_argument");
        assert_eq!(
            answer["error"]["params"]["detail"],
            "bad arguments: argument 3 is surplus: `invite stash` takes no more"
        );
    }
    assert_eq!(got[3]["error"]["params"]["call"], "check_username");
    assert_eq!(got[4], json!({"ok": true, "result": {"requests": 4}}));
    assert!(!text(&out).contains(SECRET), "{}", text(&out));
}

/// Sol finding 3, one-shot: an engine task's panic is one sanitized
/// `internal` line, and its payload reaches neither stream.
#[test]
fn a_one_shot_task_panic_is_one_sanitized_line() {
    let dir = dw_testutil::private_tempdir();
    let fault = [("DWCLI_FAULT_INJECT", "task-panic")];
    let out = dwcli(dir.path(), &["dashpay", "status"], &fault, b"");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0]["error"],
        json!({
            "code": "internal",
            "message": "an engine task panicked; the call's outcome is unknown",
            "params": {"detail": "an engine task panicked; the call's outcome is unknown"},
        })
    );
    assert!(!text(&out).contains("PANIC-SECRET"), "{}", text(&out));
}

/// Sol finding 3, session: an engine task's panic is answered and then
/// probed like dwcli's own, so a failing probe ends the session.
#[test]
fn a_session_probes_after_a_task_panic() {
    let dir = dw_testutil::private_tempdir();
    let requests = concat!(
        r#"{"args":["dashpay","status"],"id":1}"#,
        "\n",
        r#"{"args":["name","check","alice"],"id":2}"#,
        "\n",
    );
    let fault = [("DWCLI_FAULT_INJECT", "task-panic")];
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
    assert_eq!(got[1]["error"]["params"]["call"], "check_username");
    assert_eq!(got[2], json!({"ok": true, "result": {"requests": 2}}));
    assert!(!text(&out).contains("PANIC-SECRET"), "{}", text(&out));

    let fault = [("DWCLI_FAULT_INJECT", "task-panic-poison")];
    let out = dwcli(
        dir.path(),
        &["dashpay", "session"],
        &fault,
        requests.as_bytes(),
    );
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 2, "{got:?}");
    assert_eq!(got[0]["id"], 1);
    assert_eq!(got[1]["error"]["code"], "session_poisoned", "{got:?}");
    assert!(!text(&out).contains("PANIC-SECRET"), "{}", text(&out));
}

/// Sol finding 5: an id with a leading zero is malformed JSON, not a
/// request that runs.
#[test]
fn leading_zero_ids_are_malformed() {
    let dir = dw_testutil::private_tempdir();
    let requests = concat!(
        r#"{"args":["name","check","alice"],"id":01}"#,
        "\n",
        r#"{"args":["name","check","alice"],"id":-01}"#,
        "\n",
    );
    let out = dwcli(
        dir.path(),
        &["dashpay", "session"],
        &[],
        requests.as_bytes(),
    );
    assert!(out.status.success(), "{out:?}");
    let got = lines(&out);
    assert_eq!(got.len(), 3, "{got:?}");
    for (answer, col) in got.iter().zip([40, 41]) {
        assert_eq!(answer.get("id"), None);
        assert_eq!(
            answer["error"]["params"]["detail"],
            format!("malformed request at column {col}")
        );
    }
}
