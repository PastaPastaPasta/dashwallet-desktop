//! Keeps `docs/contracts/mn-engine.md` and the masternodes facade in sync
//! (ROADMAP MG-03), the way `mg_governance_contract.rs` does for governance:
//!
//! - §3 of the contract is a listing generated from the facade's source,
//!   with its SHA-256 and the contract version it was approved under. Any
//!   difference fails with a line diff. To approve a change, raise
//!   `Contract-Version` in the header, run with `DW_BLESS=1` (refused when
//!   `CI` is set), review the diff, and run again without it: the bless run
//!   itself always fails.
//! - the facade is an explicit list of files in `src/masternodes/`; every
//!   file there is either in it or in `NOT_FACADE`, and no `impl
//!   Masternodes` lives elsewhere in the crate;
//! - every `pub` type, function and constant of the facade files is
//!   re-exported from `masternodes`;
//! - every call has a §2 row whose Kind and Errors cells match its
//!   signature, and every §2 row names a call;
//! - the §4 code table matches `MasternodesError::code()`, variant for
//!   variant;
//! - every call not yet implemented returns `masternodes.not_implemented`
//!   with its own name (none in MG-03); the implemented ones refuse as §2
//!   says on a session without a synced list.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::masternodes::*;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, ImportOptions,
    SessionOptions, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use quote::ToTokens;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

/// Calls with real bodies: all of MG-03's.
const IMPLEMENTED: &[&str] = &[
    "NetworkSession.masternodes",
    "Masternodes.list_state",
    "Masternodes.list",
    "Masternodes.masternode",
    "Masternodes.payouts",
    "Masternodes.find",
    "Masternodes.evonode_credits",
    "Masternodes.resolve_collateral",
    "Masternodes.track",
    "Masternodes.untrack",
    "Masternodes.set_label",
    "Masternodes.attach_key",
    "Masternodes.detach_key",
];

/// The facade's files in `src/masternodes/` (mn-engine.md §0). A new file
/// there must join this list or `NOT_FACADE`.
const FACADE: &[&str] = &[
    "errors.rs",
    "mod.rs",
    "reads.rs",
    "records.rs",
    "tracking.rs",
];

/// Files of `src/masternodes/` that are not part of the facade: the
/// provider-transaction aggregation, and the list reader with its follower.
const NOT_FACADE: &[&str] = &["aggregate.rs", "follow.rs"];

const BEGIN: &str = "<!-- BEGIN GENERATED: masternodes-surface -->";
const END: &str = "<!-- END GENERATED: masternodes-surface -->";
const VERSION_LINE: &str = "Contract-Version: ";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn masternodes_dir() -> PathBuf {
    manifest_dir().join("src/masternodes")
}

fn contract_path() -> PathBuf {
    manifest_dir().join("../../../docs/contracts/mn-engine.md")
}

/// The contract with LF line endings, whatever the checkout did.
fn contract() -> String {
    std::fs::read_to_string(contract_path())
        .expect("read mn-engine.md")
        .replace("\r\n", "\n")
}

fn parse(path: &Path) -> syn::File {
    let text = std::fs::read_to_string(path).expect("read source");
    syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The facade's source files, in `FACADE` order, parsed.
fn facade_sources() -> Vec<(String, syn::File)> {
    FACADE
        .iter()
        .map(|name| (name.to_string(), parse(&masternodes_dir().join(name))))
        .collect()
}

/// Every Rust file under `dir`, recursively.
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

/// Every file of `src/masternodes/` is classified, and the facade's methods
/// live only in facade files, so the listing cannot miss part of the
/// surface.
#[test]
fn the_facade_file_list_is_complete() {
    assert!(FACADE.windows(2).all(|w| w[0] < w[1]), "keep FACADE sorted");
    let mut unclassified: Vec<String> = std::fs::read_dir(masternodes_dir())
        .expect("read src/masternodes")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !FACADE.contains(&n.as_str()) && !NOT_FACADE.contains(&n.as_str()))
        .collect();
    unclassified.sort();
    assert!(
        unclassified.is_empty(),
        "classify these src/masternodes entries as FACADE or NOT_FACADE: {unclassified:?}"
    );

    let mut files = Vec::new();
    rust_files(&manifest_dir().join("src"), &mut files);
    let facade: BTreeSet<PathBuf> = FACADE.iter().map(|n| masternodes_dir().join(n)).collect();
    for path in files.iter().filter(|p| !facade.contains(*p)) {
        let stray = parse(path).items.iter().any(|item| {
            matches!(item, syn::Item::Impl(i) if i.trait_.is_none() && tidy(&i.self_ty) == "Masternodes")
        });
        assert!(
            !stray,
            "{}: `impl Masternodes` outside the facade files",
            path.display()
        );
    }
}

