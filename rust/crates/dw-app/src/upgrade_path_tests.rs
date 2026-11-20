//! Upgrade path (PLAN-1.0 H-08): a data root that a released app wrote opens
//! on this build with the same wallets, balances, history, labels, settings
//! and keys.
//!
//! Each `testdata/upgrade/<tag>/` holds `data/`, written by the release
//! binary of `<tag>` (`apps/desktop/scripts/upgrade-fixture.sh`), and
//! `expected.json`: the release's own answers to the read commands, recorded
//! after a restart, and the message signatures it made. The test copies
//! `data/` into a private directory and opens it twice, each time in a child
//! process (the mock app's engine outlives its drop, so a second open in this
//! process would find its databases held). A child unlocks the vault with the
//! release's passphrase, asks the same commands, signs the same messages and
//! shuts the engine down. Every field the release answered must come back
//! equal (fields this build added are free), except the renames in
//! [`RENAMED`] and the per-unlock values in [`VOLATILE`]; and the second
//! open must answer as the first. A new release is covered by adding its
//! directory.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dw_engine::DashNetwork;
use dw_vault::{MemoryOsStore, VaultConfig};
use serde_json::{Value, json};
use tauri::Manager;
use tauri::test::{MockRuntime, mock_context, noop_assets};

use crate::launch::LaunchOptions;

const CHILD: &str = "DW_UPGRADE_PATH_CHILD";
const TEST: &str = "upgrade_path_tests::every_release_data_root_opens_with_the_same_answers";
const DONE: &str = "upgrade-path child done";
/// How long one open may take before its child is killed: generous, for hosts
/// and CI runners under heavy load.
const CHILD_DEADLINE: Duration = Duration::from_secs(600);

/// Fields a later build renamed: (command, field in the release's answer,
/// field in this build's answer).
const RENAMED: &[(&str, &str, &str)] = &[("get_network_choice", "launched", "active")];

/// Fields a later build dropped, with why that loses nothing.
const DROPPED: &[(&str, &str, &str)] = &[(
    "get_network_choice",
    "restartNeeded",
    "the window switches networks without a restart",
)];

/// Values each unlock or open sets anew: compared for presence only.
const VOLATILE: &[(&str, &str)] = &[("quick_unlock_policy", "lastPassphraseAt")];

/// Differences that are known defects: (tag, command, path, why). Each must
/// still occur, so a fix removes its line.
const KNOWN: &[(&str, &str, &str, &str)] = &[(
    "v0.1.0",
    "username_in_requests",
    "calls[36] {\"walletId\":\"da8b3e81edb50926c70d3cd3c5de4102fa7c1419b0559fe19d06813c64dcca30\"}: release true, this build false",
    "47a179cf made the unset per-wallet setting read as off; a 0.1.0 wallet that never changed it was on",
)];

