// SPDX-License-Identifier: CC0-1.0
//
// Vendored from rust-dashcore key-wallet/src/psbt/map/input.rs @ 40268cc0
// (see ../../../VENDORED.md); taproot fields, combine, `FromStr` and serde
// removed.

use core::convert::TryFrom;
use core::fmt;

use dashcore::hashes::{self, hash160, ripemd160, sha256, sha256d};

use crate::psbt::map::Map;
use crate::psbt::serialize::Deserialize;
use crate::psbt::{Error, error, raw};
use dashcore::blockdata::script::ScriptBuf;
use dashcore::blockdata::transaction::Transaction;
use dashcore::blockdata::transaction::txout::TxOut;
use dashcore::blockdata::witness::Witness;
use dashcore::crypto::ecdsa;
use dashcore::crypto::key::PublicKey;
use dashcore::secp256k1;
use dashcore::sighash::{self, EcdsaSighashType, NonStandardSighashType, TapSighashType};
use key_wallet::bip32::KeySource;
use std::collections::{BTreeMap, btree_map};

/// Type: Non-Witness UTXO PSBT_IN_NON_WITNESS_UTXO = 0x00
const PSBT_IN_NON_WITNESS_UTXO: u8 = 0x00;
/// Type: Witness UTXO PSBT_IN_WITNESS_UTXO = 0x01
const PSBT_IN_WITNESS_UTXO: u8 = 0x01;
/// Type: Partial Signature PSBT_IN_PARTIAL_SIG = 0x02
const PSBT_IN_PARTIAL_SIG: u8 = 0x02;
/// Type: Sighash Type PSBT_IN_SIGHASH_TYPE = 0x03
const PSBT_IN_SIGHASH_TYPE: u8 = 0x03;
/// Type: Redeem Script PSBT_IN_REDEEM_SCRIPT = 0x04
const PSBT_IN_REDEEM_SCRIPT: u8 = 0x04;
/// Type: Witness Script PSBT_IN_WITNESS_SCRIPT = 0x05
const PSBT_IN_WITNESS_SCRIPT: u8 = 0x05;
/// Type: BIP 32 Derivation Path PSBT_IN_BIP32_DERIVATION = 0x06
const PSBT_IN_BIP32_DERIVATION: u8 = 0x06;
/// Type: Finalized scriptSig PSBT_IN_FINAL_SCRIPTSIG = 0x07
const PSBT_IN_FINAL_SCRIPTSIG: u8 = 0x07;
/// Type: Finalized scriptWitness PSBT_IN_FINAL_SCRIPTWITNESS = 0x08
const PSBT_IN_FINAL_SCRIPTWITNESS: u8 = 0x08;
/// Type: RIPEMD160 preimage PSBT_IN_RIPEMD160 = 0x0a
const PSBT_IN_RIPEMD160: u8 = 0x0a;
/// Type: SHA256 preimage PSBT_IN_SHA256 = 0x0b
const PSBT_IN_SHA256: u8 = 0x0b;
/// Type: HASH160 preimage PSBT_IN_HASH160 = 0x0c
const PSBT_IN_HASH160: u8 = 0x0c;
/// Type: HASH256 preimage PSBT_IN_HASH256 = 0x0d
const PSBT_IN_HASH256: u8 = 0x0d;
/// Type: Proprietary Use Type PSBT_IN_PROPRIETARY = 0xFC
const PSBT_IN_PROPRIETARY: u8 = 0xFC;

