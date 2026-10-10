// SPDX-License-Identifier: CC0-1.0
//
// Vendored from rust-dashcore key-wallet/src/psbt/error.rs @ 40268cc0
// (see ../../VENDORED.md); taproot, combine and fee variants removed.

use core::fmt;

use dashcore::consensus::encode;
use dashcore::hashes;
use dashcore::io;

use crate::psbt::raw;

/// Enum for marking psbt hash error.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum PsbtHash {
    Ripemd,
    Sha256,
    Hash160,
    Hash256,
}
/// Ways that a Partially Signed Transaction might fail.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Magic bytes for a PSBT must be the ASCII for "psbt" serialized in most
    /// significant byte order.
    InvalidMagic,
    /// The separator for a PSBT must be `0xff`.
    InvalidSeparator,
    /// Known keys must be according to spec.
    InvalidKey(raw::Key),
    /// Non-proprietary key type found when proprietary key was expected
    InvalidProprietaryKey,
    /// Keys within key-value map should never be duplicated.
    DuplicateKey(raw::Key),
    /// The scriptSigs for the unsigned transaction must be empty.
    UnsignedTxHasScriptSigs,
    /// The scriptWitnesses for the unsigned transaction must be empty.
    UnsignedTxHasScriptWitnesses,
    /// A PSBT must have an unsigned transaction.
    MustHaveUnsignedTx,
    /// Signals that there are no more key-value pairs in a key-value map.
    NoMorePairs,
    /// Unable to parse as a standard sighash type.
    NonStandardSighashType(u32),
    /// Parsing errors from bitcoin_hashes
    HashParse(hashes::Error),
    /// The pre-image must hash to the corresponding psbt hash
    InvalidPreimageHashPair {
        /// Hash-type
        hash_type: PsbtHash,
        /// Pre-image
        preimage: Box<[u8]>,
        /// Hash value
        hash: Box<[u8]>,
    },
    /// Serialization error in dash consensus-encoded structures
    ConsensusEncoding(encode::Error),
    /// Parsing error indicating invalid public keys
    InvalidPublicKey(dashcore::crypto::key::Error),
    /// Parsing error indicating invalid secp256k1 public keys
    InvalidSecp256k1PublicKey(dashcore::secp256k1::Error),
    /// Parsing error indicating invalid ECDSA signatures
    InvalidEcdsaSignature(dashcore::crypto::ecdsa::Error),
    /// Error related to a xpub key
    XPubKey(&'static str),
    /// Error related to PSBT version
    Version(&'static str),
    /// PSBT data is not consumed entirely
    PartialDataConsumption,
    /// I/O error.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Error::InvalidMagic => f.write_str("invalid magic"),
            Error::InvalidSeparator => f.write_str("invalid separator"),
            Error::InvalidKey(ref rkey) => write!(f, "invalid key: {}", rkey),
            Error::InvalidProprietaryKey => {
                write!(
                    f,
                    "non-proprietary key type found when proprietary key was expected"
                )
            }
            Error::DuplicateKey(ref rkey) => write!(f, "duplicate key: {}", rkey),
            Error::UnsignedTxHasScriptSigs => {
                f.write_str("the unsigned transaction has script sigs")
            }
            Error::UnsignedTxHasScriptWitnesses => {
                f.write_str("the unsigned transaction has script witnesses")
            }
            Error::MustHaveUnsignedTx => {
                f.write_str("partially signed transactions must have an unsigned transaction")
            }
            Error::NoMorePairs => f.write_str("no more key-value pairs for this psbt map"),
            Error::NonStandardSighashType(ref sht) => {
                write!(f, "non-standard sighash type: {}", sht)
            }
            Error::HashParse(ref e) => write!(f, "hash parse error: {}", e),
            Error::InvalidPreimageHashPair {
                ref preimage,
                ref hash,
                ref hash_type,
            } => {
                // directly using debug forms of psbthash enums
                write!(
                    f,
                    "Preimage {:?} does not match {:?} hash {:?}",
                    preimage, hash_type, hash
                )
            }
            Error::ConsensusEncoding(ref e) => write!(f, "dash consensus encoding error: {}", e),
            Error::InvalidPublicKey(ref e) => write!(f, "invalid public key: {}", e),
            Error::InvalidSecp256k1PublicKey(ref e) => {
                write!(f, "invalid secp256k1 public key: {}", e)
            }
            Error::InvalidEcdsaSignature(ref e) => write!(f, "invalid ECDSA signature: {}", e),
            Error::XPubKey(s) => write!(f, "xpub key error -  {}", s),
            Error::Version(s) => write!(f, "version error {}", s),
            Error::PartialDataConsumption => {
                f.write_str("data not consumed entirely when explicitly deserializing")
            }
            Error::Io(ref e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        use self::Error::*;

        match self {
            HashParse(e) => Some(e),
            ConsensusEncoding(e) => Some(e),
            Io(e) => Some(e),
            InvalidMagic
            | InvalidSeparator
            | InvalidKey(_)
            | InvalidProprietaryKey
            | DuplicateKey(_)
            | UnsignedTxHasScriptSigs
            | UnsignedTxHasScriptWitnesses
            | MustHaveUnsignedTx
            | NoMorePairs
            | NonStandardSighashType(_)
            | InvalidPreimageHashPair { .. }
            | InvalidPublicKey(_)
            | InvalidSecp256k1PublicKey(_)
            | InvalidEcdsaSignature(_)
            | XPubKey(_)
            | Version(_)
            | PartialDataConsumption => None,
        }
    }
}

#[doc(hidden)]
impl From<hashes::Error> for Error {
    fn from(e: hashes::Error) -> Error {
        Error::HashParse(e)
    }
}

impl From<encode::Error> for Error {
    fn from(e: encode::Error) -> Self {
        Error::ConsensusEncoding(e)
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}
