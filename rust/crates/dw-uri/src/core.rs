//! `dash:` URIs exactly as dash-qt parses and writes them:
//! `GUIUtil::parseBitcoinURI`, `GUIUtil::formatBitcoinURI` and
//! `PaymentServer::handleURIOrFile` (`src/qt/guiutil.cpp`,
//! `src/qt/paymentserver.cpp`). Vectors: `testdata/uri_cases.json`.
//!
//! dash-qt behaviour reproduced here:
//! - The scheme must be `dash` in any case; `dash://` parses (the address is
//!   then empty) but the URI handler rejects it with its own message.
//! - One trailing `/` is removed from the address.
//! - `amount` is always in DASH and parsed with `BitcoinUnits::parse`; an
//!   invalid amount rejects the whole URI, an empty one is ignored.
//! - A `req-` key that is not `label`, `message`, `amount` or `IS` rejects
//!   the URI. Other unknown keys (including `r`) are ignored.
//! - The last occurrence of a repeated key wins.
//! - Percent-decoding follows Qt: see `crate::qurl` for the exact rules
//!   (e.g. `%3F` stays `%3F`, `+` stays `+`).

use crate::keyio;
use crate::qurl;
use dashcore::Network;
use dw_units::{SeparatorStyle, Unit};

/// dash-qt's limit on the length of a URI it renders as a QR code
/// (`MAX_URI_LENGTH` in `src/qt/guiconstants.h`).
pub const MAX_URI_LENGTH: usize = 255;

/// Message dash-qt shows when a URI is longer than `MAX_URI_LENGTH`.
pub const URI_TOO_LONG_MESSAGE: &str =
    "Resulting URI too long, try to reduce the text for label / message.";

/// dash-qt's `SendCoinsRecipient` fields that a `dash:` URI carries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SendCoinsRecipient {
    /// The address text, not validated.
    pub address: String,
    pub label: String,
    pub message: String,
    /// Duffs. dash-qt accepts negative amounts here (`amount=-1`).
    pub amount: i64,
}

/// `GUIUtil::parseBitcoinURI(QString, …)`. `None` is dash-qt's `false`.
pub fn parse_bitcoin_uri(uri: &str) -> Option<SendCoinsRecipient> {
    let url = qurl::parse(uri)?;
    if url.scheme != "dash" {
        return None;
    }
    let mut rv = SendCoinsRecipient {
        address: qurl::path_fully_decoded(url.path),
        ..Default::default()
    };
    if rv.address.ends_with('/') {
        rv.address.pop();
    }
    // Without a `?` there are no query items. An empty query after `?`
    // yields one empty item, which no branch below matches.
    let items = url.query.map(qurl::query_items).unwrap_or_default();
    for (raw_key, value) in items {
        let (key, required) = match raw_key.strip_prefix("req-") {
            Some(k) => (k, true),
            None => (raw_key.as_str(), false),
        };
        match key {
            "label" => rv.label = value,
            "IS" => {}
            "message" => rv.message = value,
            "amount" => {
                if !value.is_empty() {
                    rv.amount = dw_units::parse(Unit::Dash, &value)?;
                }
            }
            _ if required => return None,
            _ => {}
        }
    }
    Some(rv)
}

/// `GUIUtil::formatBitcoinURI`: `dash:<address>[?amount=…][&label=…][&message=…]`.
/// The address is not encoded; label and message are fully percent-encoded;
/// the amount uses 8 decimals with no separators and is omitted when zero.
pub fn format_bitcoin_uri(info: &SendCoinsRecipient) -> String {
    let mut ret = format!("dash:{}", info.address);
    let mut sep = '?';
    if info.amount != 0 {
        ret.push(sep);
        ret.push_str("amount=");
        ret.push_str(&dw_units::format(
            Unit::Dash,
            info.amount,
            false,
            SeparatorStyle::Never,
            false,
        ));
        sep = '&';
    }
    if !info.label.is_empty() {
        ret.push(sep);
        ret.push_str("label=");
        ret.push_str(&qurl::to_percent_encoding(&info.label));
        sep = '&';
    }
    if !info.message.is_empty() {
        ret.push(sep);
        ret.push_str("message=");
        ret.push_str(&qurl::to_percent_encoding(&info.message));
    }
    ret
}

