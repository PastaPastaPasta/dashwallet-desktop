//! Facade coverage (E0-13 D-row "coverage"): every public method of the
//! engine facades, and the free functions hosts call, is reached by a
//! command (its `#[facade]` claim) or excluded with a reason (a domain's
//! `EXCLUDED`), never both and never neither.
//!
//! The facade methods are read from the dw-engine and dw-vault sources, so
//! a method added to a facade fails `facades_are_covered` until it is bound
//! or excluded. `docs/design/E0-13-facade-coverage.md` is the generated
//! table, checked like the bindings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The facade types: `(type, crate directory)`.
pub const FACADES: &[(&str, &str)] = &[
    ("Engine", "dw-engine"),
    ("NetworkSession", "dw-engine"),
    ("DashPay", "dw-engine"),
    ("TxDraft", "dw-engine"),
    ("PreparedTx", "dw-engine"),
    ("Vault", "dw-vault"),
];

/// Free functions hosts call (dw-ffi binds the first three): `(claim, file)`.
pub const FREE: &[(&str, &str)] = &[
    ("dw_engine::fee_policy", "dw-engine/src/fees.rs"),
    ("dw_vault::mnemonic::generate", "dw-vault/src/mnemonic.rs"),
    ("dw_vault::mnemonic::check", "dw-vault/src/mnemonic.rs"),
    (
        "dw_engine::check_username",
        "dw-engine/src/platform/names.rs",
    ),
    ("dw_engine::backup::read_header", "dw-engine/src/backup.rs"),
    (
        "dw_engine::is_coinjoin_internal",
        "dw-engine/src/tx_actions.rs",
    ),
];

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The coverage table's path.
pub fn doc_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/design/E0-13-facade-coverage.md")
}

/// Removes comments and the contents of string and char literals, keeping
/// their delimiters and every newline, so braces in them do not count.
fn strip(src: &str) -> String {
    let c: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                let mut depth = 0;
                while i < c.len() {
                    if c[i] == '/' && c.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if c[i] == '*' && c.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        if c[i] == '\n' {
                            out.push('\n');
                        }
                        i += 1;
                    }
                }
            }
            'r' if matches!(c.get(i + 1), Some('"' | '#'))
                && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while c.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if c.get(j) != Some(&'"') {
                    out.push(c[i]);
                    i += 1;
                    continue;
                }
                j += 1;
                out.push_str("\"\"");
                while j < c.len() {
                    if c[j] == '"' && (1..=hashes).all(|k| c.get(j + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    if c[j] == '\n' {
                        out.push('\n');
                    }
                    j += 1;
                }
                i = j;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        i += 1;
                    } else if c[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i += 1;
                out.push_str("\"\"");
            }
            '\'' => {
                // A char literal ('x', '\n', '\u{..}'), else a lifetime.
                if c.get(i + 1) == Some(&'\\') {
                    i += 2;
                    while i < c.len() && c[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                    out.push_str("' '");
                } else if c.get(i + 2) == Some(&'\'') {
                    i += 3;
                    out.push_str("' '");
                } else {
                    out.push('\'');
                    i += 1;
                }
            }
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    out
}

/// The `pub fn` names declared directly in `impl <ty> {` blocks (inherent
/// impls; `pub(crate)` and trait impls are not the facade).
fn methods_of(src: &str, ty: &str) -> Vec<String> {
    let src = strip(src);
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut search = 0;
    while let Some(pos) = src[search..].find("impl") {
        let start = search + pos;
        search = start + 4;
        let at_line_start = src[..start]
            .rsplit('\n')
            .next()
            .is_some_and(|l| l.trim().is_empty());
        if !at_line_start {
            continue;
        }
        let Some(brace) = src[start..].find('{').map(|b| start + b) else {
            break;
        };
        let header = src[start + 4..brace].trim();
        let header = header
            .strip_prefix('<')
            .and_then(|h| h.split_once('>'))
            .map_or(header, |(_, rest)| rest.trim());
        let named = header.strip_prefix("crate::").unwrap_or(header);
        if named != ty {
            continue;
        }
        // Lines that start directly in the impl's body (depth 1) declare its items.
        let mut depth = 1usize;
        let mut line_start = brace + 1;
        let mut line_depth = 1;
        let mut i = brace + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        // `impl X { pub fn f() {} }` on one line.
                        if line_depth == 1
                            && let Some(name) = pub_fn_name(&src[line_start..i])
                        {
                            out.push(name);
                        }
                        break;
                    }
                }
                b'\n' => {
                    if line_depth == 1
                        && let Some(name) = pub_fn_name(&src[line_start..i])
                    {
                        out.push(name);
                    }
                    line_start = i + 1;
                    line_depth = depth;
                }
                _ => {}
            }
            i += 1;
        }
        search = i;
    }
    out
}

/// `f` for a line declaring `pub fn f`, `pub async fn f` or `pub const fn f`.
fn pub_fn_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("pub ")?;
    let rest = rest.strip_prefix("async ").unwrap_or(rest);
    let rest = rest.strip_prefix("const ").unwrap_or(rest);
    let rest = rest.strip_prefix("fn ")?;
    Some(
        rest.chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect(),
    )
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "tests" {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
            out.push(path);
        }
    }
}

