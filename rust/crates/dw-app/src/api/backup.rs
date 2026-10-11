//! Backups (`.dwbackup`), restore and Dash Core compatibility (import and
//! export of dumpwallet, `wallet.dat` and raw key material).
//!
//! Paths cross as strings. Backup passphrases, wallet passphrases and key
//! material are secrets: they arrive as `String`s and are wrapped in
//! `Zeroizing` before anything else touches them (the copy serde decoded
//! them from is a documented limit, G-01 H-01).

use std::path::PathBuf;

use dw_engine::backup::BackupHeader;
use dw_engine::{
    BackupInfo, BackupPolicy, CoreExportFormat, CoreMnemonicCompatibility, ExportReport,
    ExportWarning, ImportOptions, ImportReport, KeyMaterial, WalletFileKind,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::dto::{NetworkInfo, mirror_enum};
use crate::error::AppError;
use crate::ipc::commands;
use crate::state::parse_wallet_id;

pub(crate) const EXCLUDED: &[(&str, &str)] = &[];

/// A `.dwbackup` file: the wallet it holds and when it was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupSummary {
    pub path: String,
    pub wallet_id: String,
    /// UNIX seconds.
    pub created_at: u64,
    pub size_bytes: u64,
    /// Written by the automatic rotation rather than on request.
    pub automatic: bool,
}

impl From<BackupInfo> for BackupSummary {
    fn from(b: BackupInfo) -> Self {
        let BackupInfo {
            path,
            wallet_id,
            created_at,
            size_bytes,
            automatic,
        } = b;
        BackupSummary {
            path: path.to_string_lossy().into_owned(),
            wallet_id: wallet_id.to_string(),
            created_at,
            size_bytes,
            automatic,
        }
    }
}

/// How many automatic backups to keep per wallet, and where they go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupPolicyInfo {
    /// 0..=10; 0 turns automatic backups off.
    pub keep: u32,
    pub directory: String,
}

impl From<BackupPolicy> for BackupPolicyInfo {
    fn from(p: BackupPolicy) -> Self {
        let BackupPolicy { keep, directory } = p;
        BackupPolicyInfo {
            keep,
            directory: directory.to_string_lossy().into_owned(),
        }
    }
}

/// The readable header of a `.dwbackup`: no secrets, nothing decrypted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BackupFileHeader {
    pub format: String,
    pub format_version: u32,
    /// The network's directory name (`mainnet`, `devnet-<name>`, …).
    pub network: String,
    /// UNIX seconds.
    pub created_at: u64,
    /// Lower-case hex wallet ids.
    pub wallet_ids: Vec<String>,
    pub automatic: bool,
    pub app_version: String,
}

impl From<BackupHeader> for BackupFileHeader {
    fn from(h: BackupHeader) -> Self {
        let BackupHeader {
            format,
            format_version,
            network,
            created_at,
            wallet_ids,
            automatic,
            app_version,
        } = h;
        BackupFileHeader {
            format,
            format_version,
            network,
            created_at,
            wallet_ids,
            automatic,
            app_version,
        }
    }
}

/// What a user-chosen file is, detected from its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum WalletFileInfo {
    /// Dash Core `dumpwallet` text.
    DumpWallet {
        /// The network of its keys; `null` when the file has no keys.
        network: Option<NetworkInfo>,
        has_mnemonic: bool,
        has_hd_seed: bool,
        has_xprv: bool,
        /// Loose (non-HD) WIF keys, which an import leaves out.
        loose_key_count: u32,
        script_count: u32,
        label_count: u32,
    },
    /// Dash Core v21+ SQLite descriptor `wallet.dat`.
    WalletDatSqlite {
        encrypted: bool,
        has_mnemonic: bool,
    },
    /// Legacy Berkeley DB `wallet.dat`: detected, not importable yet.
    WalletDatBdb { encrypted: Option<bool> },
    /// This app's `.dwbackup` (`restoreBackup`).
    DwBackup {
        /// `null` when the backup names a network this build does not know.
        network: Option<NetworkInfo>,
        wallet_count: u32,
        created_at: u64,
        format_version: u32,
    },
    Psbt,
    Unknown,
}

