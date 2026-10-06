//! M2 dash-qt compatibility: inspect, import and export Dash Core wallet
//! files (dumpwallet, SQLite descriptor wallet.dat, hdseed/xprv/descriptors).
//! Owner: R2 (compat, `dw-compat`). Contract: docs/contracts/m2-engine.md
//! §2.6. Secrets cross only as bytes and are zeroized; exports that contain
//! keys are written by Rust straight to the chosen file.

use zeroize::Zeroizing;

use crate::api::common::{domain_error_common, ensure_open, not_implemented, parse_wallet_id};
use crate::{DashNetwork, Engine, ImportOptions, NetworkSession};

/// What a user-chosen file is (QT-106/107/110 "Restore" and "Import").
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum WalletFileKind {
    /// Dash Core `dumpwallet` text (§18.5).
    DumpWallet {
        /// The network of its keys (from the WIF/xprv prefixes).
        network: Option<DashNetwork>,
        has_mnemonic: bool,
        has_hd_seed: bool,
        has_xprv: bool,
        /// Loose (non-HD) WIF keys: swept in M5, not imported.
        loose_key_count: u32,
        script_count: u32,
        label_count: u32,
    },
    /// Dash Core v21+ SQLite descriptor `wallet.dat` (§18.1).
    WalletDatSqlite {
        encrypted: bool,
        has_mnemonic: bool,
    },
    /// Legacy Berkeley DB `wallet.dat`: detected; import lands in M6.
    WalletDatBdb {
        encrypted: Option<bool>,
    },
    /// Our `.dwbackup` bundle (`restore_backup`).
    DwBackup {
        network: DashNetwork,
        wallet_count: u32,
        created_at: u64,
        format_version: u32,
    },
    /// A binary or base64 PSBT (`parse_psbt`).
    Psbt,
    Unknown,
}

/// Key material for "Import from hdseed / xprv / listdescriptors" (QT-108).
/// Every payload is bytes and is zeroized.
#[derive(uniffi::Enum)]
pub enum KeyMaterial {
    /// Raw BIP32 seed, 16..=64 bytes (dashd `dumphdinfo` `hdseed` hex,
    /// decoded by the host).
    HdSeed { seed: Vec<u8> },
    /// Master extended private key, base58 text as UTF-8 bytes.
    Xprv { xprv: Vec<u8> },
    /// dashd `listdescriptors true` JSON as UTF-8 bytes.
    Descriptors { json: Vec<u8> },
}

impl std::fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::HdSeed { .. } => "HdSeed",
            Self::Xprv { .. } => "Xprv",
            Self::Descriptors { .. } => "Descriptors",
        };
        write!(f, "KeyMaterial::{kind}(<redacted>)")
    }
}

/// Result of an import (QT-106…108).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ImportReport {
    pub wallet_id: String,
    /// Address-book entries and labels carried over.
    pub labels_imported: u32,
    /// Loose WIF keys and scripts left out (no loose-key accounts; sweep is
    /// M5). The host tells the user how many.
    pub keys_not_imported: u32,
    pub scripts_not_imported: u32,
    /// The seed was derived with Dash Core's BIP39 quirks (QT-104).
    pub core_compat_seed: bool,
}

/// Formats "Export for dash-qt" writes (QT-109).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CoreExportFormat {
    /// Core's `dumpwallet` format, byte for byte (`importwallet` on a legacy
    /// dashd wallet).
    DumpWallet,
    /// `importdescriptors` JSON for a dashd descriptor wallet.
    ImportDescriptorsJson,
}

/// Why an export may not reproduce the wallet in dash-qt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ExportWarning {
    /// The phrase is not English or fails Dash Core's check: `upgradetohd`
    /// with it gives another wallet. The file still carries the keys.
    MnemonicNotCoreCompatible,
    /// CoinJoin funds sit on the DIP9 account, which dash-qt legacy wallets
    /// do not scan (descriptor wallets do).
    CoinJoinAccountNotScannedByLegacyCore,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ExportReport {
    pub path: String,
    pub format: CoreExportFormat,
    pub key_count: u32,
    pub warnings: Vec<ExportWarning>,
}

