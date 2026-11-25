use std::str::FromStr;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use dashcore::secp256k1::ecdh::shared_secret_point;
use dashcore::secp256k1::{PublicKey, SecretKey};
use hkdf::Hkdf;
use key_wallet::Network;
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use key_wallet::dip9::ApplicationKeyPurpose;
use key_wallet::mnemonic::Mnemonic;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::*;
use crate::{
    Credential, GrantPurpose, KdfParams, KdfPolicy, MemoryOsStore, SeedDerivation, SignerError,
    SignerScope, SystemClock, Vault, VaultConfig, VaultError, VaultSigner, WalletSecret,
};

const W: [u8; 32] = [1; 32];
const PASS: &[u8] = b"pw";
const SEED: [u8; 64] = [0x5a; 64];
const OWNER: [u8; 32] = [0x35; 32];
const APP: [u8; 32] = [0x6b; 32];

fn hex32(s: &str) -> [u8; 32] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn hex33(s: &str) -> [u8; 33] {
    hex::decode(s).unwrap().try_into().unwrap()
}

fn app_eph_secret(byte: u8) -> SecretKey {
    SecretKey::from_secret_bytes([byte; 32]).unwrap()
}

fn app_eph_pub(byte: u8) -> [u8; 33] {
    PublicKey::from_secret_key(&app_eph_secret(byte)).serialize()
}

fn vault(dir: &tempfile::TempDir) -> Vault {
    let config = VaultConfig {
        kdf: KdfPolicy::Fixed(KdfParams::TEST),
        os_store: Arc::new(MemoryOsStore::new()),
        clock: Arc::new(SystemClock),
        grant_ttl_secs: 120,
    };
    let v = Vault::open(
        dir.path().join("vault"),
        Network::Regtest,
        "regtest",
        config,
    )
    .unwrap();
    v.create(Some(PASS)).unwrap();
    let secret = WalletSecret {
        mnemonic: Zeroizing::new(b"unused".to_vec()),
        mnemonic_passphrase: Zeroizing::new(Vec::new()),
        seed: Zeroizing::new(SEED),
        derivation: SeedDerivation::Bip39,
    };
    v.store_wallet_secret(&W, &secret).unwrap();
    v
}

fn connect_purpose(request: [u8; 20]) -> GrantPurpose {
    GrantPurpose::AppConnect {
        max_credits: 1_000_000,
        request,
        plan: [7; 32],
    }
}

fn signer_for(
    v: &Vault,
    purpose: GrantPurpose,
    scope: SignerScope,
) -> Result<VaultSigner, VaultError> {
    let grant = v.authorize(purpose, Some(&W), Credential::Passphrase(PASS))?;
    let token = v.redeem_grant(&grant.id, purpose.kind(), Some(&W))?;
    v.platform_signer(&W, &token, scope)
}

fn connect_signer(v: &Vault, app: &[u8; 33]) -> VaultSigner {
    let request = request_hash(app);
    signer_for(
        v,
        connect_purpose(request),
        SignerScope::AppConnect { request },
    )
    .unwrap()
}

fn derived_secret(path: &DerivationPath) -> [u8; 32] {
    let master = ExtendedPrivKey::new_master(Network::Regtest, &SEED).unwrap();
    master
        .derive_priv(path)
        .unwrap()
        .private_key
        .to_secret_bytes()
}

fn open_payload(app_secret: &SecretKey, sealed: &AppConnectSealed, owner: &[u8; 32]) -> Vec<u8> {
    let wallet = PublicKey::from_slice(&sealed.wallet_ephemeral_pub_key).unwrap();
    let point = shared_secret_point(&wallet, app_secret);
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"dash:key-exchange:v1"), &point[..32])
        .expand(b"app-connect:v2", &mut key)
        .unwrap();
    let app_pub = PublicKey::from_secret_key(app_secret).serialize();
    let (nonce, ciphertext) = sealed.payload.split_at(12);
    Aes256Gcm::new_from_slice(&key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &aad(&app_pub, owner),
            },
        )
        .unwrap()
}