impl From<WalletFileKind> for WalletFileInfo {
    fn from(k: WalletFileKind) -> Self {
        match k {
            WalletFileKind::DumpWallet {
                network,
                has_mnemonic,
                has_hd_seed,
                has_xprv,
                loose_key_count,
                script_count,
                label_count,
            } => WalletFileInfo::DumpWallet {
                network: network.as_ref().map(NetworkInfo::from),
                has_mnemonic,
                has_hd_seed,
                has_xprv,
                loose_key_count,
                script_count,
                label_count,
            },
            WalletFileKind::WalletDatSqlite {
                encrypted,
                has_mnemonic,
            } => WalletFileInfo::WalletDatSqlite {
                encrypted,
                has_mnemonic,
            },
            WalletFileKind::WalletDatBdb { encrypted } => {
                WalletFileInfo::WalletDatBdb { encrypted }
            }
            WalletFileKind::DwBackup {
                network,
                wallet_count,
                created_at,
                format_version,
            } => WalletFileInfo::DwBackup {
                network: network.as_ref().map(NetworkInfo::from),
                wallet_count,
                created_at,
                format_version,
            },
            WalletFileKind::Psbt => WalletFileInfo::Psbt,
            WalletFileKind::Unknown => WalletFileInfo::Unknown,
        }
    }
}

/// Options of an import from Dash Core.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
pub struct CoreImportOptions {
    /// First block to scan; `0` is genesis. `null`: the importer's default
    /// (genesis for a Core wallet, since it may have funds anywhere).
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 rules.
    pub core_compat: bool,
    /// Display name, 1 to 64 characters; `null`: "Wallet N".
    pub name: Option<String>,
    /// Address lookahead (gap limit), 1..=1000; `null`: the default.
    pub lookahead: Option<u32>,
}

impl From<CoreImportOptions> for ImportOptions {
    fn from(o: CoreImportOptions) -> Self {
        let CoreImportOptions {
            birth_height,
            core_compat,
            name,
            lookahead,
        } = o;
        ImportOptions {
            birth_height,
            core_compat,
            name,
            lookahead,
        }
    }
}

/// Raw key material for "Import from hdseed, xprv or listdescriptors". A
/// secret: it is wrapped in `Zeroizing` on conversion and never printed.
#[derive(Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum KeyMaterialInput {
    /// A BIP32 seed, 16 to 64 bytes, as hex (dashd `dumphdinfo` `hdseed`).
    HdSeed { seed_hex: String },
    /// The master extended private key, base58.
    Xprv { xprv: String },
    /// dashd `listdescriptors true` JSON.
    Descriptors { json: String },
}

impl std::fmt::Debug for KeyMaterialInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            KeyMaterialInput::HdSeed { .. } => "HdSeed",
            KeyMaterialInput::Xprv { .. } => "Xprv",
            KeyMaterialInput::Descriptors { .. } => "Descriptors",
        };
        write!(f, "KeyMaterialInput::{kind}(<redacted>)")
    }
}

impl KeyMaterialInput {
    fn into_engine(self) -> Result<KeyMaterial, AppError> {
        Ok(match self {
            KeyMaterialInput::HdSeed { seed_hex } => {
                let seed_hex = Zeroizing::new(seed_hex);
                let seed = hex::decode(seed_hex.trim())
                    .map_err(|_| AppError::invalid_argument("the HD seed is not hex"))?;
                KeyMaterial::HdSeed(Zeroizing::new(seed))
            }
            KeyMaterialInput::Xprv { xprv } => {
                KeyMaterial::Xprv(Zeroizing::new(xprv.into_bytes()))
            }
            KeyMaterialInput::Descriptors { json } => {
                KeyMaterial::Descriptors(Zeroizing::new(json.into_bytes()))
            }
        })
    }
}

/// The wallet an import registered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreImportReport {
    pub wallet_id: String,
    pub labels_imported: u32,
    pub keys_not_imported: u32,
    pub scripts_not_imported: u32,
    /// The seed was derived with Dash Core's BIP39 rules.
    pub core_compat_seed: bool,
}

