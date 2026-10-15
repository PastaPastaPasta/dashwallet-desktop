//! MG-01 acceptance: no operator-key signing path remains (repo CLAUDE.md:
//! the wallet never signs with an operator key; docs/design/MG-01-salvage.md
//! §4). Two checks share the job:
//!
//! - `rust/clippy.toml` disallows the BLS secret types and platform-wallet's
//!   ProUpServTx builders by definition, so aliases and re-exports cannot
//!   hide them (the gate's `cargo clippy -D warnings`);
//! - this test scans the sources clippy does not see or cannot judge by
//!   type (Rust, the desktop UI, Swift) for building an operator-signed
//!   payload and for the names of the builders, their IPC and console
//!   commands, and BLS secret material, and fails on the first hit outside
//!   the allow-list.
//!
//! Rerun: `cd rust && cargo test -p dw-protx --test no_operator_signing`.
//!
//! Reading those payloads off the chain stays allowed (MG-03 tracks
//! operator-key changes, MG-00 D46): a match pattern `{ .. }` or an enum
//! variant is not a construction.

use std::fs;
use std::path::{Path, PathBuf};

/// Payloads only the operator key can sign.
const CONSTRUCTED: &[&str] = &[
    "ProviderUpdateServicePayload",
    "ProviderUpdateRevocationPayload",
];

/// Builders, IPC commands and console commands of the operator-signed
/// transactions (the r3 names and their camelCase / Swift forms), BLS
/// secret material, and the CoinJoin test operator key.
const FORBIDDEN: &[&str] = &[
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
    "BlsSkShare",
    "blsful",
    "bls_signatures",
    "from_ikm",
    "OperatorKey",
    "bls::testing",
];

/// `(path or directory prefix, first line)`: from that line on (the whole
/// file when `None`) hits are allowed. dw-coinjoin's test operator key and
/// the tests that use it play a masternode signing CoinJoin queue messages
/// (MG-01 D9); they sign no wallet transaction. Everything above the line
/// is still checked.
const ALLOWED_FROM: &[(&str, Option<&str>)] = &[
    (
        "rust/crates/dw-coinjoin/src/bls.rs",
        Some("pub mod testing"),
    ),
    ("rust/crates/dw-coinjoin/src/queue.rs", Some("#[cfg(test)]")),
    ("rust/crates/dw-coinjoin/tests/", None),
];

const THIS_FILE: &str = "rust/crates/dw-protx/tests/no_operator_signing.rs";

/// Directories scanned, relative to the repository root, and the file
/// extensions read in them. `Vendor/` is third-party code.
const ROOTS: &[(&str, &[&str])] = &[
    ("rust/crates", &["rs"]),
    ("rust/bin", &["rs"]),
    ("tools", &["rs"]),
    ("probes", &["rs", "swift"]),
    ("apps/desktop/src", &["ts", "tsx"]),
    ("Sources", &["swift"]),
    ("Tests", &["swift"]),
    ("Apps", &["swift"]),
];

const SKIPPED_DIRS: &[&str] = &["target", "node_modules", ".build"];

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

#[derive(Clone, Copy, PartialEq)]
enum Lang {
    Rust,
    Ts,
    Swift,
}

fn lang_of(ext: &str) -> Lang {
    match ext {
        "rs" => Lang::Rust,
        "ts" | "tsx" => Lang::Ts,
        _ => Lang::Swift,
    }
}

/// The source with comments removed and string contents kept (IPC command
/// names live in strings). Newlines inside comments are kept, so line
/// numbers stay right. Handles `//` and (nested, in Rust and Swift) `/* */`
/// comments; `"…"` strings with escapes; Rust raw strings `r#"…"#` and char
/// literals (a `'` that does not close within a char is a lifetime);
/// TypeScript `'…'` and `` `…` `` strings.
fn strip_comments(src: &str, lang: Lang) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    // Copies a quoted string from `i` (at the opening quote) through its
    // closing quote, honouring backslash escapes.
    let quoted = |i: &mut usize, out: &mut Vec<u8>, q: u8| {
        out.push(b[*i]);
        *i += 1;
        while *i < b.len() {
            let c = b[*i];
            out.push(c);
            *i += 1;
            if c == b'\\' && *i < b.len() {
                out.push(b[*i]);
                *i += 1;
            } else if c == q {
                break;
            }
        }
    };
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == b'/' && next == Some(b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && next == Some(b'*') {
            let nests = lang != Lang::Ts;
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else if nests && b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else {
                    if b[i] == b'\n' {
                        out.push(b'\n');
                    }
                    i += 1;
                }
            }
        } else if c == b'"' || (lang == Lang::Ts && (c == b'\'' || c == b'`')) {
            quoted(&mut i, &mut out, c);
        } else if lang == Lang::Rust
            && c == b'r'
            && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_'))
            && matches!(next, Some(b'"' | b'#'))
        {
            // A raw string `r"…"` / `r#"…"#` (or the identifier `r#name`).
            let hashes = b[i + 1..].iter().take_while(|&&h| h == b'#').count();
            if b.get(i + 1 + hashes) == Some(&b'"') {
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                let body = i + 2 + hashes;
                let end = b[body..]
                    .windows(close.len())
                    .position(|w| w == close.as_slice())
                    .map_or(b.len(), |p| body + p + close.len());
                out.extend_from_slice(&b[i..end]);
                i = end;
            } else {
                out.push(c);
                i += 1;
            }
        } else if lang == Lang::Rust && c == b'\'' {
            // `'x'`, `'\n'`, `'\u{…}'`: a char literal; otherwise a lifetime.
            let len = if next == Some(b'\\') {
                b[i + 2..].iter().position(|&q| q == b'\'').map(|p| p + 3)
            } else {
                let ch = src[i + 1..].chars().next().map_or(1, char::len_utf8);
                (b.get(i + 1 + ch) == Some(&b'\'')).then_some(ch + 2)
            };
            let len = len.unwrap_or(1);
            out.extend_from_slice(&b[i..(i + len).min(b.len())]);
            i += len;
        } else {
            out.push(c);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether `code` builds or aliases one of [`CONSTRUCTED`]: `Name::new(…)`,
/// `Name as Other`, or a struct literal `Name { … }` that is not a pattern.
/// A pattern ends in `..` right before the brace; a literal has no `..` or
/// one followed by a base expression (`..decoded`).
fn constructs(code: &str, name: &str) -> bool {
    let mut from = 0;
    while let Some(at) = code[from..].find(name) {
        let start = from + at + name.len();
        from = start;
        let after = &code[start..];
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            // `…PayloadType`: another identifier.
            continue;
        }
        let next = after.trim_start();
        if next.starts_with("::new") || next.starts_with("as ") {
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
            let body = &body[..end];
            let pattern = body
                .rfind("..")
                .is_some_and(|d| body[d + 2..].trim().is_empty());
            if !pattern {
                return true;
            }
        }
    }
    false
}

