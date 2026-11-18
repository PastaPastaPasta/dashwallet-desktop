//! Dash Core `wallet.dat` files.
//!
//! - **SQLite descriptor wallets** (Dash Core v21+, the v24 default): a table
//!   `main(key BLOB PRIMARY KEY, value BLOB)` holding Core-serialized records
//!   (`src/wallet/walletdb.cpp`). This module reads the records an import
//!   needs:
//!   - `walletdescriptor` (id → descriptor string, creation time, range,
//!     next index);
//!   - `walletdescriptorkey` (id, pubkey → DER private key, its hash, and in
//!     Dash the BIP39 mnemonic and mnemonic passphrase);
//!   - `walletdescriptorckey` (id, pubkey → the same, encrypted with the
//!     master key, IV = `Hash(pubkey)`; see [`crate::crypter`]);
//!   - `mkey` (the master key encrypted with the wallet passphrase);
//!   - `name` / `purpose` (address book), `flags`.
//! - **Berkeley DB** legacy wallets (magic `0x00053162` at offset 12): the
//!   same records, plus `key` / `wkey` / `ckey`, `hdchain` / `chdchain` and
//!   `keymeta`. A crypted chain's seed and mnemonic are AES-256-CBC under
//!   the master key with IV = the first 16 bytes of the chain id.
//!
//! The SQLite file is opened read-only and immutable, so importing never
//! writes next to the user's file (no `-wal`/`-shm`/journal).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use dashcore::secp256k1::{PublicKey, SecretKey};
use zeroize::Zeroizing;

use crate::crypter::{self, MasterKeyRecord};
use crate::descriptor::PkhDescriptor;

/// `SQLite format 3\0`.
pub const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
/// Berkeley DB btree magic, at byte offset 12 of the first page.
pub const BDB_BTREE_MAGIC: u32 = 0x0005_3162;
/// `WALLET_FLAG_DESCRIPTORS`.
pub const FLAG_DESCRIPTORS: u64 = 1 << 34;
/// Largest record Core writes is a transaction; nothing in a wallet record
/// we read comes close.
const MAX_FIELD: u64 = 32 * 1024 * 1024;

/// Which wallet.dat container a file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    Sqlite,
    BerkeleyDb,
}

/// Recognises a wallet.dat container from its first bytes.
pub fn sniff_container(head: &[u8]) -> Option<Container> {
    if head.starts_with(SQLITE_MAGIC) {
        return Some(Container::Sqlite);
    }
    let magic_at = |off: usize| {
        head.get(off..off + 4)
            .map(|b| [b[0], b[1], b[2], b[3]])
            .is_some_and(|b| {
                u32::from_le_bytes(b) == BDB_BTREE_MAGIC || u32::from_be_bytes(b) == BDB_BTREE_MAGIC
            })
    };
    magic_at(12).then_some(Container::BerkeleyDb)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletDatError {
    /// The file cannot be opened or read.
    Unreadable(String),
    /// A legacy (non-descriptor) SQLite wallet, or no `main` table.
    Unsupported(String),
    /// A record does not parse or keys do not match.
    Corrupt(String),
    /// Encrypted wallet and no passphrase given.
    PassphraseRequired,
    /// The passphrase does not decrypt the master key.
    WrongPassphrase,
    /// Legacy wallet with no HD chain to restore from.
    NoHdChain,
    /// Legacy HD seed whose length the importer cannot store.
    SeedLength(usize),
}

impl std::fmt::Display for WalletDatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(d) => write!(f, "unreadable: {d}"),
            Self::Unsupported(d) => write!(f, "unsupported: {d}"),
            Self::Corrupt(d) => write!(f, "corrupt: {d}"),
            Self::PassphraseRequired => f.write_str("passphrase required"),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
            Self::NoHdChain => f.write_str("no HD chain"),
            Self::SeedLength(n) => write!(f, "HD seed is {n} bytes"),
        }
    }
}

impl std::error::Error for WalletDatError {}

pub(crate) fn corrupt(what: impl Into<String>) -> WalletDatError {
    WalletDatError::Corrupt(what.into())
}

/// Reader over Core's serialization (`src/serialize.h`).
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf }
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Some(head)
    }

    pub fn compact_size(&mut self) -> Option<u64> {
        let first = *self.take(1)?.first()?;
        let n = match first {
            0..=252 => u64::from(first),
            253 => u64::from(u16::from_le_bytes(self.take(2)?.try_into().ok()?)),
            254 => u64::from(u32::from_le_bytes(self.take(4)?.try_into().ok()?)),
            255 => u64::from_le_bytes(self.take(8)?.try_into().ok()?),
        };
        (n <= MAX_FIELD).then_some(n)
    }

    /// `std::vector<unsigned char>` / `std::string`: CompactSize length + bytes.
    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.compact_size()? as usize;
        self.take(n)
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    pub fn i64(&mut self) -> Option<i64> {
        Some(i64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    /// Core's `bool`: only 0 and 1.
    pub fn bool(&mut self) -> Option<bool> {
        match self.take(1)? {
            [0] => Some(false),
            [1] => Some(true),
            _ => None,
        }
    }

    pub fn hash256(&mut self) -> Option<[u8; 32]> {
        self.take(32)?.try_into().ok()
    }
}

/// `ec_seckey_import_der` (Core `src/key.cpp`): the 32-byte secret of a DER
/// `CPrivKey`.
pub(crate) fn der_secret(der: &[u8]) -> Option<Zeroizing<[u8; 32]>> {
    let mut r = Reader::new(der);
    if r.take(1)? != [0x30] {
        return None;
    }
    let lenb = *r.take(1)?.first()?;
    if lenb & 0x80 == 0 {
        return None;
    }
    let lenb = usize::from(lenb & 0x7f);
    if !(1..=2).contains(&lenb) {
        return None;
    }
    let len_bytes = r.take(lenb)?;
    let len = len_bytes
        .iter()
        .fold(0usize, |acc, b| acc << 8 | usize::from(*b));
    let body = r.take(len)?;
    let mut b = Reader::new(body);
    if b.take(3)? != [0x02, 0x01, 0x01] || b.take(1)? != [0x04] {
        return None;
    }
    let oslen = usize::from(*b.take(1)?.first()?);
    if oslen > 32 {
        return None;
    }
    let os = b.take(oslen)?;
    let mut out = Zeroizing::new([0u8; 32]);
    out[32 - oslen..].copy_from_slice(os);
    SecretKey::from_secret_bytes(*out).ok()?;
    Some(out)
}

/// One `walletdescriptor` record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorRecord {
    pub id: [u8; 32],
    /// The public descriptor string as stored (with checksum).
    pub descriptor: String,
    pub creation_time: u64,
    pub range_start: i32,
    pub range_end: i32,
    pub next_index: i32,
}

/// An address-book entry (`name` + `purpose` records).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressBookEntry {
    pub address: String,
    pub label: String,
    /// `receive` / `send` / other, as stored; `None` without a purpose record.
    pub purpose: Option<String>,
}

/// A key record before decryption.
struct KeyRecord {
    descriptor_id: [u8; 32],
    pubkey: Vec<u8>,
    /// DER private key (plain) or AES ciphertext of the 32-byte secret.
    secret: Zeroizing<Vec<u8>>,
    mnemonic: Zeroizing<Vec<u8>>,
    mnemonic_passphrase: Zeroizing<Vec<u8>>,
    encrypted: bool,
}

/// Dash Core `CHDChain` (`hdchain` / `chdchain`). Seed and mnemonic are
/// ciphertext while `crypted` is set.
struct LegacyChain {
    id: [u8; 32],
    crypted: bool,
    seed: Zeroizing<Vec<u8>>,
    mnemonic: Zeroizing<Vec<u8>>,
    mnemonic_passphrase: Zeroizing<Vec<u8>>,
    /// Account index → (external counter, internal counter).
    accounts: BTreeMap<u32, (u32, u32)>,
}

