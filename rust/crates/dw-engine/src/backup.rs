//! Wallet backups (QT-110, QT-116): `.dwbackup` files written on request
//! and rotated automatic backups. Format: docs/contracts/dwbackup-v1.md.
//!
//! ```text
//! DWBACKUP <1 or 2>\n
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

use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;
use dw_appdb::{SqlValue, TableRows};
use dw_vault::{Credential, GrantPurpose, LockState, Vault, VaultError, WalletBackupBundle};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::compat::{TEMP_MARK, is_staging_name, move_new, publish_new_private, sync_dir};
use crate::events::unix_now;
use crate::gate::Lifecycle;
use crate::keys::{AutoBackup, Checked, Origin};
use crate::platform::runtime::PlatformSignal;
use crate::{
    DashNetwork, EngineError, EngineEvent, ImportOptions, NetworkSession, NoticeCode, WalletId,
};

/// First bytes of every `.dwbackup` file.
pub const MAGIC: &[u8] = b"DWBACKUP ";
/// The newest format this engine reads and writes: version 2 is written only
/// for a payload holding a TEXT value that is not UTF-8 (`{"tb": …}`), every
/// other backup as version 1, which older apps read.
pub const FORMAT_VERSION: u32 = 2;
/// The format of a backup whose TEXT values are all UTF-8.
const UTF8_TEXT_VERSION: u32 = 1;
/// Automatic backups kept per wallet by default and at most (dash-qt
/// `-createwalletbackups`).
pub const DEFAULT_KEEP: u32 = 10;
pub const MAX_KEEP: u32 = 10;
/// Directory of automatic backups inside the network directory.
pub const BACKUP_DIR: &str = "backups";
/// File extension.
pub const EXTENSION: &str = "dwbackup";
/// The tag of a user's backup's staging file.
const MANUAL_STAGING_TAG: &str = "dash-wallet-backup";
/// Largest backup file read back.
const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024;
/// dw-appdb setting holding the automatic backup count.
const KEEP_SETTING: &str = "backup.keep";
/// dw-appdb setting, in a wallet's `local:` scope, holding when its latest
/// automatic backup failed (UNIX seconds); absent once one succeeds.
const AUTO_FAILED_SETTING: &str = "backup.auto_failed_at";

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
        SqlValue::NonUtf8Text(t) => {
            json!({ "tb": base64::engine::general_purpose::STANDARD.encode(t) })
        }
        SqlValue::Blob(b) => json!({ "b": base64::engine::general_purpose::STANDARD.encode(b) }),
    }
}

/// Tests: the next stored-wallet delete of a rollback in a session on one
/// of these data directories fails.
#[cfg(test)]
pub(crate) static FAIL_STORED_WALLET_DELETE_IN: std::sync::Mutex<Vec<PathBuf>> =
    std::sync::Mutex::new(Vec::new());

/// The legacy address range a bundle's `settings_kv` rows of `id` carry,
/// checked before anything is registered.
fn bundled_issued(tables: &[TableRows], id: WalletId) -> Result<Option<(u32, u32)>, EngineError> {
    bundled_setting(tables, id, crate::compat::ISSUED_SETTING)
        .map(|v| {
            v.and_then(crate::compat::parse_issued)
                .ok_or_else(|| BackupFailure::Corrupt("legacy address range".into()).into())
        })
        .transpose()
}

/// The lookahead a bundle's `settings_kv` rows of `id` carry, if any;
/// `Corrupt` for one that is present but not a valid lookahead.
fn bundled_lookahead(tables: &[TableRows], id: WalletId) -> Result<Option<u32>, EngineError> {
    bundled_setting(tables, id, crate::keys::LOOKAHEAD_SETTING)
        .map(|v| {
            v.and_then(|v| v.parse::<u32>().ok())
                .filter(|g| (1..=crate::MAX_LOOKAHEAD).contains(g))
                .ok_or_else(|| BackupFailure::Corrupt("lookahead".into()).into())
        })
        .transpose()
}

/// The value of `key` in a bundle's `settings_kv` rows of `id`: `Some(None)`
/// when the row holds no text.
fn bundled_setting<'a>(
    tables: &'a [TableRows],
    id: WalletId,
    key: &str,
) -> Option<Option<&'a str>> {
    let scope = id.to_string();
    for t in tables.iter().filter(|t| t.table == "settings_kv") {
        let col = |name: &str| t.columns.iter().position(|c| c == name);
        let (Some(s), Some(k), Some(v)) = (col("scope"), col("key"), col("value")) else {
            continue;
        };
        for row in &t.rows {
            let text = |i: usize| match row.get(i) {
                Some(SqlValue::Text(t)) => Some(t.as_str()),
                _ => None,
            };
            if text(s) == Some(scope.as_str()) && text(k) == Some(key) {
                return Some(text(v));
            }
        }
    }
    None
}

/// A wallet a restore registered: its id, whether it is new, and the
/// settings of one that was there before, to put back on a rollback.
type Restored = (WalletId, bool, Option<dw_appdb::PriorRows>);

/// The keys of wallet `id`'s settings rows in a bundle.
fn bundled_setting_keys(tables: &[TableRows], id: WalletId) -> Vec<String> {
    let scope = id.to_string();
    let mut keys = Vec::new();
    for t in tables.iter().filter(|t| t.table == "settings_kv") {
        let col = |name: &str| t.columns.iter().position(|c| c == name);
        let (Some(s), Some(k)) = (col("scope"), col("key")) else {
            continue;
        };
        for row in &t.rows {
            if let (Some(SqlValue::Text(sc)), Some(SqlValue::Text(key))) = (row.get(s), row.get(k))
                && *sc == scope
            {
                keys.push(key.clone());
            }
        }
    }
    keys
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
        "tb" => match String::from_utf8(base64_value(val).ok_or_else(bad)?) {
            Ok(t) => SqlValue::Text(t),
            Err(e) => SqlValue::NonUtf8Text(e.into_bytes()),
        },
        "b" => SqlValue::Blob(base64_value(val).ok_or_else(bad)?),
        _ => return Err(bad().into()),
    })
}

fn base64_value(v: &serde_json::Value) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(v.as_str()?)
        .ok()
}

/// The header of a `.dwbackup` file (no decryption).
pub fn read_header(path: &Path) -> Result<BackupHeader, EngineError> {
    let mut start = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(64 * 1024).read_to_end(&mut start))
        .map_err(|e| BackupFailure::Corrupt(e.to_string()))?;
    let (header, _, _) = split_header(&start)?;
    Ok(header)
}

