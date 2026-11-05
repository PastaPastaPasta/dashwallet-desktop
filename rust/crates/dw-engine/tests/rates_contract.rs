//! Keeps `docs/contracts/rates-engine.md` and the rates facade in sync
//! (MP-01), the way `mg_governance_contract.rs` does for governance:
//!
//! - §3 of the contract is a listing generated from the facade's source,
//!   with its SHA-256 and the contract version it was approved under. Any
//!   difference fails with a line diff. To approve a change, raise
//!   `Contract-Version` in the header, run with `DW_BLESS=1` (refused when
//!   `CI` is set), review the diff, and run again without it: the bless run
//!   itself always fails.
//! - every file of `src/rates/` is either in the facade list or in
//!   `NOT_FACADE`, and no `impl Rates` lives elsewhere in the crate;
//! - the §4 code table matches `RatesError::code()`, variant for variant.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use dw_engine::rates::{RatesError, RatesOff};
use quote::ToTokens;
use sha2::{Digest, Sha256};

/// The facade's files in `src/rates/`.
const FACADE: &[&str] = &["errors.rs", "mod.rs", "types.rs"];

/// The service, its sources, the price arithmetic, the stored state and
/// the tests: not part of the facade.
const NOT_FACADE: &[&str] = &[
    "decimal.rs",
    "service.rs",
    "sources.rs",
    "state.rs",
    "tests.rs",
];

const BEGIN: &str = "<!-- BEGIN GENERATED: rates-surface -->";
const END: &str = "<!-- END GENERATED: rates-surface -->";
const VERSION_LINE: &str = "Contract-Version: ";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rates_dir() -> PathBuf {
    manifest_dir().join("src/rates")
}

fn contract_path() -> PathBuf {
    manifest_dir().join("../../../docs/contracts/rates-engine.md")
}

/// The contract with LF line endings, whatever the checkout did.
fn contract() -> String {
    std::fs::read_to_string(contract_path())
        .expect("read rates-engine.md")
        .replace("\r\n", "\n")
}

fn parse(path: &Path) -> syn::File {
    let text = std::fs::read_to_string(path).expect("read source");
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn facade_sources() -> Vec<(String, syn::File)> {
    FACADE
        .iter()
        .map(|name| (name.to_string(), parse(&rates_dir().join(name))))
        .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_facade_file_list_is_complete() {
    let mut unclassified: Vec<String> = std::fs::read_dir(rates_dir())
        .expect("read src/rates")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !FACADE.contains(&n.as_str()) && !NOT_FACADE.contains(&n.as_str()))
        .collect();
    unclassified.sort();
    assert!(
        unclassified.is_empty(),
        "classify these src/rates entries: {unclassified:?}"
    );
    let mut files = Vec::new();
    rust_files(&manifest_dir().join("src"), &mut files);
    let facade: BTreeSet<PathBuf> = FACADE.iter().map(|n| rates_dir().join(n)).collect();
    for path in files.iter().filter(|p| !facade.contains(*p)) {
        let stray = parse(path).items.iter().any(|item| {
            matches!(item, syn::Item::Impl(i) if i.trait_.is_none() && tidy(&i.self_ty) == "Rates")
        });
        assert!(
            !stray,
            "{}: `impl Rates` outside the facade files",
            path.display()
        );
    }
}

/// §4's table: one row `| `code` | … |` per `RatesError` variant.
#[test]
fn the_code_table_matches_the_engine() {
    let doc = contract();
    let table: BTreeSet<String> = doc
        .split_once("## 4.")
        .expect("§4")
        .1
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split_once('`').map(|(c, _)| c.to_string()))
        .collect();
    let engine: BTreeSet<String> = [
        RatesError::InvalidCurrency(String::new()),
        RatesError::Throttled,
        RatesError::Off(RatesOff::Disabled),
        RatesError::Storage(String::new()),
    ]
    .iter()
    .map(|e| e.code().to_string())
    .collect();
    assert_eq!(table, engine);
}

