//! Platform key operations of a [`VaultSigner`] (DASHPAY §3.1 `signers.rs`):
//! identity-key signatures and the DIP-15 contact crypto. The engine's
//! adapters implement dpp's `Signer<IdentityPublicKey>` and platform-wallet's
//! `ContactCryptoProvider` over these, so derived scalars stay in this crate;
//! only signatures, public keys, ECDH and HMAC products and ciphertexts
//! leave it, plus the one key DIP-15 hands out on purpose (auto-accept).
//!
//! Each operation checks the shape of its path whatever the signer's scope,
//! then the scope's [`KeyUse`] rule. The counterparts are
//! `rs-sdk-ffi/src/mnemonic_resolver_core_signer.rs` (resolver signer) and
//! `rs-platform-wallet-ffi/src/sign_with_mnemonic_resolver.rs` (identity
//! signing) at platform `bc321362b9`; the library's test
//! `SeedCryptoProvider` (`contact_requests.rs:171-345`) is the reference
//! for the outputs.

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{Message, PublicKey};
use dashcore::signer::{CompactSignature, double_sha};
use key_wallet::bip32::{ChildNumber, DerivationPath, ExtendedPrivKey};
use key_wallet::dip9::{
    DASHPAY_CONTACT_INFO_ENC_TO_USER_ID_CHILD, DASHPAY_CONTACT_INFO_PRIVATE_DATA_CHILD,
};
use zeroize::Zeroizing;

use crate::SignerError;
use crate::crypto::Key32;
use crate::signer::{KeyUse, SignerScope, VaultSigner, WalletSigner};
use crate::types::WalletId;
use crate::vault::Vault;
use crate::{dip15, paths};

/// The two DIP-15 contactInfo ciphertexts to publish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInfoSealed {
    /// `encToUserId`: AES-256-ECB of the 32-byte contact id.
    pub enc_to_user_id: [u8; 32],
    /// `privateData`: `iv ‖ AES-256-CBC`.
    pub private_data: Vec<u8>,
}

/// A contactInfo document opened with the wallet's keys. `private_data` is
/// the decrypted alias and note: erased on drop, and never shown by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct ContactInfoOpened {
    pub contact_id: [u8; 32],
    pub private_data: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for ContactInfoOpened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContactInfoOpened")
            .field("contact_id", &hex::encode(self.contact_id))
            .field("private_data_len", &self.private_data.len())
            .finish()
    }
}

/// Whether the compressed public key `derived` is the on-chain identity key
/// `key_data`: the key itself (33 bytes, `ECDSA_SECP256K1`) or its HASH160
/// (20 bytes, `ECDSA_HASH160`); any other length never matches.
/// platform-wallet `pubkey_binds_expected_key_data`.
fn key_data_matches(derived: &[u8; 33], key_data: &[u8]) -> bool {
    match key_data.len() {
        33 => derived[..] == *key_data,
        20 => hash160::Hash::hash(derived).as_byte_array()[..] == *key_data,
        _ => false,
    }
}

fn require(shape: bool, path: &DerivationPath) -> Result<(), SignerError> {
    if shape {
        Ok(())
    } else {
        Err(SignerError::PathNotAllowed(path.to_string()))
    }
}

fn secret_bytes(x: &ExtendedPrivKey) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(x.private_key.secret_bytes())
}

impl VaultSigner {
    fn require_identity_key(&self, path: &DerivationPath) -> Result<(), SignerError> {
        require(
            paths::identity_auth_key(path, self.network()).is_some(),
            path,
        )
    }

    /// Whether the identity key at `path` is the on-chain key `key_data`
    /// (33-byte public key or its 20-byte HASH160).
    pub fn identity_key_matches(
        &self,
        path: &DerivationPath,
        key_data: &[u8],
    ) -> Result<bool, SignerError> {
        self.require_identity_key(path)?;
        self.with_key(path, KeyUse::PublicKey, |secp, x| {
            key_data_matches(
                &PublicKey::from_secret_key(secp, &x.private_key).serialize(),
                key_data,
            )
        })
    }

