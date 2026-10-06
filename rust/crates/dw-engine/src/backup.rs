//! Wallet backups (QT-110, QT-116): `.dwbackup` files written on request
//! and rotated automatic backups. Format: docs/contracts/dwbackup-v1.md.
//!
//! ```text
//! DWBACKUP 1\n
//! <header JSON>\n      network, created_at, wallet ids, automatic (plain, authenticated)
//! <body JSON>\n        {"bundles": [dw-vault WalletBackupBundle, …]}
//! ```
//!
//! Each bundle (dw-vault) carries the wallet's secrets and a payload, both
//! sealed under a random key of the bundle's own, and a slot that wraps only
//! that key; the header line is the payload's AAD. The payload is JSON: the
//! wallet's name, birth height and creation time and its app.sqlite rows.
//! It holds nothing of other wallets (review M4: the wallet.sqlite snapshot
//! of the whole network is no longer written) and is built in memory: no
//! temporary file is written.
//!
//! Restoring registers the wallet from its stored seed in seed-safety order
//! with the backup's birth height and name, then inserts its app.sqlite
//! rows. Wallet state is rebuilt by the scan from the birth height.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine as _;
use dw_appdb::{SqlValue, TableRows};
use dw_vault::{LockState, VaultError, WalletBackupBundle};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::compat::write_new_private;
use crate::events::unix_now;
use crate::keys::AutoBackup;
use crate::{
    DashNetwork, EngineError, EngineEvent, ImportOptions, NetworkSession, NoticeCode, WalletId,
};

/// First bytes of every `.dwbackup` file.
pub const MAGIC: &[u8] = b"DWBACKUP ";
/// The format this engine writes and reads.
pub const FORMAT_VERSION: u32 = 1;
/// Automatic backups kept per wallet by default and at most (dash-qt
/// `-createwalletbackups`).
pub const DEFAULT_KEEP: u32 = 10;
pub const MAX_KEEP: u32 = 10;
/// Directory of automatic backups inside the network directory.
pub const BACKUP_DIR: &str = "backups";
/// File extension.
pub const EXTENSION: &str = "dwbackup";
/// Largest backup file read back.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// dw-appdb setting holding the automatic backup count.
const KEEP_SETTING: &str = "backup.keep";

/// Why a backup call failed (`backup.*` codes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupFailure {
    /// The data key is not available (locked, mixing-only or no vault).
    VaultLocked,
    /// An unencrypted vault needs a backup passphrase; a backup with a
    /// passphrase slot needs it to restore.
    PassphraseRequired,
    WrongPassphrase,
    Corrupt(String),
    UnsupportedVersion(u32),
    NetworkMismatch,
    AlreadyExists(WalletId),
    DestinationUnwritable(String),
}

impl std::fmt::Display for BackupFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::VaultLocked => f.write_str("vault locked"),
            Self::PassphraseRequired => f.write_str("passphrase required"),
            Self::WrongPassphrase => f.write_str("wrong passphrase"),
            Self::Corrupt(d) => write!(f, "corrupt: {d}"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported version {v}"),
            Self::NetworkMismatch => f.write_str("backup is for another network"),
            Self::AlreadyExists(id) => write!(f, "wallet {id} already exists"),
            Self::DestinationUnwritable(d) => write!(f, "cannot write: {d}"),
        }
    }
}

impl From<BackupFailure> for EngineError {
    fn from(f: BackupFailure) -> Self {
        EngineError::Backup(f)
    }
}

/// One backup file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub wallet_id: WalletId,
    pub created_at: u64,
    pub size_bytes: u64,
    pub automatic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupPolicy {
    pub keep: u32,
    pub directory: PathBuf,
}

/// The plain header line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupHeader {
    pub format: String,
    pub format_version: u32,
    /// Network directory name (`mainnet`, `testnet`, `regtest`, `devnet-<name>`).
    pub network: String,
    pub created_at: u64,
    /// Hex wallet ids, in bundle order.
    pub wallet_ids: Vec<String>,
    pub automatic: bool,
    pub app_version: String,
}