/// Whether the wallet's phrase restores the same wallet in dash-qt through
/// "mnemonic + passphrase + `upgradetohd`" (QT-109 a).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoreMnemonicCompatibility {
    pub core_compatible: bool,
    pub warnings: Vec<ExportWarning>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CompatError {
    /// Code `compat.file_unreadable`: missing, unreadable or a directory.
    #[error("file unreadable: {detail}")]
    FileUnreadable { detail: String },
    /// Code `compat.unsupported_format`: not a format this call imports.
    #[error("unsupported format: {detail}")]
    UnsupportedFormat { detail: String },
    /// Code `compat.corrupt`: the file parses only partly.
    #[error("corrupt: {detail}")]
    Corrupt { detail: String },
    /// Code `compat.passphrase_required`: an encrypted wallet.dat needs its
    /// passphrase.
    #[error("passphrase required")]
    PassphraseRequired,
    /// Code `compat.wrong_passphrase`: the wallet.dat passphrase is wrong.
    #[error("wrong passphrase")]
    WrongPassphrase,
    /// Code `compat.no_hd_chain`: a non-HD wallet; only loose keys (sweep, M5).
    #[error("no HD chain")]
    NoHdChain,
    /// Code `compat.network_mismatch`: keys of another network.
    #[error("keys are for {found:?}")]
    NetworkMismatch { found: DashNetwork },
    /// Code `compat.invalid_key_material`: bad seed length, xprv or JSON.
    #[error("invalid key material: {detail}")]
    InvalidKeyMaterial { detail: String },
    /// Code `compat.already_exists`: the wallet is already registered with keys.
    #[error("wallet {wallet_id} already exists")]
    AlreadyExists { wallet_id: String },
    /// Code `compat.no_vault`: create the vault first.
    #[error("no vault")]
    NoVault,
    /// Code `compat.vault_locked`: imports store keys; unlock first.
    #[error("vault locked")]
    VaultLocked,
    /// Code `compat.grant_invalid`: exports need a `RevealSecret` grant for
    /// the wallet.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `compat.watch_only`: a watch-only wallet has no keys to export.
    #[error("watch-only wallet")]
    WatchOnly,
    /// Code `compat.destination_unwritable`: the export file could not be
    /// written.
    #[error("cannot write: {detail}")]
    DestinationUnwritable { detail: String },
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `network_not_open`.
    #[error("network not open: {detail}")]
    NetworkNotOpen { detail: String },
    /// Code `wallet_not_found`.
    #[error("wallet not found: {detail}")]
    WalletNotFound { detail: String },
    /// Code `storage`.
    #[error("storage: {detail}")]
    Storage { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
    /// Code `internal`.
    #[error("internal: {detail}")]
    Internal { detail: String },
}

domain_error_common!(CompatError);
crate::api::common::export_error_code!(CompatError);

impl CompatError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::FileUnreadable { .. } => "compat.file_unreadable",
            Self::UnsupportedFormat { .. } => "compat.unsupported_format",
            Self::Corrupt { .. } => "compat.corrupt",
            Self::PassphraseRequired => "compat.passphrase_required",
            Self::WrongPassphrase => "compat.wrong_passphrase",
            Self::NoHdChain => "compat.no_hd_chain",
            Self::NetworkMismatch { .. } => "compat.network_mismatch",
            Self::InvalidKeyMaterial { .. } => "compat.invalid_key_material",
            Self::AlreadyExists { .. } => "compat.already_exists",
            Self::NoVault => "compat.no_vault",
            Self::VaultLocked => "compat.vault_locked",
            Self::GrantInvalid => "compat.grant_invalid",
            Self::WatchOnly => "compat.watch_only",
            Self::DestinationUnwritable { .. } => "compat.destination_unwritable",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

#[uniffi::export]
impl Engine {
    /// Detects the kind of a user-chosen file from its content, not its
    /// extension. Reads at most what detection needs; decrypts nothing.
    pub async fn inspect_wallet_file(&self, path: String) -> Result<WalletFileKind, CompatError> {
        let _ = path;
        not_implemented("Engine.inspect_wallet_file")
    }
}

#[uniffi::export]
impl NetworkSession {
    /// Imports a Dash Core `dumpwallet` file (QT-107): rebuilds the HD wallet
    /// from the header (mnemonic + passphrase, else HD seed, else xprv;
    /// counters raise the lookahead), carries labels into the address book
    /// and reports the loose keys left out. Vault seed-safety order as
    /// `import_wallet`. `options.core_compat` is implied for a mnemonic.
    pub async fn import_dump_wallet(
        &self,
        path: String,
        options: ImportOptions,
    ) -> Result<ImportReport, CompatError> {
        let _ = (path, options);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.import_dump_wallet")
    }

    /// Restores from a Dash Core `wallet.dat` (QT-106): SQLite descriptor
    /// wallets in M2 (decrypts `walletdescriptorckey` with `mkey` and the
    /// passphrase, extracts mnemonic and passphrase). Berkeley DB files
    /// return `NotImplemented{call: "import_wallet_dat.bdb"}` until M6.
    pub async fn import_wallet_dat(
        &self,
        path: String,
        wallet_passphrase: Option<Vec<u8>>,
        options: ImportOptions,
    ) -> Result<ImportReport, CompatError> {
        let _passphrase = wallet_passphrase.map(Zeroizing::new);
        let _ = (path, options);
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.import_wallet_dat")
    }

    /// Imports a wallet from raw key material (QT-108). An HD seed or xprv
    /// gives a wallet with no phrase (`has_mnemonic = false`); descriptors
    /// must describe BIP44 account 0 of one master key.
    pub async fn import_key_material(
        &self,
        material: KeyMaterial,
        options: ImportOptions,
    ) -> Result<ImportReport, CompatError> {
        let _material = match material {
            KeyMaterial::HdSeed { seed } => Zeroizing::new(seed),
            KeyMaterial::Xprv { xprv } => Zeroizing::new(xprv),
            KeyMaterial::Descriptors { json } => Zeroizing::new(json),
        };
        let _ = options;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.import_key_material")
    }

    /// Writes the wallet for dash-qt to `dest_path` (QT-109 b/c), mode 0600,
    /// replacing nothing (`compat.destination_unwritable` if it exists).
    /// The file holds private keys: needs a `RevealSecret` grant for the
    /// wallet, redeemed after the checks.
    pub async fn export_for_core(
        &self,
        wallet_id: String,
        format: CoreExportFormat,
        dest_path: String,
        grant_id: String,
    ) -> Result<ExportReport, CompatError> {
        let _ = (format, dest_path, grant_id);
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.export_for_core")
    }

    /// Whether "mnemonic + passphrase + `upgradetohd`" in dash-qt rebuilds
    /// this wallet (QT-109 a). The host reveals the phrase with
    /// `Vault.reveal_mnemonic` and shows the `upgradetohd` instructions only
    /// when `core_compatible`.
    pub async fn core_mnemonic_compatibility(
        &self,
        wallet_id: String,
    ) -> Result<CoreMnemonicCompatibility, CompatError> {
        parse_wallet_id(&wallet_id)?;
        ensure_open(&self.inner)?;
        not_implemented("NetworkSession.core_mnemonic_compatibility")
    }
}
