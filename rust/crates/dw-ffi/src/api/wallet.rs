//! Wallets: mnemonic generation and import, registry, balances.
//! Owners: B (generate, check, import, the vault side of remove), E1 (list,
//! info, remove, rename, balances). Contract: docs/contracts/m1-engine.md §wallet.

use zeroize::Zeroizing;

use crate::api::common::{domain_error_common, parse_wallet_id};
use crate::{EngineError, NetworkSession};

/// Core balance buckets in duffs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, uniffi::Record)]
pub struct WalletBalances {
    pub confirmed: u64,
    pub unconfirmed: u64,
    pub immature: u64,
    pub locked: u64,
    pub total: u64,
    /// Spendable balance of the DIP9 CoinJoin accounts. dash-qt's "fully
    /// mixed" rounds rule is not applied yet (CoinJoin is WS-06).
    pub coinjoin: u64,
}

impl From<dw_engine::WalletBalances> for WalletBalances {
    fn from(b: dw_engine::WalletBalances) -> Self {
        Self {
            confirmed: b.confirmed,
            unconfirmed: b.unconfirmed,
            immature: b.immature,
            locked: b.locked,
            total: b.total,
            coinjoin: b.coinjoin,
        }
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
    /// Display name, 1–64 characters after trimming (`wallet.name_rejected`
    /// otherwise); `None` = engine default ("Wallet N"). Stored in dw-appdb.
    pub name: Option<String>,
    /// First block to scan. `Some(0)` = genesis; `None` = SPV tip or latest
    /// checkpoint for a new phrase.
    pub birth_height: Option<u32>,
    /// Derive the seed with Dash Core's BIP39 quirks (weak checksum, no NFKD,
    /// salt cut at 256 bytes) via dw-compat (QT-104).
    pub core_compat: bool,
    /// Address lookahead (gap limit, 1..=1000) of the BIP44 account's chains;
    /// `None` = the default (30), or 1000 with `core_compat` (dash-qt
    /// restores, QT-105). A raised gap is stored with the wallet and applied
    /// again each time the session opens.
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
    /// `None` until the scan has processed the wallet's birth block: an
    /// unsynced wallet shows "unknown", not 0 DASH (review M-3).
    pub balances: Option<WalletBalances>,
}

impl From<dw_engine::WalletInfo> for WalletInfo {
    fn from(w: dw_engine::WalletInfo) -> Self {
        Self {
            wallet_id: w.wallet_id.to_string(),
            name: w.name,
            watch_only: w.watch_only,
            has_mnemonic: w.has_mnemonic,
            hd: w.hd,
            birth_height: w.birth_height,
            created_at: w.created_at,
            balances: w.balances.map(Into::into),
        }
    }
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
    /// Code `wallet.invalid_xpub` (M2, QT-114): not an extended public key of
    /// this network, or not at an account level the engine can watch.
    #[error("invalid extended public key: {detail}")]
    InvalidXpub { detail: String },
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
        use dw_vault::VaultError as V;
        let detail = e.to_string();
        match e {
            E::InvalidMnemonic(_) => Self::InvalidMnemonic { detail },
            E::WalletAlreadyExists(_) => Self::AlreadyExists { detail },
            E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
            E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
            E::WalletNotFound(_) => Self::WalletNotFound { detail },
            E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
            E::NotImplemented(call) => Self::NotImplemented { call },
            E::NameRejected(detail) => Self::NameRejected { detail },
            E::HeightOutOfRange(_) | E::InvalidQuery(_) | E::StaleCursor => {
                Self::InvalidArgument { detail }
            }
            E::InvalidAddress(_) | E::AddressNoKey(_) | E::AddressNotMine(_) => {
                Self::InvalidArgument { detail }
            }
            E::Vault(v) => match v {
                V::NoVault => Self::NoVault,
                // Storing keys needs the data key with full scope.
                V::Locked | V::MixingOnly => Self::VaultLocked,
                V::GrantInvalid | V::GrantPurposeMismatch => Self::GrantInvalid,
                V::InvalidArgument(_) => Self::InvalidArgument { detail },
                V::Storage(_) | V::Corrupt(_) | V::OsStoreUnavailable(_) => {
                    Self::Storage { detail }
                }
                V::NotImplemented(call) => Self::NotImplemented {
                    call: call.to_string(),
                },
                _ => Self::Internal { detail },
            },
            E::Signer(_)
            | E::Wallet(_)
            | E::Sdk(_)
            | E::Spv(_)
            | E::SpvNotRunning
            | E::TxNotFound(_)
            | E::GapLimit
            | E::RequestNotFound(_)
            | E::Send(_)
            | E::Labels(_)
            | E::OutpointNotFound(_)
            | E::Compat(_)
            | E::Backup(_)
            | E::Psbt(_)
            | E::Internal(_) => Self::Internal { detail },
        }
    }
}

