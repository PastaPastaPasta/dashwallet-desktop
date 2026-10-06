//! M2 vault calls: quick-unlock policy (slot B, IOS-011/016), recovery with
//! the phrase (IOS-014) and destroying an empty vault (IOS-009/109). Owner:
//! S1 (dw-vault slot B). `enroll_quick_unlock` / `remove_quick_unlock` are in
//! `vault.rs`. Contract: docs/contracts/m2-engine.md §2.9.

use zeroize::Zeroizing;

use crate::api::common::{not_implemented, parse_wallet_id};
use crate::{Vault, VaultCredential, VaultError, VaultStatus};

/// Biometric quick-unlock rules, enforced by `Vault.authorize` (DESIGN-opus
/// §1.8): a `QuickUnlock` credential issues a `Spend` grant only up to
/// `spend_limit_duffs` (`vault.quick_unlock_limit_exceeded` above) and only
/// while the passphrase was entered within `passphrase_max_age_secs`
/// (`vault.passphrase_stale` after). Never for `RevealSecret`,
/// `ChangeCredential` or `Wipe` (`vault.credential_required`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QuickUnlockPolicy {
    pub enrolled: bool,
    /// iOS options 0, 0.1, 0.5 (default), 1 and 5 DASH, in duffs.
    pub spend_limit_duffs: u64,
    /// 7 days.
    pub passphrase_max_age_secs: u64,
    /// Last successful passphrase entry (unlock or passphrase grant).
    pub last_passphrase_at: Option<u64>,
}

/// Outcome of `recover_with_mnemonic`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VaultRecovery {
    pub status: VaultStatus,
    /// Wallets whose secrets were in the old vault and are now watch-only:
    /// re-import their phrases to restore their keys.
    pub wallets_without_secrets: Vec<String>,
}

#[uniffi::export]
impl Vault {
    /// In-memory read.
    pub fn quick_unlock_policy(&self) -> Result<QuickUnlockPolicy, VaultError> {
        self.check_open()?;
        not_implemented("Vault.quick_unlock_policy")
    }

    /// Sets the biometric spending limit (IOS-016) to one of the iOS options
    /// (`invalid_argument` otherwise). Needs a `ChangeCredential` grant
    /// issued with the passphrase.
    pub async fn set_quick_unlock_spend_limit(
        &self,
        grant_id: String,
        spend_limit_duffs: u64,
    ) -> Result<QuickUnlockPolicy, VaultError> {
        let _ = (grant_id, spend_limit_duffs);
        self.check_open()?;
        not_implemented("Vault.set_quick_unlock_spend_limit")
    }

    /// Forgot passphrase (IOS-014, DESIGN-opus §1.8): checks that
    /// `mnemonic` + `bip39_passphrase` derive `wallet_id`
    /// (`vault.recovery_mismatch` otherwise), then replaces the vault with a
    /// new one encrypted with `new_passphrase` holding that wallet's phrase.
    /// Secrets of other wallets cannot be read without the old passphrase:
    /// they are dropped and listed in `wallets_without_secrets`. Resets the
    /// attempt throttle; removes the quick-unlock slot.
    pub async fn recover_with_mnemonic(
        &self,
        wallet_id: String,
        mnemonic: Vec<u8>,
        bip39_passphrase: Vec<u8>,
        new_passphrase: Vec<u8>,
    ) -> Result<VaultRecovery, VaultError> {
        let _secrets = (
            Zeroizing::new(mnemonic),
            Zeroizing::new(bip39_passphrase),
            Zeroizing::new(new_passphrase),
        );
        parse_wallet_id(&wallet_id)?;
        self.check_open()?;
        not_implemented("Vault.recover_with_mnemonic")
    }

    /// Deletes the vault (files, OS-store key, quick-unlock item) once no
    /// registered wallet has secrets in it (`vault.not_empty`): the last
    /// step of "Delete All" / wipe after every wallet was removed with
    /// `remove_wallet`. `credential` must satisfy the `Wipe` row of the
    /// credential table (passphrase on an encrypted vault). Returns
    /// `NoVault` status.
    pub async fn destroy(&self, credential: VaultCredential) -> Result<VaultStatus, VaultError> {
        let _credential = crate::api::vault::OwnedCredential::from(credential);
        self.check_open()?;
        not_implemented("Vault.destroy")
    }
}
