//! `dash:` URIs, address classification and QR matrices (dw-uri; QT-054,
//! QT-084/085, QT-149, IOS-042, IOS-048). Owner: E2. Pure functions.
//! Contract: docs/contracts/m1-engine.md §uri.

use dw_uri::core::{SendCoinsRecipient, UriRejection, format_bitcoin_uri, handle_uri};
use dw_uri::keyio::{self, AddressKind, Destination, DestinationError};

use crate::DashNetwork;
use crate::api::common::domain_error_common;

/// Why a string is not a Dash Core address on the network (Core's
/// `DecodeDestination` error cases).
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AddressProblem {
    InvalidBase58Length,
    InvalidBase58Prefix,
    NotBech32mOrBase58,
    InvalidBase58ChecksumOrLength,
    /// A valid Platform (DIP-18) address: not payable on L1 (QT-067).
    PlatformAddress,
    /// A bech32 problem; `detail` is Core's diagnostic text.
    Bech32 {
        detail: String,
    },
}

impl From<DestinationError> for AddressProblem {
    fn from(e: DestinationError) -> Self {
        match e {
            DestinationError::InvalidBase58Length => Self::InvalidBase58Length,
            DestinationError::InvalidBase58Prefix => Self::InvalidBase58Prefix,
            DestinationError::NotBech32mOrBase58 => Self::NotBech32mOrBase58,
            DestinationError::InvalidBase58ChecksumOrLength => Self::InvalidBase58ChecksumOrLength,
            DestinationError::PlatformAddress => Self::PlatformAddress,
            DestinationError::Bech32(msg) => Self::Bech32 {
                detail: msg.to_string(),
            },
        }
    }
}

/// What a pasted or scanned destination is (IOS-042).
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum AddressClass {
    /// Base58 L1 address; `script_hash` = P2SH.
    Core {
        script_hash: bool,
    },
    /// DIP-18 Platform address (M4 routes).
    Platform,
    /// Orchard shielded address (M4 routes).
    Shielded,
    Invalid {
        problem: AddressProblem,
    },
}

/// A `dash:` URI as dash-qt reads it (`GUIUtil::parseBitcoinURI`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PaymentUri {
    /// Valid L1 address on the network.
    pub address: String,
    /// Duffs; `None` when absent or zero.
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
}

/// QR code modules, row-major: `modules[y * size + x]`, `true` = dark.
/// Error correction level L; no quiet zone.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QrMatrix {
    pub size: u32,
    pub modules: Vec<bool>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum UriError {
    /// Code `uri.double_slash`: `dash://…` (dash-qt refuses it).
    #[error("dash:// is not a valid URI")]
    DoubleSlash,
    /// Code `uri.not_dash_uri`: does not start with `dash:`.
    #[error("not a dash: URI")]
    NotDashUri,
    /// Code `uri.unparsable`: malformed parameters or a required (`req-`) key.
    #[error("URI cannot be parsed")]
    Unparsable,
    /// Code `uri.bip70_unsupported`: `r=` with an invalid address.
    #[error("BIP70 payment requests are not supported")]
    Bip70Unsupported,
    /// Code `uri.invalid_address`.
    #[error("invalid address")]
    InvalidAddress { problem: AddressProblem },
    /// Code `uri.invalid_amount`: negative or above the maximum supply.
    #[error("invalid amount")]
    InvalidAmount,
    /// Code `uri.too_long_for_qr`: longer than 255 characters (QT-084).
    #[error("too long for a QR code")]
    TooLongForQr,
    /// Code `invalid_argument`.
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    /// Code `not_implemented`.
    #[error("not implemented: {call}")]
    NotImplemented { call: String },
}

domain_error_common!(@not_implemented UriError);

impl UriError {
    /// Stable code (docs/contracts/m1-engine.md "Error codes").
    pub fn code(&self) -> &'static str {
        match self {
            Self::DoubleSlash => "uri.double_slash",
            Self::NotDashUri => "uri.not_dash_uri",
            Self::Unparsable => "uri.unparsable",
            Self::Bip70Unsupported => "uri.bip70_unsupported",
            Self::InvalidAddress { .. } => "uri.invalid_address",
            Self::InvalidAmount => "uri.invalid_amount",
            Self::TooLongForQr => "uri.too_long_for_qr",
            Self::InvalidArgument { .. } => "invalid_argument",
            Self::NotImplemented { .. } => "not_implemented",
        }
    }
}

