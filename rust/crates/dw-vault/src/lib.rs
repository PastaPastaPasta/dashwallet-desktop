//! Secret vault of dashwallet-desktop (DESIGN-opus §1.8, DESIGN.md R2 "PIN").
//!
//! One vault per network, in `<network dir>/vault/`:
//! - a random 256-bit data key (DEK) encrypts every record with
//!   XChaCha20-Poly1305; the AAD binds vault id, network, schema version and
//!   record id;
//! - wrap slots unwrap the DEK: **P** (passphrase → Argon2id, calibrated
//!   ≥ 0.5 s, m ≥ 256 MiB, t ≥ 3), **O** (the DEK itself in the OS secret
//!   store, only while unencrypted), **B** (biometric, TODO(biometric) M2);
//! - lock states `NoVault/NoKeys/Unencrypted/Locked/UnlockedMixingOnly/Unlocked`;
//! - single-use, expiring [`AuthGrant`]s checked here, so a buggy view
//!   cannot sign, reveal or wipe without one;
//! - [`VaultSigner`] implements key_wallet's [`key_wallet::Signer`] over the
//!   stored seed; a mixing-only unlock signs CoinJoin-account paths only.
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
pub mod mnemonic;
pub mod os_store;
mod signer;
mod types;
mod vault;

pub use crypto::{CALIBRATION_TARGET, KdfParams, KdfPolicy};
pub use error::{MnemonicError, SignerError, VaultError};
pub use os_store::{KeyringOsStore, MemoryOsStore, OsSecretStore};
pub use signer::{SignerScope, VaultSigner, WalletSigner, is_coinjoin_path};
pub use types::{
    AuthGrant, Clock, Credential, DEFAULT_GRANT_TTL_SECS, GrantKind, GrantPurpose, GrantToken,
    LockState, RevealedMnemonic, SeedDerivation, SystemClock, UnlockScope, VaultConfig,
    VaultStatus, WalletId, WalletSecret,
};
pub use vault::{MAX_PASSPHRASE_BYTES, Vault, throttle_wait_secs};

/// Directory of the vault inside a network data directory.
pub const VAULT_DIR: &str = "vault";
