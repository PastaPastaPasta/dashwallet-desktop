//! The DIP-15 primitives of the vault's DashPay operations, computed as
//! `platform-encryption` computes them (platform `bc321362b9`, the crate
//! platform-wallet calls) but with every secret intermediate this code owns
//! erased (review DW-E0-03 M1). The outputs are byte-identical to that
//! crate's (the `matches_platform_encryption` tests below, and the engine's
//! fixture vectors).
//!
//! What the upstream helpers leave behind, and what is done here instead:
//! - `derive_shared_key_ecdh` copies a secp256k1 `SharedSecret` (`Copy`, no
//!   erasing drop) and never erases it. Here it is erased after the copy.
//! - `calculate_account_reference` keys `hmac` 0.12, whose padded key block
//!   (`key ^ opad`) and PRF output stay on the stack, over `sha2`, whose
//!   hasher state, block buffer and finalization copy have no erasing
//!   `Drop` (review DW-E0-03 r2 N1: the keyed midstates after `K ^ ipad`
//!   and `K ^ opad` compute further PRF outputs). Here HMAC-SHA256 runs on
//!   [`ErasingSha256`], this crate's own SHA-256, whose chaining state,
//!   message schedule and block buffer are erased when it drops; the key
//!   blocks, the inner digest and the output are `Zeroizing` buffers, and
//!   each digest is written straight into its guarded destination.
//! - `aes` without its `zeroize` feature keeps the AES key schedule after
//!   drop. dw-vault enables `aes/zeroize` and `cbc/zeroize`, which, by
//!   feature unification, also covers platform-wallet's own AES calls.
//! - `encrypt_aes_256_cbc` grows its plaintext buffer (the old allocation is
//!   freed unerased); `decrypt_aes_256_cbc` frees its decrypted copy
//!   unerased. Here each runs in one `Zeroizing` buffer sized up front.
//!
//! Out of reach: libsecp256k1's stack during ECDH (its default hash is
//! computed inside the C library); the round variables of a SHA-256
//! compression and of AES while they are in registers or spilled to the
//! stack; and moves of return values the compiler makes.

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{
    BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit,
};
use dashcore::secp256k1::ecdh::SharedSecret;
use dashcore::secp256k1::{PublicKey, SecretKey};
use zeroize::{Zeroize, Zeroizing};

use crate::SignerError;
use crate::crypto::Key32;

type Aes256CbcEnc = cbc::Encryptor<Aes256>;
type Aes256CbcDec = cbc::Decryptor<Aes256>;

const AES_BLOCK: usize = 16;
const SHA256_BLOCK: usize = 64;

/// DIP-15 ECDH: `SHA256((y&1|2) ‖ x)` of the shared point (libsecp256k1's
/// default ECDH hash).
pub(crate) fn ecdh(secret: &SecretKey, peer: &PublicKey) -> Zeroizing<[u8; 32]> {
    let mut shared = SharedSecret::new(peer, secret);
    let mut out = Zeroizing::new([0u8; 32]);
    out.copy_from_slice(shared.as_secret_bytes());
    shared.non_secure_erase();
    out
}

/// SHA-256 round constants (FIPS 180-4 §4.2.2).
const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 initial hash value (FIPS 180-4 §5.3.3).
const SHA256_IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256 (FIPS 180-4) whose whole state is this crate's: the chaining
/// value (after an HMAC key block, a key-equivalent midstate), the message
/// schedule (whose first words are the block itself) and the partial
/// block are erased on drop. `sha2` 0.10 keeps all three in buffers it
/// never erases. Only for the short HMAC inputs of DIP-15: a plain
/// portable implementation, constant-time (no data-dependent branch or
/// index), checked against `sha2` in the tests. It is never moved after
/// it absorbs secret input (both HMAC passes use it in place), so no stale
/// copy of the state is left behind.
struct ErasingSha256 {
    state: [u32; 8],
    schedule: [u32; 64],
    block: [u8; SHA256_BLOCK],
    filled: usize,
    /// Message length in bytes.
    length: u64,
}

impl ErasingSha256 {
    fn new() -> Self {
        Self {
            state: SHA256_IV,
            schedule: [0; 64],
            block: [0; SHA256_BLOCK],
            filled: 0,
            length: 0,
        }
    }

