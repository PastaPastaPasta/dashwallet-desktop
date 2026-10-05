//! `dash:` URIs, deep links and address classification for the desktop
//! wallet.
//!
//! - [`core`]: dash-qt's exact parser and writer (`GUIUtil::parseBitcoinURI`,
//!   `formatBitcoinURI`, `PaymentServer::handleURIOrFile`).
//! - [`ext`]: the iOS wallet's wider payment-string parser and URI writer.
//! - [`deeplink`]: routing of opened or scanned links.
//! - [`keyio`]: Dash Core's Base58 address and WIF decoding, plus Platform
//!   and shielded address classification.
//!
//! Golden vectors: `testdata/uri_cases.json` (dash-qt code run against
//! Qt 5.15) and Dash Core's `key_io_valid.json` / `key_io_invalid.json`
//! (copied to `testdata/core/`).

mod bech32_locate;
pub mod core;
pub mod deeplink;
pub mod ext;
pub mod keyio;
mod qurl;

pub use dashcore::Network;
