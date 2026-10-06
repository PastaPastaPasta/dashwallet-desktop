//! v24 shared masternodes: the codecs of the three new provider payloads and
//! of the share table, written from Dash Core v24.0.0-rc.2
//! `src/evo/providertx.h` because rust-dashcore at the pin has the four
//! classic payloads only:
//!
//! - [`ProDisTx`] (type 10, `providertx.h:436-482`): dissolve a shared
//!   masternode, unilateral (one signature) or unanimous (one per share);
//!   the signatures commit to the transaction through
//!   [`ProDisTx::sign_hash`] (`CProDisTx::MakeSignHash`, `providertx.cpp:506`).
//! - [`ProUpShareTx`] (type 11, `providertx.h:484-515`): change one share's
//!   reward script, signed by that share's owner key over the payload hash
//!   (`CHashSigner::VerifyHashCanonical(::SerializeHash(ptx), …)`,
//!   `specialtxman.cpp:1597-1602`).
//! - [`ProUpSharedRegTx`] (type 12, `providertx.h:517-557`): rotate the
//!   operator/voting keys, one owner signature per share over the payload
//!   hash (`specialtxman.cpp:1653-1660`).
//! - [`CollateralShare`] / [`PayoutShare`]: the share table and payout list
//!   entries (`providertx.h:31-90`).
//!
//! These payloads are consensus-valid only once the v24 fork is active
//! (`GetValidatedPayload`, `specialtxman.cpp:1099-1124`). The shared
//! *registration* (ProRegTx version 3 with the share table) also needs the
//! v24 extended network-info encoding, which this module does not write yet:
//! the engine answers shared-session calls with `NotImplemented` until it
//! does (see docs/contracts/m3-engine.md §2.4).

use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, HashEngine, sha256d};
use dashcore::{OutPoint, ScriptBuf, Transaction, TxOut, Txid};

/// Special transaction types (`primitives/transaction.h:39-41`).
pub const TRANSACTION_PROVIDER_DISSOLVE: u16 = 10;
pub const TRANSACTION_PROVIDER_UPDATE_SHARE: u16 = 11;
pub const TRANSACTION_PROVIDER_UPDATE_SHARED_REGISTRAR: u16 = 12;

/// `CompactSignature`: 65 raw bytes, no length prefix.
pub type CompactSignature = [u8; 65];

/// `CProDisTx::MAX_FEE`: the consensus ceiling on a dissolution's fee.
pub const PRODIS_MAX_FEE: u64 = 1_000_000;

fn io_err(e: std::io::Error) -> encode::Error {
    encode::Error::Io(e)
}

fn read_u8<R: std::io::Read + ?Sized>(r: &mut R) -> Result<u8, encode::Error> {
    u8::consensus_decode(r)
}

fn write_sigs<W: std::io::Write + ?Sized>(
    w: &mut W,
    sigs: &[CompactSignature],
) -> Result<usize, std::io::Error> {
    let count =
        u8::try_from(sigs.len()).map_err(|_| std::io::Error::other("more than 255 signatures"))?;
    let mut len = count.consensus_encode(w)?;
    for sig in sigs {
        w.write_all(sig)?;
        len += sig.len();
    }
    Ok(len)
}

fn read_sigs<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Vec<CompactSignature>, encode::Error> {
    let count = read_u8(r)?;
    let mut sigs = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut sig = [0u8; 65];
        r.read_exact(&mut sig).map_err(io_err)?;
        sigs.push(sig);
    }
    Ok(sigs)
}

/// `MasternodePayoutShare`: one owner payout of a version-3 payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutShare {
    pub script: ScriptBuf,
    /// Hundredths of a percent, 100…10000.
    pub reward: u16,
}

impl Encodable for PayoutShare {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        Ok(self.script.consensus_encode(w)? + self.reward.consensus_encode(w)?)
    }
}

impl Decodable for PayoutShare {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        Ok(Self {
            script: ScriptBuf::consensus_decode(r)?,
            reward: u16::consensus_decode(r)?,
        })
    }
}

/// `CCollateralShare`: one participant's part of a shared collateral.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollateralShare {
    /// Duffs, ≥ 100 DASH.
    pub amount: u64,
    pub refund_script: ScriptBuf,
    /// Empty = rewards go to the refund script.
    pub reward_script: ScriptBuf,
    pub owner_key_id: [u8; 20],
}

