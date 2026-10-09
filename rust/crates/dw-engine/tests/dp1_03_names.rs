//! DP1-03 names: `check_username` against the upstream DPNS rule vectors
//! (`fixtures/dp1_03_username_vectors.json`, provenance in its entries and
//! `fixtures/README.md`) and dash-platform-queries' own functions, the
//! offline facade, and `name_availability` against the testnet explorer
//! (ignored; `DW_E005_TESTNET_FIXTURE`).

use std::path::Path;
use std::sync::Arc;

use dash_sdk::platform::dpns_usernames::{
    convert_to_homograph_safe_chars, is_contested_username, is_valid_username,
};
use dw_engine::platform::*;
use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, SessionOptions,
    WalletId,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use serde_json::Value;
use zeroize::Zeroizing;

fn vectors() -> Value {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dp1_03_username_vectors.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn rows<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v[key].as_array().unwrap()
}

fn check(label: &str) -> UsernameCheck {
    check_username(label).unwrap()
}

fn broken(c: &UsernameCheck) -> Vec<String> {
    c.rules
        .iter()
        .filter(|r| !r.passed)
        .map(|r| {
            serde_json::to_value(r.rule)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn the_vectors_cite_the_pin_and_the_cap() {
    let v = vectors();
    let pin = v["pin"].as_str().unwrap();
    assert_eq!(
        pin,
        "dashpay/platform@bc41f1bc233dec4607d387101c1d9c2f111019b2"
    );
    assert_eq!(v["max_length"], 23);
    for key in [
        "validity",
        "normalize",
        "contested",
        "broken_rules",
        "homograph_collisions",
    ] {
        let rows = rows(&v, key);
        assert!(!rows.is_empty(), "{key}");
        for row in rows {
            let source = row["source"].as_str().unwrap_or_default();
            assert!(
                source.contains(pin) || source.contains("dashwallet-ios@"),
                "{key}: {row} has no pinned source"
            );
        }
    }
}

#[test]
fn validity_vectors() {
    let v = vectors();
    for row in rows(&v, "validity") {
        let label = row["label"].as_str().unwrap();
        assert_eq!(
            is_valid_username(label),
            row["dpns_valid"],
            "upstream {label:?}"
        );
        assert_eq!(check(label).valid, row["valid"], "{label:?}");
    }
}

#[test]
fn normalization_vectors() {
    let v = vectors();
    for row in rows(&v, "normalize") {
        let label = row["label"].as_str().unwrap();
        assert_eq!(check(label).normalized, row["normalized"], "{label:?}");
    }
}

#[test]
fn contested_vectors() {
    let v = vectors();
    for row in rows(&v, "contested") {
        let label = row["label"].as_str().unwrap();
        let want = row["contested"].as_bool().unwrap();
        assert_eq!(is_contested_username(label), want, "upstream {label:?}");
        // The engine only calls a label contested once it is valid.
        let c = check(label);
        assert_eq!(c.contested, want && c.valid, "{label:?}");
    }
}

#[test]
fn broken_rule_vectors() {
    let v = vectors();
    for row in rows(&v, "broken_rules") {
        let label = row["label"].as_str().unwrap();
        let want: Vec<String> = serde_json::from_value(row["rules"].clone()).unwrap();
        assert_eq!(broken(&check(label)), want, "{label:?}");
    }
}

#[test]
fn homograph_collisions_are_one_name() {
    let v = vectors();
    for row in rows(&v, "homograph_collisions") {
        let normalized = row["normalized"].as_str().unwrap();
        for label in row["labels"].as_array().unwrap() {
            let label = label.as_str().unwrap();
            let c = check(label);
            assert!(c.valid, "{label:?}");
            assert_eq!(c.normalized, normalized, "{label:?}");
            assert_eq!(convert_to_homograph_safe_chars(label), normalized);
        }
    }
}

/// Every label up to six characters over an alphabet that exercises each
/// rule and the folding: the engine is upstream plus the 23 cap. Lengths
/// around the contested (19) and engine (23) caps come from repetition.
#[test]
fn the_checklist_is_upstream_with_the_cap_for_every_short_label() {
    const ALPHABET: &[char] = &['a', 'o', 'I', '1', '7', '-', '_'];
    let mut labels = vec![String::new()];
    let mut frontier = vec![String::new()];
    for _ in 0..6 {
        frontier = frontier
            .iter()
            .flat_map(|p| ALPHABET.iter().map(move |c| format!("{p}{c}")))
            .collect();
        labels.extend(frontier.iter().cloned());
    }
    for n in 17..=26 {
        labels.push("a".repeat(n));
        labels.push(format!("a{}7", "-o".repeat((n - 2) / 2)));
    }
    labels.push("ab\u{e9}".into());
    labels.push("\u{430}lice".into());
    let mut counted = [0usize; 2];
    for label in &labels {
        let c = check(label);
        let upstream = is_valid_username(label) && label.chars().count() <= 23;
        assert_eq!(c.valid, upstream, "{label:?}");
        assert_eq!(
            c.contested,
            upstream && is_contested_username(label),
            "{label:?}"
        );
        assert_eq!(
            c.normalized,
            convert_to_homograph_safe_chars(label),
            "{label:?}"
        );
        assert_eq!(broken(&c).is_empty(), c.valid, "{label:?}");
        counted[usize::from(c.valid)] += 1;
    }
    eprintln!(
        "{} labels: {} valid, {} invalid",
        labels.len(),
        counted[1],
        counted[0]
    );
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

#[test]
fn offline_availability_checks_the_label_then_the_wallet() {
    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let s = engine
        .block_on(engine.open_network(
            DashNetwork::Regtest,
            SessionOptions {
                dapi_addresses: vec!["http://127.0.0.1:1".into()],
                quorum_url: Some("http://127.0.0.1:1".into()),
                spv_peers: vec!["127.0.0.1:1".into()],
                ..Default::default()
            },
        ))
        .unwrap();
    let dp = s.dashpay(WalletId([7; 32]));
    // A bad label is answered without the network or a wallet.
    assert_eq!(
        engine
            .block_on(dp.name_availability("a--b".into()))
            .unwrap(),
        NameAvailability::Invalid {
            rules: vec![UsernameRule::NoDoubleHyphen]
        }
    );
    assert_eq!(
        engine.block_on(dp.name_availability("ab".into())).unwrap(),
        NameAvailability::Invalid {
            rules: vec![UsernameRule::MinLength]
        }
    );
    let code = |e: NameError| e.code();
    assert_eq!(
        code(
            engine
                .block_on(dp.name_availability("alice".into()))
                .unwrap_err()
        ),
        "wallet_not_found"
    );
    assert_eq!(
        code(
            engine
                .block_on(dp.register_name("x".into(), "-x".into(), "g".into()))
                .unwrap_err()
        ),
        "name.invalid"
    );
    assert_eq!(
        code(
            engine
                .block_on(dp.register_name("not base58!".into(), "alice".into(), "g".into()))
                .unwrap_err()
        ),
        "invalid_argument"
    );
    engine
        .block_on(engine.close_network(DashNetwork::Regtest))
        .unwrap();
    assert_eq!(
        code(
            engine
                .block_on(dp.name_availability("alice".into()))
                .unwrap_err()
        ),
        "network_not_open"
    );
}

/// What the testnet explorer (`testnet.platform-explorer.pshenmic.dev`,
/// `/dpns/identity?dpns=<label>.dash` and `/contestedResource/…`) said on
/// 2026-10-09 for ten labels: owner of the domain, or the contest verdict.
const EXPLORER: &[(&str, Explorer)] = &[
    (
        "alice",
        Explorer::Taken("FKZZFDTfGdSWUmL2g7H9e46pMJMPQp9DHQcvjrsS6884"),
    ),
    (
        "Alice4Testing",
        Explorer::Taken("oBZwfpNKaP9A17YVdXHSXj5c5wmRDMeUWEoyEbqcRY2"),
    ),
    (
        "b01n",
        Explorer::Taken("4794iiLvNfiuQ8pv3qF7zbNmERxkDmRbHKNJvgo3EDQb"),
    ),
    (
        "hehe",
        Explorer::Taken("3xCA5xsNYEwGBEk5gTmzrbAoBr3HNi7U5aUJmskah8od"),
    ),
    (
        "cutestt8",
        Explorer::Taken("44eVN9hWFsS4mAVUUjNsksds66MDQhsuRVZZdQJdK7PU"),
    ),
    (
        "n00",
        Explorer::Taken("4794iiLvNfiuQ8pv3qF7zbNmERxkDmRbHKNJvgo3EDQb"),
    ),
    (
        "kmqwzhdvpxtn",
        Explorer::Taken("5b37CK5PtMNA8VMDTGgZ4mXsM7PfZGLFe8tbr3LFxfoK"),
    ),
    ("dasher", Explorer::Locked),
    ("dp103-free-q7k2", Explorer::Free { contested: false }),
    ("zzqqxxwwvv", Explorer::Free { contested: true }),
];

#[derive(Debug)]
enum Explorer {
    Taken(&'static str),
    Locked,
    Free { contested: bool },
}

impl Explorer {
    fn agrees(&self, a: &NameAvailability) -> bool {
        match (self, a) {
            (Self::Taken(owner), NameAvailability::Taken { owner: Some(o) }) => o == owner,
            (Self::Locked, NameAvailability::Locked) => true,
            (Self::Free { contested }, NameAvailability::Available { contested: c }) => {
                contested == c
            }
            _ => false,
        }
    }
}

/// `DW_E005_TESTNET_FIXTURE` names E0-05's testnet fixture directory
/// (`A.mnemonic`, `fixture.json`); the phrase is read, never printed.
#[test]
#[ignore = "public testnet; set DW_E005_TESTNET_FIXTURE"]
fn testnet_availability_agrees_with_the_explorer() {
    let fixture = std::path::PathBuf::from(
        std::env::var("DW_E005_TESTNET_FIXTURE").expect("DW_E005_TESTNET_FIXTURE"),
    );
    let phrase = Zeroizing::new(std::fs::read_to_string(fixture.join("A.mnemonic")).unwrap());
    let meta: Value =
        serde_json::from_slice(&std::fs::read(fixture.join("fixture.json")).unwrap()).unwrap();
    let identity = meta["A"]["identity_id"].as_str().unwrap().to_owned();
    let own_name = meta["A"]["names"][0]["label"].as_str().unwrap().to_owned();

    let dir = dw_testutil::private_tempdir();
    let engine = engine(&dir.path().join("data"));
    let s = engine
        .block_on(engine.open_network(DashNetwork::Testnet, SessionOptions::default()))
        .unwrap();
    engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
    let id = engine
        .block_on(s.import_wallet(
            Zeroizing::new(phrase.trim().as_bytes().to_vec()),
            Zeroizing::new(Vec::new()),
            ImportOptions::default(),
        ))
        .unwrap();
    let dp = s.dashpay(id);
    let available = |label: &str| engine.block_on(dp.name_availability(label.into())).unwrap();

    let mut agree = 0;
    for (label, explorer) in EXPLORER {
        let got = available(label);
        eprintln!("{label}: explorer {explorer:?}, engine {got:?}");
        if explorer.agrees(&got) {
            agree += 1;
        }
    }
    // Homographs: the explorer matches labels exactly and has no `a11ce`
    // domain, but `a11ce` is `alice`'s normalized label, so it is taken.
    let homograph = available("A11CE");
    eprintln!("A11CE: engine {homograph:?}");
    let own = available(&own_name);
    eprintln!("{own_name} (fixture A): engine {own:?}");
    eprintln!("{agree}/{} agree with the explorer", EXPLORER.len());
    assert_eq!(agree, EXPLORER.len());
    assert!(Explorer::Taken("FKZZFDTfGdSWUmL2g7H9e46pMJMPQp9DHQcvjrsS6884").agrees(&homograph));
    assert_eq!(
        own,
        NameAvailability::Taken {
            owner: Some(identity)
        }
    );
    engine.block_on(engine.shutdown()).unwrap();
}