impl BackupHeader {
    /// The network the header names, when it is one this engine knows.
    pub fn network(&self) -> Option<DashNetwork> {
        match self.network.as_str() {
            "mainnet" => Some(DashNetwork::Mainnet),
            "testnet" => Some(DashNetwork::Testnet),
            "regtest" => Some(DashNetwork::Regtest),
            other => other
                .strip_prefix("devnet-")
                .map(|n| DashNetwork::Devnet { name: n.to_owned() }),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Body {
    bundles: Vec<WalletBackupBundle>,
}

/// Parses a body, checking each bundle's version on the raw JSON first: a
/// bundle version this build does not read is `UnsupportedVersion` (a newer
/// bundle may not even parse as this build's bundle), anything unparsable
/// `Corrupt`.
fn parse_body(json: &[u8]) -> Result<Body, EngineError> {
    #[derive(Deserialize)]
    struct RawBody {
        bundles: Vec<serde_json::Value>,
    }
    let corrupt = |e: serde_json::Error| BackupFailure::Corrupt(format!("body: {e}"));
    let raw: RawBody = serde_json::from_slice(json).map_err(corrupt)?;
    let mut bundles = Vec::with_capacity(raw.bundles.len());
    for bundle in raw.bundles {
        let version = bundle
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| BackupFailure::Corrupt("bundle version".into()))?;
        if !dw_vault::reads_bundle_version(version) {
            return Err(BackupFailure::UnsupportedVersion(
                u32::try_from(version).unwrap_or(u32::MAX),
            )
            .into());
        }
        bundles.push(serde_json::from_value(bundle).map_err(corrupt)?);
    }
    Ok(Body { bundles })
}

/// Plaintext of a bundle's payload.
#[derive(Serialize, Deserialize)]
struct Payload {
    name: Option<String>,
    birth_height: Option<u32>,
    created_at: Option<u64>,
    app_rows: Vec<JsonTable>,
    // Backups written before review M4 also carry `wallet_sqlite` (base64
    // of the whole network's wallet.sqlite); it was never read back and is
    // ignored.
}

#[derive(Serialize, Deserialize)]
struct JsonTable {
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
}

fn to_json(v: &SqlValue) -> serde_json::Value {
    use serde_json::json;
    match v {
        SqlValue::Null => serde_json::Value::Null,
        SqlValue::Integer(i) => json!({ "i": i }),
        SqlValue::Real(r) => json!({ "r": r }),
        SqlValue::Text(t) => json!({ "t": t }),
        SqlValue::Blob(b) => json!({ "b": base64::engine::general_purpose::STANDARD.encode(b) }),
    }
}

fn from_json(v: &serde_json::Value) -> Result<SqlValue, EngineError> {
    let bad = || BackupFailure::Corrupt("app row value".into());
    if v.is_null() {
        return Ok(SqlValue::Null);
    }
    let obj = v.as_object().ok_or_else(bad)?;
    let (k, val) = obj.iter().next().ok_or_else(bad)?;
    Ok(match k.as_str() {
        "i" => SqlValue::Integer(val.as_i64().ok_or_else(bad)?),
        "r" => SqlValue::Real(val.as_f64().ok_or_else(bad)?),
        "t" => SqlValue::Text(val.as_str().ok_or_else(bad)?.to_owned()),
        "b" => SqlValue::Blob(
            base64::engine::general_purpose::STANDARD
                .decode(val.as_str().ok_or_else(bad)?)
                .map_err(|_| bad())?,
        ),
        _ => return Err(bad().into()),
    })
}

/// The header of a `.dwbackup` file (no decryption).
pub fn read_header(path: &Path) -> Result<BackupHeader, EngineError> {
    let mut start = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(64 * 1024).read_to_end(&mut start))
        .map_err(|e| BackupFailure::Corrupt(e.to_string()))?;
    let (header, _) = split_header(&start)?;
    Ok(header)
}

/// Splits `bytes` after the header line: (header, header line bytes, rest).
fn split_header(bytes: &[u8]) -> Result<(BackupHeader, &[u8]), EngineError> {
    let corrupt = |d: &str| EngineError::from(BackupFailure::Corrupt(d.into()));
    let rest = bytes
        .strip_prefix(MAGIC)
        .ok_or_else(|| corrupt("not a .dwbackup file"))?;
    let nl = rest
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| corrupt("magic line"))?;
    let version: u32 = std::str::from_utf8(&rest[..nl])
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| corrupt("version"))?;
    if version != FORMAT_VERSION {
        return Err(BackupFailure::UnsupportedVersion(version).into());
    }
    let rest = &rest[nl + 1..];
    let nl = rest
        .iter()
        .position(|b| *b == b'\n')
        .ok_or_else(|| corrupt("header line"))?;
    let header: BackupHeader =
        serde_json::from_slice(&rest[..nl]).map_err(|e| corrupt(&format!("header: {e}")))?;
    if header.format != "dwbackup" || header.format_version != version {
        return Err(corrupt("header format"));
    }
    Ok((header, &rest[..nl]))
}