#[test]
fn the_seal_matches_the_published_vector() {
    let app = hex33("03e83cb927e85f96ae50a95ca3be21cc7a3755139a941b15d99b01af4fecf0d270");
    let owner = hex32("9296d642338cb3bfceeb554285462719e15f004d0579ce53ea77fb6d86cd9ab9");
    let ephemeral = SecretKey::from_secret_bytes(hex32(
        "d1772740bcf476137df337bbe42122e82da08e11b632d6c3ff0d7281576e4127",
    ))
    .unwrap();
    let nonce: [u8; 12] = hex::decode("22bb26ec9e0d316c4fb36be2")
        .unwrap()
        .try_into()
        .unwrap();
    let plaintext = hex::decode(concat!(
        "7df2f40f15e6079e514d7e5cb9cb1861b89986906ed820e158c3144272df9586",
        "3a992d9275d082af7540d1333a134ff0572c2bb79567437daa88dee55af1571f",
        "b37bb0ae8d6bbf01b38e843470570d5158d1939762a88673dd6cdf71bc144753",
    ))
    .unwrap();
    let sealed = seal_with(&ephemeral, nonce, &app, &owner, &plaintext).unwrap();
    assert_eq!(
        hex::encode(sealed.wallet_ephemeral_pub_key),
        "02ee49ae562aa74ea28f95094da6800db9244b1addc1499439ffc7382cd7862d5a"
    );
    assert_eq!(
        hex::encode(&sealed.payload),
        concat!(
            "22bb26ec9e0d316c4fb36be20e3f2a358cce3609d09094351ce5e78f08e5253b",
            "0f9258616374753411c97faecb1fbde336c01d7f08380095a0a7ffe3fe207f80",
            "e92a7adb7bd8731dffe3b96a6b2c363aa1302c4e82d60f75419aa1f6f3ac5522",
            "453f2b761495fb3b7816ea8bfe66a210e14fdad8b7f7c15ffb3ef39b",
        )
    );
    assert_eq!(sealed.payload.len(), 28 + 32 * 3);
    assert_eq!(
        hex::encode(request_hash(&app)),
        "d77dc0da9df83a47755a15c42c5d9db9d7a7fd62"
    );
}

#[test]
fn session_leaves_match_the_dip_vectors() {
    let app = hex33("03e83cb927e85f96ae50a95ca3be21cc7a3755139a941b15d99b01af4fecf0d270");
    assert_eq!(
        hex::encode(session_leaf(&app, 0)),
        "a0ad9d4647bd9296611258b8f47db92b06bdaee8782fd28e51632403d8a66a1e"
    );
    assert_eq!(
        hex::encode(session_leaf(&app, 1)),
        "27a2c59bcf28894aee77c809c3c0e94201db08b66648c16fdfa689e6926b4f52"
    );
}

#[test]
fn encryption_paths_match_the_pinned_vectors() {
    let seed = Mnemonic::from_phrase(
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    )
    .unwrap()
    .to_seed("");
    let public = |rotation| {
        let key = AppKey::Encryption {
            contract: APP,
            purpose: ApplicationKeyPurpose::Encryption,
            rotation,
        };
        let path = key_path(Network::Mainnet, OWNER, &app_eph_pub(1), &key);
        let master = ExtendedPrivKey::new_master(Network::Mainnet, &seed).unwrap();
        let x = master.derive_priv(&path).unwrap();
        hex::encode(PublicKey::from_secret_key(&x.private_key).serialize())
    };
    assert_eq!(
        public(Some(0)),
        "031a882c119e2b1f8e0349b444d877afa2599efeef49a26930fd25518cb6e9d57a"
    );
    assert_eq!(
        public(Some(1)),
        "03d0f6f0f48d968b9554d8e7fe467d7135bacf29d5c4994ca593bd10ebeed696f6"
    );
    assert_eq!(
        public(None),
        "03e989de1b62f137231cc06659c810c17100becd1f5e23d5710faa7602769fe434"
    );
}

