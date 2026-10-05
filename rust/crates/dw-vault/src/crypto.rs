//! Primitives: random bytes, XChaCha20-Poly1305 with AAD, Argon2id.
//!
//! The crates and versions are the ones platform-wallet-storage's `secrets`
//! feature locks (`argon2 =0.5.3`, `chacha20poly1305 =0.10.1`,
//! `getrandom =0.2.17`); the slot and record logic around them is ours.

use std::time::{Duration, Instant};

use argon2::{Algorithm, Argon2, Block, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use crate::VaultError;

/// 256-bit key (data key or key-encryption key).
pub(crate) type Key32 = Zeroizing<[u8; 32]>;

pub(crate) const NONCE_LEN: usize = 24;
pub(crate) const SALT_LEN: usize = 16;

/// Fills `buf` from the OS CSPRNG.
pub(crate) fn fill_random(buf: &mut [u8]) -> Result<(), VaultError> {
    getrandom::getrandom(buf).map_err(|e| VaultError::Internal(format!("OS random source: {e}")))
}

pub(crate) fn random_key() -> Result<Key32, VaultError> {
    let mut k = Zeroizing::new([0u8; 32]);
    fill_random(&mut k[..])?;
    Ok(k)
}

pub(crate) fn random_array<const N: usize>() -> Result<[u8; N], VaultError> {
    let mut a = [0u8; N];
    fill_random(&mut a)?;
    Ok(a)
}

/// XChaCha20-Poly1305 ciphertext with its nonce, hex-encoded on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sealed {
    #[serde(with = "hex_bytes")]
    pub nonce: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub ct: Vec<u8>,
}

/// Encrypts `plaintext` under `key` with a fresh random nonce, binding `aad`.
pub(crate) fn seal(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Result<Sealed, VaultError> {
    let nonce: [u8; NONCE_LEN] = random_array()?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let ct = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| VaultError::Internal("AEAD encryption failed".into()))?;
    Ok(Sealed {
        nonce: nonce.to_vec(),
        ct,
    })
}

/// Decrypts and authenticates `sealed` under `key` and `aad`. `None` when the
/// tag does not verify (wrong key, tampered ciphertext or AAD).
pub(crate) fn open(key: &[u8; 32], sealed: &Sealed, aad: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if sealed.nonce.len() != NONCE_LEN {
        return None;
    }
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    cipher
        .decrypt(
            XNonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ct,
                aad,
            },
        )
        .ok()
        .map(Zeroizing::new)
}

/// Argon2id cost parameters of a passphrase slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Iterations.
    pub t: u32,
    /// Lanes.
    pub p: u32,
}

impl KdfParams {
    /// Production floor (DESIGN-opus §1.8): m ≥ 256 MiB, t ≥ 3.
    pub const FLOOR: KdfParams = KdfParams {
        m_kib: 256 * 1024,
        t: 3,
        p: 1,
    };

    /// Cheap parameters for tests. Refused unless weak parameters are allowed
    /// (see [`KdfPolicy`]).
    pub const TEST: KdfParams = KdfParams {
        m_kib: 64,
        t: 1,
        p: 1,
    };

    pub fn meets_floor(&self) -> bool {
        self.m_kib >= Self::FLOOR.m_kib && self.t >= Self::FLOOR.t && self.p >= 1
    }

    /// Bytes bound into the passphrase slot's AAD.
    pub(crate) fn aad_bytes(&self) -> [u8; 12] {
        let mut b = [0u8; 12];
        b[..4].copy_from_slice(&self.m_kib.to_le_bytes());
        b[4..8].copy_from_slice(&self.t.to_le_bytes());
        b[8..].copy_from_slice(&self.p.to_le_bytes());
        b
    }
}

/// How a new passphrase slot chooses its Argon2id parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KdfPolicy {
    /// Floor memory and lanes; iterations raised from 3 until one derivation
    /// takes at least [`CALIBRATION_TARGET`] on this machine.
    #[default]
    Calibrated,
    /// Exactly these parameters. Below the floor only in this crate's tests or
    /// with the `insecure-test-kdf` feature in a debug build, and never on
    /// mainnet/testnet vaults.
    Fixed(KdfParams),
}

/// Minimum wall time of one passphrase derivation (DESIGN-opus §1.8).
pub const CALIBRATION_TARGET: Duration = Duration::from_millis(500);
/// Upper bound for calibrated iterations, so a slow first run cannot pick a
/// cost that takes minutes on the next unlock.
const MAX_CALIBRATED_T: u32 = 24;

/// Whether parameters below the floor may be used at all in this build.
pub(crate) fn weak_kdf_allowed() -> bool {
    cfg!(test) || (cfg!(feature = "insecure-test-kdf") && cfg!(debug_assertions))
}

