//! The vault of one network: data key, wrap slots, records, lock state and
//! grants (DESIGN-opus §1.8).
//!
//! Locking:
//! - `writer` serializes every change of the vault file and every passphrase
//!   check. It is held for the whole read-modify-write (`create`, `encrypt`,
//!   `change_passphrase`, record commits, throttle updates) and across the
//!   Argon2id derivation of a passphrase attempt, so two attempts never pass
//!   the throttle check together. `inner.file` changes only under `writer`,
//!   and each write starts from `inner.file` and changes only its own part.
//!   The file on disk is read only at open and for the post-write check.
//! - `inner` is a short-held mutex over the in-memory state. Argon2id, OS
//!   store calls and file writes run outside it.
//! - `ops` is the operation gate. Every operation that turns the data key
//!   into a secret-derived result (a signer call, a revealed phrase or seed)
//!   holds it shared from its epoch check until that result exists. Every
//!   epoch change (lock, unlock, scope change, encrypt, recover, destroy)
//!   holds it exclusively, so `lock()` returns only once every operation
//!   that started before it has finished, and any operation after it sees
//!   the new epoch and fails `Locked`. A holder must not take it again, nor
//!   take `writer`: a waiting epoch change blocks new shared holders.
//!
//! Lock order: `writer`, then `ops`, then `inner`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use key_wallet::Network;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use serde::{Deserialize, Serialize};

use crate::crypto::{self, Key32, SALT_LEN, Sealed};
use crate::file::{
    self, Manifest, OsSlot, PassphraseSlot, QuickUnlockSettings, QuickUnlockSlot, Throttle,
    VaultFile,
};
use crate::mnemonic;
use crate::signer::{SignerScope, VaultSigner};
use crate::types::{
    AuthGrant, Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantKind, GrantPurpose, GrantToken,
    LockState, PASSPHRASE_MAX_AGE_SECS, QUICK_UNLOCK_SPEND_LIMITS, QuickUnlockPolicy,
    RevealedMnemonic, SeedDerivation, UnlockScope, VaultConfig, VaultStatus, WalletId,
    WalletSecret,
};
use crate::{SignerError, VaultError};

mod compat;
mod masternode;
use compat::BackupKek;
pub use compat::{CoreMnemonicCheck, WalletBackupBundle, reads_bundle_version};

/// Proof that the caller holds `Shared::writer`.
type WriteGuard<'a> = MutexGuard<'a, ()>;
/// Proof that the caller holds `Shared::ops` shared: one operation that
/// produces a secret-derived result (see the module doc).
pub(crate) type OpGuard<'a> = RwLockReadGuard<'a, ()>;
/// Proof that the caller holds `Shared::ops` exclusively, which every epoch
/// change needs.
type EpochGuard<'a> = RwLockWriteGuard<'a, ()>;

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
/// `6^(n−3)·60 s` from the third failure on (IOS-012). UX throttling only:
/// the counter is stored in the vault file outside the AEAD, so whoever can
/// edit the file can reset it. Argon2id cost is the real protection.
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
        SeedDerivation::DashCore { weak_checksum } => {
            v.extend_from_slice(&[1, weak_checksum as u8])
        }
        SeedDerivation::RawSeed => v.extend_from_slice(&[2, 0]),
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
        (2, 0) => SeedDerivation::RawSeed,
        _ => return Err(VaultError::Corrupt("seed record derivation".into())),
    };
    let mut seed = Zeroizing::new([0u8; 64]);
    seed.copy_from_slice(&payload[3..]);
    Ok((seed, derivation))
}

/// The authenticated payload of [`QuickUnlockSettings::sealed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PolicyPayload {
    spend_limit_duffs: u64,
    last_passphrase_at: Option<u64>,
}

impl Default for PolicyPayload {
    fn default() -> Self {
        Self {
            spend_limit_duffs: DEFAULT_QUICK_UNLOCK_SPEND_LIMIT,
            last_passphrase_at: None,
        }
    }
}

/// Seals `payload` under the data key and returns the file section with
/// the matching plain copy.
fn seal_policy(
    dek: &[u8; 32],
    vault_id: &[u8],
    network: &str,
    payload: PolicyPayload,
) -> Result<QuickUnlockSettings, VaultError> {
    let plain = Zeroizing::new(
        serde_json::to_vec(&payload)
            .map_err(|e| VaultError::Internal(format!("quick-unlock policy: {e}")))?,
    );
    Ok(QuickUnlockSettings {
        spend_limit_duffs: payload.spend_limit_duffs,
        last_passphrase_at: payload.last_passphrase_at,
        sealed: crypto::seal(dek, &plain, &file::quick_unlock_aad(vault_id, network))?,
    })
}

/// The authenticated policy of `f`; `None` when the file has none. A plain
/// copy that differs from the sealed one is `Corrupt`.
fn open_policy(f: &VaultFile, dek: &[u8; 32]) -> Result<Option<PolicyPayload>, VaultError> {
    let Some(settings) = &f.quick_unlock else {
        return Ok(None);
    };
    let plain = crypto::open(
        dek,
        &settings.sealed,
        &file::quick_unlock_aad(&f.vault_id, &f.network),
    )
    .ok_or_else(|| VaultError::Corrupt("quick-unlock policy failed authentication".into()))?;
    let payload: PolicyPayload = serde_json::from_slice(&plain)
        .map_err(|e| VaultError::Corrupt(format!("quick-unlock policy does not parse: {e}")))?;
    if payload.spend_limit_duffs != settings.spend_limit_duffs
        || payload.last_passphrase_at != settings.last_passphrase_at
    {
        return Err(VaultError::Corrupt(
            "quick-unlock policy differs from its sealed copy".into(),
        ));
    }
    Ok(Some(payload))
}

/// A grant held in vault memory.
struct IssuedGrant {
    grant: AuthGrant,
    /// The data key, when a passphrase authorized this grant on a vault that
    /// held no full-scope key (locked or mixing-only). Only this grant uses
    /// it; the vault's lock state is unchanged (dash-qt re-lock parity).
    key: Option<Key32>,
}

struct Inner {
    /// The vault file as last written (or read at open). Changes only under
    /// `Shared::writer`.
    file: Option<VaultFile>,
    dek: Option<Key32>,
    scope: UnlockScope,
    /// Bumped whenever the vault locks or the unlock scope changes. Grants,
    /// grant tokens and signers from an older epoch are refused.
    epoch: u64,
    /// Highest manifest generation seen by this process (rollback check).
    high_water: u64,
    grants: HashMap<String, IssuedGrant>,
    /// Last successful passphrase check in this process (UNIX seconds). The
    /// file keeps it (sealed) only while slot B is enrolled.
    last_passphrase_at: Option<u64>,
    /// The key that wraps a backup's own key for slot P's passphrase
    /// (`.dwbackup` bundle v2), derived from slot P's KEK whenever the
    /// passphrase is checked or set. Held only while `dek` is; never stored.
    backup_kek: Option<BackupKek>,
}

impl Inner {
    /// Drops the data key, revokes grants and invalidates outstanding signers.
    fn forget_key(&mut self, _ops: &EpochGuard<'_>) {
        self.dek = None;
        self.backup_kek = None;
        self.scope = UnlockScope::Full;
        self.epoch += 1;
        self.grants.clear();
    }

