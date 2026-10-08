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
        let seed = self.gated(VaultError::Locked, |_| {
            let dek = self.key_for(&token)?;
            #[cfg(test)]
            crate::signer::test_hook::fire(crate::signer::test_hook::OpPoint::Opened);
            let payload = self
                .read_record(&dek, &record_id(wallet, REC_SEED))?
                .ok_or(VaultError::NoSecret)?;
            Ok(decode_seed(&payload)?.0)
        })?;
        Ok(f(&seed))
    }
}
