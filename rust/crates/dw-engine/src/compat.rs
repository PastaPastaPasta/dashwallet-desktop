//! dash-qt compatibility (R2, M2; docs/contracts/m2-engine.md §2.6):
//! recognising user-chosen files, importing Dash Core wallets (dumpwallet,
//! SQLite descriptor `wallet.dat`, HD seed, `listdescriptors`) and writing
//! files dash-qt imports (dumpwallet, `importdescriptors` JSON).
//!
//! Every import goes through [`NetworkSession::import_secret_with`], so the
//! seed is in the vault and read back before the wallet is registered. A
//! phrase from a Dash Core file is always derived with Core's BIP39 rules
//! (weak checksum, no NFKD, salt cut at 256 bytes) and checked against the
//! file's master key or HD seed before anything is stored.
//!
//! What platform-wallet cannot hold is refused with a typed
//! `NotImplemented`, never approximated: a master xprv without its seed
//! (platform-wallet registers wallets from 64-byte seeds only), seeds of
//! other lengths, and Berkeley DB `wallet.dat` files (M6).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use dashcore::secp256k1::Secp256k1;
use dw_appdb::BookPurpose;
use dw_compat::descriptor::{ImportRequest, PkhDescriptor, import_descriptors_json};
use dw_compat::dump::{self, DumpFile, DumpHeader, HdAccount, KeyEntry, KeyRole};
use dw_compat::walletdat::{self, Container, WalletDatError};
use dw_uri::keyio;
use dw_vault::{GrantKind, SeedDerivation, WalletSecret, mnemonic};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use zeroize::Zeroizing;

use crate::events::unix_now;
use crate::history::AddressChain;
use crate::keys::mnemonic_error;
use crate::receive::AddressFilter;
use crate::{
    CORE_COMPAT_LOOKAHEAD, DashNetwork, EngineError, ImportOptions, MAX_LOOKAHEAD, NetworkSession,
    WalletId,
};

/// Largest dumpwallet file read (a wallet with a few hundred thousand keys).
const MAX_DUMP_BYTES: u64 = 256 * 1024 * 1024;
/// Bytes read to recognise a file.
const SNIFF_BYTES: usize = 4096;

/// Why a compatibility call failed (`compat.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompatFailure {
    /// Missing, unreadable or a directory.
    FileUnreadable(String),
    /// Not a format this call imports.
    UnsupportedFormat(String),
    /// The file parses only partly, or its keys disagree with each other.
    Corrupt(String),
    /// An encrypted wallet.dat needs its passphrase.
    PassphraseRequired,
    WrongPassphrase,
    /// No HD chain: only loose keys (sweep, M5).
    NoHdChain,
    /// Keys of another network. `mainnet`: the keys are mainnet keys
    /// (otherwise test-network keys, which testnet, devnet and regtest
    /// share).
    NetworkMismatch {
        mainnet: bool,
    },
    /// Bad seed length, xprv or JSON.
    InvalidKeyMaterial(String),
    /// A watch-only wallet has no keys to export.
    WatchOnly,
    /// The export file exists or cannot be written.
    DestinationUnwritable(String),
}

impl std::fmt::Display for CompatFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileUnreadable(d) => write!(f, "file unreadable: {d}"),
            Self::UnsupportedFormat(d) => write!(f, "unsupported format: {d}"),
            Self::Corrupt(d) => write!(f, "corrupt: {d}"),
            Self::PassphraseRequired => f.write_str("passphrase required"),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
            Self::NoHdChain => f.write_str("no HD chain"),
            Self::NetworkMismatch { mainnet } => {
                write!(
                    f,
                    "keys are for {}",
                    if *mainnet {
                        "mainnet"
                    } else {
                        "a test network"
                    }
                )
            }
            Self::InvalidKeyMaterial(d) => write!(f, "invalid key material: {d}"),
            Self::WatchOnly => f.write_str("watch-only wallet"),
            Self::DestinationUnwritable(d) => write!(f, "cannot write: {d}"),
        }
    }
}

impl From<CompatFailure> for EngineError {
    fn from(f: CompatFailure) -> Self {
        EngineError::Compat(f)
    }
}

/// What a user-chosen file is (QT-106/107/110). Detected from content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalletFileKind {
    DumpWallet {
        /// `Mainnet` or `Testnet` from the key prefixes (test networks share
        /// them); `None` when the file has no keys.
        network: Option<DashNetwork>,
        has_mnemonic: bool,
        has_hd_seed: bool,
        has_xprv: bool,
        loose_key_count: u32,
        script_count: u32,
        label_count: u32,
    },
    WalletDatSqlite {
        encrypted: bool,
        has_mnemonic: bool,
    },
    /// Berkeley DB: encryption cannot be told without the M6 reader.
    WalletDatBdb {
        encrypted: Option<bool>,
    },
    DwBackup {
        network: Option<DashNetwork>,
        wallet_count: u32,
        created_at: u64,
        format_version: u32,
    },
    Psbt,
    Unknown,
}

