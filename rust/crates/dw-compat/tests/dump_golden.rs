//! testdata/dumpwallet/*.txt: files written by regtest dashd v24
//! `dumpwallet`, with testdata/dumpwallet/manifest.json recording (from
//! other RPCs) what each file must contain.

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, Secp256k1, SecretKey};
use dw_compat::bip39core;
use dw_compat::dump::{self, DumpFile, DumpHeader, HdAccount, KeyEntry, KeyRole, ScriptEntry};
use dw_uri::Network;
use dw_uri::keyio::{self, Destination};
use proptest::prelude::*;
use serde_json::Value;
use zeroize::Zeroizing;

const TESTDATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../testdata");

fn manifest() -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!("{TESTDATA}/dumpwallet/manifest.json")).unwrap(),
    )
    .unwrap()
}

fn read(file: &str) -> String {
    std::fs::read_to_string(format!("{TESTDATA}/{file}")).unwrap()
}

fn p2pkh_of(secret: &keyio::Secret) -> [u8; 20] {
    let secp = Secp256k1::signing_only();
    let pk = PublicKey::from_secret_key(&secp, &SecretKey::from_byte_array(&secret.key).unwrap());
    let ser = if secret.compressed {
        pk.serialize().to_vec()
    } else {
        pk.serialize_uncompressed().to_vec()
    };
    hash160::Hash::hash(&ser).to_byte_array()
}

#[test]
fn dashd_dumps_parse_and_rewrite_byte_identically() {
    let m = manifest();
    let dumps = m["dumps"].as_array().unwrap();
    assert!(dumps.len() >= 4);
    for d in dumps {
        let file = d["file"].as_str().unwrap();
        let text = read(file);
        let parsed = dump::parse(&text).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(
            dump::write(&parsed, Network::Regtest).as_str(),
            text,
            "{file}: writer output differs"
        );

        // CRLF files (dumpwallet on Windows) parse to the same content.
        assert_eq!(
            dump::parse(&text.replace('\n', "\r\n")).unwrap(),
            parsed,
            "{file}: CRLF"
        );

        assert!(parsed.header.created_by.starts_with("Dash Core v24"));
        assert_eq!(parsed.header.best_block_height, 0);

        // Every key's WIF decodes for regtest and matches its address.
        for k in &parsed.keys {
            let secret = k
                .secret(Network::Regtest)
                .unwrap_or_else(|| panic!("{file}: bad WIF"));
            let want = keyio::decode_destination(&k.address, Network::Regtest).unwrap();
            assert_eq!(
                want,
                Destination::PubKeyHash(p2pkh_of(&secret)),
                "{file}: {}",
                k.address
            );
        }

        match d.get("mnemonic") {
            Some(mnemonic) => {
                let hd = parsed.hd.as_ref().expect("HD section");
                let pass = d["mnemonic_passphrase"].as_str().unwrap();
                assert_eq!(hd.mnemonic.as_str(), mnemonic.as_str().unwrap());
                assert_eq!(hd.mnemonic_passphrase.as_str(), pass);
                assert_eq!(hd.hd_seed_hex.as_str(), d["hd_seed"].as_str().unwrap());
                // The seed in the file is Core's BIP39 seed of the phrase.
                let seed = bip39core::core_seed(hd.mnemonic.as_bytes(), pass.as_bytes());
                let seed_hex: String = seed.iter().map(|b| format!("{b:02x}")).collect();
                assert_eq!(seed_hex, hd.hd_seed_hex.as_str());
                let rebuilt = dump::hd_section_from_seed(
                    &hd.mnemonic,
                    pass,
                    &seed[..],
                    hd.accounts.clone(),
                    Network::Regtest,
                )
                .unwrap();
                assert_eq!(&rebuilt, hd, "{file}: xprv/xpub from seed");
                assert!(matches!(
                    hd.accounts.as_slice(),
                    [HdAccount::Counters { .. }]
                ));
            }
            None => assert!(parsed.hd.is_none(), "{file}: unexpected HD section"),
        }

        let by_addr = |a: &str| parsed.keys.iter().find(|k| k.address == a);
        for a in d["labelled_addresses"].as_array().into_iter().flatten() {
            let k = by_addr(a["address"].as_str().unwrap()).expect("labelled address dumped");
            assert_eq!(
                k.role,
                KeyRole::Label(a["label"].as_str().unwrap().to_owned())
            );
            assert_eq!(k.hdkeypath.as_deref(), a["hdkeypath"].as_str());
        }
        for a in d["change_addresses"].as_array().into_iter().flatten() {
            assert_eq!(
                by_addr(a.as_str().unwrap())
                    .expect("change address dumped")
                    .role,
                KeyRole::Change
            );
        }
        for i in d["imported_keys"].as_array().into_iter().flatten() {
            let k = parsed
                .keys
                .iter()
                .find(|k| *k.wif == i["wif"].as_str().unwrap())
                .expect("imported key dumped");
            assert_eq!(
                k.role,
                KeyRole::Label(i["label"].as_str().unwrap().to_owned())
            );
            assert!(!k.is_hd());
        }
        for sc in d["scripts"].as_array().into_iter().flatten() {
            let found = parsed
                .scripts
                .iter()
                .find(|s| s.address == sc["address"].as_str().unwrap())
                .expect("script");
            assert_eq!(found.script_hex, sc["redeem_script"].as_str().unwrap());
            assert_eq!(found.time, None);
        }
    }
}

