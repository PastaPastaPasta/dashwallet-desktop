//! A transaction of any special type, as bytes Core reads
//! (`CTransaction` serialization, rc.3 `primitives/transaction.h:346-366`):
//! `int16 nVersion | uint16 nType`, inputs, outputs, lock time, and for a
//! version-3 transaction with a type other than 0 the payload with its
//! compact-size length.
//!
//! The pin's `dashcore::Transaction` refuses types 10–12 and decodes a
//! ProRegTx payload at version ≤ 2 only (MG-04b §0, D2), so the shared
//! transactions are read and written through this type; the payload is
//! decoded separately ([`super::ProRegTxV3`], [`super::ProDisTx`], …).

use dashcore::consensus::encode::{self, Decodable, Encodable};
use dashcore::hashes::{Hash, sha256d};
use dashcore::{OutPoint, Script, ScriptBuf, TxIn, TxOut, Txid};

/// `CTransaction::SPECIAL_VERSION`: from this version on, a non-zero type
/// carries a payload (rc.3 `primitives/transaction.h:244,362-364`).
pub const SPECIAL_VERSION: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecialTx {
    pub version: u16,
    pub tx_type: u16,
    pub input: Vec<TxIn>,
    pub output: Vec<TxOut>,
    pub lock_time: u32,
    /// `vExtraPayload`; empty when the transaction has none.
    pub payload: Vec<u8>,
}

impl SpecialTx {
    /// `nVersion` is an `int16_t` in Core: a version with the top bit set
    /// is negative and carries no payload.
    fn has_payload(version: u16, tx_type: u16) -> bool {
        (version as i16) >= SPECIAL_VERSION as i16 && tx_type != 0
    }

    /// The transaction's bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.consensus_encode(&mut out).expect("vec");
        out
    }

    /// Decodes a whole transaction; trailing bytes are refused.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, encode::Error> {
        dashcore::consensus::deserialize(bytes)
    }

    /// `GetHash()`: double SHA-256 of the whole serialization, payload
    /// included.
    pub fn txid(&self) -> Txid {
        let mut engine = sha256d::Hash::engine();
        self.consensus_encode(&mut engine)
            .expect("engines don't error");
        Txid::from_raw_hash(sha256d::Hash::from_engine(engine))
    }

    /// The inputs as `(prevout, nSequence)`, the form the digests take.
    pub fn prevouts(&self) -> Vec<(OutPoint, u32)> {
        self.input
            .iter()
            .map(|i| (i.previous_output, i.sequence))
            .collect()
    }

    /// `CalcTxInputsHash` (rc.3 `evo/specialtx.cpp:10-17`).
    pub fn inputs_hash(&self) -> [u8; 32] {
        inputs_hash(self.input.iter().map(|i| &i.previous_output))
    }

    /// `CalcTxOutputsHash` (rc.3 `evo/specialtx.cpp:19-26`).
    pub fn outputs_hash(&self) -> [u8; 32] {
        let mut engine = sha256d::Hash::engine();
        for out in &self.output {
            out.consensus_encode(&mut engine)
                .expect("engines don't error");
        }
        sha256d::Hash::from_engine(engine).to_byte_array()
    }
}

/// `SIGHASH_ALL`.
pub const SIGHASH_ALL: u32 = 1;

impl SpecialTx {
    /// `SignatureHash` with `SIGHASH_ALL`, the legacy algorithm (rc.3
    /// `script/interpreter.cpp:1370-1500`): the transaction with every
    /// scriptSig empty but input `index`'s, which is `script_code`, the
    /// payload included as in [`Self::to_bytes`] (`:1437-1438`), then the
    /// hash type as a `u32`. The scripts the wallet signs (P2PKH) hold no
    /// `OP_CODESEPARATOR`. `None` for an input that doesn't exist (Core
    /// returns the "one" hash).
    pub fn signature_hash_all(&self, index: usize, script_code: &Script) -> Option<[u8; 32]> {
        if index >= self.input.len() {
            return None;
        }
        let mut tx = self.clone();
        for (i, input) in tx.input.iter_mut().enumerate() {
            input.script_sig = if i == index {
                script_code.to_owned()
            } else {
                ScriptBuf::new()
            };
        }
        let mut engine = sha256d::Hash::engine();
        tx.consensus_encode(&mut engine)
            .and_then(|_| SIGHASH_ALL.consensus_encode(&mut engine))
            .expect("engines don't error");
        Some(sha256d::Hash::from_engine(engine).to_byte_array())
    }
}

