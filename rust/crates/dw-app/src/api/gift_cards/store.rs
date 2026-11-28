//! A wallet's PiggyCards accounts and orders, as rows in the wallet's
//! app.sqlite scope (dw-engine `gift_card_rows`), each value sealed to the
//! wallet's seed (dw-vault `seal_wallet_data`). The wallet's backup carries
//! them and a restore on any computer opens them; removing the wallet,
//! Delete All included, deletes them. Access tokens are never stored: they
//! live in memory ([`super::GiftCards`]) and a restart signs in again with
//! the stored password.

use std::sync::Arc;

use dashcore::hashes::sha256;
use dw_engine::{NetworkSession, WalletId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{GiftOrder, GiftSignup, fail};
use crate::error::AppError;

const ACCOUNT: &str = "giftcards.piggy.account.";
const PENDING: &str = "giftcards.piggy.pending";
const ACTIVE: &str = "giftcards.piggy.active";
const ORDER: &str = "giftcards.piggy.order.";
const ACCOUNT_PURPOSE: &str = "giftcards.piggy.account";
const ORDER_PURPOSE: &str = "giftcards.piggy.order";

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Account {
    pub signup: GiftSignup,
    pub user_id: String,
    pub password: Option<Zeroizing<String>>,
}

impl Account {
    pub fn key(&self) -> String {
        sha256::Hash::hash(self.user_id.as_bytes()).to_string()
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Order {
    pub view: GiftOrder,
    pub account: String,
    #[serde(default)]
    pub created_at: u64,
}

impl Order {
    pub fn key(&self) -> [u8; 32] {
        super::invoice_key(&self.account, &self.view.id)
    }
}

/// Everything stored for one wallet.
#[derive(Default)]
pub(super) struct Book {
    pub accounts: Vec<Account>,
    pub pending: Option<Account>,
    pub active: Option<String>,
    /// Newest first.
    pub orders: Vec<Order>,
}

impl Book {
    pub fn account(&self, key: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.key() == key)
    }

    pub fn active_account(&self) -> Option<&Account> {
        self.account(self.active.as_deref()?)
    }

    pub fn order(&self, id: &str) -> Result<&Order, AppError> {
        let mut found = self.orders.iter().filter(|o| o.view.id == id);
        match (found.next(), found.next()) {
            (Some(order), None) => Ok(order),
            _ => Err(fail("This gift card order is unavailable for this wallet.")),
        }
    }
}

/// One change to a wallet's rows.
pub(super) enum Change<'a> {
    Account(&'a Account),
    Pending(Option<&'a Account>),
    Active(Option<String>),
    Order(&'a Order),
}

pub(super) fn storage_error() -> AppError {
    fail("Could not read or save the gift card orders.")
}

#[derive(Clone)]
pub(super) struct WalletStore {
    session: Arc<NetworkSession>,
    wallet: WalletId,
}

impl WalletStore {
    pub fn new(session: Arc<NetworkSession>, wallet: WalletId) -> Self {
        Self { session, wallet }
    }

    pub fn wallet(&self) -> WalletId {
        self.wallet
    }

    pub async fn read(&self) -> Result<Book, AppError> {
        let rows = (self.session.gift_card_rows(self.wallet).await).map_err(|_| storage_error())?;
        let this = self.clone();
        blocking(move || {
            let mut book = Book::default();
            for (key, value) in rows {
                if key.starts_with(ACCOUNT) {
                    book.accounts.push(this.open(ACCOUNT_PURPOSE, &value)?);
                } else if key == PENDING {
                    book.pending = Some(this.open(ACCOUNT_PURPOSE, &value)?);
                } else if key == ACTIVE {
                    book.active = Some(value);
                } else if key.starts_with(ORDER) {
                    book.orders.push(this.open(ORDER_PURPOSE, &value)?);
                }
            }
            book.orders.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            Ok(book)
        })
        .await
    }

    pub async fn write(&self, changes: &[Change<'_>]) -> Result<(), AppError> {
        let mut plain = Vec::with_capacity(changes.len());
        for change in changes {
            plain.push(match change {
                Change::Account(a) => (
                    format!("{ACCOUNT}{}", a.key()),
                    Some((ACCOUNT_PURPOSE, encode(a)?)),
                    None,
                ),
                Change::Pending(a) => (
                    PENDING.to_owned(),
                    a.map(encode).transpose()?.map(|b| (ACCOUNT_PURPOSE, b)),
                    None,
                ),
                Change::Active(key) => (ACTIVE.to_owned(), None, key.clone()),
                Change::Order(o) => (
                    format!("{ORDER}{}", hex::encode(o.key())),
                    Some((ORDER_PURPOSE, encode(o)?)),
                    None,
                ),
            });
        }
        let this = self.clone();
        let writes = blocking(move || {
            plain
                .into_iter()
                .map(|(key, sealed, clear)| {
                    let value = match sealed {
                        Some((purpose, bytes)) => Some(this.seal(purpose, &bytes)?),
                        None => clear,
                    };
                    Ok((this.wallet, key, value))
                })
                .collect::<Result<Vec<_>, AppError>>()
        })
        .await?;
        (self.session.write_gift_card_rows(writes).await).map_err(|_| storage_error())
    }

    fn seal(&self, purpose: &str, bytes: &[u8]) -> Result<String, AppError> {
        let sealed = (self.session.vault())
            .seal_wallet_data(&self.wallet.0, purpose, bytes)
            .map_err(|_| locked_error())?;
        String::from_utf8(sealed).map_err(|_| storage_error())
    }

    fn open<T: DeserializeOwned>(&self, purpose: &str, value: &str) -> Result<T, AppError> {
        let bytes = (self.session.vault())
            .open_wallet_data(&self.wallet.0, purpose, value.as_bytes())
            .map_err(|e| match e {
                dw_vault::VaultError::Corrupt(_) => storage_error(),
                _ => locked_error(),
            })?;
        serde_json::from_slice(&bytes).map_err(|_| storage_error())
    }
}

fn encode<T: Serialize>(value: &T) -> Result<Zeroizing<Vec<u8>>, AppError> {
    serde_json::to_vec(value)
        .map(Zeroizing::new)
        .map_err(|_| storage_error())
}

fn locked_error() -> AppError {
    fail("Unlock the wallet to use gift cards.")
}

pub(super) async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|_| storage_error())?
}
