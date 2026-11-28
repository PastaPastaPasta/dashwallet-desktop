//! App data sealed to one wallet: a feature's rows that live in app.sqlite
//! and travel in the wallet's backup, readable only with the wallet's seed.
//!
//! The key is HKDF-SHA256 of the seed, with the purpose as its info, so a
//! wallet restored from a backup on another computer opens its own rows,
//! and no other wallet does. The AEAD's associated data binds the
//! ciphertext to the wallet id and the purpose. Sealing and opening need
//! the full-scope data key (unlocked or unencrypted); neither the seed nor
//! the derived key leaves this module.

use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{REC_SEED, Vault, decode_seed, record_id};
use crate::VaultError;
use crate::crypto::{self, Sealed};
use crate::types::WalletId;

const SALT: &[u8] = b"dashwallet-desktop:wallet-data:v1";
const MAX_PURPOSE: usize = 64;

fn aad(wallet: &WalletId, purpose: &str) -> Vec<u8> {
    let mut aad = b"dwd-wallet-data\0".to_vec();
    aad.extend_from_slice(wallet);
    aad.extend_from_slice(purpose.as_bytes());
    aad
}

fn checked(purpose: &str) -> Result<(), VaultError> {
    if purpose.is_empty()
        || purpose.len() > MAX_PURPOSE
        || !purpose
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
    {
        return Err(VaultError::InvalidArgument("wallet data purpose".into()));
    }
    Ok(())
}

impl Vault {
    fn with_wallet_data_key<T>(
        &self,
        wallet: &WalletId,
        purpose: &str,
        body: impl Fn(&[u8; 32]) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        checked(purpose)?;
        self.with_loaded_key(|dek| {
            let payload = self
                .read_record(dek, &record_id(wallet, REC_SEED))?
                .ok_or(VaultError::NoSecret)?;
            let (seed, _) = decode_seed(&payload)?;
            let mut key = Zeroizing::new([0u8; 32]);
            Hkdf::<Sha256>::new(Some(SALT), &seed[..])
                .expand(purpose.as_bytes(), &mut key[..])
                .map_err(|_| VaultError::Internal("hkdf".into()))?;
            body(&key)
        })
    }

    /// Seals `plaintext` to `wallet` for `purpose` (ASCII letters, digits,
    /// `.` and `-`). `NoSecret` for a watch-only wallet.
    pub fn seal_wallet_data(
        &self,
        wallet: &WalletId,
        purpose: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, VaultError> {
        let aad = aad(wallet, purpose);
        let sealed =
            self.with_wallet_data_key(wallet, purpose, |key| crypto::seal(key, plaintext, &aad))?;
        serde_json::to_vec(&sealed).map_err(|e| VaultError::Internal(e.to_string()))
    }

    /// Opens what [`Self::seal_wallet_data`] sealed for the same wallet and
    /// purpose; `Corrupt` when it does not authenticate.
    pub fn open_wallet_data(
        &self,
        wallet: &WalletId,
        purpose: &str,
        sealed: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let sealed: Sealed = serde_json::from_slice(sealed)
            .map_err(|_| VaultError::Corrupt("wallet data".into()))?;
        let aad = aad(wallet, purpose);
        self.with_wallet_data_key(wallet, purpose, |key| {
            crypto::open(key, &sealed, &aad)
                .ok_or_else(|| VaultError::Corrupt("wallet data".into()))
        })
    }
}