impl From<dw_vault::MnemonicError> for WalletError {
    fn from(e: dw_vault::MnemonicError) -> Self {
        use dw_vault::MnemonicError as M;
        match e {
            M::UnsupportedWordCount(word_count) => Self::UnsupportedWordCount { word_count },
            M::Invalid(detail) => Self::InvalidMnemonic { detail },
            M::PassphraseNotUtf8 => Self::InvalidArgument {
                detail: e.to_string(),
            },
            M::Entropy(detail) => Self::Internal { detail },
        }
    }
}

impl From<MnemonicLanguage> for dw_vault::mnemonic::Language {
    fn from(l: MnemonicLanguage) -> Self {
        match l {
            MnemonicLanguage::English => Self::English,
            MnemonicLanguage::ChineseSimplified => Self::ChineseSimplified,
            MnemonicLanguage::ChineseTraditional => Self::ChineseTraditional,
            MnemonicLanguage::Czech => Self::Czech,
            MnemonicLanguage::French => Self::French,
            MnemonicLanguage::Italian => Self::Italian,
            MnemonicLanguage::Japanese => Self::Japanese,
            MnemonicLanguage::Korean => Self::Korean,
            MnemonicLanguage::Portuguese => Self::Portuguese,
            MnemonicLanguage::Spanish => Self::Spanish,
        }
    }
}

impl From<dw_vault::mnemonic::Language> for MnemonicLanguage {
    fn from(l: dw_vault::mnemonic::Language) -> Self {
        use dw_vault::mnemonic::Language as L;
        match l {
            L::English => Self::English,
            L::ChineseSimplified => Self::ChineseSimplified,
            L::ChineseTraditional => Self::ChineseTraditional,
            L::Czech => Self::Czech,
            L::French => Self::French,
            L::Italian => Self::Italian,
            L::Japanese => Self::Japanese,
            L::Korean => Self::Korean,
            L::Portuguese => Self::Portuguese,
            L::Spanish => Self::Spanish,
        }
    }
}

crate::api::common::export_error_code!(WalletError);

impl WalletError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    fn code_str(&self) -> &'static str {
        match self {
            Self::InvalidMnemonic { .. } => "wallet.invalid_mnemonic",
            Self::UnsupportedWordCount { .. } => "wallet.unsupported_word_count",
            Self::AlreadyExists { .. } => "wallet.already_exists",
            Self::WatchOnlyExists { .. } => "wallet.watch_only_exists",
            Self::NoVault => "wallet.no_vault",
            Self::VaultLocked => "wallet.vault_locked",
            Self::GrantInvalid => "wallet.grant_invalid",
            Self::NameRejected { .. } => "wallet.name_rejected",
            Self::InvalidXpub { .. } => "wallet.invalid_xpub",
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
    let mut phrase = dw_vault::mnemonic::generate(word_count.into(), language.into())?;
    // Moved out, not copied (see vault.rs for the RustBuffer residual risk).
    Ok(std::mem::take(&mut *phrase))
}

