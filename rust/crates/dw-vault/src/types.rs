//! Public value types of the vault.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use zeroize::Zeroizing;

use crate::crypto::KdfPolicy;
use crate::os_store::{KeyringOsStore, OsSecretStore};

/// 32-byte network-scoped wallet id (key-wallet's `compute_wallet_id`).
pub type WalletId = [u8; 32];

/// Lock state of a network's vault (QT-022, QT-111/112, IOS-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LockState {
    /// No vault file on this network yet.
    NoVault,
    /// Unencrypted vault that holds no wallet secret yet (every wallet is
    /// watch-only). An encrypted vault without secrets reports
    /// `Locked`/`Unlocked`, because adding keys needs the passphrase.
    NoKeys,
    /// The data key lives in the OS secret store (slot O); no passphrase.
    Unencrypted,
    /// Passphrase-protected; the data key is not in memory.
    Locked,
    /// Data key in memory; signing limited to the CoinJoin account (QT-112).
    UnlockedMixingOnly,
    /// Data key in memory.
    Unlocked,
}

/// What `unlock` makes available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnlockScope {
    Full,
    /// dash-qt "Unlock for mixing only".
    MixingOnly,
}

/// In-memory snapshot of the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultStatus {
    pub state: LockState,
    /// A passphrase slot exists.
    pub encrypted: bool,
    /// Always `false` until slot B lands (TODO(biometric), M2).
    pub quick_unlock_enrolled: bool,
    pub failed_attempts: u32,
    /// Seconds until the next passphrase attempt is accepted, when throttled.
    pub retry_after_secs: Option<u64>,
    /// Wallets with a seed record, sorted.
    pub wallets_with_secrets: Vec<WalletId>,
}

/// Credential presented to [`crate::Vault::authorize`].
#[derive(Clone, Copy)]
pub enum Credential<'a> {
    /// Vault passphrase bytes. Also unlocks a locked vault (scope Full).
    Passphrase(&'a [u8]),
    /// Key released by the OS biometric store (slot B, M2).
    QuickUnlock(&'a [u8]),
    /// No credential: accepted when the vault is unencrypted or already
    /// unlocked with scope Full. Whether the app allows this (dash-qt's
    /// behaviour with "require authentication for every payment" off) is the
    /// host's setting.
    None,
}

impl std::fmt::Debug for Credential<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Credential::Passphrase(_) => "Passphrase(<redacted>)",
            Credential::QuickUnlock(_) => "QuickUnlock(<redacted>)",
            Credential::None => "None",
        })
    }
}

/// What a grant authorizes (DESIGN-opus §1.8 "Auth grants").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrantPurpose {
    /// Signing a transaction that debits at most `max_duffs`.
    Spend {
        max_duffs: u64,
    },
    RevealSecret,
    SignMessage,
    ChangeCredential,
    Wipe,
    MasternodeOp,
    Governance,
    PlatformOp,
}

/// [`GrantPurpose`] without its payload; what a call expects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GrantKind {
    Spend,
    RevealSecret,
    SignMessage,
    ChangeCredential,
    Wipe,
    MasternodeOp,
    Governance,
    PlatformOp,
}

impl GrantPurpose {
    pub fn kind(&self) -> GrantKind {
        match self {
            GrantPurpose::Spend { .. } => GrantKind::Spend,
            GrantPurpose::RevealSecret => GrantKind::RevealSecret,
            GrantPurpose::SignMessage => GrantKind::SignMessage,
            GrantPurpose::ChangeCredential => GrantKind::ChangeCredential,
            GrantPurpose::Wipe => GrantKind::Wipe,
            GrantPurpose::MasternodeOp => GrantKind::MasternodeOp,
            GrantPurpose::Governance => GrantKind::Governance,
            GrantPurpose::PlatformOp => GrantKind::PlatformOp,
        }
    }

