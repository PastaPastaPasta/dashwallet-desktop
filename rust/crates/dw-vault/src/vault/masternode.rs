//! Vault operations of the masternode features (MG-01 salvage of
//! `m3/r3-protx`, docs/design/MG-01-salvage.md D7):
//!
//! - a wallet's seed for one closure under a `RevealSecret` grant, so the
//!   engine can derive one provider private key to reveal (IOS-083) without
//!   the seed leaving this crate's zeroing buffers;
//! - keys attached to tracked masternodes (IOS-082), stored as records
//!   `mnkey/<proTxHash hex>/<role>` sealed under the data key like the wallet
//!   seeds. Roles are the wallet-side ones only ([`MASTERNODE_KEY_ROLES`]):
//!   the wallet never holds an operator or platform-node secret. Writing and
//!   deleting need the full-scope data key, as `store_wallet_secret` does;
//!   reading needs a redeemed `RevealSecret` grant.
//!
//! The records are vault-wide (a tracked masternode belongs to no wallet):
//! removing a wallet leaves them, and `recover_with_mnemonic` drops them
//! with every other record of the old vault (its backup copy keeps them).

use zeroize::Zeroizing;

use super::{REC_SEED, Vault, decode_seed, record_id};
use crate::VaultError;
use crate::types::{GrantKind, WalletId};

/// Record id prefix of attached masternode keys.
const MN_KEY_PREFIX: &str = "mnkey/";
/// The roles a key may be attached for: the owner and voting keys and the
/// payout address's key. `operator` and `platform` are refused.
pub const MASTERNODE_KEY_ROLES: [&str; 3] = ["owner", "voting", "payout"];
/// Longest key a caller may attach (a WIF is 52 characters).
pub const MAX_MASTERNODE_KEY_BYTES: usize = 256;

fn mn_record_prefix(pro_tx_hash: &[u8; 32]) -> String {
    format!("{MN_KEY_PREFIX}{}/", hex::encode(pro_tx_hash))
}

fn mn_record_id(pro_tx_hash: &[u8; 32], role: &str) -> String {
    format!("{}{role}", mn_record_prefix(pro_tx_hash))
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
    /// vault).
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
        let dek = self.full_dek()?;
        let writer = self.writer();
        self.commit_records_under(
            &writer,
            &dek,
            &[(mn_record_id(pro_tx_hash, role), Some(key))],
        )
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
            self.read_record(&dek, &mn_record_id(pro_tx_hash, role))
        })
    }

    /// Roles with a key attached to `pro_tx_hash`. In-memory read of the
    /// record ids; nothing is decrypted.
    pub fn masternode_key_roles(&self, pro_tx_hash: &[u8; 32]) -> Vec<String> {
        let prefix = mn_record_prefix(pro_tx_hash);
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
    /// when `role` is `None`. Needs the data key with full scope. Returns
    /// how many were deleted.
    pub fn delete_masternode_keys(
        &self,
        pro_tx_hash: &[u8; 32],
        role: Option<&str>,
    ) -> Result<usize, VaultError> {
        if let Some(r) = role {
            valid_role(r)?;
        }
        let ids: Vec<String> = self
            .masternode_key_roles(pro_tx_hash)
            .into_iter()
            .filter(|have| role.is_none_or(|r| r == have))
            .map(|r| mn_record_id(pro_tx_hash, &r))
            .collect();
        if ids.is_empty() {
            return Ok(0);
        }
        let dek = self.full_dek()?;
        let writer = self.writer();
        let changes: Vec<(String, Option<&[u8]>)> =
            ids.iter().map(|id| (id.clone(), None)).collect();
        self.commit_records_under(&writer, &dek, &changes)?;
        Ok(ids.len())
    }
}