impl From<ImportReport> for CoreImportReport {
    fn from(r: ImportReport) -> Self {
        let ImportReport {
            wallet_id,
            labels_imported,
            keys_not_imported,
            scripts_not_imported,
            core_compat_seed,
        } = r;
        CoreImportReport {
            wallet_id: wallet_id.to_string(),
            labels_imported,
            keys_not_imported,
            scripts_not_imported,
            core_compat_seed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CoreExportFormatName {
    /// `dumpwallet` text.
    DumpWallet,
    /// `importdescriptors` JSON.
    ImportDescriptorsJson,
}

mirror_enum!(CoreExportFormatName => CoreExportFormat { DumpWallet, ImportDescriptorsJson });

mirror_enum!(CoreExportFormat => CoreExportFormatName { DumpWallet, ImportDescriptorsJson });

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum CoreExportWarningName {
    MnemonicNotCoreCompatible,
    CoinJoinAccountNotScannedByLegacyCore,
}

mirror_enum!(ExportWarning => CoreExportWarningName {
    MnemonicNotCoreCompatible, CoinJoinAccountNotScannedByLegacyCore
});

/// What an export to Dash Core wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreExportReport {
    pub path: String,
    pub format: CoreExportFormatName,
    pub key_count: u32,
    pub warnings: Vec<CoreExportWarningName>,
}

impl From<ExportReport> for CoreExportReport {
    fn from(r: ExportReport) -> Self {
        let ExportReport {
            path,
            format,
            key_count,
            warnings,
        } = r;
        CoreExportReport {
            path: path.to_string_lossy().into_owned(),
            format: format.into(),
            key_count,
            warnings: warnings.into_iter().map(Into::into).collect(),
        }
    }
}

/// Whether Dash Core can rebuild the wallet from its phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CoreMnemonicCheck {
    pub core_compatible: bool,
    pub warnings: Vec<CoreExportWarningName>,
}

impl From<CoreMnemonicCompatibility> for CoreMnemonicCheck {
    fn from(c: CoreMnemonicCompatibility) -> Self {
        let CoreMnemonicCompatibility {
            core_compatible,
            warnings,
        } = c;
        CoreMnemonicCheck {
            core_compatible,
            warnings: warnings.into_iter().map(Into::into).collect(),
        }
    }
}

/// A secret the renderer sent, as the bytes the engine takes.
fn secret_bytes(s: Option<String>) -> Option<Zeroizing<Vec<u8>>> {
    s.map(|s| Zeroizing::new(s.into_bytes()))
}

commands! {
    /// dash-qt "Backup Wallet": writes a `.dwbackup` of the wallet to
    /// `destPath` (never over an existing file). `backupPassphrase` wraps
    /// the data key in the file; without one, the file opens only with this
    /// vault's passphrase. Watch-only wallets have nothing to back up.
    #[facade("NetworkSession::backup_wallet")]
    fn backup_wallet(
        state,
        wallet_id: String,
        dest_path: String,
        backup_passphrase: Option<String>,
    ) -> BackupSummary {
        let passphrase = secret_bytes(backup_passphrase);
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session
            .backup_wallet(id, PathBuf::from(dest_path), passphrase)
            .await?
            .into())
    }

    /// Restores the wallets of a `.dwbackup`: verifies it, stores each
    /// wallet's secrets under this vault's key and registers it. Returns
    /// the wallet ids.
    #[facade("NetworkSession::restore_backup")]
    fn restore_backup(state, path: String, passphrase: Option<String>) -> Vec<String> {
        let passphrase = secret_bytes(passphrase);
        let session = state.session().await?;
        let ids = session.restore_backup(PathBuf::from(path), passphrase).await?;
        Ok(ids.into_iter().map(|id| id.to_string()).collect())
    }

    /// Automatic backups of a wallet (every wallet when `walletId` is
    /// `null`), newest first.
    #[facade("NetworkSession::automatic_backups")]
    fn automatic_backups(state, wallet_id: Option<String>) -> Vec<BackupSummary> {
        let id = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let backups = state.session().await?.automatic_backups(id).await?;
        Ok(backups.into_iter().map(Into::into).collect())
    }

