//! Vault: secrets, lock state and authorization grants (DESIGN-opus §1.8).
//! Owner: workstream B (dw-vault). Contract: docs/contracts/m1-engine.md §vault.
//!
//! Secret handling across the FFI:
//! - Passphrases and quick-unlock keys come in as `Vec<u8>`; the engine wraps
//!   them in `Zeroizing` on entry and never stores them unwrapped.
//! - Revealed material goes out only through `Vault::reveal_mnemonic`, as
//!   bytes the host copies into `DashKit.SecretBytes` immediately.
//! - Residual risk: UniFFI frees the `RustBuffer` that carries returned bytes
//!   without zeroing it, and lifts inbound secrets out of an unzeroed
//!   `RustBuffer`. Only the copies Rust owns are zeroed.
//!
//! Every call delegates to the session's dw-vault vault in dw-engine. Calls
//! that run Argon2id or write the vault file go through
//! `NetworkSession::vault_op` (blocking pool, `LockState` event on change).

use std::sync::Arc;

use dw_vault::Credential;
use zeroize::Zeroizing;

use crate::NetworkSession;
use crate::api::common::{domain_error_common, parse_wallet_id};

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

/// The credential presented to `Vault::authorize`. Not `Clone`: the bytes are
/// moved into `Zeroizing` buffers on entry.
#[derive(uniffi::Enum)]
pub enum VaultCredential {
    /// The vault passphrase, UTF-8 bytes.
    Passphrase { passphrase: Vec<u8> },
    /// Key released by the OS biometric store for slot B (M2).
    QuickUnlock { wrap_key: Vec<u8> },
    /// No credential: accepted on an unencrypted vault, and on a vault
    /// unlocked with scope Full except for `RevealSecret`, `Wipe` and
    /// `ChangeCredential` (those need the passphrase whenever the vault is
    /// encrypted). Whether the host sends it for spending and signing is the
    /// "require authentication for every payment" setting.
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
#[derive(uniffi::Record)]
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
    /// Code `vault.grant_purpose_mismatch`: the grant does not cover this
    /// call (another purpose, or bound to another wallet).
    #[error("grant purpose mismatch")]
    GrantPurposeMismatch,
    /// Code `vault.credential_required`: reveal, wipe and credential change
    /// need the passphrase on an encrypted vault, even while it is unlocked.
    #[error("this purpose needs the passphrase")]
    CredentialRequired,
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

domain_error_common!(@not_implemented VaultError);

impl From<dw_vault::VaultError> for VaultError {
    fn from(e: dw_vault::VaultError) -> Self {
        use dw_vault::VaultError as V;
        match e {
            V::NoVault => Self::NoVault,
            V::AlreadyExists => Self::AlreadyExists,
            V::Locked => Self::Locked,
            V::WrongPassphrase {
                failed_attempts,
                retry_after_secs,
            } => Self::WrongPassphrase {
                failed_attempts,
                retry_after_secs,
            },
            V::Throttled { retry_after_secs } => Self::Throttled { retry_after_secs },
            V::PassphraseRejected(detail) => Self::PassphraseRejected { detail },
            V::NotEncrypted => Self::NotEncrypted,
            V::AlreadyEncrypted => Self::AlreadyEncrypted,
            V::GrantInvalid => Self::GrantInvalid,
            V::GrantPurposeMismatch => Self::GrantPurposeMismatch,
            V::CredentialRequired => Self::CredentialRequired,
            V::MixingOnly => Self::MixingOnly,
            V::NoSecret => Self::NoSecret,
            V::QuickUnlockUnavailable => Self::QuickUnlockUnavailable,
            V::OsStoreUnavailable(detail) => Self::OsStoreUnavailable { detail },
            V::Corrupt(detail) => Self::Corrupt { detail },
            V::InvalidArgument(detail) => Self::InvalidArgument { detail },
            V::Storage(detail) => Self::Storage { detail },
            V::NotImplemented(call) => Self::NotImplemented {
                call: call.to_string(),
            },
            V::Internal(detail) => Self::Internal { detail },
        }
    }
}

impl From<dw_engine::EngineError> for VaultError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::Vault(v) => v.into(),
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(_) => Self::NotImplemented { call: detail },
            _ => Self::Internal { detail },
        }
    }
}

impl From<dw_vault::LockState> for VaultLockState {
    fn from(s: dw_vault::LockState) -> Self {
        use dw_vault::LockState as L;
        match s {
            L::NoVault => Self::NoVault,
            L::NoKeys => Self::NoKeys,
            L::Unencrypted => Self::Unencrypted,
            L::Locked => Self::Locked,
            L::UnlockedMixingOnly => Self::UnlockedMixingOnly,
            L::Unlocked => Self::Unlocked,
        }
    }
}

