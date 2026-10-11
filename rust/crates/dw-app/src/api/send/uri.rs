//! `dash:` URIs, address classification and QR matrices (dw-uri; QT-054,
//! QT-084/085, QT-149, IOS-042, IOS-048).
//!
//! A string that is not a payment URI is an answer, not a failure, so
//! `parse_payment_uri` returns it as data (`UriParse::Rejected`) and the
//! renderer picks its copy by the reason.

use dw_uri::core::{SendCoinsRecipient, UriRejection, format_bitcoin_uri, handle_uri};
use dw_uri::keyio::{self, AddressKind, Destination, DestinationError};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::dto::parse_network;
use crate::error::{AppError, UriCode};

/// Dash Core `MAX_MONEY`: 21 million DASH in duffs.
pub const MAX_MONEY: u64 = 21_000_000 * 100_000_000;

/// Why a string is not a Dash Core address on the network (Core's
/// `DecodeDestination` error cases).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AddressProblem {
    InvalidBase58Length,
    InvalidBase58Prefix,
    NotBech32mOrBase58,
    InvalidBase58ChecksumOrLength,
    /// A valid Platform (DIP-18) address: not payable on L1 (QT-067).
    PlatformAddress,
    /// A bech32 problem; `detail` is Core's diagnostic text.
    Bech32 { detail: String },
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AddressClass {
    /// A Base58 L1 address; `scriptHash` for P2SH.
    Core { script_hash: bool },
    /// A DIP-18 Platform address.
    Platform,
    /// An Orchard shielded address.
    Shielded,
    Invalid { problem: AddressProblem },
}

/// A `dash:` URI as dash-qt reads it (`GUIUtil::parseBitcoinURI`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PaymentUri {
    /// A valid L1 address on the network.
    pub address: String,
    /// Duffs; `null` when absent or zero.
    pub amount: Option<u64>,
    pub label: Option<String>,
    pub message: Option<String>,
}

/// Why dash-qt's URI handler did not accept a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UriRejected {
    /// `dash://...`, which dash-qt refuses.
    DoubleSlash,
    /// Does not start with `dash:`.
    NotDashUri,
    /// Malformed parameters or an unknown required (`req-`) key.
    Unparsable,
    /// `r=` with an invalid address: a BIP70 payment request.
    Bip70Unsupported,
    InvalidAddress { problem: AddressProblem },
    /// Negative or above the maximum supply.
    InvalidAmount,
}

/// The answer of `parse_payment_uri`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum UriParse {
    Accepted { uri: PaymentUri },
    Rejected { reason: UriRejected },
}

/// QR code modules, row-major: `modules[y * size + x]`, `true` = dark.
/// Error correction level L; no quiet zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QrMatrix {
    pub size: u32,
    pub modules: Vec<bool>,
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Parses `text` for `network` (a directory name) as dash-qt's URI handler
/// does (QT-054, QT-149).
pub fn parse_payment_uri(network: &str, text: &str) -> Result<UriParse, AppError> {
    let network = parse_network(network)?.core_network();
    let rejected = |reason| Ok(UriParse::Rejected { reason });
    let r = match handle_uri(text, network) {
        Ok(r) => r,
        Err(e) => {
            return rejected(match e {
                UriRejection::DoubleSlash => UriRejected::DoubleSlash,
                UriRejection::NotDashUri => UriRejected::NotDashUri,
                UriRejection::Unparsable => UriRejected::Unparsable,
                UriRejection::Bip70Unsupported => UriRejected::Bip70Unsupported,
                UriRejection::InvalidAddress(p) => UriRejected::InvalidAddress { problem: p.into() },
            });
        }
    };
    // dash-qt accepts a negative amount and fails later, at send.
    let Some(amount) = u64::try_from(r.amount).ok().filter(|a| *a <= MAX_MONEY) else {
        return rejected(UriRejected::InvalidAmount);
    };
    Ok(UriParse::Accepted {
        uri: PaymentUri {
            address: r.address,
            amount: (amount > 0).then_some(amount),
            label: non_empty(r.label),
            message: non_empty(r.message),
        },
    })
}

/// Writes a `dash:` URI as dash-qt's `formatBitcoinURI` (QT-085). The
/// address is not validated.
pub fn build_payment_uri(
    address: String,
    amount: Option<u64>,
    label: Option<String>,
    message: Option<String>,
) -> Result<String, AppError> {
    let amount = amount.unwrap_or(0);
    if amount > MAX_MONEY {
        return Err(AppError::invalid_argument(format!(
            "amount {amount} is above the maximum supply"
        )));
    }
    let amount = i64::try_from(amount)
        .map_err(|_| AppError::invalid_argument("amount does not fit"))?;
    Ok(format_bitcoin_uri(&SendCoinsRecipient {
        address,
        label: label.unwrap_or_default(),
        message: message.unwrap_or_default(),
        amount,
    }))
}

