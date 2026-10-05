//! Coin control: UTXO list and user locks (QT-068..075). Owner: E2.
//! Locks are persisted in dw-appdb. Contract: docs/contracts/m1-engine.md §coins.

use crate::NetworkSession;
use crate::api::common::{OutPoint, domain_error_common, not_implemented, parse_wallet_id};

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

domain_error_common!(CoinsError);

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

#[uniffi::export]
impl NetworkSession {
    pub async fn utxos(
        &self,
        wallet_id: String,
        filter: UtxoFilter,
    ) -> Result<Vec<Utxo>, CoinsError> {
        let _ = (parse_wallet_id(&wallet_id)?, filter);
        not_implemented("NetworkSession.utxos")
    }

    /// Locks outpoints against automatic coin selection. Persisted.
    pub async fn lock_outpoints(
        &self,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), CoinsError> {
        let _ = (parse_wallet_id(&wallet_id)?, outpoints);
        not_implemented("NetworkSession.lock_outpoints")
    }

    pub async fn unlock_outpoints(
        &self,
        wallet_id: String,
        outpoints: Vec<OutPoint>,
    ) -> Result<(), CoinsError> {
        let _ = (parse_wallet_id(&wallet_id)?, outpoints);
        not_implemented("NetworkSession.unlock_outpoints")
    }

    pub async fn locked_outpoints(&self, wallet_id: String) -> Result<Vec<OutPoint>, CoinsError> {
        let _ = parse_wallet_id(&wallet_id)?;
        not_implemented("NetworkSession.locked_outpoints")
    }
}