/// Validates a typed or pasted phrase without importing it. Never fails
/// today; the `Result` keeps the contract's error channel.
#[uniffi::export]
pub fn check_mnemonic(phrase: Vec<u8>) -> Result<MnemonicCheck, WalletError> {
    let phrase = Zeroizing::new(phrase);
    let check = dw_vault::mnemonic::check(&phrase);
    Ok(MnemonicCheck {
        word_count: check.word_count,
        unknown_word_indices: check.unknown_word_indices,
        language: check.language.map(Into::into),
        checksum: match check.checksum {
            dw_vault::mnemonic::Checksum::Valid => MnemonicChecksum::Valid,
            dw_vault::mnemonic::Checksum::CoreOnly => MnemonicChecksum::CoreOnly,
            dw_vault::mnemonic::Checksum::Invalid => MnemonicChecksum::Invalid,
        },
    })
}

#[uniffi::export]
impl NetworkSession {
    /// Stores `mnemonic` (UTF-8 phrase bytes) and `bip39_passphrase` in the
    /// vault and reads them back, then registers the wallet (DESIGN-opus §1.8
    /// seed-safety order). Returns the wallet id. No wallet is registered
    /// without its seed in the vault (review H-1).
    ///
    /// Over a registered wallet whose seed the vault lacks, the seed is
    /// stored (keys attached). Errors: `InvalidMnemonic`, `AlreadyExists`,
    /// `NoVault`, `VaultLocked` (also when unlocked for mixing only),
    /// `NameRejected`, `InvalidArgument` (lookahead outside 1..=1000).
    pub async fn import_wallet(
        &self,
        mnemonic: Vec<u8>,
        bip39_passphrase: Vec<u8>,
        options: ImportOptions,
    ) -> Result<String, WalletError> {
        let mnemonic = Zeroizing::new(mnemonic);
        let bip39_passphrase = Zeroizing::new(bip39_passphrase);
        let id = self
            .inner
            .import_wallet(
                mnemonic,
                bip39_passphrase,
                dw_engine::ImportOptions {
                    birth_height: options.birth_height,
                    core_compat: options.core_compat,
                    name: options.name,
                    lookahead: options.lookahead,
                },
            )
            .await?;
        Ok(id.to_string())
    }

    /// Every registered wallet on this network, in creation order. In-memory
    /// read.
    pub fn wallet_infos(&self) -> Result<Vec<WalletInfo>, WalletError> {
        Ok(self
            .inner
            .wallet_infos()?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn wallet_info(&self, wallet_id: String) -> Result<WalletInfo, WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.wallet_info(&id)?.into())
    }