/// Classifies a pasted or scanned destination for `network` (whitespace
/// trimmed; mixed-case bech32 rejected as Core does).
pub fn classify_address(network: &str, text: &str) -> Result<AddressClass, AppError> {
    let network = parse_network(network)?.core_network();
    Ok(match keyio::classify_address(text, network) {
        AddressKind::Core(Destination::PubKeyHash(_)) => AddressClass::Core { script_hash: false },
        AddressKind::Core(Destination::ScriptHash(_)) => AddressClass::Core { script_hash: true },
        AddressKind::Platform(_) => AddressClass::Platform,
        AddressKind::Shielded(_) => AddressClass::Shielded,
        AddressKind::Invalid(e) => AddressClass::Invalid { problem: e.into() },
    })
}

/// The QR module matrix for `text`, as dash-qt encodes it (QT-084: byte
/// mode, ECC L, at most 255 characters).
pub fn qr_matrix(text: &str) -> Result<QrMatrix, AppError> {
    let m = dw_uri::qr::qr_matrix(text).map_err(|e| match e {
        dw_uri::qr::QrError::TooLong { len } => AppError {
            code: UriCode::TooLongForQr.into(),
            message: format!("text is {len} characters; a QR code holds at most 255"),
            data: None,
        },
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
    fn a_uri_round_trips_and_rejections_are_data() {
        let uri =
            build_payment_uri(ADDR.into(), Some(150_000_000), Some("a b".into()), None).unwrap();
        assert_eq!(uri, format!("dash:{ADDR}?amount=1.50000000&label=a%20b"));
        let UriParse::Accepted { uri: parsed } = parse_payment_uri("mainnet", &uri).unwrap()
        else {
            panic!("accepted");
        };
        assert_eq!(parsed.address, ADDR);
        assert_eq!(parsed.amount, Some(150_000_000));
        assert_eq!(parsed.label.as_deref(), Some("a b"));
        assert_eq!(parsed.message, None);

        let reason = |text: String| match parse_payment_uri("mainnet", &text).unwrap() {
            UriParse::Rejected { reason } => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(reason(format!("dash://{ADDR}")), UriRejected::DoubleSlash);
        assert_eq!(reason(ADDR.into()), UriRejected::NotDashUri);
        assert_eq!(
            reason(format!("dash:{ADDR}?amount=-1")),
            UriRejected::InvalidAmount
        );
        assert_eq!(
            reason(format!("dash:{ADDR}?amount=21000000.00000001")),
            UriRejected::InvalidAmount
        );
    }

    #[test]
    fn the_address_is_checked_against_the_network() {
        match parse_payment_uri("testnet", &format!("dash:{ADDR}")).unwrap() {
            UriParse::Rejected {
                reason: UriRejected::InvalidAddress { .. },
            } => {}
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify_address("mainnet", ADDR).unwrap(),
            AddressClass::Core { script_hash: false }
        ));
        assert!(matches!(
            classify_address("testnet", ADDR).unwrap(),
            AddressClass::Invalid { .. }
        ));
        assert_eq!(
            parse_payment_uri("moon", ADDR).unwrap_err().code_str(),
            "invalid_argument"
        );
    }

    #[test]
    fn amounts_above_the_supply_cannot_be_built() {
        assert!(build_payment_uri(ADDR.into(), Some(MAX_MONEY), None, None).is_ok());
        assert_eq!(
            build_payment_uri(ADDR.into(), Some(MAX_MONEY + 1), None, None)
                .unwrap_err()
                .code_str(),
            "invalid_argument"
        );
    }

    #[test]
    fn the_qr_matrix_is_square_and_length_limited() {
        let m = qr_matrix(&format!("dash:{ADDR}")).unwrap();
        assert_eq!(m.modules.len(), (m.size * m.size) as usize);
        assert_eq!(
            qr_matrix(&"x".repeat(256)).unwrap_err().code_str(),
            "uri.too_long_for_qr"
        );
    }

    #[test]
    fn rejections_serialise_with_a_kind() {
        let json = serde_json::to_value(UriParse::Rejected {
            reason: UriRejected::InvalidAddress {
                problem: AddressProblem::Bech32 { detail: "x".into() },
            },
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "rejected",
                "reason": { "kind": "invalidAddress", "problem": { "kind": "bech32", "detail": "x" } }
            })
        );
    }
}
