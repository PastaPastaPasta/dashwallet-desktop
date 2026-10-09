//! Keeps `docs/contracts/m4-dashpay-engine.md` and the DashPay facade in
//! sync (ROADMAP E0-08):
//!
//! - §3 of the contract is a listing generated from the facade's source,
//!   with its SHA-256 and the contract version it was approved under. Any
//!   difference fails with a line diff. To approve a change, raise
//!   `Contract-Version` in the header, run with `DW_BLESS=1` (refused when
//!   `CI` is set), review the diff, and run again without it: the bless run
//!   itself always fails.
//! - the facade is an explicit list of files in `src/platform/`; every file
//!   there is either in it or in `NOT_FACADE`, and no `impl DashPay` lives
//!   elsewhere in the crate;
//! - every `pub` item of the facade files is re-exported from `platform`;
//! - every call has a §2 row whose Kind and Errors cells match its
//!   signature, and every §2 row names a call;
//! - the §4 code table matches each error enum's `code()`, variant for
//!   variant, and lists every error enum;
//! - every call not yet implemented returns `platform.not_implemented` with
//!   its own name. A DP task that fills in a body moves the call to
//!   `IMPLEMENTED`;
//! - bearer secrets cannot serialize, print, clone or compare.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::platform::*;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use quote::ToTokens;
use sha2::{Digest, Sha256};

/// Calls with real bodies.
const IMPLEMENTED: &[&str] = &["NetworkSession.dashpay", "DashPay.wallet_id"];

/// The facade's files in `src/platform/` (m4-dashpay-engine.md §0). A new
/// file there must join this list or `NOT_FACADE`.
const FACADE: &[&str] = &[
    "contacts.rs",
    "credits.rs",
    "dashpay.rs",
    "errors.rs",
    "flows.rs",
    "identity.rs",
    "invitations.rs",
    "names.rs",
    "notifications.rs",
    "payments.rs",
    "profile.rs",
    "registration.rs",
    "startup.rs",
];

/// Files of `src/platform/` that are not part of the facade: the module
/// root, the vault signers (E0-03), `platform-status` (E0-02) and the
/// identity key policy (DP1-01).
const NOT_FACADE: &[&str] = &["keys_policy.rs", "mod.rs", "signers.rs", "status.rs"];

const BEGIN: &str = "<!-- BEGIN GENERATED: dashpay-surface -->";
const END: &str = "<!-- END GENERATED: dashpay-surface -->";
const VERSION_LINE: &str = "Contract-Version: ";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn platform_dir() -> PathBuf {
    manifest_dir().join("src/platform")
}

fn contract_path() -> PathBuf {
    manifest_dir().join("../../../docs/contracts/m4-dashpay-engine.md")
}

/// The contract with LF line endings, whatever the checkout did.
fn contract() -> String {
    std::fs::read_to_string(contract_path())
        .expect("read m4-dashpay-engine.md")
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
        .map(|name| (name.to_string(), parse(&platform_dir().join(name))))
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

/// Every file of `src/platform/` is classified, and the facade's methods
/// live only in facade files, so the listing cannot miss part of the
/// surface.
#[test]
fn the_facade_file_list_is_complete() {
    assert!(FACADE.windows(2).all(|w| w[0] < w[1]), "keep FACADE sorted");
    let mut unclassified: Vec<String> = std::fs::read_dir(platform_dir())
        .expect("read src/platform")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !FACADE.contains(&n.as_str()) && !NOT_FACADE.contains(&n.as_str()))
        .collect();
    unclassified.sort();
    assert!(
        unclassified.is_empty(),
        "classify these src/platform entries as FACADE or NOT_FACADE: {unclassified:?}"
    );

    let mut files = Vec::new();
    rust_files(&manifest_dir().join("src"), &mut files);
    let facade: BTreeSet<PathBuf> = FACADE.iter().map(|n| platform_dir().join(n)).collect();
    for path in files.iter().filter(|p| !facade.contains(*p)) {
        let stray = parse(path).items.iter().any(|item| {
            matches!(item, syn::Item::Impl(i) if i.trait_.is_none() && tidy(&i.self_ty) == "DashPay")
        });
        assert!(
            !stray,
            "{}: `impl DashPay` outside the facade files",
            path.display()
        );
    }
}

