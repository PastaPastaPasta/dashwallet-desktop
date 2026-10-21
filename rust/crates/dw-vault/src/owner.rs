//! A ProUpRegTx's owner signature (MG-04 D11, D12).
//!
//! As with a vote ([`crate::VoteSigner`]), an owner key signs no digest it
//! is given: coins sent to an owner key's address would be spendable by any
//! digest signer at that path. The signer takes the payload and hashes it
//! itself, as Core's `CHashSigner::SignHash(::SerializeHash(ptx), …)` does
//! (rc.3 `evo/providertx_service.cpp:324-334`), so the only owner
//! signatures the engine can obtain are over ProUpRegTx payloads.

use dashcore::blockdata::transaction::special_transaction::SpecialTransactionBasePayloadEncodable;
use dashcore::blockdata::transaction::special_transaction::provider_update_registrar::ProviderUpdateRegistrarPayload;
use dashcore::hashes::Hash;

use crate::SignerError;
use crate::paths::owner_key_path;
use crate::signer::{KeyUse, VaultSigner, sign_compact};
use crate::vault::masternode::ROLE_OWNER;

/// Which owner key signs a ProUpRegTx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerKey {
    /// The wallet's keychain owner key `m/9'/coin'/3'/2'/index`.
    Keychain { index: u32 },
    /// The `owner` key attached to the tracked masternode `pro_tx_hash`
    /// (wire order), for that masternode's payloads only (D12).
    Attached { pro_tx_hash: [u8; 32] },
}

impl VaultSigner {
    /// The owner's signature of `payload`: Core's `SignCompact` (65 bytes,
    /// compressed key, RFC 6979) over the payload's hash without its
    /// signature. Only a [`crate::SignerScope::MasternodeOwner`] signer
    /// reaches an owner key for it; an attached key signs only a payload
    /// whose `proTxHash` is its masternode's ([`SignerError::KeyMismatch`]
    /// otherwise, before the key is read).
    pub fn sign_update_registrar(
        &self,
        key: &OwnerKey,
        payload: &ProviderUpdateRegistrarPayload,
    ) -> Result<[u8; 65], SignerError> {
        let digest = payload.base_payload_hash().to_byte_array();
        match key {
            OwnerKey::Keychain { index } => {
                let path = owner_key_path(self.network(), *index);
                self.with_key(&path, KeyUse::OwnerPayload, |x| {
                    sign_compact(digest, &x.private_key)
                })
            }
            OwnerKey::Attached { pro_tx_hash } => {
                if payload.pro_tx_hash.to_byte_array() != *pro_tx_hash {
                    return Err(SignerError::KeyMismatch(
                        "the attached owner key of another masternode".into(),
                    ));
                }
                self.with_attached_key(pro_tx_hash, ROLE_OWNER, KeyUse::OwnerPayload, |secret| {
                    sign_compact(digest, secret)
                })
            }
        }
    }
}
