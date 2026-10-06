//! `CGovernanceVote` (Dash Core `src/governance/vote.{h,cpp}`): the
//! `govobjvote` serialization, the vote hash, what a voting key signs, and
//! recovering the signer's key id from a compact signature.
//!
//! Signing (Core `CGovernanceVote::CheckSignature`): on testnet the 65-byte
//! compact signature is over the vote's serialization hash
//! (`CHashSigner`); on every other network it is a Dash signed message
//! (`CMessageSigner`, "DarkCoin Signed Message:\n") of
//! `"<txid>-<n>|<parent>|<signal>|<outcome>|<time>"`. BLS (96-byte)
//! signatures are the operator-key form used for trigger votes.

use dashcore::Network;
use dashcore::OutPoint;
use dashcore::consensus::encode::{self, Decodable, Encodable, VarInt};
use dashcore::hashes::{Hash, hash160, sha256d};
use dashcore::secp256k1::ecdsa::{RecoverableSignature, RecoveryId};
use dashcore::secp256k1::{Message, PublicKey, Secp256k1};

use crate::object::{display_hex, outpoint_short};

/// Size of an ECDSA compact signature (`CPubKey::COMPACT_SIGNATURE_SIZE`).
pub const COMPACT_SIG_SIZE: usize = 65;
/// Size of a BLS signature (`CBLSSignature::SerSize`).
pub const BLS_SIG_SIZE: usize = 96;

/// `vote_outcome_enum_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VoteOutcome {
    Yes,
    No,
    Abstain,
}

impl VoteOutcome {
    pub fn code(self) -> i32 {
        match self {
            Self::Yes => 1,
            Self::No => 2,
            Self::Abstain => 3,
        }
    }

    pub fn from_code(code: i32) -> Option<Self> {
        match code {
            1 => Some(Self::Yes),
            2 => Some(Self::No),
            3 => Some(Self::Abstain),
            _ => None,
        }
    }

    /// Core `ConvertOutcomeToString`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "yes",
            Self::No => "no",
            Self::Abstain => "abstain",
        }
    }
}

/// `VOTE_SIGNAL_FUNDING`: the only signal the wallet casts (dash-qt) and
/// the one tallies count.
pub const SIGNAL_FUNDING: i32 = 1;

/// One governance vote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GovernanceVote {
    /// The voting masternode's collateral.
    pub masternode_outpoint: OutPoint,
    /// The voted object's hash, internal order.
    pub parent_hash: [u8; 32],
    pub outcome: i32,
    pub signal: i32,
    /// UNIX seconds.
    pub time: i64,
    pub signature: Vec<u8>,
}

impl GovernanceVote {
    /// An unsigned funding vote.
    pub fn funding(
        masternode_outpoint: OutPoint,
        parent_hash: [u8; 32],
        outcome: VoteOutcome,
        time: i64,
    ) -> Self {
        Self {
            masternode_outpoint,
            parent_hash,
            outcome: outcome.code(),
            signal: SIGNAL_FUNDING,
            time,
            signature: Vec::new(),
        }
    }

    pub fn vote_outcome(&self) -> Option<VoteOutcome> {
        VoteOutcome::from_code(self.outcome)
    }

    /// Core `CGovernanceVote::UpdateHash`: outpoint with the old `CTxIn`
    /// dummy fields, parent, signal, outcome, time. The signature is not
    /// part of it.
    pub fn hash(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(96);
        self.masternode_outpoint
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        buf.push(0);
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        buf.extend_from_slice(&self.parent_hash);
        buf.extend_from_slice(&self.signal.to_le_bytes());
        buf.extend_from_slice(&self.outcome.to_le_bytes());
        buf.extend_from_slice(&self.time.to_le_bytes());
        sha256d::Hash::hash(&buf).to_byte_array()
    }

    /// Core `GetSignatureHash()`: the hash of the serialization without the
    /// signature (`SER_GETHASH`).
    pub fn signature_hash(&self) -> [u8; 32] {
        let mut buf = Vec::with_capacity(84);
        self.encode_unsigned(&mut buf);
        sha256d::Hash::hash(&buf).to_byte_array()
    }