/// Splits `bytes` after the header line: (header, header line bytes, body).
fn split_header(bytes: &[u8]) -> Result<(BackupHeader, &[u8], &[u8]), EngineError> {
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
    if !(UTF8_TEXT_VERSION..=FORMAT_VERSION).contains(&version) {
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
    Ok((header, &rest[..nl], &rest[nl + 1..]))
}

/// The wallet ids of `body`'s bundles, which must be the header's, in its
/// order, and of its network.
fn bundle_ids(header: &BackupHeader, body: &Body) -> Result<Vec<WalletId>, EngineError> {
    if body.bundles.len() != header.wallet_ids.len() {
        return Err(BackupFailure::Corrupt("bundle count".into()).into());
    }
    body.bundles
        .iter()
        .zip(&header.wallet_ids)
        .map(|(bundle, id_hex)| {
            let id: WalletId = id_hex.parse()?;
            if bundle.wallet_id() != Some(id.0) || bundle.network() != header.network {
                return Err(BackupFailure::Corrupt("bundle does not match header".into()).into());
            }
            Ok(id)
        })
        .collect()
}

/// What is at an automatic backup's name.
enum AtName {
    Complete(BackupHeader),
    /// A file that is not a complete automatic backup of the name's wallet.
    Incomplete,
    Missing,
}

/// Checks the file at `path` as an automatic backup of `id` on `network`,
/// without its passphrase and streaming its body. Complete means: the magic
/// line, a header line of the same version naming just `id` and
/// `network`, then a body that is one whole JSON object (no cut of the file
/// leaves one) holding one bundle of that wallet and network that parses
/// as restore parses it (every field restore needs, the sealed ones
/// unopened), the file at most [`MAX_FILE_BYTES`]. Anything but a regular
/// file (a symlink is followed; a dangling one is there, and incomplete)
/// is incomplete and never opened, so a FIFO cannot block it. A read error
/// other than `NotFound` is returned: it says nothing about the file.
fn check_automatic(path: &Path, id: WalletId, network: &str) -> std::io::Result<AtName> {
    #[derive(Deserialize)]
    struct BodyShape {
        bundles: OneBundle,
    }
    struct OneBundle(WalletBackupBundle);
    impl<'de> Deserialize<'de> for OneBundle {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct Visit;
            impl<'de> serde::de::Visitor<'de> for Visit {
                type Value = OneBundle;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("exactly one bundle")
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<OneBundle, A::Error> {
                    use serde::de::Error;
                    let first = seq
                        .next_element::<WalletBackupBundle>()?
                        .ok_or_else(|| A::Error::custom("no bundle"))?;
                    if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                        return Err(A::Error::custom("more than one bundle"));
                    }
                    Ok(OneBundle(first))
                }
            }
            d.deserialize_seq(Visit)
        }
    }
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return match path.symlink_metadata() {
                Ok(_) => Ok(AtName::Incomplete),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AtName::Missing),
                Err(e) => Err(e),
            };
        }
        Err(e) => return Err(e),
    };
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
        return Ok(AtName::Incomplete);
    }
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AtName::Missing),
        Err(e) => return Err(e),
    };
    let mut reader = std::io::BufReader::new(file);
    let mut start = Vec::new();
    for _ in 0..2 {
        (&mut reader)
            .take(64 * 1024)
            .read_until(b'\n', &mut start)?;
        if !start.ends_with(b"\n") {
            return Ok(AtName::Incomplete);
        }
    }
    let header = match split_header(&start) {
        Ok((header, _, _))
            if header.network == network
                && header.wallet_ids.len() == 1
                && header.wallet_ids[0].parse::<WalletId>().ok() == Some(id) =>
        {
            header
        }
        _ => return Ok(AtName::Incomplete),
    };
    let mut body = serde_json::Deserializer::from_reader(reader);
    let shape = match BodyShape::deserialize(&mut body).and_then(|b| body.end().map(|()| b)) {
        Ok(shape) => shape,
        Err(e) if e.is_io() => return Err(e.into()),
        Err(_) => return Ok(AtName::Incomplete),
    };
    let bundle = &shape.bundles.0;
    let complete = dw_vault::reads_bundle_version(u64::from(bundle.version()))
        && bundle.network() == network
        && bundle.wallet_id() == Some(id.0);
    Ok(if complete {
        AtName::Complete(header)
    } else {
        AtName::Incomplete
    })
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