fn errors_source() -> syn::File {
    parse(&masternodes_dir().join("errors.rs"))
}

// ---------------------------------------------------------------------------
// The generated listing
// ---------------------------------------------------------------------------

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
        out.push_str(&format!("// src/masternodes/{name}\n"));
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
        "mn-engine.md §3 differs from the facade (recorded {old_fp:?} at version \
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

/// Every `pub` type, function and constant of the facade's submodules is
/// re-exported from `masternodes` by name, and nothing else is. (`mod.rs`
/// is the module root: its own items are exported by being there.)
#[test]
fn every_public_item_is_re_exported() {
    let mut declared = BTreeSet::new();
    let mut modules = BTreeSet::new();
    for (name, file) in facade_sources() {
        if name == "mod.rs" {
            continue;
        }
        modules.insert(name.trim_end_matches(".rs").to_string());
        for item in file.items {
            let ident = match item {
                syn::Item::Struct(s) if is_pub(&s.vis) => s.ident,
                syn::Item::Enum(e) if is_pub(&e.vis) => e.ident,
                syn::Item::Fn(f) if is_pub(&f.vis) => f.sig.ident,
                syn::Item::Type(t) if is_pub(&t.vis) => t.ident,
                syn::Item::Const(c) if is_pub(&c.vis) => c.ident,
                _ => continue,
            };
            declared.insert(ident.to_string());
        }
    }
    let mut exported = BTreeSet::new();
    for item in parse(&masternodes_dir().join("mod.rs")).items {
        let syn::Item::Use(u) = item else { continue };
        let syn::UseTree::Path(p) = &u.tree else {
            continue;
        };
        if !modules.contains(&p.ident.to_string()) {
            continue;
        }
        match &*p.tree {
            syn::UseTree::Name(n) => {
                exported.insert(n.ident.to_string());
            }
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    match t {
                        syn::UseTree::Name(n) => exported.insert(n.ident.to_string()),
                        other => panic!("re-export names one by one: {}", tidy(other)),
                    };
                }
            }
            other => panic!("re-export names one by one: {}", tidy(other)),
        }
    }
    assert_eq!(
        declared, exported,
        "masternodes/mod.rs re-exports vs pub items"
    );
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

struct Call {
    kind: &'static str,
    /// The `E` of `Result<_, E>`; `None` for a call that cannot fail.
    error: Option<String>,
}

fn call(sig: &syn::Signature, free: bool) -> Call {
    let error = match &sig.output {
        syn::ReturnType::Type(_, ty) => tidy(ty)
            .strip_prefix("Result<")
            .and_then(|r| r.rsplit_once(", "))
            .map(|(_, e)| e.trim_end_matches('>').to_string()),
        syn::ReturnType::Default => None,
    };
    let kind = match (free, sig.asyncness.is_some()) {
        (true, _) => "free, pure",
        (false, true) => "async",
        (false, false) => "sync",
    };
    Call { kind, error }
}

