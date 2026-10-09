//! Roadmap DP1-01: for the same seed and identity index, the key set the
//! engine derives equals iOS's: ids, purposes, security levels, key types,
//! contract bounds, DIP-13 paths and public keys.
//!
//! `fixtures/dp1_01_identity_key_vectors.json` lists where each value came
//! from (`sources`; see `fixtures/README.md`). Each public key and path is
//! derived twice: by an independent from-scratch Python derivation, and by
//! the `rs-platform-wallet-ffi` entry points SwiftDashSDK calls for iOS.
//! Keys here go phrase → seed (dw-vault) → vault `PlatformIdentity` signer →
//! `keys_policy::derive_keys`, so the test covers the path DP1-02 takes.

use std::collections::BTreeMap;
use std::sync::Arc;

use dpp::identity::identity_public_key::accessors::v0::{
    IdentityPublicKeyGettersV0, IdentityPublicKeySettersV0,
};
use dpp::identity::identity_public_key::contract_bounds::ContractBounds;
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::{IdentityPublicKey, KeyID, KeyType, Purpose, SecurityLevel};
use dpp::platform_value::string_encoding::Encoding;
use dpp::state_transition::public_key_in_creation::IdentityPublicKeyInCreation;
use dpp::system_data_contracts::dashpay_contract;
use dpp::version::PlatformVersion;
use dw_engine::platform::keys_policy;
use dw_vault::{
    Credential, GrantKind, GrantPurpose, KdfPolicy, MemoryOsStore, SignerScope, SystemClock, Vault,
    VaultConfig, VaultSigner,
};
use key_wallet::Network;
use serde_json::Value;

const WALLET: [u8; 32] = [7; 32];

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/dp1_01_identity_key_vectors.json")).unwrap()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap()
}

fn num(v: &Value) -> u32 {
    u32::try_from(v.as_u64().unwrap()).unwrap()
}

fn network(v: &Value) -> Network {
    match text(v) {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        "regtest" => Network::Regtest,
        other => panic!("network {other}"),
    }
}

/// A `PlatformIdentity` signer over an unencrypted vault holding the wallet
/// of `mnemonic` and `passphrase` (BIP39) on `network`, and the vault's
/// directory.
fn signer(
    mnemonic: &str,
    passphrase: &str,
    network: Network,
    seed: &str,
) -> (tempfile::TempDir, VaultSigner) {
    let dir = dw_testutil::private_tempdir();
    let config = VaultConfig {
        kdf: KdfPolicy::Calibrated,
        os_store: Arc::new(MemoryOsStore::new()),
        clock: Arc::new(SystemClock),
        grant_ttl_secs: 120,
    };
    let v = Vault::open(dir.path().join("vault"), network, "dp1-01", config).unwrap();
    v.create(None).unwrap();
    let secret =
        dw_vault::mnemonic::derive_secret(mnemonic.as_bytes(), passphrase.as_bytes(), false)
            .unwrap();
    assert_eq!(hex::encode(*secret.seed), seed, "BIP39 seed");
    v.store_wallet_secret(&WALLET, &secret).unwrap();
    let grant = v
        .authorize(
            GrantPurpose::PlatformOp {
                max_duffs: 0,
                max_credits: 1,
            },
            Some(&WALLET),
            Credential::None,
        )
        .unwrap();
    let token = v
        .redeem_grant(&grant.id, GrantKind::PlatformOp, Some(&WALLET))
        .unwrap();
    let signer = v
        .platform_signer(&WALLET, &token, SignerScope::PlatformIdentity)
        .unwrap();
    (dir, signer)
}

fn purpose(v: &Value) -> Purpose {
    match text(v) {
        "AUTHENTICATION" => Purpose::AUTHENTICATION,
        "ENCRYPTION" => Purpose::ENCRYPTION,
        "DECRYPTION" => Purpose::DECRYPTION,
        "TRANSFER" => Purpose::TRANSFER,
        other => panic!("purpose {other}"),
    }
}

fn security_level(v: &Value) -> SecurityLevel {
    match text(v) {
        "MASTER" => SecurityLevel::MASTER,
        "CRITICAL" => SecurityLevel::CRITICAL,
        "HIGH" => SecurityLevel::HIGH,
        "MEDIUM" => SecurityLevel::MEDIUM,
        other => panic!("security level {other}"),
    }
}

fn contract_bounds(v: &Value) -> Option<ContractBounds> {
    if v.is_null() {
        return None;
    }
    let id: [u8; 32] = hex::decode(text(&v["contract"]))
        .unwrap()
        .try_into()
        .unwrap();
    Some(ContractBounds::SingleContractDocumentType {
        id: id.into(),
        document_type_name: text(&v["document_type"]).to_owned(),
    })
}

