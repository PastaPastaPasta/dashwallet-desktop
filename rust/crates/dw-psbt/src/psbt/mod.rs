// SPDX-License-Identifier: CC0-1.0
//
// Vendored from rust-dashcore key-wallet/src/psbt/mod.rs @ 40268cc0
// (see ../../VENDORED.md); the signing, combine and fee code is removed.

//! Partially Signed Bitcoin Transactions.
//!
//! Implementation of BIP174 Partially Signed Bitcoin Transaction Format as
//! defined at <https://github.com/bitcoin/bips/blob/master/bip-0174.mediawiki>
//! except we define PSBTs containing non-standard sighash types as invalid.
//!
//! Only the container is here: the data types, their BIP174 (de)serialization
//! and the errors. Signing is `dw_psbt::sign`'s.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use dashcore::blockdata::transaction::Transaction;
use dashcore::crypto::key::PublicKey;
use key_wallet::bip32::{ExtendedPubKey, KeySource};

#[macro_use]
mod macros;
pub mod raw;
pub mod serialize;

mod error;
pub use self::error::Error;

mod map;
pub use self::map::{Input, Output, PsbtSighashType};

/// A [`PublicKey`] as a map key. rust-dashcore #1144 dropped the key's
/// derived order; this is that order (`compressed`, then the key), so maps
/// keep the order they serialize in.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct SigKey(pub PublicKey);

impl Ord for SigKey {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.0.compressed, self.0.inner).cmp(&(other.0.compressed, other.0.inner))
    }
}

impl PartialOrd for SigKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// An [`ExtendedPubKey`] as a map key, in the field order its derive had
/// before rust-dashcore #1144 dropped it (see [`SigKey`]).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct XpubKey(pub ExtendedPubKey);

impl Ord for XpubKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let fields = |x: &ExtendedPubKey| {
            (
                x.network,
                x.depth,
                x.parent_fingerprint,
                x.child_number,
                x.public_key,
                x.chain_code,
            )
        };
        fields(&self.0).cmp(&fields(&other.0))
    }
}

impl PartialOrd for XpubKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A Partially Signed Transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartiallySignedTransaction {
    /// The unsigned transaction, scriptSigs and witnesses for each input must be empty.
    pub unsigned_tx: Transaction,
    /// The version number of this PSBT. If omitted, the version number is 0.
    pub version: u32,
    /// A global map from extended public keys to the used key fingerprint and
    /// derivation path as defined by BIP 32.
    pub xpub: BTreeMap<XpubKey, KeySource>,
    /// Global proprietary key-value pairs.
    pub proprietary: BTreeMap<raw::ProprietaryKey, Vec<u8>>,
    /// Unknown global key-value pairs.
    pub unknown: BTreeMap<raw::Key, Vec<u8>>,

    /// The corresponding key-value map for each input in the unsigned transaction.
    pub inputs: Vec<Input>,
    /// The corresponding key-value map for each output in the unsigned transaction.
    pub outputs: Vec<Output>,
}

impl PartiallySignedTransaction {
    /// Checks that unsigned transaction does not have scriptSig's or witness data.
    fn unsigned_tx_checks(&self) -> Result<(), Error> {
        for txin in &self.unsigned_tx.input {
            if !txin.script_sig.is_empty() {
                return Err(Error::UnsignedTxHasScriptSigs);
            }

            if !txin.witness.is_empty() {
                return Err(Error::UnsignedTxHasScriptWitnesses);
            }
        }

        Ok(())
    }

    /// Creates a PSBT from an unsigned transaction.
    ///
    /// # Errors
    ///
    /// If transactions is not unsigned.
    pub fn from_unsigned_tx(tx: Transaction) -> Result<Self, Error> {
        let psbt = PartiallySignedTransaction {
            inputs: vec![Default::default(); tx.input.len()],
            outputs: vec![Default::default(); tx.output.len()],

            unsigned_tx: tx,
            xpub: Default::default(),
            version: 0,
            proprietary: Default::default(),
            unknown: Default::default(),
        };
        psbt.unsigned_tx_checks()?;
        Ok(psbt)
    }

    /// Extracts the `Transaction` from a PSBT by filling in the available signature information.
    pub fn extract_tx(self) -> Transaction {
        let mut tx: Transaction = self.unsigned_tx;

        for (vin, psbtin) in tx.input.iter_mut().zip(self.inputs) {
            vin.script_sig = psbtin.final_script_sig.unwrap_or_default();
            vin.witness = psbtin.final_script_witness.unwrap_or_default();
        }

        tx
    }
}

#[cfg(test)]
mod tests {
    use dashcore::blockdata::script::ScriptBuf;
    use dashcore::blockdata::transaction::txout::TxOut;
    use dashcore::blockdata::witness::Witness;
    use dashcore::{OutPoint, TxIn};

