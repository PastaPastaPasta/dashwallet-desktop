//! Dash Core message signing (`src/util/message.cpp`): `signmessage`,
//! `signmessagewithprivkey` and `verifymessage`, and dash-qt's Sign/Verify
//! Message dialog texts.
//!
//! The signed digest is `SHA256d(ser("DarkCoin Signed Message:\n") ‖ ser(message))`
//! where `ser` is a CompactSize length prefix followed by the bytes. The
//! signature is 65 bytes: header `27 + recid (+4 if compressed)` then
//! `r ‖ s`, encoded as base64.
//!
//! Verification follows Core exactly, including what it tolerates: any
//! header byte is accepted (`recid = (h - 27) & 3`, compressed flag
//! `(h - 27) & 4`), and base64 must be strict (length a multiple of 4, at most
//! two `=`, no stray bits).

use dashcore::Network;
use dashcore::hashes::{Hash, HashEngine, hash160, sha256d};
use dashcore::secp256k1::{self, Message, Secp256k1, SecretKey, ecdsa};
use dw_uri::keyio::{self, Destination, Secret};
use std::fmt;

/// The magic prefix Dash Core signs messages under.
pub const MESSAGE_MAGIC: &str = "DarkCoin Signed Message:\n";

/// Size of a compact recoverable signature.
pub const COMPACT_SIGNATURE_SIZE: usize = 65;

/// `MessageHash(message)`.
pub fn message_hash(message: &[u8]) -> [u8; 32] {
    let mut engine = sha256d::Hash::engine();
    write_compact_size(&mut engine, MESSAGE_MAGIC.len() as u64);
    engine.input(MESSAGE_MAGIC.as_bytes());
    write_compact_size(&mut engine, message.len() as u64);
    engine.input(message);
    sha256d::Hash::from_engine(engine).to_byte_array()
}

fn write_compact_size<E: HashEngine>(engine: &mut E, n: u64) {
    if n < 0xfd {
        engine.input(&[n as u8]);
    } else if n <= 0xffff {
        engine.input(&[0xfd]);
        engine.input(&(n as u16).to_le_bytes());
    } else if n <= 0xffff_ffff {
        engine.input(&[0xfe]);
        engine.input(&(n as u32).to_le_bytes());
    } else {
        engine.input(&[0xff]);
        engine.input(&n.to_le_bytes());
    }
}

/// Why signing failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignError {
    /// The WIF did not decode for the network, or the scalar is out of range
    /// (RPC: "Invalid private key").
    InvalidPrivateKey,
}

impl fmt::Display for SignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SignError::InvalidPrivateKey => f.write_str("Invalid private key"),
        }
    }
}

impl std::error::Error for SignError {}

/// `MessageSign`: base64 of the 65-byte compact signature. Signing is
/// deterministic (RFC 6979, no extra entropy), as in Core's `SignCompact`.
pub fn sign_message(secret: &Secret, message: &[u8]) -> Result<String, SignError> {
    let sk = SecretKey::from_byte_array(&secret.key).map_err(|_| SignError::InvalidPrivateKey)?;
    let secp = Secp256k1::signing_only();
    let sig = secp.sign_ecdsa_recoverable(&Message::from_digest(message_hash(message)), &sk);
    let (recid, rs) = sig.serialize_compact();
    let mut out = [0u8; COMPACT_SIGNATURE_SIZE];
    out[0] = 27 + i32::from(recid) as u8 + if secret.compressed { 4 } else { 0 };
    out[1..].copy_from_slice(&rs);
    Ok(encode_base64(&out))
}

/// `signmessagewithprivkey <wif> <message>`.
pub fn sign_message_with_wif(
    wif: &str,
    network: Network,
    message: &[u8],
) -> Result<String, SignError> {
    let secret = keyio::decode_secret(wif, network).ok_or(SignError::InvalidPrivateKey)?;
    sign_message(&secret, message)
}

/// `MessageVerificationResult` other than `OK`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// The address does not decode on the network.
    InvalidAddress,
    /// The address is valid but not P2PKH.
    AddressNoKey,
    /// The signature is not strict base64.
    MalformedSignature,
    /// No public key could be recovered (wrong length or invalid signature).
    PubkeyNotRecovered,
    /// A key was recovered but it does not belong to the address.
    NotSigned,
}

