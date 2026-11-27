use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashcore::hashes::sha256;
use dw_vault::OsSecretStore;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{GiftOrder, GiftSignup, fail};
use crate::error::AppError;

#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub(super) struct Token {
    pub account: String,
    pub access: Zeroizing<String>,
    pub expires_at: u64,
}

#[derive(Clone)]
pub(super) struct Secrets {
    service: [u8; 32],
    os: Arc<dyn OsSecretStore>,
}

impl Secrets {
    pub fn new(root: &Path, os: Arc<dyn OsSecretStore>) -> Result<Self, AppError> {
        let root = dunce::canonicalize(root).map_err(|_| storage_error())?;
        let mut namespace = b"dashwallet-desktop.piggy-cards.v1\0".to_vec();
        namespace.extend_from_slice(root.as_os_str().as_encoded_bytes());
        let service = sha256::Hash::hash(&namespace).to_byte_array();
        Ok(Self { service, os })
    }

    pub async fn load<T: DeserializeOwned + Send + 'static>(
        &self,
        label: &'static str,
    ) -> Result<Option<T>, AppError> {
        let this = self.clone();
        blocking(move || {
            let bytes = this
                .os
                .get(&this.service, label)
                .map_err(|_| keyring_error())?;
            bytes
                .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| keyring_error()))
                .transpose()
        })
        .await
    }

    pub async fn save<T: Serialize>(&self, label: &'static str, value: &T) -> Result<(), AppError> {
        let bytes = Zeroizing::new(serde_json::to_vec(value).map_err(|_| keyring_error())?);
        let this = self.clone();
        blocking(move || {
            this.os
                .put(&this.service, label, &bytes)
                .map_err(|_| keyring_error())
        })
        .await
    }

    pub async fn delete(&self, label: &'static str) -> Result<(), AppError> {
        let this = self.clone();
        blocking(move || {
            this.os
                .delete(&this.service, label)
                .map(drop)
                .map_err(|_| keyring_error())
        })
        .await
    }
}

async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|_| keyring_error())?
}

fn keyring_error() -> AppError {
    fail("The OS secure store is unavailable. Unlock or enable it before using gift cards.")
}

fn storage_error() -> AppError {
    fail("Could not read or save the gift card orders.")
}

#[derive(Serialize, Deserialize)]
pub(super) struct Order {
    pub view: GiftOrder,
    pub account: String,
    pub wallet: String,
}

const MAX_JOURNAL_SIZE: usize = 4 * 1024 * 1024;

pub(super) struct Orders {
    path: PathBuf,
    pub entries: Vec<Order>,
}

impl Orders {
    pub fn open(root: &Path) -> Result<Self, AppError> {
        let path = root.join("gift-card-orders.json");
        let entries = match dw_fs::open_owner_only(&path) {
            Ok(file) => {
                let mut text = Vec::new();
                file.take(MAX_JOURNAL_SIZE as u64 + 1)
                    .read_to_end(&mut text)
                    .map_err(|_| storage_error())?;
                if text.len() > MAX_JOURNAL_SIZE {
                    return Err(storage_error());
                }
                serde_json::from_slice(&text).map_err(|_| storage_error())?
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err(storage_error()),
        };
        Ok(Self { path, entries })
    }

    pub fn save(&self) -> Result<(), AppError> {
        let dir = self.path.parent().ok_or_else(storage_error)?;
        let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|_| storage_error())?;
        let encoded = serde_json::to_vec(&self.entries).map_err(|_| storage_error())?;
        if encoded.len() > MAX_JOURNAL_SIZE {
            return Err(storage_error());
        }
        temp.write_all(&encoded).map_err(|_| storage_error())?;
        temp.flush().map_err(|_| storage_error())?;
        temp.as_file().sync_all().map_err(|_| storage_error())?;
        temp.persist(&self.path).map_err(|_| storage_error())?;
        #[cfg(unix)]
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| storage_error())?;
        Ok(())
    }

    pub fn find(&mut self, id: &str, account: &str, wallet: &str) -> Result<&mut Order, AppError> {
        self.entries
            .iter_mut()
            .find(|o| o.view.id == id && o.account == account && o.wallet == wallet)
            .ok_or_else(|| {
                fail("This gift card order is unavailable for the current account and wallet.")
            })
    }
}