struct CryptedKey {
    pubkey: Vec<u8>,
    secret: Zeroizing<Vec<u8>>,
}

/// Records gathered while walking key/value pairs.
#[derive(Default)]
struct Gather {
    names: BTreeMap<String, String>,
    purposes: BTreeMap<String, String>,
    hd_pubs: BTreeSet<Vec<u8>>,
    key_pubs: Vec<Vec<u8>>,
}

/// The records of a wallet.dat an import uses, not yet decrypted.
pub struct SqliteWallet {
    pub flags: u64,
    pub descriptors: Vec<DescriptorRecord>,
    pub address_book: Vec<AddressBookEntry>,
    /// `activeexternalspk` / `activeinternalspk` descriptor ids.
    pub active_external: Option<[u8; 32]>,
    pub active_internal: Option<[u8; 32]>,
    /// The CoinJoin salt (`cj_salt`, or the older `ps_salt`), uint256 in
    /// internal byte order (Dash Core `src/wallet/walletdb.cpp:224-233`).
    pub coinjoin_salt: Option<[u8; 32]>,
    master_keys: Vec<MasterKeyRecord>,
    keys: Vec<KeyRecord>,
    legacy: Option<LegacyChain>,
    crypted_keys: Vec<CryptedKey>,
    /// Plain or encrypted keys that are not listed as `hdpubkey`.
    pub loose_keys: u32,
    /// `cscript` and watch-only scripts, which the HD import leaves out.
    pub scripts: u32,
    /// `Some(true)` when the address book has a mainnet address.
    pub address_mainnet: Option<bool>,
    earliest_key_time: Option<i64>,
}

impl std::fmt::Debug for SqliteWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteWallet")
            .field("flags", &self.flags)
            .field("descriptors", &self.descriptors.len())
            .field("encrypted", &self.encrypted())
            .finish_non_exhaustive()
    }
}

/// A BIP39 phrase and its passphrase, as bytes.
pub type PhraseAndPassphrase = (Zeroizing<Vec<u8>>, Zeroizing<Vec<u8>>);

/// The HD root recovered from a wallet.dat.
pub struct HdRoot {
    /// BIP39 phrase and passphrase bytes as Core stored them (Dash
    /// descriptor wallets created with `createwallet`/`upgradetohd`).
    pub mnemonic: Option<PhraseAndPassphrase>,
    /// The 32-byte secret of the master key the active receive descriptor is
    /// rooted at, and its compressed public key.
    pub master_secret: Zeroizing<[u8; 32]>,
    pub master_pubkey: [u8; 33],
    /// The active receive descriptor (public form). For a legacy chain the
    /// key text is only a network hint (`xpub`, `tpub`, or `unknown`).
    pub external: PkhDescriptor,
    /// The legacy HD seed when the chain has no mnemonic (`sethdseed`).
    pub hd_seed: Option<Zeroizing<Vec<u8>>>,
}

impl std::fmt::Debug for HdRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdRoot")
            .field("has_mnemonic", &self.mnemonic.is_some())
            .field("has_hd_seed", &self.hd_seed.is_some())
            .field("external", &self.external)
            .finish_non_exhaustive()
    }
}

impl SqliteWallet {
    /// Whether the wallet has a master key record (`encryptwallet` ran).
    pub fn encrypted(&self) -> bool {
        !self.master_keys.is_empty()
            || self.keys.iter().any(|k| k.encrypted)
            || self.legacy.as_ref().is_some_and(|c| c.crypted)
            || !self.crypted_keys.is_empty()
    }

    pub fn is_descriptor_wallet(&self) -> bool {
        self.flags & FLAG_DESCRIPTORS != 0
    }

    /// Whether some key record carries a (possibly encrypted) mnemonic.
    pub fn has_mnemonic(&self) -> bool {
        self.keys.iter().any(|k| !k.mnemonic.is_empty())
    }

    /// The active receive descriptor record.
    pub fn external_descriptor(&self) -> Option<&DescriptorRecord> {
        let id = self.active_external?;
        self.descriptors.iter().find(|d| d.id == id)
    }

    /// Highest `next_index` of the active descriptors: how many addresses of
    /// a chain the wallet handed out.
    pub fn next_index(&self) -> u32 {
        if self.is_descriptor_wallet() {
            return self
                .descriptors
                .iter()
                .filter(|d| {
                    Some(d.id) == self.active_external || Some(d.id) == self.active_internal
                })
                .map(|d| d.next_index.max(0) as u32)
                .max()
                .unwrap_or(0);
        }
        self.legacy
            .as_ref()
            .and_then(|c| c.accounts.values().map(|(ext, int)| (*ext).max(*int)).max())
            .unwrap_or(0)
    }

    /// Earliest descriptor creation time (UNIX seconds).
    pub fn birth_time(&self) -> Option<u64> {
        self.descriptors.iter().map(|d| d.creation_time).min()
    }

    /// Decrypts (when encrypted) the key of the active receive descriptor and
    /// its mnemonic. The key must be the master key the descriptor is rooted
    /// at, and its public key must match.
    pub fn hd_root(&self, passphrase: Option<&[u8]>) -> Result<HdRoot, WalletDatError> {
        if !self.is_descriptor_wallet() {
            return self.legacy_hd_root(passphrase);
        }
        let record = self
            .external_descriptor()
            .ok_or_else(|| corrupt("no active receive descriptor"))?;
        let external = PkhDescriptor::parse(&record.descriptor)
            .map_err(|e| WalletDatError::Unsupported(e.to_string()))?;
        let key = self
            .keys
            .iter()
            .find(|k| k.descriptor_id == record.id)
            .ok_or_else(|| {
                WalletDatError::Unsupported("watch-only descriptor wallet (no private key)".into())
            })?;
        let master = if key.encrypted {
            let pw = passphrase.ok_or(WalletDatError::PassphraseRequired)?;
            Some(self.master_key(pw, key)?)
        } else {
            None
        };
        let secret = match &master {
            Some(m) => {
                let plain = crypter::decrypt_secret(m, &key.secret, &key.pubkey)
                    .ok_or(WalletDatError::WrongPassphrase)?;
                crypter::secret32(&plain).ok_or(WalletDatError::WrongPassphrase)?
            }
            None => der_secret(&key.secret).ok_or_else(|| corrupt("private key DER"))?,
        };
        let pubkey = pubkey_of(&secret).ok_or_else(|| corrupt("private key out of range"))?;
        if pubkey[..] != key.pubkey[..] {
            return Err(if master.is_some() {
                WalletDatError::WrongPassphrase
            } else {
                corrupt("private key does not match its public key")
            });
        }
        // The descriptor must be rooted at this key (a master xpub, depth 0).
        let xpub = key_wallet::bip32::ExtendedPubKey::from_str(&external.key)
            .map_err(|e| corrupt(format!("descriptor key: {e}")))?;
        if xpub.depth != 0 || xpub.public_key.serialize() != pubkey {
            return Err(WalletDatError::Unsupported(
                "the receive descriptor is not rooted at the wallet's master key".into(),
            ));
        }
        let mnemonic = if key.mnemonic.is_empty() {
            None
        } else if let Some(m) = &master {
            let phrase = crypter::decrypt_secret(m, &key.mnemonic, &key.pubkey)
                .ok_or_else(|| corrupt("encrypted mnemonic"))?;
            let pass = if key.mnemonic_passphrase.is_empty() {
                Zeroizing::new(Vec::new())
            } else {
                crypter::decrypt_secret(m, &key.mnemonic_passphrase, &key.pubkey)
                    .ok_or_else(|| corrupt("encrypted mnemonic passphrase"))?
            };
            Some((phrase, pass))
        } else {
            Some((key.mnemonic.clone(), key.mnemonic_passphrase.clone()))
        };
        Ok(HdRoot {
            mnemonic,
            master_secret: secret,
            master_pubkey: pubkey,
            external,
            hd_seed: None,
        })
    }