#[test]
fn rejects_non_dumps() {
    assert!(dump::parse("hello\n").is_err());
    assert!(dump::parse("").is_err());
}

fn arb_label() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[ -~é€\t%#]{0,12}").unwrap()
}

fn sample_key(i: u8, role: KeyRole, time: i64) -> KeyEntry {
    let secret = keyio::Secret {
        key: Zeroizing::new([i.max(1); 32]),
        compressed: i.is_multiple_of(2),
    };
    let address = keyio::encode_destination(
        &Destination::PubKeyHash(p2pkh_of(&secret)),
        Network::Testnet,
    );
    KeyEntry {
        wif: Zeroizing::new(keyio::encode_secret(&secret, Network::Testnet).to_string()),
        time: dump::format_iso8601(time),
        role,
        address,
        hdkeypath: i.is_multiple_of(3).then(|| format!("m/44'/1'/0'/0/{i}")),
    }
}

proptest! {
    #[test]
    fn dump_string_round_trip(s in "\\PC{0,40}") {
        prop_assert_eq!(dump::decode_dump_string(&dump::encode_dump_string(&s)), s);
    }

    // Anything the writer produces parses back to the same content, and the
    // writer is insensitive to the order keys are given in.
    #[test]
    fn write_parse_round_trip(labels in prop::collection::vec(arb_label(), 1..6),
                              times in prop::collection::vec(1i64..2_000_000_000, 6),
                              passphrase in "[ -~]{0,20}") {
        let keys: Vec<KeyEntry> = labels.iter().enumerate().map(|(i, l)| {
            let role = match i % 3 { 0 => KeyRole::Label(l.clone()), 1 => KeyRole::Reserve, _ => KeyRole::Change };
            sample_key(i as u8 + 1, role, times[i])
        }).collect();
        let seed = bip39core::core_seed(b"abandon", passphrase.as_bytes());
        let hd = dump::hd_section_from_seed("abandon", &passphrase, &seed[..],
            vec![HdAccount::Counters { external: 3, internal: 1 }, HdAccount::Missing], Network::Testnet).unwrap();
        let file = DumpFile {
            header: DumpHeader {
                created_by: "Dash Core v24.0.0".into(),
                created_on: dump::format_iso8601(times[5]),
                best_block_height: 42,
                best_block_hash: "00".repeat(32),
                best_block_time: dump::format_iso8601(times[4]),
            },
            hd: Some(hd),
            keys: keys.clone(),
            scripts: vec![ScriptEntry {
                script_hex: "51".into(),
                time: None,
                address: keyio::encode_destination(&Destination::ScriptHash([9; 20]), Network::Testnet),
            }],
        };
        let text = dump::write(&file, Network::Testnet);
        let mut reversed = file.clone();
        reversed.keys.reverse();
        let text_reversed = dump::write(&reversed, Network::Testnet);
        prop_assert_eq!(text_reversed.as_str(), text.as_str());
        let back = dump::parse(&text).unwrap();
        prop_assert_eq!(back.hd, file.hd);
        prop_assert_eq!(back.header, file.header);
        prop_assert_eq!(back.scripts, file.scripts);
        let mut want = keys;
        want.sort_by_key(|k| dump::parse_iso8601(&k.time));
        prop_assert_eq!(back.keys.len(), want.len());
        for k in &want {
            prop_assert!(back.keys.contains(k));
        }
    }
}
