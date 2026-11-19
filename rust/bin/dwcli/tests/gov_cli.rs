//! `dwcli gov …` end to end through the binary (MG-08): one JSON line on
//! stdout whatever fails, the contract's codes in it, and no argument or
//! global option value quoted on stdout or stderr.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const SECRET: &str = "e9873d79c6d87dc0fb6a5778633389f4453213303da61f20bd67fc233aa33262";

fn dwcli(dir: &Path, pass: &str, args: &[&str]) -> Output {
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
    child.stdin.take().unwrap().write_all(b"").unwrap();
    child.wait_with_output().unwrap()
}

fn line(out: &Output) -> Value {
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "stdout: {stdout:?}, stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_str(lines[0]).unwrap()
}

fn ok(out: &Output) -> Value {
    assert!(out.status.success(), "{out:?}");
    let v = line(out);
    assert_eq!(v["ok"], true, "{v}");
    v["result"].clone()
}

fn error(out: &Output) -> (String, String) {
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let v = line(out);
    assert_eq!(v["ok"], false, "{v}");
    let text = |k: &str| v["error"][k].as_str().unwrap().to_string();
    (text("code"), text("message"))
}

#[test]
fn gov_offline_reads_settings_and_contract_refusals() {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli gov passphrase\n").unwrap();
    std::fs::write(dir.path().join("wrong"), "not the passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        assert!(dwcli(dir.path(), "pass", args).status.success());
    }
    let hash = "ab".repeat(32);

    // Sync setting persists across processes; resync refuses while disabled.
    assert_eq!(
        ok(&dwcli(dir.path(), "pass", &["gov", "sync-enabled"])),
        json!({ "enabled": false })
    );
    assert_eq!(
        error(&dwcli(dir.path(), "pass", &["gov", "resync"])).0,
        "governance.sync_disabled"
    );
    assert_eq!(
        ok(&dwcli(
            dir.path(),
            "pass",
            &["gov", "set-sync-enabled", "true"]
        )),
        json!({ "enabled": true })
    );
    assert_eq!(
        ok(&dwcli(dir.path(), "pass", &["gov", "sync-enabled"])),
        json!({ "enabled": true })
    );
    assert_eq!(
        ok(&dwcli(dir.path(), "pass", &["gov", "resync"])),
        json!({ "resync": true })
    );
    assert_eq!(
        ok(&dwcli(
            dir.path(),
            "pass",
            &["gov", "set-sync-enabled", "false"]
        )),
        json!({ "enabled": false })
    );
    let state = ok(&dwcli(dir.path(), "pass", &["gov", "sync-state"]));
    assert_eq!(state["phase"], "disabled");

    // Pure superblock budget and offline own proposals.
    let budget = ok(&dwcli(dir.path(), "pass", &["gov", "budget", "1500"]));
    assert_eq!(budget["height"], 1500);
    assert!(budget["budget"].as_u64().unwrap() > 0, "{budget}");
    assert_eq!(
        ok(&dwcli(dir.path(), "pass", &["gov", "budget", "1501"])),
        json!({ "height": 1501, "budget": 0 })
    );
    assert_eq!(
        ok(&dwcli(dir.path(), "pass", &["gov", "list-prepared"])),
        json!([])
    );

    // Contract error codes before sync or tip.
    for (args, want) in [
        (&["gov", "clock"][..], "governance.not_synced"),
        (&["gov", "info"], "governance.not_synced"),
        (&["gov", "superblocks"], "governance.not_synced"),
        (&["gov", "superblocks", "--count", "25"], "invalid_argument"),
        (&["gov", "list"], "governance.not_synced"),
        (&["gov", "get", &hash], "governance.not_synced"),
        (&["gov", "current-votes", &hash], "governance.not_synced"),
        (
            &["gov", "voting-masternodes", &hash],
            "governance.list_unavailable",
        ),
        (&["gov", "voting-masternodes", "zz"], "invalid_argument"),
        (
            &[
                "gov",
                "vote",
                &hash,
                "--outcome",
                "yes",
                "--masternode",
                &hash,
            ],
            "governance.sync_disabled",
        ),
        (
            &[
                "gov",
                "voteraw",
                &format!("{hash}-0"),
                &hash,
                "--outcome",
                "no",
                "--time",
                "1700000000",
                "--signature",
                "c2ln",
            ],
            "governance.sync_disabled",
        ),
        (
            &[
                "gov",
                "prepare",
                "--preview",
                "--name",
                "dwd_p0",
                "--url",
                "https://example.org/p0",
                "--payment-address",
                "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n",
                "--payment-amount",
                "100000000",
                "--payment-count",
                "1",
                "--first-superblock",
                "1500",
            ],
            "governance.not_synced",
        ),
        (&["gov", "submit", &hash], "governance.proposal_not_found"),
        (&["gov", "submit", "zz"], "invalid_argument"),
    ] {
        let (code, _) = error(&dwcli(dir.path(), "pass", args));
        assert_eq!(code, want, "{args:?}");
    }

    // Wrong passphrase, missing passphrase file, and bad --collateral-rpc.
    assert_eq!(
        error(&dwcli(dir.path(), "wrong", &["gov", "sync-state"])).0,
        "vault"
    );
    assert_eq!(
        error(&dwcli(dir.path(), "missing", &["gov", "sync-state"])).0,
        "setup"
    );
    let out = Command::new(env!("CARGO_BIN_EXE_dwcli"))
        .arg("--datadir")
        .arg(dir.path().join("data"))
        .args([
            "--dapi",
            "http://127.0.0.1:1",
            "--no-platform",
            "--collateral-rpc",
            &format!("ftp://user:{SECRET}@127.0.0.1:1"),
            "gov",
            "sync-state",
        ])
        .env("DWCLI_LOG", "info")
        .output()
        .unwrap();
    let (code, msg) = error(&out);
    assert_eq!(code, "setup");
    assert_eq!(msg, "invalid --collateral-rpc value");
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .to_lowercase();
    assert!(!all.contains(SECRET), "{all}");
}