fn errors_source() -> syn::File {
    parse(&platform_dir().join("errors.rs"))
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
        out.push_str(&format!("// src/platform/{name}\n"));
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
        "m4-dashpay-engine.md §3 differs from the facade (recorded {old_fp:?} at version \
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

/// Every `pub` type and function of the facade files is re-exported from
/// `platform` by name, and nothing else is.
#[test]
fn every_public_item_is_re_exported() {
    let mut declared = BTreeSet::new();
    let mut modules = BTreeSet::new();
    for (name, file) in facade_sources() {
        modules.insert(name.trim_end_matches(".rs").to_string());
        for item in file.items {
            let ident = match item {
                syn::Item::Struct(s) if is_pub(&s.vis) => s.ident,
                syn::Item::Enum(e) if is_pub(&e.vis) => e.ident,
                syn::Item::Fn(f) if is_pub(&f.vis) => f.sig.ident,
                syn::Item::Const(c) if is_pub(&c.vis) => c.ident,
                syn::Item::Type(t) if is_pub(&t.vis) => t.ident,
                _ => continue,
            };
            declared.insert(ident.to_string());
        }
    }
    let mut exported = BTreeSet::new();
    for item in parse(&platform_dir().join("mod.rs")).items {
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
        "platform/mod.rs re-exports vs pub items"
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
/// `"DashPay.<method>"`, `"NetworkSession.<method>"` or a free function's.
fn facade_calls() -> BTreeMap<String, Call> {
    let mut calls = BTreeMap::new();
    for (_, file) in facade_sources() {
        for item in file.items {
            match item {
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    let owner = tidy(&i.self_ty);
                    if owner == "DashPay" || owner == "NetworkSession" {
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
/// `free, pure`, and any other row is a `DashPay` call.
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
            format!("DashPay.{name}")
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
        .unwrap_or_else(|| panic!("no {name} row in m4-dashpay-engine.md §4"));
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

/// `InsufficientCredits` → `insufficient_credits`.
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
        // The code names its variant; `contact.self` is `IsSelf` (a keyword).
        let suffix = code(sample).rsplit('.').next().unwrap();
        let expected = match variant.as_str() {
            "IsSelf" => "self".to_string(),
            v => snake(v),
        };
        assert_eq!(
            suffix,
            expected,
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
    let s = String::new;
    check_domain(
        &enums,
        "PlatformError",
        &[
            PlatformError::Unavailable,
            PlatformError::Timeout,
            PlatformError::ProofInvalid,
            PlatformError::TrustMismatch,
            PlatformError::ContextUnavailable,
            PlatformError::SignerUnavailable,
            PlatformError::SeedMismatch,
            PlatformError::InsufficientCredits {
                needed: 0,
                available: 0,
            },
            PlatformError::GrantInvalid,
            PlatformError::GrantExceeded {
                purpose: BudgetPurpose::Credits,
                needed: 0,
                remaining: 0,
            },
            PlatformError::BroadcastUnknown { artifact: s() },
            PlatformError::WillBeSent { artifact: s() },
            PlatformError::Cancelled,
            PlatformError::NeedsGrant {
                purpose: BudgetPurpose::Funding,
            },
            PlatformError::LeaseRevoked {
                cause: RevokeCause::Lock,
            },
            PlatformError::LeaseExpired,
            PlatformError::FeatureOff { feature: s() },
            PlatformError::NotImplemented { call: s() },
            PlatformError::InvalidArgument { detail: s() },
            PlatformError::NetworkNotOpen,
            PlatformError::WalletNotFound,
            PlatformError::Storage { detail: s() },
            PlatformError::Internal { detail: s() },
        ],
        PlatformError::code,
    );
    check_domain(
        &enums,
        "IdentityError",
        &[
            IdentityError::NotFound,
            IdentityError::KeysMissing {
                purpose: KeyPurpose::Encryption,
            },
        ],
        IdentityError::code,
    );
    check_domain(
        &enums,
        "RegistrationError",
        &[
            RegistrationError::InProgress,
            RegistrationError::FundingInsufficient {
                needed: 0,
                available: 0,
            },
            RegistrationError::IslockTimeout,
            RegistrationError::Recoverable { draft: s() },
            RegistrationError::AlreadyHasUsername,
        ],
        RegistrationError::code,
    );
    check_domain(
        &enums,
        "NameError",
        &[
            NameError::Invalid { rules: vec![] },
            NameError::Taken,
            NameError::ContestOpen,
            NameError::Locked,
            NameError::UnavailableForInvite,
        ],
        NameError::code,
    );
    check_domain(
        &enums,
        "ContactError",
        &[
            ContactError::Ineligible {
                reason: Eligibility::NoDashPayKeys,
            },
            ContactError::AlreadyContact,
            ContactError::RequestPending,
            ContactError::IsSelf,
            ContactError::ChannelBroken,
            ContactError::PaymentLocked { txid: s() },
            ContactError::ScanExpired,
        ],
        ContactError::code,
    );
    check_domain(
        &enums,
        "InvitationError",
        &[
            InvitationError::Invalid,
            InvitationError::Claimed,
            InvitationError::Expired,
            InvitationError::AlreadyHasIdentity,
        ],
        InvitationError::code,
    );
    check_domain(
        &enums,
        "AvatarError",
        &[
            AvatarError::TooLarge,
            AvatarError::Unsupported,
            AvatarError::FetchFailed,
            AvatarError::HashMismatch,
            AvatarError::UploadUnconfigured,
        ],
        AvatarError::code,
    );
    check_domain(
        &enums,
        "CreditsError",
        &[
            CreditsError::FundingInsufficient {
                needed: 0,
                available: 0,
            },
            CreditsError::BelowMinimum { min: 0 },
        ],
        CreditsError::code,
    );

    let mut checked = [
        "AvatarError",
        "ContactError",
        "CreditsError",
        "IdentityError",
        "InvitationError",
        "NameError",
        "PlatformError",
        "RegistrationError",
    ]
    .to_vec();
    checked.sort_unstable();
    let declared: Vec<&str> = enums.keys().map(String::as_str).collect();
    assert_eq!(
        declared, checked,
        "every error enum needs a check and a §4 row"
    );
}

#[test]
fn wrapped_codes_pass_through() {
    let identity = PlatformError::from(IdentityError::KeysMissing {
        purpose: KeyPurpose::Decryption,
    });
    assert_eq!(identity.code(), "identity.keys_missing");
    assert_eq!(ContactError::from(identity).code(), "identity.keys_missing");

    let name = RegistrationError::from(NameError::UnavailableForInvite);
    assert_eq!(name.code(), "name.unavailable_for_invite");
    assert!(name.platform().is_none());
    let invitation = RegistrationError::from(InvitationError::Claimed);
    assert_eq!(invitation.code(), "invitation.claimed");

    // A wrapped domain's platform code has one representation.
    let flattened = RegistrationError::from(NameError::Platform(PlatformError::Timeout));
    assert!(matches!(
        flattened,
        RegistrationError::Platform(PlatformError::Timeout)
    ));
    let nested = RegistrationError::Invitation(InvitationError::Platform(PlatformError::Cancelled));
    assert_eq!(nested.platform(), Some(&PlatformError::Cancelled));
}

// ---------------------------------------------------------------------------
// Serde shape and secrets
// ---------------------------------------------------------------------------

static_assertions::assert_not_impl_any!(
    BearerSecret: serde::Serialize,
    std::fmt::Display,
    ToString,
    Clone,
    PartialEq,
    Into<String>,
    std::ops::Deref<Target = str>,
    AsRef<str>,
    AsRef<[u8]>,
    std::borrow::Borrow<str>
);
static_assertions::assert_not_impl_any!(AvatarSource: serde::Serialize, Clone);

#[test]
fn secrets_stay_out_of_debug_output_and_errors() {
    let link = "dashpay://invite?du=alice&islock=deadbeef";
    let secret =
        BearerSecret::from_utf8(zeroize::Zeroizing::new(link.as_bytes().to_vec())).unwrap();
    assert_eq!(secret.expose(), link);
    assert!(!format!("{secret:?}").contains("invite"));

    let parsed: BearerSecret = serde_json::from_str(&format!("\"{link}\"")).unwrap();
    assert_eq!(parsed.expose(), link);

    // Invalid UTF-8 is refused without quoting it.
    let bad = BearerSecret::from_utf8(zeroize::Zeroizing::new(b"sk\xff".to_vec())).unwrap_err();
    assert_eq!(
        bad,
        PlatformError::InvalidArgument {
            detail: "bearer secret is not UTF-8".into()
        }
    );

    let gravatar: AvatarSource =
        serde_json::from_str(r#"{"kind":"gravatar","email":"alice@example.com"}"#).unwrap();
    let debug = format!("{gravatar:?}");
    assert!(!debug.contains("alice"), "{debug}");
}

#[test]
fn byte_payloads_are_base64() {
    let image = AvatarImage {
        png: vec![0x89, b'P', b'N', b'G'],
        size: AvatarSize::Small,
    };
    let json = serde_json::to_value(&image).unwrap();
    assert_eq!(json["png"], "iVBORw==");
    let back: AvatarImage = serde_json::from_value(json).unwrap();
    assert_eq!(back, image);

    let file: AvatarSource =
        serde_json::from_str(r#"{"kind":"file","bytes":"iVBORw==","crop":null}"#).unwrap();
    assert!(matches!(file, AvatarSource::File { bytes, .. } if bytes == image.png));
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

/// The `PlatformError` inside any domain's error.
trait AsPlatform {
    fn as_platform(&self) -> Option<&PlatformError>;
}

impl AsPlatform for PlatformError {
    fn as_platform(&self) -> Option<&PlatformError> {
        Some(self)
    }
}

macro_rules! as_platform_via_accessor {
    ($($domain:ident),*) => {$(
        impl AsPlatform for $domain {
            fn as_platform(&self) -> Option<&PlatformError> {
                self.platform()
            }
        }
    )*};
}

as_platform_via_accessor!(
    RegistrationError,
    NameError,
    ContactError,
    InvitationError,
    AvatarError,
    CreditsError
);

#[derive(Default)]
struct Calls(BTreeSet<String>);

impl Calls {
    /// Records the call a stub reported in its `NotImplemented { call }`.
    fn stub<T: std::fmt::Debug, E: AsPlatform + std::fmt::Debug>(&mut self, r: Result<T, E>) {
        let e = r.expect_err("stubs never succeed");
        let Some(PlatformError::NotImplemented { call }) = e.as_platform() else {
            panic!("not a not_implemented stub: {e:?}");
        };
        assert!(self.0.insert(call.clone()), "{call} reported twice");
    }
}

struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: EngineEvent) {}
}

fn engine(root: &Path) -> Engine {
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
        Arc::new(NullSink),
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

#[test]
fn every_unimplemented_call_returns_not_implemented_with_its_name() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts()))
        .unwrap();
    let wallet = WalletId([7; 32]);
    let dp = session.dashpay(wallet);
    assert_eq!(dp.wallet_id(), wallet);

    let s = || "x".to_string();
    let secret = || BearerSecret::new(s());
    let request = || RegistrationRequest {
        label: s(),
        temporary_label: None,
        funding: RegistrationFunding::CoreBalance,
        initial_profile: None,
    };
    let query = ContactQuery {
        sections: vec![],
        sort: ContactSort::DisplayName,
        text: None,
    };
    let edit = ProfileEdit {
        display_name: None,
        public_message: None,
        avatar: AvatarChange::Keep,
    };
    let mut c = Calls::default();
    engine.block_on(async {
        c.stub(check_username("alice"));
        c.stub(session.stash_invitation(secret()).await);
        c.stub(session.invitation_status(s()).await);
        c.stub(session.pending_invitations().await);
        c.stub(session.forget_invitation(s()).await);
        c.stub(
            session
                .begin_flow(wallet, FlowKind::AcceptAndPay, vec![s()])
                .await,
        );
        c.stub(session.end_flow(s()));
        c.stub(session.leases());
        c.stub(dp.grant_request(s(), GrantAction::SendRequest).await);
        c.stub(dp.dispatch_status(s()).await);

        c.stub(dp.status());
        c.stub(dp.sync_status());
        c.stub(dp.sync_now().await);
        c.stub(dp.identities());
        c.stub(dp.set_main_identity(s()).await);
        c.stub(dp.identity_detail(s()).await);
        c.stub(dp.refresh_balance(s()).await);
        c.stub(dp.discover_identities(s()).await);

        c.stub(dp.registration_quote(request()).await);
        c.stub(dp.start_registration(request(), s()).await);
        c.stub(dp.registrations().await);
        c.stub(dp.resume_registration(s(), None).await);
        c.stub(dp.discard_registration(s()).await);
        c.stub(dp.finish_asset_locks(s()).await);
        c.stub(dp.prepare_faucet_lock(s()).await);

        c.stub(dp.name_availability(s()).await);
        c.stub(dp.register_name(s(), s(), s()).await);
        c.stub(dp.contest_status(s(), s()).await);
        c.stub(dp.search_users(s(), 10).await);
        c.stub(dp.resolve_user(s()).await);

        c.stub(dp.contacts(s(), query));
        c.stub(dp.contact(s(), s()));
        c.stub(dp.pending_setup_count());
        c.stub(dp.eligibility(s(), s()).await);
        c.stub(dp.send_request(s(), s(), None, s()).await);
        c.stub(dp.accept_request(s(), s(), s()).await);
        c.stub(dp.ignore(s(), s()).await);
        c.stub(dp.unignore(s(), s()).await);
        c.stub(
            dp.set_private_details(s(), s(), PrivateDetails::default(), None)
                .await,
        );
        c.stub(dp.enable_dashpay_keys(s(), s()).await);
        c.stub(dp.my_user_link(s()));
        c.stub(dp.verify_scanned(secret()).await);

        c.stub(dp.payment_lock(s(), s()));
        c.stub(dp.resolve_payment_lock(s(), s()).await);
        c.stub(
            dp.contact_activity(s(), s(), None, ActivityFilter::All)
                .await,
        );
        c.stub(dp.frequent_contacts(s(), 5));

        c.stub(dp.events(s(), None, 20).await);
        c.stub(dp.unread_count(s()));
        c.stub(dp.mark_read(s(), 1).await);

        c.stub(dp.profile(s()));
        c.stub(dp.profile_limits());
        c.stub(dp.prepare_avatar(AvatarSource::Url { url: s() }).await);
        c.stub(dp.avatar_upload_available());
        c.stub(dp.upload_avatar(s()).await);
        c.stub(dp.update_profile(s(), edit, s()).await);
        c.stub(dp.avatar(s(), AvatarSize::Small).await);

        c.stub(dp.cost_table());
        c.stub(dp.top_up_quote(s(), 1).await);
        c.stub(dp.top_up(s(), 1, s()).await);
        c.stub(dp.withdraw_quote(s(), WithdrawAmount::All).await);
        c.stub(dp.withdraw(s(), s(), WithdrawAmount::All, s()).await);
    });

    let mut want: BTreeSet<String> = facade_calls().into_keys().collect();
    for done in IMPLEMENTED {
        assert!(want.remove(*done), "{done} is not a facade call");
    }
    assert_eq!(c.0, want, "stubbed calls vs the facade's calls");

    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
}
