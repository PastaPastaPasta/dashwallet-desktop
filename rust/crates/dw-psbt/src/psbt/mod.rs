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

use std::collections::BTreeMap;

use dashcore::blockdata::transaction::Transaction;
use key_wallet::bip32::{ExtendedPubKey, KeySource};

#[macro_use]
mod macros;
pub mod raw;
pub mod serialize;

mod error;
pub use self::error::Error;

mod map;
pub use self::map::{Input, Output, PsbtSighashType};

/// A Partially Signed Transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PartiallySignedTransaction {
    /// The unsigned transaction, scriptSigs and witnesses for each input must be empty.
    pub unsigned_tx: Transaction,
    /// The version number of this PSBT. If omitted, the version number is 0.
    pub version: u32,
    /// A global map from extended public keys to the used key fingerprint and
    /// derivation path as defined by BIP 32.
    pub xpub: BTreeMap<ExtendedPubKey, KeySource>,
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

    /// Dash has no taproot: a taproot pair is kept in `unknown` and written
    /// back unchanged (upstream parsed these into `tap_*` fields).
    #[test]
    fn taproot_pairs_are_carried_as_unknown() {
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
        let mut psbt = PartiallySignedTransaction::from_unsigned_tx(tx).unwrap();
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
}
