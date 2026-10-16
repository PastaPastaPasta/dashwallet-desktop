//! A deterministic operator key that signs `dsq`/`dstx` like a masternode.
//!
//! Test-only (MG-01 D9, D21): the unit tests include this file with
//! `#[cfg(test)] #[path]` from `src/lib.rs`, and `tests/session.rs` with
//! `mod`. `dash-pkc` is a dev-dependency, so no library build can name
//! `dash_pkc`; dashcore's BLS secret types are behind clippy's
//! `disallowed-types`. It stands in for a masternode in the
//! CoinJoin tests and signs no wallet transaction; `rust/clippy.toml` and
//! dw-protx's `no_operator_signing` guard allow it here and nowhere else.
#![expect(clippy::disallowed_types)]

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
