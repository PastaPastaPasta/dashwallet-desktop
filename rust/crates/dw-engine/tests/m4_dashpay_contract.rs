//! Keeps `docs/contracts/m4-dashpay-engine.md` and the DashPay facade in
//! sync (ROADMAP E0-08):
//!
//! - §3 of the contract is a listing generated from the facade's source; it
//!   must match byte for byte. `DW_BLESS=1 cargo test -p dw-engine --test
//!   m4_dashpay_contract` rewrites it after a deliberate change.
//! - every call has a §2 row whose Kind and Errors cells match its
//!   signature, and every §2 row names a call;
//! - the §4 code table matches each error enum's `code()`, variant for
//!   variant, and lists every error enum;
//! - every call not yet implemented returns `platform.not_implemented` with
//!   its own name. A DP task that fills in a body moves the call to
//!   `IMPLEMENTED`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dw_engine::platform::*;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, SessionOptions, WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use quote::ToTokens;

/// Calls with real bodies.
const IMPLEMENTED: &[&str] = &["NetworkSession.dashpay", "DashPay.wallet_id"];

/// Files of `src/platform/` that are not part of the facade.
const NOT_FACADE: &[&str] = &["mod.rs", "signers.rs", "status.rs"];

const BEGIN: &str = "<!-- BEGIN GENERATED: dashpay-surface -->";
const END: &str = "<!-- END GENERATED: dashpay-surface -->";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

/// The facade's source files, sorted, parsed.
fn facade_sources() -> Vec<(String, syn::File)> {
    let dir = manifest_dir().join("src/platform");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read src/platform")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".rs") && !NOT_FACADE.contains(&n.as_str()))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let path = dir.join(&name);
            let text = std::fs::read_to_string(&path).expect("read facade source");
            let file = syn::parse_file(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            (name, file)
        })
        .collect()
}

fn errors_source() -> syn::File {
    facade_sources()
        .into_iter()
        .find_map(|(name, file)| (name == "errors.rs").then_some(file))
        .expect("errors.rs")
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

/// The `derive` and `serde` attributes: they fix what a binding and the
/// JSON `dwcli` prints can rely on.
fn shape_attrs(attrs: &[syn::Attribute]) -> impl Iterator<Item = String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("derive") || a.path().is_ident("serde"))
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

fn pub_fns(i: &syn::ItemImpl) -> impl Iterator<Item = &syn::Signature> {
    i.items.iter().filter_map(|it| match it {
        syn::ImplItem::Fn(f) if is_pub(&f.vis) => Some(&f.sig),
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
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    let fns: Vec<String> = pub_fns(&i).map(signature).collect();
                    if !fns.is_empty() {
                        out.push_str(&format!("impl {} {{\n", tidy(&i.self_ty)));
                        for f in fns {
                            out.push_str(&format!("    {f}\n"));
                        }
                        out.push_str("}\n");
                    }
                }
                syn::Item::Fn(f) if is_pub(&f.vis) => {
                    out.push_str(&format!("{}\n", signature(&f.sig)));
                }
                _ => {}
            }
        }
    }
    out
}

#[test]
fn surface_listing_matches_the_contract() {
    let doc = contract();
    let start = doc.find(BEGIN).expect("BEGIN marker") + BEGIN.len();
    let end = doc.find(END).expect("END marker");
    let want = format!("\n\n```rust\n{}```\n\n", listing());
    if doc[start..end] == want {
        return;
    }
    if std::env::var_os("DW_BLESS").is_some() {
        let blessed = format!("{}{want}{}", &doc[..start], &doc[end..]);
        std::fs::write(contract_path(), blessed).expect("write contract");
        return;
    }
    panic!(
        "m4-dashpay-engine.md §3 is out of date; run with DW_BLESS=1 and review the diff.\n\
         Generated listing:\n{want}"
    );
}

// ---------------------------------------------------------------------------
// Calls
// ---------------------------------------------------------------------------

struct Call {
    is_async: bool,
    /// The `E` of `Result<_, E>`; `None` for a call that cannot fail.
    error: Option<String>,
}

/// The facade's calls, keyed by the name a stub reports:
/// `"DashPay.<method>"`, `"NetworkSession.<method>"` or a free function's.
fn facade_calls() -> BTreeMap<String, Call> {
    let call = |sig: &syn::Signature| {
        let error = match &sig.output {
            syn::ReturnType::Type(_, ty) => tidy(ty)
                .strip_prefix("Result<")
                .and_then(|r| r.rsplit_once(", "))
                .map(|(_, e)| e.trim_end_matches('>').to_string()),
            syn::ReturnType::Default => None,
        };
        Call {
            is_async: sig.asyncness.is_some(),
            error,
        }
    };
    let mut calls = BTreeMap::new();
    for (_, file) in facade_sources() {
        for item in file.items {
            match item {
                syn::Item::Impl(i) if i.trait_.is_none() => {
                    let owner = tidy(&i.self_ty);
                    if owner == "DashPay" || owner == "NetworkSession" {
                        for sig in pub_fns(&i) {
                            calls.insert(format!("{owner}.{}", sig.ident), call(sig));
                        }
                    }
                }
                syn::Item::Fn(f) if is_pub(&f.vis) => {
                    calls.insert(f.sig.ident.to_string(), call(&f.sig));
                }
                _ => {}
            }
        }
    }
    calls
}

