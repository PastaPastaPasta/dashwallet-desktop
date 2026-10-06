//! Address book and labels (QT-095..098). Owner: E2; stored in dw-appdb.
//! Contract: docs/contracts/m1-engine.md §labels.

use crate::NetworkSession;
use crate::api::common::parse_wallet_id;

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

impl crate::api::common::NotImplementedError for LabelsError {
    fn not_implemented(call: &'static str) -> Self {
        Self::NotImplemented {
            call: call.to_string(),
        }
    }
}

impl From<dw_engine::EngineError> for LabelsError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        use dw_engine::LabelsFailure as F;
        let detail = e.to_string();
        match e {
            E::Labels(F::InvalidAddress) => Self::InvalidAddress,
            E::Labels(F::DuplicateAddress) => Self::DuplicateAddress,
            E::Labels(F::OwnAddress) => Self::OwnAddress,
            E::Labels(F::EntryNotFound) => Self::EntryNotFound,
            E::Labels(F::ReceiveEntryNotDeletable) => Self::ReceiveEntryNotDeletable,
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(_) => Self::NotImplemented { call: detail },
            _ => Self::Internal { detail },
        }
    }
}

impl From<AddressPurpose> for dw_engine::BookPurpose {
    fn from(p: AddressPurpose) -> Self {
        match p {
            AddressPurpose::Send => Self::Send,
            AddressPurpose::Receive => Self::Receive,
        }
    }
}

impl From<dw_engine::BookEntryInfo> for AddressBookEntry {
    fn from(e: dw_engine::BookEntryInfo) -> Self {
        Self {
            address: e.address,
            label: e.label,
            purpose: match e.purpose {
                dw_engine::BookPurpose::Send => AddressPurpose::Send,
                dw_engine::BookPurpose::Receive => AddressPurpose::Receive,
            },
            created_at: e.created_at,
        }
    }
}

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
    /// Address-book entries of `wallet_id`, sorted by label then address.
    /// `search` is dash-qt's case-insensitive wildcard match (`*`, `?`) on
    /// label or address.
    pub async fn address_book(
        &self,
        wallet_id: String,
        purpose: Option<AddressPurpose>,
        search: Option<String>,
    ) -> Result<Vec<AddressBookEntry>, LabelsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .address_book(id, purpose.map(Into::into), search)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    /// Adds or relabels an entry. A Send entry for one of the wallet's own
    /// addresses is `OwnAddress`; a Receive entry must be one of them
    /// (`invalid_argument` otherwise). An address already in the book is
    /// `DuplicateAddress` unless `replace` is set with the same purpose.
    pub async fn save_address_book_entry(
        &self,
        wallet_id: String,
        address: String,
        label: String,
        purpose: AddressPurpose,
        replace: bool,
    ) -> Result<AddressBookEntry, LabelsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .save_address_book_entry(id, address, label, purpose.into(), replace)
            .await?
            .into())
    }

    /// Deletes a Send entry.
    pub async fn delete_address_book_entry(
        &self,
        wallet_id: String,
        address: String,
    ) -> Result<(), LabelsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.delete_address_book_entry(id, address).await?)
    }

    /// Sets or clears (`None` or empty) the label of a transaction.
    pub async fn set_tx_label(
        &self,
        wallet_id: String,
        txid: String,
        label: Option<String>,
    ) -> Result<(), LabelsError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.set_tx_label(id, txid, label).await?)
    }
}