    /// Opens a legacy BDB chain: passphrase when the chain or a `ckey` is
    /// encrypted, then the mnemonic or the raw seed.
    fn legacy_hd_root(&self, passphrase: Option<&[u8]>) -> Result<HdRoot, WalletDatError> {
        let master = if self.encrypted() {
            let pw = passphrase.ok_or(WalletDatError::PassphraseRequired)?;
            Some(self.legacy_master(pw)?)
        } else {
            None
        };
        if let Some(m) = &master {
            self.verify_crypted_keys(m)?;
        }
        let Some(chain) = &self.legacy else {
            return Err(WalletDatError::NoHdChain);
        };
        let OpenedChain {
            seed,
            mnemonic,
            pass,
        } = open_chain(chain, master.as_deref())?;
        let has_mnemonic = !mnemonic.is_empty();
        if has_mnemonic {
            let derived = crate::bip39core::core_seed(&mnemonic, &pass);
            if derived.as_slice() != seed.as_slice() {
                return Err(corrupt("mnemonic does not derive the HD seed"));
            }
        } else if seed.len() != 64 {
            return Err(WalletDatError::SeedLength(seed.len()));
        }
        let (master_secret, master_pubkey) = bip32_master(&seed)?;
        let hint = match self.address_mainnet {
            Some(true) => "xpub",
            Some(false) => "tpub",
            None => "unknown",
        };
        Ok(HdRoot {
            mnemonic: has_mnemonic.then_some((mnemonic, pass)),
            master_secret,
            master_pubkey,
            external: PkhDescriptor {
                key: Zeroizing::new(hint.to_owned()),
                path: "44h/0h/0h/0".into(),
            },
            hd_seed: (!has_mnemonic).then_some(seed),
        })
    }

    /// The master key that decrypts the crypted chain, or a `ckey` when
    /// there is no chain.
    fn legacy_master(&self, passphrase: &[u8]) -> Result<Zeroizing<[u8; 32]>, WalletDatError> {
        if self.master_keys.is_empty() {
            return Err(corrupt("encrypted wallet without a master key record"));
        }
        for mk in &self.master_keys {
            let Some(master) = crypter::decrypt_master_key(mk, passphrase) else {
                continue;
            };
            if self.master_opens_legacy(&master) {
                return Ok(master);
            }
        }
        Err(WalletDatError::WrongPassphrase)
    }

    fn master_opens_legacy(&self, master: &[u8; 32]) -> bool {
        if let Some(chain) = &self.legacy
            && chain.crypted
        {
            let Some(seed) = crypter::aes_cbc_decrypt(master, &chain_iv(&chain.id), &chain.seed)
            else {
                return false;
            };
            return sha256d(&seed) == chain.id;
        }
        if let Some(key) = self.crypted_keys.first() {
            return crypter::decrypt_secret(master, &key.secret, &key.pubkey)
                .and_then(|plain| crypter::secret32(&plain))
                .is_some_and(|secret| pubkey_matches(&secret, &key.pubkey));
        }
        true
    }

    fn verify_crypted_keys(&self, master: &[u8; 32]) -> Result<(), WalletDatError> {
        for key in &self.crypted_keys {
            let Some(plain) = crypter::decrypt_secret(master, &key.secret, &key.pubkey) else {
                return Err(if self.legacy.as_ref().is_some_and(|c| c.crypted) {
                    corrupt("encrypted key")
                } else {
                    WalletDatError::WrongPassphrase
                });
            };
            let Some(secret) = crypter::secret32(&plain) else {
                return Err(corrupt("encrypted key"));
            };
            if !pubkey_matches(&secret, &key.pubkey) {
                return Err(corrupt("encrypted key does not match its public key"));
            }
        }
        Ok(())
    }

    /// Decrypts the master key with `passphrase`. Core keeps one `mkey`
    /// record per passphrase change history slot; any that decrypts a key
    /// matching `probe` is the right one.
    fn master_key(
        &self,
        passphrase: &[u8],
        probe: &KeyRecord,
    ) -> Result<Zeroizing<[u8; 32]>, WalletDatError> {
        if self.master_keys.is_empty() {
            return Err(corrupt("encrypted keys without a master key record"));
        }
        for mk in &self.master_keys {
            let Some(master) = crypter::decrypt_master_key(mk, passphrase) else {
                continue;
            };
            let opens = crypter::decrypt_secret(&master, &probe.secret, &probe.pubkey)
                .and_then(|s| crypter::secret32(&s))
                .and_then(|s| pubkey_of(&s))
                .is_some_and(|pk| pk[..] == probe.pubkey[..]);
            if opens {
                return Ok(master);
            }
        }
        Err(WalletDatError::WrongPassphrase)
    }
}

fn chain_iv(id: &[u8; 32]) -> [u8; 16] {
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&id[..16]);
    iv
}

fn sha256d(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(Sha256::digest(data));
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    out
}

struct OpenedChain {
    seed: Zeroizing<Vec<u8>>,
    mnemonic: Zeroizing<Vec<u8>>,
    pass: Zeroizing<Vec<u8>>,
}

fn open_chain(
    chain: &LegacyChain,
    master: Option<&[u8; 32]>,
) -> Result<OpenedChain, WalletDatError> {
    let iv = chain_iv(&chain.id);
    let seed = if chain.crypted {
        let master = master.ok_or_else(|| corrupt("encrypted HD chain without a master key"))?;
        crypter::aes_cbc_decrypt(master, &iv, &chain.seed).ok_or(WalletDatError::WrongPassphrase)?
    } else {
        chain.seed.clone()
    };
    if sha256d(&seed) != chain.id {
        return Err(if chain.crypted {
            WalletDatError::WrongPassphrase
        } else {
            corrupt("HD seed does not match its chain id")
        });
    }
    let mnemonic = open_field(chain.crypted, master, &iv, &chain.mnemonic)?;
    let pass = open_field(chain.crypted, master, &iv, &chain.mnemonic_passphrase)?;
    Ok(OpenedChain {
        seed,
        mnemonic,
        pass,
    })
}

fn open_field(
    crypted: bool,
    master: Option<&[u8; 32]>,
    iv: &[u8; 16],
    field: &Zeroizing<Vec<u8>>,
) -> Result<Zeroizing<Vec<u8>>, WalletDatError> {
    if field.is_empty() || !crypted {
        return Ok(field.clone());
    }
    let master = master.ok_or_else(|| corrupt("encrypted HD chain without a master key"))?;
    crypter::aes_cbc_decrypt(master, iv, field).ok_or(WalletDatError::WrongPassphrase)
}

fn bip32_master(seed: &[u8]) -> Result<(Zeroizing<[u8; 32]>, [u8; 33]), WalletDatError> {
    let mut master =
        key_wallet::bip32::ExtendedPrivKey::new_master(dashcore::Network::Mainnet, seed)
            .map_err(|_| corrupt("HD seed is not a BIP32 seed"))?;
    let pubkey = master.private_key.public_key().serialize();
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&master.private_key.to_secret_bytes());
    master.private_key.non_secure_erase();
    Ok((secret, pubkey))
}

fn pubkey_matches(secret: &[u8; 32], pubkey: &[u8]) -> bool {
    let Some(sk) = SecretKey::from_secret_bytes(*secret).ok() else {
        return false;
    };
    let pk = PublicKey::from_secret_key(&sk);
    pk.serialize().as_slice() == pubkey || pk.serialize_uncompressed().as_slice() == pubkey
}