/// Result of an import (QT-106…108).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    pub wallet_id: WalletId,
    pub labels_imported: u32,
    pub keys_not_imported: u32,
    pub scripts_not_imported: u32,
    /// The seed was derived with Dash Core's BIP39 rules.
    pub core_compat_seed: bool,
}

/// Raw key material (QT-108). Every payload is zeroized.
pub enum KeyMaterial {
    /// BIP32 seed bytes.
    HdSeed(Zeroizing<Vec<u8>>),
    /// Master xprv text.
    Xprv(Zeroizing<Vec<u8>>),
    /// `listdescriptors true` JSON.
    Descriptors(Zeroizing<Vec<u8>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreExportFormat {
    DumpWallet,
    ImportDescriptorsJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExportWarning {
    MnemonicNotCoreCompatible,
    CoinJoinAccountNotScannedByLegacyCore,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportReport {
    pub path: PathBuf,
    pub format: CoreExportFormat,
    pub key_count: u32,
    pub warnings: Vec<ExportWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreMnemonicCompatibility {
    pub core_compatible: bool,
    pub warnings: Vec<ExportWarning>,
}

fn unreadable(e: impl std::fmt::Display) -> EngineError {
    CompatFailure::FileUnreadable(e.to_string()).into()
}

/// Reads at most `limit` bytes of `path`; a directory or a larger file is
/// unreadable.
fn read_file(path: &Path, limit: u64) -> Result<Zeroizing<Vec<u8>>, EngineError> {
    let meta = std::fs::metadata(path).map_err(unreadable)?;
    if !meta.is_file() {
        return Err(unreadable(format!("{} is not a file", path.display())));
    }
    if meta.len() > limit {
        return Err(unreadable(format!(
            "{} is over {limit} bytes",
            path.display()
        )));
    }
    let mut buf = Zeroizing::new(Vec::with_capacity(meta.len() as usize));
    std::fs::File::open(path)
        .and_then(|f| f.take(limit).read_to_end(&mut buf))
        .map_err(unreadable)?;
    Ok(buf)
}

fn head(path: &Path) -> Result<Vec<u8>, EngineError> {
    let meta = std::fs::metadata(path).map_err(unreadable)?;
    if !meta.is_file() {
        return Err(unreadable(format!("{} is not a file", path.display())));
    }
    let mut buf = Vec::with_capacity(SNIFF_BYTES);
    std::fs::File::open(path)
        .and_then(|f| f.take(SNIFF_BYTES as u64).read_to_end(&mut buf))
        .map_err(unreadable)?;
    Ok(buf)
}

/// Whether a WIF / extended key string is a mainnet key; `None` when it is
/// neither a mainnet nor a test-network key.
fn key_is_mainnet(text: &str) -> Option<bool> {
    if text.starts_with("xprv") || text.starts_with("xpub") {
        return Some(true);
    }
    if text.starts_with("tprv") || text.starts_with("tpub") {
        return Some(false);
    }
    if keyio::decode_secret(text, dashcore::Network::Mainnet).is_some() {
        return Some(true);
    }
    keyio::decode_secret(text, dashcore::Network::Testnet).map(|_| false)
}

fn dump_network(dump: &DumpFile) -> Option<bool> {
    dump.hd
        .as_ref()
        .and_then(|hd| key_is_mainnet(&hd.xprv))
        .or_else(|| dump.keys.iter().find_map(|k| key_is_mainnet(&k.wif)))
}

fn network_of(mainnet: bool) -> DashNetwork {
    if mainnet {
        DashNetwork::Mainnet
    } else {
        DashNetwork::Testnet
    }
}

/// Recognises a user-chosen file from its content (QT-106/107/110). Reads
/// what detection needs; decrypts nothing.
pub fn inspect_wallet_file(path: &Path) -> Result<WalletFileKind, EngineError> {
    let start = head(path)?;
    if start.starts_with(crate::backup::MAGIC) {
        return Ok(match crate::backup::read_header(path) {
            Ok(h) => WalletFileKind::DwBackup {
                network: h.network(),
                wallet_count: h.wallet_ids.len() as u32,
                created_at: h.created_at,
                format_version: h.format_version,
            },
            Err(_) => WalletFileKind::Unknown,
        });
    }
    match walletdat::sniff_container(&start) {
        Some(Container::Sqlite) => {
            return Ok(match walletdat::read_sqlite(path) {
                Ok(w) if w.is_descriptor_wallet() => WalletFileKind::WalletDatSqlite {
                    encrypted: w.encrypted(),
                    has_mnemonic: w.has_mnemonic(),
                },
                _ => WalletFileKind::Unknown,
            });
        }
        Some(Container::BerkeleyDb) => {
            return Ok(WalletFileKind::WalletDatBdb { encrypted: None });
        }
        None => {}
    }
    if start.starts_with(b"psbt\xff") || start.starts_with(b"cHNidP") {
        return Ok(WalletFileKind::Psbt);
    }
    if start.starts_with(b"# Wallet dump created by") {
        let bytes = read_file(path, MAX_DUMP_BYTES)?;
        let text = String::from_utf8_lossy(&bytes);
        let Ok(dump) = dump::parse(&text) else {
            return Ok(WalletFileKind::Unknown);
        };
        let hd = dump.hd.as_ref();
        return Ok(WalletFileKind::DumpWallet {
            network: dump_network(&dump).map(network_of),
            has_mnemonic: hd.is_some_and(|h| !h.mnemonic.is_empty()),
            has_hd_seed: hd.is_some_and(|h| !h.hd_seed_hex.is_empty()),
            has_xprv: hd.is_some_and(|h| !h.xprv.is_empty()),
            loose_key_count: dump.keys.iter().filter(|k| !k.is_hd()).count() as u32,
            script_count: dump.scripts.len() as u32,
            label_count: dump
                .keys
                .iter()
                .filter(|k| matches!(&k.role, KeyRole::Label(l) if !l.is_empty()))
                .count() as u32,
        });
    }
    Ok(WalletFileKind::Unknown)
}

/// A wallet's root as found in a Dash Core file, before it is imported.
struct CoreRoot {
    /// Phrase and passphrase bytes as Core stored them.
    phrase: Option<dw_compat::walletdat::PhraseAndPassphrase>,
    /// The 64-byte HD seed, when the file has one and no phrase.
    seed: Option<Zeroizing<[u8; 64]>>,
    /// Compressed public key of the BIP32 master key the file's keys derive
    /// from, to check the phrase or seed against.
    master_pubkey: Option<[u8; 33]>,
    /// Addresses the wallet handed out on its busiest chain.
    used_per_chain: u32,
    labels: Vec<(String, String, BookPurpose)>,
}

/// A BIP32 master key whose private key is erased when it goes out of
/// scope, on every return path (review L4).
struct MasterKey(ExtendedPrivKey);

impl MasterKey {
    fn new(network: dashcore::Network, seed: &[u8]) -> Result<Self, EngineError> {
        ExtendedPrivKey::new_master(network, seed)
            .map(Self)
            .map_err(|e| EngineError::Internal(format!("master key: {e}")))
    }

    fn erase(&mut self) {
        self.0.private_key.non_secure_erase();
    }
}

impl std::ops::Deref for MasterKey {
    type Target = ExtendedPrivKey;
    fn deref(&self) -> &ExtendedPrivKey {
        &self.0
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.erase();
    }
}

fn master_pubkey(seed: &[u8; 64]) -> Option<[u8; 33]> {
    let master = MasterKey::new(dashcore::Network::Mainnet, seed).ok()?;
    Some(master.private_key.public_key(&Secp256k1::new()).serialize())
}

/// The lookahead of a Core import: Core's 1000 (QT-105) or more when the
/// wallet handed out more addresses, up to key-wallet's maximum.
fn core_lookahead(options: &ImportOptions, used: u32) -> u32 {
    options
        .lookahead
        .unwrap_or_else(|| CORE_COMPAT_LOOKAHEAD.max(used.saturating_add(100)))
        .min(MAX_LOOKAHEAD)
}

impl NetworkSession {
    fn check_key_network(&self, mainnet: Option<bool>) -> Result<(), EngineError> {
        let ours = self.network == DashNetwork::Mainnet;
        match mainnet {
            Some(m) if m != ours => Err(CompatFailure::NetworkMismatch { mainnet: m }.into()),
            _ => Ok(()),
        }
    }

    /// Imports a Dash Core wallet root: derives (or takes) the seed, checks
    /// it against the file's master key, registers the wallet in seed-safety
    /// order and carries the labels into the address book. The report counts
    /// no keys or scripts left out; callers that leave some out set them.
    async fn import_core_root(
        self: &Arc<Self>,
        root: CoreRoot,
        options: ImportOptions,
    ) -> Result<ImportReport, EngineError> {
        let options = ImportOptions {
            // A restored wallet may have funds anywhere in the chain.
            birth_height: options.birth_height.or(Some(0)),
            core_compat: true,
            lookahead: Some(core_lookahead(&options, root.used_per_chain)),
            name: options.name,
        };
        let CoreRoot {
            phrase,
            seed,
            master_pubkey: expected,
            labels,
            ..
        } = root;
        let core_compat_seed = phrase.is_some();
        let make_secret = move || -> Result<WalletSecret, EngineError> {
            let secret = match (phrase, seed) {
                (Some((phrase, pass)), _) => {
                    mnemonic::derive_secret(&phrase, &pass, true).map_err(mnemonic_error)?
                }
                (None, Some(seed)) => WalletSecret {
                    mnemonic: Zeroizing::new(Vec::new()),
                    mnemonic_passphrase: Zeroizing::new(Vec::new()),
                    seed,
                    derivation: SeedDerivation::RawSeed,
                },
                (None, None) => return Err(CompatFailure::NoHdChain.into()),
            };
            if let Some(want) = expected
                && master_pubkey(&secret.seed) != Some(want)
            {
                return Err(CompatFailure::Corrupt(
                    "the phrase or seed does not derive the file's master key".into(),
                )
                .into());
            }
            Ok(secret)
        };
        let wallet_id = self.import_secret_with(options, make_secret).await?;
        let labels_imported = self.import_labels(wallet_id, labels).await;
        Ok(ImportReport {
            wallet_id,
            labels_imported,
            keys_not_imported: 0,
            scripts_not_imported: 0,
            core_compat_seed,
        })
    }

    /// Adds address-book entries; entries the wallet refuses (an address
    /// outside its derived range, a duplicate) are skipped. Returns how many
    /// were stored.
    async fn import_labels(
        self: &Arc<Self>,
        id: WalletId,
        labels: Vec<(String, String, BookPurpose)>,
    ) -> u32 {
        let mut stored = 0;
        for (address, label, purpose) in labels {
            match self
                .save_address_book_entry(id, address.clone(), label, purpose, true)
                .await
            {
                Ok(_) => stored += 1,
                Err(e) => tracing::info!(%address, error = %e, "address-book entry not imported"),
            }
        }
        stored
    }

    /// dash-qt "Import dumpwallet" (QT-107). See the module doc.
    pub async fn import_dump_wallet(
        self: &Arc<Self>,
        path: PathBuf,
        options: ImportOptions,
    ) -> Result<ImportReport, EngineError> {
        drop(self.try_enter()?);
        let dump = self
            .on_runtime(async move {
                tokio::task::spawn_blocking(move || -> Result<DumpFile, EngineError> {
                    let bytes = read_file(&path, MAX_DUMP_BYTES)?;
                    let text = std::str::from_utf8(&bytes)
                        .map_err(|_| CompatFailure::Corrupt("dump file is not UTF-8".into()))?;
                    dump::parse(text).map_err(|e| {
                        if e.line == 1 {
                            CompatFailure::UnsupportedFormat(e.to_string())
                        } else {
                            CompatFailure::Corrupt(e.to_string())
                        }
                        .into()
                    })
                })
                .await?
            })
            .await?;
        self.check_key_network(dump_network(&dump))?;
        let hd = dump.hd.as_ref().ok_or(CompatFailure::NoHdChain)?;
        let seed = if hd.hd_seed_hex.is_empty() {
            None
        } else {
            let raw = Zeroizing::new(
                hex::decode(hd.hd_seed_hex.as_bytes())
                    .map_err(|_| CompatFailure::Corrupt("HD seed is not hex".into()))?,
            );
            let arr: [u8; 64] = raw[..].try_into().map_err(|_| {
                EngineError::NotImplemented("import_dump_wallet.seed_length".into())
            })?;
            Some(Zeroizing::new(arr))
        };
        let phrase = (!hd.mnemonic.is_empty()).then(|| {
            (
                Zeroizing::new(hd.mnemonic.as_bytes().to_vec()),
                Zeroizing::new(hd.mnemonic_passphrase.as_bytes().to_vec()),
            )
        });
        if phrase.is_none() && seed.is_none() {
            return Err(EngineError::NotImplemented(
                "import_dump_wallet.xprv".into(),
            ));
        }
        // The xprv in the header must be the master key of the seed.
        let expected = ExtendedPrivKey::from_str(&hd.xprv)
            .map_err(|e| CompatFailure::Corrupt(format!("extended private masterkey: {e}")))?
            .private_key
            .public_key(&Secp256k1::new())
            .serialize();
        if let (Some(seed), Some(_)) = (&seed, &phrase)
            && master_pubkey(seed) != Some(expected)
        {
            return Err(CompatFailure::Corrupt("HD seed and master key disagree".into()).into());
        }
        let used_per_chain = hd
            .accounts
            .iter()
            .filter_map(|a| match a {
                HdAccount::Counters { external, internal } => Some((*external).max(*internal)),
                HdAccount::Missing => None,
            })
            .max()
            .unwrap_or(0);
        let labels = dump
            .keys
            .iter()
            .filter(|k| k.is_hd())
            .filter_map(|k| match &k.role {
                KeyRole::Label(l) => Some((k.address.clone(), l.clone(), BookPurpose::Receive)),
                _ => None,
            })
            .collect();
        let report_keys = dump.keys.iter().filter(|k| !k.is_hd()).count() as u32;
        let report_scripts = dump.scripts.len() as u32;
        let root = CoreRoot {
            seed: if phrase.is_some() { None } else { seed },
            phrase,
            master_pubkey: Some(expected),
            used_per_chain,
            labels,
        };
        let report = self.import_core_root(root, options).await?;
        Ok(ImportReport {
            keys_not_imported: report_keys,
            scripts_not_imported: report_scripts,
            ..report
        })
    }

    /// dash-qt "Restore wallet" from a `wallet.dat` (QT-106): SQLite
    /// descriptor wallets; Berkeley DB returns `NotImplemented` until M6.
    pub async fn import_wallet_dat(
        self: &Arc<Self>,
        path: PathBuf,
        wallet_passphrase: Option<Zeroizing<Vec<u8>>>,
        options: ImportOptions,
    ) -> Result<ImportReport, EngineError> {
        drop(self.try_enter()?);
        let network_mainnet = self.network == DashNetwork::Mainnet;
        let (root, coinjoin_salt) = self
            .on_runtime(async move {
                tokio::task::spawn_blocking(
                    move || -> Result<(CoreRoot, Option<[u8; 32]>), EngineError> {
                        match walletdat::sniff_container(&head(&path)?) {
                            Some(Container::Sqlite) => {}
                            Some(Container::BerkeleyDb) => {
                                return Err(EngineError::NotImplemented(
                                    "import_wallet_dat.bdb".into(),
                                ));
                            }
                            None => {
                                return Err(CompatFailure::UnsupportedFormat(
                                    "not a wallet.dat".into(),
                                )
                                .into());
                            }
                        }
                        let wallet = walletdat::read_sqlite(&path).map_err(walletdat_failure)?;
                        let root = wallet
                            .hd_root(wallet_passphrase.as_deref().map(|p| &p[..]))
                            .map_err(walletdat_failure)?;
                        let mainnet = key_is_mainnet(&root.external.key);
                        if mainnet.is_some_and(|m| m != network_mainnet) {
                            return Err(CompatFailure::NetworkMismatch {
                                mainnet: mainnet == Some(true),
                            }
                            .into());
                        }
                        let Some(phrase) = root.mnemonic else {
                            return Err(EngineError::NotImplemented(
                                "import_wallet_dat.xprv".into(),
                            ));
                        };
                        let labels = wallet
                            .address_book
                            .iter()
                            .map(|e| {
                                let purpose = if e.purpose.as_deref() == Some("send") {
                                    BookPurpose::Send
                                } else {
                                    BookPurpose::Receive
                                };
                                (e.address.clone(), e.label.clone(), purpose)
                            })
                            .collect();
                        Ok((
                            CoreRoot {
                                phrase: Some(phrase),
                                seed: None,
                                master_pubkey: Some(root.master_pubkey),
                                used_per_chain: wallet.next_index(),
                                labels,
                            },
                            wallet.coinjoin_salt,
                        ))
                    },
                )
                .await?
            })
            .await?;
        let report = self.import_core_root(root, options).await?;
        // The wallet keeps Dash Core's salt, so the same coins count as
        // fully mixed (QT-043, m3-engine.md §5).
        if let Some(salt) = coinjoin_salt {
            let this = Arc::clone(self);
            let id = report.wallet_id;
            self.on_runtime(async move { this.import_coinjoin_salt(id, salt).await })
                .await?;
        }
        Ok(report)
    }

    /// Imports a wallet from an HD seed, xprv or `listdescriptors true` JSON
    /// (QT-108).
    pub async fn import_key_material(
        self: &Arc<Self>,
        material: KeyMaterial,
        options: ImportOptions,
    ) -> Result<ImportReport, EngineError> {
        drop(self.try_enter()?);
        let root = match material {
            KeyMaterial::HdSeed(seed) => {
                if !(16..=64).contains(&seed.len()) {
                    return Err(CompatFailure::InvalidKeyMaterial(format!(
                        "an HD seed is 16 to 64 bytes, got {}",
                        seed.len()
                    ))
                    .into());
                }
                let arr: [u8; 64] = seed[..].try_into().map_err(|_| {
                    EngineError::NotImplemented("import_key_material.seed_length".into())
                })?;
                CoreRoot {
                    phrase: None,
                    seed: Some(Zeroizing::new(arr)),
                    master_pubkey: None,
                    used_per_chain: 0,
                    labels: Vec::new(),
                }
            }
            KeyMaterial::Xprv(text) => {
                let text = std::str::from_utf8(&text)
                    .map_err(|_| CompatFailure::InvalidKeyMaterial("xprv is not text".into()))?;
                let xprv = ExtendedPrivKey::from_str(text.trim()).map_err(|e| {
                    CompatFailure::InvalidKeyMaterial(format!("not an extended private key: {e}"))
                })?;
                self.check_key_network(key_is_mainnet(text.trim()))?;
                if xprv.depth != 0 {
                    return Err(CompatFailure::InvalidKeyMaterial(
                        "not a master key (depth is not 0)".into(),
                    )
                    .into());
                }
                return Err(EngineError::NotImplemented(
                    "import_key_material.xprv".into(),
                ));
            }
            KeyMaterial::Descriptors(json) => {
                let listed = dw_compat::descriptor::parse_listdescriptors(&json)
                    .map_err(|e| CompatFailure::InvalidKeyMaterial(e.to_string()))?;
                self.check_key_network(key_is_mainnet(&listed.external.key))?;
                let master = ExtendedPrivKey::from_str(&listed.external.key).map_err(|e| {
                    CompatFailure::InvalidKeyMaterial(format!("descriptor key: {e}"))
                })?;
                if master.depth != 0 {
                    return Err(CompatFailure::InvalidKeyMaterial(
                        "descriptor key is not a master key".into(),
                    )
                    .into());
                }
                let Some((phrase, pass)) = listed.mnemonic else {
                    return Err(EngineError::NotImplemented(
                        "import_key_material.xprv".into(),
                    ));
                };
                CoreRoot {
                    phrase: Some((
                        Zeroizing::new(phrase.as_bytes().to_vec()),
                        Zeroizing::new(pass.as_bytes().to_vec()),
                    )),
                    seed: None,
                    master_pubkey: Some(
                        master.private_key.public_key(&Secp256k1::new()).serialize(),
                    ),
                    used_per_chain: listed.next_index,
                    labels: Vec::new(),
                }
            }
        };
        self.import_core_root(root, options).await
    }

    /// The warning for CoinJoin funds a legacy dash-qt wallet would not see.
    fn coinjoin_warning(&self, id: &WalletId) -> Option<ExportWarning> {
        self.hub
            .wallet_state(id)
            .and_then(|s| s.balances())
            .is_some_and(|b| b.coinjoin > 0)
            .then_some(ExportWarning::CoinJoinAccountNotScannedByLegacyCore)
    }

    /// Whether "phrase + passphrase + `upgradetohd`" in dash-qt rebuilds the
    /// wallet (QT-109 a). Needs the data key (unlocked or unencrypted vault).
    pub async fn core_mnemonic_compatibility(
        self: &Arc<Self>,
        wallet_id: WalletId,
    ) -> Result<CoreMnemonicCompatibility, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_wallet(&wallet_id)?;
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(CompatFailure::WatchOnly.into());
            }
            let vault = this.vault.clone();
            let check =
                tokio::task::spawn_blocking(move || vault.core_mnemonic_check(&wallet_id.0))
                    .await??;
            let mut warnings = Vec::new();
            if !check.core_compatible {
                warnings.push(ExportWarning::MnemonicNotCoreCompatible);
            }
            warnings.extend(this.coinjoin_warning(&wallet_id));
            Ok(CoreMnemonicCompatibility {
                core_compatible: check.core_compatible,
                warnings,
            })
        })
        .await
    }

    /// Writes the wallet for dash-qt (QT-109 b/c) to `dest`, mode 0600,
    /// never over an existing file. Needs a `RevealSecret` grant for the
    /// wallet, redeemed after every check that does not need the keys.
    pub async fn export_for_core(
        self: &Arc<Self>,
        wallet_id: WalletId,
        format: CoreExportFormat,
        dest: PathBuf,
        grant_id: String,
    ) -> Result<ExportReport, EngineError> {
        drop(self.try_enter()?);
        self.require_wallet(&wallet_id)?;
        if !self.vault.has_wallet_secret(&wallet_id.0) {
            return Err(CompatFailure::WatchOnly.into());
        }
        check_destination(&dest)?;
        let addresses = self.addresses(wallet_id, AddressFilter::default()).await?;
        let book = self.address_book(wallet_id, None, None).await?;
        let scan_height = self.wallet_scan_height(&wallet_id);
        let tip_time = self.sync_snapshot().ok().and_then(|s| s.tip_time);
        let coinjoin_warning = self.coinjoin_warning(&wallet_id);
        let network = self.network.core_network();
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.vault
                .check_grant(&grant_id, GrantKind::RevealSecret, Some(&wallet_id.0))
                .map_err(|e| match e {
                    dw_vault::VaultError::GrantInvalid
                    | dw_vault::VaultError::GrantPurposeMismatch => {
                        EngineError::Vault(dw_vault::VaultError::GrantInvalid)
                    }
                    other => EngineError::Vault(other),
                })?;
            let vault = this.vault.clone();
            tokio::task::spawn_blocking(move || -> Result<ExportReport, EngineError> {
                let token =
                    vault.redeem_grant(&grant_id, GrantKind::RevealSecret, Some(&wallet_id.0))?;
                let secret = vault.export_wallet_secret(&wallet_id.0, &token)?;
                drop(token);
                let master = MasterKey::new(network, &secret.seed[..])?;
                let mut warnings = Vec::new();
                let (text, key_count) = match format {
                    CoreExportFormat::DumpWallet => {
                        warnings.extend(coinjoin_warning);
                        let labels: std::collections::HashMap<String, String> = book
                            .iter()
                            .map(|e| (e.address.clone(), e.label.clone()))
                            .collect();
                        let dump = dump_of(
                            &secret,
                            &master,
                            &addresses,
                            &labels,
                            DumpHeader {
                                created_by: format!(
                                    "dashwallet-desktop {}",
                                    env!("CARGO_PKG_VERSION")
                                ),
                                created_on: dump::format_iso8601(unix_now() as i64),
                                best_block_height: scan_height.map_or(0, i64::from),
                                // The engine keeps no block hashes; the line
                                // says so instead of inventing one.
                                best_block_hash: "unknown".into(),
                                best_block_time: tip_time.map_or_else(
                                    || "unknown".into(),
                                    |t| dump::format_iso8601(t as i64),
                                ),
                            },
                            network,
                            &mut warnings,
                        )?;
                        let n = dump.keys.len() as u32;
                        (dump::write(&dump, network), n)
                    }
                    CoreExportFormat::ImportDescriptorsJson => {
                        (descriptors_of(&master, &addresses, network), 1)
                    }
                };
                write_new_private(&dest, text.as_bytes())?;
                Ok(ExportReport {
                    path: dest,
                    format,
                    key_count,
                    warnings,
                })
            })
            .await?
        })
        .await
    }
}

