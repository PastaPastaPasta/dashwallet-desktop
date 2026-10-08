//! Secret vault of dashwallet-desktop (DESIGN-opus §1.8, DESIGN.md R2 "PIN").
//!
//! One vault per network, in `<network dir>/vault/`:
//! - a random 256-bit data key (DEK) encrypts every record with
//!   XChaCha20-Poly1305; the AAD binds vault id, network, schema version and
//!   record id;
//! - wrap slots unwrap the DEK: **P** (passphrase → Argon2id, calibrated
//!   ≥ 0.5 s, m ≥ 256 MiB, t ≥ 3), **O** (the DEK itself in the OS secret
//!   store, only while unencrypted), **B** (quick unlock: the DEK under a
//!   random 256-bit key the OS keeps behind biometrics; spend-limited);
//! - lock states `NoVault/NoKeys/Unencrypted/Locked/UnlockedMixingOnly/Unlocked`;
//! - single-use, expiring [`AuthGrant`]s checked here, so a buggy view
//!   cannot sign, reveal or wipe without one;
//! - [`VaultSigner`] implements key_wallet's [`key_wallet::Signer`] over the
//!   stored seed; a mixing-only unlock signs CoinJoin-account paths only;
//! - Platform signers are scoped ([`SignerScope`], DASHPAY §3.3): identity
//!   keys, DashPay contact crypto (which never signs) and asset-lock
//!   funding, each refusing every path outside its own ([`paths`]); the
//!   identity signatures and DIP-15 crypto run here so derived scalars stay
//!   in this crate.
//!
//! dash-qt parity: "Encrypt wallet" adds slot P and removes slot O, "Change
//! passphrase" re-wraps the DEK (the seed is unchanged), and there is no
//! decrypt back to unencrypted.
//!
//! Residual risks: derived secp256k1 keys are erased with
//! `non_secure_erase` (the compiler may elide it); key-wallet's
//! `Mnemonic::phrase` and `to_seed` return unzeroized temporaries that are
//! copied into `Zeroizing` buffers at once; the DEK is not mlocked.

mod crypto;
mod error;
mod file;
#[cfg(test)]
mod lock_race_tests;
pub mod mnemonic;
pub mod os_store;
pub mod paths;
mod platform;
mod signer;
mod types;
mod vault;

pub use crypto::{CALIBRATION_TARGET, KdfParams, KdfPolicy};
pub use error::{MnemonicError, SignerError, VaultError};
pub use os_store::{KeyringOsStore, MemoryOsStore, OsSecretStore};
pub use paths::{is_bip44_path, is_coinjoin_path};
pub use platform::{ContactInfoOpened, ContactInfoSealed, ScanKey};
pub use signer::{SignerScope, VaultSigner, WalletSigner};
pub use types::{
    AuthGrant, Clock, Credential, DEFAULT_GRANT_TTL_SECS, DEFAULT_QUICK_UNLOCK_SPEND_LIMIT,
    GrantKind, GrantPurpose, GrantToken, LockState, PASSPHRASE_MAX_AGE_SECS,
    QUICK_UNLOCK_SPEND_LIMITS, QuickUnlockPolicy, RevealedMnemonic, SeedDerivation, SystemClock,
    UnlockScope, VaultConfig, VaultStatus, WalletId, WalletSecret,
};
pub use vault::{
    CoreMnemonicCheck, MAX_PASSPHRASE_BYTES, Vault, WalletBackupBundle, reads_bundle_version,
    throttle_wait_secs,
};

/// Directory of the vault inside a network data directory.
pub const VAULT_DIR: &str = "vault";

/// What a vault directory holds, read without opening the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VaultDirInfo {
    /// A vault file exists (readable or not).
    pub has_vault: bool,
    /// The vault file names an OS-store slot and the OS store holds its
    /// key. `false` when the store cannot be reached.
    pub has_os_store_key: bool,
}

/// Inspects `dir` (a network's `vault/` directory) for IOS-009's
/// existing-wallet detection. Reads the file and queries the OS store only.
pub fn inspect_vault_dir(dir: &std::path::Path, os_store: &dyn OsSecretStore) -> VaultDirInfo {
    let has_vault = file::file_path(dir).is_file();
    let slot = match file::read(dir) {
        Ok(Some(f)) => f.slot_o,
        _ => None,
    };
    let has_os_store_key = slot.is_some_and(|o| {
        <[u8; 32]>::try_from(o.service.as_slice()).is_ok_and(|service| {
            match os_store.get(&service, &o.label) {
                Ok(v) => v.is_some(),
                Err(e) => {
                    tracing::warn!(error = %e, "OS store unavailable while inspecting a vault");
                    false
                }
            }
        })
    });
    VaultDirInfo {
        has_vault,
        has_os_store_key,
    }
}
