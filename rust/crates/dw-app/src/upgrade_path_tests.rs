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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use dw_engine::DashNetwork;
use dw_vault::{MemoryOsStore, VaultConfig};
use serde_json::{Value, json};
use tauri::Manager;
use tauri::test::{MockRuntime, mock_context, noop_assets};

use crate::launch::LaunchOptions;

const CHILD: &str = "DW_UPGRADE_PATH_CHILD";
const TEST: &str = "upgrade_path_tests::every_release_data_root_opens_with_the_same_answers";
const DONE: &str = "upgrade-path child done";

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

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/upgrade");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("expected.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

fn copy_private(from: &Path, to: &Path) {
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
    let fixtures = fixtures();
    assert!(
        !fixtures.is_empty(),
        "no testdata/upgrade/<tag>/expected.json"
    );
    let mut report = Vec::new();
    for fixture in &fixtures {
        let tag = fixture.file_name().unwrap().to_string_lossy().into_owned();
        let expected = read_json(&fixture.join("expected.json"));
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
        compare_with_release(&expected, &first, &mut found);
        for (i, (a, b)) in first["calls"]
            .as_array()
            .unwrap()
            .iter()
            .zip(second["calls"].as_array().unwrap())
            .enumerate()
        {
            let cmd = a["cmd"].as_str().unwrap();
            let mut diffs = Vec::new();
            contains(
                cmd,
                &a["result"],
                &b["result"],
                &format!("calls[{i}]"),
                &mut diffs,
            );
            found.extend(
                diffs
                    .into_iter()
                    .map(|d| (format!("{cmd} (second open)"), d)),
            );
        }
        if first["signatures"] != second["signatures"] {
            found.push(("sign_message (second open)".into(), "differs".into()));
        }
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
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD, dir)
        .env("DW_UPGRADE_PATH_OPEN", n.to_string())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains(DONE),
        "{tag}: open {n} failed: {}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    read_json(&dir.join(format!("answers-{n}.json")))
}

fn compare_with_release(expected: &Value, answers: &Value, found: &mut Vec<(String, String)>) {
    let want = expected["calls"].as_array().unwrap();
    let got = answers["calls"].as_array().unwrap();
    assert_eq!(want.len(), got.len());
    for (i, (w, g)) in want.iter().zip(got).enumerate() {
        let cmd = w["cmd"].as_str().unwrap();
        let path = format!("calls[{i}] {}", w["args"]);
        let mut diffs = Vec::new();
        match &g["error"] {
            Value::Null => contains(cmd, &w["result"], &g["result"], &path, &mut diffs),
            e => diffs.push(format!("{path}: fails with {e}")),
        }
        found.extend(diffs.into_iter().map(|d| (cmd.to_string(), d)));
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
        network,
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
    std::fs::write(
        dir.join(format!("answers-{n}.json")),
        serde_json::to_string_pretty(&json!({ "calls": calls, "signatures": signatures })).unwrap(),
    )
    .unwrap();
    println!("{DONE}");
}