impl From<dw_vault::VaultStatus> for VaultStatus {
    fn from(s: dw_vault::VaultStatus) -> Self {
        Self {
            state: s.state.into(),
            encrypted: s.encrypted,
            quick_unlock_enrolled: s.quick_unlock_enrolled,
            failed_attempts: s.failed_attempts,
            retry_after_secs: s.retry_after_secs,
            wallets_with_secrets: s.wallets_with_secrets.iter().map(hex::encode).collect(),
        }
    }
}

impl From<UnlockScope> for dw_vault::UnlockScope {
    fn from(s: UnlockScope) -> Self {
        match s {
            UnlockScope::Full => Self::Full,
            UnlockScope::MixingOnly => Self::MixingOnly,
        }
    }
}

impl From<GrantPurpose> for dw_vault::GrantPurpose {
    fn from(p: GrantPurpose) -> Self {
        match p {
            GrantPurpose::Spend { max_duffs } => Self::Spend { max_duffs },
            GrantPurpose::RevealSecret => Self::RevealSecret,
            GrantPurpose::SignMessage => Self::SignMessage,
            GrantPurpose::ChangeCredential => Self::ChangeCredential,
            GrantPurpose::Wipe => Self::Wipe,
            GrantPurpose::MasternodeOp => Self::MasternodeOp,
            GrantPurpose::Governance => Self::Governance,
            GrantPurpose::PlatformOp => Self::PlatformOp,
        }
    }
}

impl From<dw_vault::GrantPurpose> for GrantPurpose {
    fn from(p: dw_vault::GrantPurpose) -> Self {
        use dw_vault::GrantPurpose as P;
        match p {
            P::Spend { max_duffs } => Self::Spend { max_duffs },
            P::RevealSecret => Self::RevealSecret,
            P::SignMessage => Self::SignMessage,
            P::ChangeCredential => Self::ChangeCredential,
            P::Wipe => Self::Wipe,
            P::MasternodeOp => Self::MasternodeOp,
            P::Governance => Self::Governance,
            P::PlatformOp => Self::PlatformOp,
        }
    }
}

impl From<dw_vault::AuthGrant> for AuthGrant {
    fn from(g: dw_vault::AuthGrant) -> Self {
        Self {
            id: g.id,
            purpose: g.purpose.into(),
            expires_at: g.expires_at,
            single_use: g.single_use,
        }
    }
}

/// The credential with its bytes moved into a zeroing buffer.
enum OwnedCredential {
    Passphrase(Zeroizing<Vec<u8>>),
    QuickUnlock(Zeroizing<Vec<u8>>),
    None,
}

impl From<VaultCredential> for OwnedCredential {
    fn from(c: VaultCredential) -> Self {
        match c {
            VaultCredential::Passphrase { passphrase } => {
                Self::Passphrase(Zeroizing::new(passphrase))
            }
            VaultCredential::QuickUnlock { wrap_key } => {
                Self::QuickUnlock(Zeroizing::new(wrap_key))
            }
            VaultCredential::Unencrypted => Self::None,
        }
    }
}

impl OwnedCredential {
    fn as_credential(&self) -> Credential<'_> {
        match self {
            Self::Passphrase(p) => Credential::Passphrase(p),
            Self::QuickUnlock(k) => Credential::QuickUnlock(k),
            Self::None => Credential::None,
        }
    }
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
            Self::PassphraseRejected { .. } => "vault.passphrase_rejected",
            Self::NotEncrypted => "vault.not_encrypted",
            Self::AlreadyEncrypted => "vault.already_encrypted",
            Self::GrantInvalid => "vault.grant_invalid",
            Self::GrantPurposeMismatch => "vault.grant_purpose_mismatch",
            Self::CredentialRequired => "vault.credential_required",
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

impl Vault {
    fn check_open(&self) -> Result<(), VaultError> {
        if self.session.is_open() {
            Ok(())
        } else {
            Err(VaultError::NetworkNotOpen {
                detail: self.session.network().to_string(),
            })
        }
    }

    /// Runs `f` on the engine's blocking pool via `NetworkSession::vault_op`.
    async fn op<T, F>(&self, f: F) -> Result<T, VaultError>
    where
        F: FnOnce(&dw_vault::Vault) -> Result<T, dw_vault::VaultError> + Send + 'static,
        T: Send + 'static,
    {
        Ok(self.session.vault_op(f).await?)
    }
}

#[uniffi::export]
impl Vault {
    /// Current state. In-memory read; never blocks.
    pub fn status(&self) -> Result<VaultStatus, VaultError> {
        self.check_open()?;
        Ok(self.session.vault().status().into())
    }

    /// Creates the vault. `passphrase` `Some` = encrypted (slot P, Argon2id);
    /// `None` = unencrypted (slot O in the OS secret store). Leaves the vault
    /// unlocked. Errors: `AlreadyExists`, `PassphraseRejected`, `OsStoreUnavailable`.
    pub async fn create(&self, passphrase: Option<Vec<u8>>) -> Result<VaultStatus, VaultError> {
        let passphrase = passphrase.map(Zeroizing::new);
        self.op(move |v| v.create(passphrase.as_ref().map(|p| &p[..])))
            .await
            .map(Into::into)
    }