/// The facade's calls, keyed by the name a stub reports:
/// `"Masternodes.<method>"`, `"NetworkSession.<method>"` or a free
/// function's.
fn facade_calls() -> BTreeMap<String, Call> {
    let mut calls = BTreeMap::new();
    for (_, file) in facade_sources() {
        for item in file.items {
            match item {
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    let owner = tidy(&i.self_ty);
                    if owner == "Masternodes" || owner == "NetworkSession" {
                        for f in pub_fns(&i) {
                            let name = format!("{owner}.{}", f.sig.ident);
                            calls.insert(name, call(&f.sig, false));
                        }
                    }
                }
                syn::Item::Fn(f) if is_pub(&f.vis) => {
                    calls.insert(f.sig.ident.to_string(), call(&f.sig, true));
                }
                _ => {}
            }
        }
    }
    calls
}

/// The §2 rows as cells, keyed like `facade_calls`: a first cell
/// `Owner.call(…)` names its owner, a free function's row has the Kind
/// `free, pure`, and any other row is a `Masternodes` call.
fn call_rows() -> BTreeMap<String, Vec<String>> {
    let doc = contract();
    let section = &doc[doc.find("## 2. Calls").expect("§2")..doc.find("## 3.").expect("§3")];
    let mut rows = BTreeMap::new();
    for line in section.lines().filter(|l| l.starts_with("| `")) {
        let cells: Vec<String> = line
            .trim_matches('|')
            .split(" | ")
            .map(|c| c.trim().to_string())
            .collect();
        let name = cells[0].trim_start_matches('`').split('(').next().unwrap();
        let key = if name.contains('.') || cells[1] == "free, pure" {
            name.to_string()
        } else {
            format!("Masternodes.{name}")
        };
        assert!(
            rows.insert(key.clone(), cells).is_none(),
            "two §2 rows for {key}"
        );
    }
    rows
}

#[test]
fn call_rows_match_the_signatures() {
    let mut rows = call_rows();
    for (name, call) in facade_calls() {
        let cells = rows
            .remove(&name)
            .unwrap_or_else(|| panic!("{name} has no §2 row"));
        assert_eq!(cells[1], call.kind, "{name}: §2 Kind");
        let errors = cells.last().unwrap();
        match &call.error {
            Some(e) => assert!(
                errors.starts_with(&format!("`{e}`")),
                "{name}: §2 Errors is `{errors}`, the signature says {e}"
            ),
            None => assert_eq!(errors, "—", "{name} cannot fail"),
        }
    }
    let orphans: Vec<&String> = rows.keys().collect();
    assert!(orphans.is_empty(), "§2 rows for no call: {orphans:?}");
}

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