/// Every release directory under `root`, each with a readable, parseable
/// `expected.json`; files beside them (the README) are skipped. Any entry that
/// cannot be read, or a directory without its snapshot, is an error naming it.
fn fixtures_in(root: &Path) -> Result<Vec<(PathBuf, Value)>, String> {
    let entries = std::fs::read_dir(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", root.display()))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !kind.is_dir() {
            continue;
        }
        let snapshot = path.join("expected.json");
        let text = std::fs::read_to_string(&snapshot)
            .map_err(|e| format!("{}: {e}", snapshot.display()))?;
        let expected =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", snapshot.display()))?;
        found.push((path, expected));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// What makes a snapshot too thin to stand for its release: no calls, a
/// wallet without exactly one signature, or a wallet `list_wallets` lacks.
fn snapshot_problems(expected: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let wallets: Vec<&Value> = match expected["wallets"].as_array() {
        Some(w) if !w.is_empty() => w.iter().map(|w| &w["walletId"]).collect(),
        _ => vec![],
    };
    if wallets.is_empty() {
        problems.push("no wallets".to_string());
    }
    let calls = expected["calls"].as_array().map_or(&[][..], Vec::as_slice);
    if calls.is_empty() {
        problems.push("no calls".to_string());
    }
    let signatures = expected["signatures"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if signatures.len() != wallets.len() {
        problems.push(format!(
            "{} signatures for {} wallets",
            signatures.len(),
            wallets.len()
        ));
    }
    let listed = calls
        .iter()
        .find(|c| c["cmd"] == "list_wallets")
        .and_then(|c| c["result"].as_array());
    for id in &wallets {
        if signatures.iter().filter(|s| &&s["walletId"] == id).count() != 1 {
            problems.push(format!("wallet {id} has not exactly one signature"));
        }
        if !listed.is_some_and(|l| l.iter().any(|w| &&w["walletId"] == id)) {
            problems.push(format!("wallet {id} is not in list_wallets"));
        }
    }
    problems
}

fn copy_private(from: &Path, to: &Path) {
    #[allow(unused_mut)]
    let mut mkdir = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut mkdir, 0o700);
    mkdir.create(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().unwrap().is_dir() {
            copy_private(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
        }
    }
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn every_release_data_root_opens_with_the_same_answers() {
    if let Some(dir) = std::env::var_os(CHILD) {
        child(Path::new(&dir));
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade");
    let fixtures = fixtures_in(&root).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        !fixtures.is_empty(),
        "no testdata/upgrade/<tag>/expected.json"
    );
    let mut report = Vec::new();
    for (fixture, expected) in &fixtures {
        let tag = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let problems = snapshot_problems(expected);
        assert!(
            problems.is_empty(),
            "{tag}: expected.json is incomplete: {}",
            problems.join("; ")
        );
        let dir = dw_testutil::private_tempdir();
        copy_private(&fixture.join("data"), &dir.path().join("data"));
        std::fs::copy(
            fixture.join("expected.json"),
            dir.path().join("expected.json"),
        )
        .unwrap();
        let first = open_in_child(&tag, dir.path(), 1);
        let second = open_in_child(&tag, dir.path(), 2);
        let mut found = Vec::new();
        compare_with_release(expected, &first, &mut found);
        found.extend(
            compare_opens(&first, &second)
                .into_iter()
                .map(|(cmd, d)| (format!("{cmd} (second open)"), d)),
        );
        let known: Vec<_> = KNOWN.iter().filter(|k| k.0 == tag).collect();
        for k in &known {
            assert!(
                found
                    .iter()
                    .any(|(cmd, path)| cmd == k.1 && path.starts_with(k.2)),
                "{tag}: the known defect at {} {} no longer occurs ({}): remove it from KNOWN",
                k.1,
                k.2,
                k.3
            );
        }
        found.retain(|(cmd, path)| !known.iter().any(|k| cmd == k.1 && path.starts_with(k.2)));
        report.extend(
            found
                .into_iter()
                .map(|(cmd, d)| format!("{tag}: {cmd}: {d}")),
        );
    }
    assert!(
        report.is_empty(),
        "this build answers differently from the release that wrote the data root:\n{}",
        report.join("\n")
    );
}

fn open_in_child(tag: &str, dir: &Path, n: u32) -> Value {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, dir)
        .env("DW_UPGRADE_PATH_OPEN", n.to_string());
    let (status, stdout, stderr) =
        run_with_deadline(cmd, CHILD_DEADLINE).unwrap_or_else(|e| panic!("{tag}: open {n}: {e}"));
    assert!(
        status.success() && stdout.contains(DONE),
        "{tag}: open {n} failed: {status}\n{stdout}\n{stderr}"
    );
    read_json(&dir.join(format!("answers-{n}.json")))
}

/// Runs `cmd` with its output drained on threads; past `deadline` it is
/// killed and reaped, and the error carries what it printed.
fn run_with_deadline(
    mut cmd: Command,
    deadline: Duration,
) -> Result<(ExitStatus, String, String), String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start: {e}"))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            String::from_utf8_lossy(&bytes).into_owned()
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < deadline => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Err(e) => return Err(format!("could not wait: {e}")),
        }
    };
    let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
    match status {
        Some(status) => Ok((status, stdout, stderr)),
        None => Err(format!(
            "killed after {deadline:?} without finishing\n{stdout}\n{stderr}"
        )),
    }
}

fn compare_with_release(expected: &Value, answers: &Value, found: &mut Vec<(String, String)>) {
    let want = expected["calls"].as_array().unwrap();
    let got = answers["calls"].as_array().unwrap();
    assert_eq!(want.len(), got.len());
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        let cmd = w["cmd"].as_str().unwrap();
        let path = format!("calls[{i}] {}", w["args"]);
        found.extend(
            answer_diffs(cmd, w, g, &path)
                .into_iter()
                .map(|d| (cmd.to_string(), d)),
        );
    }
    if expected["signatures"] != answers["signatures"] {
        found.push((
            "sign_message".into(),
            format!(
                "release signed {}, this build {}",
                expected["signatures"], answers["signatures"]
            ),
        ));
    }
}