fn core_network(network: DashNetwork) -> dw_uri::Network {
    dw_engine::DashNetwork::from(network).core_network()
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Parses a `dash:` URI the way dash-qt's URI handler does and checks the
/// address against `network` (QT-054, QT-149). A negative amount, which
/// dash-qt accepts and then fails to send, is rejected as `InvalidAmount`.
#[uniffi::export]
pub fn parse_payment_uri(network: DashNetwork, text: String) -> Result<PaymentUri, UriError> {
    let r = handle_uri(&text, core_network(network)).map_err(|e| match e {
        UriRejection::DoubleSlash => UriError::DoubleSlash,
        UriRejection::NotDashUri => UriError::NotDashUri,
        UriRejection::Unparsable => UriError::Unparsable,
        UriRejection::Bip70Unsupported => UriError::Bip70Unsupported,
        UriRejection::InvalidAddress(p) => UriError::InvalidAddress { problem: p.into() },
    })?;
    let amount = u64::try_from(r.amount).map_err(|_| UriError::InvalidAmount)?;
    Ok(PaymentUri {
        address: r.address,
        amount: (amount > 0).then_some(amount),
        label: non_empty(r.label),
        message: non_empty(r.message),
    })
}

/// Writes a `dash:` URI exactly as dash-qt's `formatBitcoinURI` (QT-085).
/// The address is not validated.
#[uniffi::export]
pub fn build_payment_uri(
    address: String,
    amount: Option<u64>,
    label: Option<String>,
    message: Option<String>,
) -> Result<String, UriError> {
    let amount = i64::try_from(amount.unwrap_or(0)).map_err(|_| UriError::InvalidAmount)?;
    Ok(format_bitcoin_uri(&SendCoinsRecipient {
        address,
        label: label.unwrap_or_default(),
        message: message.unwrap_or_default(),
        amount,
    }))
}

/// Classifies a pasted or scanned destination for `network` (whitespace
/// trimmed; mixed-case bech32 rejected as Core does).
#[uniffi::export]
pub fn classify_address(network: DashNetwork, text: String) -> AddressClass {
    match keyio::classify_address(&text, core_network(network)) {
        AddressKind::Core(Destination::PubKeyHash(_)) => AddressClass::Core { script_hash: false },
        AddressKind::Core(Destination::ScriptHash(_)) => AddressClass::Core { script_hash: true },
        AddressKind::Platform(_) => AddressClass::Platform,
        AddressKind::Shielded(_) => AddressClass::Shielded,
        AddressKind::Invalid(e) => AddressClass::Invalid { problem: e.into() },
    }
}

/// QR module matrix for `text`, encoded as dash-qt encodes it (QT-084: byte
/// mode, ECC L, at most 255 characters). The host renders the modules; no
/// image crosses the FFI.
#[uniffi::export]
pub fn qr_matrix(text: String) -> Result<QrMatrix, UriError> {
    let m = dw_uri::qr::qr_matrix(&text).map_err(|e| match e {
        dw_uri::qr::QrError::TooLong { .. } => UriError::TooLongForQr,
    })?;
    Ok(QrMatrix {
        size: m.size,
        modules: m.modules,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg";

    #[test]
    fn uri_round_trip_and_classification() {
        let uri =
            build_payment_uri(ADDR.into(), Some(150_000_000), Some("a b".into()), None).unwrap();
        assert_eq!(uri, format!("dash:{ADDR}?amount=1.50000000&label=a%20b"));
        let parsed = parse_payment_uri(DashNetwork::Mainnet, uri).unwrap();
        assert_eq!(parsed.address, ADDR);
        assert_eq!(parsed.amount, Some(150_000_000));
        assert_eq!(parsed.label.as_deref(), Some("a b"));
        assert_eq!(parsed.message, None);
        assert!(matches!(
            parse_payment_uri(DashNetwork::Mainnet, format!("dash://{ADDR}")),
            Err(UriError::DoubleSlash)
        ));
        assert!(matches!(
            parse_payment_uri(DashNetwork::Mainnet, format!("dash:{ADDR}?amount=-1")),
            Err(UriError::InvalidAmount)
        ));
        assert_eq!(
            classify_address(DashNetwork::Mainnet, ADDR.into()),
            AddressClass::Core { script_hash: false }
        );
        assert!(matches!(
            classify_address(DashNetwork::Testnet, ADDR.into()),
            AddressClass::Invalid { .. }
        ));
    }

    #[test]
    fn qr_matrix_maps_the_length_limit() {
        let m = qr_matrix(format!("dash:{ADDR}")).unwrap();
        assert_eq!(m.modules.len(), (m.size * m.size) as usize);
        let e = qr_matrix("x".repeat(256)).unwrap_err();
        assert_eq!(e.code(), "uri.too_long_for_qr");
    }
}
