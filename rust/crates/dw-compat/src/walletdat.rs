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
//! - **Berkeley DB** legacy wallets are only recognised here (magic
//!   `0x00053162` at offset 12); reading them is M6 work.
//!
//! The SQLite file is opened read-only and immutable, so importing never
//! writes next to the user's file (no `-wal`/`-shm`/journal).

use std::collections::BTreeMap;
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
}

impl std::fmt::Display for WalletDatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(d) => write!(f, "unreadable: {d}"),
            Self::Unsupported(d) => write!(f, "unsupported: {d}"),
            Self::Corrupt(d) => write!(f, "corrupt: {d}"),
            Self::PassphraseRequired => f.write_str("passphrase required"),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
        }
    }
}

impl std::error::Error for WalletDatError {}

fn corrupt(what: impl Into<String>) -> WalletDatError {
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

/// The records of a SQLite wallet.dat an import uses, not yet decrypted.
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
    /// The active receive descriptor (public form).
    pub external: PkhDescriptor,
}

impl std::fmt::Debug for HdRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HdRoot")
            .field("has_mnemonic", &self.mnemonic.is_some())
            .field("external", &self.external)
            .finish_non_exhaustive()
    }
}

impl SqliteWallet {
    /// Whether the wallet has a master key record (`encryptwallet` ran).
    pub fn encrypted(&self) -> bool {
        !self.master_keys.is_empty() || self.keys.iter().any(|k| k.encrypted)
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
        self.descriptors
            .iter()
            .filter(|d| Some(d.id) == self.active_external || Some(d.id) == self.active_internal)
            .map(|d| d.next_index.max(0) as u32)
            .max()
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
            return Err(WalletDatError::Unsupported(
                "legacy (non-descriptor) SQLite wallet".into(),
            ));
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
        })
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
    let mut wallet = SqliteWallet {
        flags: 0,
        descriptors: Vec::new(),
        address_book: Vec::new(),
        active_external: None,
        active_internal: None,
        coinjoin_salt: None,
        master_keys: Vec::new(),
        keys: Vec::new(),
    };
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    let mut purposes: BTreeMap<String, String> = BTreeMap::new();
    for row in rows {
        let (key, value) = row.map_err(|e| WalletDatError::Unreadable(e.to_string()))?;
        let value = Zeroizing::new(value);
        parse_record(&key, &value, &mut wallet, &mut names, &mut purposes)?;
    }
    for (address, label) in names {
        let purpose = purposes.remove(&address);
        wallet.address_book.push(AddressBookEntry {
            address,
            label,
            purpose,
        });
    }
    Ok(wallet)
}

fn utf8(b: &[u8], what: &str) -> Result<String, WalletDatError> {
    String::from_utf8(b.to_vec()).map_err(|_| corrupt(format!("{what} is not UTF-8")))
}

fn parse_record(
    key: &[u8],
    value: &[u8],
    w: &mut SqliteWallet,
    names: &mut BTreeMap<String, String>,
    purposes: &mut BTreeMap<String, String>,
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
                names.insert(address, text);
            } else {
                purposes.insert(address, text);
            }
        }
        _ => {}
    }
    Ok(())
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
}
