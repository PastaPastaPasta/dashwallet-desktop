//! Vault operations of the masternode features (MG-01 salvage of
//! `m3/r3-protx`, docs/design/MG-01-salvage.md D7):
//!
//! - a wallet's seed for one closure under a `RevealSecret` grant, so the
//!   engine can derive one provider private key to reveal (IOS-083) without
//!   the seed leaving this crate's zeroing buffers;
//! - keys attached to tracked masternodes (IOS-082), sealed under the data
//!   key like the wallet seeds. Roles are the wallet-side ones only
//!   ([`MASTERNODE_KEY_ROLES`]): the wallet never holds an operator or
//!   platform-node secret. Writing, deleting and listing need the
//!   full-scope data key, as `store_wallet_secret` does; reading needs a
//!   redeemed `RevealSecret` grant.
//!
//! A record id is `mnkey/<HMAC-SHA256 of the proTxHash>/<role>`, keyed from
//! the data key, so the vault file (and every backup of it) does not show
//! which masternodes the user controls, while the AEAD's associated data
//! still binds each record to its id. Replacing or deleting a key drains
//! the gate first ([`Vault::drain_ops`]), as a seed deletion does: a read
//! that started before has returned, one after sees the change. A `lock()`
//! racing a change either refuses it or waits for it
//! ([`Vault::with_drained_key`], D22); listing is a gated operation.
//!
//! The records are vault-wide (a tracked masternode belongs to no wallet):
//! removing a wallet leaves them, and `recover_with_mnemonic` drops them
//! with every other record of the old vault (its backup copy keeps them).

use dashcore::hashes::{Hash, HashEngine, Hmac, HmacEngine, hash160, sha256};
use dashcore::secp256k1::{PublicKey, SecretKey};
use zeroize::Zeroizing;

use super::{KEY_LOAD_ATTEMPTS, OpGuard, REC_SEED, Vault, WriteGuard, decode_seed, record_id};
use crate::crypto::Key32;
use crate::types::{GrantKind, KeySource, WalletId};
use crate::{SignerError, VaultError};

/// Record id prefix of attached masternode keys.
const MN_KEY_PREFIX: &str = "mnkey/";
/// The roles a key may be attached for: the owner and voting keys and the
/// payout address's key. `operator` and `platform` are refused.
pub const MASTERNODE_KEY_ROLES: [&str; 3] = [ROLE_OWNER, ROLE_VOTING, "payout"];

/// The role of an attached voting key, the one a vote signer reads (MG-05).
pub(crate) const ROLE_VOTING: &str = "voting";
/// The role of an attached owner key, the one a ProUpRegTx's owner
/// signature reads (MG-04 D12).
pub(crate) const ROLE_OWNER: &str = "owner";
/// Longest key a caller may attach (a WIF is 52 characters).
pub const MAX_MASTERNODE_KEY_BYTES: usize = 256;

/// Label of the record-id key derived from the data key.
const MN_ID_KEY_LABEL: &[u8] = b"dw-vault masternode record id v1";

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut engine = HmacEngine::<sha256::Hash>::new(key);
    engine.input(data);
    Hmac::<sha256::Hash>::from_engine(engine).to_byte_array()
}

/// `mnkey/<HMAC(HMAC(dek, label), proTxHash)>/`: the record-id prefix of
/// one masternode's keys.
fn mn_record_prefix(dek: &Key32, pro_tx_hash: &[u8; 32]) -> String {
    let id_key = Zeroizing::new(hmac_sha256(&dek[..], MN_ID_KEY_LABEL));
    let id = hmac_sha256(&id_key[..], pro_tx_hash);
    format!("{MN_KEY_PREFIX}{}/", hex::encode(id))
}

fn mn_record_id(dek: &Key32, pro_tx_hash: &[u8; 32], role: &str) -> String {
    format!("{}{role}", mn_record_prefix(dek, pro_tx_hash))
}

fn valid_role(role: &str) -> Result<(), VaultError> {
    if MASTERNODE_KEY_ROLES.contains(&role) {
        Ok(())
    } else {
        Err(VaultError::InvalidArgument(format!(
            "masternode key role {role:?} is not one of {MASTERNODE_KEY_ROLES:?}"
        )))
    }
}

