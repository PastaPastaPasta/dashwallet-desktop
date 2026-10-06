//! Operator signature checks for `dsq` and `dstx`.
//!
//! A masternode signs both with its operator key in the basic scheme
//! (`CActiveMasternodeManager::SignBasic`), and clients check them with
//! `CBLSSignature(vchSig, /*specificLegacyScheme=*/false).VerifyInsecure(pubKeyOperator,
//! hash, false)` (coinjoin.cpp:49-57, 77-86): the message is the 32-byte
//! signature hash. The list serializes an operator key in the basic form
//! for v2+ entries and the legacy form for v1 entries, so both forms are
//! tried.

use dashcore::blsful::{Bls12381G2Impl, Pairing, PublicKey, SerializationFormat, Signature};

/// Whether `signature` (96-byte compressed G2) is the basic-scheme
/// signature of `msg` by `operator_public_key`.
pub fn verify_basic(operator_public_key: &[u8; 48], signature: &[u8], msg: &[u8; 32]) -> bool {
    let Ok(raw) = <[u8; 96]>::try_from(signature) else {
        return false;
    };
    let Some(point) = <Bls12381G2Impl as Pairing>::Signature::from_compressed(&raw).into_option()
    else {
        return false;
    };
    let sig = Signature::<Bls12381G2Impl>::Basic(point);
    [SerializationFormat::Modern, SerializationFormat::Legacy]
        .into_iter()
        .filter_map(|f| {
            PublicKey::<Bls12381G2Impl>::from_bytes_with_mode(operator_public_key, f).ok()
        })
        .any(|pk| sig.verify(&pk, msg).is_ok())
}

/// Test helpers: a deterministic operator key that signs like a masternode.
pub mod testing {
    use dashcore::blsful::{Bls12381G2Impl, SecretKey, SignatureSchemes};

    pub struct OperatorKey(SecretKey<Bls12381G2Impl>);

    impl OperatorKey {
        pub fn from_seed(seed: &[u8]) -> Self {
            Self(SecretKey::from_hash(seed))
        }

        /// Basic-form public key, as a v2 list entry carries it.
        pub fn public_key(&self) -> [u8; 48] {
            let bytes = self.0.public_key().to_bytes();
            bytes.try_into().expect("48-byte G1 point")
        }

        /// Basic-scheme signature, 96 bytes.
        pub fn sign(&self, msg: &[u8; 32]) -> Vec<u8> {
            match self
                .0
                .sign(SignatureSchemes::Basic, msg)
                .expect("signing cannot fail")
            {
                dashcore::blsful::Signature::Basic(p) => p.to_compressed().to_vec(),
                _ => unreachable!("basic scheme requested"),
            }
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
}
