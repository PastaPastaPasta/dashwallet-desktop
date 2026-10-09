use dpp::platform_value::string_encoding::Encoding;
use dw_vault::{SignerError, VaultError};
use platform_wallet::PlatformWalletError;
use platform_wallet::error::promote_identity_insufficient_balance;
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
    /// The identity's credit balance cannot cover a Platform operation
    /// (`platform.insufficient_credits{needed, available}`, DASHPAY §3.6).
    /// `identity_id` is Base58.
    #[error(
        "identity {identity_id} has insufficient credits: {needed} needed, {available} available"
    )]
    InsufficientCredits {
        identity_id: String,
        needed: u64,
        available: u64,
    },
    #[error("spv error: {0}")]
    Spv(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("not implemented: {0}")]
    NotImplemented(String),
    /// The call needs a running SPV client.
    #[error("spv is not running")]
    SpvNotRunning,
    /// The call needs the SPV client stopped (`reset_chain_data`).
    #[error("spv is running")]
    SpvRunning,
    /// A rescan the engine started is still running.
    #[error("a rescan is in progress")]
    RescanInProgress,
    /// Not a BIP44 account-level extended public key of this network.
    #[error("invalid xpub: {0}")]
    InvalidXpub(String),
    /// The transaction's state forbids abandon or resend.
    #[error("transaction action refused: {0:?}")]
    TxActionRefused(crate::TxActionRefusal),
    /// No connected peer to announce a transaction to.
    #[error("no connected peers")]
    NoPeers,
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
    /// No stored payment request has this id.
    #[error("receive request {0} not found")]
    RequestNotFound(u64),
    /// A wallet name that is empty or longer than 64 characters after
    /// trimming.
    #[error("wallet name rejected: {0}")]
    NameRejected(String),
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
    /// A Dash Core file or export failed (`compat.*` codes).
    #[error("compat: {0}")]
    Compat(crate::compat::CompatFailure),
    /// A backup could not be written or restored (`backup.*` codes).
    #[error("backup: {0}")]
    Backup(crate::backup::BackupFailure),
    /// A PSBT could not be read, signed or sent (`psbt.*` codes).
    #[error("psbt: {0}")]
    Psbt(crate::send::psbt::PsbtFailure),
    /// A CoinJoin call failed (`coinjoin.*` codes, M3).
    #[error("coinjoin: {0}")]
    CoinJoin(crate::coinjoin::CoinJoinFailure),
    /// A masternode keychain call failed (`masternode.*` codes, M3).
    #[error("masternode: {0}")]
    Masternode(crate::masternode_keys::MasternodeFailure),
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
            EngineError::InsufficientCredits { .. } => "insufficient_credits",
            EngineError::Spv(_) => "spv",
            EngineError::Io(_) => "io",
            EngineError::NotImplemented(_) => "not_implemented",
            EngineError::SpvNotRunning => "spv_not_running",
            EngineError::SpvRunning => "spv_running",
            EngineError::RescanInProgress => "rescan_in_progress",
            EngineError::InvalidXpub(_) => "invalid_xpub",
            EngineError::TxActionRefused(_) => "tx_action_refused",
            EngineError::NoPeers => "no_peers",
            EngineError::HeightOutOfRange(_) => "height_out_of_range",
            EngineError::InvalidQuery(_) => "invalid_query",
            EngineError::StaleCursor => "stale_cursor",
            EngineError::TxNotFound(_) => "tx_not_found",
            EngineError::GapLimit => "gap_limit",
            EngineError::RequestNotFound(_) => "request_not_found",
            EngineError::NameRejected(_) => "name_rejected",
            EngineError::Vault(_) => "vault",
            EngineError::Signer(_) => "signer",
            EngineError::InvalidAddress(_) => "invalid_address",
            EngineError::AddressNoKey(_) => "address_no_key",
            EngineError::AddressNotMine(_) => "address_not_mine",
            EngineError::Internal(_) => "internal",
            EngineError::Send(_) => "send",
            EngineError::Labels(_) => "labels",
            EngineError::OutpointNotFound(_) => "outpoint_not_found",
            EngineError::Compat(_) => "compat",
            EngineError::Backup(_) => "backup",
            EngineError::Psbt(_) => "psbt",
            EngineError::CoinJoin(_) => "coinjoin",
            EngineError::Masternode(_) => "masternode",
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
        // Most Platform calls (contact requests, DPNS, transfers, tokens)
        // hand back the balance refusal still inside the SDK error.
        if let PlatformWalletError::Sdk(source)
        | PlatformWalletError::TokenOperationFailed { source, .. } = &e
            && let Some(promoted) = promote_identity_insufficient_balance(source)
        {
            return promoted.into();
        }
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
            PlatformWalletError::InsufficientIdentityCredits {
                identity_id,
                required,
                available,
            } => EngineError::InsufficientCredits {
                identity_id: identity_id.to_string(Encoding::Base58),
                needed: required,
                available,
            },
            other => EngineError::Wallet(other.to_string()),
        }
    }
}