impl CollateralShare {
    /// `RewardScript()`: where this share's owner rewards go.
    pub fn effective_reward_script(&self) -> &ScriptBuf {
        if self.reward_script.is_empty() {
            &self.refund_script
        } else {
            &self.reward_script
        }
    }
}

impl Encodable for CollateralShare {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        let mut len = (self.amount as i64).consensus_encode(w)?;
        len += self.refund_script.consensus_encode(w)?;
        len += self.reward_script.consensus_encode(w)?;
        w.write_all(&self.owner_key_id)?;
        Ok(len + 20)
    }
}

impl Decodable for CollateralShare {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        let amount = i64::consensus_decode(r)?;
        let amount = u64::try_from(amount)
            .map_err(|_| encode::Error::ParseFailed("negative share amount"))?;
        let refund_script = ScriptBuf::consensus_decode(r)?;
        let reward_script = ScriptBuf::consensus_decode(r)?;
        let mut owner_key_id = [0u8; 20];
        r.read_exact(&mut owner_key_id).map_err(io_err)?;
        Ok(Self {
            amount,
            refund_script,
            reward_script,
            owner_key_id,
        })
    }
}

/// `CProDisTx` (type 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProDisTx {
    pub version: u16,
    pub pro_tx_hash: Txid,
    pub actor_index: u16,
    pub sigs: Vec<CompactSignature>,
}

impl ProDisTx {
    pub const CURRENT_VERSION: u16 = 1;

    /// `CProDisTx::MakeSignHash(tx, sig_count)`: the digest every
    /// dissolution signature commits to (`providertx.cpp:506-533`).
    /// `tx_version` and `tx_type` are the transaction's 16-bit halves.
    pub fn sign_hash(
        &self,
        tx_version: u16,
        lock_time: u32,
        inputs: &[(OutPoint, u32)],
        outputs: &[TxOut],
        sig_count: u8,
    ) -> [u8; 32] {
        let mut engine = sha256d::Hash::engine();
        let tag = "DashSharedMNDissolve";
        // `std::string` serializes with a compact-size length.
        VarInt(tag.len() as u64)
            .consensus_encode(&mut engine)
            .expect("engines don't error");
        engine.input(tag.as_bytes());
        let en = &mut engine;
        self.version
            .consensus_encode(en)
            .expect("engines don't error");
        tx_version
            .consensus_encode(en)
            .expect("engines don't error");
        TRANSACTION_PROVIDER_DISSOLVE
            .consensus_encode(en)
            .expect("engines don't error");
        lock_time.consensus_encode(en).expect("engines don't error");
        for (prevout, _) in inputs {
            prevout.consensus_encode(en).expect("engines don't error");
        }
        for (_, sequence) in inputs {
            sequence.consensus_encode(en).expect("engines don't error");
        }
        for out in outputs {
            out.consensus_encode(en).expect("engines don't error");
        }
        self.pro_tx_hash
            .consensus_encode(en)
            .expect("engines don't error");
        self.actor_index
            .consensus_encode(en)
            .expect("engines don't error");
        sig_count.consensus_encode(en).expect("engines don't error");
        sha256d::Hash::from_engine(engine).to_byte_array()
    }
}

impl Encodable for ProDisTx {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        let mut len = self.version.consensus_encode(w)?;
        len += self.pro_tx_hash.consensus_encode(w)?;
        len += self.actor_index.consensus_encode(w)?;
        len += write_sigs(w, &self.sigs)?;
        Ok(len)
    }
}

impl Decodable for ProDisTx {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        Ok(Self {
            version: u16::consensus_decode(r)?,
            pro_tx_hash: Txid::consensus_decode(r)?,
            actor_index: u16::consensus_decode(r)?,
            sigs: read_sigs(r)?,
        })
    }
}

/// `CProUpShareTx` (type 11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProUpShareTx {
    pub version: u16,
    pub pro_tx_hash: Txid,
    pub share_index: u16,
    pub reward_script: ScriptBuf,
    pub inputs_hash: [u8; 32],
    /// 65-byte compact signature by the share's owner key.
    pub sig: Vec<u8>,
}

impl ProUpShareTx {
    pub const CURRENT_VERSION: u16 = 1;