    /// The directory of the network's automatic backups.
    #[facade("NetworkSession::backup_directory")]
    fn backup_directory(state) -> String {
        let dir = state.session().await?.backup_directory();
        Ok(dir.to_string_lossy().into_owned())
    }

    /// How many automatic backups are kept per wallet, and where.
    #[facade("NetworkSession::backup_policy")]
    fn backup_policy(state) -> BackupPolicyInfo {
        Ok(state.session().await?.backup_policy()?.into())
    }

    /// Sets how many automatic backups to keep per wallet (0..=10) and
    /// deletes the oldest beyond it.
    #[facade("NetworkSession::set_backup_policy")]
    fn set_backup_policy(state, keep: u32) -> BackupPolicyInfo {
        Ok(state.session().await?.set_backup_policy(keep).await?.into())
    }

    /// Reads a `.dwbackup`'s header: which wallets and network it holds.
    /// Decrypts nothing and opens no session.
    #[facade("dw_engine::backup::read_header")]
    fn read_backup_header(state, path: String) -> BackupFileHeader {
        let path = PathBuf::from(path);
        let header = tokio::task::spawn_blocking(move || dw_engine::backup::read_header(&path))
            .await
            .map_err(dw_engine::EngineError::from)??;
        Ok(header.into())
    }

    /// Detects what a user-chosen file is from its content, not its
    /// extension. Reads what detection needs and decrypts nothing; opens no
    /// session.
    #[facade("Engine::inspect_wallet_file")]
    fn inspect_wallet_file(state, path: String) -> WalletFileInfo {
        if path.is_empty() {
            return Err(AppError::invalid_argument("empty path"));
        }
        let kind = state.engine()?.inspect_wallet_file(PathBuf::from(path)).await?;
        Ok(kind.into())
    }

    /// Imports a Dash Core `dumpwallet` file: rebuilds the HD wallet from
    /// its header, carries the labels into the address book and reports the
    /// loose keys left out.
    #[facade("NetworkSession::import_dump_wallet")]
    fn import_dump_wallet(
        state,
        path: String,
        options: CoreImportOptions,
    ) -> CoreImportReport {
        let session = state.session().await?;
        Ok(session
            .import_dump_wallet(PathBuf::from(path), options.into())
            .await?
            .into())
    }

    /// Restores from a Dash Core `wallet.dat` (SQLite descriptor wallets;
    /// Berkeley DB is `not_implemented`). `walletPassphrase` opens an
    /// encrypted file.
    #[facade("NetworkSession::import_wallet_dat")]
    fn import_wallet_dat(
        state,
        path: String,
        wallet_passphrase: Option<String>,
        options: CoreImportOptions,
    ) -> CoreImportReport {
        let passphrase = secret_bytes(wallet_passphrase);
        let session = state.session().await?;
        Ok(session
            .import_wallet_dat(PathBuf::from(path), passphrase, options.into())
            .await?
            .into())
    }

    /// Imports a wallet from an HD seed, xprv or `listdescriptors true`
    /// JSON.
    #[facade("NetworkSession::import_key_material")]
    fn import_key_material(
        state,
        material: KeyMaterialInput,
        options: CoreImportOptions,
    ) -> CoreImportReport {
        let material = material.into_engine()?;
        let session = state.session().await?;
        Ok(session.import_key_material(material, options.into()).await?.into())
    }

    /// Whether "phrase + passphrase + `upgradetohd`" in dash-qt rebuilds
    /// the wallet. Needs the data key (unlocked or unencrypted vault).
    #[facade("NetworkSession::core_mnemonic_compatibility")]
    fn core_mnemonic_compatibility(state, wallet_id: String) -> CoreMnemonicCheck {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(state.session().await?.core_mnemonic_compatibility(id).await?.into())
    }

