//! Wallets: mnemonic generation and import, registry, balances.
//! Owners: B (generate, check, import, the vault side of remove), E1 (list,
//! info, remove, rename, balances). Contract: docs/contracts/m1-engine.md §wallet.

use zeroize::Zeroizing;

use crate::api::common::{domain_error_common, not_implemented, parse_wallet_id};
use crate::{EngineError, NetworkSession};

/// Core balance buckets in duffs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
}

impl From<dw_engine::WalletBalances> for WalletBalances {
    fn from(b: dw_engine::WalletBalances) -> Self {
        Self {
            confirmed: b.confirmed,
            unconfirmed: b.unconfirmed,
            immature: b.immature,
            locked: b.locked,
            total: b.total,
        }
    }
}

/// M0 row of `list_wallets`; superseded by `WalletInfo`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletSummary {
    /// 64-char lowercase hex.
    pub wallet_id: String,
    pub balances: WalletBalances,
}

/// M0 result of `create_wallet`. Superseded by `generate_mnemonic` +
/// `import_wallet` (review finding H1); removed when B lands the vault.
#[derive(Clone, uniffi::Record)]
pub struct CreatedWallet {
    pub wallet_id: String,
    pub mnemonic: String,
}

impl std::fmt::Debug for CreatedWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedWallet")
            .field("wallet_id", &self.wallet_id)
            .finish_non_exhaustive()
    }
}

/// BIP39 wordlists (IOS-007: restore in 10 languages).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MnemonicLanguage {
    English,
    ChineseSimplified,
    ChineseTraditional,
    Czech,
    French,
    Italian,
    Japanese,
    Korean,
    Portuguese,
    Spanish,
}

/// Checksum verdict of `check_mnemonic`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum MnemonicChecksum {
    /// Valid BIP39 checksum.
    Valid,
    /// Invalid under BIP39 but accepted by Dash Core's weak check (QT-104);
    /// importable only with `ImportOptions.core_compat`.
    CoreOnly,
    Invalid,
}

/// Word-by-word validation for the restore screen.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MnemonicCheck {
    pub word_count: u32,
    /// Zero-based positions of words not in the detected wordlist.
    pub unknown_word_indices: Vec<u32>,
    /// `None` when no wordlist contains every word.
    pub language: Option<MnemonicLanguage>,
    pub checksum: MnemonicChecksum,
}

/// Options for `import_wallet`.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct ImportOptions {
    /// Display name; `None` = engine default ("Wallet N").
    pub name: Option<String>,
    /// First block to scan. `Some(0)` = genesis; `None` = SPV tip or latest
    /// checkpoint for a new phrase.
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 quirks (weak checksum, no NFKD,
    /// salt cut at 256 bytes) via dw-compat (QT-104).
    pub core_compat: bool,
    /// Address lookahead for the restore scan; `None` = engine default.
    /// dash-qt restores use 1000 (QT-105).
    pub lookahead: Option<u32>,
}

/// One registered wallet (QT-014, QT-101, IOS-110).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WalletInfo {
    pub wallet_id: String,
    pub name: String,
    /// No signing keys are available for this wallet (QT-035, QT-114).
    pub watch_only: bool,
    /// The vault holds a mnemonic for this wallet (recovery phrase can be shown).
    pub has_mnemonic: bool,
    /// HD wallet (dash-qt HD icon, QT-021).
    pub hd: bool,
    pub birth_height: Option<u32>,
    /// UNIX seconds the wallet was added on this device.
    pub created_at: Option<u64>,
    pub balances: WalletBalances,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum WalletError {
    /// Code `wallet.invalid_mnemonic`: not a phrase in any supported wordlist,
    /// or a checksum that fails even Dash Core's weak check.
    #[error("invalid mnemonic: {detail}")]
    InvalidMnemonic { detail: String },
    /// Code `wallet.unsupported_word_count`.
    #[error("unsupported word count {word_count}")]
    UnsupportedWordCount { word_count: u32 },
    /// Code `wallet.already_exists`: the same wallet is already registered
    /// with keys on this network.
    #[error("wallet already exists: {detail}")]
    AlreadyExists { detail: String },
    /// Code `wallet.watch_only_exists`: a watch-only wallet with the same id
    /// exists and the engine could not attach the keys to it.
    #[error("watch-only wallet {wallet_id} exists")]
    WatchOnlyExists { wallet_id: String },
    /// Code `wallet.no_vault`: create the vault first.
    #[error("no vault")]
    NoVault,
    /// Code `wallet.vault_locked`: the vault must be unlocked to store keys.
    #[error("vault locked")]
    VaultLocked,
    /// Code `wallet.grant_invalid`: missing, expired or wrong-purpose grant.
    #[error("grant invalid")]
    GrantInvalid,
    /// Code `wallet.name_rejected`: empty or longer than 64 characters.
    #[error("name rejected: {detail}")]
    NameRejected { detail: String },
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

domain_error_common!(@not_implemented WalletError);

impl From<dw_engine::EngineError> for WalletError {
    fn from(e: dw_engine::EngineError) -> Self {
        use dw_engine::EngineError as E;
        let detail = e.to_string();
        match e {
            E::InvalidMnemonic(_) => Self::InvalidMnemonic { detail },
            E::WalletAlreadyExists(_) => Self::AlreadyExists { detail },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(_) => Self::NotImplemented { call: detail },
            E::Wallet(_) | E::Sdk(_) | E::Spv(_) | E::Internal(_) => Self::Internal { detail },
        }
    }
}

impl WalletError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidMnemonic { .. } => "wallet.invalid_mnemonic",
            Self::UnsupportedWordCount { .. } => "wallet.unsupported_word_count",
            Self::AlreadyExists { .. } => "wallet.already_exists",
            Self::WatchOnlyExists { .. } => "wallet.watch_only_exists",
            Self::NoVault => "wallet.no_vault",
            Self::VaultLocked => "wallet.vault_locked",
            Self::GrantInvalid => "wallet.grant_invalid",
            Self::NameRejected { .. } => "wallet.name_rejected",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NetworkNotOpen { .. } => "network_not_open",
            Self::WalletNotFound { .. } => "wallet_not_found",
            Self::Storage { .. } => "storage",
            Self::NotImplemented { .. } => "not_implemented",
            Self::Internal { .. } => "internal",
        }
    }
}

