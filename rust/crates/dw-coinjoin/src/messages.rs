//! CoinJoin wire messages (Dash Core `src/coinjoin/coinjoin.h`; command
//! names in `dw_p2p::commands`).
//!
//! | Command | Core type | Fields (wire order) |
//! |---|---|---|
//! | `dsa` | `CCoinJoinAccept` | `nDenom` i32, `txCollateral`, `nFlags` u8 only at version ≥ 70241 (coinjoin.h:150-156) |
//! | `dsq` | `CCoinJoinQueue` | `nDenom` i32, `m_protxHash`, `nTime` i64, `fReady` bool, `vchSig` (not in the hash) (coinjoin.h:262-268) |
//! | `dsi` | `CCoinJoinEntry` | `vecTxDSIn` (as `CTxIn`), `txCollateral`, `vecTxOut` (coinjoin.h:198-201) |
//! | `dssu` | `CCoinJoinStatusUpdate` | `nSessionID`, `nState`, `nStatusUpdate`, `nMessageID`, all i32 (coinjoin.h:131-134) |
//! | `dsf` | — | `nSessionID` i32, final transaction (client.cpp:110-113) |
//! | `dss` | — | `std::vector<CTxIn>` of signed inputs (client.cpp:626) |
//! | `dsc` | — | `nSessionID` i32, `nMessageID` i32 (client.cpp:124-127) |
//! | `dstx` | `CCoinJoinBroadcastTx` | `tx`, `m_protxHash`, `vchSig` (not in the hash), `sigTime` i64 (coinjoin.h:328-336) |

use dashcore::consensus::{Decodable, Encodable, encode};
use dashcore::hashes::{Hash, sha256d};
use dashcore::{Transaction, TxIn, TxOut};

use crate::denoms::{DENOMINATIONS, ENTRY_MAX_INPUTS, QUEUE_TIMEOUT_SECS};
use crate::status::{PoolMessage, PoolState};

/// First protocol version whose `dsa` carries the rebalance flags byte
/// (`COINJOIN_REBALANCE_VERSION`, Dash Core `src/version.h:73`).
pub const COINJOIN_REBALANCE_VERSION: u32 = 70241;

/// Largest pool (`GetMaxPoolParticipants` = 20 on every network,
/// chainparams.cpp) times `COINJOIN_ENTRY_MAX_SIZE`: the cap Core applies to
/// entry and signed-input vectors (`GetMaxPoolInputOutputCount`).
pub const MAX_POOL_INPUT_OUTPUT_COUNT: usize = 20 * ENTRY_MAX_INPUTS;

/// A message that does not decode.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageError {
    #[error("malformed {command}: {detail}")]
    Malformed {
        command: &'static str,
        detail: String,
    },
    #[error("{command} has {count} items, above {max}")]
    TooMany {
        command: &'static str,
        count: usize,
        max: usize,
    },
}

fn malformed(command: &'static str) -> impl Fn(encode::Error) -> MessageError {
    move |e| MessageError::Malformed {
        command,
        detail: e.to_string(),
    }
}

fn to_bytes<T: Encodable + ?Sized>(value: &T, out: &mut Vec<u8>) {
    value
        .consensus_encode(out)
        .expect("writing to a Vec cannot fail");
}

/// Decodes `T` and refuses trailing bytes.
fn decode_all<T: Decodable>(command: &'static str, bytes: &[u8]) -> Result<T, MessageError> {
    let mut cursor = bytes;
    let value = T::consensus_decode(&mut cursor).map_err(malformed(command))?;
    if !cursor.is_empty() {
        return Err(MessageError::Malformed {
            command,
            detail: format!("{} trailing bytes", cursor.len()),
        });
    }
    Ok(value)
}

/// Reads a vector capped at `max` items before decoding any of them
/// (Core `UnserializeVectorWithMaxSize`).
fn read_capped_vec<T: Decodable>(
    command: &'static str,
    r: &mut &[u8],
    max: usize,
) -> Result<Vec<T>, MessageError> {
    let count = encode::VarInt::consensus_decode(r)
        .map_err(malformed(command))?
        .0 as usize;
    if count > max {
        return Err(MessageError::TooMany {
            command,
            count,
            max,
        });
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(T::consensus_decode(r).map_err(malformed(command))?);
    }
    Ok(out)
}

/// The denomination bit of `amount` (`CoinJoin::AmountToDenomination`,
/// common.h:83-91): `1 << i` for the `i`-th denomination, 0 for others.
pub fn amount_to_denomination(amount: u64) -> u32 {
    DENOMINATIONS
        .iter()
        .position(|d| *d == amount)
        .map(|i| 1 << i)
        .unwrap_or(0)
}

