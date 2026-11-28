//! What earlier builds kept outside the wallet: the PiggyCards account in
//! the OS secure store, under a name derived from the data root, and the
//! orders in the network's `gift-card-orders.json`. [`migrate`] moves both
//! into the wallets' sealed rows once and deletes them.

use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashcore::hashes::sha256;
use dw_engine::{NetworkSession, WalletId};
use dw_vault::OsSecretStore;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

use super::GiftOrder;
use super::store::{self, Account, Change, WalletStore, blocking, storage_error};
use crate::error::AppError;

const PENDING: &str = "piggy-pending";
const ACCOUNT: &str = "piggy-account";
const ACCESS: &str = "piggy-access";
const ORDERS_FILE: &str = "gift-card-orders.json";
const MAX_JOURNAL_SIZE: u64 = 4 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct Secrets {
    service: [u8; 32],
    os: Arc<dyn OsSecretStore>,
}

impl Secrets {
    pub fn new(root: &Path, os: Arc<dyn OsSecretStore>) -> Option<Self> {
        let root = dunce::canonicalize(root).ok()?;
        let mut namespace = b"dashwallet-desktop.piggy-cards.v1\0".to_vec();
        namespace.extend_from_slice(root.as_os_str().as_encoded_bytes());
        let service = sha256::Hash::hash(&namespace).to_byte_array();
        Some(Self { service, os })
    }

    /// `Err(())`: the secure store could not be read.
    async fn load<T: DeserializeOwned + Send + 'static>(
        &self,
        label: &'static str,
    ) -> Result<Option<T>, ()> {
        let this = self.clone();
        let bytes = blocking(move || this.os.get(&this.service, label).map_err(|_| storage_error()))
            .await
            .map_err(drop)?;
        Ok(bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok()))
    }

    async fn delete(&self, label: &'static str) -> Result<(), AppError> {
        let this = self.clone();
        blocking(move || {
            (this.os.delete(&this.service, label))
                .map(drop)
                .map_err(|_| storage_error())
        })
        .await
    }
}

#[cfg(test)]
impl Secrets {
    pub async fn save_for_test<T: serde::Serialize>(&self, label: &'static str, value: &T) {
        self.os
            .put(&self.service, label, &serde_json::to_vec(value).unwrap())
            .unwrap();
    }

    pub async fn is_empty_for_test(&self) -> bool {
        [PENDING, ACCOUNT, ACCESS]
            .iter()
            .all(|label| self.os.get(&self.service, label).unwrap().is_none())
    }
}

#[derive(Deserialize, Serialize)]
struct LegacyOrder {
    view: GiftOrder,
    account: String,
    wallet: String,
}

fn read_orders(path: &Path) -> Result<Vec<LegacyOrder>, AppError> {
    match dw_fs::open_owner_only(path) {
        Ok(file) => {
            let mut text = Vec::new();
            file.take(MAX_JOURNAL_SIZE + 1)
                .read_to_end(&mut text)
                .map_err(|_| storage_error())?;
            if text.len() as u64 > MAX_JOURNAL_SIZE {
                return Err(storage_error());
            }
            serde_json::from_slice(&text).map_err(|_| storage_error())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(storage_error()),
    }
}

/// Replaces the journal with `orders`, as earlier builds wrote it.
fn write_orders(path: &Path, orders: &[LegacyOrder]) -> Result<(), AppError> {
    let dir = path.parent().ok_or_else(storage_error)?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|_| storage_error())?;
    let encoded = serde_json::to_vec(orders).map_err(|_| storage_error())?;
    temp.write_all(&encoded).map_err(|_| storage_error())?;
    temp.as_file().sync_all().map_err(|_| storage_error())?;
    temp.persist(path).map_err(|_| storage_error())?;
    #[cfg(unix)]
    std::fs::File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(|_| storage_error())?;
    Ok(())
}

fn remove_orders(path: &PathBuf) -> Result<(), AppError> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(storage_error()),
    }
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| storage_error())?;
    }
    Ok(())
}

/// Moves the earlier builds' account and orders into the wallets' sealed
/// rows: each order to the wallet it was made from (dropped once the vault
/// holds no seed for that wallet) and the account to every wallet holding
/// one of its orders. An account with no such wallet, and a signup still
/// waiting for its email code, are dropped: the store was shared by every
/// wallet of the data folder, so nothing shows whose they were, and a
/// wallet made after a wipe must not inherit them. The orders of a wallet
/// that is not loaded (closed, its seed still stored) stay in the journal,
/// with the account they need, until a later open finds it loaded. The old
/// copies are deleted only after the rows are written; a run cut short
/// repeats. `false` while something is left (or the OS secure store could
/// not be read): the next open tries again.
pub(super) async fn migrate(
    data_root: &Path,
    os: &Arc<dyn OsSecretStore>,
    session: &Arc<NetworkSession>,
) -> Result<bool, AppError> {
    let Some(secrets) = Secrets::new(data_root, Arc::clone(os)) else {
        return Ok(false);
    };
    let path = session.data_dir().join(ORDERS_FILE);
    let orders = read_orders(&path)?;
    let (Ok(account), Ok(pending), Ok(token)) = (
        secrets.load::<Account>(ACCOUNT).await,
        secrets.load::<serde_json::Value>(PENDING).await,
        secrets.load::<serde_json::Value>(ACCESS).await,
    ) else {
        return Ok(false);
    };
    if orders.is_empty() && account.is_none() && pending.is_none() && token.is_none() {
        remove_orders(&path)?;
        return Ok(true);
    }
    let mut moved: Vec<(WalletId, Vec<store::Order>)> = Vec::new();
    let mut kept: Vec<LegacyOrder> = Vec::new();
    for (index, order) in orders.into_iter().enumerate() {
        let Some(id) = order.wallet.parse::<WalletId>().ok() else {
            continue;
        };
        if !session.vault().has_wallet_secret(&id.0) {
            continue;
        }
        if session.wallet_info(&id).is_err() {
            kept.push(order);
            continue;
        }
        let row = store::Order {
            view: order.view,
            account: order.account,
            created_at: index as u64 + 1,
        };
        match moved.iter_mut().find(|(w, _)| *w == id) {
            Some((_, list)) => list.push(row),
            None => moved.push((id, vec![row])),
        }
    }
    for (id, orders) in &moved {
        let store = WalletStore::new(Arc::clone(session), *id);
        let book = store.read().await?;
        let mut changes = Vec::new();
        let account = account
            .as_ref()
            .filter(|a| orders.iter().any(|o| o.account == a.key()));
        if let Some(account) = account {
            changes.push(Change::Account(account));
            if book.active.is_none() {
                changes.push(Change::Active(Some(account.key())));
            }
        }
        for order in orders {
            if !book.orders.iter().any(|o| o.key() == order.key()) {
                changes.push(Change::Order(order));
            }
        }
        if !changes.is_empty() {
            store.write(&changes).await?;
        }
    }
    secrets.delete(ACCESS).await?;
    secrets.delete(PENDING).await?;
    if !kept.is_empty() {
        write_orders(&path, &kept)?;
        let needed = account
            .as_ref()
            .is_some_and(|a| kept.iter().any(|o| o.account == a.key()));
        if !needed {
            secrets.delete(ACCOUNT).await?;
        }
        return Ok(false);
    }
    secrets.delete(ACCOUNT).await?;
    remove_orders(&path)?;
    Ok(true)
}
