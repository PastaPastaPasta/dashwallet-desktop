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
//! - `ops` is the operation gate. Every signer call holds it shared from its
//!   epoch check until its result is released, and every use of a grant
//!   token's key holds it from the token check until the operation is done
//!   ([`Vault::gated`]): the secret reads (`reveal_mnemonic`,
//!   `export_wallet_secret`, `with_revealed_seed`, `open_backup_bundle` of
//!   this vault's own bundle) and the token-authorized writes
//!   (`wipe_wallet_secret`, `encrypt`, `enroll_quick_unlock`,
//!   `set_quick_unlock_spend_limit`), whose file write is inside.
//!   `seed_derivation` and `core_mnemonic_check`, which read a secret with
//!   the vault's own key, are gated too. Every epoch change (lock, unlock,
//!   scope change, passphrase change, encrypt, recover, destroy) holds it
//!   exclusively, so `lock()` returns only once every such operation that
//!   started before it has finished, and any operation after it sees the
//!   new epoch and fails `Locked`. Not gated: `backup_bundle`, which takes
//!   no grant or epoch and returns only ciphertext, `open_backup_bundle`
//!   through a bundle's passphrase slot (no vault key), and record writes
//!   under the vault's own key (`store_wallet_secret`,
//!   `delete_wallet_secret`).
//!
//!   A holder must not take the gate again, take `writer`, block on another
//!   lock or wait on anything outside the process (the OS store, which may
//!   prompt): a waiting epoch change may block new shared holders (std's
//!   `RwLock` prefers writers on Linux; the policy is OS-dependent), and
//!   `lock()` waits for every holder. So a gated operation uses only a key
//!   already in memory, and one that may need the OS store's copy loads it
//!   before it enters ([`Vault::full_dek`]). Writing the vault file inside
//!   is allowed; `writer` is then taken before the gate.
//!
//! Lock order: `writer`, then `ops`, then `inner`.
//!
//! Release (review DW-E0-03 r2 M2). A gated operation's result leaves the
//! vault only through [`OpGuard::release`], which [`Vault::gated`] calls
//! last: under `inner`, the epoch the operation started under (read under
//! `inner` when it entered the gate) must still be the vault's epoch, or the
//! result is dropped (secret results erase themselves on drop, an
//! `ExtendedPrivKey` included) and the call fails `Locked`.
//! Why that is enough for "nothing is released once `lock()` has returned":
//! - every epoch change writes `inner.epoch` holding `inner`, and every
//!   release reads it holding `inner`, so the mutex orders each release
//!   wholly before or wholly after each epoch change;
//! - an operation that started before a lock's change and releases after
//!   it reads a newer epoch than the one it started under, and refuses;
//! - so every result released under epoch `e` was released before the
//!   change that ended `e`, and `lock()` makes that change before it
//!   returns. A result released after `lock()` returned would be ordered
//!   after the change, which the check refuses.
//!
//! This holds by the order of the `inner` mutex, not by timing, and does
//! not depend on the gate: the gate adds that a lock waits for operations
//! already running (so no gated operation still computes with a key once
//! `lock()` has returned, and the check never actually refuses one), while
//! the check states the release rule at the one point where a result
//! leaves. The lock-race tests log every release and epoch change in
//! `inner`'s order and check the rule on that log.
//!
//! Out of reach of any vault-side check: a result released before the lock
//! that its caller holds, or has not yet looked at, when `lock()` returns,
//! and what a caller computes from it afterwards (`with_revealed_seed`'s
//! closure runs after the release, by design). A flow that must not use
//! such a result after a lock ("Lock to cancel") hands it to the transport
//! only while holding a permit of its lease, which the lock takes
//! exclusively (DASHPAY §2.6, roadmap E0-04).

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
/// Shared hold of `Shared::ops`: one operation that produces a
/// secret-derived result (see the module doc). Only [`Vault::gated`]
/// makes one, and ends it with [`OpGuard::release`].
pub(crate) struct OpGuard<'a> {
    vault: &'a Vault,
    /// The vault's epoch when the operation entered the gate.
    epoch: u64,
    _ops: RwLockReadGuard<'a, ()>,
}

