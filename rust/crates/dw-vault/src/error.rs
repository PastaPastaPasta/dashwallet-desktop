/// Vault error. Variants mirror the `vault.*` codes of
/// docs/contracts/m1-engine.md; `Display` is diagnostic text for logs.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VaultError {
    /// No vault file exists for this network.
    #[error("no vault")]
    NoVault,
    /// `create` on a network that already has a vault.
    #[error("vault already exists")]
    AlreadyExists,
    /// The data key is not in memory.
    #[error("vault locked")]
    Locked,
    /// The passphrase did not unwrap the data key.
    #[error("wrong passphrase ({failed_attempts} failed)")]
    WrongPassphrase {
        failed_attempts: u32,
        retry_after_secs: Option<u64>,
    },
    /// Too many failed attempts; the next one is accepted after the wait.
    #[error("throttled for {retry_after_secs}s")]
    Throttled { retry_after_secs: u64 },
    /// The new passphrase is empty or too long.
    #[error("passphrase rejected: {0}")]
    PassphraseRejected(String),
    /// A passphrase operation on a vault that has no passphrase slot.
    #[error("vault is not encrypted")]
    NotEncrypted,
    /// `encrypt` on a vault that already has a passphrase slot.
    #[error("vault is already encrypted")]
    AlreadyEncrypted,
    /// Unknown, expired or already used grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// The grant was issued for another purpose or another wallet.
    #[error("grant purpose mismatch")]
    GrantPurposeMismatch,
    /// The purpose needs the passphrase (or quick unlock) on an encrypted
    /// vault, even while it is unlocked: reveal, wipe, credential change.
    #[error("this purpose needs the passphrase")]
    CredentialRequired,
    /// The vault is unlocked for CoinJoin mixing only.
    #[error("unlocked for mixing only")]
    MixingOnly,
    /// The wallet has no secret in the vault.
    #[error("no secret for wallet")]
    NoSecret,
    /// No slot B on this vault (not enrolled, not encrypted), or the wrap
    /// key does not open it (the OS item is from an older enrolment).
    #[error("quick unlock unavailable")]
    QuickUnlockUnavailable,
    /// A quick-unlock `Spend` grant above the spending limit (IOS-016).
    #[error("quick unlock spending limit {limit_duffs} exceeded")]
    QuickUnlockLimitExceeded { limit_duffs: u64 },
    /// Quick unlock refused: the passphrase was last entered longer ago
    /// than the policy allows (IOS-011).
    #[error("passphrase not entered recently enough for quick unlock")]
    PassphraseStale,
    /// `destroy` while the vault still holds records.
    #[error("vault still holds wallet secrets")]
    NotEmpty,
    /// The recovery phrase does not derive the wallet (IOS-014).
    #[error("recovery phrase does not match the wallet")]
    RecoveryMismatch,
    /// The OS secret store refused the operation or does not exist.
    #[error("OS secret store unavailable: {0}")]
    OsStoreUnavailable(String),
    /// A record, the manifest or the vault file failed authentication or did
    /// not parse, or the vault was rolled back.
    #[error("vault corrupt: {0}")]
    Corrupt(String),
    /// A caller argument is malformed.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Reading or writing the vault file failed.
    #[error("vault storage: {0}")]
    Storage(String),
    /// The call needs a feature that is not built yet.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
    /// A bug (poisoned lock, impossible state).
    #[error("internal: {0}")]
    Internal(String),
}

impl VaultError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::NoVault => "vault.no_vault",
            Self::AlreadyExists => "vault.already_exists",
            Self::Locked => "vault.locked",
            Self::WrongPassphrase { .. } => "vault.wrong_passphrase",
            Self::Throttled { .. } => "vault.throttled",
            Self::PassphraseRejected(_) => "vault.passphrase_rejected",
            Self::NotEncrypted => "vault.not_encrypted",
            Self::AlreadyEncrypted => "vault.already_encrypted",
            Self::GrantInvalid => "vault.grant_invalid",
            Self::GrantPurposeMismatch => "vault.grant_purpose_mismatch",
            Self::CredentialRequired => "vault.credential_required",
            Self::MixingOnly => "vault.mixing_only",
            Self::NoSecret => "vault.no_secret",
            Self::QuickUnlockUnavailable => "vault.quick_unlock_unavailable",
            Self::QuickUnlockLimitExceeded { .. } => "vault.quick_unlock_limit_exceeded",
            Self::PassphraseStale => "vault.passphrase_stale",
            Self::NotEmpty => "vault.not_empty",
            Self::RecoveryMismatch => "vault.recovery_mismatch",
            Self::OsStoreUnavailable(_) => "vault.os_store_unavailable",
            Self::Corrupt(_) => "vault.corrupt",
            Self::InvalidArgument(_) => "invalid_argument",
            Self::Storage(_) => "storage",
            Self::NotImplemented(_) => "not_implemented",
            Self::Internal(_) => "internal",
        }
    }
}

impl From<std::io::Error> for VaultError {
    fn from(e: std::io::Error) -> Self {
        VaultError::Storage(e.to_string())
    }
}

/// Error of [`crate::VaultSigner`]. `Display` never contains key material.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignerError {
    /// The vault was locked (or its unlock scope changed) after the signer
    /// was issued.
    #[error("vault locked")]
    Locked,
    /// A mixing-only signer was asked for a path outside the CoinJoin account.
    #[error("path {0} is outside the CoinJoin account (mixing-only unlock)")]
    PathNotAllowed(String),
    /// The wallet has no seed in the vault.
    #[error("no secret for wallet")]
    NoSecret,
    /// Key derivation failed (invalid path for this key).
    #[error("derivation failed: {0}")]
    Derivation(String),
    /// The vault could not be read.
    #[error("vault: {0}")]
    Vault(VaultError),
}

impl From<VaultError> for SignerError {
    fn from(e: VaultError) -> Self {
        match e {
            VaultError::Locked => SignerError::Locked,
            VaultError::NoSecret => SignerError::NoSecret,
            other => SignerError::Vault(other),
        }
    }
}

/// Error of the mnemonic helpers in [`crate::mnemonic`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MnemonicError {
    /// Word count other than 12, 15, 18, 21 or 24.
    #[error("unsupported word count {0}")]
    UnsupportedWordCount(u32),
    /// Not a phrase this wallet accepts (unknown words, failed checksum,
    /// not UTF-8).
    #[error("invalid mnemonic: {0}")]
    Invalid(String),
    /// The BIP39 passphrase is not UTF-8 (strict BIP39 needs NFKD text).
    #[error("BIP39 passphrase is not UTF-8")]
    PassphraseNotUtf8,
    /// The random source failed.
    #[error("entropy: {0}")]
    Entropy(String),
}
