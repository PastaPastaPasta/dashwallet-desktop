//! The host signers platform-wallet needs (DASHPAY §3.1 `signers.rs`), each
//! over a scoped [`VaultSigner`] (dw-vault `SignerScope`, DASHPAY §3.3):
//!
//! - [`VaultIdentitySigner`]: dpp `Signer<IdentityPublicKey>` for state
//!   transitions, scope `PlatformIdentity`;
//! - [`VaultContactCrypto`]: platform-wallet `ContactCryptoProvider` for the
//!   DIP-15 contact crypto, scope `DashPayCrypto` (never signs);
//! - [`VaultScanKey`]: platform-wallet `ScanKeyResolver` for the identity
//!   scan of `start_wallet_subsystems`.
//!
//! Counterparts at platform `bc321362b9`, which these mirror call for call:
//! `rs-platform-wallet-ffi/src/dashpay.rs` `ResolverContactCryptoProvider`
//! (`:713-893`, the `ContactCryptoProvider` glue) over
//! `rs-sdk-ffi/src/mnemonic_resolver_core_signer.rs` (`:345-590`, the
//! derivations, path gates and zeroization), and
//! `rs-platform-wallet-ffi/src/sign_with_mnemonic_resolver.rs`
//! `dash_sdk_sign_with_mnemonic_resolver_and_path` (`:155-330`, identity
//! signing with the key binding). The vault computes every product
//! (signature, ECDH, mask, ciphertext) and returns only that; two keys reach
//! this module: the auto-accept key DIP-15 hands out on purpose, and the
//! master key [`VaultScanKey`] resolves for the identity scan, released only
//! under an `IdentityScan` grant.
//!
//! Lock: every product these adapters return was released by the vault
//! under the epoch its operation started in, and none is released once
//! `lock()` has returned (dw-vault `Vault::gated`). What they do with a
//! released product (copy a signature into `BinaryData`, hand it to
//! platform-wallet) can still finish after a lock that came later; a flow
//! that must not use such a result ("Lock to cancel") fences at its own
//! commit point under an E0-04 lease, not here.

use async_trait::async_trait;
use dashcore::secp256k1::{PublicKey, SecretKey};
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::{IdentityPublicKey, KeyType};
use dpp::platform_value::BinaryData;
use dpp::state_transition::errors::InvalidIdentityPublicKeyTypeError;
use dw_vault::{ScanKey, SignerError, SignerScope, VaultSigner};
use key_wallet::ExtendedPubKeySigner;
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use platform_wallet::manager::startup::ScanKeyError;
use platform_wallet::{
    ContactCryptoProvider, ContactInfoOpened, ContactInfoSealed, PlatformWalletError,
};
use zeroize::Zeroizing;

use super::keys_policy::key_path;
use crate::EngineError;

fn require_scope(signer: &VaultSigner, scope: SignerScope) -> Result<(), EngineError> {
    if signer.scope() == scope {
        Ok(())
    } else {
        Err(EngineError::InvalidArgument(format!(
            "a {scope:?} signer is required, not {:?}",
            signer.scope()
        )))
    }
}

/// Identity-key signer for state transitions (dpp `Signer<IdentityPublicKey>`).
///
/// A key is found by its DIP-13 slot: `m/9'/coin'/5'/0'/0'/identity'/key'`,
/// with the key index equal to the key id (platform-wallet keeps them equal,
/// `identity_ops.rs:513-518`) and the identity index one of the wallet's
/// identities this signer was built for. The key derived there must be the
/// on-chain key (its compressed public key or HASH160) before anything is
/// signed, the check `dash_sdk_sign_with_mnemonic_resolver_and_path` makes
/// with `expected_key_data`. ECDSA keys only, as the resolver
/// (`resolver_supports_key_type`); identity keys sign no address witness.
#[derive(Debug, Clone)]
pub struct VaultIdentitySigner {
    signer: VaultSigner,
    identity_indices: Vec<u32>,
}

impl VaultIdentitySigner {
    /// `signer` must have scope `PlatformIdentity`; `identity_indices` are
    /// the DIP-13 indices of the identities it signs for.
    pub fn new(
        signer: VaultSigner,
        identity_indices: impl IntoIterator<Item = u32>,
    ) -> Result<Self, EngineError> {
        require_scope(&signer, SignerScope::PlatformIdentity)?;
        Ok(Self {
            signer,
            identity_indices: identity_indices.into_iter().collect(),
        })
    }

