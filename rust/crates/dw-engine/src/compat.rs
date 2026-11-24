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
//! (platform-wallet registers wallets from 64-byte seeds only) and seeds of
//! other lengths.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use dw_appdb::BookPurpose;
use dw_compat::descriptor::{ImportRequest, PkhDescriptor, import_descriptors_json};
use dw_compat::dump::{self, DumpFile, DumpHeader, HdAccount, KeyEntry, KeyRole};
use dw_compat::walletdat::{self, Container, WalletDatError};
use dw_uri::keyio;
use dw_vault::{GrantKind, SeedDerivation, WalletSecret, mnemonic};
use key_wallet::bip32::{DerivationPath, ExtendedPrivKey};
use key_wallet::managed_account::managed_account_trait::ManagedAccountTrait;
use zeroize::{Zeroize, Zeroizing};

use crate::events::{EngineEvent, NoticeCode, unix_now};
use crate::history::AddressChain;
use crate::keys::mnemonic_error;
use crate::receive::AddressFilter;
use crate::{
    CORE_COMPAT_LOOKAHEAD, DashNetwork, EngineError, ImportOptions, MAX_LOOKAHEAD, NetworkSession,
    WalletId,
};

/// Largest dumpwallet file read (a wallet with a few hundred thousand keys).
const MAX_DUMP_BYTES: u64 = 256 * 1024 * 1024;
/// Most keys one legacy chain may have derived, keypool included. Past this
/// the pool would be too large to build, so the file is refused.
const MAX_ISSUED: u32 = walletdat::MAX_LEGACY_CHAIN_KEYS;
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
    /// A legacy `wallet.dat` read and refused on purpose: more keys on one
    /// chain than the import restores. Its funds move by sending them from
    /// Dash Core.
    WalletTooLarge,
    /// A legacy `wallet.dat` with keys on an HD account other than 0.
    ExtraHdAccount {
        account: u32,
    },
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
            Self::WalletTooLarge => f.write_str("wallet too large to restore"),
            Self::ExtraHdAccount { account } => {
                write!(f, "HD account {account} cannot be restored")
            }
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
    /// Legacy Berkeley DB `wallet.dat`. `encrypted` is `None` when the file
    /// is recognised but does not parse.
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
                // Legacy records in a SQLite file (`createfromdump
                // -format=sqlite`) are refused as the import refuses them.
                Err(e @ (WalletDatError::RangeTooLarge | WalletDatError::ExtraHdAccount(_))) => {
                    return Err(walletdat_failure(e));
                }
                _ => WalletFileKind::Unknown,
            });
        }
        Some(Container::BerkeleyDb) => {
            let w = walletdat::read_bdb(path).map_err(walletdat_failure)?;
            return Ok(WalletFileKind::WalletDatBdb {
                encrypted: Some(w.encrypted()),
            });
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
        self.0.private_key.zeroize();
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
    Some(master.private_key.public_key().to_compressed())
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
        state: ImportedState,
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
        let wallet_id = self
            .import_secret_with_state(options, make_secret, state)
            .await?;
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
            .public_key()
            .to_compressed();
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
        let report = self
            .import_core_root(root, options, ImportedState::default())
            .await?;
        Ok(ImportReport {
            keys_not_imported: report_keys,
            scripts_not_imported: report_scripts,
            ..report
        })
    }

    /// dash-qt "Restore wallet" from a `wallet.dat` (QT-106): SQLite
    /// descriptor wallets and legacy Berkeley DB wallets.
    pub async fn import_wallet_dat(
        self: &Arc<Self>,
        path: PathBuf,
        wallet_passphrase: Option<Zeroizing<Vec<u8>>>,
        options: ImportOptions,
    ) -> Result<ImportReport, EngineError> {
        drop(self.try_enter()?);
        let network_mainnet = self.network == DashNetwork::Mainnet;
        let WalletDatImport {
            root,
            loose_keys,
            scripts,
            state,
        } = self
            .on_runtime(async move {
                tokio::task::spawn_blocking(move || -> Result<WalletDatImport, EngineError> {
                    let wallet = match walletdat::sniff_container(&head(&path)?) {
                        Some(Container::Sqlite) => {
                            walletdat::read_sqlite(&path).map_err(walletdat_failure)?
                        }
                        Some(Container::BerkeleyDb) => {
                            walletdat::read_bdb(&path).map_err(walletdat_failure)?
                        }
                        None => {
                            return Err(CompatFailure::UnsupportedFormat(
                                "not a wallet.dat".into(),
                            )
                            .into());
                        }
                    };
                    let root = wallet
                        .hd_root(wallet_passphrase.as_deref().map(|p| &p[..]))
                        .map_err(walletdat_failure)?;
                    let mainnet = key_is_mainnet(&root.external.key).ok_or_else(|| {
                        CompatFailure::UnsupportedFormat(
                            "the wallet's network cannot be established from its keys".into(),
                        )
                    })?;
                    if mainnet != network_mainnet {
                        return Err(CompatFailure::NetworkMismatch { mainnet }.into());
                    }
                    let mut issued = None;
                    for (account, external, internal) in wallet.hd_accounts() {
                        if account != 0 {
                            return Err(CompatFailure::ExtraHdAccount { account }.into());
                        }
                        if external > MAX_ISSUED || internal > MAX_ISSUED {
                            return Err(CompatFailure::WalletTooLarge.into());
                        }
                        issued = Some((external, internal));
                    }
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
                    let (phrase, seed) = match (root.mnemonic, root.hd_seed) {
                        (Some(phrase), _) => (Some(phrase), None),
                        (None, Some(seed)) => {
                            let arr: [u8; 64] = match seed[..].try_into() {
                                Ok(arr) => arr,
                                Err(_) => {
                                    return Err(EngineError::NotImplemented(format!(
                                        "import_wallet_dat.seed_length.{}",
                                        seed.len()
                                    )));
                                }
                            };
                            (None, Some(Zeroizing::new(arr)))
                        }
                        (None, None) if wallet.is_descriptor_wallet() => {
                            return Err(EngineError::NotImplemented(
                                "import_wallet_dat.xprv".into(),
                            ));
                        }
                        (None, None) => return Err(CompatFailure::NoHdChain.into()),
                    };
                    let locks = wallet
                        .locked_outpoints
                        .iter()
                        .map(|(txid, vout)| dashcore::OutPoint {
                            txid: dashcore::Txid::from_byte_array(*txid),
                            vout: *vout,
                        })
                        .collect();
                    Ok(WalletDatImport {
                        root: CoreRoot {
                            phrase,
                            seed,
                            master_pubkey: Some(root.master_pubkey),
                            used_per_chain: wallet.next_index(),
                            labels,
                        },
                        loose_keys: wallet.loose_keys,
                        scripts: wallet.scripts,
                        state: ImportedState {
                            issued,
                            locks,
                            coinjoin_salt: wallet.coinjoin_salt,
                            rows: None,
                        },
                    })
                })
                .await?
            })
            .await?;
        let report = self.import_core_root(root, options, state).await?;
        Ok(ImportReport {
            keys_not_imported: loose_keys,
            scripts_not_imported: scripts,
            ..report
        })
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
                    master_pubkey: Some(master.private_key.public_key().to_compressed()),
                    used_per_chain: listed.next_index,
                    labels: Vec::new(),
                }
            }
        };
        self.import_core_root(root, options, ImportedState::default())
            .await
    }

    /// The warning for CoinJoin funds a legacy dash-qt wallet would not see.
    fn coinjoin_warning(&self, id: &WalletId) -> Option<ExportWarning> {
        self.hub
            .wallet_state(id)
            .is_some_and(|s| s.balances().coinjoin > 0)
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
            let appdb = this.live()?.appdb;
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
                write_export(&appdb, &dest, text.as_bytes())?;
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

/// What a Dash Core file carries beside its root. It is stored before the
/// wallet is registered and its range applied before anyone hears of the
/// wallet.
#[derive(Default)]
pub(crate) struct ImportedState {
    /// How many receive and change keys Core derived on account 0.
    pub(crate) issued: Option<(u32, u32)>,
    /// Core's persistent coin locks (`lockedutxo`).
    pub(crate) locks: Vec<dashcore::OutPoint>,
    pub(crate) coinjoin_salt: Option<[u8; 32]>,
    /// A restore's backed-up app rows of the wallet (labels, address book,
    /// notes, local contacts, settings, its salt and coin locks), perhaps
    /// none; `None` for any other import.
    pub(crate) rows: Option<Vec<dw_appdb::TableRows>>,
}

/// What `import_wallet_dat` carries out of the blocking read.
struct WalletDatImport {
    root: CoreRoot,
    loose_keys: u32,
    scripts: u32,
    state: ImportedState,
}

/// dw-appdb setting (wallet scope): `external,internal`, the counters of a
/// legacy Core wallet's account 0, applied again at every load because
/// key-wallet keeps its pools in memory.
pub(crate) const ISSUED_SETTING: &str = "bip44.legacy_issued";

impl NetworkSession {
    /// Derives account 0's receive and change chains up to the last address
    /// Core derived on each and keeps the gap past it. The two counters are
    /// not one lookahead, and either can be past the largest gap limit.
    /// Core may have handed out any address below its counter, so each one
    /// not yet paid is held reserved: no receive or change address is picked
    /// below the counter, and none is reported used before a payment marks
    /// it. A reservation stamped 0 never expires.
    pub(crate) async fn cover_issued(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        (external, internal): (u32, u32),
    ) -> Result<(), EngineError> {
        use key_wallet::managed_account::address_pool::AddressPoolType;
        let unsupported = || {
            EngineError::from(CompatFailure::UnsupportedFormat(
                "HD account 0 cannot be restored".into(),
            ))
        };
        let account = key_wallet::account::AccountType::Standard {
            index: 0,
            standard_account_type: key_wallet::account::StandardAccountType::BIP44Account,
        };
        let wm = manager.wallet_manager_arc();
        let mut wm = wm.write().await;
        let (wallet, info) = wm
            .get_wallet_and_info_mut(&id.0)
            .ok_or_else(|| EngineError::WalletNotFound(id.to_string()))?;
        let key = wallet.key_source_for_account(account);
        let managed = info
            .core_wallet
            .accounts
            .funds_account_mut(&account)
            .ok_or_else(unsupported)?;
        for pool in managed.managed_account_type_mut().address_pools_mut() {
            let next = match pool.pool_type() {
                AddressPoolType::External => external,
                AddressPoolType::Internal => internal,
                _ => continue,
            };
            let Some(last) = next.checked_sub(1) else {
                continue;
            };
            let from = pool.highest_generated().map_or(0, |h| h + 1);
            pool.generate_addresses(last.saturating_add(1).saturating_sub(from), &key)
                .map_err(|e| EngineError::Internal(format!("address range: {e}")))?;
            for index in 0..=last {
                if let Some(address) = pool.address_at_index(index) {
                    pool.reserve(&address, 0);
                }
            }
            pool.highest_used = pool.highest_used.max(Some(last));
            pool.maintain_gap_limit(&key)
                .map_err(|e| EngineError::Internal(format!("address range: {e}")))?;
        }
        managed.bump_monitor_revision();
        Ok(())
    }

    /// Stores `state` for wallet `id` in one transaction: the range
    /// setting, Core's coin locks and its CoinJoin salt, so the same coins
    /// count as fully mixed (QT-043, m3-engine.md §5), then a backup's rows.
    /// The salt replaces a stored one; each stored counter of the range only
    /// rises, so an older copy of the file does not narrow it; Core's locks
    /// are added to any the wallet has. A backup's rows are added to those
    /// of a `registered` wallet; those of one not registered (an earlier
    /// restore's, stopped before it registered the wallet) are replaced,
    /// even by none. Returns what the settings replaced or added and what
    /// the locks replaced, for [`Self::restore_imported_state`].
    pub(crate) async fn write_imported_state(
        self: &Arc<Self>,
        id: WalletId,
        state: &ImportedState,
        registered: bool,
    ) -> Result<Option<dw_appdb::PriorRows>, EngineError> {
        let mut settings = Vec::new();
        if let Some((external, internal)) = state.issued {
            settings.push((ISSUED_SETTING, format!("{external},{internal}")));
        }
        if let Some(salt) = state.coinjoin_salt {
            settings.push(crate::coinjoin::salt_setting(&salt));
        }
        // The coins may not be synced yet; the lock row is what coin
        // selection reads when they appear.
        let locks: Vec<(String, u32)> = (state.locks.iter())
            .map(|o| (o.txid.to_string(), o.vout))
            .collect();
        if settings.is_empty() && locks.is_empty() && state.rows.is_none() {
            return Ok(None);
        }
        let key = id.to_string();
        let rows = state.rows.clone();
        self.appdb_op(move |db| {
            db.write_settings_and_locks(
                &key,
                &settings,
                &widen_issued,
                &locks,
                crate::coins::now_secs(),
                rows.as_deref().map(|tables| dw_appdb::BackupRows {
                    tables,
                    replace: !registered,
                }),
            )
        })
        .await
        .map(Some)
    }

    /// Puts back what [`Self::write_imported_state`] replaced; whether it
    /// did.
    pub(crate) async fn restore_imported_state(&self, prior: dw_appdb::PriorRows) -> bool {
        let restored = self
            .appdb_op(move |db| db.restore_settings_and_locks(&prior))
            .await;
        if let Err(e) = &restored {
            tracing::warn!(error = %e, "could not put back the rows a failed import replaced");
        }
        restored.is_ok()
    }

    /// Applies the range of `state` to registered wallet `id`, before it is
    /// announced. The import fails if this does.
    pub(crate) async fn cover_imported_range(
        &self,
        manager: &crate::session::Manager,
        id: WalletId,
        state: &ImportedState,
    ) -> Result<(), EngineError> {
        let Some(issued) = state.issued else {
            return Ok(());
        };
        #[cfg(test)]
        if crate::keys::take_test_failure(&FAIL_COVER_IN, self.data_dir()) {
            return Err(EngineError::Internal("test: range not applied".into()));
        }
        self.cover_issued(manager, id, issued).await
    }

    /// Wallet `id`'s stored legacy counters, if stored; `Err` when they
    /// cannot be read or are malformed.
    pub(crate) async fn stored_issued(
        &self,
        id: WalletId,
    ) -> Result<Option<(u32, u32)>, EngineError> {
        #[cfg(test)]
        if crate::keys::fails_for(&crate::keys::FAIL_RESTORE_READ, id) {
            return Err(EngineError::Storage("test: range not read".into()));
        }
        let key = id.to_string();
        let value = self
            .appdb_op(move |db| db.setting(&key, ISSUED_SETTING))
            .await?;
        value
            .map(|v| {
                parse_issued(&v)
                    .ok_or_else(|| EngineError::Storage(format!("malformed stored range {v:?}")))
            })
            .transpose()
    }
}

/// The range setting's value, if valid: `"<external>,<internal>"`, each at
/// most [`MAX_ISSUED`].
pub(crate) fn parse_issued(value: &str) -> Option<(u32, u32)> {
    let (external, internal) = value.split_once(',')?;
    let issued = (external.parse().ok()?, internal.parse().ok()?);
    (issued.0 <= MAX_ISSUED && issued.1 <= MAX_ISSUED).then_some(issued)
}

/// The value stored for setting `key` when `stored` is there and `new` is
/// written: the range setting keeps the larger counter of each chain, any
/// other setting (or a stored range that does not parse) takes `new`.
fn widen_issued(key: &str, stored: &str, new: &str) -> String {
    match (
        key == ISSUED_SETTING,
        parse_issued(stored),
        parse_issued(new),
    ) {
        (true, Some((e0, i0)), Some((e1, i1))) => format!("{},{}", e0.max(e1), i0.max(i1)),
        _ => new.to_owned(),
    }
}

/// Tests: the next legacy range an import applies in a session on one of
/// these data directories fails.
#[cfg(test)]
pub(crate) static FAIL_COVER_IN: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn walletdat_failure(e: WalletDatError) -> EngineError {
    match e {
        WalletDatError::Unreadable(d) => CompatFailure::FileUnreadable(d),
        WalletDatError::Unsupported(d) => CompatFailure::UnsupportedFormat(d),
        WalletDatError::Corrupt(d) => CompatFailure::Corrupt(d),
        WalletDatError::PassphraseRequired => CompatFailure::PassphraseRequired,
        WalletDatError::WrongPassphrase => CompatFailure::WrongPassphrase,
        WalletDatError::NoHdChain => CompatFailure::NoHdChain,
        WalletDatError::RangeTooLarge => CompatFailure::WalletTooLarge,
        WalletDatError::ExtraHdAccount(account) => CompatFailure::ExtraHdAccount { account },
        WalletDatError::SeedLength(n) => {
            return EngineError::NotImplemented(format!("import_wallet_dat.seed_length.{n}"));
        }
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

/// Writes the Dash Core export `bytes` to `dest` (failing if it exists)
/// with mode 0600 through [`publish_staged`]. The staging file holds keys in
/// the clear, so its path is recorded in `appdb` before it exists and the
/// record cleared once it is gone: [`clean_export_staging`] deletes one an
/// interrupted export left. One the export could not delete fails it, the
/// error naming the file.
pub(crate) fn write_export(
    appdb: &dw_appdb::AppDb,
    dest: &Path,
    bytes: &[u8],
) -> Result<(), EngineError> {
    let unwritable =
        |e: std::io::Error| EngineError::from(CompatFailure::DestinationUnwritable(e.to_string()));
    let staging = staging_path(dest, EXPORT_STAGING_TAG).map_err(unwritable)?;
    let key = record_export_staging(appdb, &staging, dest).map_err(|e| {
        CompatFailure::DestinationUnwritable(format!("could not record the export: {e}"))
    })?;
    let published = publish_staged(&staging, dest, bytes);
    if staging.symlink_metadata().is_err()
        && let Err(e) = appdb.set_setting_durable(dw_appdb::GLOBAL_SCOPE, &key, None)
    {
        tracing::warn!(error = %e, "could not clear the record of an export's staging file");
    }
    published.map_err(unwritable)
}

/// The tag of a Dash Core export's staging file.
pub(crate) const EXPORT_STAGING_TAG: &str = "dash-wallet-export";

/// The app database's global settings recording a Dash Core export's
/// staging file, one per file: this prefix, then its name.
pub(crate) const EXPORT_STAGING_PREFIX: &str = "compat.export_staging.";

/// Records, durably, `staging` as the staging file of an export to `dest`;
/// the record's key.
pub(crate) fn record_export_staging(
    appdb: &dw_appdb::AppDb,
    staging: &Path,
    dest: &Path,
) -> dw_appdb::Result<String> {
    let name = staging.file_name().unwrap_or_default().to_string_lossy();
    let key = format!("{EXPORT_STAGING_PREFIX}{name}");
    let record = serde_json::json!({
        "staging": path_record(staging),
        "destination": path_record(dest),
    })
    .to_string();
    appdb.set_setting_durable(dw_appdb::GLOBAL_SCOPE, &key, Some(&record))?;
    Ok(key)
}

/// A path as text that gives it back byte for byte: its bytes (UTF-16
/// units on Windows) in hex.
fn path_record(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        hex::encode(path.as_os_str().as_bytes())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str()
            .encode_wide()
            .map(|u| format!("{u:04x}"))
            .collect()
    }
}

fn path_of_record(record: &str) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bytes = hex::decode(record).ok()?;
        Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if !record.len().is_multiple_of(4) {
            return None;
        }
        let units = (0..record.len())
            .step_by(4)
            .map(|i| u16::from_str_radix(record.get(i..i + 4)?, 16).ok())
            .collect::<Option<Vec<u16>>>()?;
        Some(PathBuf::from(std::ffi::OsString::from_wide(&units)))
    }
}