    /// Whether a redeemed grant of this purpose may obtain a full signer.
    pub(crate) fn signs(&self) -> bool {
        matches!(
            self,
            GrantPurpose::Spend { .. }
                | GrantPurpose::SignMessage
                | GrantPurpose::MasternodeOp
                | GrantPurpose::Governance
                | GrantPurpose::PlatformOp
        )
    }
}

/// An issued grant. Grants live in vault memory only and are revoked when
/// the vault locks or its unlock scope changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthGrant {
    pub id: String,
    pub purpose: GrantPurpose,
    /// UNIX seconds after which the grant is refused.
    pub expires_at: u64,
    /// Consumed by its first redemption.
    pub single_use: bool,
}

/// Proof that a grant was redeemed. Not cloneable and only built by the
/// vault, so holding one means the authorization check passed.
#[derive(Debug)]
pub struct GrantToken {
    pub(crate) purpose: GrantPurpose,
    pub(crate) epoch: u64,
}

impl GrantToken {
    pub fn purpose(&self) -> GrantPurpose {
        self.purpose
    }

    /// The spend cap of a `Spend` grant.
    pub fn max_duffs(&self) -> Option<u64> {
        match self.purpose {
            GrantPurpose::Spend { max_duffs } => Some(max_duffs),
            _ => None,
        }
    }
}

/// How the 64-byte seed of a wallet was derived from its phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeedDerivation {
    /// Standard BIP39 (NFKD, full salt).
    Bip39,
    /// Dash Core `CMnemonic::ToSeed` (no NFKD, salt cut at 256 bytes).
    /// `weak_checksum`: only Core's weak checksum accepts the phrase.
    DashCore { weak_checksum: bool },
}

/// Secret material of one wallet, as stored in the vault.
pub struct WalletSecret {
    /// The recovery phrase as the vault returns it on reveal (normalized:
    /// single spaces; lower case for Core-compatible phrases).
    pub mnemonic: Zeroizing<Vec<u8>>,
    /// BIP39 passphrase ("25th word"); empty when none.
    pub mnemonic_passphrase: Zeroizing<Vec<u8>>,
    /// The seed every key of the wallet derives from.
    pub seed: Zeroizing<[u8; 64]>,
    pub derivation: SeedDerivation,
}

impl std::fmt::Debug for WalletSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalletSecret")
            .field("derivation", &self.derivation)
            .finish_non_exhaustive()
    }
}

/// A revealed recovery phrase. Both fields are secret.
pub struct RevealedMnemonic {
    pub phrase: Zeroizing<Vec<u8>>,
    pub bip39_passphrase: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for RevealedMnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RevealedMnemonic(<redacted>)")
    }
}

/// Wall clock in UNIX seconds. Injected so grant expiry and throttling can be
/// tested without sleeping.
pub trait Clock: Send + Sync {
    fn now_secs(&self) -> u64;
}

/// [`Clock`] backed by `SystemTime`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Vault construction options.
#[derive(Clone)]
pub struct VaultConfig {
    /// Argon2id parameter choice for new passphrase slots.
    pub kdf: KdfPolicy,
    /// Slot O backend.
    pub os_store: Arc<dyn OsSecretStore>,
    pub clock: Arc<dyn Clock>,
    /// Lifetime of issued grants.
    pub grant_ttl_secs: u64,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            kdf: KdfPolicy::Calibrated,
            os_store: Arc::new(KeyringOsStore::new()),
            clock: Arc::new(SystemClock),
            grant_ttl_secs: DEFAULT_GRANT_TTL_SECS,
        }
    }
}

impl std::fmt::Debug for VaultConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultConfig")
            .field("kdf", &self.kdf)
            .field("os_store", &self.os_store.name())
            .field("grant_ttl_secs", &self.grant_ttl_secs)
            .finish_non_exhaustive()
    }
}

/// Default grant lifetime: long enough for one confirm-and-sign round.
pub const DEFAULT_GRANT_TTL_SECS: u64 = 120;