impl OpGuard<'_> {
    /// Releases `out` if the vault is still in the epoch this operation
    /// started under; `None` (and `out` dropped) otherwise. The check holds
    /// `inner`, the mutex every epoch change holds; the module doc
    /// ("Release") explains why that makes a release after `lock()` has
    /// returned impossible. `inner` is unlocked, then the gate, when this
    /// returns.
    fn release<T>(self, out: T) -> Option<T> {
        #[cfg_attr(not(test), allow(unused_mut))]
        let mut inner = self.vault.inner();
        let current = bool::from(inner.epoch.ct_eq(&self.epoch));
        #[cfg(test)]
        inner.record(if current {
            GateEvent::Released(self.epoch)
        } else {
            GateEvent::Refused(self.epoch)
        });
        current.then_some(out)
    }
}

/// What the lock-race tests observe of the gate, in `inner`'s order
/// ([`Vault::start_gate_log`]).
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GateEvent {
    /// The vault's epoch is now this (at the start of the log, and at each
    /// change).
    Epoch(u64),
    /// An operation entered the gate under this epoch.
    Opened(u64),
    /// An operation that started under this epoch released its result.
    Released(u64),
    /// An operation that started under this epoch had its result refused.
    Refused(u64),
    /// `lock()` was called; it now waits for the gate.
    LockCalled,
}

/// Exclusive hold of `Shared::ops`, which every epoch change needs. Only
/// [`Vault::epoch_guard`] makes one.
struct EpochGuard<'a> {
    _ops: RwLockWriteGuard<'a, ()>,
}

/// How often an operation that loads an unencrypted vault's key from the
/// OS store tries again when a concurrent `lock()` dropped it before use
/// ([`Vault::with_loaded_key`]).
const KEY_LOAD_ATTEMPTS: usize = 3;

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

/// How a grant token relates to the vault it is presented to
/// ([`Vault::token_binding`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenBinding {
    Current,
    /// Redeemed by another vault instance.
    OtherVault,
    /// Redeemed by this vault before its epoch last changed.
    EndedEpoch,
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
    /// Bumped whenever the vault locks, the unlock scope changes or the
    /// passphrase changes. Grants, grant tokens and signers from an older
    /// epoch are refused.
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
    #[cfg(test)]
    gate_log: Option<Vec<GateEvent>>,
}

impl Inner {
    /// Drops the data key, revokes grants and invalidates outstanding signers.
    fn forget_key(&mut self, ops: &EpochGuard<'_>) {
        self.dek = None;
        self.backup_kek = None;
        self.scope = UnlockScope::Full;
        self.revoke(ops);
    }

    /// Ends the epoch and drops every grant (and any key one holds), so
    /// every grant, grant token and signer issued before is refused; the
    /// data key and the lock state are kept.
    fn revoke(&mut self, _ops: &EpochGuard<'_>) {
        self.next_epoch();
        self.grants.clear();
    }

    /// Ends the current epoch. Every caller holds the gate exclusively
    /// (an [`EpochGuard`]), except a test's [`Vault::bump_epoch_ungated`].
    fn next_epoch(&mut self) {
        self.epoch += 1;
        #[cfg(test)]
        self.record(GateEvent::Epoch(self.epoch));
    }

    #[cfg(test)]
    fn record(&mut self, event: GateEvent) {
        if let Some(log) = &mut self.gate_log {
            log.push(event);
        }
    }

    /// Drops grants that expired before `now`, together with any key they
    /// hold, so a grant's own copy of the data key does not outlive the grant.
    fn drop_expired_grants(&mut self, now: u64) {
        self.grants.retain(|_, g| g.grant.expires_at >= now);
    }

    /// Installs a data key with `scope`; bumps the epoch when the state changes.
    fn install_key(&mut self, dek: Key32, scope: UnlockScope, ops: &EpochGuard<'_>) {
        let changed = self.dek.is_none() || self.scope != scope;
        self.dek = Some(dek);
        self.scope = scope;
        if changed {
            self.revoke(ops);
        }
    }
}

pub(crate) struct Shared {
    /// Random id of this in-memory vault, made at open: a grant token
    /// carries it, so only the vault that redeemed a token accepts it. Not
    /// stored, so the same file opened again is another instance.
    instance: [u8; 32],
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
                instance: crypto::random_array()?,
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
                    #[cfg(test)]
                    gate_log: None,
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

