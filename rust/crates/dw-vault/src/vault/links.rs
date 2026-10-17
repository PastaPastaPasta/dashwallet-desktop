//! The link stash (DP5-01, `docs/design/DP5-01-link-intake.md`): links the
//! user opened before the wallet could act on them, kept per network until
//! they are taken, dismissed or expire.
//!
//! - **Where.** One file per link, `<vault dir>/links/<id>.dwl` (0600, in a
//!   0700 directory), beside `vault.dwv` and never inside it: a record would
//!   turn `NoKeys` into `Unencrypted` and block `destroy` (D5).
//! - **Encrypted at rest** (D5). Each link is sealed to the vault's stash
//!   key: an ephemeral secp256k1 key, ECDH with the stash public key,
//!   HMAC-SHA256 into an XChaCha20-Poly1305 key, and associated data that
//!   binds the vault id, network, link id, kind, both times, the stash key
//!   id and the ephemeral key. The stash key is derived from the data key,
//!   so it needs no record and changes only when the data key does (a
//!   recovery). Sealing needs only the public key (`links/stash.pub`), so a
//!   link that arrives while the vault is locked is sealed too; opening
//!   needs the data key.
//! - **Before a vault exists** (DASHPAY §2.9, last row; D6) a link is a
//!   plaintext entry, owner-only. The vault seals every plaintext entry as
//!   soon as it has a key (`create`, `unlock`, a recovery, or any stash call
//!   on an open vault), overwriting the plaintext first. The same fallback
//!   holds while a vault is locked and has no valid `stash.pub` yet.
//! - **Bounded** (D7, D8): at most [`MAX_STASHED_LINKS`] entries of at most
//!   [`MAX_STASHED_LINK_BYTES`] each, with a time to live per kind. A full
//!   stash makes room: expired entries first, then the oldest
//!   non-invitation, then the oldest invitation.
//! - **Deduplicated** (D9) by kind and text: in plaintext when the key is
//!   at hand, otherwise against the links this process stashed (a tag under
//!   a per-process random key, kept in memory only); the next sync with the
//!   key collapses what is left.
//! - **Taken once** (D10): [`Vault::take_stashed_link`] opens and deletes
//!   the entry under the stash mutex, and returns the link only once the
//!   deletion is on disk.
//!
//! Nothing here logs a link or any part of one, and no error quotes one.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use dashcore::hashes::{Hash, HashEngine, Hmac, HmacEngine, sha256};
use dashcore::secp256k1::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

use super::Vault;
use crate::VaultError;
use crate::crypto::{self, Sealed, hex_bytes};
use crate::file::{self, push_field};
use crate::types::LockState;

/// Entries kept per network at most (U-01's inbox holds as many).
pub const MAX_STASHED_LINKS: usize = 16;
/// The longest link kept, in bytes (U-01's `MAX_LINK_BYTES`).
pub const MAX_STASHED_LINK_BYTES: usize = 4096;

/// The stash directory inside the vault directory.
pub(crate) const LINKS_DIR: &str = "links";
const PUB_FILE: &str = "stash.pub";
const PUB_TMP: &str = "stash.pub.tmp";
const ENTRY_EXT: &str = "dwl";
const ENTRY_TMP_EXT: &str = "tmp";
/// An entry file larger than this is not one of ours.
const MAX_ENTRY_FILE_BYTES: u64 = 16 * 1024;
const ENTRY_VERSION: u32 = 1;
const DAY: u64 = 24 * 60 * 60;

/// What a stashed link is, as the engine classified it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StashedLinkKind {
    /// A payment link (`dash:`, `pay:`, `dashwallet://pay=`).
    Payment,
    /// A DashPay user link (`dashpay://user`).
    Contact,
    /// A DashPay invitation: a bearer secret.
    Invitation,
}

impl StashedLinkKind {
    /// How long an entry of this kind is kept (D8).
    pub fn ttl_secs(self) -> u64 {
        match self {
            Self::Payment => 7 * DAY,
            Self::Contact | Self::Invitation => 30 * DAY,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Self::Payment => "payment",
            Self::Contact => "contact",
            Self::Invitation => "invitation",
        }
    }
}

