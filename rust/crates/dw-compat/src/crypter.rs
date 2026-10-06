//! Dash Core's wallet encryption (`src/wallet/crypter.cpp`).
//!
//! - The wallet passphrase becomes an AES-256 key and IV through
//!   `BytesToKeySHA512AES`: SHA-512 over `passphrase ‖ salt`, then
//!   `rounds − 1` more SHA-512 passes; key = bytes 0..32, IV = bytes 32..48.
//! - That key decrypts the `mkey` record's `vchCryptedKey` (AES-256-CBC with
//!   PKCS#7 padding) into the 32-byte master key.
//! - Each secret (private key, and in Dash descriptor wallets the mnemonic and
//!   its passphrase) is AES-256-CBC under the master key with
//!   IV = the first 16 bytes of `Hash(pubkey)` (double SHA-256).
//!
//! Every intermediate key is held in `Zeroizing` buffers.

use aes::Aes256;
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

/// `WALLET_CRYPTO_SALT_SIZE`.
pub const SALT_LEN: usize = 8;

/// A decoded `CMasterKey` (`mkey` record value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterKeyRecord {
    pub crypted_key: Vec<u8>,
    pub salt: Vec<u8>,
    /// 0 = `EVP_sha512` (the only method Core implements).
    pub derivation_method: u32,
    pub rounds: u32,
    pub other_params: Vec<u8>,
}

/// `CCrypter::BytesToKeySHA512AES` → (key, iv). `None` for zero rounds or a
/// salt that is not 8 bytes, as `SetKeyFromPassphrase` refuses them.
pub fn key_from_passphrase(
    passphrase: &[u8],
    salt: &[u8],
    rounds: u32,
) -> Option<(Zeroizing<[u8; 32]>, [u8; 16])> {
    if rounds == 0 || salt.len() != SALT_LEN {
        return None;
    }
    let mut buf = Zeroizing::new([0u8; 64]);
    let mut h = Sha512::new();
    h.update(passphrase);
    h.update(salt);
    buf.copy_from_slice(&h.finalize());
    for _ in 1..rounds {
        let next = Sha512::digest(&buf[..]);
        buf.copy_from_slice(&next);
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&buf[..32]);
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&buf[32..48]);
    Some((key, iv))
}

/// AES-256-CBC decryption with PKCS#7 padding (`CCrypter::Decrypt`). `None`
/// when the padding does not verify, which is how a wrong key usually shows.
pub fn aes_cbc_decrypt(key: &[u8; 32], iv: &[u8; 16], ct: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if ct.is_empty() || ct.len() % 16 != 0 {
        return None;
    }
    let mut buf = Zeroizing::new(ct.to_vec());
    let len = cbc::Decryptor::<Aes256>::new(key.into(), iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?
        .len();
    buf.truncate(len);
    Some(buf)
}

/// AES-256-CBC encryption with PKCS#7 padding (`CCrypter::Encrypt`).
pub fn aes_cbc_encrypt(key: &[u8; 32], iv: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    cbc::Encryptor::<Aes256>::new(key.into(), iv.into()).encrypt_padded_vec_mut::<Pkcs7>(plain)
}

/// Decrypts the wallet's master key with the passphrase. `None` for a wrong
/// passphrase (bad padding or a plaintext that is not 32 bytes) or an
/// unsupported derivation method.
pub fn decrypt_master_key(
    record: &MasterKeyRecord,
    passphrase: &[u8],
) -> Option<Zeroizing<[u8; 32]>> {
    if record.derivation_method != 0 {
        return None;
    }
    let (key, iv) = key_from_passphrase(passphrase, &record.salt, record.rounds)?;
    let plain = aes_cbc_decrypt(&key, &iv, &record.crypted_key)?;
    let arr: [u8; 32] = plain[..].try_into().ok()?;
    Some(Zeroizing::new(arr))
}

/// The IV Core uses for a secret bound to `pubkey`: `Hash(pubkey)[..16]`.
pub fn pubkey_iv(pubkey: &[u8]) -> [u8; 16] {
    let h = Sha256::digest(Sha256::digest(pubkey));
    let mut iv = [0u8; 16];
    iv.copy_from_slice(&h[..16]);
    iv
}

/// `DecryptSecret(master, ciphertext, Hash(pubkey))`.
pub fn decrypt_secret(master: &[u8; 32], ct: &[u8], pubkey: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    aes_cbc_decrypt(master, &pubkey_iv(pubkey), ct)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cbc_round_trip_and_padding() {
        let key = [7u8; 32];
        let iv = [9u8; 16];
        for len in [0usize, 1, 15, 16, 17, 64] {
            let plain: Vec<u8> = (0..len as u8).collect();
            let ct = aes_cbc_encrypt(&key, &iv, &plain);
            assert_eq!(ct.len(), (len / 16 + 1) * 16);
            assert_eq!(&aes_cbc_decrypt(&key, &iv, &ct).unwrap()[..], &plain[..]);
        }
        let ct = aes_cbc_encrypt(&key, &iv, b"secret");
        assert!(aes_cbc_decrypt(&[8u8; 32], &iv, &ct).is_none_or(|p| &p[..] != b"secret"));
    }

    #[test]
    fn master_key_round_trip() {
        let salt = [1u8; 8];
        let (k, iv) = key_from_passphrase(b"pass", &salt, 25_000).unwrap();
        let master = [0x42u8; 32];
        let record = MasterKeyRecord {
            crypted_key: aes_cbc_encrypt(&k, &iv, &master),
            salt: salt.to_vec(),
            derivation_method: 0,
            rounds: 25_000,
            other_params: Vec::new(),
        };
        assert_eq!(*decrypt_master_key(&record, b"pass").unwrap(), master);
        assert!(decrypt_master_key(&record, b"wrong").is_none());
        assert!(key_from_passphrase(b"pass", &[0u8; 7], 1).is_none());
        assert!(key_from_passphrase(b"pass", &salt, 0).is_none());
    }
}
