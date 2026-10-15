//! `CGovernanceObject` (Dash Core `src/governance/common.{h,cpp}`): the
//! network serialization of a `govobj` message, the object hash and the
//! proposal / trigger data it carries.
//!
//! Hashes are kept in internal byte order (`[u8; 32]` as on the wire);
//! [`display_hex`] gives the RPC / GUI form (reversed).

use dashcore::OutPoint;
use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, sha256d};

/// Largest `vchData` Core accepts (`MAX_DATA_SIZE`, `governance/object.h`).
pub const MAX_DATA_SIZE: usize = 512;

/// `GovernanceObject` enum of Core (`common.h`).
pub const OBJECT_TYPE_PROPOSAL: i32 = 1;
pub const OBJECT_TYPE_TRIGGER: i32 = 2;

/// One governance object as relayed on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceObject {
    /// Parent object hash; zero for proposals and triggers.
    pub parent_hash: [u8; 32],
    pub revision: i32,
    /// Creation time, UNIX seconds.
    pub time: i64,
    /// Txid of the collateral transaction (internal order); zero for
    /// masternode-signed objects (triggers).
    pub collateral_hash: [u8; 32],
    /// The data: a JSON object's UTF-8 bytes for proposals and triggers.
    pub data: Vec<u8>,
    pub object_type: i32,
    /// Signing masternode's collateral (triggers); null for proposals.
    pub masternode_outpoint: OutPoint,
    /// Operator BLS signature (triggers); empty for proposals.
    pub signature: Vec<u8>,
}

impl GovernanceObject {
    /// A new unsigned proposal object with Core's `gobject prepare` fields
    /// (parent zero, collateral hash zero until the collateral exists).
    pub fn new_proposal(revision: i32, time: i64, data: Vec<u8>) -> Self {
        Self {
            parent_hash: [0; 32],
            revision,
            time,
            collateral_hash: [0; 32],
            data,
            object_type: OBJECT_TYPE_PROPOSAL,
            masternode_outpoint: OutPoint::null(),
            signature: Vec::new(),
        }
    }

    /// Core `Governance::Object::GetHash()`: double SHA-256 of parent,
    /// revision, time, the data as a hex *string*, the masternode outpoint
    /// followed by the dummy `uint8 0` and `uint32 0xffffffff` of the old
    /// `CTxIn` format, and the signature. The collateral hash and the type
    /// are not part of it, so the hash is known before the collateral
    /// transaction (which commits to it) exists.
    pub fn hash(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(160 + self.data.len() * 2);
        buf.extend_from_slice(&self.parent_hash);
        buf.extend_from_slice(&self.revision.to_le_bytes());
        buf.extend_from_slice(&self.time.to_le_bytes());
        hex::encode(&self.data)
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        self.masternode_outpoint
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        buf.push(0);
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        self.signature
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        sha256d::Hash::hash(&buf).to_byte_array()
    }

    /// The `govobj` payload (`SERIALIZE_METHODS(Object)` for the network:
    /// every field, signature last).
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(160 + self.data.len());
        buf.extend_from_slice(&self.parent_hash);
        buf.extend_from_slice(&self.revision.to_le_bytes());
        buf.extend_from_slice(&self.time.to_le_bytes());
        buf.extend_from_slice(&self.collateral_hash);
        self.data
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        buf.extend_from_slice(&self.object_type.to_le_bytes());
        self.masternode_outpoint
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        self.signature
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        buf
    }

    /// Parses a `govobj` payload. Trailing bytes are refused.
    pub fn decode(payload: &[u8]) -> Result<Self, encode::Error> {
        let mut r = payload;
        let parent_hash = <[u8; 32]>::consensus_decode(&mut r)?;
        let revision = i32::consensus_decode(&mut r)?;
        let time = i64::consensus_decode(&mut r)?;
        let collateral_hash = <[u8; 32]>::consensus_decode(&mut r)?;
        let data = read_bytes(&mut r, MAX_DATA_SIZE * 4)?;
        let object_type = i32::consensus_decode(&mut r)?;
        let masternode_outpoint = OutPoint::consensus_decode(&mut r)?;
        let signature = read_bytes(&mut r, 1024)?;
        if !r.is_empty() {
            return Err(encode::Error::ParseFailed("trailing bytes after govobj"));
        }
        Ok(Self {
            parent_hash,
            revision,
            time,
            collateral_hash,
            data,
            object_type,
            masternode_outpoint,
            signature,
        })
    }

    /// The data as text (Core `GetDataAsPlainString`), lossy for non-UTF-8.
    pub fn data_text(&self) -> String {
        String::from_utf8_lossy(&self.data).into_owned()
    }
}

/// A length-prefixed byte vector of at most `max` bytes (the network
/// decoders cap allocations before reading).
fn read_bytes(r: &mut &[u8], max: usize) -> Result<Vec<u8>, encode::Error> {
    let len = VarInt::consensus_decode(r)?.0 as usize;
    if len > max || len > r.len() {
        return Err(encode::Error::ParseFailed("byte vector too long"));
    }
    let (head, rest) = r.split_at(len);
    let out = head.to_vec();
    *r = rest;
    Ok(out)
}

/// A hash in display (RPC) order: the bytes reversed, lowercase hex.
pub fn display_hex(hash: &[u8; 32]) -> String {
    let mut h = *hash;
    h.reverse();
    hex::encode(h)
}

/// Parses a display-order hash (64 hex characters) into internal order.
pub fn parse_display_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out: [u8; 32] = hex::decode(text).ok()?.try_into().ok()?;
    out.reverse();
    Some(out)
}

/// `COutPoint::ToStringShort()`: `txid-n` with the txid in display order.
pub fn outpoint_short(o: &OutPoint) -> String {
    format!("{}-{}", o.txid, o.vout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_refuses_trailing_bytes() {
        let obj = GovernanceObject::new_proposal(1, 1_700_000_000, br#"{"type":1}"#.to_vec());
        let bytes = obj.encode();
        assert_eq!(GovernanceObject::decode(&bytes).unwrap(), obj);
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(GovernanceObject::decode(&longer).is_err());
        assert!(GovernanceObject::decode(&bytes[..bytes.len() - 1]).is_err());
    }

    #[test]
    fn hash_ignores_collateral_and_type() {
        let a = GovernanceObject::new_proposal(1, 5, b"{}".to_vec());
        let mut b = a.clone();
        b.collateral_hash = [7; 32];
        b.object_type = OBJECT_TYPE_TRIGGER;
        assert_eq!(a.hash(), b.hash());
        b.time = 6;
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn display_hex_reverses() {
        let mut h = [0u8; 32];
        h[0] = 0xab;
        let s = display_hex(&h);
        assert!(s.ends_with("ab"));
        assert_eq!(parse_display_hex(&s), Some(h));
        assert_eq!(parse_display_hex("zz"), None);
    }
}