    /// Core balance buckets; `None` while not known yet (review M-3).
    /// In-memory read.
    pub fn balances(&self, wallet_id: String) -> Result<Option<WalletBalances>, EngineError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.balances(&id)?.map(Into::into))
    }

    /// Unloads the wallet, deletes its wallet-state rows, its app metadata
    /// and its vault records (IOS-109, QT-101 close+delete). Needs a `Wipe`
    /// grant. Emits `WalletRemoved`.
    pub async fn remove_wallet(
        &self,
        wallet_id: String,
        grant_id: String,
    ) -> Result<(), WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.remove_wallet(id, grant_id).await?)
    }

    /// Sets the display name (1–64 characters after trimming).
    pub async fn rename_wallet(&self, wallet_id: String, name: String) -> Result<(), WalletError> {
        let id = parse_wallet_id(&wallet_id)?;
        Ok(self.inner.rename_wallet(id, name).await?)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::{
        DashNetwork, Engine, EngineConfig, EngineEvent, EngineObserver, GrantPurpose, MessageError,
        SessionOptions, UnlockScope, VaultCredential, VaultError, VaultLockState,
    };

    const ABANDON_12: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    const PASSPHRASE: &[u8] = b"ffi test passphrase";

    #[derive(Default)]
    struct Recorder(Mutex<Vec<EngineEvent>>);

    impl EngineObserver for Recorder {
        fn on_event(&self, event: EngineEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn generates_and_checks_phrases() {
        let phrase = generate_mnemonic(24, MnemonicLanguage::English).unwrap();
        let check = check_mnemonic(phrase).unwrap();
        assert_eq!(check.word_count, 24);
        assert_eq!(check.language, Some(MnemonicLanguage::English));
        assert_eq!(check.checksum, MnemonicChecksum::Valid);

        assert!(matches!(
            generate_mnemonic(13, MnemonicLanguage::English),
            Err(WalletError::UnsupportedWordCount { word_count: 13 })
        ));
        let bad = check_mnemonic(ABANDON_12.replace("about", "zzz").into_bytes()).unwrap();
        assert_eq!(bad.unknown_word_indices, vec![11]);
        assert_eq!(bad.checksum, MnemonicChecksum::Invalid);
    }

    /// Import goes through the vault: nothing is registered without a
    /// usable vault (review H-1), and the stored phrase can be revealed.
    #[test]
    fn import_reveal_and_sign_through_the_vault() {
        let dir = tempfile::tempdir().unwrap();
        let rec = Arc::new(Recorder::default());
        let engine = Engine::new(
            EngineConfig {
                data_root: dir.path().to_string_lossy().into_owned(),
                worker_threads: Some(2),
            },
            rec.clone(),
        )
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let session = rt
            .block_on(engine.open_network(
                DashNetwork::Regtest,
                SessionOptions {
                    dapi_addresses: vec!["http://127.0.0.1:1".into()],
                    quorum_url: Some("http://127.0.0.1:1".into()),
                    spv_peers: vec!["127.0.0.1:1".into()],
                },
            ))
            .unwrap();
        let vault = session.vault();
        let import = |options: ImportOptions| {
            rt.block_on(session.import_wallet(ABANDON_12.into(), Vec::new(), options))
        };
        let genesis = ImportOptions {
            birth_height: Some(0),
            ..ImportOptions::default()
        };

        assert_eq!(vault.status().unwrap().state, VaultLockState::NoVault);
        assert!(matches!(import(genesis.clone()), Err(WalletError::NoVault)));
        assert!(session.wallet_infos().unwrap().is_empty());

        let status = rt
            .block_on(vault.create(Some(PASSPHRASE.to_vec())))
            .unwrap();
        assert_eq!(status.state, VaultLockState::Unlocked);
        assert!(status.encrypted);
        assert!(rec.0.lock().unwrap().contains(&EngineEvent::LockState {
            network: DashNetwork::Regtest,
            state: VaultLockState::Unlocked,
        }));

        let rejected = import(ImportOptions {
            lookahead: Some(1001),
            ..genesis.clone()
        });
        assert!(
            matches!(rejected, Err(WalletError::InvalidArgument { .. })),
            "{rejected:?}"
        );
        let rejected = import(ImportOptions {
            name: Some("".into()),
            ..genesis.clone()
        });
        assert!(
            matches!(rejected, Err(WalletError::NameRejected { .. })),
            "{rejected:?}"
        );

        let id = import(ImportOptions {
            name: Some("Main".into()),
            ..genesis.clone()
        })
        .unwrap();
        let infos = session.wallet_infos().unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].wallet_id, id);
        assert_eq!(infos[0].name, "Main");
        assert!(infos[0].has_mnemonic && !infos[0].watch_only && infos[0].hd);
        assert_eq!(infos[0].balances, None, "not scanned yet");
        assert_eq!(session.balances(id.clone()).unwrap(), None);
        rt.block_on(session.rename_wallet(id.clone(), " Daily ".into()))
            .unwrap();
        assert_eq!(session.wallet_info(id.clone()).unwrap().name, "Daily");
        assert!(matches!(
            session.wallet_info(id.to_uppercase()),
            Err(WalletError::InvalidArgument { .. })
        ));
        assert_eq!(
            vault.status().unwrap().wallets_with_secrets,
            vec![id.clone()]
        );
        assert!(matches!(
            import(genesis.clone()),
            Err(WalletError::AlreadyExists { .. })
        ));

        assert_eq!(vault.lock().unwrap().state, VaultLockState::Locked);
        assert!(matches!(
            import(genesis),
            Err(WalletError::AlreadyExists { .. })
        ));
        let wrong = rt.block_on(vault.unlock(b"nope".to_vec(), UnlockScope::Full));
        assert!(
            matches!(
                wrong,
                Err(VaultError::WrongPassphrase {
                    failed_attempts: 1,
                    ..
                })
            ),
            "{wrong:?}"
        );

        let passphrase = || VaultCredential::Passphrase {
            passphrase: PASSPHRASE.to_vec(),
        };
        // Wallet-scoped grants name their wallet.
        assert!(matches!(
            rt.block_on(vault.authorize(GrantPurpose::RevealSecret, None, passphrase())),
            Err(VaultError::InvalidArgument { .. })
        ));
        // A passphrase credential issues the grant and leaves the vault
        // locked (dash-qt re-lock parity); the grant carries the key.
        let grant = rt
            .block_on(vault.authorize(GrantPurpose::RevealSecret, Some(id.clone()), passphrase()))
            .unwrap();
        assert!(grant.single_use);
        assert_eq!(vault.status().unwrap().state, VaultLockState::Locked);
        assert!(matches!(
            rt.block_on(vault.reveal_mnemonic("XYZ".into(), grant.id.clone())),
            Err(VaultError::InvalidArgument { .. })
        ));
        let revealed = rt
            .block_on(vault.reveal_mnemonic(id.clone(), grant.id.clone()))
            .unwrap();
        assert_eq!(revealed.phrase, ABANDON_12.as_bytes());
        assert!(revealed.bip39_passphrase.is_empty());
        assert!(matches!(
            rt.block_on(vault.reveal_mnemonic(id.clone(), grant.id)),
            Err(VaultError::GrantInvalid)
        ));

        // Unlocked, reveal still needs the passphrase.
        rt.block_on(vault.unlock(PASSPHRASE.to_vec(), UnlockScope::Full))
            .unwrap();
        assert!(matches!(
            rt.block_on(vault.authorize(
                GrantPurpose::RevealSecret,
                Some(id.clone()),
                VaultCredential::Unencrypted
            )),
            Err(VaultError::CredentialRequired)
        ));

        // An address outside the wallet is refused before the grant is used.
        let grant = rt
            .block_on(vault.authorize(
                GrantPurpose::SignMessage,
                Some(id.clone()),
                VaultCredential::Unencrypted,
            ))
            .unwrap();
        let signed = rt.block_on(session.sign_message(
            id.clone(),
            "yQWsoTNJq59DqBg4Z2Qup3k3qchPaWz29n".into(),
            "hi".into(),
            grant.id,
        ));
        assert!(
            matches!(signed, Err(MessageError::AddressNotMine)),
            "{signed:?}"
        );

        // Removal needs a Wipe grant; afterwards the wallet and its keys are gone.
        assert!(matches!(
            rt.block_on(session.remove_wallet(id.clone(), "no-such-grant".into())),
            Err(WalletError::GrantInvalid)
        ));
        let wipe = rt
            .block_on(vault.authorize(GrantPurpose::Wipe, Some(id.clone()), passphrase()))
            .unwrap();
        rt.block_on(session.remove_wallet(id.clone(), wipe.id))
            .unwrap();
        assert!(session.wallet_infos().unwrap().is_empty());
        assert!(vault.status().unwrap().wallets_with_secrets.is_empty());
        assert!(rec.0.lock().unwrap().contains(&EngineEvent::WalletRemoved {
            network: DashNetwork::Regtest,
            wallet_id: id.clone(),
        }));
        assert!(matches!(
            session.wallet_info(id),
            Err(WalletError::WalletNotFound { .. })
        ));
        rt.block_on(engine.shutdown()).unwrap();
        assert!(matches!(
            vault.status(),
            Err(VaultError::NetworkNotOpen { .. })
        ));
    }
}
