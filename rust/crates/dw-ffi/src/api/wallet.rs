use zeroize::Zeroizing;

use crate::{EngineError, NetworkSession};

/// Core balance buckets in duffs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
}

impl From<dw_engine::WalletBalances> for WalletBalances {
    fn from(b: dw_engine::WalletBalances) -> Self {
        Self {
            confirmed: b.confirmed,
            unconfirmed: b.unconfirmed,
            immature: b.immature,
            locked: b.locked,
            total: b.total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletSummary {
    /// 64-char lowercase hex.
    pub wallet_id: String,
    pub balances: WalletBalances,
}

/// TODO(vault): the mnemonic crosses the FFI only until dw-vault exists; then
/// creation returns the id and the phrase is revealed via the vault's
/// `reveal_secret(grant)` into a zeroing `SecretBytes` (DESIGN-opus §1.5 rule 5).
#[derive(Clone, uniffi::Record)]
pub struct CreatedWallet {
    pub wallet_id: String,
    pub mnemonic: String,
}

impl std::fmt::Debug for CreatedWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedWallet")
            .field("wallet_id", &self.wallet_id)
            .finish_non_exhaustive()
    }
}

#[uniffi::export]
impl NetworkSession {
    /// New wallet from a fresh English mnemonic (12 or 24 words).
    pub async fn create_wallet(&self, word_count: u8) -> Result<CreatedWallet, EngineError> {
        let created = self.inner.create_wallet(word_count).await?;
        Ok(CreatedWallet {
            wallet_id: created.wallet_id.to_string(),
            mnemonic: created.mnemonic.as_str().to_owned(),
        })
    }

    /// Restore from a mnemonic. `birth_height` 0 = scan from genesis; `None` =
    /// platform-wallet default (SPV tip or latest checkpoint).
    pub async fn import_wallet(
        &self,
        mnemonic: String,
        birth_height: Option<u32>,
    ) -> Result<String, EngineError> {
        let id = self
            .inner
            .import_wallet(Zeroizing::new(mnemonic), birth_height)
            .await?;
        Ok(id.to_string())
    }

    pub fn list_wallets(&self) -> Result<Vec<WalletSummary>, EngineError> {
        Ok(self
            .inner
            .list_wallets()?
            .into_iter()
            .map(|w| WalletSummary {
                wallet_id: w.wallet_id.to_string(),
                balances: w.balances.into(),
            })
            .collect())
    }

    pub fn balances(&self, wallet_id: String) -> Result<WalletBalances, EngineError> {
        let id = wallet_id.parse().map_err(EngineError::from)?;
        Ok(self.inner.balances(&id)?.into())
    }
}