/// The amount of a single denomination bit (`DenominationToAmount`,
/// common.h:100-120); `None` for 0, out-of-range or multi-bit values.
pub fn denomination_to_amount(denom: u32) -> Option<u64> {
    if denom == 0 || !denom.is_power_of_two() {
        return None;
    }
    DENOMINATIONS.get(denom.trailing_zeros() as usize).copied()
}

/// `dsa`: join or start a session for one denomination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accept {
    pub denom: u32,
    pub collateral: Transaction,
    /// `FLAG_PROMOTION`/`FLAG_DEMOTION`; written only when the session's
    /// common version is at least [`COINJOIN_REBALANCE_VERSION`].
    pub flags: u8,
}

impl Accept {
    pub fn encode(&self, common_version: u32) -> Vec<u8> {
        let mut out = Vec::new();
        to_bytes(&(self.denom as i32), &mut out);
        to_bytes(&self.collateral, &mut out);
        if common_version >= COINJOIN_REBALANCE_VERSION {
            to_bytes(&self.flags, &mut out);
        }
        out
    }

    pub fn decode(bytes: &[u8], common_version: u32) -> Result<Self, MessageError> {
        let r = &mut &bytes[..];
        let denom = i32::consensus_decode(r).map_err(malformed("dsa"))? as u32;
        let collateral = Transaction::consensus_decode(r).map_err(malformed("dsa"))?;
        let flags = if common_version >= COINJOIN_REBALANCE_VERSION {
            u8::consensus_decode(r).map_err(malformed("dsa"))?
        } else {
            0
        };
        Ok(Self {
            denom,
            collateral,
            flags,
        })
    }
}

/// `dsq`: a masternode's queue announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Queue {
    pub denom: u32,
    /// proTxHash, wire order.
    pub pro_tx_hash: [u8; 32],
    pub time: i64,
    pub ready: bool,
    pub signature: Vec<u8>,
}

impl Queue {
    fn encode_unsigned(&self, out: &mut Vec<u8>) {
        to_bytes(&(self.denom as i32), out);
        out.extend_from_slice(&self.pro_tx_hash);
        to_bytes(&self.time, out);
        to_bytes(&self.ready, out);
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode_unsigned(&mut out);
        to_bytes(&self.signature, &mut out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let r = &mut &bytes[..];
        let denom = i32::consensus_decode(r).map_err(malformed("dsq"))? as u32;
        let pro_tx_hash = <[u8; 32]>::consensus_decode(r).map_err(malformed("dsq"))?;
        let time = i64::consensus_decode(r).map_err(malformed("dsq"))?;
        let ready = bool::consensus_decode(r).map_err(malformed("dsq"))?;
        let signature = Vec::<u8>::consensus_decode(r).map_err(malformed("dsq"))?;
        if !r.is_empty() {
            return Err(MessageError::Malformed {
                command: "dsq",
                detail: "trailing bytes".into(),
            });
        }
        Ok(Self {
            denom,
            pro_tx_hash,
            time,
            ready,
            signature,
        })
    }

    /// `GetSignatureHash` (coinjoin.cpp:43-46): double SHA-256 of the
    /// serialization without `vchSig`.
    pub fn signature_hash(&self) -> [u8; 32] {
        let mut out = Vec::new();
        self.encode_unsigned(&mut out);
        sha256d::Hash::hash(&out).to_byte_array()
    }

    /// `GetHash` (coinjoin.cpp:47): the full serialization's hash, the
    /// queue's identity.
    pub fn hash(&self) -> [u8; 32] {
        sha256d::Hash::hash(&self.encode()).to_byte_array()
    }

    /// `IsTimeOutOfBounds` (coinjoin.cpp:59-64): more than 30 s from `now`.
    pub fn is_time_out_of_bounds(&self, now: i64) -> bool {
        now < 0 || self.time < 0 || (now - self.time).abs() > QUEUE_TIMEOUT_SECS as i64
    }

    /// `CheckSignature` (coinjoin.cpp:49-57): the operator's basic-scheme
    /// BLS signature over [`Self::signature_hash`].
    pub fn verify(&self, operator_public_key: &[u8; 48]) -> bool {
        crate::bls::verify_basic(operator_public_key, &self.signature, &self.signature_hash())
    }
}

/// `dsi`: our inputs (unsigned), collateral and outputs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub inputs: Vec<TxIn>,
    pub collateral: Transaction,
    pub outputs: Vec<TxOut>,
}