    fn encode_base<W: std::io::Write + ?Sized>(&self, w: &mut W) -> Result<usize, std::io::Error> {
        let mut len = self.version.consensus_encode(w)?;
        len += self.pro_tx_hash.consensus_encode(w)?;
        len += self.share_index.consensus_encode(w)?;
        len += self.reward_script.consensus_encode(w)?;
        w.write_all(&self.inputs_hash)?;
        Ok(len + 32)
    }

    /// `::SerializeHash(ptx)` (signature excluded): what the share owner
    /// signs.
    pub fn sign_hash(&self) -> [u8; 32] {
        let mut engine = sha256d::Hash::engine();
        self.encode_base(&mut engine).expect("engines don't error");
        sha256d::Hash::from_engine(engine).to_byte_array()
    }
}

impl Encodable for ProUpShareTx {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        Ok(self.encode_base(w)? + self.sig.consensus_encode(w)?)
    }
}

impl Decodable for ProUpShareTx {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        let version = u16::consensus_decode(r)?;
        let pro_tx_hash = Txid::consensus_decode(r)?;
        let share_index = u16::consensus_decode(r)?;
        let reward_script = ScriptBuf::consensus_decode(r)?;
        let mut inputs_hash = [0u8; 32];
        r.read_exact(&mut inputs_hash).map_err(io_err)?;
        let sig = Vec::<u8>::consensus_decode(r)?;
        Ok(Self {
            version,
            pro_tx_hash,
            share_index,
            reward_script,
            inputs_hash,
            sig,
        })
    }
}

/// `CProUpSharedRegTx` (type 12). The operator key is always in the basic
/// serialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProUpSharedRegTx {
    pub version: u16,
    pub pro_tx_hash: Txid,
    pub operator_public_key: [u8; 48],
    pub voting_key_id: [u8; 20],
    pub inputs_hash: [u8; 32],
    pub sigs: Vec<CompactSignature>,
}

impl ProUpSharedRegTx {
    pub const CURRENT_VERSION: u16 = 1;

    fn encode_base<W: std::io::Write + ?Sized>(&self, w: &mut W) -> Result<usize, std::io::Error> {
        let mut len = self.version.consensus_encode(w)?;
        len += self.pro_tx_hash.consensus_encode(w)?;
        w.write_all(&self.operator_public_key)?;
        w.write_all(&self.voting_key_id)?;
        w.write_all(&self.inputs_hash)?;
        Ok(len + 48 + 20 + 32)
    }

    /// `::SerializeHash(ptx)` (signatures excluded): what every share owner
    /// signs.
    pub fn sign_hash(&self) -> [u8; 32] {
        let mut engine = sha256d::Hash::engine();
        self.encode_base(&mut engine).expect("engines don't error");
        sha256d::Hash::from_engine(engine).to_byte_array()
    }
}

impl Encodable for ProUpSharedRegTx {
    fn consensus_encode<W: std::io::Write + ?Sized>(
        &self,
        w: &mut W,
    ) -> Result<usize, std::io::Error> {
        Ok(self.encode_base(w)? + write_sigs(w, &self.sigs)?)
    }
}

impl Decodable for ProUpSharedRegTx {
    fn consensus_decode<R: std::io::Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        let version = u16::consensus_decode(r)?;
        let pro_tx_hash = Txid::consensus_decode(r)?;
        let mut operator_public_key = [0u8; 48];
        r.read_exact(&mut operator_public_key).map_err(io_err)?;
        let mut voting_key_id = [0u8; 20];
        r.read_exact(&mut voting_key_id).map_err(io_err)?;
        let mut inputs_hash = [0u8; 32];
        r.read_exact(&mut inputs_hash).map_err(io_err)?;
        Ok(Self {
            version,
            pro_tx_hash,
            operator_public_key,
            voting_key_id,
            inputs_hash,
            sigs: read_sigs(r)?,
        })
    }
}

/// Serializes a special transaction whose type rust-dashcore does not know:
/// `nVersion | nType << 16`, inputs, outputs, lock time, then the payload
/// with its compact-size length (`CTransaction` serialization).
pub fn encode_special_tx(tx: &Transaction, tx_type: u16, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let header = u32::from(tx.version) | (u32::from(tx_type) << 16);
    header.consensus_encode(&mut out).expect("vec");
    tx.input.consensus_encode(&mut out).expect("vec");
    tx.output.consensus_encode(&mut out).expect("vec");
    tx.lock_time.consensus_encode(&mut out).expect("vec");
    payload.to_vec().consensus_encode(&mut out).expect("vec");
    out
}

