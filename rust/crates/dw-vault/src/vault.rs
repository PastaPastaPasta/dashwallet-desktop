//! The vault of one network: data key, wrap slots, records, lock state and
//! grants (DESIGN-opus §1.8).
//!
//! Locking: `inner` is a short-held std mutex. Argon2id derivations and OS
//! store calls run outside it; file writes run inside it so read-modify-write
//! of the vault file is serialized.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use key_wallet::Network;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::crypto::{self, Key32, SALT_LEN, Sealed};
use crate::file::{self, Manifest, OsSlot, PassphraseSlot, Throttle, VaultFile};
use crate::signer::{SignerScope, VaultSigner};
use crate::types::{
    AuthGrant, Credential, GrantKind, GrantPurpose, GrantToken, LockState, RevealedMnemonic,
    SeedDerivation, UnlockScope, VaultConfig, VaultStatus, WalletId, WalletSecret,
};
use crate::{SignerError, VaultError};

/// Longest accepted vault passphrase, in bytes.
pub const MAX_PASSPHRASE_BYTES: usize = 1024;
/// Label of the data key in the OS store.
const OS_LABEL: &str = "dek";

const REC_MNEMONIC: &str = "mnemonic";
const REC_PASSPHRASE: &str = "mnemonic_passphrase";
const REC_SEED: &str = "seed";
/// Seed record payload: version ‖ derivation ‖ weak flag ‖ 64-byte seed.
const SEED_RECORD_VERSION: u8 = 1;
const SEED_RECORD_LEN: usize = 3 + 64;

fn record_id(wallet: &WalletId, kind: &str) -> String {
    format!("wallet/{}/{kind}", hex::encode(wallet))
}

/// Wallet id of a `wallet/<hex>/seed` record id.
fn seed_record_wallet(id: &str) -> Option<WalletId> {
    let hex_id = id.strip_prefix("wallet/")?.strip_suffix("/seed")?;
    hex::decode(hex_id).ok()?.try_into().ok()
}

/// Seconds a caller waits after `failed` consecutive failures: iOS's
/// `6^(n−3)·60 s` from the third failure on (IOS-012). UX throttling only;
/// Argon2id cost is the real protection.
pub fn throttle_wait_secs(failed: u32) -> u64 {
    if failed < 3 {
        return 0;
    }
    6u64.saturating_pow((failed - 3).min(20)).saturating_mul(60)
}

fn retry_after(t: &Throttle, now: u64) -> Option<u64> {
    let last = t.last_failure_at?;
    let wait = throttle_wait_secs(t.failed_attempts);
    // A clock that went backwards does not shorten the wait.
    let now = now.max(last);
    let until = last.saturating_add(wait);
    (until > now).then(|| until - now)
}

/// NFC-normalizes UTF-8 passphrases so the same text typed on different
/// systems yields the same bytes; non-UTF-8 input is used as given.
fn normalize_passphrase(pw: &[u8]) -> Zeroizing<Vec<u8>> {
    match std::str::from_utf8(pw) {
        Ok(s) => {
            let mut out = Zeroizing::new(String::with_capacity(s.len() * 3));
            out.extend(s.nfc());
            Zeroizing::new(out.as_bytes().to_vec())
        }
        Err(_) => Zeroizing::new(pw.to_vec()),
    }
}

/// Validates and normalizes a passphrase that will become a slot P key.
fn new_passphrase(pw: &[u8]) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    if pw.is_empty() {
        return Err(VaultError::PassphraseRejected("empty".into()));
    }
    if pw.len() > MAX_PASSPHRASE_BYTES {
        return Err(VaultError::PassphraseRejected(format!(
            "longer than {MAX_PASSPHRASE_BYTES} bytes"
        )));
    }
    Ok(normalize_passphrase(pw))
}

fn os_service(vault_id: &[u8], network: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"dw-vault/slot-o/v1");
    h.update((vault_id.len() as u32).to_le_bytes());
    h.update(vault_id);
    h.update(network.as_bytes());
    h.finalize().into()
}

fn encode_seed(secret: &WalletSecret) -> Zeroizing<Vec<u8>> {
    let mut v = Zeroizing::new(Vec::with_capacity(SEED_RECORD_LEN));
    v.push(SEED_RECORD_VERSION);
    match secret.derivation {
        SeedDerivation::Bip39 => v.extend_from_slice(&[0, 0]),
        SeedDerivation::DashCore { weak_checksum } => v.extend_from_slice(&[1, weak_checksum as u8]),
    }
    v.extend_from_slice(&secret.seed[..]);
    v
}