#[test]
fn a_session_path_is_the_dip13_application_path() {
    let app = app_eph_pub(1);
    let path = key_path(
        Network::Testnet,
        OWNER,
        &app,
        &AppKey::Session { binding_index: 2 },
    );
    let expected = format!(
        "m/9'/1'/5'/6'/0'/0x{}'/0x{}'",
        hex::encode(OWNER),
        hex::encode(session_leaf(&app, 2))
    );
    assert_eq!(path, DerivationPath::from_str(&expected).unwrap());
    assert!(crate::paths::app_session_key(&path, Network::Testnet).is_some());
    assert!(crate::paths::app_session_key(&path, Network::Mainnet).is_none());
}

#[test]
fn a_legacy_encryption_path_is_refused_on_mainnet() {
    let key = |rotation| AppKey::Encryption {
        contract: APP,
        purpose: ApplicationKeyPurpose::Encryption,
        rotation,
    };
    let legacy = key_path(Network::Mainnet, OWNER, &app_eph_pub(1), &key(None));
    assert!(crate::paths::app_encryption_key(&legacy, Network::Mainnet).is_none());
    let current = key_path(Network::Mainnet, OWNER, &app_eph_pub(1), &key(Some(0)));
    assert_eq!(
        crate::paths::app_encryption_key(&current, Network::Mainnet),
        Some((OWNER, APP))
    );
    let legacy = key_path(Network::Testnet, OWNER, &app_eph_pub(1), &key(None));
    assert!(crate::paths::app_encryption_key(&legacy, Network::Testnet).is_some());
}

#[test]
fn the_seal_carries_the_keys_the_app_registers() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let app = app_eph_pub(9);
    let signer = connect_signer(&v, &app);
    let keys = [
        AppKey::Session { binding_index: 0 },
        AppKey::Session { binding_index: 1 },
        AppKey::Encryption {
            contract: APP,
            purpose: ApplicationKeyPurpose::Encryption,
            rotation: Some(0),
        },
    ];
    let sealed = signer.app_connect_seal(OWNER, &app, &keys).unwrap();
    assert_eq!(sealed.payload.len(), 28 + 32 * keys.len());
    let plaintext = open_payload(&app_eph_secret(9), &sealed, &OWNER);
    for (slot, key) in plaintext.chunks(32).zip(&keys) {
        let path = key_path(Network::Regtest, OWNER, &app, key);
        assert_eq!(slot, derived_secret(&path));
        let secret = SecretKey::from_secret_bytes(slot.try_into().unwrap()).unwrap();
        assert_eq!(
            signer.app_public_key(OWNER, &app, key).unwrap(),
            PublicKey::from_secret_key(&secret).serialize()
        );
    }
    let again = signer.app_connect_seal(OWNER, &app, &keys).unwrap();
    assert_ne!(again.payload, sealed.payload);
    assert_ne!(
        again.wallet_ephemeral_pub_key,
        sealed.wallet_ephemeral_pub_key
    );
}

#[test]
fn the_seal_is_bound_to_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let app = app_eph_pub(9);
    let signer = connect_signer(&v, &app);
    let sealed = signer
        .app_connect_seal(OWNER, &app, &[AppKey::Session { binding_index: 0 }])
        .unwrap();
    let wallet = PublicKey::from_slice(&sealed.wallet_ephemeral_pub_key).unwrap();
    let point = shared_secret_point(&wallet, &app_eph_secret(9));
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"dash:key-exchange:v1"), &point[..32])
        .expand(b"app-connect:v2", &mut key)
        .unwrap();
    let (nonce, ciphertext) = sealed.payload.split_at(12);
    let other = Aes256Gcm::new_from_slice(&key).unwrap().decrypt(
        Nonce::from_slice(nonce),
        Payload {
            msg: ciphertext,
            aad: &aad(&app, &[0x36; 32]),
        },
    );
    assert!(other.is_err());
}