/// The consensus bytes of a payload.
pub fn payload_bytes<T: Encodable>(payload: &T) -> Vec<u8> {
    let mut out = Vec::new();
    payload.consensus_encode(&mut out).expect("vec");
    out
}

#[cfg(test)]
#[allow(non_snake_case)]
mod tests {
    use super::*;
    use dashcore::consensus::deserialize;

    fn round_trip<T: Encodable + Decodable + PartialEq + std::fmt::Debug>(v: &T) -> Vec<u8> {
        let bytes = payload_bytes(v);
        let back: T = deserialize(&bytes).unwrap();
        assert_eq!(&back, v);
        bytes
    }

    #[test]
    fn test_QT_127_prodistx_layout_matches_core() {
        let p = ProDisTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([0xAB; 32]),
            actor_index: 2,
            sigs: vec![[0x11; 65]],
        };
        let bytes = round_trip(&p);
        // version(2) + proTxHash(32) + actorIndex(2) + count(1) + 65.
        assert_eq!(bytes.len(), 2 + 32 + 2 + 1 + 65);
        assert_eq!(&bytes[..2], &[1, 0]);
        assert_eq!(bytes[36], 1, "one signature: unilateral");
    }

    #[test]
    fn test_QT_127_proupsharetx_sign_hash_excludes_the_signature() {
        let mut p = ProUpShareTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([1; 32]),
            share_index: 3,
            reward_script: ScriptBuf::from_bytes(vec![0x76, 0xa9, 0x14]),
            inputs_hash: [5; 32],
            sig: vec![],
        };
        let before = p.sign_hash();
        p.sig = vec![7; 65];
        assert_eq!(p.sign_hash(), before);
        let bytes = round_trip(&p);
        assert_eq!(bytes.len(), 2 + 32 + 2 + 1 + 3 + 32 + 1 + 65);
    }

    #[test]
    fn test_QT_127_proupsharedregtx_round_trips() {
        let p = ProUpSharedRegTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([2; 32]),
            operator_public_key: [3; 48],
            voting_key_id: [4; 20],
            inputs_hash: [6; 32],
            sigs: vec![[8; 65], [9; 65]],
        };
        let bytes = round_trip(&p);
        assert_eq!(bytes.len(), 2 + 32 + 48 + 20 + 32 + 1 + 130);
    }

    #[test]
    fn test_QT_126_collateral_share_round_trips_and_defaults_its_reward() {
        let s = CollateralShare {
            amount: 100 * 100_000_000,
            refund_script: ScriptBuf::from_bytes(vec![1, 2]),
            reward_script: ScriptBuf::new(),
            owner_key_id: [7; 20],
        };
        round_trip(&s);
        assert_eq!(s.effective_reward_script(), &s.refund_script);
        round_trip(&PayoutShare {
            script: ScriptBuf::from_bytes(vec![3]),
            reward: 10_000,
        });
    }

    #[test]
    fn test_QT_127_special_tx_header_carries_the_type() {
        let tx = Transaction {
            version: 3,
            lock_time: 0,
            input: vec![],
            output: vec![],
            special_transaction_payload: None,
        };
        let raw = encode_special_tx(&tx, TRANSACTION_PROVIDER_UPDATE_SHARE, &[1, 2, 3]);
        assert_eq!(&raw[..4], &[3, 0, 11, 0]);
        assert_eq!(&raw[raw.len() - 4..], &[3, 1, 2, 3]);
    }

    #[test]
    fn test_QT_127_dissolve_sign_hash_commits_to_the_signature_count() {
        let p = ProDisTx {
            version: 1,
            pro_tx_hash: Txid::from_byte_array([1; 32]),
            actor_index: 0,
            sigs: vec![],
        };
        let inputs = [(OutPoint::default(), 0xffff_ffffu32)];
        let a = p.sign_hash(3, 0, &inputs, &[], 1);
        let b = p.sign_hash(3, 0, &inputs, &[], 2);
        assert_ne!(a, b);
    }
}