impl Vault {
    /// Runs `f` with `wallet`'s 64-byte seed under a redeemed
    /// `RevealSecret` grant for that wallet; the decrypted seed is erased
    /// when `f` returns. The seed is read inside one vault operation (a
    /// `lock()` waits for that read); `f` runs after it, since it may block
    /// (the engine's derivation takes platform-wallet's manager lock), so a
    /// reveal whose seed was read before a lock still completes.
    pub fn with_revealed_seed<T>(
        &self,
        wallet: &WalletId,
        grant_id: &str,
        f: impl FnOnce(&[u8; 64]) -> T,
    ) -> Result<T, VaultError> {
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        let seed = self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
            let payload = self
                .read_record(&dek, &record_id(wallet, REC_SEED))?
                .ok_or(VaultError::NoSecret)?;
            Ok(decode_seed(&payload)?.0)
        })?;
        Ok(f(&seed))
    }

    /// Stores `key` for `role` of the tracked masternode `pro_tx_hash`,
    /// replacing an earlier one, then checks the write by reading the file
    /// back. Needs the data key with full scope (an unlocked or unencrypted
    /// vault); drains the gate before the write
    /// ([`Self::with_drained_key`]).
    pub fn store_masternode_key(
        &self,
        pro_tx_hash: &[u8; 32],
        role: &str,
        key: &[u8],
    ) -> Result<(), VaultError> {
        valid_role(role)?;
        if key.is_empty() || key.len() > MAX_MASTERNODE_KEY_BYTES {
            return Err(VaultError::InvalidArgument(format!(
                "a masternode key has 1..={MAX_MASTERNODE_KEY_BYTES} bytes"
            )));
        }
        self.with_drained_key(|writer, dek| {
            self.commit_records_under(
                writer,
                dek,
                &[(mn_record_id(dek, pro_tx_hash, role), Some(key))],
            )
        })
    }

    /// Runs `body` under `writer` with the gate drained and the full-scope
    /// data key (MG-01 D22). The key is loaded first, outside both (an
    /// unencrypted vault's comes from the OS store, which may block), then
    /// read again from memory once the gate is drained, in the epoch the
    /// call began in: a `lock()` (or any epoch change) before then fails
    /// `Locked`, and none can end the epoch until `body` returns. An
    /// unencrypted vault needs no prompt for its key, so there the load and
    /// the drain are tried again, at most [`KEY_LOAD_ATTEMPTS`] times in all,
    /// as [`Vault::with_loaded_key`] does.
    fn with_drained_key<T>(
        &self,
        body: impl FnOnce(&WriteGuard<'_>, &Key32) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let mut attempt = 1;
        loop {
            let epoch = self.inner().epoch;
            // Loads the key only; the one used is read again below.
            self.full_dek()?;
            #[cfg(test)]
            crate::signer::test_hook::checked();
            let writer = self.writer();
            let _drained = self.drain_ops(&writer);
            let dek = {
                let inner = self.inner();
                if inner.epoch == epoch {
                    Self::memory_dek(&inner)
                } else {
                    Err(VaultError::Locked)
                }
            };
            match dek {
                Err(VaultError::Locked) if attempt < KEY_LOAD_ATTEMPTS && self.unencrypted() => {
                    attempt += 1;
                }
                dek => {
                    let dek = dek?;
                    #[cfg(test)]
                    crate::signer::test_hook::opened();
                    return body(&writer, &dek);
                }
            }
        }
    }

    /// The key attached for `role` of `pro_tx_hash`, read under a redeemed
    /// `RevealSecret` grant for `wallet` (the wallet the user acts from;
    /// the record itself is vault-wide). `Ok(None)` when none is attached.
    /// The grant is consumed either way.
    pub fn masternode_key(
        &self,
        wallet: &WalletId,
        grant_id: &str,
        pro_tx_hash: &[u8; 32],
        role: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        valid_role(role)?;
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
            self.read_record(&dek, &mn_record_id(&dek, pro_tx_hash, role))
        })
    }

    /// Roles with a key attached to `pro_tx_hash`, in name order. Needs
    /// the data key with full scope (`Locked` otherwise): the record ids
    /// are keyed from it. Nothing is decrypted. One gated operation: a
    /// `lock()` waits for it, and a list made in an ended epoch is refused.
    pub fn masternode_key_roles(&self, pro_tx_hash: &[u8; 32]) -> Result<Vec<String>, VaultError> {
        self.with_loaded_key(|dek| {
            #[cfg(test)]
            crate::signer::test_hook::opened();
            Ok(self.roles_under(dek, pro_tx_hash))
        })
    }

    /// The key id of the key attached for `role` of `pro_tx_hash`: the
    /// hash160 of its compressed public key, which isn't secret. `Ok(None)`
    /// when none is attached or the record is no secp256k1 secret. Needs the
    /// data key with full scope, as listing the roles does; the secret is
    /// decrypted into a zeroing buffer and doesn't leave (MG-03 D31: the
    /// engine records the id of what the vault holds after a write whose
    /// outcome it couldn't tell). The vault directory is synced first, so
    /// a key read from a file whose rename wasn't yet durable (its
    /// directory fsync failed) is durable when its id is returned.
    pub fn masternode_key_id(
        &self,
        pro_tx_hash: &[u8; 32],
        role: &str,
    ) -> Result<Option<[u8; 20]>, VaultError> {
        valid_role(role)?;
        self.with_loaded_key(|dek| {
            #[cfg(test)]
            crate::signer::test_hook::opened();
            let Some(secret) = self.read_record(dek, &mn_record_id(dek, pro_tx_hash, role))? else {
                return Ok(None);
            };
            let Some(secret) = secret_of(&secret) else {
                return Ok(None);
            };
            std::fs::File::open(&self.shared.dir)?.sync_all()?;
            let public = PublicKey::from_secret_key(&secret);
            Ok(Some(
                hash160::Hash::hash(&public.serialize()).to_byte_array(),
            ))
        })
    }

    /// The secret attached for `role` of `pro_tx_hash`, for a signer issued
    /// at `epoch` whose data key is `key` (MG-05 §3): read inside the
    /// operation `op`, which must have started in that epoch, as
    /// [`Vault::signing_seed`] reads a seed. `NoSecret` when none is
    /// attached or the record is no secp256k1 secret. The caller erases it.
    pub(crate) fn attached_secret(
        &self,
        op: &OpGuard<'_>,
        epoch: u64,
        key: &KeySource,
        pro_tx_hash: &[u8; 32],
        role: &str,
    ) -> Result<SecretKey, SignerError> {
        if op.epoch != epoch {
            return Err(SignerError::Locked);
        }
        let dek = match key.copy()? {
            Some(key) => key,
            None => Zeroizing::new(**self.inner().dek.as_ref().ok_or(SignerError::Locked)?),
        };
        let record = self
            .read_record(&dek, &mn_record_id(&dek, pro_tx_hash, role))?
            .ok_or(SignerError::NoSecret)?;
        secret_of(&record).ok_or(SignerError::NoSecret)
    }

    fn roles_under(&self, dek: &Key32, pro_tx_hash: &[u8; 32]) -> Vec<String> {
        let prefix = mn_record_prefix(dek, pro_tx_hash);
        let inner = self.inner();
        let Some(f) = inner.file.as_ref() else {
            return Vec::new();
        };
        f.records
            .keys()
            .filter_map(|id| id.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }

    /// Deletes the attached key of `role`, or every key of `pro_tx_hash`
    /// when `role` is `None`. Needs the data key with full scope; the ids
    /// are listed and deleted under `writer`, after draining the gate
    /// ([`Self::with_drained_key`]).
    /// Returns how many were deleted.
    pub fn delete_masternode_keys(
        &self,
        pro_tx_hash: &[u8; 32],
        role: Option<&str>,
    ) -> Result<usize, VaultError> {
        if let Some(r) = role {
            valid_role(r)?;
        }
        self.with_drained_key(|writer, dek| {
            let changes: Vec<(String, Option<&[u8]>)> = self
                .roles_under(dek, pro_tx_hash)
                .into_iter()
                .filter(|have| role.is_none_or(|r| r == have))
                .map(|r| (mn_record_id(dek, pro_tx_hash, &r), None))
                .collect();
            if !changes.is_empty() {
                self.commit_records_under(writer, dek, &changes)?;
            }
            Ok(changes.len())
        })
    }
}

/// The secp256k1 secret a 32-byte record holds, if it is one. secp256k1
/// takes the bytes by value, so one unerased copy passes through its
/// constructor (MG-05 review L9).
fn secret_of(record: &[u8]) -> Option<SecretKey> {
    let bytes = Zeroizing::new(<[u8; 32]>::try_from(record).ok()?);
    SecretKey::from_secret_bytes(*bytes).ok()
}
