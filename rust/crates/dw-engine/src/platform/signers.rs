//! The host signers platform-wallet needs (DASHPAY §3.1 `signers.rs`), each
//! over a scoped [`VaultSigner`] (dw-vault `SignerScope`, DASHPAY §3.3):
//!
//! - [`VaultIdentitySigner`]: dpp `Signer<IdentityPublicKey>` for state
//!   transitions, scope `PlatformIdentity`;
//! - [`VaultContactCrypto`]: platform-wallet `ContactCryptoProvider` for the
//!   DIP-15 contact crypto, scope `DashPayCrypto` (never signs);
//! - [`VaultScanKey`]: platform-wallet `ScanKeyResolver` for the identity
//!   scan of `start_wallet_subsystems`;
//! - [`CountingFundingSigner`]: key-wallet `Signer` + `ExtendedPubKeySigner`
//!   for `create_funded_asset_lock_proof`, scope `PlatformFunding`, counting
//!   into a Mode B call permit (E0-04 §2a.3).
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use dashcore::secp256k1::{PublicKey, SecretKey, ecdsa};
use dpp::ProtocolError;
use dpp::address_funds::AddressWitness;
use dpp::identity::identity_public_key::accessors::v0::IdentityPublicKeyGettersV0;
use dpp::identity::signer::Signer;
use dpp::identity::{IdentityPublicKey, KeyID, KeyType};
use dpp::platform_value::BinaryData;
use dpp::state_transition::errors::InvalidIdentityPublicKeyTypeError;
use dw_vault::{ScanKey, SignerError, SignerScope, VaultSigner};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use key_wallet::{ExtendedPubKeySigner, Signer as _, SignerMethod};
use platform_wallet::manager::startup::ScanKeyError;
use platform_wallet::{
    ContactCryptoProvider, ContactInfoOpened, ContactInfoSealed, PlatformWalletError,
};
use zeroize::Zeroizing;

