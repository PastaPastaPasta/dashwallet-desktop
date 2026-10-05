//! dashwallet-desktop engine.
//!
//! [`Engine`] owns the tokio runtime and the per-network [`NetworkSession`]s.
//! A session wraps one `platform_wallet::PlatformWalletManager` backed by
//! `platform_wallet_storage::SqlitePersister` (`<data_root>/<network>/wallet.sqlite`)
//! and a `dash_sdk::Sdk` with a trusted HTTPS quorum context provider, the same
//! trust model iOS uses (DESIGN.md R2, "Platform proof trust").
//!
//! Every async method spawns its work onto the engine's runtime and awaits the
//! join handle, so callers may poll these futures from any executor (UniFFI's
//! foreign executor, `block_on`, another tokio runtime).

mod context;
mod engine;
mod error;
mod events;
mod network;
mod session;

pub use engine::{Engine, EngineConfig};
pub use error::EngineError;
pub use events::{EngineEvent, EventSink, NoticeCode};
pub use network::DashNetwork;
pub use session::{
    CreatedWallet, NetworkSession, SessionOptions, WalletBalances, WalletId, WalletSummary,
};