/// `quote`'s token text with the spacing rustfmt would use.
fn tidy(tokens: impl ToTokens) -> String {
    let mut s = tokens.to_token_stream().to_string();
    for (from, to) in [
        (" :: ", "::"),
        ("< ", "<"),
        (" <", "<"),
        (" >", ">"),
        (" ,", ","),
        ("& ", "&"),
        (" : ", ": "),
        ("( ", "("),
        (" )", ")"),
        ("# [", "#["),
        (" !", "!"),
        // rustfmt's trailing comma in a wrapped argument list
        (",)", ")"),
    ] {
        s = s.replace(from, to);
    }
    // `name (` → `name(`; a bare replace would also glue `-> (` and `: (`.
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.char_indices() {
        let glued = c == ' '
            && s[i + 1..].starts_with('(')
            && out.ends_with(|p: char| p.is_alphanumeric() || p == '_');
        if !glued {
            out.push(c);
        }
    }
    out
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

/// The attributes that change a binding's or the JSON's view of an item:
/// `derive`, `serde` and `cfg`.
fn shape_attrs(attrs: &[syn::Attribute]) -> impl Iterator<Item = String> {
    attrs
        .iter()
        .filter(|a| {
            ["derive", "serde", "cfg"]
                .iter()
                .any(|i| a.path().is_ident(i))
        })
        .map(tidy)
}

fn attr_lines(attrs: &[syn::Attribute], indent: &str) -> String {
    shape_attrs(attrs)
        .map(|a| format!("{indent}{a}\n"))
        .collect()
}

/// `name: Type`, or `Type` for a tuple field, after its shape attributes.
fn field(f: &syn::Field) -> String {
    let attrs: String = shape_attrs(&f.attrs).map(|a| a + " ").collect();
    match &f.ident {
        Some(name) => format!("{attrs}{name}: {}", tidy(&f.ty)),
        None => format!("{attrs}{}", tidy(&f.ty)),
    }
}

/// A variant's fields on one line.
fn variant_fields(fields: &syn::Fields) -> String {
    let list = fields.iter().map(field).collect::<Vec<_>>().join(", ");
    match fields {
        syn::Fields::Unit => String::new(),
        syn::Fields::Unnamed(_) => format!("({list})"),
        syn::Fields::Named(_) => format!(" {{ {list} }}"),
    }
}

/// A struct's public fields, named ones one per line as rustfmt writes
/// them; private fields collapse into `..`.
fn struct_body(fields: &syn::Fields) -> String {
    let mut list: Vec<String> = fields
        .iter()
        .filter(|f| is_pub(&f.vis))
        .map(|f| format!("pub {}", field(f)))
        .collect();
    let hidden = list.len() < fields.len();
    match fields {
        syn::Fields::Unit => ";\n".into(),
        syn::Fields::Unnamed(_) => {
            if hidden {
                list.push("..".into());
            }
            format!("({});\n", list.join(", "))
        }
        syn::Fields::Named(_) => {
            let mut body: String = list.iter().map(|f| format!("    {f},\n")).collect();
            if hidden {
                body.push_str("    ..\n");
            }
            format!(" {{\n{body}}}\n")
        }
    }
}

fn pub_fns(i: &syn::ItemImpl) -> impl Iterator<Item = &syn::ImplItemFn> {
    i.items.iter().filter_map(|it| match it {
        syn::ImplItem::Fn(f) if is_pub(&f.vis) => Some(f),
        _ => None,
    })
}

fn signature(sig: &syn::Signature) -> String {
    format!("pub {};", tidy(sig))
}

fn listing() -> String {
    let mut out = String::new();
    for (name, file) in facade_sources() {
        out.push_str(&format!("// src/rates/{name}\n"));
        for item in file.items {
            match item {
                syn::Item::Struct(s) if is_pub(&s.vis) => {
                    out.push_str(&attr_lines(&s.attrs, ""));
                    out.push_str(&format!("pub struct {}", s.ident));
                    out.push_str(&struct_body(&s.fields));
                }
                syn::Item::Enum(e) if is_pub(&e.vis) => {
                    out.push_str(&attr_lines(&e.attrs, ""));
                    out.push_str(&format!("pub enum {} {{\n", e.ident));
                    for v in &e.variants {
                        out.push_str(&attr_lines(&v.attrs, "    "));
                        out.push_str(&format!("    {}{},\n", v.ident, variant_fields(&v.fields)));
                    }
                    out.push_str("}\n");
                }
                // Trait impls change what a type can do (serialize, print,
                // convert) without changing its shape: list every header.
                syn::Item::Impl(i) if i.trait_.is_some() => {
                    let (_, path, _) = i.trait_.as_ref().unwrap();
                    out.push_str(&attr_lines(&i.attrs, ""));
                    out.push_str(&format!("impl {} for {};\n", tidy(path), tidy(&i.self_ty)));
                }
                syn::Item::Impl(i) => {
                    let fns: Vec<String> = pub_fns(&i)
                        .map(|f| {
                            format!(
                                "{}    {}\n",
                                attr_lines(&f.attrs, "    "),
                                signature(&f.sig)
                            )
                        })
                        .collect();
                    if !fns.is_empty() {
                        out.push_str(&attr_lines(&i.attrs, ""));
                        out.push_str(&format!("impl {} {{\n", tidy(&i.self_ty)));
                        out.extend(fns);
                        out.push_str("}\n");
                    }
                }
                syn::Item::Fn(f) if is_pub(&f.vis) => {
                    out.push_str(&attr_lines(&f.attrs, ""));
                    out.push_str(&format!("{}\n", signature(&f.sig)));
                }
                syn::Item::Const(c) if is_pub(&c.vis) => {
                    out.push_str(&format!("pub const {}: {};\n", c.ident, tidy(&c.ty)));
                }
                syn::Item::Type(t) if is_pub(&t.vis) => {
                    out.push_str(&format!("pub type {} = {};\n", t.ident, tidy(&t.ty)));
                }
                syn::Item::Use(u) if is_pub(&u.vis) => {
                    out.push_str(&format!("pub use {};\n", tidy(&u.tree)));
                }
                // A macro at item level can generate API.
                syn::Item::Macro(m) => out.push_str(&format!("{};\n", tidy(&m.mac))),
                _ => {}
            }
        }
    }
    out
}

/// The `Contract-Version` in the contract's header.
fn header_version(doc: &str) -> u32 {
    let line = doc
        .lines()
        .find_map(|l| l.strip_prefix(VERSION_LINE))
        .expect("Contract-Version line");
    line.trim().parse().expect("Contract-Version number")
}

/// The generated region as the contract should hold it.
fn region(listing: &str, version: u32) -> String {
    let fp = hex::encode(Sha256::digest(listing.as_bytes()));
    format!("\n<!-- surface-sha256: {fp} version: {version} -->\n\n```rust\n{listing}```\n\n")
}

/// The fingerprint, version and listing recorded in a region.
fn recorded(region: &str) -> (Option<String>, Option<u32>, String) {
    let mark = region
        .lines()
        .find_map(|l| l.strip_prefix("<!-- surface-sha256: "))
        .and_then(|l| l.strip_suffix(" -->"))
        .and_then(|l| l.split_once(" version: "));
    let listing = region
        .split_once("```rust\n")
        .and_then(|(_, rest)| rest.split_once("```"))
        .map(|(l, _)| l.to_string())
        .unwrap_or_default();
    (
        mark.map(|(fp, _)| fp.to_string()),
        mark.and_then(|(_, v)| v.parse().ok()),
        listing,
    )
}

/// A line diff (`-` old, `+` new) from the longest common subsequence.
fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        } else {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        }
    }
    out
}

