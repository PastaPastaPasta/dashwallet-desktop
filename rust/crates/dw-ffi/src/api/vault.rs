//! Vault: secrets, lock state and authorization grants (DESIGN-opus §1.8).
//! Owner: workstream B (dw-vault). Contract: docs/contracts/m1-engine.md §vault.
//!
//! Secret handling across the FFI:
//! - Passphrases and quick-unlock keys come in as `Vec<u8>`; the engine wraps
//!   them in `Zeroizing` on entry and never stores them unwrapped.
//! - Revealed material goes out only through `Vault::reveal_mnemonic`, as
//!   bytes the host copies into `DashKit.SecretBytes` immediately.
//! - Residual risk: UniFFI frees the `RustBuffer` that carries returned bytes
//!   without zeroing it. B decides whether to add a zeroing transfer type.

use std::sync::Arc;

use crate::NetworkSession;
use crate::api::common::{domain_error_common, not_implemented};

/// Lock state of a network's vault (QT-022, QT-111/112, IOS-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum VaultLockState {
    /// No vault has been created on this network yet (first run).
    NoVault,
    /// The vault exists but holds no key material: every wallet is watch-only.
    NoKeys,
    /// Secrets are wrapped by the OS secret store only (slot O); no passphrase.
    Unencrypted,
    /// Passphrase-protected and the data key is not in memory.
    Locked,
    /// Data key in memory, but signing is limited to CoinJoin mixing (QT-112).
    UnlockedMixingOnly,
    /// Data key in memory; every grant purpose is available.
    Unlocked,
}

/// What `Vault::unlock` makes available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum UnlockScope {
    Full,
    /// dash-qt "Unlock for mixing only" (QT-112).
    MixingOnly,
}

/// Snapshot of the vault. Pure in-memory read.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VaultStatus {
    pub state: VaultLockState,
    /// A passphrase slot exists (dash-qt "encrypted wallet").
    pub encrypted: bool,
    /// A quick-unlock (biometric) slot is enrolled.
    pub quick_unlock_enrolled: bool,
    /// Consecutive failed passphrase attempts (IOS-012 throttling).
    pub failed_attempts: u32,
    /// When throttled: seconds until the next attempt is accepted.
    pub retry_after_secs: Option<u64>,
    /// Wallets whose mnemonic (or other key material) is stored in the vault.
    pub wallets_with_secrets: Vec<String>,
}

/// The credential presented to `Vault::authorize`.
#[derive(Clone, uniffi::Enum)]
pub enum VaultCredential {
    /// The vault passphrase, UTF-8 bytes.
    Passphrase { passphrase: Vec<u8> },
    /// Key released by the OS biometric store for slot B (M2).
    QuickUnlock { wrap_key: Vec<u8> },
    /// No credential: valid only for an unencrypted vault when the
    /// "require authentication for every payment" setting is off.
    Unencrypted,
}

impl std::fmt::Debug for VaultCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            VaultCredential::Passphrase { .. } => "Passphrase(<redacted>)",
            VaultCredential::QuickUnlock { .. } => "QuickUnlock(<redacted>)",
            VaultCredential::Unencrypted => "Unencrypted",
        })
    }
}

/// What a grant authorizes (DESIGN-opus §1.8 "Auth grants").
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum GrantPurpose {
    /// `TxDraft::prepare`; the prepared transaction may debit at most `max_duffs`.
    Spend { max_duffs: u64 },
    /// `Vault::reveal_mnemonic`.
    RevealSecret,
    /// `NetworkSession::sign_message`.
    SignMessage,
    /// `Vault::change_passphrase`, `Vault::encrypt`, quick-unlock enrolment.
    ChangeCredential,
    /// `NetworkSession::remove_wallet`.
    Wipe,
    /// ProTx operations (M3).
    MasternodeOp,
    /// Governance votes and proposals (M3).
    Governance,
    /// Identity, DPNS, DashPay, credits, shielded (M4).
    PlatformOp,
}

/// A single authorization. Grants live in engine memory only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AuthGrant {
    /// Opaque id passed to the call the grant authorizes.
    pub id: String,
    pub purpose: GrantPurpose,
    /// UNIX seconds after which the grant is refused.
    pub expires_at: u64,
    /// The grant is consumed by its first successful use.
    pub single_use: bool,
}

