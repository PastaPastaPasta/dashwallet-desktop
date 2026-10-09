//! Roadmap E0-03: the vault-backed Platform signers against fixed vectors.
//!
//! `fixtures/e0_03_signer_vectors.json` was produced by the library's own
//! test `SeedCryptoProvider` (platform `bc321362b9`,
//! `rs-platform-wallet/src/wallet/identity/network/contact_requests.rs:171-345`,
//! `#[cfg(test)] pub(crate)`, so it cannot be called from here) in a one-off
//! test run inside a scratch platform checkout; identity keys and signatures
//! come from key-wallet's `Wallet::derive_extended_private_key` and
//! `dashcore::signer::sign`, as `dash_sdk_sign_with_mnemonic_resolver_and_path`
//! signs. Two seeds, testnet and mainnet. See `fixtures/README.md`.
//!
//! Every output must be byte-identical to the vector, and every identity
//! signature must verify the way Platform checks ECDSA identity keys
//! (`verify_data_signature`, `verify_hash_signature`).

use std::str::FromStr;
use std::sync::Arc;

use dashcore::secp256k1::{PublicKey, Secp256k1, SecretKey};
use dpp::identity::identity_public_key::v0::IdentityPublicKeyV0;
use dpp::identity::signer::Signer;
use dpp::identity::{IdentityPublicKey, KeyType, Purpose, SecurityLevel};
use dw_engine::platform::{VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
use dw_vault::{
    Credential, GrantPurpose, GrantToken, KdfPolicy, MemoryOsStore, SeedDerivation, SignerScope,
    SystemClock, Vault, VaultConfig, WalletSecret,
};
use key_wallet::Network;
use key_wallet::account::{AccountType, StandardAccountType};
use key_wallet::bip32::DerivationPath;
use platform_wallet::ContactCryptoProvider;
use serde_json::Value;
use zeroize::Zeroizing;

const WALLET: [u8; 32] = [7; 32];

fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/e0_03_signer_vectors.json")).unwrap()
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(v.as_str().unwrap()).unwrap()
}

fn array<const N: usize>(v: &Value) -> [u8; N] {
    bytes(v).try_into().unwrap()
}

fn num(v: &Value) -> u32 {
    u32::try_from(v.as_u64().unwrap()).unwrap()
}

fn network(case: &Value) -> Network {
    match case["network"].as_str().unwrap() {
        "mainnet" => Network::Mainnet,
        "testnet" => Network::Testnet,
        other => panic!("network {other}"),
    }
}

fn path(v: &Value) -> DerivationPath {
    DerivationPath::from_str(v.as_str().unwrap()).unwrap()
}

/// An unencrypted vault of the case's network holding the case's seed.
fn vault(case: &Value, dir: &tempfile::TempDir) -> Vault {
    let network = network(case);
    let tag = case["network"].as_str().unwrap();
    let config = VaultConfig {
        kdf: KdfPolicy::Calibrated,
        os_store: Arc::new(MemoryOsStore::new()),
        clock: Arc::new(SystemClock),
        grant_ttl_secs: 120,
    };
    let v = Vault::open(dir.path().join("vault"), network, tag, config).unwrap();
    v.create(None).unwrap();
    let secret = WalletSecret {
        mnemonic: Zeroizing::new(b"unused".to_vec()),
        mnemonic_passphrase: Zeroizing::new(Vec::new()),
        seed: Zeroizing::new(array(&case["seed"])),
        derivation: SeedDerivation::Bip39,
    };
    v.store_wallet_secret(&WALLET, &secret).unwrap();
    v
}

/// A redeemed grant of `purpose`, authorized without a prompt (the vault
/// is unencrypted).
fn redeemed(v: &Vault, purpose: GrantPurpose) -> GrantToken {
    let grant = v
        .authorize(purpose, Some(&WALLET), Credential::None)
        .unwrap();
    v.redeem_grant(&grant.id, purpose.kind(), Some(&WALLET))
        .unwrap()
}

/// A redeemed `PlatformOp` grant that covers identity signatures.
fn platform_token(v: &Vault) -> GrantToken {
    redeemed(
        v,
        GrantPurpose::PlatformOp {
            max_duffs: 0,
            max_credits: 1,
        },
    )
}

/// An identity signer for the DIP-13 identities `identities`, under a
/// `PlatformOp` grant.
fn identity_signer(v: &Vault, identities: impl IntoIterator<Item = u32>) -> VaultIdentitySigner {
    let signer = v
        .platform_signer(&WALLET, &platform_token(v), SignerScope::PlatformIdentity)
        .unwrap();
    VaultIdentitySigner::new(signer, identities).unwrap()
}

fn identity_key(id: u32, key_type: KeyType, data: Vec<u8>) -> IdentityPublicKey {
    IdentityPublicKey::V0(IdentityPublicKeyV0 {
        id,
        purpose: Purpose::AUTHENTICATION,
        security_level: SecurityLevel::HIGH,
        contract_bounds: None,
        key_type,
        read_only: false,
        data: data.into(),
        disabled_at: None,
    })
}

