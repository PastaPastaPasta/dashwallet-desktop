//! dashd-generated SQLite descriptor wallets (testdata/compat, made by
//! testdata/oracle/dashd/gen_compat_vectors.py): the reader recovers the
//! phrase and passphrase, Core's BIP39 seed of them is the wallet's master
//! key, and the first addresses of every chain equal dashd's
//! `deriveaddresses` (QT-106, DESIGN-opus §4.4 item 1).

use std::path::PathBuf;
use std::str::FromStr;

use dashcore::{Address, Network, PublicKey};
use dw_compat::bip39core::core_seed;
use dw_compat::descriptor::parse_listdescriptors;
use dw_compat::walletdat::{WalletDatError, read_sqlite};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/compat")
}

fn manifest() -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(testdata().join("manifest.json")).unwrap()).unwrap()
}

fn addresses(master: &ExtendedPrivKey, chain: &str, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let path = DerivationPath::from_str(&format!("{chain}/{i}")).unwrap();
            let child = master.derive_priv(&path).unwrap();
            let pk = PublicKey::new(child.private_key.public_key());
            Address::p2pkh(&pk, Network::Regtest).to_string()
        })
        .collect()
}

fn check_chains(master: &ExtendedPrivKey, expected: &serde_json::Value) {
    for (chain, path) in [
        ("external", "m/44'/1'/0'/0"),
        ("internal", "m/44'/1'/0'/1"),
        ("coinjoin", "m/9'/1'/4'/0'/0"),
    ] {
        let Some(want) = expected.get(chain) else {
            continue;
        };
        let want: Vec<String> = serde_json::from_value(want.clone()).unwrap();
        assert_eq!(addresses(master, path, want.len()), want, "{chain}");
    }
}

#[test]
fn test_qt_106_wallet_dat_restores_dashd_addresses() {
    let m = manifest();
    for w in m["wallets"].as_array().unwrap() {
        let file = w["file"].as_str().unwrap();
        let wallet = read_sqlite(&testdata().join(file)).unwrap();
        let pass = w["wallet_passphrase"]
            .as_str()
            .filter(|p| !p.is_empty())
            .map(str::as_bytes);
        if pass.is_some() {
            assert_eq!(
                wallet.hd_root(None).unwrap_err(),
                WalletDatError::PassphraseRequired,
                "{file}"
            );
        }
        let root = wallet.hd_root(pass).unwrap();
        let master = match (&root.mnemonic, w.get("mnemonic")) {
            (Some((phrase, passphrase)), Some(want)) => {
                assert_eq!(&phrase[..], want.as_str().unwrap().as_bytes(), "{file}");
                assert_eq!(
                    &passphrase[..],
                    w["mnemonic_passphrase"].as_str().unwrap().as_bytes(),
                    "{file}"
                );
                let seed = core_seed(phrase, passphrase);
                ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap()
            }
            (None, None) => {
                let xprv = ExtendedPrivKey::from_str(w["xprv"].as_str().unwrap()).unwrap();
                assert_eq!(xprv.private_key.to_secret_bytes(), *root.master_secret);
                xprv
            }
            other => panic!("{file}: mnemonic presence differs: {:?}", other.1),
        };
        assert_eq!(
            master.private_key.public_key().serialize(),
            root.master_pubkey,
            "{file}: phrase does not give the wallet's master key"
        );
        check_chains(&master, &w["addresses"]);
        for label in w["labels"].as_array().into_iter().flatten() {
            let address = label["address"].as_str().unwrap();
            let entry = wallet
                .address_book
                .iter()
                .find(|e| e.address == address)
                .unwrap_or_else(|| panic!("{file}: no book entry for {address}"));
            assert_eq!(entry.label, label["label"].as_str().unwrap());
            assert_eq!(entry.purpose.as_deref(), Some("receive"));
        }
    }
}

#[test]
fn test_qt_108_listdescriptors_restores_dashd_addresses() {
    let m = manifest();
    let json = std::fs::read(testdata().join("listdescriptors_plain.json")).unwrap();
    let listed = parse_listdescriptors(&json).unwrap();
    let plain = &m["wallets"][0];
    let xprv = ExtendedPrivKey::from_str(&listed.external.key).unwrap();
    let (phrase, passphrase) = listed.mnemonic.as_ref().unwrap();
    let seed = core_seed(phrase.as_bytes(), passphrase.as_bytes());
    let from_seed = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
    assert_eq!(xprv.private_key, from_seed.private_key);
    assert_eq!(xprv.chain_code, from_seed.chain_code);
    check_chains(&xprv, &plain["addresses"]);
}
