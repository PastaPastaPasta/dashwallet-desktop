//! On-disk vault format (`<network dir>/vault/vault.dwv`, JSON).
//!
//! ```text
//! format, vault_id (16 random bytes), network tag, created_at
//! slot_p    passphrase slot: Argon2id params + salt + DEK sealed under the KEK
//!           AAD = "dw-vault/slot-p/v1" ‖ vault_id ‖ network ‖ params ‖ salt
//! slot_o    OS-store slot: where the DEK lives in the OS secret store
//!           (only while the vault is unencrypted)
//! throttle  failed passphrase attempts (UX throttling, IOS-012; not authenticated)
//! records   record_id → XChaCha20-Poly1305(DEK, payload)
//!           AAD = "dw-vault/record/v1" ‖ vault_id ‖ network ‖ schema_ver ‖ record_id
//! manifest  XChaCha20-Poly1305(DEK, {generation, record_id → SHA-256(nonce ‖ ct)})
//!           AAD = "dw-vault/manifest/v1" ‖ vault_id ‖ network
//! ```
//!
//! The manifest makes a deleted, added, swapped or individually rolled-back
//! record detectable; its generation counter (compared with the highest
//! generation this process has seen) detects a whole-file rollback while the
//! app runs. A whole-file rollback across restarts needs a trusted monotonic
//! anchor outside the file and is not detected.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::VaultError;
use crate::crypto::{KdfParams, Sealed, hex_bytes};

pub(crate) const FORMAT: u32 = 1;
/// Record payload schema version, bound into every record's AAD.
pub(crate) const SCHEMA_VER: u16 = 1;
pub(crate) const FILE_NAME: &str = "vault.dwv";
const TMP_NAME: &str = "vault.dwv.tmp";
/// A vault holds a few dozen small records; anything larger is not ours.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VaultFile {
    pub format: u32,
    #[serde(with = "hex_bytes")]
    pub vault_id: Vec<u8>,
    pub network: String,
    pub created_at: u64,
    pub slot_p: Option<PassphraseSlot>,
    pub slot_o: Option<OsSlot>,
    pub throttle: Throttle,
    pub manifest: Sealed,
    pub records: BTreeMap<String, Sealed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PassphraseSlot {
    pub kdf: KdfParams,
    #[serde(with = "hex_bytes")]
    pub salt: Vec<u8>,
    pub wrapped_dek: Sealed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OsSlot {
    #[serde(with = "hex_bytes")]
    pub service: Vec<u8>,
    pub label: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Throttle {
    pub failed_attempts: u32,
    pub last_failure_at: Option<u64>,
}

/// Decrypted manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub generation: u64,
    /// record id → hex SHA-256 of `nonce ‖ ct`.
    pub records: BTreeMap<String, String>,
}

fn push_field(out: &mut Vec<u8>, field: &[u8]) {
    out.extend_from_slice(&(field.len() as u32).to_le_bytes());
    out.extend_from_slice(field);
}

pub(crate) fn slot_p_aad(vault_id: &[u8], network: &str, kdf: &KdfParams, salt: &[u8]) -> Vec<u8> {
    let mut a = b"dw-vault/slot-p/v1".to_vec();
    push_field(&mut a, vault_id);
    push_field(&mut a, network.as_bytes());
    push_field(&mut a, &kdf.aad_bytes());
    push_field(&mut a, salt);
    a
}

pub(crate) fn record_aad(vault_id: &[u8], network: &str, record_id: &str) -> Vec<u8> {
    let mut a = b"dw-vault/record/v1".to_vec();
    push_field(&mut a, vault_id);
    push_field(&mut a, network.as_bytes());
    push_field(&mut a, &SCHEMA_VER.to_le_bytes());
    push_field(&mut a, record_id.as_bytes());
    a
}

pub(crate) fn manifest_aad(vault_id: &[u8], network: &str) -> Vec<u8> {
    let mut a = b"dw-vault/manifest/v1".to_vec();
    push_field(&mut a, vault_id);
    push_field(&mut a, network.as_bytes());
    a
}

/// Manifest digest of one sealed record.
pub(crate) fn record_digest(sealed: &Sealed) -> String {
    let mut h = Sha256::new();
    h.update(&sealed.nonce);
    h.update(&sealed.ct);
    hex::encode(h.finalize())
}

pub(crate) fn file_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Reads and parses the vault file; `Ok(None)` when it does not exist.
pub(crate) fn read(dir: &Path) -> Result<Option<VaultFile>, VaultError> {
    let path = file_path(dir);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if meta.len() > MAX_FILE_BYTES {
        return Err(VaultError::Corrupt(format!(
            "vault file is {} bytes",
            meta.len()
        )));
    }
    let bytes = std::fs::read(&path)?;
    let file: VaultFile = serde_json::from_slice(&bytes)
        .map_err(|e| VaultError::Corrupt(format!("vault file does not parse: {e}")))?;
    if file.format != FORMAT {
        return Err(VaultError::Corrupt(format!(
            "unsupported vault format {}",
            file.format
        )));
    }
    if file.vault_id.len() != 16 {
        return Err(VaultError::Corrupt("vault id length".into()));
    }
    Ok(Some(file))
}

/// Creates `dir` restricted to the current user.
pub(crate) fn create_private_dir(dir: &Path) -> Result<(), VaultError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Writes the vault file atomically: temp file (mode 0600), fsync, rename,
/// fsync of the directory. A crash leaves either the old or the new file.
pub(crate) fn write(dir: &Path, file: &VaultFile) -> Result<(), VaultError> {
    create_private_dir(dir)?;
    let bytes = serde_json::to_vec_pretty(file)
        .map_err(|e| VaultError::Internal(format!("serialize vault: {e}")))?;
    let tmp = dir.join(TMP_NAME);
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, file_path(dir))?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aad_fields_are_length_prefixed() {
        // "ab" + "c" must not collide with "a" + "bc".
        assert_ne!(record_aad(b"ab", "c", "x"), record_aad(b"a", "bc", "x"));
        assert_ne!(manifest_aad(b"ab", "c"), manifest_aad(b"a", "bc"));
    }

    #[test]
    fn read_missing_is_none_and_garbage_is_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(dir.path()).unwrap().is_none());
        std::fs::write(file_path(dir.path()), b"{not json").unwrap();
        assert!(matches!(read(dir.path()), Err(VaultError::Corrupt(_))));
    }
}
