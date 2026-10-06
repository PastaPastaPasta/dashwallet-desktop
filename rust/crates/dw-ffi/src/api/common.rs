//! Types and helpers shared by the M1 domain modules (docs/contracts/m1-engine.md).
//!
//! Conventions used across the surface:
//! - Wallet ids are 64-char lowercase hex strings (`WalletId` in dw-engine).
//! - Txids are 64-char lowercase hex in display (RPC) byte order.
//! - Amounts are duffs: `u64` for quantities, `i64` for signed net amounts.
//! - Times are UNIX seconds (`u64`). `None` means "not known", never zero.
//! - Secrets cross as `Vec<u8>` (Swift `Data`); see `vault.rs`.

/// A transaction output reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct OutPoint {
    /// 64-char lowercase hex, display byte order.
    pub txid: String,
    pub vout: u32,
}

impl OutPoint {
    /// The engine outpoint; the txid must be 64 lowercase hex characters.
    pub(crate) fn to_core(&self) -> Result<dashcore::OutPoint, dw_engine::EngineError> {
        let lower_hex = self.txid.len() == 64
            && self
                .txid
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        let txid = lower_hex
            .then(|| self.txid.parse::<dashcore::Txid>().ok())
            .flatten()
            .ok_or_else(|| {
                dw_engine::EngineError::InvalidArgument(format!(
                    "txid {:?} is not 64 lowercase hex",
                    self.txid
                ))
            })?;
        Ok(dashcore::OutPoint::new(txid, self.vout))
    }
}

impl From<dashcore::OutPoint> for OutPoint {
    fn from(o: dashcore::OutPoint) -> Self {
        Self {
            txid: o.txid.to_string(),
            vout: o.vout,
        }
    }
}

/// Builds the typed "not implemented" case of a domain error. Every domain
/// error implements it, so a contract call that has no engine behaviour yet
/// fails with a value the UI can recognise instead of pretending to succeed.
/// No M1 call needs it after E1 and E2; it stays for later stub calls.
#[allow(dead_code)]
pub(crate) trait NotImplementedError: Sized {
    fn not_implemented(call: &'static str) -> Self;
}

/// `Err(E::NotImplemented { call })` for a contract call with no engine
/// behaviour yet. `call` is `"<Object>.<method>"` or the free function name.
#[allow(dead_code)]
pub(crate) fn not_implemented<T, E: NotImplementedError>(call: &'static str) -> Result<T, E> {
    Err(E::not_implemented(call))
}

/// Implements `NotImplementedError` and the common `From<dw_engine::EngineError>`
/// mapping for a domain error that has the six common variants
/// (`InvalidArgument`, `NetworkNotOpen`, `WalletNotFound`, `Storage`,
/// `NotImplemented`, `Internal`). Engine errors without a common counterpart
/// become `Internal`; domains that need finer mapping write their own `From`.
macro_rules! domain_error_common {
    ($name:ident) => {
        domain_error_common!(@not_implemented $name);

        impl From<dw_engine::EngineError> for $name {
            fn from(e: dw_engine::EngineError) -> Self {
                use dw_engine::EngineError as E;
                let detail = e.to_string();
                match e {
                    E::InvalidConfig(_) | E::InvalidArgument(_) => Self::InvalidArgument { detail },
                    E::NetworkNotOpen(_) => Self::NetworkNotOpen { detail },
                    E::WalletNotFound(_) => Self::WalletNotFound { detail },
                    E::StorageInUse(_) | E::Storage(_) | E::Io(_) => Self::Storage { detail },
                    E::NotImplemented(call) => Self::NotImplemented { call },
                    _ => Self::Internal { detail },
                }
            }
        }
    };
    (@not_implemented $name:ident) => {
        impl $crate::api::common::NotImplementedError for $name {
            fn not_implemented(call: &'static str) -> Self {
                Self::NotImplemented {
                    call: call.to_string(),
                }
            }
        }
    };
}

pub(crate) use domain_error_common;

/// Exports a domain error's stable code (docs/contracts/m1-engine.md §4) as
/// a `code()` method on the generated host type, so hosts can read the code
/// instead of keeping their own copy of the strings. The error type defines
/// `fn code_str(&self) -> &'static str`.
macro_rules! export_error_code {
    ($name:ident) => {
        #[uniffi::export]
        impl $name {
            /// Stable code (docs/contracts/m1-engine.md §4).
            pub fn code(&self) -> String {
                self.code_str().to_string()
            }
        }
    };
}

pub(crate) use export_error_code;

/// Parses a wallet id argument: 64 lower-case hex characters.
pub(crate) fn parse_wallet_id(
    wallet_id: &str,
) -> Result<dw_engine::WalletId, dw_engine::EngineError> {
    wallet_id.parse()
}