    /// The slot `keys_policy` derives the key at.
    fn slot(&self, identity_index: u32, key: &IdentityPublicKey) -> Option<DerivationPath> {
        key_path(self.signer.network(), identity_index, key.id()).ok()
    }

    fn supported(key: &IdentityPublicKey) -> bool {
        matches!(
            key.key_type(),
            KeyType::ECDSA_SECP256K1 | KeyType::ECDSA_HASH160
        )
    }
}

#[async_trait]
impl Signer<IdentityPublicKey> for VaultIdentitySigner {
    async fn sign(
        &self,
        key: &IdentityPublicKey,
        data: &[u8],
    ) -> Result<BinaryData, ProtocolError> {
        if !Self::supported(key) {
            return Err(ProtocolError::InvalidIdentityPublicKeyTypeError(
                InvalidIdentityPublicKeyTypeError::new(key.key_type()),
            ));
        }
        for &identity_index in &self.identity_indices {
            let Some(path) = self.slot(identity_index, key) else {
                continue;
            };
            match self
                .signer
                .sign_identity(&path, key.data().as_slice(), data)
            {
                Ok(signature) => return Ok(signature.to_vec().into()),
                // Not this identity's key: try the next identity.
                Err(SignerError::KeyMismatch(_)) => continue,
                Err(e) => return Err(ProtocolError::ExternalSignerError(e.to_string())),
            }
        }
        Err(ProtocolError::ExternalSignerError(format!(
            "key {} is not a key of this wallet's identities",
            key.id()
        )))
    }

    async fn sign_create_witness(
        &self,
        key: &IdentityPublicKey,
        _data: &[u8],
    ) -> Result<AddressWitness, ProtocolError> {
        Err(ProtocolError::InvalidIdentityPublicKeyTypeError(
            InvalidIdentityPublicKeyTypeError::new(key.key_type()),
        ))
    }

    /// dpp's preflight. Each identity index costs one vault operation (seed
    /// decrypt and derivation); a locked vault or any other signer error
    /// answers `false`, and the `sign` that follows reports the error.
    fn can_sign_with(&self, key: &IdentityPublicKey) -> bool {
        Self::supported(key)
            && self.identity_indices.iter().any(|&i| {
                self.slot(i, key).is_some_and(|path| {
                    self.signer
                        .identity_key_matches(&path, key.data().as_slice())
                        .unwrap_or(false)
                })
            })
    }
}

fn provider_error(e: SignerError) -> PlatformWalletError {
    // `ResolverContactCryptoProvider` reports every signer failure this way
    // (`dashpay.rs:740-893`); a locked vault keeps its own variant so a
    // drain can tell "come back after unlock" from a bad request.
    match e {
        SignerError::Locked => PlatformWalletError::WalletLocked,
        other => PlatformWalletError::InvalidIdentityData(other.to_string()),
    }
}

/// DIP-15 contact crypto (platform-wallet `ContactCryptoProvider`) over a
/// `DashPayCrypto` signer: receiving, auto-accept and seed-binding xpubs;
/// ECDH and the account-reference mask with identity keys; contactInfo seal
/// and open; the auto-accept key export. It signs nothing. The invitation
/// key export keeps the trait's "unsupported" default until invitation
/// creation (roadmap X3) adds it behind its feature.
#[derive(Debug, Clone)]
pub struct VaultContactCrypto {
    signer: VaultSigner,
}

impl VaultContactCrypto {
    /// `signer` must have scope `DashPayCrypto` (`Vault::dashpay_crypto_signer`,
    /// or `Vault::platform_signer` under a `PlatformOp` grant).
    pub fn new(signer: VaultSigner) -> Result<Self, EngineError> {
        require_scope(&signer, SignerScope::DashPayCrypto)?;
        Ok(Self { signer })
    }
}