/// Argon2id(passphrase, salt, params) → 32-byte key. The block matrix is
/// owned here and wiped afterwards (argon2 0.5.3 does not wipe it itself).
pub(crate) fn derive_kek(
    passphrase: &[u8],
    salt: &[u8],
    params: &KdfParams,
) -> Result<Key32, VaultError> {
    let p = Params::new(params.m_kib, params.t, params.p, Some(32))
        .map_err(|e| VaultError::Corrupt(format!("argon2 parameters: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p.clone());
    let mut blocks = vec![Block::default(); p.block_count()];
    let mut out = Zeroizing::new([0u8; 32]);
    let result = argon.hash_password_into_with_memory(passphrase, salt, &mut out[..], &mut blocks);
    for b in blocks.iter_mut() {
        b.zeroize();
    }
    result.map_err(|e| VaultError::Internal(format!("argon2: {e}")))?;
    Ok(out)
}

/// Picks parameters under `policy` and derives the key with them.
pub(crate) fn derive_new_kek(
    passphrase: &[u8],
    salt: &[u8],
    policy: KdfPolicy,
    production_network: bool,
) -> Result<(KdfParams, Key32), VaultError> {
    match policy {
        KdfPolicy::Fixed(params) => {
            if !params.meets_floor() && (!weak_kdf_allowed() || production_network) {
                return Err(VaultError::InvalidArgument(
                    "Argon2id parameters below the production floor".into(),
                ));
            }
            if !params.meets_floor() {
                tracing::warn!(?params, "vault uses Argon2id parameters below the floor (test build)");
            }
            Ok((params, derive_kek(passphrase, salt, &params)?))
        }
        KdfPolicy::Calibrated => {
            let mut params = KdfParams::FLOOR;
            let started = Instant::now();
            let key = derive_kek(passphrase, salt, &params)?;
            let elapsed = started.elapsed();
            if elapsed >= CALIBRATION_TARGET {
                return Ok((params, key));
            }
            // Scale iterations by the shortfall, with 10% headroom.
            let ratio = CALIBRATION_TARGET.as_secs_f64() / elapsed.as_secs_f64().max(1e-3);
            params.t = ((f64::from(params.t) * ratio * 1.1).ceil() as u32)
                .clamp(KdfParams::FLOOR.t + 1, MAX_CALIBRATED_T);
            drop(key);
            Ok((params, derive_kek(passphrase, salt, &params)?))
        }
    }
}

/// Lower-case hex (de)serialization of byte vectors.
pub(crate) mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        hex::decode(s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip_and_aad_binding() {
        let key = random_key().unwrap();
        let sealed = seal(&key, b"secret", b"aad-1").unwrap();
        assert_eq!(&open(&key, &sealed, b"aad-1").unwrap()[..], b"secret");
        assert!(open(&key, &sealed, b"aad-2").is_none());
        let other = random_key().unwrap();
        assert!(open(&other, &sealed, b"aad-1").is_none());
        let mut flipped = sealed.clone();
        flipped.ct[0] ^= 1;
        assert!(open(&key, &flipped, b"aad-1").is_none());
    }

    #[test]
    fn kdf_is_deterministic_and_salted() {
        let p = KdfParams::TEST;
        let a = derive_kek(b"pw", &[1; 16], &p).unwrap();
        let b = derive_kek(b"pw", &[1; 16], &p).unwrap();
        let c = derive_kek(b"pw", &[2; 16], &p).unwrap();
        assert_eq!(*a, *b);
        assert_ne!(*a, *c);
    }

    #[test]
    fn weak_fixed_params_refused_on_production_networks() {
        let r = derive_new_kek(b"pw", &[0; 16], KdfPolicy::Fixed(KdfParams::TEST), true);
        assert!(matches!(r, Err(VaultError::InvalidArgument(_))));
        assert!(derive_new_kek(b"pw", &[0; 16], KdfPolicy::Fixed(KdfParams::TEST), false).is_ok());
    }

    /// Runs the real 256 MiB derivation; the calibrated cost never drops
    /// below the floor and the derived key matches a re-derivation with the
    /// chosen parameters (what unlock does). Wall time is not asserted: it
    /// depends on machine load.
    #[test]
    fn calibrated_kdf_meets_floor() {
        let (params, key) = derive_new_kek(b"pw", &[3; 16], KdfPolicy::Calibrated, true).unwrap();
        assert!(params.meets_floor(), "{params:?}");
        assert!(params.t <= MAX_CALIBRATED_T);
        assert_eq!(*derive_kek(b"pw", &[3; 16], &params).unwrap(), *key);
    }
}