/// A stashed link without its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashedLinkMeta {
    /// 32 lower-case hex characters, issued here.
    pub id: String,
    pub kind: StashedLinkKind,
    /// UNIX seconds.
    pub stashed_at: u64,
    /// UNIX seconds; the entry is deleted from then on.
    pub expires_at: u64,
}

/// The stash's in-process state, behind `Shared::links`: it serializes
/// every stash operation in this process (one process per data root,
/// U-01 D2).
pub(crate) struct LinkState {
    /// Keys the dedupe tags; random per process, never stored.
    dedupe_key: [u8; 32],
    /// Dedupe tag → id of the links this process stashed.
    seen: HashMap<[u8; 32], String>,
}

impl LinkState {
    pub(crate) fn new() -> Result<Self, VaultError> {
        Ok(Self {
            dedupe_key: crypto::random_array()?,
            seen: HashMap::new(),
        })
    }

    fn tag(&self, kind: StashedLinkKind, link: &str) -> [u8; 32] {
        let mut e = HmacEngine::<sha256::Hash>::new(&self.dedupe_key);
        e.input(kind.tag().as_bytes());
        e.input(&[0]);
        e.input(link.as_bytes());
        Hmac::<sha256::Hash>::from_engine(e).to_byte_array()
    }

    fn forget(&mut self, id: &str) {
        self.seen.retain(|_, v| v != id);
    }
}

impl Drop for LinkState {
    fn drop(&mut self) {
        self.dedupe_key.zeroize();
    }
}

/// The stash key pair of one vault, derived from its data key.
struct StashKey {
    secret: SecretKey,
    public: PublicKey,
    id: [u8; 8],
}

impl Drop for StashKey {
    fn drop(&mut self) {
        self.secret.non_secure_erase();
    }
}

impl StashKey {
    /// HMAC-SHA256 under the data key of a domain tag, the vault id, the
    /// network and a counter, until the output is a valid scalar.
    fn derive(dek: &[u8; 32], vault_id: &[u8], network: &str) -> Result<Self, VaultError> {
        for counter in 0u8..=255 {
            let mut e = HmacEngine::<sha256::Hash>::new(dek);
            let mut msg = b"dw-vault/link-stash/v1".to_vec();
            push_field(&mut msg, vault_id);
            push_field(&mut msg, network.as_bytes());
            msg.push(counter);
            e.input(&msg);
            let mut bytes = Zeroizing::new(Hmac::<sha256::Hash>::from_engine(e).to_byte_array());
            if let Ok(secret) = SecretKey::from_secret_bytes(*bytes) {
                bytes.zeroize();
                let public = PublicKey::from_secret_key(&secret);
                return Ok(Self {
                    secret,
                    public,
                    id: key_id(&public),
                });
            }
        }
        Err(VaultError::Internal("no stash key".into()))
    }
}

fn key_id(public: &PublicKey) -> [u8; 8] {
    let digest = sha256::Hash::hash(&public.serialize()).to_byte_array();
    let mut id = [0u8; 8];
    id.copy_from_slice(&digest[..8]);
    id
}

/// `links/stash.pub`: the stash public key, for sealing while locked. Not
/// authenticated: rewritten from the data key at every sync (D19).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PubFile {
    v: u32,
    #[serde(with = "hex_bytes")]
    vault_id: Vec<u8>,
    #[serde(with = "hex_bytes")]
    public: Vec<u8>,
}

/// One entry file. Exactly one of `sealed` and `plain` is set.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntryFile {
    v: u32,
    id: String,
    kind: StashedLinkKind,
    stashed_at: u64,
    expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sealed: Option<SealedLink>,
    /// Before a vault (or its stash key) exists only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plain: Option<String>,
}