/// `<wallet id>.YYYY-MM-DD-HH-MM.dwbackup` (UTC), dash-qt's automatic name
/// with the wallet id for the wallet name.
fn automatic_name(id: &WalletId, at: u64) -> String {
    let iso = dw_compat::dump::format_iso8601(at as i64);
    // "YYYY-MM-DDTHH:MM:SSZ" → "YYYY-MM-DD-HH-MM"
    let stamp = format!("{}-{}-{}", &iso[..10], &iso[11..13], &iso[14..16]);
    format!("{id}.{stamp}.{EXTENSION}")
}

/// The wallet id of an automatic backup file name.
fn automatic_wallet(name: &str) -> Option<WalletId> {
    let stem = name.strip_suffix(&format!(".{EXTENSION}"))?;
    let (id, stamp) = stem.split_once('.')?;
    (stamp.len() == 16).then_some(())?;
    id.parse().ok()
}

fn vault_failure(e: VaultError) -> EngineError {
    match e {
        VaultError::Locked | VaultError::MixingOnly | VaultError::NoVault => {
            BackupFailure::VaultLocked.into()
        }
        VaultError::NotEncrypted => BackupFailure::PassphraseRequired.into(),
        VaultError::WrongPassphrase { .. } => BackupFailure::WrongPassphrase.into(),
        VaultError::Corrupt(d) => BackupFailure::Corrupt(d).into(),
        other => EngineError::Vault(other),
    }
}

impl NetworkSession {
    /// `<network dir>/backups`.
    pub fn backup_directory(&self) -> PathBuf {
        self.data_dir().join(BACKUP_DIR)
    }

