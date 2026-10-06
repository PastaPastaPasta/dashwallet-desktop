//! M2 wallet backups (QT-110, QT-116): `.dwbackup` bundles and automatic
//! rotating backups. Owner: R2 (compat, `dw-engine::backup`).
//! Contract: docs/contracts/m2-engine.md §2.7.
//!
//! A `.dwbackup` is a versioned container: the wallet's vault records (still
//! encrypted under the data key), a passphrase wrap slot of that key, the
//! `app.sqlite` rows of the wallet and an online backup of `wallet.sqlite`,
//! authenticated with a MAC under the data key (DESIGN-opus §2 backups row).

use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::NetworkSession;
use crate::api::common::{domain_error_common, parse_wallet_id};

/// One backup file.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BackupInfo {
    pub path: String,
    /// The wallet it holds.
    pub wallet_id: String,
    pub created_at: u64,
    pub size_bytes: u64,
    /// Written by the rotation (QT-116), not by the user (QT-110).
    pub automatic: bool,
}

/// Automatic backup rotation (dash-qt `-createwalletbackups`, default 10,
/// max 10; 0 = off).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct BackupPolicy {
    pub keep: u32,
    /// `<network dir>/backups` ("Show Automatic Backups").
    pub directory: String,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BackupError {
    /// Code `backup.vault_locked`: writing or restoring needs the data key.
    #[error("vault locked")]
    VaultLocked,
    /// Code `backup.passphrase_required`: the vault is unencrypted, so the
    /// backup needs its own passphrase (`backup_passphrase`); restoring an
    /// encrypted backup needs it too.
    #[error("passphrase required")]
    PassphraseRequired,
    /// Code `backup.wrong_passphrase`.
    #[error("wrong passphrase")]
    WrongPassphrase,
    /// Code `backup.corrupt`: the MAC or the container failed to verify.
    #[error("corrupt: {detail}")]
    Corrupt { detail: String },
    /// Code `backup.unsupported_version`: written by a newer app.
    #[error("unsupported version {version}")]
    UnsupportedVersion { version: u32 },
    /// Code `backup.network_mismatch`: a backup of another network.
    #[error("backup is for another network")]
    NetworkMismatch,
    /// Code `backup.already_exists`: the wallet is registered with keys.
    #[error("wallet {wallet_id} already exists")]
    AlreadyExists { wallet_id: String },
    /// Code `backup.destination_unwritable`: the file could not be written
    /// (or exists; backups replace nothing).
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

domain_error_common!(@not_implemented BackupError);
crate::api::common::export_error_code!(BackupError);

impl From<dw_engine::EngineError> for BackupError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::BackupFailure as F;
        use dw_engine::EngineError as E;
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::Backup(f) => match f {
                F::VaultLocked => Self::VaultLocked,
                F::PassphraseRequired => Self::PassphraseRequired,
                F::WrongPassphrase => Self::WrongPassphrase,
                F::Corrupt(detail) => Self::Corrupt { detail },
                F::UnsupportedVersion(version) => Self::UnsupportedVersion { version },
                F::NetworkMismatch => Self::NetworkMismatch,
                F::AlreadyExists(id) => Self::AlreadyExists {
                    wallet_id: id.to_string(),
                },
                F::DestinationUnwritable(detail) => Self::DestinationUnwritable { detail },
            },
            E::WalletAlreadyExists(wallet_id) => Self::AlreadyExists { wallet_id },
            E::Vault(V::NoVault | V::Locked | V::MixingOnly) => Self::VaultLocked,
            E::InvalidConfig(_) | E::InvalidArgument(_) | E::NameRejected(_) => {
                Self::InvalidArgument { detail }
            }
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            _ => Self::Internal { detail },
        }
    }
}

impl From<dw_engine::BackupInfo> for BackupInfo {
    fn from(b: dw_engine::BackupInfo) -> Self {
        Self {
            path: b.path.to_string_lossy().into_owned(),
            wallet_id: b.wallet_id.to_string(),
            created_at: b.created_at,
            size_bytes: b.size_bytes,
            automatic: b.automatic,
        }
    }
}

impl From<dw_engine::BackupPolicy> for BackupPolicy {
    fn from(p: dw_engine::BackupPolicy) -> Self {
        Self {
            keep: p.keep,
            directory: p.directory.to_string_lossy().into_owned(),
        }
    }
}

impl BackupError {
    /// Stable code (docs/contracts/m2-engine.md §4).
    fn code_str(&self) -> &'static str {
        match self {
            Self::VaultLocked => "backup.vault_locked",
            Self::PassphraseRequired => "backup.passphrase_required",
            Self::WrongPassphrase => "backup.wrong_passphrase",
            Self::Corrupt { .. } => "backup.corrupt",
            Self::UnsupportedVersion { .. } => "backup.unsupported_version",
            Self::NetworkMismatch => "backup.network_mismatch",
            Self::AlreadyExists { .. } => "backup.already_exists",
            Self::DestinationUnwritable { .. } => "backup.destination_unwritable",
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
impl NetworkSession {
    /// dash-qt "Backup Wallet…" (QT-110): writes a `.dwbackup` of one wallet
    /// to `dest_path`. Encrypted vault: wraps with the vault passphrase slot
    /// and needs the vault unlocked (`backup_passphrase` must be `None`).
    /// Unencrypted vault: `backup_passphrase` is required and wraps the
    /// bundle. Watch-only wallets carry no vault records:
    /// `NotImplemented{call: "backup_wallet.watch_only"}`.
    pub async fn backup_wallet(
        &self,
        wallet_id: String,
        dest_path: String,
        backup_passphrase: Option<Vec<u8>>,
    ) -> Result<BackupInfo, BackupError> {
        let passphrase = backup_passphrase.map(Zeroizing::new);
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self
            .inner
            .backup_wallet(id, PathBuf::from(dest_path), passphrase)
            .await?
            .into())
    }

    /// Restores the wallets of a `.dwbackup` into this network: re-encrypts
    /// their records under this vault's key (vault unlocked), restores app
    /// metadata and wallet state, and registers them in seed-safety order.
    /// Returns the wallet ids. `passphrase` unwraps the bundle.
    pub async fn restore_backup(
        &self,
        path: String,
        passphrase: Option<Vec<u8>>,
    ) -> Result<Vec<String>, BackupError> {
        let passphrase = passphrase.map(Zeroizing::new);
        Ok(self
            .inner
            .restore_backup(PathBuf::from(path), passphrase)
            .await?
            .into_iter()
            .map(|id| id.to_string())
            .collect())
    }

    /// Automatic backups of `wallet_id` (all wallets when `None`), newest
    /// first ("Show Automatic Backups", QT-116). The engine writes one
    /// `<wallet>.YYYY-MM-DD-HH-MM.dwbackup` when a wallet loads or is created
    /// and the data key is available, keeps the newest `keep`, and sends
    /// `Notice{BackupFailed}` when writing fails.
    pub async fn automatic_backups(
        &self,
        wallet_id: Option<String>,
    ) -> Result<Vec<BackupInfo>, BackupError> {
        let id = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        Ok(self
            .inner
            .automatic_backups(id)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn backup_policy(&self) -> Result<BackupPolicy, BackupError> {
        Ok(self.inner.backup_policy()?.into())
    }

    /// `keep` in 0..=10 (`invalid_argument` otherwise). Lowering it deletes
    /// the oldest automatic backups beyond the new count.
    pub async fn set_backup_policy(&self, keep: u32) -> Result<BackupPolicy, BackupError> {
        Ok(self.inner.set_backup_policy(keep).await?.into())
    }
}
