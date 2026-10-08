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
//!   (`key ^ opad`) and PRF output stay on the stack. Here HMAC-SHA256 is
//!   built from SHA-256 with both key blocks, the inner digest and the
//!   output in `Zeroizing` buffers.
//! - `aes` without its `zeroize` feature keeps the AES key schedule after
//!   drop. dw-vault enables `aes/zeroize` and `cbc/zeroize`, which, by
//!   feature unification, also covers platform-wallet's own AES calls.
//! - `encrypt_aes_256_cbc` grows its plaintext buffer (the old allocation is
//!   freed unerased); `decrypt_aes_256_cbc` frees its decrypted copy
//!   unerased. Here each runs in one `Zeroizing` buffer sized up front.
//!
//! Out of reach: libsecp256k1's stack during ECDH; SHA-256's internals
//! (sha2 0.10 cannot erase its state, its message schedule or its block
//! buffer; the key blocks are compressed straight from our buffers, and the
//! block buffer ends holding the inner digest, which alone reveals neither
//! the key nor the mask); registers, stack spills and moves of return
//! values.

use aes::Aes256;
use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{
    BlockDecrypt, BlockDecryptMut, BlockEncrypt, BlockEncryptMut, KeyInit, KeyIvInit,
};
use dashcore::secp256k1::ecdh::SharedSecret;
use dashcore::secp256k1::{PublicKey, SecretKey};
use sha2::digest::generic_array::GenericArray;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

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
    out.copy_from_slice(shared.as_ref());
    shared.non_secure_erase();
    out
}

/// HMAC-SHA256 (RFC 2104) with a 32-byte key.
fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> Zeroizing<[u8; 32]> {
    let mut block = Zeroizing::new([0u8; SHA256_BLOCK]);
    let pad = |block: &mut [u8; SHA256_BLOCK], with: u8| {
        block.fill(with);
        for (b, k) in block.iter_mut().zip(key) {
            *b ^= k;
        }
    };
    let mut inner = Zeroizing::new([0u8; 32]);
    pad(&mut block, 0x36);
    let mut h = Sha256::new();
    h.update(&block[..]);
    h.update(message);
    h.finalize_into(GenericArray::from_mut_slice(&mut inner[..]));
    let mut out = Zeroizing::new([0u8; 32]);
    pad(&mut block, 0x5c);
    let mut h = Sha256::new();
    h.update(&block[..]);
    h.update(&inner[..]);
    h.finalize_into(GenericArray::from_mut_slice(&mut out[..]));
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

/// `encToUserId`: AES-256-ECB of the two 16-byte halves of `id`
/// (`decrypt` for the inverse).
pub(crate) fn enc_to_user_id(key: &Key32, id: &[u8; 32], decrypt: bool) -> [u8; 32] {
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
    use dashcore::secp256k1::Secp256k1;

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
        let secp = Secp256k1::new();
        for (a, b) in [(1u8, 2u8), (0x42, 0xC0), (0x0D, 0xFE)] {
            let sk = SecretKey::from_slice(&[a; 32]).unwrap();
            let peer = PublicKey::from_secret_key(&secp, &SecretKey::from_slice(&[b; 32]).unwrap());
            assert_eq!(
                *ecdh(&sk, &peer),
                platform_encryption::derive_shared_key_ecdh(&sk, &peer)
            );
        }
    }

    #[test]
    fn hmac_is_rfc_4231() {
        // RFC 4231 test case 2 has a 4-byte key; pad it to 32 with zeros,
        // which HMAC does itself, so the MAC is unchanged.
        let mut k = [0u8; 32];
        k[..4].copy_from_slice(b"Jefe");
        assert_eq!(
            hex::encode(*hmac_sha256(&k, b"what do ya want for nothing?")),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
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
            let enc = enc_to_user_id(&k, &id, false);
            assert_eq!(enc, platform_encryption::encrypt_enc_to_user_id(&k, &id));
            assert_eq!(enc_to_user_id(&k, &enc, true), id);
            assert_eq!(
                enc_to_user_id(&k, &enc, true),
                platform_encryption::decrypt_enc_to_user_id(&k, &enc)
            );
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
