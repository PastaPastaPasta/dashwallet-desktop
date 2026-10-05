//! testdata/bip39_core_quirks.json: Dash Core's own bip39.cpp (compiled by
//! testdata/oracle/bip39) and regtest dashd v24 `upgradetohd` results.

use dw_compat::bip39core::{self, Bip39Error};
use dw_uri::Network;
use proptest::prelude::*;
use serde_json::Value;

fn load() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../testdata/bip39_core_quirks.json"
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str()
        .unwrap_or_else(|| panic!("missing {k} in {v}"))
}

#[test]
fn trezor_vectors_through_core() {
    let doc = load();
    for c in doc["trezor"].as_array().unwrap() {
        let entropy = unhex(s(c, "entropy"));
        let phrase = s(c, "mnemonic");
        assert_eq!(bip39core::from_entropy(&entropy).unwrap().as_str(), phrase);
        assert!(bip39core::core_check(phrase.as_bytes()));
        assert_eq!(
            bip39core::strict_check(phrase).unwrap().as_slice(),
            entropy.as_slice()
        );
        let seed = bip39core::core_seed(phrase.as_bytes(), s(c, "passphrase").as_bytes());
        assert_eq!(hex(&seed[..]), s(c, "seed"));
        let master = key_wallet::ExtendedPrivKey::new_master(Network::Mainnet, &seed[..]).unwrap();
        assert_eq!(master.to_string(), s(c, "xprv"));
    }
}

#[test]
fn check_matches_core_and_strict() {
    let doc = load();
    let cases = doc["check"].as_array().unwrap();
    let mut weak = 0;
    for c in cases {
        let phrase = s(c, "mnemonic");
        let core = c["core_check"].as_bool().unwrap();
        let strict = c["strict_check"].as_bool().unwrap();
        assert_eq!(
            bip39core::core_check(phrase.as_bytes()),
            core,
            "core {}: {phrase:?}",
            c["case"]
        );
        assert_eq!(
            bip39core::strict_check(phrase).is_ok(),
            strict,
            "strict {}: {phrase:?}",
            c["case"]
        );
        // `validate` normalizes first, so only compare on already-normal input.
        if *bip39core::normalize_phrase(phrase) == *phrase {
            match bip39core::validate(phrase) {
                Ok(v) => {
                    assert!(core, "{}", c["case"]);
                    assert_eq!(v.weak, !strict, "{}", c["case"]);
                    weak += usize::from(v.weak);
                    if let Some(e) = c["entropy"].as_str() {
                        assert_eq!(hex(&v.entropy), e);
                    }
                }
                Err(_) => assert!(!core, "{}", c["case"]),
            }
        }
    }
    assert_eq!(
        weak, 131,
        "every weak-checksum phrase in the vectors is accepted as weak"
    );
}

#[test]
fn seeds_match_core_and_portability_flag_is_right() {
    let doc = load();
    for c in doc["seed"].as_array().unwrap() {
        let (m, p) = (s(c, "mnemonic"), s(c, "passphrase"));
        let seed = bip39core::core_seed(m.as_bytes(), p.as_bytes());
        assert_eq!(hex(&seed[..]), s(c, "core_seed"), "{}", c["case"]);
        let same_as_bip39 = s(c, "core_seed") == s(c, "bip39_seed");
        assert_eq!(
            bip39core::seed_portability(m, p).is_portable(),
            same_as_bip39,
            "{}",
            c["case"]
        );
    }
}

#[test]
fn dashd_upgradetohd_results() {
    let doc = load();
    let cases = doc["dashd"]["cases"].as_array().unwrap();
    assert!(
        cases.iter().any(|c| c.get("legacy_error").is_some()),
        "the >256-byte legacy rejection is recorded"
    );
    for c in cases {
        let (m, p) = (s(c, "mnemonic"), s(c, "passphrase"));
        let seed = bip39core::core_seed(m.as_bytes(), p.as_bytes());
        match (c.get("legacy_seed"), c.get("legacy_error")) {
            (Some(want), _) => {
                assert!(p.len() <= bip39core::LEGACY_MAX_PASSPHRASE_BYTES);
                assert_eq!(hex(&seed[..]), want.as_str().unwrap(), "{}", c["case"]);
            }
            (None, Some(e)) => {
                assert!(
                    p.len() > bip39core::LEGACY_MAX_PASSPHRASE_BYTES,
                    "{}",
                    c["case"]
                );
                assert!(e.as_str().unwrap().contains("too long"));
            }
            _ => panic!("case without legacy result: {c}"),
        }
        let master = key_wallet::ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
        assert_eq!(
            master.to_string(),
            s(c, "descriptor_master_xprv"),
            "{}",
            c["case"]
        );
        let v = bip39core::validate(m).unwrap();
        assert_eq!(v.weak, s(c, "case").contains("weak"), "{}", c["case"]);
    }
}

#[test]
fn errors() {
    assert_eq!(
        bip39core::validate("abandon").unwrap_err(),
        Bip39Error::WordCount(1)
    );
    let bad = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandonx";
    assert_eq!(
        bip39core::validate(bad).unwrap_err(),
        Bip39Error::UnknownWord(11)
    );
    // Core's platformkeys_tests bad mnemonic fails both checks.
    let core_bad =
        "birth kingdom trash renew flavor utility donkey gasp regular alert pave kingdom";
    assert_eq!(
        bip39core::validate(core_bad).unwrap_err(),
        Bip39Error::Checksum
    );
    assert_eq!(
        bip39core::from_entropy(&[0; 15]).unwrap_err(),
        Bip39Error::EntropyLength(15)
    );
}

proptest! {
    #[test]
    fn entropy_round_trip(len in prop::sample::select(vec![16usize, 20, 24, 28, 32]),
                          seed in any::<[u8; 32]>()) {
        let entropy = &seed[..len];
        let phrase = bip39core::from_entropy(entropy).unwrap();
        prop_assert!(bip39core::core_check(phrase.as_bytes()));
        let v = bip39core::validate(&phrase.to_uppercase().replace(' ', "  \t")).unwrap();
        prop_assert!(!v.weak);
        prop_assert_eq!(v.entropy.as_slice(), entropy);
        prop_assert_eq!(v.phrase.as_str(), phrase.as_str());
    }

    // Strict acceptance implies Core acceptance: the weak check compares a
    // subset of the checksum bits.
    #[test]
    fn strict_implies_core(words in prop::collection::vec(0usize..2048, 12..=12), last in 0usize..2048) {
        let list = bip39core::wordlist();
        let mut w: Vec<&str> = words.iter().map(|&i| list[i]).collect();
        w[11] = list[last];
        let phrase = w.join(" ");
        if bip39core::strict_check(&phrase).is_ok() {
            prop_assert!(bip39core::core_check(phrase.as_bytes()));
        }
    }

    #[test]
    fn core_check_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let _ = bip39core::core_check(&bytes);
    }
}