/// What [`clean_export_staging`] did with a staging file it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StagingCleanup {
    Deleted(PathBuf),
    NotDeleted(PathBuf),
}

/// Deletes the staging files of Dash Core exports that did not end
/// (recorded by [`write_export`]). A record whose file is gone from a folder
/// that is there, or names no staging file of ours, is cleared silently;
/// one whose folder is not there (a volume not mounted) stays, silently,
/// and one whose file could not be deleted stays, for the next start.
pub(crate) fn clean_export_staging(appdb: &dw_appdb::AppDb) -> Vec<StagingCleanup> {
    let records = match appdb.settings_with_prefix(dw_appdb::GLOBAL_SCOPE, EXPORT_STAGING_PREFIX) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "could not read the records of export staging files");
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    for (key, record) in records {
        let paths = serde_json::from_str::<serde_json::Value>(&record)
            .ok()
            .and_then(|v| {
                let path = |k: &str| v.get(k)?.as_str().and_then(path_of_record);
                Some((path("staging")?, path("destination")?))
            });
        let ours = paths.filter(|(staging, destination)| {
            staging != destination
                && staging
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| is_staging_name(n, EXPORT_STAGING_TAG))
        });
        let keep = match ours {
            None => false,
            Some((staging, _)) => match std::fs::remove_file(&staging) {
                Ok(()) => {
                    out.push(StagingCleanup::Deleted(staging));
                    false
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    !staging.parent().is_some_and(Path::is_dir)
                }
                Err(e) => {
                    tracing::warn!(path = %staging.display(), error = %e, "could not delete an export's staging file");
                    out.push(StagingCleanup::NotDeleted(staging));
                    true
                }
            },
        };
        if !keep && let Err(e) = appdb.set_setting_durable(dw_appdb::GLOBAL_SCOPE, &key, None) {
            tracing::warn!(error = %e, "could not clear the record of an export's staging file");
        }
    }
    out
}