#[test]
fn surface_listing_matches_the_contract() {
    let doc = contract();
    let start = doc.find(BEGIN).expect("BEGIN marker") + BEGIN.len();
    let end = doc.find(END).expect("END marker");
    let version = header_version(&doc);
    let listing = listing();
    let want = region(&listing, version);
    let have = &doc[start..end];
    if have == want {
        return;
    }

    let (old_fp, old_version, old_listing) = recorded(have);
    let new_fp = hex::encode(Sha256::digest(listing.as_bytes()));
    let surface_changed = old_fp.as_deref() != Some(new_fp.as_str());
    eprintln!(
        "rates-engine.md §3 differs from the facade (recorded {old_fp:?} at version \
         {old_version:?}; now {new_fp} at {VERSION_LINE}{version}).\n--- contract §3\n+++ source\n{}",
        diff(&old_listing, &listing)
    );
    if std::env::var_os("DW_BLESS").is_none() {
        panic!(
            "§3 is out of date (diff above). If the change is deliberate, raise Contract-Version in \
             the contract header, run with DW_BLESS=1, review the diff and re-run."
        );
    }
    if std::env::var_os("CI").is_some() {
        panic!("refusing DW_BLESS under CI: approve contract changes locally");
    }
    if surface_changed && old_version.is_some_and(|v| version <= v) {
        panic!(
            "the surface changed: raise Contract-Version (now {version}) above the recorded \
             version {} before blessing",
            old_version.unwrap()
        );
    }
    let blessed = format!("{}{want}{}", &doc[..start], &doc[end..]);
    std::fs::write(contract_path(), blessed).expect("write contract");
    panic!(
        "blessed §3 as version {version} ({new_fp}); review the diff above and re-run without DW_BLESS"
    );
}
