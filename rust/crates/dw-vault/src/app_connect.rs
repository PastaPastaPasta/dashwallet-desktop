//! App Connect v2 (DashPay Connect) login keys: the DIP-13 application keys
//! of one request and their seal to the app (dips#181, dips#191).
//!
//! Only an [`SignerScope::AppConnect`] signer reaches them, only through the
//! methods here, and only for the request its grant names: every key's path
//! is built here, once the request's ephemeral key hashes to the scope's
//! `request`; a session key's path is derived from that key. The
//! private keys leave the vault once, inside the AES-256-GCM payload the app
//! decrypts with its ephemeral key; public keys and signatures are all the
//! caller sees otherwise.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use dashcore::secp256k1::ecdh::shared_secret_point;
use dashcore::secp256k1::ecdsa::RecoverableSignature;
use dashcore::secp256k1::{Message, PublicKey, SecretKey};
use dashcore::sign_message::MessageSignature;
use dashcore::signer::double_sha;
use hkdf::Hkdf;
use key_wallet::bip32::{ChildNumber, DerivationPath};
use key_wallet::dip9::ApplicationKeyPurpose;
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use crate::SignerError;
use crate::signer::{KeyUse, SignerScope, VaultSigner};

const HKDF_SALT: &[u8] = b"dash:key-exchange:v1";
const HKDF_INFO: &[u8] = b"app-connect:v2";
const AAD_TAG: &[u8] = b"dash:app-connect:v2";
const NONCE_LEN: usize = 12;
/// The most keys one response carries (`loginKeyResponse.encryptedPayload`
/// holds 1 to 17 slots).
pub const MAX_SEALED_KEYS: usize = 17;

/// An application key of one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKey {
    /// The session key of the request's auth binding at `binding_index`
    /// (its position among the request's bindings, declined ones counted).
    Session { binding_index: u8 },
    /// The shared encryption key for `contract`; `rotation: None` is the
    /// legacy test-network path without a rotation level.
    Encryption {
        contract: [u8; 32],
        purpose: ApplicationKeyPurpose,
        rotation: Option<u32>,
    },
}

/// The response the app reads: the wallet's ephemeral public key and
/// `nonce ‖ ciphertext ‖ tag`.
#[derive(Clone, PartialEq, Eq)]
pub struct AppConnectSealed {
    pub wallet_ephemeral_pub_key: [u8; 33],
    pub payload: Vec<u8>,
}

impl std::fmt::Debug for AppConnectSealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConnectSealed")
            .field("payload_len", &self.payload.len())
            .finish_non_exhaustive()
    }
}

pub use dw_uri::connect::{request_hash, session_leaf};

/// The DIP-13 path of `key`: `m/9'/coin'/5'/6'/0'/identity'/leaf'` for a
/// session key, `m/9'/coin'/5'/7'/0'/identity'/contract'/purpose'[/rotation']`
/// for an encryption key.
pub fn key_path(
    network: key_wallet::Network,
    identity: [u8; 32],
    app_eph_pub: &[u8; 33],
    key: &AppKey,
) -> DerivationPath {
    match *key {
        AppKey::Session { binding_index } => {
            DerivationPath::application_session_authentication_path(
                network,
                identity,
                session_leaf(app_eph_pub, binding_index),
            )
        }
        AppKey::Encryption {
            contract,
            purpose,
            rotation,
        } => {
            let path =
                DerivationPath::application_encryption_path(network, identity, contract, purpose);
            match rotation {
                Some(index) => path.extend([ChildNumber::Hardened { index }]),
                None => path,
            }
        }
    }
}

fn aad(app_eph_pub: &[u8; 33], owner: &[u8; 32]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(AAD_TAG.len() + 20 + 32);
    aad.extend_from_slice(AAD_TAG);
    aad.extend_from_slice(&request_hash(app_eph_pub));
    aad.extend_from_slice(owner);
    aad
}

/// Seals `plaintext` (32-byte slots) to `app_eph_pub` for `owner`, with
/// the wallet ephemeral key `ephemeral` and `nonce`.
pub(crate) fn seal_with(
    ephemeral: &SecretKey,
    nonce: [u8; NONCE_LEN],
    app_eph_pub: &[u8; 33],
    owner: &[u8; 32],
    plaintext: &[u8],
) -> Result<AppConnectSealed, SignerError> {
    let peer = PublicKey::from_slice(app_eph_pub)
        .map_err(|_| SignerError::Derivation("the app's ephemeral key is not a point".into()))?;
    let mut point = shared_secret_point(&peer, ephemeral);
    let mut shared = Zeroizing::new([0u8; 32]);
    shared.copy_from_slice(&point[..32]);
    point.zeroize();
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(HKDF_SALT), &shared[..])
        .expand(HKDF_INFO, &mut key[..])
        .map_err(|_| SignerError::Derivation("hkdf".into()))?;
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| SignerError::Derivation("aes key".into()))?;
    let aad = aad(app_eph_pub, owner);
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| SignerError::Derivation("seal".into()))?;
    let mut payload = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ciphertext);
    Ok(AppConnectSealed {
        wallet_ephemeral_pub_key: PublicKey::from_secret_key(ephemeral).serialize(),
        payload,
    })
}