    use super::*;

    fn one_in_one_out() -> PartiallySignedTransaction {
        let tx = Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::new(),
                sequence: 0xffff_ffff,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: 0,
                script_pubkey: ScriptBuf::new(),
            }],
            special_transaction_payload: None,
        };
        PartiallySignedTransaction::from_unsigned_tx(tx).unwrap()
    }

    /// Where `needle` first occurs in `bytes`.
    fn at(bytes: &[u8], needle: &[u8]) -> usize {
        bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap()
    }

    /// Dash has no taproot: a taproot pair is kept in `unknown` and written
    /// back unchanged (upstream parsed these into `tap_*` fields).
    #[test]
    fn taproot_pairs_are_carried_as_unknown() {
        let mut psbt = one_in_one_out();
        // PSBT_IN_TAP_KEY_SIG and PSBT_OUT_TAP_INTERNAL_KEY.
        let tap_in = raw::Key {
            type_value: 0x13,
            key: vec![],
        };
        let tap_out = raw::Key {
            type_value: 0x05,
            key: vec![],
        };
        psbt.inputs[0].unknown.insert(tap_in.clone(), vec![7; 64]);
        psbt.outputs[0].unknown.insert(tap_out.clone(), vec![8; 32]);

        let bytes = psbt.serialize();
        let back = PartiallySignedTransaction::deserialize(&bytes).unwrap();
        assert_eq!(back, psbt);
        assert_eq!(back.serialize(), bytes);
        assert_eq!(back.inputs[0].unknown[&tap_in], vec![7; 64]);
        assert_eq!(back.outputs[0].unknown[&tap_out], vec![8; 32]);
    }

    /// The maps keyed by `SigKey` and `XpubKey` write their pairs in the
    /// order of the derives #1144 removed, not in the keys' byte order: an
    /// uncompressed key before a compressed one, and an xpub's key before
    /// its chain code (BL-187 D7).
    #[test]
    fn signatures_and_xpubs_keep_the_pre_1144_order() {
        use dashcore::Network;
        use dashcore::crypto::ecdsa;
        use dashcore::secp256k1::{self, Message, SecretKey, ecdsa as secp_ecdsa};
        use dashcore::sighash::EcdsaSighashType;
        use key_wallet::bip32::{ChainCode, ExtendedPrivKey};

        let inner = |n: u8| {
            secp256k1::PublicKey::from_secret_key(&SecretKey::from_secret_bytes([n; 32]).unwrap())
        };
        // libsecp256k1 orders keys by their compressed encoding.
        let mut by_point: Vec<_> = (1..=4).map(inner).collect();
        by_point.sort_by_key(|k| k.serialize());
        let expected = [
            PublicKey::new_uncompressed(by_point[1]),
            PublicKey::new_uncompressed(by_point[3]),
            PublicKey::new(by_point[0]),
            PublicKey::new(by_point[2]),
        ];

        let mut psbt = one_in_one_out();
        let sig = secp_ecdsa::sign(
            Message::from_digest([1; 32]),
            &SecretKey::from_secret_bytes([1; 32]).unwrap(),
        );
        for pk in [expected[2], expected[1], expected[3], expected[0]] {
            psbt.inputs[0].partial_sigs.insert(
                SigKey(pk),
                ecdsa::Signature {
                    sig,
                    hash_ty: EcdsaSighashType::All,
                },
            );
        }

        // An xpub encodes Testnet and Regtest alike, and reads back as Testnet.
        let master = ExtendedPrivKey::new_master(Network::Testnet, &[9; 32]).unwrap();
        let mut a = ExtendedPubKey::from_priv(&master);
        let mut b = a;
        b.public_key = inner(5);
        if a.public_key.serialize() > b.public_key.serialize() {
            std::mem::swap(&mut a.public_key, &mut b.public_key);
        }
        a.chain_code = ChainCode::from_bytes([0xee; 32]);
        b.chain_code = ChainCode::from_bytes([0x11; 32]);
        let source = (master.fingerprint(), Default::default());
        psbt.xpub.insert(XpubKey(b), source.clone());
        psbt.xpub.insert(XpubKey(a), source);

        let bytes = psbt.serialize();
        let sigs: Vec<usize> = expected
            .iter()
            .map(|pk| at(&bytes, &pk.to_bytes()))
            .collect();
        assert!(sigs.is_sorted(), "{sigs:?}");
        assert!(a.encode() > b.encode(), "the byte order would put b first");
        assert!(at(&bytes, &a.encode()) < at(&bytes, &b.encode()));

        let back = PartiallySignedTransaction::deserialize(&bytes).unwrap();
        assert_eq!(back, psbt);
        assert_eq!(back.serialize(), bytes);
    }
}