/// The key a fixture row describes, as IdentityCreate would carry it.
fn expected_key(row: &Value) -> IdentityPublicKey {
    assert_eq!(text(&row["key_type"]), "ECDSA_SECP256K1");
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id: num(&row["id"]),
        purpose: purpose(&row["purpose"]),
        security_level: security_level(&row["security_level"]),
        contract_bounds: contract_bounds(&row["contract_bounds"]),
        key_type: KeyType::ECDSA_SECP256K1,
        read_only: false,
        data: hex::decode(text(&row["public_key"])).unwrap().into(),
        disabled_at: None,
    })
}

fn assert_valid_structure(keys: &BTreeMap<KeyID, IdentityPublicKey>, in_create_identity: bool) {
    let keys: Vec<IdentityPublicKeyInCreation> = keys.values().map(Into::into).collect();
    let result = IdentityPublicKeyInCreation::validate_identity_public_keys_structure(
        &keys,
        in_create_identity,
        PlatformVersion::latest(),
    )
    .unwrap();
    assert!(result.is_valid(), "{:?}", result.errors);
}

#[test]
fn dashpay_contract_is_the_ios_contract() {
    let v = vectors();
    assert_eq!(
        hex::encode(dashpay_contract::ID_BYTES),
        text(&v["dashpay_contract_id"])
    );
    assert_eq!(
        dashpay_contract::ID.to_string(Encoding::Base58),
        text(&v["dashpay_contract_id_base58"])
    );
}

#[tokio::test]
async fn registration_key_sets_match_the_vectors() {
    let v = vectors();
    let sets = v["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 14);
    for set in sets {
        let network = network(&set["network"]);
        let identity_index = num(&set["identity_index"]);
        let case = format!("{} {}", set["network"], identity_index);
        let (_dir, signer) = signer(
            text(&set["mnemonic"]),
            text(&set["passphrase"]),
            network,
            text(&set["seed"]),
        );
        let keys =
            keys_policy::derive_keys(&signer, identity_index, &keys_policy::registration_specs())
                .await
                .unwrap();
        let rows = set["keys"].as_array().unwrap();
        assert_eq!(keys.len(), rows.len(), "{case}");
        for row in rows {
            let id = num(&row["id"]);
            assert_eq!(keys[&id], expected_key(row), "{case} key {id}");
            assert_eq!(
                keys_policy::key_path(network, identity_index, id)
                    .unwrap()
                    .to_string(),
                text(&row["path"]),
                "{case} key {id}"
            );
        }
        assert_valid_structure(&keys, true);
    }
}

/// "Enable DashPay keys" on an identity whose ENCRYPTION key at id 4 was
/// disabled and which has an AUTHENTICATION key at id 5: both DashPay keys
/// go to ids 6 and 7, the slots of the `upgrade_slots` vectors.
#[tokio::test]
async fn upgrade_keys_match_the_vectors() {
    let v = vectors();
    let up = &v["upgrade_slots"];
    let base = v["sets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| {
            s["mnemonic"] == up["mnemonic"]
                && s["passphrase"] == ""
                && s["network"] == up["network"]
                && s["identity_index"] == up["identity_index"]
        })
        .unwrap();
    let identity_index = num(&up["identity_index"]);
    let (_dir, signer) = signer(
        text(&up["mnemonic"]),
        "",
        network(&up["network"]),
        text(&base["seed"]),
    );

    let mut existing: Vec<IdentityPublicKey> = base["keys"].as_array().unwrap()[..4]
        .iter()
        .map(expected_key)
        .collect();
    let mut disabled = expected_key(&base["keys"][4]);
    disabled.set_disabled_at(1);
    existing.push(disabled);
    let mut auth = expected_key(&base["keys"][2]);
    auth.set_id(5);
    existing.push(auth);

    let specs = keys_policy::upgrade_specs(&existing, None);
    let ids: Vec<KeyID> = specs.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![6, 7]);
    let keys = keys_policy::derive_keys(&signer, identity_index, &specs)
        .await
        .unwrap();
    assert_eq!(keys.len(), 2);
    // The registration pair's metadata (keys 4 and 5) at the new slots.
    for (row, pair_row) in up["keys"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&base["keys"].as_array().unwrap()[4..])
    {
        let mut expected = expected_key(pair_row);
        expected.set_id(num(&row["id"]));
        expected.set_data(hex::decode(text(&row["public_key"])).unwrap().into());
        assert_eq!(keys[&expected.id()], expected);
    }
    assert_valid_structure(&keys, false);
}

#[tokio::test]
async fn duplicate_ids_are_refused() {
    let v = vectors();
    let set = &v["sets"][0];
    let (_dir, signer) = signer(
        text(&set["mnemonic"]),
        text(&set["passphrase"]),
        network(&set["network"]),
        text(&set["seed"]),
    );
    let mut specs = keys_policy::registration_specs();
    specs.push(specs[0].clone());
    let err = keys_policy::derive_keys(&signer, 0, &specs)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("duplicate identity key id 0"),
        "{err}"
    );
}