/// Every facade method as `Type::method`, with the file declaring it
/// (relative to `rust/crates`).
pub fn facade_methods() -> BTreeMap<String, String> {
    let root = crates_dir();
    let mut out = BTreeMap::new();
    for (ty, krate) in FACADES {
        let mut files = Vec::new();
        rust_files(&root.join(krate).join("src"), &mut files);
        files.sort();
        for file in files {
            let src = std::fs::read_to_string(&file).unwrap_or_default();
            for m in methods_of(&src, ty) {
                let rel = file.strip_prefix(&root).unwrap_or(&file);
                out.insert(format!("{ty}::{m}"), rel.display().to_string());
            }
        }
    }
    for (claim, file) in FREE {
        out.insert((*claim).to_string(), (*file).to_string());
    }
    out
}

/// What each facade method maps to.
#[derive(Debug, Default)]
pub struct Coverage {
    /// Facade method → the commands claiming it.
    pub bound: BTreeMap<String, BTreeSet<&'static str>>,
    /// Facade method → (domain, reason).
    pub excluded: BTreeMap<String, (&'static str, &'static str)>,
    /// Problems: unknown claims, methods in neither or both lists, duplicates.
    pub errors: Vec<String>,
}

pub fn coverage(methods: &BTreeMap<String, String>) -> Coverage {
    let mut cov = Coverage::default();
    for (_, spec) in crate::api::specs() {
        for claim in spec.facade {
            if !methods.contains_key(*claim) {
                cov.errors.push(format!(
                    "{}: claims unknown facade method {claim}",
                    spec.name
                ));
            }
            cov.bound
                .entry((*claim).to_string())
                .or_default()
                .insert(spec.name);
        }
    }
    for (domain, method, why) in crate::api::excluded() {
        if !methods.contains_key(method) {
            cov.errors
                .push(format!("{domain}: excludes unknown facade method {method}"));
        }
        if cov.bound.contains_key(method) {
            cov.errors
                .push(format!("{method} is both bound and excluded ({domain})"));
        }
        if cov
            .excluded
            .insert(method.to_string(), (domain, why))
            .is_some()
        {
            cov.errors.push(format!("{method} is excluded twice"));
        }
    }
    for method in methods.keys() {
        if !cov.bound.contains_key(method) && !cov.excluded.contains_key(method) {
            cov.errors.push(format!(
                "{method} has no command and no exclusion (add a #[facade] claim or an EXCLUDED row)"
            ));
        }
    }
    cov
}

/// The coverage table as Markdown.
pub fn render(methods: &BTreeMap<String, String>, cov: &Coverage) -> String {
    let bound = methods
        .keys()
        .filter(|m| cov.bound.contains_key(*m))
        .count();
    let excluded = methods
        .keys()
        .filter(|m| cov.excluded.contains_key(*m))
        .count();
    let mut md = format!(
        "# E0-13 facade coverage\n\n\
         Generated by dw-app (`DW_UPDATE_BINDINGS=1 cargo test -p dw-app coverage`); do not edit.\n\
         Every public facade method, and each free function hosts call, is reached by a command\n\
         or excluded with a reason (`rust/crates/dw-app/src/coverage.rs`).\n\n\
         {} methods: {bound} bound, {excluded} excluded.\n\n\
         | Facade method | Source | Command(s) or exclusion |\n|---|---|---|\n",
        methods.len(),
    );
    for (method, file) in methods {
        let cell = match (cov.bound.get(method), cov.excluded.get(method)) {
            (Some(cmds), _) => cmds
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", "),
            (None, Some((domain, why))) => format!("excluded ({domain}): {why}"),
            (None, None) => "**missing**".into(),
        };
        let _ = writeln!(md, "| `{method}` | {file} | {cell} |");
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_reads_inherent_pub_methods_only() {
        let src = r#####"
impl Engine {
    pub fn a(&self) -> &str { "}{" }
    pub fn multi(
        &self,
    ) -> u8 {
        if true { 1 } else { 2 }
    }
    pub async fn b<F: Fn() -> ()>(&self, f: F) { let _ = '{'; }
    pub(crate) fn hidden(&self) {}
    fn private(&self) { if true { } }
    // pub fn commented() {}
    pub const fn c() -> char { '\'' }
    pub fn d<'a>(&'a self) -> &'a str { r#"{"# }
}
impl Drop for Engine { fn drop(&mut self) {} }
impl<T> Engine { pub fn e(&self) {} }
impl crate::Engine {
    pub fn f(&self) {}
}
impl EngineConfig { pub fn g(&self) {} }
"#####;
        assert_eq!(
            methods_of(src, "Engine"),
            ["a", "multi", "b", "c", "d", "e", "f"]
        );
    }

    #[test]
    fn facades_are_covered() {
        let methods = facade_methods();
        // The scan found the facades (a broken scan must not pass vacuously).
        for (ty, _) in FACADES {
            let n = methods
                .keys()
                .filter(|m| m.starts_with(&format!("{ty}::")))
                .count();
            assert!(n > 0, "no methods found for {ty}");
        }
        assert!(methods.len() > 200, "{} methods", methods.len());
        for (claim, file) in FREE {
            let name = claim.rsplit("::").next().unwrap();
            let src = std::fs::read_to_string(crates_dir().join(file)).unwrap();
            assert!(
                src.contains(&format!("pub fn {name}(")),
                "{claim} is not in {file}"
            );
        }
        let cov = coverage(&methods);
        assert!(cov.errors.is_empty(), "{}", cov.errors.join("\n"));
    }

    /// The committed table equals the generated one.
    #[test]
    fn coverage_doc_is_current() {
        let methods = facade_methods();
        let md = render(&methods, &coverage(&methods));
        if crate::codegen::tests::update() {
            std::fs::write(doc_path(), &md).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(doc_path()).unwrap_or_default(),
            md,
            "docs/design/E0-13-facade-coverage.md is stale; run DW_UPDATE_BINDINGS=1 cargo test -p dw-app coverage"
        );
    }
}