/// Every hit in one file's comment-stripped source, as `token: line n`.
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
            found.push(format!("{name} constructed or aliased"));
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
        paths.len() > 300,
        "the scan found only {} files under {}: wrong root?",
        paths.len(),
        root.display()
    );

    let mut violations = Vec::new();
    let mut allowed_seen = vec![false; ALLOWED_FROM.len()];
    for path in &paths {
        let rel = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if rel == THIS_FILE {
            continue;
        }
        let src = fs::read_to_string(path).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mut code = strip_comments(&src, lang_of(ext));
        if let Some(k) = ALLOWED_FROM.iter().position(|(p, _)| rel.starts_with(p)) {
            allowed_seen[k] = true;
            match ALLOWED_FROM[k].1 {
                None => continue,
                Some(marker) => {
                    let cut = code
                        .find(marker)
                        .unwrap_or_else(|| panic!("{rel} lost its `{marker}` marker"));
                    code.truncate(cut);
                }
            }
        }
        violations.extend(hits(&code).into_iter().map(|h| format!("{rel}: {h}")));
    }
    assert!(
        allowed_seen.iter().all(|&seen| seen),
        "an allow-listed path moved: {ALLOWED_FROM:?}"
    );
    assert!(
        violations.is_empty(),
        "operator-key signing path found (MG-01 §4):\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_finds_what_it_must() {
    let rust = |s: &str| hits(&strip_comments(s, Lang::Rust));
    // Each of these would be a hit…
    for bad in [
        "let p = ProviderUpdateServicePayload::new(a, b);",
        "ProviderUpdateRevocationPayload { version: 2, reason, payload_sig }",
        "let p = ProviderUpdateRevocationPayload {\n    version: 2,\n    reason: 0,\n};",
        "let p = ProviderUpdateServicePayload { payload_sig: s, ..decoded };",
        "use x::ProviderUpdateServicePayload as P;",
        "pub fn finalize_revoke(tx: &Transaction) {}",
        "let sk = BlsSecretKey::<BlsScIetf>::from_ikm(&ikm);",
        "use dashcore::blsful::SecretKey;",
        "let k = dw_coinjoin::bls::testing::OperatorKey::from_seed(b\"x\");",
        // Code after a string that holds a comment opener is still read.
        "let s = \"'/*abc\"; finalize_revoke();",
        "let u = \"https://x\"; finalize_revoke();",
        "let c = '\"'; finalize_revoke(); let d = '\"';",
        "let r = r#\"/* \"# ; finalize_revoke();",
        "fn f<'a>(x: &'a str) { finalize_revoke() }",
        // A name inside a string (an IPC command) is read.
        "invoke(\"protx_update_service\")",
    ] {
        assert!(!rust(bad).is_empty(), "missed: {bad}");
    }
    assert!(
        !hits(&strip_comments(
            "await invoke('prepareUpdateService', a) // x",
            Lang::Ts
        ))
        .is_empty()
    );
    assert!(!hits(&strip_comments("let s = '/*'; prepareRevoke()", Lang::Ts)).is_empty());
    // …and these are not.
    for ok in [
        "TransactionPayload::ProviderUpdateServicePayloadType(p) => track(p),",
        "if let ProviderUpdateRevocationPayload { pro_tx_hash, .. } = p {}",
        "SpecialType::ProviderUpdateService => \"service update\",",
        "// finalize_revoke is gone (MG-01 D6)",
        "/* BlsSecretKey\n /* nested */ never */ let x = 1;",
        "let pk = BlsPkBytes::from_bytes(bytes);",
    ] {
        assert!(rust(ok).is_empty(), "false hit: {ok}");
    }
    // Line numbers survive a block comment.
    assert_eq!(
        rust("/* a\nb */\nfinalize_revoke();"),
        ["finalize_revoke: line 3"]
    );
}
