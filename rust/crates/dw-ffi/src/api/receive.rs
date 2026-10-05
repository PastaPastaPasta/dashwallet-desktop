//! Receive addresses, address list and payment requests. Owner: E1
//! (requests are stored in dw-appdb, owned by E2).
//! Contract: docs/contracts/m1-engine.md §receive.

use crate::NetworkSession;
use crate::api::common::{domain_error_common, not_implemented, parse_wallet_id};

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

domain_error_common!(ReceiveError);

impl ReceiveError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
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

#[uniffi::export]
impl NetworkSession {
    /// The first unused receiving address (IOS-053). Hosts re-query after
    /// `HistoryChanged`, which is how the shown address rotates once paid.
    pub async fn current_receive_address(
        &self,
        wallet_id: String,
    ) -> Result<AddressInfo, ReceiveError> {
        let _ = parse_wallet_id(&wallet_id)?;
        not_implemented("NetworkSession.current_receive_address")
    }

    /// Issues the next unused receiving address and labels it (dash-qt
    /// "Request payment" always uses a fresh address, QT-081).
    pub async fn next_receive_address(
        &self,
        wallet_id: String,
        label: Option<String>,
    ) -> Result<AddressInfo, ReceiveError> {
        let _ = (parse_wallet_id(&wallet_id)?, label);
        not_implemented("NetworkSession.next_receive_address")
    }

    /// Addresses of the wallet's standard BIP44 account (QT-096 receiving tab).
    pub async fn addresses(
        &self,
        wallet_id: String,
        filter: AddressFilter,
    ) -> Result<Vec<AddressInfo>, ReceiveError> {
        let _ = (parse_wallet_id(&wallet_id)?, filter);
        not_implemented("NetworkSession.addresses")
    }

    /// Stores a payment request on a fresh address and returns it.
    pub async fn create_receive_request(
        &self,
        wallet_id: String,
        amount: Option<u64>,
        label: Option<String>,
        message: Option<String>,
    ) -> Result<ReceiveRequest, ReceiveError> {
        let _ = (parse_wallet_id(&wallet_id)?, amount, label, message);
        not_implemented("NetworkSession.create_receive_request")
    }

    /// Stored requests, newest first (QT-083).
    pub async fn receive_requests(
        &self,
        wallet_id: String,
    ) -> Result<Vec<ReceiveRequest>, ReceiveError> {
        let _ = parse_wallet_id(&wallet_id)?;
        not_implemented("NetworkSession.receive_requests")
    }

    pub async fn delete_receive_request(
        &self,
        wallet_id: String,
        id: u64,
    ) -> Result<(), ReceiveError> {
        let _ = (parse_wallet_id(&wallet_id)?, id);
        not_implemented("NetworkSession.delete_receive_request")
    }
}
