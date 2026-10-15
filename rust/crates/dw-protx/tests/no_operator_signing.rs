//! MG-01 acceptance: no operator-key signing path remains (repo CLAUDE.md:
//! the wallet never signs with an operator key; docs/design/MG-01-salvage.md
//! §4). Scans the Rust workspace, the desktop UI sources and the Swift
//! sources for what an operator-signed ProUpServTx / ProUpRevTx, or any BLS
//! secret, would need, and fails on the first hit outside the allow-list.
//!
//! Rerun: `cd rust && cargo test -p dw-protx --test no_operator_signing`.
//!
//! Reading those payloads off the chain stays allowed (MG-03 tracks
//! operator-key changes, MG-00 D46): a match pattern `{ .. }` or an enum
//! variant is not a construction.

use std::fs;
use std::path::{Path, PathBuf};

/// Building a payload only the operator key can sign.
const CONSTRUCTED: [&str; 2] = [
    "ProviderUpdateServicePayload",
    "ProviderUpdateRevocationPayload",
];

/// Builders, IPC commands and console commands of the operator-signed
/// transactions (the r3 names and their camelCase / Swift forms), and BLS
/// secret material.
const FORBIDDEN: [&str; 19] = [
    "update_service_placeholder",
    "finalize_update_service",
    "revoke_placeholder",
    "finalize_revoke",
    "prepare_update_service",
    "prepare_revoke",
    "masternode_update_service",
    "masternode_revoke",
    "protx_update_service",
    "protx_revoke",
    "prepareUpdateService",
    "prepareRevoke",
    "updateService",
    "revokeMasternode",
    "BlsSecretKey",
    "BlsSkBytes",
    "blsful",
    "bls_signatures",
    "from_ikm",
];

/// `(file, first line)` from which hits are allowed: dw-coinjoin's test
/// key plays a masternode signing CoinJoin queue messages (MG-01 D9); it
/// signs no wallet transaction. Everything above that line is still checked.
const ALLOWED_FROM: [(&str, &str); 1] = [("rust/crates/dw-coinjoin/src/bls.rs", "pub mod testing")];

const THIS_FILE: &str = "rust/crates/dw-protx/tests/no_operator_signing.rs";

/// Directories scanned, relative to the repository root, and the file
/// extensions read in them.
const ROOTS: [(&str, &[&str]); 5] = [
    ("rust/crates", &["rs"]),
    ("rust/bin", &["rs"]),
    ("apps/desktop/src", &["ts", "tsx"]),
    ("Sources", &["swift"]),
    ("Tests", &["swift"]),
];

const SKIPPED_DIRS: [&str; 3] = ["target", "node_modules", ".build"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn files(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name();
            if !SKIPPED_DIRS.iter().any(|s| name == *s) {
                files(&path, exts, out);
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

/// The source with comments blanked out (line structure kept): line
/// comments from `//` to the end of the line, block comments `/* … */`.
/// String contents are not parsed: a `//` inside a string cuts the rest of
/// the line, which only hides text, never invents a hit.
fn strip_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut in_block = false;
    for line in src.lines() {
        let mut rest = line;
        loop {
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        rest = &rest[end + 2..];
                        in_block = false;
                    }
                    None => break,
                }
            } else {
                let line_comment = rest.find("//");
                let block = rest.find("/*");
                match (line_comment, block) {
                    (Some(l), b) if b.is_none_or(|b| l < b) => {
                        out.push_str(&rest[..l]);
                        break;
                    }
                    (_, Some(b)) => {
                        out.push_str(&rest[..b]);
                        rest = &rest[b + 2..];
                        in_block = true;
                    }
                    _ => {
                        out.push_str(rest);
                        break;
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Whether `code` builds one of [`CONSTRUCTED`]: `Name::new(…)`, or a struct
/// literal `Name { … }` whose body has no `..` (a pattern always has one
/// unless it names every field, which a reader never needs to).
fn constructs(code: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(at) = code[from..].find(name) {
        let after = &code[from + at + name.len()..];
        let next = after.trim_start();
        if next.starts_with("::new") {
            return true;
        }
        if let Some(body) = next.strip_prefix('{') {
            let mut depth = 1;
            let end = body
                .char_indices()
                .find(|&(_, c)| {
                    match c {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })
                .map_or(body.len(), |(i, _)| i);
            if !body[..end].contains("..") {
                return true;
            }
        }
        from += at + name.len();
    }
    false
}

/// Every hit in one file's comment-stripped source, as `token: line`.
fn hits(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (n, line) in code.lines().enumerate() {
        for token in FORBIDDEN {
            if line.contains(token) {
                found.push(format!("{token}: line {}", n + 1));
            }
        }
    }
    for name in CONSTRUCTED {
        if constructs(code, name) {
            found.push(format!("{name} constructed"));
        }
    }
    found
}

#[test]
fn no_operator_key_signing_path_remains() {
    let root = repo_root();
    let mut paths = Vec::new();
    for (dir, exts) in ROOTS {
        files(&root.join(dir), exts, &mut paths);
    }
    assert!(
        paths.len() > 100,
        "the scan found only {} files under {}: wrong root?",
        paths.len(),
        root.display()
    );

    let mut violations = Vec::new();
    let mut allowed_seen = 0;
    for path in &paths {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel == THIS_FILE {
            continue;
        }
        let Ok(src) = fs::read_to_string(path) else {
            continue;
        };
        let mut code = strip_comments(&src);
        if let Some((_, marker)) = ALLOWED_FROM.iter().find(|(f, _)| *f == rel) {
            let cut = code
                .find(marker)
                .unwrap_or_else(|| panic!("{rel} lost its `{marker}` marker"));
            code.truncate(cut);
            allowed_seen += 1;
        }
        violations.extend(hits(&code).into_iter().map(|h| format!("{rel}: {h}")));
    }
    assert_eq!(
        allowed_seen,
        ALLOWED_FROM.len(),
        "an allow-listed file moved"
    );
    assert!(
        violations.is_empty(),
        "operator-key signing path found (MG-01 §4):\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_finds_what_it_must() {
    // Each of these would be a hit…
    for bad in [
        "let p = ProviderUpdateServicePayload::new(a, b);",
        "ProviderUpdateRevocationPayload { version: 2, reason, payload_sig }",
        "let p = ProviderUpdateRevocationPayload {\n    version: 2,\n    reason: 0,\n};",
        "pub fn finalize_revoke(tx: &Transaction) {}",
        "await invoke('prepareUpdateService', args)",
        "let sk = BlsSecretKey::<BlsScIetf>::from_ikm(&ikm);",
        "use dashcore::blsful::SecretKey;",
    ] {
        assert!(!hits(&strip_comments(bad)).is_empty(), "missed: {bad}");
    }
    // …and these are not.
    for ok in [
        "TransactionPayload::ProviderUpdateServicePayloadType(p) => track(p),",
        "if let ProviderUpdateRevocationPayload { pro_tx_hash, .. } = p {}",
        "SpecialType::ProviderUpdateService => \"service update\",",
        "// finalize_revoke is gone (MG-01 D6)",
        "/* BlsSecretKey\n never */ let x = 1;",
        "let pk = BlsPkBytes::from_bytes(bytes);",
    ] {
        assert!(hits(&strip_comments(ok)).is_empty(), "false hit: {ok}");
    }
}