fn pubkey_of(secret: &[u8; 32]) -> Option<[u8; 33]> {
    let sk = SecretKey::from_secret_bytes(*secret).ok()?;
    Some(PublicKey::from_secret_key(&sk).serialize())
}

/// `file://` URI of `path` for SQLite (review L5): the absolute,
/// canonical path with every byte outside the URI path characters
/// percent-encoded, so `?`, `#`, `%` and a leading `//` (which would become
/// a URI authority) stay part of the path. Opened `mode=ro` and
/// `immutable=1`: nothing in the user's file or directory is written.
fn sqlite_uri(path: &Path) -> Result<String, WalletDatError> {
    // The drive colon stays as is on Windows, so SQLite sees `/C:` and drops the `/`.
    const KEEP: &[u8] = if cfg!(windows) { b"/-._~:" } else { b"/-._~" };
    let abs = std::fs::canonicalize(path).map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    #[cfg(unix)]
    let bytes = {
        use std::os::unix::ffi::OsStrExt;
        abs.as_os_str().as_bytes().to_vec()
    };
    #[cfg(windows)]
    let bytes = windows_uri_path(
        abs.to_str()
            .ok_or_else(|| WalletDatError::Unreadable("path is not valid Unicode".into()))?,
    )
    .into_bytes();
    let mut uri = String::from("file://");
    for b in bytes {
        if b.is_ascii_alphanumeric() || KEEP.contains(&b) {
            uri.push(char::from(b));
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    Ok(uri)
}

/// The URI path (UTF-8, `/` separators) of a canonical Windows path:
/// `\\?\C:\dir\file` becomes `/C:/dir/file`, and the network path
/// `\\?\UNC\server\share\file` becomes `//server/share/file`, which Win32
/// opens after `file://`'s empty authority.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_uri_path(canonical: &str) -> String {
    let path = match canonical.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => canonical
            .strip_prefix(r"\\?\")
            .unwrap_or(canonical)
            .to_owned(),
    }
    .replace('\\', "/");
    if path.starts_with("//") {
        path
    } else {
        format!("/{path}")
    }
}

/// A wallet whose last writes are still in a `-wal` (or a hot rollback
/// `-journal`) next to it: Dash Core is running or did not shut down
/// cleanly. Read immutable, the file alone would be stale or torn, so the
/// import is refused with the reason (review L5).
fn pending_log(path: &Path) -> Option<PathBuf> {
    ["-wal", "-journal"].into_iter().find_map(|suffix| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        let log = PathBuf::from(name);
        std::fs::metadata(&log)
            .is_ok_and(|m| m.len() > 0)
            .then_some(log)
    })
}

