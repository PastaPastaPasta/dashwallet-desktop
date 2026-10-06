//! Compatibility with files and secrets produced by Dash Core / dash-qt.
//!
//! - [`bip39core`]: Core's BIP39 (weak checksum, 256-byte salt cut, no NFKD).
//! - [`dump`]: the `dumpwallet` text format, read and written.
//! - [`walletdat`]: SQLite descriptor `wallet.dat` reader (Berkeley DB files
//!   are recognised only; their reader is M6).
//! - [`crypter`]: Core's wallet encryption (`mkey`, encrypted keys and
//!   mnemonics).
//! - [`descriptor`]: descriptor checksums, `pkh` descriptors over a master
//!   key, `listdescriptors` / `importdescriptors` JSON.

pub mod bip39core;
pub mod crypter;
pub mod descriptor;
pub mod dump;
pub mod walletdat;
