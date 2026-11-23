//! Operator signature checks for `dsq` and `dstx`.
//!
//! A masternode signs both with its operator key in the basic scheme
//! (`CActiveMasternodeManager::SignBasic`), and clients check them with
//! `CBLSSignature(vchSig, /*specificLegacyScheme=*/false).VerifyInsecure(pubKeyOperator,
//! hash, false)` (coinjoin.cpp:49-57, 77-86): the message is the 32-byte
//! signature hash. The list serializes an operator key in the basic form
//! for v2+ entries and the legacy form for v1 entries, so both forms are
//! tried, each re-encoded in the basic form the signature is read in.

use dashcore::bls_sig_utils::{BlsPkBytes, BlsScheme, BlsSigBytes};

/// Whether `signature` (96-byte compressed G2) is the basic-scheme
/// signature of `msg` by `operator_public_key`.
pub fn verify_basic(operator_public_key: &[u8; 48], signature: &[u8], msg: &[u8; 32]) -> bool {
    let Ok(raw) = <[u8; 96]>::try_from(signature) else {
        return false;
    };
    let signature = BlsSigBytes::from_bytes(raw);
    [BlsScheme::Modern, BlsScheme::Legacy]
        .into_iter()
        .filter_map(|scheme| {
            BlsPkBytes::from_bytes(*operator_public_key)
                .as_scheme(scheme)
                .reencode(BlsScheme::Modern)
                .ok()
        })
        .any(|key| {
            key.as_scheme(BlsScheme::Modern)
                .verify(msg, &signature)
                .is_ok()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_operator_key::OperatorKey;

    #[test]
    fn verifies_basic_signatures_only_from_the_operator() {
        let key = OperatorKey::from_seed(b"mn-1");
        let other = OperatorKey::from_seed(b"mn-2");
        let msg = [5u8; 32];
        let sig = key.sign(&msg);
        assert_eq!(sig.len(), 96);
        assert!(verify_basic(&key.public_key(), &sig, &msg));
        assert!(!verify_basic(&other.public_key(), &sig, &msg));
        assert!(!verify_basic(&key.public_key(), &sig, &[6u8; 32]));
        assert!(!verify_basic(&key.public_key(), &sig[..95], &msg));
        assert!(!verify_basic(&key.public_key(), &[0u8; 96], &msg));
    }

    /// A v1 list entry carries the operator key in the legacy form; the
    /// signature is still checked in the basic scheme.
    #[test]
    fn verifies_with_the_legacy_form_of_the_operator_key() {
        let key = OperatorKey::from_seed(b"mn-1");
        let legacy = BlsPkBytes::from_bytes(key.public_key())
            .as_scheme(BlsScheme::Modern)
            .reencode(BlsScheme::Legacy)
            .unwrap()
            .to_bytes();
        assert_ne!(legacy, key.public_key());
        let msg = [5u8; 32];
        let sig = key.sign(&msg);
        assert!(verify_basic(&legacy, &sig, &msg));
        assert!(!verify_basic(&legacy, &sig, &[6u8; 32]));
        let other = OperatorKey::from_seed(b"mn-2");
        assert!(!verify_basic(&legacy, &other.sign(&msg), &msg));
    }
}
