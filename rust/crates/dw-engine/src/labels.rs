//! Address book and labels (QT-090, QT-095…098; contract §2.9), stored in
//! dw-appdb per wallet, with dash-qt's `AddressTableModel` /
//! `EditAddressDialog` rules:
//! - a Send entry is someone else's address; one of the wallet's own
//!   addresses cannot be added as one ("already exists as a receiving
//!   address");
//! - a Receive entry labels one of the wallet's own addresses;
//! - adding an address that is already listed is a duplicate unless the
//!   caller asks to replace (relabel) it;
//! - Receive entries can be relabelled but not deleted.

use std::sync::Arc;

use dw_appdb::{BookPurpose, LabelKind};

use crate::coins::{now_secs, wallet_owns};
use crate::send::l1_address;
use crate::{EngineError, NetworkSession, WalletId};

/// Address-book rule violations, one per `labels.*` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LabelsFailure {
    #[error("not an L1 address of this network")]
    InvalidAddress,
    #[error("address already in the address book")]
    DuplicateAddress,
    #[error("address belongs to this wallet")]
    OwnAddress,
    #[error("entry not found")]
    EntryNotFound,
    #[error("receiving entries cannot be deleted")]
    ReceiveEntryNotDeletable,
}

impl From<LabelsFailure> for EngineError {
    fn from(f: LabelsFailure) -> Self {
        EngineError::Labels(f)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookEntryInfo {
    pub address: String,
    /// Empty when unlabelled (dash-qt shows "(no label)").
    pub label: String,
    pub purpose: BookPurpose,
    pub created_at: Option<u64>,
}

/// dash-qt's filter: Qt wildcard pattern (`*`, `?`) matched anywhere in
/// the text, case-insensitively.
pub(crate) fn wildcard_contains(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    // Matches `p` at `t[start..]` with `*` spanning any run.
    fn at(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => true,
            Some(('*', rest)) => (0..=t.len()).any(|i| at(rest, &t[i..])),
            Some((c, rest)) => match t.split_first() {
                Some((tc, trest)) if *c == '?' || c == tc => at(rest, trest),
                _ => false,
            },
        }
    }
    (0..=t.len()).any(|start| at(&p, &t[start..]))
}

/// Txid argument: 64 lowercase hex characters.
fn check_txid(txid: &str) -> Result<(), EngineError> {
    if txid.len() == 64 && txid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        Ok(())
    } else {
        Err(EngineError::InvalidArgument(format!("txid {txid:?} is not 64 lowercase hex")))
    }
}

impl NetworkSession {
    /// Whether `address` (already validated) is one of the wallet's.
    async fn owns(&self, wallet_id: &WalletId, address: &dashcore::Address) -> Result<bool, EngineError> {
        let wallet = self.wallet(wallet_id).await?;
        let state = wallet.state().await;
        Ok(wallet_owns(&state.core_wallet, address))
    }

