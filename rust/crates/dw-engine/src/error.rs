use dw_vault::{SignerError, VaultError};
use platform_wallet::PlatformWalletError;
use platform_wallet_storage::WalletStorageError;

/// Engine error. The `Display` text is diagnostic detail for logs; the UI maps
/// the variant (and `code()`) to localized copy and never shows this text raw.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("network session is not open: {0}")]
    NetworkNotOpen(String),
    /// The wallet database for this network is still held by a previous
    /// session in this process (SqlitePersister's open-path registry).
    #[error("wallet storage already open in this process: {0}")]
    StorageInUse(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("wallet not found: {0}")]
    WalletNotFound(String),
    /// The phrase is not a valid BIP39 mnemonic in any supported wordlist.
    #[error("invalid mnemonic: {0}")]
    InvalidMnemonic(String),
    /// A wallet with the same id is already registered on this network.
    #[error("wallet already exists: {0}")]
    WalletAlreadyExists(String),
    #[error("wallet error: {0}")]
    Wallet(String),
    #[error("platform sdk error: {0}")]
    Sdk(String),
    #[error("spv error: {0}")]
    Spv(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    /// The call needs a running SPV client.
    #[error("spv is not running")]
    SpvNotRunning,
    /// A block height above the known tip.
    #[error("height {0} is out of range")]
    HeightOutOfRange(u32),
    /// A history query with a bad limit, date range or search text.
    #[error("invalid history query: {0}")]
    InvalidQuery(String),
    /// A history cursor from a different query, or one that cannot be parsed.
    #[error("stale history cursor")]
    StaleCursor,
    /// The wallet has no transaction with this txid.
    #[error("transaction not found: {0}")]
    TxNotFound(String),
    /// Issuing another receive address would leave more unused addresses
    /// than the gap limit lets a restore find.
    #[error("receive gap limit reached")]
    GapLimit,
    /// The vault refused the operation (no vault, locked, bad grant, …).
    #[error("vault: {0}")]
    Vault(VaultError),
    /// The vault signer refused or failed.
    #[error("signer: {0}")]
    Signer(SignerError),
    /// Not a Dash address of this network.
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    /// A valid address that does not refer to a key (P2SH).
    #[error("address does not refer to a key: {0}")]
    AddressNoKey(String),
    /// The address is not one of the wallet's addresses.
    #[error("address not in wallet: {0}")]
    AddressNotMine(String),
    /// A bug: a panic inside an engine task, a poisoned lock, a runtime that
    /// could not be built.
    #[error("internal error: {0}")]
    Internal(String),
    /// A payment could not be drafted, prepared or sent (`send.*` codes).
    #[error("send: {0}")]
    Send(crate::send::SendFailure),
    /// An address-book rule was violated (`labels.*` codes).
    #[error("labels: {0}")]
    Labels(crate::labels::LabelsFailure),
    /// Not an unspent output of the wallet (`coins.outpoint_not_found`).
    #[error("outpoint not found: {0}")]
    OutpointNotFound(dashcore::OutPoint),
}

impl EngineError {
    /// Stable machine-readable code for each variant.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::InvalidConfig(_) => "invalid_config",
            EngineError::InvalidArgument(_) => "invalid_argument",
            EngineError::NetworkNotOpen(_) => "network_not_open",
            EngineError::StorageInUse(_) => "storage_in_use",
            EngineError::Storage(_) => "storage",
            EngineError::WalletNotFound(_) => "wallet_not_found",
            EngineError::InvalidMnemonic(_) => "invalid_mnemonic",
            EngineError::WalletAlreadyExists(_) => "wallet_already_exists",
            EngineError::Wallet(_) => "wallet",
            EngineError::Sdk(_) => "sdk",
            EngineError::Spv(_) => "spv",
            EngineError::Io(_) => "io",
            EngineError::NotImplemented(_) => "not_implemented",
            EngineError::SpvNotRunning => "spv_not_running",
            EngineError::HeightOutOfRange(_) => "height_out_of_range",
            EngineError::InvalidQuery(_) => "invalid_query",
            EngineError::StaleCursor => "stale_cursor",
            EngineError::TxNotFound(_) => "tx_not_found",
            EngineError::GapLimit => "gap_limit",
            EngineError::Vault(_) => "vault",
            EngineError::Signer(_) => "signer",
            EngineError::InvalidAddress(_) => "invalid_address",
            EngineError::AddressNoKey(_) => "address_no_key",
            EngineError::AddressNotMine(_) => "address_not_mine",
            EngineError::Internal(_) => "internal",
            EngineError::Send(_) => "send",
            EngineError::Labels(_) => "labels",
            EngineError::OutpointNotFound(_) => "outpoint_not_found",
        }
    }
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        EngineError::Io(e.to_string())
    }
}

impl From<VaultError> for EngineError {
    fn from(e: VaultError) -> Self {
        EngineError::Vault(e)
    }
}

impl From<dw_appdb::AppDbError> for EngineError {
    fn from(e: dw_appdb::AppDbError) -> Self {
        EngineError::Storage(format!("app.sqlite: {e}"))
    }
}

impl From<SignerError> for EngineError {
    fn from(e: SignerError) -> Self {
        EngineError::Signer(e)
    }
}

impl From<WalletStorageError> for EngineError {
    fn from(e: WalletStorageError) -> Self {
        match e {
            WalletStorageError::AlreadyOpen { .. } => EngineError::StorageInUse(e.to_string()),
            other => EngineError::Storage(other.to_string()),
        }
    }
}

impl From<PlatformWalletError> for EngineError {
    fn from(e: PlatformWalletError) -> Self {
        match e {
            PlatformWalletError::SpvAlreadyRunning | PlatformWalletError::SpvError(_) => {
                EngineError::Spv(e.to_string())
            }
            PlatformWalletError::PersisterLoad(_) | PlatformWalletError::PersisterRestore(_) => {
                EngineError::Storage(e.to_string())
            }
            PlatformWalletError::WalletAlreadyExists(_) => {
                EngineError::WalletAlreadyExists(e.to_string())
            }
            PlatformWalletError::WalletNotFound(_) => EngineError::WalletNotFound(e.to_string()),
            other => EngineError::Wallet(other.to_string()),
        }
    }
}

impl From<dash_sdk::Error> for EngineError {
    fn from(e: dash_sdk::Error) -> Self {
        EngineError::Sdk(e.to_string())
    }
}

impl From<tokio::task::JoinError> for EngineError {
    fn from(e: tokio::task::JoinError) -> Self {
        if e.is_panic() {
            EngineError::Internal(format!("engine task panicked: {e}"))
        } else {
            EngineError::Internal(format!("engine task cancelled: {e}"))
        }
    }
}