/// The second open's answers against the first's, call by call.
fn compare_opens(first: &Value, second: &Value) -> Vec<(String, String)> {
    let a = first["calls"].as_array().unwrap();
    let b = second["calls"].as_array().unwrap();
    let mut found = Vec::new();
    if a.len() != b.len() {
        found.push((
            "calls".to_string(),
            format!("{} answers, then {}", a.len(), b.len()),
        ));
    }
    for (i, (a, b)) in a.iter().zip(b).enumerate() {
        let cmd = a["cmd"].as_str().unwrap();
        let path = format!("calls[{i}] {}", a["args"]);
        found.extend(
            answer_diffs(cmd, a, b, &path)
                .into_iter()
                .map(|d| (cmd.to_string(), d)),
        );
    }
    if first["signatures"] != second["signatures"] {
        found.push(("sign_message".into(), "differs".into()));
    }
    found
}

/// `got` answers a call as `want` did: a result containing `want`'s, or, when
/// either failed, the same failure on both sides counts as a difference too.
fn answer_diffs(cmd: &str, want: &Value, got: &Value, path: &str) -> Vec<String> {
    let mut diffs = Vec::new();
    match (want.get("error"), got.get("error")) {
        (None, None) => contains(cmd, &want["result"], &got["result"], path, &mut diffs),
        (_, Some(e)) => diffs.push(format!("{path}: fails with {e}")),
        (Some(e), None) => diffs.push(format!("{path}: failed before with {e}")),
    }
    diffs
}

/// Every field of `want` is in `got` with an equal value, recursively;
/// arrays match element by element. Differences go to `diffs`.
fn contains(cmd: &str, want: &Value, got: &Value, path: &str, diffs: &mut Vec<String>) {
    match (want, got) {
        (Value::Object(w), Value::Object(g)) => {
            for (key, wv) in w {
                if DROPPED.iter().any(|d| d.0 == cmd && d.1 == key) {
                    continue;
                }
                let name = RENAMED
                    .iter()
                    .find(|r| r.0 == cmd && r.1 == key)
                    .map_or(key.as_str(), |r| r.2);
                let at = format!("{path}.{key}");
                match g.get(name) {
                    None => diffs.push(format!("{at}: missing (release: {wv})")),
                    Some(gv) if VOLATILE.iter().any(|v| v.0 == cmd && v.1 == key) => {
                        if wv.is_null() != gv.is_null() {
                            diffs.push(format!("{at}: release {wv}, this build {gv}"));
                        }
                    }
                    Some(gv) => contains(cmd, wv, gv, &at, diffs),
                }
            }
        }
        (Value::Array(w), Value::Array(g)) if w.len() == g.len() => {
            for (i, (wv, gv)) in w.iter().zip(g).enumerate() {
                contains(cmd, wv, gv, &format!("{path}[{i}]"), diffs);
            }
        }
        (w, g) if w == g => {}
        (w, g) => diffs.push(format!("{path}: release {w}, this build {g}")),
    }
}

fn child(dir: &Path) {
    let n = std::env::var("DW_UPGRADE_PATH_OPEN").unwrap();
    let expected = read_json(&dir.join("expected.json"));
    let root = dir.join("data");
    let network = match expected["network"].as_str().unwrap() {
        "regtest" => DashNetwork::Regtest,
        other => panic!("fixture network {other}"),
    };
    let launch = LaunchOptions {
        data_dir: Some(root.clone()),
        network: network.clone(),
        network_given: true,
        ..LaunchOptions::default()
    };
    let vault = VaultConfig {
        os_store: Arc::new(MemoryOsStore::new()),
        ..VaultConfig::default()
    };
    let mut app = crate::builder_with_vault::<MockRuntime>(launch, Instant::now(), vault)
        .enable_macos_default_menu(false)
        .build(mock_context(noop_assets()))
        .expect("app");
    #[allow(deprecated)]
    app.run_iteration(|_, _| {});
    let state = app.state::<crate::state::AppState>();
    tauri::async_runtime::block_on(state.engine.wait()).expect("the engine opens the data root");
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("webview");
    let call = |cmd: &str, args: Value| {
        crate::ipc_tests::invoke(&webview, app.invoke_key(), cmd, args).map_err(|e| {
            e.to_string()
                .replace(&*root.to_string_lossy(), "$DATA_ROOT")
        })
    };
    let ok = |cmd: &str, args: Value| call(cmd, args).unwrap_or_else(|e| panic!("{cmd}: {e}"));
    ok(
        "open_network",
        json!({ "settings": crate::ipc_tests::offline_settings() }),
    );
    let passphrase = expected["passphrase"].as_str().unwrap();
    ok("unlock", json!({ "passphrase": passphrase }));

    let root_text = serde_json::to_string(&root.to_string_lossy()).unwrap();
    let root_text = &root_text[1..root_text.len() - 1];
    let calls: Vec<Value> = expected["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let cmd = c["cmd"].as_str().unwrap();
            match call(cmd, c["args"].clone()) {
                Ok(result) => {
                    let text = result.to_string().replace(root_text, "$DATA_ROOT");
                    json!({ "cmd": cmd, "args": c["args"], "result": serde_json::from_str::<Value>(&text).unwrap() })
                }
                Err(error) => json!({ "cmd": cmd, "args": c["args"], "error": error }),
            }
        })
        .collect();
    let signatures: Vec<Value> = expected["signatures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let wallet_id = &s["walletId"];
            let grant = ok(
                "authorize",
                json!({
                    "purpose": { "kind": "signMessage" },
                    "walletId": wallet_id,
                    "credential": { "kind": "passphrase", "passphrase": passphrase },
                }),
            );
            let signature = ok(
                "sign_message",
                json!({ "walletId": wallet_id, "address": s["address"], "message": s["message"], "grantId": grant["id"] }),
            );
            let verified = ok(
                "verify_message",
                json!({ "address": s["address"], "signature": signature, "message": s["message"] }),
            );
            assert_eq!(verified["outcome"], "verified", "{verified}");
            json!({ "walletId": wallet_id, "address": s["address"], "message": s["message"], "signature": signature })
        })
        .collect();
    tauri::async_runtime::block_on(state.shutdown_engine()).expect("engine shutdown");
    let marker = root.join(network.dir_name()).join(".session-open");
    assert!(
        !marker.try_exists().expect("check the open-session marker"),
        "open {n} left an unclean session: {}",
        marker.display()
    );
    std::fs::write(
        dir.join(format!("answers-{n}.json")),
        serde_json::to_string_pretty(&json!({ "calls": calls, "signatures": signatures })).unwrap(),
    )
    .unwrap();
    println!("{DONE}");
}

