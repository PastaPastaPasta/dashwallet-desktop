//! DashPay and Platform glue over platform-wallet (DASHPAY §3.1). The
//! library owns the protocol; this module supplies what it asks the host
//! for: signers over the vault, bring-up order, read models and flows.
//! [`DashPay`] is the facade the UI binding and `dwcli` call.

mod dashpay;
mod errors;
pub mod keys_policy;
mod records;
pub mod signers;
mod status;

pub use dashpay::{DashPay, check_username};
pub use errors::{
    AvatarError, ContactError, CreditsError, IdentityError, InvitationError, NameError,
    PlatformError, RegistrationError,
};
pub use records::*;
pub use signers::{VaultContactCrypto, VaultIdentitySigner, VaultScanKey};
pub use status::PlatformStatus;
