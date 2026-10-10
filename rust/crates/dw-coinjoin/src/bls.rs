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

/// Test helpers: a deterministic operator key that signs like a masternode.
pub mod testing {
    use dash_pkc::bls::{BlsScIetf, BlsSecretKey, BlsSigId};
    use dashcore::hashes::{Hash, sha256};

    pub struct OperatorKey(BlsSecretKey<BlsScIetf>);

    impl OperatorKey {
        pub fn from_seed(seed: &[u8]) -> Self {
            let ikm = sha256::Hash::hash(seed).to_byte_array();
            Self(BlsSecretKey::from_ikm(&ikm).expect("32 bytes of key material"))
        }

        /// Basic-form public key, as a v2 list entry carries it.
        pub fn public_key(&self) -> [u8; 48] {
            self.0.public_key().to_bytes()
        }

        /// Basic-scheme signature, 96 bytes.
        pub fn sign(&self, msg: &[u8; 32]) -> Vec<u8> {
            self.0.sign_with(msg, BlsSigId::Basic).to_bytes().to_vec()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::OperatorKey;
    use super::*;

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
