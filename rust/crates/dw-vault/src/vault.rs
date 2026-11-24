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
//!   `export_wallet_secret`, `with_revealed_seed`, `masternode_key`,
//!   `open_backup_bundle` of this vault's own bundle) and the
//!   token-authorized writes (`encrypt`, `enroll_quick_unlock`,
//!   `set_quick_unlock_spend_limit`), whose file write is inside.
//!   `seed_derivation` and `core_mnemonic_check`, which read a secret with
//!   the vault's own key, are gated too. Every epoch change (lock, unlock,
//!   scope change, passphrase change, encrypt, recover, destroy) holds it
//!   exclusively, so `lock()` returns only once every such operation that
//!   started before it has finished, and any operation after it sees the
//!   new epoch and fails `Locked`. Every deletion of a seed
//!   (`wipe_wallet_secret`, `delete_wallet_secret`, the old vault that
//!   `recover_with_mnemonic` replaces) holds it exclusively as well, from
//!   before the records go until they are gone, without ending the epoch
//!   unless it changes one anyway ([`Vault::drain_ops`]): a signer that read
//!   a seed has released its result before the seed is deleted, and one
//!   that starts after finds none (DP2-02). Every change of a key attached
//!   to a masternode (`store_masternode_key`, `delete_masternode_keys`)
//!   drains the same way, since it may replace or delete a secret. Not
//!   gated: `backup_bundle`, which takes no grant or epoch and returns
//!   only ciphertext, `open_backup_bundle` through a bundle's passphrase
//!   slot (no vault key), and `store_wallet_secret`, a record write under
//!   the vault's own key that never replaces a seed (a wallet holding
//!   different records is refused), so no signer can read a seed it
//!   changes.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant, SystemTime};

use key_wallet::Network;
use key_wallet::bip32::DerivationPath;
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
    AuthGrant, Clock, Credential, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, GrantKind, GrantPurpose,
    GrantToken, KeyHold, KeySource, LockState, PASSPHRASE_MAX_AGE_SECS, PolicyCommitments,
    QUICK_UNLOCK_SPEND_LIMITS, QuickUnlockPolicy, RevealedMnemonic, SIGNING_WINDOW_SECS,
    SeedDerivation, SignerLife, UnlockScope, VaultConfig, VaultStatus, WalletId, WalletSecret,
};
use crate::vote::VoteSigner;
use crate::{SignerError, VaultError};

mod compat;
mod links;
pub(crate) mod masternode;
use compat::BackupKek;
pub use compat::{CoreMnemonicCheck, WalletBackupBundle, reads_bundle_version};
pub use links::{
    ALL_LINK_KINDS, MAX_STASHED_LINK_BYTES, MAX_STASHED_LINKS, OTHER_KIND, StashedLinkKind,
    StashedLinkMeta,
};
pub use masternode::{MASTERNODE_KEY_ROLES, MAX_MASTERNODE_KEY_BYTES};

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
    /// A seed deletion now waits for the gate ([`Vault::drain_ops`]).
    DrainWaiting,
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
    /// The manifest's `policy` when this was sealed ([`put_policy`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    generation: Option<u64>,
}

impl Default for PolicyPayload {
    fn default() -> Self {
        Self {
            spend_limit_duffs: DEFAULT_QUICK_UNLOCK_SPEND_LIMIT,
            last_passphrase_at: None,
            generation: None,
        }
    }
}

