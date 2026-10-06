//! Operator BLS keys (basic scheme, BLS12-381 G1 public keys) as dash-qt's
//! Register wizard and Dash Core's `bls generate` / `bls fromsecret` use
//! them (`src/rpc/evo.cpp` `bls_generate`, `src/qt/masternodewizard.cpp`).
//!
//! Secrets are 32-byte big-endian scalars held in [`Zeroizing`] buffers.
//! Public keys are 48 bytes, in the basic (modern, IETF) serialization that
//! version-2 payloads carry; the legacy serialization of the same point is
//! what version-1 masternodes registered with.

use dashcore::blsful::{
    Bls12381G2Impl, PublicKey, SecretKey, SerializationFormat, SignatureSchemes,
};
use zeroize::Zeroizing;

/// A BLS secret as a big-endian scalar.
pub type BlsSecret = Zeroizing<[u8; 32]>;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BlsError {
    #[error("not 64 hexadecimal characters")]
    SecretHex,
    #[error("not a BLS secret key (scalar out of range)")]
    SecretRange,
    #[error("not 96 hexadecimal characters")]
    PublicHex,
    #[error("not a BLS12-381 public key")]
    PublicPoint,
    #[error("no system randomness: {0}")]
    Random(String),
    #[error("BLS signing failed: {0}")]
    Sign(String),
}

/// The BLS12-381 scalar field order `r`, big-endian.
const GROUP_ORDER: [u8; 32] = [
    0x73, 0xed, 0xa7, 0x53, 0x29, 0x9d, 0x7d, 0x48, 0x33, 0x39, 0xd8, 0x08, 0x09, 0xa1, 0xd8, 0x05,
    0x53, 0xbd, 0xa4, 0x02, 0xff, 0xfe, 0x5b, 0xfe, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01,
];

/// A secret key from a big-endian scalar in `1..r`. dashbls refuses bytes at
/// or above the group order (`PrivateKey::FromBytes(…, modOrder=false)`)
/// where blsful would reduce them, so the range is checked here.
fn secret_key(secret: &[u8; 32]) -> Result<SecretKey<Bls12381G2Impl>, BlsError> {
    if secret[..] >= GROUP_ORDER[..] || secret.iter().all(|b| *b == 0) {
        return Err(BlsError::SecretRange);
    }
    Option::from(SecretKey::<Bls12381G2Impl>::from_be_bytes(secret)).ok_or(BlsError::SecretRange)
}

/// A fresh operator secret from the system RNG (`bls generate`): 32 random
/// bytes, redrawn until they are a valid scalar.
pub fn generate() -> Result<BlsSecret, BlsError> {
    loop {
        let mut bytes = Zeroizing::new([0u8; 32]);
        getrandom::getrandom(&mut bytes[..]).map_err(|e| BlsError::Random(e.to_string()))?;
        if secret_key(&bytes).is_ok() {
            return Ok(bytes);
        }
    }
}

/// Parses `bls fromsecret`'s argument: 64 hex characters.
pub fn parse_secret_hex(text: &str) -> Result<BlsSecret, BlsError> {
    let text = text.trim();
    if text.len() != 64 {
        return Err(BlsError::SecretHex);
    }
    let mut out = Zeroizing::new([0u8; 32]);
    hex::decode_to_slice(text, &mut out[..]).map_err(|_| BlsError::SecretHex)?;
    secret_key(&out)?;
    Ok(out)
}

/// The secret as lowercase hex, in a zeroing buffer.
pub fn secret_hex(secret: &[u8; 32]) -> Zeroizing<String> {
    Zeroizing::new(hex::encode(secret))
}

/// `(basic, legacy)` serializations of the secret's public key.
pub fn public_keys(secret: &[u8; 32]) -> Result<([u8; 48], [u8; 48]), BlsError> {
    let pk = PublicKey::from(&secret_key(secret)?);
    let basic: [u8; 48] = pk
        .to_bytes()
        .as_slice()
        .try_into()
        .map_err(|_| BlsError::PublicPoint)?;
    let legacy: [u8; 48] = pk
        .to_bytes_with_mode(SerializationFormat::Legacy)
        .as_slice()
        .try_into()
        .map_err(|_| BlsError::PublicPoint)?;
    Ok((basic, legacy))
}

