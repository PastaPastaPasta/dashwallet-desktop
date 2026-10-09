//! dashwallet-desktop engine.
//!
//! [`Engine`] owns the tokio runtime and the per-network [`NetworkSession`]s.
//! A session wraps one `platform_wallet::PlatformWalletManager` backed by
//! `platform_wallet_storage::SqlitePersister` (`<data_root>/<network>/wallet.sqlite`)
//! and a `dash_sdk::Sdk` with a trusted HTTPS quorum context provider, the same
//! trust model iOS uses (DESIGN.md R2, "Platform proof trust").
//!
//! Each session also owns the network's dw-vault [`dw_vault::Vault`]
//! (`<data_root>/<network>/vault`). Wallets are created and imported through
//! it (seed stored and read back before registration), and signing goes
//! through its `VaultSigner`.
//!
//! Every async method spawns its work onto the engine's runtime and awaits the
//! join handle, so callers may poll these futures from any executor (UniFFI's
//! foreign executor, `block_on`, another tokio runtime).

pub mod backup;
pub mod coinjoin;
mod coins;
pub mod compat;
mod context;
pub mod csv_export;
mod engine;
mod error;
mod events;
mod fees;
mod fsutil;
mod gate;
pub mod history;
mod history_ops;
mod keys;
mod labels;
pub mod masternode_keys;
mod multiwallet;
mod network;
pub mod platform;
mod pump;
mod receive;
pub(crate) mod send;
mod session;
mod store;
pub mod sync;
mod tools;
mod tx_actions;
mod wallets;

pub use backup::{BackupFailure, BackupInfo, BackupPolicy};
pub use coinjoin::CoinJoinFailure;
pub use coins::{CoinFilter, CoinInfo, DUST_PROTECTION_MAX};
pub use compat::{
    CompatFailure, CoreExportFormat, CoreMnemonicCompatibility, ExportReport, ExportWarning,
    ImportReport, KeyMaterial, WalletFileKind, inspect_wallet_file,
};
pub use dw_appdb::BookPurpose;
pub use engine::{Engine, EngineConfig};
pub use error::EngineError;
pub use events::{EngineEvent, EventSink, NoticeCode};
pub use fees::{
    CoinSelectionSummary, FEE_TARGETS, FeePolicy, FeeSource, FeeTarget, MAX_BROADCAST_RATE_PER_KB,
    fee_policy,
};
pub use history::{
    AddressChain, HistoryFilter, HistoryPage, HistoryQuery, HistorySort, TxCategory, TxDetail,
    TxInputDetail, TxOutputDetail, TxRecord, TxStatus, TxStatusKind, TxType, WatchOnlyFilter,
};
pub use keys::{CORE_COMPAT_LOOKAHEAD, ImportOptions, MAX_LOOKAHEAD};
pub use labels::{BookEntryInfo, LabelsFailure};
pub use masternode_keys::{
    MasternodeFailure, MasternodeKeyInfo, MasternodeKeyRole, RevealedMasternodeKey,
};
pub use multiwallet::{AccountXpub, NetworkDataInfo, WalletLoadState, WatchOnlyOptions};
pub use network::DashNetwork;
pub use platform::{
    CREATED_HERE_BUDGET, DashPayStartup, PlatformCadence, PlatformStatus, SpvState, StartupStatus,
    SyncLoop, SyncLoopStatus,
};
pub use receive::{AddressFilter, AddressInfo, ReceiveRequest};
pub use send::psbt::{PsbtAnalysis, PsbtFailure, PsbtOutputInfo, PsbtSignability};
pub use send::{
    BroadcastOutcome, ChangePolicy, CoinSource, FeeMode, MAX_TX_FEE, PreparedInput, PreparedOutput,
    PreparedSummary, PreparedTx, Recipient, SendFailure, TxDraft, TxEstimate,
};
pub use session::{CreatedWallet, NetworkSession, SessionOptions, WalletBalances, WalletId};
pub use sync::{PeerInfo, RescanFrom, SyncPhase, SyncPhaseProgress, SyncSnapshot};
pub use tools::{
    ChainLockInfo, ENGINE_VERSION, EngineWarning, MasternodeCount, NodeInfo, RescanProgress,
    USER_AGENT, WarningCode,
};
pub use tx_actions::{TxActionRefusal, TxDetailExtras, TxNotice, is_coinjoin_internal};
pub use wallets::{MAX_WALLET_NAME, WalletInfo};
