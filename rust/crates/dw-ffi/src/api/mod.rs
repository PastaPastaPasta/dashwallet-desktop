//! Domain modules of the FFI surface. Append-only list. The M1 contract is
//! documented in docs/contracts/m1-engine.md, the M2 additions in
//! docs/contracts/m2-engine.md.

pub mod coins;
pub mod common;
pub mod engine;
pub mod error;
pub mod history;
pub mod labels;
pub mod message;
pub mod receive;
pub mod send;
#[cfg(test)]
mod send_tests;
pub mod session;
pub mod sync;
pub mod units;
pub mod uri;
pub mod vault;
pub mod wallet;
// M2 (docs/contracts/m2-engine.md).
pub mod backup;
pub mod compat;
pub mod console;
pub mod desktop;
pub mod fees;
#[cfg(test)]
mod m2_tests;
pub mod multiwallet;
pub mod psbt;
#[cfg(test)]
mod r2_tests;
pub mod tools;
pub mod tx_actions;
pub mod vault_m2;

pub use backup::*;
pub use coins::*;
pub use common::OutPoint;
pub use compat::*;
pub use console::*;
pub use desktop::*;
pub use engine::*;
pub use error::*;
pub use fees::*;
pub use history::*;
pub use labels::*;
pub use message::*;
pub use multiwallet::*;
pub use psbt::*;
pub use receive::*;
pub use send::*;
pub use session::*;
pub use sync::*;
pub use tools::*;
pub use tx_actions::*;
pub use units::*;
pub use uri::*;
pub use vault::*;
pub use vault_m2::*;
pub use wallet::*;