    /// Core `GetSignatureString()`.
    pub fn signature_string(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}",
            outpoint_short(&self.masternode_outpoint),
            display_hex(&self.parent_hash),
            self.signal,
            self.outcome,
            self.time
        )
    }

    /// The 32-byte digest the compact signature commits to on `network`.
    pub fn signing_digest(&self, network: Network) -> [u8; 32] {
        if network == Network::Testnet {
            self.signature_hash()
        } else {
            dw_message::message_hash(self.signature_string().as_bytes())
        }
    }

    /// The hash160 of the key that made the compact signature, or `None`
    /// for a BLS signature or one that recovers no key. A vote is valid for
    /// a masternode when this equals the masternode's voting key id (Core
    /// `CHashSigner::VerifyHash` / `CMessageSigner::VerifyMessage`).
    pub fn signer_key_id(&self, network: Network) -> Option<[u8; 20]> {
        let pubkey = recover_compact(&self.signature, &self.signing_digest(network))?;
        Some(hash160::Hash::hash(&pubkey).to_byte_array())
    }

    fn encode_unsigned(&self, buf: &mut Vec<u8>) {
        self.masternode_outpoint
            .consensus_encode(buf)
            .expect("writing to a Vec cannot fail");
        buf.extend_from_slice(&self.parent_hash);
        buf.extend_from_slice(&self.outcome.to_le_bytes());
        buf.extend_from_slice(&self.signal.to_le_bytes());
        buf.extend_from_slice(&self.time.to_le_bytes());
    }

    /// The `govobjvote` payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(84 + 1 + self.signature.len());
        self.encode_unsigned(&mut buf);
        self.signature
            .consensus_encode(&mut buf)
            .expect("writing to a Vec cannot fail");
        buf
    }

    /// Parses a `govobjvote` payload. Like Core's network reader, only a
    /// 65-byte compact or 96-byte BLS signature is accepted.
    pub fn decode(payload: &[u8]) -> Result<Self, encode::Error> {
        let mut r = payload;
        let masternode_outpoint = OutPoint::consensus_decode(&mut r)?;
        let parent_hash = <[u8; 32]>::consensus_decode(&mut r)?;
        let outcome = i32::consensus_decode(&mut r)?;
        let signal = i32::consensus_decode(&mut r)?;
        let time = i64::consensus_decode(&mut r)?;
        let len = VarInt::consensus_decode(&mut r)?.0 as usize;
        if len != COMPACT_SIG_SIZE && len != BLS_SIG_SIZE {
            return Err(encode::Error::ParseFailed("bad governance vote signature size"));
        }
        if r.len() != len {
            return Err(encode::Error::ParseFailed("governance vote length mismatch"));
        }
        Ok(Self {
            masternode_outpoint,
            parent_hash,
            outcome,
            signal,
            time,
            signature: r.to_vec(),
        })
    }
}

/// The serialized public key that made the 65-byte compact `sig` over
/// `digest` (compressed or not, as the header byte says).
pub fn recover_compact(sig: &[u8], digest: &[u8; 32]) -> Option<Vec<u8>> {
    if sig.len() != COMPACT_SIG_SIZE {
        return None;
    }
    let header = sig[0];
    if !(27..=34).contains(&header) {
        return None;
    }
    let compressed = header >= 31;
    let recid = RecoveryId::try_from(i32::from((header - 27) & 3)).ok()?;
    let rsig = RecoverableSignature::from_compact(&sig[1..], recid).ok()?;
    let secp = Secp256k1::verification_only();
    let pk = secp
        .recover_ecdsa(&Message::from_digest(*digest), &rsig)
        .ok()?;
    Some(if compressed {
        pk.serialize().to_vec()
    } else {
        pk.serialize_uncompressed().to_vec()
    })
}