/// A revealed recovery phrase (QT-113, IOS-006). Both fields are secret.
#[derive(Clone, uniffi::Record)]
pub struct RevealedMnemonic {
    /// Space-separated phrase, UTF-8.
    pub phrase: Vec<u8>,
    /// BIP39 passphrase ("25th word"), UTF-8; empty when none (DESIGN R1).
    pub bip39_passphrase: Vec<u8>,
}

impl std::fmt::Debug for RevealedMnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RevealedMnemonic(<redacted>)")
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum VaultError {
    /// Code `vault.no_vault`: the call needs a vault and none exists.
    #[error("no vault")]
    NoVault,
    /// Code `vault.already_exists`: `create` on a network that has a vault.
    #[error("vault already exists")]
    AlreadyExists,
    /// Code `vault.locked`: the data key is not in memory.
    #[error("vault locked")]
    Locked,
    /// Code `vault.wrong_passphrase`.
    #[error("wrong passphrase ({failed_attempts} failed)")]
    WrongPassphrase {
        failed_attempts: u32,
        retry_after_secs: Option<u64>,
    },
    /// Code `vault.throttled`: too many failures; retry later (IOS-012).
    #[error("throttled for {retry_after_secs}s")]
    Throttled { retry_after_secs: u64 },
    /// Code `vault.passphrase_rejected`: empty, too long or not UTF-8.
    #[error("passphrase rejected: {detail}")]
    PassphraseRejected { detail: String },
    /// Code `vault.not_encrypted`: a passphrase operation on an unencrypted vault.
    #[error("vault is not encrypted")]
    NotEncrypted,
    /// Code `vault.already_encrypted`: `encrypt` on an encrypted vault.
    #[error("vault is already encrypted")]
    AlreadyEncrypted,
    /// Code `vault.grant_invalid`: unknown, expired or already used grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `vault.grant_purpose_mismatch`: the grant does not cover this call.
    #[error("grant purpose mismatch")]
    GrantPurposeMismatch,
    /// Code `vault.mixing_only`: the vault is unlocked for mixing only.
    #[error("unlocked for mixing only")]
    MixingOnly,
    /// Code `vault.no_secret`: the wallet has no secret in the vault (watch-only).
    #[error("no secret for wallet")]
    NoSecret,
    /// Code `vault.quick_unlock_unavailable`: no biometric slot on this OS/vault.
    #[error("quick unlock unavailable")]
    QuickUnlockUnavailable,
    /// Code `vault.os_store_unavailable`: the OS secret store refused or is missing.
    #[error("OS secret store unavailable: {detail}")]
    OsStoreUnavailable { detail: String },
    /// Code `vault.corrupt`: a record failed authentication or did not parse.
    #[error("vault corrupt: {detail}")]
    Corrupt { detail: String },
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

domain_error_common!(VaultError);

impl VaultError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoVault => "vault.no_vault",
            Self::AlreadyExists => "vault.already_exists",
            Self::Locked => "vault.locked",
            Self::WrongPassphrase { .. } => "vault.wrong_passphrase",
            Self::Throttled { .. } => "vault.throttled",
            Self::PassphraseRejected { .. } => "vault.passphrase_rejected",
            Self::NotEncrypted => "vault.not_encrypted",
            Self::AlreadyEncrypted => "vault.already_encrypted",
            Self::GrantInvalid => "vault.grant_invalid",
            Self::GrantPurposeMismatch => "vault.grant_purpose_mismatch",
            Self::MixingOnly => "vault.mixing_only",
            Self::NoSecret => "vault.no_secret",
            Self::QuickUnlockUnavailable => "vault.quick_unlock_unavailable",
            Self::OsStoreUnavailable { .. } => "vault.os_store_unavailable",
            Self::Corrupt { .. } => "vault.corrupt",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

/// The vault of one network. Obtained from `NetworkSession::vault`.
#[derive(uniffi::Object)]
pub struct Vault {
    session: Arc<dw_engine::NetworkSession>,
}

#[uniffi::export]
impl NetworkSession {
    /// The vault for this session's network. Cheap; returns a new handle to
    /// the same engine state each time.
    pub fn vault(&self) -> Arc<Vault> {
        Arc::new(Vault {
            session: Arc::clone(&self.inner),
        })
    }
}

#[uniffi::export]
impl Vault {
    /// Current state. In-memory read; never blocks.
    pub fn status(&self) -> Result<VaultStatus, VaultError> {
        if !self.session.is_open() {
            return Err(VaultError::NetworkNotOpen {
                detail: self.session.network().to_string(),
            });
        }
        not_implemented("Vault.status")
    }

    /// Creates the vault. `passphrase` `Some` = encrypted (slot P, Argon2id);
    /// `None` = unencrypted (slot O in the OS secret store). Leaves the vault
    /// unlocked. Errors: `AlreadyExists`, `PassphraseRejected`, `OsStoreUnavailable`.
    pub async fn create(&self, passphrase: Option<Vec<u8>>) -> Result<VaultStatus, VaultError> {
        drop(passphrase.map(zeroize::Zeroizing::new));
        not_implemented("Vault.create")
    }

    /// dash-qt "Encrypt Wallet" (QT-111): adds slot P, deletes slot O. Needs
    /// a `ChangeCredential` grant. Errors: `AlreadyEncrypted`, `GrantInvalid`.
    pub async fn encrypt(
        &self,
        new_passphrase: Vec<u8>,
        grant_id: String,
    ) -> Result<VaultStatus, VaultError> {
        drop(zeroize::Zeroizing::new(new_passphrase));
        let _ = grant_id;
        not_implemented("Vault.encrypt")
    }

    /// Unwraps the data key. Errors: `WrongPassphrase`, `Throttled`, `NotEncrypted`.
    pub async fn unlock(
        &self,
        passphrase: Vec<u8>,
        scope: UnlockScope,
    ) -> Result<VaultStatus, VaultError> {
        drop(zeroize::Zeroizing::new(passphrase));
        let _ = scope;
        not_implemented("Vault.unlock")
    }

    /// Drops the data key from memory and revokes every grant. Idempotent.
    pub fn lock(&self) -> Result<VaultStatus, VaultError> {
        not_implemented("Vault.lock")
    }

    /// Re-wraps the data key under a new passphrase; the seed is unchanged
    /// (QT-111). Errors: `WrongPassphrase`, `Throttled`, `PassphraseRejected`.
    pub async fn change_passphrase(
        &self,
        old_passphrase: Vec<u8>,
        new_passphrase: Vec<u8>,
    ) -> Result<VaultStatus, VaultError> {
        drop(zeroize::Zeroizing::new(old_passphrase));
        drop(zeroize::Zeroizing::new(new_passphrase));
        not_implemented("Vault.change_passphrase")
    }

    /// Checks `credential` and issues a grant for `purpose`. A passphrase
    /// credential also unlocks a locked vault (scope Full).
    pub async fn authorize(
        &self,
        purpose: GrantPurpose,
        credential: VaultCredential,
    ) -> Result<AuthGrant, VaultError> {
        let _ = purpose;
        drop(credential);
        not_implemented("Vault.authorize")
    }

    /// Invalidates a grant before it expires. Unknown ids are ignored.
    pub fn revoke_grant(&self, grant_id: String) -> Result<(), VaultError> {
        let _ = grant_id;
        not_implemented("Vault.revoke_grant")
    }

    /// The recovery phrase and BIP39 passphrase of `wallet_id`. Needs a
    /// `RevealSecret` grant. Errors: `NoSecret`, `GrantInvalid`, `Locked`.
    pub async fn reveal_mnemonic(
        &self,
        wallet_id: String,
        grant_id: String,
    ) -> Result<RevealedMnemonic, VaultError> {
        let _ = (wallet_id, grant_id);
        not_implemented("Vault.reveal_mnemonic")
    }

    /// Enrols the biometric slot (M2): returns the wrap key the host stores in
    /// the OS biometric store. Needs a `ChangeCredential` grant.
    pub async fn enroll_quick_unlock(&self, grant_id: String) -> Result<Vec<u8>, VaultError> {
        let _ = grant_id;
        not_implemented("Vault.enroll_quick_unlock")
    }

    /// Deletes the biometric slot (M2). Idempotent.
    pub async fn remove_quick_unlock(&self) -> Result<VaultStatus, VaultError> {
        not_implemented("Vault.remove_quick_unlock")
    }
}
