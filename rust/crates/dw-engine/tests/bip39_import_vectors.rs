//! A phrase and BIP39 passphrase imported through the engine's own entry
//! (`NetworkSession::import_wallet`, strict BIP39) give the seed, wallet id
//! and first receive address that the previous rust-dashcore pin (seed from
//! the bip39 crate) gave: non-English phrases, passphrases that NFKD
//! changes, and the same text typed composed and decomposed.
//!
//! Phrases and passphrases are public vectors: bip32JP's test_JP_BIP39.json
//! entries 0, 11 and 23 (U+3000 between words, as published), BIP39's
//! all-zero entropy in the Spanish wordlist, and `abandon … about`.
//!
//! The expected values were computed at desktop main f057bee77 (rust-dashcore
//! a0f687ec62, bip39 2.2.2) by importing each case through the same entry on
//! regtest and reading `current_receive_address`; the seed is
//! `dw_vault::mnemonic::derive_secret`, the function the import calls. The
//! Japanese seeds equal the published ones, and `abandon … about` with
//! "café" equals `bip39_seed` in testdata/bip39_core_quirks.json.

use std::sync::Arc;

use dw_engine::{
    DashNetwork, Engine, EngineConfig, EngineEvent, EventSink, ImportOptions, SessionOptions,
};
use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
use zeroize::Zeroizing;

struct Silent;

impl EventSink for Silent {
    fn emit(&self, _: EngineEvent) {}
}

const JP_PASSPHRASE: &str = "㍍ガバヴァぱばぐゞちぢ十人十色";
const ABANDON_12: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const ES_ZERO_NFC: &str =
    "ábaco ábaco ábaco ábaco ábaco ábaco ábaco ábaco ábaco ábaco ábaco abierto";
const ES_ZERO_NFD: &str = "a\u{301}baco a\u{301}baco a\u{301}baco a\u{301}baco a\u{301}baco \
    a\u{301}baco a\u{301}baco a\u{301}baco a\u{301}baco a\u{301}baco a\u{301}baco abierto";

struct Case {
    name: &'static str,
    phrase: &'static str,
    passphrase: &'static str,
    seed: &'static str,
    wallet_id: &'static str,
    address: &'static str,
}

