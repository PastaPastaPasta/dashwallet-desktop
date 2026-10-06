//! Coin control: UTXO list and user locks (QT-068..075). Owner: E2.
//! Locks are persisted in dw-appdb. Contract: docs/contracts/m1-engine.md §coins.

use crate::NetworkSession;
use crate::api::common::{OutPoint, parse_wallet_id};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Utxo {
    pub outpoint: OutPoint,
    pub address: String,
    pub amount: u64,
    pub confirmations: u32,
    pub block_height: Option<u32>,
    /// Block or first-seen time, UNIX seconds.
    pub timestamp: Option<u64>,
    pub instant_locked: bool,
    pub chain_locked: bool,
    /// Locked by the user (dash-qt "Lock unspent").
    pub user_locked: bool,
    /// Reserved by a prepared, not yet broadcast transaction.
    pub reserved: bool,
    pub label: Option<String>,
    pub is_change: bool,
    pub is_coinbase: bool,
    /// A CoinJoin denomination output.
    pub coinjoin_denominated: bool,
    /// Completed mixing rounds; `None` when not denominated or unknown.
    pub coinjoin_rounds: Option<u32>,
    /// Spendable now: mature, not locked or reserved, keys available.
    pub spendable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UtxoFilter {
    pub include_locked: bool,
    /// Only fully mixed coins (QT-071 CoinJoin filtering).
    pub fully_mixed_only: bool,
    pub min_confirmations: Option<u32>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoinsError {
    /// Code `coins.outpoint_not_found`: not an unspent output of the wallet.
    #[error("outpoint not found")]
    OutpointNotFound { outpoint: OutPoint },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

impl crate::api::common::NotImplementedError for CoinsError {
    fn not_implemented(call: &'static str) -> Self {
        Self::NotImplemented {
            call: call.to_string(),
        }
    }
}

impl From<dw_engine::EngineError> for CoinsError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::OutpointNotFound(o) => Self::OutpointNotFound { outpoint: o.into() },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(_) => Self::NotImplemented { call: detail },
            _ => Self::Internal { detail },
        }
    }
}

impl CoinsError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::OutpointNotFound { .. } => "coins.outpoint_not_found",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

fn outpoints_of(list: Vec<OutPoint>) -> Result<Vec<dashcore::OutPoint>, CoinsError> {
    Ok(list
        .iter()
        .map(OutPoint::to_core)
        .collect::<Result<_, _>>()?)
}

#[uniffi::export]
impl NetworkSession {
    /// Coin control list, largest first. `fully_mixed_only` returns
    /// `NotImplemented` until CoinJoin rounds are tracked; `coinjoin_rounds`
    /// is always `None` (unknown).
    pub async fn utxos(
        &self,
        wallet_id: String,
        filter: UtxoFilter,
    ) -> Result<Vec<Utxo>, CoinsError> {
        let id = parse_wallet_id(&wallet_id)?;
        let rows = self
            .inner
            .utxos(
                id,
                dw_engine::CoinFilter {
                    include_locked: filter.include_locked,
                    fully_mixed_only: filter.fully_mixed_only,
                    min_confirmations: filter.min_confirmations,
                },
            )
            .await?;
        Ok(rows
            .into_iter()
            .map(|c| Utxo {
                outpoint: c.outpoint.into(),
                address: c.address,
                amount: c.amount,
                confirmations: c.confirmations,
                block_height: c.block_height,
                timestamp: c.timestamp,
                instant_locked: c.instant_locked,
                chain_locked: c.chain_locked,
                user_locked: c.user_locked,
                reserved: c.reserved,
                label: c.label,
                is_change: c.is_change,
                is_coinbase: c.is_coinbase,
                coinjoin_denominated: c.coinjoin_denominated,
                coinjoin_rounds: c.coinjoin_rounds,
                spendable: c.spendable,
            })
            .collect())
    }

    /// Locks outpoints against coin selection ("Lock unspent"). Persisted.
    pub async fn lock_outpoints(
        &self,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), CoinsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.lock_outpoints(id, outpoints_of(outpoints)?).await?)
    }

    /// Deletes user locks and releases dust locks.
    pub async fn unlock_outpoints(
        &self,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), CoinsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.unlock_outpoints(id, outpoints_of(outpoints)?).await?)
    }

    pub async fn locked_outpoints(&self, wallet_id: String) -> Result<Vec<OutPoint>, CoinsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .locked_outpoints(id)
            .await?
            .into_iter()
            .map(OutPoint::from)
            .collect())
    }

    /// Dust attack protection threshold in duffs; `None` = off (QT-075).
    pub async fn dust_protection(&self) -> Result<Option<u64>, CoinsError> {
        Ok(self.inner.dust_protection().await?)
    }

    /// Turns dust protection on (1..=1,000,000 duffs) or off. Small foreign
    /// incoming coins are locked the next time the wallet's coins are read.
    pub async fn set_dust_protection(&self, threshold: Option<u64>) -> Result<(), CoinsError> {
        Ok(self.inner.set_dust_protection(threshold).await?)
    }
}
