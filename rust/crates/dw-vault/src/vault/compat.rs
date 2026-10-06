//! Vault operations of the dash-qt compatibility features (R2, M2):
//! exporting a wallet's secrets for dash-qt (QT-109), checking whether its
//! phrase restores the same wallet in Dash Core, and the key material of
//! `.dwbackup` bundles (QT-110, QT-116; docs/contracts/dwbackup-v1.md).
//!
//! A backup bundle carries the wallet's vault records exactly as they are
//! stored (sealed under the vault's data key, AAD bound to this vault's id
//! and network), a slot that unwraps that data key, and the rest of the
//! backup (app metadata, wallet state) sealed under a key derived from the
//! data key. Restoring opens the records with the unwrapped key and stores
//! them again under the restoring vault's own key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use super::{REC_MNEMONIC, REC_PASSPHRASE, REC_SEED, Vault, decode_seed, record_id};
use crate::VaultError;
use crate::crypto::{self, KdfParams, Key32, SALT_LEN, Sealed, hex_bytes};
use crate::file;
use crate::types::{GrantKind, GrantToken, LockState, SeedDerivation, WalletId, WalletSecret};

/// Dash Core's seed of `phrase` + `passphrase` (`CMnemonic::ToSeed`), or
/// `None` when Core's checksum (`CMnemonic::Check`) refuses the phrase.
fn bip39_core_seed(phrase: &[u8], passphrase: &[u8]) -> Option<Zeroizing<[u8; 64]>> {
    use dw_compat::bip39core;
    bip39core::core_check(phrase).then(|| bip39core::core_seed(phrase, passphrase))
}

/// Bundle format version (docs/contracts/dwbackup-v1.md).
pub const BUNDLE_VERSION: u32 = 1;

/// How a bundle's data key is recovered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BackupKeySlot {
    /// A copy of the vault's passphrase slot: the vault passphrase at backup
    /// time unwraps the data key.
    VaultPassphrase {
        kdf: KdfParams,
        #[serde(with = "hex_bytes")]
        salt: Vec<u8>,
        wrapped_dek: Sealed,
    },
    /// A slot of its own for an unencrypted vault: the backup passphrase
    /// unwraps the data key.
    BackupPassphrase {
        kdf: KdfParams,
        #[serde(with = "hex_bytes")]
        salt: Vec<u8>,
        wrapped_dek: Sealed,
    },
    /// No wrapped key: only the vault the bundle came from (same vault id,
    /// data key available) opens it. Automatic backups of an unencrypted
    /// vault, which has no passphrase to wrap with.
    VaultKey,
}

/// One wallet's backup key material and sealed contents. Serialized as JSON
/// inside a `.dwbackup` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalletBackupBundle {
    pub(crate) version: u32,
    #[serde(with = "hex_bytes")]
    pub(crate) vault_id: Vec<u8>,
    /// Network tag of the source vault (bound into every AAD).
    pub(crate) network: String,
    #[serde(with = "hex_bytes")]
    pub(crate) wallet_id: Vec<u8>,
    pub(crate) slot: BackupKeySlot,
    /// `mnemonic` / `mnemonic_passphrase` / `seed` → the vault record as
    /// stored.
    pub(crate) records: BTreeMap<String, Sealed>,
    #[serde(with = "hex_bytes")]
    pub(crate) payload_salt: Vec<u8>,
    /// The caller's payload, sealed under the payload key; AAD = the file
    /// header the caller passes.
    pub(crate) payload: Sealed,
}

impl WalletBackupBundle {
    /// The wallet the bundle holds.
    pub fn wallet_id(&self) -> Option<WalletId> {
        self.wallet_id.as_slice().try_into().ok()
    }

    /// Network tag of the vault that wrote it.
    pub fn network(&self) -> &str {
        &self.network
    }

    /// Whether opening it needs a passphrase (unless this is the vault that
    /// wrote it and its key is available).
    pub fn has_passphrase_slot(&self) -> bool {
        !matches!(self.slot, BackupKeySlot::VaultKey)
    }
}

/// What [`Vault::core_mnemonic_check`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreMnemonicCheck {
    /// The wallet has a phrase at all.
    pub has_mnemonic: bool,
    /// Dash Core's `CMnemonic::ToSeed` of the stored phrase and passphrase is
    /// the wallet's seed, and Core's checksum accepts the phrase: phrase +
    /// passphrase + `upgradetohd` rebuild this wallet in dash-qt.
    pub core_compatible: bool,
}