fn decode_seed(payload: &[u8]) -> Result<(Zeroizing<[u8; 64]>, SeedDerivation), VaultError> {
    if payload.len() != SEED_RECORD_LEN || payload[0] != SEED_RECORD_VERSION {
        return Err(VaultError::Corrupt("seed record layout".into()));
    }
    let derivation = match (payload[1], payload[2]) {
        (0, 0) => SeedDerivation::Bip39,
        (1, w @ (0 | 1)) => SeedDerivation::DashCore {
            weak_checksum: w == 1,
        },
        _ => return Err(VaultError::Corrupt("seed record derivation".into())),
    };
    let mut seed = Zeroizing::new([0u8; 64]);
    seed.copy_from_slice(&payload[3..]);
    Ok((seed, derivation))
}

struct Inner {
    file: Option<VaultFile>,
    dek: Option<Key32>,
    scope: UnlockScope,
    /// Bumped whenever the data key leaves memory or the unlock scope
    /// changes. Grant tokens and signers from an older epoch are refused.
    epoch: u64,
    /// Highest manifest generation seen by this process (rollback check).
    high_water: u64,
    grants: HashMap<String, AuthGrant>,
}

impl Inner {
    /// Drops the data key, revokes grants and invalidates outstanding signers.
    fn forget_key(&mut self) {
        self.dek = None;
        self.scope = UnlockScope::Full;
        self.epoch += 1;
        self.grants.clear();
    }

    /// Installs a data key with `scope`; bumps the epoch when the state changes.
    fn install_key(&mut self, dek: Key32, scope: UnlockScope) {
        let changed = self.dek.is_none() || self.scope != scope;
        self.dek = Some(dek);
        self.scope = scope;
        if changed {
            self.epoch += 1;
            self.grants.clear();
        }
    }
}

pub(crate) struct Shared {
    dir: PathBuf,
    network: Network,
    /// Network tag bound into every AAD (`regtest`, `devnet-<name>`, …).
    tag: String,
    config: VaultConfig,
    inner: Mutex<Inner>,
}

/// The vault of one network. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Vault {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("dir", &self.shared.dir)
            .field("network", &self.shared.tag)
            .finish_non_exhaustive()
    }
}