    /// Writes the wallet for dash-qt to `destPath` (mode 0600, never over an
    /// existing file). The file holds private keys: needs a `revealSecret`
    /// grant for the wallet (`grantId`), redeemed after the checks.
    #[facade("NetworkSession::export_for_core")]
    fn export_for_core(
        state,
        wallet_id: String,
        format: CoreExportFormatName,
        dest_path: String,
        grant_id: String,
    ) -> CoreExportReport {
        let id = parse_wallet_id(&wallet_id)?;
        let session = state.session().await?;
        Ok(session
            .export_for_core(id, format.into(), PathBuf::from(dest_path), grant_id)
            .await?
            .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_material_decodes_by_kind_and_never_prints() {
        let seed: KeyMaterialInput =
            serde_json::from_str(r#"{"kind":"hdSeed","seedHex":"00ff10"}"#).unwrap();
        assert!(!format!("{seed:?}").contains("00ff10"));
        match seed.into_engine().unwrap() {
            KeyMaterial::HdSeed(s) => assert_eq!(&s[..], [0x00, 0xff, 0x10]),
            _ => panic!("not a seed"),
        }
        let xprv: KeyMaterialInput =
            serde_json::from_str(r#"{"kind":"xprv","xprv":"xprv9s21"}"#).unwrap();
        assert!(!format!("{xprv:?}").contains("xprv9s21"));
        match xprv.into_engine().unwrap() {
            KeyMaterial::Xprv(x) => assert_eq!(&x[..], b"xprv9s21"),
            _ => panic!("not an xprv"),
        }
        let json: KeyMaterialInput =
            serde_json::from_str(r#"{"kind":"descriptors","json":"{}"}"#).unwrap();
        assert!(matches!(
            json.into_engine().unwrap(),
            KeyMaterial::Descriptors(_)
        ));
        let bad: KeyMaterialInput =
            serde_json::from_str(r#"{"kind":"hdSeed","seedHex":"zz"}"#).unwrap();
        let err = bad.into_engine().err().expect("not hex");
        assert_eq!(err.code_str(), "invalid_argument");
        assert!(!err.to_string().contains("zz"));
    }

    #[test]
    fn import_options_default_when_left_out() {
        let o: CoreImportOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(ImportOptions::from(o), ImportOptions::default());
        let o: CoreImportOptions =
            serde_json::from_str(r#"{"birthHeight":0,"coreCompat":true,"lookahead":1000}"#)
                .unwrap();
        let o = ImportOptions::from(o);
        assert_eq!(o.birth_height, Some(0));
        assert!(o.core_compat);
        assert_eq!(o.lookahead, Some(1000));
    }

    #[test]
    fn passphrases_become_zeroizing_bytes() {
        assert!(secret_bytes(None).is_none());
        assert_eq!(&secret_bytes(Some("pässword".into())).unwrap()[..], "pässword".as_bytes());
    }

    #[test]
    fn wallet_file_kinds_are_tagged() {
        let json = serde_json::to_value(WalletFileInfo::from(WalletFileKind::WalletDatSqlite {
            encrypted: true,
            has_mnemonic: false,
        }))
        .unwrap();
        assert_eq!(json["kind"], "walletDatSqlite");
        assert_eq!(json["hasMnemonic"], false);
        let json = serde_json::to_value(WalletFileInfo::from(WalletFileKind::DwBackup {
            network: Some(dw_engine::DashNetwork::Testnet),
            wallet_count: 2,
            created_at: 9,
            format_version: 1,
        }))
        .unwrap();
        assert_eq!(json["kind"], "dwBackup");
        assert_eq!(json["network"]["dirName"], "testnet");
        assert_eq!(json["walletCount"], 2);
        let json = serde_json::to_value(WalletFileInfo::from(WalletFileKind::Psbt)).unwrap();
        assert_eq!(json, serde_json::json!({ "kind": "psbt" }));
    }

    #[test]
    fn a_backup_header_keeps_every_field() {
        let h = BackupFileHeader::from(BackupHeader {
            format: "dwbackup".into(),
            format_version: 1,
            network: "devnet-x".into(),
            created_at: 7,
            wallet_ids: vec!["aa".into()],
            automatic: true,
            app_version: "0.1".into(),
        });
        let json = serde_json::to_value(&h).unwrap();
        assert_eq!(json["walletIds"][0], "aa");
        assert_eq!(json["appVersion"], "0.1");
        assert_eq!(json["formatVersion"], 1);
    }
}
