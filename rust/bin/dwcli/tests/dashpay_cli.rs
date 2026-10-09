//! The DashPay commands end to end through the `dwcli` binary (E0-09): one
//! JSON line on stdout, exit status 1 for an error, and a bearer input read
//! from stdin that appears in no output.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

#[path = "../src/test_stub.rs"]
mod test_stub;
use test_stub::{STUB_CALL, stub_request};

fn dwcli(dir: &Path, args: &[&str], stdin: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.join("data"))
        .args(["--dapi", "http://127.0.0.1:1", "--passphrase-file"])
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

/// The single JSON line a DashPay command prints.
fn line(out: &Output) -> Value {
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout: {stdout:?}");
    serde_json::from_str(lines[0]).unwrap()
}

#[test]
fn dashpay_commands_print_one_json_line() {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli e2e passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        let out = dwcli(dir.path(), args, b"");
        assert!(out.status.success(), "{args:?}: {out:?}");
    }

    let out = dwcli(dir.path(), &["dashpay", "status"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        line(&out),
        json!({
            "ok": false,
            "error": {
                "code": "platform.not_implemented",
                "message": "not implemented: DashPay.status",
                "params": {"call": "DashPay.status"},
            },
        })
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not implemented: DashPay.status"),
        "{stderr}"
    );

    // A write's grant comes from the encrypted vault with the passphrase
    // (`identity discover` authorizes before it reaches Platform, which is
    // unreachable here).
    let out = dwcli(dir.path(), &["identity", "discover"], b"");
    assert_eq!(line(&out)["error"]["code"], "platform.unavailable");

    // The link comes from stdin and is quoted nowhere.
    let secret = "dash:?invitation=E2E-BEARER-SECRET";
    let out = dwcli(
        dir.path(),
        &["invite", "claim", "--label", "alice"],
        secret.as_bytes(),
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        line(&out)["error"]["params"],
        json!({"call": "NetworkSession.stash_invitation"})
    );
    for stream in [&out.stdout, &out.stderr] {
        assert!(!String::from_utf8_lossy(stream).contains("E2E-BEARER-SECRET"));
    }

    // A session keeps one engine for many commands.
    let requests = format!(
        "{}\n{}\n",
        stub_request("1"),
        r#"{"args":["invite","stash"],"input":"dash:?invitation=E2E-BEARER-SECRET","id":2}"#,
    );
    let out = dwcli(dir.path(), &["dashpay", "session"], requests.as_bytes());
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("E2E-BEARER-SECRET"));
    let lines: Vec<Value> = stdout
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3, "{stdout}");
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[0]["error"]["params"]["call"], STUB_CALL);
    assert_eq!(lines[1]["id"], 2);
    assert_eq!(lines[2], json!({"ok": true, "result": {"requests": 2}}));

    // A failure before the command runs is a JSON line too.
    std::fs::remove_file(dir.path().join("pass")).unwrap();
    let out = dwcli(dir.path(), &["dashpay", "status"], b"");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(line(&out)["error"]["code"], "setup");

    // A failed unlock is a JSON error with the vault's code.
    std::fs::write(dir.path().join("pass"), "wrong\n").unwrap();
    let out = dwcli(dir.path(), &["identity", "list"], b"");
    assert_eq!(out.status.code(), Some(1));
    let code = line(&out)["error"]["code"].as_str().unwrap().to_string();
    assert!(code.starts_with("vault."), "{code}");
}
