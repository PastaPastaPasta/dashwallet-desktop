//! DashPay and Platform glue over platform-wallet (DASHPAY §3.1). The
//! library owns the protocol; this module supplies what it asks the host
//! for: signers over the vault, bring-up order, read models and flows.

pub mod signers;

pub use signers::{VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
