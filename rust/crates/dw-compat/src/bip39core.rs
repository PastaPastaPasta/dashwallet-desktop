//! Dash Core's BIP39 implementation (`src/wallet/bip39.cpp`, `CMnemonic`)
//! with its non-standard behaviour, next to strict BIP39.
//!
//! Core differs from BIP39 in three ways, all reproduced here:
//! 1. **Weak checksum.** The checksum mask is computed as
//!    `(2 ^ cs) << (8 - cs)` where `^` is XOR, not a power. Only some
//!    checksum bits are compared, so Core accepts phrases that BIP39 rejects:
//!    12 words: 2 of 4 bits; 15: 3 of 5; 18: 1 of 6; 21: 2 of 7; 24: 2 of 8.
//! 2. **Salt cut at 256 bytes.** The PBKDF2 salt `"mnemonic" + passphrase` is
//!    truncated to 256 bytes, so passphrases longer than 248 bytes collide.
//!    Legacy (HD chain) wallets refuse passphrases over 256 bytes; descriptor
//!    wallets accept any length.
//! 3. **No NFKD.** Mnemonic and passphrase bytes are hashed as given.
//!
//! `validate` applies strict BIP39 first and falls back to Core's check,
//! reporting `weak = true` in that case so the UI can warn. Vectors:
//! `testdata/bip39_core_quirks.json`, produced by compiling Core's own
//! `bip39.cpp` and by running dashd.

use dashcore::hashes::{Hash, HashEngine, Hmac, HmacEngine, sha256, sha512};
use std::sync::OnceLock;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

/// PBKDF2 rounds BIP39 uses.
pub const PBKDF2_ROUNDS: u32 = 2048;
/// Core's cut-off for the PBKDF2 salt.
pub const MAX_SALT_BYTES: usize = 256;
/// Longest passphrase a legacy dash-qt wallet accepts (`CHDChain::SetMnemonic`).
pub const LEGACY_MAX_PASSPHRASE_BYTES: usize = 256;

static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();

/// The BIP39 English wordlist, the only list Dash Core supports.
pub fn wordlist() -> &'static [&'static str] {
    WORDS.get_or_init(|| {
        let words: Vec<&'static str> = include_str!("bip39_english.txt").lines().collect();
        assert_eq!(words.len(), 2048, "bip39_english.txt must have 2048 words");
        words
    })
}

fn word_index(word: &[u8]) -> Option<usize> {
    let list = wordlist();
    // The list is sorted, so a binary search finds exact byte matches.
    list.binary_search_by(|w| w.as_bytes().cmp(word)).ok()
}

/// `CMnemonic::Check` over the exact bytes given: words separated by single
/// ASCII spaces, 12/15/18/21/24 words from the English list, each at most
/// eight letters, and Core's masked checksum.
pub fn core_check(mnemonic: &[u8]) -> bool {
    if mnemonic.is_empty() {
        return false;
    }
    let word_count = mnemonic.iter().filter(|&&c| c == b' ').count() + 1;
    if !word_count.is_multiple_of(3) || !(12..=24).contains(&word_count) {
        return false;
    }
    let mut bits = Zeroizing::new([0u8; 33]);
    let mut bit_count = 0usize;
    let mut i = 0;
    while i < mnemonic.len() {
        let start = i;
        while i < mnemonic.len() && mnemonic[i] != b' ' {
            if i - start >= 9 {
                return false;
            }
            i += 1;
        }
        let Some(index) = word_index(&mnemonic[start..i]) else {
            return false;
        };
        for k in 0..11 {
            if index & (1 << (10 - k)) != 0 {
                bits[bit_count / 8] |= 1 << (7 - (bit_count % 8));
            }
            bit_count += 1;
        }
        i += 1; // the separating space
    }
    if bit_count != word_count * 11 {
        return false;
    }
    let entropy_len = word_count * 4 / 3;
    let checksum_byte = bits[entropy_len];
    let hash = sha256::Hash::hash(&bits[..entropy_len]).to_byte_array();
    let cs = word_count / 3;
    let mask = ((2 ^ cs) << (8 - cs)) as u8;
    (hash[0] & mask) == (checksum_byte & mask)
}