fn slot_aad(kind: &[u8], vault_id: &[u8], network: &str, kdf: &KdfParams, salt: &[u8]) -> Vec<u8> {
    let mut a = kind.to_vec();
    for field in [vault_id, network.as_bytes(), &kdf.aad_bytes()[..], salt] {
        a.extend_from_slice(&(field.len() as u32).to_le_bytes());
        a.extend_from_slice(field);
    }
    a
}

const BACKUP_SLOT_AAD: &[u8] = b"dw-vault/backup-slot/v1";

/// Payload key: SHA-256 over a domain tag, the data key and the bundle's
/// random salt. The data key is uniformly random, so a hash is a sound KDF.
fn payload_key(dek: &[u8; 32], salt: &[u8]) -> Key32 {
    let mut h = Sha256::new();
    h.update(b"dw-vault/backup-payload/v1");
    h.update(dek);
    h.update(salt);
    Zeroizing::new(h.finalize().into())
}

fn bundle_payload_aad(bundle_wallet: &[u8], vault_id: &[u8], header: &[u8]) -> Vec<u8> {
    let mut a = b"dw-vault/backup-payload/v1".to_vec();
    for field in [bundle_wallet, vault_id, header] {
        a.extend_from_slice(&(field.len() as u32).to_le_bytes());
        a.extend_from_slice(field);
    }
    a
}

impl Vault {
    /// Whether the vault holds a recovery phrase for `wallet` (in-memory).
    pub fn has_wallet_mnemonic(&self, wallet: &WalletId) -> bool {
        self.inner()
            .file
            .as_ref()
            .is_some_and(|f| f.records.contains_key(&record_id(wallet, REC_MNEMONIC)))
    }