fn walletdat_failure(e: WalletDatError) -> EngineError {
    match e {
        WalletDatError::Unreadable(d) => CompatFailure::FileUnreadable(d),
        WalletDatError::Unsupported(d) => CompatFailure::UnsupportedFormat(d),
        WalletDatError::Corrupt(d) => CompatFailure::Corrupt(d),
        WalletDatError::PassphraseRequired => CompatFailure::PassphraseRequired,
        WalletDatError::WrongPassphrase => CompatFailure::WrongPassphrase,
    }
    .into()
}

/// `dest` must not exist and its directory must exist.
fn check_destination(dest: &Path) -> Result<(), EngineError> {
    if dest.as_os_str().is_empty() {
        return Err(EngineError::InvalidArgument(
            "empty destination path".into(),
        ));
    }
    if dest.exists() {
        return Err(CompatFailure::DestinationUnwritable(format!(
            "{} exists; exports replace no file",
            dest.display()
        ))
        .into());
    }
    match dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) if !dir.is_dir() => Err(CompatFailure::DestinationUnwritable(format!(
            "{} is not a directory",
            dir.display()
        ))
        .into()),
        _ => Ok(()),
    }
}

/// Creates `dest` (failing if it exists) with mode 0600 and writes `bytes`.
/// A failed write removes the partial file.
pub(crate) fn write_new_private(dest: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let unwritable = |e: std::io::Error| CompatFailure::DestinationUnwritable(e.to_string());
    let mut f = opts.open(dest).map_err(unwritable)?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(dest);
        return Err(unwritable(e).into());
    }
    Ok(())
}

