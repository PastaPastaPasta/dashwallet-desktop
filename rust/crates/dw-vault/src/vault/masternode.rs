//! Vault access for the masternode keychain (IOS-083,
//! docs/contracts/m3-engine.md §2.5): a wallet's seed for one closure under
//! a `RevealSecret` grant, so the engine can derive one provider private key
//! to reveal without the seed leaving this crate's zeroing buffers.

use super::{REC_SEED, Vault, decode_seed, record_id};
use crate::VaultError;
use crate::types::{GrantKind, WalletId};

impl Vault {
    /// Runs `f` with `wallet`'s 64-byte seed under a redeemed
    /// `RevealSecret` grant for that wallet; the decrypted seed is erased
    /// when `f` returns. `f` runs inside one vault operation (a `lock()`
    /// waits for it), so it must not call back into the vault.
    pub fn with_revealed_seed<T>(
        &self,
        wallet: &WalletId,
        grant_id: &str,
        f: impl FnOnce(&[u8; 64]) -> T,
    ) -> Result<T, VaultError> {
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        let _op = self.op_guard();
        let dek = self.key_for(&token)?;
        let payload = self
            .read_record(&dek, &record_id(wallet, REC_SEED))?
            .ok_or(VaultError::NoSecret)?;
        let (seed, _) = decode_seed(&payload)?;
        Ok(f(&seed))
    }
}