#[test]
fn gov_failures_never_quote_secret_shaped_arguments() {
    let dir = dw_testutil::private_tempdir();
    std::fs::write(dir.path().join("pass"), "dwcli gov passphrase\n").unwrap();
    for args in [&["init-vault"][..], &["create"]] {
        assert!(dwcli(dir.path(), "pass", args).status.success());
    }
    // Refused command lines print a single JSON line and quote nothing.
    for bad in [
        &["gov"][..],
        &[
            "gov",
            "vote",
            SECRET,
            "--outcome",
            SECRET,
            "--masternode",
            SECRET,
        ],
        &["gov", "list", "--scope", SECRET],
        &["gov", "budget", SECRET],
    ] {
        let out = dwcli(dir.path(), "pass", bad);
        assert_eq!(out.status.code(), Some(2), "{bad:?}: {out:?}");
        assert_eq!(line(&out)["error"]["code"], "invalid_argument");
        assert!(!format!("{out:?}").to_lowercase().contains(SECRET));
    }

    let outpoint = format!("{SECRET}-0");
    let commands: &[(&[&str], &str)] = &[
        (&["gov", "list", "--text", SECRET], "governance.not_synced"),
        (&["gov", "get", SECRET], "governance.not_synced"),
        (
            &["gov", "current-votes", SECRET, "--outpoint", &outpoint],
            "governance.not_synced",
        ),
        (
            &["gov", "voting-masternodes", SECRET, "--wallet", SECRET],
            "wallet_not_found",
        ),
        (
            &[
                "gov",
                "vote",
                SECRET,
                "--outcome",
                "yes",
                "--masternode",
                SECRET,
                "--wallet",
                SECRET,
            ],
            "governance.sync_disabled",
        ),
        (
            &[
                "gov",
                "voteraw",
                &outpoint,
                SECRET,
                "--outcome",
                "yes",
                "--time",
                "1700000000",
                "--signature",
                SECRET,
            ],
            "governance.sync_disabled",
        ),
        (
            &[
                "gov",
                "prepare",
                "--name",
                SECRET,
                "--url",
                SECRET,
                "--payment-address",
                SECRET,
                "--payment-amount",
                "100000000",
                "--payment-count",
                "1",
                "--first-superblock",
                "1500",
                "--coin",
                &outpoint,
                "--wallet",
                SECRET,
            ],
            "governance.not_synced",
        ),
        (
            &["gov", "list-prepared", "--wallet", SECRET],
            "wallet_not_found",
        ),
        (
            &["gov", "submit", SECRET, "--wallet", SECRET],
            "governance.proposal_not_found",
        ),
        (
            &["gov", "--spv", "--sync-timeout", "1", "info"],
            "governance.list_unavailable",
        ),
    ];
    let mut rev_bytes = hex::decode(SECRET).unwrap();
    rev_bytes.reverse();
    let reversed = hex::encode(rev_bytes);
    for (args, want) in commands {
        let out = dwcli(dir.path(), "pass", args);
        let (code, _) = error(&out);
        assert_eq!(code, *want, "{args:?}");
        let all = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
        .to_lowercase();
        for s in [SECRET, reversed.as_str()] {
            assert!(!all.contains(s), "{args:?} quotes input: {all}");
        }
    }
}