    /// Runs `body` as one gated operation (see the module doc) and releases
    /// its result with [`OpGuard::release`]: `locked` when the vault's epoch
    /// is no longer the one `body` started under. The only way to make an
    /// [`OpGuard`], so no gated result leaves without that check.
    pub(crate) fn gated<T, E>(
        &self,
        locked: E,
        body: impl FnOnce(&OpGuard<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let ops = self
            .shared
            .ops
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let epoch = {
            #[cfg_attr(not(test), allow(unused_mut))]
            let mut inner = self.inner();
            let epoch = inner.epoch;
            #[cfg(test)]
            inner.record(GateEvent::Opened(epoch));
            epoch
        };
        let op = OpGuard {
            vault: self,
            epoch,
            _ops: ops,
        };
        let out = body(&op)?;
        op.release(out).ok_or(locked)
    }

    /// Starts logging the gate's events ([`GateEvent`]) in `inner`'s order.
    #[cfg(test)]
    pub(crate) fn start_gate_log(&self) {
        let mut inner = self.inner();
        let epoch = inner.epoch;
        inner.gate_log = Some(vec![GateEvent::Epoch(epoch)]);
    }

    /// The events logged since [`Self::start_gate_log`]; logging stops.
    #[cfg(test)]
    pub(crate) fn take_gate_log(&self) -> Vec<GateEvent> {
        self.inner().gate_log.take().unwrap_or_default()
    }

    /// Changes the epoch without the gate, as no production code may: lets a
    /// test show that [`OpGuard::release`] refuses on its own.
    #[cfg(test)]
    pub(crate) fn bump_epoch_ungated(&self) {
        self.inner().next_epoch();
    }

    /// Waits for every open operation, then excludes new ones until the
    /// guard drops: the right to change the epoch.
    fn epoch_guard(&self) -> EpochGuard<'_> {
        EpochGuard {
            _ops: self
                .shared
                .ops
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        }
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
        #[cfg(test)]
        crate::signer::test_hook::checked();
        // Argon2id runs before the gate: a lock does not wait for it.
        let salt: [u8; SALT_LEN] = crypto::random_array()?;
        let (kdf, kek) = crypto::derive_new_kek(
            &pw,
            &salt,
            self.shared.config.kdf,
            self.production_network(),
        )?;
        let aad = file::slot_p_aad(&next.vault_id, &next.network, &kdf, &salt);
        let old_slot = next.slot_o.take();
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
            next.slot_p = Some(PassphraseSlot {
                kdf,
                salt: salt.to_vec(),
                wrapped_dek: crypto::seal(&kek, &dek[..], &aad)?,
            });
            self.persist(&writer, next)
        })?;
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
    /// Waits for the gated operations already running (a signature takes
    /// about a millisecond, a gated write one file write and its read-back;
    /// see the module doc), so once it returns no
    /// signer result (signature, shared secret, ciphertext, exported key)
    /// of the old epoch is still being made, and no gated secret read is
    /// still under way; and no result of the old epoch is released after
    /// it returns (module doc, "Release").
    pub fn lock(&self) -> VaultStatus {
        #[cfg(test)]
        self.inner().record(GateEvent::LockCalled);
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.forget_key(&ops);
        self.status_of(&inner)
    }

    /// Re-wraps the data key under `new`; records (and the seed) are
    /// unchanged. The lock state is preserved, but the epoch ends (review
    /// DW-E0-03 r3 m3): every grant, pending or redeemed, every grant token
    /// and every signer issued before is refused afterwards, including
    /// those that carry a key unwrapped with the old passphrase. A user who
    /// changes the passphrase because the old one leaked thus revokes what
    /// it authorized; new grants need the new one.
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
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.revoke(&ops);
        // Backups written from now on open with the new passphrase.
        if inner.dek.is_some() {
            inner.backup_kek = Some(BackupKek::of(kdf, &salt, &kek));
        }
        drop(writer);
        Ok(self.status_of(&inner))
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
                // An unencrypted vault's grant is issued only while its key
                // is in memory, so its token never needs the OS store under
                // the gate (`key_for`). A lock between the load and the
                // issue drops the key again: load it again, a bounded number
                // of times.
                let mut loads = 0;
                let mut inner = loop {
                    let inner = self.inner();
                    let needs_load = inner.dek.is_none()
                        && matches!(
                            Self::state_of(&inner),
                            LockState::NoKeys | LockState::Unencrypted
                        );
                    if !needs_load {
                        break inner;
                    }
                    drop(inner);
                    if loads == KEY_LOAD_ATTEMPTS {
                        return Err(VaultError::Locked);
                    }
                    loads += 1;
                    // Fails when the OS store cannot produce the data key.
                    self.full_dek()?;
                };
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
            vault_instance: self.shared.instance,
            epoch: inner.epoch,
            key: issued.key,
        })
    }

    /// Whether `token` was redeemed by this vault instance in its current
    /// epoch. Both are compared in constant time, and every use of a token
    /// (its key, or a signer it issues) checks this first, under `inner`
    /// (review DW-E0-03 r2 M1). The instance id refuses a token of another
    /// vault (even one with the same wallet id and epoch number) and of an
    /// earlier opening of this vault's file; the epoch refuses a token
    /// redeemed before a lock, unlock, scope or passphrase change of this one.
    fn token_binding(&self, inner: &Inner, token: &GrantToken) -> TokenBinding {
        let same_vault = self.shared.instance.ct_eq(&token.vault_instance);
        let same_epoch = inner.epoch.ct_eq(&token.epoch);
        if !bool::from(same_vault) {
            TokenBinding::OtherVault
        } else if !bool::from(same_epoch) {
            TokenBinding::EndedEpoch
        } else {
            TokenBinding::Current
        }
    }

    /// `token`'s binding as the error its use is refused with:
    /// `GrantInvalid` for a token of another vault, `Locked` for one whose
    /// epoch has ended (a lock, unlock, scope change or passphrase change
    /// since redemption).
    fn token_current(&self, inner: &Inner, token: &GrantToken) -> Result<(), VaultError> {
        match self.token_binding(inner, token) {
            TokenBinding::Current => Ok(()),
            TokenBinding::OtherVault => Err(VaultError::GrantInvalid),
            TokenBinding::EndedEpoch => Err(VaultError::Locked),
        }
    }

    /// The full-scope data key a redeemed grant acts with, inside the gated
    /// operation `_op`: the grant's own key, or the vault's. The token check
    /// and the key read are one step under `inner`, inside the gate, so a
    /// `lock()` either returns before it (and the token is refused) or
    /// waits until the operation is done (review DW-E0-03 r3 m2). The
    /// vault's key is taken from memory only: a token without a key of its
    /// own was redeemed while the full key was in memory, and only an epoch
    /// change drops it.
    fn key_for(&self, _op: &OpGuard<'_>, token: &GrantToken) -> Result<Key32, VaultError> {
        let key = {
            let inner = self.inner();
            self.token_current(&inner, token)?;
            match &token.key {
                Some(key) => Zeroizing::new(**key),
                None => Self::memory_dek(&inner)?,
            }
        };
        #[cfg(test)]
        crate::signer::test_hook::opened();
        Ok(key)
    }

    /// The full-scope data key held in memory, inside the gated operation
    /// `_op`; `Locked` when none is (the caller loads an unencrypted vault's
    /// key with [`Self::full_dek`] before it enters the gate).
    fn gated_dek(&self, _op: &OpGuard<'_>) -> Result<Key32, VaultError> {
        Self::memory_dek(&self.inner())
    }

    /// The data key in memory, with full scope: `MixingOnly` for an
    /// encrypted vault unlocked for mixing, `Locked` when there is none.
    fn memory_dek(inner: &Inner) -> Result<Key32, VaultError> {
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        match &inner.dek {
            Some(_) if f.slot_p.is_some() && inner.scope == UnlockScope::MixingOnly => {
                Err(VaultError::MixingOnly)
            }
            Some(dek) => Ok(Zeroizing::new(**dek)),
            None => Err(VaultError::Locked),
        }
    }

    /// Runs `body` with the vault's full key as one gated operation, after
    /// loading an unencrypted vault's key from the OS store outside the
    /// gate (review DW-E0-03 r3 m1); inside, only the in-memory key is
    /// used. A `lock()` between the load and the gate drops the key again,
    /// and the operation fails `Locked`; an unencrypted vault needs no
    /// prompt for its key, so the load and the operation are tried again,
    /// at most [`KEY_LOAD_ATTEMPTS`] times in all.
    fn with_loaded_key<T>(
        &self,
        mut body: impl FnMut(&Key32) -> Result<T, VaultError>,
    ) -> Result<T, VaultError> {
        let mut attempt = 1;
        loop {
            self.full_dek()?;
            #[cfg(test)]
            crate::signer::test_hook::checked();
            match self.gated(VaultError::Locked, |op| body(&self.gated_dek(op)?)) {
                Err(VaultError::Locked) if attempt < KEY_LOAD_ATTEMPTS && self.unencrypted() => {
                    attempt += 1;
                }
                result => return result,
            }
        }
    }

    /// Whether the vault has no passphrase slot (its key is in the OS
    /// store). In-memory read.
    fn unencrypted(&self) -> bool {
        self.inner()
            .file
            .as_ref()
            .is_some_and(|f| f.slot_p.is_none())
    }

    /// The data key with full scope: in memory after unlock, or read from
    /// the OS store for an unencrypted vault (and kept in memory). Never
    /// called inside the gate, since the OS store may block (on a keyring
    /// prompt, for instance) and `lock()` waits for the gate.
    fn full_dek(&self) -> Result<Key32, VaultError> {
        let (service, label) = {
            let inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            if inner.dek.is_some() {
                return Self::memory_dek(&inner);
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
        self.commit_records_under(&self.writer(), dek, changes)
    }

    /// [`Self::commit_records`] under a `writer` guard the caller holds.
    fn commit_records_under(
        &self,
        writer: &WriteGuard<'_>,
        dek: &Key32,
        changes: &[(String, Option<&[u8]>)],
    ) -> Result<(), VaultError> {
        let mut next = self.file_copy(writer)?;
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
        self.persist(writer, next)?;
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
        self.delete_records(&self.writer(), wallet, &dek)?;
        Ok(true)
    }

    /// Deletes every record of `wallet` under a redeemed `Wipe` grant for that
    /// wallet, with the grant's key when it carries one (a passphrase grant on
    /// a locked vault). `Ok(false)` when it had none. The deletion is one
    /// gated operation: it is done before a concurrent `lock()` returns, or
    /// refused (`Locked`) when that lock came first.
    pub fn wipe_wallet_secret(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<bool, VaultError> {
        if token.purpose.kind() != GrantKind::Wipe || token.wallet.as_ref() != Some(wallet) {
            return Err(VaultError::GrantPurposeMismatch);
        }
        let writer = self.writer();
        if !self.has_any_record(wallet)? {
            return Ok(false);
        }
        self.token_current(&self.inner(), token)?;
        #[cfg(test)]
        crate::signer::test_hook::checked();
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, token)?;
            self.delete_records(&writer, wallet, &dek)?;
            Ok(true)
        })
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

    fn delete_records(
        &self,
        writer: &WriteGuard<'_>,
        wallet: &WalletId,
        dek: &Key32,
    ) -> Result<(), VaultError> {
        let changes: Vec<(String, Option<&[u8]>)> = Self::wallet_record_ids(wallet)
            .into_iter()
            .map(|id| (id, None))
            .collect();
        self.commit_records_under(writer, dek, &changes)
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
        self.with_loaded_key(|dek| {
            let payload = self
                .read_record(dek, &record_id(wallet, REC_SEED))?
                .ok_or(VaultError::NoSecret)?;
            Ok(decode_seed(&payload)?.1)
        })
    }

    /// The recovery phrase and BIP39 passphrase (QT-113, IOS-006). Needs a
    /// `RevealSecret` grant bound to `wallet`.
    pub fn reveal_mnemonic(
        &self,
        wallet: &WalletId,
        grant_id: &str,
    ) -> Result<RevealedMnemonic, VaultError> {
        let token = self.redeem_grant(grant_id, GrantKind::RevealSecret, Some(wallet))?;
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
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
        })
    }

    /// A signer for every derivation of `wallet`, authorized by a redeemed
    /// grant for that wallet whose purpose signs (spend, message). A grant
    /// authorized by passphrase on a locked or mixing-only vault hands its
    /// own key to the signer. The signer stops working when the vault locks
    /// or changes unlock scope. A `PlatformOp` grant gets scoped signers
    /// ([`Self::platform_signer`]) and the identity-scan key
    /// ([`Self::scan_key`]), not this signer.
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
        // A token of another vault is refused before this one loads its
        // key; the full check is made under the lock that issues the signer.
        if self.token_binding(&self.inner(), token) == TokenBinding::OtherVault {
            return Err(VaultError::GrantInvalid);
        }
        let own_key = match &token.key {
            Some(key) => Some(Arc::new(Zeroizing::new(**key))),
            None => {
                self.full_dek()?;
                None
            }
        };
        let inner = self.inner();
        if self.token_binding(&inner, token) != TokenBinding::Current {
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
    /// DIP-15 auto-accept key. Stops working when the vault locks, changes
    /// unlock scope or changes its passphrase (the vault stays unlocked
    /// then, and the engine issues a new one). Engine-only: dw-ffi's clippy configuration forbids
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
    /// signer's own key or the vault's, inside the operation `op`, which
    /// must have started in that epoch.
    pub(crate) fn signing_seed(
        &self,
        op: &OpGuard<'_>,
        wallet: &WalletId,
        epoch: u64,
        own_key: Option<&Key32>,
    ) -> Result<Zeroizing<[u8; 64]>, SignerError> {
        if op.epoch != epoch {
            return Err(SignerError::Locked);
        }
        let dek = {
            let inner = self.inner();
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
        #[cfg(test)]
        crate::signer::test_hook::checked();
        let wrap_key = crypto::random_key()?;
        let enrolled_at = self.now();
        // The new slot B key is a credential for the data key: it is made
        // and released in one gated operation.
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
            let policy = open_policy(&next, &dek)?.unwrap_or_default();
            let last_passphrase_at = policy
                .last_passphrase_at
                .max(self.inner().last_passphrase_at);
            next.slot_b = Some(QuickUnlockSlot {
                wrapped_dek: crypto::seal(
                    &wrap_key,
                    &dek[..],
                    &file::slot_b_aad(&next.vault_id, &next.network),
                )?,
                enrolled_at,
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
        })
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
        #[cfg(test)]
        crate::signer::test_hook::checked();
        self.gated(VaultError::Locked, |op| {
            let dek = self.key_for(op, &token)?;
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
            self.persist(&writer, next)
        })?;
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
    use key_wallet::bip32::DerivationPath;
    use std::str::FromStr;

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

    /// Opens the vault in `dir` with the shared `os_store`.
    fn open_in(dir: &Path, os_store: &Arc<crate::MemoryOsStore>) -> Vault {
        let config = VaultConfig {
            os_store: os_store.clone(),
            ..test_config(step_clock())
        };
        Vault::open(dir.join("vault"), Network::Regtest, "regtest", config).unwrap()
    }

    /// A new vault in `dir` holding wallet `[1; 32]` with seed `[seed; 64]`.
    fn vault_with_wallet(dir: &Path, passphrase: Option<&[u8]>, seed: u8) -> Vault {
        vault_with_wallet_in(
            dir,
            &Arc::new(crate::MemoryOsStore::new()),
            passphrase,
            seed,
        )
    }

    fn vault_with_wallet_in(
        dir: &Path,
        os_store: &Arc<crate::MemoryOsStore>,
        passphrase: Option<&[u8]>,
        seed: u8,
    ) -> Vault {
        let v = open_in(dir, os_store);
        v.create(passphrase).unwrap();
        let secret = WalletSecret {
            mnemonic: Zeroizing::new(b"unused".to_vec()),
            mnemonic_passphrase: Zeroizing::new(Vec::new()),
            seed: Zeroizing::new([seed; 64]),
            derivation: SeedDerivation::Bip39,
        };
        v.store_wallet_secret(&[1; 32], &secret).unwrap();
        v
    }

    fn redeem(v: &Vault, purpose: GrantPurpose, credential: Credential<'_>) -> GrantToken {
        let grant = v.authorize(purpose, Some(&[1; 32]), credential).unwrap();
        v.redeem_grant(&grant.id, purpose.kind(), Some(&[1; 32]))
            .unwrap()
    }

    /// Every use, in `v`, of a token redeemed by `from` is refused with
    /// `GrantInvalid`: a scan key, a Platform signer, a full signer, an
    /// export and a wipe.
    fn assert_refuses(v: &Vault, from: &Vault, credential: Credential<'_>, case: &str) {
        let w = [1u8; 32];
        let refused = |r: Result<(), VaultError>, what: &str| {
            assert_eq!(r, Err(VaultError::GrantInvalid), "{case}: {what}");
        };
        let platform = || redeem(from, GrantPurpose::PlatformOp, credential);
        refused(v.scan_key(&w, &platform()).map(drop), "scan key");
        for scope in [
            SignerScope::PlatformIdentity,
            SignerScope::DashPayCrypto,
            SignerScope::PlatformFunding { max_duffs: 1 },
        ] {
            refused(
                v.platform_signer(&w, &platform(), scope).map(drop),
                "Platform signer",
            );
        }
        let spend = redeem(from, GrantPurpose::Spend { max_duffs: 1 }, credential);
        refused(v.signer(&w, &spend).map(drop), "signer");
        let reveal = redeem(from, GrantPurpose::RevealSecret, credential);
        refused(v.export_wallet_secret(&w, &reveal).map(drop), "export");
        let wipe = redeem(from, GrantPurpose::Wipe, credential);
        refused(v.wipe_wallet_secret(&w, &wipe).map(drop), "wipe");
        assert!(v.has_wallet_secret(&w), "{case}: nothing was wiped");
    }

    /// Review DW-E0-03 r2 M1: a token redeemed by one vault is refused by
    /// another holding the same wallet id at the same epoch number, whether
    /// the token would use the receiving vault's key (`Credential::None`)
    /// or carries its own (a passphrase grant on a locked vault).
    #[test]
    fn a_token_of_another_vault_is_refused() {
        let dirs = [(); 3].map(|_| tempfile::tempdir().unwrap());
        // Encrypted and unlocked with scope Full, so its own key would serve.
        let target = vault_with_wallet(dirs[0].path(), Some(b"pw"), 0x5a);
        let unencrypted = vault_with_wallet(dirs[1].path(), None, 0x7f);
        assert_eq!(target.inner().epoch, unencrypted.inner().epoch);
        assert_refuses(&target, &unencrypted, Credential::None, "unencrypted");

        let locked = vault_with_wallet(dirs[2].path(), Some(b"pw"), 0x7f);
        locked.lock();
        target.lock();
        target.unlock(b"pw", UnlockScope::Full).unwrap();
        locked.lock();
        assert_eq!(target.inner().epoch, locked.inner().epoch);
        assert_refuses(&target, &locked, Credential::Passphrase(b"pw"), "own key");

        // The tokens still work in the vault that redeemed them.
        let token = redeem(&unencrypted, GrantPurpose::PlatformOp, Credential::None);
        unencrypted.scan_key(&[1; 32], &token).unwrap();
        let token = redeem(
            &locked,
            GrantPurpose::PlatformOp,
            Credential::Passphrase(b"pw"),
        );
        locked.scan_key(&[1; 32], &token).unwrap();
    }

    /// A token of an earlier opening of the same vault file is refused,
    /// though it names the same wallet, the same data key and the same
    /// epoch number: the reopened vault is another instance.
    #[test]
    fn a_token_of_an_earlier_opening_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let os_store = Arc::new(crate::MemoryOsStore::new());
        vault_with_wallet_in(dir.path(), &os_store, None, 0x5a);

        // Two later openings of that file, each loading the key on demand.
        let earlier = open_in(dir.path(), &os_store);
        let token = redeem(&earlier, GrantPurpose::PlatformOp, Credential::None);
        let reopened = open_in(dir.path(), &os_store);
        reopened.full_dek().unwrap();
        assert_eq!(earlier.inner().epoch, reopened.inner().epoch);
        assert_refuses(&reopened, &earlier, Credential::None, "reopened");
        earlier.scan_key(&[1; 32], &token).unwrap();

        // Encrypted: both openings unlocked once.
        let dir = tempfile::tempdir().unwrap();
        let first = vault_with_wallet(dir.path(), Some(b"pw"), 0x5a);
        drop(first);
        let os_store = Arc::new(crate::MemoryOsStore::new());
        let earlier = open_in(dir.path(), &os_store);
        earlier.unlock(b"pw", UnlockScope::Full).unwrap();
        let reopened = open_in(dir.path(), &os_store);
        reopened.unlock(b"pw", UnlockScope::Full).unwrap();
        assert_eq!(earlier.inner().epoch, reopened.inner().epoch);
        // Both unlocked with scope Full, so these passphrase grants carry
        // no key of their own and would use the receiving vault's.
        assert_refuses(
            &reopened,
            &earlier,
            Credential::Passphrase(b"pw"),
            "reopened, unlocked",
        );
    }

    /// A token of an earlier unlock of this vault is refused after a lock
    /// and unlock: `GrantInvalid` for a signer or scan key, `Locked` for a
    /// use of its key.
    #[test]
    fn a_token_of_an_earlier_unlock_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let v = vault_with_wallet(dir.path(), Some(b"pw"), 0x5a);
        let w = [1u8; 32];
        let platform = redeem(&v, GrantPurpose::PlatformOp, Credential::None);
        let spend = redeem(&v, GrantPurpose::Spend { max_duffs: 1 }, Credential::None);
        let reveal = redeem(
            &v,
            GrantPurpose::RevealSecret,
            Credential::Passphrase(b"pw"),
        );
        v.lock();
        v.unlock(b"pw", UnlockScope::Full).unwrap();
        assert_eq!(
            v.scan_key(&w, &platform).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            v.platform_signer(&w, &platform, SignerScope::PlatformIdentity)
                .map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            v.signer(&w, &spend).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            v.export_wallet_secret(&w, &reveal).map(drop),
            Err(VaultError::Locked)
        );
        // A token of this unlock works.
        let platform = redeem(&v, GrantPurpose::PlatformOp, Credential::None);
        v.scan_key(&w, &platform).unwrap();
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

    /// Review DW-E0-03 r3 m3: a passphrase change ends the epoch. Grants
    /// (pending or redeemed), grant tokens and signers issued before it are
    /// refused, both those that carry a key unwrapped with the old
    /// passphrase (locked vault) and those that use the vault's key
    /// (unlocked); the lock state is kept and new grants work. A refused
    /// change revokes nothing.
    #[test]
    fn a_passphrase_change_revokes_grants_tokens_and_signers() {
        let dir = tempfile::tempdir().unwrap();
        let v = vault_with_wallet(dir.path(), Some(b"old-pw"), 0x5a);
        let w = [1u8; 32];
        let key = DerivationPath::from_str("m/9'/1'/5'/0'/0'/0'/0'").unwrap();
        let peer = dashcore::secp256k1::PublicKey::from_secret_key(
            &dashcore::secp256k1::Secp256k1::new(),
            &dashcore::secp256k1::SecretKey::from_slice(&[0x42; 32]).unwrap(),
        );

        v.lock();
        let old = Credential::Passphrase(b"old-pw");
        let reveal = redeem(&v, GrantPurpose::RevealSecret, old);
        let pending = v
            .authorize(GrantPurpose::RevealSecret, Some(&w), old)
            .unwrap();
        let platform = redeem(&v, GrantPurpose::PlatformOp, old);
        let crypto = v
            .platform_signer(&w, &platform, SignerScope::DashPayCrypto)
            .unwrap();
        crypto.ecdh_shared_secret(&key, &peer).unwrap();
        v.change_passphrase(b"old-pw", b"new-pw-1").unwrap();
        assert_eq!(v.lock_state(), LockState::Locked);
        assert_eq!(
            v.export_wallet_secret(&w, &reveal).map(drop),
            Err(VaultError::Locked)
        );
        assert_eq!(
            v.reveal_mnemonic(&w, &pending.id).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            v.scan_key(&w, &platform).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            crypto.ecdh_shared_secret(&key, &peer).map(drop),
            Err(SignerError::Locked)
        );
        assert!(matches!(
            v.authorize(GrantPurpose::RevealSecret, Some(&w), old),
            Err(VaultError::WrongPassphrase { .. })
        ));
        let platform = redeem(
            &v,
            GrantPurpose::PlatformOp,
            Credential::Passphrase(b"new-pw-1"),
        );
        v.scan_key(&w, &platform).unwrap();

        v.unlock(b"new-pw-1", UnlockScope::Full).unwrap();
        let crypto = v.dashpay_crypto_signer(&w).unwrap();
        let spend = redeem(&v, GrantPurpose::Spend { max_duffs: 1 }, Credential::None);
        let spend_signer = v.signer(&w, &spend).unwrap();
        let reveal = redeem(
            &v,
            GrantPurpose::RevealSecret,
            Credential::Passphrase(b"new-pw-1"),
        );
        assert!(matches!(
            v.change_passphrase(b"wrong", b"new-pw-2"),
            Err(VaultError::WrongPassphrase { .. })
        ));
        crypto.ecdh_shared_secret(&key, &peer).unwrap();
        v.change_passphrase(b"new-pw-1", b"new-pw-2").unwrap();
        assert_eq!(v.lock_state(), LockState::Unlocked);
        assert_eq!(
            crypto.ecdh_shared_secret(&key, &peer).map(drop),
            Err(SignerError::Locked)
        );
        assert_eq!(
            spend_signer
                .with_key(&key, crate::signer::KeyUse::PublicKey, |_, _| ())
                .map(drop),
            Err(SignerError::Locked)
        );
        assert_eq!(
            v.signer(&w, &spend).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            v.export_wallet_secret(&w, &reveal).map(drop),
            Err(VaultError::Locked)
        );
        v.dashpay_crypto_signer(&w)
            .unwrap()
            .ecdh_shared_secret(&key, &peer)
            .unwrap();
        let reveal = redeem(
            &v,
            GrantPurpose::RevealSecret,
            Credential::Passphrase(b"new-pw-2"),
        );
        v.export_wallet_secret(&w, &reveal).unwrap();
    }

    #[test]
    fn seed_record_ids_parse() {
        let w = [0xab; 32];
        assert_eq!(seed_record_wallet(&record_id(&w, REC_SEED)), Some(w));
        assert_eq!(seed_record_wallet(&record_id(&w, REC_MNEMONIC)), None);
    }
}
