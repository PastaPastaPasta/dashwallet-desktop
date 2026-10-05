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

mod context;
mod engine;
mod error;
mod events;
// E1 building blocks (sync snapshot, event pump, operation gate, private
// directories) that NetworkSession does not use yet. Declared so they compile
// and their unit tests run; the allow goes when E1 wires them in.
#[allow(dead_code)]
mod fsutil;
#[allow(dead_code)]
mod gate;
mod keys;
mod network;
#[allow(dead_code)]
mod pump;
mod session;
#[allow(dead_code)]
mod sync;

pub use engine::{Engine, EngineConfig};
pub use error::EngineError;
pub use events::{EngineEvent, EventSink, NoticeCode};
pub use keys::ImportOptions;
pub use network::DashNetwork;
pub use session::{
    CreatedWallet, NetworkSession, SessionOptions, WalletBalances, WalletId, WalletSummary,
};