impl PolicyPayload {
    /// What a policy the manifest names but the file no longer holds, or
    /// holds only in an older copy, counts as: no spending without the
    /// passphrase, and no recent passphrase entry.
    fn withdrawn() -> Self {
        Self {
            spend_limit_duffs: 0,
            last_passphrase_at: None,
            generation: None,
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

/// Seals `payload` as the policy of `next` at a new manifest generation,
/// which the manifest records as the policy's, so the sealed policy it
/// replaces is refused if it is put back. Returns that generation; the
/// caller raises `high_water` to it once the file is written.
fn put_policy(
    next: &mut VaultFile,
    dek: &[u8; 32],
    high_water: u64,
    floor: Option<u64>,
    payload: PolicyPayload,
) -> Result<u64, VaultError> {
    let mut manifest = verify_manifest(next, dek, high_water)?;
    manifest.generation = manifest
        .generation
        .max(floor.unwrap_or(0))
        .checked_add(1)
        .ok_or_else(|| VaultError::Storage("the policy commitment cannot be read".into()))?;
    manifest.policy = Some(manifest.generation);
    next.manifest = seal_manifest(dek, &next.vault_id, &next.network, &manifest)?;
    next.quick_unlock = Some(seal_policy(
        dek,
        &next.vault_id,
        &next.network,
        PolicyPayload {
            generation: manifest.policy,
            ..payload
        },
    )?);
    Ok(manifest.generation)
}

/// The floor while the commitment cannot be read: no policy passes it.
const UNREAD: u64 = u64::MAX;

fn raise(floor: &mut Option<u64>, generation: u64) {
    *floor = Some(floor.unwrap_or(0).max(generation));
}

/// The authenticated policy of `f`; `None` when the file has none and never
/// had one this build wrote. A plain copy that differs from the sealed one,
/// or a manifest that does not authenticate, is `Corrupt`. Once the
/// manifest names a policy generation, a missing policy or one sealed at
/// another generation is [`PolicyPayload::withdrawn`], and so is one older
/// than `floor`, the host's commitment, or none where one is committed.
fn open_policy(
    f: &VaultFile,
    dek: &[u8; 32],
    floor: Option<u64>,
) -> Result<Option<PolicyPayload>, VaultError> {
    let manifest = verify_manifest(f, dek, 0)?;
    if floor.is_some_and(|floor| manifest.policy.is_none_or(|p| p < floor)) {
        return Ok(Some(PolicyPayload::withdrawn()));
    }
    let Some(settings) = &f.quick_unlock else {
        return Ok(manifest.policy.map(|_| PolicyPayload::withdrawn()));
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
    match manifest.policy {
        Some(generation) if payload.generation != Some(generation) => {
            Ok(Some(PolicyPayload::withdrawn()))
        }
        _ => Ok(Some(payload)),
    }
}

#[cfg(test)]
type PersistHook = Box<dyn FnMut(&Vault)>;

#[cfg(test)]
thread_local! {
    static PERSIST_HOOK: std::cell::RefCell<Option<PersistHook>> =
        const { std::cell::RefCell::new(None) };
}

/// Tests: runs this thread's hook while a passphrase check is about to
/// write the file it changed.
#[cfg(test)]
fn test_persist_hook(vault: &Vault) {
    let hook = PERSIST_HOOK.with(|h| h.borrow_mut().take());
    if let Some(mut hook) = hook {
        hook(vault);
    }
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
    /// Redeemed from a signing window that has ended since.
    EndedWindow,
}

/// Wall and monotonic clocks at one look. A suspend that freezes the
/// monotonic clock moves only the wall clock, and a wall-clock step moves
/// it either way; when the two disagree the signing window ends. A suspend
/// that advances both is caught by the host's heartbeat
/// ([`Vault::signing_window_heartbeat`]). The OS suspend notification is
/// the real signal; these are the backstop.
struct SleepSample {
    wall: SystemTime,
    mono: Instant,
}

/// How far a clock may jump before this process treats the gap as a sleep.
/// Larger than a normal NTP step.
const SLEEP_GAP: Duration = Duration::from_secs(2);

/// How old the host's last heartbeat may be while the window admits
/// grants: its one-second tick plus [`SLEEP_GAP`].
const HEARTBEAT_MAX: Duration = Duration::from_secs(3);

/// The wall and monotonic clocks moved apart by more than [`SLEEP_GAP`]
/// since `from`, in either direction.
fn clocks_disagree(from: &SleepSample, wall: SystemTime, mono: Instant) -> bool {
    let mono_dt = mono.saturating_duration_since(from.mono);
    let apart = match wall.duration_since(from.wall) {
        Ok(wall_dt) => wall_dt.abs_diff(mono_dt),
        Err(back) => back.duration().saturating_add(mono_dt),
    };
    apart > SLEEP_GAP
}

/// The payments the signing window may issue without a passphrase.
fn window_purpose(purpose: &GrantPurpose) -> bool {
    matches!(
        purpose,
        GrantPurpose::Spend { .. }
            | GrantPurpose::PlatformOp { .. }
            | GrantPurpose::MasternodeWithdraw { .. }
    )
}

/// Opens a new window. The previous one ends first, with its grants and
/// its charge, so two windows never add up past the limit. It admits
/// grants only while the user's setting allows it for this vault
/// ([`Vault::set_signing_window_allowed`]).
fn arm_signing_window(inner: &mut Inner, clock: &dyn Clock) {
    clear_signing_window(inner);
    let mono = clock.monotonic();
    inner.signing_deadline = Some(mono + Duration::from_secs(SIGNING_WINDOW_SECS));
    inner.window_charged = 0;
    inner.sleep_sample = Some(SleepSample {
        wall: clock.wall(),
        mono,
    });
}

/// The user chose the signing window, not "ask every payment", for the
/// vault in memory now.
fn window_allowed(inner: &Inner) -> bool {
    match (&inner.window_vault, &inner.file) {
        (Some(id), Some(f)) => bool::from(id.ct_eq(&f.vault_id)),
        _ => false,
    }
}

/// The window is open, the user chose it, and, where the host must beat,
/// it has beaten at least once.
fn window_admits(inner: &Inner) -> bool {
    window_allowed(inner)
        && inner.signing_deadline.is_some()
        && (!inner.heartbeat_required || inner.heartbeat.is_some())
}

fn clear_signing_window(inner: &mut Inner) {
    inner.window_generation = inner.window_generation.wrapping_add(1);
    inner.signing_deadline = None;
    inner.window_charged = 0;
    inner.sleep_sample = None;
    inner.grants.retain(|_, grant| !grant.from_window);
}

/// Ends the window when the wall and monotonic clocks disagree since the
/// last look (a suspend the monotonic clock did not count, or a wall-clock
/// step either way), or when the host's heartbeat is older than
/// [`HEARTBEAT_MAX`] (a suspend that both clocks counted, or a host that
/// stopped watching). A long gap in both clocks with a fresh heartbeat is a
/// normal wait between looks.
fn observe_sleep(inner: &mut Inner, wall: SystemTime, mono: Instant) {
    let Some(sample) = &inner.sleep_sample else {
        return;
    };
    let stale_beat = inner
        .heartbeat
        .is_some_and(|beat| mono.saturating_duration_since(beat) > HEARTBEAT_MAX);
    if clocks_disagree(sample, wall, mono) || stale_beat {
        clear_signing_window(inner);
        return;
    }
    if inner.signing_deadline.is_some() {
        inner.sleep_sample = Some(SleepSample { wall, mono });
    }
}

fn settle_window(inner: &mut Inner, clock: &dyn Clock) {
    let now = clock.monotonic();
    settle_window_at(inner, clock.wall(), now);
}

fn settle_window_at(inner: &mut Inner, wall: SystemTime, now: Instant) {
    observe_sleep(inner, wall, now);
    if inner
        .signing_deadline
        .is_some_and(|deadline| now >= deadline)
    {
        clear_signing_window(inner);
    }
}

/// The sealed spend limit. An absent policy is the default. A policy that
/// does not authenticate, or whose plain copy was altered, refuses the
/// window (`CredentialRequired` at the caller) rather than trusting the
/// plain number.
fn sealed_spend_limit(inner: &Inner) -> Result<u64, VaultError> {
    let (Some(file), Some(dek)) = (inner.file.as_ref(), inner.dek.as_ref()) else {
        return Err(VaultError::CredentialRequired);
    };
    match open_policy(file, dek, inner.policy_floor) {
        Ok(Some(policy)) => Ok(policy.spend_limit_duffs),
        Ok(None) => Ok(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT),
        Err(_) => Err(VaultError::CredentialRequired),
    }
}

/// A grant held in vault memory.
struct IssuedGrant {
    grant: AuthGrant,
    /// The data key, when a passphrase authorized this grant on a vault that
    /// held no full-scope key (locked or mixing-only). Only this grant uses
    /// it; the vault's lock state is unchanged (dash-qt re-lock parity).
    key: Option<Key32>,
    /// Issued from the signing window, with no passphrase. Closing the
    /// window drops it, so it cannot be composed into a lease afterwards.
    from_window: bool,
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
    /// When the post-unlock signing window ends, on the vault clock's
    /// monotonic timeline. Memory only: not written, so a quit ends it.
    /// A wall-clock change does not move it. Once cleared, only a new
    /// passphrase unlock arms it again.
    signing_deadline: Option<Instant>,
    /// Duff already authorized from this window, across every credential-free
    /// grant. Not refunded when a grant goes unused. Cleared with the window.
    window_charged: u64,
    /// Clocks at the last look, for [`observe_sleep`].
    sleep_sample: Option<SleepSample>,
    /// The vault id the host said uses the signing window. Any other vault,
    /// one made by a recovery included, has none until the host says so.
    window_vault: Option<Vec<u8>>,
    /// Which signing window is open: changes whenever one ends or opens,
    /// so a token redeemed from an earlier one is refused.
    window_generation: u64,
    /// The host's last [`Vault::signing_window_heartbeat`]; none until the
    /// first.
    heartbeat: Option<Instant>,
    /// [`Vault::require_signing_window_heartbeat`] was called.
    heartbeat_required: bool,
    /// [`Vault::attach_policy_commitments`]; none in tests and tools.
    commitments: Option<Arc<dyn PolicyCommitments>>,
    /// The policy generation committed there for the vault in memory.
    policy_floor: Option<u64>,
    #[cfg(test)]
    gate_log: Option<Vec<GateEvent>>,
}

impl Inner {
    /// Drops the data key, revokes grants and invalidates outstanding signers.
    fn forget_key(&mut self, ops: &EpochGuard<'_>) {
        self.dek = None;
        self.backup_kek = None;
        self.scope = UnlockScope::Full;
        clear_signing_window(self);
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
    /// The link stash's in-process state (DP5-01, `links.rs`).
    links: Mutex<links::LinkState>,
    /// A vault exists and a stash entry beside it is still plaintext, as
    /// the last stash call found (`VaultStatus::unsealed_links`, DP5-01
    /// D26).
    links_unsealed: std::sync::atomic::AtomicBool,
}

/// A passphrase change whose old passphrase was verified
/// ([`Vault::check_passphrase_change`]); [`Vault::apply_passphrase_change`]
/// applies it. Holds the unwrapped data key and the new passphrase, both
/// zeroized on drop.
pub struct CheckedPassphraseChange {
    dek: Key32,
    new: Zeroizing<Vec<u8>>,
    /// Slot P as checked: the vault id and its wrapped data key.
    slot: (Vec<u8>, crypto::Sealed),
}

impl std::fmt::Debug for CheckedPassphraseChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CheckedPassphraseChange(..)")
    }
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
        let vault = Self {
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
                    signing_deadline: None,
                    window_charged: 0,
                    sleep_sample: None,
                    window_vault: None,
                    window_generation: 0,
                    heartbeat: None,
                    heartbeat_required: false,
                    commitments: None,
                    policy_floor: None,
                    #[cfg(test)]
                    gate_log: None,
                }),
                links: Mutex::new(links::LinkState::new()?),
                links_unsealed: std::sync::atomic::AtomicBool::new(false),
            }),
        };
        // App start: expired links and a crash's leftovers go, and
        // plaintext beside a vault is sealed to `stash.pub` (DP5-01 D26,
        // D29).
        vault.sweep_links_at_open();
        Ok(vault)
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

    /// Whether `event` was logged since [`Self::start_gate_log`]; logging
    /// goes on.
    #[cfg(test)]
    pub(crate) fn gate_logged(&self, event: GateEvent) -> bool {
        (self.inner().gate_log.as_ref()).is_some_and(|log| log.contains(&event))
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

    /// [`Self::persist`] of a file whose manifest is at `generation`: the
    /// file and the high-water mark are published together, so no reader
    /// checks the old file against the new mark. The policy generation is
    /// committed first ([`Self::commit_policy`]); a failed commit writes
    /// nothing, except for a lower policy (`end_window`), which is written
    /// anyway, as without a store. With `end_window`, the signing window
    /// ends in the same step, so no window grant issued under the old policy
    /// signs under the new one; it ends when the write fails too.
    fn persist_at(
        &self,
        _writer: &WriteGuard<'_>,
        next: VaultFile,
        generation: u64,
        end_window: bool,
    ) -> Result<(), VaultError> {
        let committed = match self.commit_policy(&next.vault_id, generation) {
            Err(e) if !end_window => return Err(e),
            Err(e) => {
                tracing::warn!(error = %e, "could not commit a lower spending limit; writing it anyway");
                None
            }
            Ok(stored) => stored,
        };
        let written = file::write(&self.shared.dir, &next);
        let mut inner = self.inner();
        if let Some(stored) = committed {
            raise(&mut inner.policy_floor, stored);
        }
        if end_window {
            clear_signing_window(&mut inner);
        }
        written?;
        inner.file = Some(next);
        inner.high_water = inner.high_water.max(generation);
        Ok(())
    }

    /// Commits `generation` as the policy's, durably, before the file that
    /// seals it is written: a crash between the two leaves the commitment
    /// ahead, so the file's policy counts as withdrawn (limit 0) until the
    /// limit is set again, and an older, higher limit never counts.
    /// Returns what the store holds afterwards; `None` without a store.
    fn commit_policy(&self, vault_id: &[u8], generation: u64) -> Result<Option<u64>, VaultError> {
        let Some(store) = self.inner().commitments.clone() else {
            return Ok(None);
        };
        store
            .commit(vault_id, generation)
            .map(Some)
            .map_err(VaultError::Storage)
    }

    /// The floor read from `store` for the vault in memory; [`UNREAD`]
    /// when the read fails, until a later one succeeds.
    fn read_floor(&self, store: &dyn PolicyCommitments) -> Option<u64> {
        let id = self.vault_id()?;
        store.committed(&id).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "could not read the policy commitment; the spending limit is 0 until it is read");
            Some(UNREAD)
        })
    }

    /// The host's store for policy commitments, kept outside the vault
    /// file. The commitment of the vault in memory is read now. A file
    /// that names a newer policy than the store holds (after an upgrade, a
    /// recovery, or a passphrase check that re-sealed the passphrase time)
    /// has it recorded at the next passphrase check; a new limit is
    /// committed before it is written. A recovery keeps the replaced
    /// vault's commitment and a destroy drops it. A commitment that cannot
    /// be read counts every policy as withdrawn (limit 0) and is read again
    /// at each passphrase check. Without a store the vault has only its
    /// file's own check.
    pub fn attach_policy_commitments(&self, store: Arc<dyn PolicyCommitments>) {
        let _writer = self.writer();
        let floor = self.read_floor(&*store);
        let mut inner = self.inner();
        inner.policy_floor = floor;
        inner.commitments = Some(store);
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
                unsealed_links: false,
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
            unsealed_links: self.links_unsealed(),
        }
    }

    /// Current state. In-memory read; also drops expired grants.
    pub fn status(&self) -> VaultStatus {
        let now = self.now();
        let mut inner = self.inner();
        inner.drop_expired_grants(now);
        self.status_of(&inner)
    }

    /// The vault file's random id, made when the vault is created (or made
    /// again by a recovery) and kept by `encrypt` and passphrase changes;
    /// `None` without a vault. A new vault never has an old vault's id, so
    /// host settings keyed by it start fresh. In-memory read.
    pub fn vault_id(&self) -> Option<Vec<u8>> {
        self.inner().file.as_ref().map(|f| f.vault_id.clone())
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
    /// vault unlocked. The new vault's signing window admits nothing until
    /// the host allows it ([`Self::set_signing_window_allowed`]).
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
            policy: None,
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
        let link_dek = Zeroizing::new(*dek);
        let ops = self.epoch_guard();
        let mut inner = self.inner();
        inner.file = Some(vault_file);
        inner.high_water = 1;
        inner.policy_floor = None;
        inner.install_key(dek, UnlockScope::Full, &ops);
        inner.backup_kek = backup_kek;
        if encrypted {
            inner.last_passphrase_at = Some(now);
            arm_signing_window(&mut inner, self.clock());
        }
        let mut status = self.status_of(&inner);
        drop(inner);
        drop(ops);
        // Links stashed before the vault existed move into it (§2.9),
        // before the writer goes: no recovery or destroy runs between
        // (D23). A failure leaves the vault created and says so (D26).
        status.unsealed_links = self.sync_links_after_key(&link_dek, &vault_id);
        drop(writer);
        Ok(status)
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
        let link_dek = Zeroizing::new(*dek);
        let (mut status, vault_id) = {
            let ops = self.epoch_guard();
            let mut inner = self.inner();
            inner.install_key(dek, scope, &ops);
            inner.backup_kek = Some(backup_kek);
            if scope == UnlockScope::Full {
                arm_signing_window(&mut inner, self.clock());
            } else {
                clear_signing_window(&mut inner);
            }
            let vault_id = inner.file.as_ref().map(|f| f.vault_id.clone());
            (self.status_of(&inner), vault_id)
        };
        if let Some(vault_id) = vault_id {
            status.unsealed_links = self.sync_links_after_key(&link_dek, &vault_id);
        }
        drop(writer);
        Ok(status)
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
        self.apply_passphrase_change(self.check_passphrase_change(old, new)?)
    }

    /// The first half of [`Self::change_passphrase`]: validates `new` and
    /// verifies `old` (one KDF run; a wrong one counts as a failed attempt),
    /// changing nothing else. The engine runs it before it revokes anything
    /// (E0-04 DEC-134), so a wrong old passphrase leaves every lease alone.
    pub fn check_passphrase_change(
        &self,
        old: &[u8],
        new: &[u8],
    ) -> Result<CheckedPassphraseChange, VaultError> {
        let new = new_passphrase(new)?;
        let writer = self.writer();
        let (dek, _) = self.check_passphrase(&writer, old)?;
        let slot = self.slot_identity(&self.inner())?;
        Ok(CheckedPassphraseChange { dek, new, slot })
    }

    /// The second half of [`Self::change_passphrase`]: re-wraps the data key
    /// under the checked new passphrase. Refused with `WrongPassphrase` (not
    /// counted as an attempt) when slot P changed since the check: the old
    /// passphrase was checked against a slot that is gone.
    pub fn apply_passphrase_change(
        &self,
        change: CheckedPassphraseChange,
    ) -> Result<VaultStatus, VaultError> {
        let CheckedPassphraseChange { dek, new, slot } = change;
        let writer = self.writer();
        {
            let inner = self.inner();
            if self.slot_identity(&inner)? != slot {
                let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
                return Err(VaultError::WrongPassphrase {
                    failed_attempts: f.throttle.failed_attempts,
                    retry_after_secs: None,
                });
            }
        }
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
        clear_signing_window(&mut inner);
        // Backups written from now on open with the new passphrase.
        if inner.dek.is_some() {
            inner.backup_kek = Some(BackupKek::of(kdf, &salt, &kek));
        }
        drop(writer);
        Ok(self.status_of(&inner))
    }

    /// What identifies slot P: the vault and its wrapped data key, which
    /// every passphrase change replaces.
    fn slot_identity(&self, inner: &Inner) -> Result<(Vec<u8>, crypto::Sealed), VaultError> {
        let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
        let slot = f.slot_p.as_ref().ok_or(VaultError::NotEncrypted)?;
        Ok((f.vault_id.to_vec(), slot.wrapped_dek.clone()))
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
        let unread = {
            let inner = self.inner();
            let unread = inner.policy_floor == Some(UNREAD);
            inner.commitments.clone().filter(|_| unread)
        };
        if let Some(store) = unread {
            let floor = self.read_floor(&*store);
            self.inner().policy_floor = floor;
        }
        let (had_failures, policy, record) = {
            let mut inner = self.inner();
            let f = inner.file.as_ref().ok_or(VaultError::NoVault)?;
            let manifest = verify_manifest(f, &dek, inner.high_water)?;
            let had_failures = f.throttle != Throttle::default();
            // With slot B enrolled the passphrase time is enforced from the
            // file, so it is persisted (sealed) on every successful check.
            // A sealed policy that no longer authenticates (the manifest did,
            // just above) is left as it is: it counts as none, the passphrase
            // time is kept in memory, and the setter replaces the policy
            // (#173 item 10). Refusing here would refuse the passphrase too.
            let policy = if f.slot_b.is_some() && inner.policy_floor != Some(UNREAD) {
                match open_policy(f, &dek, inner.policy_floor) {
                    Ok(policy) => Some(policy.unwrap_or_default()),
                    Err(VaultError::Corrupt(_)) => None,
                    Err(e) => return Err(e),
                }
            } else {
                None
            };
            let floor = inner.policy_floor.unwrap_or(0);
            let record = manifest
                .policy
                .filter(|&named| named > floor)
                .zip(inner.commitments.clone())
                .map(|(named, store)| (named, store, f.vault_id.clone()));
            inner.high_water = inner.high_water.max(manifest.generation);
            inner.last_passphrase_at = Some(now);
            (had_failures, policy, record)
        };
        if let Some((named, store, vault_id)) = record {
            match store.commit(&vault_id, named) {
                Ok(stored) => raise(&mut self.inner().policy_floor, stored),
                Err(e) => tracing::warn!(error = %e, "could not record the policy commitment"),
            }
        }
        if had_failures || policy.is_some() {
            let mut next = self.file_copy(writer)?;
            next.throttle = Throttle::default();
            let mut generation = 0;
            if let Some(policy) = policy {
                let (high_water, floor) = {
                    let inner = self.inner();
                    (inner.high_water, inner.policy_floor)
                };
                generation = put_policy(
                    &mut next,
                    &dek,
                    high_water,
                    floor,
                    PolicyPayload {
                        last_passphrase_at: Some(now),
                        ..policy
                    },
                )?;
            }
            #[cfg(test)]
            test_persist_hook(self);
            // The passphrase is already proven: a failed write of the reset
            // counter (or the sealed passphrase time) must not refuse it (a
            // full disk would otherwise lock the user out). The new file is
            // kept in memory and the write is retried by the next file
            // change, which starts from memory. The file and its high-water
            // mark are published together, after the write.
            if let Err(e) = file::write(&self.shared.dir, &next) {
                tracing::warn!(error = %e, "could not persist the reset failed-attempt counter");
            }
            let mut inner = self.inner();
            inner.file = Some(next);
            inner.high_water = inner.high_water.max(generation);
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
    ///   purposes, which need the passphrase whenever slot P exists. With
    ///   slot P, a payment (`Spend`, `PlatformOp`, `MasternodeWithdraw`)
    ///   also needs the open signing window, within its sealed limit; a
    ///   limit of 0 admits none.
    /// - `QuickUnlock`: the slot B wrap key. Issues `Spend`, `PlatformOp`,
    ///   `SignMessage`, `Governance`, `MasternodeOp` and `MasternodeWithdraw`
    ///   grants whose value
    ///   ([`GrantPurpose::quick_unlock_value`]) is within the spending limit
    ///   (`QuickUnlockLimitExceeded` above it), only while the passphrase was
    ///   entered within [`PASSPHRASE_MAX_AGE_SECS`] (`PassphraseStale`).
    ///   Every other purpose is `CredentialRequired`. The lock state does not
    ///   change; on a locked or mixing-only vault the grant carries its own
    ///   key, as a passphrase grant does.
    pub fn authorize(
        &self,
        purpose: GrantPurpose,
        wallet: Option<&WalletId>,
        credential: Credential<'_>,
    ) -> Result<AuthGrant, VaultError> {
        let mut grants = self.authorize_set(&[purpose], wallet, credential)?;
        grants
            .pop()
            .ok_or_else(|| VaultError::Internal("no grant issued".into()))
    }

    /// One credential check for several grants (E0-04 design §3.3): one
    /// Argon2id run and one throttle count issue one grant per purpose, in
    /// order, all bound to `wallet` with the same lifetime. Each grant of a
    /// vault without a full-scope key carries its own copy of the key, as a
    /// single grant does. Every purpose must be allowed for the credential
    /// ([`Self::authorize`]) or none is issued. With `QuickUnlock` the sum
    /// of the grants' values must be within the spending limit (§3.7), so
    /// "Accept and pay" (`[PlatformOp{0, accept_cost}, Spend{amount + fee}]`)
    /// reaches the limit once. An empty set is `InvalidArgument`.
    pub fn authorize_set(
        &self,
        purposes: &[GrantPurpose],
        wallet: Option<&WalletId>,
        credential: Credential<'_>,
    ) -> Result<Vec<AuthGrant>, VaultError> {
        if purposes.is_empty() {
            return Err(VaultError::InvalidArgument("no grant purpose".into()));
        }
        if let Some(p) = purposes
            .iter()
            .find(|p| p.wallet_scoped() != wallet.is_some())
        {
            let why = if wallet.is_none() {
                "needs a wallet id"
            } else {
                "is not bound to a wallet"
            };
            return Err(VaultError::InvalidArgument(format!(
                "a {:?} grant {why}",
                p.kind()
            )));
        }
        match credential {
            Credential::QuickUnlock(wrap_key) => {
                self.authorize_quick_unlock(purposes, wallet, wrap_key)
            }
            Credential::Passphrase(pw) => {
                let writer = self.writer();
                let (dek, _) = self.check_passphrase(&writer, pw)?;
                let mut inner = self.inner();
                let full_in_memory = inner.dek.is_some() && inner.scope == UnlockScope::Full;
                self.issue(
                    &mut inner,
                    purposes,
                    wallet,
                    (!full_in_memory).then_some(&dek),
                    false,
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
                        if purposes.iter().any(GrantPurpose::requires_credential) =>
                    {
                        return Err(VaultError::CredentialRequired);
                    }
                    LockState::Locked => return Err(VaultError::Locked),
                    LockState::UnlockedMixingOnly => return Err(VaultError::MixingOnly),
                    LockState::NoKeys | LockState::Unencrypted | LockState::Unlocked => {}
                }
                let window = Self::state_of(&inner) == LockState::Unlocked
                    && inner.file.as_ref().is_some_and(|f| f.slot_p.is_some())
                    && purposes.iter().any(window_purpose);
                let charge = if window {
                    Some(self.admit_window(&mut inner, purposes)?)
                } else {
                    None
                };
                let grants = self.issue(&mut inner, purposes, wallet, None, window)?;
                if let Some(n) = charge {
                    inner.window_charged = inner.window_charged.saturating_add(n);
                }
                Ok(grants)
            }
        }
    }

    /// The signing-window half of [`Self::authorize_set`], under the same
    /// `inner` guard as the grant insert. The limit is the sealed policy,
    /// and it is cumulative for the window: several grants cannot add up
    /// past it. A closed or corrupt window is `CredentialRequired`.
    fn admit_window(
        &self,
        inner: &mut Inner,
        purposes: &[GrantPurpose],
    ) -> Result<u64, VaultError> {
        settle_window(inner, self.clock());
        if !window_admits(inner) {
            return Err(VaultError::CredentialRequired);
        }
        let limit = sealed_spend_limit(inner)?;
        if limit == 0 {
            return Err(VaultError::CredentialRequired);
        }
        let mut total = 0u64;
        for purpose in purposes {
            if !window_purpose(purpose) {
                continue;
            }
            let value = purpose
                .quick_unlock_value()
                .ok_or(VaultError::CredentialRequired)?;
            total = total.saturating_add(value);
        }
        if inner.window_charged.saturating_add(total) > limit {
            return Err(VaultError::CredentialRequired);
        }
        Ok(total)
    }

    fn clock_mono(&self) -> Instant {
        self.shared.config.clock.monotonic()
    }

    fn clock(&self) -> &dyn Clock {
        &*self.shared.config.clock
    }

    /// The `QuickUnlock` arm of [`Self::authorize_set`].
    fn authorize_quick_unlock(
        &self,
        purposes: &[GrantPurpose],
        wallet: Option<&WalletId>,
        wrap_key: &[u8],
    ) -> Result<Vec<AuthGrant>, VaultError> {
        // The sum saturates at `u64::MAX`, above every spending limit.
        let mut total = 0u64;
        for purpose in purposes {
            let value = purpose
                .quick_unlock_value()
                .ok_or(VaultError::CredentialRequired)?;
            total = total.saturating_add(value);
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
        let policy = open_policy(f, &dek, inner.policy_floor)?.unwrap_or_default();
        inner.high_water = inner.high_water.max(manifest.generation);

        // A clock set back gives age 0 (fresh); one set forward only stales.
        let fresh = policy
            .last_passphrase_at
            .is_some_and(|at| now.saturating_sub(at) <= PASSPHRASE_MAX_AGE_SECS);
        if !fresh {
            return Err(VaultError::PassphraseStale);
        }
        if total > policy.spend_limit_duffs {
            return Err(VaultError::QuickUnlockLimitExceeded {
                limit_duffs: policy.spend_limit_duffs,
            });
        }
        let full_in_memory = inner.dek.is_some() && inner.scope == UnlockScope::Full;
        self.issue(
            &mut inner,
            purposes,
            wallet,
            (!full_in_memory).then_some(&dek),
            false,
        )
    }

    /// Stores one new grant per purpose, dropping expired ones; each gets
    /// its own copy of `key`. `from_window` marks the credential-free
    /// payment grants of the set, so closing the window drops them; another
    /// purpose in the same set is not the window's.
    fn issue(
        &self,
        inner: &mut Inner,
        purposes: &[GrantPurpose],
        wallet: Option<&WalletId>,
        key: Option<&Key32>,
        from_window: bool,
    ) -> Result<Vec<AuthGrant>, VaultError> {
        let now = self.now();
        inner.drop_expired_grants(now);
        let expires_at = now.saturating_add(self.shared.config.grant_ttl_secs);
        // Every id is made before any grant is stored: all or nothing.
        let grants = purposes
            .iter()
            .map(|&purpose| {
                Ok(AuthGrant {
                    id: hex::encode(crypto::random_array::<16>()?),
                    purpose,
                    wallet: wallet.copied(),
                    expires_at,
                    single_use: true,
                })
            })
            .collect::<Result<Vec<_>, VaultError>>()?;
        for grant in &grants {
            inner.grants.insert(
                grant.id.clone(),
                IssuedGrant {
                    grant: grant.clone(),
                    key: key.cloned(),
                    from_window: from_window && window_purpose(&grant.purpose),
                },
            );
        }
        Ok(grants)
    }

    /// Invalidates a grant. Unknown ids are ignored.
    pub fn revoke_grant(&self, grant_id: &str) {
        self.inner().grants.remove(grant_id);
    }

    /// Checks that `grant_id` is live, of kind `expected` and bound to
    /// `wallet` (`None` for vault-wide purposes). The signing window is
    /// settled first, so a grant it issued ends with it.
    fn valid_grant<'a>(
        inner: &'a mut Inner,
        grant_id: &str,
        expected: GrantKind,
        wallet: Option<&WalletId>,
        now: u64,
        clock: &dyn Clock,
    ) -> Result<&'a IssuedGrant, VaultError> {
        settle_window(inner, clock);
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
        self.grant_purpose(grant_id, expected, wallet).map(|_| ())
    }

    /// [`Vault::check_grant`], returning the grant's purpose: lets a caller
    /// hold a cost against the grant's caps and refuse with the grant left
    /// for another try.
    pub fn grant_purpose(
        &self,
        grant_id: &str,
        expected: GrantKind,
        wallet: Option<&WalletId>,
    ) -> Result<GrantPurpose, VaultError> {
        let now = self.now();
        let mut inner = self.inner();
        let state = Self::state_of(&inner);
        let found = Self::valid_grant(&mut inner, grant_id, expected, wallet, now, self.clock())
            .map(|issued| (issued.key.is_some(), issued.grant.purpose));
        match found {
            Ok((true, purpose)) => return Ok(purpose),
            // A missing grant on a locked vault was most likely revoked by
            // the lock, so the lock is reported first.
            Ok((false, _)) | Err(VaultError::GrantInvalid) => match state {
                LockState::NoVault => return Err(VaultError::NoVault),
                LockState::Locked => return Err(VaultError::Locked),
                LockState::UnlockedMixingOnly => return Err(VaultError::MixingOnly),
                _ => {}
            },
            Err(_) => {}
        }
        found.map(|(_, purpose)| purpose)
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
        Self::valid_grant(&mut inner, grant_id, expected, wallet, now, self.clock())?;
        let issued = inner
            .grants
            .remove(grant_id)
            .ok_or(VaultError::GrantInvalid)?;
        Ok(GrantToken {
            purpose: issued.grant.purpose,
            wallet: issued.grant.wallet,
            vault_instance: self.shared.instance,
            epoch: inner.epoch,
            window: issued.from_window.then_some(inner.window_generation),
            key: issued
                .key
                .map_or(KeySource::Vault, |key| KeySource::Own(Arc::new(key))),
            issued: AtomicBool::new(false),
        })
    }

    /// Whether the identity key at `path` of `wallet` is the on-chain key
    /// `key_data` (dw-vault `VaultSigner::identity_key_matches`), asked
    /// through a `PlatformOp` grant without consuming it: lets a write
    /// refuse a key the vault does not hold with the grant left for another
    /// try (DP2-02). Needs what a `PlatformIdentity` signer from the grant
    /// would (a credit cap above 0, else `GrantPurposeMismatch`; the grant's
    /// own key or the vault's full key) and answers only yes or no: nothing
    /// signs, and no key leaves the vault. An answer, not a promise: the
    /// redemption that relies on it is [`Self::redeem_grant_holding_key`].
    pub fn grant_identity_key_matches(
        &self,
        grant_id: &str,
        wallet: &WalletId,
        path: &DerivationPath,
        key_data: &[u8],
    ) -> Result<bool, VaultError> {
        Self::key_held(
            &self.grant_identity_signer(grant_id, wallet)?,
            path,
            key_data,
        )
    }

    /// Redeems a `PlatformOp` grant for `wallet` only while the vault holds
    /// one of `candidates` (identity key paths and on-chain key data, as
    /// [`Self::grant_identity_key_matches`] asks): the check and the
    /// redemption are one section under the guard of the vault's record
    /// writes, so a wallet removal that deletes the seed either came first
    /// or waits until the grant is redeemed (DP2-02). `Ok(None)`: no
    /// candidate is held (or the wallet has no seed), and the grant is left
    /// unused. A lock in between ends the grant (`GrantInvalid`, or `Locked`
    /// while a key is derived). An unencrypted vault whose key is not cached
    /// reads it from the OS store inside the section, so record writes wait
    /// on that read.
    pub fn redeem_grant_holding_key(
        &self,
        grant_id: &str,
        wallet: &WalletId,
        candidates: &[(DerivationPath, Vec<u8>)],
    ) -> Result<Option<GrantToken>, VaultError> {
        let _writer = self.writer();
        let signer = match self.grant_identity_signer(grant_id, wallet) {
            Err(VaultError::NoSecret) => return Ok(None),
            signer => signer?,
        };
        for (path, key_data) in candidates {
            match Self::key_held(&signer, path, key_data) {
                Ok(true) => {
                    return self
                        .redeem_grant(grant_id, GrantKind::PlatformOp, Some(wallet))
                        .map(Some);
                }
                Ok(false) | Err(VaultError::NoSecret) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(None)
    }

    /// A `PlatformIdentity` signer under an unredeemed `PlatformOp` grant,
    /// for the yes-or-no questions above (no token is issued, so it cannot
    /// reach a lease).
    fn grant_identity_signer(
        &self,
        grant_id: &str,
        wallet: &WalletId,
    ) -> Result<VaultSigner, VaultError> {
        let GrantPurpose::PlatformOp { max_credits, .. } =
            self.grant_purpose(grant_id, GrantKind::PlatformOp, Some(wallet))?
        else {
            return Err(VaultError::GrantPurposeMismatch);
        };
        if max_credits == 0 {
            return Err(VaultError::GrantPurposeMismatch);
        }
        let check = |inner: &mut Inner| {
            Self::valid_grant(
                inner,
                grant_id,
                GrantKind::PlatformOp,
                Some(wallet),
                self.now(),
                self.clock(),
            )
            .map(|issued| issued.key.as_ref().map(|key| Zeroizing::new(**key)))
        };
        let own = check(&mut self.inner())?;
        if own.is_none() {
            self.full_dek()?;
        }
        #[cfg(test)]
        crate::signer::test_hook::checked();
        // The signer is issued under a guard that finds the grant again:
        // every epoch change ends it, so no probe runs on a grant that was
        // redeemed, or ended by a lock or passphrase change, in between.
        let mut inner = self.inner();
        check(&mut inner)?;
        let key = own.map_or(KeySource::Vault, |key| KeySource::Own(Arc::new(key)));
        if matches!(key, KeySource::Vault) && inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        self.signer_locked(&inner, wallet, SignerScope::PlatformIdentity, key)
    }

    fn key_held(
        signer: &VaultSigner,
        path: &DerivationPath,
        key_data: &[u8],
    ) -> Result<bool, VaultError> {
        signer
            .identity_key_matches(path, key_data)
            .map_err(|e| match e {
                SignerError::Locked => VaultError::Locked,
                SignerError::NoSecret => VaultError::NoSecret,
                SignerError::Vault(e) => e,
                e => VaultError::InvalidArgument(e.to_string()),
            })
    }

    /// The vault's epoch: it changes on every lock, every unlock that
    /// changes the lock state or scope, and every passphrase change,
    /// encrypt, recover and destroy, and each change ends every grant,
    /// token and signer issued before it. Not secret; the
    /// engine compares it around vault calls to notice an epoch change
    /// whichever call made it (E0-04 design §3.5, §8.6).
    pub fn epoch(&self) -> u64 {
        self.inner().epoch
    }

    /// Whether `token` was redeemed by this vault instance in its current
    /// epoch. Both are compared in constant time, and every use of a token
    /// (its key, or a signer it issues) checks this first, under `inner`
    /// (review DW-E0-03 r2 M1). The instance id refuses a token of another
    /// vault (even one with the same wallet id and epoch number) and of an
    /// earlier opening of this vault's file; the epoch refuses a token
    /// redeemed before a lock, unlock, scope or passphrase change of this one.
    /// A token from the signing window is refused once that window has
    /// ended (its deadline passed, or it was closed).
    fn token_binding(&self, inner: &Inner, token: &GrantToken) -> TokenBinding {
        let same_vault = self.shared.instance.ct_eq(&token.vault_instance);
        let same_epoch = inner.epoch.ct_eq(&token.epoch);
        let window_ended = token.window.is_some_and(|window| {
            window != inner.window_generation
                || inner
                    .signing_deadline
                    .is_none_or(|deadline| self.clock_mono() >= deadline)
        });
        if !bool::from(same_vault) {
            TokenBinding::OtherVault
        } else if !bool::from(same_epoch) {
            TokenBinding::EndedEpoch
        } else if window_ended {
            TokenBinding::EndedWindow
        } else {
            TokenBinding::Current
        }
    }

    /// `token`'s binding as the error its use is refused with:
    /// `GrantInvalid` for a token of another vault or of an ended signing
    /// window, `Locked` for one whose epoch has ended (a lock, unlock, scope
    /// change or passphrase change since redemption).
    fn token_current(&self, inner: &Inner, token: &GrantToken) -> Result<(), VaultError> {
        match self.token_binding(inner, token) {
            TokenBinding::Current => Ok(()),
            TokenBinding::OtherVault | TokenBinding::EndedWindow => Err(VaultError::GrantInvalid),
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
        let key = self.token_key(token)?;
        #[cfg(test)]
        crate::signer::test_hook::opened();
        Ok(key)
    }

    /// [`Self::key_for`]'s check and key read, one step under `inner`: for
    /// it, and for a caller that holds the gate exclusively instead.
    fn token_key(&self, token: &GrantToken) -> Result<Key32, VaultError> {
        let inner = self.inner();
        self.token_current(&inner, token)?;
        match token.key.copy()? {
            Some(key) => Ok(key),
            None => Self::memory_dek(&inner),
        }
    }

    /// Waits for every gated operation still running, then keeps new ones
    /// out until the guard drops, without ending the epoch: a caller that
    /// deletes a wallet's seed under it knows that no signer copied that seed
    /// before the deletion and still signs after it (DP2-02, Sol r3). An
    /// operation that read the seed has released its result by then; one
    /// that starts after finds no seed (`NoSecret`). Taken after `writer`,
    /// as the epoch changes that write the file take it.
    fn drain_ops(&self, _writer: &WriteGuard<'_>) -> EpochGuard<'_> {
        #[cfg(test)]
        self.inner().record(GateEvent::DrainWaiting);
        self.epoch_guard()
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
    /// Runs under the caller's `writer` guard, so no other change of the
    /// file interleaves.
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

        #[cfg(all(debug_assertions, any(test, feature = "fault-inject")))]
        if crate::fault::take(&self.shared.dir, crate::fault::Fault::ReadBack) {
            return Err(VaultError::Corrupt("injected read-back failure".into()));
        }
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
    ///
    /// `wallet` must be the id the engine derives from this seed: the vault
    /// does not bind ids to seeds. It never replaces a seed: a wallet that
    /// already holds different records is refused with
    /// [`VaultError::WalletSecretExists`] and nothing is written, since a
    /// signer that has read the old seed is not drained here; replacing goes
    /// through [`Self::wipe_wallet_secret`] or [`Self::delete_wallet_secret`]
    /// first, which drain. Storing the same records again seals them anew
    /// (no signer can tell). The check and the write hold `writer` together.
    pub fn store_wallet_secret(
        &self,
        wallet: &WalletId,
        secret: &WalletSecret,
    ) -> Result<(), VaultError> {
        let dek = self.full_dek()?;
        let seed = encode_seed(secret);
        // A raw seed has no phrase: its wallet stores the seed record only.
        let phrase = secret.derivation != SeedDerivation::RawSeed;
        let records = [
            (
                record_id(wallet, REC_MNEMONIC),
                phrase.then_some(&secret.mnemonic[..]),
            ),
            (
                record_id(wallet, REC_PASSPHRASE),
                phrase.then_some(&secret.mnemonic_passphrase[..]),
            ),
            (record_id(wallet, REC_SEED), Some(&seed[..])),
        ];
        let writer = self.writer();
        if self.has_any_record(wallet)? && !self.holds_records(&dek, &records)? {
            return Err(VaultError::WalletSecretExists);
        }
        self.commit_records_under(&writer, &dek, &records)
    }

    /// Whether the vault holds exactly `records` (`None`: absent), compared
    /// in constant time.
    fn holds_records(
        &self,
        dek: &Key32,
        records: &[(String, Option<&[u8]>)],
    ) -> Result<bool, VaultError> {
        let mut same = subtle::Choice::from(1);
        for (id, want) in records {
            same &= match (self.read_record(dek, id)?, want) {
                (Some(held), Some(want)) => held[..].ct_eq(want),
                (None, None) => subtle::Choice::from(1),
                _ => subtle::Choice::from(0),
            };
        }
        Ok(same.into())
    }

    /// Deletes every record of `wallet` with the vault's full-scope key.
    /// `Ok(false)` when it had none. Used to roll back a failed import; user
    /// removal goes through [`Self::wipe_wallet_secret`].
    pub fn delete_wallet_secret(&self, wallet: &WalletId) -> Result<bool, VaultError> {
        if !self.has_any_record(wallet)? {
            return Ok(false);
        }
        let dek = self.full_dek()?;
        let writer = self.writer();
        let _drained = self.drain_ops(&writer);
        self.delete_records(&writer, wallet, &dek)?;
        Ok(true)
    }

    /// Deletes every record of `wallet` under a redeemed `Wipe` grant for that
    /// wallet, with the grant's key when it carries one (a passphrase grant on
    /// a locked vault). `Ok(false)` when it had none. The deletion holds the
    /// gate exclusively ([`Self::drain_ops`]): it waits for every operation
    /// still running, so a signer of the wallet that read its seed releases
    /// its result before the seed goes, and none starts until it is gone. A
    /// concurrent `lock()` either came first, and the deletion is refused
    /// (`Locked`), or waits until it is done.
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
        let _drained = self.drain_ops(&writer);
        let dek = self.token_key(token)?;
        #[cfg(test)]
        crate::signer::test_hook::opened();
        self.delete_records(&writer, wallet, &dek)?;
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
    /// own key to the signer, which keeps it alive: the M1 flows (send,
    /// PSBT, message, CoinJoin) drop their token as soon as they hold the
    /// signer, so a weak signer would die at once. That is the one exception
    /// to E0-04 design §3.5, until P5 moves those flows onto holds; a lease
    /// uses [`Self::signer_held`]. The signer stops working when the vault
    /// locks or changes unlock scope. A `PlatformOp` grant gets scoped
    /// signers ([`Self::platform_signer`]) and an `IdentityScan` grant the
    /// identity-scan key ([`Self::scan_key`]), not this signer. A held token
    /// is `InvalidArgument` here.
    pub fn signer(&self, wallet: &WalletId, token: &GrantToken) -> Result<VaultSigner, VaultError> {
        self.spend_signer(wallet, None, token)
    }

    /// [`Self::signer`] for a token whose key `hold` holds: the signer stops
    /// when the hold drops. A token `hold` does not hold is
    /// `InvalidArgument`.
    pub fn signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        self.spend_signer(wallet, Some(hold), token)
    }

    fn spend_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        if !token.purpose.signs() {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.token_signer(wallet, token, SignerScope::Full, hold)
    }

    /// A signer limited to one Platform scope (DASHPAY §3.3):
    /// [`SignerScope::PlatformIdentity`], [`SignerScope::DashPayCrypto`],
    /// [`SignerScope::PlatformFunding`] or [`SignerScope::PlatformAddress`],
    /// authorized by a redeemed
    /// `PlatformOp` grant for `wallet` whose key is the vault's (the vault
    /// held its full-scope key when the grant was authorized). One token
    /// may issue several (a registration needs identity and funding
    /// signers). Any other scope is `InvalidArgument`; any other grant
    /// `GrantPurposeMismatch`. A token with its own key issues signers only
    /// through its hold ([`Self::platform_signer_held`]); here it is
    /// `InvalidArgument`. The signer stops working when the vault locks or
    /// changes unlock scope.
    ///
    /// The token's caps bound the scope (E0-04 design §3.4), else
    /// `GrantPurposeMismatch`: `PlatformFunding{max_duffs}` needs a token
    /// whose `max_duffs` is not 0 and at least the scope's (the engine asks
    /// for the funding budget left), and `PlatformIdentity` and
    /// `PlatformAddress` a token whose `max_credits` is not 0. `DashPayCrypto` comes with any `PlatformOp`.
    pub fn platform_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        self.scoped_signer(wallet, None, token, scope)
    }

    /// [`Self::platform_signer`] for a token whose key `hold` holds: dropping
    /// `hold` erases the key, and the signer then fails `Locked` (an
    /// operation already under way finishes). A token `hold` does not hold
    /// is `InvalidArgument`.
    pub fn platform_signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        self.scoped_signer(wallet, Some(hold), token, scope)
    }

    /// The [`SignerScope::ShieldDrainFunding`] signer of `wallet` ("Move
    /// mixed coins" to Shielded), authorized by a redeemed `Spend` grant
    /// for that wallet whose key is the vault's: that grant's own signer is
    /// [`SignerScope::Full`], so this one reaches nothing it does not. Any
    /// other grant is `GrantPurposeMismatch`, and so is a `max_duffs` of 0
    /// or above the grant's. A token with its own key issues one only
    /// through its hold ([`Self::shield_drain_signer_held`]): here it is
    /// `InvalidArgument`, though its own `Spend` signer issues directly.
    ///
    /// The signer stops when `life` drops, whatever key it signs with, as
    /// well as when the vault locks or changes unlock scope.
    pub fn shield_drain_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
        max_duffs: u64,
        life: &SignerLife,
    ) -> Result<VaultSigner, VaultError> {
        self.drain_signer(wallet, None, token, max_duffs, life)
    }

    /// [`Self::shield_drain_signer`] for a token whose key `hold` holds:
    /// the signer stops when the first of `hold` and `life` drops.
    pub fn shield_drain_signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
        max_duffs: u64,
        life: &SignerLife,
    ) -> Result<VaultSigner, VaultError> {
        self.drain_signer(wallet, Some(hold), token, max_duffs, life)
    }

    fn drain_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
        max_duffs: u64,
        life: &SignerLife,
    ) -> Result<VaultSigner, VaultError> {
        let GrantPurpose::Spend { max_duffs: cap } = token.purpose else {
            return Err(VaultError::GrantPurposeMismatch);
        };
        if max_duffs == 0 || max_duffs > cap {
            return Err(VaultError::GrantPurposeMismatch);
        }
        if hold.is_none() && matches!(token.key, KeySource::Own(_)) {
            return Err(VaultError::InvalidArgument(
                "an own-key token issues the drain signer through its hold".into(),
            ));
        }
        let scope = SignerScope::ShieldDrainFunding { max_duffs };
        Ok(self
            .token_signer(wallet, token, scope, hold)?
            .with_life(life.watch()))
    }

    /// The vote signer of `wallet` (MG-05 §3, scope
    /// [`SignerScope::MasternodeVoting`]), authorized by a redeemed
    /// `Governance` grant for that wallet whose key is the vault's; any other
    /// grant is `GrantPurposeMismatch`. A token with its own key issues one
    /// only through its hold ([`Self::vote_signer_held`]). It signs funding
    /// votes and nothing else, and stops when the vault locks or changes
    /// unlock scope.
    pub fn vote_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<VoteSigner, VaultError> {
        self.governance_signer(wallet, None, token)
    }

    /// [`Self::vote_signer`] for a token whose key `hold` holds: the signer
    /// stops when the hold drops.
    pub fn vote_signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
    ) -> Result<VoteSigner, VaultError> {
        self.governance_signer(wallet, Some(hold), token)
    }

    fn governance_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
    ) -> Result<VoteSigner, VaultError> {
        if token.purpose.kind() != GrantKind::Governance {
            return Err(VaultError::GrantPurposeMismatch);
        }
        let signer = self.token_signer(wallet, token, SignerScope::MasternodeVoting, hold)?;
        Ok(VoteSigner::new(signer))
    }

    /// A [`SignerScope::MasternodeOwner`] signer of `wallet` (MG-05 §1),
    /// authorized by a redeemed `MasternodeOp` grant for that wallet whose
    /// key is the vault's; any other grant is `GrantPurposeMismatch`. A
    /// token with its own key issues one only through its hold
    /// ([`Self::masternode_signer_held`]). The vault cannot see the debit of
    /// what it signs: the engine charges it against the grant's cap first
    /// (MG-05 §2).
    pub fn masternode_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        self.owner_signer(wallet, None, token)
    }

    /// [`Self::masternode_signer`] for a token whose key `hold` holds: the
    /// signer stops when the hold drops.
    pub fn masternode_signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        self.owner_signer(wallet, Some(hold), token)
    }

    fn owner_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        if token.purpose.kind() != GrantKind::MasternodeOp {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.token_signer(wallet, token, SignerScope::MasternodeOwner, hold)
    }

    /// The single-use [`SignerScope::MasternodeWithdrawal`] signer of
    /// `wallet` (MG-13 P3, P12), authorized by a redeemed
    /// `MasternodeWithdraw` grant for that wallet whose key is the vault's;
    /// any other grant is `GrantPurposeMismatch`. The scope carries the
    /// grant's `pro_tx_hash`, `max_credits` and `destination`. A token issues one such
    /// signer, once: a token that already issued a signer is `GrantInvalid`,
    /// and so is every later call with it, even when this one then fails
    /// (it is spent first). A token with its own key issues one only
    /// through its hold ([`Self::masternode_withdrawal_signer_held`]). The
    /// signer makes one withdrawal signature
    /// ([`VaultSigner::sign_masternode_withdrawal`]) and nothing else, and
    /// stops when the vault locks or changes unlock scope.
    pub fn masternode_withdrawal_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        self.withdrawal_signer(wallet, None, token)
    }

    /// [`Self::masternode_withdrawal_signer`] for a token whose key `hold`
    /// holds: the signer stops when the hold drops.
    pub fn masternode_withdrawal_signer_held(
        &self,
        wallet: &WalletId,
        hold: &KeyHold,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        self.withdrawal_signer(wallet, Some(hold), token)
    }

    fn withdrawal_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
    ) -> Result<VaultSigner, VaultError> {
        let GrantPurpose::MasternodeWithdraw {
            pro_tx_hash,
            max_credits,
            destination,
        } = token.purpose
        else {
            return Err(VaultError::GrantPurposeMismatch);
        };
        // One signer per grant (r2 m5): a second one would sign again.
        if token
            .issued
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(VaultError::GrantInvalid);
        }
        let scope = SignerScope::MasternodeWithdrawal {
            pro_tx_hash,
            max_credits,
            destination,
        };
        self.token_signer(wallet, token, scope, hold)
    }

    fn scoped_signer(
        &self,
        wallet: &WalletId,
        hold: Option<&KeyHold>,
        token: &GrantToken,
        scope: SignerScope,
    ) -> Result<VaultSigner, VaultError> {
        if !matches!(
            scope,
            SignerScope::PlatformIdentity
                | SignerScope::DashPayCrypto
                | SignerScope::PlatformFunding { .. }
                | SignerScope::PlatformAddress
        ) {
            return Err(VaultError::InvalidArgument(format!(
                "{scope:?} is not a Platform signer scope"
            )));
        }
        let GrantPurpose::PlatformOp {
            max_duffs,
            max_credits,
        } = token.purpose
        else {
            return Err(VaultError::GrantPurposeMismatch);
        };
        let granted = match scope {
            SignerScope::PlatformFunding { max_duffs: wanted } => {
                max_duffs != 0 && wanted <= max_duffs
            }
            SignerScope::PlatformIdentity | SignerScope::PlatformAddress => max_credits != 0,
            _ => true,
        };
        if !granted {
            return Err(VaultError::GrantPurposeMismatch);
        }
        self.token_signer(wallet, token, scope, hold)
    }

    /// Moves the own data key of a grant set into one [`KeyHold`] (E0-04
    /// design §3.5): every token's copy is erased and the tokens then issue
    /// signers only through the hold, while it lives. Every own key of a
    /// grant set is this vault's data key, so one hold serves the whole set
    /// ("Accept and pay"'s two grants).
    ///
    /// The hold covers every token or the call fails, changing nothing.
    /// Every token, vault-key ones included, must have been redeemed by this
    /// vault in its current epoch (`GrantInvalid`, `Locked`). Each must then
    /// carry its own key (authorized while the vault held no full-scope
    /// key), not be held already and never have issued a signer, even one
    /// since dropped: a direct signer's copy is one no hold could erase
    /// (`InvalidArgument`). `Ok(None)` for a valid set without own keys (the
    /// vault's key serves them, and needs no hold); a set mixing both kinds
    /// is `InvalidArgument`.
    pub fn hold_key(&self, tokens: &mut [GrantToken]) -> Result<Option<KeyHold>, VaultError> {
        // Every token's binding first, vault-key ones included (review
        // DW-E0-04-P1 r2).
        {
            let inner = self.inner();
            for token in tokens.iter() {
                self.token_current(&inner, token)?;
            }
        }
        let own = tokens
            .iter()
            .filter(|t| !matches!(t.key, KeySource::Vault))
            .count();
        if own == 0 {
            return Ok(None);
        }
        if own != tokens.len() {
            return Err(VaultError::InvalidArgument(
                "a hold covers a set whose tokens all carry their own key".into(),
            ));
        }
        let key = {
            for token in tokens.iter() {
                match &token.key {
                    KeySource::Own(_) if token.issued.load(Ordering::Relaxed) => {
                        return Err(VaultError::InvalidArgument(
                            "a token that issued a signer cannot be held".into(),
                        ));
                    }
                    KeySource::Own(_) => {}
                    _ => {
                        return Err(VaultError::InvalidArgument(
                            "the token is already held".into(),
                        ));
                    }
                }
            }
            // One key serves the set: every token's must be the same (it
            // is, for one vault and epoch; checked, not assumed).
            let first = tokens[0].key.copy()?;
            for token in &tokens[1..] {
                let same = match (&first, token.key.copy()?) {
                    (Some(a), Some(b)) => bool::from(a[..].ct_eq(&b[..])),
                    _ => false,
                };
                if !same {
                    return Err(VaultError::InvalidArgument(
                        "the tokens of a hold carry different keys".into(),
                    ));
                }
            }
            first
        };
        let hold = KeyHold::new(key.ok_or_else(|| VaultError::Internal("no own key".into()))?);
        for token in tokens.iter_mut() {
            token.key = KeySource::Held(hold.key.clone());
        }
        Ok(Some(hold))
    }

    /// A signer with `scope` under a redeemed grant for `wallet`, through
    /// `hold` when one is given; the caller has checked the grant's purpose.
    /// Which tokens may issue it (review DW-E0-04-P1 r1 ruling): a held
    /// token only through its own hold; an own-key token directly only for
    /// a purpose that signs (`Spend`, `SignMessage`: the M1 exception of
    /// [`Self::signer`]); a vault-key token directly. Anything else is
    /// `InvalidArgument`.
    pub(crate) fn token_signer(
        &self,
        wallet: &WalletId,
        token: &GrantToken,
        scope: SignerScope,
        hold: Option<&KeyHold>,
    ) -> Result<VaultSigner, VaultError> {
        if token.wallet.as_ref() != Some(wallet) {
            return Err(VaultError::GrantPurposeMismatch);
        }
        // A token of another vault is refused before this one loads its
        // key; the full check is made under the lock that issues the signer.
        if self.token_binding(&self.inner(), token) == TokenBinding::OtherVault {
            return Err(VaultError::GrantInvalid);
        }
        let refused = |why: &str| Err(VaultError::InvalidArgument(why.into()));
        match (&token.key, hold) {
            (_, Some(hold)) if !hold.holds(token) => {
                return refused("the token's key is not in this hold");
            }
            (KeySource::Held(_), None) => {
                return refused("a held token issues signers through its hold");
            }
            (KeySource::Own(_), None) if !token.purpose.signs() => {
                return refused("an own-key token of this purpose issues signers through a hold");
            }
            _ => {}
        }
        token.key.check()?;
        if matches!(token.key, KeySource::Vault) {
            self.full_dek()?;
        }
        let mut inner = self.inner();
        settle_window(&mut inner, self.clock());
        if self.token_binding(&inner, token) != TokenBinding::Current {
            return Err(VaultError::GrantInvalid);
        }
        if matches!(token.key, KeySource::Vault) && inner.dek.is_none() {
            return Err(VaultError::Locked);
        }
        let signer = self.signer_locked(&inner, wallet, scope, token.key.clone())?;
        // `hold_key` takes the tokens `&mut`, so it never runs beside this.
        token.issued.store(true, Ordering::Relaxed);
        Ok(signer)
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
    pub(crate) fn prompt_free_signer(
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
                self.signer_locked(&inner, wallet, scope, KeySource::Vault)
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
        self.signer_locked(&inner, wallet, scope, KeySource::Vault)
    }

    fn signer_locked(
        &self,
        inner: &Inner,
        wallet: &WalletId,
        scope: SignerScope,
        key: KeySource,
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
            key,
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
        key: &KeySource,
    ) -> Result<Zeroizing<[u8; 64]>, SignerError> {
        if op.epoch != epoch {
            return Err(SignerError::Locked);
        }
        // A held key is copied here, inside the gate, under its hold's
        // mutex: a use that begins after the hold dropped fails `Locked`,
        // and one already past this point finishes with its own copy.
        let dek = match key.copy()? {
            Some(key) => key,
            None => Zeroizing::new(**self.inner().dek.as_ref().ok_or(SignerError::Locked)?),
        };
        #[cfg(test)]
        crate::signer::test_hook::key_taken();
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
            let floor = self.inner().policy_floor;
            let policy = open_policy(&next, &dek, floor)?.unwrap_or_default();
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
            let high_water = self.inner().high_water;
            let generation = put_policy(
                &mut next,
                &dek,
                high_water,
                floor,
                PolicyPayload {
                    last_passphrase_at,
                    ..policy
                },
            )?;
            self.persist_at(&writer, next, generation, false)?;
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

    /// The post-unlock signing window is open and the user's setting
    /// allows it. Expiry clears it, so a later clock change cannot open it
    /// again. A fresh open is closed.
    pub fn signing_window_open(&self) -> bool {
        let mut inner = self.inner();
        settle_window(&mut inner, self.clock());
        window_admits(&inner)
    }

    /// Duff still authorizable from the open window, against the sealed
    /// limit. `None` when the window is closed or the sealed policy does
    /// not authenticate.
    pub fn signing_window_remaining(&self) -> Option<u64> {
        let mut inner = self.inner();
        settle_window(&mut inner, self.clock());
        if !window_admits(&inner) {
            return None;
        }
        let limit = sealed_spend_limit(&inner).ok()?;
        Some(limit.saturating_sub(inner.window_charged))
    }

    /// Ends the signing window without locking. Sleep calls this; lock
    /// clears it too. Grants the window issued are dropped.
    pub fn end_signing_window(&self) {
        clear_signing_window(&mut self.inner());
    }

    /// The host's sleep watcher is running and saw no sleep in its last
    /// tick. Once it has called this, a window whose last heartbeat is
    /// older than three seconds ends at the next look, so a suspend that
    /// the monotonic clock counted (as on Windows) ends the window even
    /// before the watcher's next tick notices it.
    pub fn signing_window_heartbeat(&self) {
        let clock = self.clock();
        let now = clock.monotonic();
        let wall = clock.wall();
        let mut inner = self.inner();
        settle_window_at(&mut inner, wall, now);
        inner.heartbeat = Some(now);
    }

    /// The host runs a sleep watcher that beats
    /// ([`Self::signing_window_heartbeat`]): from now on the window admits
    /// nothing until the first beat, and nothing once the last beat is older
    /// than three seconds. The desktop app calls this for every vault it
    /// opens, so a watcher that never started leaves the window closed. A
    /// host without a watcher (the CLI, tests) does not, and has only the
    /// clock check.
    pub fn require_signing_window_heartbeat(&self) {
        self.inner().heartbeat_required = true;
    }

    /// The user's "ask for my passphrase for every payment" for the vault
    /// `vault_id`, which the host stores: `allowed` is the signing window.
    /// The window admits grants only for the vault id allowed last, so a
    /// caller that never read the setting gets none, and a new vault (a
    /// recovery's, or one made after a destroy) admits none until the host
    /// allows it. Refusing it ends the open window and drops its grants.
    pub fn set_signing_window_allowed(&self, vault_id: &[u8], allowed: bool) {
        let mut inner = self.inner();
        if allowed {
            inner.window_vault = Some(vault_id.to_vec());
        } else if inner
            .window_vault
            .as_ref()
            .is_some_and(|id| bool::from(id.ct_eq(vault_id)))
        {
            inner.window_vault = None;
            clear_signing_window(&mut inner);
        }
    }

    /// Whether the host allowed the signing window for the vault in memory.
    pub fn signing_window_allowed(&self) -> bool {
        window_allowed(&self.inner())
    }

    /// A lease may redeem `ids` together only when the ones the window
    /// issued do not add up past the sealed spend limit. A passphrase grant
    /// is not counted. An id the vault no longer holds, a window grant whose
    /// window has ended included, is `GrantInvalid`.
    pub fn window_lease_allowed(&self, ids: &[String]) -> Result<(), VaultError> {
        let mut inner = self.inner();
        settle_window(&mut inner, self.clock());
        let mut total = 0u64;
        let mut any = false;
        for id in ids {
            let issued = inner.grants.get(id).ok_or(VaultError::GrantInvalid)?;
            if !issued.from_window {
                continue;
            }
            any = true;
            let value = issued
                .grant
                .purpose
                .quick_unlock_value()
                .unwrap_or(u64::MAX);
            total = total.saturating_add(value);
        }
        if !any {
            return Ok(());
        }
        if total > sealed_spend_limit(&inner)? {
            return Err(VaultError::CredentialRequired);
        }
        Ok(())
    }

    /// Whether `purposes` could still be issued with [`Credential::None`].
    /// The authority is [`Self::authorize_set`], which checks this under the
    /// same guard as the insert. The limit is cumulative for the window and
    /// is the sealed policy.
    pub fn signing_window_covers(&self, purposes: &[GrantPurpose]) -> bool {
        purposes.iter().all(window_purpose) && {
            let mut inner = self.inner();
            self.admit_window(&mut inner, purposes).is_ok()
        }
    }

    /// Moves the sleep sample's wall clock back, so the next look sees a
    /// suspend. Tests only.
    #[cfg(test)]
    pub(crate) fn test_backdate_sleep_sample(&self, ago: Duration) {
        let mut inner = self.inner();
        inner.sleep_sample = Some(SleepSample {
            wall: self
                .clock()
                .wall()
                .checked_sub(ago)
                .unwrap_or(SystemTime::UNIX_EPOCH),
            mono: self.clock_mono(),
        });
    }

    /// The quick-unlock rules (IOS-016). While locked the limit is the plain
    /// copy; with the data key in memory it is the sealed one, which is what
    /// `authorize` enforces.
    pub fn quick_unlock_policy(&self) -> QuickUnlockPolicy {
        let inner = self.inner();
        let file = inner.file.as_ref();
        let settings = file.and_then(|f| f.quick_unlock.as_ref());
        let shown = settings.map_or(DEFAULT_QUICK_UNLOCK_SPEND_LIMIT, |s| s.spend_limit_duffs);
        // A sealed limit that no longer authenticates (the manifest does) is
        // reported as such and as 0, the limit in force, not as its plain copy.
        let (spend_limit_duffs, limit_unreadable) = match (file, inner.dek.as_ref()) {
            (Some(f), Some(dek)) => match open_policy(f, dek, inner.policy_floor) {
                Ok(Some(policy)) => (policy.spend_limit_duffs, false),
                Err(VaultError::Corrupt(_)) if verify_manifest(f, dek, 0).is_ok() => (0, true),
                _ => (shown, false),
            },
            _ => (shown, false),
        };
        QuickUnlockPolicy {
            enrolled: file.is_some_and(|f| f.slot_b.is_some()),
            spend_limit_duffs,
            limit_unreadable,
            passphrase_max_age_secs: PASSPHRASE_MAX_AGE_SECS,
            last_passphrase_at: settings
                .and_then(|s| s.last_passphrase_at)
                .max(inner.last_passphrase_at),
        }
    }

    /// Sets the quick-unlock spending limit to one of
    /// [`QUICK_UNLOCK_SPEND_LIMITS`] (`InvalidArgument` otherwise). Needs an
    /// encrypted vault (`NotEncrypted`) and a `ChangeCredential` grant. A
    /// lower limit ends the signing window, with its grants and the tokens
    /// redeemed from it, in the step that publishes the new limit; a higher
    /// or equal one ends nothing. A failed commitment or write keeps the
    /// limit in memory and still ends the window.
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
            let floor = self.inner().policy_floor;
            // A sealed limit that no longer authenticates counts as none (payments
            // ask), so it is replaced rather than refused, the grant having checked
            // the passphrase (#173 item 10). A manifest that does not
            // authenticate is still `Corrupt`.
            let policy = match open_policy(&next, &dek, floor) {
                Ok(policy) => policy.unwrap_or_default(),
                Err(VaultError::Corrupt(_)) if verify_manifest(&next, &dek, 0).is_ok() => {
                    PolicyPayload::withdrawn()
                }
                Err(e) => return Err(e),
            };
            let lowered = spend_limit_duffs < policy.spend_limit_duffs;
            let high_water = self.inner().high_water;
            let generation = put_policy(
                &mut next,
                &dek,
                high_water,
                floor,
                PolicyPayload {
                    spend_limit_duffs,
                    last_passphrase_at: policy
                        .last_passphrase_at
                        .max(self.inner().last_passphrase_at),
                    generation: None,
                },
            )?;
            self.persist_at(&writer, next, generation, lowered)
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
        let spend_limit_duffs = self.carried_spend_limit(&old);

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
            policy: Some(1),
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
                    spend_limit_duffs,
                    last_passphrase_at: Some(now),
                    generation: manifest.policy,
                },
            )?),
        };

        // The `lost` wallets' seeds go with the old vault: drained first
        // (DP2-02, Sol r3), as `wipe_wallet_secret`.
        let ops = self.drain_ops(&writer);
        file::keep_replaced_copy(&self.shared.dir, now)?;
        file::write(&self.shared.dir, &next)?;
        // From here the old vault is replaced on disk; memory follows even
        // if the read-back check below fails, so nothing keeps using the
        // old key.
        {
            let mut inner = self.inner();
            inner.file = Some(next);
            inner.high_water = manifest.generation;
            inner.forget_key(&ops);
            inner.install_key(Zeroizing::new(*dek), UnlockScope::Full, &ops);
            inner.backup_kek = Some(backup_kek);
            inner.last_passphrase_at = Some(now);
            arm_signing_window(&mut inner, self.clock());
            inner.policy_floor = None;
        }
        drop(ops);
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
        // A new data key: links sealed to the old one are dropped (D17).
        // What is left unsealed shows in `status()` (D26).
        self.sync_links_after_key(&dek, &vault_id);
        drop(writer);
        Ok(lost)
    }

    /// The spending limit a recovery keeps from the vault it replaces. With
    /// the data key in memory it is the sealed limit (the default for a
    /// vault that never had one, 0 if it does not authenticate). Without
    /// the key nothing authenticates the old limit, so it is 0 until the
    /// user sets one again.
    fn carried_spend_limit(&self, old: &VaultFile) -> u64 {
        let inner = self.inner();
        let Some(dek) = &inner.dek else {
            return 0;
        };
        match open_policy(old, dek, inner.policy_floor) {
            Ok(Some(policy)) => policy.spend_limit_duffs,
            Ok(None) => DEFAULT_QUICK_UNLOCK_SPEND_LIMIT,
            Err(_) => 0,
        }
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
            // Links kept before any vault go too ("Delete All").
            let mut links = self.links_guard();
            self.remove_links(&mut links)?;
            drop(links);
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
        // The links first: a failure leaves the vault, not sealed links of
        // a vault that is gone (review r1 #4).
        let mut links = self.links_guard();
        self.remove_links(&mut links)?;
        file::remove_all(&self.shared.dir)?;
        {
            let ops = self.epoch_guard();
            let mut inner = self.inner();
            inner.file = None;
            inner.high_water = 0;
            inner.last_passphrase_at = None;
            inner.policy_floor = None;
            inner.forget_key(&ops);
        }
        let store = self.inner().commitments.clone();
        if let Some(store) = store
            && let Err(e) = store.forget(&current.vault_id)
        {
            tracing::warn!(error = %e, "could not drop the policy commitment");
        }
        drop(links);
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

    #[test]
    fn a_wall_clock_gap_ends_the_signing_window() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(
            dir.path().join("vault"),
            Network::Regtest,
            "regtest",
            test_config(step_clock()),
        )
        .unwrap();
        v.create(Some(b"correct horse")).unwrap();
        v.set_signing_window_allowed(&v.vault_id().unwrap(), true);
        assert!(v.signing_window_open());
        v.test_backdate_sleep_sample(Duration::from_secs(30));
        assert!(!v.signing_window_open());
        assert_eq!(v.status().state, LockState::Unlocked);
    }

    /// A passphrase check that re-seals the policy publishes the file and
    /// its high-water mark together: a quick-unlock authorization made
    /// while the file is being written checks a consistent pair.
    #[test]
    fn a_quick_unlock_during_a_passphrase_check_sees_a_consistent_file() {
        let dir = tempfile::tempdir().unwrap();
        let pw: &[u8] = b"correct horse";
        let v = vault_with_wallet(dir.path(), Some(pw), 3);
        let change = v
            .authorize(
                GrantPurpose::ChangeCredential,
                None,
                Credential::Passphrase(pw),
            )
            .unwrap();
        let key = v.enroll_quick_unlock(&change.id).unwrap();
        v.lock();
        let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
        let out = std::rc::Rc::clone(&seen);
        PERSIST_HOOK.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |v: &Vault| {
                let answer = v.authorize(
                    GrantPurpose::Spend { max_duffs: 1 },
                    Some(&[1; 32]),
                    Credential::QuickUnlock(&key),
                );
                *out.borrow_mut() = Some(answer.map(drop));
            }))
        });
        v.unlock(pw, UnlockScope::Full).unwrap();
        assert_eq!(*seen.borrow(), Some(Ok(())));
    }

    /// A lower limit whose write fails (here after the new file reached the
    /// disk) keeps the limit in memory and still ends the window.
    #[test]
    fn a_failed_write_of_a_lower_limit_still_ends_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let pw: &[u8] = b"correct horse";
        let v = vault_with_wallet(dir.path(), Some(pw), 3);
        let spend = GrantPurpose::Spend { max_duffs: 1 };
        let grant = v
            .authorize(spend, Some(&[1; 32]), Credential::None)
            .unwrap();
        let change = v
            .authorize(
                GrantPurpose::ChangeCredential,
                None,
                Credential::Passphrase(pw),
            )
            .unwrap();
        crate::fault::arm(&dir.path().join("vault"), crate::fault::Fault::DirSync);
        assert!(v.set_quick_unlock_spend_limit(&change.id, 0).is_err());
        assert_eq!(
            v.quick_unlock_policy().spend_limit_duffs,
            DEFAULT_QUICK_UNLOCK_SPEND_LIMIT
        );
        assert!(!v.signing_window_open());
        assert_eq!(
            v.redeem_grant(&grant.id, GrantKind::Spend, Some(&[1; 32]))
                .map(drop),
            Err(VaultError::GrantInvalid)
        );
    }

    #[derive(Default)]
    struct Rows(Mutex<std::collections::HashMap<Vec<u8>, u64>>);

    impl PolicyCommitments for Rows {
        fn committed(&self, vault_id: &[u8]) -> Result<Option<u64>, String> {
            Ok(self.0.lock().unwrap().get(vault_id).copied())
        }

        fn commit(&self, vault_id: &[u8], generation: u64) -> Result<u64, String> {
            let mut rows = self.0.lock().unwrap();
            let row = rows.entry(vault_id.to_vec()).or_default();
            *row = (*row).max(generation);
            Ok(*row)
        }

        fn forget(&self, vault_id: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().remove(vault_id);
            Ok(())
        }
    }

    fn set_limit(v: &Vault, pw: &[u8], limit: u64) -> Result<(), VaultError> {
        let change = v
            .authorize(
                GrantPurpose::ChangeCredential,
                None,
                Credential::Passphrase(pw),
            )
            .unwrap();
        v.set_quick_unlock_spend_limit(&change.id, limit).map(drop)
    }

    /// A higher limit whose write failed after the file reached the disk
    /// was committed; the lowering after it is sealed above that, so the
    /// higher file put back counts as 0.
    #[test]
    fn a_higher_limit_whose_write_failed_cannot_be_put_back_after_a_lowering() {
        let dir = tempfile::tempdir().unwrap();
        let pw: &[u8] = b"correct horse";
        let os = Arc::new(crate::MemoryOsStore::new());
        let rows = Arc::new(Rows::default());
        let v = vault_with_wallet_in(dir.path(), &os, Some(pw), 3);
        v.attach_policy_commitments(rows.clone());
        crate::fault::arm(&dir.path().join("vault"), crate::fault::Fault::DirSync);
        assert!(set_limit(&v, pw, 500_000_000).is_err());
        let higher = std::fs::read(dir.path().join("vault").join("vault.dwv")).unwrap();
        set_limit(&v, pw, 0).unwrap();
        drop(v);
        std::fs::write(dir.path().join("vault").join("vault.dwv"), higher).unwrap();
        let v = open_in(dir.path(), &os);
        v.attach_policy_commitments(rows);
        v.unlock(pw, UnlockScope::Full).unwrap();
        assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 0);
    }

    /// With a store, a lower limit whose write fails after its commitment
    /// leaves limit 0 in memory, and the window ends.
    #[test]
    fn a_failed_write_of_a_lower_limit_with_a_store_leaves_limit_0() {
        let dir = tempfile::tempdir().unwrap();
        let pw: &[u8] = b"correct horse";
        let v = vault_with_wallet(dir.path(), Some(pw), 3);
        v.attach_policy_commitments(Arc::new(Rows::default()));
        crate::fault::arm(&dir.path().join("vault"), crate::fault::Fault::DirSync);
        assert!(set_limit(&v, pw, 10_000_000).is_err());
        assert_eq!(v.quick_unlock_policy().spend_limit_duffs, 0);
        assert!(!v.signing_window_open());
    }

    /// A clock whose monotonic reading jumps (with the wall clock) at a
    /// chosen call: a suspend between two reads.
    struct JumpClock {
        origin: Instant,
        secs: std::sync::atomic::AtomicU64,
        calls: std::sync::atomic::AtomicU64,
        jump_at: std::sync::atomic::AtomicU64,
    }

    impl crate::types::Clock for JumpClock {
        fn now_secs(&self) -> u64 {
            1_800_000_000 + self.secs.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn monotonic(&self) -> Instant {
            use std::sync::atomic::Ordering::SeqCst;
            if self.calls.fetch_add(1, SeqCst) + 1 == self.jump_at.load(SeqCst) {
                self.secs.fetch_add(60, SeqCst);
            }
            self.origin + Duration::from_secs(self.secs.load(SeqCst))
        }

        fn wall(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH + Duration::from_secs(self.now_secs())
        }
    }

    /// The heartbeat records the reading it checked the last beat against:
    /// a suspend right after that reading leaves the beat at the reading,
    /// and the next look finds it stale.
    #[test]
    fn a_heartbeat_publishes_the_instant_it_checked() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(JumpClock {
            origin: Instant::now(),
            secs: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            jump_at: AtomicU64::new(u64::MAX),
        });
        let v = Vault::open(
            dir.path().join("vault"),
            Network::Regtest,
            "regtest",
            VaultConfig {
                kdf: crate::KdfPolicy::Fixed(crate::KdfParams::TEST),
                os_store: Arc::new(crate::MemoryOsStore::new()),
                clock: clock.clone(),
                grant_ttl_secs: 120,
            },
        )
        .unwrap();
        v.create(Some(b"correct horse")).unwrap();
        v.set_signing_window_allowed(&v.vault_id().unwrap(), true);
        v.signing_window_heartbeat();
        assert!(v.signing_window_open());
        clock.jump_at.store(clock.calls.load(SeqCst) + 2, SeqCst);
        v.signing_window_heartbeat();
        assert!(!v.signing_window_open());
    }

    /// The decision behind the sleep backstop, on given clock readings: a
    /// wall clock that moved more than two seconds away from the monotonic
    /// clock, forward or back, is a disagreement; a long wait in both is not.
    #[test]
    fn the_clocks_disagree_either_way_past_the_gap() {
        let mono = Instant::now();
        let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let from = SleepSample { wall, mono };
        let secs = Duration::from_secs;
        let at = |wall_dt: i64, mono_dt: u64| {
            let wall = if wall_dt >= 0 {
                wall + secs(wall_dt as u64)
            } else {
                wall - secs(wall_dt.unsigned_abs())
            };
            clocks_disagree(&from, wall, mono + secs(mono_dt))
        };
        assert!(!at(0, 0));
        assert!(!at(600, 600));
        assert!(!at(2, 0));
        assert!(!at(-2, 0));
        assert!(!at(0, 2));
        assert!(at(3, 0));
        assert!(at(-3, 0));
        assert!(at(0, 3));
        assert!(at(600, 0));
        assert!(at(-3000, 0));
        assert!(at(-1, 2));
    }

    /// What each mix of an older and a newer build leaves in the file: a
    /// manifest without the policy generation keeps the old rule (any
    /// authentic policy), and a policy sealed without it under a manifest
    /// that names one counts as withdrawn.
    #[test]
    fn the_policy_generation_across_older_builds() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(
            dir.path().join("vault"),
            Network::Regtest,
            "regtest",
            test_config(step_clock()),
        )
        .unwrap();
        v.create(Some(b"correct horse")).unwrap();
        let dek = *v.inner().dek.clone().unwrap();
        let payload = |limit, generation| PolicyPayload {
            spend_limit_duffs: limit,
            last_passphrase_at: None,
            generation,
        };
        let with = |edit: &dyn Fn(&mut VaultFile)| {
            let mut inner = v.inner();
            edit(inner.file.as_mut().unwrap());
            sealed_spend_limit(&inner).unwrap()
        };
        let reseal_manifest = |f: &mut VaultFile, policy: Option<u64>| {
            let mut manifest = verify_manifest(f, &dek, 0).unwrap();
            manifest.policy = policy;
            f.manifest = seal_manifest(&dek, &f.vault_id, &f.network, &manifest).unwrap();
        };

        assert_eq!(with(&|_| {}), DEFAULT_QUICK_UNLOCK_SPEND_LIMIT);
        assert_eq!(
            with(&|f| {
                f.quick_unlock = Some(
                    seal_policy(&dek, &f.vault_id, &f.network, payload(10_000_000, None)).unwrap(),
                );
            }),
            10_000_000
        );
        assert_eq!(
            with(&|f| {
                put_policy(f, &dek, 0, None, payload(20_000_000, None)).unwrap();
            }),
            20_000_000
        );
        assert_eq!(
            with(&|f| {
                f.quick_unlock = Some(
                    seal_policy(&dek, &f.vault_id, &f.network, payload(30_000_000, None)).unwrap(),
                );
            }),
            0
        );
        assert_eq!(
            with(&|f| {
                let generation = verify_manifest(f, &dek, 0).unwrap().policy;
                f.quick_unlock = Some(
                    seal_policy(
                        &dek,
                        &f.vault_id,
                        &f.network,
                        payload(40_000_000, generation),
                    )
                    .unwrap(),
                );
                reseal_manifest(f, None);
            }),
            40_000_000
        );
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
        v.set_signing_window_allowed(&v.vault_id().unwrap(), true);
        let secret = WalletSecret {
            mnemonic: Zeroizing::new(b"unused".to_vec()),
            mnemonic_passphrase: Zeroizing::new(Vec::new()),
            seed: Zeroizing::new([seed; 64]),
            derivation: SeedDerivation::Bip39,
        };
        v.store_wallet_secret(&[1; 32], &secret).unwrap();
        v
    }

    const PLATFORM_OP: GrantPurpose = GrantPurpose::PlatformOp {
        max_duffs: 1,
        max_credits: 1,
    };

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
        let platform = || redeem(from, PLATFORM_OP, credential);
        let scan = redeem(from, GrantPurpose::IdentityScan, credential);
        refused(v.scan_key(&w, &scan).map(drop), "scan key");
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

    /// Release 0.1: a `PlatformAddress` signer comes from a `PlatformOp`
    /// token with credits only, and signs with a DIP-17 payment key once
    /// that key's HASH160 is the address; no other key, no other address.
    #[test]
    fn a_platform_address_signer_signs_its_own_address_only() {
        use dashcore::hashes::{Hash, hash160};
        use dashcore::sign_message::MessageSignature;
        use key_wallet::bip32::ExtendedPrivKey;

        let dir = tempfile::tempdir().unwrap();
        let v = vault_with_wallet(dir.path(), None, 0x5a);
        let w = [1u8; 32];
        let no_credits = GrantPurpose::PlatformOp {
            max_duffs: 1,
            max_credits: 0,
        };
        assert_eq!(
            v.platform_signer(
                &w,
                &redeem(&v, no_credits, Credential::None),
                SignerScope::PlatformAddress
            )
            .map(drop),
            Err(VaultError::GrantPurposeMismatch)
        );
        let s = v
            .platform_signer(
                &w,
                &redeem(&v, PLATFORM_OP, Credential::None),
                SignerScope::PlatformAddress,
            )
            .unwrap();
        let path = DerivationPath::from_str("m/9'/1'/17'/0'/0'/3").unwrap();
        let key = ExtendedPrivKey::new_master(Network::Regtest, &[0x5a; 64])
            .unwrap()
            .derive_priv(&path)
            .unwrap();
        let public = dashcore::secp256k1::PublicKey::from_secret_key(&key.private_key);
        let hash = *hash160::Hash::hash(&public.serialize()).as_byte_array();
        assert_eq!(s.platform_address_matches(&path, &hash), Ok(true));
        let sig = s
            .sign_platform_address(&path, &hash, b"transition")
            .unwrap();
        let recovered = MessageSignature::from_slice(&sig)
            .unwrap()
            .recover_pubkey(dashcore::hashes::sha256d::Hash::hash(b"transition"))
            .unwrap();
        assert_eq!(recovered.inner, public);

        let mut other = hash;
        other[0] ^= 1;
        assert_eq!(s.platform_address_matches(&path, &other), Ok(false));
        assert!(matches!(
            s.sign_platform_address(&path, &other, b"transition"),
            Err(SignerError::KeyMismatch(_))
        ));
        let identity = DerivationPath::from_str("m/9'/1'/5'/0'/0'/0'/0'").unwrap();
        assert!(matches!(
            s.sign_platform_address(&identity, &hash, b"transition"),
            Err(SignerError::PathNotAllowed(_))
        ));
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
        let token = redeem(&unencrypted, GrantPurpose::IdentityScan, Credential::None);
        unencrypted.scan_key(&[1; 32], &token).unwrap();
        let mut tokens = [redeem(
            &locked,
            GrantPurpose::IdentityScan,
            Credential::Passphrase(b"pw"),
        )];
        let hold = locked.hold_key(&mut tokens).unwrap().unwrap();
        locked.scan_key_held(&[1; 32], &hold, &tokens[0]).unwrap();
    }

    /// `authorize_set` issues nothing unless it issues every grant (E0-04
    /// design §3.3): a refused set leaves the vault's grant table as it was.
    #[test]
    fn a_refused_grant_set_issues_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let v = vault_with_wallet(dir.path(), Some(b"pw"), 0x5a);
        let w = [1u8; 32];
        let spend = GrantPurpose::Spend { max_duffs: 1 };
        let count = || v.inner().grants.len();
        v.authorize(spend, Some(&w), Credential::None).unwrap();
        assert_eq!(count(), 1);
        for (set, credential) in [
            (vec![spend, GrantPurpose::RevealSecret], Credential::None),
            (vec![PLATFORM_OP, GrantPurpose::Wipe], Credential::None),
            (
                vec![spend, GrantPurpose::IdentityScan],
                Credential::QuickUnlock(&[0; 32]),
            ),
            (vec![spend, PLATFORM_OP], Credential::Passphrase(b"wrong")),
        ] {
            assert!(
                v.authorize_set(&set, Some(&w), credential).is_err(),
                "{set:?}"
            );
            assert_eq!(count(), 1, "{set:?}");
        }
        v.lock();
        assert_eq!(
            v.authorize_set(&[spend, PLATFORM_OP], Some(&w), Credential::None),
            Err(VaultError::Locked)
        );
        assert_eq!(count(), 0);
        let grants = v
            .authorize_set(
                &[spend, PLATFORM_OP],
                Some(&w),
                Credential::Passphrase(b"pw"),
            )
            .unwrap();
        assert_eq!(grants.len(), 2);
        assert_eq!(count(), 2);
        // Each carries its own copy of the key.
        assert!(v.inner().grants.values().all(|g| g.key.is_some()));
    }

    /// `hold_key` refuses a set with a token another vault redeemed, and
    /// changes no token: each keeps its own key and stays usable in its
    /// vault.
    #[test]
    fn a_hold_takes_no_key_of_another_vault() {
        let dirs = [(); 2].map(|_| tempfile::tempdir().unwrap());
        let (a, b) = (
            vault_with_wallet(dirs[0].path(), Some(b"pw"), 0x5a),
            vault_with_wallet(dirs[1].path(), Some(b"pw"), 0x7f),
        );
        a.lock();
        b.lock();
        let mut tokens = [
            redeem(&a, PLATFORM_OP, Credential::Passphrase(b"pw")),
            redeem(&b, PLATFORM_OP, Credential::Passphrase(b"pw")),
        ];
        assert_eq!(
            a.hold_key(&mut tokens).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert_eq!(
            a.hold_key(&mut tokens[1..]).map(drop),
            Err(VaultError::GrantInvalid)
        );
        assert!(tokens.iter().all(|t| matches!(t.key, KeySource::Own(_))));
        let hold = b.hold_key(&mut tokens[1..]).unwrap().unwrap();
        b.platform_signer_held(&[1; 32], &hold, &tokens[1], SignerScope::DashPayCrypto)
            .unwrap();
        let hold = a.hold_key(&mut tokens[..1]).unwrap().unwrap();
        a.platform_signer_held(&[1; 32], &hold, &tokens[0], SignerScope::DashPayCrypto)
            .unwrap();
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
        let token = redeem(&earlier, GrantPurpose::IdentityScan, Credential::None);
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
        let scan = redeem(&v, GrantPurpose::IdentityScan, Credential::None);
        let platform = redeem(&v, PLATFORM_OP, Credential::None);
        let spend = redeem(&v, GrantPurpose::Spend { max_duffs: 1 }, Credential::None);
        let reveal = redeem(
            &v,
            GrantPurpose::RevealSecret,
            Credential::Passphrase(b"pw"),
        );
        v.lock();
        v.unlock(b"pw", UnlockScope::Full).unwrap();
        assert_eq!(
            v.scan_key(&w, &scan).map(drop),
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
        let scan = redeem(&v, GrantPurpose::IdentityScan, Credential::None);
        v.scan_key(&w, &scan).unwrap();
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
            &dashcore::secp256k1::SecretKey::from_secret_bytes([0x42; 32]).unwrap(),
        );

        v.lock();
        let old = Credential::Passphrase(b"old-pw");
        let reveal = redeem(&v, GrantPurpose::RevealSecret, old);
        let pending = v
            .authorize(GrantPurpose::RevealSecret, Some(&w), old)
            .unwrap();
        let mut held = [
            redeem(&v, PLATFORM_OP, old),
            redeem(&v, GrantPurpose::IdentityScan, old),
        ];
        let hold = v.hold_key(&mut held).unwrap().unwrap();
        let crypto = v
            .platform_signer_held(&w, &hold, &held[0], SignerScope::DashPayCrypto)
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
            v.scan_key_held(&w, &hold, &held[1]).map(drop),
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
        let mut scan = [redeem(
            &v,
            GrantPurpose::IdentityScan,
            Credential::Passphrase(b"new-pw-1"),
        )];
        let hold = v.hold_key(&mut scan).unwrap().unwrap();
        v.scan_key_held(&w, &hold, &scan[0]).unwrap();

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
                .with_key(&key, crate::signer::KeyUse::PublicKey, |_| ())
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
