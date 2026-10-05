//! Recovery phrases: generation, live validation for the restore screen and
//! seed derivation (strict BIP39, or Dash Core's quirks through dw-compat).
//!
//! Every phrase and passphrase is handled as bytes in `Zeroizing` buffers.

use dw_compat::bip39core;
use key_wallet::Network;
use key_wallet::mnemonic::Mnemonic;
use key_wallet::wallet::Wallet;
use key_wallet::wallet::root_extended_keys::RootExtendedPrivKey;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, Zeroizing};

/// BIP39 wordlist languages (key-wallet's enum, re-exported for callers that
/// do not depend on key-wallet).
pub use key_wallet::mnemonic::Language;

use crate::MnemonicError;
use crate::types::{SeedDerivation, WalletId, WalletSecret};

/// Word counts BIP39 defines.
pub const WORD_COUNTS: [u32; 5] = [12, 15, 18, 21, 24];

/// A fresh phrase of `word_count` words in `language`, as UTF-8 bytes. Nothing
/// is stored.
pub fn generate(word_count: u32, language: Language) -> Result<Zeroizing<Vec<u8>>, MnemonicError> {
    if !WORD_COUNTS.contains(&word_count) {
        return Err(MnemonicError::UnsupportedWordCount(word_count));
    }
    let mut entropy = Zeroizing::new(vec![0u8; (word_count as usize / 3) * 4]);
    getrandom::getrandom(&mut entropy[..]).map_err(|e| MnemonicError::Entropy(e.to_string()))?;
    let mnemonic = Mnemonic::from_entropy(&entropy, language)
        .map_err(|e| MnemonicError::Invalid(e.to_string()))?;
    let phrase = Zeroizing::new(mnemonic.phrase());
    Ok(Zeroizing::new(phrase.as_bytes().to_vec()))
}

/// Checksum verdict of [`check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Checksum {
    Valid,
    /// Fails BIP39, passes Dash Core's weak check (English only); importable
    /// with Core compatibility.
    CoreOnly,
    Invalid,
}

/// Word-by-word validation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MnemonicCheck {
    pub word_count: u32,
    /// Zero-based positions of words not in the detected (or closest) wordlist.
    pub unknown_word_indices: Vec<u32>,
    /// `None` when no single wordlist contains every word.
    pub language: Option<Language>,
    pub checksum: Checksum,
}

/// NFKD, lower case, single spaces (key-wallet's input tolerance).
fn normalized(phrase: &str) -> Zeroizing<String> {
    let mut nfkd = Zeroizing::new(String::with_capacity(phrase.len() * 2));
    nfkd.extend(phrase.nfkd());
    let mut out = Zeroizing::new(String::with_capacity(nfkd.len()));
    for word in nfkd.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.extend(word.chars().flat_map(char::to_lowercase));
    }
    out
}

/// Validates a typed or pasted phrase without deriving anything from it.
pub fn check(phrase: &[u8]) -> MnemonicCheck {
    let Ok(text) = std::str::from_utf8(phrase) else {
        return MnemonicCheck {
            word_count: 0,
            unknown_word_indices: Vec::new(),
            language: None,
            checksum: Checksum::Invalid,
        };
    };
    let norm = normalized(text);
    let words: Vec<&str> = norm.split(' ').filter(|w| !w.is_empty()).collect();
    let word_count = words.len() as u32;

    // The wordlist with the fewest unknown words; English wins ties.
    let mut best: Option<(Language, Vec<u32>)> = None;
    for language in Language::ALL {
        // The bip39 wordlists are NFKD already (bip39 parses NFKD input
        // against them), so the normalized words compare directly.
        let list = language.word_list();
        let unknown: Vec<u32> = words
            .iter()
            .enumerate()
            .filter(|(_, w)| !list.contains(w))
            .map(|(i, _)| i as u32)
            .collect();
        if best.as_ref().is_none_or(|(_, u)| unknown.len() < u.len()) {
            best = Some((language, unknown));
        }
    }
    let (closest, unknown_word_indices) = best.unwrap_or((Language::English, Vec::new()));
    let language = (unknown_word_indices.is_empty() && !words.is_empty()).then_some(closest);

    let checksum = if language.is_none() || !WORD_COUNTS.contains(&word_count) {
        Checksum::Invalid
    } else if Mnemonic::from_phrase(&norm).is_ok() {
        Checksum::Valid
    } else if language == Some(Language::English)
        && bip39core::core_check(bip39core::normalize_phrase(&norm).as_bytes())
    {
        Checksum::CoreOnly
    } else {
        Checksum::Invalid
    };
    MnemonicCheck {
        word_count,
        unknown_word_indices,
        language,
        checksum,
    }
}