    /// Reads every secret of `wallet` for an export to dash-qt (QT-109). The
    /// token must be a redeemed `RevealSecret` grant for this wallet; a
    /// passphrase grant on a locked vault reads with its own key.
    pub fn export_wallet_secret(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<WalletSecret, VaultError> {
        if token.purpose.kind() != GrantKind::RevealSecret || token.wallet.as_ref() != Some(wallet)
        {
            return Err(VaultError::GrantPurposeMismatch);
        }
        let dek = self.key_for(token)?;
        self.read_secret(&dek, wallet)
    }

    fn read_secret(&self, dek: &Key32, wallet: &WalletId) -> Result<WalletSecret, VaultError> {
        let payload = self
            .read_record(dek, &record_id(wallet, REC_SEED))?
            .ok_or(VaultError::NoSecret)?;
        let (seed, derivation) = decode_seed(&payload)?;
        let mnemonic = self
            .read_record(dek, &record_id(wallet, REC_MNEMONIC))?
            .unwrap_or_default();
        let mnemonic_passphrase = self
            .read_record(dek, &record_id(wallet, REC_PASSPHRASE))?
            .unwrap_or_default();
        Ok(WalletSecret {
            mnemonic,
            mnemonic_passphrase,
            seed,
            derivation,
        })
    }

    /// Whether "phrase + passphrase + `upgradetohd`" in Dash Core rebuilds
    /// `wallet`: Core's checksum accepts the phrase and Core's seed of it is
    /// the stored seed. Reads the secrets internally (full data key needed:
    /// unlocked or unencrypted vault); returns only the verdict.
    pub fn core_mnemonic_check(&self, wallet: &WalletId) -> Result<CoreMnemonicCheck, VaultError> {
        let dek = self.full_dek()?;
        let secret = self.read_secret(&dek, wallet)?;
        if secret.derivation == SeedDerivation::RawSeed || secret.mnemonic.is_empty() {
            return Ok(CoreMnemonicCheck {
                has_mnemonic: false,
                core_compatible: false,
            });
        }
        let core_compatible = match bip39_core_seed(&secret.mnemonic, &secret.mnemonic_passphrase) {
            Some(core) => bool::from(core[..].ct_eq(&secret.seed[..])),
            None => false,
        };
        Ok(CoreMnemonicCheck {
            has_mnemonic: true,
            core_compatible,
        })
    }

    /// Builds the backup bundle of `wallet`, sealing `payload` with
    /// AAD = `header`. Needs the full data key (vault unlocked, or
    /// unencrypted).
    ///
    /// - Encrypted vault: the slot is a copy of the vault's passphrase slot;
    ///   `backup_passphrase` must be `None`.
    /// - Unencrypted vault with `backup_passphrase`: a new Argon2id slot
    ///   under the vault's KDF policy wraps the data key.
    /// - Unencrypted vault without one: [`BackupKeySlot::VaultKey`]; only this
    ///   vault can open the bundle (automatic backups).
    pub fn backup_bundle(
        &self,
        wallet: &WalletId,
        backup_passphrase: Option<&[u8]>,
        payload: &[u8],
        header: &[u8],
    ) -> Result<WalletBackupBundle, VaultError> {
        let dek = self.full_dek()?;
        let (vault_id, network, slot_p, records) = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let mut records = BTreeMap::new();
            for kind in [REC_MNEMONIC, REC_PASSPHRASE, REC_SEED] {
                if let Some(sealed) = f.records.get(&record_id(wallet, kind)) {
                    records.insert(kind.to_owned(), sealed.clone());
                }
            }
            (
                f.vault_id.clone(),
                f.network.clone(),
                f.slot_p.clone(),
                records,
            )
        };
        if !records.contains_key(REC_SEED) {
            return Err(VaultError::NoSecret);
        }
        let slot = match (slot_p, backup_passphrase) {
            (Some(_), Some(_)) => {
                return Err(VaultError::InvalidArgument(
                    "an encrypted vault's backups use the vault passphrase".into(),
                ));
            }
            (Some(p), None) => BackupKeySlot::VaultPassphrase {
                kdf: p.kdf,
                salt: p.salt,
                wrapped_dek: p.wrapped_dek,
            },
            (None, Some(pw)) => {
                let pw = super::new_passphrase(pw)?;
                let salt: [u8; SALT_LEN] = crypto::random_array()?;
                let (kdf, kek) = crypto::derive_new_kek(
                    &pw,
                    &salt,
                    self.shared.config.kdf,
                    self.production_network(),
                )?;
                let aad = slot_aad(BACKUP_SLOT_AAD, &vault_id, &network, &kdf, &salt);
                BackupKeySlot::BackupPassphrase {
                    kdf,
                    salt: salt.to_vec(),
                    wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
                }
            }
            (None, None) => BackupKeySlot::VaultKey,
        };
        let payload_salt: [u8; SALT_LEN] = crypto::random_array()?;
        let key = payload_key(&dek, &payload_salt);
        let sealed = crypto::seal(
            &key,
            payload,
            &bundle_payload_aad(wallet, &vault_id, header),
        )?;
        Ok(WalletBackupBundle {
            version: BUNDLE_VERSION,
            vault_id,
            network,
            wallet_id: wallet.to_vec(),
            slot,
            records,
            payload_salt: payload_salt.to_vec(),
            payload: sealed,
        })
    }

