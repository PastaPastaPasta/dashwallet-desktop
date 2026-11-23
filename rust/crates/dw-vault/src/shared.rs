//! A shared masternode's owner signatures (MG-04b §4, §6.2, D11).
//!
//! As with a ProUpRegTx ([`crate::OwnerKey`]), a share owner key signs no
//! digest it is given: the owner keys are the shared masternode's
//! authority (consent, dissolution, reward, keys), and a digest signer at
//! one would sign anything. Each signer takes the transaction or payload
//! bytes, decodes them and computes Core's digest itself (rc.3
//! `evo/providertx.cpp:335-373,506-533`, `evo/providertx.h:497-503,531-545`).
//! Only keychain owner keys `m/9'/coin'/3'/2'/i` sign them, under
//! [`crate::SignerScope::MasternodeOwner`]; the attached owner key stays
//! ProUpRegTx-only. The signature is Core's `SignCompact` of the raw
//! digest, which `CHashSigner::VerifyHashCanonical` checks against the
//! share's `keyIDOwner`.
//!
//! The bytes are checked before any key is read
//! ([`SignerError::Vault`] with [`VaultError::InvalidArgument`]); which
//! share the key may sign for is checked against the key derived at
//! `index` before signing ([`SignerError::KeyMismatch`]). Core's rules for
//! each type and D16's lock rules are the engine's (MG-04b §4, §5.1).

use dashcore::hashes::{Hash, hash160};
use dashcore::secp256k1::{PublicKey, SecretKey};
use dw_protx::shared::registration::TRANSACTION_PROVIDER_REGISTER;
use dw_protx::shared::{
    ProDisTx, ProRegTxV3, ProUpShareTx, ProUpSharedRegTx, SpecialTx, TRANSACTION_PROVIDER_DISSOLVE,
};

use crate::paths::owner_key_path;
use crate::signer::{KeyUse, VaultSigner, sign_compact};
use crate::{SignerError, VaultError};

fn malformed(what: &str) -> SignerError {
    SignerError::Vault(VaultError::InvalidArgument(what.into()))
}

/// The hash160 of `secret`'s compressed public key: a share's
/// `keyIDOwner` when the share is this key's.
fn key_id_of(secret: &SecretKey) -> [u8; 20] {
    hash160::Hash::hash(&PublicKey::from_secret_key(secret).serialize()).to_byte_array()
}

/// `bytes` as a whole transaction of type `tx_type`; trailing bytes are
/// refused.
fn special_tx(bytes: &[u8], tx_type: u16, what: &str) -> Result<SpecialTx, SignerError> {
    let tx = SpecialTx::from_bytes(bytes).map_err(|_| malformed(what))?;
    if tx.tx_type != tx_type {
        return Err(malformed(what));
    }
    Ok(tx)
}

/// `bytes` as a shared registration: a type-1 transaction whose payload is
/// a version-3 ProRegTx with a share table.
fn shared_registration(bytes: &[u8]) -> Result<(SpecialTx, ProRegTxV3), SignerError> {
    const WHAT: &str = "not a shared ProRegTx";
    let tx = special_tx(bytes, TRANSACTION_PROVIDER_REGISTER, WHAT)?;
    let payload = ProRegTxV3::from_payload(&tx.payload).map_err(|_| malformed(WHAT))?;
    if !payload.is_shared() {
        return Err(malformed(WHAT));
    }
    Ok((tx, payload))
}

/// Refuses a key that owns no share of `payload`: a wallet signs only for
/// a share it holds (MG-04b §4).
fn owns_a_share(payload: &ProRegTxV3, key_id: [u8; 20]) -> Result<(), SignerError> {
    if payload.shares.iter().any(|s| s.owner_key_id == key_id) {
        Ok(())
    } else {
        Err(SignerError::KeyMismatch("an owner key of no share".into()))
    }
}

impl VaultSigner {
    /// Signs `digest` with keychain owner key `index` once `check` accepts
    /// the key's id; nothing is signed otherwise.
    fn sign_as_owner(
        &self,
        index: u32,
        digest: [u8; 32],
        check: impl FnOnce([u8; 20]) -> Result<(), SignerError>,
    ) -> Result<[u8; 65], SignerError> {
        let path = owner_key_path(self.network(), index);
        self.with_key(&path, KeyUse::OwnerPayload, |x| {
            check(key_id_of(&x.private_key))?;
            Ok(sign_compact(digest, &x.private_key))
        })?
    }

