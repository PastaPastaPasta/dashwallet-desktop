//! Vault operations of the dash-qt compatibility features (R2, M2):
//! exporting a wallet's secrets for dash-qt (QT-109), checking whether its
//! phrase restores the same wallet in Dash Core, and the key material of
//! `.dwbackup` bundles (QT-110, QT-116; docs/contracts/dwbackup-v1.md).
//!
//! A backup bundle (version 2) is sealed under a key of its own: a fresh
//! random backup key per bundle seals the wallet's records and the payload
//! (app metadata). The slot wraps only that key — under the vault
//! passphrase, a backup passphrase, or nothing — and a second copy is
//! wrapped under a key derived from the vault's data key, so the vault that
//! wrote the bundle opens it without a passphrase. Nothing in a bundle
//! unwraps the vault's data key (review H2): a backup's passphrase opens
//! that backup only, never `vault.dwv` or another wallet's backup.
//!
//! Version 1 bundles (written by M2 before that fix) carried the records as
//! stored and a slot wrapping the data key itself; they are still read.

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

/// Bundle format version this vault writes (docs/contracts/dwbackup-v1.md).
pub const BUNDLE_VERSION: u32 = 2;
/// The legacy version still read: slot and records under the data key.
const LEGACY_BUNDLE_VERSION: u32 = 1;

/// Whether this build reads bundles of `version` (1 and 2). A reader checks
/// this on the raw JSON before parsing a bundle, so a newer bundle is
/// reported as an unsupported version rather than as corrupt.
pub fn reads_bundle_version(version: u64) -> bool {
    version == u64::from(BUNDLE_VERSION) || version == u64::from(LEGACY_BUNDLE_VERSION)
}

/// How a bundle's key is recovered. In a version 2 bundle `wrapped_key` is
/// the bundle's own backup key; in a version 1 bundle it is the source
/// vault's data key (field name `wrapped_dek` on disk then).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum BackupKeySlot {
    /// The source vault is encrypted: the vault passphrase at backup time
    /// opens it. v2: Argon2id(passphrase, slot P's salt and parameters) →
    /// KEK, SHA-256(tag ‖ KEK) wraps the backup key. v1: a copy of slot P.
    VaultPassphrase {
        kdf: KdfParams,
        #[serde(with = "hex_bytes")]
        salt: Vec<u8>,
        #[serde(alias = "wrapped_dek")]
        wrapped_key: Sealed,
    },
    /// Unencrypted vault, user backup: a slot of its own over the backup
    /// passphrase.
    BackupPassphrase {
        kdf: KdfParams,
        #[serde(with = "hex_bytes")]
        salt: Vec<u8>,
        #[serde(alias = "wrapped_dek")]
        wrapped_key: Sealed,
    },
    /// No passphrase slot: only the vault the bundle came from (same vault
    /// id, data key available) opens it. Automatic backups of an
    /// unencrypted vault, which has no passphrase to wrap with.
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
    /// v2: the backup key sealed under a key derived from the source
    /// vault's data key, so that vault opens its bundles without a
    /// passphrase. Absent in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) vault_wrapped_key: Option<Sealed>,
    /// `mnemonic` / `mnemonic_passphrase` / `seed` → the record payload
    /// sealed under the backup key (v1: the vault record as stored).
    pub(crate) records: BTreeMap<String, Sealed>,
    #[serde(with = "hex_bytes")]
    pub(crate) payload_salt: Vec<u8>,
    /// The caller's payload, sealed under the payload key; AAD = the file
    /// header the caller passes.
    pub(crate) payload: Sealed,
}