#[async_trait]
impl ContactCryptoProvider for VaultContactCrypto {
    async fn receiving_xpub(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, PlatformWalletError> {
        self.signer
            .extended_public_key(path)
            .await
            .map_err(provider_error)
    }

    async fn ecdh_shared_secret(
        &self,
        path: &DerivationPath,
        peer: &PublicKey,
    ) -> Result<Zeroizing<[u8; 32]>, PlatformWalletError> {
        self.signer
            .ecdh_shared_secret(path, peer)
            .map_err(provider_error)
    }

    async fn export_auto_accept_private_key(
        &self,
        path: &DerivationPath,
    ) -> Result<SecretKey, PlatformWalletError> {
        let scalar = self
            .signer
            .export_auto_accept_key(path)
            .map_err(provider_error)?;
        // The trait's type: a secp256k1 `SecretKey` does not erase itself.
        SecretKey::from_secret_bytes(*scalar)
            .map_err(|e| PlatformWalletError::InvalidIdentityData(e.to_string()))
    }

    async fn account_reference(
        &self,
        path: &DerivationPath,
        compact_xpub: &[u8],
        account_index: u32,
        version: u32,
    ) -> Result<u32, PlatformWalletError> {
        self.signer
            .account_reference(path, compact_xpub, account_index, version)
            .map_err(provider_error)
    }

    async fn unmask_account_reference(
        &self,
        path: &DerivationPath,
        compact_xpub: &[u8],
        account_reference: u32,
    ) -> Result<(u32, u32), PlatformWalletError> {
        self.signer
            .unmask_account_reference(path, compact_xpub, account_reference)
            .map_err(provider_error)
    }

    async fn contact_info_seal(
        &self,
        root_path: &DerivationPath,
        derivation_index: u32,
        contact_id: &[u8; 32],
        private_data_plaintext: &[u8],
        private_data_iv: &[u8; 16],
    ) -> Result<ContactInfoSealed, PlatformWalletError> {
        let sealed = self
            .signer
            .contact_info_seal(
                root_path,
                derivation_index,
                contact_id,
                private_data_plaintext,
                private_data_iv,
            )
            .map_err(provider_error)?;
        Ok(ContactInfoSealed {
            enc_to_user_id: sealed.enc_to_user_id,
            private_data: sealed.private_data,
        })
    }

    async fn contact_info_open(
        &self,
        root_path: &DerivationPath,
        derivation_index: u32,
        enc_to_user_id: &[u8; 32],
        private_data_blob: &[u8],
    ) -> Result<ContactInfoOpened, PlatformWalletError> {
        let mut opened = self
            .signer
            .contact_info_open(
                root_path,
                derivation_index,
                enc_to_user_id,
                private_data_blob,
            )
            .map_err(provider_error)?;
        // Moved, not copied: the library's type holds the plaintext from
        // here in a plain `Vec` this crate cannot erase.
        Ok(ContactInfoOpened {
            contact_id: opened.contact_id,
            private_data: std::mem::take(&mut *opened.private_data),
        })
    }
}

/// The identity-scan key (platform-wallet `ScanKeyResolver`,
/// `manager/startup.rs:104`): resolved only when the bring-up takes the
/// branch that scans, from a [`ScanKey`] the vault issues under an
/// `IdentityScan` grant (`Vault::scan_key`). The resolved master key erases
/// itself on drop (key-wallet `ExtendedPrivKey: Drop`), and the library also
/// holds it in its `ScanKeyGuard` (`startup.rs:130-150`). A resolved key is
/// beyond the vault's lock: the bring-up must be dropped on lock (E0-05).
#[derive(Debug, Clone)]
pub struct VaultScanKey {
    key: ScanKey,
}

impl VaultScanKey {
    pub fn new(key: ScanKey) -> Self {
        Self { key }
    }

    /// Resolves the master key. A locked vault or an unreadable store is
    /// `Unavailable` (the next start may succeed); a wallet without a seed
    /// or a seed that derives nothing is `Invalid` (no retry fixes it).
    pub fn resolve(&self) -> Result<ExtendedPrivKey, ScanKeyError> {
        self.key.master_key().map_err(|e| match e {
            SignerError::NoSecret
            | SignerError::Derivation(_)
            | SignerError::Vault(dw_vault::VaultError::Corrupt(_)) => {
                ScanKeyError::Invalid(e.to_string())
            }
            other => ScanKeyError::Unavailable(other.to_string()),
        })
    }

    /// The closure `start_wallet_subsystems` takes as its `scan_key`.
    pub fn resolver(
        &self,
    ) -> impl Fn() -> Result<ExtendedPrivKey, ScanKeyError> + Send + Sync + '_ {
        move || self.resolve()
    }
}
