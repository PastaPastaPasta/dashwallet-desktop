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
//!
//! Lock order: `writer`, then `inner`.

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

/// Proof that the caller holds `Shared::writer`.
type WriteGuard<'a> = MutexGuard<'a, ()>;

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
    /// Serializes file changes and passphrase checks (see the module doc).
    writer: Mutex<()>,
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

    fn writer(&self) -> WriteGuard<'_> {
        self.shared
            .writer
            .lock()
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
            quick_unlock_enrolled: false,
            failed_attempts: f.throttle.failed_attempts,
            retry_after_secs: retry_after(&f.throttle, self.now()),
            wallets_with_secrets: f
                .records
                .keys()
                .filter_map(|k| seed_record_wallet(k))
                .collect(),
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
        let writer = self.writer();
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
        let mut inner = self.inner();
        inner.file = Some(vault_file);
        inner.high_water = 1;
        inner.install_key(dek, UnlockScope::Full);
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
            let mut inner = self.inner();
            inner.forget_key();
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
        let dek = self.check_passphrase(&writer, passphrase)?;
        let mut inner = self.inner();
        inner.install_key(dek, scope);
        Ok(self.status_of(&inner))
    }

    /// Drops the data key, revokes every grant and invalidates every
    /// outstanding grant token and signer, including those that carry a
    /// grant's own key. Idempotent. On an unencrypted vault this drops the
    /// cached key (it is re-read from the OS store on demand); the state
    /// stays `Unencrypted`.
    pub fn lock(&self) -> VaultStatus {
        let mut inner = self.inner();
        inner.forget_key();
        self.status_of(&inner)
    }

    /// Re-wraps the data key under `new`; records (and the seed) are
    /// unchanged. The lock state is preserved.
    pub fn change_passphrase(&self, old: &[u8], new: &[u8]) -> Result<VaultStatus, VaultError> {
        let new = new_passphrase(new)?;
        let writer = self.writer();
        let dek = self.check_passphrase(&writer, old)?;
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
        Ok(self.status())
    }

    /// Verifies `passphrase` against slot P of the in-memory file and checks
    /// the manifest with the unwrapped key. On success resets the throttle
    /// and returns the data key; on failure records the attempt. Runs under
    /// `writer`, so attempts are serialized and each one sees the throttle
    /// the previous one left.
    fn check_passphrase(
        &self,
        writer: &WriteGuard<'_>,
        passphrase: &[u8],
    ) -> Result<Key32, VaultError> {
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

        let had_failures = {
            let mut inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let manifest = verify_manifest(f, &dek, inner.high_water)?;
            let had_failures = f.throttle != Throttle::default();
            inner.high_water = inner.high_water.max(manifest.generation);
            had_failures
        };
        if had_failures {
            let mut next = self.file_copy(writer)?;
            next.throttle = Throttle::default();
            self.persist(writer, next)?;
        }
        Ok(dek)
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
    /// - `QuickUnlock`: not available until slot B (M2).
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
            // TODO(biometric): slot B (Touch ID / Windows Hello) lands in M2.
            Credential::QuickUnlock(_) => Err(VaultError::QuickUnlockUnavailable),
            Credential::Passphrase(pw) => {
                let writer = self.writer();
                let dek = self.check_passphrase(&writer, pw)?;
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
        inner.grants.retain(|_, g| g.grant.expires_at >= now);
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
        let expired = inner
            .grants
            .get(grant_id)
            .ok_or(VaultError::GrantInvalid)?
            .grant
            .expires_at
            < now;
        if expired {
            inner.grants.remove(grant_id);
            return Err(VaultError::GrantInvalid);
        }
        let issued = &inner.grants[grant_id];
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
        self.commit_records(
            &dek,
            &[
                (record_id(wallet, REC_MNEMONIC), Some(&secret.mnemonic[..])),
                (
                    record_id(wallet, REC_PASSPHRASE),
                    Some(&secret.mnemonic_passphrase[..]),
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
    /// grant for that wallet whose purpose signs (spend, message, masternode,
    /// governance, platform). A grant authorized by passphrase on a locked or
    /// mixing-only vault hands its own key to the signer. The signer stops
    /// working when the vault locks or changes unlock scope.
    pub fn signer(&self, wallet: &WalletId, token: &GrantToken) -> Result<VaultSigner, VaultError> {
        if !token.purpose.signs() || token.wallet.as_ref() != Some(wallet) {
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
        self.signer_locked(&inner, wallet, SignerScope::Full, own_key)
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
        if inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        self.signer_locked(&inner, wallet, SignerScope::CoinJoinOnly, None)
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
    /// signer's own key or the vault's.
    pub(crate) fn signing_seed(
        &self,
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

    /// Biometric slot B. TODO(biometric): M2.
    pub fn enroll_quick_unlock(&self, _grant_id: &str) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        Err(VaultError::NotImplemented("Vault.enroll_quick_unlock"))
    }

    /// Biometric slot B. TODO(biometric): M2.
    pub fn remove_quick_unlock(&self) -> Result<VaultStatus, VaultError> {
        Err(VaultError::NotImplemented("Vault.remove_quick_unlock"))
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

    #[test]
    fn seed_record_ids_parse() {
        let w = [0xab; 32];
        assert_eq!(seed_record_wallet(&record_id(&w, REC_SEED)), Some(w));
        assert_eq!(seed_record_wallet(&record_id(&w, REC_MNEMONIC)), None);
    }
}