impl VerifyError {
    /// The error `verifymessage` raises, or `None` where the RPC returns
    /// `false` instead of an error.
    pub fn rpc_message(self) -> Option<&'static str> {
        match self {
            VerifyError::InvalidAddress => Some("Invalid address"),
            VerifyError::AddressNoKey => Some("Address does not refer to key"),
            VerifyError::MalformedSignature => Some("Malformed base64 encoding"),
            VerifyError::PubkeyNotRecovered | VerifyError::NotSigned => None,
        }
    }

    /// The status text of dash-qt's Verify Message tab (without the `<nobr>`
    /// markup dash-qt wraps some of them in).
    pub fn gui_message(self) -> &'static str {
        match self {
            VerifyError::InvalidAddress => {
                "The entered address is invalid. Please check the address and try again."
            }
            VerifyError::AddressNoKey => {
                "The entered address does not refer to a key. Please check the address and try again."
            }
            VerifyError::MalformedSignature => {
                "The signature could not be decoded. Please check the signature and try again."
            }
            VerifyError::PubkeyNotRecovered => {
                "The signature did not match the message digest. Please check the signature and try again."
            }
            VerifyError::NotSigned => "Message verification failed.",
        }
    }
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.gui_message())
    }
}

impl std::error::Error for VerifyError {}

/// dash-qt's Verify Message success text.
pub const GUI_VERIFIED: &str = "Message verified.";
/// dash-qt's Sign Message success text.
pub const GUI_SIGNED: &str = "Message signed.";

/// `MessageVerify(address, signature, message)`.
pub fn verify_message(
    address: &str,
    signature: &str,
    message: &[u8],
    network: Network,
) -> Result<(), VerifyError> {
    let dest =
        keyio::decode_destination(address, network).map_err(|_| VerifyError::InvalidAddress)?;
    let Destination::PubKeyHash(want) = dest else {
        return Err(VerifyError::AddressNoKey);
    };
    let sig = decode_base64_strict(signature).ok_or(VerifyError::MalformedSignature)?;
    let pubkey = recover_pubkey(&sig, message).ok_or(VerifyError::PubkeyNotRecovered)?;
    if hash160::Hash::hash(&pubkey).to_byte_array() != want {
        return Err(VerifyError::NotSigned);
    }
    Ok(())
}

/// `CPubKey::RecoverCompact(MessageHash(message), sig)`: the serialized public
/// key (33 or 65 bytes, as the header's compressed flag says), or `None`.
pub fn recover_pubkey(sig: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    if sig.len() != COMPACT_SIGNATURE_SIZE {
        return None;
    }
    // Core computes with `int`: (vchSig[0] - 27) can be negative and the
    // bit masks then act on the two's-complement value.
    let h = i32::from(sig[0]) - 27;
    let recid = ecdsa::RecoveryId::try_from(h & 3).ok()?;
    let compressed = h & 4 != 0;
    let rsig = ecdsa::RecoverableSignature::from_compact(&sig[1..], recid).ok()?;
    let secp = Secp256k1::verification_only();
    let pk: secp256k1::PublicKey = secp
        .recover_ecdsa(&Message::from_digest(message_hash(message)), &rsig)
        .ok()?;
    Some(if compressed {
        pk.serialize().to_vec()
    } else {
        pk.serialize_uncompressed().to_vec()
    })
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | u32::from(b) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(B64[(n >> (18 - 6 * i) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Core's `DecodeBase64`: length must be a multiple of 4; up to two trailing
/// `=` are removed; every other character must be in the alphabet; leftover
/// bits after the last full byte must be zero.
pub fn decode_base64_strict(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(4) {
        return None;
    }
    let mut body = s.as_bytes();
    for _ in 0..2 {
        if let Some((&b'=', rest)) = body.split_last() {
            body = rest;
        }
    }
    let mut out = Vec::with_capacity(body.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &c in body {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // ConvertBits<6, 8, false>: fewer than 6 leftover bits, all zero.
    if bits >= 6 || acc != 0 {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip_and_strictness() {
        for len in 0..10 {
            let data: Vec<u8> = (0..len).map(|i: u32| (i * 37) as u8).collect();
            assert_eq!(decode_base64_strict(&encode_base64(&data)).unwrap(), data);
        }
        assert!(decode_base64_strict("QQ").is_none()); // not a multiple of 4
        assert!(decode_base64_strict("QR==").is_none()); // stray bits
        assert_eq!(decode_base64_strict("QQ==").unwrap(), b"A");
        assert!(decode_base64_strict("Q===").is_none());
    }

    #[test]
    fn hash_matches_core_message_hash_test() {
        // util_tests.cpp message_hash: Hash(ser(MAGIC) || ser("..."))
        let mut pre = vec![MESSAGE_MAGIC.len() as u8];
        pre.extend_from_slice(MESSAGE_MAGIC.as_bytes());
        pre.push(3);
        pre.extend_from_slice(b"...");
        assert_eq!(
            message_hash(b"..."),
            sha256d::Hash::hash(&pre).to_byte_array()
        );
        assert_ne!(
            message_hash(b"..."),
            sha256d::Hash::hash(b"...").to_byte_array()
        );
    }
}