/// A key-value map for an input of the corresponding index in the unsigned
/// transaction.
#[derive(Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct Input {
    /// The non-witness transaction this input spends from. Should only be
    /// [Some] for inputs which spend non-segwit outputs or
    /// if it is unknown whether an input spends a segwit output.
    pub non_witness_utxo: Option<Transaction>,
    /// The transaction output this input spends from. Should only be
    /// [Some] for inputs which spend segwit outputs,
    /// including P2SH embedded ones.
    pub witness_utxo: Option<TxOut>,
    /// A map from public keys to their corresponding signature as would be
    /// pushed to the stack from a scriptSig or witness for a non-taproot inputs.
    pub partial_sigs: BTreeMap<PublicKey, ecdsa::Signature>,
    /// The sighash type to be used for this input. Signatures for this input
    /// must use the sighash type.
    pub sighash_type: Option<PsbtSighashType>,
    /// The redeem script for this input.
    pub redeem_script: Option<ScriptBuf>,
    /// The witness script for this input.
    pub witness_script: Option<ScriptBuf>,
    /// A map from public keys needed to sign this input to their corresponding
    /// master key fingerprints and derivation paths.
    pub bip32_derivation: BTreeMap<secp256k1::PublicKey, KeySource>,
    /// The finalized, fully-constructed scriptSig with signatures and any other
    /// scripts necessary for this input to pass validation.
    pub final_script_sig: Option<ScriptBuf>,
    /// The finalized, fully-constructed scriptWitness with signatures and any
    /// other scripts necessary for this input to pass validation.
    pub final_script_witness: Option<Witness>,
    /// TODO: Proof of reserves commitment
    /// RIPEMD160 hash to preimage map.
    pub ripemd160_preimages: BTreeMap<ripemd160::Hash, Vec<u8>>,
    /// SHA256 hash to preimage map.
    pub sha256_preimages: BTreeMap<sha256::Hash, Vec<u8>>,
    /// HASH160 hash to preimage map.
    pub hash160_preimages: BTreeMap<hash160::Hash, Vec<u8>>,
    /// HAS256 hash to preimage map.
    pub hash256_preimages: BTreeMap<sha256d::Hash, Vec<u8>>,
    /// Proprietary key-value pairs for this input.
    pub proprietary: BTreeMap<raw::ProprietaryKey, Vec<u8>>,
    /// Unknown key-value pairs for this input.
    pub unknown: BTreeMap<raw::Key, Vec<u8>>,
}

/// A Signature hash type for the corresponding input. As of taproot upgrade, the signature hash
/// type can be either [`EcdsaSighashType`] or [`TapSighashType`] but it is not possible to know
/// directly which signature hash type the user is dealing with. Therefore, the user is responsible
/// for converting to/from [`PsbtSighashType`] from/to the desired signature hash type they need.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PsbtSighashType {
    pub(in crate::psbt) inner: u32,
}

impl fmt::Display for PsbtSighashType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.taproot_hash_ty() {
            Err(_) => write!(f, "{:#x}", self.inner),
            Ok(taproot_hash_ty) => fmt::Display::fmt(&taproot_hash_ty, f),
        }
    }
}

impl From<EcdsaSighashType> for PsbtSighashType {
    fn from(ecdsa_hash_ty: EcdsaSighashType) -> Self {
        PsbtSighashType {
            inner: ecdsa_hash_ty as u32,
        }
    }
}

impl PsbtSighashType {
    /// Returns the [`EcdsaSighashType`] if the [`PsbtSighashType`] can be
    /// converted to one.
    pub fn ecdsa_hash_ty(self) -> Result<EcdsaSighashType, NonStandardSighashType> {
        EcdsaSighashType::from_standard(self.inner)
    }

    /// Returns the [`TapSighashType`] if the [`PsbtSighashType`] can be
    /// converted to one.
    pub fn taproot_hash_ty(self) -> Result<TapSighashType, sighash::Error> {
        if self.inner > 0xffu32 {
            Err(sighash::Error::InvalidSighashType(self.inner))
        } else {
            TapSighashType::from_consensus_u8(self.inner as u8)
        }
    }

    /// Creates a [`PsbtSighashType`] from a raw `u32`.
    ///
    /// Allows construction of a non-standard or non-valid sighash flag
    /// ([`EcdsaSighashType`], [`TapSighashType`] respectively).
    pub fn from_u32(n: u32) -> PsbtSighashType {
        PsbtSighashType { inner: n }
    }

    /// Converts [`PsbtSighashType`] to a raw `u32` sighash flag.
    ///
    /// No guarantees are made as to the standardness or validity of the returned value.
    pub fn to_u32(self) -> u32 {
        self.inner
    }
}