/// Dash Core's dumpwallet of the wallet's BIP44 account 0 keys: the HD
/// header (phrase, passphrase, seed, master keys, chain counters) and one
/// line per derived key, labelled as Core labels them (address-book entry →
/// `label=`, change chain → `change=1`, unused receive key → `reserve=1`).
/// Key birth times are written as Core writes keys without metadata
/// (1970-01-01T00:00:01Z), so `importwallet` rescans from genesis.
fn dump_of(
    secret: &WalletSecret,
    master: &ExtendedPrivKey,
    addresses: &[crate::receive::AddressInfo],
    labels: &std::collections::HashMap<String, String>,
    header: DumpHeader,
    network: dashcore::Network,
    warnings: &mut Vec<ExportWarning>,
) -> Result<DumpFile, EngineError> {
    let secp = Secp256k1::new();
    let mut keys = Vec::with_capacity(addresses.len());
    let (mut external, mut internal) = (0u32, 0u32);
    for a in addresses {
        let path = DerivationPath::from_str(&a.derivation_path)
            .map_err(|e| EngineError::Internal(format!("path {}: {e}", a.derivation_path)))?;
        let mut child = master
            .derive_priv(&secp, &path)
            .map_err(|e| EngineError::Internal(format!("derive {path}: {e}")))?;
        let wif = keyio::encode_secret(
            &keyio::Secret {
                key: Zeroizing::new(child.private_key.secret_bytes()),
                compressed: true,
            },
            network,
        );
        child.private_key.non_secure_erase();
        let role = match (a.chain, labels.get(&a.address)) {
            (_, Some(l)) => KeyRole::Label(l.clone()),
            (AddressChain::Change, None) => KeyRole::Change,
            (_, None) if a.used => KeyRole::Label(String::new()),
            (_, None) => KeyRole::Reserve,
        };
        match a.chain {
            AddressChain::Change => internal = internal.max(a.index + 1),
            _ => external = external.max(a.index + 1),
        }
        keys.push(KeyEntry {
            wif,
            time: dump::format_iso8601(1),
            role,
            address: a.address.clone(),
            hdkeypath: Some(a.derivation_path.clone()),
        });
    }
    let text = |b: &[u8], warnings: &mut Vec<ExportWarning>| match std::str::from_utf8(b) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            warnings.push(ExportWarning::MnemonicNotCoreCompatible);
            String::from_utf8_lossy(b).into_owned()
        }
    };
    let phrase = Zeroizing::new(text(&secret.mnemonic, warnings));
    let passphrase = Zeroizing::new(text(&secret.mnemonic_passphrase, warnings));
    let hd = dump::hd_section_from_seed(
        &phrase,
        &passphrase,
        &secret.seed[..],
        vec![HdAccount::Counters { external, internal }],
        network,
    )
    .map_err(|e| EngineError::Internal(format!("HD section: {e}")))?;
    if hd.xprv.as_str() != master.to_string() {
        return Err(EngineError::Internal("master key mismatch".into()));
    }
    Ok(DumpFile {
        header,
        hd: Some(hd),
        keys,
        scripts: Vec::new(),
    })
}