/// Parses an operator public key typed in the wizard: 96 hex characters
/// that decode to a G1 point in the basic serialization (dash-qt refuses
/// legacy keys for new registrations).
pub fn parse_public_hex(text: &str) -> Result<[u8; 48], BlsError> {
    let text = text.trim();
    if text.len() != 96 {
        return Err(BlsError::PublicHex);
    }
    let mut out = [0u8; 48];
    hex::decode_to_slice(text, &mut out).map_err(|_| BlsError::PublicHex)?;
    PublicKey::<Bls12381G2Impl>::from_bytes_with_mode(&out, SerializationFormat::Modern)
        .map_err(|_| BlsError::PublicPoint)?;
    Ok(out)
}

/// The legacy serialization of a basic-serialized public key (for matching
/// a list entry registered before v19).
pub fn legacy_of(basic: &[u8; 48]) -> Option<[u8; 48]> {
    let pk = PublicKey::<Bls12381G2Impl>::from_bytes_with_mode(basic, SerializationFormat::Modern)
        .ok()?;
    pk.to_bytes_with_mode(SerializationFormat::Legacy)
        .as_slice()
        .try_into()
        .ok()
}

/// Whether `secret` belongs to `public` in either serialization.
pub fn matches(secret: &[u8; 32], public: &[u8; 48]) -> bool {
    public_keys(secret).is_ok_and(|(basic, legacy)| &basic == public || &legacy == public)
}

/// Basic-scheme signature over a 32-byte payload hash, 96 bytes in the
/// modern serialization: what ProUpServTx and ProUpRevTx carry
/// (`CBLSSecretKey::Sign(hash, /*specificLegacyScheme=*/false)`).
pub fn sign_hash(secret: &[u8; 32], hash: &[u8; 32]) -> Result<[u8; 96], BlsError> {
    let sig = secret_key(secret)?
        .sign(SignatureSchemes::Basic, hash)
        .map_err(|e| BlsError::Sign(e.to_string()))?;
    sig.to_bytes_with_mode(SerializationFormat::Modern)
        .as_slice()
        .try_into()
        .map_err(|_| BlsError::Sign("signature is not 96 bytes".into()))
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::blsful::Signature;

    #[test]
    fn test_QT_124_generated_secret_round_trips_through_hex() {
        let secret = generate().unwrap();
        let text = secret_hex(&secret);
        assert_eq!(text.len(), 64);
        assert_eq!(*parse_secret_hex(&text).unwrap(), *secret);
        let (basic, legacy) = public_keys(&secret).unwrap();
        assert_ne!(
            basic, legacy,
            "the two serializations differ in the flag bits"
        );
        assert_eq!(parse_public_hex(&hex::encode(basic)).unwrap(), basic);
        assert_eq!(legacy_of(&basic), Some(legacy));
        assert!(matches(&secret, &basic) && matches(&secret, &legacy));
    }

    #[test]
    fn test_QT_123_operator_key_input_is_checked() {
        assert_eq!(parse_secret_hex("ab"), Err(BlsError::SecretHex));
        assert_eq!(
            parse_secret_hex(&"ff".repeat(32)),
            Err(BlsError::SecretRange)
        );
        assert_eq!(
            parse_secret_hex(&hex::encode(GROUP_ORDER)),
            Err(BlsError::SecretRange)
        );
        assert_eq!(
            parse_secret_hex(&"00".repeat(32)),
            Err(BlsError::SecretRange)
        );
        assert_eq!(parse_public_hex("00"), Err(BlsError::PublicHex));
        assert_eq!(
            parse_public_hex(&"11".repeat(48)),
            Err(BlsError::PublicPoint)
        );
    }

    #[test]
    fn test_QT_125_payload_signature_verifies_under_the_basic_scheme() {
        let secret = Zeroizing::new([7u8; 32]);
        let hash = [0x42u8; 32];
        let sig_bytes = sign_hash(&secret, &hash).unwrap();
        let sig = Signature::<Bls12381G2Impl>::from_bytes_with_mode(
            &sig_bytes,
            SignatureSchemes::Basic,
            SerializationFormat::Modern,
        )
        .unwrap();
        let pk = PublicKey::from(&secret_key(&secret).unwrap());
        sig.verify(&pk, hash).unwrap();
    }
}
