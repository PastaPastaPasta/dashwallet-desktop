//! Keeps `docs/contracts/mg-engine.md` and the governance facade in sync
//! (ROADMAP MG-02), the way `m4_dashpay_contract.rs` does for DashPay:
//!
//! - §3 of the contract is a listing generated from the facade's source,
//!   with its SHA-256 and the contract version it was approved under. Any
//!   difference fails with a line diff. To approve a change, raise
//!   `Contract-Version` in the header, run with `DW_BLESS=1` (refused when
//!   `CI` is set), review the diff, and run again without it: the bless run
//!   itself always fails.
//! - the facade is an explicit list of files in `src/governance/`; every
//!   file there is either in it or in `NOT_FACADE`, and no `impl Governance`
//!   lives elsewhere in the crate;
//! - every `pub` type and function of the facade files is re-exported from
//!   `governance`;
//! - every call has a §2 row whose Kind and Errors cells match its
//!   signature, and every §2 row names a call;
//! - the §4 code table matches `GovernanceError::code()`, variant for
//!   variant, and the `vote_rejected` reasons match `voting.rs`;
//! - every call not yet implemented returns `governance.not_implemented`
//!   with its own name. An MG task that fills in a body moves the call to
//!   `IMPLEMENTED`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::governance::*;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineError, EngineEvent, EventSink, SessionOptions,
    WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use quote::ToTokens;
use sha2::{Digest, Sha256};

/// Calls with real bodies.
const IMPLEMENTED: &[&str] = &[
    "NetworkSession.governance",
    "Governance.sync_enabled",
    "Governance.set_sync_enabled",
    "Governance.sync_state",
    "Governance.resync",
    "Governance.clock",
    "Governance.info",
    "Governance.upcoming_superblocks",
    "superblock_budget",
    "Governance.proposals",
    "Governance.proposal",
    "Governance.current_votes",
    "Governance.voting_masternodes",
    "Governance.relay_vote",
    "Governance.vote",
    "Governance.preview_proposal",
    "Governance.create_proposal",
    "Governance.my_proposals",
    "Governance.submit_proposal",
    "Governance.contested_names",
    "Governance.vote_name",
];

/// The facade's files in `src/governance/` (mg-engine.md §0). A new file
/// there must join this list or `NOT_FACADE`.
const FACADE: &[&str] = &[
    "contested.rs",
    "errors.rs",
    "mod.rs",
    "proposals.rs",
    "reads.rs",
    "records.rs",
    "voting.rs",
];

/// Files of `src/governance/` that are not part of the facade: the sync
/// task and its tracker, the collateral resolver (D80) with its tests,
/// `vote`'s tests (MG-05), the own proposals' tests (MG-06) and the
/// contested-name tests (MG-07).
const NOT_FACADE: &[&str] = &[
    "collateral.rs",
    "collateral_tests.rs",
    "contested_tests.rs",
    "proposals_tests.rs",
    "sync.rs",
    "voting_tests.rs",
];

const BEGIN: &str = "<!-- BEGIN GENERATED: governance-surface -->";
const END: &str = "<!-- END GENERATED: governance-surface -->";
const VERSION_LINE: &str = "Contract-Version: ";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn governance_dir() -> PathBuf {
    manifest_dir().join("src/governance")
}

fn contract_path() -> PathBuf {
    manifest_dir().join("../../../docs/contracts/mg-engine.md")
}