/// Why a phrase is not valid BIP39.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bip39Error {
    /// Not 12, 15, 18, 21 or 24 words.
    WordCount(usize),
    /// A word (0-based position) is not in the English list.
    UnknownWord(usize),
    /// The full BIP39 checksum does not match, and Core's check fails too.
    Checksum,
    /// Entropy length is not 16, 20, 24, 28 or 32 bytes.
    EntropyLength(usize),
}

impl std::fmt::Display for Bip39Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bip39Error::WordCount(n) => write!(
                f,
                "a recovery phrase has 12, 15, 18, 21 or 24 words, not {n}"
            ),
            Bip39Error::UnknownWord(i) => {
                write!(f, "word {} is not in the BIP39 English word list", i + 1)
            }
            Bip39Error::Checksum => f.write_str("the recovery phrase checksum is wrong"),
            Bip39Error::EntropyLength(n) => {
                write!(f, "entropy must be 16, 20, 24, 28 or 32 bytes, not {n}")
            }
        }
    }
}

impl std::error::Error for Bip39Error {}

/// Splits a single-spaced phrase into word indices and the packed bits.
fn unpack(phrase: &str) -> Result<(usize, Zeroizing<Vec<u8>>), Bip39Error> {
    let words: Vec<&str> = phrase.split(' ').collect();
    let n = words.len();
    if !n.is_multiple_of(3) || !(12..=24).contains(&n) {
        return Err(Bip39Error::WordCount(n));
    }
    let mut bits = Zeroizing::new(vec![0u8; 33]);
    for (pos, w) in words.iter().enumerate() {
        let index = word_index(w.as_bytes()).ok_or(Bip39Error::UnknownWord(pos))?;
        for k in 0..11 {
            if index & (1 << (10 - k)) != 0 {
                let bit = pos * 11 + k;
                bits[bit / 8] |= 1 << (7 - (bit % 8));
            }
        }
    }
    Ok((n, bits))
}

/// Strict BIP39 check of a single-spaced phrase. Returns the entropy.
pub fn strict_check(phrase: &str) -> Result<Zeroizing<Vec<u8>>, Bip39Error> {
    let (n, bits) = unpack(phrase)?;
    let entropy_len = n * 4 / 3;
    let cs = n / 3;
    let hash = sha256::Hash::hash(&bits[..entropy_len]).to_byte_array();
    let mask = 0xffu8 << (8 - cs);
    if hash[0] & mask != bits[entropy_len] & mask {
        return Err(Bip39Error::Checksum);
    }
    Ok(Zeroizing::new(bits[..entropy_len].to_vec()))
}

/// A phrase that dash-qt would accept.
pub struct ValidatedMnemonic {
    /// The normalized phrase: lower-case, single spaces. This is the exact
    /// byte string Core hashes.
    pub phrase: Zeroizing<String>,
    /// Entropy bits of the phrase (the checksum bits are not included).
    pub entropy: Zeroizing<Vec<u8>>,
    /// True when only Core's weak checksum accepts the phrase. Other BIP39
    /// wallets will reject it.
    pub weak: bool,
}

impl std::fmt::Debug for ValidatedMnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedMnemonic")
            .field("weak", &self.weak)
            .finish_non_exhaustive()
    }
}

/// Lower-cases ASCII letters, joins words with single spaces, trims.
/// Other characters are kept, so non-English words still fail the lookup.
pub fn normalize_phrase(input: &str) -> Zeroizing<String> {
    let mut out = Zeroizing::new(String::with_capacity(input.len()));
    for word in input.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().map(|c| c.to_ascii_lowercase()));
    }
    out
}

/// Normalizes `input`, then validates it with strict BIP39 and, failing
/// that, with Core's weak checksum.
pub fn validate(input: &str) -> Result<ValidatedMnemonic, Bip39Error> {
    let phrase = normalize_phrase(input);
    match strict_check(&phrase) {
        Ok(entropy) => Ok(ValidatedMnemonic {
            phrase,
            entropy,
            weak: false,
        }),
        Err(Bip39Error::Checksum) if core_check(phrase.as_bytes()) => {
            let (n, bits) = unpack(&phrase)?;
            let entropy = Zeroizing::new(bits[..n * 4 / 3].to_vec());
            Ok(ValidatedMnemonic {
                phrase,
                entropy,
                weak: true,
            })
        }
        Err(e) => Err(e),
    }
}