impl NetworkSession {
    /// [`clean_export_staging`] on this session's database, each file found
    /// sent as `Notice{ExportStagingDeleted}` or
    /// `Notice{ExportStagingNotDeleted}` with its path.
    pub(crate) async fn clean_export_staging(&self) {
        let Ok(live) = self.live() else { return };
        let Ok(found) =
            tokio::task::spawn_blocking(move || clean_export_staging(&live.appdb)).await
        else {
            return;
        };
        for cleanup in found {
            let (code, path) = match cleanup {
                StagingCleanup::Deleted(p) => (NoticeCode::ExportStagingDeleted, p),
                StagingCleanup::NotDeleted(p) => (NoticeCode::ExportStagingNotDeleted, p),
            };
            self.sink.emit(EngineEvent::Notice {
                network: Some(self.network.clone()),
                code,
                detail: path.display().to_string(),
            });
        }
    }
}

/// Creates `dest` (failing `AlreadyExists` if it exists) with mode 0600, and
/// writes and syncs `bytes`. A failed write removes the partial file.
fn write_new_private_io(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(dest)?;
    if let Err(e) = f.write_all(bytes).and_then(|()| f.sync_all()) {
        drop(f);
        let _ = std::fs::remove_file(dest);
        return Err(e);
    }
    Ok(())
}