    /// Drops grants that expired before `now`, together with any key they
    /// hold, so a grant's own copy of the data key does not outlive the grant.
    fn drop_expired_grants(&mut self, now: u64) {
        self.grants.retain(|_, g| g.grant.expires_at >= now);
    }

    /// Installs a data key with `scope`; bumps the epoch when the state changes.
    fn install_key(&mut self, dek: Key32, scope: UnlockScope, _ops: &EpochGuard<'_>) {
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
    /// Serializes file changes and passphrase checks (see the module doc).
    writer: Mutex<()>,
    /// The operation gate (see the module doc).
    ops: RwLock<()>,
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
                writer: Mutex::new(()),
                ops: RwLock::new(()),
                inner: Mutex::new(Inner {
                    file,
                    dek: None,
                    scope: UnlockScope::Full,
                    epoch: 0,
                    high_water: 0,
                    grants: HashMap::new(),
                    last_passphrase_at: None,
                    backup_kek: None,
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

    fn writer(&self) -> WriteGuard<'_> {
        self.shared
            .writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Opens one operation (shared hold of the gate; see the module doc).
    pub(crate) fn op_guard(&self) -> OpGuard<'_> {
        self.shared
            .ops
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Waits for every open operation, then excludes new ones until the
    /// guard drops: the right to change the epoch.
    fn epoch_guard(&self) -> EpochGuard<'_> {
        self.shared
            .ops
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A copy of the in-memory file to change under `writer`.
    fn file_copy(&self, _writer: &WriteGuard<'_>) -> Result<VaultFile, VaultError> {
        self.inner().file.clone().ok_or(VaultError::NoVault)
    }

    /// Writes `next` to disk, then makes it the in-memory file. `next` was
    /// built from [`Self::file_copy`] under the same `writer` guard.
    fn persist(&self, _writer: &WriteGuard<'_>, next: VaultFile) -> Result<(), VaultError> {
        file::write(&self.shared.dir, &next)?;
        self.inner().file = Some(next);
        Ok(())
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
            quick_unlock_enrolled: f.slot_b.is_some(),
            failed_attempts: f.throttle.failed_attempts,
            retry_after_secs: retry_after(&f.throttle, self.now()),
            wallets_with_secrets: f
                .records
                .keys()
                .filter_map(|k| seed_record_wallet(k))
                .collect(),
        }
    }

    /// Current state. In-memory read; also drops expired grants.
    pub fn status(&self) -> VaultStatus {
        let now = self.now();
        let mut inner = self.inner();
        inner.drop_expired_grants(now);
        self.status_of(&inner)
    }

    /// Current lock state. In-memory read; also drops expired grants.
    pub fn lock_state(&self) -> LockState {
        let now = self.now();
        let mut inner = self.inner();
        inner.drop_expired_grants(now);
        Self::state_of(&inner)
    }

    /// Drops expired grants and the keys they hold. [`Self::status`],
    /// [`Self::lock_state`] and every grant issue, check or redemption do
    /// this too; the engine also calls it every second while a network is
    /// open, so an unused grant's key is not kept past the grant's lifetime.
    pub fn purge_expired_grants(&self) {
        let now = self.now();
        self.inner().drop_expired_grants(now);
    }

    /// Creates the vault. `Some(passphrase)` = encrypted (slot P);
    /// `None` = unencrypted (data key in the OS store, slot O). Leaves the
    /// vault unlocked.
    pub fn create(&self, passphrase: Option<&[u8]>) -> Result<VaultStatus, VaultError> {
        let writer = self.writer();
        if self.inner().file.is_some() || file::read(&self.shared.dir)?.is_some() {
            return Err(VaultError::AlreadyExists);
        }
        let dek = crypto::random_key()?;
        let vault_id: [u8; 16] = crypto::random_array()?;
        let tag = &self.shared.tag;
        let mut backup_kek = None;
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
                backup_kek = Some(BackupKek::of(kdf, &salt, &kek));
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
                self.shared
                    .config
                    .os_store
                    .put(&service, OS_LABEL, &dek[..])?;
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
            slot_b: None,
            quick_unlock: None,
        };

        if let Err(e) = file::write(&self.shared.dir, &vault_file) {
            if let Some(o) = &vault_file.slot_o
                && let Ok(service) = <[u8; 32]>::try_from(o.service.as_slice())
                && let Err(cleanup) = self.shared.config.os_store.delete(&service, &o.label)
            {
                tracing::warn!(error = %cleanup, "could not remove the data key of a vault that was not created");
            }
            return Err(e);
        }
        let encrypted = vault_file.slot_p.is_some();
        let now = self.now();
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.file = Some(vault_file);
        inner.high_water = 1;
        inner.install_key(dek, UnlockScope::Full, &ops);
        inner.backup_kek = backup_kek;
        if encrypted {
            inner.last_passphrase_at = Some(now);
        }
        drop(writer);
        Ok(self.status_of(&inner))
    }

    /// dash-qt "Encrypt Wallet" (QT-111): adds slot P and deletes slot O.
    /// Needs a `ChangeCredential` grant. Leaves the vault locked, as Core
    /// does after `encryptwallet`. There is no way back to unencrypted.
    pub fn encrypt(
        &self,
        new_passphrase_bytes: &[u8],
        grant_id: &str,
    ) -> Result<VaultStatus, VaultError> {
        let pw = new_passphrase(new_passphrase_bytes)?;
        let writer = self.writer();
        let mut next = self.file_copy(&writer)?;
        if next.slot_p.is_some() {
            return Err(VaultError::AlreadyEncrypted);
        }
        let token = self.redeem_grant(grant_id, GrantKind::ChangeCredential, None)?;
        let dek = self.key_for(&token)?;
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) = crypto::derive_new_kek(
            &pw,
            &salt,
            self.shared.config.kdf,
            self.production_network(),
        )?;
        let aad = file::slot_p_aad(&next.vault_id, &next.network, &kdf, &salt);
        next.slot_p = Some(PassphraseSlot {
            kdf,
            salt: salt.to_vec(),
            wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
        });
        let old_slot = next.slot_o.take();
        self.persist(&writer, next)?;
        let status = {
            let ops = self.epoch_guard();
            let mut inner = self.inner();
            inner.forget_key(&ops);
            self.status_of(&inner)
        };
        drop(writer);

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
        let writer = self.writer();
        let (dek, backup_kek) = self.check_passphrase(&writer, passphrase)?;
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.install_key(dek, scope, &ops);
        inner.backup_kek = Some(backup_kek);
        Ok(self.status_of(&inner))
    }

    /// Drops the data key, revokes every grant and invalidates every
    /// outstanding grant token and signer, including those that carry a
    /// grant's own key. Idempotent. On an unencrypted vault this drops the
    /// cached key (it is re-read from the OS store on demand); the state
    /// stays `Unencrypted`.
    ///
    /// Waits for the signer operations and reveals already running (a
    /// signature takes about a millisecond), so once it returns no
    /// signature, shared secret, ciphertext, exported key or phrase that
    /// the old epoch allowed is still being made.
    pub fn lock(&self) -> VaultStatus {
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.forget_key(&ops);
        self.status_of(&inner)
    }

