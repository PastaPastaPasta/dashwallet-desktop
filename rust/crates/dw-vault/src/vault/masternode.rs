//! Vault operations of the masternode features (M3 R3,
//! docs/contracts/m3-engine.md §2.5):
//!
//! - keys attached to tracked masternodes (IOS-082), stored as records
//!   `mnkey/<proTxHash hex>/<role>` sealed under the data key like the wallet
//!   seeds, written with a `MasternodeOp` grant and read with a
//!   `MasternodeOp` or `RevealSecret` grant;
//! - a wallet's seed for one closure under a `RevealSecret` grant, so the
//!   engine can derive one provider private key to reveal (IOS-083) without
//!   the seed leaving this crate's buffers;
//! - a non-consuming look at which wallet a grant is bound to, for calls that
//!   act on no wallet of their own (attaching a key to a tracked masternode)
//!   and accept a grant bound to any wallet.

use zeroize::Zeroizing;

use super::{REC_SEED, Vault, decode_seed, record_id};
use crate::VaultError;
use crate::types::{GrantKind, GrantToken, WalletId};

/// Record id prefix of attached masternode keys.
const MN_KEY_PREFIX: &str = "mnkey/";
/// Longest key a caller may attach (a WIF is 52 characters, an ed25519
/// key pair 64 bytes, a hex BLS secret 64 characters).
pub const MAX_MASTERNODE_KEY_BYTES: usize = 256;

fn mn_record_id(pro_tx_hash: &[u8; 32], role: &str) -> String {
    format!("{MN_KEY_PREFIX}{}/{role}", hex::encode(pro_tx_hash))
}

fn valid_role(role: &str) -> Result<(), VaultError> {
    if role.is_empty()
        || role.len() > 32
        || !role.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    {
        return Err(VaultError::InvalidArgument(format!(
            "masternode key role {role:?} is not a lowercase name"
        )));
    }
    Ok(())
}

impl Vault {
    /// The wallet a grant is bound to, without consuming it. `None` for an
    /// unknown or expired grant and for vault-wide grants.
    pub fn grant_wallet(&self, grant_id: &str) -> Option<WalletId> {
        let now = self.now();
        let mut inner = self.inner();
        inner.drop_expired_grants(now);
        inner.grants.get(grant_id).and_then(|g| g.grant.wallet)
    }

    /// Runs `f` with `wallet`'s 64-byte seed under a redeemed
    /// `RevealSecret` grant for that wallet; the decrypted seed is erased
    /// when `f` returns.
    pub fn with_revealed_seed<T>(
        &self,
        wallet: &WalletId,
        grant_id: &str,
        f: impl FnOnce(&[u8; 64]) -> T,
    ) -> Result<T, VaultError> {
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        let dek = self.key_for(&token)?;
        let payload = self
            .read_record(&dek, &record_id(wallet, REC_SEED))?
            .ok_or(VaultError::NoSecret)?;
        let (seed, _) = decode_seed(&payload)?;
        Ok(f(&seed))
    }

    /// Stores `key` for `role` of the tracked masternode `pro_tx_hash`,
    /// replacing an earlier one, with a redeemed `MasternodeOp` grant.
    pub fn store_masternode_key(
        &self,
        token: &GrantToken,
        pro_tx_hash: &[u8; 32],
        role: &str,
        key: &[u8],
    ) -> Result<(), VaultError> {
        if token.purpose.kind() != GrantKind::MasternodeOp {
            return Err(VaultError::GrantPurposeMismatch);
        }
        valid_role(role)?;
        if key.is_empty() || key.len() > MAX_MASTERNODE_KEY_BYTES {
            return Err(VaultError::InvalidArgument(format!(
                "a masternode key has 1..={MAX_MASTERNODE_KEY_BYTES} bytes"
            )));
        }
        let dek = self.key_for(token)?;
        self.commit_records(&dek, &[(mn_record_id(pro_tx_hash, role), Some(key))])
    }

    /// The key attached for `role`, read with a redeemed `MasternodeOp` or
    /// `RevealSecret` grant. `Ok(None)` when none is attached.
    pub fn masternode_key(
        &self,
        token: &GrantToken,
        pro_tx_hash: &[u8; 32],
        role: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        if !matches!(
            token.purpose.kind(),
            GrantKind::MasternodeOp | GrantKind::RevealSecret
        ) {
            return Err(VaultError::GrantPurposeMismatch);
        }
        valid_role(role)?;
        let dek = self.key_for(token)?;
        self.read_record(&dek, &mn_record_id(pro_tx_hash, role))
    }

    /// Roles with a key attached to `pro_tx_hash`. In-memory read of the
    /// record ids; nothing is decrypted.
    pub fn masternode_key_roles(&self, pro_tx_hash: &[u8; 32]) -> Vec<String> {
        let prefix = format!("{MN_KEY_PREFIX}{}/", hex::encode(pro_tx_hash));
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
    /// when `role` is `None`. Needs the vault's full-scope data key (an
    /// unlocked or unencrypted vault). Returns how many were deleted.
    pub fn delete_masternode_keys(
        &self,
        pro_tx_hash: &[u8; 32],
        role: Option<&str>,
    ) -> Result<usize, VaultError> {
        let ids: Vec<String> = match role {
            Some(r) => {
                valid_role(r)?;
                let id = mn_record_id(pro_tx_hash, r);
                self.masternode_key_roles(pro_tx_hash)
                    .iter()
                    .any(|have| have == r)
                    .then_some(id)
                    .into_iter()
                    .collect()
            }
            None => self
                .masternode_key_roles(pro_tx_hash)
                .iter()
                .map(|r| mn_record_id(pro_tx_hash, r))
                .collect(),
        };
        if ids.is_empty() {
            return Ok(0);
        }
        let dek = self.full_dek()?;
        let changes: Vec<(String, Option<&[u8]>)> =
            ids.iter().map(|id| (id.clone(), None)).collect();
        self.commit_records(&dek, &changes)?;
        Ok(ids.len())
    }
}