    /// Opens a bundle: recovers its data key (this vault's own key when the
    /// bundle came from this vault and the key is available, otherwise the
    /// slot with `passphrase`), then the wallet's secrets and the payload.
    /// Stores nothing.
    ///
    /// Errors: `NotEncrypted` (a passphrase slot and no passphrase given —
    /// the caller's "passphrase required"), `WrongPassphrase`, `Corrupt`.
    /// Wrong passphrases are not throttled: the bundle is a file the user
    /// holds, not this vault.
    pub fn open_backup_bundle(
        &self,
        bundle: &WalletBackupBundle,
        passphrase: Option<&[u8]>,
        header: &[u8],
    ) -> Result<(WalletSecret, Zeroizing<Vec<u8>>), VaultError> {
        if bundle.version != BUNDLE_VERSION {
            return Err(VaultError::Corrupt(format!(
                "bundle version {}",
                bundle.version
            )));
        }
        let wallet: WalletId = bundle
            .wallet_id
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Corrupt("bundle wallet id".into()))?;
        let own = self.inner().file.as_ref().is_some_and(|f| {
            bool::from(f.vault_id.ct_eq(&bundle.vault_id)) && f.network == bundle.network
        });
        let own_key = if own {
            match self.lock_state() {
                LockState::Unlocked | LockState::Unencrypted | LockState::NoKeys => {
                    self.full_dek().ok()
                }
                _ => None,
            }
        } else {
            None
        };
        let dek = match (own_key, &bundle.slot) {
            (Some(k), _) => k,
            (None, BackupKeySlot::VaultKey) => {
                return Err(VaultError::Corrupt(
                    "the backup can only be opened by the vault that wrote it".into(),
                ));
            }
            (
                None,
                BackupKeySlot::VaultPassphrase {
                    kdf,
                    salt,
                    wrapped_dek,
                }
                | BackupKeySlot::BackupPassphrase {
                    kdf,
                    salt,
                    wrapped_dek,
                },
            ) => {
                let pw = passphrase.ok_or(VaultError::NotEncrypted)?;
                let aad = match &bundle.slot {
                    BackupKeySlot::VaultPassphrase { .. } => {
                        file::slot_p_aad(&bundle.vault_id, &bundle.network, kdf, salt)
                    }
                    _ => slot_aad(
                        BACKUP_SLOT_AAD,
                        &bundle.vault_id,
                        &bundle.network,
                        kdf,
                        salt,
                    ),
                };
                let kek = crypto::derive_kek(&super::normalize_passphrase(pw), salt, kdf)?;
                let opened =
                    crypto::open(&kek, wrapped_dek, &aad).ok_or(VaultError::WrongPassphrase {
                        failed_attempts: 0,
                        retry_after_secs: None,
                    })?;
                Zeroizing::new(
                    opened[..]
                        .try_into()
                        .map_err(|_| VaultError::Corrupt("data key length".into()))?,
                )
            }
        };
        let open_record = |kind: &str| -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
            let Some(sealed) = bundle.records.get(kind) else {
                return Ok(None);
            };
            let aad =
                file::record_aad(&bundle.vault_id, &bundle.network, &record_id(&wallet, kind));
            crypto::open(&dek, sealed, &aad)
                .map(Some)
                .ok_or_else(|| VaultError::Corrupt(format!("backup record {kind}")))
        };
        let seed_payload = open_record(REC_SEED)?.ok_or(VaultError::NoSecret)?;
        let (seed, derivation) = decode_seed(&seed_payload)?;
        let secret = WalletSecret {
            mnemonic: open_record(REC_MNEMONIC)?.unwrap_or_default(),
            mnemonic_passphrase: open_record(REC_PASSPHRASE)?.unwrap_or_default(),
            seed,
            derivation,
        };
        let key = payload_key(&dek, &bundle.payload_salt);
        let payload = crypto::open(
            &key,
            &bundle.payload,
            &bundle_payload_aad(&wallet, &bundle.vault_id, header),
        )
        .ok_or_else(|| VaultError::Corrupt("backup payload failed authentication".into()))?;
        Ok((secret, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::KdfPolicy;
    use crate::os_store::MemoryOsStore;
    use crate::types::{Credential, GrantPurpose, UnlockScope, VaultConfig};
    use std::sync::Arc;

    fn config() -> VaultConfig {
        VaultConfig {
            kdf: KdfPolicy::Fixed(KdfParams::TEST),
            os_store: Arc::new(MemoryOsStore::default()),
            ..VaultConfig::default()
        }
    }

    fn open(dir: &std::path::Path) -> Vault {
        Vault::open(dir, key_wallet::Network::Regtest, "regtest", config()).unwrap()
    }

    fn secret(derivation: SeedDerivation) -> WalletSecret {
        WalletSecret {
            mnemonic: Zeroizing::new(if derivation == SeedDerivation::RawSeed {
                Vec::new()
            } else {
                b"phrase".to_vec()
            }),
            mnemonic_passphrase: Zeroizing::new(b"p".to_vec()),
            seed: Zeroizing::new([9; 64]),
            derivation,
        }
    }

    #[test]
    fn raw_seed_has_no_mnemonic_records() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.create(None).unwrap();
        let w = [1u8; 32];
        v.store_wallet_secret(&w, &secret(SeedDerivation::RawSeed))
            .unwrap();
        assert!(v.has_wallet_secret(&w));
        assert!(!v.has_wallet_mnemonic(&w));
        assert_eq!(v.seed_derivation(&w).unwrap(), SeedDerivation::RawSeed);
        let check = v.core_mnemonic_check(&w).unwrap();
        assert!(!check.has_mnemonic && !check.core_compatible);
    }

    #[test]
    fn export_needs_a_reveal_token_of_the_wallet() {
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.create(Some(b"pw")).unwrap();
        let w = [2u8; 32];
        v.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        let sign = v
            .authorize(GrantPurpose::SignMessage, Some(&w), Credential::None)
            .unwrap();
        let token = v
            .redeem_grant(&sign.id, GrantKind::SignMessage, Some(&w))
            .unwrap();
        assert_eq!(
            v.export_wallet_secret(&w, &token).unwrap_err(),
            VaultError::GrantPurposeMismatch
        );
        let reveal = v
            .authorize(
                GrantPurpose::RevealSecret,
                Some(&w),
                Credential::Passphrase(b"pw"),
            )
            .unwrap();
        let token = v
            .redeem_grant(&reveal.id, GrantKind::RevealSecret, Some(&w))
            .unwrap();
        let s = v.export_wallet_secret(&w, &token).unwrap();
        assert_eq!(&s.mnemonic[..], b"phrase");
        assert_eq!(*s.seed, [9; 64]);
    }

    #[test]
    fn bundles_open_in_another_vault_with_the_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let w = [3u8; 32];
        let src = open(&dir.path().join("a"));
        src.create(Some(b"vault pw")).unwrap();
        src.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        assert!(src.backup_bundle(&w, Some(b"x"), b"p", b"h").is_err());
        let bundle = src.backup_bundle(&w, None, b"payload", b"hdr").unwrap();
        let json = serde_json::to_vec(&bundle).unwrap();
        let bundle: WalletBackupBundle = serde_json::from_slice(&json).unwrap();

        let dst = open(&dir.path().join("b"));
        dst.create(None).unwrap();
        assert_eq!(
            dst.open_backup_bundle(&bundle, None, b"hdr").unwrap_err(),
            VaultError::NotEncrypted
        );
        assert!(matches!(
            dst.open_backup_bundle(&bundle, Some(b"nope"), b"hdr"),
            Err(VaultError::WrongPassphrase { .. })
        ));
        assert!(matches!(
            dst.open_backup_bundle(&bundle, Some(b"vault pw"), b"other header"),
            Err(VaultError::Corrupt(_))
        ));
        let (s, payload) = dst
            .open_backup_bundle(&bundle, Some(b"vault pw"), b"hdr")
            .unwrap();
        assert_eq!(&payload[..], b"payload");
        assert_eq!(&s.mnemonic[..], b"phrase");
        assert_eq!(*s.seed, [9; 64]);

        // The source vault opens its own bundle without the passphrase while
        // unlocked, and needs it once locked.
        assert!(src.open_backup_bundle(&bundle, None, b"hdr").is_ok());
        src.lock();
        assert_eq!(
            src.open_backup_bundle(&bundle, None, b"hdr").unwrap_err(),
            VaultError::NotEncrypted
        );
        src.unlock(b"vault pw", UnlockScope::Full).unwrap();
    }

    #[test]
    fn unencrypted_vault_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let w = [4u8; 32];
        let src = open(&dir.path().join("a"));
        src.create(None).unwrap();
        src.store_wallet_secret(&w, &secret(SeedDerivation::RawSeed))
            .unwrap();
        let own = src.backup_bundle(&w, None, b"p", b"h").unwrap();
        assert_eq!(own.slot, BackupKeySlot::VaultKey);
        let with_pw = src
            .backup_bundle(&w, Some(b"backup pw"), b"p", b"h")
            .unwrap();

        let dst = open(&dir.path().join("b"));
        dst.create(None).unwrap();
        assert!(matches!(
            dst.open_backup_bundle(&own, None, b"h"),
            Err(VaultError::Corrupt(_))
        ));
        let (s, _) = dst
            .open_backup_bundle(&with_pw, Some(b"backup pw"), b"h")
            .unwrap();
        assert_eq!(s.derivation, SeedDerivation::RawSeed);
        assert!(s.mnemonic.is_empty());
        assert!(src.open_backup_bundle(&own, None, b"h").is_ok());
    }
}