/// The contract with LF line endings, whatever the checkout did.
fn contract() -> String {
    std::fs::read_to_string(contract_path())
        .expect("read mg-engine.md")
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
        .map(|name| (name.to_string(), parse(&governance_dir().join(name))))
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

/// Every file of `src/governance/` is classified, and the facade's methods
/// live only in facade files, so the listing cannot miss part of the
/// surface.
#[test]
fn the_facade_file_list_is_complete() {
    assert!(FACADE.windows(2).all(|w| w[0] < w[1]), "keep FACADE sorted");
    let mut unclassified: Vec<String> = std::fs::read_dir(governance_dir())
        .expect("read src/governance")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !FACADE.contains(&n.as_str()) && !NOT_FACADE.contains(&n.as_str()))
        .collect();
    unclassified.sort();
    assert!(
        unclassified.is_empty(),
        "classify these src/governance entries as FACADE or NOT_FACADE: {unclassified:?}"
    );

    let mut files = Vec::new();
    rust_files(&manifest_dir().join("src"), &mut files);
    let facade: BTreeSet<PathBuf> = FACADE.iter().map(|n| governance_dir().join(n)).collect();
    for path in files.iter().filter(|p| !facade.contains(*p)) {
        let stray = parse(path).items.iter().any(|item| {
            matches!(item, syn::Item::Impl(i) if i.trait_.is_none() && tidy(&i.self_ty) == "Governance")
        });
        assert!(
            !stray,
            "{}: `impl Governance` outside the facade files",
            path.display()
        );
    }
}