#[tokio::test]
async fn contact_crypto_matches_the_library_vectors() {
    let doc = vectors();
    let k = &doc["constants"];
    let peer = PublicKey::from_secret_key(
        &Secp256k1::new(),
        &SecretKey::from_slice(&bytes(&k["peer_secret"])).unwrap(),
    );
    let contact_id: [u8; 32] = array(&k["contact_id"]);
    let private_data = bytes(&k["private_data"]);
    let iv: [u8; 16] = array(&k["private_data_iv"]);
    let mut checked = 0;

    for case in doc["cases"].as_array().unwrap() {
        let name = format!("{} {}", case["name"], case["network"]);
        let net = network(case);
        let dir = tempfile::tempdir().unwrap();
        let v = vault(case, &dir);
        let crypto = VaultContactCrypto::new(v.dashpay_crypto_signer(&WALLET).unwrap()).unwrap();

        // DIP-15 receiving xpubs, DIP-14 256-bit children.
        for r in case["receiving_xpubs"].as_array().unwrap() {
            let p = AccountType::DashpayReceivingFunds {
                index: num(&r["account"]),
                user_identity_id: array(&k["user_id"]),
                friend_identity_id: array(&k["friend_id"]),
            }
            .derivation_path(net)
            .unwrap();
            assert_eq!(p.to_string(), r["path"].as_str().unwrap(), "{name}");
            let x = crypto.receiving_xpub(&p).await.unwrap();
            let want = &r["xpub"];
            assert_eq!(
                u64::from(x.depth),
                want["depth"].as_u64().unwrap(),
                "{name}"
            );
            assert_eq!(
                x.parent_fingerprint.to_bytes().to_vec(),
                bytes(&want["parent_fingerprint"])
            );
            assert_eq!(x.chain_code.to_bytes().to_vec(), bytes(&want["chain_code"]));
            assert_eq!(
                x.public_key.serialize().to_vec(),
                bytes(&want["public_key"])
            );
            // DIP-15 compact xpub: fingerprint ‖ chain code ‖ public key.
            let compact = [
                &x.parent_fingerprint.to_bytes()[..],
                &x.chain_code.to_bytes()[..],
                &x.public_key.serialize()[..],
            ]
            .concat();
            assert_eq!(compact, bytes(&want["compact"]), "{name}");
            checked += 1;
        }

        // Seed binding: the BIP44 account-0 xpub.
        let bip44 = AccountType::Standard {
            index: 0,
            standard_account_type: StandardAccountType::BIP44Account,
        }
        .derivation_path(net)
        .unwrap();
        assert_eq!(bip44, path(&case["bip44_account0"]["path"]));
        let x = crypto.receiving_xpub(&bip44).await.unwrap();
        assert_eq!(
            x.to_string(),
            case["bip44_account0"]["xpub"].as_str().unwrap(),
            "{name}"
        );

        // Auto-accept: public key and the exported key.
        let aa = &case["auto_accept"];
        let aa_path =
            platform_wallet::wallet::identity::crypto::auto_accept::auto_accept_derivation_path(
                net,
                num(&aa["expiry"]),
            )
            .unwrap();
        assert_eq!(aa_path, path(&aa["path"]));
        let x = crypto.receiving_xpub(&aa_path).await.unwrap();
        assert_eq!(x.public_key.serialize().to_vec(), bytes(&aa["public_key"]));
        let exported = crypto
            .export_auto_accept_private_key(&aa_path)
            .await
            .unwrap();
        assert_eq!(
            exported.secret_bytes().to_vec(),
            bytes(&aa["secret"]),
            "{name}"
        );

        // ECDH with identity keys.
        for e in case["ecdh"].as_array().unwrap() {
            let shared = crypto
                .ecdh_shared_secret(&path(&e["path"]), &peer)
                .await
                .unwrap();
            assert_eq!(shared.to_vec(), bytes(&e["shared"]), "{name} {}", e["path"]);
            checked += 1;
        }

        // Account reference and unmask.
        let compact = bytes(&case["receiving_xpubs"][0]["xpub"]["compact"]);
        for a in case["account_references"].as_array().unwrap() {
            let p = path(&a["path"]);
            let reference = crypto
                .account_reference(&p, &compact, num(&a["account_index"]), num(&a["version"]))
                .await
                .unwrap();
            assert_eq!(reference, num(&a["reference"]), "{name} {}", a["path"]);
            let unmasked = crypto
                .unmask_account_reference(&p, &compact, reference)
                .await
                .unwrap();
            assert_eq!(unmasked, (num(&a["version"]), num(&a["account_index"])));
            checked += 1;
        }

        // contactInfo seal (byte-identical) and open (of the vector).
        for c in case["contact_info"].as_array().unwrap() {
            let root = path(&c["root_path"]);
            let idx = num(&c["derivation_index"]);
            let sealed = crypto
                .contact_info_seal(&root, idx, &contact_id, &private_data, &iv)
                .await
                .unwrap();
            assert_eq!(
                sealed.enc_to_user_id.to_vec(),
                bytes(&c["enc_to_user_id"]),
                "{name}"
            );
            assert_eq!(sealed.private_data, bytes(&c["private_data"]), "{name}");
            let opened = crypto
                .contact_info_open(
                    &root,
                    idx,
                    &array(&c["enc_to_user_id"]),
                    &bytes(&c["private_data"]),
                )
                .await
                .unwrap();
            assert_eq!(opened.contact_id, contact_id);
            assert_eq!(opened.private_data, private_data);
            checked += 1;
        }
    }
    assert_eq!(checked, 4 * (2 + 3 + 3 + 3));
}