    /// Entries of `wallet_id`, sorted by label (case-insensitive,
    /// unlabelled last) then address. `search` is a dash-qt wildcard match
    /// on label or address.
    pub async fn address_book(
        self: &Arc<Self>,
        wallet_id: WalletId,
        purpose: Option<BookPurpose>,
        search: Option<String>,
    ) -> Result<Vec<BookEntryInfo>, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            this.wallet(&wallet_id).await?;
            let id = wallet_id.to_string();
            let rows = this.appdb_op(move |db| db.address_book(&id, purpose)).await?;
            let search = search.filter(|s| !s.is_empty());
            Ok(rows
                .into_iter()
                .map(|e| BookEntryInfo {
                    address: e.address,
                    label: e.label.unwrap_or_default(),
                    purpose: e.purpose,
                    created_at: Some(e.created_at),
                })
                .filter(|e| {
                    search.as_deref().is_none_or(|s| {
                        wildcard_contains(s, &e.label) || wildcard_contains(s, &e.address)
                    })
                })
                .collect())
        })
        .await
    }

    /// Adds an entry, or relabels it with `replace`. See the module rules.
    pub async fn save_address_book_entry(
        self: &Arc<Self>,
        wallet_id: WalletId,
        address: String,
        label: String,
        purpose: BookPurpose,
        replace: bool,
    ) -> Result<BookEntryInfo, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let parsed = l1_address(&address, this.network.core_network())
                .map_err(|_| LabelsFailure::InvalidAddress)?;
            // Store the canonical form.
            let address = parsed.to_string();
            let mine = this.owns(&wallet_id, &parsed).await?;
            match (purpose, mine) {
                (BookPurpose::Send, true) => return Err(LabelsFailure::OwnAddress.into()),
                (BookPurpose::Receive, false) => {
                    return Err(EngineError::InvalidArgument(
                        "a receiving entry must be one of the wallet's addresses".into(),
                    ));
                }
                _ => {}
            }
            let id = wallet_id.to_string();
            let entry = this
                .appdb_op(move |db| {
                    if let Some(existing) = db.book_entry(&id, &address)? {
                        if !replace || existing.purpose != purpose {
                            return Ok(Err(LabelsFailure::DuplicateAddress));
                        }
                    }
                    db.upsert_book_entry(&id, &address, purpose, &label, now_secs())?;
                    Ok(Ok(db.book_entry(&id, &address)?))
                })
                .await??;
            let e = entry.ok_or_else(|| EngineError::Storage("entry vanished after save".into()))?;
            Ok(BookEntryInfo {
                address: e.address,
                label: e.label.unwrap_or_default(),
                purpose: e.purpose,
                created_at: Some(e.created_at),
            })
        })
        .await
    }

    /// Deletes a Send entry.
    pub async fn delete_address_book_entry(
        self: &Arc<Self>,
        wallet_id: WalletId,
        address: String,
    ) -> Result<(), EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            this.wallet(&wallet_id).await?;
            let id = wallet_id.to_string();
            let address = l1_address(&address, this.network.core_network())
                .map(|a| a.to_string())
                .unwrap_or(address);
            this.appdb_op(move |db| match db.book_entry(&id, &address)? {
                None => Ok(Err(LabelsFailure::EntryNotFound)),
                Some(e) if e.purpose == BookPurpose::Receive => {
                    Ok(Err(LabelsFailure::ReceiveEntryNotDeletable))
                }
                Some(_) => {
                    db.delete_book_entry(&id, &address)?;
                    Ok(Ok(()))
                }
            })
            .await?
            .map_err(EngineError::from)
        })
        .await
    }

    /// Sets (`Some`, non-empty) or clears the label of a transaction.
    pub async fn set_tx_label(
        self: &Arc<Self>,
        wallet_id: WalletId,
        txid: String,
        label: Option<String>,
    ) -> Result<(), EngineError> {
        check_txid(&txid)?;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            this.wallet(&wallet_id).await?;
            let id = wallet_id.to_string();
            this.appdb_op(move |db| {
                db.set_label(&id, LabelKind::Tx, &txid, label.as_deref(), now_secs())
            })
            .await
        })
        .await
    }

    /// The label of a transaction, if any.
    pub async fn tx_label(
        self: &Arc<Self>,
        wallet_id: WalletId,
        txid: String,
    ) -> Result<Option<String>, EngineError> {
        check_txid(&txid)?;
        let id = wallet_id.to_string();
        self.appdb_op(move |db| db.label(&id, LabelKind::Tx, &txid)).await
    }

    /// The message stored with a sent transaction (QT-054).
    pub async fn tx_message(
        self: &Arc<Self>,
        wallet_id: WalletId,
        txid: String,
    ) -> Result<Option<String>, EngineError> {
        check_txid(&txid)?;
        let id = wallet_id.to_string();
        self.appdb_op(move |db| db.tx_message(&id, &txid)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_search_matches_like_qt() {
        assert!(wildcard_contains("alice", "Alice's shop"));
        assert!(wildcard_contains("SHOP", "alice's shop"));
        assert!(wildcard_contains("a*shop", "Alice's shop"));
        assert!(wildcard_contains("y?ws", "yQWsoTNJ"));
        assert!(!wildcard_contains("bob", "Alice"));
        assert!(wildcard_contains("", "anything"));
        assert!(!wildcard_contains("x?", "x"));
    }

    #[test]
    fn txids_must_be_lowercase_hex() {
        assert!(check_txid(&"ab".repeat(32)).is_ok());
        assert!(check_txid(&"AB".repeat(32)).is_err());
        assert!(check_txid("abcd").is_err());
    }
}