use super::keys_policy::{self, KeySpec, key_path};
use super::lease::{FundingRelease, SignatureCount};
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
///
/// A flow narrows it to the key ids its transition signs with
/// ([`Self::only_keys`]): any other key, the master included, is refused
/// before the vault is asked, and `can_sign_with` answers `false`, so the
/// library's key choice cannot fall on it.
///
/// Its vault signer never leaves it (DP2-02 Sol r5): a raw signer would sign
/// past the call permit's count and the key allow-list. Public keys come
/// from [`Self::derive_public_keys`]; there is no accessor:
///
/// ```compile_fail
/// fn raw(s: &dw_engine::platform::VaultIdentitySigner) {
///     let _ = s.vault_signer();
/// }
/// ```
#[derive(Debug, Clone)]
pub struct VaultIdentitySigner {
    signer: VaultSigner,
    identity_indices: Vec<u32>,
    /// Mode B: the call permit's count of released signatures (§2a.3).
    count: Option<Arc<SignatureCount>>,
    /// `None`: every key of the identities.
    keys: Option<BTreeSet<KeyID>>,
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
            count: None,
            keys: None,
        })
    }

    /// Signs with the keys `ids` only (DP2-02: a flow's transition names
    /// its keys).
    pub(crate) fn only_keys(mut self, ids: impl IntoIterator<Item = KeyID>) -> Self {
        self.keys = Some(ids.into_iter().collect());
        self
    }

    /// The public keys `specs` of the identity at `identity_index`, one of
    /// this signer's, as registration derives them
    /// ([`keys_policy::derive_keys`]). Public material only: no signature
    /// is made, so none is counted. Refused once [`Self::only_keys`]
    /// narrowed the signer, so the narrowing cannot be stepped around.
    pub async fn derive_public_keys(
        &self,
        identity_index: u32,
        specs: &[KeySpec],
    ) -> Result<BTreeMap<KeyID, IdentityPublicKey>, EngineError> {
        if self.keys.is_some() {
            return Err(EngineError::Internal(
                "a narrowed identity signer derives no keys".into(),
            ));
        }
        if !self.identity_indices.contains(&identity_index) {
            return Err(EngineError::InvalidArgument(format!(
                "identity index {identity_index} is not this signer's"
            )));
        }
        // `keys_policy::derive_keys` inlined: the signer never leaves this
        // type, not even by reference.
        keys_policy::unique_specs(specs)?;
        let mut keys = BTreeMap::new();
        for spec in specs {
            let path = key_path(self.signer.network(), identity_index, spec.id)?;
            let public_key = self.signer.public_key(&path).await?;
            keys.insert(spec.id, keys_policy::spec_key(spec, &public_key));
        }
        Ok(keys)
    }

    fn allowed(&self, key: &IdentityPublicKey) -> bool {
        self.keys.as_ref().is_none_or(|ids| ids.contains(&key.id()))
    }

    /// Reports each released signature, and a `Locked` refusal, to `count`.
    pub(crate) fn counting(mut self, count: Arc<SignatureCount>) -> Self {
        self.count = Some(count);
        self
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
        if !self.allowed(key) {
            return Err(ProtocolError::ExternalSignerError(format!(
                "key {} is not a key this flow signs with",
                key.id()
            )));
        }
        // Counted before the vault is asked (E0-04 review M-1): refused
        // once the call permit ended.
        let signing = match &self.count {
            Some(count) => match count.begin() {
                Some(signing) => Some(signing),
                None => {
                    return Err(ProtocolError::ExternalSignerError(
                        SignerError::Locked.to_string(),
                    ));
                }
            },
            None => None,
        };
        for &identity_index in &self.identity_indices {
            let Some(path) = self.slot(identity_index, key) else {
                continue;
            };
            match self
                .signer
                .sign_identity(&path, key.data().as_slice(), data)
            {
                Ok(signature) => {
                    if let Some(signing) = signing {
                        signing.released();
                    }
                    return Ok(signature.to_vec().into());
                }
                // Not this identity's key: try the next identity.
                Err(SignerError::KeyMismatch(_)) => continue,
                Err(e) => {
                    if let Some(count) = &self.count {
                        count.observe_err(&e);
                    }
                    return Err(ProtocolError::ExternalSignerError(e.to_string()));
                }
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
    /// answers `false`, and the `sign` that follows reports the error. A
    /// `Locked` refusal is reported to the call permit's count here too
    /// (review L-3): dpp may stop at the preflight, with no `sign`. Once
    /// the call permit ended it answers `false` without asking the vault.
    fn can_sign_with(&self, key: &IdentityPublicKey) -> bool {
        Self::supported(key)
            && self.allowed(key)
            && !self.count.as_ref().is_some_and(|c| c.closed())
            && self.identity_indices.iter().any(|&i| {
                self.slot(i, key).is_some_and(|path| {
                    match self
                        .signer
                        .identity_key_matches(&path, key.data().as_slice())
                    {
                        Ok(matches) => matches,
                        Err(e) => {
                            if let Some(count) = &self.count {
                                count.observe_err(&e);
                            }
                            false
                        }
                    }
                })
            })
    }
}

/// The lease's `PlatformFunding` signer during one Mode B funding call
/// (E0-04 §2a.3): it delegates every call to the vault signer and reports
/// each released signature (its sighash kept, §2a.6), and any `Locked` refusal, to the
/// call permit's count. A public key or xpub is no signature: only `sign_ecdsa` counts as
/// released, counted before the vault is asked, and refused `Locked` once
/// the permit ended (review M-1, `SignatureCount`), and refused once a lock
/// of the call was recorded unless it signed the sighash already (one lock
/// per call, O-1, `SignatureCount::admits`). Whatever key-wallet's builder requires of a signer
/// (`TransactionSigner` is implemented for every `Signer`) comes through
/// these two traits.
///
/// Its first signature waits for the call's release to be recorded against
/// the step's marker ([`FundingRelease`], review P2b r6 R6-01); a failed
/// record refuses the signature, the vault asked nothing.
#[derive(Debug, Clone)]
pub struct CountingFundingSigner {
    signer: VaultSigner,
    count: Arc<SignatureCount>,
    release: Arc<FundingRelease>,
}

impl CountingFundingSigner {
    pub(crate) fn new(
        signer: VaultSigner,
        count: Arc<SignatureCount>,
        release: Arc<FundingRelease>,
    ) -> Self {
        Self {
            signer,
            count,
            release,
        }
    }

    fn note<T>(&self, r: Result<T, SignerError>) -> Result<T, SignerError> {
        if let Err(e) = &r {
            self.count.observe_err(e);
        }
        r
    }
}

#[async_trait]
impl key_wallet::Signer for CountingFundingSigner {
    type Error = SignerError;

    fn supported_methods(&self) -> &[SignerMethod] {
        self.signer.supported_methods()
    }

    async fn sign_ecdsa(
        &self,
        path: &DerivationPath,
        sighash: [u8; 32],
    ) -> Result<(ecdsa::Signature, PublicKey), SignerError> {
        let Some(signing) = self.count.begin() else {
            return Err(SignerError::Locked);
        };
        if !self.count.admits(sighash) {
            return Err(SignerError::PathNotAllowed(format!(
                "{path}: the call's asset lock is already signed"
            )));
        }
        // Counted in `signing` meanwhile: the permit's end reads the call
        // possibly sent, so no `NotSent` row supersedes the marker under
        // the write.
        if let Err(e) = self.release.record().await {
            tracing::warn!(error = %e, "a funding signature refused: its release is not recorded");
            return Err(SignerError::PathNotAllowed(format!(
                "{path}: the funding release could not be recorded"
            )));
        }
        // The permit may have ended during the write; its end counted this
        // signing (possibly sent), and nothing is asked of the vault.
        if self.count.closed() {
            return Err(SignerError::Locked);
        }
        let signed = self.note(self.signer.sign_ecdsa(path, sighash).await)?;
        signing.released_with(sighash);
        Ok(signed)
    }

    async fn public_key(&self, path: &DerivationPath) -> Result<PublicKey, SignerError> {
        self.note(self.signer.public_key(path).await)
    }
}

#[async_trait]
impl ExtendedPubKeySigner for CountingFundingSigner {
    async fn extended_public_key(
        &self,
        path: &DerivationPath,
    ) -> Result<ExtendedPubKey, SignerError> {
        self.note(self.signer.extended_public_key(path).await)
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

    /// `contact_info_open`, telling a document apart from the signer: one
    /// whose keys do not derive or whose ciphertext does not decrypt (another
    /// client's, a corrupt one) is `None`; a signer that cannot run (locked,
    /// out of scope, no seed) is the error.
    pub(crate) fn contact_info_open_or_skip(
        &self,
        root_path: &DerivationPath,
        derivation_index: u32,
        enc_to_user_id: &[u8; 32],
        private_data_blob: &[u8],
    ) -> Result<Option<dw_vault::ContactInfoOpened>, super::PlatformError> {
        let opened = self.signer.contact_info_open(
            root_path,
            derivation_index,
            enc_to_user_id,
            private_data_blob,
        );
        match opened {
            Ok(opened) => Ok(Some(opened)),
            Err(SignerError::Decrypt(_) | SignerError::Derivation(_)) => Ok(None),
            Err(e) => Err(provider_error(e).into()),
        }
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

/// Contact crypto with no key: every call is `WalletLocked`. A contact
/// payment without DashPay crypto reserves its address through it (DP3-01
/// D6): the library's drain returns before asking a provider when nothing
/// is queued, and refuses when something is.
#[derive(Debug, Clone, Copy, Default)]
pub struct LockedContactCrypto;

#[async_trait]
impl ContactCryptoProvider for LockedContactCrypto {
    async fn receiving_xpub(
        &self,
        _: &DerivationPath,
    ) -> Result<ExtendedPubKey, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn ecdh_shared_secret(
        &self,
        _: &DerivationPath,
        _: &PublicKey,
    ) -> Result<Zeroizing<[u8; 32]>, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn export_auto_accept_private_key(
        &self,
        _: &DerivationPath,
    ) -> Result<SecretKey, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn account_reference(
        &self,
        _: &DerivationPath,
        _: &[u8],
        _: u32,
        _: u32,
    ) -> Result<u32, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn unmask_account_reference(
        &self,
        _: &DerivationPath,
        _: &[u8],
        _: u32,
    ) -> Result<(u32, u32), PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn contact_info_seal(
        &self,
        _: &DerivationPath,
        _: u32,
        _: &[u8; 32],
        _: &[u8],
        _: &[u8; 16],
    ) -> Result<ContactInfoSealed, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
    }

    async fn contact_info_open(
        &self,
        _: &DerivationPath,
        _: u32,
        _: &[u8; 32],
        _: &[u8],
    ) -> Result<ContactInfoOpened, PlatformWalletError> {
        Err(PlatformWalletError::WalletLocked)
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

#[cfg(test)]
mod tests {
    /// No signing handle leaves this module (DP2-02 Sol r5, r6). Outside its
    /// tests:
    /// - no function here returns anything that names a `VaultSigner`;
    /// - no `type` alias names one;
    /// - no field holding one is visible outside its type;
    /// - `self.signer` is only ever the receiver of a method call
    ///   (`self.signer.<method>(..)`), never returned, cloned, borrowed or
    ///   passed on.
    ///
    /// This is a tripwire over the source text, not a parser: it catches the
    /// obvious ways back to the raw signer, and review is the real check.
    /// The `compile_fail` doctest on [`super::VaultIdentitySigner`] covers
    /// the public surface; this covers `pub(crate)` too.
    #[test]
    fn no_vault_signer_leaves_the_module() {
        let source = include_str!("signers.rs");
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        let mut rest = production;
        while let Some(at) = rest.find("fn ") {
            let sig = &rest[at..];
            let end = sig.find(['{', ';']).unwrap_or(sig.len());
            let sig = &sig[..end];
            if let Some((_, returns)) = sig.split_once("->") {
                assert!(
                    !returns.contains("VaultSigner"),
                    "a signer leaves the module: {sig}"
                );
            }
            rest = &rest[at + 3..];
        }
        for line in production.lines() {
            let line = line.trim();
            assert!(
                !(line.starts_with("pub")
                    && !line.contains("fn ")
                    && line.contains(": VaultSigner")),
                "a visible signer field: {line}"
            );
            assert!(
                !(line.split_whitespace().any(|w| w == "type") && line.contains("VaultSigner")),
                "a signer alias: {line}"
            );
        }
        // Whitespace dropped, so `self\n.signer\n.sign(..)` reads as one.
        let compact: String = production.split_whitespace().collect();
        for (at, _) in compact.match_indices("self.signer") {
            let after = &compact[at + "self.signer".len()..];
            let method = after
                .strip_prefix('.')
                .and_then(|m| m.split_once('('))
                .map(|(name, _)| name)
                .filter(|name| {
                    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
                });
            let shown = &compact[at..(at + 60).min(compact.len())];
            assert!(method.is_some(), "self.signer handed out: {shown}");
            assert!(
                !matches!(method, Some("clone" | "to_owned" | "as_ref" | "borrow")),
                "self.signer copied out: {shown}"
            );
        }
    }
}