/// `importdescriptors` requests for a dashd descriptor wallet: the BIP44
/// account 0 receive and change chains (active) and the CoinJoin chain
/// (inactive, as Core lists it), from the master key, timestamp 0 (the
/// birth time is not known, so dashd rescans from genesis).
fn descriptors_of(
    master: &ExtendedPrivKey,
    addresses: &[crate::receive::AddressInfo],
    network: dashcore::Network,
) -> Zeroizing<String> {
    let coin = if network == dashcore::Network::Mainnet {
        5
    } else {
        1
    };
    let count = |chain: AddressChain, used_only: bool| {
        addresses
            .iter()
            .filter(|a| a.chain == chain && (!used_only || a.used))
            .map(|a| a.index + 1)
            .max()
            .unwrap_or(0)
    };
    let key = Zeroizing::new(master.to_string());
    let request =
        |path: String, active: bool, internal: bool, derived: u32, used: u32| ImportRequest {
            descriptor: PkhDescriptor {
                key: key.clone(),
                path,
            },
            timestamp: Some(0),
            active,
            internal,
            range_end: derived.max(CORE_COMPAT_LOOKAHEAD) - 1,
            next_index: used,
        };
    import_descriptors_json(&[
        request(
            format!("44h/{coin}h/0h/0"),
            true,
            false,
            count(AddressChain::Receiving, false),
            count(AddressChain::Receiving, true),
        ),
        request(
            format!("44h/{coin}h/0h/1"),
            true,
            true,
            count(AddressChain::Change, false),
            count(AddressChain::Change, true),
        ),
        request(format!("9h/{coin}h/4h/0h/0"), false, false, 0, 0),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn testdata(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata")
            .join(rel)
    }

    /// Review L4: what the guard's drop runs replaces the private key.
    #[test]
    fn master_key_guard_erases_the_private_key() {
        let mut m = MasterKey::new(dashcore::Network::Testnet, &[7; 64]).unwrap();
        let before = m.private_key.secret_bytes();
        m.erase();
        assert_ne!(m.private_key.secret_bytes(), before);
    }

    #[test]
    fn inspects_core_files() {
        match inspect_wallet_file(&testdata("dumpwallet/dump_hd_basic.txt")).unwrap() {
            WalletFileKind::DumpWallet {
                network,
                has_mnemonic,
                has_hd_seed,
                loose_key_count,
                script_count,
                label_count,
                ..
            } => {
                assert_eq!(network, Some(DashNetwork::Testnet));
                assert!(has_mnemonic && has_hd_seed);
                assert_eq!(loose_key_count, 2);
                assert_eq!(script_count, 2); // the multisig and its P2SH wrapper
                assert!(label_count >= 3);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            inspect_wallet_file(&testdata("compat/walletdat/desc_encrypted.dat")).unwrap(),
            WalletFileKind::WalletDatSqlite {
                encrypted: true,
                has_mnemonic: true
            }
        );
        assert_eq!(
            inspect_wallet_file(&testdata("compat/walletdat/bdb_legacy_head.dat")).unwrap(),
            WalletFileKind::WalletDatBdb { encrypted: None }
        );
        assert_eq!(
            inspect_wallet_file(&testdata("compat/psbt/unsigned.b64")).unwrap(),
            WalletFileKind::Psbt
        );
        assert_eq!(
            inspect_wallet_file(&testdata("compat/manifest.json")).unwrap(),
            WalletFileKind::Unknown
        );
        assert!(matches!(
            inspect_wallet_file(&testdata("compat")),
            Err(EngineError::Compat(CompatFailure::FileUnreadable(_)))
        ));
    }

    #[test]
    fn key_prefixes_tell_the_network() {
        assert_eq!(key_is_mainnet("xprv9s21ZrQH143K"), Some(true));
        assert_eq!(key_is_mainnet("tprv8ZgxMBicQKsP"), Some(false));
        assert_eq!(
            key_is_mainnet("cMec2DGaTXkYJYfi7x3ZGjRXkeqmAvYAoWzMAcWj5fdLaqudWsNi"),
            Some(false)
        );
        assert_eq!(key_is_mainnet("garbage"), None);
    }

    #[test]
    fn destinations_are_never_replaced() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("out.txt");
        write_new_private(&f, b"x").unwrap();
        assert!(matches!(
            check_destination(&f),
            Err(EngineError::Compat(CompatFailure::DestinationUnwritable(_)))
        ));
        assert!(write_new_private(&f, b"y").is_err());
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert!(check_destination(&dir.path().join("missing/out.txt")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
