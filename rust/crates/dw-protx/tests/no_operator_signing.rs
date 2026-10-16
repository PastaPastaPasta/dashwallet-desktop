//! MG-01 acceptance: no operator-key signing path remains (repo CLAUDE.md:
//! the wallet never signs with an operator key; docs/design/MG-01-salvage.md
//! §4). Two checks share the job:
//!
//! - `rust/clippy.toml` disallows the BLS secret types and signers and
//!   platform-wallet's ProUpServTx builders by definition, so aliases and
//!   re-exports cannot hide them (the gate's `cargo clippy -D warnings`).
//!   Clippy reads only the nearest `clippy.toml`, so a crate with its own
//!   must repeat the entries; [`every_clippy_toml_carries_the_mg01_rules`]
//!   checks that;
//! - this test scans the sources clippy does not see or cannot judge by
//!   type (Rust, the desktop UI, Swift) for the operator-signed payload
//!   types, the names of their builders, IPC and console commands, and BLS
//!   secret material and signers, and fails on any hit outside the
//!   allow-list. Rust is read with comments removed; the UI and Swift are
//!   read as they are (comments included), so no lexing can hide a name
//!   there.
//!
//! The one BLS secret allowed, dw-coinjoin's test operator key, is kept out
//! of every library build: [`dash_pkc_stays_a_dev_dependency`] checks the
//! manifests (MG-01 D21).
//!
//! Rerun: `cd rust && cargo test -p dw-protx --test no_operator_signing`.
//!
//! Reading those payloads off the chain stays possible without naming
//! their types (MG-03 tracks operator-key changes, MG-00 D46): match the
//! `TransactionPayload::ProviderUpdate…PayloadType(p)` variant and read
//! `p`'s fields. Naming the struct needs an allow-list entry reviewed
//! against this rule (MG-01 D20): telling a read from a build by text took
//! three review rounds and still had holes.

use std::fs;
use std::path::{Path, PathBuf};

/// Payloads only the operator key can sign, matched as whole identifiers
/// (the `…PayloadType` enum variants stay allowed).
const PAYLOAD_TYPES: &[&str] = &[
    "ProviderUpdateServicePayload",
    "ProviderUpdateRevocationPayload",
];

/// Builders, IPC commands and console commands of the operator-signed
/// transactions (the r3 names and their camelCase / Swift forms), BLS
/// secret material and signers (dash-pkc, dashcore, dpp, key-wallet), the
/// CoinJoin test operator key and the file and module that hold it, and
/// `include!` (no crate includes code that way; it could pull the test key
/// into a library from a generated or renamed file).
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
    "bls::SecretKey",
    "BlsModule",
    "ExtendedBLSPrivKey",
    "operator_private_key_at",
    "to_bls_extended_priv_key",
    "OperatorKey",
    "bls::testing",
    "operator_key.rs",
    "test_operator_key",
    "include!(",
];