/// A fresh BIP39 phrase (12, 15, 18, 21 or 24 words) as UTF-8 bytes. Nothing
/// is stored: the host shows it, verifies the user wrote it down (QT-103,
/// IOS-004) and then passes it to `import_wallet`.
#[uniffi::export]
pub fn generate_mnemonic(
    word_count: u8,
    language: MnemonicLanguage,
) -> Result<Vec<u8>, WalletError> {
    let _ = (word_count, language);
    not_implemented("generate_mnemonic")
}

/// Validates a typed or pasted phrase without importing it.
#[uniffi::export]
pub fn check_mnemonic(phrase: Vec<u8>) -> Result<MnemonicCheck, WalletError> {
    drop(Zeroizing::new(phrase));
    not_implemented("check_mnemonic")
}

#[uniffi::export]
impl NetworkSession {
    /// M0: registers a wallet from a fresh English phrase and returns the
    /// phrase. The phrase is stored nowhere (review finding H1). Superseded
    /// by `generate_mnemonic` + `import_wallet`; B removes it.
    pub async fn create_wallet(&self, word_count: u8) -> Result<CreatedWallet, EngineError> {
        let created = self.inner.create_wallet(word_count).await?;
        Ok(CreatedWallet {
            wallet_id: created.wallet_id.to_string(),
            mnemonic: created.mnemonic.as_str().to_owned(),
        })
    }

    /// Registers a wallet from `mnemonic` (UTF-8 phrase bytes) and stores the
    /// phrase and `bip39_passphrase` in the vault, in the seed-safety order
    /// of DESIGN-opus §1.8. Returns the wallet id.
    ///
    /// Current behaviour (until B lands the vault): only an empty passphrase
    /// and default options other than `birth_height` are accepted; anything
    /// else returns `NotImplemented`. The wallet is registered but the phrase
    /// is not stored.
    pub async fn import_wallet(
        &self,
        mnemonic: Vec<u8>,
        bip39_passphrase: Vec<u8>,
        options: ImportOptions,
    ) -> Result<String, WalletError> {
        let mut mnemonic = Zeroizing::new(mnemonic);
        let bip39_passphrase = Zeroizing::new(bip39_passphrase);
        if !bip39_passphrase.is_empty() {
            return not_implemented("NetworkSession.import_wallet(bip39_passphrase)");
        }
        if options.core_compat || options.lookahead.is_some() || options.name.is_some() {
            return not_implemented("NetworkSession.import_wallet(options)");
        }
        let phrase = match String::from_utf8(std::mem::take(&mut *mnemonic)) {
            Ok(s) => Zeroizing::new(s),
            Err(e) => {
                drop(Zeroizing::new(e.into_bytes()));
                return Err(WalletError::InvalidMnemonic {
                    detail: "mnemonic is not UTF-8".into(),
                });
            }
        };
        let id = self
            .inner
            .import_wallet(phrase, options.birth_height)
            .await?;
        Ok(id.to_string())
    }

    /// M0: registered wallets with balances. Superseded by `wallet_infos`.
    pub fn list_wallets(&self) -> Result<Vec<WalletSummary>, EngineError> {
        Ok(self
            .inner
            .list_wallets()?
            .into_iter()
            .map(|w| WalletSummary {
                wallet_id: w.wallet_id.to_string(),
                balances: w.balances.into(),
            })
            .collect())
    }

    /// Every registered wallet on this network, in creation order.
    pub fn wallet_infos(&self) -> Result<Vec<WalletInfo>, WalletError> {
        not_implemented("NetworkSession.wallet_infos")
    }

    pub fn wallet_info(&self, wallet_id: String) -> Result<WalletInfo, WalletError> {
        let _ = parse_wallet_id(&wallet_id)?;
        not_implemented("NetworkSession.wallet_info")
    }

    /// Core balance buckets. In-memory read.
    pub fn balances(&self, wallet_id: String) -> Result<WalletBalances, EngineError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.balances(&id)?.into())
    }

    /// Unloads the wallet, deletes its wallet-state rows and its vault
    /// records (IOS-109, QT-101 close+delete). Needs a `Wipe` grant.
    pub async fn remove_wallet(
        &self,
        wallet_id: String,
        grant_id: String,
    ) -> Result<(), WalletError> {
        let _ = (parse_wallet_id(&wallet_id)?, grant_id);
        not_implemented("NetworkSession.remove_wallet")
    }

    /// Sets the display name (1–64 characters after trimming).
    pub async fn rename_wallet(&self, wallet_id: String, name: String) -> Result<(), WalletError> {
        let _ = (parse_wallet_id(&wallet_id)?, name);
        not_implemented("NetworkSession.rename_wallet")
    }
}
