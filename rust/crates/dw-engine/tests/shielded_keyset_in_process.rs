//! K2: a shielded spend's Orchard key set is derived in this process,
//! from the wallet's own seed inside the vault, for one library call. It
//! never crosses IPC or the FFI and never comes from the renderer. The
//! engine binds it to one local for that call; the line checks below are a
//! tripwire for a change that keeps one longer, not a proof that none does.

use std::fs;
use std::path::{Path, PathBuf};

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// `path` is relative to the crate's manifest directory.
fn is_test_file(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    name == "tests.rs"
        || name.ends_with("_tests.rs")
        || path.components().any(|c| c.as_os_str() == "tests")
}

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The host-facing sources: the Tauri commands, the FFI, the desktop shell,
/// the console and the CLI.
fn host_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for src in [
        crates().join("dw-app/src"),
        crates().join("dw-ffi/src"),
        crates().join("dw-desktop/src"),
        crates().join("dw-console/src"),
        crates().join("../bin/dwcli/src"),
    ] {
        assert!(src.is_dir(), "{} is missing", src.display());
        files(&src, "rs", &mut out);
    }
    out
}

const KEY_TYPES: [&str; 4] = [
    "OrchardKeySet",
    "ShieldedSpendKey",
    "shielded_keys",
    "spend_auth",
];

#[test]
fn no_host_crate_names_a_spend_key_set() {
    for path in host_sources() {
        let src = fs::read_to_string(&path).unwrap();
        for name in KEY_TYPES {
            assert!(!src.contains(name), "{} names {name}", path.display());
        }
    }
}

#[test]
fn no_renderer_binding_carries_a_key_set_or_spend_authority() {
    let generated = crates().join("../../apps/desktop/src/api/generated");
    assert!(
        generated.is_dir(),
        "the generated bindings are missing at {}",
        generated.display()
    );
    let mut out = Vec::new();
    files(&generated, "ts", &mut out);
    assert!(out.len() > 100, "the generated bindings were not found");
    let banned = [
        "keyset",
        "key_set",
        "spendauth",
        "spend_auth",
        "orchardkey",
        "spendingkey",
        "spendkey",
        "spend_key",
    ];
    for path in out {
        let src = fs::read_to_string(&path).unwrap().to_lowercase();
        for name in banned {
            assert!(!src.contains(name), "{} carries {name}", path.display());
        }
        // Declarations only: the bindings' doc comments say "ask" in prose.
        let code = src.lines().filter(|l| {
            let l = l.trim_start();
            !(l.starts_with('*') || l.starts_with("/*") || l.starts_with("//"))
        });
        for word in code.flat_map(|l| l.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')) {
            assert_ne!(word, "ask", "{} carries an ask", path.display());
        }
    }
}

#[test]
fn the_engine_takes_a_key_set_only_for_one_library_call() {
    let mut out = Vec::new();
    files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        "rs",
        &mut out,
    );
    let mut named = Vec::new();
    let mut calls = 0;
    for path in &out {
        let rel = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap();
        if is_test_file(rel) {
            continue;
        }
        let rel = rel.to_string_lossy().into_owned();
        let src = fs::read_to_string(path).unwrap();
        if src
            .lines()
            .any(|l| l.contains("OrchardKeySet") && !l.trim_start().starts_with("//"))
        {
            named.push(rel.clone());
        }
        for (i, line) in src.lines().enumerate() {
            if line.contains(".shielded_keys()") {
                calls += 1;
                assert_eq!(
                    line.trim(),
                    "let keys = permit.shielded_keys()?;",
                    "{rel}:{}: a key set is bound to one local for the call",
                    i + 1
                );
                assert!(rel.ends_with("shielded_pool.rs"), "{rel} takes a key set");
            }
        }
    }
    assert_eq!(
        named,
        vec!["src/platform/lease/call.rs".to_owned()],
        "only the permit's release names the key set type: no field, cache or channel holds one"
    );
    assert_eq!(calls, 3, "transfer, withdraw and unshield each take one");
    let call = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/platform/lease/call.rs"),
    )
    .unwrap();
    let naming: Vec<&str> = (call.lines())
        .map(str::trim)
        .filter(|l| l.contains("OrchardKeySet") && !l.starts_with("//"))
        .collect();
    assert_eq!(
        naming,
        [
            "use platform_wallet::wallet::shielded::OrchardKeySet;",
            "pub(crate) fn shielded_keys(&self) -> Result<OrchardKeySet, LeaseError> {",
        ],
        "the permit names the key set only to return it"
    );
}
