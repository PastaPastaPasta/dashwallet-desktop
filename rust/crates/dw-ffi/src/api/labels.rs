//! Address book and labels (QT-095..098). Owner: E2; stored in dw-appdb.
//! Contract: docs/contracts/m1-engine.md §labels.

use crate::NetworkSession;
use crate::api::common::{domain_error_common, not_implemented, parse_wallet_id};

/// dash-qt address-book purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AddressPurpose {
    /// Someone else's address (sending tab).
    Send,
    /// One of this wallet's addresses (receiving tab).
    Receive,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AddressBookEntry {
    pub address: String,
    pub label: String,
    pub purpose: AddressPurpose,
    /// UNIX seconds.
    pub created_at: Option<u64>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LabelsError {
    /// Code `labels.invalid_address`: not an L1 address on this network.
    #[error("invalid address")]
    InvalidAddress,
    /// Code `labels.duplicate_address`: already in the book (QT-098).
    #[error("address already in the address book")]
    DuplicateAddress,
    /// Code `labels.own_address`: a Send entry for one of the wallet's addresses.
    #[error("address belongs to this wallet")]
    OwnAddress,
    /// Code `labels.entry_not_found`.
    #[error("entry not found")]
    EntryNotFound,
    /// Code `labels.receive_entry_not_deletable`: receiving entries can be
    /// relabelled but not deleted (dash-qt rule).
    #[error("receiving entries cannot be deleted")]
    ReceiveEntryNotDeletable,
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

domain_error_common!(LabelsError);

impl LabelsError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidAddress => "labels.invalid_address",
            Self::DuplicateAddress => "labels.duplicate_address",
            Self::OwnAddress => "labels.own_address",
            Self::EntryNotFound => "labels.entry_not_found",
            Self::ReceiveEntryNotDeletable => "labels.receive_entry_not_deletable",
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
    /// Address-book entries of `wallet_id`. `search` matches label or
    /// address, case-insensitively (dash-qt wildcard search).
    pub async fn address_book(
        &self,
        wallet_id: String,
        purpose: Option<AddressPurpose>,
        search: Option<String>,
    ) -> Result<Vec<AddressBookEntry>, LabelsError> {
        let _ = (parse_wallet_id(&wallet_id)?, purpose, search);
        not_implemented("NetworkSession.address_book")
    }

    /// Adds or relabels an entry. Adding an existing Send address returns
    /// `DuplicateAddress`; relabel by passing the same address and purpose
    /// with `replace = true`.
    pub async fn save_address_book_entry(
        &self,
        wallet_id: String,
        address: String,
        label: String,
        purpose: AddressPurpose,
        replace: bool,
    ) -> Result<AddressBookEntry, LabelsError> {
        let _ = (
            parse_wallet_id(&wallet_id)?,
            address,
            label,
            purpose,
            replace,
        );
        not_implemented("NetworkSession.save_address_book_entry")
    }

    /// Deletes a Send entry.
    pub async fn delete_address_book_entry(
        &self,
        wallet_id: String,
        address: String,
    ) -> Result<(), LabelsError> {
        let _ = (parse_wallet_id(&wallet_id)?, address);
        not_implemented("NetworkSession.delete_address_book_entry")
    }

    /// Sets or clears (`None`) the label of a transaction.
    pub async fn set_tx_label(
        &self,
        wallet_id: String,
        txid: String,
        label: Option<String>,
    ) -> Result<(), LabelsError> {
        let _ = (parse_wallet_id(&wallet_id)?, txid, label);
        not_implemented("NetworkSession.set_tx_label")
    }
}
