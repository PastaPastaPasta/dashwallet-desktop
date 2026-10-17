//! DP5-01 D15: fuzz seeds for the link parsers (tier T0). Links are
//! attacker-supplied: any web page can open one. Every seed in
//! `testdata/link-corpus/` (one link per file, cargo-fuzz style, so H-02's
//! fuzz farm can start from the same files), and a bounded number of
//! mutations of them and of generated links, goes through every parser the
//! intake uses. Properties: no parser panics, classification ignores
//! surrounding whitespace, and a link that the stash keeps never survives
//! log redaction (D14), whatever text surrounds it.

use std::path::PathBuf;

use dw_uri::deeplink::{
    DashPayContactCode, DashPayUserLink, DeepLink, classify_link, normalize_invitation,
};
use dw_uri::ext::{parse_payment_string, parse_payment_uri, remove_percent_encoding};
use dw_uri::redact::{REDACTED, redact_links, redacted_whole};
use proptest::prelude::*;

fn corpus() -> Vec<(String, String)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/link-corpus");
    let mut seeds: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| {
            let path = e.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let bytes = std::fs::read(&path).unwrap();
            (name, String::from_utf8_lossy(&bytes).into_owned())
        })
        .collect();
    seeds.sort();
    assert!(seeds.len() >= 40, "the seed corpus shrank: {}", seeds.len());
    seeds
}

/// Runs every parser on `text`; panics only if one does.
fn parse_all(text: &str) -> DeepLink {
    let link = classify_link(text);
    let _ = normalize_invitation(text);
    let _ = DashPayUserLink::parse(text);
    let _ = DashPayContactCode::parse(text);
    let _ = parse_payment_uri(text);
    let _ = parse_payment_string(text);
    let _ = remove_percent_encoding(text);
    let _ = redact_links(text);
    link
}

/// The values a kept link carries that must never reach a log: the
/// payee, the username and identity, an invitation's parameters.
fn carried(link: &DeepLink, text: &str) -> Vec<String> {
    let mut values = Vec::new();
    match link {
        DeepLink::Payment(req) if req.address.is_some() => {
            values.extend(req.address.clone());
            values.extend(req.label.clone());
        }
        DeepLink::DashPayUser(user) => {
            values.push(user.username.clone());
            values.push(bs58_identity(text));
        }
        DeepLink::Invitation(_) => values.push(text.trim().to_owned()),
        _ => {}
    }
    // Every parameter value as written, and decoded (review r1 #9): `pk`,
    // `du`, `assetlocktx`, amounts, whatever the parsers read or skip.
    if let Some((_, query)) = text.split_once(['?', '#']) {
        for pair in query.split(['&', '#']) {
            if let Some((_, value)) = pair.split_once('=') {
                values.push(value.to_owned());
                values.extend(remove_percent_encoding(value));
            }
        }
    }
    // Short values (`a`, `1`) occur in any text by chance.
    values.retain(|v| v.chars().count() >= 4);
    values
}