/// `CMnemonic::FromData`: the phrase for 16–32 bytes of entropy (multiple
/// of 4), with the full BIP39 checksum.
pub fn from_entropy(entropy: &[u8]) -> Result<Zeroizing<String>, Bip39Error> {
    let len = entropy.len();
    if !len.is_multiple_of(4) || !(16..=32).contains(&len) {
        return Err(Bip39Error::EntropyLength(len));
    }
    let mut bits = Zeroizing::new(entropy.to_vec());
    bits.push(sha256::Hash::hash(entropy).to_byte_array()[0]);
    let words = len * 3 / 4;
    let list = wordlist();
    let mut out = Zeroizing::new(String::new());
    for i in 0..words {
        let mut idx = 0usize;
        for j in 0..11 {
            let bit = i * 11 + j;
            idx = (idx << 1) | usize::from(bits[bit / 8] & (1 << (7 - bit % 8)) != 0);
        }
        if i > 0 {
            out.push(' ');
        }
        out.push_str(list[idx]);
    }
    Ok(out)
}

/// `CMnemonic::ToSeed`: PBKDF2-HMAC-SHA512 over the mnemonic bytes with salt
/// `("mnemonic" + passphrase)[..256]`, 2048 rounds, no normalization.
pub fn core_seed(mnemonic: &[u8], passphrase: &[u8]) -> Zeroizing<[u8; 64]> {
    let mut salt = Zeroizing::new(Vec::with_capacity(8 + passphrase.len()));
    salt.extend_from_slice(b"mnemonic");
    salt.extend_from_slice(passphrase);
    salt.truncate(MAX_SALT_BYTES);
    pbkdf2_hmac_sha512_one_block(mnemonic, &salt, PBKDF2_ROUNDS)
}

/// PBKDF2-HMAC-SHA512 producing exactly one 64-byte block.
fn pbkdf2_hmac_sha512_one_block(password: &[u8], salt: &[u8], rounds: u32) -> Zeroizing<[u8; 64]> {
    let mac = |data: &[&[u8]]| -> [u8; 64] {
        let mut engine = HmacEngine::<sha512::Hash>::new(password);
        for d in data {
            engine.input(d);
        }
        Hmac::<sha512::Hash>::from_engine(engine).to_byte_array()
    };
    let mut u = Zeroizing::new(mac(&[salt, &1u32.to_be_bytes()]));
    let mut out = Zeroizing::new(*u);
    for _ in 1..rounds {
        *u = mac(&[&u[..]]);
        for (o, x) in out.iter_mut().zip(u.iter()) {
            *o ^= x;
        }
    }
    out
}

/// How Core's seed for this phrase and passphrase relates to the BIP39
/// standard seed other wallets compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedPortability {
    /// The salt exceeds 256 bytes and Core cut it.
    pub salt_truncated: bool,
    /// The phrase or passphrase changes under NFKD, which Core skips.
    pub not_nfkd: bool,
}

impl SeedPortability {
    /// True when other BIP39 wallets derive the same seed.
    pub fn is_portable(self) -> bool {
        !self.salt_truncated && !self.not_nfkd
    }
}

/// Reports whether `core_seed(mnemonic, passphrase)` equals the BIP39
/// standard seed.
pub fn seed_portability(mnemonic: &str, passphrase: &str) -> SeedPortability {
    let nfkd_changes = |s: &str| s.nfkd().ne(s.chars());
    SeedPortability {
        salt_truncated: 8 + passphrase.len() > MAX_SALT_BYTES,
        not_nfkd: nfkd_changes(mnemonic) || nfkd_changes(passphrase),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_match_research_table() {
        let checked = |words: usize| ((2 ^ (words / 3)) << (8 - words / 3)).count_ones();
        assert_eq!([12, 15, 18, 21, 24].map(checked), [2, 3, 1, 2, 2]);
    }

    #[test]
    fn known_phrase() {
        let p = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        assert!(core_check(p.as_bytes()));
        assert_eq!(strict_check(p).unwrap().as_slice(), &[0u8; 16]);
        assert_eq!(from_entropy(&[0u8; 16]).unwrap().as_str(), p);
        assert!(!validate(&p.to_uppercase()).unwrap().weak);
    }

    #[test]
    fn portability() {
        assert!(seed_portability("a", "TREZOR").is_portable());
        assert!(seed_portability("a", &"p".repeat(249)).salt_truncated);
        assert!(!seed_portability("a", &"p".repeat(248)).salt_truncated);
        assert!(seed_portability("a", "caf\u{e9}").not_nfkd);
    }
}