#[test]
fn a_signer_reaches_only_its_own_request() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let signer = connect_signer(&v, &app_eph_pub(9));
    let other = app_eph_pub(10);
    let session = AppKey::Session { binding_index: 0 };
    assert!(matches!(
        signer.app_public_key(OWNER, &other, &session),
        Err(SignerError::PathNotAllowed(_))
    ));
    assert!(matches!(
        signer.app_connect_seal(OWNER, &other, &[session]),
        Err(SignerError::PathNotAllowed(_))
    ));
}

#[test]
fn a_session_key_never_signs() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let app = app_eph_pub(9);
    let signer = connect_signer(&v, &app);
    let session = AppKey::Session { binding_index: 0 };
    let public = signer.app_public_key(OWNER, &app, &session).unwrap();
    assert!(matches!(
        signer.sign_app_key(OWNER, &app, &session, &public, b"data"),
        Err(SignerError::PathNotAllowed(_))
    ));
    let encryption = AppKey::Encryption {
        contract: APP,
        purpose: ApplicationKeyPurpose::Encryption,
        rotation: Some(0),
    };
    let public = signer.app_public_key(OWNER, &app, &encryption).unwrap();
    assert!(
        signer
            .sign_app_key(OWNER, &app, &encryption, &public, b"data")
            .is_ok()
    );
    assert!(matches!(
        signer.sign_app_key(OWNER, &app, &encryption, &[2; 33], b"data"),
        Err(SignerError::KeyMismatch(_))
    ));
}

#[test]
fn an_app_connect_signer_reaches_no_identity_or_wallet_key() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let signer = connect_signer(&v, &app_eph_pub(9));
    let identity_key = DerivationPath::from_str("m/9'/1'/5'/0'/0'/0'/0'").unwrap();
    assert!(
        signer
            .sign_identity(&identity_key, &[2; 33], b"data")
            .is_err()
    );
    let address = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
    assert!(signer.check(crate::signer::KeyUse::Sign, &address).is_err());
    assert!(
        signer
            .check(crate::signer::KeyUse::Export, &identity_key)
            .is_err()
    );
}

#[test]
fn only_an_app_connect_grant_issues_the_scope() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    let request = request_hash(&app_eph_pub(9));
    let platform_op = GrantPurpose::PlatformOp {
        max_duffs: 1,
        max_credits: 1,
    };
    assert!(matches!(
        signer_for(&v, platform_op, SignerScope::AppConnect { request }),
        Err(VaultError::GrantPurposeMismatch)
    ));
    assert!(matches!(
        signer_for(
            &v,
            connect_purpose(request),
            SignerScope::AppConnect { request: [0; 20] }
        ),
        Err(VaultError::GrantPurposeMismatch)
    ));
    assert!(signer_for(&v, connect_purpose(request), SignerScope::PlatformIdentity).is_ok());
    for scope in [
        SignerScope::DashPayCrypto,
        SignerScope::PlatformAddress,
        SignerScope::PlatformFunding { max_duffs: 1 },
    ] {
        assert!(
            matches!(
                signer_for(&v, connect_purpose(request), scope),
                Err(VaultError::GrantPurposeMismatch)
            ),
            "{scope:?}"
        );
    }
}

#[test]
fn an_app_connect_grant_needs_a_credential_while_unlocked() {
    let dir = tempfile::tempdir().unwrap();
    let v = vault(&dir);
    v.unlock(PASS, crate::UnlockScope::Full).unwrap();
    let purpose = connect_purpose(request_hash(&app_eph_pub(9)));
    assert!(v.authorize(purpose, Some(&W), Credential::None).is_err());
    assert!(
        v.authorize(purpose, Some(&W), Credential::Passphrase(PASS))
            .is_ok()
    );
}

#[test]
fn a_sealed_response_shows_no_bytes_in_debug() {
    let sealed = AppConnectSealed {
        wallet_ephemeral_pub_key: [2; 33],
        payload: vec![0xab; 60],
    };
    let shown = format!("{sealed:?}");
    assert!(!shown.contains("abab"), "{shown}");
}