impl Vault {
    /// Opens the vault stored in `dir` (`<network dir>/vault`). A missing file
    /// is not an error: the vault reports `NoVault` until `create`.
    pub fn open(
        dir: impl Into<PathBuf>,
        network: Network,
        network_tag: &str,
        config: VaultConfig,
    ) -> Result<Self, VaultError> {
        let dir = dir.into();
        let file = file::read(&dir)?;
        if let Some(f) = &file
            && f.network != network_tag
        {
            return Err(VaultError::Corrupt(format!(
                "vault belongs to {}, not {network_tag}",
                f.network
            )));
        }
        Ok(Self {
            shared: Arc::new(Shared {
                dir,
                network,
                tag: network_tag.to_owned(),
                config,
                inner: Mutex::new(Inner {
                    file,
                    dek: None,
                    scope: UnlockScope::Full,
                    epoch: 0,
                    high_water: 0,
                    grants: HashMap::new(),
                }),
            }),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.shared.dir
    }

    pub fn network(&self) -> Network {
        self.shared.network
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn now(&self) -> u64 {
        self.shared.config.clock.now_secs()
    }

    fn production_network(&self) -> bool {
        matches!(self.shared.network, Network::Mainnet | Network::Testnet)
    }

    fn state_of(inner: &Inner) -> LockState {
        match &inner.file {
            None => LockState::NoVault,
            Some(f) if f.slot_p.is_some() => match (&inner.dek, inner.scope) {
                (None, _) => LockState::Locked,
                (Some(_), UnlockScope::MixingOnly) => LockState::UnlockedMixingOnly,
                (Some(_), UnlockScope::Full) => LockState::Unlocked,
            },
            Some(f) if f.records.is_empty() => LockState::NoKeys,
            Some(_) => LockState::Unencrypted,
        }
    }

    fn status_of(&self, inner: &Inner) -> VaultStatus {
        let Some(f) = &inner.file else {
            return VaultStatus {
                state: LockState::NoVault,
                encrypted: false,
                quick_unlock_enrolled: false,
                failed_attempts: 0,
                retry_after_secs: None,
                wallets_with_secrets: Vec::new(),
            };
        };
        VaultStatus {
            state: Self::state_of(inner),
            encrypted: f.slot_p.is_some(),
            quick_unlock_enrolled: false,
            failed_attempts: f.throttle.failed_attempts,
            retry_after_secs: retry_after(&f.throttle, self.now()),
            wallets_with_secrets: f.records.keys().filter_map(|k| seed_record_wallet(k)).collect(),
        }
    }

    /// Current state. In-memory read.
    pub fn status(&self) -> VaultStatus {
        self.status_of(&self.inner())
    }

    pub fn lock_state(&self) -> LockState {
        Self::state_of(&self.inner())
    }

    /// Creates the vault. `Some(passphrase)` = encrypted (slot P);
    /// `None` = unencrypted (data key in the OS store, slot O). Leaves the
    /// vault unlocked.
    pub fn create(&self, passphrase: Option<&[u8]>) -> Result<VaultStatus, VaultError> {
        if self.inner().file.is_some() || file::read(&self.shared.dir)?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
        let dek = crypto::random_key()?;
        let vault_id: [u8; 16] = crypto::random_array()?;
        let tag = &self.shared.tag;
        let (slot_p, slot_o) = match passphrase {
            Some(pw) => {
                let pw = new_passphrase(pw)?;
                let salt: [u8; SALT_LEN] = crypto::random_array()?;
                let (kdf, kek) = crypto::derive_new_kek(
                    &pw,
                    &salt,
                    self.shared.config.kdf,
                    self.production_network(),
                )?;
                let aad = file::slot_p_aad(&vault_id, tag, &kdf, &salt);
                let wrapped_dek = crypto::seal(&kek, &dek[..], &aad)?;
                (
                    Some(PassphraseSlot {
                        kdf,
                        salt: salt.to_vec(),
                        wrapped_dek,
                    }),
                    None,
                )
            }
            None => {
                let service = os_service(&vault_id, tag);
                self.shared.config.os_store.put(&service, OS_LABEL, &dek[..])?;
                (
                    None,
                    Some(OsSlot {
                        service: service.to_vec(),
                        label: OS_LABEL.into(),
                    }),
                )
            }
        };
        let manifest = Manifest {
            generation: 1,
            records: BTreeMap::new(),
        };
        let vault_file = VaultFile {
            format: file::FORMAT,
            vault_id: vault_id.to_vec(),
            network: tag.clone(),
            created_at: self.now(),
            slot_p,
            slot_o,
            throttle: Throttle::default(),
            manifest: seal_manifest(&dek, &vault_id, tag, &manifest)?,
            records: BTreeMap::new(),
        };

        let mut inner = self.inner();
        let written = if inner.file.is_some() {
            Err(VaultError::AlreadyExists)
        } else {
            file::write(&self.shared.dir, &vault_file)
        };
        if let Err(e) = written {
            if let Some(o) = &vault_file.slot_o
                && let Ok(service) = <[u8; 32]>::try_from(o.service.as_slice())
                && let Err(cleanup) = self.shared.config.os_store.delete(&service, &o.label)
            {
                tracing::warn!(error = %cleanup, "could not remove the data key of a vault that was not created");
            }
            return Err(e);
        }
        inner.file = Some(vault_file);
        inner.high_water = 1;
        inner.install_key(dek, UnlockScope::Full);
        Ok(self.status_of(&inner))
    }

    /// dash-qt "Encrypt Wallet" (QT-111): adds slot P and deletes slot O.
    /// Needs a `ChangeCredential` grant. Leaves the vault locked, as Core
    /// does after `encryptwallet`. There is no way back to unencrypted.
    pub fn encrypt(&self, new_passphrase_bytes: &[u8], grant_id: &str) -> Result<VaultStatus, VaultError> {
        let pw = new_passphrase(new_passphrase_bytes)?;
        {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            if f.slot_p.is_some() {
                return Err(VaultError::AlreadyEncrypted);
            }
        }
        self.redeem_grant(grant_id, GrantKind::ChangeCredential)?;
        let dek = self.full_dek()?;
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) =
            crypto::derive_new_kek(&pw, &salt, self.shared.config.kdf, self.production_network())?;

        let mut inner = self.inner();
        let mut next = inner.file.clone().ok_or(VaultError::NoVault)?;
        if next.slot_p.is_some() {
            return Err(VaultError::AlreadyEncrypted);
        }
        let aad = file::slot_p_aad(&next.vault_id, &next.network, &kdf, &salt);
        next.slot_p = Some(PassphraseSlot {
            kdf,
            salt: salt.to_vec(),
            wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
        });
        let old_slot = next.slot_o.take();
        file::write(&self.shared.dir, &next)?;
        inner.file = Some(next);
        inner.forget_key();
        let status = self.status_of(&inner);
        drop(inner);

        if let Some(o) = old_slot {
            let service: [u8; 32] = o
                .service
                .as_slice()
                .try_into()
                .map_err(|_| VaultError::Corrupt("slot O service id".into()))?;
            self.shared
                .config
                .os_store
                .delete(&service, &o.label)
                .map_err(|e| {
                    VaultError::OsStoreUnavailable(format!(
                        "vault encrypted, but deleting the OS store copy of the data key failed: {e}"
                    ))
                })?;
        }
        Ok(status)
    }

    /// Unwraps the data key with `passphrase`. Failed attempts count toward
    /// the throttle.
    pub fn unlock(&self, passphrase: &[u8], scope: UnlockScope) -> Result<VaultStatus, VaultError> {
        let (dek, disk) = self.check_passphrase(passphrase)?;
        let mut inner = self.inner();
        inner.file = Some(disk);
        inner.install_key(dek, scope);
        Ok(self.status_of(&inner))
    }

    /// Drops the data key and revokes every grant. Idempotent. On an
    /// unencrypted vault this drops the cached key (it is re-read from the
    /// OS store on demand) and revokes grants; the state stays `Unencrypted`.
    pub fn lock(&self) -> VaultStatus {
        let mut inner = self.inner();
        if inner.dek.is_some() || !inner.grants.is_empty() {
            inner.forget_key();
        }
        self.status_of(&inner)
    }

    /// Re-wraps the data key under `new`; records (and the seed) are
    /// unchanged. The lock state is preserved.
    pub fn change_passphrase(&self, old: &[u8], new: &[u8]) -> Result<VaultStatus, VaultError> {
        let new = new_passphrase(new)?;
        let (dek, disk) = self.check_passphrase(old)?;
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) =
            crypto::derive_new_kek(&new, &salt, self.shared.config.kdf, self.production_network())?;
        let mut inner = self.inner();
        let mut next = disk;
        let aad = file::slot_p_aad(&next.vault_id, &next.network, &kdf, &salt);
        next.slot_p = Some(PassphraseSlot {
            kdf,
            salt: salt.to_vec(),
            wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
        });
        file::write(&self.shared.dir, &next)?;
        inner.file = Some(next);
        Ok(self.status_of(&inner))
    }

    /// Verifies `passphrase` against slot P of the file on disk. On success
    /// returns the data key and the disk file (throttle reset); on failure
    /// records the attempt.
    fn check_passphrase(&self, passphrase: &[u8]) -> Result<(Key32, VaultFile), VaultError> {
        let pw = normalize_passphrase(passphrase);
        let disk = file::read(&self.shared.dir)?.ok_or(VaultError::NoVault)?;
        let slot = disk.slot_p.clone().ok_or(VaultError::NotEncrypted)?;
        {
            let inner = self.inner();
            let throttle = inner.file.as_ref().map(|f| &f.throttle).unwrap_or(&disk.throttle);
            if let Some(secs) = retry_after(throttle, self.now()) {
                return Err(VaultError::Throttled {
                    retry_after_secs: secs,
                });
            }
        }

        let kek = crypto::derive_kek(&pw, &slot.salt, &slot.kdf)?;
        let aad = file::slot_p_aad(&disk.vault_id, &disk.network, &slot.kdf, &slot.salt);
        let Some(opened) = crypto::open(&kek, &slot.wrapped_dek, &aad) else {
            return Err(self.record_failure());
        };
        let dek: Key32 = Zeroizing::new(
            opened[..]
                .try_into()
                .map_err(|_| VaultError::Corrupt("data key length".into()))?,
        );

        let mut inner = self.inner();
        let manifest = verify_manifest(&disk, &dek, inner.high_water)?;
        inner.high_water = inner.high_water.max(manifest.generation);
        let mut disk = disk;
        let had_failures = inner
            .file
            .as_ref()
            .is_some_and(|f| f.throttle != Throttle::default())
            || disk.throttle != Throttle::default();
        if had_failures {
            disk.throttle = Throttle::default();
            file::write(&self.shared.dir, &disk)?;
            inner.file = Some(disk.clone());
        }
        Ok((dek, disk))
    }

    /// Persists one more failed attempt and returns the error to report.
    fn record_failure(&self) -> VaultError {
        let now = self.now();
        let mut inner = self.inner();
        let Some(f) = inner.file.as_mut() else {
            return VaultError::NoVault;
        };
        f.throttle.failed_attempts = f.throttle.failed_attempts.saturating_add(1);
        f.throttle.last_failure_at = Some(now.max(f.throttle.last_failure_at.unwrap_or(0)));
        let failed_attempts = f.throttle.failed_attempts;
        let retry_after_secs = retry_after(&f.throttle, now);
        let snapshot = f.clone();
        if let Err(e) = file::write(&self.shared.dir, &snapshot) {
            tracing::warn!(error = %e, "could not persist the failed-attempt counter");
        }
        VaultError::WrongPassphrase {
            failed_attempts,
            retry_after_secs,
        }
    }

    /// Checks `credential` and issues a single-use grant for `purpose`.
    pub fn authorize(&self, purpose: GrantPurpose, credential: Credential<'_>) -> Result<AuthGrant, VaultError> {
        match credential {
            // TODO(biometric): slot B (Touch ID / Windows Hello) lands in M2.
            Credential::QuickUnlock(_) => return Err(VaultError::QuickUnlockUnavailable),
            Credential::Passphrase(pw) => {
                {
                    let inner = self.inner();
                    let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
                    if f.slot_p.is_none() {
                        return Err(VaultError::NotEncrypted);
                    }
                }
                let (dek, disk) = self.check_passphrase(pw)?;
                let mut inner = self.inner();
                inner.file = Some(disk);
                inner.install_key(dek, UnlockScope::Full);
            }
            Credential::None => match self.lock_state() {
                LockState::NoVault => return Err(VaultError::NoVault),
                LockState::Locked => return Err(VaultError::Locked),
                LockState::UnlockedMixingOnly => return Err(VaultError::MixingOnly),
                LockState::NoKeys | LockState::Unencrypted => {
                    self.full_dek()?;
                }
                LockState::Unlocked => {}
            },
        }
        let id = hex::encode(crypto::random_array::<16>()?);
        let now = self.now();
        let grant = AuthGrant {
            id: id.clone(),
            purpose,
            expires_at: now.saturating_add(self.shared.config.grant_ttl_secs),
            single_use: true,
        };
        let mut inner = self.inner();
        inner.grants.retain(|_, g| g.expires_at >= now);
        inner.grants.insert(id, grant.clone());
        Ok(grant)
    }

    /// Invalidates a grant. Unknown ids are ignored.
    pub fn revoke_grant(&self, grant_id: &str) {
        self.inner().grants.remove(grant_id);
    }

    /// Checks and consumes a grant. A purpose mismatch leaves it in place.
    pub fn redeem_grant(&self, grant_id: &str, expected: GrantKind) -> Result<GrantToken, VaultError> {
        let now = self.now();
        let mut inner = self.inner();
        let grant = inner.grants.get(grant_id).cloned().ok_or(VaultError::GrantInvalid)?;
        if now > grant.expires_at {
            inner.grants.remove(grant_id);
            return Err(VaultError::GrantInvalid);
        }
        if grant.purpose.kind() != expected {
            return Err(VaultError::GrantPurposeMismatch);
        }
        if grant.single_use {
            inner.grants.remove(grant_id);
        }
        Ok(GrantToken {
            purpose: grant.purpose,
            epoch: inner.epoch,
        })
    }

    /// The data key with full scope: in memory after unlock, or read from
    /// the OS store for an unencrypted vault.
    fn full_dek(&self) -> Result<Key32, VaultError> {
        let (service, label) = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            if let Some(dek) = &inner.dek {
                if f.slot_p.is_some() && inner.scope == UnlockScope::MixingOnly {
                    return Err(VaultError::MixingOnly);
                }
                return Ok(Zeroizing::new(**dek));
            }
            match (&f.slot_p, &f.slot_o) {
                (None, Some(o)) => (o.service.clone(), o.label.clone()),
                (None, None) => return Err(VaultError::Corrupt("vault has no key slot".into())),
                (Some(_), _) => return Err(VaultError::Locked),
            }
        };
        let service: [u8; 32] = service
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Corrupt("slot O service id".into()))?;
        let raw = self
            .shared
            .config
            .os_store
            .get(&service, &label)?
            .ok_or_else(|| VaultError::Corrupt("the OS store has no data key for this vault".into()))?;
        let dek: Key32 = Zeroizing::new(
            raw[..]
                .try_into()
                .map_err(|_| VaultError::Corrupt("data key length".into()))?,
        );
        let mut inner = self.inner();
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        let manifest = verify_manifest(f, &dek, inner.high_water)
            .map_err(|_| VaultError::Corrupt("the OS store data key does not open this vault".into()))?;
        let unencrypted = f.slot_p.is_none();
        inner.high_water = inner.high_water.max(manifest.generation);
        if inner.dek.is_none() && unencrypted {
            inner.dek = Some(Zeroizing::new(*dek));
            inner.scope = UnlockScope::Full;
        }
        Ok(dek)
    }

    /// Applies record changes (`None` deletes), bumps the manifest
    /// generation, writes the file, then reads it back from disk and checks
    /// every change landed byte for byte (seed-safety ordering, iOS rule 3).
    fn commit_records(&self, dek: &Key32, changes: &[(String, Option<&[u8]>)]) -> Result<(), VaultError> {
        let mut inner = self.inner();
        let current = inner.file.clone().ok_or(VaultError::NoVault)?;
        let mut manifest = verify_manifest(&current, dek, inner.high_water)?;
        let mut next = current;
        for (id, value) in changes {
            match value {
                Some(v) => {
                    let sealed = crypto::seal(dek, v, &file::record_aad(&next.vault_id, &next.network, id))?;
                    manifest.records.insert(id.clone(), file::record_digest(&sealed));
                    next.records.insert(id.clone(), sealed);
                }
                None => {
                    manifest.records.remove(id);
                    next.records.remove(id);
                }
            }
        }
        manifest.generation += 1;
        next.manifest = seal_manifest(dek, &next.vault_id, &next.network, &manifest)?;
        file::write(&self.shared.dir, &next)?;
        inner.high_water = manifest.generation;
        inner.file = Some(next);

        let disk = file::read(&self.shared.dir)?
            .ok_or_else(|| VaultError::Corrupt("vault file vanished after write".into()))?;
        verify_manifest(&disk, dek, inner.high_water)?;
        for (id, value) in changes {
            let stored = disk.records.get(id);
            let matches = match (value, stored) {
                (Some(v), Some(sealed)) => crypto::open(dek, sealed, &file::record_aad(&disk.vault_id, &disk.network, id))
                    .is_some_and(|read| bool::from(read[..].ct_eq(v))),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                return Err(VaultError::Corrupt(format!("read-back of {id} did not match")));
            }
        }
        Ok(())
    }

    /// Decrypts one record after checking the manifest.
    fn read_record(&self, dek: &Key32, id: &str) -> Result<Option<Zeroizing<Vec<u8>>>, VaultError> {
        let inner = self.inner();
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        verify_manifest(f, dek, inner.high_water)?;
        let Some(sealed) = f.records.get(id) else {
            return Ok(None);
        };
        crypto::open(dek, sealed, &file::record_aad(&f.vault_id, &f.network, id))
            .map(Some)
            .ok_or_else(|| VaultError::Corrupt(format!("record {id} failed authentication")))
    }

    /// Stores a wallet's phrase, BIP39 passphrase and seed, then verifies the
    /// write by reading the file back. Needs the data key with full scope.
    pub fn store_wallet_secret(&self, wallet: &WalletId, secret: &WalletSecret) -> Result<(), VaultError> {
        let dek = self.full_dek()?;
        let seed = encode_seed(secret);
        self.commit_records(
            &dek,
            &[
                (record_id(wallet, REC_MNEMONIC), Some(&secret.mnemonic[..])),
                (record_id(wallet, REC_PASSPHRASE), Some(&secret.mnemonic_passphrase[..])),
                (record_id(wallet, REC_SEED), Some(&seed[..])),
            ],
        )
    }

    /// Deletes every record of `wallet`. `Ok(false)` when it had none.
    pub fn delete_wallet_secret(&self, wallet: &WalletId) -> Result<bool, VaultError> {
        let ids: Vec<String> = [REC_MNEMONIC, REC_PASSPHRASE, REC_SEED]
            .iter()
            .map(|k| record_id(wallet, k))
            .collect();
        let present = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            ids.iter().any(|id| f.records.contains_key(id))
        };
        if !present {
            return Ok(false);
        }
        let dek = self.full_dek()?;
        let changes: Vec<(String, Option<&[u8]>)> = ids.into_iter().map(|id| (id, None)).collect();
        self.commit_records(&dek, &changes)?;
        Ok(true)
    }

    /// Whether the vault holds a seed for `wallet`. In-memory read.
    pub fn has_wallet_secret(&self, wallet: &WalletId) -> bool {
        self.inner()
            .file
            .as_ref()
            .is_some_and(|f| f.records.contains_key(&record_id(wallet, REC_SEED)))
    }

    /// How the stored seed of `wallet` was derived. Needs full scope.
    pub fn seed_derivation(&self, wallet: &WalletId) -> Result<SeedDerivation, VaultError> {
        let dek = self.full_dek()?;
        let payload = self
            .read_record(&dek, &record_id(wallet, REC_SEED))?
            .ok_or(VaultError::NoSecret)?;
        Ok(decode_seed(&payload)?.1)
    }

    /// The recovery phrase and BIP39 passphrase (QT-113, IOS-006). Needs a
    /// `RevealSecret` grant.
    pub fn reveal_mnemonic(&self, wallet: &WalletId, grant_id: &str) -> Result<RevealedMnemonic, VaultError> {
        self.redeem_grant(grant_id, GrantKind::RevealSecret)?;
        let dek = self.full_dek()?;
        let phrase = self
            .read_record(&dek, &record_id(wallet, REC_MNEMONIC))?
            .ok_or(VaultError::NoSecret)?;
        let bip39_passphrase = self
            .read_record(&dek, &record_id(wallet, REC_PASSPHRASE))?
            .unwrap_or_default();
        Ok(RevealedMnemonic {
            phrase,
            bip39_passphrase,
        })
    }

    /// A signer for every derivation of `wallet`, authorized by a redeemed
    /// grant whose purpose signs (spend, message, masternode, governance,
    /// platform). The signer stops working when the vault locks.
    pub fn signer(&self, wallet: &WalletId, token: &GrantToken) -> Result<VaultSigner, VaultError> {
        if !token.purpose.signs() {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.full_dek()?;
        let inner = self.inner();
        if token.epoch != inner.epoch {
            return Err(VaultError::GrantInvalid);
        }
        self.signer_locked(&inner, wallet, SignerScope::Full)
    }

    /// A signer limited to the CoinJoin account (`m/9'/coin'/4'/…`). Works in
    /// every state with the data key available, including mixing-only
    /// unlock; needs no grant (mixing runs unattended).
    pub fn mixing_signer(&self, wallet: &WalletId) -> Result<VaultSigner, VaultError> {
        match self.lock_state() {
            LockState::NoVault => return Err(VaultError::NoVault),
            LockState::Locked => return Err(VaultError::Locked),
            LockState::NoKeys | LockState::Unencrypted => {
                self.full_dek()?;
            }
            LockState::Unlocked | LockState::UnlockedMixingOnly => {}
        }
        let inner = self.inner();
        self.signer_locked(&inner, wallet, SignerScope::CoinJoinOnly)
    }

    fn signer_locked(&self, inner: &Inner, wallet: &WalletId, scope: SignerScope) -> Result<VaultSigner, VaultError> {
        if inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        let has_seed = inner
            .file
            .as_ref()
            .is_some_and(|f| f.records.contains_key(&record_id(wallet, REC_SEED)));
        if !has_seed {
            return Err(VaultError::NoSecret);
        }
        Ok(VaultSigner::new(self.clone(), *wallet, scope, inner.epoch))
    }

    /// Seed of `wallet` for a signer issued at `epoch`.
    pub(crate) fn signing_seed(&self, wallet: &WalletId, epoch: u64) -> Result<Zeroizing<[u8; 64]>, SignerError> {
        let dek = {
            let inner = self.inner();
            if inner.epoch != epoch {
                return Err(SignerError::Locked);
            }
            Zeroizing::new(**inner.dek.as_ref().ok_or(SignerError::Locked)?)
        };
        let payload = self
            .read_record(&dek, &record_id(wallet, REC_SEED))?
            .ok_or(SignerError::NoSecret)?;
        Ok(decode_seed(&payload)?.0)
    }

    /// Biometric slot B. TODO(biometric): M2.
    pub fn enroll_quick_unlock(&self, _grant_id: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        Err(VaultError::NotImplemented("Vault.enroll_quick_unlock"))
    }

    /// Biometric slot B. TODO(biometric): M2.
    pub fn remove_quick_unlock(&self) -> Result<VaultStatus, VaultError> {
        Err(VaultError::NotImplemented("Vault.remove_quick_unlock"))
    }
}