/// Opens `path` read-only and immutable and reads the records an import
/// needs. Fails with `Unsupported` for a SQLite file that is not a wallet,
/// and with `Unreadable` while a non-empty `-wal` or `-journal` file sits
/// next to it (close Dash Core first).
pub fn read_sqlite(path: &Path) -> Result<SqliteWallet, WalletDatError> {
    if let Some(log) = pending_log(path) {
        return Err(WalletDatError::Unreadable(format!(
            "{} holds changes not yet written to the wallet file; close Dash Core (or let it shut \
             down cleanly) and import again",
            log.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        )));
    }
    let uri = sqlite_uri(path)?;
    let conn = rusqlite::Connection::open_with_flags(
        uri,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    let mut stmt = conn
        .prepare("SELECT key, value FROM main")
        .map_err(|e| WalletDatError::Unsupported(format!("no wallet table: {e}")))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    let mut wallet = empty_wallet();
    let mut gather = Gather::default();
    for row in rows {
        let (key, value) = row.map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
        let value = Zeroizing::new(value);
        parse_record(&key, &value, &mut wallet, &mut gather)?;
    }
    finish(&mut wallet, gather)?;
    Ok(wallet)
}

/// Reads a legacy Berkeley DB `wallet.dat` (or a descriptor wallet stored in
/// one) into the same records [`read_sqlite`] produces.
pub fn read_bdb(path: &Path) -> Result<SqliteWallet, WalletDatError> {
    let meta = std::fs::metadata(path).map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    if !meta.is_file() {
        return Err(WalletDatError::Unreadable(format!(
            "{} is not a file",
            path.display()
        )));
    }
    if meta.len() > crate::bdb::MAX_FILE as u64 {
        return Err(WalletDatError::Unreadable(format!(
            "{} is over {} bytes",
            path.display(),
            crate::bdb::MAX_FILE
        )));
    }
    let bytes = std::fs::read(path).map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    read_bdb_bytes(&bytes)
}

/// [`read_bdb`] over bytes already in memory.
pub fn read_bdb_bytes(bytes: &[u8]) -> Result<SqliteWallet, WalletDatError> {
    let pairs = crate::bdb::read_pairs(bytes)?;
    let mut wallet = empty_wallet();
    let mut gather = Gather::default();
    for (key, value) in pairs {
        parse_record(&key, &value, &mut wallet, &mut gather)?;
    }
    finish(&mut wallet, gather)?;
    Ok(wallet)
}

fn empty_wallet() -> SqliteWallet {
    SqliteWallet {
        flags: 0,
        descriptors: Vec::new(),
        address_book: Vec::new(),
        active_external: None,
        active_internal: None,
        coinjoin_salt: None,
        master_keys: Vec::new(),
        keys: Vec::new(),
        legacy: None,
        crypted_keys: Vec::new(),
        loose_keys: 0,
        scripts: 0,
        address_mainnet: None,
        earliest_key_time: None,
    }
}

fn finish(w: &mut SqliteWallet, gather: Gather) -> Result<(), WalletDatError> {
    let Gather {
        names,
        mut purposes,
        hd_pubs,
        key_pubs,
    } = gather;
    for (address, label) in names {
        note_network(w, &address)?;
        let purpose = purposes.remove(&address);
        w.address_book.push(AddressBookEntry {
            address,
            label,
            purpose,
        });
    }
    w.loose_keys = if hd_pubs.is_empty() {
        if w.legacy.is_some() {
            0
        } else {
            key_pubs.len() as u32
        }
    } else {
        key_pubs.iter().filter(|p| !hd_pubs.contains(*p)).count() as u32
    };
    Ok(())
}

fn note_network(w: &mut SqliteWallet, address: &str) -> Result<(), WalletDatError> {
    let Some(mainnet) = address_mainnet(address) else {
        return Ok(());
    };
    match w.address_mainnet {
        None => w.address_mainnet = Some(mainnet),
        Some(prev) if prev != mainnet => {
            return Err(corrupt("address book mixes mainnet and testnet addresses"));
        }
        Some(_) => {}
    }
    Ok(())
}

fn address_mainnet(address: &str) -> Option<bool> {
    use dashcore::address::NetworkUnchecked;
    let parsed = dashcore::Address::<NetworkUnchecked>::from_str(address).ok()?;
    let main = parsed.is_valid_for_network(dashcore::Network::Mainnet);
    let test = parsed.is_valid_for_network(dashcore::Network::Testnet);
    match (main, test) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

fn utf8(b: &[u8], what: &str) -> Result<String, WalletDatError> {
    String::from_utf8(b.to_vec()).map_err(|_| corrupt(format!("{what} is not UTF-8")))
}

fn parse_record(
    key: &[u8],
    value: &[u8],
    w: &mut SqliteWallet,
    g: &mut Gather,
) -> Result<(), WalletDatError> {
    let mut k = Reader::new(key);
    let Some(kind) = k.bytes() else {
        return Ok(());
    };
    let mut v = Reader::new(value);
    let bad = |what: &str| corrupt(format!("{what} record"));
    match kind {
        b"flags" => w.flags = v.u64().ok_or_else(|| bad("flags"))?,
        b"walletdescriptor" => {
            let id = k.hash256().ok_or_else(|| bad("walletdescriptor"))?;
            let descriptor = utf8(
                v.bytes().ok_or_else(|| bad("walletdescriptor"))?,
                "descriptor",
            )?;
            let creation_time = v.u64().ok_or_else(|| bad("walletdescriptor"))?;
            let next_index = v.i32().ok_or_else(|| bad("walletdescriptor"))?;
            let range_start = v.i32().ok_or_else(|| bad("walletdescriptor"))?;
            let range_end = v.i32().ok_or_else(|| bad("walletdescriptor"))?;
            w.descriptors.push(DescriptorRecord {
                id,
                descriptor,
                creation_time,
                range_start,
                range_end,
                next_index,
            });
        }
        b"activeexternalspk" | b"activeinternalspk" => {
            let id = v.hash256().ok_or_else(|| bad("active spk"))?;
            if kind == b"activeexternalspk" {
                w.active_external = Some(id);
            } else {
                w.active_internal = Some(id);
            }
        }
        b"walletdescriptorkey" | b"walletdescriptorckey" => {
            let encrypted = kind == b"walletdescriptorckey";
            let descriptor_id = k.hash256().ok_or_else(|| bad("descriptor key"))?;
            let pubkey = k.bytes().ok_or_else(|| bad("descriptor key"))?.to_vec();
            let secret = Zeroizing::new(v.bytes().ok_or_else(|| bad("descriptor key"))?.to_vec());
            if !encrypted {
                // Hash(pubkey ‖ privkey), checked by Core on load.
                let _hash = v.hash256().ok_or_else(|| bad("descriptor key"))?;
            }
            // Dash appends the mnemonic and its passphrase; older or imported
            // keys have neither.
            let (mnemonic, mnemonic_passphrase) = if v.is_empty() {
                (Vec::new(), Vec::new())
            } else {
                let m = v.bytes().ok_or_else(|| bad("descriptor key mnemonic"))?;
                let p = v.bytes().ok_or_else(|| bad("descriptor key mnemonic"))?;
                (m.to_vec(), p.to_vec())
            };
            w.keys.push(KeyRecord {
                descriptor_id,
                pubkey,
                secret,
                mnemonic: Zeroizing::new(mnemonic),
                mnemonic_passphrase: Zeroizing::new(mnemonic_passphrase),
                encrypted,
            });
        }
        b"mkey" => {
            let _id = k.u32().ok_or_else(|| bad("mkey"))?;
            let crypted_key = v.bytes().ok_or_else(|| bad("mkey"))?.to_vec();
            let salt = v.bytes().ok_or_else(|| bad("mkey"))?.to_vec();
            let derivation_method = v.u32().ok_or_else(|| bad("mkey"))?;
            let rounds = v.u32().ok_or_else(|| bad("mkey"))?;
            if rounds > crypter::MAX_ROUNDS {
                return Err(corrupt(format!(
                    "mkey record asks for {rounds} key derivation rounds (at most {})",
                    crypter::MAX_ROUNDS
                )));
            }
            let other_params = v.bytes().ok_or_else(|| bad("mkey"))?.to_vec();
            w.master_keys.push(MasterKeyRecord {
                crypted_key,
                salt,
                derivation_method,
                rounds,
                other_params,
            });
        }
        b"cj_salt" => w.coinjoin_salt = Some(v.hash256().ok_or_else(|| bad("cj_salt"))?),
        b"ps_salt" if w.coinjoin_salt.is_none() => {
            w.coinjoin_salt = Some(v.hash256().ok_or_else(|| bad("ps_salt"))?);
        }
        b"name" | b"purpose" => {
            let address = utf8(k.bytes().ok_or_else(|| bad("address book"))?, "address")?;
            let text =
                String::from_utf8_lossy(v.bytes().ok_or_else(|| bad("address book"))?).into_owned();
            if kind == b"name" {
                g.names.insert(address, text);
            } else {
                g.purposes.insert(address, text);
            }
        }
        b"hdchain" | b"chdchain" => {
            let chain = parse_hdchain(&mut v)?;
            if chain.crypted != (kind == b"chdchain") {
                return Err(corrupt("HD chain type mismatch"));
            }
            w.legacy = Some(chain);
        }
        b"key" | b"ckey" => {
            let pubkey = k.bytes().ok_or_else(|| bad("key"))?.to_vec();
            let secret = Zeroizing::new(v.bytes().ok_or_else(|| bad("key"))?.to_vec());
            if !v.is_empty() {
                let checksum = v.hash256().ok_or_else(|| bad("key"))?;
                let hashed = if kind == b"ckey" {
                    sha256d(&secret)
                } else {
                    let mut buf = Vec::with_capacity(pubkey.len() + secret.len());
                    buf.extend_from_slice(&pubkey);
                    buf.extend_from_slice(&secret);
                    sha256d(&buf)
                };
                if hashed != checksum {
                    return Err(corrupt("key checksum"));
                }
            }
            g.key_pubs.push(pubkey.clone());
            if kind == b"ckey" {
                w.crypted_keys.push(CryptedKey { pubkey, secret });
            } else if !plain_key_matches(&pubkey, &secret) {
                return Err(corrupt("private key does not match its public key"));
            }
        }
        b"wkey" => {
            let pubkey = k.bytes().ok_or_else(|| bad("wkey"))?.to_vec();
            let _version = v.i32().ok_or_else(|| bad("wkey"))?;
            let secret = v.bytes().ok_or_else(|| bad("wkey"))?;
            let _created = v.i64().ok_or_else(|| bad("wkey"))?;
            let _expires = v.i64().ok_or_else(|| bad("wkey"))?;
            let _comment = v.bytes().ok_or_else(|| bad("wkey"))?;
            if !plain_key_matches(&pubkey, secret) {
                return Err(corrupt("private key does not match its public key"));
            }
            g.key_pubs.push(pubkey);
        }
        b"keymeta" => {
            let _pubkey = k.bytes().ok_or_else(|| bad("keymeta"))?;
            let version = v.i32().ok_or_else(|| bad("keymeta"))?;
            let created = v.i64().ok_or_else(|| bad("keymeta"))?;
            if version >= 12 {
                let _fingerprint = v.take(4).ok_or_else(|| bad("keymeta"))?;
                let steps = v.compact_size().ok_or_else(|| bad("keymeta"))?;
                if steps > 16 {
                    return Err(corrupt("keymeta path"));
                }
                for _ in 0..steps {
                    v.u32().ok_or_else(|| bad("keymeta"))?;
                }
                v.bool().ok_or_else(|| bad("keymeta"))?;
            }
            if created > 0 {
                w.earliest_key_time = Some(
                    w.earliest_key_time
                        .map_or(created, |prev| prev.min(created)),
                );
            }
        }
        b"hdpubkey" => {
            let pubkey = k.bytes().ok_or_else(|| bad("hdpubkey"))?.to_vec();
            let stored = parse_hd_pubkey(&mut v)?;
            if stored != pubkey {
                return Err(corrupt("hdpubkey does not match its key"));
            }
            g.hd_pubs.insert(pubkey);
        }
        b"cscript" => {
            v.bytes().ok_or_else(|| bad("script"))?;
            w.scripts = w.scripts.saturating_add(1);
        }
        // `watchs` value is a single byte, not a string.
        b"watchs" => {
            if v.is_empty() {
                return Err(bad("watchs"));
            }
            w.scripts = w.scripts.saturating_add(1);
        }
        _ => {}
    }
    Ok(())
}

const MAX_CHAIN_FIELD: usize = 4096;

fn parse_hdchain(v: &mut Reader<'_>) -> Result<LegacyChain, WalletDatError> {
    let _version = v.i32().ok_or_else(|| corrupt("hdchain record"))?;
    let id = v.hash256().ok_or_else(|| corrupt("hdchain record"))?;
    let crypted = v.bool().ok_or_else(|| corrupt("hdchain record"))?;
    let seed = bounded(v, "hdchain seed")?;
    let mnemonic = bounded(v, "hdchain mnemonic")?;
    let mnemonic_passphrase = bounded(v, "hdchain mnemonic passphrase")?;
    let n = v.compact_size().ok_or_else(|| corrupt("hdchain record"))?;
    if n > 64 {
        return Err(corrupt("hdchain accounts"));
    }
    let mut accounts = BTreeMap::new();
    for _ in 0..n {
        let index = v.u32().ok_or_else(|| corrupt("hdchain record"))?;
        let external = v.u32().ok_or_else(|| corrupt("hdchain record"))?;
        let internal = v.u32().ok_or_else(|| corrupt("hdchain record"))?;
        accounts.insert(index, (external, internal));
    }
    Ok(LegacyChain {
        id,
        crypted,
        seed: Zeroizing::new(seed),
        mnemonic: Zeroizing::new(mnemonic),
        mnemonic_passphrase: Zeroizing::new(mnemonic_passphrase),
        accounts,
    })
}

fn bounded(v: &mut Reader<'_>, what: &str) -> Result<Vec<u8>, WalletDatError> {
    let bytes = v.bytes().ok_or_else(|| corrupt(format!("{what} record")))?;
    if bytes.len() > MAX_CHAIN_FIELD {
        return Err(corrupt(format!("{what} is too long")));
    }
    Ok(bytes.to_vec())
}

fn parse_hd_pubkey(v: &mut Reader<'_>) -> Result<Vec<u8>, WalletDatError> {
    v.i32().ok_or_else(|| corrupt("hdpubkey record"))?;
    v.take(1).ok_or_else(|| corrupt("hdpubkey record"))?;
    v.take(4).ok_or_else(|| corrupt("hdpubkey record"))?;
    v.u32().ok_or_else(|| corrupt("hdpubkey record"))?;
    v.take(32).ok_or_else(|| corrupt("hdpubkey record"))?;
    Ok(v.bytes()
        .ok_or_else(|| corrupt("hdpubkey record"))?
        .to_vec())
}

fn plain_key_matches(pubkey: &[u8], der: &[u8]) -> bool {
    der_secret(der).is_some_and(|secret| pubkey_matches(&secret, pubkey))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/compat/walletdat")
            .join(name)
    }

    #[test]
    fn sniffs_containers() {
        let bdb = std::fs::read(vector("bdb_legacy_head.dat")).unwrap();
        assert_eq!(sniff_container(&bdb), Some(Container::BerkeleyDb));
        let sqlite = std::fs::read(vector("desc_plain.dat")).unwrap();
        assert_eq!(sniff_container(&sqlite), Some(Container::Sqlite));
        assert_eq!(sniff_container(b"# Wallet dump created by"), None);
        assert_eq!(sniff_container(&[]), None);
    }

    #[test]
    fn der_secret_round_trip() {
        // A compressed CPrivKey as Core's ec_seckey_export_der lays it out:
        // only the header and the octet string matter to the importer.
        let mut der = vec![0x30, 0x81, 0xd3, 0x02, 0x01, 0x01, 0x04, 0x20];
        der.extend_from_slice(&[0x11; 32]);
        der.resize(3 + 0xd3, 0);
        assert_eq!(*der_secret(&der).unwrap(), [0x11; 32]);
        assert!(der_secret(&der[..20]).is_none());
        let mut zero = der.clone();
        zero[8..40].fill(0);
        assert!(der_secret(&zero).is_none());
    }

    #[test]
    fn reads_plain_descriptor_wallet() {
        let w = read_sqlite(&vector("desc_plain.dat")).unwrap();
        assert!(w.is_descriptor_wallet());
        assert!(!w.encrypted());
        assert!(w.has_mnemonic());
        assert_eq!(w.descriptors.len(), 3);
        let root = w.hd_root(None).unwrap();
        let (m, p) = root.mnemonic.as_ref().unwrap();
        assert!(m.starts_with(b"abandon abandon"));
        assert_eq!(&p[..], b"TREZOR");
        assert_eq!(root.external.path, "44h/1h/0h/0");
        assert!(w.address_book.iter().any(|e| e.label == "savings"));
    }

    #[test]
    fn reads_encrypted_descriptor_wallet() {
        let w = read_sqlite(&vector("desc_encrypted.dat")).unwrap();
        assert!(w.encrypted());
        assert!(w.has_mnemonic());
        assert_eq!(
            w.hd_root(None).unwrap_err(),
            WalletDatError::PassphraseRequired
        );
        assert_eq!(
            w.hd_root(Some(b"wrong")).unwrap_err(),
            WalletDatError::WrongPassphrase
        );
        let root = w.hd_root(Some("wallet pass é 🔐".as_bytes())).unwrap();
        let (m, p) = root.mnemonic.as_ref().unwrap();
        assert!(m.starts_with(b"letter advice"));
        assert_eq!(&p[..], "pässwörd ñ \u{20ac} \u{1F511}".as_bytes());
    }

    #[test]
    fn reads_descriptor_wallet_without_mnemonic() {
        let w = read_sqlite(&vector("desc_nomnemonic.dat")).unwrap();
        assert!(!w.has_mnemonic());
        let root = w.hd_root(None).unwrap();
        assert!(root.mnemonic.is_none());
    }

    /// Review M1: an `mkey` record asking for more than 10^7 rounds is
    /// corrupt instead of hanging the import.
    #[test]
    fn crafted_mkey_rounds_are_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.dat");
        std::fs::copy(vector("desc_encrypted.dat"), &path).unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let mut stmt = conn.prepare("SELECT key, value FROM main").unwrap();
            let rows: Vec<(Vec<u8>, Vec<u8>)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let (key, mut value) = rows
                .into_iter()
                .find(|(k, _)| Reader::new(k).bytes() == Some(b"mkey".as_slice()))
                .unwrap();
            let mut r = Reader::new(&value);
            r.bytes().unwrap();
            r.bytes().unwrap();
            r.u32().unwrap();
            let at = value.len() - r.buf.len();
            value[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            conn.execute(
                "UPDATE main SET value = ?1 WHERE key = ?2",
                rusqlite::params![value, key],
            )
            .unwrap();
        }
        assert!(matches!(
            read_sqlite(&path),
            Err(WalletDatError::Corrupt(d)) if d.contains("rounds")
        ));
    }

    #[test]
    fn windows_paths_become_sqlite_uri_paths() {
        assert_eq!(
            windows_uri_path(r"\\?\C:\a b\wallet.dat"),
            "/C:/a b/wallet.dat"
        );
        assert_eq!(
            windows_uri_path(r"\\?\UNC\srv\share\x.dat"),
            "//srv/share/x.dat"
        );
        assert_eq!(windows_uri_path(r"D:\Wället\w.dat"), "/D:/Wället/w.dat");
    }

    /// Review L5: odd characters and a leading `//` stay in the path; a
    /// pending `-wal` is reported instead of being read stale.
    #[test]
    fn sqlite_paths_are_uri_safe_and_pending_logs_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        // `?` is not allowed in Windows file names.
        let odd = dir.path().join(if cfg!(windows) {
            "we ird #dir %41"
        } else {
            "we?ird #dir %41"
        });
        std::fs::create_dir(&odd).unwrap();
        let path = odd.join("wallet.dat");
        std::fs::copy(vector("desc_plain.dat"), &path).unwrap();
        assert!(read_sqlite(&path).unwrap().is_descriptor_wallet());
        #[cfg(unix)]
        {
            let slashes = PathBuf::from(format!("/{}", path.display()));
            assert!(slashes.to_string_lossy().starts_with("//"));
            assert!(read_sqlite(&slashes).unwrap().is_descriptor_wallet());
        }
        let uri = sqlite_uri(&path).unwrap();
        assert!(uri.starts_with("file:///"), "{uri}");
        let encoded = if cfg!(windows) {
            "we%20ird%20%23dir%20%2541"
        } else {
            "we%3Fird%20%23dir%20%2541"
        };
        assert!(uri.contains(encoded), "{uri}");

        // An empty -wal is harmless; a non-empty one is reported.
        let wal = odd.join("wallet.dat-wal");
        std::fs::write(&wal, b"").unwrap();
        assert!(read_sqlite(&path).is_ok());
        std::fs::write(&wal, [1u8; 32]).unwrap();
        match read_sqlite(&path) {
            Err(WalletDatError::Unreadable(d)) => assert!(d.contains("wallet.dat-wal"), "{d}"),
            other => panic!("expected the -wal to be reported, got {other:?}"),
        }
    }

    use crate::bdb::build;
    use crate::bdb::{TYPE_INTERNAL, TYPE_LEAF};
    use crate::bip39core::core_seed;
    use dashcore::{Address, Network, PublicKey};
    use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};

    const PHRASE: &str = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
    const MNEMONIC_PASS: &str = "TREZOR";

    fn ser_compact(buf: &mut Vec<u8>, n: usize) {
        if n < 253 {
            buf.push(n as u8);
        } else {
            buf.push(253);
            buf.extend_from_slice(&(n as u16).to_le_bytes());
        }
    }

    fn ser_bytes(buf: &mut Vec<u8>, bytes: &[u8]) {
        ser_compact(buf, bytes.len());
        buf.extend_from_slice(bytes);
    }

    fn db_key(typ: &str) -> Vec<u8> {
        let mut key = Vec::new();
        ser_bytes(&mut key, typ.as_bytes());
        key
    }

    fn db_key_pub(typ: &str, pubkey: &[u8]) -> Vec<u8> {
        let mut key = db_key(typ);
        ser_bytes(&mut key, pubkey);
        key
    }

    fn der_of(secret: &[u8; 32]) -> Vec<u8> {
        let mut der = vec![0x30, 0x81, 0xd3, 0x02, 0x01, 0x01, 0x04, 0x20];
        der.extend_from_slice(secret);
        der.resize(3 + 0xd3, 0);
        der
    }

    fn compressed(secret: &[u8; 32]) -> [u8; 33] {
        let sk = dashcore::secp256k1::SecretKey::from_secret_bytes(*secret).unwrap();
        dashcore::secp256k1::PublicKey::from_secret_key(&sk).serialize()
    }

    fn ser_chain(
        id: &[u8; 32],
        crypted: bool,
        seed: &[u8],
        mnemonic: &[u8],
        pass: &[u8],
        external: u32,
        internal: u32,
    ) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&1i32.to_le_bytes());
        value.extend_from_slice(id);
        value.push(u8::from(crypted));
        ser_bytes(&mut value, seed);
        ser_bytes(&mut value, mnemonic);
        ser_bytes(&mut value, pass);
        ser_compact(&mut value, 1);
        value.extend_from_slice(&0u32.to_le_bytes());
        value.extend_from_slice(&external.to_le_bytes());
        value.extend_from_slice(&internal.to_le_bytes());
        value
    }

    fn ser_hdpubkey(pubkey: &[u8]) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&1i32.to_le_bytes());
        value.push(0);
        value.extend_from_slice(&[0; 4]);
        value.extend_from_slice(&0u32.to_le_bytes());
        value.extend_from_slice(&[0; 32]);
        ser_bytes(&mut value, pubkey);
        value
    }

    fn receive_address(seed: &[u8]) -> String {
        let master = ExtendedPrivKey::new_master(Network::Regtest, seed).unwrap();
        let path = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
        let child = master.derive_priv(&path).unwrap();
        let pk = PublicKey::new(child.private_key.public_key());
        Address::p2pkh(&pk, Network::Regtest).to_string()
    }

    struct Built {
        bytes: Vec<u8>,
        seed: Vec<u8>,
        address: String,
    }

    fn plain_hd_wallet(swap: bool) -> Built {
        let seed = core_seed(PHRASE.as_bytes(), MNEMONIC_PASS.as_bytes());
        let id = sha256d(&seed[..]);
        let address = receive_address(&seed[..]);
        let child_secret = {
            let master = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
            let path = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
            master
                .derive_priv(&path)
                .unwrap()
                .private_key
                .to_secret_bytes()
        };
        let child_pub = compressed(&child_secret);
        let loose = [0x22u8; 32];
        let loose_pub = compressed(&loose);
        let wkey_secret = [0x33u8; 32];
        let wkey_pub = compressed(&wkey_secret);
        let mut key_value = Vec::new();
        ser_bytes(&mut key_value, &der_of(&child_secret));
        let mut hashed = Vec::new();
        hashed.extend_from_slice(&child_pub);
        hashed.extend_from_slice(&der_of(&child_secret));
        key_value.extend_from_slice(&sha256d(&hashed));
        let mut loose_value = Vec::new();
        ser_bytes(&mut loose_value, &der_of(&loose));
        let mut wkey_value = Vec::new();
        wkey_value.extend_from_slice(&1i32.to_le_bytes());
        ser_bytes(&mut wkey_value, &der_of(&wkey_secret));
        wkey_value.extend_from_slice(&1_700_000_000i64.to_le_bytes());
        wkey_value.extend_from_slice(&0i64.to_le_bytes());
        ser_bytes(&mut wkey_value, b"");
        let mut meta = Vec::new();
        meta.extend_from_slice(&1i32.to_le_bytes());
        meta.extend_from_slice(&1_600_000_000i64.to_le_bytes());
        let mut script = Vec::new();
        ser_bytes(&mut script, &[0x51]);
        let mut name_key = db_key("name");
        ser_bytes(&mut name_key, address.as_bytes());
        let mut name_value = Vec::new();
        ser_bytes(&mut name_value, b"savings");
        let chain = ser_chain(
            &id,
            false,
            &seed[..],
            PHRASE.as_bytes(),
            MNEMONIC_PASS.as_bytes(),
            2,
            1,
        );
        let pairs = [
            (db_key("hdchain"), chain),
            (db_key_pub("key", &child_pub), key_value),
            (db_key_pub("hdpubkey", &child_pub), ser_hdpubkey(&child_pub)),
            (db_key_pub("keymeta", &child_pub), meta),
            (db_key_pub("key", &loose_pub), loose_value),
            (db_key_pub("wkey", &wkey_pub), wkey_value),
            (db_key("cscript"), script),
            (name_key, name_value),
            (db_key("cj_salt"), [0xab; 32].to_vec()),
            (db_key("watchs"), vec![b'1']),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        Built {
            bytes: build::pairs(&refs, swap),
            seed: seed.to_vec(),
            address,
        }
    }

    fn encrypted_hd_wallet() -> Vec<u8> {
        let seed = core_seed(PHRASE.as_bytes(), b"second");
        let id = sha256d(&seed[..]);
        let salt = [7u8; 8];
        let (pass_key, pass_iv) = crypter::key_from_passphrase(b"bdb-pass", &salt, 2).unwrap();
        let master = [0x42u8; 32];
        let crypted_master = crypter::aes_cbc_encrypt(&pass_key, &pass_iv, &master);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&id[..16]);
        let enc = |plain: &[u8]| crypter::aes_cbc_encrypt(&master, &iv, plain);
        let chain = ser_chain(
            &id,
            true,
            &enc(&seed[..]),
            &enc(PHRASE.as_bytes()),
            &enc(b"second"),
            3,
            0,
        );
        let child_secret = {
            let xprv = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
            xprv.derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .to_secret_bytes()
        };
        let child_pub = compressed(&child_secret);
        let ct = crypter::aes_cbc_encrypt(&master, &crypter::pubkey_iv(&child_pub), &child_secret);
        let mut ckey = Vec::new();
        ser_bytes(&mut ckey, &ct);
        ckey.extend_from_slice(&sha256d(&ct));
        let mut mkey = Vec::new();
        ser_bytes(&mut mkey, &crypted_master);
        ser_bytes(&mut mkey, &salt);
        mkey.extend_from_slice(&0u32.to_le_bytes());
        mkey.extend_from_slice(&2u32.to_le_bytes());
        ser_bytes(&mut mkey, b"");
        let mut mkey_key = db_key("mkey");
        mkey_key.extend_from_slice(&1u32.to_le_bytes());
        let pairs = [
            (db_key("chdchain"), chain),
            (mkey_key, mkey),
            (db_key_pub("ckey", &child_pub), ckey),
            (db_key_pub("hdpubkey", &child_pub), ser_hdpubkey(&child_pub)),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        build::pairs(&refs, false)
    }

    #[test]
    fn reads_a_plain_legacy_hd_wallet() {
        for swap in [false, true] {
            let built = plain_hd_wallet(swap);
            let wallet = read_bdb_bytes(&built.bytes).unwrap();
            assert!(!wallet.encrypted(), "swap {swap}");
            assert!(!wallet.is_descriptor_wallet());
            assert_eq!(wallet.next_index(), 2);
            assert_eq!(wallet.loose_keys, 2);
            assert_eq!(wallet.scripts, 2);
            assert_eq!(wallet.coinjoin_salt, Some([0xab; 32]));
            assert_eq!(wallet.address_mainnet, Some(false));
            assert_eq!(wallet.earliest_key_time, Some(1_600_000_000));
            assert!(
                wallet
                    .address_book
                    .iter()
                    .any(|e| e.label == "savings" && e.address == built.address)
            );
            let root = wallet.hd_root(None).unwrap();
            let (phrase, pass) = root.mnemonic.unwrap();
            assert_eq!(&phrase[..], PHRASE.as_bytes());
            assert_eq!(&pass[..], MNEMONIC_PASS.as_bytes());
            assert!(root.hd_seed.is_none());
            let expect = ExtendedPrivKey::new_master(Network::Regtest, &built.seed).unwrap();
            assert_eq!(
                root.master_pubkey,
                expect.private_key.public_key().serialize()
            );
            assert_eq!(root.external.key.as_str(), "tpub");
        }
    }

    #[test]
    fn reads_an_encrypted_legacy_hd_wallet() {
        let bytes = encrypted_hd_wallet();
        let wallet = read_bdb_bytes(&bytes).unwrap();
        assert!(wallet.encrypted());
        assert_eq!(
            wallet.hd_root(None).unwrap_err(),
            WalletDatError::PassphraseRequired
        );
        assert_eq!(
            wallet.hd_root(Some(b"nope")).unwrap_err(),
            WalletDatError::WrongPassphrase
        );
        let root = wallet.hd_root(Some(b"bdb-pass")).unwrap();
        let (phrase, pass) = root.mnemonic.unwrap();
        assert_eq!(&phrase[..], PHRASE.as_bytes());
        assert_eq!(&pass[..], b"second");
        assert_eq!(wallet.next_index(), 3);
    }

    #[test]
    fn a_seed_without_a_mnemonic_is_returned_whole() {
        let seed = [0x5au8; 64];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, false, &seed, b"", b"", 0, 4);
        let pairs = [(db_key("hdchain"), chain)];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        let root = wallet.hd_root(None).unwrap();
        assert!(root.mnemonic.is_none());
        assert_eq!(root.hd_seed.unwrap().as_slice(), &seed);
        assert_eq!(wallet.next_index(), 4);
    }

    #[test]
    fn a_short_seed_without_a_mnemonic_is_refused() {
        let seed = [0x5au8; 32];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, false, &seed, b"", b"", 0, 0);
        let pairs = [(db_key("hdchain"), chain)];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        assert_eq!(
            wallet.hd_root(None).unwrap_err(),
            WalletDatError::SeedLength(32)
        );
    }

    #[test]
    fn keys_without_an_hd_chain_are_not_a_wallet_root() {
        let secret = [0x44u8; 32];
        let pubkey = compressed(&secret);
        let mut value = Vec::new();
        ser_bytes(&mut value, &der_of(&secret));
        let pairs = [(db_key_pub("key", &pubkey), value)];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        assert_eq!(wallet.loose_keys, 1);
        assert_eq!(wallet.hd_root(None).unwrap_err(), WalletDatError::NoHdChain);
    }

    #[test]
    fn hostile_bdb_pages_fail_closed() {
        assert!(read_bdb_bytes(&[]).is_err());
        let head = std::fs::read(vector("bdb_legacy_head.dat")).unwrap();
        assert!(read_bdb_bytes(&head).is_err());
        let internal = build::pack(
            3,
            &[build::child_item(3, false)],
            TYPE_INTERNAL,
            2,
            0,
            false,
        );
        assert!(crate::bdb::read_pairs(&build::assemble(&[internal], false)).is_err());
        let huge = vec![
            build::data_item(b"k", false, false),
            build::overflow_item(4, u32::MAX, false),
        ];
        let leaf = build::pack(3, &huge, TYPE_LEAF, 1, 0, false);
        assert!(crate::bdb::read_pairs(&build::assemble(&[leaf], false)).is_err());
        let payload = b"overflow-payload-0123456789";
        let items = vec![
            build::data_item(b"k", false, false),
            build::overflow_item(4, payload.len() as u32, false),
        ];
        let leaf = build::pack(3, &items, TYPE_LEAF, 1, 0, false);
        let overflow = build::overflow_page(4, payload, 0, false);
        let pairs = crate::bdb::read_pairs(&build::assemble(&[leaf, overflow], false)).unwrap();
        assert_eq!(pairs, vec![(b"k".to_vec(), payload.to_vec())]);
        let deleted = vec![
            build::data_item(b"gone-key", true, false),
            build::data_item(b"gone-val", false, false),
            build::data_item(b"kept-key", false, false),
            build::data_item(b"kept-val", false, false),
        ];
        let leaf = build::pack(3, &deleted, TYPE_LEAF, 1, 0, false);
        let pairs = crate::bdb::read_pairs(&build::assemble(&[leaf], false)).unwrap();
        assert_eq!(pairs, vec![(b"kept-key".to_vec(), b"kept-val".to_vec())]);
        let odd = build::pack(
            3,
            &[build::data_item(b"only", false, false)],
            TYPE_LEAF,
            1,
            0,
            false,
        );
        assert!(crate::bdb::read_pairs(&build::assemble(&[odd], false)).is_err());
    }

    /// The committed fixtures are the wallets the builder writes. dashd is
    /// not on this machine, so they are the documented record layout, not a
    /// dashd file.
    #[test]
    fn committed_bdb_fixtures_match_the_builder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/compat/walletdat");
        let plain = plain_hd_wallet(false);
        let encrypted = encrypted_hd_wallet();
        if std::env::var_os("DW_WRITE_BDB_FIXTURES").is_some() {
            std::fs::write(dir.join("bdb_hd_plain.dat"), &plain.bytes).unwrap();
            std::fs::write(dir.join("bdb_hd_encrypted.dat"), &encrypted).unwrap();
        }
        assert_eq!(
            std::fs::read(dir.join("bdb_hd_plain.dat")).unwrap(),
            plain.bytes
        );
        assert_eq!(
            std::fs::read(dir.join("bdb_hd_encrypted.dat")).unwrap(),
            encrypted
        );
    }
}

#[cfg(test)]
mod bdb_fuzz {
    use super::read_bdb_bytes;

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config { cases: 32, ..proptest::test_runner::Config::default() })]
        #[test]
        fn random_bytes_do_not_panic(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..8192)) {
            let _ = read_bdb_bytes(&data);
        }
    }
}