/// `CalcTxInputsHash`: double SHA-256 of the prevouts, in order.
pub fn inputs_hash<'a>(prevouts: impl IntoIterator<Item = &'a OutPoint>) -> [u8; 32] {
    let mut engine = sha256d::Hash::engine();
    for prevout in prevouts {
        prevout
            .consensus_encode(&mut engine)
            .expect("engines don't error");
    }
    sha256d::Hash::from_engine(engine).to_byte_array()
}

impl Encodable for SpecialTx {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        let mut len = self.version.consensus_encode(w)?;
        len += self.tx_type.consensus_encode(w)?;
        len += self.input.consensus_encode(w)?;
        len += self.output.consensus_encode(w)?;
        len += self.lock_time.consensus_encode(w)?;
        if Self::has_payload(self.version, self.tx_type) {
            len += self.payload.consensus_encode(w)?;
        } else if !self.payload.is_empty() {
            return Err(std::io::Error::other(
                "a payload needs version 3 and a special type",
            ));
        }
        Ok(len)
    }
}

impl Decodable for SpecialTx {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        let version = u16::consensus_decode(r)?;
        let tx_type = u16::consensus_decode(r)?;
        let input = Vec::<TxIn>::consensus_decode(r)?;
        let output = Vec::<TxOut>::consensus_decode(r)?;
        let lock_time = u32::consensus_decode(r)?;
        let payload = if Self::has_payload(version, tx_type) {
            Vec::<u8>::consensus_decode(r)?
        } else {
            Vec::new()
        };
        Ok(Self {
            version,
            tx_type,
            input,
            output,
            lock_time,
            payload,
        })
    }
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::Witness;

    fn tx(version: u16, tx_type: u16, payload: Vec<u8>) -> SpecialTx {
        SpecialTx {
            version,
            tx_type,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([1; 32]), 7),
                script_sig: ScriptBuf::new(),
                sequence: u32::MAX,
                witness: Witness::default(),
            }],
            output: vec![TxOut {
                value: 5,
                script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
            }],
            lock_time: 0,
            payload,
        }
    }

    #[test]
    fn test_QT_127_special_tx_round_trips_with_its_payload() {
        let t = tx(3, 10, vec![1, 2, 3]);
        let bytes = t.to_bytes();
        assert_eq!(&bytes[..4], &[3, 0, 10, 0]);
        assert_eq!(&bytes[bytes.len() - 4..], &[3, 1, 2, 3]);
        assert_eq!(SpecialTx::from_bytes(&bytes).unwrap(), t);
        let plain = tx(2, 0, vec![]);
        let raw = plain.to_bytes();
        assert_eq!(SpecialTx::from_bytes(&raw).unwrap(), plain);
        // A version-2 or type-0 transaction carries no payload.
        assert!(tx(2, 1, vec![9]).consensus_encode(&mut Vec::new()).is_err());
        let mut trailing = raw.clone();
        trailing.push(0);
        assert!(SpecialTx::from_bytes(&trailing).is_err());
    }

    #[test]
    fn test_QT_126_txid_matches_the_pins_transaction_for_a_classic_tx() {
        let plain = tx(2, 0, vec![]);
        let pin: dashcore::Transaction =
            dashcore::consensus::deserialize(&plain.to_bytes()).unwrap();
        assert_eq!(plain.txid(), pin.txid());
    }

    #[test]
    fn test_QT_126_inputs_hash_is_sha256d_of_the_prevouts() {
        let t = tx(3, 1, vec![]);
        let mut raw = Vec::new();
        t.input[0]
            .previous_output
            .consensus_encode(&mut raw)
            .unwrap();
        assert_eq!(raw.len(), 36);
        assert_eq!(t.inputs_hash(), sha256d::Hash::hash(&raw).to_byte_array());
    }
}
