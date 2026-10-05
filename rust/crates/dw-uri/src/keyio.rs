//! Port of Dash Core `src/key_io.cpp` address and WIF handling, plus the
//! Platform (DIP-18) and shielded classification the iOS wallet's
//! `DashAddressClassifier` performs.
//!
//! Error strings are Dash Core's, so they can be shown exactly as dash-qt
//! shows them. `testdata/address_cases.json` holds dashd's verdicts.

use dashcore::Network;
use dashcore::hashes::{Hash, sha256d};
use std::fmt;
use zeroize::Zeroizing;

/// An L1 destination that Dash Core can encode as a Base58 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Destination {
    /// P2PKH: HASH160 of a public key.
    PubKeyHash([u8; 20]),
    /// P2SH: HASH160 of a redeem script.
    ScriptHash([u8; 20]),
}

/// Why a string is not a Dash Core destination. `Display` gives Dash Core's
/// message for the case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationError {
    InvalidBase58Length,
    InvalidBase58Prefix,
    NotBech32mOrBase58,
    InvalidBase58ChecksumOrLength,
    /// A valid DIP-18 Platform address for this network.
    PlatformAddress,
    /// A bech32 problem found before or instead of decoding (Core's
    /// `bech32::LocateErrors` and `DecodePlatformDestination` messages).
    Bech32(&'static str),
}

impl fmt::Display for DestinationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DestinationError::InvalidBase58Length => "Invalid length for Base58 address",
            DestinationError::InvalidBase58Prefix => "Invalid prefix for Base58-encoded address",
            DestinationError::NotBech32mOrBase58 => "Not a valid Bech32m or Base58 encoding",
            DestinationError::InvalidBase58ChecksumOrLength => {
                "Invalid checksum or length of Base58 address"
            }
            DestinationError::PlatformAddress => {
                "This is a Dash Platform address, not a Dash Core address"
            }
            DestinationError::Bech32(msg) => msg,
        })
    }
}

impl std::error::Error for DestinationError {}

/// Base58 version bytes and the Platform HRP for a network
/// (`chainparams.cpp`). Devnet and regtest use the testnet values.
struct Params {
    pubkey: u8,
    script: u8,
    secret: u8,
    platform_hrp: &'static str,
}

fn params(network: Network) -> Params {
    match network {
        Network::Mainnet => Params {
            pubkey: 76,
            script: 16,
            secret: 204,
            platform_hrp: "dash",
        },
        _ => Params {
            pubkey: 140,
            script: 19,
            secret: 239,
            platform_hrp: "tdash",
        },
    }
}

/// DIP-18 type byte for a Platform P2PKH address.
pub const DIP18_TYPE_BYTE_P2PKH: u8 = 0xb0;
/// DIP-18 type byte for a Platform P2SH address.
pub const DIP18_TYPE_BYTE_P2SH: u8 = 0x80;
/// Type byte of the shielded (Orchard) display form used by the iOS wallet.
pub const SHIELDED_TYPE_BYTE: u8 = 0x10;

const BASE58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Core's `IsSpace`: the six C-locale whitespace characters.
fn is_core_space(b: u8) -> bool {
    matches!(b, b' ' | b'\x0c' | b'\n' | b'\r' | b'\t' | b'\x0b')
}

/// Core's `DecodeBase58`: leading and trailing whitespace is skipped; the
/// decoded length may not exceed `max_ret_len`; a NUL byte ends the C string,
/// so anything after it makes the input invalid.
fn decode_base58_core(s: &str, max_ret_len: usize) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && is_core_space(b[i]) {
        i += 1;
    }
    let mut zeroes = 0usize;
    while i < b.len() && b[i] == b'1' {
        zeroes += 1;
        if zeroes > max_ret_len {
            return None;
        }
        i += 1;
    }
    let mut b256: Vec<u8> = Vec::new(); // little-endian base-256 digits
    while i < b.len() && !is_core_space(b[i]) {
        let mut carry = BASE58_ALPHABET.iter().position(|&c| c == b[i])? as u32;
        for digit in b256.iter_mut() {
            carry += 58 * u32::from(*digit);
            *digit = (carry % 256) as u8;
            carry /= 256;
        }
        while carry > 0 {
            b256.push((carry % 256) as u8);
            carry /= 256;
        }
        if b256.len() + zeroes > max_ret_len {
            return None;
        }
        i += 1;
    }
    while i < b.len() && is_core_space(b[i]) {
        i += 1;
    }
    if i != b.len() {
        return None;
    }
    let mut out = vec![0u8; zeroes];
    out.extend(b256.iter().rev());
    Some(out)
}