    /// Re-wraps the data key under `new`; records (and the seed) are
    /// unchanged. The lock state is preserved.
    ///
    /// The data key is not rotated (review H2): `.dwbackup` bundles carry a
    /// key of their own and nothing that unwraps the data key, so an old
    /// backup and the old passphrase open only that backup. Rotating would
    /// silently break slot B (its wrap key is only in the OS biometric
    /// store) and every automatic `vault_key` backup, and Dash Core's
    /// `walletpassphrasechange` keeps its master key too. What remains is
    /// inherent to any copy: an old copy of `vault.dwv` still opens with
    /// the old passphrase.
    pub fn change_passphrase(&self, old: &[u8], new: &[u8]) -> Result<VaultStatus, VaultError> {
        let new = new_passphrase(new)?;
        let writer = self.writer();
        let (dek, _) = self.check_passphrase(&writer, old)?;
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) = crypto::derive_new_kek(
            &new,
            &salt,
            self.shared.config.kdf,
            self.production_network(),
        )?;
        let mut next = self.file_copy(&writer)?;
        let aad = file::slot_p_aad(&next.vault_id, &next.network, &kdf, &salt);
        next.slot_p = Some(PassphraseSlot {
            kdf,
            salt: salt.to_vec(),
            wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
        });
        self.persist(&writer, next)?;
        {
            // Backups written from now on open with the new passphrase.
            let mut inner = self.inner();
            if inner.dek.is_some() {
                inner.backup_kek = Some(BackupKek::of(kdf, &salt, &kek));
            }
        }
        Ok(self.status())
    }

    /// Verifies `passphrase` against slot P of the in-memory file and checks
    /// the manifest with the unwrapped key. On success resets the throttle
    /// and returns the data key and the backup KEK of slot P; on failure
    /// records the attempt. Runs under
    /// `writer`, so attempts are serialized and each one sees the throttle
    /// the previous one left.
    fn check_passphrase(
        &self,
        writer: &WriteGuard<'_>,
        passphrase: &[u8],
    ) -> Result<(Key32, BackupKek), VaultError> {
        let pw = normalize_passphrase(passphrase);
        let (slot, aad) = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let slot = f.slot_p.clone().ok_or(VaultError::NotEncrypted)?;
            if let Some(secs) = retry_after(&f.throttle, self.now()) {
                return Err(VaultError::Throttled {
                    retry_after_secs: secs,
                });
            }
            let aad = file::slot_p_aad(&f.vault_id, &f.network, &slot.kdf, &slot.salt);
            (slot, aad)
        };

        let kek = crypto::derive_kek(&pw, &slot.salt, &slot.kdf)?;
        let Some(opened) = crypto::open(&kek, &slot.wrapped_dek, &aad) else {
            return Err(self.record_failure(writer));
        };
        let dek: Key32 = Zeroizing::new(
            opened[..]
                .try_into()
                .map_err(|_| VaultError::Corrupt("data key length".into()))?,
        );

        let now = self.now();
        let (had_failures, policy) = {
            let mut inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let manifest = verify_manifest(f, &dek, inner.high_water)?;
            let had_failures = f.throttle != Throttle::default();
            // With slot B enrolled the passphrase time is enforced from the
            // file, so it is persisted (sealed) on every successful check.
            let policy = if f.slot_b.is_some() {
                Some(open_policy(f, &dek)?.unwrap_or_default())
            } else {
                None
            };
            inner.high_water = inner.high_water.max(manifest.generation);
            inner.last_passphrase_at = Some(now);
            (had_failures, policy)
        };
        if had_failures || policy.is_some() {
            let mut next = self.file_copy(writer)?;
            next.throttle = Throttle::default();
            if let Some(policy) = policy {
                next.quick_unlock = Some(seal_policy(
                    &dek,
                    &next.vault_id,
                    &next.network,
                    PolicyPayload {
                        last_passphrase_at: Some(now),
                        ..policy
                    },
                )?);
            }
            // The passphrase is already proven: a failed write of the reset
            // counter (or the sealed passphrase time) must not refuse it (a
            // full disk would otherwise lock the user out). The new file is
            // kept in memory and the write is retried by the next file
            // change, which starts from memory.
            if let Err(e) = file::write(&self.shared.dir, &next) {
                tracing::warn!(error = %e, "could not persist the reset failed-attempt counter");
            }
            self.inner().file = Some(next);
        }
        Ok((dek, BackupKek::of(slot.kdf, &slot.salt, &kek)))
    }

    /// Persists one more failed attempt and returns the error to report. The
    /// in-memory counter counts the attempt even when the write fails.
    fn record_failure(&self, writer: &WriteGuard<'_>) -> VaultError {
        let now = self.now();
        let mut next = match self.file_copy(writer) {
            Ok(f) => f,
            Err(e) => return e,
        };
        let t = &mut next.throttle;
        t.failed_attempts = t.failed_attempts.saturating_add(1);
        t.last_failure_at = Some(now.max(t.last_failure_at.unwrap_or(0)));
        let failed_attempts = t.failed_attempts;
        let retry_after_secs = retry_after(t, now);
        if let Err(e) = file::write(&self.shared.dir, &next) {
            tracing::warn!(error = %e, "could not persist the failed-attempt counter");
        }
        self.inner().file = Some(next);
        VaultError::WrongPassphrase {
            failed_attempts,
            retry_after_secs,
        }
    }

    /// Checks `credential` and issues a single-use grant for `purpose`.
    ///
    /// `wallet` binds the grant: wallet-scoped purposes
    /// ([`GrantPurpose::wallet_scoped`]) need the wallet the grant is for and
    /// are refused for any other; `ChangeCredential` takes `None`.
    ///
    /// Credentials:
    /// - `Passphrase`: checked against slot P (throttled). The lock state does
    ///   not change: on a locked or mixing-only vault the unwrapped key is
    ///   kept for this grant only and dropped when it is used, expires, is
    ///   revoked or the vault locks.
    /// - `None`: accepted on an unencrypted vault, and on a vault unlocked
    ///   with scope Full except for [`GrantPurpose::requires_credential`]
    ///   purposes, which need the passphrase whenever slot P exists.
    /// - `QuickUnlock`: the slot B wrap key. Issues `Spend` grants up to the
    ///   spending limit (`QuickUnlockLimitExceeded` above it) and
    ///   `SignMessage` grants, only while the passphrase was entered within
    ///   [`PASSPHRASE_MAX_AGE_SECS`] (`PassphraseStale`). Every other purpose
    ///   is `CredentialRequired`. The lock state does not change; on a locked
    ///   or mixing-only vault the grant carries its own key, as a passphrase
    ///   grant does.
    pub fn authorize(
        &self,
        purpose: GrantPurpose,
        wallet: Option<&WalletId>,
        credential: Credential<'_>,
    ) -> Result<AuthGrant, VaultError> {
        match (purpose.wallet_scoped(), wallet) {
            (true, None) => {
                return Err(VaultError::InvalidArgument(format!(
                    "a {:?} grant needs a wallet id",
                    purpose.kind()
                )));
            }
            (false, Some(_)) => {
                return Err(VaultError::InvalidArgument(format!(
                    "a {:?} grant is not bound to a wallet",
                    purpose.kind()
                )));
            }
            _ => {}
        }
        match credential {
            Credential::QuickUnlock(wrap_key) => {
                self.authorize_quick_unlock(purpose, wallet, wrap_key)
            }
            Credential::Passphrase(pw) => {
                let writer = self.writer();
                let (dek, _) = self.check_passphrase(&writer, pw)?;
                let mut inner = self.inner();
                let full_in_memory = inner.dek.is_some() && inner.scope == UnlockScope::Full;
                self.issue(
                    &mut inner,
                    purpose,
                    wallet,
                    (!full_in_memory).then_some(dek),
                )
            }
            Credential::None => {
                if matches!(
                    self.lock_state(),
                    LockState::NoKeys | LockState::Unencrypted
                ) {
                    // Fails when the OS store cannot produce the data key.
                    self.full_dek()?;
                }
                let mut inner = self.inner();
                match Self::state_of(&inner) {
                    LockState::NoVault => return Err(VaultError::NoVault),
                    LockState::Locked | LockState::UnlockedMixingOnly | LockState::Unlocked
                        if purpose.requires_credential() =>
                    {
                        return Err(VaultError::CredentialRequired);
                    }
                    LockState::Locked => return Err(VaultError::Locked),
                    LockState::UnlockedMixingOnly => return Err(VaultError::MixingOnly),
                    LockState::NoKeys | LockState::Unencrypted | LockState::Unlocked => {}
                }
                self.issue(&mut inner, purpose, wallet, None)
            }
        }
    }

    /// The `QuickUnlock` arm of [`Self::authorize`].
    fn authorize_quick_unlock(
        &self,
        purpose: GrantPurpose,
        wallet: Option<&WalletId>,
        wrap_key: &[u8],
    ) -> Result<AuthGrant, VaultError> {
        if !matches!(
            purpose,
            GrantPurpose::Spend { .. } | GrantPurpose::SignMessage
        ) {
            return Err(VaultError::CredentialRequired);
        }
        let now = self.now();
        let mut inner = self.inner();
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        let slot = f
            .slot_b
            .as_ref()
            .ok_or(VaultError::QuickUnlockUnavailable)?;
        let key: Key32 = Zeroizing::new(wrap_key.try_into().map_err(|_| {
            VaultError::InvalidArgument("the quick-unlock key must be 32 bytes".into())
        })?);
        let opened = crypto::open(
            &key,
            &slot.wrapped_dek,
            &file::slot_b_aad(&f.vault_id, &f.network),
        )
        .ok_or(VaultError::QuickUnlockUnavailable)?;
        let dek: Key32 = Zeroizing::new(
            opened[..]
                .try_into()
                .map_err(|_| VaultError::Corrupt("data key length".into()))?,
        );
        let manifest = verify_manifest(f, &dek, inner.high_water)?;
        let policy = open_policy(f, &dek)?.unwrap_or_default();
        inner.high_water = inner.high_water.max(manifest.generation);

        // A clock set back gives age 0 (fresh); one set forward only stales.
        let fresh = policy
            .last_passphrase_at
            .is_some_and(|at| now.saturating_sub(at) <= PASSPHRASE_MAX_AGE_SECS);
        if !fresh {
            return Err(VaultError::PassphraseStale);
        }
        if let GrantPurpose::Spend { max_duffs } = purpose
            && max_duffs > policy.spend_limit_duffs
        {
            return Err(VaultError::QuickUnlockLimitExceeded {
                limit_duffs: policy.spend_limit_duffs,
            });
        }
        let full_in_memory = inner.dek.is_some() && inner.scope == UnlockScope::Full;
        self.issue(
            &mut inner,
            purpose,
            wallet,
            (!full_in_memory).then_some(dek),
        )
    }

    /// Stores a new grant, dropping expired ones.
    fn issue(
        &self,
        inner: &mut Inner,
        purpose: GrantPurpose,
        wallet: Option<&WalletId>,
        key: Option<Key32>,
    ) -> Result<AuthGrant, VaultError> {
        let id = hex::encode(crypto::random_array::<16>()?);
        let now = self.now();
        let grant = AuthGrant {
            id: id.clone(),
            purpose,
            wallet: wallet.copied(),
            expires_at: now.saturating_add(self.shared.config.grant_ttl_secs),
            single_use: true,
        };
        inner.drop_expired_grants(now);
        inner.grants.insert(
            id,
            IssuedGrant {
                grant: grant.clone(),
                key,
            },
        );
        Ok(grant)
    }

    /// Invalidates a grant. Unknown ids are ignored.
    pub fn revoke_grant(&self, grant_id: &str) {
        self.inner().grants.remove(grant_id);
    }

    /// Checks that `grant_id` is live, of kind `expected` and bound to
    /// `wallet` (`None` for vault-wide purposes).
    fn valid_grant<'a>(
        inner: &'a mut Inner,
        grant_id: &str,
        expected: GrantKind,
        wallet: Option<&WalletId>,
        now: u64,
    ) -> Result<&'a IssuedGrant, VaultError> {
        inner.drop_expired_grants(now);
        let issued = inner.grants.get(grant_id).ok_or(VaultError::GrantInvalid)?;
        if issued.grant.purpose.kind() != expected || issued.grant.wallet.as_ref() != wallet {
            return Err(VaultError::GrantPurposeMismatch);
        }
        Ok(issued)
    }

    /// Checks a grant without consuming it: it exists, has not expired, is of
    /// kind `expected`, is bound to `wallet`, and a key is available to use it
    /// (its own key, or the vault's). Lets a caller refuse early, before work
    /// that only the grant's redemption would otherwise reject.
    pub fn check_grant(
        &self,
        grant_id: &str,
        expected: GrantKind,
        wallet: Option<&WalletId>,
    ) -> Result<(), VaultError> {
        let now = self.now();
        let mut inner = self.inner();
        let state = Self::state_of(&inner);
        let own_key = Self::valid_grant(&mut inner, grant_id, expected, wallet, now)
            .map(|issued| issued.key.is_some());
        match own_key {
            Ok(true) => return Ok(()),
            // A missing grant on a locked vault was most likely revoked by
            // the lock, so the lock is reported first.
            Ok(false) | Err(VaultError::GrantInvalid) => match state {
                LockState::NoVault => return Err(VaultError::NoVault),
                LockState::Locked => return Err(VaultError::Locked),
                LockState::UnlockedMixingOnly => return Err(VaultError::MixingOnly),
                _ => {}
            },
            Err(_) => {}
        }
        own_key.map(|_| ())
    }

    /// Checks and consumes a grant. `wallet` is the wallet the call acts on
    /// (`None` for vault-wide purposes); a grant bound to another wallet, or
    /// issued for another purpose, is refused with `GrantPurposeMismatch` and
    /// left in place.
    pub fn redeem_grant(
        &self,
        grant_id: &str,
        expected: GrantKind,
        wallet: Option<&WalletId>,
    ) -> Result<GrantToken, VaultError> {
        let now = self.now();
        let mut inner = self.inner();
        Self::valid_grant(&mut inner, grant_id, expected, wallet, now)?;
        let issued = inner
            .grants
            .remove(grant_id)
            .ok_or(VaultError::GrantInvalid)?;
        Ok(GrantToken {
            purpose: issued.grant.purpose,
            wallet: issued.grant.wallet,
            epoch: inner.epoch,
            key: issued.key,
        })
    }

    /// The full-scope data key a redeemed grant acts with: the grant's own
    /// key, or the vault's. Refused once the vault locked after redemption.
    fn key_for(&self, token: &GrantToken) -> Result<Key32, VaultError> {
        if self.inner().epoch != token.epoch {
            return Err(VaultError::Locked);
        }
        match &token.key {
            Some(key) => Ok(Zeroizing::new(**key)),
            None => self.full_dek(),
        }
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
            .ok_or_else(|| {
                VaultError::Corrupt("the OS store has no data key for this vault".into())
            })?;
        let dek: Key32 = Zeroizing::new(
            raw[..]
                .try_into()
                .map_err(|_| VaultError::Corrupt("data key length".into()))?,
        );
        let mut inner = self.inner();
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        let manifest = verify_manifest(f, &dek, inner.high_water).map_err(|_| {
            VaultError::Corrupt("the OS store data key does not open this vault".into())
        })?;
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
    /// Holds `writer` throughout, so no other change of the file interleaves.
    fn commit_records(
        &self,
        dek: &Key32,
        changes: &[(String, Option<&[u8]>)],
    ) -> Result<(), VaultError> {
        let writer = self.writer();
        let mut next = self.file_copy(&writer)?;
        let mut manifest = verify_manifest(&next, dek, self.inner().high_water)?;
        for (id, value) in changes {
            match value {
                Some(v) => {
                    let sealed =
                        crypto::seal(dek, v, &file::record_aad(&next.vault_id, &next.network, id))?;
                    manifest
                        .records
                        .insert(id.clone(), file::record_digest(&sealed));
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
        self.persist(&writer, next)?;
        let high_water = {
            let mut inner = self.inner();
            inner.high_water = inner.high_water.max(manifest.generation);
            inner.high_water
        };

        let disk = file::read(&self.shared.dir)?
            .ok_or_else(|| VaultError::Corrupt("vault file vanished after write".into()))?;
        verify_manifest(&disk, dek, high_water)?;
        for (id, value) in changes {
            let stored = disk.records.get(id);
            let matches = match (value, stored) {
                (Some(v), Some(sealed)) => crypto::open(
                    dek,
                    sealed,
                    &file::record_aad(&disk.vault_id, &disk.network, id),
                )
                .is_some_and(|read| bool::from(read[..].ct_eq(v))),
                (None, None) => true,
                _ => false,
            };
            if !matches {
                return Err(VaultError::Corrupt(format!(
                    "read-back of {id} did not match"
                )));
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
    pub fn store_wallet_secret(
        &self,
        wallet: &WalletId,
        secret: &WalletSecret,
    ) -> Result<(), VaultError> {
        let dek = self.full_dek()?;
        let seed = encode_seed(secret);
        // A raw seed has no phrase: its wallet stores the seed record only.
        let phrase = secret.derivation != SeedDerivation::RawSeed;
        self.commit_records(
            &dek,
            &[
                (
                    record_id(wallet, REC_MNEMONIC),
                    phrase.then_some(&secret.mnemonic[..]),
                ),
                (
                    record_id(wallet, REC_PASSPHRASE),
                    phrase.then_some(&secret.mnemonic_passphrase[..]),
                ),
                (record_id(wallet, REC_SEED), Some(&seed[..])),
            ],
        )
    }

    /// Deletes every record of `wallet` with the vault's full-scope key.
    /// `Ok(false)` when it had none. Used to roll back a failed import; user
    /// removal goes through [`Self::wipe_wallet_secret`].
    pub fn delete_wallet_secret(&self, wallet: &WalletId) -> Result<bool, VaultError> {
        if !self.has_any_record(wallet)? {
            return Ok(false);
        }
        let dek = self.full_dek()?;
        self.delete_records(wallet, &dek)?;
        Ok(true)
    }

    /// Deletes every record of `wallet` under a redeemed `Wipe` grant for that
    /// wallet, with the grant's key when it carries one (a passphrase grant on
    /// a locked vault). `Ok(false)` when it had none.
    pub fn wipe_wallet_secret(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<bool, VaultError> {
        if token.purpose.kind() != GrantKind::Wipe || token.wallet.as_ref() != Some(wallet) {
            return Err(VaultError::GrantPurposeMismatch);
        }
        if !self.has_any_record(wallet)? {
            return Ok(false);
        }
        let dek = self.key_for(token)?;
        self.delete_records(wallet, &dek)?;
        Ok(true)
    }

    fn wallet_record_ids(wallet: &WalletId) -> Vec<String> {
        [REC_MNEMONIC, REC_PASSPHRASE, REC_SEED]
            .iter()
            .map(|k| record_id(wallet, k))
            .collect()
    }

    fn has_any_record(&self, wallet: &WalletId) -> Result<bool, VaultError> {
        let inner = self.inner();
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        Ok(Self::wallet_record_ids(wallet)
            .iter()
            .any(|id| f.records.contains_key(id)))
    }

    fn delete_records(&self, wallet: &WalletId, dek: &Key32) -> Result<(), VaultError> {
        let changes: Vec<(String, Option<&[u8]>)> = Self::wallet_record_ids(wallet)
            .into_iter()
            .map(|id| (id, None))
            .collect();
        self.commit_records(dek, &changes)
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
    /// `RevealSecret` grant bound to `wallet`.
    pub fn reveal_mnemonic(
        &self,
        wallet: &WalletId,
        grant_id: &str,
    ) -> Result<RevealedMnemonic, VaultError> {
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        let _op = self.op_guard();
        let dek = self.key_for(&token)?;
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
    /// grant for that wallet whose purpose signs (spend, message). A grant
    /// authorized by passphrase on a locked or mixing-only vault hands its
    /// own key to the signer. The signer stops working when the vault locks
    /// or changes unlock scope. A `PlatformOp` grant gets scoped signers
    /// only ([`Self::platform_signer`]).
    pub fn signer(&self, wallet: &WalletId, token: &GrantToken) -> Result<VaultSigner, VaultError> {
        if !token.purpose.signs() {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.token_signer(wallet, token, SignerScope::Full)
    }

    /// A signer limited to one Platform scope (DASHPAY §3.3):
    /// [`SignerScope::PlatformIdentity`], [`SignerScope::DashPayCrypto`] or
    /// [`SignerScope::PlatformFunding`], authorized by a redeemed
    /// `PlatformOp` grant for `wallet`. One token may issue several (a
    /// registration needs identity and funding signers). Any other scope is
    /// `InvalidArgument`; any other grant `GrantPurposeMismatch`. Lifetime
    /// and own-key rules are those of [`Self::signer`].
    pub fn platform_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        if !matches!(
            scope,
            SignerScope::PlatformIdentity
                | SignerScope::DashPayCrypto
                | SignerScope::PlatformFunding { .. }
        ) {
            return Err(VaultError::InvalidArgument(format!(
                "{scope:?} is not a Platform signer scope"
            )));
        }
        if token.purpose.kind() != GrantKind::PlatformOp {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.token_signer(wallet, token, scope)
    }

    /// A signer with `scope` under a redeemed grant for `wallet`; the caller
    /// has checked the grant's purpose.
    pub(crate) fn token_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        if token.wallet.as_ref() != Some(wallet) {
            return Err(VaultError::GrantPurposeMismatch);
        }
        let own_key = match &token.key {
            Some(key) => Some(Arc::new(Zeroizing::new(**key))),
            None => {
                self.full_dek()?;
                None
            }
        };
        let inner = self.inner();
        if token.epoch != inner.epoch {
            return Err(VaultError::GrantInvalid);
        }
        if own_key.is_none() && inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        self.signer_locked(&inner, wallet, scope, own_key)
    }

    /// The background DashPay crypto signer ([`SignerScope::DashPayCrypto`],
    /// DASHPAY §2.6): no grant, so it exists only while the full key is
    /// available without a prompt, on an unencrypted vault or one unlocked
    /// with scope Full. `Locked` and `MixingOnly` otherwise. It can neither
    /// spend nor sign a state transition; the one key it exports is a
    /// DIP-15 auto-accept key. Stops working when the vault locks or changes
    /// unlock scope. Engine-only: dw-ffi's clippy configuration forbids
    /// calling it, so no view reaches key material without a grant.
    pub fn dashpay_crypto_signer(&self, wallet: &WalletId) -> Result<VaultSigner, VaultError> {
        self.prompt_free_signer(wallet, SignerScope::DashPayCrypto)
    }

    /// A signer with `scope` under the vault's own full key, issued only in
    /// the states where that key needs no prompt (see
    /// [`Self::dashpay_crypto_signer`]). The state is read under the same
    /// lock that issues the signer, so an unlock for mixing in between is
    /// refused rather than served with the mixing-only key.
    fn prompt_free_signer(
        &self,
        wallet: &WalletId,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        if matches!(
            self.lock_state(),
            LockState::NoKeys | LockState::Unencrypted
        ) {
            // Loads the data key from the OS store.
            self.full_dek()?;
        }
        let inner = self.inner();
        match Self::state_of(&inner) {
            LockState::NoVault => Err(VaultError::NoVault),
            LockState::Locked => Err(VaultError::Locked),
            LockState::UnlockedMixingOnly => Err(VaultError::MixingOnly),
            LockState::NoKeys | LockState::Unencrypted | LockState::Unlocked => {
                if inner.dek.is_none() || inner.scope != UnlockScope::Full {
                    return Err(VaultError::Locked);
                }
                self.signer_locked(&inner, wallet, scope, None)
            }
        }
    }

    /// A signer limited to the CoinJoin account (`m/9'/coin'/4'/…`). Works in
    /// every state with the data key available, including mixing-only
    /// unlock; needs no grant (mixing runs unattended).
    pub fn mixing_signer(&self, wallet: &WalletId) -> Result<VaultSigner, VaultError> {
        self.unattended_signer(wallet, SignerScope::CoinJoinOnly)
    }

    /// As [`Self::mixing_signer`], with the BIP44 accounts added
    /// ([`SignerScope::CoinJoinFunding`]): for the denomination and
    /// collateral transactions mixing makes from the wallet's own coins.
    pub fn mixing_funding_signer(&self, wallet: &WalletId) -> Result<VaultSigner, VaultError> {
        self.unattended_signer(wallet, SignerScope::CoinJoinFunding)
    }

    fn unattended_signer(
        &self,
        wallet: &WalletId,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        match self.lock_state() {
            LockState::NoVault => return Err(VaultError::NoVault),
            LockState::Locked => return Err(VaultError::Locked),
            LockState::NoKeys | LockState::Unencrypted => {
                self.full_dek()?;
            }
            LockState::Unlocked | LockState::UnlockedMixingOnly => {}
        }
        let inner = self.inner();
        if inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        self.signer_locked(&inner, wallet, scope, None)
    }

    fn signer_locked(
        &self,
        inner: &Inner,
        wallet: &WalletId,
        scope: SignerScope,
        own_key: Option<Arc<Key32>>,
    ) -> Result<VaultSigner, VaultError> {
        let has_seed = inner
            .file
            .as_ref()
            .is_some_and(|f| f.records.contains_key(&record_id(wallet, REC_SEED)));
        if !has_seed {
            return Err(VaultError::NoSecret);
        }
        Ok(VaultSigner::new(
            self.clone(),
            *wallet,
            scope,
            inner.epoch,
            own_key,
        ))
    }

    /// Seed of `wallet` for a signer issued at `epoch`, decrypted with the
    /// signer's own key or the vault's, inside the operation `_op`.
    pub(crate) fn signing_seed(
        &self,
        _op: &OpGuard<'_>,
        wallet: &WalletId,
        epoch: u64,
        own_key: Option<&Key32>,
    ) -> Result<Zeroizing<[u8; 64]>, SignerError> {
        let dek = {
            let inner = self.inner();
            if inner.epoch != epoch {
                return Err(SignerError::Locked);
            }
            match own_key {
                Some(key) => Zeroizing::new(**key),
                None => Zeroizing::new(**inner.dek.as_ref().ok_or(SignerError::Locked)?),
            }
        };
        let payload = self
            .read_record(&dek, &record_id(wallet, REC_SEED))?
            .ok_or(SignerError::NoSecret)?;
        Ok(decode_seed(&payload)?.0)
    }

    /// Adds slot B (IOS-011): seals the data key under a fresh random
    /// 256-bit wrap key and returns that key for the host to keep in the OS
    /// biometric store; the vault does not keep it. Re-enrolling replaces
    /// the slot (the old key stops working). Needs an encrypted vault
    /// (`NotEncrypted`) and a `ChangeCredential` grant, which on an
    /// encrypted vault only the passphrase issues. The spending limit of an
    /// earlier enrolment is kept; a first enrolment gets
    /// [`DEFAULT_QUICK_UNLOCK_SPEND_LIMIT`].
    pub fn enroll_quick_unlock(&self, grant_id: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let writer = self.writer();
        let mut next = self.file_copy(&writer)?;
        if next.slot_p.is_none() {
            return Err(VaultError::NotEncrypted);
        }
        let token = self.redeem_grant(grant_id, GrantKind::ChangeCredential, None)?;
        let dek = self.key_for(&token)?;
        let policy = open_policy(&next, &dek)?.unwrap_or_default();
        let last_passphrase_at = policy
            .last_passphrase_at
            .max(self.inner().last_passphrase_at);
        let wrap_key = crypto::random_key()?;
        next.slot_b = Some(QuickUnlockSlot {
            wrapped_dek: crypto::seal(
                &wrap_key,
                &dek[..],
                &file::slot_b_aad(&next.vault_id, &next.network),
            )?,
            enrolled_at: self.now(),
        });
        next.quick_unlock = Some(seal_policy(
            &dek,
            &next.vault_id,
            &next.network,
            PolicyPayload {
                last_passphrase_at,
                ..policy
            },
        )?);
        self.persist(&writer, next)?;
        Ok(Zeroizing::new(wrap_key.to_vec()))
    }

    /// Deletes slot B. Idempotent; the spending limit is kept for a later
    /// enrolment. Needs no grant: it only removes a way in.
    pub fn remove_quick_unlock(&self) -> Result<VaultStatus, VaultError> {
        let writer = self.writer();
        let mut next = self.file_copy(&writer)?;
        if next.slot_b.take().is_some() {
            self.persist(&writer, next)?;
        }
        drop(writer);
        Ok(self.status())
    }

    /// The quick-unlock rules (IOS-016). In-memory read of the plain copy,
    /// so it works while locked; `authorize` enforces the sealed copy.
    pub fn quick_unlock_policy(&self) -> QuickUnlockPolicy {
        let inner = self.inner();
        let file = inner.file.as_ref();
        let settings = file.and_then(|f| f.quick_unlock.as_ref());
        QuickUnlockPolicy {
            enrolled: file.is_some_and(|f| f.slot_b.is_some()),
            spend_limit_duffs: settings
                .map_or(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, |s| s.spend_limit_duffs),
            passphrase_max_age_secs: PASSPHRASE_MAX_AGE_SECS,
            last_passphrase_at: settings
                .and_then(|s| s.last_passphrase_at)
                .max(inner.last_passphrase_at),
        }
    }

    /// Sets the quick-unlock spending limit to one of
    /// [`QUICK_UNLOCK_SPEND_LIMITS`] (`InvalidArgument` otherwise). Needs an
    /// encrypted vault (`NotEncrypted`) and a `ChangeCredential` grant.
    pub fn set_quick_unlock_spend_limit(
        &self,
        grant_id: &str,
        spend_limit_duffs: u64,
    ) -> Result<QuickUnlockPolicy, VaultError> {
        if !QUICK_UNLOCK_SPEND_LIMITS.contains(&spend_limit_duffs) {
            return Err(VaultError::InvalidArgument(format!(
                "spending limit {spend_limit_duffs} is not one of {QUICK_UNLOCK_SPEND_LIMITS:?}"
            )));
        }
        let writer = self.writer();
        let mut next = self.file_copy(&writer)?;
        if next.slot_p.is_none() {
            return Err(VaultError::NotEncrypted);
        }
        let token = self.redeem_grant(grant_id, GrantKind::ChangeCredential, None)?;
        let dek = self.key_for(&token)?;
        let policy = open_policy(&next, &dek)?.unwrap_or_default();
        next.quick_unlock = Some(seal_policy(
            &dek,
            &next.vault_id,
            &next.network,
            PolicyPayload {
                spend_limit_duffs,
                last_passphrase_at: policy
                    .last_passphrase_at
                    .max(self.inner().last_passphrase_at),
            },
        )?);
        self.persist(&writer, next)?;
        drop(writer);
        Ok(self.quick_unlock_policy())
    }

    /// Forgot passphrase (IOS-014, DESIGN-opus §1.8).
    ///
    /// 1. Checks that `phrase` + `bip39_passphrase` derive `wallet` on this
    ///    network, with standard BIP39 or else Dash Core's derivation
    ///    (`RecoveryMismatch` when neither does, or the phrase is invalid).
    /// 2. Keeps a copy of the current file as `vault.dwv.replaced-<now>`:
    ///    the old secrets stay readable with the old passphrase.
    /// 3. Replaces the vault with a new one (new data key and vault id)
    ///    encrypted with `new_passphrase` and holding only this wallet's
    ///    phrase, BIP39 passphrase and seed, checked by reading it back.
    ///
    /// The new vault is unlocked; grants, signers, the throttle and slot B of
    /// the old one are gone. Returns the other wallets whose seeds were in
    /// the old vault: they are watch-only until their phrases are imported.
    /// Needs an existing encrypted vault (`NoVault`, `NotEncrypted`).
    pub fn recover_with_mnemonic(
        &self,
        wallet: &WalletId,
        phrase: &[u8],
        bip39_passphrase: &[u8],
        new_passphrase_bytes: &[u8],
    ) -> Result<Vec<WalletId>, VaultError> {
        let pw = new_passphrase(new_passphrase_bytes)?;
        let secret = [false, true]
            .into_iter()
            .filter_map(|core| mnemonic::derive_secret(phrase, bip39_passphrase, core).ok())
            .find(|s| {
                mnemonic::wallet_id_for_seed(&s.seed, self.shared.network)
                    .is_ok_and(|id| id == *wallet)
            })
            .ok_or(VaultError::RecoveryMismatch)?;

        let writer = self.writer();
        let old = self.file_copy(&writer)?;
        if old.slot_p.is_none() {
            return Err(VaultError::NotEncrypted);
        }
        let lost: Vec<WalletId> = old
            .records
            .keys()
            .filter_map(|k| seed_record_wallet(k))
            .filter(|w| w != wallet)
            .collect();

        let now = self.now();
        let tag = &self.shared.tag;
        let dek = crypto::random_key()?;
        let vault_id: [u8; 16] = crypto::random_array()?;
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) = crypto::derive_new_kek(
            &pw,
            &salt,
            self.shared.config.kdf,
            self.production_network(),
        )?;
        let slot_p = PassphraseSlot {
            wrapped_dek: crypto::seal(
                &kek,
                &dek[..],
                &file::slot_p_aad(&vault_id, tag, &kdf, &salt),
            )?,
            kdf,
            salt: salt.to_vec(),
        };
        let backup_kek = BackupKek::of(kdf, &salt, &kek);
        let seed = encode_seed(&secret);
        let payloads: [(String, &[u8]); 3] = [
            (record_id(wallet, REC_MNEMONIC), &secret.mnemonic[..]),
            (
                record_id(wallet, REC_PASSPHRASE),
                &secret.mnemonic_passphrase[..],
            ),
            (record_id(wallet, REC_SEED), &seed[..]),
        ];
        let mut records = BTreeMap::new();
        let mut manifest = Manifest {
            generation: 1,
            records: BTreeMap::new(),
        };
        for (id, payload) in &payloads {
            let sealed = crypto::seal(&dek, payload, &file::record_aad(&vault_id, tag, id))?;
            manifest
                .records
                .insert(id.clone(), file::record_digest(&sealed));
            records.insert(id.clone(), sealed);
        }
        let next = VaultFile {
            format: file::FORMAT,
            vault_id: vault_id.to_vec(),
            network: tag.clone(),
            created_at: now,
            slot_p: Some(slot_p),
            slot_o: None,
            throttle: Throttle::default(),
            manifest: seal_manifest(&dek, &vault_id, tag, &manifest)?,
            records,
            slot_b: None,
            quick_unlock: Some(seal_policy(
                &dek,
                &vault_id,
                tag,
                PolicyPayload {
                    last_passphrase_at: Some(now),
                    ..PolicyPayload::default()
                },
            )?),
        };

        file::keep_replaced_copy(&self.shared.dir, now)?;
        file::write(&self.shared.dir, &next)?;
        // From here the old vault is replaced on disk; memory follows even
        // if the read-back check below fails, so nothing keeps using the
        // old key.
        {
            let ops = self.epoch_guard();
            let mut inner = self.inner();
            inner.file = Some(next);
            inner.high_water = manifest.generation;
            inner.forget_key(&ops);
            inner.install_key(Zeroizing::new(*dek), UnlockScope::Full, &ops);
            inner.backup_kek = Some(backup_kek);
            inner.last_passphrase_at = Some(now);
        }
        let disk = file::read(&self.shared.dir)?
            .ok_or_else(|| VaultError::Corrupt("vault file vanished after write".into()))?;
        verify_manifest(&disk, &dek, manifest.generation)?;
        for (id, payload) in &payloads {
            let read = disk.records.get(id).and_then(|sealed| {
                crypto::open(&dek, sealed, &file::record_aad(&vault_id, tag, id))
            });
            if !read.is_some_and(|r| bool::from(r[..].ct_eq(payload))) {
                return Err(VaultError::Corrupt(format!(
                    "read-back of {id} did not match"
                )));
            }
        }
        drop(writer);
        Ok(lost)
    }

    /// Deletes the vault (IOS-009 "Delete All", IOS-109 wipe): the vault
    /// file, its replaced copies and the slot O key in the OS store. The
    /// host deletes its slot B item. Refused while the vault holds any
    /// record (`NotEmpty`): remove every wallet first. An encrypted vault
    /// needs the passphrase (`CredentialRequired` for any other credential;
    /// attempts are throttled); an unencrypted one needs none. Idempotent:
    /// without a vault it returns the `NoVault` status.
    pub fn destroy(&self, credential: Credential<'_>) -> Result<VaultStatus, VaultError> {
        let writer = self.writer();
        let Some(current) = self.inner().file.clone() else {
            drop(writer);
            return Ok(self.status());
        };
        if !current.records.is_empty() {
            return Err(VaultError::NotEmpty);
        }
        if current.slot_p.is_some() {
            match credential {
                Credential::Passphrase(pw) => {
                    self.check_passphrase(&writer, pw)?;
                }
                Credential::QuickUnlock(_) | Credential::None => {
                    return Err(VaultError::CredentialRequired);
                }
            }
        }
        file::remove_all(&self.shared.dir)?;
        {
            let ops = self.epoch_guard();
            let mut inner = self.inner();
            inner.file = None;
            inner.high_water = 0;
            inner.last_passphrase_at = None;
            inner.forget_key(&ops);
        }
        drop(writer);
        if let Some(o) = current.slot_o
            && let Ok(service) = <[u8; 32]>::try_from(o.service.as_slice())
            && let Err(e) = self.shared.config.os_store.delete(&service, &o.label)
        {
            // The vault file is gone, so the orphaned key opens nothing.
            tracing::warn!(error = %e, "could not delete the OS store key of a destroyed vault");
        }
        Ok(self.status())
    }
}

fn seal_manifest(
    dek: &[u8; 32],
    vault_id: &[u8],
    network: &str,
    manifest: &Manifest,
) -> Result<Sealed, VaultError> {
    let plain = Zeroizing::new(
        serde_json::to_vec(manifest).map_err(|e| VaultError::Internal(format!("manifest: {e}")))?,
    );
    crypto::seal(dek, &plain, &file::manifest_aad(vault_id, network))
}

/// Decrypts the manifest and checks it lists exactly the records in the file
/// with matching digests, at a generation no older than `high_water`.
fn verify_manifest(f: &VaultFile, dek: &[u8; 32], high_water: u64) -> Result<Manifest, VaultError> {
    let plain = crypto::open(
        dek,
        &f.manifest,
        &file::manifest_aad(&f.vault_id, &f.network),
    )
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
        return Err(VaultError::Corrupt(
            "record set differs from the manifest".into(),
        ));
    }
    for (id, sealed) in &f.records {
        if manifest.records.get(id) != Some(&file::record_digest(sealed)) {
            return Err(VaultError::Corrupt(format!(
                "record {id} differs from the manifest"
            )));
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
        assert!(matches!(
            new_passphrase(b""),
            Err(VaultError::PassphraseRejected(_))
        ));
    }

    /// A clock the test moves by hand.
    struct StepClock(std::sync::atomic::AtomicU64);

    impl crate::types::Clock for StepClock {
        fn now_secs(&self) -> u64 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn test_config(clock: Arc<StepClock>) -> VaultConfig {
        VaultConfig {
            kdf: crate::KdfPolicy::Fixed(crate::KdfParams::TEST),
            os_store: Arc::new(crate::MemoryOsStore::new()),
            clock,
            grant_ttl_secs: 120,
        }
    }

    fn step_clock() -> Arc<StepClock> {
        Arc::new(StepClock(std::sync::atomic::AtomicU64::new(1_000)))
    }

    /// Review L2: a grant that carries its own copy of the data key (issued
    /// by passphrase on a locked vault) is dropped, key and all, once it has
    /// expired, by `status`, `lock_state`, `check_grant` or the engine's
    /// timer, without the grant itself ever being looked up.
    #[test]
    fn expired_grants_and_their_keys_are_dropped_without_a_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let clock = step_clock();
        let v = Vault::open(
            dir.path().join("vault"),
            Network::Regtest,
            "regtest",
            test_config(clock.clone()),
        )
        .unwrap();
        v.create(Some(b"pw")).unwrap();
        v.lock();
        let w = [1u8; 32];
        let spend = GrantPurpose::Spend { max_duffs: 1 };

        type Sweep = fn(&Vault);
        let sweeps: [(&str, Sweep); 4] = [
            ("status", |v| {
                v.status();
            }),
            ("lock_state", |v| {
                v.lock_state();
            }),
            ("check_grant of another id", |v| {
                let _ = v.check_grant("unknown", GrantKind::Spend, Some(&[1u8; 32]));
            }),
            ("purge_expired_grants", |v| v.purge_expired_grants()),
        ];
        for (name, sweep) in sweeps {
            v.authorize(spend, Some(&w), Credential::Passphrase(b"pw"))
                .unwrap();
            assert!(
                v.inner().grants.values().all(|g| g.key.is_some()),
                "{name}: a passphrase grant on a locked vault holds its own key"
            );
            sweep(&v);
            assert_eq!(v.inner().grants.len(), 1, "{name}: a live grant stays");
            clock.0.fetch_add(121, std::sync::atomic::Ordering::SeqCst);
            sweep(&v);
            assert!(
                v.inner().grants.is_empty(),
                "{name}: an expired grant is dropped"
            );
        }
    }

    /// Review L1: a correct passphrase is accepted even when the reset of
    /// the failed-attempt counter cannot be written; the counter is reset in
    /// memory and reaches the disk with the next successful write.
    #[test]
    fn correct_passphrase_works_when_the_throttle_reset_cannot_be_written() {
        let dir = tempfile::tempdir().unwrap();
        let vault_dir = dir.path().join("vault");
        let open = || {
            Vault::open(
                &vault_dir,
                Network::Regtest,
                "regtest",
                test_config(step_clock()),
            )
            .unwrap()
        };
        let v = open();
        v.create(Some(b"pw")).unwrap();
        v.lock();
        assert!(matches!(
            v.unlock(b"wrong", UnlockScope::Full),
            Err(VaultError::WrongPassphrase {
                failed_attempts: 1,
                ..
            })
        ));

        // A directory where the temp file goes makes every write fail, as a
        // full disk would.
        let blocker = vault_dir.join(file::TMP_NAME);
        std::fs::create_dir(&blocker).unwrap();
        let unlocked = v.unlock(b"pw", UnlockScope::Full);
        let failures_on_disk = open().status().failed_attempts;
        v.lock();
        let granted = v.authorize(
            GrantPurpose::Wipe,
            Some(&[1u8; 32]),
            Credential::Passphrase(b"pw"),
        );
        std::fs::remove_dir(&blocker).unwrap();

        assert_eq!(unlocked.unwrap().state, LockState::Unlocked);
        assert_eq!(v.status().failed_attempts, 0, "reset in memory");
        assert_eq!(failures_on_disk, 1, "the disk still holds the old counter");
        granted.unwrap();

        // The next successful write carries the reset counter.
        v.change_passphrase(b"pw", b"pw2").unwrap();
        assert_eq!(open().status().failed_attempts, 0);
    }

    #[test]
    fn seed_record_ids_parse() {
        let w = [0xab; 32];
        assert_eq!(seed_record_wallet(&record_id(&w, REC_SEED)), Some(w));
        assert_eq!(seed_record_wallet(&record_id(&w, REC_MNEMONIC)), None);
    }
}
