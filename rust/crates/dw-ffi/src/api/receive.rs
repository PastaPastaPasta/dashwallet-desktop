//! Receive addresses, address list and payment requests. Owner: E1
//! (requests are stored in dw-appdb, owned by E2).
//! Contract: docs/contracts/m1-engine.md §receive.

use crate::NetworkSession;
use crate::api::common::{domain_error_common, parse_wallet_id};

/// External (receiving) or internal (change) BIP44 chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AddressChain {
    Receiving,
    Change,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AddressInfo {
    pub address: String,
    pub chain: AddressChain,
    pub index: u32,
    /// e.g. `m/44'/5'/0'/0/7`.
    pub derivation_path: String,
    /// Has received funds at least once.
    pub used: bool,
    pub label: Option<String>,
    /// Current balance held by this address, duffs.
    pub balance: Option<u64>,
    pub tx_count: u32,
}

/// Address list filter. `None` = either.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AddressFilter {
    pub chain: Option<AddressChain>,
    pub used: Option<bool>,
}

/// A stored payment request (QT-081/083, IOS-055).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ReceiveRequest {
    pub id: u64,
    /// UNIX seconds.
    pub created_at: u64,
    pub address: String,
    /// Duffs; `None` = any amount.
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
    /// The `dash:` URI for this request (dw-uri `format_bitcoin_uri`).
    pub uri: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ReceiveError {
    /// Code `receive.request_not_found`.
    #[error("request {id} not found")]
    RequestNotFound { id: u64 },
    /// Code `receive.gap_limit`: too many unused addresses already issued.
    #[error("gap limit reached")]
    GapLimit,
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

domain_error_common!(@not_implemented ReceiveError);

impl From<dw_engine::EngineError> for ReceiveError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::GapLimit => Self::GapLimit,
            E::RequestNotFound(id) => Self::RequestNotFound { id },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

crate::api::common::export_error_code!(ReceiveError);

impl ReceiveError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
        match self {
            Self::RequestNotFound { .. } => "receive.request_not_found",
            Self::GapLimit => "receive.gap_limit",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

impl From<AddressChain> for dw_engine::AddressChain {
    fn from(c: AddressChain) -> Self {
        match c {
            AddressChain::Receiving => Self::Receiving,
            AddressChain::Change => Self::Change,
        }
    }
}

impl From<dw_engine::AddressChain> for AddressChain {
    fn from(c: dw_engine::AddressChain) -> Self {
        match c {
            dw_engine::AddressChain::Receiving => Self::Receiving,
            dw_engine::AddressChain::Change => Self::Change,
        }
    }
}

impl From<dw_engine::AddressInfo> for AddressInfo {
    fn from(a: dw_engine::AddressInfo) -> Self {
        Self {
            address: a.address,
            chain: a.chain.into(),
            index: a.index,
            derivation_path: a.derivation_path,
            used: a.used,
            label: a.label,
            balance: a.balance,
            tx_count: a.tx_count,
        }
    }
}

impl From<dw_engine::ReceiveRequest> for ReceiveRequest {
    fn from(r: dw_engine::ReceiveRequest) -> Self {
        Self {
            id: r.id,
            created_at: r.created_at,
            address: r.address,
            amount: r.amount,
            label: r.label,
            message: r.message,
            uri: r.uri,
        }
    }
}

#[uniffi::export]
impl NetworkSession {
    /// The first unused receiving address (IOS-053). Hosts re-query after
    /// `HistoryChanged`, which is how the shown address rotates once paid.
    pub async fn current_receive_address(
        &self,
        wallet_id: String,
    ) -> Result<AddressInfo, ReceiveError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.current_receive_address(id).await?.into())
    }

    /// Issues a receiving address that was never issued before and labels
    /// it (dash-qt "Request payment" always uses a fresh address, QT-081).
    /// `GapLimit` once every address inside the gap limit is issued.
    pub async fn next_receive_address(
        &self,
        wallet_id: String,
        label: Option<String>,
    ) -> Result<AddressInfo, ReceiveError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.next_receive_address(id, label).await?.into())
    }

    /// Addresses of the wallet's BIP44 account 0 (QT-096 receiving tab), by
    /// chain then index.
    pub async fn addresses(
        &self,
        wallet_id: String,
        filter: AddressFilter,
    ) -> Result<Vec<AddressInfo>, ReceiveError> {
        let id = parse_wallet_id(&wallet_id)?;
        let filter = dw_engine::AddressFilter {
            chain: filter.chain.map(Into::into),
            used: filter.used,
        };
        Ok(self
            .inner
            .addresses(id, filter)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Stores a payment request on a freshly issued address and returns it.
    /// An amount of 0 means "any amount".
    pub async fn create_receive_request(
        &self,
        wallet_id: String,
        amount: Option<u64>,
        label: Option<String>,
        message: Option<String>,
    ) -> Result<ReceiveRequest, ReceiveError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .create_receive_request(id, amount, label, message)
            .await?
            .into())
    }

    /// Stored requests, newest first (QT-083).
    pub async fn receive_requests(
        &self,
        wallet_id: String,
    ) -> Result<Vec<ReceiveRequest>, ReceiveError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .receive_requests(id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub async fn delete_receive_request(
        &self,
        wallet_id: String,
        id: u64,
    ) -> Result<(), ReceiveError> {
        let wallet = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.delete_receive_request(wallet, id).await?)
    }
}