const CASES: [Case; 8] = [
    Case {
        name: "Japanese, 12 words",
        phrase: "あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あいこくしん　あおぞら",
        passphrase: JP_PASSPHRASE,
        seed: "a262d6fb6122ecf45be09c50492b31f92e9beb7d9a845987a02cefda57a15f9c467a17872029a9e92299b5cbdf306e3a0ee620245cbd508959b6cb7ca637bd55",
        wallet_id: "83eecea8bbcb7e8df0cf6be1eb3c06cf5236effa32fd760b27794ba461e7119d",
        address: "yg12Ed4mHZEyJW21dPtBezibSqGaWPyV5m",
    },
    Case {
        name: "Japanese, 24 words",
        phrase: "われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　われる　らいう",
        passphrase: JP_PASSPHRASE,
        seed: "a44ba7054ac2f9226929d56505a51e13acdaa8a9097923ca07ea465c4c7e294c038f3f4e7e4b373726ba0057191aced6e48ac8d183f3a11569c426f0de414623",
        wallet_id: "205c52d0d394f6154a9eacec344c4b48ff29a76e0bb5e4b00edfc1a47e750223",
        address: "yRPsTYoW4bNi3o9udjfLJoHWHRSkCZQxAQ",
    },
    Case {
        name: "Japanese, 24 words with voiced kana",
        phrase: "うちゅう　ふそく　ひしょ　がちょう　うけもつ　めいそう　みかん　そざい　いばる　うけとる　さんま　さこつ　おうさま　ぱんつ　しひょう　めした　たはつ　いちぶ　つうじょう　てさぎょう　きつね　みすえる　いりぐち　かめれおん",
        passphrase: JP_PASSPHRASE,
        seed: "346b7321d8c04f6f37b49fdf062a2fddc8e1bf8f1d33171b65074531ec546d1d3469974beccb1a09263440fc92e1042580a557fdce314e27ee4eabb25fa5e5fe",
        wallet_id: "cf826234ac65d94fd667296f6ca7e29d2ae2e79de8f41914a51be4020c885202",
        address: "ydbwLyk45k5QJo8tGboHnhtuK614WGXD16",
    },
    Case {
        name: "Spanish NFC, passphrase NFC",
        phrase: ES_ZERO_NFC,
        passphrase: "caf\u{e9}",
        seed: "f0ec91f6e8a5c86dd3df7cfa4f06b0023cb70beebee13dd80e79966adf19a9f4b1e540a6e7caec8c205023fd72bd94953a12f0603cd91a5c6e351d80653e7bd8",
        wallet_id: "5eccebec1d517ff3967c69faf01fd854c59e2526397809da7955d04c485a0343",
        address: "yLLtpDN2BH8WXUTCKB9gPpav8tHsFZQoQx",
    },
    Case {
        name: "Spanish NFD, passphrase NFD",
        phrase: ES_ZERO_NFD,
        passphrase: "cafe\u{301}",
        seed: "f0ec91f6e8a5c86dd3df7cfa4f06b0023cb70beebee13dd80e79966adf19a9f4b1e540a6e7caec8c205023fd72bd94953a12f0603cd91a5c6e351d80653e7bd8",
        wallet_id: "5eccebec1d517ff3967c69faf01fd854c59e2526397809da7955d04c485a0343",
        address: "yLLtpDN2BH8WXUTCKB9gPpav8tHsFZQoQx",
    },
    Case {
        name: "English, passphrase NFC",
        phrase: ABANDON_12,
        passphrase: "caf\u{e9}",
        seed: "af8bbd2566df7b69d926f2b09dfdbd75db6c994a3399b2cc65f928d63e3fd4e61218ee0d15f8c810be4d45e66d47b43c15a5cc753976b1666912377ff7ae9818",
        wallet_id: "570c9786c03d9a9ebd4f137e763bcd96460e2c74ae9f531a7f68d9168b508920",
        address: "yZwsj2zKGy5Wj6D1QYtRVno4yiwgkL1jYF",
    },
    Case {
        name: "English, passphrase NFD",
        phrase: ABANDON_12,
        passphrase: "cafe\u{301}",
        seed: "af8bbd2566df7b69d926f2b09dfdbd75db6c994a3399b2cc65f928d63e3fd4e61218ee0d15f8c810be4d45e66d47b43c15a5cc753976b1666912377ff7ae9818",
        wallet_id: "570c9786c03d9a9ebd4f137e763bcd96460e2c74ae9f531a7f68d9168b508920",
        address: "yZwsj2zKGy5Wj6D1QYtRVno4yiwgkL1jYF",
    },
    Case {
        name: "English, mixed non-ASCII passphrase",
        phrase: ABANDON_12,
        passphrase: "pässwörd ñ \u{20ac} \u{1F511}",
        seed: "188812f866dccd56b3bd3921a3266697b5ab1f6c55e83be1d40b9b8de674c0588ff037d27eff7b9438940111be45e0c9eaca2f22cbf6d8a227245328c1f9137f",
        wallet_id: "7453f2a658171d24d04693c80994ca30e18ba25717ace2b5aa7994f202a93876",
        address: "yT37mtSi7Jfqg8sRCTGGfr5Nj1GXeLCWRT",
    },
];

/// Imports `phrase` with `passphrase` into a fresh data root and returns the
/// wallet id and the first receive address with its path.
fn import_fresh(phrase: &str, passphrase: &str) -> (String, String, String) {
    let dir = dw_testutil::private_tempdir();
    let engine = Engine::new(
        EngineConfig {
            data_root: dir.path().to_path_buf(),
            worker_threads: Some(2),
            vault: VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            },
        },
        Arc::new(Silent),
    )
    .unwrap();
    let session = engine
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
    engine
        .block_on(session.vault_op(|v| v.create(None)))
        .unwrap();
    let id = engine
        .block_on(session.import_wallet(
            Zeroizing::new(phrase.as_bytes().to_vec()),
            Zeroizing::new(passphrase.as_bytes().to_vec()),
            ImportOptions {
                birth_height: Some(0),
                ..ImportOptions::default()
            },
        ))
        .unwrap();
    let first = engine
        .block_on(session.current_receive_address(id))
        .unwrap();
    drop(session);
    engine.block_on(engine.shutdown()).unwrap();
    (id.to_string(), first.address, first.derivation_path)
}

#[test]
fn imported_phrases_keep_the_previous_pins_seed_id_and_first_address() {
    for case in &CASES {
        let secret = dw_vault::mnemonic::derive_secret(
            case.phrase.as_bytes(),
            case.passphrase.as_bytes(),
            false,
        )
        .unwrap_or_else(|e| panic!("{}: {e:?}", case.name));
        assert_eq!(
            hex::encode(&secret.seed[..]),
            case.seed,
            "{}: seed",
            case.name
        );
        let (id, address, path) = import_fresh(case.phrase, case.passphrase);
        assert_eq!(id, case.wallet_id, "{}: wallet id", case.name);
        assert_eq!(
            address, case.address,
            "{}: first receive address",
            case.name
        );
        assert_eq!(path, "m/44'/1'/0'/0/0", "{}", case.name);
    }
}