/// Turns a 64-byte compact ECDSA signature by `pubkey` over `digest` into
/// Core's 65-byte recoverable form (header `31 + recid`, compressed key):
/// the recovery id is the one that recovers `pubkey`.
pub fn to_recoverable_compact(
    compact64: &[u8; 64],
    pubkey: &PublicKey,
    digest: &[u8; 32],
) -> Option<[u8; COMPACT_SIG_SIZE]> {
    let secp = Secp256k1::verification_only();
    let msg = Message::from_digest(*digest);
    for id in 0..4 {
        let recid = RecoveryId::try_from(id).ok()?;
        let Ok(rsig) = RecoverableSignature::from_compact(compact64, recid) else {
            continue;
        };
        if secp.recover_ecdsa(&msg, &rsig).ok().as_ref() == Some(pubkey) {
            let mut out = [0u8; COMPACT_SIG_SIZE];
            out[0] = 31 + id as u8;
            out[1..].copy_from_slice(compact64);
            return Some(out);
        }
    }
    None
}

/// The vote's object signal and outcome as Core's
/// `gobject getcurrentvotes` prints them (`outpoint:time:outcome:signal`).
pub fn current_vote_string(vote: &GovernanceVote) -> String {
    let outcome = vote.vote_outcome().map(VoteOutcome::as_str).unwrap_or("none");
    let signal = match vote.signal {
        1 => "funding",
        2 => "valid",
        3 => "delete",
        4 => "endorsed",
        _ => "none",
    };
    format!(
        "{}:{}:{}:{}",
        outpoint_short(&vote.masternode_outpoint),
        vote.time,
        outcome,
        signal
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use dashcore::secp256k1::SecretKey;

    fn sample() -> GovernanceVote {
        let txid = "f3d0bbd2e2c0e21a6a3a44a0f4b4c2fb4a2a6f1d1b4b9f6cfc2e3e2d1c0b0a09"
            .parse()
            .unwrap();
        GovernanceVote::funding(OutPoint::new(txid, 1), [3; 32], VoteOutcome::Yes, 1_700_000_000)
    }

    fn sign(vote: &mut GovernanceVote, network: Network, sk: &SecretKey) {
        let secp = Secp256k1::new();
        let digest = vote.signing_digest(network);
        let sig = secp.sign_ecdsa(&Message::from_digest(digest), sk);
        let pk = PublicKey::from_secret_key(&secp, sk);
        let full = to_recoverable_compact(&sig.serialize_compact(), &pk, &digest).unwrap();
        vote.signature = full.to_vec();
    }

    #[test]
    fn signer_key_id_recovers_the_voting_key_on_each_network() {
        let sk = SecretKey::from_slice(&[9; 32]).unwrap();
        let secp = Secp256k1::new();
        let pk = PublicKey::from_secret_key(&secp, &sk);
        let id = hash160::Hash::hash(&pk.serialize()).to_byte_array();
        for network in [Network::Mainnet, Network::Testnet, Network::Regtest] {
            let mut v = sample();
            sign(&mut v, network, &sk);
            assert_eq!(v.signer_key_id(network), Some(id), "{network:?}");
            // The digest differs between testnet and the others.
            let other = if network == Network::Testnet {
                Network::Mainnet
            } else {
                Network::Testnet
            };
            assert_ne!(v.signer_key_id(other), Some(id));
        }
    }

    #[test]
    fn encode_decode_and_hash_exclude_signature() {
        let mut v = sample();
        v.signature = vec![31; 65];
        let bytes = v.encode();
        assert_eq!(GovernanceVote::decode(&bytes).unwrap(), v);
        let h = v.hash();
        v.signature = vec![32; 65];
        assert_eq!(v.hash(), h);
        assert_eq!(v.signature_hash(), {
            let mut u = v.clone();
            u.signature.clear();
            u.signature_hash()
        });
        v.signature = vec![1; 10];
        assert!(GovernanceVote::decode(&v.encode()).is_err());
    }

    #[test]
    fn signature_string_matches_core_layout() {
        let v = sample();
        let s = v.signature_string();
        assert!(s.starts_with(
            "f3d0bbd2e2c0e21a6a3a44a0f4b4c2fb4a2a6f1d1b4b9f6cfc2e3e2d1c0b0a09-1|"
        ));
        assert!(s.ends_with("|1|1|1700000000"));
    }
}
