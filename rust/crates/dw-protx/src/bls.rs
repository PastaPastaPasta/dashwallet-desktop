//! Operator BLS *public* keys (BLS12-381 G1) as dash-qt's Register wizard
//! reads them (`src/qt/masternodewizard.cpp`). The operator secret lives on
//! the masternode, never in the wallet (MG-01 D6): this module parses and
//! re-serializes public keys only.
//!
//! Public keys are 48 bytes, in the basic (modern, IETF) serialization that
//! version-2 payloads carry; the legacy serialization of the same point is
//! what version-1 masternodes registered with.

use dashcore::bls_sig_utils::{BlsPkBytes, BlsScheme};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlsError {
    #[error("not 96 hexadecimal characters")]
    PublicHex,
    #[error("not a BLS12-381 public key")]
    PublicPoint,
}

/// Parses an operator public key typed in the wizard: 96 hex characters
/// that decode to a G1 point in the basic serialization (dash-qt refuses
/// legacy keys for new registrations).
pub fn parse_public_hex(text: &str) -> Result<[u8; 48], BlsError> {
    let mut out = [0u8; 48];
    hex::decode_to_slice(text.trim(), &mut out).map_err(|_| BlsError::PublicHex)?;
    BlsPkBytes::from_bytes(out)
        .as_scheme(BlsScheme::Modern)
        .canonicalize()
        .map_err(|_| BlsError::PublicPoint)?;
    Ok(out)
}

/// Whether Core's `CBLSLazyPublicKey::Get()` reads `key` as a valid key in
/// the basic serialization (rc.3 `bls/bls.h:100-122,492-511`): a G1 point
/// (`SetBytes`; the decode refuses the point at infinity, which Core's
/// `impl == ImplType()` resets) that serializes back to the same bytes
/// (`CheckMalleable`).
pub fn is_valid_basic_public(key: &[u8; 48]) -> bool {
    BlsPkBytes::from_bytes(*key)
        .as_scheme(BlsScheme::Modern)
        .canonicalize()
        .is_ok_and(|pk| pk.as_bytes() == key)
}

/// The legacy serialization of a basic-serialized public key (for matching
/// a list entry registered before v19).
pub fn legacy_of(basic: &[u8; 48]) -> Option<[u8; 48]> {
    BlsPkBytes::from_bytes(*basic)
        .as_scheme(BlsScheme::Modern)
        .reencode(BlsScheme::Legacy)
        .ok()
        .map(|pk| *pk.as_bytes())
}

/// The basic serialization of a listed operator key, whichever form the
/// list holds it in: a version-1 entry's legacy bytes are re-encoded, as
/// Core's `CBLSLazyPublicKey` re-serializes a kept key at the payload's
/// version (`providertx_service.cpp:902-907`). `None` if neither parses.
pub fn basic_form(listed: &[u8; 48]) -> Option<[u8; 48]> {
    let pk = BlsPkBytes::from_bytes(*listed);
    if pk.as_scheme(BlsScheme::Modern).canonicalize().is_ok() {
        return Some(*listed);
    }
    pk.as_scheme(BlsScheme::Legacy)
        .reencode(BlsScheme::Modern)
        .ok()
        .map(|pk| *pk.as_bytes())
}

/// A valid basic-serialized operator public key for tests: the G1 generator
/// (the public key of the scalar 1), whose compressed form is fixed by the
/// BLS12-381 spec.
#[cfg(test)]
pub(crate) const TEST_OPERATOR_KEY: [u8; 48] = [
    0x97, 0xf1, 0xd3, 0xa7, 0x31, 0x97, 0xd7, 0x94, 0x26, 0x95, 0x63, 0x8c, 0x4f, 0xa9, 0xac, 0x0f,
    0xc3, 0x68, 0x8c, 0x4f, 0x97, 0x74, 0xb9, 0x05, 0xa1, 0x4e, 0x3a, 0x3f, 0x17, 0x1b, 0xac, 0x58,
    0x6c, 0x55, 0xe8, 0x3f, 0xf9, 0x7a, 0x1a, 0xef, 0xfb, 0x3a, 0xf0, 0x0a, 0xdb, 0x22, 0xc6, 0xbb,
];

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn test_QT_123_operator_key_input_is_checked() {
        assert_eq!(parse_public_hex("00"), Err(BlsError::PublicHex));
        assert_eq!(parse_public_hex(&"zz".repeat(48)), Err(BlsError::PublicHex));
        assert_eq!(
            parse_public_hex(&"11".repeat(48)),
            Err(BlsError::PublicPoint)
        );
        assert_eq!(legacy_of(&[0x11; 48]), None);
    }

    #[test]
    fn test_QT_124_public_key_round_trips_and_has_a_legacy_form() {
        let basic = TEST_OPERATOR_KEY;
        assert_eq!(parse_public_hex(&hex::encode(basic)).unwrap(), basic);
        let legacy = legacy_of(&basic).unwrap();
        assert_ne!(
            basic, legacy,
            "the two serializations differ in the flag bits"
        );
    }

    #[test]
    fn test_QT_126_a_lazy_key_is_valid_as_core_reads_it() {
        assert!(is_valid_basic_public(&TEST_OPERATOR_KEY));
        // Not a point; all zeros; the point at infinity; a valid key's
        // legacy form, which isn't a basic encoding.
        assert!(!is_valid_basic_public(&[0x11; 48]));
        assert!(!is_valid_basic_public(&[0; 48]));
        let mut infinity = [0; 48];
        infinity[0] = 0xc0;
        assert!(!is_valid_basic_public(&infinity));
        assert!(!is_valid_basic_public(
            &legacy_of(&TEST_OPERATOR_KEY).unwrap()
        ));
    }

    #[test]
    fn a_listed_key_reads_in_its_basic_form() {
        let basic = TEST_OPERATOR_KEY;
        assert_eq!(basic_form(&basic), Some(basic));
        assert_eq!(basic_form(&legacy_of(&basic).unwrap()), Some(basic));
        assert_eq!(basic_form(&[0x11; 48]), None);
    }
}