impl Drop for EntryFile {
    fn drop(&mut self) {
        if let Some(p) = &mut self.plain {
            p.zeroize();
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SealedLink {
    #[serde(with = "hex_bytes")]
    key_id: Vec<u8>,
    #[serde(with = "hex_bytes")]
    epk: Vec<u8>,
    #[serde(flatten)]
    body: Sealed,
}

impl EntryFile {
    fn meta(&self) -> StashedLinkMeta {
        StashedLinkMeta {
            id: self.id.clone(),
            kind: self.kind,
            stashed_at: self.stashed_at,
            expires_at: self.expires_at,
        }
    }

    /// Whether the entry is one this module wrote for `id` (the file name).
    fn well_formed(&self, id: &str) -> bool {
        self.v == ENTRY_VERSION
            && self.id == id
            && self.sealed.is_some() != self.plain.is_some()
            && self.expires_at > self.stashed_at
            && self.expires_at - self.stashed_at <= self.kind.ttl_secs()
            && self
                .plain
                .as_ref()
                .is_none_or(|p| !p.is_empty() && p.len() <= MAX_STASHED_LINK_BYTES)
    }
}

/// `true` for an id this module issues: 32 lower-case hex characters.
pub(crate) fn is_link_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn entry_aad(vault_id: &[u8], network: &str, e: &EntryFile, key_id: &[u8], epk: &[u8]) -> Vec<u8> {
    let mut a = b"dw-vault/link/v1".to_vec();
    push_field(&mut a, vault_id);
    push_field(&mut a, network.as_bytes());
    push_field(&mut a, e.id.as_bytes());
    push_field(&mut a, e.kind.tag().as_bytes());
    push_field(&mut a, &e.stashed_at.to_le_bytes());
    push_field(&mut a, &e.expires_at.to_le_bytes());
    push_field(&mut a, key_id);
    push_field(&mut a, epk);
    a
}

/// The entry key: HMAC-SHA256 under the ECDH secret of a domain tag, the
/// ephemeral key and the stash key.
fn entry_key(shared: &[u8; 32], epk: &[u8], stash: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut e = HmacEngine::<sha256::Hash>::new(shared);
    let mut msg = b"dw-vault/link-key/v1".to_vec();
    push_field(&mut msg, epk);
    push_field(&mut msg, stash);
    e.input(&msg);
    Zeroizing::new(Hmac::<sha256::Hash>::from_engine(e).to_byte_array())
}

/// A fresh random secp256k1 secret key.
fn ephemeral_secret() -> Result<SecretKey, VaultError> {
    loop {
        let mut bytes = Zeroizing::new(crypto::random_array::<32>()?);
        if let Ok(k) = SecretKey::from_secret_bytes(*bytes) {
            bytes.zeroize();
            return Ok(k);
        }
    }
}

/// Seals `link` for the stash key `public` into `e.sealed`.
fn seal_into(
    e: &mut EntryFile,
    link: &str,
    public: &PublicKey,
    vault_id: &[u8],
    network: &str,
) -> Result<(), VaultError> {
    let mut esk = ephemeral_secret()?;
    let epk = PublicKey::from_secret_key(&esk).serialize();
    let shared = crate::dip15::ecdh(&esk, public);
    esk.non_secure_erase();
    let stash = public.serialize();
    let key = entry_key(&shared, &epk, &stash);
    let kid = key_id(public);
    let aad = entry_aad(vault_id, network, e, &kid, &epk);
    e.sealed = Some(SealedLink {
        key_id: kid.to_vec(),
        epk: epk.to_vec(),
        body: crypto::seal(&key, link.as_bytes(), &aad)?,
    });
    if let Some(mut p) = e.plain.take() {
        p.zeroize();
    }
    Ok(())
}

/// Why an entry cannot be read.
enum Unreadable {
    /// Sealed to another stash key (an older vault's): it can never open.
    OtherKey,
    /// The key opens it but authentication fails: tampered.
    Tampered,
}

/// The text of `e`: its plaintext, or its sealed body opened with `key`.
fn open_entry(
    e: &EntryFile,
    key: &StashKey,
    vault_id: &[u8],
    network: &str,
) -> Result<Zeroizing<String>, Unreadable> {
    if let Some(p) = &e.plain {
        return Ok(Zeroizing::new(p.clone()));
    }
    let Some(s) = &e.sealed else {
        return Err(Unreadable::Tampered);
    };
    if s.key_id[..] != key.id[..] {
        return Err(Unreadable::OtherKey);
    }
    let epk = PublicKey::from_slice(&s.epk).map_err(|_| Unreadable::Tampered)?;
    let shared = crate::dip15::ecdh(&key.secret, &epk);
    let k = entry_key(&shared, &s.epk, &key.public.serialize());
    let aad = entry_aad(vault_id, network, e, &s.key_id, &s.epk);
    let plain = crypto::open(&k, &s.body, &aad).ok_or(Unreadable::Tampered)?;
    let text = std::str::from_utf8(&plain).map_err(|_| Unreadable::Tampered)?;
    Ok(Zeroizing::new(text.to_owned()))
}

/// Overwrites `path` with zeros and syncs it: best effort, before the
/// plaintext's name goes (D18).
fn scrub(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().write(true).open(path) {
        let zeros = vec![0u8; meta.len() as usize];
        let _ = f.write_all(&zeros).and_then(|()| f.sync_all());
    }
}

fn sync_dir(dir: &Path) -> Result<(), VaultError> {
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Writes `bytes` to `tmp` (created new, 0600), syncs it and renames it to
/// `path`, then syncs the directory.
fn write_replacing(dir: &Path, tmp: &Path, path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    match std::fs::remove_file(tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)?;
    sync_dir(dir)
}

/// Reads one entry; `None` for a file that is not one (wrong name, too
/// large, not ours, does not parse).
fn read_entry(path: &Path, id: &str) -> Option<EntryFile> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ENTRY_FILE_BYTES {
        return None;
    }
    let bytes = Zeroizing::new(std::fs::read(path).ok()?);
    let e: EntryFile = serde_json::from_slice(&bytes).ok()?;
    e.well_formed(id).then_some(e)
}

impl Vault {
    fn links_dir(&self) -> PathBuf {
        self.shared.dir.join(LINKS_DIR)
    }

    fn entry_path(&self, id: &str) -> PathBuf {
        self.links_dir().join(format!("{id}.{ENTRY_EXT}"))
    }

    fn links_state(&self) -> std::sync::MutexGuard<'_, LinkState> {
        self.shared
            .links
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The vault id and network tag the stash binds; the id is `None`
    /// without a vault.
    fn link_binding(&self) -> (Option<Vec<u8>>, String) {
        (self.vault_id(), self.shared.tag.clone())
    }

    /// Runs `body` with the stash key, as one gated operation: `Ok(None)`
    /// when there is no key at hand (no vault, or locked). A mixing-only
    /// unlock has the data key, and reading a link signs nothing, so it
    /// counts (D11). An unencrypted vault's key is loaded from the OS
    /// store first, outside the gate.
    fn with_stash_key<T>(
        &self,
        body: impl FnOnce(&StashKey, &[u8]) -> Result<T, VaultError>,
    ) -> Result<Option<T>, VaultError> {
        match self.lock_state() {
            LockState::NoVault | LockState::Locked => return Ok(None),
            LockState::NoKeys | LockState::Unencrypted => {
                self.full_dek()?;
            }
            LockState::Unlocked | LockState::UnlockedMixingOnly => {}
        }
        self.gated(VaultError::Locked, |_op| {
            let (dek, vault_id) = {
                let inner = self.inner();
                let (Some(dek), Some(f)) = (&inner.dek, &inner.file) else {
                    return Ok(None);
                };
                (Zeroizing::new(**dek), f.vault_id.clone())
            };
            let key = StashKey::derive(&dek, &vault_id, &self.shared.tag)?;
            body(&key, &vault_id).map(Some)
        })
    }

    /// The stash public key `stash.pub` names for this vault, if valid.
    fn stored_stash_public(&self, vault_id: &[u8]) -> Option<PublicKey> {
        let path = self.links_dir().join(PUB_FILE);
        let meta = std::fs::symlink_metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > 1024 {
            return None;
        }
        let f: PubFile = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
        (f.v == 1 && f.vault_id == vault_id)
            .then(|| PublicKey::from_slice(&f.public).ok())
            .flatten()
    }

    fn ensure_links_dir(&self) -> Result<PathBuf, VaultError> {
        file::create_vault_dir(&self.shared.dir)?;
        dw_fs::create_owned_dir(&self.shared.dir, Path::new(LINKS_DIR))?;
        Ok(self.links_dir())
    }

    fn write_stash_public(&self, key: &StashKey, vault_id: &[u8]) -> Result<(), VaultError> {
        if self
            .stored_stash_public(vault_id)
            .is_some_and(|p| p == key.public)
        {
            return Ok(());
        }
        let dir = self.ensure_links_dir()?;
        let bytes = serde_json::to_vec(&PubFile {
            v: 1,
            vault_id: vault_id.to_vec(),
            public: key.public.serialize().to_vec(),
        })
        .map_err(|e| VaultError::Internal(format!("stash key file: {e}")))?;
        write_replacing(&dir, &dir.join(PUB_TMP), &dir.join(PUB_FILE), &bytes)
    }

    /// Every well-formed entry, oldest first. Expired entries and files that
    /// are not entries (left by a crash, or not ours) are deleted.
    fn load_entries(&self, now: u64) -> Result<Vec<EntryFile>, VaultError> {
        let dir = self.links_dir();
        let read = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut entries = Vec::new();
        let mut dropped = 0usize;
        for item in read {
            let item = item?;
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            let path = item.path();
            if name == PUB_FILE {
                continue;
            }
            let (stem, ext) = name.rsplit_once('.').unwrap_or((name, ""));
            if ext == ENTRY_TMP_EXT && is_link_id(stem) || name == PUB_TMP {
                scrub(&path);
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if ext != ENTRY_EXT || !is_link_id(stem) {
                continue;
            }
            match read_entry(&path, stem) {
                Some(e) if e.expires_at > now => entries.push(e),
                _ => {
                    scrub(&path);
                    std::fs::remove_file(&path)?;
                    dropped += 1;
                }
            }
        }
        if dropped > 0 {
            sync_dir(&dir)?;
            tracing::debug!(dropped, "link stash: expired or unreadable entries deleted");
        }
        entries.sort_by(|a, b| (a.stashed_at, &a.id).cmp(&(b.stashed_at, &b.id)));
        Ok(entries)
    }

    fn delete_entry(&self, e: &EntryFile) -> Result<(), VaultError> {
        let path = self.entry_path(&e.id);
        if e.plain.is_some() {
            scrub(&path);
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
        sync_dir(&self.links_dir())
    }

    fn write_entry(&self, e: &EntryFile) -> Result<(), VaultError> {
        let dir = self.ensure_links_dir()?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(e)
                .map_err(|err| VaultError::Internal(format!("link entry: {err}")))?,
        );
        let path = self.entry_path(&e.id);
        if e.sealed.is_some() {
            scrub(&path);
        }
        let tmp = dir.join(format!("{}.{ENTRY_TMP_EXT}", e.id));
        write_replacing(&dir, &tmp, &path, &bytes)
    }

    /// With the stash key: rewrites `stash.pub`, seals plaintext entries,
    /// drops entries sealed to another key or tampered with, and collapses
    /// duplicates (oldest kept). Returns what is left, oldest first.
    fn sync_links_with(
        &self,
        key: &StashKey,
        vault_id: &[u8],
        entries: Vec<EntryFile>,
    ) -> Result<Vec<EntryFile>, VaultError> {
        self.write_stash_public(key, vault_id)?;
        let tag = &self.shared.tag;
        let mut kept: Vec<EntryFile> = Vec::with_capacity(entries.len());
        let mut texts: Vec<(StashedLinkKind, Zeroizing<String>)> = Vec::new();
        let (mut sealed, mut dropped) = (0usize, 0usize);
        for mut e in entries {
            let text = match open_entry(&e, key, vault_id, tag) {
                Ok(t) => t,
                Err(Unreadable::OtherKey | Unreadable::Tampered) => {
                    self.delete_entry(&e)?;
                    dropped += 1;
                    continue;
                }
            };
            let duplicate = texts
                .iter()
                .any(|(k, t)| *k == e.kind && bool::from(t.as_bytes().ct_eq(text.as_bytes())));
            if duplicate {
                self.delete_entry(&e)?;
                dropped += 1;
                continue;
            }
            if e.plain.is_some() {
                seal_into(&mut e, &text, &key.public, vault_id, tag)?;
                self.write_entry(&e)?;
                sealed += 1;
            }
            texts.push((e.kind, text));
            kept.push(e);
        }
        if sealed + dropped > 0 {
            tracing::debug!(sealed, dropped, "link stash synced");
        }
        Ok(kept)
    }

    /// Seals what waits in plaintext once the vault has its key: after
    /// `create` (the move into the vault, DASHPAY §2.9), `unlock` and a
    /// recovery. Best effort: a failure is logged and the next stash call
    /// tries again.
    pub(crate) fn sync_links_after_key(&self) {
        let _state = self.links_state();
        let result = self.with_stash_key(|key, vault_id| {
            let entries = self.load_entries(self.now())?;
            self.sync_links_with(key, vault_id, entries).map(drop)
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, "link stash: could not seal pending links");
        }
    }

    /// Removes the stash with every entry (`destroy`).
    pub(crate) fn remove_links(&self) -> Result<(), VaultError> {
        let mut state = self.links_state();
        state.seen.clear();
        let dir = self.links_dir();
        match std::fs::read_dir(&dir) {
            Ok(read) => {
                for item in read {
                    let path = item?.path();
                    scrub(&path);
                    std::fs::remove_file(&path)?;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
        std::fs::remove_dir(&dir)?;
        sync_dir(&self.shared.dir)
    }

    /// Keeps `link` (already classified as `kind` by the caller) and returns
    /// its entry. The same link of the same kind stashed again returns the
    /// entry it already has (D9). A full stash makes room (D7). Works in
    /// every lock state; the text never reaches a log or an error. Text
    /// that is empty, over [`MAX_STASHED_LINK_BYTES`] or holds a control
    /// character (after trimming) is `InvalidArgument`.
    pub fn stash_link(
        &self,
        kind: StashedLinkKind,
        link: &str,
    ) -> Result<StashedLinkMeta, VaultError> {
        let link = link.trim();
        if link.is_empty() || link.len() > MAX_STASHED_LINK_BYTES {
            return Err(VaultError::InvalidArgument(format!(
                "a stashed link has 1 to {MAX_STASHED_LINK_BYTES} bytes"
            )));
        }
        // One line, so log redaction (cut at the end of a line) holds.
        if link.chars().any(char::is_control) {
            return Err(VaultError::InvalidArgument(
                "a stashed link has no control characters".into(),
            ));
        }
        let mut state = self.links_state();
        let now = self.now();
        let tag = state.tag(kind, link);
        let mut entries = self.load_entries(now)?;
        // With the key at hand: seal what waits and compare in plaintext.
        let (vault_id, network) = self.link_binding();
        let synced = self.with_stash_key(|key, vault_id| {
            let kept = self.sync_links_with(key, vault_id, std::mem::take(&mut entries))?;
            let mut same = None;
            for e in &kept {
                if e.kind == kind
                    && let Ok(text) = open_entry(e, key, vault_id, &network)
                    && bool::from(text.as_bytes().ct_eq(link.as_bytes()))
                {
                    same = Some(e.meta());
                    break;
                }
            }
            Ok((kept, same, key.public))
        });
        let public = match synced {
            Ok(Some((kept, same, public))) => {
                entries = kept;
                if let Some(meta) = same {
                    state.seen.insert(tag, meta.id.clone());
                    return Ok(meta);
                }
                Some(public)
            }
            // Locked meanwhile, or no key: the entries are as loaded.
            Ok(None) | Err(VaultError::Locked) => {
                if entries.is_empty() {
                    entries = self.load_entries(now)?;
                }
                None
            }
            Err(e) => return Err(e),
        };
        if public.is_none() {
            if let Some(id) = state.seen.get(&tag)
                && let Some(e) = entries.iter().find(|e| &e.id == id && e.kind == kind)
            {
                return Ok(e.meta());
            }
            // Plaintext entries can be compared without the key.
            if let Some(e) = entries.iter().find(|e| {
                e.kind == kind
                    && e.plain
                        .as_ref()
                        .is_some_and(|p| bool::from(p.as_bytes().ct_eq(link.as_bytes())))
            }) {
                let meta = e.meta();
                state.seen.insert(tag, meta.id.clone());
                return Ok(meta);
            }
        }
        // Room for one more (D7).
        while entries.len() >= MAX_STASHED_LINKS {
            let victim = entries
                .iter()
                .position(|e| e.kind != StashedLinkKind::Invitation)
                .unwrap_or(0);
            let e = entries.remove(victim);
            self.delete_entry(&e)?;
            state.forget(&e.id);
            tracing::debug!(
                kind = e.kind.tag(),
                "link stash full: the oldest entry made room"
            );
        }
        let id = hex::encode(crypto::random_array::<16>()?);
        let mut entry = EntryFile {
            v: ENTRY_VERSION,
            id: id.clone(),
            kind,
            stashed_at: now,
            expires_at: now + kind.ttl_secs(),
            sealed: None,
            plain: None,
        };
        let seal_to = public.or_else(|| {
            vault_id
                .as_deref()
                .and_then(|v| self.stored_stash_public(v))
        });
        match (seal_to, &vault_id) {
            (Some(p), Some(v)) => seal_into(&mut entry, link, &p, v, &network)?,
            // No vault yet, or no stash key on disk (D6).
            _ => entry.plain = Some(link.to_owned()),
        }
        self.write_entry(&entry)?;
        state.seen.insert(tag, id);
        tracing::debug!(
            kind = kind.tag(),
            sealed = entry.sealed.is_some(),
            "link stashed"
        );
        Ok(entry.meta())
    }

    /// The stashed links, oldest first, without their text. Deletes the
    /// expired ones; with the key at hand also seals what waits in
    /// plaintext and collapses duplicates. Works in every lock state.
    pub fn stashed_links(&self) -> Result<Vec<StashedLinkMeta>, VaultError> {
        let _state = self.links_state();
        let entries = self.load_entries(self.now())?;
        let entries = match self.with_stash_key(|key, vault_id| {
            self.sync_links_with(key, vault_id, self.load_entries(self.now())?)
        }) {
            Ok(Some(kept)) => kept,
            Ok(None) | Err(VaultError::Locked) => entries,
            Err(e) => return Err(e),
        };
        Ok(entries.iter().map(EntryFile::meta).collect())
    }

    /// Opens the entry `id` and deletes it: its kind and text, once (D10).
    /// `Ok(None)` when there is no such entry (taken, dismissed, expired,
    /// or sealed to an older vault's key). Needs the data key unless the
    /// entry is still plaintext: `Locked` otherwise, and the entry stays.
    /// The deletion is on disk before the text is returned; when it fails,
    /// the error is returned and the text dropped.
    pub fn take_stashed_link(
        &self,
        id: &str,
    ) -> Result<Option<(StashedLinkKind, Zeroizing<String>)>, VaultError> {
        if !is_link_id(id) {
            return Err(VaultError::InvalidArgument("not a stashed link id".into()));
        }
        let mut state = self.links_state();
        let now = self.now();
        let Some(entry) = read_entry(&self.entry_path(id), id) else {
            return Ok(None);
        };
        if entry.expires_at <= now {
            self.delete_entry(&entry)?;
            state.forget(id);
            return Ok(None);
        }
        let network = self.shared.tag.clone();
        let text = match &entry.plain {
            Some(p) => Some(Ok(Zeroizing::new(p.clone()))),
            None => self
                .with_stash_key(|key, vault_id| Ok(open_entry(&entry, key, vault_id, &network)))?,
        };
        let text = match text {
            None => return Err(VaultError::Locked),
            Some(Ok(text)) => text,
            Some(Err(_)) => {
                // It can never open: sealed to an older key, or tampered.
                self.delete_entry(&entry)?;
                state.forget(id);
                tracing::debug!("link stash: an unreadable entry was deleted");
                return Ok(None);
            }
        };
        self.delete_entry(&entry)?;
        state.forget(id);
        Ok(Some((entry.kind, text)))
    }

    /// Deletes the entry `id` without reading it; `false` when there was
    /// none.
    pub fn forget_stashed_link(&self, id: &str) -> Result<bool, VaultError> {
        if !is_link_id(id) {
            return Err(VaultError::InvalidArgument("not a stashed link id".into()));
        }
        let mut state = self.links_state();
        let path = self.entry_path(id);
        let Some(entry) = read_entry(&path, id) else {
            // Not an entry (or none): nothing that is not ours is deleted.
            return Ok(false);
        };
        self.delete_entry(&entry)?;
        state.forget(id);
        Ok(true)
    }
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