    /// Signs `data` with the identity key at `path`, once that key is shown
    /// to be `key_data`: the 65-byte compact recoverable signature over
    /// double-SHA256 that `dashcore::signer::sign` makes (and
    /// `dash_sdk_sign_with_mnemonic_resolver_and_path`, which binds the key
    /// the same way before signing). A key that does not match is
    /// [`SignerError::KeyMismatch`] and nothing is signed. `data` is hashed
    /// before the key is derived, so a large input does not hold up a lock.
    pub fn sign_identity(
        &self,
        path: &DerivationPath,
        key_data: &[u8],
        data: &[u8],
    ) -> Result<[u8; 65], SignerError> {
        self.require_identity_key(path)?;
        let digest: [u8; 32] = double_sha(data)
            .try_into()
            .map_err(|_| SignerError::Derivation("digest length".into()))?;
        self.with_key(path, KeyUse::Sign, |secp, x| {
            let public = PublicKey::from_secret_key(secp, &x.private_key).serialize();
            if !key_data_matches(&public, key_data) {
                return Err(SignerError::KeyMismatch(path.to_string()));
            }
            let sig = secp.sign_ecdsa_recoverable(&Message::from_digest(digest), &x.private_key);
            Ok(sig.to_compact_signature(true))
        })?
    }

    /// DIP-15 ECDH between the identity key at `path` (an encryption or
    /// decryption key) and `peer`: `SHA256((y&1|2) ‖ x)` of the shared
    /// point (as `platform_encryption::derive_shared_key_ecdh`).
    pub fn ecdh_shared_secret(
        &self,
        path: &DerivationPath,
        peer: &PublicKey,
    ) -> Result<Zeroizing<[u8; 32]>, SignerError> {
        self.require_identity_key(path)?;
        self.with_key(path, KeyUse::Agreement, |_, x| {
            dip15::ecdh(&x.private_key, peer)
        })
    }

    /// DIP-15 `accountReference`: the identity key at `path` keys the
    /// HMAC mask over `compact_xpub`.
    pub fn account_reference(
        &self,
        path: &DerivationPath,
        compact_xpub: &[u8],
        account_index: u32,
        version: u32,
    ) -> Result<u32, SignerError> {
        self.require_identity_key(path)?;
        self.with_key(path, KeyUse::Agreement, |_, x| {
            dip15::account_reference(&secret_bytes(x), compact_xpub, account_index, version)
        })
    }

    /// Inverse of [`Self::account_reference`]: `(version, account_index)`.
    pub fn unmask_account_reference(
        &self,
        path: &DerivationPath,
        compact_xpub: &[u8],
        account_reference: u32,
    ) -> Result<(u32, u32), SignerError> {
        self.require_identity_key(path)?;
        self.with_key(path, KeyUse::Agreement, |_, x| {
            dip15::unmask_account_reference(account_reference, &secret_bytes(x), compact_xpub)
        })
    }

    /// Runs `f` on the `(encToUserId, privateData)` contactInfo AES keys
    /// `root/65536'/derivation_index'` and `root/65537'/derivation_index'`
    /// (`root` an identity key), in one operation, so whatever `f` makes
    /// with them exists before any later lock returns.
    fn with_contact_info_keys<T>(
        &self,
        root: &DerivationPath,
        derivation_index: u32,
        f: impl FnOnce(&Key32, &Key32) -> T,
    ) -> Result<T, SignerError> {
        self.require_identity_key(root)?;
        let step = |i| {
            ChildNumber::from_hardened_idx(i).map_err(|e| SignerError::Derivation(e.to_string()))
        };
        let index = step(derivation_index)?;
        let enc_path = root.extend([step(DASHPAY_CONTACT_INFO_ENC_TO_USER_ID_CHILD)?, index]);
        let data_path = root.extend([step(DASHPAY_CONTACT_INFO_PRIVATE_DATA_CHILD)?, index]);
        let op = self.op(&[
            (KeyUse::ContactInfo, &enc_path),
            (KeyUse::ContactInfo, &data_path),
        ])?;
        let enc_key = op.with_key(&enc_path, KeyUse::ContactInfo, |_, x| secret_bytes(x))?;
        let data_key = op.with_key(&data_path, KeyUse::ContactInfo, |_, x| secret_bytes(x))?;
        Ok(f(&enc_key, &data_key))
    }