/// Writes `bytes` to a hidden staging file beside `dest` (mode 0600, named
/// after `tag`), syncs it and only then gives it the name `dest`, failing
/// `AlreadyExists` if that exists: a stop mid-write leaves nothing at
/// `dest`.
pub(crate) fn publish_new_private(dest: &Path, bytes: &[u8], tag: &str) -> std::io::Result<()> {
    publish_staged(&staging_path(dest, tag)?, dest, bytes)
}

/// The staging file of `dest`: `.<tag>.tmp-<pid>-<16 hex digits>` in its
/// folder, short whatever `dest`'s name.
pub(crate) fn staging_path(dest: &Path, tag: &str) -> std::io::Result<PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NONCE: AtomicU64 = AtomicU64::new(0);
    if dest.file_name().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "no file name",
        ));
    }
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed) ^ rand::random::<u64>();
    let dir = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(dir.join(format!(
        ".{tag}{TEMP_MARK}{}-{nonce:016x}",
        std::process::id()
    )))
}

/// Whether `name` is a staging file name of [`staging_path`] for `tag`.
pub(crate) fn is_staging_name(name: &str, tag: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|n| n.strip_prefix(tag))
        .and_then(|n| n.strip_prefix(TEMP_MARK))
        .is_some_and(|rest| {
            rest.split_once('-').is_some_and(|(pid, nonce)| {
                !pid.is_empty()
                    && pid.bytes().all(|b| b.is_ascii_digit())
                    && nonce.len() == 16
                    && nonce.bytes().all(|b| b.is_ascii_hexdigit())
            })
        })
}

