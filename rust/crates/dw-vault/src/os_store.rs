//! Slot O backend: the OS secret store that holds the data key of an
//! unencrypted vault (DESIGN-opus §1.8).
//!
//! [`KeyringOsStore`] goes through platform-wallet-storage's `SecretStore::Os`
//! (keyring-core backends: macOS Keychain, Linux Secret Service, Windows
//! credential manager). It fails closed: without a reachable store (headless
//! Linux, no D-Bus session) every call returns
//! [`VaultError::OsStoreUnavailable`], never a weaker fallback.
//! [`MemoryOsStore`] is an in-process fake for tests and fixture mode.

use std::collections::HashMap;
use std::sync::Mutex;

use platform_wallet_storage::secrets::{SecretBytes, SecretStore, WalletId};
use zeroize::Zeroizing;

use crate::VaultError;

/// Secret storage outside the vault file, addressed by a 32-byte service id
/// and a short label (`^[A-Za-z0-9._-]{1,64}$`).
pub trait OsSecretStore: Send + Sync {
    /// Stores `secret`, replacing any previous value.
    fn put(&self, service: &[u8; 32], label: &str, secret: &[u8]) -> Result<(), VaultError>;
    /// The stored value, or `None` when there is none.
    fn get(
        &self,
        service: &[u8; 32],
        label: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError>;
    /// Deletes the value. `Ok(false)` when there was none.
    fn delete(&self, service: &[u8; 32], label: &str) -> Result<bool, VaultError>;
    /// Short backend name for logs.
    fn name(&self) -> &'static str;
}

/// The platform keyring through platform-wallet-storage. The store is opened
/// on first use and reopened after a failure, so a Secret Service that comes
/// up later is picked up without restarting.
#[derive(Default)]
pub struct KeyringOsStore {
    store: Mutex<Option<SecretStore>>,
}

impl KeyringOsStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_store<T>(
        &self,
        f: impl FnOnce(&SecretStore) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| VaultError::Internal("keyring store lock poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(SecretStore::os().map_err(unavailable)?);
        }
        let store = guard
            .as_ref()
            .ok_or_else(|| VaultError::Internal("keyring store".into()))?;
        let result = f(store);
        if result.is_err() {
            // Drop the handle so the next call re-opens the backend.
            *guard = None;
        }
        result
    }
}

fn unavailable(e: impl std::fmt::Display) -> VaultError {
    VaultError::OsStoreUnavailable(e.to_string())
}

impl OsSecretStore for KeyringOsStore {
    fn put(&self, service: &[u8; 32], label: &str, secret: &[u8]) -> Result<(), VaultError> {
        let value = SecretBytes::from_slice(secret);
        self.with_store(|s| {
            s.set(&WalletId(*service), label, &value)
                .map_err(unavailable)
        })
    }

    fn get(
        &self,
        service: &[u8; 32],
        label: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        self.with_store(|s| {
            Ok(s.get(&WalletId(*service), label)
                .map_err(unavailable)?
                .map(|v| Zeroizing::new(v.expose_secret().to_vec())))
        })
    }

    fn delete(&self, service: &[u8; 32], label: &str) -> Result<bool, VaultError> {
        self.with_store(|s| s.delete(&WalletId(*service), label).map_err(unavailable))
    }

    fn name(&self) -> &'static str {
        "os-keyring"
    }
}

/// In-memory [`OsSecretStore`] for tests and fixture mode. Values live as long
/// as the store object; share it through an `Arc` to model "the same OS
/// store across an app restart".
/// Values of [`MemoryOsStore`], keyed by (service, label).
type MemoryEntries = HashMap<([u8; 32], String), Zeroizing<Vec<u8>>>;

#[derive(Default)]
pub struct MemoryOsStore {
    entries: Mutex<MemoryEntries>,
    unavailable: bool,
}

impl MemoryOsStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// A store that refuses every call, like a Linux session without a
    /// Secret Service.
    pub fn unavailable() -> Self {
        Self {
            entries: Mutex::default(),
            unavailable: true,
        }
    }

    /// Number of stored values.
    pub fn len(&self) -> usize {
        self.entries.lock().map(|e| e.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn entries(&self) -> Result<std::sync::MutexGuard<'_, MemoryEntries>, VaultError> {
        if self.unavailable {
            return Err(VaultError::OsStoreUnavailable(
                "no default credential store".into(),
            ));
        }
        self.entries
            .lock()
            .map_err(|_| VaultError::Internal("memory store lock poisoned".into()))
    }
}

impl OsSecretStore for MemoryOsStore {
    fn put(&self, service: &[u8; 32], label: &str, secret: &[u8]) -> Result<(), VaultError> {
        self.entries()?.insert(
            (*service, label.to_owned()),
            Zeroizing::new(secret.to_vec()),
        );
        Ok(())
    }

    fn get(
        &self,
        service: &[u8; 32],
        label: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        Ok(self
            .entries()?
            .get(&(*service, label.to_owned()))
            .map(|v| Zeroizing::new(v.to_vec())))
    }

    fn delete(&self, service: &[u8; 32], label: &str) -> Result<bool, VaultError> {
        Ok(self
            .entries()?
            .remove(&(*service, label.to_owned()))
            .is_some())
    }

    fn name(&self) -> &'static str {
        "memory"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let s = MemoryOsStore::new();
        assert!(s.get(&[1; 32], "dek").unwrap().is_none());
        s.put(&[1; 32], "dek", b"abc").unwrap();
        assert_eq!(&s.get(&[1; 32], "dek").unwrap().unwrap()[..], b"abc");
        assert!(s.delete(&[1; 32], "dek").unwrap());
        assert!(!s.delete(&[1; 32], "dek").unwrap());
    }

    #[test]
    fn unavailable_store_is_a_typed_error() {
        let s = MemoryOsStore::unavailable();
        assert!(matches!(
            s.put(&[1; 32], "dek", b"x"),
            Err(VaultError::OsStoreUnavailable(_))
        ));
    }

    /// Talks to the real OS keyring. Ignored by default: on macOS the
    /// keychain may prompt, and CI Linux has no Secret Service. Run with
    /// `cargo test -p dw-vault -- --ignored keyring`.
    #[test]
    #[ignore]
    fn keyring_round_trip_or_typed_unavailable() {
        let s = KeyringOsStore::new();
        let service = crate::crypto::random_array::<32>().unwrap();
        match s.put(&service, "dw-test", b"value") {
            Ok(()) => {
                assert_eq!(&s.get(&service, "dw-test").unwrap().unwrap()[..], b"value");
                assert!(s.delete(&service, "dw-test").unwrap());
            }
            Err(VaultError::OsStoreUnavailable(detail)) => {
                eprintln!("OS store unavailable here: {detail}");
            }
            Err(other) => panic!("unexpected error {other:?}"),
        }
    }
}