    /// DIP-15 contactInfo seal: `encToUserId` (AES-256-ECB) and
    /// `privateData` (AES-256-CBC with `iv`) under the two keys at
    /// `root/65536'/derivation_index'` and `root/65537'/derivation_index'`.
    pub fn contact_info_seal(
        &self,
        root: &DerivationPath,
        derivation_index: u32,
        contact_id: &[u8; 32],
        private_data: &[u8],
        iv: &[u8; 16],
    ) -> Result<ContactInfoSealed, SignerError> {
        self.with_contact_info_keys(root, derivation_index, |enc_key, data_key| {
            ContactInfoSealed {
                enc_to_user_id: dip15::enc_to_user_id(enc_key, contact_id, false),
                private_data: dip15::encrypt_private_data(data_key, iv, private_data),
            }
        })
    }

    /// Inverse of [`Self::contact_info_seal`].
    pub fn contact_info_open(
        &self,
        root: &DerivationPath,
        derivation_index: u32,
        enc_to_user_id: &[u8; 32],
        private_data: &[u8],
    ) -> Result<ContactInfoOpened, SignerError> {
        self.with_contact_info_keys(root, derivation_index, |enc_key, data_key| {
            Ok(ContactInfoOpened {
                contact_id: dip15::enc_to_user_id(enc_key, enc_to_user_id, true),
                private_data: dip15::decrypt_private_data(data_key, private_data)?,
            })
        })?
    }

    /// The DIP-15 auto-accept private key at `m/9'/coin'/16'/expiry'`: the
    /// one key that leaves the vault on purpose (the `dapk` QR is a bearer
    /// credential for contact auto-acceptance only). Any other path is
    /// refused whatever the scope, as the resolver signer's
    /// `export_auto_accept_private_key` does (stricter: the coin type must
    /// be the vault's).
    pub fn export_auto_accept_key(
        &self,
        path: &DerivationPath,
    ) -> Result<Zeroizing<[u8; 32]>, SignerError> {
        require(paths::is_auto_accept_key(path, self.network()), path)?;
        self.with_key(path, KeyUse::Export, |_, x| secret_bytes(x))
    }
}

/// The master key of one wallet for an identity scan (platform-wallet's
/// `ScanKeyResolver`, `manager/startup.rs:104`). Holds no secret: the key is
/// derived when [`Self::master_key`] is called and stops being available
/// once the vault locks or changes unlock scope. Issued by
/// [`Vault::scan_key`] only while the vault's full key is available without
/// a prompt.
#[derive(Clone)]
pub struct ScanKey {
    signer: VaultSigner,
}

impl std::fmt::Debug for ScanKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanKey")
            .field("wallet_id", &hex::encode(self.signer.wallet_id()))
            .finish_non_exhaustive()
    }
}

impl ScanKey {
    pub(crate) fn new(signer: VaultSigner) -> Self {
        debug_assert_eq!(signer.scope(), SignerScope::Full);
        Self { signer }
    }

    pub fn wallet_id(&self) -> WalletId {
        self.signer.wallet_id()
    }

    /// The wallet's master extended private key. It erases itself when
    /// dropped (key-wallet `ExtendedPrivKey: Drop`).
    pub fn master_key(&self) -> Result<ExtendedPrivKey, SignerError> {
        self.signer
            .with_key(&DerivationPath::master(), KeyUse::Export, |_, x| x.clone())
    }
}

impl Vault {
    /// The [`ScanKey`] of `wallet`; see [`Vault::dashpay_crypto_signer`] for
    /// the states that issue it. It yields the master key with no grant,
    /// because the unattended bring-up needs it (DASHPAY §3.2): only the
    /// engine's bring-up may call it, and dw-ffi must never expose it.
    pub fn scan_key(&self, wallet: &WalletId) -> Result<ScanKey, crate::VaultError> {
        Ok(ScanKey::new(
            self.prompt_free_signer(wallet, SignerScope::Full)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_data_binding() {
        let secp = dashcore::secp256k1::Secp256k1::new();
        let sk = dashcore::secp256k1::SecretKey::from_slice(&[7; 32]).unwrap();
        let pk = PublicKey::from_secret_key(&secp, &sk).serialize();
        assert!(key_data_matches(&pk, &pk));
        let h = hash160::Hash::hash(&pk);
        assert!(key_data_matches(&pk, h.as_byte_array()));
        let mut other = pk;
        other[5] ^= 1;
        assert!(!key_data_matches(&pk, &other));
        assert!(!key_data_matches(&pk, &h.as_byte_array()[..19]));
        assert!(!key_data_matches(&pk, &[]));
        assert!(!key_data_matches(&pk, &[0; 32]));
    }
}