    /// Erases the state of the message so far and starts a new one.
    fn reset(&mut self) {
        self.zeroize();
        self.state = SHA256_IV;
    }

    fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (SHA256_BLOCK - self.filled).min(data.len());
            self.block[self.filled..self.filled + take].copy_from_slice(&data[..take]);
            self.filled += take;
            data = &data[take..];
            if self.filled == SHA256_BLOCK {
                self.compress();
                self.filled = 0;
            }
        }
    }

    /// Pads, writes the digest into `out`, then erases the state and starts
    /// over ([`Self::reset`]), ready for the next message.
    fn finalize_into(&mut self, out: &mut [u8; 32]) {
        let bits = self.length.wrapping_mul(8);
        self.block[self.filled] = 0x80;
        self.block[self.filled + 1..].fill(0);
        if self.filled + 1 > SHA256_BLOCK - 8 {
            self.compress();
            self.block.fill(0);
        }
        self.block[SHA256_BLOCK - 8..].copy_from_slice(&bits.to_be_bytes());
        self.compress();
        for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(&self.state) {
            *chunk = word.to_be_bytes();
        }
        self.reset();
    }

    /// One compression of `block` into `state` (FIPS 180-4 §6.2.2).
    fn compress(&mut self) {
        let w = &mut self.schedule;
        for (word, bytes) in w.iter_mut().zip(self.block.as_chunks::<4>().0) {
            *word = u32::from_be_bytes(*bytes);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16]
                .wrapping_add(s0)
                .wrapping_add(w[t - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for t in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[t])
                .wrapping_add(w[t]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

impl Zeroize for ErasingSha256 {
    fn zeroize(&mut self) {
        self.state.zeroize();
        self.schedule.zeroize();
        self.block.zeroize();
        self.filled.zeroize();
        self.length.zeroize();
    }
}

impl Drop for ErasingSha256 {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// HMAC-SHA256 (RFC 2104) with a 32-byte key, on [`ErasingSha256`]: every
/// keyed state, key block and digest is erased before this returns, except
/// the output.
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut block = Zeroizing::new([0u8; SHA256_BLOCK]);
    let pad = |block: &mut [u8; SHA256_BLOCK], with: u8| {
        block.fill(with);
        for (b, k) in block.iter_mut().zip(key) {
            *b ^= k;
        }
    };
    let mut h = ErasingSha256::new();
    let mut inner = Zeroizing::new([0u8; 32]);
    pad(&mut block, 0x36);
    h.update(&block[..]);
    h.update(message);
    h.finalize_into(&mut inner);
    let mut out = Zeroizing::new([0u8; 32]);
    pad(&mut block, 0x5c);
    h.update(&block[..]);
    h.update(&inner[..]);
    h.finalize_into(&mut out);
    out
}

/// The 28-bit account secret `ASK28`: the last four bytes of
/// `HMAC-SHA256(key, compact_xpub)`, big-endian, shifted right by 4 (iOS
/// dash-shared-core, as platform-encryption).
fn ask28(key: &[u8; 32], compact_xpub: &[u8]) -> u32 {
    let ask = hmac_sha256(key, compact_xpub);
    u32::from_be_bytes([ask[28], ask[29], ask[30], ask[31]]) >> 4
}

/// DIP-15 `accountReference`: `version << 28 | (ASK28 ^ account[0..28])`.
pub(crate) fn account_reference(
    key: &[u8; 32],
    compact_xpub: &[u8],
    account_index: u32,
    version: u32,
) -> u32 {
    (version << 28) | (ask28(key, compact_xpub) ^ (account_index & 0x0FFF_FFFF))
}

/// Inverse of [`account_reference`]: `(version, account_index)`.
pub(crate) fn unmask_account_reference(
    account_reference: u32,
    key: &[u8; 32],
    compact_xpub: &[u8],
) -> (u32, u32) {
    (
        account_reference >> 28,
        (account_reference & 0x0FFF_FFFF) ^ ask28(key, compact_xpub),
    )
}

/// `encToUserId`: AES-256-ECB of the two 16-byte halves of `id`.
pub(crate) fn encrypt_enc_to_user_id(key: &Key32, id: &[u8; 32]) -> [u8; 32] {
    aes_ecb_32(key, id, false)
}

/// Inverse of [`encrypt_enc_to_user_id`].
pub(crate) fn decrypt_enc_to_user_id(key: &Key32, ciphertext: &[u8; 32]) -> [u8; 32] {
    aes_ecb_32(key, ciphertext, true)
}

fn aes_ecb_32(key: &Key32, id: &[u8; 32], decrypt: bool) -> [u8; 32] {
    let cipher = Aes256::new(GenericArray::from_slice(&key[..]));
    let mut out = *id;
    for block in out.as_chunks_mut::<AES_BLOCK>().0 {
        let block = GenericArray::from_mut_slice(block);
        if decrypt {
            cipher.decrypt_block(block);
        } else {
            cipher.encrypt_block(block);
        }
    }
    out
}

/// `privateData`: `iv ‖ AES-256-CBC(plaintext)` with PKCS#7 padding.
pub(crate) fn encrypt_private_data(key: &Key32, iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    // PKCS#7 always adds 1..=16 bytes, so the buffer never grows.
    let padded = (plaintext.len() / AES_BLOCK + 1) * AES_BLOCK;
    let mut buf = Zeroizing::new(vec![0u8; padded]);
    buf[..plaintext.len()].copy_from_slice(plaintext);
    let ciphertext = Aes256CbcEnc::new(GenericArray::from_slice(&key[..]), iv.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
        .expect("the buffer has room for the padding");
    let mut out = Vec::with_capacity(AES_BLOCK + ciphertext.len());
    out.extend_from_slice(iv);
    out.extend_from_slice(ciphertext);
    out
}

/// Inverse of [`encrypt_private_data`]. The plaintext is erased when the
/// returned buffer drops.
pub(crate) fn decrypt_private_data(
    key: &Key32,
    blob: &[u8],
) -> Result<Zeroizing<Vec<u8>>, SignerError> {
    if blob.len() < AES_BLOCK {
        return Err(SignerError::Decrypt(
            "privateData is shorter than its IV".into(),
        ));
    }
    let (iv, ciphertext) = blob.split_at(AES_BLOCK);
    let mut buf = Zeroizing::new(ciphertext.to_vec());
    let len = Aes256CbcDec::new(
        GenericArray::from_slice(&key[..]),
        GenericArray::from_slice(iv),
    )
    .decrypt_padded_mut::<Pkcs7>(&mut buf)
    .map_err(|_| SignerError::Decrypt("privateData does not decrypt under this key".into()))?
    .len();
    buf.truncate(len);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> Key32 {
        Zeroizing::new(std::array::from_fn(|i| {
            n.wrapping_mul(31).wrapping_add(i as u8)
        }))
    }

    /// The AES key schedules and CBC states erase themselves (`aes/zeroize`,
    /// `cbc/zeroize`): without those features none of them has a `Drop`.
    #[test]
    fn aes_state_erases_on_drop() {
        assert!(std::mem::needs_drop::<Aes256>());
        assert!(std::mem::needs_drop::<Aes256CbcEnc>());
        assert!(std::mem::needs_drop::<Aes256CbcDec>());
    }

    #[test]
    fn ecdh_matches_platform_encryption() {
        for (a, b) in [(1u8, 2u8), (0x42, 0xC0), (0x0D, 0xFE)] {
            let sk = SecretKey::from_secret_bytes([a; 32]).unwrap();
            let peer = PublicKey::from_secret_key(&SecretKey::from_secret_bytes([b; 32]).unwrap());
            assert_eq!(
                *ecdh(&sk, &peer),
                platform_encryption::derive_shared_key_ecdh(&sk, &peer)
            );
        }
    }

    #[test]
    fn erasing_sha256_matches_sha2() {
        use sha2::{Digest, Sha256};
        let data: Vec<u8> = (0..1100u32).map(|i| (i * 7 + i / 13) as u8).collect();
        for len in (0..=200).chain([255, 256, 1000, 1100]) {
            let msg = &data[..len];
            let expected: [u8; 32] = Sha256::digest(msg).into();
            // Whole, and split at every block-relative position.
            for split in [0, 1, 55, 56, 63, 64, 65, len / 2, len] {
                let split = split.min(len);
                let mut h = ErasingSha256::new();
                h.update(&msg[..split]);
                h.update(&msg[split..]);
                let mut out = [0u8; 32];
                h.finalize_into(&mut out);
                assert_eq!(out, expected, "length {len}, split {split}");
            }
        }
    }

    /// Once a digest is out, and after `reset`, nothing of the message is
    /// left (schedule, block, counters zero; the state is the IV), and the
    /// hasher digests the next message correctly.
    #[test]
    fn erasing_sha256_leaves_nothing() {
        let fresh = |h: &ErasingSha256| {
            h.state == SHA256_IV
                && h.schedule == [0; 64]
                && h.block == [0; 64]
                && h.filled == 0
                && h.length == 0
        };
        let mut h = ErasingSha256::new();
        h.update(&[0xA5; 100]);
        let mut first = [0u8; 32];
        h.finalize_into(&mut first);
        assert!(fresh(&h));
        h.update(&[0x5A; 70]);
        h.reset();
        assert!(fresh(&h));
        h.update(&[0xA5; 100]);
        let mut again = [0u8; 32];
        h.finalize_into(&mut again);
        assert_eq!(first, again);
    }

    #[test]
    fn hmac_is_rfc_4231() {
        // RFC 4231 test cases 1-4 (keys of 20, 4, 20 and 25 bytes). HMAC
        // pads a short key with zeros, so padding it to 32 bytes here does
        // not change the MAC.
        let padded = |k: &[u8]| {
            let mut out = [0u8; 32];
            out[..k.len()].copy_from_slice(k);
            out
        };
        let key4: Vec<u8> = (1..=25).collect();
        for (key, data, mac) in [
            (
                padded(&[0x0b; 20]),
                b"Hi There".to_vec(),
                "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
            ),
            (
                padded(b"Jefe"),
                b"what do ya want for nothing?".to_vec(),
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                padded(&[0xaa; 20]),
                vec![0xdd; 50],
                "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
            ),
            (
                padded(&key4),
                vec![0xcd; 50],
                "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
            ),
        ] {
            assert_eq!(hex::encode(*hmac_sha256(&key, &data)), mac);
        }
    }

    #[test]
    fn account_reference_matches_platform_encryption() {
        for n in 0..8u8 {
            let k = *key(n);
            let xpub: Vec<u8> = (0..69).map(|i| i as u8 ^ n).collect();
            for (account, version) in [(0, 0), (1, 1), (5, 7), (0x0FFF_FFFF, 15), (u32::MAX, 3)] {
                let ours = account_reference(&k, &xpub, account, version);
                assert_eq!(
                    ours,
                    platform_encryption::calculate_account_reference(&k, &xpub, account, version)
                );
                assert_eq!(
                    unmask_account_reference(ours, &k, &xpub),
                    platform_encryption::unmask_account_reference(ours, &k, &xpub)
                );
            }
        }
    }

    #[test]
    fn contact_info_matches_platform_encryption() {
        for n in 0..4u8 {
            let k = key(n);
            let id: [u8; 32] = std::array::from_fn(|i| i as u8 ^ n);
            let enc = encrypt_enc_to_user_id(&k, &id);
            assert_eq!(enc, platform_encryption::encrypt_enc_to_user_id(&k, &id));
            let dec = decrypt_enc_to_user_id(&k, &enc);
            assert_eq!(dec, id);
            assert_eq!(dec, platform_encryption::decrypt_enc_to_user_id(&k, &enc));
            let iv = [n ^ 0x5a; 16];
            for len in (0..=49).chain([1000]) {
                let plain: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
                let blob = encrypt_private_data(&k, &iv, &plain);
                assert_eq!(
                    blob,
                    platform_encryption::encrypt_private_data(&k, &iv, &plain),
                    "length {len}"
                );
                assert_eq!(*decrypt_private_data(&k, &blob).unwrap(), plain);
            }
        }
    }

    #[test]
    fn private_data_refuses_what_does_not_decrypt() {
        let blob = encrypt_private_data(&key(1), &[0; 16], b"alias");
        assert!(decrypt_private_data(&key(1), &blob[..15]).is_err());
        assert!(decrypt_private_data(&key(1), &blob[..20]).is_err());
        // Another key: the padding check fails (or, rarely, garbage opens).
        assert_ne!(
            decrypt_private_data(&key(2), &blob)
                .map(|p| p.to_vec())
                .ok(),
            Some(b"alias".to_vec())
        );
    }
}