    /// The join signature of keychain owner key `index` on the shared
    /// registration `tx_bytes` (the whole transaction, as prepared or
    /// combined): `MakeSharedRegConsentHash(tx)`. The key must be the
    /// owner key of at least one share in the payload
    /// ([`SignerError::KeyMismatch`] otherwise).
    pub fn sign_shared_consent(
        &self,
        index: u32,
        tx_bytes: &[u8],
    ) -> Result<[u8; 65], SignerError> {
        let (tx, payload) = shared_registration(tx_bytes)?;
        let digest = payload.consent_hash(&tx);
        self.sign_as_owner(index, digest, |key_id| owns_a_share(&payload, key_id))
    }

    /// A ProUpShareTx's owner signature by keychain owner key `index` over
    /// `payload_bytes` (the payload, any signature it carries left out):
    /// `::SerializeHash(ptx)` without `vchSig`. Which share's key signs is
    /// the engine's check: the payload carries no share table.
    pub fn sign_update_share(
        &self,
        index: u32,
        payload_bytes: &[u8],
    ) -> Result<[u8; 65], SignerError> {
        let payload: ProUpShareTx = dashcore::consensus::deserialize(payload_bytes)
            .map_err(|_| malformed("not a ProUpShareTx payload"))?;
        self.sign_as_owner(index, payload.sign_hash(), |_| Ok(()))
    }

    /// A ProUpSharedRegTx's owner signature by keychain owner key `index`
    /// over `payload_bytes`: `::SerializeHash(ptx)` without the signature
    /// count and signatures. Which share's key signs is the engine's
    /// check, as for [`Self::sign_update_share`].
    pub fn sign_update_shared_registrar(
        &self,
        index: u32,
        payload_bytes: &[u8],
    ) -> Result<[u8; 65], SignerError> {
        let payload: ProUpSharedRegTx = dashcore::consensus::deserialize(payload_bytes)
            .map_err(|_| malformed("not a ProUpSharedRegTx payload"))?;
        self.sign_as_owner(index, payload.sign_hash(), |_| Ok(()))
    }

    /// A dissolution signature by keychain owner key `index` on the
    /// ProDisTx `tx_bytes`: `MakeSignHash(tx, sig_count)`.
    ///
    /// The ProDisTx carries no share table, so `registration_bytes` is the
    /// masternode's shared registration (MG-04b D29: from the wallet's
    /// history), whose txid must be the payload's `proTxHash`. `sig_count`
    /// is 1 (unilateral: the payload's `actorIndex` must be the share whose
    /// owner key is `index`'s, as Core signs with `shares[actor_index]`'s
    /// key, `providertx_service.cpp:1267-1268`) or the registration's share
    /// count (unanimous: the key must own one of its shares); nothing else.
    pub fn sign_dissolution(
        &self,
        index: u32,
        tx_bytes: &[u8],
        registration_bytes: &[u8],
        sig_count: u8,
    ) -> Result<[u8; 65], SignerError> {
        const WHAT: &str = "not a ProDisTx";
        let tx = special_tx(tx_bytes, TRANSACTION_PROVIDER_DISSOLVE, WHAT)?;
        let payload: ProDisTx =
            dashcore::consensus::deserialize(&tx.payload).map_err(|_| malformed(WHAT))?;
        let (registration, shared) = shared_registration(registration_bytes)?;
        if registration.txid() != payload.pro_tx_hash {
            return Err(malformed("the registration of another masternode"));
        }
        let Some(actor) = shared.shares.get(usize::from(payload.actor_index)) else {
            return Err(malformed("an actor index past the share table"));
        };
        let share_count = shared.shares.len();
        if sig_count != 1 && usize::from(sig_count) != share_count {
            return Err(malformed(
                "a signature count other than 1 or the share count",
            ));
        }
        let digest = payload.sign_hash(
            tx.version,
            tx.lock_time,
            &tx.prevouts(),
            &tx.output,
            sig_count,
        );
        let actor_key_id = actor.owner_key_id;
        self.sign_as_owner(index, digest, |key_id| {
            if sig_count == 1 {
                if key_id == actor_key_id {
                    Ok(())
                } else {
                    Err(SignerError::KeyMismatch(
                        "the owner key of another share than the actor's".into(),
                    ))
                }
            } else {
                owns_a_share(&shared, key_id)
            }
        })
    }
}