fn v0_1_0_snapshot() -> Value {
    read_json(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/upgrade/v0.1.0/expected.json"),
    )
}

#[test]
fn a_call_that_fails_on_one_open_and_answers_on_the_other_differs() {
    let answered = |result: Value| json!({ "calls": [{ "cmd": "tx_label", "args": {}, "result": result }], "signatures": [] });
    let failed = json!({ "calls": [{ "cmd": "tx_label", "args": {}, "error": "storage" }], "signatures": [] });
    assert!(compare_opens(&answered(Value::Null), &answered(Value::Null)).is_empty());
    assert_eq!(compare_opens(&answered(Value::Null), &failed).len(), 1);
    assert_eq!(compare_opens(&failed, &answered(Value::Null)).len(), 1);
    assert_eq!(compare_opens(&failed, &failed).len(), 1);
    assert_eq!(
        compare_opens(&answered(json!("a")), &answered(json!("b"))).len(),
        1
    );
}

#[test]
fn discovery_names_a_release_directory_without_a_readable_snapshot() {
    let root = dw_testutil::private_tempdir();
    std::fs::write(root.path().join("README.md"), "notes").unwrap();
    std::fs::create_dir(root.path().join("v9.9.8")).unwrap();
    std::fs::write(root.path().join("v9.9.8/expected.json"), "{}").unwrap();
    assert_eq!(fixtures_in(root.path()).unwrap().len(), 1);
    std::fs::create_dir(root.path().join("v9.9.9")).unwrap();
    let e = fixtures_in(root.path()).unwrap_err();
    assert!(e.contains("v9.9.9"), "{e}");
    std::fs::write(root.path().join("v9.9.9/expected.json"), "{").unwrap();
    let e = fixtures_in(root.path()).unwrap_err();
    assert!(e.contains("v9.9.9"), "{e}");
}

#[test]
fn a_thinned_snapshot_is_refused() {
    let full = v0_1_0_snapshot();
    assert_eq!(snapshot_problems(&full), Vec::<String>::new());
    let mut no_calls = full.clone();
    no_calls["calls"] = json!([]);
    assert!(!snapshot_problems(&no_calls).is_empty());
    let mut one_signature = full.clone();
    one_signature["signatures"].as_array_mut().unwrap().pop();
    assert!(!snapshot_problems(&one_signature).is_empty());
    let mut unlisted = full.clone();
    for call in unlisted["calls"].as_array_mut().unwrap() {
        if call["cmd"] == "list_wallets" {
            call["result"].as_array_mut().unwrap().pop();
        }
    }
    assert!(!snapshot_problems(&unlisted).is_empty());
}

#[cfg(unix)]
#[test]
fn a_child_past_its_deadline_is_killed_and_its_output_kept() {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo started; exec sleep 60"]);
    let started = Instant::now();
    let e = run_with_deadline(cmd, Duration::from_secs(1)).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(30));
    assert!(e.contains("killed") && e.contains("started"), "{e}");
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo done"]);
    let (status, stdout, _) = run_with_deadline(cmd, Duration::from_secs(60)).unwrap();
    assert!(status.success() && stdout.contains("done"));
}