/// Opens one bundle of a restore. A bundle of another vault opens only with
/// its passphrase. One this vault wrote also opens with this vault's key,
/// under a `RevealSecret` grant authorized with the credential a reveal of
/// the phrase takes (review DW-E0-03 r3): none on an unencrypted vault, the
/// vault passphrase on an encrypted one. There the bundle's own slot is
/// used whenever it takes the vault's current passphrase (or is a backup
/// passphrase slot), since it does not count toward the vault's throttle:
/// a mistyped backup passphrase must not lock the vault's unlock out. The
/// passphrase is checked as the vault's (and counted) only for a bundle
/// that just the vault's key opens with it ([`Vault::own_key_only`]: one
/// made before a passphrase change, after its slot was tried, or an
/// automatic backup from before `encrypt`).
fn open_bundle(
    vault: &Vault,
    bundle: &WalletBackupBundle,
    passphrase: Option<&[u8]>,
    header: &[u8],
) -> Result<(dw_vault::WalletSecret, Zeroizing<Vec<u8>>), VaultError> {
    let by_slot = || vault.open_backup_bundle(bundle, passphrase, None, header);
    let wallet = match bundle.wallet_id() {
        Some(w) if vault.wrote_bundle(bundle) => w,
        _ => return by_slot(),
    };
    let credential = if !vault.status().encrypted {
        Credential::None
    } else {
        match (by_slot(), passphrase) {
            (Err(VaultError::WrongPassphrase { .. } | VaultError::CredentialRequired), Some(p))
                if vault.own_key_only(bundle) =>
            {
                Credential::Passphrase(p)
            }
            (opened, _) => return opened,
        }
    };
    let grant = vault.authorize(GrantPurpose::RevealSecret, Some(&wallet), credential)?;
    vault.open_backup_bundle(bundle, None, Some(&grant.id), header)
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
        let mut header = BackupHeader {
            format: "dwbackup".into(),
            format_version: UTF8_TEXT_VERSION,
            network: self.network.dir_name(),
            created_at: unix_now(),
            wallet_ids: vec![id.to_string()],
            automatic,
            app_version: env!("CARGO_PKG_VERSION").into(),
        };
        let vault = self.vault.clone();
        tokio::task::spawn_blocking(move || -> Result<Zeroizing<Vec<u8>>, EngineError> {
            let rows = live.appdb.export_wallet_rows(&id.to_string())?;
            let non_utf8 = rows
                .iter()
                .flat_map(|t| t.rows.iter().flatten())
                .any(|v| matches!(v, SqlValue::NonUtf8Text(_)));
            if non_utf8 {
                header.format_version = FORMAT_VERSION;
            }
            let version = header.format_version;
            let header_line = serde_json::to_vec(&header)
                .map_err(|e| EngineError::Internal(format!("backup header: {e}")))?;
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
            out.extend_from_slice(format!("{version}\n").as_bytes());
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
            tokio::task::spawn_blocking(move || {
                publish_new_private(&path, &bytes, MANUAL_STAGING_TAG)
            })
            .await?
            .map_err(|e| BackupFailure::DestinationUnwritable(e.to_string()))?;
            if this.automatic_backup_failed(wallet_id).await {
                this.automatic_backup(wallet_id).await;
            }
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
    /// Refused (`Storage`) before any bundle is opened while the dispatch
    /// journal is unavailable, and `RestoreOnly` while the network's
    /// RESTORED mark is set.
    pub async fn restore_backup(
        self: &Arc<Self>,
        path: PathBuf,
        passphrase: Option<Zeroizing<Vec<u8>>>,
    ) -> Result<Vec<WalletId>, EngineError> {
        crate::store_upgrade::refuse_while_restored(self.data_dir())?;
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
        self.require_journal()?;
        let tag = self.network.dir_name();
        let vault = self.vault.clone();
        // Refused before any bundle is opened, so no passphrase is checked
        // (or counted by the vault's throttle) for a restore that cannot
        // store; checked again below, after the opening.
        if !matches!(
            self.vault.lock_state(),
            LockState::Unlocked | LockState::Unencrypted | LockState::NoKeys
        ) {
            return Err(BackupFailure::VaultLocked.into());
        }
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
                    let (header, header_line, body) = split_header(&bytes)?;
                    if header.network != tag {
                        return Err(BackupFailure::NetworkMismatch.into());
                    }
                    let body: Body = parse_body(body)?;
                    let ids = bundle_ids(&header, &body)?;
                    let mut out = Vec::new();
                    for (bundle, id) in body.bundles.iter().zip(ids) {
                        let (secret, plain) = open_bundle(
                            &vault,
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
        // Registration and rollback under one admission and the lifecycle
        // lock (DEC-210 (3b)), taken in that order.
        let _op = self.enter().await?;
        let held = self.wallet_lifecycle.hold().await;
        // What this call registered, for the rollback (review L3): the id,
        // whether the wallet was new (not keys attached to a watch-only
        // wallet that was there before) and, for one that was there, its
        // settings as they were before the restore wrote any.
        let mut done: Vec<Restored> = Vec::new();
        // No bring-up is admitted for these wallets, from any entry point,
        // until the restore has committed (review r2 M4). Marked before any
        // seed is stored.
        let restoring = self
            .platform
            .restoring(opened.iter().map(|(id, _, _)| *id).collect());
        for (id, secret, payload) in opened {
            if let Err(e) = self
                .restore_one(&held, network, id, secret, payload, &mut done)
                .await
            {
                self.roll_back_restore(&held, &done).await;
                return Err(e);
            }
            #[cfg(test)]
            crate::coins::pause_at(&self.backups.pause_restore).await;
        }
        // The commit, stamped before the marks clear: a readmission their
        // clearing sends covers it (review r4 M4-R4).
        let committed = self.platform.stamp();
        drop(restoring);
        // Only now, with every bundle restored and its app rows inserted:
        // a failed restore must leave no automatic backup of a wallet it
        // rolled back, and no bring-up holding its keys.
        for &(id, _, _) in &done {
            self.platform
                .signal_at(committed, PlatformSignal::WalletAdded(id));
            // E0-04 §8.6 (review L-4): as `load_wallet`.
            self.leases.wallet_opened(id);
            self.schedule_automatic_backup(id);
        }
        Ok(done.into_iter().map(|(id, _, _)| id).collect())
    }

    /// Restores one opened bundle; pushes the wallet to `done` once it is
    /// registered.
    async fn restore_one(
        self: &Arc<Self>,
        held: &Lifecycle<'_>,
        network: dashcore::Network,
        id: WalletId,
        secret: dw_vault::WalletSecret,
        payload: Payload,
        done: &mut Vec<Restored>,
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
        // A legacy wallet's range and lookahead are applied before it is
        // announced, as its file import does; its rows are stored with them,
        // before its seed and its registration (#359).
        // Listed as of its restore, as when its rows landed after the
        // registration wrote its name.
        let mut tables = tables;
        let now = SqlValue::Integer(unix_now() as i64);
        for t in tables.iter_mut().filter(|t| t.table == "wallets") {
            if let Some(i) = t.columns.iter().position(|c| c == "created_at") {
                for row in &mut t.rows {
                    if let Some(v) = row.get_mut(i) {
                        *v = now.clone();
                    }
                }
            }
        }
        let issued = bundled_issued(&tables, id)?;
        let lookahead = bundled_lookahead(&tables, id)?;
        let existed = self.manager()?.get_wallet(&id.0).await.is_some();
        // A wallet already there gets back, if the restore fails later, the
        // settings it writes: its range and the bundle's rows (its salt).
        let prior = if existed {
            let mut keys = bundled_setting_keys(&tables, id);
            if !keys.iter().any(|k| k == crate::compat::ISSUED_SETTING) {
                keys.push(crate::compat::ISSUED_SETTING.to_owned());
            }
            let appdb = self.live()?.appdb;
            let wid = id.to_string();
            Some(tokio::task::spawn_blocking(move || appdb.prior_settings(&wid, &keys)).await??)
        } else {
            None
        };
        let state = crate::compat::ImportedState {
            issued,
            rows: Some(tables),
            ..Default::default()
        };
        let options = ImportOptions {
            birth_height: Some(payload.birth_height.unwrap_or(0)),
            core_compat: false,
            name: payload.name.clone(),
            // A wallet there before gets it with its rows, as on main: an
            // attach would rewind it before the restore commits.
            lookahead: lookahead.filter(|_| !existed),
        };
        match self
            .import_secret_locked(
                held,
                Checked::new(options)?,
                secret,
                AutoBackup::Caller,
                Origin::Imported,
                state,
            )
            .await
        {
            Ok(_) => done.push((id, !existed, prior)),
            Err(EngineError::WalletAlreadyExists(_)) => {
                return Err(BackupFailure::AlreadyExists(id).into());
            }
            Err(EngineError::Vault(e)) => return Err(vault_failure(e)),
            Err(e) => return Err(e),
        }
        tracing::info!(wallet_id = %id, "restored app metadata from backup");
        // The bell's count, which the imported rows moved (DP2-05).
        let tap = self.live()?.tap;
        tokio::task::spawn_blocking(move || tap.unread_moved(id)).await?;
        Ok(())
    }

    /// Takes a wallet a failed call registered out of the manager and
    /// deletes its stored wallet and rows, recording it as removed (DEC-210).
    /// The caller has ended its discoveries and forgotten it in the platform
    /// runtime.
    pub(crate) async fn remove_registered(
        self: &Arc<Self>,
        id: WalletId,
    ) -> Result<(), EngineError> {
        let live = self.live()?;
        // A wallet holding a stored shielded spend stays (DEC-1046), as does
        // one with a marked spend (DEC-1057).
        let removal = (self.platform)
            .remove_from_manager(&self.leases, &live.manager, id)
            .await;
        // Again, now that it is out of the manager (or failed to leave it):
        // a scan begun since the first forget passed the wallet check, and a
        // names check begun since read the wallet (D33).
        self.platform.users.scans.forget(&id);
        self.platform.recovery.fence_names(&id);
        removal?;
        self.hub.forget_wallet(&id);
        self.coin_fences.forget(&id);
        let (persister, appdb) = (Arc::clone(&live.persister), Arc::clone(&live.appdb));
        #[cfg(test)]
        let refused =
            crate::keys::take_test_failure(&FAIL_STORED_WALLET_DELETE_IN, self.data_dir());
        tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
            // Both are attempted: rows left behind would outlive a failed
            // delete of the stored wallet with no removal record.
            #[cfg(test)]
            let stored = if refused {
                Err(EngineError::Storage(
                    "test: stored wallet not deleted".into(),
                ))
            } else {
                persister.delete_wallet(id.0).map_err(EngineError::from)
            };
            #[cfg(not(test))]
            let stored = persister.delete_wallet(id.0).map_err(EngineError::from);
            let rows = appdb
                .delete_wallet(&id.to_string())
                .map_err(|e| EngineError::Storage(e.to_string()));
            stored.and(rows)
        })
        .await?
    }

    /// Undoes what a failed restore registered, newest first: a new wallet
    /// is unloaded and its rows and vault records deleted; keys attached
    /// to a watch-only wallet are deleted again and its settings put back.
    /// Best effort: a failing step is logged and the rest still runs.
    pub(crate) async fn roll_back_restore(
        self: &Arc<Self>,
        held: &Lifecycle<'_>,
        done: &[Restored],
    ) {
        for (id, created, prior) in done.iter().rev() {
            let (id, created) = (*id, *created);
            // Still marked as being restored, so no bring-up was admitted for
            // it; a status read since is stale. Its discoveries end, and none
            // starts until the rollback of it is done: the wallet out of the
            // manager, its secret gone (review DP1-05 r2 M3-R2).
            let _discoveries = self.platform.recovery.end_discoveries(id).await;
            self.platform.forget(&id);
            if created {
                let removed = self.remove_registered(id).await;
                if let Err(EngineError::ShieldedSpendUnresolved) = removed {
                    // It stays whole, its seed with it: without the seed it
                    // would never hold a background lease again, and every
                    // shielded pass of the network would be held back.
                    tracing::warn!(wallet_id = %id, "a restored wallet holding a shielded spend stays");
                    continue;
                }
                if removed.is_ok() {
                    self.list_on_startup(held, id, false).await;
                }
                if let Err(e) = removed {
                    tracing::warn!(wallet_id = %id, error = %e, "could not roll back a restored wallet");
                }
            }
            // The tap's caches of it (the bell's count, news not sent),
            // after its rows went.
            if let Ok(live) = self.live() {
                live.tap.forget(&id);
            }
            self.forget_secret(held, id).await;
            if let Some(prior) = prior {
                self.restore_imported_state(prior.clone()).await;
            }
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
        let (dir, network) = (self.backup_directory(), self.network.dir_name());
        self.on_runtime(async move {
            tokio::task::spawn_blocking(move || list_automatic(&dir, &network, wallet)).await?
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
            let _policy = this.backups.hold_policy().await;
            let appdb = this.live()?.appdb;
            let (dir, network) = (this.backup_directory(), this.network.dir_name());
            tokio::task::spawn_blocking(move || -> Result<(), EngineError> {
                let value = keep.to_string();
                if keep == 0 {
                    appdb.set_setting_clearing_local(
                        "",
                        KEEP_SETTING,
                        Some(&value),
                        AUTO_FAILED_SETTING,
                    )?;
                } else {
                    appdb.set_setting("", KEEP_SETTING, Some(&value))?;
                }
                prune(&dir, &network, None, keep, None)
            })
            .await??;
            Ok(BackupPolicy {
                keep,
                directory: this.backup_directory(),
            })
        })
        .await
    }

    /// Wallets whose latest automatic backup failed, and when (UNIX
    /// seconds). A wallet's entry goes when one succeeds, when automatic
    /// backups are turned off and with the wallet. A backup still being
    /// written is not listed, though its failure is recorded before it
    /// writes (a stop mid-write leaves the record).
    pub async fn automatic_backup_failures(
        self: &Arc<Self>,
    ) -> Result<Vec<(WalletId, u64)>, EngineError> {
        drop(self.try_enter()?);
        let appdb = self.live()?.appdb;
        let this = Arc::clone(self);
        self.on_runtime(async move {
            tokio::task::spawn_blocking(move || {
                let in_flight = this.backups.in_flight();
                let rows = appdb.local_settings_of_key(AUTO_FAILED_SETTING)?;
                Ok(rows
                    .into_iter()
                    .filter_map(|(id, at)| Some((id.parse().ok()?, at.parse().ok()?)))
                    .filter(|(id, _)| !in_flight.contains(id))
                    .collect())
            })
            .await?
        })
        .await
    }

    /// Whether the latest automatic backup of `id` failed, or its record
    /// cannot be read; not while one is being written.
    async fn automatic_backup_failed(self: &Arc<Self>, id: WalletId) -> bool {
        self.automatic_failure(id).await.is_some()
    }

    /// The failure record of `id`, with its time when it can be read; none
    /// while a backup of `id` is being written.
    async fn automatic_failure(self: &Arc<Self>, id: WalletId) -> Option<Option<u64>> {
        let Ok(live) = self.live() else { return None };
        let this = Arc::clone(self);
        let scope = dw_appdb::local_scope(&id.to_string());
        let recorded = tokio::task::spawn_blocking(move || {
            let in_flight = this.backups.in_flight();
            if in_flight.contains(&id) {
                return Ok(None);
            }
            live.appdb.setting(&scope, AUTO_FAILED_SETTING)
        })
        .await;
        match recorded {
            Ok(Ok(None)) => None,
            Ok(Ok(Some(at))) => Some(at.parse().ok()),
            _ => Some(None),
        }
    }

    /// Writes an automatic backup of `id` when the policy keeps any and the
    /// data key is available, then rotates, and sends `Backups`. A failure
    /// is recorded ([`Self::automatic_backup_failures`]) and sends
    /// `Notice{BackupFailed}`; a missing data key skips quietly (dash-qt
    /// backs up when the wallet loads; here the key may not be there yet),
    /// and so does a backup of the same minute that is already written.
    /// With `keep` 0 it only deletes the wallet's record. Runs under the
    /// wallet lifecycle lock (a wallet removed or a restore rolled back is
    /// not backed up) and the policy lock (`set_backup_policy` waits for
    /// it). The caller must not hold the lifecycle lock.
    pub(crate) async fn automatic_backup(self: &Arc<Self>, id: WalletId) -> AutomaticRun {
        let _lifecycle = self.wallet_lifecycle.hold().await;
        let _policy = self.backups.hold_policy().await;
        if self.require_wallet(&id).is_err() || !self.vault.has_wallet_secret(&id.0) {
            return AutomaticRun::Done;
        }
        let keep = match self.backup_keep() {
            Ok(0) => {
                if self.automatic_backup_failed(id).await {
                    self.record_automatic_outcome(id, None).await;
                    self.emit_backups();
                }
                return AutomaticRun::Done;
            }
            Err(_) => return AutomaticRun::Done,
            Ok(k) => k,
        };
        if !matches!(
            self.vault.lock_state(),
            LockState::Unlocked | LockState::Unencrypted
        ) {
            return AutomaticRun::Locked;
        }
        let dir = self.backup_directory();
        let network = self.network.dir_name();
        let now = unix_now();
        let path = dir.join(automatic_name(&id, now));
        let (p, n) = (path.clone(), network.clone());
        if let Ok(Ok(AtName::Complete(header))) =
            tokio::task::spawn_blocking(move || check_automatic(&p, id, &n)).await
        {
            let this_minute = header.created_at / 60 == now / 60;
            let clears = match self.automatic_failure(id).await {
                None => false,
                Some(Some(failed_at)) => this_minute || header.created_at >= failed_at,
                Some(None) => this_minute,
            };
            if clears {
                let (p, d) = (path.clone(), dir.clone());
                let durable = tokio::task::spawn_blocking(move || {
                    #[cfg(unix)]
                    std::fs::File::open(&p)?.sync_all()?;
                    #[cfg(not(unix))]
                    let _ = p;
                    sync_dir(&d)
                })
                .await
                .map_err(std::io::Error::other)
                .and_then(|r| r);
                match durable {
                    Ok(()) => {
                        self.record_automatic_outcome(id, None).await;
                        self.emit_backups();
                    }
                    Err(e) => {
                        tracing::warn!(wallet_id = %id, error = %e, "an automatic backup is not durable yet");
                    }
                }
            }
            return AutomaticRun::Done;
        }
        let flight = match self.start_automatic(id).await {
            Ok(flight) => flight,
            Err(e) => {
                tracing::warn!(wallet_id = %id, error = %e, "automatic backup not started");
                self.sink.emit(EngineEvent::Notice {
                    network: Some(self.network.clone()),
                    code: NoticeCode::BackupFailed,
                    detail: format!("wallet {id}: {e}"),
                });
                return AutomaticRun::Done;
            }
        };
        #[cfg(test)]
        crate::coins::pause_at(&self.backups.pause).await;
        let result = async {
            let bytes = self.backup_bytes(id, None, true).await?;
            let (data_dir, d) = (self.data_dir().to_path_buf(), dir.clone());
            tokio::task::spawn_blocking(move || -> Result<bool, EngineError> {
                crate::fsutil::create_owned_dir(&data_dir, Path::new(BACKUP_DIR))?;
                if !publish_automatic(&path, &bytes, id, &network)
                    .map_err(|e| BackupFailure::DestinationUnwritable(e.to_string()))?
                {
                    return Ok(false);
                }
                prune(&d, &network, Some(id), keep, Some(&path))?;
                Ok(true)
            })
            .await?
        }
        .await;
        match result {
            Ok(false) => {
                let cleared = matches!(flight.previous, Some(Some(_)));
                self.end_automatic(flight, Some(None)).await;
                if cleared {
                    self.emit_backups();
                }
                AutomaticRun::Done
            }
            Ok(true) => {
                self.end_automatic(flight, Some(None)).await;
                self.emit_backups();
                AutomaticRun::Done
            }
            Err(EngineError::Backup(BackupFailure::VaultLocked)) => {
                self.end_automatic(flight, None).await;
                AutomaticRun::Locked
            }
            Err(e) => {
                self.end_automatic(flight, Some(Some(unix_now().to_string())))
                    .await;
                tracing::warn!(wallet_id = %id, error = %e, "automatic backup failed");
                self.sink.emit(EngineEvent::Notice {
                    network: Some(self.network.clone()),
                    code: NoticeCode::BackupFailed,
                    detail: format!("wallet {id}: {e}"),
                });
                AutomaticRun::Done
            }
        }
    }

    /// Marks the automatic backup of `id` in flight and records it as
    /// failed now, durably, so a stop before it ends (a power loss too)
    /// leaves the record. A record that cannot be written fails it: the
    /// backup does not start.
    async fn start_automatic(self: &Arc<Self>, id: WalletId) -> Result<InFlight, EngineError> {
        self.backups.in_flight().insert(id);
        let mut flight = InFlight {
            session: Arc::clone(self),
            id,
            previous: None,
        };
        let this = Arc::clone(self);
        flight.previous = tokio::task::spawn_blocking(move || -> Result<_, EngineError> {
            let appdb = this.live()?.appdb;
            let scope = dw_appdb::local_scope(&id.to_string());
            let previous = appdb.setting(&scope, AUTO_FAILED_SETTING).ok();
            let now = unix_now().to_string();
            appdb.set_setting_durable(&scope, AUTO_FAILED_SETTING, Some(&now))?;
            Ok(previous)
        })
        .await??;
        Ok(flight)
    }

    /// Sends `Backups`: the backups or their failure records changed.
    fn emit_backups(&self) {
        self.sink.emit(EngineEvent::Backups {
            network: self.network.clone(),
        });
    }

    /// Ends `flight`, recording `record` (`Some(None)`: no failure), or the
    /// record it found (`None`: skipped, the failure it recorded undone).
    async fn end_automatic(self: &Arc<Self>, flight: InFlight, record: Option<Option<String>>) {
        let id = flight.id;
        let Some(record) = record.or_else(|| flight.previous.clone()) else {
            return;
        };
        let this = Arc::clone(self);
        let recorded = tokio::task::spawn_blocking(move || {
            let mut in_flight = this.backups.in_flight();
            let scope = dw_appdb::local_scope(&id.to_string());
            let recorded = this.live().is_ok_and(|live| {
                live.appdb
                    .set_setting(&scope, AUTO_FAILED_SETTING, record.as_deref())
                    .is_ok()
            });
            in_flight.remove(&id);
            recorded
        })
        .await;
        drop(flight);
        if !matches!(recorded, Ok(true)) {
            tracing::warn!(wallet_id = %id, "could not record the automatic backup's outcome");
        }
    }

    /// Records when the latest automatic backup of `id` failed, or deletes
    /// the record for `None`.
    async fn record_automatic_outcome(&self, id: WalletId, failed_at: Option<String>) {
        let Ok(live) = self.live() else { return };
        let scope = dw_appdb::local_scope(&id.to_string());
        let recorded = tokio::task::spawn_blocking(move || {
            live.appdb
                .set_setting(&scope, AUTO_FAILED_SETTING, failed_at.as_deref())
        })
        .await;
        if !matches!(recorded, Ok(Ok(()))) {
            tracing::warn!(wallet_id = %id, "could not record the automatic backup's outcome");
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

    /// Starts an automatic backup of every wallet in the background, once
    /// per session: at open (dash-qt backs a wallet up when it loads,
    /// QT-116), or while an encrypted vault is locked then, at its first
    /// unlock. A wallet that finds the vault locked again ends the run, and
    /// the next unlock starts it over (at once, if one came while the run
    /// was ending).
    pub(crate) fn back_up_every_wallet_once(self: &Arc<Self>) {
        if !matches!(
            self.vault.lock_state(),
            LockState::Unlocked | LockState::Unencrypted
        ) || self.backups.started.swap(true, Ordering::SeqCst)
        {
            return;
        }
        #[cfg(test)]
        self.backups.runs.fetch_add(1, Ordering::SeqCst);
        let this = Arc::clone(self);
        self.rt.spawn(async move {
            let Ok(manager) = this.manager() else {
                this.backups.started.store(false, Ordering::SeqCst);
                return;
            };
            for id in manager.list_wallet_ids_blocking() {
                let Ok(_op) = this.enter().await else { return };
                if this.automatic_backup(WalletId(id)).await == AutomaticRun::Locked {
                    #[cfg(test)]
                    crate::coins::pause_at(&this.backups.pause_locked).await;
                    this.backups.started.store(false, Ordering::SeqCst);
                    this.back_up_every_wallet_once();
                    break;
                }
            }
            #[cfg(test)]
            this.backups.runs_done.fetch_add(1, Ordering::SeqCst);
        });
    }

    /// After a call that changed the vault's credential (encrypt, recover,
    /// passphrase change): the backups made this session are sealed under
    /// the credential it replaced, so every wallet is backed up again, now
    /// or at the next unlock.
    pub(crate) fn back_up_after_credential_change(self: &Arc<Self>) {
        self.backups.started.store(false, Ordering::SeqCst);
        self.back_up_every_wallet_once();
    }
}

/// An automatic backup being written; dropped, it is no longer.
struct InFlight {
    session: Arc<NetworkSession>,
    id: WalletId,
    /// The failure record before it started (`None`: unreadable).
    previous: Option<Option<String>>,
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.session.backups.in_flight().remove(&self.id);
    }
}

/// Publishes the automatic backup `bytes` of `id` at `path`; `false` when a
/// complete backup is there. A file there that is not one is set aside
/// first, and kept; one that cannot be read fails the backup.
fn publish_automatic(
    path: &Path,
    bytes: &[u8],
    id: WalletId,
    network: &str,
) -> std::io::Result<bool> {
    let mut set_aside_once = false;
    for _ in 0..3 {
        match publish_new_private(path, bytes, &id.to_string()) {
            Ok(()) => return Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match check_automatic(path, id, network)? {
                    AtName::Complete(_) => return Ok(false),
                    AtName::Missing => {}
                    AtName::Incomplete if set_aside_once => return Err(e),
                    AtName::Incomplete => {
                        set_aside(path)?;
                        set_aside_once = true;
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

/// Renames the file at `path` to `<name>.incomplete` (or
/// `<name>.incomplete-<n>`), which nothing lists or deletes.
fn set_aside(path: &Path) -> std::io::Result<()> {
    let name = path.file_name().unwrap_or_default();
    for n in 1..=100 {
        let mut aside = name.to_os_string();
        aside.push(INCOMPLETE);
        if n > 1 {
            aside.push(format!("-{n}"));
        }
        match move_new(path, &path.with_file_name(aside)) {
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            other => return other,
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

/// The suffix of an automatic backup file set aside as incomplete.
const INCOMPLETE: &str = ".incomplete";

/// What [`NetworkSession::automatic_backup`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomaticRun {
    /// Written, failed, or skipped for a reason an unlock does not change.
    Done,
    /// Skipped: the data key is not available.
    Locked,
}

/// The session's automatic backup state.
#[derive(Default)]
pub(crate) struct BackupRuntime {
    /// [`NetworkSession::back_up_every_wallet_once`] ran.
    started: AtomicBool,
    /// Orders `set_backup_policy` with each automatic backup.
    policy: tokio::sync::Mutex<()>,
    /// Wallets whose automatic backup is being written.
    in_flight: std::sync::Mutex<std::collections::HashSet<WalletId>>,
    /// Tests: how often it ran, and how often its run ended.
    #[cfg(test)]
    pub(crate) runs: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    pub(crate) runs_done: std::sync::atomic::AtomicUsize,
    /// Tests: holds the next automatic backup after its same-minute check.
    #[cfg(test)]
    pub(crate) pause: std::sync::Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next restore after each wallet it registered.
    #[cfg(test)]
    pub(crate) pause_restore: std::sync::Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: holds the next run that found the vault locked, before it
    /// lets the next unlock start one.
    #[cfg(test)]
    pub(crate) pause_locked: std::sync::Mutex<Option<Arc<crate::coins::TestPause>>>,
    /// Tests: how many wait for `policy`.
    #[cfg(test)]
    policy_waiting: std::sync::atomic::AtomicUsize,
}

impl BackupRuntime {
    async fn hold_policy(&self) -> tokio::sync::MutexGuard<'_, ()> {
        #[cfg(test)]
        self.policy_waiting.fetch_add(1, Ordering::SeqCst);
        let guard = self.policy.lock().await;
        #[cfg(test)]
        self.policy_waiting.fetch_sub(1, Ordering::SeqCst);
        guard
    }

    /// Tests: how many wait for the policy lock.
    #[cfg(all(test, unix))]
    pub(crate) fn policy_waiting(&self) -> usize {
        self.policy_waiting.load(Ordering::SeqCst)
    }
}

impl BackupRuntime {
    fn in_flight(&self) -> std::sync::MutexGuard<'_, std::collections::HashSet<WalletId>> {
        self.in_flight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The complete automatic backups of `network` in `dir` (of `wallet` only,
/// when given), newest first. A file that is not a complete backup of the
/// wallet its name names is skipped.
fn list_automatic(
    dir: &Path,
    network: &str,
    wallet: Option<WalletId>,
) -> Result<Vec<BackupInfo>, EngineError> {
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
        let Ok(AtName::Complete(header)) = check_automatic(&path, id, network) else {
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

/// Deletes complete automatic backups beyond the newest `keep` of each
/// wallet (of `wallet` only, when given), then [`sweep_temporaries`]. Files
/// that are not complete backups are neither counted nor deleted. The
/// backup `written` (just written) ranks first; one whose header time is
/// later than now (written while the clock was ahead) ranks after every
/// other.
fn prune(
    dir: &Path,
    network: &str,
    wallet: Option<WalletId>,
    keep: u32,
    written: Option<&Path>,
) -> Result<(), EngineError> {
    let now = unix_now();
    let mut all = list_automatic(dir, network, wallet)?;
    let rank = |b: &BackupInfo| (Some(b.path.as_path()) != written, b.created_at > now);
    all.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(b.created_at.cmp(&a.created_at))
            .then(b.path.cmp(&a.path))
    });
    let mut seen: std::collections::HashMap<WalletId, u32> = std::collections::HashMap::new();
    let mut newest: std::collections::HashMap<WalletId, u64> = std::collections::HashMap::new();
    for b in all {
        let n = seen.entry(b.wallet_id).or_default();
        *n += 1;
        if *n <= keep {
            let at = newest.entry(b.wallet_id).or_default();
            *at = (*at).max(b.created_at);
        } else if let Err(e) = std::fs::remove_file(&b.path) {
            tracing::warn!(path = %b.path.display(), error = %e, "could not delete an old automatic backup");
        }
    }
    sweep_temporaries(dir, wallet, &newest);
    Ok(())
}

/// How old a temporary file of an automatic backup must be to be swept.
const TEMP_SWEEP_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Deletes the temporary files of automatic backups that are
/// [`TEMP_SWEEP_AGE`] old when a complete backup of their wallet (`newest`,
/// UNIX seconds) is newer. Every other is kept.
fn sweep_temporaries(
    dir: &Path,
    wallet: Option<WalletId>,
    newest: &std::collections::HashMap<WalletId, u64>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(id) = name
            .strip_prefix('.')
            .and_then(|n| n.split_once(TEMP_MARK))
            .and_then(|(tag, _)| tag.parse::<WalletId>().ok())
            .filter(|id| is_staging_name(&name, &id.to_string()))
        else {
            continue;
        };
        if wallet.is_some_and(|w| w != id) {
            continue;
        }
        let Ok(written) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let old = now
            .duration_since(written)
            .is_ok_and(|age| age > TEMP_SWEEP_AGE);
        let written = written
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(u64::MAX, |d| d.as_secs());
        if old
            && newest.get(&id).is_some_and(|&at| at > written)
            && let Err(e) = std::fs::remove_file(entry.path())
        {
            tracing::warn!(file = %name, error = %e, "could not delete a stale temporary backup file");
        }
    }
}

#[cfg(test)]
#[path = "backup_tests.rs"]
mod backup_tests;

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
        let (parsed, _, _) = split_header(&bytes).unwrap();
        assert_eq!(parsed, h);
        assert_eq!(
            parsed.network(),
            Some(DashNetwork::Devnet { name: "x".into() })
        );
        assert!(matches!(
            split_header(b"DWBACKUP 3\n{}\n"),
            Err(EngineError::Backup(BackupFailure::UnsupportedVersion(3)))
        ));
        assert!(split_header(b"SQLite format 3").is_err());

        // Version 2, and the body after its header line.
        let v2 = BackupHeader {
            format_version: 2,
            ..h.clone()
        };
        let mut bytes = b"DWBACKUP 2\n".to_vec();
        bytes.extend(serde_json::to_vec(&v2).unwrap());
        bytes.extend(b"\n{\"bundles\":[]}\n");
        let (parsed, line, body) = split_header(&bytes).unwrap();
        assert_eq!(parsed, v2);
        assert_eq!(line, serde_json::to_vec(&v2).unwrap());
        assert_eq!(body, b"{\"bundles\":[]}\n");
        let mut mismatched = b"DWBACKUP 2\n".to_vec();
        mismatched.extend(serde_json::to_vec(&h).unwrap());
        mismatched.extend(b"\n{}\n");
        assert!(matches!(
            split_header(&mismatched),
            Err(EngineError::Backup(BackupFailure::Corrupt(_)))
        ));
    }

    /// A version 2 header without its wallet's bundle is not a backup: it is
    /// neither listed nor rotated.
    #[test]
    fn a_version_2_header_without_its_bundle_is_not_a_backup() {
        let dir = dw_testutil::private_tempdir();
        let id = WalletId([0xab; 32]);
        let header = BackupHeader {
            format: "dwbackup".into(),
            format_version: 2,
            network: "regtest".into(),
            created_at: 9,
            wallet_ids: vec![id.to_string()],
            automatic: true,
            app_version: "0".into(),
        };
        let mut bytes = b"DWBACKUP 2\n".to_vec();
        bytes.extend(serde_json::to_vec(&header).unwrap());
        bytes.extend(b"\n{}\n");
        let path = dir.path().join(automatic_name(&id, 9));
        std::fs::write(&path, bytes).unwrap();
        assert!(
            list_automatic(dir.path(), "regtest", Some(id))
                .unwrap()
                .is_empty()
        );
        prune(dir.path(), "regtest", Some(id), 0, None).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn json_values_round_trip() {
        for v in [
            SqlValue::Null,
            SqlValue::Integer(-3),
            SqlValue::Real(1.5),
            SqlValue::Text("é".into()),
            SqlValue::NonUtf8Text(b"ab\xff".to_vec()),
            SqlValue::Blob(vec![0, 255]),
        ] {
            assert_eq!(from_json(&to_json(&v)).unwrap(), v);
        }
        assert_eq!(
            to_json(&SqlValue::NonUtf8Text(b"ab\xff".to_vec())),
            serde_json::json!({ "tb": "YWL/" })
        );
        assert_eq!(
            from_json(&serde_json::json!({ "tb": "w6k=" })).unwrap(),
            SqlValue::Text("é".into())
        );
        assert!(from_json(&serde_json::json!({ "tb": "not base64!" })).is_err());
    }

    /// Review M4: a user backup and an automatic backup of the same wallet
    /// at the same time both succeed and touch no shared file; the network
    /// directory gets no temporary copy (it can even be read-only).
    // Unix modes: the network directory is made read-only and backups must be 0600.
    #[cfg(unix)]
    #[test]
    fn user_and_automatic_backups_of_one_wallet_run_together() {
        use dw_vault::{KdfParams, KdfPolicy, MemoryOsStore, VaultConfig};

        struct Quiet;
        impl crate::EventSink for Quiet {
            fn emit(&self, _: EngineEvent) {}
        }
        let dir = dw_testutil::private_tempdir();
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
                    ..Default::default()
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
                    let (user, _) =
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
        // Each user backup opens in this vault, with the vault passphrase.
        for r in results {
            let path = r.unwrap().path;
            let bytes = std::fs::read(&path).unwrap();
            let (_, header_line, body) = split_header(&bytes).unwrap();
            let body: Body = serde_json::from_slice(body).unwrap();
            open_bundle(session.vault(), &body.bundles[0], Some(b"pw"), header_line).unwrap();
        }
    }

    /// Review DW-E0-03 r3: a restore opens a bundle of this vault with its
    /// key only under a `RevealSecret` grant, so on an encrypted vault it
    /// takes the passphrase; an unencrypted vault's needs none, as a reveal
    /// of its phrase needs none. The vault's throttle counts a wrong
    /// passphrase only for a bundle just the vault's key opens.
    #[test]
    fn an_own_bundle_opens_with_the_credential_a_reveal_takes() {
        use dw_vault::{
            KdfParams, KdfPolicy, MemoryOsStore, SeedDerivation, UnlockScope, VaultConfig,
        };

        let w = [7u8; 32];
        let vault = |dir: &Path, passphrase: Option<&[u8]>| {
            let config = VaultConfig {
                kdf: KdfPolicy::Fixed(KdfParams::TEST),
                os_store: Arc::new(MemoryOsStore::new()),
                ..VaultConfig::default()
            };
            let v = Vault::open(dir, dashcore::Network::Regtest, "regtest", config).unwrap();
            v.create(passphrase).unwrap();
            let secret = dw_vault::WalletSecret {
                mnemonic: Zeroizing::new(b"phrase".to_vec()),
                mnemonic_passphrase: Zeroizing::new(Vec::new()),
                seed: Zeroizing::new([9; 64]),
                derivation: SeedDerivation::Bip39,
            };
            v.store_wallet_secret(&w, &secret).unwrap();
            v
        };
        let dir = dw_testutil::private_tempdir();

        let v = vault(&dir.path().join("enc"), Some(b"old pw"));
        let bundle = v.backup_bundle(&w, None, b"p", b"h").unwrap();
        assert_eq!(
            open_bundle(&v, &bundle, None, b"h").unwrap_err(),
            VaultError::NotEncrypted
        );
        open_bundle(&v, &bundle, Some(b"old pw"), b"h").unwrap();
        v.change_passphrase(b"old pw", b"new pw").unwrap();
        // The slot still takes the old passphrase; the new one opens it
        // through the vault's key, under a grant.
        open_bundle(&v, &bundle, Some(b"old pw"), b"h").unwrap();
        let (secret, payload) = open_bundle(&v, &bundle, Some(b"new pw"), b"h").unwrap();
        assert_eq!(
            (&secret.mnemonic[..], &payload[..]),
            (&b"phrase"[..], &b"p"[..])
        );
        assert_eq!(v.status().failed_attempts, 0);
        // A bundle whose slot takes the current passphrase never checks it
        // as the vault's: a typo is not counted by the vault's throttle.
        let current = v.backup_bundle(&w, None, b"c", b"h").unwrap();
        assert!(!v.own_key_only(&current) && v.own_key_only(&bundle));
        for _ in 0..3 {
            assert!(matches!(
                open_bundle(&v, &current, Some(b"wrong"), b"h"),
                Err(VaultError::WrongPassphrase { .. })
            ));
        }
        assert_eq!(v.status().failed_attempts, 0);
        open_bundle(&v, &current, Some(b"new pw"), b"h").unwrap();
        // One made under the old passphrase: only the vault's key opens it
        // with the new one, so a wrong one is checked (and counted) there.
        assert!(matches!(
            open_bundle(&v, &bundle, Some(b"wrong"), b"h"),
            Err(VaultError::WrongPassphrase { .. })
        ));
        assert_eq!(v.status().failed_attempts, 1);

        let v = vault(&dir.path().join("plain"), None);
        let automatic = v.backup_bundle(&w, None, b"a", b"h").unwrap();
        let with_pw = v.backup_bundle(&w, Some(b"backup pw"), b"b", b"h").unwrap();
        assert!(!automatic.has_passphrase_slot());
        assert_eq!(
            v.open_backup_bundle(&automatic, None, None, b"h")
                .unwrap_err(),
            VaultError::NotEncrypted
        );
        open_bundle(&v, &automatic, None, b"h").unwrap();

        // After `encrypt`: the automatic backup opens with the vault
        // passphrase through the vault's key; a mistyped backup passphrase
        // stays the bundle's business.
        let grant = v
            .authorize(GrantPurpose::ChangeCredential, None, Credential::None)
            .unwrap();
        v.encrypt(b"vault pw", &grant.id).unwrap();
        v.unlock(b"vault pw", UnlockScope::Full).unwrap();
        assert!(v.own_key_only(&automatic) && !v.own_key_only(&with_pw));
        assert_eq!(
            open_bundle(&v, &automatic, None, b"h").unwrap_err(),
            VaultError::NotEncrypted
        );
        open_bundle(&v, &automatic, Some(b"vault pw"), b"h").unwrap();
        assert!(matches!(
            open_bundle(&v, &with_pw, Some(b"typo"), b"h"),
            Err(VaultError::WrongPassphrase { .. })
        ));
        assert_eq!(v.status().failed_attempts, 0);
        open_bundle(&v, &with_pw, Some(b"backup pw"), b"h").unwrap();
    }
}
