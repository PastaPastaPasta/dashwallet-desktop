//! A wallet Dash Core 23.0.0 to 23.1.x could create and v24.0.0 refuses: a
//! descriptor wallet whose BIP39 passphrase is over 256 bytes, its salt cut
//! at 256 (legacy wallets refuse over 256 bytes from 19.2.0 on, and
//! descriptor wallets took a mnemonic only from 23.0.0). testdata/core_v23_long_passphrase.json was made once from the
//! v23.1.10 release by testdata/oracle/dashd/gen_long_passphrase.py; the
//! dash-qt restore (QT-104) must derive dashd's master key and addresses.

use std::str::FromStr;

use dashcore::{Address, Network, PublicKey};
use dw_vault::{SeedDerivation, mnemonic};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

const VECTOR: &str = include_str!("../../../../testdata/core_v23_long_passphrase.json");

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

#[test]
fn a_v23_wallet_with_a_passphrase_over_256_bytes_restores_to_dashds_addresses() {
    let doc: serde_json::Value = serde_json::from_str(VECTOR).unwrap();
    let s = |k: &str| doc[k].as_str().unwrap();
    assert!(
        s("source").contains("/Dash Core:23.1.10/"),
        "{}",
        s("source")
    );
    assert_eq!(s("archive"), "dashcore-23.1.10-x86_64-linux-gnu.tar.gz");
    assert_eq!(
        s("archive_sha256"),
        "09f980572ac65b9b109c7d7dd497806d11cc7e8ece0031333ca390f2d4054081"
    );
    let passphrase = s("passphrase").as_bytes();
    assert_eq!(passphrase.len(), 314);
    assert_eq!(doc["passphrase_bytes"], 314);

    let secret = mnemonic::derive_secret(s("mnemonic").as_bytes(), passphrase, true).unwrap();
    assert!(matches!(
        secret.derivation,
        SeedDerivation::DashCore {
            weak_checksum: true
        }
    ));
    // "mnemonic" + the passphrase is cut at 256 bytes: the 248th byte is the last one used.
    let cut = mnemonic::derive_secret(s("mnemonic").as_bytes(), &passphrase[..248], true).unwrap();
    assert_eq!(cut.seed, secret.seed);

    let master = ExtendedPrivKey::new_master(Network::Regtest, &secret.seed[..]).unwrap();
    assert_eq!(master.to_string(), s("descriptor_master_xprv"));
    for (chain, path) in [("external", "m/44'/1'/0'/0"), ("internal", "m/44'/1'/0'/1")] {
        let want: Vec<String> = serde_json::from_value(doc[chain].clone()).unwrap();
        assert_eq!(want.len(), 5, "{chain}");
        assert_eq!(addresses(&master, path, want.len()), want, "{chain}");
    }
}