/// Why dash-qt's URI handler did not open the Send page for a URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UriRejection {
    /// The string starts with `dash://` (any case).
    DoubleSlash,
    /// The string does not start with `dash:`; dash-qt would treat it as a
    /// payment request file path.
    NotDashUri,
    /// `parseBitcoinURI` failed.
    Unparsable,
    /// The address is not valid on this network and the URI has an `r`
    /// parameter: a BIP70 payment request, which dash-qt no longer supports.
    Bip70Unsupported,
    /// The address is not valid on this network; Dash Core's reason.
    InvalidAddress(keyio::DestinationError),
}

impl UriRejection {
    /// The text dash-qt shows for this rejection. `None` for `NotDashUri`:
    /// dash-qt shows nothing for such a string unless it names a file.
    pub fn message(&self) -> Option<String> {
        const BIP70: &str = "Cannot process payment request as BIP70 is no longer supported.\nDue to discontinued support, you should request the merchant to provide you with a BIP21 compatible URI or use a wallet that does continue to support BIP70.";
        Some(match self {
            UriRejection::DoubleSlash => "'dash://' is not a valid URI. Use 'dash:' instead.".to_owned(),
            UriRejection::NotDashUri => return None,
            UriRejection::Unparsable => {
                "URI cannot be parsed! This can be caused by an invalid Dash address or malformed URI parameters.".to_owned()
            }
            UriRejection::Bip70Unsupported => BIP70.to_owned(),
            UriRejection::InvalidAddress(e) => e.to_string(),
        })
    }
}

/// `PaymentServer::handleURIOrFile` for a string that is not a file: the
/// recipient to open the Send page with, or the reason dash-qt refuses it.
pub fn handle_uri(s: &str, network: Network) -> Result<SendCoinsRecipient, UriRejection> {
    let starts_with_ci = |prefix: &str| {
        s.get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
    };
    if starts_with_ci("dash://") {
        return Err(UriRejection::DoubleSlash);
    }
    if !starts_with_ci("dash:") {
        return Err(UriRejection::NotDashUri);
    }
    let recipient = parse_bitcoin_uri(s).ok_or(UriRejection::Unparsable)?;
    if let Err(e) = keyio::decode_destination(&recipient.address, network) {
        let has_r = qurl::parse(s)
            .and_then(|u| u.query)
            .is_some_and(|q| qurl::query_items(q).iter().any(|(k, _)| k == "r"));
        return Err(if has_r {
            UriRejection::Bip70Unsupported
        } else {
            UriRejection::InvalidAddress(e)
        });
    }
    Ok(recipient)
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: &str = "XwnLY9Tf7Zsef8gMGL2fhWA9ZmMjt4KPwg";

    #[test]
    fn handler_rejections() {
        assert_eq!(
            handle_uri(&format!("DASH://{X}"), Network::Mainnet),
            Err(UriRejection::DoubleSlash)
        );
        assert_eq!(
            handle_uri(&format!("pay:{X}"), Network::Mainnet),
            Err(UriRejection::NotDashUri)
        );
        assert_eq!(
            handle_uri(&format!("dash:{X}?req-x=1"), Network::Mainnet),
            Err(UriRejection::Unparsable)
        );
        assert_eq!(
            handle_uri("dash:?r=https%3A%2F%2Fexample.com", Network::Mainnet),
            Err(UriRejection::Bip70Unsupported)
        );
        assert!(matches!(
            handle_uri(&format!("dash:{X}"), Network::Testnet),
            Err(UriRejection::InvalidAddress(_))
        ));
        assert_eq!(
            handle_uri(&format!("dash:{X}?amount=1&r=x"), Network::Mainnet)
                .unwrap()
                .amount,
            100_000_000
        );
    }

    #[test]
    fn no_query_vs_empty_query() {
        assert_eq!(parse_bitcoin_uri(&format!("dash:{X}")).unwrap().address, X);
        assert_eq!(parse_bitcoin_uri(&format!("dash:{X}?")).unwrap().address, X);
    }
}