impl Input {
    /// Obtains the [`EcdsaSighashType`] for this input if one is specified. If no sighash type is
    /// specified, returns [`EcdsaSighashType::All`].
    ///
    /// # Errors
    ///
    /// If the `sighash_type` field is set to a non-standard ECDSA sighash value.
    pub fn ecdsa_hash_ty(&self) -> Result<EcdsaSighashType, NonStandardSighashType> {
        self.sighash_type
            .map(|sighash_type| sighash_type.ecdsa_hash_ty())
            .unwrap_or(Ok(EcdsaSighashType::All))
    }

    pub(super) fn insert_pair(&mut self, pair: raw::Pair) -> Result<(), Error> {
        let raw::Pair {
            key: raw_key,
            value: raw_value,
        } = pair;

        match raw_key.type_value {
            PSBT_IN_NON_WITNESS_UTXO => {
                impl_psbt_insert_pair! {
                    self.non_witness_utxo <= <raw_key: _>|<raw_value: Transaction>
                }
            }
            PSBT_IN_WITNESS_UTXO => {
                impl_psbt_insert_pair! {
                    self.witness_utxo <= <raw_key: _>|<raw_value: TxOut>
                }
            }
            PSBT_IN_PARTIAL_SIG => {
                impl_psbt_insert_pair! {
                    self.partial_sigs <= <raw_key: PublicKey>|<raw_value: ecdsa::Signature>
                }
            }
            PSBT_IN_SIGHASH_TYPE => {
                impl_psbt_insert_pair! {
                    self.sighash_type <= <raw_key: _>|<raw_value: PsbtSighashType>
                }
            }
            PSBT_IN_REDEEM_SCRIPT => {
                impl_psbt_insert_pair! {
                    self.redeem_script <= <raw_key: _>|<raw_value: ScriptBuf>
                }
            }
            PSBT_IN_WITNESS_SCRIPT => {
                impl_psbt_insert_pair! {
                    self.witness_script <= <raw_key: _>|<raw_value: ScriptBuf>
                }
            }
            PSBT_IN_BIP32_DERIVATION => {
                impl_psbt_insert_pair! {
                    self.bip32_derivation <= <raw_key: secp256k1::PublicKey>|<raw_value: KeySource>
                }
            }
            PSBT_IN_FINAL_SCRIPTSIG => {
                impl_psbt_insert_pair! {
                    self.final_script_sig <= <raw_key: _>|<raw_value: ScriptBuf>
                }
            }
            PSBT_IN_FINAL_SCRIPTWITNESS => {
                impl_psbt_insert_pair! {
                    self.final_script_witness <= <raw_key: _>|<raw_value: Witness>
                }
            }
            PSBT_IN_RIPEMD160 => {
                psbt_insert_hash_pair(
                    &mut self.ripemd160_preimages,
                    raw_key,
                    raw_value,
                    error::PsbtHash::Ripemd,
                )?;
            }
            PSBT_IN_SHA256 => {
                psbt_insert_hash_pair(
                    &mut self.sha256_preimages,
                    raw_key,
                    raw_value,
                    error::PsbtHash::Sha256,
                )?;
            }
            PSBT_IN_HASH160 => {
                psbt_insert_hash_pair(
                    &mut self.hash160_preimages,
                    raw_key,
                    raw_value,
                    error::PsbtHash::Hash160,
                )?;
            }
            PSBT_IN_HASH256 => {
                psbt_insert_hash_pair(
                    &mut self.hash256_preimages,
                    raw_key,
                    raw_value,
                    error::PsbtHash::Hash256,
                )?;
            }
            PSBT_IN_PROPRIETARY => {
                let key = raw::ProprietaryKey::try_from(raw_key.clone())?;
                match self.proprietary.entry(key) {
                    btree_map::Entry::Vacant(empty_key) => {
                        empty_key.insert(raw_value);
                    }
                    btree_map::Entry::Occupied(_) => return Err(Error::DuplicateKey(raw_key)),
                }
            }
            _ => match self.unknown.entry(raw_key) {
                btree_map::Entry::Vacant(empty_key) => {
                    empty_key.insert(raw_value);
                }
                btree_map::Entry::Occupied(k) => return Err(Error::DuplicateKey(k.key().clone())),
            },
        }

        Ok(())
    }
}