impl Entry {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        to_bytes(&self.inputs, &mut out);
        to_bytes(&self.collateral, &mut out);
        to_bytes(&self.outputs, &mut out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let r = &mut &bytes[..];
        let inputs = read_capped_vec("dsi", r, MAX_POOL_INPUT_OUTPUT_COUNT)?;
        let collateral = Transaction::consensus_decode(r).map_err(malformed("dsi"))?;
        let outputs = read_capped_vec("dsi", r, MAX_POOL_INPUT_OUTPUT_COUNT)?;
        Ok(Self {
            inputs,
            collateral,
            outputs,
        })
    }
}

/// `nStatusUpdate` of a `dssu` (`PoolStatusUpdate`, coinjoin.h:111-115).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusUpdateKind {
    Rejected,
    Accepted,
}

/// `dssu`: the masternode's answer to `dsa`/`dsi` or a session update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusUpdate {
    pub session_id: i32,
    pub state: PoolState,
    pub kind: StatusUpdateKind,
    pub message: PoolMessage,
}

impl StatusUpdate {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        to_bytes(&self.session_id, &mut out);
        to_bytes(&(self.state as i32), &mut out);
        to_bytes(
            &match self.kind {
                StatusUpdateKind::Rejected => 0i32,
                StatusUpdateKind::Accepted => 1i32,
            },
            &mut out,
        );
        to_bytes(&(self.message as i32), &mut out);
        out
    }

    /// Decodes and range-checks state, kind and message as
    /// `ProcessPoolStateUpdate` does (client.cpp:419-427, 452-455).
    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let (session_id, state, kind, message): (i32, i32, i32, i32) = decode_all("dssu", bytes)?;
        let bad = |what: &str, v: i32| MessageError::Malformed {
            command: "dssu",
            detail: format!("{what} {v} out of range"),
        };
        let state = PoolState::from_wire(state).ok_or_else(|| bad("state", state))?;
        let kind = match kind {
            0 => StatusUpdateKind::Rejected,
            1 => StatusUpdateKind::Accepted,
            v => return Err(bad("status", v)),
        };
        let message = PoolMessage::from_wire(message).ok_or_else(|| bad("message", message))?;
        Ok(Self {
            session_id,
            state,
            kind,
            message,
        })
    }
}

/// `dsf`: the unsigned final transaction of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalTx {
    pub session_id: i32,
    pub tx: Transaction,
}

impl FinalTx {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        to_bytes(&self.session_id, &mut out);
        to_bytes(&self.tx, &mut out);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let (session_id, tx): (i32, Transaction) = decode_all("dsf", bytes)?;
        Ok(Self { session_id, tx })
    }
}

/// `dss`: our signed inputs of the final transaction.
pub fn encode_signed_inputs(inputs: &[TxIn]) -> Vec<u8> {
    let mut out = Vec::new();
    to_bytes(&inputs.to_vec(), &mut out);
    out
}

pub fn decode_signed_inputs(bytes: &[u8]) -> Result<Vec<TxIn>, MessageError> {
    let r = &mut &bytes[..];
    let v = read_capped_vec("dss", r, MAX_POOL_INPUT_OUTPUT_COUNT)?;
    Ok(v)
}

/// `dsc`: the session finished (successfully or not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Complete {
    pub session_id: i32,
    pub message: PoolMessage,
}

impl Complete {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        to_bytes(&self.session_id, &mut out);
        to_bytes(&(self.message as i32), &mut out);
        out
    }

    /// Refuses a message id outside Core's range (client.cpp:129-132).
    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let (session_id, message): (i32, i32) = decode_all("dsc", bytes)?;
        let message = PoolMessage::from_wire(message).ok_or_else(|| MessageError::Malformed {
            command: "dsc",
            detail: format!("message {message} out of range"),
        })?;
        Ok(Self {
            session_id,
            message,
        })
    }
}

/// `dstx`: a mixing transaction the masternode signed for relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastTx {
    pub tx: Transaction,
    pub pro_tx_hash: [u8; 32],
    pub signature: Vec<u8>,
    pub sig_time: i64,
}

impl BroadcastTx {
    pub fn decode(bytes: &[u8]) -> Result<Self, MessageError> {
        let (tx, pro_tx_hash, signature, sig_time): (Transaction, [u8; 32], Vec<u8>, i64) =
            decode_all("dstx", bytes)?;
        Ok(Self {
            tx,
            pro_tx_hash,
            signature,
            sig_time,
        })
    }

    /// `GetSignatureHash`: the serialization without `vchSig`.
    pub fn signature_hash(&self) -> [u8; 32] {
        let mut out = Vec::new();
        to_bytes(&self.tx, &mut out);
        out.extend_from_slice(&self.pro_tx_hash);
        to_bytes(&self.sig_time, &mut out);
        sha256d::Hash::hash(&out).to_byte_array()
    }

