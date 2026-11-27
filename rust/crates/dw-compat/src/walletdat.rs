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
use std::io::Read;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use dashcore::ecdsa::{EcdsaPublicKey, EcdsaSecretKey};
use key_wallet::bip32::{ChildNumber, ExtendedPrivKey, ExtendedPubKey};
use zeroize::{Zeroize, Zeroizing};

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
    /// A legacy wallet with more than [`MAX_LEGACY_CHAIN_KEYS`] keys on one
    /// chain: read, and refused on purpose.
    RangeTooLarge,
    /// A legacy wallet with keys on an HD account other than 0.
    ExtraHdAccount(u32),
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
            Self::RangeTooLarge => write!(
                f,
                "more than {MAX_LEGACY_CHAIN_KEYS} keys on one chain cannot be restored"
            ),
            Self::ExtraHdAccount(a) => {
                write!(f, "HD account {a} cannot be restored; only account 0 is")
            }
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
    EcdsaSecretKey::from_bytes(&out).ok()?;
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
struct HdChild {
    account: u32,
    /// 0 external, 1 internal.
    change: u32,
    index: u32,
    pubkey: [u8; 33],
}

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
    /// A `key` or `wkey` record was read.
    plain_key: bool,
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
    /// Plain or encrypted keys that are not listed as `hdpubkey`. An HD
    /// chain does not make those keys derived from it.
    pub loose_keys: u32,
    /// HD public keys, with the account, change chain and address index
    /// Core stored beside them. The network is which BIP44 coin type
    /// derives them.
    hd_children: Vec<HdChild>,
    /// Persistent coin locks (`lockedutxo`), txid in internal byte order.
    pub locked_outpoints: Vec<([u8; 32], u32)>,
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
    /// The wallet the `(key, value)` records of its `main` table describe,
    /// as [`read_sqlite`] and [`read_bdb_bytes`] read them. Each record is
    /// parsed and dropped before the next is read, and the first error,
    /// from the source or from a record, ends the read.
    pub fn from_records<K: AsRef<[u8]>, V: AsRef<[u8]>>(
        records: impl IntoIterator<Item = Result<(K, V), WalletDatError>>,
    ) -> Result<Self, WalletDatError> {
        let mut wallet = empty_wallet();
        let mut gather = Gather::default();
        for record in records {
            let (key, value) = record?;
            parse_record(key.as_ref(), value.as_ref(), &mut wallet, &mut gather)?;
        }
        finish(&mut wallet, gather)?;
        Ok(wallet)
    }

    /// The key derivation rounds a passphrase costs here: every `mkey`
    /// record's together (at most [`crypter::MAX_ROUNDS`]).
    pub fn derivation_rounds(&self) -> u64 {
        self.master_keys.iter().map(|m| u64::from(m.rounds)).sum()
    }

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

    /// Legacy HD accounts as `(account, external counter, internal counter)`.
    /// The counters are how many keys Core derived on each chain, keypool
    /// included, kept apart so a restore does not treat them as one
    /// lookahead.
    pub fn hd_accounts(&self) -> Vec<(u32, u32, u32)> {
        self.legacy
            .as_ref()
            .map(|c| {
                c.accounts
                    .iter()
                    .map(|(&account, &(external, internal))| (account, external, internal))
                    .collect()
            })
            .unwrap_or_default()
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
        if xpub.depth != 0 || xpub.public_key.to_compressed() != pubkey {
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
        // An encrypted wallet's root decrypts under the same master key that
        // opened the other records. A plaintext chain next to an `mkey` or
        // `ckey` is a modified file, not a wallet Dash Core would load.
        if master.is_some() != chain.crypted {
            return Err(corrupt(if master.is_some() {
                "encrypted wallet has a plaintext HD chain"
            } else {
                "plaintext wallet has an encrypted HD chain"
            }));
        }
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
        let hint = if self.established_network(&seed)? {
            "xpub"
        } else {
            "tpub"
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
        self.kdf_budget()?;
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
        self.kdf_budget()?;
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

impl SqliteWallet {
    /// Every `mkey` is tried until one opens the wallet. Each record is
    /// already capped; their rounds together are capped the same way, so a
    /// file of many candidates cannot multiply the work.
    fn kdf_budget(&self) -> Result<(), WalletDatError> {
        let total: u64 = self.master_keys.iter().map(|m| u64::from(m.rounds)).sum();
        if total > u64::from(crypter::MAX_ROUNDS) {
            return Err(corrupt(format!(
                "mkey records ask for {total} key derivation rounds (at most {})",
                crypter::MAX_ROUNDS
            )));
        }
        Ok(())
    }

    /// Mainnet when the stored HD children derive on coin type 5, test
    /// networks when they derive on coin type 1. Address-book labels are
    /// not evidence: a keypool with no `name` records still has to decide.
    fn established_network(&self, seed: &[u8]) -> Result<bool, WalletDatError> {
        if self.hd_children.is_empty() {
            return Err(WalletDatError::Unsupported(
                "the wallet's network cannot be established from its keys".into(),
            ));
        }
        let master = ErasedXprv(
            ExtendedPrivKey::new_master(dashcore::Network::Mainnet, seed)
                .map_err(|_| corrupt("HD seed is not a BIP32 seed"))?,
        );
        let mut accounts: BTreeMap<(u32, u32), ExtendedPubKey> = BTreeMap::new();
        let mut derives = |coin: u32, child: &HdChild| -> Result<bool, WalletDatError> {
            let xpub = match accounts.entry((coin, child.account)) {
                std::collections::btree_map::Entry::Occupied(e) => *e.get(),
                std::collections::btree_map::Entry::Vacant(e) => {
                    let path = [
                        ChildNumber::from_hardened_idx(44),
                        ChildNumber::from_hardened_idx(coin),
                        ChildNumber::from_hardened_idx(child.account),
                    ]
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| corrupt("hdpubkey account"))?;
                    let account = ErasedXprv(
                        master
                            .0
                            .derive_priv(&path)
                            .map_err(|_| corrupt("hdpubkey account"))?,
                    );
                    *e.insert(ExtendedPubKey::from_priv(&account.0))
                }
            };
            let path = [
                ChildNumber::from_normal_idx(child.change),
                ChildNumber::from_normal_idx(child.index),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| corrupt("hdpubkey record"))?;
            let derived = xpub
                .derive_pub(&path)
                .map_err(|_| corrupt("hdpubkey record"))?;
            Ok(derived.public_key.to_compressed() == child.pubkey)
        };
        let mut network = None;
        for child in &self.hd_children {
            let this = match (derives(5, child)?, derives(1, child)?) {
                (true, false) => true,
                (false, true) => false,
                _ => return Err(corrupt("HD public key does not match the wallet seed")),
            };
            if network.is_some_and(|prev| prev != this) {
                return Err(corrupt("HD public keys are for more than one network"));
            }
            network = Some(this);
        }
        let network = network == Some(true);
        if self.address_mainnet.is_some_and(|book| book != network) {
            return Err(corrupt("address book and HD keys disagree on the network"));
        }
        Ok(network)
    }
}

/// An extended private key erased on every return path.
struct ErasedXprv(ExtendedPrivKey);

impl Drop for ErasedXprv {
    fn drop(&mut self) {
        self.0.private_key.zeroize();
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
    let pubkey = master.private_key.public_key().to_compressed();
    let mut secret = Zeroizing::new([0u8; 32]);
    secret.copy_from_slice(&master.private_key.to_bytes()[..]);
    master.private_key.zeroize();
    Ok((secret, pubkey))
}

fn pubkey_matches(secret: &[u8; 32], pubkey: &[u8]) -> bool {
    let Some(sk) = EcdsaSecretKey::from_bytes(secret).ok() else {
        return false;
    };
    let pk = sk.public_key();
    pk.to_compressed().as_slice() == pubkey || pk.to_uncompressed().as_slice() == pubkey
}

fn pubkey_of(secret: &[u8; 32]) -> Option<[u8; 33]> {
    let sk = EcdsaSecretKey::from_bytes(secret).ok()?;
    Some(sk.public_key().to_compressed())
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
    SqliteWallet::from_records(rows.map(|row| {
        let (key, value) = row.map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
        Ok((key, Zeroizing::new(value)))
    }))
}

/// Reads a legacy Berkeley DB `wallet.dat` (or a descriptor wallet stored in
/// one) into the same records [`read_sqlite`] produces.
pub fn read_bdb(path: &Path) -> Result<SqliteWallet, WalletDatError> {
    let file = std::fs::File::open(path).map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
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
    let bytes = read_into_fixed(file, meta.len() as usize)?;
    read_bdb_bytes(&bytes)
}

/// Reads `source` into one zeroizing buffer sized before the first byte,
/// so no reallocation leaves an unwiped copy of the plaintext behind. A
/// source longer than `expected` (a file that grew after its size was
/// read) is refused rather than grown into.
fn read_into_fixed(
    mut source: impl Read,
    expected: usize,
) -> Result<Zeroizing<Vec<u8>>, WalletDatError> {
    let mut buf = Zeroizing::new(vec![0u8; expected + 1]);
    let mut filled = 0;
    while filled < buf.len() {
        match source.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(WalletDatError::Unreadable(e.to_string())),
        }
    }
    if filled > expected {
        return Err(WalletDatError::Unreadable(
            "the file changed while it was read".into(),
        ));
    }
    buf.truncate(filled);
    Ok(buf)
}

/// [`read_bdb`] over bytes already in memory.
pub fn read_bdb_bytes(bytes: &[u8]) -> Result<SqliteWallet, WalletDatError> {
    SqliteWallet::from_records(crate::bdb::read_pairs(bytes)?.into_iter().map(Ok))
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
        hd_children: Vec::new(),
        locked_outpoints: Vec::new(),
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
        plain_key,
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
    let encrypted = !w.master_keys.is_empty()
        || !w.crypted_keys.is_empty()
        || w.legacy.as_ref().is_some_and(|c| c.crypted);
    // Dash Core refuses any plaintext key in an encrypted wallet, one
    // beside a `ckey` of the same public key included.
    if encrypted && plain_key {
        return Err(corrupt("encrypted wallet has a plaintext key"));
    }
    if let Some(chain) = &w.legacy {
        check_chain_range(chain, &w.hd_children)?;
    }
    // A key Core did not list as `hdpubkey` is not shown to be on the HD
    // chain, including when the file has no `hdpubkey` records at all.
    w.loose_keys = key_pubs.iter().filter(|p| !hd_pubs.contains(*p)).count() as u32;
    Ok(())
}

/// The chain's counters are what the import restores, so they must cover
/// every HD key the file stores. Only account 0 is restored, up to
/// [`MAX_LEGACY_CHAIN_KEYS`] keys per chain.
fn check_chain_range(chain: &LegacyChain, children: &[HdChild]) -> Result<(), WalletDatError> {
    for (&account, &(external, internal)) in &chain.accounts {
        if account != 0 {
            return Err(WalletDatError::ExtraHdAccount(account));
        }
        if external > MAX_LEGACY_CHAIN_KEYS || internal > MAX_LEGACY_CHAIN_KEYS {
            return Err(WalletDatError::RangeTooLarge);
        }
    }
    for child in children {
        let Some(&(external, internal)) = chain.accounts.get(&child.account) else {
            return Err(corrupt(
                "hdpubkey for an account the HD chain does not have",
            ));
        };
        let counter = if child.change == 0 {
            external
        } else {
            internal
        };
        if child.index >= counter {
            return Err(corrupt("hdpubkey past its HD chain's counter"));
        }
    }
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
            let book = if kind == b"name" {
                &mut g.names
            } else {
                &mut g.purposes
            };
            book.insert(address, text);
            if book.len() > MAX_ADDRESS_BOOK_ENTRIES {
                return Err(WalletDatError::Unsupported(
                    "the wallet has more address-book entries than can be restored".into(),
                ));
            }
        }
        b"hdchain" | b"chdchain" => {
            if w.legacy.is_some() {
                return Err(corrupt("conflicting HD chain records"));
            }
            let chain = parse_hdchain(&mut v)?;
            if chain.crypted != (kind == b"chdchain") {
                return Err(corrupt("HD chain type mismatch"));
            }
            w.legacy = Some(chain);
        }
        b"lockedutxo" => {
            let txid = k
                .take(32)
                .ok_or_else(|| bad("lockedutxo"))?
                .try_into()
                .map_err(|_| bad("lockedutxo"))?;
            let vout = k.u32().ok_or_else(|| bad("lockedutxo"))?;
            if w.locked_outpoints.len() >= MAX_LOCKED_OUTPOINTS {
                return Err(WalletDatError::Unsupported(
                    "the wallet has more coin locks than can be restored".into(),
                ));
            }
            w.locked_outpoints.push((txid, vout));
        }
        b"key" | b"ckey" => {
            let pubkey = k.bytes().ok_or_else(|| bad("key"))?.to_vec();
            let secret = v.bytes().ok_or_else(|| bad("key"))?;
            if kind == b"ckey" && (secret.len() != CRYPTED_KEY_LEN || !valid_pubkey(&pubkey)) {
                return Err(corrupt("encrypted key record"));
            }
            let secret = Zeroizing::new(secret.to_vec());
            if !v.is_empty() {
                let checksum = v.hash256().ok_or_else(|| bad("key"))?;
                let hashed = if kind == b"ckey" {
                    sha256d(&secret)
                } else {
                    let mut buf = Zeroizing::new(Vec::with_capacity(pubkey.len() + secret.len()));
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
            } else {
                g.plain_key = true;
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
            g.plain_key = true;
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
            let child = parse_hd_pubkey(&mut v)?;
            if child.pubkey.as_slice() != pubkey.as_slice() {
                return Err(corrupt("hdpubkey does not match its key"));
            }
            // Refused here, before any key is derived, as the import would
            // refuse them later.
            if child.account != 0 {
                return Err(WalletDatError::ExtraHdAccount(child.account));
            }
            if child.index >= MAX_LEGACY_CHAIN_KEYS
                || w.hd_children.len() >= 2 * MAX_LEGACY_CHAIN_KEYS as usize
            {
                return Err(WalletDatError::RangeTooLarge);
            }
            g.hd_pubs.insert(pubkey);
            w.hd_children.push(child);
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

/// A `ckey`'s ciphertext: Dash Core's 32-byte private key (compressed or
/// not) under AES-256-CBC with PKCS#7 padding (`EncryptSecret`).
const CRYPTED_KEY_LEN: usize = 48;

/// A compressed (`02`/`03`) or uncompressed (`04`) serialization of a point
/// on the curve, as Dash Core writes a `CPubKey`.
fn valid_pubkey(pubkey: &[u8]) -> bool {
    matches!(
        (pubkey.len(), pubkey.first()),
        (33, Some(0x02 | 0x03)) | (65, Some(0x04))
    ) && EcdsaPublicKey::from_bytes(pubkey).is_ok()
}
/// Most keys a legacy wallet may have derived on one HD chain, keypool
/// included. Larger wallets are refused.
pub const MAX_LEGACY_CHAIN_KEYS: u32 = 10_000;

/// The most persistent coin locks (`lockedutxo`) a wallet may have.
pub const MAX_LOCKED_OUTPOINTS: usize = 10_000;

/// The most address-book labels (`name`) or purposes (`purpose`) a wallet
/// may have.
pub const MAX_ADDRESS_BOOK_ENTRIES: usize = 50_000;

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
        if accounts.insert(index, (external, internal)).is_some() {
            return Err(corrupt("duplicate hdchain account"));
        }
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

/// `CHDPubKey`: version, then `CExtPubKey` as a 74-byte blob (depth,
/// fingerprint, big-endian child, chain code, compressed pubkey), then the
/// chain id and the account and change indexes.
fn parse_hd_pubkey(v: &mut Reader<'_>) -> Result<HdChild, WalletDatError> {
    v.i32().ok_or_else(|| corrupt("hdpubkey record"))?;
    let ext = v.bytes().ok_or_else(|| corrupt("hdpubkey record"))?;
    if ext.len() != 74 {
        return Err(corrupt("hdpubkey record"));
    }
    let index = u32::from_be_bytes(
        ext[5..9]
            .try_into()
            .map_err(|_| corrupt("hdpubkey record"))?,
    );
    if index & 0x8000_0000 != 0 {
        return Err(corrupt("hdpubkey record"));
    }
    let mut pubkey = [0u8; 33];
    pubkey.copy_from_slice(&ext[41..74]);
    v.hash256().ok_or_else(|| corrupt("hdpubkey record"))?;
    let account = v.u32().ok_or_else(|| corrupt("hdpubkey record"))?;
    let change = v.u32().ok_or_else(|| corrupt("hdpubkey record"))?;
    if change > 1 {
        return Err(corrupt("hdpubkey record"));
    }
    Ok(HdChild {
        account,
        change,
        index,
        pubkey,
    })
}

fn plain_key_matches(pubkey: &[u8], der: &[u8]) -> bool {
    der_secret(der).is_some_and(|secret| pubkey_matches(&secret, pubkey))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_linked_sqlite_has_the_wal_close_deadlock_fix() {
        assert!(
            rusqlite::version_number() >= 3_051_002,
            "SQLite {} can deadlock closing a WAL database (#105); 3.51.2 fixes it",
            rusqlite::version()
        );
    }

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

    /// Records are parsed as SQLite yields them, so a malformed record is
    /// reported before an unreadable row after it.
    #[test]
    fn a_bad_record_is_reported_before_a_later_unreadable_row() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.dat");
        std::fs::copy(vector("desc_nomnemonic.dat"), &path).unwrap();
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            let updated = conn
                .execute(
                    "UPDATE main SET value = ?1 WHERE key = ?2",
                    rusqlite::params![vec![1u8], db_key("flags")],
                )
                .unwrap();
            assert_eq!(updated, 1);
            conn.execute(
                "INSERT INTO main (key, value) VALUES (?1, 7)",
                rusqlite::params![db_key("zzzz")],
            )
            .unwrap();
        }
        let got = read_sqlite(&path);
        assert!(
            matches!(&got, Err(WalletDatError::Corrupt(d)) if d.contains("flags")),
            "{got:?}"
        );
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

    /// More coin locks or address-book records than a real wallet has are
    /// refused while the file is read, before any is written.
    #[test]
    fn too_many_coin_locks_or_address_book_entries_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let base = vector("desc_nomnemonic.dat");
        let fill = |name: &str, n: usize, record: &dyn Fn(usize) -> (Vec<u8>, Vec<u8>)| {
            let path = dir.path().join(name);
            std::fs::copy(&base, &path).unwrap();
            let mut conn = rusqlite::Connection::open(&path).unwrap();
            let tx = conn.transaction().unwrap();
            for i in 0..n {
                let (key, value) = record(i);
                tx.execute(
                    "INSERT INTO main (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, value],
                )
                .unwrap();
            }
            tx.commit().unwrap();
            path
        };
        let lock = |i: usize| {
            let mut key = db_key("lockedutxo");
            let mut txid = [0xeeu8; 32];
            txid[..8].copy_from_slice(&(i as u64).to_le_bytes());
            key.extend_from_slice(&txid);
            key.extend_from_slice(&3u32.to_le_bytes());
            (key, b"1".to_vec())
        };
        let room = MAX_LOCKED_OUTPOINTS - read_sqlite(&base).unwrap().locked_outpoints.len();
        let full = read_sqlite(&fill("locks.dat", room, &lock)).unwrap();
        assert_eq!(full.locked_outpoints.len(), MAX_LOCKED_OUTPOINTS);
        let over = read_sqlite(&fill("more_locks.dat", room + 1, &lock));
        assert!(
            matches!(&over, Err(WalletDatError::Unsupported(d)) if d.contains("coin locks")),
            "{over:?}"
        );
        for kind in ["name", "purpose"] {
            let entry = |i: usize| {
                let mut value = Vec::new();
                ser_bytes(&mut value, b"receive");
                (db_key_pub(kind, format!("entry-{i}").as_bytes()), value)
            };
            let over = read_sqlite(&fill(
                &format!("{kind}.dat"),
                MAX_ADDRESS_BOOK_ENTRIES + 1,
                &entry,
            ));
            assert!(
                matches!(&over, Err(WalletDatError::Unsupported(d)) if d.contains("address-book")),
                "{kind}: {over:?}"
            );
        }
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

    /// A legacy wallet's records in a SQLite file (`createfromdump
    /// -format=sqlite`) are refused as in Berkeley DB: an account other
    /// than 0 is `ExtraHdAccount`, never an unreadable file (#81 item 1).
    #[test]
    fn legacy_records_in_sqlite_get_the_same_refusal() {
        let pairs =
            crate::bdb::read_pairs(&std::fs::read(vector("bdb_account1.dat")).unwrap()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wallet.dat");
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE main (key BLOB PRIMARY KEY, value BLOB)", [])
            .unwrap();
        for (k, v) in &pairs {
            conn.execute(
                "INSERT INTO main VALUES (?1, ?2)",
                rusqlite::params![&k[..], &v[..]],
            )
            .unwrap();
        }
        drop(conn);
        assert_eq!(
            read_sqlite(&path).err(),
            Some(WalletDatError::ExtraHdAccount(1))
        );
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
        let sk = dashcore::ecdsa::EcdsaSecretKey::from_bytes(secret).unwrap();
        sk.public_key().to_compressed()
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
        ser_hdpubkey_at(pubkey, 0, 0, 0)
    }

    fn ser_hdpubkey_at(pubkey: &[u8], account: u32, change: u32, index: u32) -> Vec<u8> {
        let mut value = Vec::new();
        value.extend_from_slice(&1i32.to_le_bytes());
        let mut ext = vec![0u8; 74];
        ext[5..9].copy_from_slice(&index.to_be_bytes());
        ext[41..41 + pubkey.len()].copy_from_slice(pubkey);
        ser_bytes(&mut value, &ext);
        value.extend_from_slice(&[0u8; 32]);
        value.extend_from_slice(&account.to_le_bytes());
        value.extend_from_slice(&change.to_le_bytes());
        value
    }

    fn receive_address(seed: &[u8]) -> String {
        let master = ExtendedPrivKey::new_master(Network::Regtest, seed).unwrap();
        let path = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
        let child = master.derive_priv(&path).unwrap();
        let pk = PublicKey::new(child.private_key.public_key());
        Address::p2pkh(&pk, Network::Regtest).to_string()
    }

    type Records = Vec<(Vec<u8>, Vec<u8>)>;

    struct Built {
        bytes: Vec<u8>,
        seed: Vec<u8>,
        address: String,
    }

    fn plain_hd_wallet(swap: bool) -> Built {
        let (pairs, seed, address) = plain_hd_records();
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        Built {
            bytes: build::pairs(&refs, swap),
            seed,
            address,
        }
    }

    /// [`plain_hd_wallet`] laid out as a larger file is: an internal root
    /// over two leaves, with the `hdchain` record's value in a two-page
    /// overflow chain.
    fn tree_overflow_hd_wallet() -> Vec<u8> {
        let (pairs, _, _) = plain_hd_records();
        let leaf = |page_num: u32, pairs: &[(Vec<u8>, Vec<u8>)], first: Vec<Vec<u8>>| {
            let mut items = first;
            for (k, v) in pairs {
                items.push(build::data_item(k, false, false));
                items.push(build::data_item(v, false, false));
            }
            build::pack(page_num, &items, TYPE_LEAF, 1, 0, false)
        };
        let (chain_key, chain) = &pairs[0];
        assert_eq!(Reader::new(chain_key).bytes(), Some(b"hdchain".as_slice()));
        let half = chain.len() / 2;
        let root = build::pack(
            3,
            &[build::child_item(4, false), build::child_item(5, false)],
            TYPE_INTERNAL,
            2,
            0,
            false,
        );
        let first = vec![
            build::data_item(chain_key, false, false),
            build::overflow_item(6, chain.len() as u32, false),
        ];
        build::assemble(
            &[
                root,
                leaf(4, &pairs[1..5], first),
                leaf(5, &pairs[5..], Vec::new()),
                build::overflow_page(6, &chain[..half], 7, false),
                build::overflow_page(7, &chain[half..], 0, false),
            ],
            false,
        )
    }

    #[test]
    fn a_tree_with_an_overflow_chain_reads_as_its_records() {
        let flat = read_bdb_bytes(&plain_hd_wallet(false).bytes).unwrap();
        let tree = read_bdb_bytes(&tree_overflow_hd_wallet()).unwrap();
        assert_eq!(
            tree.legacy.as_ref().unwrap().seed.as_slice(),
            flat.legacy.as_ref().unwrap().seed.as_slice()
        );
        assert_eq!(tree.loose_keys, flat.loose_keys);
        assert_eq!(tree.address_book.len(), flat.address_book.len());
        assert_eq!(
            tree.hd_root(None).unwrap().master_pubkey,
            flat.hd_root(None).unwrap().master_pubkey
        );
    }

    fn plain_hd_records() -> (Records, Vec<u8>, String) {
        let seed = core_seed(PHRASE.as_bytes(), MNEMONIC_PASS.as_bytes());
        let id = sha256d(&seed[..]);
        let address = receive_address(&seed[..]);
        let child_secret = {
            let master = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
            let path = DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap();
            *master.derive_priv(&path).unwrap().private_key.to_bytes()
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
        (pairs.to_vec(), seed.to_vec(), address)
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
            *xprv
                .derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .to_bytes()
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
                expect.private_key.public_key().to_compressed()
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
        let chain = ser_chain(&id, false, &seed, b"", b"", 1, 4);
        let child = {
            let master = ExtendedPrivKey::new_master(Network::Regtest, &seed).unwrap();
            master
                .derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .public_key()
                .to_compressed()
        };
        let pairs = [
            (db_key("hdchain"), chain),
            (db_key_pub("hdpubkey", &child), ser_hdpubkey(&child)),
        ];
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
        assert_eq!(&pairs[0].0[..], b"k");
        assert_eq!(&pairs[0].1[..], payload);
        let deleted = vec![
            build::data_item(b"gone-key", true, false),
            build::data_item(b"gone-val", false, false),
            build::data_item(b"kept-key", false, false),
            build::data_item(b"kept-val", false, false),
        ];
        let leaf = build::pack(3, &deleted, TYPE_LEAF, 1, 0, false);
        let pairs = crate::bdb::read_pairs(&build::assemble(&[leaf], false)).unwrap();
        assert_eq!(&pairs[0].0[..], b"kept-key");
        assert_eq!(&pairs[0].1[..], b"kept-val");
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

    /// Bytes past the last whole page are refused, not ignored.
    #[test]
    fn a_partial_trailing_page_is_refused() {
        let mut file = build::pairs(&[(b"k", b"v")], false);
        assert!(crate::bdb::read_pairs(&file).is_ok());
        file.push(0);
        assert_eq!(
            crate::bdb::read_pairs(&file).err(),
            Some(WalletDatError::Corrupt(
                "file is not a whole number of pages".into()
            ))
        );
    }

    /// A child named twice is refused when it is pushed, before it is read,
    /// so slots that all name one page cannot grow the walk's stack.
    #[test]
    fn a_btree_child_named_twice_is_refused_before_it_is_read() {
        let internal = build::pack(
            3,
            &[build::child_item(4, false), build::child_item(4, false)],
            TYPE_INTERNAL,
            2,
            0,
            false,
        );
        // Page 4 claims another number: reading it fails differently.
        let unreadable = build::overflow_page(9, b"", 0, false);
        let err =
            crate::bdb::read_pairs(&build::assemble(&[internal, unreadable], false)).unwrap_err();
        assert_eq!(err, WalletDatError::Corrupt("btree page cycle".into()));
    }

    #[test]
    fn overflow_past_the_inner_last_page_is_readable() {
        let payload = b"real-core-overflow";
        let items = vec![
            build::data_item(b"k", false, false),
            build::overflow_item(4, payload.len() as u32, false),
        ];
        let leaf = build::pack(3, &items, TYPE_LEAF, 1, 0, false);
        let overflow = build::overflow_page(4, payload, 0, false);
        // Inner last_page 2, outer last_page 4, inner root at page 3. The
        // four Dash Core fixtures have this shape (2 and 3).
        let file = build::assemble_bounds(&[leaf, overflow], 2, false);
        let pairs = crate::bdb::read_pairs(&file).unwrap();
        assert_eq!(&pairs[0].1[..], payload);
    }

    #[test]
    fn two_items_sharing_an_overflow_chain_are_refused() {
        let items = vec![
            build::data_item(b"k1", false, false),
            build::overflow_item(4, 3, false),
            build::data_item(b"k2", false, false),
            build::overflow_item(4, 3, false),
        ];
        let leaf = build::pack(3, &items, TYPE_LEAF, 1, 0, false);
        let overflow = build::overflow_page(4, b"abc", 0, false);
        let file = build::assemble(&[leaf, overflow], false);
        let err = crate::bdb::read_pairs(&file).err().unwrap();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("used twice")),
            "{err:?}"
        );
    }

    #[test]
    fn an_empty_overflow_page_is_refused() {
        let items = vec![
            build::data_item(b"k", false, false),
            build::overflow_item(4, 1, false),
        ];
        let leaf = build::pack(3, &items, TYPE_LEAF, 1, 0, false);
        let empty = build::overflow_page(4, b"", 5, false);
        let last = build::overflow_page(5, b"x", 0, false);
        let file = build::assemble(&[leaf, empty, last], false);
        let err = crate::bdb::read_pairs(&file).err().unwrap();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("empty overflow page")),
            "{err:?}"
        );
    }

    #[test]
    fn a_duplicated_hdchain_account_is_refused() {
        let seed = [0x5au8; 64];
        let mut chain = ser_chain(&sha256d(&seed), false, &seed, b"", b"", 5000, 2);
        let n = chain.len();
        chain[n - 13] = 2;
        chain.extend_from_slice(&0u32.to_le_bytes());
        chain.extend_from_slice(&1u32.to_le_bytes());
        chain.extend_from_slice(&0u32.to_le_bytes());
        let pairs = [(db_key("hdchain"), chain)];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("duplicate hdchain account")),
            "{err:?}"
        );
    }

    #[test]
    fn an_hd_key_past_its_chain_counter_is_refused() {
        let seed = [0x5au8; 64];
        let chain = ser_chain(&sha256d(&seed), false, &seed, b"", b"", 4, 2);
        let pubkey = [0x02u8; 33];
        for (change, index) in [(0, 4), (1, 2)] {
            let pairs = [
                (db_key("hdchain"), chain.clone()),
                (
                    db_key_pub("hdpubkey", &pubkey),
                    ser_hdpubkey_at(&pubkey, 0, change, index),
                ),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
            assert!(
                matches!(err, WalletDatError::Corrupt(ref d) if d.contains("past its HD chain's counter")),
                "{err:?}"
            );
        }
    }

    #[test]
    fn hd_keys_the_import_would_refuse_are_refused_before_any_derivation() {
        let seed = [0x5au8; 64];
        let chain = ser_chain(&sha256d(&seed), false, &seed, b"", b"", 1, 0);
        let pubkey = [0x02u8; 33];
        for (account, index, want) in [
            (1, 0, WalletDatError::ExtraHdAccount(1)),
            (0, MAX_LEGACY_CHAIN_KEYS, WalletDatError::RangeTooLarge),
        ] {
            let pairs = [
                (db_key("hdchain"), chain.clone()),
                (
                    db_key_pub("hdpubkey", &pubkey),
                    ser_hdpubkey_at(&pubkey, account, 0, index),
                ),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
            assert_eq!(err, want);
        }
    }

    #[test]
    fn a_dirty_last_page_is_not_a_flushed_wallet() {
        let payload = b"k";
        let items = vec![
            build::data_item(b"k", false, false),
            build::data_item(payload, false, false),
        ];
        let leaf = build::pack(3, &items, TYPE_LEAF, 1, 0, false);
        let mut file = build::assemble(&[leaf], false);
        let pagesize = 4096;
        let last = file.len() / pagesize - 1;
        file[last * pagesize] = 9;
        assert!(crate::bdb::read_pairs(&file).is_err());
    }

    #[test]
    fn an_encrypted_wallet_rejects_a_plaintext_chain() {
        let seed = core_seed(PHRASE.as_bytes(), b"second");
        let salt = [7u8; 8];
        let (pass_key, pass_iv) = crypter::key_from_passphrase(b"bdb-pass", &salt, 2).unwrap();
        let master = [0x42u8; 32];
        let crypted_master = crypter::aes_cbc_encrypt(&pass_key, &pass_iv, &master);
        let child_secret = {
            let xprv = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
            *xprv
                .derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .to_bytes()
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
        let other = core_seed(PHRASE.as_bytes(), b"");
        let other_id = sha256d(&other[..]);
        let chain = ser_chain(&other_id, false, &other[..], PHRASE.as_bytes(), b"", 1, 0);
        let pairs = [
            (db_key("hdchain"), chain),
            (mkey_key, mkey),
            (db_key_pub("ckey", &child_pub), ckey),
            (db_key_pub("hdpubkey", &child_pub), ser_hdpubkey(&child_pub)),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        let err = wallet.hd_root(Some(b"bdb-pass")).unwrap_err();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("plaintext HD chain")),
            "{err:?}"
        );
    }

    #[test]
    fn two_hd_chain_records_are_rejected() {
        let seed = [0x5au8; 64];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, false, &seed, b"", b"", 1, 0);
        let pairs = [
            (db_key("hdchain"), chain.clone()),
            (db_key("chdchain"), chain),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let err = read_bdb_bytes(&build::pairs(&refs, false)).unwrap_err();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("conflicting")),
            "{err:?}"
        );
    }

    #[test]
    fn an_hd_chain_does_not_hide_loose_keys() {
        let seed = [0x5au8; 64];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, false, &seed, b"", b"", 1, 0);
        let secret = [0x44u8; 32];
        let pubkey = compressed(&secret);
        let mut value = Vec::new();
        ser_bytes(&mut value, &der_of(&secret));
        let pairs = [
            (db_key("hdchain"), chain),
            (db_key_pub("key", &pubkey), value),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        assert_eq!(wallet.loose_keys, 1);
    }

    #[test]
    fn an_encrypted_wallet_with_a_plaintext_key_is_corrupt() {
        let plain = [0x44u8; 32];
        let plain_pub = compressed(&plain);
        let mut key = Vec::new();
        ser_bytes(&mut key, &der_of(&plain));
        let crypted_pub = compressed(&[0x45u8; 32]);
        let ct = [0x99u8; 48];
        let mut ckey = Vec::new();
        ser_bytes(&mut ckey, &ct);
        ckey.extend_from_slice(&sha256d(&ct));
        let pairs = [
            (db_key_pub("key", &plain_pub), key),
            (db_key_pub("ckey", &crypted_pub), ckey),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("plaintext key")),
            "{err:?}"
        );
    }

    /// A plaintext `key` or `wkey` whose public key also has a `ckey` is
    /// refused: Dash Core loads no plaintext key into an encrypted wallet.
    #[test]
    fn a_plaintext_key_beside_its_own_ckey_is_corrupt() {
        let secret = [0x44u8; 32];
        let pubkey = compressed(&secret);
        let mut key = Vec::new();
        ser_bytes(&mut key, &der_of(&secret));
        let mut wkey = Vec::new();
        wkey.extend_from_slice(&1i32.to_le_bytes());
        ser_bytes(&mut wkey, &der_of(&secret));
        wkey.extend_from_slice(&0i64.to_le_bytes());
        wkey.extend_from_slice(&0i64.to_le_bytes());
        ser_bytes(&mut wkey, b"");
        let ct = [0x99u8; 48];
        let mut ckey = Vec::new();
        ser_bytes(&mut ckey, &ct);
        ckey.extend_from_slice(&sha256d(&ct));
        for (kind, value) in [("key", key), ("wkey", wkey)] {
            let pairs = [
                (db_key_pub(kind, &pubkey), value),
                (db_key_pub("ckey", &pubkey), ckey.clone()),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
            assert!(
                matches!(err, WalletDatError::Corrupt(ref d) if d.contains("plaintext key")),
                "{kind}: {err:?}"
            );
        }
    }

    fn lone_ckey(pubkey: &[u8], ct: &[u8]) -> Result<SqliteWallet, WalletDatError> {
        let mut value = Vec::new();
        ser_bytes(&mut value, ct);
        value.extend_from_slice(&sha256d(ct));
        let key = db_key_pub("ckey", pubkey);
        read_bdb_bytes(&build::pairs(&[(&key, &value)], false))
    }

    /// A `ckey` is kept only with a valid public key and Core's 48-byte
    /// ciphertext, before any passphrase probe hashes or decrypts it.
    #[test]
    fn a_ckey_of_the_wrong_shape_is_corrupt() {
        let secret = [0x45u8; 32];
        let ct = [0x99u8; 48];
        let sk = dashcore::ecdsa::EcdsaSecretKey::from_bytes(&secret).unwrap();
        let pk = sk.public_key();
        let (short, long) = (pk.to_compressed(), pk.to_uncompressed());
        let mut bad_prefix = short;
        bad_prefix[0] = 0x05;
        let mut hybrid = long;
        hybrid[0] = 0x06 | (long[64] & 1);
        let bad_pubkeys: [&[u8]; 5] =
            [&[0x02; 300], &long[..64], &bad_prefix, &hybrid, &[0x04; 65]];
        for pubkey in bad_pubkeys {
            let err = lone_ckey(pubkey, &ct).err().unwrap();
            assert!(
                matches!(err, WalletDatError::Corrupt(ref d) if d.contains("encrypted key record")),
                "{} bytes: {err:?}",
                pubkey.len()
            );
        }
        for len in [0, 32, 47, 49, 64, 1024] {
            let err = lone_ckey(&short, &vec![0x99; len]).err().unwrap();
            assert!(
                matches!(err, WalletDatError::Corrupt(ref d) if d.contains("encrypted key record")),
                "{len} bytes: {err:?}"
            );
        }
        for pubkey in [&short[..], &long[..]] {
            let wallet = lone_ckey(pubkey, &ct).unwrap();
            assert!(wallet.encrypted());
            assert_eq!(wallet.crypted_keys.len(), 1);
        }
    }

    /// A plaintext key beside a master key or an encrypted HD chain, with
    /// no encrypted key, is refused as well.
    #[test]
    fn a_plaintext_key_beside_a_master_key_or_encrypted_chain_is_corrupt() {
        let plain = [0x44u8; 32];
        let mut key = Vec::new();
        ser_bytes(&mut key, &der_of(&plain));
        let key = (db_key_pub("key", &compressed(&plain)), key);
        let mut mkey = Vec::new();
        ser_bytes(&mut mkey, &[0x11u8; 48]);
        ser_bytes(&mut mkey, &[0x22u8; 8]);
        mkey.extend_from_slice(&0u32.to_le_bytes());
        mkey.extend_from_slice(&2u32.to_le_bytes());
        ser_bytes(&mut mkey, b"");
        let mut mkey_key = db_key("mkey");
        mkey_key.extend_from_slice(&1u32.to_le_bytes());
        let seed = [0x5au8; 64];
        let chain = ser_chain(&sha256d(&seed), true, &seed, b"", b"", 1, 0);
        for beside in [(mkey_key, mkey), (db_key("chdchain"), chain)] {
            let pairs = [key.clone(), beside];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            let err = read_bdb_bytes(&build::pairs(&refs, false)).err().unwrap();
            assert!(
                matches!(err, WalletDatError::Corrupt(ref d) if d.contains("plaintext key")),
                "{err:?}"
            );
        }
    }

    fn label_free_wallet(coin: u32, seed_of_child: &[u8]) -> SqliteWallet {
        let seed = core_seed(PHRASE.as_bytes(), b"");
        let id = sha256d(&seed[..]);
        let chain = ser_chain(&id, false, &seed[..], PHRASE.as_bytes(), b"", 1, 0);
        let child = ExtendedPrivKey::new_master(Network::Regtest, seed_of_child)
            .unwrap()
            .derive_priv(&DerivationPath::from_str(&format!("m/44'/{coin}'/0'/0/0")).unwrap())
            .unwrap()
            .private_key
            .public_key()
            .to_compressed();
        let pairs = [
            (db_key("hdchain"), chain),
            (db_key_pub("hdpubkey", &child), ser_hdpubkey(&child)),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        read_bdb_bytes(&build::pairs(&refs, false)).unwrap()
    }

    #[test]
    fn a_label_free_wallet_takes_its_network_from_its_keypool() {
        let seed = core_seed(PHRASE.as_bytes(), b"");
        let main = label_free_wallet(5, &seed[..]);
        assert_eq!(main.hd_root(None).unwrap().external.key.as_str(), "xpub");
        let test = label_free_wallet(1, &seed[..]);
        assert_eq!(test.hd_root(None).unwrap().external.key.as_str(), "tpub");
        let foreign = label_free_wallet(1, &[0x33u8; 64]);
        let err = foreign.hd_root(None).err().unwrap();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("does not match the wallet seed")),
            "{err:?}"
        );
    }

    #[test]
    fn the_file_image_is_read_into_one_fixed_buffer() {
        let read = read_into_fixed(&b"wallet"[..], 6).unwrap();
        assert_eq!(&read[..], b"wallet");
        assert_eq!(read.capacity(), 7, "the buffer was reallocated");
        let short = read_into_fixed(&b"wal"[..], 6).unwrap();
        assert_eq!(&short[..], b"wal");
        let grown = read_into_fixed(&b"wallet and more"[..], 6).err().unwrap();
        assert!(
            matches!(grown, WalletDatError::Unreadable(ref d) if d.contains("changed")),
            "{grown:?}"
        );
    }

    #[test]
    fn bdb_records_are_wiped_on_drop() {
        type Wiped = zeroize::Zeroizing<Vec<u8>>;
        type Pairs = Result<Vec<(Wiped, Wiped)>, WalletDatError>;
        let read: fn(&[u8]) -> Pairs = crate::bdb::read_pairs;
        assert!(read(&[]).is_err());
    }

    #[test]
    fn locked_utxos_are_kept() {
        let seed = [0x5au8; 64];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, false, &seed, b"", b"", 1, 0);
        let child = {
            let master = ExtendedPrivKey::new_master(Network::Regtest, &seed).unwrap();
            master
                .derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .public_key()
                .to_compressed()
        };
        let mut key = db_key("lockedutxo");
        let txid = [0xabu8; 32];
        key.extend_from_slice(&txid);
        key.extend_from_slice(&7u32.to_le_bytes());
        let pairs = [
            (db_key("hdchain"), chain),
            (db_key_pub("hdpubkey", &child), ser_hdpubkey(&child)),
            (key, b"1".to_vec()),
        ];
        let refs: Vec<(&[u8], &[u8])> = pairs
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        assert_eq!(wallet.locked_outpoints, vec![(txid, 7)]);
    }

    #[test]
    fn mkey_rounds_are_capped_across_records() {
        let seed = [0x5au8; 64];
        let id = sha256d(&seed);
        let chain = ser_chain(&id, true, &seed, b"phrase", b"", 1, 0);
        let rounds = crypter::MAX_ROUNDS / 2 + 1;
        let mut records = vec![(db_key("chdchain"), chain)];
        for n in 1..=2u32 {
            let mut mkey = Vec::new();
            ser_bytes(&mut mkey, &[0u8; 32]);
            ser_bytes(&mut mkey, &[n as u8; 8]);
            mkey.extend_from_slice(&0u32.to_le_bytes());
            mkey.extend_from_slice(&rounds.to_le_bytes());
            ser_bytes(&mut mkey, b"");
            let mut mkey_key = db_key("mkey");
            mkey_key.extend_from_slice(&n.to_le_bytes());
            records.push((mkey_key, mkey));
        }
        let child = [0x02u8; 33];
        records.push((db_key_pub("hdpubkey", &child), ser_hdpubkey(&child)));
        let refs: Vec<(&[u8], &[u8])> = records
            .iter()
            .map(|(k, v)| (k.as_slice(), v.as_slice()))
            .collect();
        let wallet = read_bdb_bytes(&build::pairs(&refs, false)).unwrap();
        let err = wallet.hd_root(Some(b"bdb-pass")).unwrap_err();
        assert!(
            matches!(err, WalletDatError::Corrupt(ref d) if d.contains("derivation rounds")),
            "{err:?}"
        );
    }

    fn hex_bytes(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Addresses Dash Core v24.0.2 reported for these regtest wallets
    /// (`getnewaddress` / `getrawchangeaddress`, confirmed in `dumpwallet`).
    fn core_address(phrase: &str, pass: &str, path: &str) -> String {
        let seed = core_seed(phrase.as_bytes(), pass.as_bytes());
        let master = ExtendedPrivKey::new_master(Network::Regtest, &seed[..]).unwrap();
        let child = master
            .derive_priv(&DerivationPath::from_str(path).unwrap())
            .unwrap();
        let pk = PublicKey::new(child.private_key.public_key());
        Address::p2pkh(&pk, Network::Regtest).to_string()
    }

    /// `wallet.dat` files written by Dash Core v24.0.2 (`format: bdb`), not by
    /// the test builder. Ground truth is that release's `dumphdinfo` and
    /// `dumpwallet` on regtest.
    #[test]
    fn dash_core_v24_legacy_wallets_match_core() {
        let abandon = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let zoo = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
        let plain = read_bdb(&vector("bdb_core24_hd_plain.dat")).unwrap();
        assert!(!plain.encrypted());
        assert_eq!(
            plain.legacy.as_ref().unwrap().accounts.get(&0),
            Some(&(4, 2))
        );
        assert_eq!(
            plain.legacy.as_ref().unwrap().seed.as_slice(),
            hex_bytes("5eb00bbddcf069084889a8ab9155568165f5c453ccb85e70811aaed6f6da5fc19a5ac40b389cd370d086206dec8aa6c43daea6690f20ad3d8d48b2d2ce9e38e4").as_slice()
        );
        assert_eq!(plain.loose_keys, 0);
        assert!(plain.address_book.iter().any(|e| e.label == "savings"));
        assert!(plain.address_book.iter().any(|e| e.label == "checking"));
        let root = plain.hd_root(None).unwrap();
        let (phrase, pass) = root.mnemonic.unwrap();
        assert_eq!(&phrase[..], abandon.as_bytes());
        assert!(pass.is_empty());
        assert_eq!(
            core_address(abandon, "", "m/44'/1'/0'/0/0"),
            "yRd4FhXfVGHXpsuZXPNkMrfD9GVj46pnjt"
        );
        assert_eq!(
            core_address(abandon, "", "m/44'/1'/0'/0/1"),
            "yfd64jEpzzTLrHnR1wq3iiYXh68AiU8mcw"
        );
        assert_eq!(
            core_address(abandon, "", "m/44'/1'/0'/1/0"),
            "yNwHG9D2rnrJRQ8j7TG6xnWfbr8D634HeM"
        );

        let with_pass = read_bdb(&vector("bdb_core24_hd_mnem.dat")).unwrap();
        let root = with_pass.hd_root(None).unwrap();
        let (phrase, pass) = root.mnemonic.unwrap();
        assert_eq!(&phrase[..], zoo.as_bytes());
        assert_eq!(&pass[..], b"TREZOR");
        assert_eq!(
            with_pass.legacy.as_ref().unwrap().accounts.get(&0),
            Some(&(4, 2))
        );
        assert_eq!(
            with_pass.legacy.as_ref().unwrap().seed.as_slice(),
            hex_bytes("ac27495480225222079d7be181583751e86f571027b0497b5b5d11218e0a8a13332572917f0f8e5a589620c6f15b11c61dee327651a14c34e18231052e48c069").as_slice()
        );
        assert_eq!(
            core_address(zoo, "TREZOR", "m/44'/1'/0'/0/0"),
            "yfzF8VHskautet5pKeKo1aoK1EgkyJpeU1"
        );
        assert_eq!(
            core_address(zoo, "TREZOR", "m/44'/1'/0'/1/0"),
            "yZ1ofQ8tHvDFomU7fp7tXEBo5w9nXex95R"
        );

        let enc = read_bdb(&vector("bdb_core24_hd_encrypted.dat")).unwrap();
        assert!(enc.encrypted());
        assert_eq!(
            enc.hd_root(None).unwrap_err(),
            WalletDatError::PassphraseRequired
        );
        assert_eq!(
            enc.hd_root(Some(b"nope")).unwrap_err(),
            WalletDatError::WrongPassphrase
        );
        let root = enc.hd_root(Some(b"core-wallet-pass")).unwrap();
        let (phrase, pass) = root.mnemonic.unwrap();
        assert_eq!(&phrase[..], abandon.as_bytes());
        assert!(pass.is_empty());
        assert_eq!(enc.legacy.as_ref().unwrap().accounts.get(&0), Some(&(4, 3)));
        assert_eq!(root.master_pubkey, {
            let seed = core_seed(abandon.as_bytes(), b"");
            ExtendedPrivKey::new_master(Network::Regtest, &seed[..])
                .unwrap()
                .private_key
                .public_key()
                .to_compressed()
        });

        let loose = read_bdb(&vector("bdb_core24_nonhd.dat")).unwrap();
        assert_eq!(loose.loose_keys, 3);
        assert_eq!(loose.hd_root(None).unwrap_err(), WalletDatError::NoHdChain);
        let mut book: Vec<_> = loose
            .address_book
            .iter()
            .map(|e| (e.label.as_str(), e.address.as_str()))
            .collect();
        book.sort_unstable();
        assert_eq!(
            book,
            [
                ("loose1", "yNwHG9D2rnrJRQ8j7TG6xnWfbr8D634HeM"),
                ("loose2", "yRd4FhXfVGHXpsuZXPNkMrfD9GVj46pnjt"),
                ("loose3", "yTGFWr27HBYsb8tkLbGdxLxuKbspamdbnG"),
            ]
        );
    }

    /// Synthetic fixtures the builder writes. Real Dash Core files are the
    /// `bdb_core24_*` fixtures.
    #[test]
    fn committed_bdb_fixtures_match_the_builder() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/compat/walletdat");
        let plain = plain_hd_wallet(false);
        let encrypted = encrypted_hd_wallet();
        let tree = tree_overflow_hd_wallet();
        if std::env::var_os("DW_WRITE_BDB_FIXTURES").is_some() {
            std::fs::write(dir.join("bdb_hd_plain.dat"), &plain.bytes).unwrap();
            std::fs::write(dir.join("bdb_hd_encrypted.dat"), &encrypted).unwrap();
            std::fs::write(dir.join("bdb_tree_overflow.dat"), &tree).unwrap();
        }
        assert_eq!(
            std::fs::read(dir.join("bdb_tree_overflow.dat")).unwrap(),
            tree
        );
        assert_eq!(
            std::fs::read(dir.join("bdb_hd_plain.dat")).unwrap(),
            plain.bytes
        );
        assert_eq!(
            std::fs::read(dir.join("bdb_hd_encrypted.dat")).unwrap(),
            encrypted
        );
    }

    /// Wallets the engine tests import. Written with `DW_WRITE_BDB_FIXTURES=1`.
    fn engine_security_wallets() -> Vec<(&'static str, Vec<u8>)> {
        let phrase = PHRASE.as_bytes();
        let seed = core_seed(phrase, b"");
        let id = sha256d(&seed[..]);
        let abandon = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let abandon_seed = core_seed(abandon, b"");
        let abandon_id = sha256d(&abandon_seed[..]);
        let child_of = |seed: &[u8]| {
            let master = ExtendedPrivKey::new_master(Network::Regtest, seed).unwrap();
            master
                .derive_priv(&DerivationPath::from_str("m/44'/1'/0'/0/0").unwrap())
                .unwrap()
                .private_key
                .public_key()
                .to_compressed()
        };
        let child = child_of(&seed[..]);
        let abandon_child = child_of(&abandon_seed[..]);
        let hd = |id: &[u8; 32], seed: &[u8], phrase: &[u8], external, internal| {
            ser_chain(id, false, seed, phrase, b"", external, internal)
        };
        let label_free = {
            let pairs = [
                (db_key("hdchain"), hd(&id, &seed[..], phrase, 1, 0)),
                (db_key_pub("hdpubkey", &child), ser_hdpubkey(&child)),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            build::pairs(&refs, false)
        };
        let secret = [0x44u8; 32];
        let loose_pub = compressed(&secret);
        let mut loose_value = Vec::new();
        ser_bytes(&mut loose_value, &der_of(&secret));
        let mut lock_key = db_key("lockedutxo");
        let txid = [0xabu8; 32];
        lock_key.extend_from_slice(&txid);
        lock_key.extend_from_slice(&7u32.to_le_bytes());
        let wide = {
            let pairs = [
                (
                    db_key("hdchain"),
                    hd(&abandon_id, &abandon_seed[..], abandon, 1001, 2),
                ),
                (
                    db_key_pub("hdpubkey", &abandon_child),
                    ser_hdpubkey(&abandon_child),
                ),
                (db_key_pub("key", &loose_pub), loose_value),
                (lock_key, b"1".to_vec()),
                (db_key("cj_salt"), [0x5cu8; 32].to_vec()),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            build::pairs(&refs, false)
        };
        let wide_change = {
            let pairs = [
                (
                    db_key("hdchain"),
                    hd(&abandon_id, &abandon_seed[..], abandon, 4, 2600),
                ),
                (
                    db_key_pub("hdpubkey", &abandon_child),
                    ser_hdpubkey(&abandon_child),
                ),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            build::pairs(&refs, false)
        };
        let mut account1_chain = hd(&abandon_id, &abandon_seed[..], abandon, 1, 0);
        let n = account1_chain.len();
        account1_chain[n - 12..n - 8].copy_from_slice(&1u32.to_le_bytes());
        let account1 = {
            let pairs = [
                (db_key("hdchain"), account1_chain),
                (
                    db_key_pub("hdpubkey", &abandon_child),
                    ser_hdpubkey(&abandon_child),
                ),
            ];
            let refs: Vec<(&[u8], &[u8])> = pairs
                .iter()
                .map(|(k, v)| (k.as_slice(), v.as_slice()))
                .collect();
            build::pairs(&refs, false)
        };
        vec![
            ("bdb_label_free.dat", label_free),
            ("bdb_wide_range.dat", wide),
            ("bdb_wide_change.dat", wide_change),
            ("bdb_account1.dat", account1),
        ]
    }

    #[test]
    fn engine_security_fixtures_are_stable() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/compat/walletdat");
        for (name, bytes) in engine_security_wallets() {
            let path = dir.join(name);
            if std::env::var_os("DW_WRITE_BDB_FIXTURES").is_some() {
                std::fs::write(&path, &bytes).unwrap();
            }
            assert_eq!(std::fs::read(&path).unwrap(), bytes, "{name}");
        }
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