/// Where hits are allowed. Only dw-coinjoin's test operator key and the
/// tests that use it, which play a masternode signing CoinJoin queue
/// messages (MG-01 D9); they sign no wallet transaction. None of it is in a
/// library build (D21): `dash-pkc` is a dev-dependency only
/// ([`dash_pkc_stays_a_dev_dependency`]), and [`region_escapes`] closes the
/// ways around a region that name no forbidden token.
enum Allowed {
    /// The whole file: an integration-test file, which only `cargo test`
    /// builds compile (a library can include it only with a `#[path]` or
    /// `include!` the scan catches).
    File,
    /// One inline `#[cfg(test)] mod tests`: from the marker, which must open
    /// a line and occur exactly once, through the module's closing `}` at
    /// column 0. Every line between must be blank or indented, so code after
    /// the module (rustfmt puts its `}` at column 0) is read.
    TestModule,
    /// Exactly this text, which must open a line and occur once.
    Text(&'static str),
}

const TEST_MODULE: &str = "#[cfg(test)]\nmod tests {\n";

const ALLOWED: &[(&str, Allowed)] = &[
    (
        "rust/crates/dw-coinjoin/src/lib.rs",
        Allowed::Text(
            "#[cfg(test)]\n#[path = \"../tests/support/operator_key.rs\"]\nmod test_operator_key;\n",
        ),
    ),
    ("rust/crates/dw-coinjoin/src/bls.rs", Allowed::TestModule),
    ("rust/crates/dw-coinjoin/src/queue.rs", Allowed::TestModule),
    (
        "rust/crates/dw-coinjoin/tests/support/operator_key.rs",
        Allowed::File,
    ),
    ("rust/crates/dw-coinjoin/tests/session.rs", Allowed::File),
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

/// The files under `rust/crates` and `rust/bin` with one of `names`.
fn rust_files_named(names: &[&str]) -> Vec<PathBuf> {
    let rust = repo_root().join("rust");
    let mut found = Vec::new();
    files(&rust.join("crates"), &["toml"], &mut found);
    files(&rust.join("bin"), &["toml"], &mut found);
    found.retain(|p| p.file_name().is_some_and(|n| names.iter().any(|m| n == *m)));
    found
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

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Rust source with comments removed and string contents kept (IPC command
/// names live in strings). Newlines inside comments are kept, so line
/// numbers stay right. Handles `//` and nested `/* */` comments, `"…"`
/// strings with escapes, raw strings (`r#"…"#`, `br"…"`, `cr"…"`) and char
/// literals (a `'` that does not close within one char is a lifetime).
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        // Where a raw string's `r` would be: `r…`, or `br…` / `cr…`.
        let raw_r = match c {
            b'r' => Some(i),
            b'b' | b'c' if next == Some(b'r') => Some(i + 1),
            _ => None,
        };
        let raw_hashes = raw_r
            .filter(|_| i == 0 || !is_ident(b[i - 1]))
            .map(|r| (r, b[r + 1..].iter().take_while(|&&h| h == b'#').count()))
            .filter(|&(r, hashes)| b.get(r + 1 + hashes) == Some(&b'"'));
        if c == b'/' && next == Some(b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && next == Some(b'*') {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == b'*' && b.get(i + 1) == Some(&b'/') {
                    depth -= 1;
                    i += 2;
                } else if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
                    depth += 1;
                    i += 2;
                } else {
                    if b[i] == b'\n' {
                        out.push(b'\n');
                    }
                    i += 1;
                }
            }
        } else if let Some((r, hashes)) = raw_hashes {
            let close: Vec<u8> = std::iter::once(b'"')
                .chain(std::iter::repeat_n(b'#', hashes))
                .collect();
            let body = r + 2 + hashes;
            let end = b[body..]
                .windows(close.len())
                .position(|w| w == close.as_slice())
                .map_or(b.len(), |p| body + p + close.len());
            out.extend_from_slice(&b[i..end]);
            i = end;
        } else if c == b'"' {
            out.push(c);
            i += 1;
            while i < b.len() {
                let s = b[i];
                out.push(s);
                i += 1;
                if s == b'\\' && i < b.len() {
                    out.push(b[i]);
                    i += 1;
                } else if s == b'"' {
                    break;
                }
            }
        } else if c == b'\'' {
            // `'x'`, `'\n'`, `'\u{…}'`: a char literal; otherwise a lifetime.
            // The escaped char sits at `i + 2` and may be a quote: `'\''`.
            let len = if next == Some(b'\\') {
                b.get(i + 3..)
                    .and_then(|rest| rest.iter().position(|&q| q == b'\''))
                    .map(|p| p + 4)
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

/// Whether `line` names `ident` as a whole identifier.
fn names(line: &str, ident: &str) -> bool {
    line.match_indices(ident).any(|(at, _)| {
        let b = line.as_bytes();
        let end = at + ident.len();
        (at == 0 || !is_ident(b[at - 1])) && b.get(end).is_none_or(|&c| !is_ident(c))
    })
}

/// Every hit in one file's source, as `token: line n`.
fn hits(code: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (n, line) in code.lines().enumerate() {
        for token in FORBIDDEN {
            if line.contains(token) {
                found.push(format!("{token}: line {}", n + 1));
            }
        }
        for name in PAYLOAD_TYPES {
            if names(line, name) {
                found.push(format!("{name}: line {}", n + 1));
            }
        }
        // A `#[path]` whose file name is escaped (`operat\x6fr_key.rs`) or
        // built by a macro hides which file it includes.
        if line.contains("#[path") && (line.contains('\\') || line.contains('!')) {
            found.push(format!("#[path] with an escape or macro: line {}", n + 1));
        }
    }
    found
}

/// `code` with the allowed region blanked (newlines kept, so line numbers
/// stay right), or why the region is not where it must be.
fn without_allowed(code: &str, allowed: &Allowed) -> Result<String, String> {
    let marker = match allowed {
        Allowed::File => return Ok(String::new()),
        Allowed::TestModule => TEST_MODULE,
        Allowed::Text(text) => text,
    };
    let starts: Vec<usize> = code.match_indices(marker).map(|(at, _)| at).collect();
    let [start] = starts[..] else {
        return Err(format!("must hold `{marker}` exactly once"));
    };
    if start > 0 && code.as_bytes()[start - 1] != b'\n' {
        return Err(format!("`{marker}` must open a line"));
    }
    let mut end = start + marker.len();
    if matches!(allowed, Allowed::TestModule) {
        loop {
            let line_end = code[end..].find('\n').map_or(code.len(), |p| end + p + 1);
            let line = &code[end..line_end];
            end = line_end;
            if line.trim_end() == "}" {
                break;
            }
            let at_eof = line.is_empty();
            let blank_or_indented = line.trim().is_empty() || line.starts_with([' ', '\t']);
            if at_eof || !blank_or_indented {
                return Err(format!(
                    "the `#[cfg(test)] mod tests` must end with a `}}` line before any \
                     unindented line (line {})",
                    code[..end].lines().count()
                ));
            }
        }
    }
    let blank: String = code[start..end]
        .chars()
        .map(|c| if c == '\n' { c } else { ' ' })
        .collect();
    Ok(format!("{}{blank}{}", &code[..start], &code[end..]))
}

/// Ways around the allowed regions that name no forbidden token (MG-01
/// D21): a `path` attribute in dw-coinjoin's library outside the allowed
/// include (written over several lines, in a `cfg_attr`, or to a `mod.rs`
/// that declares the key's module), and a rustfmt skip (`rustfmt::skip`,
/// with or without spaces, or the old `rustfmt_skip`) in a file with an
/// allowed region, whose end rustfmt's layout keeps honest. `code` is the file as
/// scanned, `blanked` with its allowed region removed.
fn region_escapes(rel: &str, code: &str, blanked: &str) -> Vec<String> {
    let mut found = Vec::new();
    let squeezed: String = blanked.chars().filter(|c| !c.is_whitespace()).collect();
    if rel.starts_with("rust/crates/dw-coinjoin/src/")
        && ["[path=", ",path=", "(path="]
            .iter()
            .any(|p| squeezed.contains(p))
    {
        found.push("a `path` attribute outside the allowed include".to_string());
    }
    let squeezed_code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    if squeezed_code.contains("rustfmt::skip") || squeezed_code.contains("rustfmt_skip") {
        found.push("a rustfmt skip beside an allowed region".to_string());
    }
    found
}

/// What the scan reads of one file: Rust without comments, the rest as is,
/// with CRLF line ends normalised (the allow-list markers span lines).
fn scanned(src: &str, ext: &str) -> String {
    let src = src.replace("\r\n", "\n");
    if ext == "rs" {
        strip_comments(&src)
    } else {
        src
    }
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
    let mut allowed_seen = vec![false; ALLOWED.len()];
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
        let mut code = scanned(&src, ext);
        if let Some(k) = ALLOWED.iter().position(|(p, _)| rel == *p) {
            allowed_seen[k] = true;
            let blanked =
                without_allowed(&code, &ALLOWED[k].1).unwrap_or_else(|e| panic!("{rel}: {e}"));
            if !matches!(ALLOWED[k].1, Allowed::File) {
                let escapes = region_escapes(&rel, &code, &blanked);
                violations.extend(escapes.into_iter().map(|e| format!("{rel}: {e}")));
            }
            code = blanked;
        } else if rel.starts_with("rust/crates/dw-coinjoin/src/") {
            let escapes = region_escapes(&rel, "", &code);
            violations.extend(escapes.into_iter().map(|e| format!("{rel}: {e}")));
        }
        violations.extend(hits(&code).into_iter().map(|h| format!("{rel}: {h}")));
    }
    // No rustfmt config may switch formatting off where the allowed regions
    // are (rustfmt reads the nearest one up from each file).
    for dir in ["", "rust", "rust/crates", "rust/crates/dw-coinjoin"] {
        for name in ["rustfmt.toml", ".rustfmt.toml"] {
            if let Ok(config) = fs::read_to_string(root.join(dir).join(name)) {
                assert!(
                    !config.contains("ignore") && !config.contains("disable_all_formatting"),
                    "{dir}/{name} turns rustfmt off near the allowed regions"
                );
            }
        }
    }
    assert!(
        allowed_seen.iter().all(|&seen| seen),
        "an allow-listed path moved: {:?}",
        ALLOWED.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
    assert!(
        violations.is_empty(),
        "operator-key signing path found (MG-01 §4):\n{}",
        violations.join("\n")
    );
}

/// The `disallowed-types` and `disallowed-methods` paths of a clippy.toml,
/// ignoring commented-out lines.
fn clippy_rules(text: &str) -> [Vec<String>; 2] {
    let mut rules = [Vec::new(), Vec::new()];
    let mut section = None;
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        if line.starts_with("disallowed-types") {
            section = Some(0);
        } else if line.starts_with("disallowed-methods") {
            section = Some(1);
        } else if line.starts_with(']') {
            section = None;
        }
        if let (Some(k), Some((_, rest))) = (section, line.split_once("path = \"")) {
            rules[k].push(rest[..rest.find('"').unwrap()].to_string());
        }
    }
    rules
}

/// Clippy uses the nearest `clippy.toml` (or `.clippy.toml`) only, with no
/// merging, so every one under `rust/` must carry each disallowed type and
/// method of `rust/clippy.toml`, in the same section.
#[test]
fn every_clippy_toml_carries_the_mg01_rules() {
    let rust = repo_root().join("rust");
    let root = clippy_rules(&fs::read_to_string(rust.join("clippy.toml")).unwrap());
    assert!(
        root[0].len() >= 7 && root[1].len() >= 5,
        "rust/clippy.toml lost its rules"
    );
    let tomls = rust_files_named(&["clippy.toml", ".clippy.toml"]);
    assert!(!tomls.is_empty(), "dw-ffi's clippy.toml is expected");
    for toml in tomls {
        let have = clippy_rules(&fs::read_to_string(&toml).unwrap());
        for (k, section) in ["disallowed-types", "disallowed-methods"]
            .iter()
            .enumerate()
        {
            for rule in &root[k] {
                assert!(
                    have[k].contains(rule),
                    "{} lacks the MG-01 rule {rule} in {section}",
                    toml.display()
                );
            }
        }
    }
    // The parser skips comments and keeps sections apart.
    let fake = "disallowed-types = [\n# { path = \"a::A\" },\n]\ndisallowed-methods = [\n{ path = \"b::b\" },\n]";
    assert_eq!(clippy_rules(fake), [vec![], vec!["b::b".to_string()]]);
}

#[test]
fn the_scanner_finds_what_it_must() {
    let rust = |s: &str| hits(&strip_comments(s));
    // Each of these would be a hit…
    for bad in [
        "let p = ProviderUpdateServicePayload::new(a, b);",
        "ProviderUpdateRevocationPayload { version: 2, reason, payload_sig }",
        "let p = ProviderUpdateRevocationPayload {\n    version: 2,\n    reason: 0,\n};",
        "let p = ProviderUpdateServicePayload { payload_sig: s, ..decoded };",
        "use x::ProviderUpdateServicePayload as P;",
        "type P = ProviderUpdateRevocationPayload;",
        // Any naming of the struct, even where it is not built here.
        "impl Unsigned for ProviderUpdateServicePayload {\n    fn u() -> Self { Self { version: 2 } }\n}",
        "impl From<Draft> for ProviderUpdateRevocationPayload { fn from(d: Draft) -> Self { todo!() } }",
        "fn read(r: &mut R) -> ProviderUpdateServicePayload {\n    decode(r)\n}",
        "if let ProviderUpdateRevocationPayload { pro_tx_hash, .. } = p {}",
        "let k = x.to_bls_extended_priv_key(net)?;",
        "pub fn finalize_revoke(tx: &Transaction) {}",
        "let sk = BlsSecretKey::<BlsScIetf>::from_ikm(&ikm);",
        "use dashcore::blsful::SecretKey;",
        "let s = dpp::bls::SecretKey::from_be_bytes(&op)?.sign(msg);",
        "NativeBlsModule.sign(data, &op)",
        "let k = dw_coinjoin::bls::testing::OperatorKey::from_seed(b\"x\");",
        // Code after a string that holds a comment opener is still read.
        "let s = \"'/*abc\"; finalize_revoke();",
        "let u = \"https://x\"; finalize_revoke();",
        "let c = '\"'; finalize_revoke(); let d = '\"';",
        // A raw string ending in `\` read as an escaped string would turn
        // the next string's `/*` into a comment.
        "let r = r\"\\\"; let s = \"/*\"; finalize_revoke(); let t = \"*/\";",
        "let r = br#\"\\\"#; let s = \"/*\"; finalize_revoke(); let t = \"*/\";",
        "fn f<'a>(x: &'a str) { finalize_revoke() }",
        // `'\''` is one char literal, so the quotes after it pair up.
        "let q = ['\\'','\"']; let s = \"/*\"; finalize_revoke(); let t = \"*/\";",
        // A name inside a string (an IPC command) is read.
        "invoke(\"protx_update_service\")",
    ] {
        assert!(!rust(bad).is_empty(), "missed: {bad}");
    }
    // The UI and Swift are read raw: an apostrophe in JSX text cannot hide
    // a later line.
    let ui = "<p>the wallet's key</p>\nconst u = 'https://x'; invoke('prepareRevoke')";
    assert!(!hits(&scanned(ui, "tsx")).is_empty());
    // A CRLF checkout still finds a two-line marker.
    assert_eq!(
        scanned("#[cfg(test)]\r\nmod tests", "rs"),
        "#[cfg(test)]\nmod tests"
    );
    // …and these are not.
    for ok in [
        "TransactionPayload::ProviderUpdateServicePayloadType(p) => track(&p.pro_tx_hash),",
        "Some(TransactionPayload::ProviderUpdateRevocationPayloadType(p)) => p.reason,",
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

/// The lines of a manifest that name dash-pkc outside a dev-dependency
/// table, and how many name it inside one. A `[dependencies.dash-pkc]`
/// header counts as naming it under its own table.
fn dash_pkc_uses(manifest: &str) -> (Vec<String>, usize) {
    let (mut outside, mut dev) = (Vec::new(), 0);
    let mut section = "";
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        }
        if line.starts_with('#') || !line.contains("dash-pkc") {
            continue;
        }
        let table = section.trim_matches(['[', ']']);
        if table == "dev-dependencies"
            || table.starts_with("dev-dependencies.")
            || table.contains(".dev-dependencies")
        {
            dev += 1;
        } else {
            outside.push(format!("{line} (under [{table}])"));
        }
    }
    (outside, dev)
}

/// Lines of the workspace manifest that name dash-pkc under another key
/// (`pkc = { package = "dash-pkc", … }`): a member's `pkc.workspace = true`
/// would then not say `dash-pkc`.
fn dash_pkc_aliases(root: &str) -> Vec<String> {
    root.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && l.contains("dash-pkc"))
        .filter(|l| !l.starts_with("dash-pkc") && *l != "[workspace.dependencies.dash-pkc]")
        .map(str::to_string)
        .collect()
}

/// No library build can name `dash_pkc`: every member crate lists it under
/// `[dev-dependencies]` only, and the workspace names it by its own name
/// (MG-01 D21). dashcore's BLS secret types are behind clippy's
/// `disallowed-types`.
#[test]
fn dash_pkc_stays_a_dev_dependency() {
    let root = fs::read_to_string(repo_root().join("rust/Cargo.toml")).unwrap();
    assert_eq!(dash_pkc_aliases(&root), Vec::<String>::new());
    assert!(!dash_pkc_aliases("pkc = { package = \"dash-pkc\", git = \"x\" }").is_empty());
    assert!(
        dash_pkc_aliases("dash-pkc = { git = \"x\" }\n[workspace.dependencies.dash-pkc]")
            .is_empty()
    );
    let manifests = rust_files_named(&["Cargo.toml"]);
    assert!(manifests.len() > 10, "too few member manifests");
    let mut dev_uses = 0;
    for manifest in manifests {
        let (outside, dev) = dash_pkc_uses(&fs::read_to_string(&manifest).unwrap());
        assert!(
            outside.is_empty(),
            "{} names dash-pkc outside dev-dependencies: {outside:?}",
            manifest.display()
        );
        dev_uses += dev;
    }
    assert!(dev_uses > 0, "dw-coinjoin's test key needs dash-pkc");
    // Every form a normal dependency can take is caught.
    for bad in [
        "[dependencies]\ndash-pkc = { workspace = true }",
        "[dependencies.dash-pkc]\nworkspace = true",
        "[target.'cfg(unix)'.dependencies]\ndash-pkc = \"1\"",
        "[build-dependencies]\ndash-pkc.workspace = true",
        "[dependencies]\npkc = { package = \"dash-pkc\" }",
        "dependencies.dash-pkc.workspace = true\n[package]",
    ] {
        assert!(!dash_pkc_uses(bad).0.is_empty(), "missed: {bad}");
    }
    for ok in [
        "[dev-dependencies]\ndash-pkc = { workspace = true }",
        "[dev-dependencies.dash-pkc]\nworkspace = true",
        "[target.'cfg(unix)'.dev-dependencies]\ndash-pkc = \"1\"",
    ] {
        assert_eq!(dash_pkc_uses(ok), (vec![], 1), "{ok}");
    }
}

#[test]
fn allowed_regions_are_exact() {
    let module = "fn a() {}\n#[cfg(test)]\nmod tests {\n    use x::OperatorKey;\n\n    fn t() {\n        k();\n    }\n}\n";
    let blanked = without_allowed(module, &Allowed::TestModule).unwrap();
    assert!(hits(&blanked).is_empty());
    assert_eq!(blanked.lines().count(), module.lines().count());
    // Code after the module is read, whether appended or re-exported.
    for after in [
        "pub fn sign_any(s: &[u8], d: &[u8; 32]) -> Vec<u8> {\n    OperatorKey::from_seed(s).sign(d)\n}\n",
        "pub use crate::test_operator_key::OperatorKey as Signer;\n",
    ] {
        let code = without_allowed(&format!("{module}{after}"), &Allowed::TestModule).unwrap();
        assert!(!hits(&code).is_empty(), "missed after the module: {after}");
    }
    // A module closed on its marker line, or a second marker, cannot stretch
    // the region over later code.
    for bad in [
        "#[cfg(test)]\nmod tests {}\npub fn s() {\n    OperatorKey::x()\n}\n".to_string(),
        format!("{module}{module}"),
        "#[cfg(test)]\nmod tests {\n    fn t() {}\n".to_string(),
        // Not at the start of a line (inside a string, say).
        "let s = \"x#[cfg(test)]\nmod tests {\n\";\n}\n".to_string(),
    ] {
        assert!(
            without_allowed(&bad, &Allowed::TestModule).is_err(),
            "accepted: {bad}"
        );
    }
    // Losing `#[cfg(test)]` on the included key loses the allowance.
    let text = Allowed::Text("#[cfg(test)]\nmod k;\n");
    assert!(without_allowed("#[cfg(test)]\nmod k;\n", &text).is_ok());
    assert!(without_allowed("mod k;\n", &text).is_err());
    let tail = without_allowed("#[cfg(test)]\nmod k;\npub use k::OperatorKey;\n", &text).unwrap();
    assert_eq!(hits(&tail), ["OperatorKey: line 3"]);
    // Includes that could pull the key into a library.
    for bad in [
        "#[path = \"../tests/support/operator_key.rs\"]\nmod k;",
        "#[path = \"../tests/support/operat\\x6fr_key.rs\"]\nmod k;",
        "#[path = concat!(env!(\"OUT_DIR\"), \"/k.rs\")]\nmod k;",
        "include!(concat!(env!(\"OUT_DIR\"), \"/k.rs\"));",
    ] {
        assert!(!hits(&strip_comments(bad)).is_empty(), "missed: {bad}");
    }
    assert!(hits("#[path = \"names_tests.rs\"]\nmod tests;").is_empty());
    // A `path` attribute in dw-coinjoin's library outside the allowed
    // include, however it is written, and `rustfmt::skip` beside a region.
    let lib = "rust/crates/dw-coinjoin/src/lib.rs";
    for bad in [
        "#[cfg_attr(all(), path = \"../tests/support/k.rs\")]\nmod k;",
        "#[path\n    = \"../tests/support/mod.rs\"]\nmod support;",
    ] {
        assert!(!region_escapes(lib, "", bad).is_empty(), "missed: {bad}");
    }
    for skip in [
        "#![rustfmt::skip]\n",
        "#![cfg_attr(rustfmt, rustfmt_skip)]\n",
        "#[rustfmt :: skip]\nmod bls;\n",
    ] {
        assert!(!region_escapes(lib, skip, "").is_empty(), "missed: {skip}");
    }
    assert!(region_escapes(lib, "", "let path = here();").is_empty());
    let engine = "rust/crates/dw-engine/src/platform/names.rs";
    assert!(region_escapes(engine, "", "#[path = \"names_net.rs\"]\nmod net;").is_empty());
}