    fn backup_keep(&self) -> Result<u32, EngineError> {
        let appdb = self.live()?.appdb;
        Ok(appdb
            .setting("", KEEP_SETTING)?
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_KEEP)
            .min(MAX_KEEP))
    }

    /// Builds the file bytes of a backup of `id` in memory. Reads the
    /// wallet's app.sqlite rows; needs the data key.
    async fn backup_bytes(
        &self,
        id: WalletId,
        backup_passphrase: Option<Zeroizing<Vec<u8>>>,
        automatic: bool,
    ) -> Result<Zeroizing<Vec<u8>>, EngineError> {
        let live = self.live()?;
        let name = self.hub.name_of(&id);
        let birth_height = self.hub.wallet_state(&id).map(|s| s.birth_height);
        let header = BackupHeader {
            format: "dwbackup".into(),
            format_version: FORMAT_VERSION,
            network: self.network.dir_name(),
            created_at: unix_now(),
            wallet_ids: vec![id.to_string()],
            automatic,
            app_version: env!("CARGO_PKG_VERSION").into(),
        };
        let header_line = serde_json::to_vec(&header)
            .map_err(|e| EngineError::Internal(format!("backup header: {e}")))?;
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || -> Result<Zeroizing<Vec<u8>>, EngineError> {
            let rows = live.appdb.export_wallet_rows(&id.to_string())?;
            let payload = Payload {
                name: name.as_ref().map(|n| n.name.clone()),
                birth_height,
                created_at: name.and_then(|n| n.created_at),
                app_rows: rows
                    .iter()
                    .map(|t| JsonTable {
                        table: t.table.clone(),
                        columns: t.columns.clone(),
                        rows: t
                            .rows
                            .iter()
                            .map(|r| r.iter().map(to_json).collect())
                            .collect(),
                    })
                    .collect(),
            };
            let plain = Zeroizing::new(
                serde_json::to_vec(&payload)
                    .map_err(|e| EngineError::Internal(format!("backup payload: {e}")))?,
            );
            let bundle = vault
                .backup_bundle(
                    &id.0,
                    backup_passphrase.as_deref().map(|p| &p[..]),
                    &plain,
                    &header_line,
                )
                .map_err(vault_failure)?;
            let body = serde_json::to_vec(&Body {
                bundles: vec![bundle],
            })
            .map_err(|e| EngineError::Internal(format!("backup body: {e}")))?;
            let mut out = Zeroizing::new(Vec::with_capacity(body.len() + header_line.len() + 16));
            out.extend_from_slice(MAGIC);
            out.extend_from_slice(format!("{FORMAT_VERSION}\n").as_bytes());
            out.extend_from_slice(&header_line);
            out.push(b'\n');
            out.extend_from_slice(&body);
            out.push(b'\n');
            Ok(out)
        })
        .await?
    }

    /// dash-qt "Backup Wallet…" (QT-110). See docs/contracts/m2-engine.md §2.7.
    pub async fn backup_wallet(
        self: &Arc<Self>,
        wallet_id: WalletId,
        dest: PathBuf,
        backup_passphrase: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<BackupInfo, EngineError> {
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            this.require_wallet(&wallet_id)?;
            if !this.vault.has_wallet_secret(&wallet_id.0) {
                return Err(EngineError::NotImplemented(
                    "backup_wallet.watch_only".into(),
                ));
            }
            if dest.as_os_str().is_empty() {
                return Err(EngineError::InvalidArgument(
                    "empty destination path".into(),
                ));
            }
            if dest.exists() {
                return Err(BackupFailure::DestinationUnwritable(format!(
                    "{} exists; backups replace no file",
                    dest.display()
                ))
                .into());
            }
            let encrypted = this.vault.status().encrypted;
            match (encrypted, &backup_passphrase) {
                (true, Some(_)) => {
                    return Err(EngineError::InvalidArgument(
                        "an encrypted vault's backups use the vault passphrase".into(),
                    ));
                }
                (false, None) => return Err(BackupFailure::PassphraseRequired.into()),
                (false, Some(p)) if p.is_empty() => {
                    return Err(EngineError::InvalidArgument(
                        "empty backup passphrase".into(),
                    ));
                }
                _ => {}
            }
            if !matches!(
                this.vault.lock_state(),
                LockState::Unlocked | LockState::Unencrypted | LockState::NoKeys
            ) {
                return Err(BackupFailure::VaultLocked.into());
            }
            let bytes = this
                .backup_bytes(wallet_id, backup_passphrase, false)
                .await?;
            let created_at = unix_now();
            let size_bytes = bytes.len() as u64;
            let path = dest.clone();
            tokio::task::spawn_blocking(move || write_new_private(&path, &bytes))
                .await?
                .map_err(|e| match e {
                    EngineError::Compat(f) => {
                        BackupFailure::DestinationUnwritable(f.to_string()).into()
                    }
                    other => other,
                })?;
            Ok(BackupInfo {
                path: dest,
                wallet_id,
                created_at,
                size_bytes,
                automatic: false,
            })
        })
        .await
    }

    /// Restores the wallets of a `.dwbackup` (QT-110): verifies it, stores
    /// each wallet's secrets under this vault's key (seed-safety order),
    /// registers it and inserts its app metadata. Returns the wallet ids.
    pub async fn restore_backup(
        self: &Arc<Self>,
        path: PathBuf,
        passphrase: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<Vec<WalletId>, EngineError> {
        drop(self.try_enter()?);
        let this = Arc::clone(self);
        self.on_runtime(async move { this.restore_backup_inner(path, passphrase).await })
            .await
    }

    async fn restore_backup_inner(
        self: &Arc<Self>,
        path: PathBuf,
        passphrase: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<Vec<WalletId>, EngineError> {
        let tag = self.network.dir_name();
        let vault = self.vault.clone();
        // Read and open every bundle before anything is stored.
        let opened = self
            .on_runtime(async move {
                tokio::task::spawn_blocking(move || -> Result<Vec<_>, EngineError> {
                    let meta = std::fs::metadata(&path)
                        .map_err(|e| BackupFailure::Corrupt(e.to_string()))?;
                    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
                        return Err(BackupFailure::Corrupt("not a backup file".into()).into());
                    }
                    let bytes = Zeroizing::new(
                        std::fs::read(&path).map_err(|e| BackupFailure::Corrupt(e.to_string()))?,
                    );
                    let (header, header_line) = split_header(&bytes)?;
                    if header.network != tag {
                        return Err(BackupFailure::NetworkMismatch.into());
                    }
                    let body_start =
                        MAGIC.len() + format!("{FORMAT_VERSION}\n").len() + header_line.len() + 1;
                    let body: Body = parse_body(&bytes[body_start..])?;
                    if body.bundles.len() != header.wallet_ids.len() {
                        return Err(BackupFailure::Corrupt("bundle count".into()).into());
                    }
                    let mut out = Vec::new();
                    for (bundle, id_hex) in body.bundles.iter().zip(&header.wallet_ids) {
                        let id: WalletId = id_hex.parse()?;
                        if bundle.wallet_id() != Some(id.0) || bundle.network() != tag {
                            return Err(BackupFailure::Corrupt(
                                "bundle does not match header".into(),
                            )
                            .into());
                        }
                        let (secret, plain) = vault
                            .open_backup_bundle(
                                bundle,
                                passphrase.as_deref().map(|p| &p[..]),
                                header_line,
                            )
                            .map_err(vault_failure)?;
                        let payload: Payload = serde_json::from_slice(&plain)
                            .map_err(|e| BackupFailure::Corrupt(format!("payload: {e}")))?;
                        out.push((id, secret, payload));
                    }
                    Ok(out)
                })
                .await?
            })
            .await?;
        if !matches!(
            self.vault.lock_state(),
            LockState::Unlocked | LockState::Unencrypted | LockState::NoKeys
        ) {
            return Err(BackupFailure::VaultLocked.into());
        }
        let network = self.network.core_network();
        // What this call registered, for the rollback (review L3): the id
        // and whether the wallet was new (not keys attached to a watch-only
        // wallet that was there before).
        let mut done: Vec<(WalletId, bool)> = Vec::new();
        for (id, secret, payload) in opened {
            if let Err(e) = self
                .restore_one(network, id, secret, payload, &mut done)
                .await
            {
                self.roll_back_restore(&done).await;
                return Err(e);
            }
        }
        // Only now, with every bundle restored and its app rows inserted:
        // a failed restore must leave no automatic backup of a wallet it
        // rolled back.
        for &(id, _) in &done {
            self.schedule_automatic_backup(id);
        }
        Ok(done.into_iter().map(|(id, _)| id).collect())
    }

    /// Restores one opened bundle; pushes the wallet to `done` once it is
    /// registered.
    async fn restore_one(
        self: &Arc<Self>,
        network: dashcore::Network,
        id: WalletId,
        secret: dw_vault::WalletSecret,
        payload: Payload,
        done: &mut Vec<(WalletId, bool)>,
    ) -> Result<(), EngineError> {
        let derived = dw_vault::mnemonic::wallet_id_for_seed(&secret.seed, network)
            .map_err(|e| BackupFailure::Corrupt(e.to_string()))?;
        if derived != id.0 {
            return Err(BackupFailure::Corrupt("the seed is not the wallet's".into()).into());
        }
        // Every row value is checked before anything is registered.
        let tables = payload
            .app_rows
            .iter()
            .map(|t| {
                Ok(TableRows {
                    table: t.table.clone(),
                    columns: t.columns.clone(),
                    rows: t
                        .rows
                        .iter()
                        .map(|r| r.iter().map(from_json).collect::<Result<_, _>>())
                        .collect::<Result<_, EngineError>>()?,
                })
            })
            .collect::<Result<Vec<_>, EngineError>>()?;
        let existed = self.manager()?.get_wallet(&id.0).await.is_some();
        let options = ImportOptions {
            birth_height: Some(payload.birth_height.unwrap_or(0)),
            core_compat: false,
            name: payload.name.clone(),
            lookahead: None,
        };
        match self
            .import_secret_inner(options, move || Ok(secret), AutoBackup::Caller)
            .await
        {
            Ok(_) => done.push((id, !existed)),
            Err(EngineError::WalletAlreadyExists(_)) => {
                return Err(BackupFailure::AlreadyExists(id).into());
            }
            Err(EngineError::Vault(e)) => return Err(vault_failure(e)),
            Err(e) => return Err(e),
        }
        let appdb = self.live()?.appdb;
        let wid = id.to_string();
        let inserted =
            tokio::task::spawn_blocking(move || appdb.import_wallet_rows(&wid, &tables)).await??;
        tracing::info!(wallet_id = %id, rows = inserted, "restored app metadata from backup");
        Ok(())
    }

    /// Undoes what a failed restore registered, newest first: a new wallet
    /// is unloaded and its rows and vault records deleted; keys attached
    /// to a watch-only wallet are deleted again. Best effort: a failing
    /// step is logged and the rest still runs.
    async fn roll_back_restore(self: &Arc<Self>, done: &[(WalletId, bool)]) {
        for &(id, created) in done.iter().rev() {
            if created {
                let removed = async {
                    let live = self.live()?;
                    live.manager.remove_wallet(&id.0).await?;
                    self.hub.forget_wallet(&id);
                    let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
                    tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                        persister.delete_wallet(id.0)?;
                        appdb
                            .delete_wallet(&id.to_string())
                            .map_err(|e| EngineError::Storage(e.to_string()))
                    })
                    .await?
                }
                .await;
                if let Err(e) = removed {
                    tracing::warn!(wallet_id = %id, error = %e, "could not roll back a restored wallet");
                }
            }
            self.forget_secret(id).await;
            self.sink.emit(if created {
                EngineEvent::WalletRemoved {
                    network: self.network.clone(),
                    wallet_id: id,
                }
            } else {
                // Watch-only again: hosts reload it.
                EngineEvent::WalletCreated {
                    network: self.network.clone(),
                    wallet_id: id,
                }
            });
            tracing::info!(wallet_id = %id, "rolled back a wallet of a failed restore");
        }
    }

    /// Automatic backups of `wallet` (every wallet when `None`), newest
    /// first (QT-116 "Show Automatic Backups").
    pub async fn automatic_backups(
        self: &Arc<Self>,
        wallet: Option<WalletId>,
    ) -> Result<Vec<BackupInfo>, EngineError> {
        drop(self.try_enter()?);
        let dir = self.backup_directory();
        self.on_runtime(async move {
            tokio::task::spawn_blocking(move || list_automatic(&dir, wallet)).await?
        })
        .await
    }

    pub fn backup_policy(&self) -> Result<BackupPolicy, EngineError> {
        let _op = self.try_enter()?;
        Ok(BackupPolicy {
            keep: self.backup_keep()?,
            directory: self.backup_directory(),
        })
    }

    /// Sets how many automatic backups to keep per wallet (0..=10) and
    /// deletes the oldest beyond the new count.
    pub async fn set_backup_policy(
        self: &Arc<Self>,
        keep: u32,
    ) -> Result<BackupPolicy, EngineError> {
        if keep > MAX_KEEP {
            return Err(EngineError::InvalidArgument(format!(
                "keep {keep} outside 0..={MAX_KEEP}"
            )));
        }
        let this = Arc::clone(self);
        self.on_runtime(async move {
            let _op = this.enter().await?;
            let appdb = this.live()?.appdb;
            let dir = this.backup_directory();
            tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                appdb.set_setting("", KEEP_SETTING, Some(&keep.to_string()))?;
                prune(&dir, None, keep)
            })
            .await??;
            Ok(BackupPolicy {
                keep,
                directory: this.backup_directory(),
            })
        })
        .await
    }

    /// Writes an automatic backup of `id` when the policy keeps any and the
    /// data key is available, then rotates. A failure sends
    /// `Notice{BackupFailed}`; a missing data key skips quietly (dash-qt
    /// backs up when the wallet loads; here the key may not be there yet).
    pub(crate) async fn automatic_backup(self: &Arc<Self>, id: WalletId) {
        let keep = match self.backup_keep() {
            Ok(0) | Err(_) => return,
            Ok(k) => k,
        };
        if !self.vault.has_wallet_secret(&id.0)
            || !matches!(
                self.vault.lock_state(),
                LockState::Unlocked | LockState::Unencrypted
            )
        {
            return;
        }
        let dir = self.backup_directory();
        let path = dir.join(automatic_name(&id, unix_now()));
        if path.exists() {
            return;
        }
        let result = async {
            let bytes = self.backup_bytes(id, None, true).await?;
            let d = dir.clone();
            tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                crate::fsutil::create_private_dir(&d)?;
                write_new_private(&path, &bytes)?;
                prune(&d, Some(id), keep)
            })
            .await?
        }
        .await;
        match result {
            Ok(()) => {}
            Err(EngineError::Backup(BackupFailure::VaultLocked)) => {}
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "automatic backup failed");
                self.sink.emit(EngineEvent::Notice {
                    network: Some(self.network.clone()),
                    code: NoticeCode::BackupFailed,
                    detail: format!("wallet {id}: {e}"),
                });
            }
        }
    }

    /// Starts an automatic backup of `id` in the background.
    pub(crate) fn schedule_automatic_backup(self: &Arc<Self>, id: WalletId) {
        let this = Arc::clone(self);
        self.rt.spawn(async move {
            let Ok(_op) = this.enter().await else { return };
            this.automatic_backup(id).await;
        });
    }
}