fn bare(call: &str) -> &str {
    call.rsplit('.').next().unwrap()
}

/// The §2 rows as cells, keyed by the call named in their first cell.
fn call_rows() -> BTreeMap<String, Vec<String>> {
    let doc = contract();
    let section = &doc[doc.find("## 2. Calls").expect("§2")..doc.find("## 3.").expect("§3")];
    section
        .lines()
        .filter(|l| l.starts_with("| `"))
        .map(|l| {
            let cells: Vec<String> = l
                .trim_matches('|')
                .split(" | ")
                .map(|c| c.trim().to_string())
                .collect();
            let name = cells[0]
                .trim_start_matches('`')
                .split('(')
                .next()
                .unwrap()
                .to_string();
            (name, cells)
        })
        .collect()
}

#[test]
fn call_rows_match_the_signatures() {
    let rows = call_rows();
    let calls = facade_calls();
    for (name, call) in &calls {
        let cells = rows
            .get(bare(name))
            .unwrap_or_else(|| panic!("{name} has no §2 row"));
        let kind = &cells[1];
        assert_eq!(
            kind.starts_with("async"),
            call.is_async,
            "{name}: §2 Kind is `{kind}`"
        );
        let errors = cells.last().unwrap();
        match &call.error {
            Some(e) => assert!(
                errors.starts_with(&format!("`{e}`")),
                "{name}: §2 Errors is `{errors}`, the signature says {e}"
            ),
            None => assert_eq!(errors, "—", "{name} cannot fail"),
        }
    }
    let names: BTreeSet<&str> = calls.keys().map(|c| bare(c)).collect();
    for row in rows.keys() {
        assert!(names.contains(row.as_str()), "§2 row for no call: {row}");
    }
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

/// The error enums of errors.rs with the variants that carry their own
/// code, in order (the `Platform` and `Identity` wrappers delegate).
fn error_enums() -> BTreeMap<String, Vec<String>> {
    errors_source()
        .items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Enum(e) => Some((
                e.ident.to_string(),
                e.variants
                    .iter()
                    .map(|v| v.ident.to_string())
                    .filter(|v| v != "Platform" && v != "Identity")
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
            PlatformError::GrantExceeded,
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
            ContactError::Ineligible,
            ContactError::AlreadyContact,
            ContactError::RequestPending,
            ContactError::IsSelf,
            ContactError::ChannelBroken,
            ContactError::PaymentLocked { txid: s() },
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
    check_domain::<CreditsError>(&enums, "CreditsError", &[], CreditsError::code);

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
    let wrapped = ContactError::from(identity);
    assert_eq!(wrapped.code(), "identity.keys_missing");
    let credits = CreditsError::from(PlatformError::InsufficientCredits {
        needed: 2,
        available: 1,
    });
    assert_eq!(credits.code(), "platform.insufficient_credits");
}

// ---------------------------------------------------------------------------
// Stubs
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Calls(BTreeSet<String>);

impl Calls {
    /// Records the call a stub reported. Every domain prints its `Platform`
    /// variant transparently, so the text is `NotImplemented`'s.
    fn stub<T: std::fmt::Debug, E: std::fmt::Display>(&mut self, r: Result<T, E>) {
        let e = r.expect_err("stubs never succeed").to_string();
        let call = e
            .strip_prefix("not implemented: ")
            .unwrap_or_else(|| panic!("not a not_implemented stub: {e}"));
        assert!(self.0.insert(call.to_string()), "{call} reported twice");
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
        c.stub(dp.top_up(s(), 1, s()).await);
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

#[test]
fn secrets_stay_out_of_debug_output() {
    let wif = "cVt4o7BGAig1UXywgGSmARhxMdzP5qvQsxKkSsc1XEkw3tDTQFpy";
    let funding = RegistrationFunding::FaucetAssetLock {
        outpoint: "00:0".into(),
        private_key: BearerSecret::new(wif.into()),
    };
    let debug = format!("{funding:?}");
    assert!(!debug.contains("cVt4o7"), "{debug}");

    let parsed: BearerSecret = serde_json::from_str(&format!("\"{wif}\"")).unwrap();
    assert_eq!(parsed.expose(), wif);

    let gravatar = AvatarSource::Gravatar {
        email: "alice@example.com".into(),
    };
    let debug = format!("{gravatar:?}");
    assert!(!debug.contains("alice"), "{debug}");
}