/// Derives the wallet seed from a phrase and BIP39 passphrase.
///
/// - `core_compat = false`: strict BIP39 in any supported language; the
///   passphrase must be UTF-8 (NFKD-normalized as BIP39 requires).
/// - `core_compat = true` (dash-qt restore, QT-104): English only, strict
///   BIP39 checksum first and Dash Core's weak checksum second (flagged in
///   [`SeedDerivation::DashCore::weak_checksum`]); seed from
///   `CMnemonic::ToSeed` (no NFKD, salt cut at 256 bytes), passphrase bytes
///   as given.
pub fn derive_secret(
    phrase: &[u8],
    bip39_passphrase: &[u8],
    core_compat: bool,
) -> Result<WalletSecret, MnemonicError> {
    let text = std::str::from_utf8(phrase)
        .map_err(|_| MnemonicError::Invalid("phrase is not UTF-8".into()))?;
    if core_compat {
        let v = bip39core::validate(text).map_err(|e| {
            MnemonicError::Invalid(format!("not accepted by Dash Core (English only): {e:?}"))
        })?;
        let seed = bip39core::core_seed(v.phrase.as_bytes(), bip39_passphrase);
        return Ok(WalletSecret {
            mnemonic: Zeroizing::new(v.phrase.as_bytes().to_vec()),
            mnemonic_passphrase: Zeroizing::new(bip39_passphrase.to_vec()),
            seed,
            derivation: SeedDerivation::DashCore {
                weak_checksum: v.weak,
            },
        });
    }
    let passphrase = std::str::from_utf8(bip39_passphrase).map_err(|_| MnemonicError::PassphraseNotUtf8)?;
    let mnemonic = Mnemonic::from_phrase(&normalized(text))
        .map_err(|e| MnemonicError::Invalid(e.to_string()))?;
    let mut raw = mnemonic.to_seed(passphrase);
    let seed = Zeroizing::new(raw);
    raw.zeroize();
    let canonical = Zeroizing::new(mnemonic.phrase());
    Ok(WalletSecret {
        mnemonic: Zeroizing::new(canonical.as_bytes().to_vec()),
        mnemonic_passphrase: Zeroizing::new(bip39_passphrase.to_vec()),
        seed,
        derivation: SeedDerivation::Bip39,
    })
}

/// The network-scoped wallet id platform-wallet assigns to a wallet built
/// from `seed` (`Wallet::compute_wallet_id`).
pub fn wallet_id_for_seed(seed: &[u8; 64], network: Network) -> Result<WalletId, MnemonicError> {
    let root = RootExtendedPrivKey::new_master(seed).map_err(|e| MnemonicError::Invalid(e.to_string()))?;
    let root_pub = root.to_root_extended_pub_key();
    Ok(Wallet::compute_wallet_id_from_root_extended_pub_key(&root_pub, Some(network)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABANDON: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn generate_word_counts() {
        for n in WORD_COUNTS {
            let p = generate(n, Language::English).unwrap();
            let text = std::str::from_utf8(&p).unwrap();
            assert_eq!(text.split(' ').count() as u32, n);
            assert_eq!(check(&p).checksum, Checksum::Valid);
        }
        assert_eq!(
            generate(13, Language::English).unwrap_err(),
            MnemonicError::UnsupportedWordCount(13)
        );
        let es = generate(12, Language::Spanish).unwrap();
        assert_eq!(check(&es).language, Some(Language::Spanish));
    }

    #[test]
    fn check_reports_unknown_words_and_checksums() {
        let c = check(ABANDON.to_uppercase().as_bytes());
        assert_eq!(c.checksum, Checksum::Valid);
        assert_eq!(c.language, Some(Language::English));
        let typo = ABANDON.replace("about", "abuot");
        let c = check(typo.as_bytes());
        assert_eq!(c.unknown_word_indices, vec![11]);
        assert_eq!(c.language, None);
        assert_eq!(c.checksum, Checksum::Invalid);
        let bad_sum = ABANDON.replace("about", "abandon");
        assert_ne!(check(bad_sum.as_bytes()).checksum, Checksum::Valid);
        assert_eq!(check(&[0xff]).checksum, Checksum::Invalid);
    }

    #[test]
    fn strict_seed_matches_bip39_vector() {
        let s = derive_secret(ABANDON.as_bytes(), b"TREZOR", false).unwrap();
        assert_eq!(
            hex::encode(&s.seed[..]),
            "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04"
        );
        assert_eq!(s.derivation, SeedDerivation::Bip39);
        assert_eq!(&s.mnemonic[..], ABANDON.as_bytes());
        assert!(matches!(
            derive_secret(ABANDON.as_bytes(), &[0xff], false),
            Err(MnemonicError::PassphraseNotUtf8)
        ));
    }

    #[test]
    fn wallet_id_matches_key_wallet() {
        let s = derive_secret(ABANDON.as_bytes(), b"", false).unwrap();
        let w = Wallet::from_seed_bytes(
            *s.seed,
            Network::Regtest,
            key_wallet::wallet::initialization::WalletAccountCreationOptions::None,
        )
        .unwrap();
        assert_eq!(wallet_id_for_seed(&s.seed, Network::Regtest).unwrap(), w.compute_wallet_id());
        assert_ne!(
            wallet_id_for_seed(&s.seed, Network::Regtest).unwrap(),
            wallet_id_for_seed(&s.seed, Network::Testnet).unwrap()
        );
    }
}