impl Map for Input {
    fn get_pairs(&self) -> Vec<raw::Pair> {
        let mut rv: Vec<raw::Pair> = Default::default();

        impl_psbt_get_pair! {
            rv.push(self.non_witness_utxo, PSBT_IN_NON_WITNESS_UTXO)
        }

        impl_psbt_get_pair! {
            rv.push(self.witness_utxo, PSBT_IN_WITNESS_UTXO)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.partial_sigs, PSBT_IN_PARTIAL_SIG)
        }

        impl_psbt_get_pair! {
            rv.push(self.sighash_type, PSBT_IN_SIGHASH_TYPE)
        }

        impl_psbt_get_pair! {
            rv.push(self.redeem_script, PSBT_IN_REDEEM_SCRIPT)
        }

        impl_psbt_get_pair! {
            rv.push(self.witness_script, PSBT_IN_WITNESS_SCRIPT)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.bip32_derivation, PSBT_IN_BIP32_DERIVATION)
        }

        impl_psbt_get_pair! {
            rv.push(self.final_script_sig, PSBT_IN_FINAL_SCRIPTSIG)
        }

        impl_psbt_get_pair! {
            rv.push(self.final_script_witness, PSBT_IN_FINAL_SCRIPTWITNESS)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.ripemd160_preimages, PSBT_IN_RIPEMD160)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.sha256_preimages, PSBT_IN_SHA256)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.hash160_preimages, PSBT_IN_HASH160)
        }

        impl_psbt_get_pair! {
            rv.push_map(self.hash256_preimages, PSBT_IN_HASH256)
        }

        for (key, value) in self.proprietary.iter() {
            rv.push(raw::Pair {
                key: key.to_key(),
                value: value.clone(),
            });
        }

        for (key, value) in self.unknown.iter() {
            rv.push(raw::Pair {
                key: key.clone(),
                value: value.clone(),
            });
        }

        rv
    }
}

impl_psbtmap_ser_de_serialize!(Input);

fn psbt_insert_hash_pair<H>(
    map: &mut BTreeMap<H, Vec<u8>>,
    raw_key: raw::Key,
    raw_value: Vec<u8>,
    hash_type: error::PsbtHash,
) -> Result<(), Error>
where
    H: hashes::Hash + Deserialize,
{
    if raw_key.key.is_empty() {
        return Err(Error::InvalidKey(raw_key));
    }
    let key_val: H = Deserialize::deserialize(&raw_key.key)?;
    match map.entry(key_val) {
        btree_map::Entry::Vacant(empty_key) => {
            let val: Vec<u8> = Deserialize::deserialize(&raw_value)?;
            if <H as hashes::Hash>::hash(&val) != key_val {
                return Err(Error::InvalidPreimageHashPair {
                    preimage: val.into_boxed_slice(),
                    hash: Box::from(key_val.borrow()),
                    hash_type,
                });
            }
            empty_key.insert(val);
            Ok(())
        }
        btree_map::Entry::Occupied(_) => Err(Error::DuplicateKey(raw_key)),
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn psbt_sighash_type_ecdsa() {
        for ecdsa in &[
            EcdsaSighashType::All,
            EcdsaSighashType::None,
            EcdsaSighashType::Single,
            EcdsaSighashType::AllPlusAnyoneCanPay,
            EcdsaSighashType::NonePlusAnyoneCanPay,
            EcdsaSighashType::SinglePlusAnyoneCanPay,
        ] {
            let sighash = PsbtSighashType::from(*ecdsa);
            assert_eq!(sighash.ecdsa_hash_ty().unwrap(), *ecdsa);
            assert_eq!(PsbtSighashType::from_u32(sighash.to_u32()), sighash);
        }
    }

    #[test]
    fn psbt_sighash_type_not_std() {
        let nonstd = 0xdddddddd;
        let sighash = PsbtSighashType { inner: nonstd };
        assert_eq!(format!("{}", sighash), "0xdddddddd");
        assert_eq!(sighash.ecdsa_hash_ty(), Err(NonStandardSighashType(nonstd)));
        assert_eq!(
            sighash.taproot_hash_ty(),
            Err(sighash::Error::InvalidSighashType(nonstd))
        );
    }
}