/// The codes of the §4 row whose first cell is `` `name` ``, sorted.
fn contract_row(name: &str) -> Vec<String> {
    let doc = contract();
    let prefix = format!("| `{name}` |");
    let row = doc
        .lines()
        .find(|l| l.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no {name} row in mn-engine.md §4"));
    let mut codes: Vec<String> = row
        .split('`')
        .skip(3)
        .step_by(2)
        .map(String::from)
        .collect();
    codes.sort_unstable();
    codes
}

/// A variant that wraps another error enum and passes its code through.
fn is_wrapper(v: &syn::Variant) -> bool {
    match &v.fields {
        syn::Fields::Unnamed(u) if u.unnamed.len() == 1 => {
            tidy(&u.unnamed[0].ty).ends_with("Error")
        }
        _ => false,
    }
}

/// The error enums of errors.rs with the variants that carry their own
/// code, in order.
fn error_enums() -> BTreeMap<String, Vec<String>> {
    errors_source()
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Enum(e) => Some((
                e.ident.to_string(),
                e.variants
                    .iter()
                    .filter(|v| !is_wrapper(v))
                    .map(|v| v.ident.to_string())
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

/// `AlreadyTracked` → `already_tracked`.
fn snake(ident: &str) -> String {
    let mut out = String::new();
    for (i, c) in ident.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// Checks `samples` (one per own variant of `name`, in order) against the
/// variants' names and the contract's §4 row.
fn check_domain<E: std::fmt::Debug>(
    enums: &BTreeMap<String, Vec<String>>,
    name: &str,
    samples: &[E],
    code: fn(&E) -> &'static str,
) {
    let variants = &enums[name];
    assert_eq!(
        samples.len(),
        variants.len(),
        "{name}: the test needs one sample per variant"
    );
    for (sample, variant) in samples.iter().zip(variants) {
        assert!(
            format!("{sample:?}").starts_with(variant.as_str()),
            "{name}: the samples must follow the variant order ({variant})"
        );
        // The code names its variant.
        let suffix = code(sample).rsplit('.').next().unwrap();
        assert_eq!(
            suffix,
            snake(variant),
            "{name}::{variant} has code {}",
            code(sample)
        );
    }
    let mut codes: Vec<&str> = samples.iter().map(code).collect();
    codes.sort_unstable();
    assert_eq!(codes, contract_row(name), "{name}: §4 row");
}

#[test]
fn error_codes_match_the_contract() {
    let enums = error_enums();
    let hash = || String::new();
    check_domain(
        &enums,
        "MasternodesError",
        &[
            MasternodesError::ListUnavailable,
            MasternodesError::NotFound { query: hash() },
            MasternodesError::AlreadyTracked {
                pro_tx_hash: hash(),
            },
            MasternodesError::NotTracked {
                pro_tx_hash: hash(),
            },
            MasternodesError::KeyMismatch {
                role: AttachedKeyRole::Voting,
            },
            MasternodesError::NotEvonode {
                pro_tx_hash: hash(),
            },
            MasternodesError::NoIdentity {
                pro_tx_hash: hash(),
            },
            MasternodesError::PlatformUnavailable { detail: hash() },
            MasternodesError::Unsupported { feature: hash() },
            MasternodesError::NotImplemented { call: hash() },
        ],
        MasternodesError::code,
    );
    let declared: Vec<&str> = enums.keys().map(String::as_str).collect();
    assert_eq!(
        declared,
        ["MasternodesError"],
        "every error enum needs a check and a §4 row"
    );
}

/// The m1 codes come through `Engine` unchanged, a vault error as m1's
/// `vault`.
#[test]
fn engine_codes_pass_through() {
    let e = MasternodesError::from(EngineError::NetworkNotOpen("regtest".into()));
    assert_eq!(e.code(), "network_not_open");
    assert!(matches!(e.engine(), Some(EngineError::NetworkNotOpen(_))));
    let e = MasternodesError::from(EngineError::InvalidArgument("x".into()));
    assert_eq!(e.code(), "invalid_argument");
    let e = MasternodesError::from(dw_vault::VaultError::Locked);
    assert_eq!(e.code(), "vault");
    assert!(matches!(
        e.engine(),
        Some(EngineError::Vault(dw_vault::VaultError::Locked))
    ));
    assert!(MasternodesError::ListUnavailable.engine().is_none());
}

// ---------------------------------------------------------------------------
// Behaviour without a synced list
// ---------------------------------------------------------------------------

/// Counts the masternodes events.
#[derive(Default)]
struct CountingSink(std::sync::atomic::AtomicUsize);

impl EventSink for CountingSink {
    fn emit(&self, event: EngineEvent) {
        if matches!(event, EngineEvent::Masternodes { .. }) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

fn engine(root: &Path, sink: Arc<CountingSink>) -> Engine {
    Engine::new(
        EngineConfig {
            data_root: root.to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        sink,
    )
    .unwrap()
}

fn opts() -> SessionOptions {
    SessionOptions {
        dapi_addresses: vec!["http://127.0.0.1:1".into()],
        quorum_url: Some("http://127.0.0.1:1".into()),
        spv_peers: vec!["127.0.0.1:1".into()],
        ..Default::default()
    }
}

const PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

/// Every call is implemented, and each refuses in §2's order on a session
/// whose SPV never ran.
#[test]
fn calls_refuse_as_the_contract_says_without_a_synced_list() {
    let want: BTreeSet<String> = facade_calls().into_keys().collect();
    let done: BTreeSet<String> = IMPLEMENTED.iter().map(|c| c.to_string()).collect();
    assert_eq!(done, want, "IMPLEMENTED vs the facade's calls");

    let dir = dw_testutil::private_tempdir();
    let sink = Arc::new(CountingSink::default());
    let engine = engine(&dir.path().join("data"), Arc::clone(&sink));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts()))
        .unwrap();
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let wallet = engine
        .block_on(session.import_wallet(
            Zeroizing::new(PHRASE.as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let mn = session.masternodes();
    let hash = "ab".repeat(32);
    let key = || Zeroizing::new("01".repeat(32));
    let code = |r: Result<(), MasternodesError>| r.unwrap_err().code();

    engine.block_on(async {
        let unsynced = MasternodeListState {
            synced: false,
            height: None,
        };
        assert_eq!(mn.list_state().unwrap(), unsynced);
        // A new wallet's overview: nothing yet, the list not synced.
        let overview = mn.list(wallet).await.unwrap();
        assert_eq!(overview.list, unsynced);
        assert!(overview.masternodes.is_empty());
        assert!(overview.collateral_coins.is_empty());
        assert_eq!(overview.payouts_30d, 0);
        assert_eq!(
            code(mn.list(WalletId([7; 32])).await.map(drop)),
            "wallet_not_found"
        );

        // Arguments are checked first.
        let long = Some("x".repeat(MAX_MASTERNODE_LABEL + 1));
        for refused in [
            mn.masternode(wallet, "zz".into()).await.map(drop),
            mn.payouts(wallet, "zz".into(), None).await.map(drop),
            mn.find("node.example".into()).await.map(drop),
            mn.evonode_credits("zz".into()).await.map(drop),
            mn.resolve_collateral("zz".into()).await.map(drop),
            mn.track("zz".into(), None).await.map(drop),
            mn.track(hash.clone(), long.clone()).await.map(drop),
            mn.untrack("zz".into()).await.map(drop),
            mn.set_label(hash.clone(), long).await,
            mn.attach_key(
                hash.clone(),
                AttachedKeyRole::Owner,
                Zeroizing::new("nope".into()),
            )
            .await
            .map(drop),
            mn.detach_key("zz".into(), AttachedKeyRole::Owner)
                .await
                .map(drop),
        ] {
            assert_eq!(code(refused), "invalid_argument");
        }

        // Then the list.
        for refused in [
            mn.masternode(wallet, hash.clone()).await.map(drop),
            mn.payouts(wallet, hash.clone(), None).await.map(drop),
            mn.find(hash.clone()).await.map(drop),
            mn.find("10.0.0.1".into()).await.map(drop),
            mn.evonode_credits(hash.clone()).await.map(drop),
            mn.track(hash.clone(), None).await.map(drop),
        ] {
            assert_eq!(code(refused), "masternodes.list_unavailable");
        }
        assert_eq!(
            code(mn.resolve_collateral(hash.clone()).await.map(drop)),
            "masternodes.unsupported"
        );

        // Keys and untracking need a tracked masternode.
        for refused in [
            mn.untrack(hash.clone()).await.map(drop),
            mn.attach_key(hash.clone(), AttachedKeyRole::Voting, key())
                .await
                .map(drop),
            mn.detach_key(hash.clone(), AttachedKeyRole::Payout)
                .await
                .map(drop),
        ] {
            assert_eq!(code(refused), "masternodes.not_tracked");
        }

        // An untracked masternode's label lives in app.sqlite; each change
        // is an event.
        let before = sink.0.load(std::sync::atomic::Ordering::SeqCst);
        mn.set_label(hash.clone(), Some(" Home ".into()))
            .await
            .unwrap();
        mn.set_label(hash.clone(), None).await.unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while sink.0.load(std::sync::atomic::Ordering::SeqCst) == before {
            assert!(std::time::Instant::now() < deadline, "no masternodes event");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    });

    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    // A handle outlives its session; every call then says so.
    assert_eq!(
        engine.block_on(mn.list(wallet)).unwrap_err().code(),
        "network_not_open"
    );
    assert_eq!(mn.list_state().unwrap_err().code(), "network_not_open");
}