fn seal_manifest(dek: &[u8; 32], vault_id: &[u8], network: &str, manifest: &Manifest) -> Result<Sealed, VaultError> {
    let plain = Zeroizing::new(
        serde_json::to_vec(manifest).map_err(|e| VaultError::Internal(format!("manifest: {e}")))?,
    );
    crypto::seal(dek, &plain, &file::manifest_aad(vault_id, network))
}

/// Decrypts the manifest and checks it lists exactly the records in the file
/// with matching digests, at a generation no older than `high_water`.
fn verify_manifest(f: &VaultFile, dek: &[u8; 32], high_water: u64) -> Result<Manifest, VaultError> {
    let plain = crypto::open(dek, &f.manifest, &file::manifest_aad(&f.vault_id, &f.network))
        .ok_or_else(|| VaultError::Corrupt("manifest failed authentication".into()))?;
    let manifest: Manifest = serde_json::from_slice(&plain)
        .map_err(|e| VaultError::Corrupt(format!("manifest does not parse: {e}")))?;
    if manifest.generation < high_water {
        return Err(VaultError::Corrupt(format!(
            "vault rolled back: generation {} < {high_water}",
            manifest.generation
        )));
    }
    let listed: Vec<&String> = manifest.records.keys().collect();
    let present: Vec<&String> = f.records.keys().collect();
    if listed != present {
        return Err(VaultError::Corrupt("record set differs from the manifest".into()));
    }
    for (id, sealed) in &f.records {
        if manifest.records.get(id) != Some(&file::record_digest(sealed)) {
            return Err(VaultError::Corrupt(format!("record {id} differs from the manifest")));
        }
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_follows_ios_schedule() {
        assert_eq!(throttle_wait_secs(0), 0);
        assert_eq!(throttle_wait_secs(2), 0);
        assert_eq!(throttle_wait_secs(3), 60);
        assert_eq!(throttle_wait_secs(4), 360);
        assert_eq!(throttle_wait_secs(5), 2160);
        assert!(throttle_wait_secs(u32::MAX) > 0);
    }

    #[test]
    fn retry_after_ignores_backwards_clock() {
        let t = Throttle {
            failed_attempts: 3,
            last_failure_at: Some(1000),
        };
        assert_eq!(retry_after(&t, 1000), Some(60));
        assert_eq!(retry_after(&t, 500), Some(60));
        assert_eq!(retry_after(&t, 1030), Some(30));
        assert_eq!(retry_after(&t, 1060), None);
    }

    #[test]
    fn seed_record_round_trip() {
        let secret = WalletSecret {
            mnemonic: Zeroizing::new(b"x".to_vec()),
            mnemonic_passphrase: Zeroizing::new(Vec::new()),
            seed: Zeroizing::new([7; 64]),
            derivation: SeedDerivation::DashCore {
                weak_checksum: true,
            },
        };
        let (seed, d) = decode_seed(&encode_seed(&secret)).unwrap();
        assert_eq!(*seed, [7; 64]);
        assert_eq!(d, secret.derivation);
        assert!(decode_seed(&[1, 2, 0]).is_err());
    }

    #[test]
    fn passphrase_normalization_is_nfc() {
        let nfd = "e\u{301}".as_bytes();
        let nfc = "\u{e9}".as_bytes();
        assert_eq!(*normalize_passphrase(nfd), *normalize_passphrase(nfc));
        assert_eq!(&normalize_passphrase(&[0xff, 0x00])[..], &[0xff, 0x00]);
        assert!(matches!(new_passphrase(b""), Err(VaultError::PassphraseRejected(_))));
    }

    #[test]
    fn seed_record_ids_parse() {
        let w = [0xab; 32];
        assert_eq!(seed_record_wallet(&record_id(&w, REC_SEED)), Some(w));
        assert_eq!(seed_record_wallet(&record_id(&w, REC_MNEMONIC)), None);
    }
}