    pub fn verify(&self, operator_public_key: &[u8; 48]) -> bool {
        crate::bls::verify_basic(operator_public_key, &self.signature, &self.signature_hash())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::{OutPoint, ScriptBuf, Txid};

    fn tx() -> Transaction {
        Transaction {
            version: 2,
            lock_time: 0,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([7; 32]), 1),
                script_sig: ScriptBuf::new(),
                sequence: 0xffff_ffff,
                witness: Default::default(),
            }],
            output: vec![TxOut {
                value: 30_000,
                script_pubkey: ScriptBuf::from_bytes(vec![0x76, 0xa9, 0x14]),
            }],
            special_transaction_payload: None,
        }
    }

    #[test]
    fn denomination_bits_follow_core() {
        assert_eq!(amount_to_denomination(1_000_010_000), 1);
        assert_eq!(amount_to_denomination(100_001), 16);
        assert_eq!(amount_to_denomination(100_000), 0);
        assert_eq!(denomination_to_amount(4), Some(10_000_100));
        assert_eq!(denomination_to_amount(3), None);
        assert_eq!(denomination_to_amount(32), None);
        assert_eq!(denomination_to_amount(0), None);
    }

    #[test]
    fn dsa_flags_are_version_gated() {
        let a = Accept {
            denom: 4,
            collateral: tx(),
            flags: 0,
        };
        let old = a.encode(70233);
        let new = a.encode(70242);
        assert_eq!(new.len(), old.len() + 1);
        assert_eq!(&old[..4], &4i32.to_le_bytes());
        assert_eq!(Accept::decode(&old, 70233).unwrap(), a);
        assert_eq!(Accept::decode(&new, 70242).unwrap(), a);
    }

    #[test]
    fn dsq_round_trip_and_hashes() {
        let q = Queue {
            denom: 2,
            pro_tx_hash: [3; 32],
            time: 1_700_000_000,
            ready: true,
            signature: vec![9; 96],
        };
        let bytes = q.encode();
        // 4 + 32 + 8 + 1 + (1 + 96)
        assert_eq!(bytes.len(), 142);
        assert_eq!(Queue::decode(&bytes).unwrap(), q);
        // The signature is not part of the signature hash.
        let unsigned = Queue {
            signature: vec![],
            ..q.clone()
        };
        assert_eq!(q.signature_hash(), unsigned.signature_hash());
        assert_ne!(q.hash(), unsigned.hash());
        assert!(!q.is_time_out_of_bounds(1_700_000_030));
        assert!(q.is_time_out_of_bounds(1_700_000_031));
        assert!(q.is_time_out_of_bounds(1_699_999_969));
    }

    #[test]
    fn dssu_and_dsc_range_checks() {
        let s = StatusUpdate {
            session_id: 123,
            state: PoolState::Queue,
            kind: StatusUpdateKind::Accepted,
            message: PoolMessage::NoError,
        };
        assert_eq!(StatusUpdate::decode(&s.encode()).unwrap(), s);
        let mut bad = s.encode();
        bad[4..8].copy_from_slice(&5i32.to_le_bytes()); // state 5 > POOL_STATE_MAX
        assert!(StatusUpdate::decode(&bad).is_err());
        let c = Complete {
            session_id: 9,
            message: PoolMessage::Success,
        };
        assert_eq!(Complete::decode(&c.encode()).unwrap(), c);
        let mut out = Vec::new();
        to_bytes(&9i32, &mut out);
        to_bytes(&23i32, &mut out);
        assert!(Complete::decode(&out).is_err());
    }

    #[test]
    fn entry_and_final_tx_round_trip() {
        let t = tx();
        let e = Entry {
            inputs: t.input.clone(),
            collateral: t.clone(),
            outputs: t.output.clone(),
        };
        assert_eq!(Entry::decode(&e.encode()).unwrap(), e);
        let f = FinalTx {
            session_id: 77,
            tx: t.clone(),
        };
        assert_eq!(FinalTx::decode(&f.encode()).unwrap(), f);
        let signed = encode_signed_inputs(&t.input);
        assert_eq!(decode_signed_inputs(&signed).unwrap(), t.input);
    }

    #[test]
    fn capped_vectors_refuse_oversized_counts() {
        let mut out = Vec::new();
        to_bytes(&encode::VarInt(181), &mut out);
        assert!(matches!(
            decode_signed_inputs(&out),
            Err(MessageError::TooMany { count: 181, .. })
        ));
    }
}