#[tokio::test]
async fn identity_signatures_match_and_verify_for_keys_0_to_5() {
    let doc = vectors();
    let data = bytes(&doc["constants"]["identity_data"]);
    let mut checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        let name = format!("{} {}", case["name"], case["network"]);
        let dir = tempfile::tempdir().unwrap();
        let v = vault(case, &dir);
        let signer = identity_signer(&v, [0, 1]);
        for key in case["identity_keys"].as_array().unwrap() {
            let id = num(&key["key_index"]);
            let public = bytes(&key["public_key"]);
            let hash = bytes(&key["hash160"]);
            for (key_type, key_data) in [
                (KeyType::ECDSA_SECP256K1, public.clone()),
                (KeyType::ECDSA_HASH160, hash.clone()),
            ] {
                let k = identity_key(id, key_type, key_data);
                assert!(signer.can_sign_with(&k), "{name} {}", key["path"]);
                let sig = signer.sign(&k, &data).await.unwrap().to_vec();
                // RFC 6979: the same bytes `dashcore::signer::sign` made.
                assert_eq!(sig, bytes(&key["signature"]), "{name} {}", key["path"]);
                dashcore::signer::verify_data_signature(&data, &sig, &public).unwrap();
                let digest = dashcore::signer::double_sha(&data);
                dashcore::signer::verify_hash_signature(&digest, &sig, &hash).unwrap();
            }
            checked += 1;
        }

        // A key that is no slot of identities 0 or 1 is neither signable nor
        // signed: another identity's slot, a forged key, a non-ECDSA type.
        let first = &case["identity_keys"][0];
        let foreign = identity_key(6, KeyType::ECDSA_SECP256K1, bytes(&first["public_key"]));
        assert!(!signer.can_sign_with(&foreign));
        assert!(signer.sign(&foreign, &data).await.is_err());
        let narrow = identity_signer(&v, [0]);
        let of_identity_1 = case["identity_keys"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| num(&k["identity_index"]) == 1)
            .unwrap();
        let k = identity_key(
            num(&of_identity_1["key_index"]),
            KeyType::ECDSA_SECP256K1,
            bytes(&of_identity_1["public_key"]),
        );
        assert!(!narrow.can_sign_with(&k));
        assert!(narrow.sign(&k, &data).await.is_err());
        let bls = identity_key(0, KeyType::BLS12_381, vec![0; 48]);
        assert!(!signer.can_sign_with(&bls));
        assert!(signer.sign(&bls, &data).await.is_err());
        let ecdsa = identity_key(0, KeyType::ECDSA_SECP256K1, bytes(&first["public_key"]));
        assert!(signer.sign_create_witness(&ecdsa, &data).await.is_err());
    }
    assert_eq!(checked, 4 * 12);
}

#[tokio::test]
async fn scan_key_resolves_the_master_the_identity_keys_derive_from() {
    let doc = vectors();
    for case in doc["cases"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let v = vault(case, &dir);
        let scan = VaultScanKey::new(
            v.scan_key(&WALLET, &redeemed(&v, GrantPurpose::IdentityScan))
                .unwrap(),
        );
        let resolve = scan.resolver();
        let master = resolve().unwrap();
        let secp = Secp256k1::new();
        for key in case["identity_keys"].as_array().unwrap() {
            let derived = master.derive_priv(&secp, &path(&key["path"])).unwrap();
            let public = PublicKey::from_secret_key(&secp, &derived.private_key);
            assert_eq!(public.serialize().to_vec(), bytes(&key["public_key"]));
        }
        // Locked: retryable, not terminal.
        v.lock();
        assert!(matches!(
            scan.resolve(),
            Err(platform_wallet::manager::startup::ScanKeyError::Unavailable(_))
        ));
    }
}

#[tokio::test]
async fn adapters_refuse_signers_of_another_scope() {
    let doc = vectors();
    let case = &doc["cases"][0];
    let dir = tempfile::tempdir().unwrap();
    let v = vault(case, &dir);
    let crypto = v.dashpay_crypto_signer(&WALLET).unwrap();
    assert!(VaultIdentitySigner::new(crypto.clone(), [0]).is_err());
    let mixing = v.mixing_signer(&WALLET).unwrap();
    assert!(VaultContactCrypto::new(mixing).is_err());
    // The DashPay adapter never signs: a signing path is refused outright.
    let c = VaultContactCrypto::new(crypto).unwrap();
    let auth = path(&case["identity_keys"][0]["path"]);
    assert!(c.receiving_xpub(&auth).await.is_err());
    assert!(c.export_auto_accept_private_key(&auth).await.is_err());
    assert!(
        c.export_invitation_private_key(&path(&case["auto_accept"]["path"]))
            .await
            .is_err()
    );
}
