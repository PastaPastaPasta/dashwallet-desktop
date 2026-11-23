//! M2 vault calls: quick-unlock policy (slot B, IOS-011/016), recovery with
//! the phrase (IOS-014) and destroying an empty vault (IOS-009/109). Owner:
//! S1 (dw-vault slot B). `enroll_quick_unlock` / `remove_quick_unlock` are in
//! `vault.rs`. Contract: docs/contracts/m2-engine.md §2.9. Every call runs on
//! the session's dw-vault vault; the rules live there.

use dw_engine::platform::RevokeCause;
use zeroize::Zeroizing;

use crate::api::common::parse_wallet_id;
use crate::api::vault::OwnedCredential;
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

impl From<dw_vault::QuickUnlockPolicy> for QuickUnlockPolicy {
    fn from(p: dw_vault::QuickUnlockPolicy) -> Self {
        Self {
            enrolled: p.enrolled,
            spend_limit_duffs: p.spend_limit_duffs,
            passphrase_max_age_secs: p.passphrase_max_age_secs,
            last_passphrase_at: p.last_passphrase_at,
        }
    }
}

#[uniffi::export]
impl Vault {
    /// In-memory read; works while locked.
    pub fn quick_unlock_policy(&self) -> Result<QuickUnlockPolicy, VaultError> {
        self.check_open()?;
        Ok(self.session.vault().quick_unlock_policy().into())
    }

    /// Sets the biometric spending limit (IOS-016) to one of the iOS options
    /// (`invalid_argument` otherwise). Needs a `ChangeCredential` grant
    /// issued with the passphrase.
    pub async fn set_quick_unlock_spend_limit(
        &self,
        grant_id: String,
        spend_limit_duffs: u64,
    ) -> Result<QuickUnlockPolicy, VaultError> {
        self.check_open()?;
        self.op(move |v| v.set_quick_unlock_spend_limit(&grant_id, spend_limit_duffs))
            .await
            .map(Into::into)
    }

    /// Forgot passphrase (IOS-014, DESIGN-opus §1.8): checks that
    /// `mnemonic` + `bip39_passphrase` derive `wallet_id`
    /// (`vault.recovery_mismatch` otherwise), then replaces the vault with a
    /// new one encrypted with `new_passphrase` holding that wallet's phrase.
    /// Secrets of other wallets cannot be read without the old passphrase:
    /// they are dropped and listed in `wallets_without_secrets` (the old
    /// vault file is kept as `vault.dwv.replaced-<time>`). Resets the
    /// attempt throttle; removes the quick-unlock slot. The new vault is
    /// unlocked. `wallet_not_found` when `wallet_id` is not registered.
    pub async fn recover_with_mnemonic(
        &self,
        wallet_id: String,
        mnemonic: Vec<u8>,
        bip39_passphrase: Vec<u8>,
        new_passphrase: Vec<u8>,
    ) -> Result<VaultRecovery, VaultError> {
        let mnemonic = Zeroizing::new(mnemonic);
        let bip39_passphrase = Zeroizing::new(bip39_passphrase);
        let new_passphrase = Zeroizing::new(new_passphrase);
        let id = parse_wallet_id(&wallet_id)?;
        self.check_open()?;
        self.session.wallet_info(&id)?;
        let lost = self
            .revoking_op(RevokeCause::Lock, move |v| {
                v.recover_with_mnemonic(&id.0, &mnemonic, &bip39_passphrase, &new_passphrase)
            })
            .await?;
        Ok(VaultRecovery {
            status: self.session.vault().status().into(),
            wallets_without_secrets: lost.iter().map(hex::encode).collect(),
        })
    }

    /// Deletes the vault (files, OS-store key) once it holds no secrets
    /// (`vault.not_empty`): the last step of "Delete All" / wipe after every
    /// wallet was removed with `remove_wallet`. The host deletes its
    /// quick-unlock item. `credential` must satisfy the `Wipe` row of the
    /// credential table: the passphrase on an encrypted vault
    /// (`vault.credential_required` otherwise), nothing on an unencrypted
    /// one. Returns `NoVault` status; idempotent.
    pub async fn destroy(&self, credential: VaultCredential) -> Result<VaultStatus, VaultError> {
        let credential = OwnedCredential::from(credential);
        self.check_open()?;
        self.revoking_op(RevokeCause::Lock, move |v| {
            v.destroy(credential.as_credential())
        })
        .await
        .map(Into::into)
    }
}