/// Core's `DecodeBase58Check(str, out, max_ret_len)`.
fn decode_base58_check_core(s: &str, max_ret_len: usize) -> Option<Vec<u8>> {
    if s.as_bytes().contains(&0) {
        return None;
    }
    let data = decode_base58_core(s, max_ret_len + 4)?;
    if data.len() < 4 {
        return None;
    }
    let (payload, checksum) = data.split_at(data.len() - 4);
    let hash = sha256d::Hash::hash(payload);
    if &hash.as_byte_array()[..4] != checksum {
        return None;
    }
    Some(payload.to_vec())
}

fn encode_base58_check(payload: &[u8]) -> String {
    dashcore::base58::encode_check(payload)
}

/// Encodes a destination as a Base58 address for `network`.
pub fn encode_destination(dest: &Destination, network: Network) -> String {
    let p = params(network);
    let mut data = Vec::with_capacity(21);
    match dest {
        Destination::PubKeyHash(h) => {
            data.push(p.pubkey);
            data.extend_from_slice(h);
        }
        Destination::ScriptHash(h) => {
            data.push(p.script);
            data.extend_from_slice(h);
        }
    }
    encode_base58_check(&data)
}

/// `DecodeDestination(str, params, error_str)`.
pub fn decode_destination(s: &str, network: Network) -> Result<Destination, DestinationError> {
    let p = params(network);
    // Core: `ToLower(str.substr(0, hrp.size())) == hrp`. A prefix that does
    // not end on a char boundary contains non-ASCII bytes and cannot match.
    let is_bech32 = s
        .get(..p.platform_hrp.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(p.platform_hrp));
    if !is_bech32 && let Some(data) = decode_base58_check_core(s, 21) {
        if data.len() == 21 && data[0] == p.pubkey {
            return Ok(Destination::PubKeyHash(
                data[1..].try_into().expect("20 bytes"),
            ));
        }
        if data.len() == 21 && data[0] == p.script {
            return Ok(Destination::ScriptHash(
                data[1..].try_into().expect("20 bytes"),
            ));
        }
        if data
            .first()
            .is_some_and(|&v| v == p.pubkey || v == p.script)
        {
            return Err(DestinationError::InvalidBase58Length);
        }
        return Err(DestinationError::InvalidBase58Prefix);
    }
    if !is_bech32 {
        return Err(if decode_base58_core(s, 100).is_some() {
            DestinationError::InvalidBase58ChecksumOrLength
        } else {
            DestinationError::NotBech32mOrBase58
        });
    }
    let platform = decode_platform_destination(s, network);
    let (located, _) = crate::bech32_locate::locate_errors(s);
    if !located.is_empty() {
        return Err(DestinationError::Bech32(located));
    }
    Err(match platform {
        Ok(_) => DestinationError::PlatformAddress,
        Err(msg) => DestinationError::Bech32(msg),
    })
}

/// Character positions dash-qt highlights for a bech32 string that failed
/// to decode (Core's `bech32::LocateErrors`), with Core's message. Empty
/// message and positions when the string is valid bech32/bech32m.
pub fn locate_bech32_errors(s: &str) -> (&'static str, Vec<usize>) {
    crate::bech32_locate::locate_errors(s)
}

/// A decoded DIP-18 Platform address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlatformDestination {
    PubKeyHash([u8; 20]),
    ScriptHash([u8; 20]),
}

/// bech32m decode of `s` into (hrp, 8-bit payload) with Core's
/// `ConvertBits<5, 8, false>` rules. `Err` carries Core's message.
fn bech32m_payload(s: &str) -> Result<(String, Vec<u8>), &'static str> {
    use dashcore::bech32::{self, FromBase32};
    let (hrp, data, variant) = bech32::decode(s).map_err(|_| "Invalid bech32m encoding")?;
    if variant != bech32::Variant::Bech32m {
        return Err("DIP-18 Platform addresses require bech32m checksum");
    }
    let payload =
        Vec::<u8>::from_base32(&data).map_err(|_| "Invalid Platform address payload encoding")?;
    Ok((hrp, payload))
}