impl From<dash_sdk::Error> for EngineError {
    fn from(e: dash_sdk::Error) -> Self {
        // Platform's balance refusal (`IdentityInsufficientBalanceError`)
        // keeps its figures instead of becoming SDK text.
        match promote_identity_insufficient_balance(&e) {
            Some(promoted) => promoted.into(),
            None => EngineError::Sdk(e.to_string()),
        }
    }
}

/// The detail of the `Internal` error a panicking engine task becomes. The
/// panic's message is withheld: it may quote the task's inputs, and this
/// text reaches hosts and logs.
pub const TASK_PANICKED: &str = "engine task panicked (message withheld)";

impl EngineError {
    /// Whether this is a panicking engine task's error (`TASK_PANICKED`):
    /// the task's outcome is unknown.
    pub fn is_task_panic(&self) -> bool {
        matches!(self, EngineError::Internal(d) if d == TASK_PANICKED)
    }
}

impl From<tokio::task::JoinError> for EngineError {
    fn from(e: tokio::task::JoinError) -> Self {
        if e.is_panic() {
            EngineError::Internal(TASK_PANICKED.into())
        } else {
            EngineError::Internal(format!("engine task cancelled: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dash_sdk::error::StateTransitionBroadcastError;
    use dpp::consensus::ConsensusError;
    use dpp::consensus::codes::ErrorWithCode;
    use dpp::consensus::state::identity::IdentityInsufficientBalanceError;
    use dpp::prelude::Identifier;

    /// A task's panic message never reaches the error (review DW-E0-09 r2
    /// finding 3), and the error says it was a panic.
    #[test]
    fn a_task_panic_withholds_its_message() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let e: EngineError = rt
            .block_on(async {
                tokio::spawn(async {
                    panic!("task panic carrying dashpay://invite?pk=PANIC-SECRET")
                })
                .await
            })
            .unwrap_err()
            .into();
        assert!(e.is_task_panic());
        assert_eq!(e.code(), "internal");
        assert_eq!(e.to_string(), format!("internal error: {TASK_PANICKED}"));
        assert!(!format!("{e:?}").contains("PANIC-SECRET"));
        assert!(!EngineError::Internal("other".into()).is_task_panic());
    }

    const IDENTITY: [u8; 32] = [9u8; 32];

    fn assert_insufficient_credits(e: EngineError, needed: u64, available: u64) {
        assert_eq!(e.code(), "insufficient_credits");
        match e {
            EngineError::InsufficientCredits {
                identity_id,
                needed: n,
                available: a,
            } => {
                assert_eq!(
                    identity_id,
                    Identifier::from(IDENTITY).to_string(Encoding::Base58)
                );
                assert_eq!((n, a), (needed, available));
            }
            other => panic!("expected InsufficientCredits, got {other:?}"),
        }
    }

    #[test]
    fn maps_the_typed_platform_wallet_refusal() {
        let e = PlatformWalletError::InsufficientIdentityCredits {
            identity_id: Identifier::from(IDENTITY),
            required: 25_018_360_000,
            available: 24_818_360_000,
        };
        assert_insufficient_credits(e.into(), 25_018_360_000, 24_818_360_000);
    }

    fn balance_refusal() -> ConsensusError {
        IdentityInsufficientBalanceError::new(
            Identifier::from(IDENTITY),
            24_818_360_000,
            25_018_360_000,
        )
        .into()
    }

    /// The wait-stream shape a rejected broadcast arrives in.
    fn broadcast_refusal() -> dash_sdk::Error {
        let cause = balance_refusal();
        dash_sdk::Error::StateTransitionBroadcastError(StateTransitionBroadcastError {
            code: cause.code(),
            message: cause.to_string(),
            cause: Some(cause),
        })
    }

    #[test]
    fn maps_the_balance_refusal_from_the_sdk() {
        let check_tx = dash_sdk::Error::Protocol(dpp::ProtocolError::ConsensusError(Box::new(
            balance_refusal(),
        )));
        assert_insufficient_credits(check_tx.into(), 25_018_360_000, 24_818_360_000);
        assert_insufficient_credits(broadcast_refusal().into(), 25_018_360_000, 24_818_360_000);
    }

    #[test]
    fn maps_the_balance_refusal_wrapped_by_platform_wallet() {
        let sdk = PlatformWalletError::Sdk(broadcast_refusal());
        assert_insufficient_credits(sdk.into(), 25_018_360_000, 24_818_360_000);
        let token = PlatformWalletError::TokenOperationFailed {
            operation: "transfer",
            source: broadcast_refusal(),
        };
        assert_insufficient_credits(token.into(), 25_018_360_000, 24_818_360_000);
    }

    #[test]
    fn other_sdk_errors_keep_their_codes() {
        let e: EngineError = dash_sdk::Error::Generic("boom".to_string()).into();
        assert_eq!(e.code(), "sdk");
        let e: EngineError =
            PlatformWalletError::Sdk(dash_sdk::Error::Generic("boom".to_string())).into();
        assert_eq!(e.code(), "wallet");
    }
}