/// The `id=` parameter as written.
fn bs58_identity(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    lower
        .find("id=")
        .map(|i| {
            text[i + 3..]
                .split('&')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default()
}

/// Whether the stash keeps this link (dw-engine's `stash_kind`): one of
/// the kept kinds, on one line, which redaction cuts from its start.
fn kept(link: &DeepLink, text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().any(char::is_control) || !redacted_whole(trimmed) {
        return false;
    }
    match link {
        DeepLink::Payment(req) => req.address.is_some(),
        DeepLink::DashPayUser(_) | DeepLink::Invitation(_) => true,
        _ => false,
    }
}

/// The redaction property for one kept link inside `before … after`.
fn assert_redacted(text: &str, before: &str, after: &str) {
    let link = classify_link(text);
    if !kept(&link, text) {
        return;
    }
    let line = format!("{before}{text}{after}");
    let redacted = redact_links(&line);
    assert!(redacted.contains(REDACTED), "nothing redacted in {line:?}");
    for value in carried(&link, text) {
        if before.contains(&value) || after.contains(&value) {
            continue;
        }
        assert!(
            !redacted.contains(&value),
            "{value:?} of {text:?} survives: {redacted:?}"
        );
    }
}

/// Text around a link in a log line: a delimiter, then anything.
const SURROUNDINGS: [(&str, &str); 7] = [
    ("", ""),
    // A coloured log field (review r1 #1).
    ("link\x1b[2m=\x1b[0m", "\x1b[0m tail"),
    ("link ", " routed"),
    ("text=\"", "\""),
    ("<", ">"),
    ("\t", "\n"),
    ("got '", "' twice"),
];

#[test]
fn every_seed_parses_and_kept_seeds_are_redacted() {
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    for (name, seed) in corpus() {
        let link = parse_all(&seed);
        assert_eq!(
            std::mem::discriminant(&link),
            std::mem::discriminant(&classify_link(&format!("  {seed}\n"))),
            "{name}: whitespace changed the kind"
        );
        let kind = match &link {
            DeepLink::Payment(r) if r.address.is_some() => "payment",
            DeepLink::DashPayUser(_) => "contact",
            DeepLink::Invitation(_) => "invitation",
            _ => "other",
        };
        *kinds.entry(kind).or_default() += 1;
        for (before, after) in SURROUNDINGS {
            assert_redacted(&seed, before, after);
        }
    }
    // The corpus covers each kept kind and the refusals.
    for kind in ["payment", "contact", "invitation", "other"] {
        assert!(kinds.get(kind).copied().unwrap_or(0) >= 3, "{kinds:?}");
    }
}

/// One edit of a seed: insert, delete or replace a character.
#[derive(Debug, Clone)]
enum Edit {
    Insert(usize, char),
    Delete(usize),
    Replace(usize, char),
}

fn edit_strategy() -> impl Strategy<Value = Edit> {
    let interesting = prop_oneof![
        Just(':'),
        Just('/'),
        Just('?'),
        Just('&'),
        Just('='),
        Just('%'),
        Just('#'),
        Just('@'),
        Just('['),
        Just(' '),
        any::<char>(),
    ];
    prop_oneof![
        (any::<usize>(), interesting.clone()).prop_map(|(i, c)| Edit::Insert(i, c)),
        any::<usize>().prop_map(Edit::Delete),
        (any::<usize>(), interesting).prop_map(|(i, c)| Edit::Replace(i, c)),
    ]
}

fn apply(seed: &str, edits: &[Edit]) -> String {
    let mut chars: Vec<char> = seed.chars().collect();
    for edit in edits {
        match *edit {
            Edit::Insert(i, c) => chars.insert(i % (chars.len() + 1), c),
            Edit::Delete(i) if !chars.is_empty() => {
                chars.remove(i % chars.len());
            }
            Edit::Replace(i, c) if !chars.is_empty() => {
                let n = chars.len();
                chars[i % n] = c;
            }
            _ => {}
        }
    }
    chars.into_iter().collect()
}

/// Generated links of the kept kinds, with mixed-case schemes and
/// parameter names, and values from an alphabet that includes the
/// delimiters a parser splits on.
fn generated_link() -> impl Strategy<Value = String> {
    let value = "[A-Za-z0-9%._~-]{1,40}";
    let scheme = prop_oneof![
        Just("dash:"),
        Just("DASH:"),
        Just("pay:"),
        Just("Pay:"),
        Just("dashpay://invite?"),
        Just("DashPay://Invite?"),
        Just("dashpay://user?"),
        Just("https://invitations.dashpay.io/applink?"),
        Just("dashwallet://pay="),
    ];
    let names = prop_oneof![
        Just("amount"),
        Just("label"),
        Just("message"),
        Just("du"),
        Just("pk"),
        Just("dapk"),
        Just("assetlocktx"),
        Just("islock"),
        Just("id"),
        Just("username"),
        Just("sender"),
        Just("r"),
    ];
    (scheme, value, prop::collection::vec((names, value), 0..6)).prop_map(
        |(scheme, head, params)| {
            let query = params
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            if scheme.ends_with('?') {
                format!("{scheme}{query}")
            } else {
                format!("{scheme}{head}?{query}")
            }
        },
    )
}

proptest! {
    // Bounded: the gate runs this on every change (T0), in seconds.
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn mutated_seeds_never_panic(
        seed in 0usize..1000,
        edits in prop::collection::vec(edit_strategy(), 1..8),
    ) {
        let seeds = corpus();
        let (_, seed) = &seeds[seed % seeds.len()];
        let text = apply(seed, &edits);
        parse_all(&text);
        for (before, after) in SURROUNDINGS {
            assert_redacted(&text, before, after);
        }
    }

    #[test]
    fn arbitrary_text_never_panics(text in "\\PC{0,300}") {
        parse_all(&text);
    }

    #[test]
    fn generated_links_are_redacted(link in generated_link(), pick in 0usize..SURROUNDINGS.len()) {
        parse_all(&link);
        let (before, after) = SURROUNDINGS[pick];
        assert_redacted(&link, before, after);
    }
    /// D30 (Sol r1 M3): a value with a line break inside a link, as an
    /// argument the OS passed can be. Once redaction finds a link, the
    /// event keeps only what came before it: nothing of the link's rest,
    /// on any later line.
    #[test]
    fn a_line_break_never_carries_a_link_past_redaction(
        link in generated_link(),
        at in any::<usize>(),
        cr in any::<bool>(),
    ) {
        let chars: Vec<char> = link.chars().collect();
        let at = at % (chars.len() + 1);
        let head: String = chars[..at].iter().collect();
        let tail: String = chars[at..].iter().collect();
        let event = format!(
            "unknown option --bad={head}{}{tail}\n",
            if cr { "\r" } else { "\n" }
        );
        let redacted = redact_links(&event);
        if redacted == event {
            return Ok(());
        }
        let body = redacted.trim_end_matches(['\n', '\r']);
        prop_assert!(body.ends_with(REDACTED), "{redacted:?}");
        let kept = &body[..body.len() - REDACTED.len()];
        prop_assert!(event.starts_with(kept), "{redacted:?}");
        // A link whole in the first line: nothing of it is kept.
        if !head.is_empty() && redacted_whole(&head) {
            prop_assert!(kept.len() <= "unknown option --bad=".len(), "{redacted:?}");
        }
    }
}