/// `DecodePlatformDestination(str, params, error_str)`.
pub fn decode_platform_destination(
    s: &str,
    network: Network,
) -> Result<PlatformDestination, &'static str> {
    let (hrp, payload) = bech32m_payload(s)?;
    if hrp != params(network).platform_hrp {
        return Err("Invalid Platform HRP for the selected network");
    }
    if payload.len() != 21 {
        return Err("Invalid Platform address payload length");
    }
    let hash: [u8; 20] = payload[1..].try_into().expect("20 bytes");
    match payload[0] {
        DIP18_TYPE_BYTE_P2PKH => Ok(PlatformDestination::PubKeyHash(hash)),
        DIP18_TYPE_BYTE_P2SH => Ok(PlatformDestination::ScriptHash(hash)),
        _ => Err("Unknown DIP-18 type byte"),
    }
}

/// What a pasted or scanned destination string is (iOS
/// `DashAddressClassifier.classify` plus Core's error for the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressKind {
    /// Base58 L1 address for the network.
    Core(Destination),
    /// DIP-18 bech32m Platform address for the network.
    Platform(PlatformDestination),
    /// bech32m shielded address: type byte 0x10 then 43 raw Orchard bytes.
    Shielded(Box<[u8; 43]>),
    /// None of the above; Core's reason.
    Invalid(DestinationError),
}

/// Classifies `text` (surrounding whitespace trimmed) for `network`.
///
/// Mixed-case bech32 is rejected, as BIP-173 and Dash Core require; the iOS
/// classifier lower-cases its input first and so accepts it.
pub fn classify_address(text: &str, network: Network) -> AddressKind {
    let trimmed = text.trim();
    let err = match decode_destination(trimmed, network) {
        Ok(d) => return AddressKind::Core(d),
        Err(e) => e,
    };
    if let Ok(p) = decode_platform_destination(trimmed, network) {
        return AddressKind::Platform(p);
    }
    if let Ok((hrp, payload)) = bech32m_payload(trimmed)
        && hrp == params(network).platform_hrp
        && payload.len() == 44
        && payload[0] == SHIELDED_TYPE_BYTE
    {
        return AddressKind::Shielded(Box::new(payload[1..].try_into().expect("43 bytes")));
    }
    AddressKind::Invalid(err)
}

/// A private key decoded from WIF.
pub struct Secret {
    /// The 32-byte secret scalar.
    pub key: Zeroizing<[u8; 32]>,
    /// Whether the public key is used in compressed form.
    pub compressed: bool,
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("compressed", &self.compressed)
            .finish_non_exhaustive()
    }
}

/// `DecodeSecret`: WIF for `network`. Range checks of the scalar are left to
/// the signing library.
pub fn decode_secret(s: &str, network: Network) -> Option<Secret> {
    let data = Zeroizing::new(decode_base58_check_core(s, 34)?);
    let prefix = params(network).secret;
    let compressed = match data.len() {
        33 => false,
        34 if data[33] == 1 => true,
        _ => return None,
    };
    if data[0] != prefix {
        return None;
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&data[1..33]);
    Some(Secret { key, compressed })
}

/// `EncodeSecret`.
pub fn encode_secret(secret: &Secret, network: Network) -> Zeroizing<String> {
    let mut data = Zeroizing::new(Vec::with_capacity(34));
    data.push(params(network).secret);
    data.extend_from_slice(&secret.key[..]);
    if secret.compressed {
        data.push(1);
    }
    Zeroizing::new(encode_base58_check(&data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_unit_test_addresses() {
        // src/test/util_tests.cpp message_verify
        assert!(matches!(
            decode_destination("XuXS24zs2xP1vPynvNt14kWLa64csH8Mur", Network::Mainnet),
            Ok(Destination::PubKeyHash(_))
        ));
        assert!(matches!(
            decode_destination("7iRPy8FEHBzbChrktsG85YbDZ3SuiCxsNq", Network::Mainnet),
            Ok(Destination::ScriptHash(_))
        ));
        assert_eq!(
            decode_destination("invalid address", Network::Mainnet)
                .unwrap_err()
                .to_string(),
            "Not a valid Bech32m or Base58 encoding"
        );
    }

    #[test]
    fn base58_whitespace_is_skipped_like_core() {
        assert!(
            decode_destination("  XuXS24zs2xP1vPynvNt14kWLa64csH8Mur\n", Network::Mainnet).is_ok()
        );
        assert!(
            decode_destination("XuXS24zs2xP1vPyn vNt14kWLa64csH8Mur", Network::Mainnet).is_err()
        );
    }

    #[test]
    fn wrong_network_is_prefix_error() {
        assert_eq!(
            decode_destination("XuXS24zs2xP1vPynvNt14kWLa64csH8Mur", Network::Testnet).unwrap_err(),
            DestinationError::InvalidBase58Prefix
        );
    }
}