impl WalletBackupBundle {
    /// The bundle's format version.
    pub fn version(&self) -> u32 {
        self.version
    }

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

/// The key that wraps a v2 bundle's backup key under the vault passphrase:
/// SHA-256 over a domain tag and slot P's KEK (uniformly random Argon2id
/// output, so a hash is a sound KDF), with slot P's parameters and salt so
/// a reader can derive it from the passphrase alone. It cannot unwrap the
/// data key: slot P is sealed under the KEK itself, which the hash does not
/// reveal.
#[derive(Clone)]
pub(crate) struct BackupKek {
    kdf: KdfParams,
    salt: Vec<u8>,
    key: Key32,
}

impl BackupKek {
    pub(crate) fn of(kdf: KdfParams, salt: &[u8], kek: &[u8; 32]) -> Self {
        Self {
            kdf,
            salt: salt.to_vec(),
            key: backup_kek_key(kek),
        }
    }
}

fn backup_kek_key(kek: &[u8; 32]) -> Key32 {
    let mut h = Sha256::new();
    h.update(b"dw-vault/backup-kek/v2");
    h.update(kek);
    Zeroizing::new(h.finalize().into())
}

/// The key that wraps a v2 bundle's backup key for the vault that wrote it.
fn vault_wrap_key(dek: &[u8; 32]) -> Key32 {
    let mut h = Sha256::new();
    h.update(b"dw-vault/backup-vault-wrap/v2");
    h.update(dek);
    Zeroizing::new(h.finalize().into())
}

fn slot_aad(kind: &[u8], vault_id: &[u8], network: &str, kdf: &KdfParams, salt: &[u8]) -> Vec<u8> {
    aad(
        kind,
        &[vault_id, network.as_bytes(), &kdf.aad_bytes()[..], salt],
    )
}

/// `tag ‖ field…`, each field length-prefixed.
fn aad(tag: &[u8], fields: &[&[u8]]) -> Vec<u8> {
    let mut a = tag.to_vec();
    for field in fields {
        a.extend_from_slice(&(field.len() as u32).to_le_bytes());
        a.extend_from_slice(field);
    }
    a
}

/// v1 backup-passphrase slot (wraps the data key).
const BACKUP_SLOT_AAD_V1: &[u8] = b"dw-vault/backup-slot/v1";
/// v2 backup-passphrase slot (wraps the backup key).
const BACKUP_SLOT_AAD: &[u8] = b"dw-vault/backup-slot/v2";
/// v2 vault-passphrase slot (wraps the backup key).
const VAULT_PASSPHRASE_SLOT_AAD: &[u8] = b"dw-vault/backup-vault-slot/v2";
const VAULT_WRAP_AAD: &[u8] = b"dw-vault/backup-vault-wrap/v2";
const BUNDLE_RECORD_AAD: &[u8] = b"dw-vault/backup-record/v2";

fn bundle_record_aad(vault_id: &[u8], network: &str, wallet: &[u8], kind: &str) -> Vec<u8> {
    aad(
        BUNDLE_RECORD_AAD,
        &[vault_id, network.as_bytes(), wallet, kind.as_bytes()],
    )
}

/// Payload key: SHA-256 over a domain tag, the bundle key (v2: the backup
/// key; v1: the data key) and the bundle's random salt. The key is
/// uniformly random, so a hash is a sound KDF.
fn payload_key(key: &[u8; 32], salt: &[u8]) -> Key32 {
    let mut h = Sha256::new();
    h.update(b"dw-vault/backup-payload/v1");
    h.update(key);
    h.update(salt);
    Zeroizing::new(h.finalize().into())
}

fn bundle_payload_aad(bundle_wallet: &[u8], vault_id: &[u8], header: &[u8]) -> Vec<u8> {
    aad(
        b"dw-vault/backup-payload/v1",
        &[bundle_wallet, vault_id, header],
    )
}

fn wrong_passphrase() -> VaultError {
    VaultError::WrongPassphrase {
        failed_attempts: 0,
        retry_after_secs: None,
    }
}

fn key32(bytes: &[u8]) -> Result<Key32, VaultError> {
    Ok(Zeroizing::new(bytes.try_into().map_err(|_| {
        VaultError::Corrupt("backup key length".into())
    })?))
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
    /// unencrypted). A fresh backup key seals the wallet's records and the
    /// payload; the slot wraps only that key:
    ///
    /// - Encrypted vault: the vault passphrase (slot P's KDF parameters and
    ///   salt, the backup KEK this process derived when the passphrase was
    ///   last checked or set; `Locked` without one). `backup_passphrase`
    ///   must be `None`.
    /// - Unencrypted vault with `backup_passphrase`: a new Argon2id slot
    ///   under the vault's KDF policy.
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
        let (vault_id, network, encrypted, backup_kek, stored) = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let mut stored = BTreeMap::new();
            for kind in [REC_MNEMONIC, REC_PASSPHRASE, REC_SEED] {
                if let Some(sealed) = f.records.get(&record_id(wallet, kind)) {
                    stored.insert(kind, sealed.clone());
                }
            }
            // The backup KEK belongs to the current slot P only.
            let backup_kek = match (&f.slot_p, &inner.backup_kek) {
                (Some(p), Some(k)) if p.kdf == k.kdf && p.salt == k.salt => Some(k.clone()),
                _ => None,
            };
            (
                f.vault_id.clone(),
                f.network.clone(),
                f.slot_p.is_some(),
                backup_kek,
                stored,
            )
        };
        if !stored.contains_key(REC_SEED) {
            return Err(VaultError::NoSecret);
        }
        let backup_key = crypto::random_key()?;
        let mut records = BTreeMap::new();
        for (kind, sealed) in &stored {
            let plain = crypto::open(
                &dek,
                sealed,
                &file::record_aad(&vault_id, &network, &record_id(wallet, kind)),
            )
            .ok_or_else(|| VaultError::Corrupt(format!("record {kind} failed authentication")))?;
            records.insert(
                (*kind).to_owned(),
                crypto::seal(
                    &backup_key,
                    &plain,
                    &bundle_record_aad(&vault_id, &network, wallet, kind),
                )?,
            );
        }
        let slot = match (encrypted, backup_passphrase) {
            (true, Some(_)) => {
                return Err(VaultError::InvalidArgument(
                    "an encrypted vault's backups use the vault passphrase".into(),
                ));
            }
            (true, None) => {
                let k = backup_kek.ok_or(VaultError::Locked)?;
                let aad = slot_aad(
                    VAULT_PASSPHRASE_SLOT_AAD,
                    &vault_id,
                    &network,
                    &k.kdf,
                    &k.salt,
                );
                BackupKeySlot::VaultPassphrase {
                    wrapped_key: crypto::seal(&k.key, &backup_key[..], &aad)?,
                    kdf: k.kdf,
                    salt: k.salt,
                }
            }
            (false, Some(pw)) => {
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
                    wrapped_key: crypto::seal(&kek, &backup_key[..], &aad)?,
                }
            }
            (false, None) => BackupKeySlot::VaultKey,
        };
        let vault_wrapped_key = crypto::seal(
            &vault_wrap_key(&dek),
            &backup_key[..],
            &aad(VAULT_WRAP_AAD, &[&vault_id, network.as_bytes(), wallet]),
        )?;
        let payload_salt: [u8; SALT_LEN] = crypto::random_array()?;
        let key = payload_key(&backup_key, &payload_salt);
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
            vault_wrapped_key: Some(vault_wrapped_key),
            records,
            payload_salt: payload_salt.to_vec(),
            payload: sealed,
        })
    }

    /// Opens a bundle: recovers its key (through this vault's own data key
    /// when the bundle came from this vault and the key is available,
    /// otherwise the slot with `passphrase`), then the wallet's secrets and
    /// the payload. Stores nothing. Reads versions 1 and 2; any other is
    /// `Corrupt` here, so callers check [`reads_bundle_version`] first.
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
        let legacy = match bundle.version {
            BUNDLE_VERSION => false,
            LEGACY_BUNDLE_VERSION => true,
            v => return Err(VaultError::Corrupt(format!("bundle version {v}"))),
        };
        let wallet: WalletId = bundle
            .wallet_id
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Corrupt("bundle wallet id".into()))?;
        let own = self.inner().file.as_ref().is_some_and(|f| {
            bool::from(f.vault_id.ct_eq(&bundle.vault_id)) && f.network == bundle.network
        });
        let own_dek = if own {
            match self.lock_state() {
                LockState::Unlocked | LockState::Unencrypted | LockState::NoKeys => {
                    self.full_dek().ok()
                }
                _ => None,
            }
        } else {
            None
        };
        let own_key = match (own_dek, legacy) {
            (Some(dek), true) => Some(dek),
            (Some(dek), false) => {
                let wrapped = bundle
                    .vault_wrapped_key
                    .as_ref()
                    .ok_or_else(|| VaultError::Corrupt("bundle has no vault-wrapped key".into()))?;
                let opened = crypto::open(
                    &vault_wrap_key(&dek),
                    wrapped,
                    &aad(
                        VAULT_WRAP_AAD,
                        &[&bundle.vault_id, bundle.network.as_bytes(), &wallet],
                    ),
                )
                .ok_or_else(|| VaultError::Corrupt("vault-wrapped backup key".into()))?;
                Some(key32(&opened)?)
            }
            (None, _) => None,
        };
        let key = match (own_key, &bundle.slot) {
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
                    wrapped_key,
                }
                | BackupKeySlot::BackupPassphrase {
                    kdf,
                    salt,
                    wrapped_key,
                },
            ) => {
                let pw = passphrase.ok_or(VaultError::NotEncrypted)?;
                let vault_slot = matches!(bundle.slot, BackupKeySlot::VaultPassphrase { .. });
                let aad = match (legacy, vault_slot) {
                    (true, true) => file::slot_p_aad(&bundle.vault_id, &bundle.network, kdf, salt),
                    (true, false) => slot_aad(
                        BACKUP_SLOT_AAD_V1,
                        &bundle.vault_id,
                        &bundle.network,
                        kdf,
                        salt,
                    ),
                    (false, true) => slot_aad(
                        VAULT_PASSPHRASE_SLOT_AAD,
                        &bundle.vault_id,
                        &bundle.network,
                        kdf,
                        salt,
                    ),
                    (false, false) => slot_aad(
                        BACKUP_SLOT_AAD,
                        &bundle.vault_id,
                        &bundle.network,
                        kdf,
                        salt,
                    ),
                };
                let kek = crypto::derive_kek(&super::normalize_passphrase(pw), salt, kdf)?;
                let wrap = if !legacy && vault_slot {
                    backup_kek_key(&kek)
                } else {
                    kek
                };
                let opened = crypto::open(&wrap, wrapped_key, &aad).ok_or_else(wrong_passphrase)?;
                key32(&opened)?
            }
        };
        let open_record = |kind: &str| -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
            let Some(sealed) = bundle.records.get(kind) else {
                return Ok(None);
            };
            let aad = if legacy {
                file::record_aad(&bundle.vault_id, &bundle.network, &record_id(&wallet, kind))
            } else {
                bundle_record_aad(&bundle.vault_id, &bundle.network, &wallet, kind)
            };
            crypto::open(&key, sealed, &aad)
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
        let key = payload_key(&key, &bundle.payload_salt);
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

    /// The M2 writer (bundle v1: records as stored, slot wrapping the data
    /// key), kept so the tests prove v1 bundles still open and show what
    /// the v2 format no longer hands out.
    fn legacy_v1_bundle(
        v: &Vault,
        wallet: &WalletId,
        backup_passphrase: Option<&[u8]>,
        payload: &[u8],
        header: &[u8],
    ) -> WalletBackupBundle {
        let dek = v.full_dek().unwrap();
        let f = v.inner().file.clone().unwrap();
        let mut records = BTreeMap::new();
        for kind in [REC_MNEMONIC, REC_PASSPHRASE, REC_SEED] {
            if let Some(sealed) = f.records.get(&record_id(wallet, kind)) {
                records.insert(kind.to_owned(), sealed.clone());
            }
        }
        let slot = match (f.slot_p, backup_passphrase) {
            (Some(p), _) => BackupKeySlot::VaultPassphrase {
                kdf: p.kdf,
                salt: p.salt,
                wrapped_key: p.wrapped_dek,
            },
            (None, Some(pw)) => {
                let salt = [7u8; SALT_LEN];
                let kek = crypto::derive_kek(pw, &salt, &KdfParams::TEST).unwrap();
                let aad = slot_aad(
                    BACKUP_SLOT_AAD_V1,
                    &f.vault_id,
                    &f.network,
                    &KdfParams::TEST,
                    &salt,
                );
                BackupKeySlot::BackupPassphrase {
                    kdf: KdfParams::TEST,
                    salt: salt.to_vec(),
                    wrapped_key: crypto::seal(&kek, &dek[..], &aad).unwrap(),
                }
            }
            (None, None) => BackupKeySlot::VaultKey,
        };
        let payload_salt = [8u8; SALT_LEN];
        let sealed = crypto::seal(
            &payload_key(&dek, &payload_salt),
            payload,
            &bundle_payload_aad(wallet, &f.vault_id, header),
        )
        .unwrap();
        WalletBackupBundle {
            version: LEGACY_BUNDLE_VERSION,
            vault_id: f.vault_id,
            network: f.network,
            wallet_id: wallet.to_vec(),
            slot,
            vault_wrapped_key: None,
            records,
            payload_salt: payload_salt.to_vec(),
            payload: sealed,
        }
    }

    /// The key a bundle's slot yields for `pw`, as an attacker holding the
    /// file and the passphrase would compute it.
    fn slot_key(bundle: &WalletBackupBundle, pw: &[u8]) -> Key32 {
        let (kdf, salt, wrapped, tag) = match &bundle.slot {
            BackupKeySlot::VaultPassphrase {
                kdf,
                salt,
                wrapped_key,
            } => (kdf, salt, wrapped_key, VAULT_PASSPHRASE_SLOT_AAD),
            BackupKeySlot::BackupPassphrase {
                kdf,
                salt,
                wrapped_key,
            } => (kdf, salt, wrapped_key, BACKUP_SLOT_AAD),
            BackupKeySlot::VaultKey => panic!("no passphrase slot"),
        };
        let kek = crypto::derive_kek(pw, salt, kdf).unwrap();
        let wrap = if tag == VAULT_PASSPHRASE_SLOT_AAD {
            backup_kek_key(&kek)
        } else {
            kek
        };
        let aad = slot_aad(tag, &bundle.vault_id, &bundle.network, kdf, salt);
        key32(&crypto::open(&wrap, wrapped, &aad).unwrap()).unwrap()
    }

    /// Whether `key` opens anything of the vault file: its manifest or a
    /// record of `w`.
    fn opens_vault(v: &Vault, w: &WalletId, key: &[u8; 32]) -> bool {
        let f = v.inner().file.clone().unwrap();
        let manifest = crypto::open(
            key,
            &f.manifest,
            &file::manifest_aad(&f.vault_id, &f.network),
        );
        let seed_id = record_id(w, REC_SEED);
        let record = crypto::open(
            key,
            &f.records[&seed_id],
            &file::record_aad(&f.vault_id, &f.network, &seed_id),
        );
        manifest.is_some() || record.is_some()
    }

    fn change_credential(v: &Vault) -> String {
        v.authorize(GrantPurpose::ChangeCredential, None, Credential::None)
            .unwrap()
            .id
    }

    /// Review H2: a backup's passphrase yields the backup's own key, which
    /// opens nothing in `vault.dwv`; the v1 slot yielded the data key.
    #[test]
    fn a_backup_passphrase_cannot_open_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let (w, other) = ([5u8; 32], [6u8; 32]);
        let v = open(dir.path());
        v.create(None).unwrap();
        v.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        v.store_wallet_secret(&other, &secret(SeedDerivation::Bip39))
            .unwrap();
        let dek = v.full_dek().unwrap();

        let legacy = legacy_v1_bundle(&v, &w, Some(b"weak"), b"p", b"h");
        let BackupKeySlot::BackupPassphrase {
            salt,
            wrapped_key,
            kdf,
        } = &legacy.slot
        else {
            unreachable!()
        };
        let kek = crypto::derive_kek(b"weak", salt, kdf).unwrap();
        let aad = slot_aad(
            BACKUP_SLOT_AAD_V1,
            &legacy.vault_id,
            &legacy.network,
            kdf,
            salt,
        );
        let leaked = key32(&crypto::open(&kek, wrapped_key, &aad).unwrap()).unwrap();
        assert!(
            opens_vault(&v, &other, &leaked),
            "v1 handed out the data key"
        );

        let bundle = v.backup_bundle(&w, Some(b"weak"), b"p", b"h").unwrap();
        let key = slot_key(&bundle, b"weak");
        assert_ne!(*key, *dek);
        assert!(!opens_vault(&v, &w, &key));
        assert!(!opens_vault(&v, &other, &key));
        // The bundle holds no vault record as stored.
        let stored = v.inner().file.clone().unwrap().records;
        assert!(
            bundle
                .records
                .values()
                .all(|r| !stored.values().any(|s| s == r))
        );
        // Two backups of one wallet have different keys.
        let again = v.backup_bundle(&w, Some(b"weak"), b"p", b"h").unwrap();
        assert_ne!(*slot_key(&again, b"weak"), *key);

        // Encrypted vault: the vault passphrase opens the backup's key only.
        v.encrypt(b"vault pw", &change_credential(&v)).unwrap();
        v.unlock(b"vault pw", UnlockScope::Full).unwrap();
        let enc = v.backup_bundle(&w, None, b"p", b"h").unwrap();
        let key = slot_key(&enc, b"vault pw");
        assert!(!opens_vault(&v, &w, &key));
        assert!(!opens_vault(&v, &other, &key));
        // The weak backup passphrase is not a key to the encrypted vault.
        v.lock();
        assert!(matches!(
            v.unlock(b"weak", UnlockScope::Full),
            Err(VaultError::WrongPassphrase { .. })
        ));
    }

    /// Review H2: a backup made before a passphrase change still restores
    /// with the passphrase it was made under; new backups need the new one.
    #[test]
    fn an_old_backup_restores_after_change_passphrase() {
        let dir = tempfile::tempdir().unwrap();
        let w = [7u8; 32];
        let src = open(&dir.path().join("a"));
        src.create(Some(b"old pw")).unwrap();
        src.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        let before = src.backup_bundle(&w, None, b"payload", b"hdr").unwrap();
        let legacy = legacy_v1_bundle(&src, &w, None, b"payload", b"hdr");
        // Changed while unlocked: the vault stays unlocked.
        src.change_passphrase(b"old pw", b"new pw").unwrap();
        let after = src.backup_bundle(&w, None, b"payload", b"hdr").unwrap();

        let dst = open(&dir.path().join("b"));
        dst.create(None).unwrap();
        for b in [&before, &legacy] {
            let (s, payload) = dst.open_backup_bundle(b, Some(b"old pw"), b"hdr").unwrap();
            assert_eq!(*s.seed, [9; 64]);
            assert_eq!(&payload[..], b"payload");
        }
        assert!(matches!(
            dst.open_backup_bundle(&before, Some(b"new pw"), b"hdr"),
            Err(VaultError::WrongPassphrase { .. })
        ));
        assert!(
            dst.open_backup_bundle(&after, Some(b"new pw"), b"hdr")
                .is_ok()
        );
        assert!(matches!(
            dst.open_backup_bundle(&after, Some(b"old pw"), b"hdr"),
            Err(VaultError::WrongPassphrase { .. })
        ));

        // The source opens its old backup with its own key once unlocked
        // with the new passphrase, and with the old one while locked.
        src.lock();
        assert!(
            src.open_backup_bundle(&before, Some(b"old pw"), b"hdr")
                .is_ok()
        );
        src.unlock(b"new pw", UnlockScope::Full).unwrap();
        assert!(src.open_backup_bundle(&before, None, b"hdr").is_ok());
        assert!(src.open_backup_bundle(&legacy, None, b"hdr").is_ok());
        // Unlocked again with the new passphrase, it writes new-passphrase
        // backups.
        let later = src.backup_bundle(&w, None, b"payload", b"hdr").unwrap();
        assert!(
            dst.open_backup_bundle(&later, Some(b"new pw"), b"hdr")
                .is_ok()
        );
    }

    /// Review H2: backups of an unencrypted vault (automatic, with a backup
    /// passphrase, and v1 ones) still restore after `encrypt`.
    #[test]
    fn a_backup_made_before_encrypt_restores() {
        let dir = tempfile::tempdir().unwrap();
        let w = [8u8; 32];
        let src = open(&dir.path().join("a"));
        src.create(None).unwrap();
        src.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        let automatic = src.backup_bundle(&w, None, b"a", b"h").unwrap();
        assert_eq!(automatic.slot, BackupKeySlot::VaultKey);
        let with_pw = src
            .backup_bundle(&w, Some(b"backup pw"), b"b", b"h")
            .unwrap();
        let legacy = legacy_v1_bundle(&src, &w, None, b"c", b"h");

        src.encrypt(b"vault pw", &change_credential(&src)).unwrap();
        // Locked: the automatic backup needs the vault's own key.
        assert!(matches!(
            src.open_backup_bundle(&automatic, None, b"h"),
            Err(VaultError::Corrupt(_))
        ));
        src.unlock(b"vault pw", UnlockScope::Full).unwrap();
        for (b, payload) in [(&automatic, b"a"), (&with_pw, b"b"), (&legacy, b"c")] {
            let (s, p) = src.open_backup_bundle(b, None, b"h").unwrap();
            assert_eq!(*s.seed, [9; 64]);
            assert_eq!(&p[..], payload);
        }
        let dst = open(&dir.path().join("b"));
        dst.create(None).unwrap();
        assert!(
            dst.open_backup_bundle(&with_pw, Some(b"backup pw"), b"h")
                .is_ok()
        );
        // After encrypt, new backups use the vault passphrase.
        let enc = src.backup_bundle(&w, None, b"d", b"h").unwrap();
        assert!(matches!(enc.slot, BackupKeySlot::VaultPassphrase { .. }));
        assert!(
            dst.open_backup_bundle(&enc, Some(b"vault pw"), b"h")
                .is_ok()
        );
    }

    /// Review M1: a crafted `.dwbackup` slot with an absurd Argon2id cost is
    /// corrupt, not an abort or a hang.
    #[test]
    fn crafted_kdf_cost_in_a_bundle_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let w = [9u8; 32];
        let src = open(&dir.path().join("a"));
        src.create(None).unwrap();
        src.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        let bundle = src.backup_bundle(&w, Some(b"pw"), b"p", b"h").unwrap();
        let mut json: serde_json::Value = serde_json::to_value(&bundle).unwrap();
        json["slot"]["kdf"]["m_kib"] = u32::MAX.into();
        let crafted: WalletBackupBundle = serde_json::from_value(json).unwrap();
        let dst = open(&dir.path().join("b"));
        dst.create(None).unwrap();
        assert!(matches!(
            dst.open_backup_bundle(&crafted, Some(b"pw"), b"h"),
            Err(VaultError::Corrupt(d)) if d.contains("above the limits")
        ));
    }

    /// Fix-review L5: a version 1 bundle as the M2 writer put it on disk
    /// (slot field `wrapped_dek`, internally tagged `kind`) parses through
    /// the `wrapped_key` alias and opens in another vault with its
    /// passphrase. Fixture: testdata/dwbackup/v1_bundles_wrapped_dek.json
    /// (wallet 11…11, seed [9; 64], mnemonic "phrase").
    #[test]
    fn v1_fixture_with_wrapped_dek_opens() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../testdata/dwbackup/v1_bundles_wrapped_dek.json"
        ))
        .unwrap();
        let header = fixture["header"].as_str().unwrap().as_bytes();
        let dir = tempfile::tempdir().unwrap();
        let dst = open(dir.path());
        dst.create(None).unwrap();
        for (name, pw) in [
            ("backup_passphrase", &b"fixture pw"[..]),
            ("vault_passphrase", &b"vault pw"[..]),
        ] {
            let raw = &fixture[name];
            assert!(raw["slot"].get("wrapped_dek").is_some(), "{name}");
            assert!(raw["slot"].get("wrapped_key").is_none(), "{name}");
            let bundle: WalletBackupBundle = serde_json::from_value(raw.clone()).unwrap();
            assert_eq!(bundle.version, LEGACY_BUNDLE_VERSION);
            let (s, payload) = dst.open_backup_bundle(&bundle, Some(pw), header).unwrap();
            assert_eq!(&payload[..], b"fixture payload", "{name}");
            assert_eq!(*s.seed, [9; 64]);
            assert_eq!(&s.mnemonic[..], b"phrase");
            assert!(matches!(
                dst.open_backup_bundle(&bundle, Some(b"wrong"), header),
                Err(VaultError::WrongPassphrase { .. })
            ));
        }
    }

    /// Fix-review L5: callers can tell a bundle version this build does not
    /// read (the engine reports `backup.unsupported_version`) before
    /// opening; `open_backup_bundle` itself refuses it.
    #[test]
    fn unknown_bundle_versions_are_unsupported() {
        assert!(reads_bundle_version(1) && reads_bundle_version(2));
        assert!(!reads_bundle_version(0) && !reads_bundle_version(3));
        let dir = tempfile::tempdir().unwrap();
        let v = open(dir.path());
        v.create(None).unwrap();
        let w = [0x12u8; 32];
        v.store_wallet_secret(&w, &secret(SeedDerivation::Bip39))
            .unwrap();
        let mut bundle = v.backup_bundle(&w, Some(b"bk"), b"p", b"h").unwrap();
        bundle.version = 3;
        assert_eq!(bundle.version(), 3);
        assert!(matches!(
            v.open_backup_bundle(&bundle, Some(b"bk"), b"h"),
            Err(VaultError::Corrupt(_))
        ));
    }
}
