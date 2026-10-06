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

mod coins;
mod context;
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
mod network;
mod pump;
mod receive;
pub(crate) mod send;
mod session;
pub mod sync;
mod wallets;

pub use coins::{CoinFilter, CoinInfo, DUST_PROTECTION_MAX};
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
pub use network::DashNetwork;
pub use receive::{AddressFilter, AddressInfo, ReceiveRequest};
pub use send::{
    BroadcastOutcome, ChangePolicy, CoinSource, FeeMode, MAX_TX_FEE, PreparedInput, PreparedOutput,
    PreparedSummary, PreparedTx, Recipient, SendFailure, TxDraft, TxEstimate,
};
pub use session::{CreatedWallet, NetworkSession, SessionOptions, WalletBalances, WalletId};
pub use sync::{PeerInfo, RescanFrom, SyncPhase, SyncPhaseProgress, SyncSnapshot};
pub use wallets::{MAX_WALLET_NAME, WalletInfo};
