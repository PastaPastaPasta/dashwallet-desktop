//! Compatibility with files and secrets produced by Dash Core / dash-qt.
//!
//! - [`bip39core`]: Core's BIP39 (weak checksum, 256-byte salt cut, no NFKD).
//! - [`dump`]: the `dumpwallet` text format, read and written.
//!
//! Later slices (wallet.dat readers, Core crypter, descriptor export,
//! dash-qt settings import) are owned by WS-04 and not part of this crate yet.

pub mod bip39core;
pub mod dump;