    /// dash-qt "Encrypt Wallet" (QT-111): adds slot P, deletes slot O. Needs
    /// a `ChangeCredential` grant. Leaves the vault locked. Errors:
    /// `AlreadyEncrypted`, `GrantInvalid`.
    pub async fn encrypt(
        &self,
        new_passphrase: Vec<u8>,
        grant_id: String,
    ) -> Result<VaultStatus, VaultError> {
        let new_passphrase = Zeroizing::new(new_passphrase);
        self.op(move |v| v.encrypt(&new_passphrase, &grant_id))
            .await
            .map(Into::into)
    }

    /// Unwraps the data key. Errors: `WrongPassphrase`, `Throttled`, `NotEncrypted`.
    pub async fn unlock(
        &self,
        passphrase: Vec<u8>,
        scope: UnlockScope,
    ) -> Result<VaultStatus, VaultError> {
        let passphrase = Zeroizing::new(passphrase);
        self.op(move |v| v.unlock(&passphrase, scope.into()))
            .await
            .map(Into::into)
    }

    /// Drops the data key from memory and revokes every grant. Idempotent.
    pub fn lock(&self) -> Result<VaultStatus, VaultError> {
        Ok(self.session.lock_vault()?.into())
    }

    /// Re-wraps the data key under a new passphrase; the seed is unchanged
    /// (QT-111). Errors: `WrongPassphrase`, `Throttled`, `PassphraseRejected`.
    pub async fn change_passphrase(
        &self,
        old_passphrase: Vec<u8>,
        new_passphrase: Vec<u8>,
    ) -> Result<VaultStatus, VaultError> {
        let old_passphrase = Zeroizing::new(old_passphrase);
        let new_passphrase = Zeroizing::new(new_passphrase);
        self.op(move |v| v.change_passphrase(&old_passphrase, &new_passphrase))
            .await
            .map(Into::into)
    }

    /// Checks `credential` and issues a single-use grant for `purpose`.
    ///
    /// `wallet_id` binds the grant to the wallet it is for: required for
    /// every purpose except `ChangeCredential`, which must pass `None`
    /// (`invalid_argument` otherwise). The grant is refused on any other
    /// wallet (`grant_purpose_mismatch`, or the call domain's
    /// `grant_invalid`).
    ///
    /// A passphrase credential does not change the lock state: on a locked
    /// or mixing-only vault the unwrapped key serves this grant only.
    /// Errors: `WrongPassphrase`, `Throttled`, `NotEncrypted`, `Locked`,
    /// `MixingOnly`, `CredentialRequired`, `QuickUnlockUnavailable`.
    pub async fn authorize(
        &self,
        purpose: GrantPurpose,
        wallet_id: Option<String>,
        credential: VaultCredential,
    ) -> Result<AuthGrant, VaultError> {
        let wallet = wallet_id.as_deref().map(parse_wallet_id).transpose()?;
        let credential = OwnedCredential::from(credential);
        let purpose = purpose.into();
        self.op(move |v| {
            v.authorize(
                purpose,
                wallet.as_ref().map(|w| &w.0),
                credential.as_credential(),
            )
        })
        .await
        .map(Into::into)
    }

    /// Invalidates a grant before it expires. Unknown ids are ignored.
    pub fn revoke_grant(&self, grant_id: String) -> Result<(), VaultError> {
        self.check_open()?;
        self.session.vault().revoke_grant(&grant_id);
        Ok(())
    }

    /// The recovery phrase and BIP39 passphrase of `wallet_id`. Needs a
    /// `RevealSecret` grant for `wallet_id`. Errors: `NoSecret`,
    /// `GrantInvalid`, `GrantPurposeMismatch`, `Locked`.
    pub async fn reveal_mnemonic(
        &self,
        wallet_id: String,
        grant_id: String,
    ) -> Result<RevealedMnemonic, VaultError> {
        let id = parse_wallet_id(&wallet_id)?;
        let mut revealed = self
            .op(move |v| v.reveal_mnemonic(&id.0, &grant_id))
            .await?;
        // Moved out, not copied; the empty `Zeroizing` shells drop here.
        Ok(RevealedMnemonic {
            phrase: std::mem::take(&mut *revealed.phrase),
            bip39_passphrase: std::mem::take(&mut *revealed.bip39_passphrase),
        })
    }

    /// Enrols the biometric slot (M2): returns the wrap key the host stores in
    /// the OS biometric store. Needs a `ChangeCredential` grant.
    pub async fn enroll_quick_unlock(&self, grant_id: String) -> Result<Vec<u8>, VaultError> {
        let mut key = self.op(move |v| v.enroll_quick_unlock(&grant_id)).await?;
        Ok(std::mem::take(&mut *key))
    }

    /// Deletes the biometric slot (M2). Idempotent.
    pub async fn remove_quick_unlock(&self) -> Result<VaultStatus, VaultError> {
        self.op(|v| v.remove_quick_unlock()).await.map(Into::into)
    }
}