fn errors_source() -> syn::File {
    parse(&governance_dir().join("errors.rs"))
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
        out.push_str(&format!("// src/governance/{name}\n"));
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
        "mg-engine.md §3 differs from the facade (recorded {old_fp:?} at version \
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

/// Every `pub` type and function of the facade's submodules is re-exported
/// from `governance` by name, and nothing else is. (`mod.rs` is the module
/// root: its own items are exported by being there. Constants are listed in
/// §3 but are not re-exported.)
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
                _ => continue,
            };
            declared.insert(ident.to_string());
        }
    }
    let mut exported = BTreeSet::new();
    for item in parse(&governance_dir().join("mod.rs")).items {
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
        "governance/mod.rs re-exports vs pub items"
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
/// `"Governance.<method>"`, `"NetworkSession.<method>"` or a free
/// function's.
fn facade_calls() -> BTreeMap<String, Call> {
    let mut calls = BTreeMap::new();
    for (_, file) in facade_sources() {
        for item in file.items {
            match item {
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    let owner = tidy(&i.self_ty);
                    if owner == "Governance" || owner == "NetworkSession" {
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
/// `free, pure`, and any other row is a `Governance` call.
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
            format!("Governance.{name}")
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
        .unwrap_or_else(|| panic!("no {name} row in mg-engine.md §4"));
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

/// `ProposalNotFound` → `proposal_not_found`.
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
    check_domain(
        &enums,
        "GovernanceError",
        &[
            GovernanceError::SyncDisabled,
            GovernanceError::NotSynced {
                phase: GovernanceSyncPhase::Disabled,
            },
            GovernanceError::ProposalNotFound {
                hash: String::new(),
            },
            GovernanceError::VoteRejected {
                reason: String::new(),
            },
            GovernanceError::NoPeers,
            GovernanceError::ListUnavailable,
            GovernanceError::InvalidProposal { fields: Vec::new() },
            GovernanceError::SubmitRefused {
                reason: SubmitRefusal::Public,
            },
            GovernanceError::ContestNotFound {
                label: String::new(),
            },
            GovernanceError::ContestClosed {
                label: String::new(),
            },
            GovernanceError::NotAContender {
                identity: String::new(),
            },
            GovernanceError::NotImplemented {
                call: String::new(),
            },
        ],
        GovernanceError::code,
    );
    let declared: Vec<&str> = enums.keys().map(String::as_str).collect();
    assert_eq!(
        declared,
        ["GovernanceError"],
        "every error enum needs a check and a §4 row"
    );
}

/// §4's `submit_refused` reasons are `SubmitRefusal`'s, every one of them
/// (MG-06 review r1 m-3).
#[test]
fn submit_refused_reasons_match_the_contract() {
    let doc = contract();
    let bullet = doc
        .lines()
        .skip_while(|l| !l.starts_with("- `governance.submit_refused{reason}`"))
        .take_while(|l| !l.starts_with("- `governance.not_implemented"))
        .collect::<Vec<_>>()
        .join(" ");
    let listed: BTreeSet<&str> = bullet.split('`').skip(1).step_by(2).collect();
    let reasons = [
        SubmitRefusal::Public,
        SubmitRefusal::Expired,
        SubmitRefusal::FeeFailed,
        SubmitRefusal::InFlight,
    ];
    for r in reasons {
        // Exhaustive: a new reason fails to compile here until it is listed.
        match r {
            SubmitRefusal::Public
            | SubmitRefusal::Expired
            | SubmitRefusal::FeeFailed
            | SubmitRefusal::InFlight => {}
        }
        assert!(listed.contains(r.as_str()), "§4 lacks {:?}", r.as_str());
    }
}

/// The m1 codes come through `Engine` unchanged (MG-02 D8).
#[test]
fn engine_codes_pass_through() {
    let e = GovernanceError::from(EngineError::NetworkNotOpen("regtest".into()));
    assert_eq!(e.code(), "network_not_open");
    assert!(matches!(e.engine(), Some(EngineError::NetworkNotOpen(_))));
    let e = GovernanceError::from(EngineError::InvalidArgument("x".into()));
    assert_eq!(e.code(), "invalid_argument");
    assert!(GovernanceError::NoPeers.engine().is_none());
}

/// The `vote_rejected` reasons `voting.rs` can produce: the literals given
/// to `rejected(…)` and the ones `rejection_reason` returns.
fn source_reasons() -> BTreeSet<String> {
    fn quoted(text: &str) -> impl Iterator<Item = String> + '_ {
        text.split('"').skip(1).step_by(2).map(String::from)
    }
    let mut reasons = BTreeSet::new();
    let mut scan = |name: &str, block: &syn::Block| {
        let text = tidy(block);
        if name == "rejection_reason" {
            reasons.extend(quoted(&text));
        }
        for part in text.split("rejected(\"").skip(1) {
            reasons.extend(part.split('"').next().map(String::from));
        }
    };
    for item in parse(&governance_dir().join("voting.rs")).items {
        match item {
            syn::Item::Fn(f) => scan(&f.sig.ident.to_string(), &f.block),
            syn::Item::Impl(i) => {
                for it in &i.items {
                    if let syn::ImplItem::Fn(f) = it {
                        scan(&f.sig.ident.to_string(), &f.block);
                    }
                }
            }
            _ => {}
        }
    }
    reasons
}

#[test]
fn vote_rejected_reasons_match_the_contract() {
    let doc = contract();
    let start = doc
        .find("- `governance.vote_rejected{reason}`")
        .expect("§4 vote_rejected bullet");
    let rest = &doc[start..];
    let bullet = &rest[..rest[1..].find("\n- ").map_or(rest.len(), |i| i + 1)];
    // The first sentence lists the reasons; skip the code and the word
    // `reason`.
    let list = bullet.split("`.").next().unwrap();
    let documented: BTreeSet<String> = list
        .split('`')
        .skip(5)
        .step_by(2)
        .map(String::from)
        .collect();
    assert!(!documented.is_empty(), "no reasons in the bullet: {bullet}");
    assert_eq!(
        source_reasons(),
        documented,
        "voting.rs reasons vs §4 vote_rejected"
    );
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Calls(BTreeSet<String>);

impl Calls {
    /// Records the call a stub reported in its `NotImplemented { call }`.
    /// No call is a stub since MG-05; MG-06's calls will be.
    #[allow(dead_code)]
    fn stub<T: std::fmt::Debug>(&mut self, r: Result<T, GovernanceError>) {
        let e = r.expect_err("stubs never succeed");
        let GovernanceError::NotImplemented { call } = e else {
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
fn turning_sync_off_and_on_leaves_one_task_with_its_phase() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts()))
        .unwrap();
    let gov = session.governance();
    engine.block_on(async {
        gov.set_sync_enabled(true).await.unwrap();
        // A start right after a stop runs a new task once the stopped one
        // ends; with no tip it stays `waiting`, never `disabled` (D39).
        for _ in 0..5 {
            gov.set_sync_enabled(false).await.unwrap();
            gov.set_sync_enabled(true).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            assert_eq!(
                gov.sync_state().unwrap().phase,
                GovernanceSyncPhase::Waiting
            );
        }
        // A relayed vote waits for a synced run too (D40).
        let raw = RawVote {
            collateral: format!("{}-0", "cd".repeat(32)),
            proposal: "ab".repeat(32),
            outcome: VoteOutcome::Yes,
            time: 1,
            signature: String::new(),
        };
        assert_eq!(
            gov.relay_vote(raw).await.unwrap_err().code(),
            "governance.not_synced"
        );
        gov.set_sync_enabled(false).await.unwrap();
    });
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
}

#[test]
fn every_unimplemented_call_returns_not_implemented_with_its_name() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let session = engine
        .block_on(engine.open_network(DashNetwork::Regtest, opts()))
        .unwrap();
    let wallet = WalletId([7; 32]);
    let gov = session.governance();
    let hash = "ab".repeat(32);
    let raw = |proposal: &str| RawVote {
        collateral: format!("{}-0", "cd".repeat(32)),
        proposal: proposal.into(),
        outcome: VoteOutcome::Yes,
        time: 1,
        signature: String::new(),
    };

    let c = Calls::default();
    engine.block_on(async {
        // The sync setting is off by default, and the state says so.
        assert!(!gov.sync_enabled().unwrap());
        let state = gov.sync_state().unwrap();
        assert_eq!(state.phase, GovernanceSyncPhase::Disabled);
        assert_eq!((state.objects, state.votes, state.peers), (0, 0, 0));
        // Calls that need the sync task refuse while it is off.
        assert_eq!(gov.resync().unwrap_err().code(), "governance.sync_disabled");
        assert_eq!(
            gov.relay_vote(raw(&hash)).await.unwrap_err().code(),
            "governance.sync_disabled"
        );

        // No partial tally: rows, votes and the detail wait for a run that
        // reached `synced`, ahead of any argument check (MG-00 D21).
        for refused in [
            gov.proposals(ProposalQuery {
                scope: ProposalScope::All,
                text: None,
            })
            .map(drop),
            gov.proposal("zz".into()).map(drop),
            gov.proposal(hash.clone()).map(drop),
            gov.current_votes("zz".into(), None).map(drop),
            gov.current_votes(hash.clone(), Some("x".into())).map(drop),
        ] {
            assert!(matches!(
                refused,
                Err(GovernanceError::NotSynced {
                    phase: GovernanceSyncPhase::Disabled
                })
            ));
        }

        // Arguments are checked before the tip is read.
        assert_eq!(
            gov.upcoming_superblocks(25).unwrap_err().code(),
            "invalid_argument"
        );
        assert_eq!(
            gov.voting_masternodes(wallet, "zz".into())
                .await
                .unwrap_err()
                .code(),
            "invalid_argument"
        );

        // The tip-only reads answer or say `not_synced`, never anything else.
        for read in [
            gov.clock().map(drop),
            gov.info().map(drop),
            gov.upcoming_superblocks(3).map(drop),
        ] {
            if let Err(e) = read {
                assert_eq!(e.code(), "governance.not_synced");
            }
        }

        // MG-05: whole-call errors come before the grant is redeemed.
        assert_eq!(
            gov.vote(
                wallet,
                hash.clone(),
                VoteOutcome::Yes,
                vec![hash.clone()],
                "grant".into(),
            )
            .await
            .unwrap_err()
            .code(),
            "governance.sync_disabled"
        );
    });

    // The budget is the network's: no session, no tip.
    assert_eq!(superblock_budget(&DashNetwork::Mainnet, 1), 0);
    assert!(superblock_budget(&DashNetwork::Mainnet, 1_661_600) > 0);

    let mut want: BTreeSet<String> = facade_calls().into_keys().collect();
    for done in IMPLEMENTED {
        assert!(want.remove(*done), "{done} is not a facade call");
    }
    assert_eq!(c.0, want, "stubbed calls vs the facade's calls");

    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    // A handle outlives its session; every call then says so.
    assert_eq!(gov.sync_enabled().unwrap_err().code(), "network_not_open");
    assert_eq!(gov.sync_state().unwrap_err().code(), "network_not_open");
}
