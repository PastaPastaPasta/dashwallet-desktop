//! The wallet's Orchard keys for Platform's shielded pool (0.1 E1;
//! m4-dashpay-engine §2.13, §7 item 72).
//!
//! platform-wallet derives them with ZIP-32 from the seed
//! (`OrchardKeySet::from_seed`, `m_orchard/32'/coin'/account'`, coin 5 on
//! mainnet and 1 elsewhere), as iOS and Android do through the FFI; the
//! wallet spends from account 0. Two scopes reach them, and nothing else of
//! the wallet ([`SignerScope::allows`] refuses them every BIP32 key):
//!
//! - [`SignerScope::ShieldedView`], grant-less like the DashPay crypto
//!   signer, so only while the full key needs no prompt: full viewing keys
//!   for account 0 and identity tip accounts, persisted for pool scans.
//!   The spend authority derived beside it is dropped in the vault.
//! - [`SignerScope::ShieldedSpend`], from a `PlatformOp` grant with credits:
//!   the account's whole key set, spend authority included, because the
//!   library's spends (`operations::transfer`, `withdraw`) take the key set
//!   and no signer. The key set leaves the vault for that one spend; the
//!   engine charges the spend to the grant before it asks.

use platform_wallet::wallet::shielded::{OrchardKeySet, is_shielded_identity_account};

use crate::SignerError;
use crate::signer::{SignerScope, VaultSigner, WalletSigner};
use crate::types::{GrantPurpose, GrantToken, KeyHold, WalletId};
use crate::vault::Vault;

/// The Orchard account the wallet uses.
pub const SHIELDED_ACCOUNT: u32 = 0;

/// The spend authority of a wallet's Orchard account 0, issued under a
/// `PlatformOp` grant. Holds no secret: the key set is derived when
/// [`Self::keys`] is called, which stops working once the vault locks,
/// changes unlock scope or changes its passphrase (or the grant's hold
/// drops). A key set already returned is the caller's.
#[derive(Clone)]
pub struct ShieldedSpendKey {
    signer: VaultSigner,
}

impl std::fmt::Debug for ShieldedSpendKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShieldedSpendKey")
            .field("wallet_id", &hex::encode(self.signer.wallet_id()))
            .finish_non_exhaustive()
    }
}

impl ShieldedSpendKey {
    pub fn wallet_id(&self) -> WalletId {
        self.signer.wallet_id()
    }

    /// The account's key set, spend authority included.
    pub fn keys(&self) -> Result<OrchardKeySet, SignerError> {
        let network = self.signer.network();
        self.signer
            .with_seed(|seed| orchard_keys(seed, network, SHIELDED_ACCOUNT))
    }
}

fn orchard_keys(
    seed: &[u8],
    network: dashcore::Network,
    account: u32,
) -> Result<OrchardKeySet, SignerError> {
    OrchardKeySet::from_seed(seed, network, account)
        .map_err(|e| SignerError::Derivation(e.to_string()))
}

impl Vault {
    /// The [`ShieldedSpendKey`] of `wallet`, authorized by a redeemed
    /// `PlatformOp` grant for that wallet whose `max_credits` is not 0 (a
    /// spend's quote is in credits); any other grant is
    /// `GrantPurposeMismatch`. Lifetime rules are those of
    /// [`Vault::platform_signer`]. Engine-only, like the scan key.
    pub fn shielded_spend_key(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<ShieldedSpendKey, crate::VaultError> {
        self.shielded_spend_key_with(wallet, None, token)
    }

    /// [`Vault::shielded_spend_key`] for a token whose key `hold` holds:
    /// the spend key stops when the hold drops.
    pub fn shielded_spend_key_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
    ) -> Result<ShieldedSpendKey, crate::VaultError> {
        self.shielded_spend_key_with(wallet, Some(hold), token)
    }

    fn shielded_spend_key_with(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
    ) -> Result<ShieldedSpendKey, crate::VaultError> {
        match token.purpose() {
            GrantPurpose::PlatformOp { max_credits, .. } if max_credits != 0 => {}
            _ => return Err(crate::VaultError::GrantPurposeMismatch),
        }
        self.token_signer(wallet, token, SignerScope::ShieldedSpend, hold)
            .map(|signer| ShieldedSpendKey { signer })
    }

    /// The 96-byte full viewing key of `wallet`'s Orchard account 0
    /// (platform-wallet's `AccountViewingKeys::to_fvk_bytes`). No grant: it
    /// can see the account's notes but not spend them, so, like
    /// [`Vault::dashpay_crypto_signer`], it is read only while the full key
    /// needs no prompt; `Locked` or `MixingOnly` otherwise.
    pub fn shielded_viewing_key(&self, wallet: &WalletId) -> Result<[u8; 96], crate::VaultError> {
        self.shielded_viewing_keys(wallet, &[SHIELDED_ACCOUNT])
            .map(|keys| keys[0].1)
    }

    /// [`Vault::shielded_viewing_key`] for each of `accounts`: account 0 and
    /// the identity tip accounts (`SHIELDED_IDENTITY_ACCOUNT_BASE` plus an
    /// identity's index), whose viewing keys platform-wallet's seedless bind
    /// also requires; any other account is refused. One seed read for all.
    pub fn shielded_viewing_keys(
        &self,
        wallet: &WalletId,
        accounts: &[u32],
    ) -> Result<Vec<(u32, [u8; 96])>, crate::VaultError> {
        if let Some(account) =
            (accounts.iter()).find(|&&a| a != SHIELDED_ACCOUNT && !is_shielded_identity_account(a))
        {
            return Err(crate::VaultError::Internal(format!(
                "no viewing key for Orchard account {account}"
            )));
        }
        let signer = self.prompt_free_signer(wallet, SignerScope::ShieldedView)?;
        let network = signer.network();
        signer
            .with_seed(|seed| {
                (accounts.iter())
                    .map(|&account| {
                        orchard_keys(seed, network, account)
                            .map(|keys| (account, keys.viewing_keys().to_fvk_bytes()))
                    })
                    .collect()
            })
            .map_err(|e| match e {
                SignerError::Locked => crate::VaultError::Locked,
                e => crate::VaultError::Internal(e.to_string()),
            })
    }
}
