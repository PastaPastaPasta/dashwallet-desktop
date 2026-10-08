//! Review DW-E0-03 M1: dw-ffi never calls the vault's engine-only entry
//! points (the grant-less background DashPay crypto signer and the
//! identity-scan master key). `clippy.toml` forbids them by path; this test
//! catches the same calls in a plain `cargo test`.

use std::path::{Path, PathBuf};

const FORBIDDEN: [&str; 4] = [
    "dashpay_crypto_signer",
    "scan_key",
    "master_key",
    "VaultScanKey",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn dw_ffi_calls_no_engine_only_vault_entry_point() {
    let mut files = Vec::new();
    rust_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut hits = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            if FORBIDDEN.iter().any(|name| line.contains(name)) {
                hits.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "engine-only vault calls in dw-ffi:\n{}",
        hits.join("\n")
    );
}