/// [`publish_new_private`] through `staging`. Its result covers the folder's
/// sync, so `Ok` means the name is durable. A staging file it could not
/// delete after the publish fails it too, the error naming the file.
fn publish_staged(staging: &Path, dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    publish_staged_with(staging, dest, bytes, |from, to| {
        std::fs::hard_link(from, to)
    })
}

fn publish_staged_with(
    staging: &Path,
    dest: &Path,
    bytes: &[u8],
    link: impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let dir = staging.parent().unwrap_or(Path::new("."));
    write_new_private_io(staging, bytes)?;
    let moved = publish_fault(dir).and_then(|()| move_new_with(staging, dest, &link));
    let left = match moved {
        Ok(left) => left,
        Err(e) => {
            let _ = std::fs::remove_file(staging);
            return Err(e);
        }
    };
    sync_dir(dir)?;
    match left {
        None => Ok(()),
        Some(e) => Err(std::io::Error::new(
            e.kind(),
            format!(
                "{} was written, but its temporary copy {} could not be deleted ({e}): delete it",
                dest.display(),
                staging.display()
            ),
        )),
    }
}

/// Syncs the folder `dir`, making the names in it durable (Unix; Windows
/// has no such sync). Its fault point is `backup.publish` after.
pub(crate) fn sync_dir(dir: &Path) -> std::io::Result<()> {
    publish_after_fault(dir)?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

/// What a staging file of [`staging_path`] has in its name, after its tag.
pub(crate) const TEMP_MARK: &str = ".tmp-";

/// Gives `from` the name `to`, failing `AlreadyExists` if anything is
/// there: a hard link, then `from` removed. Where the file system has no
/// hard links (FAT, exFAT), a rename that never replaces one
/// ([`dw_fs::rename_noreplace`]), or nothing where there is none. A `from`
/// left after the link fails it.
pub(crate) fn move_new(from: &Path, to: &Path) -> std::io::Result<()> {
    match move_new_with(from, to, &|from: &Path, to: &Path| {
        std::fs::hard_link(from, to)
    })? {
        None => Ok(()),
        Some(e) => Err(e),
    }
}

/// [`move_new`] with `link`; `Ok(Some(e))`: `to` has it, but `from` could
/// not be removed.
fn move_new_with(
    from: &Path,
    to: &Path,
    link: &impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<Option<std::io::Error>> {
    match link(from, to) {
        Ok(()) => Ok(std::fs::remove_file(from).err()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => dw_fs::rename_noreplace(from, to).map(|()| None),
    }
}

/// The fault point `backup.publish` between the sync and the publish of a
/// backup or a Dash Core export, armed for its directory;
/// `DW_FAULT=backup.before_publish` aborts there.
#[cfg(all(debug_assertions, any(test, feature = "fault-inject")))]
fn publish_fault(dir: &Path) -> std::io::Result<()> {
    dw_appdb::fault::hit(dir, "backup.publish", dw_appdb::fault::Stage::Before)
        .map_err(std::io::Error::other)
}

#[cfg(not(all(debug_assertions, any(test, feature = "fault-inject"))))]
#[inline(always)]
fn publish_fault(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

/// The fault point `backup.publish` after the publish, at the folder's
/// sync: the name is there but not durable.
#[cfg(all(debug_assertions, any(test, feature = "fault-inject")))]
fn publish_after_fault(dir: &Path) -> std::io::Result<()> {
    dw_appdb::fault::hit(dir, "backup.publish", dw_appdb::fault::Stage::After)
        .map_err(std::io::Error::other)
}

#[cfg(not(all(debug_assertions, any(test, feature = "fault-inject"))))]
#[inline(always)]
fn publish_after_fault(_dir: &Path) -> std::io::Result<()> {
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
    let mut keys = Vec::with_capacity(addresses.len());
    let (mut external, mut internal) = (0u32, 0u32);
    for a in addresses {
        let path = DerivationPath::from_str(&a.derivation_path)
            .map_err(|e| EngineError::Internal(format!("path {}: {e}", a.derivation_path)))?;
        let mut child = master
            .derive_priv(&path)
            .map_err(|e| EngineError::Internal(format!("derive {path}: {e}")))?;
        let wif = keyio::encode_secret(
            &keyio::Secret {
                key: child.private_key.to_bytes(),
                compressed: true,
            },
            network,
        );
        child.private_key.zeroize();
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

    #[derive(Default)]
    struct Created(std::sync::Mutex<Vec<WalletId>>);

    impl crate::EventSink for Created {
        fn emit(&self, event: crate::EngineEvent) {
            if let crate::EngineEvent::WalletCreated { wallet_id, .. } = event {
                self.0.lock().unwrap().push(wallet_id);
            }
        }
    }

    /// A regtest session with an unencrypted vault, its data dir private,
    /// and what it announced.
    fn regtest_session() -> (
        tempfile::TempDir,
        crate::Engine,
        Arc<NetworkSession>,
        Arc<Created>,
    ) {
        use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};
        let dir = dw_testutil::private_tempdir();
        let created = Arc::new(Created::default());
        let engine = crate::Engine::new(
            crate::EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(2),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..Default::default()
                },
            },
            Arc::clone(&created) as Arc<dyn crate::EventSink>,
        )
        .unwrap();
        let s = engine
            .block_on(engine.open_network(DashNetwork::Regtest, offline()))
            .unwrap();
        engine.block_on(s.vault_op(|v| v.create(None))).unwrap();
        (dir, engine, s, created)
    }

    fn offline() -> crate::SessionOptions {
        crate::SessionOptions {
            dapi_addresses: vec!["http://127.0.0.1:1".into()],
            quorum_url: Some("http://127.0.0.1:1".into()),
            spv_peers: vec!["127.0.0.1:1".into()],
            ..Default::default()
        }
    }

    /// Closes the session and opens it again: its wallets load from storage.
    fn reopen(engine: &crate::Engine, s: Arc<NetworkSession>) -> Arc<NetworkSession> {
        drop(s);
        assert!(
            engine
                .block_on(engine.close_network(DashNetwork::Regtest))
                .unwrap()
        );
        engine
            .block_on(engine.open_network(DashNetwork::Regtest, offline()))
            .unwrap()
    }

    fn import(engine: &crate::Engine, s: &Arc<NetworkSession>, name: &str) -> WalletId {
        let file = testdata(&format!("compat/walletdat/{name}"));
        (engine.block_on(s.import_wallet_dat(file, None, ImportOptions::default())))
            .unwrap()
            .wallet_id
    }

    fn addresses_of(
        engine: &crate::Engine,
        s: &Arc<NetworkSession>,
        id: WalletId,
    ) -> Vec<crate::receive::AddressInfo> {
        engine
            .block_on(s.addresses(id, crate::receive::AddressFilter::default()))
            .unwrap()
    }

    fn highest(addresses: &[crate::receive::AddressInfo], chain: AddressChain) -> u32 {
        (addresses.iter())
            .filter(|a| a.chain == chain)
            .map(|a| a.index)
            .max()
            .unwrap()
    }

    /// Attaching an older copy of a wallet.dat to a wallet keeps the larger
    /// counter of each chain, whichever copy it came from, and a reload
    /// watches that range.
    #[test]
    fn an_older_wallet_dat_does_not_narrow_the_stored_range() {
        let (_dir, engine, s, _created) = regtest_session();
        let id = wide_range_id();
        let stored = |s: &NetworkSession| {
            (s.live().unwrap().appdb)
                .setting(&id.to_string(), ISSUED_SETTING)
                .unwrap()
        };
        assert_eq!(import(&engine, &s, "bdb_wide_change.dat"), id);
        assert_eq!(stored(&s).as_deref(), Some("4,2600"));
        let attach = |name: &str| {
            assert!(s.vault().delete_wallet_secret(&id.0).unwrap());
            assert_eq!(import(&engine, &s, name), id);
        };
        attach("bdb_wide_range.dat");
        assert_eq!(stored(&s).as_deref(), Some("1001,2600"));
        attach("bdb_wide_change.dat");
        assert_eq!(stored(&s).as_deref(), Some("1001,2600"));

        let s = reopen(&engine, s);
        let addresses = addresses_of(&engine, &s, id);
        assert!(highest(&addresses, AddressChain::Receiving) >= 1000 + CORE_COMPAT_LOOKAHEAD);
        assert!(highest(&addresses, AddressChain::Change) >= 2599 + CORE_COMPAT_LOOKAHEAD);
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// The id of `bdb_wide_range.dat`'s wallet (`abandon … about`).
    fn wide_range_id() -> WalletId {
        let phrase = b"abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let seed = mnemonic::derive_secret(phrase, b"", true).unwrap().seed;
        WalletId(mnemonic::wallet_id_for_seed(&seed, dashcore::Network::Regtest).unwrap())
    }

    /// Whether wallet `id` has none of Core's rows: range, salt, locks.
    fn has_no_core_rows(s: &NetworkSession, id: WalletId) -> bool {
        let (db, wallet) = (s.live().unwrap().appdb, id.to_string());
        db.setting(&wallet, ISSUED_SETTING).unwrap().is_none()
            && db.setting(&wallet, "coinjoin.salt").unwrap().is_none()
            && db.locks(&wallet).unwrap().is_empty()
    }

    /// A legacy wallet whose range cannot be applied is not announced: the
    /// import fails, the wallet is taken out with its rows, and importing
    /// the file again works.
    #[test]
    fn a_legacy_range_not_applied_fails_the_import_before_it_is_announced() {
        let (_dir, engine, s, created) = regtest_session();
        let id = wide_range_id();
        let file = testdata("compat/walletdat/bdb_wide_range.dat");

        FAIL_COVER_IN
            .lock()
            .unwrap()
            .push(s.data_dir().to_path_buf());
        let failed =
            engine.block_on(s.import_wallet_dat(file.clone(), None, ImportOptions::default()));
        assert!(
            matches!(failed, Err(EngineError::Internal(_))),
            "{failed:?}"
        );
        assert!(
            created.0.lock().unwrap().is_empty(),
            "the wallet was announced"
        );
        let manager = s.manager().unwrap();
        assert!(engine.block_on(manager.get_wallet(&id.0)).is_none());
        assert!(has_no_core_rows(&s, id));

        let report = engine
            .block_on(s.import_wallet_dat(file, None, ImportOptions::default()))
            .unwrap();
        assert_eq!(report.wallet_id, id);
        assert_eq!(*created.0.lock().unwrap(), [id]);
        assert_eq!(engine.block_on(s.locked_outpoints(id)).unwrap().len(), 1);
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// F-B: keys attached from a wallet.dat whose range cannot be applied
    /// are refused, and the marker and lookahead they replaced are back: a
    /// reopen neither lowers the birth nor replays.
    #[test]
    fn an_attach_whose_range_fails_leaves_nothing_behind() {
        let (_dir, engine, s, _created) = regtest_session();
        let id = import(&engine, &s, "bdb_wide_range.dat");
        assert!(s.vault().delete_wallet_secret(&id.0).unwrap());
        engine.block_on(s.set_wide_scan(id, Some(crate::keys::WideScan::Done)));
        let gap = |s: &NetworkSession| {
            (s.live().unwrap().appdb)
                .setting(&id.to_string(), crate::keys::LOOKAHEAD_SETTING)
                .unwrap()
        };
        let birth = |s: &NetworkSession| {
            use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
            let wm = s.manager().unwrap().wallet_manager_arc();
            engine.block_on(async {
                let wm = wm.read().await;
                (wm.get_wallet_info(&id.0).unwrap().core_wallet).birth_height()
            })
        };
        let (gap_before, birth_before) = (gap(&s), birth(&s));
        FAIL_COVER_IN
            .lock()
            .unwrap()
            .push(s.data_dir().to_path_buf());

        let file = testdata("compat/walletdat/bdb_wide_range.dat");
        let failed = engine.block_on(s.import_wallet_dat(file, None, ImportOptions::default()));

        assert!(failed.is_err());
        assert_eq!(
            engine.block_on(s.wide_scan(id)).unwrap(),
            Some(crate::keys::WideScan::Done)
        );
        assert_eq!(gap(&s), gap_before);
        let s = reopen(&engine, s);
        assert_eq!(birth(&s), birth_before);
        assert!(
            engine
                .block_on(s.live().unwrap().replay.get(id))
                .unwrap()
                .is_none()
        );
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// Taking out a wallet whose range was not applied deletes its rows even
    /// when its stored wallet cannot be deleted.
    #[test]
    fn a_failed_stored_wallet_delete_still_deletes_the_rows() {
        let (_dir, engine, s, created) = regtest_session();
        let id = wide_range_id();
        let dir = s.data_dir().to_path_buf();
        FAIL_COVER_IN.lock().unwrap().push(dir.clone());
        (crate::backup::FAIL_STORED_WALLET_DELETE_IN.lock().unwrap()).push(dir);
        let failed = engine.block_on(s.import_wallet_dat(
            testdata("compat/walletdat/bdb_wide_range.dat"),
            None,
            ImportOptions::default(),
        ));
        assert!(
            matches!(failed, Err(EngineError::Internal(_))),
            "{failed:?}"
        );
        assert!(created.0.lock().unwrap().is_empty());
        assert!(has_no_core_rows(&s, id));
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// After a legacy import, and after a reload, neither the receive nor
    /// the change address the wallet picks is one Core may have handed out
    /// (below its counter on that chain); none of them is reported used
    /// without a payment, and the whole derived range plus the gap is
    /// still watched.
    #[test]
    fn core_issued_addresses_are_not_handed_out_again() {
        use key_wallet::wallet::managed_wallet_info::wallet_info_interface::WalletInfoInterface;
        let (_dir, engine, s, _created) = regtest_session();
        let id = import(&engine, &s, "bdb_wide_range.dat");
        let (external, internal) = (1001, 2);
        let check = |s: &Arc<NetworkSession>| {
            let current = engine.block_on(s.current_receive_address(id)).unwrap();
            assert!(current.index >= external, "{current:?}");
            let issued = engine.block_on(s.next_receive_address(id, None)).unwrap();
            assert!(issued.index >= external, "{issued:?}");
            let (change, watched) = engine.block_on(async {
                let manager = s.manager().unwrap();
                let wallet = manager.get_wallet(&id.0).await.unwrap();
                let change = wallet
                    .core()
                    .next_change_address_for_account(0)
                    .await
                    .unwrap();
                let wm = manager.wallet_manager_arc();
                let wm = wm.read().await;
                let watched = (wm.get_wallet_info(&id.0).unwrap().core_wallet)
                    .monitored_addresses()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<std::collections::HashSet<_>>();
                (change.to_string(), watched)
            });
            let addresses = addresses_of(&engine, s, id);
            let change = addresses.iter().find(|a| a.address == change).unwrap();
            assert_eq!(change.chain, AddressChain::Change);
            assert!(change.index >= internal, "{change:?}");
            assert!(
                addresses.iter().all(|a| !a.used),
                "an address reported used without a payment"
            );
            for (chain, counter) in [
                (AddressChain::Receiving, external),
                (AddressChain::Change, internal),
            ] {
                let derived: Vec<_> = (addresses.iter())
                    .filter(|a| a.chain == chain && a.index < counter)
                    .collect();
                assert_eq!(derived.len(), counter as usize, "{chain:?}");
                assert!(derived.iter().all(|a| watched.contains(&a.address)));
                assert!(highest(&addresses, chain) >= counter - 1 + CORE_COMPAT_LOOKAHEAD);
            }
        };
        check(&s);
        let s = reopen(&engine, s);
        check(&s);
        engine.block_on(engine.shutdown()).unwrap();
    }

    /// Review L4: what the guard's drop runs replaces the private key.
    #[test]
    fn master_key_guard_erases_the_private_key() {
        let mut m = MasterKey::new(dashcore::Network::Testnet, &[7; 64]).unwrap();
        let before = *m.private_key.to_bytes();
        m.erase();
        assert_ne!(*m.private_key.to_bytes(), before);
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
            inspect_wallet_file(&testdata("compat/walletdat/bdb_hd_encrypted.dat")).unwrap(),
            WalletFileKind::WalletDatBdb {
                encrypted: Some(true)
            }
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

    /// A Berkeley DB file that does not parse is reported with its error,
    /// as an import of it would be, not as a wallet.dat to import.
    #[test]
    fn inspecting_a_bdb_file_that_does_not_parse_returns_its_error() {
        let dir = dw_testutil::private_tempdir();
        let whole = std::fs::read(testdata("compat/walletdat/bdb_core24_hd_plain.dat")).unwrap();
        let truncated = dir.path().join("truncated.dat");
        std::fs::write(&truncated, &whole[..whole.len() / 2]).unwrap();
        for path in [testdata("compat/walletdat/bdb_legacy_head.dat"), truncated] {
            let got = inspect_wallet_file(&path);
            assert!(
                matches!(got, Err(EngineError::Compat(CompatFailure::Corrupt(_)))),
                "{}: {got:?}",
                path.display()
            );
        }
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

    fn app_db(dir: &Path) -> dw_appdb::AppDb {
        dw_appdb::AppDb::open(&dir.join(dw_appdb::APP_DB_FILE)).unwrap()
    }

    fn staging_records(db: &dw_appdb::AppDb) -> usize {
        db.settings_with_prefix(dw_appdb::GLOBAL_SCOPE, EXPORT_STAGING_PREFIX)
            .unwrap()
            .len()
    }

    #[test]
    fn destinations_are_never_replaced() {
        let dir = dw_testutil::private_tempdir();
        let db_dir = dw_testutil::private_tempdir();
        let db = app_db(db_dir.path());
        let f = dir.path().join("out.txt");
        write_export(&db, &f, b"x").unwrap();
        assert!(matches!(
            check_destination(&f),
            Err(EngineError::Compat(CompatFailure::DestinationUnwritable(_)))
        ));
        assert!(write_export(&db, &f, b"y").is_err());
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()), ["out.txt"]);
        assert_eq!(staging_records(&db), 0);
        assert!(check_destination(&dir.path().join("missing/out.txt")).is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_publish_replaces_no_file_and_leaves_no_temporary_file() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("out.dwbackup");
        publish_new_private(&f, b"x", "t").unwrap();
        assert_eq!(
            publish_new_private(&f, b"y", "t").unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()), ["out.dwbackup"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn without_hard_links_a_publish_renames_and_still_replaces_no_file() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("out.dwbackup");
        let unsupported =
            |_: &Path, _: &Path| Err(std::io::Error::from(std::io::ErrorKind::Unsupported));
        let staged = |f: &Path| staging_path(f, "t").unwrap();
        publish_staged_with(&staged(&f), &f, b"x", unsupported).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()), ["out.dwbackup"]);
        assert_eq!(
            publish_staged_with(&staged(&f), &f, b"y", unsupported)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()), ["out.dwbackup"]);
    }

    #[test]
    fn an_export_stopped_before_it_names_the_file_leaves_nothing_at_the_name() {
        let dir = dw_testutil::private_tempdir();
        let db_dir = dw_testutil::private_tempdir();
        let db = app_db(db_dir.path());
        let f = dir.path().join("wallet.txt");
        dw_appdb::fault::arm(dir.path(), "backup.publish", dw_appdb::fault::Stage::Before);
        assert!(matches!(
            write_export(&db, &f, b"x"),
            Err(EngineError::Compat(CompatFailure::DestinationUnwritable(_)))
        ));
        assert!(names(dir.path()).is_empty());
        assert_eq!(staging_records(&db), 0);
    }

    /// A destination whose name is as long as a name can be still
    /// publishes: the staging file's name does not grow with it.
    #[test]
    fn a_publish_to_a_long_name_stages_under_a_short_one() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join(format!("{}.dwbackup", "w".repeat(231)));
        assert_eq!(f.file_name().unwrap().len(), 240);
        publish_new_private(&f, b"x", "dash-wallet-backup").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()).len(), 1);
    }

    /// A staging file that cannot be deleted once the file is published
    /// fails the publish, the error naming it.
    #[test]
    fn a_staging_file_left_after_the_publish_is_reported() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("wallet.txt");
        let staging = staging_path(&f, EXPORT_STAGING_TAG).unwrap();
        let undeletable = |from: &Path, to: &Path| {
            std::fs::hard_link(from, to)?;
            std::fs::remove_file(from)?;
            std::fs::create_dir(from)?;
            std::fs::write(from.join("x"), b"")
        };
        let e = publish_staged_with(&staging, &f, b"x", undeletable).unwrap_err();
        assert!(
            e.to_string().contains(&staging.display().to_string()),
            "{e}"
        );
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
    }

    /// A publish whose folder cannot be synced fails, the name there but
    /// not durable.
    #[test]
    fn a_publish_whose_folder_sync_fails_fails() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("out.dwbackup");
        dw_appdb::fault::arm(dir.path(), "backup.publish", dw_appdb::fault::Stage::After);
        assert!(publish_new_private(&f, b"x", "t").is_err());
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
        assert_eq!(names(dir.path()), ["out.dwbackup"]);
    }

    /// The start's sweep deletes a recorded export staging file and clears
    /// its record; one it cannot delete is reported by path and kept on
    /// record; a record of a file that is gone, or of no staging file of
    /// ours, is cleared and nothing is deleted.
    #[test]
    fn the_start_deletes_an_export_staging_file_or_names_it() {
        let dir = dw_testutil::private_tempdir();
        let db_dir = dw_testutil::private_tempdir();
        let db = app_db(db_dir.path());
        let record = |staging: &Path, dest: &Path| {
            record_export_staging(&db, staging, dest).unwrap();
        };
        let dest = dir.path().join("wallet.txt");
        let left = staging_path(&dest, EXPORT_STAGING_TAG).unwrap();
        std::fs::write(&left, b"keys").unwrap();
        record(&left, &dest);
        let stuck = staging_path(&dest, EXPORT_STAGING_TAG).unwrap();
        std::fs::create_dir(&stuck).unwrap();
        std::fs::write(stuck.join("x"), b"").unwrap();
        record(&stuck, &dest);
        record(&staging_path(&dest, EXPORT_STAGING_TAG).unwrap(), &dest);
        let not_ours = dir.path().join("mine.txt");
        std::fs::write(&not_ours, b"mine").unwrap();
        record(&not_ours, &dest);

        let mut found = clean_export_staging(&db);
        found.sort_by_key(|c| matches!(c, StagingCleanup::NotDeleted(_)));
        assert_eq!(
            found,
            [
                StagingCleanup::Deleted(left.clone()),
                StagingCleanup::NotDeleted(stuck.clone())
            ]
        );
        assert!(!left.exists() && stuck.exists() && not_ours.exists());
        assert_eq!(staging_records(&db), 1);
        assert_eq!(
            clean_export_staging(&db),
            [StagingCleanup::NotDeleted(stuck.clone())]
        );
        std::fs::remove_dir_all(&stuck).unwrap();
        assert_eq!(clean_export_staging(&db), []);
        assert_eq!(staging_records(&db), 0);
    }

    #[test]
    fn an_export_record_whose_folder_is_not_there_stays_until_it_is() {
        let dir = dw_testutil::private_tempdir();
        let db_dir = dw_testutil::private_tempdir();
        let db = app_db(db_dir.path());
        let volume = dir.path().join("stick");
        let dest = volume.join("wallet.txt");
        let staging = staging_path(&dest, EXPORT_STAGING_TAG).unwrap();
        record_export_staging(&db, &staging, &dest).unwrap();

        assert_eq!(clean_export_staging(&db), []);
        assert_eq!(staging_records(&db), 1);

        std::fs::create_dir(&volume).unwrap();
        std::fs::write(&staging, b"keys").unwrap();
        assert_eq!(
            clean_export_staging(&db),
            [StagingCleanup::Deleted(staging.clone())]
        );
        assert!(!staging.exists());
        assert_eq!(staging_records(&db), 0);
    }

    #[test]
    fn a_path_record_gives_the_path_back() {
        let p = Path::new("/tmp/dir with space/ünï/.dash-wallet-export.tmp-1-0000000000000000");
        assert_eq!(path_of_record(&path_record(p)).as_deref(), Some(p));
        assert_eq!(path_of_record("zz"), None);
    }

    #[test]
    fn a_publish_stopped_before_it_names_the_file_leaves_nothing_at_the_name() {
        let dir = dw_testutil::private_tempdir();
        let f = dir.path().join("out.dwbackup");
        dw_appdb::fault::arm(dir.path(), "backup.publish", dw_appdb::fault::Stage::Before);
        assert!(publish_new_private(&f, b"x", "t").is_err());
        assert!(names(dir.path()).is_empty());
        publish_new_private(&f, b"x", "t").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"x");
    }
}