fn list_automatic(dir: &Path, wallet: Option<WalletId>) -> Result<Vec<BackupInfo>, EngineError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = automatic_wallet(&name) else {
            continue;
        };
        if wallet.is_some_and(|w| w != id) {
            continue;
        }
        let path = entry.path();
        let Ok(header) = read_header(&path) else {
            continue;
        };
        out.push(BackupInfo {
            size_bytes: entry.metadata().map(|m| m.len()).unwrap_or(0),
            path,
            wallet_id: id,
            created_at: header.created_at,
            automatic: true,
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.path.cmp(&a.path)));
    Ok(out)
}

/// Deletes automatic backups beyond the newest `keep` of each wallet (of
/// `wallet` only, when given).
fn prune(dir: &Path, wallet: Option<WalletId>, keep: u32) -> Result<(), EngineError> {
    let all = list_automatic(dir, wallet)?;
    let mut seen: std::collections::HashMap<WalletId, u32> = std::collections::HashMap::new();
    for b in all {
        let n = seen.entry(b.wallet_id).or_default();
        *n += 1;
        if *n > keep
            && let Err(e) = std::fs::remove_file(&b.path)
        {
            tracing::warn!(path = %b.path.display(), error = %e, "could not delete an old automatic backup");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_names_round_trip() {
        let id = WalletId([0xab; 32]);
        let name = automatic_name(&id, 1_417_713_337);
        assert_eq!(name, format!("{id}.2014-12-04-17-15.dwbackup"));
        assert_eq!(automatic_wallet(&name), Some(id));
        assert_eq!(automatic_wallet("x.2014-12-04-17-15.dwbackup"), None);
        assert_eq!(automatic_wallet(&format!("{id}.dwbackup")), None);
    }

    #[test]
    fn header_parsing_checks_magic_and_version() {
        let h = BackupHeader {
            format: "dwbackup".into(),
            format_version: 1,
            network: "devnet-x".into(),
            created_at: 5,
            wallet_ids: vec![],
            automatic: false,
            app_version: "0".into(),
        };
        let mut bytes = b"DWBACKUP 1\n".to_vec();
        bytes.extend(serde_json::to_vec(&h).unwrap());
        bytes.extend(b"\n{}\n");
        let (parsed, _) = split_header(&bytes).unwrap();
        assert_eq!(parsed, h);
        assert_eq!(
            parsed.network(),
            Some(DashNetwork::Devnet { name: "x".into() })
        );
        assert!(matches!(
            split_header(b"DWBACKUP 2\n{}\n"),
            Err(EngineError::Backup(BackupFailure::UnsupportedVersion(2)))
        ));
        assert!(split_header(b"SQLite format 3").is_err());
    }

    #[test]
    fn json_values_round_trip() {
        for v in [
            SqlValue::Null,
            SqlValue::Integer(-3),
            SqlValue::Real(1.5),
            SqlValue::Text("é".into()),
            SqlValue::Blob(vec![0, 255]),
        ] {
            assert_eq!(from_json(&to_json(&v)).unwrap(), v);
        }
    }

    /// Review M4: a user backup and an automatic backup of the same wallet
    /// at the same time both succeed and touch no shared file; the network
    /// directory gets no temporary copy (it can even be read-only).
    #[test]
    fn user_and_automatic_backups_of_one_wallet_run_together() {
        use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

        struct Quiet;
        impl crate::EventSink for Quiet {
            fn emit(&self, _: EngineEvent) {}
        }
        let dir = tempfile::tempdir().unwrap();
        let engine = crate::Engine::new(
            crate::EngineConfig {
                data_root: dir.path().join("data"),
                worker_threads: Some(4),
                vault: VaultConfig {
                    kdf: KdfPolicy::Fixed(KdfParams::TEST),
                    os_store: Arc::new(MemoryOsStore::new()),
                    ..VaultConfig::default()
                },
            },
            Arc::new(Quiet),
        )
        .unwrap();
        let session = engine
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                crate::SessionOptions {
                    dapi_addresses: vec!["http://127.0.0.1:1".into()],
                    quorum_url: Some("http://127.0.0.1:1".into()),
                    spv_peers: vec!["127.0.0.1:1".into()],
                },
            ))
            .unwrap();
        engine
            .block_on(session.vault_op(|v| v.create(Some(b"pw"))))
            .unwrap();
        let id = engine
            .block_on(
                session.import_wallet(
                    Zeroizing::new(
                        b"abandon abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon about"
                            .to_vec(),
                    ),
                    Zeroizing::new(Vec::new()),
                    ImportOptions::default(),
                ),
            )
            .unwrap();
        let out = dir.path().join("out");
        std::fs::create_dir(&out).unwrap();
        crate::fsutil::create_private_dir(&session.backup_directory()).unwrap();

        // Nothing but the destinations may be written.
        use std::os::unix::fs::PermissionsExt;
        let net = session.data_dir().to_path_buf();
        let mode = std::fs::metadata(&net).unwrap().permissions().mode();
        std::fs::set_permissions(&net, std::fs::Permissions::from_mode(0o500)).unwrap();
        let results: Vec<Result<BackupInfo, EngineError>> = engine.block_on(async {
            let mut tasks = Vec::new();
            for n in 0..6 {
                let s = Arc::clone(&session);
                let dest = out.join(format!("{n}.dwbackup"));
                tasks.push(tokio::spawn(async move {
                    let (user, ()) =
                        tokio::join!(s.backup_wallet(id, dest, None), s.automatic_backup(id));
                    user
                }));
            }
            let mut results = Vec::new();
            for t in tasks {
                results.push(t.await.unwrap());
            }
            results
        });
        std::fs::set_permissions(&net, std::fs::Permissions::from_mode(mode)).unwrap();
        for r in &results {
            let info = r.as_ref().expect("user backup");
            let meta = std::fs::metadata(&info.path).unwrap();
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }
        let stray: Vec<_> = std::fs::read_dir(&net)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".backup-"))
            .collect();
        assert!(stray.is_empty(), "{stray:?}");
        // Each user backup opens in this vault.
        for r in results {
            let path = r.unwrap().path;
            let bytes = std::fs::read(&path).unwrap();
            let (_, header_line) = split_header(&bytes).unwrap();
            let body_start =
                MAGIC.len() + format!("{FORMAT_VERSION}\n").len() + header_line.len() + 1;
            let body: Body = serde_json::from_slice(&bytes[body_start..]).unwrap();
            session
                .vault()
                .open_backup_bundle(&body.bundles[0], None, header_line)
                .unwrap();
        }
    }
}