fn random<const N: usize>() -> Result<Zeroizing<[u8; N]>, SignerError> {
    let mut out = Zeroizing::new([0u8; N]);
    getrandom::getrandom(&mut out[..])
        .map_err(|e| SignerError::Derivation(format!("OS random source: {e}")))?;
    Ok(out)
}

impl VaultSigner {
    fn app_request(&self) -> Result<[u8; 20], SignerError> {
        match self.scope() {
            SignerScope::AppConnect { request } => Ok(request),
            scope => Err(SignerError::PathNotAllowed(format!(
                "a {scope:?} signer has no App Connect keys"
            ))),
        }
    }

    /// The path of `key` for the identity `identity` and the request whose
    /// ephemeral key is `app_eph_pub`: refused unless this is that request's
    /// App Connect signer.
    pub fn app_key_path(
        &self,
        identity: [u8; 32],
        app_eph_pub: &[u8; 33],
        key: &AppKey,
    ) -> Result<DerivationPath, SignerError> {
        if self.app_request()? != request_hash(app_eph_pub) {
            return Err(SignerError::PathNotAllowed(
                "the request is not this grant's".into(),
            ));
        }
        Ok(key_path(self.network(), identity, app_eph_pub, key))
    }

    /// The compressed public key of `key`.
    pub fn app_public_key(
        &self,
        identity: [u8; 32],
        app_eph_pub: &[u8; 33],
        key: &AppKey,
    ) -> Result<[u8; 33], SignerError> {
        let path = self.app_key_path(identity, app_eph_pub, key)?;
        self.with_key(&path, KeyUse::AppConnectPublicKey, |x| {
            PublicKey::from_secret_key(&x.private_key).serialize()
        })
    }

    /// The identity-update signature of a new encryption key `key` (its
    /// proof of possession), once the key derived is `key_data`: the
    /// 65-byte compact recoverable signature over double-SHA256 of `data`.
    /// Session keys are registered as `ECDSA_HASH160` and never sign.
    pub fn sign_app_key(
        &self,
        identity: [u8; 32],
        app_eph_pub: &[u8; 33],
        key: &AppKey,
        key_data: &[u8],
        data: &[u8],
    ) -> Result<[u8; 65], SignerError> {
        let path = self.app_key_path(identity, app_eph_pub, key)?;
        let digest: [u8; 32] = double_sha(data)
            .try_into()
            .map_err(|_| SignerError::Derivation("digest length".into()))?;
        self.with_key(&path, KeyUse::AppConnectSign, |x| {
            let public = PublicKey::from_secret_key(&x.private_key).serialize();
            if public[..] != *key_data {
                return Err(SignerError::KeyMismatch(path.to_string()));
            }
            let sig = RecoverableSignature::sign_ecdsa_recoverable(
                Message::from_digest(digest),
                &x.private_key,
            );
            Ok(MessageSignature::new(sig, true).serialize())
        })?
    }

    /// Seals the private keys `keys`, in order, to the app for `owner`, the
    /// identity whose keys they are: `shared = ECDH(walletEph, appEphPub).x`,
    /// `HKDF-SHA256(shared, "dash:key-exchange:v1", "app-connect:v2")`,
    /// AES-256-GCM with a random nonce over the 32-byte slots, the AAD
    /// `"dash:app-connect:v2" ‖ hash160(appEphPub) ‖ owner`. Every key is
    /// derived in one operation; the slots are erased once sealed.
    pub fn app_connect_seal(
        &self,
        owner: [u8; 32],
        app_eph_pub: &[u8; 33],
        keys: &[AppKey],
    ) -> Result<AppConnectSealed, SignerError> {
        if keys.is_empty() || keys.len() > MAX_SEALED_KEYS {
            return Err(SignerError::Derivation(format!(
                "a response carries 1 to {MAX_SEALED_KEYS} keys, not {}",
                keys.len()
            )));
        }
        let paths = keys
            .iter()
            .map(|k| self.app_key_path(owner, app_eph_pub, k))
            .collect::<Result<Vec<_>, _>>()?;
        let uses: Vec<_> = paths.iter().map(|p| (KeyUse::AppConnectSeal, p)).collect();
        let plaintext = self.run(&uses, |op| {
            let mut slots = Zeroizing::new(Vec::with_capacity(32 * paths.len()));
            for path in &paths {
                let secret = op.with_key(path, KeyUse::AppConnectSeal, |x| {
                    Zeroizing::new(x.private_key.to_secret_bytes())
                })?;
                slots.extend_from_slice(&secret[..]);
            }
            Ok(slots)
        })?;
        let ephemeral = random::<32>()?;
        let ephemeral = SecretKey::from_secret_bytes(*ephemeral)
            .map_err(|_| SignerError::Derivation("ephemeral key".into()))?;
        let nonce = random::<NONCE_LEN>()?;
        let sealed = seal_with(&ephemeral, *nonce, app_eph_pub, &owner, &plaintext);
        let mut ephemeral = ephemeral;
        ephemeral.non_secure_erase();
        sealed
    }
}

#[cfg(test)]
#[path = "app_connect_tests.rs"]
mod tests;
